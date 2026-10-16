//! Conservative native linker input discovery for the supported host toolchains.
//! Probes use the compiler's frozen child environment and never execute the probe
//! program. Unknown drivers, overrides or linker scripts disable reuse; they do
//! not change the ordinary compilation path. This is not a hermetic OS image.
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const INVENTORY_LIMIT: usize = 8192;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(crate) struct NativeTools {
    pub files: Vec<PathBuf>,
    /// Order is significant: the proof also hashes these descriptors in order.
    pub searches: Vec<SearchDirectory>,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchDirectory {
    pub path: PathBuf,
    pub names: Vec<OsString>,
    pub libraries: bool,
}

fn library_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_lowercase();
    name.ends_with(".lib") || name.ends_with(".a") || name.ends_with(".dll") || name.contains(".so")
}

fn candidate(path: &Path) -> Result<Option<PathBuf>, String> {
    match fs::metadata(path) {
        Ok(_) => path.canonicalize().map(Some).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Native input {}: {e}", path.display())),
    }
}

/// Records absence as well as canonical destinations, including directory
/// symlinks. No subprocesses, recursive OS walks, timestamps or global env.
pub(crate) fn snapshot(
    searches: &[SearchDirectory],
) -> Result<BTreeMap<PathBuf, Option<PathBuf>>, String> {
    let mut result = BTreeMap::new();
    for search in searches {
        if !search.path.is_absolute() || search.names.len() > INVENTORY_LIMIT {
            return Err("Unbounded or relative native search directory".into());
        }
        let directory = candidate(&search.path)?;
        result.insert(search.path.clone(), directory.clone());
        let mut names: BTreeSet<OsString> = search.names.iter().cloned().collect();
        if directory.is_some() && search.libraries {
            for (count, entry) in fs::read_dir(&search.path)
                .map_err(|e| e.to_string())?
                .enumerate()
            {
                if count >= INVENTORY_LIMIT {
                    return Err("Native directory inventory exceeds its bound".into());
                }
                let entry = entry.map_err(|e| e.to_string())?;
                if library_name(&entry.file_name()) {
                    names.insert(entry.file_name());
                }
            }
        }
        for name in names {
            let mut components = Path::new(&name).components();
            if !matches!(components.next(), Some(std::path::Component::Normal(_)))
                || components.next().is_some()
            {
                return Err("Native search candidate must be a filename".into());
            }
            let path = search.path.join(name);
            result.insert(path.clone(), candidate(&path)?);
            if result.len() > INVENTORY_LIMIT {
                return Err("Native search inventory exceeds its bound".into());
            }
        }
    }
    Ok(result)
}

#[derive(Debug)]
struct LinkCommand {
    environment: BTreeMap<OsString, OsString>,
    arguments: Vec<String>,
}

/// rustc's --print link-args uses Command's Debug representation, not a shell
/// language. Decode quoted strings strictly; never invoke a shell to parse it.
fn link_command(text: &str) -> Result<LinkCommand, String> {
    let mut remaining = text.trim();
    let mut environment = BTreeMap::new();
    let mut arguments = Vec::new();
    while !remaining.is_empty() {
        let assignment = if !remaining.starts_with('"') {
            let (name, rest) = remaining
                .split_once('=')
                .ok_or("Unknown native linker command representation")?;
            if !arguments.is_empty()
                || name.is_empty()
                || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err("Unknown native linker environment assignment".into());
            }
            remaining = rest;
            Some(name.to_owned())
        } else {
            None
        };
        let mut stream = serde_json::Deserializer::from_str(remaining).into_iter::<String>();
        let value = stream
            .next()
            .ok_or("Missing quoted native command argument")?
            .map_err(|_| "Unsupported native command quoting")?;
        let consumed = stream.byte_offset();
        remaining = remaining[consumed..].trim_start();
        if let Some(name) = assignment {
            environment.insert(name.into(), value.into());
        } else {
            arguments.push(value);
        }
        if arguments.len() + environment.len() > INVENTORY_LIMIT {
            return Err("Native linker command exceeds its bound".into());
        }
    }
    if arguments.is_empty() {
        return Err("Native linker command was empty".into());
    }
    Ok(LinkCommand {
        environment,
        arguments,
    })
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("compilation cancelled".into())
    } else {
        Ok(())
    }
}

fn env_value<'a>(environment: &'a BTreeMap<OsString, OsString>, key: &str) -> Option<&'a OsStr> {
    environment.iter().find_map(|(name, value)| {
        let name = name.to_str()?;
        (if cfg!(windows) {
            name.eq_ignore_ascii_case(key)
        } else {
            name == key
        })
        .then_some(value.as_os_str())
    })
}

fn executable(
    program: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<PathBuf, String> {
    executable_location(program, environment)?
        .canonicalize()
        .map_err(|e| e.to_string())
}

/// Rustup dispatches by argv[0]. Invoke its rustc symlink under the logical
/// name; canonicalizing it first would invoke the rustup CLI instead.
fn executable_location(
    program: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<PathBuf, String> {
    if program.is_absolute() {
        return if program.is_file() {
            Ok(program.into())
        } else {
            Err(format!("Native tool {} was not found", program.display()))
        };
    }
    if program.components().count() != 1 {
        return Err("Relative native tool paths are unsupported".into());
    }
    let mut name = program.as_os_str().to_owned();
    if cfg!(windows) && program.extension().is_none() {
        name.push(".exe");
    }
    for directory in std::env::split_paths(env_value(environment, "PATH").unwrap_or_default()) {
        if !directory.is_absolute() {
            return Err("Relative native tool search paths are unsupported".into());
        }
        let path = directory.join(&name);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(format!("Native tool {} was not found", program.display()))
}

fn probe(
    tool: &Path,
    arguments: &[String],
    environment: &BTreeMap<OsString, OsString>,
    cwd: &Path,
    input: Vec<u8>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    cancelled(cancel)?;
    let mut command = Command::new(tool);
    command
        .env_clear()
        .envs(environment)
        .current_dir(cwd)
        .args(arguments);
    let output = crate::run_bounded(command, input, PROBE_TIMEOUT, cancel, None)
        .map_err(|e| e.to_string())?;
    cancelled(cancel)?;
    if !output.success {
        let bounded = |value: &str| value.chars().take(2048).collect::<String>();
        return Err(format!(
            "Native linker discovery failed: tool={} arguments={arguments:?}; stdout={:?}; stderr={:?}",
            tool.display(),
            bounded(&output.stdout),
            bounded(&output.stderr)
        ));
    }
    Ok(output.stdout.trim().to_owned())
}

pub(crate) fn resolve(
    host: &str,
    environment: &BTreeMap<OsString, OsString>,
    cancel: &AtomicBool,
) -> Result<NativeTools, String> {
    cancelled(cancel)?;
    // These can replace code or the driver's search rules without appearing in
    // rustc's command. Unsupported configurations keep the ordinary fresh build.
    for key in [
        "GCC_EXEC_PREFIX",
        "COMPILER_PATH",
        "LIBRARY_PATH",
        "GCC_SPECS",
        "LD_RUN_PATH",
        "LD_PRELOAD",
        "LD_AUDIT",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
        "COLLECT_GCC",
        "COLLECT_LTO_WRAPPER",
        "COLLECT_GCC_OPTIONS",
        "GNUTARGET",
        "LDEMULATION",
        "LINK",
        "_LINK_",
    ] {
        if env_value(environment, key).is_some_and(|v| !v.is_empty()) {
            return Err(format!("Native reuse cannot prove the {key} override"));
        }
    }
    let parent = env_value(environment, "CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or("Native probe requires an absolute owned target parent")?;
    let owned = crate::cleanup::OwnedTarget::create(&parent, "native-tools")?;
    let rustc = executable_location(
        Path::new(env_value(environment, "RUSTC").unwrap_or(OsStr::new("rustc"))),
        environment,
    )?;
    let output = owned
        .directory
        .join(format!("probe{}", std::env::consts::EXE_SUFFIX));
    let arguments = vec![
        "--crate-name".into(),
        "module_engine_native_tools_probe".into(),
        "--edition=2024".into(),
        "--print=link-args".into(),
        "-o".into(),
        output
            .to_str()
            .ok_or("Non-Unicode native probe path")?
            .into(),
        "-".into(),
    ];
    let printed = probe(
        &rustc,
        &arguments,
        environment,
        &owned.directory,
        b"fn main() {}".to_vec(),
        cancel,
    )?;
    let command = link_command(&printed)?;
    let mut effective = environment.clone();
    effective.extend(
        command
            .environment
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    #[cfg(windows)]
    let result = resolve_msvc(host, &command, &effective, cancel)?;
    #[cfg(target_os = "linux")]
    let result = resolve_gcc(host, &command, &effective, &owned.directory, cancel)?;
    #[cfg(not(any(windows, target_os = "linux")))]
    let result = {
        let _ = (host, command, effective);
        return Err("Unsupported native linker host".into());
    };
    cancelled(cancel)?;
    Ok(result)
}

fn finish(
    mut files: Vec<PathBuf>,
    searches: Vec<SearchDirectory>,
    cancel: &AtomicBool,
) -> Result<NativeTools, String> {
    cancelled(cancel)?;
    for path in snapshot(&searches)?.into_values().flatten() {
        if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    cancelled(cancel)?;
    Ok(NativeTools { files, searches })
}

#[cfg(windows)]
fn resolve_msvc(
    host: &str,
    command: &LinkCommand,
    environment: &BTreeMap<OsString, OsString>,
    cancel: &AtomicBool,
) -> Result<NativeTools, String> {
    if !host.ends_with("-pc-windows-msvc") {
        return Err("Only the native MSVC linker is audited on Windows".into());
    }
    struct Frozen<'a>(&'a BTreeMap<OsString, OsString>);
    impl find_msvc_tools::EnvGetter for Frozen<'_> {
        fn get_env(&self, name: &'static str) -> Option<find_msvc_tools::Env> {
            msvc_discovery_value(self.0, name).map(|v| find_msvc_tools::Env::Owned(v.to_owned()))
        }
    }
    bounded_msvc_environment(environment)?;
    let arch = host.split('-').next().ok_or("Missing MSVC architecture")?;
    let selected = find_msvc_tools::find_tool_with_env(arch, "link.exe", &Frozen(environment))
        .ok_or("Could not prove the selected Visual Studio linker")?;
    let actual = executable(Path::new(&command.arguments[0]), environment)?;
    if selected.is_clang_cl()
        || selected.path().canonicalize().map_err(|e| e.to_string())? != actual
        || !actual
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|n| n.eq_ignore_ascii_case("link.exe"))
    {
        return Err("The selected linker is not the discovered native MSVC linker".into());
    }
    let mut effective = environment.clone();
    effective.extend(selected.env().into_iter().map(|(k, v)| {
        (
            OsString::from(k.to_string_lossy().to_ascii_uppercase()),
            v.clone(),
        )
    }));
    let mut searches = Vec::new();
    let linker_directory = actual.parent().ok_or("Missing linker directory")?;
    searches.push(SearchDirectory {
        path: linker_directory.into(),
        names: vec![
            "link.exe".into(),
            "mspdbsrv.exe".into(),
            "cvtres.exe".into(),
        ],
        libraries: true,
    });
    let lib = env_value(&effective, "LIB").ok_or("MSVC library search path was not discovered")?;
    // find-msvc-tools 0.1.14 add_env chains split_paths(empty) when the
    // inherited variable is absent. That adds one synthetic empty final item.
    // Remove only that known discovery artifact, not user-relative LIB paths.
    let synthetic_empty = env_value(environment, "LIB").is_none_or(OsStr::is_empty)
        && selected
            .env()
            .into_iter()
            .any(|(k, _)| k.to_str().is_some_and(|k| k.eq_ignore_ascii_case("LIB")));
    let directories =
        absolute_library_directories(std::env::split_paths(lib).collect(), synthetic_empty)?;
    let mut names = Vec::new();
    for argument in &command.arguments[1..] {
        if argument.to_ascii_lowercase().starts_with("/libpath:") {
            return Err("Custom MSVC library search flags are unsupported".into());
        }
        if argument.ends_with(".lib") && Path::new(argument).components().count() == 1 {
            names.push(argument.into());
        }
    }
    for directory in directories {
        searches.push(SearchDirectory {
            path: directory,
            names: names.clone(),
            libraries: true,
        });
    }
    finish(vec![actual], searches, cancel)
}

#[cfg(any(windows, test))]
fn bounded_msvc_environment(environment: &BTreeMap<OsString, OsString>) -> Result<(), String> {
    // In find-msvc-tools 0.1.14, this legacy environment otherwise invokes
    // cl.exe through an unbounded Command::output architecture probe.
    if (env_value(environment, "VCINSTALLDIR").is_some()
        || env_value(environment, "VSTEL_MSBuildProjectFullPath").is_some())
        && env_value(environment, "VSCMD_ARG_TGT_ARCH").is_none()
    {
        return Err("Legacy MSVC developer environments without explicit VSCMD_ARG_TGT_ARCH cannot prove bounded discovery".into());
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn msvc_discovery_value<'a>(
    environment: &'a BTreeMap<OsString, OsString>,
    name: &str,
) -> Option<&'a OsStr> {
    // The locked finder uses these solely for its unbounded vswhere.exe
    // fallback. COM/registry discovery remains available. This view belongs
    // only to the finder; the real compiler/Cargo environment is unchanged.
    if name.eq_ignore_ascii_case("ProgramFiles") || name.eq_ignore_ascii_case("ProgramFiles(x86)") {
        None
    } else {
        env_value(environment, name)
    }
}

#[cfg(any(windows, test))]
fn absolute_library_directories(
    mut paths: Vec<PathBuf>,
    synthetic_empty: bool,
) -> Result<Vec<PathBuf>, String> {
    if synthetic_empty && paths.last().is_some_and(|p| p.as_os_str().is_empty()) {
        paths.pop();
    }
    if paths.is_empty() || paths.iter().any(|p| !p.is_absolute()) {
        return Err("MSVC requires absolute discovered library search paths".into());
    }
    Ok(paths)
}

#[cfg(target_os = "linux")]
fn resolve_gcc(
    host: &str,
    command: &LinkCommand,
    environment: &BTreeMap<OsString, OsString>,
    cwd: &Path,
    cancel: &AtomicBool,
) -> Result<NativeTools, String> {
    if host != "x86_64-unknown-linux-gnu" {
        return Err("Only the native x86_64 GNU/Linux GCC linker is audited".into());
    }
    let driver = executable(Path::new(&command.arguments[0]), environment)?;
    let driver_name = driver.file_name().and_then(OsStr::to_str).unwrap_or("");
    if !driver_name.contains("gcc") || !elf_file(&driver)? {
        return Err("Native reuse requires the standard ELF GCC driver, not a wrapper".into());
    }
    let version = probe(
        &driver,
        &["--version".into()],
        environment,
        cwd,
        vec![],
        cancel,
    )?;
    if !version.contains("Free Software Foundation")
        || version.to_ascii_lowercase().contains("clang")
    {
        return Err("The selected native C driver is not GCC".into());
    }
    let mut controls = Vec::new();
    let mut linker = "ld".to_owned();
    let mut names = BTreeSet::new();
    for argument in &command.arguments[1..] {
        if argument.starts_with("-B") || argument.starts_with("-fuse-ld=") || argument == "-m64" {
            controls.push(argument.clone());
        }
        if let Some(flavor) = argument.strip_prefix("-fuse-ld=") {
            linker = match flavor {
                "lld" => "ld.lld",
                "bfd" => "ld.bfd",
                "gold" => "ld.gold",
                _ => return Err("Unknown native linker selection".into()),
            }
            .into();
        }
        if let Some(name) = argument.strip_prefix("-l") {
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err("Unknown native library selection".into());
            }
            names.insert(format!("lib{name}.so"));
            names.insert(format!("lib{name}.a"));
        }
    }
    if !command.arguments.iter().any(|a| a == "-pie")
        || !command.arguments.iter().any(|a| a == "-nodefaultlibs")
    {
        return Err("Unsupported native executable link mode".into());
    }
    let query = |argument: String| {
        let mut args = controls.clone();
        args.push(argument);
        probe(&driver, &args, environment, cwd, vec![], cancel)
    };
    let mut files = vec![driver.clone()];
    let mut searches = Vec::new();
    // PATH membership also matters if a new higher-priority cc/helper appears.
    let program_name = Path::new(&command.arguments[0])
        .file_name()
        .ok_or("Missing GCC name")?
        .to_owned();
    let program_names = vec![program_name, OsString::from(&linker), "collect2".into()];
    for directory in std::env::split_paths(env_value(environment, "PATH").unwrap_or_default()) {
        searches.push(SearchDirectory {
            path: directory,
            names: program_names.clone(),
            libraries: false,
        });
    }
    for control in &controls {
        if let Some(directory) = control.strip_prefix("-B") {
            let path = PathBuf::from(directory);
            if !path.is_absolute() {
                return Err("Relative GCC tool prefix is unsupported".into());
            }
            searches.push(SearchDirectory {
                path,
                names: program_names.clone(),
                libraries: false,
            });
        }
    }
    for program in [&linker[..], "collect2"] {
        let logical = PathBuf::from(query(format!("-print-prog-name={program}"))?);
        let selected = executable(&logical, environment)?;
        if !elf_file(&selected)? {
            return Err("Native GCC helper/linker must be an ELF executable".into());
        }
        if logical.is_absolute() {
            searches.push(SearchDirectory {
                path: logical.parent().ok_or("Missing GCC helper parent")?.into(),
                names: vec![logical.file_name().ok_or("Missing GCC helper name")?.into()],
                libraries: false,
            });
        }
        files.push(selected);
    }
    let search_text = query("-print-search-dirs".into())?;
    let programs = search_text
        .lines()
        .find_map(|line| line.strip_prefix("programs: ="))
        .ok_or("GCC did not report its helper search directories")?;
    for directory in programs.split(':').map(PathBuf::from) {
        if !directory.is_absolute() {
            return Err("Relative GCC helper search directory".into());
        }
        searches.push(SearchDirectory {
            path: directory,
            names: program_names.clone(),
            libraries: false,
        });
    }
    let libraries = search_text
        .lines()
        .find_map(|line| line.strip_prefix("libraries: ="))
        .ok_or("GCC did not report its library search directories")?;
    let directories: Vec<PathBuf> = libraries.split(':').map(PathBuf::from).collect();
    if directories.is_empty() || directories.iter().any(|p| !p.is_absolute()) {
        return Err("GCC reported unsupported relative library directories".into());
    }
    for name in [
        "Scrt1.o",
        "crti.o",
        "crtbeginS.o",
        "crtendS.o",
        "crtn.o",
        "specs",
        "libgcc.a",
        "libgcc_s.so.1",
    ] {
        names.insert(name.into());
    }
    for directory in &directories {
        searches.push(SearchDirectory {
            path: directory.clone(),
            names: names.iter().map(OsString::from).collect(),
            libraries: false,
        });
    }
    let mut pending = Vec::new();
    let mut resolved_names = BTreeSet::new();
    for name in &names {
        let selected = query(format!("-print-file-name={name}"))?;
        if selected != *name {
            let path = PathBuf::from(selected);
            if !path.is_absolute() {
                return Err("GCC returned a relative native library".into());
            }
            if name == "specs" {
                // An external GCC specs file can redirect every following
                // query. The standard audited configuration uses builtins.
                return Err("External GCC specs are unsupported for native reuse".into());
            }
            resolved_names.insert(name.clone());
            pending.push(path);
        } else if name.ends_with(".o") {
            return Err(format!("GCC startup input {name} was not resolved"));
        }
    }
    for argument in &command.arguments {
        if let Some(name) = argument.strip_prefix("-l") {
            if !resolved_names.contains(&format!("lib{name}.so"))
                && !resolved_names.contains(&format!("lib{name}.a"))
            {
                return Err(format!("Native link library {name} was not resolved"));
            }
        }
    }
    let mut visited = BTreeSet::new();
    while let Some(path) = pending.pop() {
        cancelled(cancel)?;
        let canonical = path.canonicalize().map_err(|e| e.to_string())?;
        if !visited.insert(canonical.clone()) {
            continue;
        }
        if visited.len() > 256 {
            return Err("Native linker script closure exceeds its bound".into());
        }
        // Preserve the logical symlink/filename, not merely its current target.
        searches.push(SearchDirectory {
            path: path.parent().ok_or("Missing native input parent")?.into(),
            names: vec![path.file_name().ok_or("Missing native input name")?.into()],
            libraries: false,
        });
        if let Some(script) = linker_script(&canonical)? {
            for dependency in script_inputs(&script)? {
                let dependency = if let Some(name) = dependency.strip_prefix("-l") {
                    if name.is_empty()
                        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    {
                        return Err("Unknown native linker script library".into());
                    }
                    // GROUP(-lfoo) follows the same search as direct -lfoo.
                    // Record absent shared candidates too: a later libfoo.so
                    // must not silently supersede the currently selected .a.
                    let candidates = [format!("lib{name}.so"), format!("lib{name}.a")];
                    for directory in &directories {
                        searches.push(SearchDirectory {
                            path: directory.clone(),
                            names: candidates.iter().map(OsString::from).collect(),
                            libraries: false,
                        });
                    }
                    let mut resolved = false;
                    for candidate in &candidates {
                        let selected = query(format!("-print-file-name={candidate}"))?;
                        if selected != *candidate {
                            let path = PathBuf::from(selected);
                            if !path.is_absolute() {
                                return Err("Relative native script library".into());
                            }
                            pending.push(path);
                            resolved = true;
                        }
                    }
                    if !resolved {
                        return Err("Unresolved native linker script library".into());
                    }
                    continue;
                } else if Path::new(&dependency).is_absolute() {
                    PathBuf::from(dependency)
                } else {
                    let local = path.parent().unwrap().join(&dependency);
                    if local.is_file() {
                        local
                    } else {
                        let selected = query(format!("-print-file-name={dependency}"))?;
                        PathBuf::from(selected)
                    }
                };
                if !dependency.is_absolute() {
                    return Err("Unresolved native linker script dependency".into());
                }
                pending.push(dependency);
            }
        }
        files.push(canonical);
    }
    finish(files, searches, cancel)
}

#[cfg(target_os = "linux")]
fn elf_file(path: &Path) -> Result<bool, String> {
    use std::io::Read as _;
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut magic = [0; 4];
    file.read_exact(&mut magic).map_err(|e| e.to_string())?;
    Ok(magic == *b"\x7fELF")
}

#[cfg(target_os = "linux")]
fn linker_script(path: &Path) -> Result<Option<String>, String> {
    use std::io::Read as _;
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut head = [0; 8];
    file.read_exact(&mut head).map_err(|e| e.to_string())?;
    if head.starts_with(b"\x7fELF") || head == *b"!<arch>\n" {
        return Ok(None);
    }
    let mut bytes = head.to_vec();
    file.take(256 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("Unknown native linker script format".into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "Unknown native linker input format".into())
}

#[cfg(any(target_os = "linux", test))]
fn script_inputs(script: &str) -> Result<Vec<String>, String> {
    let mut clean = String::new();
    let mut remaining = script;
    while let Some((before, after)) = remaining.split_once("/*") {
        clean.push_str(before);
        let (_, rest) = after
            .split_once("*/")
            .ok_or("Unterminated native linker script comment")?;
        remaining = rest;
    }
    clean.push_str(remaining);
    let tokens: Vec<&str> = clean
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';'))
        .filter(|s| !s.is_empty())
        .collect();
    let mut result = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "GROUP" | "INPUT" | "AS_NEEDED" => {}
            "OUTPUT_FORMAT" => {
                index += 1;
                if tokens.get(index) != Some(&"elf64-x86-64") {
                    return Err("Unknown native linker output format".into());
                }
            }
            name if name.starts_with('/') || name.starts_with("-l") || name.starts_with("lib") => {
                if name.contains(['\'', '"', '$', '=']) {
                    return Err("Unknown native linker script expression".into());
                }
                result.push(name.into());
            }
            _ => return Err("Unsupported native linker script directive".into()),
        }
        index += 1;
    }
    if result.is_empty() {
        return Err("Native linker script has no proven inputs".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_actual_rustc_debug_commands_without_shell_interpretation() {
        let windows =
            link_command(r#""C:\\Program Files\\VS\\link.exe" "/NOLOGO" "kernel32.lib""#).unwrap();
        assert_eq!(windows.arguments[0], r"C:\Program Files\VS\link.exe");
        let linux = link_command(
            r#"LC_ALL="C" PATH="/rust/bin:/usr/bin" "cc" "-B/rust/gcc-ld" "-fuse-ld=lld""#,
        )
        .unwrap();
        assert_eq!(
            linux.environment.get(OsStr::new("PATH")).unwrap(),
            "/rust/bin:/usr/bin"
        );
        assert_eq!(linux.arguments, ["cc", "-B/rust/gcc-ld", "-fuse-ld=lld"]);
        assert!(link_command(r#"sh -c "cc""#).is_err());
        assert!(link_command(r#""cc" "\xFF""#).is_err());
        assert!(link_command(r#""cc" PATH="/other""#).is_err());
    }

    #[test]
    fn native_snapshot_detects_new_library_and_missing_candidate() {
        let owned =
            crate::cleanup::OwnedTarget::create(&std::env::temp_dir(), "native-search-test")
                .unwrap();
        let searches = vec![SearchDirectory {
            path: owned.directory.clone(),
            names: vec!["future.lib".into()],
            libraries: true,
        }];
        let before = snapshot(&searches).unwrap();
        assert_eq!(before.get(&owned.directory.join("future.lib")), Some(&None));
        fs::write(owned.directory.join("future.lib"), b"library").unwrap();
        let after = snapshot(&searches).unwrap();
        assert_ne!(before, after);
        fs::write(owned.directory.join("another.a"), b"archive").unwrap();
        assert_ne!(after, snapshot(&searches).unwrap());
    }

    #[test]
    fn native_snapshot_retains_absent_directory_and_rejects_escape_names() {
        let owned =
            crate::cleanup::OwnedTarget::create(&std::env::temp_dir(), "native-search-test")
                .unwrap();
        let path = owned.directory.join("missing");
        let mut searches = vec![SearchDirectory {
            path: path.clone(),
            names: vec!["libc.so".into()],
            libraries: false,
        }];
        assert_eq!(snapshot(&searches).unwrap().get(&path), Some(&None));
        searches[0].names.push("../outside".into());
        assert!(snapshot(&searches).is_err());
        searches[0].names.pop();
        searches[0].names.push("..".into());
        assert!(snapshot(&searches).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn native_snapshot_detects_symlink_target_change() {
        let owned =
            crate::cleanup::OwnedTarget::create(&std::env::temp_dir(), "native-search-test")
                .unwrap();
        fs::write(owned.directory.join("one"), b"one").unwrap();
        fs::write(owned.directory.join("two"), b"two").unwrap();
        let path = owned.directory.join("libtest.so");
        std::os::unix::fs::symlink("one", &path).unwrap();
        let searches = vec![SearchDirectory {
            path: owned.directory.clone(),
            names: vec!["libtest.so".into()],
            libraries: false,
        }];
        let before = snapshot(&searches).unwrap();
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("two", &path).unwrap();
        assert_ne!(before, snapshot(&searches).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn native_rustc_invocation_preserves_rustup_shim_name() {
        let owned =
            crate::cleanup::OwnedTarget::create(&std::env::temp_dir(), "native-shim-test").unwrap();
        fs::write(owned.directory.join("rustup"), b"identity-only fixture").unwrap();
        let logical = owned.directory.join("rustc");
        std::os::unix::fs::symlink("rustup", &logical).unwrap();
        let environment = BTreeMap::from([(
            OsString::from("PATH"),
            std::env::join_paths([&owned.directory]).unwrap(),
        )]);
        assert_eq!(
            executable_location(Path::new("rustc"), &environment).unwrap(),
            logical
        );
        assert_eq!(
            executable(Path::new("rustc"), &environment).unwrap(),
            owned.directory.join("rustup")
        );
    }

    #[test]
    fn native_script_closure_supports_gnu_crt_groups_and_rejects_unknown_commands() {
        assert_eq!(script_inputs("/* comment */ OUTPUT_FORMAT(elf64-x86-64) GROUP (/lib/libc.so.6 /usr/lib/libc_nonshared.a AS_NEEDED (/lib64/ld-linux.so.2))").unwrap(), ["/lib/libc.so.6", "/usr/lib/libc_nonshared.a", "/lib64/ld-linux.so.2"]);
        assert_eq!(
            script_inputs("GROUP(libgcc_s.so.1 -lgcc)").unwrap(),
            ["libgcc_s.so.1", "-lgcc"]
        );
        assert!(script_inputs("INCLUDE other.ld").is_err());
        assert!(script_inputs("SEARCH_DIR(/custom) INPUT(libc.so)").is_err());
    }

    #[test]
    fn cancelled_native_resolution_never_starts_a_probe() {
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            resolve("unsupported", &BTreeMap::new(), &cancelled).unwrap_err(),
            "compilation cancelled"
        );
    }

    #[test]
    fn only_discovery_synthetic_trailing_empty_lib_path_is_ignored() {
        let root = std::env::temp_dir();
        assert_eq!(
            absolute_library_directories(vec![root.clone(), PathBuf::new()], true).unwrap(),
            std::slice::from_ref(&root)
        );
        assert!(absolute_library_directories(vec![root.clone(), PathBuf::new()], false).is_err());
        assert!(absolute_library_directories(vec![PathBuf::new(), root], true).is_err());
        assert!(absolute_library_directories(vec![PathBuf::from("relative")], true).is_err());
    }

    #[test]
    fn msvc_discovery_cannot_launch_legacy_cl_or_vswhere_fallbacks() {
        let mut environment = BTreeMap::from([
            (
                OsString::from("VCINSTALLDIR"),
                OsString::from("selected-vc"),
            ),
            (
                OsString::from("ProgramFiles"),
                OsString::from("selected-programs"),
            ),
            (
                OsString::from("ProgramFiles(x86)"),
                OsString::from("selected-programs-x86"),
            ),
        ]);
        assert!(bounded_msvc_environment(&environment).is_err());
        environment.insert("VSCMD_ARG_TGT_ARCH".into(), "x64".into());
        assert!(bounded_msvc_environment(&environment).is_ok());
        assert_eq!(
            msvc_discovery_value(&environment, "VSCMD_ARG_TGT_ARCH"),
            Some(OsStr::new("x64"))
        );
        assert!(msvc_discovery_value(&environment, "ProgramFiles").is_none());
        assert!(msvc_discovery_value(&environment, "ProgramFiles(x86)").is_none());
        assert_eq!(
            environment.get(OsStr::new("ProgramFiles")).unwrap(),
            "selected-programs"
        );
        environment.remove(OsStr::new("VCINSTALLDIR"));
        environment.remove(OsStr::new("VSCMD_ARG_TGT_ARCH"));
        assert!(bounded_msvc_environment(&environment).is_ok());
    }

    /// Compiler/linker discovery is opt-in because it really links a tiny
    /// executable (without running it). No Cargo, engine or GPU is invoked.
    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    #[ignore = "requires the installed native Rust/MSVC or Rust/GCC toolchain"]
    fn native_host_resolver_matches_actual_rustc_linker() {
        let owned = crate::cleanup::OwnedTarget::create(&std::env::temp_dir(), "native-proof-test")
            .unwrap();
        let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        environment.insert(
            "CARGO_TARGET_DIR".into(),
            owned.directory.clone().into_os_string(),
        );
        let host = if cfg!(windows) {
            "x86_64-pc-windows-msvc"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let tools = resolve(host, &environment, &AtomicBool::new(false)).unwrap();
        assert!(!tools.files.is_empty());
        assert!(!tools.searches.is_empty());
        assert!(tools.files.iter().all(|p| p.is_absolute() && p.is_file()));
        assert!(tools.files.iter().all(|p| !p.starts_with(&owned.directory)));
        assert!(
            tools
                .searches
                .iter()
                .all(|s| !s.path.starts_with(&owned.directory))
        );
        let names: Vec<_> = tools
            .files
            .iter()
            .filter_map(|p| p.file_name().and_then(OsStr::to_str))
            .collect();
        #[cfg(windows)]
        {
            assert!(names.contains(&"link.exe"));
            assert!(names.iter().any(|n| n.eq_ignore_ascii_case("kernel32.lib")));
            assert!(names.iter().any(|n| n.eq_ignore_ascii_case("msvcrt.lib")));
        }
        #[cfg(target_os = "linux")]
        {
            assert!(names.contains(&"collect2"));
            assert!(
                names
                    .iter()
                    .any(|n| *n == "rust-lld" || n.starts_with("ld"))
            );
            assert!(names.contains(&"libc.so.6"));
            assert!(names.contains(&"libc_nonshared.a"));
            assert!(names.contains(&"Scrt1.o"));
            assert!(
                tools
                    .searches
                    .iter()
                    .any(|search| search.names.iter().any(|name| name == "libgcc.so"))
            );
        }
        println!(
            "native tools: {} files, {} ordered searches",
            tools.files.len(),
            tools.searches.len()
        );
    }
}
