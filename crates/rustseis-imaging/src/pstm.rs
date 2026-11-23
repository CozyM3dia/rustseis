use crate::migration::{build_double_integral, half_derivative_filter_with_plan, sample_triangle_aa};
use crate::nmo::sinc_interp;
use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace, TraceHeaders, VelocityTable};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f64::consts::PI;

/// 2D Pre-Stack Kirchhoff Time Migration (`kirtime2d` — DSR traveltime + CIG output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KirchhoffPstm2dParams {
    /// Output image CDP spacing `dx` in meters (default 6.25 m).
    #[serde(default = "default_pstm_dx")]
    pub dx_m: f32,
    /// Optional explicit output CDP range `(min_cdp, max_cdp, step)`; `None` = infer from input traces.
    #[serde(default)]
    pub cdp_step: usize,
    /// Maximum migration half-aperture in meters (default 1500 m).
    #[serde(default = "default_pstm_ap")]
    pub max_aperture_m: f32,
    /// Maximum reflection dip angle in degrees (default 55 deg).
    #[serde(default = "default_pstm_dip")]
    pub max_dip_deg: f32,
    /// Maximum migration stretch ratio (default 1.5; suppresses shallow far-offset stretch).
    #[serde(default = "default_stretch")]
    pub max_stretch_ratio: f32,
    /// Apply 2D half-derivative phase/amplitude filter (default true).
    #[serde(default = "default_true_p")]
    pub apply_half_derivative: bool,
    /// Enable Lumley-Claerbout triangle anti-aliasing (default true).
    #[serde(default = "default_true_p")]
    pub anti_alias: bool,
    /// Output Common-Image Gathers (true) binned by offset, or direct PSTM stacked section (false).
    #[serde(default)]
    pub output_cig: bool,
    /// Offset bin width in meters when `output_cig == true` (default 100 m).
    #[serde(default = "default_cig_bin")]
    pub offset_bin_m: f32,
    /// Maximum offset to include in migration in meters (0 = unlimited).
    #[serde(default)]
    pub max_offset_m: f32,
}

fn default_pstm_dx() -> f32 { 6.25 }
fn default_pstm_ap() -> f32 { 1500.0 }
fn default_pstm_dip() -> f32 { 55.0 }
fn default_stretch() -> f32 { 1.5 }
fn default_true_p() -> bool { true }
fn default_cig_bin() -> f32 { 100.0 }

impl Default for KirchhoffPstm2dParams {
    fn default() -> Self {
        Self {
            dx_m: 6.25,
            cdp_step: 1,
            max_aperture_m: 1500.0,
            max_dip_deg: 55.0,
            max_stretch_ratio: 1.5,
            apply_half_derivative: true,
            anti_alias: true,
            output_cig: false,
            offset_bin_m: 100.0,
            max_offset_m: 0.0,
        }
    }
}

/// Internal spatially indexed pre-stack input trace for O(aperture) binary-search PSTM.
struct PstmInput {
    x_s: f64,
    x_r: f64,
    x_m: f64,
    off_bin: i32,
    samples: Vec<f32>,
    integral: Vec<f64>,
}

/// 2D Pre-Stack Kirchhoff Time Migration (`kirtime2d`).
/// Uses exact Double-Square-Root (DSR) traveltime:
///   `t = sqrt((t0/2)^2 + (x_out - x_s)^2 / V^2) + sqrt((t0/2)^2 + (x_out - x_r)^2 / V^2)`
/// with spatial binary-search indexing, half-derivative filter, obliquity + geometric weight,
/// outer-20% aperture taper, stretch mute, triangle anti-aliasing, and optional CIG output.
pub fn kirchhoff_pstm_2d(
    traces: &[Trace],
    vel: &VelocityTable,
    params: &KirchhoffPstm2dParams,
) -> Result<Vec<Trace>> {
    if traces.len() < 4 || traces[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Need >= 4 traces for 2D PSTM".into()));
    }
    let dt_us = traces[0].dt_us;
    let dt_s = traces[0].dt_s() as f64;
    let nt = traces[0].samples.len();
    let dx = params.dx_m.max(0.1) as f64;

    // Project source/receiver 2D coordinates onto line axis `x`
    let has_coords = traces
        .iter()
        .any(|t| t.headers.sou_x.abs() > 1e-3 || t.headers.rec_x.abs() > 1e-3);
    let (ox, oy, ux, uy) = if has_coords {
        let mut min_cdp = i32::MAX;
        let mut max_cdp = i32::MIN;
        let mut p_min = (traces[0].headers.cdp_x, traces[0].headers.cdp_y);
        let mut p_max = p_min;
        for tr in traces {
            if !tr.is_live() { continue; }
            if tr.headers.cdp < min_cdp {
                min_cdp = tr.headers.cdp;
                p_min = (tr.headers.cdp_x, tr.headers.cdp_y);
            }
            if tr.headers.cdp > max_cdp {
                max_cdp = tr.headers.cdp;
                p_max = (tr.headers.cdp_x, tr.headers.cdp_y);
            }
        }
        let ddx = p_max.0 - p_min.0;
        let ddy = p_max.1 - p_min.1;
        let dist = ddx.hypot(ddy);
        if dist > 1.0 {
            (p_min.0, p_min.1, ddx / dist, ddy / dist)
        } else {
            (p_min.0, p_min.1, 1.0, 0.0)
        }
    } else {
        (0.0, 0.0, 1.0, 0.0)
    };

    // Hoist FFT planning once outside rayon loop!
    let nfft = (nt + nt / 4).next_power_of_two().max(64);
    let mut planner = FftPlanner::<f64>::new();
    let fft_hd = planner.plan_fft_forward(nfft);
    let ifft_hd = planner.plan_fft_inverse(nfft);

    let max_off_lim = if params.max_offset_m > 0.0 {
        params.max_offset_m as f64
    } else {
        f64::INFINITY
    };
    let obin_w = params.offset_bin_m.max(1.0) as f64;

    // Pre-filter, compute double-integrals, and project coordinates in parallel
    let mut inputs: Vec<PstmInput> = traces
        .par_iter()
        .filter_map(|tr| {
            if !tr.is_live() || tr.samples.len() < nt {
                return None;
            }
            let (xs, xr, xm) = if has_coords {
                let s = (tr.headers.sou_x - ox) * ux + (tr.headers.sou_y - oy) * uy;
                let r = (tr.headers.rec_x - ox) * ux + (tr.headers.rec_y - oy) * uy;
                (s, r, 0.5 * (s + r))
            } else {
                let m = (tr.headers.cdp as f64 - 1.0) * dx;
                let h = 0.5 * tr.headers.offset as f64;
                (m - h, m + h, m)
            };
            let abs_off = (xr - xs).abs();
            if abs_off > max_off_lim {
                return None;
            }
            let samples = if params.apply_half_derivative {
                half_derivative_filter_with_plan(&tr.samples[..nt], dt_s, fft_hd.as_ref(), ifft_hd.as_ref(), nfft)
            } else {
                tr.samples[..nt].to_vec()
            };
            let integral = if params.anti_alias {
                build_double_integral(&samples)
            } else {
                Vec::new()
            };
            let off_bin = (abs_off / obin_w).floor() as i32;
            Some(PstmInput {
                x_s: xs,
                x_r: xr,
                x_m: xm,
                off_bin,
                samples,
                integral,
            })
        })
        .collect();

    if inputs.is_empty() {
        return Err(SeismicError::InvalidFormat("No live traces within offset limit for PSTM".into()));
    }

    // Sort inputs by midpoint x_m for fast O(log N) aperture slicing!
    inputs.sort_by(|a, b| a.x_m.total_cmp(&b.x_m));

    // Build output image locations (one per unique CDP stepped by cdp_step)
    let mut cdp_pos = BTreeMap::<i32, (f64, f64, f64, usize)>::new();
    for tr in traces {
        if !tr.is_live() { continue; }
        let xm = if has_coords {
            (tr.headers.cdp_x - ox) * ux + (tr.headers.cdp_y - oy) * uy
        } else {
            (tr.headers.cdp as f64 - 1.0) * dx
        };
        let e = cdp_pos.entry(tr.headers.cdp).or_insert((0.0, 0.0, 0.0, 0));
        e.0 += xm;
        e.1 += tr.headers.cdp_x;
        e.2 += tr.headers.cdp_y;
        e.3 += 1;
    }

    let step = params.cdp_step.max(1);
    let img_locs: Vec<(i32, f64, f64, f64)> = cdp_pos
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % step == 0)
        .map(|(_, (cdp, (sx, cx, cy, cnt)))| {
            let inv = 1.0 / cnt.max(1) as f64;
            (cdp, sx * inv, cx * inv, cy * inv)
        })
        .collect();

    // Unique offset bins if CIG output requested
    let mut off_bins: Vec<i32> = if params.output_cig {
        let mut b: Vec<i32> = inputs.iter().map(|t| t.off_bin).collect();
        b.sort_unstable();
        b.dedup();
        b
    } else {
        vec![0]
    };
    if off_bins.is_empty() {
        off_bins.push(0);
    }
    let n_obins = off_bins.len();
    let obin_to_idx: BTreeMap<i32, usize> = off_bins.iter().enumerate().map(|(i, &b)| (b, i)).collect();

    let ap_m = params.max_aperture_m.max(dx as f32) as f64;
    let cos_max_dip = (params.max_dip_deg.clamp(5.0, 89.0) as f64 * PI / 180.0).cos();
    let max_stretch = params.max_stretch_ratio.max(1.05) as f64;
    let output_cig = params.output_cig;
    let anti_alias = params.anti_alias;

    // Parallel Kirchhoff PSTM over output image locations
    let out_groups: Vec<Vec<Trace>> = img_locs
        .par_iter()
        .map(|&(cdp, x_out, cdp_x, cdp_y)| {
            // Binary search input traces whose midpoint lies within [x_out - ap_m, x_out + ap_m]
            let lo = inputs.partition_point(|t| t.x_m < x_out - ap_m);
            let hi = inputs.partition_point(|t| t.x_m <= x_out + ap_m);

            // Precompute per-sample velocity & time constants ONCE per output image CDP (eliminates redundant divisions/muls!)
            let inv_dt = (1.0 / dt_s) as f32;
            let dx_f = dx as f32;
            let cos_max_dip_f = cos_max_dip as f32;
            let inv_max_stretch_f = (1.0 / max_stretch) as f32;
            let max_pos_f = (nt - 1) as f32;

            let mut half_t0_col = vec![0.0f32; nt];
            let mut half_t0_sq_col = vec![0.0f32; nt];
            let mut inv_v2_col = vec![0.0f32; nt];
            let mut inv_sqrt_v_col = vec![0.0f32; nt];
            for it0 in 0..nt {
                let t0 = (it0 as f64 * dt_s) as f32;
                let v = vel.velocity_at(cdp, t0 * 1000.0).max(150.0);
                half_t0_col[it0] = 0.5 * t0;
                half_t0_sq_col[it0] = 0.25 * t0 * t0;
                inv_v2_col[it0] = 1.0 / (v * v);
                inv_sqrt_v_col[it0] = 1.0 / v.sqrt();
            }

            let mut accum = vec![vec![0.0f32; nt]; n_obins];
            let mut fold = vec![vec![0u32; nt]; n_obins];

            for tr in &inputs[lo..hi] {
                let dist_m = (tr.x_m - x_out).abs();
                if dist_m > ap_m {
                    continue;
                }
                let b_idx = if output_cig {
                    *obin_to_idx.get(&tr.off_bin).unwrap_or(&0)
                } else {
                    0
                };

                // Outer 20% cosine aperture taper
                let ap_w = if dist_m > 0.8 * ap_m {
                    let frac = (dist_m - 0.8 * ap_m) / (0.2 * ap_m).max(1e-6);
                    (0.5 * (1.0 + (PI * frac).cos())) as f32
                } else {
                    1.0f32
                };

                let ds = (x_out - tr.x_s) as f32;
                let dr = (x_out - tr.x_r) as f32;
                let ds2 = ds * ds;
                let dr2 = dr * dr;

                let acc_row = &mut accum[b_idx];
                let fld_row = &mut fold[b_idx];
                let tr_samps = &tr.samples;
                let tr_int = &tr.integral;

                for it0 in 1..nt {
                    let half_t0_sq = half_t0_sq_col[it0];
                    let inv_v2 = inv_v2_col[it0];

                    let ts = (half_t0_sq + ds2 * inv_v2).sqrt();
                    let tr_t = (half_t0_sq + dr2 * inv_v2).sqrt();
                    let t_dsr = ts + tr_t;

                    let pos = t_dsr * inv_dt;
                    if pos >= max_pos_f {
                        break;
                    }

                    let inv_ts = 1.0 / ts;
                    let inv_tr = 1.0 / tr_t;
                    let half_t0 = half_t0_col[it0];
                    let cos_dip = (half_t0 * inv_ts).min(half_t0 * inv_tr);
                    if cos_dip < cos_max_dip_f {
                        continue;
                    }

                    let dt_dt0 = 0.5 * half_t0 * (inv_ts + inv_tr);
                    if dt_dt0 < inv_max_stretch_f {
                        continue;
                    }

                    let weight = dt_dt0 * inv_sqrt_v_col[it0] / t_dsr.sqrt() * ap_w;

                    let amp = if anti_alias {
                        let slope = ((ds * inv_ts + dr * inv_tr).abs() * inv_v2) * dx_f;
                        let half_len = ((slope * inv_dt) as usize).min(24);
                        sample_triangle_aa(tr_int, tr_samps, pos as f64, half_len)
                    } else {
                        sinc_interp(tr_samps, pos as f64)
                    };

                    acc_row[it0] += amp * weight;
                    fld_row[it0] += 1;
                }
            }

            let mut group = Vec::with_capacity(n_obins);
            for (bi, &obin) in off_bins.iter().enumerate() {
                let mut samps = vec![0.0f32; nt];
                let mut max_fold = 0u32;
                for it0 in 0..nt {
                    let f = fold[bi][it0];
                    if f > 0 {
                        samps[it0] = accum[bi][it0] / (f as f32).sqrt();
                        if f > max_fold {
                            max_fold = f;
                        }
                    }
                }
                let mut hdr = TraceHeaders::default();
                hdr.cdp = cdp;
                hdr.cdp_sloc = cdp;
                hdr.cdp_x = cdp_x;
                hdr.cdp_y = cdp_y;
                hdr.trc_type = if max_fold > 0 { 1 } else { 0 };
                hdr.tr_fold = max_fold as i32;
                hdr.cdp_nfld = max_fold as i32;
                if output_cig {
                    let off_center = (obin as f32 + 0.5) * params.offset_bin_m;
                    hdr.offset = off_center;
                    hdr.aoffset = off_center;
                    hdr.ofb_no = obin;
                    hdr.ofb_cntr = off_center;
                    hdr.seqno = (bi + 1) as i32;
                    hdr.end_ens = if bi + 1 == n_obins { 1 } else { 0 };
                } else {
                    hdr.offset = 0.0;
                    hdr.aoffset = 0.0;
                }
                let mut tr = Trace::new(dt_us, samps);
                tr.headers = hdr;
                group.push(tr);
            }
            group
        })
        .collect();

    let mut out: Vec<Trace> = out_groups.into_iter().flatten().collect();
    for (i, tr) in out.iter_mut().enumerate() {
        tr.headers.traceno = (i + 1) as i32;
    }
    if let Some(last) = out.last_mut() {
        last.headers.eoj = 1;
    }
    Ok(out)
}

/// Hale (1984) Exact 2D F-K Dip Moveout (`dmofkPanel`) parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmoFkParams {
    /// Common-offset CDP trace spacing `dx` in meters (default 6.25 m).
    #[serde(default = "default_pstm_dx")]
    pub dx_m: f32,
    /// Offset bin width in meters used to group NMO-corrected traces into common-offset panels (default 25 m).
    #[serde(default = "default_dmo_bin")]
    pub offset_bin_m: f32,
    /// Maximum frequency to apply DMO in Hz (default 80 Hz).
    #[serde(default = "default_dmo_fmax")]
    pub f_max_hz: f32,
    /// Spatial padding traces on each side of common-offset panel (default 16).
    #[serde(default = "default_dmo_pad")]
    pub pad_traces: usize,
}

fn default_dmo_bin() -> f32 { 25.0 }
fn default_dmo_fmax() -> f32 { 80.0 }
fn default_dmo_pad() -> usize { 16 }

impl Default for DmoFkParams {
    fn default() -> Self {
        Self {
            dx_m: 6.25,
            offset_bin_m: 25.0,
            f_max_hz: 80.0,
            pad_traces: 16,
        }
    }
}

/// Apply Hale (1984) exact F-K Dip Moveout (`dmofkPanel`) to a single common-offset NMO-corrected panel.
fn apply_hale_dmo_panel(panel: &mut [Trace], half_offset_m: f64, params: &DmoFkParams) {
    let nx = panel.len();
    if nx < 3 || half_offset_m.abs() < 1.0 {
        return;
    }
    let nt = panel[0].samples.len();
    let dt_s = panel[0].dt_s() as f64;
    let dx = params.dx_m.max(0.1) as f64;
    let pad = params.pad_traces;
    let nfft_x = (nx + 2 * pad).next_power_of_two().max(32);
    let nfft_t = nt.next_power_of_two().max(64);

    let mut planner = FftPlanner::<f64>::new();
    let fft_x = planner.plan_fft_forward(nfft_x);
    let ifft_x = planner.plan_fft_inverse(nfft_x);
    let ifft_t = planner.plan_fft_inverse(nfft_t);

    // 1. Spatial FFT (t_n, x) -> (t_n, k)
    let mut tx_k = vec![Complex::new(0.0, 0.0); nt * nfft_x];
    for it in 0..nt {
        let row = &mut tx_k[it * nfft_x..(it + 1) * nfft_x];
        for ix in 0..nx {
            row[ix + pad] = Complex::new(panel[ix].samples[it] as f64, 0.0);
        }
        fft_x.process(row);
    }

    // 2. Hale (1984) direct transform: P_dmo(w0, k) = sum_{t_n} P_nmo(t_n, k) * (1/A) * exp(-i * w0 * t_n * A)
    //    where A = sqrt(1 + (k * h / (w0 * t_n))^2), using FFT sign exp(-i*w*t).
    let dw = 2.0 * PI / (nfft_t as f64 * dt_s);
    let dkx = 2.0 * PI / (nfft_x as f64 * dx);
    let w_max = 2.0 * PI * (params.f_max_hz as f64).min(0.5 / dt_s);
    let kw_max = ((w_max / dw).ceil() as usize).min(nfft_t / 2);

    // Compute per-wavenumber column in parallel
    let dmo_cols: Vec<Vec<Complex<f64>>> = (0..nfft_x)
        .into_par_iter()
        .map(|ik| {
            let k = if ik <= nfft_x / 2 {
                ik as f64 * dkx
            } else {
                (ik as isize - nfft_x as isize) as f64 * dkx
            };
            let kh = k * half_offset_m;
            let kh2 = kh * kh;
            let mut spec = vec![Complex::new(0.0, 0.0); nfft_t];

            // Collect non-zero time samples for this wavenumber
            let col_tn: Vec<Complex<f64>> = (0..nt).map(|it| tx_k[it * nfft_x + ik]).collect();

            for iw in 1..=kw_max {
                let w0 = iw as f64 * dw;
                let mut sum = Complex::new(0.0, 0.0);
                for it in 1..nt {
                    let tn = it as f64 * dt_s;
                    let wt = w0 * tn;
                    let ratio2 = kh2 / (wt * wt).max(1e-12);
                    // Anti-aliasing dip stability cap (ratio < 2.5 => dip < ~68 deg)
                    if ratio2 > 6.25 {
                        continue;
                    }
                    let a = (1.0 + ratio2).sqrt();
                    let phase = -wt * a;
                    sum += col_tn[it] * Complex::from_polar(1.0 / a, phase);
                }
                spec[iw] = sum;
                if iw < nfft_t - iw {
                    spec[nfft_t - iw] = sum.conj();
                }
            }
            // Inverse temporal FFT -> (t0, k)
            ifft_t.process(&mut spec);
            let inv_t = 1.0 / nfft_t as f64;
            spec.iter().take(nt).map(|&c| c * inv_t).collect()
        })
        .collect();

    // 3. Inverse spatial FFT (t0, k) -> (t0, x)
    let inv_x = 1.0 / nfft_x as f64;
    let mut row = vec![Complex::new(0.0, 0.0); nfft_x];
    for it in 0..nt {
        for ik in 0..nfft_x {
            row[ik] = dmo_cols[ik][it];
        }
        ifft_x.process(&mut row);
        for ix in 0..nx {
            panel[ix].samples[it] = (row[ix + pad].re * inv_x) as f32;
        }
    }
    for tr in panel.iter_mut() {
        tr.headers.dmooff = (2.0 * half_offset_m) as f32;
    }
}

/// Apply 2D F-K Dip Moveout (`dmofkPanel`) across an NMO-corrected pre-stack dataset.
/// Automatically bins input traces into common-offset sections, sorts by CDP, applies Hale DMO,
/// and returns all DMO-corrected traces.
pub fn dmo_fk_2d(traces: &[Trace], params: &DmoFkParams) -> Result<Vec<Trace>> {
    if traces.len() < 4 || traces[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Need >= 4 traces for F-K DMO".into()));
    }
    let bin_w = params.offset_bin_m.max(1.0);
    let mut bins = BTreeMap::<i32, Vec<Trace>>::new();
    for tr in traces {
        let aoff = if tr.headers.aoffset > 0.0 {
            tr.headers.aoffset
        } else {
            tr.headers.offset.abs()
        };
        let b = (aoff / bin_w).round() as i32;
        bins.entry(b).or_default().push(tr.clone());
    }

    let mut out = Vec::with_capacity(traces.len());
    for (b, mut panel) in bins {
        panel.sort_by_key(|t| t.headers.cdp);
        let mean_off = panel
            .iter()
            .map(|t| {
                if t.headers.aoffset > 0.0 {
                    t.headers.aoffset as f64
                } else {
                    t.headers.offset.abs() as f64
                }
            })
            .sum::<f64>()
            / panel.len().max(1) as f64;
        let half_off = if mean_off > 1.0 { 0.5 * mean_off } else { 0.5 * b as f64 * bin_w as f64 };
        apply_hale_dmo_panel(&mut panel, half_off, params);
        out.extend(panel);
    }
    Ok(out)
}

/// 2D Finite-Difference 45° Post-Stack Time Migration (`fdmig` — Claerbout / Ma implicit omega-x Crank-Nicolson).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FdMig2dParams {
    /// CDP spacing `dx` in meters (default 6.25 m).
    #[serde(default = "default_pstm_dx")]
    pub dx_m: f32,
    /// Depth/time extrapolation step in samples (default 1 = every sample).
    #[serde(default = "default_fd_step")]
    pub dtau_samples: usize,
    /// Maximum frequency to migrate in Hz (default 85 Hz).
    #[serde(default = "default_fd_fmax")]
    pub f_max_hz: f32,
    /// Spatial absorbing pad traces on each side (default 16).
    #[serde(default = "default_dmo_pad")]
    pub pad_traces: usize,
}

fn default_fd_step() -> usize { 1 }
fn default_fd_fmax() -> f32 { 85.0 }

impl Default for FdMig2dParams {
    fn default() -> Self {
        Self {
            dx_m: 6.25,
            dtau_samples: 1,
            f_max_hz: 85.0,
            pad_traces: 16,
        }
    }
}

/// Complex tridiagonal Thomas solver `lower[i]*x[i-1] + diag[i]*x[i] + upper[i]*x[i+1] = rhs[i]`.
fn solve_tridiag_complex(
    lower: &[Complex<f64>],
    diag: &[Complex<f64>],
    upper: &[Complex<f64>],
    rhs: &[Complex<f64>],
    out: &mut [Complex<f64>],
    c_prime: &mut [Complex<f64>],
    d_prime: &mut [Complex<f64>],
) {
    let n = diag.len();
    c_prime[0] = upper[0] / diag[0];
    d_prime[0] = rhs[0] / diag[0];
    for i in 1..n {
        let denom = diag[i] - lower[i] * c_prime[i - 1];
        c_prime[i] = if i + 1 < n { upper[i] / denom } else { Complex::new(0.0, 0.0) };
        d_prime[i] = (rhs[i] - lower[i] * d_prime[i - 1]) / denom;
    }
    out[n - 1] = d_prime[n - 1];
    for i in (0..n - 1).rev() {
        out[i] = d_prime[i] - c_prime[i] * out[i + 1];
    }
}

/// 2D Implicit 45° Omega-X Finite-Difference Post-Stack Time Migration (`fdmig`).
/// Supports full laterally & vertically variant `V(x, tau)` using Claerbout's 45° continued-fraction
/// paraxial wave equation solved via Crank-Nicolson + Thomas tridiagonal solver in parallel across frequencies.
pub fn fd_migrate_2d(
    section: &[Trace],
    vel: &VelocityTable,
    params: &FdMig2dParams,
) -> Result<Vec<Trace>> {
    if section.len() < 4 || section[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Need >= 4 traces for 2D FD migration".into()));
    }
    let nx = section.len();
    let nt = section[0].samples.len();
    let dt = section[0].dt_s() as f64;
    let dx = params.dx_m.max(0.1) as f64;
    let pad = params.pad_traces;
    let nx_pad = nx + 2 * pad;

    let nfft_t = (nt + nt / 4).next_power_of_two().max(64);
    let mut planner = FftPlanner::<f64>::new();
    let fft_t = planner.plan_fft_forward(nfft_t);

    // 1. Temporal FFT of each trace -> freq-major matrix `P(w, x)`
    let dw = 2.0 * PI / (nfft_t as f64 * dt);
    let w_max = 2.0 * PI * (params.f_max_hz as f64).min(0.5 / dt);
    let kw_max = ((w_max / dw).ceil() as usize).clamp(1, nfft_t / 2);

    let mut p_init = vec![Complex::new(0.0, 0.0); (kw_max + 1) * nx_pad];
    let mut buf = vec![Complex::new(0.0, 0.0); nfft_t];
    for ix in 0..nx {
        buf.fill(Complex::new(0.0, 0.0));
        for it in 0..nt {
            buf[it] = Complex::new(section[ix].samples[it] as f64, 0.0);
        }
        fft_t.process(&mut buf);
        for iw in 1..=kw_max {
            p_init[iw * nx_pad + (ix + pad)] = buf[iw];
        }
    }

    // Precompute velocity grid V(tau, x_pad)
    let mut v_grid = vec![1500.0f64; nt * nx_pad];
    for itau in 0..nt {
        let t_ms = (itau as f64 * dt * 1000.0) as f32;
        for ix in 0..nx_pad {
            let src_ix = ix.saturating_sub(pad).min(nx - 1);
            let cdp = section[src_ix].headers.cdp;
            v_grid[itau * nx_pad + ix] = vel.velocity_at(cdp, t_ms).max(150.0) as f64;
        }
    }

    // Gentle absorbing taper in spatial pad margins
    let mut absorb = vec![1.0f64; nx_pad];
    for i in 0..pad {
        let u = (i as f64 + 1.0) / (pad as f64 + 1.0);
        let w = (0.5 * PI * u).sin().powi(2);
        absorb[i] = w;
        absorb[nx_pad - 1 - i] = w;
    }

    // 2. Parallel downward continuation across positive frequencies (imaging condition = 2 * Re(sum_w P) / nfft_t)
    //    45-deg Claerbout coefficients: alpha = 0.5, beta = 0.25, 1/6 trick gamma = 1/6
    let alpha = 0.5f64;
    let beta = 0.25f64;
    let gamma_sixth = 1.0 / 6.0;
    let dtau = dt; // step size in vertical two-way time

    let freq_images: Vec<Vec<f64>> = (1..=kw_max)
        .into_par_iter()
        .map(|iw| {
            let w = iw as f64 * dw;
            let mut p = p_init[iw * nx_pad..(iw + 1) * nx_pad].to_vec();
            let mut next_p = vec![Complex::new(0.0, 0.0); nx_pad];
            let mut lower = vec![Complex::new(0.0, 0.0); nx_pad];
            let mut diag = vec![Complex::new(0.0, 0.0); nx_pad];
            let mut upper = vec![Complex::new(0.0, 0.0); nx_pad];
            let mut rhs = vec![Complex::new(0.0, 0.0); nx_pad];
            let mut c_pr = vec![Complex::new(0.0, 0.0); nx_pad];
            let mut d_pr = vec![Complex::new(0.0, 0.0); nx_pad];

            let mut img = vec![0.0f64; nt * nx];
            let shift_factor = Complex::from_polar(1.0, w * dtau); // Thin-lens time-shift exp(+i*w*dtau)

            for itau in 0..nt {
                // Imaging condition at t = 0: accumulate real part of wavefield
                for ix in 0..nx {
                    img[itau * nx + ix] = p[ix + pad].re;
                }
                if itau + 1 == nt {
                    break;
                }

                // 2a. Thin-lens shift: P <- P * exp(+i * w * dtau)
                for v in &mut p {
                    *v *= shift_factor;
                }

                // 2b. 45-degree diffraction step via Crank-Nicolson implicit tridiagonal solve:
                //     [1 + (b - i*a) * D_xx] P^{n+1} = [1 + (b + i*a) * D_xx] P^n
                //     where b = gamma_sixth + beta * (v^2 / (4*w^2*dx^2)), a = alpha * (v^2 * dtau / (8*w*dx^2))
                //     (Using exploding-reflector v_eff = v/2 => v_eff^2 = v^2 / 4).
                let v_row = &v_grid[itau * nx_pad..(itau + 1) * nx_pad];
                for ix in 0..nx_pad {
                    let v = v_row[ix];
                    let v2_dx2 = (v * v) / (4.0 * dx * dx);
                    let b = gamma_sixth + beta * v2_dx2 / (w * w);
                    let a = 0.5 * alpha * v2_dx2 * dtau / w;

                    // LHS coefficient for D_xx: c_l = b - i*a  -> diag = 1 - 2*c_l, off = c_l
                    // RHS coefficient for D_xx: c_r = b + i*a  -> diag = 1 - 2*c_r, off = c_r
                    let c_l = Complex::new(b, -a);
                    let c_r = Complex::new(b, a);

                    let p_im1 = if ix > 0 { p[ix - 1] } else { p[0] };
                    let p_i = p[ix];
                    let p_ip1 = if ix + 1 < nx_pad { p[ix + 1] } else { p[nx_pad - 1] };

                    rhs[ix] = p_i + c_r * (p_im1 - p_i * 2.0 + p_ip1);
                    lower[ix] = if ix > 0 { c_l } else { Complex::new(0.0, 0.0) };
                    upper[ix] = if ix + 1 < nx_pad { c_l } else { Complex::new(0.0, 0.0) };
                    diag[ix] = Complex::new(1.0, 0.0) - c_l * 2.0;
                }
                // Zero-slope boundary conditions at pad edges
                diag[0] += lower[1];
                diag[nx_pad - 1] += upper[nx_pad - 2];

                solve_tridiag_complex(&lower, &diag, &upper, &rhs, &mut next_p, &mut c_pr, &mut d_pr);

                for ix in 0..nx_pad {
                    p[ix] = next_p[ix] * absorb[ix];
                }
            }
            img
        })
        .collect();

    // 3. Sum frequency images
    let scale = 2.0 / nfft_t as f64;
    let mut out = section.to_vec();
    for tr in &mut out {
        tr.samples.fill(0.0);
    }
    for f_img in freq_images {
        for itau in 0..nt {
            for ix in 0..nx {
                out[ix].samples[itau] += (f_img[itau * nx + ix] * scale) as f32;
            }
        }
    }
    Ok(out)
}
