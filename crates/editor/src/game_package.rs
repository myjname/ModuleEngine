//! Standalone game export: runtime executable, authored data, assets and compiled code.
use crate::game_host::{GameHost, render_graph, script_ids};
use module_engine_content::ContentLibrary;
use module_engine_core::{GraphDocument, WorldKind};
use module_engine_project::{AssetManifest, hash_file, resolve_relative};
use module_engine_script::{ScriptCompiler, ScriptKind};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerManifest {
    pub schema_version: u32,
    pub engine_version: String,
    pub project: String,
    pub files: BTreeMap<String, String>,
}

pub fn export(
    document: &GraphDocument,
    asset_root: &Path,
    backend_root: &Path,
    compile_root: &Path,
    destination: &Path,
) -> Result<PathBuf, String> {
    let absolute = if destination.is_absolute() {
        destination.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(destination)
    };
    let parent = absolute
        .parent()
        .ok_or("Player destination has no parent")?;
    let name = absolute
        .file_name()
        .ok_or("Player destination has no filename")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let parent = parent.canonicalize().map_err(|error| error.to_string())?;
    let destination = parent.join(name);
    let existing_empty = if destination.exists() {
        if fs::symlink_metadata(&destination)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
            || !destination.is_dir()
            || fs::read_dir(&destination)
                .map_err(|error| error.to_string())?
                .next()
                .is_some()
        {
            return Err("Game export needs a new or empty directory".into());
        }
        true
    } else {
        false
    };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let stage = parent.join(format!(
        ".module-engine-game-export-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&stage).map_err(|error| error.to_string())?;
    let stage = stage.canonicalize().map_err(|error| error.to_string())?;
    let result = export_into_staging(document, asset_root, backend_root, compile_root, &stage)
        .and_then(|_| {
            if existing_empty {
                // remove_dir refuses a racing writer's nonempty directory. It
                // never recursively removes a selected destination.
                fs::remove_dir(&destination).map_err(|error| {
                    format!("Player destination changed before publication: {error}")
                })?;
            }
            publish_new_directory(&stage, &destination)?;
            Ok(destination.clone())
        });
    // Only this operation's freshly created sibling stage may be cleaned up.
    // A failed export never leaves compiled workers/resources at the final path.
    if stage.exists() {
        if stage.parent() != Some(parent.as_path())
            || stage.canonicalize().ok().as_ref() != Some(&stage)
            || !stage.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with(".module-engine-game-export-")
            })
        {
            return Err("Player staging cleanup path failed ownership check".into());
        }
        if let Err(error) = fs::remove_dir_all(&stage) {
            return Err(format!(
                "{}; staging retained at {}: {error}",
                result
                    .as_ref()
                    .err()
                    .map_or("Player export cleanup failed", String::as_str),
                stage.display()
            ));
        }
    }
    if result.is_err() && existing_empty && !destination.exists() {
        // Restore an explicitly selected empty folder when publication failed;
        // create_dir does not replace a racing file or directory.
        let _ = fs::create_dir(&destination);
    }
    result
}

#[cfg(windows)]
fn publish_new_directory(stage: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source: Vec<_> = stage
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // Same-volume rename, write-through, and deliberately NO replace-existing.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0x0000_0008) } == 0 {
        return Err(format!(
            "Player destination was not published: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn publish_new_directory(stage: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err("Player destination appeared during export".into());
    }
    fs::rename(stage, destination).map_err(|error| error.to_string())
}

fn export_into_staging(
    document: &GraphDocument,
    asset_root: &Path,
    backend_root: &Path,
    compile_root: &Path,
    destination: &Path,
) -> Result<PathBuf, String> {
    let mut prepared = document.clone();
    let registry = crate::game_host::game_registry(
        document,
        backend_root,
        &BTreeMap::new(),
        std::sync::Arc::new(std::sync::Mutex::new(
            module_engine_core::RenderOverrides::default(),
        )),
    )?;
    if let Some(game) = &document.game {
        prepared.game = Some(game.prepare_runtime(&registry)?);
    }
    let document = &prepared;
    let game = document
        .game
        .as_ref()
        .ok_or("Project has no authored game")?;
    game.validate_scene_data()?;
    let asset_root = asset_root.canonicalize().map_err(|e| e.to_string())?;
    let backend_root = backend_root.canonicalize().map_err(|e| e.to_string())?;
    // Validate the complete plan before creating an export directory. Assets
    // belong to the selected project; immutable GPU sources belong to backend.
    let files = prepare_files(document, game, &asset_root, &backend_root)?;
    let destination = if destination.is_absolute() {
        destination.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(destination)
    };
    if destination.exists()
        && (!destination.is_dir()
            || fs::read_dir(&destination)
                .map_err(|e| e.to_string())?
                .next()
                .is_some())
    {
        return Err("Game export needs a new or empty directory".into());
    }
    fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
    let destination = destination.canonicalize().map_err(|e| e.to_string())?;
    if asset_root.starts_with(&destination) || backend_root.starts_with(&destination) {
        return Err("Export destination must not contain the source project".into());
    }
    for file in files.values() {
        let target = destination.join(&file.relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut input = fs::File::open(&file.source).map_err(|e| e.to_string())?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
        drop(output);
        if hash_file(&destination.join(&file.relative))? != file.hash {
            return Err(format!(
                "Player resource changed during export: {}",
                file.relative
            ));
        }
    }
    let compiler = ScriptCompiler::new(
        compile_root.to_path_buf(),
        compile_root.join(".work/game-export-scripts"),
    );
    for id in script_ids(game) {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Game script IDs need letters, digits, underscore or dash".into());
        }
        let source = document
            .custom_sources
            .get(&id)
            .ok_or_else(|| format!("Missing Custom source '{id}'"))?;
        compiler
            .compile_kind(source, ScriptKind::Update)
            .map_err(|e| e.to_string())?
            .export_precompiled(&destination.join("scripts").join(id))
            .map_err(|e| e.to_string())?;
    }
    module_engine_vulkan::precompile_graph_at(
        &render_graph(document),
        &document.shader_sources,
        &backend_root,
        &destination,
    )?;
    fs::copy(
        std::env::current_exe().map_err(|e| e.to_string())?,
        destination.join("Player.exe"),
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        destination.join("project.json"),
        serde_json::to_vec_pretty(document).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(destination.join("README.txt"),"ModuleEngine game\r\nLaunch Player.exe. Rust, Cargo, DXC and editor sources are not required.\r\nWASD: move. Left mouse: melee towards cursor. R: restart. Esc: close.\r\nDefeat all small monsters, enter the green passage, defeat the boss.\r\nGPU with Vulkan 1.3 support and an installed driver is required.\r\n").map_err(|e|e.to_string())?;
    let mut files = BTreeSet::new();
    collect_files(&destination, &destination, &mut files)?;
    let hashes = files
        .into_iter()
        .map(|relative| {
            Ok((
                relative.clone(),
                hash_file(&resolve_relative(&destination, &relative)?)?,
            ))
        })
        .collect::<Result<_, String>>()?;
    let manifest = PlayerManifest {
        schema_version: 1,
        engine_version: env!("CARGO_PKG_VERSION").into(),
        project: "project.json".into(),
        files: hashes,
    };
    fs::write(
        destination.join("player.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // Read from the actual exported root, with no compile root and sealed GPU artifacts.
    validate_package(&destination)?;
    let mut host = GameHost::new(document.clone(), &destination, None, WorldKind::Objects)?;
    host.audio.enabled = false;
    host.render(320, 180)?;
    Ok(destination)
}

#[derive(Debug)]
struct PreparedFile {
    relative: String,
    source: PathBuf,
    hash: String,
}

fn script_directories(game: &module_engine_core::GameProject) -> Result<BTreeSet<String>, String> {
    let mut directories = BTreeMap::<String, String>::new();
    for id in script_ids(game) {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Game script IDs need letters, digits, underscore or dash".into());
        }
        let folded = id.to_ascii_lowercase();
        if let Some(previous) = directories.insert(folded, id.clone())
            && previous != id
        {
            return Err(format!(
                "Player script directories collide on Windows: '{previous}' and '{id}'"
            ));
        }
    }
    Ok(directories
        .into_keys()
        .map(|id| format!("scripts/{id}"))
        .collect())
}

fn reserved_player_path(relative: &str, scripts: &BTreeSet<String>) -> bool {
    let relative = relative.to_lowercase();
    let first = relative.split('/').next().unwrap_or("");
    if matches!(
        first,
        "player.exe"
            | "player.json"
            | "project.json"
            | "readme.txt"
            | ".module-engine-package.json"
            | ".spirv"
            | ".sdk"
            | ".work"
            | ".tools"
            | ".user"
            | ".git"
            | "target"
    ) {
        return true;
    }
    relative == "scripts"
        || scripts
            .iter()
            .any(|folder| relative == *folder || relative.starts_with(&format!("{folder}/")))
}

/// File identities are resolved before any destination is created. A selected
/// project resource root may differ from the native source/backend package.
fn prepare_files(
    document: &GraphDocument,
    game: &module_engine_core::GameProject,
    asset_root: &Path,
    backend_root: &Path,
) -> Result<BTreeMap<String, PreparedFile>, String> {
    let scripts = script_directories(game)?;
    let manifest = AssetManifest::from_value(&document.asset_manifest)?;
    let mut library = ContentLibrary::new(asset_root)?;
    library.set_asset_paths(manifest.asset_paths());
    let mut models = vec![];
    let mut assets = BTreeSet::new();
    for scene in game.scenes.keys() {
        for entity in game.resolve_scene(scene)? {
            if let Some(path) = entity
                .components
                .get("mesh")
                .and_then(|m| m.get("path"))
                .and_then(|v| v.as_str())
            {
                models.push(path.to_owned());
            }
            if let Some(path) = entity
                .components
                .get("material")
                .and_then(|m| m.get("base_color_texture"))
                .and_then(|v| v.as_str())
            {
                library.load_texture(path)?;
                assets.insert(manifest.resolve_reference(path)?);
            }
        }
    }
    assets.extend(library.dependencies(models)?);
    for path in game.audio_cues.values() {
        assets.insert(manifest.resolve_reference(path)?);
    }
    manifest.verify(asset_root)?;
    assets.extend(manifest.assets.values().map(|e| e.path.clone()));
    let mut backend = BTreeSet::new();
    collect_files(backend_root, &backend_root.join("assets/gpu"), &mut backend)?;
    for source in document.shader_sources.values() {
        backend.extend(module_engine_project::collect_hlsl_source_dependencies(
            backend_root,
            "__inline__.hlsl",
            source,
        )?);
    }
    let mut files = BTreeMap::<String, PreparedFile>::new();
    for (root, paths) in [(asset_root, assets), (backend_root, backend)] {
        for path in paths {
            let relative = module_engine_project::normalize_relative(&path)?;
            if reserved_player_path(&relative, &scripts) {
                return Err(format!(
                    "Player generated/control paths cannot be exported as assets: {relative}"
                ));
            }
            let source = resolve_relative(root, &relative)?;
            let hash = hash_file(&source)?;
            let folded = relative.to_lowercase();
            if let Some(previous) = files.get(&folded) {
                if previous.relative != relative || previous.hash != hash {
                    return Err(format!(
                        "Player asset/backend paths or bytes collide: {} and {relative}",
                        previous.relative
                    ));
                }
                continue;
            }
            files.insert(
                folded,
                PreparedFile {
                    relative,
                    source,
                    hash,
                },
            );
        }
    }
    for path in files.keys() {
        let mut ancestors = path.split('/').collect::<Vec<_>>();
        ancestors.pop();
        while !ancestors.is_empty() {
            let directory = ancestors.join("/");
            if files.contains_key(&directory) {
                return Err(format!(
                    "Player export file conflicts with a directory: {directory}"
                ));
            }
            ancestors.pop();
        }
    }
    Ok(files)
}

pub fn validate_package(root: &Path) -> Result<GraphDocument, String> {
    let manifest: PlayerManifest =
        serde_json::from_slice(&fs::read(root.join("player.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if manifest.schema_version != 1 || manifest.engine_version != env!("CARGO_PKG_VERSION") {
        return Err("Player package version is incompatible".into());
    }
    for (relative, hash) in &manifest.files {
        if hash_file(&resolve_relative(root, relative)?)? != *hash {
            return Err(format!("Player file changed or corrupt: {relative}"));
        }
    }
    let document = serde_json::from_slice(
        &fs::read(resolve_relative(root, &manifest.project)?).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(document)
}
fn collect_files(
    root: &Path,
    directory: &Path,
    result: &mut BTreeSet<String>,
) -> Result<(), String> {
    if !directory.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            if path.is_symlink() {
                return Err("Export directories must not be symbolic links".into());
            }
            collect_files(root, &path, result)?;
        } else if path.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            resolve_relative(root, &relative)?;
            result.insert(relative);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    fn backend_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
    }
    fn fixture() -> PathBuf {
        let folder = backend_root().join(".work").join(format!(
            "player-plan-test-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&folder).unwrap();
        folder
    }
    fn no_resource_document() -> GraphDocument {
        let mut document = crate::game_host::demo_document();
        let game = document.game.as_mut().unwrap();
        game.audio_cues.clear();
        for scene in game.scenes.values_mut() {
            for entity in &mut scene.objects {
                entity.components.remove("mesh");
            }
        }
        document
    }

    #[test]
    fn player_failed_compilation_leaves_destination_retryable() {
        let backend = backend_root();
        let folder = fixture();
        let assets = folder.join("resources");
        fs::create_dir_all(assets.join("assets/data")).unwrap();
        fs::write(
            assets.join("assets/data/project.txt"),
            b"selected project data",
        )
        .unwrap();
        let mut document = no_resource_document();
        let mut manifest = AssetManifest::default();
        manifest
            .register("project-data", "assets/data/project.txt", &assets)
            .unwrap();
        document.asset_manifest = serde_json::to_value(manifest).unwrap();
        let mut invalid = document.clone();
        invalid.custom_sources.insert(
            "game-controller".into(),
            "fn update(ctx: &mut ScriptContext) { let broken = ; }".into(),
        );
        let destination = folder.join("player");
        fs::create_dir(&destination).unwrap();
        let error = export(&invalid, &assets, &backend, &backend, &destination).unwrap_err();
        assert!(
            error.contains("error") || error.contains("compil"),
            "{error}"
        );
        assert!(destination.is_dir());
        assert!(fs::read_dir(&destination).unwrap().next().is_none());
        assert!(!fs::read_dir(&folder).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".module-engine-game-export-")
        }));
        assert_eq!(
            fs::read(assets.join("assets/data/project.txt")).unwrap(),
            b"selected project data"
        );

        let exported = export(&document, &assets, &backend, &backend, &destination).unwrap();
        assert_eq!(exported, destination.canonicalize().unwrap());
        validate_package(&exported).unwrap();
        assert!(
            exported
                .join("scripts/game-controller/worker.exe")
                .is_file()
        );
        assert_eq!(
            fs::read(exported.join("assets/data/project.txt")).unwrap(),
            b"selected project data"
        );
    }

    #[test]
    fn player_publication_never_replaces_a_racing_destination() {
        let folder = fixture();
        let stage = folder.join("stage");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("new.txt"), "new package").unwrap();
        for filename in ["racing-file", "racing-directory"] {
            let destination = folder.join(filename);
            if filename.ends_with("directory") {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("existing.txt"), "other writer").unwrap();
            } else {
                fs::write(&destination, "other writer").unwrap();
            }
            assert!(publish_new_directory(&stage, &destination).is_err());
            assert_eq!(
                fs::read_to_string(stage.join("new.txt")).unwrap(),
                "new package"
            );
            let existing = if destination.is_dir() {
                destination.join("existing.txt")
            } else {
                destination
            };
            assert_eq!(fs::read_to_string(existing).unwrap(), "other writer");
        }
        let destination = folder.join("ready");
        publish_new_directory(&stage, &destination).unwrap();
        assert!(!stage.exists());
        assert_eq!(
            fs::read_to_string(destination.join("new.txt")).unwrap(),
            "new package"
        );
    }
    #[test]
    fn player_plan_resolves_model_texture_audio_ids_to_owned_physical_paths() {
        let backend = backend_root();
        let folder = fixture();
        let assets = folder.join("resources");
        fs::create_dir_all(assets.join("assets/renamed")).unwrap();
        let glb = include_bytes!("../../../assets/models/demo_crystal.glb");
        fs::write(assets.join("assets/renamed/model.glb"), glb).unwrap();
        let jsonlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json: serde_json::Value = serde_json::from_slice(&glb[20..20 + jsonlen]).unwrap();
        let binary = &glb[20 + jsonlen + 8..];
        let view = &json["bufferViews"][json["images"][0]["bufferView"].as_u64().unwrap() as usize];
        let offset = view["byteOffset"].as_u64().unwrap() as usize;
        let length = view["byteLength"].as_u64().unwrap() as usize;
        fs::write(
            assets.join("assets/renamed/image.png"),
            &binary[offset..offset + length],
        )
        .unwrap();
        fs::copy(
            backend.join("assets/audio/attack.wav"),
            assets.join("assets/renamed/attack.wav"),
        )
        .unwrap();
        let mut index = AssetManifest::default();
        for (id, path) in [
            ("model-id", "model.glb"),
            ("texture-id", "image.png"),
            ("audio-id", "attack.wav"),
        ] {
            index
                .register(id, &format!("assets/renamed/{path}"), &assets)
                .unwrap();
        }
        let mut doc = no_resource_document();
        doc.asset_manifest = serde_json::to_value(index).unwrap();
        let game = doc.game.as_mut().unwrap();
        let components = &mut game.scenes.get_mut("arena").unwrap().objects[0].components;
        components.insert(
            "mesh".into(),
            serde_json::json!({"path":"asset://model-id"}),
        );
        components.insert(
            "material".into(),
            serde_json::json!({"base_color_texture":"asset://texture-id"}),
        );
        game.audio_cues
            .insert("attack".into(), "asset://audio-id".into());
        let assets = assets.canonicalize().unwrap();
        let plan = prepare_files(&doc, doc.game.as_ref().unwrap(), &assets, &backend).unwrap();
        for filename in ["model.glb", "image.png", "attack.wav"] {
            assert!(
                plan[&format!("assets/renamed/{filename}")]
                    .source
                    .starts_with(&assets)
            );
        }
        assert!(plan.keys().all(|p| !p.contains("asset://")));
        assert!(plan["assets/gpu/forward.hlsl"].source.starts_with(&backend));
        doc.game
            .as_mut()
            .unwrap()
            .audio_cues
            .insert("attack".into(), "asset://missing".into());
        assert!(
            prepare_files(&doc, doc.game.as_ref().unwrap(), &assets, &backend)
                .unwrap_err()
                .contains("Unknown asset ID")
        );
    }

    #[test]
    fn generated_player_paths_are_rejected_before_creating_destination() {
        let backend = backend_root();
        for path in [
            "Player.EXE",
            "assets/../project.json",
            "player.json",
            "README.txt",
            ".spirv/cache.spv",
            "scripts/game-controller/worker.exe",
            ".module-engine-package.json",
        ] {
            let folder = fixture();
            let assets = folder.join("resources");
            fs::create_dir_all(&assets).unwrap();
            let normalized = module_engine_project::normalize_relative(path).unwrap();
            let source = assets.join(&normalized);
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::write(&source, "must not be overwritten").unwrap();
            let mut manifest = AssetManifest::default();
            manifest.register("reserved", path, &assets).unwrap();
            let mut document = no_resource_document();
            document.asset_manifest = serde_json::to_value(manifest).unwrap();
            let destination = folder.join("player");
            let error = export(&document, &assets, &backend, &backend, &destination).unwrap_err();
            assert!(error.contains("generated/control paths"), "{path}: {error}");
            assert!(
                !destination.exists(),
                "Reserved path must fail before destination creation"
            );
            assert_eq!(
                fs::read_to_string(&source).unwrap(),
                "must not be overwritten"
            );
        }
    }

    #[test]
    fn selected_project_resources_and_backend_sources_keep_distinct_roots() {
        let backend = backend_root();
        let folder = fixture();
        let resources = folder.join("resources");
        fs::create_dir_all(resources.join("assets/models")).unwrap();
        fs::create_dir_all(resources.join("assets/data")).unwrap();
        fs::copy(
            backend.join("assets/models/demo_crystal.glb"),
            resources.join("assets/models/demo_crystal.glb"),
        )
        .unwrap();
        fs::write(
            resources.join("assets/data/only-here.txt"),
            "owned by selected project",
        )
        .unwrap();
        let mut document = crate::game_host::demo_document();
        document.game.as_mut().unwrap().audio_cues.clear();
        let mut manifest = AssetManifest::default();
        manifest
            .register("local", "assets/data/only-here.txt", &resources)
            .unwrap();
        document.asset_manifest = serde_json::to_value(manifest).unwrap();
        let resources = resources.canonicalize().unwrap();
        let plan = prepare_files(
            &document,
            document.game.as_ref().unwrap(),
            &resources,
            &backend,
        )
        .unwrap();
        assert!(
            plan["assets/models/demo_crystal.glb"]
                .source
                .starts_with(&resources)
        );
        assert!(
            plan["assets/data/only-here.txt"]
                .source
                .starts_with(&resources)
        );
        assert_eq!(
            fs::read_to_string(&plan["assets/data/only-here.txt"].source).unwrap(),
            "owned by selected project"
        );
        assert!(plan["assets/gpu/forward.hlsl"].source.starts_with(&backend));
        assert!(
            !resources.join("assets/gpu").exists(),
            "GPU source must not be taken from the resource root"
        );
    }

    #[test]
    fn case_aliases_and_conflicting_backend_bytes_fail_preflight() {
        let backend = backend_root();
        let folder = fixture();
        let resources = folder.join("resources");
        fs::create_dir_all(resources.join("assets/Case")).unwrap();
        fs::create_dir_all(resources.join("assets/case")).unwrap();
        fs::write(resources.join("assets/Case/file.txt"), "same").unwrap();
        fs::write(resources.join("assets/case/FILE.txt"), "same").unwrap();
        let mut manifest = AssetManifest::default();
        manifest
            .register("first", "assets/Case/file.txt", &resources)
            .unwrap();
        manifest
            .register("second", "assets/case/FILE.txt", &resources)
            .unwrap();
        let mut document = no_resource_document();
        document.asset_manifest = serde_json::to_value(manifest).unwrap();
        let destination = folder.join("case-player");
        assert!(
            export(&document, &resources, &backend, &backend, &destination)
                .unwrap_err()
                .contains("collide")
        );
        assert!(!destination.exists());

        fs::create_dir_all(resources.join("assets/gpu")).unwrap();
        fs::write(
            resources.join("assets/gpu/forward.hlsl"),
            "different selected-project shader",
        )
        .unwrap();
        let mut manifest = AssetManifest::default();
        manifest
            .register("conflict", "assets/gpu/forward.hlsl", &resources)
            .unwrap();
        document.asset_manifest = serde_json::to_value(manifest).unwrap();
        let destination = folder.join("backend-player");
        assert!(
            export(&document, &resources, &backend, &backend, &destination)
                .unwrap_err()
                .contains("collide")
        );
        assert!(!destination.exists());
    }
}
