use rayon::prelude::*;
use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FxDeconMethod {
    /// Complex Wiener-Levinson 1-step forward/backward prediction (ProMAX default, Gulunay 1986).
    #[default]
    WienerLevinson,
    /// Complex normalized LMS L2-norm adaptive prediction filter.
    L2Adaptive,
}

/// F-X Deconvolution parameters (`fxdecon` — matches ProMAX `fxdecon.pdf`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FxDeconParams {
    #[serde(default)]
    pub method: FxDeconMethod,
    /// Percentage of white noise added to zero-lag autocorrelation (Wiener-Levinson, default 1.0%).
    #[serde(default = "default_wn")]
    pub white_noise_pct: f32,
    /// Adaptation step size for L2Adaptive mode (default 0.1).
    #[serde(default = "default_mu")]
    pub adaptation_rate: f32,
    /// Horizontal window length in traces (must be > filter_len, default 20).
    #[serde(default = "default_horiz_win")]
    pub horiz_window_traces: usize,
    /// Complex prediction filter length in traces (ProMAX recommends 4..7, default 5).
    #[serde(default = "default_filter_len")]
    pub filter_len: usize,
    /// Time window length in ms (default 300 ms).
    #[serde(default = "default_time_win")]
    pub time_window_ms: f32,
    /// Time window overlap on each side in ms (default 50 ms).
    #[serde(default = "default_time_ovl")]
    pub time_overlap_ms: f32,
    /// Start frequency in Hz (default 6.0 Hz).
    #[serde(default = "default_fmin")]
    pub f_min_hz: f32,
    /// End frequency in Hz (default 90.0 Hz).
    #[serde(default = "default_fmax")]
    pub f_max_hz: f32,
    /// Reapply zero-mute mask from input traces after filtering.
    #[serde(default = "default_true")]
    pub reapply_mute: bool,
}

fn default_wn() -> f32 {
    1.0
}
fn default_mu() -> f32 {
    0.1
}
fn default_horiz_win() -> usize {
    20
}
fn default_filter_len() -> usize {
    5
}
fn default_time_win() -> f32 {
    300.0
}
fn default_time_ovl() -> f32 {
    50.0
}
fn default_fmin() -> f32 {
    6.0
}
fn default_fmax() -> f32 {
    90.0
}
fn default_true() -> bool {
    true
}

impl Default for FxDeconParams {
    fn default() -> Self {
        Self {
            method: FxDeconMethod::WienerLevinson,
            white_noise_pct: 1.0,
            adaptation_rate: 0.1,
            horiz_window_traces: 20,
            filter_len: 5,
            time_window_ms: 300.0,
            time_overlap_ms: 50.0,
            f_min_hz: 6.0,
            f_max_hz: 90.0,
            reapply_mute: true,
        }
    }
}

/// Solve Hermitian Toeplitz system `R * a = g` using complex Levinson-Durbin recursion.
/// `r[0..=m]` has `r[k] = E[x[n] * conj(x[n-k])]` (`r[0]` real positive).
/// Solves 1-step forward prediction: `sum_{k=1..=p} a[k-1] * x[n-k] ≈ x[n]`.
fn solve_complex_levinson_predict(x: &[Complex<f64>], p: usize, white_noise_frac: f64) -> Vec<Complex<f64>> {
    let n = x.len();
    if p == 0 || n <= p {
        return vec![Complex::new(0.0, 0.0); p];
    }

    // Biased autocorrelation lags r[0..=p]
    let mut r = vec![Complex::new(0.0, 0.0); p + 1];
    for lag in 0..=p {
        let mut acc = Complex::new(0.0, 0.0);
        for i in lag..n {
            acc += x[i] * x[i - lag].conj();
        }
        r[lag] = acc;
    }

    let r0 = r[0].re;
    if r0 <= 1e-24 || !r0.is_finite() {
        return vec![Complex::new(0.0, 0.0); p];
    }
    let diag = r0 * (1.0 + white_noise_frac.max(1e-6));
    r[0] = Complex::new(diag, 0.0);

    // Levinson-Durbin for AR coefficients: r[0] + sum_{k=1..m} c[k] * r[-k] = err
    // where prediction filter a[k] = -c[k], with r[-k] = conj(r[k]).
    let mut c = vec![Complex::new(0.0, 0.0); p + 1];
    let mut c_prev = vec![Complex::new(0.0, 0.0); p + 1];
    let mut err = diag;

    for m in 1..=p {
        let mut num = r[m];
        for k in 1..m {
            num += c_prev[k] * r[m - k];
        }
        if err.abs() <= 1e-24 {
            break;
        }
        let km = -num / err;
        c[m] = km;
        for k in 1..m {
            c[k] = c_prev[k] + km * c_prev[m - k].conj();
        }
        err *= (1.0 - km.norm_sqr()).max(1e-12);
        c_prev[1..=m].copy_from_slice(&c[1..=m]);
    }

    // Prediction weights a[k-1] = -c[k]
    (1..=p).map(|k| -c[k]).collect()
}

/// Forward + backward complex Wiener-Levinson prediction along a 1D spatial slice `s`.
fn predict_slice_wiener(s: &[Complex<f64>], filter_len: usize, wn_frac: f64) -> Vec<Complex<f64>> {
    let n = s.len();
    let p = filter_len.min(n.saturating_sub(2)).max(1);
    if n <= 2 * p {
        return s.to_vec();
    }
    let a = solve_complex_levinson_predict(s, p, wn_frac);

    let mut fwd = vec![Complex::new(0.0, 0.0); n];
    let mut bwd = vec![Complex::new(0.0, 0.0); n];
    let mut has_fwd = vec![false; n];
    let mut has_bwd = vec![false; n];

    for i in p..n {
        let mut pred = Complex::new(0.0, 0.0);
        for k in 0..p {
            pred += a[k] * s[i - 1 - k];
        }
        fwd[i] = pred;
        has_fwd[i] = true;
    }

    // Backward prediction uses conjugate weights on reversed sequence:
    // s[i] ≈ sum_{k=0..p} conj(a[k]) * s[i + 1 + k]
    for i in 0..(n - p) {
        let mut pred = Complex::new(0.0, 0.0);
        for k in 0..p {
            pred += a[k].conj() * s[i + 1 + k];
        }
        bwd[i] = pred;
        has_bwd[i] = true;
    }

    let mut out = vec![Complex::new(0.0, 0.0); n];
    for i in 0..n {
        out[i] = match (has_fwd[i], has_bwd[i]) {
            (true, true) => (fwd[i] + bwd[i]) * 0.5,
            (true, false) => fwd[i],
            (false, true) => bwd[i],
            (false, false) => s[i],
        };
    }
    out
}

/// Forward + backward complex Normalized LMS L2 adaptive prediction along a spatial slice `s`.
fn predict_slice_nlms(s: &[Complex<f64>], filter_len: usize, mu: f64) -> Vec<Complex<f64>> {
    let n = s.len();
    let p = filter_len.min(n.saturating_sub(2)).max(1);
    if n <= 2 * p {
        return s.to_vec();
    }
    let step = mu.clamp(0.005, 0.9);
    let mut w_fwd = vec![Complex::new(0.0, 0.0); p];
    let mut fwd = vec![Complex::new(0.0, 0.0); n];
    let mut has_fwd = vec![false; n];
    for i in p..n {
        let mut pred = Complex::new(0.0, 0.0);
        let mut pow = 1e-12;
        for k in 0..p {
            let xk = s[i - 1 - k];
            pred += w_fwd[k] * xk;
            pow += xk.norm_sqr();
        }
        let err = s[i] - pred;
        let scale = step / pow;
        for k in 0..p {
            let upd = w_fwd[k] + err * s[i - 1 - k].conj() * scale;
            if upd.norm() < 1e4 {
                w_fwd[k] = upd;
            }
        }
        fwd[i] = pred;
        has_fwd[i] = true;
    }

    let mut w_bwd = vec![Complex::new(0.0, 0.0); p];
    let mut bwd = vec![Complex::new(0.0, 0.0); n];
    let mut has_bwd = vec![false; n];
    for i in (0..(n - p)).rev() {
        let mut pred = Complex::new(0.0, 0.0);
        let mut pow = 1e-12;
        for k in 0..p {
            let xk = s[i + 1 + k];
            pred += w_bwd[k] * xk;
            pow += xk.norm_sqr();
        }
        let err = s[i] - pred;
        let scale = step / pow;
        for k in 0..p {
            let upd = w_bwd[k] + err * s[i + 1 + k].conj() * scale;
            if upd.norm() < 1e4 {
                w_bwd[k] = upd;
            }
        }
        bwd[i] = pred;
        has_bwd[i] = true;
    }

    let mut out = vec![Complex::new(0.0, 0.0); n];
    for i in 0..n {
        out[i] = match (has_fwd[i], has_bwd[i]) {
            (true, true) => (fwd[i] + bwd[i]) * 0.5,
            (true, false) => fwd[i],
            (false, true) => bwd[i],
            (false, false) => s[i],
        };
    }
    out
}

/// Apply 2D F-X Deconvolution to a panel of traces (a stacked section or single gather).
/// Follows ProMAX `fxdecon.pdf`: overlapping time & space windows, cosine blending, complex forward/backward prediction.
pub fn apply_fx_decon(traces: &mut [Trace], params: &FxDeconParams) -> Result<()> {
    let nx = traces.len();
    let p = params.filter_len.clamp(1, 15);
    if nx <= 2 * p {
        // ProMAX convention: ensembles with <= 2 * filter_len traces pass unfiltered.
        return Ok(());
    }
    let nt = traces[0].samples.len();
    if nt < 4 {
        return Ok(());
    }
    let dt_ms = traces[0].dt_ms().max(1e-3);
    let dt_s = dt_ms as f64 / 1000.0;

    // Save input zero-mute mask if reapply_mute is enabled
    let zero_masks: Vec<Vec<bool>> = if params.reapply_mute {
        traces
            .iter()
            .map(|tr| tr.samples.iter().map(|&s| s == 0.0).collect())
            .collect()
    } else {
        Vec::new()
    };

    // ProMAX `fxdecon.pdf` exact windowing:
    // - Vertical: core windows of length `time_window_ms`, extended by `time_overlap_ms` on BOTH top & bottom.
    // - Horizontal: core non-overlapping output blocks of length `horiz_window_traces`, extended by `filter_len`
    //   traces on BOTH left & right (`panel size = hwin + 2 * filter_len`) so interior edges have full forward+backward prediction!
    let twin_core = ((params.time_window_ms / dt_ms).round() as usize).clamp(16, nt);
    let tovl_samp = ((params.time_overlap_ms / dt_ms).round() as usize).min(twin_core / 2);

    let mut time_windows: Vec<(usize, usize, usize, usize)> = Vec::new(); // (core_a, core_b, ext_a, ext_b)
    let mut t0 = 0usize;
    while t0 < nt {
        let t1 = (t0 + twin_core).min(nt);
        let ext_a = t0.saturating_sub(tovl_samp);
        let ext_b = (t1 + tovl_samp).min(nt);
        time_windows.push((t0, t1, ext_a, ext_b));
        if t1 == nt {
            break;
        }
        t0 = t1;
    }

    let hwin = params.horiz_window_traces.max(p + 2).min(nx);
    let mut space_windows: Vec<(usize, usize, usize, usize)> = Vec::new(); // (core_xa, core_xb, ext_xa, ext_xb)
    let mut x0 = 0usize;
    while x0 < nx {
        let x1 = (x0 + hwin).min(nx);
        let ext_xa = x0.saturating_sub(p);
        let ext_xb = (x1 + p).min(nx);
        space_windows.push((x0, x1, ext_xa, ext_xb));
        if x1 == nx {
            break;
        }
        x0 = x1;
    }

    let wn_frac = (params.white_noise_pct.max(0.0) as f64) / 100.0;
    let mu = params.adaptation_rate as f64;
    let method = params.method;

    // Process each time window in parallel, then blend smoothly into output
    let max_win_len = time_windows.iter().map(|&(_, _, ea, eb)| eb - ea).max().unwrap_or(nt);
    let nfft = (max_win_len * 2).next_power_of_two().max(64);
    let mut planner = FftPlanner::<f64>::new();
    let fft_fwd = planner.plan_fft_forward(nfft);
    let fft_inv = planner.plan_fft_inverse(nfft);

    let df = 1.0 / (nfft as f64 * dt_s);
    let fmin = params.f_min_hz.max(0.0) as f64;
    let fmax = params.f_max_hz.min((0.5 / dt_s) as f32) as f64;
    if fmax <= fmin {
        return Err(SeismicError::InvalidParameter {
            param: "f_max_hz".into(),
            message: "f_max_hz must exceed f_min_hz".into(),
        });
    }
    let k_lo = ((fmin / df).floor() as usize).clamp(1, nfft / 2);
    let k_hi = ((fmax / df).ceil() as usize).clamp(k_lo, nfft / 2);

    let in_data: Vec<&[f32]> = traces.iter().map(|t| t.samples.as_slice()).collect();

    // Compute denoised patches per extended time window in parallel
    let win_results: Vec<(usize, usize, usize, usize, Vec<Vec<f64>>)> = time_windows
        .par_iter()
        .map(|&(ca, cb, ea, eb)| {
            let wlen = eb - ea;
            let mut panel_out = vec![vec![0.0f64; wlen]; nx];

            for &(cxa, cxb, exa, exb) in &space_windows {
                let sw = exb - exa;
                if sw <= 2 * p {
                    for ix in cxa..cxb {
                        for it in 0..wlen {
                            panel_out[ix][it] = in_data[ix][ea + it] as f64;
                        }
                    }
                    continue;
                }

                // Temporal FFT of every trace in this extended (time, space) patch
                let mut spec = vec![Complex::new(0.0, 0.0); sw * nfft];
                for lx in 0..sw {
                    let row = &mut spec[lx * nfft..(lx + 1) * nfft];
                    let src = in_data[exa + lx];
                    for it in 0..wlen {
                        row[it] = Complex::new(src[ea + it] as f64, 0.0);
                    }
                    fft_fwd.process(row);
                }

                // Predict along space for each frequency bin in [k_lo..=k_hi]
                let mut out_spec = vec![Complex::new(0.0, 0.0); sw * nfft];
                let mut slice_in = vec![Complex::new(0.0, 0.0); sw];
                for kf in k_lo..=k_hi {
                    for lx in 0..sw {
                        slice_in[lx] = spec[lx * nfft + kf];
                    }
                    let pred = match method {
                        FxDeconMethod::WienerLevinson => predict_slice_wiener(&slice_in, p, wn_frac),
                        FxDeconMethod::L2Adaptive => predict_slice_nlms(&slice_in, p, mu),
                    };
                    for lx in 0..sw {
                        out_spec[lx * nfft + kf] = pred[lx];
                        if kf > 0 && kf < nfft - kf {
                            out_spec[lx * nfft + (nfft - kf)] = pred[lx].conj();
                        }
                    }
                }

                // Inverse temporal FFT for ONLY the core output traces [cxa..cxb] (zero edge artifact!)
                let inv_n = 1.0 / nfft as f64;
                for gx in cxa..cxb {
                    let lx = gx - exa;
                    let row = &mut out_spec[lx * nfft..(lx + 1) * nfft];
                    fft_inv.process(row);
                    for it in 0..wlen {
                        panel_out[gx][it] = row[it].re * inv_n;
                    }
                }
            }

            (ca, cb, ea, eb, panel_out)
        })
        .collect();

    // Blend vertical overlap regions smoothly across consecutive time windows
    let mut out_acc = vec![vec![0.0f64; nt]; nx];
    let mut out_wgt = vec![0.0f64; nt];
    for (ca, cb, ea, eb, patch) in win_results {
        let wlen = eb - ea;
        let top_ovl = ca - ea;
        let bot_ovl = eb - cb;
        for it in 0..wlen {
            let gt = ea + it;
            let mut wt = 1.0f64;
            if top_ovl > 0 && gt < ca {
                let u = (gt - ea) as f64 / top_ovl as f64;
                wt = 0.5 * (1.0 - (PI * u).cos());
            } else if bot_ovl > 0 && gt >= cb {
                let u = (eb - gt) as f64 / bot_ovl as f64;
                wt = 0.5 * (1.0 - (PI * u).cos());
            }
            wt = wt.max(1e-4);
            out_wgt[gt] += wt;
            for ix in 0..nx {
                out_acc[ix][gt] += patch[ix][it] * wt;
            }
        }
    }

    for ix in 0..nx {
        if !traces[ix].is_live() {
            continue;
        }
        for it in 0..nt {
            let val = (out_acc[ix][it] / out_wgt[it].max(1e-12)) as f32;
            traces[ix].samples[it] = if params.reapply_mute && zero_masks[ix][it] {
                0.0
            } else {
                val
            };
        }
    }

    Ok(())
}
