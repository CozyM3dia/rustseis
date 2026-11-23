//! Dockable panes. Each pane is a function over [`AppState`]; [`PaneViewer`] adapts them to
//! `egui_dock`.

pub mod catalog;
pub mod console;
pub mod copilot;
pub mod flow_editor;
pub mod geometry;
pub mod inspector;
pub mod iva;
pub mod picks;
pub mod project;
pub mod spectral;
pub mod statics;
pub mod viewer;
pub mod vsp;

use crate::state::AppState;
use egui::{RichText, Ui, WidgetText};
use egui_phosphor::regular as icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Project,
    Catalog,
    FlowEditor,
    Inspector,
    Console,
    Viewer,
    Picks,
    Headers,
    Iva,
    VelTable,
    Spectrum,
    Fk,
    Radon,
    Statics,
    ScAmp,
    FbQc,
    Vsp,
    GeomTable,
    Basemap,
    Copilot,
}

impl Pane {
    pub fn title(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::Catalog => "Process Library",
            Self::FlowEditor => "Flow",
            Self::Inspector => "Parameters",
            Self::Console => "Job Console",
            Self::Viewer => "Trace Display",
            Self::Picks => "Picks & Edits",
            Self::Headers => "Trace Headers",
            Self::Iva => "Velocity Analysis",
            Self::VelTable => "Velocity Table",
            Self::Spectrum => "Amplitude Spectrum",
            Self::Fk => "F-K Spectrum",
            Self::Radon => "Radon τ-q",
            Self::Statics => "Statics QC",
            Self::ScAmp => "Surface-Consistent Amp",
            Self::FbQc => "First Breaks",
            Self::Vsp => "VSP / DAS",
            Self::GeomTable => "Geometry",
            Self::Basemap => "Basemap",
            Self::Copilot => "Copilot",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Project => icon::TREE_STRUCTURE,
            Self::Catalog => icon::PUZZLE_PIECE,
            Self::FlowEditor => icon::FLOW_ARROW,
            Self::Inspector => icon::SLIDERS_HORIZONTAL,
            Self::Console => icon::TERMINAL_WINDOW,
            Self::Viewer => icon::WAVEFORM,
            Self::Picks => icon::CROSSHAIR,
            Self::Headers => icon::LIST_CHECKS,
            Self::Iva => icon::CHART_LINE_UP,
            Self::VelTable => icon::TABLE,
            Self::Spectrum => icon::WAVE_SINE,
            Self::Fk => icon::POLYGON,
            Self::Radon => icon::FUNNEL,
            Self::Statics => icon::RULER,
            Self::ScAmp => icon::GAUGE,
            Self::FbQc => icon::LIGHTNING,
            Self::Vsp => icon::DROP,
            Self::GeomTable => icon::GRID_FOUR,
            Self::Basemap => icon::MAP_TRIFOLD,
            Self::Copilot => icon::ROBOT,
        }
    }
}

pub struct PaneViewer<'a> {
    pub state: &'a mut AppState,
}

impl egui_dock::TabViewer for PaneViewer<'_> {
    type Tab = Pane;

    fn id(&mut self, tab: &mut Pane) -> egui::Id {
        egui::Id::unique(("rustseis-pane", *tab as u32))
    }

    fn title(&mut self, tab: &mut Pane) -> WidgetText {
        RichText::new(format!("{}  {}", tab.icon(), tab.title())).size(12.0).into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Pane) {
        let s = &mut *self.state;
        match tab {
            Pane::Project => project::ui(s, ui),
            Pane::Catalog => catalog::ui(s, ui),
            Pane::FlowEditor => flow_editor::ui(s, ui),
            Pane::Inspector => inspector::ui(s, ui),
            Pane::Console => console::ui(s, ui),
            Pane::Viewer => viewer::ui(s, ui),
            Pane::Picks => picks::ui(s, ui),
            Pane::Headers => picks::headers_ui(s, ui),
            Pane::Iva => iva::ui(s, ui),
            Pane::VelTable => iva::table_ui(s, ui),
            Pane::Spectrum => spectral::spectrum_ui(s, ui),
            Pane::Fk => spectral::fk_ui(s, ui),
            Pane::Radon => spectral::radon_ui(s, ui),
            Pane::Statics => statics::statics_ui(s, ui),
            Pane::ScAmp => statics::scamp_ui(s, ui),
            Pane::FbQc => statics::fb_ui(s, ui),
            Pane::Vsp => vsp::ui(s, ui),
            Pane::GeomTable => geometry::table_ui(s, ui),
            Pane::Basemap => geometry::basemap_ui(s, ui),
            Pane::Copilot => copilot::ui(s, ui),
        }
    }

    fn is_closeable(&self, _tab: &Pane) -> bool {
        false
    }

    fn scroll_bars(&self, _tab: &Pane) -> [bool; 2] {
        [false, false]
    }
}

/// Common "engine busy" overlay message for panes that need the live session.
pub fn busy_notice(s: &AppState, ui: &mut Ui) -> bool {
    if let Some((label, elapsed, _)) = s.busy_info() {
        let p = crate::theme::palette(s.theme);
        crate::canvas::empty_state(
            ui,
            p,
            icon::HOURGLASS,
            "Engine busy",
            &format!("{label} · {:.1} s — views resume when it finishes", elapsed.as_secs_f32()),
        );
        true
    } else {
        false
    }
}
