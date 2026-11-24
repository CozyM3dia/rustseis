//! Documents and side panels. Each is a function over [`AppState`].

pub mod catalog;
pub mod console;
pub mod copilot;
pub mod flow_editor;
pub mod geometry;
pub mod inspector;
pub mod iva;
pub mod picks;
pub mod preview;
pub mod project;
pub mod spectral;
pub mod start;
pub mod statics;
pub mod viewer;
pub mod vsp;

use crate::state::AppState;
use egui::Ui;
use egui_phosphor::regular as icon;

/// Common "engine busy" overlay message for panes that need the live session.
pub fn busy_notice(s: &AppState, ui: &mut Ui) -> bool {
    if let Some((label, elapsed, _)) = s.busy_info() {
        let p = crate::theme::palette(s.theme);
        crate::canvas::empty_state(
            ui,
            p,
            icon::HOURGLASS,
            "Working…",
            &format!("{label} · {:.1} s. This view refreshes when it finishes.", elapsed.as_secs_f32()),
        );
        true
    } else {
        false
    }
}
