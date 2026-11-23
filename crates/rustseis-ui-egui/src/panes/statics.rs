//! Statics QC board: KPI tiles, station statics stem plot with outlier call-outs, first breaks
//! against offset with a misfit histogram, and surface-consistent amplitude terms.

use crate::state::{AppState, Doc};
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets::{self, caps, cmd};
use egui::{Color32, CornerRadius, RichText, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use egui_plot::{Line, Plot, PlotPoints, Points};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct StaticsState {
    pub statics_table: Option<String>,
    pub scamp_table: Option<String>,
    pub fb_table: Option<String>,
    pub scamp_db: bool,
    pub scamp_show: [bool; 5],
}

fn sorted_keys<V>(m: &std::collections::HashMap<String, V>) -> Vec<String> {
    let mut v: Vec<String> = m.keys().cloned().collect();
    v.sort();
    v
}

fn ensure_selection(cur: &mut Option<String>, names: &[String]) {
    if cur.as_ref().is_none_or(|c| !names.contains(c)) {
        *cur = names.first().cloned();
    }
}

/// Stations whose static is more than three standard deviations from the mean.
fn outliers(m: &BTreeMap<i32, f32>) -> Vec<(i32, f32)> {
    if m.len() < 5 {
        return Vec::new();
    }
    let n = m.len() as f32;
    let mean = m.values().sum::<f32>() / n;
    let sd = (m.values().map(|v| (v - mean).powi(2)).sum::<f32>() / n).sqrt().max(1e-6);
    let mut v: Vec<(i32, f32)> = m.iter().filter(|(_, v)| ((**v - mean) / sd).abs() > 3.0).map(|(k, v)| (*k, *v)).collect();
    v.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
    v
}

fn tile(ui: &mut Ui, p: &Palette, w: f32, label: &str, value: &str, note: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 68.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(10), p.sunken, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    painter.rect_filled(egui::Rect::from_min_size(rect.min + vec2(0.0, 12.0), vec2(3.0, rect.height() - 24.0)), CornerRadius::same(2), color);
    painter.text(rect.left_top() + vec2(14.0, 10.0), egui::Align2::LEFT_TOP, label, egui::FontId::proportional(11.0), p.text_muted);
    painter.text(rect.left_top() + vec2(14.0, 26.0), egui::Align2::LEFT_TOP, value, crate::theme::font_display(19.0), p.text);
    painter.text(rect.left_bottom() + vec2(14.0, -8.0), egui::Align2::LEFT_BOTTOM, note, egui::FontId::proportional(10.5), p.text_faint);
}

fn card(ui: &mut Ui, p: &Palette, rect: egui::Rect, title: &str, sub: &str) -> egui::Rect {
    let painter = ui.painter_at(rect);
    painter.rect(rect, CornerRadius::same(10), p.sunken, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    painter.text(rect.left_top() + vec2(12.0, 10.0), egui::Align2::LEFT_TOP, title, crate::theme::font_semibold(12.5), p.text);
    let tw = painter.layout_no_wrap(title.to_string(), crate::theme::font_semibold(12.5), p.text).size().x;
    painter.text(rect.left_top() + vec2(20.0 + tw, 11.0), egui::Align2::LEFT_TOP, sub, egui::FontId::proportional(11.0), p.text_muted);
    egui::Rect::from_min_max(rect.min + vec2(10.0, 32.0), rect.max - vec2(10.0, 10.0))
}

pub fn board_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let Some(sess) = s.session() else {
        super::busy_notice(s, ui);
        return;
    };
    let st_names = sorted_keys(&sess.context.statics);
    let sc_names = sorted_keys(&sess.context.sc_amps);
    let fb_names = sorted_keys(&sess.context.first_breaks);
    ensure_selection(&mut s.statics.statics_table, &st_names);
    ensure_selection(&mut s.statics.scamp_table, &sc_names);
    ensure_selection(&mut s.statics.fb_table, &fb_names);
    let Some(sess) = s.session() else { return };
    let stat = s.statics.statics_table.as_ref().and_then(|n| sess.context.statics.get(n)).cloned();
    let amp = s.statics.scamp_table.as_ref().and_then(|n| sess.context.sc_amps.get(n)).cloned();
    let fb = s.statics.fb_table.as_ref().and_then(|n| sess.context.first_breaks.get(n)).cloned();
    if stat.is_none() && amp.is_none() && fb.is_none() {
        crate::canvas::empty_state(ui, &p, icon::RULER, "No statics yet", "Pick first breaks on the gather view, or run a flow that solves statics or amplitude terms.");
        return;
    }
    let full = ui.available_rect_before_wrap().shrink2(vec2(8.0, 6.0));
    let gap = 8.0;

    // ---- KPI tiles
    let src_out = stat.as_ref().map(|t| outliers(&t.source_statics_ms)).unwrap_or_default();
    let rec_out = stat.as_ref().map(|t| outliers(&t.receiver_statics_ms)).unwrap_or_default();
    let fb_pts: Vec<[f64; 2]> = fb.as_ref().map(|t| t.picks.iter().step_by((t.picks.len() / 40_000).max(1)).filter(|pk| pk.time_ms > 0.0).map(|pk| [pk.offset_m.abs() as f64, pk.time_ms as f64]).collect()).unwrap_or_default();
    let fit = linear_fit(&fb_pts);
    let tiles_r = egui::Rect::from_min_size(full.min, vec2(full.width(), 68.0));
    let mut u = ui.new_child(egui::UiBuilder::new().max_rect(tiles_r).layout(egui::Layout::left_to_right(egui::Align::Min)));
    u.spacing_mut().item_spacing.x = gap;
    let tw = (full.width() - 3.0 * gap) / 4.0;
    match &stat {
        Some(t) => {
            let col = if t.rms_residual_ms < 2.0 { p.success } else if t.rms_residual_ms < 6.0 { p.pick } else { p.danger };
            tile(&mut u, &p, tw, "Remaining misfit", &format!("{:.2} ms", t.rms_residual_ms), &format!("{} solution", t.kind), col);
            tile(&mut u, &p, tw, "Stations solved", &format!("{} · {}", t.source_statics_ms.len(), t.receiver_statics_ms.len()), "shots · receivers", p.info);
        }
        None => {
            tile(&mut u, &p, tw, "Remaining misfit", "–", "no statics solution yet", p.border_strong);
            tile(&mut u, &p, tw, "Stations solved", "–", "", p.border_strong);
        }
    }
    let v_refr = stat.as_ref().map(|t| t.refractor_velocity_mps).filter(|v| *v > 0.0).or_else(|| fit.map(|(_, sl)| (1000.0 / sl.max(1e-9)) as f32));
    tile(&mut u, &p, tw, "Refractor velocity", &v_refr.map(|v| format!("{v:.0} m/s")).unwrap_or_else(|| "–".into()), &format!("{} first breaks", crate::data::fmt_count(fb.as_ref().map(|t| t.picks.len()).unwrap_or(0))), p.series_c);
    let look = src_out.len() + rec_out.len();
    tile(&mut u, &p, tw, "Needs a look", &look.to_string(), if look == 0 { "no unusual stations" } else { "stations far from their neighbours" }, if look == 0 { p.success } else { p.pick });

    // ---- rows
    let rest_top = tiles_r.bottom() + gap;
    let rest_h = full.bottom() - rest_top;
    let r_stem = egui::Rect::from_min_size(pos2(full.left(), rest_top), vec2(full.width(), rest_h * 0.46));
    let bot_top = r_stem.bottom() + gap;
    let bw = full.width() - 2.0 * gap;
    let r_fb = egui::Rect::from_min_max(pos2(full.left(), bot_top), pos2(full.left() + bw * 0.42, full.bottom()));
    let r_hist = egui::Rect::from_min_max(pos2(r_fb.right() + gap, bot_top), pos2(r_fb.right() + gap + bw * 0.22, full.bottom()));
    let r_amp = egui::Rect::from_min_max(pos2(r_hist.right() + gap, bot_top), full.max);

    // Stem plot
    let inner = card(ui, &p, r_stem, "Station statics", "shots above, receivers below · amber marks stations to check");
    match &stat {
        Some(t) => stem_plot(ui, &p, inner, &t.source_statics_ms, &t.receiver_statics_ms, &src_out, &rec_out),
        None => {
            ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER, "Solve statics to see them here", egui::FontId::proportional(12.0), p.text_faint);
        }
    }
    // First breaks vs offset
    let inner = card(ui, &p, r_fb, "First breaks against offset", &fit.map(|(t0, sl)| format!("line fit {:.0} m/s, {t0:.0} ms intercept", 1000.0 / sl.max(1e-9))).unwrap_or_default());
    let mut u = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    if fb_pts.is_empty() {
        u.painter().text(inner.center(), egui::Align2::CENTER_CENTER, "No first breaks yet", egui::FontId::proportional(12.0), p.text_faint);
    } else {
        Plot::new("fb_plot").height(inner.height()).x_axis_label("offset m").y_axis_label("ms").invert_y(true).show_background(false).allow_scroll(false).show(&mut u, |pu| {
            pu.points(Points::new("picks", PlotPoints::from(fb_pts.clone())).color(with_alpha(p.pick, 150)).radius(1.3));
            if let Some((t0, slope)) = fit {
                let xmax = fb_pts.iter().map(|q| q[0]).fold(0.0, f64::max);
                pu.line(Line::new("fit", PlotPoints::from(vec![[0.0, t0], [xmax, t0 + slope * xmax]])).color(p.accent).width(2.0));
            }
        });
    }
    // Misfit histogram
    let inner = card(ui, &p, r_hist, "Misfit", "pick − line fit");
    misfit_hist(ui, &p, inner, &fb_pts, fit);
    // Amplitude terms
    let sub = amp.as_ref().map(|t| format!("{} shots · {} receivers", t.source_factors.len(), t.receiver_factors.len())).unwrap_or_default();
    let inner = card(ui, &p, r_amp, "Amplitude terms", &sub);
    match &amp {
        Some(t) => amp_bars(ui, &p, inner, &t.source_factors, &t.receiver_factors),
        None => {
            ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER, "No amplitude terms yet", egui::FontId::proportional(12.0), p.text_faint);
        }
    }
    ui.allocate_rect(full, egui::Sense::hover());
}

#[allow(clippy::too_many_arguments)]
fn stem_plot(ui: &mut Ui, p: &Palette, rect: egui::Rect, src: &BTreeMap<i32, f32>, rec: &BTreeMap<i32, f32>, src_out: &[(i32, f32)], rec_out: &[(i32, f32)]) {
    let painter = ui.painter_at(rect);
    if src.is_empty() && rec.is_empty() {
        return;
    }
    let amax = src.values().chain(rec.values()).fold(0.0f32, |a, v| a.max(v.abs())).max(1e-3);
    let mid = rect.center().y;
    let half = rect.height() * 0.5 - 14.0;
    // Each row has its own station axis (shot and receiver numbering differ).
    let range = |m: &BTreeMap<i32, f32>| (m.keys().next().copied().unwrap_or(0), m.keys().next_back().copied().unwrap_or(1));
    let x_in = move |k: i32, (lo, hi): (i32, i32)| rect.left() + 40.0 + (k - lo) as f32 / (hi - lo).max(1) as f32 * (rect.width() - 50.0);
    painter.hline(rect.x_range(), mid, Stroke::new(1.0, p.border_strong));
    for (frac, lab) in [(1.0, format!("{amax:.1} ms")), (-1.0, format!("{:.1} ms", -amax))] {
        let y = mid - frac * half * 0.5;
        painter.hline(rect.x_range(), y, Stroke::new(1.0, with_alpha(p.border, 120)));
        painter.text(pos2(rect.left(), y), egui::Align2::LEFT_CENTER, lab, font_mono(9.5), p.text_faint);
    }
    let draw = |m: &BTreeMap<i32, f32>, sign: f32, col: Color32, out: &[(i32, f32)]| {
        let rg = range(m);
        let x_of = |k: i32| x_in(k, rg);
        let step = (m.len() / (rect.width() as usize).max(1)).max(1);
        for (k, v) in m.iter().step_by(step) {
            let x = x_of(*k);
            let base = mid - sign * half * 0.5;
            let yy = base - (v / amax) * half * 0.45;
            let bad = out.iter().any(|(ok, _)| ok == k);
            let c = if bad { p.pick } else { col };
            painter.line_segment([pos2(x, base), pos2(x, yy)], Stroke::new(1.0, with_alpha(c, 200)));
            painter.circle_filled(pos2(x, yy), if bad { 3.5 } else { 1.8 }, c);
        }
        if let Some((k, v)) = out.first() {
            let x = x_of(*k);
            let yy = mid - sign * half * 0.5 - (v / amax) * half * 0.45;
            painter.circle_stroke(pos2(x, yy), 8.0, Stroke::new(1.4, p.pick));
            let label = format!("{} {k} · {v:+.1} ms", if sign > 0.0 { "shot" } else { "receiver" });
            crate::canvas::badge(&painter, pos2(x + 12.0, yy), &label, p, egui::Align2::LEFT_CENTER);
        }
    };
    draw(src, 1.0, p.info, src_out);
    draw(rec, -1.0, p.series_c, rec_out);
    painter.text(pos2(rect.right(), rect.top()), egui::Align2::RIGHT_TOP, "shots", egui::FontId::proportional(10.5), p.info);
    painter.text(pos2(rect.right(), rect.bottom()), egui::Align2::RIGHT_BOTTOM, "receivers", egui::FontId::proportional(10.5), p.series_c);
    let (a, b) = range(rec);
    painter.text(pos2(rect.left() + 40.0, rect.bottom()), egui::Align2::LEFT_BOTTOM, format!("receiver {a}"), font_mono(9.5), p.text_faint);
    painter.text(pos2(rect.right() - 70.0, rect.bottom()), egui::Align2::RIGHT_BOTTOM, format!("receiver {b}"), font_mono(9.5), p.text_faint);
    let (a, b) = range(src);
    painter.text(pos2(rect.left() + 40.0, rect.top()), egui::Align2::LEFT_TOP, format!("shot {a}"), font_mono(9.5), p.text_faint);
    painter.text(pos2(rect.right() - 70.0, rect.top()), egui::Align2::RIGHT_TOP, format!("shot {b}"), font_mono(9.5), p.text_faint);
}

fn misfit_hist(ui: &mut Ui, p: &Palette, rect: egui::Rect, pts: &[[f64; 2]], fit: Option<(f64, f64)>) {
    let painter = ui.painter_at(rect);
    let Some((t0, sl)) = fit else {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "–", egui::FontId::proportional(12.0), p.text_faint);
        return;
    };
    let res: Vec<f64> = pts.iter().map(|q| q[1] - (t0 + sl * q[0])).collect();
    // Range from the median absolute deviation so a few wild picks don't flatten the histogram.
    let mut abs: Vec<f64> = res.iter().map(|r| r.abs()).collect();
    abs.sort_by(f64::total_cmp);
    let mad = abs.get(abs.len() / 2).copied().unwrap_or(1.0);
    let lim = (mad * 4.0).max(4.0);
    let nb = 24;
    let mut h = vec![0u32; nb];
    for r in &res {
        let b = (((r + lim) / (2.0 * lim)) * nb as f64).floor();
        if b >= 0.0 && (b as usize) < nb {
            h[b as usize] += 1;
        }
    }
    let hmax = h.iter().copied().max().unwrap_or(1).max(1) as f32;
    let plot = rect.shrink2(vec2(2.0, 14.0));
    let bw = plot.width() / nb as f32;
    for (i, c) in h.iter().enumerate() {
        let hh = *c as f32 / hmax * plot.height();
        let x = plot.left() + i as f32 * bw;
        let centre = (i as f32 + 0.5) / nb as f32 * 2.0 - 1.0;
        let col = if centre.abs() < 0.25 { p.accent } else { with_alpha(p.pick, 190) };
        painter.rect_filled(egui::Rect::from_min_max(pos2(x + 1.0, plot.bottom() - hh), pos2(x + bw - 1.0, plot.bottom())), CornerRadius::same(1), col);
    }
    painter.vline(plot.center().x, plot.y_range(), Stroke::new(1.0, p.border_strong));
    painter.text(pos2(plot.left(), rect.bottom()), egui::Align2::LEFT_BOTTOM, format!("{:.0} ms", -lim), font_mono(9.5), p.text_faint);
    painter.text(pos2(plot.right(), rect.bottom()), egui::Align2::RIGHT_BOTTOM, format!("+{lim:.0} ms"), font_mono(9.5), p.text_faint);
    let outside = res.iter().filter(|r| r.abs() > lim).count();
    painter.text(pos2(plot.right(), rect.top()), egui::Align2::RIGHT_TOP, format!("typical {mad:.1} ms"), font_mono(10.0), p.text_muted);
    if outside > 0 {
        painter.text(pos2(plot.right(), rect.top() + 14.0), egui::Align2::RIGHT_TOP, format!("{outside} off the chart"), egui::FontId::proportional(10.0), p.pick);
    }
}

fn amp_bars(ui: &mut Ui, p: &Palette, rect: egui::Rect, src: &BTreeMap<i32, f32>, rec: &BTreeMap<i32, f32>) {
    let painter = ui.painter_at(rect);
    let half = rect.height() * 0.5 - 6.0;
    let rows = [(src, rect.top() + half * 0.5 + 2.0, p.info, "shots"), (rec, rect.bottom() - half * 0.5 - 2.0, p.series_c, "receivers")];
    let db = |v: f32| 20.0 * v.max(1e-6).log10();
    let dmax = src.values().chain(rec.values()).map(|v| db(*v).abs()).fold(0.5f32, f32::max).min(24.0);
    for (m, y0, col, label) in rows {
        painter.hline(rect.x_range(), y0, Stroke::new(1.0, p.border_strong));
        painter.text(pos2(rect.left(), y0 - half * 0.5), egui::Align2::LEFT_TOP, label, egui::FontId::proportional(10.5), col);
        if m.is_empty() {
            continue;
        }
        let n = m.len();
        let step = (n / (rect.width() as usize / 2).max(1)).max(1);
        let bars = n.div_ceil(step);
        let bw = rect.width() / bars as f32;
        for (i, (_, v)) in m.iter().step_by(step).enumerate() {
            let d = db(*v).clamp(-dmax, dmax);
            let h = d / dmax * half * 0.45;
            let x = rect.left() + i as f32 * bw;
            let c = if d.abs() > 6.0 { p.pick } else { with_alpha(col, 200) };
            let (a, b) = if h >= 0.0 { (y0 - h, y0) } else { (y0, y0 - h) };
            painter.rect_filled(egui::Rect::from_min_max(pos2(x + 0.3, a), pos2(x + bw - 0.3, b)), CornerRadius::ZERO, c);
        }
    }
    painter.text(pos2(rect.right(), rect.top()), egui::Align2::RIGHT_TOP, format!("±{dmax:.0} dB"), font_mono(9.5), p.text_faint);
}

pub fn inspector_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let (st_names, sc_names, fb_names) = match s.session() {
        Some(sess) => (sorted_keys(&sess.context.statics), sorted_keys(&sess.context.sc_amps), sorted_keys(&sess.context.first_breaks)),
        None => Default::default(),
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new("Statics").font(crate::theme::font_display(18.0)).color(p.text));
            ui.label(RichText::new("Time shifts and amplitude terms for every shot and receiver.").size(11.5).color(p.text_muted));
            caps(ui, &p, "SHOWING", None);
            for (label, names, cur, id) in [
                ("Statics", &st_names, &mut s.statics.statics_table, "st"),
                ("Amplitude terms", &sc_names, &mut s.statics.scamp_table, "sc"),
                ("First breaks", &fb_names, &mut s.statics.fb_table, "fb"),
            ] {
                widgets::row(ui, &p, label, |ui| {
                    egui::ComboBox::from_id_salt(("stat_sel", id)).selected_text(cur.clone().unwrap_or_else(|| "none yet".into())).width(150.0).show_ui(ui, |ui| {
                        for n in names {
                            ui.selectable_value(cur, Some(n.clone()), n);
                        }
                    });
                });
            }
            // Stations to check
            let stat = s.session().and_then(|x| s.statics.statics_table.as_ref().and_then(|n| x.context.statics.get(n))).cloned();
            if let Some(t) = stat {
                let src = outliers(&t.source_statics_ms);
                let rec = outliers(&t.receiver_statics_ms);
                caps(ui, &p, "NEEDS A LOOK", Some(&(src.len() + rec.len()).to_string()));
                if src.is_empty() && rec.is_empty() {
                    widgets::hint(ui, &p, icon::CHECK_CIRCLE, "Every station sits close to its neighbours.");
                }
                egui::Grid::new("stat_out").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
                    for (kind, list) in [("Shot", &src), ("Receiver", &rec)] {
                        for (k, v) in list.iter().take(8) {
                            ui.label(RichText::new(kind).size(11.5).color(p.text_muted));
                            ui.label(widgets::mono(k.to_string(), 11.5, p.text));
                            ui.label(widgets::mono(format!("{v:+.1} ms"), 11.5, p.pick));
                            ui.end_row();
                        }
                    }
                });
            }
            caps(ui, &p, "NEXT", None);
            if cmd(ui, &p, icon::LIGHTNING, "Pick first breaks", false).clicked() {
                s.viewer.mode = super::viewer::PickMode::FirstBreak;
                s.doc_request = Some(Doc::Viewer);
            }
            if cmd(ui, &p, icon::COLUMNS, "Compare stacks before and after", false).on_hover_text("Opens the gather view side by side").clicked() {
                s.viewer.compare = true;
                s.doc_request = Some(Doc::Viewer);
            }
        });
    });
}

fn linear_fit(pts: &[[f64; 2]]) -> Option<(f64, f64)> {
    if pts.len() < 4 {
        return None;
    }
    let n = pts.len() as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |a, q| (a.0 + q[0], a.1 + q[1]));
    let (mx, my) = (sx / n, sy / n);
    let (mut num, mut den) = (0.0, 0.0);
    for q in pts {
        num += (q[0] - mx) * (q[1] - my);
        den += (q[0] - mx).powi(2);
    }
    if den <= 0.0 {
        return None;
    }
    let slope = num / den;
    Some((my - slope * mx, slope))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    #[test]
    fn linear_fit_recovers_refractor_velocity() {
        let pts: Vec<[f64; 2]> = (0..50)
            .map(|i| {
                let x = i as f64 * 50.0;
                [x, 20.0 + x / 2.8]
            })
            .collect();
        let (t0, slope) = super::linear_fit(&pts).unwrap();
        assert!((t0 - 20.0).abs() < 1e-6);
        assert!((1000.0 / slope - 2800.0).abs() < 1e-3);
    }

    #[test]
    fn outliers_flag_only_the_spike() {
        let mut m: BTreeMap<i32, f32> = (0..40).map(|k| (k, (k as f32 * 0.3).sin())).collect();
        m.insert(17, 25.0);
        let out = super::outliers(&m);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 17);
    }
}
