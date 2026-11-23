use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Ensemble, Result, SeismicError};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FkMode {
    /// Reject energy inside velocity fan `|v_app| <= v_reject_mps` (e.g., ground roll / direct wave).
    #[default]
    RejectLowVelocity,
    /// Pass only energy with `|v_app| >= v_pass_mps`.
    PassHighVelocity,
    /// Reject dipping noise inside an explicit `(f_hz, k_cpm)` polygon.
    RejectPolygon,
    /// Pass energy inside an explicit `(f_hz, k_cpm)` polygon.
    PassPolygon,
}

/// 2D F-K Filter (`fk_filt`) parameters matching ProMAX cosine taper + amplitude pedestal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FkFilterParams {
    #[serde(default)]
    pub mode: FkMode,
    /// Trace spacing `dx` in meters (used to compute wavenumber `k` in cycles/m).
    pub dx_m: f32,
    /// Velocity boundary in m/s (for velocity fan modes: e.g., reject |f/k| < 1500 m/s).
    #[serde(default = "default_vel_cut")]
    pub vel_cut_mps: f32,
    /// Cosine transition width as fraction of `vel_cut_mps` (default 0.2).
    #[serde(default = "default_taper")]
    pub taper_frac: f32,
    /// Amplitude pedestal (ProMAX avoids hard zeros; default 0.01 = -40 dB).
    #[serde(default = "default_pedestal")]
    pub pedestal: f32,
    /// Optional polygon vertices `(freq_hz, wavenumber_cycles_per_m)` for polygon modes.
    #[serde(default)]
    pub polygon: Vec<(f32, f32)>,
}

fn default_vel_cut() -> f32 {
    1500.0
}
fn default_taper() -> f32 {
    0.2
}
fn default_pedestal() -> f32 {
    0.01
}

impl Default for FkFilterParams {
    fn default() -> Self {
        Self {
            mode: FkMode::RejectLowVelocity,
            dx_m: 12.5,
            vel_cut_mps: 1500.0,
            taper_frac: 0.2,
            pedestal: 0.01,
            polygon: Vec::new(),
        }
    }
}

fn point_in_polygon(px: f64, py: f64, poly: &[(f32, f32)]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = (poly[i].0 as f64, poly[i].1 as f64);
        let (xj, yj) = (poly[j].0 as f64, poly[j].1 as f64);
        let intersect = ((yi > py) != (yj > py))
            && (px < (xj - xi) * (py - yi) / ((yj - yi) + 1e-18) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Apply 2D F-K filter to a single ensemble/gather (`PanelProcessor`).
pub fn apply_fk_ensemble(ensemble: &mut Ensemble, params: &FkFilterParams) -> Result<()> {
    let nx = ensemble.traces.len();
    if nx < 2 {
        return Ok(());
    }
    let nt = ensemble.traces[0].samples.len();
    if nt == 0 {
        return Ok(());
    }
    if params.dx_m <= 0.0 {
        return Err(SeismicError::InvalidParameter {
            param: "dx_m".into(),
            message: "Trace spacing dx_m must be > 0".into(),
        });
    }

    let dt_s = ensemble.traces[0].dt_s() as f64;
    let dx = params.dx_m as f64;
    let nfft_t = (nt + nt / 4).next_power_of_two().max(64);
    let nfft_x = (nx + nx / 2).next_power_of_two().max(16);

    let mut planner = FftPlanner::<f64>::new();
    let fft_t = planner.plan_fft_forward(nfft_t);
    let ifft_t = planner.plan_fft_inverse(nfft_t);
    let fft_x = planner.plan_fft_forward(nfft_x);
    let ifft_x = planner.plan_fft_inverse(nfft_x);

    // 1. Forward FFT along time for each trace
    let mut grid = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];
    for ix in 0..nx {
        let row = &mut grid[ix * nfft_t..(ix + 1) * nfft_t];
        let tr = &ensemble.traces[ix];
        if tr.is_live() {
            for it in 0..nt.min(tr.samples.len()) {
                row[it] = Complex::new(tr.samples[it] as f64, 0.0);
            }
            fft_t.process(row);
        }
    }

    // 2. Forward FFT along space for each frequency bin
    let mut col = vec![Complex::new(0.0, 0.0); nfft_x];
    for ifreq in 0..nfft_t {
        for ix in 0..nfft_x {
            col[ix] = grid[ix * nfft_t + ifreq];
        }
        fft_x.process(&mut col);
        for ix in 0..nfft_x {
            grid[ix * nfft_t + ifreq] = col[ix];
        }
    }

    // 3. Apply F-K mask with cosine taper & pedestal
    let df = 1.0 / (nfft_t as f64 * dt_s);
    let dk = 1.0 / (nfft_x as f64 * dx);
    let v_cut = params.vel_cut_mps.max(1.0) as f64;
    let v_lo = v_cut * (1.0 - params.taper_frac.clamp(0.0, 0.9) as f64);
    let v_hi = v_cut * (1.0 + params.taper_frac.clamp(0.0, 0.9) as f64);
    let ped = params.pedestal.clamp(0.0, 1.0) as f64;

    for ik in 0..nfft_x {
        let k_signed = if ik <= nfft_x / 2 {
            ik as f64 * dk
        } else {
            (ik as isize - nfft_x as isize) as f64 * dk
        };
        let k_abs = k_signed.abs();

        for ifreq in 0..nfft_t {
            let f_signed = if ifreq <= nfft_t / 2 {
                ifreq as f64 * df
            } else {
                (ifreq as isize - nfft_t as isize) as f64 * df
            };
            let f_abs = f_signed.abs();

            let weight = match params.mode {
                FkMode::RejectLowVelocity | FkMode::PassHighVelocity => {
                    if k_abs < 1e-12 {
                        1.0 // k=0 (flat reflections / infinite apparent velocity) always preserved
                    } else {
                        let v_app = f_abs / k_abs;
                        if v_app <= v_lo {
                            ped
                        } else if v_app >= v_hi {
                            1.0
                        } else {
                            let frac = (v_app - v_lo) / (v_hi - v_lo).max(1e-9);
                            ped + (1.0 - ped) * 0.5 * (1.0 - (PI * frac).cos())
                        }
                    }
                }
                FkMode::RejectPolygon => {
                    if point_in_polygon(f_abs, k_signed, &params.polygon) {
                        ped
                    } else {
                        1.0
                    }
                }
                FkMode::PassPolygon => {
                    if point_in_polygon(f_abs, k_signed, &params.polygon) {
                        1.0
                    } else {
                        ped
                    }
                }
            };

            grid[ik * nfft_t + ifreq] *= weight;
        }
    }

    // 4. Inverse FFT along space
    let inv_x = 1.0 / nfft_x as f64;
    for ifreq in 0..nfft_t {
        for ix in 0..nfft_x {
            col[ix] = grid[ix * nfft_t + ifreq];
        }
        ifft_x.process(&mut col);
        for ix in 0..nfft_x {
            grid[ix * nfft_t + ifreq] = col[ix] * inv_x;
        }
    }

    // 5. Inverse FFT along time and write back
    let inv_t = 1.0 / nfft_t as f64;
    for ix in 0..nx {
        let tr = &mut ensemble.traces[ix];
        if !tr.is_live() {
            continue;
        }
        let row = &mut grid[ix * nfft_t..(ix + 1) * nfft_t];
        ifft_t.process(row);
        for it in 0..nt.min(tr.samples.len()) {
            tr.samples[it] = (row[it].re * inv_t) as f32;
        }
    }

    Ok(())
}
