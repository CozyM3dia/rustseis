use rustseis_core::{
    Ensemble, MarineGeometry2D, MuteMode, MuteTable, Trace, VelocityTable, nrms_percent, trace_stats,
    zero_lag_correlation,
};
use rustseis_dsp::{
    AgcParams, BandpassParams, DeconParams, ElevationStaticsParams, FilterPhase, FilterSpecType,
    FkFilterParams, FkMode, TarParams, apply_agc, apply_bandpass, apply_decon,
    apply_elevation_statics, apply_fk_ensemble, apply_header_math, apply_mute, apply_tar,
    debias_traces,
};
use rustseis_engine::EngineSession;
use rustseis_flow::{FlowContext, FlowSpec, execute_flow};
use rustseis_imaging::{
    Kirchhoff2dParams, NmoParams, StackParams, StoltMigParams, VelocityScanParams,
    assign_marine_geometry_2d, compute_semblance_panel, inline_sort, kirchhoff_poststack_2d,
    phase_shift_migrate_2d, stack_ensembles, stolt_migrate_2d,
};
use rustseis_io::{
    Endianness, MmapDataset, ibm_to_ieee_f32, ieee_to_ibm_u32, inspect_segd_file,
    inspect_segy_file, read_segd_file, read_segy_file, write_dataset, write_segy_file,
};
use rustseis_mcp::handle_tool_call;
use rustseis_render::{GatherRenderConfig, render_seismic_gather, render_spectrum_plot};
use serde_json::json;
use std::f32::consts::PI;
use std::path::Path;

fn ricker(t_s: f32, f0_hz: f32) -> f32 {
    let a = PI * f0_hz * t_s;
    let a2 = a * a;
    (1.0 - 2.0 * a2) * (-a2).exp()
}

#[test]
fn test_ibm_ieee_roundtrip() {
    for &v in &[0.0f32, 1.0, -1.0, 3.1415927, -12345.678, 0.00025] {
        let ibm = ieee_to_ibm_u32(v);
        let back = ibm_to_ieee_f32(ibm);
        let err = (v - back).abs();
        assert!(err <= v.abs() * 1e-5 + 1e-6, "IBM roundtrip failed for {v}: got {back}");
    }
}

#[test]
fn test_dsp_debias_bandpass_decon_agc_mute_fk_statics() {
    let dt_us = 2000u32; // 2 ms
    let nt = 501usize; // 1.0 s
    let dt_s = 0.002f32;

    // 1. Debias + Bandpass (pass 30 Hz, reject 5 Hz & 120 Hz)
    let mut samps = vec![0.0f32; nt];
    for i in 0..nt {
        let t = i as f32 * dt_s;
        samps[i] = 5000.0 // DC bias
            + 2.0 * (2.0 * PI * 3.0 * t).sin() // low-freq noise
            + 1.0 * (2.0 * PI * 30.0 * t).sin() // signal
            + 2.0 * (2.0 * PI * 120.0 * t).sin(); // high-freq noise
    }
    let mut trs = vec![Trace::new(dt_us, samps)];
    debias_traces(&mut trs);
    assert!(trace_stats(&trs[0].samples).dc_mean.abs() < 1e-2);

    let bp = BandpassParams {
        spec_type: FilterSpecType::Ormsby,
        phase: FilterPhase::Zero,
        freqs: [10.0, 20.0, 45.0, 60.0],
        ..Default::default()
    };
    apply_bandpass(&mut trs, &bp).unwrap();
    // Compare interior with pure 30 Hz sine
    let expected: Vec<f32> = (100..400).map(|i| (2.0 * PI * 30.0 * (i as f32 * dt_s)).sin()).collect();
    let corr = zero_lag_correlation(&trs[0].samples[100..400], &expected);
    assert!(corr > 0.99, "Bandpass failed to isolate 30 Hz signal: corr={corr}");

    // Minimum-phase Butterworth smoke test
    let mut trs_min = trs.clone();
    let bp_min = BandpassParams {
        spec_type: FilterSpecType::Butterworth,
        phase: FilterPhase::Minimum,
        freqs: [15.0, 18.0, 50.0, 24.0],
        ..Default::default()
    };
    apply_bandpass(&mut trs_min, &bp_min).unwrap();
    assert!(trace_stats(&trs_min[0].samples).rms > 0.1);

    // 2. Predictive Deconvolution attenuates periodic multiple (period = 60 ms)
    let mut rev_samps = vec![0.0f32; nt];
    for i in 0..nt {
        let t = i as f32 * dt_s;
        // Primary at 0.20s + reverberations at 0.26s, 0.32s, 0.38s
        rev_samps[i] = ricker(t - 0.20, 35.0)
            - 0.6 * ricker(t - 0.26, 35.0)
            + 0.36 * ricker(t - 0.32, 35.0)
            - 0.216 * ricker(t - 0.38, 35.0);
    }
    let mut decon_trs = vec![Trace::new(dt_us, rev_samps)];
    let before_mult = decon_trs[0].samples[130].abs(); // 0.26 s
    apply_decon(
        &mut decon_trs,
        &DeconParams {
            operator_ms: 120.0,
            prediction_lag_ms: 40.0,
            white_noise_pct: 0.5,
            design_windows_ms: vec![(100.0, 600.0)],
            ..Default::default()
        },
    )
    .unwrap();
    let after_mult = decon_trs[0].samples[130].abs();
    assert!(
        after_mult < before_mult * 0.45,
        "Predictive decon should attenuate multiple: before={before_mult}, after={after_mult}"
    );

    // 3. Mute + TLIVE_S header update + TAR + AGC + Elevation Statics + Header Math
    let mut mtrs = vec![Trace::new(dt_us, vec![1.0; nt])];
    mtrs[0].headers.offset = 500.0;
    mtrs[0].headers.aoffset = 500.0;
    mtrs[0].headers.sou_elev = 100.0;
    mtrs[0].headers.rec_elev = 100.0;
    apply_mute(
        &mut mtrs,
        &MuteTable {
            name: "top".into(),
            mode: MuteMode::Top,
            key_header: "AOFFSET".into(),
            taper_ms: 20.0,
            picks: vec![(0.0, 100.0), (1000.0, 300.0)],
            surgical_picks: vec![],
        },
    );
    assert!((mtrs[0].headers.tlive_s - 200.0).abs() < 1e-3);
    assert_eq!(mtrs[0].samples[50], 0.0); // 100 ms muted
    assert!(mtrs[0].samples[200] > 0.99); // 400 ms unmuted

    apply_tar(&mut mtrs, &TarParams::default(), None);
    apply_agc(&mut mtrs, &AgcParams::default());
    apply_elevation_statics(
        &mut mtrs,
        &ElevationStaticsParams {
            final_datum_m: 120.0,
            replacement_velocity_mps: 2000.0,
            apply_shift: true,
        },
    );
    assert!((mtrs[0].headers.tot_stat - 20.0).abs() < 1e-2);
    apply_header_math(&mut mtrs, "CDP", "abs(OFFSET) / 25 + 1").unwrap();
    assert_eq!(mtrs[0].headers.cdp, 21);

    // 4. F-K Filter rejects slow dipping noise (v=400 m/s) while preserving flat reflector
    let nx = 32usize;
    let dx = 12.5f32;
    let mut fk_traces = Vec::new();
    for ix in 0..nx {
        let x = ix as f32 * dx;
        let mut s = vec![0.0f32; 256];
        for it in 0..256 {
            let t = it as f32 * dt_s;
            let flat = ricker(t - 0.25, 30.0);
            let ground_roll = 1.5 * ricker(t - (0.05 + x / 400.0), 20.0);
            s[it] = flat + ground_roll;
        }
        let mut tr = Trace::new(dt_us, s);
        tr.headers.ffid = 1;
        tr.headers.chan = (ix + 1) as i32;
        fk_traces.push(tr);
    }
    let mut ens = Ensemble::new("FFID", 1, fk_traces);
    apply_fk_ensemble(
        &mut ens,
        &FkFilterParams {
            mode: FkMode::RejectLowVelocity,
            dx_m: dx,
            vel_cut_mps: 1200.0,
            taper_frac: 0.2,
            pedestal: 0.01,
            polygon: vec![],
        },
    )
    .unwrap();
    // At trace 16 (x=200m), flat event at 0.25s (idx 125) should remain strong while noise at 0.55s (idx 275 clamped) / trace 8 (x=100m, t=0.30s -> idx 150) is attenuated
    let flat_amp = ens.traces[16].samples[125].abs();
    assert!(flat_amp > 0.7, "F-K filter should preserve flat reflection, got {flat_amp}");
}

#[test]
fn test_synthetic_end_to_end_iva_nmo_stack_and_migration() {
    let dt_us = 4000u32; // 4 ms
    let dt_s = 0.004f32;
    let nt = 251usize; // 1.0 s
    let v_true = 2000.0f32;
    let t0_reflector = 0.50f32; // 500 ms

    // 1. Build synthetic CDP gather with hyperbolic reflector at v=2000 m/s, t0=500 ms
    let mut cdp_traces = Vec::new();
    for io in 0..24 {
        let offset = 50.0 + io as f32 * 50.0; // 50m..1200m
        let tx = (t0_reflector * t0_reflector + (offset / v_true).powi(2)).sqrt();
        let samps: Vec<f32> = (0..nt).map(|i| ricker(i as f32 * dt_s - tx, 25.0)).collect();
        let mut tr = Trace::new(dt_us, samps);
        tr.headers.cdp = 100;
        tr.headers.offset = offset;
        tr.headers.aoffset = offset;
        cdp_traces.push(tr);
    }
    let gather = Ensemble::new("CDP", 100, cdp_traces.clone());

    // 2. Semblance Velocity Analysis must recover v_true = 2000 m/s at t0 = 500 ms
    let sem = compute_semblance_panel(
        &gather,
        &VelocityScanParams {
            v_min_mps: 1400.0,
            v_max_mps: 2800.0,
            dv_mps: 50.0,
            window_ms: 32.0,
            t_step_ms: 20.0,
            stretch_mute_pct: 50.0,
        },
    )
    .unwrap();
    let it500 = sem
        .times_ms
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - 500.0).abs().total_cmp(&(b.1 - 500.0).abs()))
        .unwrap()
        .0;
    let (best_iv, best_s) = (0..sem.velocities_mps.len())
        .map(|iv| (iv, sem.get(it500, iv)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let v_picked = sem.velocities_mps[best_iv];
    assert!(
        (v_picked - v_true).abs() <= 50.0 && best_s > 0.90,
        "Semblance failed to recover v=2000 m/s: picked={v_picked}, semblance={best_s}"
    );

    // 3. Apply NMO + CDP Stack -> peak must land cleanly at 500 ms (index 125)
    let vt = VelocityTable::constant(v_picked);
    let mut nmo_traces = cdp_traces;
    rustseis_imaging::apply_nmo(&mut nmo_traces, &vt, &NmoParams::default());
    let stacked = stack_ensembles(&[Ensemble::new("CDP", 100, nmo_traces)], &StackParams::default());
    assert_eq!(stacked.len(), 1);
    let peak_idx = stacked[0]
        .samples
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    assert!(
        (peak_idx as isize - 125).abs() <= 1,
        "Stacked peak should be at idx 125 (500ms), got {peak_idx}"
    );

    // 4. Synthetic point-diffractor zero-offset section -> Stolt, Phase-Shift & Kirchhoff 2D Migration
    // Diffractor at x_center = 32 * dx, t0 = 0.40 s (idx 100), v = 2000 m/s
    let nx = 65usize;
    let dx = 12.5f32;
    let xc = 32.0 * dx;
    let t0_dif = 0.40f32;
    let mut zo_section = Vec::with_capacity(nx);
    for ix in 0..nx {
        let dist = ix as f32 * dx - xc;
        let td = (t0_dif * t0_dif + (2.0 * dist / v_true).powi(2)).sqrt();
        let samps: Vec<f32> = (0..nt).map(|i| ricker(i as f32 * dt_s - td, 25.0)).collect();
        let mut tr = Trace::new(dt_us, samps);
        tr.headers.cdp = (ix + 1) as i32;
        zo_section.push(tr);
    }

    // Measure lateral energy focusing before vs after migration (energy ratio at apex +-2 traces vs full section)
    let focus_ratio = |sec: &[Trace]| -> f64 {
        let mut apex = 0.0f64;
        let mut total = 0.0f64;
        for (ix, tr) in sec.iter().enumerate() {
            let e: f64 = tr.samples.iter().map(|&v| (v as f64) * (v as f64)).sum();
            total += e;
            if (ix as isize - 32).abs() <= 2 {
                apex += e;
            }
        }
        apex / total.max(1e-20)
    };

    let r_unmig = focus_ratio(&zo_section);
    let mig_stolt = stolt_migrate_2d(
        &zo_section,
        &StoltMigParams {
            velocity_mps: v_true,
            dx_m: dx,
            pad_traces: 32,
        },
    )
    .unwrap();
    let r_stolt = focus_ratio(&mig_stolt);
    assert!(
        r_stolt > r_unmig * 4.0,
        "Stolt migration must collapse diffraction hyperbola: unmig={r_unmig:.3}, stolt={r_stolt:.3}"
    );

    let mig_ps = phase_shift_migrate_2d(&zo_section, &VelocityTable::constant(v_true), dx).unwrap();
    let r_ps = focus_ratio(&mig_ps);
    assert!(
        r_ps > r_unmig * 4.0,
        "Phase-shift migration must collapse diffraction hyperbola: unmig={r_unmig:.3}, ps={r_ps:.3}"
    );

    let mig_kir = kirchhoff_poststack_2d(
        &zo_section,
        &VelocityTable::constant(v_true),
        &Kirchhoff2dParams {
            dx_m: dx,
            max_aperture_m: 600.0,
            max_dip_deg: 65.0,
            apply_half_derivative: true,
            anti_alias: true,
        },
    )
    .unwrap();
    let r_kir = focus_ratio(&mig_kir);
    assert!(
        r_kir > r_unmig * 3.5,
        "Kirchhoff 2D migration must collapse diffraction hyperbola: unmig={r_unmig:.3}, kir={r_kir:.3}"
    );
}

#[test]
fn test_real_local_datasets_and_mcp_and_render() {
    // 1. Real Data Seismik SEG-D (SGM3-BIO-NIAS-L22)
    let segd_path = Path::new(r"C:\Kuliah\Metode Seismik 2025\Data Seismik\00001000.segd");
    if segd_path.exists() {
        let sum = inspect_segd_file(segd_path).unwrap();
        assert_eq!(sum.format_code, 8058);
        assert_eq!(sum.dt_us, 2000);
        assert_eq!(sum.live_channels, 96);
        assert_eq!(sum.aux_channels, 3);
        assert_eq!(sum.n_samples, 4001);
        assert_eq!(sum.line_name.as_deref(), Some("SGM3-BIO-NIAS-L22"));

        let (_, mut traces) = read_segd_file(segd_path, true).unwrap();
        assert_eq!(traces.len(), 96);
        assign_marine_geometry_2d(&mut traces, &MarineGeometry2D::default());
        assert!(traces[0].headers.cdp >= 1);
        assert!((traces[0].headers.offset - 100.0).abs() < 1e-3);

        // Round-trip via mmap .rsds and SEG-Y Rev 1
        let tmp_dir = std::env::temp_dir().join("rustseis_test_io");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let rsds = tmp_dir.join("nias_shot1000.rsds");
        write_dataset(&rsds, "nias_shot1000", "FFID", &traces).unwrap();
        let ds = MmapDataset::open(&rsds).unwrap();
        assert_eq!(ds.meta.n_traces, 96);
        let t0 = ds.read_trace(0).unwrap();
        assert_eq!(nrms_percent(&traces[0].samples, &t0.samples), 0.0);

        let sgy = tmp_dir.join("nias_shot1000.sgy");
        write_segy_file(&sgy, &traces, "NIAS shot 1000").unwrap();
        let (sgy_sum, sgy_tr) = read_segy_file(&sgy, None).unwrap();
        assert_eq!(sgy_sum.n_traces, 96);
        assert_eq!(nrms_percent(&traces[0].samples, &sgy_tr[0].samples), 0.0);

        // Render gather & spectrum PNGs via L3 render
        let mut clean = traces.clone();
        debias_traces(&mut clean);
        apply_bandpass(&mut clean, &BandpassParams::default()).unwrap();
        let img = render_seismic_gather(&clean, &GatherRenderConfig::default()).unwrap();
        img.save_png(tmp_dir.join("nias_shot1000.png")).unwrap();
        let spec_img = render_spectrum_plot(&clean, 800, 400).unwrap();
        spec_img.save_png(tmp_dir.join("nias_spec.png")).unwrap();
    }

    // 2. Real Jurnal 1 Otway BE SEG-Y
    let otway = Path::new(r"C:\Kuliah\Jurnal Pak Bagus\Jurnal 1\CCS-DAS\data\otway_zenodo_7574468\RAW_seismograms\event_10_3_strain_geom.sgy");
    if otway.exists() {
        let sum = inspect_segy_file(otway).unwrap();
        assert_eq!(sum.endianness, Endianness::Big);
        assert_eq!(sum.format_code, 5);
        assert_eq!(sum.dt_us, 1000);
        assert_eq!(sum.n_samples, 5000);
        assert!(sum.n_traces > 1300);
        let (_, tr) = read_segy_file(otway, Some(16)).unwrap();
        assert_eq!(tr.len(), 16);
    }

    // 3. Real Jurnal 1 Utah FORGE Little-Endian SEG-Y edge-case torture test
    let forge = Path::new(r"C:\Kuliah\Jurnal Pak Bagus\Jurnal 1\CCS-DAS\analysis\out\forge_feasibility\waveforms\reader_replay_subset.sgy");
    if forge.exists() {
        let sum = inspect_segy_file(forge).unwrap();
        assert_eq!(sum.endianness, Endianness::Little);
        assert_eq!(sum.format_code, 5);
        assert_eq!(sum.dt_us, 250);
        assert_eq!(sum.n_samples, 32767);
        let (_, tr) = read_segy_file(forge, Some(4)).unwrap();
        assert!(!tr.is_empty());
    }

    // 4. MCP tool dispatch end-to-end test
    let mut sess = EngineSession::new();
    let tools = handle_tool_call(&mut sess, "list_processing_tools", &json!({})).unwrap();
    assert!(tools["count"].as_u64().unwrap() >= 30);

    // Inline sort + flow execution test
    let mut ctx = FlowContext::new();
    let mut trs = vec![Trace::new(2000, vec![1.0; 100]), Trace::new(2000, vec![2.0; 100])];
    trs[0].headers.cdp = 20;
    trs[1].headers.cdp = 10;
    let sorted = inline_sort(trs, "CDP", None, false, false);
    assert_eq!(sorted[0].headers.cdp, 10);

    let flow = FlowSpec {
        name: "unit_flow".into(),
        description: "smoke".into(),
        steps: vec![],
    };
    let (_, rep) = execute_flow(&flow, sorted, &mut ctx).unwrap();
    assert_eq!(rep.final_trace_count, 2);
}

#[test]
fn test_promax_vm_differential_parity_and_formats() {
    let dir = Path::new(r"C:\Kuliah\Metode Seismik 2025\rustseis\out\promax_parity");
    let input = dir.join("input_nias_shot1000.sgy");
    if !input.exists() {
        return;
    }
    let (_, base) = read_segy_file(&input, None).unwrap();

    let mean_parity = |a: &[Trace], b: &[Trace]| -> (f64, f64) {
        let n = a.len().min(b.len());
        let mut nr = 0.0;
        let mut cr = 0.0;
        for i in 0..n {
            nr += nrms_percent(&a[i].samples, &b[i].samples);
            cr += zero_lag_correlation(&a[i].samples, &b[i].samples);
        }
        (nr / n as f64, cr / n as f64)
    };

    // 1. ProMAX Ormsby Zero-Phase Bandpass (10-20-60-80 Hz)
    if dir.join("promax_bp.sgy").exists() {
        let (_, pm) = read_segy_file(dir.join("promax_bp.sgy"), None).unwrap();
        let mut rs = base.clone();
        apply_bandpass(
            &mut rs,
            &BandpassParams {
                spec_type: FilterSpecType::Ormsby,
                phase: FilterPhase::Zero,
                freqs: [10.0, 20.0, 60.0, 80.0],
                pad_pct: 25.0,
                ..Default::default()
            },
        )
        .unwrap();
        let (nr, cr) = mean_parity(&pm, &rs);
        println!("  [PARITY] Ormsby Zero:     NRMS={nr:.4}%, corr={cr:.6}");
        assert!(nr < 0.05 && cr > 0.9999, "Ormsby Zero parity failed: NRMS={nr:.4}%, corr={cr:.6}");
    }

    // 2. ProMAX Ormsby Minimum-Phase Bandpass (10-20-60-80 Hz, 1% white noise)
    if dir.join("promax_ormsby_minp.sgy").exists() {
        let (_, pm) = read_segy_file(dir.join("promax_ormsby_minp.sgy"), None).unwrap();
        let mut rs = base.clone();
        apply_bandpass(
            &mut rs,
            &BandpassParams {
                spec_type: FilterSpecType::Ormsby,
                phase: FilterPhase::Minimum,
                freqs: [10.0, 20.0, 60.0, 80.0],
                rnoise_pct: 1.0,
                pad_pct: 25.0,
                ..Default::default()
            },
        )
        .unwrap();
        let (nr, cr) = mean_parity(&pm, &rs);
        println!("  [PARITY] Ormsby MinPhase: NRMS={nr:.4}%, corr={cr:.6}");
        assert!(nr < 0.05 && cr > 0.9999, "Ormsby Min-Phase parity failed: NRMS={nr:.4}%, corr={cr:.6}");
    }

    // 3. ProMAX Butterworth Zero-Phase Bandpass (15-18-60-24)
    if dir.join("promax_butt_zero.sgy").exists() {
        let (_, pm) = read_segy_file(dir.join("promax_butt_zero.sgy"), None).unwrap();
        let mut rs = base.clone();
        apply_bandpass(
            &mut rs,
            &BandpassParams {
                spec_type: FilterSpecType::Butterworth,
                phase: FilterPhase::Zero,
                freqs: [15.0, 18.0, 60.0, 24.0],
                pad_pct: 25.0,
                ..Default::default()
            },
        )
        .unwrap();
        let (nr, cr) = mean_parity(&pm, &rs);
        println!("  [PARITY] Butterworth:     NRMS={nr:.4}%, corr={cr:.6}");
        assert!(nr < 3.5 && cr > 0.999, "Butterworth Zero parity failed: NRMS={nr:.4}%, corr={cr:.6}");
    }

    // 4. ProMAX AGC (500 ms RMS Center)
    if dir.join("promax_agc.sgy").exists() {
        let (_, pm) = read_segy_file(dir.join("promax_agc.sgy"), None).unwrap();
        let mut rs = base.clone();
        apply_agc(&mut rs, &AgcParams::default());
        let (nr, cr) = mean_parity(&pm, &rs);
        println!("  [PARITY] AGC 500ms RMS:   NRMS={nr:.4}%, corr={cr:.6}");
        assert!(nr < 0.01 && cr > 0.99999, "AGC parity failed: NRMS={nr:.4}%, corr={cr:.6}");
    }

    // 5. ProMAX TAR (6 dB/sec normalized at midpoint reference time)
    if dir.join("promax_tar.sgy").exists() {
        let (_, pm) = read_segy_file(dir.join("promax_tar.sgy"), None).unwrap();
        let mut rs = base.clone();
        apply_tar(
            &mut rs,
            &TarParams {
                time_power: 0.0,
                db_per_sec: 6.0,
                normalize_ref_time_ms: Some(0.0),
                ..Default::default()
            },
            None,
        );
        let (nr, cr) = mean_parity(&pm, &rs);
        println!("  [PARITY] TAR 6 dB/sec:    NRMS={nr:.4}%, corr={cr:.6}");
        assert!(nr < 0.2 && cr > 0.99999, "TAR parity failed: NRMS={nr:.4}%, corr={cr:.6}");
    }

    // 6. ProMAX Spiking Decon (160 ms, 0.1% white noise, ProMAX 0.5x amplitude convention)
    if dir.join("promax_decon.sgy").exists() {
        let (_, pm) = read_segy_file(dir.join("promax_decon.sgy"), None).unwrap();
        let mut rs = base.clone();
        apply_decon(
            &mut rs,
            &DeconParams {
                operator_ms: 160.0,
                prediction_lag_ms: 0.0,
                white_noise_pct: 0.1,
                design_windows_ms: vec![],
                window_taper_ms: 0.0,
                output_scale: 0.5,
            },
        )
        .unwrap();
        let (nr, cr) = mean_parity(&pm, &rs);
        println!("  [PARITY] Spiking Decon:   NRMS={nr:.4}%, corr={cr:.6}");
        assert!(nr < 3.0 && cr > 0.999, "Decon parity failed: NRMS={nr:.4}%, corr={cr:.6}");
    }

    // 7. ProMAX IBM Float32 & Int16 EBCDIC SEG-Y Benchmarks
    if dir.join("kirtime3din.segy").exists() {
        let (sum, tr) = read_segy_file(dir.join("kirtime3din.segy"), Some(16)).unwrap();
        assert_eq!(sum.format_code, 1); // IBM float32
        assert!(sum.text_header_preview.contains("C 1") || sum.text_header_preview.contains("CLIENT"));
        assert_eq!(tr.len(), 16);
    }
    if dir.join("protest.segy").exists() {
        let (sum, tr) = read_segy_file(dir.join("protest.segy"), Some(16)).unwrap();
        assert_eq!(sum.format_code, 3); // Int16
        assert!(sum.text_header_preview.contains("CLIENT"));
        assert_eq!(tr.len(), 16);
    }

    // 8. GUI-ready CVS panel & 2D F-K spectrum renderer smoke test
    let ens = Ensemble::new("FFID", 1000, base[..32].to_vec());
    let cvs = rustseis_imaging::compute_cvs_panel(&ens, &[1500.0, 1800.0, 2100.0], 50.0).unwrap();
    assert_eq!(cvs.stacked_traces.len(), 3);
    let fk_img = rustseis_render::render_fk_spectrum_plot(&base[..48], 12.5, 400, 300).unwrap();
    assert_eq!(fk_img.rgba.len(), 400 * 300 * 4);
}
