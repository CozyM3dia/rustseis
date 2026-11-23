//! 2D Acoustic Reverse Time Migration (RTM) & Gridded `V(x,z)` Finite-Difference Forward Modeler.
//!
//! Features:
//! - 8th-order-space / 2nd-order-time acoustic finite-difference stencil on arbitrary `VelocityGrid2D`
//! - Cerjan exponential absorbing sponge boundaries on all 4 edges
//! - Automatic CFL sub-stepping aligned to trace sample rate
//! - Source-illumination compensated zero-lag cross-correlation imaging condition
//! - 2D Laplacian low-wavenumber artifact filter
//! - Optional `wgpu` GPU compute acceleration (automatic fallback to multi-core `rayon` CPU)
//! - Cooperative progress reporting and cancellation via `JobControl`

use crate::modeling::ricker_wavelet;
use rayon::prelude::*;
use rustseis_core::{
    Ensemble, JobControl, Result, SeismicError, Trace, TraceHeaders, VelocityGrid2D,
};
use serde::{Deserialize, Serialize};

/// 8th-order central finite-difference second-derivative stencil coefficients.
pub(crate) const FD8_C0: f32 = -205.0 / 72.0;
pub(crate) const FD8_C1: f32 = 8.0 / 5.0;
pub(crate) const FD8_C2: f32 = -1.0 / 5.0;
pub(crate) const FD8_C3: f32 = 8.0 / 315.0;
pub(crate) const FD8_C4: f32 = -1.0 / 560.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rtm2dParams {
    /// Dominant frequency of the Ricker source wavelet (Hz).
    #[serde(default = "default_fdom")]
    pub dominant_freq_hz: f32,
    /// Absorbing sponge boundary width in grid cells per side.
    #[serde(default = "default_pad")]
    pub pad_cells: usize,
    /// Cerjan sponge damping factor (typical `0.012..0.020`).
    #[serde(default = "default_sponge")]
    pub sponge_coeff: f32,
    /// Snapshot decimation factor in trace samples (`1` = every trace sample, `2` = every 2nd sample).
    #[serde(default = "default_snap_stride")]
    pub snapshot_sample_stride: usize,
    /// Illumination stabilization fraction (`eps * max_illum`).
    #[serde(default = "default_illum_eps")]
    pub illumination_eps: f32,
    /// Apply 2D Laplacian `-nabla^2` filter after stacking to remove low-wavenumber RTM haze.
    #[serde(default = "default_true")]
    pub apply_laplacian_filter: bool,
    /// Attempt GPU execution via `wgpu` compute shaders (falls back to CPU cleanly if unavailable).
    #[serde(default = "default_true")]
    pub use_gpu: bool,
    /// Optional shot sub-sampling stride (`1` = migrate all shots).
    #[serde(default = "default_one")]
    pub shot_stride: usize,
}

fn default_fdom() -> f32 { 25.0 }
fn default_pad() -> usize { 24 }
fn default_sponge() -> f32 { 0.015 }
fn default_snap_stride() -> usize { 1 }
fn default_illum_eps() -> f32 { 0.02 }
fn default_true() -> bool { true }
fn default_one() -> usize { 1 }

impl Default for Rtm2dParams {
    fn default() -> Self {
        Self {
            dominant_freq_hz: default_fdom(),
            pad_cells: default_pad(),
            sponge_coeff: default_sponge(),
            snapshot_sample_stride: default_snap_stride(),
            illumination_eps: default_illum_eps(),
            apply_laplacian_filter: true,
            use_gpu: true,
            shot_stride: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RtmOutput {
    /// Migrated depth image (`nx` traces, each of length `nz`).
    pub image_traces: Vec<Trace>,
    /// Backend used (`"GPU: <adapter>"` or `"CPU (rayon)"`).
    pub backend: String,
    pub shots_migrated: usize,
    pub dt_fd_s: f64,
    pub sub_steps: usize,
}

/// Padded grid arrays shared by both CPU and GPU 2D acoustic propagators.
pub(crate) struct PaddedGrid {
    pub nx: usize,
    pub nz: usize,
    pub pad: usize,
    pub pnx: usize,
    pub pnz: usize,
    pub inv_dx2: f32,
    pub inv_dz2: f32,
    pub v2dt2: Vec<f32>,
    pub sponge: Vec<f32>,
    pub dt_fd_s: f64,
    pub sub_steps: usize,
}

impl PaddedGrid {
    pub(crate) fn build(vel: &VelocityGrid2D, dt_trace_s: f64, pad: usize, sponge_coeff: f32) -> Result<Self> {
        if vel.nx < 16 || vel.nz < 16 {
            return Err(SeismicError::InvalidFormat("RTM VelocityGrid2D must be at least 16x16".into()));
        }
        let dx = vel.dx_m.max(1e-3) as f64;
        let dz = vel.dz_m.max(1e-3) as f64;
        let (_, vmax) = vel.v_min_max();
        // CFL limit for 2D 8th-order space / 2nd-order time acoustic stencil (max ~0.58 * h / (vmax * sqrt(2)))
        let h_min = dx.min(dz);
        let dt_cfl = 0.42 * h_min / (vmax.max(500.0) as f64 * std::f64::consts::SQRT_2);
        let sub_steps = ((dt_trace_s / dt_cfl).ceil() as usize).max(1);
        let dt_fd_s = dt_trace_s / sub_steps as f64;

        let pnx = vel.nx + 2 * pad;
        let pnz = vel.nz + 2 * pad;
        let mut v2dt2 = vec![0.0f32; pnx * pnz];
        let mut sponge = vec![1.0f32; pnx * pnz];

        for pz in 0..pnz {
            let iz = pz.saturating_sub(pad).min(vel.nz - 1);
            let dz_pad = if pz < pad {
                (pad - pz) as f32
            } else if pz >= pad + vel.nz {
                (pz - (pad + vel.nz) + 1) as f32
            } else {
                0.0
            };
            for px in 0..pnx {
                let ix = px.saturating_sub(pad).min(vel.nx - 1);
                let dx_pad = if px < pad {
                    (pad - px) as f32
                } else if px >= pad + vel.nx {
                    (px - (pad + vel.nx) + 1) as f32
                } else {
                    0.0
                };
                let v = vel.get(ix, iz).max(300.0) as f64;
                v2dt2[pz * pnx + px] = (v * v * dt_fd_s * dt_fd_s) as f32;
                let d = dx_pad.max(dz_pad);
                if d > 0.0 {
                    let s = (-sponge_coeff * sponge_coeff * d * d).exp();
                    sponge[pz * pnx + px] = s;
                }
            }
        }

        Ok(Self {
            nx: vel.nx,
            nz: vel.nz,
            pad,
            pnx,
            pnz,
            inv_dx2: (1.0 / (dx * dx)) as f32,
            inv_dz2: (1.0 / (dz * dz)) as f32,
            v2dt2,
            sponge,
            dt_fd_s,
            sub_steps,
        })
    }

    #[inline]
    pub(crate) fn grid_pos(&self, vel: &VelocityGrid2D, x_m: f32, z_m: f32) -> (usize, usize) {
        let ix = (((x_m - vel.x0_m) / vel.dx_m.max(1e-3)).round() as isize)
            .clamp(0, (self.nx - 1) as isize) as usize;
        let iz = (((z_m - vel.z0_m) / vel.dz_m.max(1e-3)).round() as isize)
            .clamp(0, (self.nz - 1) as isize) as usize;
        (ix + self.pad, iz + self.pad)
    }
}

/// One 8th-order-space / 2nd-order-time CPU FD update step with Cerjan sponge boundaries.
pub(crate) fn fd_step_cpu(g: &PaddedGrid, p_prev: &[f32], p_curr: &[f32], p_next: &mut [f32]) {
    let pnx = g.pnx;
    let pnz = g.pnz;
    let inv_dx2 = g.inv_dx2;
    let inv_dz2 = g.inv_dz2;
    let v2dt2 = &g.v2dt2;
    let sponge = &g.sponge;

    p_next
        .par_chunks_mut(pnx)
        .enumerate()
        .for_each(|(pz, row_out)| {
            if pz < 4 || pz + 4 >= pnz {
                row_out.fill(0.0);
                return;
            }
            let r0 = pz * pnx;
            let r_m1 = (pz - 1) * pnx;
            let r_p1 = (pz + 1) * pnx;
            let r_m2 = (pz - 2) * pnx;
            let r_p2 = (pz + 2) * pnx;
            let r_m3 = (pz - 3) * pnx;
            let r_p3 = (pz + 3) * pnx;
            let r_m4 = (pz - 4) * pnx;
            let r_p4 = (pz + 4) * pnx;

            for px in 4..(pnx - 4) {
                let c = p_curr[r0 + px];
                let d2x = FD8_C0 * c
                    + FD8_C1 * (p_curr[r0 + px - 1] + p_curr[r0 + px + 1])
                    + FD8_C2 * (p_curr[r0 + px - 2] + p_curr[r0 + px + 2])
                    + FD8_C3 * (p_curr[r0 + px - 3] + p_curr[r0 + px + 3])
                    + FD8_C4 * (p_curr[r0 + px - 4] + p_curr[r0 + px + 4]);
                let d2z = FD8_C0 * c
                    + FD8_C1 * (p_curr[r_m1 + px] + p_curr[r_p1 + px])
                    + FD8_C2 * (p_curr[r_m2 + px] + p_curr[r_p2 + px])
                    + FD8_C3 * (p_curr[r_m3 + px] + p_curr[r_p3 + px])
                    + FD8_C4 * (p_curr[r_m4 + px] + p_curr[r_p4 + px]);
                let lap = d2x * inv_dx2 + d2z * inv_dz2;
                let idx = r0 + px;
                let s = sponge[idx];
                row_out[px] = (2.0 * c - p_prev[idx] + v2dt2[idx] * lap) * s;
            }
            for px in 0..4 {
                row_out[px] = 0.0;
                row_out[pnx - 1 - px] = 0.0;
            }
        });
}

/// Generate synthetic shot gathers on an arbitrary gridded `VelocityGrid2D` using the exact same
/// 8th-order acoustic FD wave propagator (useful for RTM/PSDM ground-truth validation).
pub fn forward_afd_grid_2d(
    vel: &VelocityGrid2D,
    shot_xs_m: &[f32],
    rec_xs_m: &[f32],
    src_z_m: f32,
    rec_z_m: f32,
    dt_us: u32,
    n_samples: usize,
    dominant_freq_hz: f32,
) -> Result<Vec<Trace>> {
    let dt_trace_s = dt_us as f64 * 1e-6;
    let g = PaddedGrid::build(vel, dt_trace_s, 24, 0.015)?;
    let n_fd = n_samples * g.sub_steps;
    let t_delay_s = 1.2 / dominant_freq_hz.max(5.0) as f64;
    let rec_cells: Vec<(usize, usize)> = rec_xs_m
        .iter()
        .map(|&rx| g.grid_pos(vel, rx, rec_z_m))
        .collect();

    let mut all_traces = Vec::with_capacity(shot_xs_m.len() * rec_xs_m.len());
    let mut traceno = 1i32;

    for (si, &sx) in shot_xs_m.iter().enumerate() {
        let (spx, spz) = g.grid_pos(vel, sx, src_z_m);
        let s_idx = spz * g.pnx + spx;
        let mut p_prev = vec![0.0f32; g.pnx * g.pnz];
        let mut p_curr = vec![0.0f32; g.pnx * g.pnz];
        let mut p_next = vec![0.0f32; g.pnx * g.pnz];
        let mut rec_buf = vec![vec![0.0f32; n_samples]; rec_xs_m.len()];

        for step in 0..n_fd {
            let t_s = step as f64 * g.dt_fd_s;
            fd_step_cpu(&g, &p_prev, &p_curr, &mut p_next);
            let w = ricker_wavelet(t_s - t_delay_s, dominant_freq_hz as f64) as f32;
            p_next[s_idx] += w * g.v2dt2[s_idx];

            if step % g.sub_steps == 0 {
                let it = step / g.sub_steps;
                if it < n_samples {
                    for (ri, &(rpx, rpz)) in rec_cells.iter().enumerate() {
                        rec_buf[ri][it] = p_next[rpz * g.pnx + rpx];
                    }
                }
            }
            std::mem::swap(&mut p_prev, &mut p_curr);
            std::mem::swap(&mut p_curr, &mut p_next);
        }

        let ffid = 1001 + si as i32;
        for (ri, samples) in rec_buf.into_iter().enumerate() {
            let rx = rec_xs_m[ri];
            let mut tr = Trace::new(dt_us, samples);
            tr.headers = TraceHeaders {
                traceno,
                ffid,
                chan: (ri + 1) as i32,
                cdp: (((0.5 * (sx + rx) - vel.x0_m) / vel.dx_m.max(1e-3)).round() as i32 + 1).max(1),
                offset: rx - sx,
                sou_x: sx as f64,
                rec_x: rx as f64,
                cdp_x: 0.5 * (sx + rx) as f64,
                ..Default::default()
            };
            traceno += 1;
            all_traces.push(tr);
        }
    }

    Ok(all_traces)
}

/// Apply 2D Laplacian filter `-nabla^2 I(x,z)` to suppress low-wavenumber RTM diving-wave haze.
pub fn apply_laplacian_filter(img: &[f32], nx: usize, nz: usize, dx: f32, dz: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; nx * nz];
    if nx < 3 || nz < 3 {
        return img.to_vec();
    }
    let idx2 = 1.0 / (dx * dx).max(1e-6);
    let idz2 = 1.0 / (dz * dz).max(1e-6);
    for iz in 1..(nz - 1) {
        for ix in 1..(nx - 1) {
            let c = img[iz * nx + ix];
            let d2x = (img[iz * nx + ix - 1] - 2.0 * c + img[iz * nx + ix + 1]) * idx2;
            let d2z = (img[(iz - 1) * nx + ix] - 2.0 * c + img[(iz + 1) * nx + ix]) * idz2;
            out[iz * nx + ix] = -(d2x + d2z);
        }
    }
    out
}

pub(crate) struct ShotTask {
    pub src_cell: (usize, usize),
    pub rec_cells: Vec<(usize, usize)>,
    /// Row-major `[ri * n_samples + it]` receiver data.
    pub rec_data: Vec<f32>,
}

fn prepare_shots(
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

fn rtm_cpu(
    g: &PaddedGrid,
    shots: &[ShotTask],
    ns: usize,
    params: &Rtm2dParams,
    job: Option<&JobControl>,
) -> Result<Vec<f32>> {
    let n_fd = ns * g.sub_steps;
    let snap_every_fd = g.sub_steps * params.snapshot_sample_stride.max(1);
    let n_snaps = (n_fd + snap_every_fd - 1) / snap_every_fd;
    let t_delay_s = 1.2 / params.dominant_freq_hz.max(5.0) as f64;
    let nx = g.nx;
    let nz = g.nz;
    let pad = g.pad;
    let pnx = g.pnx;

    let mut stack_img = vec![0.0f64; nx * nz];
    let mut snaps = vec![0.0f32; n_snaps * nx * nz];
    let mut p_prev = vec![0.0f32; g.pnx * g.pnz];
    let mut p_curr = vec![0.0f32; g.pnx * g.pnz];
    let mut p_next = vec![0.0f32; g.pnx * g.pnz];
    let mut shot_img = vec![0.0f64; nx * nz];
    let mut shot_illum = vec![0.0f64; nx * nz];

    for (si, shot) in shots.iter().enumerate() {
        if let Some(j) = job {
            j.check_cancelled()?;
            j.set_progress(
                si as f32 / shots.len().max(1) as f32,
                format!("RTM shot {}/{} (CPU)", si + 1, shots.len()),
            );
        }

        // 1. Forward source wavefield propagation
        p_prev.fill(0.0);
        p_curr.fill(0.0);
        p_next.fill(0.0);
        let s_idx = shot.src_cell.1 * pnx + shot.src_cell.0;
        for step in 0..n_fd {
            let t_s = step as f64 * g.dt_fd_s;
            fd_step_cpu(g, &p_prev, &p_curr, &mut p_next);
            let w = ricker_wavelet(t_s - t_delay_s, params.dominant_freq_hz as f64) as f32;
            p_next[s_idx] += w * g.v2dt2[s_idx];

            if step % snap_every_fd == 0 {
                let k = step / snap_every_fd;
                if k < n_snaps {
                    let base = k * nx * nz;
                    for iz in 0..nz {
                        let src_row = (iz + pad) * pnx + pad;
                        let dst_row = base + iz * nx;
                        snaps[dst_row..dst_row + nx].copy_from_slice(&p_next[src_row..src_row + nx]);
                    }
                }
            }
            std::mem::swap(&mut p_prev, &mut p_curr);
            std::mem::swap(&mut p_curr, &mut p_next);
        }

        // 2. Backward receiver wavefield propagation + zero-lag cross-correlation
        p_prev.fill(0.0);
        p_curr.fill(0.0);
        p_next.fill(0.0);
        shot_img.fill(0.0);
        shot_illum.fill(0.0);

        for rev in 0..n_fd {
            let step = n_fd - 1 - rev;
            fd_step_cpu(g, &p_prev, &p_curr, &mut p_next);
            // Linearly interpolate receiver trace at FD time step
            let f_it = step as f64 / g.sub_steps as f64;
            let it0 = (f_it.floor() as usize).min(ns.saturating_sub(1));
            let it1 = (it0 + 1).min(ns.saturating_sub(1));
            let frac = (f_it - it0 as f64) as f32;
            for (ri, &(rpx, rpz)) in shot.rec_cells.iter().enumerate() {
                let r_idx = rpz * pnx + rpx;
                let a0 = shot.rec_data[ri * ns + it0];
                let a1 = shot.rec_data[ri * ns + it1];
                let val = a0 + frac * (a1 - a0);
                p_next[r_idx] += val * g.v2dt2[r_idx];
            }

            if step % snap_every_fd == 0 {
                let k = step / snap_every_fd;
                if k < n_snaps {
                    let base = k * nx * nz;
                    for iz in 0..nz {
                        let r_row = (iz + pad) * pnx + pad;
                        let i_row = iz * nx;
                        for ix in 0..nx {
                            let s = snaps[base + i_row + ix] as f64;
                            let r = p_next[r_row + ix] as f64;
                            shot_img[i_row + ix] += s * r;
                            shot_illum[i_row + ix] += s * s;
                        }
                    }
                }
            }
            std::mem::swap(&mut p_prev, &mut p_curr);
            std::mem::swap(&mut p_curr, &mut p_next);
        }

        // 3. Source-illumination normalized stack
        let max_il = shot_illum.iter().copied().fold(0.0f64, f64::max).max(1e-30);
        let eps = params.illumination_eps.max(1e-6) as f64 * max_il;
        for i in 0..(nx * nz) {
            stack_img[i] += shot_img[i] / (shot_illum[i] + eps);
        }
    }

    Ok(stack_img.into_iter().map(|v| v as f32).collect())
}

/// Run 2D Acoustic Reverse Time Migration (RTM) on pre-stack shot gathers with a gridded `VelocityGrid2D`.
/// Automatically runs on GPU (`wgpu`) when `params.use_gpu` is true and a GPU adapter is available,
/// falling back seamlessly to multi-core CPU otherwise.
pub fn rtm_2d(
    traces: &[Trace],
    vel: &VelocityGrid2D,
    params: &Rtm2dParams,
    job: Option<&JobControl>,
) -> Result<RtmOutput> {
    if traces.is_empty() {
        return Err(SeismicError::InvalidFormat("2D RTM requires non-empty shot gathers".into()));
    }
    let dt_trace_s = traces[0].dt_us as f64 * 1e-6;
    let g = PaddedGrid::build(vel, dt_trace_s, params.pad_cells.max(8), params.sponge_coeff)?;
    let (shots, ns) = prepare_shots(traces, vel, &g, params.shot_stride);
    if shots.is_empty() || ns < 4 {
        return Err(SeismicError::InvalidFormat("No valid live shot gathers for 2D RTM".into()));
    }

    let (raw_img, backend): (Vec<f32>, String) = {
        #[cfg(feature = "gpu")]
        {
            if params.use_gpu {
                match crate::rtm_gpu::rtm_gpu(&g, &shots, ns, params, job) {
                    Ok((img, name)) => (img, format!("GPU ({name})")),
                    Err(SeismicError::Cancelled) => return Err(SeismicError::Cancelled),
                    Err(_gpu_err) => (rtm_cpu(&g, &shots, ns, params, job)?, "CPU (rayon)".into()),
                }
            } else {
                (rtm_cpu(&g, &shots, ns, params, job)?, "CPU (rayon)".into())
            }
        }
        #[cfg(not(feature = "gpu"))]
        {
            (rtm_cpu(&g, &shots, ns, params, job)?, "CPU (rayon)".into())
        }
    };

    let final_img = if params.apply_laplacian_filter {
        apply_laplacian_filter(&raw_img, vel.nx, vel.nz, vel.dx_m, vel.dz_m)
    } else {
        raw_img
    };

    let dz = vel.dz_m.max(1e-3);
    let depth_dt_us = (dz * 1000.0).round().max(100.0) as u32;
    let mut image_traces = Vec::with_capacity(vel.nx);
    for ix in 0..vel.nx {
        let mut col = Vec::with_capacity(vel.nz);
        for iz in 0..vel.nz {
            col.push(final_img[iz * vel.nx + ix]);
        }
        let x_m = vel.x_at(ix) as f64;
        let mut tr = Trace::new(depth_dt_us, col);
        tr.headers = TraceHeaders {
            traceno: (ix + 1) as i32,
            ffid: 1,
            chan: (ix + 1) as i32,
            cdp: (ix + 1) as i32,
            cdp_x: x_m,
            sou_x: x_m,
            rec_x: x_m,
            ..Default::default()
        };
        tr.headers.custom.insert("DEPTH_DZ_M".into(), dz as f64);
        tr.headers.custom.insert("DEPTH_Z0_M".into(), vel.z0_m as f64);
        image_traces.push(tr);
    }

    if let Some(j) = job {
        j.set_progress(1.0, format!("RTM complete · {} shots ({backend})", shots.len()));
    }

    Ok(RtmOutput {
        image_traces,
        backend,
        shots_migrated: shots.len(),
        dt_fd_s: g.dt_fd_s,
        sub_steps: g.sub_steps,
    })
}
