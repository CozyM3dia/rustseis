//! ArcGIS-Pro-style multi-tier Ribbon bar:
//! - Left: current workflow stage's prioritized groups (large primary tools + stacked compact tools)
//! - Right: focused document's contextual group (amber-accented) + dedicated AI Assistant group

use crate::app::{RightMode, RustSeisApp};
use crate::icons::{self, Icon};
use crate::panes::viewer::{KillMode, PickMode};
use crate::ribbon::{self, RIBBON_INNER_H};
use crate::state::{AppState, Doc, Level, Workspace};
use crate::theme::{Palette, font_semibold, with_alpha};
use crate::widgets::panel_frame;
use egui::{CornerRadius, Margin, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as glyph;

pub fn bar(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette) {
    panel_frame(p)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(RIBBON_INNER_H);
            let full = ui.available_rect_before_wrap();

            let scroll_out = egui::ScrollArea::horizontal()
                .id_salt("cmd_stage_ribbon")
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.set_min_height(RIBBON_INNER_H);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        // 1. Current workflow stage groups (left-to-right, ArcGIS Pro style).
                        stage_groups(app, ui, p);
                        // 2. Active document's contextual group (amber divider + accent title).
                        context_group(app, ui, p);
                        // 3. Always-accessible AI Assistant group.
                        ribbon::divider(ui, p);
                        let assist_open = app.right_mode == RightMode::Assistant;
                        ribbon::group(ui, p, "AI", Some(p.info), |ui| {
                            if ribbon::large_button(ui, p, Icon::Assistant, "AI", "Assistant", assist_open, false, false)
                                .on_hover_text("Open or close the geophysical AI Assistant")
                                .clicked()
                            {
                                app.right_mode = if assist_open { RightMode::Inspector } else { RightMode::Assistant };
                            }
                        });
                    });
                });

            let overflow_right = scroll_out.content_size.x > full.width() + 2.0
                && scroll_out.state.clamped_offset().x + full.width() < scroll_out.content_size.x - 2.0;
            if overflow_right {
                let fade = egui::Rect::from_min_max(pos2(full.right() - 26.0, full.top()), full.max);
                ui.painter().rect_filled(fade, 0.0, with_alpha(p.panel, 230));
                ui.painter().text(
                    fade.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph::CARET_RIGHT,
                    egui::FontId::proportional(14.0),
                    p.text_muted,
                );
                ui.interact(fade, egui::Id::unique("cmd_stage_more"), Sense::hover())
                    .on_hover_text("More tools — scroll horizontally here");
            }
        });
}

/// Insert a tool from the library into the flow and show the flow.
fn add_step(app: &mut RustSeisApp, id: &str) {
    app.state.insert_tool(id);
    app.open(Doc::Flow);
    app.state.toast(
        Level::Ok,
        format!("Added “{}” to the flow", app.state.tool(id).map(|t| t.label.clone()).unwrap_or_default()),
    );
}

fn catalog_tools_for(app: &RustSeisApp, cats: &[&str]) -> Vec<(String, String, String)> {
    app.state
        .catalog
        .iter()
        .filter(|t| cats.iter().any(|c| t.category.contains(c)))
        .map(|t| (t.menu_id.clone(), t.label.clone(), t.category.clone()))
        .collect()
}

/// Large ribbon dropdown listing library tools whose category contains any of `cats`.
#[allow(clippy::too_many_arguments)]
fn large_add_menu(
    app: &mut RustSeisApp,
    ui: &mut Ui,
    p: &Palette,
    ic: Icon,
    line1: &str,
    line2: &str,
    cats: &[&str],
) {
    let tools = catalog_tools_for(app, cats);
    let resp = ribbon::large_button(ui, p, ic, line1, line2, false, false, true);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(240.0);
        for (id, name, cat) in &tools {
            if ribbon::menu_item(ui, p, icons::for_tool(id, cat), name).clicked() {
                add_step(app, id);
            }
        }
    });
}

/// Compact stacked ribbon dropdown listing library tools whose category contains any of `cats`.
fn small_add_menu(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette, ic: Icon, label: &str, cats: &[&str]) {
    let tools = catalog_tools_for(app, cats);
    let resp = ribbon::small_button(ui, p, ic, label, false, true);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(240.0);
        for (id, name, cat) in &tools {
            if ribbon::menu_item(ui, p, icons::for_tool(id, cat), name).clicked() {
                add_step(app, id);
            }
        }
    });
}

fn stage_groups(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette) {
    let busy = app.state.is_busy();
    match app.stage {
        Workspace::Data => {
            ribbon::group(ui, p, "Input", None, |ui| {
                let r = ribbon::large_button(ui, p, Icon::OpenData, "Open", "Data", false, true, true)
                    .on_hover_text("Open a SEG-Y, SEG-D, or dataset file (Ctrl+O)");
                egui::Popup::menu(&r).show(|ui| {
                    ui.set_min_width(240.0);
                    if ribbon::menu_item(ui, p, Icon::OpenData, "SEG-Y, SEG-D or dataset file…").clicked() {
                        app.state.open_seismic_dialog();
                    }
                    if ribbon::menu_item(ui, p, Icon::SegdFolder, "SEG-D shot folder…").clicked() {
                        app.state.open_segd_dir_dialog();
                    }
                });
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::SegdFolder, "SEG-D folder", false, false)
                        .on_hover_text("Import a directory of raw SEG-D shot files")
                        .clicked()
                    {
                        app.state.open_segd_dir_dialog();
                    }
                    if ribbon::small_button(ui, p, Icon::Database, "Open DB", false, false)
                        .on_hover_text("Open a saved SQLite project database")
                        .clicked()
                    {
                        app.state.open_opf_dialog();
                    }
                    if ribbon::small_button(ui, p, Icon::SaveFile, "Save DB", false, false)
                        .on_hover_text("Save current tables and velocities to project database")
                        .clicked()
                    {
                        app.state.save_opf_dialog();
                    }
                });
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Geometry", None, |ui| {
                if ribbon::large_button(ui, p, Icon::MarineGeom, "Marine", "Geometry", false, false, false)
                    .on_hover_text("Add marine streamer geometry step to the flow")
                    .clicked()
                {
                    add_step(app, "geomspread_marine");
                }
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::LandGeom, "Land geometry", false, false)
                        .on_hover_text("Add land 2D geometry step to the flow")
                        .clicked()
                    {
                        add_step(app, "geomspread_land");
                    }
                    if ribbon::small_button(ui, p, Icon::CrookedLine, "Crooked line", false, false)
                        .on_hover_text("Add crooked-line binning geometry step to the flow")
                        .clicked()
                    {
                        add_step(app, "geomspread_crooked");
                    }
                    if ribbon::small_button(ui, p, Icon::HeaderMath, "Header maths", false, false)
                        .on_hover_text("Add trace header math expression step to the flow")
                        .clicked()
                    {
                        add_step(app, "thdrmath");
                    }
                });
                if ribbon::large_button(ui, p, Icon::Sort, "Sort", "Traces", false, false, false)
                    .on_hover_text("Add inline / CDP / offset sort step to the flow")
                    .clicked()
                {
                    add_step(app, "inline_sort");
                }
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Views", None, |ui| {
                if ribbon::large_button(ui, p, Icon::SurveyMap, "Survey", "Map", app.active == Doc::Survey, false, false)
                    .on_hover_text("Inspect shot/receiver coordinates, fold, and geometry")
                    .clicked()
                {
                    app.open(Doc::Survey);
                }
                if ribbon::large_button(ui, p, Icon::Gathers, "Gathers", "", app.active == Doc::Viewer, false, false)
                    .on_hover_text("Open the interactive seismic trace gather viewer")
                    .clicked()
                {
                    app.open(Doc::Viewer);
                }
            });
        }

        Workspace::Process => {
            ribbon::group(ui, p, "Execution", None, |ui| {
                if busy {
                    if ribbon::large_button(ui, p, Icon::CancelJob, "Cancel", "Job", true, false, false)
                        .on_hover_text("Cancel the running processing job")
                        .clicked()
                    {
                        app.state.cancel_job();
                    }
                } else if ribbon::large_button(ui, p, Icon::RunFlow, "Run", "Flow", false, true, false)
                    .on_hover_text("Run the active processing flow (Ctrl+Enter)")
                    .clicked()
                {
                    app.state.run_current_flow();
                    app.jobs_open = true;
                }
                ribbon::stack(ui, |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if ribbon::small_button(ui, p, Icon::RunToHere, "Run to here", false, false)
                            .on_hover_text("Run flow up to the currently selected step")
                            .clicked()
                        {
                            run_to_selection(&mut app.state);
                        }
                    });
                    if ribbon::small_button(ui, p, Icon::CheckFlow, "Check flow", false, false)
                        .on_hover_text("Validate flow inputs and step parameters")
                        .clicked()
                    {
                        check_flow(&mut app.state);
                    }
                    if ribbon::small_button(ui, p, Icon::History, "Job history", app.jobs_open, false)
                        .on_hover_text("Toggle background job history drawer (Ctrl+J)")
                        .clicked()
                    {
                        app.jobs_open = !app.jobs_open;
                    }
                });
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Signal", None, |ui| {
                large_add_menu(app, ui, p, Icon::Filter, "Filter &", "Gain", &["Filtering", "Amplitude"]);
                large_add_menu(app, ui, p, Icon::Decon, "Decon", "", &["Deconvolution"]);
                large_add_menu(app, ui, p, Icon::Noise, "Noise &", "Edit", &["Editing", "Data Enhancement"]);
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Subsurface", None, |ui| {
                ribbon::stack(ui, |ui| {
                    small_add_menu(app, ui, p, Icon::Statics, "Statics", &["Statics"]);
                    small_add_menu(app, ui, p, Icon::VelocityPick, "Velocity", &["Velocity"]);
                    small_add_menu(
                        app,
                        ui,
                        p,
                        Icon::KirchhoffPsdm,
                        "Imaging",
                        &["Stacking", "Migration", "Modeling", "AVO", "Well Tie"],
                    );
                });
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Workflow", None, |ui| {
                large_add_menu(app, ui, p, Icon::FlowBuilder, "Flow", "Branch", &["Flow Control", "Extensibility"]);
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::Undo, "Undo", false, false)
                        .on_hover_text("Undo last flow edit (Ctrl+Z)")
                        .clicked()
                    {
                        app.state.undo_flow();
                    }
                    if ribbon::small_button(ui, p, Icon::Redo, "Redo", false, false)
                        .on_hover_text("Redo flow edit (Ctrl+Y)")
                        .clicked()
                    {
                        app.state.redo_flow();
                    }
                    if ribbon::small_button(ui, p, Icon::Toolbox, "All tools", false, false)
                        .on_hover_text("Open searchable Library panel")
                        .clicked()
                    {
                        app.left_tab = crate::app::LeftTab::Library;
                        app.left_open = true;
                    }
                });
            });
        }

        Workspace::Velocity => {
            ribbon::group(ui, p, "Picking", None, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if ribbon::large_button(ui, p, Icon::DoneCheck, "Save", "Velocities", false, true, false)
                        .on_hover_text("Commit velocity picks to the active velocity table")
                        .clicked()
                    {
                        crate::panes::iva::save_table(&mut app.state, false);
                    }
                });
                if ribbon::large_button(ui, p, Icon::AutoPick, "Auto-Pick", "", false, false, false)
                    .on_hover_text("Automatically track semblance maxima at this CDP")
                    .clicked()
                {
                    app.state.iva.auto_pick_current();
                }
                ribbon::stack(ui, |ui| {
                    let snap = app.state.iva.snap;
                    if ribbon::small_button(ui, p, Icon::SnapPeak, "Snap to peak", snap, false)
                        .on_hover_text("Snap manual picks to local semblance maxima")
                        .clicked()
                    {
                        app.state.iva.snap = !snap;
                    }
                    if ribbon::small_button(ui, p, Icon::Clear, "Clear CDP", false, false)
                        .on_hover_text("Clear velocity picks at the current CDP location")
                        .clicked()
                    {
                        app.state.iva.clear_current();
                    }
                });
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Location", None, |ui| {
                ribbon::stack(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let step_btn = |ui: &mut Ui, ic: &str, tip: &str| -> bool {
                            let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                            let fill = if resp.hovered() { p.raised } else { p.sunken };
                            ui.painter().rect(r, CornerRadius::same(5), fill, Stroke::new(1.0, p.border_strong), StrokeKind::Inside);
                            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(13.0), p.text);
                            resp.on_hover_text(tip).clicked()
                        };
                        if step_btn(ui, glyph::CARET_LEFT, "Previous location ( [ )") {
                            app.state.iva.step_location(-1);
                        }
                        let (lbl_r, _) = ui.allocate_exact_size(vec2(72.0, 26.0), Sense::hover());
                        ui.painter().rect(lbl_r, CornerRadius::same(5), p.sunken, Stroke::new(1.0, p.border), StrokeKind::Inside);
                        ui.painter().text(
                            lbl_r.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("CDP {}", app.state.iva.current_cdp()),
                            font_semibold(12.0),
                            p.text,
                        );
                        if step_btn(ui, glyph::CARET_RIGHT, "Next location ( ] )") {
                            app.state.iva.step_location(1);
                        }
                    });
                    ui.add_space(4.0);
                    let super_label = format!("Supergather · {} CDPs", app.state.iva.super_half * 2 + 1);
                    let r = ribbon::small_button(ui, p, Icon::Gathers, &super_label, false, true);
                    egui::Popup::menu(&r).show(|ui| {
                        for n in [1usize, 3, 5, 7, 9] {
                            if ui.selectable_label(app.state.iva.super_half * 2 + 1 == n, format!("{n} CDPs")).clicked() {
                                app.state.iva.super_half = n / 2;
                            }
                        }
                    });
                });
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Display", None, |ui| {
                let show_field = app.state.iva.show_field;
                if ribbon::large_button(ui, p, Icon::VelocityField, "Velocity", "Field", show_field, false, false)
                    .on_hover_text("Show 2D interpolated RMS velocity section")
                    .clicked()
                {
                    app.state.iva.show_field = !show_field;
                }
                ribbon::stack(ui, |ui| {
                    let no_nmo = !app.state.iva.apply_nmo;
                    if ribbon::small_button(ui, p, Icon::Nmo, "Without NMO", no_nmo, false)
                        .on_hover_text("Toggle NMO correction to inspect residual moveout")
                        .clicked()
                    {
                        app.state.iva.apply_nmo = !app.state.iva.apply_nmo;
                    }
                    if ribbon::small_button(ui, p, Icon::DepthModel, "Convert to Depth", false, false)
                        .on_hover_text("Add Dix RMS-to-depth V(x,z) conversion step to the flow")
                        .clicked()
                    {
                        add_step(app, "vel_to_depth");
                    }
                });
            });
        }

        Workspace::Statics => {
            ribbon::group(ui, p, "First Breaks", None, |ui| {
                if ribbon::large_button(ui, p, Icon::FirstBreak, "Pick First", "Breaks", false, true, false)
                    .on_hover_text("Open Gather Viewer in First-Break picking mode")
                    .clicked()
                {
                    app.state.viewer.mode = PickMode::FirstBreak;
                    app.open(Doc::Viewer);
                }
                if ribbon::large_button(ui, p, Icon::AutoPick, "Auto Pick", "Breaks", false, false, false)
                    .on_hover_text("Add automatic STA/LTA first-break picker to the flow")
                    .clicked()
                {
                    add_step(app, "fbpick");
                }
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Solvers", None, |ui| {
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::Statics, "Refraction statics", false, false)
                        .on_hover_text("Add refraction statics solver to the flow")
                        .clicked()
                    {
                        add_step(app, "refstatic");
                    }
                    if ribbon::small_button(ui, p, Icon::Statics, "Residual statics", false, false)
                        .on_hover_text("Add surface-consistent residual statics to the flow")
                        .clicked()
                    {
                        add_step(app, "autostat");
                    }
                    if ribbon::small_button(ui, p, Icon::Statics, "Stack power", false, false)
                        .on_hover_text("Add maximum-power autostatics to the flow")
                        .clicked()
                    {
                        add_step(app, "pwr_stat_new");
                    }
                });
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::Statics, "Trim statics", false, false)
                        .on_hover_text("Add CDP trim statics to the flow")
                        .clicked()
                    {
                        add_step(app, "trimstat");
                    }
                    if ribbon::small_button(ui, p, Icon::Amplitude, "Amplitude terms", false, false)
                        .on_hover_text("Add surface-consistent amplitude decomposition to the flow")
                        .clicked()
                    {
                        add_step(app, "scamp2");
                    }
                });
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Analysis", None, |ui| {
                if ribbon::large_button(ui, p, Icon::StaticsQc, "Statics", "QC", app.active == Doc::Statics, false, false)
                    .on_hover_text("Inspect shot/receiver static shifts and datum QC")
                    .clicked()
                {
                    app.open(Doc::Statics);
                }
            });
        }

        Workspace::Imaging => {
            ribbon::group(ui, p, "Depth Imaging", None, |ui| {
                if ribbon::large_button(ui, p, Icon::DepthModel, "Depth", "Model", false, false, false)
                    .on_hover_text("Add Dix RMS-to-Depth V(x,z) velocity grid step")
                    .clicked()
                {
                    add_step(app, "vel_to_depth");
                }
                if ribbon::large_button(ui, p, Icon::KirchhoffPsdm, "Kirchhoff", "PSDM", false, false, false)
                    .on_hover_text("Add 2D Eikonal Kirchhoff Pre-Stack Depth Migration step")
                    .clicked()
                {
                    add_step(app, "eikonal_psdm");
                }
                if ribbon::large_button(ui, p, Icon::RtmGpu, "2D RTM", "(GPU)", false, true, false)
                    .on_hover_text("Add 2D Acoustic Reverse Time Migration (GPU/CPU) step")
                    .clicked()
                {
                    add_step(app, "rtm2d");
                }
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Time Imaging", None, |ui| {
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::Nmo, "NMO correction", false, false)
                        .on_hover_text("Add Normal Moveout correction step")
                        .clicked()
                    {
                        add_step(app, "nmo");
                    }
                    if ribbon::small_button(ui, p, Icon::Stack, "CDP stack", false, false)
                        .on_hover_text("Add CDP stack step")
                        .clicked()
                    {
                        add_step(app, "stack");
                    }
                    if ribbon::small_button(ui, p, Icon::CrookedLine, "Crooked stack", false, false)
                        .on_hover_text("Add crooked-line azimuth-weighted stack step")
                        .clicked()
                    {
                        add_step(app, "crooked_stack");
                    }
                });
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::TimeMig, "Time migration", false, false)
                        .on_hover_text("Add 2D Kirchhoff time migration step")
                        .clicked()
                    {
                        add_step(app, "kirtime2d");
                    }
                    if ribbon::small_button(ui, p, Icon::Spectra, "F-K migration", false, false)
                        .on_hover_text("Add Stolt F-K post-stack migration step")
                        .clicked()
                    {
                        add_step(app, "fkmig2d");
                    }
                    if ribbon::small_button(ui, p, Icon::TimeMig, "DMO panel", false, false)
                        .on_hover_text("Add Dip Moveout F-K correction step")
                        .clicked()
                    {
                        add_step(app, "dmofkpanel");
                    }
                });
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "QC", None, |ui| {
                if ribbon::large_button(ui, p, Icon::Compare, "Compare", "", app.state.viewer.compare, false, false)
                    .on_hover_text("Toggle split before/after comparison in Gather Viewer")
                    .clicked()
                {
                    app.state.viewer.compare = !app.state.viewer.compare;
                    app.open(Doc::Viewer);
                }
            });
        }

        Workspace::Qc => {
            ribbon::group(ui, p, "Inspection", None, |ui| {
                if ribbon::large_button(ui, p, Icon::Gathers, "Gathers", "", app.active == Doc::Viewer, false, false)
                    .on_hover_text("Inspect shot, CDP, or stacked seismic gathers")
                    .clicked()
                {
                    app.open(Doc::Viewer);
                }
                if ribbon::large_button(ui, p, Icon::Spectra, "Spectra", "", app.active == Doc::Spectral, false, false)
                    .on_hover_text("Inspect amplitude spectra, F-K panels, and Radon demultiple")
                    .clicked()
                {
                    app.open(Doc::Spectral);
                }
                if ribbon::large_button(ui, p, Icon::Compare, "Compare", "", app.state.viewer.compare, false, false)
                    .on_hover_text("Toggle side-by-side before/after comparison")
                    .clicked()
                {
                    app.state.viewer.compare = !app.state.viewer.compare;
                    app.open(Doc::Viewer);
                }
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Reservoir", None, |ui| {
                if ribbon::large_button(ui, p, Icon::Avo, "AVO", "Analysis", false, true, false)
                    .on_hover_text("Add AVO Intercept / Gradient (Shuey) analysis step")
                    .clicked()
                {
                    add_step(app, "avo_analysis");
                }
                if ribbon::large_button(ui, p, Icon::WellTie, "Well", "Tie", false, false, false)
                    .on_hover_text("Add CWLS LAS Well Tie & synthetic seismogram step")
                    .clicked()
                {
                    add_step(app, "well_tie");
                }
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Basemap", None, |ui| {
                if ribbon::large_button(ui, p, Icon::SurveyMap, "Survey", "Map", app.active == Doc::Survey, false, false)
                    .on_hover_text("Open survey geometry and fold basemap")
                    .clicked()
                {
                    app.open(Doc::Survey);
                }
            });
        }

        Workspace::Borehole => {
            ribbon::group(ui, p, "VSP / DAS", None, |ui| {
                if ribbon::large_button(ui, p, Icon::VspSep, "Separate", "Up / Down", false, true, false)
                    .on_hover_text("Add VSP upgoing / downgoing wavefield separation step")
                    .clicked()
                {
                    add_step(app, "vsp_wavefield_sep");
                }
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::BoreholeView, "Depth geometry", false, false)
                        .on_hover_text("Add borehole depth geometry assignment step")
                        .clicked()
                    {
                        add_step(app, "vsp_geom");
                    }
                    if ribbon::small_button(ui, p, Icon::VelocityPick, "Slowness", false, false)
                        .on_hover_text("Add interval slowness estimation step")
                        .clicked()
                    {
                        add_step(app, "vsp_slowness");
                    }
                });
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::Decon, "VSP decon", false, false)
                        .on_hover_text("Add downgoing-wavelet VSP deconvolution step")
                        .clicked()
                    {
                        add_step(app, "vsp_decon");
                    }
                    if ribbon::small_button(ui, p, Icon::Stack, "Corridor stack", false, false)
                        .on_hover_text("Add VSP corridor stack step")
                        .clicked()
                    {
                        add_step(app, "vsp_corridor_stack");
                    }
                });
            });
            ribbon::divider(ui, p);
            ribbon::group(ui, p, "Views", None, |ui| {
                if ribbon::large_button(ui, p, Icon::BoreholeView, "Borehole", "View", app.active == Doc::Borehole, false, false)
                    .on_hover_text("Open interactive VSP / DAS borehole viewer")
                    .clicked()
                {
                    app.open(Doc::Borehole);
                }
            });
        }
    }
}

/// Active document's contextual ribbon group (cleanly separated left-to-right with an amber rule).
fn context_group(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette) {
    match app.active {
        Doc::Viewer => {
            ribbon::context_divider(ui, p, p.pick);
            if app.state.viewer.mode != PickMode::Navigate {
                ribbon::group(ui, p, "Picking", Some(p.pick), |ui| {
                    if ribbon::large_button(ui, p, Icon::DoneCheck, "Done", "", false, true, false)
                        .on_hover_text("Save picks and return to navigation mode")
                        .clicked()
                    {
                        match app.state.viewer.mode {
                            PickMode::Mute => crate::panes::viewer::save_mute(&mut app.state),
                            PickMode::FirstBreak => crate::panes::viewer::save_first_breaks(&mut app.state),
                            PickMode::Kill => crate::panes::viewer::apply_trace_edits(&mut app.state),
                            PickMode::Navigate => {}
                        }
                        app.state.viewer.mode = PickMode::Navigate;
                    }
                    ribbon::stack(ui, |ui| {
                        let mode = app.state.viewer.mode;
                        if ribbon::small_button(ui, p, Icon::Mute, "Mute", mode == PickMode::Mute, false).clicked() {
                            app.state.viewer.mode = PickMode::Mute;
                        }
                        if ribbon::small_button(ui, p, Icon::FirstBreak, "First breaks", mode == PickMode::FirstBreak, false).clicked() {
                            app.state.viewer.mode = PickMode::FirstBreak;
                        }
                        if ribbon::small_button(ui, p, Icon::KillTrace, "Edit traces", mode == PickMode::Kill, false).clicked() {
                            app.state.viewer.mode = PickMode::Kill;
                        }
                    });
                    ribbon::stack(ui, |ui| {
                        match app.state.viewer.mode {
                            PickMode::Mute => {
                                let cur = app.state.viewer.mute_mode;
                                let next_label = match cur {
                                    rustseis_core::MuteMode::Top => "Mode: Top",
                                    rustseis_core::MuteMode::Bottom => "Mode: Bottom",
                                    rustseis_core::MuteMode::Surgical => "Mode: Window",
                                };
                                let r = ribbon::small_button(ui, p, Icon::Mute, next_label, false, true);
                                egui::Popup::menu(&r).show(|ui| {
                                    for (m, lbl) in [
                                        (rustseis_core::MuteMode::Top, "Top mute"),
                                        (rustseis_core::MuteMode::Bottom, "Bottom mute"),
                                        (rustseis_core::MuteMode::Surgical, "Surgical window"),
                                    ] {
                                        if ui.selectable_label(app.state.viewer.mute_mode == m, lbl).clicked() {
                                            app.state.viewer.mute_mode = m;
                                        }
                                    }
                                });
                                let prev = app.state.viewer.mute_preview;
                                if ribbon::small_button(ui, p, Icon::EyePreview, "Show result", prev, false).clicked() {
                                    app.state.viewer.mute_preview = !prev;
                                }
                            }
                            PickMode::FirstBreak => {
                                if ribbon::small_button(ui, p, Icon::AutoPick, "Auto-pick shot", false, false).clicked() {
                                    crate::panes::viewer::auto_pick_fb(&mut app.state);
                                }
                            }
                            PickMode::Kill => {
                                let cur = app.state.viewer.kill_mode;
                                let km_label = match cur {
                                    KillMode::KillTrace => "Kill trace",
                                    KillMode::KillChannel => "Kill channel",
                                    KillMode::ReverseChannel => "Reverse pol.",
                                };
                                let r = ribbon::small_button(ui, p, Icon::KillTrace, km_label, false, true);
                                egui::Popup::menu(&r).show(|ui| {
                                    for (m, lbl) in [
                                        (KillMode::KillTrace, "Kill trace"),
                                        (KillMode::KillChannel, "Kill channel"),
                                        (KillMode::ReverseChannel, "Reverse channel polarity"),
                                    ] {
                                        if ui.selectable_label(app.state.viewer.kill_mode == m, lbl).clicked() {
                                            app.state.viewer.kill_mode = m;
                                        }
                                    }
                                });
                            }
                            PickMode::Navigate => {}
                        }
                        if ribbon::small_button(ui, p, Icon::CancelJob, "Cancel", false, false)
                            .on_hover_text("Discard active picking mode (Esc)")
                            .clicked()
                        {
                            app.state.viewer.mode = PickMode::Navigate;
                        }
                    });
                });
            } else {
                ribbon::group(ui, p, "Gather", Some(p.pick), |ui| {
                    if ribbon::large_button(ui, p, Icon::Mute, "Pick", "Mute", false, false, false)
                        .on_hover_text("Interactively pick top, bottom, or surgical mute polygon")
                        .clicked()
                    {
                        app.state.viewer.mode = PickMode::Mute;
                    }
                    if ribbon::large_button(ui, p, Icon::FirstBreak, "First", "Breaks", false, false, false)
                        .on_hover_text("Pick or refine refraction first-break arrivals")
                        .clicked()
                    {
                        app.state.viewer.mode = PickMode::FirstBreak;
                    }
                    ribbon::stack(ui, |ui| {
                        if ribbon::small_button(ui, p, Icon::KillTrace, "Edit traces", false, false)
                            .on_hover_text("Kill noisy traces/channels or reverse polarity")
                            .clicked()
                        {
                            app.state.viewer.mode = PickMode::Kill;
                        }
                        let wiggles = app.state.viewer.wiggles;
                        if ribbon::small_button(ui, p, Icon::Wiggles, "Wiggles", wiggles, false)
                            .on_hover_text("Overlay variable-area wiggle traces")
                            .clicked()
                        {
                            app.state.viewer.wiggles = !wiggles;
                        }
                        let r = ribbon::small_button(ui, p, Icon::Export, "Export", false, true);
                        egui::Popup::menu(&r).show(|ui| {
                            if ribbon::menu_item(ui, p, Icon::Compare, "Before / after image (PNG)…").clicked() {
                                crate::panes::viewer::export_side_by_side(&mut app.state);
                            }
                        });
                    });
                });
            }
        }

        Doc::Flow => {
            ribbon::context_divider(ui, p, p.pick);
            ribbon::group(ui, p, "Flow", Some(p.pick), |ui| {
                let r = ribbon::large_button(ui, p, Icon::OpenData, "Open", "Flow", false, false, true);
                let flows = crate::panes::project::flow_files(&app.state);
                egui::Popup::menu(&r).show(|ui| {
                    ui.set_min_width(260.0);
                    for f in &flows {
                        let name = f.file_stem().map(|x| x.to_string_lossy().replace('_', " ")).unwrap_or_default();
                        if ribbon::menu_item(ui, p, Icon::FlowBuilder, &name).clicked() {
                            app.state.request_open_flow(f.clone());
                        }
                    }
                    ui.separator();
                    if ribbon::menu_item(ui, p, Icon::OpenData, "Browse…").clicked() {
                        app.state.open_flow_dialog();
                    }
                });
                if ribbon::large_button(ui, p, Icon::SaveFile, "Save", "", false, false, false)
                    .on_hover_text("Save flow (Ctrl+S)")
                    .clicked()
                {
                    app.state.save_flow(false);
                }
                ribbon::stack(ui, |ui| {
                    if ribbon::small_button(ui, p, Icon::NewFile, "New flow", false, false)
                        .on_hover_text("Start an empty processing flow")
                        .clicked()
                    {
                        app.state.flow = Default::default();
                        app.state.flow_sel = None;
                        app.state.report_uids.clear();
                    }
                    let is_json = app.state.inspector.flow_json;
                    if ribbon::small_button(ui, p, Icon::CodeJson, "Flow JSON", is_json, false)
                        .on_hover_text("Edit the processing flow directly as JSON")
                        .clicked()
                    {
                        app.state.inspector.flow_json = !is_json;
                    }
                });
            });
        }

        Doc::Spectral => {
            ribbon::context_divider(ui, p, p.pick);
            ribbon::group(ui, p, "Spectra", Some(p.pick), |ui| {
                if ribbon::large_button(ui, p, Icon::DoneCheck, "Add to", "Flow", false, true, false)
                    .on_hover_text("Insert the designed spectral/F-K/Radon filter into the flow")
                    .clicked()
                {
                    crate::panes::spectral::add_active_filter_to_flow(&mut app.state);
                    app.open(Doc::Flow);
                }
                if ribbon::large_button(ui, p, Icon::EyePreview, "Preview", "Radon", false, false, false)
                    .on_hover_text("Compute live parabolic Radon demultiple preview")
                    .clicked()
                {
                    app.state.spectral.request_radon_preview();
                }
                ribbon::stack(ui, |ui| {
                    let follow = app.state.spectral.follow_viewer;
                    if ribbon::small_button(ui, p, Icon::LinkFollow, "Follow gather", follow, false)
                        .on_hover_text("Sync spectral analysis to the active gather in Viewer")
                        .clicked()
                    {
                        app.state.spectral.follow_viewer = !follow;
                    }
                });
            });
        }

        _ => {}
    }
}

fn run_to_selection(s: &mut AppState) {
    let Some(sel) = s.flow_sel.clone() else {
        s.toast(Level::Warn, "Select a step first");
        return;
    };
    let top = sel[0];
    let mut spec = s.flow.to_spec();
    spec.steps.truncate(top + 1);
    spec.name = format!("{} (to step {})", spec.name, top + 1);
    s.report_uids = s.flow.nodes.iter().take(top + 1).map(|n| n.uid).collect();
    let name = spec.name.clone();
    s.run_flow(spec, crate::state::JobPurpose::Flow { flow_name: name });
}

fn check_flow(s: &mut AppState) {
    let spec = s.flow.to_spec();
    let unknown: Vec<String> = spec
        .steps
        .iter()
        .filter(|st| s.tool(&st.tool).is_none())
        .map(|st| st.tool.clone())
        .collect();
    let has_input = spec
        .steps
        .first()
        .is_some_and(|st| matches!(st.tool.as_str(), "segd_read" | "segy_read" | "diskread2" | "synth" | "afd"));
    if spec.steps.is_empty() {
        s.toast(Level::Warn, "The flow is empty");
    } else if !unknown.is_empty() {
        s.toast(Level::Error, format!("Unknown steps: {}", unknown.join(", ")));
    } else if !has_input && s.summary.n_traces == 0 {
        s.toast(Level::Warn, "The flow does not read data and nothing is loaded");
    } else {
        s.toast(Level::Ok, format!("Flow looks good · {} steps", spec.steps.len()));
    }
}
