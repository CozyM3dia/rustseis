use crate::nmo::sinc_interp;
use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace, VelocityTable};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Stolt F-K 2D Post-Stack Time Migration (`fkmig2d` Stolt mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoltMigParams {
    /// Constant migration velocity in m/s.
    pub velocity_mps: f32,
    /// CDP spacing `dx` in meters.
    pub dx_m: f32,
    /// Optional spatial zero-padding traces on each side (default 32).
    #[serde(default = "default_pad_x")]
    pub pad_traces: usize,
}

fn default_pad_x() -> usize {
    32
}

pub fn stolt_migrate_2d(section: &[Trace], params: &StoltMigParams) -> Result<Vec<Trace>> {
    if section.len() < 2 {
        return Err(SeismicError::InvalidFormat("Need at least 2 traces for 2D migration".into()));
    }
    let nx = section.len();
    let nt = section[0].samples.len();
    let dt = section[0].dt_s() as f64;
    let dx = params.dx_m.max(0.1) as f64;
    let v_half = (params.velocity_mps.max(100.0) as f64) * 0.5; // Exploding reflector v/2

    let nfft_t = (nt + nt / 2).next_power_of_two().max(64);
    let nfft_x = (nx + 2 * params.pad_traces).next_power_of_two().max(32);

    let mut planner = FftPlanner::<f64>::new();
    let fft_t = planner.plan_fft_forward(nfft_t);
    let ifft_t = planner.plan_fft_inverse(nfft_t);
    let fft_x = planner.plan_fft_forward(nfft_x);
    let ifft_x = planner.plan_fft_inverse(nfft_x);

    // 1. 2D Forward FFT: (t, x) -> (omega, kx)
    let mut fk = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];
    for ix in 0..nx {
        let dst_row = (ix + params.pad_traces) * nfft_t;
        for it in 0..nt.min(section[ix].samples.len()) {
            fk[dst_row + it] = Complex::new(section[ix].samples[it] as f64, 0.0);
        }
        fft_t.process(&mut fk[dst_row..dst_row + nfft_t]);
    }
    let mut col = vec![Complex::new(0.0, 0.0); nfft_x];
    for iw in 0..nfft_t {
        for ix in 0..nfft_x {
            col[ix] = fk[ix * nfft_t + iw];
        }
        fft_x.process(&mut col);
        for ix in 0..nfft_x {
            fk[ix * nfft_t + iw] = col[ix];
        }
    }

    // 2. Stolt mapping: omega = sgn(w0) * sqrt(w0^2 + (v/2 * kx)^2), Jacobian = |w0 / omega|
    let dw = 2.0 * PI / (nfft_t as f64 * dt);
    let dkx = 2.0 * PI / (nfft_x as f64 * dx);
    let mut fk_mig = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];

    let interp_complex = |row: &[Complex<f64>], f_idx: f64| -> Complex<f64> {
        let n = row.len();
        if f_idx < 0.0 || f_idx >= (n - 1) as f64 {
            return Complex::new(0.0, 0.0);
        }
        let i0 = f_idx.floor() as usize;
        let frac = f_idx - i0 as f64;
        // 4-point cubic Catmull-Rom interpolation in frequency
        let p0 = row[i0.saturating_sub(1)];
        let p1 = row[i0];
        let p2 = row[(i0 + 1).min(n - 1)];
        let p3 = row[(i0 + 2).min(n - 1)];
        let t = frac;
        let t2 = t * t;
        let t3 = t2 * t;
        let a = p1 * 2.0;
        let b = (p2 - p0) * t;
        let c = (p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3) * t2;
        let d = (-p0 + p1 * 3.0 - p2 * 3.0 + p3) * t3;
        (a + b + c + d) * 0.5
    };

    for ikx in 0..nfft_x {
        let kx = if ikx <= nfft_x / 2 {
            ikx as f64 * dkx
        } else {
            (ikx as isize - nfft_x as isize) as f64 * dkx
        };
        let row_in = &fk[ikx * nfft_t..(ikx + 1) * nfft_t];
        let row_out = &mut fk_mig[ikx * nfft_t..(ikx + 1) * nfft_t];
        let vkx2 = (v_half * kx) * (v_half * kx);

        // Positive frequencies 1..=nfft_t/2
        for iw0 in 1..=nfft_t / 2 {
            let w0 = iw0 as f64 * dw;
            let w = (w0 * w0 + vkx2).sqrt();
            let iw = w / dw;
            if iw < (nfft_t / 2) as f64 {
                let scale = w0 / w;
                let val = interp_complex(&row_in[0..=nfft_t / 2], iw) * scale;
                row_out[iw0] = val;
                if iw0 < nfft_t - iw0 {
                    row_out[nfft_t - iw0] = val.conj();
                }
            }
        }
    }

    // 3. Inverse 2D FFT: (w0, kx) -> (tau, x)
    let inv_x = 1.0 / nfft_x as f64;
    for iw in 0..nfft_t {
        for ix in 0..nfft_x {
            col[ix] = fk_mig[ix * nfft_t + iw];
        }
        ifft_x.process(&mut col);
        for ix in 0..nfft_x {
            fk_mig[ix * nfft_t + iw] = col[ix] * inv_x;
        }
    }
    let inv_t = 1.0 / nfft_t as f64;
    let mut out = section.to_vec();
    for ix in 0..nx {
        let row = &mut fk_mig[(ix + params.pad_traces) * nfft_t..(ix + params.pad_traces + 1) * nfft_t];
        ifft_t.process(row);
        for it in 0..nt {
            out[ix].samples[it] = (row[it].re * inv_t) as f32;
        }
    }
    Ok(out)
}

/// Gazdag Phase-Shift 2D Post-Stack Time Migration (`fkmig2d` Phase-Shift mode, supports `v(t)`).
pub fn phase_shift_migrate_2d(
    section: &[Trace],
    vel: &VelocityTable,
    dx_m: f32,
) -> Result<Vec<Trace>> {
    if section.len() < 2 {
        return Err(SeismicError::InvalidFormat("Need at least 2 traces for phase-shift migration".into()));
    }
    let nx = section.len();
    let nt = section[0].samples.len();
    let dt = section[0].dt_s() as f64;
    let dx = dx_m.max(0.1) as f64;
    let pad_x = 32usize;
    let nfft_t = (nt + nt / 4).next_power_of_two().max(64);
    let nfft_x = (nx + 2 * pad_x).next_power_of_two().max(32);

    let mut planner = FftPlanner::<f64>::new();
    let fft_t = planner.plan_fft_forward(nfft_t);
    let fft_x = planner.plan_fft_forward(nfft_x);
    let ifft_x = planner.plan_fft_inverse(nfft_x);

    let mut fk = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];
    for ix in 0..nx {
        let off = (ix + pad_x) * nfft_t;
        for it in 0..nt {
            fk[off + it] = Complex::new(section[ix].samples[it] as f64, 0.0);
        }
        fft_t.process(&mut fk[off..off + nfft_t]);
    }
    let mut col = vec![Complex::new(0.0, 0.0); nfft_x];
    for iw in 0..nfft_t {
        for ix in 0..nfft_x {
            col[ix] = fk[ix * nfft_t + iw];
        }
        fft_x.process(&mut col);
        for ix in 0..nfft_x {
            fk[ix * nfft_t + iw] = col[ix];
        }
    }

    let dw = 2.0 * PI / (nfft_t as f64 * dt);
    let dkx = 2.0 * PI / (nfft_x as f64 * dx);
    let mid_cdp = section[nx / 2].headers.cdp;
    let nw_pos = nfft_t / 2 + 1;

    // Extrapolate downwards in tau step-by-step and apply imaging condition t = 0 (sum over omega)
    let mut image_kx = vec![Complex::new(0.0, 0.0); nt * nfft_x];
    let mut wavefield: Vec<Complex<f64>> = (0..nfft_x)
        .flat_map(|ikx| fk[ikx * nfft_t..ikx * nfft_t + nw_pos].iter().copied())
        .collect();

    for itau in 0..nt {
        let tau_ms = (itau as f64 * dt * 1000.0) as f32;
        let v_half = (vel.velocity_at(mid_cdp, tau_ms).max(100.0) as f64) * 0.5;

        for ikx in 0..nfft_x {
            let kx = if ikx <= nfft_x / 2 {
                ikx as f64 * dkx
            } else {
                (ikx as isize - nfft_x as isize) as f64 * dkx
            };
            let vkx = v_half * kx;
            let row = &mut wavefield[ikx * nw_pos..(ikx + 1) * nw_pos];

            // Imaging condition: real inverse FFT at t=0 -> 2*Re(sum_{w>0}) + DC
            let mut sum = row[0];
            for iw in 1..nw_pos - 1 {
                sum += row[iw] * 2.0;
            }
            sum += row[nw_pos - 1];
            image_kx[itau * nfft_x + ikx] = sum / nfft_t as f64;

            // Downward phase shift by dt
            for iw in 1..nw_pos {
                let w = iw as f64 * dw;
                let rad = w * w - vkx * vkx;
                if rad > 0.0 {
                    let phase = dt * rad.sqrt();
                    row[iw] *= Complex::from_polar(1.0, phase);
                } else {
                    row[iw] = Complex::new(0.0, 0.0); // evanescent energy
                }
            }
        }
    }

    // Inverse FFT along kx for each tau slice
    let inv_x = 1.0 / nfft_x as f64;
    let mut out = section.to_vec();
    for itau in 0..nt {
        let row = &mut image_kx[itau * nfft_x..(itau + 1) * nfft_x];
        ifft_x.process(row);
        for ix in 0..nx {
            out[ix].samples[itau] = (row[ix + pad_x].re * inv_x) as f32;
        }
    }
    Ok(out)
}

/// Post-Stack 2D Kirchhoff Time Migration (`kirtime2dstk`, supports laterally & vertically variant `V_rms(x, t)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kirchhoff2dParams {
    /// CDP trace spacing `dx` in meters.
    pub dx_m: f32,
    /// Maximum half-aperture in meters (e.g., 1500–3000 m).
    #[serde(default = "default_aperture")]
    pub max_aperture_m: f32,
    /// Maximum migration dip angle in degrees (default 60 deg).
    #[serde(default = "default_max_dip")]
    pub max_dip_deg: f32,
    /// Apply 2D half-derivative (45-deg phase + sqrt(f) amplitude) rho-filter (default true).
    #[serde(default = "default_true")]
    pub apply_half_derivative: bool,
    /// Enable triangle anti-aliasing filter for steep dips (default true).
    #[serde(default = "default_true")]
    pub anti_alias: bool,
}

fn default_aperture() -> f32 {
    2000.0
}
fn default_max_dip() -> f32 {
    60.0
}
fn default_true() -> bool {
    true
}

impl Default for Kirchhoff2dParams {
    fn default() -> Self {
        Self {
            dx_m: 6.25,
            max_aperture_m: 2000.0,
            max_dip_deg: 60.0,
            apply_half_derivative: true,
            anti_alias: true,
        }
    }
}

/// Apply 2D Kirchhoff half-derivative filter `sqrt(-i * omega)` to a trace using pre-planned FFTs.
pub(crate) fn half_derivative_filter_with_plan(
    samples: &[f32],
    dt_s: f64,
    fft: &dyn rustfft::Fft<f64>,
    ifft: &dyn rustfft::Fft<f64>,
    nfft: usize,
) -> Vec<f32> {
    let n = samples.len();
    let mut buf = vec![Complex::new(0.0, 0.0); nfft];
    for i in 0..n {
        buf[i] = Complex::new(samples[i] as f64, 0.0);
    }
    fft.process(&mut buf);

    let dw = 2.0 * PI / (nfft as f64 * dt_s);
    let w_nyq = PI / dt_s;
    let rot_pos = Complex::from_polar(1.0, PI * 0.25);
    let rot_neg = rot_pos.conj();
    buf[0] = Complex::new(0.0, 0.0);
    for k in 1..nfft / 2 {
        let w = k as f64 * dw;
        let mag = (w / w_nyq).sqrt();
        buf[k] *= rot_pos * mag;
        buf[nfft - k] *= rot_neg * mag;
    }
    buf[nfft / 2] *= Complex::new(1.0, 0.0);

    ifft.process(&mut buf);
    let inv_n = 1.0 / nfft as f64;
    (0..n).map(|i| (buf[i].re * inv_n) as f32).collect()
}

/// Build double-integrated trace for O(1) Lumley-Claerbout triangle anti-aliasing filter.
pub(crate) fn build_double_integral(samples: &[f32]) -> Vec<f64> {
    let n = samples.len();
    let mut s1 = vec![0.0f64; n];
    let mut s2 = vec![0.0f64; n];
    let mut acc = 0.0f64;
    for i in 0..n {
        acc += samples[i] as f64;
        s1[i] = acc;
    }
    acc = 0.0;
    for i in 0..n {
        acc += s1[i];
        s2[i] = acc;
    }
    s2
}

#[inline]
pub(crate) fn sample_triangle_aa(s2: &[f64], raw: &[f32], pos: f64, half_len: usize) -> f32 {
    if half_len <= 1 {
        return sinc_interp(raw, pos);
    }
    let n = s2.len();
    let c = pos.round() as isize;
    let l = half_len as isize;
    if c - l < 0 || c + l >= n as isize {
        return sinc_interp(raw, pos);
    }
    let val = s2[(c + l) as usize] - 2.0 * s2[c as usize] + s2[(c - l) as usize];
    (val / ((l * l) as f64)) as f32
}

pub fn kirchhoff_poststack_2d(
    section: &[Trace],
    vel: &VelocityTable,
    params: &Kirchhoff2dParams,
) -> Result<Vec<Trace>> {
    if section.len() < 2 {
        return Err(SeismicError::InvalidFormat("Need at least 2 traces for Kirchhoff migration".into()));
    }
    let nx = section.len();
    let nt = section[0].samples.len();
    let dt_s = section[0].dt_s() as f64;
    let dx = params.dx_m.max(0.1) as f64;
    let max_ap_tr = (params.max_aperture_m as f64 / dx).ceil() as isize;
    let cos_max_dip = (params.max_dip_deg.clamp(5.0, 89.0) as f64 * PI / 180.0).cos();

    // Hoist FFT planning outside rayon loop for 10x faster pre-filtering!
    let nfft = (nt + nt / 4).next_power_of_two().max(64);
    let mut planner = FftPlanner::<f64>::new();
    let fft_hd = planner.plan_fft_forward(nfft);
    let ifft_hd = planner.plan_fft_inverse(nfft);

    let prefiltered: Vec<Vec<f32>> = section
        .par_iter()
        .map(|tr| {
            if params.apply_half_derivative {
                half_derivative_filter_with_plan(&tr.samples, dt_s, fft_hd.as_ref(), ifft_hd.as_ref(), nfft)
            } else {
                tr.samples.clone()
            }
        })
        .collect();
    let integrals: Vec<Vec<f64>> = if params.anti_alias {
        prefiltered.par_iter().map(|s| build_double_integral(s)).collect()
    } else {
        Vec::new()
    };

    let migrated_samples: Vec<Vec<f32>> = (0..nx)
        .into_par_iter()
        .map(|ix_out| {
            let mut out = vec![0.0f32; nt];
            let cdp = section[ix_out].headers.cdp;
            let j_lo = (ix_out as isize - max_ap_tr).max(0) as usize;
            let j_hi = (ix_out as isize + max_ap_tr).min((nx - 1) as isize) as usize;
            let ap_m = params.max_aperture_m.max(dx as f32) as f64;

            for it0 in 1..nt {
                let t0 = it0 as f64 * dt_s;
                let v = vel.velocity_at(cdp, (t0 * 1000.0) as f32).max(100.0) as f64;
                let inv_vh = 2.0 / v;
                let mut sum = 0.0f64;

                for jx in j_lo..=j_hi {
                    let dist = ((jx as isize - ix_out as isize).abs() as f64) * dx;
                    if dist > ap_m {
                        continue;
                    }
                    let u = dist * inv_vh;
                    let td = (t0 * t0 + u * u).sqrt();
                    let obliquity = t0 / td; // cos(theta)
                    if obliquity < cos_max_dip {
                        continue;
                    }
                    let pos = td / dt_s;
                    if pos >= (nt - 1) as f64 {
                        continue;
                    }

                    // Aperture edge cosine taper over outer 20% of aperture
                    let ap_weight = if dist > 0.8 * ap_m {
                        let frac = (dist - 0.8 * ap_m) / (0.2 * ap_m).max(1e-6);
                        0.5 * (1.0 + (PI * frac).cos())
                    } else {
                        1.0
                    };

                    // 2D Kirchhoff amplitude weight: cos(theta) / sqrt(v * td) * dx
                    let weight = (obliquity / (v * td).sqrt()) * dx * ap_weight;

                    let amp = if params.anti_alias {
                        // Local moveout slope dt/dx = 4 * dist / (v^2 * td) -> filter half-length in samples
                        let slope_s_per_tr = (4.0 * dist * dx) / (v * v * td);
                        let half_len = ((slope_s_per_tr / dt_s).floor() as usize).min(24);
                        sample_triangle_aa(&integrals[jx], &prefiltered[jx], pos, half_len)
                    } else {
                        sinc_interp(&prefiltered[jx], pos)
                    };

                    sum += amp as f64 * weight;
                }
                out[it0] = sum as f32;
            }
            out
        })
        .collect();

    let mut out_traces = section.to_vec();
    for (tr, samps) in out_traces.iter_mut().zip(migrated_samples) {
        tr.samples = samps;
    }
    Ok(out_traces)
}
