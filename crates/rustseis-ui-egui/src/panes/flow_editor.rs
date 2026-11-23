//! Workspace 1 · centre: pipeline & branching flow editor.

use crate::data::{fmt_count, fmt_ms};
use crate::flowdoc::{FlowNode, StepPath, branch_names, is_container};
use crate::panes::catalog::{ToolDrag, archetype_icon, category_color};
use crate::state::{AppState, Level};
use crate::theme::{Palette, RADIUS_MD, font_mono, mix, palette, with_alpha};
use crate::widgets::{self, chip, icon_button, primary_button, vsep};
use egui::{
    Align2, CornerRadius, Frame, Margin, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use egui_phosphor::regular as icon;
use rustseis_flow::StepReport;

#[derive(Clone, Debug)]
struct StepDrag(StepPath);

enum Action {
    Select(StepPath),
    Toggle(StepPath),
    Remove(StepPath),
    Duplicate(StepPath),
    Move { from: StepPath, parent: Vec<usize>, index: usize },
    Insert { parent: Vec<usize>, index: usize, tool: String },
    AddBranch(StepPath),
    RemoveBranch(StepPath, usize),
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let mut run = false;
    widgets::toolbar(ui, &p, |ui| {
        ui.add_enabled_ui(!s.is_busy(), |ui| {
            if primary_button(ui, &p, icon::PLAY, "Run flow").on_hover_text("Execute on the engine worker (Ctrl+Enter)").clicked() {
                run = true;
            }
        });
        vsep(ui, &p);
        if icon_button(ui, &p, icon::FILE, "New empty flow", false).clicked() {
            s.flow = Default::default();
            s.flow_sel = None;
            s.report_uids.clear();
        }
        if icon_button(ui, &p, icon::FOLDER_OPEN, "Open flow…", false).clicked() {
            s.open_flow_dialog();
        }
        if icon_button(ui, &p, icon::FLOPPY_DISK, "Save (Ctrl+S)", false).clicked() {
            s.save_flow(false);
        }
        if icon_button(ui, &p, icon::EXPORT, "Save as…", false).clicked() {
            s.save_flow(true);
        }
        vsep(ui, &p);
        let json_id = egui::Id::unique("flow_json_view");
        let mut json_view: bool = ui.data(|d| d.get_temp(json_id)).unwrap_or(false);
        if icon_button(ui, &p, icon::CODE, "Raw JSON view", json_view).clicked() {
            json_view = !json_view;
            ui.data_mut(|d| d.insert_temp(json_id, json_view));
            ui.data_mut(|d| d.remove::<String>(egui::Id::unique("flow_json_text")));
        }
        vsep(ui, &p);
        ui.add(egui::TextEdit::singleline(&mut s.flow.name).font(font_mono(12.0)).desired_width(220.0))
            .on_hover_text("Flow name");
        if s.flow.dirty {
            ui.label(RichText::new("●").color(p.pick)).on_hover_text("Unsaved changes");
        }
        ui.label(RichText::new(format!("{} steps", s.flow.count())).size(11.0).color(p.text_faint));
    });
    if run {
        s.run_current_flow();
    }

    running_banner(s, ui, &p);

    let json_view: bool = ui.data(|d| d.get_temp(egui::Id::unique("flow_json_view"))).unwrap_or(false);
    if json_view {
        json_editor(s, ui, &p);
        return;
    }

    let mut actions: Vec<Action> = Vec::new();
    let report_steps: Vec<Option<StepReport>> = s
        .flow
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let rep = s.report.as_ref()?;
            if s.report_uids.get(i) == Some(&n.uid) { rep.steps.get(i).cloned() } else { None }
        })
        .collect();

    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 40 }).show(ui, |ui| {
            if let Some(desc) = (!s.flow.description.is_empty()).then(|| s.flow.description.clone()) {
                ui.label(RichText::new(desc).size(11.5).color(p.text_muted));
                ui.add_space(6.0);
            }
            if s.flow.nodes.is_empty() {
                empty_flow(ui, &p, &mut actions);
            } else {
                let nodes = s.flow.nodes.clone();
                list(s, ui, &p, &nodes, &[], &report_steps, &mut actions, 0);
            }
        });
    });

    apply(s, actions);
}

fn running_banner(s: &AppState, ui: &mut Ui, p: &Palette) {
    if let Some((label, elapsed, eta)) = s.busy_info() {
        Frame::new().fill(with_alpha(p.accent, 18)).inner_margin(Margin::symmetric(12, 6)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(p.accent));
                ui.label(RichText::new(label).strong());
                let el = elapsed.as_secs_f64() * 1000.0;
                let txt = match eta {
                    Some(e) => format!("{} / ~{}", fmt_ms(el), fmt_ms(e as f64)),
                    None => fmt_ms(el),
                };
                ui.label(RichText::new(txt).font(font_mono(11.5)).color(p.text_muted));
            });
            let frac = eta.map(|e| (elapsed.as_secs_f64() * 1000.0 / e.max(1) as f64).min(0.97) as f32);
            widgets::progress_line(ui, p, ui.available_width(), frac);
        });
    }
}

fn empty_flow(ui: &mut Ui, p: &Palette, actions: &mut Vec<Action>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 160.0), Sense::hover());
    let hovering = resp.dnd_hover_payload::<ToolDrag>().is_some();
    ui.painter().rect(
        rect,
        CornerRadius::same(RADIUS_MD),
        if hovering { p.accent_soft } else { p.sunken },
        Stroke::new(1.0, if hovering { p.accent } else { p.border_strong }),
        StrokeKind::Inside,
    );
    ui.painter().text(rect.center() - vec2(0.0, 14.0), Align2::CENTER_CENTER, icon::FLOW_ARROW, egui::FontId::proportional(26.0), p.text_faint);
    ui.painter().text(rect.center() + vec2(0.0, 14.0), Align2::CENTER_CENTER, "Drop tools here, double-click them in the library, or open a flow", egui::FontId::proportional(12.5), p.text_muted);
    if let Some(t) = resp.dnd_release_payload::<ToolDrag>() {
        actions.push(Action::Insert { parent: vec![], index: 0, tool: t.0.clone() });
    }
}

#[allow(clippy::too_many_arguments)]
fn list(
    s: &AppState,
    ui: &mut Ui,
    p: &Palette,
    nodes: &[FlowNode],
    parent: &[usize],
    reports: &[Option<StepReport>],
    actions: &mut Vec<Action>,
    depth: usize,
) {
    for (i, node) in nodes.iter().enumerate() {
        let mut path = parent.to_vec();
        path.push(i);
        let rep = if depth == 0 { reports.get(i).cloned().flatten() } else { None };
        card(s, ui, p, node, &path, parent, i, rep.as_ref(), actions, depth);
        if i + 1 < nodes.len() {
            connector(ui, p);
        }
    }
}

fn connector(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    let x = rect.left() + 22.0;
    ui.painter().vline(x, rect.y_range(), Stroke::new(1.5, p.border_strong));
}

#[allow(clippy::too_many_arguments)]
fn card(
    s: &AppState,
    ui: &mut Ui,
    p: &Palette,
    node: &FlowNode,
    path: &StepPath,
    parent: &[usize],
    index: usize,
    rep: Option<&StepReport>,
    actions: &mut Vec<Action>,
    depth: usize,
) {
    let desc = s.tool(&node.tool);
    let known = desc.is_some();
    let cat_color = desc.map(|d| category_color(p, &d.category)).unwrap_or(p.danger);
    let selected = s.flow_sel.as_ref() == Some(path);
    let enabled = node.enabled;
    let fill = if selected { mix(p.raised, p.accent, 0.06) } else { p.raised };
    let border = if selected { with_alpha(p.accent, 200) } else if !known { p.danger } else { p.border };

    let frame = Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(Margin { left: 10, right: 8, top: 6, bottom: 6 });
    let inner = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        if !enabled {
            ui.multiply_opacity(0.55);
        }
        ui.horizontal(|ui| {
            let handle = ui.dnd_drag_source(egui::Id::unique(("step_drag", node.uid)), StepDrag(path.clone()), |ui| {
                ui.label(RichText::new(icon::DOTS_SIX_VERTICAL).color(p.text_faint));
            });
            handle.response.on_hover_cursor(egui::CursorIcon::Grab);
            ui.label(RichText::new(format!("{:02}", index + 1)).font(font_mono(11.0)).color(p.text_faint));
            let ic = desc.map(|d| archetype_icon(d.archetype)).unwrap_or(icon::WARNING);
            ui.label(RichText::new(ic).color(cat_color));
            let title = node.label.clone().unwrap_or_else(|| desc.map(|d| d.label.clone()).unwrap_or_else(|| node.tool.clone()));
            ui.label(RichText::new(title).strong().color(if enabled { p.text } else { p.text_muted }));
            ui.label(RichText::new(&node.tool).font(font_mono(10.5)).color(p.text_faint));
            if !known {
                chip(ui, "unknown tool", p.danger, with_alpha(p.danger, 24));
            }
            if s.tier2.contains(&node.tool) {
                chip(ui, "T2", p.accent, with_alpha(p.accent, 22));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, p, icon::TRASH, "Remove step", false).clicked() {
                    actions.push(Action::Remove(path.clone()));
                }
                if icon_button(ui, p, icon::COPY, "Duplicate", false).clicked() {
                    actions.push(Action::Duplicate(path.clone()));
                }
                let (tip, ic) = if enabled { ("Bypass step (ProMAX toggle)", icon::TOGGLE_RIGHT) } else { ("Enable step", icon::TOGGLE_LEFT) };
                if icon_button(ui, p, ic, tip, enabled).clicked() {
                    actions.push(Action::Toggle(path.clone()));
                }
                if !enabled {
                    chip(ui, "BYPASS", p.text_muted, p.sunken);
                }
            });
        });
        summary_line(ui, p, node);
        if let Some(r) = rep {
            telemetry(ui, p, r);
        }
        if is_container(&node.tool) {
            branches(s, ui, p, node, path, actions, depth);
        }
    });
    let rect = inner.response.rect;
    // Category rail
    let rail = Rect::from_min_max(pos2(rect.left() + 1.0, rect.top() + 6.0), pos2(rect.left() + 3.5, rect.bottom() - 6.0));
    ui.painter().rect_filled(rail, CornerRadius::same(2), if enabled { cat_color } else { p.text_faint });

    let resp = inner.response.interact(Sense::click());
    if resp.clicked() {
        actions.push(Action::Select(path.clone()));
    }
    // Drop handling: upper half inserts before, lower half after.
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let over = pointer.is_some_and(|pt| rect.contains(pt));
    let dragging_step = egui::DragAndDrop::has_payload_of_type::<StepDrag>(ui.ctx());
    let dragging_tool = egui::DragAndDrop::has_payload_of_type::<ToolDrag>(ui.ctx());
    if over && (dragging_step || dragging_tool) {
        let before = pointer.is_some_and(|pt| pt.y < rect.center().y);
        let y = if before { rect.top() - 5.0 } else { rect.bottom() + 5.0 };
        ui.painter().hline(rect.x_range(), y, Stroke::new(2.0, p.accent));
        ui.painter().circle_filled(pos2(rect.left(), y), 3.5, p.accent);
        let released = ui.input(|i| i.pointer.any_released());
        if released {
            let target = if before { index } else { index + 1 };
            if let Some(StepDrag(from)) = egui::DragAndDrop::take_payload::<StepDrag>(ui.ctx()).map(|a| (*a).clone()) {
                if !path.starts_with(&from) || from == *path {
                    actions.push(Action::Move { from, parent: parent.to_vec(), index: target });
                }
            } else if let Some(t) = egui::DragAndDrop::take_payload::<ToolDrag>(ui.ctx()) {
                actions.push(Action::Insert { parent: parent.to_vec(), index: target, tool: t.0.clone() });
            }
        }
    }
}

fn summary_line(ui: &mut Ui, p: &Palette, node: &FlowNode) {
    let mut parts: Vec<String> = Vec::new();
    if let Some(obj) = node.params.as_object() {
        let flat = obj.get("params").and_then(|v| v.as_object()).cloned();
        let mut push = |k: &str, v: &serde_json::Value| {
            if parts.len() >= 5 {
                return;
            }
            let txt = match v {
                serde_json::Value::String(s) if !s.is_empty() => {
                    let short = if s.len() > 38 { format!("…{}", &s[s.len().saturating_sub(36)..]) } else { s.clone() };
                    format!("{k}={short}")
                }
                serde_json::Value::Number(n) => format!("{k}={n}"),
                serde_json::Value::Bool(b) => format!("{k}={}", if *b { "on" } else { "off" }),
                serde_json::Value::Array(a) if !a.is_empty() && a.len() <= 4 && a.iter().all(|x| x.is_number()) => {
                    format!("{k}=[{}]", a.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))
                }
                _ => return,
            };
            parts.push(txt);
        };
        for (k, v) in obj {
            if k != "params" {
                push(k, v);
            }
        }
        if let Some(f) = flat {
            for (k, v) in &f {
                push(k, v);
            }
        }
    }
    if node.tool == "python_node" {
        if let Some(code) = node.params.get("inline_code").and_then(|v| v.as_str()) {
            let first = code.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            parts = vec![format!("py: {first}")];
        }
    }
    if !parts.is_empty() {
        ui.add_space(1.0);
        ui.label(RichText::new(parts.join("  ·  ")).font(font_mono(10.5)).color(p.text_muted));
    }
}

fn telemetry(ui: &mut Ui, p: &Palette, r: &StepReport) {
    ui.horizontal(|ui| {
        if !r.enabled {
            chip(ui, "bypassed", p.text_muted, p.sunken);
            return;
        }
        let traces = if r.traces_in == r.traces_out {
            format!("{} traces", fmt_count(r.traces_out))
        } else {
            format!("{} → {} traces", fmt_count(r.traces_in), fmt_count(r.traces_out))
        };
        chip(ui, &traces, p.success, with_alpha(p.success, 20));
        chip(ui, &fmt_ms(r.elapsed_ms as f64), p.info, with_alpha(p.info, 20));
        if let Some(st) = &r.sample_stats {
            chip(ui, &format!("rms={:.4}", st.rms), p.text_muted, p.sunken);
        }
    });
}

fn branches(s: &AppState, ui: &mut Ui, p: &Palette, node: &FlowNode, path: &StepPath, actions: &mut Vec<Action>, depth: usize) {
    let names = branch_names(&node.tool, node.branches.len().max(if node.tool == "flow_if" { 2 } else { 0 }));
    let n = if node.tool == "flow_if" { 2 } else { node.branches.len() };
    let out_branch = node.params.get("output_branch").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    ui.add_space(4.0);
    for b in 0..n {
        let mut bpath = path.clone();
        bpath.push(b);
        let empty: Vec<FlowNode> = Vec::new();
        let steps = node.branches.get(b).unwrap_or(&empty);
        let color = if node.tool == "flow_if" { if b == 0 { p.success } else { p.pick } } else { p.series_b };
        Frame::new()
            .fill(mix(p.raised, p.sunken, 0.55))
            .stroke(Stroke::new(1.0, with_alpha(color, 70)))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin { left: 8, right: 6, top: 4, bottom: 6 })
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::GIT_BRANCH).color(color));
                    ui.label(RichText::new(names.get(b).cloned().unwrap_or_default()).font(font_mono(11.0)).color(color));
                    if node.tool == "flow_split" && b == out_branch {
                        chip(ui, "output", p.accent, with_alpha(p.accent, 22));
                    }
                    ui.label(RichText::new(format!("{} steps", steps.len())).size(10.5).color(p.text_faint));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if node.tool == "flow_split" && icon_button(ui, p, icon::MINUS, "Remove branch", false).clicked() {
                            actions.push(Action::RemoveBranch(path.clone(), b));
                        }
                        ui.menu_button(RichText::new(icon::PLUS).size(13.0), |ui| {
                            add_menu(s, ui, p, bpath.clone(), steps.len(), actions);
                        })
                        .response
                        .on_hover_text("Add step to this branch");
                    });
                });
                if steps.is_empty() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                    let hover = resp.dnd_hover_payload::<ToolDrag>().is_some() || resp.dnd_hover_payload::<StepDrag>().is_some();
                    ui.painter().rect(rect, CornerRadius::same(3), if hover { p.accent_soft } else { p.sunken }, Stroke::new(1.0, p.border), StrokeKind::Inside);
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "empty — drop a tool here", egui::FontId::proportional(11.0), p.text_faint);
                    if let Some(t) = resp.dnd_release_payload::<ToolDrag>() {
                        actions.push(Action::Insert { parent: bpath.clone(), index: 0, tool: t.0.clone() });
                    } else if let Some(sd) = resp.dnd_release_payload::<StepDrag>() {
                        if !bpath.starts_with(&sd.0) {
                            actions.push(Action::Move { from: sd.0.clone(), parent: bpath.clone(), index: 0 });
                        }
                    }
                } else {
                    list(s, ui, p, steps, &bpath, &[], actions, depth + 1);
                }
            });
        ui.add_space(3.0);
    }
    if node.tool == "flow_split" && icon_button(ui, p, icon::GIT_FORK, "Add parallel branch", false).clicked() {
        actions.push(Action::AddBranch(path.clone()));
    }
}

fn add_menu(s: &AppState, ui: &mut Ui, p: &Palette, parent: Vec<usize>, index: usize, actions: &mut Vec<Action>) {
    let id = egui::Id::unique(("branch_add_q", &parent));
    let mut q: String = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    ui.add(egui::TextEdit::singleline(&mut q).hint_text("filter tools…").desired_width(220.0)).request_focus();
    ui.data_mut(|d| d.insert_temp(id, q.clone()));
    let ql = q.to_ascii_lowercase();
    ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
        for t in s.catalog.iter().filter(|t| ql.is_empty() || t.label.to_ascii_lowercase().contains(&ql) || t.menu_id.contains(&ql)) {
            let r = ui.button(RichText::new(format!("{}  {}", archetype_icon(t.archetype), t.label)).size(12.0));
            if r.clicked() {
                actions.push(Action::Insert { parent: parent.clone(), index, tool: t.menu_id.clone() });
                ui.close();
            }
        }
    });
    let _ = p;
}

fn json_editor(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let id = egui::Id::unique("flow_json_text");
    let mut text: String = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| serde_json::to_string_pretty(&s.flow.to_spec()).unwrap_or_default());
    let mut apply = false;
    widgets::body(ui, |ui| {
        ui.horizontal(|ui| {
            if ui.button(format!("{}  Apply JSON", icon::CHECK)).clicked() {
                apply = true;
            }
            if ui.button(format!("{}  Revert", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
                text = serde_json::to_string_pretty(&s.flow.to_spec()).unwrap_or_default();
            }
            let valid = serde_json::from_str::<rustseis_flow::FlowSpec>(&text);
            match valid {
                Ok(_) => {
                    ui.label(RichText::new(format!("{} valid FlowSpec", icon::CHECK)).color(p.success).size(11.5));
                }
                Err(e) => {
                    ui.label(RichText::new(format!("{} {e}", icon::WARNING)).color(p.danger).size(11.5));
                }
            }
        });
        ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(font_mono(12.0))
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(30),
            );
        });
    });
    if apply {
        match serde_json::from_str::<rustseis_flow::FlowSpec>(&text) {
            Ok(spec) => {
                let path = s.flow.path.clone();
                s.flow = crate::flowdoc::FlowDoc::from_spec(&spec, path);
                s.flow.dirty = true;
                s.flow_sel = None;
                s.toast(Level::Ok, "Flow updated from JSON");
                ui.data_mut(|d| d.remove::<String>(id));
                return;
            }
            Err(e) => s.toast(Level::Error, format!("Invalid flow JSON: {e}")),
        }
    }
    ui.data_mut(|d| d.insert_temp(id, text));
}

fn apply(s: &mut AppState, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::Select(p) => s.flow_sel = Some(p),
            Action::Toggle(p) => {
                if let Some(n) = s.flow.get_mut(&p) {
                    n.enabled = !n.enabled;
                    s.flow.dirty = true;
                }
            }
            Action::Remove(p) => {
                s.flow.remove(&p);
                if s.flow_sel.as_ref().is_some_and(|sel| sel.starts_with(&p)) {
                    s.flow_sel = None;
                }
            }
            Action::Duplicate(p) => {
                if let Some(np) = s.flow.duplicate(&p) {
                    s.flow_sel = Some(np);
                }
            }
            Action::Move { from, parent, index } => {
                let (from_parent, from_idx) = from.split_at(from.len() - 1);
                if from_parent == parent.as_slice() {
                    let to = if index > from_idx[0] { index - 1 } else { index };
                    if let Some(np) = s.flow.move_within(&from, to) {
                        s.flow_sel = Some(np);
                    }
                } else if let Some(node) = s.flow.remove(&from) {
                    // Removing may shift the destination path if it shares a prefix list.
                    let mut parent = parent.clone();
                    let depth = from_parent.len();
                    if parent.len() > depth && parent[..depth] == *from_parent && parent[depth] > from_idx[0] {
                        parent[depth] -= 1;
                    }
                    if let Some(list) = s.flow.container_mut(&parent) {
                        let idx = index.min(list.len());
                        list.insert(idx, node);
                        let mut np = parent.clone();
                        np.push(idx);
                        s.flow_sel = Some(np);
                        s.flow.dirty = true;
                    }
                }
                s.report_uids.clear();
            }
            Action::Insert { parent, index, tool } => {
                let Some(desc) = s.tool(&tool).cloned() else { continue };
                let node = s.flow.new_node(&desc.menu_id, desc.default_params.clone());
                if let Some(list) = s.flow.container_mut(&parent) {
                    let idx = index.min(list.len());
                    list.insert(idx, node);
                    let mut np = parent.clone();
                    np.push(idx);
                    s.flow_sel = Some(np);
                    s.flow.dirty = true;
                }
            }
            Action::AddBranch(p) => {
                if let Some(n) = s.flow.get_mut(&p) {
                    n.branches.push(Vec::new());
                    s.flow.dirty = true;
                }
            }
            Action::RemoveBranch(p, b) => {
                if let Some(n) = s.flow.get_mut(&p) {
                    if b < n.branches.len() {
                        n.branches.remove(b);
                        s.flow.dirty = true;
                    }
                }
            }
        }
    }
}
