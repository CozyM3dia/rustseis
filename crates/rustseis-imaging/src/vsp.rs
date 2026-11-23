use crate::statics::{FirstBreakPickParams, pick_first_breaks};
use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace, TraceHeaders};
use rustseis_dsp::shift_trace_ms;
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// VSP / Borehole DAS Geometry assignment (`vsp_geom`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VspGeometryParams {
    /// Shallowest channel measured depth in meters (default 0.0 m).
    #[serde(default)]
    pub start_depth_m: f32,
    /// Depth spacing between consecutive borehole / DAS channels in meters (default 1.0 m for DAS, 15.0 m for VSP).
    #[serde(default = "default_dz")]
    pub channel_spacing_m: f32,
    /// Horizontal source offset from wellhead in meters (default 50.0 m).
    #[serde(default = "default_well_off")]
    pub source_offset_m: f32,
    /// Only populate `rec_dep` when existing `rec_dep == 0.0` (default true).
    #[serde(default = "default_true_v")]
    pub preserve_existing_depth: bool,
    /// Sort traces by ascending borehole depth `rec_dep` (default true).
    #[serde(default = "default_true_v")]
    pub sort_by_depth: bool,
}

fn default_dz() -> f32 { 1.0 }
fn default_well_off() -> f32 { 50.0 }
fn default_true_v() -> bool { true }

impl Default for VspGeometryParams {
    fn default() -> Self {
        Self {
            start_depth_m: 0.0,
            channel_spacing_m: 1.0,
            source_offset_m: 50.0,
            preserve_existing_depth: true,
            sort_by_depth: true,
        }
    }
}

/// Assign VSP / Borehole DAS receiver depths (`rec_dep`), radial distance (`aoffset`), and depth sort.
pub fn assign_vsp_geometry(traces: &mut [Trace], params: &VspGeometryParams) {
    for (idx, tr) in traces.iter_mut().enumerate() {
        let z = if params.preserve_existing_depth && tr.headers.rec_dep.abs() > 1e-3 {
            tr.headers.rec_dep
        } else {
            params.start_depth_m + (idx as f32) * params.channel_spacing_m
        };
        tr.headers.rec_dep = z;
        tr.headers.rec_elev = -z;
        let radial = (params.source_offset_m * params.source_offset_m + z * z).sqrt();
        tr.headers.offset = params.source_offset_m;
        tr.headers.aoffset = radial;
    }
    if params.sort_by_depth {
        traces.sort_by(|a, b| a.headers.rec_dep.total_cmp(&b.headers.rec_dep));
    }
}

/// Interval slowness & velocity profile estimated along a VSP / DAS borehole (`vsp_slowness`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VspSlownessProfile {
    pub depths_m: Vec<f32>,
    pub fb_times_ms: Vec<f32>,
    pub slowness_us_per_m: Vec<f32>,
    pub interval_vel_mps: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VspSlownessParams {
    /// Sliding regression window in channels (odd >= 3, default 11).
    #[serde(default = "default_sl_win")]
    pub window_channels: usize,
    /// Automatically run `fbpick` if `FB_PICK` header is mostly zero (default true).
    #[serde(default = "default_true_v")]
    pub auto_pick_fb: bool,
    #[serde(default)]
    pub fb_params: FirstBreakPickParams,
}

fn default_sl_win() -> usize { 11 }

impl Default for VspSlownessParams {
    fn default() -> Self {
        Self {
            window_channels: 11,
            auto_pick_fb: true,
            fb_params: FirstBreakPickParams::default(),
        }
    }
}

/// Estimate interval slowness (us/m) and interval velocity (m/s) along a borehole from first-break picks.
pub fn estimate_vsp_slowness(
    traces: &mut [Trace],
    params: &VspSlownessParams,
) -> Result<VspSlownessProfile> {
    if traces.len() < 3 {
        return Err(SeismicError::InvalidFormat("Need >= 3 depth channels for VSP slowness".into()));
    }
    let has_fb = traces.iter().filter(|t| t.headers.fb_pick > 0.0).count();
    if has_fb < traces.len() / 3 && params.auto_pick_fb {
        pick_first_breaks(traces, &params.fb_params, "vsp_fb");
    }

    let n = traces.len();
    let depths: Vec<f32> = traces.iter().map(|t| t.headers.rec_dep).collect();
    let fbs: Vec<f32> = traces.iter().map(|t| t.headers.fb_pick).collect();
    let hw = (params.window_channels.max(3) / 2).max(1);

    let mut slowness_us_per_m = vec![0.0f32; n];
    let mut interval_vel_mps = vec![0.0f32; n];

    for i in 0..n {
        let lo = i.saturating_sub(hw);
        let hi = (i + hw).min(n - 1);
        let mut pts: Vec<(f64, f64)> = Vec::new();
        for j in lo..=hi {
            if fbs[j] > 0.0 {
                pts.push((depths[j] as f64, fbs[j] as f64 / 1000.0));
            }
        }
        if pts.len() >= 2 {
            let m = pts.len() as f64;
            let mz = pts.iter().map(|p| p.0).sum::<f64>() / m;
            let mt = pts.iter().map(|p| p.1).sum::<f64>() / m;
            let mut s_zz = 0.0f64;
            let mut s_zt = 0.0f64;
            for &(z, t) in &pts {
                s_zz += (z - mz) * (z - mz);
                s_zt += (z - mz) * (t - mt);
            }
            if s_zz > 1e-6 {
                let s_s_per_m = (s_zt / s_zz).clamp(1.0 / 9000.0, 1.0 / 300.0);
                slowness_us_per_m[i] = (s_s_per_m * 1e6) as f32;
                interval_vel_mps[i] = (1.0 / s_s_per_m) as f32;
            }
        }
    }

    // Store interval velocity in custom header `VSP_VINT` for QC plotting
    for (tr, &v) in traces.iter_mut().zip(interval_vel_mps.iter()) {
        if v > 0.0 {
            tr.headers.custom.insert("VSP_VINT".into(), v as f64);
        }
    }

    Ok(VspSlownessProfile {
        depths_m: depths,
        fb_times_ms: fbs,
        slowness_us_per_m,
        interval_vel_mps,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VspSepMethod {
    /// Classic VSP aligned spatial median filter (flattens downgoing via `FB_PICK`, extracts median, subtracts).
    #[default]
    MedianAlign,
    /// 2D F-K Dip separation (positive `dt/dz` = downgoing, negative `dt/dz` = upgoing).
    FkDip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VspWavefieldOutput {
    #[default]
    Upgoing,
    Downgoing,
    /// Upgoing wavefield shifted by `+FB_PICK` so reflections align flat in two-way time!
    UpgoingTwoWayAligned,
}

/// VSP / DAS Upgoing-Downgoing Wavefield Separation parameters (`vsp_wavefield_sep`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VspWavefieldSepParams {
    #[serde(default)]
    pub method: VspSepMethod,
    #[serde(default)]
    pub output: VspWavefieldOutput,
    /// Spatial median filter length in depth channels (odd, e.g. 7–15, default 9).
    #[serde(default = "default_med_len")]
    pub median_len: usize,
    /// Channel spacing `dz` in meters (for FkDip mode, inferred from `rec_dep` if <= 0).
    #[serde(default)]
    pub dz_m: f32,
    /// Cosine taper half-width around `k = 0` as fraction of Nyquist wavenumber (FkDip mode, default 0.04).
    #[serde(default = "default_fk_tap")]
    pub fk_taper_frac: f32,
    /// Reference alignment time in ms for MedianAlign (default 200 ms).
    #[serde(default = "default_align_t")]
    pub align_time_ms: f32,
}

fn default_med_len() -> usize { 9 }
fn default_fk_tap() -> f32 { 0.04 }
fn default_align_t() -> f32 { 200.0 }

impl Default for VspWavefieldSepParams {
    fn default() -> Self {
        Self {
            method: VspSepMethod::MedianAlign,
            output: VspWavefieldOutput::Upgoing,
            median_len: 9,
            dz_m: 0.0,
            fk_taper_frac: 0.04,
            align_time_ms: 200.0,
        }
    }
}

/// Separate a VSP/DAS gather into `(downgoing, upgoing)` wavefields.
pub fn separate_vsp_wavefields(
    traces: &[Trace],
    params: &VspWavefieldSepParams,
) -> Result<(Vec<Trace>, Vec<Trace>)> {
    if traces.len() < 4 || traces[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Need >= 4 depth channels for VSP separation".into()));
    }
    let nx = traces.len();
    let nt = traces[0].samples.len();

    match params.method {
        VspSepMethod::MedianAlign => {
            // Check if FB_PICK is populated; if not, auto-pick
            let mut work = traces.to_vec();
            let valid_fb = work.iter().filter(|t| t.headers.fb_pick > 0.0).count();
            if valid_fb < nx / 2 {
                pick_first_breaks(&mut work, &FirstBreakPickParams::default(), "vsp_sep_fb");
            }

            // 1. Shift each trace to align downgoing first breaks flat at `align_time_ms`
            let mut aligned = work.clone();
            let shifts_ms: Vec<f32> = aligned
                .iter()
                .map(|tr| params.align_time_ms - tr.headers.fb_pick)
                .collect();
            aligned
                .par_iter_mut()
                .zip(shifts_ms.par_iter())
                .for_each(|(tr, &sh)| {
                    if sh.abs() > 1e-4 {
                        shift_trace_ms(tr, sh);
                    }
                });

            // 2. Spatial sliding median filter across depth channels extracts coherent flat Downgoing wavefield
            let hw = (params.median_len.max(3) | 1) / 2;
            let down_aligned_samples: Vec<Vec<f32>> = (0..nx)
                .into_par_iter()
                .map(|ix| {
                    let lo = ix.saturating_sub(hw);
                    let hi = (ix + hw).min(nx - 1);
                    let win = hi - lo + 1;
                    let mut buf = vec![0.0f32; win];
                    let mut out = vec![0.0f32; nt];
                    for it in 0..nt {
                        for (k, jx) in (lo..=hi).enumerate() {
                            buf[k] = aligned[jx].samples[it];
                        }
                        buf.sort_by(|a, b| a.total_cmp(b));
                        out[it] = buf[win / 2];
                    }
                    out
                })
                .collect();

            // 3. Upgoing = Aligned - Downgoing, then un-shift both back to original time!
            let mut down = work.clone();
            let mut up = work.clone();
            for ix in 0..nx {
                for it in 0..nt {
                    let d = down_aligned_samples[ix][it];
                    down[ix].samples[it] = d;
                    up[ix].samples[it] = aligned[ix].samples[it] - d;
                }
            }
            down.par_iter_mut().zip(shifts_ms.par_iter()).for_each(|(tr, &sh)| {
                if sh.abs() > 1e-4 {
                    shift_trace_ms(tr, -sh);
                }
            });
            up.par_iter_mut().zip(shifts_ms.par_iter()).for_each(|(tr, &sh)| {
                if sh.abs() > 1e-4 {
                    shift_trace_ms(tr, -sh);
                }
            });
            Ok((down, up))
        }
        VspSepMethod::FkDip => {
            let nfft_t = (nt + nt / 4).next_power_of_two().max(64);
            let nfft_x = (nx * 2).next_power_of_two().max(32);
            let mut planner = FftPlanner::<f64>::new();
            let fft_t = planner.plan_fft_forward(nfft_t);
            let ifft_t = planner.plan_fft_inverse(nfft_t);
            let fft_x = planner.plan_fft_forward(nfft_x);
            let ifft_x = planner.plan_fft_inverse(nfft_x);

            // 2D Forward FFT: exp(-i*w*t) in time, exp(-i*k*z) in depth.
            // A downgoing wave f(t - p*z) (p > 0) has spectrum F(w)*delta(k + w*p) -> opposite signs of (w, k)!
            //Wait: let's check sign!
            // sum_z exp(-i*w*(t - p*z)) -> in time FFT gives exp(-i*w*p*z).
            // Then spatial FFT sum_z exp(-i*w*p*z) * exp(-i*k*z) peaks when k = -w*p!
            // Therefore for w > 0:
            //   Downgoing (dt/dz = +p > 0) lives in k < 0 (indices nfft_x/2+1 .. nfft_x)!
            //   Upgoing   (dt/dz = -p < 0) lives in k > 0 (indices 1 .. nfft_x/2)!
            let mut fk = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];
            for ix in 0..nx {
                let row = &mut fk[ix * nfft_t..(ix + 1) * nfft_t];
                for it in 0..nt {
                    row[it] = Complex::new(traces[ix].samples[it] as f64, 0.0);
                }
                fft_t.process(row);
            }
            let mut col = vec![Complex::new(0.0, 0.0); nfft_x];
            for iw in 0..nfft_t {
                for ix in 0..nfft_x { col[ix] = fk[ix * nfft_t + iw]; }
                fft_x.process(&mut col);
                for ix in 0..nfft_x { fk[ix * nfft_t + iw] = col[ix]; }
            }

            let tap_bins = ((params.fk_taper_frac.clamp(0.01, 0.2) as f64) * (nfft_x as f64 * 0.5)).max(2.0);
            let mut fk_down = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];
            let mut fk_up = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];

            for iw in 0..nfft_t {
                let w_sign = if iw == 0 || iw == nfft_t / 2 {
                    0.0
                } else if iw < nfft_t / 2 {
                    1.0
                } else {
                    -1.0
                };
                for ik in 0..nfft_x {
                    let k_signed = if ik <= nfft_x / 2 {
                        ik as f64
                    } else {
                        (ik as isize - nfft_x as isize) as f64
                    };
                    // Product w_sign * k_signed: negative => Downgoing, positive => Upgoing
                    let prod = w_sign * k_signed;
                    let w_up = if prod >= tap_bins {
                        1.0
                    } else if prod <= -tap_bins {
                        0.0
                    } else {
                        let u = (prod + tap_bins) / (2.0 * tap_bins);
                        0.5 * (1.0 - (PI * u).cos())
                    };
                    let v = fk[ik * nfft_t + iw];
                    fk_up[ik * nfft_t + iw] = v * w_up;
                    fk_down[ik * nfft_t + iw] = v * (1.0 - w_up);
                }
            }

            let recon = |mut spec: Vec<Complex<f64>>| -> Vec<Trace> {
                let mut c = vec![Complex::new(0.0, 0.0); nfft_x];
                let inv_x = 1.0 / nfft_x as f64;
                let inv_t = 1.0 / nfft_t as f64;
                for iw in 0..nfft_t {
                    for ix in 0..nfft_x { c[ix] = spec[ix * nfft_t + iw]; }
                    ifft_x.process(&mut c);
                    for ix in 0..nfft_x { spec[ix * nfft_t + iw] = c[ix] * inv_x; }
                }
                let mut out = traces.to_vec();
                for ix in 0..nx {
                    let row = &mut spec[ix * nfft_t..(ix + 1) * nfft_t];
                    ifft_t.process(row);
                    for it in 0..nt {
                        out[ix].samples[it] = (row[it].re * inv_t) as f32;
                    }
                }
                out
            };

            Ok((recon(fk_down), recon(fk_up)))
        }
    }
}

/// Apply VSP wavefield separation (`vsp_wavefield_sep`) returning the requested wavefield.
pub fn apply_vsp_wavefield_sep(
    traces: &[Trace],
    params: &VspWavefieldSepParams,
) -> Result<Vec<Trace>> {
    let (down, mut up) = separate_vsp_wavefields(traces, params)?;
    match params.output {
        VspWavefieldOutput::Downgoing => Ok(down),
        VspWavefieldOutput::Upgoing => Ok(up),
        VspWavefieldOutput::UpgoingTwoWayAligned => {
            up.par_iter_mut().for_each(|tr| {
                if tr.headers.fb_pick > 0.0 {
                    shift_trace_ms(tr, tr.headers.fb_pick);
                }
            });
            Ok(up)
        }
    }
}

/// Deterministic VSP Waveshaping Deconvolution (`vsp_decon`):
/// Designs per-depth inverse operator from the Downgoing wavefield and applies it to the Upgoing wavefield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VspDeconParams {
    /// White noise stabilization percentage in frequency-domain division (default 1.0%).
    #[serde(default = "default_vsp_wn")]
    pub white_noise_pct: f32,
    /// Design window duration after `FB_PICK` on downgoing trace in ms (default 400 ms).
    #[serde(default = "default_vsp_win")]
    pub design_window_ms: f32,
    /// Separation parameters used to extract downgoing & upgoing wavefields.
    #[serde(default)]
    pub sep: VspWavefieldSepParams,
}

fn default_vsp_wn() -> f32 { 1.0 }
fn default_vsp_win() -> f32 { 400.0 }

impl Default for VspDeconParams {
    fn default() -> Self {
        Self {
            white_noise_pct: 1.0,
            design_window_ms: 400.0,
            sep: VspWavefieldSepParams::default(),
        }
    }
}

/// Apply deterministic VSP deconvolution (`vsp_decon`):
/// `U_decon(w) = U(w) * conj(D_win(w)) / (|D_win(w)|^2 + eps)` -> zero-phase waveshaped upgoing gather
/// shifted directly to two-way reflection time!
pub fn apply_vsp_decon(traces: &[Trace], params: &VspDeconParams) -> Result<Vec<Trace>> {
    let (down, mut up) = separate_vsp_wavefields(traces, &params.sep)?;
    let nt = up[0].samples.len();
    let dt_ms = up[0].dt_ms().max(1e-3);
    let nfft = (nt * 2).next_power_of_two().max(64);
    let wn = (params.white_noise_pct.max(0.05) as f64) / 100.0;
    let win_samp = ((params.design_window_ms / dt_ms).round() as usize).clamp(16, nt);

    let mut planner = FftPlanner::<f64>::new();
    let fft_fwd = planner.plan_fft_forward(nfft);
    let fft_inv = planner.plan_fft_inverse(nfft);

    up.par_iter_mut()
        .zip(down.par_iter())
        .for_each(|(u_tr, d_tr)| {
            let mut u_buf = vec![Complex::new(0.0, 0.0); nfft];
            let mut d_buf = vec![Complex::new(0.0, 0.0); nfft];

            let i_fb = if d_tr.headers.fb_pick > 0.0 {
                ((d_tr.headers.fb_pick / dt_ms).round() as usize).min(nt.saturating_sub(1))
            } else {
                0
            };
            let d_lo = i_fb.saturating_sub((30.0 / dt_ms).round() as usize);
            let d_hi = (i_fb + win_samp).min(nt);

            for it in 0..nt {
                u_buf[it] = Complex::new(u_tr.samples[it] as f64, 0.0);
            }
            // Place downgoing window shifted so FB sits at t=0 -> correlation automatically aligns U to 2-way time!
            for it in d_lo..d_hi {
                let shift_idx = (it as isize - i_fb as isize).rem_euclid(nfft as isize) as usize;
                d_buf[shift_idx] = Complex::new(d_tr.samples[it] as f64, 0.0);
            }

            fft_fwd.process(&mut u_buf);
            fft_fwd.process(&mut d_buf);

            let max_p = d_buf.iter().map(|c| c.norm_sqr()).fold(0.0f64, f64::max).max(1e-24);
            let eps = wn * max_p;
            let dt_s = u_tr.dt_s() as f64;
            let fb_s = u_tr.headers.fb_pick.max(0.0) as f64 / 1000.0;
            let dw = 2.0 * PI / (nfft as f64 * dt_s);
            for k in 0..nfft {
                // Zero-phase decon: U(w) * conj(D0(w)) / (|D0(w)|^2 + eps), where D0 is aligned to t=0.
                // Then apply +t_fb time shift exp(-i * w * fb_s) so upgoing reflections align flat at 2-way time (t_u + t_fb)!
                let w = if k <= nfft / 2 {
                    k as f64 * dw
                } else {
                    (k as isize - nfft as isize) as f64 * dw
                };
                let shift_twt = Complex::from_polar(1.0, -w * fb_s);
                let denom = d_buf[k].norm_sqr() + eps;
                u_buf[k] = ((u_buf[k] * d_buf[k].conj()) / denom) * shift_twt;
            }
            fft_inv.process(&mut u_buf);
            let inv_n = 1.0 / nfft as f64;
            for it in 0..nt {
                u_tr.samples[it] = (u_buf[it].re * inv_n) as f32;
            }
        });

    Ok(up)
}

/// VSP Corridor Stack parameters (`vsp_corridor_stack`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VspCorridorStackParams {
    /// Start of corridor after `2 * FB_PICK` (two-way aligned time) in ms (default 10 ms).
    #[serde(default = "default_corr_start")]
    pub corridor_start_ms: f32,
    /// Width of outside corridor after `2 * FB_PICK` in ms (default 150 ms).
    #[serde(default = "default_corr_width")]
    pub corridor_width_ms: f32,
    /// Cosine mute taper at corridor edges in ms (default 15 ms).
    #[serde(default = "default_corr_taper")]
    pub taper_ms: f32,
    /// Repeat stacked trace `repeat_traces` times for easy visual display / well-tie splice (default 5).
    #[serde(default = "default_repeat")]
    pub repeat_traces: usize,
    /// Input traces are already two-way aligned (true) or still in one-way time requiring `+FB_PICK` shift (false).
    #[serde(default)]
    pub already_two_way_aligned: bool,
}

fn default_corr_start() -> f32 { 10.0 }
fn default_corr_width() -> f32 { 150.0 }
fn default_corr_taper() -> f32 { 15.0 }
fn default_repeat() -> usize { 5 }

impl Default for VspCorridorStackParams {
    fn default() -> Self {
        Self {
            corridor_start_ms: 10.0,
            corridor_width_ms: 150.0,
            taper_ms: 15.0,
            repeat_traces: 5,
            already_two_way_aligned: false,
        }
    }
}

/// Compute VSP Outside Corridor Stack (`vsp_corridor_stack`) from an upgoing VSP/DAS wavefield.
pub fn compute_vsp_corridor_stack(
    upgoing: &[Trace],
    params: &VspCorridorStackParams,
) -> Result<Vec<Trace>> {
    if upgoing.is_empty() || upgoing[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Empty traces for VSP corridor stack".into()));
    }
    let dt_us = upgoing[0].dt_us;
    let dt_ms = upgoing[0].dt_ms().max(1e-3);
    let nt = upgoing[0].samples.len();

    let mut acc = vec![0.0f64; nt];
    let mut wsum = vec![0.0f64; nt];
    let max_fb = upgoing.iter().map(|t| t.headers.fb_pick).fold(0.0f32, f32::max);
    let taper = params.taper_ms.max(1.0);

    for tr in upgoing {
        if !tr.is_live() {
            continue;
        }
        let mut aligned = tr.clone();
        let fb = tr.headers.fb_pick;
        if !params.already_two_way_aligned && fb > 0.0 {
            shift_trace_ms(&mut aligned, fb);
        }
        // In two-way time, the first-break line sits at 2 * fb.
        // Outside corridor keeps [2*fb + start, 2*fb + start + width] (and keeps everything deeper than 2*max_fb for deepest channels).
        let t_lo = 2.0 * fb + params.corridor_start_ms;
        let t_hi = if (fb - max_fb).abs() < 20.0 {
            (nt as f32) * dt_ms
        } else {
            t_lo + params.corridor_width_ms.max(20.0)
        };

        for it in 0..nt {
            let t = it as f32 * dt_ms;
            if t < t_lo - taper || t > t_hi + taper {
                continue;
            }
            let w_l = if t >= t_lo {
                1.0
            } else {
                0.5 * (1.0 - (std::f32::consts::PI * ((t - (t_lo - taper)) / taper).clamp(0.0, 1.0)).cos())
            };
            let w_h = if t <= t_hi {
                1.0
            } else {
                0.5 * (1.0 + (std::f32::consts::PI * ((t - t_hi) / taper).clamp(0.0, 1.0)).cos())
            };
            let w = (w_l * w_h) as f64;
            if w > 1e-4 {
                acc[it] += aligned.samples[it] as f64 * w;
                wsum[it] += w;
            }
        }
    }

    let stacked: Vec<f32> = (0..nt)
        .map(|it| {
            if wsum[it] > 1e-4 {
                (acc[it] / wsum[it]) as f32
            } else {
                0.0
            }
        })
        .collect();

    let n_rep = params.repeat_traces.clamp(1, 32);
    let mut out = Vec::with_capacity(n_rep);
    for i in 0..n_rep {
        let mut hdr = TraceHeaders::default();
        hdr.traceno = (i + 1) as i32;
        hdr.seqno = (i + 1) as i32;
        hdr.cdp = (i + 1) as i32;
        hdr.trc_type = 1;
        hdr.tr_fold = upgoing.len() as i32;
        hdr.end_ens = if i + 1 == n_rep { 1 } else { 0 };
        hdr.eoj = if i + 1 == n_rep { 1 } else { 0 };
        let mut tr = Trace::new(dt_us, stacked.clone());
        tr.headers = hdr;
        out.push(tr);
    }
    Ok(out)
}
