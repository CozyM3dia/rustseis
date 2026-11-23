//! Workspace 1 · left: searchable Tier 1 + Tier 2 process library (`full_tool_catalog()`).

use crate::state::AppState;
use crate::theme::{Palette, font_mono, palette, with_alpha};
use crate::widgets;
use egui::{CollapsingHeader, RichText, ScrollArea, Ui};
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
    widgets::toolbar(ui, &p, |ui| {
        ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(p.text_muted));
        let r = ui.add(
            egui::TextEdit::singleline(&mut query)
                .hint_text("Search 61 tools — name, id, category…")
                .desired_width(ui.available_width()),
        );
        if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            r.request_focus();
        }
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
        widgets::body(ui, |ui| {
            ui.label(
                RichText::new(format!("{total} tools · double-click or drag onto the flow"))
                    .size(11.0)
                    .color(p.text_faint),
            );
            for (cat, tools) in &groups {
                let color = category_color(&p, cat);
                CollapsingHeader::new(
                    RichText::new(format!("{cat}  ")).strong().color(p.text),
                )
                .id_salt(("cat", cat))
                .default_open(!q.is_empty() || category_rank(cat) <= 1)
                .show(ui, |ui| {
                    for t in tools {
                        let tier2 = s.tier2.contains(&t.menu_id);
                        let row_id = egui::Id::unique(("tool_drag", &t.menu_id));
                        let inner = ui.dnd_drag_source(row_id, ToolDrag(t.menu_id.clone()), |ui| {
                            tool_row(ui, &p, t, tier2, color);
                        });
                        let resp = inner.response;
                        if resp.double_clicked() {
                            add = Some(t.menu_id.clone());
                        }
                        resp.on_hover_ui(|ui| {
                            ui.label(RichText::new(&t.label).strong());
                            ui.label(RichText::new(&t.menu_id).font(font_mono(11.0)).color(p.accent));
                            ui.label(&t.description);
                            ui.label(RichText::new(format!("{:?} · {}", t.archetype, if tier2 { "Tier 2" } else { "Tier 1" })).size(11.0).color(p.text_faint));
                        });
                    }
                });
            }
        });
    });
    if let Some(id) = add {
        s.insert_tool(&id);
    }
}

fn tool_row(ui: &mut Ui, p: &Palette, t: &ToolDescriptor, tier2: bool, color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(archetype_icon(t.archetype)).color(color));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if tier2 {
                widgets::chip(ui, "T2", p.accent, with_alpha(p.accent, 22));
            }
            let id_w = ui.available_width() * 0.42;
            ui.add_sized(
                [id_w.min(130.0), 16.0],
                egui::Label::new(RichText::new(&t.menu_id).font(font_mono(10.5)).color(p.text_faint)).truncate(),
            );
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(&t.label).size(12.5)).truncate());
            });
        });
    });
}
