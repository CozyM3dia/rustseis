pub mod catalog;
pub mod python_node;

pub use catalog::{full_tool_catalog, tier1_tool_catalog, tier2_tool_catalog};
pub use python_node::{PythonNodeParams, execute_python_node};

use rayon::prelude::*;
use rustseis_core::{
    CrookedLineGeometry2D, Ensemble, FirstBreakTable, LandGeometry2D, MarineGeometry2D, MuteTable,
    Result, SeismicError, StaticsTable, SurfaceConsistentAmpTable, Trace, TraceStats, VelocityTable,
    trace_stats,
};
use rustseis_dsp::{
    AdaptiveSubParams, AgcParams, AirBlastParams, BandpassParams, CurveletDenoiseParams,
    DeconParams, ElevationStaticsParams, FkFilterParams, FxDeconParams, GroundRollParams,
    RadonPanel, RadonParams, ScAmpParams, ScDeconParams, Srme2dParams, TarParams, TeqParams,
    TimeVariantBandpassParams, TraceEditParams, TvsParams, TvwParams,
};
use rustseis_imaging::{
    Afd2dParams, AutostatMethod, AutostatParams, DmoFkParams, FdMig2dParams, FirstBreakPickParams,
    Kirchhoff2dParams, KirchhoffPstm2dParams, NmoParams, RefractionStaticsParams, SemblancePanel,
    StackParams, StoltMigParams, Synth2dParams, TrimStatParams, VelocityScanParams,
    VspCorridorStackParams, VspDeconParams, VspGeometryParams, VspSlownessParams,
    VspSlownessProfile, VspWavefieldSepParams,
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
    pub mutes: HashMap<String, MuteTable>,
    pub first_breaks: HashMap<String, FirstBreakTable>,
    pub statics: HashMap<String, StaticsTable>,
    pub sc_amps: HashMap<String, SurfaceConsistentAmpTable>,
    pub radon_panels: HashMap<String, RadonPanel>,
    pub vsp_profiles: HashMap<String, VspSlownessProfile>,
    pub buffers: HashMap<String, Vec<Trace>>,
    pub ensemble_key: String,
}

impl FlowContext {
    pub fn new() -> Self {
        Self {
            velocities: HashMap::new(),
            mutes: HashMap::new(),
            first_breaks: HashMap::new(),
            statics: HashMap::new(),
            sc_amps: HashMap::new(),
            radon_panels: HashMap::new(),
            vsp_profiles: HashMap::new(),
            buffers: HashMap::new(),
            ensemble_key: "FFID".to_string(),
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

    for (idx, step) in spec.steps.iter().enumerate() {
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
                let table: MuteTable = if let Some(name) = step.params.get("mute_table").and_then(|v| v.as_str()) {
                    ctx.mutes.get(name).cloned().ok_or_else(|| {
                        SeismicError::NotFound(format!("Mute table '{name}' not found"))
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
                rustseis_dsp::apply_decon(&mut traces, &p)?;
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

                let mut picked_funcs = Vec::new();
                let n_cdps = cdp_ranges.len();
                let stride = (n_cdps / 8).max(1);
                for (ei, &(cdp, start, end)) in cdp_ranges.iter().enumerate() {
                    let fold = end - start;
                    let selected = if target_cdps.is_empty() {
                        ei % stride == 0 && fold >= 8
                    } else {
                        target_cdps.contains(&cdp)
                    };
                    if selected {
                        let ens = Ensemble::new("CDP", cdp, traces[start..end].to_vec());
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
