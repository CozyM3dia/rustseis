use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, SurfaceConsistentAmpTable, Trace};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ScAmpMeasure {
    #[default]
    Rms,
    MeanAbs,
    /// ProMAX 21-subgate expanding Hilbert envelope median (`scamp2.pdf`).
    EnvelopeMedian,
}

/// Surface-Consistent Amplitudes (`scamp2` — faithful to ProMAX `scamp2.pdf`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScAmpParams {
    /// Amplitude gate start time in ms (default 200 ms).
    #[serde(default = "default_gate_start")]
    pub gate_start_ms: f32,
    /// Amplitude gate end time in ms (default 3000 ms).
    #[serde(default = "default_gate_end")]
    pub gate_end_ms: f32,
    #[serde(default)]
    pub measure: ScAmpMeasure,
    /// Offset bin width in meters when offset component is enabled (default 100 m).
    #[serde(default = "default_off_bin")]
    pub offset_bin_m: f32,
    /// Components to solve & optionally apply.
    #[serde(default = "default_true")]
    pub solve_source: bool,
    #[serde(default = "default_true")]
    pub solve_receiver: bool,
    #[serde(default = "default_true")]
    pub solve_offset: bool,
    #[serde(default)]
    pub solve_cdp: bool,
    #[serde(default)]
    pub solve_channel: bool,
    /// Which solved components to divide out on Apply (ProMAX lets you decompose all 5 but apply e.g. source+receiver only).
    #[serde(default = "default_true")]
    pub apply_source: bool,
    #[serde(default = "default_true")]
    pub apply_receiver: bool,
    #[serde(default)]
    pub apply_offset: bool,
    #[serde(default)]
    pub apply_cdp: bool,
    #[serde(default)]
    pub apply_channel: bool,
    /// Number of Gauss-Seidel iterations (default 25).
    #[serde(default = "default_iters")]
    pub iterations: usize,
    /// Damping factor for Gauss-Seidel stability (default 0.05).
    #[serde(default = "default_damp")]
    pub damping: f32,
    /// Preserve global mean amplitude (true) vs normalize toward unity (false = ProMAX raw default).
    #[serde(default = "default_true")]
    pub preserve_mean_amp: bool,
}

fn default_gate_start() -> f32 { 200.0 }
fn default_gate_end() -> f32 { 3000.0 }
fn default_off_bin() -> f32 { 100.0 }
fn default_iters() -> usize { 25 }
fn default_damp() -> f32 { 0.05 }
fn default_true() -> bool { true }

impl Default for ScAmpParams {
    fn default() -> Self {
        Self {
            gate_start_ms: 200.0,
            gate_end_ms: 3000.0,
            measure: ScAmpMeasure::Rms,
            offset_bin_m: 100.0,
            solve_source: true,
            solve_receiver: true,
            solve_offset: true,
            solve_cdp: false,
            solve_channel: false,
            apply_source: true,
            apply_receiver: true,
            apply_offset: false,
            apply_cdp: false,
            apply_channel: false,
            iterations: 25,
            damping: 0.05,
            preserve_mean_amp: true,
        }
    }
}

/// Measure trace amplitude inside `[i0..i1)` using Rms, MeanAbs, or ProMAX 21-subgate Hilbert envelope median.
fn measure_trace_amp(samples: &[f32], i0: usize, i1: usize, mode: ScAmpMeasure) -> Option<f64> {
    if i1 <= i0 || i1 > samples.len() {
        return None;
    }
    let slice = &samples[i0..i1];
    match mode {
        ScAmpMeasure::Rms => {
            let mut sum = 0.0f64;
            let mut cnt = 0usize;
            for &s in slice {
                if s != 0.0 && s.is_finite() {
                    sum += (s as f64) * (s as f64);
                    cnt += 1;
                }
            }
            if cnt == 0 || sum <= 1e-24 { None } else { Some((sum / cnt as f64).sqrt()) }
        }
        ScAmpMeasure::MeanAbs => {
            let mut sum = 0.0f64;
            let mut cnt = 0usize;
            for &s in slice {
                if s != 0.0 && s.is_finite() {
                    sum += (s as f64).abs();
                    cnt += 1;
                }
            }
            if cnt == 0 || sum <= 1e-24 { None } else { Some(sum / cnt as f64) }
        }
        ScAmpMeasure::EnvelopeMedian => {
            // Compute Hilbert analytic envelope via FFT
            let n = slice.len();
            let nfft = n.next_power_of_two().max(32);
            let mut planner = FftPlanner::<f64>::new();
            let fwd = planner.plan_fft_forward(nfft);
            let inv = planner.plan_fft_inverse(nfft);
            let mut buf = vec![Complex::new(0.0, 0.0); nfft];
            for i in 0..n {
                buf[i] = Complex::new(slice[i] as f64, 0.0);
            }
            fwd.process(&mut buf);
            for k in 1..nfft / 2 {
                buf[k] *= 2.0;
            }
            for k in (nfft / 2 + 1)..nfft {
                buf[k] = Complex::new(0.0, 0.0);
            }
            inv.process(&mut buf);
            let inv_n = 1.0 / nfft as f64;
            let env: Vec<f64> = (0..n).map(|i| buf[i].norm() * inv_n).collect();

            // 21 expanding subgates as specified in ProMAX `scamp2.pdf`
            let mut sub_means = Vec::with_capacity(21);
            for sg in 1..=21 {
                let end = ((sg * n) / 21).max(1).min(n);
                let mut sum = 0.0f64;
                let mut cnt = 0usize;
                for i in 0..end {
                    if slice[i] != 0.0 {
                        sum += env[i];
                        cnt += 1;
                    }
                }
                if cnt > 0 {
                    sub_means.push(sum / cnt as f64);
                }
            }
            if sub_means.is_empty() {
                return None;
            }
            sub_means.sort_by(|a, b| a.total_cmp(b));
            let med = sub_means[sub_means.len() / 2];
            if med <= 1e-18 { None } else { Some(med) }
        }
    }
}

#[derive(Clone, Copy)]
struct TraceKeys {
    src: i32,
    rec: i32,
    ofb: i32,
    cdp: i32,
    chn: i32,
}

fn extract_keys(tr: &Trace, offset_bin_m: f32) -> TraceKeys {
    let src = if tr.headers.sou_sloc != 0 { tr.headers.sou_sloc } else { tr.headers.ffid };
    let rec = if tr.headers.rec_sloc != 0 { tr.headers.rec_sloc } else { tr.headers.chan };
    let aoff = if tr.headers.aoffset > 0.0 { tr.headers.aoffset } else { tr.headers.offset.abs() };
    let ofb = (aoff / offset_bin_m.max(1.0)).floor() as i32;
    TraceKeys {
        src,
        rec,
        ofb,
        cdp: tr.headers.cdp,
        chn: tr.headers.chan,
    }
}

/// Remove mean from a component map to eliminate null-space drift between domains.
fn zero_mean_map(map: &mut BTreeMap<i32, f64>) {
    if map.is_empty() {
        return;
    }
    let mean = map.values().sum::<f64>() / map.len() as f64;
    for v in map.values_mut() {
        *v -= mean;
    }
}

/// Run Gauss-Seidel surface-consistent additive decomposition:
/// `y_k ≈ mu + S[src] + R[rec] + O[ofb] + C[cdp] + H[chn]`.
fn gauss_seidel_decompose(
    obs: &[(TraceKeys, f64)],
    solve_flags: [bool; 5],
    iterations: usize,
    damping: f64,
) -> (f64, [BTreeMap<i32, f64>; 5], f64) {
    if obs.is_empty() {
        return (0.0, Default::default(), 0.0);
    }
    let mu = obs.iter().map(|(_, y)| *y).sum::<f64>() / obs.len() as f64;
    let mut comps: [BTreeMap<i32, f64>; 5] = Default::default();

    // Index observations by key for each active domain
    let mut buckets: [BTreeMap<i32, Vec<usize>>; 5] = Default::default();
    for (idx, &(k, _)) in obs.iter().enumerate() {
        let ks = [k.src, k.rec, k.ofb, k.cdp, k.chn];
        for d in 0..5 {
            if solve_flags[d] {
                buckets[d].entry(ks[d]).or_default().push(idx);
                comps[d].entry(ks[d]).or_insert(0.0);
            }
        }
    }

    let damp = damping.max(0.0);
    for _ in 0..iterations.max(1) {
        for d in 0..5 {
            if !solve_flags[d] {
                continue;
            }
            for (&key, indices) in &buckets[d] {
                let mut num = 0.0f64;
                for &idx in indices {
                    let (k, y) = obs[idx];
                    let ks = [k.src, k.rec, k.ofb, k.cdp, k.chn];
                    let mut other = mu;
                    for od in 0..5 {
                        if od != d && solve_flags[od] {
                            other += *comps[od].get(&ks[od]).unwrap_or(&0.0);
                        }
                    }
                    num += y - other;
                }
                let denom = indices.len() as f64 + damp;
                comps[d].insert(key, num / denom);
            }
            zero_mean_map(&mut comps[d]);
        }
    }

    // Compute final RMS residual
    let mut sq_err = 0.0f64;
    for &(k, y) in obs {
        let ks = [k.src, k.rec, k.ofb, k.cdp, k.chn];
        let mut pred = mu;
        for d in 0..5 {
            if solve_flags[d] {
                pred += *comps[d].get(&ks[d]).unwrap_or(&0.0);
            }
        }
        let r = y - pred;
        sq_err += r * r;
    }
    let rms = (sq_err / obs.len() as f64).sqrt();
    (mu, comps, rms)
}

/// Compute and apply Surface-Consistent Amplitudes (`scamp2`).
/// Returns the solved `SurfaceConsistentAmpTable` for QC plotting and OPF storage.
pub fn apply_sc_amp(
    traces: &mut [Trace],
    params: &ScAmpParams,
    table_name: &str,
) -> Result<SurfaceConsistentAmpTable> {
    if traces.is_empty() {
        return Err(SeismicError::InvalidFormat("Empty traces for scamp2".into()));
    }
    let dt_ms = traces[0].dt_ms().max(1e-3);
    let nt = traces[0].samples.len();
    let i0 = ((params.gate_start_ms / dt_ms).round().max(0.0) as usize).min(nt.saturating_sub(1));
    let i1 = ((params.gate_end_ms / dt_ms).round().max((i0 + 1) as f32) as usize).min(nt);

    // Pass 1: parallel per-trace log-amplitude extraction
    let raw_obs: Vec<Option<(TraceKeys, f64)>> = traces
        .par_iter()
        .map(|tr| {
            if !tr.is_live() {
                return None;
            }
            let amp = measure_trace_amp(&tr.samples, i0, i1, params.measure)?;
            Some((extract_keys(tr, params.offset_bin_m), amp.ln()))
        })
        .collect();
    let obs: Vec<(TraceKeys, f64)> = raw_obs.into_iter().flatten().collect();
    if obs.is_empty() {
        return Err(SeismicError::InvalidFormat("No live nonzero traces in scamp2 gate".into()));
    }

    // Pass 2: Gauss-Seidel decomposition
    let solve_flags = [
        params.solve_source,
        params.solve_receiver,
        params.solve_offset,
        params.solve_cdp,
        params.solve_channel,
    ];
    let (mu, comps, rms_res) =
        gauss_seidel_decompose(&obs, solve_flags, params.iterations, params.damping as f64);

    // Pass 3: Apply selected components
    let apply_flags = [
        params.apply_source && params.solve_source,
        params.apply_receiver && params.solve_receiver,
        params.apply_offset && params.solve_offset,
        params.apply_cdp && params.solve_cdp,
        params.apply_channel && params.solve_channel,
    ];
    let global_shift = if params.preserve_mean_amp { 0.0 } else { mu };

    traces.par_iter_mut().for_each(|tr| {
        if !tr.is_live() {
            return;
        }
        let k = extract_keys(tr, params.offset_bin_m);
        let ks = [k.src, k.rec, k.ofb, k.cdp, k.chn];
        let mut log_gain = global_shift;
        for d in 0..5 {
            if apply_flags[d] {
                log_gain += *comps[d].get(&ks[d]).unwrap_or(&0.0);
            }
        }
        let scalar = (-log_gain).exp() as f32;
        if scalar.is_finite() && scalar > 0.0 {
            for s in &mut tr.samples {
                *s *= scalar;
            }
            tr.headers.amp_norm = scalar;
        }
    });

    let to_linear = |m: &BTreeMap<i32, f64>| -> BTreeMap<i32, f32> {
        m.iter().map(|(&k, &v)| (k, v.exp() as f32)).collect()
    };

    Ok(SurfaceConsistentAmpTable {
        name: table_name.to_string(),
        mean_log_amp: mu,
        source_factors: to_linear(&comps[0]),
        receiver_factors: to_linear(&comps[1]),
        offset_factors: to_linear(&comps[2]),
        cdp_factors: to_linear(&comps[3]),
        channel_factors: to_linear(&comps[4]),
        rms_log_residual: rms_res as f32,
    })
}

/// Surface-Consistent Deconvolution (`scdecon` / `scdeconm1`–`m3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScDeconParams {
    /// Design gate start in ms (default 200 ms).
    #[serde(default = "default_gate_start")]
    pub gate_start_ms: f32,
    /// Design gate end in ms (default 3000 ms).
    #[serde(default = "default_gate_end")]
    pub gate_end_ms: f32,
    /// Wiener spiking/predictive operator length in ms (default 120 ms).
    #[serde(default = "default_sc_op")]
    pub operator_ms: f32,
    /// Prediction lag in ms (0 or <= dt = spiking decon; > dt = predictive gapped decon).
    #[serde(default)]
    pub prediction_lag_ms: f32,
    /// White noise stabilization percentage (default 0.5%).
    #[serde(default = "default_sc_wn")]
    pub white_noise_pct: f32,
    /// Offset bin size in meters (default 100 m).
    #[serde(default = "default_off_bin")]
    pub offset_bin_m: f32,
    /// Decompose & deconvolve source component (default true).
    #[serde(default = "default_true")]
    pub decon_source: bool,
    /// Decompose & deconvolve receiver component (default true).
    #[serde(default = "default_true")]
    pub decon_receiver: bool,
    /// Include offset component in decomposition (default true, not applied unless `decon_offset` true).
    #[serde(default = "default_true")]
    pub solve_offset: bool,
    #[serde(default)]
    pub decon_offset: bool,
    /// Gauss-Seidel iterations per frequency bin (default 12).
    #[serde(default = "default_sc_iters")]
    pub iterations: usize,
}

fn default_sc_op() -> f32 { 120.0 }
fn default_sc_wn() -> f32 { 0.5 }
fn default_sc_iters() -> usize { 12 }

impl Default for ScDeconParams {
    fn default() -> Self {
        Self {
            gate_start_ms: 200.0,
            gate_end_ms: 3000.0,
            operator_ms: 120.0,
            prediction_lag_ms: 0.0,
            white_noise_pct: 0.5,
            offset_bin_m: 100.0,
            decon_source: true,
            decon_receiver: true,
            solve_offset: true,
            decon_offset: false,
            iterations: 12,
        }
    }
}

/// Real symmetric Levinson-Durbin solver for Wiener spiking/predictive decon operator from autocorrelation `r[0..op_len+lag]`.
fn solve_real_wiener_decon(r: &[f64], op_len: usize, lag_samp: usize, wn_frac: f64) -> Vec<f64> {
    let p = op_len.max(2);
    let r0 = r[0];
    if r0 <= 1e-24 {
        let mut id = vec![0.0; p];
        id[0] = 1.0;
        return id;
    }
    let diag = r0 * (1.0 + wn_frac.max(1e-5));

    // Build Toeplitz autocorrelation a[0..p] and RHS g[0..p]
    let mut a_r = vec![0.0f64; p];
    a_r[0] = diag;
    for k in 1..p {
        a_r[k] = *r.get(k).unwrap_or(&0.0);
    }

    if lag_samp == 0 {
        // Spiking decon: solve R * f = [1, 0, ..., 0] via Levinson-Durbin
        let mut f = vec![0.0f64; p];
        let mut f_prev = vec![0.0f64; p];
        f[0] = 1.0;
        let mut err = diag;
        for m in 1..p {
            let mut num = 0.0f64;
            for k in 0..m {
                num += f[k] * a_r[m - k];
            }
            if err.abs() <= 1e-24 {
                break;
            }
            let km = -num / err;
            f_prev[..m].copy_from_slice(&f[..m]);
            f[m] = km;
            for k in 1..m {
                f[k] = f_prev[k] + km * f_prev[m - k];
            }
            err *= (1.0 - km * km).max(1e-12);
        }
        // Normalize unit zero-lag gain (or energy preservation)
        let scale = (r0 / err.max(1e-18)).sqrt().min(10.0);
        for v in &mut f {
            *v *= scale;
        }
        f
    } else {
        // Predictive decon with gap `lag_samp`: solve R * a = r[lag..lag+p], output error filter `[1, 0..0, -a]`
        let mut rhs = vec![0.0f64; p];
        for k in 0..p {
            rhs[k] = *r.get(lag_samp + k).unwrap_or(&0.0);
        }
        // General symmetric Toeplitz Levinson solver for R * x = rhs
        let mut e_vec = vec![0.0f64; p];
        let mut e_prev = vec![0.0f64; p];
        let mut x = vec![0.0f64; p];
        e_vec[0] = 1.0;
        let mut err = a_r[0];
        x[0] = rhs[0] / err;

        for m in 1..p {
            let mut num = 0.0f64;
            for k in 0..m {
                num += e_vec[k] * a_r[m - k];
            }
            let km = -num / err.max(1e-24);
            e_prev[..m].copy_from_slice(&e_vec[..m]);
            e_vec[m] = km;
            for k in 1..m {
                e_vec[k] = e_prev[k] + km * e_prev[m - k];
            }
            err *= (1.0 - km * km).max(1e-12);

            let mut pred_rhs = 0.0f64;
            for k in 0..m {
                pred_rhs += x[k] * a_r[m - k];
            }
            let alpha = (rhs[m] - pred_rhs) / err.max(1e-24);
            x[m] = alpha;
            for k in 0..m {
                x[k] += alpha * e_vec[m - k];
            }
        }
        let mut pef = vec![0.0f64; lag_samp + p];
        pef[0] = 1.0;
        for k in 0..p {
            pef[lag_samp + k] = -x[k];
        }
        pef
    }
}

/// Apply Surface-Consistent Deconvolution (`scdecon`):
/// Decomposes per-trace log amplitude spectra across Source, Receiver, and Offset domains via
/// Gauss-Seidel, reconstructs surface-consistent power spectra -> autocorrelations -> Wiener-Levinson
/// minimum-phase operators per `(source, receiver)` pair, and convolves each trace.
pub fn apply_sc_decon(traces: &mut [Trace], params: &ScDeconParams) -> Result<()> {
    if traces.is_empty() {
        return Err(SeismicError::InvalidFormat("Empty traces for scdecon".into()));
    }
    let dt_ms = traces[0].dt_ms().max(1e-3);
    let nt = traces[0].samples.len();
    let i0 = ((params.gate_start_ms / dt_ms).round().max(0.0) as usize).min(nt.saturating_sub(1));
    let i1 = ((params.gate_end_ms / dt_ms).round().max((i0 + 16) as f32) as usize).min(nt);
    let win_len = (i1 - i0).max(16);
    let nfft = (win_len * 2).next_power_of_two().max(128);
    let n_pos = nfft / 2 + 1;

    let mut planner = FftPlanner::<f64>::new();
    let fft_fwd = planner.plan_fft_forward(nfft);
    let fft_inv = planner.plan_fft_inverse(nfft);

    // Pass 1: Compute per-trace log power spectra in parallel
    let trace_specs: Vec<Option<(TraceKeys, Vec<f64>)>> = traces
        .par_iter()
        .map(|tr| {
            if !tr.is_live() {
                return None;
            }
            let mut buf = vec![Complex::new(0.0, 0.0); nfft];
            let mut energy = 0.0f64;
            for j in 0..win_len {
                let s = tr.samples[i0 + j] as f64;
                let hann = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * j as f64 / (win_len - 1).max(1) as f64).cos());
                let v = s * hann;
                energy += v * v;
                buf[j] = Complex::new(v, 0.0);
            }
            if energy <= 1e-24 {
                return None;
            }
            fft_fwd.process(&mut buf);
            let floor = 1e-12 * energy;
            let log_pow: Vec<f64> = (0..n_pos).map(|k| (buf[k].norm_sqr() + floor).ln()).collect();
            Some((extract_keys(tr, params.offset_bin_m), log_pow))
        })
        .collect();

    let live_specs: Vec<(TraceKeys, Vec<f64>)> = trace_specs.into_iter().flatten().collect();
    if live_specs.is_empty() {
        return Err(SeismicError::InvalidFormat("No live traces in scdecon gate".into()));
    }

    let solve_flags = [
        params.decon_source,
        params.decon_receiver,
        params.solve_offset,
        false,
        false,
    ];

    // Pass 2: Frequency-by-frequency Gauss-Seidel log-spectrum decomposition in parallel
    let freq_comps: Vec<(f64, BTreeMap<i32, f64>, BTreeMap<i32, f64>, BTreeMap<i32, f64>)> = (0..n_pos)
        .into_par_iter()
        .map(|kf| {
            let obs: Vec<(TraceKeys, f64)> = live_specs.iter().map(|(k, lp)| (*k, lp[kf])).collect();
            let (mu, comps, _) = gauss_seidel_decompose(&obs, solve_flags, params.iterations, 0.05);
            (mu, comps[0].clone(), comps[1].clone(), comps[2].clone())
        })
        .collect();

    // Build unique operator keys `(src_opt, rec_opt, ofb_opt)` and solve Wiener decon operator once per unique combo
    let op_len = ((params.operator_ms / dt_ms).round() as usize).clamp(4, 256);
    let lag_samp = if params.prediction_lag_ms <= dt_ms * 0.5 {
        0usize
    } else {
        (params.prediction_lag_ms / dt_ms).round() as usize
    };
    let wn_frac = (params.white_noise_pct.max(0.01) as f64) / 100.0;

    let mut unique_combos: Vec<(Option<i32>, Option<i32>, Option<i32>)> = traces
        .iter()
        .filter(|t| t.is_live())
        .map(|t| {
            let k = extract_keys(t, params.offset_bin_m);
            (
                params.decon_source.then_some(k.src),
                params.decon_receiver.then_some(k.rec),
                params.decon_offset.then_some(k.ofb),
            )
        })
        .collect();
    unique_combos.sort_unstable();
    unique_combos.dedup();

    let inv_n = 1.0 / nfft as f64;
    let ops_vec: Vec<((Option<i32>, Option<i32>, Option<i32>), Vec<f64>)> = unique_combos
        .par_iter()
        .map(|&combo| {
            let mut pow_spec = vec![Complex::new(0.0, 0.0); nfft];
            for kf in 0..n_pos {
                let (mu, ref smap, ref rmap, ref omap) = freq_comps[kf];
                let mut lp = mu;
                if let Some(s) = combo.0 { lp += *smap.get(&s).unwrap_or(&0.0); }
                if let Some(r) = combo.1 { lp += *rmap.get(&r).unwrap_or(&0.0); }
                if let Some(o) = combo.2 { lp += *omap.get(&o).unwrap_or(&0.0); }
                let p = lp.clamp(-40.0, 40.0).exp();
                pow_spec[kf] = Complex::new(p, 0.0);
                if kf > 0 && kf < nfft - kf {
                    pow_spec[nfft - kf] = Complex::new(p, 0.0);
                }
            }
            // Inverse FFT of power spectrum -> autocorrelation r[0..op_len+lag]
            fft_inv.process(&mut pow_spec);
            let need = (op_len + lag_samp + 2).min(nfft / 2);
            let r: Vec<f64> = (0..need).map(|k| pow_spec[k].re * inv_n).collect();
            let filt = solve_real_wiener_decon(&r, op_len, lag_samp, wn_frac);
            (combo, filt)
        })
        .collect();

    let op_map: BTreeMap<(Option<i32>, Option<i32>, Option<i32>), Vec<f64>> = ops_vec.into_iter().collect();

    // Pass 3: Convolve each trace with its surface-consistent operator (RMS-preserving)
    traces.par_iter_mut().for_each(|tr| {
        if !tr.is_live() {
            return;
        }
        let k = extract_keys(tr, params.offset_bin_m);
        let combo = (
            params.decon_source.then_some(k.src),
            params.decon_receiver.then_some(k.rec),
            params.decon_offset.then_some(k.ofb),
        );
        let Some(op) = op_map.get(&combo) else {
            return;
        };
        let n = tr.samples.len();
        let in_rms = (tr.samples.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / n.max(1) as f64).sqrt();
        let mut out = vec![0.0f64; n];
        for i in 0..n {
            let mut acc = 0.0f64;
            let kmax = op.len().min(i + 1);
            for j in 0..kmax {
                acc += op[j] * tr.samples[i - j] as f64;
            }
            out[i] = acc;
        }
        let out_rms = (out.iter().map(|&s| s * s).sum::<f64>() / n.max(1) as f64).sqrt();
        let scale = if out_rms > 1e-18 && in_rms > 1e-18 { in_rms / out_rms } else { 1.0 };
        for i in 0..n {
            tr.samples[i] = (out[i] * scale) as f32;
        }
    });

    Ok(())
}
