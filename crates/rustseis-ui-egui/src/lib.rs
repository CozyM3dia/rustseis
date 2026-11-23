//! RustSeis L6 — pure-Rust egui workstation.
//!
//! Layers: [`theme`] (visual contract) → [`canvas`] / [`widgets`] (drawing primitives) →
//! [`panes`] (documents and side panels) → [`commands`] (command bar) → [`app::RustSeisApp`] (shell). Heavy engine work
//! runs through [`worker`]; all state mutation goes through `EngineSession::dispatch`.

pub mod app;
pub mod canvas;
pub mod commands;
pub mod data;
pub mod flowdoc;
pub mod icons;
pub mod panes;
pub mod ribbon;
pub mod specs;
pub mod state;
pub mod theme;
pub mod ui_copy;
pub mod tour;
pub mod widgets;
pub mod worker;

pub use app::RustSeisApp;
pub use state::{AppState, Workspace};
