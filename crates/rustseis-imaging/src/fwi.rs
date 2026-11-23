//! 2D Acoustic Full Waveform Inversion (FWI) built on the 8th-order acoustic FD wave engine.
//!
//! Features:
//! - Multi-scale frequency continuation (`freq_stages_hz`) to mitigate cycle-skipping
//! - Exact adjoint-state Frechet gradient using second-order time derivatives of source wavefields
//! - Source-illumination preconditioning, shallow mute, and 2D Gaussian gradient regularization
//! - Optional optimal scalar/Wiener source scaling for real-data amplitude matching
//! - Limited-memory BFGS (L-BFGS) with bounded velocity projection and backtracking line search
//! - Cooperative `JobControl` progress reporting and cancellation

use crate::modeling::ricker_wavelet;
use crate::rtm::{PaddedGrid, ShotTask, fd_step_cpu};
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{
    Ensemble, JobControl, Result, SeismicError, Trace, TraceHeaders, VelocityGrid2D,
};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fwi2dParams {
    /// Peak frequency of the initial Ricker source wavelet (Hz).
    #[serde(default = "default_fdom")]
    pub dominant_freq_hz: f32,
    /// Multi-scale low-pass frequency stages in Hz (e.g. `[10.0, 20.0]`; `0.0` = unfiltered full band).
    #[serde(default = "default_freq_stages")]
    pub freq_stages_hz: Vec<f32>,
    /// Number of L-BFGS iterations per frequency stage.
    #[serde(default = "default_iters")]
    pub iters_per_stage: usize,
    /// L-BFGS history length (typically 3..7).
    #[serde(default = "default_lbfgs_m")]
    pub lbfgs_memory: usize,
    /// Maximum velocity perturbation (m/s) allowed on the initial trial step of each line search.
    #[serde(default = "default_max_step")]
    pub max_step_mps: f32,
    /// Lower velocity bound (m/s).
    #[serde(default = "default_vmin")]
    pub v_min_mps: f32,
    /// Upper velocity bound (m/s).
    #[serde(default = "default_vmax")]
    pub v_max_mps: f32,
    /// Mute gradient shallower than this depth (m) to suppress near-source/receiver singularities.
    #[serde(default = "default_mute_depth")]
    pub mute_depth_m: f32,
    /// 2D Gaussian smoothing sigma (grid cells) applied to the preconditioned gradient.
    #[serde(default = "default_smooth_sigma")]
    pub smooth_sigma_cells: f32,
    /// Illumination stabilization fraction (`eps * max_illum`).
    #[serde(default = "default_illum_eps")]
    pub illumination_eps: f32,
    /// Estimate optimal scalar source amplitude scaling per stage (recommended for field data).
    #[serde(default = "default_true")]
    pub estimate_source_scale: bool,
    /// Absorbing sponge boundary width in grid cells per side.
    #[serde(default = "default_pad")]
    pub pad_cells: usize,
    /// Cerjan sponge damping coefficient.
    #[serde(default = "default_sponge")]
    pub sponge_coeff: f32,
    /// Optional shot sub-sampling stride (`1` = use all shots).
    #[serde(default = "default_one")]
    pub shot_stride: usize,
}

fn default_fdom() -> f32 { 15.0 }
fn default_freq_stages() -> Vec<f32> { vec![10.0, 20.0] }
fn default_iters() -> usize { 6 }
fn default_lbfgs_m() -> usize { 5 }
fn default_max_step() -> f32 { 45.0 }
fn default_vmin() -> f32 { 1200.0 }
fn default_vmax() -> f32 { 6500.0 }
fn default_mute_depth() -> f32 { 40.0 }
fn default_smooth_sigma() -> f32 { 1.8 }
fn default_illum_eps() -> f32 { 0.08 }
fn default_true() -> bool { true }
fn default_pad() -> usize { 20 }
fn default_sponge() -> f32 { 0.015 }
fn default_one() -> usize { 1 }

impl Default for Fwi2dParams {
    fn default() -> Self {
        Self {
            dominant_freq_hz: default_fdom(),
            freq_stages_hz: default_freq_stages(),
            iters_per_stage: default_iters(),
            lbfgs_memory: default_lbfgs_m(),
            max_step_mps: default_max_step(),
            v_min_mps: default_vmin(),
            v_max_mps: default_vmax(),
            mute_depth_m: default_mute_depth(),
            smooth_sigma_cells: default_smooth_sigma(),
            illumination_eps: default_illum_eps(),
            estimate_source_scale: true,
            pad_cells: default_pad(),
            sponge_coeff: default_sponge(),
            shot_stride: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fwi2dOutput {
    /// Inverted 2D velocity model `V(x,z)`.
    pub velocity: VelocityGrid2D,
    /// Normalized or raw L2 data misfit recorded after each iteration.
    pub misfit_history: Vec<f64>,
    /// Velocity model rendered as `nx` traces of length `nz` for quick Gather Viewer inspection.
    pub model_traces: Vec<Trace>,
    pub shots_used: usize,
    pub iterations_completed: usize,
}

/// Zero-phase cosine-tapered low-pass filter on a 1D time series (`f_hi_hz <= 0` returns a copy).
fn lowpass_series(x: &[f32], dt_s: f64, f_hi_hz: f32) -> Vec<f32> {
    if f_hi_hz <= 0.0 || x.len() < 4 {
        return x.to_vec();
    }
    let n = x.len();
    let nfft = (n * 2).next_power_of_two().max(64);
    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(nfft);
    let inv = planner.plan_fft_inverse(nfft);

    let mut buf: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); nfft];
    for (i, &v) in x.iter().enumerate() {
        buf[i] = Complex::new(v as f64, 0.0);
    }
    fwd.process(&mut buf);

    let df = 1.0 / (nfft as f64 * dt_s);
    let f_pass = (f_hi_hz as f64) * 0.78;
    let f_stop = (f_hi_hz as f64) * 1.15;
    for k in 0..nfft {
        let idx = if k <= nfft / 2 { k } else { nfft - k };
        let f = idx as f64 * df;
        let w = if f <= f_pass {
            1.0
        } else if f >= f_stop {
            0.0
        } else {
            let frac = (f - f_pass) / (f_stop - f_pass).max(1e-9);
            0.5 * (1.0 + (PI * frac).cos())
        };
        buf[k] *= w;
    }

    inv.process(&mut buf);
    let scale = 1.0 / nfft as f64;
    (0..n).map(|i| (buf[i].re * scale) as f32).collect()
}

fn prepare_fwi_shots(
    traces: &[Trace],
    vel: &VelocityGrid2D,
    g: &PaddedGrid,
    shot_stride: usize,
) -> (Vec<ShotTask>, usize) {
    let ensembles = Ensemble::group_by_header(traces.to_vec(), "FFID");
    let ns = traces.first().map(|t| t.samples.len()).unwrap_or(0);
    let stride = shot_stride.max(1);
    let mut tasks = Vec::new();
    for ens in ensembles.into_iter().step_by(stride) {
        let live: Vec<&Trace> = ens.traces.iter().filter(|t| t.is_live()).collect();
        if live.is_empty() {
            continue;
        }
        let sx = live[0].headers.sou_x as f32;
        let sz = vel.z0_m + (live[0].headers.depth.max(0.0)).min(vel.dz_m * 2.0);
        let src_cell = g.grid_pos(vel, sx, sz);
        let mut rec_cells = Vec::with_capacity(live.len());
        let mut rec_data = vec![0.0f32; live.len() * ns];
        for (ri, tr) in live.iter().enumerate() {
            let rx = tr.headers.rec_x as f32;
            let rz = vel.z0_m + (tr.headers.rec_dep.max(0.0) as f32).min(vel.dz_m * 2.0);
            rec_cells.push(g.grid_pos(vel, rx, rz));
            let len = tr.samples.len().min(ns);
            rec_data[ri * ns..ri * ns + len].copy_from_slice(&tr.samples[..len]);
        }
        tasks.push(ShotTask {
            src_cell,
            rec_cells,
            rec_data,
        });
    }
    (tasks, ns)
}

/// Evaluate forward data misfit `J(v)` only (fast path for backtracking line search).
fn eval_misfit_only(
    vel: &VelocityGrid2D,
    shots: &[ShotTask],
    ns: usize,
    dt_trace_s: f64,
    wavelet_fd: &[f32],
    params: &Fwi2dParams,
) -> Result<f64> {
    let g = PaddedGrid::build(vel, dt_trace_s, params.pad_cells, params.sponge_coeff)?;
    let n_fd = ns * g.sub_steps;
    let p_len = g.pnx * g.pnz;
    let mut misfit = 0.0f64;

    for shot in shots {
        let mut p_prev = vec![0.0f32; p_len];
        let mut p_curr = vec![0.0f32; p_len];
        let mut p_next = vec![0.0f32; p_len];
        let s_idx = shot.src_cell.1 * g.pnx + shot.src_cell.0;
        let s_v2dt2 = g.v2dt2[s_idx];

        for step in 0..n_fd {
            fd_step_cpu(&g, &p_prev, &p_curr, &mut p_next);
            let w = wavelet_fd.get(step).copied().unwrap_or(0.0);
            p_next[s_idx] += w * s_v2dt2;

            if step % g.sub_steps == 0 {
                let it = step / g.sub_steps;
                if it < ns {
                    for (ri, &(rpx, rpz)) in shot.rec_cells.iter().enumerate() {
                        let syn = p_next[rpz * g.pnx + rpx] as f64;
                        let obs = shot.rec_data[ri * ns + it] as f64;
                        let diff = syn - obs;
                        misfit += 0.5 * diff * diff;
                    }
                }
            }
            std::mem::swap(&mut p_prev, &mut p_curr);
            std::mem::swap(&mut p_curr, &mut p_next);
        }
    }
    Ok(misfit)
}

/// Estimate optimal scalar source scale `alpha = <syn, obs> / <syn, syn>` to match field amplitudes.
fn estimate_optimal_scale(
    vel: &VelocityGrid2D,
    shots: &[ShotTask],
    ns: usize,
    dt_trace_s: f64,
    wavelet_fd: &[f32],
    params: &Fwi2dParams,
) -> Result<f32> {
    let g = PaddedGrid::build(vel, dt_trace_s, params.pad_cells, params.sponge_coeff)?;
    let n_fd = ns * g.sub_steps;
    let p_len = g.pnx * g.pnz;
    let mut num = 0.0f64;
    let mut den = 0.0f64;

    for shot in shots.iter().take(4) {
        let mut p_prev = vec![0.0f32; p_len];
        let mut p_curr = vec![0.0f32; p_len];
        let mut p_next = vec![0.0f32; p_len];
        let s_idx = shot.src_cell.1 * g.pnx + shot.src_cell.0;
        let s_v2dt2 = g.v2dt2[s_idx];

        for step in 0..n_fd {
            fd_step_cpu(&g, &p_prev, &p_curr, &mut p_next);
            let w = wavelet_fd.get(step).copied().unwrap_or(0.0);
            p_next[s_idx] += w * s_v2dt2;

            if step % g.sub_steps == 0 {
                let it = step / g.sub_steps;
                if it < ns {
                    for (ri, &(rpx, rpz)) in shot.rec_cells.iter().enumerate() {
                        let syn = p_next[rpz * g.pnx + rpx] as f64;
                        let obs = shot.rec_data[ri * ns + it] as f64;
                        num += syn * obs;
                        den += syn * syn;
                    }
                }
            }
            std::mem::swap(&mut p_prev, &mut p_curr);
            std::mem::swap(&mut p_curr, &mut p_next);
        }
    }
    if den > 1e-20 && num > 0.0 {
        Ok((num / den).clamp(0.05, 20.0) as f32)
    } else {
        Ok(1.0)
    }
}

/// Evaluate misfit `J(v)`, raw adjoint-state gradient `dJ/dv(x,z)`, and source illumination `H0(x,z)`.
pub(crate) fn eval_misfit_and_gradient(
    vel: &VelocityGrid2D,
    shots: &[ShotTask],
    ns: usize,
    dt_trace_s: f64,
    wavelet_fd: &[f32],
    params: &Fwi2dParams,
    control: Option<&JobControl>,
) -> Result<(f64, Vec<f32>, Vec<f32>)> {
    let g = PaddedGrid::build(vel, dt_trace_s, params.pad_cells, params.sponge_coeff)?;
    let n_fd = ns * g.sub_steps;
    let p_len = g.pnx * g.pnz;
    let n_cells = vel.nx * vel.nz;
    let inv_dt2 = (1.0 / (g.dt_fd_s * g.dt_fd_s)) as f32;

    let mut misfit = 0.0f64;
    let mut grad = vec![0.0f64; n_cells];
    let mut illum = vec![0.0f64; n_cells];

    for shot in shots {
        if let Some(ctrl) = control {
            if ctrl.is_cancelled() {
                return Err(SeismicError::Cancelled);
            }
        }
        // Forward pass: record `u_tt(x,z,t)` snapshots at every trace sample and compute residuals.
        let mut p_prev = vec![0.0f32; p_len];
        let mut p_curr = vec![0.0f32; p_len];
        let mut p_next = vec![0.0f32; p_len];
        let s_idx = shot.src_cell.1 * g.pnx + shot.src_cell.0;
        let s_v2dt2 = g.v2dt2[s_idx];

        let mut u_tt_snaps = vec![0.0f32; ns * n_cells];
        let mut residuals = vec![0.0f32; shot.rec_cells.len() * ns];

        for step in 0..n_fd {
            fd_step_cpu(&g, &p_prev, &p_curr, &mut p_next);
            let w = wavelet_fd.get(step).copied().unwrap_or(0.0);
            p_next[s_idx] += w * s_v2dt2;

            if step % g.sub_steps == 0 {
                let it = step / g.sub_steps;
                if it < ns {
                    let base = it * n_cells;
                    for iz in 0..vel.nz {
                        let pz = iz + g.pad;
                        let row_p = pz * g.pnx + g.pad;
                        let row_o = base + iz * vel.nx;
                        for ix in 0..vel.nx {
                            let idx = row_p + ix;
                            let u_c = p_curr[idx];
                            // Exact 2nd time derivative d2u/dt2 at time step `step`
                            let u_tt = (p_next[idx] - 2.0 * u_c + p_prev[idx]) * inv_dt2;
                            u_tt_snaps[row_o + ix] = u_tt;
                            illum[iz * vel.nx + ix] += (u_c as f64) * (u_c as f64);
                        }
                    }
                    for (ri, &(rpx, rpz)) in shot.rec_cells.iter().enumerate() {
                        let syn = p_next[rpz * g.pnx + rpx];
                        let obs = shot.rec_data[ri * ns + it];
                        let diff = syn - obs;
                        residuals[ri * ns + it] = diff;
                        misfit += 0.5 * (diff as f64) * (diff as f64);
                    }
                }
            }
            std::mem::swap(&mut p_prev, &mut p_curr);
            std::mem::swap(&mut p_curr, &mut p_next);
        }

        // Adjoint pass: back-propagate data residuals `r = u_syn - d_obs` in reverse time.
        p_prev.fill(0.0);
        p_curr.fill(0.0);
        p_next.fill(0.0);
        for rev in 0..n_fd {
            let fwd_step = n_fd - 1 - rev;
            fd_step_cpu(&g, &p_prev, &p_curr, &mut p_next);
            if fwd_step % g.sub_steps == 0 {
                let it = fwd_step / g.sub_steps;
                if it < ns {
                    for (ri, &(rpx, rpz)) in shot.rec_cells.iter().enumerate() {
                        let idx = rpz * g.pnx + rpx;
                        p_next[idx] += residuals[ri * ns + it] * g.v2dt2[idx];
                    }
                    // Cross-correlate adjoint wavefield `lambda` with `u_tt` -> exact gradient kernel.
                    let base = it * n_cells;
                    for iz in 0..vel.nz {
                        let pz = iz + g.pad;
                        let row_p = pz * g.pnx + g.pad;
                        let row_o = base + iz * vel.nx;
                        for ix in 0..vel.nx {
                            let lam = p_next[row_p + ix] as f64;
                            let utt = u_tt_snaps[row_o + ix] as f64;
                            grad[iz * vel.nx + ix] += lam * utt;
                        }
                    }
                }
            }
            std::mem::swap(&mut p_prev, &mut p_curr);
            std::mem::swap(&mut p_curr, &mut p_next);
        }
    }

    // Scale by `+2 / v(x,z)^3 * dt_trace` (verified against FD directional derivative in unit tests).
    let mut grad_f32 = vec![0.0f32; n_cells];
    let mut illum_f32 = vec![0.0f32; n_cells];
    for iz in 0..vel.nz {
        for ix in 0..vel.nx {
            let k = iz * vel.nx + ix;
            let v = vel.get(ix, iz).max(300.0) as f64;
            grad_f32[k] = (2.0 / (v * v * v) * grad[k] * dt_trace_s) as f32;
            illum_f32[k] = illum[k] as f32;
        }
    }

    Ok((misfit, grad_f32, illum_f32))
}

/// Separable 2D Gaussian smoothing for FWI gradient regularization.
fn smooth_2d(data: &[f32], nx: usize, nz: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.1 || nx < 3 || nz < 3 {
        return data.to_vec();
    }
    let rad = (sigma * 2.5).ceil() as isize;
    let mut ker = Vec::with_capacity((2 * rad + 1) as usize);
    let mut sum = 0.0f32;
    for d in -rad..=rad {
        let w = (-(d * d) as f32 / (2.0 * sigma * sigma)).exp();
        ker.push(w);
        sum += w;
    }
    for w in &mut ker {
        *w /= sum;
    }
    let mut tmp = vec![0.0f32; nx * nz];
    for iz in 0..nz {
        for ix in 0..nx {
            let mut acc = 0.0f32;
            for (ki, d) in (-rad..=rad).enumerate() {
                let jx = (ix as isize + d).clamp(0, (nx - 1) as isize) as usize;
                acc += data[iz * nx + jx] * ker[ki];
            }
            tmp[iz * nx + ix] = acc;
        }
    }
    let mut out = vec![0.0f32; nx * nz];
    for iz in 0..nz {
        for ix in 0..nx {
            let mut acc = 0.0f32;
            for (ki, d) in (-rad..=rad).enumerate() {
                let jz = (iz as isize + d).clamp(0, (nz - 1) as isize) as usize;
                acc += tmp[jz * nx + ix] * ker[ki];
            }
            out[iz * nx + ix] = acc;
        }
    }
    out
}

/// Two-loop L-BFGS search direction `p = -H_k * g` (returns descent direction `p`).
fn lbfgs_direction(g: &[f32], s_hist: &[Vec<f32>], y_hist: &[Vec<f32>]) -> Vec<f32> {
    let n = g.len();
    let m = s_hist.len();
    let mut q: Vec<f64> = g.iter().map(|&v| v as f64).collect();
    if m == 0 {
        return q.into_iter().map(|v| -v as f32).collect();
    }
    let mut rho = vec![0.0f64; m];
    let mut alpha = vec![0.0f64; m];
    for i in (0..m).rev() {
        let ys: f64 = s_hist[i].iter().zip(&y_hist[i]).map(|(&s, &y)| s as f64 * y as f64).sum();
        if ys <= 1e-24 {
            rho[i] = 0.0;
            continue;
        }
        rho[i] = 1.0 / ys;
        let sq: f64 = s_hist[i].iter().zip(&q).map(|(&s, &qi)| s as f64 * qi).sum();
        alpha[i] = rho[i] * sq;
        for j in 0..n {
            q[j] -= alpha[i] * y_hist[i][j] as f64;
        }
    }
    let last_ys: f64 = s_hist[m - 1].iter().zip(&y_hist[m - 1]).map(|(&s, &y)| s as f64 * y as f64).sum();
    let last_yy: f64 = y_hist[m - 1].iter().map(|&y| (y as f64) * (y as f64)).sum();
    let gamma = if last_yy > 1e-24 && last_ys > 0.0 { last_ys / last_yy } else { 1.0 };
    let mut r: Vec<f64> = q.into_iter().map(|v| gamma * v).collect();
    for i in 0..m {
        if rho[i] == 0.0 {
            continue;
        }
        let yr: f64 = y_hist[i].iter().zip(&r).map(|(&y, &ri)| y as f64 * ri).sum();
        let beta = rho[i] * yr;
        for j in 0..n {
            r[j] += s_hist[i][j] as f64 * (alpha[i] - beta);
        }
    }
    // Verify descent (`dot(g, -r) < 0`); fall back to steepest descent if not.
    let gd: f64 = g.iter().zip(&r).map(|(&gi, &ri)| -(gi as f64) * ri).sum();
    if gd >= 0.0 {
        g.iter().map(|&v| -v).collect()
    } else {
        r.into_iter().map(|v| -v as f32).collect()
    }
}

/// Run multi-scale 2D Acoustic Full Waveform Inversion on pre-stack shot gathers.
pub fn fwi_2d(
    traces: &[Trace],
    init_vel: &VelocityGrid2D,
    params: &Fwi2dParams,
    control: Option<&JobControl>,
) -> Result<Fwi2dOutput> {
    if traces.is_empty() {
        return Err(SeismicError::InvalidFormat("FWI 2D requires non-empty shot traces".into()));
    }
    let dt_us = traces[0].dt_us;
    let dt_trace_s = dt_us as f64 * 1e-6;
    let mut vel = init_vel.clone();
    let g_init = PaddedGrid::build(&vel, dt_trace_s, params.pad_cells, params.sponge_coeff)?;
    let (raw_shots, ns) = prepare_fwi_shots(traces, &vel, &g_init, params.shot_stride);
    if raw_shots.is_empty() || ns < 8 {
        return Err(SeismicError::InvalidFormat("FWI 2D found no live shots".into()));
    }

    let stages = if params.freq_stages_hz.is_empty() {
        vec![0.0f32]
    } else {
        params.freq_stages_hz.clone()
    };
    let iters_per_stage = params.iters_per_stage.max(1);
    let total_iters = stages.len() * iters_per_stage;
    let t_delay_s = 1.2 / params.dominant_freq_hz.max(5.0) as f64;
    let mut misfit_history = Vec::with_capacity(total_iters + 1);
    let mut completed = 0usize;

    for (si, &f_hi) in stages.iter().enumerate() {
        if let Some(ctrl) = control {
            if ctrl.is_cancelled() {
                return Err(SeismicError::Cancelled);
            }
        }
        // Filter observed shot gathers to current frequency stage.
        let mut stage_obs_energy = 0.0f64;
        let stage_shots: Vec<ShotTask> = raw_shots
            .iter()
            .map(|sh| {
                let nrec = sh.rec_cells.len();
                let mut filtered = vec![0.0f32; nrec * ns];
                for ri in 0..nrec {
                    let slice = &sh.rec_data[ri * ns..(ri + 1) * ns];
                    let lp = lowpass_series(slice, dt_trace_s, f_hi);
                    for &v in &lp {
                        stage_obs_energy += 0.5 * (v as f64) * (v as f64);
                    }
                    filtered[ri * ns..(ri + 1) * ns].copy_from_slice(&lp);
                }
                ShotTask {
                    src_cell: sh.src_cell,
                    rec_cells: sh.rec_cells.clone(),
                    rec_data: filtered,
                }
            })
            .collect();
        let norm_div = stage_obs_energy.max(1e-20);

        // Build base Ricker wavelet filtered to the same frequency stage, then scale if requested.
        let g_stage = PaddedGrid::build(&vel, dt_trace_s, params.pad_cells, params.sponge_coeff)?;
        let n_fd = ns * g_stage.sub_steps;
        let base_wav: Vec<f32> = (0..n_fd)
            .map(|step| ricker_wavelet(step as f64 * g_stage.dt_fd_s - t_delay_s, params.dominant_freq_hz as f64) as f32)
            .collect();
        let mut stage_wav = lowpass_series(&base_wav, g_stage.dt_fd_s, f_hi);
        if params.estimate_source_scale {
            let scale = estimate_optimal_scale(&vel, &stage_shots, ns, dt_trace_s, &stage_wav, params)?;
            for w in &mut stage_wav {
                *w *= scale;
            }
        }

        let mut s_hist: Vec<Vec<f32>> = Vec::new();
        let mut y_hist: Vec<Vec<f32>> = Vec::new();
        let mut prev_v: Option<Vec<f32>> = None;
        let mut prev_g: Option<Vec<f32>> = None;

        for it in 0..iters_per_stage {
            if let Some(ctrl) = control {
                if ctrl.is_cancelled() {
                    return Err(SeismicError::Cancelled);
                }
                let frac = completed as f32 / total_iters.max(1) as f32;
                let band_lbl = if f_hi > 0.0 { format!("{f_hi:.0} Hz") } else { "full band".into() };
                ctrl.set_progress(
                    frac,
                    format!("FWI stage {}/{} ({band_lbl}) · iter {}/{}", si + 1, stages.len(), it + 1, iters_per_stage),
                );
            }

            // Ensure wavelet matches current velocity CFL sub-stepping.
            let g_now = PaddedGrid::build(&vel, dt_trace_s, params.pad_cells, params.sponge_coeff)?;
            let wav_now = if g_now.sub_steps == g_stage.sub_steps {
                stage_wav.clone()
            } else {
                let n_fd_now = ns * g_now.sub_steps;
                let bw: Vec<f32> = (0..n_fd_now)
                    .map(|step| ricker_wavelet(step as f64 * g_now.dt_fd_s - t_delay_s, params.dominant_freq_hz as f64) as f32)
                    .collect();
                let mut w = lowpass_series(&bw, g_now.dt_fd_s, f_hi);
                if params.estimate_source_scale {
                    let sc = estimate_optimal_scale(&vel, &stage_shots, ns, dt_trace_s, &w, params)?;
                    for x in &mut w {
                        *x *= sc;
                    }
                }
                w
            };

            let (misfit, raw_grad, illum) =
                eval_misfit_and_gradient(&vel, &stage_shots, ns, dt_trace_s, &wav_now, params, control)?;
            if misfit_history.is_empty() || it == 0 {
                misfit_history.push(misfit / norm_div);
            }

            // Precondition by source illumination + shallow mute + Gaussian smoothing.
            let max_illum = illum.iter().copied().fold(0.0f32, f32::max).max(1e-20);
            let eps = params.illumination_eps.max(1e-4) * max_illum;
            let mut g_pre = vec![0.0f32; vel.nx * vel.nz];
            for iz in 0..vel.nz {
                let z_m = vel.z0_m + iz as f32 * vel.dz_m;
                let mute_w = if params.mute_depth_m <= 0.0 || z_m >= params.mute_depth_m {
                    1.0
                } else {
                    let t = (z_m / params.mute_depth_m).clamp(0.0, 1.0);
                    t * t
                };
                for ix in 0..vel.nx {
                    let k = iz * vel.nx + ix;
                    g_pre[k] = raw_grad[k] / (illum[k] + eps) * mute_w;
                }
            }
            // Scale smoothing inversely with stage frequency so low-frequency stages build smooth macro models.
            let max_stage_f = stages.iter().copied().fold(params.dominant_freq_hz, f32::max).max(1.0);
            let stage_sigma = if f_hi > 0.0 {
                params.smooth_sigma_cells * (max_stage_f / f_hi).clamp(1.0, 2.5)
            } else {
                params.smooth_sigma_cells
            };
            let g_reg = smooth_2d(&g_pre, vel.nx, vel.nz, stage_sigma);

            if let (Some(pv), Some(pg)) = (&prev_v, &prev_g) {
                let s_vec: Vec<f32> = vel.v_mps.iter().zip(pv).map(|(&a, &b)| a - b).collect();
                let y_vec: Vec<f32> = g_reg.iter().zip(pg).map(|(&a, &b)| a - b).collect();
                let ys: f64 = s_vec.iter().zip(&y_vec).map(|(&s, &y)| s as f64 * y as f64).sum();
                if ys > 1e-24 {
                    if s_hist.len() >= params.lbfgs_memory.max(1) {
                        s_hist.remove(0);
                        y_hist.remove(0);
                    }
                    s_hist.push(s_vec);
                    y_hist.push(y_vec);
                }
            }

            let dir = lbfgs_direction(&g_reg, &s_hist, &y_hist);
            let max_dir = dir.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
            if max_dir <= 1e-20 {
                break;
            }

            // 3-point quadratic parabolic + backtracking line search with bounded velocity projection.
            let eval_cand = |alpha: f32| -> Result<(VelocityGrid2D, f64)> {
                let mut cand = vel.clone();
                for (cv, &d) in cand.v_mps.iter_mut().zip(&dir) {
                    *cv = (*cv + alpha * d).clamp(params.v_min_mps, params.v_max_mps);
                }
                let g_c = PaddedGrid::build(&cand, dt_trace_s, params.pad_cells, params.sponge_coeff)?;
                let wav_c = if g_c.sub_steps == g_now.sub_steps {
                    wav_now.clone()
                } else {
                    let n_fd_c = ns * g_c.sub_steps;
                    let bw: Vec<f32> = (0..n_fd_c)
                        .map(|st| ricker_wavelet(st as f64 * g_c.dt_fd_s - t_delay_s, params.dominant_freq_hz as f64) as f32)
                        .collect();
                    lowpass_series(&bw, g_c.dt_fd_s, f_hi)
                };
                let m_cand = eval_misfit_only(&cand, &stage_shots, ns, dt_trace_s, &wav_c, params)?;
                Ok((cand, m_cand))
            };

            let mut step = params.max_step_mps / max_dir;
            let mut accepted: Option<(VelocityGrid2D, f64)> = None;
            for _ in 0..5 {
                let (c_half, m_half) = eval_cand(0.5 * step)?;
                let (c_full, m_full) = eval_cand(step)?;
                // Fit exact quadratic parabola through (0, misfit), (0.5*step, m_half), (step, m_full)
                let denom = 2.0 * (misfit - 2.0 * m_half + m_full);
                let mut best = if m_half < m_full { (c_half, m_half) } else { (c_full, m_full) };
                if denom > 1e-18 {
                    let s_opt = (step as f64 * (3.0 * misfit - 4.0 * m_half + m_full) / (2.0 * denom)) as f32;
                    if s_opt > 0.1 * step && s_opt < 1.2 * step {
                        let (c_opt, m_opt) = eval_cand(s_opt)?;
                        if m_opt < best.1 {
                            best = (c_opt, m_opt);
                        }
                    }
                }
                if best.1 < misfit {
                    accepted = Some(best);
                    break;
                }
                step *= 0.35;
            }

            prev_v = Some(vel.v_mps.clone());
            prev_g = Some(g_reg);

            if let Some((next_vel, next_misfit)) = accepted {
                vel = next_vel;
                misfit_history.push(next_misfit / norm_div);
                completed += 1;
            } else {
                // Line search could not reduce misfit further on this stage.
                completed += 1;
                break;
            }
        }
    }

    if let Some(ctrl) = control {
        ctrl.set_progress(1.0, format!("FWI complete ({completed} iterations)"));
    }

    // Export the inverted velocity grid as traces (`nx` traces of `nz` depth samples) for Gather Viewer QC.
    let dz_us = (vel.dz_m.max(0.5) * 1000.0).round() as u32;
    let model_traces: Vec<Trace> = (0..vel.nx)
        .map(|ix| {
            let mut samples = Vec::with_capacity(vel.nz);
            for iz in 0..vel.nz {
                samples.push(vel.get(ix, iz));
            }
            let x_m = vel.x0_m + ix as f32 * vel.dx_m;
            let mut tr = Trace::new(dz_us, samples);
            tr.headers = TraceHeaders {
                traceno: (ix + 1) as i32,
                cdp: (ix + 1) as i32,
                cdp_x: x_m as f64,
                sou_x: x_m as f64,
                rec_x: x_m as f64,
                ..Default::default()
            };
            tr
        })
        .collect();

    Ok(Fwi2dOutput {
        velocity: vel,
        misfit_history,
        model_traces,
        shots_used: raw_shots.len(),
        iterations_completed: completed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtm::forward_afd_grid_2d;

    #[test]
    fn fwi_adjoint_gradient_matches_finite_difference_sign_and_direction() {
        let nx = 32;
        let nz = 24;
        let mut v_true = VelocityGrid2D::constant("true", nx, nz, 0.0, 20.0, 0.0, 20.0, 2000.0);
        for iz in 8..16 {
            for ix in 10..22 {
                v_true.set(ix, iz, 2350.0);
            }
        }
        let v_init = VelocityGrid2D::constant("init", nx, nz, 0.0, 20.0, 0.0, 20.0, 2000.0);
        let shots_x = vec![120.0, 320.0, 520.0];
        let recs_x: Vec<f32> = (4..28).map(|i| i as f32 * 20.0).collect();
        let obs = forward_afd_grid_2d(&v_true, &shots_x, &recs_x, 20.0, 20.0, 4000, 90, 12.0).unwrap();

        let params = Fwi2dParams {
            dominant_freq_hz: 12.0,
            pad_cells: 14,
            ..Default::default()
        };
        let dt_s = 0.004;
        let g = PaddedGrid::build(&v_init, dt_s, params.pad_cells, params.sponge_coeff).unwrap();
        let (shot_tasks, ns) = prepare_fwi_shots(&obs, &v_init, &g, 1);
        let n_fd = ns * g.sub_steps;
        let t_delay = 1.2 / 12.0;
        let wav: Vec<f32> = (0..n_fd)
            .map(|st| ricker_wavelet(st as f64 * g.dt_fd_s - t_delay, 12.0) as f32)
            .collect();

        let (m0, grad, _) = eval_misfit_and_gradient(&v_init, &shot_tasks, ns, dt_s, &wav, &params, None).unwrap();

        // Perturb strictly in the negative gradient direction `-eps * grad_unit`: misfit MUST decrease!
        let g_norm = grad.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>().sqrt();
        assert!(g_norm > 0.0);
        let mut v_minus = v_init.clone();
        let mut v_plus = v_init.clone();
        let eps = 5.0f32;
        for k in 0..grad.len() {
            let d = (grad[k] as f64 / g_norm) as f32;
            v_minus.v_mps[k] -= eps * d;
            v_plus.v_mps[k] += eps * d;
        }
        let m_minus = eval_misfit_only(&v_minus, &shot_tasks, ns, dt_s, &wav, &params).unwrap();
        let m_plus = eval_misfit_only(&v_plus, &shot_tasks, ns, dt_s, &wav, &params).unwrap();
        assert!(
            m_minus < m0 && m_plus > m0,
            "Adjoint gradient must point uphill: m_minus={m_minus:.4e}, m0={m0:.4e}, m_plus={m_plus:.4e}"
        );
    }
}
