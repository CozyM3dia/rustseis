use rustseis_core::{
    CrookedLineGeometry2D, Ensemble, Trace, TraceHeaders, VelocityFunction, VelocityTable,
    zero_lag_correlation,
};
use rustseis_dsp::{
    AirBlastParams, CurveletDenoiseParams, FxDeconMethod, FxDeconParams, GroundRollParams,
    RadonFilterAction, RadonMode, RadonParams, ScAmpMeasure, ScAmpParams, ScDeconParams,
    Srme2dParams, apply_airblast_attenuation, apply_curvelet_denoise, apply_fx_decon,
    apply_ground_roll_attenuation, apply_radon_filter_gather, apply_sc_amp, apply_sc_decon,
    apply_srme_2d, shift_trace_ms,
};
use rustseis_engine::{EngineCommand, EngineSession};
use rustseis_flow::{FlowSpec, FlowStep};
use rustseis_imaging::{
    Afd2dParams, AutostatMethod, AutostatParams, DmoFkParams, FdMig2dParams, FirstBreakMethod,
    FirstBreakPickParams, KirchhoffPstm2dParams, NmoDirection, NmoParams, RefractionStaticsParams,
    StackParams, Synth2dParams, SynthDiffractor, SynthReflector, TrimStatParams,
    VspCorridorStackParams, VspDeconParams, VspGeometryParams, VspSepMethod, VspSlownessParams,
    VspWavefieldOutput, VspWavefieldSepParams, apply_nmo, apply_trim_statics, apply_vsp_decon,
    apply_vsp_wavefield_sep, assign_crooked_line_geometry_2d, assign_vsp_geometry,
    compute_vsp_corridor_stack, dmo_fk_2d, estimate_vsp_slowness, fd_migrate_2d,
    generate_synthetic_2d, inline_sort, kirchhoff_pstm_2d, pick_first_breaks, ricker_wavelet,
    run_afd_2d, separate_vsp_wavefields, solve_and_apply_autostat,
    solve_and_apply_refraction_statics, stack_ensembles,
};
use rustseis_io::OpfDatabase;
use rustseis_mcp::{handle_tool_call, mcp_tool_definitions};
use rustseis_render::{
    GatherRenderConfig, render_gather_with_picks, render_radon_panel, render_side_by_side,
    render_xy_curve_plot,
};
use serde_json::json;
use std::path::Path;

fn snr_db(clean: &[Trace], test: &[Trace]) -> f64 {
    let mut sig = 0.0f64;
    let mut err = 0.0f64;
    for (c, t) in clean.iter().zip(test.iter()) {
        for (&cs, &ts) in c.samples.iter().zip(t.samples.iter()) {
            sig += (cs as f64) * (cs as f64);
            let d = ts as f64 - cs as f64;
            err += d * d;
        }
    }
    10.0 * (sig / err.max(1e-24)).log10()
}

fn rms_window(traces: &[Trace], i0: usize, i1: usize) -> f64 {
    let mut sum = 0.0f64;
    let mut cnt = 0usize;
    for tr in traces {
        for &s in &tr.samples[i0..i1.min(tr.samples.len())] {
            sum += (s as f64) * (s as f64);
            cnt += 1;
        }
    }
    (sum / cnt.max(1) as f64).sqrt()
}

fn mean_corr(a: &[Trace], b: &[Trace]) -> f64 {
    let mut sum = 0.0f64;
    for (ta, tb) in a.iter().zip(b.iter()) {
        sum += zero_lag_correlation(&ta.samples, &tb.samples);
    }
    sum / a.len().max(1) as f64
}

fn pseudo_normal(seed: &mut u64) -> f64 {
    let next = |s: &mut u64| -> f64 {
        let mut x = *s;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *s = x;
        (x >> 11) as f64 / ((1u64 << 53) as f64)
    };
    let u1 = next(seed).max(1e-12);
    let u2 = next(seed);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

#[test]
fn test_wave1_fxdecon_radon_groundroll_airblast_curvelet_srme() {
    // 1. F-X Decon (Wiener-Levinson & L2 Adaptive) on 2 dipping events + heavy Gaussian noise
    let nx = 40usize;
    let nt = 301usize;
    let dt_us = 2000u32;
    let dt_s = 0.002f64;
    let mut clean_dip = Vec::with_capacity(nx);
    let mut noisy_dip = Vec::with_capacity(nx);
    let mut seed = 123456789u64;

    for ix in 0..nx {
        let mut s_clean = vec![0.0f32; nt];
        let mut s_noisy = vec![0.0f32; nt];
        let t1 = 0.18 + ix as f64 * 0.0025; // dipping event 1
        let t2 = 0.44 - ix as f64 * 0.0018; // opposing dipping event 2
        for it in 0..nt {
            let t = it as f64 * dt_s;
            let sig = ricker_wavelet(t - t1, 30.0) + 0.85 * ricker_wavelet(t - t2, 25.0);
            s_clean[it] = sig as f32;
            s_noisy[it] = (sig + 0.28 * pseudo_normal(&mut seed)) as f32;
        }
        let mut h = TraceHeaders::default();
        h.traceno = (ix + 1) as i32;
        h.cdp = (ix + 1) as i32;
        h.trc_type = 1;
        let mut tc = Trace::new(dt_us, s_clean);
        tc.headers = h.clone();
        let mut tn = Trace::new(dt_us, s_noisy);
        tn.headers = h;
        clean_dip.push(tc);
        noisy_dip.push(tn);
    }

    let snr_in = snr_db(&clean_dip, &noisy_dip);
    let mut fx_wl = noisy_dip.clone();
    apply_fx_decon(
        &mut fx_wl,
        &FxDeconParams {
            method: FxDeconMethod::WienerLevinson,
            filter_len: 5,
            horiz_window_traces: 40,
            time_window_ms: 600.0,
            time_overlap_ms: 60.0,
            white_noise_pct: 1.0,
            f_min_hz: 5.0,
            f_max_hz: 85.0,
            reapply_mute: false,
            ..Default::default()
        },
    )
    .unwrap();
    let snr_wl = snr_db(&clean_dip, &fx_wl);
    let corr_wl = mean_corr(&clean_dip, &fx_wl);
    println!("[WAVE1 FX-Decon WL]   SNR: {snr_in:.2} dB -> {snr_wl:.2} dB (+{:.2} dB), corr={corr_wl:.4}", snr_wl - snr_in);
    assert!(snr_wl - snr_in > 5.0, "FX-Decon WL should boost SNR by > 5 dB");
    assert!(corr_wl > 0.90, "FX-Decon WL correlation too low: {corr_wl}");

    let mut fx_lms = noisy_dip.clone();
    apply_fx_decon(
        &mut fx_lms,
        &FxDeconParams {
            method: FxDeconMethod::L2Adaptive,
            adaptation_rate: 0.12,
            filter_len: 5,
            horiz_window_traces: 40,
            time_window_ms: 600.0,
            reapply_mute: false,
            ..Default::default()
        },
    )
    .unwrap();
    let snr_lms = snr_db(&clean_dip, &fx_lms);
    println!("[WAVE1 FX-Decon LMS]  SNR: {snr_in:.2} dB -> {snr_lms:.2} dB (+{:.2} dB)", snr_lms - snr_in);
    assert!(snr_lms > snr_in + 2.0, "FX-Decon LMS should improve SNR");

    // 2. Curvelet Multiscale Directional Denoising on the same noisy dipping panel
    let mut curv = noisy_dip.clone();
    apply_curvelet_denoise(
        &mut curv,
        &CurveletDenoiseParams {
            n_scales: 4,
            base_angles: 8,
            threshold_sigma: 2.2,
            preserve_coarse: true,
        },
    )
    .unwrap();
    let snr_curv = snr_db(&clean_dip, &curv);
    let corr_curv = mean_corr(&clean_dip, &curv);
    println!("[WAVE1 Curvelet]      SNR: {snr_in:.2} dB -> {snr_curv:.2} dB (+{:.2} dB), corr={corr_curv:.4}", snr_curv - snr_in);
    assert!(snr_curv - snr_in > 5.5, "Curvelet should boost SNR by > 5.5 dB");
    assert!(corr_curv > 0.91, "Curvelet correlation too low: {corr_curv}");

    // 3. Parabolic Radon Demultiple: flat primary (q=0 at 300ms) + parabolic multiple (q=+180ms at 500ms)
    let nx_r = 36usize;
    let nt_r = 451usize;
    let ref_off = 1800.0f64;
    let mut prim_only = Vec::with_capacity(nx_r);
    let mut prim_plus_mult = Vec::with_capacity(nx_r);
    for ix in 0..nx_r {
        let off = 100.0 + ix as f64 * ((ref_off - 100.0) / (nx_r - 1) as f64);
        let u = off / ref_off;
        let t_mult = 0.50 + 0.180 * u * u;
        let mut sp = vec![0.0f32; nt_r];
        let mut sm = vec![0.0f32; nt_r];
        for it in 0..nt_r {
            let t = it as f64 * dt_s;
            let p = ricker_wavelet(t - 0.30, 28.0);
            let m = 0.9 * ricker_wavelet(t - t_mult, 25.0);
            sp[it] = p as f32;
            sm[it] = (p + m) as f32;
        }
        let mut h = TraceHeaders::default();
        h.cdp = 100;
        h.offset = off as f32;
        h.aoffset = off as f32;
        h.trc_type = 1;
        let mut tp = Trace::new(dt_us, sp);
        tp.headers = h.clone();
        let mut tm = Trace::new(dt_us, sm);
        tm.headers = h;
        prim_only.push(tp);
        prim_plus_mult.push(tm);
    }

    let mult_rms_before = rms_window(&prim_plus_mult, 220, 380);
    let mut radon_out = prim_plus_mult.clone();
    let rpanel = apply_radon_filter_gather(
        &mut radon_out,
        &RadonParams {
            mode: RadonMode::Parabolic,
            q_min_ms: -80.0,
            q_max_ms: 300.0,
            n_q: 55,
            ref_offset_m: ref_off as f32,
            f_min_hz: 5.0,
            f_max_hz: 80.0,
            damping: 0.01,
            sparse_iters: 2,
            action: RadonFilterAction::SubtractZone,
            q_zone_min_ms: 60.0,
            q_zone_max_ms: 300.0,
            q_taper_ms: 25.0,
            time_variant_zone: Vec::new(),
        },
    )
    .unwrap();
    let mult_rms_after = rms_window(&radon_out, 220, 380);
    let mult_atten_pct = (1.0 - mult_rms_after / mult_rms_before) * 100.0;
    let prim_corr = mean_corr(&prim_only, &radon_out);
    println!("[WAVE1 Parabolic Radon] Multiple attenuation={mult_atten_pct:.1}%, Primary corr={prim_corr:.4}");
    assert!(mult_atten_pct > 82.0, "Radon should attenuate multiple > 82%, got {mult_atten_pct:.1}%");
    assert!(prim_corr > 0.96, "Radon primary preservation corr should be > 0.96, got {prim_corr:.4}");
    let r_img = render_radon_panel(&rpanel.times_ms, &rpanel.q_values_ms, &rpanel.data, Some((60.0, 300.0)), 640, 480).unwrap();
    assert_eq!(r_img.rgba.len(), 640 * 480 * 4);

    // 4. Ground Roll (LFAF) + Air Blast
    let nx_g = 48usize;
    let nt_g = 501usize;
    let mut refl_clean = Vec::with_capacity(nx_g);
    let mut with_gr_ab = Vec::with_capacity(nx_g);
    for ix in 0..nx_g {
        let off = 25.0 + ix as f64 * 12.5;
        let t_gr = off / 450.0;
        let t_ab = off / 330.0;
        let mut sc = vec![0.0f32; nt_g];
        let mut sn = vec![0.0f32; nt_g];
        for it in 0..nt_g {
            let t = it as f64 * dt_s;
            let refl = ricker_wavelet(t - 0.35, 40.0);
            let gr = 2.5 * ricker_wavelet(t - t_gr, 10.0);
            let ab = if (t - t_ab).abs() < 0.03 { 8.0 * ricker_wavelet(t - t_ab, 55.0) } else { 0.0 };
            sc[it] = refl as f32;
            sn[it] = (refl + gr + ab) as f32;
        }
        let mut h = TraceHeaders::default();
        h.ffid = 1;
        h.chan = (ix + 1) as i32;
        h.offset = off as f32;
        h.aoffset = off as f32;
        h.trc_type = 1;
        let mut tc = Trace::new(dt_us, sc); tc.headers = h.clone();
        let mut tn = Trace::new(dt_us, sn); tn.headers = h;
        refl_clean.push(tc);
        with_gr_ab.push(tn);
    }
    let snr_gr_in = snr_db(&refl_clean, &with_gr_ab);
    apply_airblast_attenuation(&mut with_gr_ab, &AirBlastParams::default());
    apply_ground_roll_attenuation(
        &mut with_gr_ab,
        &GroundRollParams {
            f_max_hz: 18.0,
            f_taper_hz: 6.0,
            max_velocity_mps: 850.0,
            dx_m: 12.5,
            cone_lead_ms: 50.0,
            strength: 1.0,
        },
    )
    .unwrap();
    let snr_gr_out = snr_db(&refl_clean, &with_gr_ab);
    println!("[WAVE1 AirBlast+LFAF] SNR: {snr_gr_in:.2} dB -> {snr_gr_out:.2} dB (+{:.2} dB)", snr_gr_out - snr_gr_in);
    assert!(snr_gr_out - snr_gr_in > 9.0, "AirBlast + LFAF should boost SNR by > 9 dB");

    // 5. 2D SRME + Adaptive Subtraction on synthetic marine survey with seafloor primary (300ms) + 1st multiple (600ms)
    let srme_shots = generate_synthetic_2d(&Synth2dParams {
        n_shots: 24,
        n_chans: 16,
        shot_interval_m: 25.0,
        receiver_interval_m: 25.0,
        near_offset_m: 0.0,
        split_spread: false,
        dt_us: 2000,
        n_samples: 451,
        dominant_freq_hz: 28.0,
        reflectors: vec![SynthReflector {
            t0_ms: 300.0,
            dip_ms_per_km: 0.0,
            velocity_mps: 1500.0,
            amplitude: 1.0,
            surface_multiple_amp: -0.55,
        }],
        ..Default::default()
    })
    .unwrap();
    let srme_prim_only = generate_synthetic_2d(&Synth2dParams {
        n_shots: 24,
        n_chans: 16,
        shot_interval_m: 25.0,
        receiver_interval_m: 25.0,
        near_offset_m: 0.0,
        split_spread: false,
        dt_us: 2000,
        n_samples: 451,
        dominant_freq_hz: 28.0,
        reflectors: vec![SynthReflector {
            t0_ms: 300.0,
            dip_ms_per_km: 0.0,
            velocity_mps: 1500.0,
            amplitude: 1.0,
            surface_multiple_amp: 0.0,
        }],
        ..Default::default()
    })
    .unwrap();

    // Compare interior shots (full aperture) before & after 2D SRME
    let interior_before: Vec<Trace> = srme_shots.iter().filter(|t| t.headers.ffid >= 8 && t.headers.ffid <= 16).cloned().collect();
    let interior_prim: Vec<Trace> = srme_prim_only.iter().filter(|t| t.headers.ffid >= 8 && t.headers.ffid <= 16).cloned().collect();
    let mult_before_srme = rms_window(&interior_before, 260, 380);

    let mut srme_work = srme_shots.clone();
    let _model = apply_srme_2d(
        &mut srme_work,
        &Srme2dParams {
            dx_m: 25.0,
            f_min_hz: 5.0,
            f_max_hz: 75.0,
            use_reciprocity: true,
            subtract_multiples: true,
            ..Default::default()
        },
    )
    .unwrap();
    let interior_after: Vec<Trace> = srme_work.iter().filter(|t| t.headers.ffid >= 8 && t.headers.ffid <= 16).cloned().collect();
    let mult_after_srme = rms_window(&interior_after, 260, 380);
    let srme_atten = (1.0 - mult_after_srme / mult_before_srme) * 100.0;
    let srme_prim_corr = mean_corr(&interior_prim, &interior_after);
    println!("[WAVE1 2D SRME+ASUB]  Multiple attenuation={srme_atten:.1}%, Primary corr={srme_prim_corr:.4}");
    assert!(srme_atten > 70.0, "2D SRME should attenuate surface multiple > 70%, got {srme_atten:.1}%");
    assert!(srme_prim_corr > 0.90, "2D SRME primary correlation should be > 0.90, got {srme_prim_corr:.4}");

    // 6. Live ProMAX 5000.0.2.0 VM differential parity check for FXDECON (if VM reference present)
    let p_in = Path::new(r"C:\Kuliah\Metode Seismik 2025\rustseis\out\promax_parity\input_nias_shot1000.sgy");
    let p_ref = Path::new(r"C:\Kuliah\Metode Seismik 2025\rustseis\out\promax_parity\promax_fxdecon.sgy");
    if p_in.exists() && p_ref.exists() {
        let (_, mut fx_tr) = rustseis_io::read_segy_file(p_in, None).unwrap();
        let (_, ref_tr) = rustseis_io::read_segy_file(p_ref, None).unwrap();
        apply_fx_decon(
            &mut fx_tr,
            &FxDeconParams {
                method: FxDeconMethod::WienerLevinson,
                white_noise_pct: 1.0,
                horiz_window_traces: 24,
                filter_len: 5,
                time_window_ms: 1000.0,
                time_overlap_ms: 100.0,
                f_min_hz: 6.0,
                f_max_hz: 90.0,
                reapply_mute: false,
                ..Default::default()
            },
        )
        .unwrap();
        let mut nrms_sum = 0.0;
        let mut corr_sum = 0.0;
        for (a, b) in ref_tr.iter().zip(fx_tr.iter()) {
            nrms_sum += rustseis_core::nrms_percent(&a.samples, &b.samples);
            corr_sum += zero_lag_correlation(&a.samples, &b.samples);
        }
        let m_nrms = nrms_sum / ref_tr.len() as f64;
        let m_corr = corr_sum / ref_tr.len() as f64;
        println!("[PARITY ProMAX FXDECON] 96 traces: mean_NRMS={m_nrms:.2}%, mean_corr={m_corr:.6}");
        assert!(m_nrms < 15.0 && m_corr > 0.985, "ProMAX FXDECON parity drift: NRMS={m_nrms}, corr={m_corr}");
    }
}

#[test]
fn test_wave2_surface_consistent_and_land_statics_ground_truth() {
    // 1. Surface-Consistent Amplitudes (scamp2): inject exact known source & receiver gain anomalies
    let base = generate_synthetic_2d(&Synth2dParams {
        n_shots: 18,
        n_chans: 16,
        shot_interval_m: 25.0,
        receiver_interval_m: 25.0,
        near_offset_m: 50.0,
        split_spread: true,
        dt_us: 2000,
        n_samples: 501,
        dominant_freq_hz: 30.0,
        reflectors: vec![
            SynthReflector { t0_ms: 400.0, dip_ms_per_km: 0.0, velocity_mps: 2000.0, amplitude: 1.0, surface_multiple_amp: 0.0 },
            SynthReflector { t0_ms: 700.0, dip_ms_per_km: 0.0, velocity_mps: 2400.0, amplitude: 0.8, surface_multiple_amp: 0.0 },
        ],
        ..Default::default()
    })
    .unwrap();

    let mut distorted = base.clone();
    let mut true_src_g = std::collections::BTreeMap::<i32, f32>::new();
    let mut true_rec_g = std::collections::BTreeMap::<i32, f32>::new();
    for tr in &mut distorted {
        let sg = 0.5 + 1.5 * (((tr.headers.sou_sloc * 37) % 10).abs() as f32 / 9.0);
        let rg = 0.6 + 1.2 * (((tr.headers.rec_sloc * 53) % 10).abs() as f32 / 9.0);
        true_src_g.insert(tr.headers.sou_sloc, sg);
        true_rec_g.insert(tr.headers.rec_sloc, rg);
        for s in &mut tr.samples {
            *s *= sg * rg;
        }
    }

    let mut sc_rms = distorted.clone();
    let sc_tbl = apply_sc_amp(
        &mut sc_rms,
        &ScAmpParams {
            gate_start_ms: 200.0,
            gate_end_ms: 950.0,
            measure: ScAmpMeasure::Rms,
            solve_source: true,
            solve_receiver: true,
            solve_offset: true,
            apply_source: true,
            apply_receiver: true,
            apply_offset: false,
            iterations: 35,
            damping: 0.001,
            preserve_mean_amp: true,
            ..Default::default()
        },
        "sc_test",
    )
    .unwrap();

    // Check relative accuracy of recovered source gain ratios
    let mut max_rel_err = 0.0f32;
    let first_s = *true_src_g.keys().next().unwrap();
    let ref_true = true_src_g[&first_s];
    let ref_est = sc_tbl.source_factors[&first_s];
    for (&s, &tg) in &true_src_g {
        let eg = sc_tbl.source_factors[&s];
        let rel = ((eg / ref_est) - (tg / ref_true)).abs() / (tg / ref_true);
        max_rel_err = max_rel_err.max(rel);
    }
    println!("[WAVE2 SCAMP2 Rms]    max source gain ratio error={:.2}%, log_res={:.5}", max_rel_err * 100.0, sc_tbl.rms_log_residual);
    assert!(max_rel_err < 0.06, "SCAMP2 source gain ratio error too high: {max_rel_err}");

    // Also verify ProMAX 21-subgate Hilbert EnvelopeMedian mode runs cleanly
    let mut sc_env = distorted.clone();
    let sc_env_tbl = apply_sc_amp(
        &mut sc_env,
        &ScAmpParams { measure: ScAmpMeasure::EnvelopeMedian, ..Default::default() },
        "sc_env",
    )
    .unwrap();
    assert!(sc_env_tbl.source_factors.len() == true_src_g.len());

    // 2. Surface-Consistent Decon (scdecon)
    let mut scd_tr = base.clone();
    apply_sc_decon(
        &mut scd_tr,
        &ScDeconParams {
            gate_start_ms: 200.0,
            gate_end_ms: 950.0,
            operator_ms: 80.0,
            prediction_lag_ms: 0.0,
            white_noise_pct: 1.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(scd_tr.iter().all(|t| t.samples.iter().all(|s| s.is_finite())));

    // 3. First-Break Picking (AIC & Coppens) + Delay-Time Refraction Statics (refstatic)
    let mut land_refr = generate_synthetic_2d(&Synth2dParams {
        n_shots: 16,
        n_chans: 20,
        shot_interval_m: 25.0,
        receiver_interval_m: 25.0,
        near_offset_m: 125.0,
        split_spread: false,
        dt_us: 2000,
        n_samples: 501,
        dominant_freq_hz: 30.0,
        reflectors: vec![SynthReflector { t0_ms: 600.0, dip_ms_per_km: 0.0, velocity_mps: 2500.0, amplitude: 0.5, surface_multiple_amp: 0.0 }],
        refractor_vel_mps: 2800.0,
        weathering_vel_mps: 600.0,
        max_static_ms: 14.0,
        noise_rms: 0.02,
        seed: 777,
        ..Default::default()
    })
    .unwrap();

    let fb_tbl = pick_first_breaks(
        &mut land_refr,
        &FirstBreakPickParams {
            method: FirstBreakMethod::Aic,
            v_min_mps: 1000.0,
            v_max_mps: 5000.0,
            early_pad_ms: 60.0,
            late_pad_ms: 100.0,
            min_snr: 1.5,
            ..Default::default()
        },
        "fb_aic",
    );
    assert!(fb_tbl.picks.len() >= (land_refr.len() * 8) / 10, "Too few FB picks: {}", fb_tbl.picks.len());

    let ref_tbl = solve_and_apply_refraction_statics(
        &mut land_refr,
        &RefractionStaticsParams {
            min_offset_m: 100.0,
            max_offset_m: 1500.0,
            weathering_velocity_mps: 600.0,
            replacement_velocity_mps: 2000.0,
            final_datum_m: 0.0,
            iterations: 45,
            damping: 0.01,
            apply_shift: true,
        },
        "ref_test",
    )
    .unwrap();
    let v1_err_pct = ((ref_tbl.refractor_velocity_mps - 2800.0).abs() / 2800.0) * 100.0;
    println!("[WAVE2 FB+RefStatic]  picks={}, V_refr={:.1} m/s (true 2800, err={v1_err_pct:.2}%), rms_misfit={:.2} ms",
        fb_tbl.picks.len(), ref_tbl.refractor_velocity_mps, ref_tbl.rms_residual_ms);
    assert!(v1_err_pct < 6.0, "Refraction velocity error too high: {v1_err_pct:.2}%");
    assert!(ref_tbl.rms_residual_ms < 5.0, "Refraction traveltime misfit too high: {} ms", ref_tbl.rms_residual_ms);

    // 4. Residual Autostatics (Correlation & Max-Power) + CDP Trim Statics
    // Start from clean NMO-corrected CDP gathers, inject known random shot+receiver statics up to +/-12 ms!
    let mut clean_nmo = generate_synthetic_2d(&Synth2dParams {
        n_shots: 24,
        n_chans: 16,
        shot_interval_m: 25.0,
        receiver_interval_m: 25.0,
        near_offset_m: 50.0,
        split_spread: true,
        dt_us: 2000,
        n_samples: 501,
        dominant_freq_hz: 28.0,
        reflectors: vec![
            SynthReflector { t0_ms: 380.0, dip_ms_per_km: 0.0, velocity_mps: 2000.0, amplitude: 1.0, surface_multiple_amp: 0.0 },
            SynthReflector { t0_ms: 660.0, dip_ms_per_km: 0.0, velocity_mps: 2000.0, amplitude: 0.9, surface_multiple_amp: 0.0 },
        ],
        ..Default::default()
    })
    .unwrap();
    apply_nmo(&mut clean_nmo, &VelocityTable::constant(2000.0), &NmoParams { direction: NmoDirection::Forward, ..Default::default() });
    let clean_sorted = inline_sort(clean_nmo, "CDP", Some("AOFFSET"), false, false);
    let clean_stack = stack_ensembles(&Ensemble::group_by_header(clean_sorted.clone(), "CDP"), &StackParams::default());

    let mut jittered = clean_sorted.clone();
    let mut stat_seed = 987654321u64;
    let mut s_true = std::collections::BTreeMap::<i32, f32>::new();
    let mut r_true = std::collections::BTreeMap::<i32, f32>::new();
    for tr in &mut jittered {
        let ss = *s_true.entry(tr.headers.sou_sloc).or_insert_with(|| (pseudo_normal(&mut stat_seed).clamp(-2.0, 2.0) * 5.0) as f32);
        let rs = *r_true.entry(tr.headers.rec_sloc).or_insert_with(|| (pseudo_normal(&mut stat_seed).clamp(-2.0, 2.0) * 5.0) as f32);
        shift_trace_ms(tr, ss + rs);
    }
    let broken_stack = stack_ensembles(&Ensemble::group_by_header(jittered.clone(), "CDP"), &StackParams::default());
    let rms_broken = rms_window(&broken_stack, 150, 380);
    let rms_clean = rms_window(&clean_stack, 150, 380);

    // 4a. Correlation Autostatics (`autostat`)
    let mut corr_work = jittered.clone();
    let tbl_corr = solve_and_apply_autostat(
        &mut corr_work,
        &AutostatParams {
            method: AutostatMethod::Correlation,
            gate_start_ms: 250.0,
            gate_end_ms: 850.0,
            max_shift_ms: 24.0,
            outer_iterations: 6,
            inner_iterations: 25,
            pilot_mix_half_width: 2,
            ..Default::default()
        },
        "autostat_corr",
    )
    .unwrap();
    let corr_stack = stack_ensembles(&Ensemble::group_by_header(corr_work.clone(), "CDP"), &StackParams::default());
    let rms_corr_fixed = rms_window(&corr_stack, 150, 380);

    // 4b. Max-Power Stack Autostatics (`pwr_stat_new`)
    let mut pwr_work = jittered.clone();
    let tbl_pwr = solve_and_apply_autostat(
        &mut pwr_work,
        &AutostatParams {
            method: AutostatMethod::MaxPower,
            gate_start_ms: 250.0,
            gate_end_ms: 850.0,
            max_shift_ms: 24.0,
            outer_iterations: 6,
            pilot_mix_half_width: 2,
            ..Default::default()
        },
        "autostat_pwr",
    )
    .unwrap();
    let pwr_stack = stack_ensembles(&Ensemble::group_by_header(pwr_work, "CDP"), &StackParams::default());
    let rms_pwr_fixed = rms_window(&pwr_stack, 150, 380);

    // 4c. CDP Trim Statics (`trimstat`)
    apply_trim_statics(&mut corr_work, &TrimStatParams::default()).unwrap();
    let trim_stack = stack_ensembles(&Ensemble::group_by_header(corr_work, "CDP"), &StackParams::default());
    let rms_trim_fixed = rms_window(&trim_stack, 150, 380);

    println!(
        "[WAVE2 Autostat]      Stack RMS: clean={rms_clean:.4}, jittered={rms_broken:.4}, corr_stat={rms_corr_fixed:.4} (res={:.2}ms), max_pwr={rms_pwr_fixed:.4} (res={:.2}ms), trim={rms_trim_fixed:.4}",
        tbl_corr.rms_residual_ms, tbl_pwr.rms_residual_ms
    );
    assert!(rms_corr_fixed > rms_broken * 1.25, "Correlation autostat should significantly restore stack power");
    assert!(rms_pwr_fixed > rms_broken * 1.25, "Max-Power autostat should significantly restore stack power");
    assert!(rms_trim_fixed >= rms_corr_fixed * 0.99, "Trim statics should preserve or boost stack coherence");
}

#[test]
fn test_wave3_pstm_dmo_fdmig_crooked_and_afd() {
    // 1. 2D Kirchhoff PSTM on a synthetic point diffractor at (x = 400 m, t0 = 500 ms, V = 2000 m/s)
    let pstm_in = generate_synthetic_2d(&Synth2dParams {
        n_shots: 32,
        n_chans: 16,
        shot_interval_m: 25.0,
        receiver_interval_m: 25.0,
        near_offset_m: 50.0,
        split_spread: false,
        dt_us: 2000,
        n_samples: 501,
        dominant_freq_hz: 28.0,
        reflectors: vec![],
        diffractors: vec![SynthDiffractor {
            x_m: 400.0,
            t0_ms: 500.0,
            velocity_mps: 2000.0,
            amplitude: 1.0,
        }],
        ..Default::default()
    })
    .unwrap();

    let vt = VelocityTable::constant(2000.0);
    let pstm_img = kirchhoff_pstm_2d(
        &pstm_in,
        &vt,
        &KirchhoffPstm2dParams {
            dx_m: 12.5,
            cdp_step: 1,
            max_aperture_m: 600.0,
            max_dip_deg: 60.0,
            output_cig: false,
            ..Default::default()
        },
    )
    .unwrap();

    // Locate global peak in PSTM image -> MUST collapse directly onto x ≈ 400 m, t ≈ 500 ms!
    let mut best_amp = 0.0f32;
    let mut best_x = 0.0f64;
    let mut best_t = 0.0f32;
    for tr in &pstm_img {
        for (it, &s) in tr.samples.iter().enumerate() {
            if s.abs() > best_amp {
                best_amp = s.abs();
                best_x = tr.headers.cdp_x;
                best_t = it as f32 * 2.0;
            }
        }
    }
    println!("[WAVE3 2D PSTM]       Peak collapsed to x={best_x:.1} m (true 400 m), t={best_t:.1} ms (true 500 ms)");
    assert!((best_x - 400.0).abs() <= 25.0, "PSTM diffractor X off: {best_x}");
    assert!((best_t - 500.0).abs() <= 16.0, "PSTM diffractor T off: {best_t}");

    // Also test PSTM CIG output mode (`output_cig: true`)
    let cigs = kirchhoff_pstm_2d(
        &pstm_in,
        &vt,
        &KirchhoffPstm2dParams {
            dx_m: 12.5,
            cdp_step: 4,
            max_aperture_m: 500.0,
            output_cig: true,
            offset_bin_m: 100.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(cigs.len() > pstm_img.len() / 4, "CIG output should contain multiple offset bins per CDP");

    // 2. 2D 45-deg Omega-X Finite-Difference Post-Stack Migration (`fdmig`) on zero-offset diffractor
    let nx_z = 64usize;
    let nt_z = 401usize;
    let dx_z = 12.5f64;
    let dt_z = 0.002f64;
    let x0_d = 32.0 * dx_z;
    let t0_d = 0.40;
    let v_d = 2000.0;
    let mut zo_sec = Vec::with_capacity(nx_z);
    for ix in 0..nx_z {
        let x = ix as f64 * dx_z;
        let t_hyp = (t0_d * t0_d + (2.0 * (x - x0_d) / v_d).powi(2)).sqrt();
        let mut samps = vec![0.0f32; nt_z];
        for it in 0..nt_z {
            samps[it] = ricker_wavelet(it as f64 * dt_z - t_hyp, 25.0) as f32;
        }
        let mut h = TraceHeaders::default();
        h.cdp = (ix + 1) as i32;
        h.cdp_x = x;
        h.trc_type = 1;
        let mut tr = Trace::new(2000, samps);
        tr.headers = h;
        zo_sec.push(tr);
    }
    let fd_out = fd_migrate_2d(
        &zo_sec,
        &VelocityTable::constant(2000.0),
        &FdMig2dParams { dx_m: dx_z as f32, f_max_hz: 70.0, pad_traces: 16, dtau_samples: 1 },
    )
    .unwrap();
    let mut fd_best_ix = 0usize;
    let mut fd_best_it = 0usize;
    let mut fd_best_a = 0.0f32;
    for (ix, tr) in fd_out.iter().enumerate() {
        for (it, &s) in tr.samples.iter().enumerate() {
            if s.abs() > fd_best_a {
                fd_best_a = s.abs();
                fd_best_ix = ix;
                fd_best_it = it;
            }
        }
    }
    println!("[WAVE3 45-deg FDMIG]  Peak collapsed to ix={fd_best_ix} (true 32), t={:.1} ms (true 400 ms)", fd_best_it as f32 * 2.0);
    assert!((fd_best_ix as isize - 32).abs() <= 2, "FDMIG X peak off: {fd_best_ix}");
    assert!(((fd_best_it as f32 * 2.0) - 400.0).abs() <= 14.0, "FDMIG T peak off");

    // 3. Hale 2D F-K DMO (`dmofkpanel`) on NMO-corrected common-offset gather
    let mut dmo_in = generate_synthetic_2d(&Synth2dParams {
        n_shots: 24,
        n_chans: 4,
        shot_interval_m: 25.0,
        receiver_interval_m: 100.0,
        near_offset_m: 200.0,
        dt_us: 2000,
        n_samples: 401,
        dominant_freq_hz: 25.0,
        reflectors: vec![SynthReflector { t0_ms: 400.0, dip_ms_per_km: 150.0, velocity_mps: 2000.0, amplitude: 1.0, surface_multiple_amp: 0.0 }],
        ..Default::default()
    })
    .unwrap();
    apply_nmo(&mut dmo_in, &VelocityTable::constant(2000.0), &NmoParams::default());
    let dmo_out = dmo_fk_2d(&dmo_in, &DmoFkParams { dx_m: 25.0, offset_bin_m: 50.0, f_max_hz: 70.0, pad_traces: 12 }).unwrap();
    assert_eq!(dmo_out.len(), dmo_in.len());
    assert!(dmo_out.iter().any(|t| t.headers.dmooff > 100.0 && t.samples.iter().any(|&s| s.abs() > 0.1)));

    // 4. 2D Acoustic Finite-Difference Wave-Equation Modeler (`afd`): verify direct & reflected arrival kinematics!
    let afd_traces = run_afd_2d(&Afd2dParams {
        nx: 120,
        nz: 90,
        dh_m: 10.0,
        layers: vec![(0.0, 1500.0), (260.0, 2600.0)],
        shot_xs_m: vec![600.0],
        receiver_start_m: 300.0,
        receiver_step_m: 20.0,
        n_receivers: 31,
        dt_us: 2000,
        max_time_ms: 550.0,
        dominant_freq_hz: 22.0,
        free_surface: false,
        sponge_cells: 16,
    })
    .unwrap();
    assert_eq!(afd_traces.len(), 31);
    // Zero-offset trace at rx = 600 m (ir = 15): reflected wave arrives at t_delay + 2*(260-20)/1500
    let zo_tr = &afd_traces[15];
    let t_delay_ms: f32 = (1.2 / 22.0) * 1000.0;
    let t_refl_expected_ms: f32 = t_delay_ms + (2.0 * 240.0 / 1500.0) * 1000.0; // ~374.5 ms
    let i_lo = ((t_refl_expected_ms - 45.0) / 2.0) as usize;
    let i_hi = ((t_refl_expected_ms + 45.0) / 2.0).min(zo_tr.samples.len() as f32 - 1.0) as usize;
    let (best_i, best_v) = zo_tr.samples[i_lo..=i_hi]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap();
    let t_refl_actual_ms = (i_lo + best_i) as f32 * 2.0;
    println!("[WAVE3 2D AFD]        Reflected peak at {t_refl_actual_ms:.1} ms (expected {t_refl_expected_ms:.1} ms, amp={:.4})", best_v.abs());
    assert!(best_v.abs() > 1e-3, "AFD should produce clear reflection energy");
    assert!((t_refl_actual_ms - t_refl_expected_ms).abs() < 20.0, "AFD reflection arrival time mismatch");

    // 5. Crooked-Line 2D Geometry (`geomspread_crooked`)
    let mut stations = Vec::new();
    for i in 0..20 {
        let x = i as f64 * 50.0;
        let y = (i as f64 * 0.25).sin() * 120.0; // S-curved survey line
        stations.push((i + 1, x, y, 100.0f32));
    }
    let mut cl_traces = Vec::new();
    for s in 1..=10 {
        for r in (s + 1)..=(s + 8).min(20) {
            let mut tr = Trace::new(2000, vec![1.0; 16]);
            tr.headers.sou_sloc = s;
            tr.headers.rec_sloc = r;
            tr.headers.trc_type = 1;
            cl_traces.push(tr);
        }
    }
    // Add one far crossline outlier (500 m off-line -> must be killed by 200 m corridor)
    let mut out_tr = Trace::new(2000, vec![1.0; 16]);
    out_tr.headers.sou_sloc = 999;
    out_tr.headers.rec_sloc = 999;
    out_tr.headers.sou_x = 300.0;
    out_tr.headers.sou_y = 650.0;
    out_tr.headers.rec_x = 350.0;
    out_tr.headers.rec_y = 650.0;
    out_tr.headers.trc_type = 1;
    cl_traces.push(out_tr);

    assign_crooked_line_geometry_2d(
        &mut cl_traces,
        &CrookedLineGeometry2D {
            stations,
            slalom_vertices: vec![],
            cdp_bin_size_m: 25.0,
            crossline_half_width_m: 200.0,
            auto_smooth_passes: 4,
        },
    );
    assert!(cl_traces[0].headers.cdp >= 1 && cl_traces[0].headers.cdp_nfld >= 1);
    assert!(!cl_traces.last().unwrap().is_live(), "Off-corridor trace should be flagged dead by crooked-line binning");
}

#[test]
fn test_wave4_vsp_and_real_otway_das() {
    // 1. Synthetic VSP/DAS validation: 40 depths (100..490 m, dz=10 m), V=2200 m/s, reflector at z=600 m
    let n_dep = 40usize;
    let nt = 501usize;
    let dt_us = 2000u32;
    let dt_s = 0.002f64;
    let v_true = 2200.0f64;
    let z_refl = 600.0f64;

    let mut vsp_syn = Vec::with_capacity(n_dep);
    let mut up_true = Vec::with_capacity(n_dep);
    for iz in 0..n_dep {
        let z = 100.0 + iz as f64 * 10.0;
        let t_down = z / v_true;
        let t_up = (2.0 * z_refl - z) / v_true; // upgoing reflection from z=600 m
        let mut s_tot = vec![0.0f32; nt];
        let mut s_up = vec![0.0f32; nt];
        for it in 0..nt {
            let t = it as f64 * dt_s;
            let d = 1.0 * ricker_wavelet(t - t_down, 30.0);
            let u = 0.35 * ricker_wavelet(t - t_up, 30.0);
            s_tot[it] = (d + u) as f32;
            s_up[it] = u as f32;
        }
        let mut h = TraceHeaders::default();
        h.chan = (iz + 1) as i32;
        h.rec_dep = z as f32;
        h.fb_pick = (t_down * 1000.0) as f32;
        h.trc_type = 1;
        let mut tt = Trace::new(dt_us, s_tot); tt.headers = h.clone();
        let mut tu = Trace::new(dt_us, s_up); tu.headers = h;
        vsp_syn.push(tt);
        up_true.push(tu);
    }

    // 1a. Slowness / interval velocity recovery
    let prof = estimate_vsp_slowness(&mut vsp_syn, &VspSlownessParams { window_channels: 9, auto_pick_fb: false, ..Default::default() }).unwrap();
    let mid_v = prof.interval_vel_mps[n_dep / 2];
    println!("[WAVE4 VSP Slowness]  Recovered V_int={mid_v:.1} m/s (true 2200.0 m/s)");
    assert!((mid_v - 2200.0).abs() < 30.0, "VSP interval velocity mismatch: {mid_v}");

    // 1b. MedianAlign & FkDip wavefield separation
    let (_, up_med) = separate_vsp_wavefields(&vsp_syn, &VspWavefieldSepParams { method: VspSepMethod::MedianAlign, median_len: 9, ..Default::default() }).unwrap();
    let (_, up_fk) = separate_vsp_wavefields(&vsp_syn, &VspWavefieldSepParams { method: VspSepMethod::FkDip, ..Default::default() }).unwrap();
    let corr_med = mean_corr(&up_true[5..35], &up_med[5..35]);
    let corr_fk = mean_corr(&up_true[5..35], &up_fk[5..35]);
    println!("[WAVE4 VSP Sep]       Upgoing corr: MedianAlign={corr_med:.4}, FkDip={corr_fk:.4}");
    assert!(corr_med > 0.90, "VSP MedianAlign upgoing correlation too low: {corr_med}");
    assert!(corr_fk > 0.90, "VSP FkDip upgoing correlation too low: {corr_fk}");

    // 1c. VSP Decon + Corridor Stack -> peak must sit at exact 2-way time 2 * 600 / 2200 = 545.5 ms!
    let decon_up = apply_vsp_decon(&vsp_syn, &VspDeconParams::default()).unwrap();
    let corr_stk = compute_vsp_corridor_stack(
        &decon_up,
        &VspCorridorStackParams {
            already_two_way_aligned: true,
            corridor_start_ms: 10.0,
            corridor_width_ms: 250.0,
            repeat_traces: 5,
            ..Default::default()
        },
    )
    .unwrap();
    let (pk_i, _) = corr_stk[0]
        .samples
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap();
    let pk_ms = pk_i as f32 * 2.0;
    let true_twt_ms = (2.0 * z_refl / v_true * 1000.0) as f32;
    println!("[WAVE4 VSP Corridor]  Stack peak at {pk_ms:.1} ms (true 2-way TWT={true_twt_ms:.1} ms)");
    assert!((pk_ms - true_twt_ms).abs() < 12.0, "VSP corridor stack peak off: {pk_ms} vs {true_twt_ms}");

    // 2. Real Field Data Validation: Otway CO2CRC Borehole DAS (Jurnal 1)
    let otway_dir = Path::new(r"C:\Kuliah\Jurnal Pak Bagus\Jurnal 1\CCS-DAS\data\otway_zenodo_7574468\RAW_seismograms");
    if otway_dir.exists() {
        let first_sgy = std::fs::read_dir(otway_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|s| s.to_str()).map(|s| s.eq_ignore_ascii_case("sgy")).unwrap_or(false));
        if let Some(sgy_path) = first_sgy {
            let (_, mut das_tr) = rustseis_io::read_segy_file(&sgy_path, Some(256)).unwrap();
            assign_vsp_geometry(
                &mut das_tr,
                &VspGeometryParams {
                    start_depth_m: 100.0,
                    channel_spacing_m: 1.0,
                    source_offset_m: 50.0,
                    preserve_existing_depth: false,
                    sort_by_depth: true,
                },
            );
            let das_up = apply_vsp_wavefield_sep(
                &das_tr,
                &VspWavefieldSepParams {
                    method: VspSepMethod::FkDip,
                    output: VspWavefieldOutput::Upgoing,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(das_up.len(), das_tr.len());
            let out_dir = Path::new(r"C:\Kuliah\Metode Seismik 2025\rustseis\out");
            let img = render_side_by_side(
                &das_tr,
                &das_up,
                &GatherRenderConfig { width: 1200, height: 700, draw_wiggles: false, max_time_ms: Some(2000.0), ..Default::default() },
            )
            .unwrap();
            img.save_png(out_dir.join("otway_das_raw_vs_upgoing.png")).unwrap();
            println!("[WAVE4 Real Otway DAS] Processed {} channels from {}, saved side-by-side QC PNG", das_tr.len(), sgy_path.display());
        }
    }
}

#[test]
fn test_wave5_sqlite_opf_branching_python_node_and_mcp() {
    // 1. SQLite OPF roundtrip via EngineSession
    let tmp_db = std::env::temp_dir().join(format!("rustseis_test_{}.opf.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&tmp_db);

    let mut sess = EngineSession::new();
    let vt = VelocityTable::new("nias_vel", vec![VelocityFunction::new(100, vec![(0.0, 1500.0), (2000.0, 2400.0)])]);
    sess.dispatch(EngineCommand::UpsertVelocityTable(vt)).unwrap();

    // Run declarative flow with `synth` -> `flow_if` (far offsets filtered) -> `flow_split` -> `python_node`!
    let flow = FlowSpec {
        name: "Wave5_Platform_Test".into(),
        description: "Verify IF/ELSE, SPLIT, Buffer, Python Node, and OPF SQLite".into(),
        steps: vec![
            FlowStep {
                tool: "synth".into(),
                enabled: true,
                label: None,
                params: json!({"n_shots": 4, "n_chans": 8, "n_samples": 128, "dt_us": 2000}),
            },
            FlowStep {
                tool: "fbpick".into(),
                enabled: true,
                label: None,
                params: json!({"save_table": "fb_w5"}),
            },
            FlowStep {
                tool: "flow_if".into(),
                enabled: true,
                label: None,
                params: json!({
                    "header": "AOFFSET",
                    "min": 150.0,
                    "max": 9999.0,
                    "then_steps": [
                        { "tool": "thdrmath", "params": { "target": "PAD_TRC", "expr": "1" } }
                    ],
                    "else_steps": [
                        { "tool": "thdrmath", "params": { "target": "PAD_TRC", "expr": "0" } }
                    ]
                }),
            },
            FlowStep {
                tool: "flow_split".into(),
                enabled: true,
                label: None,
                params: json!({
                    "output_branch": 0,
                    "branches": [
                        [ { "tool": "buffer_save", "params": { "name": "branch0" } } ],
                        [ { "tool": "debias", "params": {} }, { "tool": "buffer_save", "params": { "name": "branch1" } } ]
                    ]
                }),
            },
            FlowStep {
                tool: "python_node".into(),
                enabled: true,
                label: None,
                params: json!({
                    "inline_code": "def process(traces, params):\n    g = params.get('gain', 2.0)\n    for tr in traces:\n        tr['samples'] = [s * g for s in tr['samples']]\n    return traces\n",
                    "user_params": { "gain": -2.0 }
                }),
            },
        ],
    };

    sess.dispatch(EngineCommand::RunFlow(flow)).unwrap();
    assert_eq!(sess.active_traces.len(), 32);
    assert!(sess.context.buffers.contains_key("branch0"));
    assert!(sess.context.buffers.contains_key("branch1"));
    // Verify flow_if tagged far vs near offsets accurately
    for tr in &sess.active_traces {
        let expected = if tr.headers.aoffset >= 150.0 { 1 } else { 0 };
        assert_eq!(tr.headers.pad_trc, expected);
    }
    // Verify python_node multiplied samples by -2.0 compared to branch0 snapshot
    let b0 = &sess.context.buffers["branch0"][0].samples;
    let py_out = &sess.active_traces[0].samples;
    for (&a, &b) in b0.iter().zip(py_out.iter()) {
        assert!((b - (-2.0 * a)).abs() < 1e-4, "python_node gain mismatch: {b} vs {}", -2.0 * a);
    }

    // Save & Reload OPF SQLite database
    sess.dispatch(EngineCommand::SaveOpfDatabase { db_path: tmp_db.clone() }).unwrap();
    let db = OpfDatabase::open(&tmp_db).unwrap();
    let entries = db.list_entries().unwrap();
    assert!(entries.iter().any(|(c, n, _)| c == "VEL" && n == "nias_vel"));
    assert!(entries.iter().any(|(c, n, _)| c == "FBPICK" && n == "fb_w5"));

    let mut sess2 = EngineSession::new();
    sess2.dispatch(EngineCommand::LoadOpfDatabase { db_path: tmp_db.clone() }).unwrap();
    assert!(sess2.context.velocities.contains_key("nias_vel"));
    assert!(sess2.context.first_breaks.contains_key("fb_w5"));

    // Verify new L3 renderers (picks overlay + XY curve)
    let picks: Vec<Option<f32>> = sess.active_traces.iter().map(|t| Some(t.headers.fb_pick)).collect();
    let p_img = render_gather_with_picks(&sess.active_traces, &GatherRenderConfig { width: 400, height: 300, ..Default::default() }, &picks).unwrap();
    assert_eq!(p_img.rgba.len(), 400 * 300 * 4);
    let xy_img = render_xy_curve_plot(&[1.0, 2.0, 3.0, 4.0], &[-5.0, 2.0, 8.0, -3.0], 400, 300).unwrap();
    assert_eq!(xy_img.rgba.len(), 400 * 300 * 4);

    // Verify MCP advertises all 61 processing tools & 13 MCP tools
    let mcp_defs = mcp_tool_definitions();
    assert!(mcp_defs.as_array().unwrap().len() >= 13);
    let cat = handle_tool_call(&mut sess, "list_processing_tools", &json!({})).unwrap();
    let count = cat["count"].as_u64().unwrap();
    println!("[WAVE5 Platform+MCP]  tools={count}, OPF tables={}, python_node=OK, IF/SPLIT=OK", entries.len());
    assert!(count >= 60);

    let _ = std::fs::remove_file(&tmp_db);
}
