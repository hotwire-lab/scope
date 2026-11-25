use std::collections::VecDeque;
use std::sync::Arc;

use hotwire_core::ui::{
    apply_anchored_zoom, apply_horizontal_pan, GestureDelta, PinchPanTracker,
};
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::dsp::scope_dsp::{
    channel_mean_mv, compute_fft_dbv_into, compute_math_samples, divs_to_y, format_freq,
    format_time_us, format_voltage_mv, sample_mv_to_divs, trig_mv_to_y, y_to_divs, y_to_trig_mv,
};
use crate::protocol::codec::resolve_atten_preset;
use crate::protocol::types::{
    ChannelCoupling, ChannelViewConfig, DisplayMode, MathMode, PersistenceMode, ScopeFrame,
    CHANNEL_COLORS, CHANNEL_NAMES, MATH_COLOR, V_PER_DIV_STEPS_MV,
};

#[derive(Default)]
struct ScratchSpline {
    xs: Vec<f64>,
    ys: Vec<f64>,
    secants: Vec<f64>,
    tangents: Vec<f64>,
    math_mv: Vec<f32>,
    fft_re: Vec<f32>,
    fft_im: Vec<f32>,
    fft_mags: Vec<f32>,
    fft_bin_hz: f32,
    fft_cache_ver: u64,
    fft_cache_ch: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragTarget {
    None,
    ChannelPos(usize),
    TriggerLevel,
    CursorA,
    CursorB,
    Pan,
    Pinch,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PhosphorKey {
    width: u32,
    height: u32,
    zoom_bits: u64,
    pan_bits: u64,
    fft_on: bool,
    display_mode: DisplayMode,
    math_mode: MathMode,
    atten_code: u8,
    sample_rate_hz: u32,
    samples_per_ch: usize,
    clear_ver: u64,
    cfgs_hash: u64,
}

fn hash_channel_cfgs(cfgs: &[ChannelViewConfig; 4]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for c in cfgs {
        for b in [
            c.vdiv_idx as u8,
            c.coupling as u8,
            c.invert as u8,
            c.probe_x10 as u8,
        ] {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h ^= c.offset_divs.to_bits() as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

const PAD_L: f64 = 68.0;
const PAD_R: f64 = 34.0;
const PAD_T: f64 = 24.0;
const PAD_B: f64 = 26.0;

#[component]
#[allow(clippy::too_many_arguments)]
pub fn ScopeCanvas(
    latest_frame: RwSignal<Option<Arc<ScopeFrame>>>,
    frame_version: RwSignal<u64>,
    channel_mask: RwSignal<u8>,
    channel_cfgs: RwSignal<[ChannelViewConfig; 4]>,
    selected_ch: RwSignal<usize>,
    atten_code: RwSignal<u8>,
    trig_level_mv: RwSignal<u16>,
    trig_channel: RwSignal<u8>,
    trig_edge: RwSignal<u8>,
    on_apply_trig: Callback<()>,
    show_fft: RwSignal<bool>,
    fft_channel: RwSignal<u8>,
    show_cursors: RwSignal<bool>,
    cursor_a_norm: RwSignal<f64>,
    cursor_b_norm: RwSignal<f64>,
    zoom: RwSignal<f64>,
    pan_norm: RwSignal<f64>,
    persistence_mode: RwSignal<PersistenceMode>,
    persist_clear_ver: RwSignal<u64>,
    math_mode: RwSignal<MathMode>,
    display_mode: RwSignal<DisplayMode>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<html::Canvas>::new();
    let drag_target = StoredValue::new(DragTarget::None);
    let gesture_tracker = StoredValue::new(PinchPanTracker::new());
    let drag_hud = RwSignal::new(None::<String>);

    // Persistence & zero-allocation rendering state
    let history = StoredValue::new_local(VecDeque::<Arc<ScopeFrame>>::with_capacity(28));
    let scratch = StoredValue::new_local(ScratchSpline::default());
    let last_recorded_ver = StoredValue::new(0u64);
    let last_clear_ver = StoredValue::new(0u64);
    let last_mode = StoredValue::new(PersistenceMode::Off);
    let phosphor_canvas = StoredValue::new_local(None::<web_sys::HtmlCanvasElement>);
    let phosphor_key = StoredValue::new(None::<PhosphorKey>);
    let resize_ver = RwSignal::new(0u64);
    let raf_pending = StoredValue::new(false);

    if let Some(win) = web_sys::window() {
        let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
            resize_ver.update(|v| *v = v.wrapping_add(1));
        });
        let _ = win.add_event_listener_with_callback("resize", cb.as_ref().unchecked_ref());
        cb.forget();
    }

    let calc_plot_geom = move |css_w: f64, css_h: f64, fft_on: bool, disp: DisplayMode| -> (f64, f64) {
        let plot_w = (css_w - PAD_L - PAD_R).max(120.0);
        let scope_h = if fft_on && disp == DisplayMode::Yt {
            ((css_h - PAD_T - PAD_B - 30.0) * 0.64).max(70.0)
        } else {
            (css_h - PAD_T - PAD_B).max(70.0)
        };
        (plot_w, scope_h)
    };

    let render_now = move || {
        let ver = frame_version.get_untracked();
        let frame_opt = latest_frame.get_untracked();
        let ch_mask = channel_mask.get_untracked();
        let cfgs = channel_cfgs.get_untracked();
        let sel_ch = selected_ch.get_untracked().min(3);
        let trig_mv = trig_level_mv.get_untracked();
        let trig_ch = (trig_channel.get_untracked() as usize).min(3);
        let t_edge = trig_edge.get_untracked();
        let fft_on = show_fft.get_untracked();
        let fft_ch = (fft_channel.get_untracked() as usize).min(3);
        let cursors_on = show_cursors.get_untracked();
        let ca = cursor_a_norm.get_untracked();
        let cb = cursor_b_norm.get_untracked();
        let z = zoom.get_untracked().max(1.0);
        let pan = pan_norm.get_untracked();
        let persist = persistence_mode.get_untracked();
        let clear_v = persist_clear_ver.get_untracked();
        let m_mode = math_mode.get_untracked();
        let d_mode = display_mode.get_untracked();
        let hud_msg = drag_hud.get_untracked();
        let cur_drag = drag_target.get_value();

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
        let css_w = rect.width().max(240.0);
        let css_h = rect.height().max(110.0);
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

        // Outer instrument bezel background
        ctx.set_fill_style_str("#060a10");
        ctx.fill_rect(0.0, 0.0, css_w, css_h);

        let (plot_w, scope_h) = calc_plot_geom(css_w, css_h, fft_on, d_mode);

        // Draw authentic 10x8 oscilloscope graticule with center crosshair sub-divisions
        let grat_ch = if d_mode == DisplayMode::Xy { 1 } else { sel_ch };
        draw_graticule(
            &ctx,
            PAD_L,
            PAD_T,
            plot_w,
            scope_h,
            &cfgs[grat_ch],
            CHANNEL_COLORS[grat_ch],
            d_mode,
        );

        // Compute per-channel DC means for current frame (used for AC coupling & trigger handle)
        let mut cur_means = [0.0f32; 4];
        if let Some(frame) = frame_opt.as_ref() {
            for (ch, mean_slot) in cur_means.iter_mut().enumerate() {
                if cfgs[ch].coupling == ChannelCoupling::Ac && !frame.channels_mv[ch].is_empty() {
                    *mean_slot = channel_mean_mv(&frame.channels_mv[ch]);
                }
            }
        }

        // Draw faint 0V reference dashed line for the channel currently being dragged (or selected)
        if d_mode == DisplayMode::Yt {
            let ref_ch = match cur_drag {
                DragTarget::ChannelPos(c) => c,
                _ => sel_ch,
            };
            if (ch_mask & (1 << ref_ch)) != 0 {
                let y_gnd = divs_to_y(cfgs[ref_ch].offset_divs as f64, PAD_T, scope_h);
                if y_gnd >= PAD_T && y_gnd <= PAD_T + scope_h {
                    ctx.save();
                    ctx.set_stroke_style_str(CHANNEL_COLORS[ref_ch]);
                    ctx.set_global_alpha(if matches!(cur_drag, DragTarget::ChannelPos(_)) {
                        0.55
                    } else {
                        0.18
                    });
                    ctx.set_line_width(1.0);
                    let _ = ctx.set_line_dash(&js_sys::Array::of2(&3.into(), &5.into()));
                    ctx.begin_path();
                    ctx.move_to(PAD_L, y_gnd);
                    ctx.line_to(PAD_L + plot_w, y_gnd);
                    ctx.stroke();
                    ctx.restore();
                }
            }
        }

        if let Some(frame) = frame_opt.as_ref() {
            let n = frame.samples_per_ch;
            let span = 1.0 / z;
            let start_norm = pan.clamp(0.0, (1.0 - span).max(0.0));
            let end_norm = (start_norm + span).min(1.0);

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

                scratch.update_value(|sc| {
                    history.with_value(|h| {
                        let count = h.len();
                        for (idx, old_frame) in h.iter().enumerate() {
                            if idx + 1 == count {
                                break;
                            }
                            let age = (now_ts - old_frame.timestamp_ms).clamp(0.0, decay_ms);
                            let frac = (1.0 - age / decay_ms).clamp(0.0, 1.0);
                            let alpha = frac.powf(1.6) * 0.48;
                            if alpha > 0.02 {
                                draw_scope_trace(
                                    &ctx,
                                    old_frame,
                                    None,
                                    &cfgs,
                                    m_mode,
                                    d_mode,
                                    sc,
                                    PAD_L,
                                    PAD_T,
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
                    display_mode: d_mode,
                    math_mode: m_mode,
                    atten_code: frame.atten_code,
                    sample_rate_hz: frame.sample_rate_hz,
                    samples_per_ch: frame.samples_per_ch,
                    clear_ver: clear_v,
                    cfgs_hash: hash_channel_cfgs(&cfgs),
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
                                    draw_scope_trace(
                                        &p_ctx,
                                        frame,
                                        Some(&cur_means),
                                        &cfgs,
                                        m_mode,
                                        d_mode,
                                        sc,
                                        PAD_L,
                                        PAD_T,
                                        plot_w,
                                        scope_h,
                                        start_norm,
                                        end_norm,
                                        span,
                                        0.22,
                                        1.2,
                                        false,
                                    );
                                });
                            }
                        }
                    }

                    ctx.save();
                    let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
                    let _ = ctx.draw_image_with_html_canvas_element(&p_canvas, 0.0, 0.0);
                    ctx.restore();
                }
            }

            // --- Live Phosphor Trace (outer bloom + crisp core) ---
            scratch.update_value(|sc| {
                // Soft outer phosphor glow pass
                draw_scope_trace(
                    &ctx,
                    frame,
                    Some(&cur_means),
                    &cfgs,
                    m_mode,
                    d_mode,
                    sc,
                    PAD_L,
                    PAD_T,
                    plot_w,
                    scope_h,
                    start_norm,
                    end_norm,
                    span,
                    0.20,
                    4.4,
                    false,
                );
                // Bright razor-sharp core pass
                draw_scope_trace(
                    &ctx,
                    frame,
                    Some(&cur_means),
                    &cfgs,
                    m_mode,
                    d_mode,
                    sc,
                    PAD_L,
                    PAD_T,
                    plot_w,
                    scope_h,
                    start_norm,
                    end_norm,
                    span,
                    1.0,
                    1.75,
                    true,
                );
            });

            // --- Trigger Markers & Time Axis (Y-T mode) ---
            if d_mode == DisplayMode::Yt {
                // Vertical trigger time marker
                let trig_norm =
                    (frame.trigger_index as f64) / (n.saturating_sub(1).max(1) as f64);
                if trig_norm >= start_norm && trig_norm <= end_norm {
                    let tx = PAD_L + ((trig_norm - start_norm) / span) * plot_w;
                    ctx.save();
                    ctx.set_stroke_style_str("rgba(249, 115, 22, 0.35)");
                    ctx.set_line_width(1.0);
                    let _ = ctx.set_line_dash(&js_sys::Array::of2(&4.into(), &4.into()));
                    ctx.begin_path();
                    ctx.move_to(tx, PAD_T);
                    ctx.line_to(tx, PAD_T + scope_h);
                    ctx.stroke();
                    ctx.restore();

                    // Top orange trigger-time pointer triangle
                    ctx.set_fill_style_str("#f97316");
                    ctx.begin_path();
                    ctx.move_to(tx, PAD_T + 7.0);
                    ctx.line_to(tx - 5.5, PAD_T - 2.0);
                    ctx.line_to(tx + 5.5, PAD_T - 2.0);
                    ctx.close_path();
                    ctx.fill();
                }

                // Trigger level horizontal dashed line + draggable right-margin handle (T◄)
                let ty_raw = trig_mv_to_y(trig_mv, cur_means[trig_ch], cfgs[trig_ch], PAD_T, scope_h);
                let ty_clamped = ty_raw.clamp(PAD_T + 2.0, PAD_T + scope_h - 2.0);
                let trig_offscreen = ty_raw < PAD_T || ty_raw > PAD_T + scope_h;

                ctx.save();
                ctx.set_stroke_style_str(if matches!(cur_drag, DragTarget::TriggerLevel) {
                    "rgba(249, 115, 22, 0.90)"
                } else {
                    "rgba(249, 115, 22, 0.45)"
                });
                ctx.set_line_width(if matches!(cur_drag, DragTarget::TriggerLevel) {
                    1.5
                } else {
                    1.0
                });
                let _ = ctx.set_line_dash(&js_sys::Array::of2(&5.into(), &5.into()));
                ctx.begin_path();
                ctx.move_to(PAD_L, ty_clamped);
                ctx.line_to(PAD_L + plot_w, ty_clamped);
                ctx.stroke();
                ctx.restore();

                draw_trigger_handle(
                    &ctx,
                    PAD_L + plot_w,
                    ty_clamped,
                    trig_ch,
                    trig_offscreen,
                    matches!(cur_drag, DragTarget::TriggerLevel),
                );

                // Time axis labels (relative to trigger instant t=0, like real scopes!)
                let total_us = (n as f64) / (frame.sample_rate_hz as f64) * 1_000_000.0;
                let trig_us = trig_norm * total_us;
                let start_us = start_norm * total_us - trig_us;
                let visible_us = span * total_us;
                let us_per_div = visible_us / 10.0;

                ctx.set_fill_style_str("#8b949e");
                ctx.set_font("10px ui-monospace, SFMono-Regular, Menlo, monospace");
                for d in 0..=5 {
                    let frac = (d as f64) / 5.0;
                    let x = PAD_L + frac * plot_w;
                    let t_us = start_us + frac * visible_us;
                    let _ = ctx.fill_text(&format_time_us(t_us as f32), x - 18.0, PAD_T + scope_h + 15.0);
                }

                // Top-left instrument status HUD
                let mut hud_left = format!(
                    "H: {}/div  ·  {}/ch",
                    format_time_us(us_per_div as f32),
                    format_freq(frame.sample_rate_hz as f32)
                );
                if let Some(badge) = persist.badge() {
                    hud_left.push_str("  ·  ");
                    hud_left.push_str(badge);
                }
                ctx.set_fill_style_str("#94a3b8");
                ctx.set_font("600 10.5px ui-monospace, SFMono-Regular, Menlo, monospace");
                let _ = ctx.fill_text(&hud_left, PAD_L + 2.0, PAD_T - 7.0);

                // A/B Cursors + live waveform intersection markers
                if cursors_on {
                    let ax = PAD_L + ca.clamp(0.0, 1.0) * plot_w;
                    let bx = PAD_L + cb.clamp(0.0, 1.0) * plot_w;
                    draw_cursor(&ctx, ax, PAD_T, scope_h, "#38bdf8", "A");
                    draw_cursor(&ctx, bx, PAD_T, scope_h, "#f472b6", "B");

                    let ref_ch = if !frame.channels_mv[sel_ch].is_empty() {
                        sel_ch
                    } else {
                        (0..4).find(|&c| !frame.channels_mv[c].is_empty()).unwrap_or(0)
                    };
                    let samples = &frame.channels_mv[ref_ch];
                    if !samples.is_empty() {
                        let max_i = samples.len() - 1;
                        for (c_norm, cx, c_col) in [(ca, ax, "#38bdf8"), (cb, bx, "#f472b6")] {
                            let idx = ((start_norm + c_norm * span).clamp(0.0, 1.0) * max_i as f64)
                                .round() as usize;
                            let divs = sample_mv_to_divs(samples[idx], cur_means[ref_ch], cfgs[ref_ch]) as f64;
                            let cy = divs_to_y(divs, PAD_T, scope_h);
                            if cy >= PAD_T && cy <= PAD_T + scope_h {
                                ctx.save();
                                ctx.set_stroke_style_str(c_col);
                                ctx.set_global_alpha(0.30);
                                ctx.set_line_width(1.0);
                                let _ = ctx.set_line_dash(&js_sys::Array::of2(&2.into(), &4.into()));
                                ctx.begin_path();
                                ctx.move_to(PAD_L, cy);
                                ctx.line_to(PAD_L + plot_w, cy);
                                ctx.stroke();
                                ctx.restore();

                                ctx.set_fill_style_str(c_col);
                                ctx.begin_path();
                                let _ = ctx.arc(cx, cy, 4.0, 0.0, std::f64::consts::TAU);
                                ctx.fill();
                            }
                        }
                    }
                }

                // Optional FFT Lower Pane (cached per frame_version + channel)
                if fft_on {
                    let fft_top = PAD_T + scope_h + 30.0;
                    let fft_h = (css_h - fft_top - 22.0).max(60.0);
                    let chosen = if !frame.channels_mv[fft_ch].is_empty() {
                        fft_ch
                    } else {
                        (0..4).find(|&c| !frame.channels_mv[c].is_empty()).unwrap_or(0)
                    };
                    scratch.update_value(|sc| {
                        draw_fft_pane(
                            &ctx,
                            PAD_L,
                            fft_top,
                            plot_w,
                            fft_h,
                            &frame.channels_mv[chosen],
                            frame.sample_rate_hz,
                            ver,
                            chosen,
                            sc,
                            CHANNEL_COLORS[chosen],
                            CHANNEL_NAMES[chosen],
                        );
                    });
                }
            } else {
                // X-Y Mode HUD
                let mut hud_left = format!(
                    "X-Y MODE  ·  X: CH1 ({})  ·  Y: CH2 ({})",
                    cfgs[0].format_vdiv(),
                    cfgs[1].format_vdiv()
                );
                if let Some(badge) = persist.badge() {
                    hud_left.push_str("  ·  ");
                    hud_left.push_str(badge);
                }
                ctx.set_fill_style_str("#38bdf8");
                ctx.set_font("700 10.5px ui-monospace, SFMono-Regular, Menlo, monospace");
                let _ = ctx.fill_text(&hud_left, PAD_L + 2.0, PAD_T - 7.0);
            }

            // Top-right Trigger Status Pill
            let edge_sym = if t_edge == 0 { "↗" } else { "↘" };
            let disp_trig_mv = (trig_mv as f32) * cfgs[trig_ch].probe_mult();
            let trig_info = format!(
                "T{} {} {}  {}",
                trig_ch + 1,
                edge_sym,
                format_voltage_mv(disp_trig_mv),
                if frame.triggered { "TRIG'D" } else { "AUTO" }
            );
            ctx.set_fill_style_str(if frame.triggered { "#4ade80" } else { "#fbbf24" });
            ctx.set_font("700 10.5px ui-monospace, SFMono-Regular, Menlo, monospace");
            let tw = ctx.measure_text(&trig_info).map(|m| m.width()).unwrap_or(130.0);
            let _ = ctx.fill_text(&trig_info, PAD_L + plot_w - tw, PAD_T - 7.0);

            if frame.adc_saturated {
                ctx.set_fill_style_str("#f87171");
                let _ = ctx.fill_text("CLIP!", PAD_L + plot_w - tw - 44.0, PAD_T - 7.0);
            }
        } else {
            ctx.set_fill_style_str("#6e7681");
            ctx.set_font("500 14px system-ui, -apple-system, sans-serif");
            let _ = ctx.fill_text(
                "No waveform captured — click Demo or Connect USB",
                PAD_L + plot_w * 0.5 - 155.0,
                PAD_T + scope_h * 0.5,
            );
        }

        // --- Draggable Left-Margin Channel Ground Markers (1►, 2►, 3►, 4►) ---
        // Draw unselected channels first, then selected channel last so its handle is always on top
        if d_mode == DisplayMode::Yt {
            let mut order = [0usize, 1, 2, 3];
            order.sort_by_key(|&c| if c == sel_ch { 1 } else { 0 });
            for ch in order {
                if (ch_mask & (1 << ch)) == 0 {
                    continue;
                }
                let y_gnd = divs_to_y(cfgs[ch].offset_divs as f64, PAD_T, scope_h);
                let y_clamped = y_gnd.clamp(PAD_T + 6.0, PAD_T + scope_h - 6.0);
                let is_sel = ch == sel_ch;
                let is_drag = matches!(cur_drag, DragTarget::ChannelPos(c) if c == ch);

                // Check if waveform clips above or below the 8-div screen
                let mut clip_dir = 0i8;
                if let Some(frame) = frame_opt.as_ref() {
                    let s = &frame.channels_mv[ch];
                    if !s.is_empty() {
                        let mut hi = f32::NEG_INFINITY;
                        let mut lo = f32::INFINITY;
                        for &v in s {
                            if v > hi { hi = v; }
                            if v < lo { lo = v; }
                        }
                        let d_hi = sample_mv_to_divs(hi, cur_means[ch], cfgs[ch]);
                        let d_lo = sample_mv_to_divs(lo, cur_means[ch], cfgs[ch]);
                        if d_hi > 4.05 && d_lo > 4.05 {
                            clip_dir = 1;
                        } else if d_hi < -4.05 && d_lo < -4.05 {
                            clip_dir = -1;
                        }
                    }
                }

                draw_channel_marker(
                    &ctx,
                    PAD_L,
                    y_clamped,
                    ch,
                    CHANNEL_COLORS[ch],
                    is_sel,
                    is_drag,
                    clip_dir,
                );
            }

            if m_mode != MathMode::Off {
                let y_m = divs_to_y(0.0, PAD_T, scope_h);
                draw_math_marker(&ctx, PAD_L, y_m);
            }
        }

        // Floating HUD pill while dragging channel position or trigger level
        if let Some(msg) = hud_msg {
            let box_w = (msg.len() as f64 * 7.2 + 18.0).max(110.0);
            let bx = PAD_L + plot_w * 0.5 - box_w * 0.5;
            let by = PAD_T + 10.0;
            ctx.set_fill_style_str("rgba(13, 19, 29, 0.92)");
            ctx.fill_rect(bx, by, box_w, 24.0);
            ctx.set_stroke_style_str("#38bdf8");
            ctx.set_line_width(1.2);
            ctx.stroke_rect(bx, by, box_w, 24.0);
            ctx.set_fill_style_str("#e6edf3");
            ctx.set_font("700 11px ui-monospace, SFMono-Regular, Menlo, monospace");
            let _ = ctx.fill_text(&msg, bx + 9.0, by + 16.0);
        }
    };

    // Coalesce redraws onto browser requestAnimationFrame cadence
    Effect::new(move |_| {
        let _ = resize_ver.get();
        let _ = frame_version.get();
        let _ = latest_frame.with(|f| f.is_some());
        let _ = channel_mask.get();
        let _ = channel_cfgs.get();
        let _ = selected_ch.get();
        let _ = trig_level_mv.get();
        let _ = trig_channel.get();
        let _ = trig_edge.get();
        let _ = show_fft.get();
        let _ = fft_channel.get();
        let _ = show_cursors.get();
        let _ = cursor_a_norm.get();
        let _ = cursor_b_norm.get();
        let _ = zoom.get();
        let _ = pan_norm.get();
        let _ = persistence_mode.get();
        let _ = persist_clear_ver.get();
        let _ = math_mode.get();
        let _ = display_mode.get();
        let _ = drag_hud.get();

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
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
        let rect = canvas.get_bounding_client_rect();
        let x = ev.client_x() as f64 - rect.left();

        // Mouse wheel over left channel margin adjusts active channel's V/div (1-2-5 steps!)
        if x < PAD_L + 6.0 {
            let delta: isize = if ev.delta_y() < 0.0 { -1 } else { 1 };
            let ch = selected_ch.get_untracked().min(3);
            channel_cfgs.update(|cfgs| {
                let next = (cfgs[ch].vdiv_idx as isize + delta)
                    .clamp(0, (V_PER_DIV_STEPS_MV.len() - 1) as isize)
                    as usize;
                cfgs[ch].vdiv_idx = next;
            });
            return;
        }

        // Otherwise zoom horizontal timebase anchored at cursor X (Y-T mode only)
        let fft_on = show_fft.get_untracked();
        let d_mode = display_mode.get_untracked();
        if d_mode == DisplayMode::Xy {
            return;
        }
        let (plot_w, _) = calc_plot_geom(rect.width().max(240.0), rect.height().max(110.0), fft_on, d_mode);
        let anchor_norm = ((x - PAD_L) / plot_w).clamp(0.0, 1.0);
        let factor = if ev.delta_y() < 0.0 { 1.25 } else { 0.8 };
        let old_z = zoom.get_untracked().max(1.0);
        let (new_z, new_pan) =
            apply_anchored_zoom(old_z, pan_norm.get_untracked(), factor, anchor_norm, 1.0, 32.0);
        if (new_z - old_z).abs() > 1e-4 {
            zoom.set(new_z);
            pan_norm.set(new_pan);
        }
    };

    let on_pointer_down = move |ev: web_sys::PointerEvent| {
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
        let rect = canvas.get_bounding_client_rect();
        let x = ev.client_x() as f64 - rect.left();
        let y = ev.client_y() as f64 - rect.top();
        let fft_on = show_fft.get_untracked();
        let d_mode = display_mode.get_untracked();
        let (plot_w, scope_h) = calc_plot_geom(rect.width().max(240.0), rect.height().max(110.0), fft_on, d_mode);

        let mut active_ptrs = 0usize;
        gesture_tracker.update_value(|gt| {
            gt.pointer_down(ev.pointer_id(), x, y);
            active_ptrs = gt.active_count();
        });
        if active_ptrs >= 2 {
            let _ = canvas.set_pointer_capture(ev.pointer_id());
            drag_target.set_value(DragTarget::Pinch);
            drag_hud.set(None);
            return;
        }

        if d_mode == DisplayMode::Yt {
            // 1. Check left-margin draggable channel ground markers (1►..4►)
            if x <= PAD_L + 16.0 && y >= PAD_T - 10.0 && y <= PAD_T + scope_h + 10.0 {
                let mask = channel_mask.get_untracked();
                let cfgs = channel_cfgs.get_untracked();
                let mut best_ch: Option<usize> = None;
                let mut best_dist = 18.0f64;
                for (ch, cfg) in cfgs.iter().enumerate() {
                    if (mask & (1 << ch)) == 0 {
                        continue;
                    }
                    let y_gnd = divs_to_y(cfg.offset_divs as f64, PAD_T, scope_h)
                        .clamp(PAD_T + 6.0, PAD_T + scope_h - 6.0);
                    let dist = (y - y_gnd).abs();
                    if dist <= best_dist {
                        best_dist = dist;
                        best_ch = Some(ch);
                    }
                }
                if let Some(ch) = best_ch {
                    selected_ch.set(ch);
                    drag_target.set_value(DragTarget::ChannelPos(ch));
                    let _ = canvas.set_pointer_capture(ev.pointer_id());
                    drag_hud.set(Some(format!(
                        "CH{} Pos: {:+.2} div",
                        ch + 1,
                        cfgs[ch].offset_divs
                    )));
                    return;
                }
            }

            // 2. Check right-margin draggable Trigger Handle (T◄)
            if x >= PAD_L + plot_w - 14.0 && y >= PAD_T - 10.0 && y <= PAD_T + scope_h + 10.0 {
                let trig_ch = (trig_channel.get_untracked() as usize).min(3);
                let cfgs = channel_cfgs.get_untracked();
                let dc_mean = latest_frame
                    .get_untracked()
                    .map(|f| {
                        if cfgs[trig_ch].coupling == ChannelCoupling::Ac {
                            channel_mean_mv(&f.channels_mv[trig_ch])
                        } else {
                            0.0
                        }
                    })
                    .unwrap_or(0.0);
                let ty = trig_mv_to_y(
                    trig_level_mv.get_untracked(),
                    dc_mean,
                    cfgs[trig_ch],
                    PAD_T,
                    scope_h,
                )
                .clamp(PAD_T + 2.0, PAD_T + scope_h - 2.0);
                if (y - ty).abs() <= 22.0 {
                    drag_target.set_value(DragTarget::TriggerLevel);
                    let _ = canvas.set_pointer_capture(ev.pointer_id());
                    let disp_mv = (trig_level_mv.get_untracked() as f32) * cfgs[trig_ch].probe_mult();
                    drag_hud.set(Some(format!(
                        "Trig CH{}: {}",
                        trig_ch + 1,
                        format_voltage_mv(disp_mv)
                    )));
                    return;
                }
            }

            // 3. Check A/B Time Cursors
            let norm_x = ((x - PAD_L) / plot_w).clamp(0.0, 1.0);
            if show_cursors.get_untracked() && x >= PAD_L && x <= PAD_L + plot_w {
                let da = (norm_x - cursor_a_norm.get_untracked()).abs() * plot_w;
                let db = (norm_x - cursor_b_norm.get_untracked()).abs() * plot_w;
                if da < 14.0 && da <= db {
                    drag_target.set_value(DragTarget::CursorA);
                    let _ = canvas.set_pointer_capture(ev.pointer_id());
                    return;
                }
                if db < 14.0 {
                    drag_target.set_value(DragTarget::CursorB);
                    let _ = canvas.set_pointer_capture(ev.pointer_id());
                    return;
                }
            }
        }

        // 4. Default: Horizontal Pan (and pinch-to-zoom on multi-touch)
        drag_target.set_value(DragTarget::Pan);
        let _ = canvas.set_pointer_capture(ev.pointer_id());
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
        let y = ev.client_y() as f64 - rect.top();
        let fft_on = show_fft.get_untracked();
        let d_mode = display_mode.get_untracked();
        let (plot_w, scope_h) = calc_plot_geom(rect.width().max(240.0), rect.height().max(110.0), fft_on, d_mode);
        let norm_x = ((x - PAD_L) / plot_w).clamp(0.0, 1.0);

        let mut gesture_delta = None;
        gesture_tracker.update_value(|gt| {
            gesture_delta = gt.pointer_move(ev.pointer_id(), x, y);
        });

        match target {
            DragTarget::ChannelPos(ch) => {
                let raw_divs = y_to_divs(y, PAD_T, scope_h).clamp(-4.0, 4.0) as f32;
                // Gentle 0.05-div precision with magnetic snap near 0.0
                let snapped = if raw_divs.abs() < 0.08 {
                    0.0
                } else {
                    (raw_divs * 20.0).round() / 20.0
                };
                channel_cfgs.update(|cfgs| {
                    cfgs[ch].offset_divs = snapped;
                });
                drag_hud.set(Some(format!("CH{} Pos: {:+.2} div", ch + 1, snapped)));
            }
            DragTarget::TriggerLevel => {
                let trig_ch = (trig_channel.get_untracked() as usize).min(3);
                let cfgs = channel_cfgs.get_untracked();
                let dc_mean = latest_frame
                    .get_untracked()
                    .map(|f| {
                        if cfgs[trig_ch].coupling == ChannelCoupling::Ac {
                            channel_mean_mv(&f.channels_mv[trig_ch])
                        } else {
                            0.0
                        }
                    })
                    .unwrap_or(0.0);
                let max_mv = resolve_atten_preset(atten_code.get_untracked()).full_scale_mv;
                let next_mv = y_to_trig_mv(y, dc_mean, cfgs[trig_ch], PAD_T, scope_h, max_mv);
                if next_mv != trig_level_mv.get_untracked() {
                    trig_level_mv.set(next_mv);
                    on_apply_trig.run(());
                }
                let disp_mv = (next_mv as f32) * cfgs[trig_ch].probe_mult();
                drag_hud.set(Some(format!(
                    "Trig CH{}: {}",
                    trig_ch + 1,
                    format_voltage_mv(disp_mv)
                )));
            }
            DragTarget::CursorA => cursor_a_norm.set(norm_x),
            DragTarget::CursorB => cursor_b_norm.set(norm_x),
            DragTarget::Pan => {
                if let Some(GestureDelta::Pan { dx, .. }) = gesture_delta {
                    let next_pan = apply_horizontal_pan(
                        zoom.get_untracked().max(1.0),
                        pan_norm.get_untracked(),
                        dx,
                        plot_w,
                    );
                    pan_norm.set(next_pan);
                }
            }
            DragTarget::Pinch => {
                if let Some(GestureDelta::Pinch {
                    scale_ratio,
                    center_x,
                    pan_dx,
                    ..
                }) = gesture_delta
                {
                    let anchor = ((center_x - PAD_L) / plot_w).clamp(0.0, 1.0);
                    let (next_z, zoomed_pan) = apply_anchored_zoom(
                        zoom.get_untracked().max(1.0),
                        pan_norm.get_untracked(),
                        scale_ratio,
                        anchor,
                        1.0,
                        32.0,
                    );
                    let panned = apply_horizontal_pan(next_z, zoomed_pan, pan_dx, plot_w);
                    zoom.set(next_z);
                    pan_norm.set(panned);
                }
            }
            DragTarget::None => {}
        }
    };

    let on_pointer_up = move |ev: web_sys::PointerEvent| {
        let mut remaining = 0usize;
        gesture_tracker.update_value(|gt| {
            gt.pointer_up(ev.pointer_id());
            remaining = gt.active_count();
        });
        if drag_target.get_value() == DragTarget::Pinch && remaining == 1 {
            drag_target.set_value(DragTarget::Pan);
            return;
        }
        if remaining == 0 && drag_target.get_value() != DragTarget::None {
            drag_target.set_value(DragTarget::None);
            drag_hud.set(None);
        }
    };

    let on_dblclick = move |ev: web_sys::MouseEvent| {
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
        let rect = canvas.get_bounding_client_rect();
        let x = ev.client_x() as f64 - rect.left();
        if x <= PAD_L + 16.0 {
            let ch = selected_ch.get_untracked().min(3);
            channel_cfgs.update(|cfgs| {
                cfgs[ch].offset_divs = 0.0;
            });
        }
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
                on:pointercancel=on_pointer_up
                on:dblclick=on_dblclick
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
fn draw_scope_trace(
    ctx: &web_sys::CanvasRenderingContext2d,
    frame: &ScopeFrame,
    precomputed_means: Option<&[f32; 4]>,
    cfgs: &[ChannelViewConfig; 4],
    math_mode: MathMode,
    display_mode: DisplayMode,
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
    if display_mode == DisplayMode::Xy {
        draw_xy_trace(
            ctx, frame, cfgs, pad_l, pad_t, plot_w, scope_h, alpha, line_width,
        );
        return;
    }

    let n = frame.samples_per_ch;
    if n < 2 {
        return;
    }
    let denom_n = (n - 1) as f64;
    let start_idx = ((start_norm * denom_n).floor() as usize).min(n - 1);
    let end_idx = ((end_norm * denom_n).ceil() as usize)
        .max(start_idx + 1)
        .min(n);
    let draw_start = start_idx.saturating_sub(1);
    let draw_end = (end_idx + 1).min(n);
    let visible_count = (end_idx - start_idx).max(1);
    let dx_per_sample = plot_w / (visible_count as f64);

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
        let cfg = cfgs[ch];
        let dc_mean = if cfg.coupling == ChannelCoupling::Ac {
            precomputed_means
                .map(|m| m[ch])
                .unwrap_or_else(|| channel_mean_mv(samples))
        } else {
            0.0
        };
        stroke_single_channel(
            ctx,
            samples,
            dc_mean,
            cfg,
            CHANNEL_COLORS[ch],
            sc,
            pad_l,
            pad_t,
            plot_w,
            scope_h,
            denom_n,
            start_norm,
            span,
            draw_start,
            draw_end,
            visible_count,
            dx_per_sample,
            use_spline,
            show_sample_dots,
        );
    }

    // Optional Math channel (CH1 - CH2 or CH1 + CH2) centered at 0 div using CH1's V/div
    if math_mode != MathMode::Off {
        compute_math_samples(frame, math_mode, &mut sc.math_mv);
        if sc.math_mv.len() >= draw_end {
            let math_cfg = ChannelViewConfig {
                vdiv_idx: cfgs[0].vdiv_idx,
                offset_divs: 0.0,
                coupling: ChannelCoupling::Dc,
                invert: false,
                probe_x10: cfgs[0].probe_x10,
            };
            let math_slice = std::mem::take(&mut sc.math_mv);
            stroke_single_channel(
                ctx,
                &math_slice,
                0.0,
                math_cfg,
                MATH_COLOR,
                sc,
                pad_l,
                pad_t,
                plot_w,
                scope_h,
                denom_n,
                start_norm,
                span,
                draw_start,
                draw_end,
                visible_count,
                dx_per_sample,
                use_spline,
                false,
            );
            sc.math_mv = math_slice;
        }
    }

    ctx.restore();
}

#[allow(clippy::too_many_arguments)]
fn stroke_single_channel(
    ctx: &web_sys::CanvasRenderingContext2d,
    samples: &[f32],
    dc_mean: f32,
    cfg: ChannelViewConfig,
    color: &str,
    sc: &mut ScratchSpline,
    pad_l: f64,
    pad_t: f64,
    plot_w: f64,
    scope_h: f64,
    denom_n: f64,
    start_norm: f64,
    span: f64,
    draw_start: usize,
    draw_end: usize,
    visible_count: usize,
    dx_per_sample: f64,
    use_spline: bool,
    show_sample_dots: bool,
) {
    ctx.set_stroke_style_str(color);

    if use_spline {
        sc.xs.clear();
        sc.ys.clear();
        for (idx, &mv) in samples[draw_start..draw_end].iter().enumerate() {
            let i = draw_start + idx;
            let norm_i = (i as f64) / denom_n;
            let x = pad_l + ((norm_i - start_norm) / span) * plot_w;
            let divs = sample_mv_to_divs(mv, dc_mean, cfg) as f64;
            let y = divs_to_y(divs, pad_t, scope_h);
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
            let divs = sample_mv_to_divs(samples[i], dc_mean, cfg) as f64;
            let y = divs_to_y(divs, pad_t, scope_h);
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

#[allow(clippy::too_many_arguments)]
fn draw_xy_trace(
    ctx: &web_sys::CanvasRenderingContext2d,
    frame: &ScopeFrame,
    cfgs: &[ChannelViewConfig; 4],
    pad_l: f64,
    pad_t: f64,
    plot_w: f64,
    scope_h: f64,
    alpha: f64,
    line_width: f64,
) {
    let ch1 = &frame.channels_mv[0];
    let ch2 = &frame.channels_mv[1];
    let len = ch1.len().min(ch2.len());
    if len < 2 {
        return;
    }

    let m1 = channel_mean_mv(ch1);
    let m2 = channel_mean_mv(ch2);
    let cfg_x = cfgs[0];
    let cfg_y = cfgs[1];

    ctx.save();
    ctx.set_global_alpha(alpha);
    ctx.begin_path();
    ctx.rect(pad_l, pad_t, plot_w, scope_h);
    ctx.clip();
    ctx.set_stroke_style_str("#22d3ee");
    ctx.set_line_width(line_width);
    ctx.set_line_join("round");
    ctx.begin_path();

    for i in 0..len {
        // Center X-Y Lissajous around each channel's AC midpoint (or DC if user prefers)
        let dx_mv = (ch1[i] - m1) * cfg_x.probe_mult() * if cfg_x.invert { -1.0 } else { 1.0 };
        let dy_mv = (ch2[i] - m2) * cfg_y.probe_mult() * if cfg_y.invert { -1.0 } else { 1.0 };
        let x_divs = (dx_mv / cfg_x.v_per_div_mv().max(1.0)) as f64;
        let y_divs = (dy_mv / cfg_y.v_per_div_mv().max(1.0)) as f64;
        let x = pad_l + ((x_divs + 5.0) / 10.0) * plot_w;
        let y = divs_to_y(y_divs, pad_t, scope_h);
        if i == 0 {
            ctx.move_to(x, y);
        } else {
            ctx.line_to(x, y);
        }
    }
    ctx.stroke();
    ctx.restore();
}

#[allow(clippy::too_many_arguments)]
fn draw_graticule(
    ctx: &web_sys::CanvasRenderingContext2d,
    x0: f64,
    y0: f64,
    w: f64,
    h: f64,
    sel_cfg: &ChannelViewConfig,
    sel_color: &str,
    display_mode: DisplayMode,
) {
    // Deep digital oscilloscope screen surface
    ctx.set_fill_style_str("#0a1019");
    ctx.fill_rect(x0, y0, w, h);

    // 1. Major 10x8 division grid lines (subtle dotted/thin benchtop style)
    ctx.set_stroke_style_str("#1a2538");
    ctx.set_line_width(1.0);
    ctx.begin_path();
    for i in 1..10 {
        let x = x0 + (i as f64 / 10.0) * w;
        ctx.move_to(x, y0);
        ctx.line_to(x, y0 + h);
    }
    for j in 1..8 {
        let y = y0 + (j as f64 / 8.0) * h;
        ctx.move_to(x0, y);
        ctx.line_to(x0 + w, y);
    }
    ctx.stroke();

    // 2. Highlighted Center Crosshair Axes + 5 Sub-Division Micro-Ticks per Division
    let cx = x0 + w * 0.5;
    let cy = y0 + h * 0.5;
    ctx.set_stroke_style_str("#2b3c57");
    ctx.set_line_width(1.2);
    ctx.begin_path();
    ctx.move_to(cx, y0);
    ctx.line_to(cx, y0 + h);
    ctx.move_to(x0, cy);
    ctx.line_to(x0 + w, cy);

    // 50 horizontal sub-ticks along center X axis (5 per division)
    for k in 1..50 {
        let x = x0 + (k as f64 / 50.0) * w;
        let tick = if k % 5 == 0 { 5.0 } else { 2.5 };
        ctx.move_to(x, cy - tick);
        ctx.line_to(x, cy + tick);
    }
    // 40 vertical sub-ticks along center Y axis (5 per division)
    for k in 1..40 {
        let y = y0 + (k as f64 / 40.0) * h;
        let tick = if k % 5 == 0 { 5.0 } else { 2.5 };
        ctx.move_to(cx - tick, y);
        ctx.line_to(cx + tick, y);
    }
    ctx.stroke();

    // 3. Left Axis Voltage Scale (in the selected channel's color & coordinate system)
    ctx.set_font("10px ui-monospace, SFMono-Regular, Menlo, monospace");
    ctx.set_fill_style_str(if display_mode == DisplayMode::Xy {
        "#38bdf8"
    } else {
        sel_color
    });
    let vdiv_mv = sel_cfg.v_per_div_mv() as f64;
    let off = if display_mode == DisplayMode::Xy {
        0.0
    } else {
        sel_cfg.offset_divs as f64
    };
    for j in 0..=8 {
        if j % 2 != 0 {
            continue;
        }
        let y = y0 + (j as f64 / 8.0) * h;
        let divs_from_center = 4.0 - (j as f64);
        let raw_mv = (divs_from_center - off) * vdiv_mv;
        let mv = if display_mode == DisplayMode::Yt && sel_cfg.invert {
            -raw_mv
        } else {
            raw_mv
        };
        let label = if mv.abs() >= 1000.0 {
            format!("{:+.1}V", mv / 1000.0)
        } else if mv.abs() < 1.0 {
            "0.0V".to_string()
        } else {
            format!("{:+.0}mV", mv)
        };
        let _ = ctx.fill_text(&label, 4.0, y + 3.5);
    }

    // Crisp outer graticule border
    ctx.set_stroke_style_str("#334155");
    ctx.set_line_width(1.2);
    ctx.stroke_rect(x0, y0, w, h);
}

#[allow(clippy::too_many_arguments)]
fn draw_channel_marker(
    ctx: &web_sys::CanvasRenderingContext2d,
    pad_l: f64,
    y: f64,
    ch: usize,
    color: &str,
    selected: bool,
    dragging: bool,
    clip_dir: i8,
) {
    let tip_x = pad_l + 2.0;
    let w = if selected || dragging { 22.0 } else { 19.0 };
    let h = if selected || dragging { 16.0 } else { 14.0 };
    let left_x = tip_x - w;
    let half_h = h * 0.5;

    ctx.save();
    ctx.begin_path();
    ctx.move_to(left_x, y - half_h);
    ctx.line_to(tip_x - 6.0, y - half_h);
    ctx.line_to(tip_x, y);
    ctx.line_to(tip_x - 6.0, y + half_h);
    ctx.line_to(left_x, y + half_h);
    ctx.close_path();

    ctx.set_fill_style_str(color);
    ctx.fill();
    if selected || dragging {
        ctx.set_stroke_style_str("#ffffff");
        ctx.set_line_width(1.5);
        ctx.stroke();
    }

    ctx.set_fill_style_str("#060a10");
    ctx.set_font("800 10px ui-monospace, SFMono-Regular, Menlo, monospace");
    let _ = ctx.fill_text(&format!("{}", ch + 1), left_x + 4.5, y + 3.5);

    if clip_dir != 0 {
        ctx.set_fill_style_str(color);
        let arrow = if clip_dir > 0 { "▲" } else { "▼" };
        let ay = if clip_dir > 0 { y - half_h - 3.0 } else { y + half_h + 9.0 };
        let _ = ctx.fill_text(arrow, left_x + 3.5, ay);
    }
    ctx.restore();
}

fn draw_math_marker(ctx: &web_sys::CanvasRenderingContext2d, pad_l: f64, y: f64) {
    let tip_x = pad_l + 2.0;
    let left_x = tip_x - 19.0;
    ctx.save();
    ctx.begin_path();
    ctx.move_to(left_x, y - 7.0);
    ctx.line_to(tip_x - 6.0, y - 7.0);
    ctx.line_to(tip_x, y);
    ctx.line_to(tip_x - 6.0, y + 7.0);
    ctx.line_to(left_x, y + 7.0);
    ctx.close_path();
    ctx.set_fill_style_str(MATH_COLOR);
    ctx.fill();
    ctx.set_fill_style_str("#060a10");
    ctx.set_font("800 10px ui-monospace, SFMono-Regular, Menlo, monospace");
    let _ = ctx.fill_text("M", left_x + 3.5, y + 3.5);
    ctx.restore();
}

fn draw_trigger_handle(
    ctx: &web_sys::CanvasRenderingContext2d,
    right_edge_x: f64,
    y: f64,
    trig_ch: usize,
    offscreen: bool,
    dragging: bool,
) {
    let tip_x = right_edge_x - 2.0;
    let w = if dragging { 27.0 } else { 24.0 };
    let half_h = if dragging { 8.5 } else { 7.5 };
    let rx = tip_x + w;

    ctx.save();
    ctx.begin_path();
    ctx.move_to(tip_x, y);
    ctx.line_to(tip_x + 6.0, y - half_h);
    ctx.line_to(rx, y - half_h);
    ctx.line_to(rx, y + half_h);
    ctx.line_to(tip_x + 6.0, y + half_h);
    ctx.close_path();

    ctx.set_fill_style_str(if offscreen { "#9a3412" } else { "#f97316" });
    ctx.fill();
    if dragging {
        ctx.set_stroke_style_str("#ffffff");
        ctx.set_line_width(1.5);
        ctx.stroke();
    }
    ctx.set_fill_style_str("#060a10");
    ctx.set_font("800 9.5px ui-monospace, SFMono-Regular, Menlo, monospace");
    let _ = ctx.fill_text(&format!("T{}", trig_ch + 1), tip_x + 7.0, y + 3.3);
    ctx.restore();
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
    ctx.set_line_width(1.3);
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
    frame_ver: u64,
    fft_ch: usize,
    sc: &mut ScratchSpline,
    color: &str,
    ch_name: &str,
) {
    ctx.set_fill_style_str("#090e17");
    ctx.fill_rect(x0, y0, w, h);
    ctx.set_stroke_style_str("#1a2538");
    ctx.set_line_width(1.0);
    ctx.stroke_rect(x0, y0, w, h);

    if sc.fft_cache_ver != frame_ver || sc.fft_cache_ch != fft_ch || sc.fft_mags.is_empty() {
        sc.fft_bin_hz = compute_fft_dbv_into(
            samples_mv,
            sample_rate_hz,
            &mut sc.fft_re,
            &mut sc.fft_im,
            &mut sc.fft_mags,
        );
        sc.fft_cache_ver = frame_ver;
        sc.fft_cache_ch = fft_ch;
    }

    let bin_hz = sc.fft_bin_hz;
    let mags = &sc.fft_mags;
    if mags.len() < 4 {
        return;
    }

    let min_db = -90.0f64;
    let max_db = 10.0f64;

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

    // Subtle filled gradient under FFT spectrum
    let max_bin = mags.len() - 1;
    let mut peak_bin = 1usize;
    let mut peak_db = -120.0f32;
    ctx.set_stroke_style_str(color);
    ctx.set_line_width(1.5);
    ctx.begin_path();
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
        &format!(
            "FFT {ch_name} · Peak: {} ({peak_db:.1} dBV)",
            format_freq(peak_freq)
        ),
        x0 + 8.0,
        y0 + 13.0,
    );
}
