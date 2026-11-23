//! 2D Fast Sweeping Eikonal Traveltime Solver + Pre-Stack Kirchhoff Depth Migration (PSDM).
//!
//! Uses Zhao (2005) 4-direction Gauss-Seidel Fast Sweeping Method (FSM) on `VelocityGrid2D` to
//! compute exact first-arrival traveltime fields `T(x, z)` from every active surface station
//! column in parallel, followed by obliquity-weighted, aperture-tapered Kirchhoff depth summation
//! producing both a depth-migrated section and optional offset-domain Depth Common Image Gathers.

use rayon::prelude::*;
use rustseis_core::{JobControl, Result, SeismicError, Trace, TraceHeaders, VelocityGrid2D};
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::f64::consts::PI;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Solve the 2D Eikonal equation `|grad T(x,z)| = 1 / V(x,z)` from point source `(src_x_m, src_z_m)`
/// using Zhao's Fast Sweeping Method (FSM). Returns traveltimes in **seconds** in row-major `[iz * nx + ix]`.
pub fn solve_eikonal_2d(grid: &VelocityGrid2D, src_x_m: f32, src_z_m: f32) -> Vec<f32> {
    let nx = grid.nx;
    let nz = grid.nz;
    let dx = grid.dx_m.max(1e-3) as f64;
    let dz = grid.dz_m.max(1e-3) as f64;
    let n = nx * nz;
    if n == 0 {
        return Vec::new();
    }

    let slow: Vec<f64> = grid.v_mps.iter().map(|&v| 1.0 / v.max(300.0) as f64).collect();
    let mut t = vec![1e9f64; n];
    let mut frozen = vec![false; n];

    // Exact analytical point-source neighbourhood initialization (5x5 disk eliminates
    // 1st-order Cartesian point-source singularity error) + 2nd-order upwind Godunov FSM.
    let fx = ((src_x_m - grid.x0_m) / grid.dx_m.max(1e-3)).clamp(0.0, (nx - 1) as f32);
    let fz = ((src_z_m - grid.z0_m) / grid.dz_m.max(1e-3)).clamp(0.0, (nz - 1) as f32);
    let sx0 = fx.round() as isize;
    let sz0 = fz.round() as isize;
    let s_src = 1.0 / grid.sample(src_x_m, src_z_m).max(300.0) as f64;

    for dz_i in -3..=3 {
        for dx_i in -3..=3 {
            if dx_i * dx_i + dz_i * dz_i > 10 {
                continue;
            }
            let ix = sx0 + dx_i;
            let iz = sz0 + dz_i;
            if ix >= 0 && (ix as usize) < nx && iz >= 0 && (iz as usize) < nz {
                let idx = iz as usize * nx + ix as usize;
                let gx = grid.x_at(ix as usize) as f64;
                let gz = grid.z_at(iz as usize) as f64;
                let dist = ((gx - src_x_m as f64).powi(2) + (gz - src_z_m as f64).powi(2)).sqrt();
                let s_mid = 1.0
                    / grid
                        .sample(0.5 * (gx as f32 + src_x_m), 0.5 * (gz as f32 + src_z_m))
                        .max(300.0) as f64;
                // Simpson 3-point slowness integral along straight local ray
                let s_avg = (s_src + 4.0 * s_mid + slow[idx]) / 6.0;
                t[idx] = dist * s_avg;
                if dx_i * dx_i + dz_i * dz_i <= 4 {
                    frozen[idx] = true;
                }
            }
        }
    }

    // 2nd-order upwind helper along a 1D axis: returns (effective_t, effective_h)
    let upwind_axis = |t: &[f64], t_l1: f64, t_l2: Option<f64>, t_r1: f64, t_r2: Option<f64>, h: f64| -> (f64, f64) {
        let mut best_t = 1e9f64;
        let mut best_h = h;
        if t_l1 < best_t {
            best_t = t_l1;
            best_h = h;
            if let Some(tl2) = t_l2 {
                if tl2 < t_l1 && t_l1 < 1e8 {
                    let t_eff = (4.0 * t_l1 - tl2) / 3.0;
                    let h_eff = (2.0 / 3.0) * h;
                    if t_eff < t_l1 + 1e-12 {
                        best_t = t_eff;
                        best_h = h_eff;
                    }
                }
            }
        }
        if t_r1 < t_l1 {
            best_t = t_r1;
            best_h = h;
            if let Some(tr2) = t_r2 {
                if tr2 < t_r1 && t_r1 < 1e8 {
                    let t_eff = (4.0 * t_r1 - tr2) / 3.0;
                    let h_eff = (2.0 / 3.0) * h;
                    if t_eff < t_r1 + 1e-12 {
                        best_t = t_eff;
                        best_h = h_eff;
                    }
                }
            }
        }
        let _ = t;
        (best_t, best_h)
    };

    // Upwind Godunov 2D Eikonal quadratic update at cell (ix, iz)
    let update = |t: &mut [f64], ix: usize, iz: usize| -> f64 {
        let idx = iz * nx + ix;
        if frozen[idx] {
            return 0.0;
        }
        let t_l1 = if ix > 0 { t[idx - 1] } else { 1e9 };
        let t_l2 = if ix > 1 { Some(t[idx - 2]) } else { None };
        let t_r1 = if ix + 1 < nx { t[idx + 1] } else { 1e9 };
        let t_r2 = if ix + 2 < nx { Some(t[idx + 2]) } else { None };
        let (tx, hx) = upwind_axis(t, t_l1, t_l2, t_r1, t_r2, dx);

        let t_u1 = if iz > 0 { t[idx - nx] } else { 1e9 };
        let t_u2 = if iz > 1 { Some(t[idx - 2 * nx]) } else { None };
        let t_d1 = if iz + 1 < nz { t[idx + nx] } else { 1e9 };
        let t_d2 = if iz + 2 < nz { Some(t[idx + 2 * nx]) } else { None };
        let (tz, hz) = upwind_axis(t, t_u1, t_u2, t_d1, t_d2, dz);

        let s = slow[idx];
        let cand = if tx >= 1e8 && tz >= 1e8 {
            return 0.0;
        } else if (tx - tz).abs() >= s * hx.max(hz) {
            (tx + s * hx).min(tz + s * hz)
        } else {
            let inv_dx2 = 1.0 / (hx * hx);
            let inv_dz2 = 1.0 / (hz * hz);
            let a = inv_dx2 + inv_dz2;
            let b = -2.0 * (tx * inv_dx2 + tz * inv_dz2);
            let c = tx * tx * inv_dx2 + tz * tz * inv_dz2 - s * s;
            let disc = b * b - 4.0 * a * c;
            if disc >= 0.0 {
                let root = (-b + disc.sqrt()) / (2.0 * a);
                if root >= tx.max(tz) {
                    root
                } else {
                    (tx + s * hx).min(tz + s * hz)
                }
            } else {
                (tx + s * hx).min(tz + s * hz)
            }
        };
        let old = t[idx];
        if cand < old {
            t[idx] = cand;
            old - cand
        } else {
            0.0
        }
    };

    // 4 Gauss-Seidel sweeping orders cover all ray directions
    for _iter in 0..16 {
        let mut max_diff = 0.0f64;
        for iz in 0..nz {
            for ix in 0..nx {
                max_diff = max_diff.max(update(&mut t, ix, iz));
            }
        }
        for iz in 0..nz {
            for ix in (0..nx).rev() {
                max_diff = max_diff.max(update(&mut t, ix, iz));
            }
        }
        for iz in (0..nz).rev() {
            for ix in (0..nx).rev() {
                max_diff = max_diff.max(update(&mut t, ix, iz));
            }
        }
        for iz in (0..nz).rev() {
            for ix in 0..nx {
                max_diff = max_diff.max(update(&mut t, ix, iz));
            }
        }
        if max_diff < 1e-6 {
            break;
        }
    }

    t.into_iter().map(|v| v as f32).collect()
}

/// Apply 2D Kirchhoff half-derivative filter `sqrt(-i omega)` to preserve zero-phase wavelet shape.
fn half_derivative_2d(samples: &[f32], dt_s: f64) -> Vec<f32> {
    let n = samples.len();
    if n < 4 {
        return samples.to_vec();
    }
    let nfft = n.next_power_of_two();
    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(nfft);
    let inv = planner.plan_fft_inverse(nfft);
    let mut buf: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); nfft];
    for (i, &s) in samples.iter().enumerate() {
        buf[i] = Complex::new(s as f64, 0.0);
    }
    fwd.process(&mut buf);
    let df = 1.0 / (nfft as f64 * dt_s);
    let half = nfft / 2;
    let phase_pos = Complex::from_polar(1.0, -PI / 4.0);
    let phase_neg = Complex::from_polar(1.0, PI / 4.0);
    buf[0] = Complex::new(0.0, 0.0);
    for k in 1..half {
        let w = (2.0 * PI * k as f64 * df).sqrt();
        buf[k] *= phase_pos * w;
        buf[nfft - k] *= phase_neg * w;
    }
    buf[half] = Complex::new(0.0, 0.0);
    inv.process(&mut buf);
    let scale = 1.0 / nfft as f64;
    (0..n).map(|i| (buf[i].re * scale) as f32).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KirchhoffPsdm2dParams {
    /// Lateral half-aperture from trace midpoint in metres.
    #[serde(default = "default_aperture")]
    pub aperture_m: f32,
    /// Maximum reflector dip angle in degrees (cosine obliquity cutoff).
    #[serde(default = "default_max_dip")]
    pub max_dip_deg: f32,
    /// Surface Eikonal station decimation stride in grid columns (`1` = exact per column).
    #[serde(default = "default_tt_stride")]
    pub traveltime_stride: usize,
    /// Optional number of offset bins for output Depth Common Image Gathers (`0` = stacked depth image only).
    #[serde(default)]
    pub n_cig_offset_bins: usize,
    /// Maximum absolute offset (m) for CIG binning.
    #[serde(default = "default_max_offset")]
    pub max_cig_offset_m: f32,
    /// Apply 2D half-derivative phase filter before Kirchhoff summation.
    #[serde(default = "default_true")]
    pub apply_half_deriv: bool,
}

fn default_aperture() -> f32 { 1500.0 }
fn default_max_dip() -> f32 { 65.0 }
fn default_tt_stride() -> usize { 1 }
fn default_max_offset() -> f32 { 3000.0 }
fn default_true() -> bool { true }

impl Default for KirchhoffPsdm2dParams {
    fn default() -> Self {
        Self {
            aperture_m: default_aperture(),
            max_dip_deg: default_max_dip(),
            traveltime_stride: default_tt_stride(),
            n_cig_offset_bins: 0,
            max_cig_offset_m: default_max_offset(),
            apply_half_deriv: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PsdmOutput {
    /// Depth-migrated stacked image (`nx` traces, each of length `nz`).
    pub image_traces: Vec<Trace>,
    /// Optional offset-domain Depth Common Image Gathers (`nx * n_cig_offset_bins` traces).
    pub cig_traces: Vec<Trace>,
    pub n_eikonal_tables: usize,
}

/// 2D Pre-Stack Kirchhoff Depth Migration (PSDM) using Fast Sweeping Eikonal traveltime tables.
pub fn kirchhoff_psdm_2d(
    traces: &[Trace],
    vel: &VelocityGrid2D,
    params: &KirchhoffPsdm2dParams,
    job: Option<&JobControl>,
) -> Result<PsdmOutput> {
    if traces.is_empty() || vel.nx == 0 || vel.nz == 0 {
        return Err(SeismicError::InvalidFormat("Kirchhoff PSDM requires traces and a non-empty VelocityGrid2D".into()));
    }
    let nx = vel.nx;
    let nz = vel.nz;
    let dx = vel.dx_m.max(1e-3);
    let dz = vel.dz_m.max(1e-3);
    let dt_s = traces[0].dt_us as f64 * 1e-6;
    let stride = params.traveltime_stride.max(1);

    // 1. Identify which surface grid columns are needed by any source or receiver
    let x_to_col = |x: f32| -> usize {
        let raw = (((x - vel.x0_m) / dx).round() as isize).clamp(0, (nx - 1) as isize) as usize;
        ((raw / stride) * stride).min(nx - 1)
    };
    let mut needed_cols = BTreeSet::new();
    for tr in traces {
        if !tr.is_live() {
            continue;
        }
        let sx = tr.headers.sou_x as f32;
        let rx = tr.headers.rec_x as f32;
        needed_cols.insert(x_to_col(sx));
        needed_cols.insert(x_to_col(rx));
    }
    let cols: Vec<usize> = needed_cols.into_iter().collect();
    if cols.is_empty() {
        return Err(SeismicError::InvalidFormat("No live traces for PSDM".into()));
    }

    // 2. Solve Eikonal traveltime tables for all active surface columns in parallel
    if let Some(j) = job {
        j.set_progress(0.05, format!("PSDM: computing {} Eikonal tables…", cols.len()));
    }
    let done_tt = AtomicUsize::new(0);
    let tt_vec: Vec<(usize, Vec<f32>)> = cols
        .par_iter()
        .map(|&col| -> Result<(usize, Vec<f32>)> {
            if let Some(j) = job {
                j.check_cancelled()?;
            }
            let x_src = vel.x_at(col);
            let table = solve_eikonal_2d(vel, x_src, vel.z0_m);
            let k = done_tt.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(j) = job {
                if k % 8 == 0 || k == cols.len() {
                    j.set_progress(
                        0.05 + 0.35 * (k as f32 / cols.len() as f32),
                        format!("PSDM: Eikonal {k}/{}", cols.len()),
                    );
                }
            }
            Ok((col, table))
        })
        .collect::<Result<Vec<_>>>()?;

    let mut tt_map = std::collections::HashMap::with_capacity(tt_vec.len());
    for (c, tbl) in tt_vec {
        tt_map.insert(c, tbl);
    }

    // 3. Preprocess traces (half-derivative phase filter)
    let prep: Vec<(usize, usize, f32, f32, Vec<f32>)> = traces
        .par_iter()
        .filter(|tr| tr.is_live() && !tr.samples.is_empty())
        .map(|tr| {
            let sx = tr.headers.sou_x as f32;
            let rx = tr.headers.rec_x as f32;
            let mid_x = 0.5 * (sx + rx);
            let aoff = tr.headers.offset.abs();
            let s = if params.apply_half_deriv {
                half_derivative_2d(&tr.samples, dt_s)
            } else {
                tr.samples.clone()
            };
            (x_to_col(sx), x_to_col(rx), mid_x, aoff, s)
        })
        .collect();

    // 4. Parallel Kirchhoff depth summation across image columns ix = 0..nx
    let ap = params.aperture_m.max(dx);
    let cos_min = (params.max_dip_deg.clamp(10.0, 88.0) as f64).to_radians().cos() as f32;
    let n_bins = params.n_cig_offset_bins;
    let max_off = params.max_cig_offset_m.max(1.0);
    let done_ix = AtomicUsize::new(0);

    let cols_img: Vec<(Vec<f32>, Vec<Vec<f32>>)> = (0..nx)
        .into_par_iter()
        .map(|ix| -> Result<(Vec<f32>, Vec<Vec<f32>>)> {
            if let Some(j) = job {
                j.check_cancelled()?;
            }
            let x_img = vel.x_at(ix);
            let mut img = vec![0.0f64; nz];
            let mut w_img = vec![0.0f64; nz];
            let mut cigs = if n_bins > 0 {
                vec![vec![0.0f64; nz]; n_bins]
            } else {
                Vec::new()
            };

            for (sc, rc, mid_x, aoff, samples) in &prep {
                let d_mid = (x_img - *mid_x).abs();
                if d_mid > ap {
                    continue;
                }
                let Some(ts) = tt_map.get(sc) else { continue };
                let Some(tr_t) = tt_map.get(rc) else { continue };
                let xs = vel.x_at(*sc);
                let xr = vel.x_at(*rc);
                let ap_taper = (0.5 * (1.0 + (PI as f32 * d_mid / ap).cos())).max(0.0) as f64;
                let ns = samples.len();
                let bin = if n_bins > 0 {
                    ((*aoff / max_off) * n_bins as f32).floor().clamp(0.0, (n_bins - 1) as f32) as usize
                } else {
                    0
                };

                for iz in 1..nz {
                    let z_img = vel.z_at(iz);
                    let idx = iz * nx + ix;
                    let t_tot = (ts[idx] + tr_t[idx]) as f64;
                    let f_idx = t_tot / dt_s;
                    let i0 = f_idx.floor() as isize;
                    if i0 < 0 || (i0 as usize + 1) >= ns {
                        continue;
                    }
                    let rs = ((x_img - xs).powi(2) + (z_img - vel.z0_m).powi(2)).sqrt().max(dz);
                    let rr = ((x_img - xr).powi(2) + (z_img - vel.z0_m).powi(2)).sqrt().max(dz);
                    let cos_s = (z_img - vel.z0_m) / rs;
                    let cos_r = (z_img - vel.z0_m) / rr;
                    let obliq = 0.5 * (cos_s + cos_r);
                    if obliq < cos_min {
                        continue;
                    }
                    let frac = (f_idx - i0 as f64) as f32;
                    let amp = samples[i0 as usize] + frac * (samples[i0 as usize + 1] - samples[i0 as usize]);
                    // 2D Kirchhoff obliquity & cylindrical spreading weight
                    let w = ap_taper * (obliq as f64) / ((rs * rr).sqrt() as f64).max(1.0);
                    img[iz] += w * amp as f64;
                    w_img[iz] += w;
                    if n_bins > 0 {
                        cigs[bin][iz] += w * amp as f64;
                    }
                }
            }

            let k = done_ix.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(j) = job {
                if k % 16 == 0 || k == nx {
                    j.set_progress(
                        0.40 + 0.60 * (k as f32 / nx as f32),
                        format!("PSDM: migrating column {k}/{nx}"),
                    );
                }
            }

            let out_col: Vec<f32> = (0..nz)
                .map(|iz| if w_img[iz] > 1e-12 { (img[iz] / w_img[iz].sqrt()) as f32 } else { 0.0 })
                .collect();
            let out_cigs: Vec<Vec<f32>> = cigs
                .into_iter()
                .map(|v| v.into_iter().map(|x| x as f32).collect())
                .collect();
            Ok((out_col, out_cigs))
        })
        .collect::<Result<Vec<_>>>()?;

    // Represent depth spacing dz_m as pseudo-dt_us = dz_m * 1000 so depth renders cleanly in m (1 "ms" = 1 m)
    let depth_dt_us = (dz * 1000.0).round().max(100.0) as u32;
    let mut image_traces = Vec::with_capacity(nx);
    let mut cig_traces = Vec::with_capacity(nx * n_bins);
    for (ix, (col, cigs)) in cols_img.into_iter().enumerate() {
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

        for (b, c_samples) in cigs.into_iter().enumerate() {
            let mut ct = Trace::new(depth_dt_us, c_samples);
            ct.headers = TraceHeaders {
                traceno: (ix * n_bins + b + 1) as i32,
                ffid: 1,
                chan: (b + 1) as i32,
                cdp: (ix + 1) as i32,
                cdp_x: x_m,
                offset: ((b as f32 + 0.5) / n_bins as f32) * max_off,
                ..Default::default()
            };
            ct.headers.custom.insert("DEPTH_DZ_M".into(), dz as f64);
            cig_traces.push(ct);
        }
    }

    Ok(PsdmOutput {
        image_traces,
        cig_traces,
        n_eikonal_tables: tt_map.len(),
    })
}
