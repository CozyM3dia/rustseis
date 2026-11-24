use rayon::prelude::*;
use rustseis_core::{MuteTable, Result, SeismicError, Trace};
use serde::{Deserialize, Serialize};

/// Wiener-Levinson Spiking / Predictive Deconvolution (`decon`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeconParams {
    /// Operator length in milliseconds (e.g., 120–240 ms).
    pub operator_ms: f32,
    /// Prediction lag / gap in milliseconds. `0.0` or `<= dt_ms` = spiking decon; `> dt_ms` = predictive decon.
    #[serde(default)]
    pub prediction_lag_ms: f32,
    /// Percent pre-whitening added to autocorrelation lag 0 (default 0.1%).
    #[serde(default = "default_white_noise")]
    pub white_noise_pct: f32,
    /// Optional design windows `(t_start_ms, t_end_ms)`. Empty = use full live trace window.
    #[serde(default)]
    pub design_windows_ms: Vec<(f32, f32)>,
    /// Optional cosine taper length (ms) applied to both ends of design window before autocorrelation.
    #[serde(default)]
    pub window_taper_ms: f32,
    /// Output amplitude scaling factor (default `1.0`; use `0.5` for exact ProMAX Spiking Decon amplitude parity).
    #[serde(default = "default_decon_scale")]
    pub output_scale: f32,
}

fn default_white_noise() -> f32 {
    0.1
}
fn default_decon_scale() -> f32 {
    1.0
}

impl Default for DeconParams {
    fn default() -> Self {
        Self {
            operator_ms: 160.0,
            prediction_lag_ms: 0.0,
            white_noise_pct: 0.1,
            design_windows_ms: Vec::new(),
            window_taper_ms: 0.0,
            output_scale: 1.0,
        }
    }
}

/// Solve symmetric positive-definite Toeplitz system `R * a = g` via Levinson-Durbin recursion.
fn levinson_durbin(r: &[f64], g: &[f64]) -> Vec<f64> {
    let n = g.len();
    if n == 0 || r[0].abs() < 1e-24 {
        return vec![0.0; n];
    }
    let mut a = vec![0.0f64; n]; // solution vector
    let mut f = vec![0.0f64; n]; // forward/backward reflection filter
    let mut f_next = vec![0.0f64; n];

    f[0] = 1.0 / r[0];
    a[0] = g[0] / r[0];

    for k in 1..n {
        // Compute reflection coefficient
        let mut ef = 0.0f64;
        for j in 0..k {
            ef += f[j] * r[k - j];
        }
        let denom = 1.0 - ef * ef;
        if denom.abs() < 1e-15 {
            break;
        }
        // Update reflection vector f
        f_next[0] = f[0] / denom;
        for j in 1..k {
            f_next[j] = (f[j] - ef * f[k - j]) / denom;
        }
        f_next[k] = (-ef * f[0]) / denom;
        f[..=k].copy_from_slice(&f_next[..=k]);

        // Update solution a
        let mut ea = 0.0f64;
        for j in 0..k {
            ea += a[j] * r[k - j];
        }
        let err = g[k] - ea;
        for j in 0..=k {
            a[j] += err * f[k - j];
        }
    }
    a
}

fn design_operator(samples: &[f32], op_len: usize, lag: usize, white_noise_pct: f32, taper_samples: usize) -> Vec<f64> {
    let max_lag = op_len + lag;
    if samples.len() <= max_lag {
        return Vec::new();
    }
    let tapered: Vec<f64> = if taper_samples > 0 && taper_samples * 2 < samples.len() {
        let n = samples.len();
        samples
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let w = if i < taper_samples {
                    0.5 * (1.0 - (std::f64::consts::PI * i as f64 / taper_samples as f64).cos())
                } else if i >= n - taper_samples {
                    let k = n - 1 - i;
                    0.5 * (1.0 - (std::f64::consts::PI * k as f64 / taper_samples as f64).cos())
                } else {
                    1.0
                };
                v as f64 * w
            })
            .collect()
    } else {
        samples.iter().map(|&v| v as f64).collect()
    };

    let mut ac = vec![0.0f64; max_lag + 1];
    for l in 0..=max_lag {
        let mut s = 0.0f64;
        for i in 0..(tapered.len() - l) {
            s += tapered[i] * tapered[i + l];
        }
        ac[l] = s;
    }
    if ac[0] <= 1e-24 {
        return Vec::new();
    }
    // Pre-whitening
    ac[0] *= 1.0 + (white_noise_pct.max(0.0) as f64 / 100.0);

    // Solve R * p = r_shifted where r_shifted[i] = ac[lag + i]
    let g: Vec<f64> = (0..op_len).map(|i| ac[lag + i]).collect();
    levinson_durbin(&ac[..op_len], &g)
}

fn apply_prediction_error(src: &[f32], pred: &[f64], lag: usize, scale: f32) -> Vec<f32> {
    let n = src.len();
    if pred.is_empty() {
        return src.to_vec();
    }
    let op_len = pred.len();
    let sc = scale as f64;
    let mut out = vec![0.0f32; n];
    for i in 0..n {
        let mut predicted = 0.0f64;
        for k in 0..op_len {
            let idx = i as isize - (lag + k) as isize;
            if idx >= 0 {
                predicted += pred[k] * src[idx as usize] as f64;
            }
        }
        out[i] = ((src[i] as f64 - predicted) * sc) as f32;
    }
    out
}

pub fn apply_decon_trace(trace: &mut Trace, params: &DeconParams) -> Result<()> {
    if !trace.is_live() || trace.samples.is_empty() {
        return Ok(());
    }
    let dt_ms = trace.dt_ms().max(1e-6);
    let op_len = ((params.operator_ms / dt_ms).round() as usize).max(2);
    let lag = if params.prediction_lag_ms <= dt_ms {
        1usize // Spiking decon (unit prediction lag = 1 sample)
    } else {
        (params.prediction_lag_ms / dt_ms).round() as usize
    };

    let (ls, le) = trace.live_sample_range();
    let n = trace.samples.len();

    let taper_samps = ((params.window_taper_ms.max(0.0) / dt_ms).round()) as usize;

    if params.design_windows_ms.len() <= 1 {
        let (w0, w1) = if let Some(&(t0, t1)) = params.design_windows_ms.first() {
            (
                ((t0 / dt_ms).floor() as usize).max(ls).min(n),
                ((t1 / dt_ms).ceil() as usize + 1).min(le).min(n),
            )
        } else {
            (ls, le)
        };
        if w1 <= w0 + op_len + lag {
            return Ok(());
        }
        let pred = design_operator(&trace.samples[w0..w1], op_len, lag, params.white_noise_pct, taper_samps);
        trace.samples = apply_prediction_error(&trace.samples, &pred, lag, params.output_scale);
        return Ok(());
    }

    // Multi-window ProMAX linear taper interpolation between operators
    let mut wins = params.design_windows_ms.clone();
    wins.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut results = Vec::with_capacity(wins.len());
    for &(t0, t1) in &wins {
        let w0 = ((t0 / dt_ms).floor() as usize).max(ls).min(n);
        let w1 = ((t1 / dt_ms).ceil() as usize + 1).min(le).min(n);
        let pred = if w1 > w0 + op_len + lag {
            design_operator(&trace.samples[w0..w1], op_len, lag, params.white_noise_pct, taper_samps)
        } else {
            Vec::new()
        };
        results.push(apply_prediction_error(&trace.samples, &pred, lag, params.output_scale));
    }

    let mut blended = vec![0.0f32; n];
    for i in 0..n {
        let t = i as f32 * dt_ms;
        if t <= wins[0].1 {
            blended[i] = results[0][i];
        } else if t >= wins[wins.len() - 1].0 {
            blended[i] = results[wins.len() - 1][i];
        } else {
            let mut v = results[0][i];
            for w in 0..wins.len() - 1 {
                let end_a = wins[w].1;
                let start_b = wins[w + 1].0;
                if t >= wins[w].0 && t <= end_a {
                    v = results[w][i];
                    break;
                } else if t > end_a && t < start_b {
                    let frac = (t - end_a) / (start_b - end_a).max(1e-6);
                    v = (1.0 - frac) * results[w][i] + frac * results[w + 1][i];
                    break;
                }
            }
            blended[i] = v;
        }
    }
    trace.samples = blended;
    Ok(())
}

pub fn apply_decon(traces: &mut [Trace], params: &DeconParams) -> Result<()> {
    apply_decon_with_gate(traces, params, None)
}

/// Apply spiking or predictive decon, optionally resolving a per-trace design window `(t_start, t_end)`
/// from a picked surgical `MuteTable` (ProMAX Miscellaneous Time Gate).
pub fn apply_decon_with_gate(
    traces: &mut [Trace],
    params: &DeconParams,
    gate: Option<&MuteTable>,
) -> Result<()> {
    if params.operator_ms <= 0.0 {
        return Err(SeismicError::InvalidParameter {
            param: "operator_ms".into(),
            message: "Decon operator length must be > 0".into(),
        });
    }
    match gate {
        None => traces.par_iter_mut().try_for_each(|tr| apply_decon_trace(tr, params)),
        Some(gt) => traces.par_iter_mut().try_for_each(|tr| {
            let key = tr.headers.get_f64(&gt.key_header).unwrap_or(tr.headers.aoffset as f64) as f32;
            if let Some((t0, t1)) = gt.surgical_window_at(key) {
                let mut local = params.clone();
                local.design_windows_ms = vec![(t0.min(t1), t0.max(t1))];
                apply_decon_trace(tr, &local)
            } else {
                apply_decon_trace(tr, params)
            }
        }),
    }
}

/// Compute normalized average autocorrelation `(lags_ms, corr)` across live traces for Decon QC
/// (inspecting first zero-crossing / second zero-crossing for prediction lag and reverberation decay for operator length).
pub fn compute_average_autocorrelation(
    traces: &[Trace],
    max_lag_ms: f32,
    window_ms: Option<(f32, f32)>,
) -> (Vec<f32>, Vec<f32>) {
    let Some(first) = traces.iter().find(|t| t.is_live() && !t.samples.is_empty()) else {
        return (Vec::new(), Vec::new());
    };
    let dt_ms = first.dt_ms().max(1e-6);
    let max_lag = ((max_lag_ms / dt_ms).round() as usize).clamp(4, 2000);
    let mut acc = vec![0.0f64; max_lag + 1];
    let mut used = 0usize;
    for tr in traces.iter().filter(|t| t.is_live() && !t.samples.is_empty()) {
        let (ls, le) = tr.live_sample_range();
        let n = tr.samples.len();
        let (w0, w1) = match window_ms {
            Some((a, b)) => (
                ((a.min(b) / dt_ms).floor() as usize).max(ls).min(n),
                ((a.max(b) / dt_ms).ceil() as usize + 1).min(le).min(n),
            ),
            None => (ls, le),
        };
        if w1 <= w0 + 4 {
            continue;
        }
        let s = &tr.samples[w0..w1];
        let lim = max_lag.min(s.len().saturating_sub(1));
        let mut r0 = 0.0f64;
        for &v in s {
            r0 += (v as f64) * (v as f64);
        }
        if r0 <= 1e-24 {
            continue;
        }
        let inv = 1.0 / r0;
        for l in 0..=lim {
            let mut sum = 0.0f64;
            for i in 0..(s.len() - l) {
                sum += (s[i] as f64) * (s[i + l] as f64);
            }
            acc[l] += sum * inv;
        }
        used += 1;
    }
    if used == 0 {
        return (Vec::new(), Vec::new());
    }
    let lags: Vec<f32> = (0..=max_lag).map(|l| l as f32 * dt_ms).collect();
    let norm = acc[0].abs().max(1e-12);
    let corr: Vec<f32> = acc.iter().map(|&v| (v / norm) as f32).collect();
    (lags, corr)
}
