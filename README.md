# HotWire Oscilloscope — 4-Channel 12-bit GDMA ADC Scope & FFT

Zero-install browser **Digital Storage Oscilloscope (DSO)** and **FFT Spectrum Analyzer** Progressive Web App (PWA) built in **Rust + WebAssembly (Leptos 0.7 CSR + Trunk)** for **Seeed Studio XIAO ESP32-S3** and **XIAO ESP32-C6** over **Web Serial API** (Desktop) and **WebUSB API** (Android & Desktop fallback).

* **Live App:** https://kavierim.github.io/HotWire-Oscilloscope/
* **Repository:** https://github.com/kavierim/HotWire-Oscilloscope
* **Offline Demo:** Built-in 4-channel signal generator (`Demo` button) works without hardware.

## High-Speed ADC Capture Architecture

ESP32-S3 and ESP32-C6 internal SAR ADCs have important hardware limits that must be engineered around for a responsive, low-jitter oscilloscope:

1. **GDMA Continuous Streaming (`esp_adc/adc_continuous.h`):**
   - Uses General-Purpose DMA directly into internal SRAM circular ring buffers with zero CPU polling overhead and hardware timer pacing.
   - Wi-Fi / BLE radios are kept off so there is no RF switching noise or ADC2 contention (all channels reside on **ADC1**).
2. **O(1) eFuse Curve-Fitting Calibration LUT:**
   - ESP32 SAR ADCs are non-linear near the rails. At startup (and attenuation changes), the firmware calls `adc_cali_create_scheme_curve_fitting()` to precompute a **4096-entry `raw -> mV` lookup table** in SRAM. Every DMA sample is converted to calibrated millivolts in a single array lookup.
3. **Dual-Core Split & On-Chip Pre-Trigger Ring Buffer (ESP32-S3):**
   - **Core 1** continuously drains GDMA blocks into an 8192-sample ring buffer per channel, runs the rising/falling edge trigger detector, and extracts exact pre-trigger + post-trigger windows (`512`–`4096` samples/channel).
   - **Core 0** handles USB CDC binary protocol (`0xAA 0x55`) commands and non-blocking frame transmission.
4. **Sample Rate Modes (Spec vs. Turbo Overclock):**
   - **10 / 20 / 50 / 83.3 kSPS (Spec Max):** Within Espressif's official `SOC_ADC_SAMPLE_FREQ_THRES_HIGH` (`83,333 Hz` aggregate, full 12-bit calibrated accuracy on both XIAO ESP32-S3 and ESP32-C6).
   - **200 kSPS (S3 Turbo):** Initializes GDMA via `adc_continuous`, then adjusts `APB_SARADC.ctrl2.timer_target = 25` (within the 5 MHz / 25-cycle SAR clock envelope).
   - **500 kSPS / 1 MSPS (S3 Overclock):** Tunes `APB_SARADC.ctrl.sar_clk_div` and `timer_target` for fast digital/PWM/edge inspection (reduced ENOB/settling time compared to ≤83.3 kSPS).
5. **Built-in LEDC Test Signal Generator (`D6`):**
   - Outputs a hardware 3.3 V 50% duty square wave at `1 / 5 / 10 / 50 kHz` on `D6` so you can verify probes and triggering with a single jumper wire to `D0`.

## Pinout & Input Safety (0 – 3.3 V)

| Channel | XIAO Pin | XIAO ESP32-S3 | XIAO ESP32-C6 |
| :--- | :--- | :--- | :--- |
| **CH1** | `D0 / A0` | `GPIO1` (`ADC1_CH0`) | `GPIO0` (`ADC1_CH0`) |
| **CH2** | `D1 / A1` | `GPIO2` (`ADC1_CH1`) | `GPIO1` (`ADC1_CH1`) |
| **CH3** | `D2 / A2` | `GPIO3` (`ADC1_CH2`) | `GPIO2` (`ADC1_CH2`) |
| **CH4** | `D3 / A3` | `GPIO4` (`ADC1_CH3`) | *N/A (3-ch max)* |
| **Test Gen** | `D6` | `GPIO43` (LEDC PWM) | `GPIO16` (LEDC PWM) |

> **Warning:** Never apply voltages above **3.3 V** or negative voltages directly to XIAO ADC pins. Use a 10:1 resistive divider or AC-coupling bias network for higher or bipolar signals.

## Development & Build

```bash
rustup target add wasm32-unknown-unknown
cargo clippy --all-targets -- -D warnings
cargo test
trunk serve
trunk build --release
```

## License

Apache-2.0 — see [`LICENSE`](./LICENSE).
