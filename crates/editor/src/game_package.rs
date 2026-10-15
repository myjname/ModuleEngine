//! Standalone game export: runtime executable, authored data, assets and compiled code.
use crate::game_host::{render_graph, script_ids};
use module_engine_content::ContentLibrary;
use module_engine_core::GraphDocument;
use module_engine_project::{AssetManifest, hash_file, resolve_relative};
use module_engine_script::{ScriptCompiler, ScriptKind};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub use module_engine_player::package::PlayerManifest;
use module_engine_player::package::{CARGO_PLAYER_EXECUTABLE, PLAYER_EXECUTABLE, native_platform};

pub(crate) fn selected_features(
    document: &GraphDocument,
    backend: &Path,
) -> Result<BTreeSet<String>, String> {
    let composition = crate::host::composition_at(backend)?;
    crate::engine_profiles::validate(document, backend)?;
    let activity = composition.activity(document)?;
    let mut features = BTreeSet::from(["ui".into()]);
    let required = composition.required_contracts(document)?;
    let builtin = module_engine_core::SystemRegistry::builtin();
    for (feature, catalog) in [
        ("arpg", module_engine_arpg::registry()),
        ("puzzle", module_engine_project_puzzle::registry()),
    ] {
        if required
            .module_versions
            .keys()
            .any(|id| builtin.module_version(id).is_none() && catalog.module_version(id).is_some())
        {
            features.insert(feature.into());
        }
    }
    if activity.modules.contains("render.vulkan") {
        features.insert("vulkan".into());
    }
    if let Some(game) = &document.game {
        for contribution in composition
            .systems
            .active_game_contributions(&game.execution)
        {
            match contribution.id.as_str() {
                "arpg.presentation" => {
                    features.insert("arpg".into());
                }
                "puzzle.presentation" => {
                    features.insert("puzzle".into());
                }
                _ => {}
            }
            for request in (contribution.service_requests)(game)? {
                if request.id == "audio.wav" {
                    features.insert("audio".into());
                } else {
                    return Err(format!(
                        "Player has no compiled feature mapping for service '{}'",
                        request.id
                    ));
                }
            }
        }
    }
    let subset = module_engine_player::composition_for_features(backend, &features)?;
    module_engine_project::EngineProfile::from_document(document)?.validate_with_sdk(
        document,
        &subset,
        Some(&module_engine_core::SdkIdentity::current()),
    )?;
    Ok(features)
}
#[cfg(test)]
pub fn native_feature_key(document: &GraphDocument, backend: &Path) -> Result<String, String> {
    Ok(selected_features(document, backend)?
        .into_iter()
        .collect::<Vec<_>>()
        .join("-"))
}
/// Build a real feature-selected binary from the checked Full sources, or use
/// the matching checked native Player delivered with a Team installation.
pub fn build_native_player(document: &GraphDocument, backend: &Path) -> Result<PathBuf, String> {
    build_native_player_variant(document, backend, selected_features(document, backend)?)
}
/// Team data may add/remove cues without changing its frozen audio system.
/// Deliver both exact native variants on hosts that implement audio. A host
/// without playback may package only projects that explicitly request no audio.
pub fn team_feature_sets(
    document: &GraphDocument,
    backend: &Path,
) -> Result<Vec<BTreeSet<String>>, String> {
    let base = selected_features(document, backend)?;
    require_native_service_support(&base)?;
    let activity = crate::host::composition_at(backend)?.activity(document)?;
    if activity.modules.contains("game.audio")
        && module_engine_host_services::WAV_PLAYBACK_SUPPORTED
    {
        let mut without = base.clone();
        without.remove("audio");
        let mut with = base;
        with.insert("audio".into());
        Ok(vec![without, with])
    } else {
        Ok(vec![base])
    }
}
pub fn team_player_variants(
    document: &GraphDocument,
    backend: &Path,
) -> Result<Vec<(String, PathBuf)>, String> {
    team_feature_sets(document, backend)?
        .into_iter()
        .map(|features| {
            let key = features.iter().cloned().collect::<Vec<_>>().join("-");
            Ok((
                key,
                build_native_player_variant(document, backend, features)?,
            ))
        })
        .collect()
}
fn build_native_player_variant(
    document: &GraphDocument,
    backend: &Path,
    features: BTreeSet<String>,
) -> Result<PathBuf, String> {
    require_native_service_support(&features)?;
    let subset = module_engine_player::composition_for_features(backend, &features)?;
    module_engine_project::EngineProfile::from_document(document)?.validate_with_sdk(
        document,
        &subset,
        Some(&module_engine_core::SdkIdentity::current()),
    )?;
    let key = features.iter().cloned().collect::<Vec<_>>().join("-");
    let prebuilt = backend.join("players").join(&key).join(PLAYER_EXECUTABLE);
    if prebuilt.is_file() {
        let manifest: crate::distribution::PackageManifest = serde_json::from_slice(
            &fs::read(backend.join(crate::distribution::MANIFEST_NAME))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let relative = format!("players/{key}/{PLAYER_EXECUTABLE}");
        if manifest.backend_files.get(&relative) != Some(&hash_file(&prebuilt)?) {
            return Err("Prebuilt Player is not covered by the checked backend inventory".into());
        }
        return Ok(prebuilt);
    }
    if !backend.join("crates/player/Cargo.toml").is_file() {
        return Err(format!(
            "This installation lacks the '{key}' native Player; rebuild the Team package from Full sources"
        ));
    }
    let build_root = native_cache_root(backend, "player-build");
    fs::create_dir_all(&build_root).map_err(|e| e.to_string())?;
    // One shared dependency target; serialize build + immutable snapshot creation
    // across exporting editor/CLI processes. Windows closes this lease on crash.
    #[cfg(windows)]
    let _build_lease = {
        use std::os::windows::fs::OpenOptionsExt;
        let started = std::time::Instant::now();
        loop {
            match fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(build_root.join("build.lock"))
            {
                Ok(file) => break file,
                Err(error)
                    if error.raw_os_error() == Some(32)
                        && started.elapsed() < std::time::Duration::from_secs(600) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(50))
                }
                Err(error) => return Err(format!("Native Player build lease: {error}")),
            }
        }
    };
    // Cargo serializes compilation, but its lock ends before we hash/copy the
    // result. Hold our own lease through immutable snapshot publication too.
    #[cfg(unix)]
    let _build_lease = {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(build_root.join("build.lock"))
            .map_err(|error| format!("Native Player build lease: {error}"))?;
        let started = std::time::Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => break file,
                Err(fs::TryLockError::WouldBlock)
                    if started.elapsed() < std::time::Duration::from_secs(600) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(50))
                }
                Err(error) => return Err(format!("Native Player build lease: {error}")),
            }
        }
    };
    let compiler = ScriptCompiler::new(backend.into(), build_root.clone());
    let target = native_cache_root(backend, "player-native-target");
    let mut command = std::process::Command::new(
        compiler
            .cargo_binary
            .as_deref()
            .unwrap_or_else(|| Path::new("cargo")),
    );
    command
        .current_dir(backend)
        .args([
            "build",
            "--offline",
            "--locked",
            "--release",
            "-p",
            "module-engine-player",
            "--bin",
            "module-engine-player",
            "--no-default-features",
            "--features",
        ])
        .arg(features.iter().cloned().collect::<Vec<_>>().join(","))
        .arg("--target-dir")
        .arg(&target);
    if let Some(path) = compiler.cargo_home {
        command.env("CARGO_HOME", path);
    }
    if let Some(path) = compiler.rustc_binary {
        command.env("RUSTC", path);
    }
    if let Some(path) = compiler.rustup_home {
        command.env("RUSTUP_HOME", path);
    }
    if let Some(toolchain) = compiler.rustup_toolchain {
        command.env("RUSTUP_TOOLCHAIN", toolchain);
    }
    command.env("CARGO_INCREMENTAL", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command
        .output()
        .map_err(|e| format!("Player Cargo launch: {e}"))?;
    let log = String::from_utf8_lossy(&output.stderr);
    fs::create_dir_all(&build_root).map_err(|e| e.to_string())?;
    fs::write(
        build_root.join(format!("{key}.log")),
        output.stderr.as_slice(),
    )
    .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("Selected Player build failed: {log}"));
    }
    let executable = target.join("release").join(CARGO_PLAYER_EXECUTABLE);
    if !executable.is_file() {
        return Err("Cargo did not produce the selected Player executable".into());
    }
    let snapshot = build_root
        .join(&key)
        .join(hash_file(&executable)?)
        .join(PLAYER_EXECUTABLE);
    if !snapshot.is_file() {
        fs::create_dir_all(snapshot.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(&executable, &snapshot).map_err(|e| e.to_string())?;
    }
    if hash_file(&snapshot)? != hash_file(&executable)? {
        return Err("Native Player snapshot integrity mismatch".into());
    }
    Ok(snapshot)
}

fn native_cache_root(backend: &Path, directory: &str) -> PathBuf {
    let root = backend.join(".work").join(directory);
    // A Windows checkout can also be used through WSL. Keep its already tested
    // Windows cache in place, and isolate non-Windows host Cargo outputs/leases.
    if cfg!(windows) {
        root
    } else {
        root.join(native_platform())
    }
}

/// Feature closure and platform capability are distinct. Never silently remove
/// a requested service to produce a package that only works in its verifier,
/// where external effects are intentionally disabled.
fn require_native_service_support(features: &BTreeSet<String>) -> Result<(), String> {
    if features.contains("audio") && !module_engine_host_services::WAV_PLAYBACK_SUPPORTED {
        return Err(format!(
            "Native Player export requests host service 'audio.wav', but native WAV playback is unavailable on {}. Explicitly remove the audio request/cues or choose an Engine profile without this service; the exporter will not change authored data or silently disable sound.",
            std::env::consts::OS
        ));
    }
    Ok(())
}
fn select_player_assets(document: &mut GraphDocument, backend: &Path) -> Result<(), String> {
    let composition = crate::host::composition_at(backend)?;
    let Some(game) = document.game.as_ref() else {
        return Ok(());
    };
    let mut references = crate::host::component_asset_references(document, backend)?;
    let mut requests = vec![];
    for contribution in composition
        .systems
        .active_game_contributions(&game.execution)
    {
        requests.extend((contribution.service_requests)(game)?);
    }
    for request in &requests {
        references.extend(request.asset_references.iter().cloned());
    }
    // Arbitrary Rust can construct asset IDs dynamically. Its declared manifest
    // remains conservative until source modules provide a narrower contract.
    if script_ids(game).is_empty() {
        let mut manifest = AssetManifest::from_value(&document.asset_manifest)?;
        let paths = references
            .iter()
            .map(|r| manifest.resolve_reference(r))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let mut retained = manifest
            .assets
            .iter()
            .filter(|(id, entry)| {
                references.contains(&format!("asset://{id}")) || paths.contains(&entry.path)
            })
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        let mut pending = retained.iter().cloned().collect::<Vec<_>>();
        while let Some(id) = pending.pop() {
            for dependency in &manifest.assets[&id].dependencies {
                if !manifest.assets.contains_key(dependency) {
                    return Err(format!("Asset {id}: missing dependency {dependency}"));
                }
                if retained.insert(dependency.clone()) {
                    pending.push(dependency.clone());
                }
            }
        }
        manifest.assets.retain(|id, _| retained.contains(id));
        document.asset_manifest = serde_json::to_value(manifest).map_err(|e| e.to_string())?;
    }
    if !requests.iter().any(|request| request.id == "audio.wav") {
        document.game.as_mut().unwrap().audio_cues.clear();
    }
    Ok(())
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

#[cfg(target_os = "linux")]
fn publish_new_directory(stage: &Path, destination: &Path) -> Result<(), String> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(stage.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    let target =
        CString::new(destination.as_os_str().as_bytes()).map_err(|error| error.to_string())?;
    // Never replace a racing empty directory: exists()+rename() is not atomic
    // on Unix. An unsupported kernel/filesystem fails instead of weakening the
    // established no-replace publication guarantee.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)
        ) {
            return Err(format!(
                "Player destination filesystem/kernel does not support atomic no-replace directory publication ({error}). Choose a native Linux filesystem such as ext4 instead of this mounted filesystem. Export refused without falling back to replacement."
            ));
        }
        return Err(format!(
            "Player destination was not published atomically without replacement: {error}"
        ));
    }
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux")))]
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
    crate::engine_profiles::validate(document, backend_root)?;
    crate::host::validate_document_at(document, backend_root)?;
    if document.game.is_some() {
        prepared.game = Some(crate::host::prepare_game_at(document, backend_root)?);
    }
    select_player_assets(&mut prepared, backend_root)?;
    let features = selected_features(&prepared, backend_root)?;
    require_native_service_support(&features)?;
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
    if features.contains("vulkan") {
        module_engine_vulkan::precompile_graph_at(
            &render_graph(document),
            &document.shader_sources,
            &backend_root,
            &destination,
        )?;
    }
    let player = build_native_player(document, &backend_root)?;
    fs::copy(player, destination.join(PLAYER_EXECUTABLE)).map_err(|e| e.to_string())?;
    fs::write(
        destination.join("project.json"),
        serde_json::to_vec_pretty(document).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(destination.join("README.txt"),format!("ModuleEngine standalone Player\nLaunch ./{PLAYER_EXECUTABLE}. Rust, Cargo, DXC and engine sources are not required.\nNative platform: {}. Packages are not cross-platform binaries.\nSelected native features: {}\nR restarts; Escape closes. Gameplay controls are shown by the selected project.\nAn installed compatible GPU driver is required for GPU rendering; Linux GUI also requires a working Wayland or X11 session and the system runtime libraries used by this native build.\n",native_platform(),features.iter().cloned().collect::<Vec<_>>().join(", "))).map_err(|e|e.to_string())?;
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
        schema_version: 2,
        native_features: features.into_iter().collect(),
        sdk_data_api_fingerprint: module_engine_core::SDK_DATA_API_FINGERPRINT.into(),
        engine_version: env!("CARGO_PKG_VERSION").into(),
        project: "project.json".into(),
        files: hashes,
        native_platform: Some(native_platform()),
    };
    fs::write(
        destination.join("player.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // Read from the actual exported root, with no compile root and sealed GPU artifacts.
    validate_package(&destination)?;
    verify_native_player(&destination)?;
    Ok(destination)
}

/// Verify the copied binary itself, including its exact feature fingerprint.
/// Output stays outside the sealed artifact. The child never receives a compiler root.
fn verify_native_player(root: &Path) -> Result<(), String> {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let prefix = std::env::temp_dir().join(format!(
        "module-player-verify-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let output_path = prefix.with_extension("stdout");
    let error_path = prefix.with_extension("stderr");
    struct TemporaryLogs([PathBuf; 2]);
    impl Drop for TemporaryLogs {
        fn drop(&mut self) {
            for path in &self.0 {
                let _ = fs::remove_file(path);
            }
        }
    }
    let _temporary_logs = TemporaryLogs([output_path.clone(), error_path.clone()]);
    let mut command = Command::new(root.join(PLAYER_EXECUTABLE));
    command
        .arg("--verify-package")
        .arg(root)
        .current_dir(root)
        .stdout(Stdio::from(
            fs::File::create_new(&output_path).map_err(|e| e.to_string())?,
        ))
        .stderr(Stdio::from(
            fs::File::create_new(&error_path).map_err(|e| e.to_string())?,
        ));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Copied Player launch: {e}"))?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(40) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Copied Player verification exceeded 40 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let error = fs::read_to_string(&error_path).unwrap_or_default();
    if !status.success() {
        return Err(format!("Copied Player verification failed: {error}"));
    }
    Ok(())
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
            | "player"
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
    for reference in crate::host::component_asset_references(document, backend_root)? {
        assets.insert(manifest.resolve_reference(&reference)?);
    }
    for entity in crate::host::active_game_entities_at(document, backend_root)? {
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
    assets.extend(library.dependencies(models)?);
    let composition = crate::host::composition_at(backend_root)?;
    for contribution in composition
        .systems
        .active_game_contributions(&game.execution)
    {
        for request in (contribution.service_requests)(game)? {
            for reference in request.asset_references {
                assets.insert(manifest.resolve_reference(&reference)?);
            }
        }
    }
    manifest.verify(asset_root)?;
    assets.extend(manifest.assets.values().map(|e| e.path.clone()));
    let mut backend = BTreeSet::new();
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
    module_engine_player::package::validate_package_metadata(root)
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
        // Publication tests need the atomic rename guarantees of their native
        // filesystem. A WSL source checkout can live on DrvFS, which rejects
        // RENAME_NOREPLACE; TMPDIR points these disposable cases at native Linux
        // storage without changing the actual backend or weakening export.
        let root = if cfg!(target_os = "linux") {
            std::env::temp_dir()
        } else {
            backend_root().join(".work")
        };
        let folder = root.join(format!(
            "player-plan-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
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
    fn native_build_cache_keeps_windows_path_and_isolates_unix_hosts() {
        let backend = Path::new("backend");
        let cache = native_cache_root(backend, "player-native-target");
        #[cfg(windows)]
        assert_eq!(cache, backend.join(".work/player-native-target"));
        #[cfg(unix)]
        assert_eq!(
            cache,
            backend
                .join(".work/player-native-target")
                .join(native_platform())
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_requested_audio_rejects_export_without_mutating_the_project() {
        let backend = backend_root();
        let document = crate::game_host::demo_document();
        let before = serde_json::to_vec(&document).unwrap();
        assert!(
            selected_features(&document, &backend)
                .unwrap()
                .contains("audio")
        );
        let folder = fixture();
        let destination = folder.join("unsupported audio export");
        let error = export(&document, &backend, &backend, &backend, &destination).unwrap_err();
        assert!(
            error.contains("audio.wav") && error.contains("Explicitly remove"),
            "{error}"
        );
        assert!(!destination.exists());
        assert_eq!(serde_json::to_vec(&document).unwrap(), before);
        assert!(!fs::read_dir(&folder).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".module-engine-game-export-")
        }));
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
                .join(format!(
                    "scripts/game-controller/worker{}",
                    std::env::consts::EXE_SUFFIX
                ))
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
        #[cfg(any(windows, target_os = "linux"))]
        {
            let empty_racer = folder.join("racing-empty-directory");
            fs::create_dir(&empty_racer).unwrap();
            assert!(publish_new_directory(&stage, &empty_racer).is_err());
            assert!(fs::read_dir(&empty_racer).unwrap().next().is_none());
            assert!(stage.join("new.txt").is_file());
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
        doc.shader_sources.insert(
            "include-contract".into(),
            "#include \"forward.hlsl\"\n".into(),
        );
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
    fn selected_player_assets_keep_transitive_manifest_dependencies_without_custom() {
        let backend = backend_root();
        let artifacts = module_engine_script::TestArtifactGuard::new(
            &backend.join(".work/test-temp"),
            "player-dependency-closure",
        )
        .unwrap();
        let resources = artifacts.path();
        fs::create_dir_all(resources.join("assets/models")).unwrap();
        fs::create_dir_all(resources.join("assets/data")).unwrap();
        fs::copy(
            backend.join("assets/models/demo_crystal.glb"),
            resources.join("assets/models/demo_crystal.glb"),
        )
        .unwrap();
        for name in ["buffer.bin", "metadata.json", "unused.bin"] {
            fs::write(resources.join("assets/data").join(name), name.as_bytes()).unwrap();
        }
        let mut manifest = AssetManifest::default();
        for (id, path) in [
            ("model", "assets/models/demo_crystal.glb"),
            ("model-alias", "assets/models/demo_crystal.glb"),
            ("buffer", "assets/data/buffer.bin"),
            ("metadata", "assets/data/metadata.json"),
            ("unused", "assets/data/unused.bin"),
        ] {
            manifest.register(id, path, resources).unwrap();
        }
        manifest.assets.get_mut("model").unwrap().dependencies = vec!["buffer".into()];
        manifest.assets.get_mut("buffer").unwrap().dependencies = vec!["metadata".into()];
        // Legal manifest cycles are bounded by visited IDs, not traversal depth.
        manifest.assets.get_mut("metadata").unwrap().dependencies = vec!["buffer".into()];
        manifest.verify(resources).unwrap();
        for reference in ["asset://model", "assets/models/demo_crystal.glb"] {
            let mut authored = crate::game_host::puzzle_document();
            let game = authored.game.as_mut().unwrap();
            assert!(script_ids(game).is_empty());
            game.scenes.get_mut("board").unwrap().objects[0]
                .components
                .insert("mesh".into(), serde_json::json!({"path":reference}));
            authored.asset_manifest = serde_json::to_value(&manifest).unwrap();
            let mut prepared = authored.clone();
            select_player_assets(&mut prepared, &backend).unwrap();
            let selected = AssetManifest::from_value(&prepared.asset_manifest).unwrap();
            assert_eq!(
                selected
                    .assets
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["buffer", "metadata", "model", "model-alias"]
            );
            selected.verify(resources).unwrap();
            let plan = prepare_files(
                &prepared,
                prepared.game.as_ref().unwrap(),
                resources,
                &backend,
            )
            .unwrap();
            for path in [
                "assets/models/demo_crystal.glb",
                "assets/data/buffer.bin",
                "assets/data/metadata.json",
            ] {
                assert!(
                    plan.contains_key(path),
                    "missing exported dependency {path}"
                );
            }
            assert!(!plan.contains_key("assets/data/unused.bin"));
            assert_eq!(prepared.game, authored.game);
            assert!(
                AssetManifest::from_value(&authored.asset_manifest)
                    .unwrap()
                    .assets
                    .contains_key("unused")
            );
        }
    }

    #[test]
    fn generated_player_paths_are_rejected_before_creating_destination() {
        let backend = backend_root();
        for path in [
            "Player.EXE",
            "Player",
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
        document.shader_sources.insert(
            "include-contract".into(),
            "#include \"forward.hlsl\"\n".into(),
        );
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
        document.shader_sources.insert(
            "include-contract".into(),
            "#include \"forward.hlsl\"\n".into(),
        );
        let destination = folder.join("backend-player");
        assert!(
            export(&document, &resources, &backend, &backend, &destination)
                .unwrap_err()
                .contains("collide")
        );
        assert!(!destination.exists());
    }
}
