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
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, tooltip));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip)
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

// ------------------------------------------------------------------------------------------
// Interface v2 primitives
// ------------------------------------------------------------------------------------------

/// Floating panel surface (Explorer, documents, Inspector).
pub fn panel_frame(p: &Palette) -> Frame {
    Frame::new()
        .fill(p.panel)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(crate::theme::RADIUS_LG))
}

/// Translucent overlay used on top of seismic canvases.
pub fn glass_frame(p: &Palette) -> Frame {
    let fill = if p.mode == crate::theme::ThemeMode::Dark {
        Color32::from_rgba_unmultiplied(15, 18, 22, 214)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 228)
    };
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, with_alpha(p.text, 24)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 6))
        .shadow(egui::Shadow { offset: [0, 8], blur: 22, spread: 0, color: Color32::from_black_alpha(90) })
}

/// Command-bar button: phosphor icon + label.
pub fn cmd(ui: &mut Ui, p: &Palette, icon: &str, label: &str, active: bool) -> Response {
    let font = egui::FontId::proportional(12.5);
    let access_label = label.to_string();
    // A trailing "▾" marks a menu; draw it with the icon font's caret.
    let label = label.replace('▾', egui_phosphor::regular::CARET_DOWN);
    let text = if icon.is_empty() { label } else { format!("{icon}  {label}") };
    let g = ui.painter().layout_no_wrap(text.clone(), font.clone(), p.text);
    let size = vec2(g.size().x + 20.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let hovered = resp.hovered() && enabled;
    let (fill, stroke, fg) = if active {
        (p.accent_soft, Stroke::new(1.0, p.accent_line), p.accent_text)
    } else if hovered {
        (p.raised, Stroke::NONE, p.text)
    } else {
        (Color32::TRANSPARENT, Stroke::NONE, if enabled { mix(p.text, p.text_muted, 0.25) } else { p.text_faint })
    };
    ui.painter().rect(rect, CornerRadius::same(7), fill, stroke, StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font, fg);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, access_label.as_str()));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Primary command (accent fill).
pub fn cmd_primary(ui: &mut Ui, p: &Palette, icon: &str, label: &str) -> Response {
    let font = crate::theme::font_semibold(12.5);
    let text = if icon.is_empty() { label.to_string() } else { format!("{icon}  {label}") };
    let g = ui.painter().layout_no_wrap(text.clone(), font.clone(), p.on_accent);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 22.0, 32.0), Sense::click());
    let enabled = ui.is_enabled();
    let fill = if !enabled { mix(p.raised, p.accent, 0.3) } else if resp.hovered() { mix(p.accent, p.text, 0.12) } else { p.accent };
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font, p.on_accent);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn cmd_sep(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(13.0, 22.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, p.border_strong));
}

/// Small caps label inside the command bar (e.g. "ADD").
pub fn dim_label(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).font(crate::theme::font_semibold(10.5)).color(p.text_faint));
}

/// Amber contextual label (e.g. "GATHER", "PICKING").
pub fn ctx_label(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).font(crate::theme::font_semibold(10.5)).color(p.pick));
}

/// Section header inside panels: small caps, optional trailing hint.
pub fn caps(ui: &mut Ui, p: &Palette, text: &str, hint: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).font(crate::theme::font_semibold(11.0)).color(p.text_faint));
        if let Some(h) = hint {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(h).size(11.5).color(p.text_muted));
            });
        }
    });
}

/// Switch control.
pub fn toggle(ui: &mut Ui, p: &Palette, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(32.0, 18.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let fill = mix(p.border_strong, p.accent, t);
    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    let knob = if *on { p.on_accent } else { p.text_muted };
    ui.painter().circle_filled(pos2(x, rect.center().y), 7.0, knob);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *on, ""));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Labelled row with content aligned right.
pub fn row(ui: &mut Ui, p: &Palette, key: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(key).color(p.text_muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}

/// Mono value text helper.
pub fn mono(text: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(text.into()).font(font_mono(size)).color(color)
}

/// Small coloured status dot.
pub fn dot(ui: &mut Ui, color: Color32, r: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(r * 2.0 + 2.0, r * 2.0 + 2.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), r, color);
}

/// Insight card: something RustSeis noticed, with up to two actions. Returns (primary, secondary).
pub fn insight(ui: &mut Ui, p: &Palette, title: &str, body: &str, primary: &str, secondary: &str) -> (bool, bool) {
    let mut out = (false, false);
    Frame::new()
        .fill(mix(p.panel, p.accent, if p.mode == crate::theme::ThemeMode::Dark { 0.07 } else { 0.06 }))
        .stroke(Stroke::new(1.0, p.accent_line))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(egui_phosphor::regular::LIGHTBULB).color(p.accent));
                ui.label(RichText::new(title).font(crate::theme::font_semibold(12.5)).color(p.accent_text));
            });
            ui.label(RichText::new(body).size(12.0).color(p.text_muted));
            ui.horizontal(|ui| {
                if !primary.is_empty() && cmd_primary(ui, p, "", primary).clicked() {
                    out.0 = true;
                }
                if !secondary.is_empty() && cmd(ui, p, "", secondary, false).clicked() {
                    out.1 = true;
                }
            });
        });
    out
}

/// The RustSeis mark: brand disc with a wavelet.
pub fn logo(ui: &mut Ui, p: &Palette, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let painter = ui.painter();
    let c = rect.center();
    let r = size * 0.5 - 0.5;
    let steps = 14;
    for i in 0..steps {
        let t = i as f32 / (steps - 1) as f32;
        let off = -0.14 * r * t;
        painter.circle_filled(c + vec2(off, off), r * (1.0 - t * 0.6), mix(p.brand_b, p.brand_a, t));
    }
    let pts: Vec<egui::Pos2> = (0..=28)
        .map(|i| {
            let x = i as f32 / 28.0;
            let tt = (x - 0.42) * 5.5;
            let a = (1.0 - 2.0 * tt * tt) * (-tt * tt).exp();
            pos2(rect.left() + size * 0.18 + x * size * 0.64, c.y - a * size * 0.26)
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(size * 0.075, Color32::WHITE)));
}
