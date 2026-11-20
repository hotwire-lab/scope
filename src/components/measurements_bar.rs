use std::sync::Arc;

use leptos::prelude::*;

use crate::dsp::scope_dsp::{format_freq, format_time_us, format_voltage_mv, measure_channel};
use crate::protocol::types::{ScopeFrame, CHANNEL_COLORS, CHANNEL_NAMES};

#[component]
pub fn MeasurementsBar(
    latest_frame: RwSignal<Option<Arc<ScopeFrame>>>,
    show_cursors: RwSignal<bool>,
    cursor_a_norm: RwSignal<f64>,
    cursor_b_norm: RwSignal<f64>,
    zoom: RwSignal<f64>,
    pan_norm: RwSignal<f64>,
) -> impl IntoView {
    view! {
        <section class="measurements-bar">
            {move || {
                let Some(frame) = latest_frame.get() else {
                    return view! {
                        <div class="meas-empty">
                            "Click " <strong>"Demo"</strong> " or " <strong>"Connect USB"</strong> " to start capturing ADC waveforms."
                        </div>
                    }
                    .into_any();
                };

                let total_us = (frame.samples_per_ch as f64) / (frame.sample_rate_hz as f64) * 1_000_000.0;
                let visible_us = total_us / zoom.get().max(1.0);
                let us_per_div = visible_us / 10.0;

                let cards = (0..4usize)
                    .filter(|&ch| !frame.channels_mv[ch].is_empty())
                    .map(|ch| {
                        let m = measure_channel(&frame.channels_mv[ch], frame.sample_rate_hz);
                        let color = CHANNEL_COLORS[ch];
                        let name = CHANNEL_NAMES[ch];
                        let freq_str = m
                            .freq_hz
                            .map(format_freq)
                            .unwrap_or_else(|| "—".to_string());
                        let duty_str = m
                            .duty_pct
                            .map(|d| format!("{d:.1}%"))
                            .unwrap_or_else(|| "—".to_string());

                        view! {
                            <div class="meas-card" style=format!("border-left: 3px solid {color};")>
                                <div class="meas-card-title" style=format!("color: {color};")>{name}</div>
                                <div class="meas-grid">
                                    <span>"Vpp "<strong>{format_voltage_mv(m.vpp_mv)}</strong></span>
                                    <span>"Vrms "<strong>{format_voltage_mv(m.vrms_mv)}</strong></span>
                                    <span>"Vavg "<strong>{format_voltage_mv(m.vavg_mv)}</strong></span>
                                    <span>"Freq "<strong>{freq_str}</strong></span>
                                    <span>"Min/Max "<strong>{format!("{:.2}/{:.2}V", m.vmin_mv / 1000.0, m.vmax_mv / 1000.0)}</strong></span>
                                    <span>"Duty "<strong>{duty_str}</strong></span>
                                </div>
                            </div>
                        }
                    })
                    .collect_view();

                let cursor_card = if show_cursors.get() {
                    let span = 1.0 / zoom.get().max(1.0);
                    let start = pan_norm.get().clamp(0.0, (1.0 - span).max(0.0));
                    let ta_us = (start + cursor_a_norm.get() * span) * total_us;
                    let tb_us = (start + cursor_b_norm.get() * span) * total_us;
                    let dt_us = (tb_us - ta_us).abs();
                    let inv_dt = if dt_us > 0.001 {
                        format_freq((1_000_000.0 / dt_us) as f32)
                    } else {
                        "—".to_string()
                    };
                    // Sample voltage at cursors A & B on trigger channel (or first active channel)
                    let ref_ch = if !frame.channels_mv[frame.trig_channel as usize].is_empty() {
                        frame.trig_channel as usize
                    } else {
                        (0..4).find(|&c| !frame.channels_mv[c].is_empty()).unwrap_or(0)
                    };
                    let samples = &frame.channels_mv[ref_ch];
                    let (va_str, vb_str, dv_str) = if !samples.is_empty() {
                        let max_i = samples.len() - 1;
                        let ia = ((start + cursor_a_norm.get() * span).clamp(0.0, 1.0) * max_i as f64).round() as usize;
                        let ib = ((start + cursor_b_norm.get() * span).clamp(0.0, 1.0) * max_i as f64).round() as usize;
                        let va = samples[ia];
                        let vb = samples[ib];
                        (
                            format_voltage_mv(va),
                            format_voltage_mv(vb),
                            format_voltage_mv((vb - va).abs()),
                        )
                    } else {
                        ("—".to_string(), "—".to_string(), "—".to_string())
                    };
                    view! {
                        <div class="meas-card meas-card-cursor">
                            <div class="meas-card-title">{format!("Cursors A / B ({})", CHANNEL_NAMES[ref_ch])}</div>
                            <div class="meas-grid">
                                <span>"Δt "<strong>{format_time_us(dt_us as f32)}</strong></span>
                                <span>"1/Δt "<strong>{inv_dt}</strong></span>
                                <span>"ΔV "<strong>{dv_str}</strong></span>
                                <span>"VA "<strong>{va_str}</strong></span>
                                <span>"VB "<strong>{vb_str}</strong></span>
                                <span>"Div "<strong>{format_time_us(us_per_div as f32)}</strong></span>
                            </div>
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <div class="meas-card meas-card-meta">
                            <div class="meas-card-title">"Acquisition"</div>
                            <div class="meas-grid">
                                <span>"Timebase "<strong>{format!("{}/div", format_time_us(us_per_div as f32))}</strong></span>
                                <span>"Rate/Ch "<strong>{format_freq(frame.sample_rate_hz as f32)}</strong></span>
                                <span>"Depth "<strong>{format!("{} pts", frame.samples_per_ch)}</strong></span>
                                <span>
                                    "Mode "
                                    <strong class=if frame.overclocked { "text-warn" } else { "text-ok" }>
                                        {if frame.overclocked { "Turbo OC" } else if frame.calibrated_mv { "eFuse Cal" } else { "Raw 12b" }}
                                    </strong>
                                </span>
                            </div>
                        </div>
                    }
                    .into_any()
                };

                view! {
                    <div class="meas-row">
                        {cards}
                        {cursor_card}
                    </div>
                }
                .into_any()
            }}
        </section>
    }
}
