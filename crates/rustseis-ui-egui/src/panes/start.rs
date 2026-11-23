//! Start page: quick actions, sample lines, recent flows and a first-line checklist.

use super::project::{NewProject, flow_files, flow_title, list_row, refresh_files, sample_files};
use crate::state::{AppState, StageStatus, Workspace};
use crate::theme::{Palette, mix, palette, with_alpha};
use crate::widgets::{self, caps};
use egui::{Color32, CornerRadius, RichText, Sense, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as icon;
use std::path::PathBuf;

pub fn ui(s: &mut AppState, ui: &mut Ui) {
    let p = *palette(s.theme);
    refresh_files(s);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let w = ui.available_width().min(1180.0);
        let margin = ((ui.available_width() - w) * 0.5).max(24.0);
        egui::Frame::new().inner_margin(egui::Margin { left: margin.min(120.0) as i8, right: margin.min(120.0) as i8, top: 28, bottom: 24 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            hero(ui, &p);
            ui.add_space(6.0);
            tiles(s, ui, &p);
            ui.add_space(12.0);
            let col_w = (ui.available_width() - 24.0) / 3.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.vertical(|ui| {
                    ui.set_width(col_w * 2.0 + 12.0);
                    samples(s, ui, &p);
                    ui.add_space(10.0);
                    flows(s, ui, &p);
                });
                ui.vertical(|ui| {
                    ui.set_width(col_w);
                    checklist(s, ui, &p);
                });
            });
        });
    });
}

fn hero(ui: &mut Ui, p: &Palette) {
    ui.horizontal(|ui| {
        widgets::logo(ui, p, 44.0);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new("Welcome to RustSeis").font(crate::theme::font_display(26.0)).color(p.text));
            ui.label(RichText::new("Seismic processing from field records to an image, on your own machine.").size(13.5).color(p.text_muted));
        });
    });
}

fn tile(ui: &mut Ui, p: &Palette, ic: &str, title: &str, body: &str, accent: Color32, w: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 112.0), Sense::click());
    let hov = resp.hovered();
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(14), if hov { mix(p.raised, accent, 0.10) } else { p.raised }, Stroke::new(1.0, if hov { with_alpha(accent, 160) } else { p.border }), egui::StrokeKind::Inside);
    let disc = pos2(rect.left() + 30.0, rect.top() + 32.0);
    painter.circle_filled(disc, 16.0, with_alpha(accent, 36));
    painter.text(disc, egui::Align2::CENTER_CENTER, ic, egui::FontId::proportional(17.0), accent);
    painter.text(pos2(rect.left() + 16.0, rect.top() + 62.0), egui::Align2::LEFT_TOP, title, crate::theme::font_semibold(14.0), p.text);
    let g = painter.layout(body.to_string(), egui::FontId::proportional(11.5), p.text_muted, w - 32.0);
    painter.galley(pos2(rect.left() + 16.0, rect.top() + 82.0), g, p.text_muted);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn tiles(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let w = (ui.available_width() - 36.0) / 4.0;
    let first_sample = sample_files(s).into_iter().next();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        if tile(ui, p, icon::FOLDER_OPEN, "Open data", "SEG-Y files or a folder of SEG-D shots", p.accent, w).clicked() {
            s.open_seismic_dialog();
        }
        if tile(ui, p, icon::FLOW_ARROW, "Open a flow", "Pick up a processing sequence", p.info, w).clicked() {
            s.open_flow_dialog();
        }
        if tile(ui, p, icon::FOLDER_SIMPLE_PLUS, "New project", "Keep a line's data, flows and tables together", p.series_c, w).clicked() {
            s.project.new_project = Some(NewProject { area: "Survey".into(), line: "Line 01".into(), ..Default::default() });
        }
        let r = tile(ui, p, icon::PLAY_CIRCLE, "Try a sample", "Load a ready-made line and look around", p.pick, w);
        if r.clicked() {
            if let Some(f) = &first_sample {
                s.open_seismic(f, None);
            } else {
                s.toast(crate::state::Level::Warn, "No sample data found next to the flows folder");
            }
        }
    });
}

fn card(ui: &mut Ui, p: &Palette, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(p.raised).stroke(Stroke::new(1.0, p.border)).corner_radius(CornerRadius::same(14)).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 6.0;
        add(ui);
    });
}

fn samples(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let list = sample_files(s);
    card(ui, p, |ui| {
        caps(ui, p, "SAMPLE LINES", Some("open one to explore"));
        if list.is_empty() {
            ui.label(RichText::new("Put SEG-Y files in the out folder next to your flows to list them here.").size(11.5).color(p.text_muted));
            return;
        }
        let mut open: Option<PathBuf> = None;
        let n = list.len().min(6);
        let w = ((ui.available_width() - 10.0 * (n.min(3) as f32 - 1.0)) / n.min(3) as f32).max(120.0);
        for chunk in list[..n].chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for f in chunk {
                    let name = f.file_stem().map(|x| x.to_string_lossy().replace('_', " ")).unwrap_or_default();
                    let size = std::fs::metadata(f).map(|m| m.len() as usize).unwrap_or(0);
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 120.0), Sense::click());
                    let painter = ui.painter();
                    let hov = resp.hovered();
                    painter.rect(rect, CornerRadius::same(10), p.sunken, Stroke::new(1.0, if hov { p.accent_line } else { p.border }), egui::StrokeKind::Inside);
                    // Stylised preview: a few wavelets across the card.
                    let pv = egui::Rect::from_min_max(rect.min + vec2(8.0, 8.0), pos2(rect.right() - 8.0, rect.top() + 72.0));
                    painter.rect_filled(pv, CornerRadius::same(6), p.canvas);
                    let seed = name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
                    for k in 0..5 {
                        let y0 = pv.top() + 10.0 + k as f32 * 12.0 + ((seed >> k) % 5) as f32;
                        let pts: Vec<egui::Pos2> = (0..=40)
                            .map(|i| {
                                let x = pv.left() + i as f32 / 40.0 * pv.width();
                                let dip = (i as f32 / 40.0 - 0.5).powi(2) * (8.0 + k as f32 * 3.0);
                                pos2(x, y0 + dip + ((i as f32 * 0.9 + k as f32).sin()) * 1.6)
                            })
                            .collect();
                        painter.add(egui::Shape::line(pts, Stroke::new(1.3, with_alpha(p.text, 90 + k * 25))));
                    }
                    let clip = egui::Rect::from_min_max(pos2(rect.left() + 10.0, rect.top() + 76.0), pos2(rect.right() - 10.0, rect.bottom()));
                    ui.painter().with_clip_rect(clip).text(pos2(rect.left() + 10.0, rect.top() + 82.0), egui::Align2::LEFT_TOP, &name, crate::theme::font_semibold(12.0), p.text);
                    ui.painter().text(pos2(rect.left() + 10.0, rect.top() + 100.0), egui::Align2::LEFT_TOP, crate::data::fmt_bytes(size), egui::FontId::proportional(10.5), p.text_faint);
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(f.display().to_string()).clicked() {
                        open = Some(f.clone());
                    }
                }
            });
        }
        if let Some(f) = open {
            s.open_seismic(&f, None);
        }
    });
}

fn flows(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let mut list: Vec<(PathBuf, std::time::SystemTime)> = flow_files(s).into_iter().map(|f| {
        let t = std::fs::metadata(&f).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        (f, t)
    }).collect();
    list.sort_by(|a, b| b.1.cmp(&a.1));
    card(ui, p, |ui| {
        caps(ui, p, "FLOWS", Some("most recent first"));
        if list.is_empty() {
            ui.label(RichText::new("No flow files yet. Build one in the Process stage and save it.").size(11.5).color(p.text_muted));
        }
        let mut open: Option<PathBuf> = None;
        for (f, t) in list.iter().take(8) {
            let age = t.elapsed().map(|d| ago(d.as_secs())).unwrap_or_default();
            if list_row(ui, p, icon::FLOW_ARROW, &flow_title(f), &age, s.flow.path.as_ref() == Some(f)).clicked() {
                open = Some(f.clone());
            }
        }
        if let Some(f) = open {
            s.request_open_flow(f);
        }
    });
}

fn ago(secs: u64) -> String {
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// Progress ring + per-stage checklist driven by the live stage status.
fn checklist(s: &mut AppState, ui: &mut Ui, p: &Palette) {
    let steps: Vec<(Workspace, &str, StageStatus)> = [
        (Workspace::Data, "Bring in data and geometry"),
        (Workspace::Process, "Run a processing flow"),
        (Workspace::Statics, "Solve statics"),
        (Workspace::Velocity, "Pick velocities"),
        (Workspace::Imaging, "Stack and migrate"),
        (Workspace::Qc, "Check the result"),
    ]
    .into_iter()
    .map(|(w, t)| (w, t, s.stage_status(w)))
    .collect();
    let done = steps.iter().filter(|x| x.2 == StageStatus::Done).count();
    card(ui, p, |ui| {
        caps(ui, p, "YOUR FIRST LINE", None);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
            let c = rect.center();
            let r = 26.0;
            ui.painter().circle_stroke(c, r, Stroke::new(6.0, p.sunken));
            let frac = done as f32 / steps.len() as f32;
            if frac > 0.0 {
                let n = (64.0 * frac).ceil() as usize;
                let pts: Vec<egui::Pos2> = (0..=n)
                    .map(|i| {
                        let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 64.0 * std::f32::consts::TAU;
                        c + vec2(a.cos(), a.sin()) * r
                    })
                    .collect();
                ui.painter().add(egui::Shape::line(pts, Stroke::new(6.0, p.accent)));
            }
            ui.painter().text(c, egui::Align2::CENTER_CENTER, format!("{done}/{}", steps.len()), crate::theme::font_semibold(13.0), p.text);
            ui.vertical(|ui| {
                ui.label(RichText::new(if done == steps.len() { "All done. Nice work." } else { "From field records to an image" }).font(crate::theme::font_semibold(13.0)).color(p.text));
                ui.label(RichText::new("Each step lights up as you go.").size(11.5).color(p.text_muted));
            });
        });
        ui.add_space(4.0);
        let mut go: Option<Workspace> = None;
        for (w, t, st) in &steps {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(6), p.sunken);
            }
            let c = pos2(rect.left() + 14.0, rect.center().y);
            match st {
                StageStatus::Done => {
                    ui.painter().circle_filled(c, 8.0, p.accent);
                    ui.painter().text(c, egui::Align2::CENTER_CENTER, icon::CHECK, egui::FontId::proportional(10.0), p.on_accent);
                }
                StageStatus::Attention => {
                    ui.painter().circle_stroke(c, 7.0, Stroke::new(1.6, p.pick));
                    ui.painter().circle_filled(c, 3.0, p.pick);
                }
                StageStatus::NotStarted => {
                    ui.painter().circle_stroke(c, 7.0, Stroke::new(1.4, p.border_strong));
                }
            }
            let fg = if *st == StageStatus::Done { p.text_muted } else { p.text };
            ui.painter().text(pos2(rect.left() + 32.0, rect.center().y), egui::Align2::LEFT_CENTER, *t, egui::FontId::proportional(12.5), fg);
            ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, w.title(), egui::FontId::proportional(10.5), p.text_faint);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                go = Some(*w);
            }
        }
        if let Some(w) = go {
            s.stage_request = Some(w);
        }
    });
}
