use memmap2::Mmap;
use rustseis_core::{Result, SeismicError, Trace, TraceHeaders};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Fast internal RustSeis dataset format (clean-room replacement for ProMAX Disk Datasets):
/// - `<name>.rsds.bin`: contiguous little-endian IEEE `f32` samples (memory-mapped for zero-copy GUI/DSP access)
/// - `<name>.rsds.json`: metadata + trace headers array
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetMeta {
    pub name: String,
    pub dt_us: u32,
    pub n_samples: usize,
    pub n_traces: usize,
    pub sort_order: String,
    pub headers: Vec<TraceHeaders>,
}

pub struct MmapDataset {
    pub meta: DatasetMeta,
    mmap: Mmap,
}

fn paths_for(base: impl AsRef<Path>) -> (PathBuf, PathBuf) {
    let base_str = base.as_ref().to_string_lossy();
    let stem = base_str.trim_end_matches(".rsds.bin").trim_end_matches(".rsds.json").trim_end_matches(".rsds");
    (PathBuf::from(format!("{stem}.rsds.bin")), PathBuf::from(format!("{stem}.rsds.json")))
}

/// Write traces to a `.rsds` internal dataset (`diskwrite`).
pub fn write_dataset(base_path: impl AsRef<Path>, name: impl Into<String>, sort_order: impl Into<String>, traces: &[Trace]) -> Result<()> {
    if traces.is_empty() {
        return Err(SeismicError::InvalidFormat("Cannot write empty dataset".into()));
    }
    let (bin_path, json_path) = paths_for(base_path);
    if let Some(parent) = bin_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let dt_us = traces[0].dt_us;
    let n_samples = traces[0].samples.len();

    let mut w = BufWriter::new(File::create(&bin_path)?);
    let mut headers = Vec::with_capacity(traces.len());
    for tr in traces {
        if tr.samples.len() != n_samples {
            return Err(SeismicError::DimensionMismatch(format!(
                "Expected {n_samples} samples, got {}",
                tr.samples.len()
            )));
        }
        for &s in &tr.samples {
            w.write_all(&s.to_le_bytes())?;
        }
        headers.push(tr.headers.clone());
    }
    w.flush()?;

    let meta = DatasetMeta {
        name: name.into(),
        dt_us,
        n_samples,
        n_traces: traces.len(),
        sort_order: sort_order.into(),
        headers,
    };
    fs::write(&json_path, serde_json::to_vec(&meta).map_err(|e| SeismicError::Serialization(e.to_string()))?)?;
    Ok(())
}

impl MmapDataset {
    pub fn open(base_path: impl AsRef<Path>) -> Result<Self> {
        let (bin_path, json_path) = paths_for(base_path);
        let meta_bytes = fs::read(&json_path)?;
        let meta: DatasetMeta =
            serde_json::from_slice(&meta_bytes).map_err(|e| SeismicError::Serialization(e.to_string()))?;
        let file = File::open(&bin_path)?;
        // SAFETY: read-only mapping of an immutable dataset file.
        let mmap = unsafe { Mmap::map(&file)? };
        let expected_bytes = meta.n_traces * meta.n_samples * 4;
        if mmap.len() < expected_bytes {
            return Err(SeismicError::InvalidFormat(format!(
                "Truncated .rsds.bin: {} < {}",
                mmap.len(),
                expected_bytes
            )));
        }
        Ok(Self { meta, mmap })
    }

    pub fn read_trace(&self, index: usize) -> Option<Trace> {
        if index >= self.meta.n_traces {
            return None;
        }
        let ns = self.meta.n_samples;
        let start = index * ns * 4;
        let slice = &self.mmap[start..start + ns * 4];
        let mut samples = Vec::with_capacity(ns);
        for chunk in slice.chunks_exact(4) {
            samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Some(Trace {
            dt_us: self.meta.dt_us,
            headers: self.meta.headers[index].clone(),
            samples,
        })
    }

    pub fn read_all(&self) -> Vec<Trace> {
        (0..self.meta.n_traces).filter_map(|i| self.read_trace(i)).collect()
    }
}
