use rayon::prelude::*;
use rustseis_core::{Result, SeismicError, Trace, TraceHeaders};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Generate a zero-phase Ricker wavelet sampled at `dt_s` around `t_offset_s = t - t_event`.
#[inline]
pub fn ricker_wavelet(t_offset_s: f64, f_dom_hz: f64) -> f64 {
    let a = PI * f_dom_hz * t_offset_s;
    let a2 = a * a;
    if a2 > 25.0 {
        0.0
    } else {
        (1.0 - 2.0 * a2) * (-a2).exp()
    }
}

/// Deterministic Xorshift64 PRNG for reproducible zero-dependency synthetic noise & statics tests.
struct Rng64(u64);
impl Rng64 {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }
    fn normal(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-12);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthReflector {
    /// Two-way zero-offset traveltime at `x = 0` in ms.
    pub t0_ms: f32,
    /// Reflector dip slope `dt0/dx` in ms/km (0.0 = flat horizontal layer).
    #[serde(default)]
    pub dip_ms_per_km: f32,
    /// RMS velocity in m/s.
    pub velocity_mps: f32,
    /// Reflection amplitude (can be negative).
    pub amplitude: f32,
    /// Optional 1st-order surface multiple amplitude factor (e.g. -0.45 for strong water-bottom multiple).
    #[serde(default)]
    pub surface_multiple_amp: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthDiffractor {
    pub x_m: f32,
    pub t0_ms: f32,
    pub velocity_mps: f32,
    pub amplitude: f32,
}

/// 2D Analytic / Kirchhoff Synthetic Seismic Modeler (`synth`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Synth2dParams {
    #[serde(default = "default_n_shots")]
    pub n_shots: usize,
    #[serde(default = "default_n_chans")]
    pub n_chans: usize,
    #[serde(default = "default_shot_dx")]
    pub shot_interval_m: f32,
    #[serde(default = "default_rec_dx")]
    pub receiver_interval_m: f32,
    #[serde(default = "default_near_off")]
    pub near_offset_m: f32,
    /// Split-spread land geometry (true) vs end-on marine streamer (false).
    #[serde(default)]
    pub split_spread: bool,
    #[serde(default = "default_dt_us")]
    pub dt_us: u32,
    #[serde(default = "default_nt")]
    pub n_samples: usize,
    #[serde(default = "default_fdom")]
    pub dominant_freq_hz: f32,
    #[serde(default = "default_reflectors")]
    pub reflectors: Vec<SynthReflector>,
    #[serde(default)]
    pub diffractors: Vec<SynthDiffractor>,
    /// Optional synthetic refracted first break (`v_refr_mps > 0` activates direct + refracted head wave).
    #[serde(default)]
    pub refractor_vel_mps: f32,
    /// Near-surface weathering velocity for refracted delay time (default 600 m/s).
    #[serde(default = "default_vw")]
    pub weathering_vel_mps: f32,
    /// Optional per-station weathering delay amplitude in ms (creates realistic refraction/residual statics!).
    #[serde(default)]
    pub max_static_ms: f32,
    /// Optional random Gaussian noise RMS amplitude.
    #[serde(default)]
    pub noise_rms: f32,
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_n_shots() -> usize { 24 }
fn default_n_chans() -> usize { 32 }
fn default_shot_dx() -> f32 { 25.0 }
fn default_rec_dx() -> f32 { 25.0 }
fn default_near_off() -> f32 { 50.0 }
fn default_dt_us() -> u32 { 2000 }
fn default_nt() -> usize { 1001 }
fn default_fdom() -> f32 { 30.0 }
fn default_vw() -> f32 { 600.0 }
fn default_seed() -> u64 { 2026 }
fn default_reflectors() -> Vec<SynthReflector> {
    vec![
        SynthReflector {
            t0_ms: 400.0,
            dip_ms_per_km: 0.0,
            velocity_mps: 1500.0,
            amplitude: 1.0,
            surface_multiple_amp: -0.45,
        },
        SynthReflector {
            t0_ms: 850.0,
            dip_ms_per_km: 80.0,
            velocity_mps: 2100.0,
            amplitude: 0.85,
            surface_multiple_amp: 0.0,
        },
    ]
}

impl Default for Synth2dParams {
    fn default() -> Self {
        Self {
            n_shots: 24,
            n_chans: 32,
            shot_interval_m: 25.0,
            receiver_interval_m: 25.0,
            near_offset_m: 50.0,
            split_spread: false,
            dt_us: 2000,
            n_samples: 1001,
            dominant_freq_hz: 30.0,
            reflectors: default_reflectors(),
            diffractors: Vec::new(),
            refractor_vel_mps: 0.0,
            weathering_vel_mps: 600.0,
            max_static_ms: 0.0,
            noise_rms: 0.0,
            seed: 2026,
        }
    }
}

/// Generate a complete 2D synthetic seismic dataset (`synth`) with exact geometry headers,
/// reflectors (including dipping events via exact Levin/DSR moveout), diffractors, multiples,
/// optional weathering statics, refracted first breaks, and reproducible noise.
pub fn generate_synthetic_2d(params: &Synth2dParams) -> Result<Vec<Trace>> {
    if params.n_shots == 0 || params.n_chans == 0 || params.n_samples < 16 {
        return Err(SeismicError::InvalidParameter {
            param: "synth".into(),
            message: "n_shots, n_chans, and n_samples must be positive".into(),
        });
    }
    let dt_s = params.dt_us as f64 * 1e-6;
    let nt = params.n_samples;
    let f_dom = params.dominant_freq_hz.max(5.0) as f64;
    let dx_r = params.receiver_interval_m.max(1.0) as f64;
    let dx_s = params.shot_interval_m.max(1.0) as f64;
    let cdp_bin = 0.5 * dx_r;

    // Deterministic per-station static delay (ms) if max_static_ms > 0
    let station_delay_ms = |sloc: i32| -> f64 {
        if params.max_static_ms.abs() <= 1e-6 {
            return 0.0;
        }
        let mut rng = Rng64::new(params.seed.wrapping_add((sloc as i64 as u64).wrapping_mul(0x9E3779B97F4A7C15)));
        (rng.next_f64() * 2.0 - 1.0) * params.max_static_ms as f64
    };

    let mut traces = Vec::with_capacity(params.n_shots * params.n_chans);
    let mut tr_id = 0i32;

    for ishot in 0..params.n_shots {
        let ffid = (ishot + 1) as i32;
        let sx = ishot as f64 * dx_s;
        let sou_sloc = (sx / dx_r).round() as i32 + 100;
        let s_delay_s = station_delay_ms(sou_sloc) / 1000.0;

        for ichan in 0..params.n_chans {
            tr_id += 1;
            let offset = if params.split_spread {
                let half = (params.n_chans as f64 - 1.0) * 0.5;
                (ichan as f64 - half) * dx_r
            } else {
                params.near_offset_m as f64 + ichan as f64 * dx_r
            };
            let rx = sx + offset;
            let rec_sloc = (rx / dx_r).round() as i32 + 100;
            let r_delay_s = station_delay_ms(rec_sloc) / 1000.0;
            let tot_delay_s = s_delay_s + r_delay_s;

            let mx = 0.5 * (sx + rx);
            let aoff = offset.abs();

            let mut samps = vec![0.0f64; nt];

            // 1. Planar (flat or dipping) reflectors with exact Levin dipping-bed moveout:
            //    t(x_m, h) = sqrt(t0(x_m)^2 + (2*h*cos(dip)/V)^2)
            for refl in &params.reflectors {
                let v = refl.velocity_mps.max(200.0) as f64;
                let slope_s_per_m = (refl.dip_ms_per_km as f64) * 1e-6; // dt0/dx = 2*sin(theta)/v
                let sin_dip = (0.5 * v * slope_s_per_m).clamp(-0.85, 0.85);
                let cos_dip = (1.0 - sin_dip * sin_dip).sqrt();
                let t0_m = (refl.t0_ms as f64 / 1000.0) + slope_s_per_m * mx;
                if t0_m > 0.01 {
                    let t_arr = (t0_m * t0_m + (aoff * cos_dip / v).powi(2)).sqrt() + tot_delay_s;
                    let amp = refl.amplitude as f64;
                    let i_c = (t_arr / dt_s).round() as isize;
                    let hw = ((0.12 / dt_s).ceil() as isize).max(20);
                    let a0 = (i_c - hw).max(0) as usize;
                    let a1 = (i_c + hw).min((nt - 1) as isize) as usize;
                    for it in a0..=a1 {
                        samps[it] += amp * ricker_wavelet(it as f64 * dt_s - t_arr, f_dom);
                    }

                    // Optional 1st-order surface multiple at 2*t0
                    if refl.surface_multiple_amp.abs() > 1e-6 {
                        let t0_mult = 2.0 * t0_m;
                        let t_mult = (t0_mult * t0_mult + (aoff * cos_dip / v).powi(2)).sqrt() + 2.0 * tot_delay_s;
                        let m_amp = refl.surface_multiple_amp as f64;
                        let im_c = (t_mult / dt_s).round() as isize;
                        let m0 = (im_c - hw).max(0) as usize;
                        let m1 = (im_c + hw).min((nt - 1) as isize) as usize;
                        for it in m0..=m1 {
                            samps[it] += m_amp * ricker_wavelet(it as f64 * dt_s - t_mult, f_dom);
                        }
                    }
                }
            }

            // 2. Point diffractors via exact DSR traveltime
            for diff in &params.diffractors {
                let v = diff.velocity_mps.max(200.0) as f64;
                let half_t0 = 0.5 * (diff.t0_ms as f64 / 1000.0);
                let ts = (half_t0 * half_t0 + ((diff.x_m as f64 - sx) / v).powi(2)).sqrt();
                let tr_t = (half_t0 * half_t0 + ((diff.x_m as f64 - rx) / v).powi(2)).sqrt();
                let t_arr = ts + tr_t + tot_delay_s;
                let amp = diff.amplitude as f64;
                let i_c = (t_arr / dt_s).round() as isize;
                let hw = ((0.12 / dt_s).ceil() as isize).max(20);
                let a0 = (i_c - hw).max(0) as usize;
                let a1 = (i_c + hw).min((nt - 1) as isize) as usize;
                for it in a0..=a1 {
                    samps[it] += amp * ricker_wavelet(it as f64 * dt_s - t_arr, f_dom);
                }
            }

            // 3. Optional refracted first-break head wave (for fbpick + refstatic validation!)
            if params.refractor_vel_mps > 300.0 && aoff > 1.0 {
                let v1 = params.refractor_vel_mps as f64;
                let v0 = params.weathering_vel_mps.max(150.0).min(params.refractor_vel_mps * 0.8) as f64;
                let cos_ic = (1.0 - (v0 / v1).powi(2)).sqrt();
                // Base weathering thickness 15 m + station perturbation matching tot_delay_s
                let base_delay = 2.0 * (15.0 * cos_ic / v0);
                let t_fb = (base_delay + tot_delay_s + aoff / v1).max(0.01);
                // Minimum-phase causal onset pulse at t_fb
                let i0 = (t_fb / dt_s).floor().max(0.0) as usize;
                let dur = (0.08 / dt_s).ceil() as usize;
                for it in i0..(i0 + dur).min(nt) {
                    let tau = it as f64 * dt_s - t_fb;
                    if tau >= 0.0 {
                        let env = (tau * 80.0).min(1.0) * (-tau * 45.0).exp();
                        samps[it] += 1.4 * env * (2.0 * PI * f_dom * tau).sin();
                    }
                }
            }

            // 4. Optional deterministic Gaussian noise
            if params.noise_rms > 0.0 {
                let mut rng = Rng64::new(params.seed.wrapping_add((tr_id as u64).wrapping_mul(6364136223846793005)));
                let sigma = params.noise_rms as f64;
                for s in &mut samps {
                    *s += sigma * rng.normal();
                }
            }

            let mut hdr = TraceHeaders::default();
            hdr.traceno = tr_id;
            hdr.ffid = ffid;
            hdr.sin = ffid;
            hdr.source = ffid;
            hdr.chan = (ichan + 1) as i32;
            hdr.nchans = params.n_chans as i32;
            hdr.seqno = (ichan + 1) as i32;
            hdr.end_ens = if ichan + 1 == params.n_chans { 1 } else { 0 };
            hdr.sou_sloc = sou_sloc;
            hdr.rec_sloc = rec_sloc;
            hdr.sou_x = sx;
            hdr.rec_x = rx;
            hdr.cdp_x = mx;
            hdr.offset = offset as f32;
            hdr.aoffset = aoff as f32;
            let cdp = ((mx / cdp_bin).round() as i32) + 1;
            hdr.cdp = cdp;
            hdr.cdp_sloc = cdp;
            hdr.trc_type = 1;
            hdr.tlive_e = (nt - 1) as f32 * (params.dt_us as f32 / 1000.0);
            hdr.tfull_e = hdr.tlive_e;

            let mut tr = Trace::new(
                params.dt_us,
                samps.into_iter().map(|v| v as f32).collect(),
            );
            tr.headers = hdr;
            traces.push(tr);
        }
    }
    if let Some(last) = traces.last_mut() {
        last.headers.eoj = 1;
    }
    Ok(traces)
}

/// 2D Acoustic Finite-Difference Wave-Equation Modeler (`afd`).
/// Solves `d^2p/dt^2 = v(x, z)^2 * (d^2p/dx^2 + d^2p/dz^2) + s(t)` with 4th-order spatial stencil,
/// automatic CFL sub-stepping, sponge absorbing boundaries, and optional free surface at `z = 0`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Afd2dParams {
    #[serde(default = "default_afd_nx")]
    pub nx: usize,
    #[serde(default = "default_afd_nz")]
    pub nz: usize,
    #[serde(default = "default_afd_dh")]
    pub dh_m: f32,
    /// Layered velocity model `(depth_top_m, velocity_mps)` (sorted by depth).
    #[serde(default = "default_afd_layers")]
    pub layers: Vec<(f32, f32)>,
    /// Shot X positions in meters.
    #[serde(default = "default_afd_shots")]
    pub shot_xs_m: Vec<f32>,
    /// Receiver X range `(start_m, step_m, count)`.
    #[serde(default = "default_afd_recs")]
    pub receiver_start_m: f32,
    #[serde(default = "default_afd_rec_step")]
    pub receiver_step_m: f32,
    #[serde(default = "default_afd_nrec")]
    pub n_receivers: usize,
    /// Output sample interval in microseconds (default 2000 us = 2 ms).
    #[serde(default = "default_dt_us")]
    pub dt_us: u32,
    /// Total recording time in ms (default 600 ms).
    #[serde(default = "default_afd_tmax")]
    pub max_time_ms: f32,
    #[serde(default = "default_afd_fdom")]
    pub dominant_freq_hz: f32,
    /// Free surface at z=0 (true = generates physical surface multiples, false = absorbing top).
    #[serde(default)]
    pub free_surface: bool,
    /// Sponge absorbing boundary width in grid cells (default 20).
    #[serde(default = "default_sponge")]
    pub sponge_cells: usize,
}

fn default_afd_nx() -> usize { 140 }
fn default_afd_nz() -> usize { 100 }
fn default_afd_dh() -> f32 { 10.0 }
fn default_afd_layers() -> Vec<(f32, f32)> { vec![(0.0, 1500.0), (250.0, 2200.0), (500.0, 3000.0)] }
fn default_afd_shots() -> Vec<f32> { vec![700.0] }
fn default_afd_recs() -> f32 { 200.0 }
fn default_afd_rec_step() -> f32 { 20.0 }
fn default_afd_nrec() -> usize { 48 }
fn default_afd_tmax() -> f32 { 600.0 }
fn default_afd_fdom() -> f32 { 25.0 }
fn default_sponge() -> usize { 20 }

impl Default for Afd2dParams {
    fn default() -> Self {
        Self {
            nx: 140,
            nz: 100,
            dh_m: 10.0,
            layers: default_afd_layers(),
            shot_xs_m: default_afd_shots(),
            receiver_start_m: 200.0,
            receiver_step_m: 20.0,
            n_receivers: 48,
            dt_us: 2000,
            max_time_ms: 600.0,
            dominant_freq_hz: 25.0,
            free_surface: false,
            sponge_cells: 20,
        }
    }
}

/// Run 2D Acoustic Finite-Difference Modeling (`afd`) for all shots in parallel via `rayon`.
pub fn run_afd_2d(params: &Afd2dParams) -> Result<Vec<Trace>> {
    if params.nx < 32 || params.nz < 32 || params.shot_xs_m.is_empty() || params.n_receivers == 0 {
        return Err(SeismicError::InvalidParameter {
            param: "afd".into(),
            message: "AFD grid must be >= 32x32 with >= 1 shot and receiver".into(),
        });
    }
    let pad = params.sponge_cells.clamp(8, 40);
    let nx_tot = params.nx + 2 * pad;
    let nz_top_pad = if params.free_surface { 0 } else { pad };
    let nz_tot = params.nz + nz_top_pad + pad;
    let dh = params.dh_m.max(1.0) as f64;

    // Build 2D velocity grid v[iz * nx_tot + ix]
    let mut layers = params.layers.clone();
    if layers.is_empty() {
        layers.push((0.0, 2000.0));
    }
    layers.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut v_grid = vec![layers[0].1 as f64; nz_tot * nx_tot];
    let mut v_max = 100.0f64;
    for iz in 0..nz_tot {
        let z_m = ((iz as isize - nz_top_pad as isize).max(0) as f64) * dh;
        let mut vel = layers[0].1 as f64;
        for &(z_top, v_l) in &layers {
            if z_m >= z_top as f64 {
                vel = v_l as f64;
            }
        }
        v_max = v_max.max(vel);
        for ix in 0..nx_tot {
            v_grid[iz * nx_tot + ix] = vel;
        }
    }

    // Automatic CFL stability sub-stepping (4th-order 2D FD stability: c_max * dt / dh <= 0.49)
    let out_dt = params.dt_us as f64 * 1e-6;
    let cfl_dt = 0.42 * dh / v_max;
    let substeps = (out_dt / cfl_dt).ceil().max(1.0) as usize;
    let dt_sim = out_dt / substeps as f64;
    let nt_out = ((params.max_time_ms as f64 / 1000.0) / out_dt).round() as usize + 1;
    let nt_sim = nt_out * substeps;

    // Precompute (v * dt_sim / dh)^2
    let coeff2: Vec<f64> = v_grid.iter().map(|&v| (v * dt_sim / dh).powi(2)).collect();

    // Cerjan sponge damping factor (1.0 inside, exponential decay inside pad margins)
    let mut damp = vec![1.0f64; nz_tot * nx_tot];
    let alpha_sp = 0.015f64;
    for iz in 0..nz_tot {
        for ix in 0..nx_tot {
            let mut d = 0usize;
            if ix < pad { d = d.max(pad - ix); }
            if ix >= nx_tot - pad { d = d.max(ix - (nx_tot - pad) + 1); }
            if !params.free_surface && iz < nz_top_pad { d = d.max(nz_top_pad - iz); }
            if iz >= nz_tot - pad { d = d.max(iz - (nz_tot - pad) + 1); }
            if d > 0 {
                let df = d as f64;
                damp[iz * nx_tot + ix] = (-(alpha_sp * df * alpha_sp * df)).exp();
            }
        }
    }

    let f_dom = params.dominant_freq_hz.max(5.0) as f64;
    let t_delay = 1.2 / f_dom;
    let rec_ixs: Vec<(usize, f64)> = (0..params.n_receivers)
        .map(|ir| {
            let rx = params.receiver_start_m as f64 + ir as f64 * params.receiver_step_m as f64;
            let ix = ((rx / dh).round() as usize).min(params.nx - 1) + pad;
            (ix, rx)
        })
        .collect();
    let rec_iz = nz_top_pad + 2;

    // Propagate each shot in parallel!
    let shot_gathers: Vec<Vec<Trace>> = params
        .shot_xs_m
        .par_iter()
        .enumerate()
        .map(|(ishot, &sx_f)| {
            let sx = sx_f as f64;
            let src_ix = ((sx / dh).round() as usize).min(params.nx - 1) + pad;
            let src_iz = nz_top_pad + 2;

            let mut p_prev = vec![0.0f64; nz_tot * nx_tot];
            let mut p_curr = vec![0.0f64; nz_tot * nx_tot];
            let mut p_next = vec![0.0f64; nz_tot * nx_tot];
            let mut rec_traces = vec![vec![0.0f32; nt_out]; params.n_receivers];

            // 4th-order spatial Laplacian weights: (-1/12, 4/3, -5/2, 4/3, -1/12)
            const C0: f64 = -2.5;
            const C1: f64 = 4.0 / 3.0;
            const C2: f64 = -1.0 / 12.0;

            for it_sim in 0..nt_sim {
                if it_sim % substeps == 0 {
                    let i_out = it_sim / substeps;
                    if i_out < nt_out {
                        for (ir, &(rix, _)) in rec_ixs.iter().enumerate() {
                            rec_traces[ir][i_out] = p_curr[rec_iz * nx_tot + rix] as f32;
                        }
                    }
                }

                let t_s = it_sim as f64 * dt_sim;
                let src_val = ricker_wavelet(t_s - t_delay, f_dom);

                for iz in 2..(nz_tot - 2) {
                    let row = iz * nx_tot;
                    let row_m1 = (iz - 1) * nx_tot;
                    let row_m2 = (iz - 2) * nx_tot;
                    let row_p1 = (iz + 1) * nx_tot;
                    let row_p2 = (iz + 2) * nx_tot;
                    for ix in 2..(nx_tot - 2) {
                        let idx = row + ix;
                        let lap = C0 * 2.0 * p_curr[idx]
                            + C1 * (p_curr[idx - 1] + p_curr[idx + 1] + p_curr[row_m1 + ix] + p_curr[row_p1 + ix])
                            + C2 * (p_curr[idx - 2] + p_curr[idx + 2] + p_curr[row_m2 + ix] + p_curr[row_p2 + ix]);
                        let mut val = 2.0 * p_curr[idx] - p_prev[idx] + coeff2[idx] * lap;
                        val *= damp[idx];
                        p_next[idx] = val;
                    }
                }
                // Inject Ricker point source
                p_next[src_iz * nx_tot + src_ix] += src_val;

                // Free-surface Dirichlet condition p(z=0) = 0 if enabled
                if params.free_surface {
                    for ix in 0..nx_tot {
                        p_next[ix] = 0.0;
                    }
                }

                std::mem::swap(&mut p_prev, &mut p_curr);
                std::mem::swap(&mut p_curr, &mut p_next);
            }

            let ffid = (ishot + 1) as i32;
            let cdp_bin = (0.5 * params.receiver_step_m.max(1.0)) as f64;
            rec_traces
                .into_iter()
                .enumerate()
                .map(|(ir, samps)| {
                    let rx = rec_ixs[ir].1;
                    let mx = 0.5 * (sx + rx);
                    let off = rx - sx;
                    let mut hdr = TraceHeaders::default();
                    hdr.ffid = ffid;
                    hdr.sin = ffid;
                    hdr.chan = (ir + 1) as i32;
                    hdr.nchans = params.n_receivers as i32;
                    hdr.seqno = (ir + 1) as i32;
                    hdr.end_ens = if ir + 1 == params.n_receivers { 1 } else { 0 };
                    hdr.sou_x = sx;
                    hdr.rec_x = rx;
                    hdr.cdp_x = mx;
                    hdr.offset = off as f32;
                    hdr.aoffset = off.abs() as f32;
                    hdr.cdp = (mx / cdp_bin).round() as i32 + 1;
                    hdr.cdp_sloc = hdr.cdp;
                    hdr.trc_type = 1;
                    let mut tr = Trace::new(params.dt_us, samps);
                    tr.headers = hdr;
                    tr
                })
                .collect()
        })
        .collect();

    let mut out: Vec<Trace> = shot_gathers.into_iter().flatten().collect();
    for (i, tr) in out.iter_mut().enumerate() {
        tr.headers.traceno = (i + 1) as i32;
    }
    if let Some(last) = out.last_mut() {
        last.headers.eoj = 1;
    }
    Ok(out)
}
