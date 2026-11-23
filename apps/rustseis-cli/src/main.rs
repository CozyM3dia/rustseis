use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rustseis_core::{nrms_percent, zero_lag_correlation};
use rustseis_engine::{EngineCommand, EngineSession};
use rustseis_flow::{FlowSpec, full_tool_catalog};
use rustseis_render::{GatherRenderConfig, SeismicColormap, render_radon_panel, render_seismic_gather, render_semblance_panel, render_spectrum_plot};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rustseis", version, about = "Clean-room Rust seismic data processing suite (ProMAX Tier 1 + MCP)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all implemented Tier 1 processing modules and default JSON parameter schemas.
    Tools {
        #[arg(long)]
        json: bool,
    },
    /// Inspect a SEG-D (.segd) or SEG-Y (.sgy/.segy) file header.
    Inspect {
        path: PathBuf,
    },
    /// Execute a declarative JSON or TOML processing flow end-to-end.
    Run {
        flow: PathBuf,
        /// Optional PNG path to render the final traces (gather or 2D section).
        #[arg(long)]
        render_png: Option<PathBuf>,
        /// Optional PNG path to render the average amplitude spectrum.
        #[arg(long)]
        spectrum_png: Option<PathBuf>,
        /// Optional PNG path to render the first semblance panel if `vaprecom71` ran.
        #[arg(long)]
        semblance_png: Option<PathBuf>,
        /// Optional PNG path to render the first Radon panel if `radon_trans` / `radon_filter` ran.
        #[arg(long)]
        radon_png: Option<PathBuf>,
        /// Optional SQLite OPF database path to persist all solved tables & run history.
        #[arg(long)]
        save_opf: Option<PathBuf>,
        /// Max time in ms for gather/section PNG rendering.
        #[arg(long)]
        max_time_ms: Option<f32>,
    },
    /// Render a SEG-D or SEG-Y file directly to a PNG image for quick visual QC.
    Render {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, default_value_t = 1200)]
        width: u32,
        #[arg(long, default_value_t = 800)]
        height: u32,
        #[arg(long)]
        max_time_ms: Option<f32>,
        #[arg(long)]
        max_traces: Option<usize>,
        #[arg(long, default_value_t = false)]
        greyscale: bool,
    },
    /// Compare two SEG-Y files for differential numerical parity (NRMS % & correlation).
    Parity {
        reference: PathBuf,
        candidate: PathBuf,
        #[arg(long)]
        max_traces: Option<usize>,
    },
    /// Start the Model Context Protocol (MCP) JSON-RPC stdio server for AI agents.
    Mcp,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Tools { json } => {
            let tools = full_tool_catalog();
            if json {
                println!("{}", serde_json::to_string_pretty(&tools)?);
            } else {
                println!("RustSeis Tier 1 + Tier 2 Tool Catalog ({} modules):", tools.len());
                for t in tools {
                    println!("  {:<20} [{:<12?}] {:<26} — {}", t.menu_id, t.archetype, t.category, t.label);
                }
            }
        }
        Commands::Inspect { path } => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "segd" {
                let s = rustseis_io::inspect_segd_file(&path)?;
                println!("{}", serde_json::to_string_pretty(&s)?);
            } else {
                let s = rustseis_io::inspect_segy_file(&path)?;
                println!("{}", serde_json::to_string_pretty(&s)?);
            }
        }
        Commands::Run {
            flow,
            render_png,
            spectrum_png,
            semblance_png,
            radon_png,
            save_opf,
            max_time_ms,
        } => {
            let text = fs::read_to_string(&flow)
                .with_context(|| format!("Failed to read flow file {}", flow.display()))?;
            let spec: FlowSpec = if flow
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("toml"))
                .unwrap_or(false)
            {
                toml::from_str(&text)?
            } else {
                serde_json::from_str(&text)?
            };

            let mut session = EngineSession::new();
            let _val = session.dispatch(EngineCommand::RunFlow(spec))?;
            if let Some(ref rep) = session.last_report {
                println!(
                    "Flow '{}' finished in {} ms -> {} final traces:",
                    rep.flow_name, rep.total_elapsed_ms, rep.final_trace_count
                );
                for st in &rep.steps {
                    let status = if st.enabled { "OK" } else { "SKIP" };
                    let rms_str = st
                        .sample_stats
                        .as_ref()
                        .map(|s| format!("rms={:.4} dc={:.4}", s.rms, s.dc_mean))
                        .unwrap_or_default();
                    println!(
                        "  [{:02}] {:<18} {:<4} {:5} -> {:5} traces ({:5} ms) {}",
                        st.index, st.tool, status, st.traces_in, st.traces_out, st.elapsed_ms, rms_str
                    );
                }

                if let Some(sp) = semblance_png {
                    if let Some(panel) = rep.semblance_panels.first() {
                        let pick = panel.auto_pick(1200.0, 0.05, 250.0);
                        let img = render_semblance_panel(
                            &panel.times_ms,
                            &panel.velocities_mps,
                            &panel.values,
                            Some(&pick),
                            800,
                            900,
                        )?;
                        img.save_png(&sp)?;
                        println!("Saved semblance panel PNG -> {}", sp.display());
                    }
                }
                if let Some(rp_path) = radon_png {
                    if let Some(panel) = rep.radon_panels.first() {
                        let img = render_radon_panel(
                            &panel.times_ms,
                            &panel.q_values_ms,
                            &panel.data,
                            None,
                            900,
                            800,
                        )?;
                        img.save_png(&rp_path)?;
                        println!("Saved Radon panel PNG -> {}", rp_path.display());
                    }
                }
            }

            if let Some(opf_path) = save_opf {
                session.dispatch(EngineCommand::SaveOpfDatabase { db_path: opf_path.clone() })?;
                println!("Saved SQLite OPF database -> {}", opf_path.display());
            }

            if let Some(rp) = render_png {
                let mut cfg = GatherRenderConfig::default();
                cfg.max_time_ms = max_time_ms;
                let img = render_seismic_gather(&session.active_traces, &cfg)?;
                img.save_png(&rp)?;
                println!("Saved seismic PNG -> {}", rp.display());
            }

            if let Some(sp) = spectrum_png {
                let img = render_spectrum_plot(&session.active_traces, 900, 500)?;
                img.save_png(&sp)?;
                println!("Saved spectrum PNG -> {}", sp.display());
            }
        }
        Commands::Render {
            input,
            output,
            width,
            height,
            max_time_ms,
            max_traces,
            greyscale,
        } => {
            let ext = input
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let traces = if ext == "segd" {
                rustseis_io::read_segd_file(&input, true)?.1
            } else {
                rustseis_io::read_segy_file(&input, max_traces)?.1
            };
            let cfg = GatherRenderConfig {
                width,
                height,
                colormap: if greyscale {
                    SeismicColormap::Greyscale
                } else {
                    SeismicColormap::BlueWhiteRed
                },
                max_time_ms,
                ..Default::default()
            };
            let img = render_seismic_gather(&traces, &cfg)?;
            img.save_png(&output)?;
            println!("Rendered {} traces -> {}", traces.len(), output.display());
        }
        Commands::Parity {
            reference,
            candidate,
            max_traces,
        } => {
            let (_, r) = rustseis_io::read_segy_file(&reference, max_traces)?;
            let (_, c) = rustseis_io::read_segy_file(&candidate, max_traces)?;
            let n = r.len().min(c.len());
            let mut nrms_sum = 0.0;
            let mut corr_sum = 0.0;
            let mut max_nrms = 0.0f64;
            for i in 0..n {
                let nr = nrms_percent(&r[i].samples, &c[i].samples);
                let cr = zero_lag_correlation(&r[i].samples, &c[i].samples);
                nrms_sum += nr;
                corr_sum += cr;
                if nr > max_nrms {
                    max_nrms = nr;
                }
            }
            let mean_nrms = nrms_sum / n.max(1) as f64;
            let mean_corr = corr_sum / n.max(1) as f64;
            println!(
                "Parity ({} traces): mean_NRMS={:.4}% (max={:.4}%), mean_corr={:.6} -> {}",
                n,
                mean_nrms,
                max_nrms,
                mean_corr,
                if mean_nrms < 1.0 && mean_corr > 0.998 { "PASS" } else { "CHECK" }
            );
        }
        Commands::Mcp => {
            rustseis_mcp::run_stdio_mcp_server()?;
        }
    }
    Ok(())
}
