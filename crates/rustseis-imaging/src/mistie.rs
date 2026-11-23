//! 2D Multi-Line Mistie Analysis & Global Least-Squares Balancing (`mistie`).
//!
//! Automatically detects 2D line intersections by `(CDP_X, CDP_Y)` proximity, measures time shift
//! (`dt_ms`), constant phase rotation (`phase_deg`), and amplitude scale (`gain_ratio`) via
//! analytic-signal cross-correlation, solves the global damped least-squares mistie network
//! (optionally pinning a reference master line), and applies balanced corrections to all lines.

use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::f64::consts::PI;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistieParams {
    /// Header key identifying each 2D survey line (default `"ILINE_NO"`).
    #[serde(default = "default_line_key")]
    pub line_key: String,
    /// Maximum Euclidean distance (m) between closest traces on two lines to count as an intersection.
    #[serde(default = "default_max_dist")]
    pub max_intersection_dist_m: f64,
    /// Half-width (in traces) averaged around each intersection trace to suppress random noise.
    #[serde(default = "default_half_win")]
    pub stack_half_width: usize,
    /// Analysis time window start (ms).
    #[serde(default = "default_t_start")]
    pub t_start_ms: f32,
    /// Analysis time window end (ms; `0.0` = full trace length).
    #[serde(default)]
    pub t_end_ms: f32,
    /// Maximum allowed time mistie lag (ms).
    #[serde(default = "default_max_lag")]
    pub max_lag_ms: f32,
    /// Minimum normalized analytic correlation required to include an intersection in the solve.
    #[serde(default = "default_min_corr")]
    pub min_correlation: f32,
    /// Optional master reference line ID pinned to zero time/phase/gain correction.
    #[serde(default)]
    pub reference_line: Option<i32>,
    /// Apply solved time shifts to traces.
    #[serde(default = "default_true")]
    pub apply_time: bool,
    /// Apply solved constant phase rotations to traces.
    #[serde(default = "default_true")]
    pub apply_phase: bool,
    /// Apply solved amplitude balancing gain to traces.
    #[serde(default = "default_true")]
    pub apply_gain: bool,
}

fn default_line_key() -> String { "ILINE_NO".into() }
fn default_max_dist() -> f64 { 50.0 }
fn default_half_win() -> usize { 1 }
fn default_t_start() -> f32 { 100.0 }
fn default_max_lag() -> f32 { 80.0 }
fn default_min_corr() -> f32 { 0.30 }
fn default_true() -> bool { true }

impl Default for MistieParams {
    fn default() -> Self {
        Self {
            line_key: default_line_key(),
            max_intersection_dist_m: default_max_dist(),
            stack_half_width: default_half_win(),
            t_start_ms: default_t_start(),
            t_end_ms: 0.0,
            max_lag_ms: default_max_lag(),
            min_correlation: default_min_corr(),
            reference_line: None,
            apply_time: true,
            apply_phase: true,
            apply_gain: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistieIntersection {
    pub line_a: i32,
    pub line_b: i32,
    pub distance_m: f64,
    /// Measured shift `t_b - t_a` (ms): positive means line B arrives later than line A.
    pub dt_ms: f32,
    /// Measured constant phase `phi_b - phi_a` (deg).
    pub phase_deg: f32,
    /// Measured RMS ratio `rms_b / rms_a`.
    pub gain_ratio: f32,
    pub correlation: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineMistieCorrection {
    pub line_id: i32,
    /// Correction applied to line traces (ms; added to trace time).
    pub dt_shift_ms: f32,
    /// Phase rotation applied to line traces (deg).
    pub phase_rot_deg: f32,
    /// Multiplicative gain factor applied to line traces.
    pub gain_scale: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MistieReport {
    pub intersections: Vec<MistieIntersection>,
    pub corrections: Vec<LineMistieCorrection>,
    pub rms_dt_before_ms: f32,
    pub rms_dt_after_ms: f32,
    pub rms_phase_before_deg: f32,
    pub rms_phase_after_deg: f32,
}

/// Measure `(dt_ms, phase_deg, gain_ratio, correlation)` of trace `b` relative to trace `a`
/// using the analytic-signal complex cross-correlation `C(tau) = IFFT(A*(f) * B(f))` (f >= 0).
pub fn measure_pair_mistie(a: &[f32], b: &[f32], dt_s: f64, max_lag_ms: f32) -> (f32, f32, f32, f32) {
    let n = a.len().min(b.len());
    if n < 8 {
        return (0.0, 0.0, 1.0, 0.0);
    }
    let rms_a = (a[..n].iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
    let rms_b = (b[..n].iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
    if rms_a < 1e-12 || rms_b < 1e-12 {
        return (0.0, 0.0, 1.0, 0.0);
    }
    let gain_ratio = (rms_b / rms_a) as f32;

    let nfft = (n * 2).next_power_of_two().max(64);
    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(nfft);
    let inv = planner.plan_fft_inverse(nfft);

    let mut fa = vec![Complex::new(0.0, 0.0); nfft];
    let mut fb = vec![Complex::new(0.0, 0.0); nfft];
    for i in 0..n {
        fa[i] = Complex::new(a[i] as f64, 0.0);
        fb[i] = Complex::new(b[i] as f64, 0.0);
    }
    fwd.process(&mut fa);
    fwd.process(&mut fb);

    // Analytic cross-spectrum (positive frequencies doubled, negative frequencies zeroed)
    let mut xc = vec![Complex::new(0.0, 0.0); nfft];
    xc[0] = fa[0].conj() * fb[0];
    let half = nfft / 2;
    for k in 1..half {
        xc[k] = fa[k].conj() * fb[k] * 2.0;
    }
    xc[half] = fa[half].conj() * fb[half];
    inv.process(&mut xc);

    let norm = (nfft as f64) * (n as f64) * rms_a * rms_b;
    let max_lag = ((max_lag_ms as f64 * 1e-3 / dt_s).ceil() as isize).clamp(1, (n / 2) as isize);

    let mut best_lag = 0isize;
    let mut best_env = -1.0f64;
    let mut best_val = Complex::new(0.0, 0.0);

    for lag in -max_lag..=max_lag {
        let idx = ((lag + nfft as isize) as usize) % nfft;
        let val = xc[idx];
        let env = val.norm();
        if env > best_env {
            best_env = env;
            best_lag = lag;
            best_val = val;
        }
    }

    // Parabolic sub-sample refinement of envelope peak
    let idx_m = (((best_lag - 1) + nfft as isize) as usize) % nfft;
    let idx_p = (((best_lag + 1) + nfft as isize) as usize) % nfft;
    let em = xc[idx_m].norm();
    let e0 = best_env;
    let ep = xc[idx_p].norm();
    let denom = em - 2.0 * e0 + ep;
    let frac = if denom.abs() > 1e-12 {
        (0.5 * (em - ep) / denom).clamp(-0.5, 0.5)
    } else {
        0.0
    };

    // Convention: `xc[lag] = sum_t a[t] * b[t + lag]`; if b(t) = a(t - dt), peak is at lag = +dt.
    let dt_ms = ((best_lag as f64 + frac) * dt_s * 1000.0) as f32;
    // If b = rot(a, +phi) where positive-freq factor is e^{-i*phi}, then A*B has angle -phi -> phi = -arg(C).
    let phase_deg = (-best_val.arg().to_degrees()) as f32;
    let corr = (best_env / norm.max(1e-12)).clamp(0.0, 1.0) as f32;
    (dt_ms, phase_deg, gain_ratio, corr)
}

/// Solve `A x = b` via Gaussian elimination with partial pivoting.
fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for i in 0..n {
        let mut piv = i;
        let mut max_v = a[i][i].abs();
        for r in (i + 1)..n {
            if a[r][i].abs() > max_v {
                max_v = a[r][i].abs();
                piv = r;
            }
        }
        if max_v < 1e-14 {
            continue;
        }
        if piv != i {
            a.swap(i, piv);
            b.swap(i, piv);
        }
        let diag = a[i][i];
        for r in (i + 1)..n {
            let f = a[r][i] / diag;
            if f.abs() < 1e-15 {
                continue;
            }
            for c in i..n {
                a[r][c] -= f * a[i][c];
            }
            b[r] -= f * b[i];
        }
    }
    let mut x = vec![0.0f64; n];
    for i in (0..n).rev() {
        let mut sum = b[i];
        for c in (i + 1)..n {
            sum -= a[i][c] * x[c];
        }
        x[i] = if a[i][i].abs() > 1e-14 { sum / a[i][i] } else { 0.0 };
    }
    x
}

/// Solve global damped least-squares mistie scalar field:
/// For each weighted pair `(i, j, w, obs = val_j - val_i)`, find corrections `c_k` minimizing
/// `sum w * ((val_j + c_j) - (val_i + c_i))^2 = sum w * (c_j - c_i + obs)^2`.
fn solve_mistie_network(
    n_lines: usize,
    obs: &[(usize, usize, f64, f64)],
    ref_idx: Option<usize>,
) -> Vec<f64> {
    let mut mat = vec![vec![0.0f64; n_lines]; n_lines];
    let mut rhs = vec![0.0f64; n_lines];

    for &(i, j, w, d) in obs {
        // Minimize w * (c_j - c_i + d)^2
        mat[i][i] += w;
        mat[j][j] += w;
        mat[i][j] -= w;
        mat[j][i] -= w;
        rhs[i] += w * d;
        rhs[j] -= w * d;
    }

    // Small Tikhonov damping + reference line pin (or zero-sum gauge constraint).
    let damp = 1e-6;
    for i in 0..n_lines {
        mat[i][i] += damp;
    }
    if let Some(ri) = ref_idx {
        let pin = 1e6;
        mat[ri][ri] += pin;
    } else {
        // Soft zero-mean gauge across all lines so solution is unique and balanced.
        let g = 1e-2 / (n_lines.max(1) as f64);
        for i in 0..n_lines {
            for j in 0..n_lines {
                mat[i][j] += g;
            }
        }
    }

    solve_linear(mat, rhs)
}

fn apply_shift_phase_gain(trace: &mut Trace, dt_shift_ms: f32, phase_rot_deg: f32, gain_scale: f32) {
    if !trace.is_live() || trace.samples.is_empty() {
        return;
    }
    let n = trace.samples.len();
    let nfft = (n * 2).next_power_of_two().max(64);
    let dt_s = trace.dt_s() as f64;
    let df = 1.0 / (nfft as f64 * dt_s);
    let tau = dt_shift_ms as f64 * 1e-3;
    let phi = (phase_rot_deg as f64).to_radians();
    let g = gain_scale as f64;

    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(nfft);
    let inv = planner.plan_fft_inverse(nfft);

    let mut buf = vec![Complex::new(0.0, 0.0); nfft];
    for (i, &s) in trace.samples.iter().enumerate() {
        buf[i] = Complex::new(s as f64, 0.0);
    }
    fwd.process(&mut buf);

    let half = nfft / 2;
    buf[0] *= g;
    for k in 1..half {
        let f = k as f64 * df;
        let angle = -2.0 * PI * f * tau - phi;
        buf[k] *= Complex::from_polar(g, angle);
        buf[nfft - k] *= Complex::from_polar(g, -angle);
    }
    buf[half] *= g * (-2.0 * PI * (half as f64 * df) * tau).cos();

    inv.process(&mut buf);
    let scale = 1.0 / nfft as f64;
    for i in 0..n {
        trace.samples[i] = (buf[i].re * scale) as f32;
    }
}

fn trace_xy(tr: &Trace) -> (f64, f64) {
    if tr.headers.cdp_x != 0.0 || tr.headers.cdp_y != 0.0 {
        (tr.headers.cdp_x, tr.headers.cdp_y)
    } else {
        (tr.headers.sou_x, tr.headers.sou_y)
    }
}

/// Analyze 2D multi-line misties across intersecting lines and apply balanced corrections in place.
pub fn analyze_and_balance_misties(
    traces: &mut [Trace],
    params: &MistieParams,
) -> Result<MistieReport> {
    if traces.is_empty() {
        return Err(SeismicError::InvalidFormat("Mistie analysis requires non-empty traces".into()));
    }
    let dt_s = traces[0].dt_s() as f64;
    let ns = traces[0].samples.len();

    // Group trace indices by line ID header.
    let mut lines_map: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (idx, tr) in traces.iter().enumerate() {
        if !tr.is_live() {
            continue;
        }
        let lid = tr.headers.get_f64(&params.line_key).unwrap_or(0.0).round() as i32;
        lines_map.entry(lid).or_default().push(idx);
    }
    let line_ids: Vec<i32> = lines_map.keys().copied().collect();
    if line_ids.len() < 2 {
        return Err(SeismicError::InvalidFormat(format!(
            "Mistie analysis needs at least 2 distinct '{}' lines (found {})",
            params.line_key,
            line_ids.len()
        )));
    }

    let i0 = ((params.t_start_ms as f64 * 1e-3 / dt_s).round() as usize).min(ns.saturating_sub(1));
    let i1 = if params.t_end_ms > params.t_start_ms {
        ((params.t_end_ms as f64 * 1e-3 / dt_s).round() as usize).min(ns)
    } else {
        ns
    };
    let win_len = i1.saturating_sub(i0);
    if win_len < 16 {
        return Err(SeismicError::InvalidFormat("Mistie analysis window too short".into()));
    }

    let local_stack = |indices: &[usize], pos: usize| -> Vec<f32> {
        let lo = pos.saturating_sub(params.stack_half_width);
        let hi = (pos + params.stack_half_width + 1).min(indices.len());
        let mut acc = vec![0.0f32; win_len];
        let count = (hi - lo).max(1) as f32;
        for &ti in &indices[lo..hi] {
            let s = &traces[ti].samples;
            for k in 0..win_len {
                acc[k] += s.get(i0 + k).copied().unwrap_or(0.0);
            }
        }
        for v in &mut acc {
            *v /= count;
        }
        acc
    };

    // Find closest trace pair between every distinct pair of lines.
    let mut intersections = Vec::new();
    let max_d2 = params.max_intersection_dist_m * params.max_intersection_dist_m;
    let mut dt_obs = Vec::new();
    let mut ph_obs = Vec::new();
    let mut gn_obs = Vec::new();

    for a in 0..line_ids.len() {
        let la = line_ids[a];
        let idxs_a = &lines_map[&la];
        for b in (a + 1)..line_ids.len() {
            let lb = line_ids[b];
            let idxs_b = &lines_map[&lb];
            let mut best_d2 = f64::INFINITY;
            let mut best_pair = (0usize, 0usize);
            for (pa, &tia) in idxs_a.iter().enumerate() {
                let (xa, ya) = trace_xy(&traces[tia]);
                for (pb, &tib) in idxs_b.iter().enumerate() {
                    let (xb, yb) = trace_xy(&traces[tib]);
                    let d2 = (xa - xb) * (xa - xb) + (ya - yb) * (ya - yb);
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best_pair = (pa, pb);
                    }
                }
            }
            if best_d2 <= max_d2 {
                let sa = local_stack(idxs_a, best_pair.0);
                let sb = local_stack(idxs_b, best_pair.1);
                let (dt_ms, phase_deg, gain_ratio, corr) =
                    measure_pair_mistie(&sa, &sb, dt_s, params.max_lag_ms);
                intersections.push(MistieIntersection {
                    line_a: la,
                    line_b: lb,
                    distance_m: best_d2.sqrt(),
                    dt_ms,
                    phase_deg,
                    gain_ratio,
                    correlation: corr,
                });
                if corr >= params.min_correlation {
                    let w = (corr as f64) * (corr as f64);
                    dt_obs.push((a, b, w, dt_ms as f64));
                    ph_obs.push((a, b, w, phase_deg as f64));
                    gn_obs.push((a, b, w, (gain_ratio.max(1e-4) as f64).ln()));
                }
            }
        }
    }

    if dt_obs.is_empty() {
        return Err(SeismicError::InvalidFormat(
            "No valid 2D line intersections met distance and correlation thresholds".into(),
        ));
    }

    let ref_idx = params
        .reference_line
        .and_then(|rl| line_ids.iter().position(|&id| id == rl));

    let c_dt = solve_mistie_network(line_ids.len(), &dt_obs, ref_idx);
    let c_ph = solve_mistie_network(line_ids.len(), &ph_obs, ref_idx);
    let c_gn = solve_mistie_network(line_ids.len(), &gn_obs, ref_idx);

    let rms_dt_before = (dt_obs.iter().map(|o| o.3 * o.3).sum::<f64>() / dt_obs.len() as f64).sqrt() as f32;
    let rms_ph_before = (ph_obs.iter().map(|o| o.3 * o.3).sum::<f64>() / ph_obs.len() as f64).sqrt() as f32;
    let rms_dt_after = (dt_obs
        .iter()
        .map(|&(a, b, _, d)| (d + c_dt[b] - c_dt[a]).powi(2))
        .sum::<f64>()
        / dt_obs.len() as f64)
        .sqrt() as f32;
    let rms_ph_after = (ph_obs
        .iter()
        .map(|&(a, b, _, d)| (d + c_ph[b] - c_ph[a]).powi(2))
        .sum::<f64>()
        / ph_obs.len() as f64)
        .sqrt() as f32;

    let mut corr_by_line = BTreeMap::new();
    let mut corrections = Vec::with_capacity(line_ids.len());
    for (i, &lid) in line_ids.iter().enumerate() {
        let corr = LineMistieCorrection {
            line_id: lid,
            dt_shift_ms: if params.apply_time { c_dt[i] as f32 } else { 0.0 },
            phase_rot_deg: if params.apply_phase { c_ph[i] as f32 } else { 0.0 },
            gain_scale: if params.apply_gain { c_gn[i].exp() as f32 } else { 1.0 },
        };
        corr_by_line.insert(lid, corr.clone());
        corrections.push(corr);
    }

    let key = params.line_key.clone();
    traces.par_iter_mut().for_each(|tr| {
        if !tr.is_live() {
            return;
        }
        let lid = tr.headers.get_f64(&key).unwrap_or(0.0).round() as i32;
        if let Some(c) = corr_by_line.get(&lid) {
            apply_shift_phase_gain(tr, c.dt_shift_ms, c.phase_rot_deg, c.gain_scale);
        }
    });

    Ok(MistieReport {
        intersections,
        corrections,
        rms_dt_before_ms: rms_dt_before,
        rms_dt_after_ms: rms_dt_after,
        rms_phase_before_deg: rms_ph_before,
        rms_phase_after_deg: rms_ph_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustseis_core::TraceHeaders;

    fn synth_reflectivity_trace(dt_us: u32, n: usize) -> Trace {
        let dt_s = dt_us as f64 * 1e-6;
        let mut s = vec![0.0f32; n];
        for (t0, amp) in [(0.35, 1.0), (0.62, -0.7), (0.88, 0.85)] {
            for i in 0..n {
                let t = i as f64 * dt_s - t0;
                let u = PI * 25.0 * t;
                let u2 = u * u;
                s[i] += (amp * (1.0 - 2.0 * u2) * (-u2).exp()) as f32;
            }
        }
        Trace::new(dt_us, s)
    }

    #[test]
    fn mistie_recovers_and_balances_known_time_phase_and_gain_shifts() {
        let dt_us = 2000;
        let n = 600;
        let base = synth_reflectivity_trace(dt_us, n);

        // Line 1 (master reference) runs West-East along Y = 500 m
        // Line 2 runs South-North along X = 500 m with +14 ms delay, +35 deg phase, 0.65x amplitude
        let mut traces = Vec::new();
        for ix in 0..11 {
            let mut tr = base.clone();
            tr.headers = TraceHeaders {
                traceno: ix + 1,
                iline_no: 1,
                cdp: ix + 1,
                cdp_x: ix as f64 * 100.0,
                cdp_y: 500.0,
                ..Default::default()
            };
            traces.push(tr);
        }
        for iy in 0..11 {
            let mut tr = base.clone();
            apply_shift_phase_gain(&mut tr, 14.0, 35.0, 0.65);
            tr.headers = TraceHeaders {
                traceno: 20 + iy + 1,
                iline_no: 2,
                cdp: iy + 1,
                cdp_x: 500.0,
                cdp_y: iy as f64 * 100.0,
                ..Default::default()
            };
            traces.push(tr);
        }

        let params = MistieParams {
            reference_line: Some(1),
            ..Default::default()
        };
        let rep = analyze_and_balance_misties(&mut traces, &params).unwrap();
        assert_eq!(rep.intersections.len(), 1);
        let ixn = &rep.intersections[0];
        assert!((ixn.dt_ms - 14.0).abs() < 0.5, "dt={}", ixn.dt_ms);
        assert!((ixn.phase_deg - 35.0).abs() < 2.0, "phase={}", ixn.phase_deg);
        assert!((ixn.gain_ratio - 0.65).abs() < 0.03, "gain={}", ixn.gain_ratio);
        assert!(rep.rms_dt_after_ms < 0.05);

        // Verify the crossing traces on Line 1 (idx 5) and Line 2 (idx 16) now match closely!
        let (dt_after, ph_after, gain_after, corr_after) =
            measure_pair_mistie(&traces[5].samples, &traces[16].samples, 0.002, 40.0);
        assert!(dt_after.abs() < 0.5, "residual dt_after={dt_after}");
        assert!(ph_after.abs() < 2.0, "residual ph_after={ph_after}");
        assert!((gain_after - 1.0).abs() < 0.03, "residual gain_after={gain_after}");
        assert!(corr_after > 0.99, "post-balance correlation={corr_after}");
    }
}
