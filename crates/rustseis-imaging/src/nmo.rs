use rayon::prelude::*;
use rustseis_core::{Trace, VelocityTable};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;
use std::sync::LazyLock;

const SINC_LUT_STEPS: usize = 1024;

/// L1-cache-resident pre-normalized 8-point Hann-windowed sinc table (32 KB).
/// Eliminates 16 transcendental sin/cos calls per interpolated sample across NMO, Kirchhoff, and PSTM.
static SINC_LUT: LazyLock<Box<[[f32; 8]; SINC_LUT_STEPS + 1]>> = LazyLock::new(|| {
    let mut lut = Box::new([[0.0f32; 8]; SINC_LUT_STEPS + 1]);
    for s in 0..=SINC_LUT_STEPS {
        let frac = s as f64 / SINC_LUT_STEPS as f64;
        let mut w = [0.0f64; 8];
        let mut wsum = 0.0f64;
        for t in 0..8 {
            // tap t corresponds to sample offset (t as isize - 3) relative to floor(pos)
            let x = frac - (t as f64 - 3.0);
            let sinc = if x.abs() < 1e-12 {
                1.0
            } else {
                let px = PI * x;
                px.sin() / px
            };
            let win = 0.5 * (1.0 + (PI * x / 5.0).cos());
            w[t] = sinc * win;
            wsum += w[t];
        }
        let inv = if wsum.abs() > 1e-12 { 1.0 / wsum } else { 1.0 };
        for t in 0..8 {
            lut[s][t] = (w[t] * inv) as f32;
        }
    }
    lut
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NmoDirection {
    #[default]
    Forward,
    Inverse,
}

/// ProMAX Normal Moveout Correction (`nmo`) parameters (matches `nmo.menu` & `nmo.pdf`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NmoParams {
    #[serde(default)]
    pub direction: NmoDirection,
    /// Stretch mute percentage (e.g., `30.0` mutes samples stretched > 30%; `0.0` disables).
    #[serde(default = "default_stretch")]
    pub stretch_mute_pct: f32,
    /// Taper length in samples for stretch mute ramp (default 8 samples).
    #[serde(default = "default_taper_samples")]
    pub stretch_taper_samples: usize,
    /// Apply remaining `NA_STAT` header static during NMO (ProMAX `DO_STAT` default true).
    #[serde(default = "default_true")]
    pub do_stat: bool,
}

fn default_stretch() -> f32 {
    30.0
}
fn default_taper_samples() -> usize {
    8
}
fn default_true() -> bool {
    true
}

impl Default for NmoParams {
    fn default() -> Self {
        Self {
            direction: NmoDirection::Forward,
            stretch_mute_pct: 30.0,
            stretch_taper_samples: 8,
            do_stat: true,
        }
    }
}

/// 8-point Hann-windowed sinc interpolation at fractional sample position `pos` (L1-cached LUT fast path).
#[inline]
pub(crate) fn sinc_interp(samples: &[f32], pos: f64) -> f32 {
    let n = samples.len();
    if pos < 0.0 || pos > (n - 1) as f64 {
        return 0.0;
    }
    let idx = pos.floor() as isize;
    let frac = pos - idx as f64;
    if frac < 1e-7 {
        return samples[idx as usize];
    }
    let lut_idx = (frac * SINC_LUT_STEPS as f64 + 0.5) as usize;
    let w = &SINC_LUT[lut_idx.min(SINC_LUT_STEPS)];
    if idx >= 3 && (idx as usize + 4) < n {
        let base = (idx - 3) as usize;
        let s = &samples[base..base + 8];
        return s[0] * w[0]
            + s[1] * w[1]
            + s[2] * w[2]
            + s[3] * w[3]
            + s[4] * w[4]
            + s[5] * w[5]
            + s[6] * w[6]
            + s[7] * w[7];
    }
    // Boundary fallback with edge renormalization
    let mut acc = 0.0f32;
    let mut wsum = 0.0f32;
    for t in 0..8 {
        let k = idx - 3 + t as isize;
        if k >= 0 && (k as usize) < n {
            acc += samples[k as usize] * w[t];
            wsum += w[t];
        }
    }
    if wsum.abs() > 1e-7 { acc / wsum } else { 0.0 }
}

pub fn apply_nmo_trace(trace: &mut Trace, vel: &VelocityTable, params: &NmoParams) {
    if !trace.is_live() || trace.samples.is_empty() {
        return;
    }
    let n = trace.samples.len();
    let dt_s = trace.dt_s() as f64;
    let dt_ms = trace.dt_ms() as f64;
    let x = trace.headers.aoffset as f64;
    let cdp = trace.headers.cdp;
    let stat_s = if params.do_stat {
        trace.headers.na_stat as f64 / 1000.0
    } else {
        0.0
    };

    let src = trace.samples.clone();
    let mut out = vec![0.0f32; n];
    let mut muted = vec![false; n];
    let stretch_limit = if params.stretch_mute_pct > 0.0 {
        Some(params.stretch_mute_pct as f64 / 100.0)
    } else {
        None
    };

    match params.direction {
        NmoDirection::Forward => {
            for i in 0..n {
                let t0 = i as f64 * dt_s;
                let v = vel.velocity_at(cdp, (t0 * 1000.0) as f32).max(100.0) as f64;
                let tx = (t0 * t0 + (x / v) * (x / v)).sqrt();
                let stretch = if t0 > 1e-6 { (tx - t0) / t0 } else { f64::INFINITY };
                if let Some(lim) = stretch_limit {
                    if stretch > lim {
                        muted[i] = true;
                        continue;
                    }
                }
                let sample_pos = (tx - stat_s) / dt_s;
                out[i] = sinc_interp(&src, sample_pos);
            }
            // Apply smooth stretch-mute cosine ramp
            if let Some(first_unmuted) = muted.iter().position(|&m| !m) {
                let taper = params.stretch_taper_samples;
                for k in 0..taper {
                    let idx = first_unmuted + k;
                    if idx < n {
                        let w = 0.5 * (1.0 - (PI * k as f64 / taper.max(1) as f64).cos());
                        out[idx] *= w as f32;
                    }
                }
                let t_live_ms = (first_unmuted as f64 * dt_ms) as f32;
                let t_full_ms = ((first_unmuted + taper) as f64 * dt_ms) as f32;
                trace.headers.tlive_s = trace.headers.tlive_s.max(t_live_ms);
                trace.headers.tfull_s = trace.headers.tfull_s.max(t_full_ms);
            }
            if params.do_stat {
                trace.headers.na_stat = 0.0;
            }
        }
        NmoDirection::Inverse => {
            // Inverse NMO: map NMO-corrected time t0 back to offset time tx
            for i in 0..n {
                let tx = i as f64 * dt_s;
                let v = vel.velocity_at(cdp, (tx * 1000.0) as f32).max(100.0) as f64;
                let t0_sq = tx * tx - (x / v) * (x / v);
                if t0_sq < 0.0 {
                    continue;
                }
                let t0 = t0_sq.sqrt();
                out[i] = sinc_interp(&src, (t0 + stat_s) / dt_s);
            }
        }
    }

    trace.samples = out;
}

pub fn apply_nmo(traces: &mut [Trace], vel: &VelocityTable, params: &NmoParams) {
    traces
        .par_iter_mut()
        .for_each(|tr| apply_nmo_trace(tr, vel, params));
}
