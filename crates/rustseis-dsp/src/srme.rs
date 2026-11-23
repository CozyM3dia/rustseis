use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 2D Surface-Related Multiple Elimination (`w_srme_v3` — Verschuur-Berkhout-Wapenaar 1992).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Srme2dParams {
    /// Nominal shot/receiver grid spacing in meters (default 25.0 m).
    #[serde(default = "default_dx_srme")]
    pub dx_m: f32,
    /// Minimum frequency to include in spatial autoconvolution in Hz (default 4.0 Hz).
    #[serde(default = "default_fmin_srme")]
    pub f_min_hz: f32,
    /// Maximum frequency to include in spatial autoconvolution in Hz (default 80.0 Hz).
    #[serde(default = "default_fmax_srme")]
    pub f_max_hz: f32,
    /// Use source-receiver reciprocity to populate missing offsets (default true).
    #[serde(default = "default_true_s")]
    pub use_reciprocity: bool,
    /// Automatically run adaptive subtraction (`w_asub`) to output primaries (true),
    /// or output the raw predicted multiple model directly (false).
    #[serde(default = "default_true_s")]
    pub subtract_multiples: bool,
    /// Adaptive subtraction settings when `subtract_multiples == true`.
    #[serde(default)]
    pub adaptive: AdaptiveSubParams,
}

fn default_dx_srme() -> f32 { 25.0 }
fn default_fmin_srme() -> f32 { 4.0 }
fn default_fmax_srme() -> f32 { 80.0 }
fn default_true_s() -> bool { true }

impl Default for Srme2dParams {
    fn default() -> Self {
        Self {
            dx_m: 25.0,
            f_min_hz: 4.0,
            f_max_hz: 80.0,
            use_reciprocity: true,
            subtract_multiples: true,
            adaptive: AdaptiveSubParams::default(),
        }
    }
}

/// 2D Adaptive Matching Filter Subtraction (`w_asub`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveSubParams {
    /// Matching filter length in ms (centered zero-lag, default 40.0 ms).
    #[serde(default = "default_filt_ms")]
    pub filter_len_ms: f32,
    /// Time window length in ms for local matching filter estimation (default 400.0 ms).
    #[serde(default = "default_twin_ms")]
    pub time_window_ms: f32,
    /// Space window in traces for 2D matching filter design (default 15 traces).
    #[serde(default = "default_xwin")]
    pub space_window_traces: usize,
    /// Prewhitening regularization fraction (default 0.02 = 2%).
    #[serde(default = "default_reg")]
    pub prewhitening: f32,
    /// Number of IRLS L1 iterations (1 = pure L2 least-squares; 3 = L1 robust against strong primaries).
    #[serde(default = "default_irls")]
    pub irls_iters: usize,
}

fn default_filt_ms() -> f32 { 40.0 }
fn default_twin_ms() -> f32 { 400.0 }
fn default_xwin() -> usize { 15 }
fn default_reg() -> f32 { 0.02 }
fn default_irls() -> usize { 2 }

impl Default for AdaptiveSubParams {
    fn default() -> Self {
        Self {
            filter_len_ms: 40.0,
            time_window_ms: 400.0,
            space_window_traces: 15,
            prewhitening: 0.02,
            irls_iters: 2,
        }
    }
}

/// Solve small real symmetric positive-definite system `A * x = b` (`n x n`) via Cholesky.
fn solve_spd_real(a: &[f64], b: &[f64], n: usize) -> Vec<f64> {
    let mut l = vec![0.0f64; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                l[i * n + j] = sum.max(1e-18).sqrt();
            } else {
                l[i * n + j] = sum / l[j * n + j].max(1e-18);
            }
        }
    }
    let mut y = vec![0.0f64; n];
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[i * n + k] * y[k];
        }
        y[i] = sum / l[i * n + i].max(1e-18);
    }
    let mut x = vec![0.0f64; n];
    for i in (0..n).rev() {
        let mut sum = y[i];
        for k in (i + 1)..n {
            sum -= l[k * n + i] * x[k];
        }
        x[i] = sum / l[i * n + i].max(1e-18);
    }
    x
}

/// Apply 2D windowed least-squares / IRLS-L1 adaptive matching subtraction (`w_asub`):
/// Finds local filter `h` minimizing `|| d - h * m ||_p^p` and outputs `d - h * m`.
pub fn apply_adaptive_subtraction(
    data: &mut [Trace],
    model: &[Trace],
    params: &AdaptiveSubParams,
) -> Result<()> {
    if data.len() != model.len() || data.is_empty() {
        return Err(SeismicError::InvalidFormat(
            "Adaptive subtraction requires non-empty data and model with equal trace counts".into(),
        ));
    }
    let nx = data.len();
    let nt = data[0].samples.len();
    let dt_ms = data[0].dt_ms().max(1e-3);
    let nf = (((params.filter_len_ms / dt_ms).round() as usize) | 1).clamp(3, 61);
    let half_f = nf / 2;
    let tw = ((params.time_window_ms / dt_ms).round() as usize).clamp(nf * 2, nt);
    let t_step = (tw / 2).max(8);
    let xw = params.space_window_traces.clamp(1, nx);
    let x_step = (xw / 2).max(1);
    let reg = params.prewhitening.clamp(1e-5, 0.5) as f64;
    let irls_iters = params.irls_iters.clamp(1, 5);

    // Build overlapping (t, x) patches
    let mut t_wins = Vec::new();
    let mut t0 = 0;
    while t0 < nt {
        let t1 = (t0 + tw).min(nt);
        t_wins.push((t0, t1));
        if t1 == nt { break; }
        t0 += t_step;
    }
    let mut x_wins = Vec::new();
    let mut x0 = 0;
    while x0 < nx {
        let x1 = (x0 + xw).min(nx);
        x_wins.push((x0, x1));
        if x1 == nx { break; }
        x0 += x_step;
    }

    let mut sub_acc = vec![vec![0.0f64; nt]; nx];
    let mut sub_wgt = vec![vec![0.0f64; nt]; nx];

    for &(xa, xb) in &x_wins {
        for &(ta, tb) in &t_wins {
            // Check if model has any non-zero energy in this window
            let mut m_energy = 0.0f64;
            for ix in xa..xb {
                for it in ta..tb {
                    let v = model[ix].samples[it] as f64;
                    m_energy += v * v;
                }
            }
            if m_energy < 1e-20 {
                continue;
            }

            let n_rows = (xb - xa) * (tb - ta);
            let mut weights = vec![1.0f64; n_rows];
            let mut filt = vec![0.0f64; nf];

            for _iter in 0..irls_iters {
                let mut ata = vec![0.0f64; nf * nf];
                let mut atb = vec![0.0f64; nf];
                let mut row_idx = 0usize;

                for ix in xa..xb {
                    let m_tr = &model[ix].samples;
                    let d_tr = &data[ix].samples;
                    for it in ta..tb {
                        let w = weights[row_idx];
                        row_idx += 1;
                        let d_val = d_tr[it] as f64;
                        let mut m_vec = [0.0f64; 64];
                        for k in 0..nf {
                            let idx = it as isize + (k as isize - half_f as isize);
                            if idx >= 0 && (idx as usize) < nt {
                                m_vec[k] = m_tr[idx as usize] as f64;
                            }
                        }
                        for i in 0..nf {
                            let wi = w * m_vec[i];
                            atb[i] += wi * d_val;
                            for j in 0..=i {
                                ata[i * nf + j] += wi * m_vec[j];
                            }
                        }
                    }
                }
                for i in 0..nf {
                    for j in 0..i {
                        ata[j * nf + i] = ata[i * nf + j];
                    }
                }
                let diag_avg = (0..nf).map(|i| ata[i * nf + i]).sum::<f64>() / nf as f64;
                let mu = (reg * diag_avg).max(1e-18);
                for i in 0..nf {
                    ata[i * nf + i] += mu;
                }
                filt = solve_spd_real(&ata, &atb, nf);

                // Update IRLS L1 weights based on residual r = d - h*m
                if irls_iters > 1 {
                    let mut res_abs = Vec::with_capacity(n_rows);
                    for ix in xa..xb {
                        let m_tr = &model[ix].samples;
                        let d_tr = &data[ix].samples;
                        for it in ta..tb {
                            let mut pred = 0.0f64;
                            for k in 0..nf {
                                let idx = it as isize + (k as isize - half_f as isize);
                                if idx >= 0 && (idx as usize) < nt {
                                    pred += filt[k] * m_tr[idx as usize] as f64;
                                }
                            }
                            res_abs.push((d_tr[it] as f64 - pred).abs());
                        }
                    }
                    let max_r = res_abs.iter().cloned().fold(0.0f64, f64::max).max(1e-12);
                    let eps = 0.05 * max_r;
                    for (w, r) in weights.iter_mut().zip(res_abs.iter()) {
                        *w = 1.0 / (r + eps);
                    }
                }
            }

            // Convolve model with matched filter & accumulate with triangular window taper
            let w_xlen = (xb - xa) as f64;
            let w_tlen = (tb - ta) as f64;
            for ix in xa..xb {
                let wx = if xb - xa <= 2 {
                    1.0
                } else {
                    let u = ((ix - xa) as f64 + 0.5) / w_xlen;
                    (std::f64::consts::PI * u).sin().max(0.05)
                };
                let m_tr = &model[ix].samples;
                for it in ta..tb {
                    let wt = if tb - ta <= 2 {
                        1.0
                    } else {
                        let u = ((it - ta) as f64 + 0.5) / w_tlen;
                        (std::f64::consts::PI * u).sin().max(0.05)
                    };
                    let mut pred = 0.0f64;
                    for k in 0..nf {
                        let idx = it as isize + (k as isize - half_f as isize);
                        if idx >= 0 && (idx as usize) < nt {
                            pred += filt[k] * m_tr[idx as usize] as f64;
                        }
                    }
                    let w = wx * wt;
                    sub_acc[ix][it] += pred * w;
                    sub_wgt[ix][it] += w;
                }
            }
        }
    }

    for ix in 0..nx {
        if !data[ix].is_live() {
            continue;
        }
        for it in 0..nt {
            if sub_wgt[ix][it] > 1e-12 {
                let matched = (sub_acc[ix][it] / sub_wgt[ix][it]) as f32;
                data[ix].samples[it] -= matched;
            }
        }
    }
    Ok(())
}

/// 2D Surface-Related Multiple Prediction (`w_srme_v3`).
/// Maps input shots onto a regular surface grid `P(ir, is, w)` (applying reciprocity when enabled),
/// performs frequency-domain matrix multiplication `M(w) = P(w) * P(w)` across frequencies in parallel,
/// and maps predicted surface multiples back to each input trace's `(source, receiver)` position.
pub fn predict_srme_2d(traces: &[Trace], params: &Srme2dParams) -> Result<Vec<Trace>> {
    if traces.len() < 4 || traces[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Need >= 4 traces for 2D SRME".into()));
    }
    let nt = traces[0].samples.len();
    let dt_s = traces[0].dt_s() as f64;
    let dx = params.dx_m.max(1.0) as f64;

    // Project source & receiver coordinates onto the 1D line coordinate `s`
    let has_coords = traces
        .iter()
        .any(|t| t.headers.sou_x.abs() > 1e-3 || t.headers.rec_x.abs() > 1e-3);
    let (ox, oy, ux, uy) = if has_coords {
        let sx0 = traces[0].headers.sou_x;
        let sy0 = traces[0].headers.sou_y;
        let mut far_x = sx0;
        let mut far_y = sy0;
        let mut max_d = 0.0f64;
        for tr in traces {
            let d = (tr.headers.sou_x - sx0).hypot(tr.headers.sou_y - sy0);
            if d > max_d {
                max_d = d;
                far_x = tr.headers.sou_x;
                far_y = tr.headers.sou_y;
            }
        }
        if max_d > 1e-3 {
            (sx0, sy0, (far_x - sx0) / max_d, (far_y - sy0) / max_d)
        } else {
            (sx0, sy0, 1.0, 0.0)
        }
    } else {
        (0.0, 0.0, 1.0, 0.0)
    };

    // Map each trace to integer grid indices `(i_s, i_r)`
    let mut raw_coords: Vec<(f64, f64)> = Vec::with_capacity(traces.len());
    let mut min_pos = f64::INFINITY;
    let mut max_pos = f64::NEG_INFINITY;
    for tr in traces {
        let (s_pos, r_pos) = if has_coords {
            let sp = (tr.headers.sou_x - ox) * ux + (tr.headers.sou_y - oy) * uy;
            let rp = (tr.headers.rec_x - ox) * ux + (tr.headers.rec_y - oy) * uy;
            (sp, rp)
        } else {
            let sp = tr.headers.ffid as f64 * dx;
            let rp = sp - tr.headers.offset as f64;
            (sp, rp)
        };
        min_pos = min_pos.min(s_pos).min(r_pos);
        max_pos = max_pos.max(s_pos).max(r_pos);
        raw_coords.push((s_pos, r_pos));
    }

    let n_grid = (((max_pos - min_pos) / dx).round() as usize + 1).clamp(2, 1024);
    let trace_ij: Vec<(usize, usize)> = raw_coords
        .iter()
        .map(|&(sp, rp)| {
            let is = (((sp - min_pos) / dx).round() as usize).min(n_grid - 1);
            let ir = (((rp - min_pos) / dx).round() as usize).min(n_grid - 1);
            (is, ir)
        })
        .collect();

    // Populate regular 2D data cube P[is, ir] (averaging duplicates & applying reciprocity)
    let mut cell_map: BTreeMap<(usize, usize), (Vec<f64>, usize)> = BTreeMap::new();
    for (idx, &(is, ir)) in trace_ij.iter().enumerate() {
        if !traces[idx].is_live() {
            continue;
        }
        let entry = cell_map.entry((is, ir)).or_insert_with(|| (vec![0.0; nt], 0));
        for it in 0..nt.min(traces[idx].samples.len()) {
            entry.0[it] += traces[idx].samples[it] as f64;
        }
        entry.1 += 1;
    }

    let mut p_time: Vec<Option<Vec<f64>>> = vec![None; n_grid * n_grid];
    for ((is, ir), (mut sum, cnt)) in cell_map {
        let inv = 1.0 / cnt.max(1) as f64;
        for v in &mut sum {
            *v *= inv;
        }
        p_time[is * n_grid + ir] = Some(sum);
    }

    // Reciprocity: P(r, s) = P(s, r)
    if params.use_reciprocity {
        for is in 0..n_grid {
            for ir in 0..n_grid {
                if p_time[is * n_grid + ir].is_none() {
                    if let Some(ref sym) = p_time[ir * n_grid + is] {
                        p_time[is * n_grid + ir] = Some(sym.clone());
                    }
                }
            }
        }
    }

    // Near-offset linear interpolation along receiver axis for missing zero-offset gap
    for is in 0..n_grid {
        let mut live_ir: Vec<usize> = Vec::new();
        for ir in 0..n_grid {
            if p_time[is * n_grid + ir].is_some() {
                live_ir.push(ir);
            }
        }
        if live_ir.len() >= 2 {
            for w in live_ir.windows(2) {
                let (a, b) = (w[0], w[1]);
                if b - a > 1 && b - a <= 12 {
                    let va = p_time[is * n_grid + a].as_ref().unwrap().clone();
                    let vb = p_time[is * n_grid + b].as_ref().unwrap().clone();
                    for ir in (a + 1)..b {
                        let f = (ir - a) as f64 / (b - a) as f64;
                        let interp: Vec<f64> = (0..nt).map(|it| va[it] * (1.0 - f) + vb[it] * f).collect();
                        p_time[is * n_grid + ir] = Some(interp);
                    }
                }
            }
        }
    }

    let nfft = (nt * 2).next_power_of_two().max(64);
    let df = 1.0 / (nfft as f64 * dt_s);
    let k_lo = ((params.f_min_hz.max(0.5) as f64 / df).floor() as usize).clamp(1, nfft / 2);
    let k_hi = ((params.f_max_hz as f64 / df).ceil() as usize).clamp(k_lo, nfft / 2);
    let n_freq = k_hi - k_lo + 1;

    let mut planner = FftPlanner::<f64>::new();
    let fft_fwd = planner.plan_fft_forward(nfft);
    let fft_inv = planner.plan_fft_inverse(nfft);

    // Temporal FFT of populated grid entries -> compact frequency-major cube P_freq[kf_rel][ir * n_grid + is]
    let mut p_freq = vec![Complex::new(0.0, 0.0); n_freq * n_grid * n_grid];
    let mut buf = vec![Complex::new(0.0, 0.0); nfft];
    for is in 0..n_grid {
        for ir in 0..n_grid {
            if let Some(ref tr) = p_time[is * n_grid + ir] {
                buf.fill(Complex::new(0.0, 0.0));
                for it in 0..nt {
                    buf[it] = Complex::new(tr[it], 0.0);
                }
                fft_fwd.process(&mut buf);
                for kf in k_lo..=k_hi {
                    p_freq[(kf - k_lo) * n_grid * n_grid + ir * n_grid + is] = buf[kf];
                }
            }
        }
    }

    // Unique output `(is, ir)` pairs needed for input traces
    let mut needed_pairs: Vec<(usize, usize)> = trace_ij.clone();
    needed_pairs.sort_unstable();
    needed_pairs.dedup();

    // Frequency-domain spatial convolution M(ir, is, w) = sum_k P(ir, k, w) * P(k, is, w) * dx
    let m_freq_by_kf: Vec<Vec<Complex<f64>>> = (0..n_freq)
        .into_par_iter()
        .map(|f_idx| {
            let p_slice = &p_freq[f_idx * n_grid * n_grid..(f_idx + 1) * n_grid * n_grid];
            let mut out = Vec::with_capacity(needed_pairs.len());
            for &(is, ir) in &needed_pairs {
                let mut acc = Complex::new(0.0, 0.0);
                let row_ir = &p_slice[ir * n_grid..(ir + 1) * n_grid];
                for k in 0..n_grid {
                    acc += row_ir[k] * p_slice[k * n_grid + is];
                }
                out.push(acc * dx);
            }
            out
        })
        .collect();

    // Inverse temporal FFT for each unique `(is, ir)` pair
    let inv_n = 1.0 / nfft as f64;
    let pair_waveforms: Vec<Vec<f32>> = (0..needed_pairs.len())
        .into_par_iter()
        .map(|p_idx| {
            let mut spec = vec![Complex::new(0.0, 0.0); nfft];
            for f_idx in 0..n_freq {
                let kf = k_lo + f_idx;
                let v = m_freq_by_kf[f_idx][p_idx];
                spec[kf] = v;
                if kf > 0 && kf < nfft - kf {
                    spec[nfft - kf] = v.conj();
                }
            }
            fft_inv.process(&mut spec);
            (0..nt).map(|it| (spec[it].re * inv_n) as f32).collect()
        })
        .collect();

    let pair_lookup: BTreeMap<(usize, usize), usize> = needed_pairs
        .iter()
        .enumerate()
        .map(|(i, &pair)| (pair, i))
        .collect();

    let mut multiple_model = traces.to_vec();
    for (idx, &pair) in trace_ij.iter().enumerate() {
        if let Some(&p_idx) = pair_lookup.get(&pair) {
            multiple_model[idx].samples = pair_waveforms[p_idx].clone();
        }
    }

    Ok(multiple_model)
}

/// Full 2D SRME tool (`w_srme_v3`): predict surface multiples and optionally subtract via `w_asub`.
pub fn apply_srme_2d(traces: &mut [Trace], params: &Srme2dParams) -> Result<Vec<Trace>> {
    let model = predict_srme_2d(traces, params)?;
    if params.subtract_multiples {
        apply_adaptive_subtraction(traces, &model, &params.adaptive)?;
    } else {
        for (dst, src) in traces.iter_mut().zip(model.iter()) {
            dst.samples.copy_from_slice(&src.samples);
        }
    }
    Ok(model)
}
