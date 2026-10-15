//! Compiled Rust Custom workers exchanging owned, serialized command packets.
//! Worker processes isolate failure from the editor; they are not a security sandbox.

use module_engine_core::{ControlCommand, ScriptContext};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

mod persistent;
pub use persistent::StateDeclaration;

const PACKET_MARKER: &str = "__MODULE_ENGINE_SCRIPT_PACKET__";
const MAX_PACKET: usize = 1024 * 1024;
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
            rustup_home: uses_portable.then(|| tool_root.join(".tools/rustup")),
            rustup_toolchain: uses_portable.then(|| project_toolchain(&tool_root)),
            project_root,
            work_dir,
            compile_timeout: Duration::from_secs(180),
        }
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
            "module_engine_worker_{}_{}",
            std::process::id(),
            NEXT_WORKER.fetch_add(1, Ordering::Relaxed)
        );
        let directory = self.work_dir.join(&name);
        fs::create_dir_all(directory.join("src")).map_err(|e| ScriptError::Setup(e.to_string()))?;
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
        let target = self
            .target_dir
            .clone()
            .unwrap_or_else(|| directory.join("target"));
        let target = if target.is_absolute() {
            target
        } else {
            root.join(target)
        };
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
                .current_dir(&directory);
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
        let resolved = run_bounded(metadata, vec![], self.compile_timeout)?;
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
            .current_dir(&directory);
        if let Some(home) = &self.cargo_home {
            command.env("CARGO_HOME", home);
        }
        let output = run_bounded(command, vec![], self.compile_timeout)?;
        if !output.success {
            return Err(ScriptError::Compile(format!(
                "{}{}",
                output.stderr, output.stdout
            )));
        }
        let executable = target
            .join("debug")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if !executable.is_file() {
            return Err(ScriptError::Compile(
                "Cargo succeeded without producing the worker executable".into(),
            ));
        }
        Ok(CompiledScript {
            source: source.into(),
            kind,
            executable,
            working_dir: compiler_tool_root(&root),
            worker_lock,
            dependency_versions,
            state_declaration: state_declaration.clone(),
            instance: persistent::new_instance(&state_declaration),
        })
    }
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
        self.migrate_state(state)?;
        Ok(self)
    }

    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn kind(&self) -> ScriptKind {
        self.kind
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
        if let Some(active) = &self.active {
            candidate.inherit_state_from(active)?;
        }
        self.active = Some(candidate);
        Ok(())
    }
    pub fn apply(&mut self, candidate: CompiledScript) -> Result<(), ScriptError> {
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
use module_engine_core::ScriptContext;
mod user_program;
fn run() -> Result<(), Box<dyn std::error::Error>> {{
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    loop {{
        let mut bytes = Vec::new();
        let count = std::io::Read::take(&mut reader, 1048577).read_until(b'\n', &mut bytes)?;
        if count == 0 {{ break; }}
        if bytes.len() > 1048576 {{ return Err("input too large".into()); }}
        let input: serde_json::Value = serde_json::from_slice(&bytes)?;
        let request_id = input.get("request_id").and_then(serde_json::Value::as_u64).ok_or("missing request_id")?;
        let mut context: ScriptContext = serde_json::from_value(input.get("context").cloned().ok_or("missing context")?)?;
        context.commands.clear();
        context.emitted_events.clear();
        {invoke}
        if context.commands.len() > 4096 || context.emitted_events.len() > 4096 {{ return Err("too many commands/events".into()); }}
        let packet = serde_json::json!({{"request_id":request_id,"commands":context.commands,"speed":speed,"state":context.state,"events":context.emitted_events}});
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
) -> Result<ProcessOutput, ScriptError> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW, including worker console.
    }
    let mut child = command
        .spawn()
        .map_err(|e| ScriptError::Setup(e.to_string()))?;
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
            Some(status) => break status,
            None if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ScriptError::Timeout(timeout.as_millis()));
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
    Ok(ProcessOutput {
        success: status.success(),
        stdout: collect(stdout)?,
        stderr: collect(stderr)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{EcsWorld, ObjectsWorld, RenderOverrides, WorldProvider};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn compiler_assets_classification_keeps_full_sources_named_sdk_in_their_own_package() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = workspace.join(format!(
            ".work/test-temp/compiler-root-{}-{stamp}",
            std::process::id()
        ));
        let full = base.join("full/.sdk");
        let team = base.join("team/.sdk");
        fs::create_dir_all(full.join("crates/editor/src")).unwrap();
        fs::create_dir_all(team.join("crates/engine-core")).unwrap();
        fs::write(full.join("crates/editor/src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(team.join("crates/engine-core/Cargo.toml"), "").unwrap();
        assert_eq!(compiler_tool_root(&full), full);
        assert_eq!(compiler_tool_root(&team), base.join("team"));
        let allowed = workspace.join(".work/test-temp").canonicalize().unwrap();
        let base = base.canonicalize().unwrap();
        assert!(base.starts_with(allowed));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn binding_assets_preserves_declared_state_without_stealing_active_worker() {
        let compiler_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = compiler_root.join(format!(
            ".work/test-temp/script-assets-{}-{stamp}",
            std::process::id()
        ));
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
        let compiler = ScriptCompiler::new(
            compiler_root.clone(),
            compiler_root.join(".work/asset-route-workers"),
        );
        let active = compiler
            .compile_kind(source, ScriptKind::Update)
            .unwrap()
            .with_asset_root(&first_root)
            .unwrap();
        let context = ScriptContext::new(0., 1. / 60., vec![]);
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
        drop(rebound);
        drop(same);
        drop(active);
        let allowed = compiler_root
            .join(".work/test-temp")
            .canonicalize()
            .unwrap();
        let base = base.canonicalize().unwrap();
        assert!(base.starts_with(allowed));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn real_persistent_state_is_transactional_hot_applicable_and_precompiled_relocatable() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let work = root
            .join(".work")
            .join(format!("state-test-{}", std::process::id()));
        let mut compiler = ScriptCompiler::new(root.clone(), work.clone());
        compiler.cargo_home = resolve_cargo_home(&root, None);
        compiler.target_dir = Some(work.join("target"));
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
        let work = root
            .join(".work")
            .join(format!("script-test-{}", std::process::id()));
        let mut compiler = ScriptCompiler::new(root.clone(), work.clone());
        // Actual subprocess builds remove inherited CARGO_HOME and set the
        // project/sibling cache selected without consulting that environment.
        compiler.cargo_home = resolve_cargo_home(&root, None);
        assert!(compiler.cargo_home.is_some());
        // Do not contend with an outer cargo test process's target directory.
        compiler.target_dir = Some(work.join("target"));
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
    fn cache_selection_follows_project_then_sibling_then_portable_then_environment() {
        let directory = std::env::temp_dir().join(format!(
            "module-script-cache-{}-{}",
            std::process::id(),
            NEXT_WORKER.fetch_add(1, Ordering::Relaxed)
        ));
        let project = directory.join("project");
        fs::create_dir_all(project.join(".tools/cargo")).unwrap();
        let inherited = directory.join("other-cache");
        assert_eq!(
            resolve_cargo_home(&project, Some(inherited.clone())),
            Some(project.join(".tools/cargo"))
        );
        fs::create_dir_all(directory.join("cargo-home")).unwrap();
        assert_eq!(
            resolve_cargo_home(&project, Some(inherited.clone())),
            Some(directory.join("cargo-home"))
        );
        fs::create_dir_all(project.join(".work/cargo-home")).unwrap();
        assert_eq!(
            resolve_cargo_home(&project, Some(inherited)),
            Some(project.join(".work/cargo-home"))
        );
        assert_eq!(
            resolve_cargo_home(&project, None),
            Some(project.join(".work/cargo-home"))
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
        // Tests retain scratch evidence rather than recursively deleting paths.
    }
}
