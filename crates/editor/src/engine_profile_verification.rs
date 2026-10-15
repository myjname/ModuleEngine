//! Explicit Stage7 evidence through public migration, project and native Player
//! paths. This module owns only newly created evidence folders. It never seals
//! source inventories or rewrites the supplied legacy project/binding/assets.

use crate::{
    distribution::{self, Distribution},
    engine_profiles::{EngineChoice, ProfileKind},
    project,
    workspace_files::{self, ProjectTemplate},
};
use module_engine_core::{GameInput, GraphDocument, SdkIdentity};
use module_engine_host::{GameHost, HostOptions, RenderMode};
use module_engine_host_services::{HostServiceRegistry, install_audio};
use module_engine_project::{
    AssetManifest, EngineProfile, hash_bytes, hash_file, resolve_relative,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DT: f64 = 1. / 60.;

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}
fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("{}: {error}", path.display()))
}

struct SourceEvidence {
    source: PathBuf,
    bytes: Vec<u8>,
    binding: PathBuf,
    binding_bytes: Option<Vec<u8>>,
    resource_hashes: BTreeMap<String, String>,
}
impl SourceEvidence {
    fn new(source: &Path, document: &GraphDocument) -> Result<Self, String> {
        let source = source.canonicalize().map_err(|error| error.to_string())?;
        let root = source
            .parent()
            .ok_or("Source project has no asset folder")?;
        let binding = distribution::project_binding_path(&source)?;
        let resource_hashes = AssetManifest::from_value(&document.asset_manifest)?
            .assets
            .values()
            .map(|asset| {
                Ok((
                    asset.path.clone(),
                    hash_file(&resolve_relative(root, &asset.path)?)?,
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            bytes: read_bytes(&source)?,
            binding_bytes: binding
                .is_file()
                .then(|| read_bytes(&binding))
                .transpose()?,
            source,
            binding,
            resource_hashes,
        })
    }
    fn verify(&self) -> Result<Value, String> {
        require(
            read_bytes(&self.source)? == self.bytes,
            "Verification changed original project bytes",
        )?;
        let binding = self
            .binding
            .is_file()
            .then(|| read_bytes(&self.binding))
            .transpose()?;
        require(
            binding == self.binding_bytes,
            "Verification changed the original engine binding",
        )?;
        let root = self.source.parent().ok_or("Source asset root missing")?;
        for (relative, expected) in &self.resource_hashes {
            require(
                hash_file(&resolve_relative(root, relative)?)? == *expected,
                "Verification changed an original resource",
            )?;
        }
        Ok(
            json!({"path":self.source,"project_sha256":hash_bytes(&self.bytes),"binding_sha256":self.binding_bytes.as_ref().map(|bytes|hash_bytes(bytes)),"resource_sha256":self.resource_hashes,"original_bytes_binding_and_resources_unchanged":true}),
        )
    }

    fn check_report_destination(&self, path: &Path) -> Result<(), String> {
        if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err("Verification summary cannot replace a symbolic link".into());
        }
        let target = if path.exists() {
            path.canonicalize().map_err(|error| error.to_string())?
        } else {
            path.parent()
                .ok_or("Verification summary folder missing")?
                .canonicalize()
                .map_err(|error| error.to_string())?
                .join(
                    path.file_name()
                        .ok_or("Verification summary filename missing")?,
                )
        };
        require(
            target != self.source && target != self.binding,
            "Verification summary would overwrite source metadata",
        )?;
        let root = self
            .source
            .parent()
            .ok_or("Original asset folder missing")?;
        for relative in self.resource_hashes.keys() {
            require(
                target
                    != resolve_relative(root, relative)?
                        .canonicalize()
                        .map_err(|error| error.to_string())?,
                "Verification summary would overwrite an original resource",
            )?;
        }
        Ok(())
    }
}

fn rejected_legacy_contract(
    raw: &[u8],
    document: &GraphDocument,
    distribution: &Distribution,
    case: &Path,
) -> Result<Value, String> {
    let mut value: Value =
        serde_json::from_slice(raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw))
            .map_err(|error| error.to_string())?;
    let world = crate::host::composition_at(&distribution.package_root)?
        .modules
        .render_route(document)?
        .world
        .module
        .clone();
    value["module_versions"][&world] = json!("unknown-source-version");
    let source = case.join("rejected-legacy-source.json");
    write_json(&source, &value)?;
    let binding = distribution::project_binding_path(&source)?;
    write_json(
        &binding,
        &distribution::ProjectEngineBinding::from_distribution(distribution)?,
    )?;
    let before = read_bytes(&source)?;
    let binding_before = read_bytes(&binding)?;
    let destination = case.join("rejected-unknown-contract-copy/project.json");
    let error = workspace_files::migrate_copy(&source, &destination, ProfileKind::Arpg)
        .err()
        .ok_or("Unknown active legacy contract unexpectedly migrated")?;
    require(
        error.contains("unknown-source-version"),
        "Unknown legacy contract rejected for an unrelated reason",
    )?;
    require(
        !destination.exists() && !distribution::project_binding_path(&destination)?.exists(),
        "Rejected legacy migration published a copy or binding",
    )?;
    require(
        read_bytes(&source)? == before && read_bytes(&binding)? == binding_before,
        "Rejected migration changed source fixture bytes",
    )?;
    Ok(
        json!({"error":error,"source_sha256":hash_bytes(&before),"binding_sha256":hash_bytes(&binding_before),"source_bytes_unchanged":true,"copy_and_binding_absent":true}),
    )
}

fn boss_values(
    document: &GraphDocument,
    legacy_defaults: bool,
    backend: &Path,
) -> Result<BTreeMap<String, bool>, String> {
    let mut values = BTreeMap::new();
    let game = document
        .game
        .as_ref()
        .ok_or("ARPG legacy source has no authored game")?;
    let composition = crate::host::composition_at(backend)?;
    for (scene, authored_scene) in &game.scenes {
        let entities = if legacy_defaults {
            game.resolve_scene(scene)?
        } else {
            game.resolve_scene_with_registry(scene, &composition.systems)?
        };
        for entity in entities {
            if let Some(boss) = entity.components.get("boss") {
                let authored = authored_scene
                    .objects
                    .iter()
                    .find(|object| object.object.id == entity.object.id)
                    .ok_or("Resolved boss has no authored object")?;
                let value = if legacy_defaults {
                    authored
                        .overrides
                        .get("boss.victory_on_death")
                        .and_then(Value::as_bool)
                        .or_else(|| boss.get("victory_on_death").and_then(Value::as_bool))
                        .unwrap_or(true)
                } else {
                    boss.get("victory_on_death")
                        .and_then(Value::as_bool)
                        .ok_or("Migrated boss has no explicit effective rule")?
                };
                values.insert(format!("{scene}/{}", entity.object.id), value);
            }
        }
    }
    Ok(values)
}

fn rejected_publications(
    document: &GraphDocument,
    source: &Path,
    distribution: &Distribution,
    case: &Path,
) -> Result<Value, String> {
    let composition = crate::host::composition_at(&distribution.package_root)?;
    let world_module = composition
        .modules
        .render_route(document)?
        .world
        .module
        .clone();
    let mut missing_module = document.clone();
    require(
        missing_module
            .module_versions
            .remove(&world_module)
            .is_some(),
        "Expected active world implementation pin",
    )?;
    let mut missing_sdk = document.clone();
    missing_sdk
        .engine_lock
        .as_mut()
        .ok_or("Migrated Engine lock missing")?
        .sdk = None;
    let mut wrong_sdk = document.clone();
    let mut sdk = SdkIdentity::current();
    sdk.version.push_str("-incompatible");
    composition
        .capture_lock(
            &wrong_sdk,
            EngineProfile::from_document(document)?.identity,
            Some(sdk),
        )?
        .apply_to(&mut wrong_sdk);
    let compile_root = distribution.compile_root()?;
    let asset_root = source.parent().ok_or("Migrated project folder missing")?;
    let mut results = BTreeMap::new();
    for (name, candidate) in [
        ("missing-active-module-pin", missing_module),
        ("missing-sdk-pin", missing_sdk),
        ("wrong-sdk-identity", wrong_sdk),
    ] {
        let save = case.join(format!("rejected-{name}/project.json"));
        let save_error = workspace_files::save_as(source, &save, &candidate, distribution)
            .err()
            .ok_or_else(|| format!("Invalid {name} unexpectedly saved"))?;
        require(
            !save.exists() && !distribution::project_binding_path(&save)?.exists(),
            "Rejected Save As published project metadata",
        )?;
        let package = case.join(format!("rejected-{name}-player"));
        let export_error = crate::game_package::export(
            &candidate,
            asset_root,
            &distribution.package_root,
            &compile_root,
            &package,
        )
        .err()
        .ok_or_else(|| format!("Invalid {name} unexpectedly exported"))?;
        require(
            !package.exists(),
            "Rejected Player export published its destination",
        )?;
        results.insert(name,json!({"save_rejected":save_error,"player_export_rejected":export_error,"project_binding_and_player_destinations_absent":true}));
    }
    Ok(json!(results))
}

/// Exercise the unchanged source controller through the real contribution's
/// legacy input adapter and externally-tagged Hit event wire. Diagnostic object
/// positions/components are edited only in an unsaved, isolated in-memory copy.
fn controller_compatibility(
    document: &GraphDocument,
    asset_root: &Path,
    distribution: &Distribution,
) -> Result<Value, String> {
    let mut diagnostic = document.clone();
    let game = diagnostic
        .game
        .as_mut()
        .ok_or("Migrated ARPG project missing")?;
    let entry = game.entry_scene.clone();
    let composition = crate::host::composition_at(&distribution.package_root)?;
    let resolved = game.resolve_scene_with_registry(&entry, &composition.systems)?;
    let player = resolved
        .iter()
        .find(|entity| entity.components.contains_key("player"))
        .ok_or("Legacy ARPG fixture has no player")?;
    require(
        player.object.id == 1,
        "Known legacy game-controller fixture expects player ID1",
    )?;
    let team = player
        .components
        .get("actor")
        .and_then(|actor| actor.get("team"))
        .and_then(Value::as_str)
        .ok_or("Legacy player team missing")?;
    let target = resolved
        .iter()
        .find(|entity| {
            entity
                .components
                .get("actor")
                .and_then(|actor| actor.get("team"))
                .and_then(Value::as_str)
                .is_some_and(|other| other != team)
                && entity.components.contains_key("health")
        })
        .ok_or("Legacy ARPG fixture has no enemy")?
        .object
        .id;
    let enemies = resolved
        .iter()
        .filter(|entity| entity.components.contains_key("ai"))
        .map(|entity| entity.object.id)
        .collect::<Vec<_>>();
    for prefab in game.prefabs.values_mut() {
        prefab.components.remove("ai");
        prefab.components.remove("obstacle");
    }
    let target_position = [0., 0.45, -0.8];
    for (index, object) in game
        .scenes
        .get_mut(&entry)
        .ok_or("Entry scene missing")?
        .objects
        .iter_mut()
        .enumerate()
    {
        object.components.remove("obstacle");
        if enemies.contains(&object.object.id) {
            object.components.remove("ai");
            object.object.transform.translation = [1000. + index as f32 * 3., 0.45, 1000.];
        }
        if object.object.id == 1 {
            object.object.transform.translation = [0., 0.45, 0.];
        }
        if object.object.id == target {
            object.object.transform.translation = target_position;
        }
    }
    let world_node = composition
        .modules
        .render_route(&diagnostic)?
        .world
        .id
        .clone();
    let mut services = HostServiceRegistry::new();
    install_audio(&mut services)?;
    let mut host = GameHost::build(
        diagnostic,
        composition,
        HostOptions {
            asset_root: asset_root.into(),
            backend_root: Some(distribution.package_root.clone()),
            compile_root: Some(distribution.compile_root()?),
            render: RenderMode::Headless { world_node },
            services,
            services_enabled: false,
        },
        None,
    )?;
    require(
        host.device == "Headless" && !host.services.enabled(),
        "Controller proof initialized graphical/audio service execution",
    )?;
    let input = module_engine_arpg::ArpgInput {
        aim_world: target_position,
        attack: true,
        interact: true,
        ..Default::default()
    }
    .into_input();
    host.step(DT, &input)?;
    require(
        host.session
            .state()
            .world
            .object(1)
            .is_some_and(|object| object.color == [250, 180, 75, 255]),
        "Authored controller did not receive legacy input.interact",
    )?;
    let interact_event = host.session.events().iter().any(|event| {
        event.name == "Sound" && event.payload.get("cue").and_then(Value::as_str) == Some("hit")
    });
    require(
        interact_event,
        "Authored controller did not emit its interaction Sound event",
    )?;
    let mut saw_hit = host
        .session
        .events()
        .iter()
        .any(|event| event.name == "Hit");
    let mut controller = Value::Null;
    for _ in 0..90 {
        controller = host
            .slots
            .get("game-controller")
            .ok_or("Expected authored game-controller Custom slot")?
            .lock()
            .map_err(|_| "Controller slot poisoned")?
            .state()
            .map_err(|error| error.to_string())?;
        if controller
            .get("hits")
            .and_then(Value::as_u64)
            .is_some_and(|hits| hits > 0)
        {
            break;
        }
        host.step(DT, &GameInput::default())?;
        saw_hit |= host
            .session
            .events()
            .iter()
            .any(|event| event.name == "Hit");
    }
    require(
        saw_hit
            && controller
                .get("hits")
                .and_then(Value::as_u64)
                .is_some_and(|hits| hits > 0),
        "Authored controller did not count a native Hit event",
    )?;
    Ok(
        json!({"real_headless_host":true,"services_enabled":false,"unchanged_authored_source":true,"legacy_input_interact":true,"interaction_color":[250,180,75,255],"interaction_sound_event":interact_event,"native_hit_seen":saw_hit,"accepted_custom_state":controller}),
    )
}

fn file_hashes(root: &Path) -> Result<BTreeMap<String, String>, String> {
    fn visit(
        root: &Path,
        folder: &Path,
        files: &mut BTreeMap<String, String>,
    ) -> Result<(), String> {
        for entry in fs::read_dir(folder).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_symlink() {
                return Err("Verification artifacts cannot contain symbolic links".into());
            }
            if kind.is_dir() {
                visit(root, &entry.path(), files)?;
            } else if kind.is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(relative, hash_file(&entry.path())?);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}
fn copy_package(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_symlink() {
            return Err("Package relocation cannot follow symbolic links".into());
        }
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_package(&entry.path(), &target)?;
        } else if kind.is_file() {
            let mut input = fs::File::open(entry.path()).map_err(|error| error.to_string())?;
            let mut output = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(target)
                .map_err(|error| error.to_string())?;
            std::io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = input
                    .metadata()
                    .map_err(|error| error.to_string())?
                    .permissions()
                    .mode();
                output
                    .set_permissions(fs::Permissions::from_mode(mode & 0o777))
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

fn native_cli(package: &Path, mode: &str, evidence: &Path, cwd: &Path) -> Result<Value, String> {
    fs::create_dir(evidence).map_err(|error| error.to_string())?;
    let stdout = evidence.join("stdout.json");
    let stderr = evidence.join("stderr.txt");
    let mut command = Command::new(package.join(module_engine_player::package::PLAYER_EXECUTABLE));
    command
        .arg(mode)
        .arg(package)
        .arg(evidence)
        .current_dir(cwd)
        .env_remove("MODULE_ENGINE_PACKAGE_ROOT")
        .stdout(Stdio::from(
            fs::File::create(&stdout).map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(
            fs::File::create(&stderr).map_err(|error| error.to_string())?,
        ));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Relocated Player launch: {error}"))?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(180) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "Relocated Player {mode} exceeded 180 seconds; logs at {}",
                evidence.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    require(
        status.success(),
        &format!(
            "Relocated Player {mode} failed: {}",
            fs::read_to_string(&stderr).unwrap_or_default()
        ),
    )?;
    let report: Value = serde_json::from_slice(&read_bytes(&stdout)?)
        .map_err(|error| format!("Player stdout report: {error}"))?;
    for field in [
        "package_hashes_verified",
        "exact_contract_lock_verified",
        "authored_project_bytes_unchanged",
    ] {
        require(
            report.get(field).and_then(Value::as_bool) == Some(true),
            &format!("Player did not verify {field}"),
        )?;
    }
    require(
        report["source_compilation"] == json!(false) && report["services_enabled"] == json!(false),
        "Relocated verification used source compilation or live services",
    )?;
    if mode == "--headless-check" {
        require(
            report["headless"] == json!(true)
                && report["frame"].is_null()
                && report["device"] == json!("Headless"),
            "Headless Player allocated a graphical renderer",
        )?;
    } else {
        require(
            report["headless"] == json!(false)
                && report["frame"]["stats"]["shaded_pixels"]
                    .as_u64()
                    .is_some_and(|pixels| pixels > 0),
            "Graphical Player produced no shaded 3D frame",
        )?;
    }
    require(
        !read_bytes(&evidence.join("player-verification.json"))?.is_empty(),
        "Player did not save its verification report",
    )?;
    Ok(report)
}

fn export_relocate(
    document: &GraphDocument,
    source: &Path,
    distribution: &Distribution,
    case: &Path,
    name: &str,
    cwd: &Path,
) -> Result<Value, String> {
    let asset_root = source.parent().ok_or("Author project asset root missing")?;
    let destination = case.join(format!("native-{name}"));
    let exported = crate::game_package::export(
        document,
        asset_root,
        &distribution.package_root,
        &distribution.compile_root()?,
        &destination,
    )?;
    let manifest: crate::game_package::PlayerManifest =
        serde_json::from_slice(&read_bytes(&exported.join("player.json"))?)
            .map_err(|error| error.to_string())?;
    let packaged = crate::game_package::validate_package(&exported)?;
    let assets = AssetManifest::from_value(&packaged.asset_manifest)?;
    require(
        !exported.join(".sdk").exists()
            && !exported.join(".tools").exists()
            && !exported.join("crates").exists()
            && !exported.join("Cargo.toml").exists()
            && !exported.join("Cargo.lock").exists()
            && !exported.join("engine-sdk.json").exists()
            && !exported
                .join(crate::distribution::EDITOR_EXECUTABLE)
                .exists(),
        "Standalone Player contains authoring/compiler/SDK artifacts",
    )?;
    let is_puzzle = name == "puzzle";
    require(
        manifest
            .native_features
            .iter()
            .any(|feature| feature == if is_puzzle { "puzzle" } else { "arpg" }),
        "Native Player lacks its selected project package",
    )?;
    require(
        !manifest
            .native_features
            .iter()
            .any(|feature| feature == if is_puzzle { "arpg" } else { "puzzle" }),
        "Native Player compiled an unrelated game project",
    )?;
    if is_puzzle {
        require(
            !manifest
                .native_features
                .iter()
                .any(|feature| feature == "audio")
                && assets.assets.is_empty()
                && manifest
                    .files
                    .keys()
                    .all(|path| !path.to_ascii_lowercase().ends_with(".wav")),
            "Puzzle Player retained audio or unrelated authored assets",
        )?;
    }
    let before = file_hashes(&exported)?;
    let relocated = case.join(format!("relocated {name} package"));
    copy_package(&exported, &relocated)?;
    require(
        file_hashes(&relocated)? == before,
        "Relocation changed Player artifact bytes",
    )?;
    let graphical = native_cli(
        &relocated,
        "--verify-package",
        &case.join(format!("{name}-native-graphical")),
        cwd,
    )?;
    let headless = native_cli(
        &relocated,
        "--headless-check",
        &case.join(format!("{name}-native-headless")),
        cwd,
    )?;
    require(
        file_hashes(&relocated)? == before && file_hashes(&exported)? == before,
        "Native verification modified the original or relocated package",
    )?;
    require(
        graphical["native_features"] == json!(manifest.native_features)
            && headless["native_features"] == json!(manifest.native_features),
        "Native executable fingerprint differs from Player manifest",
    )?;
    Ok(
        json!({"exported":exported,"relocated":relocated,"copied_artifacts_sha256":before,"manifest_sha256":hash_file(&relocated.join("player.json"))?,"player_sha256":hash_file(&relocated.join(module_engine_player::package::PLAYER_EXECUTABLE))?,"native_features":manifest.native_features,"native_platform":manifest.native_platform,"assets":assets,"omissions":{"editor_executable":true,"sdk_sources_and_metadata":true,"rust_toolchain":true,"cargo_manifests":true,"unrelated_game_native_feature":true,"puzzle_audio":is_puzzle},"graphical":graphical,"headless":headless,"original_and_relocated_artifact_hashes_unchanged":true}),
    )
}

/// The caller supplies the real legacy ARPG JSON and a separate evidence folder.
/// Publication uses checked existing APIs, and native exports build the selected
/// Player features through the production exporter. Root owns invoking this CLI.
pub fn run(source: &Path, output: &Path) -> Result<Value, String> {
    let raw = read_bytes(source)?;
    let legacy = module_engine_project::inspect_legacy_document(&raw)?;
    let original = SourceEvidence::new(source, &legacy)?;
    let strict_error = project::read(source)
        .err()
        .ok_or("Normal read silently accepted legacy format")?;
    require(
        strict_error.contains("Migration required"),
        "Normal legacy read lacked an explicit migration diagnostic",
    )?;
    original.verify()?;
    let distribution = distribution::detect(source, &legacy, None)?;
    require(
        distribution.can_author_engine(),
        "Stage7 verification requires the checked complete Full source package",
    )?;
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    original.check_report_destination(&output.join("engine-profile-report.json"))?;
    let case = output.join(format!(
        "engine-profile-case-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos()
    ));
    fs::create_dir(&case).map_err(|error| error.to_string())?;
    let case = case.canonicalize().map_err(|error| error.to_string())?;
    let result = (|| {
        let before_bosses = boss_values(&legacy, true, &distribution.package_root)?;
        require(
            !before_bosses.is_empty(),
            "Supplied ARPG fixture has no legacy boss rule to preserve",
        )?;
        let (migrated, migration) = workspace_files::migrate_copy(
            source,
            &case.join("migrated-arpg/project.json"),
            ProfileKind::Arpg,
        )?;
        let reopened = project::read(&migrated.path)?;
        require(
            reopened == migrated.document,
            "Explicitly migrated project did not reopen unchanged",
        )?;
        require(
            reopened.custom_sources == legacy.custom_sources,
            "Migration rewrote authored Custom source",
        )?;
        require(
            boss_values(&reopened, false, &distribution.package_root)? == before_bosses,
            "Migration changed the effective boss victory rule",
        )?;
        let capability = distribution::detect(&migrated.path, &reopened, None)?;
        let rejected_legacy = rejected_legacy_contract(&raw, &reopened, &capability, &case)?;
        let rejected = rejected_publications(&reopened, &migrated.path, &capability, &case)?;
        let controller = controller_compatibility(
            &reopened,
            migrated.path.parent().ok_or("Migrated root missing")?,
            &capability,
        )?;
        let empty = workspace_files::new_project_with_choice(
            &migrated.path,
            &case.join("new-empty-inherited"),
            &reopened,
            &capability,
            ProjectTemplate::Empty,
            EngineChoice::InheritCurrent,
        )?;
        let profile = EngineProfile::from_document(&reopened)?;
        let empty_game = empty
            .document
            .game
            .as_ref()
            .ok_or("New Empty has no scene")?;
        require(
            empty_game.execution == profile.blueprint.default_execution
                && empty_game
                    .execution
                    .stages
                    .iter()
                    .flat_map(|stage| &stage.nodes)
                    .all(|node| !node.module.starts_with("game.")),
            "Empty inherited implicit ARPG gameplay",
        )?;
        require(
            !empty
                .document
                .custom_sources
                .contains_key("game-controller")
                && EngineProfile::from_document(&empty.document)?.identity == profile.identity,
            "Empty copied project controller or changed inherited identity",
        )?;
        project::save(&empty.path, &empty.document)?;
        require(
            project::read(&empty.path)? == empty.document,
            "Empty Save/reopen changed profile/data",
        )?;
        let puzzle = workspace_files::new_project_with_choice(
            &migrated.path,
            &case.join("new-puzzle-selected"),
            &reopened,
            &capability,
            ProjectTemplate::Puzzle,
            EngineChoice::Selected(ProfileKind::Puzzle),
        )?;
        project::save(&puzzle.path, &puzzle.document)?;
        let puzzle_document = project::read(&puzzle.path)?;
        require(
            puzzle_document == puzzle.document
                && EngineProfile::from_document(&puzzle_document)?.identity.id
                    == "module-engine.puzzle",
            "Selected Puzzle Save/reopen changed identity/data",
        )?;
        let cwd = case.join("unrelated process cwd");
        fs::create_dir(&cwd).map_err(|error| error.to_string())?;
        let puzzle_player = export_relocate(
            &puzzle_document,
            &puzzle.path,
            &capability,
            &case,
            "puzzle",
            &cwd,
        )?;
        let arpg_player =
            export_relocate(&reopened, &migrated.path, &capability, &case, "arpg", &cwd)?;
        Ok::<_, String>(
            json!({"schema_version":1,"kind":"engine-profile-migration-native-player-proof","functional_checks_passed":true,"case":case,"strict_legacy_rejection":strict_error,"migration":{"report":migration,"copy_path":migrated.path,"copy_sha256":hash_file(&migrated.path)?,"engine_profile":EngineProfile::from_document(&reopened)?.identity,"boss_effective_values":before_bosses,"authored_custom_text_unchanged":true,"reopened_equal":true},"rejected_unknown_active_legacy_contract":rejected_legacy,"rejected_publications":rejected,"authored_controller_compatibility":controller,"new_empty":{"path":empty.path,"inherited_profile":profile.identity,"execution":empty_game.execution,"project_controller_omitted":true,"saved_reopened_equal":true},"new_puzzle":{"path":puzzle.path,"profile":EngineProfile::from_document(&puzzle_document)?.identity,"saved_reopened_equal":true},"native_puzzle":puzzle_player,"native_arpg":arpg_player,"source":original.verify()?}),
        )
    })();
    let source_evidence = original.verify()?;
    match result {
        Ok(report) => {
            write_json(&case.join("engine-profile-report.json"), &report)?;
            write_json(&output.join("engine-profile-report.json"), &report)?;
            Ok(report)
        }
        Err(error) => {
            let report = json!({"functional_checks_passed":false,"case":case,"error":error,"source":source_evidence});
            write_json(&case.join("engine-profile-report.json"), &report)?;
            write_json(&output.join("engine-profile-report.json"), &report)?;
            Err(error)
        }
    }
}
