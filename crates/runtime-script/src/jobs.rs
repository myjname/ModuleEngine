//! Owned background work. Dropping a UI lease only requests cancellation;
//! shutdown and the reaper join threads away from the UI.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

/// Nonblocking ownership of a background job. Cancellation is advisory: the
/// closure always runs, including a cleanup closure whose lease is discarded.
#[derive(Debug)]
pub struct JobLease {
    cancelled: Arc<AtomicBool>,
}
impl JobLease {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
impl Drop for JobLease {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Job {
    handle: JoinHandle<()>,
    cancelled: Arc<AtomicBool>,
}
#[derive(Default)]
struct State {
    jobs: BTreeMap<u64, Job>,
    reaping: usize,
    errors: Vec<String>,
}
struct Registry {
    state: Arc<(Mutex<State>, Condvar)>,
    completed: mpsc::Sender<u64>,
    next: AtomicU64,
    finishing: Mutex<()>,
}
impl Registry {
    fn new() -> Self {
        let state = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let (completed, receiver) = mpsc::channel::<u64>();
        let reaper_state = state.clone();
        thread::spawn(move || {
            while let Ok(id) = receiver.recv() {
                let job = {
                    let mut state = reaper_state
                        .0
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let job = state.jobs.remove(&id);
                    if job.is_some() {
                        state.reaping += 1;
                    }
                    job
                };
                if let Some(job) = job {
                    let error = job.handle.join().err().map(panic_message);
                    let mut state = reaper_state
                        .0
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(error) = error {
                        state.errors.push(error);
                    }
                    state.reaping -= 1;
                    reaper_state.1.notify_all();
                }
            }
        });
        Self {
            state,
            completed,
            next: AtomicU64::new(1),
            finishing: Mutex::new(()),
        }
    }

    fn spawn<T: Send + 'static>(
        &self,
        run: impl FnOnce(&AtomicBool) -> T + Send + 'static,
    ) -> (mpsc::Receiver<T>, JobLease) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        let job_cancelled = cancelled.clone();
        let completed = self.completed.clone();
        let (result, receiver) = mpsc::channel();
        let (start, started) = mpsc::channel();
        let mut state = self
            .state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let handle = thread::spawn(move || {
            // Publish the handle before work can complete; the reaper can never
            // lose a completion racing registration. Unbounded send stays fast.
            let _ = started.recv();
            let _completion = Completion {
                id,
                sender: completed,
            };
            let value = run(&job_cancelled);
            let _ = result.send(value); // abandoned results drop on this thread
        });
        state.jobs.insert(
            id,
            Job {
                handle,
                cancelled: cancelled.clone(),
            },
        );
        self.state.1.notify_all();
        let _ = start.send(());
        drop(state);
        (receiver, JobLease { cancelled })
    }

    fn finish(&self) -> Result<(), String> {
        let _finishing = self
            .finishing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            let jobs = {
                let (state, changed) = &*self.state;
                let mut state = state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while state.jobs.is_empty() && state.reaping != 0 {
                    state = changed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                if state.jobs.is_empty() {
                    let errors = std::mem::take(&mut state.errors);
                    return if errors.is_empty() {
                        Ok(())
                    } else {
                        Err(errors.join("; "))
                    };
                }
                for job in state.jobs.values() {
                    job.cancelled.store(true, Ordering::Release);
                }
                std::mem::take(&mut state.jobs)
            };
            let errors = jobs
                .into_values()
                .filter_map(|job| job.handle.join().err().map(panic_message))
                .collect::<Vec<_>>();
            self.state
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .errors
                .extend(errors);
            // A job can own another preparation/drop job. Inspect the registry
            // again after joins rather than trusting a single initial snapshot.
        }
    }
}

struct Completion {
    id: u64,
    sender: mpsc::Sender<u64>,
}
impl Drop for Completion {
    fn drop(&mut self) {
        let _ = self.sender.send(self.id);
    }
}
fn panic_message(error: Box<dyn std::any::Any + Send>) -> String {
    let detail = error
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| error.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic");
    format!("Background job panicked: {detail}")
}
fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Registry::new)
}

pub fn spawn_job<T: Send + 'static>(
    run: impl FnOnce(&AtomicBool) -> T + Send + 'static,
) -> (mpsc::Receiver<T>, JobLease) {
    registry().spawn(run)
}

/// Shutdown boundary only: stop game/renderer owners first, then call this off
/// the UI thread, then drain artifact cleanup. Every registered job is joined.
/// GPU jobs without a cooperative cancellation seam are still awaited.
pub fn finish_background_jobs() -> Result<(), String> {
    registry().finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn lease_drop_never_joins_and_final_drain_joins_cancelled_work() {
        let registry = Registry::new();
        let (started, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (result, lease) = registry.spawn(move |cancelled| {
            started.send(()).unwrap();
            released.recv().unwrap();
            cancelled.load(Ordering::Acquire)
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        // Release even a regressed blocking Drop after a bounded observation,
        // so a failure reports an assertion instead of hanging the test suite.
        let (dropped, observed_drop) = mpsc::channel();
        let dropper = thread::spawn(move || {
            drop(lease);
            let _ = dropped.send(());
        });
        let nonblocking = observed_drop.recv_timeout(Duration::from_secs(2));
        let pending = matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty));
        release.send(()).unwrap();
        dropper.join().unwrap();
        registry.finish().unwrap();
        assert!(nonblocking.is_ok(), "lease Drop waited for the blocked job");
        assert!(pending);
        assert!(result.recv_timeout(Duration::from_secs(2)).unwrap());
        let state = registry.state.0.lock().unwrap();
        assert!(state.jobs.is_empty() && state.reaping == 0);
    }

    #[test]
    fn abandoned_result_is_dropped_on_background_thread_and_completed_handles_are_reaped() {
        struct DropThread(mpsc::Sender<thread::ThreadId>);
        impl Drop for DropThread {
            fn drop(&mut self) {
                let _ = self.0.send(thread::current().id());
            }
        }
        let registry = Registry::new();
        let caller = thread::current().id();
        let (dropped, observed) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (receiver, lease) = registry.spawn(move |_| {
            released.recv().unwrap();
            DropThread(dropped)
        });
        drop(receiver);
        drop(lease); // advisory cancellation must not skip cleanup work
        release.send(()).unwrap();
        assert_ne!(
            observed.recv_timeout(Duration::from_secs(2)).unwrap(),
            caller
        );
        let state = registry.state.0.lock().unwrap();
        let (state, _) = registry
            .state
            .1
            .wait_timeout_while(state, Duration::from_secs(2), |state| {
                !state.jobs.is_empty() || state.reaping != 0
            })
            .unwrap();
        assert!(
            state.jobs.is_empty() && state.reaping == 0,
            "completed handles must reap before shutdown"
        );
        drop(state);
        registry.finish().unwrap();
        assert!(registry.state.0.lock().unwrap().jobs.is_empty());
    }

    #[test]
    fn final_job_drain_stops_real_child_before_releasing_its_test_source_directory() {
        let registry = Registry::new();
        let artifacts =
            crate::TestArtifactGuard::new(&std::env::temp_dir(), "module-background-process")
                .unwrap();
        let root = artifacts.path().to_path_buf();
        let started = root.join("started");
        let compiler = crate::ScriptCompiler::new(root.clone(), root.join("compile"));
        std::fs::write(
            root.join("source.rs"),
            "retained until the subprocess exits",
        )
        .unwrap();
        let child_root = root.clone();
        let (result, lease) = registry.spawn(move |cancelled| {
            let _compiler_scope = compiler;
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "tests::cancelled_compilation_creates_no_candidate_and_owned_process_cancellation_waits_for_exit", "--nocapture"])
                .env("MODULE_ENGINE_CLEANUP_CHILD", child_root)
                .env("MODULE_ENGINE_CLEANUP_CHILD_DELAY_MS", "30000");
            crate::run_bounded(command, vec![], Duration::from_secs(30), cancelled, None).map(|_| ())
        });
        let start = std::time::Instant::now();
        while !started.is_file() && start.elapsed() < Duration::from_secs(2) {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            started.is_file(),
            "the registered child must actually start"
        );
        drop(artifacts);
        assert!(
            root.join("source.rs").is_file(),
            "background compiler must retain its fixture lease"
        );
        drop(lease);
        registry.finish().unwrap();
        assert!(
            matches!(result.recv_timeout(Duration::from_secs(2)).unwrap(), Err(crate::ScriptError::Worker(error)) if error.contains("cancelled"))
        );
        crate::finish_artifact_cleanup(Duration::from_secs(5)).unwrap();
        assert!(
            !root.exists(),
            "successful fixture must remove only after the child and compiler close"
        );
    }
}
