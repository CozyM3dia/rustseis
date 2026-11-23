//! Workspace 3: Interactive Velocity Analysis (semblance | live NMO / PSTM CIG | CVS + Dix QC).

use crate::canvas::{self, CanvasOpts, CanvasState, Raster, View2D, badge, clip_amplitude, draw_raster, draw_wiggles};
use crate::data::{self, EnsembleIndex, RasterExtent, TraceSource, crop_rgba, fmt_count, render_gather_raster, sorted_by};
use crate::state::{AppState, JobPurpose, Level};
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets::{self, chip, icon_button, primary_button, section, vsep};
use crate::worker::EngineJob;
use egui::{Color32, RichText, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;
use egui_plot::{Legend, Line, Plot, PlotPoints, Points};
use rustseis_core::{Ensemble, Trace, VelocityFunction, VelocityTable};
use rustseis_engine::EngineCommand;
use rustseis_imaging::{CvsPanel, NmoParams, SemblancePanel, VelocityScanParams};
use rustseis_render::SeismicColormap;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

pub struct IvaState {
    pub source: TraceSource,
    pub table_name: String,
    pub functions: BTreeMap<i32, Vec<(f32, f32)>>,
    pub pos: usize,
    pub super_half: usize,
    pub scan: VelocityScanParams,
    pub nmo_stretch: f32,
    pub apply_nmo: bool,
    pub cvs_count: usize,
    pub snap: bool,
    pending_load: Option<String>,
    index: EnsembleIndex,
    canvas_sem: CanvasState,
    canvas_gather: CanvasState,
    canvas_cvs: CanvasState,
    t_view: Option<(f64, f64)>,
    gather: Vec<Trace>,
    gather_cdp: i32,
    gather_key: u64,
    sem: Option<SemblancePanel>,
    sem_key: u64,
    sem_raster: Option<Raster>,
    nmo_key: u64,
    nmo_raster: Option<Raster>,
    nmo_traces: Vec<Trace>,
    nmo_clip: f32,
    cvs: Option<CvsPanel>,
    cvs_raster: Option<Raster>,
    cvs_clip: f32,
    drag: Option<usize>,
}

impl Default for IvaState {
    fn default() -> Self {
        Self {
            source: TraceSource::Active,
            table_name: "ui_velocity".into(),
            functions: BTreeMap::new(),
            pos: 0,
            super_half: 1,
            scan: VelocityScanParams { v_min_mps: 1400.0, v_max_mps: 4500.0, dv_mps: 25.0, window_ms: 24.0, t_step_ms: 8.0, stretch_mute_pct: 50.0 },
            nmo_stretch: 40.0,
            apply_nmo: true,
            cvs_count: 21,
            snap: true,
            pending_load: None,
            index: EnsembleIndex::default(),
            canvas_sem: CanvasState::default(),
            canvas_gather: CanvasState::default(),
            canvas_cvs: CanvasState::default(),
            t_view: None,
            gather: Vec::new(),
            gather_cdp: 0,
            gather_key: 0,
            sem: None,
            sem_key: 0,
            sem_raster: None,
            nmo_key: 0,
            nmo_raster: None,
            nmo_traces: Vec::new(),
            nmo_clip: 1.0,
            cvs: None,
            cvs_raster: None,
            cvs_clip: 1.0,
            drag: None,
        }
    }
}

impl IvaState {
    pub fn select_table(&mut self, name: &str) {
        self.pending_load = Some(name.to_string());
    }
    fn current_picks(&self) -> Vec<(f32, f32)> {
        self.functions.get(&self.gather_cdp).cloned().unwrap_or_default()
    }
    fn function(&self) -> VelocityFunction {
        VelocityFunction::new(self.gather_cdp, self.current_picks())
    }
    pub fn table(&self) -> VelocityTable {
        let fns = self.functions.iter().filter(|(_, p)| !p.is_empty()).map(|(c, p)| VelocityFunction::new(*c, p.clone())).collect();
        VelocityTable::new(self.table_name.clone(), fns)
    }
}

fn hash_of(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

fn scan_hash(s: &VelocityScanParams) -> u64 {
    hash_of([s.v_min_mps, s.v_max_mps, s.dv_mps, s.window_ms, s.t_step_ms, s.stretch_mute_pct].map(f32::to_bits))
}

fn picks_hash(p: &[(f32, f32)]) -> u64 {
    hash_of(p.iter().map(|(a, b)| (a.to_bits(), b.to_bits())).collect::<Vec<_>>())
}

struct SemResult {
    panel: SemblancePanel,
    img: egui::ColorImage,
    key: u64,
}
struct CvsResult {
    panel: CvsPanel,
    img: egui::ColorImage,
    key: u64,
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    load_pending(s);
    toolbar(s, ui, &p);
    if s.is_busy() && s.iva.sem_raster.is_none() {
        super::busy_notice(s, ui);
        return;
    }
    if s.session().is_some() {
        prepare(s, ui.ctx());
    }
    if s.iva.gather.is_empty() {
        canvas::empty_state(ui, &p, icon::CHART_LINE_UP, "No CDP gathers", "Load data sorted by CDP (inline_sort CDP, AOFFSET) or pick a buffer holding prestack CDPs");
        return;
    }
    if s.iva.index.ranges.len() > 1 && s.iva.gather.len() < 4 {
        widgets::banner(ui, &p, p.pick, icon::WARNING, "Very low fold — the source may not be CDP-sorted. Use \"Sort by CDP\" in the toolbar.");
    }

    let avail = ui.available_size();
    let col_w = [avail.x * 0.30, avail.x * 0.40, avail.x * 0.30 - 12.0];
    let h = avail.y;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.vertical(|ui| {
            ui.set_width(col_w[0]);
            semblance_canvas(s, ui, &p, vec2(col_w[0], h));
        });
        ui.vertical(|ui| {
            ui.set_width(col_w[1]);
            gather_canvas(s, ui, &p, vec2(col_w[1], h));
        });
        ui.vertical(|ui| {
            ui.set_width(col_w[2]);
            cvs_canvas(s, ui, &p, vec2(col_w[2], h * 0.58));
            dix_plot(s, ui, &p, vec2(col_w[2], h * 0.42 - 6.0));
        });
    });
}

fn load_pending(s: &mut AppState) {
    let Some(name) = s.iva.pending_load.clone() else { return };
    let Some(sess) = s.session() else { return };
    if let Some(vt) = sess.context.velocities.get(&name).cloned() {
        s.iva.functions = vt.functions.iter().map(|f| (f.cdp, f.picks.clone())).collect();
        s.iva.table_name = name.clone();
        s.toast(Level::Ok, format!("Velocity table {name} · {} functions", vt.functions.len()));
    }
    s.iva.pending_load = None;
}

fn toolbar(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let sources = s.session().map(TraceSource::all).unwrap_or_default();
    widgets::toolbar(ui, p, |ui| {
        egui::ComboBox::from_id_salt("iva_src").selected_text(RichText::new(s.iva.source.label()).size(12.0)).width(140.0).show_ui(ui, |ui| {
            for src in &sources {
                if ui.selectable_label(*src == s.iva.source, src.label()).clicked() {
                    s.iva.source = src.clone();
                    s.iva.pos = 0;
                }
            }
        });
        let n = s.iva.index.ranges.len();
        if icon_button(ui, p, icon::CARET_LEFT, "Previous CDP location", false).clicked() {
            s.iva.pos = s.iva.pos.saturating_sub(stride(s));
        }
        let mut cdp = s.iva.gather_cdp;
        if ui.add(egui::DragValue::new(&mut cdp).prefix("CDP ").speed(1.0)).changed() {
            if let Some(i) = s.iva.index.ranges.iter().position(|r| r.key_value >= cdp as i64) {
                s.iva.pos = i;
            }
        }
        if icon_button(ui, p, icon::CARET_RIGHT, "Next CDP location", false).clicked() {
            s.iva.pos = (s.iva.pos + stride(s)).min(n.saturating_sub(1));
        }
        ui.label(RichText::new(format!("{} of {} CDPs · fold {}", s.iva.pos + 1, fmt_count(n), s.iva.gather.len())).font(font_mono(11.0)).color(p.text_muted));
        vsep(ui, p);
        ui.label(RichText::new("super").size(11.5));
        let mut sg = s.iva.super_half * 2 + 1;
        egui::ComboBox::from_id_salt("iva_super").selected_text(format!("{sg} CDP")).width(66.0).show_ui(ui, |ui| {
            for v in [1usize, 3, 5, 7, 9, 11] {
                ui.selectable_value(&mut sg, v, format!("{v} CDP"));
            }
        });
        s.iva.super_half = sg / 2;
        ui.menu_button(format!("{}  Scan", icon::SLIDERS), |ui| {
            let sc = &mut s.iva.scan;
            egui::Grid::new("scan_grid").num_columns(2).show(ui, |ui| {
                ui.label("v min");
                ui.add(egui::DragValue::new(&mut sc.v_min_mps).range(300.0..=8000.0).speed(10.0).suffix(" m/s"));
                ui.end_row();
                ui.label("v max");
                ui.add(egui::DragValue::new(&mut sc.v_max_mps).range(500.0..=12000.0).speed(10.0).suffix(" m/s"));
                ui.end_row();
                ui.label("dv");
                ui.add(egui::DragValue::new(&mut sc.dv_mps).range(5.0..=500.0).speed(1.0).suffix(" m/s"));
                ui.end_row();
                ui.label("gate");
                ui.add(egui::DragValue::new(&mut sc.window_ms).range(4.0..=200.0).speed(1.0).suffix(" ms"));
                ui.end_row();
                ui.label("t step");
                ui.add(egui::DragValue::new(&mut sc.t_step_ms).range(2.0..=100.0).speed(1.0).suffix(" ms"));
                ui.end_row();
                ui.label("stretch");
                ui.add(egui::DragValue::new(&mut sc.stretch_mute_pct).range(0.0..=200.0).suffix(" %"));
                ui.end_row();
                ui.label("CVS strips");
                ui.add(egui::DragValue::new(&mut s.iva.cvs_count).range(5..=61));
                ui.end_row();
            });
        });
        vsep(ui, p);
        ui.checkbox(&mut s.iva.apply_nmo, "Live NMO").on_hover_text("Off: inspect PSTM common-image gathers for residual moveout");
        ui.add(egui::DragValue::new(&mut s.iva.nmo_stretch).range(0.0..=200.0).prefix("stretch ").suffix(" %"));
        ui.checkbox(&mut s.iva.snap, "Snap").on_hover_text("Snap new picks to the local semblance maximum");
        vsep(ui, p);
        if ui.button(format!("{}  Auto-pick", icon::SPARKLE)).on_hover_text("SemblancePanel::auto_pick (dynamic programming)").clicked() {
            if let Some(sem) = &s.iva.sem {
                let f = sem.auto_pick(1200.0, 0.05, 250.0);
                s.iva.functions.insert(s.iva.gather_cdp, f.picks);
            }
        }
        if ui.button(format!("{}  Clear", icon::TRASH)).clicked() {
            s.iva.functions.remove(&s.iva.gather_cdp);
        }
        if ui.button(format!("{}  Sort by CDP", icon::ARROWS_LEFT_RIGHT)).on_hover_text("Run inline_sort (CDP, AOFFSET) on the active traces — undoable").clicked() {
            let spec: rustseis_flow::FlowSpec = serde_json::from_value(serde_json::json!({
                "name": "ui_cdp_sort",
                "steps": [{"tool": "inline_sort", "params": {"primary": "CDP", "secondary": "AOFFSET"}}]
            }))
            .expect("static flow");
            s.iva.source = TraceSource::Active;
            s.run_flow(spec, JobPurpose::Flow { flow_name: "ui_cdp_sort".into() });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(!s.is_busy(), |ui| {
                if primary_button(ui, p, icon::FLOPPY_DISK, "Save velocity table").clicked() {
                    save_table(s, false);
                }
            });
        });
    });
}

fn stride(s: &AppState) -> usize {
    (s.iva.index.ranges.len() / 40).max(s.iva.super_half * 2 + 1).max(1)
}

fn save_table(s: &mut AppState, persist: bool) {
    let vt = s.iva.table();
    let n = vt.functions.len();
    if n == 0 {
        s.toast(Level::Warn, "No velocity picks to save");
        return;
    }
    match s.dispatch_now(EngineCommand::UpsertVelocityTable(vt)) {
        Ok(_) => s.toast(Level::Ok, format!("UpsertVelocityTable · {} · {n} functions", s.iva.table_name)),
        Err(e) => {
            s.toast(Level::Error, e);
            return;
        }
    }
    if persist {
        if let Some(db) = s.project.opf_path.clone() {
            s.submit(EngineJob::Command(EngineCommand::SaveOpfDatabase { db_path: db }), JobPurpose::Opf);
        } else {
            s.save_opf_dialog();
        }
    }
}

fn prepare(s: &mut AppState, ctx: &egui::Context) {
    if s.session().is_none() {
        return;
    }
    if !s.iva.index.matches(&s.iva.source, "CDP", s.rev) {
        let idx = {
            let sess = s.session().expect("checked");
            EnsembleIndex::build(s.iva.source.traces(sess), s.iva.source.clone(), "CDP", s.rev)
        };
        s.iva.index = idx;
        if s.iva.pos >= s.iva.index.ranges.len() {
            s.iva.pos = s.iva.index.ranges.len() / 2;
        }
        // Start in the middle of the line where fold is full.
        if s.iva.gather_key == 0 {
            s.iva.pos = s.iva.index.ranges.len() / 2;
        }
    }
    let n = s.iva.index.ranges.len();
    if n == 0 {
        s.iva.gather.clear();
        return;
    }
    let pos = s.iva.pos.min(n - 1);
    let k = s.iva.super_half;
    let gkey = hash_of((s.rev, &s.iva.source, pos, k));
    if gkey != s.iva.gather_key {
        let lo = s.iva.index.ranges[pos.saturating_sub(k)].start;
        let hi = s.iva.index.ranges[(pos + k).min(n - 1)].end;
        let gather = {
            let sess = s.session().expect("checked");
            sorted_by(s.iva.source.traces(sess)[lo..hi].to_vec(), "AOFFSET")
        };
        s.iva.gather = gather;
        s.iva.gather_cdp = s.iva.index.ranges[pos].key_value as i32;
        s.iva.gather_key = gkey;
        s.iva.nmo_key = 0;
        let (dt, ns) = data::time_axis_ms(&s.iva.gather);
        let tmax = (ns as f64 - 0.5) * dt as f64;
        let refit = s.iva.t_view.is_none();
        s.iva.canvas_gather.set_home(View2D::new(0.0, s.iva.gather.len() as f64, -0.5 * dt as f64, tmax), true);
        if refit {
            s.iva.t_view = Some((-0.5 * dt as f64, tmax));
        }
    }
    // Semblance + CVS in the background.
    let skey = hash_of((gkey, scan_hash(&s.iva.scan), s.iva.cvs_count));
    if skey != s.iva.sem_key {
        s.iva.sem_key = skey;
        let ens = Ensemble::new("CDP", s.iva.gather_cdp, s.iva.gather.clone());
        let scan = s.iva.scan.clone();
        s.compute.submit("iva.semblance", move || {
            let panel = rustseis_imaging::compute_semblance_panel(&ens, &scan).map_err(|e| e.to_string())?;
            let nv = panel.velocities_mps.len();
            let nt = panel.times_ms.len();
            let img = rustseis_render::render_semblance_panel(&panel.times_ms, &panel.velocities_mps, &panel.values, None, (nv + 64) as u32, (nt + 64) as u32)
                .map_err(|e| e.to_string())?;
            let img = crop_rgba(&img, 32, 32, nv, nt);
            Ok(SemResult { panel, img, key: skey })
        });
        let ens = Ensemble::new("CDP", s.iva.gather_cdp, s.iva.gather.clone());
        let sc = s.iva.scan.clone();
        let count = s.iva.cvs_count.max(3);
        s.compute.submit("iva.cvs", move || {
            let vels: Vec<f32> = (0..count).map(|i| sc.v_min_mps + (sc.v_max_mps - sc.v_min_mps) * i as f32 / (count - 1) as f32).collect();
            let panel = rustseis_imaging::compute_cvs_panel(&ens, &vels, sc.stretch_mute_pct).map_err(|e| e.to_string())?;
            let img = render_gather_raster(&panel.stacked_traces, SeismicColormap::Greyscale, 99.0, data::MAX_RASTER_SIDE)?;
            Ok(CvsResult { panel, img, key: skey })
        });
    }
    // Live NMO (inline: < few ms for typical supergathers).
    let picks = s.iva.current_picks();
    let nkey = hash_of((gkey, picks_hash(&picks), s.iva.nmo_stretch.to_bits(), s.iva.apply_nmo));
    if nkey != s.iva.nmo_key {
        s.iva.nmo_key = nkey;
        let mut tr = s.iva.gather.clone();
        if s.iva.apply_nmo && picks.len() >= 1 {
            let vt = VelocityTable::new("iva_live", vec![VelocityFunction::new(s.iva.gather_cdp, picks)]);
            let params = NmoParams { stretch_mute_pct: s.iva.nmo_stretch, ..nmo_defaults() };
            rustseis_imaging::apply_nmo(&mut tr, &vt, &params);
        }
        if let Ok(img) = render_gather_raster(&tr, SeismicColormap::BlueWhiteRed, 98.0, data::MAX_RASTER_SIDE) {
            let extent = super::viewer::gather_extent(&tr);
            match &mut s.iva.nmo_raster {
                Some(r) => r.replace(img, extent),
                None => s.iva.nmo_raster = Some(Raster::new(ctx, "iva_nmo", img, extent)),
            }
        }
        s.iva.nmo_clip = clip_amplitude(&tr, 98.0);
        s.iva.nmo_traces = tr;
    }
}

fn nmo_defaults() -> NmoParams {
    serde_json::from_value(serde_json::json!({})).expect("NmoParams has serde defaults")
}

pub fn on_compute(s: &mut AppState, ctx: &egui::Context, key: &str, value: Box<dyn std::any::Any + Send>) {
    match key {
        "iva.semblance" => {
            if let Ok(r) = value.downcast::<SemResult>() {
                if r.key != s.iva.sem_key {
                    return;
                }
                let pnl = &r.panel;
                let dv = pnl.velocities_mps.get(1).zip(pnl.velocities_mps.first()).map(|(a, b)| a - b).unwrap_or(50.0) as f64;
                let dtt = pnl.times_ms.get(1).zip(pnl.times_ms.first()).map(|(a, b)| a - b).unwrap_or(10.0) as f64;
                let extent = RasterExtent {
                    x0: *pnl.velocities_mps.first().unwrap_or(&0.0) as f64 - dv / 2.0,
                    x1: *pnl.velocities_mps.last().unwrap_or(&1.0) as f64 + dv / 2.0,
                    y0: *pnl.times_ms.first().unwrap_or(&0.0) as f64 - dtt / 2.0,
                    y1: *pnl.times_ms.last().unwrap_or(&1.0) as f64 + dtt / 2.0,
                };
                let r = *r;
                match &mut s.iva.sem_raster {
                    Some(x) => x.replace(r.img, extent),
                    None => s.iva.sem_raster = Some(Raster::new(ctx, "iva_sem", r.img, extent)),
                }
                let home_y = s.iva.t_view.unwrap_or((extent.y0, extent.y1));
                s.iva.canvas_sem.set_home(View2D::new(extent.x0, extent.x1, home_y.0, home_y.1), true);
                s.iva.sem = Some(r.panel);
            }
        }
        "iva.cvs" => {
            if let Ok(r) = value.downcast::<CvsResult>() {
                if r.key != s.iva.sem_key {
                    return;
                }
                let r = *r;
                let extent = super::viewer::gather_extent(&r.panel.stacked_traces);
                s.iva.cvs_clip = clip_amplitude(&r.panel.stacked_traces, 99.0);
                match &mut s.iva.cvs_raster {
                    Some(x) => x.replace(r.img, extent),
                    None => s.iva.cvs_raster = Some(Raster::new(ctx, "iva_cvs", r.img, extent)),
                }
                let home_y = s.iva.t_view.unwrap_or((extent.y0, extent.y1));
                s.iva.canvas_cvs.set_home(View2D::new(0.0, r.panel.stacked_traces.len() as f64, home_y.0, home_y.1), true);
                s.iva.cvs = Some(r.panel);
            }
        }
        _ => {}
    }
}

/// Keep the three panels on one TWT axis.
fn sync_in(state: &mut CanvasState, t: Option<(f64, f64)>) {
    if let (Some(v), Some((a, b))) = (state.view.as_mut(), t) {
        v.y0 = a;
        v.y1 = b;
    }
}
fn sync_out(out: &canvas::CanvasOutput, t: &mut Option<(f64, f64)>) {
    if out.changed_view {
        *t = Some((out.frame.view.y0, out.frame.view.y1));
    }
}

fn semblance_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2) {
    sync_in(&mut s.iva.canvas_sem, s.iva.t_view);
    let mut opts = CanvasOpts::seismic("m/s");
    opts.primary_pans = false;
    opts.background = p.canvas;
    opts.x_ticks = 4;
    opts.x_label = Box::new(|x| Some(format!("{x:.0}")));
    let raster = s.iva.sem_raster.as_ref();
    let out = canvas::canvas(ui, egui::Id::unique("iva_sem"), &mut s.iva.canvas_sem, p, size, opts, |painter, frame| {
        if let Some(r) = raster {
            draw_raster(painter, frame, r, Color32::WHITE);
        }
    });
    sync_out(&out, &mut s.iva.t_view);
    let painter = ui.painter_at(out.frame.rect);
    let picks = s.iva.current_picks();
    // Function curve and interval-velocity labels
    let pts: Vec<egui::Pos2> = picks.iter().map(|(t, v)| out.frame.to_screen(*v as f64, *t as f64)).collect();
    if pts.len() > 1 {
        painter.add(egui::Shape::line(pts.clone(), Stroke::new(3.5, Color32::from_black_alpha(160))));
        painter.add(egui::Shape::line(pts.clone(), Stroke::new(1.8, Color32::WHITE)));
    }
    for (i, c) in pts.iter().enumerate() {
        let active = s.iva.drag == Some(i);
        painter.circle(*c, if active { 5.5 } else { 4.5 }, Color32::from_black_alpha(140), Stroke::new(2.0, p.pick));
    }
    // Hover readout
    if let (Some((hv, ht)), Some(sem)) = (out.hover, &s.iva.sem) {
        if let Some(val) = semblance_at(sem, hv as f32, ht as f32) {
            badge(&painter, out.frame.rect.right_bottom() + vec2(-6.0, -6.0), &format!("t {ht:.0} ms · v {hv:.0} m/s · S {val:.2}"), p, egui::Align2::RIGHT_BOTTOM);
        }
        s.crosshair.time_ms = Some(ht as f32);
    }
    badge(&painter, out.frame.rect.left_top() + vec2(6.0, 6.0), &format!("Semblance · CDP {}", s.iva.gather_cdp), p, egui::Align2::LEFT_TOP);
    if s.compute.is_busy("iva.semblance") {
        badge(&painter, out.frame.rect.right_top() + vec2(-6.0, 6.0), "computing…", p, egui::Align2::RIGHT_TOP);
    }
    edit_picks(s, &out, &pts, PickSurface::Semblance);
}

fn semblance_at(sem: &SemblancePanel, v: f32, t: f32) -> Option<f32> {
    let nv = sem.velocities_mps.len();
    let nt = sem.times_ms.len();
    if nv < 2 || nt < 2 {
        return None;
    }
    let dv = sem.velocities_mps[1] - sem.velocities_mps[0];
    let dt = sem.times_ms[1] - sem.times_ms[0];
    let iv = ((v - sem.velocities_mps[0]) / dv).round();
    let it = ((t - sem.times_ms[0]) / dt).round();
    if iv < 0.0 || it < 0.0 || iv as usize >= nv || it as usize >= nt {
        return None;
    }
    Some(sem.get(it as usize, iv as usize))
}

fn snap_velocity(sem: &SemblancePanel, v: f32, t: f32) -> f32 {
    let nv = sem.velocities_mps.len();
    let nt = sem.times_ms.len();
    if nv < 3 || nt < 2 {
        return v;
    }
    let dv = sem.velocities_mps[1] - sem.velocities_mps[0];
    let dt = sem.times_ms[1] - sem.times_ms[0];
    let iv0 = ((v - sem.velocities_mps[0]) / dv).round() as isize;
    let it = (((t - sem.times_ms[0]) / dt).round() as isize).clamp(0, nt as isize - 1) as usize;
    let mut best = (iv0.clamp(0, nv as isize - 1) as usize, f32::MIN);
    for d in -4..=4 {
        let iv = iv0 + d;
        if iv >= 0 && (iv as usize) < nv {
            let val = sem.get(it, iv as usize);
            if val > best.1 {
                best = (iv as usize, val);
            }
        }
    }
    sem.velocities_mps[best.0]
}

#[derive(Clone, Copy, PartialEq)]
enum PickSurface {
    Semblance,
    Cvs,
}

fn edit_picks(s: &mut AppState, out: &canvas::CanvasOutput, handle_pts: &[egui::Pos2], surface: PickSurface) {
    let resp = &out.response;
    let Some(ptr) = resp.hover_pos() else {
        if !resp.dragged() {
            s.iva.drag = None;
        }
        return;
    };
    let (hx, ht) = out.frame.from_screen(ptr);
    let v_at = |s: &AppState| -> f32 {
        match surface {
            PickSurface::Semblance => hx as f32,
            PickSurface::Cvs => {
                let Some(c) = &s.iva.cvs else { return hx as f32 };
                let i = hx.floor().clamp(0.0, (c.velocities_mps.len().saturating_sub(1)) as f64) as usize;
                c.velocities_mps[i]
            }
        }
    };
    let hit = handle_pts.iter().enumerate().map(|(i, c)| (i, c.distance(ptr))).filter(|(_, d)| *d < 9.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i);
    let cdp = s.iva.gather_cdp;
    if resp.drag_started_by(egui::PointerButton::Primary) {
        s.iva.drag = hit;
    }
    if resp.dragged_by(egui::PointerButton::Primary) {
        if let Some(i) = s.iva.drag {
            let v = v_at(s);
            if let Some(list) = s.iva.functions.get_mut(&cdp) {
                if let Some(pk) = list.get_mut(i) {
                    *pk = (ht.max(0.0) as f32, v);
                }
            }
        }
    }
    if resp.drag_stopped() {
        s.iva.drag = None;
        if let Some(list) = s.iva.functions.get_mut(&cdp) {
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
    }
    if resp.clicked() && hit.is_none() {
        let mut v = v_at(s);
        if surface == PickSurface::Semblance && s.iva.snap {
            if let Some(sem) = &s.iva.sem {
                v = snap_velocity(sem, v, ht as f32);
            }
        }
        let list = s.iva.functions.entry(cdp).or_default();
        list.push((ht.max(0.0) as f32, v));
        list.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    if resp.secondary_clicked() {
        if let (Some(i), Some(list)) = (hit, s.iva.functions.get_mut(&cdp)) {
            if i < list.len() {
                list.remove(i);
            }
        }
    }
}

fn gather_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2) {
    sync_in(&mut s.iva.canvas_gather, s.iva.t_view);
    let offsets: Vec<f64> = s.iva.gather.iter().map(|t| t.headers.offset.abs() as f64).collect();
    let mut opts = CanvasOpts::seismic("offset m");
    opts.background = p.canvas;
    opts.x_label = Box::new(move |x| offsets.get(x.floor().max(0.0) as usize).map(|o| format!("{o:.0}")));
    let raster = s.iva.nmo_raster.as_ref();
    let out = canvas::canvas(ui, egui::Id::unique("iva_gather"), &mut s.iva.canvas_gather, p, size, opts, |painter, frame| {
        if let Some(r) = raster {
            draw_raster(painter, frame, r, Color32::WHITE);
        }
    });
    sync_out(&out, &mut s.iva.t_view);
    let painter = ui.painter_at(out.frame.rect);
    draw_wiggles(&painter, &out.frame, &s.iva.nmo_traces, 0.0, s.iva.nmo_clip, 1.0, Color32::from_rgba_unmultiplied(10, 12, 18, 220), Some(Color32::from_rgba_unmultiplied(10, 12, 18, 170)));
    // Pick times as guide lines so flattening is easy to judge.
    for (t, _) in s.iva.current_picks() {
        let y = out.frame.y_to_screen(t as f64);
        painter.hline(out.frame.rect.x_range(), y, Stroke::new(1.0, with_alpha(p.pick, 110)));
    }
    let title = if s.iva.apply_nmo { format!("NMO-corrected supergather · {} tr", s.iva.gather.len()) } else { format!("Gather / CIG (no NMO) · {} tr", s.iva.gather.len()) };
    badge(&painter, out.frame.rect.left_top() + vec2(6.0, 6.0), &title, p, egui::Align2::LEFT_TOP);
    if let Some((_, t)) = out.hover {
        s.crosshair.time_ms = Some(t as f32);
    }
}

fn cvs_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2) {
    sync_in(&mut s.iva.canvas_cvs, s.iva.t_view);
    let vels: Vec<f32> = s.iva.cvs.as_ref().map(|c| c.velocities_mps.clone()).unwrap_or_default();
    let vlab = vels.clone();
    let mut opts = CanvasOpts::seismic("CVS m/s");
    opts.primary_pans = false;
    opts.background = p.canvas;
    opts.x_ticks = 4;
    opts.left_gutter = 40.0;
    opts.x_label = Box::new(move |x| vlab.get(x.floor().max(0.0) as usize).map(|v| format!("{v:.0}")));
    let raster = s.iva.cvs_raster.as_ref();
    let out = canvas::canvas(ui, egui::Id::unique("iva_cvs"), &mut s.iva.canvas_cvs, p, size, opts, |painter, frame| {
        if let Some(r) = raster {
            draw_raster(painter, frame, r, Color32::from_white_alpha(140));
        }
    });
    sync_out(&out, &mut s.iva.t_view);
    let painter = ui.painter_at(out.frame.rect);
    if let Some(c) = &s.iva.cvs {
        draw_wiggles(&painter, &out.frame, &c.stacked_traces, 0.0, s.iva.cvs_clip, 1.4, p.text, Some(with_alpha(p.text, 150)));
    }
    // Velocity function mapped onto strip index.
    let mut handle_pts = Vec::new();
    if vels.len() >= 2 {
        let v0 = vels[0] as f64;
        let dvs = (vels[vels.len() - 1] - vels[0]) as f64 / (vels.len() - 1) as f64;
        let to_x = |v: f32| (v as f64 - v0) / dvs + 0.5;
        let f = s.iva.function();
        if f.picks.len() >= 2 {
            let (t0, t1) = (f.picks[0].0, f.picks[f.picks.len() - 1].0);
            let pts: Vec<egui::Pos2> = (0..=60).map(|i| {
                let t = t0 + (t1 - t0) * i as f32 / 60.0;
                out.frame.to_screen(to_x(f.velocity_at(t)), t as f64)
            }).collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.8, p.pick)));
        }
        for (t, v) in &f.picks {
            let c = out.frame.to_screen(to_x(*v), *t as f64);
            painter.circle(c, 3.5, Color32::from_black_alpha(120), Stroke::new(1.5, p.pick));
            handle_pts.push(c);
        }
    }
    badge(&painter, out.frame.rect.left_top() + vec2(6.0, 6.0), "Constant-velocity stacks", p, egui::Align2::LEFT_TOP);
    edit_picks(s, &out, &handle_pts, PickSurface::Cvs);
}

fn dix_plot(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2) {
    let f = s.iva.function();
    let iv = f.interval_velocities();
    let rms: Vec<[f64; 2]> = f.picks.iter().map(|(t, v)| [*v as f64, *t as f64]).collect();
    let mut step: Vec<[f64; 2]> = Vec::new();
    let mut bad: Vec<[f64; 2]> = Vec::new();
    let mut prev_v: Option<f32> = None;
    for (t0, t1, v) in &iv {
        step.push([*v as f64, *t0 as f64]);
        step.push([*v as f64, *t1 as f64]);
        let nonphysical = !v.is_finite() || *v < 300.0 || prev_v.is_some_and(|pv| *v < pv * 0.80);
        if nonphysical {
            bad.push([*v as f64, ((*t0 + *t1) * 0.5) as f64]);
        }
        prev_v = Some(*v);
    }
    let tview = s.iva.t_view;
    ui.allocate_ui(size, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Dix interval velocity").font(crate::theme::font_display(12.0)));
            if !bad.is_empty() {
                chip(ui, &format!("{} inversion(s)", bad.len()), p.pick, with_alpha(p.pick, 24));
            } else if iv.len() >= 1 {
                chip(ui, "physical", p.success, with_alpha(p.success, 20));
            }
        });
        let mut plot = Plot::new("dix_plot")
            .height(size.y - 24.0)
            .invert_y(true)
            .legend(Legend::default().position(egui_plot::Corner::RightBottom))
            .x_axis_label("m/s")
            .y_axis_label("ms")
            .show_background(false)
            .allow_scroll(false);
        if let Some((a, b)) = tview {
            plot = plot.include_y(a).include_y(b);
        }
        plot.show(ui, |pu| {
            if rms.len() >= 2 {
                pu.line(Line::new("V rms", PlotPoints::from(rms.clone())).color(p.accent).width(1.6));
            }
            pu.points(Points::new("picks", PlotPoints::from(rms)).color(p.accent).radius(2.5));
            if step.len() >= 2 {
                pu.line(Line::new("V int (Dix)", PlotPoints::from(step)).color(p.series_c).width(1.8));
            }
            if !bad.is_empty() {
                pu.points(Points::new("non-physical", PlotPoints::from(bad)).color(p.pick).radius(5.0).shape(egui_plot::MarkerShape::Diamond));
            }
        });
    });
}

/// Velocity Table pane: functions per CDP, save / persist / load.
pub fn table_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    load_pending(s);
    let tables: Vec<String> = s.session().map(|x| {
        let mut v: Vec<String> = x.context.velocities.keys().cloned().collect();
        v.sort();
        v
    }).unwrap_or_default();
    widgets::toolbar(ui, &p, |ui| {
        ui.add(egui::TextEdit::singleline(&mut s.iva.table_name).font(font_mono(12.0)).desired_width(140.0)).on_hover_text("Velocity table name");
        egui::ComboBox::from_id_salt("vt_load").selected_text("Load…").width(70.0).show_ui(ui, |ui| {
            for n in &tables {
                if ui.selectable_label(false, n).clicked() {
                    s.iva.select_table(n);
                }
            }
        });
    });
    widgets::body(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!s.is_busy(), |ui| {
                if primary_button(ui, &p, icon::FLOPPY_DISK, "Save").on_hover_text("UpsertVelocityTable").clicked() {
                    save_table(s, false);
                }
                if ui.button(format!("{}  Save + OPF", icon::DATABASE)).on_hover_text("Upsert, then SaveOpfDatabase").clicked() {
                    save_table(s, true);
                }
            });
        });
        section(ui, &p, "Picked functions", None);
        let mut jump: Option<i32> = None;
        let mut del: Option<i32> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("vt_grid").striped(true).num_columns(4).spacing([12.0, 3.0]).show(ui, |ui| {
                for h in ["CDP", "picks", "t range (ms)", ""] {
                    ui.label(RichText::new(h).size(11.0).color(p.text_muted));
                }
                ui.end_row();
                for (cdp, picks) in &s.iva.functions {
                    if picks.is_empty() {
                        continue;
                    }
                    let cur = *cdp == s.iva.gather_cdp;
                    if ui.selectable_label(cur, RichText::new(cdp.to_string()).font(font_mono(11.5))).clicked() {
                        jump = Some(*cdp);
                    }
                    ui.label(RichText::new(picks.len().to_string()).font(font_mono(11.0)));
                    let (a, b) = (picks.first().map(|x| x.0).unwrap_or(0.0), picks.last().map(|x| x.0).unwrap_or(0.0));
                    ui.label(RichText::new(format!("{a:.0}–{b:.0}")).font(font_mono(11.0)).color(p.text_muted));
                    if ui.small_button(icon::X).clicked() {
                        del = Some(*cdp);
                    }
                    ui.end_row();
                }
            });
            if s.iva.functions.values().all(|v| v.is_empty()) {
                widgets::hint(ui, &p, icon::INFO, "Click on the semblance or CVS panels to pick (t, v). Auto-pick proposes a function.");
            }
        });
        if let Some(c) = jump {
            if let Some(i) = s.iva.index.ranges.iter().position(|r| r.key_value == c as i64) {
                s.iva.pos = i;
            }
        }
        if let Some(c) = del {
            s.iva.functions.remove(&c);
        }
    });
}
