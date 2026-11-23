use rustfft::{FftPlanner, num_complex::Complex};
use rustseis_core::{Result, SeismicError, Trace, VelocityFunction};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Stroke, Transform};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SeismicColormap {
    /// Classic ProMAX seismic Blue-White-Red diverging palette.
    #[default]
    BlueWhiteRed,
    /// Greyscale variable density.
    Greyscale,
    /// Semblance heat colormap (dark navy -> cyan -> yellow -> red).
    Semblance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatherRenderConfig {
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub colormap: SeismicColormap,
    /// Percentile for amplitude clip normalization (e.g., 98.0).
    #[serde(default = "default_clip")]
    pub clip_percentile: f32,
    /// Overlay wiggle traces + positive VA fill when trace count <= 140.
    #[serde(default = "default_true")]
    pub draw_wiggles: bool,
    /// Max time to render in ms (`None` = full trace).
    #[serde(default)]
    pub max_time_ms: Option<f32>,
}

fn default_clip() -> f32 {
    98.0
}
fn default_true() -> bool {
    true
}

impl Default for GatherRenderConfig {
    fn default() -> Self {
        Self {
            width: 1200,
            height: 800,
            colormap: SeismicColormap::BlueWhiteRed,
            clip_percentile: 98.0,
            draw_wiggles: true,
            max_time_ms: None,
        }
    }
}

/// Pure-Rust RGBA image buffer returned by `L3 render` (used by BOTH future Opus `egui` GUI & MCP PNGs).
pub struct RenderedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RenderedImage {
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<()> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = File::create(path.as_ref())?;
        let w = BufWriter::new(file);
        let mut encoder = png::Encoder::new(w, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| SeismicError::InvalidFormat(e.to_string()))?;
        writer
            .write_image_data(&self.rgba)
            .map_err(|e| SeismicError::InvalidFormat(e.to_string()))?;
        Ok(())
    }
}

#[inline]
fn map_color(norm: f32, cmap: SeismicColormap) -> [u8; 4] {
    match cmap {
        SeismicColormap::BlueWhiteRed => {
            let v = norm.clamp(-1.0, 1.0);
            if v >= 0.0 {
                let r = 255u8;
                let g = ((1.0 - v) * 255.0) as u8;
                let b = ((1.0 - v) * 255.0) as u8;
                [r, g, b, 255]
            } else {
                let t = -v;
                let r = ((1.0 - t) * 255.0) as u8;
                let g = ((1.0 - t) * 255.0) as u8;
                let b = 255u8;
                [r, g, b, 255]
            }
        }
        SeismicColormap::Greyscale => {
            let g = (((norm.clamp(-1.0, 1.0) + 1.0) * 0.5) * 255.0) as u8;
            [g, g, g, 255]
        }
        SeismicColormap::Semblance => {
            let t = norm.clamp(0.0, 1.0);
            let (r, g, b) = if t < 0.25 {
                let u = t / 0.25;
                (15.0 + u * 10.0, 20.0 + u * 80.0, 60.0 + u * 140.0)
            } else if t < 0.55 {
                let u = (t - 0.25) / 0.30;
                (25.0 + u * 50.0, 100.0 + u * 130.0, 200.0 - u * 80.0)
            } else if t < 0.80 {
                let u = (t - 0.55) / 0.25;
                (75.0 + u * 180.0, 230.0 + u * 20.0, 120.0 - u * 100.0)
            } else {
                let u = (t - 0.80) / 0.20;
                (255.0, 250.0 - u * 210.0, 20.0)
            };
            [r as u8, g as u8, b as u8, 255]
        }
    }
}

fn compute_clip_scale(traces: &[Trace], max_samp: usize, percentile: f32) -> f32 {
    let mut abs_vals = Vec::new();
    let step_tr = (traces.len() / 64).max(1);
    let step_s = (max_samp / 256).max(1);
    for tr in traces.iter().step_by(step_tr) {
        if !tr.is_live() {
            continue;
        }
        for &s in tr.samples.iter().take(max_samp).step_by(step_s) {
            if s.is_finite() && s != 0.0 {
                abs_vals.push(s.abs());
            }
        }
    }
    if abs_vals.is_empty() {
        return 1.0;
    }
    abs_vals.sort_by(|a, b| a.total_cmp(b));
    let idx = (((percentile.clamp(50.0, 100.0) / 100.0) * (abs_vals.len() - 1) as f32).round()
        as usize)
        .min(abs_vals.len() - 1);
    abs_vals[idx].max(1e-12)
}

/// Render a seismic gather or stacked/migrated 2D section to RGBA (`L3 render`).
pub fn render_seismic_gather(traces: &[Trace], cfg: &GatherRenderConfig) -> Result<RenderedImage> {
    if traces.is_empty() || traces[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Cannot render empty trace set".into()));
    }
    let w = cfg.width.max(64);
    let h = cfg.height.max(64);
    let mut pixmap = Pixmap::new(w, h)
        .ok_or_else(|| SeismicError::InvalidFormat("Invalid render dimensions".into()))?;
    pixmap.fill(Color::from_rgba8(18, 22, 30, 255));

    let pad_l = 48u32;
    let pad_r = 16u32;
    let pad_t = 24u32;
    let pad_b = 24u32;
    let plot_w = (w - pad_l - pad_r).max(1);
    let plot_h = (h - pad_t - pad_b).max(1);

    let nt_total = traces[0].samples.len();
    let dt_ms = traces[0].dt_ms().max(1e-3);
    let nt = cfg
        .max_time_ms
        .map(|m| ((m / dt_ms).round() as usize + 1).min(nt_total))
        .unwrap_or(nt_total)
        .max(2);
    let nx = traces.len();
    let clip = compute_clip_scale(traces, nt, cfg.clip_percentile);

    let pixels = pixmap.data_mut();
    for py in 0..plot_h {
        let it = ((py as f64 / (plot_h - 1).max(1) as f64) * (nt - 1) as f64).round() as usize;
        let y_img = (pad_t + py) as usize;
        for px in 0..plot_w {
            let ix = ((px as f64 / (plot_w - 1).max(1) as f64) * (nx - 1) as f64).round() as usize;
            let amp = traces[ix].samples.get(it).copied().unwrap_or(0.0);
            let norm = (amp / clip).clamp(-1.0, 1.0);
            let c = map_color(norm, cfg.colormap);
            let x_img = (pad_l + px) as usize;
            let off = (y_img * w as usize + x_img) * 4;
            pixels[off..off + 4].copy_from_slice(&c);
        }
    }

    if cfg.draw_wiggles && nx <= 140 {
        let mut stroke_paint = Paint::default();
        stroke_paint.set_color_rgba8(20, 20, 24, 210);
        stroke_paint.anti_alias = true;
        let stroke = Stroke { width: 1.0, ..Default::default() };
        let trace_dx = plot_w as f32 / nx.max(1) as f32;
        let gain_px = trace_dx * 0.75;
        let y_step = (nt / plot_h as usize).max(1);

        for ix in 0..nx {
            if !traces[ix].is_live() {
                continue;
            }
            let x0 = pad_l as f32 + (ix as f32 + 0.5) * trace_dx;
            let mut pb = PathBuilder::new();
            let mut started = false;
            for it in (0..nt).step_by(y_step) {
                let y = pad_t as f32 + (it as f32 / (nt - 1) as f32) * plot_h as f32;
                let amp = (traces[ix].samples[it] / clip).clamp(-1.5, 1.5);
                let x = x0 + amp * gain_px;
                if !started {
                    pb.move_to(x, y);
                    started = true;
                } else {
                    pb.line_to(x, y);
                }
            }
            if let Some(path) = pb.finish() {
                pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
            }
        }
    }

    let mut border_paint = Paint::default();
    border_paint.set_color_rgba8(200, 210, 225, 180);
    let mut grid_paint = Paint::default();
    grid_paint.set_color_rgba8(140, 155, 175, 70);
    let thin = Stroke { width: 1.0, ..Default::default() };

    for g in 1..10 {
        let gy = pad_t as f32 + (g as f32 / 10.0) * plot_h as f32;
        let mut pb = PathBuilder::new();
        pb.move_to(pad_l as f32, gy);
        pb.line_to((pad_l + plot_w) as f32, gy);
        if let Some(path) = pb.finish() {
            pixmap.stroke_path(&path, &grid_paint, &thin, Transform::identity(), None);
        }
    }
    let mut pb = PathBuilder::new();
    pb.move_to(pad_l as f32, pad_t as f32);
    pb.line_to((pad_l + plot_w) as f32, pad_t as f32);
    pb.line_to((pad_l + plot_w) as f32, (pad_t + plot_h) as f32);
    pb.line_to(pad_l as f32, (pad_t + plot_h) as f32);
    pb.close();
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &border_paint, &thin, Transform::identity(), None);
    }

    Ok(RenderedImage { width: w, height: h, rgba: pixmap.take() })
}

pub fn render_semblance_panel(
    times_ms: &[f32],
    vels_mps: &[f32],
    values: &[f32],
    picks: Option<&VelocityFunction>,
    width: u32,
    height: u32,
) -> Result<RenderedImage> {
    let nt = times_ms.len();
    let nv = vels_mps.len();
    if nt == 0 || nv == 0 || values.len() < nt * nv {
        return Err(SeismicError::InvalidFormat("Invalid semblance panel data".into()));
    }
    let w = width.max(64);
    let h = height.max(64);
    let mut pixmap = Pixmap::new(w, h)
        .ok_or_else(|| SeismicError::InvalidFormat("Invalid dimensions".into()))?;
    pixmap.fill(Color::from_rgba8(15, 20, 32, 255));

    let pad = 32u32;
    let pw = (w - 2 * pad).max(1);
    let ph = (h - 2 * pad).max(1);
    let vmax_s = values.iter().cloned().fold(0.0f32, f32::max).max(1e-6);

    let pixels = pixmap.data_mut();
    for py in 0..ph {
        let it = ((py as f64 / (ph - 1).max(1) as f64) * (nt - 1) as f64).round() as usize;
        for px in 0..pw {
            let iv = ((px as f64 / (pw - 1).max(1) as f64) * (nv - 1) as f64).round() as usize;
            let norm = (values[it * nv + iv] / vmax_s).clamp(0.0, 1.0);
            let c = map_color(norm, SeismicColormap::Semblance);
            let off = (((pad + py) * w + (pad + px)) as usize) * 4;
            pixels[off..off + 4].copy_from_slice(&c);
        }
    }

    if let Some(vf) = picks {
        if vf.picks.len() >= 2 {
            let t_min = times_ms[0];
            let t_max = times_ms[nt - 1].max(t_min + 1.0);
            let v_min = vels_mps[0];
            let v_max = vels_mps[nv - 1].max(v_min + 1.0);
            let mut pb = PathBuilder::new();
            for (i, &(t, v)) in vf.picks.iter().enumerate() {
                let x = pad as f32 + ((v - v_min) / (v_max - v_min)).clamp(0.0, 1.0) * pw as f32;
                let y = pad as f32 + ((t - t_min) / (t_max - t_min)).clamp(0.0, 1.0) * ph as f32;
                if i == 0 { pb.move_to(x, y); } else { pb.line_to(x, y); }
            }
            if let Some(path) = pb.finish() {
                let mut white = Paint::default();
                white.set_color_rgba8(255, 255, 255, 240);
                white.anti_alias = true;
                let st = Stroke { width: 2.5, ..Default::default() };
                pixmap.stroke_path(&path, &white, &st, Transform::identity(), None);
            }
        }
    }

    Ok(RenderedImage { width: w, height: h, rgba: pixmap.take() })
}

pub fn compute_average_spectrum(traces: &[Trace]) -> Result<(Vec<f32>, Vec<f32>)> {
    if traces.is_empty() || traces[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Empty traces for spectrum".into()));
    }
    let nt = traces[0].samples.len();
    let dt_s = traces[0].dt_s() as f64;
    let nfft = nt.next_power_of_two().max(64);
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(nfft);
    let n_pos = nfft / 2 + 1;
    let mut acc = vec![0.0f64; n_pos];
    let mut live_n = 0usize;

    let mut buf = vec![Complex::new(0.0, 0.0); nfft];
    for tr in traces {
        if !tr.is_live() {
            continue;
        }
        buf.fill(Complex::new(0.0, 0.0));
        for i in 0..nt.min(tr.samples.len()) {
            buf[i] = Complex::new(tr.samples[i] as f64, 0.0);
        }
        fft.process(&mut buf);
        for k in 0..n_pos {
            acc[k] += buf[k].norm();
        }
        live_n += 1;
    }
    if live_n == 0 {
        return Err(SeismicError::InvalidFormat("No live traces for spectrum".into()));
    }
    let max_a = acc.iter().cloned().fold(0.0f64, f64::max).max(1e-20);
    let df = 1.0 / (nfft as f64 * dt_s);
    let freqs: Vec<f32> = (0..n_pos).map(|k| (k as f64 * df) as f32).collect();
    let db: Vec<f32> = acc
        .iter()
        .map(|&a| (20.0 * ((a / max_a).max(1e-6)).log10()) as f32)
        .collect();
    Ok((freqs, db))
}

/// Render 1D average amplitude spectrum (dB vs Hz) to RGBA/PNG.
pub fn render_spectrum_plot(traces: &[Trace], width: u32, height: u32) -> Result<RenderedImage> {
    let (freqs, db) = compute_average_spectrum(traces)?;
    let w = width.max(128);
    let h = height.max(128);
    let mut pixmap = Pixmap::new(w, h)
        .ok_or_else(|| SeismicError::InvalidFormat("Invalid dimensions".into()))?;
    pixmap.fill(Color::from_rgba8(18, 22, 30, 255));

    let pad = 36u32;
    let pw = (w - 2 * pad) as f32;
    let ph = (h - 2 * pad) as f32;
    let f_max = *freqs.last().unwrap_or(&125.0);

    let mut pb = PathBuilder::new();
    for (i, (&f, &d)) in freqs.iter().zip(db.iter()).enumerate() {
        let x = pad as f32 + (f / f_max.max(1.0)) * pw;
        let y_norm = (d.clamp(-80.0, 0.0) + 80.0) / 80.0;
        let y = pad as f32 + (1.0 - y_norm) * ph;
        if i == 0 { pb.move_to(x, y); } else { pb.line_to(x, y); }
    }
    if let Some(path) = pb.finish() {
        let mut cyan = Paint::default();
        cyan.set_color_rgba8(64, 210, 255, 255);
        cyan.anti_alias = true;
        let st = Stroke { width: 2.0, ..Default::default() };
        pixmap.stroke_path(&path, &cyan, &st, Transform::identity(), None);
    }

    Ok(RenderedImage { width: w, height: h, rgba: pixmap.take() })
}

/// 2D F-K log-amplitude spectrum (`f_hz` vs signed `k_cpm`) for Spectral/F-K QC & MCP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FkSpectrum {
    pub freqs_hz: Vec<f32>,
    pub wavenumbers_cpm: Vec<f32>,
    pub db_grid: Vec<f32>,
}

pub fn compute_fk_spectrum(traces: &[Trace], dx_m: f32) -> Result<FkSpectrum> {
    if traces.len() < 2 || traces[0].samples.is_empty() {
        return Err(SeismicError::InvalidFormat("Need >= 2 traces for F-K spectrum".into()));
    }
    let nx = traces.len();
    let nt = traces[0].samples.len();
    let dt_s = traces[0].dt_s() as f64;
    let dx = dx_m.max(0.1) as f64;
    let nfft_t = nt.next_power_of_two().max(64);
    let nfft_x = (nx * 2).next_power_of_two().max(32);

    let mut planner = FftPlanner::<f64>::new();
    let fft_t = planner.plan_fft_forward(nfft_t);
    let fft_x = planner.plan_fft_forward(nfft_x);

    let mut grid = vec![Complex::new(0.0, 0.0); nfft_x * nfft_t];
    for ix in 0..nx {
        let row = &mut grid[ix * nfft_t..(ix + 1) * nfft_t];
        if traces[ix].is_live() {
            for it in 0..nt.min(traces[ix].samples.len()) {
                row[it] = Complex::new(traces[ix].samples[it] as f64, 0.0);
            }
            fft_t.process(row);
        }
    }
    let mut col = vec![Complex::new(0.0, 0.0); nfft_x];
    for ifreq in 0..=nfft_t / 2 {
        for ix in 0..nfft_x { col[ix] = grid[ix * nfft_t + ifreq]; }
        fft_x.process(&mut col);
        for ix in 0..nfft_x { grid[ix * nfft_t + ifreq] = col[ix]; }
    }

    let nf = nfft_t / 2 + 1;
    let df = 1.0 / (nfft_t as f64 * dt_s);
    let dk = 1.0 / (nfft_x as f64 * dx);
    let freqs_hz: Vec<f32> = (0..nf).map(|i| (i as f64 * df) as f32).collect();

    let half_k = nfft_x / 2;
    let wavenumbers_cpm: Vec<f32> = (0..nfft_x)
        .map(|i| ((i as isize - half_k as isize) as f64 * dk) as f32)
        .collect();

    let mut mags = vec![0.0f64; nf * nfft_x];
    let mut max_m = 1e-20f64;
    for ifreq in 0..nf {
        for ik_shifted in 0..nfft_x {
            let ik = (ik_shifted + half_k) % nfft_x;
            let m = grid[ik * nfft_t + ifreq].norm();
            mags[ifreq * nfft_x + ik_shifted] = m;
            if m > max_m { max_m = m; }
        }
    }
    let db_grid: Vec<f32> = mags
        .into_iter()
        .map(|m| (20.0 * (m / max_m).max(1e-3).log10()).clamp(-60.0, 0.0) as f32)
        .collect();

    Ok(FkSpectrum { freqs_hz, wavenumbers_cpm, db_grid })
}

/// Render a 2D F-K log-amplitude spectrum to RGBA/PNG.
pub fn render_fk_spectrum_plot(
    traces: &[Trace],
    dx_m: f32,
    width: u32,
    height: u32,
) -> Result<RenderedImage> {
    let fk = compute_fk_spectrum(traces, dx_m)?;
    let nf = fk.freqs_hz.len();
    let nk = fk.wavenumbers_cpm.len();
    let w = width.max(128);
    let h = height.max(128);
    let mut pixmap = Pixmap::new(w, h)
        .ok_or_else(|| SeismicError::InvalidFormat("Invalid dimensions".into()))?;
    pixmap.fill(Color::from_rgba8(15, 20, 32, 255));

    let pad = 32u32;
    let pw = (w - 2 * pad).max(1);
    let ph = (h - 2 * pad).max(1);
    let pixels = pixmap.data_mut();
    for py in 0..ph {
        let ifreq = ((py as f64 / (ph - 1).max(1) as f64) * (nf - 1) as f64).round() as usize;
        for px in 0..pw {
            let ik = ((px as f64 / (pw - 1).max(1) as f64) * (nk - 1) as f64).round() as usize;
            let norm = (fk.db_grid[ifreq * nk + ik] + 60.0) / 60.0;
            let c = map_color(norm, SeismicColormap::Semblance);
            let off = (((pad + py) * w + (pad + px)) as usize) * 4;
            pixels[off..off + 4].copy_from_slice(&c);
        }
    }
    Ok(RenderedImage { width: w, height: h, rgba: pixmap.take() })
}

/// Render a Radon `(tau, q)` or `(tau, p)` panel with optional multiple-zone vertical bounds.
pub fn render_radon_panel(
    times_ms: &[f32],
    q_values_ms: &[f32],
    data: &[f32],
    zone_bounds_ms: Option<(f32, f32)>,
    width: u32,
    height: u32,
) -> Result<RenderedImage> {
    let nt = times_ms.len();
    let nq = q_values_ms.len();
    if nt == 0 || nq == 0 || data.len() < nq * nt {
        return Err(SeismicError::InvalidFormat("Invalid Radon panel data".into()));
    }
    let w = width.max(128);
    let h = height.max(128);
    let mut pixmap = Pixmap::new(w, h)
        .ok_or_else(|| SeismicError::InvalidFormat("Invalid dimensions".into()))?;
    pixmap.fill(Color::from_rgba8(15, 20, 32, 255));

    let pad = 32u32;
    let pw = (w - 2 * pad).max(1);
    let ph = (h - 2 * pad).max(1);

    let mut abs_v: Vec<f32> = data
        .iter()
        .step_by((data.len() / 8192).max(1))
        .filter(|v| v.is_finite() && **v != 0.0)
        .map(|v| v.abs())
        .collect();
    abs_v.sort_by(|a, b| a.total_cmp(b));
    let clip = abs_v
        .get(((abs_v.len().saturating_sub(1)) as f32 * 0.985) as usize)
        .copied()
        .unwrap_or(1.0)
        .max(1e-12);

    let pixels = pixmap.data_mut();
    for py in 0..ph {
        let it = ((py as f64 / (ph - 1).max(1) as f64) * (nt - 1) as f64).round() as usize;
        for px in 0..pw {
            let iq = ((px as f64 / (pw - 1).max(1) as f64) * (nq - 1) as f64).round() as usize;
            let norm = (data[iq * nt + it] / clip).clamp(-1.0, 1.0);
            let c = map_color(norm, SeismicColormap::BlueWhiteRed);
            let off = (((pad + py) * w + (pad + px)) as usize) * 4;
            pixels[off..off + 4].copy_from_slice(&c);
        }
    }

    let q_min = q_values_ms[0];
    let q_max = q_values_ms[nq - 1].max(q_min + 1e-3);
    let draw_vline = |pm: &mut Pixmap, q: f32, rgba: [u8; 4], width_px: f32| {
        if q >= q_min && q <= q_max {
            let x = pad as f32 + ((q - q_min) / (q_max - q_min)) * pw as f32;
            let mut pb = PathBuilder::new();
            pb.move_to(x, pad as f32);
            pb.line_to(x, (pad + ph) as f32);
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
                let st = Stroke { width: width_px, ..Default::default() };
                pm.stroke_path(&path, &paint, &st, Transform::identity(), None);
            }
        }
    };
    draw_vline(&mut pixmap, 0.0, [255, 255, 255, 180], 1.5);
    if let Some((q_lo, q_hi)) = zone_bounds_ms {
        draw_vline(&mut pixmap, q_lo, [255, 210, 40, 235], 2.0);
        draw_vline(&mut pixmap, q_hi, [255, 210, 40, 235], 2.0);
    }

    Ok(RenderedImage { width: w, height: h, rgba: pixmap.take() })
}

/// Render a seismic gather with a crisp pick curve overlay (first-break picks or mute picks in ms).
pub fn render_gather_with_picks(
    traces: &[Trace],
    cfg: &GatherRenderConfig,
    picks_ms: &[Option<f32>],
) -> Result<RenderedImage> {
    let base = render_seismic_gather(traces, cfg)?;
    let w = base.width;
    let h = base.height;
    let mut pixmap = Pixmap::from_vec(base.rgba, tiny_skia::IntSize::from_wh(w, h).unwrap())
        .ok_or_else(|| SeismicError::InvalidFormat("Invalid pixmap".into()))?;

    let pad_l = 48u32;
    let pad_r = 16u32;
    let pad_t = 24u32;
    let pad_b = 24u32;
    let plot_w = (w - pad_l - pad_r).max(1) as f32;
    let plot_h = (h - pad_t - pad_b).max(1) as f32;

    let nt_total = traces[0].samples.len();
    let dt_ms = traces[0].dt_ms().max(1e-3);
    let max_t = cfg
        .max_time_ms
        .unwrap_or((nt_total.saturating_sub(1)) as f32 * dt_ms)
        .max(1.0);
    let nx = traces.len().max(1);

    let mut pb = PathBuilder::new();
    let mut active = false;
    for (ix, opt_t) in picks_ms.iter().enumerate().take(nx) {
        if let Some(t_ms) = opt_t {
            if *t_ms >= 0.0 && *t_ms <= max_t {
                let x = pad_l as f32 + (ix as f32 / (nx - 1).max(1) as f32) * plot_w;
                let y = pad_t as f32 + (*t_ms / max_t) * plot_h;
                if !active {
                    pb.move_to(x, y);
                    active = true;
                } else {
                    pb.line_to(x, y);
                }
            } else {
                active = false;
            }
        } else {
            active = false;
        }
    }
    if let Some(path) = pb.finish() {
        let mut yellow = Paint::default();
        yellow.set_color_rgba8(255, 225, 40, 245);
        yellow.anti_alias = true;
        let st = Stroke { width: 2.5, ..Default::default() };
        pixmap.stroke_path(&path, &yellow, &st, Transform::identity(), None);
    }

    Ok(RenderedImage { width: w, height: h, rgba: pixmap.take() })
}

/// Render a 1D X-Y diagnostic curve (e.g. statics vs station, SC amplitude vs shot, VSP velocity vs depth).
pub fn render_xy_curve_plot(
    xs: &[f32],
    ys: &[f32],
    width: u32,
    height: u32,
) -> Result<RenderedImage> {
    if xs.len() < 2 || xs.len() != ys.len() {
        return Err(SeismicError::InvalidFormat("Need >= 2 equal-length (x, y) points".into()));
    }
    let w = width.max(128);
    let h = height.max(128);
    let mut pixmap = Pixmap::new(w, h)
        .ok_or_else(|| SeismicError::InvalidFormat("Invalid dimensions".into()))?;
    pixmap.fill(Color::from_rgba8(18, 22, 30, 255));

    let pad = 36u32;
    let pw = (w - 2 * pad) as f32;
    let ph = (h - 2 * pad) as f32;

    let x_min = xs.iter().cloned().fold(f32::INFINITY, f32::min);
    let x_max = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let y_min = ys.iter().cloned().fold(f32::INFINITY, f32::min);
    let y_max = ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let x_span = (x_max - x_min).abs().max(1e-6);
    let y_span = (y_max - y_min).abs().max(1e-6);

    if y_min < 0.0 && y_max > 0.0 {
        let y0 = pad as f32 + (1.0 - (0.0 - y_min) / y_span) * ph;
        let mut pb = PathBuilder::new();
        pb.move_to(pad as f32, y0);
        pb.line_to(pad as f32 + pw, y0);
        if let Some(path) = pb.finish() {
            let mut gp = Paint::default();
            gp.set_color_rgba8(150, 165, 185, 110);
            pixmap.stroke_path(&path, &gp, &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
        }
    }

    let mut pb = PathBuilder::new();
    for (i, (&x, &y)) in xs.iter().zip(ys.iter()).enumerate() {
        let px = pad as f32 + ((x - x_min) / x_span) * pw;
        let py = pad as f32 + (1.0 - (y - y_min) / y_span) * ph;
        if i == 0 { pb.move_to(px, py); } else { pb.line_to(px, py); }
    }
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(80, 225, 165, 255);
        paint.anti_alias = true;
        pixmap.stroke_path(&path, &paint, &Stroke { width: 2.0, ..Default::default() }, Transform::identity(), None);
    }

    Ok(RenderedImage { width: w, height: h, rgba: pixmap.take() })
}

/// Render two seismic sections/gathers side-by-side in one composite PNG (Before vs After QC).
pub fn render_side_by_side(
    left_traces: &[Trace],
    right_traces: &[Trace],
    cfg: &GatherRenderConfig,
) -> Result<RenderedImage> {
    let mut half_cfg = cfg.clone();
    half_cfg.width = (cfg.width / 2).max(64);
    let left = render_seismic_gather(left_traces, &half_cfg)?;
    let right = render_seismic_gather(right_traces, &half_cfg)?;
    let gap = 8u32;
    let total_w = left.width + gap + right.width;
    let total_h = left.height.max(right.height);
    let mut rgba = vec![0u8; (total_w * total_h * 4) as usize];
    for y in 0..total_h {
        for x in 0..total_w {
            let dst = ((y * total_w + x) * 4) as usize;
            if x < left.width && y < left.height {
                let src = ((y * left.width + x) * 4) as usize;
                rgba[dst..dst + 4].copy_from_slice(&left.rgba[src..src + 4]);
            } else if x >= left.width + gap && y < right.height {
                let rx = x - left.width - gap;
                let src = ((y * right.width + rx) * 4) as usize;
                rgba[dst..dst + 4].copy_from_slice(&right.rgba[src..src + 4]);
            } else {
                rgba[dst..dst + 4].copy_from_slice(&[255, 210, 40, 255]);
            }
        }
    }
    Ok(RenderedImage { width: total_w, height: total_h, rgba })
}
