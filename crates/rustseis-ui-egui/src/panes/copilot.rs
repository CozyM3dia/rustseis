//! Workspace 6 · Copilot: an offline MCP tool console over the live session, reusing
//! `rustseis_mcp::handle_tool_call` with explicit approval for mutating commands.

use crate::state::{AppState, JobPurpose, Level, compact_json};
use crate::theme::{Palette, RADIUS_MD, font_mono, mix, palette, with_alpha};
use crate::widgets::{self, chip, primary_button};
use crate::worker::EngineJob;
use egui::{CornerRadius, Frame, Margin, RichText, ScrollArea, Stroke, Ui};
use egui_phosphor::regular as icon;
use rustseis_engine::CommandPolicy;
use serde_json::{Map, Value, json};

#[derive(Clone, Debug)]
pub enum Status {
    AwaitingApproval,
    Declined,
    Running,
    Done(Result<Value, String>),
}

#[derive(Clone, Debug)]
pub struct Call {
    pub id: u64,
    pub tool: String,
    pub args: Value,
    pub policy: CommandPolicy,
    pub status: Status,
    pub typed: String,
}

#[derive(Default)]
pub struct CopilotState {
    pub input: String,
    pub calls: Vec<Call>,
    pub form_tool: Option<String>,
    pub form: Map<String, Value>,
    next_id: u64,
    history_pos: Option<usize>,
}

impl CopilotState {
    pub fn on_result(&mut self, call_id: u64, result: Result<Value, String>) {
        if let Some(c) = self.calls.iter_mut().find(|c| c.id == call_id) {
            c.status = Status::Done(result);
        }
    }
}

pub fn policy_of(tool: &str) -> CommandPolicy {
    match tool {
        "run_flow" | "load_opf_db" | "undo" => CommandPolicy::Mutating,
        _ => CommandPolicy::ReadOnly,
    }
}

fn tool_defs() -> Vec<Value> {
    rustseis_mcp::mcp_tool_definitions().as_array().cloned().unwrap_or_default()
}

/// Parse `tool {json}` or `tool key=value key2=value2`.
pub fn parse_command(line: &str) -> Result<(String, Value), String> {
    let line = line.trim();
    if line.is_empty() {
        return Err("empty command".into());
    }
    let (name, rest) = match line.find(|c: char| c.is_whitespace()) {
        Some(i) => (&line[..i], line[i..].trim()),
        None => (line, ""),
    };
    let names: Vec<String> = tool_defs().iter().filter_map(|d| d.get("name").and_then(|n| n.as_str()).map(String::from)).collect();
    if !names.iter().any(|n| n == name) {
        let close: Vec<&String> = names.iter().filter(|n| n.contains(name) || name.contains(n.as_str())).collect();
        return Err(if close.is_empty() {
            format!("unknown tool `{name}` — type `help` for the 13 MCP tools")
        } else {
            format!("unknown tool `{name}` — did you mean {}?", close.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", "))
        });
    }
    if rest.is_empty() {
        return Ok((name.to_string(), json!({})));
    }
    if rest.starts_with('{') {
        let v: Value = serde_json::from_str(rest).map_err(|e| format!("arguments are not valid JSON: {e}"))?;
        return Ok((name.to_string(), v));
    }
    let mut map = Map::new();
    for kv in rest.split_whitespace() {
        let (k, v) = kv.split_once('=').ok_or_else(|| format!("expected key=value, got `{kv}`"))?;
        let val = serde_json::from_str::<Value>(v).unwrap_or_else(|_| Value::String(v.to_string()));
        map.insert(k.to_string(), val);
    }
    Ok((name.to_string(), Value::Object(map)))
}

fn propose(s: &mut AppState, tool: String, args: Value, typed: String) {
    let id = s.copilot.next_id + 1;
    s.copilot.next_id = id;
    let policy = policy_of(&tool);
    let status = if policy == CommandPolicy::Mutating { Status::AwaitingApproval } else { Status::Running };
    s.copilot.calls.push(Call { id, tool: tool.clone(), args: args.clone(), policy, status, typed });
    if policy == CommandPolicy::ReadOnly {
        execute(s, id);
    }
}

fn execute(s: &mut AppState, id: u64) {
    let Some(c) = s.copilot.calls.iter().find(|c| c.id == id).cloned() else { return };
    if s.submit(EngineJob::Mcp { call_id: id, name: c.tool.clone(), args: c.args.clone() }, JobPurpose::Copilot) {
        if let Some(cm) = s.copilot.calls.iter_mut().find(|x| x.id == id) {
            cm.status = Status::Running;
        }
    } else if let Some(cm) = s.copilot.calls.iter_mut().find(|x| x.id == id) {
        cm.status = Status::Done(Err("engine busy — run again when the current job finishes".into()));
    }
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let defs = tool_defs();
    widgets::toolbar(ui, &p, |ui| {
        ui.label(RichText::new(icon::ROBOT).color(p.accent));
        ui.label(RichText::new("MCP tool console").font(crate::theme::font_display(13.0)));
        chip(ui, &format!("{} tools", defs.len()), p.text_muted, p.sunken);
        chip(ui, "offline", p.success, with_alpha(p.success, 20));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("clear").clicked() {
                s.copilot.calls.clear();
            }
        });
    });

    // Quick actions
    widgets::body(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let quick: [(&str, &str, Value); 5] = [
                (icon::TABLE, "inspect_tables", json!({})),
                (icon::WAVEFORM, "inspect_active_traces", json!({})),
                (icon::PUZZLE_PIECE, "list_processing_tools", json!({})),
                (icon::ARROW_COUNTER_CLOCKWISE, "undo", json!({})),
                (icon::FILE, "inspect_seismic_file", json!({"path": ""})),
            ];
            for (ic, name, args) in quick {
                if ui.button(RichText::new(format!("{ic}  {name}")).size(11.5)).clicked() {
                    if name == "inspect_seismic_file" {
                        if let Some(f) = rfd::FileDialog::new().add_filter("Seismic", &["sgy", "segy", "segd"]).pick_file() {
                            propose(s, name.into(), json!({"path": f.display().to_string()}), String::new());
                        }
                    } else {
                        propose(s, name.into(), args, String::new());
                    }
                }
            }
            if ui.button(RichText::new(format!("{}  run_flow ← editor", icon::PLAY)).size(11.5)).on_hover_text("Send the Flow Studio document through MCP run_flow").clicked() {
                let spec = serde_json::to_value(s.flow.to_spec()).unwrap_or(json!({}));
                propose(s, "run_flow".into(), json!({"flow": spec}), String::new());
            }
        });
    });

    let input_h = 118.0;
    let avail = ui.available_height() - input_h;
    let mut approve: Option<u64> = None;
    let mut decline: Option<u64> = None;
    let mut undo = false;
    ScrollArea::vertical().max_height(avail.max(80.0)).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        widgets::body(ui, |ui| {
            if s.copilot.calls.is_empty() {
                intro(ui, &p, &defs);
            }
            for c in &s.copilot.calls {
                call_card(ui, &p, c, &mut approve, &mut decline, &mut undo);
                ui.add_space(6.0);
            }
        });
    });
    if let Some(id) = approve {
        s.log(Level::Info, format!("Copilot: user approved mutating call #{id}"));
        execute(s, id);
    }
    if let Some(id) = decline {
        if let Some(c) = s.copilot.calls.iter_mut().find(|c| c.id == id) {
            c.status = Status::Declined;
        }
    }
    if undo {
        s.undo();
    }
    composer(s, ui, &p, &defs);
}

fn intro(ui: &mut Ui, p: &Palette, defs: &[Value]) {
    ui.label(RichText::new("Drive the live session with the same 13 tools the stdio MCP server exposes.").color(p.text_muted));
    ui.label(RichText::new("Type `tool {json}` or `tool key=value`, or build a call from the form below. Mutating calls (run_flow, load_opf_db, undo) wait for your approval.").size(11.5).color(p.text_faint));
    ui.add_space(6.0);
    egui::Grid::new("mcp_tools").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
        for d in defs {
            let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let desc = d.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let mutating = policy_of(name) == CommandPolicy::Mutating;
            ui.label(RichText::new(name).font(font_mono(11.5)).color(if mutating { p.pick } else { p.accent }));
            ui.label(RichText::new(desc).size(11.5).color(p.text_muted));
            ui.end_row();
        }
    });
}

fn call_card(ui: &mut Ui, p: &Palette, c: &Call, approve: &mut Option<u64>, decline: &mut Option<u64>, undo: &mut bool) {
    let mutating = c.policy == CommandPolicy::Mutating;
    let border = match &c.status {
        Status::AwaitingApproval => p.pick,
        Status::Done(Err(_)) => p.danger,
        Status::Declined => p.border,
        _ => p.border,
    };
    if !c.typed.is_empty() {
        ui.horizontal(|ui| {
            ui.label(RichText::new("›").font(font_mono(12.0)).color(p.text_faint));
            ui.label(RichText::new(&c.typed).font(font_mono(11.5)).color(p.text_muted));
        });
    }
    Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::LIGHTNING).color(if mutating { p.pick } else { p.accent }));
                ui.label(RichText::new(&c.tool).font(font_mono(12.0)).strong());
                if mutating {
                    chip(ui, "mutating", p.pick, with_alpha(p.pick, 22));
                } else {
                    chip(ui, "read-only", p.text_muted, p.sunken);
                }
                ui.label(RichText::new(format!("#{}", c.id)).font(font_mono(10.5)).color(p.text_faint));
            });
            let args = compact_json(&c.args, 240);
            if args != "{}" {
                ui.label(RichText::new(args).font(font_mono(10.5)).color(p.text_muted));
            }
            match &c.status {
                Status::AwaitingApproval => {
                    widgets::banner(ui, p, p.pick, icon::SHIELD_CHECK, "This call changes the session (traces or OPF tables). Allow it?");
                    ui.horizontal(|ui| {
                        if primary_button(ui, p, icon::CHECK, "Allow").clicked() {
                            *approve = Some(c.id);
                        }
                        if ui.button(format!("{}  Decline", icon::X)).clicked() {
                            *decline = Some(c.id);
                        }
                    });
                }
                Status::Declined => {
                    ui.label(RichText::new(format!("{} declined — nothing was changed", icon::PROHIBIT)).color(p.text_faint).size(11.5));
                }
                Status::Running => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label(RichText::new("running on the engine worker…").size(11.5).color(p.text_muted));
                    });
                }
                Status::Done(Ok(v)) => result_view(ui, p, c, v, undo),
                Status::Done(Err(e)) => {
                    ui.label(RichText::new(format!("{} {e}", icon::WARNING)).color(p.danger).font(font_mono(11.0)));
                }
            }
        });
}

fn result_view(ui: &mut Ui, p: &Palette, c: &Call, v: &Value, undo: &mut bool) {
    // Friendly summaries for the chattiest tools.
    let summary: Option<String> = match c.tool.as_str() {
        "list_processing_tools" => v.get("count").map(|n| {
            let names: Vec<&str> = v.get("tools").and_then(|t| t.as_array()).map(|a| a.iter().filter_map(|t| t.get("menu_id").and_then(|m| m.as_str())).collect()).unwrap_or_default();
            format!("{n} tools: {}", names.join(", "))
        }),
        "inspect_active_traces" => Some(format!(
            "{} traces · {} samples · dt {} µs · dominant {:.1} Hz · CDP {}–{}",
            v.get("trace_count").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("n_samples").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("dt_us").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("dominant_freq_hz").and_then(|x| x.as_f64()).unwrap_or(0.0),
            v.get("first_cdp").and_then(|x| x.as_i64()).unwrap_or(0),
            v.get("last_cdp").and_then(|x| x.as_i64()).unwrap_or(0),
        )),
        "run_flow" => v.get("final_trace_count").map(|n| format!("flow finished · {n} traces · {} ms", v.get("total_elapsed_ms").and_then(|x| x.as_u64()).unwrap_or(0))),
        _ => None,
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::CHECK).color(p.success));
        ui.label(RichText::new(summary.clone().unwrap_or_else(|| compact_json(v, 160))).size(11.5));
    });
    ui.horizontal(|ui| {
        egui::CollapsingHeader::new(RichText::new("result JSON").size(11.0).color(p.text_faint)).id_salt(("res", c.id)).show(ui, |ui| {
            let pretty = serde_json::to_string_pretty(v).unwrap_or_default();
            let shown: String = pretty.chars().take(20_000).collect();
            ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                ui.label(RichText::new(shown).font(font_mono(10.5)).color(p.text_muted));
            });
            if ui.small_button(format!("{} copy", icon::COPY)).clicked() {
                ui.ctx().copy_text(pretty);
            }
        });
        if c.policy == CommandPolicy::Mutating && c.tool != "undo" && ui.small_button(format!("{} undo (Ctrl+Z)", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
            *undo = true;
        }
    });
}

fn composer(s: &mut AppState, ui: &mut Ui, p: &Palette, defs: &[Value]) {
    Frame::new().fill(mix(p.panel, p.chrome, 0.5)).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        // Form builder
        ui.horizontal(|ui| {
            ui.label(RichText::new("Form").size(11.0).color(p.text_faint));
            let current = s.copilot.form_tool.clone().unwrap_or_else(|| "choose tool…".into());
            egui::ComboBox::from_id_salt("cp_form_tool").selected_text(RichText::new(current).font(font_mono(11.5))).width(190.0).show_ui(ui, |ui| {
                for d in defs {
                    let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if ui.selectable_label(s.copilot.form_tool.as_deref() == Some(&name), &name).clicked() {
                        s.copilot.form = default_form(d);
                        s.copilot.form_tool = Some(name);
                    }
                }
            });
            if let Some(tool) = s.copilot.form_tool.clone() {
                let schema = defs.iter().find(|d| d.get("name").and_then(|n| n.as_str()) == Some(tool.as_str())).cloned().unwrap_or(json!({}));
                let props = schema.pointer("/inputSchema/properties").and_then(|x| x.as_object()).cloned().unwrap_or_default();
                for (k, spec) in &props {
                    let ty = spec.get("type").and_then(|t| t.as_str()).unwrap_or("string");
                    let entry = s.copilot.form.entry(k.clone()).or_insert(Value::Null);
                    ui.label(RichText::new(k).font(font_mono(10.5)).color(p.text_muted));
                    match ty {
                        "integer" | "number" => {
                            let mut f = entry.as_f64().unwrap_or(0.0);
                            if ui.add(egui::DragValue::new(&mut f).speed(1.0)).changed() {
                                *entry = if ty == "integer" { json!(f.round() as i64) } else { json!(f) };
                            }
                        }
                        "boolean" => {
                            let mut b = entry.as_bool().unwrap_or(false);
                            if ui.checkbox(&mut b, "").changed() {
                                *entry = json!(b);
                            }
                        }
                        "object" => {
                            ui.label(RichText::new("(object — use the command line)").size(10.5).color(p.text_faint));
                        }
                        _ => {
                            let mut st = entry.as_str().unwrap_or("").to_string();
                            if ui.add(egui::TextEdit::singleline(&mut st).desired_width(150.0).font(font_mono(11.0))).changed() {
                                *entry = json!(st);
                            }
                            if k.contains("path") && ui.small_button(icon::FOLDER_OPEN).clicked() {
                                let dlg = rfd::FileDialog::new();
                                let f = if k == "output_path" || tool.starts_with("save") { dlg.save_file() } else { dlg.pick_file() };
                                if let Some(f) = f {
                                    *entry = json!(f.display().to_string());
                                }
                            }
                        }
                    }
                }
                if ui.button(format!("{} call", icon::PAPER_PLANE_RIGHT)).clicked() {
                    let args: Map<String, Value> = s.copilot.form.iter().filter(|(_, v)| !v.is_null() && v.as_str() != Some("")).map(|(k, v)| (k.clone(), v.clone())).collect();
                    propose(s, tool.clone(), Value::Object(args), String::new());
                }
            }
        });
        // Command line
        ui.horizontal(|ui| {
            ui.label(RichText::new("›").font(font_mono(14.0)).color(p.accent));
            let r = ui.add(
                egui::TextEdit::singleline(&mut s.copilot.input)
                    .font(font_mono(12.5))
                    .hint_text("inspect_tables   ·   render_gather_png output_path=qc/gather.png width=1600   ·   help")
                    .desired_width(ui.available_width() - 70.0),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if r.has_focus() {
                let typed: Vec<String> = s.copilot.calls.iter().filter(|c| !c.typed.is_empty()).map(|c| c.typed.clone()).collect();
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !typed.is_empty() {
                    let pos = s.copilot.history_pos.map(|x| x.saturating_sub(1)).unwrap_or(typed.len() - 1);
                    s.copilot.history_pos = Some(pos);
                    s.copilot.input = typed[pos].clone();
                }
            }
            let send = ui.button(format!("{}", icon::PAPER_PLANE_RIGHT)).clicked() || enter;
            if send && !s.copilot.input.trim().is_empty() {
                let line = std::mem::take(&mut s.copilot.input);
                s.copilot.history_pos = None;
                if line.trim() == "help" {
                    s.copilot.calls.clear();
                } else {
                    match parse_command(&line) {
                        Ok((tool, args)) => propose(s, tool, args, line),
                        Err(e) => {
                            let id = s.copilot.next_id + 1;
                            s.copilot.next_id = id;
                            s.copilot.calls.push(Call { id, tool: "parse".into(), args: json!({}), policy: CommandPolicy::ReadOnly, status: Status::Done(Err(e)), typed: line });
                        }
                    }
                }
                r.request_focus();
            }
            // Autocomplete hint
            let first = s.copilot.input.split_whitespace().next().unwrap_or("").to_string();
            if !first.is_empty() && !s.copilot.input.contains(' ') {
                let matches: Vec<String> = defs.iter().filter_map(|d| d.get("name").and_then(|n| n.as_str())).filter(|n| n.starts_with(&first) && *n != first).map(String::from).collect();
                if let Some(m) = matches.first() {
                    if ui.input(|i| i.key_pressed(egui::Key::Tab)) {
                        s.copilot.input = format!("{m} ");
                    }
                }
            }
        });
    });
}

fn default_form(def: &Value) -> Map<String, Value> {
    let mut m = Map::new();
    if let Some(props) = def.pointer("/inputSchema/properties").and_then(|x| x.as_object()) {
        for (k, spec) in props {
            m.insert(k.clone(), spec.get("default").cloned().unwrap_or(Value::Null));
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_and_kv_commands() {
        let (t, a) = parse_command(r#"render_gather_png {"output_path":"a.png","width":800}"#).unwrap();
        assert_eq!(t, "render_gather_png");
        assert_eq!(a["width"], 800);
        let (t, a) = parse_command("render_fk_png output_path=fk.png dx_m=12.5").unwrap();
        assert_eq!(t, "render_fk_png");
        assert_eq!(a["dx_m"], 12.5);
        assert_eq!(a["output_path"], "fk.png");
        assert!(parse_command("inspect").unwrap_err().contains("did you mean"));
    }

    #[test]
    fn mutating_policy_matches_engine() {
        assert_eq!(policy_of("run_flow"), CommandPolicy::Mutating);
        assert_eq!(policy_of("load_opf_db"), CommandPolicy::Mutating);
        assert_eq!(policy_of("inspect_tables"), CommandPolicy::ReadOnly);
    }
}
