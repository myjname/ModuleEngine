//! Explicit delivery profiles and source capability checks, independent of tools.
use module_engine_core::{Graph, GraphDocument};
use module_engine_project::{
    collect_hlsl_source_dependencies, hash_bytes, hash_file, normalize_relative, resolve_relative,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub const MANIFEST_NAME: &str = ".module-engine-package.json";
pub const EDITOR_EXECUTABLE: &str = if cfg!(windows) {
    "ModuleEngine.exe"
} else {
    "ModuleEngine"
};
pub const PROJECT_BINDING_NAME: &str = ".module-engine-project.json";
pub fn project_binding_path(project_path: &Path) -> Result<PathBuf, String> {
    let name = project_path
        .file_name()
        .ok_or("Project binding needs a JSON filename")?;
    let mut sidecar = name.to_os_string();
    sidecar.push(PROJECT_BINDING_NAME);
    Ok(project_path.with_file_name(sidecar))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistributionProfile {
    Full,
    Team,
}
/// Project-local locator for the installed engine. Capabilities always come
/// from the package's checked manifest and source/SDK inventory, not this file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectEngineBinding {
    pub schema_version: u32,
    pub engine_version: String,
    pub package_root: PathBuf,
    pub profile: DistributionProfile,
    #[serde(default)]
    pub blueprint_hash: Option<String>,
    #[serde(default)]
    pub gameplay_source_ids: BTreeSet<String>,
}
impl ProjectEngineBinding {
    pub fn from_distribution(distribution: &Distribution) -> Result<Self, String> {
        let package_root = distribution
            .package_root
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let manifest = read_manifest(&package_root)?;
        check_manifest_version(&manifest)?;
        if manifest.profile == DistributionProfile::Team
            && distribution.profile != DistributionProfile::Team
        {
            return Err("Team package cannot produce a Full project binding".into());
        }
        Ok(Self {
            schema_version: 1,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            package_root,
            profile: distribution.profile,
            blueprint_hash: (distribution.profile == DistributionProfile::Team)
                .then(|| distribution.frozen_hash.clone()),
            gameplay_source_ids: distribution.gameplay_source_ids.clone(),
        })
    }
}
fn project_binding(project_path: &Path) -> Result<Option<ProjectEngineBinding>, String> {
    let path = project_binding_path(project_path)?;
    if !path.exists() {
        return Ok(None);
    }
    let binding: ProjectEngineBinding = serde_json::from_slice(
        &fs::read(&path).map_err(|e| format!("Project engine binding: {e}"))?,
    )
    .map_err(|e| format!("Project engine binding: {e}"))?;
    if binding.schema_version != 1 || binding.engine_version != env!("CARGO_PKG_VERSION") {
        return Err("Project engine binding version does not match executable".into());
    }
    #[cfg(target_os = "linux")]
    let binding = resolve_foreign_binding_root(binding, windows_binding_in_wsl)?;
    if !binding.package_root.is_absolute() {
        return Err("Project engine binding requires an absolute installed package path".into());
    }
    // A binding is never a substitute for an explicit package anchor.
    let manifest = read_manifest(&binding.package_root)
        .map_err(|e| format!("Bound engine package is unavailable: {e}"))?;
    check_manifest_version(&manifest)?;
    if manifest.profile == DistributionProfile::Team && binding.profile != DistributionProfile::Team
    {
        return Err("Project binding cannot promote a Team package to Full".into());
    }
    if binding.profile == DistributionProfile::Team
        && !binding
            .blueprint_hash
            .as_ref()
            .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("Team project binding lacks its frozen engine blueprint".into());
    }
    Ok(Some(binding))
}

/// Recognize only a drive-absolute Windows path. Canonical Windows bindings may
/// carry the verbatim drive prefix; UNC shares and device namespaces do not get
/// translated into a different engine installation.
#[cfg(any(target_os = "linux", test))]
fn windows_drive_absolute(path: &Path) -> Option<&str> {
    let text = path.to_str()?;
    let drive = text.strip_prefix(r"\\?\").unwrap_or(text);
    let bytes = drive.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
    .then_some(drive)
}

#[cfg(any(target_os = "linux", test))]
fn resolve_foreign_binding_root(
    mut binding: ProjectEngineBinding,
    convert: impl FnOnce(&str) -> Result<PathBuf, String>,
) -> Result<ProjectEngineBinding, String> {
    if let Some(windows) = windows_drive_absolute(&binding.package_root) {
        let converted = convert(windows)?;
        if !converted.is_absolute() {
            return Err("Windows project engine binding conversion returned a non-absolute Linux path; the binding was not ignored or rewritten".into());
        }
        binding.package_root = converted;
    }
    Ok(binding)
}

#[cfg(target_os = "linux")]
fn parse_wslpath_output(output: &[u8]) -> Result<PathBuf, String> {
    let text = std::str::from_utf8(output)
        .map_err(|error| format!("WSL path conversion returned invalid UTF-8: {error}"))?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    if text.is_empty() || text.contains(['\n', '\r', '\0']) || !Path::new(text).is_absolute() {
        return Err("WSL path conversion did not return one absolute Linux path".into());
    }
    Ok(PathBuf::from(text))
}

#[cfg(target_os = "linux")]
fn windows_binding_in_wsl(windows: &str) -> Result<PathBuf, String> {
    let fail = |detail: String| {
        format!(
            "Windows project engine binding '{windows}' cannot be resolved on this Linux host: {detail}. WSL must expose the same installed package through /usr/bin/wslpath; otherwise use an explicit native Linux package binding. The binding was not ignored or rewritten"
        )
    };
    let tool = Path::new("/usr/bin/wslpath");
    if !tool.is_file() {
        return Err(fail("/usr/bin/wslpath is unavailable".into()));
    }
    let output = std::process::Command::new(tool)
        .args(["-u", windows])
        .output()
        .map_err(|error| fail(format!("wslpath launch failed: {error}")))?;
    if !output.status.success() {
        return Err(fail(format!(
            "wslpath failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let path = parse_wslpath_output(&output.stdout).map_err(fail)?;
    path.canonicalize().map_err(|error| {
        fail(format!(
            "converted installed package '{}' is unavailable: {error}",
            path.display()
        ))
    })
}
#[derive(Clone, Debug, Default)]
pub struct SourceStatus {
    pub available: bool,
    pub root: Option<PathBuf>,
    pub modified: Vec<String>,
    pub diagnostics: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub schema_version: u32,
    pub engine_version: String,
    pub profile: DistributionProfile,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub module_sources: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub gameplay_source_ids: BTreeSet<String>,
    #[serde(default)]
    pub blueprint_hash: Option<String>,
    #[serde(default)]
    pub sdk_files: BTreeMap<String, String>,
    #[serde(default)]
    pub backend_files: BTreeMap<String, String>,
}
#[derive(Clone, Debug)]
pub struct Distribution {
    pub profile: DistributionProfile,
    pub package_root: PathBuf,
    pub source_status: SourceStatus,
    pub diagnostics: Vec<String>,
    manifest: Option<PackageManifest>,
    frozen_hash: String,
    gameplay_source_ids: BTreeSet<String>,
}
pub fn detect(
    project_path: &Path,
    document: &GraphDocument,
    requested: Option<DistributionProfile>,
) -> Result<Distribution, String> {
    let current = std::env::current_dir().map_err(|e| e.to_string())?;
    let absolute = if project_path.is_absolute() {
        project_path.to_path_buf()
    } else {
        current.join(project_path)
    };
    let project_root = absolute.parent().unwrap_or(&current).to_path_buf();
    let binding = project_binding(&absolute)?;
    let executable_root = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let mut candidates = vec![];
    if let Some(root) = &executable_root {
        candidates.push(root.clone());
        if let Some(parent) = root.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    candidates.push(project_root.clone());
    candidates.push(current);
    if let Some(binding) = &binding {
        candidates.push(binding.package_root.clone());
    }

    candidates.dedup();
    // Only explicit package anchors can grant source access. Build-time paths
    // are never an authorization fallback for a copied executable.
    let mut found = vec![];
    for root in candidates {
        if root.join(".sdk").is_dir() && !root.join(MANIFEST_NAME).is_file() {
            return Err(
                "SDK package marker is missing; original source fallback is disabled".into(),
            );
        }
        if root.join(MANIFEST_NAME).is_file() {
            found.push((root.clone(), read_manifest(&root)?));
        }
    }
    let chosen = found
        .iter()
        .find(|(_, m)| m.profile == DistributionProfile::Team)
        .or_else(|| {
            binding
                .as_ref()
                .and_then(|b| found.iter().find(|(p, _)| *p == b.package_root))
        })
        .or_else(|| found.iter().find(|(p, _)| *p == project_root))
        .or_else(|| found.first())
        .cloned();
    let (package_root, manifest) = chosen
        .map(|(r, m)| (r, Some(m)))
        .unwrap_or((project_root, None));
    if let Some(m) = &manifest {
        check_manifest_version(m)?;
        if m.profile == DistributionProfile::Team {
            verify_team_backend(&package_root, m, document)?;
        }
    }
    let profile = if manifest
        .as_ref()
        .is_some_and(|m| m.profile == DistributionProfile::Team)
    {
        if requested == Some(DistributionProfile::Full) {
            return Err("Team distribution cannot enable Engine with --profile full".into());
        }
        DistributionProfile::Team
    } else {
        if binding
            .as_ref()
            .is_some_and(|b| b.profile == DistributionProfile::Team)
        {
            if requested == Some(DistributionProfile::Full) {
                return Err("Team project binding cannot enable Engine with --profile full".into());
            }
            DistributionProfile::Team
        } else {
            requested.unwrap_or(DistributionProfile::Full)
        }
    };
    let mut source_status = SourceStatus::default();
    let mut diagnostics = vec![];
    if let Some(m) = &manifest {
        if m.profile == DistributionProfile::Full {
            source_status = inspect_full_sources(&package_root, m, document);
        }
    } else {
        source_status
            .diagnostics
            .push("Complete source manifest is missing; Engine authoring is unavailable".into());
    }
    let gameplay_source_ids = manifest
        .as_ref()
        .filter(|m| m.profile == DistributionProfile::Team)
        .map(|m| m.gameplay_source_ids.clone())
        .or_else(|| {
            binding
                .as_ref()
                .filter(|b| b.profile == DistributionProfile::Team)
                .map(|b| b.gameplay_source_ids.clone())
        })
        .unwrap_or_else(|| gameplay_sources(document));
    let frozen_hash = manifest
        .as_ref()
        .filter(|m| m.profile == DistributionProfile::Team)
        .and_then(|m| m.blueprint_hash.clone())
        .or_else(|| {
            binding
                .as_ref()
                .filter(|b| b.profile == DistributionProfile::Team)
                .and_then(|b| b.blueprint_hash.clone())
        })
        .unwrap_or(blueprint_hash(document, &gameplay_source_ids)?);
    if profile == DistributionProfile::Team {
        diagnostics.push("Team profile permanently disables Engine authoring".into());
    }
    let distribution = Distribution {
        profile,
        package_root,
        source_status,
        diagnostics,
        manifest,
        frozen_hash,
        gameplay_source_ids,
    };
    distribution.validate_document(document)?;
    Ok(distribution)
}
impl Distribution {
    pub fn can_author_engine(&self) -> bool {
        self.profile == DistributionProfile::Full && self.source_status.available
    }
    pub fn can_edit_custom_source(&self, id: &str, document: &GraphDocument) -> bool {
        let allowed = if self.profile == DistributionProfile::Team {
            self.gameplay_source_ids.clone()
        } else {
            gameplay_sources(document)
        };
        allowed.contains(id) && document.custom_sources.contains_key(id)
    }
    pub fn validate_document(&self, document: &GraphDocument) -> Result<(), String> {
        if self.profile == DistributionProfile::Team && !document.engine.initialized {
            return Err("Team requires initialized Engine policy".into());
        }
        if self.profile == DistributionProfile::Team
            && blueprint_hash(document, &self.gameplay_source_ids)? != self.frozen_hash
        {
            return Err("Team project changed its frozen engine blueprint, exposure configuration or Engine Rust source".into());
        }
        Ok(())
    }
    pub fn validate_project_document(
        &self,
        candidate: &GraphDocument,
        baseline: &GraphDocument,
    ) -> Result<(), String> {
        module_engine_core::validate_project_edit(candidate, baseline, &crate::host::registry()?)?;
        let allowed = if self.profile == DistributionProfile::Team {
            self.gameplay_source_ids.clone()
        } else {
            gameplay_sources(baseline)
        };
        for (id, text) in &baseline.custom_sources {
            if !allowed.contains(id) && candidate.custom_sources.get(id) != Some(text) {
                return Err(format!("Rust source '{id}' belongs to Engine"));
            }
        }
        self.validate_document(candidate)
    }
    pub fn compile_root(&self) -> Result<PathBuf, String> {
        if self.profile == DistributionProfile::Team {
            let m = self.manifest.as_ref().ok_or(
                "Team Custom SDK package metadata is missing; original source fallback is disabled",
            )?;
            if !m.sdk_files.contains_key(".sdk/Cargo.toml")
                || !m.sdk_files.contains_key(".sdk/Cargo.lock")
                || !m
                    .sdk_files
                    .contains_key(".sdk/crates/engine-core/src/lib.rs")
            {
                return Err("Team Custom SDK is incomplete".into());
            }
            verify_sdk(&self.package_root, m)?;
            Ok(self.package_root.join(".sdk"))
        } else if self.source_status.available {
            Ok(self.package_root.clone())
        } else {
            Err("Matching Custom SDK or complete source package is required; original workspace fallback is disabled".into())
        }
    }
    pub fn personal_layout_path(&self, workspace: &str) -> PathBuf {
        let workspace = if workspace == "engine" {
            "engine"
        } else {
            "project"
        };
        self.package_root
            .join(".user")
            .join(match self.profile {
                DistributionProfile::Full => "full",
                DistributionProfile::Team => "team",
            })
            .join(format!("{workspace}.json"))
    }
}
pub fn gameplay_sources(document: &GraphDocument) -> BTreeSet<String> {
    let mut active = BTreeSet::new();
    let mut pending = document
        .root
        .nodes
        .iter()
        .filter(|n| n.module == "viewport.output")
        .map(|n| n.id.clone())
        .collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if active.insert(id.clone()) {
            pending.extend(
                document
                    .root
                    .edges
                    .iter()
                    .filter(|e| e.to_node == id)
                    .map(|e| e.from_node.clone()),
            );
        }
    }
    let mut sources: BTreeSet<String> = document
        .root
        .nodes
        .iter()
        .filter(|n| n.module == "custom.rust.update" && active.contains(&n.id))
        .filter_map(|n| {
            n.params
                .get("source_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    if let Some(game) = &document.game {
        sources.extend(crate::game_host::script_ids(game));
    }
    sources
}
pub fn blueprint_hash(
    document: &GraphDocument,
    gameplay: &BTreeSet<String>,
) -> Result<String, String> {
    let mut value = serde_json::to_value(document).map_err(|e| e.to_string())?;
    fn scrub(graph: &mut serde_json::Value, gameplay: &BTreeSet<String>) {
        if let Some(nodes) = graph
            .get_mut("nodes")
            .and_then(serde_json::Value::as_array_mut)
        {
            for node in nodes {
                let game = node.get("module").and_then(serde_json::Value::as_str)
                    == Some("custom.rust.update")
                    && node
                        .pointer("/params/source_id")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|id| gameplay.contains(id));
                if let Some(object) = node.as_object_mut() {
                    object.remove("position");
                    object.remove("label");
                }
                if game
                    && let Some(params) = node
                        .get_mut("params")
                        .and_then(serde_json::Value::as_object_mut)
                {
                    params.remove("code_layout");
                    params.remove("code_layout_keys");
                }
                if let Some(inner) = node.get_mut("subgraph") {
                    scrub(inner, gameplay);
                }
            }
        }
    }
    scrub(&mut value["root"], gameplay);
    let fixed_sources = document
        .custom_sources
        .iter()
        .filter(|(id, _)| !gameplay.contains(*id))
        .collect::<BTreeMap<_, _>>();
    let projected = serde_json::json!({"schema_version":document.schema_version,"root":value["root"],"engine":value.get("engine"),"shader_sources":document.shader_sources,"module_versions":document.module_versions,"engine_rust_sources":fixed_sources,"custom_source_ids":document.custom_sources.keys().collect::<Vec<_>>(),"game_execution":document.game.as_ref().map(|g|&g.execution),"engine_profile":document.engine_profile,"engine_lock":document.engine_lock});
    Ok(hash_bytes(
        &serde_json::to_vec(&projected).map_err(|e| e.to_string())?,
    ))
}
pub fn verify_pins(root: &Path, files: &BTreeMap<String, String>) -> Result<(), String> {
    for (relative, expected) in files {
        if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("Invalid content pin: {relative}"));
        }
        let path = resolve_relative(root, relative)?;
        if !hash_file(&path)?.eq_ignore_ascii_case(expected) {
            return Err(format!("Package content differs: {relative}"));
        }
    }
    Ok(())
}
fn inspect_full_sources(
    root: &Path,
    manifest: &PackageManifest,
    document: &GraphDocument,
) -> SourceStatus {
    let mut status = SourceStatus {
        root: Some(root.into()),
        ..Default::default()
    };
    let mut blocking = vec![];
    for required in NATIVE_FILES {
        if !manifest.files.contains_key(*required) {
            blocking.push(format!("Source inventory omits {required}"));
        }
    }
    for (relative, expected) in &manifest.files {
        if !native_source_path(relative) {
            continue;
        }
        match resolve_relative(root, relative).and_then(|path| hash_file(&path)) {
            Ok(actual)
                if expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                if !actual.eq_ignore_ascii_case(expected) {
                    status.modified.push(relative.clone());
                }
            }
            Ok(_) => blocking.push(format!("Invalid source baseline hash {relative}")),
            Err(error) => blocking.push(error),
        }
    }
    fn visit(graph: &Graph, ids: &mut BTreeSet<String>) {
        for n in &graph.nodes {
            ids.insert(n.module.clone());
            if let Some(g) = &n.subgraph {
                visit(g, ids);
            }
        }
    }
    let mut ids = module_engine_core::effective_modules(document);
    visit(&document.root, &mut ids);
    for id in ids {
        if !manifest.module_sources.get(&id).is_some_and(|paths| {
            !paths.is_empty()
                && paths.iter().all(|p| {
                    manifest.files.contains_key(p)
                        && native_source_path(p)
                        && normalize_relative(p).is_ok()
                })
        }) {
            blocking.push(format!("Active module source inventory missing: {id}"));
        }
    }
    if !fs::read_to_string(root.join("Cargo.toml")).is_ok_and(|text| {
        crate::project::matching_workspace_version(&text, env!("CARGO_PKG_VERSION"))
    }) {
        blocking.push("Source workspace version does not match executable".into());
    }
    status.available = blocking.is_empty();
    status.diagnostics.extend(blocking);
    for relative in manifest.backend_files.keys() {
        if !root.join(relative).is_file() {
            status
                .diagnostics
                .push(format!("Optional package/tool file missing: {relative}"));
        }
    }
    status
}

/// Refresh an explicit baseline after source editing/export. This never enables
/// a partial SDK: the native required inventory is checked before writing.
pub fn write_full_manifest(
    root: &Path,
    document: &GraphDocument,
) -> Result<PackageManifest, String> {
    let mut files = BTreeMap::new();
    for name in ["Cargo.toml", "Cargo.lock", "engine-sdk.json"] {
        files.insert(name.into(), hash_file(&root.join(name))?);
    }
    for name in ["crates", "assets/gpu", "assets/engine-contracts"] {
        inventory_tree(root, &root.join(name), &mut files)?;
    }
    files.retain(|p, _| native_source_path(p));
    for source in document.shader_sources.values() {
        for path in collect_hlsl_source_dependencies(root, "__inline__.hlsl", source)? {
            files.insert(path.clone(), hash_file(&root.join(path))?);
        }
    }
    let mut backend_files = BTreeMap::new();
    for name in [
        "README.md",
        "LICENSE",
        "toolchain-manifest.json",
        "run.ps1",
        "run.sh",
        "verify.ps1",
        "verify.sh",
    ] {
        if root.join(name).is_file() {
            backend_files.insert(name.into(), hash_file(&root.join(name))?);
        }
    }
    for name in ["docs", "examples", "scripts", "tools/dxc"] {
        inventory_tree(root, &root.join(name), &mut backend_files)?;
    }
    let sources = files
        .keys()
        .filter(|p| p.starts_with("crates/") && (p.ends_with(".rs") || p.ends_with("Cargo.toml")))
        .cloned()
        .collect::<Vec<_>>();
    let mut ids = module_engine_core::effective_modules(document);
    for descriptor in crate::host::registry()?.descriptors() {
        ids.insert(descriptor.id.clone());
    }
    let module_sources = ids.into_iter().map(|id| (id, sources.clone())).collect();
    let mut sdk_files = BTreeMap::new();
    if root.join(".sdk").is_dir() {
        inventory_tree(root, &root.join(".sdk"), &mut sdk_files)?;
    }
    let manifest = PackageManifest {
        schema_version: 1,
        engine_version: env!("CARGO_PKG_VERSION").into(),
        profile: DistributionProfile::Full,
        files,
        module_sources,
        gameplay_source_ids: gameplay_sources(document),
        blueprint_hash: None,
        sdk_files,
        backend_files,
    };
    let status = inspect_full_sources(root, &manifest, document);
    if !status.available {
        return Err(format!(
            "Cannot seal incomplete source package: {}",
            status.diagnostics.join("; ")
        ));
    }
    save_manifest(root, &manifest)?;
    Ok(manifest)
}
pub fn write_team_manifest(
    root: &Path,
    document: &GraphDocument,
) -> Result<PackageManifest, String> {
    if !document.engine.initialized {
        return Err("Team package requires initialized Engine policy".into());
    }
    let mut sdk_files = BTreeMap::new();
    inventory_tree(root, &root.join(".sdk"), &mut sdk_files)?;
    let mut backend_files = BTreeMap::new();
    for name in ["assets/gpu", "tools/dxc", "players"] {
        inventory_tree(root, &root.join(name), &mut backend_files)?;
    }
    for source in document.shader_sources.values() {
        for path in collect_hlsl_source_dependencies(root, "__inline__.hlsl", source)? {
            backend_files.insert(path.clone(), hash_file(&root.join(path))?);
        }
    }
    for name in [EDITOR_EXECUTABLE, "toolchain-manifest.json"] {
        if !root.join(name).is_file() {
            return Err(format!("Team package missing {name}"));
        }
        backend_files.insert(name.into(), hash_file(&root.join(name))?);
    }
    let gameplay_source_ids = gameplay_sources(document);
    let manifest = PackageManifest {
        schema_version: 1,
        engine_version: env!("CARGO_PKG_VERSION").into(),
        profile: DistributionProfile::Team,
        files: BTreeMap::new(),
        module_sources: BTreeMap::new(),
        blueprint_hash: Some(blueprint_hash(document, &gameplay_source_ids)?),
        gameplay_source_ids,
        sdk_files,
        backend_files,
    };
    for required in [
        ".sdk/Cargo.toml",
        ".sdk/Cargo.lock",
        ".sdk/crates/engine-core/Cargo.toml",
        ".sdk/crates/engine-core/src/lib.rs",
    ] {
        if !manifest.sdk_files.contains_key(required) {
            return Err(format!("Team SDK missing {required}"));
        }
    }
    verify_sdk(root, &manifest)?;
    verify_team_backend(root, &manifest, document)?;
    save_manifest(root, &manifest)?;
    Ok(manifest)
}
pub fn read_manifest(root: &Path) -> Result<PackageManifest, String> {
    serde_json::from_slice(&fs::read(root.join(MANIFEST_NAME)).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
fn save_manifest(root: &Path, manifest: &PackageManifest) -> Result<(), String> {
    use std::io::Write;
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    );
    let path = root.join(MANIFEST_NAME);
    let temporary = root.join(format!("{MANIFEST_NAME}.tmp-{nonce}"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    if path.exists() {
        fs::copy(&path, root.join(format!("{MANIFEST_NAME}.backup-{nonce}")))
            .map_err(|e| e.to_string())?;
    }
    fs::rename(&temporary, &path)
        .map_err(|e| format!("{e}; metadata retained at {}", temporary.display()))
}
fn inventory_tree(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "Package inventory rejects symlinks: {}",
                path.display()
            ));
        }
        let name = entry.file_name();
        if name == ".work"
            || name == "target"
            || name == ".git"
            || name == ".user"
            || name == ".tools"
        {
            continue;
        }
        if kind.is_dir() {
            inventory_tree(root, &path, files)?;
        } else if kind.is_file() {
            let relative = normalize_relative(
                &path
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy(),
            )?;
            files.insert(relative, hash_file(&path)?);
        }
    }
    Ok(())
}

/// Environment helper for legacy callers. A Team marker/SDK at the executable
/// or requested package always wins; this never borrows native compiled sources.
pub fn package_compile_root() -> Result<PathBuf, String> {
    let mut candidates = vec![];
    if let Some(root) = std::env::var_os("MODULE_ENGINE_PACKAGE_ROOT") {
        candidates.push(PathBuf::from(root));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        candidates.push(parent.to_path_buf());
        if let Some(parent) = parent.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    for root in candidates {
        if root.join(".sdk").is_dir() && !root.join(MANIFEST_NAME).is_file() {
            return Err(
                "SDK package marker is missing; original source fallback is disabled".into(),
            );
        }
        if root.join(MANIFEST_NAME).is_file() {
            let m = read_manifest(&root)?;
            if m.profile == DistributionProfile::Team {
                check_manifest_version(&m)?;
                verify_sdk(&root, &m)?;
                return Ok(root.join(".sdk"));
            }
        }
    }
    Err("No Team SDK selected".into())
}

const NATIVE_FILES: &[&str] = &[
    "engine-sdk.json",
    "assets/engine-contracts/v0.4.0.json",
    "crates/editor/src/engine_profiles.rs",
    "crates/editor/src/engine_profile_verification.rs",
    "crates/runtime-script/src/cleanup.rs",
    "crates/runtime-script/src/cargo_build.rs",
    "crates/runtime-script/src/program_store.rs",
    "crates/runtime-script/src/cache_context.rs",
    "crates/runtime-script/src/sdk_cache.rs",
    "crates/runtime-script/src/native_tools.rs",
    "crates/runtime-script/src/freshness.rs",
    "crates/runtime-script/src/dep_info.rs",
    "crates/runtime-script/src/freshness/audited_dependencies.rs",
    "crates/runtime-script/src/jobs.rs",
    "crates/engine-core/src/engine_lock.rs",
    "crates/project-tools/src/engine_profile.rs",
    "crates/project-tools/src/asset_fields.rs",
    "crates/project-tools/src/engine_migration.rs",
    "crates/player/Cargo.toml",
    "crates/player/build.rs",
    "crates/player/src/composition.rs",
    "crates/player/src/lib.rs",
    "crates/player/src/main.rs",
    "crates/player/src/package.rs",
    "crates/player/src/presentations.rs",
    "crates/player/src/source_modules.rs",
    "crates/player/src/ui.rs",
    "crates/game-host/Cargo.toml",
    "crates/game-host/src/lib.rs",
    "crates/game-host/src/game_host.rs",
    "crates/game-host/src/game_worker.rs",
    "crates/game-host/src/activity.rs",
    "crates/game-host/src/compat_tests.rs",
    "crates/game-host/src/test_support.rs",
    "crates/game-host/src/neutral_tests.rs",
    "crates/host-services/Cargo.toml",
    "crates/host-services/src/lib.rs",
    "crates/host-services/src/audio.rs",
    "crates/render-vulkan/src/adapter.rs",
    "crates/editor/src/source_modules.rs",
    "crates/engine-core/src/component_schema.rs",
    "crates/engine-core/src/composition.rs",
    "crates/editor/src/workspace_files.rs",
    "crates/editor/src/native_dialogs.rs",
    "crates/editor/src/editor_workflow.rs",
    "crates/editor/src/assets_ui.rs",
    "crates/editor/src/assets_workspace_ui.rs",
    "crates/editor/src/scene_tools.rs",
    "crates/editor/src/scene_view_ui.rs",
    "crates/editor/src/scene_workspace.rs",
    "crates/editor/src/scene_assets_verification.rs",
    "crates/editor/src/window_navigation.rs",
    "crates/editor/src/ui_capture.rs",
    "crates/editor/src/game_host.rs",
    "crates/editor/src/game_worker.rs",
    "crates/editor/src/game_worker_verification.rs",
    "crates/editor/src/game_ui.rs",
    "crates/editor/src/game_package.rs",
    "crates/editor/src/runtime_delivery.rs",
    "crates/editor/src/game_verification.rs",
    "crates/editor/src/player.rs",
    "crates/engine-core/src/game.rs",
    "crates/engine-core/src/game_contributions.rs",
    "crates/editor/src/game_templates.rs",
    "crates/editor/src/puzzle_verification.rs",
    "crates/project-puzzle/Cargo.toml",
    "crates/project-puzzle/src/lib.rs",
    "crates/project-puzzle/src/components.rs",
    "crates/project-puzzle/src/systems.rs",
    "crates/project-puzzle/src/project.rs",
    "crates/project-puzzle/src/tests.rs",
    "crates/project-puzzle/src/ui.rs",
    "crates/example-two-arenas/Cargo.toml",
    "crates/example-two-arenas/src/lib.rs",
    "crates/example-two-arenas/src/content.rs",
    "crates/gameplay-arpg/Cargo.toml",
    "crates/gameplay-arpg/src/lib.rs",
    "crates/gameplay-arpg/src/components.rs",
    "crates/gameplay-arpg/src/reusable_contract_tests.rs",
    "crates/gameplay-arpg/src/events.rs",
    "crates/gameplay-arpg/src/presentation.rs",
    "crates/gameplay-arpg/src/project.rs",
    "crates/gameplay-arpg/src/queries.rs",
    "crates/gameplay-arpg/src/state.rs",
    "crates/gameplay-arpg/src/systems.rs",
    "crates/gameplay-arpg/src/verification.rs",
    "crates/gameplay-arpg/src/regressions.rs",
    "crates/gameplay-arpg/src/ui/mod.rs",
    "crates/gameplay-arpg/src/ui/combat_visuals.rs",
    "crates/engine-core/src/scenes.rs",
    "crates/engine-core/src/systems.rs",
    "crates/engine-content/Cargo.toml",
    "crates/engine-content/src/lib.rs",
    "crates/runtime-script/src/persistent.rs",
    "Cargo.toml",
    "Cargo.lock",
    "crates/editor/Cargo.toml",
    "crates/editor/src/main.rs",
    "crates/editor/src/app.rs",
    "crates/editor/src/host.rs",
    "crates/editor/src/project.rs",
    "crates/editor/src/distribution.rs",
    "crates/editor/src/authoring.rs",
    "crates/editor/src/activity.rs",
    "crates/editor/src/editor_benchmark.rs",
    "crates/editor/src/canvas.rs",
    "crates/editor/src/dock.rs",
    "crates/editor/src/panes.rs",
    "crates/editor/src/properties.rs",
    "crates/editor/src/layout.rs",
    "crates/editor/src/panels.rs",
    "crates/editor/src/scalar.rs",
    "crates/editor/src/verification.rs",
    "crates/editor/src/settings_verification.rs",
    "crates/engine-core/Cargo.toml",
    "crates/engine-core/src/lib.rs",
    "crates/engine-core/src/world.rs",
    "crates/engine-core/src/runtime.rs",
    "crates/engine-core/src/settings.rs",
    "crates/engine-core/src/backend.rs",
    "crates/engine-core/src/control.rs",
    "crates/engine-core/src/graph.rs",
    "crates/engine-core/src/registry.rs",
    "crates/engine-core/src/render.rs",
    "crates/render-vulkan/Cargo.toml",
    "crates/render-vulkan/src/lib.rs",
    "crates/render-vulkan/src/vulkan.rs",
    "crates/render-vulkan/src/compiler.rs",
    "crates/render-vulkan/src/geometry.rs",
    "crates/render-vulkan/src/graph.rs",
    "crates/custom-rust/Cargo.toml",
    "crates/custom-rust/src/lib.rs",
    "crates/custom-rust/src/command.rs",
    "crates/runtime-script/Cargo.toml",
    "crates/runtime-script/src/lib.rs",
    "crates/project-tools/Cargo.toml",
    "crates/project-tools/src/lib.rs",
    "crates/project-tools/src/assets.rs",
    "crates/project-tools/src/asset_workspace.rs",
    "crates/project-tools/src/history.rs",
    "crates/project-tools/src/merge.rs",
    "assets/gpu/forward.hlsl",
    "assets/gpu/fullscreen.hlsl",
    "assets/gpu/depth_normal.hlsl",
    "assets/gpu/gbuffer.hlsl",
    "assets/gpu/deferred.hlsl",
    "assets/gpu/ssao.hlsl",
    "assets/gpu/tonemap.hlsl",
    "assets/gpu/fxaa.hlsl",
    "assets/gpu/color_encode.hlsl",
    "assets/gpu/custom_compute.hlsl",
    "assets/gpu/custom_raster.hlsl",
    "assets/gpu/custom_fullscreen.hlsl",
];
fn native_source_path(path: &str) -> bool {
    matches!(
        path,
        "Cargo.toml" | "Cargo.lock" | "engine-sdk.json" | "assets/engine-contracts/v0.4.0.json"
    ) || (path.starts_with("crates/") && (path.ends_with(".rs") || path.ends_with("Cargo.toml")))
        || path.ends_with(".hlsl")
        || path.ends_with(".hlsli")
}
fn check_manifest_version(m: &PackageManifest) -> Result<(), String> {
    if m.schema_version != 1 || m.engine_version != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "Package version '{}' does not match executable {}",
            m.engine_version,
            env!("CARGO_PKG_VERSION")
        ));
    }
    Ok(())
}
fn verify_sdk(root: &Path, m: &PackageManifest) -> Result<(), String> {
    for file in NATIVE_FILES
        .iter()
        .filter(|p| p.starts_with("crates/engine-core/"))
    {
        if !m.sdk_files.contains_key(&format!(".sdk/{file}")) {
            return Err(format!("Team SDK inventory missing {file}"));
        }
    }
    for file in [".sdk/Cargo.toml", ".sdk/Cargo.lock", ".sdk/engine-sdk.json"] {
        if !m.sdk_files.contains_key(file) {
            return Err(format!("Team SDK inventory missing {file}"));
        }
    }
    verify_pins(root, &m.sdk_files)?;
    if !fs::read_to_string(root.join(".sdk/Cargo.toml")).is_ok_and(|text| {
        crate::project::matching_workspace_version(&text, env!("CARGO_PKG_VERSION"))
    }) {
        return Err("Team SDK version does not match executable".into());
    }
    Ok(())
}
fn team_backend_requirements(document: &GraphDocument) -> Vec<&'static str> {
    let mut files = vec![EDITOR_EXECUTABLE, "toolchain-manifest.json"];
    if module_engine_core::connected_modules(document).contains("render.vulkan") {
        if cfg!(windows) {
            files.extend([
                "tools/dxc/dxc.exe",
                "tools/dxc/dxcompiler.dll",
                "tools/dxc/dxil.dll",
                "tools/dxc/LICENSE-MS.txt",
                "tools/dxc/LICENSE-LLVM.txt",
            ]);
        } else if cfg!(target_os = "linux") {
            files.extend(["tools/dxc/bin/dxc", "tools/dxc/lib/libdxcompiler.so"]);
        }
    }
    files
}

fn verify_team_backend(
    root: &Path,
    m: &PackageManifest,
    document: &GraphDocument,
) -> Result<(), String> {
    for file in team_backend_requirements(document) {
        if !m.backend_files.contains_key(file) {
            return Err(format!("Team backend inventory missing {file}"));
        }
    }
    for file in NATIVE_FILES.iter().filter(|p| p.starts_with("assets/gpu/")) {
        if !m.backend_files.contains_key(*file) {
            return Err(format!("Team backend inventory missing {file}"));
        }
    }
    if m.blueprint_hash.is_none() {
        return Err("Team engine blueprint reference is missing".into());
    }
    verify_pins(root, &m.backend_files)
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    use module_engine_core::WorldKind;
    use std::sync::atomic::{AtomicU64, Ordering};
    fn fixture() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let p = std::env::temp_dir().join(format!(
            "ModuleEngine delivery {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn cleanup(p: &Path) {
        let p = p.canonicalize().unwrap();
        let t = std::env::temp_dir().canonicalize().unwrap();
        assert!(p.starts_with(&t) && p != t);
        fs::remove_dir_all(p).unwrap();
    }
    fn copy_native_fixture(root: &Path) {
        let original = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        for relative in NATIVE_FILES {
            let p = root.join(relative);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::copy(original.join(relative), p).unwrap();
        }
    }
    #[test]
    fn windows_binding_recognition_accepts_only_absolute_drive_paths() {
        for (path, expected) in [
            (r"E:\ModuleEngine", r"E:\ModuleEngine"),
            ("e:/Module Engine", "e:/Module Engine"),
            (r"\\?\E:\ModuleEngine", r"E:\ModuleEngine"),
        ] {
            assert_eq!(windows_drive_absolute(Path::new(path)), Some(expected));
        }
        for path in [
            "E:relative",
            "/mnt/e/ModuleEngine",
            r"\\server\share",
            r"\\?\UNC\server\share",
            r"\\.\PhysicalDrive0",
            r"\\?\E:relative",
            "ab:/ModuleEngine",
            "1:/ModuleEngine",
            "é:/ModuleEngine",
        ] {
            assert_eq!(windows_drive_absolute(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn foreign_binding_resolution_changes_only_the_in_memory_locator() {
        let binding = ProjectEngineBinding {
            schema_version: 1,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            package_root: PathBuf::from(r"\\?\E:\ModuleEngine"),
            profile: DistributionProfile::Team,
            blueprint_hash: Some("a".repeat(64)),
            gameplay_source_ids: BTreeSet::from(["game-controller".into(), "user-update".into()]),
        };
        let original = serde_json::to_vec(&binding).unwrap();
        let expected = std::env::temp_dir().join("same installed engine");
        let resolved = resolve_foreign_binding_root(binding.clone(), |windows| {
            assert_eq!(windows, r"E:\ModuleEngine");
            Ok(expected.clone())
        })
        .unwrap();
        assert_eq!(resolved.package_root, expected);
        let mut before = serde_json::to_value(&binding).unwrap();
        let mut after = serde_json::to_value(&resolved).unwrap();
        before.as_object_mut().unwrap().remove("package_root");
        after.as_object_mut().unwrap().remove("package_root");
        assert_eq!(
            before, after,
            "profile, SDK/version and frozen policy must not change"
        );
        assert_eq!(serde_json::to_vec(&binding).unwrap(), original);
        assert!(
            resolve_foreign_binding_root(binding.clone(), |_| Ok(PathBuf::from("relative")))
                .unwrap_err()
                .contains("non-absolute")
        );
        assert_eq!(
            resolve_foreign_binding_root(binding.clone(), |_| Err("converter unavailable".into()))
                .unwrap_err(),
            "converter unavailable"
        );
        assert_eq!(serde_json::to_vec(&binding).unwrap(), original);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wslpath_output_requires_one_absolute_utf8_path() {
        assert_eq!(
            parse_wslpath_output(b"/mnt/e/Module Engine\n").unwrap(),
            PathBuf::from("/mnt/e/Module Engine")
        );
        assert_eq!(
            parse_wslpath_output(b"/mnt/e/trailing space \r\n").unwrap(),
            PathBuf::from("/mnt/e/trailing space ")
        );
        for invalid in [
            b"".as_slice(),
            b"relative\n".as_slice(),
            b"E:\\ModuleEngine\n".as_slice(),
            b"/one\n/two\n".as_slice(),
            b"/one\n\n".as_slice(),
            b"/one\0\n".as_slice(),
            b"/one\xff\n".as_slice(),
        ] {
            assert!(parse_wslpath_output(invalid).is_err(), "{invalid:?}");
        }
    }
    #[test]
    fn team_backend_uses_native_tools_only_for_a_connected_vulkan_renderer() {
        let mut doc = crate::host::settings_demo_document(WorldKind::Ecs);
        let required = team_backend_requirements(&doc);
        assert!(required.contains(&EDITOR_EXECUTABLE));
        #[cfg(windows)]
        assert!(required.contains(&"tools/dxc/dxc.exe"));
        #[cfg(target_os = "linux")]
        {
            assert!(required.contains(&"tools/dxc/bin/dxc"));
            assert!(required.contains(&"tools/dxc/lib/libdxcompiler.so"));
            assert!(
                !required
                    .iter()
                    .any(|file| file.ends_with(".exe") || file.ends_with(".dll"))
            );
        }
        // A neutral/headless Team does not acquire an unused compiler service.
        doc.root.edges.clear();
        let required = team_backend_requirements(&doc);
        assert_eq!(required, vec![EDITOR_EXECUTABLE, "toolchain-manifest.json"]);
    }
    #[test]
    fn full_sources_are_complete_versioned_and_editable_independent_of_tools() {
        let root = fixture();
        copy_native_fixture(&root);
        let doc = crate::host::settings_demo_document(WorldKind::Objects);
        let m = write_full_manifest(&root, &doc).unwrap();
        assert!(inspect_full_sources(&root, &m, &doc).available);
        // Actual Rust source remains editable after sealing.
        let p = root.join("crates/editor/src/host.rs");
        let original = fs::read(&p).unwrap();
        let mut changed = original.clone();
        changed.extend_from_slice(b"\n// legitimate edit\n");
        fs::write(&p, &changed).unwrap();
        let status = inspect_full_sources(&root, &m, &doc);
        assert!(status.available);
        assert!(
            status
                .modified
                .iter()
                .any(|x| x == "crates/editor/src/host.rs")
        );
        assert!(!root.join("tools/dxc/dxc.exe").exists());
        assert!(
            status.available,
            "Missing compiler must not disable source authoring"
        );
        fs::remove_file(&p).unwrap();
        assert!(!inspect_full_sources(&root, &m, &doc).available);
        fs::write(&p, &original).unwrap();
        for required in [
            "crates/editor/src/source_modules.rs",
            "crates/engine-core/src/component_schema.rs",
            "crates/engine-core/src/composition.rs",
        ] {
            let mut incomplete = m.clone();
            incomplete.files.remove(required);
            assert!(
                !inspect_full_sources(&root, &incomplete, &doc).available,
                "required source contract omitted: {required}"
            );
        }
        let mut missing_module = m.clone();
        missing_module.module_sources.remove("game.motion");
        assert!(!inspect_full_sources(&root, &missing_module, &doc).available);
        let cargo = root.join("Cargo.toml");
        let original = fs::read_to_string(&cargo).unwrap();
        fs::write(
            cargo,
            original.replace(
                &format!("version = \"{}\"", env!("CARGO_PKG_VERSION")),
                "version = \"9.9.9\"",
            ),
        )
        .unwrap();
        assert!(!inspect_full_sources(&root, &m, &doc).available);
        cleanup(&root);
    }
    #[test]
    fn team_blueprint_allows_gameplay_and_cosmetics_but_locks_architecture() {
        let doc = crate::host::settings_demo_document(WorldKind::Objects);
        let game = gameplay_sources(&doc);
        assert!(game.contains("controller"));
        assert!(!game.contains("speed"));
        let dist = Distribution {
            profile: DistributionProfile::Team,
            package_root: fixture(),
            source_status: SourceStatus::default(),
            diagnostics: vec![],
            manifest: None,
            frozen_hash: blueprint_hash(&doc, &game).unwrap(),
            gameplay_source_ids: game,
        };
        // No marker must produce a diagnostic rather than panic/native fallback.
        assert!(dist.compile_root().unwrap_err().contains("metadata"));
        let mut edit = doc.clone();
        edit.custom_sources
            .get_mut("controller")
            .unwrap()
            .push_str("\n// gameplay edit");
        edit.root.nodes[0].position = [999., 999.];
        dist.validate_project_document(&edit, &doc).unwrap();
        edit.custom_sources
            .get_mut("speed")
            .unwrap()
            .push_str("\n// engine edit");
        assert!(dist.validate_document(&edit).is_err());
        let mut edit = doc.clone();
        edit.root.nodes[0].module = "world.ecs".into();
        assert!(dist.validate_document(&edit).is_err());
        let mut edit = doc.clone();
        edit.engine.initialized = false;
        assert!(dist.validate_document(&edit).is_err());
        let mut edit = doc.clone();
        module_engine_core::detach_component(
            &mut edit,
            &crate::host::registry().unwrap(),
            1,
            "game.motion",
        )
        .unwrap();
        dist.validate_document(&edit).unwrap();
        assert_eq!(
            module_engine_core::prepare_team_document(&edit)
                .properties
                .inactive,
            Default::default()
        );
        cleanup(&dist.package_root);
    }
    #[test]
    fn damaged_sdk_marker_cannot_use_original_full_sources() {
        let root = fixture();
        fs::create_dir_all(root.join(".sdk")).unwrap();
        let doc = crate::host::settings_demo_document(WorldKind::Objects);
        let result = detect(&root.join("project.json"), &doc, None);
        assert!(result.unwrap_err().contains("marker"));
        cleanup(&root);
    }
    #[test]
    fn external_project_binding_uses_checked_package_and_never_grants_missing_sources() {
        let package = fixture();
        let project = fixture();
        copy_native_fixture(&package);
        let doc = crate::host::settings_demo_document(WorldKind::Objects);
        write_full_manifest(&package, &doc).unwrap();
        let distribution = detect(&package.join("project.json"), &doc, None).unwrap();
        let path = project.join("project.json");
        let binding = ProjectEngineBinding::from_distribution(&distribution).unwrap();
        fs::write(
            project_binding_path(&path).unwrap(),
            serde_json::to_vec_pretty(&binding).unwrap(),
        )
        .unwrap();
        let bound = detect(&path, &doc, None).unwrap();
        assert_eq!(
            bound.package_root.canonicalize().unwrap(),
            package.canonicalize().unwrap()
        );
        assert!(bound.can_author_engine());
        fs::remove_file(package.join("crates/editor/src/host.rs")).unwrap();
        let damaged = detect(&path, &doc, None).unwrap();
        assert!(!damaged.can_author_engine());
        assert!(
            damaged.compile_root().is_err(),
            "Binding cannot borrow the current directory's Full source"
        );
        fs::remove_file(package.join(MANIFEST_NAME)).unwrap();
        assert!(
            detect(&path, &doc, None)
                .unwrap_err()
                .contains("unavailable")
        );
        // Sidecar applies to its own JSON, without changing an original sibling.
        assert!(detect(&project.join("original.json"), &doc, None).is_ok());
        cleanup(&project);
        cleanup(&package);
    }
    #[test]
    fn bound_team_package_keeps_manifest_blueprint_even_if_sidecar_is_edited() {
        let package = fixture();
        let project = fixture();
        let doc = crate::host::settings_demo_document(WorldKind::Objects);
        let gameplay = gameplay_sources(&doc);
        let mut backend_files = BTreeMap::new();
        let mandatory = team_backend_requirements(&doc);
        for relative in mandatory.into_iter().chain(
            NATIVE_FILES
                .iter()
                .copied()
                .filter(|p| p.starts_with("assets/gpu/")),
        ) {
            let path = package.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"verified backend fixture").unwrap();
            backend_files.insert(relative.into(), hash_file(&path).unwrap());
        }
        save_manifest(
            &package,
            &PackageManifest {
                schema_version: 1,
                engine_version: env!("CARGO_PKG_VERSION").into(),
                profile: DistributionProfile::Team,
                files: BTreeMap::new(),
                module_sources: BTreeMap::new(),
                gameplay_source_ids: gameplay.clone(),
                blueprint_hash: Some(blueprint_hash(&doc, &gameplay).unwrap()),
                sdk_files: BTreeMap::new(),
                backend_files,
            },
        )
        .unwrap();
        let dist = detect(&package.join("project.json"), &doc, None).unwrap();
        let path = project.join("project.json");
        let sidecar = project_binding_path(&path).unwrap();
        let mut binding = ProjectEngineBinding::from_distribution(&dist).unwrap();
        fs::write(&sidecar, serde_json::to_vec(&binding).unwrap()).unwrap();
        assert_eq!(
            detect(&path, &doc, None).unwrap().profile,
            DistributionProfile::Team
        );
        assert!(detect(&path, &doc, Some(DistributionProfile::Full)).is_err());
        let mut illegal = doc.clone();
        illegal.root.nodes[0].module = "world.ecs".into();
        binding.blueprint_hash = Some(blueprint_hash(&illegal, &gameplay).unwrap());
        fs::write(&sidecar, serde_json::to_vec(&binding).unwrap()).unwrap();
        assert!(
            detect(&path, &illegal, None)
                .unwrap_err()
                .contains("frozen")
        );
        binding.profile = DistributionProfile::Full;
        fs::write(&sidecar, serde_json::to_vec(&binding).unwrap()).unwrap();
        assert!(detect(&path, &doc, None).unwrap_err().contains("promote"));
        cleanup(&project);
        cleanup(&package);
    }
}
