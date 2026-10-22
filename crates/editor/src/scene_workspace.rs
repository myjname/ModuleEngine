//! Glue between authored-scene windows, validated document edits and history.
//! Scene uses a retained preview host; Game owns a distinct Play copy.
use super::{Editor, GraphDocument, WorldKind, egui};
use crate::{
    game_ui,
    scene_view_ui::{PresentedSceneFrame, SceneAction, SceneViewportState},
};
use module_engine_core::{AuthoredObject, Transform, WorldObject};
use module_engine_project::AssetManifest;
use std::path::PathBuf;

impl Editor {
    pub(super) fn game_context(&mut self) -> (PathBuf, Option<PathBuf>, WorldKind) {
        let root = self.project_asset_root();
        let backend = self
            .distribution
            .as_ref()
            .map(|package| package.package_root.clone())
            .unwrap_or_else(crate::project::source_root);
        let compile = self
            .distribution
            .as_ref()
            .and_then(|package| package.compile_root().ok());
        self.game_workbench
            .set_engine_context(compile.as_deref(), &backend);
        let kind = if self
            .document
            .root
            .nodes
            .iter()
            .any(|node| node.module == "world.ecs")
        {
            WorldKind::Ecs
        } else {
            WorldKind::Objects
        };
        (root, compile, kind)
    }
    pub(super) fn scene_selection(&self) -> Option<(String, u64)> {
        self.game_workbench
            .selected()
            .map(|id| (self.game_workbench.scene.clone(), id))
    }
    /// Shared entry point for Inspector fields, model placement and resource links.
    /// Candidate validation/preview refresh finish before replacing source data.
    pub(super) fn commit_scene_document(
        &mut self,
        candidate: GraphDocument,
        label: &str,
    ) -> Result<(), String> {
        if self.playing || self.game_workbench.is_play_session() {
            return Err("Stop Play before editing the authored scene".into());
        }
        if self.json_dirty || self.game_workbench.has_graph_draft() {
            return Err(
                "Apply or discard document/execution drafts before editing the scene".into(),
            );
        }
        if candidate == self.document {
            return Ok(());
        }
        self.validate_candidate(&candidate, false)?;
        candidate
            .game
            .as_ref()
            .ok_or("No authored game project")?
            .validate_scene_data()?;
        // Resource validation also runs when Scene is hidden and no preview host
        // exists yet. This path does not initialize a GPU backend.
        if self.game_workbench.preview_host.is_none() {
            let root = self.project_asset_root();
            let manifest = AssetManifest::from_value(&candidate.asset_manifest)?;
            let mut content = module_engine_content::ContentLibrary::new(&root)?;
            content.set_asset_paths(manifest.asset_paths());
            let game = candidate.game.as_ref().unwrap();
            for scene in game.scenes.keys() {
                for entity in game.resolve_scene(scene)? {
                    if let Some(reference) = entity
                        .components
                        .get("mesh")
                        .and_then(|value| value.get("path"))
                        .and_then(serde_json::Value::as_str)
                    {
                        content.load_model(reference)?;
                    }
                    if let Some(reference) = entity
                        .components
                        .get("material")
                        .and_then(|value| value.get("base_color_texture"))
                        .and_then(serde_json::Value::as_str)
                    {
                        content.load_texture(reference)?;
                    }
                }
            }
        }
        let usages_changed = scene_resource_metadata_changed(&self.document, &candidate);
        self.game_workbench.accept_author_candidate(&candidate)?;
        self.game_workbench.error = None;
        self.document = candidate;
        self.changed(false);
        if usages_changed {
            self.assets.invalidate_usages();
        }
        self.status = label.into();
        Ok(())
    }
    pub(super) fn scene_hierarchy_panel(&mut self, ui: &mut egui::Ui) {
        self.game_context();
        let mut candidate = self.document.clone();
        let old_scene = self.game_workbench.scene.clone();
        if self.game_workbench.hierarchy_ui(ui, &mut candidate)
            && let Err(error) = self.commit_scene_document(candidate, "Hierarchy changed")
        {
            self.game_workbench.error = Some(error.clone());
            self.status = error;
        }
        if self.game_workbench.scene != old_scene {
            self.cancel_active_scene_gestures();
        }
        if let Some(error) = &self.game_workbench.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }
    pub(super) fn scene_inspector_panel(&mut self, ui: &mut egui::Ui) {
        self.game_context();
        let mut candidate = self.document.clone();
        if self.game_workbench.inspector_ui(ui, &mut candidate)
            && let Err(error) = self.commit_scene_document(candidate, "Scene object changed")
        {
            self.game_workbench.error = Some(error.clone());
            self.status = error;
        }
        if let Some(error) = &self.game_workbench.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
    }
    pub(super) fn scene_panel(&mut self, ui: &mut egui::Ui, view: &mut SceneViewportState) {
        let owner = view.presentation_owner();
        if self.document.game.is_none() {
            ui.label("Create or open a game project through File.");
            return;
        }
        self.game_workbench.mark_scene_view_visible(owner);
        let (root, compile, kind) = self.game_context();
        if let Err(error) =
            self.game_workbench
                .ensure_preview(&self.document, &root, compile.as_deref(), kind)
        {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
            if self.game_workbench.preview_host.is_none() {
                return;
            }
        }
        if self.game_workbench.preview_host.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Preparing Scene… Operation status is shown in the bottom bar.");
            });
            return;
        }
        let mut snapshot = match self
            .game_workbench
            .preview_host
            .as_mut()
            .unwrap()
            .author_snapshot()
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                view.error = Some(error.clone());
                ui.colored_label(egui::Color32::LIGHT_RED, error);
                return;
            }
        };
        let mut objects = self
            .game_workbench
            .preview_host
            .as_ref()
            .unwrap()
            .session
            .world_objects();
        let selection = self.scene_selection().map(|(_, id)| id);
        view.set_render_revision(
            self.game_workbench
                .preview_host
                .as_ref()
                .unwrap()
                .presentation_revision(),
        );
        let actions = view.interact(
            ui,
            &objects,
            &snapshot,
            selection,
            self.playing || self.game_workbench.is_play_session(),
        );
        let visual_edit = actions.iter().any(|action| {
            matches!(
                action,
                SceneAction::SetTransform { .. }
                    | SceneAction::CancelGesture { .. }
                    | SceneAction::DropModel { .. }
                    | SceneAction::DropTexture { .. }
            )
        });
        for action in actions {
            if let Err(error) = self.apply_scene_action(action) {
                view.error = Some(error.clone());
                self.status = error;
            }
        }
        let selected = self.game_workbench.selected();
        if let Some(host) = &mut self.game_workbench.preview_host {
            if visual_edit {
                match host.author_snapshot() {
                    Ok(updated) => snapshot = updated,
                    Err(error) => view.error = Some(error),
                }
                objects = host.session.world_objects();
            }
            view.set_render_revision(host.presentation_revision());
            view.set_gpu_presentation(self.gpu_bridge.clone());
            if host.has_pending_authored_presentation_for(owner)
                || host.has_ready_authored_presentation_for(owner)
            {
                view.request_presentation_refresh();
            }
            view.paint_presented(
                ui,
                &objects,
                &snapshot,
                selected,
                &mut |camera, width, height| {
                    host.camera = camera;
                    let frame = host.render_presented_for(owner, width, height)?;
                    Ok(frame.map(|frame| PresentedSceneFrame {
                        frame,
                        camera: host.last_presented_camera().unwrap_or(camera),
                        revision: host.last_presented_revision(),
                        pending: host.has_pending_authored_presentation_for(owner),
                    }))
                },
            );
        }
    }
    pub(super) fn apply_scene_action(&mut self, action: SceneAction) -> Result<(), String> {
        match action {
            SceneAction::Select(id) => self.game_workbench.select(id),
            SceneAction::BeginGesture { id } => {
                if self.playing || self.game_workbench.is_play_session() {
                    return Err("Stop Play before moving scene objects".into());
                }
                if self.game_workbench.selected() != Some(id) {
                    return Err("Gesture selection changed".into());
                }
                self.flush_history();
                self.scene_gesture_before = Some(self.document.clone());
                self.scene_gesture_scene = Some(self.game_workbench.scene.clone());
            }
            SceneAction::SetTransform { id, transform } => {
                let mut candidate = self.document.clone();
                let scene = candidate
                    .game
                    .as_mut()
                    .ok_or("No authored scene")?
                    .scenes
                    .get_mut(&self.game_workbench.scene)
                    .ok_or("Selected scene is missing")?;
                let object = scene
                    .objects
                    .iter_mut()
                    .find(|object| object.object.id == id)
                    .ok_or("Selected object is missing")?;
                object.object.transform = transform;
                self.commit_scene_document(candidate, "Scene transform changed")?;
            }
            SceneAction::CommitGesture => self.finish_scene_gesture(),
            SceneAction::CancelGesture { id, transform } => {
                self.restore_gesture_transform(id, transform)?;
            }
            SceneAction::DropModel { asset_ref, point } => {
                self.place_scene_model(&asset_ref, point)?;
            }
            SceneAction::DropTexture { asset_ref, id } => {
                self.assign_scene_resource(id, &asset_ref, "texture")?;
            }
            SceneAction::Error(error) => return Err(error),
        }
        Ok(())
    }
    fn finish_scene_gesture(&mut self) {
        self.scene_gesture_scene = None;
        if let Some(before) = self.scene_gesture_before.take() {
            if self.document != before {
                self.history
                    .record("Scene transform gesture", &self.document);
            }
            self.history_dirty = false;
        }
    }
    fn restore_gesture_transform(&mut self, id: u64, transform: Transform) -> Result<(), String> {
        let Some(before) = self.scene_gesture_before.take() else {
            return Ok(());
        };
        let original_scene = self.scene_gesture_scene.take();
        if let Some(scene) = original_scene {
            let mut candidate = self.document.clone();
            if let Some(object) = candidate
                .game
                .as_mut()
                .and_then(|game| game.scenes.get_mut(&scene))
                .and_then(|scene| {
                    scene
                        .objects
                        .iter_mut()
                        .find(|object| object.object.id == id)
                })
            {
                object.object.transform = transform;
                // Restoring a gesture is allowed when Play was just requested;
                // the author's document still precedes its independent copy.
                self.game_workbench.restore_author_candidate(&candidate)?;
                self.document = candidate;
            }
        }
        self.history_dirty = self.document != before;
        self.json_draft = serde_json::to_string_pretty(&self.document).unwrap_or_default();
        self.assets.invalidate_usages();
        self.status = "Scene gesture cancelled".into();
        Ok(())
    }
    /// Global transitions must finish/cancel pending drags before saving snapshots.
    pub(super) fn cancel_active_scene_gestures(&mut self) {
        let actions = self
            .engine_panes
            .values_mut()
            .chain(self.project_panes.values_mut())
            .filter_map(|pane| pane.scene_view.cancel_gesture())
            .collect::<Vec<_>>();
        for action in actions {
            if let Err(error) = self.apply_scene_action(action) {
                self.status = error;
            }
        }
        if let Some(before) = self.scene_gesture_before.take() {
            let _ = self.game_workbench.restore_author_candidate(&before);
            self.document = before;
            self.history_dirty = false;
            self.json_draft = serde_json::to_string_pretty(&self.document).unwrap_or_default();
            self.assets.invalidate_usages();
        }
        self.scene_gesture_scene = None;
    }
    pub(super) fn place_scene_model(
        &mut self,
        reference: &str,
        point: [f32; 3],
    ) -> Result<u64, String> {
        self.flush_history();
        let manifest = AssetManifest::from_value(&self.document.asset_manifest)?;
        let path = manifest.resolve_reference(reference)?;
        let extension = std::path::Path::new(&path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("");
        if !["gltf", "glb"]
            .iter()
            .any(|kind| extension.eq_ignore_ascii_case(kind))
        {
            return Err("Scene placement requires a static model".into());
        }
        let mut candidate = self.document.clone();
        let game = candidate.game.as_mut().ok_or("No authored game project")?;
        self.game_workbench.normalize_selection(game);
        let scene = game.scenes.get_mut(&self.game_workbench.scene).unwrap();
        let id = scene
            .objects
            .iter()
            .map(|object| object.object.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Object IDs exhausted")?;
        let mut object = AuthoredObject::new(WorldObject {
            id,
            parent: None,
            transform: Transform {
                translation: point,
                ..Transform::default()
            },
            color: [255; 4],
        });
        game_ui::attach_resource(&mut object, None, reference, "model")?;
        scene.objects.push(object);
        self.commit_scene_document(candidate, "Model placed in Scene")?;
        self.game_workbench.select(Some(id));
        self.flush_history();
        Ok(id)
    }
    pub(super) fn assign_scene_resource(
        &mut self,
        id: u64,
        reference: &str,
        kind: &str,
    ) -> Result<(), String> {
        self.flush_history();
        let mut candidate = self.document.clone();
        let game = candidate.game.as_mut().ok_or("No authored game project")?;
        let templates = game.prefabs.clone();
        let object = game
            .scenes
            .get_mut(&self.game_workbench.scene)
            .ok_or("Selected scene missing")?
            .objects
            .iter_mut()
            .find(|object| object.object.id == id)
            .ok_or("Selected object missing")?;
        let template = object.prefab.as_ref().and_then(|name| templates.get(name));
        game_ui::attach_resource(object, template, reference, kind)?;
        self.commit_scene_document(candidate, "Scene resource assigned")?;
        self.game_workbench.select(Some(id));
        self.flush_history();
        Ok(())
    }
}

/// Transforms/colors never change resource usages. Comparing lightweight
/// authored records avoids scanning every asset tile during a mouse drag.
fn scene_resource_metadata_changed(before: &GraphDocument, after: &GraphDocument) -> bool {
    if before.asset_manifest != after.asset_manifest {
        return true;
    }
    match (&before.game, &after.game) {
        (Some(a), Some(b)) => {
            if a.audio_cues != b.audio_cues
                || a.scenes.len() != b.scenes.len()
                || a.prefabs.len() != b.prefabs.len()
            {
                return true;
            }
            for (name, prefab) in &a.prefabs {
                if b.prefabs
                    .get(name)
                    .is_none_or(|other| prefab.components != other.components)
                {
                    return true;
                }
            }
            for (name, scene) in &a.scenes {
                let Some(other) = b.scenes.get(name) else {
                    return true;
                };
                if scene.objects.len() != other.objects.len() || scene.name != other.name {
                    return true;
                }
                for (object, other) in scene.objects.iter().zip(&other.objects) {
                    if object.object.id != other.object.id
                        || object.prefab != other.prefab
                        || object.components != other.components
                        || object
                            .overrides
                            .iter()
                            .filter(|(key, _)| key.as_str() != "color")
                            .ne(other
                                .overrides
                                .iter()
                                .filter(|(key, _)| key.as_str() != "color"))
                        || object.references != other.references
                    {
                        return true;
                    }
                }
            }
            false
        }
        (None, None) => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        dock::{DockLayout, EditorType, WorkspaceKind},
        panes::{PaneMemory, PaneSnapshot},
    };
    use super::*;
    use crate::{
        game_host,
        game_host::GameHost,
        scene_tools::{Axis, DragGesture, Space, Tool},
    };

    fn editor() -> Editor {
        let root = crate::project::source_root();
        let mut document = game_host::demo_document();
        // Scene/Play ownership tests need a native host, not an audio endpoint.
        // Keep the test content explicitly silent on unsupported platforms;
        // production templates and audio capability checks retain their cues.
        if !module_engine_host_services::WAV_PLAYBACK_SUPPORTED {
            document.game.as_mut().unwrap().audio_cues.clear();
        }
        let mut editor = Editor::new(document, root.join("unused-scene-editor-tests.json"));
        editor.playing = false;
        editor.game_workbench.scene = "arena".into();
        editor.game_workbench.select(Some(1));
        editor
    }
    #[test]
    fn initial_scene_preparation_draws_waiting_ui_before_host_is_ready() {
        let mut editor = editor();
        editor.game_workbench.set_deferred_preparation(true);
        let ctx = egui::Context::default();
        let mut view = SceneViewportState::default();
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1000., 700.),
                )),
                ..Default::default()
            },
            |ui| editor.scene_panel(ui, &mut view),
        );
        assert!(editor.game_workbench.preview_host.is_none());
        assert!(editor.game_workbench.has_pending_preparation());
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("Preparing Scene"))));
        output.drop_without_applying_deltas();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while editor.game_workbench.has_pending_preparation() {
            editor
                .game_workbench
                .poll_preparation(&editor.document)
                .unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "Scene preparation did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(editor.game_workbench.preview_host.is_some());
        assert!(
            editor
                .game_workbench
                .activity_snapshots()
                .iter()
                .any(|activity| activity.state == crate::activity::ActivityState::Succeeded)
        );
    }
    #[test]
    fn transform_drag_does_not_invalidate_resource_usage_metadata() {
        let before = game_host::demo_document();
        let mut after = before.clone();
        after
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .object
            .transform
            .translation[0] += 1.;
        assert!(!scene_resource_metadata_changed(&before, &after));
        after
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .overrides
            .insert("color".into(), serde_json::json!([20, 30, 40, 255]));
        assert!(!scene_resource_metadata_changed(&before, &after));
        after
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .insert("mesh".into(), serde_json::json!({"path":"asset://example"}));
        assert!(scene_resource_metadata_changed(&before, &after));
    }
    #[test]
    fn gesture_is_one_history_entry_escape_restores_and_play_blocks_author_edits() {
        let mut editor = editor();
        let initial = editor.document.clone();
        let original = initial.game.as_ref().unwrap().scenes["arena"]
            .objects
            .iter()
            .find(|object| object.object.id == 1)
            .unwrap()
            .object
            .transform;
        editor
            .apply_scene_action(SceneAction::BeginGesture { id: 1 })
            .unwrap();
        for index in 1..=8 {
            let transform = Transform {
                translation: [
                    index as f32 * 0.1,
                    original.translation[1],
                    original.translation[2],
                ],
                ..original
            };
            editor
                .apply_scene_action(SceneAction::SetTransform { id: 1, transform })
                .unwrap();
            editor.flush_history();
        }
        assert_eq!(
            editor.history.entries().len(),
            1,
            "Mid-drag debounce must not create undo entries"
        );
        editor
            .apply_scene_action(SceneAction::CommitGesture)
            .unwrap();
        assert_eq!(editor.history.entries().len(), 2);
        editor.undo();
        assert_eq!(editor.document, initial);
        editor.redo();
        let before_cancel = editor.document.clone();
        let original = before_cancel.game.as_ref().unwrap().scenes["arena"]
            .objects
            .iter()
            .find(|object| object.object.id == 1)
            .unwrap()
            .object
            .transform;
        editor
            .apply_scene_action(SceneAction::BeginGesture { id: 1 })
            .unwrap();
        editor
            .apply_scene_action(SceneAction::SetTransform {
                id: 1,
                transform: Transform {
                    translation: [5., 0., 0.],
                    ..original
                },
            })
            .unwrap();
        editor
            .apply_scene_action(SceneAction::CancelGesture {
                id: 1,
                transform: original,
            })
            .unwrap();
        assert_eq!(editor.document, before_cancel);
        assert_eq!(editor.history.entries().len(), 2);
        editor.playing = true;
        assert!(
            editor
                .apply_scene_action(SceneAction::SetTransform {
                    id: 1,
                    transform: Transform::default()
                })
                .is_err()
        );
        assert_eq!(editor.document, before_cancel);
    }
    #[test]
    fn hierarchy_and_inspector_share_scene_selection_without_legacy_runtime_selection() {
        let mut editor = editor();
        let ctx = egui::Context::default();
        editor
            .apply_scene_action(SceneAction::Select(Some(10)))
            .unwrap();
        let selection = editor.scene_selection().unwrap();
        assert_eq!(selection, ("arena".into(), 10));
        assert_eq!(
            editor.active_object,
            Some(1),
            "Engine preview keeps its own selection"
        );
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 800.),
                )),
                ..Default::default()
            },
            |ui| {
                ui.columns(2, |columns| {
                    editor.scene_hierarchy_panel(&mut columns[0]);
                    editor.scene_inspector_panel(&mut columns[1]);
                });
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(editor.scene_selection().unwrap(), selection);
        assert!(
            editor.game_workbench.host.is_none(),
            "Author windows do not create Game Play copy"
        );
    }
    #[test]
    fn retained_preview_survives_transform_updates_and_game_camera_is_independent() {
        let mut editor = editor();
        let (root, compile, kind) = editor.game_context();
        editor
            .game_workbench
            .ensure_preview(&editor.document, &root, compile.as_deref(), kind)
            .unwrap();
        let preview_address =
            editor.game_workbench.preview_host.as_ref().unwrap() as *const GameHost;
        let model = editor
            .game_workbench
            .preview_host
            .as_mut()
            .unwrap()
            .content
            .load_model("assets/models/demo_crystal.glb")
            .unwrap();
        let initial = editor.document.game.as_ref().unwrap().scenes["arena"]
            .objects
            .iter()
            .find(|object| object.object.id == 1)
            .unwrap()
            .object
            .transform;
        editor
            .apply_scene_action(SceneAction::BeginGesture { id: 1 })
            .unwrap();
        for index in 1..=12 {
            editor
                .apply_scene_action(SceneAction::SetTransform {
                    id: 1,
                    transform: Transform {
                        translation: [
                            index as f32 * 0.1,
                            initial.translation[1],
                            initial.translation[2],
                        ],
                        ..initial
                    },
                })
                .unwrap();
            assert_eq!(
                editor.game_workbench.preview_host.as_ref().unwrap() as *const GameHost,
                preview_address
            );
            let retained = editor
                .game_workbench
                .preview_host
                .as_mut()
                .unwrap()
                .content
                .load_model("assets/models/demo_crystal.glb")
                .unwrap();
            assert!(std::sync::Arc::ptr_eq(&model, &retained));
        }
        editor
            .apply_scene_action(SceneAction::CommitGesture)
            .unwrap();
        editor
            .game_workbench
            .ensure(&editor.document, &root, compile.as_deref(), kind)
            .unwrap();
        let game_camera = editor.game_workbench.host.as_ref().unwrap().camera;
        editor
            .game_workbench
            .preview_host
            .as_mut()
            .unwrap()
            .camera
            .yaw = 2.;
        assert_eq!(
            editor.game_workbench.host.as_ref().unwrap().camera,
            game_camera
        );
        assert_eq!(
            editor
                .game_workbench
                .preview_host
                .as_ref()
                .unwrap()
                .session
                .state()
                .tick,
            0
        );
        let snapshot = editor
            .game_workbench
            .preview_host
            .as_mut()
            .unwrap()
            .author_snapshot()
            .unwrap();
        assert!(
            snapshot
                .items
                .iter()
                .any(|item| item.mesh.vertices.len() != 8),
            "Picking snapshot includes imported model geometry"
        );
    }
    #[test]
    fn game_opened_before_play_uses_latest_author_scene_and_pause_retains_live_copy() {
        let mut editor = editor();
        let (root, compile, kind) = editor.game_context();
        editor
            .game_workbench
            .ensure_preview(&editor.document, &root, compile.as_deref(), kind)
            .unwrap();
        // Opening Game prepares its stopped view before any gameplay tick.
        editor
            .game_workbench
            .ensure(&editor.document, &root, compile.as_deref(), kind)
            .unwrap();
        let host = editor.game_workbench.host.as_mut().unwrap();
        let decoded = host
            .content
            .load_model("assets/models/demo_crystal.glb")
            .unwrap();
        let slot = host.slots.values().next().unwrap().clone();
        host.camera.yaw = 1.2;
        host.camera.pitch = 0.8;
        host.camera.distance = 17.;
        let orbit = (host.camera.yaw, host.camera.pitch, host.camera.distance);
        editor
            .game_workbench
            .preview_host
            .as_mut()
            .unwrap()
            .camera
            .yaw = -0.4;
        let scene_camera = editor.game_workbench.preview_host.as_ref().unwrap().camera;
        let original = editor.document.game.as_ref().unwrap().scenes["arena"]
            .objects
            .iter()
            .find(|object| object.object.id == 1)
            .unwrap()
            .object
            .transform;
        let first_pose = Transform {
            translation: [2., original.translation[1], original.translation[2]],
            ..original
        };
        editor
            .apply_scene_action(SceneAction::SetTransform {
                id: 1,
                transform: first_pose,
            })
            .unwrap();
        let model_id = editor
            .place_scene_model("assets/models/demo_crystal.glb", [2., 0., 2.])
            .unwrap();
        // Exercise Editor's stopped frame, including the graphics sync that
        // previously masked a stale GameSession with a newer host document.
        editor.tick_simulation(1. / 60.);
        let host = editor.game_workbench.host.as_mut().unwrap();
        assert_eq!(
            host.session.state().world.object(1).unwrap().transform,
            first_pose
        );
        assert!(host.session.state().world.contains(model_id));
        assert_eq!(host.session.state().tick, 0);
        assert_eq!(host.camera.target, first_pose.translation);
        assert_eq!(
            (host.camera.yaw, host.camera.pitch, host.camera.distance),
            orbit
        );
        assert!(std::sync::Arc::ptr_eq(
            &decoded,
            &host
                .content
                .load_model("assets/models/demo_crystal.glb")
                .unwrap()
        ));
        assert!(std::sync::Arc::ptr_eq(
            &slot,
            host.slots.values().next().unwrap()
        ));
        assert!(!editor.game_workbench.is_play_session());
        assert_eq!(
            editor.game_workbench.preview_host.as_ref().unwrap().camera,
            scene_camera
        );
        // Explicit Assets Refresh also clears the stopped Game's decode cache
        // when file contents change without a new path or a new asset ID.
        editor.game_workbench.invalidate_assets();
        let reloaded = editor
            .game_workbench
            .host
            .as_mut()
            .unwrap()
            .content
            .load_model("assets/models/demo_crystal.glb")
            .unwrap();
        assert!(!std::sync::Arc::ptr_eq(&decoded, &reloaded));
        // A final edit immediately followed by Play must also reach the first
        // game tick, even when there was no stopped render after that edit.
        let start_pose = Transform {
            translation: [3.5, first_pose.translation[1], first_pose.translation[2]],
            ..first_pose
        };
        editor
            .apply_scene_action(SceneAction::SetTransform {
                id: 1,
                transform: start_pose,
            })
            .unwrap();
        let authored = editor.document.clone();
        editor.playing = true;
        editor.tick_simulation(1. / 60.);
        assert!(editor.playing, "{:?}", editor.runtime_error);
        assert!(editor.game_workbench.is_play_session());
        let host = editor.game_workbench.host.as_ref().unwrap();
        assert_eq!(
            host.session.state().world.object(1).unwrap().transform,
            start_pose
        );
        assert!(host.session.state().world.contains(model_id));
        assert_eq!(
            editor.document, authored,
            "Game never writes its running state into the author document"
        );
        // Pause still owns a live copy. A harmless authored metadata/graphics
        // update must not replace its progressed state with the starting pose.
        let runtime_position = [4.5, start_pose.translation[1], start_pose.translation[2]];
        editor
            .game_workbench
            .host
            .as_mut()
            .unwrap()
            .session
            .apply_commands(&[module_engine_core::WorldCommand::SetPosition {
                id: 1,
                position: runtime_position,
            }])
            .unwrap();
        editor.playing = false;
        editor.document.name.push_str(" · paused");
        let expected_author = editor.document.clone();
        editor.tick_simulation(1. / 60.);
        assert!(editor.game_workbench.is_play_session());
        assert_eq!(
            editor
                .game_workbench
                .host
                .as_ref()
                .unwrap()
                .session
                .state()
                .world
                .object(1)
                .unwrap()
                .transform
                .translation,
            runtime_position
        );
        assert_eq!(editor.document, expected_author);
        editor.stop_game();
        assert!(!editor.game_workbench.is_play_session());
        assert_eq!(editor.document, expected_author);
        editor
            .game_workbench
            .ensure(&editor.document, &root, compile.as_deref(), kind)
            .unwrap();
        let host = editor.game_workbench.host.as_ref().unwrap();
        assert_eq!(
            host.session.state().world.object(1).unwrap().transform,
            start_pose
        );
        assert!(host.session.state().world.contains(model_id));
        assert_eq!(host.session.state().tick, 0);
        assert_eq!(
            editor.game_workbench.preview_host.as_ref().unwrap().camera,
            scene_camera
        );
    }
    #[test]
    fn global_transition_cancels_personal_pane_gesture_before_saving() {
        let mut editor = editor();
        let initial = editor.document.clone();
        let objects = initial
            .game
            .as_ref()
            .unwrap()
            .resolve_scene("arena")
            .unwrap()
            .into_iter()
            .map(|entity| entity.object)
            .collect::<Vec<_>>();
        let camera = crate::scene_view_ui::SceneViewportState::default().camera;
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800., 600.));
        let mut pane = PaneMemory::new(PaneSnapshot::default(), &initial);
        pane.scene_view.gesture = Some(
            DragGesture::begin(
                &objects,
                1,
                Tool::Translate,
                Space::World,
                Axis::X,
                camera,
                rect,
                rect.center(),
            )
            .unwrap(),
        );
        editor.project_layout = DockLayout::single(EditorType::Scene);
        editor.workspace = WorkspaceKind::Project;
        editor.project_panes.insert(1, pane);
        editor
            .apply_scene_action(SceneAction::BeginGesture { id: 1 })
            .unwrap();
        editor
            .apply_scene_action(SceneAction::SetTransform {
                id: 1,
                transform: Transform {
                    translation: [2., 0., 0.],
                    ..objects
                        .iter()
                        .find(|object| object.id == 1)
                        .unwrap()
                        .transform
                },
            })
            .unwrap();
        editor.cancel_active_scene_gestures();
        assert_eq!(editor.document, initial);
        assert!(editor.project_panes[&1].scene_view.gesture.is_none());
        assert!(editor.scene_gesture_before.is_none());
        assert_eq!(editor.history.entries().len(), 1);
    }
}
