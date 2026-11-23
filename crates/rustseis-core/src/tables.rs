use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 1D velocity-time function at a single CDP location (RMS stacking velocity, m/s vs. TWT ms).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VelocityFunction {
    pub cdp: i32,
    /// Sorted `(time_ms, velocity_mps)` pairs.
    pub picks: Vec<(f32, f32)>,
}

impl VelocityFunction {
    pub fn new(cdp: i32, mut picks: Vec<(f32, f32)>) -> Self {
        picks.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self { cdp, picks }
    }

    /// Linearly interpolates velocity at `time_ms`, clamping outside pick bounds.
    pub fn velocity_at(&self, time_ms: f32) -> f32 {
        if self.picks.is_empty() {
            return 1500.0;
        }
        if time_ms <= self.picks[0].0 {
            return self.picks[0].1;
        }
        let last = self.picks[self.picks.len() - 1];
        if time_ms >= last.0 {
            return last.1;
        }
        for w in self.picks.windows(2) {
            if time_ms >= w[0].0 && time_ms <= w[1].0 {
                let span = (w[1].0 - w[0].0).max(1e-6);
                let frac = (time_ms - w[0].0) / span;
                return w[0].1 + frac * (w[1].1 - w[0].1);
            }
        }
        last.1
    }

    /// Dix interval velocity conversion from RMS picks.
    pub fn interval_velocities(&self) -> Vec<(f32, f32, f32)> {
        let mut out = Vec::new();
        for w in self.picks.windows(2) {
            let (t1, v1) = (w[0].0 as f64 / 1000.0, w[0].1 as f64);
            let (t2, v2) = (w[1].0 as f64 / 1000.0, w[1].1 as f64);
            let dt = (t2 - t1).max(1e-6);
            let num = (v2 * v2 * t2 - v1 * v1 * t1).max(0.0);
            let vint = (num / dt).sqrt() as f32;
            out.push((w[0].0, w[1].0, vint));
        }
        out
    }
}

/// Spatially variant 2D velocity field `V(cdp, t)` (clean-room replacement for ProMAX VEL OPF).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VelocityTable {
    pub name: String,
    pub functions: Vec<VelocityFunction>,
}

impl VelocityTable {
    pub fn new(name: impl Into<String>, mut functions: Vec<VelocityFunction>) -> Self {
        functions.sort_by_key(|f| f.cdp);
        Self {
            name: name.into(),
            functions,
        }
    }

    pub fn constant(vel_mps: f32) -> Self {
        Self::new(
            "constant",
            vec![VelocityFunction::new(0, vec![(0.0, vel_mps)])],
        )
    }

    /// Bilinear interpolation across CDP and time (constant extrapolation outside CDP bounds).
    pub fn velocity_at(&self, cdp: i32, time_ms: f32) -> f32 {
        if self.functions.is_empty() {
            return 1500.0;
        }
        if self.functions.len() == 1 || cdp <= self.functions[0].cdp {
            return self.functions[0].velocity_at(time_ms);
        }
        let last = &self.functions[self.functions.len() - 1];
        if cdp >= last.cdp {
            return last.velocity_at(time_ms);
        }
        for w in self.functions.windows(2) {
            if cdp >= w[0].cdp && cdp <= w[1].cdp {
                let span = (w[1].cdp - w[0].cdp).max(1) as f32;
                let frac = (cdp - w[0].cdp) as f32 / span;
                let v0 = w[0].velocity_at(time_ms);
                let v1 = w[1].velocity_at(time_ms);
                return v0 + frac * (v1 - v0);
            }
        }
        last.velocity_at(time_ms)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MuteMode {
    Top,
    Bottom,
    Surgical,
}

/// Mute curve table (e.g., `(abs_offset_m, mute_time_ms)` or surgical `(x, t_start_ms, t_end_ms)`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MuteTable {
    pub name: String,
    pub mode: MuteMode,
    /// Header key used as the horizontal axis (typically `"AOFFSET"` or `"CDP"`).
    pub key_header: String,
    /// Taper ramp length in milliseconds (cosine taper).
    pub taper_ms: f32,
    /// For Top/Bottom: `(key_value, time_ms)`.
    pub picks: Vec<(f32, f32)>,
    /// For Surgical: `(key_value, t_start_ms, t_end_ms)`.
    #[serde(default)]
    pub surgical_picks: Vec<(f32, f32, f32)>,
}

impl MuteTable {
    pub fn time_at(&self, key_val: f32) -> Option<f32> {
        if self.picks.is_empty() {
            return None;
        }
        let mut sorted = self.picks.clone();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        if key_val <= sorted[0].0 {
            return Some(sorted[0].1);
        }
        let last = sorted[sorted.len() - 1];
        if key_val >= last.0 {
            return Some(last.1);
        }
        for w in sorted.windows(2) {
            if key_val >= w[0].0 && key_val <= w[1].0 {
                let frac = (key_val - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
                return Some(w[0].1 + frac * (w[1].1 - w[0].1));
            }
        }
        Some(last.1)
    }

    pub fn surgical_window_at(&self, key_val: f32) -> Option<(f32, f32)> {
        if self.surgical_picks.is_empty() {
            return None;
        }
        let mut sorted = self.surgical_picks.clone();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        if key_val <= sorted[0].0 {
            return Some((sorted[0].1, sorted[0].2));
        }
        let last = sorted[sorted.len() - 1];
        if key_val >= last.0 {
            return Some((last.1, last.2));
        }
        for w in sorted.windows(2) {
            if key_val >= w[0].0 && key_val <= w[1].0 {
                let frac = (key_val - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
                return Some((
                    w[0].1 + frac * (w[1].1 - w[0].1),
                    w[0].2 + frac * (w[1].2 - w[0].2),
                ));
            }
        }
        Some((last.1, last.2))
    }
}

/// 2D Marine Streamer Geometry definition (directly matches `Data Seismik` NIAS line!).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarineGeometry2D {
    /// Distance from source to near receiver channel (meters, positive = trailing streamer).
    pub near_offset_m: f64,
    /// Receiver group interval along streamer (meters).
    pub receiver_interval_m: f64,
    /// Nominal shot interval (meters) used if shot coordinates are absent.
    pub nominal_shot_interval_m: f64,
    /// CDP bin size (meters, typically `receiver_interval_m / 2.0`).
    pub cdp_bin_size_m: f64,
    /// Channel ordering: true if channel 1 is nearest to source (`1..N`), false if reversed (`N..1`).
    pub chan1_is_near: bool,
    /// Source depth below sea level (meters).
    pub source_depth_m: f32,
    /// Streamer depth below sea level (meters).
    pub streamer_depth_m: f32,
}

impl Default for MarineGeometry2D {
    fn default() -> Self {
        Self {
            near_offset_m: 100.0,
            receiver_interval_m: 12.5,
            nominal_shot_interval_m: 25.0,
            cdp_bin_size_m: 6.25,
            chan1_is_near: true,
            source_depth_m: 5.0,
            streamer_depth_m: 6.0,
        }
    }
}

/// 2D Straight-Line Land Geometry definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LandGeometry2D {
    pub station_interval_m: f64,
    pub cdp_bin_size_m: f64,
    pub final_datum_elev_m: f32,
    pub replacement_velocity_mps: f32,
    /// Map of `(station_id, x, y, elev)`.
    pub stations: Vec<(i32, f64, f64, f32)>,
}

/// 2D Crooked-Line (Slalom-Line) Geometry definition (`geomspread_crooked`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrookedLineGeometry2D {
    /// Station survey coordinates `(station_id, x, y, elev)`.
    pub stations: Vec<(i32, f64, f64, f32)>,
    /// Optional explicit slalom/processing-line bend vertices `(x, y)`.
    /// If empty, computed automatically as a smoothed polyline along the midpoint cloud.
    #[serde(default)]
    pub slalom_vertices: Vec<(f64, f64)>,
    /// Inline CDP bin spacing along the curved slalom line (meters).
    pub cdp_bin_size_m: f64,
    /// Crossline half-width of CDP bins (meters; traces farther from slalom line are rejected/flagged).
    #[serde(default = "default_crossline_hw")]
    pub crossline_half_width_m: f64,
    /// Number of smoothing passes when auto-generating slalom line from midpoints.
    #[serde(default = "default_smooth_passes")]
    pub auto_smooth_passes: usize,
}

fn default_crossline_hw() -> f64 {
    250.0
}
fn default_smooth_passes() -> usize {
    4
}

/// Single trace first-break pick record (`fbpick`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirstBreakPick {
    pub ffid: i32,
    pub chan: i32,
    pub sou_sloc: i32,
    pub rec_sloc: i32,
    pub offset_m: f32,
    pub time_ms: f32,
    pub snr: f32,
}

/// Collection of first-break picks (for `fbpick`, `refstatic`, and GUI overlay).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FirstBreakTable {
    pub name: String,
    pub picks: Vec<FirstBreakPick>,
}

/// Surface-consistent statics solution table (`refstatic`, `autostat`, `pwr_stat_new`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StaticsTable {
    pub name: String,
    pub kind: String,
    /// Solved source statics `SOU_STAT` keyed by `sou_sloc` (ms).
    pub source_statics_ms: BTreeMap<i32, f32>,
    /// Solved receiver statics `REC_STAT` keyed by `rec_sloc` (ms).
    pub receiver_statics_ms: BTreeMap<i32, f32>,
    /// Solved CDP structural term keyed by `cdp` (ms).
    #[serde(default)]
    pub cdp_structure_ms: BTreeMap<i32, f32>,
    /// Solved residual moveout term keyed by `cdp` (ms / km^2).
    #[serde(default)]
    pub residual_nmo: BTreeMap<i32, f32>,
    /// Estimated refractor velocity (m/s, for refraction statics) or 0.0.
    #[serde(default)]
    pub refractor_velocity_mps: f32,
    /// Final RMS traveltime/lag residual in ms.
    #[serde(default)]
    pub rms_residual_ms: f32,
}

/// Surface-consistent amplitude decomposition solution (`scamp2`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SurfaceConsistentAmpTable {
    pub name: String,
    /// Global log-amplitude mean.
    pub mean_log_amp: f64,
    /// Linear amplitude factors `exp(component)` per domain key.
    pub source_factors: BTreeMap<i32, f32>,
    pub receiver_factors: BTreeMap<i32, f32>,
    pub offset_factors: BTreeMap<i32, f32>,
    pub cdp_factors: BTreeMap<i32, f32>,
    pub channel_factors: BTreeMap<i32, f32>,
    pub rms_log_residual: f32,
}
