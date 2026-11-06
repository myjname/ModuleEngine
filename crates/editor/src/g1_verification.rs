//! Explicit G1 acceptance through the normal authored composition and services.
//! This diagnostic owns its fresh profile only; it never changes the input project.
use module_engine_core::{GameInput, GraphDocument};
use module_engine_g1::{Direction, Phase, action_input, direction_input, profile, view};
use module_engine_host::{GameHost, HostOptions, RenderMode};
use module_engine_project::{hash_bytes, hash_file};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn tick(host: &mut GameHost, input: &GameInput) -> Result<(), String> {
    require(
        host.step(1. / 60., input)? == 1,
        "Expected exactly one accepted G1 tick",
    )
}
fn action(host: &mut GameHost, name: &str) -> Result<(), String> {
    tick(host, &action_input(name))
}
fn build(
    document: &GraphDocument,
    root: &Path,
    backend: &Path,
    state: &Path,
) -> Result<GameHost, String> {
    GameHost::build(
        document.clone(),
        crate::host::composition_at(backend)?,
        HostOptions {
            asset_root: root.into(),
            backend_root: Some(backend.into()),
            compile_root: None,
            render: RenderMode::Graph,
            services: module_engine_player::service_registry_at(state.into())?,
            services_enabled: true,
        },
        None,
    )
}
fn image(host: &mut GameHost, folder: &Path, label: &str) -> Result<Value, String> {
    let frame = host.render(960, 540)?;
    require(
        frame.gpu.is_none() && frame.is_valid(),
        "Diagnostic did not return complete CPU pixels",
    )?;
    let distinct = frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    require(
        distinct > 32,
        "G1 render lacks expected geometry/color variation",
    )?;
    let path = folder.join(format!("{label}.png"));
    module_engine_player::write_png(&path, &frame)?;
    Ok(
        json!({"path":path,"sha256":hash_file(&path)?,"pixels_sha256":hash_bytes(&frame.rgba),
        "distinct_colors":distinct,"resolution":[frame.width,frame.height]}),
    )
}
fn save_path(root: &Path) -> Result<std::path::PathBuf, String> {
    let mut found = vec![];
    for namespace in fs::read_dir(root).map_err(|e| e.to_string())? {
        let namespace = namespace.map_err(|e| e.to_string())?;
        if namespace.file_type().map_err(|e| e.to_string())?.is_dir() {
            for entry in fs::read_dir(namespace.path()).map_err(|e| e.to_string())? {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    found.push(path);
                }
            }
        }
    }
    require(found.len() == 1, "Expected one fresh G1 profile slot")?;
    Ok(found.remove(0))
}

pub fn run(source: &Path, folder: &Path) -> Result<Value, String> {
    require(
        !folder.exists(),
        "G1 diagnostic output must be a new directory",
    )?;
    let document = crate::project::read(source)?;
    let distribution = crate::distribution::detect(source, &document, None)?;
    distribution.validate_document(&document)?;
    let backend = distribution.package_root;
    let root = source.parent().ok_or("G1 project has no parent")?;
    let source_sha = hash_file(source)?;
    let composition = crate::host::composition_at(&backend)?;
    let selected = module_engine_player::native_features::installed_native_features()?
        .select_document(&document, &composition)?;
    require(
        selected
            == ["ui", "g1", "vulkan", "audio", "persistence"]
                .map(String::from)
                .into_iter()
                .collect(),
        "G1 composition selected unrelated or omitted required native features",
    )?;
    fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let state = folder.join("profile");
    let mut host = build(&document, root, &backend, &state)?;
    tick(&mut host, &GameInput::default())?;
    require(
        view(host.session.state())?.phase == Phase::Menu,
        "G1 did not open main menu",
    )?;
    // Exercise the real mixer without a long loud test sequence. No claim of
    // physical audibility follows from submitted waveOut buffers.
    let mut volumes = action_input("g1.set_volumes");
    volumes
        .action_values
        .insert("g1.music_volume".into(), json!(0.));
    volumes
        .action_values
        .insert("g1.sfx_volume".into(), json!(0.));
    tick(&mut host, &volumes)?;
    action(&mut host, "g1.play")?;
    let first = image(&mut host, folder, "level01-start")?;
    let mut levels = vec![];
    for (index, level) in module_engine_g1::level_catalog().iter().enumerate() {
        require(
            view(host.session.state())?.current_level == index,
            "Campaign loaded the wrong board",
        )?;
        let before = image(&mut host, folder, &format!("level{:02}-start", index + 1))?;
        if index == 9 {
            volumes
                .action_values
                .insert("g1.music_volume".into(), json!(0.1));
            volumes
                .action_values
                .insert("g1.sfx_volume".into(), json!(0.2));
            tick(&mut host, &volumes)?;
            let slot = save_path(&state)?;
            let destination = folder
                .join("benchmark-profile")
                .join(slot.strip_prefix(&state).map_err(|e| e.to_string())?);
            fs::create_dir_all(
                destination
                    .parent()
                    .ok_or("Missing benchmark profile parent")?,
            )
            .map_err(|e| e.to_string())?;
            fs::copy(slot, destination).map_err(|e| e.to_string())?;
            volumes
                .action_values
                .insert("g1.music_volume".into(), json!(0.));
            volumes
                .action_values
                .insert("g1.sfx_volume".into(), json!(0.));
            tick(&mut host, &volumes)?;
        }
        for token in level.solution.chars() {
            let direction = match token {
                'N' => Direction::North,
                'E' => Direction::East,
                'S' => Direction::South,
                'W' => Direction::West,
                _ => return Err("Bad source witness".into()),
            };
            tick(&mut host, &direction_input(direction))?;
        }
        let current = view(host.session.state())?;
        require(
            current.phase
                == if index == 9 {
                    Phase::Complete
                } else {
                    Phase::Victory
                },
            "G1 witness failed actual Host",
        )?;
        require(
            current.moves == level.solution.len() as u64,
            "Successful move count differs from witness",
        )?;
        let after = image(&mut host, folder, &format!("level{:02}-solved", index + 1))?;
        require(
            before["pixels_sha256"] != after["pixels_sha256"],
            "Solved board pixels did not update",
        )?;
        levels.push(json!({"id":level.id,"moves":current.moves,"before":before,"solved":after}));
        if index < 9 {
            action(&mut host, "g1.next")?;
        }
    }
    require(
        profile(host.session.state())?.campaign.unlocked_count() == 10,
        "Campaign did not unlock all ten levels",
    )?;
    action(&mut host, "g1.menu")?;
    action(&mut host, "g1.levels")?;
    let mut choose = action_input("g1.select_level");
    choose
        .action_values
        .insert("g1.level_id".into(), json!("level03"));
    tick(&mut host, &choose)?;
    require(
        view(host.session.state())?.current_level == 2,
        "Replay selection did not select third level",
    )?;
    let token = module_engine_g1::level_solution("level03")
        .ok_or("Missing third witness")?
        .chars()
        .next()
        .ok_or("Empty witness")?;
    tick(
        &mut host,
        &direction_input(match token {
            'N' => Direction::North,
            'E' => Direction::East,
            'S' => Direction::South,
            _ => Direction::West,
        }),
    )?;
    let expected = profile(host.session.state())?;
    let service_before = host.services.diagnostics();
    drop(host);
    let mut restored = build(&document, root, &backend, &state)?;
    tick(&mut restored, &GameInput::default())?;
    require(
        profile(restored.session.state())? == expected,
        "Reopened profile differs from persisted campaign/attempt/history/settings",
    )?;
    action(&mut restored, "g1.continue")?;
    action(&mut restored, "g1.undo")?;
    require(
        view(restored.session.state())?.moves == 0,
        "Persisted Undo failed after process-style Host reload",
    )?;
    let services = restored.services.diagnostics();
    drop(restored);
    let slot = save_path(&state)?;
    let valid_save = fs::read(&slot).map_err(|e| e.to_string())?;
    let corrupt = b"{ intentionally truncated G1 profile";
    fs::write(&slot, corrupt).map_err(|e| e.to_string())?;
    let mut recovering = build(&document, root, &backend, &state)?;
    tick(&mut recovering, &GameInput::default())?;
    action(&mut recovering, "g1.play")?;
    require(
        fs::read(&slot).map_err(|e| e.to_string())? == corrupt,
        "Playing overwrote a corrupt profile without explicit reset",
    )?;
    let blocked = recovering.services.diagnostics();
    let mut reset = action_input("g1.reset_progress");
    reset
        .action_values
        .insert("g1.confirm_reset".into(), json!(true));
    tick(&mut recovering, &reset)?;
    let recovery = recovering.services.diagnostics();
    let recovery_path = recovery["persistence.json"]["recovery_file"]
        .as_str()
        .ok_or("Reset did not expose quarantine path")?;
    require(
        fs::read(recovery_path).map_err(|e| e.to_string())? == corrupt,
        "Recovery did not preserve exact corrupt bytes",
    )?;
    require(
        fs::read(&slot).map_err(|e| e.to_string())? != corrupt,
        "Explicit reset did not create a fresh profile",
    )?;
    drop(recovering);
    fs::write(
        folder.join("valid-profile-before-corruption.json"),
        valid_save,
    )
    .map_err(|e| e.to_string())?;
    require(
        hash_file(source)? == source_sha,
        "Diagnostic changed authored source",
    )?;
    let report = json!({"functional_checks_passed":true,"source":source,"source_sha256":source_sha,
        "native_features":selected,"levels":levels,"first_image":first,
        "services_before_reload":service_before,"services_after_undo":services,"blocked_save":blocked,
        "explicit_reset":recovery,"resume_and_persisted_undo":true,"authored_source_preserved":true,
        "physical_audibility_verified":false,
        "scope":"Actual authored Host, imported GLTF/texture, Vulkan diagnostic pixels, ten-level progression and native audio/persistence. Window/physical input/performance are separate checks."});
    fs::write(
        folder.join("g1-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}
