//! Workspace 1 · left: Area → Line → Flows / Datasets / OPF tables, plus the SQLite OPF browser.

use crate::data::{TraceSource, fmt_bytes, fmt_count};
use crate::state::{AppState, JobPurpose, Level, Workspace};
use crate::theme::{font_mono, palette};
use crate::widgets::{self, chip, icon_button, kv, section};
use crate::worker::EngineJob;
use egui::{CollapsingHeader, RichText, ScrollArea, Ui};
use egui_phosphor::regular as icon;
use serde_json::Value;
use std::path::PathBuf;

pub struct ProjectState {
    pub manifest: Option<Value>,
    pub opf_path: Option<PathBuf>,
    pub opf_entries: Option<(PathBuf, Result<Vec<(String, String, String)>, String>)>,
    pub segd_max_shots: usize,
    pub new_project: Option<NewProject>,
    files_cache: Option<(f64, Vec<PathBuf>, Vec<PathBuf>)>,
}

impl Default for ProjectState {
    fn default() -> Self {
        Self { manifest: None, opf_path: None, opf_entries: None, segd_max_shots: 40, new_project: None, files_cache: None }
    }
}

#[derive(Default, Clone)]
pub struct NewProject {
    pub root: String,
    pub area: String,
    pub line: String,
}

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    widgets::toolbar(ui, &p, |ui| {
        ui.menu_button(format!("{}  Open data", icon::FOLDER_OPEN), |ui| {
            if ui.button(format!("{}  SEG-Y / SEG-D / .rsds file…", icon::FILE)).clicked() {
                ui.close();
                s.open_seismic_dialog();
            }
            ui.horizontal(|ui| {
                if ui.button(format!("{}  SEG-D shot directory…", icon::FOLDERS)).clicked() {
                    ui.close();
                    s.open_segd_dir_dialog();
                }
                ui.add(egui::DragValue::new(&mut s.project.segd_max_shots).range(1..=5000).suffix(" shots"));
            });
        });
        if icon_button(ui, &p, icon::FILE_CODE, "Open flow (.json / .toml)", false).clicked() {
            s.open_flow_dialog();
        }
        if icon_button(ui, &p, icon::DATABASE, "Load OPF database (.opf.sqlite)", false).clicked() {
            s.open_opf_dialog();
        }
        if icon_button(ui, &p, icon::FLOPPY_DISK, "Save session tables to OPF database", false).clicked() {
            s.save_opf_dialog();
        }
        if icon_button(ui, &p, icon::PLUS, "New Area / Line project", false).clicked() {
            s.project.new_project = Some(NewProject { area: "NIAS".into(), line: "LINE_01".into(), ..Default::default() });
        }
    });

    new_project_modal(s, ui);

    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        widgets::body(ui, |ui| {
            project_header(s, ui, &p);
            datasets(s, ui, &p);
            tables(s, ui, &p);
            opf_browser(s, ui, &p);
            flow_files(s, ui, &p);
        });
    });
}

fn project_header(s: &mut AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    section(ui, p, "Project", None);
    match &s.project.manifest {
        Some(m) => {
            let area = m.get("area").and_then(|v| v.as_str()).unwrap_or("?");
            let line = m.get("line").and_then(|v| v.as_str()).unwrap_or("?");
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::MOUNTAINS).color(p.accent));
                ui.label(RichText::new(area).strong());
                ui.label(RichText::new("›").color(p.text_faint));
                ui.label(RichText::new(line).strong());
            });
            if let Some(root) = s.session().and_then(|x| x.root_dir.clone()) {
                ui.label(RichText::new(root.display().to_string()).font(font_mono(10.5)).color(p.text_faint));
            }
        }
        None => {
            widgets::hint(ui, p, icon::INFO, "Session-only workspace. Create an Area / Line project to get datasets/, flows/, tables/ and qc/ folders.");
        }
    }
}

fn datasets(s: &mut AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    section(ui, p, "Datasets", Some("click to focus all viewers"));
    let Some(session) = s.session() else {
        widgets::hint(ui, p, icon::HOURGLASS, "Engine busy — dataset list refreshes when the job finishes.");
        return;
    };
    let mut sources: Vec<(TraceSource, usize, usize)> = TraceSource::all(session)
        .into_iter()
        .map(|src| {
            let t = src.traces(session);
            (src, t.len(), crate::data::traces_bytes(t))
        })
        .collect();
    let summary = s.summary.clone();
    let mut clicked: Option<TraceSource> = None;
    for (src, n, bytes) in sources.drain(..) {
        let selected = s.focus.source == src;
        let frame = egui::Frame::new()
            .fill(if selected { crate::theme::mix(p.raised, p.accent, 0.07) } else { p.raised })
            .stroke(egui::Stroke::new(1.0, if selected { crate::theme::with_alpha(p.accent, 150) } else { p.border }))
            .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS_MD))
            .inner_margin(egui::Margin::same(8));
        let resp = frame.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let ic = if src == TraceSource::Active { icon::WAVEFORM } else { icon::STACK };
                ui.label(RichText::new(ic).color(if selected { p.accent } else { p.text_muted }));
                ui.label(RichText::new(src.label()).strong().color(if selected { p.accent } else { p.text }));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    chip(ui, &fmt_bytes(bytes), p.text_muted, p.sunken);
                    chip(ui, &format!("{} tr", fmt_count(n)), p.text_muted, p.sunken);
                });
            });
            if src == TraceSource::Active && n > 0 {
                ui.label(RichText::new(summary.headline()).font(font_mono(11.0)).color(p.text_muted));
                ui.horizontal(|ui| {
                    if let Some((a, b)) = summary.ffid {
                        ui.label(RichText::new(format!("FFID {a}–{b}")).font(font_mono(10.5)).color(p.text_faint));
                    }
                    if let Some((a, b)) = summary.cdp {
                        ui.label(RichText::new(format!("CDP {a}–{b}")).font(font_mono(10.5)).color(p.text_faint));
                    }
                });
            }
        });
        let r = resp.response.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() {
            clicked = Some(src);
        }
        ui.add_space(4.0);
    }
    if let Some(src) = clicked {
        s.focus.source = src;
        s.focus.ens = 0;
        s.viewer.request_refit = true;
    }
}

fn tables(s: &mut AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    section(ui, p, "OPF tables", Some("session context"));
    let Some(session) = s.session() else { return };
    let ctx = &session.context;
    let mut nav: Option<(Workspace, &'static str, String)> = None;
    let mut group = |ui: &mut Ui, title: &str, ic: &str, names: Vec<(String, String)>, ws: Workspace, kind: &'static str| {
        let n = names.len();
        CollapsingHeader::new(RichText::new(format!("{ic}  {title}")).strong())
            .id_salt(("opf", title))
            .default_open(n > 0)
            .show(ui, |ui| {
                if names.is_empty() {
                    ui.label(RichText::new("—").color(p.text_faint));
                }
                for (name, info) in names {
                    ui.horizontal(|ui| {
                        if ui.link(RichText::new(&name).font(font_mono(11.5))).on_hover_text("Open in its workspace").clicked() {
                            nav = Some((ws, kind, name.clone()));
                        }
                        ui.label(RichText::new(info).size(11.0).color(p.text_faint));
                    });
                }
            });
    };
    let mut vel: Vec<(String, String)> = ctx.velocities.iter().map(|(k, v)| (k.clone(), format!("{} fn", v.functions.len()))).collect();
    vel.sort();
    group(ui, "VEL · velocity", icon::CHART_LINE_UP, vel, Workspace::Velocity, "vel");
    let mut mutes: Vec<(String, String)> = ctx.mutes.iter().map(|(k, v)| (k.clone(), format!("{:?} · {} picks", v.mode, v.picks.len().max(v.surgical_picks.len())))).collect();
    mutes.sort();
    group(ui, "MUTE", icon::SCISSORS, mutes, Workspace::Viewer, "mute");
    let mut fb: Vec<(String, String)> = ctx.first_breaks.iter().map(|(k, v)| (k.clone(), format!("{} picks", fmt_count(v.picks.len())))).collect();
    fb.sort();
    group(ui, "FBPICK", icon::LIGHTNING, fb, Workspace::Statics, "fb");
    let mut st: Vec<(String, String)> = ctx.statics.iter().map(|(k, v)| (k.clone(), format!("{} · rms {:.2} ms", v.kind, v.rms_residual_ms))).collect();
    st.sort();
    group(ui, "STAT", icon::RULER, st, Workspace::Statics, "stat");
    let mut sc: Vec<(String, String)> = ctx.sc_amps.iter().map(|(k, v)| (k.clone(), format!("{} src · {} rec", v.source_factors.len(), v.receiver_factors.len()))).collect();
    sc.sort();
    group(ui, "SCAMP", icon::GAUGE, sc, Workspace::Statics, "scamp");
    let mut rd: Vec<(String, String)> = ctx.radon_panels.iter().map(|(k, v)| (k.clone(), format!("{} q × {} t", v.q_values_ms.len(), v.times_ms.len()))).collect();
    rd.sort();
    group(ui, "Radon panels", icon::FUNNEL, rd, Workspace::Spectral, "radon");
    let mut vp: Vec<(String, String)> = ctx.vsp_profiles.iter().map(|(k, v)| (k.clone(), format!("{} depths", v.depths_m.len()))).collect();
    vp.sort();
    group(ui, "VSP profiles", icon::DROP, vp, Workspace::Statics, "vsp");

    if let Some((ws, kind, name)) = nav {
        s.workspace_request = Some(ws);
        match kind {
            "vel" => s.iva.select_table(&name),
            "mute" => s.viewer.mute_table_name = name,
            "fb" => s.statics.fb_table = Some(name),
            "stat" => s.statics.statics_table = Some(name),
            "scamp" => s.statics.scamp_table = Some(name),
            "radon" => s.spectral.radon_source = Some(name),
            "vsp" => s.vsp.profile_name = Some(name),
            _ => {}
        }
    }
}

fn opf_browser(s: &mut AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    section(ui, p, "OPF database", Some("SQLite"));
    let path = s.project.opf_path.clone();
    ui.horizontal(|ui| {
        match &path {
            Some(pth) => {
                ui.label(RichText::new(icon::DATABASE).color(p.accent));
                ui.label(RichText::new(pth.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).font(font_mono(11.5)))
                    .on_hover_text(pth.display().to_string());
            }
            None => {
                ui.label(RichText::new("No database attached").color(p.text_faint));
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.button(format!("{}  Attach…", icon::FOLDER_OPEN)).on_hover_text("Choose a database to browse (does not load it)").clicked() {
            if let Some(pth) = rfd::FileDialog::new().add_filter("OPF SQLite", &["sqlite", "db"]).pick_file() {
                s.project.opf_path = Some(pth);
                s.project.opf_entries = None;
            }
        }
        let has = path.is_some();
        if ui.add_enabled(has && !s.is_busy(), egui::Button::new(format!("{}  Load", icon::DOWNLOAD))).on_hover_text("LoadOpfDatabase into the session").clicked() {
            if let Some(pth) = path.clone() {
                s.load_opf(pth);
            }
        }
        if ui.add_enabled(!s.is_busy(), egui::Button::new(format!("{}  Save", icon::FLOPPY_DISK))).on_hover_text("SaveOpfDatabase from the session").clicked() {
            s.save_opf_dialog();
        }
    });
    if let Some(pth) = &path {
        let stale = s.project.opf_entries.as_ref().is_none_or(|(p0, _)| p0 != pth);
        if stale || ui.small_button(format!("{} refresh", icon::ARROW_CLOCKWISE)).clicked() {
            let entries = rustseis_io::OpfDatabase::open(pth).and_then(|db| db.list_entries()).map_err(|e| e.to_string());
            s.project.opf_entries = Some((pth.clone(), entries));
        }
    }
    if let Some((_, entries)) = &s.project.opf_entries {
        match entries {
            Ok(list) if list.is_empty() => {
                ui.label(RichText::new("Database is empty").color(p.text_faint));
            }
            Ok(list) => {
                egui::Grid::new("opf_entries").striped(true).num_columns(3).spacing([10.0, 3.0]).show(ui, |ui| {
                    for (kind, name, updated) in list.iter().take(200) {
                        ui.label(RichText::new(kind).font(font_mono(10.5)).color(p.accent));
                        ui.label(RichText::new(name).font(font_mono(11.0)));
                        ui.label(RichText::new(updated).size(10.5).color(p.text_faint));
                        ui.end_row();
                    }
                });
            }
            Err(e) => {
                ui.colored_label(p.danger, e);
            }
        }
    }
}

fn flow_files(s: &mut AppState, ui: &mut Ui, p: &crate::theme::Palette) {
    section(ui, p, "Flows & sample data", None);
    let now = s.time;
    let refresh = s.project.files_cache.as_ref().is_none_or(|(t, _, _)| now - *t > 5.0);
    if refresh {
        let mut flows = Vec::new();
        let mut data = Vec::new();
        for dir in &s.flow_dirs {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let path = e.path();
                    if matches!(path.extension().and_then(|x| x.to_str()), Some("json" | "toml")) {
                        flows.push(path);
                    }
                }
            }
            if let Some(out) = dir.parent().map(|d| d.join("out")) {
                if let Ok(rd) = std::fs::read_dir(out) {
                    for e in rd.flatten() {
                        let path = e.path();
                        if matches!(path.extension().and_then(|x| x.to_str()), Some("sgy" | "segy")) {
                            data.push(path);
                        }
                    }
                }
            }
        }
        flows.sort();
        data.sort();
        s.project.files_cache = Some((now, flows, data));
    }
    let Some((_, flows, data)) = s.project.files_cache.clone() else { return };
    let mut open_flow: Option<PathBuf> = None;
    let mut open_data: Option<PathBuf> = None;
    for f in &flows {
        let name = f.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        let current = s.flow.path.as_ref() == Some(f);
        let r = ui.selectable_label(current, RichText::new(format!("{}  {name}", icon::FLOW_ARROW)).size(12.0));
        if r.on_hover_text(f.display().to_string()).clicked() {
            open_flow = Some(f.clone());
        }
    }
    if !data.is_empty() {
        ui.add_space(4.0);
        for d in &data {
            let name = d.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            let size = std::fs::metadata(d).map(|m| m.len() as usize).unwrap_or(0);
            ui.horizontal(|ui| {
                let r = ui.selectable_label(false, RichText::new(format!("{}  {name}", icon::FILE)).size(12.0));
                ui.label(RichText::new(fmt_bytes(size)).size(10.5).color(p.text_faint));
                if r.on_hover_text(format!("Open {}", d.display())).clicked() {
                    open_data = Some(d.clone());
                }
            });
        }
    }
    if let Some(f) = open_flow {
        if s.flow.dirty {
            s.toast(Level::Warn, "Unsaved flow replaced — previous edits discarded");
        }
        s.open_flow(&f);
    }
    if let Some(d) = open_data {
        s.open_seismic(&d, None);
    }
}

fn new_project_modal(s: &mut AppState, ui: &mut Ui) {
    let Some(mut form) = s.project.new_project.clone() else { return };
    let p = *palette(s.theme);
    let mut close = false;
    let mut submit = false;
    egui::Modal::new(egui::Id::unique("new_project")).show(ui.ctx(), |ui| {
        ui.set_width(420.0);
        ui.label(RichText::new("New project").font(crate::theme::font_display(17.0)));
        ui.label(RichText::new("Creates datasets/, flows/, tables/ (with project.opf.sqlite) and qc/ under the root folder.").color(p.text_muted).size(12.0));
        ui.add_space(8.0);
        egui::Grid::new("np_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Root folder");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut form.root).desired_width(250.0));
                if ui.button(icon::FOLDER_OPEN).clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        form.root = d.display().to_string();
                    }
                }
            });
            ui.end_row();
            ui.label("Area");
            ui.text_edit_singleline(&mut form.area);
            ui.end_row();
            ui.label("Line");
            ui.text_edit_singleline(&mut form.line);
            ui.end_row();
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let ok = !form.root.trim().is_empty() && !form.area.trim().is_empty() && !form.line.trim().is_empty();
            ui.add_enabled_ui(ok && !s.is_busy(), |ui| {
                if widgets::primary_button(ui, &p, icon::CHECK, "Create").clicked() {
                    submit = true;
                }
            });
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    if submit {
        let job = EngineJob::InitProject { root: PathBuf::from(form.root.trim()), area: form.area.trim().into(), line: form.line.trim().into() };
        s.submit(job, JobPurpose::Project);
        s.project.new_project = None;
    } else if close {
        s.project.new_project = None;
    } else {
        s.project.new_project = Some(form);
    }
}

pub fn kv_grid(ui: &mut Ui, p: &crate::theme::Palette, id: &str, rows: &[(&str, String)]) {
    egui::Grid::new(id).num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
        for (k, v) in rows {
            kv(ui, p, k, v.clone());
        }
    });
}
