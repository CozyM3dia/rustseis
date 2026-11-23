//! Step inspector: typed controls for the selected flow step.

use crate::specs::{SpecParam, ToolSpec};
use crate::state::AppState;
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets::{self, chip};
use egui::{CollapsingHeader, RichText, ScrollArea, Ui};
use egui_phosphor::regular as icon;
use serde_json::{Value, json};

#[derive(Default)]
pub struct InspectorState {
    pub raw_json: bool,
    /// Show the whole flow as editable text instead of cards.
    pub flow_json: bool,
    raw_text: Option<(u64, String)>,
    last_undo_at: f64,
}

/// Known enum choices for string params, keyed by (tool, field) with field-only fallback.
pub fn enum_choices(tool: &str, key: &str) -> Option<&'static [&'static str]> {
    Some(match (tool, key) {
        (_, "spec_type") => &["Ormsby", "Butterworth"],
        (_, "phase") => &["Zero", "Minimum"],
        ("fk_filt", "mode") => &["RejectLowVelocity", "PassHighVelocity", "RejectPolygon", "PassPolygon"],
        ("radon_trans" | "radon_filter" | "taup", "mode") => &["Parabolic", "Linear"],
        (_, "action") => &["SubtractZone", "PassZone"],
        ("fxdecon", "method") => &["WienerLevinson", "L2Adaptive"],
        ("fbpick" | "vsp_slowness", "method") => &["Aic", "ModifiedCoppens"],
        ("autostat" | "pwr_stat_new", "method") => &["Correlation", "MaxPower"],
        ("vsp_wavefield_sep", "method") => &["MedianAlign", "FkDip"],
        ("stack", "method") => &["Mean", "Median"],
        ("fkmig2d", "method") => &["stolt", "phase_shift"],
        (_, "output") => &["Upgoing", "Downgoing", "UpgoingTwoWayAligned"],
        (_, "direction") => &["Forward", "Inverse"],
        ("agc", "basis") => &["Rms", "Mean", "Median"],
        ("teq", "basis") => &["Rms", "Mean", "Peak"],
        (_, "anchor") => &["Start", "Center", "End"],
        (_, "spherical") => &["None", "OneOverDist", "OneOverTV2"],
        (_, "measure") => &["Rms", "MeanAbs", "EnvelopeMedian"],
        ("mute", "mode") => &["Top", "Bottom", "Surgical"],
        _ => return None,
    })
}

const HEADER_FIELDS: &[&str] = &["header", "key", "primary", "secondary", "ensemble_key", "target", "sort_order", "key_header"];
pub const HEADER_NAMES: &[&str] = &[
    "FFID", "CHAN", "CDP", "OFFSET", "AOFFSET", "SOU_SLOC", "REC_SLOC", "TRACENO", "CDP_X", "CDP_Y", "SOU_X", "SOU_Y",
    "REC_X", "REC_Y", "REC_DEP", "FB_PICK", "TOT_STAT", "CDP_NFLD", "ILINE_NO", "XLINE_NO", "LINE",
];

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let Some(path) = s.flow_sel.clone() else {
        crate::canvas::empty_state(ui, &p, icon::SLIDERS_HORIZONTAL, "Nothing selected", "Click a step in the flow to change its settings.");
        return;
    };
    let Some(node) = s.flow.get(&path).cloned() else {
        s.flow_sel = None;
        return;
    };
    let desc = s.tool(&node.tool).cloned();
    let spec = s.specs.get(&node.tool).cloned();
    let names = TableNames::from(s);

    egui::Frame::new().inner_margin(egui::Margin { left: 14, right: 14, top: 12, bottom: 4 }).show(ui, |ui| {
        ui.label(RichText::new(desc.as_ref().map(|d| d.label.clone()).unwrap_or(node.tool.clone())).font(crate::theme::font_display(17.0)).color(p.text));
        ui.horizontal(|ui| {
            if let Some(d) = &desc {
                let col = super::catalog::category_color(&p, &d.category);
                chip(ui, &d.category, col, with_alpha(col, 22));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut raw = s.inspector.raw_json;
                if crate::widgets::segmented(ui, &p, &mut raw, &[(true, "Text", "Edit the settings as text"), (false, "Form", "Controls")]) {
                    s.inspector.raw_json = raw;
                    s.inspector.raw_text = None;
                }
            });
        });
    });

    let mut node_mut = node.clone();
    let mut changed = false;
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        widgets::body(ui, |ui| {
            if let Some(d) = &desc {
                ui.label(RichText::new(crate::ui_copy::clean(&d.description)).size(12.0).color(p.text_muted));
            }
            if let Some(r) = last_run(s, &path) {
                ui.add_space(4.0);
                egui::Frame::new().fill(p.sunken).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(RichText::new("LAST RUN").font(crate::theme::font_semibold(10.5)).color(p.text_faint));
                    let traces = if r.traces_in == r.traces_out { format!("{} traces", crate::data::fmt_count(r.traces_out)) } else { format!("{} → {} traces", crate::data::fmt_count(r.traces_in), crate::data::fmt_count(r.traces_out)) };
                    ui.label(RichText::new(format!("{traces} · {}", crate::data::fmt_ms(r.elapsed_ms as f64))).font(font_mono(11.5)).color(p.text));
                    if let Some(st) = &r.sample_stats {
                        ui.label(RichText::new(format!("signal level (rms) {:.4}", st.rms)).size(11.0).color(p.text_muted));
                    }
                });
            }
            ui.add_space(6.0);
            egui::Grid::new("step_meta").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("Use this step").color(p.text_muted));
                changed |= widgets::toggle(ui, &p, &mut node_mut.enabled).changed();
                ui.end_row();
                ui.label(RichText::new("Name in the flow").color(p.text_muted));
                let mut label = node_mut.label.clone().unwrap_or_default();
                if ui.add(egui::TextEdit::singleline(&mut label).hint_text("optional")).changed() {
                    node_mut.label = if label.trim().is_empty() { None } else { Some(label) };
                    changed = true;
                }
                ui.end_row();
            });

            ui.add_space(4.0);
            widgets::caps(ui, &p, "SETTINGS", None);
            if s.inspector.raw_json {
                changed |= raw_editor(s, ui, &p, &mut node_mut);
            } else {
                let tool = node_mut.tool.clone();
                if node_mut.tool == "python_node" {
                    changed |= python_editor(ui, &p, &mut node_mut.params);
                }
                changed |= edit_object(ui, &p, &tool, "", &mut node_mut.params, spec.as_ref(), &names, 0);
                if crate::flowdoc::is_container(&tool) {
                    widgets::hint(ui, &p, icon::GIT_BRANCH, "Branch steps are edited as nested blocks in the flow.");
                }
            }

            if let Some(sp) = &spec {
                reference(ui, &p, sp);
            }
        });
    });

    if changed {
        if s.time - s.inspector.last_undo_at > 0.6 || s.flow_undo.is_empty() {
            s.push_flow_undo();
            s.inspector.last_undo_at = s.time;
        }
        if let Some(n) = s.flow.get_mut(&path) {
            *n = node_mut;
            s.flow.dirty = true;
        }
    }
}

/// Telemetry for a top-level step from the last run, if the flow has not changed shape since.
fn last_run(s: &AppState, path: &[usize]) -> Option<rustseis_flow::StepReport> {
    if path.len() != 1 {
        return None;
    }
    let i = path[0];
    let uid = s.flow.nodes.get(i)?.uid;
    if s.report_uids.get(i) != Some(&uid) {
        return None;
    }
    s.report.as_ref()?.steps.get(i).cloned()
}

/// Names of session tables for reference-field suggestions.
pub struct TableNames {
    pub velocities: Vec<String>,
    pub mutes: Vec<String>,
    pub buffers: Vec<String>,
    pub others: Vec<String>,
}

impl TableNames {
    fn from(s: &AppState) -> Self {
        let mut t = Self { velocities: vec![], mutes: vec![], buffers: vec![], others: vec![] };
        if let Some(sess) = s.session() {
            let c = &sess.context;
            t.velocities = c.velocities.keys().cloned().collect();
            t.mutes = c.mutes.keys().cloned().collect();
            t.buffers = c.buffers.keys().cloned().collect();
            t.others = c.first_breaks.keys().chain(c.statics.keys()).chain(c.sc_amps.keys()).chain(c.radon_panels.keys()).cloned().collect();
            t.velocities.sort();
            t.mutes.sort();
            t.buffers.sort();
        }
        t
    }
    fn for_key(&self, key: &str) -> Option<&[String]> {
        match key {
            "velocity_table" => Some(&self.velocities),
            "mute_table" => Some(&self.mutes),
            "model_buffer" => Some(&self.buffers),
            _ => None,
        }
    }
}

pub fn humanize(key: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut unit = None;
    let parts: Vec<&str> = key.split('_').collect();
    for (i, w) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let u = match *w {
            "ms" => Some("ms"),
            "m" => Some("m"),
            "mps" => Some("m/s"),
            "hz" => Some("Hz"),
            "pct" => Some("%"),
            "us" => Some("µs"),
            "deg" => Some("°"),
            "cpm" => Some("cyc/m"),
            "db" => Some("dB"),
            _ => None,
        };
        if last && u.is_some() && i > 0 {
            unit = u;
            continue;
        }
        let mut c = w.chars();
        words.push(match c.next() {
            Some(f) if i == 0 => f.to_uppercase().collect::<String>() + c.as_str(),
            Some(f) => f.to_string() + c.as_str(),
            None => String::new(),
        });
    }
    let base = words.join(" ");
    match unit {
        Some(u) => format!("{base} ({u})"),
        None => base,
    }
}

fn unit_suffix(key: &str) -> &'static str {
    let k = key.to_ascii_lowercase();
    if k.ends_with("_ms") { " ms" } else if k.ends_with("_mps") { " m/s" } else if k.ends_with("_hz") { " Hz" } else if k.ends_with("_pct") { " %" } else if k.ends_with("_us") { " µs" } else if k.ends_with("_deg") { "°" } else if k.ends_with("_m") { " m" } else { "" }
}

/// Fuzzy match of a JSON key to a documented parameter (shared word stems).
fn match_spec<'a>(spec: Option<&'a ToolSpec>, key: &str) -> Option<&'a SpecParam> {
    let spec = spec?;
    let toks: Vec<String> = key
        .split('_')
        .filter(|t| t.len() >= 3 && !matches!(*t, "mps" | "pct"))
        .map(|t| t.to_ascii_lowercase())
        .collect();
    if toks.is_empty() {
        return None;
    }
    let mut best: Option<(usize, &SpecParam)> = None;
    for prm in &spec.params {
        let hay = format!("{} {}", prm.prompt, prm.name).to_ascii_lowercase();
        let score = toks.iter().filter(|t| hay.contains(t.as_str()) || hay.contains(&t[..t.len().min(5)])).count();
        if score > 0 && best.is_none_or(|(b, _)| score > b) {
            best = Some((score, prm));
        }
    }
    best.map(|(_, p)| p)
}

#[allow(clippy::too_many_arguments)]
fn edit_object(
    ui: &mut Ui,
    p: &Palette,
    tool: &str,
    prefix: &str,
    v: &mut Value,
    spec: Option<&ToolSpec>,
    names: &TableNames,
    depth: usize,
) -> bool {
    let Some(obj) = v.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    let keys: Vec<String> = obj.keys().cloned().collect();
    let (nested, flat): (Vec<String>, Vec<String>) = keys.into_iter().partition(|k| obj.get(k).is_some_and(|x| x.is_object()));
    egui::Grid::new(("params", tool, prefix, depth)).num_columns(2).spacing([10.0, 5.0]).min_col_width(110.0).show(ui, |ui| {
        for k in &flat {
            if crate::flowdoc::is_container(tool) && matches!(k.as_str(), "then_steps" | "else_steps" | "branches") {
                continue;
            }
            if tool == "python_node" && k == "inline_code" {
                continue;
            }
            let Some(val) = obj.get_mut(k) else { continue };
            let sp = match_spec(spec, k);
            let lbl = ui.label(RichText::new(humanize(k)).color(p.text_muted));
            lbl.on_hover_ui(|ui| {
                ui.set_max_width(320.0);
                match sp {
                    Some(sp) => {
                        ui.label(RichText::new(&sp.prompt).strong());
                        if !sp.help.is_empty() {
                            ui.label(crate::ui_copy::clean(&sp.help));
                        }
                    }
                    None => {
                        ui.label(RichText::new(humanize(k)).strong());
                    }
                }
            });
            changed |= edit_value(ui, p, tool, k, val, names);
            ui.end_row();
        }
    });
    for k in &nested {
        let Some(val) = obj.get_mut(k) else { continue };
        CollapsingHeader::new(RichText::new(humanize(k)).strong())
            .id_salt(("nested", tool, prefix, k.as_str()))
            .default_open(true)
            .show(ui, |ui| {
                changed |= edit_object(ui, p, tool, &format!("{prefix}{k}."), val, spec, names, depth + 1);
            });
    }
    changed
}

fn edit_value(ui: &mut Ui, p: &Palette, tool: &str, key: &str, v: &mut Value, names: &TableNames) -> bool {
    match v {
        Value::Bool(b) => ui.checkbox(b, "").changed(),
        Value::Number(n) => {
            if n.is_f64() {
                let mut x = n.as_f64().unwrap_or(0.0);
                let speed = (x.abs() * 0.005).clamp(0.001, 50.0);
                let r = ui.add(egui::DragValue::new(&mut x).speed(speed).suffix(unit_suffix(key)).max_decimals(4));
                if r.changed() {
                    *v = json!(x);
                    return true;
                }
                false
            } else {
                let mut x = n.as_i64().unwrap_or(0);
                let r = ui.add(egui::DragValue::new(&mut x).speed(0.2).suffix(unit_suffix(key)));
                if r.changed() {
                    *v = json!(x);
                    return true;
                }
                false
            }
        }
        Value::String(sv) => edit_string(ui, p, tool, key, sv, names),
        Value::Null => {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new("auto").font(font_mono(11.0)).color(p.text_faint));
                if ui.small_button("set").on_hover_text("Replace the default `null` with an explicit value").clicked() {
                    *v = default_for_null_key(key);
                    changed = true;
                }
            });
            changed
        }
        Value::Array(arr) => edit_array(ui, p, key, arr),
        Value::Object(_) => false,
    }
}

/// Infer a sensible non-null default JSON value for an optional parameter key.
pub fn default_for_null_key(key: &str) -> Value {
    let k = key.to_ascii_lowercase();
    if k.starts_with("save_")
        || k.ends_with("_table")
        || k.ends_with("_panel")
        || k.ends_with("_name")
        || k.ends_with("_path")
        || k.ends_with("_key")
        || k.ends_with("_header")
        || k.ends_with("_buffer")
        || k.ends_with("_code")
        || matches!(
            k.as_str(),
            "path" | "name" | "header" | "key" | "method" | "mode" | "output" | "direction" | "basis" | "anchor" | "python_exe" | "label"
        )
    {
        json!("")
    } else if k.starts_with("max_") || k.starts_with("min_") && k.contains("traces") || k.contains("count") || k.contains("shots") || k.contains("iters") {
        json!(100)
    } else {
        json!(0.0)
    }
}

fn edit_string(ui: &mut Ui, p: &Palette, tool: &str, key: &str, sv: &mut String, names: &TableNames) -> bool {
    if let Some(choices) = enum_choices(tool, key) {
        let mut changed = false;
        egui::ComboBox::from_id_salt(("enum", tool, key)).selected_text(sv.as_str()).width(170.0).show_ui(ui, |ui| {
            for c in choices {
                changed |= ui.selectable_value(sv, c.to_string(), *c).changed();
            }
        });
        return changed;
    }
    if HEADER_FIELDS.contains(&key) {
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui.add(egui::TextEdit::singleline(sv).font(font_mono(12.0)).desired_width(110.0)).changed();
            egui::ComboBox::from_id_salt(("hdr", tool, key)).selected_text("").width(24.0).show_ui(ui, |ui| {
                for h in HEADER_NAMES {
                    if ui.selectable_label(sv == h, *h).clicked() {
                        *sv = h.to_string();
                        changed = true;
                    }
                }
            });
        });
        return changed;
    }
    if key == "path" || key.ends_with("_path") || key == "python_exe" {
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui.add(egui::TextEdit::singleline(sv).font(font_mono(11.0)).desired_width(190.0)).on_hover_text(sv.as_str()).changed();
            if ui.small_button(icon::FILE).on_hover_text("Pick file").clicked() {
                let dlg = rfd::FileDialog::new();
                let pick = if tool.ends_with("_write") || tool == "diskwrite" { dlg.save_file() } else { dlg.pick_file() };
                if let Some(f) = pick {
                    *sv = f.display().to_string();
                    changed = true;
                }
            }
            if tool == "segd_read" && ui.small_button(icon::FOLDER).on_hover_text("Pick shot directory").clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    *sv = d.display().to_string();
                    changed = true;
                }
            }
        });
        return changed;
    }
    if let Some(list) = names.for_key(key) {
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui.add(egui::TextEdit::singleline(sv).font(font_mono(12.0)).desired_width(130.0)).changed();
            if !list.is_empty() {
                egui::ComboBox::from_id_salt(("tbl", key)).selected_text("").width(24.0).show_ui(ui, |ui| {
                    for n in list {
                        if ui.selectable_label(sv == n, n).clicked() {
                            *sv = n.clone();
                            changed = true;
                        }
                    }
                });
            }
        });
        return changed;
    }
    let _ = p;
    ui.add(egui::TextEdit::singleline(sv).font(font_mono(12.0)).desired_width(180.0)).changed()
}

fn edit_array(ui: &mut Ui, p: &Palette, key: &str, arr: &mut Vec<Value>) -> bool {
    let all_num = !arr.is_empty() && arr.iter().all(|x| x.is_number());
    if all_num && arr.len() <= 8 {
        let mut changed = false;
        ui.horizontal(|ui| {
            for x in arr.iter_mut() {
                let mut f = x.as_f64().unwrap_or(0.0);
                let speed = (f.abs() * 0.01).clamp(0.01, 50.0);
                if ui.add(egui::DragValue::new(&mut f).speed(speed).max_decimals(3)).changed() {
                    *x = if x.is_i64() || x.is_u64() { json!(f.round() as i64) } else { json!(f) };
                    changed = true;
                }
            }
        });
        return changed;
    }
    let rows_numeric = arr.iter().all(|r| r.as_array().is_some_and(|c| c.iter().all(|x| x.is_number())));
    if rows_numeric && (!arr.is_empty() || matches!(key, "picks" | "windows" | "gates_ms" | "design_windows_ms" | "time_variant_zone" | "polygon" | "stations" | "slalom_vertices" | "surgical_picks")) {
        let width = arr.first().and_then(|r| r.as_array()).map(|r| r.len()).unwrap_or(match key {
            "time_variant_zone" | "surgical_picks" => 3,
            "stations" => 4,
            _ => 2,
        });
        let mut changed = false;
        let mut remove: Option<usize> = None;
        ui.vertical(|ui| {
            for (ri, row) in arr.iter_mut().enumerate().take(64) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{ri:>2}")).font(font_mono(10.5)).color(p.text_faint));
                    if let Some(cells) = row.as_array_mut() {
                        for c in cells.iter_mut() {
                            let mut f = c.as_f64().unwrap_or(0.0);
                            let speed = (f.abs() * 0.01).clamp(0.1, 50.0);
                            if ui.add(egui::DragValue::new(&mut f).speed(speed).max_decimals(2)).changed() {
                                *c = json!(f);
                                changed = true;
                            }
                        }
                    }
                    if ui.small_button(icon::X).clicked() {
                        remove = Some(ri);
                    }
                });
            }
            if arr.len() > 64 {
                ui.label(RichText::new(format!("… {} more rows (edit in JSON view)", arr.len() - 64)).size(11.0).color(p.text_faint));
            }
            if ui.small_button(format!("{} row", icon::PLUS)).clicked() {
                let last = arr.last().cloned().unwrap_or_else(|| json!(vec![0.0; width]));
                arr.push(last);
                changed = true;
            }
        });
        if let Some(r) = remove {
            arr.remove(r);
            changed = true;
        }
        return changed;
    }
    // Fallback: inline JSON
    let id = egui::Id::unique(("arr_json", key));
    let mut text: String = ui.data(|d| d.get_temp(id)).unwrap_or_else(|| Value::Array(arr.clone()).to_string());
    let mut changed = false;
    let r = ui.add(egui::TextEdit::singleline(&mut text).font(font_mono(11.0)).desired_width(200.0));
    if r.lost_focus() {
        if let Ok(Value::Array(new)) = serde_json::from_str::<Value>(&text) {
            *arr = new;
            changed = true;
        }
        ui.data_mut(|d| d.remove::<String>(id));
    } else if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    }
    changed
}

fn python_editor(ui: &mut Ui, p: &Palette, params: &mut Value) -> bool {
    let mut changed = false;
    let use_file = params.get("script_path").is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()));
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::FILE_PY).color(p.pick));
        ui.label(RichText::new(if use_file { "Script file mode (inline code ignored)" } else { "Inline code — def process(traces, params): return traces" }).size(11.5).color(p.text_muted));
    });
    if !use_file {
        let mut code = params.get("inline_code").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let r = ui.add(
            egui::TextEdit::multiline(&mut code)
                .font(font_mono(12.0))
                .code_editor()
                .desired_rows(12)
                .desired_width(f32::INFINITY),
        );
        if r.changed() {
            params["inline_code"] = json!(code);
            changed = true;
        }
    }
    changed
}

fn raw_editor(s: &mut AppState, ui: &mut Ui, p: &Palette, node: &mut crate::flowdoc::FlowNode) -> bool {
    let uid = node.uid;
    let text = match &s.inspector.raw_text {
        Some((u, t)) if *u == uid => t.clone(),
        _ => serde_json::to_string_pretty(&node.params).unwrap_or_default(),
    };
    let mut text = text;
    ui.add(egui::TextEdit::multiline(&mut text).font(font_mono(11.5)).code_editor().desired_rows(18).desired_width(f32::INFINITY));
    let parsed = serde_json::from_str::<Value>(&text);
    let mut changed = false;
    ui.horizontal(|ui| match &parsed {
        Ok(v) => {
            if ui.button(format!("{}  Apply", icon::CHECK)).clicked() {
                node.params = v.clone();
                changed = true;
            }
            ui.label(RichText::new("looks valid").color(p.success).size(11.0));
        }
        Err(e) => {
            ui.label(RichText::new(format!("{} {e}", icon::WARNING)).color(p.danger).size(11.0));
        }
    });
    s.inspector.raw_text = if changed { None } else { Some((uid, text)) };
    changed
}

fn reference(ui: &mut Ui, p: &Palette, spec: &ToolSpec) {
    ui.add_space(8.0);
    CollapsingHeader::new(RichText::new(format!("{}  About these settings", icon::INFO)).color(p.text_muted))
        .id_salt(("ref", &spec.menu_id))
        .default_open(false)
        .show(ui, |ui| {
            for prm in &spec.params {
                ui.add_space(3.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(crate::ui_copy::clean(&prm.prompt)).strong().size(12.0));
                    if let Some(d) = &prm.default {
                        ui.label(RichText::new(format!("= {d}")).font(font_mono(10.5)).color(p.text_faint));
                    }
                });
                if !prm.help.is_empty() {
                    ui.label(RichText::new(crate::ui_copy::clean(&prm.help)).size(11.0).color(p.text_muted));
                }
                if !prm.choices.is_empty() {
                    ui.label(RichText::new(prm.choices.iter().map(|c| c.0.as_str()).collect::<Vec<_>>().join(" · ")).size(10.5).color(p.text_faint));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_extracts_units() {
        assert_eq!(humanize("operator_ms"), "Operator (ms)");
        assert_eq!(humanize("vel_cut_mps"), "Vel cut (m/s)");
        assert_eq!(humanize("chan1_is_near"), "Chan1 is near");
    }

    #[test]
    fn null_defaults_match_field_types() {
        assert_eq!(default_for_null_key("save_table"), json!(""));
        assert_eq!(default_for_null_key("save_panel"), json!(""));
        assert_eq!(default_for_null_key("velocity_table"), json!(""));
        assert_eq!(default_for_null_key("path"), json!(""));
        assert_eq!(default_for_null_key("max_shots"), json!(100));
        assert_eq!(default_for_null_key("length_ms"), json!(0.0));
    }
}
