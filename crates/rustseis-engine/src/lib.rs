use chrono::{DateTime, Utc};
use rustseis_core::{
    FirstBreakTable, MuteTable, Result, SeismicError, StaticsTable, SurfaceConsistentAmpTable,
    Trace, VelocityTable,
};
use rustseis_flow::{FlowContext, FlowReport, FlowSpec, execute_flow};
use rustseis_io::OpfDatabase;
use rustseis_render::{
    GatherRenderConfig, render_fk_spectrum_plot, render_gather_with_picks, render_radon_panel,
    render_seismic_gather, render_spectrum_plot,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// VISTA-style Shared Selection & Crosshair Bus (links Trace Display, Basemap,
/// Geometry Spreadsheet, Header Table, IVA, Radon, VSP, and Spectral/F-K Inspector in L6 GUI).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CrosshairState {
    pub active_dataset: Option<String>,
    pub trace_index: Option<usize>,
    pub ffid: Option<i32>,
    pub cdp: Option<i32>,
    pub chan: Option<i32>,
    pub offset_m: Option<f32>,
    pub time_ms: Option<f32>,
    pub map_x: Option<f64>,
    pub map_y: Option<f64>,
    pub depth_m: Option<f32>,
}

/// ProMAX-compatible `Area -> Line -> Flows` project manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub id: Uuid,
    pub area: String,
    pub line: String,
    pub created_at: DateTime<Utc>,
    pub flows: Vec<String>,
    pub datasets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandPolicy {
    ReadOnly,
    Mutating,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineCommand {
    SetCrosshair(CrosshairState),
    UpsertVelocityTable(VelocityTable),
    UpsertMuteTable(MuteTable),
    UpsertFirstBreakTable(FirstBreakTable),
    UpsertStaticsTable(StaticsTable),
    UpsertScAmpTable(SurfaceConsistentAmpTable),
    SaveOpfDatabase {
        db_path: PathBuf,
    },
    LoadOpfDatabase {
        db_path: PathBuf,
    },
    RunFlow(FlowSpec),
    RenderGatherPng {
        output_path: PathBuf,
        config: GatherRenderConfig,
    },
    RenderGatherWithPicksPng {
        output_path: PathBuf,
        config: GatherRenderConfig,
        picks_ms: Vec<Option<f32>>,
    },
    RenderSpectrumPng {
        output_path: PathBuf,
        width: u32,
        height: u32,
    },
    RenderFkPng {
        output_path: PathBuf,
        dx_m: f32,
        width: u32,
        height: u32,
    },
    RenderRadonPng {
        panel_name: String,
        output_path: PathBuf,
        zone_bounds_ms: Option<(f32, f32)>,
        width: u32,
        height: u32,
    },
}

impl EngineCommand {
    pub fn policy(&self) -> CommandPolicy {
        match self {
            Self::SetCrosshair(_)
            | Self::RenderGatherPng { .. }
            | Self::RenderGatherWithPicksPng { .. }
            | Self::RenderSpectrumPng { .. }
            | Self::RenderFkPng { .. }
            | Self::RenderRadonPng { .. }
            | Self::SaveOpfDatabase { .. } => CommandPolicy::ReadOnly,
            Self::UpsertVelocityTable(_)
            | Self::UpsertMuteTable(_)
            | Self::UpsertFirstBreakTable(_)
            | Self::UpsertStaticsTable(_)
            | Self::UpsertScAmpTable(_)
            | Self::LoadOpfDatabase { .. }
            | Self::RunFlow(_) => CommandPolicy::Mutating,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandAuditEntry {
    pub timestamp: DateTime<Utc>,
    pub summary: String,
    pub policy: CommandPolicy,
}

/// Unified L4 Engine Session — single command bus shared by CLI, MCP, and future Opus `L6 ui-egui`.
pub struct EngineSession {
    pub root_dir: Option<PathBuf>,
    pub manifest: Option<ProjectManifest>,
    pub crosshair: CrosshairState,
    pub context: FlowContext,
    pub active_traces: Vec<Trace>,
    pub last_report: Option<FlowReport>,
    pub audit_log: Vec<CommandAuditEntry>,
    undo_stack: Vec<Vec<Trace>>,
    redo_stack: Vec<Vec<Trace>>,
}

impl Default for EngineSession {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineSession {
    pub fn new() -> Self {
        Self {
            root_dir: None,
            manifest: None,
            crosshair: CrosshairState::default(),
            context: FlowContext::new(),
            active_traces: Vec::new(),
            last_report: None,
            audit_log: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn init_project(
        &mut self,
        root: impl AsRef<Path>,
        area: impl Into<String>,
        line: impl Into<String>,
    ) -> Result<ProjectManifest> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("datasets"))?;
        fs::create_dir_all(root.join("flows"))?;
        fs::create_dir_all(root.join("tables"))?;
        fs::create_dir_all(root.join("qc"))?;

        let _ = OpfDatabase::open(root.join("tables").join("project.opf.sqlite"))?;

        let manifest = ProjectManifest {
            id: Uuid::new_v4(),
            area: area.into(),
            line: line.into(),
            created_at: Utc::now(),
            flows: Vec::new(),
            datasets: Vec::new(),
        };
        let bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| SeismicError::Serialization(e.to_string()))?;
        fs::write(root.join("project.json"), bytes)?;
        self.root_dir = Some(root);
        self.manifest = Some(manifest.clone());
        Ok(manifest)
    }

    /// Dispatch a command through the unified Command Bus (records audit trail & undo snapshot).
    pub fn dispatch(&mut self, cmd: EngineCommand) -> Result<serde_json::Value> {
        let policy = cmd.policy();
        let (summary, output) = match cmd {
            EngineCommand::SetCrosshair(ch) => {
                self.crosshair = ch.clone();
                ("SetCrosshair".into(), serde_json::to_value(ch).unwrap())
            }
            EngineCommand::UpsertVelocityTable(vt) => {
                let name = vt.name.clone();
                self.context.velocities.insert(name.clone(), vt);
                (format!("UpsertVelocityTable({name})"), serde_json::json!({"saved": name}))
            }
            EngineCommand::UpsertMuteTable(mt) => {
                let name = mt.name.clone();
                self.context.mutes.insert(name.clone(), mt);
                (format!("UpsertMuteTable({name})"), serde_json::json!({"saved": name}))
            }
            EngineCommand::UpsertFirstBreakTable(fb) => {
                let name = fb.name.clone();
                self.context.first_breaks.insert(name.clone(), fb);
                (format!("UpsertFirstBreakTable({name})"), serde_json::json!({"saved": name}))
            }
            EngineCommand::UpsertStaticsTable(st) => {
                let name = st.name.clone();
                self.context.statics.insert(name.clone(), st);
                (format!("UpsertStaticsTable({name})"), serde_json::json!({"saved": name}))
            }
            EngineCommand::UpsertScAmpTable(sc) => {
                let name = sc.name.clone();
                self.context.sc_amps.insert(name.clone(), sc);
                (format!("UpsertScAmpTable({name})"), serde_json::json!({"saved": name}))
            }
            EngineCommand::SaveOpfDatabase { db_path } => {
                let db = OpfDatabase::open(&db_path)?;
                for vt in self.context.velocities.values() {
                    db.save_velocity(vt)?;
                }
                for mt in self.context.mutes.values() {
                    db.save_mute(mt)?;
                }
                for fb in self.context.first_breaks.values() {
                    db.save_first_breaks(fb)?;
                }
                for st in self.context.statics.values() {
                    db.save_statics(st)?;
                }
                for sc in self.context.sc_amps.values() {
                    db.save_sc_amp(sc)?;
                }
                if let Some(ref rep) = self.last_report {
                    let rjson = serde_json::to_string(rep).unwrap_or_default();
                    db.record_flow_run(&rep.flow_name, rep.total_elapsed_ms, rep.final_trace_count, &rjson)?;
                }
                let entries = db.list_entries()?;
                (
                    format!("SaveOpfDatabase({})", db_path.display()),
                    serde_json::json!({"db_path": db_path, "saved_tables": entries.len()}),
                )
            }
            EngineCommand::LoadOpfDatabase { db_path } => {
                let db = OpfDatabase::open(&db_path)?;
                let mut count = 0usize;
                for (k, v) in db.load_all_velocities()? {
                    self.context.velocities.insert(k, v);
                    count += 1;
                }
                for (k, v) in db.load_all_mutes()? {
                    self.context.mutes.insert(k, v);
                    count += 1;
                }
                for (k, v) in db.load_all_first_breaks()? {
                    self.context.first_breaks.insert(k, v);
                    count += 1;
                }
                for (k, v) in db.load_all_statics()? {
                    self.context.statics.insert(k, v);
                    count += 1;
                }
                for (k, v) in db.load_all_sc_amps()? {
                    self.context.sc_amps.insert(k, v);
                    count += 1;
                }
                (
                    format!("LoadOpfDatabase({})", db_path.display()),
                    serde_json::json!({"db_path": db_path, "loaded_tables": count}),
                )
            }
            EngineCommand::RunFlow(spec) => {
                let flow_name = spec.name.clone();
                if !self.active_traces.is_empty() {
                    self.undo_stack.push(self.active_traces.clone());
                    self.redo_stack.clear();
                }
                let prev = std::mem::take(&mut self.active_traces);
                let (out_traces, report) = execute_flow(&spec, prev, &mut self.context)?;
                self.active_traces = out_traces;
                self.last_report = Some(report.clone());
                let val = serde_json::to_value(&report)
                    .map_err(|e| SeismicError::Serialization(e.to_string()))?;
                (format!("RunFlow({flow_name})"), val)
            }
            EngineCommand::RenderGatherPng { output_path, config } => {
                let img = render_seismic_gather(&self.active_traces, &config)?;
                img.save_png(&output_path)?;
                (
                    format!("RenderGatherPng({})", output_path.display()),
                    serde_json::json!({"path": output_path, "width": img.width, "height": img.height}),
                )
            }
            EngineCommand::RenderGatherWithPicksPng {
                output_path,
                config,
                picks_ms,
            } => {
                let img = render_gather_with_picks(&self.active_traces, &config, &picks_ms)?;
                img.save_png(&output_path)?;
                (
                    format!("RenderGatherWithPicksPng({})", output_path.display()),
                    serde_json::json!({"path": output_path, "width": img.width, "height": img.height}),
                )
            }
            EngineCommand::RenderSpectrumPng { output_path, width, height } => {
                let img = render_spectrum_plot(&self.active_traces, width, height)?;
                img.save_png(&output_path)?;
                (
                    format!("RenderSpectrumPng({})", output_path.display()),
                    serde_json::json!({"path": output_path, "width": img.width, "height": img.height}),
                )
            }
            EngineCommand::RenderFkPng { output_path, dx_m, width, height } => {
                let img = render_fk_spectrum_plot(&self.active_traces, dx_m, width, height)?;
                img.save_png(&output_path)?;
                (
                    format!("RenderFkPng({})", output_path.display()),
                    serde_json::json!({"path": output_path, "width": img.width, "height": img.height}),
                )
            }
            EngineCommand::RenderRadonPng {
                panel_name,
                output_path,
                zone_bounds_ms,
                width,
                height,
            } => {
                let panel = self.context.radon_panels.get(&panel_name).ok_or_else(|| {
                    SeismicError::NotFound(format!("Radon panel '{panel_name}' not found"))
                })?;
                let img = render_radon_panel(
                    &panel.times_ms,
                    &panel.q_values_ms,
                    &panel.data,
                    zone_bounds_ms,
                    width,
                    height,
                )?;
                img.save_png(&output_path)?;
                (
                    format!("RenderRadonPng({})", output_path.display()),
                    serde_json::json!({"path": output_path, "width": img.width, "height": img.height}),
                )
            }
        };

        self.audit_log.push(CommandAuditEntry {
            timestamp: Utc::now(),
            summary,
            policy,
        });
        Ok(output)
    }

    pub fn undo(&mut self) -> bool {
        if let Some(prev) = self.undo_stack.pop() {
            let cur = std::mem::replace(&mut self.active_traces, prev);
            self.redo_stack.push(cur);
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(next) = self.redo_stack.pop() {
            let cur = std::mem::replace(&mut self.active_traces, next);
            self.undo_stack.push(cur);
            true
        } else {
            false
        }
    }
}
