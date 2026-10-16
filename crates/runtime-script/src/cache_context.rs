//! Selected native build context. All child commands see one frozen environment.
use crate::{
    freshness::{CompileContext, ToolIdentity},
    *,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, ffi::OsString};

pub(crate) struct FrozenEnvironment(pub(crate) BTreeMap<OsString, OsString>);
impl std::ops::Deref for FrozenEnvironment {
    type Target = BTreeMap<OsString, OsString>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::fmt::Debug for FrozenEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FrozenEnvironment(<private>)")
    }
}

pub(crate) struct Resolved {
    pub context: CompileContext,
    pub compiler: ScriptCompiler,
    _metadata: Arc<cleanup::OwnedTarget>,
}
fn setup(error: impl std::fmt::Display) -> ScriptError {
    ScriptError::Setup(error.to_string())
}
fn capture_environment(
    values: impl IntoIterator<Item = (OsString, OsString)>,
    windows: bool,
) -> BTreeMap<OsString, OsString> {
    values
        .into_iter()
        .map(|(key, value)| {
            let key = if windows {
                key.to_str()
                    .filter(|key| key.is_ascii())
                    .map(|key| OsString::from(key.to_ascii_uppercase()))
                    .unwrap_or(key)
            } else {
                key
            };
            (key, value)
        })
        .collect()
}
fn executable(program: &Path, env: &BTreeMap<OsString, OsString>) -> Result<PathBuf, ScriptError> {
    if program.components().count() > 1 || program.is_absolute() {
        let path = if program.is_absolute() {
            program.into()
        } else {
            std::env::current_dir().map_err(setup)?.join(program)
        };
        if !path.is_file() {
            return Err(setup("Selected compiler invocation is not a file"));
        }
        return Ok(path);
    }
    for dir in std::env::split_paths(
        env.get(std::ffi::OsStr::new("PATH"))
            .map(OsString::as_os_str)
            .unwrap_or_default(),
    ) {
        let file = dir.join(format!(
            "{}{}",
            program.to_string_lossy(),
            std::env::consts::EXE_SUFFIX
        ));
        if file.is_file() {
            return Ok(file);
        }
    }
    Err(setup(format!(
        "Selected tool {} was not found",
        program.display()
    )))
}
fn query(
    compiler: &ScriptCompiler,
    tool: &Path,
    args: &[&str],
    cwd: &Path,
    env: &BTreeMap<OsString, OsString>,
    cancel: &AtomicBool,
) -> Result<String, ScriptError> {
    let mut command = Command::new(tool);
    command.env_clear().envs(env).current_dir(cwd).args(args);
    let result = run_bounded(command, vec![], compiler.compile_timeout, cancel, None)?;
    if !result.success {
        return Err(setup(format!(
            "Selected native tool probe failed: {}{}",
            result.stderr, result.stdout
        )));
    }
    Ok(result.stdout.trim().into())
}
fn file_hash(
    path: &Path,
    cancel: &AtomicBool,
    hashes: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
) -> Result<Option<Vec<u8>>, ScriptError> {
    if let Some(hash) = hashes.get(path) {
        return Ok(hash.clone());
    }
    let hash = (|| {
        let Ok(mut file) = fs::File::open(path) else {
            return Ok(None);
        };
        let mut hasher = Sha256::new();
        let mut bytes = [0u8; 65536];
        let mut total = 0u64;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(ScriptError::Worker("compilation cancelled".into()));
            }
            let Ok(count) = file.read(&mut bytes) else {
                return Ok(None);
            };
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > 1024 * 1024 * 1024 {
                return Ok(None);
            }
            hasher.update(&bytes[..count]);
        }
        Ok(Some(hasher.finalize().to_vec()))
    })()?;
    hashes.insert(path.into(), hash.clone());
    Ok(hash)
}
fn recognized_selection(
    selected: &Path,
    actual: &Path,
    cancel: &AtomicBool,
    hashes: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
) -> Result<bool, ScriptError> {
    if selected == actual || selected.canonicalize().ok().as_deref() == Some(actual) {
        return Ok(true);
    }
    let Some(parent) = selected.parent() else {
        return Ok(false);
    };
    let rustup = parent.join(format!("rustup{}", std::env::consts::EXE_SUFFIX));
    Ok(rustup.is_file()
        && file_hash(selected, cancel, hashes)?
            .zip(file_hash(&rustup, cancel, hashes)?)
            .is_some_and(|(a, b)| a == b))
}

fn code_inputs(sysroot: &Path, host: &str) -> Result<Vec<PathBuf>, ScriptError> {
    let mut inputs = Vec::new();
    for directory in [sysroot.join("bin"), sysroot.join("lib")] {
        for entry in fs::read_dir(directory).map_err(setup)? {
            let path = entry.map_err(setup)?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if path.is_file()
                && (name.ends_with(".dll") || name.contains(".so") || name.ends_with(".dylib"))
            {
                inputs.push(path.canonicalize().map_err(setup)?);
            }
        }
    }
    // The native target's bundled linker/self-contained code belongs to the
    // proof too. Other targets, documentation and rust-src are not compiler inputs.
    let mut directories = vec![sysroot.join("lib/rustlib").join(host)];
    while let Some(directory) = directories.pop() {
        if inputs.len() > 4096 {
            return Err(setup("Native compiler input inventory exceeds its bound"));
        }
        for entry in fs::read_dir(directory).map_err(setup)? {
            let entry = entry.map_err(setup)?;
            let path = entry.path();
            let kind = entry.file_type().map_err(setup)?;
            if kind.is_dir() {
                directories.push(path)
            } else if path.is_file() {
                inputs.push(path.canonicalize().map_err(setup)?)
            }
        }
    }
    inputs.sort();
    inputs.dedup();
    Ok(inputs)
}

pub(crate) fn resolve(
    compiler: &ScriptCompiler,
    cancel: &AtomicBool,
) -> Result<Resolved, ScriptError> {
    if cancel.load(Ordering::Acquire) {
        return Err(ScriptError::Worker("compilation cancelled".into()));
    }
    validate_supported_sdk(&compiler.sdk_identity)?;
    let root = compiler.project_root.canonicalize().map_err(setup)?;
    validate_sdk_match(&read_sdk_identity(&root)?, &compiler.sdk_identity)?;
    fs::create_dir_all(&compiler.work_dir).map_err(setup)?;
    let work = compiler.work_dir.canonicalize().map_err(setup)?;
    let target = compiler
        .target_dir
        .clone()
        .unwrap_or_else(|| work.join("target"));
    let target = if target.is_absolute() {
        target
    } else {
        root.join(target)
    };
    fs::create_dir_all(&target).map_err(setup)?;
    let target = target.canonicalize().map_err(setup)?;
    let mut environment = capture_environment(std::env::vars_os(), cfg!(windows));
    environment.insert("CARGO_TARGET_DIR".into(), target.clone().into_os_string());
    environment.insert("CARGO_INCREMENTAL".into(), "0".into());
    environment.remove(std::ffi::OsStr::new("CARGO_HOME"));
    for (key, value) in [
        (
            "CARGO_HOME",
            compiler.cargo_home.as_ref().map(|p| p.as_os_str()),
        ),
        (
            "RUSTC",
            compiler.rustc_binary.as_ref().map(|p| p.as_os_str()),
        ),
        (
            "RUSTUP_HOME",
            compiler.rustup_home.as_ref().map(|p| p.as_os_str()),
        ),
    ] {
        if let Some(value) = value {
            environment.insert(key.into(), value.into());
        }
    }
    if let Some(toolchain) = &compiler.rustup_toolchain {
        environment.insert("RUSTUP_TOOLCHAIN".into(), toolchain.into());
    }
    let selected_cargo = executable(
        compiler
            .cargo_binary
            .as_deref()
            .unwrap_or(Path::new("cargo")),
        &environment,
    )?;
    let selected_rustc = executable(
        compiler
            .rustc_binary
            .as_deref()
            .unwrap_or(Path::new("rustc")),
        &environment,
    )?;
    let rustc_version = query(
        compiler,
        &selected_rustc,
        &["-vV"],
        &work,
        &environment,
        cancel,
    )?;
    let cargo_version = query(
        compiler,
        &selected_cargo,
        &["-vV"],
        &work,
        &environment,
        cancel,
    )?;
    let native_host = cargo_build::native_host(&rustc_version)?;
    let sysroot = PathBuf::from(query(
        compiler,
        &selected_rustc,
        &["--print", "sysroot"],
        &work,
        &environment,
        cancel,
    )?)
    .canonicalize()
    .map_err(setup)?;
    let rustc = sysroot
        .join(format!("bin/rustc{}", std::env::consts::EXE_SUFFIX))
        .canonicalize()
        .map_err(setup)?;
    let cargo = sysroot
        .join(format!("bin/cargo{}", std::env::consts::EXE_SUFFIX))
        .canonicalize()
        .map_err(setup)?;
    let mut hashes = BTreeMap::new();
    let recognized = recognized_selection(&selected_cargo, &cargo, cancel, &mut hashes)?
        && recognized_selection(&selected_rustc, &rustc, cancel, &mut hashes)?;
    let mut inputs = if recognized {
        code_inputs(&sysroot, &native_host)?
    } else {
        vec![]
    };
    inputs.extend([
        selected_cargo.canonicalize().map_err(setup)?,
        selected_rustc.canonicalize().map_err(setup)?,
    ]);
    let (cargo, rustc) = if recognized {
        (cargo, rustc)
    } else {
        (selected_cargo, selected_rustc)
    };
    environment.insert("RUSTC".into(), rustc.clone().into_os_string());
    let mut native_searches = Vec::new();
    if recognized {
        match crate::native_tools::resolve(&native_host, &environment, cancel) {
            Ok(native) => {
                inputs.extend(native.files);
                native_searches = native.searches;
            }
            Err(reason) => {
                if cancel.load(Ordering::Acquire) {
                    return Err(ScriptError::Worker("compilation cancelled".into()));
                }
                #[cfg(test)]
                eprintln!("Native Custom reuse unsupported: {reason}");
                #[cfg(not(test))]
                let _ = reason;
                inputs.clear();
            }
        }
    }
    if cfg!(windows)
        && environment
            .keys()
            .any(|key| key.to_str().is_none_or(|key| !key.is_ascii()))
    {
        inputs.clear();
    }
    let code_input_directories = vec![
        sysroot.join("bin"),
        sysroot.join("lib"),
        sysroot.join("lib/rustlib").join(&native_host),
    ];
    let root_lock = fs::read_to_string(root.join("Cargo.lock")).map_err(setup)?;
    let packages = lock_packages(&root_lock)?;
    let json: Vec<_> = packages
        .iter()
        .filter(|p| p.name == "serde_json" && p.version.starts_with("1."))
        .collect();
    if json.len() != 1 {
        return Err(setup(
            "Root Cargo.lock must identify one pinned serde_json 1.x package",
        ));
    }
    let json = &json[0].version;
    let metadata = cleanup::OwnedTarget::create(&work, "metadata").map_err(setup)?;
    let directory = &metadata.directory;
    let name = format!(
        "module_engine_metadata_{}_{}",
        std::process::id(),
        NEXT_WORKER.fetch_add(1, Ordering::Relaxed)
    );
    fs::create_dir(directory.join("src")).map_err(setup)?;
    let core = root.join("crates/engine-core");
    let text = core.to_string_lossy();
    let portable = if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("//{}", unc.replace('\\', "/"))
    } else {
        text.strip_prefix("\\\\?\\")
            .unwrap_or(&text)
            .replace('\\', "/")
    };
    let manifest = format!(
        "[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[dependencies]\nmodule-engine-core = {{ path = {} }}\nserde_json = \"={json}\"\n",
        serde_json::to_string(&portable).map_err(setup)?
    );
    fs::write(directory.join("Cargo.toml"), manifest).map_err(setup)?;
    fs::write(directory.join("src/main.rs"), "fn main() {}\n").map_err(setup)?;
    fs::write(directory.join("Cargo.lock"),format!("{root_lock}\n[[package]]\nname = {name:?}\nversion = \"0.0.0\"\ndependencies = [\n \"module-engine-core\",\n \"serde_json {json}\",\n]\n")).map_err(setup)?;
    let mut command = Command::new(&cargo);
    command
        .env_clear()
        .envs(&environment)
        .current_dir(directory)
        .args([
            "metadata",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(directory.join("Cargo.toml"));
    let result = run_bounded(command, vec![], compiler.compile_timeout, cancel, None)?;
    if !result.success {
        return Err(ScriptError::Compile(format!(
            "{}{}",
            result.stderr, result.stdout
        )));
    }
    let worker_lock = fs::read_to_string(directory.join("Cargo.lock")).map_err(setup)?;
    verify_worker_lock(&root_lock, &worker_lock, &name)?;
    let context = CompileContext {
        sdk_root: root,
        work_root: work,
        target_root: target,
        cargo_home: compiler.cargo_home.clone(),
        sdk_identity: compiler.sdk_identity.clone(),
        tool: ToolIdentity {
            cargo: cargo.clone(),
            rustc: rustc.clone(),
            cargo_version,
            rustc_version,
            sysroot,
            native_host,
            code_inputs: inputs,
            code_input_directories,
            native_searches,
        },
        environment: environment.clone(),
        metadata: result.stdout,
        root_lock,
        worker_lock,
        worker_package: name,
    };
    let mut compiler = compiler.clone();
    compiler.program_store = None;
    compiler.cargo_binary = Some(cargo);
    compiler.rustc_binary = Some(rustc);
    // RUSTC selection is frozen at the verified actual compiler. The full
    // environment proof records the caller's original selected value.
    compiler.effective_environment = Some(Arc::new(FrozenEnvironment(environment)));
    Ok(Resolved {
        context,
        compiler,
        _metadata: metadata,
    })
}

/// Positive Linux cache tests use the ordinary editor loader environment, not
/// Cargo's added test-binary directories. No production/global env is changed.
#[cfg(test)]
pub(crate) fn native_cache_test_child(name: &str, ignored: bool) -> bool {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (name, ignored);
        false
    }
    #[cfg(target_os = "linux")]
    {
        const MARKER: &str = "MODULE_ENGINE_NATIVE_CACHE_TEST_CHILD";
        if std::env::var(MARKER).ok().as_deref() == Some(name) {
            return false;
        }
        let Some(value) = std::env::var_os("LD_LIBRARY_PATH") else {
            return false;
        };
        let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        let profile = executable.parent().unwrap().parent().unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let compiler =
            ScriptCompiler::new(root.into(), root.join(".work/unused-native-test-query"));
        let mut command = Command::new(
            compiler
                .rustc_binary
                .as_deref()
                .unwrap_or(Path::new("rustc")),
        );
        if let Some(home) = &compiler.rustup_home {
            command.env("RUSTUP_HOME", home);
        }
        if let Some(toolchain) = &compiler.rustup_toolchain {
            command.env("RUSTUP_TOOLCHAIN", toolchain);
        }
        command.args(["--print", "sysroot", "--print", "target-libdir"]);
        let result = run_bounded(
            command,
            vec![],
            Duration::from_secs(15),
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        assert!(result.success, "selected native test sysroot probe failed");
        let mut queried = result.stdout.lines();
        let library = PathBuf::from(queried.next().expect("selected sysroot"))
            .join("lib")
            .canonicalize()
            .unwrap();
        let native_library = PathBuf::from(queried.next().expect("selected native target libdir"))
            .canonicalize()
            .unwrap();
        assert!(queried.next().is_none(), "unexpected rustc query output");
        let mut removed = false;
        let retained = std::env::split_paths(&value)
            .filter(|path| {
                let known = path.canonicalize().ok().is_some_and(|path| {
                    path == library
                        || path == native_library
                        || path == profile
                        || Some(path.as_path()) == executable.parent()
                });
                removed |= known;
                !known
            })
            .collect::<Vec<_>>();
        if !removed {
            return false;
        }
        let mut child = Command::new(&executable);
        child
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env(MARKER, name);
        if ignored {
            child.arg("--ignored");
        }
        if retained.is_empty() {
            child.env_remove("LD_LIBRARY_PATH");
        } else {
            child.env("LD_LIBRARY_PATH", std::env::join_paths(retained).unwrap());
        }
        let output = run_bounded(
            child,
            vec![],
            Duration::from_secs(900),
            &AtomicBool::new(false),
            None,
        )
        .unwrap();
        assert!(
            output.success,
            "native cache child {name}: {}{}",
            output.stdout, output.stderr
        );
        print!("{}", output.stdout);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires restored native tools and audited registry sources"]
    fn actual_supported_native_context_is_cacheable() {
        if native_cache_test_child(
            "cache_context::tests::actual_supported_native_context_is_cacheable",
            true,
        ) {
            return;
        }
        let fixture = TestArtifactGuard::new(&std::env::temp_dir(), "actual-proof").unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let compiler = ScriptCompiler::new(root.into(), fixture.path().join("work"));
        let cancel = AtomicBool::new(false);
        let resolved = resolve(&compiler, &cancel).unwrap();
        let prepared = freshness::prepare(
            &resolved.context,
            "fn speed(_:f64,_:f64)->f64{7.0}",
            ScriptKind::Speed,
            None,
            &cancel,
        )
        .unwrap();
        assert_eq!(prepared.eligibility, freshness::Eligibility::Cacheable);
        println!("actual native context eligible; source/tool proof complete");
    }
    #[test]
    fn windows_environment_canonicalization_preserves_values_and_selected_override() {
        let values = vec![
            (OsString::from("Path"), OsString::from("native path")),
            (OsString::from("rustc"), OsString::from("inherited")),
        ];
        let mut windows = capture_environment(values.clone(), true);
        assert_eq!(
            windows.get(std::ffi::OsStr::new("PATH")),
            Some(&OsString::from("native path"))
        );
        windows.insert("RUSTC".into(), "selected".into());
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[std::ffi::OsStr::new("RUSTC")], "selected");
        let linux = capture_environment(values, false);
        assert!(linux.contains_key(std::ffi::OsStr::new("Path")));
        assert!(!linux.contains_key(std::ffi::OsStr::new("PATH")));
    }
}
