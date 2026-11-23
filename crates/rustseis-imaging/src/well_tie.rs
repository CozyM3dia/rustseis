//! Seismic-to-well tie: converts LAS sonic & density logs into acoustic impedance, reflection
//! coefficients, depth-to-time curve, synthetic seismogram (Ricker or extracted statistical
//! wavelet), and optimal bulk-shift / phase-rotation correlation against a stacked seismic trace.

use crate::modeling::ricker_wavelet;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace, TraceHeaders};
use rustseis_io::{LasWell, read_las_file};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WellTieParams {
    /// Path to a CWLS LAS 2.0 / 3.0 file.
    #[serde(default)]
    pub las_path: String,
    /// Optional explicit sonic/velocity curve mnemonic (default: auto-detect DTC/DT/DTCO/VP).
    #[serde(default)]
    pub sonic_curve: Option<String>,
    /// Optional explicit density curve mnemonic (default: auto-detect RHOB/RHOZ/DEN, else Gardner).
    #[serde(default)]
    pub density_curve: Option<String>,
    /// Target CDP in the stacked seismic section (`None` = middle trace).
    #[serde(default)]
    pub target_cdp: Option<i32>,
    /// Two-way traveltime at the top of the logged interval in ms (used if no checkshots given).
    #[serde(default)]
    pub top_twt_ms: Option<f32>,
    /// Overburden average velocity from datum (z=0) to top of log when `top_twt_ms` is None (m/s).
    #[serde(default = "default_v_overburden")]
    pub overburden_vel_mps: f32,
    /// Optional checkshot table `(depth_m, twt_ms)` to calibrate the integrated sonic time-depth curve.
    #[serde(default)]
    pub checkshots: Vec<(f32, f32)>,
    /// Dominant frequency (Hz) for Ricker wavelet when `extract_wavelet == false`.
    #[serde(default = "default_fdom")]
    pub wavelet_freq_hz: f32,
    /// Extract zero-phase statistical wavelet from seismic amplitude spectrum instead of Ricker.
    #[serde(default)]
    pub extract_wavelet: bool,
    /// Maximum bulk time shift to scan (ms).
    #[serde(default = "default_max_shift")]
    pub max_shift_ms: f32,
    /// Scan constant phase rotations `[-180, 180)` degrees in `phase_step_deg` increments (`0` = skip).
    #[serde(default = "default_phase_step")]
    pub phase_step_deg: f32,
}

fn default_v_overburden() -> f32 { 2000.0 }
fn default_fdom() -> f32 { 30.0 }
fn default_max_shift() -> f32 { 80.0 }
fn default_phase_step() -> f32 { 15.0 }

impl Default for WellTieParams {
    fn default() -> Self {
        Self {
            las_path: String::new(),
            sonic_curve: None,
            density_curve: None,
            target_cdp: None,
            top_twt_ms: None,
            overburden_vel_mps: default_v_overburden(),
            checkshots: Vec::new(),
            wavelet_freq_hz: default_fdom(),
            extract_wavelet: false,
            max_shift_ms: default_max_shift(),
            phase_step_deg: default_phase_step(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WellTieResult {
    pub well_name: String,
    pub target_cdp: i32,
    pub depth_range_m: (f32, f32),
    pub twt_range_ms: (f32, f32),
    pub best_shift_ms: f32,
    pub best_phase_deg: f32,
    pub correlation: f32,
    pub zero_shift_correlation: f32,
    pub n_valid_samples: usize,
    pub dt_us: u32,
    /// `(depth_m, twt_ms)` time-depth table (decimated to ≤ 200 knots).
    pub time_depth: Vec<(f32, f32)>,
    /// Synthetic seismogram trace aligned to the seismic time axis.
    pub synthetic_samples: Vec<f32>,
    /// Reflection coefficient series on the seismic time axis.
    pub reflectivity: Vec<f32>,
    /// Wavelet samples used for convolution (centred at `len / 2`).
    pub wavelet: Vec<f32>,
}

/// Fill interior `None` gaps in a log curve by linear interpolation and edge hold.
fn fill_gaps(vals: &[Option<f32>]) -> Option<Vec<f32>> {
    let valid: Vec<(usize, f32)> = vals
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.map(|x| (i, x)))
        .collect();
    if valid.len() < 4 {
        return None;
    }
    let mut out = vec![0.0f32; vals.len()];
    let (first_i, first_v) = valid[0];
    for x in &mut out[..=first_i] {
        *x = first_v;
    }
    for w in valid.windows(2) {
        let (i0, v0) = w[0];
        let (i1, v1) = w[1];
        let span = (i1 - i0) as f32;
        for i in i0..=i1 {
            let f = (i - i0) as f32 / span;
            out[i] = v0 + f * (v1 - v0);
        }
    }
    let (last_i, last_v) = valid[valid.len() - 1];
    for x in &mut out[last_i..] {
        *x = last_v;
    }
    Some(out)
}

/// Extract a zero-phase statistical wavelet from the average amplitude spectrum of `traces`.
pub fn extract_statistical_wavelet(traces: &[Trace], half_len_samples: usize) -> Vec<f32> {
    let n_out = 2 * half_len_samples + 1;
    let Some(first) = traces.first() else {
        return vec![1.0; 1];
    };
    let ns = first.samples.len().max(64);
    let nfft = ns.next_power_of_two();
    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(nfft);
    let inv = planner.plan_fft_inverse(nfft);

    let mut avg_amp = vec![0.0f64; nfft];
    let mut count = 0usize;
    let step = (traces.len() / 64).max(1);
    for tr in traces.iter().step_by(step) {
        if !tr.is_live() {
            continue;
        }
        let mut buf: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); nfft];
        for (i, &s) in tr.samples.iter().take(nfft).enumerate() {
            let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / tr.samples.len().max(2) as f64).cos();
            buf[i] = Complex::new(s as f64 * w, 0.0);
        }
        fwd.process(&mut buf);
        for (a, c) in avg_amp.iter_mut().zip(&buf) {
            *a += c.norm();
        }
        count += 1;
    }
    if count == 0 {
        return vec![1.0];
    }
    // Zero-phase IFFT of smoothed amplitude spectrum
    let mut spec: Vec<Complex<f64>> = avg_amp
        .iter()
        .map(|&a| Complex::new(a / count as f64, 0.0))
        .collect();
    inv.process(&mut spec);

    let mut wav = vec![0.0f32; n_out];
    let mut peak = 0.0f64;
    for k in 0..n_out {
        let lag = k as isize - half_len_samples as isize;
        let idx = ((lag.rem_euclid(nfft as isize)) as usize) % nfft;
        let w = 0.5 + 0.5 * (PI * lag as f64 / (half_len_samples.max(1) as f64)).cos();
        let v = (spec[idx].re / nfft as f64) * w;
        wav[k] = v as f32;
        peak = peak.max(v.abs());
    }
    if peak > 1e-12 {
        for v in &mut wav {
            *v /= peak as f32;
        }
    }
    wav
}

/// Apply constant phase rotation (degrees) to a 1D signal via FFT analytic signal.
pub fn rotate_phase(signal: &[f32], phase_deg: f32) -> Vec<f32> {
    if signal.is_empty() || phase_deg.abs() < 1e-4 {
        return signal.to_vec();
    }
    let n = signal.len();
    let nfft = n.next_power_of_two().max(16);
    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(nfft);
    let inv = planner.plan_fft_inverse(nfft);
    let mut buf: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); nfft];
    for (i, &s) in signal.iter().enumerate() {
        buf[i] = Complex::new(s as f64, 0.0);
    }
    fwd.process(&mut buf);
    let phi = (phase_deg as f64).to_radians();
    let rot_pos = Complex::from_polar(1.0, phi);
    let rot_neg = Complex::from_polar(1.0, -phi);
    let half = nfft / 2;
    for i in 1..half {
        buf[i] *= rot_pos;
    }
    for i in (half + 1)..nfft {
        buf[i] *= rot_neg;
    }
    inv.process(&mut buf);
    (0..n).map(|i| (buf[i].re / nfft as f64) as f32).collect()
}

fn convolve_centered(r: &[f32], wavelet: &[f32]) -> Vec<f32> {
    let n = r.len();
    let hw = wavelet.len() / 2;
    let mut out = vec![0.0f32; n];
    for i in 0..n {
        let mut acc = 0.0f64;
        for (k, &w) in wavelet.iter().enumerate() {
            let j = i as isize + hw as isize - k as isize;
            if j >= 0 && (j as usize) < n {
                acc += r[j as usize] as f64 * w as f64;
            }
        }
        out[i] = acc as f32;
    }
    out
}

fn pearson_window(a: &[f32], b: &[f32], i0: usize, i1: usize, lag: isize) -> f32 {
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut n = 0usize;
    for i in i0..i1.min(a.len()) {
        let j = i as isize + lag;
        if j < 0 || (j as usize) >= b.len() {
            continue;
        }
        let x = a[i] as f64;
        let y = b[j as usize] as f64;
        sx += x;
        sy += y;
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
        n += 1;
    }
    if n < 8 {
        return 0.0;
    }
    let nf = n as f64;
    let cov = sxy - sx * sy / nf;
    let vx = (sxx - sx * sx / nf).max(0.0);
    let vy = (syy - sy * sy / nf).max(0.0);
    let den = (vx * vy).sqrt();
    if den < 1e-12 { 0.0 } else { (cov / den) as f32 }
}

/// Run a complete seismic-to-well tie from a LAS file path against a seismic section.
pub fn compute_well_tie_from_file(traces: &[Trace], params: &WellTieParams) -> Result<WellTieResult> {
    if params.las_path.trim().is_empty() {
        return Err(SeismicError::InvalidParameter {
            param: "las_path".into(),
            message: "LAS file path is required".into(),
        });
    }
    let well = read_las_file(&params.las_path)?;
    compute_well_tie(&well, traces, params)
}

/// Run a seismic-to-well tie using an already-parsed `LasWell`.
pub fn compute_well_tie(
    well: &LasWell,
    traces: &[Trace],
    params: &WellTieParams,
) -> Result<WellTieResult> {
    let vp_raw = well.vp_mps(params.sonic_curve.as_deref())?;
    let rho_raw = well.density_kg_m3(params.density_curve.as_deref(), &vp_raw);
    let vp = fill_gaps(&vp_raw).ok_or_else(|| {
        SeismicError::InvalidFormat(format!("Too few valid sonic/Vp samples in well '{}'", well.name))
    })?;
    let rho = fill_gaps(&rho_raw).ok_or_else(|| {
        SeismicError::InvalidFormat(format!("Too few valid density samples in well '{}'", well.name))
    })?;
    let n_log = well.depths_m.len().min(vp.len()).min(rho.len());
    if n_log < 8 {
        return Err(SeismicError::InvalidFormat("Well log has fewer than 8 depth samples".into()));
    }

    // Integrate two-way traveltime TWT(z) in ms
    let z0 = well.depths_m[0];
    let z1 = well.depths_m[n_log - 1];
    let mut twt_ms = vec![0.0f64; n_log];
    let t_start_ms = params
        .top_twt_ms
        .map(|t| t as f64)
        .unwrap_or_else(|| (2.0 * z0.max(0.0) as f64 / params.overburden_vel_mps.max(300.0) as f64) * 1000.0);
    twt_ms[0] = t_start_ms;
    for i in 1..n_log {
        let dz = (well.depths_m[i] - well.depths_m[i - 1]).abs() as f64;
        let v_avg = 0.5 * (vp[i - 1] + vp[i]) as f64;
        twt_ms[i] = twt_ms[i - 1] + (2.0 * dz / v_avg.max(300.0)) * 1000.0;
    }

    // Optional checkshot calibration: piecewise-linear drift adjustment
    if params.checkshots.len() >= 2 {
        let mut cs = params.checkshots.clone();
        cs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let raw_at = |z: f32| -> f64 {
            let idx = well.depths_m[..n_log]
                .binary_search_by(|d| d.total_cmp(&z))
                .unwrap_or_else(|i| i.min(n_log - 1));
            twt_ms[idx]
        };
        let drifts: Vec<(f32, f64)> = cs.iter().map(|&(z, t_cs)| (z, t_cs as f64 - raw_at(z))).collect();
        for i in 0..n_log {
            let z = well.depths_m[i];
            let drift = if z <= drifts[0].0 {
                drifts[0].1
            } else if z >= drifts[drifts.len() - 1].0 {
                drifts[drifts.len() - 1].1
            } else {
                let mut d_val = 0.0;
                for w in drifts.windows(2) {
                    if z >= w[0].0 && z <= w[1].0 {
                        let f = ((z - w[0].0) / (w[1].0 - w[0].0).max(1e-6)) as f64;
                        d_val = w[0].1 + f * (w[1].1 - w[0].1);
                        break;
                    }
                }
                d_val
            };
            twt_ms[i] += drift;
        }
    }

    let t_min_ms = twt_ms[0];
    let t_max_ms = twt_ms[n_log - 1];

    // Pick target trace from seismic section (or default 2ms synthetic grid if traces empty)
    let (dt_us, ns, target_cdp, seismic_opt): (u32, usize, i32, Option<&Trace>) = if traces.is_empty() {
        let dt_us = 2000u32;
        let ns = ((t_max_ms / 2.0).ceil() as usize + 64).max(256);
        (dt_us, ns, params.target_cdp.unwrap_or(0), None)
    } else {
        let tr = match params.target_cdp {
            Some(c) => traces
                .iter()
                .min_by_key(|t| (t.headers.cdp - c).abs())
                .unwrap_or(&traces[0]),
            None => &traces[traces.len() / 2],
        };
        (tr.dt_us, tr.samples.len(), tr.headers.cdp, Some(tr))
    };

    let dt_ms = dt_us as f64 / 1000.0;
    let dt_s = dt_us as f64 * 1e-6;

    // Resample acoustic impedance Z = rho * Vp onto uniform TWT grid, then compute reflection coefficients
    let mut ai = vec![0.0f64; ns];
    let mut valid_mask = vec![false; ns];
    let mut k = 0usize;
    for it in 0..ns {
        let t = it as f64 * dt_ms;
        if t < t_min_ms || t > t_max_ms {
            continue;
        }
        while k + 1 < n_log && twt_ms[k + 1] < t {
            k += 1;
        }
        let (v_val, r_val) = if k + 1 >= n_log {
            (vp[n_log - 1] as f64, rho[n_log - 1] as f64)
        } else {
            let span = (twt_ms[k + 1] - twt_ms[k]).max(1e-9);
            let f = ((t - twt_ms[k]) / span).clamp(0.0, 1.0);
            (
                vp[k] as f64 + f * (vp[k + 1] - vp[k]) as f64,
                rho[k] as f64 + f * (rho[k + 1] - rho[k]) as f64,
            )
        };
        ai[it] = v_val * r_val;
        valid_mask[it] = true;
    }

    let mut reflectivity = vec![0.0f32; ns];
    for it in 1..ns {
        if valid_mask[it - 1] && valid_mask[it] {
            let denom = ai[it] + ai[it - 1];
            if denom > 1e-6 {
                reflectivity[it] = ((ai[it] - ai[it - 1]) / denom) as f32;
            }
        }
    }

    // Build wavelet (statistical zero-phase from seismic, or Ricker)
    let half_w = ((0.08 / dt_s).round() as usize).clamp(12, 128);
    let wavelet: Vec<f32> = if params.extract_wavelet && !traces.is_empty() {
        extract_statistical_wavelet(traces, half_w)
    } else {
        (0..=(2 * half_w))
            .map(|k| {
                let t_off = (k as isize - half_w as isize) as f64 * dt_s;
                ricker_wavelet(t_off, params.wavelet_freq_hz.max(5.0) as f64) as f32
            })
            .collect()
    };

    let base_synth = convolve_centered(&reflectivity, &wavelet);
    let i0 = ((t_min_ms / dt_ms).floor() as usize).min(ns.saturating_sub(1));
    let i1 = ((t_max_ms / dt_ms).ceil() as usize + 1).min(ns);

    let (mut best_corr, mut best_shift_samples, mut best_phase_deg, mut zero_corr) =
        (0.0f32, 0isize, 0.0f32, 0.0f32);
    let mut best_synth = base_synth.clone();

    if let Some(seis) = seismic_opt {
        zero_corr = pearson_window(&base_synth, &seis.samples, i0, i1, 0);
        best_corr = zero_corr;
        let max_lag = (params.max_shift_ms as f64 / dt_ms).round() as isize;
        let phases: Vec<f32> = if params.phase_step_deg > 0.0 {
            let mut ps = Vec::new();
            let mut p = -180.0f32;
            while p < 180.0 {
                ps.push(p);
                p += params.phase_step_deg.max(5.0);
            }
            ps
        } else {
            vec![0.0]
        };
        for &ph in &phases {
            let cand = rotate_phase(&base_synth, ph);
            for lag in -max_lag..=max_lag {
                let c = pearson_window(&cand, &seis.samples, i0, i1, lag);
                if c > best_corr {
                    best_corr = c;
                    best_shift_samples = lag;
                    best_phase_deg = ph;
                }
            }
        }
        // Apply best phase + bulk shift to returned aligned synthetic
        let rot = rotate_phase(&base_synth, best_phase_deg);
        let mut shifted = vec![0.0f32; ns];
        for i in 0..ns {
            let src = i as isize - best_shift_samples;
            if src >= 0 && (src as usize) < ns {
                shifted[i] = rot[src as usize];
            }
        }
        best_synth = shifted;
    }

    let best_shift_ms = (best_shift_samples as f64 * dt_ms) as f32;
    let td_step = (n_log / 160).max(1);
    let time_depth: Vec<(f32, f32)> = (0..n_log)
        .step_by(td_step)
        .map(|i| (well.depths_m[i], twt_ms[i] as f32 + best_shift_ms))
        .collect();

    let n_valid = vp_raw.iter().filter(|x| x.is_some()).count();

    Ok(WellTieResult {
        well_name: well.name.clone(),
        target_cdp,
        depth_range_m: (z0, z1),
        twt_range_ms: (t_min_ms as f32 + best_shift_ms, t_max_ms as f32 + best_shift_ms),
        best_shift_ms,
        best_phase_deg,
        correlation: best_corr,
        zero_shift_correlation: zero_corr,
        n_valid_samples: n_valid,
        dt_us,
        time_depth,
        synthetic_samples: best_synth,
        reflectivity,
        wavelet,
    })
}

/// Helper to wrap a `WellTieResult` synthetic trace into a `Trace` for viewer comparison.
pub fn well_tie_to_trace(res: &WellTieResult) -> Trace {
    let mut tr = Trace::new(res.dt_us, res.synthetic_samples.clone());
    tr.headers = TraceHeaders {
        traceno: 1,
        ffid: 1,
        chan: 1,
        cdp: res.target_cdp,
        ..Default::default()
    };
    tr.headers
        .custom
        .insert("WELL_CORR".into(), res.correlation as f64);
    tr.headers
        .custom
        .insert("WELL_SHIFT_MS".into(), res.best_shift_ms as f64);
    tr
}
