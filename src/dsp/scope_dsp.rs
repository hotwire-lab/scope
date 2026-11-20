use std::f32::consts::PI;

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
    let (freq_hz, period_us, duty_pct) = if vpp >= 25.0 && samples_mv.len() >= 8 {
        let mid = 0.5 * (vmin + vmax);
        let hyst = (vpp * 0.08).max(8.0);
        let hi_th = mid + hyst;
        let lo_th = mid - hyst;

        let mut high = samples_mv[0] >= mid;
        let mut rising_edges: Vec<f32> = Vec::new();
        let mut falling_edges: Vec<f32> = Vec::new();

        for i in 1..samples_mv.len() {
            let prev = samples_mv[i - 1];
            let curr = samples_mv[i];
            if !high && curr >= hi_th {
                high = true;
                let denom = (curr - prev).abs().max(1e-6);
                let frac = ((mid - prev) / denom).clamp(0.0, 1.0);
                rising_edges.push((i - 1) as f32 + frac);
            } else if high && curr <= lo_th {
                high = false;
                let denom = (prev - curr).abs().max(1e-6);
                let frac = ((prev - mid) / denom).clamp(0.0, 1.0);
                falling_edges.push((i - 1) as f32 + frac);
            }
        }

        let sr = sample_rate_hz as f32;
        if rising_edges.len() >= 2 {
            let first = rising_edges[0];
            let last = *rising_edges.last().unwrap();
            let cycles = (rising_edges.len() - 1) as f32;
            let avg_period_samples = (last - first) / cycles;
            if avg_period_samples > 1.0 {
                let freq = sr / avg_period_samples;
                let period = 1_000_000.0 / freq;

                // Duty cycle from high-time between matched rising and next falling edge
                let mut high_sum = 0.0f32;
                let mut high_cnt = 0usize;
                for &r in &rising_edges {
                    if let Some(&f) = falling_edges.iter().find(|&&fe| fe > r) {
                        let dt = f - r;
                        if dt > 0.0 && dt < avg_period_samples {
                            high_sum += dt;
                            high_cnt += 1;
                        }
                    }
                }
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

/// Compute single-sided magnitude spectrum in dBV (0 dBV = 1 Vrms) using a Hann window.
/// Returns `(bin_freq_step_hz, magnitudes_dbv)` of length `n_fft / 2`.
pub fn compute_fft_dbv(samples_mv: &[f32], sample_rate_hz: u32) -> (f32, Vec<f32>) {
    if samples_mv.len() < 32 || sample_rate_hz == 0 {
        return (0.0, Vec::new());
    }

    // Largest power of two <= samples_mv.len(), capped at 4096
    let mut n = 1usize;
    while n * 2 <= samples_mv.len() && n * 2 <= 4096 {
        n *= 2;
    }

    // Remove DC mean and apply Hann window (convert mV to V)
    let mean_mv: f32 = samples_mv[..n].iter().sum::<f32>() / (n as f32);
    let mut re = vec![0.0f32; n];
    let mut im = vec![0.0f32; n];
    let denom = (n - 1).max(1) as f32;
    for i in 0..n {
        let w = 0.5 * (1.0 - (2.0 * PI * (i as f32) / denom).cos());
        re[i] = ((samples_mv[i] - mean_mv) * 0.001) * w;
    }

    fft_in_place(&mut re, &mut im);

    let half = n / 2;
    let mut mags_dbv = Vec::with_capacity(half);
    // Coherent gain of Hann window is 0.5; single-sided RMS scaling = 2 / (n * 0.5 * sqrt(2))
    let scale = 2.0 / ((n as f32) * 0.5 * std::f32::consts::SQRT_2);
    for i in 0..half {
        let mag = (re[i] * re[i] + im[i] * im[i]).sqrt() * scale;
        let dbv = 20.0 * mag.max(1e-6).log10();
        mags_dbv.push(dbv.clamp(-100.0, 20.0));
    }

    let bin_hz = (sample_rate_hz as f32) / (n as f32);
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
}
