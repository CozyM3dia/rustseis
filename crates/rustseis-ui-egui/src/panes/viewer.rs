//! Workspace 2: interactive trace display, before/after compare and picking.

use crate::canvas::{self, CanvasOpts, CanvasState, Raster, View2D, badge, clip_amplitude, draw_raster, draw_wiggles};
use crate::data::{
    self, ENSEMBLE_KEYS, LABEL_KEYS, MAX_RASTER_SIDE, RasterExtent, SECTION_KEY, TraceSource, decimated_clone, fmt_count,
    header_f64, render_gather_raster,
};
use crate::state::{AppState, JobPurpose, Level};
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets::{self, icon_button, segmented, vsep};
use egui::{Color32, Pos2, RichText, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use rustseis_core::{MuteMode, MuteTable, Trace};
use rustseis_render::{GatherRenderConfig, SeismicColormap};
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PickMode {
    #[default]
    Navigate,
    Mute,
    FirstBreak,
    Kill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KillMode {
    #[default]
    KillTrace,
    KillChannel,
    ReverseChannel,
}

pub struct ViewerState {
    pub canvas: CanvasState,
    pub canvas_b: CanvasState,
    pub colormap: SeismicColormap,
    pub clip: f32,
    pub wiggles: bool,
    pub va_fill: bool,
    pub wiggle_gain: f32,
    pub label_key: String,
    pub mode: PickMode,
    pub compare: bool,
    pub compare_source: TraceSource,
    pub request_refit: bool,
    raster: Option<Raster>,
    raster_key: u64,
    raster_b: Option<Raster>,
    raster_b_key: u64,
    detail: Option<Raster>,
    detail_key: u64,
    clip_amp: (u64, f32),
    clip_amp_b: (u64, f32),
    last_home_key: u64,
    // Mute editing
    pub mute_table_name: String,
    pub mute_mode: MuteMode,
    pub mute_key: String,
    pub mute_taper_ms: f32,
    pub mute_picks: Vec<(f32, f32)>,
    pub surgical_picks: Vec<(f32, f32, f32)>,
    pub mute_preview: bool,
    mute_drag: Option<(usize, u8)>,
    // First breaks: (ffid, chan) -> time
    pub fb_picks: HashMap<(i32, i32), f32>,
    pub fb_table_name: String,
    pub fb_params: rustseis_imaging::FirstBreakPickParams,
    pub fb_show_header: bool,
    // Kill / reverse
    pub kill_mode: KillMode,
    pub kill_tracenos: BTreeSet<i32>,
    pub kill_channels: BTreeSet<i32>,
    pub reverse_channels: BTreeSet<i32>,
}

impl Default for ViewerState {
    fn default() -> Self {
        Self {
            canvas: CanvasState::default(),
            canvas_b: CanvasState::default(),
            colormap: SeismicColormap::BlueWhiteRed,
            clip: 98.0,
            wiggles: true,
            va_fill: true,
            wiggle_gain: 1.0,
            label_key: "CHAN".into(),
            mode: PickMode::Navigate,
            compare: false,
            compare_source: TraceSource::Active,
            request_refit: true,
            raster: None,
            raster_key: 0,
            raster_b: None,
            raster_b_key: 0,
            detail: None,
            detail_key: 0,
            clip_amp: (0, 1.0),
            clip_amp_b: (0, 1.0),
            last_home_key: 0,
            mute_table_name: "ui_top_mute".into(),
            mute_mode: MuteMode::Top,
            mute_key: "AOFFSET".into(),
            mute_taper_ms: 40.0,
            mute_picks: Vec::new(),
            surgical_picks: Vec::new(),
            mute_preview: false,
            mute_drag: None,
            fb_picks: HashMap::new(),
            fb_table_name: "ui_fb_picks".into(),
            fb_params: Default::default(),
            fb_show_header: true,
            kill_mode: KillMode::KillTrace,
            kill_tracenos: BTreeSet::new(),
            kill_channels: BTreeSet::new(),
            reverse_channels: BTreeSet::new(),
        }
    }
}

impl ViewerState {
    pub fn mute_table(&self) -> MuteTable {
        MuteTable {
            name: self.mute_table_name.clone(),
            mode: self.mute_mode,
            key_header: self.mute_key.clone(),
            taper_ms: self.mute_taper_ms,
            picks: self.mute_picks.clone(),
            surgical_picks: self.surgical_picks.clone(),
        }
    }
    pub fn load_mute(&mut self, t: &MuteTable) {
        self.mute_table_name = t.name.clone();
        self.mute_mode = t.mode;
        self.mute_key = t.key_header.clone();
        self.mute_taper_ms = t.taper_ms;
        self.mute_picks = t.picks.clone();
        self.surgical_picks = t.surgical_picks.clone();
    }
    pub fn pending_edits(&self) -> usize {
        self.kill_tracenos.len() + self.kill_channels.len() + self.reverse_channels.len()
    }
}

fn hash_of(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

pub fn colormap_name(c: SeismicColormap) -> &'static str {
    match c {
        SeismicColormap::BlueWhiteRed => "BWR",
        SeismicColormap::Greyscale => "Grey",
        SeismicColormap::Semblance => "Heat",
    }
}

/// Owned raster result from the compute pool.
pub struct RasterResult {
    pub img: egui::ColorImage,
    pub extent: RasterExtent,
    pub key: u64,
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    s.ensure_index();
    toolbar(s, ui, &p);
    if super::busy_notice(s, ui) && s.viewer.raster.is_none() {
        return;
    }
    let Some(range) = s.focus_range() else {
        canvas::empty_state(ui, &p, icon::WAVEFORM, "No traces", "Open SEG-Y / SEG-D data or run a flow (Flow Studio → Run)");
        return;
    };
    // Snapshot of what we need from the session (None while the engine is busy: use cached).
    let busy = s.is_busy();
    ensure_rasters(s, ui.ctx(), range);

    let avail = ui.available_size();
    if s.viewer.compare {
        let w = (avail.x - 6.0) * 0.5;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.vertical(|ui| {
                ui.set_width(w);
                gather_canvas(s, ui, &p, vec2(w, avail.y), range, false, busy);
            });
            ui.vertical(|ui| {
                ui.set_width(w);
                gather_canvas(s, ui, &p, vec2(w, avail.y), range, true, busy);
            });
        });
        // Keep B synced to A.
        s.viewer.canvas_b.view = s.viewer.canvas.view;
    } else {
        gather_canvas(s, ui, &p, avail, range, false, busy);
    }
}

fn toolbar(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let sources = s.session().map(TraceSource::all).unwrap_or_else(|| vec![s.focus.source.clone()]);
    widgets::toolbar(ui, p, |ui| {
        egui::ComboBox::from_id_salt("viewer_source")
            .selected_text(RichText::new(s.focus.source.label()).size(12.0))
            .width(150.0)
            .show_ui(ui, |ui| {
                for src in &sources {
                    if ui.selectable_label(*src == s.focus.source, src.label()).clicked() {
                        s.focus.source = src.clone();
                        s.focus.ens = 0;
                        s.viewer.request_refit = true;
                    }
                }
            });
        egui::ComboBox::from_id_salt("viewer_key")
            .selected_text(RichText::new(if s.focus.key == SECTION_KEY { "Whole section".to_string() } else { format!("by {}", s.focus.key) }).size(12.0))
            .width(110.0)
            .show_ui(ui, |ui| {
                for k in ENSEMBLE_KEYS {
                    let label = if *k == SECTION_KEY { "Whole section".to_string() } else { format!("Ensemble by {k}") };
                    if ui.selectable_label(s.focus.key == *k, label).clicked() {
                        s.focus.key = k.to_string();
                        s.focus.ens = 0;
                        s.viewer.request_refit = true;
                    }
                }
            });
        let n = s.index.ranges.len();
        if icon_button(ui, p, icon::CARET_LEFT, "Previous ensemble ( [ )", false).clicked() {
            s.step_ensemble(-1);
        }
        let mut e = s.focus.ens + 1;
        let r = ui.add(egui::DragValue::new(&mut e).range(1..=n.max(1)).speed(0.25).prefix("# "));
        if r.changed() {
            s.focus.ens = e.saturating_sub(1);
        }
        if icon_button(ui, p, icon::CARET_RIGHT, "Next ensemble ( ] )", false).clicked() {
            s.step_ensemble(1);
        }
        if let Some(rg) = s.focus_range() {
            let key = if s.focus.key == SECTION_KEY { "section".to_string() } else { format!("{} {}", s.focus.key, rg.key_value) };
            ui.label(RichText::new(format!("{key} · {} tr · of {}", fmt_count(rg.len()), fmt_count(n))).font(font_mono(11.0)).color(p.text_muted));
        }
        vsep(ui, p);
        segmented(
            ui,
            p,
            &mut s.viewer.colormap,
            &[
                (SeismicColormap::BlueWhiteRed, "BWR", "Blue-white-red variable density"),
                (SeismicColormap::Greyscale, "Grey", "Greyscale variable density"),
                (SeismicColormap::Semblance, "Heat", "Heat / semblance palette"),
            ],
        );
        ui.add(egui::Slider::new(&mut s.viewer.clip, 90.0..=100.0).text("clip %").max_decimals(1).step_by(0.5));
        if icon_button(ui, p, icon::WAVE_SINE, "Wiggle overlay (vector, VA fill)", s.viewer.wiggles).clicked() {
            s.viewer.wiggles = !s.viewer.wiggles;
        }
        if s.viewer.wiggles {
            ui.add(egui::DragValue::new(&mut s.viewer.wiggle_gain).range(0.1..=8.0).speed(0.02).prefix("gain ×").max_decimals(2));
        }
        egui::ComboBox::from_id_salt("viewer_label")
            .selected_text(RichText::new(format!("x: {}", s.viewer.label_key)).size(12.0))
            .width(90.0)
            .show_ui(ui, |ui| {
                for k in LABEL_KEYS {
                    ui.selectable_value(&mut s.viewer.label_key, k.to_string(), *k);
                }
            });
        vsep(ui, p);
        if icon_button(ui, p, icon::COLUMNS, "Before / after split compare", s.viewer.compare).clicked() {
            s.viewer.compare = !s.viewer.compare;
            if s.viewer.compare {
                if let Some(first_buf) = sources.iter().find(|x| **x != s.focus.source) {
                    s.viewer.compare_source = first_buf.clone();
                }
            }
        }
        if s.viewer.compare {
            egui::ComboBox::from_id_salt("viewer_cmp")
                .selected_text(RichText::new(format!("B: {}", s.viewer.compare_source.label())).size(12.0))
                .width(150.0)
                .show_ui(ui, |ui| {
                    for src in &sources {
                        ui.selectable_value(&mut s.viewer.compare_source, src.clone(), src.label());
                    }
                });
            if icon_button(ui, p, icon::DOWNLOAD, "Export side-by-side PNG (render_side_by_side)", false).clicked() {
                export_side_by_side(s);
            }
        }
        vsep(ui, p);
        let before = s.viewer.mode;
        segmented(
            ui,
            p,
            &mut s.viewer.mode,
            &[
                (PickMode::Navigate, "Navigate", "Pan / zoom; hover updates the crosshair bus; click commits it"),
                (PickMode::Mute, "Mute", "Click to add, drag to move, right-click to delete mute picks"),
                (PickMode::FirstBreak, "First break", "Click or drag to pick; right-drag erases"),
                (PickMode::Kill, "Kill / Rev", "Click traces to toggle kill / reverse"),
            ],
        );
        if before != s.viewer.mode && s.viewer.mode != PickMode::Navigate {
            s.workspace_request = None;
        }
        if icon_button(ui, p, icon::CORNERS_OUT, "Fit view (double-click canvas)", false).clicked() {
            s.viewer.canvas.reset();
        }
    });
}

fn ensure_rasters(s: &mut AppState, ctx: &egui::Context, range: data::EnsRange) {
    if s.session().is_none() {
        return;
    }
    let v = &s.viewer;
    let mute_hash = if v.mute_preview && v.mode == PickMode::Mute {
        hash_of((&v.mute_picks.iter().map(|(a, b)| (a.to_bits(), b.to_bits())).collect::<Vec<_>>(), v.mute_taper_ms.to_bits(), v.mute_key.as_str(), v.mute_mode as u8))
    } else {
        0
    };
    let key = hash_of((s.rev, &s.focus.source, s.focus.key.as_str(), range.start, range.end, v.colormap as u8, v.clip.to_bits(), mute_hash));

    // ---- primary raster (A)
    if key != s.viewer.raster_key {
        let (cmap, clip) = (s.viewer.colormap, s.viewer.clip);
        let table = (mute_hash != 0).then(|| s.viewer.mute_table());
        let (job_traces, extent, amp) = {
            let session = s.session().expect("checked");
            let traces = &s.focus.source.traces(session)[range.start..range.end];
            let mut job = decimated_clone(traces, 0, traces.len(), MAX_RASTER_SIDE);
            if let Some(t) = &table {
                rustseis_dsp::apply_mute(&mut job, t);
            }
            (job, gather_extent(traces), clip_amplitude(traces, clip))
        };
        s.viewer.raster_key = key;
        s.viewer.detail = None;
        s.viewer.detail_key = 0;
        s.viewer.clip_amp = (key, amp);
        let cells = job_traces.len() * job_traces.first().map(|t| t.samples.len()).unwrap_or(0);
        if cells <= 2_500_000 {
            match render_gather_raster(&job_traces, cmap, clip, MAX_RASTER_SIDE) {
                Ok(img) => set_raster(&mut s.viewer.raster, ctx, "viewer_a", img, extent),
                Err(e) => s.log(Level::Warn, format!("render: {e}")),
            }
        } else {
            s.compute.submit("viewer.raster", move || {
                render_gather_raster(&job_traces, cmap, clip, MAX_RASTER_SIDE).map(|img| RasterResult { img, extent, key })
            });
        }
    }

    // ---- home view / refit
    let (home, home_key) = {
        let session = s.session().expect("checked");
        let traces = &s.focus.source.traces(session)[range.start..range.end];
        let (dt, ns) = data::time_axis_ms(traces);
        let home = View2D::new(0.0, traces.len() as f64, -0.5 * dt as f64, (ns as f64 - 0.5) * dt as f64);
        (home, hash_of((s.rev, &s.focus.source, s.focus.key.as_str(), traces.len(), ns)))
    };
    let refit = s.viewer.request_refit || home_key != s.viewer.last_home_key;
    s.viewer.canvas.set_home(home, refit);
    s.viewer.canvas_b.set_home(home, refit);
    s.viewer.last_home_key = home_key;
    s.viewer.request_refit = false;

    // ---- compare raster (B)
    if s.viewer.compare {
        let src_b = s.viewer.compare_source.clone();
        let (cmap, clip) = (s.viewer.colormap, s.viewer.clip);
        let b_range = {
            let session = s.session().expect("checked");
            matching_range(s, src_b.traces(session), range)
        };
        let key_b = hash_of((s.rev, &src_b, b_range, cmap as u8, clip.to_bits()));
        if key_b != s.viewer.raster_b_key {
            s.viewer.raster_b_key = key_b;
            match b_range {
                Some((a, b)) => {
                    let (job, extent, amp) = {
                        let session = s.session().expect("checked");
                        let sub = &src_b.traces(session)[a..b];
                        (decimated_clone(sub, 0, sub.len(), MAX_RASTER_SIDE), gather_extent(sub), clip_amplitude(sub, clip))
                    };
                    s.viewer.clip_amp_b = (key_b, amp);
                    s.compute.submit("viewer.raster_b", move || {
                        render_gather_raster(&job, cmap, clip, MAX_RASTER_SIDE).map(|img| RasterResult { img, extent, key: key_b })
                    });
                }
                None => s.viewer.raster_b = None,
            }
        }
    }

    // ---- detail refine for sections wider than one texture
    if range.len() > MAX_RASTER_SIDE {
        if let Some(view) = s.viewer.canvas.view {
            let idle = s.time - s.viewer.canvas.last_interaction > 0.25;
            let i0 = view.x0.floor().max(0.0) as usize;
            let i1 = (view.x1.ceil().max(0.0) as usize).min(range.len());
            let visible = i1.saturating_sub(i0);
            if idle && visible > 0 && visible < range.len() * 3 / 4 {
                let dkey = hash_of((s.viewer.raster_key, i0 / 16, i1 / 16));
                if dkey != s.viewer.detail_key {
                    s.viewer.detail_key = dkey;
                    let (lo, hi) = (i0.saturating_sub(visible / 8), (i1 + visible / 8).min(range.len()));
                    let job = {
                        let session = s.session().expect("checked");
                        let traces = &s.focus.source.traces(session)[range.start..range.end];
                        decimated_clone(traces, lo, hi, 2400)
                    };
                    let (cmap, clip) = (s.viewer.colormap, s.viewer.clip);
                    let extent = RasterExtent { x0: lo as f64, x1: hi as f64, y0: home.y0, y1: home.y1 };
                    s.compute.submit("viewer.detail", move || {
                        render_gather_raster(&job, cmap, clip, MAX_RASTER_SIDE).map(|img| RasterResult { img, extent, key: dkey })
                    });
                }
            }
        }
    }
}

fn set_raster(slot: &mut Option<Raster>, ctx: &egui::Context, name: &str, img: egui::ColorImage, extent: RasterExtent) {
    match slot {
        Some(r) => r.replace(img, extent),
        None => *slot = Some(Raster::new(ctx, name, img, extent)),
    }
}

/// Route compute results back into viewer state.
pub fn on_compute(s: &mut AppState, ctx: &egui::Context, key: &str, value: Box<dyn std::any::Any + Send>) {
    match key {
        "viewer.raster" | "viewer.raster_b" | "viewer.detail" => {
            let Ok(r) = value.downcast::<RasterResult>() else { return };
            let r = *r;
            match key {
                "viewer.raster" if r.key == s.viewer.raster_key => set_raster(&mut s.viewer.raster, ctx, "viewer_a", r.img, r.extent),
                "viewer.raster_b" if r.key == s.viewer.raster_b_key => set_raster(&mut s.viewer.raster_b, ctx, "viewer_b", r.img, r.extent),
                "viewer.detail" if r.key == s.viewer.detail_key => set_raster(&mut s.viewer.detail, ctx, "viewer_detail", r.img, r.extent),
                _ => {}
            }
        }
        "viewer.fb_auto" => {
            if let Ok(t) = value.downcast::<rustseis_core::FirstBreakTable>() {
                let n = t.picks.len();
                for pk in t.picks {
                    if pk.time_ms > 0.0 {
                        s.viewer.fb_picks.insert((pk.ffid, pk.chan), pk.time_ms);
                    }
                }
                s.toast(Level::Ok, format!("Auto-picked {n} first breaks"));
            }
        }
        _ => {}
    }
}

/// Trace columns span [i, i+1]; samples span [t - dt/2, t + dt/2].
pub fn gather_extent(traces: &[Trace]) -> RasterExtent {
    let (dt, ns) = data::time_axis_ms(traces);
    RasterExtent { x0: 0.0, x1: traces.len() as f64, y0: -0.5 * dt as f64, y1: (ns as f64 - 0.5) * dt as f64 }
}

fn matching_range(s: &AppState, b: &[Trace], a_range: data::EnsRange) -> Option<(usize, usize)> {
    if b.is_empty() {
        return None;
    }
    if s.focus.key == SECTION_KEY {
        return Some((0, b.len()));
    }
    let g = data::header_getter(&s.focus.key);
    let start = b.iter().position(|t| g(t) == a_range.key_value)?;
    let end = b[start..].iter().position(|t| g(t) != a_range.key_value).map(|e| start + e).unwrap_or(b.len());
    Some((start, end))
}

fn export_side_by_side(s: &mut AppState) {
    let Some(session) = s.session() else { return };
    let Some(range) = s.focus_range() else { return };
    let a = &s.focus.source.traces(session)[range.start..range.end];
    let b_all = s.viewer.compare_source.traces(session);
    let Some((b0, b1)) = matching_range(s, b_all, range) else {
        s.toast(Level::Warn, "Compare source has no matching ensemble");
        return;
    };
    let cfg = GatherRenderConfig { width: 1800, height: 900, colormap: s.viewer.colormap, clip_percentile: s.viewer.clip, draw_wiggles: true, max_time_ms: None };
    let result = rustseis_render::render_side_by_side(a, &b_all[b0..b1], &cfg);
    match result {
        Ok(img) => {
            if let Some(path) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name("before_after.png").save_file() {
                match img.save_png(&path) {
                    Ok(()) => s.toast(Level::Ok, format!("Saved {}", path.display())),
                    Err(e) => s.toast(Level::Error, e.to_string()),
                }
            }
        }
        Err(e) => s.toast(Level::Error, e.to_string()),
    }
}

fn gather_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2, range: data::EnsRange, is_b: bool, busy: bool) {
    // Labels need header access; take a light copy of label values for visible ticks.
    let label_key = s.viewer.label_key.clone();
    let labels: Vec<f64> = s
        .session()
        .map(|sess| {
            let tr = if is_b {
                let all = s.viewer.compare_source.traces(sess);
                matching_range(s, all, range).map(|(a, b)| &all[a..b]).unwrap_or(&[])
            } else {
                &s.focus.source.traces(sess)[range.start..range.end]
            };
            // Sampled labels (every trace up to 20k, else decimated) — cheap and only on this frame.
            let step = (tr.len() / 20_000).max(1);
            tr.iter().step_by(step).map(|t| header_f64(t, &label_key)).collect()
        })
        .unwrap_or_default();
    let label_step = ((range.len() / 20_000).max(1)) as f64;
    let mut opts = CanvasOpts::seismic(label_key.clone());
    opts.background = p.canvas;
    opts.primary_pans = s.viewer.mode == PickMode::Navigate;
    opts.x_label = Box::new(move |x| {
        let i = (x / label_step).floor() as isize;
        if i < 0 {
            return None;
        }
        labels.get(i as usize).map(|v| if v.fract().abs() < 1e-6 { format!("{v:.0}") } else { format!("{v:.1}") })
    });
    let id = egui::Id::unique(("viewer_canvas", is_b));
    let raster = if is_b { s.viewer.raster_b.as_ref() } else { s.viewer.raster.as_ref() };
    let detail = if is_b { None } else { s.viewer.detail.as_ref() };
    let state = if is_b { &mut s.viewer.canvas_b } else { &mut s.viewer.canvas };
    let out = canvas::canvas(ui, id, state, p, size, opts, |painter, frame| {
        if let Some(r) = raster {
            draw_raster(painter, frame, r, Color32::WHITE);
        }
        if let Some(d) = detail {
            draw_raster(painter, frame, d, Color32::WHITE);
        }
    });

    // Vector wiggles + overlays need live traces.
    let painter = ui.painter_at(out.frame.rect);
    if let Some(sess) = s.session() {
        let traces: &[Trace] = if is_b {
            let all = s.viewer.compare_source.traces(sess);
            matching_range(s, all, range).map(|(a, b)| &all[a..b]).unwrap_or(&[])
        } else {
            &s.focus.source.traces(sess)[range.start..range.end]
        };
        if s.viewer.wiggles {
            let clip = if is_b { s.viewer.clip_amp_b.1 } else { s.viewer.clip_amp.1 };
            let line = if s.viewer.colormap == SeismicColormap::Greyscale { Color32::from_rgb(10, 30, 60) } else { Color32::from_rgba_unmultiplied(12, 14, 20, 230) };
            let fill = s.viewer.va_fill.then_some(Color32::from_rgba_unmultiplied(12, 14, 20, 190));
            draw_wiggles(&painter, &out.frame, traces, 0.0, clip, s.viewer.wiggle_gain, line, fill);
        }
        if !is_b {
            overlays(s, &painter, &out.frame, traces, p);
        }
    }
    // Crosshair bus (shared selection) line.
    if let Some(ti) = s.crosshair.trace_index {
        if !is_b && ti >= range.start && ti < range.end {
            let x = out.frame.x_to_screen((ti - range.start) as f64 + 0.5);
            painter.vline(x, out.frame.rect.y_range(), Stroke::new(1.0, with_alpha(p.accent, 200)));
        }
    }
    if let Some(t) = s.crosshair.time_ms {
        let y = out.frame.y_to_screen(t as f64);
        painter.hline(out.frame.rect.x_range(), y, Stroke::new(1.0, with_alpha(p.accent, 120)));
    }

    let title = if is_b {
        format!("B · {}", s.viewer.compare_source.label())
    } else if s.viewer.compare {
        format!("A · {}", s.focus.source.label())
    } else {
        let key = if s.focus.key == SECTION_KEY { "section".into() } else { format!("{} {}", s.focus.key, range.key_value) };
        format!("{key} · {} traces", fmt_count(range.len()))
    };
    badge(&painter, out.frame.rect.left_top() + vec2(6.0, 6.0), &title, p, egui::Align2::LEFT_TOP);
    if busy {
        badge(&painter, out.frame.rect.right_top() + vec2(-6.0, 6.0), "engine busy · cached view", p, egui::Align2::RIGHT_TOP);
    }
    if is_b && s.viewer.raster_b.is_none() {
        painter.text(out.frame.rect.center(), egui::Align2::CENTER_CENTER, "No matching ensemble in B", egui::FontId::proportional(13.0), p.text_muted);
    }
    if !is_b {
        interact(s, &out, range);
    }
}

fn mute_curve_times(v: &ViewerState, traces: &[Trace]) -> Vec<Option<f32>> {
    let table = v.mute_table();
    traces
        .iter()
        .map(|t| {
            let k = header_f64(t, &v.mute_key) as f32;
            match v.mute_mode {
                MuteMode::Surgical => table.surgical_window_at(k).map(|w| w.0),
                _ => table.time_at(k),
            }
        })
        .collect()
}

fn overlays(s: &AppState, painter: &egui::Painter, frame: &canvas::PlotFrame, traces: &[Trace], p: &Palette) {
    let v = &s.viewer;
    let x_of = |i: usize| frame.x_to_screen(i as f64 + 0.5);
    let visible = |i: usize| (i as f64 + 1.0) >= frame.view.x0 && (i as f64) <= frame.view.x1;
    let n = traces.len();
    let step = ((n as f32 / frame.rect.width().max(1.0)).ceil() as usize).max(1);

    // Mute curve + shaded region
    if !v.mute_picks.is_empty() || !v.surgical_picks.is_empty() {
        let table = v.mute_table();
        let mut top_pts = Vec::new();
        let mut bot_pts = Vec::new();
        for i in (0..n).step_by(step) {
            if !visible(i) {
                continue;
            }
            let k = header_f64(&traces[i], &v.mute_key) as f32;
            let x = x_of(i);
            match v.mute_mode {
                MuteMode::Surgical => {
                    if let Some((a, b)) = table.surgical_window_at(k) {
                        top_pts.push(pos2(x, frame.y_to_screen(a as f64)));
                        bot_pts.push(pos2(x, frame.y_to_screen(b as f64)));
                    }
                }
                _ => {
                    if let Some(t) = table.time_at(k) {
                        top_pts.push(pos2(x, frame.y_to_screen(t as f64)));
                    }
                }
            }
        }
        let shade = with_alpha(p.pick, 34);
        let edge = frame.rect;
        for w in top_pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let rect_poly = match v.mute_mode {
                MuteMode::Top => vec![pos2(a.x, edge.top()), pos2(b.x, edge.top()), b, a],
                MuteMode::Bottom => vec![a, b, pos2(b.x, edge.bottom()), pos2(a.x, edge.bottom())],
                MuteMode::Surgical => Vec::new(),
            };
            if !rect_poly.is_empty() {
                painter.add(egui::Shape::convex_polygon(rect_poly, shade, Stroke::NONE));
            }
        }
        if v.mute_mode == MuteMode::Surgical {
            for (wa, wb) in top_pts.windows(2).zip(bot_pts.windows(2)) {
                painter.add(egui::Shape::convex_polygon(vec![wa[0], wa[1], wb[1], wb[0]], shade, Stroke::NONE));
            }
            painter.add(egui::Shape::line(bot_pts, Stroke::new(1.5, p.pick)));
        }
        painter.add(egui::Shape::line(top_pts, Stroke::new(2.0, p.pick)));
        // Control points at the trace whose key is nearest each pick.
        if v.mode == PickMode::Mute {
            let keys: Vec<f32> = traces.iter().map(|t| header_f64(t, &v.mute_key) as f32).collect();
            let handles: Vec<(f32, Vec<f32>)> = match v.mute_mode {
                MuteMode::Surgical => v.surgical_picks.iter().map(|(k, a, b)| (*k, vec![*a, *b])).collect(),
                _ => v.mute_picks.iter().map(|(k, t)| (*k, vec![*t])).collect(),
            };
            for (k, ts) in handles {
                if let Some(i) = nearest_key(&keys, k) {
                    for t in ts {
                        let c = pos2(x_of(i), frame.y_to_screen(t as f64));
                        painter.circle(c, 4.5, p.chrome, Stroke::new(2.0, p.pick));
                    }
                }
            }
        }
    }

    // First breaks: picked (bright) and header FB_PICK (dim).
    let mut fb_line = Vec::new();
    for i in (0..n).step_by(step) {
        if !visible(i) {
            continue;
        }
        let h = &traces[i].headers;
        let picked = v.fb_picks.get(&(h.ffid, h.chan)).copied();
        let t = picked.or_else(|| (v.fb_show_header && h.fb_pick > 0.0).then_some(h.fb_pick));
        if let Some(t) = t {
            let c = pos2(x_of(i), frame.y_to_screen(t as f64));
            if picked.is_some() {
                fb_line.push(c);
                if frame.px_per_x() > 5.0 {
                    painter.circle_filled(c, 2.5, p.pick);
                }
            } else {
                painter.circle_filled(c, 1.6, with_alpha(p.info, 200));
            }
        } else if fb_line.len() > 1 {
            painter.add(egui::Shape::line(std::mem::take(&mut fb_line), Stroke::new(1.5, with_alpha(p.pick, 220))));
        } else {
            fb_line.clear();
        }
    }
    if fb_line.len() > 1 {
        painter.add(egui::Shape::line(fb_line, Stroke::new(1.5, with_alpha(p.pick, 220))));
    }

    // Pending kills / reversals.
    if v.pending_edits() > 0 {
        for i in (0..n).step_by(step) {
            if !visible(i) {
                continue;
            }
            let h = &traces[i].headers;
            let killed = v.kill_tracenos.contains(&h.traceno) || v.kill_channels.contains(&h.chan);
            let reversed = v.reverse_channels.contains(&h.chan);
            if killed || reversed {
                let x0 = frame.x_to_screen(i as f64);
                let x1 = frame.x_to_screen(i as f64 + 1.0).max(x0 + 1.5);
                let r = egui::Rect::from_x_y_ranges(x0..=x1, frame.rect.y_range());
                let c = if killed { with_alpha(p.danger, 70) } else { with_alpha(p.info, 60) };
                painter.rect_filled(r, 0.0, c);
                if x1 - x0 > 9.0 {
                    painter.text(pos2((x0 + x1) * 0.5, frame.rect.top() + 14.0), egui::Align2::CENTER_CENTER, if killed { icon::X } else { icon::SWAP }, egui::FontId::proportional(11.0), Color32::WHITE);
                }
            }
        }
    }
}

fn nearest_key(keys: &[f32], k: f32) -> Option<usize> {
    keys.iter().enumerate().min_by(|a, b| (a.1 - k).abs().total_cmp(&(b.1 - k).abs())).map(|(i, _)| i)
}

fn interact(s: &mut AppState, out: &canvas::CanvasOutput, range: data::EnsRange) {
    let Some((hx, ht)) = out.hover else {
        if !out.response.dragged() {
            s.viewer.mute_drag = None;
        }
        return;
    };
    let local = hx.floor();
    let n = range.len();
    let in_trace = local >= 0.0 && (local as usize) < n;
    let li = (local.max(0.0) as usize).min(n.saturating_sub(1));
    let abs = range.start + li;
    if in_trace {
        s.hover_trace(abs, Some(ht as f32));
    }
    let resp = &out.response;
    match s.viewer.mode {
        PickMode::Navigate => {
            if resp.clicked() && in_trace {
                s.focus_trace(abs);
            }
        }
        PickMode::Mute => mute_interact(s, out, range, li, ht as f32),
        PickMode::FirstBreak => {
            let Some(sess) = s.session() else { return };
            let t = &s.focus.source.traces(sess)[abs];
            let key = (t.headers.ffid, t.headers.chan);
            let primary = resp.is_pointer_button_down_on() && resp.ctx.input(|i| i.pointer.primary_down());
            let secondary = resp.ctx.input(|i| i.pointer.secondary_down()) && resp.hovered();
            if in_trace && (primary || resp.clicked()) {
                s.viewer.fb_picks.insert(key, ht.max(0.0) as f32);
            } else if in_trace && (secondary || resp.secondary_clicked()) {
                s.viewer.fb_picks.remove(&key);
            }
        }
        PickMode::Kill => {
            if resp.clicked() && in_trace {
                let Some(sess) = s.session() else { return };
                let h = s.focus.source.traces(sess)[abs].headers.clone();
                let v = &mut s.viewer;
                let toggle = |set: &mut BTreeSet<i32>, k: i32| {
                    if !set.remove(&k) {
                        set.insert(k);
                    }
                };
                match v.kill_mode {
                    KillMode::KillTrace => toggle(&mut v.kill_tracenos, h.traceno),
                    KillMode::KillChannel => toggle(&mut v.kill_channels, h.chan),
                    KillMode::ReverseChannel => toggle(&mut v.reverse_channels, h.chan),
                }
            }
        }
    }
}

fn mute_interact(s: &mut AppState, out: &canvas::CanvasOutput, range: data::EnsRange, li: usize, t: f32) {
    let Some(sess) = s.session() else { return };
    let traces = &s.focus.source.traces(sess)[range.start..range.end];
    let key_vals: Vec<f32> = traces.iter().map(|tr| header_f64(tr, &s.viewer.mute_key) as f32).collect();
    let k_here = key_vals.get(li).copied().unwrap_or(0.0);
    let frame = &out.frame;
    let pointer: Option<Pos2> = out.response.hover_pos();
    let v = &mut s.viewer;
    // Hit-test existing handles in screen space.
    let handle_pos = |k: f32, tt: f32| -> Option<Pos2> {
        nearest_key(&key_vals, k).map(|i| pos2(frame.x_to_screen(i as f64 + 0.5), frame.y_to_screen(tt as f64)))
    };
    let mut hit: Option<(usize, u8)> = None;
    if let Some(pt) = pointer {
        let mut best = 9.0f32;
        match v.mute_mode {
            MuteMode::Surgical => {
                for (i, (k, a, b)) in v.surgical_picks.iter().enumerate() {
                    for (which, tt) in [(0u8, *a), (1u8, *b)] {
                        if let Some(hp) = handle_pos(*k, tt) {
                            let d = hp.distance(pt);
                            if d < best {
                                best = d;
                                hit = Some((i, which));
                            }
                        }
                    }
                }
            }
            _ => {
                for (i, (k, tt)) in v.mute_picks.iter().enumerate() {
                    if let Some(hp) = handle_pos(*k, *tt) {
                        let d = hp.distance(pt);
                        if d < best {
                            best = d;
                            hit = Some((i, 0));
                        }
                    }
                }
            }
        }
    }
    let resp = &out.response;
    if resp.drag_started_by(egui::PointerButton::Primary) {
        v.mute_drag = hit;
    }
    if resp.dragged_by(egui::PointerButton::Primary) {
        if let Some((i, which)) = v.mute_drag {
            match v.mute_mode {
                MuteMode::Surgical => {
                    if let Some(pk) = v.surgical_picks.get_mut(i) {
                        pk.0 = k_here;
                        if which == 0 { pk.1 = t.max(0.0) } else { pk.2 = t.max(pk.1 + 1.0) }
                    }
                }
                _ => {
                    if let Some(pk) = v.mute_picks.get_mut(i) {
                        *pk = (k_here, t.max(0.0));
                    }
                }
            }
        }
    }
    if resp.drag_stopped() {
        v.mute_drag = None;
        v.mute_picks.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.surgical_picks.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    if resp.clicked() && hit.is_none() {
        match v.mute_mode {
            MuteMode::Surgical => v.surgical_picks.push((k_here, t.max(0.0), t.max(0.0) + 120.0)),
            _ => v.mute_picks.push((k_here, t.max(0.0))),
        }
        v.mute_picks.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.surgical_picks.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    if resp.secondary_clicked() {
        if let Some((i, _)) = hit {
            match v.mute_mode {
                MuteMode::Surgical => {
                    v.surgical_picks.remove(i);
                }
                _ => {
                    v.mute_picks.remove(i);
                }
            }
        }
    }
}

/// Save the edited mute table through the command bus.
pub fn save_mute(s: &mut AppState) {
    let t = s.viewer.mute_table();
    let name = t.name.clone();
    match s.dispatch_now(rustseis_engine::EngineCommand::UpsertMuteTable(t)) {
        Ok(_) => s.toast(Level::Ok, format!("UpsertMuteTable · {name}")),
        Err(e) => s.toast(Level::Error, e),
    }
}

/// Build a FirstBreakTable from the edited picks (geometry from the focus source) and upsert it.
pub fn save_first_breaks(s: &mut AppState) {
    let Some(sess) = s.session() else { return };
    let traces = s.focus.source.traces(sess);
    let mut picks = Vec::with_capacity(s.viewer.fb_picks.len());
    for t in traces {
        let h = &t.headers;
        if let Some(&tm) = s.viewer.fb_picks.get(&(h.ffid, h.chan)) {
            picks.push(rustseis_core::FirstBreakPick { ffid: h.ffid, chan: h.chan, sou_sloc: h.sou_sloc, rec_sloc: h.rec_sloc, offset_m: h.offset, time_ms: tm, snr: 0.0 });
        }
    }
    let n = picks.len();
    let table = rustseis_core::FirstBreakTable { name: s.viewer.fb_table_name.clone(), picks };
    match s.dispatch_now(rustseis_engine::EngineCommand::UpsertFirstBreakTable(table)) {
        Ok(_) => s.toast(Level::Ok, format!("UpsertFirstBreakTable · {n} picks")),
        Err(e) => s.toast(Level::Error, e),
    }
}

/// Auto-pick first breaks on the focus ensemble (AIC / Modified Coppens) in the background.
pub fn auto_pick_fb(s: &mut AppState) {
    let Some(sess) = s.session() else { return };
    let Some(range) = s.focus_range() else { return };
    let mut work = s.focus.source.traces(sess)[range.start..range.end].to_vec();
    let params = s.viewer.fb_params.clone();
    s.compute.submit("viewer.fb_auto", move || Ok(rustseis_imaging::pick_first_breaks(&mut work, &params, "ui_auto")));
}

/// Apply pending kills / reversals as an undoable `trc_edit` flow on the active traces.
pub fn apply_trace_edits(s: &mut AppState) {
    if s.focus.source != TraceSource::Active {
        s.toast(Level::Warn, "Trace edits apply to the active trace set — switch the viewer source to Active");
        return;
    }
    let v = &s.viewer;
    let params = serde_json::json!({
        "kill_tracenos": v.kill_tracenos.iter().collect::<Vec<_>>(),
        "kill_channels": v.kill_channels.iter().collect::<Vec<_>>(),
        "kill_ffids": [],
        "reverse_channels": v.reverse_channels.iter().collect::<Vec<_>>(),
        "reverse_ffids": [],
    });
    let spec: rustseis_flow::FlowSpec = match serde_json::from_value(serde_json::json!({
        "name": "ui_trace_edit",
        "description": "Interactive kill / reverse from Trace Display",
        "steps": [{"tool": "trc_edit", "params": params}]
    })) {
        Ok(x) => x,
        Err(e) => {
            s.toast(Level::Error, e.to_string());
            return;
        }
    };
    if s.run_flow(spec, JobPurpose::TraceEdit) {
        s.viewer.kill_tracenos.clear();
        s.viewer.kill_channels.clear();
        s.viewer.reverse_channels.clear();
    }
}

pub fn mute_time_preview(s: &AppState) -> Vec<Option<f32>> {
    let Some(sess) = s.session() else { return Vec::new() };
    let Some(range) = s.focus_range() else { return Vec::new() };
    mute_curve_times(&s.viewer, &s.focus.source.traces(sess)[range.start..range.end])
}
