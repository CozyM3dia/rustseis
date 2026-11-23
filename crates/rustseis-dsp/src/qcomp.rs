//! Stabilized Inverse-Q Filtering (Wang 2002/2006) & Constant-Phase Rotation / Time Shift.
//!
//! - `qcomp`: Gabor STFT stabilized inverse-Q compensation for seismic anelastic attenuation
//!   (amplitude loss + Kjartansson causal phase dispersion), supporting constant `Q` or
//!   time-varying `Q(t)` profiles with strict maximum gain stabilization (`max_gain_db`).
//! - `phase_rot`: Exact FFT Hilbert constant-phase rotation, fractional-sample time shift,
//!   and polarity inversion for seismic-to-well tie and QC conditioning.

use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::Trace;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum QCompMode {
    /// Compensate both amplitude attenuation (stabilized) and phase dispersion.
    #[default]
    Both,
    /// Compensate phase dispersion only (unconditionally stable, zero noise amplification).
    PhaseOnly,
    /// Compensate amplitude attenuation only (stabilized).
    AmplitudeOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QCompParams {
    /// Quality factor `Q` (typical sedimentary range `30..250`; higher = less attenuation).
    #[serde(default = "default_q")]
    pub q_value: f32,
    /// Optional time-varying `[(time_ms, Q), ...]` profile; overrides `q_value` when non-empty.
    #[serde(default)]
    pub q_profile: Vec<(f32, f32)>,
    /// Reference frequency `f_ref` (Hz) for Kjartansson phase dispersion (default `50.0` Hz).
    #[serde(default = "default_fref")]
    pub reference_freq_hz: f32,
    /// Maximum allowed amplitude boost in dB (stabilization cap, default `24.0` dB).
    #[serde(default = "default_max_gain_db")]
    pub max_gain_db: f32,
    /// Compensation mode (`Both`, `PhaseOnly`, or `AmplitudeOnly`).
    #[serde(default)]
    pub mode: QCompMode,
    /// Gabor STFT window length in ms (default `200.0` ms, 75% overlap).
    #[serde(default = "default_win_ms")]
    pub window_ms: f32,
}

fn default_q() -> f32 { 80.0 }
fn default_fref() -> f32 { 50.0 }
fn default_max_gain_db() -> f32 { 24.0 }
fn default_win_ms() -> f32 { 200.0 }

impl Default for QCompParams {
    fn default() -> Self {
        Self {
            q_value: default_q(),
            q_profile: Vec::new(),
            reference_freq_hz: default_fref(),
            max_gain_db: default_max_gain_db(),
            mode: QCompMode::Both,
            window_ms: default_win_ms(),
        }
    }
}

/// Evaluate effective path-averaged `Q_eff(t)` from constant `q_value` or piecewise `q_profile`.
fn q_at_ms(params: &QCompParams, t_ms: f32) -> f64 {
    if params.q_profile.is_empty() {
        return (params.q_value as f64).max(5.0);
    }
    let prof = &params.q_profile;
    if t_ms <= prof[0].0 {
        return (prof[0].1 as f64).max(5.0);
    }
    for w in prof.windows(2) {
        if t_ms <= w[1].0 {
            let frac = ((t_ms - w[0].0) / (w[1].0 - w[0].0).max(1e-6)) as f64;
            let q = w[0].1 as f64 + frac * (w[1].1 as f64 - w[0].1 as f64);
            return q.max(5.0);
        }
    }
    (prof.last().unwrap().1 as f64).max(5.0)
}

/// Core Gabor STFT Q-operator (`inverse = true` for inverse-Q compensation, `false` for forward Q).
fn apply_q_stft_trace(trace: &mut Trace, params: &QCompParams, inverse: bool) {
    if !trace.is_live() || trace.samples.len() < 8 {
        return;
    }
    let n = trace.samples.len();
    let dt_s = trace.dt_s() as f64;
    let win_len = ((params.window_ms as f64 * 1e-3 / dt_s).round() as usize)
        .clamp(16, n)
        .next_power_of_two()
        .min(512);
    let hop = (win_len / 4).max(4);
    let nfft = (win_len * 2).next_power_of_two().max(64);
    let df = 1.0 / (nfft as f64 * dt_s);

    let hann: Vec<f64> = (0..win_len)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f64 / win_len as f64).cos()))
        .collect();

    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(nfft);
    let inv = planner.plan_fft_inverse(nfft);
    let inv_scale = 1.0 / nfft as f64;

    let max_gain = 10.0f64.powf(params.max_gain_db.clamp(0.0, 60.0) as f64 / 20.0);
    // Tikhonov floor tuned so peak `beta / (beta^2 + sigma^2)` equals `max_gain`.
    let sigma = 0.5 / max_gain.max(1.0);
    let sigma2 = sigma * sigma;
    let f_ref = (params.reference_freq_hz as f64).max(1.0);

    let mut out = vec![0.0f64; n];
    let mut w_acc = vec![0.0f64; n];
    let mut buf = vec![Complex::new(0.0, 0.0); nfft];

    let mut start = 0usize;
    loop {
        let center_s = (start as f64 + 0.5 * win_len as f64).min(n as f64) * dt_s;
        let q_eff = q_at_ms(params, (center_s * 1000.0) as f32);
        let gamma = 1.0 / (PI * q_eff);

        buf.fill(Complex::new(0.0, 0.0));
        for i in 0..win_len {
            let idx = start + i;
            if idx < n {
                buf[i] = Complex::new(trace.samples[idx] as f64 * hann[i], 0.0);
            }
        }
        fwd.process(&mut buf);

        for k in 1..nfft {
            let is_neg = k > nfft / 2;
            let f_bin = if is_neg { nfft - k } else { k };
            let f = f_bin as f64 * df;
            if f <= 0.0 {
                continue;
            }
            let beta = (-PI * f * center_s / q_eff).exp();
            let amp = match params.mode {
                QCompMode::PhaseOnly => 1.0,
                QCompMode::Both | QCompMode::AmplitudeOnly => {
                    if inverse {
                        // Stabilized inverse amplitude gain bounded cleanly by `max_gain`
                        let g = (beta / (beta * beta + sigma2)).min(max_gain);
                        // Preserve unity DC / zero-time behavior
                        g * (1.0 + sigma2)
                    } else {
                        beta
                    }
                }
            };
            let phase = match params.mode {
                QCompMode::AmplitudeOnly => 0.0,
                QCompMode::Both | QCompMode::PhaseOnly => {
                    // Causal Kjartansson phase dispersion relative to reference frequency `f_ref`.
                    let disp = 2.0 * PI * f * center_s * ((f / f_ref).powf(-gamma) - 1.0);
                    let signed = if inverse { -disp } else { disp };
                    if is_neg { -signed } else { signed }
                }
            };
            let op = Complex::from_polar(amp, phase);
            buf[k] *= op;
        }

        inv.process(&mut buf);
        for i in 0..win_len {
            let idx = start + i;
            if idx < n {
                // Synthesis Hann window ensures smooth overlap-add without block-edge clicks.
                let w = hann[i];
                out[idx] += buf[i].re * inv_scale * w;
                w_acc[idx] += w * w;
            }
        }

        if start + win_len >= n {
            break;
        }
        start += hop;
    }

    for i in 0..n {
        trace.samples[i] = if w_acc[i] > 1e-9 {
            (out[i] / w_acc[i]) as f32
        } else {
            trace.samples[i]
        };
    }
}

/// Apply stabilized Inverse-Q filtering to a single trace.
pub fn apply_qcomp_trace(trace: &mut Trace, params: &QCompParams) {
    apply_q_stft_trace(trace, params, true);
}

/// Apply forward earth-Q attenuation & dispersion (useful for modeling & verification tests).
pub fn apply_forward_q_trace(trace: &mut Trace, params: &QCompParams) {
    apply_q_stft_trace(trace, params, false);
}

/// Apply stabilized Inverse-Q filtering in parallel across all traces.
pub fn apply_qcomp(traces: &mut [Trace], params: &QCompParams) {
    traces.par_iter_mut().for_each(|tr| apply_qcomp_trace(tr, params));
}

// ---------------------------------------------------------------------------
// Constant-phase rotation, fractional time shift, and polarity flip (`phase_rot`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseRotParams {
    /// Constant phase rotation angle in degrees (`-180..180`).
    #[serde(default)]
    pub phase_deg: f32,
    /// Optional fractional-sample static time shift in milliseconds (positive shifts down/later).
    #[serde(default)]
    pub shift_ms: f32,
    /// Optional polarity inversion (multiply by `-1`).
    #[serde(default)]
    pub invert_polarity: bool,
    /// Optional scalar amplitude gain multiplier (default `1.0`).
    #[serde(default = "default_one")]
    pub gain: f32,
}

fn default_one() -> f32 { 1.0 }

impl Default for PhaseRotParams {
    fn default() -> Self {
        Self {
            phase_deg: 0.0,
            shift_ms: 0.0,
            invert_polarity: false,
            gain: 1.0,
        }
    }
}

/// Apply exact analytic-signal phase rotation, fractional FFT time shift, and gain to one trace.
pub fn apply_phase_rot_trace(trace: &mut Trace, params: &PhaseRotParams) {
    if !trace.is_live() || trace.samples.is_empty() {
        return;
    }
    let n = trace.samples.len();
    let sign = if params.invert_polarity { -1.0f64 } else { 1.0f64 } * params.gain as f64;
    if params.phase_deg.abs() < 1e-6 && params.shift_ms.abs() < 1e-6 {
        if (sign - 1.0).abs() > 1e-7 {
            for s in &mut trace.samples {
                *s = (*s as f64 * sign) as f32;
            }
        }
        return;
    }

    let nfft = (n * 2).next_power_of_two().max(64);
    let dt_s = trace.dt_s() as f64;
    let df = 1.0 / (nfft as f64 * dt_s);
    let phi = (params.phase_deg as f64).to_radians();
    let tau = params.shift_ms as f64 * 1e-3;

    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(nfft);
    let inv = planner.plan_fft_inverse(nfft);

    let mut buf = vec![Complex::new(0.0, 0.0); nfft];
    for (i, &s) in trace.samples.iter().enumerate() {
        buf[i] = Complex::new(s as f64, 0.0);
    }
    fwd.process(&mut buf);

    let half = nfft / 2;
    buf[0] *= sign;
    for k in 1..half {
        let f = k as f64 * df;
        let angle = -2.0 * PI * f * tau - phi;
        buf[k] *= Complex::from_polar(sign, angle);
        buf[nfft - k] *= Complex::from_polar(sign, -angle);
    }
    buf[half] *= sign * (-2.0 * PI * (half as f64 * df) * tau).cos();

    inv.process(&mut buf);
    let scale = 1.0 / nfft as f64;
    for i in 0..n {
        trace.samples[i] = (buf[i].re * scale) as f32;
    }
}

/// Apply constant-phase rotation, fractional shift, and gain in parallel across traces.
pub fn apply_phase_rot(traces: &mut [Trace], params: &PhaseRotParams) {
    traces.par_iter_mut().for_each(|tr| apply_phase_rot_trace(tr, params));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ricker(n: usize, dt_s: f64, t0_s: f64, fdom: f64) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f64 * dt_s - t0_s;
                let u = PI * fdom * t;
                let u2 = u * u;
                ((1.0 - 2.0 * u2) * (-u2).exp()) as f32
            })
            .collect()
    }

    #[test]
    fn inverse_q_recovers_attenuated_and_dispersed_wavelet() {
        let dt_us = 2000;
        let dt_s = 0.002;
        let n = 512;
        let orig = ricker(n, dt_s, 0.6, 30.0);
        let mut tr = Trace::new(dt_us, orig.clone());
        let params = QCompParams {
            q_value: 60.0,
            reference_freq_hz: 50.0,
            max_gain_db: 30.0,
            mode: QCompMode::Both,
            window_ms: 200.0,
            ..Default::default()
        };
        apply_forward_q_trace(&mut tr, &params);
        let atten_peak = tr.samples.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        assert!(atten_peak < 0.55, "Forward Q should noticeably attenuate the 0.6s wavelet (peak={atten_peak})");

        apply_qcomp_trace(&mut tr, &params);
        // Correlation with original zero-phase wavelet should be very high (> 0.96)
        let num: f64 = orig.iter().zip(&tr.samples).map(|(&a, &b)| a as f64 * b as f64).sum();
        let d1: f64 = orig.iter().map(|&a| (a as f64).powi(2)).sum::<f64>().sqrt();
        let d2: f64 = tr.samples.iter().map(|&b| (b as f64).powi(2)).sum::<f64>().sqrt();
        let corr = num / (d1 * d2).max(1e-12);
        let rec_peak = tr.samples.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        assert!(corr > 0.96, "Inverse-Q recovery correlation too low: {corr:.4}");
        assert!(rec_peak > 0.85, "Inverse-Q amplitude recovery too weak: {rec_peak:.3}");
    }

    #[test]
    fn phase_rot_and_shift_are_invertible() {
        let orig = ricker(256, 0.002, 0.25, 25.0);
        let mut tr = Trace::new(2000, orig.clone());
        apply_phase_rot_trace(
            &mut tr,
            &PhaseRotParams { phase_deg: 45.0, shift_ms: 12.5, invert_polarity: false, gain: 2.0 },
        );
        apply_phase_rot_trace(
            &mut tr,
            &PhaseRotParams { phase_deg: -45.0, shift_ms: -12.5, invert_polarity: false, gain: 0.5 },
        );
        let rms_err = (orig.iter().zip(&tr.samples).map(|(&a, &b)| (a - b).powi(2)).sum::<f32>() / orig.len() as f32).sqrt();
        assert!(rms_err < 1e-4, "Phase rotation + shift round-trip error too high: {rms_err}");
    }
}
