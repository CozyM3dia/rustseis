//! RustSeis visual identity: two calibrated workstation palettes, the type stack, and the
//! egui / egui_dock style contract. Every pane reads colours from [`Palette`], never from literals.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    Style, TextStyle, Visuals,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

impl ThemeMode {
    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

/// Semantic colour tokens. Names describe the role, not the hue.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub mode: ThemeMode,
    /// Window chrome behind the dock (title strip, tab bar, status bar).
    pub chrome: Color32,
    /// Pane body.
    pub panel: Color32,
    /// Cards, step blocks, popovers.
    pub raised: Color32,
    /// Inputs, code, plot backgrounds.
    pub sunken: Color32,
    /// Surround of seismic rasters (always dark enough for colour-mapped data).
    pub canvas: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    /// Primary interaction colour (selection, focus, primary buttons).
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    /// Picks, interpretation overlays, warnings that are not errors.
    pub pick: Color32,
    pub pick_soft: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub info: Color32,
    /// Secondary series colour for plots (receivers, upgoing, model).
    pub series_b: Color32,
    pub series_c: Color32,
    pub grid: Color32,
    pub crosshair: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}
const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_premultiplied(
        ((r as u16 * a as u16) / 255) as u8,
        ((g as u16 * a as u16) / 255) as u8,
        ((b as u16 * a as u16) / 255) as u8,
        a,
    )
}

/// "Abyssal": deep ink-navy with sonar teal and core-sample amber.
pub const ABYSSAL: Palette = Palette {
    mode: ThemeMode::Dark,
    chrome: rgb(7, 10, 14),
    panel: rgb(12, 16, 22),
    raised: rgb(19, 25, 33),
    sunken: rgb(9, 12, 17),
    canvas: rgb(6, 8, 11),
    border: rgb(29, 38, 49),
    border_strong: rgb(45, 58, 73),
    text: rgb(220, 227, 234),
    text_muted: rgb(135, 150, 166),
    text_faint: rgb(86, 99, 112),
    accent: rgb(47, 200, 184),
    accent_soft: rgba(47, 200, 184, 34),
    on_accent: rgb(4, 20, 19),
    pick: rgb(244, 178, 62),
    pick_soft: rgba(244, 178, 62, 40),
    danger: rgb(242, 109, 109),
    success: rgb(87, 207, 140),
    info: rgb(108, 168, 255),
    series_b: rgb(108, 168, 255),
    series_c: rgb(222, 128, 214),
    grid: rgba(150, 170, 195, 22),
    crosshair: rgba(235, 242, 250, 170),
};

/// "Survey": cool drafting paper with deep teal ink.
pub const SURVEY: Palette = Palette {
    mode: ThemeMode::Light,
    chrome: rgb(226, 230, 235),
    panel: rgb(244, 246, 248),
    raised: rgb(255, 255, 255),
    sunken: rgb(236, 239, 243),
    canvas: rgb(26, 31, 38),
    border: rgb(211, 217, 224),
    border_strong: rgb(182, 192, 203),
    text: rgb(20, 27, 34),
    text_muted: rgb(84, 96, 110),
    text_faint: rgb(136, 147, 159),
    accent: rgb(14, 140, 129),
    accent_soft: rgba(14, 140, 129, 30),
    on_accent: rgb(255, 255, 255),
    pick: rgb(196, 120, 0),
    pick_soft: rgba(196, 120, 0, 36),
    danger: rgb(200, 58, 58),
    success: rgb(30, 138, 82),
    info: rgb(37, 99, 201),
    series_b: rgb(37, 99, 201),
    series_c: rgb(160, 60, 150),
    grid: rgba(40, 60, 80, 26),
    crosshair: rgba(255, 255, 255, 190),
};

pub fn palette(mode: ThemeMode) -> &'static Palette {
    match mode {
        ThemeMode::Dark => &ABYSSAL,
        ThemeMode::Light => &SURVEY,
    }
}

/// Spacing scale (px). Panes compose these instead of inventing numbers.
pub mod space {
    pub const XS: f32 = 2.0;
    pub const SM: f32 = 4.0;
    pub const MD: f32 = 8.0;
    pub const LG: f32 = 12.0;
    pub const XL: f32 = 16.0;
}

pub const RADIUS_SM: u8 = 3;
pub const RADIUS_MD: u8 = 5;
pub const RADIUS_LG: u8 = 7;

pub fn display_family() -> FontFamily {
    FontFamily::Name("display".into())
}

pub fn text_style_caption() -> TextStyle {
    TextStyle::Name("caption".into())
}
pub fn text_style_title() -> TextStyle {
    TextStyle::Name("title".into())
}
pub fn text_style_num() -> TextStyle {
    TextStyle::Name("num".into())
}
pub fn text_style_num_lg() -> TextStyle {
    TextStyle::Name("num_lg".into())
}

pub fn font_display(size: f32) -> FontId {
    FontId::new(size, display_family())
}
pub fn font_mono(size: f32) -> FontId {
    FontId::monospace(size)
}
pub fn font_body(size: f32) -> FontId {
    FontId::proportional(size)
}

/// System faces we prefer; all optional. Missing files fall back to egui's bundled fonts.
const BODY_CANDIDATES: &[&str] = &["C:/Windows/Fonts/segoeui.ttf", "/usr/share/fonts/truetype/inter/Inter-Regular.ttf"];
const DISPLAY_CANDIDATES: &[&str] = &["C:/Windows/Fonts/bahnschrift.ttf"];
const MONO_CANDIDATES: &[&str] = &[
    "C:/Windows/Fonts/CascadiaMono.ttf",
    "C:/Windows/Fonts/consola.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
];

fn first_existing(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

/// Builds the font stack: body (Segoe UI), display (Bahnschrift), mono (Cascadia Mono),
/// with Phosphor icons as a fallback in every family so icons can sit inline with text.
pub fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "phosphor".into(),
        Arc::new(FontData::from_static(egui_phosphor::Variant::Regular.font_bytes())),
    );

    let body = first_existing(BODY_CANDIDATES);
    let display = first_existing(DISPLAY_CANDIDATES);
    let mono = first_existing(MONO_CANDIDATES);

    if let Some(bytes) = body {
        fonts.font_data.insert("rs-body".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "rs-body".into());
    }
    if let Some(bytes) = mono {
        fonts.font_data.insert("rs-mono".into(), Arc::new(FontData::from_owned(bytes)));
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "rs-mono".into());
    }
    let mut display_stack: Vec<String> = Vec::new();
    if let Some(bytes) = display {
        fonts.font_data.insert("rs-display".into(), Arc::new(FontData::from_owned(bytes)));
        display_stack.push("rs-display".into());
    }
    display_stack.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(display_family(), display_stack);

    for fam in [FontFamily::Proportional, FontFamily::Monospace, display_family()] {
        let list = fonts.families.entry(fam).or_default();
        let at = list.len().min(1);
        list.insert(at, "phosphor".into());
    }
    fonts
}

fn text_styles() -> std::collections::BTreeMap<TextStyle, FontId> {
    use TextStyle::*;
    [
        (Small, FontId::proportional(11.0)),
        (Body, FontId::proportional(13.0)),
        (Button, FontId::proportional(13.0)),
        (Monospace, FontId::monospace(12.0)),
        (Heading, FontId::new(17.0, display_family())),
        (text_style_caption(), FontId::proportional(10.5)),
        (text_style_title(), FontId::new(14.0, display_family())),
        (text_style_num(), FontId::monospace(11.5)),
        (text_style_num_lg(), FontId::monospace(15.0)),
    ]
    .into()
}

/// Full egui [`Style`] for a palette: compact scientific density, crisp 1 px borders.
pub fn style_for(p: &Palette) -> Style {
    let mut style = Style::default();
    style.text_styles = text_styles();
    style.animation_time = 0.10;
    style.explanation_tooltips = false;

    let sp = &mut style.spacing;
    sp.item_spacing = egui::vec2(6.0, 4.0);
    sp.button_padding = egui::vec2(8.0, 3.0);
    sp.interact_size = egui::vec2(36.0, 22.0);
    sp.window_margin = Margin::same(10);
    sp.menu_margin = Margin::same(6);
    sp.indent = 14.0;
    sp.slider_width = 120.0;
    sp.combo_width = 120.0;
    sp.text_edit_width = 160.0;
    sp.icon_width = 13.0;
    sp.icon_width_inner = 7.0;
    sp.icon_spacing = 5.0;
    sp.tooltip_width = 360.0;
    sp.menu_width = 220.0;
    sp.scroll = egui::style::ScrollStyle::thin();
    sp.scroll.bar_width = 8.0;
    sp.scroll.floating_width = 2.0;

    style.interaction.tooltip_delay = 0.35;
    style.interaction.selectable_labels = false;

    style.visuals = visuals_for(p);
    style
}

pub fn visuals_for(p: &Palette) -> Visuals {
    let mut v = match p.mode {
        ThemeMode::Dark => Visuals::dark(),
        ThemeMode::Light => Visuals::light(),
    };
    let r_sm = CornerRadius::same(RADIUS_SM);
    let r_md = CornerRadius::same(RADIUS_MD);

    v.override_text_color = None;
    v.weak_text_color = Some(p.text_muted);
    v.hyperlink_color = p.accent;
    v.faint_bg_color = p.raised;
    v.extreme_bg_color = p.sunken;
    v.text_edit_bg_color = Some(p.sunken);
    v.code_bg_color = p.sunken;
    v.warn_fg_color = p.pick;
    v.error_fg_color = p.danger;
    v.panel_fill = p.panel;
    v.window_fill = p.raised;
    v.window_stroke = Stroke::new(1.0, p.border_strong);
    v.window_corner_radius = CornerRadius::same(RADIUS_LG);
    v.menu_corner_radius = r_md;
    let shadow_alpha = if p.mode == ThemeMode::Dark { 120 } else { 40 };
    v.window_shadow = Shadow { offset: [0, 6], blur: 22, spread: 0, color: Color32::from_black_alpha(shadow_alpha) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(shadow_alpha) };
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = true;
    v.striped = true;
    v.slider_trailing_fill = true;
    v.button_frame = true;
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.55 };

    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r_sm;

    w.inactive.bg_fill = p.raised;
    w.inactive.weak_bg_fill = p.raised;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r_sm;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = mix(p.raised, p.accent, 0.10);
    w.hovered.weak_bg_fill = mix(p.raised, p.accent, 0.10);
    w.hovered.bg_stroke = Stroke::new(1.0, mix(p.border_strong, p.accent, 0.55));
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r_sm;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = mix(p.raised, p.accent, 0.22);
    w.active.weak_bg_fill = mix(p.raised, p.accent, 0.22);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r_sm;
    w.active.expansion = 0.0;

    w.open.bg_fill = mix(p.raised, p.accent, 0.12);
    w.open.weak_bg_fill = mix(p.raised, p.accent, 0.12);
    w.open.bg_stroke = Stroke::new(1.0, p.accent);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r_sm;
    v
}

/// Linear blend in gamma space; good enough for UI tints.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn with_alpha(c: Color32, alpha: u8) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// Installs fonts once and the style for `mode`.
pub fn install(ctx: &egui::Context, mode: ThemeMode, with_fonts: bool) {
    if with_fonts {
        ctx.set_fonts(font_definitions());
    }
    apply(ctx, mode);
}

pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    let p = palette(mode);
    let style = style_for(p);
    let theme = match mode {
        ThemeMode::Dark => egui::Theme::Dark,
        ThemeMode::Light => egui::Theme::Light,
    };
    ctx.set_style_of(theme, style);
    ctx.set_theme(theme);
}

/// egui_dock style aligned with the palette: flat tab strip, hairline separators.
pub fn dock_style(egui_style: &Style, p: &Palette) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(egui_style);
    s.dock_area_padding = Some(Margin::ZERO);
    s.main_surface_border_stroke = Stroke::NONE;
    s.main_surface_border_rounding = CornerRadius::ZERO;

    s.separator.width = 1.0;
    s.separator.extra_interact_width = 4.0;
    s.separator.color_idle = p.border;
    s.separator.color_hovered = p.accent;
    s.separator.color_dragged = p.accent;

    s.tab_bar.bg_fill = p.chrome;
    s.tab_bar.height = 28.0;
    s.tab_bar.hline_color = p.border;
    s.tab_bar.corner_radius = CornerRadius::ZERO;
    s.tab_bar.fill_tab_bar = false;
    s.tab_bar.inner_margin = Margin::symmetric(4, 0);

    let tab_r = CornerRadius { nw: RADIUS_SM, ne: RADIUS_SM, sw: 0, se: 0 };
    s.tab.active.bg_fill = p.panel;
    s.tab.active.text_color = p.text;
    s.tab.active.outline_color = p.border;
    s.tab.active.corner_radius = tab_r;
    s.tab.focused.bg_fill = p.panel;
    s.tab.focused.text_color = p.accent;
    s.tab.focused.outline_color = p.border;
    s.tab.focused.corner_radius = tab_r;
    s.tab.inactive.bg_fill = Color32::TRANSPARENT;
    s.tab.inactive.text_color = p.text_muted;
    s.tab.inactive.outline_color = Color32::TRANSPARENT;
    s.tab.inactive.corner_radius = tab_r;
    s.tab.hovered.bg_fill = mix(p.chrome, p.panel, 0.6);
    s.tab.hovered.text_color = p.text;
    s.tab.hovered.outline_color = Color32::TRANSPARENT;
    s.tab.hovered.corner_radius = tab_r;
    s.tab.active_with_kb_focus = s.tab.active.clone();
    s.tab.inactive_with_kb_focus = s.tab.inactive.clone();
    s.tab.focused_with_kb_focus = s.tab.focused.clone();
    s.tab.tab_body.bg_fill = p.panel;
    s.tab.tab_body.stroke = Stroke::NONE;
    s.tab.tab_body.inner_margin = Margin::ZERO;
    s.tab.tab_body.corner_radius = CornerRadius::ZERO;
    s.tab.hline_below_active_tab_name = true;
    s.tab.spacing = 2.0;
    s.tab.minimum_width = Some(84.0);

    s.buttons.close_tab_color = p.text_faint;
    s.buttons.close_tab_active_color = p.text;
    s.buttons.close_tab_bg_fill = mix(p.raised, p.danger, 0.25);
    s.buttons.add_tab_color = p.text_faint;
    s.buttons.add_tab_active_color = p.accent;
    s.buttons.add_tab_bg_fill = p.raised;

    s.overlay.selection_color = with_alpha(p.accent, 60);
    s.overlay.button_color = p.raised;
    s.overlay.button_border_stroke = Stroke::new(1.0, p.border_strong);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f32 {
        let f = |v: u8| {
            let s = v as f32 / 255.0;
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn text_meets_contrast_on_every_surface() {
        for p in [&ABYSSAL, &SURVEY] {
            for surface in [p.chrome, p.panel, p.raised, p.sunken] {
                assert!(contrast(p.text, surface) >= 7.0, "{:?} body text contrast", p.mode);
                assert!(contrast(p.text_muted, surface) >= 4.5, "{:?} muted text contrast", p.mode);
            }
            assert!(contrast(p.accent, p.panel) >= 3.0, "{:?} accent contrast", p.mode);
        }
    }
}
