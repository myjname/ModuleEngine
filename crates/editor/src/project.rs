//! Project persistence, migration and complete editable source export.
#[cfg(test)]
use module_engine_core::{Graph, ModuleRegistry};
use module_engine_core::{GraphDocument, Runtime, Severity, validate_graph};
use module_engine_project::{
    AssetManifest, collect_hlsl_source_dependencies, export_assets, hash_file, resolve_relative,
    save_document_with_backup,
};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub fn read(path: &Path) -> Result<GraphDocument, String> {
    let document = module_engine_project::read_document_strict(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let package = crate::distribution::detect(path, &document, None)?;
    crate::engine_profiles::validate(&document, &package.package_root)?;
    Ok(document)
}
/// Fill only missing implementation pins. A conflicting declared pin is retained.
#[cfg(test)]
pub fn stamp_module_versions(document: &mut GraphDocument, registry: &ModuleRegistry) {
    fn visit(
        graph: &Graph,
        registry: &ModuleRegistry,
        versions: &mut std::collections::BTreeMap<String, String>,
    ) {
        for node in &graph.nodes {
            if let Some(version) = registry.module_version(&node.module) {
                versions
                    .entry(node.module.clone())
                    .or_insert_with(|| version.into());
            }
            if let Some(inner) = &node.subgraph {
                visit(inner, registry, versions);
            }
        }
    }
    visit(&document.root, registry, &mut document.module_versions);
    if let Some(game) = &document.game {
        for node in game.execution.stages.iter().flat_map(|stage| &stage.nodes) {
            if let Some(version) = registry.module_version(&node.module) {
                document
                    .module_versions
                    .entry(node.module.clone())
                    .or_insert_with(|| version.into());
            }
        }
    }
    for id in module_engine_core::effective_modules(document) {
        if let Some(version) = registry.module_version(&id) {
            document
                .module_versions
                .entry(id)
                .or_insert_with(|| version.into());
        }
    }
}

/// Pin active schema owners independently of their scheduled consumers.
pub fn stamp_composition_versions(
    document: &mut GraphDocument,
    composition: &module_engine_core::CompositionContext,
) -> Result<(), String> {
    // Explicit approval helper for constructors and diagnostic fixtures only.
    // Keep conflicting declarations so rejection tests cannot repair them.
    let before = document.module_versions.clone();
    let profile = module_engine_project::EngineProfile::from_document(document)?;
    composition
        .capture_lock(
            document,
            profile.identity,
            Some(module_engine_core::SdkIdentity::current()),
        )?
        .apply_to(document);
    for (id, version) in before {
        if composition
            .module_version(&id)
            .is_some_and(|installed| installed != version)
        {
            document.module_versions.insert(id, version);
        }
    }
    Ok(())
}

/// Save a complete document and retain exact old bytes in a unique backup.
pub fn save(path: &Path, document: &GraphDocument) -> Result<(), String> {
    let distribution = crate::distribution::detect(path, document, None)?;
    distribution.validate_document(document)?;
    crate::host::validate_game_at(document, &distribution.package_root)?;
    crate::engine_profiles::validate(document, &distribution.package_root)?;
    save_document_with_backup(path, document).map(|_| ())
}

/// An exported source workspace in the current directory takes precedence over
/// the build-time workspace, allowing a copied executable to locate its sources.
pub fn source_root() -> PathBuf {
    if let Ok(root) = crate::distribution::package_compile_root() {
        return root;
    }
    // A damaged SDK still cannot borrow the native build-time workspace.
    let mut anchors = vec![];
    if let Some(root) = std::env::var_os("MODULE_ENGINE_PACKAGE_ROOT") {
        anchors.push(PathBuf::from(root));
    }
    if let Ok(cwd) = std::env::current_dir() {
        anchors.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(p) = exe.parent()
    {
        anchors.push(p.into());
        if let Some(parent) = p.parent() {
            anchors.push(parent.into());
        }
    }
    if let Some(root) = anchors.into_iter().find(|p| p.join(".sdk").is_dir()) {
        return root.join(".sdk");
    }
    if let Ok(current) = std::env::current_dir()
        && fs::read_to_string(current.join("Cargo.toml"))
            .is_ok_and(|text| matching_workspace_version(&text, env!("CARGO_PKG_VERSION")))
        && current.join("crates/engine-core/Cargo.toml").is_file()
    {
        return current;
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Recognize only an explicit version in the workspace.package table. Unknown
/// TOML syntax falls back to the compiled workspace instead of guessing.
pub(crate) fn matching_workspace_version(manifest: &str, expected: &str) -> bool {
    let mut in_package = false;
    let mut matched = None;
    for line in manifest.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.starts_with('[') {
            in_package = line.split('#').next().unwrap_or_default().trim() == "[workspace.package]";
            continue;
        }
        if !in_package || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "version"
        {
            if matched.is_some() {
                return false;
            }
            let value = value.trim();
            let correct = ['"', '\''].iter().any(|quote| {
                value
                    .strip_prefix(&format!("{quote}{expected}{quote}"))
                    .is_some_and(|tail| {
                        let tail = tail.trim();
                        tail.is_empty() || tail.starts_with('#')
                    })
            });
            matched = Some(correct);
        }
    }
    matched == Some(true)
}
#[cfg(test)]
pub fn export(destination: &Path, document: &GraphDocument) -> Result<PathBuf, String> {
    export_with_asset_root(destination, document, &source_root())
}

/// Registered assets are relative to asset_root (normally the opened JSON's
/// directory). Inline HLSL includes follow the current compiler's workspace
/// context; source_id is an opaque key, not a source filename.
#[cfg(test)]
pub fn export_with_asset_root(
    destination: &Path,
    document: &GraphDocument,
    asset_root: &Path,
) -> Result<PathBuf, String> {
    export_with_package_root(destination, document, asset_root, &source_root())
}
pub fn export_with_package_root(
    destination: &Path,
    document: &GraphDocument,
    asset_root: &Path,
    package_root: &Path,
) -> Result<PathBuf, String> {
    let composition = crate::host::composition_at(package_root)?;
    let registry = composition.modules.clone();
    let mut packaged = document.clone();
    crate::engine_profiles::validate(&packaged, package_root)?;
    crate::host::validate_document_at(&packaged, package_root)?;
    let diagnostics = validate_graph(&packaged, &registry);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(format!(
            "Сначала исправьте ошибки графа: {}",
            diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    Runtime::from_document(&packaged, &registry).map_err(|e| e.to_string())?;

    let root = package_root.canonicalize().map_err(|e| e.to_string())?;
    let capability = crate::distribution::detect(&root.join("project.json"), &packaged, None)?;
    if !capability.can_author_engine() {
        return Err(
            "Complete matching Full source package is required for native source export".into(),
        );
    }
    let resource = package_root.to_path_buf();
    let mut assets =
        AssetManifest::from_value(&packaged.asset_manifest)?.with_hlsl_dependencies(asset_root)?;
    add_game_assets_at(&packaged, asset_root, package_root, &mut assets)?;
    assets.verify(asset_root)?;
    let mut shader_dependencies = AssetManifest::default();
    for source in packaged.shader_sources.values() {
        for relative in collect_hlsl_source_dependencies(&resource, "__inline__.hlsl", source)? {
            if !shader_dependencies
                .assets
                .values()
                .any(|a| a.path == relative)
            {
                shader_dependencies.register(
                    format!("shader-include-{}", shader_dependencies.assets.len()),
                    &relative,
                    &resource,
                )?;
            }
        }
    }
    let complete_assets = complete_manifest(&assets, &shader_dependencies)?;
    if !complete_assets.assets.is_empty() || !packaged.asset_manifest.is_null() {
        packaged.asset_manifest =
            serde_json::to_value(&complete_assets).map_err(|e| e.to_string())?;
    }
    let target = prospective_path(destination)?;
    validate_destination(&root, &target)?;
    validate_destination(
        &resource.canonicalize().map_err(|e| e.to_string())?,
        &target,
    )?;
    if target.exists() {
        if !target.is_dir()
            || fs::read_dir(&target)
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
        {
            return Err(
                "Папка экспорта уже содержит файлы. Укажите новую или пустую папку.".into(),
            );
        }
    } else {
        fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    }
    let target = target.canonicalize().map_err(|e| e.to_string())?;
    validate_destination(&root, &target)?;
    validate_destination(
        &resource.canonicalize().map_err(|e| e.to_string())?,
        &target,
    )?;

    for relative in [
        "Cargo.toml",
        "Cargo.lock",
        ".gitignore",
        ".gitattributes",
        "README.md",
        "README.ru.md",
        "AGENTS.md",
        ".agents/skills/moduleengine-debug/SKILL.md",
        ".codex/config.toml",
        "LICENSE",
        "run.ps1",
        "run.sh",
        "verify.ps1",
        "verify.sh",
        "toolchain-manifest.json",
        "engine-sdk.json",
    ] {
        let source = root.join(relative);
        if source.is_file() {
            let destination = target.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            copy_new(&source, &destination)?;
        } else if matches!(
            relative,
            "Cargo.toml" | "Cargo.lock" | "toolchain-manifest.json"
        ) {
            return Err(format!("Missing source export requirement: {relative}"));
        }
    }
    for relative in ["crates", "docs", "examples", "assets", "scripts"] {
        copy_tree(&root.join(relative), &target.join(relative))?;
    }
    copy_tree(&root.join("tools/dxc"), &target.join("tools/dxc"))?;
    export_assets(asset_root, &assets, &target)?;
    export_assets(&resource, &shader_dependencies, &target)?;
    // Check that registered content survived source-tree collision checks and
    // that every included file has the exact bytes selected by the compiler.
    for entry in assets.assets.values() {
        if hash_file(&resolve_relative(&target, &entry.path)?)? != entry.sha256.to_ascii_lowercase()
        {
            return Err(format!("Exported asset hash differs: {}", entry.path));
        }
    }
    save_document_with_backup(&target.join("project.json"), &packaged)?;
    crate::distribution::write_full_manifest(&target, &packaged)?;
    Ok(target)
}

fn complete_manifest(
    assets: &AssetManifest,
    shaders: &AssetManifest,
) -> Result<AssetManifest, String> {
    let mut complete = assets.clone();
    for (id, entry) in &shaders.assets {
        if let Some(existing) = complete.assets.values().find(|asset| {
            module_engine_project::normalize_relative(&asset.path).ok()
                == module_engine_project::normalize_relative(&entry.path).ok()
        }) {
            if !existing.sha256.eq_ignore_ascii_case(&entry.sha256) {
                return Err(format!("Conflicting asset/include bytes at {}", entry.path));
            }
            continue;
        }
        let mut key = id.clone();
        let mut suffix = 1;
        while complete.assets.contains_key(&key) {
            key = format!("{id}:{suffix}");
            suffix += 1;
        }
        complete.assets.insert(key, entry.clone());
    }
    for entry in complete.assets.values() {
        let relative = module_engine_project::normalize_relative(&entry.path)?;
        if relative.eq_ignore_ascii_case("project.json")
            || relative.eq_ignore_ascii_case(crate::distribution::MANIFEST_NAME)
            || relative.eq_ignore_ascii_case("ModuleEngine.exe")
            || relative.eq_ignore_ascii_case("ModuleEngine")
            || matches!(
                relative.split('/').next(),
                Some(".work" | "target" | ".git" | ".sdk" | ".user" | ".tools")
            )
        {
            return Err(format!(
                "Reserved project/cache paths cannot be exported as assets: {relative}"
            ));
        }
    }
    Ok(complete)
}
fn validate_destination(root: &Path, target: &Path) -> Result<(), String> {
    if target == root {
        return Err("Папка экспорта должна отличаться от исходного проекта.".into());
    }
    for copied in [
        "crates", "docs", "examples", "assets", "scripts", "tools", ".sdk", ".user", ".tools",
    ] {
        let tree = root.join(copied);
        let tree = if tree.exists() {
            tree.canonicalize().map_err(|e| e.to_string())?
        } else {
            tree
        };
        if target.starts_with(&tree) {
            return Err(format!(
                "Папка экспорта должна находиться вне {copied}, чтобы исключить копирование проекта в самого себя."
            ));
        }
    }
    Ok(())
}

/// Resolve existing ancestors before creating anything. Use the returned path
/// for creation too, so lexical traversal and symlinks cannot change the target.
fn prospective_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("Export path escapes filesystem root".into());
                }
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut missing = vec![];
    let mut ancestor = normalized.as_path();
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .ok_or("Missing export path ancestor")?
                .to_os_string(),
        );
        ancestor = ancestor.parent().ok_or("Missing export path ancestor")?;
    }
    let mut resolved = ancestor.canonicalize().map_err(|e| e.to_string())?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn copy_new(source: &Path, target: &Path) -> Result<(), String> {
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| format!("{}: {e}", target.display()))?;
    std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Preserve launchable tools/scripts without propagating setuid/setgid
        // bits into a relocated source or Team package.
        let mode = input
            .metadata()
            .map_err(|error| error.to_string())?
            .permissions()
            .mode();
        fs::set_permissions(target, fs::Permissions::from_mode(mode & 0o777))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err(format!(
                "Source export does not follow symlinks: {}",
                entry.path().display()
            ));
        }
        let name = entry.file_name();
        if name == ".work" || name == "target" {
            continue;
        }
        let destination = target.join(name);
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if kind.is_file() {
            copy_new(&entry.path(), &destination)?;
        }
    }
    Ok(())
}

/// Deployable Project-only bundle with frozen blueprint, gameplay and Core SDK.
pub fn export_team_with_package_root(
    destination: &Path,
    document: &GraphDocument,
    asset_root: &Path,
    ready_executable: &Path,
    package_root: &Path,
) -> Result<PathBuf, String> {
    let capability =
        crate::distribution::detect(&package_root.join("project.json"), document, None)?;
    if !capability.can_author_engine() {
        return Err("Team export requires complete matching Full source package".into());
    }
    let composition = crate::host::composition_at(package_root)?;
    let registry = composition.modules.clone();
    let mut normalized = document.clone();
    crate::host::ensure_authoring_configuration(&mut normalized);
    let mut packaged = crate::runtime_delivery::prepare(
        &module_engine_core::prepare_team_document(&normalized),
        package_root,
    )?;
    // Pins also archive inactive contracts. Removing them would change the
    // integrity lock without an explicit profile publication.
    let diagnostics = validate_graph(&packaged, &registry);
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join("; "));
    }
    Runtime::from_document(&packaged, &registry).map_err(|e| e.to_string())?;
    if !ready_executable.is_file() {
        return Err(format!(
            "Ready executable missing: {}",
            ready_executable.display()
        ));
    }
    let native_players = crate::game_package::team_player_variants(&packaged, package_root)?;
    let source = package_root.to_path_buf();
    let resource = package_root.to_path_buf();
    // Team authors can still place declared assets not used by the current
    // scene. Unlike the standalone Player, retain their declared asset library;
    // both deliveries use the same working graph/source closure.
    let mut assets =
        AssetManifest::from_value(&packaged.asset_manifest)?.with_hlsl_dependencies(asset_root)?;
    add_game_assets_at(&packaged, asset_root, package_root, &mut assets)?;
    let mut headers = AssetManifest::default();
    for path in crate::runtime_delivery::shader_dependencies(&packaged, &resource)? {
        if !headers.assets.values().any(|a| a.path == path) {
            headers.register(
                format!("shader-include-{}", headers.assets.len()),
                &path,
                &resource,
            )?;
        }
    }
    let complete = complete_manifest(&assets, &headers)?;
    if !complete.assets.is_empty() || !packaged.asset_manifest.is_null() {
        packaged.asset_manifest = serde_json::to_value(complete).map_err(|e| e.to_string())?;
    }
    let target = prospective_path(destination)?;
    validate_destination(&source.canonicalize().map_err(|e| e.to_string())?, &target)?;
    validate_destination(
        &resource.canonicalize().map_err(|e| e.to_string())?,
        &target,
    )?;
    if target.exists()
        && (!target.is_dir()
            || fs::read_dir(&target)
                .map_err(|e| e.to_string())?
                .next()
                .is_some())
    {
        return Err("Team export requires a new or empty directory".into());
    }
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    copy_new(
        ready_executable,
        &target.join(crate::distribution::EDITOR_EXECUTABLE),
    )?;
    for (native_key, native_player) in native_players {
        let native_target = target
            .join("players")
            .join(native_key)
            .join(module_engine_player::package::PLAYER_EXECUTABLE);
        fs::create_dir_all(native_target.parent().unwrap()).map_err(|e| e.to_string())?;
        copy_new(&native_player, &native_target)?;
    }
    for relative in ["toolchain-manifest.json", "LICENSE"] {
        copy_new(&source.join(relative), &target.join(relative))?;
    }
    copy_tree(&source.join("scripts"), &target.join("scripts"))?;
    copy_tree(&resource.join("assets"), &target.join("assets"))?;
    if module_engine_core::connected_modules(&packaged).contains("render.vulkan") {
        copy_tree(&resource.join("tools/dxc"), &target.join("tools/dxc"))?;
    }
    export_assets(asset_root, &assets, &target)?;
    export_assets(&resource, &headers, &target)?;
    create_custom_sdk(&source, &target)?;
    #[cfg(windows)]
    fs::write(target.join("run.ps1"),
        "param([string]$Project='project.json')\n$ErrorActionPreference='Stop'\nPush-Location $PSScriptRoot\ntry { & (Join-Path $PSScriptRoot 'ModuleEngine.exe') $Project --profile team; if ($LASTEXITCODE -ne 0) { throw 'ModuleEngine failed' } } finally { Pop-Location }\n").map_err(|e|e.to_string())?;
    #[cfg(windows)]
    fs::write(target.join("README.md"),
        "# ModuleEngine Team 0.4\n\nRun .\\ModuleEngine.exe from this directory, or use .\\run.ps1.\nEngine architecture and gameplay system order are frozen by .module-engine-package.json. Edit Project settings, authored scenes/templates, participating component values and scheduled gameplay Update Rust sources.\nRust Custom requires the pinned Rust/Cargo toolchain plus host MSVC/Windows SDK. Doctor: .\\scripts\\bootstrap.ps1 -Json. Explicit restoration: .\\scripts\\bootstrap.ps1 -Restore; tools/cache remain package-local. .sdk contains the pinned Core compilation SDK, not the native editor authoring workspace. Team never enables Engine from another source tree. This profile is an authoring policy, not a security sandbox.\n").map_err(|e|e.to_string())?;
    #[cfg(unix)]
    write_unix_team_launcher(&target)?;
    save_document_with_backup(&target.join("project.json"), &packaged)?;
    crate::distribution::write_team_manifest(&target, &packaged)?;
    Ok(target)
}

#[cfg(unix)]
fn write_unix_team_launcher(target: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::write(target.join("run.sh"),
        "#!/bin/sh\nset -eu\npackage_root=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\ncd \"$package_root\"\nexec \"$package_root/ModuleEngine\" \"${1:-project.json}\" --profile team\n")
        .map_err(|error| error.to_string())?;
    fs::set_permissions(target.join("run.sh"), fs::Permissions::from_mode(0o755))
        .map_err(|error| error.to_string())?;
    fs::write(target.join("README.md"),
        "# ModuleEngine Team 0.4\n\nRun `./run.sh` or `./ModuleEngine project.json --profile team`. Native binaries are specific to the OS and architecture that built this package. A working Vulkan driver and Wayland or X11 session are required for the Linux GUI.\n\nThe Engine architecture and gameplay system order are frozen by `.module-engine-package.json`. Project settings, authored scenes/templates, participating component values and scheduled gameplay Rust sources remain editable. Rust Custom requires the pinned native Rust/Cargo toolchain and native C/C++ linker. GPU shader editing requires the packaged Linux DXC (`tools/dxc/bin/dxc`, `tools/dxc/lib/libdxcompiler.so` and its system dependencies). No Windows SDK, PowerShell or Windows DXC is used by the Linux application. The `.sdk` directory contains the pinned Core compilation SDK; Team never enables Engine from another source tree. This profile is an authoring policy, not a security sandbox.\n")
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
fn add_game_assets(
    document: &GraphDocument,
    root: &Path,
    manifest: &mut AssetManifest,
) -> Result<(), String> {
    add_game_assets_at(document, root, &source_root(), manifest)
}

fn add_game_assets_at(
    document: &GraphDocument,
    root: &Path,
    backend_root: &Path,
    manifest: &mut AssetManifest,
) -> Result<(), String> {
    let Some(game) = &document.game else {
        return Ok(());
    };
    let mut library = module_engine_content::ContentLibrary::new(root)?;
    library.set_asset_paths(manifest.asset_paths());
    let mut models = vec![];
    let mut paths = std::collections::BTreeSet::new();
    for reference in crate::host::component_asset_references(document, backend_root)? {
        paths.insert(manifest.resolve_reference(&reference)?);
    }
    for entity in crate::host::active_game_entities_at(document, backend_root)? {
        if let Some(path) = entity
            .components
            .get("mesh")
            .and_then(|m| m.get("path"))
            .and_then(serde_json::Value::as_str)
        {
            models.push(path.to_owned());
        }
        if let Some(path) = entity
            .components
            .get("material")
            .and_then(|m| m.get("base_color_texture"))
            .and_then(serde_json::Value::as_str)
        {
            library.load_texture(path)?;
            paths.insert(manifest.resolve_reference(path)?);
        }
    }
    paths.extend(library.dependencies(models)?);
    if crate::host::composition_at(backend_root)?
        .activity(document)?
        .modules
        .contains("game.audio")
    {
        for path in game.audio_cues.values() {
            paths.insert(manifest.resolve_reference(path)?);
        }
    }
    for path in paths {
        if !manifest.assets.values().any(|a| a.path == path) {
            let mut index = manifest.assets.len();
            let mut id = format!("game-resource-{index}");
            while manifest.assets.contains_key(&id) {
                index += 1;
                id = format!("game-resource-{index}");
            }
            manifest.register(id, &path, root)?;
        }
    }
    Ok(())
}

fn create_custom_sdk(source: &Path, target: &Path) -> Result<(), String> {
    let sdk = target.join(".sdk");
    fs::create_dir_all(&sdk).map_err(|e| e.to_string())?;
    copy_new(
        &source.join("engine-sdk.json"),
        &sdk.join("engine-sdk.json"),
    )?;
    fs::create_dir_all(sdk.join("crates/engine-core")).map_err(|e| e.to_string())?;
    copy_tree(
        &source.join("crates/engine-core/src"),
        &sdk.join("crates/engine-core/src"),
    )?;
    let root_manifest = fs::read_to_string(source.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let package = root_manifest
        .find("[workspace.package]")
        .ok_or("SDK source workspace metadata missing")?;
    let sdk_manifest = format!(
        "[workspace]\nmembers=[\"crates/engine-core\"]\nresolver=\"3\"\n\n{}",
        &root_manifest[package..]
    );
    fs::write(sdk.join("Cargo.toml"), sdk_manifest).map_err(|e| e.to_string())?;
    let original_core = fs::read_to_string(source.join("crates/engine-core/Cargo.toml"))
        .map_err(|e| e.to_string())?;
    let mut core = String::new();
    let mut skip_example = false;
    for line in original_core.lines() {
        if line.trim().starts_with('[') {
            skip_example = line.trim() == "[[example]]";
        }
        if !skip_example {
            core.push_str(line);
            core.push('\n');
        }
    }
    fs::write(sdk.join("crates/engine-core/Cargo.toml"), core).map_err(|e| e.to_string())?;
    let origin_lock = fs::read_to_string(source.join("Cargo.lock")).map_err(|e| e.to_string())?;
    fs::write(sdk.join("Cargo.lock"), &origin_lock).map_err(|e| e.to_string())?;
    let compiler =
        module_engine_script::ScriptCompiler::new(source.into(), target.join(".work/sdk-export"));
    let mut command = std::process::Command::new(
        compiler
            .cargo_binary
            .as_deref()
            .unwrap_or_else(|| Path::new("cargo")),
    );
    command
        .args([
            "metadata",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(sdk.join("Cargo.toml"));
    if let Some(p) = compiler.rustc_binary {
        command.env("RUSTC", p);
    }
    if let Some(p) = compiler.cargo_home {
        command.env("CARGO_HOME", p);
    }
    if let Some(p) = compiler.rustup_home {
        command.env("RUSTUP_HOME", p);
    }
    if let Some(p) = compiler.rustup_toolchain {
        command.env("RUSTUP_TOOLCHAIN", p);
    }
    if let Some(p) = compiler.target_dir {
        command.env("CARGO_TARGET_DIR", p);
    }
    let output = command
        .output()
        .map_err(|e| format!("SDK lock normalization: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "SDK lock normalization failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    // Cargo may prune the workspace lock, but it may not select new package
    // versions/sources merely because they happen to exist in a local cache.
    fn packages(lock: &str) -> std::collections::BTreeSet<String> {
        lock.split("[[package]]")
            .skip(1)
            .map(|block| {
                block
                    .lines()
                    .filter(|line| {
                        let line = line.trim();
                        line.starts_with("name =")
                            || line.starts_with("version =")
                            || line.starts_with("source =")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect()
    }
    let sdk_lock = fs::read_to_string(sdk.join("Cargo.lock")).map_err(|e| e.to_string())?;
    if !packages(&sdk_lock).is_subset(&packages(&origin_lock)) {
        return Err("SDK dependencies differ from the authoritative source Cargo.lock".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{WorldKind, demo_document};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ModuleEngine project {name} {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn cleanup(path: &Path) {
        let temp = std::env::temp_dir().canonicalize().unwrap();
        let target = path.canonicalize().unwrap();
        assert!(target.starts_with(&temp) && target != temp);
        fs::remove_dir_all(target).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn source_and_team_copy_preserves_unix_executable_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = fixture("native executable modes");
        let source = root.join("native tool");
        let copied = root.join("copied tool");
        fs::write(&source, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o4755)).unwrap();
        copy_new(&source, &copied).unwrap();
        assert_eq!(
            fs::metadata(&copied).unwrap().permissions().mode() & 0o7777,
            0o755
        );
        assert!(
            copy_new(&source, &copied).is_err(),
            "never replace an existing target"
        );
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn unix_team_launcher_quotes_paths_and_uses_native_executable() {
        use std::os::unix::fs::PermissionsExt;
        let root = fixture("Team launcher with spaces");
        write_unix_team_launcher(&root).unwrap();
        let engine = root.join(crate::distribution::EDITOR_EXECUTABLE);
        fs::write(&engine, "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\"\n").unwrap();
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
        let unrelated = fixture("unrelated working folder");
        let output = std::process::Command::new(root.join("run.sh"))
            .arg("project with spaces.json")
            .current_dir(&unrelated)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            vec![
                root.to_str().unwrap(),
                "project with spaces.json",
                "--profile",
                "team"
            ]
        );
        assert_eq!(
            fs::metadata(root.join("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert!(!root.join("ModuleEngine.exe").exists());
        cleanup(&unrelated);
        cleanup(&root);
    }
    #[test]
    fn workspace_version_check_is_conservative() {
        let matching = "[workspace]\nmembers=[]\n[workspace.package] # pinned\nversion = \"0.2.0\" # release\nedition=\"2024\"\n";
        assert!(matching_workspace_version(matching, "0.2.0"));
        assert!(matching_workspace_version(
            "[workspace.package]\nversion = '0.2.0'\n",
            "0.2.0"
        ));
        assert!(!matching_workspace_version(matching, "0.1.0"));
        assert!(!matching_workspace_version(
            "[package]\nversion = \"0.2.0\"\n",
            "0.2.0"
        ));
        assert!(!matching_workspace_version(
            "[workspace.package]\nversion={ workspace=true }\n",
            "0.2.0"
        ));
        assert!(!matching_workspace_version(
            "[workspace.package]\nversion=\"0.2.0\"\nversion=\"0.1.0\"\n",
            "0.2.0"
        ));
        assert!(!matching_workspace_version(
            "[workspace.package]\nversion=\"0.2.0\" trailing\n",
            "0.2.0"
        ));
    }
    #[test]
    fn save_preserves_old_schema_bytes_sources_and_pins() {
        let temp = fixture("migration");
        let file = temp.join("project.json");
        let mut old = demo_document(WorldKind::Objects);
        old.schema_version = 1;
        let bytes = serde_json::to_vec_pretty(&old).unwrap();
        fs::write(&file, &bytes).unwrap();
        assert!(read(&file).unwrap_err().contains("migration"));
        assert_eq!(fs::read(&file).unwrap(), bytes);
        let mut current = crate::host::settings_demo_document(WorldKind::Objects);
        current
            .custom_sources
            .get_mut("speed")
            .unwrap()
            .push_str("// Preserved source comment\n");
        save(&file, &current).unwrap();
        assert_eq!(read(&file).unwrap(), current);
        let accepted = fs::read(&file).unwrap();
        current
            .module_versions
            .insert("world.objects".into(), "99.0-incompatible".into());
        assert!(save(&file, &current).is_err());
        stamp_module_versions(&mut current, &crate::host::registry().unwrap());
        assert_eq!(
            current.module_versions["world.objects"],
            "99.0-incompatible"
        );
        assert_eq!(fs::read(&file).unwrap(), accepted);
        let backups: Vec<_> = fs::read_dir(&temp)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with("backup-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(&backups[0]).unwrap(), bytes);
        assert!(!file.with_extension("json.tmp").exists());
        cleanup(&temp);
    }
    #[test]
    fn full_and_team_game_asset_collectors_keep_stable_ids_after_file_move() {
        let temp = fixture("stable references");
        fs::create_dir_all(temp.join("assets/models")).unwrap();
        fs::write(
            temp.join("assets/models/source.glb"),
            include_bytes!("../../../assets/models/demo_crystal.glb"),
        )
        .unwrap();
        let mut doc = crate::game_host::demo_document();
        let game = doc.game.as_mut().unwrap();
        game.audio_cues.clear();
        for scene in game.scenes.values_mut() {
            for object in &mut scene.objects {
                object.components.remove("mesh");
            }
        }
        let mut manifest = AssetManifest::default();
        manifest
            .register("stable-model", "assets/models/source.glb", &temp)
            .unwrap();
        doc.asset_manifest = serde_json::to_value(&manifest).unwrap();
        doc.game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .insert(
                "mesh".into(),
                serde_json::json!({"path":"asset://stable-model"}),
            );
        let service =
            module_engine_project::AssetWorkspace::new(&temp, Vec::<String>::new()).unwrap();
        let moved = service
            .move_asset(&doc, "stable-model", "assets/models/renamed.glb")
            .unwrap();
        for prepared in [
            moved.document.clone(),
            module_engine_core::prepare_team_document(&moved.document),
        ] {
            let mut exported = AssetManifest::from_value(&prepared.asset_manifest).unwrap();
            add_game_assets(&prepared, &temp, &mut exported).unwrap();
            exported.verify(&temp).unwrap();
            assert_eq!(
                exported.assets["stable-model"].path,
                "assets/models/renamed.glb"
            );
            assert!(
                exported
                    .assets
                    .values()
                    .all(|a| !a.path.starts_with("asset://"))
            );
            assert_eq!(
                prepared.game.as_ref().unwrap().scenes["arena"].objects[0].components["mesh"]["path"],
                "asset://stable-model"
            );
        }
        let mut bad = moved.document;
        bad.game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .get_mut("mesh")
            .unwrap()["path"] = serde_json::json!("asset://missing");
        let mut index = AssetManifest::from_value(&bad.asset_manifest).unwrap();
        assert!(
            add_game_assets(&bad, &temp, &mut index)
                .unwrap_err()
                .contains("Unknown asset ID")
        );
        cleanup(&temp);
    }
    #[test]
    fn composition_pins_schema_owner_independently_of_its_consumer() {
        use module_engine_core::*;
        struct Consumer;
        impl ExecutableSystem for Consumer {
            fn step(
                &mut self,
                _: &mut SystemContext<'_>,
                _: &mut serde_json::Value,
            ) -> Result<(), String> {
                Ok(())
            }
        }
        let mut composition = crate::host::composition_at(&source_root()).unwrap();
        composition
            .systems
            .install_module("fixture.owner", "0.7.0")
            .unwrap();
        composition
            .systems
            .components_mut()
            .register(ComponentSchema {
                id: "fixture.data".into(),
                owner_module: "fixture.owner".into(),
                version: "data/2".into(),
                label: "Data".into(),
                defaults: serde_json::json!({}),
                fields: vec![],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        composition
            .systems
            .register(
                SystemDescriptor {
                    id: "fixture.consumer".into(),
                    state_schema: "consumer/1".into(),
                    components: vec!["fixture.data".into()],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(Consumer)),
            )
            .unwrap();
        let composition =
            CompositionContext::new(composition.modules, composition.systems).unwrap();
        let mut document = crate::source_modules::tests::document();
        document.game.as_mut().unwrap().execution = ExecutionGraph {
            resources: vec![],
            stages: vec![SystemStage {
                name: "Update".into(),
                nodes: vec![SystemNode {
                    id: "consumer".into(),
                    module: "fixture.consumer".into(),
                    params: serde_json::json!({}),
                    bindings: Default::default(),
                    after: vec![],
                }],
            }],
        };
        stamp_composition_versions(&mut document, &composition).unwrap();
        assert_eq!(document.module_versions["fixture.owner"], "0.7.0");
        assert_eq!(
            document.module_versions["fixture.consumer"],
            env!("CARGO_PKG_VERSION")
        );
        document
            .module_versions
            .insert("fixture.owner".into(), "0.6.0".into());
        stamp_composition_versions(&mut document, &composition).unwrap();
        assert_eq!(
            document.module_versions["fixture.owner"], "0.6.0",
            "conflicting pin must not be silently replaced"
        );
        assert!(
            composition
                .validate_lock(&document, Some(&module_engine_core::SdkIdentity::current()))
                .unwrap_err()
                .contains("fixture.owner")
        );
    }

    #[test]
    fn export_includes_new_packages_tools_and_relative_external_assets() {
        let temp = fixture("export");
        let asset_root = temp.join("external files");
        fs::create_dir_all(asset_root.join("resources")).unwrap();
        fs::write(
            asset_root.join("resources/filter.hlsl"),
            "#include \"shared.hlsli\"\n",
        )
        .unwrap();
        fs::write(
            asset_root.join("resources/shared.hlsli"),
            "// include preserved\n",
        )
        .unwrap();
        fs::write(asset_root.join("resources/model.bin"), [5u8, 7, 9]).unwrap();
        let mut assets = AssetManifest::default();
        assets
            .register("shader", "resources/filter.hlsl", &asset_root)
            .unwrap();
        assets
            .register("mesh", "resources/model.bin", &asset_root)
            .unwrap();
        let mut doc = crate::host::settings_demo_document(WorldKind::Ecs);
        doc.asset_manifest = serde_json::to_value(assets).unwrap();
        // Inline shaders use the workspace include roots, regardless of source ID.
        doc.shader_sources.insert(
            "arbitrary:key".into(),
            "#include \"custom_compute.hlsl\"\n".into(),
        );
        let target = temp.join("export with spaces");
        let output = export_with_asset_root(&target, &doc, &asset_root).unwrap();
        let package: crate::distribution::PackageManifest =
            serde_json::from_slice(&fs::read(output.join(".module-engine-package.json")).unwrap())
                .unwrap();
        for path in [
            "README.md",
            "README.ru.md",
            "AGENTS.md",
            ".agents/skills/moduleengine-debug/SKILL.md",
            ".codex/config.toml",
        ] {
            assert_eq!(
                fs::read(output.join(path)).unwrap(),
                fs::read(source_root().join(path)).unwrap(),
                "Full export must preserve exact public guidance bytes: {path}"
            );
            assert_eq!(
                package.backend_files.get(path),
                Some(&hash_file(&source_root().join(path)).unwrap()),
                "Full backend inventory must pin its public guidance: {path}"
            );
        }
        for path in [
            "crates/render-vulkan/Cargo.toml",
            "crates/runtime-script/Cargo.toml",
            "crates/project-tools/Cargo.toml",
            "scripts/bootstrap.ps1",
            "toolchain-manifest.json",
            "assets/gpu/custom_compute.hlsl",
            "resources/shared.hlsli",
            "resources/model.bin",
        ] {
            assert!(output.join(path).is_file(), "missing {path}");
        }
        #[cfg(windows)]
        for path in [
            "tools/dxc/dxc.exe",
            "tools/dxc/dxcompiler.dll",
            "tools/dxc/dxil.dll",
            "tools/dxc/LICENSE-MS.txt",
            "tools/dxc/LICENSE-LLVM.txt",
            "tools/dxc/download.json",
        ] {
            assert!(output.join(path).is_file(), "missing {path}");
        }
        #[cfg(target_os = "linux")]
        for path in ["tools/dxc/bin/dxc", "tools/dxc/lib/libdxcompiler.so"] {
            // Full source authoring is deliberately independent of installed
            // tools. Preserve a native compiler when one was supplied.
            let original = source_root().join(path);
            if original.is_file() {
                assert_eq!(
                    hash_file(&original).unwrap(),
                    hash_file(&output.join(path)).unwrap()
                );
            }
        }
        assert!(output.join(crate::distribution::MANIFEST_NAME).is_file());
        assert!(
            crate::distribution::detect(
                &output.join("project.json"),
                &read(&output.join("project.json")).unwrap(),
                None
            )
            .unwrap()
            .can_author_engine()
        );
        assert!(!output.join(".work").exists());
        assert!(!output.join("target").exists());
        let restored = read(&output.join("project.json")).unwrap();
        assert_eq!(restored.custom_sources, doc.custom_sources);
        assert_eq!(restored.shader_sources, doc.shader_sources);
        let manifest = AssetManifest::from_value(&restored.asset_manifest).unwrap();
        manifest.verify(&output).unwrap();
        assert_eq!(
            manifest.assets["mesh"],
            AssetManifest::from_value(&doc.asset_manifest)
                .unwrap()
                .assets["mesh"]
        );
        assert!(
            manifest.assets["shader"]
                .dependencies
                .iter()
                .any(|id| manifest.assets[id].path == "resources/shared.hlsli")
        );
        assert!(
            manifest
                .assets
                .values()
                .any(|entry| entry.path == "assets/gpu/custom_compute.hlsl")
        );
        assert_eq!(
            fs::read(output.join("resources/shared.hlsli")).unwrap(),
            b"// include preserved\n"
        );
        let registry = crate::host::registry().unwrap();
        assert!(registry.descriptor("gpu.custom").is_some());
        assert!(registry.is_renderer("render.vulkan"));
        assert_eq!(
            restored.module_versions["world.ecs"],
            registry.module_version("world.ecs").unwrap()
        );
        assert!(export_with_asset_root(&target, &doc, &asset_root).is_err());
        // An external asset cannot replace the header selected by the compiler.
        fs::create_dir_all(asset_root.join("assets/gpu")).unwrap();
        fs::write(
            asset_root.join("assets/gpu/custom_compute.hlsl"),
            "different include bytes",
        )
        .unwrap();
        let mut conflicting = AssetManifest::from_value(&doc.asset_manifest).unwrap();
        conflicting
            .register(
                "conflicting-header",
                "assets/gpu/custom_compute.hlsl",
                &asset_root,
            )
            .unwrap();
        let mut bad = doc.clone();
        bad.asset_manifest = serde_json::to_value(conflicting).unwrap();
        let rejected = temp.join("rejected collision");
        assert!(export_with_asset_root(&rejected, &bad, &asset_root).is_err());
        assert!(!rejected.exists());
        fs::write(asset_root.join("project.json"), b"original asset bytes").unwrap();
        let mut reserved = AssetManifest::default();
        reserved
            .register("graph-file", "project.json", &asset_root)
            .unwrap();
        bad.asset_manifest = serde_json::to_value(reserved).unwrap();
        assert!(export_with_asset_root(&rejected, &bad, &asset_root).is_err());
        assert!(!rejected.exists());
        assert_eq!(
            fs::read(asset_root.join("project.json")).unwrap(),
            b"original asset bytes"
        );
        cleanup(&temp);
    }
    #[test]
    fn export_rejects_copied_trees_and_preserves_nonempty_destination() {
        let root = source_root().canonicalize().unwrap();
        for copied in [
            "crates", "docs", "examples", "assets", "scripts", "tools", ".sdk", ".user", ".tools",
        ] {
            assert!(validate_destination(&root, &root.join(copied).join("new export")).is_err());
        }
        let temp = fixture("nonempty");
        fs::write(temp.join("keep.txt"), b"untouched").unwrap();
        assert!(export(&temp, &demo_document(WorldKind::Objects)).is_err());
        assert_eq!(fs::read(temp.join("keep.txt")).unwrap(), b"untouched");
        cleanup(&temp);
    }

    #[test]
    fn legacy_pin_migration_is_explicit_and_preserves_other_versions() {
        let root = fixture("known pin migration");
        let path = root.join("project.json");
        let mut doc = demo_document(WorldKind::Objects);
        doc.schema_version = 2;
        doc.module_versions
            .insert("world.objects".into(), "0.2.0".into());
        doc.module_versions
            .insert("world.third_party".into(), "0.2.0".into());
        doc.module_versions
            .insert("clock.time".into(), "9.9.0".into());
        let bytes = serde_json::to_vec(&doc).unwrap();
        fs::write(&path, &bytes).unwrap();
        assert!(read(&path).unwrap_err().contains("migration"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let mut current = doc;
        current.schema_version = 3;
        let current_bytes = serde_json::to_vec(&current).unwrap();
        fs::write(&path, &current_bytes).unwrap();
        assert!(read(&path).unwrap_err().contains("migration"));
        assert_eq!(fs::read(&path).unwrap(), current_bytes);
        cleanup(&root);
    }
    #[test]
    fn feature_only_module_versions_are_stamped_without_overwriting_conflicts() {
        let mut doc = demo_document(WorldKind::Objects);
        doc.engine.enabled_modules.insert("game.motion".into());
        let registry = crate::host::registry().unwrap();
        stamp_module_versions(&mut doc, &registry);
        assert_eq!(
            doc.module_versions["game.motion"],
            env!("CARGO_PKG_VERSION")
        );
        doc.module_versions
            .insert("game.motion".into(), "incompatible".into());
        stamp_module_versions(&mut doc, &registry);
        assert_eq!(doc.module_versions["game.motion"], "incompatible");
    }

    #[test]
    fn team_delivery_keeps_audio_and_silent_arpg_variants_when_cues_change_but_puzzle_stays_independent()
     {
        let backend = module_engine_vulkan::resource_root();
        let mut arpg = crate::game_host::demo_document();
        let with_cues = crate::game_package::team_feature_sets(&arpg, &backend);
        if module_engine_host_services::WAV_PLAYBACK_SUPPORTED {
            let variants = with_cues.as_ref().unwrap();
            assert_eq!(variants.len(), 2);
            assert!(
                variants
                    .iter()
                    .all(|features| features.contains("arpg") && !features.contains("puzzle"))
            );
            assert_eq!(
                variants
                    .iter()
                    .filter(|features| features.contains("audio"))
                    .count(),
                1
            );
        } else {
            assert!(with_cues.as_ref().unwrap_err().contains("audio.wav"));
        }
        let modules = arpg.module_versions.clone();
        let lock = arpg.engine_lock.clone();
        arpg.game.as_mut().unwrap().audio_cues.clear();
        let without_cues = crate::game_package::team_feature_sets(&arpg, &backend).unwrap();
        if module_engine_host_services::WAV_PLAYBACK_SUPPORTED {
            assert_eq!(without_cues, with_cues.unwrap());
        } else {
            assert_eq!(without_cues.len(), 1);
            assert!(!without_cues[0].contains("audio"));
        }
        assert_eq!(arpg.module_versions, modules);
        assert_eq!(arpg.engine_lock, lock);
        let puzzle = crate::game_host::puzzle_document();
        let variants = crate::game_package::team_feature_sets(&puzzle, &backend).unwrap();
        assert_eq!(variants.len(), 1);
        assert!(variants[0].contains("puzzle"));
        assert!(!variants[0].contains("arpg") && !variants[0].contains("audio"));
    }

    #[test]
    fn custom_only_schema_consumers_select_the_owner_package_without_activating_gameplay_or_audio()
    {
        let backend = module_engine_vulkan::resource_root();
        let composition = crate::host::composition_at(&backend).unwrap();
        for (component, owner, expected, unexpected, mut document) in [
            (
                "health",
                "game.health",
                "arpg",
                "puzzle",
                crate::game_host::demo_document(),
            ),
            (
                "puzzle.board",
                "puzzle.turn",
                "puzzle",
                "arpg",
                crate::game_host::puzzle_document(),
            ),
        ] {
            let mut game = module_engine_core::GameProject::empty();
            game.execution = module_engine_core::ExecutionGraph {
                resources: [
                    ("world", "GameWorld"),
                    ("input", "GameInput"),
                    ("events", "GameEvents"),
                ]
                .into_iter()
                .map(|(name, data_type)| module_engine_core::SystemResource {
                    name: name.into(),
                    data_type: data_type.into(),
                    external: true,
                    initial: serde_json::Value::Null,
                })
                .collect(),
                stages: vec![module_engine_core::SystemStage {
                    name: "Custom schema consumer".into(),
                    nodes: vec![module_engine_core::SystemNode {
                        id: "consumer".into(),
                        module: "custom.rust.update".into(),
                        params: serde_json::json!({"source_id":"schema-consumer","components":[component]}),
                        bindings: ["world", "input", "events"]
                            .into_iter()
                            .map(|name| (name.into(), name.into()))
                            .collect(),
                        after: vec![],
                    }],
                }],
            };
            document.game = Some(game);
            document.custom_sources = std::collections::BTreeMap::from([(
                "schema-consumer".into(),
                "fn update(_ctx: &mut ScriptContext) {}".into(),
            )]);
            let identity = module_engine_project::EngineProfile::from_document(&document)
                .unwrap()
                .identity;
            composition
                .capture_lock(
                    &document,
                    identity,
                    Some(module_engine_core::SdkIdentity::current()),
                )
                .unwrap()
                .apply_to(&mut document);
            let game = document.game.as_ref().unwrap();
            assert!(
                composition
                    .systems
                    .active_game_contributions(&game.execution)
                    .is_empty()
            );
            let required = composition.required_contracts(&document).unwrap();
            assert!(required.module_versions.contains_key(owner));
            assert!(required.component_schemas.contains_key(component));
            let key = crate::game_package::native_feature_key(&document, &backend).unwrap();
            let features = key.split('-').collect::<std::collections::BTreeSet<_>>();
            assert!(features.contains(expected));
            assert!(!features.contains(unexpected) && !features.contains("audio"));
        }
    }
}
