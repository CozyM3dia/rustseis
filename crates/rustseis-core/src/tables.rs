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

    /// Smooth velocity picks laterally across adjacent CDP functions and gently in time
    /// (clean-room equivalent of ProMAX Velocity Manipulation lateral/vertical smoothing).
    pub fn smoothed(&self, lateral_radius: usize) -> Self {
        if self.functions.len() <= 1 || lateral_radius == 0 {
            return self.clone();
        }
        let n = self.functions.len();
        let mut out_fns = Vec::with_capacity(n);
        for (i, f) in self.functions.iter().enumerate() {
            let lo = i.saturating_sub(lateral_radius);
            let hi = (i + lateral_radius).min(n - 1);
            let mut new_picks = Vec::with_capacity(f.picks.len());
            for &(t_ms, _) in &f.picks {
                let mut wsum = 0.0f64;
                let mut vsum = 0.0f64;
                for j in lo..=hi {
                    let dist = (j as isize - i as isize).unsigned_abs();
                    let w = (lateral_radius + 1 - dist) as f64;
                    vsum += w * self.functions[j].velocity_at(t_ms) as f64;
                    wsum += w;
                }
                let v = if wsum > 0.0 { (vsum / wsum) as f32 } else { f.velocity_at(t_ms) };
                new_picks.push((t_ms, v));
            }
            out_fns.push(VelocityFunction::new(f.cdp, new_picks));
        }
        Self::new(self.name.clone(), out_fns)
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
    /// Sail-line azimuth clockwise from North in degrees (`90.0` = East). Used when shot coords are absent.
    #[serde(default = "default_marine_azimuth")]
    pub sail_line_azimuth_deg: f64,
    /// Perpendicular (crossline) offset from source to streamer line (meters).
    #[serde(default)]
    pub perp_offset_m: f64,
    /// Optional starting shot station number (`0` = use trace `FFID`).
    #[serde(default)]
    pub first_shot_station: i32,
    /// X coordinate of first shot when shot coordinates are absent (meters).
    #[serde(default)]
    pub first_shot_x: f64,
    /// Y coordinate of first shot when shot coordinates are absent (meters).
    #[serde(default)]
    pub first_shot_y: f64,
    /// Compute and record sea-level source/receiver datum statics into `SOU_STAT`/`REC_STAT`/`NA_STAT`.
    #[serde(default)]
    pub apply_sea_level_static: bool,
    /// Water velocity used for sea-level static calculation (m/s, ~1481 m/s ≈ 4860 ft/s).
    #[serde(default = "default_water_vel")]
    pub water_velocity_mps: f32,
}

fn default_marine_azimuth() -> f64 {
    90.0
}

fn default_water_vel() -> f32 {
    1481.0
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
            sail_line_azimuth_deg: default_marine_azimuth(),
            perp_offset_m: 0.0,
            first_shot_station: 0,
            first_shot_x: 0.0,
            first_shot_y: 0.0,
            apply_sea_level_static: false,
            water_velocity_mps: default_water_vel(),
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

/// Gridded 2D depth interval velocity model `V(x, z)` (m/s) in row-major `[iz * nx + ix]` order.
/// Shared foundation for Kirchhoff PSDM, 2D acoustic RTM, AVO ray-parameter conversion, and depth well-tie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VelocityGrid2D {
    pub name: String,
    pub nx: usize,
    pub nz: usize,
    pub x0_m: f32,
    pub dx_m: f32,
    pub z0_m: f32,
    pub dz_m: f32,
    /// Interval velocity in m/s, length `nx * nz`, indexed as `iz * nx + ix`.
    pub v_mps: Vec<f32>,
}

impl VelocityGrid2D {
    pub fn constant(
        name: impl Into<String>,
        nx: usize,
        nz: usize,
        x0_m: f32,
        dx_m: f32,
        z0_m: f32,
        dz_m: f32,
        v_mps: f32,
    ) -> Self {
        Self {
            name: name.into(),
            nx,
            nz,
            x0_m,
            dx_m,
            z0_m,
            dz_m,
            v_mps: vec![v_mps; nx * nz],
        }
    }

    /// Analytical linear depth-gradient model `V(z) = v0 + grad_s_inv * (z - z0)`.
    pub fn linear_gradient(
        name: impl Into<String>,
        nx: usize,
        nz: usize,
        x0_m: f32,
        dx_m: f32,
        z0_m: f32,
        dz_m: f32,
        v0_mps: f32,
        grad_per_s: f32,
    ) -> Self {
        let mut v = vec![0.0f32; nx * nz];
        for iz in 0..nz {
            let z = z0_m + iz as f32 * dz_m;
            let vz = (v0_mps + grad_per_s * (z - z0_m)).max(300.0);
            for ix in 0..nx {
                v[iz * nx + ix] = vz;
            }
        }
        Self {
            name: name.into(),
            nx,
            nz,
            x0_m,
            dx_m,
            z0_m,
            dz_m,
            v_mps: v,
        }
    }

    #[inline]
    pub fn x_at(&self, ix: usize) -> f32 {
        self.x0_m + ix as f32 * self.dx_m
    }

    #[inline]
    pub fn z_at(&self, iz: usize) -> f32 {
        self.z0_m + iz as f32 * self.dz_m
    }

    #[inline]
    pub fn get(&self, ix: usize, iz: usize) -> f32 {
        self.v_mps[iz.min(self.nz - 1) * self.nx + ix.min(self.nx - 1)]
    }

    #[inline]
    pub fn set(&mut self, ix: usize, iz: usize, val: f32) {
        if ix < self.nx && iz < self.nz {
            self.v_mps[iz * self.nx + ix] = val;
        }
    }

    /// Bilinear interpolation of interval velocity at `(x_m, z_m)` with edge clamping.
    pub fn sample(&self, x_m: f32, z_m: f32) -> f32 {
        if self.nx == 0 || self.nz == 0 || self.v_mps.is_empty() {
            return 1500.0;
        }
        let fx = ((x_m - self.x0_m) / self.dx_m.max(1e-6)).clamp(0.0, (self.nx - 1) as f32);
        let fz = ((z_m - self.z0_m) / self.dz_m.max(1e-6)).clamp(0.0, (self.nz - 1) as f32);
        let ix0 = fx.floor() as usize;
        let iz0 = fz.floor() as usize;
        let ix1 = (ix0 + 1).min(self.nx - 1);
        let iz1 = (iz0 + 1).min(self.nz - 1);
        let tx = fx - ix0 as f32;
        let tz = fz - iz0 as f32;
        let v00 = self.get(ix0, iz0);
        let v10 = self.get(ix1, iz0);
        let v01 = self.get(ix0, iz1);
        let v11 = self.get(ix1, iz1);
        let v0 = v00 + tx * (v10 - v00);
        let v1 = v01 + tx * (v11 - v01);
        v0 + tz * (v1 - v0)
    }

    pub fn v_min_max(&self) -> (f32, f32) {
        let mut vmin = f32::INFINITY;
        let mut vmax = f32::NEG_INFINITY;
        for &v in &self.v_mps {
            if v.is_finite() {
                vmin = vmin.min(v);
                vmax = vmax.max(v);
            }
        }
        if !vmin.is_finite() { (1500.0, 1500.0) } else { (vmin, vmax) }
    }

    /// Separable boxcar smoothing (2 passes ≈ triangular, 3 passes ≈ Gaussian) to stabilize
    /// Eikonal ray tracing and prevent artificial internal reflections in RTM.
    pub fn smooth(&mut self, rx: usize, rz: usize, passes: usize) {
        if self.nx == 0 || self.nz == 0 || passes == 0 || (rx == 0 && rz == 0) {
            return;
        }
        let mut tmp = vec![0.0f32; self.nx * self.nz];
        for _ in 0..passes {
            if rx > 0 {
                for iz in 0..self.nz {
                    let row = iz * self.nx;
                    for ix in 0..self.nx {
                        let lo = ix.saturating_sub(rx);
                        let hi = (ix + rx).min(self.nx - 1);
                        let mut sum = 0.0f64;
                        for j in lo..=hi {
                            sum += self.v_mps[row + j] as f64;
                        }
                        tmp[row + ix] = (sum / (hi - lo + 1) as f64) as f32;
                    }
                }
                self.v_mps.copy_from_slice(&tmp);
            }
            if rz > 0 {
                for ix in 0..self.nx {
                    for iz in 0..self.nz {
                        let lo = iz.saturating_sub(rz);
                        let hi = (iz + rz).min(self.nz - 1);
                        let mut sum = 0.0f64;
                        for j in lo..=hi {
                            sum += self.v_mps[j * self.nx + ix] as f64;
                        }
                        tmp[iz * self.nx + ix] = (sum / (hi - lo + 1) as f64) as f32;
                    }
                }
                self.v_mps.copy_from_slice(&tmp);
            }
        }
    }

    /// Convert a picked RMS `VelocityTable` (indexed by CDP and TWT ms) into a gridded depth
    /// interval velocity model `V(x, z)` via Dix conversion and monotonic time-to-depth stretch.
    ///
    /// `cdp_x_pairs` maps `(cdp, x_m)` along the line (if empty, `x = cdp * cdp_spacing_m`).
    #[allow(clippy::too_many_arguments)]
    pub fn from_velocity_table(
        table: &VelocityTable,
        name: impl Into<String>,
        x0_m: f32,
        dx_m: f32,
        nx: usize,
        z0_m: f32,
        dz_m: f32,
        nz: usize,
        cdp_x_pairs: &[(i32, f32)],
        cdp_spacing_m: f32,
        v_min_clamp: f32,
        v_max_clamp: f32,
        smooth_x_cells: usize,
        smooth_z_cells: usize,
    ) -> Self {
        let mut grid = Self::constant(name, nx, nz, x0_m, dx_m, z0_m, dz_m, 1500.0);
        if nx == 0 || nz == 0 {
            return grid;
        }
        let v_lo = v_min_clamp.max(300.0);
        let v_hi = v_max_clamp.max(v_lo + 100.0);

        // Map x_m -> approximate CDP number
        let x_to_cdp = |x: f32| -> f32 {
            if cdp_x_pairs.len() >= 2 {
                if x <= cdp_x_pairs[0].1 {
                    return cdp_x_pairs[0].0 as f32;
                }
                let last = cdp_x_pairs[cdp_x_pairs.len() - 1];
                if x >= last.1 {
                    return last.0 as f32;
                }
                for w in cdp_x_pairs.windows(2) {
                    let (c0, x0) = (w[0].0 as f32, w[0].1);
                    let (c1, x1) = (w[1].0 as f32, w[1].1);
                    if (x >= x0 && x <= x1) || (x >= x1 && x <= x0) {
                        let span = x1 - x0;
                        if span.abs() < 1e-6 {
                            return c0;
                        }
                        return c0 + (x - x0) / span * (c1 - c0);
                    }
                }
                last.0 as f32
            } else if cdp_x_pairs.len() == 1 {
                cdp_x_pairs[0].0 as f32
            } else {
                x / cdp_spacing_m.max(1e-3)
            }
        };

        // Determine max time needed to cover depth z_max
        let z_max = z0_m + (nz.saturating_sub(1)) as f32 * dz_m;
        let max_t_s = ((2.0 * z_max / v_lo) as f64).clamp(1.0, 20.0);
        let dt_s = 0.004f64;
        let nt = ((max_t_s / dt_s).ceil() as usize).max(32);

        for ix in 0..nx {
            let x = x0_m + ix as f32 * dx_m;
            let cdp_f = x_to_cdp(x);
            let cdp_lo = cdp_f.floor() as i32;
            let cdp_hi = cdp_f.ceil() as i32;
            let frac = cdp_f - cdp_lo as f32;

            // Sample smooth RMS velocity curve v_rms(t) at this x
            let v_rms_at = |t_ms: f32| -> f64 {
                let va = table.velocity_at(cdp_lo, t_ms) as f64;
                let vb = table.velocity_at(cdp_hi, t_ms) as f64;
                va + frac as f64 * (vb - va)
            };

            // Compute Dix interval velocity V_int(t) and integrate z(t) = 0.5 * integral V_int dt
            let mut z_of_t = Vec::with_capacity(nt);
            let mut vint_of_t = Vec::with_capacity(nt);
            let mut z_cur = 0.0f64;
            let mut prev_v2t = 0.0f64;
            let mut prev_vint = v_rms_at(0.0).clamp(v_lo as f64, v_hi as f64);
            for it in 0..nt {
                let t_s = it as f64 * dt_s;
                let vr = v_rms_at((t_s * 1000.0) as f32);
                let v2t = vr * vr * t_s;
                let vint = if it == 0 {
                    vr.clamp(v_lo as f64, v_hi as f64)
                } else {
                    let dv2t = v2t - prev_v2t;
                    if dv2t > 0.0 {
                        (dv2t / dt_s).sqrt().clamp(v_lo as f64, v_hi as f64)
                    } else {
                        prev_vint
                    }
                };
                if it > 0 {
                    z_cur += 0.5 * 0.5 * (prev_vint + vint) * dt_s;
                }
                z_of_t.push(z_cur);
                vint_of_t.push(vint as f32);
                prev_v2t = v2t;
                prev_vint = vint;
            }

            // Resample V_int(z) onto uniform depth grid
            let mut k = 0usize;
            for iz in 0..nz {
                let z = (z0_m + iz as f32 * dz_m).max(0.0) as f64;
                while k + 1 < z_of_t.len() && z_of_t[k + 1] < z {
                    k += 1;
                }
                let val = if k + 1 >= z_of_t.len() {
                    vint_of_t[vint_of_t.len() - 1]
                } else {
                    let dz_seg = (z_of_t[k + 1] - z_of_t[k]).max(1e-9);
                    let w = ((z - z_of_t[k]) / dz_seg).clamp(0.0, 1.0) as f32;
                    vint_of_t[k] + w * (vint_of_t[k + 1] - vint_of_t[k])
                };
                grid.set(ix, iz, val);
            }
        }

        if smooth_x_cells > 0 || smooth_z_cells > 0 {
            grid.smooth(smooth_x_cells, smooth_z_cells, 2);
        }
        grid
    }
}
