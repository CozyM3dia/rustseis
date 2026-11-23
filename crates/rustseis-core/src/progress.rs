use crate::{Result, SeismicError};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Lock-free cooperative cancellation flag and live progress reporter shared between
/// long-running imaging/flow workers (PSDM, 2D RTM, full-line flows) and the UI/CLI.
#[derive(Debug, Clone)]
pub struct JobControl {
    inner: Arc<JobControlInner>,
}

#[derive(Debug)]
struct JobControlInner {
    cancelled: AtomicBool,
    progress_bp: AtomicU32,
    stage: Mutex<String>,
}

impl Default for JobControl {
    fn default() -> Self {
        Self::new()
    }
}

impl JobControl {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(JobControlInner {
                cancelled: AtomicBool::new(false),
                progress_bp: AtomicU32::new(0),
                stage: Mutex::new(String::new()),
            }),
        }
    }

    pub fn reset(&self) {
        self.inner.cancelled.store(false, Ordering::SeqCst);
        self.inner.progress_bp.store(0, Ordering::Relaxed);
        if let Ok(mut s) = self.inner.stage.lock() {
            s.clear();
        }
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(SeismicError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn set_progress(&self, frac: f32, stage: impl Into<String>) {
        let bp = (frac.clamp(0.0, 1.0) * 10_000.0).round() as u32;
        self.inner.progress_bp.store(bp, Ordering::Relaxed);
        if let Ok(mut s) = self.inner.stage.lock() {
            *s = stage.into();
        }
    }

    pub fn progress(&self) -> f32 {
        self.inner.progress_bp.load(Ordering::Relaxed) as f32 / 10_000.0
    }

    pub fn snapshot(&self) -> (f32, String) {
        let frac = self.progress();
        let msg = self
            .inner
            .stage
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        (frac, msg)
    }
}
