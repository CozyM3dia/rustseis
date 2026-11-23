//! Full-window Backstage Start screen (ArcGIS Pro / RustGIS style):
//! Clean accent sidebar on the left (`←`, Start, New, Open, Save, Save As, About) and spacious
//! dark/light main area on the right with quick-start template cards and Current Project summary.
//! Shown edge-to-edge on first launch without cluttering the window with menus or ribbons.

use super::project::{NewProject, flow_files, refresh_files};
use crate::app::RustSeisApp;
use crate::data::fmt_count;
use crate::icons::{self, Icon};
use crate::state::{Doc, Level};
use crate::theme::{Palette, ThemeMode, font_display, font_mono, font_semibold, mix, with_alpha};
use crate::widgets;
use egui::{Align2, Color32, CornerRadius, Frame, Margin, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as glyph;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StartTab {
    #[default]
    Start,
    About,
}

/// Full-window Backstage Start experience rendered when `app.active == Doc::Start`.
pub fn backstage_ui(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette) {
    refresh_files(&mut app.state);

    let full = ui.max_rect();
    let sidebar_w = 196.0;
    let sidebar_rect = egui::Rect::from_min_max(full.min, pos2(full.left() + sidebar_w, full.bottom()));
    let main_rect = egui::Rect::from_min_max(pos2(full.left() + sidebar_w, full.top()), full.max);

    // Paint clean split backgrounds edge-to-edge.
    let rail_bg = match p.mode {
        ThemeMode::Dark => Color32::from_rgb(176, 70, 42),
        ThemeMode::Light => Color32::from_rgb(186, 74, 44),
    };
    let main_bg = match p.mode {
        ThemeMode::Dark => Color32::from_rgb(30, 34, 42),
        ThemeMode::Light => Color32::from_rgb(245, 246, 249),
    };
    ui.painter().rect_filled(sidebar_rect, 0.0, rail_bg);
    ui.painter().rect_filled(main_rect, 0.0, main_bg);

    // 1. Left accent sidebar.
    let mut rail_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(sidebar_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    rail_ui.set_clip_rect(sidebar_rect);
    left_sidebar(app, &mut rail_ui);

    // 2. Right spacious main area.
    let mut main_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(main_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    main_ui.set_clip_rect(main_rect);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut main_ui, |ui| {
        Frame::new()
            .inner_margin(Margin { left: 44, right: 44, top: 34, bottom: 32 })
            .show(ui, |ui| match app.start_tab {
                StartTab::Start => start_content(app, ui, p, main_bg),
                StartTab::About => about_content(app, ui, p),
            });
    });

    ui.allocate_rect(full, Sense::hover());
}

fn left_sidebar(app: &mut RustSeisApp, ui: &mut Ui) {
    ui.add_space(12.0);
    // Back arrow to enter / return to the workstation.
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let (r, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(6), Color32::from_rgba_unmultiplied(255, 255, 255, 30));
        }
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            glyph::ARROW_LEFT,
            egui::FontId::proportional(18.0),
            Color32::WHITE,
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Enter workstation")
            .clicked()
        {
            let target = app.last_workspace_doc;
            app.open(target);
        }
    });

    ui.add_space(18.0);

    // Start (active section)
    if rail_item(ui, None, "Start", app.start_tab == StartTab::Start).clicked() {
        app.start_tab = StartTab::Start;
    }
    ui.add_space(4.0);
    if rail_item(ui, Some(glyph::FILE_PLUS), "New", false).clicked() {
        app.state.project.new_project = Some(NewProject {
            area: "Survey".into(),
            line: "Line 01".into(),
            ..Default::default()
        });
    }
    if rail_item(ui, Some(glyph::FOLDER_OPEN), "Open", false).clicked() {
        app.state.open_seismic_dialog();
    }
    if rail_item(ui, Some(glyph::FLOPPY_DISK), "Save", false).clicked() {
        app.state.save_flow(false);
    }
    if rail_item(ui, Some(glyph::FLOPPY_DISK_BACK), "Save As", false).clicked() {
        app.state.save_flow(true);
    }
    if rail_item(ui, Some(glyph::INFO), "About", app.start_tab == StartTab::About).clicked() {
        app.start_tab = StartTab::About;
    }

    // Subtle theme toggle anchored near the bottom of the rail.
    let remaining = (ui.available_height() - 48.0).max(12.0);
    ui.add_space(remaining);
    let (ic, lbl) = match app.state.theme {
        ThemeMode::Dark => (glyph::SUN, "Light mode"),
        ThemeMode::Light => (glyph::MOON, "Dark mode"),
    };
    if rail_item(ui, Some(ic), lbl, false).clicked() {
        app.state.theme = app.state.theme.toggled();
    }
}

fn rail_item(ui: &mut Ui, ic: Option<&str>, label: &str, active: bool) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 38.0), Sense::click());
    if active {
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 38));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 20));
    }
    match ic {
        Some(icon_str) => {
            ui.painter().text(
                pos2(rect.left() + 20.0, rect.center().y),
                Align2::LEFT_CENTER,
                icon_str,
                egui::FontId::proportional(15.0),
                Color32::from_rgba_unmultiplied(255, 255, 255, 235),
            );
            ui.painter().text(
                pos2(rect.left() + 44.0, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                if active { font_semibold(13.5) } else { egui::FontId::proportional(13.5) },
                Color32::WHITE,
            );
        }
        None => {
            ui.painter().text(
                pos2(rect.left() + 44.0, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                if active { font_semibold(13.5) } else { egui::FontId::proportional(13.5) },
                Color32::WHITE,
            );
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn start_content(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette, main_bg: Color32) {
    ui.spacing_mut().item_spacing.y = 8.0;

    // Header: RustSeis logo + Welcome title + concise subtitle.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        widgets::logo(ui, p, 42.0);
        ui.label(RichText::new("Welcome to RustSeis").font(font_display(26.0)).color(p.text));
    });
    ui.label(
        RichText::new("Create a new seismic processing flow, open field data, or start from a bundled sample.")
            .size(13.0)
            .color(p.text_muted),
    );

    ui.add_space(22.0);

    // Section 1: New Project (5 clean uniform cards matching the RustGIS Backstage layout).
    ui.label(RichText::new("New Project").font(font_semibold(14.5)).color(p.text));
    ui.add_space(6.0);

    let max_row_w = ui.available_width().min(1060.0);
    let gap = 14.0;
    let card_w = ((max_row_w - gap * 4.0) / 5.0).clamp(158.0, 206.0);

    let demo_flow = find_demo_flow(&app.state);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);

        // 1. Blank Flow
        if project_card(
            ui,
            p,
            main_bg,
            card_w,
            Icon::FlowBuilder,
            "Blank Flow",
            "Build a pipeline from scratch",
        )
        .clicked()
        {
            app.state.flow = Default::default();
            app.state.flow_sel = None;
            app.state.report_uids.clear();
            app.open(Doc::Flow);
        }

        // 2. NIAS 2D Demo (bundled sample)
        if project_card(
            ui,
            p,
            main_bg,
            card_w,
            Icon::Gathers,
            "NIAS 2D Demo",
            "40-shot marine SEG-D & Radon QC",
        )
        .clicked()
        {
            if let Some(path) = demo_flow {
                app.state.open_flow(&path);
                app.state.run_current_flow();
                app.open(Doc::Flow);
            } else {
                app.state.toast(Level::Warn, "Bundled demo flow not found next to executable");
            }
        }

        // 3. Open Data...
        if project_card(
            ui,
            p,
            main_bg,
            card_w,
            Icon::OpenData,
            "Open Data…",
            "Open a SEG-Y or SEG-D file",
        )
        .clicked()
        {
            app.state.open_seismic_dialog();
        }

        // 4. Open Flow...
        if project_card(
            ui,
            p,
            main_bg,
            card_w,
            Icon::SegdFolder,
            "Open Flow…",
            "Open a .json processing flow",
        )
        .clicked()
        {
            app.state.open_flow_dialog();
        }

        // 5. Project DB...
        if project_card(
            ui,
            p,
            main_bg,
            card_w,
            Icon::Database,
            "Project DB…",
            "Open a .sqlite project database",
        )
        .clicked()
        {
            app.state.open_opf_dialog();
        }
    });

    ui.add_space(28.0);

    // Section 2: Current Project summary (exact minimal key-value grid from reference).
    ui.label(RichText::new("Current Project").font(font_semibold(14.5)).color(p.text));
    ui.add_space(6.0);

    let (proj_name, proj_loc) = match &app.state.project.manifest {
        Some(m) => {
            let area = m.get("area").and_then(|v| v.as_str()).unwrap_or("Project");
            let line = m.get("line").and_then(|v| v.as_str()).unwrap_or("");
            let name = if line.is_empty() { area.to_string() } else { format!("{area} / {line}") };
            let loc = app
                .state
                .project
                .opf_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Not saved yet".into());
            (name, loc)
        }
        None => {
            let name = if !app.state.flow.name.is_empty() && app.state.flow.count() > 0 {
                app.state.flow.name.replace('_', " ")
            } else {
                "Untitled project".into()
            };
            let loc = app
                .state
                .flow
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Not saved yet".into());
            (name, loc)
        }
    };

    let traces_val = if app.state.summary.n_traces > 0 {
        format!("{} traces", fmt_count(app.state.summary.n_traces))
    } else {
        "None loaded".into()
    };
    let flow_val = if app.state.flow.count() > 0 {
        format!("{} ({} steps)", app.state.flow.name.replace('_', " "), app.state.flow.count())
    } else {
        "None".into()
    };
    let tables_count = app
        .state
        .session()
        .map(|sess| {
            let c = &sess.context;
            c.velocities.len()
                + c.velocity_grids.len()
                + c.first_breaks.len()
                + c.statics.len()
                + c.sc_amps.len()
                + c.mutes.len()
        })
        .unwrap_or(0);

    egui::Grid::new("start_current_project_grid")
        .num_columns(2)
        .spacing(vec2(36.0, 8.0))
        .show(ui, |ui| {
            for (k, v) in [
                ("Name", proj_name),
                ("Location", proj_loc),
                ("Traces", traces_val),
                ("Active flow", flow_val),
                ("Tables", tables_count.to_string()),
            ] {
                ui.label(RichText::new(k).size(12.5).color(p.text_muted));
                ui.label(RichText::new(v).size(12.5).color(p.text));
                ui.end_row();
            }
        });

    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let btn_props = egui::Button::new(RichText::new("Project Properties…").size(12.5).color(p.text))
            .fill(p.panel)
            .stroke(Stroke::new(1.0, p.border_strong))
            .corner_radius(CornerRadius::same(6))
            .min_size(vec2(0.0, 30.0));
        if ui.add(btn_props).clicked() {
            app.state.project.new_project = Some(NewProject {
                area: "Survey".into(),
                line: "Line 01".into(),
                ..Default::default()
            });
        }

        let btn_open = egui::Button::new(RichText::new("Open Workspace →").size(12.5).color(p.text))
            .fill(p.raised)
            .stroke(Stroke::new(1.0, p.border_strong))
            .corner_radius(CornerRadius::same(6))
            .min_size(vec2(0.0, 30.0));
        if ui.add(btn_open).clicked() {
            let target = app.last_workspace_doc;
            app.open(target);
        }
    });
}

fn project_card(
    ui: &mut Ui,
    p: &Palette,
    main_bg: Color32,
    w: f32,
    icon: Icon,
    title: &str,
    subtitle: &str,
) -> egui::Response {
    let h = 166.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let hov = resp.hovered();
    let painter = ui.painter();

    let card_fill = if hov {
        mix(main_bg, p.text, 0.05)
    } else {
        mix(main_bg, p.panel, 0.45)
    };
    let border_col = if hov {
        with_alpha(p.info, 190)
    } else {
        with_alpha(p.border_strong, 170)
    };
    painter.rect(rect, CornerRadius::same(8), card_fill, Stroke::new(1.0, border_col), StrokeKind::Inside);

    // Dark recessed preview box at the top of the card (matching RustGIS screenshot).
    let well = egui::Rect::from_min_max(
        rect.min + vec2(12.0, 12.0),
        pos2(rect.right() - 12.0, rect.top() + 98.0),
    );
    let well_fill = match p.mode {
        ThemeMode::Dark => Color32::from_rgb(20, 23, 29),
        ThemeMode::Light => Color32::from_rgb(232, 236, 242),
    };
    painter.rect_filled(well, CornerRadius::same(5), well_fill);

    let icon_rect = egui::Rect::from_center_size(well.center(), vec2(46.0, 46.0));
    icons::paint(painter, icon_rect, icon, p);

    // Title + muted 1-line subtitle underneath.
    let clip = egui::Rect::from_min_max(pos2(rect.left() + 12.0, rect.top() + 106.0), pos2(rect.right() - 12.0, rect.bottom() - 8.0));
    painter.with_clip_rect(clip).text(
        pos2(rect.left() + 12.0, rect.top() + 112.0),
        Align2::LEFT_TOP,
        title,
        font_semibold(13.0),
        p.text,
    );
    painter.with_clip_rect(clip).text(
        pos2(rect.left() + 12.0, rect.top() + 134.0),
        Align2::LEFT_TOP,
        subtitle,
        egui::FontId::proportional(11.0),
        p.text_muted,
    );

    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn about_content(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        widgets::logo(ui, p, 42.0);
        ui.label(RichText::new("About RustSeis").font(font_display(26.0)).color(p.text));
    });
    ui.label(
        RichText::new("Pure-Rust 2D Seismic Processing & Depth Imaging Workstation")
            .size(13.5)
            .color(p.text_muted),
    );
    ui.add_space(20.0);

    egui::Grid::new("about_grid")
        .num_columns(2)
        .spacing(vec2(32.0, 8.0))
        .show(ui, |ui| {
            for (k, v) in [
                ("Processing tools", format!("{} built-in tools (Tiers 1, 2 & 3)", app.state.catalog.len())),
                ("Depth imaging", "Dix V(x,z) grid · 2D Eikonal Kirchhoff PSDM · 2D Acoustic RTM (wgpu GPU + CPU)".into()),
                ("Reservoir & logs", "Shuey AVO Intercept/Gradient · CWLS LAS 2.0/3.0 Well Tie · Crooked-Line Stack".into()),
                ("Formats", "SEG-D · SEG-Y · RSDS · CWLS LAS · SQLite OPF Project Database".into()),
                ("Shortcuts", "Ctrl+O Open · Ctrl+Enter Run · Ctrl+K Palette · Ctrl+1..7 Stages".into()),
            ] {
                ui.label(RichText::new(k).size(12.5).color(p.text_muted));
                ui.label(RichText::new(v).font(font_mono(12.0)).color(p.text));
                ui.end_row();
            }
        });
}

fn find_demo_flow(s: &crate::state::AppState) -> Option<std::path::PathBuf> {
    let flows = flow_files(s);
    flows
        .iter()
        .find(|f| f.file_name().is_some_and(|n| n.to_string_lossy().contains("ui_qc_prestack_demo")))
        .or_else(|| flows.first())
        .cloned()
}
