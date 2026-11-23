use crate::headers::{TraceHeaders, TraceType};
use serde::{Deserialize, Serialize};

/// Core seismic trace: IEEE f32 amplitude samples + sample interval (`dt_us`) + ProMAX headers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub dt_us: u32,
    pub headers: TraceHeaders,
    pub samples: Vec<f32>,
}

impl Trace {
    pub fn new(dt_us: u32, samples: Vec<f32>) -> Self {
        let len_ms = if samples.is_empty() {
            0.0
        } else {
            ((samples.len() - 1) as f32) * (dt_us as f32 / 1000.0)
        };
        let mut headers = TraceHeaders::default();
        headers.tlive_s = 0.0;
        headers.tfull_s = 0.0;
        headers.tfull_e = len_ms;
        headers.tlive_e = len_ms;
        Self {
            dt_us,
            headers,
            samples,
        }
    }

    #[inline]
    pub fn dt_ms(&self) -> f32 {
        self.dt_us as f32 / 1000.0
    }

    #[inline]
    pub fn dt_s(&self) -> f32 {
        self.dt_us as f32 / 1_000_000.0
    }

    #[inline]
    pub fn n_samples(&self) -> usize {
        self.samples.len()
    }

    #[inline]
    pub fn duration_ms(&self) -> f32 {
        if self.samples.len() <= 1 {
            0.0
        } else {
            (self.samples.len() - 1) as f32 * self.dt_ms()
        }
    }

    #[inline]
    pub fn is_live(&self) -> bool {
        self.headers.trc_type == TraceType::Live as i32
    }

    /// Returns inclusive-exclusive `[start_idx, end_idx)` live sample range honoring
    /// ProMAX `TLIVE_S` and `TLIVE_E` headers (falls back to full trace if unset).
    pub fn live_sample_range(&self) -> (usize, usize) {
        let n = self.samples.len();
        if n == 0 || self.dt_us == 0 {
            return (0, 0);
        }
        let dt_ms = self.dt_ms();
        let start = if self.headers.tlive_s > 0.0 {
            ((self.headers.tlive_s / dt_ms).floor() as usize).min(n)
        } else {
            0
        };
        let end = if self.headers.tlive_e > self.headers.tlive_s {
            (((self.headers.tlive_e / dt_ms).ceil() as usize) + 1).min(n)
        } else {
            n
        };
        if start >= end { (0, n) } else { (start, end) }
    }

    pub fn kill(&mut self) {
        self.headers.trc_type = TraceType::Dead as i32;
        self.samples.fill(0.0);
    }

    pub fn reverse_polarity(&mut self) {
        for s in &mut self.samples {
            *s = -*s;
        }
    }
}

/// A seismic gather / ensemble (e.g., Shot gather keyed by `FFID` or CDP gather keyed by `CDP`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ensemble {
    pub key_name: String,
    pub key_value: i32,
    pub traces: Vec<Trace>,
}

impl Ensemble {
    pub fn new(key_name: impl Into<String>, key_value: i32, mut traces: Vec<Trace>) -> Self {
        let n = traces.len();
        for (i, tr) in traces.iter_mut().enumerate() {
            tr.headers.seqno = (i + 1) as i32;
            tr.headers.end_ens = if i + 1 == n { 1 } else { 0 };
        }
        Self {
            key_name: key_name.into(),
            key_value,
            traces,
        }
    }

    /// Groups flat traces into ensembles by a ProMAX header key (contiguous or sorted).
    pub fn group_by_header(traces: Vec<Trace>, key_name: &str) -> Vec<Self> {
        let mut ensembles = Vec::new();
        let mut current: Vec<Trace> = Vec::new();
        let mut current_key: Option<i32> = None;

        for tr in traces {
            let k = tr.headers.get_f64(key_name).unwrap_or(0.0).round() as i32;
            match current_key {
                Some(ck) if ck == k => current.push(tr),
                Some(ck) => {
                    ensembles.push(Self::new(key_name, ck, std::mem::take(&mut current)));
                    current.push(tr);
                    current_key = Some(k);
                }
                None => {
                    current.push(tr);
                    current_key = Some(k);
                }
            }
        }
        if let Some(ck) = current_key {
            ensembles.push(Self::new(key_name, ck, current));
        }
        ensembles
    }

    /// Flattens ensembles back into a trace vector while marking `END_ENS` and `EOJ`.
    pub fn flatten(ensembles: Vec<Self>) -> Vec<Trace> {
        let mut out = Vec::new();
        for ens in ensembles {
            let n = ens.traces.len();
            for (i, mut tr) in ens.traces.into_iter().enumerate() {
                tr.headers.seqno = (i + 1) as i32;
                tr.headers.end_ens = if i + 1 == n { 1 } else { 0 };
                tr.headers.eoj = 0;
                out.push(tr);
            }
        }
        if let Some(last) = out.last_mut() {
            last.headers.eoj = 1;
        }
        out
    }
}
