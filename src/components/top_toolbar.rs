use hotwire_core::protocol::{BoardIdentity, ToolId};
use hotwire_core::ui::HotWireToolbar;
use leptos::prelude::*;

use crate::transport::serial::TransportSnapshot;

#[component]
pub fn TopToolbar(
    transport: RwSignal<TransportSnapshot>,
    identity: RwSignal<Option<BoardIdentity>>,
    running: RwSignal<bool>,
    fps: RwSignal<f64>,
    on_connect: Callback<()>,
    on_disconnect: Callback<()>,
    on_start_demo: Callback<()>,
    on_toggle_run: Callback<()>,
    on_single_shot: Callback<()>,
    drawer_open: RwSignal<bool>,
    on_toggle_drawer: Callback<()>,
    controls_open: RwSignal<bool>,
) -> impl IntoView {
    let is_active = move || transport.get().status.is_active();

    view! {
        <HotWireToolbar
            active_tool=ToolId::Oscilloscope
            subtitle="12-bit GDMA · Rust/WASM"
            transport=transport
            identity=identity
            on_connect=on_connect
            on_disconnect=on_disconnect
            on_start_demo=on_start_demo
            flasher_open=drawer_open
            on_toggle_flasher=on_toggle_drawer
        >
            {move || {
                is_active().then(|| {
                    view! {
                        <span class="fps-badge">
                            {move || format!("{:.1} FPS", fps.get())}
                        </span>
                    }
                })
            }}
            <button
                type="button"
                class=move || if running.get() { "btn btn-danger" } else { "btn btn-primary" }
                disabled=move || !is_active()
                on:click=move |_| on_toggle_run.run(())
            >
                {move || if running.get() { "■ Stop" } else { "▶ Run" }}
            </button>
            <button
                type="button"
                class="btn btn-secondary"
                disabled=move || !is_active()
                on:click=move |_| on_single_shot.run(())
                title="Capture single triggered frame"
            >
                "Single"
            </button>
            <button
                type="button"
                class=move || {
                    if controls_open.get() {
                        "btn btn-secondary mobile-only-btn"
                    } else {
                        "btn btn-accent mobile-only-btn"
                    }
                }
                aria-expanded=move || controls_open.get().to_string()
                on:click=move |_| controls_open.update(|v| *v = !*v)
                title="Show or hide acquisition & trigger controls"
            >
                {move || if controls_open.get() { "Controls ▴" } else { "Controls ▾" }}
            </button>
        </HotWireToolbar>
    }
}
