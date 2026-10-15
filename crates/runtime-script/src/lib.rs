//! Compiled Rust Custom workers exchanging owned, serialized command packets.
//! Worker processes isolate failure from the editor; they are not a security sandbox.

use module_engine_core::{ControlCommand, ScriptContext, SdkIdentity};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

mod persistent;
pub use persistent::StateDeclaration;
mod cleanup;
pub use cleanup::{
    TestArtifactGuard, TestArtifactLease, finish_artifact_cleanup, retain_test_artifacts,
};
mod jobs;
pub use jobs::{JobLease, finish_background_jobs, spawn_job};

const PACKET_MARKER: &str = "__MODULE_ENGINE_SCRIPT_PACKET__";
const MAX_PACKET: usize = 1024 * 1024;
const SDK_IDENTITY_ARGUMENT: &str = "--module-engine-sdk-identity";
static NEXT_WORKER: AtomicU64 = AtomicU64::new(1);

/// Supported compiled entry-point signatures. Other Rust in the source remains
/// ordinary compiled Rust, including constructs opaque to the visual editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptKind {
    Update,
    Speed,
}

/// Owned result from a successful worker invocation, with no cross-process ABI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptTick {
    pub commands: Vec<ControlCommand>,
    pub speed: Option<f64>,
    /// Candidate state, not yet accepted by the host's world/render transaction.
    #[serde(default)]
    pub state: serde_json::Value,
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
    #[serde(default)]
    pub state_schema: String,
    #[serde(skip)]
    program_id: u64,
    #[serde(skip)]
    revision: u64,
}

#[derive(Clone, Debug, Error)]
pub enum ScriptError {
    #[error("Script setup failed: {0}")]
    Setup(String),
    #[error("Rust compilation failed:\n{0}")]
    Compile(String),
    #[error("Script worker failed: {0}")]
    Worker(String),
    #[error("Script worker exceeded {0} ms and was stopped")]
    Timeout(u128),
    #[error("Invalid owned script packet: {0}")]
    Packet(String),
}

/// Candidate compiler. compile is blocking and belongs on the host's background
/// thread; every candidate has a distinct executable, so an active worker is
/// never overwritten. Compilation uses cached Cargo dependencies offline.
#[derive(Clone, Debug)]
pub struct ScriptCompiler {
    pub project_root: PathBuf,
    pub work_dir: PathBuf,
    pub cargo_home: Option<PathBuf>,
    pub target_dir: Option<PathBuf>,
    pub compile_timeout: Duration,
    /// Explicit CARGO/RUSTC overrides or restored project-local rustup shims.
    pub cargo_binary: Option<PathBuf>,
    pub rustc_binary: Option<PathBuf>,
    pub rustup_home: Option<PathBuf>,
    pub rustup_toolchain: Option<String>,
    /// Exact host/data API contract expected from the selected SDK. Source file
    /// inventory and compiler/toolchain pins are separate checks.
    pub sdk_identity: SdkIdentity,
    // Retain only a fresh test fixture's owner, never an existing project/cache.
    artifact_scope: Option<Arc<cleanup::TestArtifacts>>,
}

impl ScriptCompiler {
    pub fn new(project_root: PathBuf, work_dir: PathBuf) -> Self {
        let explicit_cargo = std::env::var_os("CARGO").map(PathBuf::from);
        let explicit_rustc = std::env::var_os("RUSTC").map(PathBuf::from);
        let tool_root = compiler_tool_root(&project_root);
        let portable_bin = tool_root.join(".tools/cargo/bin");
        let executable =
            |name: &str| portable_bin.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let portable_cargo = Some(executable("cargo")).filter(|p| p.is_file());
        let portable_rustc = Some(executable("rustc")).filter(|p| p.is_file());
        let uses_portable = (explicit_cargo.is_none() && portable_cargo.is_some())
            || (explicit_rustc.is_none() && portable_rustc.is_some());
        let pinned_rustup = portable_manifest_rustup(cfg!(windows), uses_portable);
        let cargo_home = resolve_cargo_home(
            &tool_root,
            std::env::var_os("CARGO_HOME").map(PathBuf::from),
        );
        Self {
            // Workers are a separate Cargo workspace. Sharing the editor's
            // build directory can replace dependency artifacts between its
            // unit tests and rustdoc, or contend with a running editor build.
            target_dir: Some(worker_target_dir(
                &tool_root,
                std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from),
            )),
            cargo_home,
            cargo_binary: resolve_tool_program(&tool_root, "cargo", explicit_cargo),
            rustc_binary: resolve_tool_program(&tool_root, "rustc", explicit_rustc),
            rustup_home: pinned_rustup.then(|| tool_root.join(".tools/rustup")),
            rustup_toolchain: pinned_rustup.then(|| project_toolchain(&tool_root)),
            artifact_scope: cleanup::fixture_for(&work_dir),
            project_root,
            work_dir,
            compile_timeout: Duration::from_secs(180),
            sdk_identity: SdkIdentity::current(),
        }
    }

    pub fn with_sdk_identity(mut self, sdk: SdkIdentity) -> Result<Self, ScriptError> {
        validate_supported_sdk(&sdk)?;
        self.sdk_identity = sdk;
        Ok(self)
    }

    /// Select an explicit update or speed function by parsing syntax, then let
    /// rustc type-check the complete source. No source is executed/interpreted here.
    pub fn compile(&self, source: &str) -> Result<CompiledScript, ScriptError> {
        let file = syn::parse_file(source).map_err(|e| ScriptError::Compile(e.to_string()))?;
        let has = |name: &str| {
            file.items
                .iter()
                .any(|i| matches!(i, syn::Item::Fn(f) if f.sig.ident == name))
        };
        let kind = if has("update") {
            ScriptKind::Update
        } else if has("speed") {
            ScriptKind::Speed
        } else {
            return Err(ScriptError::Compile(
                "expected fn update(ctx: &mut ScriptContext) or fn speed(time:f64,base:f64)->f64"
                    .into(),
            ));
        };
        self.compile_kind(source, kind)
    }

    /// Explicit mode also supports a function generated by an opaque Rust macro.
    pub fn compile_kind(
        &self,
        source: &str,
        kind: ScriptKind,
    ) -> Result<CompiledScript, ScriptError> {
        self.compile_kind_cancellable(source, kind, &AtomicBool::new(false))
    }

    /// A background owner may cancel an obsolete compile. No worker/candidate
    /// is published after cancellation; owned diagnostics remain available.
    pub fn compile_kind_cancellable(
        &self,
        source: &str,
        kind: ScriptKind,
        cancelled: &AtomicBool,
    ) -> Result<CompiledScript, ScriptError> {
        if cancelled.load(Ordering::Acquire) {
            return Err(ScriptError::Worker("compilation cancelled".into()));
        }
        validate_supported_sdk(&self.sdk_identity)?;
        if source.len() > MAX_PACKET {
            return Err(ScriptError::Compile("source exceeds 1 MiB".into()));
        }
        let state_declaration = StateDeclaration::from_source(source)?;
        let root = self
            .project_root
            .canonicalize()
            .map_err(|e| ScriptError::Setup(e.to_string()))?;
        let core = root.join("crates/engine-core");
        if !core.join("Cargo.toml").exists() {
            return Err(ScriptError::Setup(
                "engine-core Cargo.toml is missing".into(),
            ));
        }
        let selected_sdk = read_sdk_identity(&root)?;
        validate_sdk_match(&selected_sdk, &self.sdk_identity)?;
        let root_lock = fs::read_to_string(root.join("Cargo.lock")).map_err(|e| {
            ScriptError::Setup(format!("A restored root Cargo.lock is required: {e}"))
        })?;
        let root_packages = lock_packages(&root_lock)?;
        let json_versions: Vec<_> = root_packages
            .iter()
            .filter(|p| p.name == "serde_json" && p.version.starts_with("1."))
            .collect();
        if json_versions.len() != 1 {
            return Err(ScriptError::Setup(
                "Root Cargo.lock must identify one pinned serde_json 1.x package".into(),
            ));
        }
        let json_version = &json_versions[0].version;
        fs::create_dir_all(&self.work_dir).map_err(|e| ScriptError::Setup(e.to_string()))?;
        let name = format!(
            "module_engine_worker_{}_{}_{}",
            std::process::id(),
            NEXT_WORKER.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| ScriptError::Setup(error.to_string()))?
                .as_nanos(),
        );
        let target = self
            .target_dir
            .clone()
            .unwrap_or_else(|| self.work_dir.join(&name).join("target"));
        let target = if target.is_absolute() {
            target
        } else {
            root.join(target)
        };
        let artifacts = Arc::new(
            cleanup::CandidateArtifacts::create(
                &self.work_dir,
                &target,
                &name,
                self.artifact_scope.clone(),
            )
            .map_err(ScriptError::Setup)?,
        );
        let directory = &artifacts.directory;
        let result = (|| {
            fs::create_dir_all(directory.join("src"))
                .map_err(|e| ScriptError::Setup(e.to_string()))?;
            fs::write(directory.join("source.rs"), source)
                .map_err(|error| ScriptError::Setup(error.to_string()))?;
            // JSON strings are valid TOML basic strings for these owned path values.
            let core_text = core.to_string_lossy();
            // Windows canonicalize returns a verbatim prefix which Cargo interprets
            // as a path URL if represented as //?/C: in a manifest.
            let portable = if let Some(unc) = core_text.strip_prefix("\\\\?\\UNC\\") {
                format!("//{}", unc.replace('\\', "/"))
            } else {
                core_text
                    .strip_prefix("\\\\?\\")
                    .unwrap_or(&core_text)
                    .replace('\\', "/")
            };
            let core_path =
                serde_json::to_string(&portable).map_err(|e| ScriptError::Setup(e.to_string()))?;
            let manifest = format!(
                "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nmodule-engine-core = {{ path = {core_path} }}\nserde_json = \"={json_version}\"\n"
            );
            fs::write(directory.join("Cargo.toml"), manifest)
                .map_err(|e| ScriptError::Setup(e.to_string()))?;
            let worker_lock = directory.join("Cargo.lock");
            let seed = format!(
                "{root_lock}\n[[package]]\nname = \"{name}\"\nversion = \"0.0.0\"\ndependencies = [\n \"module-engine-core\",\n \"serde_json {json_version}\",\n]\n"
            );
            fs::write(&worker_lock, seed).map_err(|e| ScriptError::Setup(e.to_string()))?;
            fs::write(
                directory.join("src/user_program.rs"),
                user_module_source(source, kind),
            )
            .map_err(|e| ScriptError::Setup(e.to_string()))?;
            fs::write(directory.join("src/main.rs"), worker_source(kind))
                .map_err(|e| ScriptError::Setup(e.to_string()))?;
            let make_command = || {
                let mut command = Command::new(
                    self.cargo_binary
                        .as_deref()
                        .unwrap_or_else(|| Path::new("cargo")),
                );
                command
                    .env("CARGO_TARGET_DIR", &target)
                    .env("CARGO_INCREMENTAL", "0")
                    .env_remove("CARGO_HOME")
                    .current_dir(directory);
                if let Some(home) = &self.cargo_home {
                    command.env("CARGO_HOME", home);
                }
                if let Some(rustc) = &self.rustc_binary {
                    command.env("RUSTC", rustc);
                }
                if let Some(home) = &self.rustup_home {
                    command.env("RUSTUP_HOME", home);
                }
                if let Some(toolchain) = &self.rustup_toolchain {
                    command.env("RUSTUP_TOOLCHAIN", toolchain);
                }
                command
            };
            // Cargo may prune unused root-workspace packages and normalize the newly
            // inserted worker entry. Resolve before building, preserving seeded pins.
            let mut metadata = make_command();
            metadata
                .args(["metadata", "--offline", "--format-version", "1"])
                .arg("--manifest-path")
                .arg(directory.join("Cargo.toml"));
            let resolved = run_bounded(
                metadata,
                vec![],
                self.compile_timeout,
                cancelled,
                Some((&artifacts, "Cargo metadata")),
            )?;
            if !resolved.success {
                return Err(ScriptError::Compile(format!(
                    "{}{}",
                    resolved.stderr, resolved.stdout
                )));
            }
            let dependency_versions = verify_worker_lock(
                &root_lock,
                &fs::read_to_string(&worker_lock).map_err(|e| ScriptError::Setup(e.to_string()))?,
                &name,
            )?;
            let mut command = make_command();
            command
                .arg("build")
                .arg("--offline")
                .arg("--locked")
                .arg("--manifest-path")
                .arg(directory.join("Cargo.toml"))
                .env("CARGO_TARGET_DIR", &target)
                .env("CARGO_INCREMENTAL", "0")
                .current_dir(directory);
            if let Some(home) = &self.cargo_home {
                command.env("CARGO_HOME", home);
            }
            let output = run_bounded(
                command,
                vec![],
                self.compile_timeout,
                cancelled,
                Some((&artifacts, "Cargo build")),
            )?;
            if !output.success {
                return Err(ScriptError::Compile(format!(
                    "{}{}",
                    output.stderr, output.stdout
                )));
            }
            let built_executable = target
                .join("debug")
                .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if !built_executable.is_file() {
                return Err(ScriptError::Compile(
                    "Cargo succeeded without producing the worker executable".into(),
                ));
            }
            let executable = artifacts.executable_path();
            fs::copy(&built_executable, &executable)
                .map_err(|error| ScriptError::Setup(error.to_string()))?;
            artifacts.trim_build_outputs();
            // Query only the generated harness: no user entry point or simulation
            // runs. This verifies the API actually linked from the selected sources,
            // including same-version metadata/source mismatches, before hot apply.
            let mut probe = Command::new(&executable);
            probe.arg(SDK_IDENTITY_ARGUMENT).current_dir(&root);
            let identity = run_bounded(
                probe,
                vec![],
                self.compile_timeout,
                cancelled,
                Some((&artifacts, "Linked SDK probe")),
            )?;
            if !identity.success {
                return Err(ScriptError::Setup(format!(
                    "Compiled SDK identity probe failed: {}{}",
                    identity.stderr, identity.stdout
                )));
            }
            let linked_sdk: SdkIdentity = serde_json::from_str(&identity.stdout)
                .map_err(|error| ScriptError::Setup(format!("Compiled SDK identity: {error}")))?;
            validate_sdk_match(&linked_sdk, &self.sdk_identity)?;
            if cancelled.load(Ordering::Acquire) {
                return Err(ScriptError::Worker("compilation cancelled".into()));
            }
            artifacts.ready();
            Ok(CompiledScript {
                source: source.into(),
                kind,
                executable,
                working_dir: compiler_tool_root(&root),
                worker_lock,
                dependency_versions,
                state_declaration: state_declaration.clone(),
                instance: persistent::new_instance(&state_declaration),
                sdk_identity: linked_sdk,
                _artifacts: Some(artifacts.clone()),
                _retained_scopes: cleanup::fixture_for(&compiler_tool_root(&root))
                    .into_iter()
                    .collect(),
            })
        })();
        if let Err(error) = &result {
            artifacts.failed(&error.to_string());
        }
        result
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SdkManifest {
    schema_version: u32,
    sdk: SdkIdentity,
}

/// Inspect selected SDK metadata without compiler/device/worker creation. The
/// package's file inventory must independently verify the actual SDK sources.
pub fn read_sdk_identity(root: &Path) -> Result<SdkIdentity, ScriptError> {
    let path = root.join("engine-sdk.json");
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|file| file.take(16 * 1024 + 1).read_to_end(&mut bytes))
        .map_err(|error| {
            ScriptError::Setup(format!("Selected SDK metadata {}: {error}", path.display()))
        })?;
    if bytes.len() > 16 * 1024 {
        return Err(ScriptError::Setup(
            "Selected SDK metadata exceeds 16 KiB".into(),
        ));
    }
    let manifest: SdkManifest = serde_json::from_slice(&bytes)
        .map_err(|error| ScriptError::Setup(format!("Selected SDK metadata: {error}")))?;
    if manifest.schema_version != 1 {
        return Err(ScriptError::Setup(
            "Unsupported selected SDK metadata format".into(),
        ));
    }
    manifest.sdk.validate().map_err(ScriptError::Setup)?;
    Ok(manifest.sdk)
}

fn validate_supported_sdk(sdk: &SdkIdentity) -> Result<(), ScriptError> {
    sdk.validate().map_err(ScriptError::Setup)?;
    let supported = SdkIdentity::current();
    if sdk != &supported {
        return Err(ScriptError::Setup(format!(
            "SDK identity differs: this host supports exact version '{}' and data API '{}'",
            supported.version, supported.data_api_fingerprint
        )));
    }
    Ok(())
}

fn validate_sdk_match(actual: &SdkIdentity, expected: &SdkIdentity) -> Result<(), ScriptError> {
    validate_supported_sdk(expected)?;
    actual.validate().map_err(ScriptError::Setup)?;
    if actual != expected {
        return Err(ScriptError::Setup(format!(
            "Selected/compiled SDK identity version '{}' data API '{}' differs from expected version '{}' data API '{}'",
            actual.version,
            actual.data_api_fingerprint,
            expected.version,
            expected.data_api_fingerprint
        )));
    }
    Ok(())
}

/// Keep worker artifacts apart from the outer workspace's native build, even
/// when its target base is explicitly selected through the environment.
fn worker_target_dir(tool_root: &Path, inherited: Option<PathBuf>) -> PathBuf {
    inherited
        .unwrap_or_else(|| tool_root.join("target"))
        .join("custom-workers")
}

/// A Team SDK uses its containing installation's tools and caches.
fn compiler_tool_root(project_root: &Path) -> PathBuf {
    if project_root.file_name().is_some_and(|name| name == ".sdk")
        && !project_root.join("crates/editor/src/main.rs").is_file()
        && let Some(parent) = project_root.parent()
    {
        return parent.to_path_buf();
    }
    project_root.to_path_buf()
}

fn resolve_cargo_home(project_root: &Path, environment: Option<PathBuf>) -> Option<PathBuf> {
    resolve_cargo_home_for_platform(project_root, environment, cfg!(target_os = "linux"))
}

fn resolve_cargo_home_for_platform(
    project_root: &Path,
    environment: Option<PathBuf>,
    linux: bool,
) -> Option<PathBuf> {
    // The shipped project's legacy cache can belong to the Windows build. An
    // explicitly selected native Linux cache must stay isolated from it.
    if linux && environment.is_some() {
        return environment;
    }
    let local = project_root.join(".work/cargo-home");
    let sibling = project_root.parent().map(|p| p.join("cargo-home"));
    let portable = project_root.join(".tools/cargo");
    [Some(local), sibling, Some(portable)]
        .into_iter()
        .flatten()
        .find(|p| p.is_dir())
        .or(environment)
}

fn resolve_tool_program(root: &Path, name: &str, explicit: Option<PathBuf>) -> Option<PathBuf> {
    explicit.or_else(|| {
        let path = root
            .join(".tools/cargo/bin")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        path.is_file().then_some(path)
    })
}

/// The shipped bootstrap/manifest currently pins Windows MSVC only. Native
/// Linux tools may live under .tools too, but must retain their own installed
/// rustup home/toolchain (or explicitly inherited RUSTUP_* environment).
fn portable_manifest_rustup(windows: bool, portable_tools_selected: bool) -> bool {
    windows && portable_tools_selected
}

fn project_toolchain(root: &Path) -> String {
    fs::read(root.join("toolchain-manifest.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|manifest| {
            Some(format!(
                "{}-{}",
                manifest.get("rust")?.get("version")?.as_str()?,
                manifest.get("rust")?.get("target")?.as_str()?
            ))
        })
        .unwrap_or_else(|| "1.99.0-x86_64-pc-windows-msvc".into())
}

/// Successfully compiled immutable candidate. Replacing this value is the hot
/// apply operation; failed compilation never changes it or the host's world.
#[derive(Clone, Debug)]
pub struct CompiledScript {
    source: String,
    kind: ScriptKind,
    executable: PathBuf,
    working_dir: PathBuf,
    worker_lock: PathBuf,
    dependency_versions: Vec<(String, String)>,
    state_declaration: StateDeclaration,
    instance: std::sync::Arc<std::sync::Mutex<persistent::ScriptInstance>>,
    sdk_identity: SdkIdentity,
    // Struct fields drop in declaration order: the instance (and its child)
    // must close before the last artifact lease can delete the executable.
    _artifacts: Option<Arc<cleanup::CandidateArtifacts>>,
    // Precompiled package/asset fixtures also outlive asynchronous actors even
    // though their installed package files are never runtime-owned artifacts.
    _retained_scopes: Vec<Arc<cleanup::TestArtifacts>>,
}

impl CompiledScript {
    /// Asset reads by ordinary Rust code are relative to the project's folder,
    /// independently of the SDK/source folder used to compile the executable.
    pub fn asset_root(&self) -> &Path {
        &self.working_dir
    }

    /// Rebind assets without sharing or stealing a running worker. Accepted
    /// declared state is copied into a new instance when the root changes;
    /// process-local globals restart with that instance. Same-root rebinding is
    /// a no-op so it does not restart an already active worker.
    pub fn with_asset_root(mut self, root: &Path) -> Result<Self, ScriptError> {
        let root = root
            .canonicalize()
            .map_err(|e| ScriptError::Setup(e.to_string()))?;
        if !root.is_dir() {
            return Err(ScriptError::Setup("Asset root must be a directory".into()));
        }
        if self.working_dir == root {
            return Ok(self);
        }
        let state = self.state()?;
        self.instance = persistent::new_instance(&self.state_declaration);
        self.working_dir = root;
        if let Some(scope) = cleanup::fixture_for(&self.working_dir)
            && !self
                ._retained_scopes
                .iter()
                .any(|saved| Arc::ptr_eq(saved, &scope))
        {
            self._retained_scopes.push(scope);
        }
        self.migrate_state(state)?;
        Ok(self)
    }

    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn kind(&self) -> ScriptKind {
        self.kind
    }
    pub fn sdk_identity(&self) -> &SdkIdentity {
        &self.sdk_identity
    }
    pub fn executable(&self) -> &Path {
        &self.executable
    }
    /// Candidate lockfile resolved from the exported project's exact root pins.
    pub fn worker_lock(&self) -> &Path {
        &self.worker_lock
    }
    /// Verified dependency versions, excluding the generated worker package.
    pub fn dependency_versions(&self) -> &[(String, String)] {
        &self.dependency_versions
    }

    /// Preserve this candidate's exact source/build log after its final owner
    /// closes. Executables remain reproducible and failed history is bounded.
    pub fn retain_diagnostics(&self, reason: &str) {
        if let Some(artifacts) = &self._artifacts {
            artifacts.failed(reason);
        }
    }

    /// Evaluate with a reusable bounded JSON-lines worker. The returned state is
    /// a candidate: call commit_tick only after accepting world/render commands.
    pub fn tick(
        &self,
        context: &ScriptContext,
        timeout: Duration,
    ) -> Result<ScriptTick, ScriptError> {
        self.evaluate(context, timeout)
    }
}

/// Optional active-source owner. The editor may instead manage candidates itself.
pub struct ScriptHost {
    pub compiler: ScriptCompiler,
    active: Option<CompiledScript>,
}

impl ScriptHost {
    pub fn new(compiler: ScriptCompiler) -> Self {
        Self {
            compiler,
            active: None,
        }
    }
    pub fn active(&self) -> Option<&CompiledScript> {
        self.active.as_ref()
    }
    pub fn active_source(&self) -> Option<&str> {
        self.active.as_ref().map(CompiledScript::source)
    }
    /// Blocking convenience; call off the UI thread. Failure preserves active.
    pub fn compile_and_apply(&mut self, source: &str) -> Result<(), ScriptError> {
        let candidate = self.compiler.compile(source)?;
        self.apply(candidate)
    }
    pub fn apply(&mut self, candidate: CompiledScript) -> Result<(), ScriptError> {
        validate_supported_sdk(candidate.sdk_identity())?;
        if let Some(active) = &self.active {
            candidate.inherit_state_from(active)?;
        }
        self.active = Some(candidate);
        Ok(())
    }
    pub fn tick(
        &self,
        context: &ScriptContext,
        timeout: Duration,
    ) -> Result<ScriptTick, ScriptError> {
        self.active
            .as_ref()
            .ok_or_else(|| ScriptError::Worker("no active compiled script".into()))?
            .tick(context, timeout)
    }
}

/// Initial authored command program, portable across Object/ECS storage.
pub fn command_demo_source() -> String {
    "fn update(ctx: &mut ScriptContext) {\n    // The same IDs and local transforms work with both worlds.\n    let x = (ctx.time * 0.7).sin() as f32;\n    ctx.set_position(1, [x, 0.25, 0.0]);\n    ctx.set_color(1, [75, 205, 245, 255]);\n    ctx.set_render_parameter(\"tonemap\", \"exposure\", 1.4);\n}\n".into()
}

fn user_module_source(source: &str, kind: ScriptKind) -> String {
    // Only the generated compiler input is normalized. CompiledScript::source
    // always retains the exact authored bytes, including BOM/shebang/attributes.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let source = if source.starts_with("#!") && !source.starts_with("#![") {
        source
            .find('\n')
            .map(|index| &source[index..])
            .unwrap_or("")
    } else {
        source
    };
    let wrapper = match kind {
        ScriptKind::Update => "pub fn invoke(ctx: &mut ScriptContext) { let _: () = update(ctx); }",
        ScriptKind::Speed => "pub fn invoke(time: f64, base: f64) -> f64 { speed(time, base) }",
    };
    // Rust item names resolve independently of declaration order. Imports at the
    // end leave valid inner attributes at the start of this actual module file.
    format!("{source}\n#[allow(unused_imports)] use module_engine_core::*;\n{wrapper}\n")
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct LockedPackage {
    name: String,
    version: String,
    source: Option<String>,
}

// Cargo's generated lockfile package identity fields are quoted basic strings.
// Avoid another dependency merely to inspect this small fixed serialization.
fn lock_packages(text: &str) -> Result<BTreeSet<LockedPackage>, ScriptError> {
    let mut packages = BTreeSet::new();
    let mut current: Option<LockedPackage> = None;
    let flush = |current: &mut Option<LockedPackage>,
                 packages: &mut BTreeSet<LockedPackage>|
     -> Result<(), ScriptError> {
        if let Some(package) = current.take() {
            if package.name.is_empty() || package.version.is_empty() {
                return Err(ScriptError::Setup(
                    "Malformed Cargo.lock package identity".into(),
                ));
            }
            packages.insert(package);
        }
        Ok(())
    };
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            flush(&mut current, &mut packages)?;
            current = Some(LockedPackage::default());
        } else if line.starts_with('[') && !line.starts_with("[\"") {
            flush(&mut current, &mut packages)?;
        } else if let Some(package) = &mut current {
            for key in ["name", "version", "source"] {
                if let Some(value) = line.strip_prefix(&format!("{key} = ")) {
                    let value: String = serde_json::from_str(value)
                        .map_err(|e| ScriptError::Setup(format!("Invalid lockfile {key}: {e}")))?;
                    match key {
                        "name" => package.name = value,
                        "version" => package.version = value,
                        _ => package.source = Some(value),
                    }
                    break;
                }
            }
        }
    }
    flush(&mut current, &mut packages)?;
    if packages.is_empty() {
        return Err(ScriptError::Setup("Root Cargo.lock has no packages".into()));
    }
    Ok(packages)
}

fn verify_worker_lock(
    root: &str,
    worker: &str,
    name: &str,
) -> Result<Vec<(String, String)>, ScriptError> {
    let allowed = lock_packages(root)?;
    let selected = lock_packages(worker)?;
    let mut versions = vec![];
    for package in selected {
        if package.name == name {
            continue;
        }
        if !allowed.contains(&package) {
            return Err(ScriptError::Compile(format!(
                "Worker dependency {} {} ({:?}) is not pinned in the root Cargo.lock; restore the project dependencies before compiling Custom",
                package.name, package.version, package.source
            )));
        }
        versions.push((package.name, package.version));
    }
    Ok(versions)
}

fn worker_source(kind: ScriptKind) -> String {
    let invoke = match kind {
        ScriptKind::Update => "user_program::invoke(&mut context); let speed: Option<f64> = None;",
        ScriptKind::Speed => {
            "let result = user_program::invoke(context.time, context.base); if !result.is_finite() { return Err(\"speed returned a nonfinite value\".into()); } let speed = Some(result);"
        }
    };
    format!(
        r#"use std::io::{{BufRead, Write}};
use module_engine_core::{{ScriptContext, SdkIdentity}};
mod user_program;
fn run() -> Result<(), Box<dyn std::error::Error>> {{
    if std::env::args().nth(1).as_deref() == Some("{SDK_IDENTITY_ARGUMENT}") {{
        println!("{{}}", serde_json::to_string(&SdkIdentity::current())?);
        return Ok(());
    }}
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    loop {{
        let mut bytes = Vec::new();
        let count = std::io::Read::take(&mut reader, 1048577).read_until(b'\n', &mut bytes)?;
        if count == 0 {{ break; }}
        if bytes.len() > 1048576 {{ return Err("input too large".into()); }}
        let input: serde_json::Value = serde_json::from_slice(&bytes)?;
        let request_id = input.get("request_id").and_then(serde_json::Value::as_u64).ok_or("missing request_id")?;
        let sdk: SdkIdentity = serde_json::from_value(input.get("sdk").cloned().ok_or("missing SDK identity")?)?;
        if sdk != SdkIdentity::current() {{ return Err("SDK identity differs from linked data API".into()); }}
        let mut context: ScriptContext = serde_json::from_value(input.get("context").cloned().ok_or("missing context")?)?;
        context.commands.clear();
        context.emitted_events.clear();
        {invoke}
        if context.commands.len() > 4096 || context.emitted_events.len() > 4096 {{ return Err("too many commands/events".into()); }}
        let packet = serde_json::json!({{"request_id":request_id,"sdk":SdkIdentity::current(),"commands":context.commands,"speed":speed,"state":context.state,"events":context.emitted_events}});
        let output = serde_json::to_string(&packet)?;
        if output.len() > 1048576 {{ return Err("output too large".into()); }}
        println!("\n{PACKET_MARKER}{{}}", output);
        std::io::stdout().flush()?;
    }}
    Ok(())
}}
fn main() {{ if let Err(error) = run() {{ eprintln!("{{error}}"); std::process::exit(1); }} }}
"#
    )
}

struct ProcessOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

fn read_pipe(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<(Vec<u8>, bool)> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut result = Vec::new();
        let mut truncated = false;
        let mut buffer = [0_u8; 8192];
        while let Ok(count) = pipe.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let take = count.min(MAX_PACKET.saturating_sub(result.len()));
            result.extend_from_slice(&buffer[..take]);
            truncated |= take < count;
        }
        let _ = sender.send((result, truncated));
    });
    receiver
}

fn run_bounded(
    mut command: Command,
    input: Vec<u8>,
    timeout: Duration,
    cancelled: &AtomicBool,
    log: Option<(&cleanup::CandidateArtifacts, &str)>,
) -> Result<ProcessOutput, ScriptError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(ScriptError::Worker("compilation cancelled".into()));
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process = OwnedChild::spawn(&mut command)?;
    let child = &mut process;
    let stdout = read_pipe(child.stdout.take().unwrap());
    let stderr = read_pipe(child.stderr.take().unwrap());
    let mut stdin = child.stdin.take().unwrap();
    // A worker stuck before reading input cannot block the caller in write_all.
    thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let start = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| ScriptError::Worker(e.to_string()))?
        {
            Some(status) => break Ok(status),
            None if start.elapsed() >= timeout || cancelled.load(Ordering::Acquire) => {
                child.stop();
                break Err(if cancelled.load(Ordering::Acquire) {
                    ScriptError::Worker("compilation cancelled".into())
                } else {
                    ScriptError::Timeout(timeout.as_millis())
                });
            }
            None => thread::sleep(Duration::from_millis(2)),
        }
    };
    let collect = |receiver: mpsc::Receiver<(Vec<u8>, bool)>| -> Result<String, ScriptError> {
        let (bytes, truncated) = receiver
            .recv_timeout(Duration::from_millis(150))
            .map_err(|_| ScriptError::Worker("worker left an output pipe open".into()))?;
        if truncated {
            return Err(ScriptError::Packet("worker output exceeds 1 MiB".into()));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let stdout = collect(stdout);
    let stderr = collect(stderr);
    if let Some((artifacts, phase)) = log {
        artifacts.log(
            phase,
            stdout
                .as_ref()
                .map_or_else(|error| error.to_string(), Clone::clone)
                .as_str(),
            stderr
                .as_ref()
                .map_or_else(|error| error.to_string(), Clone::clone)
                .as_str(),
        );
    }
    Ok(ProcessOutput {
        success: status?.success(),
        stdout: stdout?,
        stderr: stderr?,
    })
}

#[derive(Debug)]
struct OwnedChild {
    child: std::process::Child,
    #[cfg(windows)]
    job: WindowsJob,
    #[cfg(target_os = "linux")]
    stopped: bool,
    #[cfg(target_os = "linux")]
    exit_status: Option<std::process::ExitStatus>,
}
impl OwnedChild {
    fn spawn(command: &mut Command) -> Result<Self, ScriptError> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW.
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // Set before exec, rather than racing Cargo/rustc or a worker's
            // first child after spawn. Descendants inherit this private group.
            command.process_group(0);
        }
        Self::new(
            command
                .spawn()
                .map_err(|error| ScriptError::Setup(error.to_string()))?,
        )
    }

    fn new(mut child: std::process::Child) -> Result<Self, ScriptError> {
        #[cfg(windows)]
        let job = WindowsJob::new(&child).map_err(|error| {
            let _ = child.kill();
            let _ = child.wait();
            ScriptError::Setup(format!("Cannot own child process tree: {error}"))
        })?;
        #[cfg(target_os = "linux")]
        {
            // SAFETY: getpgid only inspects this live/unreaped child. Refuse to
            // signal an inherited group which could also contain the editor.
            if unsafe { libc::getpgid(child.id() as libc::pid_t) } != child.id() as libc::pid_t {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ScriptError::Setup(
                    "Child was not started in its own Linux process group".into(),
                ));
            }
        }
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
            #[cfg(target_os = "linux")]
            stopped: false,
            #[cfg(target_os = "linux")]
            exit_status: None,
        })
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        #[cfg(target_os = "linux")]
        {
            if self.stopped {
                return Ok(self.exit_status);
            }
            // WNOWAIT keeps the group leader unreaped. Its PID cannot be reused
            // between observing exit and terminating the remaining owned group.
            // Child::try_wait would reap it too soon and allow a PGID reuse race.
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            // SAFETY: valid writable siginfo, this owned child PID, no handler.
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id() as libc::id_t,
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: waitid initialized siginfo (zero PID means still running).
            if unsafe { info.assume_init().si_pid() } == 0 {
                return Ok(None);
            }
            self.stop();
            Ok(self.exit_status)
        }
        #[cfg(not(target_os = "linux"))]
        self.child.try_wait()
    }

    fn stop(&mut self) {
        #[cfg(windows)]
        self.job.terminate();
        #[cfg(target_os = "linux")]
        {
            if self.stopped {
                return;
            }
            self.stopped = true;
            // SAFETY: the unreaped private group leader still reserves this ID.
            // Negative PID addresses exactly its group; never PID 0 or -1.
            unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
            let _ = self.child.kill();
            self.exit_status = self.child.wait().ok();
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
impl std::ops::Deref for OwnedChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.stop();
    }
}

// Own the actual process handles, rather than starting taskkill after cancellation.
// Its startup alone can outlast a short worker. Descendants inherit this job;
// terminating it covers Cargo/rustc and Custom-created children, without PID reuse.
#[cfg(windows)]
#[derive(Debug)]
struct WindowsJob(isize);
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const std::ffi::c_void, name: *const u16) -> isize;
    fn AssignProcessToJobObject(job: isize, process: isize) -> i32;
    fn SetInformationJobObject(
        job: isize,
        class: i32,
        data: *const std::ffi::c_void,
        size: u32,
    ) -> i32;
    fn TerminateJobObject(job: isize, code: u32) -> i32;
    fn QueryInformationJobObject(
        job: isize,
        class: i32,
        data: *mut std::ffi::c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
    fn CloseHandle(handle: isize) -> i32;
}
#[cfg(windows)]
impl WindowsJob {
    fn new(child: &std::process::Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: null requests an unnamed job with default security attributes.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let job = Self(handle);
        #[repr(C)]
        #[derive(Default)]
        struct BasicLimits {
            process_user_time: i64,
            job_user_time: i64,
            flags: u32,
            minimum_working_set: usize,
            maximum_working_set: usize,
            active_process_limit: u32,
            affinity: usize,
            priority_class: u32,
            scheduling_class: u32,
        }
        #[repr(C)]
        #[derive(Default)]
        struct ExtendedLimits {
            basic: BasicLimits,
            io_counters: [u64; 6],
            process_memory_limit: usize,
            job_memory_limit: usize,
            peak_process_memory: usize,
            peak_job_memory: usize,
        }
        let mut limits = ExtendedLimits::default();
        limits.basic.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        // SAFETY: the extended-limit layout is the Windows C ABI. Closing the
        // last non-inheritable handle also kills children after a host crash.
        if unsafe {
            SetInformationJobObject(
                handle,
                9,
                (&limits as *const ExtendedLimits).cast(),
                std::mem::size_of::<ExtendedLimits>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: both handles are live; Child owns its process handle.
        if unsafe { AssignProcessToJobObject(handle, child.as_raw_handle() as isize) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }
    fn terminate(&self) {
        #[repr(C)]
        #[derive(Default)]
        struct Accounting {
            times: [i64; 4],
            page_faults: u32,
            total_processes: u32,
            active_processes: u32,
            terminated_processes: u32,
        }
        // SAFETY: this wrapper exclusively owns the live job. Wait for all owned
        // descendants to leave before their artifact leases can be released.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
        loop {
            let mut accounting = Accounting::default();
            let success = unsafe {
                QueryInformationJobObject(
                    self.0,
                    1,
                    (&mut accounting as *mut Accounting).cast(),
                    std::mem::size_of::<Accounting>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if success == 0 || accounting.active_processes == 0 {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}
#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        self.terminate();
        // SAFETY: close exactly the handle returned by CreateJobObjectW.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{EcsWorld, ObjectsWorld, RenderOverrides, WorldProvider};
    use std::collections::BTreeMap;

    fn owned_program(source: &str, sdk_identity: SdkIdentity) -> CompiledScript {
        let declaration = StateDeclaration {
            schema: "fixture.counter/1".into(),
            initial: serde_json::json!({"count":0}),
        };
        CompiledScript {
            source: source.into(),
            kind: ScriptKind::Update,
            executable: PathBuf::from("not-started"),
            working_dir: PathBuf::from("."),
            worker_lock: PathBuf::new(),
            dependency_versions: vec![],
            state_declaration: declaration.clone(),
            instance: persistent::new_instance(&declaration),
            sdk_identity,
            _artifacts: None,
            _retained_scopes: vec![],
        }
    }

    #[test]
    fn selected_sdk_metadata_rejects_before_cargo_or_candidate_files_are_created() {
        let artifacts =
            TestArtifactGuard::new(&std::env::temp_dir(), "module-sdk-metadata").unwrap();
        let root = artifacts.path().to_path_buf();
        fs::create_dir_all(root.join("crates/engine-core")).unwrap();
        fs::write(
            root.join("crates/engine-core/Cargo.toml"),
            "not read by Cargo",
        )
        .unwrap();
        let work = root.join("candidate-work");
        let mut compiler = ScriptCompiler::new(root.clone(), work.clone());
        compiler.cargo_binary = Some(root.join("must-not-start-cargo"));
        let missing = compiler
            .compile("fn update(_: &mut ScriptContext) {}")
            .unwrap_err();
        assert!(missing.to_string().contains("engine-sdk.json"));
        let mut wrong_version = SdkIdentity::current();
        wrong_version.version.push_str("-different");
        let mut wrong_api = SdkIdentity::current();
        wrong_api.data_api_fingerprint = "module-engine-data-api/v1".into();
        for sdk in [wrong_version, wrong_api] {
            let bytes =
                serde_json::to_vec(&serde_json::json!({"schema_version":1,"sdk":sdk})).unwrap();
            fs::write(root.join("engine-sdk.json"), &bytes).unwrap();
            let error = compiler
                .compile("fn update(_: &mut ScriptContext) {}")
                .unwrap_err();
            assert!(error.to_string().contains("SDK"), "{error}");
            assert!(!work.exists());
            assert_eq!(fs::read(root.join("engine-sdk.json")).unwrap(), bytes);
        }
        let mut unsupported = SdkIdentity::current();
        unsupported.version = "future-version".into();
        assert!(compiler.with_sdk_identity(unsupported).is_err());
    }

    #[test]
    fn incompatible_sdk_apply_preserves_last_accepted_program_and_owned_state() {
        let compiler = ScriptCompiler::new(PathBuf::from("."), PathBuf::from("not-created"));
        let mut host = ScriptHost::new(compiler);
        let previous = owned_program("previous", SdkIdentity::current());
        previous
            .migrate_state(serde_json::json!({"count":7}))
            .unwrap();
        host.apply(previous.clone()).unwrap();
        let mut wrong = SdkIdentity::current();
        wrong.version.push_str("-other");
        let candidate = owned_program("candidate", wrong);
        candidate
            .migrate_state(serde_json::json!({"count":99}))
            .unwrap();
        assert!(candidate.inherit_state_from(&previous).is_err());
        assert!(host.apply(candidate).is_err());
        assert_eq!(host.active_source(), Some("previous"));
        assert_eq!(
            host.active().unwrap().state().unwrap(),
            serde_json::json!({"count":7})
        );
        assert_eq!(previous.state().unwrap(), serde_json::json!({"count":7}));
        assert_eq!(previous.worker_process_id().unwrap(), None);
        let compatible = owned_program("compatible", SdkIdentity::current());
        host.apply(compatible).unwrap();
        assert_eq!(host.active_source(), Some("compatible"));
        assert_eq!(
            host.active().unwrap().state().unwrap(),
            serde_json::json!({"count":7})
        );
    }

    #[test]
    fn compiler_assets_classification_keeps_full_sources_named_sdk_in_their_own_package() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let artifacts =
            TestArtifactGuard::new(&workspace.join(".work/test-temp"), "compiler-root").unwrap();
        let base = artifacts.path();
        let full = base.join("full/.sdk");
        let team = base.join("team/.sdk");
        fs::create_dir_all(full.join("crates/editor/src")).unwrap();
        fs::create_dir_all(team.join("crates/engine-core")).unwrap();
        fs::write(full.join("crates/editor/src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(team.join("crates/engine-core/Cargo.toml"), "").unwrap();
        assert_eq!(compiler_tool_root(&full), full);
        assert_eq!(compiler_tool_root(&team), base.join("team"));
    }

    #[test]
    fn binding_assets_preserves_declared_state_without_stealing_active_worker() {
        let compiler_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let artifacts =
            TestArtifactGuard::new(&compiler_root.join(".work/test-temp"), "script-assets")
                .unwrap();
        let base = artifacts.path();
        let first_root = base.join("first");
        let second_root = base.join("second");
        fs::create_dir_all(first_root.join("assets")).unwrap();
        fs::create_dir_all(second_root.join("assets")).unwrap();
        fs::write(first_root.join("assets/marker.txt"), "first project").unwrap();
        fs::write(second_root.join("assets/marker.txt"), "second project").unwrap();
        let source = r#"// module-engine-state: {"schema":"assets.counter.v1","initial":{"count":0,"asset":""}}
fn update(ctx: &mut ScriptContext) {
    ctx.state["count"] = serde_json::json!(ctx.state["count"].as_u64().unwrap() + 1);
    ctx.state["asset"] = serde_json::json!(std::fs::read_to_string("assets/marker.txt").unwrap());
}
"#;
        let compiler = ScriptCompiler::new(compiler_root.clone(), base.join("compile"));
        // Exercise the default compiler, not just target-path construction:
        // compiling a real worker must leave the outer Cargo/rustdoc inputs
        // byte/metadata-identical in its native dependency directory.
        // The test binary's directory is the actual outer deps directory in
        // debug/release and native/explicit-target builds alike.
        let native_dependencies = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let native_artifacts = || {
            fs::read_dir(&native_dependencies)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with("libmodule_engine_core-") && name.ends_with(".rlib")
                })
                .map(|entry| {
                    let metadata = entry.metadata().unwrap();
                    (entry.path(), (metadata.len(), metadata.modified().unwrap()))
                })
                .collect::<BTreeMap<_, _>>()
        };
        let before = native_artifacts();
        assert!(
            !before.is_empty(),
            "the outer build must have actual core artifacts"
        );
        let active = compiler
            .compile_kind(source, ScriptKind::Update)
            .unwrap()
            .with_asset_root(&first_root)
            .unwrap();
        #[cfg(not(target_os = "linux"))]
        assert_eq!(active.executable().parent(), active.worker_lock().parent());
        #[cfg(target_os = "linux")]
        assert!(
            active
                .executable()
                .starts_with(compiler.target_dir.as_ref().unwrap().join("executables"))
        );
        #[cfg(not(target_os = "linux"))]
        assert!(
            !active
                .executable()
                .starts_with(compiler.target_dir.as_ref().unwrap())
        );
        assert_eq!(
            native_artifacts(),
            before,
            "worker compilation replaced outer Cargo artifacts"
        );
        let context = ScriptContext::new(0., 1. / 60., vec![]);
        let active_executable = active.executable().to_path_buf();
        let active_source = active.worker_lock().parent().unwrap().join("source.rs");
        let tick = active.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(tick.state["asset"], "first project");
        active.commit_tick(&tick).unwrap();
        let pid = active.worker_process_id().unwrap().unwrap();
        let same = active.clone().with_asset_root(&first_root).unwrap();
        assert_eq!(same.worker_process_id().unwrap(), Some(pid));
        let rebound = active.clone().with_asset_root(&second_root).unwrap();
        assert_eq!(rebound.state().unwrap()["count"], 1);
        assert!(rebound.worker_process_id().unwrap().is_none());
        let next = rebound.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(next.state["asset"], "second project");
        assert_eq!(next.state["count"], 2);
        rebound.commit_tick(&next).unwrap();
        assert_eq!(active.worker_process_id().unwrap(), Some(pid));
        assert_eq!(active.state().unwrap()["asset"], "first project");
        assert_eq!(active.state().unwrap()["count"], 1);
        let next = active.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(next.state["asset"], "first project");
        active.commit_tick(&next).unwrap();
        let hot_source = source.replace(" + 1", " + 3");
        let candidate = compiler
            .compile_kind(&hot_source, ScriptKind::Update)
            .unwrap()
            .with_asset_root(&first_root)
            .unwrap();
        candidate.inherit_state_from(&active).unwrap();
        let next = candidate.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(next.state["asset"], "first project");
        assert_eq!(next.state["count"], 5);
        drop(candidate);
        // A runtime diagnostic is not a completed failure: the last accepted
        // candidate must remain restartable even while other failures age out.
        active.retain_diagnostics("runtime error on a still accepted program");
        active.stop_worker().unwrap();
        assert!(active.worker_process_id().unwrap().is_none());
        for index in 0..20 {
            let failure = cleanup::CandidateArtifacts::create(
                &compiler.work_dir,
                compiler.target_dir.as_ref().unwrap(),
                &format!(
                    "module_engine_worker_completed_failure_{}_{}",
                    std::process::id(),
                    index
                ),
                compiler.artifact_scope.clone(),
            )
            .unwrap();
            failure.failed("completed independent compile failure");
        }
        finish_artifact_cleanup(Duration::from_secs(5)).unwrap();
        assert!(
            active_executable.is_file(),
            "retention pruned a paused accepted executable"
        );
        assert_eq!(fs::read_to_string(&active_source).unwrap(), source);
        let restarted = active.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(restarted.state["asset"], "first project");
        assert_eq!(restarted.state["count"], 3);
        active.commit_tick(&restarted).unwrap();
        drop(rebound);
        drop(same);
        drop(active);
        finish_artifact_cleanup(Duration::from_secs(5)).unwrap();
        assert!(
            !active_executable.exists(),
            "last fork must stop before its executable is retired"
        );
        assert!(
            active_source.is_file(),
            "test source survives until the fixture outcome is known"
        );
        assert!(
            compiler.target_dir.as_ref().unwrap().is_dir(),
            "shared dependencies survive retiring candidates"
        );
    }

    #[test]
    fn real_persistent_state_is_transactional_hot_applicable_and_precompiled_relocatable() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let artifacts =
            TestArtifactGuard::new(&root.join(".work/test-temp"), "state-test").unwrap();
        let work = artifacts.path().to_path_buf();
        let mut compiler = ScriptCompiler::new(root.clone(), work.clone());
        compiler.cargo_home = resolve_cargo_home(&root, None);
        let source = r#"// module-engine-state: {"schema":"counter.v1","initial":{"count":0,"asset":""}}
fn update(ctx: &mut ScriptContext) {
    let count = ctx.state["count"].as_u64().unwrap() + 1;
    ctx.state["count"] = serde_json::json!(count);
    ctx.state["asset"] = serde_json::json!(std::fs::read_to_string("assets/marker.txt").unwrap_or_default());
    let id = if ctx.input["reject"].as_bool().unwrap_or(false) { 999999 } else { 1 };
    ctx.set_position(id, [count as f32, 0.0, 0.0]);
    ctx.emit_event(serde_json::json!({"Custom":{"name":"counter","payload":count}}));
}
"#;
        let active = compiler.compile(source).unwrap();
        let objects = ObjectsWorld::demo().objects().unwrap();
        let mut context = ScriptContext::new(1.0, 0.016, objects.clone());
        context.input = serde_json::json!({"reject":true});
        let rejected = active.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(rejected.state["count"], 1);
        assert_eq!(active.state().unwrap()["count"], 0);
        let mut command_context = context.clone();
        command_context.commands = rejected.commands.clone();
        let (world, _) = command_context.split_commands();
        assert!(module_engine_core::validate_world_commands(&objects, &world).is_err());
        active.reject_tick(&rejected).unwrap();
        assert!(active.worker_process_id().unwrap().is_none());
        assert!(active.commit_tick(&rejected).is_err());
        assert_eq!(active.state().unwrap()["count"], 0);

        context.input = serde_json::json!({"reject":false});
        let accepted = active.tick(&context, Duration::from_secs(2)).unwrap();
        let pid = active.worker_process_id().unwrap().unwrap();
        assert_eq!(accepted.state["count"], 1);
        assert_eq!(accepted.events[0]["Custom"]["name"], "counter");
        active.commit_tick(&accepted).unwrap();
        assert!(active.commit_tick(&accepted).is_err());
        let next = active.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(active.worker_process_id().unwrap(), Some(pid));
        assert_eq!(next.state["count"], 2);
        active.commit_tick(&next).unwrap();
        let fork = active.fork_instance();
        assert_eq!(fork.state().unwrap()["count"], 0);
        let fork_tick = fork.tick(&context, Duration::from_secs(2)).unwrap();
        fork.commit_tick(&fork_tick).unwrap();
        assert_eq!(fork.state().unwrap()["count"], 1);
        assert_eq!(active.state().unwrap()["count"], 2);
        assert_ne!(
            fork.worker_process_id().unwrap(),
            active.worker_process_id().unwrap()
        );

        let mut rejected_sdk_host = ScriptHost::new(compiler.clone());
        rejected_sdk_host.apply(active.clone()).unwrap();
        rejected_sdk_host
            .compiler
            .sdk_identity
            .version
            .push_str("-incompatible");
        assert!(rejected_sdk_host.compile_and_apply(source).is_err());
        assert_eq!(rejected_sdk_host.active_source(), Some(source));
        assert_eq!(
            rejected_sdk_host.active().unwrap().state().unwrap()["count"],
            2
        );
        assert_eq!(active.worker_process_id().unwrap(), Some(pid));
        let after_sdk_rejection = rejected_sdk_host
            .tick(&context, Duration::from_secs(2))
            .unwrap();
        assert_eq!(after_sdk_rejection.state["count"], 3);
        assert_eq!(active.state().unwrap()["count"], 2);
        assert_eq!(active.sdk_identity(), &SdkIdentity::current());

        let edited = source.replace("unwrap() + 1", "unwrap() + 2");
        let candidate = compiler.compile(&edited).unwrap();
        candidate.inherit_state_from(&active).unwrap();
        assert_eq!(candidate.state().unwrap()["count"], 2);
        let tick = candidate.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(tick.state["count"], 4);
        assert!(active.commit_tick(&tick).is_err());
        candidate.commit_tick(&tick).unwrap();

        let incompatible = compiler
            .compile(&edited.replace("counter.v1", "counter.v2"))
            .unwrap();
        assert!(incompatible.inherit_state_from(&candidate).is_err());
        assert_eq!(candidate.state().unwrap()["count"], 4);
        incompatible
            .migrate_state(serde_json::json!({"count":40,"asset":"migrated"}))
            .unwrap();
        assert!(
            incompatible
                .migrate_state(serde_json::json!({"count":"wrong","asset":""}))
                .is_err()
        );
        let migrated = incompatible.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(migrated.state["count"], 42);
        incompatible.reset_state().unwrap();
        assert!(incompatible.commit_tick(&migrated).is_err());
        assert_eq!(incompatible.state().unwrap()["count"], 0);

        let invalid = compiler
            .compile(&source.replace(
                "serde_json::json!(count);",
                "serde_json::json!(\"bad type\");",
            ))
            .unwrap();
        assert!(matches!(
            invalid.tick(&context, Duration::from_secs(2)),
            Err(ScriptError::Packet(_))
        ));
        assert_eq!(invalid.state().unwrap()["count"], 0);
        assert!(invalid.worker_process_id().unwrap().is_none());
        let panic = compiler
            .compile("fn update(_: &mut ScriptContext) { panic!(\"intentional test failure\"); }")
            .unwrap();
        assert!(panic.tick(&context, Duration::from_secs(2)).is_err());
        assert!(panic.worker_process_id().unwrap().is_none());
        let noisy = compiler
            .compile("fn update(_: &mut ScriptContext) { print!(\"{}\", \"x\".repeat(1048577)); }")
            .unwrap();
        assert!(matches!(
            noisy.tick(&context, Duration::from_secs(2)),
            Err(ScriptError::Packet(_))
        ));
        assert!(noisy.worker_process_id().unwrap().is_none());
        let mut oversized = context.clone();
        oversized.events = vec![serde_json::json!("x".repeat(MAX_PACKET))];
        assert!(matches!(
            active.tick(&oversized, Duration::from_secs(2)),
            Err(ScriptError::Packet(_))
        ));
        assert_eq!(active.state().unwrap()["count"], 2);
        assert!(active.worker_process_id().unwrap().is_none());

        let export = work.join("export/scripts/counter");
        let manifest = candidate.export_precompiled(&export).unwrap();
        let relocated = work.join("relocated-player");
        let destination = relocated.join("scripts/counter");
        fs::create_dir_all(&destination).unwrap();
        fs::create_dir_all(relocated.join("assets")).unwrap();
        fs::write(relocated.join("assets/marker.txt"), "relocated asset").unwrap();
        for file in [
            "script.json",
            &format!("worker{}", std::env::consts::EXE_SUFFIX),
        ] {
            fs::copy(export.join(file), destination.join(file)).unwrap();
        }
        let loaded =
            CompiledScript::load_precompiled(&destination.join("script.json"), &relocated).unwrap();
        assert!(loaded.worker_lock().as_os_str().is_empty());
        assert_eq!(loaded.state().unwrap()["count"], 0);
        let standalone = loaded.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(standalone.state["count"], 2);
        assert_eq!(standalone.state["asset"], "relocated asset");
        loaded.commit_tick(&standalone).unwrap();
        let mut wrong: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        wrong["executable"] = serde_json::json!("../worker.exe");
        fs::write(
            destination.join("bad-script.json"),
            serde_json::to_vec(&wrong).unwrap(),
        )
        .unwrap();
        assert!(
            CompiledScript::load_precompiled(&destination.join("bad-script.json"), &relocated)
                .is_err()
        );
    }

    #[test]
    fn actual_compiled_custom_hot_apply_changes_both_worlds_and_exposure_and_bounds_workers() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let artifacts =
            TestArtifactGuard::new(&root.join(".work/test-temp"), "script-test").unwrap();
        let work = artifacts.path().to_path_buf();
        let compiler = ScriptCompiler::new(root.clone(), work.clone());
        // Retain the Windows project/sibling cache regression. Native Linux
        // subprocesses keep the constructor's explicitly selected Cargo home,
        // so testing a shared source checkout does not write its Windows cache.
        #[cfg(windows)]
        let compiler = {
            let mut compiler = compiler;
            compiler.cargo_home = resolve_cargo_home(&root, None);
            assert!(compiler.cargo_home.is_some());
            compiler
        };
        // The default custom-workers dependency target is shared across test
        // fixtures and separate from the outer native Cargo build.
        let mut host = ScriptHost::new(compiler);
        let source = command_demo_source();
        host.compile_and_apply(&source).unwrap();
        let root_lock = fs::read_to_string(host.compiler.project_root.join("Cargo.lock")).unwrap();
        let root_packages = lock_packages(&root_lock).unwrap();
        let active = host.active().unwrap();
        assert!(active.worker_lock().is_file());
        assert!(active.dependency_versions().iter().all(|(name, version)| {
            root_packages
                .iter()
                .any(|p| &p.name == name && &p.version == version)
        }));
        assert!(
            active
                .dependency_versions()
                .iter()
                .any(|(name, _)| name == "serde_json")
        );
        let mut objects = ObjectsWorld::demo();
        let mut ecs = EcsWorld::demo();
        let mut context = ScriptContext::new(2.0, 0.016, objects.objects().unwrap());
        let tick = host.tick(&context, Duration::from_secs(2)).unwrap();
        context.commands = tick.commands;
        let (world, render) = context.split_commands();
        objects.apply_commands(&world).unwrap();
        ecs.apply_commands(&world).unwrap();
        assert_eq!(objects.snapshot(), ecs.snapshot());
        assert_ne!(
            objects.objects().unwrap()[0].transform.translation[0],
            -1.35
        );
        assert_eq!(objects.objects().unwrap()[0].color, [75, 205, 245, 255]);
        let mut overrides = RenderOverrides::default();
        overrides.apply_commands(&render).unwrap();
        assert_eq!(overrides.parameters["tonemap"]["exposure"], 1.4);
        let bad = "fn update(ctx:&mut ScriptContext) { ctx.does_not_exist(); }";
        assert!(host.compile_and_apply(bad).is_err());
        assert_eq!(host.active_source(), Some(source.as_str()));
        finish_artifact_cleanup(Duration::from_secs(5)).unwrap();
        let failed = fs::read_dir(&work)
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                fs::read_to_string(entry.path().join("source.rs"))
                    .ok()
                    .as_deref()
                    == Some(bad)
            })
            .expect("failed source must be retained with its compiler diagnostics")
            .path();
        assert!(
            fs::read_to_string(failed.join("build.log"))
                .unwrap()
                .contains("does_not_exist")
        );
        assert!(failed.join("failure.txt").is_file());
        assert!(
            !failed
                .join(format!("worker{}", std::env::consts::EXE_SUFFIX))
                .exists()
        );
        assert!(
            host.active().unwrap().executable().is_file(),
            "failed compilation must retain the last accepted binary"
        );
        let asynchronous =
            "async fn update(ctx:&mut ScriptContext) { ctx.set_color(1,[1,2,3,255]); }";
        assert!(host.compile_and_apply(asynchronous).is_err());
        assert_eq!(host.active_source(), Some(source.as_str()));
        assert!(
            host.compile_and_apply("fn update(_ctx:&mut ScriptContext)->u8 { 7 }")
                .is_err()
        );
        assert_eq!(host.active_source(), Some(source.as_str()));
        assert!(
            !host
                .tick(&context, Duration::from_secs(2))
                .unwrap()
                .commands
                .is_empty()
        );
        host.compile_and_apply(
            "fn update(ctx:&mut ScriptContext) { ctx.set_color(1,[255,10,10,255]); }",
        )
        .unwrap();
        assert_ne!(host.active_source(), Some(source.as_str()));
        let tick = host.tick(&context, Duration::from_secs(2)).unwrap();
        assert_eq!(tick.commands.len(), 1);
        host.compile_and_apply("fn speed(time:f64,base:f64)->f64 { base*3.0+time }")
            .unwrap();
        assert_eq!(
            host.tick(&context, Duration::from_secs(2)).unwrap().speed,
            Some(5.0)
        );
        let attributed = "\u{feff}#!/usr/bin/env rust-script\n#![allow(dead_code)]\nfn helper()->u8 { 42 }\nfn update(ctx:&mut ScriptContext) { ctx.set_color(1,[helper(),10,20,255]); }\n";
        host.compile_and_apply(attributed).unwrap();
        assert_eq!(host.active_source(), Some(attributed));
        let tick = host.tick(&context, Duration::from_secs(2)).unwrap();
        assert!(
            matches!(&tick.commands[0],ControlCommand::World(module_engine_core::WorldCommand::SetColor{color,..}) if *color==[42,10,20,255])
        );
        host.compile_and_apply(
            "fn update(ctx:&mut ScriptContext) { if !ctx.world_queries_available { ctx.set_render_parameter(\"tonemap\",\"exposure\",2.25); } }",
        )
        .unwrap();
        let render_only = ScriptContext::without_world_queries(2.0, 0.016);
        let tick = host.tick(&render_only, Duration::from_secs(2)).unwrap();
        assert_eq!(
            tick.commands,
            vec![ControlCommand::Render(
                module_engine_core::RenderCommand::SetParameter {
                    pass: "tonemap".into(),
                    name: "exposure".into(),
                    value: serde_json::json!(2.25),
                }
            )]
        );
        assert!(
            host.tick(&context, Duration::from_secs(2))
                .unwrap()
                .commands
                .is_empty()
        );
        host.compile_and_apply(
            "fn update(_ctx:&mut ScriptContext) { loop { std::hint::spin_loop(); } }",
        )
        .unwrap();
        let start = Instant::now();
        assert!(matches!(
            host.tick(&context, Duration::from_millis(100)),
            Err(ScriptError::Timeout(_))
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn worker_lock_refuses_new_cache_versions_and_same_version_from_another_source() {
        let root = "version = 4\n[[package]]\nname = \"serde_json\"\nversion = \"1.0.100\"\nsource = \"registry+official\"\n";
        let newer = root.replace("1.0.100", "1.0.999");
        assert!(verify_worker_lock(root, &newer, "worker").is_err());
        let other_source = root.replace("registry+official", "git+other");
        assert!(verify_worker_lock(root, &other_source, "worker").is_err());
        assert_eq!(
            verify_worker_lock(root, root, "worker").unwrap(),
            vec![("serde_json".into(), "1.0.100".into())]
        );
    }

    #[test]
    fn cancelled_compilation_creates_no_candidate_and_owned_process_cancellation_waits_for_exit() {
        if let Some(directory) = std::env::var_os("MODULE_ENGINE_CLEANUP_CHILD") {
            let directory = PathBuf::from(directory);
            #[cfg(windows)]
            if std::env::var_os("MODULE_ENGINE_CLEANUP_CHILD_ABRUPT_OWNER").is_some() {
                use std::os::windows::process::CommandExt;
                let descendant = directory.join("abrupt-descendant");
                fs::create_dir(&descendant).unwrap();
                let mut command = Command::new(std::env::current_exe().unwrap());
                command.args(["--exact", "tests::cancelled_compilation_creates_no_candidate_and_owned_process_cancellation_waits_for_exit", "--nocapture"])
                    .env_remove("MODULE_ENGINE_CLEANUP_CHILD_ABRUPT_OWNER")
                    .env("MODULE_ENGINE_CLEANUP_CHILD", &descendant)
                    .env("MODULE_ENGINE_CLEANUP_CHILD_DELAY_MS", "500")
                    .creation_flags(0x08000000);
                let _owned = OwnedChild::new(command.spawn().unwrap()).unwrap();
                let start = Instant::now();
                while !descendant.join("started").is_file() {
                    assert!(start.elapsed() < Duration::from_secs(2));
                    thread::sleep(Duration::from_millis(2));
                }
                fs::write(directory.join("started"), "owner exits without Drop").unwrap();
                std::process::exit(0);
            }
            if std::env::var_os("MODULE_ENGINE_CLEANUP_CHILD_SPAWN_DESCENDANT").is_some() {
                let descendant = directory.join("descendant");
                fs::create_dir(&descendant).unwrap();
                let mut command = Command::new(std::env::current_exe().unwrap());
                command.args(["--exact", "tests::cancelled_compilation_creates_no_candidate_and_owned_process_cancellation_waits_for_exit", "--nocapture"])
                    .env_remove("MODULE_ENGINE_CLEANUP_CHILD_SPAWN_DESCENDANT")
                    .env("MODULE_ENGINE_CLEANUP_CHILD", &descendant)
                    .env("MODULE_ENGINE_CLEANUP_CHILD_DELAY_MS", "500");
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    command.creation_flags(0x08000000);
                }
                // The detached-leader subprocess fixture deliberately exits
                // before this child, so the outer owner's group cleanup is
                // what the regression exercises.
                #[allow(clippy::zombie_processes)]
                let mut descendant_child = command.spawn().unwrap();
                let start = Instant::now();
                while !descendant.join("started").is_file() {
                    assert!(start.elapsed() < Duration::from_secs(2));
                    thread::sleep(Duration::from_millis(2));
                }
                fs::write(directory.join("started"), "tree started").unwrap();
                if std::env::var_os("MODULE_ENGINE_CLEANUP_CHILD_DETACH_DESCENDANT").is_some() {
                    return;
                }
                let _ = descendant_child.wait();
                fs::write(directory.join("completed"), "unexpected tree survivor").unwrap();
                return;
            }
            fs::write(directory.join("started"), "started").unwrap();
            let delay = std::env::var("MODULE_ENGINE_CLEANUP_CHILD_DELAY_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(300)
                .min(30_000);
            thread::sleep(Duration::from_millis(delay));
            fs::write(directory.join("completed"), "unexpected survivor").unwrap();
            return;
        }
        let artifacts = TestArtifactGuard::new(&std::env::temp_dir(), "module-cancel").unwrap();
        let work = artifacts.path().join("never-created");
        let compiler = ScriptCompiler::new(artifacts.path().to_path_buf(), work.clone());
        assert!(
            compiler
                .compile_kind_cancellable(
                    "fn update(_: &mut ScriptContext) {}",
                    ScriptKind::Update,
                    &AtomicBool::new(true)
                )
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(!work.exists());

        let started = artifacts.path().join("started");
        let completed = artifacts.path().join("completed");
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let wait_started = started.clone();
        let signal_thread = thread::spawn(move || {
            let start = Instant::now();
            while !wait_started.exists() && start.elapsed() < Duration::from_secs(2) {
                thread::sleep(Duration::from_millis(2));
            }
            signal.store(true, Ordering::Release);
        });
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "tests::cancelled_compilation_creates_no_candidate_and_owned_process_cancellation_waits_for_exit", "--nocapture"])
            .env("MODULE_ENGINE_CLEANUP_CHILD", artifacts.path());
        let result = run_bounded(command, vec![], Duration::from_secs(3), &cancelled, None);
        signal_thread.join().unwrap();
        assert!(
            started.is_file(),
            "the cancellation fixture must really start"
        );
        assert!(matches!(result, Err(ScriptError::Worker(error)) if error.contains("cancelled")));
        thread::sleep(Duration::from_millis(350));
        assert!(
            !completed.exists(),
            "cancelled child continued executing after run_bounded returned"
        );

        #[cfg(any(windows, target_os = "linux"))]
        {
            fs::remove_file(&started).unwrap();
            let cancelled = Arc::new(AtomicBool::new(false));
            let signal = cancelled.clone();
            let wait_started = started.clone();
            let signal_thread = thread::spawn(move || {
                let start = Instant::now();
                while !wait_started.exists() && start.elapsed() < Duration::from_secs(3) {
                    thread::sleep(Duration::from_millis(2));
                }
                signal.store(true, Ordering::Release);
            });
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "tests::cancelled_compilation_creates_no_candidate_and_owned_process_cancellation_waits_for_exit", "--nocapture"])
                .env("MODULE_ENGINE_CLEANUP_CHILD", artifacts.path())
                .env("MODULE_ENGINE_CLEANUP_CHILD_SPAWN_DESCENDANT", "1");
            let result = run_bounded(command, vec![], Duration::from_secs(5), &cancelled, None);
            signal_thread.join().unwrap();
            assert!(started.is_file(), "the owned descendant must really start");
            assert!(
                matches!(result, Err(ScriptError::Worker(error)) if error.contains("cancelled"))
            );
            thread::sleep(Duration::from_millis(550));
            assert!(!completed.exists());
            assert!(
                !artifacts.path().join("descendant/completed").exists(),
                "cancelled descendant continued executing after its owner returned"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_successful_owner_exit_still_terminates_its_descendants_before_return() {
        let artifacts =
            TestArtifactGuard::new(&std::env::temp_dir(), "module-linux-owned-group").unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "tests::cancelled_compilation_creates_no_candidate_and_owned_process_cancellation_waits_for_exit", "--nocapture"])
            .env("MODULE_ENGINE_CLEANUP_CHILD", artifacts.path())
            .env("MODULE_ENGINE_CLEANUP_CHILD_SPAWN_DESCENDANT", "1")
            .env("MODULE_ENGINE_CLEANUP_CHILD_DETACH_DESCENDANT", "1");
        let result = run_bounded(
            command,
            vec![],
            Duration::from_secs(5),
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        assert!(result.success);
        assert!(artifacts.path().join("descendant/started").is_file());
        thread::sleep(Duration::from_millis(550));
        assert!(
            !artifacts.path().join("descendant/completed").exists(),
            "successful child left an owned descendant executing or holding its pipes"
        );
    }

    #[cfg(windows)]
    #[test]
    fn owned_job_kills_children_after_host_process_exits_without_running_drop() {
        use std::os::windows::process::CommandExt;
        let artifacts =
            TestArtifactGuard::new(&std::env::temp_dir(), "module-abrupt-owner").unwrap();
        // Deliberately do not put this owner in our OwnedChild. Only its own
        // last-handle-close protection can terminate the started descendant.
        let mut owner = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::cancelled_compilation_creates_no_candidate_and_owned_process_cancellation_waits_for_exit", "--nocapture"])
            .env("MODULE_ENGINE_CLEANUP_CHILD", artifacts.path())
            .env("MODULE_ENGINE_CLEANUP_CHILD_ABRUPT_OWNER", "1")
            .creation_flags(0x08000000)
            .spawn().unwrap();
        let start = Instant::now();
        let status = loop {
            if let Some(status) = owner.try_wait().unwrap() {
                break status;
            }
            if start.elapsed() > Duration::from_secs(4) {
                let _ = owner.kill();
                let _ = owner.wait();
                panic!("abrupt owner fixture did not exit");
            }
            thread::sleep(Duration::from_millis(2));
        };
        assert!(status.success());
        assert!(artifacts.path().join("started").is_file());
        assert!(artifacts.path().join("abrupt-descendant/started").is_file());
        thread::sleep(Duration::from_millis(550));
        assert!(
            !artifacts
                .path()
                .join("abrupt-descendant/completed")
                .exists(),
            "child survived its owner's process exit"
        );
    }

    #[test]
    fn worker_builds_use_a_separate_target_under_both_default_and_explicit_bases() {
        let root = Path::new("engine");
        assert_eq!(
            worker_target_dir(root, None),
            root.join("target/custom-workers")
        );
        let explicit = PathBuf::from("external-build-cache");
        assert_eq!(
            worker_target_dir(root, Some(explicit.clone())),
            explicit.join("custom-workers")
        );
    }

    #[test]
    fn windows_portable_manifest_never_overrides_native_linux_rustup_policy() {
        // Pure policy coverage; these flags are not a Linux compilation/run.
        assert!(portable_manifest_rustup(true, true));
        assert!(!portable_manifest_rustup(true, false));
        assert!(!portable_manifest_rustup(false, true));
        assert!(!portable_manifest_rustup(false, false));
    }

    #[test]
    fn cache_selection_follows_project_then_sibling_then_portable_then_environment() {
        let artifacts =
            TestArtifactGuard::new(&std::env::temp_dir(), "module-script-cache").unwrap();
        let directory = artifacts.path();
        let project = directory.join("project");
        fs::create_dir_all(project.join(".tools/cargo")).unwrap();
        let inherited = directory.join("other-cache");
        assert_eq!(
            resolve_cargo_home_for_platform(&project, Some(inherited.clone()), false),
            Some(project.join(".tools/cargo"))
        );
        fs::create_dir_all(directory.join("cargo-home")).unwrap();
        assert_eq!(
            resolve_cargo_home_for_platform(&project, Some(inherited.clone()), false),
            Some(directory.join("cargo-home"))
        );
        fs::create_dir_all(project.join(".work/cargo-home")).unwrap();
        assert_eq!(
            resolve_cargo_home_for_platform(&project, Some(inherited.clone()), false),
            Some(project.join(".work/cargo-home"))
        );
        assert_eq!(
            resolve_cargo_home_for_platform(&project, None, false),
            Some(project.join(".work/cargo-home"))
        );
        assert_eq!(
            resolve_cargo_home_for_platform(&project, Some(inherited.clone()), true),
            Some(inherited),
            "explicit Linux Cargo home must not use any legacy Windows cache"
        );
        assert_eq!(
            resolve_cargo_home_for_platform(&project, None, true),
            Some(project.join(".work/cargo-home")),
            "without an explicit Linux cache retain ordinary project discovery"
        );
        let shims = project.join(".tools/cargo/bin");
        fs::create_dir_all(&shims).unwrap();
        let cargo = shims.join(format!("cargo{}", std::env::consts::EXE_SUFFIX));
        fs::write(&cargo, []).unwrap();
        assert_eq!(resolve_tool_program(&project, "cargo", None), Some(cargo));
        let overridden = directory.join("explicit-cargo");
        assert_eq!(
            resolve_tool_program(&project, "cargo", Some(overridden.clone())),
            Some(overridden)
        );
        assert!(resolve_tool_program(&project, "rustc", None).is_none());
        assert_eq!(compiler_tool_root(&project.join(".sdk")), project);
        assert_eq!(compiler_tool_root(&project), project);
    }
}
