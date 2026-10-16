//! Bounded owner-local immutable programs. No live ScriptInstance is cached.
use crate::{
    freshness::{self, DependencyHints, Eligibility, FreshnessSeal, SealOutcome, Validation},
    *,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex};

#[derive(Clone, Copy, Debug)]
pub struct ProgramStoreLimits {
    pub max_entries: usize,
    pub max_artifact_bytes: u64,
    pub max_contexts: usize,
    pub max_in_flight: usize,
}
impl Default for ProgramStoreLimits {
    fn default() -> Self {
        Self {
            max_entries: 16,
            max_artifact_bytes: 256 * 1024 * 1024,
            max_contexts: 4,
            max_in_flight: 2,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ProgramStoreStats {
    pub program_hits: u64,
    pub cache_misses: u64,
    pub cargo_build_invocations: u64,
    pub root_artifacts_built: u64,
    pub root_artifacts_fresh: u64,
    pub dependency_artifacts_built: u64,
    pub dependency_artifacts_fresh: u64,
    pub evictions: u64,
    pub coalesced_waits: u64,
    pub uncacheable: u64,
    pub proof_rejections: u64,
    pub unsupported_configurations: u64,
    pub unsupported_dependencies: u64,
    pub artifact_repairs: u64,
    pub new_dependency_misses: u64,
    pub resident_entries: u64,
    pub resident_artifact_bytes: u64,
    pub resident_contexts: u64,
    pub in_flight: u64,
    pub proof_micros: u64,
    pub build_micros: u64,
}
#[derive(Default)]
struct Counters {
    program_hits: AtomicU64,
    cache_misses: AtomicU64,
    cargo_build_invocations: AtomicU64,
    root_artifacts_built: AtomicU64,
    root_artifacts_fresh: AtomicU64,
    dependency_artifacts_built: AtomicU64,
    dependency_artifacts_fresh: AtomicU64,
    evictions: AtomicU64,
    coalesced_waits: AtomicU64,
    uncacheable: AtomicU64,
    proof_rejections: AtomicU64,
    unsupported_configurations: AtomicU64,
    unsupported_dependencies: AtomicU64,
    artifact_repairs: AtomicU64,
    new_dependency_misses: AtomicU64,
    resident_entries: AtomicU64,
    resident_artifact_bytes: AtomicU64,
    resident_contexts: AtomicU64,
    in_flight: AtomicU64,
    proof_micros: AtomicU64,
    build_micros: AtomicU64,
}
struct Program {
    source: String,
    kind: ScriptKind,
    executable: PathBuf,
    working_dir: PathBuf,
    worker_lock: PathBuf,
    dependency_versions: Vec<(String, String)>,
    state_declaration: StateDeclaration,
    sdk_identity: SdkIdentity,
    artifacts: Option<Arc<cleanup::CandidateArtifacts>>,
    scopes: Vec<Arc<cleanup::TestArtifacts>>,
}
impl Program {
    fn capture(script: &CompiledScript) -> Self {
        Self {
            source: script.source.clone(),
            kind: script.kind,
            executable: script.executable.clone(),
            working_dir: script.working_dir.clone(),
            worker_lock: script.worker_lock.clone(),
            dependency_versions: script.dependency_versions.clone(),
            state_declaration: script.state_declaration.clone(),
            sdk_identity: script.sdk_identity.clone(),
            artifacts: script._artifacts.clone(),
            scopes: script._retained_scopes.clone(),
        }
    }
    fn instantiate(&self) -> CompiledScript {
        CompiledScript {
            source: self.source.clone(),
            kind: self.kind,
            executable: self.executable.clone(),
            working_dir: self.working_dir.clone(),
            worker_lock: self.worker_lock.clone(),
            dependency_versions: self.dependency_versions.clone(),
            state_declaration: self.state_declaration.clone(),
            instance: persistent::new_instance(&self.state_declaration),
            sdk_identity: self.sdk_identity.clone(),
            _artifacts: self.artifacts.clone(),
            _retained_scopes: self.scopes.clone(),
        }
    }
}
struct Entry {
    program: Arc<Program>,
    seal: FreshnessSeal,
    sdk: String,
    bytes: u64,
    used: u64,
    build: Arc<BuildSpec>,
}
struct BuildSpec {
    compiler: ScriptCompiler,
    facts: freshness::BuildFacts,
    directory: PathBuf,
}
#[derive(Default)]
struct State {
    entries: BTreeMap<String, Entry>,
    hints: BTreeMap<String, (DependencyHints, u64)>,
    flights: BTreeSet<String>,
    active: usize,
    clock: u64,
}
#[derive(Debug, PartialEq, Eq)]
enum Claim {
    Ready,
    Waiting,
    Leader,
}
impl State {
    fn claim(&mut self, key: &str) -> Claim {
        if self.entries.contains_key(key) {
            Claim::Ready
        } else if self.flights.insert(key.into()) {
            Claim::Leader
        } else {
            Claim::Waiting
        }
    }
    fn retire_if_same(&mut self, key: &str, program: &Arc<Program>) -> Option<Entry> {
        if self
            .entries
            .get(key)
            .is_some_and(|entry| Arc::ptr_eq(&entry.program, program))
        {
            self.entries.remove(key)
        } else {
            None
        }
    }
}
/// Lifetime belongs to an editor/host. Dropping this owner releases cached code
/// while active scripts retain their independent artifact leases.
pub struct CompiledProgramStore {
    limits: ProgramStoreLimits,
    state: Mutex<State>,
    changed: Condvar,
    counters: Counters,
}
impl std::fmt::Debug for CompiledProgramStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledProgramStore")
            .field("limits", &self.limits)
            .field("stats", &self.stats())
            .finish()
    }
}
impl Default for CompiledProgramStore {
    fn default() -> Self {
        Self::new(ProgramStoreLimits::default()).expect("nonzero default program limits")
    }
}
impl CompiledProgramStore {
    pub fn new(limits: ProgramStoreLimits) -> Result<Self, ScriptError> {
        if limits.max_entries == 0
            || limits.max_artifact_bytes == 0
            || limits.max_contexts == 0
            || limits.max_in_flight == 0
            || limits.max_in_flight > 32
            || limits.max_entries > 1024
            || limits.max_contexts > 256
        {
            return Err(ScriptError::Setup(
                "Invalid bounded Custom program store limits".into(),
            ));
        }
        Ok(Self {
            limits,
            state: Mutex::default(),
            changed: Condvar::new(),
            counters: Counters::default(),
        })
    }
    /// Atomic snapshot only: safe for progress/UI reporting without filesystem IO.
    pub fn stats(&self) -> ProgramStoreStats {
        let c = &self.counters;
        macro_rules! load {($($name:ident),*)=>{ProgramStoreStats{$($name:c.$name.load(Ordering::Relaxed)),*}}}
        load!(
            program_hits,
            cache_misses,
            cargo_build_invocations,
            root_artifacts_built,
            root_artifacts_fresh,
            dependency_artifacts_built,
            dependency_artifacts_fresh,
            evictions,
            coalesced_waits,
            uncacheable,
            proof_rejections,
            unsupported_configurations,
            unsupported_dependencies,
            artifact_repairs,
            new_dependency_misses,
            resident_entries,
            resident_artifact_bytes,
            resident_contexts,
            in_flight,
            proof_micros,
            build_micros
        )
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn reject(&self, reason: &freshness::MissReason) {
        use freshness::MissReason;
        self.counters
            .proof_rejections
            .fetch_add(1, Ordering::Relaxed);
        let counter = match reason {
            MissReason::UnsupportedConfiguration => Some(&self.counters.unsupported_configurations),
            MissReason::UnsupportedDependency | MissReason::UnprovenToolchain => {
                Some(&self.counters.unsupported_dependencies)
            }
            MissReason::InvalidArtifact => Some(&self.counters.artifact_repairs),
            MissReason::NewDependency => Some(&self.counters.new_dependency_misses),
            _ => None,
        };
        if let Some(counter) = counter {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn admit(&self, cancel: &AtomicBool) -> Result<Admission<'_>, ScriptError> {
        let mut state = self.lock();
        while state.active >= self.limits.max_in_flight {
            check_cancel(cancel)?;
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        check_cancel(cancel)?;
        state.active += 1;
        self.counters.in_flight.fetch_add(1, Ordering::Relaxed);
        Ok(Admission { store: self })
    }
    fn update_resident(&self, state: &State) {
        self.counters
            .resident_entries
            .store(state.entries.len() as u64, Ordering::Relaxed);
        self.counters.resident_artifact_bytes.store(
            state.entries.values().map(|e| e.bytes).sum(),
            Ordering::Relaxed,
        );
        self.counters.resident_contexts.store(
            state
                .entries
                .values()
                .map(|e| &e.sdk)
                .collect::<BTreeSet<_>>()
                .len() as u64,
            Ordering::Relaxed,
        );
    }
    fn hint_key(context: &freshness::CompileContext, source: &str, kind: ScriptKind) -> String {
        let mut hash = Sha256::new();
        for bytes in [
            context.sdk_root.as_os_str().as_encoded_bytes(),
            context.work_root.as_os_str().as_encoded_bytes(),
            context.target_root.as_os_str().as_encoded_bytes(),
            source.as_bytes(),
            if kind == ScriptKind::Update {
                b"update"
            } else {
                b"speed"
            },
        ] {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        format!("{:x}", hash.finalize())
    }
    fn remember_hints(&self, key: String, hints: DependencyHints) {
        let mut state = self.lock();
        state.clock += 1;
        let now = state.clock;
        state.hints.insert(key, (hints, now));
        while state.hints.len() > self.limits.max_entries {
            let key = state
                .hints
                .iter()
                .min_by_key(|(_, (_, used))| used)
                .unwrap()
                .0
                .clone();
            state.hints.remove(&key);
        }
    }
    fn publish(
        &self,
        key: String,
        sdk: String,
        program: Arc<Program>,
        seal: FreshnessSeal,
        bytes: u64,
        build: Arc<BuildSpec>,
    ) {
        if bytes > self.limits.max_artifact_bytes {
            return;
        }
        let mut retired = Vec::new();
        {
            let mut state = self.lock();
            state.clock += 1;
            let now = state.clock;
            if let Some(old) = state.entries.insert(
                key,
                Entry {
                    program,
                    seal,
                    sdk,
                    bytes,
                    used: now,
                    build,
                },
            ) {
                retired.push(old)
            }
            loop {
                let total: u64 = state.entries.values().map(|e| e.bytes).sum();
                let contexts = state
                    .entries
                    .values()
                    .map(|e| &e.sdk)
                    .collect::<BTreeSet<_>>()
                    .len();
                if state.entries.len() <= self.limits.max_entries
                    && total <= self.limits.max_artifact_bytes
                    && contexts <= self.limits.max_contexts
                {
                    break;
                }
                let key = state
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.used)
                    .unwrap()
                    .0
                    .clone();
                retired.push(state.entries.remove(&key).unwrap());
                self.counters.evictions.fetch_add(1, Ordering::Relaxed);
            }
            self.update_resident(&state);
        }
        // Last leases can enqueue cleanup; never retire code under the map lock.
        drop(retired);
    }
    pub(crate) fn compile(
        &self,
        compiler: &ScriptCompiler,
        source: &str,
        kind: ScriptKind,
        cancel: &AtomicBool,
    ) -> Result<CompiledScript, ScriptError> {
        if source.len() > MAX_PACKET {
            return Err(ScriptError::Compile("source exceeds 1 MiB".into()));
        }
        StateDeclaration::from_source(source)?;
        let mut force_next_sdk = false;
        loop {
            let admission = self.admit(cancel)?;
            let proof_started = Instant::now();
            let resolved = crate::cache_context::resolve(compiler, cancel)?;
            let hint_key = Self::hint_key(&resolved.context, source, kind);
            let hints = self
                .lock()
                .hints
                .get(&hint_key)
                .map(|(hints, _)| hints.clone());
            let prepared =
                freshness::prepare(&resolved.context, source, kind, hints.as_ref(), cancel)?;
            self.counters
                .proof_micros
                .fetch_add(elapsed(proof_started), Ordering::Relaxed);
            let key = prepared.root_lookup_key.clone();
            let mut force_fresh_sdk = std::mem::take(&mut force_next_sdk);
            let candidate = {
                let mut state = self.lock();
                state.clock += 1;
                let now = state.clock;
                state.entries.get_mut(&key).map(|entry| {
                    entry.used = now;
                    (
                        entry.program.clone(),
                        entry.seal.clone(),
                        entry.build.clone(),
                    )
                })
            };
            if let Some((program, seal, build)) = candidate {
                let started = Instant::now();
                let sdk_lease = crate::sdk_cache::Lease::acquire(
                    &build.facts.target_directory,
                    cancel,
                    compiler.compile_timeout,
                )?;
                let sdk_proven = if let Some(lease) = &sdk_lease {
                    lease.verify(&prepared, cancel, false)?
                } else {
                    false
                };
                let validation = if sdk_proven {
                    freshness::validate(&prepared, &seal, cancel)?
                } else {
                    Validation::Miss {
                        scope: freshness::MissScope::Sdk,
                        reason: freshness::MissReason::InvalidArtifact,
                        hints: hints.clone().unwrap_or_default(),
                    }
                };
                self.counters
                    .proof_micros
                    .fetch_add(elapsed(started), Ordering::Relaxed);
                if matches!(validation, Validation::Hit) {
                    let started = Instant::now();
                    let fresh = build.compiler.check_cargo_fresh(
                        &build.facts,
                        &build.directory,
                        program.artifacts.as_deref(),
                        cancel,
                        &self.counters.cargo_build_invocations,
                    )?;
                    self.counters
                        .build_micros
                        .fetch_add(elapsed(started), Ordering::Relaxed);
                    if let Some(facts) = fresh {
                        self.record_build(&facts);
                        let root = serde_json::from_str::<serde_json::Value>(&facts.metadata)
                            .ok()
                            .and_then(|v| v["resolve"]["root"].as_str().map(String::from));
                        let root_fresh = facts.units.iter().any(|unit| {
                            Some(unit.package_id.as_str()) == root.as_deref()
                                && unit.kinds.iter().any(|k| k == "bin")
                                && unit.fresh
                        });
                        if root_fresh {
                            let started = Instant::now();
                            let sealed = freshness::seal_from_build(&prepared, &facts, cancel)?;
                            self.counters
                                .proof_micros
                                .fetch_add(elapsed(started), Ordering::Relaxed);
                            if !sdk_lease.as_ref().unwrap().unchanged(&prepared, cancel)? {
                                self.reject(&freshness::MissReason::ChangedDuringBuild);
                                force_fresh_sdk = true;
                            } else {
                                match sealed {
                                SealOutcome::Cacheable(post_seal)=>{
                                    if seal.same_products(&post_seal) && matches!(sdk_lease.as_ref().unwrap().publish(&prepared,cancel)?,crate::sdk_cache::Publication::Published) {
                                        check_cancel(cancel)?;self.counters.program_hits.fetch_add(1,Ordering::Relaxed);return Ok(program.instantiate())
                                    }
                                    self.reject(&freshness::MissReason::InvalidArtifact);force_fresh_sdk=true;
                                }
                                SealOutcome::ChangedDuringBuild=>return Err(ScriptError::Compile("Custom inputs changed during cached Cargo verification; apply again after saving inputs".into())),
                                SealOutcome::Uncacheable{hints,reason}=>{self.reject(&reason);self.remember_hints(hint_key.clone(),hints);force_fresh_sdk=true;},
                            }
                            }
                        }
                    }
                }
                if let Validation::Miss {
                    hints,
                    scope,
                    reason,
                } = validation
                {
                    self.reject(&reason);
                    force_fresh_sdk = scope == freshness::MissScope::Sdk;
                    self.remember_hints(hint_key.clone(), hints)
                }
                let old = {
                    let mut state = self.lock();
                    let old = state.retire_if_same(&key, &program);
                    self.update_resident(&state);
                    old
                };
                drop(old);
            }
            {
                let mut state = self.lock();
                // A different producer can publish after our first lookup.
                // Claiming a flight and checking publication are one decision.
                let claim = state.claim(&key);
                if claim == Claim::Ready {
                    continue;
                }
                if claim == Claim::Waiting {
                    self.counters
                        .coalesced_waits
                        .fetch_add(1, Ordering::Relaxed);
                    drop(state);
                    drop(admission);
                    let mut state = self.lock();
                    while state.flights.contains(&key) {
                        check_cancel(cancel)?;
                        state = self
                            .changed
                            .wait_timeout(state, Duration::from_millis(20))
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .0;
                    }
                    continue;
                }
            }
            let _flight = Flight {
                store: self,
                key: key.clone(),
            };
            self.counters.cache_misses.fetch_add(1, Ordering::Relaxed);
            let mut selected = resolved.compiler;
            let eligible = matches!(prepared.eligibility, Eligibility::Cacheable);
            let mut sdk_lease = None;
            let persistent = if eligible && !force_fresh_sdk {
                match sdk_namespace(&resolved.context.target_root, &prepared.sdk_key) {
                    Ok(path) => {
                        if let Some(lease) = crate::sdk_cache::Lease::acquire(
                            &path,
                            cancel,
                            compiler.compile_timeout,
                        )? {
                            if lease.verify(&prepared, cancel, true)? {
                                sdk_lease = Some(lease);
                                Some(path)
                            } else {
                                self.reject(&freshness::MissReason::InvalidArtifact);
                                None
                            }
                        } else {
                            None
                        }
                    }
                    Err(_) => {
                        self.reject(&freshness::MissReason::InvalidArtifact);
                        None
                    }
                }
            } else {
                None
            };
            if let Some(target) = persistent {
                selected.target_dir = Some(target);
            } else {
                let target = cleanup::OwnedTarget::create(
                    &resolved.context.target_root.join("uncached-generations"),
                    "sdk",
                )
                .map_err(ScriptError::Setup)?;
                selected.target_dir = Some(target.directory.clone());
                selected.owned_target = Some(target);
                sdk_lease = crate::sdk_cache::Lease::acquire(
                    selected.target_dir.as_ref().unwrap(),
                    cancel,
                    compiler.compile_timeout,
                )?;
            }
            let started = Instant::now();
            let mut facts = None;
            let script = selected.compile_uncached(
                source,
                kind,
                cancel,
                Some(&mut facts),
                Some(&self.counters.cargo_build_invocations),
            );
            self.counters
                .build_micros
                .fetch_add(elapsed(started), Ordering::Relaxed);
            if let Some(facts) = &facts {
                self.record_build(facts)
            }
            let script = script?;
            let facts = facts
                .ok_or_else(|| ScriptError::Setup("Compiled Custom build facts missing".into()))?;
            let started = Instant::now();
            let seal = freshness::seal_from_build(&prepared, &facts, cancel);
            self.counters
                .proof_micros
                .fetch_add(elapsed(started), Ordering::Relaxed);
            check_cancel(cancel)?;
            if let Some(lease) = &sdk_lease
                && !lease.unchanged(&prepared, cancel)?
            {
                self.reject(&freshness::MissReason::ChangedDuringBuild);
                force_next_sdk = true;
                continue;
            }
            match seal? {
                SealOutcome::Cacheable(seal) if eligible => {
                    let bytes = fs::metadata(&script.executable)
                        .map_err(|e| ScriptError::Setup(e.to_string()))?
                        .len();
                    let publication = if let Some(lease) = &sdk_lease {
                        lease.publish(&prepared, cancel)?
                    } else {
                        crate::sdk_cache::Publication::Unavailable
                    };
                    if matches!(publication, crate::sdk_cache::Publication::Changed) {
                        self.reject(&freshness::MissReason::ChangedDuringBuild);
                        force_next_sdk = true;
                        continue;
                    }
                    if matches!(publication, crate::sdk_cache::Publication::Published) {
                        self.publish(
                            key,
                            format!(
                                "{}:{}",
                                prepared.sdk_key,
                                selected.target_dir.as_ref().unwrap().display()
                            ),
                            Arc::new(Program::capture(&script)),
                            seal,
                            bytes,
                            Arc::new(BuildSpec {
                                compiler: selected,
                                facts,
                                directory: script._artifacts.as_ref().unwrap().directory.clone(),
                            }),
                        );
                    } else {
                        self.reject(&freshness::MissReason::InvalidArtifact);
                        self.counters.uncacheable.fetch_add(1, Ordering::Relaxed);
                        if let Some(artifacts) = &script._artifacts {
                            artifacts.trim_build_outputs()
                        }
                    }
                }
                SealOutcome::ChangedDuringBuild => return Err(ScriptError::Compile(
                    "Custom inputs changed during compilation; apply again after saving the inputs"
                        .into(),
                )),
                SealOutcome::Uncacheable { hints, reason } => {
                    self.reject(&reason);
                    #[cfg(test)]
                    eprintln!("Custom reuse seal rejected: {reason:?}");
                    if let Some(artifacts) = &script._artifacts {
                        artifacts.trim_build_outputs()
                    }
                    self.counters.uncacheable.fetch_add(1, Ordering::Relaxed);
                    self.remember_hints(hint_key, hints);
                }
                SealOutcome::Cacheable(_) => {
                    if let Some(artifacts) = &script._artifacts {
                        artifacts.trim_build_outputs()
                    }
                    self.counters.uncacheable.fetch_add(1, Ordering::Relaxed);
                }
            }
            ensure_usable_candidate(&script.executable)?;
            return Ok(script);
        }
    }
    fn record_build(&self, facts: &freshness::BuildFacts) {
        let root_id = serde_json::from_str::<serde_json::Value>(&facts.metadata)
            .ok()
            .and_then(|metadata| metadata["resolve"]["root"].as_str().map(String::from));
        for unit in &facts.units {
            let root = Some(unit.package_id.as_str()) == root_id.as_deref();
            let counter = if root {
                if !unit.kinds.iter().any(|kind| kind == "bin") {
                    continue;
                }
                if unit.fresh {
                    &self.counters.root_artifacts_fresh
                } else {
                    &self.counters.root_artifacts_built
                }
            } else if unit.fresh {
                &self.counters.dependency_artifacts_fresh
            } else {
                &self.counters.dependency_artifacts_built
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }
}
fn ensure_usable_candidate(executable: &Path) -> Result<(), ScriptError> {
    let usable = fs::symlink_metadata(executable).is_ok_and(|metadata| {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    });
    if usable {
        Ok(())
    } else {
        Err(ScriptError::Compile(
            "Compiled Custom executable became unusable during verification; apply again after checking the build files".into(),
        ))
    }
}
fn elapsed(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u64::MAX as u128) as u64
}
fn check_cancel(cancel: &AtomicBool) -> Result<(), ScriptError> {
    if cancel.load(Ordering::Acquire) {
        Err(ScriptError::Worker("compilation cancelled".into()))
    } else {
        Ok(())
    }
}
struct Admission<'a> {
    store: &'a CompiledProgramStore,
}
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        let mut state = self.store.lock();
        state.active -= 1;
        self.store
            .counters
            .in_flight
            .fetch_sub(1, Ordering::Relaxed);
        self.store.changed.notify_all();
    }
}
struct Flight<'a> {
    store: &'a CompiledProgramStore,
    key: String,
}
impl Drop for Flight<'_> {
    fn drop(&mut self) {
        self.store.lock().flights.remove(&self.key);
        self.store.changed.notify_all();
    }
}
fn sdk_namespace(base: &Path, key: &str) -> Result<PathBuf, ScriptError> {
    if key.len() != 64 || !key.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(ScriptError::Setup(
            "Invalid internal SDK content key".into(),
        ));
    }
    // MSVC's output-file handling still has practical MAX_PATH boundaries.
    // A short directory locator is not identity: the complete key below must
    // match before this namespace may be reused. Collisions use a fresh owner.
    let parent = base.join("sdk-v3");
    fs::create_dir_all(&parent).map_err(|e| ScriptError::Setup(e.to_string()))?;
    let parent = parent
        .canonicalize()
        .map_err(|e| ScriptError::Setup(e.to_string()))?;
    let path = parent.join(&key[..16]);
    let marker = path.join("module-engine-sdk-cache.txt");
    let contents = format!("ModuleEngine SDK dependency cache v1\n{key}\n");
    if !path.exists() {
        // Publish the complete marker and directory together. Never replace an
        // unmarked pre-existing directory, including an empty one.
        let staged =
            cleanup::OwnedTarget::create(&parent, "initializing").map_err(ScriptError::Setup)?;
        fs::write(
            staged.directory.join("module-engine-sdk-cache.txt"),
            &contents,
        )
        .map_err(|e| ScriptError::Setup(e.to_string()))?;
        match publish_namespace(&staged.directory, &path) {
            Ok(()) => staged.disarm_after_move(),
            Err(error) if !path.exists() => {
                return Err(ScriptError::Setup(format!(
                    "Cannot publish SDK cache namespace without replacement: {error}"
                )));
            }
            Err(_) => {}
        }
    }
    let actual = path
        .canonicalize()
        .map_err(|e| ScriptError::Setup(e.to_string()))?;
    let actual_marker = fs::symlink_metadata(&marker)
        .ok()
        .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .and_then(|_| fs::File::open(&marker).ok())
        .and_then(|file| {
            let mut bytes = Vec::new();
            file.take(256).read_to_end(&mut bytes).ok()?;
            String::from_utf8(bytes).ok()
        });
    if actual != path || actual_marker.as_deref() != Some(contents.as_str()) {
        return Err(ScriptError::Setup("SDK cache namespace is not an owned marked directory; choose a clean Custom target directory".into()));
    }

    Ok(path)
}

#[cfg(target_os = "linux")]
fn publish_namespace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    // Both directories are beneath the same owned cache parent. NOREPLACE
    // prevents Linux rename from adopting an existing empty directory.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(windows)]
fn publish_namespace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // Zero flags explicitly disallow replacing an existing destination.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
#[cfg(not(any(windows, target_os = "linux")))]
fn publish_namespace(_from: &Path, _to: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "Native program cache supports Windows and Linux",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .into()
    }
    #[test]
    fn uncacheable_candidate_must_remain_usable_before_return() {
        let fixture = TestArtifactGuard::new(&std::env::temp_dir(), "candidate-usability").unwrap();
        let executable = fixture.path().join("worker");
        fs::write(&executable, b"owned candidate").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(ensure_usable_candidate(&executable).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(ensure_usable_candidate(&executable).is_err());
        }
        fs::remove_file(&executable).unwrap();
        assert!(ensure_usable_candidate(&executable).is_err());
    }
    #[test]
    fn sdk_namespace_publishes_complete_markers_and_never_adopts_existing_directories() {
        let fixture = TestArtifactGuard::new(&std::env::temp_dir(), "sdk-publication").unwrap();
        let key = "a".repeat(64);
        let barrier = Arc::new(Barrier::new(9));
        let threads = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let root = fixture.path().to_path_buf();
                let key = key.clone();
                thread::spawn(move || {
                    barrier.wait();
                    sdk_namespace(&root, &key).unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let paths = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(paths.len(), 1);
        let foreign_key = "b".repeat(64);
        let foreign = fixture.path().join("sdk-v3").join(&foreign_key[..16]);
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("sentinel"), "foreign owner").unwrap();
        assert!(sdk_namespace(fixture.path(), &foreign_key).is_err());
        assert_eq!(
            fs::read_to_string(foreign.join("sentinel")).unwrap(),
            "foreign owner"
        );
        let empty_key = "c".repeat(64);
        let empty = fixture.path().join("sdk-v3").join(&empty_key[..16]);
        fs::create_dir(&empty).unwrap();
        assert!(sdk_namespace(fixture.path(), &empty_key).is_err());
        assert!(!empty.join("module-engine-sdk-cache.txt").exists());
    }
    #[test]
    fn sdk_namespace_short_locator_checks_full_identity_and_preserves_collision_owner() {
        let fixture = TestArtifactGuard::new(&std::env::temp_dir(), "sdk-collision").unwrap();
        let first_key = "a".repeat(64);
        let colliding_key = format!("{}{}", "a".repeat(16), "b".repeat(48));
        let first = sdk_namespace(fixture.path(), &first_key).unwrap();
        assert_eq!(first.file_name().unwrap(), &first_key[..16]);
        let marker = first.join("module-engine-sdk-cache.txt");
        let original_marker = fs::read(&marker).unwrap();
        let sentinel = first.join("owner-output");
        fs::write(&sentinel, b"first owner must survive").unwrap();

        // The short name collides, while the full content identities differ.
        // Production treats this error as a fresh owned generation, never as
        // permission to adopt, replace, or clean up the first namespace.
        assert!(sdk_namespace(fixture.path(), &colliding_key).is_err());
        assert_eq!(fs::read(&marker).unwrap(), original_marker);
        assert_eq!(fs::read(&sentinel).unwrap(), b"first owner must survive");
        assert_eq!(sdk_namespace(fixture.path(), &first_key).unwrap(), first);
    }
    #[test]
    fn bounded_admission_cancels_before_tool_or_filesystem_work() {
        let store = Arc::new(
            CompiledProgramStore::new(ProgramStoreLimits {
                max_in_flight: 1,
                ..Default::default()
            })
            .unwrap(),
        );
        let admission = store.admit(&AtomicBool::new(false)).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let child_store = store.clone();
        let child_cancel = cancelled.clone();
        let child =
            thread::spawn(move || child_store.admit(&child_cancel).err().unwrap().to_string());
        cancelled.store(true, Ordering::Release);
        assert!(child.join().unwrap().contains("cancelled"));
        assert_eq!(store.stats().in_flight, 1);
        drop(admission);
        assert_eq!(store.stats().in_flight, 0);
        for index in 0..100 {
            store.remember_hints(index.to_string(), DependencyHints::default());
        }
        assert_eq!(store.lock().hints.len(), store.limits.max_entries);
        assert_eq!(store.stats().resident_entries, 0);
        assert!(
            CompiledProgramStore::new(ProgramStoreLimits {
                max_contexts: 0,
                ..Default::default()
            })
            .is_err()
        );
        assert!(serde_json::to_value(store.stats()).unwrap()["program_hits"].is_number());
    }
    #[test]
    fn real_program_coalesces_preserves_independent_state_and_outlives_eviction() {
        if crate::cache_context::native_cache_test_child(
            "program_store::tests::real_program_coalesces_preserves_independent_state_and_outlives_eviction",
            false,
        ) {
            return;
        }
        let fixture = TestArtifactGuard::new(&std::env::temp_dir(), "program-reuse").unwrap();
        let store = Arc::new(
            CompiledProgramStore::new(ProgramStoreLimits {
                max_entries: 1,
                ..Default::default()
            })
            .unwrap(),
        );
        let compiler = ScriptCompiler::new(root(), fixture.path().join("work"))
            .with_program_store(store.clone());
        let source = r#"// module-engine-state: {"schema":"store.counter.v1","initial":{"count":0}}
static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn update(ctx: &mut ScriptContext) {
    ctx.state["count"] = serde_json::json!(ctx.state["count"].as_u64().unwrap()+1);
    ctx.emit_event(serde_json::json!(CALLS.fetch_add(1,std::sync::atomic::Ordering::SeqCst)));
}"#;
        let barrier = Arc::new(Barrier::new(3));
        let a_compiler = compiler.clone();
        let a_barrier = barrier.clone();
        let a = thread::spawn(move || {
            a_barrier.wait();
            a_compiler.compile(source)
        });
        let b_compiler = compiler.clone();
        let b_barrier = barrier.clone();
        let b = thread::spawn(move || {
            b_barrier.wait();
            b_compiler.compile(source)
        });
        barrier.wait();
        let a = a.join().unwrap();
        let b = b.join().unwrap();
        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!(store.stats().root_artifacts_built, 1, "{:?}", store.stats());
        assert_eq!(store.stats().cargo_build_invocations, 2);
        assert_eq!(store.stats().root_artifacts_fresh, 1);
        assert_eq!(store.stats().program_hits, 1);
        assert_eq!(a.executable(), b.executable());
        assert!(!Arc::ptr_eq(&a.instance, &b.instance));
        // A delayed failed validator must not remove a newer repair, and a
        // producer that looked up before publication must see Ready on claim.
        let retired = {
            let mut state = store.lock();
            let key = state.entries.keys().next().unwrap().clone();
            let old = state.entries.remove(&key).unwrap();
            let stale = old.program.clone();
            let repaired = Arc::new(Program::capture(&a));
            state.entries.insert(
                key.clone(),
                Entry {
                    program: repaired.clone(),
                    seal: old.seal.clone(),
                    sdk: old.sdk.clone(),
                    bytes: old.bytes,
                    used: old.used,
                    build: old.build.clone(),
                },
            );
            assert!(state.retire_if_same(&key, &stale).is_none());
            assert!(Arc::ptr_eq(&state.entries[&key].program, &repaired));
            assert_eq!(state.claim(&key), Claim::Ready);
            assert!(!state.flights.contains(&key));
            old
        };
        drop(retired);
        let context = ScriptContext::new(0.0, 0.016, Vec::new());
        let tick = a.tick(&context, Duration::from_secs(2)).unwrap();
        a.commit_tick(&tick).unwrap();
        assert_eq!(tick.events[0], 0);
        assert_eq!(a.state().unwrap()["count"], 1);
        let tick = b.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(tick.events[0], 0);
        assert_eq!(tick.state["count"], 1);
        assert_ne!(
            a.worker_process_id().unwrap(),
            b.worker_process_id().unwrap()
        );
        let c = compiler.compile(source).unwrap();
        assert_eq!(c.state().unwrap()["count"], 0);
        assert_eq!(store.stats().root_artifacts_built, 1);
        let old = a.executable().to_path_buf();
        let replacement = compiler
            .compile("fn speed(_:f64,base:f64)->f64{base+9.0}")
            .unwrap();
        assert_eq!(store.stats().resident_entries, 1);
        assert_eq!(store.stats().evictions, 1);
        assert!(old.is_file());
        let tick = a.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(tick.events[0], 1);
        a.export_precompiled(&fixture.path().join("exported"))
            .unwrap();
        drop(store);
        drop(compiler);
        assert!(old.is_file());
        drop(a);
        drop(b);
        drop(c);
        drop(replacement);
        finish_artifact_cleanup(Duration::from_secs(10)).unwrap();
        assert!(!old.exists());
    }
    #[test]
    fn real_missing_and_corrupt_root_and_sdk_products_repair_in_owned_scopes() {
        if crate::cache_context::native_cache_test_child(
            "program_store::tests::real_missing_and_corrupt_root_and_sdk_products_repair_in_owned_scopes",
            false,
        ) {
            return;
        }
        let fixture = TestArtifactGuard::new(&std::env::temp_dir(), "program-repair").unwrap();
        let mut store = Arc::new(CompiledProgramStore::default());
        let mut compiler = ScriptCompiler::new(root(), fixture.path().join("work"))
            .with_program_store(store.clone());
        compiler.target_dir = Some(fixture.reproducible_target("target").unwrap());
        let source = "fn speed(_:f64,_:f64)->f64{7.0}";
        let first = compiler.compile(source).unwrap();
        let initial_target = store
            .lock()
            .entries
            .values()
            .next()
            .unwrap()
            .build
            .facts
            .target_directory
            .clone();
        fs::write(first.executable(), b"corrupt copied executable").unwrap();
        let second = compiler.compile(source).unwrap();
        assert_ne!(first.executable(), second.executable());
        assert_eq!(
            store
                .lock()
                .entries
                .values()
                .next()
                .unwrap()
                .build
                .facts
                .target_directory,
            initial_target,
            "Root-only repair must preserve proven SDK deps"
        );
        fs::remove_file(second.executable()).unwrap();
        let third = compiler.compile(source).unwrap();
        assert_ne!(second.executable(), third.executable());
        let core = store
            .lock()
            .entries
            .values()
            .next()
            .unwrap()
            .build
            .facts
            .units
            .iter()
            .flat_map(|unit| &unit.artifacts)
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("libmodule_engine_core-") && name.ends_with(".rlib")
                    })
            })
            .unwrap()
            .clone();
        let old_time = fs::metadata(&core).unwrap().modified().unwrap();
        fs::write(&core, b"corrupt SDK archive").unwrap();
        fs::File::options()
            .write(true)
            .open(&core)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old_time))
            .unwrap();
        // A new owner and a changed root source have no in-memory seal. The
        // persisted receipt must reject this corrupt old-mtime SDK before Cargo.
        let previous = std::mem::replace(&mut store, Arc::new(CompiledProgramStore::default()));
        assert_eq!(previous.stats().root_artifacts_built, 3);
        compiler = compiler.with_program_store(store.clone());
        drop(previous);
        let new_source = format!("{source} // another root source");
        let source = new_source.as_str();
        let repaired = compiler.compile(source).unwrap();
        let repaired_target = store
            .lock()
            .entries
            .values()
            .next()
            .unwrap()
            .build
            .facts
            .target_directory
            .clone();
        assert_ne!(
            repaired_target, initial_target,
            "SDK repair must bypass stale Cargo dependency products"
        );
        assert!(
            repaired_target.starts_with(
                compiler
                    .target_dir
                    .as_ref()
                    .unwrap()
                    .join("uncached-generations")
            )
        );
        let context = ScriptContext::new(0.0, 0.016, Vec::new());
        assert_eq!(
            repaired
                .tick(&context, Duration::from_secs(2))
                .unwrap()
                .speed,
            Some(7.0)
        );
        let missing = store
            .lock()
            .entries
            .values()
            .next()
            .unwrap()
            .build
            .facts
            .units
            .iter()
            .flat_map(|unit| &unit.artifacts)
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("libmodule_engine_core-") && name.ends_with(".rlib")
                    })
            })
            .unwrap()
            .clone();
        fs::remove_file(missing).unwrap();
        let missing_repaired = compiler.compile(source).unwrap();
        let missing_target = store
            .lock()
            .entries
            .values()
            .next()
            .unwrap()
            .build
            .facts
            .target_directory
            .clone();
        assert_ne!(missing_target, repaired_target);
        let hit = compiler.compile(source).unwrap();
        assert_eq!(hit.executable(), missing_repaired.executable());
        assert_eq!(store.stats().root_artifacts_built, 2);
        assert!(store.stats().artifact_repairs >= 2);
        let executable = hit.executable().to_path_buf();
        drop(store);
        drop(compiler);
        assert!(executable.is_file());
        drop(hit);
        drop(repaired);
        drop(missing_repaired);
        drop(third);
        drop(second);
        drop(first);
        finish_artifact_cleanup(Duration::from_secs(10)).unwrap();
        assert!(!repaired_target.exists());
        assert!(!missing_target.exists());
    }

    #[test]
    fn real_include_content_change_with_preserved_mtime_never_reuses_stale_code() {
        if crate::cache_context::native_cache_test_child(
            "program_store::tests::real_include_content_change_with_preserved_mtime_never_reuses_stale_code",
            false,
        ) {
            return;
        }
        let fixture = TestArtifactGuard::new(&std::env::temp_dir(), "program-include").unwrap();
        let include = fixture.path().join("included.rs");
        fs::write(&include, "pub fn speed(_:f64,_:f64)->f64{7.0}\n").unwrap();
        let source = format!("include!({:?});", include.to_string_lossy());
        let store = Arc::new(CompiledProgramStore::default());
        let compiler = ScriptCompiler::new(root(), fixture.path().join("work"))
            .with_program_store(store.clone());
        let first = compiler.compile_kind(&source, ScriptKind::Speed).unwrap();
        assert_eq!(
            store.stats().resident_entries,
            0,
            "new external dependency must first become a bounded hint"
        );
        let second = compiler.compile_kind(&source, ScriptKind::Speed).unwrap();
        let third = compiler.compile_kind(&source, ScriptKind::Speed).unwrap();
        assert_eq!(second.executable(), third.executable());
        assert_eq!(store.stats().program_hits, 1);
        let old_time = fs::metadata(&include).unwrap().modified().unwrap();
        fs::write(&include, "pub fn speed(_:f64,_:f64)->f64{8.0}\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&include)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old_time))
            .unwrap();
        let changed = compiler.compile_kind(&source, ScriptKind::Speed).unwrap();
        let context = ScriptContext::new(0.0, 0.016, Vec::new());
        assert_eq!(
            changed
                .tick(&context, Duration::from_secs(2))
                .unwrap()
                .speed,
            Some(8.0)
        );
        assert_eq!(
            first.tick(&context, Duration::from_secs(2)).unwrap().speed,
            Some(7.0)
        );
        assert_ne!(changed.executable(), third.executable());
        assert_eq!(store.stats().root_artifacts_built, 3);
    }
}
