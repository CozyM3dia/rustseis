//! The command bar: the current stage's actions on the left, the focused document's actions on
//! the right under an amber label.

use crate::app::RustSeisApp;
use crate::panes::viewer::{KillMode, PickMode};
use crate::state::{AppState, Doc, Level, Workspace};
use crate::theme::Palette;
use crate::widgets::{cmd, cmd_primary, cmd_sep, ctx_label, dim_label, panel_frame};
use egui::{Margin, RichText, Ui};
use egui_phosphor::regular as icon;
use rustseis_render::SeismicColormap;

pub fn bar(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette) {
    panel_frame(p).corner_radius(egui::CornerRadius::same(10)).inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let full = ui.available_rect_before_wrap();
        // Document commands first (right-aligned) so the stage commands get whatever is left.
        let mut right = ui.new_child(egui::UiBuilder::new().max_rect(full).layout(egui::Layout::right_to_left(egui::Align::Center)));
        right.spacing_mut().item_spacing.x = 2.0;
        context_commands(app, &mut right, p);
        let used = right.min_rect().width();
        let left_rect = egui::Rect::from_min_max(full.min, egui::pos2((full.right() - used - 12.0).max(full.left()), full.bottom()));
        let mut left = ui.new_child(egui::UiBuilder::new().max_rect(left_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        left.set_clip_rect(left_rect.intersect(ui.clip_rect()));
        let scroll_out = egui::ScrollArea::horizontal()
            .id_salt("cmd_stage")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(&mut left, |ui| {
                ui.set_height(left_rect.height());
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    stage_commands(app, ui, p);
                });
            });
        let overflow_right = scroll_out.content_size.x > left_rect.width() + 2.0
            && scroll_out.state.clamped_offset().x + left_rect.width() < scroll_out.content_size.x - 2.0;
        if overflow_right {
            let fade = egui::Rect::from_min_max(egui::pos2(left_rect.right() - 24.0, left_rect.top()), left_rect.max);
            ui.painter().rect_filled(fade, 0.0, crate::theme::with_alpha(p.panel, 225));
            ui.painter().text(
                fade.center(),
                egui::Align2::CENTER_CENTER,
                icon::CARET_RIGHT,
                egui::FontId::proportional(13.0),
                p.text_muted,
            );
            ui.interact(fade, egui::Id::unique("cmd_stage_more"), egui::Sense::hover())
                .on_hover_text("More commands — scroll horizontally here");
        }
        ui.allocate_rect(full, egui::Sense::hover());
    });
}

/// Insert a tool from the library into the flow and show the flow.
fn add_step(app: &mut RustSeisApp, id: &str) {
    app.state.insert_tool(id);
    app.open(Doc::Flow);
    app.state.toast(Level::Ok, format!("Added “{}” to the flow", app.state.tool(id).map(|t| t.label.clone()).unwrap_or_default()));
}

/// Menu listing library tools whose category contains any of `cats`.
fn add_menu(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette, ic: &str, label: &str, cats: &[&str]) {
    let tools: Vec<(String, String)> = app
        .state
        .catalog
        .iter()
        .filter(|t| cats.iter().any(|c| t.category.contains(c)))
        .map(|t| (t.menu_id.clone(), t.label.clone()))
        .collect();
    let resp = cmd(ui, p, ic, &format!("{label}  ▾"), false);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(220.0);
        for (id, name) in &tools {
            if ui.button(name).clicked() {
                add_step(app, id);
            }
        }
    });
}

fn stage_commands(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette) {
    let busy = app.state.is_busy();
    match app.stage {
        Workspace::Data => {
            let r = cmd(ui, p, icon::FOLDER_OPEN, "Open data  ▾", false);
            egui::Popup::menu(&r).show(|ui| {
                if ui.button("SEG-Y, SEG-D or dataset file…").clicked() {
                    app.state.open_seismic_dialog();
                }
                if ui.button("SEG-D shot folder…").clicked() {
                    app.state.open_segd_dir_dialog();
                }
            });
            cmd_sep(ui, p);
            dim_label(ui, p, "GEOMETRY");
            for (id, label) in [("geomspread_marine", "Marine"), ("geomspread_land", "Land"), ("geomspread_crooked", "Crooked line"), ("thdrmath", "Header maths"), ("inline_sort", "Sort")] {
                if cmd(ui, p, "", label, false).on_hover_text("Adds this step to the flow").clicked() {
                    add_step(app, id);
                }
            }
            cmd_sep(ui, p);
            if cmd(ui, p, icon::MAP_TRIFOLD, "Survey map", app.active == Doc::Survey).clicked() {
                app.open(Doc::Survey);
            }
            if cmd(ui, p, icon::WAVEFORM, "Gathers", app.active == Doc::Viewer).clicked() {
                app.open(Doc::Viewer);
            }
        }
        Workspace::Process => {
            ui.add_enabled_ui(!busy, |ui| {
                if cmd_primary(ui, p, icon::PLAY, "Run flow").on_hover_text("Ctrl+Enter").clicked() {
                    app.state.run_current_flow();
                    app.jobs_open = true;
                }
                if cmd(ui, p, icon::SKIP_FORWARD, "Run to here", false).on_hover_text("Run up to the selected step").clicked() {
                    run_to_selection(&mut app.state);
                }
            });
            if cmd(ui, p, icon::CHECK_CIRCLE, "Check", false).clicked() {
                check_flow(&mut app.state);
            }
            cmd_sep(ui, p);
            dim_label(ui, p, "ADD");
            add_menu(app, ui, p, icon::WAVE_SINE, "Filter", &["Filtering", "Amplitude"]);
            add_menu(app, ui, p, icon::ARROWS_IN_LINE_VERTICAL, "Decon", &["Deconvolution"]);
            add_menu(app, ui, p, icon::WAVES, "Noise", &["Editing", "Data Enhancement"]);
            add_menu(app, ui, p, icon::RULER, "Statics", &["Statics"]);
            add_menu(app, ui, p, icon::CHART_LINE_UP, "Velocity", &["Velocity"]);
            add_menu(app, ui, p, icon::STACK, "Imaging", &["Stacking", "Migration", "Modeling"]);
            add_menu(app, ui, p, icon::GIT_BRANCH, "Branch", &["Flow Control", "Extensibility"]);
        }
        Workspace::Velocity => {
            let iva = &mut app.state.iva;
            if cmd(ui, p, icon::SPARKLE, "Auto-pick", false).clicked() {
                iva.auto_pick_current();
            }
            if cmd(ui, p, icon::MAGNET, "Snap to peak", iva.snap).clicked() {
                iva.snap = !iva.snap;
            }
            if cmd(ui, p, icon::ERASER, "Clear", false).clicked() {
                iva.clear_current();
            }
            cmd_sep(ui, p);
            if cmd(ui, p, icon::CARET_LEFT, "", false).on_hover_text("Previous location ( [ )").clicked() {
                iva.step_location(-1);
            }
            ui.label(RichText::new(format!("CDP {}", iva.current_cdp())).strong().color(p.text));
            if cmd(ui, p, icon::CARET_RIGHT, "", false).on_hover_text("Next location ( ] )").clicked() {
                iva.step_location(1);
            }
            let r = cmd(ui, p, "", &format!("Supergather · {} CDPs  ▾", iva.super_half * 2 + 1), false);
            egui::Popup::menu(&r).show(|ui| {
                for n in [1usize, 3, 5, 7, 9] {
                    if ui.selectable_label(app.state.iva.super_half * 2 + 1 == n, format!("{n} CDPs")).clicked() {
                        app.state.iva.super_half = n / 2;
                    }
                }
            });
            let iva = &mut app.state.iva;
            cmd_sep(ui, p);
            if cmd(ui, p, icon::GRADIENT, "Velocity field", iva.show_field).clicked() {
                iva.show_field = !iva.show_field;
            }
            if cmd(ui, p, "", "Without NMO", !iva.apply_nmo).on_hover_text("Inspect image gathers for residual moveout").clicked() {
                iva.apply_nmo = !iva.apply_nmo;
            }
        }
        Workspace::Statics => {
            if cmd(ui, p, icon::LIGHTNING, "Pick first breaks", false).clicked() {
                app.state.viewer.mode = PickMode::FirstBreak;
                app.open(Doc::Viewer);
            }
            cmd_sep(ui, p);
            dim_label(ui, p, "SOLVE");
            for (id, label) in [("fbpick", "Auto first breaks"), ("refstatic", "Refraction"), ("autostat", "Residual"), ("pwr_stat_new", "Stack power"), ("trimstat", "Trim")] {
                if cmd(ui, p, "", label, false).on_hover_text("Adds this step to the flow").clicked() {
                    add_step(app, id);
                }
            }
            if cmd(ui, p, "", "Amplitude terms", false).clicked() {
                add_step(app, "scamp2");
            }
            cmd_sep(ui, p);
            if cmd(ui, p, icon::CHART_BAR, "Statics QC", app.active == Doc::Statics).clicked() {
                app.open(Doc::Statics);
            }
        }
        Workspace::Imaging => {
            dim_label(ui, p, "ADD");
            for (id, label) in [("nmo", "NMO"), ("stack", "Stack"), ("kirtime2d", "Time migration"), ("fdmig", "Steep-dip migration"), ("fkmig2d", "F-K migration"), ("dmofkpanel", "DMO")] {
                if cmd(ui, p, "", label, false).clicked() {
                    add_step(app, id);
                }
            }
            cmd_sep(ui, p);
            if cmd(ui, p, icon::COLUMNS, "Compare", app.state.viewer.compare).clicked() {
                app.state.viewer.compare = !app.state.viewer.compare;
                app.open(Doc::Viewer);
            }
        }
        Workspace::Qc => {
            if cmd(ui, p, icon::WAVEFORM, "Gathers", app.active == Doc::Viewer).clicked() {
                app.open(Doc::Viewer);
            }
            if cmd(ui, p, icon::WAVE_SINE, "Spectra", app.active == Doc::Spectral).clicked() {
                app.open(Doc::Spectral);
            }
            if cmd(ui, p, icon::COLUMNS, "Compare", app.state.viewer.compare).clicked() {
                app.state.viewer.compare = !app.state.viewer.compare;
                app.open(Doc::Viewer);
            }
            cmd_sep(ui, p);
            if cmd(ui, p, icon::MAP_TRIFOLD, "Survey map", false).clicked() {
                app.open(Doc::Survey);
            }
        }
        Workspace::Borehole => {
            dim_label(ui, p, "ADD");
            for (id, label) in [("vsp_geom", "Depth geometry"), ("vsp_slowness", "Slowness"), ("vsp_wavefield_sep", "Separate up / down"), ("vsp_decon", "Deconvolution"), ("vsp_corridor_stack", "Corridor stack")] {
                if cmd(ui, p, "", label, false).clicked() {
                    add_step(app, id);
                }
            }
            cmd_sep(ui, p);
            if cmd(ui, p, icon::DROP, "Borehole view", app.active == Doc::Borehole).clicked() {
                app.open(Doc::Borehole);
            }
        }
    }
}

fn context_commands(app: &mut RustSeisApp, ui: &mut Ui, p: &Palette) {
    // Right-to-left layout: add items in reverse visual order.
    match app.active {
        Doc::Viewer => {
            let v = &mut app.state.viewer;
            if v.mode != PickMode::Navigate {
                if cmd_primary(ui, p, icon::CHECK, "Done").clicked() {
                    match v.mode {
                        PickMode::Mute => crate::panes::viewer::save_mute(&mut app.state),
                        PickMode::FirstBreak => crate::panes::viewer::save_first_breaks(&mut app.state),
                        PickMode::Kill => crate::panes::viewer::apply_trace_edits(&mut app.state),
                        PickMode::Navigate => {}
                    }
                    app.state.viewer.mode = PickMode::Navigate;
                }
                if cmd(ui, p, "", "Cancel", false).on_hover_text("Esc").clicked() {
                    app.state.viewer.mode = PickMode::Navigate;
                }
                let v = &mut app.state.viewer;
                cmd_sep(ui, p);
                if v.mode == PickMode::Kill {
                    for (m, label) in [(KillMode::ReverseChannel, "Reverse channel"), (KillMode::KillChannel, "Kill channel"), (KillMode::KillTrace, "Kill trace")] {
                        if cmd(ui, p, "", label, v.kill_mode == m).clicked() {
                            v.kill_mode = m;
                        }
                    }
                }
                if v.mode == PickMode::Mute {
                    if cmd(ui, p, icon::EYE, "Show result", v.mute_preview).clicked() {
                        v.mute_preview = !v.mute_preview;
                    }
                    for (m, label) in [(rustseis_core::MuteMode::Surgical, "Window"), (rustseis_core::MuteMode::Bottom, "Bottom"), (rustseis_core::MuteMode::Top, "Top")] {
                        if cmd(ui, p, "", label, v.mute_mode == m).clicked() {
                            v.mute_mode = m;
                        }
                    }
                }
                if v.mode == PickMode::FirstBreak && cmd(ui, p, icon::SPARKLE, "Auto-pick shot", false).clicked() {
                    crate::panes::viewer::auto_pick_fb(&mut app.state);
                }
                let v = &mut app.state.viewer;
                cmd_sep(ui, p);
                for (m, ic, label) in [(PickMode::Kill, icon::PROHIBIT, "Edit traces"), (PickMode::FirstBreak, icon::LIGHTNING, "First breaks"), (PickMode::Mute, icon::SCISSORS, "Mute")] {
                    if cmd(ui, p, ic, label, v.mode == m).clicked() {
                        v.mode = m;
                    }
                }
                ctx_label(ui, p, "PICKING");
            } else {
                let r = cmd(ui, p, icon::EXPORT, "Export", false);
                egui::Popup::menu(&r).show(|ui| {
                    if ui.button("Before / after image (PNG)…").clicked() {
                        crate::panes::viewer::export_side_by_side(&mut app.state);
                    }
                });
                for (m, ic, label) in [(PickMode::Kill, icon::PROHIBIT, "Edit traces"), (PickMode::FirstBreak, icon::LIGHTNING, "First breaks"), (PickMode::Mute, icon::SCISSORS, "Pick mute")] {
                    if cmd(ui, p, ic, label, false).clicked() {
                        app.state.viewer.mode = m;
                    }
                }
                cmd_sep(ui, p);
                let v = &mut app.state.viewer;
                if cmd(ui, p, icon::WAVE_SINE, "Wiggles", v.wiggles).clicked() {
                    v.wiggles = !v.wiggles;
                }
                ctx_label(ui, p, "GATHER");
            }
        }
        Doc::Flow => {
            if cmd(ui, p, icon::CODE, "JSON", app.state.inspector.flow_json).on_hover_text("Edit the flow as text").clicked() {
                app.state.inspector.flow_json = !app.state.inspector.flow_json;
            }
            if cmd(ui, p, icon::FLOPPY_DISK, "Save", false).on_hover_text("Ctrl+S").clicked() {
                app.state.save_flow(false);
            }
            let r = cmd(ui, p, icon::FOLDER_OPEN, "Open  ▾", false);
            let flows = crate::panes::project::flow_files(&app.state);
            egui::Popup::menu(&r).show(|ui| {
                ui.set_min_width(260.0);
                for f in &flows {
                    let name = f.file_stem().map(|x| x.to_string_lossy().replace('_', " ")).unwrap_or_default();
                    if ui.button(name).clicked() {
                        app.state.open_flow(f);
                    }
                }
                ui.separator();
                if ui.button("Browse…").clicked() {
                    app.state.open_flow_dialog();
                }
            });
            if cmd(ui, p, icon::FILE_PLUS, "New", false).clicked() {
                app.state.flow = Default::default();
                app.state.flow_sel = None;
                app.state.report_uids.clear();
            }
            ctx_label(ui, p, "FLOW");
        }
        Doc::Velocity => {
            ui.add_enabled_ui(!app.state.is_busy(), |ui| {
                if cmd_primary(ui, p, icon::CHECK, "Save velocities").clicked() {
                    crate::panes::iva::save_table(&mut app.state, false);
                }
            });
        }
        Doc::Spectral => {
            if cmd_primary(ui, p, icon::PLUS, "Add to flow").clicked() {
                crate::panes::spectral::add_active_filter_to_flow(&mut app.state);
                app.open(Doc::Flow);
            }
            if cmd(ui, p, icon::EYE, "Preview Radon", false).clicked() {
                app.state.spectral.request_radon_preview();
            }
            let s = &mut app.state.spectral;
            let mut follow = s.follow_viewer;
            if cmd(ui, p, icon::LINK, "Follow the gather view", follow).clicked() {
                follow = !follow;
            }
            s.follow_viewer = follow;
            ctx_label(ui, p, "SPECTRA");
        }
        Doc::Statics | Doc::Borehole | Doc::Survey | Doc::Start => {}
    }
    let _ = (SeismicColormap::BlueWhiteRed, Level::Info);
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
    let unknown: Vec<String> = spec.steps.iter().filter(|st| s.tool(&st.tool).is_none()).map(|st| st.tool.clone()).collect();
    let has_input = spec.steps.first().is_some_and(|st| matches!(st.tool.as_str(), "segd_read" | "segy_read" | "diskread2" | "synth" | "afd"));
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
