use std::f32::consts::PI;

use crate::protocol::codec::{
    compute_xor_checksum, encode_response_frame, encode_scope_frame_payload, resolve_atten_preset,
    resolve_depth_preset, resolve_rate_preset,
};
use crate::protocol::types::*;

/// Offline 4-channel oscilloscope simulator for zero-hardware testing & PWA demo mode.
/// - CH1 (D0): 1 kHz (or test_gen freq) clean sine wave + subtle 12-bit quantization noise
/// - CH2 (D1): PWM square wave (35% duty) with realistic RC rise-time ringing
/// - CH3 (D2): 2.5 kHz triangle / ramp wave
/// - CH4 (D3): AM-modulated audio/carrier burst
pub struct DemoSimulator {
    rate_code: u8,
    channel_mask: u8,
    atten_code: u8,
    samples_code: u8,
    trig_mode: u8,
    trig_edge: u8,
    trig_ch: u8,
    trig_level_mv: u16,
    pre_trigger_pct: u8,
    run_mode: u8,
    test_gen_code: u8,
    cali_enabled: bool,
    phase: f64,
    rng_state: u32,
}

impl Default for DemoSimulator {
    fn default() -> Self {
        Self::new()
    }
}

impl DemoSimulator {
    pub fn new() -> Self {
        Self {
            rate_code: 3, // 83.3 kSPS
            channel_mask: 0x03, // CH1 + CH2
            atten_code: 3, // 12 dB (0-3.1V)
            samples_code: 1, // 1024
            trig_mode: SCOPE_TRIG_AUTO,
            trig_edge: SCOPE_EDGE_RISING,
            trig_ch: 0,
            trig_level_mv: 1550,
            pre_trigger_pct: 50,
            run_mode: SCOPE_RUN_CONTINUOUS,
            test_gen_code: 1, // 1 kHz
            cali_enabled: true,
            phase: 0.0,
            rng_state: 0x1234_5678,
        }
    }

    fn next_noise(&mut self) -> f32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        ((x as f32) / (u32::MAX as f32) - 0.5) * 14.0
    }

    pub fn handle_command(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 8 <= bytes.len() {
            if bytes[i] == PROTOCOL_SYNC_0 && bytes[i + 1] == PROTOCOL_SYNC_1 {
                let frame = &bytes[i..i + 8];
                if compute_xor_checksum(&frame[..7]) == frame[7] {
                    match frame[2] {
                        CMD_GET_CAPS | CMD_PING_STATUS => {
                            let caps = r#"{"board":"XIAO-ESP32-S3","firmware":"1.0.0-scope-demo","scope":true,"max_channels":4,"max_rate_hz":1000000,"adc_bits":12}"#;
                            out.extend_from_slice(&encode_response_frame(
                                TYPE_BOARD_CAPABILITIES,
                                caps.as_bytes(),
                            ));
                        }
                        CMD_SCOPE_CFG => {
                            self.rate_code = frame[3].min(6);
                            self.channel_mask = (frame[4] & 0x0f).max(1);
                            self.atten_code = frame[5].min(3);
                            self.samples_code = frame[6].min(3);
                        }
                        CMD_SCOPE_TRIG => {
                            let p1 = frame[3];
                            self.trig_mode = p1 & 0x03;
                            self.trig_edge = (p1 >> 4) & 0x01;
                            self.trig_ch = (p1 >> 6) & 0x03;
                            self.trig_level_mv = u16::from_le_bytes([frame[4], frame[5]]);
                            self.pre_trigger_pct = frame[6].min(75);
                        }
                        CMD_SCOPE_RUN => {
                            self.run_mode = frame[3].min(2);
                            self.test_gen_code = frame[5].min(4);
                            self.cali_enabled = (frame[6] & 0x01) != 0;
                            if self.run_mode == SCOPE_RUN_SINGLE {
                                out.extend_from_slice(&self.generate_frame());
                                self.run_mode = SCOPE_RUN_STOP;
                            }
                        }
                        _ => {}
                    }
                    i += 8;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    pub fn tick_stream(&mut self) -> Option<Vec<u8>> {
        if self.run_mode == SCOPE_RUN_CONTINUOUS {
            Some(self.generate_frame())
        } else {
            None
        }
    }

    fn generate_frame(&mut self) -> Vec<u8> {
        let rate_preset = resolve_rate_preset(self.rate_code);
        let atten_preset = resolve_atten_preset(self.atten_code);
        let depth_preset = resolve_depth_preset(self.samples_code);

        let active_cnt = self.channel_mask.count_ones().max(1);
        let per_ch_rate = (rate_preset.total_rate_hz / active_cnt).max(1000);
        let n = depth_preset.samples;
        let vref = atten_preset.full_scale_mv;
        let trig_idx = ((n as u32 * self.pre_trigger_pct as u32) / 100) as usize;

        // Advance phase slightly so Free Run scrolls, while triggered modes stay phase-locked
        self.phase += 0.17;
        let base_freq = match self.test_gen_code {
            2 => 5_000.0f64,
            3 => 10_000.0f64,
            4 => 50_000.0f64,
            _ => 1_000.0f64,
        };

        let locked = self.trig_mode != SCOPE_TRIG_FREE_RUN;
        let phase_offset = if locked {
            // Phase-lock CH1 to trigger level crossing
            let mid = vref as f64 * 0.5;
            let amp = vref as f64 * 0.36;
            let norm = ((self.trig_level_mv as f64 - mid) / amp).clamp(-0.98, 0.98);
            let mut phi = norm.asin();
            if self.trig_edge == SCOPE_EDGE_FALLING {
                phi = std::f64::consts::PI - phi;
            }
            phi
        } else {
            self.phase
        };

        let mut planar = Vec::with_capacity(active_cnt as usize * n);
        let mut saturated = false;

        for ch in 0..4u8 {
            if (self.channel_mask & (1 << ch)) == 0 {
                continue;
            }
            for i in 0..n {
                let dt = ((i as f64) - (trig_idx as f64)) / (per_ch_rate as f64);
                let noise = self.next_noise();
                let mv = match ch {
                    0 => {
                        // CH1: Clean Sine Wave
                        let mid = vref as f32 * 0.5;
                        let amp = vref as f32 * 0.36;
                        mid + amp * (2.0 * std::f64::consts::PI * base_freq * dt + phase_offset).sin() as f32
                            + noise
                    }
                    1 => {
                        // CH2: PWM Square Wave (35% duty) with slight overshoot
                        let cyc = (base_freq * dt + if locked { 0.0 } else { self.phase * 0.15 }).rem_euclid(1.0);
                        let low = vref as f32 * 0.12;
                        let high = vref as f32 * 0.84;
                        let edge_ring = if cyc < 0.04 {
                            (cyc as f32 / 0.04 * PI * 3.0).sin() * 70.0 * (-cyc as f32 * 40.0).exp()
                        } else {
                            0.0
                        };
                        (if cyc < 0.35 { high + edge_ring } else { low }) + noise * 0.7
                    }
                    2 => {
                        // CH3: Triangle wave (2.5x frequency)
                        let cyc = (base_freq * 2.5 * dt + if locked { 0.2 } else { self.phase * 0.2 }).rem_euclid(1.0) as f32;
                        let tri = if cyc < 0.5 { cyc * 2.0 } else { (1.0 - cyc) * 2.0 };
                        vref as f32 * (0.15 + 0.65 * tri) + noise
                    }
                    _ => {
                        // CH4: AM Modulated burst
                        let env = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * (base_freq * 0.25) * dt).sin() as f32;
                        let carrier = (2.0 * std::f64::consts::PI * (base_freq * 4.0) * dt + phase_offset).sin() as f32;
                        vref as f32 * 0.5 + vref as f32 * 0.35 * env * carrier + noise
                    }
                };

                if mv >= vref as f32 - 5.0 || mv <= 2.0 {
                    saturated = true;
                }
                let clamped = mv.clamp(0.0, vref as f32);
                let out_u16 = if self.cali_enabled {
                    clamped.round() as u16
                } else {
                    ((clamped / vref as f32) * 4095.0).round() as u16
                };
                planar.push(out_u16);
            }
        }

        let mut flags = SCOPE_FLAG_PACKED_12BIT;
        if locked {
            flags |= SCOPE_FLAG_TRIGGERED;
        }
        if rate_preset.overclocked {
            flags |= SCOPE_FLAG_OVERCLOCKED;
        }
        if self.cali_enabled {
            flags |= SCOPE_FLAG_CALIBRATED_MV;
        }
        if saturated {
            flags |= SCOPE_FLAG_ADC_SATURATED;
        }

        let payload = encode_scope_frame_payload(
            per_ch_rate,
            self.channel_mask,
            self.atten_code,
            flags,
            self.trig_ch,
            n as u16,
            trig_idx as u16,
            self.trig_level_mv,
            vref,
            &planar,
        );
        encode_response_frame(TYPE_SCOPE_FRAME, &payload)
    }
}

impl hotwire_core::transport::DemoBackend for DemoSimulator {
    fn device_name(&self) -> &str {
        "XIAO-ESP32-S3 (Demo)"
    }

    fn vid_pid(&self) -> &str {
        "DEMO:0001"
    }

    fn tick_interval_ms(&self) -> Option<i32> {
        Some(50)
    }

    fn handle_command(&mut self, bytes: &[u8]) -> Vec<u8> {
        DemoSimulator::handle_command(self, bytes)
    }

    fn tick(&mut self) -> Option<Vec<u8>> {
        self.tick_stream()
    }
}
