//! Owned producer retirement. Render/Drop never joins these GPU waits.
use std::{
    sync::{Mutex, OnceLock},
    thread::JoinHandle,
};

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ProducerRetirementReport {
    pub registered_jobs: usize,
    pub joined_jobs: usize,
    pub completed_submissions: usize,
    pub unconfirmed_submissions: usize,
    pub failed_jobs: usize,
}

#[derive(Default)]
struct Registry {
    jobs: Vec<JoinHandle<Result<(), String>>>,
    report: ProducerRetirementReport,
    errors: Vec<String>,
}
fn registry() -> &'static Mutex<Registry> {
    static JOBS: OnceLock<Mutex<Registry>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(Registry::default()))
}
fn collect(job: JoinHandle<Result<(), String>>, state: &mut Registry) {
    state.report.joined_jobs += 1;
    match job.join() {
        Ok(Ok(())) => state.report.completed_submissions += 1,
        outcome => {
            state.report.failed_jobs += 1;
            state.report.unconfirmed_submissions += 1;
            if state.errors.len() < 8 {
                state.errors.push(match outcome {
                    Ok(Err(error)) => error,
                    _ => "GPU producer retirement thread panicked; resources remain pinned".into(),
                });
            }
        }
    }
}
pub(crate) fn retire(job: impl FnOnce() -> Result<(), String> + Send + 'static) {
    // The closure owns a ManuallyDrop bundle: failure to spawn or a panic must
    // leak the native work and its device pin rather than free busy resources.
    let owner = std::thread::Builder::new()
        .name("gpu-producer-retirement".into())
        .spawn(job);
    let mut state = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut index = 0;
    while index < state.jobs.len() {
        if state.jobs[index].is_finished() {
            let job = state.jobs.swap_remove(index);
            collect(job, &mut state);
        } else {
            index += 1;
        }
    }
    state.report.registered_jobs += 1;
    match owner {
        Ok(job) => state.jobs.push(job),
        Err(error) => {
            state.report.failed_jobs += 1;
            state.report.unconfirmed_submissions += 1;
            if state.errors.len() < 8 {
                state.errors.push(format!(
                    "GPU producer retirement could not start; resources remain pinned: {error}"
                ));
            }
        }
    }
}

/// Application shutdown only, after all renderer owners have been dropped.
/// Each background job waits at most 30 seconds for its own producer fence.
pub fn finish_producer_retirements() -> (ProducerRetirementReport, Vec<String>) {
    let mut result = Registry::default();
    loop {
        let batch = std::mem::take(
            &mut *registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        result.report.registered_jobs += batch.report.registered_jobs;
        result.report.joined_jobs += batch.report.joined_jobs;
        result.report.completed_submissions += batch.report.completed_submissions;
        result.report.unconfirmed_submissions += batch.report.unconfirmed_submissions;
        result.report.failed_jobs += batch.report.failed_jobs;
        result
            .errors
            .extend(batch.errors.into_iter().take(8 - result.errors.len()));
        if batch.jobs.is_empty() {
            break;
        }
        for job in batch.jobs {
            collect(job, &mut result);
        }
    }
    (result.report, result.errors)
}
