//! Workspace 1 · bottom: job console (live progress, step timings, audit log, messages).

use crate::data::{fmt_count, fmt_ms};
use crate::state::{AppState, Level};
use crate::theme::{font_mono, palette, with_alpha};
use crate::widgets::{self, chip, segmented};
use egui::{RichText, ScrollArea, Ui};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;
use rustseis_engine::CommandPolicy;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Steps,
    Jobs,
    Audit,
    Log,
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let id = egui::Id::unique("console_tab");
    let mut tab: Tab = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    widgets::toolbar(ui, &p, |ui| {
        segmented(
            ui,
            &p,
            &mut tab,
            &[
                (Tab::Steps, "Step report", "Per-step FlowReport telemetry"),
                (Tab::Jobs, "Jobs", "Engine job history"),
                (Tab::Audit, "Audit log", "EngineSession.audit_log"),
                (Tab::Log, "Messages", "Workstation log"),
            ],
        );
        widgets::vsep(ui, &p);
        match s.busy_info() {
            Some((label, el, eta)) => {
                ui.add(egui::Spinner::new().size(13.0).color(p.accent));
                ui.label(RichText::new(label).strong());
                let txt = match eta {
                    Some(e) => format!("{} of ~{}", fmt_ms(el.as_secs_f64() * 1000.0), fmt_ms(e as f64)),
                    None => fmt_ms(el.as_secs_f64() * 1000.0),
                };
                ui.label(RichText::new(txt).font(font_mono(11.0)).color(p.text_muted));
                let frac = eta.map(|e| (el.as_secs_f64() * 1000.0 / e.max(1) as f64).min(0.97) as f32);
                widgets::progress_line(ui, &p, 140.0, frac);
            }
            None => {
                ui.label(RichText::new(format!("{} engine idle", icon::CHECK)).color(p.success).size(11.5));
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(id, tab));

    match tab {
        Tab::Steps => steps(s, ui, &p),
        Tab::Jobs => jobs(s, ui, &p),
        Tab::Audit => audit(s, ui, &p),
        Tab::Log => log(s, ui, &p),
    }
}

fn steps(s: &AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    let Some(rep) = &s.report else {
        widgets::body(ui, |ui| widgets::hint(ui, p, icon::INFO, "Run a flow to see per-step trace counts, timings and sample statistics."));
        return;
    };
    let total = rep.total_elapsed_ms.max(1) as f32;
    widgets::body(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&rep.flow_name).strong());
            chip(ui, &format!("{} traces", fmt_count(rep.final_trace_count)), p.success, with_alpha(p.success, 20));
            chip(ui, &fmt_ms(rep.total_elapsed_ms as f64), p.info, with_alpha(p.info, 20));
            if !rep.semblance_panels.is_empty() {
                chip(ui, &format!("{} semblance", rep.semblance_panels.len()), p.text_muted, p.sunken);
            }
            if !rep.radon_panels.is_empty() {
                chip(ui, &format!("{} radon", rep.radon_panels.len()), p.text_muted, p.sunken);
            }
            if !rep.vsp_profiles.is_empty() {
                chip(ui, &format!("{} vsp", rep.vsp_profiles.len()), p.text_muted, p.sunken);
            }
        });
    });
    TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .column(Column::exact(28.0))
        .column(Column::initial(140.0).at_least(80.0))
        .column(Column::initial(150.0))
        .column(Column::initial(86.0))
        .column(Column::remainder().at_least(120.0))
        .column(Column::initial(110.0))
        .header(20.0, |mut h| {
            for t in ["#", "Tool", "Traces in → out", "Time", "Share", "RMS / peak"] {
                h.col(|ui| {
                    ui.label(RichText::new(t).size(11.0).color(p.text_muted));
                });
            }
        })
        .body(|body| {
            body.rows(20.0, rep.steps.len(), |mut row| {
                let st = &rep.steps[row.index()];
                row.col(|ui| {
                    ui.label(RichText::new(format!("{:02}", st.index + 1)).font(font_mono(11.0)).color(p.text_faint));
                });
                row.col(|ui| {
                    let c = if st.enabled { p.text } else { p.text_faint };
                    ui.label(RichText::new(&st.tool).font(font_mono(11.5)).color(c));
                });
                row.col(|ui| {
                    ui.label(RichText::new(format!("{} → {}", fmt_count(st.traces_in), fmt_count(st.traces_out))).font(font_mono(11.0)));
                });
                row.col(|ui| {
                    ui.label(RichText::new(if st.enabled { fmt_ms(st.elapsed_ms as f64) } else { "bypass".into() }).font(font_mono(11.0)));
                });
                row.col(|ui| {
                    let frac = st.elapsed_ms as f32 / total;
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width() - 4.0, 8.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, p.sunken);
                    let mut r = rect;
                    r.set_width((rect.width() * frac).max(1.0));
                    ui.painter().rect_filled(r, 2.0, if frac > 0.25 { p.pick } else { p.accent });
                });
                row.col(|ui| {
                    if let Some(ss) = &st.sample_stats {
                        ui.label(RichText::new(format!("{:.3} / {:.2}", ss.rms, ss.peak_abs)).font(font_mono(11.0)).color(p.text_muted));
                    }
                });
            });
        });
}

fn jobs(s: &AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    if s.history.is_empty() {
        widgets::body(ui, |ui| widgets::hint(ui, p, icon::INFO, "No engine jobs yet."));
        return;
    }
    ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        widgets::body(ui, |ui| {
            for j in &s.history {
                ui.horizontal(|ui| {
                    let (ic, c) = if j.ok { (icon::CHECK_CIRCLE, p.success) } else { (icon::WARNING, p.danger) };
                    ui.label(RichText::new(ic).color(c));
                    ui.label(RichText::new(j.finished.hms()).font(font_mono(10.5)).color(p.text_faint));
                    ui.label(RichText::new(&j.label).strong());
                    ui.label(RichText::new(fmt_ms(j.elapsed.as_secs_f64() * 1000.0)).font(font_mono(11.0)).color(p.info));
                    ui.label(RichText::new(&j.summary).font(font_mono(10.5)).color(p.text_muted));
                });
            }
        });
    });
}

fn audit(s: &AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    TableBuilder::new(ui)
        .striped(true)
        .stick_to_bottom(true)
        .column(Column::exact(150.0))
        .column(Column::exact(86.0))
        .column(Column::remainder())
        .header(20.0, |mut h| {
            for t in ["Timestamp (UTC)", "Policy", "Command"] {
                h.col(|ui| {
                    ui.label(RichText::new(t).size(11.0).color(p.text_muted));
                });
            }
        })
        .body(|body| {
            body.rows(19.0, s.audit.len(), |mut row| {
                let e = &s.audit[row.index()];
                row.col(|ui| {
                    ui.label(RichText::new(e.timestamp.format("%H:%M:%S%.3f").to_string()).font(font_mono(11.0)).color(p.text_faint));
                });
                row.col(|ui| {
                    let (t, c) = match e.policy {
                        CommandPolicy::ReadOnly => ("read-only", p.text_muted),
                        CommandPolicy::Mutating => ("mutating", p.pick),
                    };
                    chip(ui, t, c, with_alpha(c, 18));
                });
                row.col(|ui| {
                    ui.label(RichText::new(&e.summary).font(font_mono(11.0)));
                });
            });
        });
}

fn log(s: &AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        widgets::body(ui, |ui| {
            for l in &s.log {
                let c = match l.level {
                    Level::Info => p.text_muted,
                    Level::Ok => p.success,
                    Level::Warn => p.pick,
                    Level::Error => p.danger,
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(l.stamp.hms()).font(font_mono(10.5)).color(p.text_faint));
                    ui.label(RichText::new(&l.text).font(font_mono(11.5)).color(c));
                });
            }
        });
    });
}
