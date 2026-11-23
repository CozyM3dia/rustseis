//! RustSeis visual identity (interface v2): two calibrated palettes, the type stack and the egui
//! style contract. Panes read colours from [`Palette`], never from literals.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    Style, TextStyle, Visuals,
    epaint::text::VariationCoords,
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
    /// Window ground behind the floating panels.
    pub chrome: Color32,
    /// Floating panels (Explorer, documents, Inspector, command bar).
    pub panel: Color32,
    /// Cards and hovered rows inside panels.
    pub raised: Color32,
    /// Inputs, plot wells, code.
    pub sunken: Color32,
    /// Seismic canvas well.
    pub canvas: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    /// Interaction accent (selection, focus, primary action): sonar mint.
    pub accent: Color32,
    /// Solid tint of the accent for selected fills.
    pub accent_soft: Color32,
    pub accent_line: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    /// Picks, contextual labels, warnings.
    pub pick: Color32,
    pub pick_soft: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub info: Color32,
    pub series_b: Color32,
    pub series_c: Color32,
    pub series_d: Color32,
    pub grid: Color32,
    pub crosshair: Color32,
    /// Brand gradient ends (logo only).
    pub brand_a: Color32,
    pub brand_b: Color32,
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

/// "Midnight": near-black ground, graphite panels, sonar-mint accent.
pub const MIDNIGHT: Palette = Palette {
    mode: ThemeMode::Dark,
    chrome: rgb(11, 13, 16),
    panel: rgb(19, 23, 28),
    raised: rgb(26, 31, 37),
    sunken: rgb(15, 19, 23),
    canvas: rgb(13, 16, 20),
    border: rgb(30, 36, 43),
    border_strong: rgb(42, 49, 57),
    text: rgb(236, 239, 243),
    text_muted: rgb(154, 164, 176),
    text_faint: rgb(125, 135, 148),
    accent: rgb(60, 207, 191),
    accent_soft: rgb(22, 48, 44),
    accent_line: rgb(36, 86, 78),
    accent_text: rgb(159, 234, 223),
    on_accent: rgb(6, 34, 31),
    pick: rgb(255, 201, 74),
    pick_soft: rgb(42, 36, 20),
    danger: rgb(255, 107, 107),
    success: rgb(93, 211, 158),
    info: rgb(110, 168, 254),
    series_b: rgb(110, 168, 254),
    series_c: rgb(197, 138, 249),
    series_d: rgb(255, 154, 107),
    grid: rgba(150, 170, 195, 20),
    crosshair: rgba(236, 239, 243, 150),
    brand_a: rgb(255, 181, 71),
    brand_b: rgb(255, 107, 74),
};

/// "Daylight": white panels on a soft grey ground, deeper mint for contrast.
pub const DAYLIGHT: Palette = Palette {
    mode: ThemeMode::Light,
    chrome: rgb(233, 236, 239),
    panel: rgb(255, 255, 255),
    raised: rgb(246, 247, 249),
    sunken: rgb(241, 243, 245),
    canvas: rgb(247, 248, 250),
    border: rgb(226, 230, 234),
    border_strong: rgb(203, 210, 217),
    text: rgb(18, 22, 27),
    text_muted: rgb(79, 90, 102),
    text_faint: rgb(102, 112, 124),
    accent: rgb(13, 124, 114),
    accent_soft: rgb(224, 245, 241),
    accent_line: rgb(153, 218, 209),
    accent_text: rgb(10, 110, 101),
    on_accent: rgb(255, 255, 255),
    pick: rgb(183, 121, 31),
    pick_soft: rgb(252, 243, 224),
    danger: rgb(200, 58, 58),
    success: rgb(30, 138, 82),
    info: rgb(37, 99, 201),
    series_b: rgb(37, 99, 201),
    series_c: rgb(126, 72, 190),
    series_d: rgb(200, 96, 40),
    grid: rgba(40, 60, 80, 22),
    crosshair: rgba(18, 22, 27, 150),
    brand_a: rgb(255, 181, 71),
    brand_b: rgb(255, 107, 74),
};

pub fn palette(mode: ThemeMode) -> &'static Palette {
    match mode {
        ThemeMode::Dark => &MIDNIGHT,
        ThemeMode::Light => &DAYLIGHT,
    }
}

/// Spacing scale (px).
pub mod space {
    pub const XS: f32 = 2.0;
    pub const SM: f32 = 4.0;
    pub const MD: f32 = 8.0;
    pub const LG: f32 = 12.0;
    pub const XL: f32 = 16.0;
    /// Gap between floating panels and around the window edge.
    pub const GUTTER: f32 = 6.0;
}

pub const RADIUS_SM: u8 = 6;
pub const RADIUS_MD: u8 = 8;
pub const RADIUS_LG: u8 = 12;

pub fn semibold_family() -> FontFamily {
    FontFamily::Name("semibold".into())
}
/// Titles use the semibold face.
pub fn display_family() -> FontFamily {
    semibold_family()
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

pub fn font_semibold(size: f32) -> FontId {
    FontId::new(size, semibold_family())
}
pub fn font_display(size: f32) -> FontId {
    font_semibold(size)
}
pub fn font_mono(size: f32) -> FontId {
    FontId::monospace(size)
}
pub fn font_body(size: f32) -> FontId {
    FontId::proportional(size)
}

/// System faces we prefer, all optional; missing files fall back to egui's bundled fonts.
/// Segoe UI Variable has a weight axis, so one file gives both regular and semibold.
const BODY_VARIABLE: &[&str] = &["C:/Windows/Fonts/SegUIVar.ttf"];
const BODY_STATIC: &[&str] = &[
    "C:/Windows/Fonts/segoeui.ttf",
    "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
    "/System/Library/Fonts/SFNS.ttf",
];
const BODY_BOLD: &[&str] = &["C:/Windows/Fonts/segoeuib.ttf"];
const MONO_CANDIDATES: &[&str] = &[
    "C:/Windows/Fonts/CascadiaMono.ttf",
    "C:/Windows/Fonts/consola.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
];

fn first_existing(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

fn weighted(bytes: Vec<u8>, weight: f32) -> FontData {
    FontData::from_owned(bytes).tweak(egui::FontTweak {
        coords: VariationCoords::new([(b"wght", weight)]),
        ..Default::default()
    })
}

/// Body (regular), semibold and mono families, each with Phosphor icons as a fallback so icons
/// can sit inline with text.
pub fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "phosphor".into(),
        Arc::new(FontData::from_static(egui_phosphor::Variant::Regular.font_bytes())),
    );
    let mut semibold: Vec<String> = Vec::new();
    if let Some(var) = first_existing(BODY_VARIABLE) {
        fonts.font_data.insert("rs-body".into(), Arc::new(weighted(var.clone(), 400.0)));
        fonts.font_data.insert("rs-semibold".into(), Arc::new(weighted(var, 600.0)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "rs-body".into());
        semibold.push("rs-semibold".into());
    } else if let Some(stat) = first_existing(BODY_STATIC) {
        fonts.font_data.insert("rs-body".into(), Arc::new(FontData::from_owned(stat)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "rs-body".into());
        if let Some(bold) = first_existing(BODY_BOLD) {
            fonts.font_data.insert("rs-semibold".into(), Arc::new(FontData::from_owned(bold)));
            semibold.push("rs-semibold".into());
        }
    }
    if let Some(bytes) = first_existing(MONO_CANDIDATES) {
        fonts.font_data.insert("rs-mono".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "rs-mono".into());
    }
    semibold.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(semibold_family(), semibold);
    for fam in [FontFamily::Proportional, FontFamily::Monospace, semibold_family()] {
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
        (Button, FontId::proportional(12.5)),
        (Monospace, FontId::monospace(11.5)),
        (Heading, font_semibold(16.0)),
        (text_style_caption(), FontId::proportional(10.5)),
        (text_style_title(), font_semibold(13.5)),
        (text_style_num(), FontId::monospace(11.5)),
        (text_style_num_lg(), FontId::monospace(16.0)),
    ]
    .into()
}

pub fn style_for(p: &Palette) -> Style {
    let mut style = Style::default();
    style.text_styles = text_styles();
    style.animation_time = 0.12;
    style.explanation_tooltips = false;
    let sp = &mut style.spacing;
    sp.item_spacing = egui::vec2(6.0, 5.0);
    sp.button_padding = egui::vec2(10.0, 4.0);
    sp.interact_size = egui::vec2(36.0, 26.0);
    sp.window_margin = Margin::same(12);
    sp.menu_margin = Margin::same(6);
    sp.indent = 14.0;
    sp.slider_width = 130.0;
    sp.combo_width = 130.0;
    sp.text_edit_width = 170.0;
    sp.icon_width = 14.0;
    sp.icon_width_inner = 8.0;
    sp.icon_spacing = 6.0;
    sp.tooltip_width = 340.0;
    sp.menu_width = 230.0;
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
    let r = CornerRadius::same(RADIUS_SM);
    v.override_text_color = None;
    v.weak_text_color = Some(p.text_muted);
    v.hyperlink_color = p.accent_text;
    v.faint_bg_color = p.raised;
    v.extreme_bg_color = p.sunken;
    v.text_edit_bg_color = Some(p.sunken);
    v.code_bg_color = p.sunken;
    v.warn_fg_color = p.pick;
    v.error_fg_color = p.danger;
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.window_stroke = Stroke::new(1.0, p.border_strong);
    v.window_corner_radius = CornerRadius::same(RADIUS_LG);
    v.menu_corner_radius = CornerRadius::same(RADIUS_MD);
    let a = if p.mode == ThemeMode::Dark { 130 } else { 36 };
    v.window_shadow = Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(a) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(a) };
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.striped = false;
    v.slider_trailing_fill = true;
    v.button_frame = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    for (wv, fill, stroke) in [
        (&mut w.inactive, p.raised, p.border_strong),
        (&mut w.hovered, mix(p.raised, p.text, 0.06), mix(p.border_strong, p.accent, 0.45)),
        (&mut w.active, p.accent_soft, p.accent),
        (&mut w.open, p.accent_soft, p.accent_line),
    ] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = fill;
        wv.bg_stroke = Stroke::new(1.0, stroke);
        wv.fg_stroke = Stroke::new(1.0, p.text);
        wv.corner_radius = r;
        wv.expansion = 0.0;
    }
    v
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

pub fn with_alpha(c: Color32, alpha: u8) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

pub fn install(ctx: &egui::Context, mode: ThemeMode, with_fonts: bool) {
    if with_fonts {
        ctx.set_fonts(font_definitions());
    }
    apply(ctx, mode);
}

pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    let p = palette(mode);
    let theme = match mode {
        ThemeMode::Dark => egui::Theme::Dark,
        ThemeMode::Light => egui::Theme::Light,
    };
    ctx.set_style_of(theme, style_for(p));
    ctx.set_theme(theme);
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
        for p in [&MIDNIGHT, &DAYLIGHT] {
            for surface in [p.chrome, p.panel, p.raised, p.sunken] {
                assert!(contrast(p.text, surface) >= 7.0, "{:?} body text", p.mode);
                assert!(contrast(p.text_muted, surface) >= 4.5, "{:?} muted text", p.mode);
                assert!(contrast(p.text_faint, surface) >= 3.6, "{:?} faint text", p.mode);
            }
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "{:?} text on accent", p.mode);
            assert!(contrast(p.accent_text, p.accent_soft) >= 4.5, "{:?} accent text on tint", p.mode);
        }
    }
}
