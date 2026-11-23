//! Defensive CWLS LAS 2.0 / 3.0 well-log reader for seismic-to-well tie workflows.
//!
//! Handles wrapped (`WRAP. YES`) and unwrapped (`WRAP. NO`) ASCII sections, mixed whitespace /
//! comma / tab delimiters, `-999.25` (or custom) null sentinels, depth units (`m` / `ft`),
//! sonic slowness units (`us/ft` / `us/m`), and density units (`g/cm3`, `g/cc`, `kg/m3`).

use rustseis_core::{Result, SeismicError};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LasCurve {
    pub mnemonic: String,
    pub unit: String,
    pub description: String,
    pub values: Vec<Option<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LasWell {
    pub name: String,
    pub uwi: String,
    pub field: String,
    pub version: String,
    pub null_value: f64,
    pub depth_unit: String,
    /// Measured depth converted to metres.
    pub depths_m: Vec<f32>,
    pub curves: Vec<LasCurve>,
}

impl LasWell {
    /// Find the first curve matching any mnemonic in `aliases` (case-insensitive).
    pub fn find_curve(&self, aliases: &[&str]) -> Option<&LasCurve> {
        for a in aliases {
            if let Some(c) = self
                .curves
                .iter()
                .find(|c| c.mnemonic.eq_ignore_ascii_case(a))
            {
                return Some(c);
            }
        }
        None
    }

    /// Extract P-wave velocity `Vp(z)` in m/s from sonic slowness (`DTC`/`DT`/`DTCO`/`DT4P`)
    /// or direct velocity (`VP`/`VEL`).
    pub fn vp_mps(&self, curve_override: Option<&str>) -> Result<Vec<Option<f32>>> {
        let curve = if let Some(name) = curve_override {
            self.find_curve(&[name]).ok_or_else(|| {
                SeismicError::NotFound(format!("Sonic/Vp curve '{name}' not found in LAS well '{}'", self.name))
            })?
        } else {
            self.find_curve(&["VP", "VEL", "DTC", "DT", "DTCO", "DT4P", "AC", "SONIC"])
                .ok_or_else(|| {
                    SeismicError::NotFound(format!(
                        "No sonic (DTC/DT/DTCO) or velocity (VP) curve in LAS well '{}'",
                        self.name
                    ))
                })?
        };
        let mnem = curve.mnemonic.to_ascii_uppercase();
        let unit = curve.unit.to_ascii_lowercase().replace(' ', "");
        let is_vel = mnem.starts_with("VP") || mnem == "VEL" || unit.contains("m/s") || unit.contains("ft/s");
        let out = curve
            .values
            .iter()
            .map(|opt| {
                opt.and_then(|v| {
                    if !v.is_finite() || v <= 0.0 {
                        return None;
                    }
                    let vp = if is_vel {
                        if unit.contains("ft") {
                            v * 0.3048
                        } else if unit.contains("km") {
                            v * 1000.0
                        } else {
                            v
                        }
                    } else if unit.contains("us/m") || unit.contains("µs/m") || unit.contains("usec/m") {
                        1_000_000.0 / v
                    } else if unit.contains("s/m") && !unit.contains("us") {
                        1.0 / v
                    } else {
                        // Standard oilfield sonic slowness: us/ft -> m/s = 304,800 / DT
                        304_800.0 / v
                    };
                    if vp > 300.0 && vp < 10_000.0 {
                        Some(vp)
                    } else {
                        None
                    }
                })
            })
            .collect();
        Ok(out)
    }

    /// Extract bulk density `rho(z)` in kg/m³ from `RHOB`/`RHOZ`/`DEN`/`ZDEN`, or fall back to
    /// Gardner's empirical relation `rho = 310 * Vp^0.25` when no density curve is present.
    pub fn density_kg_m3(
        &self,
        curve_override: Option<&str>,
        vp_fallback: &[Option<f32>],
    ) -> Vec<Option<f32>> {
        let curve = if let Some(name) = curve_override {
            self.find_curve(&[name])
        } else {
            self.find_curve(&["RHOB", "RHOZ", "DEN", "ZDEN", "RHO", "RHOB_OBS"])
        };
        if let Some(c) = curve {
            let unit = c.unit.to_ascii_lowercase().replace(' ', "");
            let is_kg_m3 = unit.contains("kg/m") || unit.contains("kgm");
            return c
                .values
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    if let Some(v) = *opt {
                        if v.is_finite() && v > 0.0 {
                            let rho = if is_kg_m3 || v > 20.0 { v } else { v * 1000.0 };
                            if rho > 800.0 && rho < 4500.0 {
                                return Some(rho);
                            }
                        }
                    }
                    vp_fallback
                        .get(i)
                        .and_then(|&vp| vp.map(|v| 310.0 * v.powf(0.25)))
                })
                .collect();
        }
        vp_fallback
            .iter()
            .map(|opt| opt.map(|v| 310.0 * v.powf(0.25)))
            .collect()
    }
}

pub fn read_las_file(path: impl AsRef<Path>) -> Result<LasWell> {
    let bytes = std::fs::read(path.as_ref())?;
    let text = String::from_utf8_lossy(&bytes);
    parse_las_str(&text)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Version,
    Well,
    Curve,
    Params,
    Other,
    Ascii,
}

fn parse_header_line(line: &str) -> Option<(String, String, String, String)> {
    let dot = line.find('.')?;
    let mnem = line[..dot].trim().to_string();
    let rest = &line[dot + 1..];
    let (unit_and_val, desc) = match rest.rfind(':') {
        Some(ci) => (&rest[..ci], rest[ci + 1..].trim().to_string()),
        None => (rest, String::new()),
    };
    let unit_and_val = unit_and_val.trim_start();
    let (unit, val) = match unit_and_val.find(char::is_whitespace) {
        Some(wi) => (
            unit_and_val[..wi].trim().to_string(),
            unit_and_val[wi..].trim().to_string(),
        ),
        None => (unit_and_val.trim().to_string(), String::new()),
    };
    Some((mnem, unit, val, desc))
}

pub fn parse_las_str(text: &str) -> Result<LasWell> {
    let mut sec = Section::None;
    let mut version = "2.0".to_string();
    let mut wrap = false;
    let mut delim_char: Option<char> = None;
    let mut null_val = -999.25f64;
    let mut well_name = String::new();
    let mut uwi = String::new();
    let mut field = String::new();
    let mut depth_unit = "m".to_string();
    let mut curves: Vec<LasCurve> = Vec::new();
    let mut ascii_tokens: Vec<f64> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(after) = line.strip_prefix('~') {
            let head = after.trim().to_ascii_uppercase();
            sec = if head.starts_with('V') {
                Section::Version
            } else if head.starts_with('W') {
                Section::Well
            } else if head.starts_with('C') {
                Section::Curve
            } else if head.starts_with('P') {
                Section::Params
            } else if head.starts_with('O') {
                Section::Other
            } else if head.starts_with('A') {
                Section::Ascii
            } else {
                Section::None
            };
            continue;
        }

        match sec {
            Section::Version => {
                if let Some((m, _u, v, _d)) = parse_header_line(line) {
                    match m.to_ascii_uppercase().as_str() {
                        "VERS" => {
                            if !v.is_empty() {
                                version = v;
                            }
                        }
                        "WRAP" => {
                            wrap = v.eq_ignore_ascii_case("YES");
                        }
                        "DLM" => {
                            let d = v.to_ascii_uppercase();
                            if d.contains("COMMA") {
                                delim_char = Some(',');
                            } else if d.contains("TAB") {
                                delim_char = Some('\t');
                            }
                        }
                        _ => {}
                    }
                }
            }
            Section::Well => {
                if let Some((m, u, v, _d)) = parse_header_line(line) {
                    match m.to_ascii_uppercase().as_str() {
                        "NULL" => {
                            if let Ok(n) = v.parse::<f64>() {
                                null_val = n;
                            }
                        }
                        "WELL" => well_name = v,
                        "UWI" => uwi = v,
                        "FLD" => field = v,
                        "STRT" | "STOP" | "STEP" if !u.is_empty() => depth_unit = u,
                        _ => {}
                    }
                }
            }
            Section::Curve => {
                if let Some((m, u, _v, d)) = parse_header_line(line) {
                    if !m.is_empty() {
                        if curves.is_empty() && !u.is_empty() {
                            depth_unit = u.clone();
                        }
                        curves.push(LasCurve {
                            mnemonic: m,
                            unit: u,
                            description: d,
                            values: Vec::new(),
                        });
                    }
                }
            }
            Section::Ascii => {
                if let Some(ch) = delim_char {
                    for tok in line.split(ch) {
                        let t = tok.trim();
                        if !t.is_empty() {
                            ascii_tokens.push(t.parse::<f64>().unwrap_or(f64::NAN));
                        }
                    }
                } else {
                    for tok in line.split_whitespace() {
                        ascii_tokens.push(tok.parse::<f64>().unwrap_or(f64::NAN));
                    }
                }
            }
            Section::Params | Section::Other | Section::None => {}
        }
    }

    if curves.is_empty() {
        return Err(SeismicError::InvalidFormat("LAS file contains no ~Curve definitions".into()));
    }
    let nc = curves.len();
    let n_rows = ascii_tokens.len() / nc;
    if n_rows == 0 {
        return Err(SeismicError::InvalidFormat("LAS file contains no ~ASCII log samples".into()));
    }
    let _ = wrap; // Token-stream parsing seamlessly handles both WRAP=YES and WRAP=NO!
    let is_feet = depth_unit.to_ascii_lowercase().starts_with('f');
    let depth_scale = if is_feet { 0.3048f64 } else { 1.0f64 };
    let mut depths_m = Vec::with_capacity(n_rows);
    for c in &mut curves {
        c.values.reserve(n_rows);
    }
    for row in 0..n_rows {
        let base = row * nc;
        let d_raw = ascii_tokens[base];
        depths_m.push((d_raw * depth_scale) as f32);
        for ci in 0..nc {
            let val = ascii_tokens[base + ci];
            let is_null = !val.is_finite() || (val - null_val).abs() < 1e-3;
            curves[ci].values.push((!is_null).then_some(val as f32));
        }
    }

    Ok(LasWell {
        name: if well_name.is_empty() { "Unnamed Well".into() } else { well_name },
        uwi,
        field,
        version,
        null_value: null_val,
        depth_unit,
        depths_m,
        curves,
    })
}
