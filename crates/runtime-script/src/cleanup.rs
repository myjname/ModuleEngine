//! Ownership-based cleanup. Never sweep unmarked pre-existing work or a shared
//! dependency cache. Completed failed candidates retain their source and log.
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MARKER: &str = "module-engine-candidate.json";
const FAILED_RETAIN: usize = 16;
static NEXT: AtomicU64 = AtomicU64::new(1);

type CleanupJob = Box<dyn FnOnce() + Send>;
struct CleanupQueue {
    sender: mpsc::Sender<CleanupJob>,
    pending: Arc<(Mutex<usize>, Condvar)>,
}
fn queue() -> &'static CleanupQueue {
    static QUEUE: OnceLock<CleanupQueue> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<CleanupJob>();
        let pending = Arc::new((Mutex::new(0), Condvar::new()));
        let worker_pending = pending.clone();
        std::thread::spawn(move || {
            while let Ok(job) = receiver.recv() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                let (count, changed) = &*worker_pending;
                let mut count = count
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *count -= 1;
                changed.notify_all();
            }
        });
        CleanupQueue { sender, pending }
    })
}
fn enqueue(job: impl FnOnce() + Send + 'static) {
    let queue = queue();
    let mut count = queue
        .pending
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *count += 1;
    if queue.sender.send(Box::new(job)).is_err() {
        *count -= 1;
    }
}

/// Call after all runtime owners/background compilers have stopped, off the UI
/// thread, before process exit. Dependency caches are never queued for deletion.
pub fn finish_artifact_cleanup(timeout: Duration) -> Result<(), String> {
    let queue = queue();
    let (count, changed) = &*queue.pending;
    let count = count
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (count, _) = changed
        .wait_timeout_while(count, timeout, |count| *count != 0)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *count == 0 {
        Ok(())
    } else {
        Err(format!(
            "{} runtime artifact cleanup jobs are still pending",
            *count
        ))
    }
}

fn fixtures() -> &'static Mutex<BTreeMap<PathBuf, Weak<TestArtifacts>>> {
    static FIXTURES: OnceLock<Mutex<BTreeMap<PathBuf, Weak<TestArtifacts>>>> = OnceLock::new();
    FIXTURES.get_or_init(Mutex::default)
}

pub(crate) fn fixture_for(path: &Path) -> Option<Arc<TestArtifacts>> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let entries = fixtures()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    entries
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.components().count())
        .and_then(|(_, owner)| owner.upgrade())
}

/// Owns a newly created test directory. Declare it before compilers/workers so
/// their normal drop order closes workers first. Compiler clones also retain a
/// lease, including when a background compile outlives its receiving UI.
/// Success removes the fixture; panic/preserve retains source and diagnostics,
/// removing only explicitly registered reproducible directories after workers.
pub struct TestArtifactGuard {
    inner: Arc<TestArtifacts>,
}

/// Retain a matching fresh test fixture during generic preparation work which
/// has no ScriptCompiler. Ordinary project directories return None.
#[derive(Clone, Debug)]
pub struct TestArtifactLease {
    _owner: Arc<TestArtifacts>,
}
pub fn retain_test_artifacts(path: &Path) -> Option<TestArtifactLease> {
    fixture_for(path).map(|owner| TestArtifactLease { _owner: owner })
}

#[derive(Debug)]
pub(crate) struct TestArtifacts {
    root: PathBuf,
    parent: PathBuf,
    failed: AtomicBool,
    targets: Mutex<Vec<PathBuf>>,
    _parent_scope: Option<Arc<TestArtifacts>>,
}

impl TestArtifactGuard {
    pub fn new(parent: &Path, label: &str) -> Result<Self, String> {
        if label.is_empty() || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("Test artifact label needs ASCII letters, numbers or hyphens".into());
        }
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let parent = parent.canonicalize().map_err(|error| error.to_string())?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let root = parent.join(format!(
            "{label}-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // create_dir, not create_dir_all: never adopt another run's artifacts.
        fs::create_dir(&root).map_err(|error| error.to_string())?;
        let inner = Arc::new(TestArtifacts {
            _parent_scope: fixture_for(&parent),
            root,
            parent,
            failed: AtomicBool::new(false),
            targets: Mutex::new(vec![]),
        });
        fixtures()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(inner.root.clone(), Arc::downgrade(&inner));
        Ok(Self { inner })
    }

    pub fn path(&self) -> &Path {
        &self.inner.root
    }

    /// Shared dependency targets must never be registered here. Only a new,
    /// reproducible child of this guard's fresh fixture may be trimmed on failure.
    pub fn reproducible_target(&self, relative: &str) -> Result<PathBuf, String> {
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("Reproducible test target must be a child without traversal".into());
        }
        let path = self.inner.root.join(relative);
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        let path = path.canonicalize().map_err(|error| error.to_string())?;
        if !path.starts_with(&self.inner.root) || path == self.inner.root {
            return Err("Reproducible target escapes its owned test fixture".into());
        }
        self.inner
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(path.clone());
        Ok(path)
    }

    pub fn preserve(&self, reason: &str) {
        self.inner.failed.store(true, Ordering::Release);
        let _ = fs::write(self.path().join("test-failure.txt"), reason);
    }
}

impl Drop for TestArtifactGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.preserve("Test panicked; source and diagnostics retained.\n");
        }
        // This helper runs only in tests; production drops merely enqueue IO.
        let _ = finish_artifact_cleanup(Duration::from_secs(5));
    }
}

impl Drop for TestArtifacts {
    fn drop(&mut self) {
        fixtures()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.root);
        if self.failed.load(Ordering::Acquire) || std::thread::panicking() {
            for path in self
                .targets
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                remove_or_defer(path.clone(), self.root.clone());
            }
        } else {
            remove_or_defer(self.root.clone(), self.parent.clone());
        }
    }
}

#[derive(Debug)]
pub(crate) struct CandidateArtifacts {
    pub(crate) directory: PathBuf,
    parent: PathBuf,
    target: PathBuf,
    package: String,
    ready: AtomicBool,
    keep_diagnostics: AtomicBool,
    cleanup_queued: bool,
    _fixture: Option<Arc<TestArtifacts>>,
}

impl CandidateArtifacts {
    pub(crate) fn create(
        work: &Path,
        target: &Path,
        package: &str,
        fixture: Option<Arc<TestArtifacts>>,
    ) -> Result<Self, String> {
        fs::create_dir_all(work).map_err(|error| error.to_string())?;
        let parent = work.canonicalize().map_err(|error| error.to_string())?;
        let directory = parent.join(package);
        fs::create_dir(&directory).map_err(|error| error.to_string())?;
        fs::create_dir_all(target).map_err(|error| error.to_string())?;
        let target = target.canonicalize().map_err(|error| error.to_string())?;
        let this = Self {
            directory,
            parent,
            target,
            package: package.into(),
            ready: AtomicBool::new(false),
            keep_diagnostics: AtomicBool::new(false),
            cleanup_queued: false,
            _fixture: fixture,
        };
        this.mark("building");
        Ok(this)
    }

    fn mark(&self, state: &str) {
        let completed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|time| time.as_nanos().to_string())
            .unwrap_or_default();
        let _ = fs::write(self.directory.join(MARKER), serde_json::to_vec(&serde_json::json!({"owner":"module-engine-script/1","state":state,"completed_unix_nanos":completed})).unwrap_or_default());
    }

    pub(crate) fn log(&self, phase: &str, stdout: &str, stderr: &str) {
        use std::io::Write;
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.directory.join("build.log"))
        {
            let _ = writeln!(
                file,
                "\n{phase}\nstdout:\n{}\nstderr:\n{}",
                bounded(stdout),
                bounded(stderr)
            );
        }
    }

    pub(crate) fn failed(&self, error: &str) {
        self.keep_diagnostics.store(true, Ordering::Release);
        let _ = fs::write(self.directory.join("failure.txt"), bounded(error));
    }

    pub(crate) fn ready(&self) {
        self.ready.store(true, Ordering::Release);
        self.mark("ready");
    }

    /// A successful worker runs its own copied executable. Only unique worker
    /// package products are removed; dependencies and other candidates survive.
    pub(crate) fn trim_build_outputs(&self) {
        if self.target.starts_with(&self.directory) && self.target != self.directory {
            remove_or_defer(self.target.clone(), self.directory.clone());
        } else {
            trim_package(&self.target, &self.package);
        }
    }
}

fn bounded(text: &str) -> &str {
    let mut end = text.len().min(256 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

impl Drop for CandidateArtifacts {
    fn drop(&mut self) {
        if self.cleanup_queued {
            return;
        }
        let directory = self.directory.clone();
        let parent = self.parent.clone();
        let target = self.target.clone();
        let package = self.package.clone();
        let ready = self.ready.load(Ordering::Acquire);
        let keep = self.keep_diagnostics.load(Ordering::Acquire);
        let fixture = self._fixture.clone();
        enqueue(move || {
            let retired = CandidateArtifacts {
                directory,
                parent,
                target,
                package,
                ready: AtomicBool::new(ready),
                keep_diagnostics: AtomicBool::new(keep),
                cleanup_queued: true,
                _fixture: fixture,
            };
            retired.cleanup();
        });
    }
}

impl CandidateArtifacts {
    fn cleanup(&self) {
        self.trim_build_outputs();
        if self.ready.load(Ordering::Acquire) && !self.keep_diagnostics.load(Ordering::Acquire) {
            if self._fixture.is_some() {
                // The test's outcome, known only when its guard drops, decides
                // whether source/log should survive. Its executable is rebuildable.
                for file in ["worker.exe", "worker"] {
                    remove_or_defer(self.directory.join(file), self.directory.clone());
                }
                self.mark("released");
            } else {
                self.mark("released");
                remove_or_defer(self.directory.clone(), self.parent.clone());
            }
        } else {
            for file in ["worker.exe", "worker"] {
                remove_or_defer(self.directory.join(file), self.directory.clone());
            }
            self.mark("failed");
            prune_failed(&self.parent);
        }
    }
}

fn trim_package(target: &Path, package: &str) {
    for relative in [
        "debug",
        "debug/deps",
        "debug/.fingerprint",
        "debug/incremental",
    ] {
        let folder = target.join(relative);
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name
                .strip_prefix(package)
                .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('-'))
            {
                remove_or_defer(entry.path(), folder.clone());
            }
        }
    }
}

fn prune_failed(parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let mut failed = entries
        .flatten()
        .filter_map(|entry| {
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("module_engine_worker_")
                || !entry.file_type().ok()?.is_dir()
            {
                return None;
            }
            let marker: serde_json::Value =
                serde_json::from_slice(&fs::read(entry.path().join(MARKER)).ok()?).ok()?;
            if marker["owner"] != "module-engine-script/1" || marker["state"] != "failed" {
                return None;
            }
            let completed = marker["completed_unix_nanos"]
                .as_str()?
                .parse::<u128>()
                .ok()?;
            Some((completed, entry.path()))
        })
        .collect::<Vec<_>>();
    failed.sort_by(|a, b| b.cmp(a));
    for (_, directory) in failed.into_iter().skip(FAILED_RETAIN) {
        remove_or_defer(directory, parent.to_path_buf());
    }
}

fn checked_remove(path: &Path, parent: &Path) -> std::io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let parent = parent.canonicalize()?;
    let resolved = path.canonicalize()?;
    if !resolved.starts_with(&parent) || resolved == parent {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Cleanup refused a path outside its owned parent",
        ));
    }
    // A generated worker can create arbitrary files, including junctions.
    // Inspect every descendant without following links before any deletion.
    // Worker owners have already stopped, so their tree is no longer mutating.
    refuse_reparse_tree(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn refuse_reparse_tree(root: &Path) -> std::io::Result<()> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        if is_reparse(&metadata) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "Cleanup retained a tree containing a symlink/junction/reparse point: {}",
                    path.display()
                ),
            ));
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(())
}

fn remove_or_defer(path: PathBuf, parent: PathBuf) {
    for delay in [0, 10, 30] {
        if delay != 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        match checked_remove(&path, &parent) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                eprintln!("{error}");
                return;
            }
            Err(_) => {}
        }
    }
    // Antivirus/process handle release can lag on Windows. Do not panic or
    // block an editor for seconds. The bounded retry remains ownership-checked.
    enqueue(move || {
        for delay in [100, 250, 1000] {
            std::thread::sleep(Duration::from_millis(delay));
            match checked_remove(&path, &parent) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                    eprintln!("{error}");
                    return;
                }
                Err(_) => {}
            }
        }
        eprintln!(
            "Retained locked runtime artifact for later cleanup: {}",
            path.display()
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_waits_for_background_owner_and_preserves_failure_diagnostics() {
        let guard = TestArtifactGuard::new(&std::env::temp_dir(), "module-cleanup").unwrap();
        let root = guard.path().to_path_buf();
        let lease = fixture_for(&root).unwrap();
        fs::write(root.join("source.rs"), "source diagnostics").unwrap();
        drop(guard);
        assert!(root.exists());
        drop(lease);
        assert!(!root.exists());

        let outer = TestArtifactGuard::new(&std::env::temp_dir(), "module-cleanup-outer").unwrap();
        let guard = TestArtifactGuard::new(outer.path(), "failed").unwrap();
        let root = guard.path().to_path_buf();
        let target = guard.reproducible_target("target").unwrap();
        fs::write(
            target.join("rebuildable.bin"),
            "large reproducible artifact",
        )
        .unwrap();
        fs::write(root.join("source.rs"), "failing source").unwrap();
        let lease = fixture_for(&root).unwrap();
        guard.preserve("failed semantic assertion");
        drop(guard);
        assert!(target.exists());
        drop(lease);
        assert!(!target.exists());
        assert_eq!(
            fs::read_to_string(root.join("source.rs")).unwrap(),
            "failing source"
        );
        assert!(root.join("test-failure.txt").is_file());
    }

    #[test]
    fn failed_retention_only_prunes_completed_owned_candidates_and_keeps_shared_dependencies() {
        let guard = TestArtifactGuard::new(&std::env::temp_dir(), "module-candidates").unwrap();
        let work = guard.path().join("candidates");
        let target = guard.path().join("shared-target");
        fs::create_dir_all(target.join("debug/deps")).unwrap();
        fs::write(target.join("debug/deps/serde-retained.rlib"), "dependency").unwrap();
        let active = Arc::new(
            CandidateArtifacts::create(&work, &target, "module_engine_worker_active", None)
                .unwrap(),
        );
        active.ready();
        active.failed("runtime failure while the accepted candidate still owns its binary");
        fs::write(
            active.directory.join("worker.exe"),
            "paused active executable",
        )
        .unwrap();
        fs::write(active.directory.join("source.rs"), "last accepted source").unwrap();
        let clone = active.clone();
        let active_path = active.directory.clone();
        drop(active);
        assert!(active_path.exists());
        assert!(active_path.join("worker.exe").is_file());
        fs::create_dir_all(work.join("module_engine_worker_unmarked")).unwrap();
        for index in 0..20 {
            let candidate = CandidateArtifacts::create(
                &work,
                &target,
                &format!("module_engine_worker_failed_{index}"),
                None,
            )
            .unwrap();
            candidate.failed("compile diagnostics");
            fs::write(candidate.directory.join("source.rs"), "bad source").unwrap();
        }
        finish_artifact_cleanup(Duration::from_secs(5)).unwrap();
        assert!(active_path.exists());
        assert!(active_path.join("worker.exe").is_file());
        assert!(active_path.join("source.rs").is_file());
        assert!(work.join("module_engine_worker_unmarked").is_dir());
        assert!(target.join("debug/deps/serde-retained.rlib").is_file());
        assert_eq!(
            fs::read_dir(&work)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    fs::read(entry.path().join(MARKER))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                        .is_some_and(|marker| marker["state"] == "failed")
                })
                .count(),
            FAILED_RETAIN
        );
        assert!(!work.join("module_engine_worker_failed_0").exists());
        assert!(work.join("module_engine_worker_failed_19").is_dir());
        drop(clone);
        finish_artifact_cleanup(Duration::from_secs(5)).unwrap();
        assert!(!active_path.join("worker.exe").exists());
        assert_eq!(
            fs::read_to_string(active_path.join("source.rs")).unwrap(),
            "last accepted source"
        );
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(active_path.join(MARKER)).unwrap()).unwrap();
        assert_eq!(
            marker["state"], "failed",
            "failure becomes eligible only after the final lease closes"
        );
        assert!(guard.reproducible_target("../outside").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn nested_windows_junction_refuses_cleanup_without_changing_external_data() {
        use std::os::windows::process::CommandExt;
        let external = TestArtifactGuard::new(&std::env::temp_dir(), "module-external").unwrap();
        fs::write(external.path().join("keep.txt"), "external data").unwrap();
        let guard = TestArtifactGuard::new(&std::env::temp_dir(), "module-reparse").unwrap();
        let owned = guard.path().join("owned");
        fs::create_dir(&owned).unwrap();
        fs::write(owned.join("source.rs"), "retained source").unwrap();
        let junction = owned.join("junction");
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(external.path())
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let error = checked_remove(&owned, guard.path()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("reparse point"));
        assert_eq!(
            fs::read_to_string(external.path().join("keep.txt")).unwrap(),
            "external data"
        );
        assert!(
            owned.join("source.rs").is_file(),
            "preflight must refuse before any deletion"
        );
        assert!(junction.starts_with(guard.path()));
        fs::remove_dir(&junction).unwrap(); // remove the junction itself, never recursively
        checked_remove(&owned, guard.path()).unwrap();
        assert_eq!(
            fs::read_to_string(external.path().join("keep.txt")).unwrap(),
            "external data"
        );
    }
}
