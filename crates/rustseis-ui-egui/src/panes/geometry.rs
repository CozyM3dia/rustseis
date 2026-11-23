//! Workspace 6: virtualized geometry spreadsheet and 2D / crooked-line basemap.

use crate::data::{TraceSource, fmt_count, header_f64};
use crate::state::AppState;
use crate::theme::{font_mono, palette, with_alpha};
use crate::widgets::{self, chip};
use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;
use egui_plot::{Legend, Line, MarkerShape, Plot, PlotPoints, Points};
use std::collections::{BTreeMap, HashSet};

const COLS: &[(&str, &str, u8)] = &[
    ("TRACENO", "TRACENO", 0),
    ("FFID", "FFID", 0),
    ("CHAN", "CHAN", 0),
    ("CDP", "CDP", 0),
    ("OFFSET", "OFFSET", 1),
    ("SOU_X", "SOU_X", 1),
    ("SOU_Y", "SOU_Y", 1),
    ("REC_X", "REC_X", 1),
    ("REC_Y", "REC_Y", 1),
    ("CDP_X", "CDP_X", 1),
    ("CDP_Y", "CDP_Y", 1),
    ("CDP_NFLD", "CDP_NFLD", 0),
    ("FB_PICK", "FB_PICK", 2),
    ("TOT_STAT", "TOT_STAT", 2),
    ("XLINE_DIST", "XLINE_DIST", 1),
];

#[derive(Default)]
pub struct GeometryState {
    pub source: TraceSource,
    pub scroll_to: Option<usize>,
    map: Option<MapData>,
    map_key: (u64, Option<TraceSource>),
    pub show_receivers: bool,
    pub show_sources: bool,
    pub corridor_half_width: f64,
}

struct MapData {
    sources: Vec<[f64; 2]>,
    receivers: Vec<[f64; 2]>,
    cdps: Vec<(i32, f64, f64)>,
    crooked: bool,
    has_coords: bool,
}

pub fn table_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let sources = s.session().map(TraceSource::all).unwrap_or_default();
    widgets::toolbar(ui, &p, |ui| {
        egui::ComboBox::from_id_salt("geom_src").selected_text(RichText::new(s.geometry.source.label()).size(12.0)).width(150.0).show_ui(ui, |ui| {
            for src in &sources {
                ui.selectable_value(&mut s.geometry.source, src.clone(), src.label());
            }
        });
        if ui.button(format!("{}  Go to crosshair", icon::CROSSHAIR)).clicked() {
            s.geometry.scroll_to = s.crosshair.trace_index;
        }
        let n = s.session().map(|x| s.geometry.source.traces(x).len()).unwrap_or(0);
        chip(ui, &format!("{} rows", fmt_count(n)), p.text_muted, p.sunken);
    });
    let scroll_to = s.geometry.scroll_to.take();
    let Some(sess) = s.session() else {
        super::busy_notice(s, ui);
        return;
    };
    let traces = s.geometry.source.traces(sess);
    if traces.is_empty() {
        crate::canvas::empty_state(ui, &p, icon::GRID_FOUR, "No traces", "Load data to inspect geometry headers");
        return;
    }
    let selected = if s.geometry.source == s.focus.source { s.crosshair.trace_index } else { None };
    let mut clicked: Option<usize> = None;
    let mut tb = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::right_to_left(egui::Align::Center))
        .column(Column::exact(64.0));
    for _ in COLS {
        tb = tb.column(Column::initial(78.0).at_least(48.0).clip(true));
    }
    if let Some(r) = scroll_to {
        tb = tb.scroll_to_row(r, Some(egui::Align::Center));
    }
    tb.header(22.0, |mut h| {
        h.col(|ui| {
            ui.label(RichText::new("#").size(11.0).color(p.text_muted));
        });
        for (title, _, _) in COLS {
            h.col(|ui| {
                ui.label(RichText::new(*title).font(font_mono(10.5)).color(p.text_muted));
            });
        }
    })
    .body(|body| {
        body.rows(18.0, traces.len(), |mut row| {
            let i = row.index();
            row.set_selected(selected == Some(i));
            let t = &traces[i];
            row.col(|ui| {
                ui.label(RichText::new(i.to_string()).font(font_mono(10.5)).color(p.text_faint));
            });
            for (_, key, prec) in COLS {
                row.col(|ui| {
                    let v = header_f64(t, key);
                    let txt = match prec {
                        0 => format!("{v:.0}"),
                        1 => format!("{v:.1}"),
                        _ => format!("{v:.2}"),
                    };
                    ui.label(RichText::new(txt).font(font_mono(11.0)).color(if v == 0.0 { p.text_faint } else { p.text }));
                });
            }
            if row.response().clicked() {
                clicked = Some(i);
            }
        });
    });
    if let Some(i) = clicked {
        if s.geometry.source != s.focus.source {
            s.focus.source = s.geometry.source.clone();
        }
        s.focus_trace(i);
    }
}

fn build_map(traces: &[rustseis_core::Trace]) -> MapData {
    let mut seen_src = HashSet::new();
    let mut sources = Vec::new();
    let mut rec_seen = HashSet::new();
    let mut receivers = Vec::new();
    let mut cdp_acc: BTreeMap<i32, (f64, f64, usize)> = BTreeMap::new();
    let mut crooked = false;
    let mut has_coords = false;
    for t in traces {
        let h = &t.headers;
        if h.sou_x != 0.0 || h.sou_y != 0.0 || h.cdp_x != 0.0 || h.rec_x != 0.0 {
            has_coords = true;
        }
        if seen_src.insert(h.ffid) {
            sources.push([h.sou_x, h.sou_y]);
        }
        let rk = ((h.rec_x * 0.5).round() as i64, (h.rec_y * 0.5).round() as i64);
        if rec_seen.len() < 40_000 && rec_seen.insert(rk) {
            receivers.push([h.rec_x, h.rec_y]);
        }
        let e = cdp_acc.entry(h.cdp).or_insert((0.0, 0.0, 0));
        e.0 += h.cdp_x;
        e.1 += h.cdp_y;
        e.2 += 1;
        if !crooked && h.custom.contains_key("XLINE_DIST") {
            crooked = true;
        }
    }
    let cdps = cdp_acc.into_iter().map(|(c, (x, y, n))| (c, x / n as f64, y / n as f64)).collect();
    MapData { sources, receivers, cdps, crooked, has_coords }
}

pub fn basemap_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    if s.geometry.corridor_half_width == 0.0 {
        s.geometry.corridor_half_width = 250.0;
        s.geometry.show_sources = true;
        s.geometry.show_receivers = true;
    }
    widgets::toolbar(ui, &p, |ui| {
        ui.toggle_value(&mut s.geometry.show_sources, format!("{} sources", icon::BROADCAST));
        ui.toggle_value(&mut s.geometry.show_receivers, format!("{} receivers", icon::CELL_SIGNAL_FULL));
        ui.add(egui::DragValue::new(&mut s.geometry.corridor_half_width).range(0.0..=5000.0).speed(5.0).prefix("bin corridor ±").suffix(" m"));
        if let Some(m) = &s.geometry.map {
            if m.crooked {
                chip(ui, "crooked line", p.pick, with_alpha(p.pick, 22));
            }
            chip(ui, &format!("{} CDPs · {} shots", fmt_count(m.cdps.len()), fmt_count(m.sources.len())), p.text_muted, p.sunken);
        }
    });
    let Some(sess) = s.session() else {
        if s.geometry.map.is_none() {
            super::busy_notice(s, ui);
            return;
        }
        draw_map(s, ui, &p);
        return;
    };
    let key = (s.rev, Some(s.geometry.source.clone()));
    if s.geometry.map_key != key {
        let traces = s.geometry.source.traces(sess);
        s.geometry.map = Some(build_map(traces));
        s.geometry.map_key = key;
    }
    draw_map(s, ui, &p);
}

fn draw_map(s: &mut AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    let Some(m) = &s.geometry.map else { return };
    if !m.has_coords {
        crate::canvas::empty_state(ui, p, icon::MAP_TRIFOLD, "No survey coordinates", "Assign geometry (geomspread_marine / land / crooked) to populate SOU/REC/CDP X-Y");
        return;
    }
    let line: Vec<[f64; 2]> = m.cdps.iter().map(|(_, x, y)| [*x, *y]).collect();
    // Corridor boundaries: offset the CDP polyline by ±half-width along local normals.
    let hw = s.geometry.corridor_half_width;
    let mut left = Vec::with_capacity(line.len());
    let mut right = Vec::with_capacity(line.len());
    let stride = (line.len() / 800).max(1);
    for i in (0..line.len()).step_by(stride) {
        let a = line[i.saturating_sub(stride)];
        let b = line[(i + stride).min(line.len() - 1)];
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            continue;
        }
        let (nx, ny) = (-dy / len, dx / len);
        left.push([line[i][0] + nx * hw, line[i][1] + ny * hw]);
        right.push([line[i][0] - nx * hw, line[i][1] - ny * hw]);
    }
    let cross = match (s.crosshair.map_x, s.crosshair.map_y) {
        (Some(x), Some(y)) if x != 0.0 || y != 0.0 => Some([x, y]),
        _ => None,
    };
    let show_s = s.geometry.show_sources;
    let show_r = s.geometry.show_receivers;
    let resp = Plot::new("basemap")
        .data_aspect(1.0)
        .legend(Legend::default())
        .x_axis_label("X (m)")
        .y_axis_label("Y (m)")
        .show_background(false)
        .show(ui, |pu| {
            if show_r {
                pu.points(Points::new("receivers", PlotPoints::from(m.receivers.clone())).color(with_alpha(p.series_b, 120)).radius(1.0));
            }
            if show_s {
                pu.points(Points::new("sources", PlotPoints::from(m.sources.clone())).color(p.pick).radius(1.8).shape(MarkerShape::Diamond));
            }
            if hw > 0.0 && left.len() > 1 {
                pu.line(Line::new("bin corridor", PlotPoints::from(left)).color(with_alpha(p.text_muted, 130)).style(egui_plot::LineStyle::dashed_loose()));
                pu.line(Line::new("bin corridor ", PlotPoints::from(right)).color(with_alpha(p.text_muted, 130)).style(egui_plot::LineStyle::dashed_loose()));
            }
            if line.len() > 1 {
                pu.line(Line::new("CDP line", PlotPoints::from(line.clone())).color(p.accent).width(2.0));
            }
            if let Some(c) = cross {
                pu.points(Points::new("crosshair", PlotPoints::from(vec![c])).color(p.crosshair).radius(7.0).filled(false).shape(MarkerShape::Circle));
                pu.points(Points::new("crosshair·", PlotPoints::from(vec![c])).color(p.accent).radius(2.5));
            }
            pu.pointer_coordinate()
        });
    if resp.response.clicked() {
        if let Some(pt) = resp.inner {
            let nearest = m.cdps.iter().min_by(|a, b| {
                let da = (a.1 - pt.x).powi(2) + (a.2 - pt.y).powi(2);
                let db = (b.1 - pt.x).powi(2) + (b.2 - pt.y).powi(2);
                da.total_cmp(&db)
            });
            if let Some((cdp, _, _)) = nearest.copied() {
                let src = s.geometry.source.clone();
                if let Some(idx) = s.session().and_then(|sess| src.traces(sess).iter().position(|t| t.headers.cdp == cdp)) {
                    s.focus.source = src;
                    s.focus_trace(idx);
                    s.geometry.scroll_to = Some(idx);
                }
            }
        }
    }
}
