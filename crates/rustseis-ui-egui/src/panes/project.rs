//! Left panel: Explorer (project, data with thumbnails, flows, sample data) and Tables (session
//! tables and the project database).

use crate::data::{self, TraceSource, fmt_bytes, fmt_count};
use crate::state::{AppState, Doc, JobPurpose, Level};
use crate::theme::{Palette, font_mono, mix, palette, with_alpha};
use crate::widgets::{self, caps, cmd, cmd_primary, mono};
use crate::worker::EngineJob;
use egui::{CornerRadius, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct ProjectState {
    pub manifest: Option<Value>,
    pub opf_path: Option<PathBuf>,
    pub opf_entries: Option<(PathBuf, Result<Vec<(String, String, String)>, String>)>,
    pub segd_max_shots: usize,
    pub new_project: Option<NewProject>,
    files_cache: Option<(f64, Vec<PathBuf>, Vec<PathBuf>)>,
    thumbs: HashMap<String, (u64, egui::TextureHandle)>,
}

impl Default for ProjectState {
    fn default() -> Self {
        Self { manifest: None, opf_path: None, opf_entries: None, segd_max_shots: 40, new_project: None, files_cache: None, thumbs: HashMap::new() }
    }
}

#[derive(Default, Clone)]
pub struct NewProject {
    pub root: String,
    pub area: String,
    pub line: String,
}

fn pad(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.set_min_width(ui.available_width());
        add(ui);
    });
}

/// Rescan flow and sample-data folders every few seconds.
pub fn refresh_files(s: &mut AppState) {
    let now = s.time;
    if s.project.files_cache.as_ref().is_some_and(|(t, _, _)| now - *t < 5.0) {
        return;
    }
    let mut flows = Vec::new();
    let mut samples = Vec::new();
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
                        samples.push(path);
                    }
                }
            }
        }
    }
    flows.sort();
    flows.dedup();
    samples.sort();
    samples.dedup();
    s.project.files_cache = Some((now, flows, samples));
}

pub fn flow_files(s: &AppState) -> Vec<PathBuf> {
    s.project.files_cache.as_ref().map(|c| c.1.clone()).unwrap_or_default()
}

pub fn sample_files(s: &AppState) -> Vec<PathBuf> {
    s.project.files_cache.as_ref().map(|c| c.2.clone()).unwrap_or_default()
}

/// Friendly name for a flow file: `ui_qc_prestack_demo.json` → "Ui qc prestack demo".
pub fn flow_title(path: &std::path::Path) -> String {
    let stem = path.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    let s = stem.replace(['_', '-'], " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

/// Small cached preview of the first ensemble of a source.
pub fn thumbnail(s: &mut AppState, ctx: &egui::Context, src: &TraceSource) -> Option<egui::TextureId> {
    let name = format!("{src:?}");
    let key = {
        let sess = s.session()?;
        let t = src.traces(sess);
        if t.is_empty() {
            return None;
        }
        crate::data::hash_parts(&(s.rev, t.len(), &name))
    };
    if let Some((k, tex)) = s.project.thumbs.get(&name) {
        if *k == key {
            return Some(tex.id());
        }
    }
    let img = {
        let sess = s.session()?;
        let t = src.traces(sess);
        let key_name = data::suggest_key(t);
        let end = if key_name == data::SECTION_KEY {
            t.len()
        } else {
            let g = data::header_getter(key_name);
            let first = g(&t[0]);
            t.iter().position(|x| g(x) != first).unwrap_or(t.len())
        };
        let sub = data::decimated_clone(t, 0, end, 120);
        data::render_gather_raster(&sub, rustseis_render::SeismicColormap::Greyscale, 97.0, 160).ok()?
    };
    let tex = ctx.load_texture(format!("thumb_{name}"), img, egui::TextureOptions::LINEAR);
    let id = tex.id();
    s.project.thumbs.insert(name, (key, tex));
    Some(id)
}

pub fn explorer_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    refresh_files(s);
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        pad(ui, |ui| {
            project_header(s, ui, &p);
            ui.add_space(6.0);
            datasets(s, ui, &p);
            ui.add_space(6.0);
            flows(s, ui, &p);
            ui.add_space(6.0);
            samples(s, ui, &p);
        });
    });
}

fn project_header(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    caps(ui, p, "PROJECT", None);
    match &s.project.manifest {
        Some(m) => {
            let area = m.get("area").and_then(|v| v.as_str()).unwrap_or("?").to_string();
            let line = m.get("line").and_then(|v| v.as_str()).unwrap_or("?").to_string();
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::MOUNTAINS).color(p.accent));
                ui.label(RichText::new(area).font(crate::theme::font_semibold(13.0)).color(p.text));
                ui.label(RichText::new("›").color(p.text_faint));
                ui.label(RichText::new(line).font(crate::theme::font_semibold(13.0)).color(p.text));
            });
            if let Some(root) = s.session().and_then(|x| x.root_dir.clone()) {
                ui.label(RichText::new(root.display().to_string()).font(font_mono(10.5)).color(p.text_faint));
            }
        }
        None => {
            ui.label(RichText::new("Working without a project. Results stay in this session until you save them.").size(11.5).color(p.text_muted));
            if cmd(ui, p, icon::PLUS, "Create a project", false).clicked() {
                s.project.new_project = Some(NewProject { area: "Survey".into(), line: "Line 01".into(), ..Default::default() });
            }
        }
    }
}

fn datasets(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    caps(ui, p, "DATA", None);
    let Some(session) = s.session() else {
        ui.label(RichText::new("Working… the list refreshes when the job finishes.").size(11.5).color(p.text_muted));
        return;
    };
    let list: Vec<(TraceSource, usize, usize)> = TraceSource::all(session)
        .into_iter()
        .map(|src| {
            let t = src.traces(session);
            (src, t.len(), crate::data::traces_bytes(t))
        })
        .filter(|(_, n, _)| *n > 0)
        .collect();
    if list.is_empty() {
        ui.label(RichText::new("Nothing loaded yet.").size(11.5).color(p.text_muted));
        let r = cmd(ui, p, icon::FOLDER_OPEN, "Open data…", false);
        if r.clicked() {
            s.open_seismic_dialog();
        }
        return;
    }
    let summary = s.summary.clone();
    let mut clicked: Option<TraceSource> = None;
    let ctx = ui.ctx().clone();
    for (src, n, bytes) in list {
        let selected = s.focus.source == src;
        let thumb = thumbnail(s, &ctx, &src);
        let fill = if selected { mix(p.raised, p.accent, 0.08) } else { p.raised };
        let stroke = Stroke::new(1.0, if selected { p.accent_line } else { p.border });
        let resp = egui::Frame::new().fill(fill).stroke(stroke).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(54.0, 40.0), Sense::hover());
                match thumb {
                    Some(id) => {
                        ui.painter().image(id, r, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
                    }
                    None => {
                        ui.painter().rect_filled(r, CornerRadius::same(6), p.sunken);
                    }
                }
                ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, with_alpha(p.text, 20)), egui::StrokeKind::Inside);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(src.label()).font(crate::theme::font_semibold(12.5)).color(if selected { p.accent_text } else { p.text }));
                    ui.label(RichText::new(format!("{} traces · {}", fmt_count(n), fmt_bytes(bytes))).size(11.0).color(p.text_muted));
                    if src == TraceSource::Active {
                        if let Some((a, b)) = summary.ffid {
                            ui.label(RichText::new(format!("shots {a}–{b}")).size(10.5).color(p.text_faint));
                        }
                    }
                });
            });
        });
        let r = resp.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() {
            clicked = Some(src);
        }
    }
    if let Some(src) = clicked {
        if let Some(sess) = s.session() {
            s.focus.key = data::suggest_key(src.traces(sess)).to_string();
        }
        s.focus.source = src;
        s.focus.ens = 0;
        s.viewer.request_refit = true;
        s.doc_request = Some(Doc::Viewer);
    }
}

fn flows(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let list = flow_files(s);
    caps(ui, p, "FLOWS", Some(&list.len().to_string()));
    let mut open: Option<PathBuf> = None;
    for f in &list {
        let current = s.flow.path.as_ref() == Some(f);
        let r = list_row(ui, p, icon::FLOW_ARROW, &flow_title(f), "", current);
        if r.on_hover_text(f.display().to_string()).clicked() {
            open = Some(f.clone());
        }
    }
    if cmd(ui, p, icon::FOLDER_OPEN, "Open a flow file…", false).clicked() {
        s.open_flow_dialog();
    }
    if let Some(f) = open {
        if s.flow.dirty {
            s.toast(Level::Warn, "Unsaved changes to the previous flow were discarded");
        }
        s.open_flow(&f);
    }
}

fn samples(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let list = sample_files(s);
    if list.is_empty() {
        return;
    }
    caps(ui, p, "SAMPLE DATA", None);
    let mut open: Option<PathBuf> = None;
    for d in &list {
        let name = d.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        let size = std::fs::metadata(d).map(|m| m.len() as usize).unwrap_or(0);
        if list_row(ui, p, icon::FILE, &name, &fmt_bytes(size), false).on_hover_text(format!("Open {}", d.display())).clicked() {
            open = Some(d.clone());
        }
    }
    if let Some(d) = open {
        s.open_seismic(&d, None);
    }
}

/// Full-width clickable list row: icon, label, trailing detail.
pub fn list_row(ui: &mut Ui, p: &Palette, ic: &str, label: &str, detail: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let fill = if selected { p.accent_soft } else if resp.hovered() { p.raised } else { egui::Color32::TRANSPARENT };
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    let fg = if selected { p.accent_text } else { p.text };
    ui.painter().text(pos2(rect.left() + 10.0, rect.center().y), egui::Align2::LEFT_CENTER, ic, egui::FontId::proportional(14.0), if selected { p.accent } else { p.text_muted });
    let clip = egui::Rect::from_min_max(pos2(rect.left() + 32.0, rect.top()), pos2(rect.right() - 60.0, rect.bottom()));
    ui.painter().with_clip_rect(clip).text(pos2(rect.left() + 32.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(12.5), fg);
    if !detail.is_empty() {
        ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, detail, egui::FontId::proportional(10.5), p.text_faint);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn tables_ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        pad(ui, |ui| {
            tables(s, ui, &p);
            ui.add_space(8.0);
            database(s, ui, &p);
        });
    });
}

fn tables(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let Some(session) = s.session() else {
        ui.label(RichText::new("Working… tables refresh when the job finishes.").size(11.5).color(p.text_muted));
        return;
    };
    let ctx = &session.context;
    let mut groups: Vec<(&str, &str, Doc, &'static str, Vec<(String, String)>)> = Vec::new();
    let sorted = |mut v: Vec<(String, String)>| {
        v.sort();
        v
    };
    groups.push(("Velocities", icon::CHART_LINE_UP, Doc::Velocity, "vel", sorted(ctx.velocities.iter().map(|(k, v)| (k.clone(), format!("{} locations", v.functions.len()))).collect())));
    groups.push(("Mutes", icon::SCISSORS, Doc::Viewer, "mute", sorted(ctx.mutes.iter().map(|(k, v)| (k.clone(), format!("{} points", v.picks.len().max(v.surgical_picks.len())))).collect())));
    groups.push(("First breaks", icon::LIGHTNING, Doc::Statics, "fb", sorted(ctx.first_breaks.iter().map(|(k, v)| (k.clone(), format!("{} picks", fmt_count(v.picks.len())))).collect())));
    groups.push(("Statics", icon::RULER, Doc::Statics, "stat", sorted(ctx.statics.iter().map(|(k, v)| (k.clone(), format!("misfit {:.2} ms", v.rms_residual_ms))).collect())));
    groups.push(("Amplitude terms", icon::GAUGE, Doc::Statics, "scamp", sorted(ctx.sc_amps.iter().map(|(k, v)| (k.clone(), format!("{} shots · {} receivers", v.source_factors.len(), v.receiver_factors.len()))).collect())));
    groups.push(("Radon panels", icon::FUNNEL, Doc::Spectral, "radon", sorted(ctx.radon_panels.iter().map(|(k, v)| (k.clone(), format!("{} curvatures", v.q_values_ms.len()))).collect())));
    groups.push(("Borehole profiles", icon::DROP, Doc::Borehole, "vsp", sorted(ctx.vsp_profiles.iter().map(|(k, v)| (k.clone(), format!("{} depths", v.depths_m.len()))).collect())));
    let total: usize = groups.iter().map(|g| g.4.len()).sum();
    caps(ui, p, "IN THIS SESSION", Some(&total.to_string()));
    if total == 0 {
        ui.label(RichText::new("Velocities, mutes, picks and other tables appear here once a flow or a picking tool creates them.").size(11.5).color(p.text_muted));
    }
    let mut nav: Option<(Doc, &'static str, String)> = None;
    for (title, ic, doc, kind, names) in groups {
        if names.is_empty() {
            continue;
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(ic).color(p.text_muted));
            ui.label(RichText::new(title).font(crate::theme::font_semibold(12.0)).color(p.text));
            ui.label(RichText::new(names.len().to_string()).size(11.0).color(p.text_faint));
        });
        for (name, info) in names {
            if list_row(ui, p, "", &name, &info, false).on_hover_text("Open where it is used").clicked() {
                nav = Some((doc, kind, name));
            }
        }
    }
    if let Some((doc, kind, name)) = nav {
        s.doc_request = Some(doc);
        match kind {
            "vel" => s.iva.select_table(&name),
            "mute" => {
                if let Some(t) = s.session().and_then(|x| x.context.mutes.get(&name)).cloned() {
                    s.viewer.load_mute(&t);
                }
            }
            "fb" => s.statics.fb_table = Some(name),
            "stat" => s.statics.statics_table = Some(name),
            "scamp" => s.statics.scamp_table = Some(name),
            "radon" => s.spectral.radon_source = Some(name),
            "vsp" => s.vsp.profile_name = Some(name),
            _ => {}
        }
    }
}

fn database(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    caps(ui, p, "PROJECT DATABASE", None);
    let path = s.project.opf_path.clone();
    match &path {
        Some(pth) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::DATABASE).color(p.accent));
                ui.label(mono(pth.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), 11.5, p.text)).on_hover_text(pth.display().to_string());
            });
        }
        None => {
            ui.label(RichText::new("Save the session's tables into one file you can reopen later.").size(11.5).color(p.text_muted));
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(!s.is_busy(), |ui| {
            if cmd_primary(ui, p, icon::FLOPPY_DISK, "Save tables").clicked() {
                s.save_opf_dialog();
            }
        });
        if cmd(ui, p, icon::FOLDER_OPEN, "Choose…", false).on_hover_text("Choose a database to look inside").clicked() {
            if let Some(pth) = rfd::FileDialog::new().add_filter("Project database", &["sqlite", "db"]).pick_file() {
                s.project.opf_path = Some(pth);
                s.project.opf_entries = None;
            }
        }
        if path.is_some() && cmd(ui, p, icon::DOWNLOAD, "Load", false).on_hover_text("Bring its tables into the session").clicked() {
            if let Some(pth) = path.clone() {
                s.load_opf(pth);
            }
        }
    });
    if let Some(pth) = &path {
        if s.project.opf_entries.as_ref().is_none_or(|(p0, _)| p0 != pth) {
            let entries = rustseis_io::OpfDatabase::open(pth).and_then(|db| db.list_entries()).map_err(|e| e.to_string());
            s.project.opf_entries = Some((pth.clone(), entries));
        }
    }
    if let Some((_, entries)) = &s.project.opf_entries {
        match entries {
            Ok(list) if list.is_empty() => {
                ui.label(RichText::new("The database is empty").size(11.5).color(p.text_faint));
            }
            Ok(list) => {
                egui::Frame::new().fill(p.sunken).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::Grid::new("opf_entries").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
                        for (kind, name, _updated) in list.iter().take(200) {
                            ui.label(RichText::new(kind.to_lowercase()).size(10.5).color(p.text_faint));
                            ui.label(mono(name, 11.0, p.text));
                            ui.end_row();
                        }
                    });
                });
            }
            Err(e) => {
                ui.colored_label(p.danger, e);
            }
        }
    }
}

pub fn new_project_modal(s: &mut AppState, ctx: &egui::Context) {
    let Some(mut form) = s.project.new_project.clone() else { return };
    let p = *palette(s.theme);
    let mut close = false;
    let mut submit = false;
    egui::Modal::new(egui::Id::unique("new_project")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(RichText::new("New project").font(crate::theme::font_display(20.0)).color(p.text));
        ui.label(RichText::new("A project keeps data, flows, tables and QC images for one line together in a folder.").color(p.text_muted).size(12.0));
        ui.add_space(6.0);
        egui::Grid::new("np_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Folder");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut form.root).desired_width(260.0).hint_text("Where to create it"));
                if ui.button(icon::FOLDER_OPEN).clicked() {
                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                        form.root = d.display().to_string();
                    }
                }
            });
            ui.end_row();
            ui.label("Survey");
            ui.add(egui::TextEdit::singleline(&mut form.area).desired_width(260.0));
            ui.end_row();
            ui.label("Line");
            ui.add(egui::TextEdit::singleline(&mut form.line).desired_width(260.0));
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ok = !form.root.trim().is_empty() && !form.area.trim().is_empty() && !form.line.trim().is_empty();
            ui.add_enabled_ui(ok && !s.is_busy(), |ui| {
                if cmd_primary(ui, &p, icon::CHECK, "Create project").clicked() {
                    submit = true;
                }
            });
            if cmd(ui, &p, "", "Cancel", false).clicked() {
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

pub fn kv_grid(ui: &mut Ui, p: &Palette, id: &str, rows: &[(&str, String)]) {
    egui::Grid::new(id).num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
        for (k, v) in rows {
            widgets::kv(ui, p, k, v.clone());
        }
    });
}
