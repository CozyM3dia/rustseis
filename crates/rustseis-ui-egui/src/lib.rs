//! RustSeis L6 — pure-Rust egui workstation.
//!
//! Layers: [`theme`] (visual contract) → [`canvas`] / [`widgets`] (drawing primitives) →
//! [`panes`] (six interlinked workspaces) → [`app::RustSeisApp`] (shell). Heavy engine work
//! runs through [`worker`]; all state mutation goes through `EngineSession::dispatch`.

pub mod app;
pub mod canvas;
pub mod data;
pub mod flowdoc;
pub mod panes;
pub mod specs;
pub mod state;
pub mod theme;
pub mod widgets;
pub mod worker;

pub use app::RustSeisApp;
pub use state::{AppState, Workspace};
