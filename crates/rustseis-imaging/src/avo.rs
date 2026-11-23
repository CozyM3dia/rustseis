//! Amplitude Variation with Offset / Angle (AVO / AVA) analysis:
//! offset-to-angle ray-parameter mapping, Shuey 2-term & 3-term regression (Intercept R0,
//! Gradient G, Curvature C), Fluid Factor, angle stacks, and synthetic Zoeppritz/Shuey helper.

use rayon::prelude::*;
use rustseis_core::{Ensemble, Result, SeismicError, Trace, TraceHeaders, VelocityTable};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AvoOutputMode {
    #[default]
    Intercept,
    Gradient,
    Product,
    FluidFactor,
    Curvature,
    AngleStack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvoParams {
    #[serde(default)]
    pub output: AvoOutputMode,
    /// Minimum incidence angle included in regression / angle stack (degrees).
    #[serde(default = "default_min_angle")]
    pub min_angle_deg: f32,
    /// Maximum incidence angle included in regression / angle stack (degrees).
    #[serde(default = "default_max_angle")]
    pub max_angle_deg: f32,
    /// Use 3-term Shuey (`R0 + G sin^2(theta) + C sin^2(theta) tan^2(theta)`).
    #[serde(default)]
    pub three_term: bool,
    /// Minimum live traces within angle range required to solve regression at a sample.
    #[serde(default = "default_min_traces")]
    pub min_traces: usize,
    /// Optional explicit background slope `m` for Fluid Factor `G - m * R0` (`None` = auto-fit).
    #[serde(default)]
    pub background_slope: Option<f32>,
    /// Optional time window `[t_start_ms, t_end_ms]` for crossplot summary points.
    #[serde(default = "default_window")]
    pub crossplot_window_ms: [f32; 2],
}

fn default_min_angle() -> f32 { 2.0 }
fn default_max_angle() -> f32 { 38.0 }
fn default_min_traces() -> usize { 4 }
fn default_window() -> [f32; 2] { [200.0, 2500.0] }

impl Default for AvoParams {
    fn default() -> Self {
        Self {
            output: AvoOutputMode::Intercept,
            min_angle_deg: default_min_angle(),
            max_angle_deg: default_max_angle(),
            three_term: false,
            min_traces: default_min_traces(),
            background_slope: None,
            crossplot_window_ms: default_window(),
        }
    }
}

/// Summary & attribute sections produced by AVO analysis on NMO-corrected CDP gathers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvoSummary {
    pub name: String,
    pub n_cdps: usize,
    pub background_slope: f32,
    pub rms_intercept: f32,
    pub rms_gradient: f32,
    /// Decimated `(R0, G, time_ms, cdp)` samples for R0-vs-G crossplot QC.
    pub crossplot: Vec<[f32; 4]>,
}

/// Full multi-attribute AVO output.
#[derive(Debug, Clone)]
pub struct AvoOutput {
    pub intercept: Vec<Trace>,
    pub gradient: Vec<Trace>,
    pub product: Vec<Trace>,
    pub fluid_factor: Vec<Trace>,
    pub curvature: Vec<Trace>,
    pub angle_stack: Vec<Trace>,
    pub summary: AvoSummary,
}

/// Exact Shuey (1985) 3-term P-P reflection coefficient at incidence angle `theta_rad`.
pub fn shuey_reflectivity(
    vp1: f64,
    vs1: f64,
    rho1: f64,
    vp2: f64,
    vs2: f64,
    rho2: f64,
    theta_rad: f64,
) -> (f64, f64, f64, f64) {
    let vp = 0.5 * (vp1 + vp2);
    let vs = 0.5 * (vs1 + vs2);
    let rho = 0.5 * (rho1 + rho2);
    let dvp = vp2 - vp1;
    let dvs = vs2 - vs1;
    let drho = rho2 - rho1;
    let k = (vs / vp).powi(2);

    let r0 = 0.5 * (dvp / vp + drho / rho);
    let g = 0.5 * (dvp / vp) - 2.0 * k * (drho / rho + 2.0 * dvs / vs);
    let c = 0.5 * (dvp / vp);
    let s2 = theta_rad.sin().powi(2);
    let t2 = theta_rad.tan().powi(2);
    let r = r0 + g * s2 + c * s2 * t2;
    (r, r0, g, c)
}

/// Walden (1991) / Dix ray-parameter incidence angle `sin(theta)` at `(offset_m, t0_s)`.
#[inline]
pub fn sin_incidence_angle(offset_m: f64, t0_s: f64, v_rms_mps: f64, v_int_mps: f64) -> f64 {
    if t0_s <= 1e-5 || v_rms_mps <= 1.0 {
        return 0.0;
    }
    let denom = v_rms_mps * (v_rms_mps * v_rms_mps * t0_s * t0_s + offset_m * offset_m).sqrt();
    if denom <= 1e-9 {
        0.0
    } else {
        ((offset_m.abs() * v_int_mps) / denom).clamp(0.0, 0.98)
    }
}

/// Solve 3x3 symmetric positive-definite linear system `A x = b` via Gaussian elimination.
fn solve_3x3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    for i in 0..3 {
        let mut piv = i;
        for r in (i + 1)..3 {
            if a[r][i].abs() > a[piv][i].abs() {
                piv = r;
            }
        }
        if a[piv][i].abs() < 1e-12 {
            return None;
        }
        if piv != i {
            a.swap(i, piv);
            b.swap(i, piv);
        }
        let d = a[i][i];
        for r in (i + 1)..3 {
            let f = a[r][i] / d;
            for c in i..3 {
                a[r][c] -= f * a[i][c];
            }
            b[r] -= f * b[i];
        }
    }
    let mut x = [0.0f64; 3];
    for i in (0..3).rev() {
        let mut s = b[i];
        for c in (i + 1)..3 {
            s -= a[i][c] * x[c];
        }
        x[i] = s / a[i][i];
    }
    Some(x)
}

/// Compute AVO Intercept (R0), Gradient (G), Curvature (C), Product (R0*G), Fluid Factor,
/// and Angle Stack across NMO-corrected CDP gathers.
pub fn compute_avo_2d(
    traces: &[Trace],
    vel: &VelocityTable,
    params: &AvoParams,
    name: impl Into<String>,
) -> Result<AvoOutput> {
    if traces.is_empty() {
        return Err(SeismicError::InvalidFormat("AVO analysis requires non-empty CDP gathers".into()));
    }
    let ensembles = Ensemble::group_by_header(traces.to_vec(), "CDP");
    let min_sin = (params.min_angle_deg.clamp(0.0, 80.0) as f64).to_radians().sin();
    let max_sin = (params.max_angle_deg.clamp(params.min_angle_deg + 1.0, 85.0) as f64)
        .to_radians()
        .sin();
    let min_tr = params.min_traces.max(if params.three_term { 3 } else { 2 });

    struct CdpAvo {
        cdp: i32,
        headers: TraceHeaders,
        dt_us: u32,
        r0: Vec<f32>,
        g: Vec<f32>,
        c: Vec<f32>,
        prod: Vec<f32>,
        astack: Vec<f32>,
    }

    let per_cdp: Vec<CdpAvo> = ensembles
        .par_iter()
        .filter_map(|ens| {
            let first = ens.traces.first()?;
            let ns = first.samples.len();
            let dt_us = first.dt_us;
            let dt_s = dt_us as f64 * 1e-6;
            let cdp = first.headers.cdp;

            // Precompute V_rms(t) and smooth Dix V_int(t) at this CDP
            let mut v_rms = vec![1500.0f64; ns];
            let mut v_int = vec![1500.0f64; ns];
            let mut prev_v2t = 0.0f64;
            for it in 0..ns {
                let t_s = it as f64 * dt_s;
                let vr = vel.velocity_at(cdp, (t_s * 1000.0) as f32).max(500.0) as f64;
                v_rms[it] = vr;
                let v2t = vr * vr * t_s;
                v_int[it] = if it == 0 {
                    vr
                } else {
                    let dv2t = v2t - prev_v2t;
                    if dv2t > 0.0 {
                        (dv2t / dt_s).sqrt().clamp(500.0, 8000.0)
                    } else {
                        v_int[it - 1]
                    }
                };
                prev_v2t = v2t;
            }

            let mut r0_v = vec![0.0f32; ns];
            let mut g_v = vec![0.0f32; ns];
            let mut c_v = vec![0.0f32; ns];
            let mut prod_v = vec![0.0f32; ns];
            let mut astack_v = vec![0.0f32; ns];

            for it in 1..ns {
                let t0_s = it as f64 * dt_s;
                let vr = v_rms[it];
                let vi = v_int[it];

                let mut n_ok = 0usize;
                let mut sum_amp = 0.0f64;

                if !params.three_term {
                    let (mut s00, mut s01, mut s11) = (0.0f64, 0.0f64, 0.0f64);
                    let (mut b0, mut b1) = (0.0f64, 0.0f64);
                    for tr in &ens.traces {
                        if !tr.is_live() || it >= tr.samples.len() {
                            continue;
                        }
                        let amp = tr.samples[it] as f64;
                        if !amp.is_finite() || amp == 0.0 {
                            continue;
                        }
                        let sin_th = sin_incidence_angle(tr.headers.offset as f64, t0_s, vr, vi);
                        if sin_th < min_sin || sin_th > max_sin {
                            continue;
                        }
                        let x = sin_th * sin_th;
                        s00 += 1.0;
                        s01 += x;
                        s11 += x * x;
                        b0 += amp;
                        b1 += x * amp;
                        sum_amp += amp;
                        n_ok += 1;
                    }
                    if n_ok >= min_tr {
                        astack_v[it] = (sum_amp / n_ok as f64) as f32;
                        let reg = 1e-6 * s00;
                        let det = (s00 + reg) * (s11 + reg) - s01 * s01;
                        if det.abs() > 1e-12 {
                            let r0 = ((s11 + reg) * b0 - s01 * b1) / det;
                            let g = (-s01 * b0 + (s00 + reg) * b1) / det;
                            r0_v[it] = r0 as f32;
                            g_v[it] = g as f32;
                            prod_v[it] = (r0 * g) as f32;
                        }
                    }
                } else {
                    let mut a = [[0.0f64; 3]; 3];
                    let mut b = [0.0f64; 3];
                    for tr in &ens.traces {
                        if !tr.is_live() || it >= tr.samples.len() {
                            continue;
                        }
                        let amp = tr.samples[it] as f64;
                        if !amp.is_finite() || amp == 0.0 {
                            continue;
                        }
                        let sin_th = sin_incidence_angle(tr.headers.offset as f64, t0_s, vr, vi);
                        if sin_th < min_sin || sin_th > max_sin {
                            continue;
                        }
                        let s2 = sin_th * sin_th;
                        let cos2 = (1.0 - s2).max(1e-4);
                        let st2 = s2 * (s2 / cos2);
                        let row = [1.0, s2, st2];
                        for r in 0..3 {
                            b[r] += row[r] * amp;
                            for col in 0..3 {
                                a[r][col] += row[r] * row[col];
                            }
                        }
                        sum_amp += amp;
                        n_ok += 1;
                    }
                    if n_ok >= min_tr {
                        astack_v[it] = (sum_amp / n_ok as f64) as f32;
                        for d in 0..3 {
                            a[d][d] += 1e-5;
                        }
                        if let Some([r0, g, c]) = solve_3x3(a, b) {
                            r0_v[it] = r0 as f32;
                            g_v[it] = g as f32;
                            c_v[it] = c as f32;
                            prod_v[it] = (r0 * g) as f32;
                        }
                    }
                }
            }

            let mut hdr = first.headers.clone();
            hdr.offset = 0.0;
            hdr.cdp_nfld = ens.traces.len() as i32;
            Some(CdpAvo {
                cdp,
                headers: hdr,
                dt_us,
                r0: r0_v,
                g: g_v,
                c: c_v,
                prod: prod_v,
                astack: astack_v,
            })
        })
        .collect();

    // Estimate background R0-vs-G mudrock slope m = sum(R0*G)/sum(R0^2) inside crossplot window
    let (mut sum_rr, mut sum_rg, mut sum_gg) = (0.0f64, 0.0f64, 0.0f64);
    let mut n_win = 0usize;
    let [tw0, tw1] = params.crossplot_window_ms;
    for item in &per_cdp {
        let dt_ms = item.dt_us as f32 / 1000.0;
        for it in 0..item.r0.len() {
            let t_ms = it as f32 * dt_ms;
            if t_ms < tw0 || t_ms > tw1 {
                continue;
            }
            let r0 = item.r0[it] as f64;
            let g = item.g[it] as f64;
            if r0 == 0.0 && g == 0.0 {
                continue;
            }
            sum_rr += r0 * r0;
            sum_rg += r0 * g;
            sum_gg += g * g;
            n_win += 1;
        }
    }
    let bg_slope = params.background_slope.unwrap_or_else(|| {
        if sum_rr > 1e-12 {
            (sum_rg / sum_rr).clamp(-6.0, 2.0) as f32
        } else {
            -1.0
        }
    });

    let mut intercept = Vec::with_capacity(per_cdp.len());
    let mut gradient = Vec::with_capacity(per_cdp.len());
    let mut product = Vec::with_capacity(per_cdp.len());
    let mut fluid_factor = Vec::with_capacity(per_cdp.len());
    let mut curvature = Vec::with_capacity(per_cdp.len());
    let mut angle_stack = Vec::with_capacity(per_cdp.len());
    let mut crossplot = Vec::new();
    let stride = (n_win / 4000).max(1);
    let mut seen = 0usize;

    for (idx, item) in per_cdp.into_iter().enumerate() {
        let dt_ms = item.dt_us as f32 / 1000.0;
        let ff: Vec<f32> = item
            .r0
            .iter()
            .zip(&item.g)
            .enumerate()
            .map(|(it, (&r0, &g))| {
                let t_ms = it as f32 * dt_ms;
                if t_ms >= tw0 && t_ms <= tw1 && (r0 != 0.0 || g != 0.0) {
                    if seen % stride == 0 && crossplot.len() < 5000 {
                        crossplot.push([r0, g, t_ms, item.cdp as f32]);
                    }
                    seen += 1;
                }
                g - bg_slope * r0
            })
            .collect();

        let make = |samples: Vec<f32>| {
            let mut tr = Trace::new(item.dt_us, samples);
            tr.headers = item.headers.clone();
            tr.headers.traceno = (idx + 1) as i32;
            tr.headers.ffid = 1;
            tr.headers.chan = (idx + 1) as i32;
            tr
        };
        intercept.push(make(item.r0));
        gradient.push(make(item.g));
        product.push(make(item.prod));
        fluid_factor.push(make(ff));
        curvature.push(make(item.c));
        angle_stack.push(make(item.astack));
    }

    let summary = AvoSummary {
        name: name.into(),
        n_cdps: intercept.len(),
        background_slope: bg_slope,
        rms_intercept: if n_win > 0 { (sum_rr / n_win as f64).sqrt() as f32 } else { 0.0 },
        rms_gradient: if n_win > 0 { (sum_gg / n_win as f64).sqrt() as f32 } else { 0.0 },
        crossplot,
    };

    Ok(AvoOutput {
        intercept,
        gradient,
        product,
        fluid_factor,
        curvature,
        angle_stack,
        summary,
    })
}
