/// Geophysical QC and differential parity metrics (used by `parity` tests and MCP agents).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TraceStats {
    pub count: usize,
    pub dc_mean: f64,
    pub rms: f64,
    pub peak_abs: f32,
    pub min: f32,
    pub max: f32,
    pub zero_fraction: f64,
}

pub fn trace_stats(samples: &[f32]) -> TraceStats {
    if samples.is_empty() {
        return TraceStats {
            count: 0,
            dc_mean: 0.0,
            rms: 0.0,
            peak_abs: 0.0,
            min: 0.0,
            max: 0.0,
            zero_fraction: 1.0,
        };
    }
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut peak = 0.0f32;
    let mut zeros = 0usize;
    for &s in samples {
        let sf = s as f64;
        sum += sf;
        sum_sq += sf * sf;
        if s < min {
            min = s;
        }
        if s > max {
            max = s;
        }
        if s.abs() > peak {
            peak = s.abs();
        }
        if s == 0.0 {
            zeros += 1;
        }
    }
    let n = samples.len() as f64;
    TraceStats {
        count: samples.len(),
        dc_mean: sum / n,
        rms: (sum_sq / n).sqrt(),
        peak_abs: peak,
        min,
        max,
        zero_fraction: zeros as f64 / n,
    }
}

/// Normalized Root-Mean-Square difference (Kragh & Christie 2002, standard 4D/parity seismic metric).
/// Returns percentage in `[0.0, 200.0]`: `0.0` = bit-exact match.
pub fn nrms_percent(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let mut diff_sq = 0.0f64;
    let mut a_sq = 0.0f64;
    let mut b_sq = 0.0f64;
    for i in 0..n {
        let da = a[i] as f64;
        let db = b[i] as f64;
        let d = da - db;
        diff_sq += d * d;
        a_sq += da * da;
        b_sq += db * db;
    }
    let rms_diff = (diff_sq / n as f64).sqrt();
    let denom = ((a_sq / n as f64).sqrt() + (b_sq / n as f64).sqrt()) * 0.5;
    if denom < 1e-12 {
        0.0
    } else {
        100.0 * rms_diff / denom
    }
}

/// Zero-lag normalized cross-correlation in `[-1.0, 1.0]`.
pub fn zero_lag_correlation(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut a_sq = 0.0f64;
    let mut b_sq = 0.0f64;
    for i in 0..n {
        let da = a[i] as f64;
        let db = b[i] as f64;
        dot += da * db;
        a_sq += da * da;
        b_sq += db * db;
    }
    let denom = (a_sq * b_sq).sqrt();
    if denom < 1e-24 { 0.0 } else { dot / denom }
}
