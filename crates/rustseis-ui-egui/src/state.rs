//! Application state shared by every pane. All engine mutations go through
//! [`EngineSession::dispatch`] (inline for light table/crosshair commands, on the worker thread
//! for flows, OPF I/O and copilot calls).

use crate::data::{EnsRange, EnsembleIndex, TraceSource, fmt_count, traces_bytes};
use crate::flowdoc::{FlowDoc, FlowNode, StepPath};
use crate::panes::copilot::CopilotState;
use crate::panes::geometry::GeometryState;
use crate::panes::inspector::InspectorState;
use crate::panes::iva::IvaState;
use crate::panes::preview::PreviewState;
use crate::panes::project::ProjectState;
use crate::panes::spectral::SpectralState;
use crate::panes::statics::StaticsState;
use crate::panes::viewer::ViewerState;
use crate::panes::vsp::VspState;
use crate::specs::SpecLibrary;
use crate::theme::ThemeMode;
use crate::worker::{ComputePool, EngineDone, EngineJob, EngineOutcome, EngineWorker};
use rustseis_engine::{CommandAuditEntry, CrosshairState, EngineCommand, EngineSession};
use rustseis_flow::{FlowReport, FlowSpec, ToolDescriptor, full_tool_catalog, tier2_tool_catalog};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Workflow stages shown in the top bar, in processing order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Workspace {
    Data,
    Process,
    Velocity,
    Statics,
    Imaging,
    Qc,
    Borehole,
}

impl Workspace {
    pub const ALL: [Workspace; 7] = [Self::Data, Self::Process, Self::Velocity, Self::Statics, Self::Imaging, Self::Qc, Self::Borehole];
    pub fn title(self) -> &'static str {
        match self {
            Self::Data => "Data",
            Self::Process => "Process",
            Self::Velocity => "Velocity",
            Self::Statics => "Statics",
            Self::Imaging => "Imaging",
            Self::Qc => "QC",
            Self::Borehole => "Borehole",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Self::Data => "Bring in data, assign geometry and check the survey",
            Self::Process => "Build and run processing flows",
            Self::Velocity => "Pick velocities on semblance while gathers flatten live",
            Self::Statics => "First breaks, statics and amplitude corrections",
            Self::Imaging => "Stack and migrate, then inspect the image",
            Self::Qc => "Look at gathers, spectra, F-K and Radon panels",
            Self::Borehole => "Fibre and borehole VSP records",
        }
    }
    /// The document a stage opens.
    pub fn doc(self) -> Doc {
        match self {
            Self::Data => Doc::Survey,
            Self::Process => Doc::Flow,
            Self::Velocity => Doc::Velocity,
            Self::Statics => Doc::Statics,
            Self::Imaging | Self::Qc => Doc::Viewer,
            Self::Borehole => Doc::Borehole,
        }
    }
}

/// Documents shown in the centre panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Doc {
    Start,
    Viewer,
    Flow,
    Velocity,
    Spectral,
    Statics,
    Borehole,
    Survey,
}

impl Doc {
    pub fn title(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Viewer => "Gathers",
            Self::Flow => "Flow",
            Self::Velocity => "Velocity analysis",
            Self::Spectral => "Spectral QC",
            Self::Statics => "Statics QC",
            Self::Borehole => "Borehole",
            Self::Survey => "Survey",
        }
    }
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            Self::Start => i::HOUSE,
            Self::Viewer => i::WAVEFORM,
            Self::Flow => i::FLOW_ARROW,
            Self::Velocity => i::CHART_LINE_UP,
            Self::Spectral => i::WAVE_SINE,
            Self::Statics => i::RULER,
            Self::Borehole => i::DROP,
            Self::Survey => i::MAP_TRIFOLD,
        }
    }
    /// Stage a document belongs to by default.
    pub fn stage(self) -> Workspace {
        match self {
            Self::Start | Self::Survey => Workspace::Data,
            Self::Flow => Workspace::Process,
            Self::Velocity => Workspace::Velocity,
            Self::Statics => Workspace::Statics,
            Self::Viewer | Self::Spectral => Workspace::Qc,
            Self::Borehole => Workspace::Borehole,
        }
    }
}

/// Progress shown as a dot on each stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageStatus {
    NotStarted,
    Attention,
    Done,
}

pub enum EngineSlot {
    Ready(Box<EngineSession>),
    Busy { label: String, started: Instant, eta_ms: Option<u128> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobPurpose {
    Flow { flow_name: String },
    OpenData { label: String },
    TraceEdit,
    Opf,
    Copilot,
    Project,
}

#[derive(Clone, Debug)]
pub struct JobRecord {
    pub label: String,
    pub elapsed: Duration,
    pub ok: bool,
    pub summary: String,
    pub finished: chrono_like::Stamp,
}

/// Tiny wall-clock stamp so the UI crate does not depend on chrono directly.
pub mod chrono_like {
    #[derive(Clone, Copy, Debug)]
    pub struct Stamp(pub u64);
    impl Stamp {
        pub fn now() -> Self {
            Self(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
        }
        pub fn hms(self) -> String {
            let s = self.0 % 86_400;
            format!("{:02}:{:02}:{:02} UTC", s / 3600, (s / 60) % 60, s % 60)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub level: Level,
    pub text: String,
    pub stamp: chrono_like::Stamp,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub level: Level,
    pub text: String,
    pub born: f64,
}

#[derive(Clone, Debug, Default)]
pub struct DataFocus {
    pub source: TraceSource,
    pub key: String,
    pub ens: usize,
}

#[derive(Clone, Debug, Default)]
pub struct DataSummary {
    pub n_traces: usize,
    pub n_samples: usize,
    pub dt_ms: f32,
    pub bytes: usize,
    pub ffid: Option<(i32, i32)>,
    pub cdp: Option<(i32, i32)>,
    pub buffers: usize,
}

impl DataSummary {
    pub fn of(session: &EngineSession) -> Self {
        let t = &session.active_traces;
        let mm = |f: &dyn Fn(&rustseis_core::Trace) -> i32| -> Option<(i32, i32)> {
            if t.is_empty() {
                return None;
            }
            let (mut lo, mut hi) = (i32::MAX, i32::MIN);
            for tr in t.iter().step_by((t.len() / 20_000).max(1)) {
                let v = f(tr);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            Some((lo, hi))
        };
        Self {
            n_traces: t.len(),
            n_samples: t.first().map(|x| x.samples.len()).unwrap_or(0),
            dt_ms: t.first().map(|x| x.dt_ms()).unwrap_or(0.0),
            bytes: traces_bytes(t),
            ffid: mm(&|x| x.headers.ffid),
            cdp: mm(&|x| x.headers.cdp),
            buffers: session.context.buffers.len(),
        }
    }
    pub fn headline(&self) -> String {
        if self.n_traces == 0 {
            return "No traces loaded".into();
        }
        format!(
            "{} tr · {} smp · {:.1} ms · {:.2} s",
            fmt_count(self.n_traces),
            fmt_count(self.n_samples),
            self.dt_ms,
            (self.n_samples.saturating_sub(1)) as f32 * self.dt_ms / 1000.0
        )
    }
}

pub struct AppState {
    pub theme: ThemeMode,
    pub engine: EngineSlot,
    pub worker: EngineWorker,
    pub compute: ComputePool,
    /// Bumped whenever trace data may have changed (flows, undo, edits).
    pub rev: u64,
    /// Bumped whenever OPF tables / panels may have changed.
    pub tables_rev: u64,
    pub catalog: Vec<ToolDescriptor>,
    pub tier2: HashSet<String>,
    pub specs: SpecLibrary,
    pub flow: FlowDoc,
    pub flow_sel: Option<StepPath>,
    pub flow_undo: Vec<FlowDoc>,
    pub flow_redo: Vec<FlowDoc>,
    pub pending_open_flow: Option<PathBuf>,
    pub report: Option<FlowReport>,
    /// Top-level node uids at the time `report` was produced (maps step telemetry to cards).
    pub report_uids: Vec<u64>,
    pub history: Vec<JobRecord>,
    pub audit: Vec<CommandAuditEntry>,
    pub log: Vec<LogLine>,
    pub toasts: Vec<Toast>,
    pub crosshair: CrosshairState,
    pub focus: DataFocus,
    pub index: EnsembleIndex,
    pub summary: DataSummary,
    pub flow_eta: HashMap<String, u128>,
    pub busy_purpose: Option<JobPurpose>,
    pub job_control: rustseis_core::JobControl,
    pub doc_request: Option<Doc>,
    pub stage_request: Option<Workspace>,
    pub flow_dirs: Vec<PathBuf>,
    pub time: f64,
    pub viewer: ViewerState,
    pub iva: IvaState,
    pub spectral: SpectralState,
    pub statics: StaticsState,
    pub vsp: VspState,
    pub geometry: GeometryState,
    pub copilot: CopilotState,
    pub project: ProjectState,
    pub inspector: InspectorState,
    /// Name of the mute table saved last from the gather view (new mute steps link to it).
    pub last_mute_table: Option<String>,
    pub preview: PreviewState,
}

impl AppState {
    pub fn new(ctx: &egui::Context) -> Self {
        let catalog = full_tool_catalog();
        let tier2 = tier2_tool_catalog().into_iter().map(|t| t.menu_id).collect();
        let specs = SpecLibrary::discover();
        let mut flow_dirs = Vec::new();
        if let Some(root) = specs.root.as_ref().and_then(|r| r.parent()) {
            flow_dirs.push(root.join("flows"));
        }
        if let Ok(cwd) = std::env::current_dir() {
            let f = cwd.join("flows");
            if !flow_dirs.contains(&f) {
                flow_dirs.push(f);
            }
        }
        let mut s = Self {
            theme: ThemeMode::Dark,
            engine: EngineSlot::Ready(Box::new(EngineSession::new())),
            worker: EngineWorker::spawn(ctx.clone()),
            compute: ComputePool::spawn(ctx.clone(), 2),
            rev: 1,
            tables_rev: 1,
            catalog,
            tier2,
            specs,
            flow: FlowDoc::default(),
            flow_sel: None,
            flow_undo: Vec::new(),
            flow_redo: Vec::new(),
            pending_open_flow: None,
            report: None,
            report_uids: Vec::new(),
            history: Vec::new(),
            audit: Vec::new(),
            log: Vec::new(),
            toasts: Vec::new(),
            crosshair: CrosshairState::default(),
            focus: DataFocus { source: TraceSource::Active, key: "FFID".into(), ens: 0 },
            index: EnsembleIndex::default(),
            summary: DataSummary::default(),
            flow_eta: HashMap::new(),
            busy_purpose: None,
            job_control: rustseis_core::JobControl::new(),
            doc_request: None,
            stage_request: None,
            flow_dirs,
            time: 0.0,
            viewer: ViewerState::default(),
            iva: IvaState::default(),
            spectral: SpectralState::default(),
            statics: StaticsState::default(),
            vsp: VspState::default(),
            geometry: GeometryState::default(),
            copilot: CopilotState::default(),
            project: ProjectState::default(),
            inspector: InspectorState::default(),
            last_mute_table: None,
            preview: PreviewState::default(),
        };
        s.log(Level::Info, format!(
            "RustSeis ready · {} processing steps",
            s.catalog.len()
        ));
        s
    }

    // ---------------------------------------------------------------- engine access

    pub fn session(&self) -> Option<&EngineSession> {
        match &self.engine {
            EngineSlot::Ready(s) => Some(s),
            EngineSlot::Busy { .. } => None,
        }
    }

    pub fn session_mut(&mut self) -> Option<&mut EngineSession> {
        match &mut self.engine {
            EngineSlot::Ready(s) => Some(s),
            EngineSlot::Busy { .. } => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.engine, EngineSlot::Busy { .. })
    }

    pub fn busy_info(&self) -> Option<(&str, Duration, Option<u128>)> {
        match &self.engine {
            EngineSlot::Busy { label, started, eta_ms } => Some((label.as_str(), started.elapsed(), *eta_ms)),
            _ => None,
        }
    }

    /// Hand the session to the worker. Returns false (with a toast) if it is already away.
    pub fn submit(&mut self, job: EngineJob, purpose: JobPurpose) -> bool {
        let label = job.label();
        let eta = match &purpose {
            JobPurpose::Flow { flow_name } => self.flow_eta.get(flow_name).copied(),
            _ => None,
        };
        let slot = std::mem::replace(&mut self.engine, EngineSlot::Busy { label: label.clone(), started: Instant::now(), eta_ms: eta });
        match slot {
            EngineSlot::Ready(mut session) => {
                self.job_control.reset();
                session.context.job_control = Some(self.job_control.clone());
                self.worker.submit(session, job);
                self.busy_purpose = Some(purpose);
                self.log(Level::Info, format!("▶ {label}"));
                true
            }
            busy => {
                self.engine = busy;
                self.toast(Level::Warn, "Engine is busy — wait for the current job to finish");
                false
            }
        }
    }

    pub fn cancel_job(&mut self) {
        if self.is_busy() {
            self.job_control.cancel();
            self.toast(Level::Warn, "Cancelling current job…");
        }
    }

    pub fn run_flow(&mut self, spec: FlowSpec, purpose: JobPurpose) -> bool {
        self.submit(EngineJob::Command(EngineCommand::RunFlow(spec)), purpose)
    }

    /// Run the editor's flow document.
    pub fn run_current_flow(&mut self) {
        if self.flow.nodes.is_empty() {
            self.toast(Level::Warn, "Flow is empty — add tools from the library first");
            return;
        }
        let spec = self.flow.to_spec();
        self.report_uids = self.flow.nodes.iter().map(|n| n.uid).collect();
        let name = spec.name.clone();
        self.run_flow(spec, JobPurpose::Flow { flow_name: name });
    }

    /// Light commands (table upserts, crosshair) dispatched inline on the UI thread.
    pub fn dispatch_now(&mut self, cmd: EngineCommand) -> Result<Value, String> {
        let is_table = matches!(
            cmd,
            EngineCommand::UpsertVelocityTable(_)
                | EngineCommand::UpsertMuteTable(_)
                | EngineCommand::UpsertFirstBreakTable(_)
                | EngineCommand::UpsertStaticsTable(_)
                | EngineCommand::UpsertScAmpTable(_)
        );
        let Some(session) = self.session_mut() else {
            return Err("engine busy".into());
        };
        let r = session.dispatch(cmd).map_err(|e| e.to_string());
        self.audit = session.audit_log.clone();
        if is_table && r.is_ok() {
            self.tables_rev += 1;
        }
        r
    }

    /// Commit the UI crosshair to the engine bus (audited). Hover updates stay UI-local.
    pub fn commit_crosshair(&mut self) {
        let ch = self.crosshair.clone();
        if self.session().is_some() {
            let _ = self.dispatch_now(EngineCommand::SetCrosshair(ch));
        }
    }

    pub fn undo(&mut self) {
        let did = self.session_mut().map(|s| s.undo()).unwrap_or(false);
        if did {
            self.rev += 1;
            self.after_data_change();
            self.toast(Level::Info, "Undo · restored previous trace set");
        } else {
            self.toast(Level::Warn, "Nothing to undo");
        }
    }

    pub fn redo(&mut self) {
        let did = self.session_mut().map(|s| s.redo()).unwrap_or(false);
        if did {
            self.rev += 1;
            self.after_data_change();
            self.toast(Level::Info, "Redo");
        } else {
            self.toast(Level::Warn, "Nothing to redo");
        }
    }

    /// Snapshot the flow before a structural/parameter edit so Ctrl+Z in the Flow editor undoes it.
    pub fn push_flow_undo(&mut self) {
        self.flow_undo.push(self.flow.clone());
        if self.flow_undo.len() > 64 {
            self.flow_undo.remove(0);
        }
        self.flow_redo.clear();
    }

    pub fn undo_flow(&mut self) {
        if let Some(prev) = self.flow_undo.pop() {
            self.flow_redo.push(self.flow.clone());
            self.flow = prev;
            if self.flow_sel.as_ref().is_some_and(|p| self.flow.get(p).is_none()) {
                self.flow_sel = None;
            }
            self.toast(Level::Info, "Undo flow edit");
        } else {
            self.toast(Level::Warn, "Nothing to undo in this flow");
        }
    }

    pub fn redo_flow(&mut self) {
        if let Some(next) = self.flow_redo.pop() {
            self.flow_undo.push(self.flow.clone());
            self.flow = next;
            if self.flow_sel.as_ref().is_some_and(|p| self.flow.get(p).is_none()) {
                self.flow_sel = None;
            }
            self.toast(Level::Info, "Redo flow edit");
        } else {
            self.toast(Level::Warn, "Nothing to redo in this flow");
        }
    }

    /// Open a flow immediately if the current flow is clean, or prompt if there are unsaved edits.
    pub fn request_open_flow(&mut self, path: PathBuf) {
        if self.flow.dirty && self.flow.path.as_ref() != Some(&path) {
            self.pending_open_flow = Some(path);
        } else {
            self.open_flow(&path);
        }
    }

    fn after_data_change(&mut self) {
        if let Some(s) = self.session() {
            self.summary = DataSummary::of(s);
        }
        let src_ok = match &self.focus.source {
            TraceSource::Active => true,
            TraceSource::Buffer(n) => self.session().is_some_and(|s| s.context.buffers.contains_key(n)),
        };
        if !src_ok {
            self.focus.source = TraceSource::Active;
        }
    }

    /// Drain finished engine jobs. Returns true if a job finished this frame.
    pub fn poll_engine(&mut self) -> bool {
        let Some(done) = self.worker.poll() else { return false };
        let EngineDone { session, label, outcome, elapsed } = done;
        self.audit = session.audit_log.clone();
        self.engine = EngineSlot::Ready(session);
        let purpose = self.busy_purpose.take();
        let mut ok = true;
        let mut summary = String::new();
        match outcome {
            EngineOutcome::Command(Ok(v)) => {
                if let Some(rep) = self.session().and_then(|s| s.last_report.clone()) {
                    if v.get("flow_name").is_some() {
                        summary = format!("{} traces · {}", fmt_count(rep.final_trace_count), crate::data::fmt_ms(rep.total_elapsed_ms as f64));
                        self.flow_eta.insert(rep.flow_name.clone(), rep.total_elapsed_ms);
                        self.report = Some(rep);
                    }
                }
                if summary.is_empty() {
                    summary = compact_json(&v, 120);
                }
            }
            EngineOutcome::Command(Err(e)) => {
                ok = false;
                summary = e;
            }
            EngineOutcome::Mcp { call_id, result } => {
                ok = result.is_ok();
                summary = match &result {
                    Ok(v) => compact_json(v, 120),
                    Err(e) => e.clone(),
                };
                if result.as_ref().is_ok_and(|v| v.get("flow_name").is_some()) {
                    if let Some(rep) = self.session().and_then(|s| s.last_report.clone()) {
                        self.report = Some(rep);
                        self.report_uids.clear();
                    }
                }
                self.copilot.on_result(call_id, result);
            }
            EngineOutcome::Project(r) => {
                ok = r.is_ok();
                summary = match r {
                    Ok(v) => {
                        self.project.manifest = Some(v.clone());
                        compact_json(&v, 120)
                    }
                    Err(e) => e,
                };
            }
        }
        // Any engine job may have touched traces or tables.
        self.rev += 1;
        self.tables_rev += 1;
        self.after_data_change();
        if self.focus.source == TraceSource::Active
            && self.focus.key == "FFID"
            && self.summary.n_traces > 1
            && self.summary.ffid.is_some_and(|(a, b)| a == b)
        {
            // Post-stack output (single FFID): show it as a section.
            self.focus.key = crate::data::SECTION_KEY.into();
            self.focus.ens = 0;
        }
        self.history.push(JobRecord { label: label.clone(), elapsed, ok, summary: summary.clone(), finished: chrono_like::Stamp::now() });
        if ok {
            self.log(Level::Ok, format!("✓ {label} · {}", crate::data::fmt_ms(elapsed.as_secs_f64() * 1000.0)));
            match &purpose {
                Some(JobPurpose::OpenData { label }) => {
                    self.focus.ens = 0;
                    self.viewer.request_refit = true;
                    self.toast(Level::Ok, format!("Loaded {label} · {}", self.summary.headline()));
                    self.doc_request = Some(Doc::Viewer);
                }
                Some(JobPurpose::Flow { .. }) => {
                    self.viewer.request_refit = true;
                    self.toast(Level::Ok, format!("{label} finished · {summary}"));
                }
                Some(JobPurpose::TraceEdit) => self.toast(Level::Ok, "Trace edits applied (Ctrl+Z to undo)"),
                Some(JobPurpose::Opf) => self.toast(Level::Ok, format!("{label} · {summary}")),
                _ => {}
            }
        } else {
            self.log(Level::Error, format!("✗ {label}: {summary}"));
            self.toast(Level::Error, format!("{label} failed: {summary}"));
        }
        true
    }

    /// Live status dot for a stage, derived from what the session holds.
    pub fn stage_status(&self, w: Workspace) -> StageStatus {
        let ran = |tools: &[&str]| -> bool {
            self.report.as_ref().is_some_and(|r| r.steps.iter().any(|st| st.enabled && tools.contains(&st.tool.as_str())))
        };
        let ctx = self.session().map(|x| &x.context);
        match w {
            Workspace::Data => {
                if self.summary.n_traces == 0 {
                    StageStatus::NotStarted
                } else if self.summary.cdp.is_some_and(|(a, b)| a != b) {
                    StageStatus::Done
                } else {
                    StageStatus::Attention
                }
            }
            Workspace::Process => match self.history.iter().rev().find(|j| j.label.starts_with("Run flow")) {
                Some(j) if j.ok => StageStatus::Done,
                Some(_) => StageStatus::Attention,
                None => StageStatus::NotStarted,
            },
            Workspace::Velocity => {
                if self.iva.has_unsaved() {
                    StageStatus::Attention
                } else if ctx.is_some_and(|c| !c.velocities.is_empty()) {
                    StageStatus::Done
                } else {
                    StageStatus::NotStarted
                }
            }
            Workspace::Statics => {
                if ctx.is_some_and(|c| !c.statics.is_empty()) {
                    StageStatus::Done
                } else if ctx.is_some_and(|c| !c.first_breaks.is_empty()) {
                    StageStatus::Attention
                } else {
                    StageStatus::NotStarted
                }
            }
            Workspace::Imaging => {
                if ran(&["kirtime2d", "kirtime2dstk", "fdmig", "fkmig2d", "dmofkpanel", "eikonal_psdm", "rtm2d"]) {
                    StageStatus::Done
                } else if ran(&["stack", "crooked_stack"]) {
                    StageStatus::Attention
                } else {
                    StageStatus::NotStarted
                }
            }
            Workspace::Qc => {
                if self.summary.n_traces == 0 {
                    StageStatus::NotStarted
                } else if self.viewer.has_insight() {
                    StageStatus::Attention
                } else {
                    StageStatus::Done
                }
            }
            Workspace::Borehole => {
                if ctx.is_some_and(|c| !c.vsp_profiles.is_empty()) || ran(&["vsp_wavefield_sep", "vsp_geom"]) {
                    StageStatus::Done
                } else {
                    StageStatus::NotStarted
                }
            }
        }
    }

    // ---------------------------------------------------------------- data focus

    /// Rebuild the ensemble index for the focus source/key if stale.
    pub fn ensure_index(&mut self) {
        let Some(session) = self.session() else { return };
        if !self.index.matches(&self.focus.source, &self.focus.key, self.rev) {
            let traces = self.focus.source.traces(session);
            let idx = EnsembleIndex::build(traces, self.focus.source.clone(), &self.focus.key, self.rev);
            self.index = idx;
            if self.focus.ens >= self.index.ranges.len() {
                self.focus.ens = self.index.ranges.len().saturating_sub(1);
            }
        }
    }

    pub fn focus_range(&self) -> Option<EnsRange> {
        self.index.ranges.get(self.focus.ens).copied()
    }

    pub fn step_ensemble(&mut self, delta: i64) {
        let n = self.index.ranges.len() as i64;
        if n == 0 {
            return;
        }
        self.focus.ens = (self.focus.ens as i64 + delta).clamp(0, n - 1) as usize;
    }

    /// Jump the focus to the ensemble containing absolute trace `idx` of the focus source.
    pub fn focus_trace(&mut self, idx: usize) {
        self.ensure_index();
        if let Some(e) = self.index.find_trace(idx) {
            self.focus.ens = e;
        }
        let hdr = self.session().and_then(|s| self.focus.source.traces(s).get(idx).map(|t| t.headers.clone()));
        {
            if let Some(h) = hdr.as_ref() {
                self.crosshair.trace_index = Some(idx);
                self.crosshair.ffid = Some(h.ffid);
                self.crosshair.cdp = Some(h.cdp);
                self.crosshair.chan = Some(h.chan);
                self.crosshair.offset_m = Some(h.offset);
                self.crosshair.map_x = Some(h.cdp_x);
                self.crosshair.map_y = Some(h.cdp_y);
                if h.rec_dep != 0.0 {
                    self.crosshair.depth_m = Some(h.rec_dep);
                }
            }
        }
        self.commit_crosshair();
    }

    /// Update the UI crosshair from a hovered absolute trace index (no audit entry).
    pub fn hover_trace(&mut self, abs_idx: usize, time_ms: Option<f32>) {
        let Some(h) = self.session().and_then(|s| self.focus.source.traces(s).get(abs_idx).map(|t| t.headers.clone())) else { return };
        let h = &h;
        let label = self.focus.source.label();
        let ch = &mut self.crosshair;
        ch.active_dataset = Some(label);
        ch.trace_index = Some(abs_idx);
        ch.ffid = Some(h.ffid);
        ch.cdp = Some(h.cdp);
        ch.chan = Some(h.chan);
        ch.offset_m = Some(h.offset);
        ch.time_ms = time_ms;
        ch.map_x = Some(if h.cdp_x != 0.0 { h.cdp_x } else { h.rec_x });
        ch.map_y = Some(if h.cdp_y != 0.0 { h.cdp_y } else { h.rec_y });
        ch.depth_m = if h.rec_dep != 0.0 { Some(h.rec_dep) } else { None };
    }

    // ---------------------------------------------------------------- notifications

    pub fn toast(&mut self, level: Level, text: impl Into<String>) {
        let text = text.into();
        self.toasts.retain(|t| t.text != text);
        self.toasts.push(Toast { level, text, born: self.time });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    pub fn log(&mut self, level: Level, text: impl Into<String>) {
        self.log.push(LogLine { level, text: text.into(), stamp: chrono_like::Stamp::now() });
        if self.log.len() > 2000 {
            self.log.drain(0..500);
        }
    }

    // ---------------------------------------------------------------- file actions

    pub fn open_seismic_dialog(&mut self) {
        let file = rfd::FileDialog::new()
            .set_title("Open seismic data")
            .add_filter("Seismic", &["sgy", "segy", "segd", "rsds"])
            .add_filter("All files", &["*"])
            .pick_file();
        if let Some(path) = file {
            self.open_seismic(&path, None);
        }
    }

    pub fn open_segd_dir_dialog(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Open SEG-D shot directory").pick_folder() {
            let max = self.project.segd_max_shots;
            self.open_seismic(&dir, Some(max));
        }
    }

    /// Load data through a one-step flow so it lands in the session like any other flow output.
    pub fn open_seismic(&mut self, path: &Path, max_shots: Option<usize>) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let p = path.display().to_string();
        let step = if path.is_dir() || ext == "segd" {
            json!({"tool": "segd_read", "params": {"path": p, "max_shots": max_shots, "live_only": true}})
        } else if ext == "rsds" {
            json!({"tool": "diskread2", "params": {"path": p}})
        } else {
            json!({"tool": "segy_read", "params": {"path": p, "max_traces": null}})
        };
        let label = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(p.clone());
        let spec: FlowSpec = match serde_json::from_value(json!({"name": format!("open_{label}"), "steps": [step]})) {
            Ok(s) => s,
            Err(e) => {
                self.toast(Level::Error, e.to_string());
                return;
            }
        };
        self.focus.source = TraceSource::Active;
        if ext == "sgy" || ext == "segy" {
            // Post-stack sections have no shot grouping; view them whole.
            self.focus.key = crate::data::SECTION_KEY.into();
        } else {
            self.focus.key = "FFID".into();
        }
        self.run_flow(spec, JobPurpose::OpenData { label });
        self.doc_request = Some(Doc::Viewer);
    }

    pub fn open_flow_dialog(&mut self) {
        let mut dlg = rfd::FileDialog::new().set_title("Open flow").add_filter("Flow", &["json", "toml"]);
        if let Some(dir) = self.flow_dirs.iter().find(|d| d.is_dir()) {
            dlg = dlg.set_directory(dir);
        }
        if let Some(path) = dlg.pick_file() {
            self.open_flow(&path);
        }
    }

    pub fn open_flow(&mut self, path: &Path) {
        match FlowDoc::load(path) {
            Ok(doc) => {
                self.toast(Level::Ok, format!("Opened flow {} · {} steps", doc.name, doc.count()));
                self.flow = doc;
                self.flow_sel = None;
                self.flow_undo.clear();
                self.flow_redo.clear();
                self.report_uids.clear();
                self.doc_request = Some(Doc::Flow);
            }
            Err(e) => self.toast(Level::Error, format!("Cannot open flow: {e}")),
        }
    }

    pub fn save_flow(&mut self, save_as: bool) {
        let target = if save_as || self.flow.path.is_none() {
            let mut dlg = rfd::FileDialog::new()
                .set_title("Save flow")
                .add_filter("JSON flow", &["json"])
                .add_filter("TOML flow", &["toml"])
                .set_file_name(format!("{}.json", self.flow.name));
            if let Some(dir) = self.flow_dirs.iter().find(|d| d.is_dir()) {
                dlg = dlg.set_directory(dir);
            }
            dlg.save_file()
        } else {
            self.flow.path.clone()
        };
        if let Some(path) = target {
            match self.flow.save(&path) {
                Ok(()) => self.toast(Level::Ok, format!("Saved {}", path.display())),
                Err(e) => self.toast(Level::Error, format!("Save failed: {e}")),
            }
        }
    }

    pub fn open_opf_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().set_title("Load OPF database").add_filter("OPF SQLite", &["sqlite", "db"]).pick_file() {
            self.load_opf(path);
        }
    }

    pub fn load_opf(&mut self, path: PathBuf) {
        self.project.opf_path = Some(path.clone());
        self.submit(EngineJob::Command(EngineCommand::LoadOpfDatabase { db_path: path }), JobPurpose::Opf);
        self.doc_request = Some(Doc::Viewer);
    }

    pub fn save_opf_dialog(&mut self) {
        let path = self.project.opf_path.clone().or_else(|| {
            rfd::FileDialog::new()
                .set_title("Save OPF database")
                .add_filter("OPF SQLite", &["sqlite"])
                .set_file_name("project.opf.sqlite")
                .save_file()
        });
        if let Some(path) = path {
            self.project.opf_path = Some(path.clone());
            self.submit(EngineJob::Command(EngineCommand::SaveOpfDatabase { db_path: path }), JobPurpose::Opf);
        }
    }

    /// Insert a catalog tool after the selected step (or at the end).
    pub fn insert_tool(&mut self, menu_id: &str) {
        let Some(node) = self.new_linked_node(menu_id) else { return };
        self.push_flow_undo();
        let sel = self.flow_sel.clone();
        let path = self.flow.insert_after(sel.as_ref(), node);
        self.flow_sel = Some(path);
    }

    /// A fresh node for a catalog tool with its table references pointed at this session's tables.
    pub fn new_linked_node(&mut self, menu_id: &str) -> Option<FlowNode> {
        let desc = self.catalog.iter().find(|t| t.menu_id == menu_id)?;
        let mut params = desc.default_params.clone();
        let tool = desc.menu_id.clone();
        self.link_tables(&tool, &mut params);
        Some(self.flow.new_node(&tool, params))
    }

    /// Velocity table new steps should use: the velocity-analysis table once saved, else the first saved one.
    pub fn preferred_velocity_table(&self) -> Option<String> {
        let vels = &self.session()?.context.velocities;
        if vels.contains_key(&self.iva.table_name) {
            return Some(self.iva.table_name.clone());
        }
        let mut names: Vec<&String> = vels.keys().collect();
        names.sort();
        names.first().map(|n| n.to_string())
    }

    /// Mute table saved most recently in the gather view, else the first saved one.
    pub fn preferred_mute_table(&self) -> Option<String> {
        let mutes = &self.session()?.context.mutes;
        if let Some(n) = self.last_mute_table.as_ref().filter(|n| mutes.contains_key(*n)) {
            return Some(n.clone());
        }
        let mut names: Vec<&String> = mutes.keys().filter(|n| !self.viewer.is_gate_name(n)).collect();
        names.sort();
        names.first().map(|n| n.to_string())
    }

    /// Fill table references of a new step from what the session already holds.
    pub fn link_tables(&self, tool: &str, params: &mut Value) {
        if VELOCITY_TOOLS.contains(&tool) {
            if let Some(v) = self.preferred_velocity_table() {
                params["velocity_table"] = json!(v);
            }
        } else if tool == "mute" {
            if let Some(m) = self.preferred_mute_table() {
                params["mute_table"] = json!(m);
            }
        } else if tool == "w_srme_v3" {
            // Keep the predicted multiples so Adaptive Subtraction and the gather view can use them.
            if params.get("save_model_buffer").is_none_or(|v| v.as_str().is_none_or(str::is_empty)) {
                params["save_model_buffer"] = json!(SRME_MODEL_BUFFER);
            }
        } else if tool == "w_asub" {
            let model = self
                .flow
                .find_paths(|n| n.tool == "w_srme_v3")
                .last()
                .and_then(|p| self.flow.get(p))
                .and_then(|n| n.params.get("save_model_buffer").and_then(|v| v.as_str()).map(str::to_string));
            params["model_buffer"] = json!(model.unwrap_or_else(|| SRME_MODEL_BUFFER.to_string()));
        }
    }

    /// Edit the step of `tool` that `matches` (selected one first, then the first in the flow),
    /// or add a new one at root index `insert_at` (default: after the selection). Undoable.
    /// Returns the step path and whether a new step was created.
    pub fn upsert_flow_step(
        &mut self,
        tool: &str,
        matches: impl Fn(&FlowNode) -> bool,
        insert_at: Option<usize>,
        edit: impl FnOnce(&mut FlowNode),
    ) -> Option<(StepPath, bool)> {
        let hit = |n: &FlowNode| n.tool == tool && matches(n);
        let selected = self.flow_sel.clone().filter(|p| self.flow.get(p).is_some_and(hit));
        let existing = selected.or_else(|| self.flow.find_paths(hit).into_iter().next());
        let (path, created) = match existing {
            Some(path) => {
                self.push_flow_undo();
                (path, false)
            }
            None => {
                let node = self.new_linked_node(tool)?;
                self.push_flow_undo();
                let path = match insert_at {
                    Some(i) => {
                        let i = i.min(self.flow.nodes.len());
                        self.flow.nodes.insert(i, node);
                        vec![i]
                    }
                    None => {
                        let sel = self.flow_sel.clone();
                        self.flow.insert_after(sel.as_ref(), node)
                    }
                };
                self.report_uids.clear();
                (path, true)
            }
        };
        if let Some(n) = self.flow.get_mut(&path) {
            edit(n);
        }
        self.flow.dirty = true;
        self.flow_sel = Some(path.clone());
        Some((path, created))
    }

    pub fn tool(&self, menu_id: &str) -> Option<&ToolDescriptor> {
        let id = menu_id.to_ascii_lowercase();
        self.catalog.iter().find(|t| t.menu_id == id)
    }
}

/// Steps that read a velocity table; new ones link to the session's velocity table.
pub const VELOCITY_TOOLS: &[&str] = &["nmo", "kirtime2dstk", "kirtime2d", "fdmig", "vel_to_depth"];
/// Buffer SRME stores its predicted multiples in (read by Adaptive Subtraction).
pub const SRME_MODEL_BUFFER: &str = "srme_model";

pub fn compact_json(v: &Value, max: usize) -> String {
    let s = v.to_string();
    if s.chars().count() > max {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    } else {
        s
    }
}
