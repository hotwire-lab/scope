use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use crate::protocol::types::BoardCapabilitiesFrame;
use crate::transport::serial::SerialTransport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoardId {
    Esp32S3,
    Esp32C6,
}

impl BoardId {
    fn as_str(&self) -> &'static str {
        match self {
            BoardId::Esp32S3 => "esp32s3",
            BoardId::Esp32C6 => "esp32c6",
        }
    }

    fn from_str(s: &str) -> Self {
        if s == "esp32c6" {
            BoardId::Esp32C6
        } else {
            BoardId::Esp32S3
        }
    }

    fn label(&self) -> &'static str {
        match self {
            BoardId::Esp32S3 => "Seeed Studio XIAO ESP32-S3",
            BoardId::Esp32C6 => "Seeed Studio XIAO ESP32-C6",
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            BoardId::Esp32S3 => {
                "4-Ch ADC1 (D0–D3 / GPIO1–4), Dual-Core GDMA, 83.3 kSPS spec + up to 1 MSPS Turbo OC, Test Gen on D6"
            }
            BoardId::Esp32C6 => {
                "3-Ch ADC1 (D0–D2 / GPIO0–2), GDMA Continuous up to 83.3 kSPS, eFuse Curve-Fitting Calibration, Test Gen on D6"
            }
        }
    }
}

#[component]
pub fn BottomDrawer(
    open: RwSignal<bool>,
    on_close: Callback<()>,
    caps: RwSignal<Option<BoardCapabilitiesFrame>>,
    serial_transport: StoredValue<SerialTransport, LocalStorage>,
) -> impl IntoView {
    let selected_board = RwSignal::new(BoardId::Esp32S3);
    let flash_logs = RwSignal::new(Vec::<String>::new());
    let flash_progress = RwSignal::new(0u32);
    let flashing = RwSignal::new(false);
    let flash_error = RwSignal::new(None::<String>);
    let show_custom_bin = RwSignal::new(false);
    let selected_file_name = RwSignal::new(None::<String>);
    let selected_file = StoredValue::new_local(None::<web_sys::File>);
    let flash_offset = RwSignal::new("full".to_string());
    let log_div_ref = NodeRef::<html::Div>::new();

    Effect::new(move |_| {
        if let Some(c) = caps.get() {
            if c.board.to_uppercase().contains("C6") {
                selected_board.set(BoardId::Esp32C6);
            } else {
                selected_board.set(BoardId::Esp32S3);
            }
        }
    });

    let start_flash = move |force_other_port: bool| {
        if flashing.get_untracked() {
            return;
        }
        flashing.set(true);
        flash_error.set(None);
        flash_progress.set(0);
        flash_logs.set(Vec::new());

        let board = selected_board.get_untracked();
        let offset = flash_offset.get_untracked();
        let custom = selected_file.get_value();
        let transport = serial_transport.get_value();

        spawn_local(async move {
            let res = transport
                .flash_firmware(
                    board.as_str(),
                    force_other_port,
                    &offset,
                    custom.as_ref(),
                    move |line| {
                        flash_logs.update(|logs| {
                            logs.push(line);
                            if logs.len() > 140 {
                                let drain = logs.len() - 140;
                                logs.drain(0..drain);
                            }
                        });
                        if let Some(el) = log_div_ref.get_untracked() {
                            el.set_scroll_top(el.scroll_height());
                        }
                    },
                    move |pct| flash_progress.set(pct),
                )
                .await;

            if let Err(err) = res {
                flash_error.set(Some(format!(
                    "{err} (If USB re-enumerated, hold BOOT + tap RESET and click \"Select Other Port\")"
                )));
                flash_logs.update(|logs| logs.push(format!("ERROR: {err}")));
            }
            flashing.set(false);
        });
    };

    view! {
        <div class=move || if open.get() { "drawer-backdrop open" } else { "drawer-backdrop" }>
            <aside class="bottom-drawer">
                <div class="drawer-header">
                    <h3>"⚡ Firmware Flasher & Hardware Pinout"</h3>
                    <button class="btn btn-sm btn-ghost" on:click=move |_| on_close.run(())>"✕ Close"</button>
                </div>

                <div class="drawer-body">
                    <div class="drawer-col">
                        <h4>"Web Flasher"</h4>
                        <div class="form-row">
                            <label for="board-select">"Target Board:"</label>
                            <select
                                id="board-select"
                                class="ctrl-select"
                                prop:value=move || selected_board.get().as_str()
                                on:change=move |ev| {
                                    selected_board.set(BoardId::from_str(&event_target_value(&ev)));
                                }
                            >
                                <option value="esp32s3">{BoardId::Esp32S3.label()}</option>
                                <option value="esp32c6">{BoardId::Esp32C6.label()}</option>
                            </select>
                        </div>
                        <p class="drawer-hint">{move || selected_board.get().hint()}</p>

                        <div class="form-row">
                            <label>
                                <input
                                    type="checkbox"
                                    prop:checked=move || show_custom_bin.get()
                                    on:change=move |ev| {
                                        let checked = ev.target()
                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                            .map(|i| i.checked())
                                            .unwrap_or(false);
                                        show_custom_bin.set(checked);
                                        if checked {
                                            flash_offset.set("0x10000".to_string());
                                        } else {
                                            flash_offset.set("full".to_string());
                                            selected_file.set_value(None);
                                            selected_file_name.set(None);
                                        }
                                    }
                                />
                                " Flash custom .bin file"
                            </label>
                        </div>

                        {move || {
                            if show_custom_bin.get() {
                                Some(view! {
                                    <div class="form-row">
                                        <input
                                            type="file"
                                            accept=".bin"
                                            on:change=move |ev| {
                                                let file = ev.target()
                                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                    .and_then(|i| i.files())
                                                    .and_then(|fl| fl.get(0));
                                                selected_file_name.set(file.as_ref().map(|f| f.name()));
                                                selected_file.set_value(file);
                                            }
                                        />
                                        <select
                                            class="ctrl-select ctrl-select-sm"
                                            prop:value=move || flash_offset.get()
                                            on:change=move |ev| flash_offset.set(event_target_value(&ev))
                                        >
                                            <option value="0x10000">"App @ 0x10000"</option>
                                            <option value="0x0">"Merged @ 0x0"</option>
                                        </select>
                                        {move || selected_file_name.get().map(|name| view! {
                                            <span class="mono-readout">{name}</span>
                                        })}
                                    </div>
                                })
                            } else {
                                None
                            }
                        }}

                        <div class="inline-controls">
                            <button
                                class="btn btn-primary"
                                disabled=move || flashing.get()
                                on:click=move |_| start_flash(false)
                            >
                                {move || if flashing.get() { "Flashing..." } else { "Flash Firmware" }}
                            </button>
                            <button
                                class="btn btn-secondary"
                                disabled=move || flashing.get()
                                on:click=move |_| start_flash(true)
                            >
                                "Select Other Port"
                            </button>
                        </div>

                        <div class="progress-bar">
                            <div class="progress-fill" style=move || format!("width: {}%;", flash_progress.get()) />
                        </div>

                        {move || flash_error.get().map(|e| view! { <div class="flash-error">{e}</div> })}

                        <div class="flash-log" node_ref=log_div_ref>
                            {move || {
                                flash_logs
                                    .get()
                                    .into_iter()
                                    .map(|l| view! { <div>{l}</div> })
                                    .collect_view()
                            }}
                        </div>
                    </div>

                    <div class="drawer-col">
                        <h4>"ADC Pinout & Safety (0 – 3.3 V)"</h4>
                        <table class="pin-table">
                            <thead>
                                <tr>
                                    <th>"Scope Ch"</th>
                                    <th>"XIAO Pin"</th>
                                    <th>"ESP32-S3"</th>
                                    <th>"ESP32-C6"</th>
                                </tr>
                            </thead>
                            <tbody>
                                <tr><td style="color:#facc15">"CH1"</td><td>"D0 / A0"</td><td>"GPIO1 (ADC1_CH0)"</td><td>"GPIO0 (ADC1_CH0)"</td></tr>
                                <tr><td style="color:#38bdf8">"CH2"</td><td>"D1 / A1"</td><td>"GPIO2 (ADC1_CH1)"</td><td>"GPIO1 (ADC1_CH1)"</td></tr>
                                <tr><td style="color:#f472b6">"CH3"</td><td>"D2 / A2"</td><td>"GPIO3 (ADC1_CH2)"</td><td>"GPIO2 (ADC1_CH2)"</td></tr>
                                <tr><td style="color:#4ade80">"CH4"</td><td>"D3 / A3"</td><td>"GPIO4 (ADC1_CH3)"</td><td>"— (3-ch max)"</td></tr>
                                <tr><td>"Test Gen"</td><td>"D6"</td><td>"GPIO43 (LEDC PWM)"</td><td>"GPIO16 (LEDC PWM)"</td></tr>
                            </tbody>
                        </table>
                        <p class="drawer-hint">
                            "⚠ Never exceed 3.3 V or apply negative voltages directly to XIAO ADC pins. Use a 10:1 resistor divider or AC-coupling bias network for higher/bipolar signals."
                        </p>
                    </div>
                </div>
            </aside>
        </div>
    }
}
