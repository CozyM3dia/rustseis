//! Workspace 4: Radon, F-K & spectral QC studio.

use crate::canvas::{self, CanvasOpts, CanvasState, Raster, View2D, badge, draw_raster};
use crate::data::{self, RasterExtent, crop_rgba, render_gather_raster};
use crate::state::{AppState, Level};
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets::{self, chip, primary_button, segmented, vsep};
use egui::{Color32, RichText, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use egui_plot::{Line, Plot, PlotPoints, VLine};
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

pub fn spectrum_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    widgets::toolbar(ui, &p, |ui| {
        ui.label(RichText::new(ensemble_title(s)).font(font_mono(11.0)).color(p.text_muted));
        vsep(ui, &p);
        ui.label(RichText::new("Ormsby").size(11.5));
        for i in 0..4 {
            let (lo, hi) = (if i == 0 { 0.0 } else { s.spectral.ormsby[i - 1] }, if i == 3 { 500.0 } else { s.spectral.ormsby[i + 1] });
            ui.add(egui::DragValue::new(&mut s.spectral.ormsby[i]).range(lo..=hi).speed(0.2).suffix(" Hz").max_decimals(1));
        }
        ui.checkbox(&mut s.spectral.preview_filter, "Preview filtered");
        vsep(ui, &p);
        if ui.button(format!("{}  Send to flow", icon::FLOW_ARROW)).on_hover_text("Insert or update a `filter` step with these corner frequencies").clicked() {
            let f = s.spectral.ormsby;
            upsert_step(s, "filter", |params| params["freqs"] = json!(f));
        }
    });
    if s.session().is_some() {
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
            canvas::empty_state(ui, &p, icon::WAVE_SINE, "No spectrum", "Select data in the Trace Viewer");
        }
        return;
    };
    let filtered = s.spectral.spec_filtered.clone();
    let peak = freqs.iter().zip(&db).max_by(|a, b| a.1.total_cmp(b.1)).map(|(f, _)| *f).unwrap_or(0.0);
    let bw = band_width(&freqs, &db, -12.0);
    widgets::body(ui, |ui| {
        ui.horizontal(|ui| {
            chip(ui, &format!("peak {peak:.1} Hz"), p.accent, with_alpha(p.accent, 22));
            if let Some((a, b)) = bw {
                chip(ui, &format!("-12 dB band {a:.0}–{b:.0} Hz"), p.text_muted, p.sunken);
            }
            chip(ui, &format!("Nyquist {:.0} Hz", freqs.last().copied().unwrap_or(0.0)), p.text_muted, p.sunken);
        });
    });
    let orm = s.spectral.ormsby;
    let resp = Plot::new("spectrum_plot")
        .allow_drag(false)
        .allow_double_click_reset(true)
        .x_axis_label("Hz")
        .y_axis_label("dB")
        .include_y(0.0)
        .include_y(-80.0)
        .show_background(false)
        .legend(egui_plot::Legend::default())
        .show(ui, |pu| {
            let pts: Vec<[f64; 2]> = freqs.iter().zip(&db).map(|(f, d)| [*f as f64, d.max(-90.0) as f64]).collect();
            pu.line(Line::new("input", PlotPoints::from(pts)).color(p.series_b).width(1.6).fill(-90.0).fill_alpha(0.12));
            if let Some((ff, fd)) = &filtered {
                let pts: Vec<[f64; 2]> = ff.iter().zip(fd).map(|(f, d)| [*f as f64, d.max(-90.0) as f64]).collect();
                pu.line(Line::new("filtered", PlotPoints::from(pts)).color(p.accent).width(1.8));
            }
            // Trapezoid response (0 dB pass, -60 dB stop)
            let trap = vec![[0.0, -60.0], [orm[0] as f64, -60.0], [orm[1] as f64, 0.0], [orm[2] as f64, 0.0], [orm[3] as f64, -60.0], [freqs.last().copied().unwrap_or(250.0) as f64, -60.0]];
            pu.line(Line::new("Ormsby", PlotPoints::from(trap)).color(with_alpha(p.pick, 200)).width(1.2).style(egui_plot::LineStyle::dashed_dense()));
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
            }
        }
    }
    if r.drag_stopped() {
        s.spectral.drag_marker = None;
    }
}

fn band_width(f: &[f32], db: &[f32], thresh: f32) -> Option<(f32, f32)> {
    let lo = f.iter().zip(db).find(|(_, d)| **d >= thresh).map(|(x, _)| *x)?;
    let hi = f.iter().zip(db).rev().find(|(_, d)| **d >= thresh).map(|(x, _)| *x)?;
    Some((lo, hi))
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
}

pub fn fk_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    widgets::toolbar(ui, &p, |ui| {
        ui.label(RichText::new(ensemble_title(s)).font(font_mono(11.0)).color(p.text_muted));
        vsep(ui, &p);
        let fk = &mut s.spectral.fk;
        ui.add(egui::DragValue::new(&mut fk.dx_m).range(0.5..=200.0).speed(0.1).prefix("dx ").suffix(" m"));
        egui::ComboBox::from_id_salt("fk_mode").selected_text(format!("{:?}", fk.mode)).width(150.0).show_ui(ui, |ui| {
            for m in [FkMode::RejectLowVelocity, FkMode::PassHighVelocity, FkMode::RejectPolygon, FkMode::PassPolygon] {
                ui.selectable_value(&mut fk.mode, m, format!("{m:?}"));
            }
        });
        let polygon_mode = matches!(fk.mode, FkMode::RejectPolygon | FkMode::PassPolygon);
        if !polygon_mode {
            ui.add(egui::DragValue::new(&mut fk.vel_cut_mps).range(100.0..=10000.0).speed(5.0).prefix("v ").suffix(" m/s"));
        } else if ui.small_button(format!("{} clear polygon", icon::TRASH)).clicked() {
            fk.polygon.clear();
        }
        ui.add(egui::DragValue::new(&mut fk.taper_frac).range(0.0..=1.0).speed(0.01).prefix("taper "));
        ui.checkbox(&mut s.spectral.fk_show_filtered, "Show filtered");
        vsep(ui, &p);
        if ui.button(format!("{}  Send to flow", icon::FLOW_ARROW)).clicked() {
            let v = serde_json::to_value(&s.spectral.fk).unwrap_or(json!({}));
            upsert_step(s, "fk_filt", |params| {
                if let (Some(dst), Some(src)) = (params.as_object_mut(), v.as_object()) {
                    for (k, val) in src {
                        dst.insert(k.clone(), val.clone());
                    }
                }
            });
        }
    });
    if s.session().is_some() {
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
                    Ok(FkResult { key, img, extent, gather, dt_ms })
                });
            }
        }
    }
    let Some(extent) = s.spectral.fk_extent else {
        if !super::busy_notice(s, ui) {
            canvas::empty_state(ui, &p, icon::POLYGON, "No F-K spectrum", "Select an ensemble with at least two traces");
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
            badge(&ui.painter_at(out.frame.rect), out.frame.rect.left_top() + vec2(6.0, 6.0), "F-K filtered gather (preview)", &p, egui::Align2::LEFT_TOP);
        });
    });
}

fn fk_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2, extent: RasterExtent) {
    let mut opts = CanvasOpts::seismic("k (cycles/m)");
    opts.y_down = false;
    opts.x_axis_top = false;
    opts.y_title = "Hz".into();
    opts.primary_pans = false;
    opts.background = p.canvas;
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
        badge(&painter, out.frame.rect.right_bottom() + vec2(-6.0, -6.0), &format!("k {k:.4} · f {f:.1} Hz · v_app {v:.0} m/s"), p, egui::Align2::RIGHT_BOTTOM);
    }
    badge(&painter, out.frame.rect.left_top() + vec2(6.0, 6.0), if s.spectral.fk_show_filtered { "F-K spectrum · filtered" } else { "F-K spectrum · input" }, p, egui::Align2::LEFT_TOP);
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
}

fn radon_img(panel: &RadonPanel) -> Result<egui::ColorImage, String> {
    let (nq, nt) = (panel.q_values_ms.len(), panel.times_ms.len());
    let img = rustseis_render::render_radon_panel(&panel.times_ms, &panel.q_values_ms, &panel.data, None, (nq + 64) as u32, (nt + 64) as u32).map_err(|e| e.to_string())?;
    Ok(crop_rgba(&img, 32, 32, nq, nt))
}

pub fn radon_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let names: Vec<String> = s.session().map(|x| {
        let mut v: Vec<String> = x.context.radon_panels.keys().cloned().collect();
        v.sort();
        v
    }).unwrap_or_default();
    let mut recompute = std::mem::take(&mut s.spectral.preview_requested);
    widgets::toolbar(ui, &p, |ui| {
        let label = s.spectral.radon_source.clone().map(|n| format!("panel · {n}")).unwrap_or_else(|| format!("compute · {}", ensemble_title(s)));
        egui::ComboBox::from_id_salt("radon_src").selected_text(RichText::new(label).size(12.0)).width(190.0).show_ui(ui, |ui| {
            if ui.selectable_label(s.spectral.radon_source.is_none(), "Compute from focus ensemble").clicked() {
                s.spectral.radon_source = None;
            }
            for n in &names {
                if ui.selectable_label(s.spectral.radon_source.as_ref() == Some(n), format!("FlowContext panel · {n}")).clicked() {
                    s.spectral.radon_source = Some(n.clone());
                }
            }
        });
        let r = &mut s.spectral.radon;
        segmented(ui, &p, &mut r.mode, &[(RadonMode::Parabolic, "Parabolic", "t = τ + q (x/x_ref)²"), (RadonMode::Linear, "Linear", "τ-p")]);
        ui.add(egui::DragValue::new(&mut r.q_min_ms).range(-2000.0..=r.q_max_ms - 1.0).prefix("q ").suffix(" ms"));
        ui.add(egui::DragValue::new(&mut r.q_max_ms).range(r.q_min_ms + 1.0..=4000.0).prefix("… ").suffix(" ms"));
        ui.add(egui::DragValue::new(&mut r.n_q).range(11..=401).prefix("nq "));
        ui.add(egui::DragValue::new(&mut r.sparse_iters).range(0..=6).prefix("IRLS "));
        vsep(ui, &p);
        segmented(ui, &p, &mut r.action, &[(RadonFilterAction::SubtractZone, "Subtract zone", "Model multiples in zone and subtract"), (RadonFilterAction::PassZone, "Pass zone", "Keep only the zone")]);
        if primary_button(ui, &p, icon::PLAY, "Preview").on_hover_text("apply_radon_filter_gather on the focus ensemble").clicked() {
            recompute = true;
        }
        if ui.button(format!("{}  Send to flow", icon::FLOW_ARROW)).clicked() {
            let v = serde_json::to_value(&s.spectral.radon).unwrap_or(json!({}));
            upsert_step(s, "radon_filter", |params| params["params"] = v);
        }
    });

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
        } else if let Some((ens, ekey)) = focus_ensemble(s, 400) {
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
                    Ok(RadonPreview { key, imgs: [r(&ens)?, r(&multiples)?, r(&out)?] })
                });
            }
        }
    }

    let Some(panel) = s.spectral.radon_panel.clone() else {
        if !super::busy_notice(s, ui) {
            canvas::empty_state(ui, &p, icon::FUNNEL, "No Radon panel", "Select a CDP ensemble (≥ 3 traces) or a FlowContext panel from radon_filter / radon_trans");
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
            for (i, title) in ["Before", "Modeled multiples", "Demultipled primaries"].iter().enumerate() {
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
                    badge(&ui.painter_at(out.frame.rect), out.frame.rect.left_top() + vec2(6.0, 6.0), title, &p, egui::Align2::LEFT_TOP);
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
    let mut opts = CanvasOpts::seismic("q (ms)");
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
    painter.rect_filled(shade, 0.0, with_alpha(p.pick, 26));
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
    badge(&painter, out.frame.rect.left_top() + vec2(6.0, 6.0), &format!("Radon {:?} · {} q · ref {:.0} m", panel.mode, panel.q_values_ms.len(), panel.ref_offset_m), p, egui::Align2::LEFT_TOP);
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
        }
    }
    if resp.drag_stopped() {
        s.spectral.radon_drag = None;
    }
    if let Some((q, t)) = out.hover {
        badge(&painter, out.frame.rect.right_bottom() + vec2(-6.0, -6.0), &format!("τ {t:.0} ms · q {q:.1} ms"), p, egui::Align2::RIGHT_BOTTOM);
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
