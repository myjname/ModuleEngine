//! Persisted SDK-only output receipts. A namespace lock spans verification,
//! Cargo and receipt publication; immutable worker instances never retain it.
use crate::{
    freshness::{self, PreparedInputs},
    *,
};
use std::collections::BTreeMap;
const RECEIPT: &str = "module-engine-sdk-outputs.json";
const LOCK: &str = "module-engine-sdk.lock";
const LIMIT: u64 = 2 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u32,
    sdk_key: String,
    target: PathBuf,
    files: BTreeMap<PathBuf, String>,
}
pub(crate) struct Lease {
    directory: PathBuf,
    _lock: fs::File,
    baseline: std::cell::RefCell<Option<BTreeMap<PathBuf, String>>>,
}
pub(crate) enum Publication {
    Published,
    Unavailable,
    Changed,
}
impl Lease {
    pub(crate) fn acquire(
        directory: &Path,
        cancel: &AtomicBool,
        timeout: Duration,
    ) -> Result<Option<Self>, ScriptError> {
        let path = directory.join(LOCK);
        if path
            .symlink_metadata()
            .ok()
            .is_some_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            return Ok(None);
        }
        let file = match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
        {
            Ok(file) => file,
            Err(_) => return Ok(None),
        };
        let started = Instant::now();
        loop {
            check(cancel)?;
            match file.try_lock() {
                Ok(()) => {
                    return Ok(Some(Self {
                        directory: directory.into(),
                        _lock: file,
                        baseline: std::cell::RefCell::new(None),
                    }));
                }
                Err(fs::TryLockError::WouldBlock) if started.elapsed() < timeout => {
                    thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return Ok(None),
            }
        }
    }
    pub(crate) fn verify(
        &self,
        prepared: &PreparedInputs,
        cancel: &AtomicBool,
        allow_empty: bool,
    ) -> Result<bool, ScriptError> {
        check(cancel)?;
        let path = self.directory.join(RECEIPT);
        if !path.exists() {
            let empty = allow_empty
                && fs::read_dir(&self.directory).ok().is_some_and(|entries| {
                    entries.into_iter().all(|entry| {
                        entry.ok().is_some_and(|entry| {
                            matches!(
                                entry.file_name().to_str(),
                                Some("module-engine-sdk-cache.txt" | LOCK)
                            )
                        })
                    })
                });
            return Ok(empty);
        }
        let Some(receipt) = read_receipt(&path) else {
            return Ok(false);
        };
        if receipt.schema != 1
            || receipt.sdk_key != prepared.sdk_key
            || receipt.target != self.directory
        {
            return Ok(false);
        }
        let current = freshness::sdk_outputs(prepared, &self.directory, cancel)?;
        let valid = current.is_ok_and(|files| files == receipt.files);
        if valid {
            *self.baseline.borrow_mut() = Some(receipt.files);
        }
        Ok(valid)
    }
    pub(crate) fn publish(
        &self,
        prepared: &PreparedInputs,
        cancel: &AtomicBool,
    ) -> Result<Publication, ScriptError> {
        check(cancel)?;
        let files = match freshness::sdk_outputs(prepared, &self.directory, cancel)? {
            Ok(files) => files,
            Err(_) => {
                return Ok(if self.baseline.borrow().is_some() {
                    Publication::Changed
                } else {
                    Publication::Unavailable
                });
            }
        };
        if files.is_empty() {
            return Ok(Publication::Unavailable);
        }
        if self
            .baseline
            .borrow()
            .as_ref()
            .is_some_and(|before| before != &files)
        {
            return Ok(Publication::Changed);
        }
        let bytes = serde_json::to_vec(&Receipt {
            schema: 1,
            sdk_key: prepared.sdk_key.clone(),
            target: self.directory.clone(),
            files,
        })
        .map_err(|e| ScriptError::Setup(e.to_string()))?;
        if bytes.len() as u64 > LIMIT {
            return Ok(Publication::Unavailable);
        }
        let path = self.directory.join(format!(
            ".sdk-receipt-{}-{}",
            std::process::id(),
            NEXT_WORKER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(_) => return Ok(Publication::Unavailable),
        };
        let mut temporary = Temporary { path, moved: false };
        if file
            .write_all(&bytes)
            .and_then(|_| file.sync_all())
            .is_err()
        {
            return Ok(Publication::Unavailable);
        }
        drop(file);
        check(cancel)?;
        if fs::rename(&temporary.path, self.directory.join(RECEIPT)).is_err() {
            return Ok(Publication::Unavailable);
        }
        temporary.moved = true;
        Ok(Publication::Published)
    }
    pub(crate) fn unchanged(
        &self,
        prepared: &PreparedInputs,
        cancel: &AtomicBool,
    ) -> Result<bool, ScriptError> {
        let baseline = self.baseline.borrow();
        let Some(before) = baseline.as_ref() else {
            return Ok(true);
        };
        match freshness::sdk_outputs(prepared, &self.directory, cancel)? {
            Ok(after) if &after == before => Ok(true),
            Ok(_after) => {
                #[cfg(test)]
                {
                    let changed = before
                        .keys()
                        .chain(_after.keys())
                        .filter(|path| before.get(*path) != _after.get(*path))
                        .collect::<std::collections::BTreeSet<_>>();
                    for path in changed.into_iter().take(16) {
                        eprintln!(
                            "SDK output changed during Cargo: {}",
                            path.strip_prefix(&self.directory).unwrap_or(path).display()
                        );
                    }
                }
                Ok(false)
            }
            Err(_) => Ok(false),
        }
    }
}
struct Temporary {
    path: PathBuf,
    moved: bool,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.moved {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn read_receipt(path: &Path) -> Option<Receipt> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > LIMIT {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn check(cancel: &AtomicBool) -> Result<(), ScriptError> {
    if cancel.load(Ordering::Acquire) {
        Err(ScriptError::Worker("compilation cancelled".into()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_cancellation_releases_sdk_receipt_waiter_without_stealing_owner_lock() {
        let fixture = TestArtifactGuard::new(&std::env::temp_dir(), "sdk-lock-cancel").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let directory = fixture.path().to_path_buf();
        let lease = Lease::acquire(&directory, &cancel, Duration::from_secs(2))
            .unwrap()
            .unwrap();
        let child_cancel = cancel.clone();
        let child_directory = directory.clone();
        let child = thread::spawn(move || {
            Lease::acquire(&child_directory, &child_cancel, Duration::from_secs(2))
                .err()
                .unwrap()
        });
        cancel.store(true, Ordering::Release);
        assert!(child.join().unwrap().to_string().contains("cancelled"));
        let another = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.join(LOCK))
            .unwrap();
        assert!(matches!(
            another.try_lock(),
            Err(fs::TryLockError::WouldBlock)
        ));
        drop(lease);
        assert!(another.try_lock().is_ok());
    }
}
