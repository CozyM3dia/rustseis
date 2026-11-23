use rayon::prelude::*;
use rustseis_core::{
    FirstBreakPick, FirstBreakTable, Result, SeismicError, StaticsTable, Trace,
};
use rustseis_dsp::shift_trace_ms;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FirstBreakMethod {
    /// Akaike Information Criterion (Maeda 1985) — sharpest onset pick on clean & moderate SNR gathers.
    #[default]
    Aic,
    /// Modified Coppens / STA-LTA energy ratio picker.
    ModifiedCoppens,
}

/// Automated First-Break Picker parameters (`fbpick`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstBreakPickParams {
    #[serde(default)]
    pub method: FirstBreakMethod,
    /// Minimum refractor/direct apparent velocity defining late corridor edge (m/s, default 800 m/s).
    #[serde(default = "default_fb_vmin")]
    pub v_min_mps: f32,
    /// Maximum refractor apparent velocity defining early corridor edge (m/s, default 5500 m/s).
    #[serde(default = "default_fb_vmax")]
    pub v_max_mps: f32,
    /// Early corridor padding before `offset / v_max` in ms (default 40 ms).
    #[serde(default = "default_fb_pad")]
    pub early_pad_ms: f32,
    /// Late corridor padding after `offset / v_min` in ms (default 80 ms).
    #[serde(default = "default_fb_lpad")]
    pub late_pad_ms: f32,
    /// Short-term average window for ModifiedCoppens in ms (default 20 ms).
    #[serde(default = "default_sta")]
    pub sta_ms: f32,
    /// Long-term average window for ModifiedCoppens in ms (default 100 ms).
    #[serde(default = "default_lta")]
    pub lta_ms: f32,
    /// Minimum SNR required to accept a pick (default 2.0).
    #[serde(default = "default_min_snr")]
    pub min_snr: f32,
}

fn default_fb_vmin() -> f32 { 800.0 }
fn default_fb_vmax() -> f32 { 5500.0 }
fn default_fb_pad() -> f32 { 40.0 }
fn default_fb_lpad() -> f32 { 80.0 }
fn default_sta() -> f32 { 20.0 }
fn default_lta() -> f32 { 100.0 }
fn default_min_snr() -> f32 { 2.0 }

impl Default for FirstBreakPickParams {
    fn default() -> Self {
        Self {
            method: FirstBreakMethod::Aic,
            v_min_mps: 800.0,
            v_max_mps: 5500.0,
            early_pad_ms: 40.0,
            late_pad_ms: 80.0,
            sta_ms: 20.0,
            lta_ms: 100.0,
            min_snr: 2.0,
        }
    }
}

/// Pick first-break onset on a single trace inside `[i_lo, i_hi)`.
/// Returns `(pick_time_ms, snr)`.
fn pick_first_break_trace(tr: &Trace, params: &FirstBreakPickParams) -> Option<(f32, f32)> {
    if !tr.is_live() || tr.samples.len() < 16 {
        return None;
    }
    let dt_ms = tr.dt_ms().max(1e-3);
    let nt = tr.samples.len();
    let aoff = if tr.headers.aoffset > 0.0 {
        tr.headers.aoffset
    } else {
        tr.headers.offset.abs()
    };

    let t_early = ((aoff / params.v_max_mps.max(100.0)) * 1000.0 - params.early_pad_ms).max(0.0);
    let t_late = ((aoff / params.v_min_mps.max(100.0)) * 1000.0 + params.late_pad_ms)
        .min((nt - 1) as f32 * dt_ms);
    let i_lo = ((t_early / dt_ms).floor() as usize).min(nt.saturating_sub(8));
    let i_hi = ((t_late / dt_ms).ceil() as usize).clamp(i_lo + 8, nt);
    let seg = &tr.samples[i_lo..i_hi];
    let n = seg.len();

    // Prefix sums of squared energy for O(n) AIC & STA/LTA
    let mut csum = vec![0.0f64; n + 1];
    let mut csum2 = vec![0.0f64; n + 1];
    for i in 0..n {
        let v = seg[i] as f64;
        csum[i + 1] = csum[i] + v;
        csum2[i + 1] = csum2[i] + v * v;
    }

    let pick_local = match params.method {
        FirstBreakMethod::Aic => {
            // Restrict AIC end to shortly after first strong peak so later reflections don't pull AIC
            let mut max_abs = 0.0f32;
            for &s in seg {
                if s.abs() > max_abs {
                    max_abs = s.abs();
                }
            }
            if max_abs <= 1e-12 {
                return None;
            }
            let first_peak = seg
                .iter()
                .position(|&s| s.abs() >= 0.45 * max_abs)
                .unwrap_or(n / 2);
            let search_hi = (first_peak + (30.0 / dt_ms).round() as usize).clamp(6, n - 2);

            let mut best_k = 2usize;
            let mut best_aic = f64::INFINITY;
            for k in 2..search_hi {
                let m1 = csum[k] / k as f64;
                let v1 = (csum2[k] / k as f64 - m1 * m1).max(1e-24);
                let rem = (search_hi - k) as f64;
                let m2 = (csum[search_hi] - csum[k]) / rem;
                let v2 = ((csum2[search_hi] - csum2[k]) / rem - m2 * m2).max(1e-24);
                let aic = (k as f64) * v1.ln() + (rem - 1.0).max(1.0) * v2.ln();
                if aic < best_aic {
                    best_aic = aic;
                    best_k = k;
                }
            }
            best_k as f32
        }
        FirstBreakMethod::ModifiedCoppens => {
            let sta = ((params.sta_ms / dt_ms).round() as usize).clamp(2, n / 3);
            let lta = ((params.lta_ms / dt_ms).round() as usize).clamp(sta + 2, n - 2);
            let eps = 1e-4 * (csum2[n] / n as f64).max(1e-24);
            let mut best_k = sta;
            let mut best_ratio = -1.0f64;
            for k in sta..(n - sta) {
                let lead_e = (csum2[k + sta] - csum2[k]) / sta as f64;
                let lag_start = k.saturating_sub(lta);
                let lag_e = (csum2[k] - csum2[lag_start]) / (k - lag_start).max(1) as f64;
                let ratio = lead_e / (lag_e + eps);
                if ratio > best_ratio {
                    best_ratio = ratio;
                    best_k = k;
                }
            }
            best_k as f32
        }
    };

    let idx_k = (pick_local.round() as usize).clamp(1, n - 2);
    let pre_n = idx_k.max(1);
    let post_hi = (idx_k + (40.0 / dt_ms).round() as usize).min(n);
    let pre_rms = (csum2[pre_n] / pre_n as f64).sqrt();
    let post_rms = ((csum2[post_hi] - csum2[idx_k]) / (post_hi - idx_k).max(1) as f64).sqrt();
    let snr = (post_rms / pre_rms.max(1e-12)) as f32;
    if snr < params.min_snr {
        return None;
    }

    let pick_ms = (i_lo as f32 + pick_local) * dt_ms;
    Some((pick_ms, snr))
}

/// Run First-Break Picking (`fbpick`) across all traces, writing `FB_PICK` header & returning a `FirstBreakTable`.
pub fn pick_first_breaks(
    traces: &mut [Trace],
    params: &FirstBreakPickParams,
    table_name: &str,
) -> FirstBreakTable {
    let picks: Vec<Option<FirstBreakPick>> = traces
        .par_iter_mut()
        .map(|tr| {
            if let Some((t_ms, snr)) = pick_first_break_trace(tr, params) {
                tr.headers.fb_pick = t_ms;
                let src = if tr.headers.sou_sloc != 0 { tr.headers.sou_sloc } else { tr.headers.ffid };
                let rec = if tr.headers.rec_sloc != 0 { tr.headers.rec_sloc } else { tr.headers.chan };
                let off = if tr.headers.aoffset > 0.0 {
                    tr.headers.aoffset
                } else {
                    tr.headers.offset.abs()
                };
                Some(FirstBreakPick {
                    ffid: tr.headers.ffid,
                    chan: tr.headers.chan,
                    sou_sloc: src,
                    rec_sloc: rec,
                    offset_m: off,
                    time_ms: t_ms,
                    snr,
                })
            } else {
                None
            }
        })
        .collect();

    FirstBreakTable {
        name: table_name.to_string(),
        picks: picks.into_iter().flatten().collect(),
    }
}

/// Delay-Time / GLI Refraction Statics (`refstatic`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefractionStaticsParams {
    /// Minimum offset in meters for refracted arrivals (beyond crossover distance, default 100 m).
    #[serde(default = "default_ref_min_off")]
    pub min_offset_m: f32,
    /// Maximum offset in meters to include in inversion (default 3000 m).
    #[serde(default = "default_ref_max_off")]
    pub max_offset_m: f32,
    /// Near-surface weathering velocity `V0` in m/s (default 600 m/s).
    #[serde(default = "default_v_weather")]
    pub weathering_velocity_mps: f32,
    /// Replacement velocity `V_rep` to final datum in m/s (default 2000 m/s).
    #[serde(default = "default_v_rep")]
    pub replacement_velocity_mps: f32,
    /// Final datum elevation in meters (default 0.0 m).
    #[serde(default)]
    pub final_datum_m: f32,
    /// Number of Gauss-Seidel inversion iterations (default 40).
    #[serde(default = "default_ref_iters")]
    pub iterations: usize,
    /// Damping regularization for delay times (default 0.05).
    #[serde(default = "default_ref_damp")]
    pub damping: f32,
    /// Apply solved refraction statics immediately to trace samples (default true).
    #[serde(default = "default_true_st")]
    pub apply_shift: bool,
}

fn default_ref_min_off() -> f32 { 100.0 }
fn default_ref_max_off() -> f32 { 3000.0 }
fn default_v_weather() -> f32 { 600.0 }
fn default_v_rep() -> f32 { 2000.0 }
fn default_ref_iters() -> usize { 40 }
fn default_ref_damp() -> f32 { 0.05 }
fn default_true_st() -> bool { true }

impl Default for RefractionStaticsParams {
    fn default() -> Self {
        Self {
            min_offset_m: 100.0,
            max_offset_m: 3000.0,
            weathering_velocity_mps: 600.0,
            replacement_velocity_mps: 2000.0,
            final_datum_m: 0.0,
            iterations: 40,
            damping: 0.05,
            apply_shift: true,
        }
    }
}

/// Solve Delay-Time Refraction Statics (`refstatic`) from `FB_PICK` headers:
/// Model: `t_fb(s, r) = delay_s(s) + delay_r(r) + |x_sr| * slowness_refr`
/// Weathering thickness: `z_w = delay * V0 * V1 / sqrt(V1^2 - V0^2)`
/// Datum static: `stat = -z_w / V0 + (z_base - final_datum) / V_rep` (sign convention: negative shifts earlier).
pub fn solve_and_apply_refraction_statics(
    traces: &mut [Trace],
    params: &RefractionStaticsParams,
    table_name: &str,
) -> Result<StaticsTable> {
    struct Obs {
        src: i32,
        rec: i32,
        off_m: f64,
        t_s: f64,
    }
    let mut obs = Vec::new();
    let mut src_elev = BTreeMap::<i32, f64>::new();
    let mut rec_elev = BTreeMap::<i32, f64>::new();

    for tr in traces.iter() {
        if !tr.is_live() || tr.headers.fb_pick <= 0.0 {
            continue;
        }
        let off = if tr.headers.aoffset > 0.0 {
            tr.headers.aoffset as f64
        } else {
            tr.headers.offset.abs() as f64
        };
        if off < params.min_offset_m as f64 || off > params.max_offset_m as f64 {
            continue;
        }
        let src = if tr.headers.sou_sloc != 0 { tr.headers.sou_sloc } else { tr.headers.ffid };
        let rec = if tr.headers.rec_sloc != 0 { tr.headers.rec_sloc } else { tr.headers.chan };
        obs.push(Obs {
            src,
            rec,
            off_m: off,
            t_s: tr.headers.fb_pick as f64 / 1000.0,
        });
        src_elev.entry(src).or_insert(tr.headers.sou_elev as f64);
        rec_elev.entry(rec).or_insert(tr.headers.rec_elev as f64);
    }

    if obs.len() < 4 {
        return Err(SeismicError::InvalidFormat(
            "Need >= 4 valid FB_PICK traces within offset range for refstatic".into(),
        ));
    }

    // Initial linear regression t = intercept + slowness * offset
    let n_f = obs.len() as f64;
    let mean_x = obs.iter().map(|o| o.off_m).sum::<f64>() / n_f;
    let mean_t = obs.iter().map(|o| o.t_s).sum::<f64>() / n_f;
    let mut s_xx = 0.0f64;
    let mut s_xt = 0.0f64;
    for o in &obs {
        let dx = o.off_m - mean_x;
        s_xx += dx * dx;
        s_xt += dx * (o.t_s - mean_t);
    }
    let mut slowness = (s_xt / s_xx.max(1e-6)).clamp(1.0 / 8000.0, 1.0 / 800.0);
    let init_delay = ((mean_t - slowness * mean_x) * 0.5).max(0.0);

    let mut d_src = BTreeMap::<i32, f64>::new();
    let mut d_rec = BTreeMap::<i32, f64>::new();
    let mut src_idx = BTreeMap::<i32, Vec<usize>>::new();
    let mut rec_idx = BTreeMap::<i32, Vec<usize>>::new();

    for (i, o) in obs.iter().enumerate() {
        d_src.entry(o.src).or_insert(init_delay);
        d_rec.entry(o.rec).or_insert(init_delay);
        src_idx.entry(o.src).or_default().push(i);
        rec_idx.entry(o.rec).or_default().push(i);
    }

    let damp = params.damping.max(0.0) as f64;
    for _ in 0..params.iterations.max(5) {
        // Update source delay times
        for (&s, idxs) in &src_idx {
            let mut sum = 0.0f64;
            for &i in idxs {
                let o = &obs[i];
                sum += o.t_s - *d_rec.get(&o.rec).unwrap_or(&0.0) - slowness * o.off_m;
            }
            let val = ((sum + damp * init_delay) / (idxs.len() as f64 + damp)).max(0.0);
            d_src.insert(s, val);
        }
        // Update receiver delay times
        for (&r, idxs) in &rec_idx {
            let mut sum = 0.0f64;
            for &i in idxs {
                let o = &obs[i];
                sum += o.t_s - *d_src.get(&o.src).unwrap_or(&0.0) - slowness * o.off_m;
            }
            let val = ((sum + damp * init_delay) / (idxs.len() as f64 + damp)).max(0.0);
            d_rec.insert(r, val);
        }
        // Couple co-located source & receiver stations if station IDs overlap (eliminates DC shift ambiguity!)
        for (&s, vs) in d_src.iter_mut() {
            if let Some(vr) = d_rec.get_mut(&s) {
                let avg = 0.5 * (*vs + *vr);
                *vs = avg;
                *vr = avg;
            }
        }
        // Re-estimate global refractor slowness
        let mut num = 0.0f64;
        let mut den = 0.0f64;
        for o in &obs {
            let rem = o.t_s - *d_src.get(&o.src).unwrap_or(&0.0) - *d_rec.get(&o.rec).unwrap_or(&0.0);
            num += o.off_m * rem;
            den += o.off_m * o.off_m;
        }
        slowness = (num / den.max(1e-6)).clamp(1.0 / 8000.0, 1.0 / 800.0);
    }

    // Compute RMS traveltime misfit in ms
    let mut sq_misfit = 0.0f64;
    for o in &obs {
        let pred = *d_src.get(&o.src).unwrap_or(&0.0) + *d_rec.get(&o.rec).unwrap_or(&0.0) + slowness * o.off_m;
        let diff_ms = (o.t_s - pred) * 1000.0;
        sq_misfit += diff_ms * diff_ms;
    }
    let rms_misfit_ms = (sq_misfit / n_f).sqrt() as f32;

    let v1 = 1.0 / slowness;
    let v0 = (params.weathering_velocity_mps as f64).min(v1 * 0.85).max(100.0);
    let v_rep = (params.replacement_velocity_mps as f64).max(200.0);
    let datum = params.final_datum_m as f64;
    let cos_ic = (1.0 - (v0 / v1) * (v0 / v1)).sqrt().max(0.1);

    // Convert delay time tau -> weathering thickness z_w = tau * v0 / cos(ic) -> datum static (ms)
    let delay_to_static_ms = |tau_s: f64, elev_m: f64| -> f32 {
        let z_w = tau_s * v0 / cos_ic;
        let z_base = elev_m - z_w;
        // Static removes vertical traveltime through weathering (-z_w/v0) and replaces from base of weathering to datum
        let stat_s = -(z_w / v0) + (datum - z_base) / v_rep;
        (stat_s * 1000.0) as f32
    };

    let mut source_statics_ms = BTreeMap::new();
    for (&s, &tau) in &d_src {
        let elev = *src_elev.get(&s).unwrap_or(&datum);
        source_statics_ms.insert(s, delay_to_static_ms(tau, elev));
    }
    let mut receiver_statics_ms = BTreeMap::new();
    for (&r, &tau) in &d_rec {
        let elev = *rec_elev.get(&r).unwrap_or(&datum);
        receiver_statics_ms.insert(r, delay_to_static_ms(tau, elev));
    }

    // Write headers and optionally shift traces
    let apply = params.apply_shift;
    traces.par_iter_mut().for_each(|tr| {
        if !tr.is_live() {
            return;
        }
        let src = if tr.headers.sou_sloc != 0 { tr.headers.sou_sloc } else { tr.headers.ffid };
        let rec = if tr.headers.rec_sloc != 0 { tr.headers.rec_sloc } else { tr.headers.chan };
        let ss = *source_statics_ms.get(&src).unwrap_or(&0.0);
        let rs = *receiver_statics_ms.get(&rec).unwrap_or(&0.0);
        let tot = ss + rs;
        tr.headers.sou_stat = ss;
        tr.headers.rec_stat = rs;
        tr.headers.tot_stat += tot;
        if apply {
            shift_trace_ms(tr, tot);
        } else {
            tr.headers.na_stat += tot;
        }
    });

    Ok(StaticsTable {
        name: table_name.to_string(),
        kind: "Refraction".to_string(),
        source_statics_ms,
        receiver_statics_ms,
        cdp_structure_ms: BTreeMap::new(),
        residual_nmo: BTreeMap::new(),
        refractor_velocity_mps: v1 as f32,
        rms_residual_ms: rms_misfit_ms,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AutostatMethod {
    /// Correlation Autostatics (Wiggins 1976 / ProMAX `autostat`): pick trace-to-pilot lags & decompose via Gauss-Seidel.
    #[default]
    Correlation,
    /// Maximum-Power Stack Autostatics (Ronen & Claerbout 1985 / ProMAX `pwr_stat_new`):
    /// directly maximizes total CDP stack power by correlating source/receiver super-gathers against leave-one-out pilots.
    MaxPower,
}

/// Surface-Consistent Residual Autostatics (`autostat`, `pwr_stat_new`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutostatParams {
    #[serde(default)]
    pub method: AutostatMethod,
    /// Correlation gate start time in ms (default 200 ms).
    #[serde(default = "default_as_start")]
    pub gate_start_ms: f32,
    /// Correlation gate end time in ms (default 2500 ms).
    #[serde(default = "default_as_end")]
    pub gate_end_ms: f32,
    /// Maximum allowed static shift in ms (default 24.0 ms).
    #[serde(default = "default_max_shift")]
    pub max_shift_ms: f32,
    /// Number of outer pilot-update iterations (default 5).
    #[serde(default = "default_outer_iters")]
    pub outer_iterations: usize,
    /// Number of inner Gauss-Seidel decomposition iterations per outer pass (Correlation mode, default 20).
    #[serde(default = "default_inner_iters")]
    pub inner_iterations: usize,
    /// Minimum normalized correlation threshold to accept a lag pick (default 0.25).
    #[serde(default = "default_min_corr")]
    pub min_correlation: f32,
    /// Number of neighboring CDPs to laterally mix into the pilot trace (0 = pure CDP pilot, 1 = 3-CDP mix).
    #[serde(default = "default_pilot_mix")]
    pub pilot_mix_half_width: usize,
    /// Apply solved residual statics to traces (default true).
    #[serde(default = "default_true_st")]
    pub apply_shift: bool,
}

fn default_as_start() -> f32 { 200.0 }
fn default_as_end() -> f32 { 2500.0 }
fn default_max_shift() -> f32 { 24.0 }
fn default_outer_iters() -> usize { 5 }
fn default_inner_iters() -> usize { 20 }
fn default_min_corr() -> f32 { 0.25 }
fn default_pilot_mix() -> usize { 1 }

impl Default for AutostatParams {
    fn default() -> Self {
        Self {
            method: AutostatMethod::Correlation,
            gate_start_ms: 200.0,
            gate_end_ms: 2500.0,
            max_shift_ms: 24.0,
            outer_iterations: 5,
            inner_iterations: 20,
            min_correlation: 0.25,
            pilot_mix_half_width: 1,
            apply_shift: true,
        }
    }
}

/// Cross-correlate `trace` against `pilot` over lags `-max_lag..=+max_lag`.
/// Positive returned lag `d` means `trace` needs to be shifted by `+d` samples (`shift_trace_ms(+d*dt)`)
/// to align with `pilot` (i.e. `trace` currently arrives EARLIER than `pilot`, or vice versa:
/// `pilot[t] ≈ trace[t - d]`, so shifting `trace` by `+d` aligns `out[t] = trace[t - d] ≈ pilot[t]`).
fn xcorr_best_shift(
    trace: &[f32],
    pilot: &[f32],
    i0: usize,
    i1: usize,
    max_lag: usize,
) -> (f64, f64) {
    let nt = trace.len().min(pilot.len());
    let mut best_lag = 0isize;
    let mut best_cc = -1.0f64;
    let n_lags = 2 * max_lag + 1;
    let mut cc_vals = vec![0.0f64; n_lags];

    for (idx, lag) in (-(max_lag as isize)..=(max_lag as isize)).enumerate() {
        let mut num = 0.0f64;
        let mut e_tr = 0.0f64;
        let mut e_pi = 0.0f64;
        for t in i0..i1 {
            let j = t as isize - lag;
            if j >= 0 && (j as usize) < nt {
                let a = trace[j as usize] as f64;
                let b = pilot[t] as f64;
                num += a * b;
                e_tr += a * a;
                e_pi += b * b;
            }
        }
        let denom = (e_tr * e_pi).sqrt();
        let cc = if denom > 1e-18 { num / denom } else { 0.0 };
        cc_vals[idx] = cc;
        if cc > best_cc {
            best_cc = cc;
            best_lag = lag;
        }
    }

    // Parabolic sub-sample refinement around peak lag
    let best_idx = (best_lag + max_lag as isize) as usize;
    let sub_lag = if best_idx > 0 && best_idx + 1 < n_lags {
        let y_l = cc_vals[best_idx - 1];
        let y_c = cc_vals[best_idx];
        let y_r = cc_vals[best_idx + 1];
        let denom = y_l - 2.0 * y_c + y_r;
        if denom.abs() > 1e-12 {
            let delta = (0.5 * (y_l - y_r) / denom).clamp(-0.5, 0.5);
            best_lag as f64 + delta
        } else {
            best_lag as f64
        }
    } else {
        best_lag as f64
    };
    (sub_lag, best_cc)
}

/// Build CDP pilot stacks (optionally laterally mixed over +/- `mix_hw` neighboring CDPs).
fn build_cdp_pilots(
    traces: &[Trace],
    shifts_ms: &[f32],
    mix_hw: usize,
) -> BTreeMap<i32, Vec<f32>> {
    let nt = traces[0].samples.len();
    let mut raw_stacks = BTreeMap::<i32, (Vec<f64>, usize)>::new();
    for (idx, tr) in traces.iter().enumerate() {
        if !tr.is_live() {
            continue;
        }
        let mut shifted = tr.clone();
        if shifts_ms[idx].abs() > 1e-4 {
            shift_trace_ms(&mut shifted, shifts_ms[idx]);
        }
        let e = raw_stacks.entry(tr.headers.cdp).or_insert_with(|| (vec![0.0; nt], 0));
        for it in 0..nt.min(shifted.samples.len()) {
            e.0[it] += shifted.samples[it] as f64;
        }
        e.1 += 1;
    }

    let cdp_keys: Vec<i32> = raw_stacks.keys().copied().collect();
    let norm_stacks: Vec<Vec<f32>> = cdp_keys
        .iter()
        .map(|cdp| {
            let (ref acc, cnt) = raw_stacks[cdp];
            let inv = 1.0 / cnt.max(1) as f64;
            acc.iter().map(|&v| (v * inv) as f32).collect()
        })
        .collect();

    let mut pilots = BTreeMap::new();
    for (ci, &cdp) in cdp_keys.iter().enumerate() {
        if mix_hw == 0 || cdp_keys.len() <= 1 {
            pilots.insert(cdp, norm_stacks[ci].clone());
        } else {
            let lo = ci.saturating_sub(mix_hw);
            let hi = (ci + mix_hw).min(cdp_keys.len() - 1);
            let mut mix = vec![0.0f64; nt];
            let mut w_sum = 0.0f64;
            for j in lo..=hi {
                let w = if j == ci { 1.0 } else { 0.5 };
                for it in 0..nt {
                    mix[it] += norm_stacks[j][it] as f64 * w;
                }
                w_sum += w;
            }
            pilots.insert(cdp, mix.into_iter().map(|v| (v / w_sum) as f32).collect());
        }
    }
    pilots
}

/// Solve and optionally apply Surface-Consistent Residual Autostatics (`autostat` / `pwr_stat_new`).
pub fn solve_and_apply_autostat(
    traces: &mut [Trace],
    params: &AutostatParams,
    table_name: &str,
) -> Result<StaticsTable> {
    if traces.len() < 4 || traces[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Need >= 4 traces for autostat".into()));
    }
    let dt_ms = traces[0].dt_ms().max(1e-3);
    let nt = traces[0].samples.len();
    let i0 = ((params.gate_start_ms / dt_ms).round().max(0.0) as usize).min(nt.saturating_sub(8));
    let i1 = ((params.gate_end_ms / dt_ms).round().max((i0 + 8) as f32) as usize).min(nt);
    let max_lag = ((params.max_shift_ms / dt_ms).ceil() as usize).clamp(1, (i1 - i0) / 3);

    let mut s_stat = BTreeMap::<i32, f64>::new();
    let mut r_stat = BTreeMap::<i32, f64>::new();
    let mut g_stat = BTreeMap::<i32, f64>::new();

    let mut src_buckets = BTreeMap::<i32, Vec<usize>>::new();
    let mut rec_buckets = BTreeMap::<i32, Vec<usize>>::new();
    let mut cdp_buckets = BTreeMap::<i32, Vec<usize>>::new();

    for (idx, tr) in traces.iter().enumerate() {
        if !tr.is_live() {
            continue;
        }
        let src = if tr.headers.sou_sloc != 0 { tr.headers.sou_sloc } else { tr.headers.ffid };
        let rec = if tr.headers.rec_sloc != 0 { tr.headers.rec_sloc } else { tr.headers.chan };
        let cdp = tr.headers.cdp;
        s_stat.entry(src).or_insert(0.0);
        r_stat.entry(rec).or_insert(0.0);
        g_stat.entry(cdp).or_insert(0.0);
        src_buckets.entry(src).or_default().push(idx);
        rec_buckets.entry(rec).or_default().push(idx);
        cdp_buckets.entry(cdp).or_default().push(idx);
    }

    let mut rms_res_ms = 0.0f64;
    let max_shift_f = params.max_shift_ms.max(dt_ms) as f64;

    for _outer in 0..params.outer_iterations.max(1) {
        let cur_shifts: Vec<f32> = traces
            .iter()
            .map(|tr| {
                let src = if tr.headers.sou_sloc != 0 { tr.headers.sou_sloc } else { tr.headers.ffid };
                let rec = if tr.headers.rec_sloc != 0 { tr.headers.rec_sloc } else { tr.headers.chan };
                (*s_stat.get(&src).unwrap_or(&0.0) + *r_stat.get(&rec).unwrap_or(&0.0)) as f32
            })
            .collect();

        let pilots = build_cdp_pilots(traces, &cur_shifts, params.pilot_mix_half_width);

        match params.method {
            AutostatMethod::Correlation => {
                // Pick residual lag for every live trace against its CDP pilot (in parallel)
                let lags: Vec<Option<(usize, i32, i32, i32, f64)>> = traces
                    .par_iter()
                    .enumerate()
                    .map(|(idx, tr)| {
                        if !tr.is_live() {
                            return None;
                        }
                        let pilot = pilots.get(&tr.headers.cdp)?;
                        let mut shifted = tr.clone();
                        if cur_shifts[idx].abs() > 1e-4 {
                            shift_trace_ms(&mut shifted, cur_shifts[idx]);
                        }
                        let (lag_samp, cc) = xcorr_best_shift(&shifted.samples, pilot, i0, i1, max_lag);
                        if cc < params.min_correlation as f64 {
                            return None;
                        }
                        // Total required shift = current shift + residual lag
                        let total_lag_ms = cur_shifts[idx] as f64 + lag_samp * dt_ms as f64;
                        let src = if tr.headers.sou_sloc != 0 { tr.headers.sou_sloc } else { tr.headers.ffid };
                        let rec = if tr.headers.rec_sloc != 0 { tr.headers.rec_sloc } else { tr.headers.chan };
                        Some((idx, src, rec, tr.headers.cdp, total_lag_ms))
                    })
                    .collect();

                let valid_lags: Vec<(usize, i32, i32, i32, f64)> = lags.into_iter().flatten().collect();
                if valid_lags.is_empty() {
                    break;
                }

                // Gauss-Seidel decomposition: total_lag = S[src] + R[rec] + G[cdp]
                let mut sb = BTreeMap::<i32, Vec<usize>>::new();
                let mut rb = BTreeMap::<i32, Vec<usize>>::new();
                let mut cb = BTreeMap::<i32, Vec<usize>>::new();
                for (li, &(_, s, r, c, _)) in valid_lags.iter().enumerate() {
                    sb.entry(s).or_default().push(li);
                    rb.entry(r).or_default().push(li);
                    cb.entry(c).or_default().push(li);
                }

                let damp = 0.1f64;
                for _ in 0..params.inner_iterations.max(5) {
                    for (&s, idxs) in &sb {
                        let sum: f64 = idxs
                            .iter()
                            .map(|&li| {
                                let (_, _, r, c, d) = valid_lags[li];
                                d - *r_stat.get(&r).unwrap_or(&0.0) - *g_stat.get(&c).unwrap_or(&0.0)
                            })
                            .sum();
                        let val = (sum / (idxs.len() as f64 + damp)).clamp(-max_shift_f, max_shift_f);
                        s_stat.insert(s, val);
                    }
                    let sm = s_stat.values().sum::<f64>() / s_stat.len().max(1) as f64;
                    for v in s_stat.values_mut() { *v -= sm; }

                    for (&r, idxs) in &rb {
                        let sum: f64 = idxs
                            .iter()
                            .map(|&li| {
                                let (_, s, _, c, d) = valid_lags[li];
                                d - *s_stat.get(&s).unwrap_or(&0.0) - *g_stat.get(&c).unwrap_or(&0.0)
                            })
                            .sum();
                        let val = (sum / (idxs.len() as f64 + damp)).clamp(-max_shift_f, max_shift_f);
                        r_stat.insert(r, val);
                    }
                    let rm = r_stat.values().sum::<f64>() / r_stat.len().max(1) as f64;
                    for v in r_stat.values_mut() { *v -= rm; }

                    for (&c, idxs) in &cb {
                        let sum: f64 = idxs
                            .iter()
                            .map(|&li| {
                                let (_, s, r, _, d) = valid_lags[li];
                                d - *s_stat.get(&s).unwrap_or(&0.0) - *r_stat.get(&r).unwrap_or(&0.0)
                            })
                            .sum();
                        let val = (sum / (idxs.len() as f64 + 0.25)).clamp(-max_shift_f, max_shift_f);
                        g_stat.insert(c, val);
                    }
                    let gm = g_stat.values().sum::<f64>() / g_stat.len().max(1) as f64;
                    for v in g_stat.values_mut() { *v -= gm; }
                }

                let sq: f64 = valid_lags
                    .iter()
                    .map(|&(_, s, r, c, d)| {
                        let p = *s_stat.get(&s).unwrap_or(&0.0)
                            + *r_stat.get(&r).unwrap_or(&0.0)
                            + *g_stat.get(&c).unwrap_or(&0.0);
                        (d - p) * (d - p)
                    })
                    .sum();
                rms_res_ms = (sq / valid_lags.len() as f64).sqrt();
            }
            AutostatMethod::MaxPower => {
                // Ronen & Claerbout (1985): stack cross-correlations across ALL traces sharing source `s`,
                // then pick peak of summed cross-correlation (immune to single-trace cycle skips!).
                let n_lags = 2 * max_lag + 1;
                let update_domain = |buckets: &BTreeMap<i32, Vec<usize>>,
                                     stat_map: &mut BTreeMap<i32, f64>,
                                     cur_sh: &[f32]| {
                    let updates: Vec<(i32, f64)> = buckets
                        .par_iter()
                        .map(|(&key, idxs)| {
                            let mut cc_sum = vec![0.0f64; n_lags];
                            for &ti in idxs {
                                let tr = &traces[ti];
                                let Some(pilot) = pilots.get(&tr.headers.cdp) else { continue };
                                let mut shifted = tr.clone();
                                if cur_sh[ti].abs() > 1e-4 {
                                    shift_trace_ms(&mut shifted, cur_sh[ti]);
                                }
                                // Leave-one-out pilot removes self-correlation bias!
                                let fold = cdp_buckets.get(&tr.headers.cdp).map(|v| v.len()).unwrap_or(2) as f32;
                                for (li, lag) in (-(max_lag as isize)..=(max_lag as isize)).enumerate() {
                                    let mut dot = 0.0f64;
                                    for t in i0..i1 {
                                        let j = t as isize - lag;
                                        if j >= 0 && (j as usize) < nt {
                                            let p_loo = if fold > 1.5 {
                                                (pilot[t] * fold - shifted.samples[t]) / (fold - 1.0)
                                            } else {
                                                pilot[t]
                                            };
                                            dot += shifted.samples[j as usize] as f64 * p_loo as f64;
                                        }
                                    }
                                    cc_sum[li] += dot;
                                }
                            }
                            let (best_li, _) = cc_sum
                                .iter()
                                .enumerate()
                                .max_by(|a, b| a.1.total_cmp(b.1))
                                .unwrap_or((max_lag, &0.0));
                            let delta_samp = if best_li > 0 && best_li + 1 < n_lags {
                                let yl = cc_sum[best_li - 1];
                                let yc = cc_sum[best_li];
                                let yr = cc_sum[best_li + 1];
                                let den = yl - 2.0 * yc + yr;
                                let sub = if den.abs() > 1e-12 { (0.5 * (yl - yr) / den).clamp(-0.5, 0.5) } else { 0.0 };
                                (best_li as isize - max_lag as isize) as f64 + sub
                            } else {
                                (best_li as isize - max_lag as isize) as f64
                            };
                            (key, delta_samp * dt_ms as f64)
                        })
                        .collect();

                    let mut sq_upd = 0.0f64;
                    for (k, d_ms) in updates {
                        let e = stat_map.entry(k).or_insert(0.0);
                        *e = (*e + 0.7 * d_ms).clamp(-max_shift_f, max_shift_f);
                        sq_upd += d_ms * d_ms;
                    }
                    let mean = stat_map.values().sum::<f64>() / stat_map.len().max(1) as f64;
                    for v in stat_map.values_mut() {
                        *v -= mean;
                    }
                    (sq_upd / stat_map.len().max(1) as f64).sqrt()
                };

                let rms_s = update_domain(&src_buckets, &mut s_stat, &cur_shifts);
                // Recompute shifts after source update before receiver update
                let mid_shifts: Vec<f32> = traces
                    .iter()
                    .map(|tr| {
                        let src = if tr.headers.sou_sloc != 0 { tr.headers.sou_sloc } else { tr.headers.ffid };
                        let rec = if tr.headers.rec_sloc != 0 { tr.headers.rec_sloc } else { tr.headers.chan };
                        (*s_stat.get(&src).unwrap_or(&0.0) + *r_stat.get(&rec).unwrap_or(&0.0)) as f32
                    })
                    .collect();
                let rms_r = update_domain(&rec_buckets, &mut r_stat, &mid_shifts);
                rms_res_ms = 0.5 * (rms_s + rms_r);
            }
        }
    }

    let apply = params.apply_shift;
    traces.par_iter_mut().for_each(|tr| {
        if !tr.is_live() {
            return;
        }
        let src = if tr.headers.sou_sloc != 0 { tr.headers.sou_sloc } else { tr.headers.ffid };
        let rec = if tr.headers.rec_sloc != 0 { tr.headers.rec_sloc } else { tr.headers.chan };
        let ss = *s_stat.get(&src).unwrap_or(&0.0) as f32;
        let rs = *r_stat.get(&rec).unwrap_or(&0.0) as f32;
        let tot = ss + rs;
        match params.method {
            AutostatMethod::Correlation => {
                tr.headers.cs_stat = ss;
                tr.headers.cr_stat = rs;
            }
            AutostatMethod::MaxPower => {
                tr.headers.ps_stat = ss;
                tr.headers.pr_stat = rs;
            }
        }
        tr.headers.tot_stat += tot;
        if apply {
            shift_trace_ms(tr, tot);
        } else {
            tr.headers.na_stat += tot;
        }
    });

    Ok(StaticsTable {
        name: table_name.to_string(),
        kind: match params.method {
            AutostatMethod::Correlation => "CorrelationAutostat".into(),
            AutostatMethod::MaxPower => "MaxPowerAutostat".into(),
        },
        source_statics_ms: s_stat.into_iter().map(|(k, v)| (k, v as f32)).collect(),
        receiver_statics_ms: r_stat.into_iter().map(|(k, v)| (k, v as f32)).collect(),
        cdp_structure_ms: g_stat.into_iter().map(|(k, v)| (k, v as f32)).collect(),
        residual_nmo: BTreeMap::new(),
        refractor_velocity_mps: 0.0,
        rms_residual_ms: rms_res_ms as f32,
    })
}

/// CDP Trim Statics (`trimstat` — non-surface-consistent per-trace alignment to CDP pilot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrimStatParams {
    #[serde(default = "default_as_start")]
    pub gate_start_ms: f32,
    #[serde(default = "default_as_end")]
    pub gate_end_ms: f32,
    /// Maximum allowed trim static shift in ms (default 12.0 ms).
    #[serde(default = "default_trim_max")]
    pub max_shift_ms: f32,
    /// Minimum correlation to apply trim static (default 0.35).
    #[serde(default = "default_trim_corr")]
    pub min_correlation: f32,
}

fn default_trim_max() -> f32 { 12.0 }
fn default_trim_corr() -> f32 { 0.35 }

impl Default for TrimStatParams {
    fn default() -> Self {
        Self {
            gate_start_ms: 200.0,
            gate_end_ms: 2500.0,
            max_shift_ms: 12.0,
            min_correlation: 0.35,
        }
    }
}

/// Apply CDP Trim Statics (`trimstat`): aligns each trace in a CDP gather to the leave-one-out CDP pilot.
pub fn apply_trim_statics(traces: &mut [Trace], params: &TrimStatParams) -> Result<()> {
    if traces.len() < 2 || traces[0].samples.is_empty() {
        return Ok(());
    }
    let dt_ms = traces[0].dt_ms().max(1e-3);
    let nt = traces[0].samples.len();
    let i0 = ((params.gate_start_ms / dt_ms).round().max(0.0) as usize).min(nt.saturating_sub(8));
    let i1 = ((params.gate_end_ms / dt_ms).round().max((i0 + 8) as f32) as usize).min(nt);
    let max_lag = ((params.max_shift_ms / dt_ms).ceil() as usize).clamp(1, (i1 - i0) / 3);

    let zeros = vec![0.0f32; traces.len()];
    let pilots = build_cdp_pilots(traces, &zeros, 0);

    traces.par_iter_mut().for_each(|tr| {
        if !tr.is_live() {
            return;
        }
        let Some(pilot) = pilots.get(&tr.headers.cdp) else { return };
        let (lag_samp, cc) = xcorr_best_shift(&tr.samples, pilot, i0, i1, max_lag);
        if cc >= params.min_correlation as f64 {
            let shift_ms = (lag_samp * dt_ms as f64)
                .clamp(-(params.max_shift_ms as f64), params.max_shift_ms as f64) as f32;
            tr.headers.trimstat = shift_ms;
            tr.headers.tot_stat += shift_ms;
            shift_trace_ms(tr, shift_ms);
        }
    });

    Ok(())
}
