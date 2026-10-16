//! Cargo's product paths are authoritative. A target triple is proven by the
//! root build script, not inferred from a filename or compiler-artifact.target.
use crate::{ScriptError, freshness::BuildUnit};
use serde_json::Value;
#[cfg(windows)]
use std::io::{Seek, SeekFrom};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const LINE_LIMIT: usize = 1024 * 1024;
const PRODUCT_LIMIT: usize = 64;
const DIAGNOSTIC_LIMIT: usize = 256 * 1024;
const HOST_KEY: &str = "MODULE_ENGINE_WORKER_BUILD_HOST";
const TARGET_KEY: &str = "MODULE_ENGINE_WORKER_BUILD_TARGET";

#[derive(Clone, Debug)]
pub struct Selector {
    package: String,
    manifest: PathBuf,
    name: String,
}

impl Selector {
    /// Select one workspace binary by its exact manifest, package and target.
    /// Unlike a generated Custom package, a virtual workspace has no resolve root.
    pub fn workspace_binary(
        text: &str,
        manifest: &Path,
        package_name: &str,
        binary_name: &str,
    ) -> Result<Self, ScriptError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|error| ScriptError::Compile(format!("Invalid Cargo metadata: {error}")))?;
        let manifest = manifest.canonicalize().map_err(setup)?;
        let packages: Vec<_> = value["packages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|package| {
                package["name"].as_str() == Some(package_name)
                    && package["manifest_path"].as_str().is_some_and(|path| {
                        Path::new(path).canonicalize().ok().as_ref() == Some(&manifest)
                    })
                    && package["targets"].as_array().is_some_and(|targets| {
                        targets.iter().any(|target| {
                            target["name"].as_str() == Some(binary_name)
                                && target["kind"]
                                    .as_array()
                                    .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
                        })
                    })
            })
            .collect();
        if packages.len() != 1 {
            return Err(ScriptError::Compile(
                "Cargo metadata did not identify exactly one selected native package/binary".into(),
            ));
        }
        let package = packages[0]["id"]
            .as_str()
            .ok_or_else(|| ScriptError::Compile("Cargo package identity is missing".into()))?;
        Ok(Self {
            package: package.into(),
            manifest,
            name: binary_name.into(),
        })
    }

    pub(crate) fn from_metadata(
        text: &str,
        manifest: &Path,
        name: &str,
    ) -> Result<Self, ScriptError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|e| ScriptError::Compile(format!("Invalid Cargo metadata: {e}")))?;
        let root = value["resolve"]["root"].as_str().ok_or_else(|| {
            ScriptError::Compile("Cargo metadata did not identify the worker root package".into())
        })?;
        let manifest = manifest.canonicalize().map_err(setup)?;
        let matches = value["packages"].as_array().is_some_and(|packages| {
            packages.iter().any(|package| {
                package["id"].as_str() == Some(root)
                    && package["name"].as_str() == Some(name)
                    && package["manifest_path"].as_str().is_some_and(|path| {
                        Path::new(path).canonicalize().ok().as_ref() == Some(&manifest)
                    })
            })
        });
        if !matches {
            return Err(ScriptError::Compile(
                "Cargo metadata root does not match the owned worker manifest".into(),
            ));
        }
        Ok(Self {
            package: root.into(),
            manifest,
            name: name.into(),
        })
    }
}

#[derive(Default, Debug)]
pub struct BuildOutput {
    executables: BTreeSet<PathBuf>,
    pub(crate) products: BTreeSet<PathBuf>,
    native: BTreeSet<(String, String)>,
    finished: Option<bool>,
    error: Option<String>,
    pub(crate) diagnostics: String,
    pub(crate) units: Vec<BuildUnit>,
    pub(crate) root_dep_info: Vec<PathBuf>,
}

impl BuildOutput {
    /// Rendered compiler diagnostics, bounded independently of the JSON stream.
    pub fn diagnostics(&self) -> &str {
        &self.diagnostics
    }

    pub fn executable(&self, host: &str, target_root: &Path) -> Result<PathBuf, ScriptError> {
        if let Some(error) = &self.error {
            return Err(ScriptError::Compile(error.clone()));
        }
        if self.finished != Some(true) {
            return Err(ScriptError::Compile(
                "Cargo did not report a successful build-finished message".into(),
            ));
        }
        if self.native.len() != 1 || !self.native.contains(&(host.into(), host.into())) {
            return Err(ScriptError::Compile(format!(
                "Worker native target proof is missing or incompatible; expected HOST=TARGET={host}. Remove a foreign build.target/CARGO_BUILD_TARGET override."
            )));
        }
        if self.executables.len() != 1 {
            return Err(ScriptError::Compile(format!(
                "Cargo reported {} distinct worker executables; expected exactly one",
                self.executables.len()
            )));
        }
        let executable = self.executables.first().unwrap();
        let executable = executable.canonicalize().map_err(|error| {
            ScriptError::Compile(format!(
                "Cargo-reported worker executable is unavailable at {}: {error}",
                executable.display()
            ))
        })?;
        let target = target_root.canonicalize().map_err(setup)?;
        if !executable.starts_with(&target) || !executable.is_file() {
            return Err(ScriptError::Compile(
                "Cargo worker executable escapes the selected target directory or is not a file"
                    .into(),
            ));
        }
        validate_native_executable(&executable)?;
        Ok(executable)
    }
}

pub struct Collector {
    selector: Selector,
    line: Vec<u8>,
    dropping_line: bool,
    output: BuildOutput,
    collect_dependency_units: bool,
}

impl Collector {
    pub(crate) fn new(selector: Selector) -> Self {
        Self {
            selector,
            line: vec![],
            dropping_line: false,
            output: BuildOutput::default(),
            collect_dependency_units: true,
        }
    }

    /// Reuse the same product/native checks without retaining dependency facts
    /// for a large Player build. Custom freshness still collects those facts.
    pub fn executable_only(selector: Selector) -> Self {
        Self {
            collect_dependency_units: false,
            ..Self::new(selector)
        }
    }

    /// Drain indefinitely, but retain at most one bounded line and a bounded
    /// set of root products and dependency unit facts.
    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                if !self.dropping_line {
                    self.parse_line();
                }
                self.line.clear();
                self.dropping_line = false;
            } else if !self.dropping_line {
                if self.line.len() == LINE_LIMIT {
                    self.output
                        .error
                        .get_or_insert_with(|| "Cargo JSON line exceeds 1 MiB".into());
                    self.line.clear();
                    self.dropping_line = true;
                } else {
                    self.line.push(byte);
                }
            }
        }
    }

    pub fn finish(mut self) -> BuildOutput {
        if !self.dropping_line && !self.line.is_empty() {
            self.parse_line();
        }
        for unit in &mut self.output.units {
            unit.dep_info = dependency_info_paths(&unit.artifacts, &unit.kinds);
            if unit.package_id == self.selector.package {
                self.output
                    .root_dep_info
                    .extend(unit.dep_info.iter().cloned());
            }
        }
        self.output.root_dep_info.sort();
        self.output.root_dep_info.dedup();
        self.output
    }

    fn facts(&mut self, value: &Value) {
        let Some(package) = value["package_id"].as_str() else {
            return;
        };
        if !matches!(
            value["reason"].as_str(),
            Some("compiler-artifact" | "build-script-executed")
        ) {
            return;
        }
        if package.len() > 16384 || self.output.units.len() >= 256 {
            self.output
                .error
                .get_or_insert_with(|| "Cargo dependency fact limit exceeded".into());
            return;
        }
        let is_artifact = value["reason"] == "compiler-artifact";
        let kinds: Vec<String> = value["target"]["kind"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect();
        let index = self.output.units.iter().position(|unit| {
            unit.package_id == package
                && (!is_artifact && unit.kinds.iter().any(|k| k == "custom-build")
                    || is_artifact && (unit.kinds == kinds || unit.kinds.is_empty()))
        });
        let index = index.unwrap_or_else(|| {
            self.output.units.push(BuildUnit {
                package_id: package.into(),
                manifest_path: PathBuf::new(),
                kinds: vec![],
                artifacts: vec![],
                dep_info: vec![],
                out_dir: None,
                environment: BTreeMap::new(),
                fresh: false,
            });
            self.output.units.len() - 1
        });
        let unit = &mut self.output.units[index];
        if is_artifact {
            unit.kinds = kinds;
            unit.manifest_path = value["manifest_path"].as_str().unwrap_or("").into();
            unit.fresh = value["fresh"].as_bool().unwrap_or(false);
            let files = value["filenames"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if files.len() > 64
                || files
                    .iter()
                    .any(|v| v.as_str().is_none_or(|s| s.len() > 16384))
            {
                self.output
                    .error
                    .get_or_insert_with(|| "Cargo dependency artifact limit exceeded".into());
                return;
            }
            unit.artifacts = files
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect();
        } else {
            unit.kinds = vec!["custom-build".into()];
            if let Some(out) = value["out_dir"].as_str().filter(|s| s.len() <= 16384) {
                unit.out_dir = Some(out.into());
            }
            if let Some(env) = value["env"].as_array() {
                if env.len() > 128 {
                    self.output.error.get_or_insert_with(|| {
                        "Cargo dependency environment limit exceeded".into()
                    });
                    return;
                }
                for pair in env {
                    if let (Some(key), Some(val)) = (pair[0].as_str(), pair[1].as_str()) {
                        if key.len() + val.len() > 32768 {
                            self.output.error.get_or_insert_with(|| {
                                "Cargo dependency environment value limit exceeded".into()
                            });
                            return;
                        }
                        unit.environment
                            .insert(OsString::from(key), OsString::from(val));
                    }
                }
            }
        }
    }

    fn parse_line(&mut self) {
        // Cargo explicitly permits non-JSON stdout from compiler plugins.
        if self.line.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
            return;
        }
        let value: Value = match serde_json::from_slice(&self.line) {
            Ok(value) => value,
            Err(error) => {
                self.output
                    .error
                    .get_or_insert_with(|| format!("Invalid Cargo JSON message: {error}"));
                return;
            }
        };
        if value["reason"] == "build-finished" {
            let success = value["success"].as_bool();
            if self.output.finished.is_some() || success.is_none() {
                self.output.error.get_or_insert_with(|| {
                    "Invalid or duplicate Cargo build-finished message".into()
                });
            }
            self.output.finished = success;
            return;
        }
        if value["reason"] == "compiler-message" {
            if let Some(rendered) = value["message"]["rendered"].as_str() {
                let mut end = rendered
                    .len()
                    .min(DIAGNOSTIC_LIMIT.saturating_sub(self.output.diagnostics.len()));
                while !rendered.is_char_boundary(end) {
                    end -= 1;
                }
                self.output.diagnostics.push_str(&rendered[..end]);
            }
            return;
        }
        if self.collect_dependency_units {
            self.facts(&value);
        }
        if value["package_id"].as_str() != Some(&self.selector.package) {
            return;
        }
        match value["reason"].as_str() {
            Some("compiler-artifact") => {
                if value["manifest_path"]
                    .as_str()
                    .and_then(|path| Path::new(path).canonicalize().ok())
                    .as_ref()
                    != Some(&self.selector.manifest)
                {
                    return;
                }
                let kinds = value["target"]["kind"].as_array();
                let is_bin = kinds.is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"));
                let is_build =
                    kinds.is_some_and(|kinds| kinds.iter().any(|kind| kind == "custom-build"));
                if !(is_build
                    || is_bin
                        && value["target"]["name"] == self.selector.name
                        && value["profile"]["test"] == false)
                {
                    return;
                }
                if let Some(path) = value["executable"].as_str() {
                    self.product(PathBuf::from(path));
                    if is_bin
                        && self.output.executables.len() < PRODUCT_LIMIT
                        && path.len() <= 16384
                    {
                        self.output.executables.insert(path.into());
                    } else if is_bin {
                        self.output.error.get_or_insert_with(|| {
                            "Too many or oversized Cargo worker executable paths".into()
                        });
                    }
                }
                if let Some(files) = value["filenames"].as_array() {
                    for file in files.iter().filter_map(Value::as_str) {
                        self.product(file.into());
                    }
                }
            }
            Some("build-script-executed") => {
                if let Some(path) = value["out_dir"].as_str() {
                    self.product(path.into());
                }
                let env = value["env"].as_array();
                let get = |key: &str| {
                    env.and_then(|env| env.iter().find(|item| item[0].as_str() == Some(key)))
                        .and_then(|item| item[1].as_str())
                };
                if let (Some(host), Some(target)) = (get(HOST_KEY), get(TARGET_KEY)) {
                    if self.output.native.len() < PRODUCT_LIMIT
                        && host.len() <= 256
                        && target.len() <= 256
                    {
                        self.output.native.insert((host.into(), target.into()));
                    } else {
                        self.output
                            .error
                            .get_or_insert_with(|| "Too many Cargo native target proofs".into());
                    }
                }
            }
            _ => {}
        }
    }

    fn product(&mut self, path: PathBuf) {
        if self.output.products.len() < PRODUCT_LIMIT && path.as_os_str().len() <= 16384 {
            self.output.products.insert(path);
        } else {
            self.output
                .error
                .get_or_insert_with(|| "Too many root Cargo products".into());
        }
    }
}

fn dependency_info_paths(artifacts: &[PathBuf], kinds: &[String]) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for artifact in artifacts {
        let Some(parent) = artifact.parent() else {
            continue;
        };
        let Some(stem) = artifact.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let candidate = parent.join(format!("{stem}.d"));
        if candidate.is_file() {
            paths.insert(candidate);
        }
        if let Some(stem) = stem.strip_prefix("lib") {
            let candidate = parent.join(format!("{stem}.d"));
            if candidate.is_file() {
                paths.insert(candidate);
            }
        }
        if kinds.iter().any(|k| k == "custom-build")
            && let Ok(entries) = fs::read_dir(parent)
        {
            for entry in entries.take(64).flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "d") && path.is_file() {
                    paths.insert(path);
                }
            }
        }
    }
    paths.into_iter().collect()
}

fn setup(error: std::io::Error) -> ScriptError {
    ScriptError::Setup(error.to_string())
}

pub fn native_host(verbose: &str) -> Result<String, ScriptError> {
    let hosts: Vec<_> = verbose
        .lines()
        .filter_map(|line| line.strip_prefix("host: "))
        .collect();
    let compatible = |host: &str| {
        host.starts_with(&format!("{}-", std::env::consts::ARCH))
            && match std::env::consts::OS {
                "windows" => host.contains("-windows-"),
                "linux" => host.contains("-linux-"),
                _ => false,
            }
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if hosts.len() != 1 || !compatible(hosts[0]) {
        return Err(ScriptError::Setup(
            "Selected rustc -vV did not report one host compatible with this engine process".into(),
        ));
    }
    Ok(hosts[0].into())
}

pub(crate) fn build_script(host: &str) -> String {
    format!(
        r#"fn main() {{
    // This guard reads no package files beyond itself. Without an explicit
    // input Cargo treats diagnostic logs/ownership markers as build inputs.
    println!("cargo:rerun-if-changed=build.rs");
    let host = std::env::var("HOST").expect("Cargo HOST");
    let target = std::env::var("TARGET").expect("Cargo TARGET");
    assert!(host == {host:?} && target == host,
        "MODULE_ENGINE_NATIVE_TARGET_REQUIRED: expected HOST=TARGET={{}}, got HOST={{}} TARGET={{}}; remove foreign build.target/CARGO_BUILD_TARGET", {host:?}, host, target);
    println!("cargo:rustc-env={HOST_KEY}={{host}}");
    println!("cargo:rustc-env={TARGET_KEY}={{target}}");
}}
"#
    )
}

fn validate_native_executable(path: &Path) -> Result<(), ScriptError> {
    let mut file = fs::File::open(path).map_err(setup)?;
    let mut header = [0_u8; 64];
    file.read_exact(&mut header).map_err(setup)?;
    #[cfg(windows)]
    let valid = {
        let offset = u32::from_le_bytes(header[60..64].try_into().unwrap()) as u64;
        if &header[..2] != b"MZ" || !(64..=1024 * 1024).contains(&offset) {
            false
        } else {
            file.seek(SeekFrom::Start(offset)).map_err(setup)?;
            let mut pe = [0_u8; 26];
            file.read_exact(&mut pe).map_err(setup)?;
            pe_header_matches(&pe, std::env::consts::ARCH)
        }
    };
    #[cfg(not(windows))]
    let valid = elf_header_matches(&header, std::env::consts::ARCH);
    if !valid {
        return Err(ScriptError::Compile("Cargo product is not a native executable for this engine process; it will not be executed".into()));
    }
    Ok(())
}

#[cfg(any(not(windows), test))]
fn elf_header_matches(header: &[u8], arch: &str) -> bool {
    let machine = match arch {
        "x86_64" => 62,
        "aarch64" => 183,
        _ => return false,
    };
    header.len() >= 20
        && &header[..4] == b"\x7fELF"
        && header[4] == 2
        && header[5] == 1
        && matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        && u16::from_le_bytes([header[18], header[19]]) == machine
}

#[cfg(any(windows, test))]
fn pe_header_matches(header: &[u8], arch: &str) -> bool {
    let machine = match arch {
        "x86_64" => 0x8664,
        "aarch64" => 0xaa64,
        _ => return false,
    };
    header.len() >= 26
        && &header[..4] == b"PE\0\0"
        && u16::from_le_bytes([header[4], header[5]]) == machine
        && u16::from_le_bytes([header[24], header[25]]) == 0x20b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestArtifactGuard;
    use serde_json::json;

    fn fixture() -> (TestArtifactGuard, Selector) {
        let guard = TestArtifactGuard::new(&std::env::temp_dir(), "cargo-products").unwrap();
        let manifest = guard.path().join("Cargo.toml");
        fs::write(&manifest, "owned manifest").unwrap();
        let selector = Selector::from_metadata(
            &json!({
                "resolve":{"root":"worker-id"},
                "packages":[{"id":"worker-id", "name":"worker", "manifest_path":manifest}]
            })
            .to_string(),
            &manifest,
            "worker",
        )
        .unwrap();
        (guard, selector)
    }

    fn artifact(selector: &Selector, executable: &Path) -> Value {
        json!({"reason":"compiler-artifact", "package_id":selector.package,
            "manifest_path":selector.manifest, "target":{"name":selector.name, "kind":["bin"]},
            "profile":{"test":false}, "executable":executable, "filenames":[executable]})
    }

    fn feed(collector: &mut Collector, value: Value) {
        collector.feed(value.to_string().as_bytes());
        collector.feed(b"\n");
    }

    fn proof(collector: &mut Collector, host: &str, target: &str) {
        feed(
            collector,
            json!({"reason":"build-script-executed", "package_id":"worker-id",
            "env":[[HOST_KEY,host],[TARGET_KEY,target]],"out_dir":"unused"}),
        );
        feed(
            collector,
            json!({"reason":"build-finished", "success":true}),
        );
    }

    #[test]
    fn workspace_selector_uses_manifest_and_binary_without_virtual_workspace_root() {
        let (guard, _) = fixture();
        let manifest = guard.path().join("Cargo.toml");
        let package = json!({"id":"selected-player", "name":"module-engine-player", "manifest_path":manifest,
            "targets":[{"name":"module-engine-player","kind":["bin"]}]});
        let mut metadata = json!({"resolve":null,"packages":[package]});
        let select = |value: &Value| {
            Selector::workspace_binary(
                &value.to_string(),
                &manifest,
                "module-engine-player",
                "module-engine-player",
            )
        };
        assert_eq!(select(&metadata).unwrap().package, "selected-player");
        metadata["packages"][0]["targets"][0]["kind"] = json!(["lib"]);
        assert!(select(&metadata).is_err());
        metadata["packages"][0]["targets"][0]["kind"] = json!(["bin"]);
        let duplicate = metadata["packages"][0].clone();
        metadata["packages"].as_array_mut().unwrap().push(duplicate);
        assert!(select(&metadata).is_err());
    }

    #[test]
    fn executable_only_collector_ignores_large_dependency_closure_and_stale_default_path() {
        let (guard, selector) = fixture();
        let target = guard.path().join("target");
        let stale = target.join("release/old-default");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, b"stale default output must never be selected").unwrap();
        let actual = target.join("explicit-native/release/actual");
        fs::create_dir_all(actual.parent().unwrap()).unwrap();
        fs::copy(std::env::current_exe().unwrap(), &actual).unwrap();
        let mut collector = Collector::executable_only(selector.clone());
        for index in 0..400 {
            let mut dependency = artifact(&selector, &stale);
            dependency["package_id"] = format!("dependency-{index}").into();
            feed(&mut collector, dependency);
        }
        feed(&mut collector, artifact(&selector, &actual));
        proof(&mut collector, "native", "native");
        let output = collector.finish();
        assert!(output.units.is_empty());
        assert_eq!(
            output.executable("native", &target).unwrap(),
            actual.canonicalize().unwrap()
        );
        assert_eq!(
            fs::read(&stale).unwrap(),
            b"stale default output must never be selected"
        );
        fs::write(&actual, [0_u8; 64]).unwrap();
        assert!(
            output
                .executable("native", &target)
                .unwrap_err()
                .to_string()
                .contains("not a native executable")
        );
    }

    #[test]
    fn artifact_selection_uses_root_manifest_bin_and_one_distinct_executable() {
        let (guard, selector) = fixture();
        let path = guard.path().join("one");
        let own = artifact(&selector, &path);
        let mut collector = Collector::new(selector);
        let mut dependency = own.clone();
        dependency["package_id"] = "dependency-id".into();
        feed(&mut collector, dependency);
        let mut tests = own.clone();
        tests["profile"]["test"] = true.into();
        feed(&mut collector, tests);
        let mut other_manifest = own.clone();
        other_manifest["manifest_path"] = "missing".into();
        feed(&mut collector, other_manifest);
        let mut build = own.clone();
        build["target"]["kind"] = json!(["custom-build"]);
        build["executable"] = guard
            .path()
            .join("build-script")
            .to_string_lossy()
            .as_ref()
            .into();
        feed(&mut collector, build);
        feed(&mut collector, own.clone());
        feed(&mut collector, own);
        let result = collector.finish();
        assert_eq!(result.executables, BTreeSet::from([path]));
        assert!(result.products.contains(&guard.path().join("build-script")));
    }

    #[test]
    fn missing_ambiguous_and_foreign_proofs_fail_before_executable_execution() {
        let (guard, selector) = fixture();
        let mut missing = Collector::new(selector.clone());
        proof(&mut missing, "native", "native");
        assert!(
            missing
                .finish()
                .executable("native", guard.path())
                .unwrap_err()
                .to_string()
                .contains("0 distinct")
        );
        let mut ambiguous = Collector::new(selector.clone());
        feed(
            &mut ambiguous,
            artifact(&selector, &guard.path().join("one")),
        );
        feed(
            &mut ambiguous,
            artifact(&selector, &guard.path().join("two")),
        );
        proof(&mut ambiguous, "native", "native");
        assert!(
            ambiguous
                .finish()
                .executable("native", guard.path())
                .unwrap_err()
                .to_string()
                .contains("2 distinct")
        );
        for target in ["foreign", "native"] {
            let mut invalid = Collector::new(selector.clone());
            feed(
                &mut invalid,
                artifact(&selector, &guard.path().join("does-not-exist")),
            );
            if target == "foreign" {
                proof(&mut invalid, "native", target);
            } else {
                feed(
                    &mut invalid,
                    json!({"reason":"build-finished","success":true}),
                );
            }
            assert!(
                invalid
                    .finish()
                    .executable("native", guard.path())
                    .unwrap_err()
                    .to_string()
                    .contains("native target proof")
            );
        }
    }

    #[test]
    fn aggregate_stream_is_drained_beyond_log_budget_and_each_line_is_bounded() {
        let (guard, selector) = fixture();
        let mut collector = Collector::new(selector.clone());
        let line = json!({"reason":"compiler-artifact","package_id":"dependency", "padding":"x".repeat(4096)}).to_string() + "\n";
        for _ in 0..400 {
            collector.feed(line.as_bytes());
        }
        let own = artifact(&selector, &guard.path().join("late-worker"));
        feed(&mut collector, own.clone());
        proof(&mut collector, "native", "native");
        let result = collector.finish();
        assert!(result.error.is_none());
        assert_eq!(result.executables.len(), 1);
        let mut oversized = Collector::new(selector);
        oversized.feed(&vec![b'x'; LINE_LIMIT + 1]);
        oversized.feed(b"\n");
        feed(&mut oversized, own);
        let result = oversized.finish();
        assert!(result.error.unwrap().contains("exceeds"));
        assert_eq!(
            result.executables.len(),
            1,
            "must continue draining after an invalid line"
        );
    }

    #[test]
    fn malformed_duplicate_finish_and_unbounded_root_products_are_rejected() {
        let (guard, selector) = fixture();
        let mut collector = Collector::new(selector.clone());
        collector.feed(b"macro output is not JSON\n{invalid}\n");
        assert!(
            collector
                .finish()
                .error
                .unwrap()
                .contains("Invalid Cargo JSON")
        );
        let mut collector = Collector::new(selector.clone());
        proof(&mut collector, "native", "native");
        feed(
            &mut collector,
            json!({"reason":"build-finished","success":true}),
        );
        assert!(collector.finish().error.unwrap().contains("duplicate"));
        let mut collector = Collector::new(selector.clone());
        for index in 0..100 {
            feed(
                &mut collector,
                artifact(&selector, &guard.path().join(format!("worker{index}"))),
            );
        }
        let result = collector.finish();
        assert!(result.error.is_some());
        assert!(result.products.len() <= PRODUCT_LIMIT);
        assert!(result.executables.len() <= PRODUCT_LIMIT);
    }

    #[test]
    fn executable_path_must_stay_in_target_and_headers_must_match_native_machine() {
        let (guard, selector) = fixture();
        let outside = guard.path().join("outside");
        fs::write(&outside, b"not an executable").unwrap();
        let target = guard.path().join("target");
        fs::create_dir(&target).unwrap();
        let mut collector = Collector::new(selector.clone());
        feed(&mut collector, artifact(&selector, &outside));
        proof(&mut collector, "native", "native");
        assert!(
            collector
                .finish()
                .executable("native", &target)
                .unwrap_err()
                .to_string()
                .contains("escapes")
        );
        let mut elf = [0; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        elf[16] = 3;
        elf[18] = 62;
        assert!(elf_header_matches(&elf, "x86_64"));
        assert!(!elf_header_matches(&elf, "aarch64"));
        assert!(!elf_header_matches(&elf[..10], "x86_64"));
        elf[5] = 2;
        assert!(!elf_header_matches(&elf, "x86_64"));
        let mut pe = [0; 26];
        pe[..4].copy_from_slice(b"PE\0\0");
        pe[4..6].copy_from_slice(&0x8664_u16.to_le_bytes());
        pe[24..26].copy_from_slice(&0x20b_u16.to_le_bytes());
        assert!(pe_header_matches(&pe, "x86_64"));
        assert!(!pe_header_matches(&pe, "aarch64"));
        assert!(!pe_header_matches(&pe[..24], "x86_64"));
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_product_cannot_escape_selected_target() {
        let (guard, selector) = fixture();
        let target = guard.path().join("target");
        fs::create_dir(&target).unwrap();
        let outside = guard.path().join("outside");
        fs::write(&outside, "external file").unwrap();
        let link = target.join("worker");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let mut collector = Collector::new(selector.clone());
        feed(&mut collector, artifact(&selector, &link));
        proof(&mut collector, "native", "native");
        assert!(
            collector
                .finish()
                .executable("native", &target)
                .unwrap_err()
                .to_string()
                .contains("escapes")
        );
        assert_eq!(fs::read_to_string(outside).unwrap(), "external file");
        fs::remove_file(link).unwrap(); // retire only this test's own symlink
    }

    #[test]
    fn selected_host_requires_one_compatible_non_traversing_triple() {
        let host = format!(
            "{}-unknown-{}-abi",
            std::env::consts::ARCH,
            std::env::consts::OS
        );
        assert_eq!(
            native_host(&format!("rustc test\nhost: {host}\nrelease: test\n")).unwrap(),
            host
        );
        assert!(native_host("rustc test\n").is_err());
        assert!(native_host(&format!("host: {host}\nhost: {host}\n")).is_err());
        assert!(native_host("host: foreign-unknown-os\n").is_err());
        assert!(native_host(&format!("host: {host}/../../other\n")).is_err());
    }

    #[test]
    fn rendered_compiler_errors_remain_readable_and_bounded_at_utf8_boundaries() {
        let (_guard, selector) = fixture();
        let mut collector = Collector::new(selector);
        feed(
            &mut collector,
            json!({"reason":"compiler-message", "package_id":"dependency", "message":{"rendered":"error: unknown name\n --> user_program.rs:3:5\n"}}),
        );
        for _ in 0..100 {
            feed(
                &mut collector,
                json!({"reason":"compiler-message", "message":{"rendered":"я".repeat(4096)}}),
            );
        }
        let output = collector.finish();
        assert!(output.error.is_none());
        assert!(
            output
                .diagnostics
                .starts_with("error: unknown name\n --> user_program.rs:3:5\n")
        );
        assert!(!output.diagnostics.contains("\\n"));
        assert!(output.diagnostics.len() <= DIAGNOSTIC_LIMIT);
        assert!(output.diagnostics.len() >= DIAGNOSTIC_LIMIT - 2);
    }
}
