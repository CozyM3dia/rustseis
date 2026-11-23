//! Crisp multi-color vector iconography for the RustSeis ribbon, menus, library, and flow cards.
//!
//! Icons are painted directly via `egui::Painter` on a normalized `0..1` coordinate grid so they
//! stay razor-sharp at both `30×30` (primary ribbon buttons) and `16×16` (stacked compact rows,
//! dropdown menus, library items, and flow badges), adapting cleanly to Dark and Light themes.

use crate::theme::{Palette, ThemeMode, with_alpha};
use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    // Execution & workflow
    RunFlow,
    RunToHere,
    CancelJob,
    CheckFlow,
    FlowBuilder,
    Branch,
    History,
    Toolbox,
    CodeJson,
    NewFile,
    SaveFile,
    Undo,
    Redo,
    DoneCheck,
    // Data & geometry
    OpenData,
    SegdFolder,
    Database,
    MarineGeom,
    LandGeom,
    CrookedLine,
    HeaderMath,
    Sort,
    SurveyMap,
    // Signal & QC
    Gathers,
    Filter,
    Amplitude,
    Decon,
    Noise,
    Spectra,
    Wiggles,
    Compare,
    Export,
    EyePreview,
    LinkFollow,
    // Statics & picking
    FirstBreak,
    Mute,
    KillTrace,
    Statics,
    StaticsQc,
    // Velocity & depth
    VelocityPick,
    AutoPick,
    SnapPeak,
    Clear,
    VelocityField,
    DepthModel,
    // Imaging & reservoir
    Nmo,
    Stack,
    TimeMig,
    KirchhoffPsdm,
    RtmGpu,
    Avo,
    WellTie,
    // Borehole & AI
    VspSep,
    BoreholeView,
    Assistant,
}

#[derive(Clone, Copy)]
struct IconColors {
    ink: Color32,
    ink_soft: Color32,
    slate: Color32,
    green: Color32,
    blue: Color32,
    cyan: Color32,
    amber: Color32,
    red: Color32,
    purple: Color32,
}

impl IconColors {
    fn from_palette(p: &Palette) -> Self {
        match p.mode {
            ThemeMode::Dark => Self {
                ink: Color32::from_rgb(220, 228, 240),
                ink_soft: Color32::from_rgb(138, 152, 172),
                slate: Color32::from_rgb(46, 54, 68),
                green: Color32::from_rgb(46, 204, 113),
                blue: Color32::from_rgb(84, 166, 255),
                cyan: Color32::from_rgb(56, 212, 198),
                amber: Color32::from_rgb(248, 184, 62),
                red: Color32::from_rgb(239, 92, 92),
                purple: Color32::from_rgb(176, 134, 255),
            },
            ThemeMode::Light => Self {
                ink: Color32::from_rgb(38, 48, 62),
                ink_soft: Color32::from_rgb(112, 124, 140),
                slate: Color32::from_rgb(220, 227, 236),
                green: Color32::from_rgb(24, 152, 82),
                blue: Color32::from_rgb(35, 108, 212),
                cyan: Color32::from_rgb(12, 144, 132),
                amber: Color32::from_rgb(208, 132, 22),
                red: Color32::from_rgb(204, 56, 56),
                purple: Color32::from_rgb(120, 72, 206),
            },
        }
    }
}

/// Map any catalog tool `menu_id` + `category` to a domain-specific multi-color icon.
pub fn for_tool(menu_id: &str, category: &str) -> Icon {
    match menu_id {
        "segd_read" => Icon::SegdFolder,
        "segy_read" | "diskread2" | "synth" | "afd" => Icon::OpenData,
        "segy_write" | "diskwrite2" => Icon::Export,
        "geomspread_marine" => Icon::MarineGeom,
        "geomspread_land" => Icon::LandGeom,
        "geomspread_crooked" => Icon::CrookedLine,
        "thdrmath" | "hdrmerge" | "hdrload" => Icon::HeaderMath,
        "inline_sort" | "sort" => Icon::Sort,
        "fbpick" => Icon::FirstBreak,
        "mute" | "topmute" | "botmute" => Icon::Mute,
        "kill" | "tkill" | "trace_edit" => Icon::KillTrace,
        "refstatic" | "autostat" | "pwr_stat_new" | "trimstat" | "apply_statics" => Icon::Statics,
        "scamp2" | "agc" | "tar" | "gain" => Icon::Amplitude,
        "bpfilt" | "notch" | "fkfilt" | "radon" => Icon::Filter,
        "decon" | "spiking_decon" | "pred_decon" | "qcomp" => Icon::Decon,
        "fxdecon" | "median" | "svd" | "despike" => Icon::Noise,
        "nmo" => Icon::Nmo,
        "stack" => Icon::Stack,
        "crooked_stack" => Icon::CrookedLine,
        "vel_to_depth" => Icon::DepthModel,
        "eikonal_psdm" => Icon::KirchhoffPsdm,
        "rtm2d" => Icon::RtmGpu,
        "kirtime2d" | "fkmig2d" | "psmig2d" | "pstm2d" | "dmofkpanel" => Icon::TimeMig,
        "avo_analysis" => Icon::Avo,
        "well_tie" => Icon::WellTie,
        "vsp_wavefield_sep" => Icon::VspSep,
        "vsp_geom" | "vsp_slowness" | "vsp_decon" | "vsp_corridor_stack" => Icon::BoreholeView,
        _ => {
            if category.contains("Input") || category.contains("Output") {
                Icon::OpenData
            } else if category.contains("Geometry") {
                Icon::SurveyMap
            } else if category.contains("Editing") {
                Icon::Mute
            } else if category.contains("Amplitude") {
                Icon::Amplitude
            } else if category.contains("Filtering") {
                Icon::Filter
            } else if category.contains("Deconvolution") {
                Icon::Decon
            } else if category.contains("Enhancement") {
                Icon::Noise
            } else if category.contains("Statics") || category.contains("Refraction") {
                Icon::Statics
            } else if category.contains("Velocity") {
                Icon::VelocityPick
            } else if category.contains("Stacking") {
                Icon::Stack
            } else if category.contains("Migration") {
                Icon::KirchhoffPsdm
            } else if category.contains("Modeling") {
                Icon::RtmGpu
            } else if category.contains("AVO") {
                Icon::Avo
            } else if category.contains("Well") {
                Icon::WellTie
            } else if category.contains("VSP") {
                Icon::BoreholeView
            } else if category.contains("Flow Control") {
                Icon::Branch
            } else if category.contains("Plugin") || category.contains("Extensibility") {
                Icon::CodeJson
            } else {
                Icon::Toolbox
            }
        }
    }
}

/// Paint `icon` centered cleanly inside `rect`.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, p: &Palette) {
    let s = rect.width().min(rect.height());
    let r = Rect::from_center_size(rect.center(), vec2(s, s));
    let c = IconColors::from_palette(p);
    let pt = |x: f32, y: f32| pos2(r.left() + x * s, r.top() + y * s);
    let sw = (s / 18.0).clamp(1.1, 2.0);
    let thin = (s / 24.0).clamp(0.9, 1.5);
    let poly = |pts: &[(f32, f32)], fill: Color32, stroke: Stroke| {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| pt(x, y)).collect();
        painter.add(Shape::convex_polygon(v, fill, stroke));
    };
    let line = |pts: &[(f32, f32)], stroke: Stroke| {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| pt(x, y)).collect();
        painter.add(Shape::line(v, stroke));
    };
    let box_r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pt(x0, y0), pt(x1, y1));

    match icon {
        Icon::RunFlow => {
            poly(&[(0.22, 0.14), (0.86, 0.50), (0.22, 0.86)], c.green, Stroke::new(thin, with_alpha(c.ink, 180)));
        }
        Icon::RunToHere => {
            poly(&[(0.14, 0.18), (0.64, 0.50), (0.14, 0.82)], c.green, Stroke::new(thin, with_alpha(c.ink, 160)));
            let bar = box_r(0.73, 0.18, 0.86, 0.82);
            painter.rect(bar, CornerRadius::same(2), c.cyan, Stroke::new(thin, c.ink), StrokeKind::Inside);
        }
        Icon::CancelJob => {
            painter.circle_stroke(pt(0.5, 0.5), s * 0.40, Stroke::new(sw, c.red));
            let sq = box_r(0.32, 0.32, 0.68, 0.68);
            painter.rect_filled(sq, CornerRadius::same(2), c.red);
        }
        Icon::CheckFlow => {
            painter.circle_filled(pt(0.5, 0.5), s * 0.39, with_alpha(c.green, 55));
            painter.circle_stroke(pt(0.5, 0.5), s * 0.39, Stroke::new(sw, c.green));
            line(&[(0.31, 0.52), (0.45, 0.66), (0.71, 0.36)], Stroke::new(sw * 1.15, c.green));
        }
        Icon::DoneCheck => {
            painter.circle_filled(pt(0.5, 0.5), s * 0.41, c.green);
            line(&[(0.29, 0.52), (0.44, 0.67), (0.73, 0.35)], Stroke::new(sw * 1.25, Color32::WHITE));
        }
        Icon::FlowBuilder => {
            // ArcGIS ModelBuilder-inspired connected multi-color nodes
            line(&[(0.26, 0.24), (0.50, 0.49), (0.74, 0.74)], Stroke::new(sw, c.ink_soft));
            painter.circle_filled(pt(0.24, 0.24), s * 0.15, c.blue);
            painter.circle_stroke(pt(0.24, 0.24), s * 0.15, Stroke::new(thin, c.ink));
            let mid = box_r(0.38, 0.38, 0.62, 0.62);
            painter.rect(mid, CornerRadius::same(3), c.amber, Stroke::new(thin, c.ink), StrokeKind::Inside);
            painter.circle_filled(pt(0.76, 0.76), s * 0.16, c.green);
            painter.circle_stroke(pt(0.76, 0.76), s * 0.16, Stroke::new(thin, c.ink));
        }
        Icon::Branch => {
            line(&[(0.24, 0.22), (0.24, 0.78)], Stroke::new(sw, c.ink_soft));
            line(&[(0.24, 0.46), (0.48, 0.46), (0.72, 0.26)], Stroke::new(sw, c.purple));
            line(&[(0.48, 0.46), (0.72, 0.74)], Stroke::new(sw, c.cyan));
            painter.circle_filled(pt(0.24, 0.22), s * 0.11, c.amber);
            painter.circle_filled(pt(0.74, 0.26), s * 0.12, c.purple);
            painter.circle_filled(pt(0.74, 0.74), s * 0.12, c.cyan);
            painter.circle_stroke(pt(0.24, 0.22), s * 0.11, Stroke::new(thin, c.ink));
            painter.circle_stroke(pt(0.74, 0.26), s * 0.12, Stroke::new(thin, c.ink));
            painter.circle_stroke(pt(0.74, 0.74), s * 0.12, Stroke::new(thin, c.ink));
        }
        Icon::History => {
            // Circular clock rewind arrow (ArcGIS History style)
            let n = 18;
            let arc: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let a = -2.35 + (i as f32 / n as f32) * 5.05;
                    pos2(r.center().x + a.cos() * s * 0.36, r.center().y + a.sin() * s * 0.36)
                })
                .collect();
            painter.add(Shape::line(arc, Stroke::new(sw, c.ink)));
            poly(&[(0.12, 0.34), (0.31, 0.28), (0.22, 0.48)], c.blue, Stroke::NONE);
            line(&[(0.50, 0.30), (0.50, 0.52), (0.66, 0.62)], Stroke::new(sw, c.cyan));
        }
        Icon::Toolbox => {
            // Crisp red/coral toolbox (ArcGIS Tools style)
            let handle = box_r(0.35, 0.18, 0.65, 0.34);
            painter.rect_stroke(handle, CornerRadius::same(2), Stroke::new(sw, c.ink), StrokeKind::Inside);
            let body = box_r(0.12, 0.32, 0.88, 0.84);
            painter.rect(body, CornerRadius::same(3), c.red, Stroke::new(thin, c.ink), StrokeKind::Inside);
            line(&[(0.14, 0.54), (0.86, 0.54)], Stroke::new(thin, c.ink));
            let latch = box_r(0.43, 0.47, 0.57, 0.62);
            painter.rect(latch, CornerRadius::same(1), c.amber, Stroke::new(thin, c.ink), StrokeKind::Inside);
        }
        Icon::CodeJson => {
            let box_rect = box_r(0.10, 0.18, 0.90, 0.82);
            painter.rect(box_rect, CornerRadius::same(3), c.slate, Stroke::new(sw, c.ink), StrokeKind::Inside);
            line(&[(0.26, 0.38), (0.44, 0.51), (0.26, 0.64)], Stroke::new(sw, c.green));
            line(&[(0.52, 0.64), (0.74, 0.64)], Stroke::new(sw, c.cyan));
        }
        Icon::NewFile => {
            let doc = box_r(0.18, 0.12, 0.74, 0.86);
            painter.rect(doc, CornerRadius::same(2), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            line(&[(0.28, 0.34), (0.62, 0.34)], Stroke::new(thin, c.ink_soft));
            line(&[(0.28, 0.50), (0.56, 0.50)], Stroke::new(thin, c.ink_soft));
            painter.circle_filled(pt(0.74, 0.72), s * 0.18, c.green);
            line(&[(0.65, 0.72), (0.83, 0.72)], Stroke::new(sw, Color32::WHITE));
            line(&[(0.74, 0.63), (0.74, 0.81)], Stroke::new(sw, Color32::WHITE));
        }
        Icon::SaveFile => {
            let body = box_r(0.14, 0.14, 0.86, 0.86);
            painter.rect(body, CornerRadius::same(3), c.blue, Stroke::new(thin, c.ink), StrokeKind::Inside);
            let shutter = box_r(0.28, 0.14, 0.70, 0.42);
            painter.rect(shutter, CornerRadius::same(1), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            let label = box_r(0.26, 0.54, 0.74, 0.86);
            painter.rect(label, CornerRadius::same(1), c.amber, Stroke::new(thin, c.ink), StrokeKind::Inside);
        }
        Icon::Undo => {
            line(&[(0.26, 0.42), (0.62, 0.42), (0.78, 0.52), (0.78, 0.70), (0.62, 0.78), (0.34, 0.78)], Stroke::new(sw, c.blue));
            poly(&[(0.14, 0.42), (0.34, 0.24), (0.34, 0.60)], c.blue, Stroke::NONE);
        }
        Icon::Redo => {
            line(&[(0.74, 0.42), (0.38, 0.42), (0.22, 0.52), (0.22, 0.70), (0.38, 0.78), (0.66, 0.78)], Stroke::new(sw, c.cyan));
            poly(&[(0.86, 0.42), (0.66, 0.24), (0.66, 0.60)], c.cyan, Stroke::NONE);
        }
        Icon::OpenData => {
            let tab = box_r(0.12, 0.18, 0.46, 0.36);
            painter.rect(tab, CornerRadius::same(2), c.amber, Stroke::new(thin, c.ink), StrokeKind::Inside);
            let back = box_r(0.12, 0.28, 0.88, 0.82);
            painter.rect(back, CornerRadius::same(3), with_alpha(c.amber, 200), Stroke::new(thin, c.ink), StrokeKind::Inside);
            let card = box_r(0.22, 0.36, 0.78, 0.74);
            painter.rect_filled(card, CornerRadius::same(2), c.slate);
            line(&[(0.26, 0.55), (0.36, 0.55), (0.44, 0.42), (0.52, 0.68), (0.60, 0.48), (0.72, 0.55)], Stroke::new(sw, c.cyan));
        }
        Icon::SegdFolder => {
            let b1 = box_r(0.12, 0.26, 0.76, 0.84);
            painter.rect(b1, CornerRadius::same(2), c.blue, Stroke::new(thin, c.ink), StrokeKind::Inside);
            let b2 = box_r(0.24, 0.16, 0.88, 0.74);
            painter.rect(b2, CornerRadius::same(2), c.amber, Stroke::new(thin, c.ink), StrokeKind::Inside);
            line(&[(0.34, 0.44), (0.46, 0.32), (0.56, 0.58), (0.68, 0.40), (0.78, 0.46)], Stroke::new(thin * 1.1, c.slate));
        }
        Icon::Database => {
            let body = box_r(0.18, 0.22, 0.82, 0.78);
            painter.rect(body, CornerRadius::same(4), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            let top = box_r(0.18, 0.16, 0.82, 0.34);
            painter.rect(top, CornerRadius::same(6), c.blue, Stroke::new(thin, c.ink), StrokeKind::Inside);
            line(&[(0.20, 0.54), (0.80, 0.54)], Stroke::new(thin, c.cyan));
            line(&[(0.20, 0.70), (0.80, 0.70)], Stroke::new(thin, c.ink_soft));
        }
        Icon::MarineGeom => {
            // Boat hull + ocean waves + hydrophone streamer
            poly(&[(0.16, 0.36), (0.48, 0.36), (0.42, 0.48), (0.20, 0.48)], c.amber, Stroke::new(thin, c.ink));
            line(&[(0.12, 0.58), (0.28, 0.52), (0.44, 0.60), (0.62, 0.52), (0.86, 0.58)], Stroke::new(sw, c.blue));
            line(&[(0.40, 0.48), (0.56, 0.72), (0.86, 0.72)], Stroke::new(thin, c.cyan));
            for x in [0.58, 0.71, 0.84] {
                painter.circle_filled(pt(x, 0.72), s * 0.06, c.cyan);
            }
        }
        Icon::LandGeom => {
            // Terrain slope + planted red/amber geophone pins
            poly(&[(0.10, 0.78), (0.38, 0.46), (0.64, 0.60), (0.90, 0.36), (0.90, 0.84), (0.10, 0.84)], with_alpha(c.green, 110), Stroke::NONE);
            line(&[(0.10, 0.76), (0.38, 0.46), (0.64, 0.60), (0.90, 0.36)], Stroke::new(sw, c.green));
            for (x, y) in [(0.38, 0.46), (0.64, 0.60), (0.82, 0.43)] {
                line(&[(x, y), (x, y - 0.18)], Stroke::new(thin, c.ink));
                painter.circle_filled(pt(x, y - 0.20), s * 0.065, c.amber);
            }
        }
        Icon::CrookedLine => {
            // S-curve crooked traverse + corridor bins
            line(&[(0.14, 0.76), (0.36, 0.30), (0.62, 0.68), (0.86, 0.22)], Stroke::new(sw * 1.1, c.amber));
            for (x, y) in [(0.24, 0.54), (0.48, 0.48), (0.74, 0.45)] {
                line(&[(x - 0.09, y - 0.06), (x + 0.09, y + 0.06)], Stroke::new(thin, c.cyan));
                painter.circle_filled(pt(x, y), s * 0.055, c.ink);
            }
        }
        Icon::HeaderMath => {
            let box_rect = box_r(0.14, 0.14, 0.86, 0.86);
            painter.rect(box_rect, CornerRadius::same(3), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            line(&[(0.50, 0.16), (0.50, 0.84)], Stroke::new(thin, c.ink_soft));
            line(&[(0.16, 0.50), (0.84, 0.50)], Stroke::new(thin, c.ink_soft));
            line(&[(0.24, 0.33), (0.40, 0.33)], Stroke::new(sw, c.blue));
            line(&[(0.32, 0.25), (0.32, 0.41)], Stroke::new(sw, c.blue));
            line(&[(0.58, 0.64), (0.76, 0.64)], Stroke::new(sw, c.amber));
            line(&[(0.58, 0.74), (0.76, 0.74)], Stroke::new(sw, c.amber));
        }
        Icon::Sort => {
            line(&[(0.32, 0.20), (0.32, 0.80)], Stroke::new(sw, c.cyan));
            poly(&[(0.32, 0.14), (0.18, 0.32), (0.46, 0.32)], c.cyan, Stroke::NONE);
            line(&[(0.68, 0.20), (0.68, 0.80)], Stroke::new(sw, c.amber));
            poly(&[(0.68, 0.86), (0.54, 0.68), (0.82, 0.68)], c.amber, Stroke::NONE);
        }
        Icon::SurveyMap => {
            let bg = box_r(0.12, 0.18, 0.88, 0.84);
            painter.rect(bg, CornerRadius::same(3), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            line(&[(0.38, 0.20), (0.38, 0.82)], Stroke::new(thin, c.ink_soft));
            line(&[(0.64, 0.20), (0.64, 0.82)], Stroke::new(thin, c.ink_soft));
            line(&[(0.20, 0.72), (0.46, 0.52), (0.76, 0.34)], Stroke::new(sw, c.cyan));
            painter.circle_filled(pt(0.72, 0.34), s * 0.10, c.red);
            painter.circle_filled(pt(0.72, 0.34), s * 0.04, Color32::WHITE);
        }
        Icon::Gathers => {
            let bg = box_r(0.12, 0.14, 0.88, 0.86);
            painter.rect(bg, CornerRadius::same(3), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            for (i, x) in [0.28, 0.50, 0.72].iter().enumerate() {
                let shift = (i as f32) * 0.08;
                line(&[(*x, 0.20), (*x, 0.80)], Stroke::new(thin * 0.8, c.ink_soft));
                line(
                    &[
                        (*x, 0.24 + shift),
                        (*x + 0.09, 0.34 + shift),
                        (*x - 0.08, 0.46 + shift),
                        (*x + 0.07, 0.58 + shift),
                        (*x, 0.68 + shift),
                    ],
                    Stroke::new(sw, if i == 1 { c.amber } else { c.cyan }),
                );
            }
        }
        Icon::Filter => {
            // ArcGIS cobalt funnel icon
            poly(&[(0.14, 0.18), (0.86, 0.18), (0.58, 0.52), (0.42, 0.52)], c.blue, Stroke::new(thin, c.ink));
            let stem = box_r(0.42, 0.50, 0.58, 0.84);
            painter.rect(stem, CornerRadius::same(1), c.cyan, Stroke::new(thin, c.ink), StrokeKind::Inside);
        }
        Icon::Amplitude => {
            for (x, h, col) in [(0.16, 0.24, c.blue), (0.36, 0.42, c.cyan), (0.56, 0.58, c.green), (0.76, 0.74, c.amber)] {
                let bar = box_r(x, 0.86 - h, x + 0.12, 0.86);
                painter.rect(bar, CornerRadius::same(1), col, Stroke::new(thin * 0.8, c.ink), StrokeKind::Inside);
            }
            line(&[(0.16, 0.58), (0.42, 0.40), (0.84, 0.14)], Stroke::new(thin, c.ink));
        }
        Icon::Decon => {
            // Broad muted pulse -> sharp glowing violet/amber spike
            line(&[(0.10, 0.72), (0.24, 0.56), (0.36, 0.72)], Stroke::new(sw, c.ink_soft));
            line(&[(0.40, 0.54), (0.52, 0.54)], Stroke::new(thin, c.ink_soft));
            line(&[(0.54, 0.72), (0.68, 0.72), (0.72, 0.16), (0.76, 0.72), (0.90, 0.72)], Stroke::new(sw * 1.1, c.purple));
            painter.circle_filled(pt(0.72, 0.18), s * 0.07, c.amber);
        }
        Icon::Noise => {
            line(&[(0.12, 0.34), (0.28, 0.22), (0.44, 0.44), (0.62, 0.24), (0.86, 0.36)], Stroke::new(sw, c.cyan));
            line(&[(0.14, 0.72), (0.24, 0.56), (0.34, 0.82), (0.46, 0.54), (0.58, 0.80), (0.72, 0.60), (0.86, 0.74)], Stroke::new(thin, c.red));
            line(&[(0.18, 0.82), (0.82, 0.54)], Stroke::new(sw, c.amber));
        }
        Icon::Spectra => {
            let bg = box_r(0.12, 0.16, 0.88, 0.84);
            painter.rect(bg, CornerRadius::same(3), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            poly(&[(0.18, 0.78), (0.38, 0.30), (0.62, 0.44), (0.82, 0.78)], with_alpha(c.cyan, 110), Stroke::NONE);
            line(&[(0.18, 0.78), (0.34, 0.34), (0.50, 0.48), (0.64, 0.28), (0.82, 0.78)], Stroke::new(sw, c.amber));
        }
        Icon::Wiggles => {
            line(&[(0.50, 0.12), (0.50, 0.88)], Stroke::new(thin, c.ink_soft));
            line(&[(0.50, 0.14), (0.76, 0.28), (0.26, 0.44), (0.80, 0.60), (0.30, 0.76), (0.50, 0.86)], Stroke::new(sw * 1.1, c.cyan));
        }
        Icon::Compare => {
            let l = box_r(0.10, 0.16, 0.46, 0.84);
            let rr = box_r(0.54, 0.16, 0.90, 0.84);
            painter.rect(l, CornerRadius::same(2), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            painter.rect(rr, CornerRadius::same(2), with_alpha(c.blue, 60), Stroke::new(thin, c.cyan), StrokeKind::Inside);
            line(&[(0.20, 0.42), (0.38, 0.36)], Stroke::new(thin, c.ink_soft));
            line(&[(0.20, 0.62), (0.38, 0.58)], Stroke::new(thin, c.ink_soft));
            line(&[(0.62, 0.42), (0.82, 0.42)], Stroke::new(sw, c.cyan));
            line(&[(0.62, 0.62), (0.82, 0.62)], Stroke::new(sw, c.amber));
            line(&[(0.50, 0.10), (0.50, 0.90)], Stroke::new(sw, c.amber));
        }
        Icon::Export => {
            line(&[(0.16, 0.56), (0.16, 0.84), (0.84, 0.84), (0.84, 0.56)], Stroke::new(sw, c.ink));
            line(&[(0.50, 0.66), (0.50, 0.18)], Stroke::new(sw * 1.1, c.blue));
            poly(&[(0.50, 0.12), (0.32, 0.34), (0.68, 0.34)], c.blue, Stroke::NONE);
        }
        Icon::EyePreview => {
            line(&[(0.12, 0.50), (0.32, 0.28), (0.68, 0.28), (0.88, 0.50), (0.68, 0.72), (0.32, 0.72), (0.12, 0.50)], Stroke::new(sw, c.ink));
            painter.circle_filled(pt(0.50, 0.50), s * 0.15, c.cyan);
            painter.circle_filled(pt(0.50, 0.50), s * 0.06, c.slate);
        }
        Icon::LinkFollow => {
            let l = box_r(0.12, 0.34, 0.54, 0.66);
            let rr = box_r(0.46, 0.34, 0.88, 0.66);
            painter.rect_stroke(l, CornerRadius::same(6), Stroke::new(sw, c.blue), StrokeKind::Inside);
            painter.rect_stroke(rr, CornerRadius::same(6), Stroke::new(sw, c.cyan), StrokeKind::Inside);
        }
        Icon::FirstBreak => {
            // Vivid amber lightning bolt
            poly(&[(0.56, 0.10), (0.22, 0.54), (0.48, 0.54), (0.40, 0.90), (0.78, 0.44), (0.52, 0.44)], c.amber, Stroke::new(thin, c.ink));
        }
        Icon::Mute => {
            // Shaded mute wedge + crisp scissor blades
            poly(&[(0.12, 0.16), (0.88, 0.16), (0.88, 0.46), (0.12, 0.28)], with_alpha(c.amber, 85), Stroke::NONE);
            line(&[(0.12, 0.28), (0.88, 0.46)], Stroke::new(sw, c.amber));
            line(&[(0.28, 0.76), (0.72, 0.44)], Stroke::new(sw, c.ink));
            line(&[(0.28, 0.44), (0.72, 0.76)], Stroke::new(sw, c.ink));
            painter.circle_stroke(pt(0.24, 0.78), s * 0.09, Stroke::new(thin, c.cyan));
            painter.circle_stroke(pt(0.24, 0.44), s * 0.09, Stroke::new(thin, c.cyan));
        }
        Icon::KillTrace => {
            line(&[(0.50, 0.16), (0.50, 0.84)], Stroke::new(thin, c.ink_soft));
            painter.circle_stroke(pt(0.50, 0.50), s * 0.34, Stroke::new(sw, c.red));
            line(&[(0.26, 0.26), (0.74, 0.74)], Stroke::new(sw, c.red));
        }
        Icon::Statics => {
            // Datum line + vertical shift lollipops
            line(&[(0.10, 0.50), (0.90, 0.50)], Stroke::new(thin, c.ink_soft));
            for (x, y, col) in [(0.22, 0.24, c.cyan), (0.42, 0.74, c.purple), (0.62, 0.28, c.green), (0.80, 0.68, c.amber)] {
                line(&[(x, 0.50), (x, y)], Stroke::new(sw, col));
                painter.circle_filled(pt(x, y), s * 0.08, col);
            }
        }
        Icon::StaticsQc => {
            let bg = box_r(0.12, 0.14, 0.88, 0.86);
            painter.rect(bg, CornerRadius::same(3), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            line(&[(0.18, 0.52), (0.82, 0.52)], Stroke::new(thin, c.green));
            for (x, top, bot, col) in [(0.24, 0.34, 0.52, c.cyan), (0.42, 0.52, 0.74, c.amber), (0.60, 0.26, 0.52, c.purple), (0.74, 0.52, 0.68, c.red)] {
                painter.rect_filled(box_r(x, top, x + 0.10, bot), CornerRadius::same(1), col);
            }
        }
        Icon::VelocityPick => {
            // Semblance contours + yellow pick trend
            let bg = box_r(0.12, 0.14, 0.88, 0.86);
            painter.rect(bg, CornerRadius::same(3), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            painter.circle_filled(pt(0.36, 0.34), s * 0.13, with_alpha(c.blue, 170));
            painter.circle_filled(pt(0.52, 0.54), s * 0.14, with_alpha(c.cyan, 190));
            painter.circle_filled(pt(0.68, 0.72), s * 0.13, with_alpha(c.red, 190));
            line(&[(0.28, 0.22), (0.38, 0.36), (0.54, 0.54), (0.72, 0.76)], Stroke::new(sw, c.amber));
        }
        Icon::AutoPick => {
            // Sparkle wand
            line(&[(0.18, 0.82), (0.64, 0.36)], Stroke::new(sw * 1.2, c.ink));
            // 4-point star at tip
            let star = |cx: f32, cy: f32, rad: f32, col: Color32| {
                poly(&[(cx, cy - rad), (cx + rad * 0.32, cy - rad * 0.32), (cx + rad, cy), (cx + rad * 0.32, cy + rad * 0.32), (cx, cy + rad), (cx - rad * 0.32, cy + rad * 0.32), (cx - rad, cy), (cx - rad * 0.32, cy - rad * 0.32)], col, Stroke::NONE);
            };
            star(0.70, 0.28, 0.20, c.amber);
            star(0.32, 0.28, 0.10, c.cyan);
        }
        Icon::SnapPeak => {
            // Horseshoe magnet
            line(&[(0.24, 0.24), (0.24, 0.62), (0.34, 0.78), (0.66, 0.78), (0.76, 0.62), (0.76, 0.24)], Stroke::new(sw * 1.6, c.ink_soft));
            painter.rect_filled(box_r(0.16, 0.16, 0.32, 0.34), CornerRadius::same(1), c.red);
            painter.rect_filled(box_r(0.68, 0.16, 0.84, 0.34), CornerRadius::same(1), c.red);
            painter.circle_filled(pt(0.50, 0.22), s * 0.08, c.amber);
        }
        Icon::Clear => {
            poly(&[(0.44, 0.20), (0.82, 0.56), (0.56, 0.82), (0.18, 0.46)], c.amber, Stroke::new(thin, c.ink));
            poly(&[(0.18, 0.46), (0.44, 0.72), (0.34, 0.82), (0.14, 0.62)], c.red, Stroke::new(thin, c.ink));
            line(&[(0.46, 0.84), (0.86, 0.84)], Stroke::new(sw, c.ink_soft));
        }
        Icon::VelocityField => {
            let bg = box_r(0.12, 0.16, 0.88, 0.84);
            painter.rect_filled(box_r(0.12, 0.16, 0.88, 0.38), CornerRadius::same(2), c.blue);
            painter.rect_filled(box_r(0.12, 0.38, 0.88, 0.62), CornerRadius::same(0), c.cyan);
            painter.rect_filled(box_r(0.12, 0.62, 0.88, 0.84), CornerRadius::same(2), c.amber);
            painter.rect_stroke(bg, CornerRadius::same(3), Stroke::new(thin, c.ink), StrokeKind::Inside);
            line(&[(0.14, 0.42), (0.50, 0.36), (0.86, 0.44)], Stroke::new(thin, c.ink));
            line(&[(0.14, 0.64), (0.52, 0.58), (0.86, 0.66)], Stroke::new(thin, c.ink));
        }
        Icon::DepthModel => {
            // Layered 2D V(x,z) velocity grid with horizons
            let bg = box_r(0.12, 0.14, 0.88, 0.86);
            painter.rect_filled(bg, CornerRadius::same(3), with_alpha(c.blue, 140));
            poly(&[(0.12, 0.48), (0.46, 0.36), (0.88, 0.48), (0.88, 0.86), (0.12, 0.86)], with_alpha(c.cyan, 160), Stroke::NONE);
            poly(&[(0.12, 0.68), (0.50, 0.58), (0.88, 0.70), (0.88, 0.86), (0.12, 0.86)], with_alpha(c.amber, 190), Stroke::NONE);
            line(&[(0.14, 0.48), (0.46, 0.36), (0.86, 0.48)], Stroke::new(sw, c.ink));
            line(&[(0.14, 0.68), (0.50, 0.58), (0.86, 0.70)], Stroke::new(sw, c.ink));
            painter.rect_stroke(bg, CornerRadius::same(3), Stroke::new(thin, c.ink), StrokeKind::Inside);
        }
        Icon::Nmo => {
            // Curved hyperbola flattened to horizontal line
            line(&[(0.14, 0.74), (0.34, 0.52), (0.50, 0.46), (0.66, 0.52), (0.86, 0.74)], Stroke::new(thin, c.ink_soft));
            line(&[(0.50, 0.46), (0.50, 0.28)], Stroke::new(thin, c.amber));
            line(&[(0.14, 0.28), (0.86, 0.28)], Stroke::new(sw * 1.2, c.cyan));
        }
        Icon::Stack => {
            line(&[(0.20, 0.16), (0.44, 0.48)], Stroke::new(thin, c.cyan));
            line(&[(0.50, 0.16), (0.50, 0.48)], Stroke::new(thin, c.blue));
            line(&[(0.80, 0.16), (0.56, 0.48)], Stroke::new(thin, c.cyan));
            line(&[(0.50, 0.48), (0.50, 0.84)], Stroke::new(sw * 1.3, c.green));
            poly(&[(0.50, 0.88), (0.34, 0.68), (0.66, 0.68)], c.green, Stroke::NONE);
        }
        Icon::TimeMig => {
            // Bow-tie collapsed to crisp focus point
            line(&[(0.14, 0.74), (0.50, 0.36), (0.86, 0.74)], Stroke::new(thin, c.ink_soft));
            line(&[(0.22, 0.66), (0.50, 0.46), (0.78, 0.66)], Stroke::new(thin, c.cyan));
            painter.circle_filled(pt(0.50, 0.34), s * 0.12, c.amber);
            painter.circle_stroke(pt(0.50, 0.34), s * 0.12, Stroke::new(thin, c.ink));
        }
        Icon::KirchhoffPsdm => {
            // Inspired by ArcGIS Raster Analysis: crisp geological strata peaks + glowing amber source + focused depth rays
            poly(&[(0.12, 0.82), (0.42, 0.24), (0.68, 0.82)], c.ink, Stroke::NONE);
            poly(&[(0.46, 0.82), (0.66, 0.42), (0.88, 0.82)], c.ink_soft, Stroke::NONE);
            poly(&[(0.18, 0.82), (0.42, 0.36), (0.62, 0.82)], c.blue, Stroke::NONE);
            line(&[(0.14, 0.82), (0.86, 0.82)], Stroke::new(sw, c.cyan));
            painter.circle_filled(pt(0.76, 0.24), s * 0.11, c.amber);
        }
        Icon::RtmGpu => {
            // Concentric two-way wavefield arcs + glowing GPU core
            painter.circle_filled(pt(0.50, 0.50), s * 0.13, c.amber);
            for (rad, col) in [(0.24, c.cyan), (0.36, c.blue), (0.44, c.green)] {
                painter.circle_stroke(pt(0.50, 0.50), s * rad, Stroke::new(thin * 1.1, col));
            }
            let chip = box_r(0.64, 0.64, 0.92, 0.92);
            painter.rect(chip, CornerRadius::same(2), c.green, Stroke::new(thin, c.ink), StrokeKind::Inside);
        }
        Icon::Avo => {
            // R0 vs G crossplot with Class III anomaly
            line(&[(0.14, 0.50), (0.86, 0.50)], Stroke::new(thin, c.ink_soft));
            line(&[(0.50, 0.14), (0.50, 0.86)], Stroke::new(thin, c.ink_soft));
            line(&[(0.20, 0.22), (0.80, 0.78)], Stroke::new(sw, c.blue));
            painter.circle_filled(pt(0.30, 0.70), s * 0.11, c.amber);
            painter.circle_stroke(pt(0.30, 0.70), s * 0.11, Stroke::new(thin, c.red));
        }
        Icon::WellTie => {
            // Vertical borehole + sonic log + synthetic seismogram
            line(&[(0.26, 0.14), (0.26, 0.86)], Stroke::new(sw * 1.2, c.purple));
            line(&[(0.44, 0.16), (0.56, 0.28), (0.40, 0.44), (0.60, 0.60), (0.46, 0.82)], Stroke::new(sw, c.amber));
            line(&[(0.74, 0.16), (0.86, 0.30), (0.66, 0.46), (0.86, 0.62), (0.74, 0.82)], Stroke::new(sw, c.cyan));
        }
        Icon::VspSep => {
            // Downgoing blue vs upgoing green diagonal waves
            line(&[(0.16, 0.18), (0.82, 0.78)], Stroke::new(sw, c.blue));
            line(&[(0.16, 0.42), (0.62, 0.82)], Stroke::new(thin, c.blue));
            line(&[(0.20, 0.80), (0.84, 0.22)], Stroke::new(sw * 1.1, c.green));
            poly(&[(0.86, 0.20), (0.68, 0.22), (0.82, 0.38)], c.green, Stroke::NONE);
        }
        Icon::BoreholeView => {
            let well = box_r(0.20, 0.14, 0.38, 0.86);
            painter.rect(well, CornerRadius::same(2), c.slate, Stroke::new(thin, c.ink), StrokeKind::Inside);
            for y in [0.30, 0.50, 0.70] {
                painter.circle_filled(pt(0.29, y), s * 0.055, c.amber);
                line(&[(0.44, y), (0.84, y - 0.08)], Stroke::new(sw, c.cyan));
            }
        }
        Icon::Assistant => {
            // ArcGIS AI Assistant style: crisp dark bubble with 3 dots + bright cobalt sparkle star
            let bubble = box_r(0.12, 0.24, 0.76, 0.72);
            painter.rect(bubble, CornerRadius::same(4), c.slate, Stroke::new(sw, c.ink), StrokeKind::Inside);
            poly(&[(0.24, 0.70), (0.24, 0.86), (0.42, 0.70)], c.slate, Stroke::new(thin, c.ink));
            for x in [0.30, 0.44, 0.58] {
                painter.circle_filled(pt(x, 0.48), s * 0.048, c.ink);
            }
            // Bright cobalt 4-point sparkle star at top-right
            let (cx, cy, rad) = (0.78, 0.24, 0.19);
            poly(
                &[
                    (cx, cy - rad),
                    (cx + rad * 0.30, cy - rad * 0.30),
                    (cx + rad, cy),
                    (cx + rad * 0.30, cy + rad * 0.30),
                    (cx, cy + rad),
                    (cx - rad * 0.30, cy + rad * 0.30),
                    (cx - rad, cy),
                    (cx - rad * 0.30, cy - rad * 0.30),
                ],
                c.blue,
                Stroke::NONE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DAYLIGHT, MIDNIGHT};

    #[test]
    fn tool_icon_mapping_covers_tier3_tools() {
        assert_eq!(for_tool("vel_to_depth", "Velocity"), Icon::DepthModel);
        assert_eq!(for_tool("eikonal_psdm", "Migration"), Icon::KirchhoffPsdm);
        assert_eq!(for_tool("rtm2d", "Migration"), Icon::RtmGpu);
        assert_eq!(for_tool("avo_analysis", "AVO"), Icon::Avo);
        assert_eq!(for_tool("well_tie", "Well Tie"), Icon::WellTie);
        assert_eq!(for_tool("crooked_stack", "Stacking"), Icon::CrookedLine);
    }

    #[test]
    fn icon_colors_are_distinct_for_both_themes() {
        let d = IconColors::from_palette(&MIDNIGHT);
        let l = IconColors::from_palette(&DAYLIGHT);
        assert_ne!(d.ink, l.ink);
        assert_ne!(d.slate, l.slate);
    }
}
