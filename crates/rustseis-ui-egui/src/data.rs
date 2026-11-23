//! Read-side helpers over the engine session: where traces come from, how they group into
//! ensembles, and how large trace sets are decimated before rasterisation.

use rustseis_core::Trace;
use rustseis_engine::EngineSession;
use rustseis_render::{GatherRenderConfig, RenderedImage, SeismicColormap, render_seismic_gather};

/// Which trace set a view reads: the session's active traces or a named `FlowContext` buffer.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub enum TraceSource {
    #[default]
    Active,
    Buffer(String),
}

impl TraceSource {
    pub fn label(&self) -> String {
        match self {
            Self::Active => "Active traces".into(),
            Self::Buffer(n) => format!("Buffer · {n}"),
        }
    }

    pub fn traces<'a>(&self, session: &'a EngineSession) -> &'a [Trace] {
        match self {
            Self::Active => &session.active_traces,
            Self::Buffer(name) => session.context.buffers.get(name).map(|v| v.as_slice()).unwrap_or(&[]),
        }
    }

    pub fn all(session: &EngineSession) -> Vec<TraceSource> {
        let mut out = vec![TraceSource::Active];
        let mut names: Vec<&String> = session.context.buffers.keys().collect();
        names.sort();
        out.extend(names.into_iter().map(|n| TraceSource::Buffer(n.clone())));
        out
    }
}

/// Ensemble keys offered in pickers. `SECTION` means "no grouping: the whole trace set".
pub const ENSEMBLE_KEYS: &[&str] = &["FFID", "CDP", "CHAN", "SOU_SLOC", "REC_SLOC", "OFFSET", "SECTION"];
pub const SECTION_KEY: &str = "SECTION";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnsRange {
    pub key_value: i64,
    pub start: usize,
    pub end: usize,
}

impl EnsRange {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

/// Contiguous grouping identical to `Ensemble::group_by_header`, but index-only (no clones).
#[derive(Clone, Debug, Default)]
pub struct EnsembleIndex {
    pub source: TraceSource,
    pub key: String,
    pub rev: u64,
    pub ranges: Vec<EnsRange>,
}

impl EnsembleIndex {
    pub fn build(traces: &[Trace], source: TraceSource, key: &str, rev: u64) -> Self {
        let mut ranges = Vec::new();
        if key == SECTION_KEY || traces.is_empty() {
            if !traces.is_empty() {
                ranges.push(EnsRange { key_value: 0, start: 0, end: traces.len() });
            }
        } else {
            let getter = header_getter(key);
            let mut start = 0usize;
            let mut cur = getter(&traces[0]);
            for (i, tr) in traces.iter().enumerate().skip(1) {
                let k = getter(tr);
                if k != cur {
                    ranges.push(EnsRange { key_value: cur, start, end: i });
                    start = i;
                    cur = k;
                }
            }
            ranges.push(EnsRange { key_value: cur, start, end: traces.len() });
        }
        Self { source, key: key.to_string(), rev, ranges }
    }

    pub fn matches(&self, source: &TraceSource, key: &str, rev: u64) -> bool {
        &self.source == source && self.key == key && self.rev == rev
    }

    pub fn find_trace(&self, trace_idx: usize) -> Option<usize> {
        self.ranges.binary_search_by(|r| {
            if trace_idx < r.start {
                std::cmp::Ordering::Greater
            } else if trace_idx >= r.end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }).ok()
    }

    pub fn find_key(&self, key_value: i64) -> Option<usize> {
        self.ranges.iter().position(|r| r.key_value == key_value)
    }
}

/// Fast header access for common keys, falling back to the dynamic `get_f64` lookup.
pub fn header_getter(key: &str) -> Box<dyn Fn(&Trace) -> i64 + Send + Sync> {
    match key.to_ascii_uppercase().as_str() {
        "FFID" => Box::new(|t: &Trace| t.headers.ffid as i64),
        "CDP" => Box::new(|t: &Trace| t.headers.cdp as i64),
        "CHAN" => Box::new(|t: &Trace| t.headers.chan as i64),
        "SOU_SLOC" => Box::new(|t: &Trace| t.headers.sou_sloc as i64),
        "REC_SLOC" => Box::new(|t: &Trace| t.headers.rec_sloc as i64),
        "OFFSET" => Box::new(|t: &Trace| t.headers.offset.round() as i64),
        "AOFFSET" => Box::new(|t: &Trace| t.headers.aoffset.round() as i64),
        "REC_DEP" => Box::new(|t: &Trace| t.headers.rec_dep.round() as i64),
        other => {
            let k = other.to_string();
            Box::new(move |t: &Trace| header_f64(t, &k).round() as i64)
        }
    }
}

/// Header value as f64 including `custom` entries (e.g. `XLINE_DIST`, `SLALOM_S`).
pub fn header_f64(t: &Trace, key: &str) -> f64 {
    t.headers
        .get_f64(key)
        .or_else(|| t.headers.custom.get(&key.to_ascii_uppercase()).copied())
        .or_else(|| t.headers.custom.get(key).copied())
        .unwrap_or(0.0)
}

/// Header keys useful as horizontal-axis labels.
pub const LABEL_KEYS: &[&str] = &["CHAN", "OFFSET", "AOFFSET", "CDP", "FFID", "TRACENO", "REC_DEP", "SOU_SLOC", "REC_SLOC"];

pub fn time_axis_ms(traces: &[Trace]) -> (f32, usize) {
    traces.first().map(|t| (t.dt_ms().max(1e-3), t.samples.len())).unwrap_or((4.0, 0))
}

/// Raster description in data units: the texture's outer pixel edges map to these extents.
#[derive(Clone, Copy, Debug)]
pub struct RasterExtent {
    pub x0: f64,
    pub x1: f64,
    pub y0: f64,
    pub y1: f64,
}

/// Data-resolution gather raster request (1 texel per trace column per sample row, capped).
#[derive(Clone, Debug)]
pub struct GatherRasterJob {
    pub traces: Vec<Trace>,
    pub colormap: SeismicColormap,
    pub clip_percentile: f32,
    /// Data extent of the trace range this job covers (trace units).
    pub x0: f64,
    pub x1: f64,
    pub max_rows: usize,
}

/// Max texture side we ever upload; keeps VRAM bounded on huge sections.
pub const MAX_RASTER_SIDE: usize = 4096;

/// Decimated clone of `traces[i0..i1]` limited to `max_cols` columns (nearest-trace sampling,
/// exactly what the renderer would do internally, but without cloning 1.9 GB first).
pub fn decimated_clone(traces: &[Trace], i0: usize, i1: usize, max_cols: usize) -> Vec<Trace> {
    let i1 = i1.min(traces.len());
    let n = i1.saturating_sub(i0);
    if n == 0 {
        return Vec::new();
    }
    if n <= max_cols {
        return traces[i0..i1].to_vec();
    }
    (0..max_cols)
        .map(|c| {
            let idx = i0 + ((c as f64 + 0.5) / max_cols as f64 * n as f64).floor() as usize;
            traces[idx.min(i1 - 1)].clone()
        })
        .collect()
}

/// Render a gather at data resolution, stripping the renderer's baked padding so the result is
/// exactly the plot area. Returns RGBA + (cols, rows).
pub fn render_gather_raster(
    traces: &[Trace],
    colormap: SeismicColormap,
    clip_percentile: f32,
    max_rows: usize,
) -> Result<egui::ColorImage, String> {
    if traces.is_empty() || traces[0].samples.is_empty() {
        return Err("empty trace set".into());
    }
    let cols = traces.len().min(MAX_RASTER_SIDE);
    let rows = traces[0].samples.len().min(max_rows.clamp(16, MAX_RASTER_SIDE));
    let cfg = GatherRenderConfig {
        width: (cols + 64) as u32,
        height: (rows + 48) as u32,
        colormap,
        clip_percentile,
        draw_wiggles: false,
        max_time_ms: None,
    };
    let img = render_seismic_gather(traces, &cfg).map_err(|e| e.to_string())?;
    Ok(crop_rgba(&img, 48, 24, cols, rows))
}

/// Crop a sub-rectangle of a rendered RGBA buffer into an egui image.
pub fn crop_rgba(img: &RenderedImage, x: usize, y: usize, w: usize, h: usize) -> egui::ColorImage {
    let iw = img.width as usize;
    let ih = img.height as usize;
    let w = w.min(iw.saturating_sub(x)).max(1);
    let h = h.min(ih.saturating_sub(y)).max(1);
    let mut out = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        let start = (row * iw + x) * 4;
        out.extend_from_slice(&img.rgba[start..start + w * 4]);
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &out)
}

/// Whole rendered image to egui (no crop).
pub fn to_color_image(img: &RenderedImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba)
}

/// Sorted-by-key clone of a trace slice (stable), e.g. supergathers sorted by AOFFSET.
pub fn sorted_by(mut traces: Vec<Trace>, key: &str) -> Vec<Trace> {
    let g = header_getter(key);
    traces.sort_by_key(|t| g(t));
    traces
}

pub fn fmt_count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn fmt_ms(ms: f64) -> String {
    if ms >= 10_000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{ms:.0} ms")
    }
}

pub fn fmt_bytes(b: usize) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} kB", b / 1e3)
    }
}

pub fn traces_bytes(traces: &[Trace]) -> usize {
    traces.iter().map(|t| t.samples.len() * 4 + 512).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(ffid: i32, chan: i32) -> Trace {
        let mut t = Trace::new(4000, vec![0.0; 10]);
        t.headers.ffid = ffid;
        t.headers.chan = chan;
        t
    }

    #[test]
    fn ensemble_index_matches_contiguous_grouping() {
        let traces = vec![tr(1, 1), tr(1, 2), tr(2, 1), tr(2, 2), tr(2, 3), tr(1, 9)];
        let idx = EnsembleIndex::build(&traces, TraceSource::Active, "FFID", 0);
        let lens: Vec<usize> = idx.ranges.iter().map(|r| r.len()).collect();
        assert_eq!(lens, vec![2, 3, 1]);
        assert_eq!(idx.find_trace(3), Some(1));
        assert_eq!(idx.find_trace(5), Some(2));
        let section = EnsembleIndex::build(&traces, TraceSource::Active, SECTION_KEY, 0);
        assert_eq!(section.ranges.len(), 1);
    }

    #[test]
    fn decimation_caps_columns_and_keeps_order() {
        let traces: Vec<Trace> = (0..10_000).map(|i| tr(i, 1)).collect();
        let d = decimated_clone(&traces, 0, traces.len(), 100);
        assert_eq!(d.len(), 100);
        assert!(d.windows(2).all(|w| w[0].headers.ffid < w[1].headers.ffid));
    }

    #[test]
    fn raster_is_cropped_to_data_resolution() {
        let traces: Vec<Trace> = (0..37)
            .map(|i| {
                let mut t = Trace::new(4000, (0..250).map(|s| ((s + i) as f32 * 0.1).sin()).collect());
                t.headers.chan = i;
                t
            })
            .collect();
        let img = render_gather_raster(&traces, SeismicColormap::BlueWhiteRed, 98.0, 4096).unwrap();
        assert_eq!(img.size, [37, 250]);
    }

    #[test]
    fn thousands_separator() {
        assert_eq!(fmt_count(121152), "121,152");
        assert_eq!(fmt_count(96), "96");
    }
}
