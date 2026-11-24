//! Scripted screenshot tour (`rustseis-desktop --tour <dir>`): runs real flows, drives every
//! workspace into a representative state, and saves exact frame captures via
//! `ViewportCommand::Screenshot`. Used for visual QA and documentation.

use crate::app::RustSeisApp;
use crate::data::TraceSource;
use crate::panes::viewer::PickMode;
use crate::state::Doc;
use crate::theme::ThemeMode;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type Setup = Box<dyn Fn(&mut RustSeisApp)>;

pub enum Step {
    OpenFlow(PathBuf),
    Run,
    /// Wait until the engine and compute pool have been idle for a few frames.
    Settle,
    Do(Setup),
    Pause(f64),
    Shot(String),
    Quit,
}

pub struct Tour {
    dir: PathBuf,
    steps: Vec<Step>,
    idx: usize,
    idle_frames: u32,
    pause_until: Option<f64>,
    pending_shot: Option<Arc<Mutex<Option<String>>>>,
    pub log: Vec<String>,
}

impl Tour {
    pub fn new(dir: impl Into<PathBuf>, steps: Vec<Step>) -> Self {
        Self { dir: dir.into(), steps, idx: 0, idle_frames: 0, pause_until: None, pending_shot: None, log: Vec::new() }
    }

    pub fn finished(&self) -> bool {
        self.idx >= self.steps.len()
    }

    /// Default tour over the bundled demo flows in `flows_dir`.
    pub fn standard(dir: impl Into<PathBuf>, flows_dir: &Path) -> Self {
        let prestack = flows_dir.join("ui_qc_prestack_demo.json");
        let vsp = flows_dir.join("otway_das_vsp_demo.json");
        let buf = |n: &str| TraceSource::Buffer(n.to_string());
        let mut s: Vec<Step> = vec![
            Step::Do(Box::new(|a| a.open(Doc::Start))),
            Step::Pause(0.8),
            Step::Shot("00_start".into()),
            Step::OpenFlow(prestack),
            Step::Run,
            Step::Settle,
            Step::Do(Box::new(|a| {
                a.open(Doc::Flow);
                a.state.flow_sel = Some(vec![11]);
            })),
            Step::Pause(0.6),
            Step::Shot("01_flow_studio".into()),
        ];
        // Trace viewer: shot gather with first-break picks.
        let b = buf("shot_gathers");
        s.push(Step::Do(Box::new(move |a| {
            a.open(Doc::Viewer);
            let st = &mut a.state;
            st.focus.source = b.clone();
            st.focus.key = "FFID".into();
            st.focus.ens = 12;
            st.viewer.request_refit = true;
            st.viewer.label_key = "CHAN".into();
            st.viewer.mode = PickMode::FirstBreak;
            if let Some(t) = st.session().and_then(|x| x.context.first_breaks.get("nias_fb")).cloned() {
                for pk in t.picks {
                    st.viewer.fb_picks.insert((pk.ffid, pk.chan), pk.time_ms);
                }
            }
        })));
        s.extend([Step::Settle, Step::Pause(0.5), Step::Shot("02_viewer_first_breaks".into())]);
        // Mute picking with preview.
        s.push(Step::Do(Box::new(|a| {
            let v = &mut a.state.viewer;
            v.mode = PickMode::Mute;
            v.mute_picks = vec![(100.0, 120.0), (600.0, 420.0), (1200.0, 820.0)];
            v.mute_preview = true;
        })));
        s.extend([Step::Settle, Step::Pause(0.4), Step::Shot("03_viewer_mute_preview".into())]);
        // Before / after Radon compare on NMO CDP gathers.
        let (b1, b2) = (buf("nmo_before_radon"), buf("nmo_after_radon"));
        s.push(Step::Do(Box::new(move |a| {
            let st = &mut a.state;
            st.viewer.mode = PickMode::Navigate;
            st.viewer.mute_preview = false;
            st.focus.source = b1.clone();
            st.focus.key = "CDP".into();
            st.ensure_index();
            st.focus.ens = st.index.ranges.len() / 2;
            st.viewer.compare = true;
            st.viewer.compare_source = b2.clone();
            st.viewer.label_key = "OFFSET".into();
            st.viewer.request_refit = true;
        })));
        s.extend([Step::Settle, Step::Pause(0.5), Step::Shot("04_viewer_before_after_radon".into())]);
        // Imaging stage ribbon view (Depth Model / Kirchhoff PSDM / 2D RTM GPU).
        s.push(Step::Do(Box::new(|a| {
            a.stage = crate::state::Workspace::Imaging;
            a.left_tab = crate::app::LeftTab::Library;
            a.left_open = true;
        })));
        s.extend([Step::Pause(0.4), Step::Shot("04b_imaging_ribbon_library".into())]);
        s.push(Step::Do(Box::new(|a| {
            a.left_tab = crate::app::LeftTab::Explorer;
        })));
        // Velocity analysis on CDP gathers with the auto-picked table.
        let b = buf("cdp_gathers");
        s.push(Step::Do(Box::new(move |a| {
            a.open(Doc::Velocity);
            a.state.viewer.compare = false;
            a.state.iva.source = b.clone();
            a.state.iva.select_table("nias_vel");
        })));
        s.extend([Step::Pause(0.3), Step::Settle, Step::Pause(0.4), Step::Shot("05_velocity_analysis".into())]);
        // Spectral QC on a shot gather + the flow's Radon panel.
        let b = buf("shot_gathers");
        s.push(Step::Do(Box::new(move |a| {
            a.open(Doc::Spectral);
            let st = &mut a.state;
            st.focus.source = b.clone();
            st.focus.key = "FFID".into();
            st.focus.ens = 12;
            st.spectral.radon_source = Some("nias_radon_qc".into());
        })));
        s.extend([Step::Pause(0.3), Step::Settle, Step::Pause(0.4), Step::Shot("06_spectral_fk_radon".into())]);
        let b = buf("nmo_before_radon");
        s.push(Step::Do(Box::new(move |a| {
            let st = &mut a.state;
            st.focus.source = b.clone();
            st.focus.key = "CDP".into();
            st.ensure_index();
            st.focus.ens = st.index.ranges.len() / 2;
            st.spectral.radon_source = None;
            st.spectral.request_radon_preview();
        })));
        s.extend([Step::Pause(0.3), Step::Settle, Step::Pause(0.4), Step::Shot("07_spectral_radon_demultiple".into())]);
        // Statics workspace.
        s.push(Step::Do(Box::new(|a| a.open(Doc::Statics))));
        s.extend([Step::Pause(0.3), Step::Settle, Step::Pause(0.3), Step::Shot("08_statics".into())]);
        // Start page with a project loaded.
        s.push(Step::Do(Box::new(|a| a.open(Doc::Start))));
        s.extend([Step::Pause(0.5), Step::Shot("08b_start_after_run".into())]);
        // Survey: geometry, basemap, copilot.
        let b = buf("shot_gathers");
        s.push(Step::Do(Box::new(move |a| {
            a.open(Doc::Survey);
            a.state.geometry.source = b.clone();
            a.state.focus.source = b.clone();
            a.state.focus_trace(1500);
            a.right_mode = crate::app::RightMode::Assistant;
            crate::panes::copilot::tour_seed(&mut a.state);
        })));
        s.extend([Step::Pause(0.3), Step::Settle, Step::Pause(0.6), Step::Shot("09_survey_geometry_copilot".into())]);
        // Light theme, viewer.
        let b = buf("shot_gathers");
        s.push(Step::Do(Box::new(move |a| {
            a.right_mode = crate::app::RightMode::Inspector;
            a.state.theme = ThemeMode::Light;
            a.open(Doc::Viewer);
            a.state.focus.source = b.clone();
            a.state.focus.key = "FFID".into();
            a.state.focus.ens = 20;
            a.state.viewer.mode = PickMode::Navigate;
            a.state.viewer.colormap = rustseis_render::SeismicColormap::Greyscale;
            a.state.viewer.request_refit = true;
        })));
        s.extend([Step::Settle, Step::Pause(0.5), Step::Shot("10_light_theme_viewer".into())]);
        s.push(Step::Do(Box::new(|a| {
            a.state.theme = ThemeMode::Dark;
            a.state.viewer.colormap = rustseis_render::SeismicColormap::BlueWhiteRed;
        })));
        // Borehole DAS / VSP.
        if vsp.exists() {
            s.extend([Step::OpenFlow(vsp), Step::Run, Step::Settle]);
            s.push(Step::Do(Box::new(|a| {
                a.open(Doc::Borehole);
                a.state.vsp.source = TraceSource::Active;
            })));
            s.extend([Step::Pause(0.3), Step::Settle, Step::Pause(0.5), Step::Shot("11_vsp_das".into())]);
        }
        s.push(Step::Quit);
        Self::new(dir, s)
    }

    /// Practicum tour (`--tour-practicum <dir>`): runs the Modules 1–7 starter flow on the real
    /// data and captures each practicum control (survey charts, F-K polygon, autocorrelation,
    /// mute / kill / decon gate panels, velocity analysis, step preview, ribbons).
    pub fn practicum(dir: impl Into<PathBuf>, flows_dir: &Path) -> Self {
        use crate::panes::{geometry::SurveyView, spectral::LeftView};
        use crate::state::Workspace;
        let flow = flows_dir.join("praktikum_2d_marine_modul_1_7.json");
        let buf = |n: &str| TraceSource::Buffer(n.to_string());
        let shot = |name: &str| [Step::Settle, Step::Pause(0.6), Step::Shot(name.into())];
        let mut s: Vec<Step> = vec![Step::OpenFlow(flow), Step::Run, Step::Settle];
        s.push(Step::Do(Box::new(|a| {
            a.stage = Workspace::Process;
            a.open(Doc::Flow);
            a.state.flow_sel = Some(vec![9]);
        })));
        s.extend(shot("p01_flow_process_ribbon"));
        let b = buf("01_geometry");
        s.push(Step::Do(Box::new(move |a| {
            a.open(Doc::Survey);
            a.state.geometry.source = b.clone();
            a.state.focus.source = b.clone();
            a.state.focus_trace(1500);
            a.state.geometry.view = SurveyView::StackingChart;
        })));
        s.extend(shot("p02_survey_stacking_chart"));
        s.push(Step::Do(Box::new(|a| a.state.geometry.view = SurveyView::Fold)));
        s.extend(shot("p03_survey_fold"));
        let b = buf("01_geometry");
        s.push(Step::Do(Box::new(move |a| {
            a.open(Doc::Spectral);
            let st = &mut a.state;
            st.focus.source = b.clone();
            st.focus.key = "FFID".into();
            st.focus.ens = 20;
            st.spectral.left_view = LeftView::Autocorrelation;
            st.spectral.fk.mode = rustseis_dsp::FkMode::RejectPolygon;
            st.spectral.fk.polygon = vec![(0.0, 0.0), (40.0, -0.03), (40.0, -0.005), (12.0, 0.0), (40.0, 0.005), (40.0, 0.03)];
        })));
        s.extend(shot("p04_spectral_autocorr_fk_polygon"));
        let b = buf("03_edited");
        s.push(Step::Do(Box::new(move |a| {
            a.open(Doc::Viewer);
            let st = &mut a.state;
            st.focus.source = b.clone();
            st.focus.key = "FFID".into();
            st.focus.ens = 20;
            st.viewer.request_refit = true;
            st.viewer.mode = PickMode::Mute;
            st.viewer.mute_picks = vec![(100.0, 60.0), (700.0, 460.0), (1288.0, 860.0)];
        })));
        s.extend(shot("p05_viewer_mute_add_to_flow"));
        s.push(Step::Do(Box::new(|a| {
            let v = &mut a.state.viewer;
            v.mode = PickMode::DeconGate;
            v.gate_picks = vec![(100.0, 250.0, 2200.0), (1288.0, 950.0, 3200.0)];
        })));
        s.extend(shot("p06_viewer_decon_gate"));
        s.push(Step::Do(Box::new(|a| {
            a.state.viewer.mode = PickMode::Kill;
            a.state.viewer.kill_channels.extend([7, 8]);
            a.state.viewer.reverse_channels.insert(30);
        })));
        s.extend(shot("p07_viewer_kill_save_into_flow"));
        let b = buf("05_cdp_gathers");
        s.push(Step::Do(Box::new(move |a| {
            a.state.viewer.mode = PickMode::Navigate;
            a.state.viewer.kill_channels.clear();
            a.state.viewer.reverse_channels.clear();
            a.stage = Workspace::Velocity;
            a.open(Doc::Velocity);
            a.state.iva.source = b.clone();
            a.state.iva.select_table("praktikum_vel");
            a.state.iva.super_half = 7;
            a.state.iva.nmo_eta = 0.05;
        })));
        s.extend([Step::Pause(0.3), Step::Settle]);
        s.extend(shot("p08_velocity_supergather_eta"));
        let b = buf("01_geometry");
        s.push(Step::Do(Box::new(move |a| {
            a.stage = Workspace::Process;
            a.open(Doc::Flow);
            let st = &mut a.state;
            st.focus.source = b.clone();
            st.focus.key = "FFID".into();
            st.focus.ens = 20;
            st.flow_sel = Some(vec![5]);
            crate::panes::preview::request(st, &vec![5]);
        })));
        s.extend(shot("p09_step_preview_inspector"));
        s.push(Step::Do(Box::new(|a| {
            a.open(Doc::Viewer);
            a.state.viewer.request_refit = true;
        })));
        s.extend(shot("p10_step_preview_compare"));
        s.push(Step::Do(Box::new(|a| {
            a.stage = Workspace::Imaging;
            a.open(Doc::Viewer);
            a.state.viewer.compare = false;
            a.state.focus.source = TraceSource::Active;
            a.state.focus.key = crate::data::SECTION_KEY.into();
            a.state.focus.ens = 0;
            a.state.viewer.request_refit = true;
        })));
        s.extend(shot("p11_imaging_ribbon_migrated"));
        s.push(Step::Do(Box::new(|a| a.stage = Workspace::Data)));
        s.extend(shot("p12_data_ribbon_modeling"));
        s.push(Step::Quit);
        Self::new(dir, s)
    }

    /// Advance the tour; called once per frame before the UI is drawn.
    pub fn tick(app: &mut RustSeisApp, ctx: &egui::Context) {
        let Some(mut tour) = app.tour.take() else { return };
        ctx.request_repaint();
        let now = ctx.input(|i| i.time);
        let idle = !app.state.is_busy() && !app.state.compute.any_busy();
        tour.idle_frames = if idle { tour.idle_frames + 1 } else { 0 };

        if let Some(p) = &tour.pending_shot {
            if p.lock().map(|g| g.is_some()).unwrap_or(false) {
                let done = p.lock().ok().and_then(|mut g| g.take()).unwrap_or_default();
                tour.log.push(done);
                tour.pending_shot = None;
            } else {
                app.tour = Some(tour);
                return;
            }
        }
        if let Some(t) = tour.pause_until {
            if now < t {
                app.tour = Some(tour);
                return;
            }
            tour.pause_until = None;
        }
        let Some(step) = tour.steps.get(tour.idx) else {
            app.tour = Some(tour);
            return;
        };
        let mut advance = true;
        match step {
            Step::OpenFlow(p) => app.state.open_flow(p),
            Step::Run => app.state.run_current_flow(),
            Step::Settle => advance = tour.idle_frames >= 8,
            Step::Do(f) => f(app),
            Step::Pause(secs) => tour.pause_until = Some(now + secs),
            Step::Shot(name) => {
                let path = tour.dir.join(format!("{name}.png"));
                let slot = Arc::new(Mutex::new(None));
                let slot2 = Arc::clone(&slot);
                let ctx2 = ctx.clone();
                ctx.request_screenshot(move |img| {
                    let msg = match save_png(&img, &path) {
                        Ok(()) => format!("saved {}", path.display()),
                        Err(e) => format!("FAILED {}: {e}", path.display()),
                    };
                    if let Ok(mut g) = slot2.lock() {
                        *g = Some(msg);
                    }
                    ctx2.request_repaint();
                });
                tour.pending_shot = Some(slot);
            }
            Step::Quit => {
                let _ = std::fs::write(tour.dir.join("tour_log.txt"), tour.log.join("\n"));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if advance {
            tour.idx += 1;
        }
        app.tour = Some(tour);
    }
}

pub fn save_png(img: &egui::ColorImage, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut rgba = Vec::with_capacity(img.pixels.len() * 4);
    for c in &img.pixels {
        rgba.extend_from_slice(&c.to_srgba_unmultiplied());
    }
    let r = rustseis_render::RenderedImage { width: img.size[0] as u32, height: img.size[1] as u32, rgba };
    r.save_png(path).map_err(|e| e.to_string())
}
