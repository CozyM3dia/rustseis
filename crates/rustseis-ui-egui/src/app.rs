//! Application shell (interface v2): top bar with workflow stages, command bar, floating
//! Explorer / document / Inspector panels, status line, toasts and the command palette.

use crate::data::{fmt_bytes, fmt_ms};
use crate::state::{AppState, Doc, Level, StageStatus, Workspace};
use crate::theme::{self, Palette, ThemeMode, font_mono, font_semibold, mix, palette, space, with_alpha};
use crate::widgets::{self, panel_frame};
use egui::{Align2, CornerRadius, Frame, Margin, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftTab {
    Explorer,
    Library,
    Tables,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightMode {
    Inspector,
    Assistant,
}

pub struct RustSeisApp {
    pub state: AppState,
    pub stage: Workspace,
    pub docs: Vec<Doc>,
    pub active: Doc,
    pub left_tab: LeftTab,
    pub left_open: bool,
    pub right_mode: RightMode,
    pub jobs_open: bool,
    applied_theme: Option<ThemeMode>,
    palette_open: bool,
    palette_query: String,
    palette_sel: usize,
    frame_ms: VecDeque<f32>,
    pub tour: Option<crate::tour::Tour>,
}

impl RustSeisApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_context(&cc.egui_ctx, true)
    }

    /// Construct against any egui context (headless tests pass `install_fonts = false`).
    pub fn with_context(ctx: &egui::Context, install_fonts: bool) -> Self {
        theme::install(ctx, ThemeMode::Dark, install_fonts);
        egui_extras::install_image_loaders(ctx);
        Self {
            state: AppState::new(ctx),
            stage: Workspace::Data,
            docs: vec![Doc::Start],
            active: Doc::Start,
            left_tab: LeftTab::Explorer,
            left_open: true,
            right_mode: RightMode::Inspector,
            jobs_open: false,
            applied_theme: Some(ThemeMode::Dark),
            palette_open: false,
            palette_query: String::new(),
            palette_sel: 0,
            frame_ms: VecDeque::with_capacity(120),
            tour: None,
        }
    }

    /// Open a file passed on the command line (seismic data, a flow, or a project database).
    pub fn open_path(&mut self, path: &std::path::Path) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "json" | "toml" => self.state.open_flow(path),
            "sqlite" | "db" => self.state.load_opf(path.to_path_buf()),
            _ => self.state.open_seismic(path, None),
        }
    }

    /// Show a document (opening its tab if needed) and move the stage bar to match.
    pub fn open(&mut self, doc: Doc) {
        if !self.docs.contains(&doc) {
            self.docs.push(doc);
        }
        self.active = doc;
        let keep_stage = matches!((self.stage, doc), (Workspace::Imaging, Doc::Viewer) | (Workspace::Statics, Doc::Viewer) | (Workspace::Process, Doc::Viewer));
        if !keep_stage {
            self.stage = doc.stage();
        }
    }

    pub fn set_stage(&mut self, w: Workspace) {
        self.stage = w;
        let d = w.doc();
        if !self.docs.contains(&d) {
            self.docs.push(d);
        }
        self.active = d;
    }

    fn close_doc(&mut self, doc: Doc) {
        if let Some(i) = self.docs.iter().position(|d| *d == doc) {
            self.docs.remove(i);
            if self.docs.is_empty() {
                self.docs.push(Doc::Start);
            }
            if self.active == doc {
                self.active = self.docs.get(i.saturating_sub(1)).copied().unwrap_or(Doc::Start);
            }
        }
    }

    fn route_compute(&mut self, ctx: &egui::Context) {
        for d in self.state.compute.drain() {
            match d.result {
                Ok(v) => {
                    let key = d.key;
                    if key.starts_with("viewer.") {
                        crate::panes::viewer::on_compute(&mut self.state, ctx, key, v);
                    } else if key.starts_with("iva.") {
                        crate::panes::iva::on_compute(&mut self.state, ctx, key, v);
                    } else if key.starts_with("spec.") {
                        crate::panes::spectral::on_compute(&mut self.state, ctx, key, v);
                    } else if key.starts_with("vsp.") {
                        crate::panes::vsp::on_compute(&mut self.state, ctx, key, v);
                    }
                }
                Err(e) => self.state.log(Level::Warn, format!("{} · {e}", d.key)),
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: egui::Key| ctx.input(|i| i.key_pressed(k));
        if cmd {
            let keys = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num6, egui::Key::Num7];
            for (i, k) in keys.iter().enumerate() {
                if pressed(*k) {
                    self.set_stage(Workspace::ALL[i]);
                }
            }
            if pressed(egui::Key::K) {
                self.palette_open = !self.palette_open;
                self.palette_query.clear();
                self.palette_sel = 0;
            }
            if pressed(egui::Key::Enter) {
                self.state.run_current_flow();
            }
            if pressed(egui::Key::S) {
                self.state.save_flow(shift);
            }
            if pressed(egui::Key::O) {
                self.state.open_seismic_dialog();
            }
            if pressed(egui::Key::B) {
                self.left_open = !self.left_open;
            }
            if pressed(egui::Key::J) {
                self.jobs_open = !self.jobs_open;
            }
            if !typing && pressed(egui::Key::Z) {
                if shift {
                    if self.active == Doc::Flow { self.state.redo_flow() } else { self.state.redo() }
                } else if self.active == Doc::Flow {
                    self.state.undo_flow()
                } else {
                    self.state.undo()
                }
            }
            if !typing && pressed(egui::Key::Y) {
                if self.active == Doc::Flow { self.state.redo_flow() } else { self.state.redo() }
            }
        } else if !typing {
            if pressed(egui::Key::F12) {
                let dir = std::env::current_dir().unwrap_or_default().join("screenshots");
                let stamp = crate::state::chrono_like::Stamp::now().0;
                let path = dir.join(format!("rustseis_{}_{stamp}.png", self.active.title().replace(' ', "_")));
                let p2 = path.clone();
                ctx.request_screenshot(move |img| {
                    let _ = crate::tour::save_png(&img, &p2);
                });
                self.state.toast(Level::Ok, format!("Screenshot saved to {}", path.display()));
            }
            if pressed(egui::Key::OpenBracket) {
                self.state.step_ensemble(-1);
            }
            if pressed(egui::Key::CloseBracket) {
                self.state.step_ensemble(1);
            }
            if pressed(egui::Key::Escape) && self.state.viewer.mode != crate::panes::viewer::PickMode::Navigate {
                self.state.viewer.mode = crate::panes::viewer::PickMode::Navigate;
            }
        }
    }

    // ------------------------------------------------------------------ top bar

    fn top_bar(&mut self, ui: &mut Ui, p: &Palette) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.add_space(4.0);
            widgets::logo(ui, p, 24.0);
            ui.label(RichText::new("RustSeis").font(font_semibold(15.0)).color(p.text));
            self.project_chip(ui, p);

            let pills_w = stage_pills_width(ui, p);
            let right_w = 470.0;
            let free = ui.available_width() - right_w;
            ui.add_space(((free - pills_w) * 0.5).max(8.0));
            self.stage_pills(ui, p);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(4.0);
                let assist = self.right_mode == RightMode::Assistant;
                if top_icon(ui, p, icon::SPARKLE, "Assistant", assist).clicked() {
                    self.right_mode = if assist { RightMode::Inspector } else { RightMode::Assistant };
                }
                let theme_icon = if self.state.theme == ThemeMode::Dark { icon::SUN } else { icon::MOON };
                if top_icon(ui, p, theme_icon, "Light or dark", false).clicked() {
                    self.state.theme = self.state.theme.toggled();
                }
                if self.run_pill(ui, p).clicked() {
                    self.jobs_open = !self.jobs_open;
                }
                if search_box(ui, p).clicked() {
                    self.palette_open = true;
                    self.palette_query.clear();
                    self.palette_sel = 0;
                }
            });
        });
    }

    fn project_chip(&mut self, ui: &mut Ui, p: &Palette) {
        let (area, line) = match &self.state.project.manifest {
            Some(m) => (
                m.get("area").and_then(|v| v.as_str()).unwrap_or("Project").to_string(),
                m.get("line").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            ),
            None => ("Untitled project".to_string(), if self.state.summary.n_traces > 0 { self.state.focus.source.label() } else { String::new() }),
        };
        let text = if line.is_empty() { format!("{}  {area}  {}", icon::STACK, icon::CARET_DOWN) } else { format!("{}  {area}  /  {line}  {}", icon::STACK, icon::CARET_DOWN) };
        let btn = egui::Button::new(RichText::new(text).size(12.5).color(p.text_muted))
            .fill(p.panel)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 30.0));
        let resp = ui.add(btn);
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(240.0);
            if ui.button(format!("{}  Start page", icon::HOUSE)).clicked() {
                self.open(Doc::Start);
            }
            ui.separator();
            if ui.button(format!("{}  Open seismic data…", icon::FOLDER_OPEN)).clicked() {
                self.state.open_seismic_dialog();
            }
            if ui.button(format!("{}  Open SEG-D shot folder…", icon::FOLDERS)).clicked() {
                self.state.open_segd_dir_dialog();
            }
            if ui.button(format!("{}  Open flow…", icon::FLOW_ARROW)).clicked() {
                self.state.open_flow_dialog();
            }
            ui.separator();
            if ui.button(format!("{}  New project…", icon::PLUS)).clicked() {
                self.state.project.new_project = Some(crate::panes::project::NewProject { area: "Survey".into(), line: "Line 01".into(), ..Default::default() });
            }
            if ui.button(format!("{}  Open project database…", icon::DATABASE)).clicked() {
                self.state.open_opf_dialog();
            }
            if ui.button(format!("{}  Save project database", icon::FLOPPY_DISK)).clicked() {
                self.state.save_opf_dialog();
            }
        });
    }

    fn stage_pills(&mut self, ui: &mut Ui, p: &Palette) {
        let font = egui::FontId::proportional(13.0);
        let widths: Vec<f32> = Workspace::ALL.iter().map(|w| ui.painter().layout_no_wrap(w.title().to_string(), font.clone(), p.text).size().x + 34.0).collect();
        let total = vec2(widths.iter().sum::<f32>() + 8.0, 34.0);
        let (outer, _) = ui.allocate_exact_size(total, Sense::hover());
        ui.painter().rect(outer, CornerRadius::same(10), p.panel, Stroke::new(1.0, p.border), StrokeKind::Inside);
        let mut x = outer.left() + 4.0;
        for (i, (w, wd)) in Workspace::ALL.iter().zip(widths).enumerate() {
            let r = egui::Rect::from_min_size(pos2(x, outer.top() + 3.0), vec2(wd, outer.height() - 6.0));
            let resp = ui.interact(r, egui::Id::unique(("stage", i)), Sense::click()).on_hover_text(format!("{} — {} (Ctrl+{})", w.title(), w.hint(), i + 1));
            let on = self.stage == *w;
            if on {
                ui.painter().rect(r, CornerRadius::same(7), p.raised, Stroke::new(1.0, p.border_strong), StrokeKind::Inside);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(7), with_alpha(p.raised, 160));
            }
            let dot = match self.state.stage_status(*w) {
                StageStatus::Done => p.success,
                StageStatus::Attention => p.pick,
                StageStatus::NotStarted => p.border_strong,
            };
            ui.painter().circle_filled(pos2(r.left() + 14.0, r.center().y), 3.2, dot);
            let col = if on || resp.hovered() { p.text } else { p.text_muted };
            ui.painter().text(pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, w.title(), font.clone(), col);
            if resp.clicked() {
                self.set_stage(*w);
            }
            x += wd;
        }
    }

    fn run_pill(&self, ui: &mut Ui, p: &Palette) -> egui::Response {
        let (text, dot, busy) = match self.state.busy_info() {
            Some((label, el, eta)) => {
                let t = match eta {
                    Some(e) => format!("{label} · {} of ~{}", fmt_ms(el.as_secs_f64() * 1000.0), fmt_ms(e as f64)),
                    None => format!("{label} · {}", fmt_ms(el.as_secs_f64() * 1000.0)),
                };
                (t, p.accent, true)
            }
            None => match self.state.history.last() {
                Some(j) if !j.ok => ("Last job failed".to_string(), p.danger, false),
                Some(j) => (format!("Ready · {}", fmt_ms(j.elapsed.as_secs_f64() * 1000.0)), p.success, false),
                None => ("Ready".to_string(), p.success, false),
            },
        };
        let font = egui::FontId::proportional(12.0);
        let g = ui.painter().layout_no_wrap(text, font, p.text_muted);
        let w = (g.size().x + 32.0).min(300.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
        let fill = if busy { p.accent_soft } else { p.panel };
        let stroke = if busy { p.accent_line } else { p.border };
        ui.painter().rect(rect, CornerRadius::same(8), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        let c = rect.left_center() + vec2(13.0, 0.0);
        if busy {
            let t = ui.input(|i| i.time) as f32;
            ui.painter().circle_filled(c, 3.0 + (t * 4.0).sin().abs() * 1.3, dot);
            if let Some((_, el, Some(eta))) = self.state.busy_info() {
                let f = (el.as_secs_f64() * 1000.0 / eta.max(1) as f64).min(0.97) as f32;
                let mut bar = egui::Rect::from_min_max(pos2(rect.left() + 8.0, rect.bottom() - 3.0), pos2(rect.right() - 8.0, rect.bottom() - 1.5));
                ui.painter().rect_filled(bar, 1.0, with_alpha(p.accent, 50));
                bar.set_width(bar.width() * f);
                ui.painter().rect_filled(bar, 1.0, p.accent);
            }
            ui.ctx().request_repaint();
        } else {
            ui.painter().circle_filled(c, 3.5, dot);
        }
        let clip = egui::Rect::from_min_max(pos2(rect.left() + 22.0, rect.top()), pos2(rect.right() - 8.0, rect.bottom()));
        ui.painter().with_clip_rect(clip).galley(pos2(rect.left() + 23.0, rect.center().y - g.size().y * 0.5), g, p.text_muted);
        resp.on_hover_text("Engine status · click for the job list (Ctrl+J)")
    }

    // ------------------------------------------------------------------ panels

    fn left_panel(&mut self, ui: &mut Ui, p: &Palette) {
        let slot = ui.available_rect_before_wrap();
        ui.allocate_rect(slot, Sense::hover());
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(slot).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.set_clip_rect(slot.intersect(ui.clip_rect()));
        let ui = &mut ui;
        panel_frame(p).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.set_min_size(ui.available_size());
            ui.horizontal(|ui| {
                ui.set_height(40.0);
                ui.add_space(8.0);
                for (t, label) in [(LeftTab::Explorer, "Explorer"), (LeftTab::Library, "Library"), (LeftTab::Tables, "Tables")] {
                    if tab_pill(ui, p, label, self.left_tab == t).clicked() {
                        self.left_tab = t;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    if top_icon(ui, p, icon::SIDEBAR_SIMPLE, "Hide panel (Ctrl+B)", false).clicked() {
                        self.left_open = false;
                    }
                });
            });
            hline(ui, p);
            match self.left_tab {
                LeftTab::Explorer => crate::panes::project::explorer_ui(&mut self.state, ui),
                LeftTab::Library => crate::panes::catalog::ui(&mut self.state, ui),
                LeftTab::Tables => crate::panes::project::tables_ui(&mut self.state, ui),
            }
        });
    }

    fn rail(&mut self, ui: &mut Ui, p: &Palette) {
        panel_frame(p).inner_margin(Margin::symmetric(6, 8)).show(ui, |ui| {
            ui.set_min_height(ui.available_height());
            ui.vertical_centered(|ui| {
                for (t, ic, tip) in [(LeftTab::Explorer, icon::STACK, "Explorer"), (LeftTab::Library, icon::SQUARES_FOUR, "Library"), (LeftTab::Tables, icon::TABLE, "Tables")] {
                    if top_icon(ui, p, ic, tip, false).clicked() {
                        self.left_tab = t;
                        self.left_open = true;
                    }
                }
            });
        });
    }

    fn center_panel(&mut self, ui: &mut Ui, p: &Palette) {
        panel_frame(p).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let mut close: Option<Doc> = None;
            let mut pick: Option<Doc> = None;
            ui.horizontal(|ui| {
                ui.set_height(38.0);
                ui.add_space(6.0);
                if !self.left_open && top_icon(ui, p, icon::SIDEBAR_SIMPLE, "Show panel (Ctrl+B)", false).clicked() {
                    self.left_open = true;
                }
                for d in self.docs.clone() {
                    let (r, x) = doc_pill(ui, p, d, self.active == d, self.docs.len() > 1);
                    if x {
                        close = Some(d);
                    } else if r.clicked() {
                        pick = Some(d);
                    }
                }
                ui.menu_button(RichText::new(icon::PLUS).color(p.text_faint), |ui| {
                    for d in [Doc::Viewer, Doc::Flow, Doc::Velocity, Doc::Spectral, Doc::Statics, Doc::Borehole, Doc::Survey, Doc::Start] {
                        if ui.button(format!("{}  {}", d.icon(), d.title())).clicked() {
                            pick = Some(d);
                        }
                    }
                });
            });
            if let Some(d) = close {
                self.close_doc(d);
            }
            if let Some(d) = pick {
                self.open(d);
            }
            hline(ui, p);
            let jobs_h = if self.jobs_open { 230.0 } else { 0.0 };
            let avail = ui.available_size();
            let doc_rect = egui::Rect::from_min_size(ui.cursor().min, vec2(avail.x, (avail.y - jobs_h).max(80.0)));
            let mut doc_ui = ui.new_child(egui::UiBuilder::new().max_rect(doc_rect).layout(egui::Layout::top_down(egui::Align::Min)));
            doc_ui.set_clip_rect(doc_rect);
            self.doc_content(&mut doc_ui);
            if self.jobs_open {
                let jr = egui::Rect::from_min_max(pos2(doc_rect.left(), doc_rect.bottom()), pos2(doc_rect.right(), doc_rect.bottom() + jobs_h));
                ui.painter().hline(jr.x_range(), jr.top(), Stroke::new(1.0, p.border));
                let mut ju = ui.new_child(egui::UiBuilder::new().max_rect(jr).layout(egui::Layout::top_down(egui::Align::Min)));
                ju.set_clip_rect(jr);
                crate::panes::console::ui(&mut self.state, &mut ju);
            }
            ui.allocate_rect(egui::Rect::from_min_size(doc_rect.min, avail), Sense::hover());
        });
    }

    fn doc_content(&mut self, ui: &mut Ui) {
        let s = &mut self.state;
        match self.active {
            Doc::Start => crate::panes::start::ui(s, ui),
            Doc::Viewer => crate::panes::viewer::ui(s, ui),
            Doc::Flow => crate::panes::flow_editor::ui(s, ui),
            Doc::Velocity => crate::panes::iva::ui(s, ui),
            Doc::Spectral => crate::panes::spectral::board_ui(s, ui),
            Doc::Statics => crate::panes::statics::board_ui(s, ui),
            Doc::Borehole => crate::panes::vsp::ui(s, ui),
            Doc::Survey => crate::panes::geometry::survey_ui(s, ui),
        }
    }

    fn right_panel(&mut self, ui: &mut Ui, p: &Palette) {
        let slot = ui.available_rect_before_wrap();
        ui.allocate_rect(slot, Sense::hover());
        let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(slot).layout(egui::Layout::top_down(egui::Align::Min)));
        ui.set_clip_rect(slot.intersect(ui.clip_rect()));
        let ui = &mut ui;
        panel_frame(p).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.set_min_size(ui.available_size());
            if self.right_mode == RightMode::Assistant {
                crate::panes::copilot::ui(&mut self.state, ui);
                return;
            }
            let s = &mut self.state;
            match self.active {
                Doc::Viewer => crate::panes::picks::inspector_ui(s, ui),
                Doc::Flow => crate::panes::inspector::ui(s, ui),
                Doc::Velocity => crate::panes::iva::inspector_ui(s, ui),
                Doc::Spectral => crate::panes::spectral::inspector_ui(s, ui),
                Doc::Statics => crate::panes::statics::inspector_ui(s, ui),
                Doc::Borehole => crate::panes::vsp::inspector_ui(s, ui),
                Doc::Survey => crate::panes::geometry::inspector_ui(s, ui),
                Doc::Start => {}
            }
        });
    }

    fn status_line(&mut self, ui: &mut Ui, p: &Palette) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            ui.add_space(10.0);
            let ch = &self.state.crosshair;
            let field = |ui: &mut Ui, k: &str, v: Option<String>| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.label(RichText::new(k).size(11.5).color(p.text_faint));
                    ui.label(RichText::new(v.unwrap_or_else(|| "—".into())).font(font_mono(11.0)).color(p.text));
                });
            };
            field(ui, "Trace", ch.trace_index.map(crate::data::fmt_count));
            field(ui, "Shot", ch.ffid.map(|v| v.to_string()));
            field(ui, "Channel", ch.chan.map(|v| v.to_string()));
            field(ui, "CDP", ch.cdp.map(|v| v.to_string()));
            field(ui, "Offset", ch.offset_m.map(|v| format!("{v:.1} m")));
            field(ui, "Time", ch.time_ms.map(|v| format!("{v:.0} ms")));
            if ch.depth_m.is_some() {
                field(ui, "Depth", ch.depth_m.map(|v| format!("{v:.1} m")));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                let avg = if self.frame_ms.is_empty() { 0.0 } else { self.frame_ms.iter().sum::<f32>() / self.frame_ms.len() as f32 };
                let worst = self.frame_ms.iter().copied().fold(0.0f32, f32::max);
                ui.label(RichText::new(format!("{avg:.1} ms")).font(font_mono(11.0)).color(if worst > 16.6 { p.pick } else { p.text }))
                    .on_hover_text(format!("UI time per frame (worst {worst:.1} ms of the last 90; 16.6 ms keeps 60 FPS)"));
                ui.label(RichText::new("Frame").size(11.5).color(p.text_faint));
                ui.label(RichText::new(fmt_bytes(self.state.summary.bytes)).font(font_mono(11.0)).color(p.text));
                ui.label(RichText::new("Memory").size(11.5).color(p.text_faint));
                if self.state.compute.any_busy() {
                    ui.add(egui::Spinner::new().size(10.0).color(p.text_faint));
                }
            });
        });
    }

    fn toasts(&mut self, ctx: &egui::Context, p: &Palette) {
        let now = self.state.time;
        self.state.toasts.retain(|t| now - t.born < 4.5);
        if self.state.toasts.is_empty() {
            return;
        }
        let mut dismiss: Option<usize> = None;
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -34.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for (rev_i, t) in self.state.toasts.iter().enumerate().rev() {
                    let (ic, c) = match t.level {
                        Level::Info => (icon::INFO, p.info),
                        Level::Ok => (icon::CHECK_CIRCLE, p.success),
                        Level::Warn => (icon::WARNING, p.pick),
                        Level::Error => (icon::WARNING_OCTAGON, p.danger),
                    };
                    let alpha = ((4.5 - (now - t.born) as f32) / 0.4).clamp(0.0, 1.0);
                    ui.scope(|ui| {
                        ui.multiply_opacity(alpha);
                        widgets::glass_frame(p).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                            ui.set_max_width(420.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(ic).color(c));
                                ui.label(RichText::new(&t.text).size(12.5).color(p.text));
                                if ui
                                    .add(egui::Button::new(RichText::new(icon::X).size(11.0).color(p.text_faint)).frame(false))
                                    .on_hover_text("Dismiss")
                                    .clicked()
                                {
                                    dismiss = Some(rev_i);
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                }
            });
        if let Some(i) = dismiss {
            if i < self.state.toasts.len() {
                self.state.toasts.remove(i);
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }

    fn unsaved_flow_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(next_path) = self.state.pending_open_flow.clone() else { return };
        let next_name = crate::panes::project::flow_title(&next_path);
        let cur_name = self.state.flow.name.replace('_', " ");
        let mut choice: Option<u8> = None;
        let modal = egui::Modal::new(egui::Id::unique("unsaved_flow_guard")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new("Unsaved flow changes").font(crate::theme::font_display(18.0)).color(p.text));
            ui.label(
                RichText::new(format!("“{cur_name}” has unsaved edits. What would you like to do before opening “{next_name}”?"))
                    .size(12.5)
                    .color(p.text_muted),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if widgets::cmd_primary(ui, p, icon::FLOPPY_DISK, "Save & open").clicked() {
                    choice = Some(1);
                }
                if widgets::cmd(ui, p, icon::TRASH, "Discard & open", false).clicked() {
                    choice = Some(2);
                }
                if widgets::cmd(ui, p, "", "Cancel", false).clicked() {
                    choice = Some(0);
                }
            });
        });
        if modal.should_close() {
            choice = Some(0);
        }
        match choice {
            Some(1) => {
                self.state.save_flow(false);
                if !self.state.flow.dirty {
                    self.state.pending_open_flow = None;
                    self.state.open_flow(&next_path);
                }
            }
            Some(2) => {
                self.state.pending_open_flow = None;
                self.state.open_flow(&next_path);
            }
            Some(0) => {
                self.state.pending_open_flow = None;
            }
            _ => {}
        }
    }

    fn command_palette(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.palette_open {
            return;
        }
        #[derive(Clone)]
        enum Cmd {
            Stage(Workspace),
            Doc(Doc),
            OpenData,
            OpenFlow,
            SaveFlow,
            RunFlow,
            LoadDb,
            SaveDb,
            Theme,
            Undo,
            Redo,
            AddTool(String),
        }
        let mut cmds: Vec<(String, String, Cmd)> = Workspace::ALL
            .iter()
            .enumerate()
            .map(|(i, w)| (format!("{}  Go to {}", icon::ARROW_RIGHT, w.title()), format!("Ctrl+{}", i + 1), Cmd::Stage(*w)))
            .collect();
        for d in [Doc::Spectral, Doc::Survey, Doc::Start] {
            cmds.push((format!("{}  Open {}", d.icon(), d.title()), String::new(), Cmd::Doc(d)));
        }
        cmds.extend([
            (format!("{}  Open seismic data…", icon::FOLDER_OPEN), "Ctrl+O".into(), Cmd::OpenData),
            (format!("{}  Open flow…", icon::FLOW_ARROW), String::new(), Cmd::OpenFlow),
            (format!("{}  Save flow", icon::FLOPPY_DISK), "Ctrl+S".into(), Cmd::SaveFlow),
            (format!("{}  Run flow", icon::PLAY), "Ctrl+Enter".into(), Cmd::RunFlow),
            (format!("{}  Open project database…", icon::DATABASE), String::new(), Cmd::LoadDb),
            (format!("{}  Save project database", icon::DATABASE), String::new(), Cmd::SaveDb),
            (format!("{}  Switch light / dark", icon::CIRCLE_HALF), String::new(), Cmd::Theme),
            (format!("{}  Undo", icon::ARROW_COUNTER_CLOCKWISE), "Ctrl+Z".into(), Cmd::Undo),
            (format!("{}  Redo", icon::ARROW_CLOCKWISE), "Ctrl+Y".into(), Cmd::Redo),
        ]);
        for t in &self.state.catalog {
            cmds.push((format!("{}  Add step · {}", icon::PLUS, t.label), t.category.clone(), Cmd::AddTool(t.menu_id.clone())));
        }
        let q = self.palette_query.to_ascii_lowercase();
        let hits: Vec<(String, String, Cmd)> = cmds
            .into_iter()
            .filter(|(l, k, _)| q.is_empty() || l.to_ascii_lowercase().contains(&q) || k.to_ascii_lowercase().contains(&q))
            .take(12)
            .collect();
        let mut run: Option<Cmd> = None;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("cmd_palette")).show(ctx, |ui| {
            ui.set_width(560.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.palette_query)
                    .hint_text("Go to a stage, open something, or add a step…")
                    .font(egui::FontId::proportional(15.0))
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(10, 8)),
            );
            r.request_focus();
            if r.changed() {
                self.palette_sel = 0;
            }
            ui.input(|i| {
                if i.key_pressed(egui::Key::ArrowDown) {
                    self.palette_sel = (self.palette_sel + 1).min(hits.len().saturating_sub(1));
                }
                if i.key_pressed(egui::Key::ArrowUp) {
                    self.palette_sel = self.palette_sel.saturating_sub(1);
                }
                if i.key_pressed(egui::Key::Escape) {
                    close = true;
                }
                if i.key_pressed(egui::Key::Enter) {
                    run = hits.get(self.palette_sel).map(|h| h.2.clone());
                }
            });
            ui.add_space(6.0);
            for (i, (label, key, cmd)) in hits.iter().enumerate() {
                let sel = i == self.palette_sel;
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                if sel || resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(7), if sel { p.accent_soft } else { p.raised });
                }
                ui.painter().text(rect.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), if sel { p.text } else { p.text_muted });
                ui.painter().text(rect.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, key, egui::FontId::proportional(11.5), p.text_faint);
                if resp.clicked() {
                    run = Some(cmd.clone());
                }
            }
            if hits.is_empty() {
                ui.label(RichText::new("Nothing matches").color(p.text_faint));
            }
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(cmd) = run {
            close = true;
            match cmd {
                Cmd::Stage(w) => self.set_stage(w),
                Cmd::Doc(d) => self.open(d),
                Cmd::OpenData => self.state.open_seismic_dialog(),
                Cmd::OpenFlow => self.state.open_flow_dialog(),
                Cmd::SaveFlow => self.state.save_flow(false),
                Cmd::RunFlow => self.state.run_current_flow(),
                Cmd::LoadDb => self.state.open_opf_dialog(),
                Cmd::SaveDb => self.state.save_opf_dialog(),
                Cmd::Theme => self.state.theme = self.state.theme.toggled(),
                Cmd::Undo => {
                    if self.active == Doc::Flow { self.state.undo_flow() } else { self.state.undo() }
                }
                Cmd::Redo => {
                    if self.active == Doc::Flow { self.state.redo_flow() } else { self.state.redo() }
                }
                Cmd::AddTool(id) => {
                    self.state.insert_tool(&id);
                    self.open(Doc::Flow);
                }
            }
        }
        if close {
            self.palette_open = false;
        }
    }

    /// One full UI pass (also driven by headless tests).
    pub fn frame(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let t_frame = std::time::Instant::now();
        self.state.time = ctx.input(|i| i.time);
        if self.state.poll_engine() {
            ctx.request_repaint();
        }
        self.route_compute(&ctx);
        crate::tour::Tour::tick(self, &ctx);
        if self.applied_theme != Some(self.state.theme) {
            theme::apply(&ctx, self.state.theme);
            self.applied_theme = Some(self.state.theme);
        }
        self.shortcuts(&ctx);
        if let Some(w) = self.state.stage_request.take() {
            self.set_stage(w);
        }
        if let Some(d) = self.state.doc_request.take() {
            self.open(d);
        }
        let p = *palette(self.state.theme);
        let g = space::GUTTER as i8;

        egui::Panel::top("rs_top").frame(Frame::NONE.fill(p.chrome).inner_margin(Margin { left: 8, right: 8, top: 4, bottom: 2 })).exact_size(50.0).show_separator_line(false).show(ui, |ui| self.top_bar(ui, &p));
        egui::Panel::top("rs_cmd").frame(Frame::NONE.fill(p.chrome).inner_margin(Margin { left: g, right: g, top: 2, bottom: 0 })).exact_size(46.0).show_separator_line(false).show(ui, |ui| {
            crate::commands::bar(self, ui, &p);
        });
        egui::Panel::bottom("rs_status").frame(Frame::NONE.fill(p.chrome)).exact_size(26.0).show_separator_line(false).show(ui, |ui| self.status_line(ui, &p));

        let picking = self.state.viewer.mode != crate::panes::viewer::PickMode::Navigate && self.active == Doc::Viewer;
        if self.active != Doc::Start {
            if self.left_open && !picking {
                egui::Panel::left("rs_left")
                    .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin { left: g, right: 0, top: g, bottom: 0 }))
                    .resizable(true)
                    .default_size(266.0)
                    .size_range(210.0..=440.0)
                    .show_separator_line(false)
                    .show(ui, |ui| self.left_panel(ui, &p));
            } else {
                egui::Panel::left("rs_rail")
                    .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin { left: g, right: 0, top: g, bottom: 0 }))
                    .exact_size(52.0)
                    .show_separator_line(false)
                    .show(ui, |ui| self.rail(ui, &p));
            }
        }
        if self.active != Doc::Start || self.right_mode == RightMode::Assistant {
            let w = if self.right_mode == RightMode::Assistant { 380.0 } else { 316.0 };
            egui::Panel::right("rs_right")
                .frame(Frame::NONE.fill(p.chrome).inner_margin(Margin { left: 0, right: g, top: g, bottom: 0 }))
                .resizable(true)
                .default_size(w)
                .size_range(260.0..=520.0)
                .show_separator_line(false)
                .show(ui, |ui| self.right_panel(ui, &p));
        }
        egui::CentralPanel::default().frame(Frame::NONE.fill(p.chrome).inner_margin(Margin::same(g))).show(ui, |ui| self.center_panel(ui, &p));

        self.toasts(&ctx, &p);
        self.command_palette(&ctx, &p);
        self.unsaved_flow_modal(&ctx, &p);
        crate::panes::project::new_project_modal(&mut self.state, &ctx);

        if self.state.is_busy() || self.state.compute.any_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.frame_ms.len() >= 90 {
            self.frame_ms.pop_front();
        }
        self.frame_ms.push_back(t_frame.elapsed().as_secs_f32() * 1000.0);
    }
}

impl eframe::App for RustSeisApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        palette(self.state.theme).chrome.to_normalized_gamma_f32()
    }
}

fn stage_pills_width(ui: &Ui, p: &Palette) -> f32 {
    let font = egui::FontId::proportional(13.0);
    Workspace::ALL.iter().map(|w| ui.painter().layout_no_wrap(w.title().to_string(), font.clone(), p.text).size().x + 34.0).sum::<f32>() + 8.0
}

fn hline(ui: &mut Ui, p: &Palette) {
    let r = ui.available_rect_before_wrap();
    ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, p.border));
    ui.add_space(1.0);
}

/// Square icon button used in the top bar and rails.
pub fn top_icon(ui: &mut Ui, p: &Palette, ic: &str, tip: &str, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 30.0), Sense::click());
    let fill = if active { p.accent_soft } else if resp.hovered() { p.raised } else { egui::Color32::TRANSPARENT };
    let stroke = if active { Stroke::new(1.0, p.accent_line) } else { Stroke::NONE };
    ui.painter().rect(rect, CornerRadius::same(8), fill, stroke, StrokeKind::Inside);
    let col = if active { p.accent_text } else if resp.hovered() { p.text } else { p.text_muted };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, ic, egui::FontId::proportional(16.0), col);
    resp.on_hover_text(tip)
}

fn search_box(ui: &mut Ui, p: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(230.0, 30.0), Sense::click());
    ui.painter().rect(rect, CornerRadius::same(8), p.panel, Stroke::new(1.0, if resp.hovered() { p.border_strong } else { p.border }), StrokeKind::Inside);
    ui.painter().text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, icon::MAGNIFYING_GLASS, egui::FontId::proportional(13.0), p.text_faint);
    ui.painter().text(rect.left_center() + vec2(32.0, 0.0), Align2::LEFT_CENTER, "Go to anything", egui::FontId::proportional(12.5), p.text_faint);
    let k = egui::Rect::from_center_size(rect.right_center() - vec2(30.0, 0.0), vec2(44.0, 18.0));
    ui.painter().rect(k, CornerRadius::same(4), egui::Color32::TRANSPARENT, Stroke::new(1.0, p.border_strong), StrokeKind::Inside);
    ui.painter().text(k.center(), Align2::CENTER_CENTER, "Ctrl K", font_mono(10.5), p.text_faint);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn tab_pill(ui: &mut Ui, p: &Palette, label: &str, on: bool) -> egui::Response {
    let font = if on { font_semibold(12.5) } else { egui::FontId::proportional(12.5) };
    let g = ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 26.0), Sense::click());
    if on {
        ui.painter().rect_filled(rect, CornerRadius::same(7), p.raised);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(7), with_alpha(p.raised, 140));
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, if on { p.text } else { p.text_faint });
    resp
}

/// Document tab pill; returns (response, close-clicked).
fn doc_pill(ui: &mut Ui, p: &Palette, d: Doc, on: bool, closable: bool) -> (egui::Response, bool) {
    let label = format!("{}  {}", d.icon(), d.title());
    let font = egui::FontId::proportional(12.5);
    let g = ui.painter().layout_no_wrap(label.clone(), font.clone(), p.text);
    let extra = if closable && on { 22.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 22.0 + extra, 28.0), Sense::click());
    if on {
        ui.painter().rect_filled(rect, CornerRadius::same(7), p.raised);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(7), with_alpha(p.raised, 140));
    }
    let col = if on { p.text } else { p.text_muted };
    ui.painter().text(pos2(rect.left() + 11.0, rect.center().y), Align2::LEFT_CENTER, label, font, col);
    let mut closed = false;
    if closable && on {
        let xr = egui::Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(16.0, 16.0));
        let xresp = ui.interact(xr, egui::Id::unique(("doc_close", d as u8)), Sense::click());
        if xresp.hovered() {
            ui.painter().rect_filled(xr, CornerRadius::same(4), mix(p.raised, p.text, 0.1));
        }
        ui.painter().text(xr.center(), Align2::CENTER_CENTER, icon::X, egui::FontId::proportional(11.0), p.text_faint);
        closed = xresp.clicked();
    }
    (resp, closed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_undo_redo_and_unsaved_guard() {
        let ctx = egui::Context::default();
        let mut app = RustSeisApp::with_context(&ctx, false);
        assert_eq!(app.state.flow.count(), 0);
        app.state.insert_tool("agc");
        assert_eq!(app.state.flow.count(), 1);
        assert!(app.state.flow.dirty);

        // Requesting another flow while dirty triggers confirmation modal instead of discarding.
        let dummy = std::path::PathBuf::from("nonexistent_other_flow.json");
        app.state.request_open_flow(dummy.clone());
        assert_eq!(app.state.pending_open_flow, Some(dummy));
        assert_eq!(app.state.flow.count(), 1);

        // Undo / redo restores the flow document cleanly.
        app.state.undo_flow();
        assert_eq!(app.state.flow.count(), 0);
        app.state.redo_flow();
        assert_eq!(app.state.flow.count(), 1);
    }

    #[test]
    fn stage_request_preserves_imaging_stage() {
        let ctx = egui::Context::default();
        let mut app = RustSeisApp::with_context(&ctx, false);
        app.state.stage_request = Some(Workspace::Imaging);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1280.0, 800.0))),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.frame(ui));
        assert_eq!(app.stage, Workspace::Imaging);
        assert_eq!(app.active, Doc::Viewer);
    }
}
