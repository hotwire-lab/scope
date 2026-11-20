use leptos::prelude::*;

use crate::protocol::types::BoardCapabilitiesFrame;
use crate::transport::serial::{ConnectionStatus, TransportKind, TransportSnapshot};

#[component]
pub fn TopToolbar(
    transport: RwSignal<TransportSnapshot>,
    caps: RwSignal<Option<BoardCapabilitiesFrame>>,
    running: RwSignal<bool>,
    fps: RwSignal<f64>,
    on_connect: Callback<()>,
    on_disconnect: Callback<()>,
    on_start_demo: Callback<()>,
    on_toggle_run: Callback<()>,
    on_single_shot: Callback<()>,
    drawer_open: RwSignal<bool>,
    on_toggle_drawer: Callback<()>,
) -> impl IntoView {
    let is_connected = move || transport.get().status == ConnectionStatus::Connected;
    let is_connecting = move || transport.get().status == ConnectionStatus::Connecting;

    let status_class = move || format!("status-pill status-{}", transport.get().status.as_class());
    let status_title = move || {
        let t = transport.get();
        t.error_message.unwrap_or(t.device_name)
    };
    let status_label = move || {
        let t = transport.get();
        if t.status == ConnectionStatus::Connected {
            if let Some(c) = caps.get() {
                if t.transport == TransportKind::Demo {
                    format!("{} (Demo)", c.board)
                } else {
                    c.board
                }
            } else {
                t.device_name
            }
        } else if t.status == ConnectionStatus::Connecting {
            "Connecting...".to_string()
        } else {
            "Disconnected".to_string()
        }
    };

    view! {
        <header class="top-toolbar">
            <div class="toolbar-row">
                <div class="brand-cluster">
                    <div class="brand-logo" aria-hidden="true">
                        <svg viewBox="0 0 24 24" width="20" height="20">
                            <path
                                d="M2 12 C5 4, 8 4, 11 12 C14 20, 17 20, 20 12 L22 12"
                                fill="none"
                                stroke="#facc15"
                                stroke-width="2.2"
                                stroke-linecap="round"
                            />
                        </svg>
                    </div>
                    <div class="brand-text">
                        <span class="brand-title">"HotWire Oscilloscope"</span>
                        <span class="brand-badge">"12-bit GDMA · Rust/WASM"</span>
                    </div>
                </div>

                <div class=status_class title=status_title>
                    <span class="status-dot" />
                    <span class="status-label">{status_label}</span>
                    {move || {
                        let t = transport.get();
                        if t.status == ConnectionStatus::Connected && !t.vid_pid.is_empty() {
                            Some(view! { <span class="status-vidpid">{t.vid_pid}</span> })
                        } else {
                            None
                        }
                    }}
                </div>

                <div class="control-spacer" />

                <div class="toolbar-actions">
                    {move || {
                        if is_connected() {
                            view! {
                                <span class="fps-badge">
                                    {move || format!("{:.1} FPS", fps.get())}
                                </span>
                                <button
                                    class=move || if running.get() { "btn btn-danger" } else { "btn btn-primary" }
                                    on:click=move |_| on_toggle_run.run(())
                                >
                                    {move || if running.get() { "■ Stop" } else { "▶ Run" }}
                                </button>
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| on_single_shot.run(())
                                    title="Capture single triggered frame"
                                >
                                    "Single"
                                </button>
                                <button
                                    class="btn btn-ghost"
                                    on:click=move |_| on_disconnect.run(())
                                >
                                    "Disconnect"
                                </button>
                            }
                            .into_any()
                        } else {
                            view! {
                                <button
                                    class="btn btn-primary"
                                    disabled=is_connecting
                                    on:click=move |_| on_connect.run(())
                                >
                                    {move || if is_connecting() { "Connecting..." } else { "Connect USB" }}
                                </button>
                                <button
                                    class="btn btn-accent"
                                    on:click=move |_| on_start_demo.run(())
                                    title="Run offline 4-channel oscilloscope simulator"
                                >
                                    "Demo"
                                </button>
                            }
                            .into_any()
                        }
                    }}

                    <button
                        class=move || if drawer_open.get() { "btn btn-secondary active" } else { "btn btn-secondary" }
                        on:click=move |_| on_toggle_drawer.run(())
                        title="Firmware Flasher & Pinout"
                    >
                        "⚡ Flasher"
                    </button>
                </div>
            </div>
        </header>
    }
}
