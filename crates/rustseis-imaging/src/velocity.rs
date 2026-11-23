use crate::nmo::sinc_interp;
use rayon::prelude::*;
use rustseis_core::{Ensemble, Result, SeismicError, Trace, VelocityFunction};
use serde::{Deserialize, Serialize};

/// Parameters for Velocity Analysis Precompute (`vaprecom71`: Semblance + CVS).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VelocityScanParams {
    pub v_min_mps: f32,
    pub v_max_mps: f32,
    pub dv_mps: f32,
    /// Semblance smoothing gate length in milliseconds (default 24 ms).
    #[serde(default = "default_win_ms")]
    pub window_ms: f32,
    /// Output time step in milliseconds (default 10 ms for compact semblance panels).
    #[serde(default = "default_t_step_ms")]
    pub t_step_ms: f32,
    /// Stretch mute percentage applied during semblance scan (default 50%).
    #[serde(default = "default_scan_stretch")]
    pub stretch_mute_pct: f32,
}

fn default_win_ms() -> f32 {
    24.0
}
fn default_t_step_ms() -> f32 {
    10.0
}
fn default_scan_stretch() -> f32 {
    50.0
}

impl Default for VelocityScanParams {
    fn default() -> Self {
        Self {
            v_min_mps: 1400.0,
            v_max_mps: 4500.0,
            dv_mps: 50.0,
            window_ms: 24.0,
            t_step_ms: 10.0,
            stretch_mute_pct: 50.0,
        }
    }
}

/// 2D Semblance panel `S(t_idx, v_idx)` for Interactive Velocity Analysis (IVA) & MCP auto-picking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemblancePanel {
    pub cdp: i32,
    pub times_ms: Vec<f32>,
    pub velocities_mps: Vec<f32>,
    /// Row-major `[n_times * n_vels]` semblance values in `[0.0, 1.0]`.
    pub values: Vec<f32>,
}

impl SemblancePanel {
    #[inline]
    pub fn get(&self, it: usize, iv: usize) -> f32 {
        self.values[it * self.velocities_mps.len() + iv]
    }

    /// Automatic smooth velocity picker using dynamic programming over semblance ridges
    /// (enables MCP agents to propose initial IVA velocity picks!).
    pub fn auto_pick(&self, max_dv_per_sec: f32, min_semblance: f32, stride_ms: f32) -> VelocityFunction {
        let nt = self.times_ms.len();
        let nv = self.velocities_mps.len();
        if nt == 0 || nv == 0 {
            return VelocityFunction::new(self.cdp, vec![(0.0, 1500.0)]);
        }

        // Dynamic programming maximizing cumulative semblance with velocity-gradient regularization
        let mut dp = vec![f64::NEG_INFINITY; nt * nv];
        let mut back = vec![0usize; nt * nv];
        let dv = if nv > 1 {
            (self.velocities_mps[1] - self.velocities_mps[0]).abs().max(1.0)
        } else {
            50.0
        };

        for iv in 0..nv {
            dp[iv] = self.get(0, iv) as f64;
        }
        for it in 1..nt {
            let dt_s = ((self.times_ms[it] - self.times_ms[it - 1]).abs() / 1000.0).max(1e-4);
            let max_step = ((max_dv_per_sec * dt_s / dv).ceil() as isize).max(1);
            for iv in 0..nv {
                let s = self.get(it, iv) as f64;
                let mut best_score = f64::NEG_INFINITY;
                let mut best_prev = iv;
                let lo = (iv as isize - max_step).max(0) as usize;
                let hi = (iv as isize + max_step).min((nv - 1) as isize) as usize;
                for prev in lo..=hi {
                    // Slight penalty for velocity inversions (decreasing v with depth)
                    let pen = if iv < prev { 0.02 * (prev - iv) as f64 } else { 0.002 * (iv - prev) as f64 };
                    let cand = dp[(it - 1) * nv + prev] + s - pen;
                    if cand > best_score {
                        best_score = cand;
                        best_prev = prev;
                    }
                }
                dp[it * nv + iv] = best_score;
                back[it * nv + iv] = best_prev;
            }
        }

        // Backtrack optimal path
        let mut best_end = 0usize;
        let mut best_val = f64::NEG_INFINITY;
        for iv in 0..nv {
            let v = dp[(nt - 1) * nv + iv];
            if v > best_val {
                best_val = v;
                best_end = iv;
            }
        }
        let mut path = vec![0usize; nt];
        path[nt - 1] = best_end;
        for it in (1..nt).rev() {
            path[it - 1] = back[it * nv + path[it]];
        }

        // Subsample picks at stride_ms where semblance exceeds threshold
        let mut picks = Vec::new();
        let mut last_t = -stride_ms;
        for it in 0..nt {
            let t = self.times_ms[it];
            let v = self.velocities_mps[path[it]];
            let s = self.get(it, path[it]);
            if (it == 0 || t - last_t >= stride_ms) && (s >= min_semblance || it == 0) {
                picks.push((t, v));
                last_t = t;
            }
        }
        VelocityFunction::new(self.cdp, picks)
    }
}

/// Compute Taner-Koehler semblance panel for a CDP gather or supergather (`vaprecom71`).
pub fn compute_semblance_panel(gather: &Ensemble, params: &VelocityScanParams) -> Result<SemblancePanel> {
    if gather.traces.is_empty() {
        return Err(SeismicError::InvalidFormat("Empty gather for semblance".into()));
    }
    if params.dv_mps <= 0.0 || params.v_max_mps < params.v_min_mps {
        return Err(SeismicError::InvalidParameter {
            param: "dv_mps".into(),
            message: "Invalid velocity scan range".into(),
        });
    }

    let dt_s = gather.traces[0].dt_s() as f64;
    let dt_ms = gather.traces[0].dt_ms();
    let duration_ms = gather.traces[0].duration_ms();
    let live_traces: Vec<&Trace> = gather.traces.iter().filter(|t| t.is_live()).collect();

    let mut velocities_mps = Vec::new();
    let mut v = params.v_min_mps;
    while v <= params.v_max_mps + 1e-3 {
        velocities_mps.push(v);
        v += params.dv_mps;
    }

    let mut times_ms = Vec::new();
    let t_step = params.t_step_ms.max(dt_ms);
    let mut t = 0.0f32;
    while t <= duration_ms + 1e-3 {
        times_ms.push(t);
        t += t_step;
    }

    let half_win = ((params.window_ms * 0.5) / dt_ms).round() as isize;
    let stretch_lim = params.stretch_mute_pct as f64 / 100.0;
    let nv = velocities_mps.len();

    let rows: Vec<Vec<f32>> = times_ms
        .par_iter()
        .map(|&t0_ms| {
            let mut row = vec![0.0f32; nv];
            let it_center = (t0_ms / dt_ms).round() as isize;
            for (iv, &vel) in velocities_mps.iter().enumerate() {
                let vel64 = vel as f64;
                let mut num = 0.0f64;
                let mut den = 0.0f64;
                let mut valid_fold_sum = 0usize;
                let mut win_samples = 0usize;

                for k in (it_center - half_win)..=(it_center + half_win) {
                    if k < 0 {
                        continue;
                    }
                    let tk = k as f64 * dt_s;
                    let mut sum_amp = 0.0f64;
                    let mut sum_sq = 0.0f64;
                    let mut m = 0usize;

                    for tr in &live_traces {
                        let x = tr.headers.aoffset as f64;
                        let tx = (tk * tk + (x / vel64) * (x / vel64)).sqrt();
                        if tk > 1e-6 && (tx - tk) / tk > stretch_lim {
                            continue;
                        }
                        let pos = tx / dt_s;
                        if pos >= 0.0 && pos < (tr.samples.len() - 1) as f64 {
                            let val = sinc_interp(&tr.samples, pos) as f64;
                            sum_amp += val;
                            sum_sq += val * val;
                            m += 1;
                        }
                    }
                    if m > 1 {
                        num += sum_amp * sum_amp;
                        den += m as f64 * sum_sq;
                        valid_fold_sum += m;
                        win_samples += 1;
                    }
                }
                row[iv] = if den > 1e-20 && valid_fold_sum > 0 && win_samples > 0 {
                    (num / den).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                };
            }
            row
        })
        .collect();

    let mut values = Vec::with_capacity(times_ms.len() * nv);
    for r in rows {
        values.extend_from_slice(&r);
    }

    Ok(SemblancePanel {
        cdp: gather.key_value,
        times_ms,
        velocities_mps,
        values,
    })
}

/// Constant Velocity Stack (CVS) strips for the 3rd panel of Interactive Velocity Analysis (IVA).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CvsPanel {
    pub cdp: i32,
    pub velocities_mps: Vec<f32>,
    /// One stacked trace per velocity in `velocities_mps`.
    pub stacked_traces: Vec<Trace>,
}

/// Compute Constant Velocity Stack (CVS) traces across a list of trial velocities for a CDP gather/supergather.
pub fn compute_cvs_panel(
    gather: &Ensemble,
    velocities_mps: &[f32],
    stretch_mute_pct: f32,
) -> Result<CvsPanel> {
    if gather.traces.is_empty() || velocities_mps.is_empty() {
        return Err(SeismicError::InvalidFormat("Empty gather or velocity list for CVS".into()));
    }
    let dt_us = gather.traces[0].dt_us;
    let dt_s = gather.traces[0].dt_s() as f64;
    let nt = gather.traces[0].samples.len();
    let stretch_lim = stretch_mute_pct as f64 / 100.0;
    let live: Vec<&Trace> = gather.traces.iter().filter(|t| t.is_live()).collect();

    let stacked_traces: Vec<Trace> = velocities_mps
        .par_iter()
        .map(|&vel| {
            let v64 = vel.max(100.0) as f64;
            let mut out = vec![0.0f32; nt];
            for it in 0..nt {
                let t0 = it as f64 * dt_s;
                let mut sum = 0.0f64;
                let mut fold = 0usize;
                for tr in &live {
                    let x = tr.headers.aoffset as f64;
                    let tx = (t0 * t0 + (x / v64) * (x / v64)).sqrt();
                    if t0 > 1e-6 && (tx - t0) / t0 > stretch_lim {
                        continue;
                    }
                    let pos = tx / dt_s;
                    if pos >= 0.0 && pos < (tr.samples.len() - 1) as f64 {
                        sum += sinc_interp(&tr.samples, pos) as f64;
                        fold += 1;
                    }
                }
                if fold > 0 {
                    out[it] = (sum / fold as f64) as f32;
                }
            }
            let mut tr = Trace::new(dt_us, out);
            tr.headers.cdp = gather.key_value;
            tr.headers.tr_fold = live.len() as i32;
            tr.headers.custom.insert("CVS_VEL".into(), vel as f64);
            tr
        })
        .collect();

    Ok(CvsPanel {
        cdp: gather.key_value,
        velocities_mps: velocities_mps.to_vec(),
        stacked_traces,
    })
}
