use hotwire_core::flasher::{FirmwareProfile, FlasherPanel};
use hotwire_core::protocol::{BoardIdentity, FirmwareMismatch, ToolId};
use hotwire_core::ui::BottomSheet;
use leptos::prelude::*;

use crate::transport::serial::SerialTransport;

fn scope_firmware_profiles() -> Vec<FirmwareProfile> {
    FirmwareProfile::standard_profiles(ToolId::Oscilloscope)
}

#[component]
pub fn BottomDrawer(
    open: RwSignal<bool>,
    on_close: Callback<()>,
    identity: RwSignal<Option<BoardIdentity>>,
    mismatch: RwSignal<Option<FirmwareMismatch>>,
    serial_transport: StoredValue<SerialTransport, LocalStorage>,
) -> impl IntoView {
    view! {
        <BottomSheet
            open=open
            on_close=on_close
            title="⚡ Firmware Flasher & Hardware Pinout"
        >
            <div class="drawer-col">
                <h4>"Web Flasher"</h4>
                <FlasherPanel
                    profiles=scope_firmware_profiles()
                    transport=serial_transport
                    expected_tool=ToolId::Oscilloscope
                    identity=identity
                    mismatch=mismatch
                />
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
        </BottomSheet>
    }
}
