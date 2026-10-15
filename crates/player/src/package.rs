//! Strict package loading. No editor policy, migration, compiler root, or source
//! checkout discovery participates in a standalone Player launch.
use module_engine_core::{GraphDocument, SCHEMA_VERSION, SDK_DATA_API_FINGERPRINT, SdkIdentity};
use module_engine_host::{GameHost, HostOptions, RenderMode};
use module_engine_project::{
    AssetManifest, EngineProfile, hash_file, normalize_relative, resolve_relative,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

/// Native packages contain binaries for the machine that built them. A Linux
/// export must never look for a Windows executable merely because its source
/// project was authored there.
pub const PLAYER_EXECUTABLE: &str = if cfg!(windows) {
    "Player.exe"
} else {
    "Player"
};
pub const CARGO_PLAYER_EXECUTABLE: &str = if cfg!(windows) {
    "module-engine-player.exe"
} else {
    "module-engine-player"
};

pub fn native_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerManifest {
    pub schema_version: u32,
    pub engine_version: String,
    pub project: String,
    pub files: BTreeMap<String, String>,
    pub native_features: Vec<String>,
    pub sdk_data_api_fingerprint: String,
    /// Older schema-2 Windows packages omitted this field. New packages make
    /// their native platform explicit without changing authored project data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_platform: Option<String>,
}

fn read_manifest(root: &Path) -> Result<PlayerManifest, String> {
    let manifest: PlayerManifest = serde_json::from_slice(
        &fs::read(root.join("player.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("Player manifest: {error}"))?;
    if manifest.schema_version != 2 || manifest.engine_version != env!("CARGO_PKG_VERSION") {
        return Err("Player package requires the exact manifest/engine version".into());
    }
    if manifest.sdk_data_api_fingerprint != SDK_DATA_API_FINGERPRINT {
        return Err("Player package SDK data/API fingerprint differs from this build".into());
    }
    if let Some(platform) = &manifest.native_platform
        && platform != &native_platform()
    {
        return Err(format!(
            "Player native platform differs: package {platform}, running {}. Export the package on its target platform.",
            native_platform()
        ));
    }
    let features: BTreeSet<_> = manifest.native_features.iter().cloned().collect();
    if features.len() != manifest.native_features.len()
        || features
            .iter()
            .any(|id| !matches!(id.as_str(), "ui" | "arpg" | "puzzle" | "vulkan" | "audio"))
    {
        return Err("Player manifest needs unique known native feature IDs".into());
    }
    let project = normalize_relative(&manifest.project)?;
    if project != manifest.project || !manifest.files.contains_key(&project) {
        return Err("Player project must be a normalized file covered by package hashes".into());
    }
    let mut folded = BTreeSet::new();
    for (relative, hash) in &manifest.files {
        if normalize_relative(relative)? != *relative || !folded.insert(relative.to_lowercase()) {
            return Err(format!(
                "Player paths must be normalized and distinct on Windows: {relative}"
            ));
        }
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "Player file needs a lowercase SHA-256 hash: {relative}"
            ));
        }
        if hash_file(&resolve_relative(root, relative)?)? != *hash {
            return Err(format!("Player file changed or corrupt: {relative}"));
        }
    }
    Ok(manifest)
}

fn validate_sealed_tree(
    root: &Path,
    directory: &Path,
    manifest: &PlayerManifest,
) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    if fs::symlink_metadata(directory)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("Player artifact directories must remain inside the package".into());
    }
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            return Err("Player artifacts must not be symbolic links".into());
        }
        if kind.is_dir() {
            validate_sealed_tree(root, &entry.path(), manifest)?;
        } else if kind.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if !manifest.files.contains_key(&relative) {
                return Err(format!(
                    "Player artifact is not covered by package hashes: {relative}"
                ));
            }
        }
    }
    Ok(())
}

/// Check a selected Player package from an Editor compiled with a superset of
/// native packages. Its declared installation still validates every active pin.
pub fn validate_package_metadata(root: &Path) -> Result<GraphDocument, String> {
    let manifest = read_manifest(root)?;
    validate_package_contents(root, &manifest)
}

fn validate_package_contents(
    root: &Path,
    manifest: &PlayerManifest,
) -> Result<GraphDocument, String> {
    for directory in [".spirv", "scripts", "assets"] {
        validate_sealed_tree(root, &root.join(directory), manifest)?;
    }
    let document: GraphDocument = serde_json::from_slice(
        &fs::read(resolve_relative(root, &manifest.project)?).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("Player project: {error}"))?;
    if document.schema_version != SCHEMA_VERSION {
        return Err("Player project migration must be completed before packaging".into());
    }
    let features = manifest.native_features.iter().cloned().collect();
    let composition = crate::composition_for_features(root, &features)?;
    EngineProfile::from_document(&document)?.validate_with_sdk(
        &document,
        &composition,
        Some(&SdkIdentity::current()),
    )?;
    let errors = composition
        .validate_document(&document)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == module_engine_core::Severity::Error)
        .map(|diagnostic| diagnostic.message)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let assets = AssetManifest::from_value(&document.asset_manifest)?;
    for asset in assets.assets.values() {
        if !manifest
            .files
            .contains_key(&normalize_relative(&asset.path)?)
        {
            return Err(format!(
                "Player asset is not covered by package hashes: {}",
                asset.path
            ));
        }
    }
    assets.verify(root)?;
    Ok(document)
}

/// A launched binary must match the exact compiled feature fingerprint, even
/// when a larger build could otherwise execute the document.
pub fn validate_package(root: &Path) -> Result<GraphDocument, String> {
    let manifest = read_manifest(root)?;
    let features: BTreeSet<_> = manifest.native_features.iter().cloned().collect();
    if features != crate::compiled_features() {
        return Err(format!(
            "Player native features differ: package {features:?}, compiled {:?}",
            crate::compiled_features()
        ));
    }
    validate_package_contents(root, &manifest)
}

pub fn build_package_host(
    root: &Path,
    headless: bool,
    services_enabled: bool,
) -> Result<GameHost, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let document = validate_package(&root)?;
    let composition = crate::composition_at(&root)?;
    let render = if headless {
        let world = if let Ok(route) = composition.modules.render_route(&document) {
            route.world.id.clone()
        } else {
            let worlds = document
                .root
                .nodes
                .iter()
                .filter(|node| composition.modules.is_world_provider(&node.module))
                .collect::<Vec<_>>();
            if worlds.len() != 1 {
                return Err("Headless package needs one explicitly declared world".into());
            }
            worlds[0].id.clone()
        };
        RenderMode::Headless { world_node: world }
    } else {
        let route = composition.modules.render_route(&document)?;
        if route.renderer.module == "render.vulkan" && !root.join(".spirv").is_dir() {
            return Err("Vulkan Player requires its sealed .spirv artifacts; source compilation is unavailable".into());
        }
        RenderMode::Graph
    };
    GameHost::build(
        document,
        composition,
        HostOptions {
            asset_root: root.clone(),
            backend_root: Some(root),
            compile_root: None,
            render,
            services: crate::service_registry()?,
            services_enabled,
        },
        None,
    )
}

/// A real relocated binary can emit construction, simulation and pixel evidence
/// without opening a window. This never rewrites or migrates its authored JSON.
pub fn verify_package(root: &Path, output: Option<&Path>, headless: bool) -> Result<Value, String> {
    let manifest = read_manifest(root)?;
    let project_path = resolve_relative(root, &manifest.project)?;
    let before = hash_file(&project_path)?;
    let mut host = build_package_host(root, headless, false)?;
    for _ in 0..3 {
        host.step(1. / 60., &module_engine_core::GameInput::default())?;
    }
    host.set_detailed_stats(true);
    let rendered = if headless {
        None
    } else {
        Some(host.render(960, 540)?)
    };
    if let Some(output) = output {
        fs::create_dir_all(output).map_err(|error| error.to_string())?;
        let output_root = output.canonicalize().map_err(|error| error.to_string())?;
        for filename in ["player-frame.png", "player-verification.json"] {
            let target = output_root.join(filename);
            let target = if target.exists() {
                target.canonicalize().map_err(|error| error.to_string())?
            } else {
                target
            };
            for relative in manifest.files.keys() {
                if target == resolve_relative(root, relative)? {
                    return Err(format!(
                        "Player evidence would overwrite a sealed package file: {relative}"
                    ));
                }
            }
        }
        if let Some(frame) = &rendered {
            crate::write_png(&output.join("player-frame.png"), frame)?;
        }
    }
    if hash_file(&project_path)? != before {
        return Err("Player verification changed authored bytes".into());
    }
    let report = json!({
        "schema_version":1,"kind":"standalone-player-package","package_root":root.canonicalize().map_err(|error|error.to_string())?,
        "engine_version":env!("CARGO_PKG_VERSION"),"native_features":crate::compiled_features(),"sdk_data_api_fingerprint":SDK_DATA_API_FINGERPRINT,
        "package_hashes_verified":true,"exact_contract_lock_verified":true,"source_compilation":false,"headless":headless,
        "state":host.session.state(),"diagnostics":host.game_diagnostics(),"device":host.device,
        "frame":rendered.as_ref().map(|frame|json!({"width":frame.width,"height":frame.height,"stats":frame.stats})),
        "services_enabled":false,"services":host.services.diagnostics(),"authored_project_bytes_unchanged":true,
    });
    if let Some(output) = output {
        fs::write(
            output.join("player-verification.json"),
            serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::*;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        document: GraphDocument,
        manifest: PlayerManifest,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "ModuleEngine player package {} {}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            let mut document = demo_document(WorldKind::Ecs);
            document.root.nodes.retain(|node| node.id == "world");
            document.root.edges.clear();
            document.custom_sources.clear();
            document.game = Some(GameProject::empty());
            document.engine.enabled_modules.insert("world.ecs".into());
            Self::pin_document(&root, &mut document);
            let mut fixture = Self {
                root,
                document,
                manifest: PlayerManifest {
                    schema_version: 2,
                    engine_version: env!("CARGO_PKG_VERSION").into(),
                    project: "project.json".into(),
                    files: BTreeMap::new(),
                    native_features: crate::compiled_features().into_iter().collect(),
                    sdk_data_api_fingerprint: SDK_DATA_API_FINGERPRINT.into(),
                    native_platform: Some(native_platform()),
                },
            };
            fixture.write_document();
            fixture
        }
        fn pin_document(root: &Path, document: &mut GraphDocument) {
            let composition = crate::composition_at(root).unwrap();
            let profile = EngineProfile::new(
                "fixture.player-headless",
                "1",
                module_engine_project::EngineBlueprint {
                    root: document.root.clone(),
                    engine: document.engine.clone(),
                    custom_sources: document.custom_sources.clone(),
                    shader_sources: document.shader_sources.clone(),
                    default_execution: document.game.as_ref().unwrap().execution.clone(),
                    available_modules: composition
                        .required_contracts(document)
                        .unwrap()
                        .module_versions
                        .into_keys()
                        .collect(),
                },
            )
            .unwrap();
            document.engine_profile = Some(profile.document_value().unwrap());
            composition
                .capture_lock(document, profile.identity, None)
                .unwrap()
                .apply_to(document);
        }
        fn add_render_graph(&mut self) {
            let mut graph = demo_document(WorldKind::Ecs).root;
            graph.nodes.retain(|node| {
                matches!(
                    node.id.as_str(),
                    "world" | "extract" | "renderer" | "viewport"
                )
            });
            graph
                .edges
                .retain(|edge| matches!(edge.from_node.as_str(), "world" | "extract" | "renderer"));
            self.document.root = graph;
            Self::pin_document(&self.root, &mut self.document);
            self.write_document();
        }
        fn write_manifest(&self) {
            fs::write(
                self.root.join("player.json"),
                serde_json::to_vec_pretty(&self.manifest).unwrap(),
            )
            .unwrap();
        }
        fn write_document(&mut self) {
            fs::write(
                self.root.join("project.json"),
                serde_json::to_vec_pretty(&self.document).unwrap(),
            )
            .unwrap();
            self.manifest.files.insert(
                "project.json".into(),
                hash_file(&self.root.join("project.json")).unwrap(),
            );
            self.write_manifest();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let root = self.root.canonicalize().unwrap();
            let temporary = std::env::temp_dir().canonicalize().unwrap();
            assert!(root.starts_with(&temporary) && root != temporary);
            assert!(
                root.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("ModuleEngine player package ")
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn strict_package_reads_verify_hashes_sdk_and_format_without_migrating_bytes() {
        let mut fixture = Fixture::new();
        let before = fs::read(fixture.root.join("project.json")).unwrap();
        assert_eq!(validate_package(&fixture.root).unwrap(), fixture.document);
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), before);
        fixture.manifest.sdk_data_api_fingerprint = "module-engine-data-api/v1".into();
        fixture.write_manifest();
        assert!(
            validate_package(&fixture.root)
                .unwrap_err()
                .contains("SDK data/API fingerprint")
        );
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), before);
        fixture.manifest.sdk_data_api_fingerprint = SDK_DATA_API_FINGERPRINT.into();
        fixture.manifest.schema_version = 1;
        fixture.write_manifest();
        assert!(
            validate_package(&fixture.root)
                .unwrap_err()
                .contains("manifest/engine version")
        );
        fixture.manifest.schema_version = 2;
        fixture.document.schema_version = 3;
        fixture.write_document();
        let legacy = fs::read(fixture.root.join("project.json")).unwrap();
        assert!(
            validate_package(&fixture.root)
                .unwrap_err()
                .contains("migration")
        );
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), legacy);
        fs::write(
            fixture.root.join("project.json"),
            b"corrupted authored bytes",
        )
        .unwrap();
        assert!(
            validate_package(&fixture.root)
                .unwrap_err()
                .contains("changed or corrupt")
        );
        assert_eq!(
            fs::read(fixture.root.join("project.json")).unwrap(),
            b"corrupted authored bytes"
        );
    }

    #[test]
    fn native_platform_mismatch_is_rejected_without_touching_authored_data() {
        let mut fixture = Fixture::new();
        let before = fs::read(fixture.root.join("project.json")).unwrap();
        fixture.manifest.native_platform = Some("foreign-os-foreign-arch".into());
        fixture.write_manifest();
        for error in [
            validate_package(&fixture.root).unwrap_err(),
            validate_package_metadata(&fixture.root).unwrap_err(),
        ] {
            assert!(error.contains("native platform differs"), "{error}");
        }
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), before);
        assert!(!fixture.root.join(".work").exists());
        // Existing schema-2 packages retain their compatibility: an omitted
        // optional field is never synthesized into player.json on a read.
        fixture.manifest.native_platform = None;
        fixture.write_manifest();
        let manifest_before = fs::read(fixture.root.join("player.json")).unwrap();
        assert_eq!(validate_package(&fixture.root).unwrap(), fixture.document);
        assert_eq!(
            fs::read(fixture.root.join("player.json")).unwrap(),
            manifest_before
        );
    }

    #[test]
    fn native_executable_names_match_the_compiled_target() {
        assert_eq!(
            PLAYER_EXECUTABLE,
            format!("Player{}", std::env::consts::EXE_SUFFIX)
        );
        assert_eq!(
            CARGO_PLAYER_EXECUTABLE,
            format!("module-engine-player{}", std::env::consts::EXE_SUFFIX)
        );
        assert_eq!(
            native_platform(),
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
        );
        #[cfg(target_os = "linux")]
        assert!(!PLAYER_EXECUTABLE.ends_with(".exe"));
    }

    #[test]
    fn normal_player_reads_reject_missing_module_pin_or_lock_without_filling_them() {
        let mut fixture = Fixture::new();
        assert!(
            fixture
                .document
                .module_versions
                .remove("world.ecs")
                .is_some()
        );
        fixture.write_document();
        let bytes = fs::read(fixture.root.join("project.json")).unwrap();
        assert!(
            validate_package(&fixture.root)
                .unwrap_err()
                .contains("lock fingerprint")
        );
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), bytes);
        fixture.document.engine_lock = None;
        fixture.write_document();
        let bytes = fs::read(fixture.root.join("project.json")).unwrap();
        assert!(
            validate_package(&fixture.root)
                .unwrap_err()
                .contains("engine lock")
        );
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), bytes);
    }

    #[test]
    fn binary_features_are_exact_while_metadata_can_validate_a_smaller_native_installation() {
        let mut fixture = Fixture::new();
        fixture.manifest.native_features.reverse();
        fixture.write_manifest();
        assert_eq!(validate_package(&fixture.root).unwrap(), fixture.document);
        if fixture.manifest.native_features.is_empty() {
            fixture.manifest.native_features.push("ui".into());
            fixture.write_manifest();
            assert!(
                validate_package(&fixture.root)
                    .unwrap_err()
                    .contains("native features differ")
            );
            assert!(
                validate_package_metadata(&fixture.root)
                    .unwrap_err()
                    .contains("absent from this build")
            );
        } else {
            fixture.manifest.native_features.pop();
            fixture.write_manifest();
            assert_eq!(
                validate_package_metadata(&fixture.root).unwrap(),
                fixture.document
            );
            assert!(
                validate_package(&fixture.root)
                    .unwrap_err()
                    .contains("native features differ")
            );
        }
    }

    #[test]
    fn duplicate_and_unknown_native_features_are_rejected_before_package_construction() {
        let mut fixture = Fixture::new();
        let before = fs::read(fixture.root.join("project.json")).unwrap();
        for features in [
            vec!["ui".into(), "ui".into()],
            vec!["unavailable-native".into()],
        ] {
            fixture.manifest.native_features = features;
            fixture.write_manifest();
            for error in [
                validate_package(&fixture.root).unwrap_err(),
                validate_package_metadata(&fixture.root).unwrap_err(),
            ] {
                assert!(error.contains("unique known native feature IDs"), "{error}");
            }
            assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), before);
            assert!(!fixture.root.join(".work").exists());
        }
    }

    #[cfg(feature = "puzzle")]
    #[test]
    fn metadata_uses_declared_native_installation_for_an_active_puzzle_package() {
        let mut fixture = Fixture::new();
        fixture.document.game = Some(module_engine_project_puzzle::project());
        Fixture::pin_document(&fixture.root, &mut fixture.document);
        fixture.write_document();
        assert_eq!(validate_package(&fixture.root).unwrap(), fixture.document);
        let report = verify_package(&fixture.root, None, true).unwrap();
        assert_eq!(report["diagnostics"]["outcome"], json!("Playing"));
        assert_eq!(report["state"]["tick"], json!(3));
        assert!(report["frame"].is_null());
        assert_eq!(report["services"], json!({}));
        let before = fs::read(fixture.root.join("project.json")).unwrap();
        fixture.manifest.native_features.retain(|id| id != "puzzle");
        fixture.write_manifest();
        let error = validate_package_metadata(&fixture.root).unwrap_err();
        assert!(error.contains("puzzle.turn"), "{error}");
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), before);
        assert!(!fixture.root.join(".work").exists());
    }

    #[test]
    fn native_artifacts_must_be_covered_by_manifest_hashes() {
        let mut fixture = Fixture::new();
        for folder in [".spirv", "scripts", "assets"] {
            fs::create_dir(fixture.root.join(folder)).unwrap();
            let relative = format!("{folder}/unsealed.bin");
            fs::write(fixture.root.join(&relative), b"native artifact").unwrap();
            assert!(
                validate_package_metadata(&fixture.root)
                    .unwrap_err()
                    .contains("not covered by package hashes")
            );
            fixture.manifest.files.insert(
                relative.clone(),
                hash_file(&fixture.root.join(relative)).unwrap(),
            );
            fixture.write_manifest();
            validate_package_metadata(&fixture.root).unwrap();
        }
    }

    #[test]
    fn headless_package_executes_without_renderer_services_compiler_or_source_discovery() {
        let fixture = Fixture::new();
        let before = fs::read(fixture.root.join("project.json")).unwrap();
        let report = verify_package(&fixture.root, None, true).unwrap();
        assert_eq!(report["headless"], json!(true));
        assert_eq!(report["state"]["tick"], json!(3));
        assert!(report["frame"].is_null());
        assert_eq!(report["device"], json!("Headless"));
        assert_eq!(report["services"], json!({}));
        assert_eq!(report["source_compilation"], json!(false));
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), before);
        assert!(
            !fixture.root.join("Cargo.toml").exists()
                && !fixture.root.join(".sdk").exists()
                && !fixture.root.join("tools").exists()
        );
    }

    #[cfg(feature = "arpg")]
    #[test]
    fn headless_package_omits_a_requested_audio_service_even_when_services_are_enabled() {
        let mut fixture = Fixture::new();
        fixture.add_render_graph();
        fs::create_dir(fixture.root.join("assets")).unwrap();
        let relative = "assets/unparsed.wav";
        fs::write(
            fixture.root.join(relative),
            b"sealed bytes, deliberately not a WAV file",
        )
        .unwrap();
        let mut assets = AssetManifest::default();
        assets.register("cue", relative, &fixture.root).unwrap();
        fixture.document.asset_manifest = serde_json::to_value(assets).unwrap();
        fixture.manifest.files.insert(
            relative.into(),
            hash_file(&fixture.root.join(relative)).unwrap(),
        );
        let game = fixture.document.game.as_mut().unwrap();
        game.audio_cues.insert("hit".into(), "asset://cue".into());
        game.execution = ExecutionGraph {
            resources: vec![SystemResource {
                name: "events".into(),
                data_type: "GameEvents".into(),
                external: true,
                initial: json!([]),
            }],
            stages: vec![SystemStage {
                name: "Audio events".into(),
                nodes: vec![SystemNode {
                    id: "audio".into(),
                    module: "game.audio".into(),
                    params: json!({}),
                    bindings: BTreeMap::from([("events".into(), "events".into())]),
                    after: vec![],
                }],
            }],
        };
        Fixture::pin_document(&fixture.root, &mut fixture.document);
        fixture.write_document();
        let before = fs::read(fixture.root.join("project.json")).unwrap();
        let mut host = build_package_host(&fixture.root, true, true).unwrap();
        assert_eq!(host.device, "Headless");
        assert!(host.services.is_empty());
        host.step(1. / 60., &GameInput::default()).unwrap();
        assert_eq!(host.session.state().tick, 1);
        assert_eq!(host.services.diagnostics(), json!({}));
        let error = match build_package_host(&fixture.root, false, true) {
            Ok(_) => panic!("Native audio ignored its missing factory or invalid WAV bytes"),
            Err(error) => error,
        };
        assert!(error.contains("Host service 'audio.wav'"), "{error}");
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), before);
        assert!(!fixture.root.join(".work").exists());
    }

    #[test]
    fn rendered_package_verification_writes_actual_cpu_pixels_and_state_report() {
        let mut fixture = Fixture::new();
        fixture.add_render_graph();
        fixture
            .document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("empty")
            .unwrap()
            .objects
            .push(AuthoredObject::new(WorldObject {
                id: 11,
                parent: None,
                transform: Transform::default(),
                color: [228, 94, 67, 255],
            }));
        fixture.write_document();
        let before = fs::read(fixture.root.join("project.json")).unwrap();
        let evidence = fixture.root.join("evidence");
        let report = verify_package(&fixture.root, Some(&evidence), false).unwrap();
        assert_eq!(report["headless"], json!(false));
        assert_eq!(report["state"]["tick"], json!(3));
        assert_eq!(report["frame"]["width"], json!(960));
        assert_eq!(report["frame"]["height"], json!(540));
        assert!(report["frame"]["stats"]["shaded_pixels"].as_u64().unwrap() > 100);
        assert_eq!(report["services"], json!({}));
        let png = png::Decoder::new(std::io::BufReader::new(
            fs::File::open(evidence.join("player-frame.png")).unwrap(),
        ))
        .read_info()
        .unwrap();
        assert_eq!((png.info().width, png.info().height), (960, 540));
        let saved: Value =
            serde_json::from_slice(&fs::read(evidence.join("player-verification.json")).unwrap())
                .unwrap();
        assert_eq!(saved, report);
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), before);
        validate_package(&fixture.root).unwrap();
        assert!(!fixture.root.join(".work").exists());
    }

    #[cfg(feature = "vulkan")]
    #[test]
    fn headless_package_does_not_construct_its_selected_vulkan_renderer_or_compile_shaders() {
        let mut fixture = Fixture::new();
        fixture.add_render_graph();
        let renderer = fixture
            .document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "renderer")
            .unwrap();
        renderer.module = "render.vulkan".into();
        renderer.subgraph = Some(Box::new(module_engine_vulkan::default_graph(
            module_engine_vulkan::RenderPath::Forward,
        )));
        Fixture::pin_document(&fixture.root, &mut fixture.document);
        fixture.write_document();
        let before = fs::read(fixture.root.join("project.json")).unwrap();
        let report = verify_package(&fixture.root, None, true).unwrap();
        assert_eq!(report["device"], json!("Headless"));
        assert!(report["frame"].is_null());
        assert_eq!(report["services"], json!({}));
        assert_eq!(report["state"]["tick"], json!(3));
        assert!(!fixture.root.join(".spirv").exists());
        assert!(!fixture.root.join(".work").exists());
        assert!(!fixture.root.join("tools").exists());
        let error = match build_package_host(&fixture.root, false, false) {
            Ok(_) => panic!("Unsealed Vulkan package constructed a native renderer"),
            Err(error) => error,
        };
        assert!(error.contains("sealed .spirv artifacts"), "{error}");
        assert_eq!(fs::read(fixture.root.join("project.json")).unwrap(), before);
        assert!(!fixture.root.join(".work").exists());
    }

    #[test]
    fn verification_output_cannot_overwrite_sealed_package_bytes() {
        let mut fixture = Fixture::new();
        fs::create_dir(fixture.root.join("evidence")).unwrap();
        let relative = "evidence/player-verification.json";
        fs::write(fixture.root.join(relative), b"already sealed package data").unwrap();
        fixture.manifest.files.insert(
            relative.into(),
            hash_file(&fixture.root.join(relative)).unwrap(),
        );
        fixture.write_manifest();
        assert!(
            verify_package(&fixture.root, Some(&fixture.root.join("evidence")), true)
                .unwrap_err()
                .contains("would overwrite a sealed package file")
        );
        assert_eq!(
            fs::read(fixture.root.join(relative)).unwrap(),
            b"already sealed package data"
        );
        validate_package(&fixture.root).unwrap();
    }
}
