//! Workspace 5: statics, surface-consistent amplitude and first-break QC charts.

use crate::state::AppState;
use crate::theme::{font_mono, palette, with_alpha};
use crate::widgets::{self, chip, stat};
use egui::{RichText, Ui};
use egui_phosphor::regular as icon;
use egui_plot::{Legend, Line, Plot, PlotPoints, Points};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct StaticsState {
    pub statics_table: Option<String>,
    pub scamp_table: Option<String>,
    pub fb_table: Option<String>,
    pub scamp_db: bool,
    pub scamp_show: [bool; 5],
}

fn table_picker(ui: &mut Ui, id: &str, current: &mut Option<String>, names: &[String]) {
    if current.as_ref().is_none_or(|c| !names.contains(c)) {
        *current = names.first().cloned();
    }
    egui::ComboBox::from_id_salt(id)
        .selected_text(RichText::new(current.clone().unwrap_or_else(|| "—".into())).font(font_mono(12.0)))
        .width(170.0)
        .show_ui(ui, |ui| {
            for n in names {
                ui.selectable_value(current, Some(n.clone()), n);
            }
        });
}

fn sorted_keys<V>(m: &std::collections::HashMap<String, V>) -> Vec<String> {
    let mut v: Vec<String> = m.keys().cloned().collect();
    v.sort();
    v
}

fn series(m: &BTreeMap<i32, f32>, f: impl Fn(f32) -> f64) -> Vec<[f64; 2]> {
    m.iter().map(|(k, v)| [*k as f64, f(*v)]).collect()
}

pub fn statics_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let Some(sess) = s.session() else {
        super::busy_notice(s, ui);
        return;
    };
    let names = sorted_keys(&sess.context.statics);
    let mut sel = s.statics.statics_table.clone();
    widgets::toolbar(ui, &p, |ui| {
        ui.label(RichText::new("Statics table").size(12.0));
        table_picker(ui, "stat_pick", &mut sel, &names);
    });
    s.statics.statics_table = sel.clone();
    let Some(sess) = s.session() else { return };
    let Some(t) = sel.and_then(|n| sess.context.statics.get(&n)) else {
        crate::canvas::empty_state(ui, &p, icon::RULER, "No statics solutions", "Run refstatic, autostat or pwr_stat_new in a flow");
        return;
    };
    widgets::body(ui, |ui| {
        ui.horizontal(|ui| {
            stat(ui, &p, "Method", &t.kind);
            ui.add_space(14.0);
            stat(ui, &p, "RMS residual", &format!("{:.3} ms", t.rms_residual_ms));
            ui.add_space(14.0);
            if t.refractor_velocity_mps > 0.0 {
                stat(ui, &p, "Refractor velocity", &format!("{:.1} m/s", t.refractor_velocity_mps));
                ui.add_space(14.0);
            }
            stat(ui, &p, "Sources / receivers", &format!("{} / {}", t.source_statics_ms.len(), t.receiver_statics_ms.len()));
            if !t.cdp_structure_ms.is_empty() {
                ui.add_space(14.0);
                stat(ui, &p, "CDP terms", &t.cdp_structure_ms.len().to_string());
            }
        });
    });
    let src = series(&t.source_statics_ms, |v| v as f64);
    let rec = series(&t.receiver_statics_ms, |v| v as f64);
    let cdp = series(&t.cdp_structure_ms, |v| v as f64);
    Plot::new("statics_plot")
        .legend(Legend::default())
        .x_axis_label("station (SOU_SLOC / REC_SLOC / CDP)")
        .y_axis_label("static (ms)")
        .show_background(false)
        .show(ui, |pu| {
            pu.line(Line::new("source", PlotPoints::from(src.clone())).color(p.pick).width(1.5));
            pu.points(Points::new("source pts", PlotPoints::from(src)).color(p.pick).radius(1.8));
            pu.line(Line::new("receiver", PlotPoints::from(rec.clone())).color(p.series_b).width(1.5));
            pu.points(Points::new("receiver pts", PlotPoints::from(rec)).color(p.series_b).radius(1.8));
            if !cdp.is_empty() {
                pu.line(Line::new("CDP structure", PlotPoints::from(cdp)).color(p.series_c).width(1.2));
            }
            pu.hline(egui_plot::HLine::new("0", 0.0).color(with_alpha(p.text, 60)));
        });
}

pub fn scamp_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let Some(sess) = s.session() else {
        super::busy_notice(s, ui);
        return;
    };
    let names = sorted_keys(&sess.context.sc_amps);
    let mut sel = s.statics.scamp_table.clone();
    if s.statics.scamp_show == [false; 5] {
        s.statics.scamp_show = [true, true, true, false, false];
    }
    let mut show = s.statics.scamp_show;
    let mut db = s.statics.scamp_db;
    widgets::toolbar(ui, &p, |ui| {
        ui.label(RichText::new("SC-amp table").size(12.0));
        table_picker(ui, "scamp_pick", &mut sel, &names);
        widgets::vsep(ui, &p);
        for (i, n) in ["source", "receiver", "offset", "CDP", "channel"].iter().enumerate() {
            ui.toggle_value(&mut show[i], *n);
        }
        widgets::vsep(ui, &p);
        ui.checkbox(&mut db, "dB");
    });
    s.statics.scamp_table = sel.clone();
    s.statics.scamp_show = show;
    s.statics.scamp_db = db;
    let Some(sess) = s.session() else { return };
    let Some(t) = sel.and_then(|n| sess.context.sc_amps.get(&n)) else {
        crate::canvas::empty_state(ui, &p, icon::GAUGE, "No surface-consistent amplitude tables", "Run scamp2 in a flow");
        return;
    };
    widgets::body(ui, |ui| {
        ui.horizontal(|ui| {
            stat(ui, &p, "Mean log amp", &format!("{:.4}", t.mean_log_amp));
            ui.add_space(14.0);
            stat(ui, &p, "RMS log residual", &format!("{:.4}", t.rms_log_residual));
            ui.add_space(14.0);
            stat(ui, &p, "Terms", &format!("{} src · {} rec · {} off", t.source_factors.len(), t.receiver_factors.len(), t.offset_factors.len()));
        });
    });
    let conv = |v: f32| if db { 20.0 * (v.max(1e-6) as f64).log10() } else { v as f64 };
    let sets: [(&str, &BTreeMap<i32, f32>, egui::Color32); 5] = [
        ("source", &t.source_factors, p.pick),
        ("receiver", &t.receiver_factors, p.series_b),
        ("offset", &t.offset_factors, p.success),
        ("CDP", &t.cdp_factors, p.series_c),
        ("channel", &t.channel_factors, p.accent),
    ];
    Plot::new("scamp_plot")
        .legend(Legend::default())
        .x_axis_label("domain key")
        .y_axis_label(if db { "gain (dB)" } else { "linear factor" })
        .show_background(false)
        .show(ui, |pu| {
            for (i, (name, m, c)) in sets.iter().enumerate() {
                if show[i] && !m.is_empty() {
                    pu.line(Line::new(*name, PlotPoints::from(series(m, conv))).color(*c).width(1.4));
                }
            }
            pu.hline(egui_plot::HLine::new("unity", if db { 0.0 } else { 1.0 }).color(with_alpha(p.text, 60)));
        });
}

pub fn fb_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let Some(sess) = s.session() else {
        super::busy_notice(s, ui);
        return;
    };
    let names = sorted_keys(&sess.context.first_breaks);
    let mut sel = s.statics.fb_table.clone();
    widgets::toolbar(ui, &p, |ui| {
        ui.label(RichText::new("First-break table").size(12.0));
        table_picker(ui, "fb_pick_tbl", &mut sel, &names);
    });
    s.statics.fb_table = sel.clone();
    let Some(sess) = s.session() else { return };
    let Some(t) = sel.and_then(|n| sess.context.first_breaks.get(&n)) else {
        crate::canvas::empty_state(ui, &p, icon::LIGHTNING, "No first-break tables", "Run fbpick in a flow or pick in the Trace Viewer (FB mode)");
        return;
    };
    let pts: Vec<[f64; 2]> = t.picks.iter().step_by((t.picks.len() / 40_000).max(1)).map(|pk| [pk.offset_m.abs() as f64, pk.time_ms as f64]).collect();
    // Linear refractor fit t = t0 + |x| / v over far offsets.
    let fit = linear_fit(&pts);
    let refr = sess.context.statics.values().find(|x| x.refractor_velocity_mps > 0.0).map(|x| x.refractor_velocity_mps);
    widgets::body(ui, |ui| {
        ui.horizontal(|ui| {
            stat(ui, &p, "Picks", &crate::data::fmt_count(t.picks.len()));
            ui.add_space(14.0);
            if let Some((t0, slope)) = fit {
                stat(ui, &p, "Apparent velocity", &format!("{:.0} m/s", 1000.0 / slope.max(1e-9)));
                ui.add_space(14.0);
                stat(ui, &p, "Intercept", &format!("{t0:.1} ms"));
            }
            if let Some(v) = refr {
                ui.add_space(14.0);
                chip(ui, &format!("refstatic V_refr {v:.0} m/s"), p.success, with_alpha(p.success, 20));
            }
        });
    });
    Plot::new("fb_plot")
        .x_axis_label("|offset| (m)")
        .y_axis_label("first break (ms)")
        .invert_y(true)
        .show_background(false)
        .legend(Legend::default())
        .show(ui, |pu| {
            pu.points(Points::new("picks", PlotPoints::from(pts.clone())).color(with_alpha(p.pick, 170)).radius(1.4));
            if let Some((t0, slope)) = fit {
                let xmax = pts.iter().map(|q| q[0]).fold(0.0, f64::max);
                pu.line(Line::new("linear fit", PlotPoints::from(vec![[0.0, t0], [xmax, t0 + slope * xmax]])).color(p.accent).width(1.6));
            }
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
    #[test]
    fn linear_fit_recovers_refractor_velocity() {
        let pts: Vec<[f64; 2]> = (0..50).map(|i| {
            let x = i as f64 * 50.0;
            [x, 20.0 + x / 2.8]
        }).collect();
        let (t0, slope) = super::linear_fit(&pts).unwrap();
        assert!((t0 - 20.0).abs() < 1e-6);
        assert!((1000.0 / slope - 2800.0).abs() < 1e-3);
    }
}
