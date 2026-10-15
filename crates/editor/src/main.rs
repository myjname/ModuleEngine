//! Native ModuleEngine editor and deterministic command-line verification.

mod activity;
mod app;
mod assets_ui;
mod authoring;
mod canvas;
mod distribution;
mod engine_profile_verification;
mod engine_profiles;
mod game_host;
mod game_package;
mod game_templates;
mod game_ui;
mod game_verification;
mod game_worker;
mod game_worker_verification;
mod host;
mod layout;
mod native_dialogs;
mod player;
mod project;
mod puzzle_verification;
mod scalar;
mod scene_assets_verification;
mod scene_tools;
mod scene_view_ui;
mod settings_verification;
#[cfg(test)]
mod source_modules;
mod ui_capture;
mod verification;
mod workspace_files;

use module_engine_core::{
    Camera, ControlCommand, Frame, GraphDocument, ModuleRegistry, Runtime, ScriptContext,
    WorldKind, demo_document,
};
use module_engine_script::{CompiledScript, ScriptCompiler, ScriptKind};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

fn main() {
    let result = run();
    // The native window is already closed. Finish detached Game owners here
    // so their persistent Custom children are destroyed before process exit.
    let cleanup = game_worker::finish_after_ui();
    if let Err(error) = result.and(cleanup) {
        eprintln!("ModuleEngine: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let (args, requested_profile) = parse_profile(std::env::args().skip(1).collect())?;
    let player_root = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .filter(|p| p.join("player.json").is_file());
    if player_root.is_some()
        && args.first().is_some_and(|arg| {
            !matches!(
                arg.as_str(),
                "--play" | "--game-check" | "--player-benchmark" | "--help" | "-h"
            )
        })
    {
        return Err(
            "This is a standalone Player package. Editor/Engine commands are unavailable.".into(),
        );
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "ModuleEngine 0.4\n\n  module-engine-editor [project.json] [--profile full|team]\n  module-engine-editor --new-game game-project.json\n  module-engine-editor --play game-project.json\n  module-engine-editor --export-game game-project.json folder\n  module-engine-editor --game-check game-project.json [output-folder]\n  module-engine-editor --player-benchmark game-project.json [output-folder]\n  module-engine-editor --check project.json\n  module-engine-editor --render project.json output.png\n  module-engine-editor --export project.json folder\n  module-engine-editor --export-team project.json folder\n  module-engine-editor --self-check [output-folder]\n  module-engine-editor --integration-check [output-folder]\n  module-engine-editor --settings-check [output-folder]\n  module-engine-editor --editor-ui-check game-project.json output-folder\n\nMouse: drag nodes; output pin -> compatible input pin; double click compound.\n3D: drag to orbit; wheel to zoom. See README.md for Russian documentation."
        );
        println!(
            "  module-engine-editor --scene-assets-check game-project.json output-folder\n\nScene: G/R/S tools, MMB orbit, Shift+MMB pan, wheel zoom, F frame, Escape cancel."
        );
        println!("  module-engine-editor --editor-benchmark game-project.json output-folder");
        println!("  module-engine-editor --game-worker-check game-project.json output-folder");
        println!("  module-engine-editor --new-puzzle puzzle-project.json");
        println!(
            "  module-engine-editor --migrate-copy source.json new-project.json neutral|arpg|puzzle"
        );
        println!("  module-engine-editor --puzzle-check puzzle-project.json output-folder");
        return Ok(());
    }
    match args.first().map(String::as_str) {
        Some("--inspect-installed-contracts") => {
            let output = Path::new(
                args.get(1)
                    .ok_or("Provide contract inventory JSON destination")?,
            );
            fs::write(
                output,
                serde_json::to_vec_pretty(&engine_profiles::inspected_contracts(
                    &project::source_root(),
                )?)
                .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            return Ok(());
        }
        Some("--migrate-copy") => {
            let source = Path::new(args.get(1).ok_or("Provide legacy source JSON")?);
            let destination = Path::new(args.get(2).ok_or("Provide a different destination JSON")?);
            let kind = engine_profiles::ProfileKind::parse(
                args.get(3)
                    .ok_or("Explicitly select Engine: neutral, arpg or puzzle")?,
            )?;
            let (saved, report) = workspace_files::migrate_copy(source, destination, kind)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"path":saved.path,"migration":report})
                )
                .map_err(|e| e.to_string())?
            );
            return Ok(());
        }
        Some("--engine-profile-check") => {
            let source = Path::new(args.get(1).ok_or("Provide the legacy author project")?);
            let output = Path::new(args.get(2).ok_or("Provide a new evidence directory")?);
            println!(
                "{}",
                serde_json::to_string_pretty(&engine_profile_verification::run(source, output)?)
                    .map_err(|e| e.to_string())?
            );
            return Ok(());
        }
        Some("--puzzle-check") => {
            let source = Path::new(args.get(1).ok_or("Provide puzzle project JSON")?);
            let folder = Path::new(args.get(2).ok_or("Provide separate verification folder")?);
            println!(
                "{}",
                serde_json::to_string_pretty(&puzzle_verification::run(source, folder)?)
                    .map_err(|error| error.to_string())?
            );
            return Ok(());
        }
        Some("--new-puzzle") => {
            let path = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("puzzle-project.json"));
            project::save(&path, &game_templates::puzzle_document())?;
            println!("Authored puzzle project: {}", path.display());
            return Ok(());
        }
        Some("--game-worker-check") => {
            let source = Path::new(args.get(1).ok_or("Provide source project JSON")?);
            let output = Path::new(args.get(2).ok_or("Provide separate verification folder")?);
            println!(
                "{}",
                serde_json::to_string_pretty(&game_worker_verification::run(source, output)?)
                    .map_err(|e| e.to_string())?
            );
            return Ok(());
        }
        Some("--editor-benchmark") => {
            let path = PathBuf::from(args.get(1).ok_or("Provide a source project JSON")?);
            let folder = PathBuf::from(args.get(2).ok_or("Provide a separate benchmark folder")?);
            let document = project::read(&path)?;
            let package = distribution::detect(&path, &document, requested_profile)?;
            package.validate_document(&document)?;
            if !package.can_author_engine() {
                return Err(
                    "Editor graph benchmark requires Full with complete source files".into(),
                );
            }
            return app::performance::run(document, path, package, &folder);
        }
        Some("--scene-assets-check") => {
            let source = Path::new(args.get(1).ok_or("Provide project JSON")?);
            let output = Path::new(args.get(2).ok_or("Provide separate verification folder")?);
            println!(
                "{}",
                serde_json::to_string_pretty(&scene_assets_verification::run(source, output)?)
                    .map_err(|e| e.to_string())?
            );
            return Ok(());
        }
        Some("--player-benchmark") => {
            let path = PathBuf::from(args.get(1).ok_or("Provide a game project JSON")?);
            let folder = args
                .get(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("outputs/native-player-verification"));
            let root = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let (document, compile) = if root.join("player.json").is_file() {
                (game_package::validate_package(root)?, None)
            } else {
                let document = project::read(&path)?;
                let package = distribution::detect(&path, &document, requested_profile)?;
                package.validate_document(&document)?;
                (document, Some(package.compile_root()?))
            };
            let kind = game_host::world_kind(&document)?;
            return player::run_benchmark(document, root, compile.as_deref(), kind, &folder);
        }
        Some("--game-check") => {
            let path = PathBuf::from(args.get(1).ok_or("Provide a game project JSON")?);
            let folder = args
                .get(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("outputs/game-verification"));
            let root = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let (document, compile) = if root.join("player.json").is_file() {
                (game_package::validate_package(root)?, None)
            } else {
                let document = project::read(&path)?;
                let package = distribution::detect(&path, &document, requested_profile)?;
                package.validate_document(&document)?;
                (document, Some(package.compile_root()?))
            };
            let report = game_verification::run(&document, root, compile.as_deref(), &folder)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            return Ok(());
        }
        Some("--new-game") => {
            let path = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("game-project.json"));
            project::save(&path, &game_host::demo_document())?;
            println!("Authored game project: {}", path.display());
            return Ok(());
        }
        Some("--editor-ui-check") => {
            let path = PathBuf::from(args.get(1).ok_or("Provide a source project JSON")?);
            let folder = PathBuf::from(args.get(2).ok_or("Provide a verification folder")?);
            let report = app::editor_workflow::verify_workflow(&path, &folder)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            return Ok(());
        }
        Some("--play") => {
            let path = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("game-project.json"));
            let root = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let (document, compile) = if root.join("player.json").is_file() {
                (game_package::validate_package(root)?, None)
            } else {
                let document = project::read(&path)?;
                let package = distribution::detect(&path, &document, requested_profile)?;
                package.validate_document(&document)?;
                (document, Some(package.compile_root()?))
            };
            let kind = game_host::world_kind(&document)?;
            return player::run(document, root, compile.as_deref(), kind);
        }
        Some("--export-game") => {
            let path = PathBuf::from(args.get(1).ok_or("Provide a game project JSON")?);
            let destination =
                PathBuf::from(args.get(2).ok_or("Provide an empty export directory")?);
            let document = project::read(&path)?;
            let package = distribution::detect(&path, &document, requested_profile)?;
            package.validate_document(&document)?;
            let output = game_package::export(
                &document,
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
                &package.package_root,
                &package.compile_root()?,
                &destination,
            )?;
            println!("Standalone Player: {}", output.join("Player.exe").display());
            return Ok(());
        }
        Some("--seal-source-project") => {
            let root = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
            let document: GraphDocument = serde_json::from_slice(
                &fs::read(root.join("project.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            distribution::write_full_manifest(&root, &document)?;
            println!("Complete source inventory recorded: {}", root.display());
            return Ok(());
        }
        Some("--capabilities") => {
            let path = args.get(1).ok_or("Укажите файл графа")?;
            let document = project::read(Path::new(path))?;
            let package = distribution::detect(Path::new(path), &document, requested_profile)?;
            let root = package
                .compile_root()
                .map(|p| p.to_string_lossy().into_owned());
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "version":env!("CARGO_PKG_VERSION"),"profile":package.profile,
                    "engine_available":package.can_author_engine(),
                    "package_root":package.package_root,"source_root":package.source_status.root,
                    "modified_sources":package.source_status.modified,
                    "source_diagnostics":package.source_status.diagnostics,
                    "diagnostics":package.diagnostics,"custom_compile_root":root,
                }))
                .map_err(|e| e.to_string())?
            );
            return Ok(());
        }
        Some("--settings-check") => {
            let folder = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("outputs/settings-verification"));
            let report = settings_verification::run(&folder)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            return Ok(());
        }
        Some("--integration-check") => {
            let folder = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("outputs/integration"));
            let report = verification::integration_self_check(&folder)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            return Ok(());
        }
        Some("--self-check") => {
            return self_check(
                args.get(1)
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("outputs/verification")),
            );
        }
        Some("--check") => {
            let path = args.get(1).ok_or("Укажите файл графа")?;
            let document = project::read(Path::new(path))?;
            let distribution = distribution::detect(Path::new(path), &document, requested_profile)?;
            distribution.validate_document(&document)?;
            if document.game.is_some() {
                let kind = game_host::world_kind(&document)?;
                let asset_root = Path::new(path)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                crate::game_host::new(
                    document,
                    asset_root,
                    Some(&distribution.compile_root()?),
                    kind,
                )?;
                println!("Valid authored game and executable module graph");
                return Ok(());
            }
            let runtime =
                Runtime::from_document(&document, &host::registry_at(&distribution.package_root)?)
                    .map_err(|e| e.to_string())?;
            let scripts = prepare_scripts_at(
                &document,
                &runtime,
                &distribution.compile_root()?,
                Path::new(path)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
            scalar::world_speed_compiled(
                &document,
                runtime.world_speed_source().as_deref(),
                0.0,
                &|id, time, base| compiled_speed(&scripts, id, time, base, &document),
            )?;
            println!(
                "Valid executable graph: {} ({})",
                document.name,
                runtime.world_module_id()
            );
            return Ok(());
        }
        Some("--render") => {
            let path = args.get(1).ok_or("Укажите файл графа")?;
            let out = args.get(2).ok_or("Укажите выходной PNG")?;
            reject_same_render_path(Path::new(path), Path::new(out))?;
            let document = project::read(Path::new(path))?;
            let distribution = distribution::detect(Path::new(path), &document, requested_profile)?;
            distribution.validate_document(&document)?;
            if document.game.is_some() {
                let kind = game_host::world_kind(&document)?;
                let mut host = crate::game_host::new(
                    document,
                    Path::new(path)
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new(".")),
                    Some(&distribution.compile_root()?),
                    kind,
                )?;
                host.services.set_enabled(false);
                for _ in 0..90 {
                    host.step(1. / 60., &module_engine_core::GameInput::default())?;
                }
                write_png(Path::new(out), &host.render(960, 640)?)?;
                println!("Rendered authored game: {out}");
                return Ok(());
            }
            let mut runtime =
                Runtime::from_document(&document, &host::registry_at(&distribution.package_root)?)
                    .map_err(|e| e.to_string())?;
            advance_at(
                &document,
                &mut runtime,
                90,
                &distribution.compile_root()?,
                Path::new(path)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
            let frame = runtime.render(960, 640, Camera::default())?;
            write_png(Path::new(out), &frame)?;
            println!("Rendered {}: {:?}", out, frame.stats);
            return Ok(());
        }
        Some("--export") => {
            let path = args.get(1).ok_or("Укажите файл графа")?;
            let destination = args.get(2).ok_or("Укажите папку экспорта")?;
            let document = project::read(Path::new(path))?;
            let distribution = distribution::detect(Path::new(path), &document, requested_profile)?;
            distribution.validate_document(&document)?;
            if !distribution.can_author_engine() {
                return Err("Экспорт полного движка требует профиля Full и полного набора совместимых исходников.".into());
            }
            let asset_root = Path::new(path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let exported = project::export_with_package_root(
                Path::new(destination),
                &document,
                asset_root,
                &distribution.package_root,
            )?;
            println!("Exported source project: {}", exported.display());
            return Ok(());
        }
        Some("--export-team") => {
            let path = args.get(1).ok_or("Укажите файл графа")?;
            let destination = args.get(2).ok_or("Укажите папку Team-поставки")?;
            let document = project::read(Path::new(path))?;
            let package = distribution::detect(Path::new(path), &document, requested_profile)?;
            package.validate_document(&document)?;
            if !package.can_author_engine() {
                return Err(
                    "Создание Team-поставки требует Full и полного набора совместимых исходников."
                        .into(),
                );
            }
            let asset_root = Path::new(path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let executable = std::env::current_exe().map_err(|e| e.to_string())?;
            let exported = project::export_team_with_package_root(
                Path::new(destination),
                &document,
                asset_root,
                &executable,
                &package.package_root,
            )?;
            println!("Exported Team package: {}", exported.display());
            return Ok(());
        }
        _ => {}
    }
    if args.is_empty()
        && let Ok(executable) = std::env::current_exe()
        && let Some(root) = executable.parent()
        && root.join("player.json").is_file()
    {
        let document = game_package::validate_package(root)?;
        let kind = game_host::world_kind(&document)?;
        return player::run(document, root, None, kind);
    }
    let path = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(default_project_path);
    let mut document = if path.exists() {
        project::read(&path)?
    } else {
        host::settings_demo_document(WorldKind::Objects)
    };
    let distribution = distribution::detect(&path, &document, requested_profile)?;
    distribution.validate_document(&document)?;
    if distribution.profile == distribution::DistributionProfile::Full {
        host::ensure_authoring_configuration(&mut document);
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1580.0, 940.0])
            .with_min_inner_size([1100.0, 720.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let open_custom = std::env::var_os("MODULE_ENGINE_SHOW_CUSTOM").is_some();
    eframe::run_native(
        "ModuleEngine / Engine Constructor",
        options,
        Box::new(move |cc| {
            app::configure_style(&cc.egui_ctx);
            let mut editor = app::Editor::new_with_distribution(document, path, distribution);
            if open_custom {
                editor.show_custom();
            }
            if let Ok(view) = std::env::var("MODULE_ENGINE_CAPTURE_VIEW") {
                editor.show_verification_view(&view);
            }
            Ok(Box::new(editor))
        }),
    )
    .map_err(|e| e.to_string())
}

fn parse_profile(
    mut args: Vec<String>,
) -> Result<(Vec<String>, Option<distribution::DistributionProfile>), String> {
    let mut profile = None;
    while let Some(index) = args.iter().position(|arg| arg == "--profile") {
        if profile.is_some() {
            return Err("Профиль указан больше одного раза.".into());
        }
        profile = Some(match args.get(index + 1).map(String::as_str) {
            Some("full") => distribution::DistributionProfile::Full,
            Some("team") => distribution::DistributionProfile::Team,
            _ => return Err("После --profile укажите full или team.".into()),
        });
        args.drain(index..=index + 1);
    }
    Ok((args, profile))
}

fn default_project_path() -> PathBuf {
    let executable = std::env::current_exe().ok();
    let direct = executable.as_ref().and_then(|path| path.parent());
    if let Some(root) = direct
        && root.join(distribution::MANIFEST_NAME).is_file()
        && root.join("project.json").is_file()
    {
        return if root.join("game-project-v4.json").is_file() {
            root.join("game-project-v4.json")
        } else if root.join("game-project.json").is_file() {
            root.join("game-project.json")
        } else {
            root.join("project.json")
        };
    }
    let current = PathBuf::from("project.json");
    if current.is_file() {
        return if Path::new("game-project-v4.json").is_file() {
            PathBuf::from("game-project-v4.json")
        } else if Path::new("game-project.json").is_file() {
            PathBuf::from("game-project.json")
        } else {
            current
        };
    }
    if let Some(root) = direct.and_then(Path::parent)
        && root.join(distribution::MANIFEST_NAME).is_file()
        && root.join("project.json").is_file()
    {
        return if root.join("game-project-v4.json").is_file() {
            root.join("game-project-v4.json")
        } else if root.join("game-project.json").is_file() {
            root.join("game-project.json")
        } else {
            root.join("project.json")
        };
    }
    current
}

fn advance(document: &GraphDocument, runtime: &mut Runtime, frames: usize) -> Result<(), String> {
    advance_at(
        document,
        runtime,
        frames,
        &project::source_root(),
        &project::source_root(),
    )
}

fn advance_at(
    document: &GraphDocument,
    runtime: &mut Runtime,
    frames: usize,
    compile_root: &Path,
    asset_root: &Path,
) -> Result<(), String> {
    let source = runtime.world_speed_source();
    let scripts = prepare_scripts_at(document, runtime, compile_root, asset_root)?;
    for frame in 0..frames {
        let time = frame as f64 / 60.0;
        runtime.begin_frame();
        runtime.apply_project_settings(time)?;
        let speed =
            scalar::world_speed_compiled(document, source.as_deref(), time, &|id, time, base| {
                compiled_speed(&scripts, id, time, base, document)
            })?;
        let mut commands = vec![];
        let mut pending_ticks = vec![];
        for source in runtime.command_sources() {
            let script = scripts
                .get(&source.source_id)
                .ok_or("Missing compiled update")?;
            let mut context = script_context(runtime, time, 1.0 / 60.0, source.world)?;
            context.asset_paths =
                module_engine_project::AssetManifest::from_value(&document.asset_manifest)?
                    .asset_paths();
            let tick = script
                .tick(&context, std::time::Duration::from_millis(150))
                .map_err(|e| e.to_string())?;
            commands.extend(
                tick.commands
                    .iter()
                    .filter(|c| match c {
                        ControlCommand::World(_) => source.world,
                        ControlCommand::Render(_) => source.render,
                    })
                    .cloned(),
            );
            pending_ticks.push((script, tick));
        }
        if let Err(error) = runtime.apply_project_controls(&commands) {
            for (script, tick) in pending_ticks {
                let _ = script.reject_tick(&tick);
            }
            return Err(error);
        }
        for (script, tick) in &pending_ticks {
            script.validate_tick(tick).map_err(|e| e.to_string())?;
        }
        for (script, tick) in pending_ticks {
            script.commit_tick(&tick).map_err(|e| e.to_string())?;
        }
        runtime.step(1.0 / 60.0, speed);
    }
    Ok(())
}

fn script_context(
    runtime: &Runtime,
    time: f64,
    dt: f64,
    world_connected: bool,
) -> Result<ScriptContext, String> {
    let mut context = match runtime.objects() {
        Ok(objects) => ScriptContext::new(time, dt, objects),
        Err(_) if !world_connected => ScriptContext::without_world_queries(time, dt),
        Err(error) => return Err(format!("Custom world query failed: {error}")),
    };
    runtime.fill_script_context(&mut context);
    Ok(context)
}

fn compiled_speed(
    scripts: &BTreeMap<String, CompiledScript>,
    id: &str,
    time: f64,
    base: f64,
    document: &GraphDocument,
) -> Option<Result<f64, String>> {
    scripts.get(id).map(|script| {
        let mut context = ScriptContext::new(time, 0., vec![]);
        context.base = base;
        context.asset_paths =
            module_engine_project::AssetManifest::from_value(&document.asset_manifest)?
                .asset_paths();
        script
            .tick(&context, std::time::Duration::from_millis(150))
            .map_err(|e| e.to_string())?
            .speed
            .ok_or_else(|| "Compiled scalar returned no value".into())
    })
}

fn prepare_scripts_at(
    document: &GraphDocument,
    runtime: &Runtime,
    compile_root: &Path,
    asset_root: &Path,
) -> Result<BTreeMap<String, CompiledScript>, String> {
    let compiler = ScriptCompiler::new(
        compile_root.to_path_buf(),
        compile_root.join(".work/scripts"),
    );
    let mut requested = BTreeMap::new();
    for source in runtime.command_sources() {
        requested.insert(source.source_id, ScriptKind::Update);
    }
    let mut seen = BTreeSet::new();
    let mut pending = runtime.world_speed_source().into_iter().collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(node) = document.root.nodes.iter().find(|n| n.id == id) {
            if node.module == "custom.rust"
                && let Some(key) = node
                    .params
                    .get("source_id")
                    .and_then(serde_json::Value::as_str)
            {
                requested.insert(key.into(), ScriptKind::Speed);
            }
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
    requested
        .into_iter()
        .map(|(id, kind)| {
            let source = document
                .custom_sources
                .get(&id)
                .ok_or_else(|| format!("Missing Rust source '{id}'"))?;
            Ok((
                id,
                compiler
                    .compile_kind(source, kind)
                    .and_then(|script| script.with_asset_root(asset_root))
                    .map_err(|e| e.to_string())?,
            ))
        })
        .collect()
}

fn write_png(path: &Path, frame: &Frame) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        frame.width as u32,
        frame.height as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&frame.rgba)
        .map_err(|e| e.to_string())
}

fn reject_same_render_path(input: &Path, output: &Path) -> Result<(), String> {
    if output.exists()
        && input.canonicalize().map_err(|e| e.to_string())?
            == output.canonicalize().map_err(|e| e.to_string())?
    {
        return Err("Выходной PNG не должен заменять исходный файл графа.".into());
    }
    Ok(())
}

fn self_check(folder: PathBuf) -> Result<(), String> {
    fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let registry = ModuleRegistry::builtin();
    let objects = self_check_document(WorldKind::Objects)?;
    let ecs = self_check_document(WorldKind::Ecs)?;
    let mut a = Runtime::from_document(&objects, &registry).map_err(|e| e.to_string())?;
    let mut b = Runtime::from_document(&ecs, &registry).map_err(|e| e.to_string())?;
    advance(&objects, &mut a, 120)?;
    advance(&ecs, &mut b, 120)?;
    let snapshots_equal = serde_json::to_value(a.snapshot()).map_err(|e| e.to_string())?
        == serde_json::to_value(b.snapshot()).map_err(|e| e.to_string())?;
    let frame_a = a.render(960, 640, Camera::default())?;
    let frame_b = b.render(960, 640, Camera::default())?;
    if !snapshots_equal || frame_a.rgba != frame_b.rgba {
        return Err("World implementations differ after equivalent simulation".into());
    }
    if frame_a.stats.shaded_pixels == 0 {
        return Err("3D scene contains no visible rendered geometry".into());
    }
    write_png(&folder.join("objects-3d.png"), &frame_a)?;
    write_png(&folder.join("ecs-3d.png"), &frame_b)?;
    project::save(&folder.join("objects-world.json"), &objects)?;
    project::save(&folder.join("ecs-world.json"), &ecs)?;
    let report = serde_json::json!({"world_snapshots_equal":snapshots_equal,"world_frames_equal":true,
        "steps":120,"dt":1.0/60.0,"rgba_bytes":frame_a.rgba.len(),"geometry_pixels":frame_a.stats.shaded_pixels,
        "worlds":[a.world_module_id(),b.world_module_id()],"source":"Actual Rust runtime + CPU perspective/depth renderer"});
    fs::write(
        folder.join("report.json"),
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "Self-check passed: identical 3D snapshots and RGBA frames; {} geometry pixels. Evidence: {}",
        frame_a.stats.shaded_pixels,
        folder.display()
    );
    Ok(())
}

fn self_check_document(kind: WorldKind) -> Result<GraphDocument, String> {
    let mut document = demo_document(kind);
    engine_profiles::finish_template(
        &mut document,
        engine_profiles::ProfileKind::Neutral,
        &module_engine_vulkan::resource_root(),
    )?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn self_check_constructor_publishes_neutral_profile_before_public_persistence() {
        let backend = module_engine_vulkan::resource_root();
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let document = self_check_document(kind).unwrap();
            engine_profiles::validate(&document, &backend).unwrap();
            assert_eq!(
                document.engine_lock.as_ref().unwrap().profile.id,
                "module-engine.neutral"
            );
            assert!(document.engine_profile.is_some());
            assert_eq!(
                Runtime::from_document(&document, &ModuleRegistry::builtin())
                    .unwrap()
                    .world_kind(),
                kind
            );
        }
    }
    #[test]
    fn render_only_scripts_do_not_require_optional_world_queries() {
        struct OldProvider;
        impl module_engine_core::WorldProvider for OldProvider {
            fn step(&mut self, _: f32, _: f32) {}
            fn snapshot(&self) -> module_engine_core::SceneSnapshot {
                module_engine_core::SceneSnapshot {
                    items: vec![],
                    time: 0.,
                }
            }
        }
        let mut registry = ModuleRegistry::builtin();
        registry
            .register_world(
                module_engine_core::world_descriptor("test.old", "Old", "Render-only provider"),
                |_| Ok(Box::new(OldProvider)),
            )
            .unwrap();
        let mut document = demo_document(WorldKind::Objects);
        document
            .root
            .nodes
            .iter_mut()
            .find(|n| n.id == "world")
            .unwrap()
            .module = "test.old".into();
        crate::project::stamp_module_versions(&mut document, &registry);
        let runtime = Runtime::from_document(&document, &registry).unwrap();
        let context = script_context(&runtime, 1., 0.1, false).unwrap();
        assert!(!context.world_queries_available && context.objects.is_empty());
        assert!(script_context(&runtime, 1., 0.1, true).is_err());
    }
}
