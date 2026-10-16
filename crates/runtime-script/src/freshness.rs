//! Conservative content proofs for editor-owned immutable Custom programs.
//! This is an audited native build subset, not arbitrary Cargo hermeticity.
use crate::{ScriptError, ScriptKind};
use module_engine_core::SdkIdentity;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

mod audited_dependencies;

pub(crate) type Digest = String;
#[derive(Clone, Debug)]
pub(crate) struct ToolIdentity {
    pub cargo: PathBuf,
    pub rustc: PathBuf,
    pub cargo_version: String,
    pub rustc_version: String,
    pub sysroot: PathBuf,
    pub native_host: String,
    /// Actual selected compiler shared libraries and native rustlib inputs.
    pub code_inputs: Vec<PathBuf>,
    /// Resolver-owned directories whose membership can select compiler/link inputs.
    pub code_input_directories: Vec<PathBuf>,
    pub native_searches: Vec<crate::native_tools::SearchDirectory>,
}
#[derive(Clone)]
pub(crate) struct CompileContext {
    pub sdk_root: PathBuf,
    pub work_root: PathBuf,
    pub target_root: PathBuf,
    pub cargo_home: Option<PathBuf>,
    pub sdk_identity: SdkIdentity,
    pub tool: ToolIdentity,
    /// Effective inherited environment after Command env overrides/removals.
    pub environment: BTreeMap<OsString, OsString>,
    /// Actual Cargo metadata/normalized lock after offline resolution, before
    /// choosing a reusable SDK target namespace. Worker identity is excluded.
    pub metadata: String,
    pub root_lock: String,
    pub worker_lock: String,
    pub worker_package: String,
}
impl std::fmt::Debug for CompileContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompileContext")
            .field("sdk_root", &self.sdk_root)
            .field("work_root", &self.work_root)
            .field("target_root", &self.target_root)
            .field("environment", &"<private>")
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, Default)]
pub(crate) struct DependencyHints {
    pub root_files: Vec<PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MissReason {
    UnsupportedConfiguration,
    UnsupportedDependency,
    UnprovenToolchain,
    UnreadableInput,
    AmbiguousDepInfo,
    NewDependency,
    ContentChanged,
    ChangedDuringBuild,
    InvalidArtifact,
    LimitExceeded,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Eligibility {
    Cacheable,
    Uncacheable(MissReason),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MissScope {
    Root,
    Sdk,
}
#[derive(Clone, Debug)]
pub(crate) enum Validation {
    Hit,
    Miss {
        scope: MissScope,
        reason: MissReason,
        hints: DependencyHints,
    },
}
#[derive(Clone, Debug)]
pub(crate) struct BuildUnit {
    pub package_id: String,
    pub manifest_path: PathBuf,
    pub kinds: Vec<String>,
    pub artifacts: Vec<PathBuf>,
    pub dep_info: Vec<PathBuf>,
    pub out_dir: Option<PathBuf>,
    pub environment: BTreeMap<OsString, OsString>,
    pub fresh: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct BuildFacts {
    pub metadata: String,
    pub worker_lock: String,
    pub worker_package: String,
    pub executable: PathBuf,
    pub native_host: String,
    pub native_target: String,
    pub target_directory: PathBuf,
    pub units: Vec<BuildUnit>,
    pub root_dep_info: Vec<PathBuf>,
    /// Exact bytes written by lib.rs to its owned compiler workspace.
    pub generated_inputs: Vec<(PathBuf, Vec<u8>)>,
}
#[derive(Clone, Debug)]
pub(crate) enum SealOutcome {
    Cacheable(FreshnessSeal),
    Uncacheable {
        reason: MissReason,
        hints: DependencyHints,
    },
    /// The build must not be published: a captured input changed while Cargo ran.
    ChangedDuringBuild,
}
#[derive(Clone, Debug)]
pub(crate) struct PreparedInputs {
    pub sdk_key: Digest,
    pub root_lookup_key: Digest,
    pub eligibility: Eligibility,
    context: CompileContext,
    source: String,
    kind: ScriptKind,
    hints: DependencyHints,
    packages: BTreeMap<String, Package>,
    sdk_files: BTreeMap<PathBuf, Digest>,
    root_files: BTreeMap<PathBuf, Digest>,
    configuration_inventory: BTreeMap<PathBuf, Option<Digest>>,
    tool_inventory: BTreeMap<PathBuf, Option<Digest>>,
    native_inventory: BTreeMap<PathBuf, Option<PathBuf>>,
}
#[derive(Clone, Debug)]
pub(crate) struct FreshnessSeal {
    pub(crate) executable: PathBuf,
    executable_hash: Digest,
    sdk_key: Digest,
    root_key: Digest,
    extra_sdk_files: BTreeMap<PathBuf, Digest>,
    generated_root_inputs: BTreeMap<PathBuf, Digest>,
    absent_worker_configuration: Vec<PathBuf>,
    hints: DependencyHints,
}
impl FreshnessSeal {
    /// A warm Cargo check must prove the same immutable program, rather than
    /// silently sealing different runnable bytes after the precheck succeeded.
    pub(crate) fn same_products(&self, other: &Self) -> bool {
        self.executable == other.executable
            && self.executable_hash == other.executable_hash
            && self.sdk_key == other.sdk_key
            && self.root_key == other.root_key
            && self.generated_root_inputs == other.generated_root_inputs
            && self.extra_sdk_files == other.extra_sdk_files
            && self.absent_worker_configuration == other.absent_worker_configuration
            && self.hints.root_files == other.hints.root_files
    }
}

const MAX_FILES: usize = 4096;
const MAX_HINTS: usize = 128;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOOL_BYTES: u64 = 1024 * 1024 * 1024;
const TREE_MAGIC: &[u8] = b"ModuleEngine audited registry tree v1\0";

#[derive(Clone, Debug)]
struct Package {
    name: String,
    root: PathBuf,
    registry: bool,
    crate_names: BTreeSet<String>,
}
type ConfigurationSnapshot = (
    Vec<(PathBuf, Vec<u8>)>,
    BTreeMap<PathBuf, Option<Digest>>,
    bool,
);
type LoaderInputs = (Vec<PathBuf>, BTreeMap<PathBuf, Option<Digest>>);
type DependencySnapshot = (BTreeMap<String, Package>, Digest, BTreeMap<PathBuf, Digest>);

fn check(cancel: &AtomicBool) -> Result<(), MissReason> {
    if cancel.load(Ordering::Acquire) {
        Err(MissReason::Cancelled)
    } else {
        Ok(())
    }
}
fn cancelled(reason: MissReason) -> Result<MissReason, ScriptError> {
    if reason == MissReason::Cancelled {
        Err(ScriptError::Worker("compilation cancelled".into()))
    } else {
        Ok(reason)
    }
}
fn part(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
fn text(hash: &mut Sha256, value: &str) {
    part(hash, value.as_bytes());
}
fn path_part(hash: &mut Sha256, value: &Path) {
    part(hash, value.as_os_str().as_encoded_bytes());
}
fn finish(hash: Sha256) -> String {
    format!("{:x}", hash.finalize())
}
fn hash_bytes(bytes: &[u8]) -> String {
    finish(Sha256::new_with_prefix(bytes))
}
fn regular(path: &Path) -> Result<PathBuf, MissReason> {
    let metadata = fs::symlink_metadata(path).map_err(|_| MissReason::UnreadableInput)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(MissReason::UnreadableInput);
    }
    path.canonicalize().map_err(|_| MissReason::UnreadableInput)
}
fn runnable(path: &Path) -> Result<PathBuf, MissReason> {
    let path = regular(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&path)
            .map_err(|_| MissReason::UnreadableInput)?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err(MissReason::InvalidArtifact);
        }
    }
    Ok(path)
}
fn file_hash(path: &Path, limit: u64, cancel: &AtomicBool) -> Result<Digest, MissReason> {
    check(cancel)?;
    let path = regular(path)?;
    let mut file = fs::File::open(path).map_err(|_| MissReason::UnreadableInput)?;
    if file
        .metadata()
        .map_err(|_| MissReason::UnreadableInput)?
        .len()
        > limit
    {
        return Err(MissReason::LimitExceeded);
    }
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0; 65536];
    loop {
        check(cancel)?;
        let read = file
            .read(&mut buffer)
            .map_err(|_| MissReason::UnreadableInput)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > limit {
            return Err(MissReason::LimitExceeded);
        }
        hash.update(&buffer[..read]);
    }
    Ok(finish(hash))
}
fn tree_files(
    root: &Path,
    ignore_cargo_marker: bool,
    cancel: &AtomicBool,
) -> Result<BTreeMap<String, PathBuf>, MissReason> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    let mut directories = 0;
    while let Some(directory) = pending.pop() {
        check(cancel)?;
        directories += 1;
        if directories > MAX_FILES {
            return Err(MissReason::LimitExceeded);
        }
        for entry in fs::read_dir(directory).map_err(|_| MissReason::UnreadableInput)? {
            let entry = entry.map_err(|_| MissReason::UnreadableInput)?;
            let kind = entry.file_type().map_err(|_| MissReason::UnreadableInput)?;
            let path = entry.path();
            if kind.is_symlink() {
                return Err(MissReason::UnsupportedDependency);
            }
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| MissReason::UnsupportedDependency)?;
                let relative = relative
                    .to_str()
                    .ok_or(MissReason::UnsupportedDependency)?
                    .replace('\\', "/");
                if ignore_cargo_marker && relative == ".cargo-ok" {
                    continue;
                }
                result.insert(relative, path);
                if result.len() > MAX_FILES {
                    return Err(MissReason::LimitExceeded);
                }
            } else {
                return Err(MissReason::UnsupportedDependency);
            }
        }
    }
    Ok(result)
}
fn tree_hash(
    root: &Path,
    ignore_cargo_marker: bool,
    cancel: &AtomicBool,
) -> Result<(Digest, usize, u64, BTreeMap<PathBuf, Digest>), MissReason> {
    let files = tree_files(root, ignore_cargo_marker, cancel)?;
    let mut hash = Sha256::new_with_prefix(TREE_MAGIC);
    hash.update((files.len() as u64).to_le_bytes());
    let mut total = 0;
    let mut snapshots = BTreeMap::new();
    for (relative, path) in &files {
        check(cancel)?;
        let path = regular(path)?;
        if fs::metadata(&path)
            .map_err(|_| MissReason::UnreadableInput)?
            .len()
            > MAX_SOURCE_BYTES.saturating_sub(total)
        {
            return Err(MissReason::LimitExceeded);
        }
        let bytes = fs::read(&path).map_err(|_| MissReason::UnreadableInput)?;
        total += bytes.len() as u64;
        if total > MAX_SOURCE_BYTES {
            return Err(MissReason::LimitExceeded);
        }
        part(&mut hash, relative.as_bytes());
        part(&mut hash, &bytes);
        snapshots.insert(path, hash_bytes(&bytes));
    }
    Ok((finish(hash), files.len(), total, snapshots))
}
fn config_chain(context: &CompileContext) -> Result<ConfigurationSnapshot, MissReason> {
    let mut candidates = BTreeSet::new();
    for ancestor in context.work_root.ancestors() {
        candidates.insert(ancestor.join(".cargo/config"));
        candidates.insert(ancestor.join(".cargo/config.toml"));
        candidates.insert(ancestor.join("rust-toolchain"));
        candidates.insert(ancestor.join("rust-toolchain.toml"));
    }
    if let Some(home) = &context.cargo_home {
        candidates.insert(home.join("config"));
        candidates.insert(home.join("config.toml"));
    }
    let mut files = vec![];
    let mut inventory = BTreeMap::new();
    // Cargo may otherwise read its implicit global home, or resolve a relative
    // home against the generated worker cwd. Neither provenance is proven here.
    // Keep the ordinary compile path but allocate a fresh SDK generation.
    let mut supported = context
        .cargo_home
        .as_ref()
        .is_some_and(|home| home.is_absolute());
    for path in candidates {
        if path.try_exists().map_err(|_| MissReason::UnreadableInput)? {
            let actual = regular(&path)?;
            let bytes = fs::read(&actual).map_err(|_| MissReason::UnreadableInput)?;
            if bytes.len() > 64 * 1024 {
                return Err(MissReason::LimitExceeded);
            }
            if !path
                .file_name()
                .is_some_and(|name| name == "rust-toolchain" || name == "rust-toolchain.toml")
                && !simple_config(&bytes, &context.tool.native_host)
            {
                #[cfg(test)]
                eprintln!("Custom proof unsupported Cargo config: {}", path.display());
                supported = false;
            }
            inventory.insert(path, Some(hash_bytes(&bytes)));
            files.push((actual, bytes));
        } else {
            inventory.insert(path, None);
        }
    }
    Ok((files, inventory, supported))
}
fn simple_config(bytes: &[u8], host: &str) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut build = false;
    let mut seen = false;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[build]" && !build {
            build = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        if !build || key.trim() != "target" || seen {
            return false;
        }
        let value = value.trim();
        let valid = [format!("\"{host}\""), format!("'{host}'")]
            .iter()
            .any(|expected| {
                value
                    .strip_prefix(expected)
                    .is_some_and(|tail| tail.trim().is_empty() || tail.trim().starts_with('#'))
            });
        if !valid {
            return false;
        }
        seen = true;
    }
    true
}
fn supported_environment(context: &CompileContext) -> bool {
    for (key, value) in &context.environment {
        if value.is_empty() {
            continue;
        }
        let Some(key) = key.to_str() else {
            #[cfg(test)]
            eprintln!("Custom proof unsupported non-Unicode environment key");
            return false;
        };
        if cfg!(windows) && !key.is_ascii() {
            #[cfg(test)]
            eprintln!("Custom proof unsupported non-ASCII Windows environment key");
            return false;
        }
        if matches!(
            key,
            "RUSTC_WRAPPER"
                | "RUSTC_WORKSPACE_WRAPPER"
                | "RUSTFLAGS"
                | "CARGO_ENCODED_RUSTFLAGS"
                | "CARGO_BUILD_RUSTFLAGS"
                | "CARGO_BUILD_RUSTC"
                | "CARGO_BUILD_RUSTC_WRAPPER"
                | "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"
                | "RUSTC_BOOTSTRAP"
                | "RUSTC_STAGE"
                | "RUST_TARGET_PATH"
        ) || key.starts_with("CARGO_PROFILE_")
            || (cfg!(unix)
                && (key.starts_with("DYLD_")
                    || (key.starts_with("LD_") && key != "LD_LIBRARY_PATH")))
            || (key.starts_with("CARGO_TARGET_") && key != "CARGO_TARGET_DIR")
        {
            #[cfg(test)]
            eprintln!("Custom proof unsupported environment key: {key}");
            return false;
        }
        if key == "CARGO_BUILD_TARGET" && value != OsStr::new(&context.tool.native_host) {
            #[cfg(test)]
            eprintln!("Custom proof unsupported environment key: CARGO_BUILD_TARGET");
            return false;
        }
    }
    true
}

fn loader_inputs(
    context: &CompileContext,
    cancel: &AtomicBool,
) -> Result<LoaderInputs, MissReason> {
    loader_inputs_on_platform(context, cfg!(target_os = "linux"), cancel)
}
fn loader_inputs_on_platform(
    context: &CompileContext,
    linux: bool,
    cancel: &AtomicBool,
) -> Result<LoaderInputs, MissReason> {
    if !linux {
        return Ok((vec![], BTreeMap::new()));
    }
    let Some(value) = context.environment.get(OsStr::new("LD_LIBRARY_PATH")) else {
        return Ok((vec![], BTreeMap::new()));
    };
    if value.is_empty() {
        return Ok((vec![], BTreeMap::new()));
    }
    // A DXC-only directory cannot replace rustc/LLVM/system libraries:
    // its complete membership is restricted to these two distinct sonames.
    let paths = std::env::split_paths(value).collect::<Vec<_>>();
    if paths.len() != 1 || paths[0].as_os_str().is_empty() {
        #[cfg(test)]
        eprintln!("Custom proof unsupported loader key: LD_LIBRARY_PATH (not one directory)");
        return Err(MissReason::UnsupportedConfiguration);
    }
    let allowed = paths[0].canonicalize().map_err(|_| {
        #[cfg(test)]
        eprintln!("Custom proof unsupported loader key: LD_LIBRARY_PATH (unreadable directory)");
        MissReason::UnsupportedConfiguration
    })?;
    for entry in fs::read_dir(&allowed).map_err(|_| MissReason::UnsupportedConfiguration)? {
        let entry = entry.map_err(|_| MissReason::UnsupportedConfiguration)?;
        let kind = entry
            .file_type()
            .map_err(|_| MissReason::UnsupportedConfiguration)?;
        if !kind.is_file() || kind.is_symlink() {
            #[cfg(test)]
            eprintln!(
                "Custom proof unsupported DXC directory membership: {}",
                allowed.display()
            );
            return Err(MissReason::UnsupportedConfiguration);
        }
    }
    let files = tree_files(&allowed, false, cancel)?;
    if files.keys().map(String::as_str).collect::<BTreeSet<_>>()
        != BTreeSet::from(["libdxcompiler.so", "libdxil.so"])
    {
        #[cfg(test)]
        eprintln!(
            "Custom proof unsupported DXC directory membership: {}",
            allowed.display()
        );
        return Err(MissReason::UnsupportedConfiguration);
    }
    let mut inventory = Sha256::new();
    for name in files.keys() {
        text(&mut inventory, name);
    }
    Ok((
        files.into_values().collect(),
        BTreeMap::from([(allowed, Some(finish(inventory)))]),
    ))
}

fn tool_inventory(
    context: &CompileContext,
    cancel: &AtomicBool,
) -> Result<BTreeMap<PathBuf, Option<Digest>>, MissReason> {
    let mut inventory = BTreeMap::new();
    if context.tool.code_input_directories.is_empty() {
        return Err(MissReason::UnprovenToolchain);
    }
    for directory in &context.tool.code_input_directories {
        check(cancel)?;
        if !directory
            .try_exists()
            .map_err(|_| MissReason::UnreadableInput)?
        {
            inventory.insert(directory.clone(), None);
            continue;
        }
        let directory = directory
            .canonicalize()
            .map_err(|_| MissReason::UnreadableInput)?;
        let mut names = BTreeSet::new();
        if directory == context.tool.sysroot.join("bin")
            || directory == context.tool.sysroot.join("lib")
        {
            for entry in fs::read_dir(&directory).map_err(|_| MissReason::UnreadableInput)? {
                let entry = entry.map_err(|_| MissReason::UnreadableInput)?;
                let kind = entry.file_type().map_err(|_| MissReason::UnreadableInput)?;
                if kind.is_symlink() {
                    return Err(MissReason::UnprovenToolchain);
                }
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| MissReason::UnprovenToolchain)?;
                if kind.is_file() {
                    names.insert(name);
                }
                if names.len() > MAX_FILES {
                    return Err(MissReason::LimitExceeded);
                }
            }
        } else {
            names.extend(tree_files(&directory, false, cancel)?.into_keys());
        }
        let mut hash = Sha256::new();
        for name in names {
            text(&mut hash, &name);
        }
        inventory.insert(directory, Some(finish(hash)));
    }
    Ok(inventory)
}

fn audit_native_tokens(source: &str) -> Result<(), MissReason> {
    fn safe(tokens: proc_macro2::TokenStream) -> bool {
        tokens.into_iter().all(|token| match token {
            proc_macro2::TokenTree::Group(group) => safe(group.stream()),
            proc_macro2::TokenTree::Ident(ident) => !matches!(
                ident.to_string().trim_start_matches("r#"),
                "link"
                    | "link_name"
                    | "link_ordinal"
                    | "link_section"
                    | "link_args"
                    | "asm"
                    | "global_asm"
                    | "extern"
            ),
            _ => true,
        })
    }
    let tokens = source
        .parse()
        .map_err(|_| MissReason::UnsupportedDependency)?;
    if safe(tokens) {
        Ok(())
    } else {
        Err(MissReason::UnsupportedDependency)
    }
}

fn lock_checksums(text: &str) -> Result<BTreeMap<(String, String, String), String>, MissReason> {
    let mut result = BTreeMap::new();
    for block in text.split("[[package]]").skip(1) {
        let value = |key: &str| -> Option<String> {
            block
                .lines()
                .find_map(|line| line.trim().strip_prefix(&format!("{key} = ")))
                .and_then(|value| serde_json::from_str(value).ok())
        };
        if let Some(source) = value("source") {
            let key = (
                value("name").ok_or(MissReason::UnsupportedDependency)?,
                value("version").ok_or(MissReason::UnsupportedDependency)?,
                source,
            );
            let checksum = value("checksum").ok_or(MissReason::UnsupportedDependency)?;
            if result.insert(key, checksum).is_some() {
                return Err(MissReason::UnsupportedDependency);
            }
        }
    }
    Ok(result)
}
fn dependencies(
    context: &CompileContext,
    cancel: &AtomicBool,
) -> Result<DependencySnapshot, MissReason> {
    let metadata: Value =
        serde_json::from_str(&context.metadata).map_err(|_| MissReason::UnsupportedDependency)?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or(MissReason::UnsupportedDependency)?;
    let root = metadata["resolve"]["root"]
        .as_str()
        .ok_or(MissReason::UnsupportedDependency)?;
    if packages.len() > 64 {
        return Err(MissReason::UnsupportedDependency);
    }
    let locked = lock_checksums(&context.worker_lock)?;
    let root_locked = lock_checksums(&context.root_lock)?;
    let mut result = BTreeMap::new();
    let mut normalized = Sha256::new();
    let mut snapshots = BTreeMap::new();
    let mut audited_seen = BTreeSet::new();
    let mut entries = packages.iter().collect::<Vec<_>>();
    entries.sort_by_key(|package| package["name"].as_str().unwrap_or_default());
    for package in entries {
        check(cancel)?;
        let id = package["id"]
            .as_str()
            .ok_or(MissReason::UnsupportedDependency)?;
        let name = package["name"]
            .as_str()
            .ok_or(MissReason::UnsupportedDependency)?;
        if id == root {
            if name != context.worker_package {
                return Err(MissReason::UnsupportedDependency);
            }
            continue;
        }
        let version = package["version"]
            .as_str()
            .ok_or(MissReason::UnsupportedDependency)?;
        let manifest = PathBuf::from(
            package["manifest_path"]
                .as_str()
                .ok_or(MissReason::UnsupportedDependency)?,
        );
        let directory = manifest
            .parent()
            .ok_or(MissReason::UnsupportedDependency)?
            .canonicalize()
            .map_err(|_| MissReason::UnreadableInput)?;
        let registry = if let Some(source) = package["source"].as_str() {
            let known = audited_dependencies::PACKAGES
                .iter()
                .find(|known| {
                    known.name == name && known.version == version && known.source == source
                })
                .ok_or(MissReason::UnsupportedDependency)?;
            let key = (name.to_owned(), version.to_owned(), source.to_owned());
            if locked.get(&key).map(String::as_str) != Some(known.archive)
                || root_locked.get(&key).map(String::as_str) != Some(known.archive)
            {
                return Err(MissReason::UnsupportedDependency);
            }
            let (digest, count, bytes, files) = tree_hash(&directory, true, cancel)?;
            if digest != known.tree || count != known.files || bytes != known.bytes {
                return Err(MissReason::UnsupportedDependency);
            }
            text(&mut normalized, source);
            text(&mut normalized, &digest);
            snapshots.extend(files);
            audited_seen.insert(name);
            true
        } else {
            if name != "module-engine-core"
                || directory
                    != context
                        .sdk_root
                        .join("crates/engine-core")
                        .canonicalize()
                        .map_err(|_| MissReason::UnreadableInput)?
            {
                return Err(MissReason::UnsupportedDependency);
            }
            if package["targets"].as_array().is_none_or(|targets| {
                targets.iter().any(|target| {
                    target["kind"].as_array().is_some_and(|kinds| {
                        kinds
                            .iter()
                            .any(|kind| kind == "custom-build" || kind == "proc-macro")
                    })
                })
            }) {
                return Err(MissReason::UnsupportedDependency);
            }
            false
        };
        text(&mut normalized, name);
        text(&mut normalized, version);
        path_part(&mut normalized, &directory);
        if let Some(node) = metadata["resolve"]["nodes"]
            .as_array()
            .and_then(|nodes| nodes.iter().find(|node| node["id"] == id))
        {
            let mut features = node["features"]
                .as_array()
                .ok_or(MissReason::UnsupportedDependency)?
                .iter()
                .map(|value| value.as_str().ok_or(MissReason::UnsupportedDependency))
                .collect::<Result<Vec<_>, _>>()?;
            features.sort_unstable();
            for feature in features {
                text(&mut normalized, feature);
            }
        } else {
            return Err(MissReason::UnsupportedDependency);
        }
        result.insert(
            id.to_owned(),
            Package {
                name: name.into(),
                root: directory,
                registry,
                crate_names: package["targets"]
                    .as_array()
                    .ok_or(MissReason::UnsupportedDependency)?
                    .iter()
                    .filter(|target| {
                        !target["kind"]
                            .as_array()
                            .is_some_and(|kinds| kinds.iter().any(|kind| kind == "custom-build"))
                    })
                    .map(|target| {
                        target["name"]
                            .as_str()
                            .filter(|name| !name.is_empty() && !name.contains(['/', '\\']))
                            .map(str::to_owned)
                            .ok_or(MissReason::UnsupportedDependency)
                    })
                    .collect::<Result<BTreeSet<_>, _>>()?,
            },
        );
    }
    if audited_seen.len() != audited_dependencies::PACKAGES.len() {
        return Err(MissReason::UnsupportedDependency);
    }
    Ok((result, finish(normalized), snapshots))
}

pub(crate) fn prepare(
    context: &CompileContext,
    source: &str,
    kind: ScriptKind,
    hints: Option<&DependencyHints>,
    cancel: &AtomicBool,
) -> Result<PreparedInputs, ScriptError> {
    check(cancel).map_err(|_| ScriptError::Worker("compilation cancelled".into()))?;
    let mut prepared = PreparedInputs {
        sdk_key: String::new(),
        root_lookup_key: String::new(),
        eligibility: Eligibility::Cacheable,
        context: context.clone(),
        source: source.into(),
        kind,
        hints: hints.cloned().unwrap_or_default(),
        packages: BTreeMap::new(),
        sdk_files: BTreeMap::new(),
        root_files: BTreeMap::new(),
        configuration_inventory: BTreeMap::new(),
        tool_inventory: BTreeMap::new(),
        native_inventory: BTreeMap::new(),
    };
    if let Err(reason) = prepare_inner(&mut prepared, cancel) {
        prepared.eligibility = Eligibility::Uncacheable(cancelled(reason)?);
    }
    Ok(prepared)
}
fn prepare_inner(prepared: &mut PreparedInputs, cancel: &AtomicBool) -> Result<(), MissReason> {
    let context = &prepared.context;
    let mut sdk = Sha256::new_with_prefix(b"ModuleEngine SDK proof v1\0");
    let root = context
        .sdk_root
        .canonicalize()
        .map_err(|_| MissReason::UnreadableInput)?;
    path_part(&mut sdk, &root);
    text(&mut sdk, &context.sdk_identity.version);
    text(&mut sdk, &context.sdk_identity.data_api_fingerprint);
    for file in ["Cargo.toml", "Cargo.lock", "engine-sdk.json"] {
        let path = root.join(file);
        let hash = file_hash(&path, MAX_SOURCE_BYTES, cancel)?;
        text(&mut sdk, file);
        text(&mut sdk, &hash);
        prepared.sdk_files.insert(regular(&path)?, hash);
    }
    if fs::read_to_string(root.join("Cargo.lock")).map_err(|_| MissReason::UnreadableInput)?
        != context.root_lock
    {
        return Err(MissReason::ContentChanged);
    }
    let (digest, _, _, files) = tree_hash(&root.join("crates/engine-core"), false, cancel)?;
    text(&mut sdk, &digest);
    prepared.sdk_files.extend(files);
    let (configs, inventory, supported) = config_chain(context)?;
    prepared.configuration_inventory = inventory;
    if !supported || !supported_environment(context) {
        return Err(MissReason::UnsupportedConfiguration);
    }
    let (loader_files, loader_inventory) = loader_inputs(context, cancel)?;
    prepared.tool_inventory.extend(loader_inventory);
    for path in loader_files {
        let hash = file_hash(&path, MAX_TOOL_BYTES, cancel)?;
        path_part(&mut sdk, &path);
        text(&mut sdk, &hash);
        prepared.sdk_files.insert(path, hash);
    }
    audit_native_tokens(&prepared.source)?;
    audit_sdk_macros(&root.join("crates/engine-core"), cancel)?;
    let (packages, closure, files) = dependencies(context, cancel)?;
    text(&mut sdk, &closure);
    prepared.packages = packages;
    prepared.sdk_files.extend(files);
    for (path, bytes) in configs {
        path_part(&mut sdk, &path);
        part(&mut sdk, &bytes);
        prepared.sdk_files.insert(path, hash_bytes(&bytes));
    }
    if context.tool.native_host.is_empty()
        || context.tool.rustc_version.is_empty()
        || context.tool.cargo_version.is_empty()
        || context.tool.code_inputs.is_empty()
    {
        return Err(MissReason::UnprovenToolchain);
    }
    path_part(&mut sdk, &context.tool.sysroot);
    text(&mut sdk, &context.tool.native_host);
    text(&mut sdk, &context.tool.rustc_version);
    text(&mut sdk, &context.tool.cargo_version);
    let native_lib = context
        .tool
        .sysroot
        .join("lib/rustlib")
        .join(&context.tool.native_host)
        .join("lib");
    if !context
        .tool
        .code_inputs
        .iter()
        .any(|path| path.starts_with(&native_lib))
    {
        return Err(MissReason::UnprovenToolchain);
    }
    let tools = context
        .tool
        .code_inputs
        .iter()
        .chain([&context.tool.cargo, &context.tool.rustc])
        .collect::<BTreeSet<_>>();
    for path in tools {
        let path = regular(path)?;
        let hash = file_hash(&path, MAX_TOOL_BYTES, cancel)?;
        path_part(&mut sdk, &path);
        text(&mut sdk, &hash);
        prepared.sdk_files.insert(path, hash);
    }
    prepared
        .tool_inventory
        .extend(tool_inventory(context, cancel)?);
    for (path, digest) in &prepared.tool_inventory {
        path_part(&mut sdk, path);
        if let Some(digest) = digest {
            text(&mut sdk, digest);
        } else {
            sdk.update([0]);
        }
    }
    prepared.native_inventory = crate::native_tools::snapshot(&context.tool.native_searches)
        .map_err(|_| MissReason::UnprovenToolchain)?;
    check(cancel)?;
    for search in &context.tool.native_searches {
        path_part(&mut sdk, &search.path);
        sdk.update([u8::from(search.libraries)]);
        for name in &search.names {
            part(&mut sdk, name.as_encoded_bytes());
        }
    }
    for (path, candidate) in &prepared.native_inventory {
        path_part(&mut sdk, path);
        if let Some(candidate) = candidate {
            sdk.update([1]);
            path_part(&mut sdk, candidate);
        } else {
            sdk.update([0]);
        }
    }
    // Audited dependency build scripts inspect these build/tool inputs. UI-only
    // variables deliberately do not allocate a cold SDK namespace.
    for key in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "CARGO_HOME",
        "CARGO_BUILD_TARGET",
        "LIB",
        "LIBPATH",
        "INCLUDE",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "LD_LIBRARY_PATH",
        "RUSTC_STAGE",
        "RUSTC_BOOTSTRAP",
        "RUST_LIBC_UNSTABLE_FREEBSD_VERSION",
        "RUST_LIBC_UNSTABLE_MUSL_V1_2_3",
        "RUST_LIBC_UNSTABLE_GNU_TIME_BITS",
        "RUST_LIBC_UNSTABLE_GNU_FILE_OFFSET_BITS",
        "CARGO_CFG_LIBC_UNSTABLE_FREEBSD_VERSION",
        "CARGO_CFG_LIBC_UNSTABLE_MUSL_V1_2_3",
        "CARGO_CFG_LIBC_UNSTABLE_GNU_TIME_BITS",
        "CARGO_CFG_LIBC_UNSTABLE_GNU_FILE_OFFSET_BITS",
        "WIND_RELEASE_ID",
    ] {
        // Cargo may synthesize this Linux-only loader variable for Windows
        // tests. Windows MSVC does not consume it; the exact root env still does.
        if key == "LD_LIBRARY_PATH" && !cfg!(target_os = "linux") {
            continue;
        }
        text(&mut sdk, key);
        match context.environment.get(OsStr::new(key)) {
            Some(value) => {
                sdk.update([1]);
                part(&mut sdk, value.as_encoded_bytes());
            }
            None => sdk.update([0]),
        }
    }
    prepared.sdk_key = finish(sdk);
    let mut root_key = Sha256::new_with_prefix(b"ModuleEngine root proof v1\0");
    text(&mut root_key, &prepared.sdk_key);
    text(&mut root_key, &prepared.source);
    text(&mut root_key, &format!("{:?}", prepared.kind));
    path_part(&mut root_key, &context.work_root);
    path_part(&mut root_key, &context.target_root);
    for (key, value) in &context.environment {
        if key != OsStr::new("CARGO_TARGET_DIR") {
            part(&mut root_key, key.as_encoded_bytes());
            part(&mut root_key, value.as_encoded_bytes());
        }
    }
    if prepared.hints.root_files.len() > MAX_HINTS {
        return Err(MissReason::LimitExceeded);
    }
    let mut missing = false;
    let hints = prepared.hints.root_files.iter().collect::<BTreeSet<_>>();
    for path in hints {
        match file_hash(path, MAX_SOURCE_BYTES, cancel) {
            Ok(hash) => {
                let path = regular(path)?;
                // include! accepts arbitrary extensions; unsupported binary or
                // non-token inputs miss safely rather than hiding native IO.
                audit_native_tokens(
                    &fs::read_to_string(&path).map_err(|_| MissReason::UnsupportedDependency)?,
                )?;
                path_part(&mut root_key, &path);
                text(&mut root_key, &hash);
                prepared.root_files.insert(path, hash);
            }
            Err(MissReason::UnreadableInput) => missing = true,
            Err(reason) => return Err(reason),
        }
    }
    prepared.root_lookup_key = finish(root_key);
    if missing {
        // Missing historical hints are not dependencies of a successful new
        // build. Return only captured paths so the bounded hint index can heal
        // after an include was removed, rather than miss forever.
        prepared.hints.root_files = prepared.root_files.keys().cloned().collect();
        prepared.eligibility = Eligibility::Uncacheable(MissReason::NewDependency);
    }
    Ok(())
}

fn audit_sdk_macros(root: &Path, cancel: &AtomicBool) -> Result<(), MissReason> {
    use syn::visit::Visit;
    struct Audit<'a> {
        root: &'a Path,
        directory: PathBuf,
        valid: bool,
    }
    impl<'ast> Visit<'ast> for Audit<'_> {
        fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
            if item.attrs.iter().any(|attr| {
                attr.path().is_ident("cfg")
                    && matches!(&attr.meta,syn::Meta::List(list) if list.tokens.to_string()=="test")
            }) {
                return;
            }
            syn::visit::visit_item_mod(self, item);
        }
        fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
            if attr.path().is_ident("path") {
                let valid = match &attr.meta {
                    syn::Meta::NameValue(value) => match &value.value {
                        syn::Expr::Lit(lit) => match &lit.lit {
                            syn::Lit::Str(value) => self
                                .directory
                                .join(value.value())
                                .canonicalize()
                                .ok()
                                .is_some_and(|path| path.starts_with(self.root)),
                            _ => false,
                        },
                        _ => false,
                    },
                    _ => false,
                };
                self.valid &= valid;
            }
            syn::visit::visit_attribute(self, attr);
        }
        fn visit_macro(&mut self, mac: &'ast syn::Macro) {
            let name = mac
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
                .unwrap_or_default();
            if matches!(name.as_str(), "env" | "option_env") {
                self.valid &= syn::parse2::<syn::LitStr>(mac.tokens.clone())
                    .ok()
                    .is_some_and(|literal| literal.value() == "CARGO_PKG_VERSION");
            } else if matches!(name.as_str(), "include" | "include_str" | "include_bytes") {
                self.valid &= syn::parse2::<syn::LitStr>(mac.tokens.clone())
                    .ok()
                    .is_some_and(|literal| {
                        self.directory
                            .join(literal.value())
                            .canonicalize()
                            .ok()
                            .is_some_and(|path| path.starts_with(self.root))
                    });
            } else if name == "macro_rules" {
                // Macro-generated compile-time IO is outside this small audit.
                let tokens = mac.tokens.to_string();
                self.valid &= !tokens
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .any(|word| {
                        matches!(
                            word,
                            "include" | "include_str" | "include_bytes" | "env" | "option_env"
                        )
                    });
            }
            syn::visit::visit_macro(self, mac);
        }
    }
    let root = root
        .canonicalize()
        .map_err(|_| MissReason::UnreadableInput)?;
    for path in tree_files(&root, false, cancel)?
        .values()
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
    {
        check(cancel)?;
        let source = fs::read_to_string(path).map_err(|_| MissReason::UnreadableInput)?;
        audit_native_tokens(&source)?;
        let file = syn::parse_file(&source).map_err(|_| MissReason::UnsupportedDependency)?;
        let mut audit = Audit {
            root: &root,
            directory: path.parent().unwrap().to_path_buf(),
            valid: true,
        };
        audit.visit_file(&file);
        if !audit.valid {
            return Err(MissReason::UnsupportedDependency);
        }
    }
    Ok(())
}

fn miss(reason: MissReason, hints: DependencyHints) -> SealOutcome {
    SealOutcome::Uncacheable { reason, hints }
}
fn checked_hash(
    path: &Path,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<Result<Digest, MissReason>, ScriptError> {
    match file_hash(path, limit, cancel) {
        Ok(hash) => Ok(Ok(hash)),
        Err(reason) => Ok(Err(cancelled(reason)?)),
    }
}
fn root_manifest(facts: &BuildFacts) -> Result<PathBuf, MissReason> {
    let metadata: Value =
        serde_json::from_str(&facts.metadata).map_err(|_| MissReason::UnsupportedDependency)?;
    let root = metadata["resolve"]["root"]
        .as_str()
        .ok_or(MissReason::UnsupportedDependency)?;
    metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package["id"] == root && package["name"] == facts.worker_package)
        })
        .and_then(|package| package["manifest_path"].as_str())
        .map(PathBuf::from)
        .ok_or(MissReason::UnsupportedDependency)
}
fn package_version(package: &Package, metadata: &str) -> Option<String> {
    let value: Value = serde_json::from_str(metadata).ok()?;
    value["packages"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == package.name)
        .and_then(|entry| entry["version"].as_str())
        .map(str::to_owned)
}
fn generated_env(
    name: &OsStr,
    package: &str,
    version: &str,
    manifest: &Path,
    out: Option<&Path>,
    host: &str,
    cargo: &Path,
) -> Option<Option<OsString>> {
    let value = match name.to_str()? {
        "CARGO" => cargo.as_os_str().to_owned(),
        "CARGO_MANIFEST_DIR" => manifest.parent()?.as_os_str().to_owned(),
        "CARGO_MANIFEST_PATH" => manifest.as_os_str().to_owned(),
        "CARGO_PKG_NAME" => package.into(),
        "CARGO_CRATE_NAME" => package.replace('-', "_").into(),
        "CARGO_PKG_VERSION" => version.into(),
        "CARGO_PKG_VERSION_MAJOR" => version.split('.').next()?.into(),
        "CARGO_PKG_VERSION_MINOR" => version.split('.').nth(1)?.into(),
        "CARGO_PKG_VERSION_PATCH" => version.split('.').nth(2)?.split('-').next()?.into(),
        "CARGO_PKG_VERSION_PRE" => version.split_once('-').map_or("", |(_, pre)| pre).into(),
        "MODULE_ENGINE_WORKER_BUILD_HOST" | "MODULE_ENGINE_WORKER_BUILD_TARGET" => host.into(),
        "OUT_DIR" => return Some(out.map(|out| out.as_os_str().to_owned())),
        _ => return None,
    };
    Some(Some(value))
}
#[derive(Clone, Copy)]
struct EnvContext<'a> {
    package: &'a str,
    version: &'a str,
    manifest: &'a Path,
    out: Option<&'a Path>,
    sdk: bool,
    target_directory: &'a Path,
}
fn env_matches(
    info: &crate::dep_info::DepInfo,
    prepared: &PreparedInputs,
    context: EnvContext<'_>,
) -> Result<(), MissReason> {
    let EnvContext {
        package,
        version,
        manifest,
        out,
        sdk,
        target_directory,
    } = context;
    for (key, observed) in &info.env {
        let normalized;
        let key = if cfg!(windows) {
            let Some(value) = key.to_str().filter(|value| value.is_ascii()) else {
                return Err(MissReason::UnsupportedConfiguration);
            };
            normalized = OsString::from(value.to_ascii_uppercase());
            normalized.as_os_str()
        } else {
            key.as_os_str()
        };
        let generated = generated_env(
            key,
            package,
            version,
            manifest,
            out,
            &prepared.context.tool.native_host,
            &prepared.context.tool.cargo,
        );
        let unknown_cargo_provenance = !sdk
            && generated.is_none()
            && key.to_str().is_some_and(|key| key.starts_with("CARGO_"));
        if key == OsStr::new("CARGO_TARGET_DIR") {
            let actual = observed
                .as_ref()
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .and_then(|path| path.canonicalize().ok());
            if actual.is_none() || actual != target_directory.canonicalize().ok() {
                return Err(MissReason::ChangedDuringBuild);
            }
            continue;
        }
        let expected = if let Some(generated) = generated {
            generated
        } else if sdk {
            return Err(MissReason::UnsupportedDependency);
        } else {
            prepared.context.environment.get(key).cloned()
        };
        if &expected != observed {
            // Cargo supplies additional package metadata to rustc. Without an
            // explicit provenance mapping, allow this valid program to compile
            // uncached rather than misreporting an authored input mutation.
            return Err(if unknown_cargo_provenance {
                MissReason::UnsupportedConfiguration
            } else {
                MissReason::ChangedDuringBuild
            });
        }
    }
    Ok(())
}

pub(crate) fn seal_from_build(
    prepared: &PreparedInputs,
    facts: &BuildFacts,
    cancel: &AtomicBool,
) -> Result<SealOutcome, ScriptError> {
    check(cancel).map_err(|_| ScriptError::Worker("compilation cancelled".into()))?;
    let mut actual = prepared.context.clone();
    actual.metadata = facts.metadata.clone();
    actual.worker_lock = facts.worker_lock.clone();
    actual.worker_package = facts.worker_package.clone();
    let after = prepare(
        &actual,
        &prepared.source,
        prepared.kind,
        Some(&prepared.hints),
        cancel,
    )?;
    // Includes directory membership and absence/presence of configuration. This
    // before/after check is deliberately not called an atomic FS snapshot.
    if prepared.sdk_files != after.sdk_files
        || prepared.root_files != after.root_files
        || prepared.configuration_inventory != after.configuration_inventory
        || prepared.tool_inventory != after.tool_inventory
        || prepared.native_inventory != after.native_inventory
    {
        return Ok(SealOutcome::ChangedDuringBuild);
    }
    if let Eligibility::Uncacheable(reason) = &prepared.eligibility {
        return Ok(miss(reason.clone(), prepared.hints.clone()));
    }
    if after.eligibility != Eligibility::Cacheable
        || prepared.sdk_key != after.sdk_key
        || prepared.root_lookup_key != after.root_lookup_key
    {
        return Ok(SealOutcome::ChangedDuringBuild);
    }
    if facts.native_host != prepared.context.tool.native_host
        || facts.native_target != facts.native_host
    {
        return Err(ScriptError::Setup(
            "Custom native build proof changed".into(),
        ));
    }
    let manifest = match root_manifest(facts) {
        Ok(path) => path,
        Err(reason) => return Ok(miss(reason, prepared.hints.clone())),
    };
    let base_directory = manifest
        .parent()
        .ok_or_else(|| ScriptError::Setup("Worker manifest has no parent".into()))?
        .canonicalize()
        .map_err(|_| ScriptError::Setup("Worker directory is unavailable".into()))?;
    let base = base_directory.as_path();
    // Each miss uses a fresh owned worker directory. It supports no local
    // configuration; include its expected absence in the later hit proof.
    let absent_worker_configuration = [
        ".cargo/config",
        ".cargo/config.toml",
        "rust-toolchain",
        "rust-toolchain.toml",
    ]
    .map(|name| base.join(name))
    .to_vec();
    if absent_worker_configuration
        .iter()
        .any(|path| !matches!(path.try_exists(), Ok(false)))
    {
        return Ok(miss(
            MissReason::UnsupportedConfiguration,
            prepared.hints.clone(),
        ));
    }
    let mut generated = BTreeSet::new();
    let mut generated_root_inputs = BTreeMap::new();
    let mut required = BTreeSet::from(["main.rs", "user_program.rs", "build.rs"]);
    for (path, expected) in &facts.generated_inputs {
        let path = match regular(path) {
            Ok(path) => path,
            Err(reason) => return Ok(miss(reason, prepared.hints.clone())),
        };
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            return Ok(miss(MissReason::InvalidArtifact, prepared.hints.clone()));
        };
        let expected_source = match name {
            "main.rs" => Some(crate::worker_source(prepared.kind)),
            "user_program.rs" => Some(crate::user_module_source(&prepared.source, prepared.kind)),
            "build.rs" => Some(crate::cargo_build::build_script(
                &prepared.context.tool.native_host,
            )),
            "source.rs" => Some(prepared.source.clone()),
            _ => None,
        };
        if !path.starts_with(base)
            || expected_source
                .as_ref()
                .is_some_and(|source| source.as_bytes() != expected)
        {
            return Ok(SealOutcome::ChangedDuringBuild);
        }
        required.remove(name);
        match checked_hash(&path, MAX_SOURCE_BYTES, cancel)? {
            Ok(hash) if hash == hash_bytes(expected) => {
                generated_root_inputs.insert(path.clone(), hash);
            }
            Ok(_) => return Ok(SealOutcome::ChangedDuringBuild),
            Err(reason) => return Ok(miss(reason, prepared.hints.clone())),
        }
        generated.insert(path);
    }
    if !required.is_empty() || facts.root_dep_info.is_empty() {
        return Ok(miss(MissReason::AmbiguousDepInfo, prepared.hints.clone()));
    }
    let target = prepared
        .context
        .target_root
        .canonicalize()
        .map_err(|_| ScriptError::Setup("Custom target root unavailable".into()))?;
    if !facts
        .target_directory
        .canonicalize()
        .ok()
        .is_some_and(|actual| actual.starts_with(&target))
    {
        return Ok(miss(
            MissReason::UnsupportedConfiguration,
            prepared.hints.clone(),
        ));
    }
    let root_id = serde_json::from_str::<Value>(&facts.metadata)
        .ok()
        .and_then(|value| value["resolve"]["root"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let root_out = facts
        .units
        .iter()
        .find(|unit| unit.package_id == root_id && unit.out_dir.is_some())
        .and_then(|unit| unit.out_dir.as_deref());
    let mut discovered = BTreeSet::new();
    let mut hints = prepared.hints.clone();
    for dep in &facts.root_dep_info {
        let info = match crate::dep_info::read(dep, base, cancel)? {
            Ok(info) => info,
            Err(reason) => return Ok(miss(reason, hints)),
        };
        if let Err(reason) = env_matches(
            &info,
            prepared,
            EnvContext {
                package: &facts.worker_package,
                version: "0.0.0",
                manifest: &manifest,
                out: root_out,
                sdk: false,
                target_directory: &facts.target_directory,
            },
        ) {
            return Ok(if reason == MissReason::ChangedDuringBuild {
                SealOutcome::ChangedDuringBuild
            } else {
                miss(reason, hints)
            });
        }
        for path in &info.files {
            let path = match regular(path) {
                Ok(path) => path,
                Err(reason) => return Ok(miss(reason, hints)),
            };
            if generated.contains(&path) || prepared.sdk_files.contains_key(&path) {
                continue;
            }
            if path.starts_with(base) || path.starts_with(&target) {
                return Ok(miss(MissReason::UnsupportedDependency, hints));
            }
            discovered.insert(path);
        }
    }
    if discovered.len() > MAX_HINTS {
        return Ok(miss(MissReason::LimitExceeded, hints));
    }
    hints.root_files = discovered.into_iter().collect();
    if hints
        .root_files
        .iter()
        .any(|path| !prepared.root_files.contains_key(path))
    {
        // Hash-after alone cannot prove which contents rustc actually consumed.
        return Ok(miss(MissReason::NewDependency, hints));
    }
    let mut extra_sdk_files = BTreeMap::new();
    let mut dependency_units = 0;
    for unit in &facts.units {
        let Some(package) = prepared.packages.get(&unit.package_id) else {
            continue;
        };
        dependency_units += 1;
        if unit.manifest_path.canonicalize().ok().as_ref() != Some(&package.root.join("Cargo.toml"))
        {
            return Ok(miss(MissReason::UnsupportedDependency, hints));
        }
        let version = package_version(package, &facts.metadata)
            .ok_or_else(|| ScriptError::Setup("Dependency metadata changed".into()))?;
        let out = facts
            .units
            .iter()
            .find(|other| other.package_id == unit.package_id && other.out_dir.is_some())
            .and_then(|unit| unit.out_dir.as_deref());
        if let Some(out) = out
            && !out
                .canonicalize()
                .ok()
                .is_some_and(|path| path.starts_with(&target))
        {
            return Ok(miss(MissReason::UnsupportedDependency, hints));
        }
        for dep in &unit.dep_info {
            let info = match crate::dep_info::read(dep, &package.root, cancel)? {
                Ok(info) => info,
                Err(reason) => return Ok(miss(reason, hints)),
            };
            if let Err(reason) = env_matches(
                &info,
                prepared,
                EnvContext {
                    package: &package.name,
                    version: &version,
                    manifest: &unit.manifest_path,
                    out,
                    sdk: true,
                    target_directory: &facts.target_directory,
                },
            ) {
                return Ok(if reason == MissReason::ChangedDuringBuild {
                    SealOutcome::ChangedDuringBuild
                } else {
                    miss(reason, hints)
                });
            }
            for path in info.files {
                let path = match regular(&path) {
                    Ok(path) => path,
                    Err(reason) => return Ok(miss(reason, hints)),
                };
                if prepared.sdk_files.contains_key(&path) {
                    continue;
                }
                if !package.registry
                    || !out
                        .and_then(|out| out.canonicalize().ok())
                        .is_some_and(|out| path.starts_with(out))
                {
                    return Ok(miss(MissReason::UnsupportedDependency, hints));
                }
                let hash = match checked_hash(&path, MAX_SOURCE_BYTES, cancel)? {
                    Ok(hash) => hash,
                    Err(reason) => return Ok(miss(reason, hints)),
                };
                extra_sdk_files.insert(path, hash);
            }
        }
        for artifact in &unit.artifacts {
            let path = match regular(artifact) {
                Ok(path) => path,
                Err(reason) => return Ok(miss(reason, hints)),
            };
            if !path.starts_with(&target) {
                return Ok(miss(MissReason::UnsupportedDependency, hints));
            }
            let hash = match checked_hash(&path, MAX_TOOL_BYTES, cancel)? {
                Ok(hash) => hash,
                Err(reason) => return Ok(miss(reason, hints)),
            };
            extra_sdk_files.insert(path, hash);
        }
    }
    if dependency_units == 0 {
        return Ok(miss(MissReason::UnsupportedDependency, hints));
    }
    let executable = match runnable(&facts.executable) {
        Ok(path) => path,
        Err(reason) => return Ok(miss(reason, hints)),
    };
    let executable_hash = checked_hash(&executable, MAX_TOOL_BYTES, cancel)?
        .map_err(|_| ScriptError::Setup("Compiled worker proof is unavailable".into()))?;
    check(cancel).map_err(|_| ScriptError::Worker("compilation cancelled".into()))?;
    Ok(SealOutcome::Cacheable(FreshnessSeal {
        executable,
        executable_hash,
        sdk_key: prepared.sdk_key.clone(),
        root_key: prepared.root_lookup_key.clone(),
        extra_sdk_files,
        generated_root_inputs,
        absent_worker_configuration,
        hints,
    }))
}

pub(crate) fn validate(
    prepared: &PreparedInputs,
    seal: &FreshnessSeal,
    cancel: &AtomicBool,
) -> Result<Validation, ScriptError> {
    let fail = |scope, reason| Validation::Miss {
        scope,
        reason,
        hints: seal.hints.clone(),
    };
    check(cancel).map_err(|_| ScriptError::Worker("compilation cancelled".into()))?;
    if let Eligibility::Uncacheable(reason) = &prepared.eligibility {
        return Ok(fail(MissScope::Sdk, reason.clone()));
    }
    if prepared.sdk_key != seal.sdk_key {
        return Ok(fail(MissScope::Sdk, MissReason::ContentChanged));
    }
    if prepared.root_lookup_key != seal.root_key {
        return Ok(fail(MissScope::Root, MissReason::ContentChanged));
    }
    if seal
        .absent_worker_configuration
        .iter()
        .any(|path| !matches!(path.try_exists(), Ok(false)))
    {
        return Ok(fail(MissScope::Sdk, MissReason::UnsupportedConfiguration));
    }
    for (path, expected) in &seal.generated_root_inputs {
        match checked_hash(path, MAX_SOURCE_BYTES, cancel)? {
            Ok(actual) if actual == *expected => {}
            _ => return Ok(fail(MissScope::Root, MissReason::InvalidArtifact)),
        }
    }
    for (path, expected) in &seal.extra_sdk_files {
        match checked_hash(path, MAX_TOOL_BYTES, cancel)? {
            Ok(actual) if actual == *expected => {}
            _ => return Ok(fail(MissScope::Sdk, MissReason::InvalidArtifact)),
        }
    }
    if runnable(&seal.executable).is_err() {
        return Ok(fail(MissScope::Root, MissReason::InvalidArtifact));
    }
    match checked_hash(&seal.executable, MAX_TOOL_BYTES, cancel)? {
        Ok(actual) if actual == seal.executable_hash => Ok(Validation::Hit),
        _ => Ok(fail(MissScope::Root, MissReason::InvalidArtifact)),
    }
}

/// Cargo-controlled material for the exact audited SDK closure. Worker products
/// are deliberately excluded: each new source owns a different root package.
/// The store holds its cross-process namespace lease while comparing/publishing
/// this receipt, before Cargo is allowed to consume preexisting SDK products.
pub(crate) fn sdk_outputs(
    prepared: &PreparedInputs,
    target: &Path,
    cancel: &AtomicBool,
) -> Result<Result<BTreeMap<PathBuf, Digest>, MissReason>, ScriptError> {
    match sdk_outputs_inner(prepared, target, cancel) {
        Ok(files) => Ok(Ok(files)),
        Err(reason) => Ok(Err(cancelled(reason)?)),
    }
}
fn cargo_slot(name: &str, package: &str) -> bool {
    name.strip_prefix(package)
        .and_then(|tail| tail.strip_prefix('-'))
        .is_some_and(|hash| {
            hash.len() == 16
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}
fn sdk_outputs_inner(
    prepared: &PreparedInputs,
    target: &Path,
    cancel: &AtomicBool,
) -> Result<BTreeMap<PathBuf, Digest>, MissReason> {
    check(cancel)?;
    let target = target
        .canonicalize()
        .map_err(|_| MissReason::UnreadableInput)?;
    if !target.starts_with(
        prepared
            .context
            .target_root
            .canonicalize()
            .map_err(|_| MissReason::UnreadableInput)?,
    ) {
        return Err(MissReason::InvalidArtifact);
    }
    let packages = prepared
        .packages
        .values()
        .map(|package| package.name.as_str())
        .collect::<BTreeSet<_>>();
    let crates = prepared
        .packages
        .values()
        .flat_map(|package| package.crate_names.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut selected = BTreeSet::new();
    let mut visited = 0_usize;
    let mut add_tree = |root: &Path| -> Result<(), MissReason> {
        selected.extend(tree_files(root, false, cancel)?.into_values());
        if selected.len() > MAX_FILES {
            return Err(MissReason::LimitExceeded);
        }
        Ok(())
    };
    for prefix in [
        target.clone(),
        target.join(&prepared.context.tool.native_host),
    ] {
        for profile in ["debug", "release"] {
            for folder in [".fingerprint", "build"] {
                let directory = prefix.join(profile).join(folder);
                if !directory
                    .try_exists()
                    .map_err(|_| MissReason::UnreadableInput)?
                {
                    continue;
                }
                for entry in fs::read_dir(directory).map_err(|_| MissReason::UnreadableInput)? {
                    check(cancel)?;
                    visited += 1;
                    if visited > MAX_FILES * 4 {
                        return Err(MissReason::LimitExceeded);
                    }
                    let entry = entry.map_err(|_| MissReason::UnreadableInput)?;
                    let name = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| MissReason::UnsupportedDependency)?;
                    if packages.iter().any(|package| cargo_slot(&name, package)) {
                        let kind = entry.file_type().map_err(|_| MissReason::UnreadableInput)?;
                        if !kind.is_dir() || kind.is_symlink() {
                            return Err(MissReason::InvalidArtifact);
                        }
                        add_tree(&entry.path())?;
                    }
                }
            }
        }
    }
    for prefix in [
        target.clone(),
        target.join(&prepared.context.tool.native_host),
    ] {
        for profile in ["debug", "release"] {
            let directory = prefix.join(profile).join("deps");
            if !directory
                .try_exists()
                .map_err(|_| MissReason::UnreadableInput)?
            {
                continue;
            }
            for entry in fs::read_dir(directory).map_err(|_| MissReason::UnreadableInput)? {
                check(cancel)?;
                visited += 1;
                if visited > MAX_FILES * 4 {
                    return Err(MissReason::LimitExceeded);
                }
                let entry = entry.map_err(|_| MissReason::UnreadableInput)?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| MissReason::UnsupportedDependency)?;
                let stem = name.split_once('.').map(|(stem, _)| stem).unwrap_or(&name);
                if crates.iter().any(|package| {
                    cargo_slot(stem, package)
                        || stem
                            .strip_prefix("lib")
                            .is_some_and(|stem| cargo_slot(stem, package))
                }) {
                    selected.insert(entry.path());
                    if selected.len() > MAX_FILES {
                        return Err(MissReason::LimitExceeded);
                    }
                }
            }
        }
    }
    // Cargo can consume cached compiler query results before deciding freshness.
    let rustc_info = target.join(".rustc_info.json");
    if rustc_info
        .try_exists()
        .map_err(|_| MissReason::UnreadableInput)?
    {
        selected.insert(rustc_info);
    }
    let mut result = BTreeMap::new();
    for path in selected {
        let actual = regular(&path)?;
        if !actual.starts_with(&target) {
            return Err(MissReason::InvalidArtifact);
        }
        result.insert(actual.clone(), file_hash(&actual, MAX_TOOL_BYTES, cancel)?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(1);
    struct Fixture {
        directory: PathBuf,
        context: CompileContext,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
    fn write(path: &Path, bytes: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn registry_root() -> PathBuf {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let homes = [
            std::env::var_os("CARGO_HOME").map(PathBuf::from),
            Some(workspace.join(".work/cargo-home")),
            Some(workspace.join(".tools/linux-x86_64/cargo")),
        ];
        homes
            .into_iter()
            .flatten()
            .find_map(|home| {
                fs::read_dir(home.join("registry/src"))
                    .ok()?
                    .flatten()
                    .map(|entry| entry.path())
                    .find(|root| root.join("sha2-0.10.9/Cargo.toml").is_file())
            })
            .expect("The offline locked registry used by the Runtime tests must be available")
    }
    fn fixture() -> Fixture {
        let directory = std::env::temp_dir().join(format!(
            "module_engine_freshness_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let sdk = directory.join("sdk");
        let work = directory.join("work");
        let target = directory.join("target");
        let sysroot = directory.join("tool");
        let cargo_home = directory.join("cargo-home");
        fs::create_dir(&cargo_home).unwrap();
        let host = "fixture-native";
        for root in [&work, &target] {
            fs::create_dir_all(root).unwrap();
        }
        write(
            &sdk.join("Cargo.toml"),
            "[workspace]\nmembers=[\"crates/engine-core\"]\n",
        );
        write(&sdk.join("engine-sdk.json"), "{}");
        write(
            &sdk.join("crates/engine-core/Cargo.toml"),
            "[package]\nname=\"module-engine-core\"\nversion=\"0.1.0\"\n",
        );
        write(
            &sdk.join("crates/engine-core/src/lib.rs"),
            "pub fn core() {}\n",
        );
        let mut lock = String::from("version = 4\n");
        let registry = registry_root();
        let worker = "module_engine_worker_fixture";
        let mut packages = vec![
            json!({"id":worker,"name":worker,"version":"0.0.0","source":null,"manifest_path":work.join("Cargo.toml"),"targets":[{"kind":["bin"],"name":worker}]}),
            json!({"id":"core","name":"module-engine-core","version":"0.1.0","source":null,"manifest_path":sdk.join("crates/engine-core/Cargo.toml"),"targets":[{"kind":["lib"],"name":"module_engine_core"}]}),
        ];
        for package in audited_dependencies::PACKAGES {
            lock.push_str(&format!(
                "\n[[package]]\nname = {:?}\nversion = {:?}\nsource = {:?}\nchecksum = {:?}\n",
                package.name, package.version, package.source, package.archive
            ));
            packages.push(json!({"id":package.name,"name":package.name,"version":package.version,"source":package.source,"manifest_path":registry.join(format!("{}-{}/Cargo.toml",package.name,package.version)),"targets":[{"kind":["lib"],"name":package.name.replace('-',"_")}]}));
        }
        write(&sdk.join("Cargo.lock"), &lock);
        write(&work.join("Cargo.toml"), "[package]\n");
        let cargo = sysroot.join("bin/cargo");
        let rustc = sysroot.join("bin/rustc");
        let native = sysroot.join(format!("lib/rustlib/{host}/lib/standard.rlib"));
        for path in [&cargo, &rustc, &native] {
            write(path, b"fixture-tool");
        }
        let nodes = packages
            .iter()
            .map(|package| json!({"id":package["id"],"features":[]}))
            .collect::<Vec<_>>();
        let metadata =
            json!({"packages":packages,"resolve":{"root":worker,"nodes":nodes}}).to_string();
        let context = CompileContext {
            sdk_root: sdk,
            work_root: work,
            target_root: target,
            cargo_home: Some(cargo_home),
            sdk_identity: SdkIdentity::current(),
            tool: ToolIdentity {
                cargo,
                rustc,
                cargo_version: "fixture cargo".into(),
                rustc_version: "fixture rustc".into(),
                sysroot: sysroot.clone(),
                native_host: host.into(),
                code_inputs: vec![native],
                code_input_directories: vec![
                    sysroot.join("bin"),
                    sysroot.join(format!("lib/rustlib/{host}")),
                ],
                native_searches: vec![],
            },
            environment: BTreeMap::new(),
            metadata,
            root_lock: lock.clone(),
            worker_lock: lock,
            worker_package: worker.into(),
        };
        Fixture { directory, context }
    }
    fn prepared(f: &Fixture, hints: Option<&DependencyHints>) -> PreparedInputs {
        let proof = prepare(
            &f.context,
            "fn update() {}",
            ScriptKind::Update,
            hints,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(proof.eligibility, Eligibility::Cacheable);
        proof
    }
    fn facts(f: &Fixture, external: Option<&Path>) -> BuildFacts {
        let root = &f.context.work_root;
        let generated_inputs = vec![
            (
                root.join("main.rs"),
                crate::worker_source(ScriptKind::Update).into_bytes(),
            ),
            (
                root.join("user_program.rs"),
                crate::user_module_source("fn update() {}", ScriptKind::Update).into_bytes(),
            ),
            (
                root.join("build.rs"),
                crate::cargo_build::build_script(&f.context.tool.native_host).into_bytes(),
            ),
        ];
        for (path, bytes) in &generated_inputs {
            write(path, bytes);
        }
        let dep = root.join("worker.d");
        let paths = generated_inputs
            .iter()
            .map(|(path, _)| path.as_path())
            .chain(external)
            .map(|path| path.to_string_lossy().replace(' ', "\\ "))
            .collect::<Vec<_>>()
            .join(" ");
        write(&dep, format!("worker: {paths}\n"));
        let core_dep = root.join("core.d");
        write(
            &core_dep,
            format!(
                "core: {}\n",
                f.context
                    .sdk_root
                    .join("crates/engine-core/src/lib.rs")
                    .to_string_lossy()
                    .replace(' ', "\\ ")
            ),
        );
        let artifact = f.context.target_root.join("core.rlib");
        let executable = f.context.target_root.join("worker.exe");
        write(&artifact, "core artifact");
        write(&executable, "worker binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
        BuildFacts {
            metadata: f.context.metadata.clone(),
            worker_lock: f.context.worker_lock.clone(),
            worker_package: f.context.worker_package.clone(),
            executable,
            native_host: f.context.tool.native_host.clone(),
            native_target: f.context.tool.native_host.clone(),
            target_directory: f.context.target_root.clone(),
            units: vec![BuildUnit {
                package_id: "core".into(),
                manifest_path: f.context.sdk_root.join("crates/engine-core/Cargo.toml"),
                kinds: vec!["lib".into()],
                artifacts: vec![artifact],
                dep_info: vec![core_dep],
                out_dir: None,
                environment: BTreeMap::new(),
                fresh: false,
            }],
            root_dep_info: vec![dep],
            generated_inputs,
        }
    }
    #[test]
    fn source_environment_and_restored_mtime_content_choose_correct_keys() {
        let mut f = fixture();
        let first = prepared(&f, None);
        f.context.environment.insert("DISPLAY".into(), ":9".into());
        f.context
            .environment
            .insert("XDG_SESSION_TYPE".into(), "wayland".into());
        f.context
            .environment
            .insert("CARGO_TARGET_DIR".into(), "owned-generation".into());
        let ui = prepared(&f, None);
        assert_eq!(first.sdk_key, ui.sdk_key);
        assert_ne!(first.root_lookup_key, ui.root_lookup_key);
        let source = prepare(
            &f.context,
            "fn update() { let x=1; }",
            ScriptKind::Update,
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(ui.sdk_key, source.sdk_key);
        assert_ne!(ui.root_lookup_key, source.root_lookup_key);
        let mut empty = f.context.clone();
        empty
            .environment
            .insert("OPTIONAL_INPUT".into(), OsString::new());
        let empty = prepare(
            &empty,
            "fn update() {}",
            ScriptKind::Update,
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_ne!(empty.root_lookup_key, ui.root_lookup_key);
        let core = f.context.sdk_root.join("crates/engine-core/src/lib.rs");
        let modified = fs::metadata(&core).unwrap().modified().unwrap();
        write(&core, "pub fn more() {}\n");
        fs::File::options()
            .write(true)
            .open(&core)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_ne!(ui.sdk_key, prepared(&f, None).sdk_key);
    }
    #[test]
    fn seal_discovers_includes_before_reuse_and_detects_during_build_changes() {
        let f = fixture();
        let include = f.directory.join("custom.rs");
        write(&include, "const VALUE:u8=1;");
        let first = prepared(&f, None);
        let build = facts(&f, Some(&include));
        let hints = match seal_from_build(&first, &build, &AtomicBool::new(false)).unwrap() {
            SealOutcome::Uncacheable {
                reason: MissReason::NewDependency,
                hints,
            } => hints,
            result => panic!("Expected first-build include miss, got {result:?}"),
        };
        let before = prepared(&f, Some(&hints));
        let seal = match seal_from_build(&before, &build, &AtomicBool::new(false)).unwrap() {
            SealOutcome::Cacheable(seal) => seal,
            result => panic!("{result:?}"),
        };
        assert!(matches!(
            validate(&before, &seal, &AtomicBool::new(false)).unwrap(),
            Validation::Hit
        ));
        let modified = fs::metadata(&include).unwrap().modified().unwrap();
        write(&include, "const VALUE:u8=2;");
        fs::File::options()
            .write(true)
            .open(&include)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert!(matches!(
            seal_from_build(&before, &build, &AtomicBool::new(false)).unwrap(),
            SealOutcome::ChangedDuringBuild
        ));
        let after = prepared(&f, Some(&hints));
        assert_eq!(before.sdk_key, after.sdk_key);
        assert_ne!(before.root_lookup_key, after.root_lookup_key);
        assert!(matches!(
            validate(&after, &seal, &AtomicBool::new(false)).unwrap(),
            Validation::Miss {
                scope: MissScope::Root,
                ..
            }
        ));
    }
    #[test]
    fn seal_checks_configuration_absence_membership_and_artifact_contents() {
        let f = fixture();
        let before = prepared(&f, None);
        let build = facts(&f, None);
        let seal = match seal_from_build(&before, &build, &AtomicBool::new(false)).unwrap() {
            SealOutcome::Cacheable(seal) => seal,
            result => panic!("{result:?}"),
        };
        write(&build.executable, "tampered worker");
        assert!(matches!(
            validate(&before, &seal, &AtomicBool::new(false)).unwrap(),
            Validation::Miss {
                scope: MissScope::Root,
                ..
            }
        ));
        write(&build.executable, "worker binary");
        write(&build.generated_inputs[0].0, "tampered generated main");
        assert!(matches!(
            validate(&before, &seal, &AtomicBool::new(false)).unwrap(),
            Validation::Miss {
                scope: MissScope::Root,
                ..
            }
        ));
        write(&build.generated_inputs[0].0, &build.generated_inputs[0].1);
        write(&build.units[0].artifacts[0], "tampered library");
        assert!(matches!(
            validate(&before, &seal, &AtomicBool::new(false)).unwrap(),
            Validation::Miss {
                scope: MissScope::Sdk,
                ..
            }
        ));
        write(&f.context.work_root.join(".cargo/config.toml"), "");
        assert!(matches!(
            validate(&before, &seal, &AtomicBool::new(false)).unwrap(),
            Validation::Miss {
                scope: MissScope::Sdk,
                reason: MissReason::UnsupportedConfiguration,
                ..
            }
        ));
        assert!(matches!(
            seal_from_build(&before, &build, &AtomicBool::new(false)).unwrap(),
            SealOutcome::ChangedDuringBuild
        ));
        fs::remove_file(f.context.work_root.join(".cargo/config.toml")).unwrap();
        write(
            &f.context.tool.sysroot.join("bin/new-compiler.dll"),
            "new loader input",
        );
        assert!(matches!(
            seal_from_build(&before, &build, &AtomicBool::new(false)).unwrap(),
            SealOutcome::ChangedDuringBuild
        ));
    }
    #[test]
    fn unsupported_inputs_and_missing_hints_remain_safe_misses() {
        let mut f = fixture();
        for (key, value) in [
            ("RUSTFLAGS", "--cfg custom"),
            ("LD_PRELOAD", "inject.so"),
            ("LD_LIBRARY_PATH", "/arbitrary"),
            ("DYLD_LIBRARY_PATH", "/arbitrary"),
        ] {
            f.context.environment = BTreeMap::from([(key.into(), value.into())]);
            let proof = prepare(
                &f.context,
                "fn update() {}",
                ScriptKind::Update,
                None,
                &AtomicBool::new(false),
            )
            .unwrap();
            if key == "RUSTFLAGS" || cfg!(unix) {
                assert!(matches!(proof.eligibility, Eligibility::Uncacheable(_)));
            } else {
                assert_eq!(proof.eligibility, Eligibility::Cacheable);
            }
        }
        f.context.environment.clear();
        let hints = DependencyHints {
            root_files: vec![f.directory.join("missing.rs")],
        };
        let proof = prepare(
            &f.context,
            "fn update() {}",
            ScriptKind::Update,
            Some(&hints),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            proof.eligibility,
            Eligibility::Uncacheable(MissReason::NewDependency)
        );
        assert!(proof.hints.root_files.is_empty());
        let refreshed = prepare(
            &f.context,
            "fn update() {}",
            ScriptKind::Update,
            Some(&proof.hints),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(refreshed.eligibility, Eligibility::Cacheable);
        for source in [
            "#[link(name=\"custom\")] extern \"C\" {}",
            "fn x(){ global_asm!(\".incbin secret\"); }",
            "macro_rules! x {()=>{#[link(name=\"custom\")] extern \"C\" {}}}",
            "#[r#link(name=\"custom\")] unsafe extern \"C\" {}",
            "fn x(){ core::arch::r#global_asm!(\".incbin secret\"); }",
            "#[used] #[unsafe(link_section=\".drectve\")] static FLAGS:[u8;12]=*b\"/DEFAULTLIB:x\";",
            "#[link_args=\"@native-options.rsp\"] unsafe extern \"C\" {}",
        ] {
            assert!(audit_native_tokens(source).is_err());
        }
        assert!(
            prepare(
                &f.context,
                "fn update() {}",
                ScriptKind::Update,
                None,
                &AtomicBool::new(true)
            )
            .is_err()
        );
        let debug = format!("{:?}", f.context);
        assert!(debug.contains("<private>"));
    }
    #[test]
    fn registry_tree_digest_rejects_added_files_and_has_fixed_framing() {
        let f = fixture();
        let tree = f.directory.join("tree");
        write(&tree.join("a.rs"), "one");
        write(&tree.join("nested/b.rs"), "two");
        write(&tree.join(".cargo-ok"), "marker");
        let (digest, count, bytes, _) = tree_hash(&tree, true, &AtomicBool::new(false)).unwrap();
        let mut expected = Sha256::new_with_prefix(TREE_MAGIC);
        expected.update(2_u64.to_le_bytes());
        part(&mut expected, b"a.rs");
        part(&mut expected, b"one");
        part(&mut expected, b"nested/b.rs");
        part(&mut expected, b"two");
        assert_eq!(digest, finish(expected));
        assert_eq!((count, bytes), (2, 6));
        write(&tree.join("unexpected"), "");
        assert_ne!(
            digest,
            tree_hash(&tree, true, &AtomicBool::new(false)).unwrap().0
        );
        let root = registry_root();
        for known in audited_dependencies::PACKAGES {
            let (actual, count, bytes, _) = tree_hash(
                &root.join(format!("{}-{}", known.name, known.version)),
                true,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(
                (actual.as_str(), count, bytes),
                (known.tree, known.files, known.bytes)
            );
        }
    }
    #[test]
    fn loader_exception_requires_exact_regular_dxc_membership() {
        let mut f = fixture();
        let libs = f.directory.join("original-dxc/lib");
        write(&libs.join("libdxcompiler.so"), "compiler");
        write(&libs.join("libdxil.so"), "dxil");
        f.context
            .environment
            .insert("LD_LIBRARY_PATH".into(), libs.as_os_str().to_owned());
        assert!(loader_inputs_on_platform(&f.context, true, &AtomicBool::new(false)).is_ok());
        fs::create_dir(libs.join("empty-subdir")).unwrap();
        assert!(loader_inputs_on_platform(&f.context, true, &AtomicBool::new(false)).is_err());
        fs::remove_dir(libs.join("empty-subdir")).unwrap();
        write(&libs.join("libLLVM.so"), "injection");
        assert!(loader_inputs_on_platform(&f.context, true, &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn sdk_output_receipt_tracks_controls_membership_and_excludes_worker_products() {
        let mut f = fixture();
        // Cargo target names can differ from the package name for a modified SDK.
        let mut metadata: Value = serde_json::from_str(&f.context.metadata).unwrap();
        metadata["packages"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|package| package["id"] == "core")
            .unwrap()["targets"][0]["name"] = json!("custom_sdk");
        f.context.metadata = metadata.to_string();
        let proof = prepared(&f, None);
        let target = &f.context.target_root;
        let hash = "0123456789abcdef";
        let library = target.join(format!("debug/deps/libcustom_sdk-{hash}.rlib"));
        let output = target.join(format!("debug/build/serde-{hash}/output"));
        let fingerprint = target.join(format!("debug/.fingerprint/serde-{hash}/lib-serde.json"));
        write(&library, "SDK binary");
        write(&output, "cfg=one");
        write(&fingerprint, "fingerprint");
        write(
            &target.join(format!("debug/build/serde-{hash}/out/private.rs")),
            "generated",
        );
        write(&target.join(".rustc_info.json"), "compiler queries");
        let before = sdk_outputs(&proof, target, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(before.len(), 5);
        write(
            &target.join(format!("debug/deps/module_engine_worker_other-{hash}.exe")),
            "new worker",
        );
        write(
            &target.join(format!(
                "debug/.fingerprint/module_engine_worker_other-{hash}/bin.json"
            )),
            "new worker metadata",
        );
        write(
            &target.join(format!("debug/deps/libserde-other-owner-{hash}.rlib")),
            "unrelated",
        );
        assert_eq!(
            before,
            sdk_outputs(&proof, target, &AtomicBool::new(false))
                .unwrap()
                .unwrap()
        );
        let modified = fs::metadata(&output).unwrap().modified().unwrap();
        write(&output, "cfg=two");
        fs::File::options()
            .write(true)
            .open(&output)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let after = sdk_outputs(&proof, target, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_ne!(before, after);
        write(
            &target.join(format!("debug/build/serde-{hash}/out/new.rs")),
            "new generated dependency",
        );
        assert_ne!(
            after,
            sdk_outputs(&proof, target, &AtomicBool::new(false))
                .unwrap()
                .unwrap()
        );
        assert!(sdk_outputs(&proof, target, &AtomicBool::new(true)).is_err());
        assert!(!cargo_slot("serde-0123456789abcdeF", "serde"));
        assert!(!cargo_slot("serde-other-owner-0123456789abcdef", "serde"));
    }
    #[test]
    fn warm_seal_cannot_bless_changed_immutable_program_or_dependency() {
        let f = fixture();
        let proof = prepared(&f, None);
        let build = facts(&f, None);
        let sealed = || match seal_from_build(&proof, &build, &AtomicBool::new(false)).unwrap() {
            SealOutcome::Cacheable(seal) => seal,
            result => panic!("{result:?}"),
        };
        let first = sealed();
        assert!(first.same_products(&sealed()));
        write(&build.executable, "changed runnable");
        assert!(!first.same_products(&sealed()));
        write(&build.executable, "worker binary");
        write(&build.units[0].artifacts[0], "changed SDK artifact");
        assert!(!first.same_products(&sealed()));
    }
    #[test]
    fn controlled_target_env_compares_canonical_directory_identity() {
        let f = fixture();
        let proof = prepared(&f, None);
        let manifest = f.context.work_root.join("Cargo.toml");
        let context = EnvContext {
            package: "worker",
            version: "0.0.0",
            manifest: &manifest,
            out: None,
            sdk: false,
            target_directory: &f.context.target_root,
        };
        let mut info = crate::dep_info::DepInfo::default();
        info.env.insert(
            "CARGO_TARGET_DIR".into(),
            Some(f.context.target_root.join(".").into_os_string()),
        );
        assert!(env_matches(&info, &proof, context).is_ok());
        info.env.insert(
            "CARGO_TARGET_DIR".into(),
            Some(f.context.work_root.clone().into_os_string()),
        );
        assert_eq!(
            env_matches(&info, &proof, context),
            Err(MissReason::ChangedDuringBuild)
        );
    }
    #[test]
    fn unmapped_cargo_metadata_mismatch_is_uncacheable_but_authored_env_stays_strict() {
        let mut f = fixture();
        f.context
            .environment
            .insert("ORDINARY_INPUT".into(), "before".into());
        let proof = prepared(&f, None);
        let manifest = f.context.work_root.join("Cargo.toml");
        let context = EnvContext {
            package: "worker",
            version: "0.0.0",
            manifest: &manifest,
            out: None,
            sdk: false,
            target_directory: &f.context.target_root,
        };
        let mut info = crate::dep_info::DepInfo::default();
        for name in ["CARGO_PKG_DESCRIPTION", "CARGO_PKG_AUTHORS"] {
            info.env = BTreeMap::from([(name.into(), Some(OsString::new()))]);
            assert_eq!(
                env_matches(&info, &proof, context),
                Err(MissReason::UnsupportedConfiguration)
            );
        }
        info.env = BTreeMap::from([("ORDINARY_INPUT".into(), Some("after".into()))]);
        assert_eq!(
            env_matches(&info, &proof, context),
            Err(MissReason::ChangedDuringBuild)
        );
        info.env = BTreeMap::from([("CARGO_PKG_NAME".into(), Some("other-worker".into()))]);
        assert_eq!(
            env_matches(&info, &proof, context),
            Err(MissReason::ChangedDuringBuild)
        );
    }
    #[test]
    fn implicit_or_relative_cargo_home_never_claims_configuration_proof() {
        let mut f = fixture();
        for home in [None, Some(PathBuf::from("relative-cargo-home"))] {
            f.context.cargo_home = home;
            let proof = prepare(
                &f.context,
                "fn update() {}",
                ScriptKind::Update,
                None,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(
                proof.eligibility,
                Eligibility::Uncacheable(MissReason::UnsupportedConfiguration)
            );
        }
        #[cfg(windows)]
        {
            f.context.cargo_home = Some(PathBuf::from("E:drive-relative-home"));
            let proof = prepare(
                &f.context,
                "fn update() {}",
                ScriptKind::Update,
                None,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(
                proof.eligibility,
                Eligibility::Uncacheable(MissReason::UnsupportedConfiguration)
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn same_content_without_execute_permission_requires_artifact_repair() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture();
        let proof = prepared(&f, None);
        let build = facts(&f, None);
        let seal = match seal_from_build(&proof, &build, &AtomicBool::new(false)).unwrap() {
            SealOutcome::Cacheable(seal) => seal,
            result => panic!("{result:?}"),
        };
        let modified = fs::metadata(&build.executable).unwrap().modified().unwrap();
        fs::set_permissions(&build.executable, fs::Permissions::from_mode(0o644)).unwrap();
        fs::File::options()
            .write(true)
            .open(&build.executable)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(
            file_hash(&build.executable, MAX_TOOL_BYTES, &AtomicBool::new(false)).unwrap(),
            seal.executable_hash
        );
        assert!(matches!(
            validate(&proof, &seal, &AtomicBool::new(false)).unwrap(),
            Validation::Miss {
                scope: MissScope::Root,
                reason: MissReason::InvalidArtifact,
                ..
            }
        ));
        assert!(matches!(
            seal_from_build(&proof, &build, &AtomicBool::new(false)).unwrap(),
            SealOutcome::Uncacheable {
                reason: MissReason::InvalidArtifact,
                ..
            }
        ));
        fs::set_permissions(&build.executable, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            validate(&proof, &seal, &AtomicBool::new(false)).unwrap(),
            Validation::Hit
        ));
    }
}
