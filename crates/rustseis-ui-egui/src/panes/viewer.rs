//! Gather view: trace display with header tracks, a glass HUD, before/after compare and picking.

use crate::canvas::{self, CanvasOpts, CanvasState, Raster, View2D, badge, clip_amplitude, draw_raster, draw_wiggles};
use crate::data::{
    self, ENSEMBLE_KEYS, LABEL_KEYS, MAX_RASTER_SIDE, RasterExtent, SECTION_KEY, TraceSource, decimated_clone, fmt_count,
    header_f64, render_gather_raster,
};
use crate::state::{AppState, JobPurpose, Level};
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets::{self, caps};
use egui::{Color32, CornerRadius, Pos2, RichText, Stroke, Ui, pos2, vec2};
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
    pub show_tracks: bool,
    pub stats: GatherStats,
    raster: Option<Raster>,
    raster_key: u64,
    raster_b: Option<Raster>,
    raster_b_key: u64,
    detail: Option<Raster>,
    detail_key: u64,
    clip_amp: (u64, f32),
    clip_amp_b: (u64, f32),
    last_home_key: u64,
    labels_cache: [(u64, String, std::sync::Arc<Vec<f64>>); 2],
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
            wiggles: false,
            va_fill: true,
            wiggle_gain: 1.0,
            label_key: "CHAN".into(),
            mode: PickMode::Navigate,
            compare: false,
            compare_source: TraceSource::Active,
            request_refit: true,
            show_tracks: true,
            stats: GatherStats::default(),
            raster: None,
            raster_key: 0,
            raster_b: None,
            raster_b_key: 0,
            detail: None,
            detail_key: 0,
            clip_amp: (0, 1.0),
            clip_amp_b: (0, 1.0),
            last_home_key: 0,
            labels_cache: Default::default(),
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

/// Per-ensemble statistics behind the header tracks, histogram and insight card.
#[derive(Default)]
pub struct GatherStats {
    key: u64,
    pub offsets: Vec<f32>,
    pub energy: Vec<f32>,
    pub dead: Vec<usize>,
    pub spiky: Vec<usize>,
    /// Sorted |amplitude| sample for the histogram and clip mapping.
    pub amp_sorted: Vec<f32>,
    pub hist: Vec<u32>,
    pub hist_max: f32,
}

impl ViewerState {
    /// Something worth pointing out about the gather on screen.
    pub fn has_insight(&self) -> bool {
        !self.stats.dead.is_empty() || !self.stats.spiky.is_empty()
    }
}

fn ensure_stats(s: &mut AppState, range: data::EnsRange) {
    let key = hash_of((s.rev, &s.focus.source, range.start, range.end));
    if key == s.viewer.stats.key {
        return;
    }
    let Some(sess) = s.session() else { return };
    let traces = &s.focus.source.traces(sess)[range.start..range.end];
    let offsets: Vec<f32> = traces.iter().map(|t| t.headers.offset).collect();
    let energy: Vec<f32> = traces
        .iter()
        .map(|t| {
            let n = t.samples.len().max(1) as f32;
            (t.samples.iter().map(|v| v * v).sum::<f32>() / n).sqrt()
        })
        .collect();
    let mut sorted_e: Vec<f32> = energy.iter().copied().filter(|e| *e > 0.0).collect();
    sorted_e.sort_by(f32::total_cmp);
    let median = sorted_e.get(sorted_e.len() / 2).copied().unwrap_or(0.0);
    let dead: Vec<usize> = energy.iter().enumerate().filter(|(_, e)| **e <= median * 1e-4).map(|(i, _)| i).collect();
    let spiky: Vec<usize> = if traces.len() >= 8 { energy.iter().enumerate().filter(|(_, e)| **e > median * 12.0).map(|(i, _)| i).collect() } else { Vec::new() };
    // Amplitude sample (≤ 40k values).
    let total: usize = traces.iter().map(|t| t.samples.len()).sum();
    let stride = (total / 40_000).max(1);
    let mut amps: Vec<f32> = traces.iter().flat_map(|t| t.samples.iter()).step_by(stride).map(|v| v.abs()).filter(|v| v.is_finite()).collect();
    amps.sort_by(f32::total_cmp);
    let hist_max = amps.get((amps.len() as f32 * 0.999) as usize).copied().unwrap_or(1.0).max(1e-12);
    let mut hist = vec![0u32; 56];
    for a in &amps {
        let b = ((a / hist_max) * hist.len() as f32) as usize;
        if b < hist.len() {
            hist[b] += 1;
        }
    }
    s.viewer.stats = GatherStats { key, offsets, energy, dead, spiky, amp_sorted: amps, hist, hist_max };
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    s.ensure_index();
    if super::busy_notice(s, ui) && s.viewer.raster.is_none() {
        return;
    }
    let Some(range) = s.focus_range() else {
        canvas::empty_state(ui, &p, icon::WAVEFORM, "Nothing to look at yet", "Open a SEG-Y or SEG-D file, or run a flow. Results show up here.");
        return;
    };
    let busy = s.is_busy();
    ensure_rasters(s, ui.ctx(), range);
    ensure_stats(s, range);

    let full = ui.available_rect_before_wrap();
    let section = s.focus.key == SECTION_KEY;
    let tracks_h = if s.viewer.show_tracks && !section { 44.0 } else { 0.0 };
    let stepper_h = if s.index.ranges.len() > 1 { 58.0 } else { 0.0 };
    let body = egui::Rect::from_min_max(pos2(full.left(), full.top() + tracks_h), pos2(full.right(), full.bottom() - stepper_h));
    let mut child = |r: egui::Rect| ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));

    let frame_a;
    if s.viewer.compare {
        let w = (body.width() - 6.0) * 0.5;
        let ra = egui::Rect::from_min_size(body.min, vec2(w, body.height()));
        let rb = egui::Rect::from_min_size(pos2(body.left() + w + 6.0, body.top()), vec2(w, body.height()));
        frame_a = gather_canvas(s, &mut child(ra), &p, ra.size(), range, false, busy);
        gather_canvas(s, &mut child(rb), &p, rb.size(), range, true, busy);
        s.viewer.canvas_b.view = s.viewer.canvas.view;
    } else {
        frame_a = gather_canvas(s, &mut child(body), &p, body.size(), range, false, busy);
    }
    if tracks_h > 0.0 {
        let tr = egui::Rect::from_min_max(full.min, pos2(frame_a.rect.right(), full.top() + tracks_h - 2.0));
        header_tracks(s, &mut child(tr), &p, tr, &frame_a);
    }
    hud(s, &mut child(frame_a.rect), &p, frame_a.rect);
    zoom_buttons(s, &mut child(frame_a.rect), &p, frame_a.rect);
    if stepper_h > 0.0 {
        let r = egui::Rect::from_min_max(pos2(full.left(), full.bottom() - stepper_h + 6.0), full.max);
        stepper(s, &mut child(r), &p, r);
    }
    ui.allocate_rect(full, egui::Sense::hover());
}

/// Offset and trace-energy tracks aligned with the gather columns; dead traces in red.
fn header_tracks(s: &mut AppState, ui: &mut Ui, p: &Palette, rect: egui::Rect, frame: &canvas::PlotFrame) {
    let painter = ui.painter_at(rect);
    let st = &s.viewer.stats;
    let n = st.offsets.len();
    if n == 0 {
        return;
    }
    let half = rect.height() * 0.5;
    let rows = [egui::Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + half - 1.0)), egui::Rect::from_min_max(pos2(rect.left(), rect.top() + half + 1.0), rect.max)];
    for (r, label) in rows.iter().zip(["offset", "energy"]) {
        painter.rect_filled(*r, CornerRadius::same(4), p.sunken);
        painter.text(pos2(r.left() + 8.0, r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(10.0), p.text_faint);
    }
    let x_of = |i: usize| frame.x_to_screen(i as f64 + 0.5);
    let step = ((n as f32 / frame.rect.width().max(1.0)).ceil() as usize).max(1);
    let (omin, omax) = st.offsets.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let orng = (omax - omin).max(1e-3);
    let r0 = rows[0].shrink2(vec2(0.0, 3.0));
    let pts: Vec<Pos2> = (0..n).step_by(step).map(|i| pos2(x_of(i), r0.bottom() - (st.offsets[i] - omin) / orng * r0.height())).filter(|q| q.x >= rect.left() + 52.0 && q.x <= rect.right()).collect();
    painter.add(egui::Shape::line(pts, Stroke::new(1.4, p.info)));
    painter.text(pos2(rect.right() - 6.0, rows[0].center().y), egui::Align2::RIGHT_CENTER, format!("{omin:.0} … {omax:.0} m"), font_mono(9.5), p.text_muted);
    let emax = st.energy.iter().copied().fold(0.0f32, f32::max).max(1e-12);
    let r1 = rows[1].shrink2(vec2(0.0, 3.0));
    let bw = (frame.px_per_x() as f32 * step as f32 - 1.0).max(1.0);
    for i in (0..n).step_by(step) {
        let x = x_of(i);
        if x < rect.left() + 52.0 || x > rect.right() {
            continue;
        }
        let dead = st.dead.binary_search(&i).is_ok();
        let spike = st.spiky.binary_search(&i).is_ok();
        let h = if dead { r1.height() } else { (st.energy[i] / emax).sqrt() * r1.height() };
        let col = if dead { p.danger } else if spike { p.pick } else { with_alpha(p.accent, 170) };
        painter.rect_filled(egui::Rect::from_min_max(pos2(x - bw * 0.5, r1.bottom() - h), pos2(x + bw * 0.5, r1.bottom())), CornerRadius::ZERO, col);
    }
    if !st.dead.is_empty() {
        painter.text(pos2(rect.right() - 6.0, rows[1].center().y), egui::Align2::RIGHT_CENTER, format!("{} dead", st.dead.len()), egui::FontId::proportional(10.0), p.danger);
    }
}

/// Glass HUD over the gather: colour map, clip and wiggles.
fn hud(s: &mut AppState, ui: &mut Ui, p: &Palette, rect: egui::Rect) {
    let pos = rect.left_top() + vec2(56.0, 36.0);
    let area = egui::Rect::from_min_size(pos, vec2(420.0, 40.0));
    let mut u = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::left_to_right(egui::Align::Center)));
    widgets::glass_frame(p).show(&mut u, |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (c, tip) in [(SeismicColormap::BlueWhiteRed, "Red and blue"), (SeismicColormap::Greyscale, "Grey"), (SeismicColormap::Semblance, "Heat")] {
            let (r, resp) = ui.allocate_exact_size(vec2(26.0, 16.0), egui::Sense::click());
            let stops: [Color32; 3] = match c {
                SeismicColormap::BlueWhiteRed => [Color32::from_rgb(40, 70, 200), Color32::WHITE, Color32::from_rgb(200, 40, 40)],
                SeismicColormap::Greyscale => [Color32::BLACK, Color32::from_gray(128), Color32::WHITE],
                SeismicColormap::Semblance => [Color32::from_rgb(20, 10, 60), Color32::from_rgb(220, 60, 40), Color32::from_rgb(255, 240, 120)],
            };
            let w = r.width() / 2.0;
            let mut mesh = egui::Mesh::default();
            for k in 0..2 {
                let rr = egui::Rect::from_min_size(pos2(r.left() + w * k as f32, r.top()), vec2(w, r.height()));
                let i = mesh.vertices.len() as u32;
                mesh.colored_vertex(rr.left_top(), stops[k]);
                mesh.colored_vertex(rr.right_top(), stops[k + 1]);
                mesh.colored_vertex(rr.right_bottom(), stops[k + 1]);
                mesh.colored_vertex(rr.left_bottom(), stops[k]);
                mesh.add_triangle(i, i + 1, i + 2);
                mesh.add_triangle(i, i + 2, i + 3);
            }
            ui.painter().add(mesh);
            let on = s.viewer.colormap == c;
            ui.painter().rect_stroke(r.expand(2.0), CornerRadius::same(4), Stroke::new(if on { 1.5 } else { 1.0 }, if on { p.accent } else { with_alpha(p.text, 40) }), egui::StrokeKind::Outside);
            if resp.on_hover_text(tip).clicked() {
                s.viewer.colormap = c;
            }
        }
        ui.add_space(4.0);
        ui.label(RichText::new("clip").size(11.0).color(p.text_muted));
        ui.add(egui::DragValue::new(&mut s.viewer.clip).range(80.0..=100.0).speed(0.05).max_decimals(1).suffix(" %"));
        ui.add_space(4.0);
        ui.label(RichText::new("wiggles").size(11.0).color(p.text_muted));
        widgets::toggle(ui, p, &mut s.viewer.wiggles);
        if s.viewer.wiggles {
            ui.add(egui::DragValue::new(&mut s.viewer.wiggle_gain).range(0.1..=8.0).speed(0.02).prefix("×").max_decimals(1));
        }
    });
}

fn zoom_buttons(s: &mut AppState, ui: &mut Ui, p: &Palette, rect: egui::Rect) {
    let area = egui::Rect::from_min_size(rect.right_bottom() - vec2(44.0, 128.0), vec2(36.0, 112.0));
    let mut u = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Center)));
    widgets::glass_frame(p).inner_margin(egui::Margin::same(3)).show(&mut u, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let mut zoom = |f: f64| {
            if let Some(v) = s.viewer.canvas.view.as_mut() {
                let (cx, cy) = ((v.x0 + v.x1) * 0.5, (v.y0 + v.y1) * 0.5);
                let (hx, hy) = ((v.x1 - v.x0) * 0.5 * f, (v.y1 - v.y0) * 0.5 * f);
                *v = View2D::new(cx - hx, cx + hx, cy - hy, cy + hy);
                s.viewer.canvas.last_interaction = s.time;
            }
        };
        if glass_icon(ui, p, icon::PLUS, "Zoom in").clicked() {
            zoom(0.7);
        }
        if glass_icon(ui, p, icon::MINUS, "Zoom out").clicked() {
            zoom(1.0 / 0.7);
        }
        if glass_icon(ui, p, icon::CORNERS_OUT, "Fit (double-click)").clicked() {
            s.viewer.canvas.reset();
        }
    });
}

fn glass_icon(ui: &mut Ui, p: &Palette, ic: &str, tip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(6), with_alpha(p.text, 20));
    }
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(15.0), p.text);
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Shot stepper and line overview: one bar per ensemble, click or drag to jump.
fn stepper(s: &mut AppState, ui: &mut Ui, p: &Palette, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(10), p.sunken);
    let n = s.index.ranges.len();
    let cur = s.focus.ens.min(n.saturating_sub(1));
    let key_name: String = match s.focus.key.as_str() {
        "FFID" => "Shot".into(),
        "CDP" => "CDP".into(),
        "CHAN" => "Channel".into(),
        "OFFSET" | "AOFFSET" => "Offset".into(),
        k => k.to_string(),
    };
    let kv = s.index.ranges.get(cur).map(|r| r.key_value).unwrap_or(0);
    // Left: stepper
    let left = egui::Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(230.0, rect.height() - 12.0));
    let mut u = ui.new_child(egui::UiBuilder::new().max_rect(left).layout(egui::Layout::left_to_right(egui::Align::Center)));
    u.spacing_mut().item_spacing.x = 4.0;
    if widgets::cmd(&mut u, p, icon::CARET_LEFT, "", false).on_hover_text("Previous  [").clicked() {
        s.step_ensemble(-1);
    }
    u.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(RichText::new(format!("{key_name} {kv}")).font(crate::theme::font_semibold(14.0)).color(p.text));
        ui.label(RichText::new(format!("{} of {} · {} traces", cur + 1, fmt_count(n), s.index.ranges.get(cur).map(|r| r.len()).unwrap_or(0))).size(10.5).color(p.text_muted));
    });
    if widgets::cmd(&mut u, p, icon::CARET_RIGHT, "", false).on_hover_text("Next  ]").clicked() {
        s.step_ensemble(1);
    }
    // Right: overview bars
    let ov = egui::Rect::from_min_max(pos2(left.right() + 12.0, rect.top() + 10.0), pos2(rect.right() - 12.0, rect.bottom() - 10.0));
    let maxlen = s.index.ranges.iter().map(|r| r.len()).max().unwrap_or(1).max(1) as f32;
    let bars = n.min(ov.width() as usize / 2).max(1);
    let bw = ov.width() / bars as f32;
    for b in 0..bars {
        let i0 = b * n / bars;
        let i1 = ((b + 1) * n / bars).max(i0 + 1);
        let len = s.index.ranges[i0..i1.min(n)].iter().map(|r| r.len()).max().unwrap_or(0) as f32;
        let h = (len / maxlen).max(0.08) * ov.height();
        let is_cur = cur >= i0 && cur < i1;
        let col = if is_cur { p.accent } else { with_alpha(p.text_muted, 90) };
        let x = ov.left() + b as f32 * bw;
        painter.rect_filled(egui::Rect::from_min_max(pos2(x + 0.5, ov.bottom() - h), pos2(x + bw - 0.5, ov.bottom())), CornerRadius::same(1), col);
    }
    let cx = ov.left() + (cur as f32 + 0.5) / n as f32 * ov.width();
    painter.vline(cx, rect.y_range().shrink(4.0), Stroke::new(1.5, p.accent));
    let resp = ui.interact(ov, egui::Id::unique("viewer_overview"), egui::Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if let Some(ptr) = resp.hover_pos() {
        let i = (((ptr.x - ov.left()) / ov.width()) * n as f32).floor().clamp(0.0, n as f32 - 1.0) as usize;
        let kv = s.index.ranges[i].key_value;
        badge(ui.painter(), pos2(ptr.x, rect.top() - 4.0), &format!("{key_name} {kv}"), p, egui::Align2::CENTER_BOTTOM);
        if resp.clicked() || resp.dragged() {
            s.focus.ens = i;
        }
    }
}

/// Inspector for the gather view.
pub fn inspector_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let sources = s.session().map(TraceSource::all).unwrap_or_else(|| vec![s.focus.source.clone()]);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let title = s.focus_range().map(|r| if s.focus.key == SECTION_KEY { "Whole section".to_string() } else { format!("{} {}", s.focus.key, r.key_value) }).unwrap_or_else(|| "No data".into());
            ui.label(RichText::new(title).font(crate::theme::font_display(18.0)).color(p.text));
            if let Some(r) = s.focus_range() {
                ui.label(RichText::new(format!("{} traces · {}", fmt_count(r.len()), s.focus.source.label())).size(11.5).color(p.text_muted));
            }

            // ---- what's shown
            caps(ui, &p, "SHOWING", None);
            widgets::row(ui, &p, "Data", |ui| {
                egui::ComboBox::from_id_salt("viewer_source").selected_text(s.focus.source.label()).width(160.0).show_ui(ui, |ui| {
                    for src in &sources {
                        if ui.selectable_label(*src == s.focus.source, src.label()).clicked() {
                            s.focus.source = src.clone();
                            s.focus.ens = 0;
                            s.viewer.request_refit = true;
                            if let Some(sess) = s.session() {
                                s.focus.key = data::suggest_key(src.traces(sess)).to_string();
                            }
                        }
                    }
                });
            });
            widgets::row(ui, &p, "Group by", |ui| {
                egui::ComboBox::from_id_salt("viewer_key").selected_text(if s.focus.key == SECTION_KEY { "Whole section".to_string() } else { s.focus.key.clone() }).width(160.0).show_ui(ui, |ui| {
                    for k in ENSEMBLE_KEYS {
                        let label = if *k == SECTION_KEY { "Whole section".to_string() } else { k.to_string() };
                        if ui.selectable_label(s.focus.key == *k, label).clicked() {
                            s.focus.key = k.to_string();
                            s.focus.ens = 0;
                            s.viewer.request_refit = true;
                        }
                    }
                });
            });
            widgets::row(ui, &p, "Label traces by", |ui| {
                egui::ComboBox::from_id_salt("viewer_label").selected_text(s.viewer.label_key.clone()).width(160.0).show_ui(ui, |ui| {
                    for k in LABEL_KEYS {
                        ui.selectable_value(&mut s.viewer.label_key, k.to_string(), *k);
                    }
                });
            });
            widgets::row(ui, &p, "Header tracks", |ui| {
                widgets::toggle(ui, &p, &mut s.viewer.show_tracks);
            });
            widgets::row(ui, &p, "Compare side by side", |ui| {
                let mut c = s.viewer.compare;
                if widgets::toggle(ui, &p, &mut c).changed() {
                    s.viewer.compare = c;
                    if c {
                        if let Some(other) = sources.iter().find(|x| **x != s.focus.source) {
                            s.viewer.compare_source = other.clone();
                        }
                    }
                }
            });
            if s.viewer.compare {
                widgets::row(ui, &p, "Right side", |ui| {
                    egui::ComboBox::from_id_salt("viewer_cmp").selected_text(s.viewer.compare_source.label()).width(160.0).show_ui(ui, |ui| {
                        for src in &sources {
                            ui.selectable_value(&mut s.viewer.compare_source, src.clone(), src.label());
                        }
                    });
                });
            }

            // ---- lineage
            let made_by = lineage(s);
            if !made_by.is_empty() {
                caps(ui, &p, "MADE BY", None);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for (i, (label, cat)) in made_by.iter().enumerate() {
                        let col = super::catalog::category_color(&p, cat);
                        // One painted widget per chip so the wrapping layout can break between chips.
                        let font = egui::FontId::proportional(11.0);
                        let g = ui.painter().layout_no_wrap(label.clone(), font.clone(), p.text);
                        let w = (g.size().x + 16.0).min(ui.available_width().max(60.0));
                        let (r, _) = ui.allocate_exact_size(vec2(w, 20.0), egui::Sense::hover());
                        ui.painter().rect(r, CornerRadius::same(10), with_alpha(col, 26), Stroke::new(1.0, with_alpha(col, 90)), egui::StrokeKind::Inside);
                        ui.painter().with_clip_rect(r.shrink(2.0)).text(r.center(), egui::Align2::CENTER_CENTER, label, font, p.text);
                        if i + 1 < made_by.len() {
                            ui.label(RichText::new("›").color(p.text_faint));
                        }
                    }
                });
            }

            // ---- histogram
            caps(ui, &p, "AMPLITUDES", Some(&format!("clip at {:.1} %", s.viewer.clip)));
            histogram(s, ui, &p);

            // ---- under cursor
            caps(ui, &p, "UNDER THE CURSOR", None);
            under_cursor(s, ui, &p);

            // ---- insight
            let dead = s.viewer.stats.dead.len();
            let spiky = s.viewer.stats.spiky.len();
            if dead > 0 || spiky > 0 {
                ui.add_space(4.0);
                let body = match (dead, spiky) {
                    (d, 0) => format!("{d} trace{} in this gather carry no signal. Killing them keeps them out of later statistics and stacks.", if d == 1 { "" } else { "s" }),
                    (0, k) => format!("{k} trace{} are far stronger than their neighbours. They may be noise bursts worth editing.", if k == 1 { "" } else { "s" }),
                    (d, k) => format!("{d} dead and {k} unusually strong traces in this gather."),
                };
                let (go, _) = widgets::insight(ui, &p, "Some traces look wrong", &body, "Edit traces", "");
                if go {
                    s.viewer.mode = PickMode::Kill;
                }
            }
        });
    });
}

/// Steps of the current flow that produced the displayed source.
fn lineage(s: &AppState) -> Vec<(String, String)> {
    let spec = s.flow.to_spec();
    let mut out = Vec::new();
    for st in &spec.steps {
        if !st.enabled {
            continue;
        }
        if st.tool == "buffer_save" {
            if let TraceSource::Buffer(name) = &s.focus.source {
                if st.params.get("name").and_then(|v| v.as_str()) == Some(name.as_str()) {
                    return out;
                }
            }
            continue;
        }
        let (label, cat) = s.tool(&st.tool).map(|t| (t.label.clone(), t.category.clone())).unwrap_or_else(|| (st.tool.clone(), String::new()));
        out.push((label, cat));
    }
    if matches!(s.focus.source, TraceSource::Buffer(_)) {
        return Vec::new();
    }
    out
}

fn histogram(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let st = &s.viewer.stats;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 84.0), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(8), p.sunken);
    if st.hist.is_empty() || st.amp_sorted.is_empty() {
        return;
    }
    let inner = rect.shrink2(vec2(8.0, 8.0));
    let hmax = st.hist.iter().copied().max().unwrap_or(1).max(1) as f32;
    let bw = inner.width() / st.hist.len() as f32;
    let clip_amp = {
        let i = ((s.viewer.clip / 100.0) * (st.amp_sorted.len() - 1) as f32) as usize;
        st.amp_sorted[i.min(st.amp_sorted.len() - 1)]
    };
    let clip_x = inner.left() + (clip_amp / st.hist_max).clamp(0.0, 1.0) * inner.width();
    for (i, c) in st.hist.iter().enumerate() {
        let h = ((*c as f32).ln_1p() / hmax.ln_1p()) * inner.height();
        let x = inner.left() + i as f32 * bw;
        let col = if x > clip_x { with_alpha(p.pick, 140) } else { with_alpha(p.accent, 200) };
        painter.rect_filled(egui::Rect::from_min_max(pos2(x + 0.5, inner.bottom() - h), pos2(x + bw - 0.5, inner.bottom())), CornerRadius::ZERO, col);
    }
    painter.vline(clip_x, rect.y_range(), Stroke::new(2.0, p.pick));
    painter.circle_filled(pos2(clip_x, rect.top() + 6.0), 4.0, p.pick);
    let over = 100.0 - s.viewer.clip;
    painter.text(pos2(inner.right(), inner.top()), egui::Align2::RIGHT_TOP, format!("{over:.1} % of samples clipped"), egui::FontId::proportional(10.0), p.text_muted);
    let resp = resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal).on_hover_text("Drag to set where colours saturate");
    if resp.dragged() || resp.clicked() {
        if let Some(ptr) = resp.interact_pointer_pos() {
            let a = ((ptr.x - inner.left()) / inner.width()).clamp(0.0, 1.0) * st.hist_max;
            let rank = st.amp_sorted.partition_point(|v| *v < a);
            s.viewer.clip = (rank as f32 / st.amp_sorted.len() as f32 * 100.0).clamp(80.0, 100.0);
        }
    }
}

fn under_cursor(s: &AppState, ui: &mut Ui, p: &Palette) {
    let (Some(ti), Some(sess)) = (s.crosshair.trace_index, s.session()) else {
        ui.label(RichText::new("Hover the gather to read headers and amplitude.").size(11.5).color(p.text_muted));
        return;
    };
    let traces = s.focus.source.traces(sess);
    let Some(tr) = traces.get(ti) else { return };
    let h = &tr.headers;
    let t = s.crosshair.time_ms.unwrap_or(0.0);
    let dt = tr.dt_ms();
    let amp = if dt > 0.0 { tr.samples.get((t / dt).round() as usize).copied() } else { None };
    egui::Grid::new("viewer_cursor").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        for (k, v) in [
            ("Shot", h.ffid.to_string()),
            ("Channel", h.chan.to_string()),
            ("CDP", h.cdp.to_string()),
            ("Offset", format!("{:.0} m", h.offset)),
            ("Time", format!("{t:.1} ms")),
            ("Amplitude", amp.map(|a| format!("{a:.4e}")).unwrap_or_else(|| "–".into())),
        ] {
            ui.label(RichText::new(k).size(11.5).color(p.text_muted));
            ui.label(widgets::mono(v, 11.5, p.text));
            ui.end_row();
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

pub fn export_side_by_side(s: &mut AppState) {
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

fn gather_canvas(s: &mut AppState, ui: &mut Ui, p: &Palette, size: egui::Vec2, range: data::EnsRange, is_b: bool, busy: bool) -> canvas::PlotFrame {
    // Axis labels: cached per (data, ensemble, key); falls back to a header that varies
    // (e.g. CHAN is constant on stacked sections, so CDP is shown instead).
    let preferred = s.viewer.label_key.clone();
    let slot = usize::from(is_b);
    let src = if is_b { s.viewer.compare_source.clone() } else { s.focus.source.clone() };
    let lkey = hash_of((s.rev, &src, range.start, range.end, preferred.as_str()));
    if s.viewer.labels_cache[slot].0 != lkey {
        let computed = s.session().map(|sess| {
            let tr: &[Trace] = if is_b {
                let all = src.traces(sess);
                matching_range(s, all, range).map(|(a, b)| &all[a..b]).unwrap_or(&[])
            } else {
                &src.traces(sess)[range.start..range.end]
            };
            let step = (tr.len() / 20_000).max(1);
            let varies = |k: &str| {
                let probe: Vec<f64> = tr.iter().step_by((tr.len() / 64).max(1)).map(|t| header_f64(t, k)).collect();
                probe.iter().any(|v| (*v - probe[0]).abs() > 1e-9)
            };
            let key = if tr.len() < 2 || varies(&preferred) {
                preferred.clone()
            } else {
                ["CDP", "OFFSET", "TRACENO", "REC_DEP", "CHAN"].iter().find(|k| varies(k)).map(|k| k.to_string()).unwrap_or(preferred.clone())
            };
            let vals: Vec<f64> = tr.iter().step_by(step).map(|t| header_f64(t, &key)).collect();
            (key, vals)
        });
        if let Some((key, vals)) = computed {
            s.viewer.labels_cache[slot] = (lkey, key, std::sync::Arc::new(vals));
        }
    }
    let label_key = s.viewer.labels_cache[slot].1.clone();
    let labels = std::sync::Arc::clone(&s.viewer.labels_cache[slot].2);
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
            let line = if s.viewer.colormap == SeismicColormap::Greyscale { Color32::from_rgba_unmultiplied(10, 30, 60, 200) } else { Color32::from_rgba_unmultiplied(12, 14, 20, 150) };
            let fill = s.viewer.va_fill.then_some(Color32::from_rgba_unmultiplied(12, 14, 20, 120));
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
        format!("After · {}", s.viewer.compare_source.label())
    } else if s.viewer.compare {
        format!("Before · {}", s.focus.source.label())
    } else {
        String::new()
    };
    if !title.is_empty() {
        badge(&painter, out.frame.rect.center_top() + vec2(0.0, 8.0), &title, p, egui::Align2::CENTER_TOP);
    }
    if busy {
        badge(&painter, out.frame.rect.right_top() + vec2(-8.0, 8.0), "engine busy · showing the last view", p, egui::Align2::RIGHT_TOP);
    }
    if is_b && s.viewer.raster_b.is_none() {
        painter.text(out.frame.rect.center(), egui::Align2::CENTER_CENTER, "Nothing matching this gather on this side", egui::FontId::proportional(13.0), p.text_muted);
    }
    if !is_b {
        if s.viewer.mode != PickMode::Navigate {
            loupe(s, ui, &out, range, p);
        }
        interact(s, &out, range);
    }
    out.frame
}

/// Magnifier that follows the cursor while picking.
fn loupe(s: &AppState, ui: &Ui, out: &canvas::CanvasOutput, range: data::EnsRange, p: &Palette) {
    let Some(ptr) = out.response.hover_pos() else { return };
    let Some(raster) = s.viewer.raster.as_ref() else { return };
    let size = 150.0;
    let fr = out.frame.rect;
    let mut min = ptr + vec2(24.0, -size - 24.0);
    if min.x + size > fr.right() {
        min.x = ptr.x - size - 24.0;
    }
    if min.y < fr.top() {
        min.y = ptr.y + 24.0;
    }
    let rect = egui::Rect::from_min_size(min, vec2(size, size));
    let (cx, cy) = out.frame.from_screen(ptr);
    let v = out.frame.view;
    let (hx, hy) = ((v.x1 - v.x0).abs() * 0.08, (v.y1 - v.y0).abs() * 0.08);
    let lf = canvas::PlotFrame { rect: rect.shrink(2.0), view: View2D::new(cx - hx, cx + hx, cy - hy, cy + hy), y_down: true };
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, CornerRadius::same(12), p.canvas);
    let inner = ui.painter().with_clip_rect(rect.shrink(2.0));
    draw_raster(&inner, &lf, raster, Color32::WHITE);
    if let Some(sess) = s.session() {
        let traces = &s.focus.source.traces(sess)[range.start..range.end];
        draw_wiggles(&inner, &lf, traces, 0.0, s.viewer.clip_amp.1, s.viewer.wiggle_gain, Color32::from_rgba_unmultiplied(12, 14, 20, 230), Some(Color32::from_rgba_unmultiplied(12, 14, 20, 170)));
        overlays(s, &inner, &lf, traces, p);
    }
    let c = rect.center();
    inner.line_segment([c - vec2(8.0, 0.0), c + vec2(8.0, 0.0)], Stroke::new(1.0, p.pick));
    inner.line_segment([c - vec2(0.0, 8.0), c + vec2(0.0, 8.0)], Stroke::new(1.0, p.pick));
    ui.painter().rect_stroke(rect, CornerRadius::same(12), Stroke::new(1.5, with_alpha(p.text, 60)), egui::StrokeKind::Inside);
    ui.painter().text(rect.left_bottom() + vec2(8.0, -6.0), egui::Align2::LEFT_BOTTOM, format!("{cy:.0} ms"), font_mono(10.0), p.text);
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
        if v.mute_mode != MuteMode::Surgical && v.mute_taper_ms > 0.0 {
            let sign = if v.mute_mode == MuteMode::Top { 1.0 } else { -1.0 };
            let dy = frame.y_to_screen(v.mute_taper_ms as f64) - frame.y_to_screen(0.0);
            let taper: Vec<Pos2> = top_pts.iter().map(|q| pos2(q.x, q.y + sign * dy)).collect();
            painter.add(egui::Shape::dashed_line(&taper, Stroke::new(1.0, with_alpha(p.pick, 180)), 5.0, 4.0));
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
