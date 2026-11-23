//! Workspace 5 · VSP / borehole DAS: total | separated | corridor stack | velocity vs depth.

use crate::canvas::{self, CanvasOpts, CanvasState, Raster, View2D, badge, clip_amplitude, draw_raster, draw_wiggles};
use crate::data::{self, RasterExtent, TraceSource, render_gather_raster};
use crate::state::AppState;
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets::{self, chip, primary_button, segmented, vsep};
use egui::{Color32, RichText, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;
use egui_plot::{Legend, Line, Plot, PlotPoints};
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
    let sources = s.session().map(TraceSource::all).unwrap_or_default();
    let profiles: Vec<String> = s.session().map(|x| {
        let mut v: Vec<String> = x.context.vsp_profiles.keys().cloned().collect();
        v.sort();
        v
    }).unwrap_or_default();
    let mut force = false;
    widgets::toolbar(ui, &p, |ui| {
        egui::ComboBox::from_id_salt("vsp_src").selected_text(RichText::new(s.vsp.source.label()).size(12.0)).width(140.0).show_ui(ui, |ui| {
            for src in &sources {
                ui.selectable_value(&mut s.vsp.source, src.clone(), src.label());
            }
        });
        vsep(ui, &p);
        segmented(ui, &p, &mut s.vsp.sep.method, &[(VspSepMethod::MedianAlign, "Median", "FB-aligned spatial median"), (VspSepMethod::FkDip, "F-K dip", "Dip-sign separation in F-K")]);
        match s.vsp.sep.method {
            VspSepMethod::MedianAlign => {
                ui.add(egui::DragValue::new(&mut s.vsp.sep.median_len).range(3..=51).prefix("len "));
            }
            VspSepMethod::FkDip => {
                ui.add(egui::DragValue::new(&mut s.vsp.sep.fk_taper_frac).range(0.0..=0.5).speed(0.005).prefix("taper "));
            }
        }
        vsep(ui, &p);
        ui.label(RichText::new("corridor").size(11.5));
        ui.add(egui::DragValue::new(&mut s.vsp.corridor.corridor_start_ms).range(0.0..=500.0).suffix(" ms"));
        ui.add(egui::DragValue::new(&mut s.vsp.corridor.corridor_width_ms).range(10.0..=1000.0).prefix("w ").suffix(" ms"));
        if primary_button(ui, &p, icon::PLAY, "Separate").on_hover_text("vsp_slowness + wavefield separation + corridor stack on a copy").clicked() {
            force = true;
        }
        vsep(ui, &p);
        segmented(ui, &p, &mut s.vsp.colormap, &[(SeismicColormap::Greyscale, "Grey", ""), (SeismicColormap::BlueWhiteRed, "BWR", "")]);
        if !profiles.is_empty() {
            egui::ComboBox::from_id_salt("vsp_prof").selected_text(s.vsp.profile_name.clone().map(|n| format!("profile · {n}")).unwrap_or_else(|| "profile · computed".into())).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.vsp.profile_name, None, "computed here");
                for n in &profiles {
                    ui.selectable_value(&mut s.vsp.profile_name, Some(n.clone()), n);
                }
            });
        }
    });

    submit(s, force);

    if s.vsp.total.is_none() {
        if !super::busy_notice(s, ui) {
            if s.compute.is_busy("vsp.compute") {
                canvas::empty_state(ui, &p, icon::HOURGLASS, "Separating wavefields…", "Running on the compute pool");
            } else {
                canvas::empty_state(ui, &p, icon::DROP, "No borehole data", "Run a VSP / DAS flow (e.g. otway_das_vsp_demo) — traces should carry REC_DEP");
            }
        }
        return;
    }
    if let Some(e) = &s.vsp.error {
        widgets::banner(ui, &p, p.pick, icon::WARNING, e);
    }
    let avail = ui.available_size();
    let w_total = avail.x * 0.32;
    let w_sep = avail.x * 0.32;
    let w_cor = avail.x * 0.12;
    let w_plot = avail.x - w_total - w_sep - w_cor - 20.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.vertical(|ui| {
            ui.set_width(w_total);
            panel(s, ui, &p, 0, vec2(w_total, avail.y));
        });
        ui.vertical(|ui| {
            ui.set_width(w_sep);
            panel(s, ui, &p, 1, vec2(w_sep, avail.y));
        });
        ui.vertical(|ui| {
            ui.set_width(w_cor);
            panel(s, ui, &p, 2, vec2(w_cor, avail.y));
        });
        ui.vertical(|ui| {
            ui.set_width(w_plot);
            velocity_plot(s, ui, &p, vec2(w_plot, avail.y));
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
            badge(&painter, out.frame.rect.left_top() + vec2(6.0, 6.0), &format!("Total wavefield · {} ch", s.vsp.depths.len()), p, egui::Align2::LEFT_TOP);
        }
        1 => {
            let label = match s.vsp.sep_view {
                SepView::Down => "Downgoing",
                SepView::Up => "Upgoing",
                SepView::UpTwoWay => "Upgoing · two-way aligned",
            };
            badge(&painter, out.frame.rect.left_top() + vec2(6.0, 6.0), label, p, egui::Align2::LEFT_TOP);
            // Inline view switcher in the panel's corner.
            let r = egui::Rect::from_min_size(out.frame.rect.right_top() + vec2(-206.0, 4.0), vec2(200.0, 22.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
            segmented(&mut child, p, &mut s.vsp.sep_view, &[(SepView::Down, "Down", "Downgoing"), (SepView::Up, "Up", "Upgoing"), (SepView::UpTwoWay, "Up 2-way", "Upgoing aligned in two-way time")]);
        }
        _ => {
            draw_wiggles(&painter, &out.frame, &s.vsp.corridor_traces, 0.0, s.vsp.corridor_clip, 1.6, p.text, Some(with_alpha(p.text, 160)));
            badge(&painter, out.frame.rect.left_top() + vec2(4.0, 6.0), "Corridor", p, egui::Align2::LEFT_TOP);
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
            ui.label(RichText::new("Interval velocity").font(crate::theme::font_display(12.0)));
            if let Some(pr) = &prof {
                let finite: Vec<f32> = pr.interval_vel_mps.iter().copied().filter(|v| v.is_finite() && *v > 0.0).collect();
                if !finite.is_empty() {
                    let mean = finite.iter().sum::<f32>() / finite.len() as f32;
                    chip(ui, &format!("mean {mean:.0} m/s"), p.accent, with_alpha(p.accent, 22));
                }
            }
        });
        let Some(pr) = prof else {
            ui.label(RichText::new("No slowness profile").color(p.text_faint));
            return;
        };
        let vint: Vec<[f64; 2]> = pr.depths_m.iter().zip(&pr.interval_vel_mps).filter(|(_, v)| v.is_finite() && **v > 0.0).map(|(d, v)| [*v as f64, *d as f64]).collect();
        let fbt: Vec<[f64; 2]> = pr.depths_m.iter().zip(&pr.fb_times_ms).map(|(d, t)| [*t as f64, *d as f64]).collect();
        let depth = s.crosshair.depth_m;
        Plot::new("vsp_vel")
            .height(size.y * 0.62)
            .invert_y(true)
            .x_axis_label("m/s")
            .y_axis_label("depth (m)")
            .show_background(false)
            .legend(Legend::default())
            .show(ui, |pu| {
                pu.line(Line::new("V interval", PlotPoints::from(vint)).color(p.accent).width(1.6));
                if let Some(d) = depth {
                    pu.hline(egui_plot::HLine::new("crosshair", d as f64).color(with_alpha(p.accent, 120)));
                }
            });
        ui.label(RichText::new("First-break time").font(font_mono(10.5)).color(p.text_faint));
        Plot::new("vsp_fb")
            .invert_y(true)
            .x_axis_label("ms")
            .y_axis_label("depth (m)")
            .show_background(false)
            .show(ui, |pu| {
                pu.line(Line::new("FB", PlotPoints::from(fbt)).color(p.pick).width(1.4));
                if let Some(d) = depth {
                    pu.hline(egui_plot::HLine::new("crosshair", d as f64).color(with_alpha(p.accent, 120)));
                }
            });
    });
}
