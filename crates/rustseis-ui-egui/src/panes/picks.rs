//! Workspace 2 · side panes: pick tables (mute / first break / kill) and trace header inspector.

use super::viewer::{self, KillMode, PickMode};
use crate::state::{AppState, Level};
use crate::theme::{font_mono, palette, with_alpha};
use crate::widgets::{self, chip, primary_button, section, segmented};
use egui::{RichText, ScrollArea, Ui};
use egui_phosphor::regular as icon;
use rustseis_core::MuteMode;

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    widgets::toolbar(ui, &p, |ui| {
        segmented(
            ui,
            &p,
            &mut s.viewer.mode,
            &[
                (PickMode::Navigate, "Navigate", "Crosshair only"),
                (PickMode::Mute, "Mute", "Top / bottom / surgical mute picking"),
                (PickMode::FirstBreak, "FB", "First-break picking"),
                (PickMode::Kill, "Kill", "Trace kill / reverse"),
            ],
        );
    });
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        widgets::body(ui, |ui| match s.viewer.mode {
            PickMode::Navigate => navigate_help(s, ui),
            PickMode::Mute => mute_panel(s, ui),
            PickMode::FirstBreak => fb_panel(s, ui),
            PickMode::Kill => kill_panel(s, ui),
        });
    });
    let _ = p;
}

fn navigate_help(s: &AppState, ui: &mut Ui) {
    let p = palette(s.theme);
    section(ui, p, "Navigation", None);
    for (k, v) in [
        ("Wheel", "zoom around pointer"),
        ("Shift + wheel", "scroll traces"),
        ("Drag / middle-drag", "pan"),
        ("Double-click", "fit"),
        ("[  ]", "previous / next ensemble"),
        ("Click", "commit crosshair to the engine bus"),
    ] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(k).font(font_mono(11.0)).color(p.accent));
            ui.label(RichText::new(v).size(12.0).color(p.text_muted));
        });
    }
    ui.add_space(8.0);
    widgets::hint(ui, p, icon::INFO, "Pick modes keep middle-drag for panning; the primary button edits picks.");
}

fn mute_panel(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    section(ui, &p, "Mute table", Some("→ UpsertMuteTable"));
    let existing: Vec<String> = s.session().map(|x| {
        let mut v: Vec<String> = x.context.mutes.keys().cloned().collect();
        v.sort();
        v
    }).unwrap_or_default();
    egui::Grid::new("mute_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut s.viewer.mute_table_name).font(font_mono(12.0)).desired_width(130.0));
            if !existing.is_empty() {
                egui::ComboBox::from_id_salt("mute_load").selected_text("load").width(60.0).show_ui(ui, |ui| {
                    for n in &existing {
                        if ui.selectable_label(false, n).clicked() {
                            if let Some(t) = s.session().and_then(|x| x.context.mutes.get(n)).cloned() {
                                s.viewer.load_mute(&t);
                            }
                        }
                    }
                });
            }
        });
        ui.end_row();
        ui.label("Mode");
        segmented(ui, &p, &mut s.viewer.mute_mode, &[(MuteMode::Top, "Top", "Mute above the curve"), (MuteMode::Bottom, "Bottom", "Mute below the curve"), (MuteMode::Surgical, "Surgical", "Mute inside a window")]);
        ui.end_row();
        ui.label("Key header");
        egui::ComboBox::from_id_salt("mute_key").selected_text(&s.viewer.mute_key).show_ui(ui, |ui| {
            for k in ["AOFFSET", "OFFSET", "CHAN", "CDP", "REC_DEP"] {
                ui.selectable_value(&mut s.viewer.mute_key, k.to_string(), k);
            }
        });
        ui.end_row();
        ui.label("Taper");
        ui.add(egui::DragValue::new(&mut s.viewer.mute_taper_ms).range(0.0..=500.0).suffix(" ms"));
        ui.end_row();
    });
    ui.checkbox(&mut s.viewer.mute_preview, "Preview mute applied to the display");
    ui.add_space(6.0);
    let n = if s.viewer.mute_mode == MuteMode::Surgical { s.viewer.surgical_picks.len() } else { s.viewer.mute_picks.len() };
    ui.horizontal(|ui| {
        chip(ui, &format!("{n} picks"), p.pick, with_alpha(p.pick, 22));
        if ui.small_button(format!("{} clear", icon::TRASH)).clicked() {
            s.viewer.mute_picks.clear();
            s.viewer.surgical_picks.clear();
        }
    });
    egui::Grid::new("mute_picks").striped(true).num_columns(3).spacing([12.0, 2.0]).show(ui, |ui| {
        ui.label(RichText::new(&s.viewer.mute_key).size(11.0).color(p.text_muted));
        ui.label(RichText::new("t (ms)").size(11.0).color(p.text_muted));
        ui.label("");
        ui.end_row();
        let mut remove = None;
        if s.viewer.mute_mode == MuteMode::Surgical {
            for (i, (k, a, b)) in s.viewer.surgical_picks.iter_mut().enumerate() {
                ui.add(egui::DragValue::new(k).speed(1.0));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(a).speed(1.0));
                    ui.add(egui::DragValue::new(b).speed(1.0));
                });
                if ui.small_button(icon::X).clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
            if let Some(i) = remove {
                s.viewer.surgical_picks.remove(i);
            }
        } else {
            for (i, (k, t)) in s.viewer.mute_picks.iter_mut().enumerate() {
                ui.add(egui::DragValue::new(k).speed(1.0));
                ui.add(egui::DragValue::new(t).speed(1.0));
                if ui.small_button(icon::X).clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
            if let Some(i) = remove {
                s.viewer.mute_picks.remove(i);
            }
        }
    });
    ui.add_space(8.0);
    ui.add_enabled_ui(n > 0 && !s.is_busy(), |ui| {
        if primary_button(ui, &p, icon::FLOPPY_DISK, "Save mute table").clicked() {
            viewer::save_mute(s);
        }
    });
    widgets::hint(ui, &p, icon::INFO, "Click empty space to add a pick, drag a handle to move it, right-click a handle to delete.");
}

fn fb_panel(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    section(ui, &p, "First-break picks", Some("→ UpsertFirstBreakTable"));
    egui::Grid::new("fb_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Table");
        ui.add(egui::TextEdit::singleline(&mut s.viewer.fb_table_name).font(font_mono(12.0)).desired_width(150.0));
        ui.end_row();
        ui.label("Method");
        let fp = &mut s.viewer.fb_params;
        segmented(ui, &p, &mut fp.method, &[(rustseis_imaging::FirstBreakMethod::Aic, "AIC", "Akaike information criterion"), (rustseis_imaging::FirstBreakMethod::ModifiedCoppens, "Coppens", "Modified Coppens energy ratio")]);
        ui.end_row();
        ui.label("Velocity corridor");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut fp.v_min_mps).range(100.0..=9000.0).speed(10.0).suffix(" m/s"));
            ui.label("–");
            ui.add(egui::DragValue::new(&mut fp.v_max_mps).range(100.0..=12000.0).speed(10.0).suffix(" m/s"));
        });
        ui.end_row();
        ui.label("Pads");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut fp.early_pad_ms).range(0.0..=500.0).suffix(" ms early"));
            ui.add(egui::DragValue::new(&mut fp.late_pad_ms).range(0.0..=1000.0).suffix(" ms late"));
        });
        ui.end_row();
        ui.label("Min SNR");
        ui.add(egui::DragValue::new(&mut fp.min_snr).range(0.0..=50.0).speed(0.05));
        ui.end_row();
    });
    ui.checkbox(&mut s.viewer.fb_show_header, "Show existing FB_PICK header values");
    ui.add_space(6.0);
    let n = s.viewer.fb_picks.len();
    ui.horizontal(|ui| {
        chip(ui, &format!("{n} picks"), p.pick, with_alpha(p.pick, 22));
        let busy = s.compute.is_busy("viewer.fb_auto");
        ui.add_enabled_ui(!busy && s.session().is_some(), |ui| {
            if ui.button(format!("{}  Auto-pick ensemble", icon::LIGHTNING)).clicked() {
                viewer::auto_pick_fb(s);
            }
        });
        if busy {
            ui.add(egui::Spinner::new().size(12.0));
        }
        if ui.small_button(format!("{} clear", icon::TRASH)).clicked() {
            s.viewer.fb_picks.clear();
        }
    });
    let tables: Vec<String> = s.session().map(|x| x.context.first_breaks.keys().cloned().collect()).unwrap_or_default();
    if !tables.is_empty() {
        egui::ComboBox::from_id_salt("fb_load").selected_text("Load existing table…").show_ui(ui, |ui| {
            for n in &tables {
                if ui.selectable_label(false, n).clicked() {
                    if let Some(t) = s.session().and_then(|x| x.context.first_breaks.get(n)).cloned() {
                        for pk in t.picks {
                            s.viewer.fb_picks.insert((pk.ffid, pk.chan), pk.time_ms);
                        }
                        s.viewer.fb_table_name = n.clone();
                        s.toast(Level::Ok, format!("Loaded {n}"));
                    }
                }
            }
        });
    }
    ui.add_space(8.0);
    ui.add_enabled_ui(n > 0 && !s.is_busy(), |ui| {
        if primary_button(ui, &p, icon::FLOPPY_DISK, "Save first-break table").clicked() {
            viewer::save_first_breaks(s);
        }
    });
    widgets::hint(ui, &p, icon::INFO, "Click or drag across traces to pick; right-drag erases. Picks are keyed by FFID + CHAN.");
}

fn kill_panel(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    section(ui, &p, "Trace kill / reverse", Some("→ RunFlow(trc_edit)"));
    segmented(
        ui,
        &p,
        &mut s.viewer.kill_mode,
        &[
            (KillMode::KillTrace, "Kill trace", "Toggle kill by TRACENO"),
            (KillMode::KillChannel, "Kill channel", "Toggle kill for this CHAN in every shot"),
            (KillMode::ReverseChannel, "Reverse channel", "Toggle polarity reversal for this CHAN"),
        ],
    );
    ui.add_space(6.0);
    let fmt = |set: &std::collections::BTreeSet<i32>| -> String {
        if set.is_empty() { "—".into() } else { set.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ") }
    };
    egui::Grid::new("kill_grid").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
        ui.label(RichText::new("Kill TRACENO").color(p.text_muted));
        ui.label(RichText::new(fmt(&s.viewer.kill_tracenos)).font(font_mono(11.0)).color(p.danger));
        ui.end_row();
        ui.label(RichText::new("Kill CHAN").color(p.text_muted));
        ui.label(RichText::new(fmt(&s.viewer.kill_channels)).font(font_mono(11.0)).color(p.danger));
        ui.end_row();
        ui.label(RichText::new("Reverse CHAN").color(p.text_muted));
        ui.label(RichText::new(fmt(&s.viewer.reverse_channels)).font(font_mono(11.0)).color(p.info));
        ui.end_row();
    });
    ui.add_space(8.0);
    let n = s.viewer.pending_edits();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(n > 0 && !s.is_busy(), |ui| {
            if primary_button(ui, &p, icon::CHECK, "Apply edits").on_hover_text("Runs a one-step trc_edit flow on the active traces — undoable with Ctrl+Z").clicked() {
                viewer::apply_trace_edits(s);
            }
        });
        if ui.button("Clear").clicked() {
            s.viewer.kill_tracenos.clear();
            s.viewer.kill_channels.clear();
            s.viewer.reverse_channels.clear();
        }
    });
    widgets::hint(ui, &p, icon::WARNING, "Killing zeroes samples. The engine keeps an undo snapshot for every flow run.");
}

/// Trace header inspector for the crosshair trace.
pub fn headers_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let id = egui::Id::unique("hdr_filter");
    let mut q: String = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    widgets::toolbar(ui, &p, |ui| {
        ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(p.text_muted));
        let w = ui.available_width();
        ui.add(egui::TextEdit::singleline(&mut q).hint_text("filter headers").desired_width(w));
    });
    ui.data_mut(|d| d.insert_temp(id, q.clone()));
    let Some(sess) = s.session() else {
        super::busy_notice(s, ui);
        return;
    };
    let Some(ti) = s.crosshair.trace_index else {
        crate::canvas::empty_state(ui, &p, icon::CROSSHAIR, "No trace selected", "Hover or click a trace in any view");
        return;
    };
    let Some(tr) = s.focus.source.traces(sess).get(ti) else { return };
    let Ok(serde_json::Value::Object(map)) = serde_json::to_value(&tr.headers) else { return };
    let ql = q.to_ascii_lowercase();
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        widgets::body(ui, |ui| {
            ui.label(RichText::new(format!("Trace #{ti} · {} samples · dt {} µs", tr.samples.len(), tr.dt_us)).font(font_mono(11.0)).color(p.text_muted));
            egui::Grid::new("hdr_grid").striped(true).num_columns(2).spacing([14.0, 1.0]).show(ui, |ui| {
                for (k, v) in &map {
                    if k == "custom" {
                        continue;
                    }
                    if !ql.is_empty() && !k.contains(&ql) {
                        continue;
                    }
                    let zero = v.as_f64() == Some(0.0);
                    ui.label(RichText::new(k.to_ascii_uppercase()).font(font_mono(11.0)).color(if zero { p.text_faint } else { p.text_muted }));
                    ui.label(RichText::new(v.to_string()).font(font_mono(11.0)).color(if zero { p.text_faint } else { p.text }));
                    ui.end_row();
                }
                for (k, v) in &tr.headers.custom {
                    if !ql.is_empty() && !k.to_ascii_lowercase().contains(&ql) {
                        continue;
                    }
                    ui.label(RichText::new(k).font(font_mono(11.0)).color(p.accent));
                    ui.label(RichText::new(format!("{v:.3}")).font(font_mono(11.0)));
                    ui.end_row();
                }
            });
        });
    });
}
