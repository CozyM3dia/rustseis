use rayon::prelude::*;
use rustseis_core::{Ensemble, Trace, TraceHeaders, TraceType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StackMethod {
    /// Standard fold-normalized arithmetic mean: `sum / fold^norm_power`.
    #[default]
    Mean,
    /// Sample-by-sample median stack (robust to noise spikes).
    Median,
    /// Inverse-variance (diversity) weighted stack.
    Diversity,
}

/// ProMAX CDP/Ensemble Stack (`stack`) parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackParams {
    #[serde(default)]
    pub method: StackMethod,
    /// Fold normalization exponent (`1.0` = true mean, `0.5` = sqrt-fold, `0.0` = raw sum).
    #[serde(default = "default_norm_power")]
    pub fold_norm_power: f32,
    /// Exclude exact-zero (muted) samples from fold denominator (ProMAX live-sample convention).
    #[serde(default = "default_true")]
    pub exclude_muted_zeros: bool,
}

fn default_norm_power() -> f32 {
    1.0
}
fn default_true() -> bool {
    true
}

impl Default for StackParams {
    fn default() -> Self {
        Self {
            method: StackMethod::Mean,
            fold_norm_power: 1.0,
            exclude_muted_zeros: true,
        }
    }
}

pub fn stack_single_ensemble(ens: &Ensemble, params: &StackParams) -> Option<Trace> {
    let live: Vec<&Trace> = ens.traces.iter().filter(|t| t.is_live() && !t.samples.is_empty()).collect();
    if live.is_empty() {
        return None;
    }
    let dt_us = live[0].dt_us;
    let ns = live[0].samples.len();
    let mut out = vec![0.0f32; ns];

    match params.method {
        StackMethod::Mean => {
            for i in 0..ns {
                let mut sum = 0.0f64;
                let mut fold = 0usize;
                for tr in &live {
                    if i >= tr.samples.len() {
                        continue;
                    }
                    let v = tr.samples[i];
                    if params.exclude_muted_zeros && v == 0.0 {
                        continue;
                    }
                    sum += v as f64;
                    fold += 1;
                }
                if fold > 0 {
                    let denom = (fold as f64).powf(params.fold_norm_power as f64);
                    out[i] = (sum / denom) as f32;
                }
            }
        }
        StackMethod::Median => {
            let mut buf = Vec::with_capacity(live.len());
            for i in 0..ns {
                buf.clear();
                for tr in &live {
                    if i >= tr.samples.len() {
                        continue;
                    }
                    let v = tr.samples[i];
                    if params.exclude_muted_zeros && v == 0.0 {
                        continue;
                    }
                    buf.push(v);
                }
                if !buf.is_empty() {
                    buf.sort_by(|a, b| a.total_cmp(b));
                    out[i] = buf[buf.len() / 2];
                }
            }
        }
        StackMethod::Diversity => {
            // Inverse-power weighted stack per trace
            let weights: Vec<f64> = live
                .iter()
                .map(|tr| {
                    let pwr = tr.samples.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>()
                        / tr.samples.len().max(1) as f64;
                    if pwr > 1e-20 { 1.0 / pwr } else { 0.0 }
                })
                .collect();
            for i in 0..ns {
                let mut num = 0.0f64;
                let mut wsum = 0.0f64;
                for (ti, tr) in live.iter().enumerate() {
                    if i >= tr.samples.len() {
                        continue;
                    }
                    let v = tr.samples[i];
                    if params.exclude_muted_zeros && v == 0.0 {
                        continue;
                    }
                    num += weights[ti] * v as f64;
                    wsum += weights[ti];
                }
                out[i] = if wsum > 1e-20 { (num / wsum) as f32 } else { 0.0 };
            }
        }
    }

    let first_h = &live[0].headers;
    let avg_x = live.iter().map(|t| t.headers.cdp_x).sum::<f64>() / live.len() as f64;
    let avg_y = live.iter().map(|t| t.headers.cdp_y).sum::<f64>() / live.len() as f64;
    let avg_elev = live.iter().map(|t| t.headers.cdp_elev as f64).sum::<f64>() / live.len() as f64;
    let len_ms = if ns > 1 { (ns - 1) as f32 * (dt_us as f32 / 1000.0) } else { 0.0 };

    let mut h = TraceHeaders::default();
    h.cdp = first_h.cdp;
    h.cdp_sloc = first_h.cdp_sloc;
    h.cdp_x = avg_x;
    h.cdp_y = avg_y;
    h.cdp_elev = avg_elev as f32;
    h.tr_fold = live.len() as i32;
    h.cdp_nfld = live.len() as i32;
    h.offset = 0.0;
    h.aoffset = 0.0;
    h.trc_type = TraceType::Live as i32;
    h.tlive_s = 0.0;
    h.tfull_s = 0.0;
    h.tfull_e = len_ms;
    h.tlive_e = len_ms;

    Some(Trace {
        dt_us,
        headers: h,
        samples: out,
    })
}

/// Stack a list of ensembles (typically CDP gathers) into a 2D stacked section (`stack`).
pub fn stack_ensembles(ensembles: &[Ensemble], params: &StackParams) -> Vec<Trace> {
    let mut stacked: Vec<Trace> = ensembles
        .par_iter()
        .filter_map(|e| stack_single_ensemble(e, params))
        .collect();
    let n = stacked.len();
    for (i, tr) in stacked.iter_mut().enumerate() {
        tr.headers.traceno = (i + 1) as i32;
        tr.headers.seqno = (i + 1) as i32;
        tr.headers.end_ens = if i + 1 == n { 1 } else { 0 };
        tr.headers.eoj = if i + 1 == n { 1 } else { 0 };
    }
    stacked
}
