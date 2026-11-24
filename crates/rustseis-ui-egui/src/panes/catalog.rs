//! Library panel: every processing step, searchable and grouped by category.

use crate::state::AppState;
use crate::theme::{Palette, palette};
use egui::{RichText, ScrollArea, Ui};
use egui_phosphor::regular as icon;
use rustseis_flow::{ToolArchetype, ToolDescriptor};

/// Drag payload: a catalog tool being dropped onto the flow editor.
#[derive(Clone, Debug)]
pub struct ToolDrag(pub String);

pub fn archetype_icon(a: ToolArchetype) -> &'static str {
    match a {
        ToolArchetype::Input => icon::DOWNLOAD,
        ToolArchetype::Output => icon::UPLOAD,
        ToolArchetype::SimpleTrace => icon::WAVE_SINE,
        ToolArchetype::Ensemble => icon::STACK,
        ToolArchetype::Panel => icon::GRID_NINE,
        ToolArchetype::Complex => icon::CUBE,
        ToolArchetype::FlowControl => icon::GIT_BRANCH,
        ToolArchetype::ExternalPlugin => icon::FILE_PY,
    }
}

pub fn category_rank(cat: &str) -> usize {
    const ORDER: &[&str] = &[
        "Input / Output", "Geometry", "Editing", "Amplitude", "Filtering", "Deconvolution", "Data Enhancement",
        "Statics", "Refraction", "Velocity", "Stacking", "Migration", "Modeling", "VSP", "Flow Control", "Plugins",
    ];
    ORDER.iter().position(|k| cat.contains(k)).unwrap_or(ORDER.len())
}

/// Category tint so related steps share a colour in the flow editor.
pub fn category_color(p: &Palette, cat: &str) -> egui::Color32 {
    let r = category_rank(cat);
    match r {
        0 => p.info,
        1 => p.series_c,
        2..=3 => p.pick,
        4..=6 => p.accent,
        7..=8 => p.success,
        9..=11 => p.series_b,
        12 => p.text_muted,
        13 => p.info,
        _ => p.text_muted,
    }
}

#[derive(Default)]
pub struct CatalogFilter {
    pub query: String,
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    let id = egui::Id::unique("catalog_query");
    let mut query: String = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        egui::Frame::new().fill(p.sunken).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(egui::CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(p.text_muted));
                let r = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Find a step").frame(egui::Frame::NONE).desired_width(ui.available_width()));
                if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                    r.request_focus();
                }
            });
        });
    });
    ui.data_mut(|d| d.insert_temp(id, query.clone()));

    let q = query.to_ascii_lowercase();
    let mut groups: Vec<(String, Vec<ToolDescriptor>)> = Vec::new();
    for t in &s.catalog {
        let hit = q.is_empty()
            || t.label.to_ascii_lowercase().contains(&q)
            || t.menu_id.contains(&q)
            || t.category.to_ascii_lowercase().contains(&q)
            || t.description.to_ascii_lowercase().contains(&q);
        if !hit {
            continue;
        }
        match groups.iter_mut().find(|(c, _)| *c == t.category) {
            Some((_, v)) => v.push(t.clone()),
            None => groups.push((t.category.clone(), vec![t.clone()])),
        }
    }
    groups.sort_by_key(|(c, _)| category_rank(c));
    let total: usize = groups.iter().map(|(_, v)| v.len()).sum();

    let mut add: Option<String> = None;
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 12 }).show(ui, |ui| {
            ui.label(RichText::new(format!("{total} steps · double-click or drag one onto the flow")).size(11.0).color(p.text_faint));
            for (cat, tools) in &groups {
                let color = category_color(&p, cat);
                let header = egui::CollapsingHeader::new(RichText::new(format!("{cat}   {}", tools.len())).font(crate::theme::font_semibold(12.0)).color(p.text))
                    .id_salt(("cat", cat))
                    .default_open(!q.is_empty() || category_rank(cat) <= 1)
                    .show(ui, |ui| {
                        for t in tools {
                            let row_id = egui::Id::unique(("tool_drag", &t.menu_id));
                            let mut add_clicked = false;
                            let scoped = ui.scope_builder(
                                egui::UiBuilder::new().id_salt(row_id).sense(egui::Sense::click_and_drag()),
                                |ui| {
                                    add_clicked = tool_row(ui, &p, t, color);
                                },
                            );
                            let resp = scoped.response;
                            if resp.drag_started() {
                                egui::DragAndDrop::set_payload(ui.ctx(), ToolDrag(t.menu_id.clone()));
                            }
                            if resp.double_clicked() || add_clicked {
                                add = Some(t.menu_id.clone());
                            }
                            resp.on_hover_ui(|ui| {
                                ui.set_max_width(300.0);
                                ui.label(RichText::new(&t.label).strong());
                                ui.label(crate::ui_copy::clean(&t.description));
                            });
                        }
                    });
                let r = header.header_response.rect;
                ui.painter().circle_filled(egui::pos2(r.right() - 8.0, r.center().y), 3.5, color);
            }
        });
    });
    if let Some(id) = add {
        s.insert_tool(&id);
        s.doc_request = Some(crate::state::Doc::Flow);
    }
}

fn tool_row(ui: &mut Ui, p: &Palette, t: &ToolDescriptor, _color: egui::Color32) -> bool {
    let mut clicked_add = false;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 27.0), egui::Sense::hover());
    let row_hovered = resp.hovered() || ui.rect_contains_pointer(rect);
    if row_hovered {
        ui.painter().rect_filled(rect, egui::CornerRadius::same(6), p.raised);
    }
    let ic_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 14.0, rect.center().y), egui::vec2(16.0, 16.0));
    crate::icons::paint(ui.painter(), ic_rect, crate::icons::for_tool(&t.menu_id, &t.category), p);
    let btn_rect = egui::Rect::from_center_size(egui::pos2(rect.right() - 14.0, rect.center().y), egui::vec2(22.0, 20.0));
    let clip = egui::Rect::from_min_max(egui::pos2(rect.left() + 30.0, rect.top()), egui::pos2(btn_rect.left() - 4.0, rect.bottom()));
    ui.painter().with_clip_rect(clip).text(egui::pos2(rect.left() + 30.0, rect.center().y), egui::Align2::LEFT_CENTER, &t.label, egui::FontId::proportional(12.5), p.text);
    let btn_resp = ui.interact(btn_rect, egui::Id::unique(("tool_add_btn", &t.menu_id)), egui::Sense::click());
    btn_resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), format!("Add {}", t.label)));
    let btn_resp = btn_resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Add step to flow");
    if btn_resp.hovered() {
        ui.painter().rect_filled(btn_rect, egui::CornerRadius::same(4), p.accent_soft);
    }
    ui.painter().text(
        btn_rect.center(),
        egui::Align2::CENTER_CENTER,
        icon::PLUS,
        egui::FontId::proportional(12.0),
        if btn_resp.hovered() { p.accent_text } else if row_hovered { p.text } else { p.text_faint },
    );
    if btn_resp.clicked() {
        clicked_add = true;
    }
    clicked_add
}
