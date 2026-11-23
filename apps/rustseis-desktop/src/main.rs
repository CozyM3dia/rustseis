//! `rustseis-desktop.exe` — native eframe host for the RustSeis workstation.
//!
//! Usage: `rustseis-desktop [file] [--tour <screenshot-dir>]` where `file` is seismic data (.sgy/.segd/.rsds), a flow
//! (.json/.toml) or an OPF database (.opf.sqlite).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rustseis_ui_egui::RustSeisApp;

fn icon() -> eframe::egui::IconData {
    // 64x64 procedural mark: teal wavelet with a positive lobe on deep ink.
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let i = (y * n + x) * 4;
            let (fx, fy) = (x as f32 / (n - 1) as f32, y as f32 / (n - 1) as f32);
            let r = ((fx - 0.5).powi(2) + (fy - 0.5).powi(2)).sqrt();
            let inside = (fx - 0.5).abs() < 0.46 && (fy - 0.5).abs() < 0.46 && r < 0.62;
            let t = (fx - 0.5) * 6.0;
            let wave = 0.5 - 0.3 * (1.0 - 2.0 * t * t) * (-t * t).exp();
            let on_wave = (fy - wave).abs() < 0.045;
            let lobe = fy < 0.5 && fy > wave && (fx - 0.5).abs() < 0.12;
            let (cr, cg, cb, ca) = if !inside {
                (0, 0, 0, 0)
            } else if on_wave {
                (47, 200, 184, 255)
            } else if lobe {
                (30, 120, 112, 255)
            } else {
                (12, 18, 26, 255)
            };
            rgba[i..i + 4].copy_from_slice(&[cr, cg, cb, ca]);
        }
    }
    eframe::egui::IconData { rgba, width: n as u32, height: n as u32 }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tour_dir = args.iter().position(|a| a == "--tour").map(|i| {
        std::path::PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "screenshots".into()))
    });
    let initial = args.iter().find(|a| !a.starts_with("--") && Some(std::path::PathBuf::from(a.as_str())) != tour_dir).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("RustSeis — 2D Seismic Processing Workstation")
            .with_inner_size([1600.0, 960.0])
            .with_min_inner_size([1100.0, 680.0])
            .with_maximized(tour_dir.is_some())
            .with_icon(icon()),
        ..Default::default()
    };
    eframe::run_native(
        "RustSeis",
        options,
        Box::new(move |cc| {
            let mut app = RustSeisApp::new(cc);
            if let Some(p) = initial.as_ref() {
                app.open_path(p);
            }
            if let Some(dir) = tour_dir.as_ref() {
                let flows = std::env::current_dir().unwrap_or_default().join("flows");
                app.tour = Some(rustseis_ui_egui::tour::Tour::standard(dir.clone(), &flows));
            }
            Ok(Box::new(app))
        }),
    )
}
