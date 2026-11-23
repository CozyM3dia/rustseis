//! Velocity analysis: semblance with contours and confidence band, live-corrected gather with a
//! flatness meter per pick, constant-velocity strips, and a velocity-field strip for the line.

use crate::canvas::{self, CanvasOpts, CanvasState, Raster, View2D, badge, clip_amplitude, draw_raster, draw_wiggles};
use crate::data::{self, EnsembleIndex, RasterExtent, TraceSource, crop_rgba, fmt_count, render_gather_raster, sorted_by};
use crate::state::{AppState, JobPurpose, Level};
use crate::theme::{Palette, font_mono, mix, palette, with_alpha};
use crate::widgets::{self, caps, cmd, cmd_primary, mono};
use crate::worker::EngineJob;
use egui::{Color32, CornerRadius, RichText, Sense, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use egui_plot::{Line, Plot, PlotPoints, Points};
use rustseis_core::{Ensemble, Trace, VelocityFunction, VelocityTable};
use rustseis_engine::EngineCommand;
use rustseis_imaging::{CvsPanel, NmoParams, SemblancePanel, VelocityScanParams};
use rustseis_render::SeismicColormap;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

pub struct IvaState {
    pub source: TraceSource,
    user_picked_source: bool,
    pub table_name: String,
    pub functions: BTreeMap<i32, Vec<(f32, f32)>>,
    pub pos: usize,
    pub super_half: usize,
    pub scan: VelocityScanParams,
    pub nmo_stretch: f32,
    pub apply_nmo: bool,
    pub cvs_count: usize,
    pub snap: bool,
    pub show_field: bool,
    saved_hash: u64,
    jump_cdp: Option<i32>,
    pending_load: Option<String>,
    index: EnsembleIndex,
    canvas_sem: CanvasState,
    canvas_gather: CanvasState,
    canvas_cvs: CanvasState,
    t_view: Option<(f64, f64)>,
    hover_t: Option<f32>,
    gather: Vec<Trace>,
    gather_cdp: i32,
    gather_key: u64,
    sem: Option<SemblancePanel>,
    sem_key: u64,
    sem_raster: Option<Raster>,
    contours: Vec<(f32, Vec<[(f32, f32); 2]>)>,
    nmo_key: u64,
    nmo_raster: Option<Raster>,
    nmo_traces: Vec<Trace>,
    nmo_clip: f32,
    flatness: Vec<(f32, f32)>,
    cvs: Option<CvsPanel>,
    cvs_raster: Option<Raster>,
    cvs_clip: f32,
    field_tex: Option<egui::TextureHandle>,
    field_key: u64,
    field_range: (f32, f32, f32),
    drag: Option<usize>,
}

impl Default for IvaState {
    fn default() -> Self {
        Self {
            source: TraceSource::Active,
            user_picked_source: false,
            table_name: "ui_velocity".into(),
            functions: BTreeMap::new(),
            pos: 0,
            super_half: 1,
            scan: VelocityScanParams { v_min_mps: 1400.0, v_max_mps: 4500.0, dv_mps: 25.0, window_ms: 24.0, t_step_ms: 8.0, stretch_mute_pct: 50.0 },
            nmo_stretch: 40.0,
            apply_nmo: true,
            cvs_count: 21,
            snap: true,
            show_field: true,
            saved_hash: picks_map_hash(&BTreeMap::new()),
            jump_cdp: None,
            pending_load: None,
            index: EnsembleIndex::default(),
            canvas_sem: CanvasState::default(),
            canvas_gather: CanvasState::default(),
            canvas_cvs: CanvasState::default(),
            t_view: None,
            hover_t: None,
            gather: Vec::new(),
            gather_cdp: 0,
            gather_key: 0,
            sem: None,
            sem_key: 0,
            sem_raster: None,
            contours: Vec::new(),
            nmo_key: 0,
            nmo_raster: None,
            nmo_traces: Vec::new(),
            nmo_clip: 1.0,
            flatness: Vec::new(),
            cvs: None,
            cvs_raster: None,
            cvs_clip: 1.0,
            field_tex: None,
            field_key: 0,
            field_range: (0.0, 1.0, 1.0),
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
    /// Picks exist that were not saved to the session yet.
    pub fn has_unsaved(&self) -> bool {
        self.functions.values().any(|v| !v.is_empty()) && picks_map_hash(&self.functions) != self.saved_hash
    }
    pub fn current_cdp(&self) -> i32 {
        self.gather_cdp
    }
    pub fn n_locations(&self) -> usize {
        self.index.ranges.len()
    }
    pub fn auto_pick_current(&mut self) {
        if let Some(sem) = &self.sem {
            let f = sem.auto_pick(1200.0, 0.05, 250.0);
            self.functions.insert(self.gather_cdp, f.picks);
        }
    }
    pub fn clear_current(&mut self) {
        self.functions.remove(&self.gather_cdp);
    }
    fn stride(&self) -> usize {
        (self.index.ranges.len() / 40).max(self.super_half * 2 + 1).max(1)
    }
    /// Move to the previous / next analysis location.
    pub fn step_location(&mut self, dir: i32) {
        let n = self.index.ranges.len();
        if n == 0 {
            return;
        }
        let st = self.stride();
        self.pos = if dir < 0 { self.pos.saturating_sub(st) } else { (self.pos + st).min(n - 1) };
    }
    /// CDPs whose interval velocities invert or turn non-physical.
    fn flagged(&self) -> Vec<i32> {
        self.functions
            .iter()
            .filter(|(_, p)| p.len() >= 2)
            .filter(|(c, p)| !inversions(&VelocityFunction::new(**c, (*p).clone())).is_empty())
            .map(|(c, _)| *c)
            .collect()
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

fn picks_map_hash(m: &BTreeMap<i32, Vec<(f32, f32)>>) -> u64 {
    hash_of(m.iter().filter(|(_, v)| !v.is_empty()).map(|(c, v)| (*c, picks_hash(v))).collect::<Vec<_>>())
}

/// Interval-velocity intervals that drop sharply or go non-physical: (mid time, v_int).
fn inversions(f: &VelocityFunction) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut prev: Option<f32> = None;
    for (t0, t1, v) in f.interval_velocities() {
        if !v.is_finite() || v < 300.0 || prev.is_some_and(|pv| v < pv * 0.80) {
            out.push(((t0 + t1) * 0.5, v));
        }
        prev = Some(v);
    }
    out
}

struct SemResult {
    panel: SemblancePanel,
    img: egui::ColorImage,
    contours: Vec<(f32, Vec<[(f32, f32); 2]>)>,
    key: u64,
}
struct CvsResult {
    panel: CvsPanel,
    img: egui::ColorImage,
    key: u64,
}

/// Marching squares on the semblance grid; segments in (velocity, time) data coordinates.
fn contour_segments(sem: &SemblancePanel, level: f32) -> Vec<[(f32, f32); 2]> {
    let nv = sem.velocities_mps.len();
    let nt = sem.times_ms.len();
    let mut out = Vec::new();
    if nv < 2 || nt < 2 {
        return out;
    }
    let v = |iv: usize| sem.velocities_mps[iv];
    let t = |it: usize| sem.times_ms[it];
    let lerp = |a: f32, b: f32, fa: f32, fb: f32| if (fb - fa).abs() < 1e-9 { (a + b) * 0.5 } else { a + (b - a) * (level - fa) / (fb - fa) };
    for it in 0..nt - 1 {
        for iv in 0..nv - 1 {
            let f00 = sem.get(it, iv);
            let f01 = sem.get(it, iv + 1);
            let f10 = sem.get(it + 1, iv);
            let f11 = sem.get(it + 1, iv + 1);
            let idx = (f00 > level) as u8 | ((f01 > level) as u8) << 1 | ((f11 > level) as u8) << 2 | ((f10 > level) as u8) << 3;
            if idx == 0 || idx == 15 {
                continue;
            }
            // Edge crossing points: top (t=it), right (v=iv+1), bottom (t=it+1), left (v=iv).
            let top = (lerp(v(iv), v(iv + 1), f00, f01), t(it));
            let right = (v(iv + 1), lerp(t(it), t(it + 1), f01, f11));
            let bottom = (lerp(v(iv), v(iv + 1), f10, f11), t(it + 1));
            let left = (v(iv), lerp(t(it), t(it + 1), f00, f10));
            let segs: &[(u8, u8)] = match idx {
                1 | 14 => &[(3, 0)],
                2 | 13 => &[(0, 1)],
                3 | 12 => &[(3, 1)],
                4 | 11 => &[(1, 2)],
                5 => &[(3, 0), (1, 2)],
                6 | 9 => &[(0, 2)],
                7 | 8 => &[(3, 2)],
                10 => &[(0, 1), (2, 3)],
                _ => &[],
            };
            let pt = |e: u8| match e {
                0 => top,
                1 => right,
                2 => bottom,
                _ => left,
            };
            for (a, b) in segs {
                out.push([pt(*a), pt(*b)]);
            }
        }
    }
    out
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    if s.is_busy() && s.iva.sem_raster.is_none() {
        super::busy_notice(s, ui);
        return;
    }
    if s.session().is_some() {
        prepare(s, ui.ctx());
        if s.iva.pending_load.is_some() {
            load_pending(s);
            prepare(s, ui.ctx());
        }
    }
    if s.iva.gather.is_empty() {
        canvas::empty_state(ui, &p, icon::CHART_LINE_UP, "No CDP gathers yet", "Run a flow that sorts traces by CDP, or choose a saved gather set in the Inspector.");
        return;
    }
    let low_fold = s.iva.index.ranges.len() > 1 && s.iva.gather.len() < 4;

    let full = ui.available_rect_before_wrap().shrink2(vec2(8.0, 6.0));
    let field_h = if s.iva.show_field { 96.0 } else { 0.0 };
    let gap = 8.0;
    let top_h = full.height() - field_h - if field_h > 0.0 { gap } else { 0.0 };
    let w = full.width() - 2.0 * gap;
    let cols = [w * 0.30, w * 0.42, w * 0.28];
    let mut x = full.left();
    let mut rects = Vec::new();
    for c in cols {
        rects.push(egui::Rect::from_min_size(pos2(x, full.top()), vec2(c, top_h)));
        x += c + gap;
    }
    let mut child = |r: egui::Rect| ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
    {
        let mut u = child(rects[0]);
        semblance_canvas(s, &mut u, &p, rects[0].size());
    }
    {
        let mut u = child(rects[1]);
        gather_canvas(s, &mut u, &p, rects[1].size());
    }
    {
        let mut u = child(rects[2]);
        cvs_canvas(s, &mut u, &p, rects[2].size());
    }
    if field_h > 0.0 {
        let r = egui::Rect::from_min_size(pos2(full.left(), full.bottom() - field_h), vec2(full.width(), field_h));
        let mut u = child(r);
        field_strip(s, &mut u, &p, r);
    }
    if low_fold {
        let r = egui::Rect::from_center_size(pos2(full.center().x, full.top() + 22.0), vec2(460.0, 30.0));
        let mut u = child(r);
        widgets::banner(&mut u, &p, p.pick, icon::WARNING, "Very few traces per CDP: these gathers may not be sorted by CDP.");
    }
    ui.allocate_rect(full, Sense::hover());
}

fn load_pending(s: &mut AppState) {
    let Some(name) = s.iva.pending_load.clone() else { return };
    let Some(sess) = s.session() else { return };
    if let Some(vt) = sess.context.velocities.get(&name).cloned() {
        s.iva.functions = vt.functions.iter().map(|f| (f.cdp, f.picks.clone())).collect();
        s.iva.table_name = name.clone();
        s.iva.saved_hash = picks_map_hash(&s.iva.functions);
        // Jump to the picked location nearest the current one.
        let cur = s.iva.gather_cdp;
        if let Some(c) = s.iva.functions.keys().min_by_key(|c| (**c - cur).abs()).copied() {
            s.iva.jump_cdp = Some(c);
        }
        s.toast(Level::Ok, format!("Loaded velocities “{name}” · {} locations", vt.functions.len()));
    }
    s.iva.pending_load = None;
}

pub fn save_table(s: &mut AppState, persist: bool) {
    let vt = s.iva.table();
    let n = vt.functions.len();
    if n == 0 {
        s.toast(Level::Warn, "No velocity picks to save");
        return;
    }
    match s.dispatch_now(EngineCommand::UpsertVelocityTable(vt)) {
        Ok(_) => {
            s.iva.saved_hash = picks_map_hash(&s.iva.functions);
            s.toast(Level::Ok, format!("Saved velocities “{}” · {n} locations", s.iva.table_name));
        }
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

/// Pick the best pre-stack CDP gather buffer when Active is post-stack (fold < 3).
fn best_cdp_buffer(sess: &rustseis_engine::EngineSession) -> Option<TraceSource> {
    let score_traces = |tr: &[Trace]| -> usize {
        if tr.len() < 4 {
            return 0;
        }
        let idx = EnsembleIndex::build(tr, TraceSource::Active, "CDP", 0);
        idx.ranges.iter().map(|r| r.len()).max().unwrap_or(0)
    };
    if score_traces(&sess.active_traces) >= 3 {
        return None;
    }
    let mut cands: Vec<(i32, usize, String)> = sess
        .context
        .buffers
        .iter()
        .filter_map(|(name, tr)| {
            let max_fold = score_traces(tr);
            if max_fold < 3 {
                return None;
            }
            let low = name.to_ascii_lowercase();
            let rank = if low.contains("cdp") && !low.contains("nmo") {
                30
            } else if low.contains("cdp") {
                20
            } else if low.contains("before") || low.contains("nmo") {
                10
            } else {
                0
            };
            Some((rank, max_fold, name.clone()))
        })
        .collect();
    cands.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2)));
    cands.into_iter().next().map(|(_, _, n)| TraceSource::Buffer(n))
}

fn prepare(s: &mut AppState, ctx: &egui::Context) {
    if s.session().is_none() {
        return;
    }
    // If the user hasn't explicitly chosen a source and Active is post-stack (e.g. after a stack
    // step), automatically switch to a pre-stack CDP gather buffer when one exists.
    if !s.iva.user_picked_source && s.iva.source == TraceSource::Active {
        if let Some(best) = s.session().and_then(best_cdp_buffer) {
            s.iva.source = best;
            s.iva.gather_key = 0;
        }
    }
    if !s.iva.index.matches(&s.iva.source, "CDP", s.rev) {
        let idx = {
            let sess = s.session().expect("checked");
            EnsembleIndex::build(s.iva.source.traces(sess), s.iva.source.clone(), "CDP", s.rev)
        };
        s.iva.index = idx;
        if s.iva.pos >= s.iva.index.ranges.len() || s.iva.gather_key == 0 {
            s.iva.pos = s.iva.index.ranges.len() / 2;
        }
        s.iva.field_key = 0;
    }
    if let Some(c) = s.iva.jump_cdp.take() {
        if let Some(i) = s.iva.index.ranges.iter().position(|r| r.key_value == c as i64) {
            s.iva.pos = i;
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
    // Semblance (with contours) and constant-velocity strips in the background.
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
            let peak = panel.values.iter().copied().fold(0.0f32, f32::max).max(1e-6);
            let contours = [0.35f32, 0.55, 0.75].iter().map(|f| (*f, contour_segments(&panel, peak * f))).collect();
            Ok(SemResult { panel, img, contours, key: skey })
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
    // Live NMO (inline: a few ms for typical supergathers) and per-pick flatness.
    let picks = s.iva.current_picks();
    let nkey = hash_of((gkey, picks_hash(&picks), s.iva.nmo_stretch.to_bits(), s.iva.apply_nmo));
    if nkey != s.iva.nmo_key {
        s.iva.nmo_key = nkey;
        let mut tr = s.iva.gather.clone();
        if s.iva.apply_nmo && !picks.is_empty() {
            let vt = VelocityTable::new("iva_live", vec![VelocityFunction::new(s.iva.gather_cdp, picks.clone())]);
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
        s.iva.flatness = picks.iter().map(|(t, _)| (*t, flatness_at(&tr, *t, s.iva.scan.window_ms.max(16.0)))).collect();
        s.iva.nmo_traces = tr;
    }
    // Velocity field for the whole line.
    let fkey = hash_of((picks_map_hash(&s.iva.functions), n, s.rev));
    if s.iva.show_field && fkey != s.iva.field_key {
        s.iva.field_key = fkey;
        build_field(s, ctx);
    }
}

/// Coherence of the corrected gather in a window around `t`: 1 = perfectly flat.
fn flatness_at(traces: &[Trace], t_ms: f32, win_ms: f32) -> f32 {
    let Some(first) = traces.first() else { return 0.0 };
    let dt = first.dt_ms();
    if dt <= 0.0 {
        return 0.0;
    }
    let c = (t_ms / dt).round() as isize;
    let h = ((win_ms * 0.5) / dt).round().max(1.0) as isize;
    let (mut num, mut den, mut n_live) = (0.0f64, 0.0f64, 0usize);
    for tr in traces {
        if tr.samples.iter().any(|v| *v != 0.0) {
            n_live += 1;
        }
    }
    if n_live < 2 {
        return 0.0;
    }
    for i in (c - h)..=(c + h) {
        if i < 0 {
            continue;
        }
        let (mut sum, mut sq) = (0.0f64, 0.0f64);
        for tr in traces {
            if let Some(v) = tr.samples.get(i as usize) {
                sum += *v as f64;
                sq += (*v as f64) * (*v as f64);
            }
        }
        num += sum * sum;
        den += sq;
    }
    if den <= 0.0 { 0.0 } else { (num / (n_live as f64 * den)) as f32 }
}

fn nmo_defaults() -> NmoParams {
    serde_json::from_value(serde_json::json!({})).expect("NmoParams has serde defaults")
}

/// Colour ramp for velocity: deep blue → mint → amber → coral.
pub fn velocity_ramp(t: f32) -> Color32 {
    let stops = [(0.0, Color32::from_rgb(27, 42, 107)), (0.35, Color32::from_rgb(40, 140, 170)), (0.6, Color32::from_rgb(60, 207, 191)), (0.8, Color32::from_rgb(255, 201, 74)), (1.0, Color32::from_rgb(255, 107, 74))];
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let (a, ca) = w[0];
        let (b, cb) = w[1];
        if t <= b {
            return mix(ca, cb, (t - a) / (b - a));
        }
    }
    stops[stops.len() - 1].1
}

fn build_field(s: &mut AppState, ctx: &egui::Context) {
    let vt = s.iva.table();
    let n = s.iva.index.ranges.len();
    if vt.functions.is_empty() || n == 0 {
        s.iva.field_tex = None;
        return;
    }
    let (dt, ns) = data::time_axis_ms(&s.iva.gather);
    let tmax = ns as f32 * dt;
    let w = n.min(480);
    let h = 72usize;
    let (vmin, vmax) = (s.iva.scan.v_min_mps, s.iva.scan.v_max_mps);
    let mut img = egui::ColorImage::filled([w, h], Color32::TRANSPARENT);
    for ix in 0..w {
        let pos = ix * n / w;
        let cdp = s.iva.index.ranges[pos].key_value as i32;
        for iy in 0..h {
            let t = (iy as f32 + 0.5) / h as f32 * tmax;
            let v = vt.velocity_at(cdp, t);
            img.pixels[iy * w + ix] = velocity_ramp((v - vmin) / (vmax - vmin).max(1.0));
        }
    }
    s.iva.field_range = (vmin, vmax, tmax);
    match &mut s.iva.field_tex {
        Some(t) => t.set(img, egui::TextureOptions::LINEAR),
        None => s.iva.field_tex = Some(ctx.load_texture("iva_field", img, egui::TextureOptions::LINEAR)),
    }
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
                s.iva.contours = r.contours;
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

/// Keep the three panels on one time axis.
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

fn panel_title(painter: &egui::Painter, rect: egui::Rect, p: &Palette, title: &str, sub: &str) {
    let pos = rect.left_top() + vec2(10.0, 8.0);
    let g = painter.layout_no_wrap(title.to_string(), crate::theme::font_semibold(12.0), p.text);
    let g2 = painter.layout_no_wrap(sub.to_string(), egui::FontId::proportional(11.0), p.text_muted);
    let w = g.size().x + if sub.is_empty() { 0.0 } else { g2.size().x + 8.0 };
    let bg = egui::Rect::from_min_size(pos - vec2(6.0, 4.0), vec2(w + 12.0, 22.0));
    painter.rect_filled(bg, CornerRadius::same(6), with_alpha(p.panel, 220));
    painter.galley(pos, g.clone(), p.text);
    if !sub.is_empty() {
        painter.galley(pos + vec2(g.size().x + 8.0, 1.0), g2, p.text_muted);
    }
}

/// Velocity range at time `t` where semblance stays within 85 % of the local peak.
fn confidence_band(sem: &SemblancePanel, v: f32, t: f32) -> Option<(f32, f32)> {
    let nv = sem.velocities_mps.len();
    let nt = sem.times_ms.len();
    if nv < 3 || nt < 2 {
        return None;
    }
    let dv = sem.velocities_mps[1] - sem.velocities_mps[0];
    let dt = sem.times_ms[1] - sem.times_ms[0];
    let it = (((t - sem.times_ms[0]) / dt).round() as isize).clamp(0, nt as isize - 1) as usize;
    let iv = (((v - sem.velocities_mps[0]) / dv).round() as isize).clamp(0, nv as isize - 1) as usize;
    let peak = sem.get(it, iv);
    if peak <= 0.0 {
        return None;
    }
    let thr = peak * 0.85;
    let (mut lo, mut hi) = (iv, iv);
    while lo > 0 && sem.get(it, lo - 1) >= thr {
        lo -= 1;
    }
    while hi + 1 < nv && sem.get(it, hi + 1) >= thr {
        hi += 1;
    }
    Some((sem.velocities_mps[lo] - dv * 0.5, sem.velocities_mps[hi] + dv * 0.5))
}

fn semblance_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2) {
    sync_in(&mut s.iva.canvas_sem, s.iva.t_view);
    let mut opts = CanvasOpts::seismic("velocity m/s");
    opts.primary_pans = false;
    opts.background = p.canvas;
    opts.x_ticks = 4;
    opts.x_label = Box::new(|x| Some(format!("{x:.0}")));
    let raster = s.iva.sem_raster.as_ref();
    let contours = &s.iva.contours;
    let out = canvas::canvas(ui, egui::Id::unique("iva_sem"), &mut s.iva.canvas_sem, p, size, opts, |painter, frame| {
        if let Some(r) = raster {
            draw_raster(painter, frame, r, Color32::WHITE);
        }
        for (lvl, segs) in contours {
            let a = (60.0 + 140.0 * lvl) as u8;
            let st = Stroke::new(if *lvl > 0.7 { 1.2 } else { 0.8 }, Color32::from_white_alpha(a));
            for [(v0, t0), (v1, t1)] in segs {
                painter.line_segment([frame.to_screen(*v0 as f64, *t0 as f64), frame.to_screen(*v1 as f64, *t1 as f64)], st);
            }
        }
    });
    sync_out(&out, &mut s.iva.t_view);
    let painter = ui.painter_at(out.frame.rect);
    let picks = s.iva.current_picks();
    // Confidence band around the function.
    if let Some(sem) = &s.iva.sem {
        let band: Vec<(f32, f32, f32)> = picks.iter().filter_map(|(t, v)| confidence_band(sem, *v, *t).map(|(a, b)| (*t, a, b))).collect();
        if band.len() >= 2 {
            let mut poly: Vec<egui::Pos2> = band.iter().map(|(t, a, _)| out.frame.to_screen(*a as f64, *t as f64)).collect();
            poly.extend(band.iter().rev().map(|(t, _, b)| out.frame.to_screen(*b as f64, *t as f64)));
            let left: Vec<egui::Pos2> = band.iter().map(|(t, a, _)| out.frame.to_screen(*a as f64, *t as f64)).collect();
            let right: Vec<egui::Pos2> = band.iter().map(|(t, _, b)| out.frame.to_screen(*b as f64, *t as f64)).collect();
            for side in [left, right] {
                painter.add(egui::Shape::dashed_line(&side, Stroke::new(1.0, with_alpha(p.pick, 170)), 4.0, 3.0));
            }
            let _ = poly;
        }
    }
    let pts: Vec<egui::Pos2> = picks.iter().map(|(t, v)| out.frame.to_screen(*v as f64, *t as f64)).collect();
    if pts.len() > 1 {
        painter.add(egui::Shape::line(pts.clone(), Stroke::new(4.0, Color32::from_black_alpha(150))));
        painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.0, p.pick)));
    }
    for (i, c) in pts.iter().enumerate() {
        let active = s.iva.drag == Some(i);
        painter.circle(*c, if active { 6.0 } else { 4.5 }, p.pick, Stroke::new(2.0, Color32::from_black_alpha(200)));
    }
    if let (Some((hv, ht)), Some(sem)) = (out.hover, &s.iva.sem) {
        if let Some(val) = semblance_at(sem, hv as f32, ht as f32) {
            let peak = sem.values.iter().copied().fold(0.0f32, f32::max).max(1e-6);
            badge(&painter, out.frame.rect.right_bottom() + vec2(-8.0, -8.0), &format!("{ht:.0} ms · {hv:.0} m/s · coherence {:.0}%", val / peak * 100.0), p, egui::Align2::RIGHT_BOTTOM);
        }
        s.crosshair.time_ms = Some(ht as f32);
        s.iva.hover_t = Some(ht as f32);
    }
    let sub = if s.compute.is_busy("iva.semblance") { "computing…".to_string() } else { format!("CDP {}", s.iva.gather_cdp) };
    panel_title(&painter, out.frame.rect, p, "Semblance", &sub);
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

fn flat_color(p: &Palette, c: f32) -> Color32 {
    if c >= 0.6 {
        p.success
    } else if c >= 0.35 {
        p.pick
    } else {
        p.danger
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
    // Pick guides and a flatness meter for each pick on the right edge.
    let r = out.frame.rect;
    for (t, c) in &s.iva.flatness {
        let y = out.frame.y_to_screen(*t as f64);
        if y < r.top() || y > r.bottom() {
            continue;
        }
        let col = flat_color(p, *c);
        painter.add(egui::Shape::dashed_line(&[pos2(r.left(), y), pos2(r.right() - 64.0, y)], Stroke::new(1.0, with_alpha(col, 150)), 6.0, 4.0));
        let track = egui::Rect::from_min_size(pos2(r.right() - 58.0, y - 4.0), vec2(48.0, 8.0));
        painter.rect_filled(track, CornerRadius::same(4), with_alpha(p.panel, 230));
        let mut fill = track;
        fill.set_width(48.0 * c.clamp(0.0, 1.0));
        painter.rect_filled(fill, CornerRadius::same(4), col);
    }
    let title = if s.iva.apply_nmo { "Corrected gather" } else { "Gather without NMO" };
    panel_title(&painter, r, p, title, &format!("{} traces · flatness at each pick", s.iva.gather.len()));
    if let Some((_, t)) = out.hover {
        s.crosshair.time_ms = Some(t as f32);
        s.iva.hover_t = Some(t as f32);
    }
    if let Some(t) = s.iva.hover_t {
        let y = out.frame.y_to_screen(t as f64);
        painter.hline(r.x_range(), y, Stroke::new(1.0, with_alpha(p.crosshair, 120)));
    }
}

fn cvs_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2) {
    sync_in(&mut s.iva.canvas_cvs, s.iva.t_view);
    let vels: Vec<f32> = s.iva.cvs.as_ref().map(|c| c.velocities_mps.clone()).unwrap_or_default();
    let vlab = vels.clone();
    let mut opts = CanvasOpts::seismic("constant velocity m/s");
    opts.primary_pans = false;
    opts.background = p.canvas;
    opts.x_ticks = 4;
    opts.left_gutter = 40.0;
    opts.x_label = Box::new(move |x| vlab.get(x.floor().max(0.0) as usize).map(|v| format!("{v:.0}")));
    let raster = s.iva.cvs_raster.as_ref();
    let out = canvas::canvas(ui, egui::Id::unique("iva_cvs"), &mut s.iva.canvas_cvs, p, size, opts, |painter, frame| {
        if let Some(r) = raster {
            draw_raster(painter, frame, r, Color32::from_white_alpha(120));
        }
    });
    sync_out(&out, &mut s.iva.t_view);
    let painter = ui.painter_at(out.frame.rect);
    // Best strip at the hovered time: the one with the most stack energy.
    if let (Some(c), Some(t)) = (&s.iva.cvs, s.iva.hover_t) {
        if let Some(first) = c.stacked_traces.first() {
            let dt = first.dt_ms();
            let i0 = ((t / dt.max(1e-3)) as isize - 6).max(0) as usize;
            let best = c
                .stacked_traces
                .iter()
                .enumerate()
                .map(|(i, tr)| (i, tr.samples.iter().skip(i0).take(13).map(|v| v * v).sum::<f32>()))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
            if let Some(b) = best {
                let x0 = out.frame.x_to_screen(b as f64);
                let x1 = out.frame.x_to_screen(b as f64 + 1.0);
                let rr = egui::Rect::from_x_y_ranges(x0..=x1, out.frame.rect.y_range());
                painter.rect_filled(rr, CornerRadius::ZERO, with_alpha(p.accent, 28));
                painter.rect_stroke(rr, CornerRadius::same(2), Stroke::new(1.2, p.accent), egui::StrokeKind::Inside);
                if let Some(v) = c.velocities_mps.get(b) {
                    badge(&painter, pos2(rr.center().x, out.frame.rect.bottom() - 8.0), &format!("best {v:.0} m/s"), p, egui::Align2::CENTER_BOTTOM);
                }
            }
        }
    }
    if let Some(c) = &s.iva.cvs {
        draw_wiggles(&painter, &out.frame, &c.stacked_traces, 0.0, s.iva.cvs_clip, 1.2, with_alpha(p.text, 150), Some(with_alpha(p.text, 70)));
    }
    let mut handle_pts = Vec::new();
    if vels.len() >= 2 {
        let v0 = vels[0] as f64;
        let dvs = (vels[vels.len() - 1] - vels[0]) as f64 / (vels.len() - 1) as f64;
        let to_x = |v: f32| (v as f64 - v0) / dvs + 0.5;
        let f = s.iva.function();
        if f.picks.len() >= 2 {
            let (t0, t1) = (f.picks[0].0, f.picks[f.picks.len() - 1].0);
            let pts: Vec<egui::Pos2> = (0..=60)
                .map(|i| {
                    let t = t0 + (t1 - t0) * i as f32 / 60.0;
                    out.frame.to_screen(to_x(f.velocity_at(t)), t as f64)
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(1.8, p.pick)));
        }
        for (t, v) in &f.picks {
            let c = out.frame.to_screen(to_x(*v), *t as f64);
            painter.circle(c, 3.5, p.pick, Stroke::new(1.5, Color32::from_black_alpha(200)));
            handle_pts.push(c);
        }
    }
    panel_title(&painter, out.frame.rect, p, "Constant-velocity stacks", "");
    edit_picks(s, &out, &handle_pts, PickSurface::Cvs);
}

/// Velocity field along the line with analysis locations; click to jump.
fn field_strip(s: &mut AppState, ui: &mut Ui, p: &Palette, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(8), p.sunken);
    painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    let label_w = 150.0;
    let plot = egui::Rect::from_min_max(pos2(rect.left() + label_w, rect.top() + 10.0), pos2(rect.right() - 92.0, rect.bottom() - 18.0));
    painter.text(rect.left_top() + vec2(12.0, 10.0), egui::Align2::LEFT_TOP, "Velocity field", crate::theme::font_semibold(12.0), p.text);
    let n = s.iva.index.ranges.len();
    let picked = s.iva.functions.values().filter(|v| !v.is_empty()).count();
    painter.text(rect.left_top() + vec2(12.0, 28.0), egui::Align2::LEFT_TOP, format!("{picked} of {} locations picked", fmt_count(n)), egui::FontId::proportional(11.0), p.text_muted);
    let flagged = s.iva.flagged();
    if !flagged.is_empty() {
        painter.text(rect.left_top() + vec2(12.0, 46.0), egui::Align2::LEFT_TOP, format!("{}  {} to check", icon::WARNING, flagged.len()), egui::FontId::proportional(11.0), p.pick);
    }
    match &s.iva.field_tex {
        Some(t) => {
            painter.image(t.id(), plot, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            painter.rect_filled(plot, CornerRadius::same(4), p.canvas);
            painter.text(plot.center(), egui::Align2::CENTER_CENTER, "Pick a few locations to see the field fill in", egui::FontId::proportional(11.5), p.text_faint);
        }
    }
    if n > 0 {
        let x_of = |pos: usize| plot.left() + (pos as f32 + 0.5) / n as f32 * plot.width();
        // Analysis locations
        for (i, r) in s.iva.index.ranges.iter().enumerate() {
            let cdp = r.key_value as i32;
            if s.iva.functions.get(&cdp).is_some_and(|v| !v.is_empty()) {
                let x = x_of(i);
                let col = if flagged.contains(&cdp) { p.pick } else { p.text };
                painter.vline(x, plot.y_range(), Stroke::new(1.0, with_alpha(col, 110)));
                painter.circle_filled(pos2(x, plot.bottom() + 6.0), 3.0, col);
            }
        }
        let cx = x_of(s.iva.pos.min(n - 1));
        painter.vline(cx, plot.y_range(), Stroke::new(2.0, p.accent));
        painter.text(pos2(cx, plot.top() - 1.0), egui::Align2::CENTER_BOTTOM, "▼", egui::FontId::proportional(9.0), p.accent);
        let a = s.iva.index.ranges.first().map(|r| r.key_value).unwrap_or(0);
        let b = s.iva.index.ranges.last().map(|r| r.key_value).unwrap_or(0);
        painter.text(pos2(plot.left(), rect.bottom() - 4.0), egui::Align2::LEFT_BOTTOM, format!("CDP {a}"), font_mono(10.0), p.text_faint);
        painter.text(pos2(plot.right(), rect.bottom() - 4.0), egui::Align2::RIGHT_BOTTOM, format!("CDP {b}"), font_mono(10.0), p.text_faint);
        let resp = ui.interact(plot.expand2(vec2(0.0, 8.0)), egui::Id::unique("iva_field"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if let Some(ptr) = resp.hover_pos() {
            let pos = (((ptr.x - plot.left()) / plot.width()) * n as f32).floor().clamp(0.0, n as f32 - 1.0) as usize;
            let cdp = s.iva.index.ranges[pos].key_value;
            let t = (ptr.y - plot.top()) / plot.height() * s.iva.field_range.2;
            let v = s.iva.table().velocity_at(cdp as i32, t);
            let txt = if s.iva.field_tex.is_some() { format!("CDP {cdp} · {t:.0} ms · {v:.0} m/s") } else { format!("CDP {cdp}") };
            badge(&painter, pos2(ptr.x, plot.top() + 4.0), &txt, p, egui::Align2::CENTER_TOP);
            if resp.clicked() || resp.dragged() {
                s.iva.pos = pos;
            }
        }
    }
    // Legend
    let lg = egui::Rect::from_min_max(pos2(rect.right() - 80.0, plot.top() + 4.0), pos2(rect.right() - 70.0, plot.bottom() - 4.0));
    let steps = 24;
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let seg = egui::Rect::from_min_max(pos2(lg.left(), lg.top() + t0 * lg.height()), pos2(lg.right(), lg.top() + (t0 + 1.0 / steps as f32) * lg.height() + 0.5));
        painter.rect_filled(seg, CornerRadius::ZERO, velocity_ramp(t0));
    }
    let (vmin, vmax, _) = s.iva.field_range;
    let (vmin, vmax) = if s.iva.field_tex.is_some() { (vmin, vmax) } else { (s.iva.scan.v_min_mps, s.iva.scan.v_max_mps) };
    painter.text(pos2(lg.right() + 4.0, lg.top()), egui::Align2::LEFT_TOP, format!("{vmin:.0}"), font_mono(9.5), p.text_muted);
    painter.text(pos2(lg.right() + 4.0, lg.bottom()), egui::Align2::LEFT_BOTTOM, format!("{vmax:.0}"), font_mono(9.5), p.text_muted);
}

/// Inspector: picks with coherence and flatness, RMS / interval chart, table and scan settings.
pub fn inspector_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    load_pending(s);
    let sources = s.session().map(TraceSource::all).unwrap_or_default();
    let tables: Vec<String> = s
        .session()
        .map(|x| {
            let mut v: Vec<String> = x.context.velocities.keys().cloned().collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new(format!("CDP {}", s.iva.gather_cdp)).font(crate::theme::font_display(18.0)).color(p.text));
            ui.label(RichText::new(format!("location {} of {} · {} traces in the supergather", s.iva.pos + 1, fmt_count(s.iva.index.ranges.len()), s.iva.gather.len())).size(11.5).color(p.text_muted));
            widgets::row(ui, &p, "Gathers from", |ui| {
                egui::ComboBox::from_id_salt("iva_src").selected_text(s.iva.source.label()).width(150.0).show_ui(ui, |ui| {
                    for src in &sources {
                        if ui.selectable_label(*src == s.iva.source, src.label()).clicked() {
                            s.iva.source = src.clone();
                            s.iva.user_picked_source = true;
                            s.iva.pos = 0;
                        }
                    }
                });
            });
            ui.add_space(4.0);

            // ---- picks
            let picks = s.iva.current_picks();
            caps(ui, &p, "PICKS", Some(&format!("{}", picks.len())));
            if picks.is_empty() {
                widgets::hint(ui, &p, icon::CURSOR_CLICK, "Click the semblance where energy peaks. Right-click a pick to remove it, drag to move it.");
                if cmd_primary(ui, &p, icon::SPARKLE, "Auto-pick this location").clicked() {
                    s.iva.auto_pick_current();
                }
            } else {
                let peak = s.iva.sem.as_ref().map(|x| x.values.iter().copied().fold(0.0f32, f32::max).max(1e-6)).unwrap_or(1.0);
                let mut remove: Option<usize> = None;
                egui::Grid::new("iva_picks").num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
                    for h in ["time", "velocity", "coherence", "flat"] {
                        ui.label(RichText::new(h).size(10.5).color(p.text_faint));
                    }
                    ui.end_row();
                    for (i, (t, v)) in picks.iter().enumerate() {
                        ui.label(mono(format!("{t:.0} ms"), 11.5, p.text));
                        ui.label(mono(format!("{v:.0}"), 11.5, p.text));
                        let coh = s.iva.sem.as_ref().and_then(|sem| semblance_at(sem, *v, *t)).map(|x| x / peak).unwrap_or(0.0);
                        bar(ui, &p, coh, p.accent);
                        let fl = s.iva.flatness.get(i).map(|x| x.1).unwrap_or(0.0);
                        let r = bar(ui, &p, fl, flat_color(&p, fl));
                        if r.secondary_clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                });
                if let Some(i) = remove {
                    if let Some(l) = s.iva.functions.get_mut(&s.iva.gather_cdp) {
                        l.remove(i);
                    }
                }
            }

            // ---- chart
            ui.add_space(4.0);
            caps(ui, &p, "RMS AND INTERVAL VELOCITY", None);
            let f = s.iva.function();
            let inv = inversions(&f);
            dix_chart(ui, &p, &f, &inv, s.iva.t_view);
            if !inv.is_empty() {
                let (t, v) = inv[0];
                let (go, _) = widgets::insight(
                    ui,
                    &p,
                    "Interval velocity drops here",
                    &format!("Between picks near {t:.0} ms the interval velocity falls to {v:.0} m/s. Check whether a multiple was picked instead of a primary."),
                    "Auto-pick again",
                    "",
                );
                if go {
                    s.iva.auto_pick_current();
                }
            }

            // ---- table
            ui.add_space(4.0);
            caps(ui, &p, "VELOCITY TABLE", if s.iva.has_unsaved() { Some("unsaved changes") } else { None });
            widgets::row(ui, &p, "Name", |ui| {
                ui.add(egui::TextEdit::singleline(&mut s.iva.table_name).font(font_mono(12.0)).desired_width(150.0));
            });
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!s.is_busy(), |ui| {
                    if cmd_primary(ui, &p, icon::CHECK, "Save").clicked() {
                        save_table(s, false);
                    }
                    if cmd(ui, &p, icon::DATABASE, "Save to project", false).clicked() {
                        save_table(s, true);
                    }
                });
                let r = cmd(ui, &p, icon::FOLDER_OPEN, "Load  ▾", false);
                egui::Popup::menu(&r).show(|ui| {
                    if tables.is_empty() {
                        ui.label(RichText::new("No saved velocities yet").color(p.text_muted));
                    }
                    for n in &tables {
                        if ui.button(n).clicked() {
                            s.iva.select_table(n);
                        }
                    }
                });
            });
            locations_list(s, ui, &p);

            // ---- scan settings
            ui.add_space(4.0);
            egui::CollapsingHeader::new(RichText::new("Scan settings").font(crate::theme::font_semibold(12.0)).color(p.text_muted)).id_salt("iva_scan").show(ui, |ui| {
                let sc = &mut s.iva.scan;
                egui::Grid::new("scan_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Lowest velocity");
                    ui.add(egui::DragValue::new(&mut sc.v_min_mps).range(300.0..=8000.0).speed(10.0).suffix(" m/s"));
                    ui.end_row();
                    ui.label("Highest velocity");
                    ui.add(egui::DragValue::new(&mut sc.v_max_mps).range(500.0..=12000.0).speed(10.0).suffix(" m/s"));
                    ui.end_row();
                    ui.label("Velocity step");
                    ui.add(egui::DragValue::new(&mut sc.dv_mps).range(5.0..=500.0).speed(1.0).suffix(" m/s"));
                    ui.end_row();
                    ui.label("Window");
                    ui.add(egui::DragValue::new(&mut sc.window_ms).range(4.0..=200.0).speed(1.0).suffix(" ms"));
                    ui.end_row();
                    ui.label("Time step");
                    ui.add(egui::DragValue::new(&mut sc.t_step_ms).range(2.0..=100.0).speed(1.0).suffix(" ms"));
                    ui.end_row();
                    ui.label("Stretch limit");
                    ui.add(egui::DragValue::new(&mut sc.stretch_mute_pct).range(0.0..=200.0).suffix(" %"));
                    ui.end_row();
                    ui.label("Corrected-gather stretch");
                    ui.add(egui::DragValue::new(&mut s.iva.nmo_stretch).range(0.0..=200.0).suffix(" %"));
                    ui.end_row();
                    ui.label("Constant-velocity strips");
                    ui.add(egui::DragValue::new(&mut s.iva.cvs_count).range(5..=61));
                    ui.end_row();
                });
                if ui.button(format!("{}  Sort the data by CDP", icon::ARROWS_LEFT_RIGHT)).on_hover_text("Re-orders the loaded traces by CDP and offset. Undo with Ctrl+Z.").clicked() {
                    let spec: rustseis_flow::FlowSpec = serde_json::from_value(serde_json::json!({
                        "name": "Sort by CDP",
                        "steps": [{"tool": "inline_sort", "params": {"primary": "CDP", "secondary": "AOFFSET"}}]
                    }))
                    .expect("static flow");
                    s.iva.source = TraceSource::Active;
                    s.run_flow(spec, JobPurpose::Flow { flow_name: "Sort by CDP".into() });
                }
            });
        });
    });
}

/// Small horizontal 0..1 bar.
fn bar(ui: &mut Ui, p: &Palette, v: f32, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 14.0), Sense::click());
    let track = egui::Rect::from_center_size(rect.center(), vec2(64.0, 6.0));
    ui.painter().rect_filled(track, CornerRadius::same(3), p.raised);
    let mut f = track;
    f.set_width(64.0 * v.clamp(0.0, 1.0));
    ui.painter().rect_filled(f, CornerRadius::same(3), color);
    resp.on_hover_text(format!("{:.0}%", v * 100.0))
}

fn dix_chart(ui: &mut Ui, p: &Palette, f: &VelocityFunction, inv: &[(f32, f32)], tview: Option<(f64, f64)>) {
    if f.picks.len() < 2 {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(8), p.sunken);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Two or more picks draw the velocity curves", egui::FontId::proportional(11.5), p.text_faint);
        return;
    }
    let rms: Vec<[f64; 2]> = f.picks.iter().map(|(t, v)| [*v as f64, *t as f64]).collect();
    let mut step: Vec<[f64; 2]> = Vec::new();
    for (t0, t1, v) in f.interval_velocities() {
        step.push([v as f64, t0 as f64]);
        step.push([v as f64, t1 as f64]);
    }
    let bad: Vec<[f64; 2]> = inv.iter().map(|(t, v)| [*v as f64, *t as f64]).collect();
    let mut plot = Plot::new("dix_plot")
        .height(220.0)
        .invert_y(true)
        .x_axis_label("m/s")
        .y_axis_label("ms")
        .show_background(false)
        .allow_scroll(false)
        .allow_drag(false)
        .show_axes([true, true]);
    if let Some((a, b)) = tview {
        plot = plot.include_y(a).include_y(b);
    }
    plot.show(ui, |pu| {
        if rms.len() >= 2 {
            pu.line(Line::new("RMS", PlotPoints::from(rms.clone())).color(p.pick).width(2.0));
        }
        pu.points(Points::new("picks", PlotPoints::from(rms)).color(p.pick).radius(3.0));
        if step.len() >= 2 {
            pu.line(Line::new("Interval", PlotPoints::from(step)).color(p.info).width(1.8));
        }
        if !bad.is_empty() {
            pu.points(Points::new("check", PlotPoints::from(bad)).color(p.danger).radius(5.0).shape(egui_plot::MarkerShape::Diamond));
        }
    });
    ui.horizontal(|ui| {
        for (c, t) in [(p.pick, "RMS"), (p.info, "Interval")] {
            widgets::dot(ui, c, 3.5);
            ui.label(RichText::new(t).size(11.0).color(p.text_muted));
        }
    });
}

/// Picked locations; click to jump, × to clear.
fn locations_list(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let flagged = s.iva.flagged();
    let mut jump: Option<i32> = None;
    let mut del: Option<i32> = None;
    let any = s.iva.functions.values().any(|v| !v.is_empty());
    if !any {
        return;
    }
    egui::Frame::new().fill(p.sunken).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        egui::ScrollArea::vertical().max_height(160.0).id_salt("iva_locs").show(ui, |ui| {
            for (cdp, picks) in &s.iva.functions {
                if picks.is_empty() {
                    continue;
                }
                let cur = *cdp == s.iva.gather_cdp;
                ui.horizontal(|ui| {
                    let col = if flagged.contains(cdp) { p.pick } else { p.success };
                    widgets::dot(ui, col, 3.0);
                    if ui.selectable_label(cur, mono(format!("CDP {cdp}"), 11.5, if cur { p.accent_text } else { p.text })).clicked() {
                        jump = Some(*cdp);
                    }
                    ui.label(RichText::new(format!("{} picks", picks.len())).size(11.0).color(p.text_muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(icon::X).on_hover_text("Clear this location").clicked() {
                            del = Some(*cdp);
                        }
                    });
                });
            }
        });
    });
    if let Some(c) = jump {
        if let Some(i) = s.iva.index.ranges.iter().position(|r| r.key_value == c as i64) {
            s.iva.pos = i;
        }
    }
    if let Some(c) = del {
        s.iva.functions.remove(&c);
    }
}

/// Compact table list for the Tables panel.
pub fn table_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    locations_list(s, ui, &p);
}
