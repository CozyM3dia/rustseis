//! Flow document: a pipeline rail with step cards, run timings, data-shape markers and branches.

use crate::data::{fmt_count, fmt_ms};
use crate::flowdoc::{FlowNode, StepPath, branch_names, is_container};
use crate::panes::catalog::{ToolDrag, archetype_icon, category_color};
use crate::state::{AppState, Level};
use crate::theme::{Palette, font_mono, mix, palette, with_alpha};
use crate::widgets::{self, chip, icon_button};
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
    running_banner(s, ui, &p);
    if s.inspector.flow_json {
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
    let max_ms = report_steps.iter().flatten().map(|r| r.elapsed_ms).max().unwrap_or(1).max(1);

    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 18, right: 18, top: 14, bottom: 40 }).show(ui, |ui| {
            header(s, ui, &p, &report_steps);
            ui.add_space(10.0);
            if s.flow.nodes.is_empty() {
                empty_flow(ui, &p, &mut actions);
            } else {
                let nodes = s.flow.nodes.clone();
                list(s, ui, &p, &nodes, &[], &report_steps, max_ms, &mut actions, 0);
            }
        });
    });

    apply(s, actions);
}

/// Flow name, description, last-run summary and the "where the time goes" bar.
fn header(s: &mut AppState, ui: &mut Ui, p: &Palette, reps: &[Option<StepReport>]) {
    ui.horizontal(|ui| {
        let w = ui.available_width().min(520.0);
        let mut name = s.flow.name.replace('_', " ");
        let r = ui.add(egui::TextEdit::singleline(&mut name).font(crate::theme::font_display(20.0)).frame(egui::Frame::NONE).desired_width(w));
        if r.changed() {
            s.flow.name = name.replace(' ', "_");
            s.flow.dirty = true;
        }
        if s.flow.dirty {
            ui.label(RichText::new("● unsaved").size(11.0).color(p.pick));
        }
    });
    if !s.flow.description.is_empty() {
        ui.label(RichText::new(&s.flow.description).size(12.0).color(p.text_muted));
    }
    let done: Vec<&StepReport> = reps.iter().flatten().filter(|r| r.enabled).collect();
    ui.add_space(4.0);
    if done.is_empty() {
        ui.label(RichText::new(format!("{} steps · not run yet · press Run flow or Ctrl+Enter", s.flow.count())).size(11.5).color(p.text_faint));
        return;
    }
    let total: u128 = done.iter().map(|r| r.elapsed_ms).sum::<u128>().max(1);
    let last = done.last().map(|r| r.traces_out).unwrap_or(0);
    ui.label(RichText::new(format!("{} steps · last run {} · {} traces out", s.flow.count(), fmt_ms(total as f64), fmt_count(last))).size(11.5).color(p.text_muted));
    // Where the time goes
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(6), p.sunken);
    let mut x = rect.left();
    let mut ranked: Vec<(usize, u128)> = Vec::new();
    for (i, r) in reps.iter().enumerate() {
        let Some(r) = r else { continue };
        if !r.enabled || r.elapsed_ms == 0 {
            continue;
        }
        let w = rect.width() * r.elapsed_ms as f32 / total as f32;
        let col = s.tool(&r.tool).map(|d| category_color(p, &d.category)).unwrap_or(p.text_muted);
        let seg = Rect::from_min_max(pos2(x, rect.top()), pos2(x + w, rect.bottom()));
        ui.painter().rect_filled(seg.shrink2(vec2(0.5, 0.0)), CornerRadius::same(2), col);
        let label = s.tool(&r.tool).map(|d| d.label.clone()).unwrap_or_default();
        ui.interact(seg, egui::Id::unique(("time_seg", i)), Sense::hover()).on_hover_text(format!("{label} · {}", fmt_ms(r.elapsed_ms as f64)));
        x += w;
        ranked.push((i, r.elapsed_ms));
    }
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Most time:").size(11.0).color(p.text_faint));
        for (i, ms) in ranked.iter().take(3) {
            let r = reps[*i].as_ref().expect("ranked from reports");
            let d = s.tool(&r.tool);
            let col = d.map(|d| category_color(p, &d.category)).unwrap_or(p.text_muted);
            widgets::dot(ui, col, 3.5);
            ui.label(RichText::new(format!("{} {}%", d.map(|d| d.label.clone()).unwrap_or_default(), (*ms as f32 / total as f32 * 100.0).round())).size(11.0).color(p.text_muted));
        }
    });
}

fn running_banner(s: &AppState, ui: &mut Ui, p: &Palette) {
    if let Some((label, elapsed, eta)) = s.busy_info() {
        let (prog, stage_msg) = s.job_control.snapshot();
        Frame::new().fill(with_alpha(p.accent, 18)).inner_margin(Margin::symmetric(12, 6)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0).color(p.accent));
                ui.label(RichText::new(label).strong());
                if !stage_msg.is_empty() {
                    ui.label(RichText::new(format!("· {stage_msg}")).size(11.5).color(p.text));
                }
                let el = elapsed.as_secs_f64() * 1000.0;
                let txt = if prog > 0.01 {
                    format!("{} · {:.0}%", fmt_ms(el), prog * 100.0)
                } else {
                    match eta {
                        Some(e) => format!("{} of about {}", fmt_ms(el), fmt_ms(e as f64)),
                        None => fmt_ms(el),
                    }
                };
                ui.label(RichText::new(txt).font(font_mono(11.5)).color(p.text_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::cmd(ui, p, icon::X, "Cancel", false).clicked() {
                        s.job_control.cancel();
                    }
                });
            });
            let frac = if prog > 0.01 {
                Some(prog.clamp(0.0, 1.0))
            } else {
                eta.map(|e| (elapsed.as_secs_f64() * 1000.0 / e.max(1) as f64).min(0.97) as f32)
            };
            widgets::progress_line(ui, p, ui.available_width(), frac);
        });
    }
}

fn empty_flow(ui: &mut Ui, p: &Palette, actions: &mut Vec<Action>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 180.0), Sense::hover());
    let hovering = resp.dnd_hover_payload::<ToolDrag>().is_some();
    ui.painter().rect(
        rect,
        CornerRadius::same(12),
        if hovering { p.accent_soft } else { p.sunken },
        Stroke::new(1.0, if hovering { p.accent } else { p.border_strong }),
        StrokeKind::Inside,
    );
    ui.painter().text(rect.center() - vec2(0.0, 18.0), Align2::CENTER_CENTER, icon::FLOW_ARROW, egui::FontId::proportional(28.0), p.text_faint);
    ui.painter().text(rect.center() + vec2(0.0, 16.0), Align2::CENTER_CENTER, "Start with a step that reads data", crate::theme::font_semibold(13.0), p.text);
    ui.painter().text(rect.center() + vec2(0.0, 36.0), Align2::CENTER_CENTER, "Drag one in from the Library, or use the Add menus above", egui::FontId::proportional(12.0), p.text_muted);
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
    max_ms: u128,
    actions: &mut Vec<Action>,
    depth: usize,
) {
    for (i, node) in nodes.iter().enumerate() {
        let mut path = parent.to_vec();
        path.push(i);
        let rep = if depth == 0 { reports.get(i).cloned().flatten() } else { None };
        card(s, ui, p, node, &path, parent, i, rep.as_ref(), max_ms, actions, depth);
        if i + 1 < nodes.len() {
            connector(ui, p, node, rep.as_ref());
        }
    }
}

/// Rail segment between two steps, with a marker when the data changes shape.
fn connector(ui: &mut Ui, p: &Palette, node: &FlowNode, rep: Option<&StepReport>) {
    let marker = match rep {
        Some(r) if r.enabled && r.traces_out < r.traces_in => Some((icon::ARROWS_IN, format!("traces combine · {} → {}", fmt_count(r.traces_in), fmt_count(r.traces_out)), p.series_c)),
        Some(r) if r.enabled && r.traces_out > r.traces_in && r.traces_in > 0 => Some((icon::ARROWS_OUT, format!("more traces · {} → {}", fmt_count(r.traces_in), fmt_count(r.traces_out)), p.info)),
        _ if node.enabled && node.tool.contains("sort") => Some((icon::ARROWS_DOWN_UP, "order changes".to_string(), p.pick)),
        _ => None,
    };
    let h = if marker.is_some() { 26.0 } else { 12.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let x = rect.left() + RAIL_X;
    ui.painter().vline(x, rect.y_range(), Stroke::new(2.0, p.border_strong));
    if let Some((ic, text, col)) = marker {
        let c = pos2(x, rect.center().y);
        ui.painter().circle_filled(c, 8.0, p.panel);
        ui.painter().circle_stroke(c, 8.0, Stroke::new(1.0, col));
        ui.painter().text(c, Align2::CENTER_CENTER, ic, egui::FontId::proportional(10.0), col);
        ui.painter().text(pos2(x + 16.0, c.y), Align2::LEFT_CENTER, text, egui::FontId::proportional(11.0), col);
    }
}

const RAIL_X: f32 = 16.0;

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
    max_ms: u128,
    actions: &mut Vec<Action>,
    depth: usize,
) {
    let desc = s.tool(&node.tool);
    let known = desc.is_some();
    let cat_color = desc.map(|d| category_color(p, &d.category)).unwrap_or(p.danger);
    let selected = s.flow_sel.as_ref() == Some(path);
    let enabled = node.enabled;
    let fill = if !enabled {
        egui::Color32::TRANSPARENT
    } else if selected {
        mix(p.raised, p.accent, 0.07)
    } else {
        p.raised
    };
    let border = if selected { p.accent_line } else if !known { p.danger } else { p.border };

    let frame = Frame::new()
        .fill(fill)
        .stroke(if enabled { Stroke::new(1.0, border) } else { Stroke::NONE })
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin { left: 36, right: 10, top: 8, bottom: 8 });
    let inner = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        if !enabled {
            ui.multiply_opacity(0.55);
        }
        ui.horizontal(|ui| {
            let handle = ui.dnd_drag_source(egui::Id::unique(("step_drag", node.uid)), StepDrag(path.clone()), |ui| {
                ui.label(RichText::new(icon::DOTS_SIX_VERTICAL).color(p.text_faint));
            });
            handle.response.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text("Drag to reorder");
            let title = node.label.clone().unwrap_or_else(|| desc.map(|d| d.label.clone()).unwrap_or_else(|| node.tool.clone()));
            ui.label(RichText::new(title).font(crate::theme::font_semibold(13.0)).color(if enabled { p.text } else { p.text_muted }));
            if !known {
                chip(ui, "not available", p.danger, with_alpha(p.danger, 24));
            }
            if !enabled {
                chip(ui, "skipped", p.text_muted, p.sunken);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let hovered = ui.rect_contains_pointer(ui.max_rect().expand2(vec2(600.0, 10.0)));
                if hovered || selected {
                    if icon_button(ui, p, icon::TRASH, "Remove step", false).clicked() {
                        actions.push(Action::Remove(path.clone()));
                    }
                    if icon_button(ui, p, icon::COPY, "Duplicate", false).clicked() {
                        actions.push(Action::Duplicate(path.clone()));
                    }
                }
                let mut on = enabled;
                if widgets::toggle(ui, p, &mut on).on_hover_text(if enabled { "Skip this step" } else { "Use this step" }).clicked() {
                    actions.push(Action::Toggle(path.clone()));
                }
                if let Some(r) = rep.filter(|r| r.enabled) {
                    time_bar(ui, p, r, max_ms, cat_color);
                }
            });
        });
        summary_line(ui, p, node);
        if is_container(&node.tool) {
            branches(s, ui, p, node, path, actions, depth);
        }
    });
    let rect = inner.response.rect;
    // Rail node
    let c = pos2(rect.left() + RAIL_X, rect.top() + 20.0);
    if enabled {
        ui.painter().circle_filled(c, 7.0, cat_color);
        ui.painter().circle_stroke(c, 7.0, Stroke::new(2.0, p.panel));
    } else {
        ui.painter().circle_stroke(c, 6.0, Stroke::new(1.4, p.text_faint));
        dashed_rect(ui.painter(), rect, Stroke::new(1.0, p.border_strong));
    }
    ui.painter().text(c + vec2(0.0, 16.0), Align2::CENTER_TOP, format!("{}", index + 1), font_mono(9.5), p.text_faint);

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
        ui.painter().circle_filled(pos2(rect.left() + RAIL_X, y), 4.0, p.accent);
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

fn dashed_rect(painter: &egui::Painter, r: Rect, st: Stroke) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.add(egui::Shape::dashed_line(&pts, st, 5.0, 4.0));
}

/// Elapsed time as a small bar plus the trace count.
fn time_bar(ui: &mut Ui, p: &Palette, r: &StepReport, max_ms: u128, col: egui::Color32) {
    ui.label(RichText::new(fmt_ms(r.elapsed_ms as f64)).font(font_mono(10.5)).color(p.text_muted));
    let (rect, _) = ui.allocate_exact_size(vec2(64.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), p.sunken);
    let mut f = rect;
    f.set_width((rect.width() * r.elapsed_ms as f32 / max_ms as f32).max(2.0));
    ui.painter().rect_filled(f, CornerRadius::same(3), col);
    ui.label(RichText::new(format!("{} tr", fmt_count(r.traces_out))).font(font_mono(10.5)).color(p.text_faint));
}

fn summary_line(ui: &mut Ui, p: &Palette, node: &FlowNode) {
    let mut parts: Vec<String> = Vec::new();
    if let Some(obj) = node.params.as_object() {
        let flat = obj.get("params").and_then(|v| v.as_object()).cloned();
        let mut push = |k: &str, v: &serde_json::Value| {
            if parts.len() >= 4 {
                return;
            }
            let key = super::inspector::humanize(k);
            let txt = match v {
                serde_json::Value::String(s) if !s.is_empty() => {
                    let short = if s.chars().count() > 32 {
                        let tail: String = s.chars().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
                        format!("…{tail}")
                    } else {
                        s.clone()
                    };
                    format!("{key} {short}")
                }
                serde_json::Value::Number(n) => format!("{key} {n}"),
                serde_json::Value::Bool(b) => format!("{key} {}", if *b { "on" } else { "off" }),
                serde_json::Value::Array(a) if !a.is_empty() && a.len() <= 4 && a.iter().all(|x| x.is_number()) => {
                    format!("{key} {}", a.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" / "))
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
            parts = vec![format!("script: {first}")];
        }
    }
    if !parts.is_empty() {
        ui.add_space(1.0);
        ui.label(RichText::new(parts.join("   ·   ")).size(11.5).color(p.text_muted));
    }
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
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Empty: drop a step here", egui::FontId::proportional(11.0), p.text_faint);
                    if let Some(t) = resp.dnd_release_payload::<ToolDrag>() {
                        actions.push(Action::Insert { parent: bpath.clone(), index: 0, tool: t.0.clone() });
                    } else if let Some(sd) = resp.dnd_release_payload::<StepDrag>() {
                        if !bpath.starts_with(&sd.0) {
                            actions.push(Action::Move { from: sd.0.clone(), parent: bpath.clone(), index: 0 });
                        }
                    }
                } else {
                    list(s, ui, p, steps, &bpath, &[], 1, actions, depth + 1);
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
    ui.add(egui::TextEdit::singleline(&mut q).hint_text("Find a step…").desired_width(220.0)).request_focus();
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
            if ui.button(format!("{}  Apply", icon::CHECK)).clicked() {
                apply = true;
            }
            if ui.button(format!("{}  Revert", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
                text = serde_json::to_string_pretty(&s.flow.to_spec()).unwrap_or_default();
            }
            let valid = serde_json::from_str::<rustseis_flow::FlowSpec>(&text);
            match valid {
                Ok(_) => {
                    ui.label(RichText::new(format!("{} looks valid", icon::CHECK)).color(p.success).size(11.5));
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
                s.push_flow_undo();
                let path = s.flow.path.clone();
                s.flow = crate::flowdoc::FlowDoc::from_spec(&spec, path);
                s.flow.dirty = true;
                s.flow_sel = None;
                s.toast(Level::Ok, "Flow updated from text");
                ui.data_mut(|d| d.remove::<String>(id));
                return;
            }
            Err(e) => s.toast(Level::Error, format!("The flow text has a problem: {e}")),
        }
    }
    ui.data_mut(|d| d.insert_temp(id, text));
}

fn apply(s: &mut AppState, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::Select(p) => s.flow_sel = Some(p),
            Action::Toggle(p) => {
                if s.flow.get(&p).is_some() {
                    s.push_flow_undo();
                }
                if let Some(n) = s.flow.get_mut(&p) {
                    n.enabled = !n.enabled;
                    s.flow.dirty = true;
                }
            }
            Action::Remove(p) => {
                if s.flow.get(&p).is_some() {
                    s.push_flow_undo();
                }
                s.flow.remove(&p);
                if s.flow_sel.as_ref().is_some_and(|sel| sel.starts_with(&p)) {
                    s.flow_sel = None;
                }
            }
            Action::Duplicate(p) => {
                if s.flow.get(&p).is_some() {
                    s.push_flow_undo();
                }
                if let Some(np) = s.flow.duplicate(&p) {
                    s.flow_sel = Some(np);
                }
            }
            Action::Move { from, parent, index } => {
                s.push_flow_undo();
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
                s.push_flow_undo();
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
                if s.flow.get(&p).is_some() {
                    s.push_flow_undo();
                }
                if let Some(n) = s.flow.get_mut(&p) {
                    n.branches.push(Vec::new());
                    s.flow.dirty = true;
                }
            }
            Action::RemoveBranch(p, b) => {
                if s.flow.get(&p).is_some_and(|n| b < n.branches.len()) {
                    s.push_flow_undo();
                }
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
