//! Host-owned transactional state and bounded, reusable JSON-lines workers.

use super::{
    CompiledScript, MAX_PACKET, NEXT_WORKER, PACKET_MARKER, ScriptError, ScriptKind, ScriptTick,
    validate_sdk_match, validate_supported_sdk,
};
use module_engine_core::{ScriptContext, SdkIdentity};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, atomic::Ordering, mpsc},
    thread,
    time::Duration,
};

const STATE_PREFIX: &str = "// module-engine-state:";
const PROTOCOL_VERSION: u32 = 2;
const MAX_MANIFEST: u64 = (MAX_PACKET * 8) as u64;

/// Source-declared state schema and defaults. A schema name/version and the
/// structure of its initial JSON value together define hot-apply compatibility.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateDeclaration {
    pub schema: String,
    pub initial: Value,
}

impl Default for StateDeclaration {
    fn default() -> Self {
        Self {
            schema: String::new(),
            initial: Value::Null,
        }
    }
}

impl StateDeclaration {
    pub fn from_source(source: &str) -> Result<Self, ScriptError> {
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        let declarations: Vec<_> = source
            .lines()
            .take_while(|line| {
                let line = line.trim();
                line.is_empty()
                    || line.starts_with("//")
                    || (line.starts_with("#!") && !line.starts_with("#!["))
            })
            .filter_map(|line| line.trim().strip_prefix(STATE_PREFIX))
            .collect();
        if declarations.len() > 1 {
            return Err(ScriptError::Compile(
                "only one module-engine-state declaration is allowed".into(),
            ));
        }
        let Some(declaration) = declarations.first() else {
            return Ok(Self::default());
        };
        let parsed: Self = serde_json::from_str(declaration)
            .map_err(|error| ScriptError::Compile(format!("invalid state declaration: {error}")))?;
        if parsed.schema.trim().is_empty()
            || parsed.schema.len() > 256
            || !parsed.initial.is_object()
        {
            return Err(ScriptError::Compile(
                "state declaration needs a 1..256 byte schema and an initial JSON object".into(),
            ));
        }
        parsed.validate(&parsed.initial)?;
        Ok(parsed)
    }

    pub fn compatible_with(&self, previous: &Self) -> bool {
        self.schema == previous.schema && shape(&self.initial) == shape(&previous.initial)
    }

    /// Objects have exact declared keys and values preserve JSON types. Arrays
    /// may grow; a nonempty default declares their item structure, while an empty
    /// default is an explicitly opaque array owned by the schema version.
    pub fn validate(&self, state: &Value) -> Result<(), ScriptError> {
        if serde_json::to_vec(state)
            .map_err(|e| ScriptError::Packet(e.to_string()))?
            .len()
            > MAX_PACKET
        {
            return Err(ScriptError::Packet("state exceeds 1 MiB".into()));
        }
        if !value_matches(&self.initial, state) {
            return Err(ScriptError::Packet(format!(
                "state does not match declared schema {}",
                self.schema
            )));
        }
        Ok(())
    }
}

fn shape(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), shape(value)))
                .collect(),
        ),
        Value::Array(array) => serde_json::json!([array
            .first()
            .map(shape)
            .unwrap_or(Value::String("opaque".into()))]),
        Value::Null => Value::String("null".into()),
        Value::Bool(_) => Value::String("bool".into()),
        Value::Number(_) => Value::String("number".into()),
        Value::String(_) => Value::String("string".into()),
    }
}

fn value_matches(default: &Value, value: &Value) -> bool {
    match (default, value) {
        (Value::Object(defaults), Value::Object(values)) => {
            defaults.len() == values.len()
                && defaults.iter().all(|(key, default)| {
                    values
                        .get(key)
                        .is_some_and(|value| value_matches(default, value))
                })
        }
        (Value::Array(defaults), Value::Array(values)) => defaults
            .first()
            .is_none_or(|default| values.iter().all(|value| value_matches(default, value))),
        (Value::Null, Value::Null)
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::String(_), Value::String(_)) => true,
        _ => false,
    }
}

#[derive(Debug)]
pub(super) struct ScriptInstance {
    program_id: u64,
    revision: u64,
    state: Value,
    worker: Option<Worker>,
}

pub(super) fn new_instance(declaration: &StateDeclaration) -> Arc<Mutex<ScriptInstance>> {
    Arc::new(Mutex::new(ScriptInstance {
        program_id: NEXT_WORKER.fetch_add(1, Ordering::Relaxed),
        revision: 0,
        state: declaration.initial.clone(),
        worker: None,
    }))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrecompiledManifest {
    protocol_version: u32,
    executable: String,
    source: String,
    kind: ScriptKind,
    state: StateDeclaration,
    dependency_versions: Vec<(String, String)>,
    sdk: SdkIdentity,
}

impl CompiledScript {
    /// Another graph node may use the same compiled code, but owns independent
    /// state and a worker. Ordinary Clone deliberately shares one active owner.
    pub fn fork_instance(&self) -> Self {
        let mut fork = self.clone();
        fork.instance = new_instance(&self.state_declaration);
        fork
    }

    pub fn state_declaration(&self) -> &StateDeclaration {
        &self.state_declaration
    }

    /// Read the last accepted state; evaluating a candidate never modifies it.
    pub fn state(&self) -> Result<Value, ScriptError> {
        Ok(self.lock_instance()?.state.clone())
    }

    /// Retain state when installing a compatible candidate. The caller keeps
    /// the old CompiledScript installed if this returns an incompatibility.
    pub fn inherit_state_from(&self, previous: &Self) -> Result<(), ScriptError> {
        validate_supported_sdk(&self.sdk_identity)?;
        validate_sdk_match(previous.sdk_identity(), self.sdk_identity())?;
        if Arc::ptr_eq(&self.instance, &previous.instance) {
            return Ok(());
        }
        if !self
            .state_declaration
            .compatible_with(&previous.state_declaration)
        {
            return Err(ScriptError::Packet("incompatible state schema: keep the previous program until explicit reset or migration".into()));
        }
        let state = previous.state()?;
        self.migrate_state(state)
    }

    /// Explicit reset also terminates worker globals; this is suitable for a
    /// new Play session. It must not be triggered implicitly by a failed apply.
    pub fn reset_state(&self) -> Result<(), ScriptError> {
        self.migrate_state(self.state_declaration.initial.clone())
    }

    /// Explicit migration performed by host/tooling. This accepts only a value
    /// matching this candidate's declared schema; it does not execute old code.
    pub fn migrate_state(&self, state: Value) -> Result<(), ScriptError> {
        self.state_declaration.validate(&state)?;
        let mut instance = self.lock_instance()?;
        instance.state = state;
        instance.revision = instance.revision.wrapping_add(1);
        instance.worker = None;
        Ok(())
    }

    /// Commit only after the entire command/event transaction was accepted.
    /// A response from an older revision or another program cannot be committed.
    pub fn validate_tick(&self, tick: &ScriptTick) -> Result<(), ScriptError> {
        self.state_declaration.validate(&tick.state)?;
        let instance = self.lock_instance()?;
        if tick.program_id != instance.program_id
            || tick.revision != instance.revision
            || tick.state_schema != self.state_declaration.schema
        {
            return Err(ScriptError::Packet("stale script state transaction".into()));
        }
        Ok(())
    }

    /// Commit a previously validated response. Hosts using several workers can
    /// validate_tick on each before starting their command/state transaction.
    pub fn commit_tick(&self, tick: &ScriptTick) -> Result<(), ScriptError> {
        self.state_declaration.validate(&tick.state)?;
        let mut instance = self.lock_instance()?;
        if tick.program_id != instance.program_id
            || tick.revision != instance.revision
            || tick.state_schema != self.state_declaration.schema
        {
            return Err(ScriptError::Packet("stale script state transaction".into()));
        }
        instance.state = tick.state.clone();
        instance.revision = instance.revision.wrapping_add(1);
        Ok(())
    }

    /// Process ID is useful for diagnostics and verifying reuse. None means the
    /// worker has not started or has been stopped after an error/reset.
    pub fn worker_process_id(&self) -> Result<Option<u32>, ScriptError> {
        Ok(self
            .lock_instance()?
            .worker
            .as_ref()
            .map(|worker| worker.child.id()))
    }

    /// Stop the ordinary worker process without losing committed owned state.
    pub fn stop_worker(&self) -> Result<(), ScriptError> {
        self.lock_instance()?.worker = None;
        Ok(())
    }

    /// Reject a candidate after world/render/event validation failed. Owned
    /// state remains unchanged and the worker is restarted before another tick,
    /// preventing uncommitted worker globals from leaking into the next call.
    pub fn reject_tick(&self, tick: &ScriptTick) -> Result<(), ScriptError> {
        let mut instance = self.lock_instance()?;
        if tick.program_id != instance.program_id || tick.revision != instance.revision {
            return Err(ScriptError::Packet(
                "stale rejected script transaction".into(),
            ));
        }
        instance.worker = None;
        instance.revision = instance.revision.wrapping_add(1);
        Ok(())
    }

    pub(super) fn evaluate(
        &self,
        context: &ScriptContext,
        timeout: Duration,
    ) -> Result<ScriptTick, ScriptError> {
        validate_supported_sdk(&self.sdk_identity)?;
        if !context.time.is_finite()
            || !context.dt.is_finite()
            || context.dt < 0.0
            || !context.base.is_finite()
        {
            return Err(ScriptError::Packet(
                "time/dt/base must be finite and dt nonnegative".into(),
            ));
        }
        let mut instance = self.lock_instance()?;
        let mut context = context.clone();
        context.state = instance.state.clone();
        context.state_schema = self.state_declaration.schema.clone();
        context.commands.clear();
        context.emitted_events.clear();
        if instance.worker.is_none() {
            instance.worker = Some(Worker::start(&self.executable, &self.working_dir)?);
        }
        let result = instance
            .worker
            .as_mut()
            .unwrap()
            .exchange(&context, &self.sdk_identity, timeout)
            .and_then(|value| {
                validate_packet_sdk(&value, &self.sdk_identity)?;
                let commands = serde_json::from_value(
                    value
                        .get("commands")
                        .cloned()
                        .ok_or_else(|| ScriptError::Packet("missing commands".into()))?,
                )
                .map_err(|e| ScriptError::Packet(e.to_string()))?;
                let events: Vec<Value> = serde_json::from_value(
                    value
                        .get("events")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .map_err(|e| ScriptError::Packet(e.to_string()))?;
                let state = value
                    .get("state")
                    .cloned()
                    .ok_or_else(|| ScriptError::Packet("missing state".into()))?;
                self.state_declaration.validate(&state)?;
                let speed = value.get("speed").and_then(Value::as_f64);
                let tick = ScriptTick {
                    commands,
                    speed,
                    state,
                    events,
                    state_schema: self.state_declaration.schema.clone(),
                    program_id: instance.program_id,
                    revision: instance.revision,
                };
                if tick.commands.len() > 4096 || tick.events.len() > 4096 {
                    return Err(ScriptError::Packet(
                        "output exceeds 4096 commands/events".into(),
                    ));
                }
                if self.kind == ScriptKind::Speed && tick.speed.is_none() {
                    return Err(ScriptError::Packet(
                        "speed function returned a nonfinite value".into(),
                    ));
                }
                Ok(tick)
            });
        if let Err(error) = &result {
            self.retain_diagnostics(&error.to_string());
            instance.worker = None;
        }
        result
    }

    fn lock_instance(&self) -> Result<std::sync::MutexGuard<'_, ScriptInstance>, ScriptError> {
        self.instance
            .lock()
            .map_err(|_| ScriptError::Worker("script instance lock was poisoned".into()))
    }

    /// Export one standalone worker and a relocatable manifest. No SDK, Cargo,
    /// Rust compiler or source compilation is involved when loading this file.
    pub fn export_precompiled(&self, directory: &Path) -> Result<PathBuf, ScriptError> {
        validate_supported_sdk(&self.sdk_identity)?;
        fs::create_dir_all(directory).map_err(|e| ScriptError::Setup(e.to_string()))?;
        let executable = format!("worker{}", std::env::consts::EXE_SUFFIX);
        fs::copy(&self.executable, directory.join(&executable))
            .map_err(|e| ScriptError::Setup(e.to_string()))?;
        let manifest = PrecompiledManifest {
            protocol_version: PROTOCOL_VERSION,
            executable,
            source: self.source.clone(),
            kind: self.kind,
            state: self.state_declaration.clone(),
            dependency_versions: self.dependency_versions.clone(),
            sdk: self.sdk_identity.clone(),
        };
        let path = directory.join("script.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&manifest).map_err(|e| ScriptError::Setup(e.to_string()))?,
        )
        .map_err(|e| ScriptError::Setup(e.to_string()))?;
        Ok(path)
    }

    /// Load with package-root-relative asset access. The manifest's executable
    /// must remain inside its directory, including after resolving symlinks.
    pub fn load_precompiled(
        manifest_path: &Path,
        package_root: &Path,
    ) -> Result<Self, ScriptError> {
        Self::load_precompiled_with_sdk(manifest_path, package_root, &SdkIdentity::current())
    }

    /// Validate the exact portable API against this Player's supported identity;
    /// native loading does not inspect or require physical SDK source files.
    pub fn load_precompiled_with_sdk(
        manifest_path: &Path,
        package_root: &Path,
        expected_sdk: &SdkIdentity,
    ) -> Result<Self, ScriptError> {
        validate_supported_sdk(expected_sdk)?;
        let mut bytes = Vec::new();
        fs::File::open(manifest_path)
            .map_err(|e| ScriptError::Setup(e.to_string()))?
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| ScriptError::Setup(e.to_string()))?;
        if bytes.len() as u64 > MAX_MANIFEST {
            return Err(ScriptError::Setup(
                "precompiled manifest exceeds 8 MiB".into(),
            ));
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| ScriptError::Setup(error.to_string()))?;
        if value.get("protocol_version").and_then(Value::as_u64)
            != Some(u64::from(PROTOCOL_VERSION))
        {
            return Err(ScriptError::Setup("Unsupported precompiled script protocol; protocol 2 requires an exact SDK identity".into()));
        }
        let manifest: PrecompiledManifest =
            serde_json::from_value(value).map_err(|e| ScriptError::Setup(e.to_string()))?;
        validate_sdk_match(&manifest.sdk, expected_sdk)?;
        if manifest.protocol_version != PROTOCOL_VERSION || manifest.source.len() > MAX_PACKET {
            return Err(ScriptError::Setup(
                "unsupported precompiled script protocol/source size".into(),
            ));
        }
        if StateDeclaration::from_source(&manifest.source)? != manifest.state {
            return Err(ScriptError::Setup(
                "precompiled state declaration differs from source metadata".into(),
            ));
        }
        let relative = Path::new(&manifest.executable);
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || relative.as_os_str().is_empty()
        {
            return Err(ScriptError::Setup(
                "precompiled executable must be a relative path without traversal".into(),
            ));
        }
        let directory = manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .canonicalize()
            .map_err(|e| ScriptError::Setup(e.to_string()))?;
        let executable = directory
            .join(relative)
            .canonicalize()
            .map_err(|e| ScriptError::Setup(e.to_string()))?;
        if !executable.starts_with(&directory) || !executable.is_file() {
            return Err(ScriptError::Setup(
                "precompiled executable escapes manifest directory".into(),
            ));
        }
        let working_dir = package_root
            .canonicalize()
            .map_err(|e| ScriptError::Setup(e.to_string()))?;
        let retained_scopes = [
            super::cleanup::fixture_for(manifest_path),
            super::cleanup::fixture_for(&working_dir),
        ]
        .into_iter()
        .flatten()
        .collect();
        Ok(Self {
            source: manifest.source,
            kind: manifest.kind,
            executable,
            working_dir,
            worker_lock: PathBuf::new(),
            dependency_versions: manifest.dependency_versions,
            instance: new_instance(&manifest.state),
            state_declaration: manifest.state,
            sdk_identity: manifest.sdk,
            _artifacts: None,
            _retained_scopes: retained_scopes,
        })
    }
}

fn validate_packet_sdk(packet: &Value, expected: &SdkIdentity) -> Result<(), ScriptError> {
    let sdk: SdkIdentity = serde_json::from_value(
        packet
            .get("sdk")
            .cloned()
            .ok_or_else(|| ScriptError::Packet("Missing worker SDK identity".into()))?,
    )
    .map_err(|error| ScriptError::Packet(format!("Worker SDK identity: {error}")))?;
    validate_sdk_match(&sdk, expected).map_err(|error| ScriptError::Packet(error.to_string()))
}

#[derive(Debug)]
struct Worker {
    child: super::OwnedChild,
    input: mpsc::SyncSender<Vec<u8>>,
    output: mpsc::Receiver<Result<Value, ScriptError>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    sequence: u64,
}

impl Worker {
    fn start(executable: &Path, directory: &Path) -> Result<Self, ScriptError> {
        let mut command = Command::new(executable);
        command
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = super::OwnedChild::spawn(&mut command)?;
        let mut stdin = child.stdin.take().unwrap();
        let (input, requests) = mpsc::sync_channel::<Vec<u8>>(1);
        thread::spawn(move || {
            while let Ok(bytes) = requests.recv() {
                if stdin
                    .write_all(&bytes)
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let stdout = child.stdout.take().unwrap();
        let (responses, output) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            let mut ignored = 0;
            loop {
                line.clear();
                let count = (&mut reader)
                    .take((MAX_PACKET + 1) as u64)
                    .read_until(b'\n', &mut line);
                match count {
                    Ok(0) => {
                        let _ =
                            responses.send(Err(ScriptError::Worker("worker closed output".into())));
                        break;
                    }
                    Ok(_) if line.len() > MAX_PACKET => {
                        let _ = responses.send(Err(ScriptError::Packet(
                            "worker output exceeds 1 MiB".into(),
                        )));
                        break;
                    }
                    Err(error) => {
                        let _ = responses.send(Err(ScriptError::Worker(error.to_string())));
                        break;
                    }
                    _ => {}
                }
                if let Some(index) = line
                    .windows(PACKET_MARKER.len())
                    .position(|bytes| bytes == PACKET_MARKER.as_bytes())
                {
                    let value = serde_json::from_slice(&line[index + PACKET_MARKER.len()..])
                        .map_err(|e| ScriptError::Packet(e.to_string()));
                    if responses.send(value).is_err() {
                        break;
                    }
                    ignored = 0;
                } else {
                    ignored += line.len();
                    if ignored > MAX_PACKET {
                        let _ = responses.send(Err(ScriptError::Packet(
                            "worker debug output exceeds 1 MiB per request".into(),
                        )));
                        break;
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let diagnostic = Arc::clone(&stderr);
        let mut pipe = child.stderr.take().unwrap();
        thread::spawn(move || {
            let mut bytes = [0_u8; 8192];
            while let Ok(count) = pipe.read(&mut bytes) {
                if count == 0 {
                    break;
                }
                if let Ok(mut buffer) = diagnostic.lock() {
                    let remaining = 8192_usize.saturating_sub(buffer.len());
                    buffer.extend_from_slice(&bytes[..count.min(remaining)]);
                }
            }
        });
        Ok(Self {
            child,
            input,
            output,
            stderr,
            sequence: 0,
        })
    }

    fn exchange(
        &mut self,
        context: &ScriptContext,
        sdk: &SdkIdentity,
        timeout: Duration,
    ) -> Result<Value, ScriptError> {
        self.sequence = self.sequence.wrapping_add(1);
        let mut bytes = serde_json::to_vec(
            &serde_json::json!({"request_id":self.sequence,"sdk":sdk,"context":context}),
        )
        .map_err(|e| ScriptError::Packet(e.to_string()))?;
        if bytes.len() > MAX_PACKET {
            return Err(ScriptError::Packet("input exceeds 1 MiB".into()));
        }
        bytes.push(b'\n');
        self.input
            .try_send(bytes)
            .map_err(|_| ScriptError::Worker("worker input unavailable".into()))?;
        let response = self
            .output
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => ScriptError::Timeout(timeout.as_millis()),
                mpsc::RecvTimeoutError::Disconnected => {
                    ScriptError::Worker("worker output unavailable".into())
                }
            })?;
        let value = response.map_err(|error| {
            let details = self
                .stderr
                .lock()
                .ok()
                .map(|s| String::from_utf8_lossy(&s).to_string())
                .unwrap_or_default();
            if details.is_empty() {
                error
            } else {
                ScriptError::Worker(format!("{error}: {details}"))
            }
        })?;
        if value.get("request_id").and_then(Value::as_u64) != Some(self.sequence) {
            return Err(ScriptError::Packet(
                "worker returned a stale request ID".into(),
            ));
        }
        Ok(value)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.child.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_packets_require_exact_sdk_before_any_payload_is_accepted() {
        let current = SdkIdentity::current();
        validate_packet_sdk(&serde_json::json!({"sdk":current}), &current).unwrap();
        for sdk in [
            Value::Null,
            serde_json::json!({"version":"old","data_api_fingerprint":"module-engine-data-api/v2"}),
            serde_json::json!({"version":current.version,"data_api_fingerprint":"module-engine-data-api/v1"}),
        ] {
            assert!(matches!(
                validate_packet_sdk(
                    &serde_json::json!({"sdk":sdk,"state":{"count":99},"commands":[]}),
                    &current
                ),
                Err(ScriptError::Packet(_))
            ));
        }
        assert!(validate_packet_sdk(&serde_json::json!({"state":{"count":99}}), &current).is_err());
    }

    #[test]
    fn precompiled_identity_is_exact_without_a_physical_sdk_or_worker_start() {
        let artifacts =
            super::super::TestArtifactGuard::new(&std::env::temp_dir(), "module-precompiled-sdk")
                .unwrap();
        let root = artifacts.path().to_path_buf();
        fs::write(
            root.join("not-started.exe"),
            b"not an executable; loading metadata must not run it",
        )
        .unwrap();
        let source = "fn update(_: &mut ScriptContext) {}";
        let manifest = PrecompiledManifest {
            protocol_version: PROTOCOL_VERSION,
            executable: "not-started.exe".into(),
            source: source.into(),
            kind: ScriptKind::Update,
            state: StateDeclaration::default(),
            dependency_versions: vec![],
            sdk: SdkIdentity::current(),
        };
        let path = root.join("script.json");
        let value = serde_json::to_value(manifest).unwrap();
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let loaded = CompiledScript::load_precompiled(&path, &root).unwrap();
        assert_eq!(loaded.sdk_identity(), &SdkIdentity::current());
        assert_eq!(loaded.worker_process_id().unwrap(), None);
        assert_eq!(loaded.state().unwrap(), Value::Null);
        assert!(!root.join("engine-sdk.json").exists());
        for edit in 0..4 {
            let mut wrong = value.clone();
            match edit {
                0 => wrong["sdk"]["version"] = serde_json::json!("other"),
                1 => {
                    wrong["sdk"]["data_api_fingerprint"] =
                        serde_json::json!("module-engine-data-api/v1")
                }
                2 => {
                    wrong.as_object_mut().unwrap().remove("sdk");
                }
                _ => wrong["protocol_version"] = serde_json::json!(1),
            }
            fs::write(&path, serde_json::to_vec(&wrong).unwrap()).unwrap();
            assert!(CompiledScript::load_precompiled(&path, &root).is_err());
            assert_eq!(loaded.worker_process_id().unwrap(), None);
            assert_eq!(loaded.state().unwrap(), Value::Null);
        }
    }

    #[test]
    fn state_schema_checks_structure_types_version_and_explicit_array_items() {
        let source = "// module-engine-state: {\"schema\":\"combat.v1\",\"initial\":{\"timer\":0.0,\"hits\":[{\"id\":0}]}}\nfn update(_: &mut ScriptContext) {}";
        let declared = StateDeclaration::from_source(source).unwrap();
        assert!(
            declared
                .validate(&serde_json::json!({"timer":2.0,"hits":[{"id":5},{"id":8}]}))
                .is_ok()
        );
        assert!(
            declared
                .validate(&serde_json::json!({"timer":"wrong","hits":[]}))
                .is_err()
        );
        assert!(
            declared
                .validate(&serde_json::json!({"timer":0,"hits":[{"name":"unexpected"}]}))
                .is_err()
        );
        assert!(declared.compatible_with(
            &StateDeclaration::from_source(&source.replace("0.0", "3.0")).unwrap()
        ));
        assert!(!declared.compatible_with(
            &StateDeclaration::from_source(&source.replace("combat.v1", "combat.v2")).unwrap()
        ));
        assert!(
            !declared.compatible_with(
                &StateDeclaration::from_source(
                    &source.replace("\"timer\":0.0", "\"cooldown\":0.0")
                )
                .unwrap()
            )
        );
        assert_eq!(StateDeclaration::from_source("#![doc = r#\"\n// module-engine-state: {\"schema\":\"not metadata\",\"initial\":{}}\n\"#]\nfn update(_: &mut ScriptContext) {}").unwrap(), StateDeclaration::default());
        let header = source.lines().next().unwrap();
        assert!(StateDeclaration::from_source(&format!("{header}\n{source}")).is_err());
        assert_eq!(StateDeclaration::from_source("fn update(_: &mut ScriptContext) { let text = r#\"\n// module-engine-state: {\"schema\":\"not metadata\",\"initial\":{}}\n\"#; }").unwrap(), StateDeclaration::default());
    }
}
