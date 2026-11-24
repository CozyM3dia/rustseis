pub mod catalog;
pub mod python_node;

pub use catalog::{full_tool_catalog, tier1_tool_catalog, tier2_tool_catalog, tier3_tool_catalog};
pub use python_node::{PythonNodeParams, execute_python_node};

use rayon::prelude::*;
use rustseis_core::{
    CrookedLineGeometry2D, Ensemble, FirstBreakTable, JobControl, LandGeometry2D, MarineGeometry2D,
    MuteTable, Result, SeismicError, StaticsTable, SurfaceConsistentAmpTable, Trace, TraceStats,
    VelocityGrid2D, VelocityTable, trace_stats,
};
use rustseis_dsp::{
    AdaptiveSubParams, AgcParams, AirBlastParams, BandpassParams, CurveletDenoiseParams,
    DeconParams, ElevationStaticsParams, FkFilterParams, FxDeconParams, GroundRollParams,
    PhaseRotParams, QCompParams, RadonPanel, RadonParams, ScAmpParams, ScDeconParams, Srme2dParams,
    TarParams, TeqParams, TimeVariantBandpassParams, TraceEditParams, TvsParams, TvwParams,
};
use rustseis_imaging::{
    Afd2dParams, AutostatMethod, AutostatParams, AvoOutputMode, AvoParams, AvoSummary,
    CrookedStackParams, DmoFkParams, FdMig2dParams, FirstBreakPickParams, Fwi2dParams,
    Kirchhoff2dParams, KirchhoffPsdm2dParams, KirchhoffPstm2dParams, MistieParams, MistieReport,
    NmoParams, RefractionStaticsParams, Rtm2dParams, SemblancePanel, StackParams, StoltMigParams,
    Synth2dParams, TrimStatParams, VelocityScanParams, VspCorridorStackParams, VspDeconParams,
    VspGeometryParams, VspSlownessParams, VspSlownessProfile, VspWavefieldSepParams, WellTieParams,
    WellTieResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Instant;

/// Tool execution archetype directly mirroring ProMAX `PromaxToolTypes.xml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolArchetype {
    Input,
    Output,
    SimpleTrace,
    Ensemble,
    Panel,
    Complex,
    FlowControl,
    ExternalPlugin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub menu_id: String,
    pub label: String,
    pub category: String,
    pub archetype: ToolArchetype,
    pub description: String,
    pub default_params: Value,
}

/// Declarative flow step (clean-room replacement for ProMAX binary flows).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowStep {
    pub tool: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default = "default_params_obj")]
    pub params: Value,
}

fn default_enabled() -> bool {
    true
}
fn default_params_obj() -> Value {
    json!({})
}

/// Declarative processing flow specification (JSON or TOML).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<FlowStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReport {
    pub index: usize,
    pub tool: String,
    pub enabled: bool,
    pub traces_in: usize,
    pub traces_out: usize,
    pub elapsed_ms: u128,
    pub sample_stats: Option<TraceStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowReport {
    pub flow_name: String,
    pub total_elapsed_ms: u128,
    pub final_trace_count: usize,
    pub steps: Vec<StepReport>,
    /// Semblance panels produced by `vaprecom71` during the flow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semblance_panels: Vec<SemblancePanel>,
    /// Radon panels produced by `radon_trans` / `radon_filter`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub radon_panels: Vec<RadonPanel>,
    /// VSP / DAS slowness profiles produced by `vsp_slowness`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vsp_profiles: Vec<VspSlownessProfile>,
}

/// Shared runtime context (named OPF tables & intermediate trace buffers replacing ProMAX OPFs).
#[derive(Debug, Clone, Default)]
pub struct FlowContext {
    pub velocities: HashMap<String, VelocityTable>,
    pub velocity_grids: HashMap<String, VelocityGrid2D>,
    pub mutes: HashMap<String, MuteTable>,
    pub first_breaks: HashMap<String, FirstBreakTable>,
    pub statics: HashMap<String, StaticsTable>,
    pub sc_amps: HashMap<String, SurfaceConsistentAmpTable>,
    pub radon_panels: HashMap<String, RadonPanel>,
    pub vsp_profiles: HashMap<String, VspSlownessProfile>,
    pub avo_summaries: HashMap<String, AvoSummary>,
    pub well_ties: HashMap<String, WellTieResult>,
    pub fwi_misfits: HashMap<String, Vec<f64>>,
    pub mistie_reports: HashMap<String, MistieReport>,
    pub buffers: HashMap<String, Vec<Trace>>,
    pub ensemble_key: String,
    pub job_control: Option<JobControl>,
}

impl FlowContext {
    pub fn new() -> Self {
        Self {
            velocities: HashMap::new(),
            velocity_grids: HashMap::new(),
            mutes: HashMap::new(),
            first_breaks: HashMap::new(),
            statics: HashMap::new(),
            sc_amps: HashMap::new(),
            radon_panels: HashMap::new(),
            vsp_profiles: HashMap::new(),
            avo_summaries: HashMap::new(),
            well_ties: HashMap::new(),
            fwi_misfits: HashMap::new(),
            mistie_reports: HashMap::new(),
            buffers: HashMap::new(),
            ensemble_key: "FFID".to_string(),
            job_control: None,
        }
    }
}

fn parse_param<T: for<'de> Deserialize<'de>>(val: &Value, tool: &str) -> Result<T> {
    serde_json::from_value(val.clone()).map_err(|e| SeismicError::InvalidParameter {
        param: tool.into(),
        message: e.to_string(),
    })
}

fn parse_nested_or_flat<T: for<'de> Deserialize<'de> + Default>(val: &Value, tool: &str) -> Result<T> {
    if let Some(inner) = val.get("params") {
        parse_param(inner, tool)
    } else {
        Ok(parse_param(val, tool).unwrap_or_default())
    }
}

fn resolve_velocity(params: &Value, ctx: &FlowContext) -> VelocityTable {
    if let Some(v) = params.get("constant_vel_mps").and_then(|x| x.as_f64()) {
        return VelocityTable::constant(v as f32);
    }
    if let Some(inline) = params.get("velocity_picks") {
        if let Ok(vt) = serde_json::from_value::<VelocityTable>(inline.clone()) {
            return vt;
        }
    }
    if let Some(name) = params.get("velocity_table").and_then(|x| x.as_str()) {
        if let Some(vt) = ctx.velocities.get(name) {
            return vt.clone();
        }
    }
    ctx.velocities
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| VelocityTable::constant(1800.0))
}

/// Execute a declarative ProMAX-compatible Tier 1 + Tier 2 flow end-to-end.
pub fn execute_flow(
    spec: &FlowSpec,
    initial_traces: Vec<Trace>,
    ctx: &mut FlowContext,
) -> Result<(Vec<Trace>, FlowReport)> {
    let t_all = Instant::now();
    let mut traces = initial_traces;
    let mut step_reports = Vec::with_capacity(spec.steps.len());
    let mut semblance_panels = Vec::new();
    let mut radon_panels = Vec::new();
    let mut vsp_profiles = Vec::new();

    let total_steps = spec.steps.len().max(1);
    for (idx, step) in spec.steps.iter().enumerate() {
        if let Some(jc) = &ctx.job_control {
            jc.check_cancelled()?;
            jc.set_progress(
                idx as f32 / total_steps as f32,
                format!("Step {}/{} · {}", idx + 1, total_steps, step.tool),
            );
        }
        let t_step = Instant::now();
        let n_in = traces.len();
        if !step.enabled {
            step_reports.push(StepReport {
                index: idx,
                tool: step.tool.clone(),
                enabled: false,
                traces_in: n_in,
                traces_out: n_in,
                elapsed_ms: 0,
                sample_stats: None,
            });
            continue;
        }

        let tool_id = step.tool.to_ascii_lowercase();
        match tool_id.as_str() {
            // ==================== TIER 1: I/O ====================
            "segd_read" => {
                let path = step.params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    SeismicError::InvalidParameter {
                        param: "path".into(),
                        message: "segd_read requires 'path'".into(),
                    }
                })?;
                let max_shots = step.params.get("max_shots").and_then(|v| v.as_u64()).map(|n| n as usize);
                let live_only = step.params.get("live_only").and_then(|v| v.as_bool()).unwrap_or(true);
                let p = std::path::Path::new(path);
                traces = if p.is_dir() {
                    rustseis_io::read_segd_dir(p, max_shots, live_only)?.1
                } else {
                    rustseis_io::read_segd_file(p, live_only)?.1
                };
                ctx.ensemble_key = "FFID".into();
            }
            "segy_read" => {
                let path = step.params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    SeismicError::InvalidParameter {
                        param: "path".into(),
                        message: "segy_read requires 'path'".into(),
                    }
                })?;
                let max_traces = step.params.get("max_traces").and_then(|v| v.as_u64()).map(|n| n as usize);
                traces = rustseis_io::read_segy_file(path, max_traces)?.1;
            }
            "segy_write" => {
                let path = step.params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    SeismicError::InvalidParameter {
                        param: "path".into(),
                        message: "segy_write requires 'path'".into(),
                    }
                })?;
                let comment = step
                    .params
                    .get("comment")
                    .and_then(|v| v.as_str())
                    .unwrap_or("RustSeis export");
                rustseis_io::write_segy_file(path, &traces, comment)?;
            }
            "diskread2" => {
                let path = step.params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    SeismicError::InvalidParameter {
                        param: "path".into(),
                        message: "diskread2 requires 'path'".into(),
                    }
                })?;
                let ds = rustseis_io::MmapDataset::open(path)?;
                ctx.ensemble_key = ds.meta.sort_order.clone();
                traces = ds.read_all();
            }
            "diskwrite" => {
                let path = step.params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    SeismicError::InvalidParameter {
                        param: "path".into(),
                        message: "diskwrite requires 'path'".into(),
                    }
                })?;
                let name = step.params.get("name").and_then(|v| v.as_str()).unwrap_or("dataset");
                let sort = step
                    .params
                    .get("sort_order")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&ctx.ensemble_key);
                rustseis_io::write_dataset(path, name, sort, &traces)?;
            }

            // ==================== GEOMETRY & HEADERS ====================
            "geomspread_marine" | "geom_load" => {
                let geom: MarineGeometry2D = parse_param(&step.params, "geomspread_marine")?;
                rustseis_imaging::assign_marine_geometry_2d(&mut traces, &geom);
            }
            "geomspread_land" => {
                let geom: LandGeometry2D = parse_param(&step.params, "geomspread_land")?;
                rustseis_imaging::assign_land_geometry_2d(&mut traces, &geom);
            }
            "geomspread_crooked" => {
                let geom: CrookedLineGeometry2D = parse_param(&step.params, "geomspread_crooked")?;
                rustseis_imaging::assign_crooked_line_geometry_2d(&mut traces, &geom);
            }
            "inline_sort" => {
                let primary = step.params.get("primary").and_then(|v| v.as_str()).unwrap_or("CDP");
                let secondary = step.params.get("secondary").and_then(|v| v.as_str());
                let p_desc = step.params.get("primary_desc").and_then(|v| v.as_bool()).unwrap_or(false);
                let s_desc = step.params.get("secondary_desc").and_then(|v| v.as_bool()).unwrap_or(false);
                traces = rustseis_imaging::inline_sort(traces, primary, secondary, p_desc, s_desc);
                ctx.ensemble_key = primary.to_string();
            }
            "ens_define" => {
                let key = step.params.get("key").and_then(|v| v.as_str()).unwrap_or("CDP");
                let ens = Ensemble::group_by_header(traces, key);
                traces = Ensemble::flatten(ens);
                ctx.ensemble_key = key.to_string();
            }
            "thdrmath" => {
                let target = step.params.get("target").and_then(|v| v.as_str()).ok_or_else(|| {
                    SeismicError::InvalidParameter {
                        param: "target".into(),
                        message: "thdrmath requires 'target'".into(),
                    }
                })?;
                let expr = step.params.get("expr").and_then(|v| v.as_str()).ok_or_else(|| {
                    SeismicError::InvalidParameter {
                        param: "expr".into(),
                        message: "thdrmath requires 'expr'".into(),
                    }
                })?;
                rustseis_dsp::apply_header_math(&mut traces, target, expr)?;
            }
            "trace_len" => {
                let len_ms = step.params.get("length_ms").and_then(|v| v.as_f64()).unwrap_or(4000.0) as f32;
                traces.par_iter_mut().for_each(|tr| {
                    rustseis_dsp::set_trace_length(tr, len_ms);
                });
            }
            "resamp" => {
                let dt_us = step.params.get("new_dt_us").and_then(|v| v.as_u64()).unwrap_or(4000) as u32;
                traces
                    .par_iter_mut()
                    .try_for_each(|tr| rustseis_dsp::resample_trace(tr, dt_us))?;
            }
            "debias" => rustseis_dsp::debias_traces(&mut traces),
            "trc_edit" => {
                let p: TraceEditParams = parse_param(&step.params, "trc_edit")?;
                rustseis_dsp::apply_trace_edit(&mut traces, &p);
            }
            "mute" => {
                let table_name = step
                    .params
                    .get("mute_table")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let table: MuteTable = if let Some(name) = table_name {
                    ctx.mutes.get(name).cloned().ok_or_else(|| {
                        let avail: Vec<&str> = ctx.mutes.keys().map(String::as_str).collect();
                        SeismicError::NotFound(format!(
                            "Mute table '{name}' not found (available: {})",
                            if avail.is_empty() { "none".into() } else { avail.join(", ") }
                        ))
                    })?
                } else {
                    parse_param(&step.params, "mute")?
                };
                rustseis_dsp::apply_mute(&mut traces, &table);
            }

            // ==================== AMPLITUDE & GAIN ====================
            "tar" => {
                let p: TarParams = parse_param(&step.params, "tar")?;
                let vt = step
                    .params
                    .get("velocity_table")
                    .and_then(|v| v.as_str())
                    .and_then(|n| ctx.velocities.get(n));
                rustseis_dsp::apply_tar(&mut traces, &p, vt);
            }
            "agc" => {
                let p: AgcParams = parse_param(&step.params, "agc")?;
                rustseis_dsp::apply_agc(&mut traces, &p);
            }
            "teq" => {
                let p: TeqParams = parse_param(&step.params, "teq")?;
                rustseis_dsp::apply_teq(&mut traces, &p);
            }
            "tvs" => {
                let p: TvsParams = parse_param(&step.params, "tvs")?;
                rustseis_dsp::apply_tvs(&mut traces, &p);
            }
            "scamp2" => {
                let p: ScAmpParams = parse_nested_or_flat(&step.params, "scamp2")?;
                let name = step
                    .params
                    .get("save_table")
                    .and_then(|v| v.as_str())
                    .unwrap_or("scamp");
                let table = rustseis_dsp::apply_sc_amp(&mut traces, &p, name)?;
                ctx.sc_amps.insert(name.to_string(), table);
            }

            // ==================== FILTERING, DECON & DENOISING ====================
            "filter" => {
                let p: BandpassParams = parse_param(&step.params, "filter")?;
                rustseis_dsp::apply_bandpass(&mut traces, &p)?;
            }
            "tv_filter" => {
                let p: TimeVariantBandpassParams = parse_param(&step.params, "tv_filter")?;
                rustseis_dsp::apply_time_variant_bandpass(&mut traces, &p)?;
            }
            "tvw" => {
                let p: TvwParams = parse_param(&step.params, "tvw")?;
                rustseis_dsp::apply_tvw(&mut traces, &p)?;
            }
            "decon" => {
                let p: DeconParams = parse_param(&step.params, "decon")?;
                let gate_name = step
                    .params
                    .get("design_gate_table")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let gate = if let Some(name) = gate_name {
                    Some(ctx.mutes.get(name).ok_or_else(|| {
                        SeismicError::NotFound(format!("Decon gate table '{name}' not found"))
                    })?)
                } else {
                    None
                };
                rustseis_dsp::apply_decon_with_gate(&mut traces, &p, gate)?;
            }
            "scdecon" => {
                let p: ScDeconParams = parse_nested_or_flat(&step.params, "scdecon")?;
                rustseis_dsp::apply_sc_decon(&mut traces, &p)?;
            }
            "fk_filt" => {
                let p: FkFilterParams = parse_param(&step.params, "fk_filt")?;
                let key = step
                    .params
                    .get("ensemble_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&ctx.ensemble_key);
                let mut ensembles = Ensemble::group_by_header(traces, key);
                ensembles
                    .par_iter_mut()
                    .try_for_each(|ens| rustseis_dsp::apply_fk_ensemble(ens, &p))?;
                traces = Ensemble::flatten(ensembles);
            }
            "fxdecon" => {
                let p: FxDeconParams = parse_nested_or_flat(&step.params, "fxdecon")?;
                if let Some(key) = step.params.get("ensemble_key").and_then(|v| v.as_str()) {
                    let mut ensembles = Ensemble::group_by_header(traces, key);
                    ensembles
                        .par_iter_mut()
                        .try_for_each(|ens| rustseis_dsp::apply_fx_decon(&mut ens.traces, &p))?;
                    traces = Ensemble::flatten(ensembles);
                } else {
                    rustseis_dsp::apply_fx_decon(&mut traces, &p)?;
                }
            }
            "radon_trans" | "taup" => {
                let p: RadonParams = parse_nested_or_flat(&step.params, "radon_trans")?;
                let save_name = step
                    .params
                    .get("save_panel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("radon_qc");
                let key = step
                    .params
                    .get("ensemble_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&ctx.ensemble_key);
                let ensembles = Ensemble::group_by_header(traces, key);
                let mid = ensembles.len() / 2;
                let qc_idx = ensembles
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.traces.len() >= 3)
                    .max_by(|(i, a), (j, b)| {
                        a.traces
                            .len()
                            .cmp(&b.traces.len())
                            .then_with(|| j.abs_diff(mid).cmp(&i.abs_diff(mid)))
                    })
                    .map(|(i, _)| i)
                    .or_else(|| (!ensembles.is_empty()).then_some(0));
                if let Some(ei) = qc_idx {
                    let panel = rustseis_dsp::forward_radon_gather(&ensembles[ei].traces, &p)?;
                    ctx.radon_panels.insert(save_name.to_string(), panel.clone());
                    radon_panels.push(panel);
                }
                traces = Ensemble::flatten(ensembles);
            }
            "radon_filter" => {
                let p: RadonParams = parse_nested_or_flat(&step.params, "radon_filter")?;
                let key = step
                    .params
                    .get("ensemble_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&ctx.ensemble_key);
                let save_name = step
                    .params
                    .get("save_panel")
                    .and_then(|v| v.as_str());
                let mut ensembles = Ensemble::group_by_header(traces, key);
                // Pick the highest-fold ensemble nearest the middle of the line for QC panel output
                // (avoids marine/land fold taper-in ensembles where ei == 0 has fold < 3).
                let mid = ensembles.len() / 2;
                let qc_idx = ensembles
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.traces.len() >= 3)
                    .max_by(|(i, a), (j, b)| {
                        a.traces
                            .len()
                            .cmp(&b.traces.len())
                            .then_with(|| j.abs_diff(mid).cmp(&i.abs_diff(mid)))
                    })
                    .map(|(i, _)| i);
                let panels: Vec<Option<RadonPanel>> = ensembles
                    .par_iter_mut()
                    .enumerate()
                    .map(|(ei, ens)| {
                        if ens.traces.len() < 3 {
                            return Ok(None);
                        }
                        let panel = rustseis_dsp::apply_radon_filter_gather(&mut ens.traces, &p)?;
                        Ok(if qc_idx == Some(ei) { Some(panel) } else { None })
                    })
                    .collect::<Result<Vec<_>>>()?;
                if let Some(first_panel) = panels.into_iter().flatten().next() {
                    if let Some(sn) = save_name {
                        ctx.radon_panels.insert(sn.to_string(), first_panel.clone());
                    }
                    radon_panels.push(first_panel);
                }
                traces = Ensemble::flatten(ensembles);
            }
            "lfaf" => {
                let p: GroundRollParams = parse_nested_or_flat(&step.params, "lfaf")?;
                let key = step
                    .params
                    .get("ensemble_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&ctx.ensemble_key);
                let mut ensembles = Ensemble::group_by_header(traces, key);
                ensembles
                    .par_iter_mut()
                    .try_for_each(|ens| rustseis_dsp::apply_ground_roll_attenuation(&mut ens.traces, &p))?;
                traces = Ensemble::flatten(ensembles);
            }
            "airblast" => {
                let p: AirBlastParams = parse_nested_or_flat(&step.params, "airblast")?;
                rustseis_dsp::apply_airblast_attenuation(&mut traces, &p);
            }
            "curvelet_denoise" => {
                let p: CurveletDenoiseParams = parse_nested_or_flat(&step.params, "curvelet_denoise")?;
                if let Some(key) = step.params.get("ensemble_key").and_then(|v| v.as_str()) {
                    let mut ensembles = Ensemble::group_by_header(traces, key);
                    ensembles
                        .par_iter_mut()
                        .try_for_each(|ens| rustseis_dsp::apply_curvelet_denoise(&mut ens.traces, &p))?;
                    traces = Ensemble::flatten(ensembles);
                } else {
                    rustseis_dsp::apply_curvelet_denoise(&mut traces, &p)?;
                }
            }
            "w_srme_v3" => {
                let p: Srme2dParams = parse_nested_or_flat(&step.params, "w_srme_v3")?;
                let model = rustseis_dsp::apply_srme_2d(&mut traces, &p)?;
                if let Some(buf_name) = step.params.get("save_model_buffer").and_then(|v| v.as_str()) {
                    ctx.buffers.insert(buf_name.to_string(), model);
                }
            }
            "w_asub" => {
                let p: AdaptiveSubParams = parse_nested_or_flat(&step.params, "w_asub")?;
                let buf_name = step
                    .params
                    .get("model_buffer")
                    .and_then(|v| v.as_str())
                    .unwrap_or("srme_model");
                let model = ctx.buffers.get(buf_name).cloned().ok_or_else(|| {
                    SeismicError::NotFound(format!("Model buffer '{buf_name}' not found for w_asub"))
                })?;
                rustseis_dsp::apply_adaptive_subtraction(&mut traces, &model, &p)?;
            }

            // ==================== STATICS ====================
            "elev_stat" => {
                let p: ElevationStaticsParams = parse_param(&step.params, "elev_stat")?;
                rustseis_dsp::apply_elevation_statics(&mut traces, &p);
            }
            "fbpick" => {
                let p: FirstBreakPickParams = parse_nested_or_flat(&step.params, "fbpick")?;
                let name = step
                    .params
                    .get("save_table")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fb_picks");
                let table = rustseis_imaging::pick_first_breaks(&mut traces, &p, name);
                ctx.first_breaks.insert(name.to_string(), table);
            }
            "refstatic" => {
                let p: RefractionStaticsParams = parse_nested_or_flat(&step.params, "refstatic")?;
                let name = step
                    .params
                    .get("save_table")
                    .and_then(|v| v.as_str())
                    .unwrap_or("refstatic");
                let table = rustseis_imaging::solve_and_apply_refraction_statics(&mut traces, &p, name)?;
                ctx.statics.insert(name.to_string(), table);
            }
            "autostat" | "pwr_stat_new" => {
                let mut p: AutostatParams = parse_nested_or_flat(&step.params, &tool_id)?;
                if tool_id == "pwr_stat_new" {
                    p.method = AutostatMethod::MaxPower;
                }
                let name = step
                    .params
                    .get("save_table")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&tool_id);
                let table = rustseis_imaging::solve_and_apply_autostat(&mut traces, &p, name)?;
                ctx.statics.insert(name.to_string(), table);
            }
            "trimstat" => {
                let p: TrimStatParams = parse_nested_or_flat(&step.params, "trimstat")?;
                rustseis_imaging::apply_trim_statics(&mut traces, &p)?;
            }

            // ==================== VELOCITY, NMO, STACK & MIGRATION ====================
            "vaprecom71" => {
                let scan: VelocityScanParams = if let Some(sv) = step.params.get("scan") {
                    parse_param(sv, "vaprecom71.scan")?
                } else {
                    VelocityScanParams::default()
                };
                let target_cdps: Vec<i32> = step
                    .params
                    .get("cdps")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let auto_pick = step.params.get("auto_pick").and_then(|v| v.as_bool()).unwrap_or(true);
                let save_name = step
                    .params
                    .get("save_table")
                    .and_then(|v| v.as_str())
                    .unwrap_or("auto_vel");

                let mut cdp_ranges: Vec<(i32, usize, usize)> = Vec::new();
                let mut i = 0usize;
                while i < traces.len() {
                    let cdp = traces[i].headers.cdp;
                    let start = i;
                    while i < traces.len() && traces[i].headers.cdp == cdp {
                        i += 1;
                    }
                    cdp_ranges.push((cdp, start, i));
                }

                let cdp_increment = step
                    .params
                    .get("cdp_increment")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let super_cdps = step
                    .params
                    .get("supergather_cdps")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1)
                    .max(1) as usize;
                let super_half = super_cdps / 2;

                let mut picked_funcs = Vec::new();
                let n_cdps = cdp_ranges.len();
                let stride = if cdp_increment > 0 {
                    cdp_increment
                } else {
                    (n_cdps / 8).max(1)
                };
                for (ei, &(cdp, start, end)) in cdp_ranges.iter().enumerate() {
                    let lo_i = ei.saturating_sub(super_half);
                    let hi_i = (ei + super_half).min(n_cdps.saturating_sub(1));
                    let sg_start = cdp_ranges[lo_i].1;
                    let sg_end = cdp_ranges[hi_i].2;
                    let fold = sg_end - sg_start;
                    let selected = if target_cdps.is_empty() {
                        ei % stride == 0 && fold >= 8
                    } else {
                        target_cdps.contains(&cdp)
                    };
                    if selected {
                        let _ = (start, end);
                        let ens = Ensemble::new("CDP", cdp, traces[sg_start..sg_end].to_vec());
                        let panel = rustseis_imaging::compute_semblance_panel(&ens, &scan)?;
                        if auto_pick {
                            picked_funcs.push(panel.auto_pick(1200.0, 0.05, 250.0));
                        }
                        semblance_panels.push(panel);
                    }
                }
                if !picked_funcs.is_empty() {
                    ctx.velocities
                        .insert(save_name.to_string(), VelocityTable::new(save_name, picked_funcs));
                }
            }
            "nmo" => {
                let vt = resolve_velocity(&step.params, ctx);
                let p: NmoParams = parse_nested_or_flat(&step.params, "nmo")?;
                rustseis_imaging::apply_nmo(&mut traces, &vt, &p);
            }
            "stack" => {
                let p: StackParams = parse_param(&step.params, "stack").unwrap_or_default();
                let key = step
                    .params
                    .get("ensemble_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("CDP");
                let ensembles = Ensemble::group_by_header(traces, key);
                traces = rustseis_imaging::stack_ensembles(&ensembles, &p);
                ctx.ensemble_key = "LINE".into();
            }
            "dmofkpanel" => {
                let p: DmoFkParams = parse_nested_or_flat(&step.params, "dmofkpanel")?;
                traces = rustseis_imaging::dmo_fk_2d(&traces, &p)?;
            }
            "fkmig2d" => {
                let method = step.params.get("method").and_then(|v| v.as_str()).unwrap_or("stolt");
                let dx_m = step.params.get("dx_m").and_then(|v| v.as_f64()).unwrap_or(6.25) as f32;
                if method.eq_ignore_ascii_case("phase_shift") {
                    let vt = resolve_velocity(&step.params, ctx);
                    traces = rustseis_imaging::phase_shift_migrate_2d(&traces, &vt, dx_m)?;
                } else {
                    let vel = step.params.get("velocity_mps").and_then(|v| v.as_f64()).unwrap_or(1800.0) as f32;
                    let pad = step.params.get("pad_traces").and_then(|v| v.as_u64()).unwrap_or(32) as usize;
                    traces = rustseis_imaging::stolt_migrate_2d(
                        &traces,
                        &StoltMigParams {
                            velocity_mps: vel,
                            dx_m,
                            pad_traces: pad,
                        },
                    )?;
                }
            }
            "fdmig" => {
                let vt = resolve_velocity(&step.params, ctx);
                let p: FdMig2dParams = parse_nested_or_flat(&step.params, "fdmig")?;
                traces = rustseis_imaging::fd_migrate_2d(&traces, &vt, &p)?;
            }
            "kirtime2dstk" => {
                let vt = resolve_velocity(&step.params, ctx);
                let p: Kirchhoff2dParams = parse_nested_or_flat(&step.params, "kirtime2dstk")?;
                traces = rustseis_imaging::kirchhoff_poststack_2d(&traces, &vt, &p)?;
            }
            "kirtime2d" => {
                let vt = resolve_velocity(&step.params, ctx);
                let p: KirchhoffPstm2dParams = parse_nested_or_flat(&step.params, "kirtime2d")?;
                traces = rustseis_imaging::kirchhoff_pstm_2d(&traces, &vt, &p)?;
            }

            // ==================== MODELING ====================
            "synth" => {
                let p: Synth2dParams = parse_nested_or_flat(&step.params, "synth")?;
                traces = rustseis_imaging::generate_synthetic_2d(&p)?;
                ctx.ensemble_key = "FFID".into();
            }
            "afd" => {
                let p: Afd2dParams = parse_nested_or_flat(&step.params, "afd")?;
                traces = rustseis_imaging::run_afd_2d(&p)?;
                ctx.ensemble_key = "FFID".into();
            }

            // ==================== VSP & BOREHOLE DAS ====================
            "vsp_geom" => {
                let p: VspGeometryParams = parse_nested_or_flat(&step.params, "vsp_geom")?;
                rustseis_imaging::assign_vsp_geometry(&mut traces, &p);
            }
            "vsp_slowness" => {
                let p: VspSlownessParams = parse_nested_or_flat(&step.params, "vsp_slowness")?;
                let save = step
                    .params
                    .get("save_profile")
                    .and_then(|v| v.as_str())
                    .unwrap_or("vsp_vel");
                let prof = rustseis_imaging::estimate_vsp_slowness(&mut traces, &p)?;
                ctx.vsp_profiles.insert(save.to_string(), prof.clone());
                vsp_profiles.push(prof);
            }
            "vsp_wavefield_sep" => {
                let p: VspWavefieldSepParams = parse_nested_or_flat(&step.params, "vsp_wavefield_sep")?;
                traces = rustseis_imaging::apply_vsp_wavefield_sep(&traces, &p)?;
            }
            "vsp_decon" => {
                let p: VspDeconParams = parse_nested_or_flat(&step.params, "vsp_decon")?;
                traces = rustseis_imaging::apply_vsp_decon(&traces, &p)?;
            }
            "vsp_corridor_stack" => {
                let p: VspCorridorStackParams = parse_nested_or_flat(&step.params, "vsp_corridor_stack")?;
                traces = rustseis_imaging::compute_vsp_corridor_stack(&traces, &p)?;
            }

            // ==================== FLOW CONTROL & EXTENSIBILITY ====================
            "flow_filter" => {
                let hdr = step.params.get("header").and_then(|v| v.as_str()).unwrap_or("CDP");
                let min_v = step.params.get("min").and_then(|v| v.as_f64()).unwrap_or(f64::NEG_INFINITY);
                let max_v = step.params.get("max").and_then(|v| v.as_f64()).unwrap_or(f64::INFINITY);
                traces.retain(|tr| {
                    let v = tr.headers.get_f64(hdr).unwrap_or(0.0);
                    v >= min_v && v <= max_v
                });
            }
            "flow_if" => {
                let hdr = step.params.get("header").and_then(|v| v.as_str()).unwrap_or("CDP");
                let min_v = step.params.get("min").and_then(|v| v.as_f64()).unwrap_or(f64::NEG_INFINITY);
                let max_v = step.params.get("max").and_then(|v| v.as_f64()).unwrap_or(f64::INFINITY);
                let then_steps: Vec<FlowStep> = step
                    .params
                    .get("then_steps")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let else_steps: Vec<FlowStep> = step
                    .params
                    .get("else_steps")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();

                let mut matched = Vec::new();
                let mut unmatched = Vec::new();
                let mut order = Vec::with_capacity(traces.len());
                for tr in traces {
                    let v = tr.headers.get_f64(hdr).unwrap_or(0.0);
                    if v >= min_v && v <= max_v {
                        order.push(true);
                        matched.push(tr);
                    } else {
                        order.push(false);
                        unmatched.push(tr);
                    }
                }
                if !then_steps.is_empty() && !matched.is_empty() {
                    let sub = FlowSpec {
                        name: format!("{}.if_then", spec.name),
                        description: String::new(),
                        steps: then_steps,
                    };
                    matched = execute_flow(&sub, matched, ctx)?.0;
                }
                if !else_steps.is_empty() && !unmatched.is_empty() {
                    let sub = FlowSpec {
                        name: format!("{}.if_else", spec.name),
                        description: String::new(),
                        steps: else_steps,
                    };
                    unmatched = execute_flow(&sub, unmatched, ctx)?.0;
                }
                // Recombine preserving original trace order if counts match, else concatenate
                if matched.len() + unmatched.len() == order.len() {
                    let mut m_it = matched.into_iter();
                    let mut u_it = unmatched.into_iter();
                    traces = Vec::with_capacity(order.len());
                    for is_m in order {
                        if is_m {
                            if let Some(t) = m_it.next() { traces.push(t); }
                        } else if let Some(t) = u_it.next() {
                            traces.push(t);
                        }
                    }
                } else {
                    matched.extend(unmatched);
                    traces = matched;
                }
            }
            "flow_split" => {
                let branches: Vec<Vec<FlowStep>> = step
                    .params
                    .get("branches")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let out_branch = step
                    .params
                    .get("output_branch")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let mut results: Vec<Vec<Trace>> = Vec::with_capacity(branches.len());
                for (bi, b_steps) in branches.into_iter().enumerate() {
                    let sub = FlowSpec {
                        name: format!("{}.branch_{bi}", spec.name),
                        description: String::new(),
                        steps: b_steps,
                    };
                    let (b_out, b_rep) = execute_flow(&sub, traces.clone(), ctx)?;
                    semblance_panels.extend(b_rep.semblance_panels);
                    radon_panels.extend(b_rep.radon_panels);
                    vsp_profiles.extend(b_rep.vsp_profiles);
                    results.push(b_out);
                }
                if let Some(chosen) = results.into_iter().nth(out_branch) {
                    traces = chosen;
                }
            }
            "buffer_save" => {
                let name = step
                    .params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("snapshot");
                ctx.buffers.insert(name.to_string(), traces.clone());
            }
            "python_node" => {
                let p: PythonNodeParams = parse_nested_or_flat(&step.params, "python_node")?;
                traces = execute_python_node(traces, &p)?;
            }

            // =========================================================================
            // TIER 3 — DEPTH VELOCITY MODEL, AVO, WELL TIE, CROOKED STACK, PSDM & RTM
            // =========================================================================
            "vel_to_depth" => {
                let vt = resolve_velocity(&step.params, ctx);
                let save_grid = step
                    .params
                    .get("save_grid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("depth_vel");
                let dx_m = step.params.get("dx_m").and_then(|v| v.as_f64()).unwrap_or(25.0).max(1.0) as f32;
                let dz_m = step.params.get("dz_m").and_then(|v| v.as_f64()).unwrap_or(10.0).max(1.0) as f32;
                let z0_m = step.params.get("z0_m").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let z_max_m = step.params.get("z_max_m").and_then(|v| v.as_f64()).unwrap_or(3000.0).max(100.0) as f32;
                let smooth_x = step.params.get("smooth_x").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
                let smooth_z = step.params.get("smooth_z").and_then(|v| v.as_u64()).unwrap_or(3) as usize;

                // Infer lateral extent and (cdp, x_m) mapping from active traces if present
                let mut x_min = f32::INFINITY;
                let mut x_max = f32::NEG_INFINITY;
                let mut cdp_x_map: std::collections::BTreeMap<i32, (f64, usize)> = std::collections::BTreeMap::new();
                for tr in &traces {
                    for &x in &[tr.headers.sou_x as f32, tr.headers.rec_x as f32, tr.headers.cdp_x as f32] {
                        if x.is_finite() && x != 0.0 {
                            x_min = x_min.min(x);
                            x_max = x_max.max(x);
                        }
                    }
                    let e = cdp_x_map.entry(tr.headers.cdp).or_insert((0.0, 0));
                    e.0 += tr.headers.cdp_x;
                    e.1 += 1;
                }
                let x0_m = step
                    .params
                    .get("x0_m")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or_else(|| if x_min.is_finite() { x_min } else { 0.0 });
                let x_end = step
                    .params
                    .get("x_max_m")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or_else(|| if x_max.is_finite() && x_max > x0_m { x_max } else { x0_m + 2000.0 });
                let nx = (((x_end - x0_m) / dx_m).ceil() as usize + 1).clamp(16, 4096);
                let nz = (((z_max_m - z0_m) / dz_m).ceil() as usize + 1).clamp(16, 2048);
                let cdp_pairs: Vec<(i32, f32)> = cdp_x_map
                    .into_iter()
                    .map(|(c, (sx, n))| (c, (sx / n as f64) as f32))
                    .collect();
                let grid = VelocityGrid2D::from_velocity_table(
                    &vt,
                    save_grid,
                    x0_m,
                    dx_m,
                    nx,
                    z0_m,
                    dz_m,
                    nz,
                    &cdp_pairs,
                    dx_m * 0.5,
                    1200.0,
                    6500.0,
                    smooth_x,
                    smooth_z,
                );
                ctx.velocity_grids.insert(save_grid.to_string(), grid);
            }
            "avo_analysis" => {
                let p: AvoParams = parse_nested_or_flat(&step.params, "avo_analysis")?;
                let vt = resolve_velocity(&step.params, ctx);
                let save_summary = step
                    .params
                    .get("save_summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("avo_qc");
                let out = rustseis_imaging::compute_avo_2d(&traces, &vt, &p, save_summary)?;
                if let Some(gb) = step.params.get("save_gradient_buffer").and_then(|v| v.as_str()) {
                    if !gb.is_empty() {
                        ctx.buffers.insert(gb.to_string(), out.gradient.clone());
                    }
                }
                if let Some(fb) = step.params.get("save_fluid_buffer").and_then(|v| v.as_str()) {
                    if !fb.is_empty() {
                        ctx.buffers.insert(fb.to_string(), out.fluid_factor.clone());
                    }
                }
                ctx.avo_summaries.insert(save_summary.to_string(), out.summary);
                traces = match p.output {
                    AvoOutputMode::Intercept => out.intercept,
                    AvoOutputMode::Gradient => out.gradient,
                    AvoOutputMode::Product => out.product,
                    AvoOutputMode::FluidFactor => out.fluid_factor,
                    AvoOutputMode::Curvature => out.curvature,
                    AvoOutputMode::AngleStack => out.angle_stack,
                };
            }
            "well_tie" => {
                let p: WellTieParams = parse_nested_or_flat(&step.params, "well_tie")?;
                let save_tie = step
                    .params
                    .get("save_tie")
                    .and_then(|v| v.as_str())
                    .unwrap_or("well_tie");
                let res = rustseis_imaging::compute_well_tie_from_file(&traces, &p)?;
                let synth_tr = rustseis_imaging::well_tie_to_trace(&res);
                ctx.well_ties.insert(save_tie.to_string(), res);
                if step
                    .params
                    .get("append_synthetic_trace")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    traces.push(synth_tr);
                }
            }
            "crooked_stack" => {
                let p: CrookedStackParams = parse_nested_or_flat(&step.params, "crooked_stack")?;
                traces = rustseis_imaging::stack_crooked_line_2d(&traces, &p)?;
            }
            "eikonal_psdm" => {
                let p: KirchhoffPsdm2dParams = parse_nested_or_flat(&step.params, "eikonal_psdm")?;
                let grid_name = step
                    .params
                    .get("velocity_grid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("depth_vel");
                let grid = ctx
                    .velocity_grids
                    .get(grid_name)
                    .or_else(|| ctx.velocity_grids.values().next())
                    .cloned()
                    .ok_or_else(|| {
                        SeismicError::NotFound(format!(
                            "VelocityGrid2D '{grid_name}' not found (run vel_to_depth first)"
                        ))
                    })?;
                let out = rustseis_imaging::kirchhoff_psdm_2d(
                    &traces,
                    &grid,
                    &p,
                    ctx.job_control.as_ref(),
                )?;
                if let Some(cb) = step.params.get("save_cig_buffer").and_then(|v| v.as_str()) {
                    if !cb.is_empty() && !out.cig_traces.is_empty() {
                        ctx.buffers.insert(cb.to_string(), out.cig_traces);
                    }
                }
                traces = out.image_traces;
            }
            "rtm2d" => {
                let p: Rtm2dParams = parse_nested_or_flat(&step.params, "rtm2d")?;
                let grid_name = step
                    .params
                    .get("velocity_grid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("depth_vel");
                let grid = ctx
                    .velocity_grids
                    .get(grid_name)
                    .or_else(|| ctx.velocity_grids.values().next())
                    .cloned()
                    .ok_or_else(|| {
                        SeismicError::NotFound(format!(
                            "VelocityGrid2D '{grid_name}' not found (run vel_to_depth first)"
                        ))
                    })?;
                let out = rustseis_imaging::rtm_2d(
                    &traces,
                    &grid,
                    &p,
                    ctx.job_control.as_ref(),
                )?;
                traces = out.image_traces;
            }
            "fwi2d" => {
                let p: Fwi2dParams = parse_nested_or_flat(&step.params, "fwi2d")?;
                let grid_name = step
                    .params
                    .get("velocity_grid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("depth_vel");
                let save_grid = step
                    .params
                    .get("save_grid")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fwi_vel");
                let grid = ctx
                    .velocity_grids
                    .get(grid_name)
                    .or_else(|| ctx.velocity_grids.values().next())
                    .cloned()
                    .ok_or_else(|| {
                        SeismicError::NotFound(format!(
                            "VelocityGrid2D '{grid_name}' not found (run vel_to_depth first)"
                        ))
                    })?;
                let mut out = rustseis_imaging::fwi_2d(
                    &traces,
                    &grid,
                    &p,
                    ctx.job_control.as_ref(),
                )?;
                out.velocity.name = save_grid.to_string();
                ctx.velocity_grids.insert(save_grid.to_string(), out.velocity);
                ctx.fwi_misfits.insert(save_grid.to_string(), out.misfit_history);
                if step
                    .params
                    .get("output_model_traces")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    traces = out.model_traces;
                } else {
                    ctx.buffers.insert(format!("{save_grid}_traces"), out.model_traces);
                }
            }
            "qcomp" => {
                let p: QCompParams = parse_nested_or_flat(&step.params, "qcomp")?;
                rustseis_dsp::apply_qcomp(&mut traces, &p);
            }
            "phase_rot" => {
                let p: PhaseRotParams = parse_nested_or_flat(&step.params, "phase_rot")?;
                rustseis_dsp::apply_phase_rot(&mut traces, &p);
            }
            "mistie" => {
                let p: MistieParams = parse_nested_or_flat(&step.params, "mistie")?;
                let save_report = step
                    .params
                    .get("save_report")
                    .and_then(|v| v.as_str())
                    .unwrap_or("mistie_qc");
                let rep = rustseis_imaging::analyze_and_balance_misties(&mut traces, &p)?;
                ctx.mistie_reports.insert(save_report.to_string(), rep);
            }

            other => {
                return Err(SeismicError::NotFound(format!("Unknown flow tool '{other}'")));
            }
        }

        let sample_stats = traces.first().map(|t| trace_stats(&t.samples));
        step_reports.push(StepReport {
            index: idx,
            tool: step.tool.clone(),
            enabled: true,
            traces_in: n_in,
            traces_out: traces.len(),
            elapsed_ms: t_step.elapsed().as_millis(),
            sample_stats,
        });
    }

    let report = FlowReport {
        flow_name: spec.name.clone(),
        total_elapsed_ms: t_all.elapsed().as_millis(),
        final_trace_count: traces.len(),
        steps: step_reports,
        semblance_panels,
        radon_panels,
        vsp_profiles,
    };

    Ok((traces, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn radon_filter_saves_qc_panel_when_first_cdp_has_low_fold() {
        let mut traces = Vec::new();
        let make_trace = |cdp: i32, offset: f32| {
            let mut samples = vec![0.0f32; 64];
            samples[20] = 1.0;
            let mut tr = Trace::new(4000, samples);
            tr.headers.cdp = cdp;
            tr.headers.offset = offset;
            tr
        };
        // CDP 1 has fold 1 (taper-in), CDP 2 has fold 6 (full fold).
        traces.push(make_trace(1, 100.0));
        for k in 0..6 {
            traces.push(make_trace(2, 100.0 + k as f32 * 50.0));
        }
        let spec: FlowSpec = serde_json::from_value(json!({
            "name": "radon_taper_qc",
            "steps": [{
                "tool": "radon_filter",
                "params": {
                    "ensemble_key": "CDP",
                    "save_panel": "qc_panel",
                    "params": { "mode": "Parabolic", "q_min_ms": -50.0, "q_max_ms": 150.0, "n_q": 21, "q_zone_min_ms": 30.0, "q_zone_max_ms": 150.0, "sparse_iters": 1 }
                }
            }]
        }))
        .unwrap();
        let mut ctx = FlowContext::new();
        let (_out, rep) = execute_flow(&spec, traces, &mut ctx).unwrap();
        assert!(ctx.radon_panels.contains_key("qc_panel"), "expected Radon QC panel to be saved from full-fold CDP");
        assert_eq!(rep.radon_panels.len(), 1);
    }
}
