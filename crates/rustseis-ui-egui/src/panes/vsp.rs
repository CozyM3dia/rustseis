//! Borehole view: receiver depth track, total and separated wavefields, corridor stack and an
//! interval-velocity log, all sharing one depth cursor.

use crate::canvas::{self, CanvasOpts, CanvasState, Raster, View2D, badge, clip_amplitude, draw_raster, draw_wiggles};
use crate::data::{self, RasterExtent, TraceSource, render_gather_raster};
use crate::state::AppState;
use crate::theme::{Palette, palette, with_alpha};
use crate::widgets::{self, caps, cmd_primary, segmented};
use egui::{Color32, RichText, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use egui_plot::{Line, Plot, PlotPoints};
use rustseis_core::Trace;
use rustseis_imaging::{VspCorridorStackParams, VspSepMethod, VspSlownessParams, VspSlownessProfile, VspWavefieldSepParams};
use rustseis_render::SeismicColormap;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SepView {
    Down,
    #[default]
    Up,
    UpTwoWay,
}

pub struct VspState {
    pub source: TraceSource,
    pub sep: VspWavefieldSepParams,
    pub corridor: VspCorridorStackParams,
    pub profile_name: Option<String>,
    pub sep_view: SepView,
    pub colormap: SeismicColormap,
    pub rerun: bool,
    key: u64,
    total: Option<Raster>,
    seps: [Option<Raster>; 3],
    corridor_traces: Vec<Trace>,
    corridor_clip: f32,
    depths: Vec<f32>,
    fb: Vec<f32>,
    profile: Option<VspSlownessProfile>,
    error: Option<String>,
    canvas: [CanvasState; 3],
    t_view: Option<(f64, f64)>,
}

impl Default for VspState {
    fn default() -> Self {
        Self {
            source: TraceSource::Active,
            sep: VspWavefieldSepParams { method: VspSepMethod::FkDip, fk_taper_frac: 0.05, ..Default::default() },
            corridor: VspCorridorStackParams::default(),
            profile_name: None,
            sep_view: SepView::Up,
            colormap: SeismicColormap::Greyscale,
            rerun: false,
            key: 0,
            total: None,
            seps: [None, None, None],
            corridor_traces: Vec::new(),
            corridor_clip: 1.0,
            depths: Vec::new(),
            fb: Vec::new(),
            profile: None,
            error: None,
            canvas: Default::default(),
            t_view: None,
        }
    }
}

struct VspResult {
    key: u64,
    total: egui::ColorImage,
    seps: [Option<egui::ColorImage>; 3],
    corridor: Vec<Trace>,
    depths: Vec<f32>,
    fb: Vec<f32>,
    profile: Option<VspSlownessProfile>,
    dt: f64,
    ns: usize,
    error: Option<String>,
}

fn hash_of(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let force = std::mem::take(&mut s.vsp.rerun);
    submit(s, force);
    if s.vsp.total.is_none() {
        if !super::busy_notice(s, ui) {
            if s.compute.is_busy("vsp.compute") {
                canvas::empty_state(ui, &p, icon::HOURGLASS, "Separating wavefields…", "This runs in the background; keep working meanwhile.");
            } else {
                canvas::empty_state(ui, &p, icon::DROP, "No borehole data yet", "Load a borehole or fibre record whose traces carry receiver depths.");
            }
        }
        return;
    }
    let full = ui.available_rect_before_wrap().shrink2(vec2(8.0, 6.0));
    let gap = 6.0;
    let track_h = 46.0;
    let w = full.width() - 3.0 * gap;
    let ws = [w * 0.30, w * 0.30, w * 0.12, w * 0.28];
    let mut x = full.left();
    let mut rects = Vec::new();
    for c in ws {
        rects.push(egui::Rect::from_min_max(pos2(x, full.top() + track_h + gap), pos2(x + c, full.bottom())));
        x += c + gap;
    }
    let track = egui::Rect::from_min_max(full.min, pos2(rects[1].right(), full.top() + track_h));
    let mut child = |r: egui::Rect| ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
    for i in 0..3 {
        let r = rects[i];
        panel(s, &mut child(r), &p, i, r.size());
    }
    depth_track(s, &mut child(track), &p, track, rects[0], rects[1]);
    let r = rects[3];
    velocity_plot(s, &mut child(r), &p, r.size());
    if let Some(e) = s.vsp.error.clone() {
        let r = egui::Rect::from_center_size(pos2(full.center().x, full.bottom() - 24.0), vec2(520.0, 30.0));
        widgets::banner(&mut child(r), &p, p.pick, icon::WARNING, &e);
    }
    ui.allocate_rect(full, egui::Sense::hover());
}

/// Receiver positions along the well with the shared depth cursor.
fn depth_track(s: &mut AppState, ui: &mut Ui, p: &Palette, rect: egui::Rect, total: egui::Rect, sep: egui::Rect) {
    let painter = ui.painter_at(rect);
    painter.rect(rect, egui::CornerRadius::same(8), p.sunken, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    let n = s.vsp.depths.len();
    if n == 0 {
        return;
    }
    let (dmin, dmax) = s.vsp.depths.iter().fold((f32::MAX, f32::MIN), |(a, b), d| (a.min(*d), b.max(*d)));
    painter.text(rect.left_top() + vec2(12.0, 8.0), egui::Align2::LEFT_TOP, "Well", crate::theme::font_semibold(12.0), p.text);
    painter.text(rect.left_top() + vec2(12.0, 26.0), egui::Align2::LEFT_TOP, format!("{n} receivers · {dmin:.0}–{dmax:.0} m"), egui::FontId::proportional(10.5), p.text_muted);
    // Casing line under each wavefield panel, receivers as ticks.
    for pr in [total, sep] {
        let y = rect.center().y;
        let x0 = (pr.left() + 48.0).max(rect.left() + 180.0).min(pr.right() - 20.0);
        let x1 = pr.right() - 8.0;
        if x1 <= x0 {
            continue;
        }
        painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(4.0, with_alpha(p.text_muted, 70)));
        let step = (n / ((x1 - x0) as usize / 3).max(1)).max(1);
        for i in (0..n).step_by(step) {
            let x = x0 + (i as f32 + 0.5) / n as f32 * (x1 - x0);
            painter.line_segment([pos2(x, y - 5.0), pos2(x, y + 5.0)], Stroke::new(1.0, with_alpha(p.info, 160)));
        }
        if let Some(d) = s.crosshair.depth_m.filter(|d| *d >= dmin && *d <= dmax) {
            if let Some(i) = s.vsp.depths.iter().position(|x| *x >= d) {
                let x = x0 + (i as f32 + 0.5) / n as f32 * (x1 - x0);
                painter.circle_filled(pos2(x, y), 5.0, p.accent);
                badge(&painter, pos2(x, rect.top() + 4.0), &format!("{d:.0} m"), p, egui::Align2::CENTER_TOP);
            }
        }
    }
}

/// Inspector for the borehole view.
pub fn inspector_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let sources = s.session().map(TraceSource::all).unwrap_or_default();
    let profiles: Vec<String> = s
        .session()
        .map(|x| {
            let mut v: Vec<String> = x.context.vsp_profiles.keys().cloned().collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new("Borehole").font(crate::theme::font_display(18.0)).color(p.text));
            ui.label(RichText::new("Splits the record into waves travelling down from the surface and waves reflected back up.").size(11.5).color(p.text_muted));
            widgets::row(ui, &p, "Data", |ui| {
                egui::ComboBox::from_id_salt("vsp_src").selected_text(s.vsp.source.label()).width(150.0).show_ui(ui, |ui| {
                    for src in &sources {
                        ui.selectable_value(&mut s.vsp.source, src.clone(), src.label());
                    }
                });
            });
            caps(ui, &p, "SEPARATION", None);
            widgets::row(ui, &p, "Method", |ui| {
                egui::ComboBox::from_id_salt("vsp_method").selected_text(if s.vsp.sep.method == VspSepMethod::FkDip { "By dip direction" } else { "Median after aligning" }).width(160.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.vsp.sep.method, VspSepMethod::FkDip, "By dip direction");
                    ui.selectable_value(&mut s.vsp.sep.method, VspSepMethod::MedianAlign, "Median after aligning");
                });
            });
            match s.vsp.sep.method {
                VspSepMethod::MedianAlign => widgets::row(ui, &p, "Median length", |ui| {
                    ui.add(egui::DragValue::new(&mut s.vsp.sep.median_len).range(3..=51).suffix(" traces"));
                }),
                VspSepMethod::FkDip => widgets::row(ui, &p, "Taper", |ui| {
                    ui.add(egui::DragValue::new(&mut s.vsp.sep.fk_taper_frac).range(0.0..=0.5).speed(0.005));
                }),
            }
            caps(ui, &p, "CORRIDOR STACK", None);
            widgets::row(ui, &p, "Starts after first break", |ui| {
                ui.add(egui::DragValue::new(&mut s.vsp.corridor.corridor_start_ms).range(0.0..=500.0).suffix(" ms"));
            });
            widgets::row(ui, &p, "Width", |ui| {
                ui.add(egui::DragValue::new(&mut s.vsp.corridor.corridor_width_ms).range(10.0..=1000.0).suffix(" ms"));
            });
            caps(ui, &p, "DISPLAY", None);
            widgets::row(ui, &p, "Colours", |ui| {
                segmented(ui, &p, &mut s.vsp.colormap, &[(SeismicColormap::Greyscale, "Grey", ""), (SeismicColormap::BlueWhiteRed, "Red and blue", "")]);
            });
            if !profiles.is_empty() {
                widgets::row(ui, &p, "Velocity log from", |ui| {
                    egui::ComboBox::from_id_salt("vsp_prof").selected_text(s.vsp.profile_name.clone().unwrap_or_else(|| "this view".into())).width(150.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.vsp.profile_name, None, "this view");
                        for n in &profiles {
                            ui.selectable_value(&mut s.vsp.profile_name, Some(n.clone()), n);
                        }
                    });
                });
            }
            ui.add_space(4.0);
            if cmd_primary(ui, &p, icon::ARROW_CLOCKWISE, "Separate again").clicked() {
                s.vsp.rerun = true;
            }
        });
    });
}

fn submit(s: &mut AppState, force: bool) {
    let key = hash_of((s.rev, &s.vsp.source, serde_json::to_string(&s.vsp.sep).unwrap_or_default(), serde_json::to_string(&s.vsp.corridor).unwrap_or_default(), s.vsp.colormap as u8));
    if key == s.vsp.key && !force {
        return;
    }
    let input = {
        let Some(sess) = s.session() else { return };
        let traces = s.vsp.source.traces(sess);
        if traces.len() < 4 {
            return;
        }
        data::sorted_by(data::decimated_clone(traces, 0, traces.len(), 2048), "REC_DEP")
    };
    s.vsp.key = key;
    let sep = s.vsp.sep.clone();
    let corr = s.vsp.corridor.clone();
    let cmap = s.vsp.colormap;
    s.compute.submit("vsp.compute", move || {
        let mut work = input;
        let (dt, ns) = data::time_axis_ms(&work);
        let mut errors = Vec::new();
        let profile = match rustseis_imaging::estimate_vsp_slowness(&mut work, &VspSlownessParams::default()) {
            Ok(p) => Some(p),
            Err(e) => {
                errors.push(format!("slowness: {e}"));
                None
            }
        };
        let depths: Vec<f32> = work.iter().map(|t| t.headers.rec_dep).collect();
        let fb: Vec<f32> = work.iter().map(|t| t.headers.fb_pick).collect();
        let total = render_gather_raster(&work, cmap, 98.0, data::MAX_RASTER_SIDE)?;
        let mut seps: [Option<egui::ColorImage>; 3] = [None, None, None];
        let mut corridor = Vec::new();
        match rustseis_imaging::separate_vsp_wavefields(&work, &sep) {
            Ok((down, up)) => {
                seps[0] = render_gather_raster(&down, cmap, 98.0, data::MAX_RASTER_SIDE).ok();
                seps[1] = render_gather_raster(&up, cmap, 98.0, data::MAX_RASTER_SIDE).ok();
                let mut two = up.clone();
                for tr in &mut two {
                    if tr.headers.fb_pick > 0.0 {
                        let shift = tr.headers.fb_pick;
                        rustseis_dsp::shift_trace_ms(tr, shift);
                    }
                }
                seps[2] = render_gather_raster(&two, cmap, 98.0, data::MAX_RASTER_SIDE).ok();
                match rustseis_imaging::compute_vsp_corridor_stack(&up, &corr) {
                    Ok(c) => corridor = c,
                    Err(e) => errors.push(format!("corridor: {e}")),
                }
            }
            Err(e) => errors.push(format!("separation: {e}")),
        }
        Ok(VspResult { key, total, seps, corridor, depths, fb, profile, dt: dt as f64, ns, error: (!errors.is_empty()).then(|| errors.join(" · ")) })
    });
}

pub fn on_compute(s: &mut AppState, ctx: &egui::Context, key: &str, value: Box<dyn std::any::Any + Send>) {
    if key != "vsp.compute" {
        return;
    }
    let Ok(r) = value.downcast::<VspResult>() else { return };
    if r.key != s.vsp.key {
        return;
    }
    let r = *r;
    let n = r.depths.len() as f64;
    let ext = RasterExtent { x0: 0.0, x1: n, y0: -0.5 * r.dt, y1: (r.ns as f64 - 0.5) * r.dt };
    let put = |slot: &mut Option<Raster>, name: &str, img: egui::ColorImage, ext: RasterExtent| match slot {
        Some(x) => x.replace(img, ext),
        None => *slot = Some(Raster::new(ctx, name, img, ext)),
    };
    let first = s.vsp.total.is_none();
    put(&mut s.vsp.total, "vsp_total", r.total, ext);
    for (i, img) in r.seps.into_iter().enumerate() {
        match img {
            Some(img) => put(&mut s.vsp.seps[i], &format!("vsp_sep_{i}"), img, ext),
            None => s.vsp.seps[i] = None,
        }
    }
    s.vsp.corridor_clip = clip_amplitude(&r.corridor, 99.0);
    let nc = r.corridor.len() as f64;
    s.vsp.corridor_traces = r.corridor;
    s.vsp.depths = r.depths;
    s.vsp.fb = r.fb;
    s.vsp.profile = r.profile;
    s.vsp.error = r.error;
    s.vsp.canvas[0].set_home(View2D::new(0.0, n, ext.y0, ext.y1), first);
    s.vsp.canvas[1].set_home(View2D::new(0.0, n, ext.y0, ext.y1), first);
    s.vsp.canvas[2].set_home(View2D::new(0.0, nc.max(1.0), ext.y0, ext.y1), first);
    if first {
        s.vsp.t_view = Some((ext.y0, ext.y1));
    }
}

fn panel(s: &mut AppState, ui: &mut Ui, p: &Palette, which: usize, size: egui::Vec2) {
    if let (Some(v), Some((a, b))) = (s.vsp.canvas[which].view.as_mut(), s.vsp.t_view) {
        v.y0 = a;
        v.y1 = b;
    }
    let depths = s.vsp.depths.clone();
    let mut opts = CanvasOpts::seismic(if which == 2 { "stack" } else { "depth m" });
    opts.background = p.canvas;
    if which == 2 {
        opts.x_label = Box::new(|_| None);
        opts.left_gutter = 10.0;
        opts.y_label = Box::new(|_| None);
        opts.y_title = String::new();
    } else {
        opts.x_label = Box::new(move |x| depths.get(x.floor().max(0.0) as usize).map(|d| format!("{d:.0}")));
        if which == 1 {
            opts.left_gutter = 10.0;
            opts.y_label = Box::new(|_| None);
            opts.y_title = String::new();
        }
    }
    let raster = match which {
        0 => s.vsp.total.as_ref(),
        1 => s.vsp.seps[match s.vsp.sep_view {
            SepView::Down => 0,
            SepView::Up => 1,
            SepView::UpTwoWay => 2,
        }]
        .as_ref(),
        _ => None,
    };
    let out = canvas::canvas(ui, egui::Id::unique(("vsp", which)), &mut s.vsp.canvas[which], p, size, opts, |painter, frame| {
        if let Some(r) = raster {
            draw_raster(painter, frame, r, Color32::WHITE);
        }
    });
    if out.changed_view {
        s.vsp.t_view = Some((out.frame.view.y0, out.frame.view.y1));
        let (x0, x1) = (out.frame.view.x0, out.frame.view.x1);
        if which < 2 {
            if let Some(v) = s.vsp.canvas[1 - which].view.as_mut() {
                v.x0 = x0;
                v.x1 = x1;
            }
        }
    }
    let painter = ui.painter_at(out.frame.rect);
    match which {
        0 => {
            let pts: Vec<egui::Pos2> = s.vsp.fb.iter().enumerate().filter(|(_, t)| **t > 0.0).map(|(i, t)| out.frame.to_screen(i as f64 + 0.5, *t as f64)).collect();
            if pts.len() > 1 {
                painter.add(egui::Shape::line(pts, Stroke::new(1.4, p.pick)));
            }
            badge(&painter, out.frame.rect.center_top() + vec2(0.0, 8.0), "Total", p, egui::Align2::CENTER_TOP);
        }
        1 => {
            // Glass tabs for the separated wavefields.
            let r = egui::Rect::from_min_size(out.frame.rect.center_top() + vec2(-130.0, 6.0), vec2(260.0, 34.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
            widgets::glass_frame(p).inner_margin(egui::Margin::same(3)).show(&mut child, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (v, label) in [(SepView::Down, "Down"), (SepView::Up, "Up"), (SepView::UpTwoWay, "Up, two-way")] {
                    if widgets::cmd(ui, p, "", label, s.vsp.sep_view == v).clicked() {
                        s.vsp.sep_view = v;
                    }
                }
            });
        }
        _ => {
            draw_wiggles(&painter, &out.frame, &s.vsp.corridor_traces, 0.0, s.vsp.corridor_clip, 1.6, p.text, Some(with_alpha(p.text, 160)));
            badge(&painter, out.frame.rect.center_top() + vec2(0.0, 8.0), "Corridor", p, egui::Align2::CENTER_TOP);
        }
    }
    // Depth crosshair
    if which < 2 {
        if let Some((x, t)) = out.hover {
            let i = x.floor().max(0.0) as usize;
            if let Some(d) = s.vsp.depths.get(i) {
                s.crosshair.depth_m = Some(*d);
                s.crosshair.time_ms = Some(t as f32);
            }
        }
        if let Some(d) = s.crosshair.depth_m {
            if let Some(i) = s.vsp.depths.iter().position(|x| *x >= d) {
                let xs = out.frame.x_to_screen(i as f64 + 0.5);
                painter.vline(xs, out.frame.rect.y_range(), Stroke::new(1.0, with_alpha(p.accent, 190)));
            }
        }
    }
}

fn velocity_plot(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2) {
    let prof = match &s.vsp.profile_name {
        Some(n) => s.session().and_then(|x| x.context.vsp_profiles.get(n)).cloned(),
        None => s.vsp.profile.clone(),
    };
    ui.allocate_ui(size, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Velocity log").font(crate::theme::font_semibold(12.5)).color(p.text));
            if let Some(pr) = &prof {
                let finite: Vec<f32> = pr.interval_vel_mps.iter().copied().filter(|v| v.is_finite() && *v > 0.0).collect();
                if !finite.is_empty() {
                    let mean = finite.iter().sum::<f32>() / finite.len() as f32;
                    ui.label(RichText::new(format!("average {mean:.0} m/s")).size(11.0).color(p.text_muted));
                }
            }
        });
        let Some(pr) = prof else {
            ui.label(RichText::new("No velocity log yet").color(p.text_faint));
            return;
        };
        let vint: Vec<[f64; 2]> = pr.depths_m.iter().zip(&pr.interval_vel_mps).filter(|(_, v)| v.is_finite() && **v > 0.0).map(|(d, v)| [*v as f64, *d as f64]).collect();
        let fbt: Vec<[f64; 2]> = pr.depths_m.iter().zip(&pr.fb_times_ms).filter(|(_, t)| **t > 0.0).map(|(d, t)| [*t as f64, *d as f64]).collect();
        let depth = s.crosshair.depth_m;
        Plot::new("vsp_vel")
            .height(size.y * 0.62)
            .invert_y(true)
            .x_axis_label("m/s")
            .y_axis_label("depth (m)")
            .show_background(false)
            .show(ui, |pu| {
                pu.line(Line::new("interval velocity", PlotPoints::from(vint)).color(p.accent).width(1.8));
                if let Some(d) = depth {
                    pu.hline(egui_plot::HLine::new("crosshair", d as f64).color(with_alpha(p.accent, 120)));
                }
            });
        ui.label(RichText::new("First arrival time").size(11.0).color(p.text_muted));
        Plot::new("vsp_fb")
            .invert_y(true)
            .x_axis_label("ms")
            .y_axis_label("depth (m)")
            .show_background(false)
            .show(ui, |pu| {
                pu.points(egui_plot::Points::new("first arrival", PlotPoints::from(fbt)).color(p.pick).radius(1.6));
                if let Some(d) = depth {
                    pu.hline(egui_plot::HLine::new("crosshair", d as f64).color(with_alpha(p.accent, 120)));
                }
            });
    });
}
