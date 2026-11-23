use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RadonMode {
    /// Parabolic Radon: `t = tau + q * (x / ref_offset_m)^2` (`q` in ms at `ref_offset_m`).
    #[default]
    Parabolic,
    /// Linear Tau-P: `t = tau + p * x` (`p` in ms/km, i.e., `1e-6 * p` s/m -> `p_s_per_m = p / 1e6`,
    /// or more standard: `p` in ms/1000m = us/m. Here `p_min..p_max` is in ms at `ref_offset_m`,
    /// so `t = tau + q * (x / ref_offset_m)` which unifies UI picking cleanly!).
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RadonFilterAction {
    /// Model events inside `[q_cut_min, q_cut_max]` (typically multiples `q >= q_min`) and subtract from input.
    #[default]
    SubtractZone,
    /// Pass only events inside `[q_cut_min, q_cut_max]` and reconstruct directly.
    PassZone,
}

/// Radon / Tau-P forward & inverse transform parameters (`radon_trans`, `radon_filter`, `taup`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadonParams {
    #[serde(default)]
    pub mode: RadonMode,
    /// Minimum moveout `q_min` (ms at `ref_offset_m`, e.g. -100.0 ms).
    #[serde(default = "default_q_min")]
    pub q_min_ms: f32,
    /// Maximum moveout `q_max` (ms at `ref_offset_m`, e.g. +400.0 ms).
    #[serde(default = "default_q_max")]
    pub q_max_ms: f32,
    /// Number of Radon moveout `q` traces (default 61).
    #[serde(default = "default_nq")]
    pub n_q: usize,
    /// Reference offset in meters (default: max offset in gather if <= 0).
    #[serde(default = "default_ref_off")]
    pub ref_offset_m: f32,
    /// Minimum frequency in Hz (default 4.0 Hz).
    #[serde(default = "default_fmin")]
    pub f_min_hz: f32,
    /// Maximum frequency in Hz (default 90.0 Hz).
    #[serde(default = "default_fmax")]
    pub f_max_hz: f32,
    /// Tikhonov damping fraction (prewhitening, default 0.01 = 1%).
    #[serde(default = "default_damping")]
    pub damping: f32,
    /// Number of IRLS high-resolution sparse iterations (0 = standard L2 Radon, 2–3 = sparse Cauchy Radon).
    #[serde(default = "default_sparse_iters")]
    pub sparse_iters: usize,
    /// Filter mode (SubtractZone for ProMAX demultiple, or PassZone).
    #[serde(default)]
    pub action: RadonFilterAction,
    /// Moveout zone `[q_zone_min_ms, q_zone_max_ms]` to subtract (multiples) or pass (primaries).
    #[serde(default = "default_zone_min")]
    pub q_zone_min_ms: f32,
    #[serde(default = "default_zone_max")]
    pub q_zone_max_ms: f32,
    /// Taper width across `q` zone boundary in ms (default 20.0 ms).
    #[serde(default = "default_q_taper")]
    pub q_taper_ms: f32,
    /// Optional time-variant zone polygon `(time_ms, q_min_ms, q_max_ms)` overriding fixed zone.
    #[serde(default)]
    pub time_variant_zone: Vec<(f32, f32, f32)>,
}

fn default_q_min() -> f32 { -100.0 }
fn default_q_max() -> f32 { 400.0 }
fn default_nq() -> usize { 61 }
fn default_ref_off() -> f32 { 0.0 }
fn default_fmin() -> f32 { 4.0 }
fn default_fmax() -> f32 { 90.0 }
fn default_damping() -> f32 { 0.01 }
fn default_sparse_iters() -> usize { 2 }
fn default_zone_min() -> f32 { 40.0 }
fn default_zone_max() -> f32 { 400.0 }
fn default_q_taper() -> f32 { 20.0 }

impl Default for RadonParams {
    fn default() -> Self {
        Self {
            mode: RadonMode::Parabolic,
            q_min_ms: -100.0,
            q_max_ms: 400.0,
            n_q: 61,
            ref_offset_m: 0.0,
            f_min_hz: 4.0,
            f_max_hz: 90.0,
            damping: 0.01,
            sparse_iters: 2,
            action: RadonFilterAction::SubtractZone,
            q_zone_min_ms: 40.0,
            q_zone_max_ms: 400.0,
            q_taper_ms: 20.0,
            time_variant_zone: Vec::new(),
        }
    }
}

/// Radon domain panel `(tau, q)` for visual QC (`L3 render_radon_panel`) and interactive mute picking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadonPanel {
    pub mode: RadonMode,
    pub ref_offset_m: f32,
    pub dt_us: u32,
    pub times_ms: Vec<f32>,
    pub q_values_ms: Vec<f32>,
    /// Row-major `[n_q * n_t]` Radon transform amplitudes (`panel[iq * nt + it]`).
    pub data: Vec<f32>,
}

/// Solve complex Hermitian positive-definite system `A * x = b` (`n x n`) using Cholesky `L * L^H`.
/// Falls back gracefully to diagonal scaling if ill-conditioned.
fn solve_hpd_cholesky(a: &[Complex<f64>], b: &[Complex<f64>], n: usize) -> Vec<Complex<f64>> {
    let mut l = vec![Complex::new(0.0, 0.0); n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum -= l[i * n + k] * l[j * n + k].conj();
            }
            if i == j {
                let diag = sum.re.max(1e-18);
                l[i * n + j] = Complex::new(diag.sqrt(), 0.0);
            } else {
                l[i * n + j] = sum / l[j * n + j].re.max(1e-18);
            }
        }
    }

    // Forward substitution: L * y = b
    let mut y = vec![Complex::new(0.0, 0.0); n];
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[i * n + k] * y[k];
        }
        y[i] = sum / l[i * n + i].re.max(1e-18);
    }

    // Backward substitution: L^H * x = y
    let mut x = vec![Complex::new(0.0, 0.0); n];
    for i in (0..n).rev() {
        let mut sum = y[i];
        for k in (i + 1)..n {
            sum -= l[k * n + i].conj() * x[k];
        }
        x[i] = sum / l[i * n + i].re.max(1e-18);
    }
    x
}

fn normalized_offsets(traces: &[Trace], mode: RadonMode, ref_offset_m: f32) -> (f64, Vec<f64>) {
    let max_off = traces
        .iter()
        .map(|t| {
            let o = if t.headers.aoffset > 0.0 {
                t.headers.aoffset as f64
            } else {
                t.headers.offset.abs() as f64
            };
            o
        })
        .fold(0.0f64, f64::max);
    let ref_off = if ref_offset_m > 1.0 {
        ref_offset_m as f64
    } else if max_off > 1.0 {
        max_off
    } else {
        (traces.len().saturating_sub(1).max(1) as f64) * 25.0
    };

    let phi: Vec<f64> = traces
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let x = if max_off > 1.0 {
                match mode {
                    RadonMode::Parabolic => {
                        if t.headers.aoffset > 0.0 {
                            t.headers.aoffset as f64
                        } else {
                            t.headers.offset.abs() as f64
                        }
                    }
                    RadonMode::Linear => {
                        if t.headers.offset.abs() > 0.0 {
                            t.headers.offset as f64
                        } else {
                            t.headers.aoffset as f64
                        }
                    }
                }
            } else {
                i as f64 * 25.0
            };
            let u = x / ref_off;
            match mode {
                RadonMode::Parabolic => u * u,
                RadonMode::Linear => u,
            }
        })
        .collect();
    (ref_off, phi)
}

/// Forward high-resolution least-squares Radon transform `(t, x) -> (tau, q)`.
pub fn forward_radon_gather(traces: &[Trace], params: &RadonParams) -> Result<RadonPanel> {
    if traces.len() < 3 || traces[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Need >= 3 traces for Radon transform".into()));
    }
    let nx = traces.len();
    let nt = traces[0].samples.len();
    let dt_us = traces[0].dt_us;
    let dt_s = traces[0].dt_s() as f64;
    let nq = params.n_q.clamp(5, 256);
    let (ref_off, phi) = normalized_offsets(traces, params.mode, params.ref_offset_m);

    let q_vals_ms: Vec<f32> = (0..nq)
        .map(|i| {
            let frac = i as f32 / (nq - 1).max(1) as f32;
            params.q_min_ms + frac * (params.q_max_ms - params.q_min_ms)
        })
        .collect();
    let q_vals_s: Vec<f64> = q_vals_ms.iter().map(|&q| q as f64 / 1000.0).collect();

    let nfft = (nt + nt / 2).next_power_of_two().max(64);
    let mut planner = FftPlanner::<f64>::new();
    let fft_fwd = planner.plan_fft_forward(nfft);
    let fft_inv = planner.plan_fft_inverse(nfft);

    // Temporal FFT of input gather
    let mut d_spec = vec![Complex::new(0.0, 0.0); nx * nfft];
    for ix in 0..nx {
        let row = &mut d_spec[ix * nfft..(ix + 1) * nfft];
        if traces[ix].is_live() {
            for it in 0..nt.min(traces[ix].samples.len()) {
                row[it] = Complex::new(traces[ix].samples[it] as f64, 0.0);
            }
            fft_fwd.process(row);
        }
    }

    let df = 1.0 / (nfft as f64 * dt_s);
    let k_lo = ((params.f_min_hz.max(0.5) as f64 / df).floor() as usize).clamp(1, nfft / 2);
    let k_hi = ((params.f_max_hz as f64 / df).ceil() as usize).clamp(k_lo, nfft / 2);
    let damp = (params.damping.max(1e-5)) as f64;
    let sparse_iters = params.sparse_iters.min(5);

    // Solve per frequency bin in parallel using Hermitian Toeplitz Gram construction + phase recurrence
    // (cuts matrix assembly by ~25x and eliminates nx*nq sin/cos calls per frequency!).
    let q0_s = q_vals_s[0];
    let dq_s = if nq > 1 { q_vals_s[1] - q_vals_s[0] } else { 0.0 };

    let freq_solutions: Vec<(usize, Vec<Complex<f64>>)> = (k_lo..=k_hi)
        .into_par_iter()
        .map(|kf| {
            let omega = 2.0 * PI * (kf as f64) * df;

            // 1. Compute rhs = L^H * d AND Toeplitz first-column lags g[0..nq] in ONE pass using phase recurrence!
            //    L[ix, iq] = exp(-i * omega * (q0 + iq*dq) * phi[ix]) = e0 * step^iq
            //    G[i, j] = sum_x conj(L[x, i]) * L[x, j] = sum_x exp(+i * omega * (i - j) * dq * phi[x])
            //    So for lag = i - j >= 0: g[lag] = sum_x (conj(step))^lag!
            let mut rhs = vec![Complex::new(0.0, 0.0); nq];
            let mut g_col = vec![Complex::new(0.0, 0.0); nq];
            for ix in 0..nx {
                let d_val = d_spec[ix * nfft + kf];
                let p = phi[ix];
                let e0_conj = Complex::from_polar(1.0, omega * q0_s * p); // conj(L[ix, 0])
                let step_conj = Complex::from_polar(1.0, omega * dq_s * p); // conj(step)
                let mut e_rhs = e0_conj * d_val;
                let mut e_gram = Complex::new(1.0, 0.0);
                for iq in 0..nq {
                    rhs[iq] += e_rhs;
                    g_col[iq] += e_gram;
                    e_rhs *= step_conj;
                    e_gram *= step_conj;
                }
            }

            // Fill Hermitian Toeplitz Gram matrix from its single first column g_col[0..nq]
            let mut gram = vec![Complex::new(0.0, 0.0); nq * nq];
            for i in 0..nq {
                gram[i * nq + i] = Complex::new(g_col[0].re, 0.0);
                for j in 0..i {
                    let val = g_col[i - j];
                    gram[i * nq + j] = val;
                    gram[j * nq + i] = val.conj();
                }
            }

            let trace_avg = (0..nq).map(|i| gram[i * nq + i].re).sum::<f64>() / nq as f64;
            let mu_base = (damp * trace_avg).max(1e-12);

            // Iteration 0: standard L2 damped least-squares
            let mut a_sys = gram.clone();
            for i in 0..nq {
                a_sys[i * nq + i] += Complex::new(mu_base, 0.0);
            }
            let mut m = solve_hpd_cholesky(&a_sys, &rhs, nq);

            // Optional IRLS high-resolution Cauchy sparsification (Sacchi & Ulrych 1995)
            for _ in 0..sparse_iters {
                let max_m = m.iter().map(|c| c.norm()).fold(0.0f64, f64::max).max(1e-12);
                let sigma = 0.08 * max_m;
                let sigma2 = sigma * sigma;
                a_sys.copy_from_slice(&gram);
                for i in 0..nq {
                    let w_inv = 1.0 / (1.0 + m[i].norm_sqr() / sigma2);
                    // Penalize small amplitudes strongly while preserving strong coherent peaks
                    let reg = mu_base * (0.15 + 6.0 * w_inv);
                    a_sys[i * nq + i] += Complex::new(reg, 0.0);
                }
                m = solve_hpd_cholesky(&a_sys, &rhs, nq);
            }

            (kf, m)
        })
        .collect();

    // Assemble Radon spectrum and inverse FFT to (tau, q)
    let mut m_spec = vec![Complex::new(0.0, 0.0); nq * nfft];
    for (kf, m_vec) in freq_solutions {
        for iq in 0..nq {
            m_spec[iq * nfft + kf] = m_vec[iq];
            if kf > 0 && kf < nfft - kf {
                m_spec[iq * nfft + (nfft - kf)] = m_vec[iq].conj();
            }
        }
    }

    let inv_n = 1.0 / nfft as f64;
    let mut data = vec![0.0f32; nq * nt];
    for iq in 0..nq {
        let row = &mut m_spec[iq * nfft..(iq + 1) * nfft];
        fft_inv.process(row);
        for it in 0..nt {
            data[iq * nt + it] = (row[it].re * inv_n) as f32;
        }
    }

    let times_ms: Vec<f32> = (0..nt).map(|it| (it as f64 * dt_s * 1000.0) as f32).collect();
    Ok(RadonPanel {
        mode: params.mode,
        ref_offset_m: ref_off as f32,
        dt_us,
        times_ms,
        q_values_ms: q_vals_ms,
        data,
    })
}

/// Inverse Radon transform: map `(tau, q)` RadonPanel back to `(t, x)` at target trace offsets.
pub fn inverse_radon_to_traces(
    panel: &RadonPanel,
    template_traces: &[Trace],
    f_min_hz: f32,
    f_max_hz: f32,
) -> Result<Vec<Trace>> {
    let nx = template_traces.len();
    let nq = panel.q_values_ms.len();
    let nt = panel.times_ms.len();
    if nx == 0 || nq == 0 || nt == 0 {
        return Err(SeismicError::InvalidFormat("Empty Radon panel or template".into()));
    }
    let dt_s = panel.dt_us as f64 * 1e-6;
    let (_, phi) = normalized_offsets(template_traces, panel.mode, panel.ref_offset_m);
    let q_vals_s: Vec<f64> = panel.q_values_ms.iter().map(|&q| q as f64 / 1000.0).collect();

    let nfft = (nt + nt / 2).next_power_of_two().max(64);
    let mut planner = FftPlanner::<f64>::new();
    let fft_fwd = planner.plan_fft_forward(nfft);
    let fft_inv = planner.plan_fft_inverse(nfft);

    let mut m_spec = vec![Complex::new(0.0, 0.0); nq * nfft];
    for iq in 0..nq {
        let row = &mut m_spec[iq * nfft..(iq + 1) * nfft];
        for it in 0..nt {
            row[it] = Complex::new(panel.data[iq * nt + it] as f64, 0.0);
        }
        fft_fwd.process(row);
    }

    let df = 1.0 / (nfft as f64 * dt_s);
    let k_lo = ((f_min_hz.max(0.5) as f64 / df).floor() as usize).clamp(1, nfft / 2);
    let k_hi = ((f_max_hz as f64 / df).ceil() as usize).clamp(k_lo, nfft / 2);

    let q0_s = q_vals_s[0];
    let dq_s = if nq > 1 { q_vals_s[1] - q_vals_s[0] } else { 0.0 };
    let recon_freq: Vec<(usize, Vec<Complex<f64>>)> = (k_lo..=k_hi)
        .into_par_iter()
        .map(|kf| {
            let omega = 2.0 * PI * (kf as f64) * df;
            let mut d_kf = vec![Complex::new(0.0, 0.0); nx];
            for ix in 0..nx {
                let p = phi[ix];
                let mut e = Complex::from_polar(1.0, -omega * q0_s * p);
                let step = Complex::from_polar(1.0, -omega * dq_s * p);
                let mut acc = Complex::new(0.0, 0.0);
                for iq in 0..nq {
                    acc += e * m_spec[iq * nfft + kf];
                    e *= step;
                }
                d_kf[ix] = acc;
            }
            (kf, d_kf)
        })
        .collect();

    let mut d_spec = vec![Complex::new(0.0, 0.0); nx * nfft];
    for (kf, d_vec) in recon_freq {
        for ix in 0..nx {
            d_spec[ix * nfft + kf] = d_vec[ix];
            if kf > 0 && kf < nfft - kf {
                d_spec[ix * nfft + (nfft - kf)] = d_vec[ix].conj();
            }
        }
    }

    let inv_n = 1.0 / nfft as f64;
    let mut out = template_traces.to_vec();
    for ix in 0..nx {
        let row = &mut d_spec[ix * nfft..(ix + 1) * nfft];
        fft_inv.process(row);
        let n_out = out[ix].samples.len().min(nt);
        for it in 0..n_out {
            out[ix].samples[it] = (row[it].re * inv_n) as f32;
        }
    }
    Ok(out)
}

fn zone_bounds_at(params: &RadonParams, t_ms: f32) -> (f32, f32) {
    if params.time_variant_zone.is_empty() {
        return (params.q_zone_min_ms, params.q_zone_max_ms);
    }
    let tv = &params.time_variant_zone;
    if t_ms <= tv[0].0 {
        return (tv[0].1, tv[0].2);
    }
    let last = tv[tv.len() - 1];
    if t_ms >= last.0 {
        return (last.1, last.2);
    }
    for w in tv.windows(2) {
        if t_ms >= w[0].0 && t_ms <= w[1].0 {
            let f = (t_ms - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
            return (w[0].1 + f * (w[1].1 - w[0].1), w[0].2 + f * (w[1].2 - w[0].2));
        }
    }
    (last.1, last.2)
}

#[inline]
fn zone_weight(q: f32, q_lo: f32, q_hi: f32, taper: f32) -> f32 {
    let tap = taper.max(1e-3);
    let w_lo = if q >= q_lo {
        1.0
    } else if q <= q_lo - tap {
        0.0
    } else {
        let u = (q - (q_lo - tap)) / tap;
        0.5 * (1.0 - (std::f32::consts::PI * u).cos())
    };
    let w_hi = if q <= q_hi {
        1.0
    } else if q >= q_hi + tap {
        0.0
    } else {
        let u = (q - q_hi) / tap;
        0.5 * (1.0 + (std::f32::consts::PI * u).cos())
    };
    w_lo * w_hi
}

/// Apply Radon Demultiple or Pass-Zone Filter to a single gather (`radon_filter`).
/// Returns the computed Radon panel (pre-mute) for optional visual QC.
pub fn apply_radon_filter_gather(traces: &mut [Trace], params: &RadonParams) -> Result<RadonPanel> {
    let panel = forward_radon_gather(traces, params)?;
    let nq = panel.q_values_ms.len();
    let nt = panel.times_ms.len();

    let mut masked = panel.clone();
    for it in 0..nt {
        let (q_lo, q_hi) = zone_bounds_at(params, panel.times_ms[it]);
        for iq in 0..nq {
            let q = panel.q_values_ms[iq];
            let w = zone_weight(q, q_lo, q_hi, params.q_taper_ms);
            masked.data[iq * nt + it] *= w;
        }
    }

    let modeled = inverse_radon_to_traces(&masked, traces, params.f_min_hz, params.f_max_hz)?;
    match params.action {
        RadonFilterAction::SubtractZone => {
            for (tr, m) in traces.iter_mut().zip(modeled.iter()) {
                if !tr.is_live() {
                    continue;
                }
                for (s, &ms) in tr.samples.iter_mut().zip(m.samples.iter()) {
                    *s -= ms;
                }
            }
        }
        RadonFilterAction::PassZone => {
            for (tr, m) in traces.iter_mut().zip(modeled.iter()) {
                if !tr.is_live() {
                    continue;
                }
                tr.samples.copy_from_slice(&m.samples);
            }
        }
    }
    Ok(panel)
}
