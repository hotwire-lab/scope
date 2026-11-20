use leptos::prelude::*;

use crate::protocol::types::{
    ChannelViewConfig, MathMode, CHANNEL_COLORS, MATH_COLOR, V_PER_DIV_STEPS_MV,
};

#[component]
#[allow(clippy::too_many_arguments)]
pub fn ChannelStrip(
    channel_mask: RwSignal<u8>,
    channel_cfgs: RwSignal<[ChannelViewConfig; 4]>,
    selected_ch: RwSignal<usize>,
    math_mode: RwSignal<MathMode>,
    on_toggle_channel: Callback<u8>,
    on_autoset: Callback<()>,
    on_stack: Callback<()>,
    on_clear_persist: Callback<()>,
) -> impl IntoView {
    let step_vdiv = move |delta: isize| {
        let ch = selected_ch.get_untracked().min(3);
        channel_cfgs.update(|cfgs| {
            let cur = cfgs[ch].vdiv_idx as isize;
            let next = (cur + delta).clamp(0, (V_PER_DIV_STEPS_MV.len() - 1) as isize) as usize;
            cfgs[ch].vdiv_idx = next;
        });
        on_clear_persist.run(());
    };

    let step_offset = move |delta_divs: f32| {
        let ch = selected_ch.get_untracked().min(3);
        channel_cfgs.update(|cfgs| {
            let next = ((cfgs[ch].offset_divs + delta_divs) * 4.0).round() / 4.0;
            cfgs[ch].offset_divs = next.clamp(-4.0, 4.0);
        });
        on_clear_persist.run(());
    };

    let zero_offset = move || {
        let ch = selected_ch.get_untracked().min(3);
        channel_cfgs.update(|cfgs| {
            cfgs[ch].offset_divs = 0.0;
        });
        on_clear_persist.run(());
    };

    let toggle_coupling = move || {
        let ch = selected_ch.get_untracked().min(3);
        channel_cfgs.update(|cfgs| {
            cfgs[ch].coupling = cfgs[ch].coupling.toggle();
        });
        on_clear_persist.run(());
    };

    let toggle_invert = move || {
        let ch = selected_ch.get_untracked().min(3);
        channel_cfgs.update(|cfgs| {
            cfgs[ch].invert = !cfgs[ch].invert;
        });
        on_clear_persist.run(());
    };

    let toggle_probe = move || {
        let ch = selected_ch.get_untracked().min(3);
        channel_cfgs.update(|cfgs| {
            cfgs[ch].probe_x10 = !cfgs[ch].probe_x10;
        });
        on_clear_persist.run(());
    };

    view! {
        <section class="channel-strip" aria-label="Oscilloscope Channel Controls">
            <div class="ch-badges-row">
                {(0..4usize)
                    .map(|ch| {
                        let color = CHANNEL_COLORS[ch];
                        view! {
                            <div
                                class=move || {
                                    let active = (channel_mask.get() & (1 << ch)) != 0;
                                    let sel = selected_ch.get() == ch;
                                    match (active, sel) {
                                        (true, true) => "ch-badge active selected",
                                        (true, false) => "ch-badge active",
                                        _ => "ch-badge",
                                    }
                                }
                                style=move || {
                                    let active = (channel_mask.get() & (1 << ch)) != 0;
                                    let sel = selected_ch.get() == ch;
                                    if active && sel {
                                        format!("border-color: {color}; box-shadow: 0 0 0 1px {color}, 0 0 12px color-mix(in srgb, {color} 28%, transparent);")
                                    } else if active {
                                        format!("border-color: color-mix(in srgb, {color} 55%, #283244);")
                                    } else {
                                        String::new()
                                    }
                                }
                                on:click=move |_| {
                                    let active = (channel_mask.get_untracked() & (1 << ch)) != 0;
                                    if !active {
                                        on_toggle_channel.run(ch as u8);
                                    }
                                    selected_ch.set(ch);
                                }
                                title="Click to select channel · Drag tag on left of scope to move vertically"
                            >
                                <button
                                    type="button"
                                    class="ch-power-dot"
                                    style=move || {
                                        if (channel_mask.get() & (1 << ch)) != 0 {
                                            format!("background: {color}; box-shadow: 0 0 6px {color};")
                                        } else {
                                            "background: #334155;".to_string()
                                        }
                                    }
                                    title="Toggle channel On/Off"
                                    on:click=move |ev| {
                                        ev.stop_propagation();
                                        on_toggle_channel.run(ch as u8);
                                    }
                                />
                                <span class="ch-badge-id" style=format!("color: {color};")>
                                    {format!("CH{}", ch + 1)}
                                </span>
                                {move || {
                                    let cfg = channel_cfgs.get()[ch];
                                    let active = (channel_mask.get() & (1 << ch)) != 0;
                                    if active {
                                        view! {
                                            <span class="ch-badge-vdiv">{cfg.format_vdiv()}</span>
                                            <span class="ch-badge-tag">{cfg.coupling.label()}</span>
                                            {if cfg.probe_x10 {
                                                Some(view! { <span class="ch-badge-tag">"10×"</span> })
                                            } else {
                                                None
                                            }}
                                            {if cfg.invert {
                                                Some(view! { <span class="ch-badge-tag tag-inv">"INV"</span> })
                                            } else {
                                                None
                                            }}
                                            <span class="ch-badge-pos">
                                                {format!("{:+.2}d", cfg.offset_divs)}
                                            </span>
                                        }
                                        .into_any()
                                    } else {
                                        view! { <span class="ch-badge-off">"OFF"</span> }.into_any()
                                    }
                                }}
                            </div>
                        }
                    })
                    .collect_view()}

                {move || {
                    let mm = math_mode.get();
                    if mm != MathMode::Off {
                        Some(view! {
                            <div
                                class="ch-badge active"
                                style=format!("border-color: {MATH_COLOR};")
                            >
                                <span class="ch-badge-id" style=format!("color: {MATH_COLOR};")>"M"</span>
                                <span class="ch-badge-vdiv">{mm.label()}</span>
                            </div>
                        })
                    } else {
                        None
                    }
                }}
            </div>

            <div class="ch-editor-row">
                <span
                    class="ch-editor-label"
                    style=move || format!("color: {};", CHANNEL_COLORS[selected_ch.get().min(3)])
                >
                    {move || format!("CH{} Vertical:", selected_ch.get() + 1)}
                </span>

                <div class="knob-group" title="Volts per division (1-2-5 sequence)">
                    <span class="knob-caption">"V/div"</span>
                    <button type="button" class="btn btn-xs btn-secondary" on:click=move |_| step_vdiv(-1)>"−"</button>
                    <span class="knob-val">
                        {move || channel_cfgs.get()[selected_ch.get().min(3)].format_vdiv()}
                    </span>
                    <button type="button" class="btn btn-xs btn-secondary" on:click=move |_| step_vdiv(1)>"+"</button>
                </div>

                <div class="knob-group" title="Vertical Position (or drag channel tag 1►..4► directly on scope)">
                    <span class="knob-caption">"Pos"</span>
                    <button type="button" class="btn btn-xs btn-secondary" on:click=move |_| step_offset(-0.5)>"▼"</button>
                    <span class="knob-val">
                        {move || format!("{:+.2}d", channel_cfgs.get()[selected_ch.get().min(3)].offset_divs)}
                    </span>
                    <button type="button" class="btn btn-xs btn-secondary" on:click=move |_| step_offset(0.5)>"▲"</button>
                    <button type="button" class="btn btn-xs btn-ghost" on:click=move |_| zero_offset() title="Center ground (0 div)">"0"</button>
                </div>

                <button
                    type="button"
                    class=move || {
                        let c = channel_cfgs.get()[selected_ch.get().min(3)].coupling;
                        if c.label() == "AC" { "btn btn-xs btn-accent active" } else { "btn btn-xs btn-secondary" }
                    }
                    on:click=move |_| toggle_coupling()
                    title="DC or AC coupling (AC removes DC offset to zoom in on ripple)"
                >
                    {move || channel_cfgs.get()[selected_ch.get().min(3)].coupling.label()}
                </button>

                <button
                    type="button"
                    class=move || {
                        if channel_cfgs.get()[selected_ch.get().min(3)].invert {
                            "btn btn-xs btn-accent active"
                        } else {
                            "btn btn-xs btn-secondary"
                        }
                    }
                    on:click=move |_| toggle_invert()
                    title="Invert channel polarity"
                >
                    "INV"
                </button>

                <button
                    type="button"
                    class=move || {
                        if channel_cfgs.get()[selected_ch.get().min(3)].probe_x10 {
                            "btn btn-xs btn-accent active"
                        } else {
                            "btn btn-xs btn-secondary"
                        }
                    }
                    on:click=move |_| toggle_probe()
                    title="Probe attenuation multiplier (1x direct / 10x divider)"
                >
                    {move || if channel_cfgs.get()[selected_ch.get().min(3)].probe_x10 { "10×" } else { "1×" }}
                </button>

                <div class="control-divider-sm" />

                <button
                    type="button"
                    class="btn btn-xs btn-auto"
                    on:click=move |_| on_autoset.run(())
                    title="Auto-Set: optimal 1-2-5 V/div, clean channel stacking & centered trigger"
                >
                    "AUTO"
                </button>
                <button
                    type="button"
                    class="btn btn-xs btn-secondary"
                    on:click=move |_| on_stack.run(())
                    title="Stack active channels cleanly into separate vertical lanes"
                >
                    "Stack"
                </button>
            </div>
        </section>
    }
}
