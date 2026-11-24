//! Try the selected flow step on the gather on screen: the step runs alone on a copy of that
//! gather (with the session's tables), and the result lands in two buffers the gather view
//! compares side by side: what the step outputs and, for trace-preserving steps, what it removed.

use crate::data::TraceSource;
use crate::flowdoc::{StepPath, lower};
use crate::state::{AppState, Doc, Level};
use crate::theme::{Palette, font_mono};
use crate::widgets::{self, caps, cmd, cmd_primary};
use egui::{RichText, Ui};
use egui_phosphor::regular as icon;
use rustseis_core::Trace;
use rustseis_flow::{FlowContext, FlowSpec, ToolArchetype};
use std::hash::{Hash, Hasher};

pub const AFTER_BUFFER: &str = "preview_after";
pub const REMOVED_BUFFER: &str = "preview_removed";
const COMPUTE_KEY: &str = "preview.step";

/// Steps too heavy (or meaningless) to run on a single gather while the user waits.
const NOT_PREVIEWABLE: &[&str] = &["fwi2d", "rtm2d", "eikonal_psdm", "afd", "synth", "well_tie", "mistie", "vaprecom71", "buffer_save"];

#[derive(Default)]
pub struct PreviewState {
    /// Re-run whenever the step's settings or the gather change.
    pub live: bool,
    pub last: Option<PreviewSummary>,
    /// Finished while the engine was busy; stored into the session once it is back.
    pending: Option<PreviewResult>,
    requested_key: u64,
}

#[derive(Clone, Debug)]
pub struct PreviewSummary {
    pub uid: u64,
    pub label: String,
    pub elapsed_ms: f64,
    pub traces_in: usize,
    pub traces_out: usize,
    /// Output energy / input energy, when the step keeps the trace count.
    pub kept: Option<f32>,
    pub error: Option<String>,
}

struct PreviewResult {
    summary: PreviewSummary,
    after: Vec<Trace>,
    removed: Option<Vec<Trace>>,
}

fn hash_of(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

fn is_preview_buffer(src: &TraceSource) -> bool {
    matches!(src, TraceSource::Buffer(n) if n == AFTER_BUFFER || n == REMOVED_BUFFER)
}

/// Whether a step can be tried on one gather.
pub fn can_preview(tool: &str, archetype: Option<ToolArchetype>) -> bool {
    !NOT_PREVIEWABLE.contains(&tool)
        && !crate::flowdoc::is_container(tool)
        && !matches!(archetype, None | Some(ToolArchetype::Input | ToolArchetype::Output | ToolArchetype::FlowControl))
}

fn energy(t: &[Trace]) -> f64 {
    t.iter().map(|x| x.samples.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>()).sum()
}

/// Tables a single step may read, plus any named trace buffer sliced to the previewed gather
/// (so trace-aligned steps like `w_asub` match the single-gather trace count).
fn step_context(s: &AppState, params: &serde_json::Value, range: crate::data::EnsRange, src_len: usize) -> FlowContext {
    let Some(sess) = s.session() else { return FlowContext::new() };
    let c = &sess.context;
    let mut ctx = FlowContext {
        velocities: c.velocities.clone(),
        velocity_grids: c.velocity_grids.clone(),
        mutes: c.mutes.clone(),
        first_breaks: c.first_breaks.clone(),
        statics: c.statics.clone(),
        sc_amps: c.sc_amps.clone(),
        radon_panels: c.radon_panels.clone(),
        vsp_profiles: c.vsp_profiles.clone(),
        avo_summaries: c.avo_summaries.clone(),
        well_ties: c.well_ties.clone(),
        fwi_misfits: c.fwi_misfits.clone(),
        mistie_reports: c.mistie_reports.clone(),
        buffers: Default::default(),
        ensemble_key: c.ensemble_key.clone(),
        job_control: None,
    };
    let mut named = Vec::new();
    collect_strings(params, &mut named);
    for n in named {
        if let Some(b) = c.buffers.get(&n) {
            let sliced = if b.len() == src_len && range.end <= b.len() {
                b[range.start..range.end].to_vec()
            } else {
                let g = crate::data::header_getter(&s.focus.key);
                let matched: Vec<Trace> = b.iter().filter(|t| g(t) == range.key_value).cloned().collect();
                if matched.len() == range.len() { matched } else { b.clone() }
            };
            ctx.buffers.insert(n, sliced);
        }
    }
    ctx
}

fn collect_strings(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

/// Run the step at `path` on a copy of the focus gather in the background.
pub fn request(s: &mut AppState, path: &StepPath) {
    let Some(node) = s.flow.get(path).cloned() else { return };
    s.ensure_index();
    let (Some(range), Some(sess)) = (s.focus_range(), s.session()) else {
        s.toast(Level::Warn, "Load data and pick a gather first");
        return;
    };
    let src_traces = s.focus.source.traces(sess);
    let src_len = src_traces.len();
    let gather: Vec<Trace> = src_traces[range.start..range.end].to_vec();
    let mut step = lower(&node);
    step.enabled = true;
    let label = node.label.clone().or_else(|| s.tool(&node.tool).map(|t| t.label.clone())).unwrap_or_else(|| node.tool.clone());
    let mut ctx = step_context(s, &step.params, range, src_len);
    let spec = FlowSpec { name: format!("preview_{}", node.tool), description: String::new(), steps: vec![step] };
    let uid = node.uid;
    s.preview.requested_key = hash_of((uid, node.params.to_string(), &s.focus.source, range.start, range.end, s.rev));
    s.compute.submit(COMPUTE_KEY, move || {
        let t0 = std::time::Instant::now();
        let traces_in = gather.len();
        let res = rustseis_flow::execute_flow(&spec, gather.clone(), &mut ctx);
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok(match res {
            Ok((after, _)) => {
                let same_shape = after.len() == gather.len() && after.iter().zip(&gather).all(|(a, b)| a.samples.len() == b.samples.len());
                let removed = same_shape.then(|| {
                    gather
                        .iter()
                        .zip(&after)
                        .map(|(b, a)| {
                            let mut d = b.clone();
                            d.samples.iter_mut().zip(&a.samples).for_each(|(x, y)| *x -= *y);
                            d
                        })
                        .collect::<Vec<_>>()
                });
                let kept = same_shape.then(|| (energy(&after) / energy(&gather).max(1e-30)) as f32);
                let summary = PreviewSummary { uid, label, elapsed_ms, traces_in, traces_out: after.len(), kept, error: None };
                PreviewResult { summary, after, removed }
            }
            Err(e) => PreviewResult {
                summary: PreviewSummary { uid, label, elapsed_ms, traces_in, traces_out: 0, kept: None, error: Some(e.to_string()) },
                after: Vec::new(),
                removed: None,
            },
        })
    });
}

pub fn on_compute(s: &mut AppState, key: &str, value: Box<dyn std::any::Any + Send>) {
    if key != COMPUTE_KEY {
        return;
    }
    if let Ok(r) = value.downcast::<PreviewResult>() {
        s.preview.pending = Some(*r);
        apply_pending(s);
    }
}

/// Store a finished preview in the session buffers and point the gather view's compare at it.
fn apply_pending(s: &mut AppState) {
    if s.preview.pending.is_none() || s.session().is_none() {
        return;
    }
    let r = s.preview.pending.take().expect("checked");
    if let Some(err) = &r.summary.error {
        s.toast(Level::Error, format!("Preview of {} failed: {err}", r.summary.label));
        s.preview.last = Some(r.summary);
        return;
    }
    let sess = s.session_mut().expect("checked");
    sess.context.buffers.insert(AFTER_BUFFER.into(), r.after);
    match r.removed {
        Some(rm) => {
            sess.context.buffers.insert(REMOVED_BUFFER.into(), rm);
        }
        None => {
            sess.context.buffers.remove(REMOVED_BUFFER);
        }
    }
    if is_preview_buffer(&s.focus.source) {
        // The gather index of a preview buffer is now stale.
        s.focus.source = TraceSource::Active;
        s.rev += 1;
    }
    let showing_removed = s.viewer.compare_source == TraceSource::Buffer(REMOVED_BUFFER.into());
    let has_removed = s.session().is_some_and(|x| x.context.buffers.contains_key(REMOVED_BUFFER));
    s.viewer.compare = true;
    s.viewer.compare_source = TraceSource::Buffer(if showing_removed && has_removed { REMOVED_BUFFER } else { AFTER_BUFFER }.into());
    s.viewer.invalidate_compare();
    s.preview.last = Some(r.summary);
}

/// Inspector section for the selected step.
pub fn panel(s: &mut AppState, ui: &mut Ui, p: &Palette, path: &StepPath) {
    apply_pending(s);
    let Some(node) = s.flow.get(path).cloned() else { return };
    let archetype = s.tool(&node.tool).map(|t| t.archetype);
    if !can_preview(&node.tool, archetype) {
        return;
    }
    let busy = s.compute.is_busy(COMPUTE_KEY);
    let has_data = s.summary.n_traces > 0 && s.session().is_some();
    ui.add_space(4.0);
    caps(ui, p, "TRY IT ON THE GATHER ON SCREEN", None);
    if !has_data {
        widgets::hint(ui, p, icon::INFO, "Load data or run the flow, then preview this step on any gather.");
        return;
    }
    if is_preview_buffer(&s.focus.source) {
        widgets::hint(ui, p, icon::INFO, "The gather view shows a preview; switch it back to a real data source to preview again.");
        return;
    }
    // Live mode: re-run when the settings or the gather change.
    if s.preview.live && !busy {
        if let Some(range) = s.focus_range() {
            let key = hash_of((node.uid, node.params.to_string(), &s.focus.source, range.start, range.end, s.rev));
            if key != s.preview.requested_key {
                request(s, path);
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(!busy, |ui| {
            if cmd_primary(ui, p, icon::PLAY, "Preview on gather").on_hover_text("Run only this step on a copy of the gather on screen; the flow and the loaded data are not changed").clicked() {
                request(s, path);
                s.doc_request = Some(Doc::Viewer);
            }
        });
        if busy {
            ui.add(egui::Spinner::new().size(14.0));
        }
        ui.label(RichText::new("live").size(11.0).color(p.text_muted));
        widgets::toggle(ui, p, &mut s.preview.live).on_hover_text("Re-run the preview whenever a setting or the gather changes");
    });
    if matches!(archetype, Some(ToolArchetype::Complex)) {
        widgets::hint(ui, p, icon::WARNING, "This step normally works across many gathers; on one gather the result is only indicative.");
    }
    let Some(sum) = s.preview.last.clone().filter(|x| x.uid == node.uid) else { return };
    match &sum.error {
        Some(e) => {
            ui.label(RichText::new(format!("{} {e}", icon::WARNING)).size(11.5).color(p.danger));
        }
        None => {
            let shape = if sum.traces_in == sum.traces_out { format!("{} traces", sum.traces_out) } else { format!("{} → {} traces", sum.traces_in, sum.traces_out) };
            let kept = sum.kept.map(|k| format!(" · keeps {:.0}% of the energy", k * 100.0)).unwrap_or_default();
            ui.label(RichText::new(format!("{shape} · {}{kept}", crate::data::fmt_ms(sum.elapsed_ms))).font(font_mono(11.5)).color(p.text));
            let has_removed = s.session().is_some_and(|x| x.context.buffers.contains_key(REMOVED_BUFFER));
            ui.horizontal_wrapped(|ui| {
                for (buf, label, enabled) in [(AFTER_BUFFER, "Show result", true), (REMOVED_BUFFER, "Show what it removed", has_removed)] {
                    let on = s.viewer.compare && s.viewer.compare_source == TraceSource::Buffer(buf.into());
                    ui.add_enabled_ui(enabled, |ui| {
                        if cmd(ui, p, if buf == AFTER_BUFFER { icon::COLUMNS } else { icon::MINUS_CIRCLE }, label, on).clicked() {
                            s.viewer.compare = true;
                            s.viewer.compare_source = TraceSource::Buffer(buf.into());
                            s.viewer.invalidate_compare();
                            s.doc_request = Some(Doc::Viewer);
                        }
                    });
                }
            });
        }
    }
}
