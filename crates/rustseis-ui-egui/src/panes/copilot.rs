//! Assistant panel: an offline tool console over the live session (the same tools the MCP server
//! exposes), with an approval card before anything changes the data.

use crate::state::{AppState, JobPurpose, Level, compact_json};
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets;
use crate::worker::EngineJob;
use egui::{CornerRadius, Frame, Margin, RichText, ScrollArea, Stroke, Ui};
use egui_phosphor::regular as icon;
use rustseis_engine::CommandPolicy;
use serde_json::{Map, Value, json};

#[derive(Clone, Debug)]
pub enum Status {
    AwaitingApproval,
    Queued,
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

/// Try to map a plain-English multi-word request into a safe MCP tool call.
fn try_natural_language(line: &str) -> Option<(String, Value)> {
    if !line.contains(char::is_whitespace) {
        return None;
    }
    let low = line.to_ascii_lowercase();
    let nums: Vec<f64> = low
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f64>().ok())
        .collect();
    if low.contains("table") || low.contains("loaded") || low.contains("summary") || low.contains("session") {
        return Some(("inspect_tables".into(), json!({})));
    }
    if low.contains("header") || low.contains("trace") && (low.contains("show") || low.contains("inspect") || low.contains("list")) {
        return Some(("inspect_headers".into(), json!({})));
    }
    if low.contains("tool") || low.contains("catalog") || low.contains("what can") {
        return Some(("list_tools".into(), json!({})));
    }
    if low.contains("bandpass") || (low.contains("filter") && nums.len() >= 2) {
        let freqs = if nums.len() >= 4 {
            [nums[0], nums[1], nums[2], nums[3]]
        } else if nums.len() >= 2 {
            let (lo, hi) = (nums[0].min(nums[1]), nums[0].max(nums[1]));
            [(lo * 0.65).max(1.0), lo, hi, hi * 1.25]
        } else {
            [6.0, 12.0, 60.0, 80.0]
        };
        return Some((
            "run_flow".into(),
            json!({
                "flow": {
                    "name": "assistant_bandpass",
                    "description": line,
                    "steps": [{
                        "tool": "filter",
                        "params": { "spec_type": "Ormsby", "phase": "Zero", "freqs": freqs, "pad_pct": 25.0 }
                    }]
                }
            }),
        ));
    }
    if low.contains("agc") || low.contains("gain") {
        let win = nums.first().copied().unwrap_or(500.0);
        return Some((
            "run_flow".into(),
            json!({
                "flow": {
                    "name": "assistant_agc",
                    "description": line,
                    "steps": [{ "tool": "agc", "params": { "window_ms": win, "basis": "Rms" } }]
                }
            }),
        ));
    }
    if low.contains("undo") {
        return Some(("undo".into(), json!({})));
    }
    None
}

/// Parse `tool {json}`, `tool key=value`, or a plain-English request.
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
        if let Some(mapped) = try_natural_language(line) {
            return Ok(mapped);
        }
        let close: Vec<&String> = names.iter().filter(|n| n.contains(name) || name.contains(n.as_str())).collect();
        return Err(if close.is_empty() {
            format!("I don't know “{name}”. Try plain phrases like “show tables” or “bandpass 10 to 60 Hz”, or pick a tool name above.")
        } else {
            format!("I don't know “{name}”. Did you mean {}?", close.iter().map(|s| format!("“{s}”")).collect::<Vec<_>>().join(", "))
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
    let status = if policy == CommandPolicy::Mutating { Status::AwaitingApproval } else { Status::Queued };
    s.copilot.calls.push(Call { id, tool: tool.clone(), args: args.clone(), policy, status, typed });
    pump(s);
}

/// Start the oldest queued call if the engine is free (calls run one at a time, in order).
pub fn pump(s: &mut AppState) {
    if s.is_busy() {
        return;
    }
    if let Some(id) = s.copilot.calls.iter().find(|c| matches!(c.status, Status::Queued)).map(|c| c.id) {
        execute(s, id);
    }
}

/// Seed the transcript for the screenshot tour: two read-only calls and one pending approval.
pub fn tour_seed(s: &mut AppState) {
    propose(s, "inspect_tables".into(), json!({}), "What's in this session?".into());
    propose(s, "list_processing_tools".into(), json!({}), String::new());
    let spec = serde_json::to_value(s.flow.to_spec()).unwrap_or(json!({}));
    propose(s, "run_flow".into(), json!({"flow": spec}), String::new());
}

fn execute(s: &mut AppState, id: u64) {
    let Some(c) = s.copilot.calls.iter().find(|c| c.id == id).cloned() else { return };
    if s.submit(EngineJob::Mcp { call_id: id, name: c.tool.clone(), args: c.args.clone() }, JobPurpose::Copilot) {
        if let Some(cm) = s.copilot.calls.iter_mut().find(|x| x.id == id) {
            cm.status = Status::Running;
        }
    } else if let Some(cm) = s.copilot.calls.iter_mut().find(|x| x.id == id) {
        cm.status = Status::Queued;
    }
}

/// Plain name for an assistant tool.
fn friendly(tool: &str) -> String {
    match tool {
        "inspect_tables" => "Look at the session's tables".into(),
        "inspect_active_traces" => "Describe the current traces".into(),
        "list_processing_tools" => "List the processing steps".into(),
        "run_flow" => "Run a flow".into(),
        "load_opf_db" => "Load a project database".into(),
        "undo" => "Undo the last change".into(),
        "inspect_seismic_file" => "Look inside a seismic file".into(),
        "parse" => "Didn't understand that".into(),
        t => crate::panes::inspector::humanize(t),
    }
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let defs = tool_defs();
    Frame::new().inner_margin(Margin { left: 14, right: 14, top: 12, bottom: 6 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::SPARKLE).size(16.0).color(p.accent));
            ui.label(RichText::new("Assistant").font(crate::theme::font_display(17.0)).color(p.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !s.copilot.calls.is_empty() && widgets::cmd(ui, &p, icon::ERASER, "Clear", false).clicked() {
                    s.copilot.calls.clear();
                }
            });
        });
        ui.label(RichText::new("Works offline on this session. Anything that changes your data asks first.").size(11.5).color(p.text_muted));
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            let quick: [(&str, &str, &str); 5] = [
                (icon::TABLE, "What's in this session?", "inspect_tables"),
                (icon::WAVEFORM, "Describe the traces", "inspect_active_traces"),
                (icon::PUZZLE_PIECE, "List processing steps", "list_processing_tools"),
                (icon::FILE, "Look inside a file…", "inspect_seismic_file"),
                (icon::ARROW_COUNTER_CLOCKWISE, "Undo the last change", "undo"),
            ];
            for (ic, label, name) in quick {
                if pill(ui, &p, ic, label).clicked() {
                    if name == "inspect_seismic_file" {
                        if let Some(f) = rfd::FileDialog::new().add_filter("Seismic", &["sgy", "segy", "segd"]).pick_file() {
                            propose(s, name.into(), json!({"path": f.display().to_string()}), label.into());
                        }
                    } else {
                        propose(s, name.into(), json!({}), label.into());
                    }
                }
            }
            if pill(ui, &p, icon::PLAY, "Run the open flow").clicked() {
                let spec = serde_json::to_value(s.flow.to_spec()).unwrap_or(json!({}));
                propose(s, "run_flow".into(), json!({"flow": spec}), "Run the open flow".into());
            }
        });
    });
    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, p.border));

    let input_h = 64.0;
    let avail = ui.available_height() - input_h;
    let mut approve: Option<u64> = None;
    let mut decline: Option<u64> = None;
    let mut undo = false;
    let flow_name = s.flow.name.replace('_', " ");
    let flow_steps = s.flow.count();
    ScrollArea::vertical().max_height(avail.max(80.0)).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        Frame::new().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if s.copilot.calls.is_empty() {
                intro(ui, &p, &defs);
            }
            for c in &s.copilot.calls {
                call_card(ui, &p, c, &flow_name, flow_steps, &mut approve, &mut decline, &mut undo);
                ui.add_space(10.0);
            }
        });
    });
    if let Some(id) = approve {
        s.log(Level::Info, format!("Assistant: change #{id} approved"));
        if let Some(c) = s.copilot.calls.iter_mut().find(|c| c.id == id) {
            c.status = Status::Queued;
        }
    }
    pump(s);
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

fn pill(ui: &mut Ui, p: &Palette, ic: &str, label: &str) -> egui::Response {
    let text = format!("{ic}  {label}");
    let g = ui.painter().layout_no_wrap(text.clone(), egui::FontId::proportional(12.0), p.text);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(g.size().x + 20.0, 28.0), egui::Sense::click());
    let hov = resp.hovered();
    ui.painter().rect(rect, CornerRadius::same(14), if hov { p.accent_soft } else { p.raised }, Stroke::new(1.0, if hov { p.accent_line } else { p.border }), egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(12.0), if hov { p.accent_text } else { p.text });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn intro(ui: &mut Ui, p: &Palette, defs: &[Value]) {
    ui.label(RichText::new("Ask with a button above, or type a request below.").color(p.text));
    ui.label(RichText::new("Type plain requests like “show tables” or “bandpass 10 to 60 Hz”, or an exact MCP tool such as “render_gather_png output_path=qc/gather.png”. Press Tab to complete a tool name.").size(11.5).color(p.text_muted));
    ui.add_space(8.0);
    ui.label(RichText::new("WHAT IT CAN DO").font(crate::theme::font_semibold(10.5)).color(p.text_faint));
    for d in defs {
        let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let desc = crate::ui_copy::clean(d.get("description").and_then(|v| v.as_str()).unwrap_or(""));
        let mutating = policy_of(name) == CommandPolicy::Mutating;
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            widgets::dot(ui, if mutating { p.pick } else { p.accent }, 3.0);
            ui.label(RichText::new(friendly(name)).size(12.0).color(p.text));
            ui.label(RichText::new(name).font(font_mono(10.5)).color(p.text_faint));
        });
        ui.label(RichText::new(desc).size(11.0).color(p.text_muted));
    }
}

#[allow(clippy::too_many_arguments)]
fn call_card(ui: &mut Ui, p: &Palette, c: &Call, flow_name: &str, flow_steps: usize, approve: &mut Option<u64>, decline: &mut Option<u64>, undo: &mut bool) {
    let mutating = c.policy == CommandPolicy::Mutating;
    // The request, as a bubble on the right.
    let asked = if c.typed.is_empty() { friendly(&c.tool) } else { c.typed.clone() };
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        Frame::new().fill(p.accent_soft).corner_radius(CornerRadius { nw: 12, ne: 12, sw: 12, se: 4 }).inner_margin(Margin::symmetric(12, 7)).show(ui, |ui| {
            ui.set_max_width(260.0);
            ui.label(RichText::new(asked).size(12.0).color(p.accent_text));
        });
    });
    ui.add_space(6.0);
    // Tool chip
    ui.horizontal(|ui| {
        let col = if mutating { p.pick } else { p.accent };
        Frame::new().fill(with_alpha(col, 20)).stroke(Stroke::new(1.0, with_alpha(col, 90))).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
            ui.label(RichText::new(format!("{}  {}", icon::WRENCH, c.tool)).font(font_mono(10.5)).color(col));
        });
        match &c.status {
            Status::Running => {
                ui.add(egui::Spinner::new().size(12.0));
            }
            Status::Done(Ok(_)) => {
                ui.label(RichText::new(icon::CHECK).color(p.success));
            }
            _ => {}
        }
    });
    ui.add_space(4.0);
    match &c.status {
        Status::AwaitingApproval => {
            Frame::new().fill(with_alpha(p.pick, 16)).stroke(Stroke::new(1.0, with_alpha(p.pick, 140))).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SHIELD_CHECK).color(p.pick));
                    ui.label(RichText::new("This will change your data").font(crate::theme::font_semibold(12.5)).color(p.text));
                });
                let what = match c.tool.as_str() {
                    "run_flow" => format!("Run “{flow_name}” ({flow_steps} steps) on the loaded traces. You can undo it afterwards."),
                    "load_opf_db" => "Replace the session's tables with the ones in the chosen database.".to_string(),
                    "undo" => "Step back to the state before the last change.".to_string(),
                    _ => "Change the session.".to_string(),
                };
                ui.label(RichText::new(what).size(12.0).color(p.text_muted));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if widgets::cmd_primary(ui, p, icon::CHECK, "Run it").clicked() {
                        *approve = Some(c.id);
                    }
                    if widgets::cmd(ui, p, "", "Not now", false).clicked() {
                        *decline = Some(c.id);
                    }
                });
            });
        }
        Status::Declined => {
            ui.label(RichText::new("Skipped. Nothing was changed.").size(12.0).color(p.text_faint));
        }
        Status::Queued => {
            ui.label(RichText::new("Waiting for the current job to finish…").size(12.0).color(p.text_muted));
        }
        Status::Running => {
            ui.label(RichText::new("Working on it…").size(12.0).color(p.text_muted));
        }
        Status::Done(Ok(v)) => result_view(ui, p, c, v, undo),
        Status::Done(Err(e)) => {
            ui.label(RichText::new(e).size(12.0).color(p.danger));
        }
    }
}

fn result_view(ui: &mut Ui, p: &Palette, c: &Call, v: &Value, undo: &mut bool) {
    let summary: String = match c.tool.as_str() {
        "list_processing_tools" => format!("There are {} processing steps. Here is how they spread across categories:", v.get("count").and_then(|n| n.as_u64()).unwrap_or(0)),
        "inspect_active_traces" => format!(
            "{} traces of {} samples every {} µs. The strongest frequency is about {:.0} Hz, covering CDP {} to {}.",
            v.get("trace_count").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("n_samples").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("dt_us").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("dominant_freq_hz").and_then(|x| x.as_f64()).unwrap_or(0.0),
            v.get("first_cdp").and_then(|x| x.as_i64()).unwrap_or(0),
            v.get("last_cdp").and_then(|x| x.as_i64()).unwrap_or(0),
        ),
        "run_flow" => format!(
            "Done. The flow produced {} traces in {:.1} s.",
            v.get("final_trace_count").and_then(|x| x.as_u64()).unwrap_or(0),
            v.get("total_elapsed_ms").and_then(|x| x.as_f64()).unwrap_or(0.0) / 1000.0
        ),
        "inspect_tables" => {
            let parts: Vec<String> = v
                .as_object()
                .map(|o| o.iter().filter_map(|(k, x)| x.as_array().map(|a| (k, a.len()))).filter(|(_, n)| *n > 0).map(|(k, n)| {
                    let what = match k.as_str() {
                        "buffers" => "saved gather sets",
                        "velocities" => "velocity tables",
                        "mutes" => "mute tables",
                        "first_breaks" => "first-break tables",
                        "statics" => "statics solutions",
                        "sc_amps" => "amplitude-term tables",
                        "radon_panels" => "Radon panels",
                        "vsp_profiles" => "borehole profiles",
                        other => other,
                    };
                    format!("{n} {what}")
                }).collect())
                .unwrap_or_default();
            if parts.is_empty() { "The session has no tables yet.".to_string() } else { format!("The session holds {}.", parts.join(", ")) }
        }
        _ => compact_json(v, 200),
    };
    ui.label(RichText::new(summary).size(12.5).color(p.text));
    // Inline chart: steps per category.
    if c.tool == "list_processing_tools" {
        if let Some(tools) = v.get("tools").and_then(|t| t.as_array()) {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for t in tools {
                let cat = t.get("category").and_then(|c| c.as_str()).unwrap_or("Other").to_string();
                match counts.iter_mut().find(|(c, _)| *c == cat) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((cat, 1)),
                }
            }
            counts.sort_by(|a, b| b.1.cmp(&a.1));
            let max = counts.first().map(|c| c.1).unwrap_or(1).max(1) as f32;
            Frame::new().fill(p.sunken).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                for (cat, n) in counts.iter().take(10) {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(110.0, 14.0), egui::Sense::hover());
                        ui.painter().with_clip_rect(r).text(r.left_center(), egui::Align2::LEFT_CENTER, cat, egui::FontId::proportional(11.0), p.text_muted);
                        let w = ui.available_width() - 30.0;
                        let (b, _) = ui.allocate_exact_size(egui::vec2(w, 8.0), egui::Sense::hover());
                        let mut f = b;
                        f.set_width(w * *n as f32 / max);
                        ui.painter().rect_filled(f, CornerRadius::same(4), crate::panes::catalog::category_color(p, cat));
                        ui.label(RichText::new(n.to_string()).font(font_mono(10.5)).color(p.text_faint));
                    });
                }
            });
        }
    }
    ui.horizontal(|ui| {
        egui::CollapsingHeader::new(RichText::new("Details").size(11.0).color(p.text_faint)).id_salt(("res", c.id)).show(ui, |ui| {
            let pretty = serde_json::to_string_pretty(v).unwrap_or_default();
            let shown: String = pretty.chars().take(20_000).collect();
            ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                ui.label(RichText::new(shown).font(font_mono(10.5)).color(p.text_muted));
            });
            if ui.small_button(format!("{} copy", icon::COPY)).clicked() {
                ui.ctx().copy_text(pretty);
            }
        });
        if c.policy == CommandPolicy::Mutating && c.tool != "undo" && ui.small_button(format!("{} Undo", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
            *undo = true;
        }
    });
}

fn composer(s: &mut AppState, ui: &mut Ui, p: &Palette, defs: &[Value]) {
    Frame::new().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        Frame::new().fill(p.sunken).stroke(Stroke::new(1.0, p.border_strong)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut s.copilot.input)
                        .font(egui::FontId::proportional(13.0))
                        .frame(egui::Frame::NONE)
                        .hint_text("Try “show tables”, “bandpass 10 60”, or a tool name…")
                        .desired_width(ui.available_width() - 34.0),
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
                let send = widgets::cmd(ui, p, icon::PAPER_PLANE_RIGHT, "", false).on_hover_text("Send (Enter)").clicked() || enter;
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
        egui::CollapsingHeader::new(RichText::new("Build a request with a form").size(11.0).color(p.text_faint)).id_salt("cp_form").show(ui, |ui| {
            form_builder(s, ui, p, defs);
        });
    });
}

fn form_builder(s: &mut AppState, ui: &mut Ui, p: &Palette, defs: &[Value]) {
    let current = s.copilot.form_tool.clone().map(|t| friendly(&t)).unwrap_or_else(|| "Choose…".into());
    egui::ComboBox::from_id_salt("cp_form_tool").selected_text(current).width(220.0).show_ui(ui, |ui| {
        for d in defs {
            let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if ui.selectable_label(s.copilot.form_tool.as_deref() == Some(&name), friendly(&name)).clicked() {
                s.copilot.form = default_form(d);
                s.copilot.form_tool = Some(name);
            }
        }
    });
    let Some(tool) = s.copilot.form_tool.clone() else { return };
    let schema = defs.iter().find(|d| d.get("name").and_then(|n| n.as_str()) == Some(tool.as_str())).cloned().unwrap_or(json!({}));
    let props = schema.pointer("/inputSchema/properties").and_then(|x| x.as_object()).cloned().unwrap_or_default();
    egui::Grid::new("cp_form_grid").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
        for (k, spec) in &props {
            let ty = spec.get("type").and_then(|t| t.as_str()).unwrap_or("string");
            let entry = s.copilot.form.entry(k.clone()).or_insert(Value::Null);
            ui.label(RichText::new(crate::panes::inspector::humanize(k)).size(11.5).color(p.text_muted));
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
                    ui.label(RichText::new("type it in the box above").size(10.5).color(p.text_faint));
                }
                _ => {
                    ui.horizontal(|ui| {
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
                    });
                }
            }
            ui.end_row();
        }
    });
    if widgets::cmd_primary(ui, p, icon::PAPER_PLANE_RIGHT, "Send").clicked() {
        let args: Map<String, Value> = s.copilot.form.iter().filter(|(_, v)| !v.is_null() && v.as_str() != Some("")).map(|(k, v)| (k.clone(), v.clone())).collect();
        propose(s, tool.clone(), Value::Object(args), friendly(&tool));
    }
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
        assert!(parse_command("inspect").unwrap_err().contains("Did you mean"));
    }

    #[test]
    fn mutating_policy_matches_engine() {
        assert_eq!(policy_of("run_flow"), CommandPolicy::Mutating);
        assert_eq!(policy_of("load_opf_db"), CommandPolicy::Mutating);
        assert_eq!(policy_of("inspect_tables"), CommandPolicy::ReadOnly);
    }

    #[test]
    fn parses_plain_english_requests() {
        let (t, _) = parse_command("show tables").unwrap();
        assert_eq!(t, "inspect_tables");
        let (t, a) = parse_command("apply bandpass 10 to 60 Hz").unwrap();
        assert_eq!(t, "run_flow");
        assert_eq!(a["flow"]["steps"][0]["tool"], "filter");
        assert_eq!(policy_of(&t), CommandPolicy::Mutating);
        let (t, a) = parse_command("apply agc 400").unwrap();
        assert_eq!(t, "run_flow");
        assert_eq!(a["flow"]["steps"][0]["tool"], "agc");
    }
}
