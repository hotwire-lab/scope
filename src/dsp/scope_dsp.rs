use std::f32::consts::PI;

use crate::protocol::types::{
    ChannelCoupling, ChannelViewConfig, MathMode, ScopeFrame, V_PER_DIV_STEPS_MV,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelMeasurements {
    pub active: bool,
    pub vmin_mv: f32,
    pub vmax_mv: f32,
    pub vpp_mv: f32,
    pub vavg_mv: f32,
    pub vrms_mv: f32,
    pub freq_hz: Option<f32>,
    pub period_us: Option<f32>,
    pub duty_pct: Option<f32>,
}

/// Compute standard benchtop oscilloscope measurements for a single channel's mV samples.
pub fn measure_channel(samples_mv: &[f32], sample_rate_hz: u32) -> ChannelMeasurements {
    if samples_mv.is_empty() || sample_rate_hz == 0 {
        return ChannelMeasurements::default();
    }

    let mut vmin = f32::INFINITY;
    let mut vmax = f32::NEG_INFINITY;
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;

    for &v in samples_mv {
        if v < vmin {
            vmin = v;
        }
        if v > vmax {
            vmax = v;
        }
        let vd = v as f64;
        sum += vd;
        sum_sq += vd * vd;
    }

    let n = samples_mv.len() as f64;
    let vavg = (sum / n) as f32;
    let vrms = (sum_sq / n).sqrt() as f32;
    let vpp = (vmax - vmin).max(0.0);

    // Estimate frequency & duty cycle via Schmitt-trigger mid-level crossings with hysteresis
    // in a single O(n) streaming pass with zero heap allocations.
    let (freq_hz, period_us, duty_pct) = if vpp >= 25.0 && samples_mv.len() >= 8 {
        let mid = 0.5 * (vmin + vmax);
        let hyst = (vpp * 0.08).max(8.0);
        let hi_th = mid + hyst;
        let lo_th = mid - hyst;

        let mut high = samples_mv[0] >= mid;
        let mut first_rise: Option<f32> = None;
        let mut last_rise: f32 = 0.0;
        let mut rise_count: usize = 0;
        let mut open_rise: Option<f32> = None;
        let mut high_sum = 0.0f32;
        let mut high_cnt = 0usize;

        for i in 1..samples_mv.len() {
            let prev = samples_mv[i - 1];
            let curr = samples_mv[i];
            if !high && curr >= hi_th {
                high = true;
                let denom = (curr - prev).abs().max(1e-6);
                let frac = ((mid - prev) / denom).clamp(0.0, 1.0);
                let pos = (i - 1) as f32 + frac;
                if first_rise.is_none() {
                    first_rise = Some(pos);
                }
                last_rise = pos;
                rise_count += 1;
                open_rise = Some(pos);
            } else if high && curr <= lo_th {
                high = false;
                let denom = (prev - curr).abs().max(1e-6);
                let frac = ((prev - mid) / denom).clamp(0.0, 1.0);
                let pos = (i - 1) as f32 + frac;
                if let Some(r) = open_rise.take() {
                    let dt = pos - r;
                    if dt > 0.0 {
                        high_sum += dt;
                        high_cnt += 1;
                    }
                }
            }
        }

        let sr = sample_rate_hz as f32;
        if let Some(first) = first_rise.filter(|_| rise_count >= 2) {
            let cycles = (rise_count - 1) as f32;
            let avg_period_samples = (last_rise - first) / cycles;
            if avg_period_samples > 1.0 {
                let freq = sr / avg_period_samples;
                let period = 1_000_000.0 / freq;
                let duty = if high_cnt > 0 {
                    Some(((high_sum / high_cnt as f32) / avg_period_samples * 100.0).clamp(0.0, 100.0))
                } else {
                    None
                };
                (Some(freq), Some(period), duty)
            } else {
                (None, None, None)
            }
        } else {
            (None, None, None)
        }
    } else {
        (None, None, None)
    };

    ChannelMeasurements {
        active: true,
        vmin_mv: vmin,
        vmax_mv: vmax,
        vpp_mv: vpp,
        vavg_mv: vavg,
        vrms_mv: vrms,
        freq_hz,
        period_us,
        duty_pct,
    }
}

/// Compute single-sided magnitude spectrum in dBV (0 dBV = 1 Vrms) using a Hann window,
/// reusing caller-provided scratch buffers to avoid per-frame allocations.
/// Returns `bin_freq_step_hz` and writes `n_fft / 2` magnitudes into `out_dbv`.
pub fn compute_fft_dbv_into(
    samples_mv: &[f32],
    sample_rate_hz: u32,
    re: &mut Vec<f32>,
    im: &mut Vec<f32>,
    out_dbv: &mut Vec<f32>,
) -> f32 {
    out_dbv.clear();
    if samples_mv.len() < 32 || sample_rate_hz == 0 {
        return 0.0;
    }

    // Largest power of two <= samples_mv.len(), capped at 4096
    let mut n = 1usize;
    while n * 2 <= samples_mv.len() && n * 2 <= 4096 {
        n *= 2;
    }

    // Remove DC mean and apply Hann window (convert mV to V)
    let mean_mv: f32 = samples_mv[..n].iter().sum::<f32>() / (n as f32);
    re.clear();
    re.resize(n, 0.0);
    im.clear();
    im.resize(n, 0.0);
    let denom = (n - 1).max(1) as f32;
    for i in 0..n {
        let w = 0.5 * (1.0 - (2.0 * PI * (i as f32) / denom).cos());
        re[i] = ((samples_mv[i] - mean_mv) * 0.001) * w;
    }

    fft_in_place(re, im);

    let half = n / 2;
    out_dbv.reserve(half);
    // Coherent gain of Hann window is 0.5; single-sided RMS scaling = 2 / (n * 0.5 * sqrt(2))
    let scale = 2.0 / ((n as f32) * 0.5 * std::f32::consts::SQRT_2);
    for i in 0..half {
        let mag = (re[i] * re[i] + im[i] * im[i]).sqrt() * scale;
        let dbv = 20.0 * mag.max(1e-6).log10();
        out_dbv.push(dbv.clamp(-100.0, 20.0));
    }

    (sample_rate_hz as f32) / (n as f32)
}

/// Compute single-sided magnitude spectrum in dBV (0 dBV = 1 Vrms) using a Hann window.
/// Returns `(bin_freq_step_hz, magnitudes_dbv)` of length `n_fft / 2`.
#[allow(dead_code)]
pub fn compute_fft_dbv(samples_mv: &[f32], sample_rate_hz: u32) -> (f32, Vec<f32>) {
    let mut re = Vec::new();
    let mut im = Vec::new();
    let mut mags_dbv = Vec::new();
    let bin_hz = compute_fft_dbv_into(samples_mv, sample_rate_hz, &mut re, &mut im, &mut mags_dbv);
    (bin_hz, mags_dbv)
}

fn fft_in_place(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    if n <= 1 {
        return;
    }

    // Bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // Cooley-Tukey radix-2 DIT
    let mut len = 2usize;
    while len <= n {
        let half = len >> 1;
        let angle = -2.0 * PI / (len as f32);
        let wlen_re = angle.cos();
        let wlen_im = angle.sin();

        let mut i = 0usize;
        while i < n {
            let mut w_re = 1.0f32;
            let mut w_im = 0.0f32;
            for k in 0..half {
                let u_re = re[i + k];
                let u_im = im[i + k];
                let v_re = re[i + k + half] * w_re - im[i + k + half] * w_im;
                let v_im = re[i + k + half] * w_im + im[i + k + half] * w_re;

                re[i + k] = u_re + v_re;
                im[i + k] = u_im + v_im;
                re[i + k + half] = u_re - v_re;
                im[i + k + half] = u_im - v_im;

                let next_w_re = w_re * wlen_re - w_im * wlen_im;
                let next_w_im = w_re * wlen_im + w_im * wlen_re;
                w_re = next_w_re;
                w_im = next_w_im;
            }
            i += len;
        }
        len <<= 1;
    }
}

pub fn format_freq(hz: f32) -> String {
    if hz >= 1_000_000.0 {
        format!("{:.3} MHz", hz / 1_000_000.0)
    } else if hz >= 1_000.0 {
        format!("{:.2} kHz", hz / 1_000.0)
    } else {
        format!("{:.1} Hz", hz)
    }
}

pub fn format_time_us(us: f32) -> String {
    let abs = us.abs();
    if abs >= 1_000_000.0 {
        format!("{:.3} s", us / 1_000_000.0)
    } else if abs >= 1_000.0 {
        format!("{:.2} ms", us / 1_000.0)
    } else if abs >= 1.0 {
        format!("{:.1} µs", us)
    } else {
        format!("{:.0} ns", us * 1_000.0)
    }
}

pub fn format_voltage_mv(mv: f32) -> String {
    if mv.abs() >= 1000.0 {
        format!("{:.3} V", mv / 1000.0)
    } else {
        format!("{:.1} mV", mv)
    }
}

/// Compute mean value in mV (used for AC coupling DC-blocking).
pub fn channel_mean_mv(samples_mv: &[f32]) -> f32 {
    if samples_mv.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples_mv.iter().map(|&v| v as f64).sum();
    (sum / samples_mv.len() as f64) as f32
}

/// Convert raw ADC millivolts to vertical divisions from screen center (`-4.0..+4.0` on 8-div screen).
pub fn sample_mv_to_divs(raw_mv: f32, dc_mean_mv: f32, cfg: ChannelViewConfig) -> f32 {
    let base_mv = match cfg.coupling {
        ChannelCoupling::Dc => raw_mv,
        ChannelCoupling::Ac => raw_mv - dc_mean_mv,
    };
    let signed_mv = if cfg.invert { -base_mv } else { base_mv };
    let disp_mv = signed_mv * cfg.probe_mult();
    cfg.offset_divs + disp_mv / cfg.v_per_div_mv().max(1.0)
}

/// Convert vertical divisions from center (`+4.0` top, `-4.0` bottom) to canvas Y pixel.
pub fn divs_to_y(divs_from_center: f64, pad_t: f64, scope_h: f64) -> f64 {
    pad_t + (4.0 - divs_from_center) * (scope_h / 8.0)
}

/// Convert canvas Y pixel back to divisions from screen center (`+4.0` top, `-4.0` bottom).
pub fn y_to_divs(y: f64, pad_t: f64, scope_h: f64) -> f64 {
    4.0 - ((y - pad_t) / scope_h.max(1.0)) * 8.0
}

/// Convert hardware trigger level (in raw ADC mV) to canvas Y using the trigger channel's scale & offset.
pub fn trig_mv_to_y(
    trig_mv: u16,
    dc_mean_mv: f32,
    cfg: ChannelViewConfig,
    pad_t: f64,
    scope_h: f64,
) -> f64 {
    let divs = sample_mv_to_divs(trig_mv as f32, dc_mean_mv, cfg) as f64;
    divs_to_y(divs, pad_t, scope_h)
}

/// Convert a dragged canvas Y position back into a raw ADC trigger level in mV (clamped to `[25, max_mv]`).
pub fn y_to_trig_mv(
    y: f64,
    dc_mean_mv: f32,
    cfg: ChannelViewConfig,
    pad_t: f64,
    scope_h: f64,
    max_mv: u16,
) -> u16 {
    let divs = y_to_divs(y, pad_t, scope_h) as f32;
    let disp_mv = (divs - cfg.offset_divs) * cfg.v_per_div_mv();
    let signed_mv = disp_mv / cfg.probe_mult();
    let base_mv = if cfg.invert { -signed_mv } else { signed_mv };
    let raw_mv = match cfg.coupling {
        ChannelCoupling::Dc => base_mv,
        ChannelCoupling::Ac => base_mv + dc_mean_mv,
    };
    let clamped = raw_mv.clamp(25.0, max_mv.max(50) as f32);
    // Snap to 25 mV steps for clean hardware trigger updates
    (((clamped / 25.0).round() as u16) * 25).clamp(25, max_mv.max(50))
}

/// Arrange active channels cleanly into separate non-overlapping vertical lanes.
pub fn compute_stack_configs(
    channel_mask: u8,
    mut cfgs: [ChannelViewConfig; 4],
    frame: Option<&ScopeFrame>,
) -> [ChannelViewConfig; 4] {
    let active: Vec<usize> = (0..4)
        .filter(|&ch| (channel_mask & (1 << ch)) != 0)
        .collect();
    let count = active.len().max(1);
    let lane_divs = 8.0 / (count as f32);

    for (slot_idx, &ch) in active.iter().enumerate() {
        let lane_center = 4.0 - lane_divs * (slot_idx as f32 + 0.5);
        let cfg = &mut cfgs[ch];
        // Center the channel's signal midpoint in its assigned lane
        let mid_raw_mv = frame
            .and_then(|f| {
                let s = &f.channels_mv[ch];
                if s.is_empty() {
                    None
                } else {
                    let mut lo = f32::INFINITY;
                    let mut hi = f32::NEG_INFINITY;
                    for &v in s {
                        if v < lo {
                            lo = v;
                        }
                        if v > hi {
                            hi = v;
                        }
                    }
                    Some(0.5 * (lo + hi))
                }
            })
            .unwrap_or(1550.0);

        let dc_offset_mv = match cfg.coupling {
            ChannelCoupling::Dc => {
                let s = if cfg.invert { -mid_raw_mv } else { mid_raw_mv };
                s * cfg.probe_mult()
            }
            ChannelCoupling::Ac => 0.0,
        };
        let off = lane_center - dc_offset_mv / cfg.v_per_div_mv().max(1.0);
        cfg.offset_divs = (off * 4.0).round() / 4.0;
        cfg.offset_divs = cfg.offset_divs.clamp(-3.75, 3.75);
    }
    cfgs
}

/// Benchtop Auto-Set: chooses optimal 1-2-5 V/div for each active channel, stacks them cleanly
/// into non-overlapping lanes, and centers the trigger level on the trigger channel.
pub fn compute_autoset(
    frame: &ScopeFrame,
    channel_mask: u8,
    mut cfgs: [ChannelViewConfig; 4],
    trig_ch: u8,
) -> ([ChannelViewConfig; 4], u16) {
    let active: Vec<usize> = (0..4)
        .filter(|&ch| (channel_mask & (1 << ch)) != 0 && !frame.channels_mv[ch].is_empty())
        .collect();
    let count = active.len().max(1);
    let target_span_divs = (6.4 / (count as f32)).clamp(1.5, 4.8);

    for &ch in &active {
        let samples = &frame.channels_mv[ch];
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &v in samples {
            if v < lo {
                lo = v;
            }
            if v > hi {
                hi = v;
            }
        }
        let vpp_raw = (hi - lo).max(80.0);
        let needed_base_vdiv = vpp_raw / target_span_divs;
        let chosen_idx = V_PER_DIV_STEPS_MV
            .iter()
            .position(|&step| step >= needed_base_vdiv)
            .unwrap_or(V_PER_DIV_STEPS_MV.len() - 1);
        cfgs[ch].vdiv_idx = chosen_idx;
    }

    cfgs = compute_stack_configs(channel_mask, cfgs, Some(frame));

    // Center trigger level on trigger channel (or first active channel)
    let trig_idx = (trig_ch as usize).min(3);
    let tch = if !frame.channels_mv[trig_idx].is_empty() {
        trig_idx
    } else {
        active.first().copied().unwrap_or(0)
    };
    let trig_samples = &frame.channels_mv[tch];
    let trig_mv = if !trig_samples.is_empty() {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &v in trig_samples {
            if v < lo {
                lo = v;
            }
            if v > hi {
                hi = v;
            }
        }
        let mid = 0.5 * (lo + hi);
        (((mid / 25.0).round() as u16) * 25).clamp(25, frame.vref_mv.max(50))
    } else {
        frame.vref_mv / 2
    };

    (cfgs, trig_mv)
}

/// Compute Math channel waveform (`CH1 - CH2` or `CH1 + CH2`) in raw mV into `out`.
pub fn compute_math_samples(frame: &ScopeFrame, mode: MathMode, out: &mut Vec<f32>) {
    out.clear();
    if mode == MathMode::Off {
        return;
    }
    let ch1 = &frame.channels_mv[0];
    let ch2 = &frame.channels_mv[1];
    let len = ch1.len().min(ch2.len());
    if len == 0 {
        return;
    }
    out.reserve(len);
    match mode {
        MathMode::Ch1MinusCh2 => {
            for i in 0..len {
                out.push(ch1[i] - ch2[i]);
            }
        }
        MathMode::Ch1PlusCh2 => {
            for i in 0..len {
                out.push(ch1[i] + ch2[i]);
            }
        }
        MathMode::Off => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_1khz_sine_accurately() {
        let sr = 100_000u32;
        let n = 1024usize;
        let samples: Vec<f32> = (0..n)
            .map(|i| {
                let t = (i as f32) / (sr as f32);
                1650.0 + 1000.0 * (2.0 * PI * 1000.0 * t).sin()
            })
            .collect();
        let m = measure_channel(&samples, sr);
        assert!(m.active);
        assert!((m.vpp_mv - 2000.0).abs() < 15.0);
        let f = m.freq_hz.expect("detected frequency");
        assert!((f - 1000.0).abs() < 20.0, "freq={f}");
    }

    #[test]
    fn fft_finds_fundamental_peak() {
        let sr = 64_000u32;
        let n = 512usize;
        let samples: Vec<f32> = (0..n)
            .map(|i| {
                let t = (i as f32) / (sr as f32);
                1500.0 + 1000.0 * (2.0 * PI * 4000.0 * t).sin()
            })
            .collect();
        let (bin_hz, mags) = compute_fft_dbv(&samples, sr);
        assert!((bin_hz - 125.0).abs() < 0.1);
        let (peak_bin, _) = mags
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        // 4000 Hz / 125 Hz = bin 32
        assert_eq!(peak_bin, 32);
    }

    #[test]
    fn vertical_trigger_coordinate_roundtrip() {
        let cfg = ChannelViewConfig {
            vdiv_idx: 4, // 500 mV/div
            offset_divs: -1.5,
            coupling: ChannelCoupling::Dc,
            invert: false,
            probe_x10: false,
        };
        let y = trig_mv_to_y(1650, 1500.0, cfg, 20.0, 400.0);
        let mv_back = y_to_trig_mv(y, 1500.0, cfg, 20.0, 400.0, 3100);
        assert_eq!(mv_back, 1650);

        // Also test inverted + AC coupling + 10x probe roundtrip
        let cfg_ac = ChannelViewConfig {
            vdiv_idx: 3, // 200 mV/div * 10x = 2V/div
            offset_divs: 1.0,
            coupling: ChannelCoupling::Ac,
            invert: true,
            probe_x10: true,
        };
        let y2 = trig_mv_to_y(1750, 1500.0, cfg_ac, 20.0, 400.0);
        let mv_back2 = y_to_trig_mv(y2, 1500.0, cfg_ac, 20.0, 400.0, 3100);
        assert_eq!(mv_back2, 1750);
    }

    #[test]
    fn autoset_stacks_channels_and_centers_trigger() {
        let frame = ScopeFrame {
            sample_rate_hz: 50_000,
            channel_mask: 0x03,
            atten_code: 3,
            status_flags: 0,
            triggered: true,
            overclocked: false,
            calibrated_mv: true,
            adc_saturated: false,
            trig_channel: 0,
            samples_per_ch: 4,
            trigger_index: 2,
            trigger_level_mv: 1500,
            vref_mv: 3100,
            channels_mv: [
                vec![500.0, 2500.0, 500.0, 2500.0],
                vec![1000.0, 2000.0, 1000.0, 2000.0],
                vec![],
                vec![],
            ],
            timestamp_ms: 0.0,
        };
        let defaults = [
            ChannelViewConfig::default_for_channel(0),
            ChannelViewConfig::default_for_channel(1),
            ChannelViewConfig::default_for_channel(2),
            ChannelViewConfig::default_for_channel(3),
        ];
        let (cfgs, trig_mv) = compute_autoset(&frame, 0x03, defaults, 0);
        assert_eq!(trig_mv, 1500);
        // CH1 should sit in upper half, CH2 in lower half
        assert!(cfgs[0].offset_divs > cfgs[1].offset_divs);
    }
}
