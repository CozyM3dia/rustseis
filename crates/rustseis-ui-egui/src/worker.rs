//! Background execution.
//!
//! * [`EngineWorker`] owns no state: the UI hands the whole [`EngineSession`] to it for a heavy
//!   command (`RunFlow`, MCP calls, OPF load/save) and gets it back with the result. While the
//!   session is away the UI keeps painting cached textures, so a 66 s full-line flow never
//!   blocks a frame.
//! * [`ComputePool`] runs short pure functions over cloned ensembles (semblance, Radon, F-K,
//!   VSP separation, large raster renders). Each job has a key; only the newest generation per
//!   key is delivered, so dragging a pick never queues stale work.

use rustseis_engine::{EngineCommand, EngineSession};
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub enum EngineJob {
    Command(EngineCommand),
    Mcp { call_id: u64, name: String, args: Value },
    InitProject { root: PathBuf, area: String, line: String },
}

impl EngineJob {
    pub fn label(&self) -> String {
        match self {
            Self::Command(EngineCommand::RunFlow(spec)) => format!("Run flow · {}", spec.name),
            Self::Command(EngineCommand::LoadOpfDatabase { db_path }) => {
                format!("Load OPF · {}", file_name(db_path))
            }
            Self::Command(EngineCommand::SaveOpfDatabase { db_path }) => {
                format!("Save OPF · {}", file_name(db_path))
            }
            Self::Command(_) => "Engine command".into(),
            Self::Mcp { name, .. } => format!("Copilot · {name}"),
            Self::InitProject { area, line, .. } => format!("New project · {area}/{line}"),
        }
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

pub enum EngineOutcome {
    Command(Result<Value, String>),
    Mcp { call_id: u64, result: Result<Value, String> },
    Project(Result<Value, String>),
}

pub struct EngineDone {
    pub session: Box<EngineSession>,
    pub label: String,
    pub outcome: EngineOutcome,
    pub elapsed: Duration,
}

pub struct EngineWorker {
    tx: Sender<(Box<EngineSession>, EngineJob)>,
    rx: Receiver<EngineDone>,
}

impl EngineWorker {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (job_tx, job_rx) = channel::<(Box<EngineSession>, EngineJob)>();
        let (done_tx, done_rx) = channel::<EngineDone>();
        thread::Builder::new()
            .name("rustseis-engine".into())
            .spawn(move || {
                while let Ok((mut session, job)) = job_rx.recv() {
                    let label = job.label();
                    let t0 = Instant::now();
                    let outcome = run_engine_job(&mut session, job);
                    let done = EngineDone { session, label, outcome, elapsed: t0.elapsed() };
                    if done_tx.send(done).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn engine worker");
        Self { tx: job_tx, rx: done_rx }
    }

    pub fn submit(&self, session: Box<EngineSession>, job: EngineJob) {
        // The receiver lives as long as the worker thread; a send failure means the thread
        // panicked, which the UI surfaces through `poll` never returning the session.
        let _ = self.tx.send((session, job));
    }

    pub fn poll(&self) -> Option<EngineDone> {
        self.rx.try_recv().ok()
    }
}

fn run_engine_job(session: &mut EngineSession, job: EngineJob) -> EngineOutcome {
    let guarded = |f: &mut dyn FnMut() -> Result<Value, String>| -> Result<Value, String> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(r) => r,
            Err(p) => Err(panic_message(p)),
        }
    };
    match job {
        EngineJob::Command(cmd) => {
            let mut cmd = Some(cmd);
            EngineOutcome::Command(guarded(&mut || {
                session.dispatch(cmd.take().expect("single dispatch")).map_err(|e| e.to_string())
            }))
        }
        EngineJob::Mcp { call_id, name, args } => {
            let result = guarded(&mut || {
                rustseis_mcp::handle_tool_call(session, &name, &args).map_err(|e| e.to_string())
            });
            EngineOutcome::Mcp { call_id, result }
        }
        EngineJob::InitProject { root, area, line } => {
            let (a, l) = (area.clone(), line.clone());
            EngineOutcome::Project(guarded(&mut || {
                session
                    .init_project(&root, a.clone(), l.clone())
                    .map_err(|e| e.to_string())
                    .and_then(|m| serde_json::to_value(m).map_err(|e| e.to_string()))
            }))
        }
    }
}

fn panic_message(p: Box<dyn Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        format!("engine panic: {s}")
    } else if let Some(s) = p.downcast_ref::<String>() {
        format!("engine panic: {s}")
    } else {
        "engine panic".into()
    }
}

pub type ComputeOutput = Result<Box<dyn Any + Send>, String>;
type ComputeFn = Box<dyn FnOnce() -> ComputeOutput + Send>;

pub struct ComputeDone {
    pub key: &'static str,
    pub generation: u64,
    pub result: ComputeOutput,
    pub elapsed: Duration,
}

pub struct ComputePool {
    tx: Sender<(&'static str, u64, ComputeFn)>,
    rx: Receiver<ComputeDone>,
    latest: HashMap<&'static str, u64>,
    in_flight: HashMap<&'static str, u64>,
    counter: u64,
}

impl ComputePool {
    pub fn spawn(ctx: egui::Context, threads: usize) -> Self {
        let (job_tx, job_rx) = channel::<(&'static str, u64, ComputeFn)>();
        let (done_tx, done_rx) = channel::<ComputeDone>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        for i in 0..threads.max(1) {
            let job_rx = Arc::clone(&job_rx);
            let done_tx = done_tx.clone();
            let ctx = ctx.clone();
            thread::Builder::new()
                .name(format!("rustseis-compute-{i}"))
                .spawn(move || {
                    loop {
                        let next = { job_rx.lock().map(|rx| rx.recv()) };
                        let Ok(Ok((key, generation, f))) = next else { break };
                        let t0 = Instant::now();
                        let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                            Ok(r) => r,
                            Err(p) => Err(panic_message(p)),
                        };
                        if done_tx.send(ComputeDone { key, generation, result, elapsed: t0.elapsed() }).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                })
                .expect("spawn compute worker");
        }
        Self { tx: job_tx, rx: done_rx, latest: HashMap::new(), in_flight: HashMap::new(), counter: 0 }
    }

    /// Submit a job; any earlier pending job for the same key becomes stale.
    pub fn submit<T: Any + Send>(
        &mut self,
        key: &'static str,
        f: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> u64 {
        self.counter += 1;
        let generation = self.counter;
        self.latest.insert(key, generation);
        *self.in_flight.entry(key).or_default() += 1;
        let boxed: ComputeFn = Box::new(move || f().map(|v| Box::new(v) as Box<dyn Any + Send>));
        let _ = self.tx.send((key, generation, boxed));
        generation
    }

    pub fn is_busy(&self, key: &'static str) -> bool {
        self.in_flight.get(key).copied().unwrap_or(0) > 0
    }

    pub fn any_busy(&self) -> bool {
        self.in_flight.values().any(|n| *n > 0)
    }

    /// Returns only results that are still the newest for their key.
    pub fn drain(&mut self) -> Vec<ComputeDone> {
        let mut out = Vec::new();
        while let Ok(done) = self.rx.try_recv() {
            if let Some(n) = self.in_flight.get_mut(done.key) {
                *n = n.saturating_sub(1);
            }
            if self.latest.get(done.key) == Some(&done.generation) {
                out.push(done);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_pool_delivers_only_latest_generation() {
        let ctx = egui::Context::default();
        let mut pool = ComputePool::spawn(ctx, 1);
        let (gate_tx, gate_rx) = channel::<()>();
        pool.submit("k", move || {
            let _ = gate_rx.recv();
            Ok(1u32)
        });
        pool.submit("k", || Ok(2u32));
        gate_tx.send(()).unwrap();
        let mut got = Vec::new();
        let t0 = Instant::now();
        while got.is_empty() && t0.elapsed() < Duration::from_secs(5) {
            got.extend(pool.drain());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got.len(), 1);
        let v = got.pop().unwrap().result.unwrap().downcast::<u32>().unwrap();
        assert_eq!(*v, 2);
    }

    #[test]
    fn engine_worker_round_trips_session() {
        let ctx = egui::Context::default();
        let worker = EngineWorker::spawn(ctx);
        let spec: rustseis_flow::FlowSpec = serde_json::from_value(serde_json::json!({
            "name": "ui_worker_test",
            "steps": [{"tool": "synth", "params": {}}]
        }))
        .unwrap();
        worker.submit(Box::new(EngineSession::new()), EngineJob::Command(EngineCommand::RunFlow(spec)));
        let t0 = Instant::now();
        let done = loop {
            if let Some(d) = worker.poll() {
                break d;
            }
            assert!(t0.elapsed() < Duration::from_secs(60), "worker timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        match done.outcome {
            EngineOutcome::Command(Ok(_)) => {}
            EngineOutcome::Command(Err(e)) => panic!("flow failed: {e}"),
            _ => panic!("unexpected outcome"),
        }
        assert!(!done.session.active_traces.is_empty());
        assert_eq!(done.session.audit_log.len(), 1);
    }
}
