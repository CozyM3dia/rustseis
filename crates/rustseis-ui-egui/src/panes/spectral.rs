//! Spectral QC board: amplitude spectrum with labelled bands, F-K with kept / removed zones,
//! Radon with primaries / multiples, and a before / removed / after preview.

use crate::canvas::{self, CanvasOpts, CanvasState, Raster, View2D, badge, draw_raster};
use crate::data::{self, RasterExtent, crop_rgba, render_gather_raster};
use crate::state::{AppState, Level};
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets::{self, caps, cmd, cmd_primary};
use egui::{Color32, RichText, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use egui_plot::{Line, Plot, PlotPoint, PlotPoints, Polygon, Text, VLine};
use rustseis_core::{Ensemble, Trace};
use rustseis_dsp::{BandpassParams, FkFilterParams, FkMode, RadonFilterAction, RadonMode, RadonPanel, RadonParams};
use rustseis_render::SeismicColormap;
use serde_json::json;
use std::hash::{Hash, Hasher};

pub struct SpectralState {
    // 1D spectrum
    pub ormsby: [f32; 4],
    pub preview_filter: bool,
    spec: Option<(Vec<f32>, Vec<f32>)>,
    spec_filtered: Option<(Vec<f32>, Vec<f32>)>,
    spec_key: u64,
    drag_marker: Option<usize>,
    // F-K
    pub fk: FkFilterParams,
    pub fk_show_filtered: bool,
    fk_raster: Option<Raster>,
    fk_key: u64,
    fk_extent: Option<RasterExtent>,
    fk_canvas: CanvasState,
    fk_gather: Option<Raster>,
    fk_gather_canvas: CanvasState,
    // Radon
    pub radon_source: Option<String>,
    pub radon: RadonParams,
    radon_panel: Option<RadonPanel>,
    radon_raster: Option<Raster>,
    radon_key: u64,
    radon_canvas: CanvasState,
    radon_preview: [Option<Raster>; 3],
    radon_preview_key: u64,
    radon_preview_canvas: [CanvasState; 3],
    radon_drag: Option<u8>,
    preview_requested: bool,
    /// Recompute when the gather on screen changes.
    pub follow_viewer: bool,
    /// Which tool the user touched last: 0 band-pass, 1 F-K, 2 Radon.
    pub active: u8,
    fk_kept: Option<f32>,
    radon_removed: Option<f32>,
}

impl Default for SpectralState {
    fn default() -> Self {
        Self {
            ormsby: [6.0, 12.0, 70.0, 90.0],
            preview_filter: true,
            spec: None,
            spec_filtered: None,
            spec_key: 0,
            drag_marker: None,
            fk: FkFilterParams { mode: FkMode::RejectLowVelocity, dx_m: 12.5, vel_cut_mps: 1500.0, taper_frac: 0.2, pedestal: 0.01, polygon: Vec::new() },
            fk_show_filtered: false,
            fk_raster: None,
            fk_key: 0,
            fk_extent: None,
            fk_canvas: CanvasState::default(),
            fk_gather: None,
            fk_gather_canvas: CanvasState::default(),
            radon_source: None,
            radon: RadonParams::default(),
            radon_panel: None,
            radon_raster: None,
            radon_key: 0,
            radon_canvas: CanvasState::default(),
            radon_preview: [None, None, None],
            radon_preview_key: 0,
            radon_preview_canvas: Default::default(),
            radon_drag: None,
            preview_requested: false,
            follow_viewer: true,
            active: 0,
            fk_kept: None,
            radon_removed: None,
        }
    }
}

impl SpectralState {
    /// Ask the Radon pane to run a demultiple preview on its next frame.
    pub fn request_radon_preview(&mut self) {
        self.preview_requested = true;
    }
}

fn hash_of(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// Clone of the focus ensemble, capped so spectral work stays interactive on whole sections.
fn focus_ensemble(s: &mut AppState, cap: usize) -> Option<(Vec<Trace>, u64)> {
    s.ensure_index();
    let range = s.focus_range()?;
    let sess = s.session()?;
    let traces = &s.focus.source.traces(sess)[range.start..range.end];
    let (a, b) = if traces.len() > cap {
        let mid = traces.len() / 2;
        (mid - cap / 2, mid + cap / 2)
    } else {
        (0, traces.len())
    };
    Some((traces[a..b].to_vec(), hash_of((s.rev, &s.focus.source, range.start, range.end))))
}

fn ensemble_title(s: &AppState) -> String {
    match s.focus_range() {
        Some(r) if s.focus.key == data::SECTION_KEY => format!("section · {} tr", r.len()),
        Some(r) => format!("{} {} · {} tr", s.focus.key, r.key_value, r.len()),
        None => "no data".into(),
    }
}

// ============================================================== 1D spectrum

struct SpecResult {
    key: u64,
    raw: (Vec<f32>, Vec<f32>),
    filtered: Option<(Vec<f32>, Vec<f32>)>,
}

fn spectrum_card(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    if s.session().is_some() && s.spectral.follow_viewer {
        if let Some((ens, ekey)) = focus_ensemble(s, 600) {
            let orm = s.spectral.ormsby;
            let pf = s.spectral.preview_filter;
            let key = hash_of((ekey, orm.map(f32::to_bits), pf));
            if key != s.spectral.spec_key {
                s.spectral.spec_key = key;
                s.compute.submit("spec.spectrum", move || {
                    let raw = rustseis_render::compute_average_spectrum(&ens).map_err(|e| e.to_string())?;
                    let filtered = if pf {
                        let mut f = ens.clone();
                        let params = BandpassParams { freqs: orm, ..BandpassParams::default() };
                        rustseis_dsp::apply_bandpass(&mut f, &params).map_err(|e| e.to_string())?;
                        rustseis_render::compute_average_spectrum(&f).ok()
                    } else {
                        None
                    };
                    Ok(SpecResult { key, raw, filtered })
                });
            }
        }
    }
    let Some((freqs, db)) = s.spectral.spec.clone() else {
        if !super::busy_notice(s, ui) {
            canvas::empty_state(ui, &p, icon::WAVE_SINE, "No spectrum yet", "Open data or pick a gather in the gather view.");
        }
        return;
    };
    let filtered = s.spectral.spec_filtered.clone();
    let orm = s.spectral.ormsby;
    let resp = Plot::new("spectrum_plot")
        .allow_drag(false)
        .allow_double_click_reset(true)
        .x_axis_label("Hz")
        .y_axis_label("dB")
        .include_y(0.0)
        .include_y(-80.0)
        .show_background(false)
        .show(ui, |pu| {
            let nyq = freqs.last().copied().unwrap_or(250.0) as f64;
            // Labelled bands: what each part of the spectrum usually holds.
            let bands = [
                (0.0, orm[1] as f64, p.series_d, "ground roll & swell"),
                (orm[1] as f64, orm[2] as f64, p.accent, "reflections"),
                (orm[2] as f64, nyq, p.series_c, "noise"),
            ];
            for (a, b, c, label) in bands {
                if b > a {
                    pu.polygon(Polygon::new(label, PlotPoints::from(vec![[a, -90.0], [b, -90.0], [b, 6.0], [a, 6.0]])).fill_color(with_alpha(c, 14)).stroke(Stroke::NONE));
                    if (b - a) > nyq * 0.12 {
                        pu.text(Text::new(label, PlotPoint::new((a + b) * 0.5, -84.0), RichText::new(label).size(11.0).color(c)));
                    }
                }
            }
            let pts: Vec<[f64; 2]> = freqs.iter().zip(&db).map(|(f, d)| [*f as f64, d.max(-90.0) as f64]).collect();
            pu.line(Line::new("before", PlotPoints::from(pts)).color(p.text_muted).width(1.4));
            if let Some((ff, fd)) = &filtered {
                let pts: Vec<[f64; 2]> = ff.iter().zip(fd).map(|(f, d)| [*f as f64, d.max(-90.0) as f64]).collect();
                pu.line(Line::new("after", PlotPoints::from(pts)).color(p.accent).width(2.0));
            }
            // Trapezoid response (0 dB pass, -60 dB stop)
            let trap = vec![[0.0, -60.0], [orm[0] as f64, -60.0], [orm[1] as f64, 0.0], [orm[2] as f64, 0.0], [orm[3] as f64, -60.0], [freqs.last().copied().unwrap_or(250.0) as f64, -60.0]];
            pu.line(Line::new("filter", PlotPoints::from(trap)).color(with_alpha(p.pick, 200)).width(1.2).style(egui_plot::LineStyle::dashed_dense()));
            for (i, f) in orm.iter().enumerate() {
                pu.vline(VLine::new(format!("f{}", i + 1), *f as f64).color(p.pick).width(if Some(i) == s.spectral.drag_marker { 2.5 } else { 1.4 }));
            }
            pu.pointer_coordinate()
        });
    // Marker dragging
    let r = &resp.response;
    if let Some(ptr) = resp.inner {
        if r.drag_started() {
            let px_of = |f: f32| resp.transform.position_from_point(&egui_plot::PlotPoint::new(f as f64, 0.0)).x;
            let ptr_px = resp.transform.position_from_point(&ptr).x;
            s.spectral.drag_marker = orm.iter().enumerate().map(|(i, f)| (i, (px_of(*f) - ptr_px).abs())).filter(|(_, d)| *d < 10.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
        }
        if r.dragged() {
            if let Some(i) = s.spectral.drag_marker {
                let lo = if i == 0 { 0.0 } else { s.spectral.ormsby[i - 1] + 0.5 };
                let hi = if i == 3 { 1000.0 } else { s.spectral.ormsby[i + 1] - 0.5 };
                s.spectral.ormsby[i] = (ptr.x as f32).clamp(lo, hi.max(lo));
                s.spectral.active = 0;
            }
        }
    }
    if r.drag_stopped() {
        s.spectral.drag_marker = None;
    }
}

/// Update the selected step if it is `tool`, else the first such step, else append one.
fn upsert_step(s: &mut AppState, tool: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let sel_is_tool = s.flow_sel.as_ref().and_then(|p| s.flow.get(p)).is_some_and(|n| n.tool == tool);
    let path = if sel_is_tool {
        s.flow_sel.clone()
    } else {
        s.flow.nodes.iter().position(|n| n.tool == tool).map(|i| vec![i])
    };
    match path {
        Some(pth) => {
            if let Some(n) = s.flow.get_mut(&pth) {
                edit(&mut n.params);
                s.flow.dirty = true;
            }
            s.flow_sel = Some(pth);
            s.toast(Level::Ok, format!("Updated `{tool}` step in the flow"));
        }
        None => {
            s.insert_tool(tool);
            if let Some(pth) = s.flow_sel.clone() {
                if let Some(n) = s.flow.get_mut(&pth) {
                    edit(&mut n.params);
                }
            }
            s.toast(Level::Ok, format!("Added `{tool}` step to the flow"));
        }
    }
}

// ============================================================== F-K

struct FkResult {
    key: u64,
    img: egui::ColorImage,
    extent: RasterExtent,
    gather: Option<egui::ColorImage>,
    dt_ms: f64,
    kept: f32,
}

fn energy(t: &[Trace]) -> f32 {
    t.iter().map(|x| x.samples.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>()).sum::<f64>() as f32
}

fn fk_card(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    if s.session().is_some() && s.spectral.follow_viewer {
        if let Some((ens, ekey)) = focus_ensemble(s, 1024) {
            let fk = s.spectral.fk.clone();
            let show_f = s.spectral.fk_show_filtered;
            let key = hash_of((ekey, serde_json::to_string(&fk).unwrap_or_default(), show_f));
            if key != s.spectral.fk_key && ens.len() >= 2 {
                s.spectral.fk_key = key;
                s.compute.submit("spec.fk", move || {
                    let mut filtered = Ensemble::new("FK", 0, ens.clone());
                    rustseis_dsp::apply_fk_ensemble(&mut filtered, &fk).map_err(|e| e.to_string())?;
                    let src = if show_f { &filtered.traces } else { &ens };
                    let spec = rustseis_render::compute_fk_spectrum(src, fk.dx_m).map_err(|e| e.to_string())?;
                    let (nk, nf) = (spec.wavenumbers_cpm.len(), spec.freqs_hz.len());
                    let img = rustseis_render::render_fk_spectrum_plot(src, fk.dx_m, (nk + 64) as u32, (nf + 64) as u32).map_err(|e| e.to_string())?;
                    let img = crop_rgba(&img, 32, 32, nk, nf);
                    let dk = (spec.wavenumbers_cpm.get(1).copied().unwrap_or(0.0) - spec.wavenumbers_cpm[0]) as f64;
                    let df = (spec.freqs_hz.get(1).copied().unwrap_or(1.0) - spec.freqs_hz[0]) as f64;
                    let extent = RasterExtent {
                        x0: spec.wavenumbers_cpm[0] as f64 - dk / 2.0,
                        x1: *spec.wavenumbers_cpm.last().unwrap() as f64 + dk / 2.0,
                        y0: -df / 2.0,
                        y1: *spec.freqs_hz.last().unwrap() as f64 + df / 2.0,
                    };
                    let gather = render_gather_raster(&filtered.traces, SeismicColormap::BlueWhiteRed, 98.0, data::MAX_RASTER_SIDE).ok();
                    let dt_ms = ens.first().map(|t| t.dt_ms() as f64).unwrap_or(4.0);
                    let kept = energy(&filtered.traces) / energy(&ens).max(1e-30);
                    Ok(FkResult { key, img, extent, gather, dt_ms, kept })
                });
            }
        }
    }
    let Some(extent) = s.spectral.fk_extent else {
        if !super::busy_notice(s, ui) {
            canvas::empty_state(ui, &p, icon::POLYGON, "No F-K view yet", "Pick a gather with at least two traces.");
        }
        return;
    };
    let avail = ui.available_size();
    let w_fk = avail.x * 0.56;
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(w_fk);
            fk_canvas(s, ui, &p, vec2(w_fk, avail.y), extent);
        });
        ui.vertical(|ui| {
            let w = avail.x - w_fk - 8.0;
            ui.set_width(w);
            let mut opts = CanvasOpts::seismic("trace");
            opts.background = p.canvas;
            let raster = s.spectral.fk_gather.as_ref();
            let out = canvas::canvas(ui, egui::Id::unique("fk_gather"), &mut s.spectral.fk_gather_canvas, &p, vec2(w, avail.y), opts, |painter, frame| {
                if let Some(r) = raster {
                    draw_raster(painter, frame, r, Color32::WHITE);
                }
            });
            badge(&ui.painter_at(out.frame.rect), out.frame.rect.center_top() + vec2(0.0, 8.0), "after F-K", &p, egui::Align2::CENTER_TOP);
        });
    });
}

fn fk_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2, extent: RasterExtent) {
    let mut opts = CanvasOpts::seismic("k (cyc/m)");
    opts.y_down = false;
    opts.x_axis_top = false;
    opts.y_title = "Hz".into();
    opts.primary_pans = false;
    opts.background = p.canvas;
    opts.x_ticks = 5;
    opts.x_label = Box::new(|x| Some(format!("{x:.3}")));
    let raster = s.spectral.fk_raster.as_ref();
    let out = canvas::canvas(ui, egui::Id::unique("fk_canvas"), &mut s.spectral.fk_canvas, p, size, opts, |painter, frame| {
        if let Some(r) = raster {
            draw_raster(painter, frame, r, Color32::WHITE);
        }
    });
    let painter = ui.painter_at(out.frame.rect);
    let fk = &s.spectral.fk;
    let fmax = extent.y1;
    // Velocity fan lines: f = v |k|
    let fan = |v: f64, color: Color32, w: f32| {
        if v <= 0.0 {
            return;
        }
        for sign in [-1.0, 1.0] {
            let k_end = sign * fmax / v;
            painter.line_segment([out.frame.to_screen(0.0, 0.0), out.frame.to_screen(k_end, fmax)], Stroke::new(w, color));
        }
    };
    if matches!(fk.mode, FkMode::RejectLowVelocity | FkMode::PassHighVelocity) && fk.vel_cut_mps > 0.0 {
        // Removed zone: slower than the cut, between the fan and the wavenumber axis.
        let v = fk.vel_cut_mps as f64;
        for sign in [-1.0f64, 1.0] {
            let kmax = if sign < 0.0 { extent.x0 } else { extent.x1 };
            let k_at_top = sign * fmax / v;
            let mut poly = vec![out.frame.to_screen(0.0, 0.0), out.frame.to_screen(kmax, 0.0)];
            if k_at_top.abs() < kmax.abs() {
                poly.push(out.frame.to_screen(kmax, fmax));
                poly.push(out.frame.to_screen(k_at_top, fmax));
            } else {
                poly.push(out.frame.to_screen(kmax, (v * kmax.abs()).min(fmax)));
            }
            painter.add(egui::Shape::convex_polygon(poly, with_alpha(p.danger, 38), Stroke::NONE));
            let lab = pos2(if sign < 0.0 { out.frame.rect.left() + 8.0 } else { out.frame.rect.right() - 8.0 }, out.frame.rect.bottom() - 8.0);
            let align = if sign < 0.0 { egui::Align2::LEFT_BOTTOM } else { egui::Align2::RIGHT_BOTTOM };
            painter.text(lab, align, "removed", egui::FontId::proportional(11.0), p.danger);
        }
        painter.text(out.frame.to_screen(0.0, fmax * 0.88), egui::Align2::CENTER_CENTER, "kept", egui::FontId::proportional(11.5), p.accent_text);
    }
    match fk.mode {
        FkMode::RejectLowVelocity | FkMode::PassHighVelocity => {
            let v = fk.vel_cut_mps as f64;
            fan(v, p.pick, 1.8);
            fan(v * (1.0 + fk.taper_frac as f64), with_alpha(p.pick, 110), 1.0);
            fan(v * (1.0 - fk.taper_frac as f64).max(0.05), with_alpha(p.pick, 110), 1.0);
        }
        FkMode::RejectPolygon | FkMode::PassPolygon => {
            let pts: Vec<egui::Pos2> = fk.polygon.iter().map(|(f, k)| out.frame.to_screen(*k as f64, *f as f64)).collect();
            if pts.len() >= 2 {
                let mut closed = pts.clone();
                closed.push(pts[0]);
                painter.add(egui::Shape::line(closed, Stroke::new(1.8, p.pick)));
            }
            for c in &pts {
                painter.circle(*c, 4.0, Color32::from_black_alpha(140), Stroke::new(1.8, p.pick));
            }
        }
    }
    // Reference fans for common apparent velocities.
    for v in [1500.0f64, 3000.0] {
        fan(v, with_alpha(Color32::WHITE, 40), 1.0);
    }
    if let Some((k, f)) = out.hover {
        let v = if k.abs() > 1e-9 { f / k.abs() } else { f64::INFINITY };
        badge(&painter, out.frame.rect.right_bottom() + vec2(-6.0, -6.0), &format!("{f:.1} Hz · {k:.4} cycles/m · apparent {v:.0} m/s"), p, egui::Align2::RIGHT_BOTTOM);
    }
    badge(&painter, out.frame.rect.center_top() + vec2(0.0, 8.0), if s.spectral.fk_show_filtered { "after" } else { "before" }, p, egui::Align2::CENTER_TOP);
    // Polygon editing
    if matches!(s.spectral.fk.mode, FkMode::RejectPolygon | FkMode::PassPolygon) {
        if out.response.clicked() {
            if let Some((k, f)) = out.hover {
                s.spectral.fk.polygon.push((f.max(0.0) as f32, k as f32));
            }
        }
        if out.response.secondary_clicked() {
            if let Some(ptr) = out.response.hover_pos() {
                let near = s.spectral.fk.polygon.iter().enumerate().map(|(i, (f, k))| (i, out.frame.to_screen(*k as f64, *f as f64).distance(ptr))).filter(|(_, d)| *d < 10.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
                if let Some(i) = near {
                    s.spectral.fk.polygon.remove(i);
                }
            }
        }
    } else if out.response.dragged_by(egui::PointerButton::Primary) {
        if let Some((k, f)) = out.hover {
            if k.abs() > 1e-6 && f > 0.0 {
                s.spectral.fk.vel_cut_mps = ((f / k.abs()) as f32).clamp(100.0, 10000.0);
                s.spectral.active = 1;
            }
        }
    }
}

// ============================================================== Radon

struct RadonResult {
    key: u64,
    panel: RadonPanel,
    img: egui::ColorImage,
}
struct RadonPreview {
    key: u64,
    imgs: [egui::ColorImage; 3],
    removed: f32,
}

fn radon_img(panel: &RadonPanel) -> Result<egui::ColorImage, String> {
    let (nq, nt) = (panel.q_values_ms.len(), panel.times_ms.len());
    let img = rustseis_render::render_radon_panel(&panel.times_ms, &panel.q_values_ms, &panel.data, None, (nq + 64) as u32, (nt + 64) as u32).map_err(|e| e.to_string())?;
    Ok(crop_rgba(&img, 32, 32, nq, nt))
}

fn radon_card(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let names: Vec<String> = s.session().map(|x| {
        let mut v: Vec<String> = x.context.radon_panels.keys().cloned().collect();
        v.sort();
        v
    }).unwrap_or_default();
    let _ = &names;
    let recompute = std::mem::take(&mut s.spectral.preview_requested);

    // Forward panel
    if s.session().is_some() {
        if let Some(name) = s.spectral.radon_source.clone() {
            let key = hash_of((s.tables_rev, &name));
            if key != s.spectral.radon_key {
                s.spectral.radon_key = key;
                if let Some(panel) = s.session().and_then(|x| x.context.radon_panels.get(&name)).cloned() {
                    s.compute.submit("spec.radon", move || {
                        let img = radon_img(&panel)?;
                        Ok(RadonResult { key, panel, img })
                    });
                }
            }
        } else if let Some((ens, ekey)) = focus_ensemble(s, 400).filter(|_| s.spectral.follow_viewer) {
            let rp = s.spectral.radon.clone();
            let key = hash_of((ekey, rp.mode as u8, rp.q_min_ms.to_bits(), rp.q_max_ms.to_bits(), rp.n_q, rp.sparse_iters, rp.f_min_hz.to_bits(), rp.f_max_hz.to_bits()));
            if key != s.spectral.radon_key && ens.len() >= 3 {
                s.spectral.radon_key = key;
                s.compute.submit("spec.radon", move || {
                    let panel = rustseis_dsp::forward_radon_gather(&ens, &rp).map_err(|e| e.to_string())?;
                    let img = radon_img(&panel)?;
                    Ok(RadonResult { key, panel, img })
                });
            }
        }
        if recompute {
            if let Some((ens, ekey)) = focus_ensemble(s, 400) {
                let rp = s.spectral.radon.clone();
                let key = hash_of((ekey, serde_json::to_string(&rp).unwrap_or_default()));
                s.spectral.radon_preview_key = key;
                s.compute.submit("spec.radon_preview", move || {
                    let mut out = ens.clone();
                    rustseis_dsp::apply_radon_filter_gather(&mut out, &rp).map_err(|e| e.to_string())?;
                    let multiples: Vec<Trace> = ens
                        .iter()
                        .zip(&out)
                        .map(|(a, b)| {
                            let mut t = a.clone();
                            for (x, y) in t.samples.iter_mut().zip(&b.samples) {
                                *x -= *y;
                            }
                            t
                        })
                        .collect();
                    let r = |t: &[Trace]| render_gather_raster(t, SeismicColormap::BlueWhiteRed, 98.0, data::MAX_RASTER_SIDE);
                    let removed = energy(&multiples) / energy(&ens).max(1e-30);
                    Ok(RadonPreview { key, imgs: [r(&ens)?, r(&multiples)?, r(&out)?], removed })
                });
            }
        }
    }

    let Some(panel) = s.spectral.radon_panel.clone() else {
        if !super::busy_notice(s, ui) {
            canvas::empty_state(ui, &p, icon::FUNNEL, "No Radon view yet", "Pick a CDP gather with at least three traces, or choose a saved Radon panel in the Inspector.");
        }
        return;
    };
    let avail = ui.available_size();
    let has_preview = s.spectral.radon_preview.iter().all(|r| r.is_some());
    let w_panel = if has_preview { avail.x * 0.34 } else { avail.x };
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(w_panel);
            radon_canvas(s, ui, &p, vec2(w_panel, avail.y), &panel);
        });
        if has_preview {
            let w = (avail.x - w_panel - 8.0) / 3.0 - 4.0;
            for (i, title) in ["Before", "Removed", "After"].iter().enumerate() {
                ui.vertical(|ui| {
                    ui.set_width(w);
                    let mut opts = CanvasOpts::seismic("trace");
                    opts.background = p.canvas;
                    opts.left_gutter = if i == 0 { 46.0 } else { 10.0 };
                    if i > 0 {
                        opts.y_label = Box::new(|_| None);
                        opts.y_title = String::new();
                    }
                    let raster = s.spectral.radon_preview[i].as_ref();
                    let state = &mut s.spectral.radon_preview_canvas[i];
                    let out = canvas::canvas(ui, egui::Id::unique(("radon_prev", i)), state, &p, vec2(w, avail.y), opts, |painter, frame| {
                        if let Some(r) = raster {
                            draw_raster(painter, frame, r, Color32::WHITE);
                        }
                    });
                    badge(&ui.painter_at(out.frame.rect), out.frame.rect.center_top() + vec2(0.0, 8.0), title, &p, egui::Align2::CENTER_TOP);
                    if out.changed_view {
                        let v = out.frame.view;
                        for (j, c) in s.spectral.radon_preview_canvas.iter_mut().enumerate() {
                            if j != i {
                                c.view = Some(v);
                            }
                        }
                    }
                });
            }
        }
    });
}

fn radon_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2, panel: &RadonPanel) {
    let mut opts = CanvasOpts::seismic("moveout at far offset (ms)");
    opts.primary_pans = false;
    opts.background = p.canvas;
    opts.x_ticks = 5;
    let raster = s.spectral.radon_raster.as_ref();
    let out = canvas::canvas(ui, egui::Id::unique("radon_canvas"), &mut s.spectral.radon_canvas, p, size, opts, |painter, frame| {
        if let Some(r) = raster {
            draw_raster(painter, frame, r, Color32::WHITE);
        }
    });
    let painter = ui.painter_at(out.frame.rect);
    let r = &s.spectral.radon;
    let zone = [r.q_zone_min_ms, r.q_zone_max_ms];
    let zx: Vec<f32> = zone.iter().map(|q| out.frame.x_to_screen(*q as f64)).collect();
    let shade = egui::Rect::from_x_y_ranges(zx[0].min(zx[1])..=zx[0].max(zx[1]), out.frame.rect.y_range());
    painter.rect_filled(shade, 0.0, with_alpha(p.danger, 30));
    painter.text(pos2(shade.center().x, out.frame.rect.top() + 40.0), egui::Align2::CENTER_CENTER, "multiples", egui::FontId::proportional(11.5), p.danger);
    let x_zero = out.frame.x_to_screen(0.0);
    if (x_zero - shade.center().x).abs() > 60.0 {
        painter.text(pos2(x_zero, out.frame.rect.top() + 40.0), egui::Align2::CENTER_CENTER, "primaries", egui::FontId::proportional(11.5), p.accent_text);
    }
    for (i, x) in zx.iter().enumerate() {
        let active = s.spectral.radon_drag == Some(i as u8);
        painter.vline(*x, out.frame.rect.y_range(), Stroke::new(if active { 2.6 } else { 1.8 }, p.pick));
        painter.text(pos2(*x + 4.0, out.frame.rect.bottom() - 18.0), egui::Align2::LEFT_BOTTOM, format!("{:.0} ms", zone[i]), font_mono(10.5), p.pick);
    }
    // q = 0 reference
    let x0 = out.frame.x_to_screen(0.0);
    painter.vline(x0, out.frame.rect.y_range(), Stroke::new(1.0, with_alpha(Color32::WHITE, 90)));
    // Time-variant zone polygon (if any)
    if !r.time_variant_zone.is_empty() {
        let lo: Vec<egui::Pos2> = r.time_variant_zone.iter().map(|(t, a, _)| out.frame.to_screen(*a as f64, *t as f64)).collect();
        let hi: Vec<egui::Pos2> = r.time_variant_zone.iter().map(|(t, _, b)| out.frame.to_screen(*b as f64, *t as f64)).collect();
        painter.add(egui::Shape::line(lo, Stroke::new(1.4, p.series_c)));
        painter.add(egui::Shape::line(hi, Stroke::new(1.4, p.series_c)));
    }
    let _ = panel;
    if s.compute.is_busy("spec.radon") || s.compute.is_busy("spec.radon_preview") {
        badge(&painter, out.frame.rect.right_top() + vec2(-6.0, 6.0), "computing…", p, egui::Align2::RIGHT_TOP);
    }
    // Drag zone bounds
    let resp = &out.response;
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(ptr) = resp.hover_pos() {
            s.spectral.radon_drag = zx.iter().enumerate().map(|(i, x)| (i as u8, (x - ptr.x).abs())).filter(|(_, d)| *d < 10.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
        }
    }
    if resp.dragged_by(egui::PointerButton::Primary) {
        if let (Some(i), Some((q, _))) = (s.spectral.radon_drag, out.hover) {
            let r = &mut s.spectral.radon;
            if i == 0 {
                r.q_zone_min_ms = (q as f32).min(r.q_zone_max_ms - 1.0);
            } else {
                r.q_zone_max_ms = (q as f32).max(r.q_zone_min_ms + 1.0);
            }
            s.spectral.active = 2;
        }
    }
    if resp.drag_stopped() {
        s.spectral.radon_drag = None;
    }
    if let Some((q, t)) = out.hover {
        badge(&painter, out.frame.rect.right_bottom() + vec2(-6.0, -6.0), &format!("{t:.0} ms · moveout {q:.1} ms"), p, egui::Align2::RIGHT_BOTTOM);
    }
}

pub fn on_compute(s: &mut AppState, ctx: &egui::Context, key: &str, value: Box<dyn std::any::Any + Send>) {
    match key {
        "spec.spectrum" => {
            if let Ok(r) = value.downcast::<SpecResult>() {
                if r.key == s.spectral.spec_key {
                    s.spectral.spec = Some(r.raw);
                    s.spectral.spec_filtered = r.filtered;
                }
            }
        }
        "spec.fk" => {
            if let Ok(r) = value.downcast::<FkResult>() {
                if r.key != s.spectral.fk_key {
                    return;
                }
                let r = *r;
                let first = s.spectral.fk_extent.is_none();
                s.spectral.fk_extent = Some(r.extent);
                s.spectral.fk_kept = Some(r.kept);
                match &mut s.spectral.fk_raster {
                    Some(x) => x.replace(r.img, r.extent),
                    None => s.spectral.fk_raster = Some(Raster::new(ctx, "fk", r.img, r.extent)),
                }
                s.spectral.fk_canvas.set_home(View2D::new(r.extent.x0, r.extent.x1, 0.0, r.extent.y1), first);
                if let Some(g) = r.gather {
                    let n = g.size[0] as f64;
                    let rows = g.size[1] as f64;
                    let ext = RasterExtent { x0: 0.0, x1: n, y0: -0.5 * r.dt_ms, y1: (rows - 0.5) * r.dt_ms };
                    match &mut s.spectral.fk_gather {
                        Some(x) => x.replace(g, ext),
                        None => s.spectral.fk_gather = Some(Raster::new(ctx, "fk_gather", g, ext)),
                    }
                    s.spectral.fk_gather_canvas.set_home(View2D::new(0.0, n, ext.y0, ext.y1), first);
                }
            }
        }
        "spec.radon" => {
            if let Ok(r) = value.downcast::<RadonResult>() {
                if r.key != s.spectral.radon_key {
                    return;
                }
                let r = *r;
                let q = &r.panel.q_values_ms;
                let t = &r.panel.times_ms;
                let dq = (q.get(1).copied().unwrap_or(1.0) - q[0]) as f64;
                let dt = (t.get(1).copied().unwrap_or(1.0) - t[0]) as f64;
                let ext = RasterExtent { x0: q[0] as f64 - dq / 2.0, x1: *q.last().unwrap() as f64 + dq / 2.0, y0: t[0] as f64 - dt / 2.0, y1: *t.last().unwrap() as f64 + dt / 2.0 };
                let first = s.spectral.radon_raster.is_none();
                match &mut s.spectral.radon_raster {
                    Some(x) => x.replace(r.img, ext),
                    None => s.spectral.radon_raster = Some(Raster::new(ctx, "radon", r.img, ext)),
                }
                s.spectral.radon_canvas.set_home(View2D::new(ext.x0, ext.x1, ext.y0, ext.y1), first);
                s.spectral.radon_panel = Some(r.panel);
            }
        }
        "spec.radon_preview" => {
            if let Ok(r) = value.downcast::<RadonPreview>() {
                if r.key != s.spectral.radon_preview_key {
                    return;
                }
                let r = *r;
                s.spectral.radon_removed = Some(r.removed);
                let dt = s.spectral.radon_panel.as_ref().map(|x| x.dt_us as f64 / 1000.0).unwrap_or(4.0);
                for (i, img) in r.imgs.into_iter().enumerate() {
                    let n = img.size[0] as f64;
                    let rows = img.size[1] as f64;
                    let ext = RasterExtent { x0: 0.0, x1: n, y0: -0.5 * dt, y1: (rows - 0.5) * dt };
                    match &mut s.spectral.radon_preview[i] {
                        Some(x) => x.replace(img, ext),
                        None => s.spectral.radon_preview[i] = Some(Raster::new(ctx, &format!("radon_prev_{i}"), img, ext)),
                    }
                    s.spectral.radon_preview_canvas[i].set_home(View2D::new(0.0, n, ext.y0, ext.y1), true);
                }
            }
        }
        _ => {}
    }
}

// ============================================================== board + inspector

fn card_rect(ui: &mut Ui, p: &Palette, rect: egui::Rect, title: &str, sub: &str) -> egui::Rect {
    let painter = ui.painter_at(rect);
    painter.rect(rect, egui::CornerRadius::same(10), p.sunken, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    painter.text(rect.left_top() + vec2(12.0, 10.0), egui::Align2::LEFT_TOP, title, crate::theme::font_semibold(12.5), p.text);
    let tw = painter.layout_no_wrap(title.to_string(), crate::theme::font_semibold(12.5), p.text).size().x;
    painter.text(rect.left_top() + vec2(20.0 + tw, 11.0), egui::Align2::LEFT_TOP, sub, egui::FontId::proportional(11.0), p.text_muted);
    egui::Rect::from_min_max(rect.min + vec2(6.0, 32.0), rect.max - vec2(6.0, 6.0))
}

/// The spectral board: spectrum and F-K on top, Radon below.
pub fn board_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let full = ui.available_rect_before_wrap().shrink2(vec2(8.0, 6.0));
    let gap = 8.0;
    let top_h = (full.height() - gap) * 0.5;
    let left_w = (full.width() - gap) * 0.42;
    let r_spec = egui::Rect::from_min_size(full.min, vec2(left_w, top_h));
    let r_fk = egui::Rect::from_min_max(pos2(r_spec.right() + gap, full.top()), pos2(full.right(), full.top() + top_h));
    let r_radon = egui::Rect::from_min_max(pos2(full.left(), full.top() + top_h + gap), full.max);
    let ens = ensemble_title(s);
    let kept = band_kept(s);
    let sub_spec = match kept {
        Some(k) => format!("{ens} · the filter keeps {:.0}% of the energy", k * 100.0),
        None => ens.clone(),
    };
    let inner = card_rect(ui, &p, r_spec, "Amplitude spectrum", &sub_spec);
    let mut u = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    spectrum_card(s, &mut u);
    let sub_fk = match s.spectral.fk_kept {
        Some(k) => format!("cut at {:.0} m/s · keeps {:.0}% of the energy", s.spectral.fk.vel_cut_mps, k * 100.0),
        None => format!("cut at {:.0} m/s", s.spectral.fk.vel_cut_mps),
    };
    let inner = card_rect(ui, &p, r_fk, "Frequency–wavenumber", &sub_fk);
    let mut u = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    fk_card(s, &mut u);
    let sub_r = match s.spectral.radon_removed {
        Some(r) => format!("multiples zone {:.0} to {:.0} ms · removes {:.0}% of the energy", s.spectral.radon.q_zone_min_ms, s.spectral.radon.q_zone_max_ms, r * 100.0),
        None => format!("multiples zone {:.0} to {:.0} ms · press Preview Radon to see the result", s.spectral.radon.q_zone_min_ms, s.spectral.radon.q_zone_max_ms),
    };
    let inner = card_rect(ui, &p, r_radon, "Radon", &sub_r);
    let mut u = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    radon_card(s, &mut u);
    ui.allocate_rect(full, egui::Sense::hover());
}

/// Fraction of spectral power the band-pass keeps.
fn band_kept(s: &AppState) -> Option<f32> {
    let (_, raw) = s.spectral.spec.as_ref()?;
    let (_, filt) = s.spectral.spec_filtered.as_ref()?;
    let pw = |d: &[f32]| d.iter().map(|x| 10f64.powf(*x as f64 / 10.0)).sum::<f64>();
    Some((pw(filt) / pw(raw).max(1e-30)) as f32)
}

fn meter(ui: &mut Ui, p: &Palette, label: &str, frac: Option<f32>, color: Color32, verb: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.0).color(p.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let txt = frac.map(|f| format!("{verb} {:.0}%", f * 100.0)).unwrap_or_else(|| "–".into());
            ui.label(RichText::new(txt).font(font_mono(11.5)).color(p.text_muted));
        });
    });
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, egui::CornerRadius::same(3), p.raised);
    if let Some(f) = frac {
        let mut r = rect;
        r.set_width(rect.width() * f.clamp(0.0, 1.0));
        ui.painter().rect_filled(r, egui::CornerRadius::same(3), color);
    }
}

pub fn inspector_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let names: Vec<String> = s
        .session()
        .map(|x| {
            let mut v: Vec<String> = x.context.radon_panels.keys().cloned().collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new("Spectral QC").font(crate::theme::font_display(18.0)).color(p.text));
            ui.label(RichText::new(ensemble_title(s)).size(11.5).color(p.text_muted));
            widgets::row(ui, &p, "Follow the gather view", |ui| {
                widgets::toggle(ui, &p, &mut s.spectral.follow_viewer);
            });

            caps(ui, &p, "WHAT IT CHANGES", None);
            meter(ui, &p, "Band-pass", band_kept(s), p.accent, "keeps");
            meter(ui, &p, "F-K", s.spectral.fk_kept, p.info, "keeps");
            meter(ui, &p, "Radon", s.spectral.radon_removed, p.danger, "removes");

            // ---- band-pass
            ui.add_space(4.0);
            caps(ui, &p, "BAND-PASS", Some("drag the corners on the spectrum"));
            ui.horizontal(|ui| {
                for i in 0..4 {
                    let (lo, hi) = (if i == 0 { 0.0 } else { s.spectral.ormsby[i - 1] }, if i == 3 { 500.0 } else { s.spectral.ormsby[i + 1] });
                    if ui.add(egui::DragValue::new(&mut s.spectral.ormsby[i]).range(lo..=hi).speed(0.2).suffix(" Hz").max_decimals(1)).changed() {
                        s.spectral.active = 0;
                    }
                }
            });
            widgets::row(ui, &p, "Show the filtered spectrum", |ui| {
                widgets::toggle(ui, &p, &mut s.spectral.preview_filter);
            });
            if cmd(ui, &p, icon::PLUS, "Add band-pass to the flow", false).clicked() {
                let f = s.spectral.ormsby;
                upsert_step(s, "filter", |params| params["freqs"] = json!(f));
            }

            // ---- F-K
            ui.add_space(4.0);
            caps(ui, &p, "F-K", Some("drag on the F-K plot to set the cut"));
            {
                let fk = &mut s.spectral.fk;
                widgets::row(ui, &p, "What to do", |ui| {
                    let name = |m: FkMode| match m {
                        FkMode::RejectLowVelocity => "Remove slow events",
                        FkMode::PassHighVelocity => "Keep fast events",
                        FkMode::RejectPolygon => "Remove a drawn area",
                        FkMode::PassPolygon => "Keep a drawn area",
                    };
                    egui::ComboBox::from_id_salt("fk_mode").selected_text(name(fk.mode)).width(160.0).show_ui(ui, |ui| {
                        for m in [FkMode::RejectLowVelocity, FkMode::PassHighVelocity, FkMode::RejectPolygon, FkMode::PassPolygon] {
                            ui.selectable_value(&mut fk.mode, m, name(m));
                        }
                    });
                });
                let polygon = matches!(fk.mode, FkMode::RejectPolygon | FkMode::PassPolygon);
                if polygon {
                    widgets::hint(ui, &p, icon::CURSOR_CLICK, "Click the F-K plot to add corners; right-click a corner to remove it.");
                    if cmd(ui, &p, icon::ERASER, "Clear the area", false).clicked() {
                        fk.polygon.clear();
                    }
                } else {
                    widgets::row(ui, &p, "Cut velocity", |ui| {
                        ui.add(egui::DragValue::new(&mut fk.vel_cut_mps).range(100.0..=10000.0).speed(5.0).suffix(" m/s"));
                    });
                }
                widgets::row(ui, &p, "Trace spacing", |ui| {
                    ui.add(egui::DragValue::new(&mut fk.dx_m).range(0.5..=200.0).speed(0.1).suffix(" m"));
                });
                widgets::row(ui, &p, "Taper", |ui| {
                    ui.add(egui::DragValue::new(&mut fk.taper_frac).range(0.0..=1.0).speed(0.01));
                });
            }
            widgets::row(ui, &p, "Show after filtering", |ui| {
                widgets::toggle(ui, &p, &mut s.spectral.fk_show_filtered);
            });
            if cmd(ui, &p, icon::PLUS, "Add F-K to the flow", false).clicked() {
                let v = serde_json::to_value(&s.spectral.fk).unwrap_or(json!({}));
                upsert_step(s, "fk_filt", |params| {
                    if let (Some(dst), Some(src)) = (params.as_object_mut(), v.as_object()) {
                        for (k, val) in src {
                            dst.insert(k.clone(), val.clone());
                        }
                    }
                });
            }

            // ---- Radon
            ui.add_space(4.0);
            caps(ui, &p, "RADON", Some("drag the zone edges"));
            widgets::row(ui, &p, "From", |ui| {
                let label = s.spectral.radon_source.clone().unwrap_or_else(|| "The gather on screen".into());
                egui::ComboBox::from_id_salt("radon_src").selected_text(label).width(160.0).show_ui(ui, |ui| {
                    if ui.selectable_label(s.spectral.radon_source.is_none(), "The gather on screen").clicked() {
                        s.spectral.radon_source = None;
                    }
                    for n in &names {
                        if ui.selectable_label(s.spectral.radon_source.as_ref() == Some(n), format!("Saved panel · {n}")).clicked() {
                            s.spectral.radon_source = Some(n.clone());
                        }
                    }
                });
            });
            {
                let r = &mut s.spectral.radon;
                widgets::row(ui, &p, "Curve shape", |ui| {
                    egui::ComboBox::from_id_salt("radon_mode").selected_text(if r.mode == RadonMode::Parabolic { "Parabolic" } else { "Linear" }).width(120.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.mode, RadonMode::Parabolic, "Parabolic");
                        ui.selectable_value(&mut r.mode, RadonMode::Linear, "Linear");
                    });
                });
                widgets::row(ui, &p, "Moveout range", |ui| {
                    let (qmin, qmax) = (r.q_min_ms, r.q_max_ms);
                    ui.add(egui::DragValue::new(&mut r.q_max_ms).range(qmin + 1.0..=4000.0).suffix(" ms"));
                    ui.add(egui::DragValue::new(&mut r.q_min_ms).range(-2000.0..=qmax - 1.0).suffix(" ms"));
                });
                widgets::row(ui, &p, "Curves", |ui| {
                    ui.add(egui::DragValue::new(&mut r.n_q).range(11..=401));
                });
                widgets::row(ui, &p, "Sharpening passes", |ui| {
                    ui.add(egui::DragValue::new(&mut r.sparse_iters).range(0..=6));
                });
                widgets::row(ui, &p, "Inside the zone", |ui| {
                    egui::ComboBox::from_id_salt("radon_action").selected_text(if r.action == RadonFilterAction::SubtractZone { "Remove it" } else { "Keep only it" }).width(120.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.action, RadonFilterAction::SubtractZone, "Remove it");
                        ui.selectable_value(&mut r.action, RadonFilterAction::PassZone, "Keep only it");
                    });
                });
            }
            ui.horizontal(|ui| {
                if cmd_primary(ui, &p, icon::EYE, "Preview").clicked() {
                    s.spectral.request_radon_preview();
                }
                if cmd(ui, &p, icon::PLUS, "Add Radon to the flow", false).clicked() {
                    let v = serde_json::to_value(&s.spectral.radon).unwrap_or(json!({}));
                    upsert_step(s, "radon_filter", |params| params["params"] = v);
                }
            });
        });
    });
}

/// Command-bar action: add whichever filter the user worked on last.
pub fn add_active_filter_to_flow(s: &mut AppState) {
    match s.spectral.active {
        1 => {
            let v = serde_json::to_value(&s.spectral.fk).unwrap_or(json!({}));
            upsert_step(s, "fk_filt", |params| {
                if let (Some(dst), Some(src)) = (params.as_object_mut(), v.as_object()) {
                    for (k, val) in src {
                        dst.insert(k.clone(), val.clone());
                    }
                }
            });
        }
        2 => {
            let v = serde_json::to_value(&s.spectral.radon).unwrap_or(json!({}));
            upsert_step(s, "radon_filter", |params| params["params"] = v);
        }
        _ => {
            let f = s.spectral.ormsby;
            upsert_step(s, "filter", |params| params["freqs"] = json!(f));
        }
    }
}
