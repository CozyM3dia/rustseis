//! Application shell: title bar, workspace switcher, dock presets, status bar, toasts and the
//! command palette.

use crate::data::{fmt_bytes, fmt_ms};
use crate::panes::{Pane, PaneViewer};
use crate::state::{AppState, Level, Workspace};
use crate::theme::{self, Palette, ThemeMode, font_display, font_mono, mix, palette, with_alpha};
use crate::widgets::icon_button;
use egui::{Align2, CornerRadius, Frame, Margin, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_dock::{DockArea, DockState, NodeIndex};
use egui_phosphor::regular as icon;
use std::collections::{HashMap, VecDeque};

pub struct RustSeisApp {
    pub state: AppState,
    pub workspace: Workspace,
    docks: HashMap<Workspace, DockState<Pane>>,
    applied_theme: Option<ThemeMode>,
    palette_open: bool,
    palette_query: String,
    palette_sel: usize,
    frame_ms: VecDeque<f32>,
    pub tour: Option<crate::tour::Tour>,
}

fn dock_for(ws: Workspace) -> DockState<Pane> {
    match ws {
        Workspace::Flow => {
            let mut d = DockState::new(vec![Pane::FlowEditor]);
            let t = d.main_surface_mut();
            let [center, _left] = t.split_left(NodeIndex::root(), 0.21, vec![Pane::Catalog, Pane::Project]);
            let [center, _right] = t.split_right(center, 0.66, vec![Pane::Inspector]);
            let [_c, _bottom] = t.split_below(center, 0.70, vec![Pane::Console]);
            d
        }
        Workspace::Viewer => {
            let mut d = DockState::new(vec![Pane::Viewer]);
            let t = d.main_surface_mut();
            let [_c, right] = t.split_right(NodeIndex::root(), 0.77, vec![Pane::Picks]);
            let [_r, _h] = t.split_below(right, 0.58, vec![Pane::Headers]);
            d
        }
        Workspace::Velocity => {
            let mut d = DockState::new(vec![Pane::Iva]);
            let t = d.main_surface_mut();
            let [_c, _r] = t.split_right(NodeIndex::root(), 0.84, vec![Pane::VelTable]);
            d
        }
        Workspace::Spectral => {
            let mut d = DockState::new(vec![Pane::Radon]);
            let t = d.main_surface_mut();
            let [_radon, top] = t.split_above(NodeIndex::root(), 0.52, vec![Pane::Spectrum]);
            let [_s, _fk] = t.split_right(top, 0.40, vec![Pane::Fk]);
            d
        }
        Workspace::Statics => {
            let mut d = DockState::new(vec![Pane::Vsp]);
            let t = d.main_surface_mut();
            let [_vsp, top] = t.split_above(NodeIndex::root(), 0.56, vec![Pane::Statics]);
            let [_st, rest] = t.split_right(top, 0.36, vec![Pane::ScAmp]);
            let [_sc, _fb] = t.split_right(rest, 0.52, vec![Pane::FbQc]);
            d
        }
        Workspace::Survey => {
            let mut d = DockState::new(vec![Pane::Basemap]);
            let t = d.main_surface_mut();
            let [base, _cop] = t.split_right(NodeIndex::root(), 0.63, vec![Pane::Copilot]);
            let [_b, _g] = t.split_below(base, 0.52, vec![Pane::GeomTable]);
            d
        }
    }
}

impl RustSeisApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_context(&cc.egui_ctx, true)
    }

    /// Construct against any egui context (used by headless tests with `fonts = false`).
    pub fn with_context(ctx: &egui::Context, install_fonts: bool) -> Self {
        theme::install(ctx, ThemeMode::Dark, install_fonts);
        egui_extras::install_image_loaders(ctx);
        let docks = Workspace::ALL.iter().map(|w| (*w, dock_for(*w))).collect();
        Self {
            state: AppState::new(ctx),
            workspace: Workspace::Flow,
            docks,
            applied_theme: Some(ThemeMode::Dark),
            palette_open: false,
            palette_query: String::new(),
            palette_sel: 0,
            frame_ms: VecDeque::with_capacity(120),
            tour: None,
        }
    }

    /// Open a file passed on the command line (`.sgy/.segd/.rsds` data, `.json/.toml` flow, `.sqlite` OPF).
    pub fn open_path(&mut self, path: &std::path::Path) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "json" | "toml" => self.state.open_flow(path),
            "sqlite" | "db" => self.state.load_opf(path.to_path_buf()),
            _ => self.state.open_seismic(path, None),
        }
    }

    pub fn reset_layout(&mut self) {
        self.docks.insert(self.workspace, dock_for(self.workspace));
    }

    fn route_compute(&mut self, ctx: &egui::Context) {
        let done = self.state.compute.drain();
        for d in done {
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
            for (i, k) in [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num6].iter().enumerate() {
                if pressed(*k) {
                    self.workspace = Workspace::ALL[i];
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
            if pressed(egui::Key::T) && !typing {
                self.state.theme = self.state.theme.toggled();
            }
            if !typing && pressed(egui::Key::Z) {
                if shift { self.state.redo() } else { self.state.undo() }
            }
            if !typing && pressed(egui::Key::Y) {
                self.state.redo();
            }
        } else if !typing {
            if pressed(egui::Key::F12) {
                let dir = std::env::current_dir().unwrap_or_default().join("screenshots");
                let stamp = crate::state::chrono_like::Stamp::now().0;
                let name = self.workspace.title().replace([' ', '&'], "_");
                let path = dir.join(format!("rustseis_{name}_{stamp}.png"));
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
            if pressed(egui::Key::F) && self.workspace == Workspace::Viewer {
                self.state.viewer.canvas.reset();
            }
        }
    }

    fn title_bar(&mut self, ui: &mut Ui, p: &Palette) {
        Frame::new().fill(p.chrome).inner_margin(Margin { left: 10, right: 10, top: 6, bottom: 0 }).show(ui, |ui| {
            ui.set_height(34.0);
            ui.horizontal(|ui| {
                brand(ui, p);
                ui.add_space(14.0);
                for (i, ws) in Workspace::ALL.iter().enumerate() {
                    if workspace_tab(ui, p, *ws, self.workspace == *ws, i + 1).clicked() {
                        self.workspace = *ws;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let theme_icon = if self.state.theme == ThemeMode::Dark { icon::SUN } else { icon::MOON };
                    if icon_button(ui, p, theme_icon, "Toggle Abyssal / Survey theme (Ctrl+T)", false).clicked() {
                        self.state.theme = self.state.theme.toggled();
                    }
                    if icon_button(ui, p, icon::COMMAND, "Command palette (Ctrl+K)", self.palette_open).clicked() {
                        self.palette_open = !self.palette_open;
                        self.palette_query.clear();
                    }
                    if icon_button(ui, p, icon::ARROW_CLOCKWISE, "Redo (Ctrl+Y)", false).clicked() {
                        self.state.redo();
                    }
                    if icon_button(ui, p, icon::ARROW_COUNTER_CLOCKWISE, "Undo last flow (Ctrl+Z)", false).clicked() {
                        self.state.undo();
                    }
                    ui.add_space(6.0);
                    job_pill(ui, p, &self.state);
                    ui.add_space(6.0);
                    data_chip(ui, p, &self.state);
                });
            });
        });
        let r = ui.min_rect();
        ui.painter().hline(r.x_range(), r.bottom(), Stroke::new(1.0, p.border));
    }

    fn status_bar(&mut self, ui: &mut Ui, p: &Palette) {
        Frame::new().fill(p.chrome).inner_margin(Margin::symmetric(10, 3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let ch = &self.state.crosshair;
                let field = |ui: &mut Ui, k: &str, v: Option<String>| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(RichText::new(k).font(font_mono(10.0)).color(p.text_faint));
                        ui.label(RichText::new(v.unwrap_or_else(|| "—".into())).font(font_mono(11.0)).color(p.text));
                    });
                };
                ui.label(RichText::new(icon::CROSSHAIR).color(p.accent));
                field(ui, "TRACE", ch.trace_index.map(|v| v.to_string()));
                field(ui, "FFID", ch.ffid.map(|v| v.to_string()));
                field(ui, "CHAN", ch.chan.map(|v| v.to_string()));
                field(ui, "CDP", ch.cdp.map(|v| v.to_string()));
                field(ui, "OFFSET", ch.offset_m.map(|v| format!("{v:.1} m")));
                field(ui, "TIME", ch.time_ms.map(|v| format!("{v:.1} ms")));
                field(ui, "X", ch.map_x.filter(|v| *v != 0.0).map(|v| format!("{v:.1}")));
                field(ui, "Y", ch.map_y.filter(|v| *v != 0.0).map(|v| format!("{v:.1}")));
                field(ui, "DEPTH", ch.depth_m.map(|v| format!("{v:.1} m")));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let avg = if self.frame_ms.is_empty() { 0.0 } else { self.frame_ms.iter().sum::<f32>() / self.frame_ms.len() as f32 };
                    let worst = self.frame_ms.iter().copied().fold(0.0f32, f32::max);
                    ui.label(RichText::new(format!("ui {avg:.1} ms · max {worst:.1} ms")).font(font_mono(10.5)).color(if worst > 16.6 { p.pick } else { p.text_faint }))
                        .on_hover_text("CPU time per UI frame (budget 16.6 ms for 60 FPS); egui repaints only on input or animation");
                    ui.label(RichText::new(format!("{} in session", fmt_bytes(self.state.summary.bytes))).font(font_mono(10.5)).color(p.text_faint));
                    if self.state.compute.any_busy() {
                        ui.add(egui::Spinner::new().size(10.0).color(p.text_faint));
                    }
                    ui.label(RichText::new(self.workspace.hint()).size(11.0).color(p.text_faint));
                });
            });
        });
    }

    fn toasts(&mut self, ctx: &egui::Context, p: &Palette) {
        let now = self.state.time;
        self.state.toasts.retain(|t| now - t.born < 4.5);
        if self.state.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("toasts")).anchor(Align2::RIGHT_BOTTOM, vec2(-14.0, -34.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            for t in self.state.toasts.iter().rev() {
                let (ic, c) = match t.level {
                    Level::Info => (icon::INFO, p.info),
                    Level::Ok => (icon::CHECK, p.success),
                    Level::Warn => (icon::WARNING, p.pick),
                    Level::Error => (icon::WARNING, p.danger),
                };
                let age = (now - t.born) as f32;
                let alpha = ((4.5 - age) / 0.4).clamp(0.0, 1.0);
                ui.scope(|ui| {
                    ui.multiply_opacity(alpha);
                    Frame::new()
                        .fill(p.raised)
                        .stroke(Stroke::new(1.0, with_alpha(c, 140)))
                        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                        .inner_margin(Margin::symmetric(10, 7))
                        .shadow(ui.visuals().popup_shadow)
                        .show(ui, |ui| {
                            ui.set_max_width(420.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(ic).color(c));
                                ui.label(RichText::new(&t.text).size(12.5));
                            });
                        });
                });
                ui.add_space(6.0);
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }

    fn command_palette(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.palette_open {
            return;
        }
        #[derive(Clone)]
        enum Cmd {
            Ws(Workspace),
            OpenData,
            OpenFlow,
            SaveFlow,
            RunFlow,
            LoadOpf,
            SaveOpf,
            Theme,
            Undo,
            Redo,
            ResetLayout,
            AddTool(String),
        }
        let mut cmds: Vec<(String, String, Cmd)> = Workspace::ALL.iter().enumerate().map(|(i, w)| (format!("{}  Go to {}", w.icon(), w.title()), format!("Ctrl+{}", i + 1), Cmd::Ws(*w))).collect();
        cmds.extend([
            (format!("{}  Open seismic data…", icon::FOLDER_OPEN), "Ctrl+O".into(), Cmd::OpenData),
            (format!("{}  Open flow…", icon::FILE_CODE), String::new(), Cmd::OpenFlow),
            (format!("{}  Save flow", icon::FLOPPY_DISK), "Ctrl+S".into(), Cmd::SaveFlow),
            (format!("{}  Run flow", icon::PLAY), "Ctrl+Enter".into(), Cmd::RunFlow),
            (format!("{}  Load OPF database…", icon::DATABASE), String::new(), Cmd::LoadOpf),
            (format!("{}  Save OPF database", icon::DATABASE), String::new(), Cmd::SaveOpf),
            (format!("{}  Toggle theme", icon::CIRCLE_HALF), "Ctrl+T".into(), Cmd::Theme),
            (format!("{}  Undo", icon::ARROW_COUNTER_CLOCKWISE), "Ctrl+Z".into(), Cmd::Undo),
            (format!("{}  Redo", icon::ARROW_CLOCKWISE), "Ctrl+Y".into(), Cmd::Redo),
            (format!("{}  Reset workspace layout", icon::FRAME_CORNERS), String::new(), Cmd::ResetLayout),
        ]);
        for t in &self.state.catalog {
            cmds.push((format!("{}  Add tool · {}", icon::PLUS, t.label), t.menu_id.clone(), Cmd::AddTool(t.menu_id.clone())));
        }
        let q = self.palette_query.to_ascii_lowercase();
        let hits: Vec<(String, String, Cmd)> = cmds.into_iter().filter(|(l, k, _)| q.is_empty() || l.to_ascii_lowercase().contains(&q) || k.to_ascii_lowercase().contains(&q)).take(14).collect();
        let mut run: Option<Cmd> = None;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("cmd_palette")).show(ctx, |ui| {
            ui.set_width(520.0);
            let r = ui.add(egui::TextEdit::singleline(&mut self.palette_query).hint_text("Type a command or tool name…").font(egui::FontId::proportional(15.0)).desired_width(f32::INFINITY));
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
            ui.add_space(4.0);
            for (i, (label, key, cmd)) in hits.iter().enumerate() {
                let sel = i == self.palette_sel;
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                if sel || resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(4), if sel { p.accent_soft } else { with_alpha(p.raised, 200) });
                }
                ui.painter().text(rect.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), if sel { p.text } else { p.text_muted });
                ui.painter().text(rect.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, key, font_mono(10.5), p.text_faint);
                if resp.clicked() {
                    run = Some(cmd.clone());
                }
            }
            if hits.is_empty() {
                ui.label(RichText::new("No matches").color(p.text_faint));
            }
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(cmd) = run {
            close = true;
            let s = &mut self.state;
            match cmd {
                Cmd::Ws(w) => self.workspace = w,
                Cmd::OpenData => s.open_seismic_dialog(),
                Cmd::OpenFlow => s.open_flow_dialog(),
                Cmd::SaveFlow => s.save_flow(false),
                Cmd::RunFlow => s.run_current_flow(),
                Cmd::LoadOpf => s.open_opf_dialog(),
                Cmd::SaveOpf => s.save_opf_dialog(),
                Cmd::Theme => s.theme = s.theme.toggled(),
                Cmd::Undo => s.undo(),
                Cmd::Redo => s.redo(),
                Cmd::ResetLayout => self.reset_layout(),
                Cmd::AddTool(id) => {
                    s.insert_tool(&id);
                    self.workspace = Workspace::Flow;
                }
            }
        }
        if close {
            self.palette_open = false;
        }
    }

    /// One full UI pass (also used by headless tests).
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
        if let Some(ws) = self.state.workspace_request.take() {
            self.workspace = ws;
        }
        let p = *palette(self.state.theme);

        egui::Panel::top("rs_title").frame(Frame::NONE).show_separator_line(false).show(ui, |ui| self.title_bar(ui, &p));
        egui::Panel::bottom("rs_status").frame(Frame::NONE).show_separator_line(false).show(ui, |ui| self.status_bar(ui, &p));
        egui::CentralPanel::default().frame(Frame::new().fill(p.chrome)).show(ui, |ui| {
            let style = theme::dock_style(ui.style(), &p);
            let ws = self.workspace;
            let dock = self.docks.entry(ws).or_insert_with(|| dock_for(ws));
            let mut viewer = PaneViewer { state: &mut self.state };
            DockArea::new(dock)
                .id(egui::Id::unique(("dock", ws as u8)))
                .style(style)
                .show_close_buttons(false)
                .show_add_buttons(false)
                .tab_context_menus(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show_inside(ui, &mut viewer);
        });
        self.toasts(&ctx, &p);
        self.command_palette(&ctx, &p);

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

fn brand(ui: &mut Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(6), mix(p.chrome, p.accent, 0.16), Stroke::new(1.0, with_alpha(p.accent, 150)), StrokeKind::Inside);
    // A tiny wavelet with a positive VA lobe: the mark.
    let c = rect.center();
    let pts: Vec<egui::Pos2> = (0..=24)
        .map(|i| {
            let x = i as f32 / 24.0;
            let t = (x - 0.5) * 6.0;
            let a = (1.0 - 2.0 * t * t) * (-t * t).exp();
            pos2(rect.left() + 3.0 + x * (rect.width() - 6.0), c.y - a * 7.0)
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(1.8, p.accent)));
    ui.add_space(2.0);
    ui.label(RichText::new("RustSeis").font(font_display(17.0)).color(p.text));
    ui.label(RichText::new("2D").font(font_mono(10.0)).color(p.text_faint));
}

fn workspace_tab(ui: &mut Ui, p: &Palette, ws: Workspace, active: bool, n: usize) -> egui::Response {
    let label = format!("{}  {}", ws.icon(), ws.title());
    let font = egui::FontId::proportional(12.5);
    let g = ui.painter().layout_no_wrap(label.clone(), font.clone(), p.text);
    let size = vec2(g.size().x + 22.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp.hovered();
    if active {
        let r = egui::Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y + 1.0));
        ui.painter().rect(r, CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 }, p.panel, Stroke::new(1.0, p.border), StrokeKind::Inside);
        ui.painter().hline(rect.x_range().shrink(6.0), rect.top() + 1.0, Stroke::new(2.0, p.accent));
    } else if hovered {
        ui.painter().rect_filled(rect.shrink2(vec2(0.0, 3.0)), CornerRadius::same(4), mix(p.chrome, p.panel, 0.7));
    }
    let color = if active { p.text } else if hovered { p.text } else { p.text_muted };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, color);
    resp.on_hover_text(format!("{} — {} (Ctrl+{n})", ws.title(), ws.hint()))
}

fn data_chip(ui: &mut Ui, p: &Palette, s: &AppState) {
    let text = if s.summary.n_traces > 0 { s.summary.headline() } else { "no data loaded".into() };
    let font = font_mono(11.0);
    let g = ui.painter().layout_no_wrap(text, font, if s.summary.n_traces > 0 { p.text } else { p.text_faint });
    let (rect, resp) = ui.allocate_exact_size(g.size() + vec2(30.0, 10.0), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(5), p.sunken, Stroke::new(1.0, p.border), StrokeKind::Inside);
    ui.painter().text(rect.left_center() + vec2(9.0, 0.0), Align2::CENTER_CENTER, icon::WAVEFORM, egui::FontId::proportional(12.0), p.accent);
    ui.painter().galley(rect.min + vec2(20.0, 5.0), g, p.text);
    resp.on_hover_text(format!("Active traces · {} buffers in FlowContext", s.summary.buffers));
}

fn job_pill(ui: &mut Ui, p: &Palette, s: &AppState) {
    let (text, color, busy) = match s.busy_info() {
        Some((label, el, eta)) => {
            let t = match eta {
                Some(e) => format!("{label} · {} / ~{}", fmt_ms(el.as_secs_f64() * 1000.0), fmt_ms(e as f64)),
                None => format!("{label} · {}", fmt_ms(el.as_secs_f64() * 1000.0)),
            };
            (t, p.accent, true)
        }
        None => match s.history.last() {
            Some(j) if !j.ok => ("last job failed".to_string(), p.danger, false),
            Some(j) => (format!("idle · last {}", fmt_ms(j.elapsed.as_secs_f64() * 1000.0)), p.text_muted, false),
            None => ("engine idle".to_string(), p.text_muted, false),
        },
    };
    let font = egui::FontId::proportional(11.5);
    let g = ui.painter().layout_no_wrap(text, font, color);
    let w = (g.size().x + 30.0).min(360.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(12), if busy { with_alpha(p.accent, 26) } else { p.sunken }, Stroke::new(1.0, if busy { with_alpha(p.accent, 120) } else { p.border }), StrokeKind::Inside);
    let dot = rect.left_center() + vec2(12.0, 0.0);
    if busy {
        let t = ui.input(|i| i.time) as f32;
        let r = 3.0 + (t * 4.0).sin().abs() * 1.5;
        ui.painter().circle_filled(dot, r, p.accent);
        ui.ctx().request_repaint();
    } else {
        ui.painter().circle_filled(dot, 3.0, if color == p.danger { p.danger } else { p.success });
    }
    let clip = egui::Rect::from_min_max(pos2(rect.left() + 20.0, rect.top()), pos2(rect.right() - 9.0, rect.bottom()));
    ui.painter().with_clip_rect(clip).galley(pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0), g, color);
}
