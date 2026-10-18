//! Progress reporting and cooperative cancellation for long-running work.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Shared between the UI and a background job. Cheap to clone.
#[derive(Clone, Default)]
pub struct JobCtx {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    cancel: AtomicBool,
    /// Progress in 1/10000ths.
    progress: AtomicU32,
    stage: std::sync::Mutex<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}
impl std::error::Error for Cancelled {}

impl JobCtx {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.inner.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancel.load(Ordering::Relaxed)
    }
    /// Returns `Err(Cancelled)` once cancellation was requested; use with `?` in loops.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() { Err(Cancelled) } else { Ok(()) }
    }
    pub fn set_progress(&self, fraction: f32) {
        self.inner.progress.store((fraction.clamp(0.0, 1.0) * 10000.0) as u32, Ordering::Relaxed);
    }
    pub fn progress(&self) -> f32 {
        self.inner.progress.load(Ordering::Relaxed) as f32 / 10000.0
    }
    pub fn set_stage(&self, stage: impl Into<String>) {
        *self.inner.stage.lock().unwrap() = stage.into();
    }
    pub fn stage(&self) -> String {
        self.inner.stage.lock().unwrap().clone()
    }
    /// Maps sub-step progress `i/n` into the `[from, to]` range of the overall bar.
    pub fn step(&self, from: f32, to: f32, i: usize, n: usize) {
        self.set_progress(from + (to - from) * (i as f32 / n.max(1) as f32));
    }
}
