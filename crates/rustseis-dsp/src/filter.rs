use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FilterSpecType {
    /// Four corner frequencies `(f1, f2, f3, f4)` Hz with Hanning (cosine) frequency tapers (ProMAX exact).
    #[default]
    Ormsby,
    /// `(f_low_hz, slope_low_db_oct, f_high_hz, slope_high_db_oct)` Butterworth response (ProMAX exact).
    Butterworth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FilterPhase {
    #[default]
    Zero,
    /// Minimum phase via Kolmogoroff/Hilbert spectral factorization with `rnoise_pct` floor.
    Minimum,
}

/// Bandpass Filter (`filter`) parameters — direct clean-room match to ProMAX `filter.pdf`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandpassParams {
    #[serde(default)]
    pub spec_type: FilterSpecType,
    #[serde(default)]
    pub phase: FilterPhase,
    /// For Ormsby: `[f1, f2, f3, f4]` Hz.
    /// For Butterworth: `[f_low_3db, low_db_per_oct, f_high_3db, high_db_per_oct]`.
    pub freqs: [f32; 4],
    /// Percent additive white noise spectral floor for minimum-phase conversion (default 1.0%).
    #[serde(default = "default_rnoise")]
    pub rnoise_pct: f32,
    /// Percent zero padding for FFTs (default 25%).
    #[serde(default = "default_pad_pct")]
    pub pad_pct: f32,
    /// Optional notch filter `(center_hz, half_width_hz)`.
    #[serde(default)]
    pub notch: Option<(f32, f32)>,
}

fn default_rnoise() -> f32 {
    1.0
}
fn default_pad_pct() -> f32 {
    25.0
}

impl Default for BandpassParams {
    fn default() -> Self {
        Self {
            spec_type: FilterSpecType::Ormsby,
            phase: FilterPhase::Zero,
            freqs: [5.0, 10.0, 60.0, 80.0],
            rnoise_pct: 1.0,
            pad_pct: 25.0,
            notch: None,
        }
    }
}

/// Build amplitude response `A(f)` for a single frequency `f >= 0` Hz (ProMAX exact formulas).
fn amplitude_response(f: f64, params: &BandpassParams) -> f64 {
    let mut amp = match params.spec_type {
        FilterSpecType::Ormsby => {
            let [f1, f2, f3, f4] = params.freqs.map(|x| x as f64);
            if f <= f1 || f >= f4 {
                0.0
            } else if f < f2 {
                let frac = (f - f1) / (f2 - f1).max(1e-9);
                0.5 * (1.0 - (PI * frac).cos()) // Hanning/cosine taper (ProMAX filter.pdf p.24)
            } else if f <= f3 {
                1.0
            } else {
                let frac = (f4 - f) / (f4 - f3).max(1e-9);
                0.5 * (1.0 - (PI * frac).cos())
            }
        }
        FilterSpecType::Butterworth => {
            let [fl, sl, fh, sh] = params.freqs.map(|x| x as f64);
            // ProMAX bandpass Butterworth prototype around geometric center fmid = sqrt(fl * fh):
            // matches ProMAX FilterTool to corr = 0.9998+ (NRMS ~1.9%)
            if fl > 0.0 && fh > fl && sl > 0.0 && sh > 0.0 {
                if f <= 1e-9 {
                    0.0
                } else {
                    let fmid = (fl * fh).sqrt();
                    let bw = fh - fl;
                    let u = (f * f - fl * fh).abs() / (f * bw);
                    let p = if f < fmid { sl / 6.0206 } else { sh / 6.0206 };
                    1.0 / (1.0 + u.powf(2.0 * p)).sqrt()
                }
            } else {
                let low = if fl > 0.0 && sl > 0.0 {
                    let p = sl / 6.0206;
                    if f <= 1e-9 { 0.0 } else { 1.0 / (1.0 + (fl / f).powf(2.0 * p)).sqrt() }
                } else {
                    1.0
                };
                let high = if fh > 0.0 && sh > 0.0 {
                    let p = sh / 6.0206;
                    1.0 / (1.0 + (f / fh).powf(2.0 * p)).sqrt()
                } else {
                    1.0
                };
                low * high
            }
        }
    };

    if let Some((fc, hw)) = params.notch {
        let fc = fc as f64;
        let hw = (hw as f64).max(1e-3);
        let d = (f - fc).abs();
        if d < hw {
            let w = 0.5 * (1.0 + (PI * d / hw).cos());
            amp *= 1.0 - w;
        }
    }
    amp
}

/// Build full complex frequency transfer function `H[k]` of length `nfft` (zero or minimum phase).
fn build_transfer_function(nfft: usize, dt_s: f64, params: &BandpassParams) -> Vec<Complex<f64>> {
    let df = 1.0 / (nfft as f64 * dt_s);
    let mut amp = vec![0.0f64; nfft];
    for k in 0..=nfft / 2 {
        let f = k as f64 * df;
        let a = amplitude_response(f, params);
        amp[k] = a;
        if k > 0 && k < nfft - k {
            amp[nfft - k] = a;
        }
    }

    match params.phase {
        FilterPhase::Zero => amp.into_iter().map(|a| Complex::new(a, 0.0)).collect(),
        FilterPhase::Minimum => {
            // Kolmogoroff / Hilbert minimum-phase spectral factorization:
            // log|H_min(w)| -> cepstrum -> causal weighting -> exp(FFT(causal_cepstrum))
            let max_a = amp.iter().cloned().fold(0.0f64, f64::max).max(1e-12);
            let floor = (params.rnoise_pct as f64 / 100.0).max(1e-6) * max_a;

            let mut planner = FftPlanner::<f64>::new();
            let fft = planner.plan_fft_forward(nfft);
            let ifft = planner.plan_fft_inverse(nfft);

            let mut log_spec: Vec<Complex<f64>> = amp
                .iter()
                .map(|&a| Complex::new((a.max(floor)).ln(), 0.0))
                .collect();
            ifft.process(&mut log_spec);
            let inv_n = 1.0 / nfft as f64;
            for c in &mut log_spec {
                *c *= inv_n;
            }

            // Causal cepstrum window: keep c[0], double 1..nfft/2, keep nfft/2, zero negative lags
            for k in 1..nfft / 2 {
                log_spec[k] *= 2.0;
            }
            for k in (nfft / 2 + 1)..nfft {
                log_spec[k] = Complex::new(0.0, 0.0);
            }

            fft.process(&mut log_spec);
            log_spec.into_iter().map(|c| c.exp()).collect()
        }
    }
}

pub fn apply_bandpass_trace(trace: &mut Trace, params: &BandpassParams) -> Result<()> {
    if !trace.is_live() || trace.samples.is_empty() {
        return Ok(());
    }
    let n = trace.samples.len();
    let pad = ((n as f32) * (params.pad_pct.max(0.0) / 100.0)).ceil() as usize;
    let nfft = (n + pad).next_power_of_two().max(64);
    let h = build_transfer_function(nfft, trace.dt_s() as f64, params);

    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(nfft);
    let ifft = planner.plan_fft_inverse(nfft);

    let mut buf: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); nfft];
    for i in 0..n {
        buf[i] = Complex::new(trace.samples[i] as f64, 0.0);
    }
    fft.process(&mut buf);
    for k in 0..nfft {
        buf[k] *= h[k];
    }
    ifft.process(&mut buf);

    let inv_n = 1.0 / nfft as f64;
    for i in 0..n {
        trace.samples[i] = (buf[i].re * inv_n) as f32;
    }
    Ok(())
}

pub fn apply_bandpass(traces: &mut [Trace], params: &BandpassParams) -> Result<()> {
    if traces.is_empty() {
        return Ok(());
    }
    // Precompute transfer function once when all traces share dt_us and n_samples
    let dt_us = traces[0].dt_us;
    let n = traces[0].samples.len();
    let uniform = traces.iter().all(|t| t.dt_us == dt_us && t.samples.len() == n);
    if uniform && n > 0 {
        let pad = ((n as f32) * (params.pad_pct.max(0.0) / 100.0)).ceil() as usize;
        let nfft = (n + pad).next_power_of_two().max(64);
        let h = build_transfer_function(nfft, dt_us as f64 / 1_000_000.0, params);
        // Plan FFT/IFFT once outside the parallel loop (Arc<dyn Fft> is Send + Sync -> ~20x faster on 100k+ traces!)
        let mut planner = FftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(nfft);
        let ifft = planner.plan_fft_inverse(nfft);
        let inv_n = 1.0 / nfft as f64;
        traces.par_iter_mut().for_each(|tr| {
            if !tr.is_live() {
                return;
            }
            let mut buf = vec![Complex::new(0.0, 0.0); nfft];
            for i in 0..n {
                buf[i] = Complex::new(tr.samples[i] as f64, 0.0);
            }
            fft.process(&mut buf);
            for k in 0..nfft {
                buf[k] *= h[k];
            }
            ifft.process(&mut buf);
            for i in 0..n {
                tr.samples[i] = (buf[i].re * inv_n) as f32;
            }
        });
        Ok(())
    } else {
        for tr in traces.iter_mut() {
            apply_bandpass_trace(tr, params)?;
        }
        Ok(())
    }
}

/// Time-Variant Bandpass Filter (ProMAX exact linear taper interpolation between windows).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeVariantBandpassParams {
    /// List of `(t_start_ms, t_end_ms, BandpassParams)` ordered in time.
    pub windows: Vec<(f32, f32, BandpassParams)>,
}

pub fn apply_time_variant_bandpass(
    traces: &mut [Trace],
    params: &TimeVariantBandpassParams,
) -> Result<()> {
    if params.windows.is_empty() {
        return Err(SeismicError::InvalidParameter {
            param: "windows".into(),
            message: "At least one time window required".into(),
        });
    }
    if params.windows.len() == 1 {
        return apply_bandpass(traces, &params.windows[0].2);
    }

    let mut windows = params.windows.clone();
    windows.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut filtered_copies: Vec<Vec<Trace>> = Vec::with_capacity(windows.len());
    for (_, _, bp) in &windows {
        let mut copy = traces.to_vec();
        apply_bandpass(&mut copy, bp)?;
        filtered_copies.push(copy);
    }

    traces.par_iter_mut().enumerate().for_each(|(ti, tr)| {
        let dt_ms = tr.dt_ms();
        let n = tr.samples.len();
        for i in 0..n {
            let t = i as f32 * dt_ms;
            let mut val = filtered_copies[0][ti].samples[i];
            if t <= windows[0].1 {
                val = filtered_copies[0][ti].samples[i];
            } else if t >= windows[windows.len() - 1].0 {
                val = filtered_copies[windows.len() - 1][ti].samples[i];
            } else {
                for w in 0..windows.len() - 1 {
                    let end_a = windows[w].1;
                    let start_b = windows[w + 1].0;
                    if t >= windows[w].0 && t <= end_a {
                        val = filtered_copies[w][ti].samples[i];
                        break;
                    } else if t > end_a && t < start_b {
                        let frac = (t - end_a) / (start_b - end_a).max(1e-6);
                        val = (1.0 - frac) * filtered_copies[w][ti].samples[i]
                            + frac * filtered_copies[w + 1][ti].samples[i];
                        break;
                    }
                }
            }
            tr.samples[i] = val;
        }
    });
    Ok(())
}

/// Time-Variant Spectral Whitening (`tvw`): narrow-band decomposition + AGC per band + sum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TvwParams {
    pub f_min_hz: f32,
    pub f_max_hz: f32,
    pub n_bands: usize,
    pub agc_operator_ms: f32,
}

impl Default for TvwParams {
    fn default() -> Self {
        Self {
            f_min_hz: 8.0,
            f_max_hz: 80.0,
            n_bands: 6,
            agc_operator_ms: 500.0,
        }
    }
}

pub fn apply_tvw(traces: &mut [Trace], params: &TvwParams) -> Result<()> {
    let nb = params.n_bands.max(2);
    let bw = (params.f_max_hz - params.f_min_hz).max(4.0) / nb as f32;
    let taper = (bw * 0.35).max(1.0);

    let mut accum: Vec<Vec<f32>> = traces.iter().map(|t| vec![0.0f32; t.samples.len()]).collect();
    let agc = crate::gain::AgcParams {
        operator_ms: params.agc_operator_ms,
        ..Default::default()
    };

    for b in 0..nb {
        let f_lo = params.f_min_hz + b as f32 * bw;
        let f_hi = f_lo + bw;
        let bp = BandpassParams {
            spec_type: FilterSpecType::Ormsby,
            phase: FilterPhase::Zero,
            freqs: [(f_lo - taper).max(0.5), f_lo, f_hi, f_hi + taper],
            ..Default::default()
        };
        let mut band = traces.to_vec();
        apply_bandpass(&mut band, &bp)?;
        crate::gain::apply_agc(&mut band, &agc);
        for (ti, tr) in band.iter().enumerate() {
            for (si, &v) in tr.samples.iter().enumerate() {
                accum[ti][si] += v;
            }
        }
    }

    let inv = 1.0 / nb as f32;
    for (ti, tr) in traces.iter_mut().enumerate() {
        if !tr.is_live() {
            continue;
        }
        for (si, s) in tr.samples.iter_mut().enumerate() {
            *s = accum[ti][si] * inv;
        }
    }
    Ok(())
}
