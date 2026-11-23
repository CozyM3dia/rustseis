use crate::filter::{BandpassParams, FilterPhase, FilterSpecType, apply_bandpass};
use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, Trace};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Ground Roll / Surface Wave Attenuation (`lfaf` — Low-Frequency Array Forming).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundRollParams {
    /// Maximum frequency of ground roll energy in Hz (default 18.0 Hz).
    #[serde(default = "default_gr_fmax")]
    pub f_max_hz: f32,
    /// Transition roll-off bandwidth above `f_max_hz` in Hz (default 6.0 Hz).
    #[serde(default = "default_gr_taper")]
    pub f_taper_hz: f32,
    /// Apparent velocity upper bound of ground roll in m/s (default 900.0 m/s).
    #[serde(default = "default_gr_vel")]
    pub max_velocity_mps: f32,
    /// Receiver spacing `dx` in meters (used for spatial F-K separation if offsets unavailable).
    #[serde(default = "default_dx")]
    pub dx_m: f32,
    /// Optional offset-cone arrival window: start attenuating after `offset / max_velocity_mps - lead_ms`.
    #[serde(default = "default_lead_ms")]
    pub cone_lead_ms: f32,
    /// Attenuation factor `0.0..1.0` (1.0 = full subtraction of estimated ground roll).
    #[serde(default = "default_one")]
    pub strength: f32,
}

fn default_gr_fmax() -> f32 { 18.0 }
fn default_gr_taper() -> f32 { 6.0 }
fn default_gr_vel() -> f32 { 900.0 }
fn default_dx() -> f32 { 12.5 }
fn default_lead_ms() -> f32 { 60.0 }
fn default_one() -> f32 { 1.0 }

impl Default for GroundRollParams {
    fn default() -> Self {
        Self {
            f_max_hz: 18.0,
            f_taper_hz: 6.0,
            max_velocity_mps: 900.0,
            dx_m: 12.5,
            cone_lead_ms: 60.0,
            strength: 1.0,
        }
    }
}

/// Attenuate dispersive low-frequency ground roll in a shot/receiver gather (`lfaf`):
/// Extracts low-frequency band, isolates slow coherent surface waves (`|f/k| <= max_velocity_mps`)
/// inside the time-offset surface-wave cone, and subtracts them while preserving reflections.
pub fn apply_ground_roll_attenuation(traces: &mut [Trace], params: &GroundRollParams) -> Result<()> {
    if traces.len() < 4 || traces[0].samples.is_empty() {
        return Ok(());
    }
    let nx = traces.len();
    let nt = traces[0].samples.len();
    let dt_s = traces[0].dt_s() as f64;
    let dx = params.dx_m.max(0.1) as f64;

    // 1. Extract low-frequency band containing ground roll
    let mut low_band = traces.to_vec();
    let bp = BandpassParams {
        spec_type: FilterSpecType::Ormsby,
        phase: FilterPhase::Zero,
        freqs: [0.5, 2.0, params.f_max_hz.max(4.0), (params.f_max_hz + params.f_taper_hz.max(2.0))],
        pad_pct: 25.0,
        rnoise_pct: 0.1,
        notch: None,
    };
    apply_bandpass(&mut low_band, &bp)?;

    // 2. Isolate slow apparent-velocity energy in 2D F-K of the low-frequency band
    let nfft_t = (nt + nt / 4).next_power_of_two().max(64);
    let nfft_x = (nx * 2).next_power_of_two().max(32);
    let mut planner = FftPlanner::<f64>::new();
    let fft_t = planner.plan_fft_forward(nfft_t);
    let ifft_t = planner.plan_fft_inverse(nfft_t);
    let fft_x = planner.plan_fft_forward(nfft_x);
    let ifft_x = planner.plan_fft_inverse(nfft_x);

    let mut grid = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];
    for ix in 0..nx {
        let row = &mut grid[ix * nfft_t..(ix + 1) * nfft_t];
        for it in 0..nt {
            row[it] = Complex::new(low_band[ix].samples[it] as f64, 0.0);
        }
        fft_t.process(row);
    }
    let mut col = vec![Complex::new(0.0, 0.0); nfft_x];
    for ifreq in 0..nfft_t {
        for ix in 0..nfft_x {
            col[ix] = grid[ix * nfft_t + ifreq];
        }
        fft_x.process(&mut col);
        for ix in 0..nfft_x {
            grid[ix * nfft_t + ifreq] = col[ix];
        }
    }

    let df = 1.0 / (nfft_t as f64 * dt_s);
    let dk = 1.0 / (nfft_x as f64 * dx);
    let v_cut = params.max_velocity_mps.max(50.0) as f64;
    let v_taper = v_cut * 1.25;

    for ifreq in 0..nfft_t {
        let f = if ifreq <= nfft_t / 2 {
            ifreq as f64 * df
        } else {
            (nfft_t - ifreq) as f64 * df
        };
        for ik in 0..nfft_x {
            let k = if ik <= nfft_x / 2 {
                ik as f64 * dk
            } else {
                (nfft_x - ik) as f64 * dk
            };
            let v_app = if k.abs() < 1e-9 { f64::INFINITY } else { f / k.abs() };
            // Keep ONLY the slow ground roll noise model to subtract!
            let noise_w = if v_app <= v_cut {
                1.0
            } else if v_app >= v_taper {
                0.0
            } else {
                let u = (v_app - v_cut) / (v_taper - v_cut);
                0.5 * (1.0 + (PI * u).cos())
            };
            grid[ik * nfft_t + ifreq] *= noise_w;
        }
    }

    let inv_x = 1.0 / nfft_x as f64;
    for ifreq in 0..nfft_t {
        for ix in 0..nfft_x {
            col[ix] = grid[ix * nfft_t + ifreq];
        }
        ifft_x.process(&mut col);
        for ix in 0..nfft_x {
            grid[ix * nfft_t + ifreq] = col[ix] * inv_x;
        }
    }

    let inv_t = 1.0 / nfft_t as f64;
    let strength = params.strength.clamp(0.0, 1.0) as f64;
    let lead_s = params.cone_lead_ms.max(0.0) as f64 / 1000.0;
    let ramp_s = 0.06f64;

    for ix in 0..nx {
        if !traces[ix].is_live() {
            continue;
        }
        let row = &mut grid[ix * nfft_t..(ix + 1) * nfft_t];
        ifft_t.process(row);
        let aoff = if traces[ix].headers.aoffset > 0.0 {
            traces[ix].headers.aoffset as f64
        } else {
            traces[ix].headers.offset.abs() as f64
        };
        let t_cone = (aoff / v_cut - lead_s).max(0.0);
        for it in 0..nt {
            let t = it as f64 * dt_s;
            let cone_w = if aoff <= 1e-3 || t >= t_cone + ramp_s {
                1.0
            } else if t <= t_cone {
                0.0
            } else {
                0.5 * (1.0 - (PI * (t - t_cone) / ramp_s).cos())
            };
            let gr_est = row[it].re * inv_t;
            traces[ix].samples[it] -= (strength * cone_w * gr_est) as f32;
        }
    }
    Ok(())
}

/// Air Blast Attenuation parameters (`airblast`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirBlastParams {
    /// Speed of sound in air (m/s, default 330.0 m/s).
    #[serde(default = "default_vair")]
    pub velocity_mps: f32,
    /// Zero-offset intercept time in ms (default 0.0 ms).
    #[serde(default)]
    pub t0_ms: f32,
    /// Corridor half-width around `t0 + |offset|/velocity` in ms (default 60.0 ms).
    #[serde(default = "default_air_win")]
    pub half_window_ms: f32,
    /// Envelope anomaly threshold ratio vs background trace RMS (default 2.0).
    #[serde(default = "default_air_thresh")]
    pub threshold_ratio: f32,
    /// Cosine taper length at corridor edges in ms (default 20.0 ms).
    #[serde(default = "default_air_taper")]
    pub taper_ms: f32,
}

fn default_vair() -> f32 { 330.0 }
fn default_air_win() -> f32 { 60.0 }
fn default_air_thresh() -> f32 { 2.0 }
fn default_air_taper() -> f32 { 20.0 }

impl Default for AirBlastParams {
    fn default() -> Self {
        Self {
            velocity_mps: 330.0,
            t0_ms: 0.0,
            half_window_ms: 60.0,
            threshold_ratio: 2.0,
            taper_ms: 20.0,
        }
    }
}

/// Attenuate anomalous acoustic air-wave bursts along `t = t0 + |offset| / v_air` (`airblast`).
pub fn apply_airblast_attenuation(traces: &mut [Trace], params: &AirBlastParams) {
    let v_air = params.velocity_mps.max(100.0);
    let half_w = params.half_window_ms.max(5.0);
    let taper = params.taper_ms.max(2.0);
    let thresh = params.threshold_ratio.max(1.0);

    traces.par_iter_mut().for_each(|tr| {
        if !tr.is_live() || tr.samples.len() < 8 {
            return;
        }
        let dt_ms = tr.dt_ms().max(1e-3);
        let nt = tr.samples.len();
        let aoff = if tr.headers.aoffset > 0.0 {
            tr.headers.aoffset
        } else {
            tr.headers.offset.abs()
        };
        let t_center = params.t0_ms + (aoff / v_air) * 1000.0;
        let t_lo = t_center - half_w;
        let t_hi = t_center + half_w;
        let i_lo = ((t_lo / dt_ms).floor().max(0.0) as usize).min(nt);
        let i_hi = ((t_hi / dt_ms).ceil().max(0.0) as usize).min(nt);
        if i_lo >= i_hi {
            return;
        }

        // Robust background amplitude estimate: median of 30ms sub-window RMS outside the air-blast corridor
        // (immune to adjacent ground roll bursts or strong reflections).
        let sub_len = ((30.0 / dt_ms).round() as usize).max(4);
        let mut bg_windows = Vec::new();
        let mut pos = 0usize;
        while pos + sub_len <= nt {
            let end = pos + sub_len;
            if end <= i_lo || pos >= i_hi {
                let mut sum = 0.0f64;
                for i in pos..end {
                    let v = tr.samples[i] as f64;
                    sum += v * v;
                }
                let r = (sum / sub_len as f64).sqrt() as f32;
                if r > 1e-12 {
                    bg_windows.push(r);
                }
            }
            pos += sub_len;
        }
        let bg_rms = if !bg_windows.is_empty() {
            bg_windows.sort_by(|a, b| a.total_cmp(b));
            // Lower quartile / median avoids ground-roll contamination
            bg_windows[bg_windows.len() / 3].max(1e-6)
        } else {
            1e-6
        };

        // Measure peak local RMS inside the air-blast corridor (using 20 ms sliding gate)
        let loc_hw = ((10.0 / dt_ms).round() as usize).max(2);
        let mut peak_loc_rms = 0.0f32;
        for i in i_lo..i_hi {
            let a = i.saturating_sub(loc_hw).max(i_lo);
            let b = (i + loc_hw + 1).min(i_hi);
            let mut sum = 0.0f64;
            for j in a..b {
                let v = tr.samples[j] as f64;
                sum += v * v;
            }
            let r = (sum / (b - a).max(1) as f64).sqrt() as f32;
            if r > peak_loc_rms {
                peak_loc_rms = r;
            }
        }

        // If anomalous air blast detected, attenuate corridor smoothly down to background level
        if peak_loc_rms > bg_rms * thresh && peak_loc_rms > 1e-12 {
            let target_gain = (bg_rms / peak_loc_rms).clamp(0.0, 1.0);
            for i in i_lo..i_hi {
                let t = i as f32 * dt_ms;
                let dist_edge = (t - t_lo).min(t_hi - t);
                let w = if dist_edge >= taper {
                    1.0
                } else {
                    0.5 * (1.0 - (std::f32::consts::PI * (dist_edge / taper).clamp(0.0, 1.0)).cos())
                };
                let scale = 1.0 - w * (1.0 - target_gain);
                tr.samples[i] *= scale;
            }
        }
    });
}

/// Multiscale Directional Curvelet-Frame Denoising (`curvelet_denoise`, SLB VISTA parity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurveletDenoiseParams {
    /// Number of radial dyadic frequency scales (default 4).
    #[serde(default = "default_n_scales")]
    pub n_scales: usize,
    /// Number of directional wedges at the coarsest directional scale (doubles every 2 scales, default 8).
    #[serde(default = "default_n_angles")]
    pub base_angles: usize,
    /// Soft-threshold multiplier `k * sigma_MAD` (default 2.5; higher = more aggressive denoising).
    #[serde(default = "default_thresh_sigma")]
    pub threshold_sigma: f32,
    /// Preserve coarsest low-frequency scale untouched (default true).
    #[serde(default = "default_true_c")]
    pub preserve_coarse: bool,
}

fn default_n_scales() -> usize { 4 }
fn default_n_angles() -> usize { 8 }
fn default_thresh_sigma() -> f32 { 2.5 }
fn default_true_c() -> bool { true }

impl Default for CurveletDenoiseParams {
    fn default() -> Self {
        Self {
            n_scales: 4,
            base_angles: 8,
            threshold_sigma: 2.5,
            preserve_coarse: true,
        }
    }
}

/// Meyer-style smooth transition window `v(u)` for partition of unity `cos^2 + sin^2 = 1`.
#[inline]
fn meyer_bump(x: f64, lo: f64, hi: f64) -> f64 {
    if x <= lo {
        0.0
    } else if x >= hi {
        1.0
    } else {
        let u = (x - lo) / (hi - lo);
        (0.5 * PI * u).sin()
    }
}

/// Apply Parseval-tight 2D multiscale directional curvelet-frame soft-thresholding denoising.
/// Decomposes the 2D seismic panel into radial dyadic scales × angular directional wedges in F-K,
/// estimates per-subband noise via MAD (`median(|c|) / 0.6745`), soft-thresholds, and reconstructs.
pub fn apply_curvelet_denoise(traces: &mut [Trace], params: &CurveletDenoiseParams) -> Result<()> {
    let nx = traces.len();
    if nx < 4 || traces[0].samples.is_empty() {
        return Ok(());
    }
    let nt = traces[0].samples.len();
    let n_scales = params.n_scales.clamp(2, 6);
    let base_angles = params.base_angles.clamp(4, 32);

    let nfft_t = nt.next_power_of_two().max(64);
    let nfft_x = nx.next_power_of_two().max(16);

    let mut planner = FftPlanner::<f64>::new();
    let fft_t = planner.plan_fft_forward(nfft_t);
    let ifft_t = planner.plan_fft_inverse(nfft_t);
    let fft_x = planner.plan_fft_forward(nfft_x);
    let ifft_x = planner.plan_fft_inverse(nfft_x);

    // 1. 2D Forward FFT of panel
    let mut fk = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];
    for ix in 0..nx {
        let row = &mut fk[ix * nfft_t..(ix + 1) * nfft_t];
        if traces[ix].is_live() {
            for it in 0..nt {
                row[it] = Complex::new(traces[ix].samples[it] as f64, 0.0);
            }
            fft_t.process(row);
        }
    }
    let mut col = vec![Complex::new(0.0, 0.0); nfft_x];
    for it in 0..nfft_t {
        for ix in 0..nfft_x {
            col[ix] = fk[ix * nfft_t + it];
        }
        fft_x.process(&mut col);
        for ix in 0..nfft_x {
            fk[ix * nfft_t + it] = col[ix];
        }
    }

    // Normalized frequency coordinates (u_x, u_t) in [-0.5, 0.5]
    let u_t: Vec<f64> = (0..nfft_t)
        .map(|i| {
            if i <= nfft_t / 2 {
                i as f64 / nfft_t as f64
            } else {
                (i as isize - nfft_t as isize) as f64 / nfft_t as f64
            }
        })
        .collect();
    let u_x: Vec<f64> = (0..nfft_x)
        .map(|i| {
            if i <= nfft_x / 2 {
                i as f64 / nfft_x as f64
            } else {
                (i as isize - nfft_x as isize) as f64 / nfft_x as f64
            }
        })
        .collect();

    // Estimate global fine-scale noise sigma via finest diagonal high-frequency energy
    // (will also compute per-subband MAD in space-time)
    let r_edges: Vec<f64> = (0..=n_scales)
        .map(|s| {
            if s == 0 {
                0.0
            } else {
                0.5 * 2.0f64.powi(-(n_scales as i32 - s as i32))
            }
        })
        .collect();

    // Build list of (scale_idx, angle_idx, n_angles_at_scale) subbands
    let mut subbands: Vec<(usize, usize, usize)> = Vec::new();
    // Scale 0 = isotropic coarse low-pass disk
    subbands.push((0, 0, 1));
    for s in 1..n_scales {
        let n_ang = base_angles * (1usize << ((s - 1) / 2));
        for a in 0..n_ang {
            subbands.push((s, a, n_ang));
        }
    }

    // Precompute normalization sum of squared windows so the frame is EXACTLY Parseval-tight:
    // sum_m |W_m(u)|^2 = 1 for every (ix, it).
    let n_bins = nfft_x * nfft_t;
    let eval_window = |s: usize, a: usize, n_ang: usize, ux: f64, ut: f64| -> f64 {
        let r = ux.hypot(ut);
        let wr = if s == 0 {
            let r_hi = r_edges[1];
            1.0 - meyer_bump(r, r_hi * 0.65, r_hi * 1.35)
        } else {
            let r_lo = r_edges[s];
            let r_hi = r_edges[s + 1];
            let up = meyer_bump(r, r_lo * 0.65, r_lo * 1.35);
            let down = if s + 1 == n_scales {
                1.0
            } else {
                1.0 - meyer_bump(r, r_hi * 0.65, r_hi * 1.35)
            };
            up * down
        };
        if wr <= 1e-6 || s == 0 {
            return wr;
        }
        // Angular wedge centered at theta_c in [-PI, PI] (symmetrized over theta and theta + PI)
        let theta = ut.atan2(ux); // [-PI, PI]
        let theta_mod = if theta < 0.0 { theta + PI } else { theta }; // [0, PI)
        let dtheta = PI / n_ang as f64;
        let center = (a as f64 + 0.5) * dtheta;
        let mut diff = (theta_mod - center).abs();
        if diff > 0.5 * PI {
            diff = PI - diff;
        }
        let wa = if diff >= dtheta {
            0.0
        } else {
            (0.5 * PI * (1.0 - diff / dtheta)).sin()
        };
        wr * wa
    };

    let mut norm_sq = vec![0.0f64; n_bins];
    for &(s, a, n_ang) in &subbands {
        for ix in 0..nfft_x {
            let ux = u_x[ix];
            let row_off = ix * nfft_t;
            for it in 0..nfft_t {
                let w = eval_window(s, a, n_ang, ux, u_t[it]);
                norm_sq[row_off + it] += w * w;
            }
        }
    }
    let inv_norm: Vec<f64> = norm_sq
        .iter()
        .map(|&v| if v > 1e-12 { 1.0 / v.sqrt() } else { 0.0 })
        .collect();

    // Compute finest-scale noise sigma from finest radial shell (robust global noise estimate)
    let global_sigma = {
        let mut fine = vec![Complex::new(0.0, 0.0); n_bins];
        for ix in 0..nfft_x {
            for it in 0..nfft_t {
                let r = u_x[ix].hypot(u_t[it]);
                if r >= r_edges[n_scales - 1] {
                    fine[ix * nfft_t + it] = fk[ix * nfft_t + it];
                }
            }
        }
        let mut cbuf = vec![Complex::new(0.0, 0.0); nfft_x];
        for it in 0..nfft_t {
            for ix in 0..nfft_x { cbuf[ix] = fine[ix * nfft_t + it]; }
            ifft_x.process(&mut cbuf);
            for ix in 0..nfft_x { fine[ix * nfft_t + it] = cbuf[ix] / nfft_x as f64; }
        }
        let mut abs_v = Vec::with_capacity(nx * nt);
        for ix in 0..nx {
            let row = &mut fine[ix * nfft_t..(ix + 1) * nfft_t];
            ifft_t.process(row);
            for it in 0..nt {
                abs_v.push((row[it].re / nfft_t as f64).abs());
            }
        }
        abs_v.sort_by(|a, b| a.total_cmp(b));
        let med = abs_v.get(abs_v.len() / 2).copied().unwrap_or(0.0);
        med / 0.67448975
    };

    let k_thresh = params.threshold_sigma.max(0.0) as f64;
    let preserve_coarse = params.preserve_coarse;
    let inv_nt = 1.0 / nfft_t as f64;
    let inv_nx = 1.0 / nfft_x as f64;

    // Process each directional curvelet subband in parallel and accumulate reconstructed FK spectrum
    let subband_fk_contribs: Vec<Vec<Complex<f64>>> = subbands
        .par_iter()
        .map(|&(s, a, n_ang)| {
            let mut sb = vec![Complex::new(0.0, 0.0); n_bins];
            let mut w_cache = vec![0.0f64; n_bins];
            let mut w_l2_sq = 0.0f64;
            for ix in 0..nfft_x {
                let ux = u_x[ix];
                let off = ix * nfft_t;
                for it in 0..nfft_t {
                    let w = eval_window(s, a, n_ang, ux, u_t[it]) * inv_norm[off + it];
                    w_cache[off + it] = w;
                    w_l2_sq += w * w;
                    sb[off + it] = fk[off + it] * w;
                }
            }

            if s == 0 && preserve_coarse {
                // Reconstruct coarse band directly: W * (W * FK) = W^2 * FK
                for idx in 0..n_bins {
                    sb[idx] *= w_cache[idx];
                }
                return sb;
            }

            // 2D IFFT to space-time subband coefficients
            let mut cbuf = vec![Complex::new(0.0, 0.0); nfft_x];
            for it in 0..nfft_t {
                for ix in 0..nfft_x { cbuf[ix] = sb[ix * nfft_t + it]; }
                ifft_x.process(&mut cbuf);
                for ix in 0..nfft_x { sb[ix * nfft_t + it] = cbuf[ix] * inv_nx; }
            }
            for ix in 0..nfft_x {
                let row = &mut sb[ix * nfft_t..(ix + 1) * nfft_t];
                ifft_t.process(row);
                for v in row.iter_mut() {
                    *v *= inv_nt;
                }
            }

            // Noise standard deviation propagated into this Parseval subband:
            // sigma_sb = sigma_global * sqrt(mean(W^2))
            let subband_gain = (w_l2_sq / n_bins as f64).sqrt();
            // Slightly stronger threshold on finest scale (standard Starck/Candes curvelet rule)
            let scale_boost = if s + 1 == n_scales { 1.15 } else { 1.0 };
            let thr = k_thresh * scale_boost * global_sigma * subband_gain;

            // Soft thresholding in real space-time domain
            for v in sb.iter_mut() {
                let r = v.re;
                let mag = r.abs();
                let shrunk = if mag > thr { r.signum() * (mag - thr) } else { 0.0 };
                *v = Complex::new(shrunk, 0.0);
            }

            // Forward 2D FFT back to F-K and multiply by synthesis window W
            for ix in 0..nfft_x {
                fft_t.process(&mut sb[ix * nfft_t..(ix + 1) * nfft_t]);
            }
            for it in 0..nfft_t {
                for ix in 0..nfft_x { cbuf[ix] = sb[ix * nfft_t + it]; }
                fft_x.process(&mut cbuf);
                for ix in 0..nfft_x {
                    sb[ix * nfft_t + it] = cbuf[ix] * w_cache[ix * nfft_t + it];
                }
            }
            sb
        })
        .collect();

    // Sum all subbands in F-K and perform single 2D IFFT back to (t, x)
    let mut recon_fk = vec![Complex::new(0.0, 0.0); n_bins];
    for contrib in subband_fk_contribs {
        for idx in 0..n_bins {
            recon_fk[idx] += contrib[idx];
        }
    }

    for it in 0..nfft_t {
        for ix in 0..nfft_x { col[ix] = recon_fk[ix * nfft_t + it]; }
        ifft_x.process(&mut col);
        for ix in 0..nfft_x { recon_fk[ix * nfft_t + it] = col[ix] * inv_nx; }
    }
    for ix in 0..nx {
        if !traces[ix].is_live() {
            continue;
        }
        let row = &mut recon_fk[ix * nfft_t..(ix + 1) * nfft_t];
        ifft_t.process(row);
        for it in 0..nt {
            traces[ix].samples[it] = (row[it].re * inv_nt) as f32;
        }
    }

    Ok(())
}
