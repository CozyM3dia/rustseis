//! Crooked-line slalom binning QC, crossline-offset weighting, azimuth sector filtering,
//! and fold-normalized crooked-line stacking.

use rustseis_core::{Ensemble, Result, SeismicError, Trace};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrookedStackParams {
    /// Maximum crossline distance from slalom bin centre allowed into the stack (m).
    #[serde(default = "default_max_xline")]
    pub max_xline_dist_m: f32,
    /// Apply cosine crossline-offset taper weight `cos((pi/2) * |xline| / max_xline_dist_m)`.
    #[serde(default = "default_true")]
    pub xline_taper_weight: bool,
    /// Optional source-receiver azimuth filter `[min_deg, max_deg]` in `[0, 180]` (line-relative or compass).
    #[serde(default)]
    pub azimuth_range_deg: Option<[f32; 2]>,
    /// Normalize output stack by sum of weights (true) or raw count (false).
    #[serde(default = "default_true")]
    pub normalize_weights: bool,
}

fn default_max_xline() -> f32 { 300.0 }
fn default_true() -> bool { true }

impl Default for CrookedStackParams {
    fn default() -> Self {
        Self {
            max_xline_dist_m: default_max_xline(),
            xline_taper_weight: true,
            azimuth_range_deg: None,
            normalize_weights: true,
        }
    }
}

/// Source-to-receiver azimuth in `[0, 180)` degrees (bidirectional seismic azimuth).
#[inline]
fn trace_azimuth_deg(tr: &Trace) -> f32 {
    let dx = tr.headers.rec_x - tr.headers.sou_x;
    let dy = tr.headers.rec_y - tr.headers.sou_y;
    if dx.abs() + dy.abs() < 1e-6 {
        return 0.0;
    }
    let mut deg = dy.atan2(dx).to_degrees();
    while deg < 0.0 {
        deg += 180.0;
    }
    while deg >= 180.0 {
        deg -= 180.0;
    }
    deg as f32
}

/// Stack NMO-corrected crooked-line CDP gathers with crossline corridor rejection, cosine
/// crossline-distance weighting, and optional azimuth-sector filtering.
pub fn stack_crooked_line_2d(traces: &[Trace], params: &CrookedStackParams) -> Result<Vec<Trace>> {
    if traces.is_empty() {
        return Err(SeismicError::InvalidFormat("Crooked-line stack requires non-empty traces".into()));
    }
    let ensembles = Ensemble::group_by_header(traces.to_vec(), "CDP");
    let max_xl = params.max_xline_dist_m.max(1.0);
    let mut out = Vec::with_capacity(ensembles.len());

    for (idx, ens) in ensembles.into_iter().enumerate() {
        let Some(first) = ens.traces.first() else {
            continue;
        };
        let ns = first.samples.len();
        let mut acc = vec![0.0f64; ns];
        let mut w_sum = vec![0.0f64; ns];
        let mut n_kept = 0usize;
        let mut xl_acc = 0.0f64;
        let mut az_acc = 0.0f64;
        let mut cx_acc = 0.0f64;
        let mut cy_acc = 0.0f64;

        for tr in &ens.traces {
            if !tr.is_live() {
                continue;
            }
            let xline = tr
                .headers
                .custom
                .get("XLINE_DIST")
                .copied()
                .unwrap_or(0.0)
                .abs() as f32;
            if xline > max_xl {
                continue;
            }
            let az = trace_azimuth_deg(tr);
            if let Some([a0, a1]) = params.azimuth_range_deg {
                if az < a0 || az > a1 {
                    continue;
                }
            }
            let w = if params.xline_taper_weight {
                ((std::f32::consts::FRAC_PI_2 * (xline / max_xl)).cos()).max(0.05) as f64
            } else {
                1.0
            };
            for (i, &s) in tr.samples.iter().take(ns).enumerate() {
                if s != 0.0 && s.is_finite() {
                    acc[i] += w * s as f64;
                    w_sum[i] += w;
                }
            }
            xl_acc += xline as f64;
            az_acc += az as f64;
            cx_acc += tr.headers.cdp_x;
            cy_acc += tr.headers.cdp_y;
            n_kept += 1;
        }

        let samples: Vec<f32> = (0..ns)
            .map(|i| {
                if w_sum[i] > 1e-9 {
                    let den = if params.normalize_weights {
                        w_sum[i]
                    } else {
                        n_kept.max(1) as f64
                    };
                    (acc[i] / den) as f32
                } else {
                    0.0
                }
            })
            .collect();

        let mut tr = Trace::new(first.dt_us, samples);
        tr.headers = first.headers.clone();
        tr.headers.traceno = (idx + 1) as i32;
        tr.headers.ffid = 1;
        tr.headers.chan = (idx + 1) as i32;
        tr.headers.offset = 0.0;
        tr.headers.cdp_nfld = n_kept as i32;
        if n_kept > 0 {
            tr.headers.cdp_x = cx_acc / n_kept as f64;
            tr.headers.cdp_y = cy_acc / n_kept as f64;
            tr.headers
                .custom
                .insert("MEAN_XLINE_DIST".into(), xl_acc / n_kept as f64);
            tr.headers
                .custom
                .insert("MEAN_AZIMUTH_DEG".into(), az_acc / n_kept as f64);
        }
        out.push(tr);
    }
    Ok(out)
}
