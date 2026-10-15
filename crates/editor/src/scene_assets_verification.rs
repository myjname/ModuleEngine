//! Reproducible authoring evidence on a separate project, never the supplied file.
use crate::{
    game_host::GameHost,
    project,
    scene_tools::{Axis, GizmoGeometry},
    scene_view_ui::{SceneAction, SceneViewportState},
    workspace_files,
};
use eframe::egui::{self, Event, Pos2, Rect, Vec2};
use module_engine_core::{AuthoredObject, Frame, GraphDocument, Transform, WorldKind, WorldObject};
use module_engine_project::{
    AssetManifest, AssetWorkspace, DeletePolicy, SnapshotHistory, asset_reference,
};
use serde_json::{Value, json};
use std::{fs, path::Path, sync::Arc, time::Instant};

/// Uses a fresh case directory even if the caller repeats an acceptance run.
pub fn run(source: &Path, output: &Path) -> Result<Value, String> {
    let original_hash = module_engine_project::hash_file(source)?;
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let case = output.join(format!(
        "case-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ));
    fs::create_dir(&case).map_err(|e| e.to_string())?;
    let original = project::read(source)?;
    let distribution = crate::distribution::detect(source, &original, None)?;
    if !distribution.can_author_engine() {
        return Err(
            "The complete Full source package is required to verify all three exports".into(),
        );
    }
    let saved = workspace_files::new_project(
        source,
        &case.join("author-project"),
        &original,
        &distribution,
        workspace_files::ProjectTemplate::Empty,
    )?;
    let root = saved.path.parent().ok_or("Project root missing")?;
    let mut document = saved.document;
    let workspace = AssetWorkspace::new(root, Vec::<String>::new())?;
    workspace.synchronize(&mut document)?;
    let mut content = module_engine_content::ContentLibrary::new(root)?;
    let path = content.import_model(
        &distribution
            .package_root
            .join("assets/models/demo_crystal.glb"),
        "assets/imported/crystal",
    )?;
    let operation = workspace.adopt_import(&document, std::slice::from_ref(&path))?;
    // Import history must remove and recreate physical files, not just the index.
    let before = operation.record.undo(root, &operation.document)?;
    if root.join(&path).exists() {
        return Err("Import Undo left its physical model behind".into());
    }
    document = operation.record.redo(root, &before)?;
    let manifest = AssetManifest::from_value(&document.asset_manifest)?;
    let id = manifest
        .assets
        .iter()
        .find(|(_, entry)| entry.path == path)
        .map(|(id, _)| id.clone())
        .ok_or("Import did not receive an ID")?;
    let reference = asset_reference(&id);
    let mut object = AuthoredObject::new(WorldObject {
        id: 1,
        parent: None,
        transform: Transform {
            scale: [2.; 3],
            ..Transform::default()
        },
        color: [200, 170, 255, 255],
    });
    object
        .components
        .insert("mesh".into(), json!({"path":reference}));
    let game = document.game.as_mut().ok_or("Game missing")?;
    game.scenes
        .get_mut(&game.entry_scene)
        .ok_or("Scene missing")?
        .objects
        .push(object);
    project::save(&saved.path, &document)?;
    let compile = distribution.compile_root()?;
    let mut timings = vec![];
    // Isolated imported model and authored two-arena gameplay data.
    let arena = workspace_files::template_document(
        &document,
        workspace_files::ProjectTemplate::TwoArenas,
        &distribution,
    )?;
    for (label, mut benchmark) in [("single-model", document.clone()), ("two-arenas", arena)] {
        let game = benchmark.game.as_mut().ok_or("Benchmark game missing")?;
        let player = game
            .scenes
            .get_mut(&game.entry_scene)
            .ok_or("Benchmark scene missing")?
            .objects
            .iter_mut()
            .find(|o| o.object.id == 1)
            .ok_or("Benchmark object missing")?;
        player
            .components
            .insert("mesh".into(), json!({"path":reference}));
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            timings.push(drag_benchmark(
                &benchmark,
                root,
                &distribution.package_root,
                &compile,
                kind,
                &reference,
                &case,
                label,
            )?);
        }
    }
    let usages = workspace.usages(&document, &id)?;
    if usages.is_empty()
        || workspace
            .delete_asset(&document, &id, DeletePolicy::RefuseUsed)
            .is_ok()
    {
        return Err("Used model deletion bypassed the usage guard".into());
    }
    let moved = workspace.move_asset(&document, &id, "assets/imported/crystal/renamed.glb")?;
    project::save(&saved.path, &moved.document)?;
    let reopened = project::read(&saved.path)?;
    if reopened != moved.document
        || AssetManifest::from_value(&reopened.asset_manifest)?.resolve_reference(&reference)?
            != "assets/imported/crystal/renamed.glb"
    {
        return Err("Rename/save/reopen lost stable identity".into());
    }
    let undone = moved.record.undo(root, &reopened)?;
    project::save(&saved.path, &undone)?;
    let redone = moved.record.redo(root, &undone)?;
    project::save(&saved.path, &redone)?;
    let deleted = workspace.delete_asset(&redone, &id, DeletePolicy::Detach)?;
    project::save(&saved.path, &deleted.document)?;
    if deleted
        .document
        .game
        .as_ref()
        .unwrap()
        .scenes
        .values()
        .flat_map(|s| &s.objects)
        .count()
        != 1
    {
        return Err("Detach deleted the scene object".into());
    }
    if !workspace
        .list_deleted()?
        .iter()
        .any(|r| r.id == deleted.record.id)
    {
        return Err("Deleted asset is absent from project trash".into());
    }
    let trash_restore = workspace.restore_deleted(&deleted.document, &deleted.record.id)?;
    if AssetManifest::from_value(&trash_restore.document.asset_manifest)?
        .resolve_reference(&reference)?
        != "assets/imported/crystal/renamed.glb"
    {
        return Err("Trash Restore changed the original ID".into());
    }
    let detached = trash_restore.record.undo(root, &trash_restore.document)?;
    let restored = deleted.record.undo(root, &detached)?;
    project::save(&saved.path, &restored)?;
    let copy = workspace_files::save_as(
        &saved.path,
        &case.join("relocated/project.json"),
        &restored,
        &crate::distribution::detect(&saved.path, &restored, None)?,
    )?;
    let copy_root = copy.path.parent().ok_or("Copy root missing")?;
    let mut copied_content = module_engine_content::ContentLibrary::new(copy_root)?;
    copied_content
        .set_asset_paths(AssetManifest::from_value(&copy.document.asset_manifest)?.asset_paths());
    if copied_content.load_model(&reference)?.primitives.is_empty() {
        return Err("Save As lost the model".into());
    }
    let full = project::export_with_package_root(
        &case.join("full"),
        &restored,
        root,
        &distribution.package_root,
    )?;
    let full_document = project::read(&full.join("project.json"))?;
    let full_access =
        crate::distribution::detect(&full.join("project.json"), &full_document, None)?;
    if !full_access.can_author_engine() {
        return Err("Full export lost complete source access".into());
    }
    verify_exported_model(&full, &full_document, &reference)?;
    let team = project::export_team_with_package_root(
        &case.join("team"),
        &restored,
        root,
        &std::env::current_exe().map_err(|e| e.to_string())?,
        &distribution.package_root,
    )?;
    let team_document = project::read(&team.join("project.json"))?;
    let team_access =
        crate::distribution::detect(&team.join("project.json"), &team_document, None)?;
    if team_access.can_author_engine()
        || team_access.profile != crate::distribution::DistributionProfile::Team
    {
        return Err("Team export unexpectedly granted Engine authoring".into());
    }
    verify_exported_model(&team, &team_document, &reference)?;
    let player = crate::game_package::export(
        &restored,
        root,
        &distribution.package_root,
        &compile,
        &case.join("player"),
    )?;
    let player_document = crate::game_package::validate_package(&player)?;
    verify_exported_model(&player, &player_document, &reference)?;
    let mut ready = GameHost::new_at(
        player_document.clone(),
        &player,
        &player,
        None,
        WorldKind::Objects,
    )?;
    ready.audio.enabled = false;
    ready.set_detailed_stats(true);
    let player_frame = ready.render(640, 360)?;
    if player_frame.stats.shaded_pixels == 0 {
        return Err("Ready Player model has no rendered geometry".into());
    }
    crate::write_png(&case.join("player-model.png"), &player_frame)?;
    if module_engine_project::hash_file(source)? != original_hash {
        return Err("Acceptance modified the supplied project".into());
    }
    let report = json!({"passed":true,"source_unchanged":true,"case":case,"asset_id":id,"reference":reference,"import_undo_redo":true,"rename_undo_redo":true,"used_delete_guard":true,"usage_locations":usages.iter().map(|u|u.location.clone()).collect::<Vec<_>>(),"detach_keeps_object":true,"trash_restore_keeps_id":true,"save_as_relocation":true,"exports":{"full":full,"full_engine_available":true,"team":team,"team_engine_available":false,"player":player,"player_precompiled_render_pixels":player_frame.stats.shaded_pixels},"drag_benchmark":timings,"timing_scope":"Actual egui pointer events, Scene actions, author-preview validation/update, synchronous Vulkan rendering/readback, RGBA conversion and egui TextureHandle update queue. Headless egui; excludes native window/compositor, egui GPU texture upload, vsync, cold compilation and initial GPU setup."});
    fs::write(
        case.join("scene-assets-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        output.join("scene-assets-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}

fn verify_exported_model(
    root: &Path,
    document: &GraphDocument,
    reference: &str,
) -> Result<(), String> {
    let manifest = AssetManifest::from_value(&document.asset_manifest)?;
    manifest.verify(root)?;
    let mut content = module_engine_content::ContentLibrary::new(root)?;
    content.set_asset_paths(manifest.asset_paths());
    if content.load_model(reference)?.primitives.is_empty() {
        return Err("Exported ID resolves to an empty model".into());
    }
    Ok(())
}

struct DragHarness {
    host: GameHost,
    authored: GraphDocument,
    history: SnapshotHistory,
    ctx: egui::Context,
    view: SceneViewportState,
    begins: usize,
    updates: usize,
    commits: usize,
    last_frame: Option<Frame>,
    capture_frame: bool,
}
impl DragHarness {
    fn frame(&mut self, events: Vec<Event>, time: f64) -> Result<f64, String> {
        let started = Instant::now();
        let snapshot = self.host.author_snapshot()?;
        let objects = self.host.session.world_objects();
        let mut error = None;
        let Self {
            host,
            authored,
            history,
            ctx,
            view,
            begins,
            updates,
            commits,
            last_frame,
            capture_frame,
        } = self;
        let result = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(960., 600.))),
                focused: true,
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    for action in view.interact(ui, &objects, &snapshot, Some(1), false) {
                        match action {
                            SceneAction::BeginGesture { .. } => *begins += 1,
                            SceneAction::SetTransform { id, transform } => {
                                *updates += 1;
                                let game = authored.game.as_mut().unwrap();
                                game.scenes
                                    .get_mut(&game.entry_scene)
                                    .unwrap()
                                    .objects
                                    .iter_mut()
                                    .find(|o| o.object.id == id)
                                    .unwrap()
                                    .object
                                    .transform = transform;
                                if let Err(e) = host.refresh_author_preview(authored) {
                                    error = Some(e);
                                }
                            }
                            SceneAction::CommitGesture => {
                                *commits += 1;
                                history.record("Move object", authored);
                            }
                            SceneAction::CancelGesture { .. } => {
                                error =
                                    Some("Unexpected cancellation during pointer benchmark".into())
                            }
                            SceneAction::Error(e) => error = Some(e),
                            _ => {}
                        }
                    }
                    match host.author_snapshot() {
                        Ok(updated) => {
                            let updated_objects = host.session.world_objects();
                            view.paint(
                                ui,
                                &updated_objects,
                                &updated,
                                Some(1),
                                &mut |camera, width, height| {
                                    host.camera = camera;
                                    let frame = host.render(width, height)?;
                                    if *capture_frame {
                                        *last_frame = Some(frame.clone());
                                    }
                                    Ok(frame)
                                },
                            );
                            if let Some(e) = &view.error {
                                error = Some(e.clone());
                            }
                        }
                        Err(e) => error = Some(e),
                    }
                });
            },
        );
        if result.textures_delta.set.is_empty() && error.is_none() {
            error = Some("Scene did not queue a texture update".into());
        }
        result.drop_without_applying_deltas();
        if let Some(error) = error {
            return Err(error);
        }
        Ok(started.elapsed().as_secs_f64() * 1000.)
    }
}
#[allow(clippy::too_many_arguments)]
fn drag_benchmark(
    document: &GraphDocument,
    root: &Path,
    backend: &Path,
    compile: &Path,
    kind: WorldKind,
    reference: &str,
    output: &Path,
    label: &str,
) -> Result<Value, String> {
    let mut host = GameHost::new_at(document.clone(), root, backend, Some(compile), kind)?;
    host.audio.enabled = false;
    let cached = host.content.load_model(reference)?;
    let slots: Vec<_> = host
        .slots
        .iter()
        .map(|(id, slot)| (id.clone(), Arc::clone(slot)))
        .collect();
    let initial = host.session.world_objects();
    let selected = initial
        .iter()
        .find(|o| o.id == 1)
        .ok_or("Benchmark selection missing")?
        .transform;
    let mut view = SceneViewportState::default();
    view.camera.distance = 9.;
    view.camera.target = selected.translation;
    let mut harness = DragHarness {
        host,
        authored: document.clone(),
        history: SnapshotHistory::new(document.clone(), 10),
        ctx: egui::Context::default(),
        view,
        begins: 0,
        updates: 0,
        commits: 0,
        last_frame: None,
        capture_frame: false,
    };
    for index in 0..10 {
        harness.frame(vec![], index as f64 / 60.)?;
    }
    let geometry = GizmoGeometry::build(
        &initial,
        1,
        harness.view.tool,
        harness.view.space,
        harness.view.camera,
        harness.view.rect.ok_or("Scene viewport not laid out")?,
    )?;
    let handle = geometry
        .axes
        .iter()
        .find(|a| a.axis == Axis::X)
        .ok_or("X handle missing")?
        .end;
    let button = |position, pressed| Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    harness.frame(vec![Event::PointerMoved(handle), button(handle, true)], 0.2)?;
    let mut measured = vec![];
    for index in 0..120 {
        measured.push(harness.frame(
            vec![Event::PointerMoved(
                handle + Vec2::new(5. + index as f32 * 0.3, 0.),
            )],
            0.3 + index as f64 / 60.,
        )?);
    }
    harness.capture_frame = true;
    harness.frame(vec![button(handle + Vec2::new(41., 0.), false)], 2.4)?;
    if harness.begins != 1
        || harness.updates < 100
        || harness.commits != 1
        || harness.history.entries().len() != 2
    {
        return Err(format!(
            "Drag/history was not exercised: {} begins, {} updates, {} commits, {} records",
            harness.begins,
            harness.updates,
            harness.commits,
            harness.history.entries().len()
        ));
    }
    let final_objects = harness.host.session.world_objects();
    if final_objects.iter().find(|o| o.id == 1).unwrap().transform == selected {
        return Err("Pointer drag did not change the selected transform".into());
    }
    if !Arc::ptr_eq(&cached, &harness.host.content.load_model(reference)?)
        || slots.iter().any(|(id, slot)| {
            harness
                .host
                .slots
                .get(id)
                .is_none_or(|current| !Arc::ptr_eq(slot, current))
        })
    {
        return Err("Transform replaced decoded content or compiled Custom slots".into());
    }
    let final_document = harness.authored.clone();
    let undone = harness
        .history
        .undo()
        .ok_or("Missing completed gesture Undo")?;
    if &undone != document {
        return Err("Gesture Undo did not restore the authored baseline".into());
    }
    harness.host.refresh_author_preview(&undone)?;
    let redone = harness
        .history
        .redo()
        .ok_or("Missing completed gesture Redo")?;
    if redone != final_document {
        return Err("Gesture Redo lost the completed transform".into());
    }
    harness.host.refresh_author_preview(&redone)?;
    let measured_frame = harness.last_frame.ok_or("No rendered drag frame")?;
    harness.host.set_detailed_stats(true);
    let frame = harness
        .host
        .render(measured_frame.width, measured_frame.height)?;
    if frame.stats.shaded_pixels == 0 {
        return Err("Scene drag viewport has no rendered geometry".into());
    }
    crate::write_png(&output.join(format!("drag-{label}-{kind:?}.png")), &frame)?;
    measured.sort_by(f64::total_cmp);
    Ok(
        json!({"scenario":label,"world":format!("{kind:?}"),"active_scene_objects":initial.len(),"frames":120,"gesture_begins":harness.begins,"transform_updates":harness.updates,"commits":harness.commits,"undo_entries_added":1,"undo_redo":true,"median_ms":measured[60],"p95_ms":measured[113],"max_ms":measured[119],"decoded_model_retained":true,"compiled_custom_slots_retained":true,"compiled_slot_count":slots.len(),"viewport":[frame.width,frame.height],"geometry_pixels":frame.stats.shaded_pixels}),
    )
}
