//! Inspector while picking on the gather view (mute, first breaks, trace edits) and the trace
//! header list.

use super::viewer::{self, KillMode, PickMode};
use crate::state::{AppState, Level};
use crate::theme::{Palette, font_mono, palette};
use crate::widgets::{self, caps, cmd, cmd_primary, mono};
use egui::{RichText, ScrollArea, Ui};
use egui_phosphor::regular as icon;
use egui_plot::{Line, Plot, PlotPoints, Points};
use rustseis_core::MuteMode;

/// Inspector for the gather view: picking panels while picking, the gather inspector otherwise.
pub fn inspector_ui(s: &mut AppState, ui: &mut Ui) {
    if s.viewer.mode == PickMode::Navigate {
        viewer::inspector_ui(s, ui);
        return;
    }
    let p = *palette(s.theme);
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            match s.viewer.mode {
                PickMode::Mute => mute_panel(s, ui, &p),
                PickMode::FirstBreak => fb_panel(s, ui, &p),
                PickMode::Kill => kill_panel(s, ui, &p),
                PickMode::Navigate => {}
            }
        });
    });
}

fn title(ui: &mut Ui, p: &Palette, t: &str, sub: &str) {
    ui.label(RichText::new(t).font(crate::theme::font_display(18.0)).color(p.text));
    ui.label(RichText::new(sub).size(11.5).color(p.text_muted));
}

fn mute_panel(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    title(ui, p, "Mute", "Click the gather to add a point, drag to move it, right-click to remove it. Middle-drag pans.");
    let existing: Vec<String> = s
        .session()
        .map(|x| {
            let mut v: Vec<String> = x.context.mutes.keys().cloned().collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    widgets::row(ui, p, "Name", |ui| {
        ui.add(egui::TextEdit::singleline(&mut s.viewer.mute_table_name).font(font_mono(12.0)).desired_width(150.0));
    });
    widgets::row(ui, p, "Measured along", |ui| {
        egui::ComboBox::from_id_salt("mute_key").selected_text(&s.viewer.mute_key).width(110.0).show_ui(ui, |ui| {
            for k in ["AOFFSET", "OFFSET", "CHAN", "CDP", "REC_DEP"] {
                ui.selectable_value(&mut s.viewer.mute_key, k.to_string(), k);
            }
        });
    });
    widgets::row(ui, p, "Taper", |ui| {
        ui.add(egui::DragValue::new(&mut s.viewer.mute_taper_ms).range(0.0..=500.0).suffix(" ms"));
    });
    widgets::row(ui, p, "Show the muted result", |ui| {
        widgets::toggle(ui, p, &mut s.viewer.mute_preview);
    });

    // Mute vs key chart
    let n = if s.viewer.mute_mode == MuteMode::Surgical { s.viewer.surgical_picks.len() } else { s.viewer.mute_picks.len() };
    caps(ui, p, &format!("MUTE AGAINST {}", s.viewer.mute_key), Some(&format!("{n} points")));
    let pts: Vec<[f64; 2]> = if s.viewer.mute_mode == MuteMode::Surgical {
        s.viewer.surgical_picks.iter().map(|(k, a, _)| [*k as f64, *a as f64]).collect()
    } else {
        s.viewer.mute_picks.iter().map(|(k, t)| [*k as f64, *t as f64]).collect()
    };
    let lower: Vec<[f64; 2]> = s.viewer.surgical_picks.iter().map(|(k, _, b)| [*k as f64, *b as f64]).collect();
    Plot::new("mute_chart").height(150.0).invert_y(true).show_background(false).allow_drag(false).allow_scroll(false).x_axis_label(s.viewer.mute_key.clone()).y_axis_label("ms").show(ui, |pu| {
        if pts.len() >= 2 {
            pu.line(Line::new("mute", PlotPoints::from(pts.clone())).color(p.pick).width(2.0));
        }
        if lower.len() >= 2 && s.viewer.mute_mode == MuteMode::Surgical {
            pu.line(Line::new("window end", PlotPoints::from(lower)).color(p.pick).width(1.2));
        }
        pu.points(Points::new("points", PlotPoints::from(pts)).color(p.pick).radius(3.5));
    });

    // Points
    let mut remove = None;
    egui::Frame::new().fill(p.sunken).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        if n == 0 {
            ui.label(RichText::new("No points yet").size(11.5).color(p.text_muted));
        }
        egui::Grid::new("mute_picks").num_columns(3).spacing([12.0, 3.0]).show(ui, |ui| {
            if s.viewer.mute_mode == MuteMode::Surgical {
                for (i, (k, a, b)) in s.viewer.surgical_picks.iter_mut().enumerate() {
                    ui.add(egui::DragValue::new(k).speed(1.0));
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(a).speed(1.0).suffix(" ms"));
                        ui.add(egui::DragValue::new(b).speed(1.0).suffix(" ms"));
                    });
                    if ui.small_button(icon::X).clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            } else {
                for (i, (k, t)) in s.viewer.mute_picks.iter_mut().enumerate() {
                    ui.add(egui::DragValue::new(k).speed(1.0));
                    ui.add(egui::DragValue::new(t).speed(1.0).suffix(" ms"));
                    if ui.small_button(icon::X).clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            }
        });
    });
    if let Some(i) = remove {
        if s.viewer.mute_mode == MuteMode::Surgical {
            s.viewer.surgical_picks.remove(i);
        } else {
            s.viewer.mute_picks.remove(i);
        }
    }
    ui.horizontal(|ui| {
        ui.add_enabled_ui(n > 0 && !s.is_busy(), |ui| {
            if cmd_primary(ui, p, icon::CHECK, "Save mute").clicked() {
                viewer::save_mute(s);
            }
        });
        if cmd(ui, p, icon::ERASER, "Clear", false).clicked() {
            s.viewer.mute_picks.clear();
            s.viewer.surgical_picks.clear();
        }
        if !existing.is_empty() {
            let r = cmd(ui, p, icon::FOLDER_OPEN, "Load  ▾", false);
            egui::Popup::menu(&r).show(|ui| {
                for name in &existing {
                    if ui.button(name).clicked() {
                        if let Some(t) = s.session().and_then(|x| x.context.mutes.get(name)).cloned() {
                            s.viewer.load_mute(&t);
                        }
                    }
                }
            });
        }
    });
}

fn fb_panel(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    title(ui, p, "First breaks", "Click or drag across traces to pick, right-drag to erase. Picks follow shot and channel.");
    let n = s.viewer.fb_picks.len();
    ui.horizontal(|ui| {
        ui.label(mono(n.to_string(), 22.0, p.pick));
        ui.label(RichText::new("picks so far").color(p.text_muted));
    });
    caps(ui, p, "AUTOMATIC PICKER", None);
    {
        let fp = &mut s.viewer.fb_params;
        widgets::row(ui, p, "Method", |ui| {
            egui::ComboBox::from_id_salt("fb_method").selected_text(match fp.method {
                rustseis_imaging::FirstBreakMethod::Aic => "Information criterion",
                rustseis_imaging::FirstBreakMethod::ModifiedCoppens => "Energy ratio",
            }).width(150.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut fp.method, rustseis_imaging::FirstBreakMethod::Aic, "Information criterion");
                ui.selectable_value(&mut fp.method, rustseis_imaging::FirstBreakMethod::ModifiedCoppens, "Energy ratio");
            });
        });
        // widgets::row lays children right-to-left, so add right-side DragValue first.
        widgets::row(ui, p, "Velocity range", |ui| {
            ui.add(egui::DragValue::new(&mut fp.v_max_mps).range(100.0..=12000.0).speed(10.0).suffix(" m/s"));
            ui.label("–");
            ui.add(egui::DragValue::new(&mut fp.v_min_mps).range(100.0..=9000.0).speed(10.0).suffix(" m/s"));
        });
        widgets::row(ui, p, "Search before / after", |ui| {
            ui.add(egui::DragValue::new(&mut fp.late_pad_ms).range(0.0..=1000.0).suffix(" ms"));
            ui.add(egui::DragValue::new(&mut fp.early_pad_ms).range(0.0..=500.0).suffix(" ms"));
        });
        widgets::row(ui, p, "Minimum signal-to-noise", |ui| {
            ui.add(egui::DragValue::new(&mut fp.min_snr).range(0.0..=50.0).speed(0.05));
        });
    }
    widgets::row(ui, p, "Show picks stored in headers", |ui| {
        widgets::toggle(ui, p, &mut s.viewer.fb_show_header);
    });
    let busy = s.compute.is_busy("viewer.fb_auto");
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!busy && s.session().is_some(), |ui| {
            if cmd(ui, p, icon::SPARKLE, "Auto-pick this gather", false).clicked() {
                viewer::auto_pick_fb(s);
            }
        });
        if busy {
            ui.add(egui::Spinner::new().size(14.0));
        }
    });
    caps(ui, p, "TABLE", None);
    widgets::row(ui, p, "Name", |ui| {
        ui.add(egui::TextEdit::singleline(&mut s.viewer.fb_table_name).font(font_mono(12.0)).desired_width(150.0));
    });
    let tables: Vec<String> = s.session().map(|x| x.context.first_breaks.keys().cloned().collect()).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(n > 0 && !s.is_busy(), |ui| {
            if cmd_primary(ui, p, icon::CHECK, "Save first breaks").clicked() {
                viewer::save_first_breaks(s);
            }
        });
        if cmd(ui, p, icon::ERASER, "Clear", false).clicked() {
            s.viewer.fb_picks.clear();
        }
        if !tables.is_empty() {
            let r = cmd(ui, p, icon::FOLDER_OPEN, "Load  ▾", false);
            egui::Popup::menu(&r).show(|ui| {
                for name in &tables {
                    if ui.button(name).clicked() {
                        if let Some(t) = s.session().and_then(|x| x.context.first_breaks.get(name)).cloned() {
                            for pk in t.picks {
                                s.viewer.fb_picks.insert((pk.ffid, pk.chan), pk.time_ms);
                            }
                            s.viewer.fb_table_name = name.clone();
                            s.toast(Level::Ok, format!("Loaded first breaks “{name}”"));
                        }
                    }
                }
            });
        }
    });
}

fn kill_panel(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    title(ui, p, "Edit traces", "Click traces to mark them. Nothing changes until you press Done; every edit can be undone.");
    if s.focus.source != crate::data::TraceSource::Active {
        let (go, _) = widgets::insight(
            ui,
            p,
            "Viewing a saved buffer (read-only)",
            "Trace edits only apply to Latest result (Active). Switch to Latest result before marking traces.",
            "Switch to Latest result",
            "",
        );
        if go {
            s.focus.source = crate::data::TraceSource::Active;
            s.focus.ens = 0;
            s.viewer.request_refit = true;
        }
        return;
    }
    let fmt = |set: &std::collections::BTreeSet<i32>| -> String {
        if set.is_empty() { "none".into() } else { set.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ") }
    };
    let what = match s.viewer.kill_mode {
        KillMode::KillTrace => "Clicking silences one trace.",
        KillMode::KillChannel => "Clicking silences that channel in every shot.",
        KillMode::ReverseChannel => "Clicking flips the polarity of that channel in every shot.",
    };
    widgets::hint(ui, p, icon::CURSOR_CLICK, what);
    caps(ui, p, "MARKED", Some(&format!("{}", s.viewer.pending_edits())));
    egui::Grid::new("kill_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label(RichText::new("Silenced traces").color(p.text_muted));
        ui.label(mono(fmt(&s.viewer.kill_tracenos), 11.5, p.danger));
        ui.end_row();
        ui.label(RichText::new("Silenced channels").color(p.text_muted));
        ui.label(mono(fmt(&s.viewer.kill_channels), 11.5, p.danger));
        ui.end_row();
        ui.label(RichText::new("Flipped channels").color(p.text_muted));
        ui.label(mono(fmt(&s.viewer.reverse_channels), 11.5, p.info));
        ui.end_row();
    });
    let dead: Vec<i32> = {
        let st = &s.viewer.stats;
        match (s.session(), s.focus_range()) {
            (Some(sess), Some(r)) => st.dead.iter().filter_map(|i| s.focus.source.traces(sess).get(r.start + i).map(|t| t.headers.traceno)).collect(),
            _ => Vec::new(),
        }
    };
    if !dead.is_empty() {
        let (go, _) = widgets::insight(ui, p, &format!("{} dead traces in this gather", dead.len()), "Mark them all for silencing in one go.", "Mark dead traces", "");
        if go {
            s.viewer.kill_tracenos.extend(dead);
        }
    }
    if cmd(ui, p, icon::ERASER, "Clear marks", false).clicked() {
        s.viewer.kill_tracenos.clear();
        s.viewer.kill_channels.clear();
        s.viewer.reverse_channels.clear();
    }
}

/// Trace header list for the crosshair trace.
pub fn headers_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let id = egui::Id::unique("hdr_filter");
    let mut q: String = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(p.text_muted));
            let w = ui.available_width();
            ui.add(egui::TextEdit::singleline(&mut q).hint_text("Filter headers").desired_width(w));
        });
    });
    ui.data_mut(|d| d.insert_temp(id, q.clone()));
    let Some(sess) = s.session() else {
        super::busy_notice(s, ui);
        return;
    };
    let Some(ti) = s.crosshair.trace_index else {
        crate::canvas::empty_state(ui, &p, icon::CROSSHAIR, "No trace selected", "Hover or click a trace in any view.");
        return;
    };
    let Some(tr) = s.focus.source.traces(sess).get(ti) else { return };
    let Ok(serde_json::Value::Object(map)) = serde_json::to_value(&tr.headers) else { return };
    let ql = q.to_ascii_lowercase();
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 4)).show(ui, |ui| {
            ui.label(RichText::new(format!("Trace {ti} · {} samples · {} µs", tr.samples.len(), tr.dt_us)).size(11.5).color(p.text_muted));
            egui::Grid::new("hdr_grid").striped(true).num_columns(2).spacing([14.0, 2.0]).show(ui, |ui| {
                for (k, v) in &map {
                    if k == "custom" || (!ql.is_empty() && !k.contains(&ql)) {
                        continue;
                    }
                    let zero = v.as_f64() == Some(0.0);
                    ui.label(mono(k.to_ascii_uppercase(), 11.0, if zero { p.text_faint } else { p.text_muted }));
                    ui.label(mono(v.to_string(), 11.0, if zero { p.text_faint } else { p.text }));
                    ui.end_row();
                }
                for (k, v) in &tr.headers.custom {
                    if !ql.is_empty() && !k.to_ascii_lowercase().contains(&ql) {
                        continue;
                    }
                    ui.label(mono(k, 11.0, p.accent_text));
                    ui.label(mono(format!("{v:.3}"), 11.0, p.text));
                    ui.end_row();
                }
            });
        });
    });
}
