use rustseis_core::{Result, SeismicError, nrms_percent, trace_stats, zero_lag_correlation};
use rustseis_engine::{EngineCommand, EngineSession};
use rustseis_flow::{FlowSpec, full_tool_catalog};
use rustseis_render::{GatherRenderConfig, compute_average_spectrum};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

/// Returns the MCP tool manifest advertised via `tools/list` (Tier 1 + Tier 2).
pub fn mcp_tool_definitions() -> Value {
    json!([
        {
            "name": "list_processing_tools",
            "description": "List all Tier 1 & Tier 2 ProMAX/VISTA-compatible seismic processing tools and their default parameter schemas.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "inspect_seismic_file",
            "description": "Inspect a SEG-D (.segd) or SEG-Y (.sgy/.segy) file header without loading all traces.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path to a .segd or .sgy/.segy file" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "run_flow",
            "description": "Execute a declarative ProMAX-compatible JSON processing flow (Tier 1 + Tier 2, including IF/SPLIT and Python nodes).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "flow": { "type": "object", "description": "FlowSpec JSON with name and steps[]" }
                },
                "required": ["flow"]
            }
        },
        {
            "name": "inspect_active_traces",
            "description": "Summarize trace count, headers, DC/RMS/peak stats, and dominant spectral frequency of the active session traces.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "inspect_tables",
            "description": "List all named OPF tables in the active session (velocities, mutes, first_breaks, statics, sc_amps, radon_panels, vsp_profiles, buffers).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "save_opf_db",
            "description": "Persist all session OPF tables and last flow report to a SQLite OPF database file.",
            "inputSchema": {
                "type": "object",
                "properties": { "db_path": { "type": "string" } },
                "required": ["db_path"]
            }
        },
        {
            "name": "load_opf_db",
            "description": "Load persisted OPF tables (VEL, MUTE, FBPICK, STAT, SCAMP) from a SQLite OPF database file into the active session.",
            "inputSchema": {
                "type": "object",
                "properties": { "db_path": { "type": "string" } },
                "required": ["db_path"]
            }
        },
        {
            "name": "render_gather_png",
            "description": "Render the active seismic traces (gather or 2D section) to a PNG image for visual QC.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "output_path": { "type": "string" },
                    "width": { "type": "integer", "default": 1200 },
                    "height": { "type": "integer", "default": 800 },
                    "max_time_ms": { "type": "number" },
                    "draw_wiggles": { "type": "boolean", "default": true }
                },
                "required": ["output_path"]
            }
        },
        {
            "name": "render_spectrum_png",
            "description": "Render the average 1D amplitude spectrum (dB vs Hz) of the active traces to PNG.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "output_path": { "type": "string" },
                    "width": { "type": "integer", "default": 900 },
                    "height": { "type": "integer", "default": 500 }
                },
                "required": ["output_path"]
            }
        },
        {
            "name": "render_fk_png",
            "description": "Render the 2D F-K log-amplitude spectrum (k vs f) of the active gather to PNG.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "output_path": { "type": "string" },
                    "dx_m": { "type": "number", "default": 12.5 },
                    "width": { "type": "integer", "default": 900 },
                    "height": { "type": "integer", "default": 700 }
                },
                "required": ["output_path"]
            }
        },
        {
            "name": "render_radon_png",
            "description": "Render a named Radon (tau, q) panel from FlowContext to PNG with optional multiple-zone boundary lines.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "panel_name": { "type": "string", "default": "radon_qc" },
                    "output_path": { "type": "string" },
                    "q_zone_min_ms": { "type": "number" },
                    "q_zone_max_ms": { "type": "number" },
                    "width": { "type": "integer", "default": 900 },
                    "height": { "type": "integer", "default": 700 }
                },
                "required": ["output_path"]
            }
        },
        {
            "name": "compare_segy_parity",
            "description": "Compute differential numerical parity (NRMS % and zero-lag correlation) between two SEG-Y files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "reference_path": { "type": "string" },
                    "candidate_path": { "type": "string" },
                    "max_traces": { "type": "integer", "default": 200 }
                },
                "required": ["reference_path", "candidate_path"]
            }
        },
        {
            "name": "undo",
            "description": "Undo the last mutating flow execution in the active RustSeis session.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

pub fn handle_tool_call(session: &mut EngineSession, name: &str, args: &Value) -> Result<Value> {
    match name {
        "list_processing_tools" => {
            let cat = full_tool_catalog();
            Ok(json!({ "count": cat.len(), "tools": cat }))
        }
        "inspect_seismic_file" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| SeismicError::InvalidParameter {
                    param: "path".into(),
                    message: "Missing path".into(),
                })?;
            let p = PathBuf::from(path);
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "segd" {
                let s = rustseis_io::inspect_segd_file(&p)?;
                Ok(json!({ "format": "SEG-D", "summary": s }))
            } else {
                let s = rustseis_io::inspect_segy_file(&p)?;
                Ok(json!({ "format": "SEG-Y", "summary": s }))
            }
        }
        "run_flow" => {
            let flow_val = args.get("flow").ok_or_else(|| SeismicError::InvalidParameter {
                param: "flow".into(),
                message: "Missing 'flow' object".into(),
            })?;
            let spec: FlowSpec = serde_json::from_value(flow_val.clone())
                .map_err(|e| SeismicError::Serialization(e.to_string()))?;
            session.dispatch(EngineCommand::RunFlow(spec))
        }
        "inspect_active_traces" => {
            if session.active_traces.is_empty() {
                return Ok(json!({ "trace_count": 0 }));
            }
            let first = &session.active_traces[0];
            let stats = trace_stats(&first.samples);
            let (freqs, db) = compute_average_spectrum(&session.active_traces)?;
            let peak_idx = db
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i)
                .unwrap_or(0);
            let dom_freq_hz = freqs.get(peak_idx).copied().unwrap_or(0.0);
            Ok(json!({
                "trace_count": session.active_traces.len(),
                "dt_us": first.dt_us,
                "n_samples": first.samples.len(),
                "duration_ms": first.duration_ms(),
                "first_trace_stats": stats,
                "dominant_freq_hz": dom_freq_hz,
                "first_ffid": first.headers.ffid,
                "first_cdp": first.headers.cdp,
                "last_cdp": session.active_traces.last().map(|t| t.headers.cdp).unwrap_or(0),
            }))
        }
        "inspect_tables" => {
            let ctx = &session.context;
            Ok(json!({
                "velocities": ctx.velocities.keys().collect::<Vec<_>>(),
                "mutes": ctx.mutes.keys().collect::<Vec<_>>(),
                "first_breaks": ctx.first_breaks.keys().collect::<Vec<_>>(),
                "statics": ctx.statics.keys().collect::<Vec<_>>(),
                "sc_amps": ctx.sc_amps.keys().collect::<Vec<_>>(),
                "radon_panels": ctx.radon_panels.keys().collect::<Vec<_>>(),
                "vsp_profiles": ctx.vsp_profiles.keys().collect::<Vec<_>>(),
                "buffers": ctx.buffers.keys().collect::<Vec<_>>(),
            }))
        }
        "save_opf_db" => {
            let p = args.get("db_path").and_then(|v| v.as_str()).unwrap_or("project.opf.sqlite");
            session.dispatch(EngineCommand::SaveOpfDatabase { db_path: PathBuf::from(p) })
        }
        "load_opf_db" => {
            let p = args.get("db_path").and_then(|v| v.as_str()).unwrap_or("project.opf.sqlite");
            session.dispatch(EngineCommand::LoadOpfDatabase { db_path: PathBuf::from(p) })
        }
        "render_gather_png" => {
            let out = args
                .get("output_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| SeismicError::InvalidParameter {
                    param: "output_path".into(),
                    message: "Missing output_path".into(),
                })?;
            let mut cfg = GatherRenderConfig::default();
            if let Some(w) = args.get("width").and_then(|v| v.as_u64()) { cfg.width = w as u32; }
            if let Some(h) = args.get("height").and_then(|v| v.as_u64()) { cfg.height = h as u32; }
            if let Some(mt) = args.get("max_time_ms").and_then(|v| v.as_f64()) { cfg.max_time_ms = Some(mt as f32); }
            if let Some(dw) = args.get("draw_wiggles").and_then(|v| v.as_bool()) { cfg.draw_wiggles = dw; }
            session.dispatch(EngineCommand::RenderGatherPng {
                output_path: PathBuf::from(out),
                config: cfg,
            })
        }
        "render_spectrum_png" => {
            let out = args
                .get("output_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| SeismicError::InvalidParameter {
                    param: "output_path".into(),
                    message: "Missing output_path".into(),
                })?;
            let w = args.get("width").and_then(|v| v.as_u64()).unwrap_or(900) as u32;
            let h = args.get("height").and_then(|v| v.as_u64()).unwrap_or(500) as u32;
            session.dispatch(EngineCommand::RenderSpectrumPng {
                output_path: PathBuf::from(out),
                width: w,
                height: h,
            })
        }
        "render_fk_png" => {
            let out = args
                .get("output_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| SeismicError::InvalidParameter {
                    param: "output_path".into(),
                    message: "Missing output_path".into(),
                })?;
            let dx = args.get("dx_m").and_then(|v| v.as_f64()).unwrap_or(12.5) as f32;
            let w = args.get("width").and_then(|v| v.as_u64()).unwrap_or(900) as u32;
            let h = args.get("height").and_then(|v| v.as_u64()).unwrap_or(700) as u32;
            session.dispatch(EngineCommand::RenderFkPng {
                output_path: PathBuf::from(out),
                dx_m: dx,
                width: w,
                height: h,
            })
        }
        "render_radon_png" => {
            let out = args
                .get("output_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| SeismicError::InvalidParameter {
                    param: "output_path".into(),
                    message: "Missing output_path".into(),
                })?;
            let panel_name = args.get("panel_name").and_then(|v| v.as_str()).unwrap_or("radon_qc").to_string();
            let zone_bounds_ms = match (
                args.get("q_zone_min_ms").and_then(|v| v.as_f64()),
                args.get("q_zone_max_ms").and_then(|v| v.as_f64()),
            ) {
                (Some(a), Some(b)) => Some((a as f32, b as f32)),
                _ => None,
            };
            let w = args.get("width").and_then(|v| v.as_u64()).unwrap_or(900) as u32;
            let h = args.get("height").and_then(|v| v.as_u64()).unwrap_or(700) as u32;
            session.dispatch(EngineCommand::RenderRadonPng {
                panel_name,
                output_path: PathBuf::from(out),
                zone_bounds_ms,
                width: w,
                height: h,
            })
        }
        "compare_segy_parity" => {
            let r_path = args.get("reference_path").and_then(|v| v.as_str()).unwrap_or("");
            let c_path = args.get("candidate_path").and_then(|v| v.as_str()).unwrap_or("");
            let max_tr = args.get("max_traces").and_then(|v| v.as_u64()).map(|n| n as usize);
            let (_, ref_tr) = rustseis_io::read_segy_file(r_path, max_tr)?;
            let (_, cand_tr) = rustseis_io::read_segy_file(c_path, max_tr)?;
            let n = ref_tr.len().min(cand_tr.len());
            if n == 0 {
                return Err(SeismicError::InvalidFormat("No overlapping traces to compare".into()));
            }
            let mut nrms_sum = 0.0f64;
            let mut corr_sum = 0.0f64;
            let mut max_nrms = 0.0f64;
            for i in 0..n {
                let nr = nrms_percent(&ref_tr[i].samples, &cand_tr[i].samples);
                let cr = zero_lag_correlation(&ref_tr[i].samples, &cand_tr[i].samples);
                nrms_sum += nr;
                corr_sum += cr;
                if nr > max_nrms { max_nrms = nr; }
            }
            let mean_nrms = nrms_sum / n as f64;
            let mean_corr = corr_sum / n as f64;
            Ok(json!({
                "traces_compared": n,
                "mean_nrms_pct": mean_nrms,
                "max_nrms_pct": max_nrms,
                "mean_zero_lag_corr": mean_corr,
                "pass_1pct_gate": mean_nrms < 1.0 && mean_corr > 0.998
            }))
        }
        "undo" => Ok(json!({ "undone": session.undo() })),
        other => Err(SeismicError::NotFound(format!("Unknown MCP tool '{other}'"))),
    }
}

/// Run a newline-delimited JSON-RPC 2.0 MCP server over stdio (`rustseis mcp`).
pub fn run_stdio_mcp_server() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut session = EngineSession::new();

    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}});
                writeln!(out, "{err}")?;
                out.flush()?;
                continue;
            }
        };

        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));

        let resp = match method {
            "initialize" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "rustseis-mcp", "version": env!("CARGO_PKG_VERSION") }
                }
            })),
            "notifications/initialized" => None,
            "tools/list" => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": mcp_tool_definitions() }
            })),
            "tools/call" => {
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let tool_args = params.get("arguments").cloned().unwrap_or(json!({}));
                match handle_tool_call(&mut session, tool_name, &tool_args) {
                    Ok(val) => Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&val).unwrap_or_default() }]
                        }
                    })),
                    Err(e) => Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "isError": true,
                            "content": [{ "type": "text", "text": e.to_string() }]
                        }
                    })),
                }
            }
            _ => id.map(|req_id| {
                json!({
                    "jsonrpc": "2.0",
                    "id": req_id,
                    "error": { "code": -32601, "message": format!("Method not found: {method}") }
                })
            }),
        };

        if let Some(r) = resp {
            writeln!(out, "{r}")?;
            out.flush()?;
        }
    }
    Ok(())
}
