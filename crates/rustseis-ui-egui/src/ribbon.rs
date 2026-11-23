//! ArcGIS-Pro-inspired multi-tier Ribbon primitives:
//! - Named groups with centered bottom labels and crisp vertical dividers
//! - Large priority tool tiles (30×30 multi-color icon + 1–2 line centered label + optional caret)
//! - Stacked compact secondary tool rows (2–3 rows of 16×16 multi-color icon + left-aligned label)
//! - Rich dropdown menu items paired with 16×16 multi-color tool icons

use crate::icons::{self, Icon};
use crate::theme::{Palette, ThemeMode, font_semibold, mix, with_alpha};
use egui::{Align2, Color32, CornerRadius, FontId, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as glyph;

/// Total inner height allocated for the ribbon controls + bottom group label.
pub const RIBBON_INNER_H: f32 = 88.0;
/// Height of the top button row inside each ribbon group.
pub const RIBBON_BODY_H: f32 = 68.0;
/// Height of the centered group title along the bottom of each ribbon group.
pub const RIBBON_FOOTER_H: f32 = 16.0;

/// Draw a named Ribbon group: controls in the top 68 px, centered muted (or accent) group label
/// along the bottom edge.
pub fn group(ui: &mut Ui, p: &Palette, title: &str, title_color: Option<Color32>, add_contents: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let body = ui.horizontal(|ui| {
            ui.set_min_height(RIBBON_BODY_H);
            ui.set_max_height(RIBBON_BODY_H);
            ui.spacing_mut().item_spacing.x = 4.0;
            add_contents(ui);
        });
        let font = if title_color.is_some() { font_semibold(10.5) } else { FontId::proportional(10.5) };
        let title_w = ui.painter().layout_no_wrap(title.to_string(), font.clone(), p.text_faint).size().x + 8.0;
        let content_w = body.response.rect.width().max(title_w).max(52.0);
        let (lbl_rect, _) = ui.allocate_exact_size(vec2(content_w, RIBBON_FOOTER_H), Sense::hover());
        let col = title_color.unwrap_or(p.text_faint);
        ui.painter().text(lbl_rect.center(), Align2::CENTER_CENTER, title, font, col);
    });
}

/// Subtle vertical divider between Ribbon groups (matching ArcGIS Pro's thin muted separator).
pub fn divider(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(12.0, RIBBON_INNER_H - 4.0), Sense::hover());
    let y0 = rect.top() + 5.0;
    let y1 = rect.bottom() - 3.0;
    ui.painter().vline(rect.center().x, y0..=y1, Stroke::new(1.0, with_alpha(p.border_strong, 190)));
}

/// Accent vertical divider separating stage groups from right-hand contextual document groups.
pub fn context_divider(ui: &mut Ui, _p: &Palette, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, RIBBON_INNER_H - 4.0), Sense::hover());
    let y0 = rect.top() + 4.0;
    let y1 = rect.bottom() - 4.0;
    ui.painter().vline(rect.center().x, y0..=y1, Stroke::new(1.5, with_alpha(accent, 130)));
}

/// Large priority ribbon button: 30×30 multi-color vector icon stacked above 1 or 2 crisp lines.
#[allow(clippy::too_many_arguments)]
pub fn large_button(
    ui: &mut Ui,
    p: &Palette,
    icon: Icon,
    line1: &str,
    line2: &str,
    active: bool,
    primary: bool,
    has_menu: bool,
) -> Response {
    let font = if primary { font_semibold(11.5) } else { FontId::proportional(11.5) };
    let caret_suffix = format!(" {}", glyph::CARET_DOWN);
    let l1_display = if line2.is_empty() && has_menu { format!("{line1}{caret_suffix}") } else { line1.to_string() };
    let l2_display = if !line2.is_empty() && has_menu { format!("{line2}{caret_suffix}") } else { line2.to_string() };

    let w1 = ui.painter().layout_no_wrap(l1_display.clone(), font.clone(), p.text).size().x;
    let w2 = if l2_display.is_empty() {
        0.0
    } else {
        ui.painter().layout_no_wrap(l2_display.clone(), font.clone(), p.text).size().x
    };
    let width = w1.max(w2).max(42.0) + 14.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, RIBBON_BODY_H), Sense::click());
    let enabled = ui.is_enabled();
    let hovered = resp.hovered() && enabled;

    let (fill, stroke, text_col) = tile_colors(p, active, primary, hovered, enabled);
    if fill != Color32::TRANSPARENT || stroke != Stroke::NONE {
        ui.painter().rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
    }

    let icon_rect = egui::Rect::from_center_size(pos2(rect.center().x, rect.top() + 20.0), vec2(30.0, 30.0));
    icons::paint(ui.painter(), icon_rect, icon, p);

    if l2_display.is_empty() {
        ui.painter().text(pos2(rect.center().x, rect.top() + 48.0), Align2::CENTER_CENTER, l1_display, font, text_col);
    } else {
        ui.painter().text(pos2(rect.center().x, rect.top() + 44.0), Align2::CENTER_CENTER, l1_display, font.clone(), text_col);
        ui.painter().text(pos2(rect.center().x, rect.top() + 57.5), Align2::CENTER_CENTER, l2_display, font, text_col);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Vertical column holding 2 or 3 compact secondary ribbon buttons cleanly aligned beside large buttons.
pub fn stack(ui: &mut Ui, add_rows: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.set_min_height(RIBBON_BODY_H);
        ui.spacing_mut().item_spacing.y = 1.5;
        add_rows(ui);
    });
}

/// Compact secondary ribbon button (21 px tall): 16×16 multi-color icon + left-aligned label.
pub fn small_button(ui: &mut Ui, p: &Palette, icon: Icon, label: &str, active: bool, has_menu: bool) -> Response {
    let font = FontId::proportional(11.5);
    let display = if has_menu { format!("{label} {}", glyph::CARET_DOWN) } else { label.to_string() };
    let text_w = ui.painter().layout_no_wrap(display.clone(), font.clone(), p.text).size().x;
    let width = (26.0 + text_w + 8.0).max(66.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 21.0), Sense::click());
    let enabled = ui.is_enabled();
    let hovered = resp.hovered() && enabled;

    let (fill, stroke, text_col) = tile_colors(p, active, false, hovered, enabled);
    if fill != Color32::TRANSPARENT || stroke != Stroke::NONE {
        ui.painter().rect(rect, CornerRadius::same(4), fill, stroke, StrokeKind::Inside);
    }

    let icon_rect = egui::Rect::from_center_size(pos2(rect.left() + 12.0, rect.center().y), vec2(15.0, 15.0));
    icons::paint(ui.painter(), icon_rect, icon, p);
    ui.painter().text(pos2(rect.left() + 25.0, rect.center().y), Align2::LEFT_CENTER, display, font, text_col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Rich menu item inside a ribbon dropdown popup: 16×16 multi-color icon + label.
pub fn menu_item(ui: &mut Ui, p: &Palette, icon: Icon, label: &str) -> Response {
    let font = FontId::proportional(12.5);
    let text_w = ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.text).size().x;
    let width = ui.available_width().max(text_w + 34.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 25.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(5), p.raised);
    }
    let icon_rect = egui::Rect::from_center_size(pos2(rect.left() + 13.0, rect.center().y), vec2(15.0, 15.0));
    icons::paint(ui.painter(), icon_rect, icon, p);
    ui.painter().text(pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, label, font, p.text);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn tile_colors(p: &Palette, active: bool, primary: bool, hovered: bool, enabled: bool) -> (Color32, Stroke, Color32) {
    if !enabled {
        return (Color32::TRANSPARENT, Stroke::NONE, p.text_faint);
    }
    let active_tile = match p.mode {
        ThemeMode::Dark => Color32::from_rgb(44, 52, 66),
        ThemeMode::Light => mix(p.raised, p.info, 0.14),
    };
    let active_border = match p.mode {
        ThemeMode::Dark => Color32::from_rgb(76, 94, 120),
        ThemeMode::Light => with_alpha(p.info, 120),
    };
    if primary {
        let base = if p.mode == ThemeMode::Dark {
            mix(p.raised, p.accent, 0.22)
        } else {
            p.accent_soft
        };
        let fill = if hovered { mix(base, p.accent, 0.22) } else { base };
        let stroke = Stroke::new(1.0, if hovered { p.accent } else { p.accent_line });
        (fill, stroke, if p.mode == ThemeMode::Dark { p.text } else { p.accent_text })
    } else if active {
        let fill = if hovered { mix(active_tile, p.text, 0.06) } else { active_tile };
        (fill, Stroke::new(1.0, active_border), p.text)
    } else if hovered {
        (p.raised, Stroke::new(1.0, p.border_strong), p.text)
    } else {
        (Color32::TRANSPARENT, Stroke::NONE, mix(p.text, p.text_muted, 0.18))
    }
}
