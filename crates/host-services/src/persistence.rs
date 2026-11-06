use crate::{HostService, HostServiceRegistry, PERSISTENCE_SERVICE_ID};
use module_engine_core::GameEvent;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

const MAX_SAVE_BYTES: usize = 8 * 1024 * 1024;
const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidSavePolicy {
    #[default]
    Reject,
    ReadOnly,
}

/// The application chooses the writable root. Source packages choose stable
/// identities and own the complete opaque payload, including resume/undo data.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistenceConfig {
    pub namespace: String,
    pub data_key: String,
    pub write_event: String,
    pub version: u32,
    #[serde(default = "default_slot")]
    pub slot: String,
    #[serde(default = "default_payload_field")]
    pub payload_field: String,
    /// Opt in to a usable, write-blocked service after a load failure. The
    /// default still rejects construction; no policy silently replaces a save.
    #[serde(default)]
    pub invalid_save: InvalidSavePolicy,
    /// This explicit event needs {"confirm":true}; ordinary host reset/restart
    /// never removes or quarantines a save.
    #[serde(default)]
    pub reset_event: Option<String>,
}
fn default_slot() -> String {
    "state".into()
}
fn default_payload_field() -> String {
    "data".into()
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format_version: u32,
    namespace: String,
    data_key: String,
    version: u32,
    data: Value,
}

/// Installing a factory performs no filesystem access. Omitted/disabled
/// requests never create directories, inspect saves, or write files.
pub fn install_persistence_at(
    registry: &mut HostServiceRegistry,
    state_root: PathBuf,
) -> Result<(), String> {
    registry.register(PERSISTENCE_SERVICE_ID, move |request, context| {
        let config: PersistenceConfig = serde_json::from_value(request.config.clone())
            .map_err(|error| format!("Invalid persistence configuration: {error}"))?;
        Ok(Box::new(PersistenceService::new(
            &state_root,
            config,
            context.retired.clone(),
        )?))
    })
}

struct PersistenceService {
    config: PersistenceConfig,
    path: PathBuf,
    data: Option<Value>,
    retired: Arc<AtomicBool>,
    enabled: bool,
    requests: u64,
    writes: u64,
    reset_requests: u64,
    resets: u64,
    write_blocked: bool,
    last_error: Option<String>,
    error_kind: Option<&'static str>,
    recovery_file: Option<PathBuf>,
}
impl PersistenceService {
    fn new(
        root: &Path,
        config: PersistenceConfig,
        retired: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        for (label, value, max) in [
            ("namespace", config.namespace.as_str(), 96),
            ("slot", config.slot.as_str(), 64),
            ("data_key", config.data_key.as_str(), 256),
            ("write_event", config.write_event.as_str(), 256),
            ("payload_field", config.payload_field.as_str(), 256),
        ] {
            if value.trim().is_empty() || value.len() > max || value.contains('\0') {
                return Err(format!(
                    "Persistence {label} must contain 1..={max} non-NUL UTF-8 bytes"
                ));
            }
        }
        if config.version == 0 || root.as_os_str().is_empty() {
            return Err(
                "Persistence needs a nonempty application root and positive payload version".into(),
            );
        }
        if config.reset_event.as_ref().is_some_and(|event| {
            event.trim().is_empty()
                || event.len() > 256
                || event.contains('\0')
                || event == &config.write_event
        }) {
            return Err("Persistence reset_event must be nonempty, at most 256 non-NUL UTF-8 bytes and distinct from write_event".into());
        }
        // Identities are data, never relative paths. Hex also avoids Windows
        // reserved names, case folding, separators and trailing-dot aliases.
        let path = root
            .join(format!("ns-{}", hex(config.namespace.as_bytes())))
            .join(format!("slot-{}.json", hex(config.slot.as_bytes())));
        let _guard = publication_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (data, write_blocked, last_error, error_kind) = match read_envelope(&path, &config) {
            Ok(data) => (data, false, None, None),
            Err(error) if config.invalid_save == InvalidSavePolicy::ReadOnly => {
                let kind = error.kind();
                (None, true, Some(error.message), Some(kind))
            }
            Err(error) => return Err(error.message),
        };
        Ok(Self {
            config,
            path,
            data,
            retired,
            enabled: true,
            requests: 0,
            writes: 0,
            reset_requests: 0,
            resets: 0,
            write_blocked,
            last_error,
            error_kind,
            recovery_file: None,
        })
    }
    fn write(&mut self, data: &Value) -> Result<(), String> {
        self.write_with(data, |from, to| fs::rename(from, to))
    }
    fn write_with(
        &mut self,
        data: &Value,
        publish: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<(), String> {
        let _guard = publication_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.retired.load(Ordering::Acquire) {
            return Ok(());
        }
        if self.write_blocked {
            return Err(self.last_error.clone().unwrap_or_else(|| {
                "Persistence writes are blocked until a confirmed reset".into()
            }));
        }
        // Recheck the published identity/version every time: an external corrupt
        // or future-version save must not be overwritten by an older live host.
        let current = match read_envelope(&self.path, &self.config) {
            Ok(current) => current,
            Err(error) => {
                let kind = error.kind();
                self.write_blocked = self.config.invalid_save == InvalidSavePolicy::ReadOnly;
                return Err(self.latch(kind, error.message));
            }
        };
        if current.as_ref() == Some(data) {
            self.data = current;
            self.clear_error();
            return Ok(());
        }
        let result = (|| {
            let envelope = Envelope {
                format_version: FORMAT_VERSION,
                namespace: self.config.namespace.clone(),
                data_key: self.config.data_key.clone(),
                version: self.config.version,
                data: data.clone(),
            };
            let bytes = serde_json::to_vec_pretty(&envelope).map_err(|error| error.to_string())?;
            if bytes.len() > MAX_SAVE_BYTES {
                return Err(format!("Persistence save exceeds {MAX_SAVE_BYTES} bytes"));
            }
            publish_atomic(&self.path, &bytes, &self.retired, publish)
        })();
        match result {
            Ok(true) => {
                self.data = Some(data.clone());
                self.writes = self.writes.saturating_add(1);
                self.clear_error();
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(error) => Err(self.latch("write", error)),
        }
    }
    fn latch(&mut self, kind: &'static str, error: String) -> String {
        self.error_kind = Some(kind);
        self.last_error = Some(error.clone());
        error
    }
    fn clear_error(&mut self) {
        self.last_error = None;
        self.error_kind = None;
    }
    fn reset_save_with(
        &mut self,
        quarantine: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<(), String> {
        let _guard = publication_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match quarantine_existing(&self.path, &self.retired, quarantine) {
            Ok(QuarantineResult::Retired) => Ok(()),
            Ok(QuarantineResult::Complete(recovery)) => {
                if recovery.is_some() {
                    self.recovery_file = recovery;
                }
                self.data = None;
                self.write_blocked = false;
                self.resets = self.resets.saturating_add(1);
                self.clear_error();
                Ok(())
            }
            Err(error) => {
                self.write_blocked = true;
                Err(self.latch("reset", error))
            }
        }
    }
}
impl HostService for PersistenceService {
    fn initial_module_data(&self) -> BTreeMap<String, Value> {
        self.data
            .as_ref()
            .map(|data| BTreeMap::from([(self.config.data_key.clone(), data.clone())]))
            .unwrap_or_default()
    }
    fn consume(&mut self, events: &[GameEvent]) -> Result<(), String> {
        for event in events {
            if self.config.reset_event.as_ref() == Some(&event.name) {
                self.reset_requests = self.reset_requests.saturating_add(1);
                if !self.enabled || self.retired.load(Ordering::Acquire) {
                    continue;
                }
                if event.payload.get("confirm").and_then(Value::as_bool) != Some(true) {
                    self.write_blocked = true;
                    return Err(self.latch("reset", "Persistence reset requires an explicit confirm=true payload; previous save preserved".into()));
                }
                self.reset_save_with(|from, to| fs::rename(from, to))?;
                continue;
            }
            if event.name != self.config.write_event {
                continue;
            }
            self.requests = self.requests.saturating_add(1);
            if !self.enabled || self.retired.load(Ordering::Acquire) {
                continue;
            }
            if self.write_blocked {
                return Err(self.last_error.clone().unwrap_or_else(|| {
                    "Persistence writes are blocked until a confirmed reset".into()
                }));
            }
            let Some(data) = event.payload.get(&self.config.payload_field) else {
                return Err(self.latch(
                    "write",
                    format!(
                        "Persistence event '{}' needs field '{}'",
                        event.name, self.config.payload_field
                    ),
                ));
            };
            self.write(data)?;
        }
        Ok(())
    }
    fn reset(&mut self) -> Result<(), String> {
        self.requests = 0;
        self.writes = 0;
        self.reset_requests = 0;
        self.resets = 0;
        Ok(())
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    fn stop(&mut self) {
        self.enabled = false;
    }
    fn diagnostics(&self) -> Value {
        json!({"enabled":self.enabled,"namespace":self.config.namespace,"slot":self.config.slot,
            "data_key":self.config.data_key,"version":self.config.version,"loaded":self.data.is_some(),
            "requests":self.requests,"writes":self.writes,"reset_requests":self.reset_requests,"resets":self.resets,
            "write_blocked":self.write_blocked,"last_error":self.last_error,"error_kind":self.error_kind,
            "recovery_file":self.recovery_file.as_ref().map(|path|path.to_string_lossy().into_owned())})
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                DIGITS[(byte >> 4) as usize] as char,
                DIGITS[(byte & 15) as usize] as char,
            ]
        })
        .collect()
}
fn publication_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
#[derive(Debug)]
struct ReadError {
    message: String,
    io: bool,
}
impl ReadError {
    fn kind(&self) -> &'static str {
        if self.io { "io" } else { "invalid_save" }
    }
}
fn read_envelope(path: &Path, config: &PersistenceConfig) -> Result<Option<Value>, ReadError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ReadError {
                message: format!("Cannot read persistence save '{}': {error}", path.display()),
                io: true,
            });
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_SAVE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ReadError {
            message: format!("Cannot read persistence save: {error}"),
            io: true,
        })?;
    if bytes.len() > MAX_SAVE_BYTES {
        return Err(ReadError {
            message: format!("Persistence save exceeds {MAX_SAVE_BYTES} bytes"),
            io: false,
        });
    }
    let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|error| ReadError {
        message: format!(
            "Invalid persistence save '{}': {error}; original preserved",
            path.display()
        ),
        io: false,
    })?;
    if envelope.format_version != FORMAT_VERSION
        || envelope.version != config.version
        || envelope.namespace != config.namespace
        || envelope.data_key != config.data_key
    {
        return Err(ReadError {
            message: format!(
                "Persistence save '{}' has an incompatible format, payload version or namespace; original preserved",
                path.display()
            ),
            io: false,
        });
    }
    Ok(Some(envelope.data))
}
#[derive(Debug)]
enum QuarantineResult {
    Retired,
    Complete(Option<PathBuf>),
}
fn quarantine_existing(
    path: &Path,
    retired: &AtomicBool,
    quarantine: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<QuarantineResult, String> {
    if retired.load(Ordering::Acquire) {
        return Ok(QuarantineResult::Retired);
    }
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(QuarantineResult::Complete(None)),
        Err(error) => return Err(format!("Cannot inspect persistence save before reset: {error}; previous save preserved")),
        Ok(metadata) if !metadata.file_type().is_file() => return Err("Cannot reset persistence save: the published entry is not a regular file; original preserved".into()),
        Ok(_) => {}
    }
    let parent = path
        .parent()
        .ok_or("Persistence reset has no parent directory")?;
    static NEXT_RECOVERY: AtomicU64 = AtomicU64::new(1);
    let mut reserved = None;
    for _ in 0..16 {
        let serial = NEXT_RECOVERY.fetch_add(1, Ordering::Relaxed);
        let recovery = parent.join(format!(
            ".save-recovery-{}-{serial}.json",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&recovery)
        {
            Ok(file) => {
                drop(file);
                reserved = Some(recovery);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Cannot reserve persistence recovery file: {error}; previous save preserved"
                ));
            }
        }
    }
    let recovery =
        reserved.ok_or("Persistence recovery names are occupied; previous save preserved")?;
    if retired.load(Ordering::Acquire) {
        let _ = fs::remove_file(&recovery);
        return Ok(QuarantineResult::Retired);
    }
    // Replace only the empty entry created by this operation. Never move a
    // directory or overwrite a pre-existing recovery file. The original entry
    // remains published if this same-directory atomic rename fails.
    match quarantine(path, &recovery) {
        Ok(()) => Ok(QuarantineResult::Complete(Some(recovery))),
        Err(error) => {
            let _ = fs::remove_file(&recovery);
            Err(format!(
                "Cannot quarantine persistence save: {error}; previous save preserved"
            ))
        }
    }
}
fn publish_atomic(
    path: &Path,
    bytes: &[u8],
    retired: &AtomicBool,
    publish: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<bool, String> {
    if retired.load(Ordering::Acquire) {
        return Ok(false);
    }
    let parent = path
        .parent()
        .ok_or("Persistence save has no parent directory")?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Cannot create persistence directory: {error}"))?;
    static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(1);
    let mut candidate = None;
    for _ in 0..16 {
        let serial = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".save-{}-{serial}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => {
                candidate = Some((temporary, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Cannot create persistence candidate: {error}")),
        }
    }
    let (temporary, mut file) = candidate.ok_or("Persistence candidate names are occupied")?;
    let result = (|| {
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("Cannot flush persistence candidate: {error}"))?;
        drop(file);
        if retired.load(Ordering::Acquire) {
            return Ok(false);
        }
        publish(&temporary, path).map_err(|error| {
            format!("Cannot publish persistence save: {error}; previous save preserved")
        })?;
        Ok(true)
    })();
    // This path was created by this operation. Never remove an unrelated file.
    let _ = fs::remove_file(&temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../work/host-service-tests")
            .join(format!(
                "ModuleEngine-persistence-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
    }
    fn config(namespace: &str) -> PersistenceConfig {
        PersistenceConfig {
            namespace: namespace.into(),
            data_key: "fixture.resume/1".into(),
            write_event: "fixture.save".into(),
            version: 1,
            slot: default_slot(),
            payload_field: default_payload_field(),
            invalid_save: InvalidSavePolicy::Reject,
            reset_event: None,
        }
    }
    fn service(root: &Path, namespace: &str) -> PersistenceService {
        PersistenceService::new(root, config(namespace), Arc::new(AtomicBool::new(false))).unwrap()
    }
    fn recoverable(namespace: &str) -> PersistenceConfig {
        let mut config = config(namespace);
        config.invalid_save = InvalidSavePolicy::ReadOnly;
        config.reset_event = Some("fixture.storage.reset".into());
        config
    }
    #[test]
    fn legacy_default_reject_remains_retryable_after_external_envelope_repair() {
        let root = root("default-retry");
        let config:PersistenceConfig=serde_json::from_value(json!({"namespace":"profile","data_key":"fixture.resume/1","write_event":"fixture.save","version":1})).unwrap();
        assert_eq!(config.invalid_save, InvalidSavePolicy::Reject);
        assert!(config.reset_event.is_none());
        let mut saved =
            PersistenceService::new(&root, config, Arc::new(AtomicBool::new(false))).unwrap();
        saved.write(&json!({"old":1})).unwrap();
        let good = fs::read(&saved.path).unwrap();
        fs::write(&saved.path, b"corrupt").unwrap();
        assert!(saved.write(&json!({"new":2})).is_err());
        assert!(!saved.write_blocked);
        assert_eq!(saved.diagnostics()["error_kind"], json!("invalid_save"));
        saved.consume(&[]).unwrap();
        assert!(saved.last_error.is_some());
        fs::write(&saved.path, &good).unwrap();
        saved.write(&json!({"new":2})).unwrap();
        assert!(saved.last_error.is_none());
        assert_eq!(
            service(&root, "profile").initial_module_data()["fixture.resume/1"],
            json!({"new":2})
        );
        cleanup(&root);
    }
    fn confirmed_reset() -> GameEvent {
        GameEvent::new("fixture.storage.reset", json!({"confirm":true}))
    }
    #[test]
    fn opt_in_bad_envelope_blocks_writes_until_confirmed_reset_then_saves_same_batch() {
        let root = root("recover-corrupt");
        let mut original = service(&root, "profile");
        original.write(&json!({"old":1})).unwrap();
        fs::write(&original.path, b"broken user save\n").unwrap();
        assert!(
            PersistenceService::new(&root, config("profile"), original.retired.clone()).is_err()
        );
        let mut recover =
            PersistenceService::new(&root, recoverable("profile"), original.retired.clone())
                .unwrap();
        assert!(recover.initial_module_data().is_empty());
        assert_eq!(recover.diagnostics()["error_kind"], json!("invalid_save"));
        let error = recover.diagnostics()["last_error"].clone();
        recover.consume(&[]).unwrap();
        recover.reset().unwrap();
        assert_eq!(recover.diagnostics()["last_error"], error);
        assert!(
            recover
                .consume(&[GameEvent::new(
                    "fixture.save",
                    json!({"data":{"replacement":1}})
                )])
                .is_err()
        );
        assert_eq!(fs::read(&original.path).unwrap(), b"broken user save\n");
        assert!(
            recover
                .consume(&[GameEvent::new(
                    "fixture.storage.reset",
                    json!({"confirm":false})
                )])
                .is_err()
        );
        assert!(recover.write_blocked);
        assert_eq!(fs::read(&original.path).unwrap(), b"broken user save\n");
        recover
            .consume(&[
                confirmed_reset(),
                GameEvent::new("fixture.save", json!({"data":{"fresh":2}})),
            ])
            .unwrap();
        assert!(!recover.write_blocked);
        assert!(recover.last_error.is_none());
        assert_eq!(recover.resets, 1);
        assert_eq!(recover.writes, 1);
        let recovery = recover.recovery_file.as_ref().unwrap();
        assert_eq!(fs::read(recovery).unwrap(), b"broken user save\n");
        assert_eq!(
            service(&root, "profile").initial_module_data()["fixture.resume/1"],
            json!({"fresh":2})
        );
        cleanup(&root);
    }
    #[test]
    fn foreign_and_future_envelopes_remain_untouched_in_opt_in_read_only_mode() {
        let root = root("foreign");
        let mut original = service(&root, "profile");
        original.write(&json!({"opaque":1})).unwrap();
        let good = fs::read(&original.path).unwrap();
        for (field, value) in [
            ("format_version", json!(2)),
            ("version", json!(2)),
            ("namespace", json!("foreign")),
            ("data_key", json!("other.namespace")),
        ] {
            let mut envelope: Value = serde_json::from_slice(&good).unwrap();
            envelope[field] = value;
            let bytes = serde_json::to_vec(&envelope).unwrap();
            fs::write(&original.path, &bytes).unwrap();
            let recover =
                PersistenceService::new(&root, recoverable("profile"), original.retired.clone())
                    .unwrap();
            assert!(recover.write_blocked && recover.initial_module_data().is_empty());
            assert_eq!(recover.diagnostics()["error_kind"], json!("invalid_save"));
            assert_eq!(fs::read(&original.path).unwrap(), bytes);
        }
        cleanup(&root);
    }
    #[test]
    fn load_io_is_distinct_from_invalid_payload_and_reset_never_moves_a_directory() {
        let root = root("load-io");
        let original = service(&root, "profile");
        fs::create_dir_all(&original.path).unwrap();
        fs::write(original.path.join("unrelated.txt"), b"preserve").unwrap();
        assert!(
            PersistenceService::new(&root, config("profile"), original.retired.clone()).is_err()
        );
        let mut recover =
            PersistenceService::new(&root, recoverable("profile"), original.retired.clone())
                .unwrap();
        assert_eq!(recover.diagnostics()["error_kind"], json!("io"));
        assert!(recover.write_blocked);
        assert!(recover.initial_module_data().is_empty());
        assert!(recover.consume(&[confirmed_reset()]).is_err());
        assert!(recover.write_blocked && recover.last_error.is_some());
        assert!(original.path.is_dir());
        assert_eq!(
            fs::read(original.path.join("unrelated.txt")).unwrap(),
            b"preserve"
        );
        cleanup(&root);
    }
    #[test]
    fn explicit_reset_preserves_domain_invalid_opaque_payload_without_interpreting_it() {
        let root = root("domain-invalid");
        let mut original = service(&root, "profile");
        original
            .write(&json!({"not_a_typed_profile":[null,{"literal.key":"future"}]}))
            .unwrap();
        let bytes = fs::read(&original.path).unwrap();
        let mut recover =
            PersistenceService::new(&root, recoverable("profile"), original.retired.clone())
                .unwrap();
        assert!(!recover.write_blocked);
        assert!(
            recover.initial_module_data()["fixture.resume/1"]
                .get("not_a_typed_profile")
                .is_some()
        );
        recover
            .consume(&[
                confirmed_reset(),
                GameEvent::new(
                    "fixture.save",
                    json!({"data":{"source_owned_default":true}}),
                ),
            ])
            .unwrap();
        assert_eq!(
            fs::read(recover.recovery_file.as_ref().unwrap()).unwrap(),
            bytes
        );
        assert_eq!(
            service(&root, "profile").initial_module_data()["fixture.resume/1"],
            json!({"source_owned_default":true})
        );
        cleanup(&root);
    }
    #[test]
    fn reset_publication_failure_keeps_original_blocked_and_latched_until_successful_reset() {
        let root = root("reset-failure");
        let mut saved = service(&root, "profile");
        saved.write(&json!({"old":1})).unwrap();
        let bytes = fs::read(&saved.path).unwrap();
        let error = saved
            .reset_save_with(|_, _| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected reset ACL failure",
                ))
            })
            .unwrap_err();
        assert!(error.contains("previous save preserved"));
        assert!(saved.write_blocked);
        assert_eq!(saved.diagnostics()["error_kind"], json!("reset"));
        assert_eq!(fs::read(&saved.path).unwrap(), bytes);
        assert_eq!(
            fs::read_dir(saved.path.parent().unwrap()).unwrap().count(),
            1
        );
        saved.consume(&[]).unwrap();
        saved.reset().unwrap();
        assert_eq!(saved.last_error.as_deref(), Some(error.as_str()));
        assert!(saved.write(&json!({"must_not_replace":1})).is_err());
        assert_eq!(fs::read(&saved.path).unwrap(), bytes);
        saved
            .reset_save_with(|from, to| fs::rename(from, to))
            .unwrap();
        assert!(!saved.write_blocked && saved.last_error.is_none());
        assert_eq!(
            fs::read(saved.recovery_file.as_ref().unwrap()).unwrap(),
            bytes
        );
        assert!(!saved.path.exists());
        cleanup(&root);
    }
    #[test]
    fn write_errors_latch_through_idle_and_host_reset_then_clear_on_successful_retry() {
        let root = root("write-failure");
        let mut saved = service(&root, "profile");
        saved.write(&json!({"old":1})).unwrap();
        let bytes = fs::read(&saved.path).unwrap();
        let error = saved
            .write_with(&json!({"new":2}), |_, _| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected write ACL failure",
                ))
            })
            .unwrap_err();
        assert_eq!(saved.diagnostics()["error_kind"], json!("write"));
        assert!(!saved.write_blocked);
        assert_eq!(fs::read(&saved.path).unwrap(), bytes);
        saved.consume(&[]).unwrap();
        saved.reset().unwrap();
        assert_eq!(saved.last_error.as_deref(), Some(error.as_str()));
        saved.write(&json!({"new":2})).unwrap();
        assert!(saved.last_error.is_none() && saved.error_kind.is_none());
        assert_eq!(
            service(&root, "profile").initial_module_data()["fixture.resume/1"],
            json!({"new":2})
        );
        cleanup(&root);
    }
    #[test]
    fn disabled_retired_and_unconfigured_reset_events_preserve_file_and_latched_error() {
        let root = root("reset-gates");
        let mut original = service(&root, "profile");
        original.write(&json!(1)).unwrap();
        let bytes = fs::read(&original.path).unwrap();
        original.consume(&[confirmed_reset()]).unwrap();
        assert_eq!(fs::read(&original.path).unwrap(), bytes);
        fs::write(&original.path, b"corrupt").unwrap();
        let mut recover =
            PersistenceService::new(&root, recoverable("profile"), original.retired.clone())
                .unwrap();
        let error = recover.last_error.clone();
        recover.set_enabled(false);
        recover.consume(&[confirmed_reset()]).unwrap();
        recover.set_enabled(true);
        recover.retired.store(true, Ordering::Release);
        recover.consume(&[confirmed_reset()]).unwrap();
        assert_eq!(recover.last_error, error);
        assert!(recover.write_blocked);
        assert_eq!(fs::read(&original.path).unwrap(), b"corrupt");
        assert_eq!(
            fs::read_dir(original.path.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        cleanup(&root);
    }
    fn cleanup(root: &Path) {
        if root.exists() {
            let absolute = root.canonicalize().unwrap();
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../work/host-service-tests")
                .canonicalize()
                .unwrap();
            assert!(absolute.starts_with(&fixtures) && absolute != fixtures);
            fs::remove_dir_all(absolute).unwrap();
        }
    }
    #[test]
    fn round_trip_preserves_opaque_undo_null_and_namespace_isolation() {
        let root = root("roundtrip");
        let mut a = service(&root, "project/profile-A");
        assert!(a.initial_module_data().is_empty());
        assert!(!root.exists());
        let data =
            json!({"board":[1,2],"undo":[{"literal.key":null}],"unknown":{"future":[false,3]}});
        a.consume(&[GameEvent::new("fixture.save", json!({"data":data}))])
            .unwrap();
        let bytes = fs::read(&a.path).unwrap();
        a.consume(&[GameEvent::new("fixture.save", json!({"data":data}))])
            .unwrap();
        assert_eq!(a.writes, 1);
        assert_eq!(fs::read(&a.path).unwrap(), bytes);
        assert_eq!(
            service(&root, "project/profile-A").initial_module_data()["fixture.resume/1"],
            data
        );
        let mut b = service(&root, "project/profile-a");
        assert!(b.initial_module_data().is_empty());
        b.consume(&[GameEvent::new("fixture.save", json!({"data":null}))])
            .unwrap();
        assert_eq!(
            service(&root, "project/profile-a").initial_module_data()["fixture.resume/1"],
            Value::Null
        );
        assert_eq!(
            service(&root, "project/profile-A").initial_module_data()["fixture.resume/1"],
            data
        );
        cleanup(&root);
    }
    #[test]
    fn corruption_version_and_publish_failure_preserve_previous_bytes() {
        let root = root("errors");
        let mut saved = service(&root, "one");
        saved.write(&json!({"good":1})).unwrap();
        let good = fs::read(&saved.path).unwrap();
        let error = publish_atomic(
            &saved.path,
            b"replacement",
            &AtomicBool::new(false),
            |_, _| Err(std::io::Error::other("injected publish failure")),
        )
        .unwrap_err();
        assert!(error.contains("previous save preserved"));
        assert_eq!(fs::read(&saved.path).unwrap(), good);
        assert_eq!(
            fs::read_dir(saved.path.parent().unwrap()).unwrap().count(),
            1
        );
        let mut incompatible = config("one");
        incompatible.version = 2;
        assert!(PersistenceService::new(&root, incompatible, saved.retired.clone()).is_err());
        assert_eq!(fs::read(&saved.path).unwrap(), good);
        fs::write(&saved.path, b"corrupt").unwrap();
        assert!(saved.write(&json!({"good":2})).is_err());
        assert_eq!(fs::read(&saved.path).unwrap(), b"corrupt");
        assert!(PersistenceService::new(&root, config("one"), saved.retired.clone()).is_err());
        cleanup(&root);
    }
    #[test]
    fn muted_retired_reset_and_idle_never_write_or_delete_a_save() {
        let root = root("effects");
        let mut saved = service(&root, "..\\outside/Юникод");
        saved
            .consume(&[GameEvent::new("other", Value::Null)])
            .unwrap();
        saved.set_enabled(false);
        saved
            .consume(&[GameEvent::new("fixture.save", json!({"data":1}))])
            .unwrap();
        assert!(!root.exists());
        saved.set_enabled(true);
        saved.write(&json!(7)).unwrap();
        let bytes = fs::read(&saved.path).unwrap();
        saved.reset().unwrap();
        saved.retired.store(true, Ordering::Release);
        saved
            .consume(&[GameEvent::new("fixture.save", json!({"data":8}))])
            .unwrap();
        saved.stop();
        assert_eq!(fs::read(&saved.path).unwrap(), bytes);
        assert!(saved.path.starts_with(&root));
        cleanup(&root);
    }
    #[test]
    fn disabled_factory_does_not_read_invalid_configuration_or_make_directories() {
        let root = root("disabled");
        let mut registry = HostServiceRegistry::new();
        install_persistence_at(&mut registry, root.clone()).unwrap();
        let context = crate::HostServiceContext {
            asset_root: Path::new("missing"),
            asset_paths: &BTreeMap::new(),
            retired: Arc::new(AtomicBool::new(false)),
        };
        let request = module_engine_core::HostServiceRequest {
            id: PERSISTENCE_SERVICE_ID.into(),
            config: Value::Null,
            asset_references: Default::default(),
        };
        assert!(
            registry
                .build(&[request], &context, false)
                .unwrap()
                .initial_module_data()
                .unwrap()
                .is_empty()
        );
        assert!(!root.exists());
    }
}
