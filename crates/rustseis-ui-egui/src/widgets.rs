//! Small composable pieces of RustSeis chrome. Panes build from these so spacing, borders and
//! type stay consistent.

use crate::theme::{Palette, RADIUS_MD, RADIUS_SM, font_display, font_mono, mix, space, with_alpha};
use egui::{
    Align2, Color32, CornerRadius, Frame, Margin, Response, RichText, Sense, Stroke, StrokeKind, Ui,
    Vec2, WidgetText, pos2, vec2,
};

/// Horizontal toolbar strip at the top of a pane.
pub fn toolbar<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    let resp = Frame::new()
        .fill(p.panel)
        .inner_margin(Margin { left: 8, right: 8, top: 5, bottom: 5 })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                add(ui)
            })
            .inner
        });
    let r = resp.response.rect;
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.border));
    resp.inner
}

/// Thin vertical divider inside toolbars.
pub fn vsep(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 18.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, p.border));
}

/// Card container (raised surface with hairline border).
pub fn card<R>(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(RADIUS_MD))
        .inner_margin(Margin::same(8))
        .show(ui, add)
        .inner
}

/// Pane-body padding.
pub fn body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new().inner_margin(Margin::same(8)).show(ui, add).inner
}

/// Section heading: display face, muted, with an optional trailing hint.
pub fn section(ui: &mut Ui, p: &Palette, title: &str, hint: Option<&str>) {
    ui.add_space(space::SM);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(font_display(12.5)).color(p.text));
        if let Some(h) = hint {
            ui.label(RichText::new(h).size(11.0).color(p.text_faint));
        }
    });
    ui.add_space(space::XS);
}

/// Icon-only button with tooltip. `active` paints it in the selected state.
pub fn icon_button(ui: &mut Ui, p: &Palette, icon: &str, tooltip: &str, active: bool) -> Response {
    let size = vec2(26.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();
    let enabled = ui.is_enabled();
    let (fill, stroke, fg) = if active {
        (p.accent_soft, Stroke::new(1.0, with_alpha(p.accent, 170)), p.accent)
    } else if hovered && enabled {
        (mix(p.panel, p.raised, 0.9), Stroke::new(1.0, p.border_strong), p.text)
    } else {
        (Color32::TRANSPARENT, Stroke::NONE, if enabled { p.text_muted } else { p.text_faint })
    };
    ui.painter().rect(rect, CornerRadius::same(RADIUS_SM), fill, stroke, StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, egui::FontId::proportional(15.0), fg);
    resp.on_hover_text(tooltip)
}

/// Text button with leading icon.
pub fn button(ui: &mut Ui, icon: &str, text: &str) -> Response {
    ui.button(format!("{icon}  {text}"))
}

/// Primary call-to-action button (accent fill).
pub fn primary_button(ui: &mut Ui, p: &Palette, icon: &str, text: &str) -> Response {
    let label = WidgetText::from(RichText::new(format!("{icon}  {text}")).color(p.on_accent).strong());
    let b = egui::Button::new(label)
        .fill(if ui.is_enabled() { p.accent } else { mix(p.raised, p.accent, 0.3) })
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(RADIUS_SM));
    ui.add(b)
}

/// Segmented control over `options`; returns true when the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, p: &Palette, value: &mut T, options: &[(T, &str, &str)]) -> bool {
    let mut changed = false;
    let font = egui::FontId::proportional(12.0);
    let pad = vec2(9.0, 3.0);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label, _)| ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.text).size().x + pad.x * 2.0)
        .collect();
    let total = vec2(widths.iter().sum::<f32>() + 2.0, 22.0);
    let (rect, _) = ui.allocate_exact_size(total, Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(RADIUS_SM), p.sunken, Stroke::new(1.0, p.border), StrokeKind::Inside);
    let mut x = rect.left() + 1.0;
    for ((opt, label, tip), w) in options.iter().zip(widths) {
        let r = egui::Rect::from_min_size(pos2(x, rect.top() + 1.0), vec2(w, rect.height() - 2.0));
        let id = egui::Id::unique(("seg", *label, r.min.x as i32, r.min.y as i32));
        let resp = ui.interact(r, id, Sense::click()).on_hover_text(*tip);
        let selected = *value == *opt;
        if selected {
            ui.painter().rect(r, CornerRadius::same(RADIUS_SM - 1), p.raised, Stroke::new(1.0, with_alpha(p.accent, 140)), StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(RADIUS_SM - 1), with_alpha(p.raised, 120));
        }
        let color = if selected { p.accent } else if resp.hovered() { p.text } else { p.text_muted };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *label, font.clone(), color);
        if resp.clicked() && !selected {
            *value = *opt;
            changed = true;
        }
        x += w;
    }
    changed
}

/// Rounded chip (status, counts, tiers).
pub fn chip(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) -> Response {
    let font = font_mono(10.5);
    let g = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let size = g.size() + vec2(10.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(9), bg, Stroke::new(1.0, with_alpha(fg, 70)), StrokeKind::Inside);
    ui.painter().galley(rect.min + vec2(5.0, 2.0), g, fg);
    resp
}

/// Compact numeric stat: small caption over a mono value.
pub fn stat(ui: &mut Ui, p: &Palette, caption: &str, value: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(RichText::new(caption).size(10.5).color(p.text_faint));
        ui.label(RichText::new(value).font(font_mono(13.0)).color(p.text));
    });
}

/// Key/value row for property grids.
pub fn kv(ui: &mut Ui, p: &Palette, key: &str, value: impl Into<String>) {
    ui.label(RichText::new(key).color(p.text_muted));
    ui.label(RichText::new(value.into()).font(font_mono(11.5)).color(p.text));
    ui.end_row();
}

/// Inline hint line (muted small text with an icon).
pub fn hint(ui: &mut Ui, p: &Palette, icon: &str, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(icon).color(p.text_faint));
        ui.label(RichText::new(text).size(11.5).color(p.text_faint));
    });
}

/// Pane-level status/notice banner.
pub fn banner(ui: &mut Ui, p: &Palette, color: Color32, icon: &str, text: &str) {
    Frame::new()
        .fill(with_alpha(color, 26))
        .stroke(Stroke::new(1.0, with_alpha(color, 90)))
        .corner_radius(CornerRadius::same(RADIUS_SM))
        .inner_margin(Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(icon).color(color));
                ui.label(RichText::new(text).size(12.0).color(p.text));
            });
        });
}

/// Compact labelled drag value (`label [value unit]`).
pub fn drag_f32(ui: &mut Ui, label: &str, v: &mut f32, speed: f64, range: std::ops::RangeInclusive<f32>, suffix: &str) -> Response {
    ui.label(RichText::new(label).size(11.5));
    ui.add(egui::DragValue::new(v).speed(speed).range(range).suffix(suffix).max_decimals(2))
}

/// Animated indeterminate / determinate progress line.
pub fn progress_line(ui: &mut Ui, p: &Palette, width: f32, fraction: Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 3.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), p.sunken);
    match fraction {
        Some(f) => {
            let mut r = rect;
            r.set_width(rect.width() * f.clamp(0.0, 1.0));
            painter.rect_filled(r, CornerRadius::same(2), p.accent);
        }
        None => {
            let t = ui.input(|i| i.time) as f32;
            let seg = rect.width() * 0.28;
            let x = rect.left() + ((t * 0.7).fract() * (rect.width() + seg)) - seg;
            let r = egui::Rect::from_min_max(pos2(x.max(rect.left()), rect.top()), pos2((x + seg).min(rect.right()), rect.bottom()));
            painter.rect_filled(r, CornerRadius::same(2), p.accent);
            ui.ctx().request_repaint();
        }
    }
}

/// Minimum-size helper: fills remaining space with a centered placeholder.
pub fn fill_remaining(ui: &mut Ui) -> Vec2 {
    ui.available_size_before_wrap().max(vec2(64.0, 64.0))
}
