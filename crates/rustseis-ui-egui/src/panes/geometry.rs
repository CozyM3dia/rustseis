//! Survey view: basemap coloured by fold with shots, bin corridor, north arrow and scale bar,
//! over a virtualised header table with fold bars.

use crate::data::{TraceSource, fmt_count, header_f64};
use crate::state::AppState;
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets::{self, caps, cmd};
use egui::{RichText, Ui};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;
use egui_plot::{Line, MarkerShape, Plot, PlotPoints, Points};
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
    cdps: Vec<(i32, f64, f64, usize)>,
    crooked: bool,
    has_coords: bool,
}

pub fn table_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let scroll_to = s.geometry.scroll_to.take();
    let Some(sess) = s.session() else {
        super::busy_notice(s, ui);
        return;
    };
    let traces = s.geometry.source.traces(sess);
    if traces.is_empty() {
        crate::canvas::empty_state(ui, &p, icon::GRID_FOUR, "No traces", "Load data to see its headers.");
        return;
    }
    let selected = if s.geometry.source == s.focus.source { s.crosshair.trace_index } else { None };
    let fold: std::collections::HashMap<i32, usize> = s.geometry.map.as_ref().map(|m| m.cdps.iter().map(|c| (c.0, c.3)).collect()).unwrap_or_default();
    let max_fold = fold.values().copied().max().unwrap_or(1).max(1);
    let mut clicked: Option<usize> = None;
    let mut tb = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::right_to_left(egui::Align::Center))
        .column(Column::exact(64.0))
        .column(Column::exact(86.0));
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
        h.col(|ui| {
            ui.label(RichText::new("FOLD").font(font_mono(10.5)).color(p.text_muted));
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
            row.col(|ui| {
                let f = fold.get(&t.headers.cdp).copied().unwrap_or(0);
                let (r, _) = ui.allocate_exact_size(egui::vec2(78.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(r, egui::CornerRadius::same(3), p.sunken);
                let mut fr = r;
                fr.set_width(r.width() * f as f32 / max_fold as f32);
                ui.painter().rect_filled(fr, egui::CornerRadius::same(3), fold_color(&p, f as f32 / max_fold as f32));
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
    let cdps = cdp_acc.into_iter().map(|(c, (x, y, n))| (c, x / n as f64, y / n as f64, n)).collect();
    MapData { sources, receivers, cdps, crooked, has_coords }
}

pub fn basemap_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    if s.geometry.corridor_half_width == 0.0 {
        s.geometry.corridor_half_width = 250.0;
        s.geometry.show_sources = true;
        s.geometry.show_receivers = true;
    }
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
        crate::canvas::empty_state(ui, p, icon::MAP_TRIFOLD, "No coordinates yet", "Add a geometry step (marine, land or crooked line) to the flow and run it.");
        return;
    }
    let line: Vec<[f64; 2]> = m.cdps.iter().map(|(_, x, y, _)| [*x, *y]).collect();
    let max_fold = m.cdps.iter().map(|c| c.3).max().unwrap_or(1).max(1);
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
            // CDP line coloured by fold, in a handful of buckets.
            let buckets = 6usize;
            let mut seg: Vec<Vec<[f64; 2]>> = vec![Vec::new(); buckets];
            let mut cur: Option<usize> = None;
            for (c, x, y, n) in &m.cdps {
                let _ = c;
                let b = ((*n as f32 / max_fold as f32) * (buckets as f32 - 1.0)).round() as usize;
                if cur != Some(b) {
                    if let Some(prev) = cur {
                        seg[prev].push([f64::NAN, f64::NAN]);
                        if let Some(last) = seg[prev].iter().rev().nth(1).copied() {
                            seg[b].push(last);
                        }
                    }
                    cur = Some(b);
                }
                seg[b].push([*x, *y]);
            }
            for (b, pts) in seg.into_iter().enumerate() {
                if pts.len() > 1 {
                    let col = fold_color(p, b as f32 / (buckets as f32 - 1.0));
                    pu.line(Line::new(format!("fold {}", b), PlotPoints::from(pts)).color(col).width(3.0));
                }
            }
            if let Some(c) = cross {
                pu.points(Points::new("crosshair", PlotPoints::from(vec![c])).color(p.crosshair).radius(7.0).filled(false).shape(MarkerShape::Circle));
                pu.points(Points::new("crosshair·", PlotPoints::from(vec![c])).color(p.accent).radius(2.5));
            }
            pu.pointer_coordinate()
        });
    map_furniture(ui, p, &resp, max_fold);
    if resp.response.clicked() {
        if let Some(pt) = resp.inner {
            let nearest = m.cdps.iter().map(|c| (c.0, c.1, c.2)).min_by(|a, b| {
                let da = (a.1 - pt.x).powi(2) + (a.2 - pt.y).powi(2);
                let db = (b.1 - pt.x).powi(2) + (b.2 - pt.y).powi(2);
                da.total_cmp(&db)
            });
            if let Some((cdp, _, _)) = nearest {
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

/// Fold colour ramp: low fold amber → full fold mint.
fn fold_color(p: &Palette, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { crate::theme::mix(p.danger, p.pick, t * 2.0) } else { crate::theme::mix(p.pick, p.accent, (t - 0.5) * 2.0) }
}

/// North arrow, scale bar and fold legend over the basemap.
fn map_furniture(ui: &mut Ui, p: &Palette, resp: &egui_plot::PlotResponse<Option<egui_plot::PlotPoint>>, max_fold: usize) {
    let rect = resp.response.rect;
    let painter = ui.painter_at(rect);
    // North arrow
    let c = rect.right_top() + egui::vec2(-28.0, 30.0);
    painter.circle_filled(c, 16.0, with_alpha(p.panel, 220));
    painter.add(egui::Shape::convex_polygon(vec![c + egui::vec2(0.0, -11.0), c + egui::vec2(6.0, 7.0), c + egui::vec2(0.0, 3.0), c + egui::vec2(-6.0, 7.0)], p.text, egui::Stroke::NONE));
    painter.text(c + egui::vec2(0.0, -24.0), egui::Align2::CENTER_BOTTOM, "N", crate::theme::font_semibold(10.0), p.text_muted);
    // Scale bar: a round length near 1/5 of the plot width.
    let m_per_px = resp.transform.dvalue_dpos()[0].abs();
    if m_per_px.is_finite() && m_per_px > 0.0 {
        let target = m_per_px * rect.width() as f64 * 0.2;
        let mag = 10f64.powf(target.log10().floor());
        let nice = [1.0, 2.0, 5.0, 10.0].iter().map(|f| f * mag).min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs())).unwrap_or(mag);
        let w = (nice / m_per_px) as f32;
        let o = rect.left_bottom() + egui::vec2(60.0, -34.0);
        painter.rect_filled(egui::Rect::from_min_size(o - egui::vec2(8.0, 18.0), egui::vec2(w + 16.0, 28.0)), egui::CornerRadius::same(6), with_alpha(p.panel, 220));
        painter.line_segment([o, o + egui::vec2(w, 0.0)], egui::Stroke::new(3.0, p.text));
        for x in [0.0, w] {
            painter.line_segment([o + egui::vec2(x, -4.0), o + egui::vec2(x, 4.0)], egui::Stroke::new(1.5, p.text));
        }
        let label = if nice >= 1000.0 { format!("{:.0} km", nice / 1000.0) } else { format!("{nice:.0} m") };
        painter.text(o + egui::vec2(w * 0.5, -6.0), egui::Align2::CENTER_BOTTOM, label, egui::FontId::proportional(10.5), p.text);
    }
    // Fold legend
    let lg = egui::Rect::from_min_size(rect.right_bottom() + egui::vec2(-168.0, -44.0), egui::vec2(150.0, 30.0));
    painter.rect_filled(lg.expand(4.0), egui::CornerRadius::same(6), with_alpha(p.panel, 220));
    let bar = egui::Rect::from_min_size(lg.left_top() + egui::vec2(0.0, 16.0), egui::vec2(lg.width(), 6.0));
    let n = 30;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let r = egui::Rect::from_min_size(bar.left_top() + egui::vec2(t * bar.width(), 0.0), egui::vec2(bar.width() / n as f32 + 0.5, bar.height()));
        painter.rect_filled(r, egui::CornerRadius::ZERO, fold_color(p, t));
    }
    painter.text(lg.left_top(), egui::Align2::LEFT_TOP, "fold", egui::FontId::proportional(10.0), p.text_muted);
    painter.text(lg.right_top(), egui::Align2::RIGHT_TOP, format!("1 … {max_fold}"), font_mono(9.5), p.text_muted);
}

/// The survey document: basemap above, header table below.
pub fn survey_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    if s.geometry.corridor_half_width == 0.0 {
        s.geometry.corridor_half_width = 250.0;
        s.geometry.show_sources = true;
        s.geometry.show_receivers = true;
    }
    let full = ui.available_rect_before_wrap().shrink2(egui::vec2(8.0, 6.0));
    let map_h = (full.height() * 0.6).max(200.0);
    let rm = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), map_h));
    let rt = egui::Rect::from_min_max(egui::pos2(full.left(), rm.bottom() + 8.0), full.max);
    let mut child = |r: egui::Rect| ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
    {
        let mut u = child(rm);
        egui::Frame::new().fill(p.sunken).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(egui::CornerRadius::same(10)).inner_margin(egui::Margin::same(6)).show(&mut u, |ui| {
            ui.set_min_size(rm.size() - egui::vec2(14.0, 14.0));
            basemap_ui(s, ui);
        });
    }
    {
        let mut u = child(rt);
        egui::Frame::new().fill(p.sunken).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(egui::CornerRadius::same(10)).inner_margin(egui::Margin::same(6)).show(&mut u, |ui| {
            ui.set_min_size(rt.size() - egui::vec2(14.0, 14.0));
            table_ui(s, ui);
        });
    }
    ui.allocate_rect(full, egui::Sense::hover());
}

/// Survey inspector: what the geometry looks like and the steps that build it.
pub fn inspector_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let sources = s.session().map(TraceSource::all).unwrap_or_default();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new("Survey").font(crate::theme::font_display(18.0)).color(p.text));
            widgets::row(ui, &p, "Data", |ui| {
                egui::ComboBox::from_id_salt("geom_src").selected_text(s.geometry.source.label()).width(150.0).show_ui(ui, |ui| {
                    for src in &sources {
                        ui.selectable_value(&mut s.geometry.source, src.clone(), src.label());
                    }
                });
            });
            if let Some(m) = &s.geometry.map {
                let max_fold = m.cdps.iter().map(|c| c.3).max().unwrap_or(0);
                let len: f64 = m.cdps.windows(2).map(|w| ((w[1].1 - w[0].1).powi(2) + (w[1].2 - w[0].2).powi(2)).sqrt()).sum();
                caps(ui, &p, "AT A GLANCE", None);
                egui::Grid::new("survey_glance").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    for (k, v) in [
                        ("Shots", fmt_count(m.sources.len())),
                        ("Receiver positions", fmt_count(m.receivers.len())),
                        ("CDPs", fmt_count(m.cdps.len())),
                        ("Highest fold", max_fold.to_string()),
                        ("Line length", if len >= 1000.0 { format!("{:.1} km", len / 1000.0) } else { format!("{len:.0} m") }),
                        ("Line shape", if m.crooked { "crooked".to_string() } else { "straight".to_string() }),
                    ] {
                        ui.label(RichText::new(k).size(12.0).color(p.text_muted));
                        ui.label(widgets::mono(v, 12.0, p.text));
                        ui.end_row();
                    }
                });
            }
            caps(ui, &p, "MAP", None);
            widgets::row(ui, &p, "Shots", |ui| {
                widgets::toggle(ui, &p, &mut s.geometry.show_sources);
            });
            widgets::row(ui, &p, "Receivers", |ui| {
                widgets::toggle(ui, &p, &mut s.geometry.show_receivers);
            });
            widgets::row(ui, &p, "Bin corridor half-width", |ui| {
                ui.add(egui::DragValue::new(&mut s.geometry.corridor_half_width).range(0.0..=5000.0).speed(5.0).suffix(" m"));
            });
            if cmd(ui, &p, icon::CROSSHAIR, "Find the selected trace in the table", false).clicked() {
                s.geometry.scroll_to = s.crosshair.trace_index;
            }
        });
    });
}
