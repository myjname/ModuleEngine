//! Author project creation/relocation. Engine installation and editable game
//! resources have separate roots; saving never creates a runtime projection.
use crate::distribution::{Distribution, ProjectEngineBinding, project_binding_path};
use module_engine_core::{AuthoredScene, GameProject, GraphDocument};
use module_engine_project::{
    AssetManifest, collect_hlsl_dependencies, hash_file, normalize_relative, resolve_relative,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectTemplate {
    Empty,
    TwoArenas,
    Puzzle,
}
#[derive(Clone, Debug)]
pub struct SavedProject {
    pub path: PathBuf,
    pub document: GraphDocument,
}

/// Explicitly migrate into another project. Preparation validates all contracts,
/// resources and active Rust candidates before publication can create output.
pub fn migrate_copy(
    source_path: &Path,
    destination_path: &Path,
    kind: crate::engine_profiles::ProfileKind,
) -> Result<(SavedProject, module_engine_project::MigrationReport), String> {
    let source = absolute_path(source_path)?;
    let destination = absolute_path(destination_path)?;
    let same_source = if cfg!(windows) {
        source
            .to_string_lossy()
            .eq_ignore_ascii_case(&destination.to_string_lossy())
    } else {
        source == destination
    };
    if same_source {
        return Err(
            "Migration writes a separate copy; source filename cannot be its destination".into(),
        );
    }
    let raw = fs::read(&source).map_err(|e| e.to_string())?;
    let legacy = module_engine_project::inspect_legacy_document(&raw)?;
    let distribution = crate::distribution::detect(&source, &legacy, None)?;
    let candidate =
        crate::engine_profiles::migration_candidate(&raw, kind, &distribution.package_root)?;
    distribution.validate_document(&candidate.document)?;
    crate::host::validate_document_at(&candidate.document, &distribution.package_root)?;
    let asset_root = source.parent().ok_or("Source project folder missing")?;
    AssetManifest::from_value(&candidate.document.asset_manifest)?.verify(asset_root)?;
    // This checks actual referenced model/texture dependencies as well as the
    // complete author resource set, with no world/simulation construction.
    let files = collect_resources(&candidate.document, asset_root, &distribution.package_root)?;
    let mut scripts = BTreeMap::new();
    fn root_scripts(
        graph: &module_engine_core::Graph,
        scripts: &mut BTreeMap<String, module_engine_script::ScriptKind>,
    ) {
        for node in &graph.nodes {
            if matches!(node.module.as_str(), "custom.rust" | "custom.rust.update")
                && let Some(id) = node
                    .params
                    .get("source_id")
                    .and_then(serde_json::Value::as_str)
            {
                scripts.insert(
                    id.into(),
                    if node.module == "custom.rust" {
                        module_engine_script::ScriptKind::Speed
                    } else {
                        module_engine_script::ScriptKind::Update
                    },
                );
            }
            if let Some(inner) = &node.subgraph {
                root_scripts(inner, scripts);
            }
        }
    }
    root_scripts(&candidate.document.root, &mut scripts);
    if let Some(game) = &candidate.document.game {
        for id in crate::game_host::script_ids(game) {
            scripts.insert(id, module_engine_script::ScriptKind::Update);
        }
    }
    if !scripts.is_empty() {
        let compiler_root = distribution.compile_root()?;
        let compiler = module_engine_script::ScriptCompiler::new(
            compiler_root.clone(),
            compiler_root.join(".work/migration-scripts"),
        );
        for (id, kind) in scripts {
            let text = candidate
                .document
                .custom_sources
                .get(&id)
                .ok_or_else(|| format!("Missing migration Custom '{id}'"))?;
            compiler
                .compile_kind(text, kind)
                .map_err(|e| e.to_string())?;
        }
    }
    if fs::read(&source).map_err(|e| e.to_string())? != raw {
        return Err("Source changed during migration preparation".into());
    }
    check_roots(
        asset_root,
        destination
            .parent()
            .ok_or("Migration destination folder missing")?,
        &files,
    )?;
    let saved = publish(destination, candidate.document, files, &distribution, false)?;
    Ok((saved, candidate.report))
}

/// Build game content around the selected Engine, retaining its graph, source
/// identifiers, shader text, version pins and execution contracts exactly.
#[cfg(test)]
pub fn template_document(
    current: &GraphDocument,
    template: ProjectTemplate,
    distribution: &Distribution,
) -> Result<GraphDocument, String> {
    template_document_with_choice(
        current,
        template,
        distribution,
        crate::engine_profiles::EngineChoice::InheritCurrent,
    )
}
pub fn template_document_with_choice(
    current: &GraphDocument,
    template: ProjectTemplate,
    distribution: &Distribution,
    choice: crate::engine_profiles::EngineChoice,
) -> Result<GraphDocument, String> {
    use crate::engine_profiles::EngineChoice;
    use module_engine_project::EngineProfile;
    distribution.validate_document(current)?;
    let composition = crate::host::composition_at(&distribution.package_root)?;
    let current_profile = EngineProfile::from_document(current)?;
    let profile = match choice {
        EngineChoice::InheritCurrent => current_profile.clone(),
        EngineChoice::Selected(kind) => {
            let selected = crate::engine_profiles::select(kind, &distribution.package_root)?;
            if !distribution.can_author_engine() && selected.identity != current_profile.identity {
                return Err("Team can inherit its checked Engine; selecting another profile requires Full sources".into());
            }
            selected
        }
    };
    let mut document = current.clone();
    if distribution.can_author_engine() {
        document.module_versions.clear();
        document.engine_lock = None;
    }
    document.legacy_archive.clear();
    if matches!(choice, EngineChoice::Selected(_)) {
        document.root = profile.blueprint.root.clone();
        document.engine = profile.blueprint.engine.clone();
        document.shader_sources = profile.blueprint.shader_sources.clone();
    }
    let engine_sources = if distribution.can_author_engine() {
        crate::engine_profiles::engine_sources(&document)
    } else {
        let project_ids = current
            .game
            .as_ref()
            .map(crate::game_host::script_ids)
            .unwrap_or_default();
        current
            .custom_sources
            .iter()
            .filter(|(id, _)| !project_ids.contains(*id))
            .map(|(id, source)| (id.clone(), source.clone()))
            .collect()
    };
    document.custom_sources = engine_sources;
    for (id, source) in &mut document.custom_sources {
        if distribution.can_edit_custom_source(id, current) {
            *source = "fn update(_ctx: &mut ScriptContext) {}\n".into();
        }
    }
    document.asset_manifest = serde_json::Value::Null;
    document.project = Default::default();
    document.game = Some(match template {
        ProjectTemplate::Empty => {
            let mut game = GameProject::empty();
            game.entry_scene = "scene".into();
            game.scenes = BTreeMap::from([(
                "scene".into(),
                AuthoredScene {
                    name: "Scene".into(),
                    objects: vec![],
                },
            )]);
            game.execution = profile.blueprint.default_execution.clone();
            if !distribution.can_author_engine()
                && let Some(existing) = &current.game
            {
                // A Team installation explicitly freezes its execution contract.
                // Reuse that contract with fresh project code, not old logic.
                game.execution = existing.execution.clone();
                for id in crate::game_host::script_ids(&game) {
                    document
                        .custom_sources
                        .insert(id, "fn update(_ctx: &mut ScriptContext) {}\n".into());
                }
            }
            game
        }
        ProjectTemplate::TwoArenas => {
            let content = module_engine_two_arenas::document(document.clone());
            for (id, source) in content.custom_sources {
                if document.custom_sources.contains_key(&id)
                    && !distribution.can_edit_custom_source(&id, current)
                {
                    return Err(format!(
                        "Project source '{id}' conflicts with an Engine-owned source"
                    ));
                }
                document.custom_sources.insert(id, source);
            }
            content.game.unwrap()
        }
        ProjectTemplate::Puzzle => module_engine_project_puzzle::project(),
    });
    document.name = match template {
        ProjectTemplate::Empty => "New project",
        ProjectTemplate::TwoArenas => "Two arenas",
        ProjectTemplate::Puzzle => "Crate puzzle",
    }
    .into();
    document.properties.active.objects.clear();
    document.properties.active.components.clear();
    document.properties.inactive.objects.clear();
    document.properties.inactive.components.clear();
    document.properties.detached_components.clear();
    if distribution.can_author_engine() {
        crate::engine_profiles::attach(&mut document, &composition, &profile)?;
    } else if let Some(game) = &document.game {
        for id in crate::game_host::script_ids(game) {
            document
                .custom_sources
                .insert(id, "fn update(_ctx: &mut ScriptContext) {}\n".into());
        }
        profile.validate_with_sdk(
            &document,
            &composition,
            Some(&module_engine_core::SdkIdentity::current()),
        )?;
    }
    distribution.validate_document(&document)?;
    validate_document(&document, distribution)?;
    composition.validate_game(&document).map_err(|error| {
        if distribution.can_author_engine() {
            format!("New Project violates the selected Engine contract: {error}")
        } else {
            format!("New Project cannot satisfy the frozen Engine contract: {error}")
        }
    })?;
    Ok(document)
}

/// Copy an entire author project to another JSON, preserving unused assets,
/// archived values, templates and sources. The caller changes its working state
/// only after this function has published the project successfully.
pub fn save_as(
    source_path: &Path,
    destination_path: &Path,
    document: &GraphDocument,
    distribution: &Distribution,
) -> Result<SavedProject, String> {
    save_as_impl(source_path, destination_path, document, distribution, false)
}
/// Used only after the native Save dialog has explicitly confirmed replacement.
/// Existing project metadata receives exact byte backups; resource conflicts
/// still fail before any old metadata is changed.
pub fn save_as_confirmed(
    source_path: &Path,
    destination_path: &Path,
    document: &GraphDocument,
    distribution: &Distribution,
) -> Result<SavedProject, String> {
    if !destination_path.exists() {
        return save_as(source_path, destination_path, document, distribution);
    }
    save_as_impl(source_path, destination_path, document, distribution, true)
}
fn save_as_impl(
    source_path: &Path,
    destination_path: &Path,
    document: &GraphDocument,
    distribution: &Distribution,
    overwrite_confirmed: bool,
) -> Result<SavedProject, String> {
    distribution.validate_document(document)?;
    crate::engine_profiles::validate(document, &distribution.package_root)?;
    // Like Save, Save As can retain a graph that is still being edited. Engine
    // policy and resource ownership are checked; Tools validates executability.
    let source = absolute_path(source_path)?;
    let destination = absolute_path(destination_path)?;
    if destination == source {
        return Err("Save As requires another filename; use Save for the current project".into());
    }
    let source_root = source
        .parent()
        .ok_or("Source project has no folder")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let destination_root = destination.parent().ok_or("Destination has no folder")?;
    let files = if destination_root == source_root {
        BTreeMap::new()
    } else {
        collect_resources(document, &source_root, &distribution.package_root)?
    };
    check_roots(&source_root, destination_root, &files)?;
    publish(
        destination,
        document.clone(),
        files,
        distribution,
        overwrite_confirmed,
    )
}

/// Create a project.json in a new/empty folder, using the current architecture.
pub fn new_project(
    source_path: &Path,
    destination_folder: &Path,
    current: &GraphDocument,
    distribution: &Distribution,
    template: ProjectTemplate,
) -> Result<SavedProject, String> {
    new_project_with_choice(
        source_path,
        destination_folder,
        current,
        distribution,
        template,
        crate::engine_profiles::EngineChoice::InheritCurrent,
    )
}
pub fn new_project_with_choice(
    source_path: &Path,
    destination_folder: &Path,
    current: &GraphDocument,
    distribution: &Distribution,
    template: ProjectTemplate,
    choice: crate::engine_profiles::EngineChoice,
) -> Result<SavedProject, String> {
    let document = template_document_with_choice(current, template, distribution, choice)?;
    let source = absolute_path(source_path)?;
    let source_root = source
        .parent()
        .ok_or("Source project has no folder")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let folder = absolute_path(destination_folder)?;
    if folder == source_root
        || (folder.exists()
            && fs::read_dir(&folder)
                .map_err(|e| e.to_string())?
                .next()
                .is_some())
    {
        return Err("New Project requires a new or empty folder".into());
    }
    let resource_root = if template == ProjectTemplate::TwoArenas {
        distribution
            .package_root
            .canonicalize()
            .map_err(|e| e.to_string())?
    } else {
        source_root.clone()
    };
    let files = if template == ProjectTemplate::TwoArenas {
        collect_resources(&document, &resource_root, &distribution.package_root)?
    } else {
        BTreeMap::new()
    };
    check_roots(&resource_root, &folder, &files)?;
    publish(
        folder.join("project.json"),
        document,
        files,
        distribution,
        false,
    )
}

fn validate_document(document: &GraphDocument, distribution: &Distribution) -> Result<(), String> {
    crate::engine_profiles::validate(document, &distribution.package_root)?;
    let registry = crate::host::registry_at(&distribution.package_root)?;
    let errors = module_engine_core::validate_graph(document, &registry)
        .into_iter()
        .filter(|d| d.severity == module_engine_core::Severity::Error)
        .map(|d| d.message)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(format!("Project cannot be saved: {}", errors.join("; ")));
    }
    Ok(())
}

/// Resolve existing ancestors before checking containment, including a target
/// whose final directory has not been created yet. Parent traversal is rejected.
fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("Project paths may not contain parent traversal".into());
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let mut ancestor = path.as_path();
    let mut suffix = vec![];
    while !ancestor.exists() {
        suffix.push(
            ancestor
                .file_name()
                .ok_or("Invalid project path")?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .ok_or("Project path has no existing ancestor")?;
    }
    let mut result = ancestor.canonicalize().map_err(|e| e.to_string())?;
    for part in suffix.into_iter().rev() {
        result.push(part);
    }
    Ok(result)
}
fn check_roots(
    source: &Path,
    target: &Path,
    files: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    if source == target {
        return Ok(());
    }
    if target.starts_with(source.join("assets")) {
        return Err("Project destination may not be inside the source assets tree".into());
    }
    for file in files.values() {
        let parent = file.parent().ok_or("Resource has no source directory")?;
        if parent != source && target.starts_with(parent) {
            return Err(format!(
                "Project destination may not be inside a copied resource directory: {}",
                parent.display()
            ));
        }
    }
    Ok(())
}
fn add_file(
    root: &Path,
    relative: &str,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let relative = normalize_relative(relative)?;
    files.insert(relative.clone(), resolve_relative(root, &relative)?);
    Ok(())
}
fn collect_tree(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    if fs::symlink_metadata(directory)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "Project resources reject symbolic links: {}",
            directory.display()
        ));
    }
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let path = entry.path();
        if kind.is_symlink() {
            return Err(format!(
                "Project resources reject symbolic links: {}",
                path.display()
            ));
        }
        if kind.is_dir() {
            collect_tree(root, &path, files)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy();
            add_file(root, &relative, files)?;
        }
    }
    Ok(())
}
fn resource_owner_is_known(
    document: &GraphDocument,
    composition: &module_engine_core::CompositionContext,
    owner: &str,
) -> bool {
    composition.module_version(owner).is_some_and(|installed| {
        document
            .module_versions
            .get(owner)
            .is_none_or(|saved| saved == installed)
    })
}
fn resource_component_schema<'a>(
    document: &GraphDocument,
    composition: &'a module_engine_core::CompositionContext,
    component: &str,
) -> Option<&'a module_engine_core::ComponentSchema> {
    let schema = composition.systems.components().schema(component)?;
    if !resource_owner_is_known(document, composition, &schema.owner_module) {
        return None;
    }
    if let Some(saved) = document
        .engine_lock
        .as_ref()
        .and_then(|lock| lock.component_schemas.get(component))
        && module_engine_core::SchemaPin::capture(schema).ok().as_ref() != Some(saved)
    {
        // An inactive archived contract is opaque even if an installed schema
        // happens to reuse its component ID.
        return None;
    }
    Some(schema)
}
fn add_resource_value(value: Option<&serde_json::Value>, paths: &mut BTreeSet<String>) {
    if let Some(reference) = value.and_then(serde_json::Value::as_str) {
        paths.insert(reference.into());
    }
}
fn component_resources(
    components: &BTreeMap<String, serde_json::Value>,
    document: &GraphDocument,
    composition: &module_engine_core::CompositionContext,
    paths: &mut BTreeSet<String>,
) {
    for (id, value) in components {
        let Some(schema) = resource_component_schema(document, composition, id) else {
            continue;
        };
        for field in &schema.fields {
            if matches!(
                field.value_type,
                module_engine_core::PropertyType::AssetReference { .. }
            ) {
                add_resource_value(
                    schema
                        .field_path(field)
                        .and_then(|path| module_engine_core::component_field(value, path)),
                    paths,
                );
            }
        }
    }
}
fn property_resources(
    data: &module_engine_core::PropertyData,
    document: &GraphDocument,
    composition: &module_engine_core::CompositionContext,
    paths: &mut BTreeSet<String>,
) {
    use module_engine_core::{PropertyScope, PropertyType};
    for module in composition.modules.descriptors() {
        if !resource_owner_is_known(document, composition, &module.id) {
            continue;
        }
        for field in &module.properties {
            if !matches!(field.value_type, PropertyType::AssetReference { .. }) {
                continue;
            }
            match &field.scope {
                PropertyScope::Global => add_resource_value(data.global.get(&field.id), paths),
                PropertyScope::Module => add_resource_value(
                    data.modules
                        .get(&module.id)
                        .and_then(|values| values.get(&field.id)),
                    paths,
                ),
                PropertyScope::Object => {
                    for values in data.objects.values() {
                        add_resource_value(values.get(&field.id), paths);
                    }
                }
                PropertyScope::Component { component } => {
                    for values in data.components.values() {
                        add_resource_value(
                            values
                                .get(component)
                                .and_then(|values| values.get(&field.id)),
                            paths,
                        );
                    }
                }
            }
        }
    }
    // Older property-store component records use flat field IDs rather than
    // the authored scene's aggregate JSON. Retain their known asset fields too.
    for components in data.components.values() {
        for (id, values) in components {
            let Some(schema) = resource_component_schema(document, composition, id) else {
                continue;
            };
            for field in &schema.fields {
                if matches!(field.value_type, PropertyType::AssetReference { .. }) {
                    add_resource_value(
                        values
                            .get(&field.id)
                            .or_else(|| schema.field_path(field).and_then(|path| values.get(path))),
                        paths,
                    );
                }
            }
        }
    }
}
fn collect_resources(
    document: &GraphDocument,
    root: &Path,
    backend: &Path,
) -> Result<BTreeMap<String, PathBuf>, String> {
    // shader_sources contains Engine-owned inline code. Its #include lookup,
    // compiler and cache remain in the selected installed package through the
    // per-JSON binding. Only physical author HLSL assets use this resource root.
    let mut files = BTreeMap::new();
    collect_tree(root, &root.join("assets"), &mut files)?;
    let manifest =
        AssetManifest::from_value(&document.asset_manifest)?.with_hlsl_dependencies(root)?;
    let mut paths: BTreeSet<String> = manifest
        .assets
        .values()
        .map(|entry| entry.path.clone())
        .collect();
    // Unassigned models/shaders are still author content; their transitive files
    // can live elsewhere inside the source project's boundary.
    for path in files.keys() {
        let extension = Path::new(path)
            .extension()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        if ["gltf", "glb", "hlsl", "hlsli"]
            .iter()
            .any(|kind| extension.eq_ignore_ascii_case(kind))
        {
            paths.insert(path.clone());
        }
    }
    // Only registered asset fields have reference semantics. Unknown/inactive
    // payloads can contain identical-looking keys with an unrelated meaning.
    // The complete manifest and physical assets tree remain author content.
    let composition = crate::host::composition_at(backend)?;
    property_resources(
        &document.properties.active,
        document,
        &composition,
        &mut paths,
    );
    property_resources(
        &document.properties.inactive,
        document,
        &composition,
        &mut paths,
    );
    if let Some(game) = &document.game {
        for prefab in game.prefabs.values() {
            component_resources(&prefab.components, document, &composition, &mut paths);
        }
        for object in game.scenes.values().flat_map(|scene| &scene.objects) {
            component_resources(&object.components, document, &composition, &mut paths);
            let inherited = object.prefab.as_ref().and_then(|id| game.prefabs.get(id));
            for (field, value) in &object.overrides {
                let owner = object
                    .components
                    .keys()
                    .chain(
                        inherited
                            .into_iter()
                            .flat_map(|prefab| prefab.components.keys()),
                    )
                    .filter(|id| {
                        field
                            .strip_prefix(id.as_str())
                            .is_some_and(|tail| tail.starts_with('.'))
                    })
                    .max_by_key(|id| id.len());
                if let Some(schema) =
                    owner.and_then(|id| resource_component_schema(document, &composition, id))
                    && schema.fields.iter().any(|registered| {
                        registered.id == *field
                            && matches!(
                                registered.value_type,
                                module_engine_core::PropertyType::AssetReference { .. }
                            )
                    })
                {
                    add_resource_value(Some(value), &mut paths);
                }
            }
        }
        paths.extend(game.audio_cues.values().cloned());
    }
    let mut library = module_engine_content::ContentLibrary::new(root)?;
    library.set_asset_paths(manifest.asset_paths());
    paths = paths
        .into_iter()
        .map(|path| manifest.resolve_reference(&path))
        .collect::<Result<_, _>>()?;
    let declared = paths.clone();
    for path in &declared {
        match Path::new(path)
            .extension()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "gltf" | "glb" => paths.extend(library.load_model(path)?.dependencies.iter().cloned()),
            "hlsl" | "hlsli" => paths.extend(collect_hlsl_dependencies(root, path)?),
            _ => {}
        }
    }
    for path in paths {
        add_file(root, &path, &mut files)?;
    }
    Ok(files)
}

fn nonce() -> Result<String, String> {
    Ok(format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ))
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn create_directories(path: &Path, created: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut missing = vec![];
    let mut ancestor = path;
    while !ancestor.exists() {
        missing.push(ancestor.to_path_buf());
        ancestor = ancestor
            .parent()
            .ok_or("Destination directory has no ancestor")?;
    }
    for directory in missing.into_iter().rev() {
        fs::create_dir(&directory).map_err(|e| e.to_string())?;
        created.push(directory);
    }
    Ok(())
}
fn remove_empty_created_directories(created: Vec<PathBuf>) {
    for directory in created.into_iter().rev() {
        let _ = fs::remove_dir(directory);
    }
}
fn publish_metadata(
    staged: &Path,
    target: &Path,
    original: Option<&[u8]>,
    created: &mut Vec<PathBuf>,
    replaced: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), String> {
    if let Some(original) = original {
        if fs::read(target).map_err(|e| e.to_string())? != original {
            return Err(format!(
                "Destination changed after overwrite confirmation: {}",
                target.display()
            ));
        }
        let mut name = target
            .file_name()
            .ok_or("Metadata target has no filename")?
            .to_os_string();
        name.push(format!(".backup-{}", nonce()?));
        let backup = target.with_file_name(name);
        write_new(&backup, original)?;
        created.push(backup.clone());
        fs::rename(staged, target).map_err(|e| e.to_string())?;
        replaced.push((target.to_path_buf(), backup));
    } else {
        move_new_file(staged, target)?;
        created.push(target.to_path_buf());
    }
    Ok(())
}
/// Publish a flushed staging file without replacing a racing writer. Windows
/// hard links are unavailable in some managed workspaces, so use the native
/// same-volume atomic move without MOVEFILE_REPLACE_EXISTING.
#[cfg(windows)]
fn move_new_file(staged: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let staged: Vec<_> = staged
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<_> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // Both paths are absolute siblings on the same volume; no copy fallback.
    if unsafe { MoveFileExW(staged.as_ptr(), target.as_ptr(), 0x0000_0008) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
#[cfg(not(windows))]
fn move_new_file(staged: &Path, target: &Path) -> Result<(), String> {
    fs::hard_link(staged, target).map_err(|e| e.to_string())
}
/// Fresh destinations are committed by one directory rename. For an existing
/// folder, resources are preflighted/staged and the new JSON is published last;
/// failed commits remove only files created by this operation.
fn publish(
    destination: PathBuf,
    document: GraphDocument,
    files: BTreeMap<String, PathBuf>,
    distribution: &Distribution,
    overwrite_confirmed: bool,
) -> Result<SavedProject, String> {
    let root = destination.parent().ok_or("Destination has no folder")?;
    let name = destination
        .file_name()
        .ok_or("Destination has no filename")?;
    if destination.exists() && !overwrite_confirmed {
        return Err("Destination project already exists; choose another filename".into());
    }
    let old_project = if destination.exists() {
        if !destination.is_file() {
            return Err("Destination project is not a file".into());
        }
        Some(fs::read(&destination).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let binding = ProjectEngineBinding::from_distribution(distribution)?;
    let binding_bytes = serde_json::to_vec_pretty(&binding).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
    let existing_binding = project_binding_path(&destination)?;
    let binding_name = existing_binding
        .file_name()
        .ok_or("Binding has no filename")?
        .to_string_lossy()
        .into_owned();
    let binding_exists = existing_binding.exists();
    let old_binding = if binding_exists {
        Some(fs::read(&existing_binding).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let mut binding_changed = !binding_exists;
    if binding_exists {
        if fs::symlink_metadata(&existing_binding)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Destination project binding is a symbolic link".into());
        }
        let existing: ProjectEngineBinding =
            serde_json::from_slice(&fs::read(&existing_binding).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        binding_changed = existing != binding;
        if binding_changed && !overwrite_confirmed {
            return Err("Destination folder is bound to another engine/profile".into());
        }
    }
    if root.join(crate::distribution::MANIFEST_NAME).exists()
        && root
            != distribution
                .package_root
                .canonicalize()
                .map_err(|e| e.to_string())?
    {
        return Err("Destination is another engine package; choose a project folder".into());
    }
    for (relative, source) in &files {
        if relative == &name.to_string_lossy()
            || relative == &binding_name
            || relative == crate::distribution::MANIFEST_NAME
        {
            return Err(format!(
                "Resource path conflicts with project metadata: {relative}"
            ));
        }
        let target = root.join(relative);
        if target.exists()
            && (!absolute_path(&target)?.starts_with(root)
                || fs::symlink_metadata(&target)
                    .map_err(|e| e.to_string())?
                    .file_type()
                    .is_symlink())
        {
            return Err(format!(
                "Destination resource leaves project or is a symbolic link: {relative}"
            ));
        }
        if target.exists() && (!target.is_file() || hash_file(&target)? != hash_file(source)?) {
            return Err(format!(
                "Destination already contains a different resource: {relative}"
            ));
        }
        // Existing intermediate symlinks/reparse points may not redirect output.
        let parent = absolute_path(target.parent().ok_or("Resource has no parent")?)?;
        if !parent.starts_with(root) {
            return Err(format!("Destination resource leaves project: {relative}"));
        }
    }
    // An existing writable project does not imply access to its parent (e.g.
    // a workspace mounted directly below a protected drive root). Only fresh
    // directory publication needs a sibling staging directory for atomic rename.
    let parent = if root.is_dir() {
        root
    } else {
        root.parent().ok_or("Destination folder has no parent")?
    };
    let stage = parent.join(format!(".module-engine-save-{}", nonce()?));
    let mut parent_directories = vec![];
    if let Err(error) = create_directories(parent, &mut parent_directories)
        .and_then(|_| fs::create_dir(&stage).map_err(|e| e.to_string()))
    {
        remove_empty_created_directories(parent_directories);
        return Err(error);
    }
    let result = (|| {
        for (relative, source) in &files {
            let target = stage.join(relative);
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(source, &target).map_err(|e| e.to_string())?;
            if hash_file(&target)? != hash_file(source)? {
                return Err(format!("Resource changed during copy: {relative}"));
            }
        }
        if !files.is_empty() {
            AssetManifest::from_value(&document.asset_manifest)?.verify(&stage)?;
        }
        if binding_changed {
            write_new(&stage.join(&binding_name), &binding_bytes)?;
        }
        write_new(&stage.join(name), &bytes)?;
        if !root.exists() {
            fs::rename(&stage, root).map_err(|e| e.to_string())?;
            return Ok(());
        }
        let mut created = vec![];
        let mut created_directories = vec![];
        let mut replaced = vec![];
        let commit = (|| {
            for relative in files.keys() {
                let target = root.join(relative);
                if target.exists() {
                    if !absolute_path(&target)?.starts_with(root)
                        || fs::symlink_metadata(&target)
                            .map_err(|e| e.to_string())?
                            .file_type()
                            .is_symlink()
                    {
                        return Err(format!(
                            "Destination resource was redirected during save: {relative}"
                        ));
                    }
                    if hash_file(&target)? != hash_file(&stage.join(relative))? {
                        return Err(format!(
                            "Destination resource changed during save: {relative}"
                        ));
                    }
                    continue;
                }
                create_directories(target.parent().unwrap(), &mut created_directories)?;
                if !target
                    .parent()
                    .unwrap()
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(root)
                {
                    return Err(format!(
                        "Destination resource parent was redirected: {relative}"
                    ));
                }
                // create_new reserves a destination without overwriting a race.
                let mut output = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&target)
                    .map_err(|e| e.to_string())?;
                created.push(target);
                let mut input = fs::File::open(stage.join(relative)).map_err(|e| e.to_string())?;
                std::io::copy(&mut input, &mut output)
                    .and_then(|_| output.sync_all())
                    .map_err(|e| e.to_string())?;
            }
            if binding_changed {
                publish_metadata(
                    &stage.join(&binding_name),
                    &existing_binding,
                    old_binding.as_deref(),
                    &mut created,
                    &mut replaced,
                )?;
            }
            publish_metadata(
                &stage.join(name),
                &destination,
                old_project.as_deref(),
                &mut created,
                &mut replaced,
            )
        })();
        if commit.is_err() {
            // Metadata had valid backups before replacement. Restore binding if
            // publishing the JSON failed; the editor has not changed its state.
            let mut restore_failed = false;
            for (target, backup) in replaced.into_iter().rev() {
                let recovery = stage.join(format!("recovery-{}", nonce()?));
                if fs::copy(&backup, &recovery)
                    .and_then(|_| fs::rename(&recovery, target))
                    .is_err()
                {
                    restore_failed = true;
                }
            }
            if restore_failed {
                return Err(format!(
                    "Metadata restore failed; exact backups retained in {}",
                    root.display()
                ));
            }
            for path in created.into_iter().rev() {
                let _ = fs::remove_file(path);
            }
            remove_empty_created_directories(created_directories);
        }
        commit
    })();
    // This is our fresh sibling staging directory, never a computed user tree.
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    if result.is_err() {
        remove_empty_created_directories(parent_directories);
    }
    result?;
    Ok(SavedProject {
        path: destination,
        document,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distribution::{self, DistributionProfile};
    use std::sync::atomic::{AtomicU64, Ordering};
    fn fixture() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "ModuleEngine author files {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn cleanup(root: &Path) {
        let root = root.canonicalize().unwrap();
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        assert!(root.starts_with(&temporary) && root != temporary);
        fs::remove_dir_all(root).unwrap();
    }
    fn document() -> GraphDocument {
        let mut document = crate::game_host::demo_document();
        let game = document.game.as_mut().unwrap();
        game.audio_cues.clear();
        for scene in game.scenes.values_mut() {
            for object in &mut scene.objects {
                object.components.remove("mesh");
            }
        }
        document
    }
    fn distribution(
        document: &GraphDocument,
        profile: DistributionProfile,
    ) -> (module_engine_script::TestArtifactGuard, Distribution) {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.work/test-temp");
        let sources =
            module_engine_script::TestArtifactGuard::new(&base, "workspace-full-source").unwrap();
        distribution::copy_native_fixture(sources.path());
        distribution::write_full_manifest(sources.path(), document).unwrap();
        let capability = distribution::detect(
            &sources.path().join("game-project.json"),
            document,
            Some(profile),
        )
        .unwrap();
        assert_eq!(capability.package_root, sources.path());
        assert_eq!(capability.profile, profile);
        if profile == DistributionProfile::Full {
            assert!(
                capability.can_author_engine(),
                "{:?}",
                capability.source_status
            );
        } else {
            assert!(!capability.can_author_engine());
        }
        (sources, capability)
    }
    fn model_files(root: &Path) {
        fs::create_dir_all(root.join("resources/model")).unwrap();
        let glb = include_bytes!("../../../assets/models/demo_crystal.glb");
        let jsonlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let mut json: serde_json::Value = serde_json::from_slice(&glb[20..20 + jsonlen]).unwrap();
        let binary = &glb[20 + jsonlen + 8..];
        let index = json["images"][0]["bufferView"].as_u64().unwrap() as usize;
        let view = &json["bufferViews"][index];
        let offset = view["byteOffset"].as_u64().unwrap() as usize;
        let length = view["byteLength"].as_u64().unwrap() as usize;
        fs::write(
            root.join("resources/model/checker.png"),
            &binary[offset..offset + length],
        )
        .unwrap();
        fs::write(root.join("resources/model/model.bin"), binary).unwrap();
        json["buffers"][0]["uri"] = serde_json::json!("model.bin");
        json["images"][0] = serde_json::json!({"uri":"checker.png"});
        fs::write(
            root.join("resources/model/model.gltf"),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
    }
    fn opaque_resource_keys() -> serde_json::Value {
        serde_json::json!({
            "base_color_texture":"opaque-token",
            "mesh.path":"asset://not-a-declared-asset",
            "material.base_color_texture":"../opaque-value",
            "mesh":{"path":"C:\\opaque\\not-a-resource.glb"},
            "nested":[{"material":{"base_color_texture":"also-opaque"}}]
        })
    }
    #[test]
    fn save_as_preserves_unknown_inactive_resource_like_keys_without_interpreting_them() {
        let root = fixture();
        let source = root.join("source");
        fs::create_dir_all(source.join("assets/unassigned")).unwrap();
        fs::write(source.join("assets/unassigned/notes.txt"), b"author file").unwrap();
        let mut document = document();
        let opaque = opaque_resource_keys();
        document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .insert("future.opaque".into(), opaque.clone());
        document.game.as_mut().unwrap().prefabs.insert(
            "unused future".into(),
            module_engine_core::Prefab {
                color: [1, 2, 3, 255],
                components: BTreeMap::from([("future.opaque".into(), opaque.clone())]),
            },
        );
        document
            .properties
            .inactive
            .global
            .insert("future.global".into(), opaque.clone());
        document.properties.inactive.components.insert(
            99,
            BTreeMap::from([(
                "future.opaque".into(),
                BTreeMap::from([("base_color_texture".into(), serde_json::json!("not-a-file"))]),
            )]),
        );
        document
            .legacy_archive
            .insert("/unknown_record".into(), opaque);
        let json = source.join("project.json");
        let original = serde_json::to_vec_pretty(&document).unwrap();
        fs::write(&json, &original).unwrap();
        for (name, profile) in [
            ("full", DistributionProfile::Full),
            ("team", DistributionProfile::Team),
        ] {
            let (_capability_sources, capability) = distribution(&document, profile);
            let saved = save_as(
                &json,
                &root.join(name).join("project.json"),
                &document,
                &capability,
            )
            .unwrap();
            assert_eq!(saved.document, document);
            assert_eq!(crate::project::read(&saved.path).unwrap(), document);
            assert_eq!(fs::read(&json).unwrap(), original);
            assert_eq!(
                fs::read(
                    saved
                        .path
                        .parent()
                        .unwrap()
                        .join("assets/unassigned/notes.txt")
                )
                .unwrap(),
                b"author file"
            );
            assert!(!saved.path.parent().unwrap().join("opaque-token").exists());
        }
        cleanup(&root);
    }
    #[test]
    fn explicit_migration_and_followup_save_as_preserve_unknown_payloads_and_source_bytes() {
        let root = fixture();
        let source = root.join("legacy");
        fs::create_dir_all(source.join("assets/unassigned")).unwrap();
        fs::write(
            source.join("assets/unassigned/notes.txt"),
            b"legacy author file",
        )
        .unwrap();
        let mut document = crate::game_templates::puzzle_document();
        let (_capability_sources, capability) = distribution(&document, DistributionProfile::Full);
        document.schema_version = 3;
        document.engine_lock = None;
        document.engine_profile = None;
        document.legacy_archive.clear();
        let opaque = opaque_resource_keys();
        document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .values_mut()
            .next()
            .unwrap()
            .objects[0]
            .components
            .insert("future.opaque".into(), opaque.clone());
        document
            .properties
            .inactive
            .global
            .insert("future.global".into(), opaque.clone());
        let mut value = serde_json::to_value(&document).unwrap();
        value["unknown_source_record"] = opaque.clone();
        let mut original = vec![0xef, 0xbb, 0xbf];
        original.extend(serde_json::to_vec_pretty(&value).unwrap());
        original.extend(b"\r\n");
        let json = source.join("project.json");
        fs::write(&json, &original).unwrap();
        let binding = project_binding_path(&json).unwrap();
        let original_binding = serde_json::to_vec_pretty(
            &ProjectEngineBinding::from_distribution(&capability).unwrap(),
        )
        .unwrap();
        fs::write(&binding, &original_binding).unwrap();
        let (migrated, report) = migrate_copy(
            &json,
            &root.join("migrated/project.json"),
            crate::engine_profiles::ProfileKind::Puzzle,
        )
        .unwrap();
        assert_eq!(report.source_schema, 3);
        assert_eq!(
            report.source_sha256,
            module_engine_project::hash_bytes(&original)
        );
        assert_eq!(
            migrated.document.legacy_archive["/unknown_source_record"],
            opaque
        );
        let game = migrated.document.game.as_ref().unwrap();
        assert_eq!(
            game.scenes.values().next().unwrap().objects[0].components["future.opaque"],
            opaque
        );
        assert_eq!(
            migrated.document.properties.inactive.global["future.global"],
            opaque
        );
        assert_eq!(
            crate::project::read(&migrated.path).unwrap(),
            migrated.document
        );
        let rebound = distribution::detect(&migrated.path, &migrated.document, None).unwrap();
        let saved = save_as(
            &migrated.path,
            &root.join("saved-again/project.json"),
            &migrated.document,
            &rebound,
        )
        .unwrap();
        assert_eq!(
            crate::project::read(&saved.path).unwrap(),
            migrated.document
        );
        assert_eq!(fs::read(&json).unwrap(), original);
        assert_eq!(fs::read(&binding).unwrap(), original_binding);
        for target in [&migrated.path, &saved.path] {
            assert_eq!(
                fs::read(target.parent().unwrap().join("assets/unassigned/notes.txt")).unwrap(),
                b"legacy author file"
            );
        }
        cleanup(&root);
    }
    #[cfg(not(windows))]
    #[test]
    fn migration_copy_accepts_case_distinct_filename_and_preserves_legacy_source() {
        let root = fixture();
        let source = root.join("legacy");
        fs::create_dir_all(source.join("assets/unassigned")).unwrap();
        let resource = source.join("assets/unassigned/notes.bin");
        let resource_bytes = b"legacy author resource\0\xff";
        fs::write(&resource, resource_bytes).unwrap();
        let mut document = crate::game_templates::puzzle_document();
        let (_capability_sources, capability) = distribution(&document, DistributionProfile::Full);
        document.schema_version = 3;
        document.engine_lock = None;
        document.engine_profile = None;
        document.legacy_archive.clear();
        let opaque = serde_json::json!({"literal.key":null,"nested":[false,{"0":"opaque"}]});
        document
            .properties
            .inactive
            .global
            .insert("future.global".into(), opaque.clone());
        let mut value = serde_json::to_value(&document).unwrap();
        value["unknown_source_record"] = opaque.clone();
        let mut original = vec![0xef, 0xbb, 0xbf];
        original.extend(serde_json::to_vec_pretty(&value).unwrap());
        original.extend(b"\r\n");
        let json = source.join("project.json");
        fs::write(&json, &original).unwrap();
        let binding = project_binding_path(&json).unwrap();
        let original_binding = serde_json::to_vec_pretty(
            &ProjectEngineBinding::from_distribution(&capability).unwrap(),
        )
        .unwrap();
        fs::write(&binding, &original_binding).unwrap();
        let case_distinct = source.join("PROJECT.json");
        assert!(
            !case_distinct.exists(),
            "Case-distinct migration requires a case-sensitive fixture filesystem"
        );
        let assert_source_preserved = || {
            assert_eq!(fs::read(&json).unwrap(), original);
            assert_eq!(fs::read(&binding).unwrap(), original_binding);
            assert_eq!(fs::read(&resource).unwrap(), resource_bytes);
        };

        let kind = crate::engine_profiles::ProfileKind::Puzzle;
        let (control, _) = migrate_copy(&json, &source.join("migrated.json"), kind).unwrap();
        assert_eq!(
            crate::project::read(&control.path).unwrap(),
            control.document
        );
        assert_eq!(
            control.document.legacy_archive["/unknown_source_record"],
            opaque
        );
        assert_source_preserved();
        let same_file = migrate_copy(&json, &json, kind).unwrap_err();
        assert!(same_file.contains("source filename cannot be its destination"));
        assert_source_preserved();

        let result = migrate_copy(&json, &case_distinct, kind);
        assert_source_preserved();
        let (migrated, report) = result
            .expect("A distinct case-sensitive filename must be accepted as a migration copy");
        assert_eq!(migrated.path, case_distinct);
        assert_eq!(report.source_schema, 3);
        assert_eq!(
            report.source_sha256,
            module_engine_project::hash_bytes(&original)
        );
        assert_eq!(
            crate::project::read(&migrated.path).unwrap(),
            migrated.document
        );
        assert_eq!(migrated.document, control.document);
        assert_eq!(
            migrated.document.properties.inactive.global["future.global"],
            opaque
        );
        assert_eq!(
            fs::read(project_binding_path(&migrated.path).unwrap()).unwrap(),
            original_binding
        );
        assert_source_preserved();
        cleanup(&root);
    }
    #[test]
    fn save_as_preserves_author_archive_unused_files_and_transitive_prefab_assets() {
        let root = fixture();
        let source = root.join("original");
        let target = root.join("copied");
        fs::create_dir_all(source.join("assets/unassigned")).unwrap();
        fs::write(
            source.join("assets/unassigned/later.txt"),
            b"unassigned author file",
        )
        .unwrap();
        model_files(&source);
        fs::create_dir_all(source.join("resources/shader/parts")).unwrap();
        fs::write(
            source.join("resources/shader/filter.hlsl"),
            "#include \"parts/common.hlsli\"\n",
        )
        .unwrap();
        fs::write(
            source.join("resources/shader/parts/common.hlsli"),
            "float value() { return 1.0; }\n",
        )
        .unwrap();
        fs::write(source.join("resources/registered.bin"), [2, 4, 8]).unwrap();
        let mut document = document();
        let mut manifest = AssetManifest::default();
        manifest
            .register("registered unused", "resources/registered.bin", &source)
            .unwrap();
        manifest
            .register("unused shader", "resources/shader/filter.hlsl", &source)
            .unwrap();
        document.asset_manifest = serde_json::to_value(&manifest).unwrap();
        document.game.as_mut().unwrap().prefabs.insert(
            "unused visual".into(),
            module_engine_core::Prefab {
                color: [1, 2, 3, 255],
                components: BTreeMap::from([(
                    "mesh".into(),
                    serde_json::json!({"path":"resources/model/model.gltf"}),
                )]),
            },
        );
        document.properties.inactive.global.insert(
            "disabled future value".into(),
            serde_json::json!({"unchanged":[1,2,3]}),
        );
        document
            .custom_sources
            .insert("unused-text".into(), "// preserve even unattached\n".into());
        fs::write(
            source.join("project.json"),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
        let original = fs::read(source.join("project.json")).unwrap();
        let (_distribution_sources, distribution) =
            distribution(&document, DistributionProfile::Full);
        let saved = save_as(
            &source.join("project.json"),
            &target.join("project.json"),
            &document,
            &distribution,
        )
        .unwrap();
        assert_eq!(saved.document, document);
        assert_eq!(crate::project::read(&saved.path).unwrap(), document);
        assert_eq!(fs::read(source.join("project.json")).unwrap(), original);
        assert_eq!(
            fs::read(target.join("assets/unassigned/later.txt")).unwrap(),
            b"unassigned author file"
        );
        for name in ["model.gltf", "model.bin", "checker.png"] {
            assert_eq!(
                fs::read(source.join("resources/model").join(name)).unwrap(),
                fs::read(target.join("resources/model").join(name)).unwrap()
            );
        }
        for relative in [
            "resources/registered.bin",
            "resources/shader/filter.hlsl",
            "resources/shader/parts/common.hlsli",
        ] {
            assert_eq!(
                fs::read(source.join(relative)).unwrap(),
                fs::read(target.join(relative)).unwrap()
            );
        }
        let binding: ProjectEngineBinding =
            serde_json::from_slice(&fs::read(project_binding_path(&saved.path).unwrap()).unwrap())
                .unwrap();
        assert_eq!(
            binding.package_root,
            distribution.package_root.canonicalize().unwrap()
        );
        assert_eq!(
            distribution::detect(&saved.path, &document, None)
                .unwrap()
                .profile,
            DistributionProfile::Full
        );
        cleanup(&root);
    }
    #[test]
    fn save_as_refuses_conflicts_and_nested_copies_without_publishing_or_mutation() {
        let root = fixture();
        let source = root.join("source");
        let target = root.join("destination");
        fs::create_dir_all(source.join("assets")).unwrap();
        fs::create_dir_all(target.join("assets")).unwrap();
        fs::write(source.join("assets/file.txt"), b"original").unwrap();
        fs::write(target.join("assets/file.txt"), b"different").unwrap();
        let document = document();
        let before = document.clone();
        let (_distribution_sources, distribution) =
            distribution(&document, DistributionProfile::Full);
        let result = save_as(
            &source.join("project.json"),
            &target.join("project.json"),
            &document,
            &distribution,
        );
        assert!(result.unwrap_err().contains("different resource"));
        assert_eq!(document, before);
        assert!(!target.join("project.json").exists());
        assert!(
            !project_binding_path(&target.join("project.json"))
                .unwrap()
                .exists()
        );
        assert_eq!(
            fs::read(target.join("assets/file.txt")).unwrap(),
            b"different"
        );
        assert!(
            save_as(
                &source.join("project.json"),
                &source.join("assets/copy/project.json"),
                &document,
                &distribution
            )
            .is_err()
        );
        assert!(!source.join("assets/copy").exists());
        fs::create_dir_all(source.join("resources")).unwrap();
        fs::write(source.join("resources/data.bin"), [3, 6, 9]).unwrap();
        let mut with_resource = document.clone();
        let mut manifest = AssetManifest::default();
        manifest
            .register("declared", "resources/data.bin", &source)
            .unwrap();
        with_resource.asset_manifest = serde_json::to_value(manifest).unwrap();
        assert!(
            save_as(
                &source.join("project.json"),
                &source.join("resources/copy/project.json"),
                &with_resource,
                &distribution
            )
            .is_err()
        );
        assert!(!source.join("resources/copy").exists());
        assert!(!fs::read_dir(&root).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".module-engine-save-")
        }));
        cleanup(&root);
    }
    #[test]
    fn projects_and_outputs_inside_the_installation_root_are_separate_from_copied_assets() {
        let root = fixture();
        let installation = root.join("installation");
        fs::create_dir_all(installation.join("assets/unassigned")).unwrap();
        fs::write(
            installation.join("assets/unassigned/notes.txt"),
            b"retain imported author content",
        )
        .unwrap();
        let document = document();
        let (_distribution_sources, distribution) =
            distribution(&document, DistributionProfile::Full);
        let source = installation.join("game-project.json");
        let original_bytes = serde_json::to_vec_pretty(&document).unwrap();
        fs::write(&source, &original_bytes).unwrap();
        let saved = new_project(
            &source,
            &installation.join("projects/new-game"),
            &document,
            &distribution,
            ProjectTemplate::Empty,
        )
        .unwrap();
        assert!(saved.path.is_file());
        assert!(
            !installation.join("projects/new-game/assets").exists(),
            "Empty does not inherit previous project resources"
        );
        assert_eq!(fs::read(&source).unwrap(), original_bytes);
        let copy = save_as(
            &source,
            &installation.join("outputs/copy/project.json"),
            &document,
            &distribution,
        )
        .unwrap();
        assert_eq!(crate::project::read(&copy.path).unwrap(), document);
        assert_eq!(fs::read(&source).unwrap(), original_bytes);
        cleanup(&root);
    }
    #[test]
    fn same_folder_save_as_shares_resources_and_keeps_original_bytes() {
        let root = fixture();
        let document = document();
        fs::write(root.join("old.json"), b"old authored bytes").unwrap();
        let (_distribution_sources, distribution) =
            distribution(&document, DistributionProfile::Full);
        let saved = save_as(
            &root.join("old.json"),
            &root.join("new.json"),
            &document,
            &distribution,
        )
        .unwrap();
        assert_eq!(
            fs::read(root.join("old.json")).unwrap(),
            b"old authored bytes"
        );
        assert_eq!(crate::project::read(&saved.path).unwrap(), document);
        assert!(
            save_as(
                &root.join("old.json"),
                &root.join("new.json"),
                &document,
                &distribution
            )
            .is_err()
        );
        cleanup(&root);
    }
    #[test]
    fn save_as_keeps_an_in_progress_disconnected_author_graph() {
        let root = fixture();
        let original = document();
        let (_distribution_sources, distribution) =
            distribution(&original, DistributionProfile::Full);
        let source = root.join("original.json");
        let original_bytes = serde_json::to_vec_pretty(&original).unwrap();
        fs::write(&source, &original_bytes).unwrap();
        let mut draft = original;
        draft
            .root
            .edges
            .retain(|edge| !(edge.to_node == "renderer" && edge.to_port == "scene"));
        assert!(
            module_engine_core::validate_graph(&draft, &crate::host::registry().unwrap())
                .iter()
                .any(|diagnostic| diagnostic.severity == module_engine_core::Severity::Error)
        );
        let saved = save_as(&source, &root.join("draft.json"), &draft, &distribution).unwrap();
        assert_eq!(crate::project::read(&saved.path).unwrap(), draft);
        assert_eq!(fs::read(&source).unwrap(), original_bytes);
        cleanup(&root);
    }
    #[test]
    fn external_project_retains_installed_inline_includes_and_copies_author_hlsl_dependencies() {
        let root = fixture();
        let source = root.join("external project");
        fs::create_dir_all(source.join("assets/gpu")).unwrap();
        fs::create_dir_all(source.join("resources")).unwrap();
        fs::write(
            source.join("assets/gpu/fullscreen.hlsl"),
            b"// project-owned shadow, not Engine include\n",
        )
        .unwrap();
        fs::write(
            source.join("resources/author.hlsl"),
            "#include \"local.hlsli\"\n",
        )
        .unwrap();
        fs::write(
            source.join("resources/local.hlsli"),
            "float authored() { return 2.; }\n",
        )
        .unwrap();
        let mut current = document();
        current.shader_sources.insert(
            "ownership-regression".into(),
            "#include <fullscreen.hlsl>\n// installed Engine include\n".into(),
        );
        current.shader_sources.insert(
            "mesh.path".into(),
            "// opaque shader ID; this is not an asset path\n".into(),
        );
        current.custom_sources.insert(
            "base_color_texture".into(),
            "// opaque Rust ID; this is not an asset path\n".into(),
        );
        let mut manifest = AssetManifest::default();
        manifest
            .register("author shader", "resources/author.hlsl", &source)
            .unwrap();
        current.asset_manifest = serde_json::to_value(manifest).unwrap();
        let json = source.join("project.json");
        fs::write(&json, serde_json::to_vec_pretty(&current).unwrap()).unwrap();
        for profile in [DistributionProfile::Full, DistributionProfile::Team] {
            let (_dist_sources, dist) = distribution(&current, profile);
            let dependencies = module_engine_project::collect_hlsl_source_dependencies(
                &dist.package_root,
                "__inline__.hlsl",
                &current.shader_sources["ownership-regression"],
            )
            .unwrap();
            assert_eq!(dependencies, vec!["assets/gpu/fullscreen.hlsl"]);
            let name = if profile == DistributionProfile::Full {
                "full copy"
            } else {
                "team copy"
            };
            let saved = save_as(
                &json,
                &root.join(name).join("project.json"),
                &current,
                &dist,
            )
            .unwrap();
            let rebound = distribution::detect(&saved.path, &saved.document, None).unwrap();
            assert_eq!(rebound.profile, profile);
            assert_eq!(saved.document.shader_sources, current.shader_sources);
            let copied_root = saved.path.parent().unwrap();
            assert_eq!(
                fs::read(copied_root.join("resources/local.hlsli")).unwrap(),
                fs::read(source.join("resources/local.hlsli")).unwrap()
            );
            for relative in dependencies {
                assert_eq!(
                    hash_file(&rebound.package_root.join(&relative)).unwrap(),
                    hash_file(&dist.package_root.join(&relative)).unwrap()
                );
                assert_ne!(
                    hash_file(&rebound.package_root.join(&relative)).unwrap(),
                    hash_file(&copied_root.join(&relative)).unwrap()
                );
            }
        }
        cleanup(&root);
    }
    #[test]
    fn confirmed_overwrite_keeps_exact_metadata_backups_and_rejects_resource_conflicts_first() {
        let root = fixture();
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(source.join("assets")).unwrap();
        fs::create_dir_all(target.join("assets")).unwrap();
        fs::write(source.join("assets/item.txt"), b"same resource").unwrap();
        fs::write(target.join("assets/item.txt"), b"same resource").unwrap();
        fs::write(source.join("source.json"), b"original source bytes").unwrap();
        let document = document();
        let (_full_sources, full) = distribution(&document, DistributionProfile::Full);
        let (_team_sources, team) = distribution(&document, DistributionProfile::Team);
        let path = target.join("project.json");
        let old_json = b"previous project exact bytes\r\n";
        fs::write(&path, old_json).unwrap();
        let sidecar = project_binding_path(&path).unwrap();
        let old_binding =
            serde_json::to_vec_pretty(&ProjectEngineBinding::from_distribution(&team).unwrap())
                .unwrap();
        fs::write(&sidecar, &old_binding).unwrap();
        assert!(save_as(&source.join("source.json"), &path, &document, &full).is_err());
        let saved =
            save_as_confirmed(&source.join("source.json"), &path, &document, &full).unwrap();
        assert_eq!(crate::project::read(&saved.path).unwrap(), document);
        assert_eq!(
            fs::read(source.join("source.json")).unwrap(),
            b"original source bytes"
        );
        let backups = fs::read_dir(&target)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".backup-")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 2);
        assert!(backups.iter().any(|p| fs::read(p).unwrap() == old_json));
        assert!(backups.iter().any(|p| fs::read(p).unwrap() == old_binding));
        assert_eq!(
            distribution::detect(&path, &document, None)
                .unwrap()
                .profile,
            DistributionProfile::Full
        );
        fs::write(
            target.join("assets/item.txt"),
            b"different destination resource",
        )
        .unwrap();
        let metadata = fs::read(&path).unwrap();
        assert!(
            save_as_confirmed(&source.join("source.json"), &path, &document, &full)
                .unwrap_err()
                .contains("different resource")
        );
        assert_eq!(fs::read(&path).unwrap(), metadata);
        assert_eq!(
            fs::read_dir(&target)
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".backup-"))
                .count(),
            2
        );
        cleanup(&root);
    }
    #[test]
    fn templates_keep_engine_and_custom_ids_and_team_baseline_on_new_project() {
        let root = fixture();
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        let mut current = document();
        current.custom_sources.insert(
            "engine-source".into(),
            "fn value(_ctx: &ScriptContext) -> f64 { 7.0 }".into(),
        );
        let (full_sources, full) = distribution(&current, DistributionProfile::Full);
        // Two Arenas reads its real template resources from the installed package.
        // Keep this content fixture local: other tests deliberately replace assets.
        let template_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut template_assets = AssetManifest::default();
        for relative in [
            "assets/models/demo_crystal.glb",
            "assets/audio/attack.wav",
            "assets/audio/hit.wav",
            "assets/audio/death.wav",
            "assets/audio/victory.wav",
        ] {
            template_assets
                .register(relative, relative, &template_root)
                .unwrap();
        }
        template_assets.verify(&template_root).unwrap();
        let exported = module_engine_project::export_assets(
            &template_root,
            &template_assets,
            full_sources.path(),
        )
        .unwrap();
        assert_eq!(exported.len(), template_assets.assets.len());
        template_assets.verify(full_sources.path()).unwrap();
        let (_team_sources, team) = distribution(&current, DistributionProfile::Team);
        for template in [ProjectTemplate::Empty, ProjectTemplate::TwoArenas] {
            let new = template_document(&current, template, &team).unwrap();
            assert_eq!(new.engine, current.engine);
            assert_eq!(new.root, current.root);
            assert_eq!(new.module_versions, current.module_versions);
            assert_eq!(new.shader_sources, current.shader_sources);
            assert_eq!(
                new.game.as_ref().unwrap().execution,
                current.game.as_ref().unwrap().execution
            );
            assert_eq!(
                new.custom_sources.keys().collect::<Vec<_>>(),
                current.custom_sources.keys().collect::<Vec<_>>()
            );
            assert_eq!(
                new.custom_sources["engine-source"],
                current.custom_sources["engine-source"]
            );
            assert!(new.custom_sources["game-controller"].contains("fn update(_ctx"));
            team.validate_document(&new).unwrap();
        }
        let saved = new_project(
            &source.join("project.json"),
            &root.join("empty"),
            &current,
            &team,
            ProjectTemplate::Empty,
        )
        .unwrap();
        assert!(
            saved.document.game.as_ref().unwrap().scenes["scene"]
                .objects
                .is_empty()
        );
        let bound = distribution::detect(&saved.path, &saved.document, None).unwrap();
        assert_eq!(bound.profile, DistributionProfile::Team);
        assert!(!bound.can_author_engine());
        assert!(
            distribution::detect(
                &saved.path,
                &saved.document,
                Some(DistributionProfile::Full)
            )
            .is_err()
        );
        let mut illegal = saved.document.clone();
        illegal.root.nodes[0].module = "world.objects".into();
        assert!(distribution::detect(&saved.path, &illegal, None).is_err());
        let saved = new_project(
            &source.join("project.json"),
            &root.join("arenas"),
            &current,
            &full,
            ProjectTemplate::TwoArenas,
        )
        .unwrap();
        template_assets
            .verify(saved.path.parent().unwrap())
            .unwrap();
        assert_eq!(saved.document.game.unwrap().scenes.len(), 2);
        cleanup(&root);
    }

    #[test]
    fn new_empty_resets_connected_root_project_logic_and_preserves_team_lock_and_engine_sources() {
        let mut current = document();
        current.root.nodes.push(module_engine_core::make_node(
            "root-project-controller",
            "custom.rust.update",
            "Project update",
            [0., 0.],
            serde_json::json!({"source_id":"root-project-controller"}),
        ));
        current.root.edges.push(module_engine_core::make_edge(
            "root-project-controller",
            "world",
            "world",
            "commands",
        ));
        let project_text = "fn update(ctx: &mut ScriptContext) { ctx.set_color(1, [9,8,7,255]); }";
        current
            .custom_sources
            .insert("root-project-controller".into(), project_text.into());
        current.custom_sources.insert(
            "protected-unused-engine-source".into(),
            "// Engine-owned authoring source, retained even without a connected node".into(),
        );
        let profile = module_engine_project::EngineProfile::from_document(&current)
            .unwrap()
            .identity;
        for kind in [DistributionProfile::Full, DistributionProfile::Team] {
            let (_capability_sources, capability) = distribution(&current, kind);
            assert!(capability.can_edit_custom_source("root-project-controller", &current));
            assert!(!capability.can_edit_custom_source("protected-unused-engine-source", &current));
            let fresh = template_document(&current, ProjectTemplate::Empty, &capability).unwrap();
            assert_eq!(fresh.root, current.root);
            assert_eq!(
                fresh.custom_sources["root-project-controller"],
                "fn update(_ctx: &mut ScriptContext) {}\n"
            );
            assert_ne!(
                fresh.custom_sources["root-project-controller"],
                project_text
            );
            assert_eq!(
                module_engine_project::EngineProfile::from_document(&fresh)
                    .unwrap()
                    .identity,
                profile
            );
            if kind == DistributionProfile::Team {
                assert_eq!(fresh.module_versions, current.module_versions);
                assert_eq!(fresh.engine_lock, current.engine_lock);
                assert_eq!(
                    fresh.custom_sources.keys().collect::<Vec<_>>(),
                    current.custom_sources.keys().collect::<Vec<_>>()
                );
                assert_eq!(
                    fresh.custom_sources["protected-unused-engine-source"],
                    current.custom_sources["protected-unused-engine-source"]
                );
                assert_eq!(
                    fresh.game.as_ref().unwrap().execution,
                    current.game.as_ref().unwrap().execution
                );
                capability.validate_document(&fresh).unwrap();
            } else {
                assert!(fresh.game.as_ref().unwrap().execution.stages.is_empty());
                assert!(!fresh.custom_sources.contains_key("game-controller"));
            }
        }
        assert_eq!(
            current.custom_sources["root-project-controller"],
            project_text
        );
    }

    #[test]
    fn two_arenas_can_replace_project_controller_referenced_by_root_but_rejects_engine_owned_id() {
        let mut shared = document();
        shared.root.nodes.push(module_engine_core::make_node(
            "root-project-controller",
            "custom.rust.update",
            "Project update",
            [0., 0.],
            serde_json::json!({"source_id":"game-controller"}),
        ));
        shared.root.edges.push(module_engine_core::make_edge(
            "root-project-controller",
            "world",
            "world",
            "commands",
        ));
        shared.custom_sources.insert(
            "game-controller".into(),
            "fn update(ctx: &mut ScriptContext) { ctx.set_color(1,[9,8,7,255]); }".into(),
        );
        let (_full_sources, full) = distribution(&shared, DistributionProfile::Full);
        assert!(full.can_edit_custom_source("game-controller", &shared));
        let fresh = template_document(&shared, ProjectTemplate::TwoArenas, &full).unwrap();
        assert_eq!(fresh.root, shared.root);
        assert!(fresh.custom_sources["game-controller"].contains("game.counter.v1"));
        assert_ne!(
            fresh.custom_sources["game-controller"],
            shared.custom_sources["game-controller"]
        );

        let mut protected = document();
        protected
            .game
            .as_mut()
            .unwrap()
            .execution
            .stages
            .retain(|stage| {
                stage
                    .nodes
                    .iter()
                    .all(|node| node.module != "custom.rust.update")
            });
        protected.root.nodes.push(module_engine_core::make_node(
            "engine-speed",
            "custom.rust",
            "Engine speed",
            [0., 0.],
            serde_json::json!({"source_id":"game-controller"}),
        ));
        protected.root.edges.push(module_engine_core::make_edge(
            "engine-speed",
            "value",
            "world",
            "speed",
        ));
        protected.custom_sources.insert(
            "game-controller".into(),
            "fn speed(_time:f64,base:f64)->f64 { base }".into(),
        );
        let composition =
            crate::host::composition_at(&module_engine_vulkan::resource_root()).unwrap();
        let identity = module_engine_project::EngineProfile::from_document(&protected)
            .unwrap()
            .identity;
        composition
            .capture_lock(
                &protected,
                identity,
                Some(module_engine_core::SdkIdentity::current()),
            )
            .unwrap()
            .apply_to(&mut protected);
        let (_full_sources, full) = distribution(&protected, DistributionProfile::Full);
        assert!(!full.can_edit_custom_source("game-controller", &protected));
        let before = protected.clone();
        assert!(
            template_document(&protected, ProjectTemplate::TwoArenas, &full)
                .unwrap_err()
                .contains("Engine-owned source")
        );
        assert_eq!(protected, before);
    }
    #[test]
    fn save_as_full_and_team_preserve_ids_and_physical_resources_from_archived_values() {
        let root = fixture();
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        model_files(&source);
        let mut document = document();
        let mut manifest = AssetManifest::default();
        manifest
            .register("stable-model", "resources/model/model.gltf", &source)
            .unwrap();
        manifest
            .register("stable-texture", "resources/model/checker.png", &source)
            .unwrap();
        document.asset_manifest = serde_json::to_value(&manifest).unwrap();
        document
            .game
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
        document.properties.inactive.global.insert(
            "future material".into(),
            serde_json::json!({"base_color_texture":"asset://stable-texture"}),
        );
        fs::write(
            source.join("project.json"),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
        for (name, profile) in [
            ("full", DistributionProfile::Full),
            ("team", DistributionProfile::Team),
        ] {
            let (_context_sources, context) = distribution(&document, profile);
            let saved = save_as(
                &source.join("project.json"),
                &root.join(name).join("project.json"),
                &document,
                &context,
            )
            .unwrap();
            assert_eq!(saved.document, document);
            let target = saved.path.parent().unwrap();
            for file in ["model.gltf", "model.bin", "checker.png"] {
                assert_eq!(
                    fs::read(source.join("resources/model").join(file)).unwrap(),
                    fs::read(target.join("resources/model").join(file)).unwrap()
                );
            }
            let mut content = module_engine_content::ContentLibrary::new(target).unwrap();
            content.set_asset_paths(manifest.asset_paths());
            assert!(
                !content
                    .load_model("asset://stable-model")
                    .unwrap()
                    .primitives
                    .is_empty()
            );
            assert_eq!(
                content
                    .load_texture("asset://stable-texture")
                    .unwrap()
                    .width,
                8
            );
            assert_eq!(
                distribution::detect(&saved.path, &saved.document, None)
                    .unwrap()
                    .profile,
                profile
            );
        }
        cleanup(&root);
    }

    #[test]
    fn team_puzzle_new_empty_rejects_before_publication_and_preserves_existing_project() {
        let artifacts = module_engine_script::TestArtifactGuard::new(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.work/test-temp"),
            "team-puzzle-new-empty",
        )
        .unwrap();
        let source = artifacts.path().join("existing-team");
        fs::create_dir_all(&source).unwrap();
        let document = crate::game_host::puzzle_document();
        let before = document.clone();
        let (_team_sources, team) = distribution(&document, DistributionProfile::Team);
        let path = source.join("project.json");
        let raw = serde_json::to_vec_pretty(&document).unwrap();
        fs::write(&path, &raw).unwrap();
        let binding_path = project_binding_path(&path).unwrap();
        let binding =
            serde_json::to_vec_pretty(&ProjectEngineBinding::from_distribution(&team).unwrap())
                .unwrap();
        fs::write(&binding_path, &binding).unwrap();
        let composition = crate::host::composition_at(&team.package_root).unwrap();
        composition.validate_game(&document).unwrap();

        // Both a new folder and an already-created empty folder must stay
        // unpublished when the frozen schedule requires Puzzle scene content.
        for existing_folder in [false, true] {
            let destination = artifacts.path().join(if existing_folder {
                "empty-destination"
            } else {
                "absent-destination"
            });
            if existing_folder {
                fs::create_dir(&destination).unwrap();
            }
            let error = new_project(
                &path,
                &destination,
                &document,
                &team,
                ProjectTemplate::Empty,
            )
            .unwrap_err();
            assert!(error.contains("frozen Engine contract"), "{error}");
            assert!(
                error.contains("Puzzle scene needs exactly one board"),
                "{error}"
            );
            assert!(!destination.join("project.json").exists());
            assert!(
                !project_binding_path(&destination.join("project.json"))
                    .unwrap()
                    .exists()
            );
            if existing_folder {
                assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
            } else {
                assert!(!destination.exists());
            }
        }

        assert_eq!(document, before);
        assert_eq!(fs::read(&path).unwrap(), raw);
        assert_eq!(fs::read(&binding_path).unwrap(), binding);
        team.validate_document(&document).unwrap();
        composition.validate_game(&document).unwrap();
        assert_eq!(
            distribution::detect(&path, &document, None)
                .unwrap()
                .profile,
            DistributionProfile::Team
        );
    }
}
