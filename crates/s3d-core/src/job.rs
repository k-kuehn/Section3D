//! Stage reporting and cooperative cancellation for long-running work.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared between the UI and a background job. Cheap to clone.
#[derive(Clone, Default)]
pub struct JobCtx {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    cancel: AtomicBool,
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
    pub fn set_stage(&self, stage: impl Into<String>) {
        *self.inner.stage.lock().unwrap() = stage.into();
    }
    pub fn stage(&self) -> String {
        self.inner.stage.lock().unwrap().clone()
    }
}
