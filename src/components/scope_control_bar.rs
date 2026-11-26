use leptos::prelude::*;

use crate::protocol::codec::resolve_atten_preset;
use crate::protocol::types::*;

#[component]
#[allow(clippy::too_many_arguments)]
pub fn ScopeControlBar(
    controls_open: RwSignal<bool>,
    board_caps: RwSignal<Option<BoardCapabilitiesFrame>>,
    channel_mask: RwSignal<u8>,
    rate_code: RwSignal<u8>,
    atten_code: RwSignal<u8>,
    depth_code: RwSignal<u8>,
    trig_mode: RwSignal<u8>,
    trig_edge: RwSignal<u8>,
    trig_channel: RwSignal<u8>,
    trig_level_mv: RwSignal<u16>,
    test_gen_code: RwSignal<u8>,
    show_fft: RwSignal<bool>,
    show_cursors: RwSignal<bool>,
    fft_channel: RwSignal<u8>,
    persistence_mode: RwSignal<PersistenceMode>,
    math_mode: RwSignal<MathMode>,
    display_mode: RwSignal<DisplayMode>,
    on_clear_persist: Callback<()>,
    on_apply_cfg: Callback<()>,
    on_apply_trig: Callback<()>,
    on_apply_run: Callback<()>,
    on_reset_view: Callback<()>,
) -> impl IntoView {
    let max_channels = move || {
        board_caps
            .get()
            .map(|c| usize::from(c.max_channels.clamp(1, 4)))
            .unwrap_or(4)
    };
    let max_rate_hz = move || {
        board_caps
            .get()
            .map(|c| c.max_rate_hz)
            .unwrap_or(u32::MAX)
    };

    view! {
        <section class=move || {
            if controls_open.get() {
                "control-bar mobile-open"
            } else {
                "control-bar"
            }
        }>
            <div class="control-group">
                <label class="group-label" for="rate-select">"Sample Rate (Total)"</label>
                <select
                    id="rate-select"
                    class="ctrl-select"
                    prop:value=move || rate_code.get().to_string()
                    on:change=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<u8>() {
                            rate_code.set(v);
                            on_apply_cfg.run(());
                        }
                    }
                >
                    {move || {
                        let max_hz = max_rate_hz();
                        SCOPE_RATE_PRESETS
                            .iter()
                            .filter(|p| p.total_rate_hz <= max_hz)
                            .map(|p| {
                                view! {
                                    <option value=p.code.to_string()>{p.label}</option>
                                }
                            })
                            .collect_view()
                    }}
                </select>
            </div>

            <div class="control-group">
                <label class="group-label" for="atten-select">"Input Range"</label>
                <select
                    id="atten-select"
                    class="ctrl-select"
                    prop:value=move || atten_code.get().to_string()
                    on:change=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<u8>() {
                            atten_code.set(v);
                            let max_mv = resolve_atten_preset(v).full_scale_mv;
                            if trig_level_mv.get_untracked() > max_mv {
                                trig_level_mv.set(max_mv / 2);
                                on_apply_trig.run(());
                            }
                            on_apply_cfg.run(());
                        }
                    }
                >
                    {SCOPE_ATTEN_PRESETS
                        .iter()
                        .map(|p| {
                            view! {
                                <option value=p.code.to_string()>{p.label}</option>
                            }
                        })
                        .collect_view()}
                </select>
            </div>

            <div class="control-group">
                <label class="group-label" for="depth-select">"Depth"</label>
                <select
                    id="depth-select"
                    class="ctrl-select ctrl-select-sm"
                    prop:value=move || depth_code.get().to_string()
                    on:change=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<u8>() {
                            depth_code.set(v);
                            on_apply_cfg.run(());
                        }
                    }
                >
                    {SCOPE_DEPTH_PRESETS
                        .iter()
                        .map(|p| {
                            view! {
                                <option value=p.code.to_string()>{p.label}</option>
                            }
                        })
                        .collect_view()}
                </select>
            </div>

            <div class="control-divider" />

            <div class="control-group">
                <span class="group-label">"Trigger"</span>
                <div class="inline-controls">
                    <select
                        class="ctrl-select ctrl-select-sm"
                        prop:value=move || trig_mode.get().to_string()
                        on:change=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u8>() {
                                trig_mode.set(v);
                                on_apply_trig.run(());
                            }
                        }
                    >
                        <option value="0">"Auto"</option>
                        <option value="1">"Normal"</option>
                        <option value="3">"Free Run"</option>
                    </select>
                    <select
                        class="ctrl-select ctrl-select-sm"
                        prop:value=move || trig_channel.get().to_string()
                        on:change=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u8>() {
                                trig_channel.set(v);
                                on_apply_trig.run(());
                            }
                        }
                    >
                        {move || {
                            let mask = channel_mask.get();
                            (0..max_channels())
                                .filter(move |ch| mask & (1u8 << ch) != 0)
                                .map(|ch| {
                                    view! { <option value=ch.to_string()>{format!("CH{}", ch + 1)}</option> }
                                })
                                .collect_view()
                        }}
                    </select>
                    <button
                        class="btn btn-sm btn-secondary"
                        title="Toggle Rising / Falling edge"
                        on:click=move |_| {
                            trig_edge.update(|e| *e = if *e == 0 { 1 } else { 0 });
                            on_apply_trig.run(());
                        }
                    >
                        {move || if trig_edge.get() == 0 { "↗ Rise" } else { "↘ Fall" }}
                    </button>
                    <input
                        type="range"
                        class="trig-slider"
                        min="25"
                        max=move || resolve_atten_preset(atten_code.get()).full_scale_mv.to_string()
                        step="25"
                        prop:value=move || trig_level_mv.get().to_string()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u16>() {
                                trig_level_mv.set(v);
                                on_apply_trig.run(());
                            }
                        }
                    />
                    <span class="mono-readout">
                        {move || format!("{:.2}V", trig_level_mv.get() as f32 / 1000.0)}
                    </span>
                </div>
            </div>

            <div class="control-divider" />

            <div class="control-group">
                <label class="group-label" for="testgen-select">"Test Gen (D6)"</label>
                <select
                    id="testgen-select"
                    class="ctrl-select ctrl-select-sm"
                    prop:value=move || test_gen_code.get().to_string()
                    on:change=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<u8>() {
                            test_gen_code.set(v);
                            on_apply_run.run(());
                        }
                    }
                >
                    {TEST_GEN_PRESETS
                        .iter()
                        .map(|p| {
                            view! {
                                <option value=p.code.to_string()>{p.label}</option>
                            }
                        })
                        .collect_view()}
                </select>
            </div>

            <div class="control-group">
                <span class="group-label">"Analysis"</span>
                <div class="inline-controls">
                    <button
                        class=move || if show_fft.get() { "btn btn-sm btn-accent active" } else { "btn btn-sm btn-secondary" }
                        on:click=move |_| show_fft.update(|v| *v = !*v)
                    >
                        "FFT"
                    </button>
                    {move || {
                        if show_fft.get() {
                            Some(view! {
                                <select
                                    class="ctrl-select ctrl-select-sm"
                                    prop:value=move || fft_channel.get().to_string()
                                    on:change=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse::<u8>() {
                                            fft_channel.set(v);
                                        }
                                    }
                                >
                                    {move || {
                                        let mask = channel_mask.get();
                                        (0..max_channels())
                                            .filter(move |ch| mask & (1u8 << ch) != 0)
                                            .map(|ch| {
                                                view! { <option value=ch.to_string()>{format!("CH{}", ch + 1)}</option> }
                                            })
                                            .collect_view()
                                    }}
                                </select>
                            })
                        } else {
                            None
                        }
                    }}
                    <button
                        class=move || if show_cursors.get() { "btn btn-sm btn-accent active" } else { "btn btn-sm btn-secondary" }
                        on:click=move |_| show_cursors.update(|v| *v = !*v)
                    >
                        "Cursors"
                    </button>
                    <button
                        class=move || if display_mode.get() == DisplayMode::Xy { "btn btn-sm btn-accent active" } else { "btn btn-sm btn-secondary" }
                        on:click=move |_| {
                            display_mode.update(|m| {
                                *m = if *m == DisplayMode::Yt { DisplayMode::Xy } else { DisplayMode::Yt };
                            });
                            on_clear_persist.run(());
                        }
                        title="Toggle Y-T (time domain) vs X-Y (Lissajous CH1 vs CH2)"
                    >
                        {move || if display_mode.get() == DisplayMode::Xy { "X-Y" } else { "Y-T" }}
                    </button>
                    <select
                        class="ctrl-select ctrl-select-sm"
                        title="Math channel (purple trace)"
                        prop:value=move || math_mode.get().as_code()
                        on:change=move |ev| {
                            math_mode.set(MathMode::from_code(&event_target_value(&ev)));
                            on_clear_persist.run(());
                        }
                    >
                        <option value="off">"Math: Off"</option>
                        <option value="sub">"Math: CH1−CH2"</option>
                        <option value="add">"Math: CH1+CH2"</option>
                    </select>
                    <select
                        class="ctrl-select ctrl-select-sm"
                        title="Waveform Persistence / Afterglow"
                        prop:value=move || persistence_mode.get().as_code()
                        on:change=move |ev| {
                            persistence_mode.set(PersistenceMode::from_code(&event_target_value(&ev)));
                        }
                    >
                        <option value="off">"Persist: Off"</option>
                        <option value="500ms">"Persist: 0.5 s"</option>
                        <option value="2s">"Persist: 2 s"</option>
                        <option value="5s">"Persist: 5 s"</option>
                        <option value="inf">"Persist: ∞"</option>
                    </select>
                    {move || {
                        if persistence_mode.get() != PersistenceMode::Off {
                            Some(view! {
                                <button
                                    class="btn btn-sm btn-secondary"
                                    on:click=move |_| on_clear_persist.run(())
                                    title="Clear accumulated persistence trails"
                                >
                                    "Clear"
                                </button>
                            })
                        } else {
                            None
                        }
                    }}
                    <button
                        class="btn btn-sm btn-ghost"
                        on:click=move |_| on_reset_view.run(())
                        title="Reset horizontal zoom & pan"
                    >
                        "Reset Zoom"
                    </button>
                </div>
            </div>
        </section>
    }
}
