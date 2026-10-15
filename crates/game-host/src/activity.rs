//! Observable host work, independent of the UI.
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug)]
pub struct ActivitySnapshot {
    pub title: String,
    pub detail: String,
    pub elapsed: Duration,
    pub state: ActivityState,
    pub error: Option<String>,
}

#[derive(Debug)]
struct ActivityRecord {
    title: String,
    detail: String,
    started: Instant,
    finished: Option<Instant>,
    state: ActivityState,
    error: Option<String>,
}

/// Clone one handle into a worker and retain another in the UI owner.
/// Use a new handle for a new request; owners decide how long to retain results.
#[derive(Clone, Debug)]
pub struct BuildProgress {
    inner: Arc<Mutex<ActivityRecord>>,
}
impl BuildProgress {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ActivityRecord {
                title: title.into(),
                detail: "Ожидание запуска".into(),
                started: Instant::now(),
                finished: None,
                state: ActivityState::Running,
                error: None,
            })),
        }
    }
    fn record(&self) -> MutexGuard<'_, ActivityRecord> {
        // Reporting a failed worker must remain possible after a poisoned lock.
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    /// Update an ongoing task. A late stage cannot restart a completed task.
    pub fn stage(&self, detail: impl Into<String>) {
        let mut record = self.record();
        if record.state == ActivityState::Running {
            record.detail = detail.into();
        }
    }
    /// Mark success only after the owner has accepted/applied the result.
    pub fn succeed(&self, detail: impl Into<String>) {
        let mut record = self.record();
        if record.state == ActivityState::Running {
            record.detail = detail.into();
            record.finished = Some(Instant::now());
            record.state = ActivityState::Succeeded;
            record.error = None;
        }
    }
    /// A final acceptance error may replace an earlier compilation success.
    /// Later success/stage messages cannot hide this error.
    pub fn fail(&self, error: impl Into<String>) {
        let mut record = self.record();
        record.error = Some(error.into());
        if record.state != ActivityState::Failed {
            record.finished = Some(Instant::now());
        }
        record.state = ActivityState::Failed;
    }
    pub fn snapshot(&self) -> ActivitySnapshot {
        let record = self.record();
        ActivitySnapshot {
            title: record.title.clone(),
            detail: record.detail.clone(),
            elapsed: record
                .finished
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(record.started),
            state: record.state,
            error: record.error.clone(),
        }
    }
}
