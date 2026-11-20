use std::collections::VecDeque;
use std::sync::Arc;

use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::dsp::scope_dsp::{compute_fft_dbv, format_freq, format_time_us};
use crate::protocol::types::{PersistenceMode, ScopeFrame, CHANNEL_COLORS, CHANNEL_NAMES};

#[derive(Default)]
struct ScratchSpline {
    xs: Vec<f64>,
    ys: Vec<f64>,
    secants: Vec<f64>,
    tangents: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragTarget {
    None,
    CursorA,
    CursorB,
    Pan,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PhosphorKey {
    width: u32,
    height: u32,
    zoom_bits: u64,
    pan_bits: u64,
    fft_on: bool,
    atten_code: u8,
    sample_rate_hz: u32,
    samples_per_ch: usize,
    clear_ver: u64,
}

#[component]
#[allow(clippy::too_many_arguments)]
pub fn ScopeCanvas(
    latest_frame: RwSignal<Option<Arc<ScopeFrame>>>,
    frame_version: RwSignal<u64>,
    trig_level_mv: RwSignal<u16>,
    trig_channel: RwSignal<u8>,
    show_fft: RwSignal<bool>,
    fft_channel: RwSignal<u8>,
    show_cursors: RwSignal<bool>,
    cursor_a_norm: RwSignal<f64>,
    cursor_b_norm: RwSignal<f64>,
    zoom: RwSignal<f64>,
    pan_norm: RwSignal<f64>,
    persistence_mode: RwSignal<PersistenceMode>,
    persist_clear_ver: RwSignal<u64>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<html::Canvas>::new();
    let drag_target = StoredValue::new(DragTarget::None);
    let drag_start_x = StoredValue::new(0.0f64);
    let drag_start_pan = StoredValue::new(0.0f64);

    // Persistence & zero-allocation rendering state
    let history = StoredValue::new_local(VecDeque::<Arc<ScopeFrame>>::with_capacity(64));
    let scratch = StoredValue::new_local(ScratchSpline::default());
    let last_recorded_ver = StoredValue::new(0u64);
    let last_clear_ver = StoredValue::new(0u64);
    let last_mode = StoredValue::new(PersistenceMode::Off);
    let phosphor_canvas = StoredValue::new_local(None::<web_sys::HtmlCanvasElement>);
    let phosphor_key = StoredValue::new(None::<PhosphorKey>);
    let resize_ver = RwSignal::new(0u64);
    let raf_pending = StoredValue::new(false);

    // Trigger canvas redraw when the browser window is resized (important when paused/single-shot)
    if let Some(win) = web_sys::window() {
        let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
            resize_ver.update(|v| *v = v.wrapping_add(1));
        });
        let _ = win.add_event_listener_with_callback("resize", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    let render_now = move || {
        let ver = frame_version.get_untracked();
        let frame_opt = latest_frame.get_untracked();
        let trig_mv = trig_level_mv.get_untracked();
        let trig_ch = trig_channel.get_untracked();
        let fft_on = show_fft.get_untracked();
        let fft_ch = fft_channel.get_untracked() as usize;
        let cursors_on = show_cursors.get_untracked();
        let ca = cursor_a_norm.get_untracked();
        let cb = cursor_b_norm.get_untracked();
        let z = zoom.get_untracked().max(1.0);
        let pan = pan_norm.get_untracked();
        let persist = persistence_mode.get_untracked();
        let clear_v = persist_clear_ver.get_untracked();

        // Clear persistence buffers on explicit clear or mode change
        if clear_v != last_clear_ver.get_value() || persist != last_mode.get_value() {
            last_clear_ver.set_value(clear_v);
            last_mode.set_value(persist);
            history.update_value(|h| h.clear());
            phosphor_key.set_value(None);
        }

        let is_new_frame = ver != last_recorded_ver.get_value();
        if is_new_frame {
            last_recorded_ver.set_value(ver);
        }

        let Some(canvas) = canvas_ref.get() else {
            return;
        };

        let rect = canvas.get_bounding_client_rect();
        let css_w = rect.width().max(320.0);
        let css_h = rect.height().max(260.0);
        let dpr = web_sys::window()
            .map(|w| w.device_pixel_ratio().clamp(1.0, 2.5))
            .unwrap_or(1.0);

        let target_w = (css_w * dpr).round() as u32;
        let target_h = (css_h * dpr).round() as u32;
        if canvas.width() != target_w || canvas.height() != target_h {
            canvas.set_width(target_w);
            canvas.set_height(target_h);
        }

        let Ok(Some(ctx_obj)) = canvas.get_context("2d") else {
            return;
        };
        let Ok(ctx) = ctx_obj.dyn_into::<web_sys::CanvasRenderingContext2d>() else {
            return;
        };

        let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);

        // Background
        ctx.set_fill_style_str("#0a0f17");
        ctx.fill_rect(0.0, 0.0, css_w, css_h);

        let pad_l = 52.0;
        let pad_r = 16.0;
        let pad_t = 22.0;
        let pad_b = 26.0;

        let scope_h = if fft_on {
            (css_h - pad_t - pad_b - 28.0) * 0.64
        } else {
            css_h - pad_t - pad_b
        };
        let plot_w = (css_w - pad_l - pad_r).max(100.0);

        let vref_mv = frame_opt
            .as_ref()
            .map(|f| f.vref_mv as f64)
            .unwrap_or(3100.0);

        // Draw oscilloscope graticule (10 x 8)
        draw_graticule(&ctx, pad_l, pad_t, plot_w, scope_h, vref_mv);

        if let Some(frame) = frame_opt.as_ref() {
            let n = frame.samples_per_ch;
            let span = 1.0 / z;
            let start_norm = pan.clamp(0.0, (1.0 - span).max(0.0));
            let end_norm = (start_norm + span).min(1.0);

            // Trigger time vertical marker
            let trig_norm = (frame.trigger_index as f64) / (n.saturating_sub(1).max(1) as f64);
            if trig_norm >= start_norm && trig_norm <= end_norm {
                let tx = pad_l + ((trig_norm - start_norm) / span) * plot_w;
                ctx.save();
                ctx.set_stroke_style_str("rgba(249, 115, 22, 0.45)");
                ctx.set_line_width(1.0);
                let _ = ctx.set_line_dash(&js_sys::Array::of2(&4.into(), &4.into()));
                ctx.begin_path();
                ctx.move_to(tx, pad_t);
                ctx.line_to(tx, pad_t + scope_h);
                ctx.stroke();
                ctx.restore();

                // Orange trigger triangle at top
                ctx.set_fill_style_str("#f97316");
                ctx.begin_path();
                ctx.move_to(tx, pad_t + 6.0);
                ctx.line_to(tx - 5.0, pad_t - 2.0);
                ctx.line_to(tx + 5.0, pad_t - 2.0);
                ctx.close_path();
                ctx.fill();
            }

            // Trigger level horizontal dashed line
            let ty = pad_t + scope_h * (1.0 - (trig_mv as f64 / vref_mv).clamp(0.0, 1.0));
            ctx.save();
            ctx.set_stroke_style_str("rgba(249, 115, 22, 0.55)");
            ctx.set_line_width(1.0);
            let _ = ctx.set_line_dash(&js_sys::Array::of2(&5.into(), &5.into()));
            ctx.begin_path();
            ctx.move_to(pad_l, ty);
            ctx.line_to(pad_l + plot_w, ty);
            ctx.stroke();
            ctx.restore();

            ctx.set_fill_style_str("#f97316");
            ctx.set_font("600 10px ui-monospace, SFMono-Regular, Menlo, monospace");
            let label_y = (ty - 4.0).clamp(pad_t + 11.0, pad_t + scope_h - 3.0);
            let _ = ctx.fill_text(&format!("T{}", trig_ch + 1), pad_l + 4.0, label_y);

            // --- Persistence Layer ---
            if let Some(decay_ms) = persist.decay_ms() {
                if is_new_frame {
                    history.update_value(|h| {
                        h.push_back(frame.clone());
                        while h.len() > 24 {
                            h.pop_front();
                        }
                    });
                }
                let now_ts = frame.timestamp_ms;
                history.update_value(|h| {
                    while let Some(front) = h.front() {
                        if now_ts - front.timestamp_ms > decay_ms {
                            h.pop_front();
                        } else {
                            break;
                        }
                    }
                });

                // Render historical decaying frames (oldest -> newest, excluding the very latest)
                scratch.update_value(|sc| {
                    history.with_value(|h| {
                        let count = h.len();
                        for (idx, old_frame) in h.iter().enumerate() {
                            if idx + 1 == count {
                                break;
                            }
                            let age = (now_ts - old_frame.timestamp_ms).clamp(0.0, decay_ms);
                            let frac = (1.0 - age / decay_ms).clamp(0.0, 1.0);
                            let alpha = frac.powf(1.6) * 0.52;
                            if alpha > 0.02 {
                                draw_frame_channels(
                                    &ctx,
                                    old_frame,
                                    sc,
                                    pad_l,
                                    pad_t,
                                    plot_w,
                                    scope_h,
                                    start_norm,
                                    end_norm,
                                    span,
                                    alpha,
                                    1.2,
                                    false,
                                );
                            }
                        }
                    });
                });
            } else if persist == PersistenceMode::Infinite {
                let desired_key = PhosphorKey {
                    width: target_w,
                    height: target_h,
                    zoom_bits: z.to_bits(),
                    pan_bits: start_norm.to_bits(),
                    fft_on,
                    atten_code: frame.atten_code,
                    sample_rate_hz: frame.sample_rate_hz,
                    samples_per_ch: frame.samples_per_ch,
                    clear_ver: clear_v,
                };

                let needs_reset = phosphor_key.get_value() != Some(desired_key);
                let offscreen = ensure_phosphor_canvas(&phosphor_canvas, target_w, target_h);

                if let Some(p_canvas) = offscreen {
                    if let Ok(Some(p_ctx_obj)) = p_canvas.get_context("2d") {
                        if let Ok(p_ctx) = p_ctx_obj.dyn_into::<web_sys::CanvasRenderingContext2d>()
                        {
                            let _ = p_ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
                            if needs_reset {
                                p_ctx.clear_rect(0.0, 0.0, css_w, css_h);
                                phosphor_key.set_value(Some(desired_key));
                            }
                            if is_new_frame || needs_reset {
                                scratch.update_value(|sc| {
                                    draw_frame_channels(
                                        &p_ctx,
                                        frame,
                                        sc,
                                        pad_l,
                                        pad_t,
                                        plot_w,
                                        scope_h,
                                        start_norm,
                                        end_norm,
                                        span,
                                        0.24,
                                        1.2,
                                        false,
                                    );
                                });
                            }
                        }
                    }

                    // Blit accumulated infinite phosphor layer behind live trace
                    ctx.save();
                    let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
                    let _ = ctx.draw_image_with_html_canvas_element(&p_canvas, 0.0, 0.0);
                    ctx.restore();
                }
            }

            // Draw current live waveform on top at full brightness
            scratch.update_value(|sc| {
                draw_frame_channels(
                    &ctx,
                    frame,
                    sc,
                    pad_l,
                    pad_t,
                    plot_w,
                    scope_h,
                    start_norm,
                    end_norm,
                    span,
                    1.0,
                    1.8,
                    true,
                );
            });

            // Time axis labels
            let total_us = (n as f64) / (frame.sample_rate_hz as f64) * 1_000_000.0;
            let start_us = start_norm * total_us;
            let visible_us = span * total_us;
            ctx.set_fill_style_str("#8b949e");
            ctx.set_font("10px ui-monospace, SFMono-Regular, Menlo, monospace");
            for d in 0..=5 {
                let frac = (d as f64) / 5.0;
                let x = pad_l + frac * plot_w;
                let t_us = start_us + frac * visible_us;
                let _ = ctx.fill_text(&format_time_us(t_us as f32), x - 14.0, pad_t + scope_h + 14.0);
            }

            // Status HUD top-left (Persistence badge)
            if let Some(badge) = persist.badge() {
                ctx.set_fill_style_str("#38bdf8");
                ctx.set_font("700 10px ui-monospace, SFMono-Regular, Menlo, monospace");
                let _ = ctx.fill_text(badge, pad_l + 2.0, pad_t - 6.0);
            }

            // Status HUD top-right
            let trig_badge = if frame.triggered {
                "TRIG'D"
            } else {
                "AUTO / FREE"
            };
            ctx.set_fill_style_str(if frame.triggered { "#4ade80" } else { "#fbbf24" });
            ctx.set_font("700 11px ui-monospace, SFMono-Regular, Menlo, monospace");
            let _ = ctx.fill_text(trig_badge, pad_l + plot_w - 72.0, pad_t - 6.0);

            if frame.adc_saturated {
                ctx.set_fill_style_str("#f87171");
                let _ = ctx.fill_text("CLIP!", pad_l + plot_w - 122.0, pad_t - 6.0);
            }

            // A/B Cursors
            if cursors_on {
                let ax = pad_l + ca.clamp(0.0, 1.0) * plot_w;
                let bx = pad_l + cb.clamp(0.0, 1.0) * plot_w;
                draw_cursor(&ctx, ax, pad_t, scope_h, "#38bdf8", "A");
                draw_cursor(&ctx, bx, pad_t, scope_h, "#f472b6", "B");
            }

            // Optional FFT Lower Pane
            if fft_on {
                let fft_top = pad_t + scope_h + 30.0;
                let fft_h = (css_h - fft_top - 22.0).max(60.0);
                let chosen = if !frame.channels_mv[fft_ch].is_empty() {
                    fft_ch
                } else {
                    (0..4).find(|&c| !frame.channels_mv[c].is_empty()).unwrap_or(0)
                };
                draw_fft_pane(
                    &ctx,
                    pad_l,
                    fft_top,
                    plot_w,
                    fft_h,
                    &frame.channels_mv[chosen],
                    frame.sample_rate_hz,
                    CHANNEL_COLORS[chosen],
                    CHANNEL_NAMES[chosen],
                );
            }
        } else {
            ctx.set_fill_style_str("#6e7681");
            ctx.set_font("500 14px system-ui, -apple-system, sans-serif");
            let _ = ctx.fill_text(
                "No waveform captured — click Demo or Connect USB",
                pad_l + plot_w * 0.5 - 155.0,
                pad_t + scope_h * 0.5,
            );
        }
    };

    // Coalesce redraws onto the browser's requestAnimationFrame cadence (max 1 paint per vsync)
    Effect::new(move |_| {
        let _ = resize_ver.get();
        let _ = frame_version.get();
        let _ = latest_frame.with(|f| f.is_some());
        let _ = trig_level_mv.get();
        let _ = trig_channel.get();
        let _ = show_fft.get();
        let _ = fft_channel.get();
        let _ = show_cursors.get();
        let _ = cursor_a_norm.get();
        let _ = cursor_b_norm.get();
        let _ = zoom.get();
        let _ = pan_norm.get();
        let _ = persistence_mode.get();
        let _ = persist_clear_ver.get();

        if raf_pending.get_value() {
            return;
        }
        raf_pending.set_value(true);
        if let Some(win) = web_sys::window() {
            let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
                raf_pending.set_value(false);
                render_now();
            });
            let _ = win.request_animation_frame(cb.unchecked_ref());
        } else {
            raf_pending.set_value(false);
            render_now();
        }
    });

    let on_wheel = move |ev: web_sys::WheelEvent| {
        ev.prevent_default();
        let factor = if ev.delta_y() < 0.0 { 1.25 } else { 0.8 };
        let old_z = zoom.get_untracked().max(1.0);
        let new_z = (old_z * factor).clamp(1.0, 32.0);
        if (new_z - old_z).abs() > 1e-4 {
            let old_span = 1.0 / old_z;
            let new_span = 1.0 / new_z;
            let center = pan_norm.get_untracked() + old_span * 0.5;
            zoom.set(new_z);
            pan_norm.set((center - new_span * 0.5).clamp(0.0, (1.0 - new_span).max(0.0)));
        }
    };

    let on_pointer_down = move |ev: web_sys::PointerEvent| {
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
        let rect = canvas.get_bounding_client_rect();
        let x = ev.client_x() as f64 - rect.left();
        let pad_l = 52.0;
        let plot_w = (rect.width() - pad_l - 16.0).max(100.0);
        let norm_x = ((x - pad_l) / plot_w).clamp(0.0, 1.0);

        if show_cursors.get_untracked() {
            let da = (norm_x - cursor_a_norm.get_untracked()).abs() * plot_w;
            let db = (norm_x - cursor_b_norm.get_untracked()).abs() * plot_w;
            if da < 14.0 && da <= db {
                drag_target.set_value(DragTarget::CursorA);
                return;
            }
            if db < 14.0 {
                drag_target.set_value(DragTarget::CursorB);
                return;
            }
        }
        drag_target.set_value(DragTarget::Pan);
        drag_start_x.set_value(x);
        drag_start_pan.set_value(pan_norm.get_untracked());
    };

    let on_pointer_move = move |ev: web_sys::PointerEvent| {
        let target = drag_target.get_value();
        if target == DragTarget::None {
            return;
        }
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
        let rect = canvas.get_bounding_client_rect();
        let x = ev.client_x() as f64 - rect.left();
        let pad_l = 52.0;
        let plot_w = (rect.width() - pad_l - 16.0).max(100.0);
        let norm_x = ((x - pad_l) / plot_w).clamp(0.0, 1.0);

        match target {
            DragTarget::CursorA => cursor_a_norm.set(norm_x),
            DragTarget::CursorB => cursor_b_norm.set(norm_x),
            DragTarget::Pan => {
                let z = zoom.get_untracked().max(1.0);
                let span = 1.0 / z;
                let dx_norm = (x - drag_start_x.get_value()) / plot_w * span;
                pan_norm.set((drag_start_pan.get_value() - dx_norm).clamp(0.0, (1.0 - span).max(0.0)));
            }
            DragTarget::None => {}
        }
    };

    let on_pointer_up = move |_ev: web_sys::PointerEvent| {
        drag_target.set_value(DragTarget::None);
    };

    view! {
        <div class="scope-canvas-wrap">
            <canvas
                node_ref=canvas_ref
                class="scope-canvas"
                on:wheel=on_wheel
                on:pointerdown=on_pointer_down
                on:pointermove=on_pointer_move
                on:pointerup=on_pointer_up
                on:pointerleave=on_pointer_up
            />
        </div>
    }
}

fn ensure_phosphor_canvas(
    slot: &StoredValue<Option<web_sys::HtmlCanvasElement>, LocalStorage>,
    width: u32,
    height: u32,
) -> Option<web_sys::HtmlCanvasElement> {
    let existing = slot.get_value();
    let canvas = match existing {
        Some(c) => c,
        None => {
            let doc = web_sys::window()?.document()?;
            let el = doc.create_element("canvas").ok()?;
            let c = el.dyn_into::<web_sys::HtmlCanvasElement>().ok()?;
            slot.set_value(Some(c.clone()));
            c
        }
    };
    if canvas.width() != width || canvas.height() != height {
        canvas.set_width(width);
        canvas.set_height(height);
    }
    Some(canvas)
}

#[allow(clippy::too_many_arguments)]
fn draw_frame_channels(
    ctx: &web_sys::CanvasRenderingContext2d,
    frame: &ScopeFrame,
    sc: &mut ScratchSpline,
    pad_l: f64,
    pad_t: f64,
    plot_w: f64,
    scope_h: f64,
    start_norm: f64,
    end_norm: f64,
    span: f64,
    alpha: f64,
    line_width: f64,
    show_sample_dots: bool,
) {
    let n = frame.samples_per_ch;
    if n < 2 {
        return;
    }
    let vref_mv = (frame.vref_mv as f64).max(500.0);
    let denom_n = (n - 1) as f64;
    let start_idx = ((start_norm * denom_n).floor() as usize).min(n - 1);
    let end_idx = ((end_norm * denom_n).ceil() as usize)
        .max(start_idx + 1)
        .min(n);
    let draw_start = start_idx.saturating_sub(1);
    let draw_end = (end_idx + 1).min(n);
    let visible_count = (end_idx - start_idx).max(1);
    let dx_per_sample = plot_w / (visible_count as f64);
    // Use smooth cubic Hermite splines only when zoomed in so points are visibly spaced apart
    // (at 1x zoom, lineTo is pixel-identical and 20x faster, keeping the GUI silky responsive)
    let use_spline = if show_sample_dots {
        dx_per_sample >= 4.0 && visible_count <= 450 && draw_end > draw_start + 1
    } else {
        dx_per_sample >= 6.0 && visible_count <= 250 && draw_end > draw_start + 1
    };

    ctx.save();
    ctx.set_global_alpha(alpha);
    ctx.begin_path();
    ctx.rect(pad_l, pad_t, plot_w, scope_h);
    ctx.clip();
    ctx.set_line_width(line_width);
    ctx.set_line_join("round");
    ctx.set_line_cap("round");

    for ch in 0..4usize {
        let samples = &frame.channels_mv[ch];
        if samples.len() < draw_end {
            continue;
        }
        let color = CHANNEL_COLORS[ch];
        ctx.set_stroke_style_str(color);

        if use_spline {
            // Monotone cubic Hermite spline (Fritsch-Carlson) using reusable scratch buffers (0 allocs)
            sc.xs.clear();
            sc.ys.clear();
            for (idx, &mv) in samples[draw_start..draw_end].iter().enumerate() {
                let i = draw_start + idx;
                let norm_i = (i as f64) / denom_n;
                let x = pad_l + ((norm_i - start_norm) / span) * plot_w;
                let y = pad_t + scope_h * (1.0 - ((mv as f64) / vref_mv).clamp(0.0, 1.0));
                sc.xs.push(x);
                sc.ys.push(y);
            }

            let m_cnt = sc.xs.len();
            sc.secants.clear();
            for k in 0..(m_cnt - 1) {
                let dx = (sc.xs[k + 1] - sc.xs[k]).max(1e-6);
                sc.secants.push((sc.ys[k + 1] - sc.ys[k]) / dx);
            }

            sc.tangents.clear();
            sc.tangents.resize(m_cnt, 0.0);
            sc.tangents[0] = sc.secants[0];
            sc.tangents[m_cnt - 1] = sc.secants[m_cnt - 2];
            for k in 1..(m_cnt - 1) {
                let s0 = sc.secants[k - 1];
                let s1 = sc.secants[k];
                if s0 * s1 > 0.0 {
                    sc.tangents[k] = 2.0 * s0 * s1 / (s0 + s1);
                }
            }

            for k in 0..(m_cnt - 1) {
                let sk = sc.secants[k];
                if sk.abs() < 1e-9 {
                    sc.tangents[k] = 0.0;
                    sc.tangents[k + 1] = 0.0;
                } else {
                    let a = sc.tangents[k] / sk;
                    let b = sc.tangents[k + 1] / sk;
                    let h = (a * a + b * b).sqrt();
                    if h > 3.0 {
                        let t = 3.0 / h;
                        sc.tangents[k] = t * a * sk;
                        sc.tangents[k + 1] = t * b * sk;
                    }
                }
            }

            ctx.begin_path();
            ctx.move_to(sc.xs[0], sc.ys[0]);
            for k in 0..(m_cnt - 1) {
                let dx = (sc.xs[k + 1] - sc.xs[k]) / 3.0;
                let cp1x = sc.xs[k] + dx;
                let cp1y = sc.ys[k] + sc.tangents[k] * dx;
                let cp2x = sc.xs[k + 1] - dx;
                let cp2y = sc.ys[k + 1] - sc.tangents[k + 1] * dx;
                ctx.bezier_curve_to(cp1x, cp1y, cp2x, cp2y, sc.xs[k + 1], sc.ys[k + 1]);
            }
            ctx.stroke();

            if show_sample_dots && dx_per_sample >= 10.0 {
                ctx.set_fill_style_str(color);
                for k in 0..m_cnt {
                    ctx.begin_path();
                    let _ = ctx.arc(sc.xs[k], sc.ys[k], 2.4, 0.0, std::f64::consts::TAU);
                    ctx.fill();
                }
            }
        } else {
            ctx.begin_path();
            let step = (visible_count as f64 / (plot_w * 1.5)).floor().max(1.0) as usize;
            let mut first = true;
            let mut i = draw_start;
            while i < draw_end {
                let norm_i = (i as f64) / denom_n;
                let x = pad_l + ((norm_i - start_norm) / span) * plot_w;
                let y = pad_t + scope_h * (1.0 - ((samples[i] as f64) / vref_mv).clamp(0.0, 1.0));
                if first {
                    ctx.move_to(x, y);
                    first = false;
                } else {
                    ctx.line_to(x, y);
                }
                i += step;
            }
            ctx.stroke();
        }
    }

    ctx.restore();
}

fn draw_graticule(
    ctx: &web_sys::CanvasRenderingContext2d,
    x0: f64,
    y0: f64,
    w: f64,
    h: f64,
    vref_mv: f64,
) {
    ctx.set_fill_style_str("#0d131d");
    ctx.fill_rect(x0, y0, w, h);

    ctx.set_stroke_style_str("#1e293b");
    ctx.set_line_width(1.0);

    // 10 vertical divisions
    for i in 0..=10 {
        let x = x0 + (i as f64 / 10.0) * w;
        ctx.begin_path();
        ctx.move_to(x, y0);
        ctx.line_to(x, y0 + h);
        ctx.stroke();
    }

    // 8 horizontal divisions
    ctx.set_fill_style_str("#8b949e");
    ctx.set_font("10px ui-monospace, SFMono-Regular, Menlo, monospace");
    for j in 0..=8 {
        let y = y0 + (j as f64 / 8.0) * h;
        ctx.begin_path();
        ctx.move_to(x0, y);
        ctx.line_to(x0 + w, y);
        ctx.stroke();

        if j % 2 == 0 {
            let v = (vref_mv / 1000.0) * (1.0 - (j as f64 / 8.0));
            let _ = ctx.fill_text(&format!("{v:.2}V"), 6.0, y + 3.5);
        }
    }

    ctx.set_stroke_style_str("#334155");
    ctx.stroke_rect(x0, y0, w, h);
}

fn draw_cursor(
    ctx: &web_sys::CanvasRenderingContext2d,
    x: f64,
    y0: f64,
    h: f64,
    color: &str,
    label: &str,
) {
    ctx.save();
    ctx.set_stroke_style_str(color);
    ctx.set_line_width(1.4);
    let _ = ctx.set_line_dash(&js_sys::Array::of2(&3.into(), &3.into()));
    ctx.begin_path();
    ctx.move_to(x, y0);
    ctx.line_to(x, y0 + h);
    ctx.stroke();
    ctx.restore();

    ctx.set_fill_style_str(color);
    ctx.set_font("700 10px ui-monospace, SFMono-Regular, Menlo, monospace");
    let _ = ctx.fill_text(label, x + 3.0, y0 + 12.0);
}

#[allow(clippy::too_many_arguments)]
fn draw_fft_pane(
    ctx: &web_sys::CanvasRenderingContext2d,
    x0: f64,
    y0: f64,
    w: f64,
    h: f64,
    samples_mv: &[f32],
    sample_rate_hz: u32,
    color: &str,
    ch_name: &str,
) {
    ctx.set_fill_style_str("#0c111a");
    ctx.fill_rect(x0, y0, w, h);
    ctx.set_stroke_style_str("#1e293b");
    ctx.set_line_width(1.0);
    ctx.stroke_rect(x0, y0, w, h);

    let (bin_hz, mags) = compute_fft_dbv(samples_mv, sample_rate_hz);
    if mags.len() < 4 {
        return;
    }

    let min_db = -90.0f64;
    let max_db = 10.0f64;

    // Grid lines
    ctx.set_fill_style_str("#8b949e");
    ctx.set_font("10px ui-monospace, SFMono-Regular, Menlo, monospace");
    for db in [-80.0, -50.0, -20.0, 0.0] {
        let y = y0 + h * (1.0 - (db - min_db) / (max_db - min_db));
        ctx.begin_path();
        ctx.move_to(x0, y);
        ctx.line_to(x0 + w, y);
        ctx.stroke();
        let _ = ctx.fill_text(&format!("{db:.0}dB"), 6.0, y + 3.5);
    }

    // Spectrum trace
    ctx.set_stroke_style_str(color);
    ctx.set_line_width(1.5);
    ctx.begin_path();
    let max_bin = mags.len() - 1;
    let mut peak_bin = 1usize;
    let mut peak_db = -120.0f32;
    for (i, &dbv) in mags.iter().enumerate() {
        if i >= 1 && dbv > peak_db {
            peak_db = dbv;
            peak_bin = i;
        }
        let x = x0 + (i as f64 / max_bin as f64) * w;
        let y = y0 + h * (1.0 - ((dbv as f64 - min_db) / (max_db - min_db)).clamp(0.0, 1.0));
        if i == 0 {
            ctx.move_to(x, y);
        } else {
            ctx.line_to(x, y);
        }
    }
    ctx.stroke();

    // Frequency labels + peak readout
    let nyquist = sample_rate_hz as f32 * 0.5;
    for d in 0..=4 {
        let frac = d as f32 / 4.0;
        let x = x0 + frac as f64 * w;
        let _ = ctx.fill_text(&format_freq(frac * nyquist), x - 12.0, y0 + h + 13.0);
    }

    let peak_freq = peak_bin as f32 * bin_hz;
    ctx.set_fill_style_str(color);
    ctx.set_font("600 10px ui-monospace, SFMono-Regular, Menlo, monospace");
    let _ = ctx.fill_text(
        &format!("FFT {ch_name} · Peak: {} ({peak_db:.1} dBV)", format_freq(peak_freq)),
        x0 + 8.0,
        y0 + 13.0,
    );
}
