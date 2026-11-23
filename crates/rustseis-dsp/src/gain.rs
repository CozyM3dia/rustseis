use rayon::prelude::*;
use rustseis_core::{Trace, VelocityTable};
use serde::{Deserialize, Serialize};

/// ProMAX True Amplitude Recovery (`tar`) parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TarParams {
    /// Power of time gain `t^time_power` (e.g., 1.5 or 2.0). `0.0` disables.
    #[serde(default)]
    pub time_power: f32,
    /// dB/second gain (`10^(db_per_sec * t / 20)`). `0.0` disables.
    #[serde(default)]
    pub db_per_sec: f32,
    /// Spherical divergence basis (`None`, `OneOverDist` -> `t*v(t)`, `OneOverTV2` -> `t*v(t)^2`).
    #[serde(default)]
    pub spherical: SphericalBasis,
    /// Inelastic attenuation constant `alpha` (`exp(alpha * t * v(t))`).
    #[serde(default)]
    pub inelastic_alpha: f32,
    /// Maximum time (ms) to apply gain; held constant afterwards. `None` = full trace.
    #[serde(default)]
    pub max_time_ms: Option<f32>,
    /// Optional ProMAX normalization reference time (ms): divides gain curve by `gain(ref_time)`.
    /// Use `Some(0.0)` to normalize at trace midpoint (ProMAX `NORM_SRC = Reference Time` default).
    #[serde(default)]
    pub normalize_ref_time_ms: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SphericalBasis {
    #[default]
    None,
    /// `g(t) = t * v(t)`
    OneOverDist,
    /// `g(t) = t * v(t)^2`
    OneOverTV2,
}

impl Default for TarParams {
    fn default() -> Self {
        Self {
            time_power: 1.5,
            db_per_sec: 0.0,
            spherical: SphericalBasis::None,
            inelastic_alpha: 0.0,
            max_time_ms: None,
            normalize_ref_time_ms: None,
        }
    }
}

pub fn apply_tar_trace(trace: &mut Trace, params: &TarParams, vel: Option<&VelocityTable>) {
    if !trace.is_live() || trace.samples.is_empty() {
        return;
    }
    let dt_s = trace.dt_s();
    let max_t_s = params.max_time_ms.map(|m| m / 1000.0);
    let cdp = trace.headers.cdp;
    let v0 = vel.map(|vt| vt.velocity_at(cdp, 0.0)).unwrap_or(1500.0).max(1.0);

    let eval_gain = |raw_t: f32| -> f64 {
        let t = match max_t_s {
            Some(mx) => raw_t.min(mx),
            None => raw_t,
        };
        let mut g = 1.0f64;
        if params.time_power != 0.0 {
            g *= (t as f64).powf(params.time_power as f64);
        }
        if params.db_per_sec != 0.0 {
            g *= 10.0_f64.powf((params.db_per_sec as f64 * t as f64) / 20.0);
        }
        let v = vel.map(|vt| vt.velocity_at(cdp, t * 1000.0)).unwrap_or(1500.0).max(1.0);
        match params.spherical {
            SphericalBasis::None => {}
            SphericalBasis::OneOverDist => {
                g *= (t as f64) * (v as f64 / v0 as f64);
            }
            SphericalBasis::OneOverTV2 => {
                let vr = v as f64 / v0 as f64;
                g *= (t as f64) * vr * vr;
            }
        }
        if params.inelastic_alpha != 0.0 {
            g *= (params.inelastic_alpha as f64 * t as f64 * v as f64).exp();
        }
        g
    };

    let norm = match params.normalize_ref_time_ms {
        Some(ref_ms) => {
            let t_ref_s = if ref_ms <= 0.0 {
                // ProMAX convention: 0.0 defaults to middle sample of trace
                ((trace.samples.len() - 1) / 2) as f32 * dt_s
            } else {
                ref_ms / 1000.0
            };
            eval_gain(t_ref_s.max(dt_s)).max(1e-12)
        }
        None => 1.0,
    };

    for (i, s) in trace.samples.iter_mut().enumerate() {
        let raw_t = if params.db_per_sec != 0.0 && params.time_power == 0.0 {
            i as f32 * dt_s
        } else {
            (i as f32 * dt_s).max(dt_s)
        };
        let g = eval_gain(raw_t) / norm;
        *s = (*s as f64 * g) as f32;
    }
}

pub fn apply_tar(traces: &mut [Trace], params: &TarParams, vel: Option<&VelocityTable>) {
    traces.par_iter_mut().for_each(|tr| apply_tar_trace(tr, params, vel));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgcBasis {
    #[default]
    Rms,
    Mean,
    Median,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgcAnchor {
    Start,
    #[default]
    Center,
    End,
}

/// ProMAX Automatic Gain Control (`agc`) parameters (exact growing/constant/shrinking window).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgcParams {
    pub operator_ms: f32,
    #[serde(default)]
    pub basis: AgcBasis,
    #[serde(default)]
    pub anchor: AgcAnchor,
    /// Target amplitude scalar (default 1.0).
    #[serde(default = "default_one")]
    pub target_amp: f32,
}

fn default_one() -> f32 {
    1.0
}

impl Default for AgcParams {
    fn default() -> Self {
        Self {
            operator_ms: 500.0,
            basis: AgcBasis::Rms,
            anchor: AgcAnchor::Center,
            target_amp: 1.0,
        }
    }
}

pub fn apply_agc_trace(trace: &mut Trace, params: &AgcParams) {
    if !trace.is_live() || trace.samples.is_empty() {
        return;
    }
    let dt_ms = trace.dt_ms().max(1e-6);
    let win_len = ((params.operator_ms / dt_ms).round() as usize).max(1);
    let (live_s, live_e) = trace.live_sample_range();
    let n = live_e - live_s;
    if n == 0 {
        return;
    }
    let src = trace.samples[live_s..live_e].to_vec();
    let mut out = vec![0.0f32; n];
    let half = win_len / 2;

    match params.basis {
        AgcBasis::Rms | AgcBasis::Mean => {
            // Prefix-sum O(N) exact growing/constant/shrinking window
            let mut prefix = vec![0.0f64; n + 1];
            for i in 0..n {
                let v = src[i] as f64;
                prefix[i + 1] = prefix[i]
                    + match params.basis {
                        AgcBasis::Rms => v * v,
                        AgcBasis::Mean => v.abs(),
                        AgcBasis::Median => unreachable!(),
                    };
            }
            for i in 0..n {
                let (lo, hi) = match params.anchor {
                    AgcAnchor::Center => (i.saturating_sub(half), (i + half + 1).min(n)),
                    AgcAnchor::Start => (i, (i + win_len).min(n)),
                    AgcAnchor::End => (i.saturating_sub(win_len - 1), (i + 1).min(n)),
                };
                let cnt = (hi - lo).max(1) as f64;
                let acc = prefix[hi] - prefix[lo];
                let stat = match params.basis {
                    AgcBasis::Rms => (acc / cnt).sqrt(),
                    AgcBasis::Mean => acc / cnt,
                    AgcBasis::Median => unreachable!(),
                };
                out[i] = if stat > 1e-12 {
                    (src[i] as f64 * (params.target_amp as f64 / stat)) as f32
                } else {
                    0.0
                };
            }
        }
        AgcBasis::Median => {
            let mut buf = Vec::with_capacity(win_len + 2);
            for i in 0..n {
                let (lo, hi) = match params.anchor {
                    AgcAnchor::Center => (i.saturating_sub(half), (i + half + 1).min(n)),
                    AgcAnchor::Start => (i, (i + win_len).min(n)),
                    AgcAnchor::End => (i.saturating_sub(win_len - 1), (i + 1).min(n)),
                };
                buf.clear();
                for &v in &src[lo..hi] {
                    buf.push(v.abs());
                }
                buf.sort_by(|a, b| a.total_cmp(b));
                let med = buf[buf.len() / 2] as f64;
                out[i] = if med > 1e-12 {
                    (src[i] as f64 * (params.target_amp as f64 / med)) as f32
                } else {
                    0.0
                };
            }
        }
    }

    trace.samples[live_s..live_e].copy_from_slice(&out);
}

pub fn apply_agc(traces: &mut [Trace], params: &AgcParams) {
    traces.par_iter_mut().for_each(|tr| apply_agc_trace(tr, params));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TeqBasis {
    #[default]
    Rms,
    Mean,
    Peak,
}

/// Trace Equalization (`teq`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeqParams {
    #[serde(default)]
    pub basis: TeqBasis,
    #[serde(default = "default_one")]
    pub target_amp: f32,
    #[serde(default)]
    pub gate_start_ms: Option<f32>,
    #[serde(default)]
    pub gate_end_ms: Option<f32>,
}

impl Default for TeqParams {
    fn default() -> Self {
        Self {
            basis: TeqBasis::Rms,
            target_amp: 1.0,
            gate_start_ms: None,
            gate_end_ms: None,
        }
    }
}

pub fn apply_teq(traces: &mut [Trace], params: &TeqParams) {
    traces.par_iter_mut().for_each(|tr| {
        if !tr.is_live() || tr.samples.is_empty() {
            return;
        }
        let dt_ms = tr.dt_ms().max(1e-6);
        let (ls, le) = tr.live_sample_range();
        let gs = params
            .gate_start_ms
            .map(|m| ((m / dt_ms).floor() as usize).max(ls))
            .unwrap_or(ls)
            .min(tr.samples.len());
        let ge = params
            .gate_end_ms
            .map(|m| ((m / dt_ms).ceil() as usize + 1).min(le))
            .unwrap_or(le)
            .min(tr.samples.len());
        if gs >= ge {
            return;
        }
        let sl = &tr.samples[gs..ge];
        let stat = match params.basis {
            TeqBasis::Rms => {
                (sl.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / sl.len() as f64).sqrt()
            }
            TeqBasis::Mean => {
                sl.iter().map(|&v| (v as f64).abs()).sum::<f64>() / sl.len() as f64
            }
            TeqBasis::Peak => sl.iter().fold(0.0f64, |m, &v| m.max((v as f64).abs())),
        };
        if stat > 1e-12 {
            let scale = (params.target_amp as f64 / stat) as f32;
            for s in &mut tr.samples {
                *s *= scale;
            }
            tr.headers.amp_norm *= scale;
        }
    });
}

/// Time-Variant Scaling (`tvs`): normalize RMS amplitude across user time gates with linear interpolation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TvsParams {
    /// Gate windows `(t_start_ms, t_end_ms)`.
    pub gates_ms: Vec<(f32, f32)>,
    #[serde(default = "default_one")]
    pub target_amp: f32,
}

pub fn apply_tvs(traces: &mut [Trace], params: &TvsParams) {
    if params.gates_ms.is_empty() {
        return;
    }
    traces.par_iter_mut().for_each(|tr| {
        if !tr.is_live() || tr.samples.is_empty() {
            return;
        }
        let dt_ms = tr.dt_ms().max(1e-6);
        let n = tr.samples.len();
        let mut knots: Vec<(f32, f32)> = Vec::new();
        for &(g0, g1) in &params.gates_ms {
            let i0 = ((g0 / dt_ms).floor() as usize).min(n);
            let i1 = ((g1 / dt_ms).ceil() as usize + 1).min(n);
            if i0 >= i1 {
                continue;
            }
            let rms = (tr.samples[i0..i1]
                .iter()
                .map(|&v| (v as f64) * (v as f64))
                .sum::<f64>()
                / (i1 - i0) as f64)
                .sqrt();
            let gain = if rms > 1e-12 {
                (params.target_amp as f64 / rms) as f32
            } else {
                1.0
            };
            knots.push(((g0 + g1) * 0.5, gain));
        }
        if knots.is_empty() {
            return;
        }
        knots.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (i, s) in tr.samples.iter_mut().enumerate() {
            let t = i as f32 * dt_ms;
            let g = if t <= knots[0].0 {
                knots[0].1
            } else if t >= knots[knots.len() - 1].0 {
                knots[knots.len() - 1].1
            } else {
                let mut val = knots[0].1;
                for w in knots.windows(2) {
                    if t >= w[0].0 && t <= w[1].0 {
                        let frac = (t - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
                        val = w[0].1 + frac * (w[1].1 - w[0].1);
                        break;
                    }
                }
                val
            };
            *s *= g;
        }
    });
}
