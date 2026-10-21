//! Background jobs: one at a time, with a stage line, cancel and panic capture.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Instant;

use eframe::egui;
use s3d_core::job::JobCtx;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Import,
    Hollow,
    Split,
    Features,
    Edit,
    Arrange,
    Export,
    Save,
    Open,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            JobKind::Import => "Importing",
            JobKind::Hollow => "Hollowing",
            JobKind::Split => "Splitting",
            JobKind::Features => "Adding features",
            JobKind::Edit => "Applying edit",
            JobKind::Arrange => "Arranging beds",
            JobKind::Export => "Exporting",
            JobKind::Save => "Saving",
            JobKind::Open => "Opening project",
        }
    }
}

pub struct Running<T> {
    pub kind: JobKind,
    pub ctx: JobCtx,
    pub started: Instant,
    rx: Receiver<Result<T, String>>,
}

pub struct Jobs<T> {
    pub current: Option<Running<T>>,
}

impl<T> Default for Jobs<T> {
    fn default() -> Self {
        Jobs { current: None }
    }
}

impl<T: Send + 'static> Jobs<T> {
    pub fn busy(&self) -> bool {
        self.current.is_some()
    }

    /// Starts `work` on a background thread. Any running job is cancelled.
    pub fn start(&mut self, kind: JobKind, egui_ctx: &egui::Context, work: impl FnOnce(&JobCtx) -> Result<T, String> + Send + 'static) {
        self.cancel();
        let ctx = JobCtx::new();
        let (tx, rx) = channel();
        let job_ctx = ctx.clone();
        let repaint = egui_ctx.clone();
        std::thread::Builder::new()
            .name(format!("job-{kind:?}"))
            .spawn(move || {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&job_ctx)));
                let r = match r {
                    Ok(r) => r,
                    Err(p) => Err(format!(
                        "internal error: {}",
                        p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
                    )),
                };
                let _ = tx.send(r);
                repaint.request_repaint();
            })
            .expect("spawn job thread");
        self.current = Some(Running { kind, ctx, started: Instant::now(), rx });
    }

    pub fn cancel(&mut self) {
        if let Some(r) = self.current.take() {
            r.ctx.cancel();
        }
    }

    /// Returns a finished job's result. Results of cancelled jobs are dropped.
    pub fn poll(&mut self) -> Option<(JobKind, Result<T, String>)> {
        let r = self.current.as_ref()?;
        match r.rx.try_recv() {
            Ok(res) => {
                let kind = r.kind;
                let cancelled = r.ctx.is_cancelled();
                self.current = None;
                if cancelled { None } else { Some((kind, res)) }
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                let kind = r.kind;
                self.current = None;
                Some((kind, Err("job stopped unexpectedly".into())))
            }
        }
    }
}
