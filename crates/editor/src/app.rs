//! Native authoring UI. Models and validators remain independent of egui.

use crate::{
    authoring::{PreviewBuilder, Scripts},
    canvas::{self, CanvasAction, CanvasEdge, CanvasNode, CanvasPort, CanvasState},
    host, project, scalar,
};
use eframe::egui::{self, Color32, RichText, Vec2};
use module_engine_core::{
    Camera, ControlCommand, Graph, GraphDocument, ModuleRegistry, Runtime, Severity, WorldKind,
    make_edge, make_node, node_ports, software3d_graph, validate_graph,
};
use module_engine_custom::{
    CodeNodeKind, CommandNodeKind, CommandProgram, CustomProgram, demo_source,
};
use module_engine_project::{
    CheckpointStore, ConflictChoice, MergeResult, SnapshotHistory, diff_documents, merge_documents,
};
use module_engine_script::ScriptKind;
use module_engine_vulkan::RenderPath;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[path = "assets_workspace_ui.rs"]
mod assets_workspace_ui;
#[path = "dock.rs"]
mod dock;
#[path = "editor_workflow.rs"]
pub(crate) mod editor_workflow;
#[path = "panels.rs"]
mod panels;
#[path = "panes.rs"]
mod panes;
#[path = "editor_benchmark.rs"]
pub(crate) mod performance;
#[path = "properties.rs"]
pub(crate) mod properties;
#[path = "scene_workspace.rs"]
mod scene_workspace;
#[path = "window_navigation.rs"]
mod window_navigation;
use dock::{DockLayout, EditorDescriptor, EditorType, WorkspaceKind};
use panes::PaneMemory;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Graph,
    File,
    Custom,
    Shader,
    History,
}
#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
enum CustomView {
    Text,
    Graph,
    Both,
}

pub struct Editor {
    interactive_ui: bool,
    game_workbench: crate::game_ui::GameWorkbench,
    pub(super) assets: assets_workspace_ui::AssetsWorkbench,
    scene_gesture_before: Option<GraphDocument>,
    scene_gesture_scene: Option<String>,
    pub document: GraphDocument,
    registry: ModuleRegistry,
    runtime: Option<Runtime>,
    diagnostics: Vec<module_engine_core::Diagnostic>,
    runtime_error: Option<String>,
    game_preparation_error: Option<(String, String)>,
    game_execution_error: Option<String>,
    status: String,
    dirty: bool,
    path: String,
    tab: Tab,
    custom_view: CustomView,
    graph_path: Vec<usize>,
    graph_canvas: CanvasState,
    code_canvas: CanvasState,
    current_source: String,
    program: Option<CustomProgram>,
    command_program: Option<CommandProgram>,
    source_error: Option<String>,
    state_migration: String,
    code_positions: BTreeMap<String, [f32; 2]>,
    json_draft: String,
    json_dirty: bool,
    param_draft: String,
    inspector_id: Option<String>,
    playing: bool,
    time: f64,
    camera: Camera,
    texture: Option<egui::TextureHandle>,
    last_render: f64,
    last_tick: f64,
    stats: String,
    scripts: Scripts,
    preview_builder: PreviewBuilder,
    rust_edits: BTreeMap<String, f64>,
    current_shader: String,
    history: SnapshotHistory,
    history_dirty: bool,
    history_edit_time: f64,
    checkpoint_label: String,
    merge_base: String,
    merge_theirs: String,
    merge_result: Option<MergeResult>,
    diff_text: String,
    distribution: Option<crate::distribution::Distribution>,
    policy_baseline: GraphDocument,
    workspace: WorkspaceKind,
    engine_layout: DockLayout,
    project_layout: DockLayout,
    engine_panes: BTreeMap<u64, PaneMemory>,
    project_panes: BTreeMap<u64, PaneMemory>,
    panel_registry: Vec<panes::ModulePanelRegistration>,
    active_object: Option<u64>,
    authored_object_ids: std::collections::BTreeSet<u64>,
    diagnostic_personal_root: Option<PathBuf>,
    #[cfg(test)]
    test_personal_settings: module_engine_script::TestArtifactGuard,
    personal_dirty: bool,
    personal_save_time: f64,
    last_ui_frame: Option<u64>,
    authoring_intent: bool,
    workflow: editor_workflow::Workflow,
    ui_capture: crate::ui_capture::Capture,
    engine_navigation: window_navigation::WindowNavigation,
    project_navigation: window_navigation::WindowNavigation,
    // The palette is a separate pane: its own graph_path must never become
    // the destination of an insertion intended for the last focused graph.
    engine_library_graph: Option<u64>,
    project_library_graph: Option<u64>,
    module_search: String,
}

impl Editor {
    /// Bind legacy PropertySchema dependency scans to the accepted live world.
    /// A pending world replacement cannot borrow the previous provider's IDs.
    pub(super) fn asset_property_object_ids(&self) -> Option<std::collections::BTreeSet<u64>> {
        let runtime = self.runtime.as_ref()?;
        if !runtime.world_source_matches(&self.document) {
            return None;
        }
        Some(
            runtime
                .objects()
                .map(|objects| objects.into_iter().map(|object| object.id).collect())
                .unwrap_or_else(|_| {
                    runtime
                        .snapshot()
                        .items
                        .into_iter()
                        .map(|item| item.id)
                        .collect()
                }),
        )
    }

    pub fn show_custom(&mut self) {
        self.reveal_editor(EditorType::RustCustom);
        self.tab = Tab::Custom;
        self.custom_view = CustomView::Both;
        self.code_canvas.zoom = 0.55;
        if let Some(source) = self
            .document
            .root
            .nodes
            .iter()
            .find(|n| n.module == "custom.rust.update")
            .and_then(|n| n.params.get("source_id"))
            .and_then(serde_json::Value::as_str)
        {
            self.current_source = source.into();
            self.parse_custom();
            self.restore_code_positions();
        }
        self.seed_capture_pane();
    }
    pub fn show_verification_view(&mut self, view: &str) {
        match view {
            "render" => self.reveal_editor(EditorType::ModuleGraph),
            "shader" => self.reveal_editor(EditorType::Hlsl),
            "history" => self.reveal_editor(EditorType::History),
            "engine" => {
                self.workspace = WorkspaceKind::Engine;
                self.graph_canvas.zoom = 0.5;
            }
            "settings" => self.reveal_editor(EditorType::ProjectSettings),
            "game" => {
                self.workspace = WorkspaceKind::Project;
                self.reveal_editor(EditorType::Game);
            }
            "scene" => {
                self.workspace = WorkspaceKind::Project;
                self.reveal_editor(EditorType::Scene);
                self.camera = crate::scene_view_ui::SceneViewportState::default().camera;
                if let Some(game) = &self.document.game {
                    self.game_workbench.normalize_selection(game);
                    let selected = game
                        .resolve_scene(&self.game_workbench.scene)
                        .ok()
                        .and_then(|objects| {
                            objects
                                .into_iter()
                                .find(|entity| entity.components.contains_key("player"))
                        })
                        .map(|entity| entity.object.id);
                    self.game_workbench.select(selected);
                }
            }
            "assets" => {
                self.workspace = WorkspaceKind::Project;
                self.reveal_editor(EditorType::Assets);
            }
            "systems" => self.reveal_editor(EditorType::GameSystems),
            _ => {}
        }
        match view {
            "render" => {
                if let Some(index) = self
                    .document
                    .root
                    .nodes
                    .iter()
                    .position(|n| n.module == "render.vulkan")
                {
                    self.graph_path = vec![index];
                    self.graph_canvas.zoom = 0.48;
                }
            }
            "shader" => {
                self.tab = Tab::Shader;
                self.current_shader = self
                    .document
                    .shader_sources
                    .keys()
                    .next()
                    .cloned()
                    .unwrap_or_default();
            }
            "history" => self.tab = Tab::History,
            "custom" => self.show_custom(),
            _ => {}
        }
        self.seed_capture_pane();
    }
    #[cfg(test)]
    pub fn new(document: GraphDocument, path: PathBuf) -> Self {
        let distribution = crate::distribution::detect(
            &path,
            &document,
            Some(crate::distribution::DistributionProfile::Full),
        )
        .expect(
            "Tests require a verified Full source inventory; Team markers must never be bypassed",
        );
        assert!(
            distribution.can_author_engine(),
            "Verified Full fixture unavailable: {:?}",
            distribution.source_status
        );
        let mut editor = Self::new_with_policy(document, path, Some(distribution));
        editor.workspace = WorkspaceKind::Engine;
        editor
    }
    pub fn new_with_distribution(
        document: GraphDocument,
        path: PathBuf,
        distribution: crate::distribution::Distribution,
    ) -> Self {
        Self::new_with_policy(document, path, Some(distribution))
    }
    /// Isolate only personal UI state for an explicitly requested diagnostic run.
    /// The distribution and its source/SDK permissions remain unchanged.
    pub(crate) fn new_with_diagnostic_personal_root(
        document: GraphDocument,
        path: PathBuf,
        distribution: crate::distribution::Distribution,
        personal_root: Option<PathBuf>,
    ) -> Self {
        Self::new_with_policy_and_personal_root(document, path, Some(distribution), personal_root)
    }
    fn new_with_policy(
        document: GraphDocument,
        path: PathBuf,
        distribution: Option<crate::distribution::Distribution>,
    ) -> Self {
        Self::new_with_policy_and_personal_root(document, path, distribution, None)
    }
    fn new_with_policy_and_personal_root(
        document: GraphDocument,
        path: PathBuf,
        distribution: Option<crate::distribution::Distribution>,
        diagnostic_personal_root: Option<PathBuf>,
    ) -> Self {
        let mut canvas = CanvasState::new("engine-graph");
        canvas.zoom = 0.82;
        let (registry, initial_status) = match distribution
            .as_ref()
            .map(|p| host::registry_at(&p.package_root))
            .unwrap_or_else(host::registry)
        {
            Ok(registry) => (
                registry,
                "Готово. Изменяйте граф или откройте Custom.".into(),
            ),
            Err(error) => (
                ModuleRegistry::builtin(),
                format!("Ошибка регистрации модулей: {error}"),
            ),
        };
        let history = SnapshotHistory::new(document.clone(), 200);
        let policy_baseline = document.clone();
        let scripts = match &distribution {
            Some(profile) => match profile.compile_root() {
                Ok(root) => Scripts::with_root(root),
                Err(error) => Scripts::disabled(error),
            },
            None => Scripts::disabled("Distribution/source manifest unavailable".into()),
        };
        let mut editor = Self {
            interactive_ui: false,
            game_workbench: crate::game_ui::GameWorkbench::default(),
            assets: assets_workspace_ui::AssetsWorkbench::default(),
            scene_gesture_before: None,
            scene_gesture_scene: None,
            json_draft: serde_json::to_string_pretty(&document).unwrap_or_default(),
            workflow: editor_workflow::Workflow::new(document.clone()),
            document,
            registry,
            runtime: None,
            diagnostics: vec![],
            runtime_error: None,
            game_preparation_error: None,
            game_execution_error: None,
            status: initial_status,
            dirty: false,
            path: path.to_string_lossy().into(),
            tab: Tab::Graph,
            custom_view: CustomView::Both,
            graph_path: vec![],
            graph_canvas: canvas,
            code_canvas: CanvasState::new("rust-graph"),
            current_source: "speed".into(),
            program: None,
            command_program: None,
            source_error: None,
            state_migration: "{}".into(),
            code_positions: BTreeMap::new(),
            json_dirty: false,
            param_draft: String::new(),
            inspector_id: None,
            playing: true,
            time: 0.,
            camera: Camera::default(),
            texture: None,
            last_render: -1.,
            last_tick: 0.,
            stats: String::new(),
            scripts,
            preview_builder: PreviewBuilder::new(),
            rust_edits: BTreeMap::new(),
            current_shader: String::new(),
            history,
            history_dirty: false,
            history_edit_time: 0.,
            checkpoint_label: "Версия проекта".into(),
            merge_base: "project-base.json".into(),
            merge_theirs: "project-theirs.json".into(),
            merge_result: None,
            diff_text: String::new(),
            distribution,
            policy_baseline,
            workspace: WorkspaceKind::Project,
            engine_layout: DockLayout::default_for(WorkspaceKind::Engine),
            project_layout: DockLayout::default_for(WorkspaceKind::Project),
            engine_panes: BTreeMap::new(),
            project_panes: BTreeMap::new(),
            panel_registry: vec![],
            active_object: Some(1),
            authored_object_ids: std::collections::BTreeSet::new(),
            diagnostic_personal_root,
            #[cfg(test)]
            test_personal_settings: module_engine_script::TestArtifactGuard::new(
                &std::env::temp_dir().join("module-engine-editor-settings"),
                "editor-personal-settings",
            )
            .expect("A test Editor requires an isolated personal settings directory"),
            personal_dirty: false,
            personal_save_time: 0.0,
            last_ui_frame: None,
            authoring_intent: false,
            ui_capture: crate::ui_capture::Capture::new(),
            engine_navigation: window_navigation::WindowNavigation::default(),
            project_navigation: window_navigation::WindowNavigation::default(),
            engine_library_graph: None,
            project_library_graph: None,
            module_search: String::new(),
        };
        if std::env::var_os("EFRAME_SCREENSHOT_TO").is_some() {
            // eframe's test-only capture exits after two frames. Prepare the real
            // Vulkan route before those frames so the capture verifies its UI.
            editor.diagnostics = validate_graph(&editor.document, &editor.registry);
            match Runtime::from_document(&editor.document, &editor.registry) {
                Ok(runtime) => {
                    editor.authored_object_ids = runtime
                        .objects()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|o| o.id)
                        .collect();
                    editor.runtime = Some(runtime);
                }
                Err(error) => editor.runtime_error = Some(error.to_string()),
            }
            editor.playing = false;
        } else {
            editor.rebuild();
        }
        let _ = editor.register_module_panel(
            100,
            "scene.extract",
            "Participating Modules",
            false,
            panes::participating_modules_panel,
        );
        editor.load_personal_layouts();
        if let Err(error) = editor.scripts.set_asset_root(&editor.project_asset_root()) {
            editor.status = error;
        }
        editor.parse_custom();
        editor.restore_code_positions();
        editor.compile_connected_scripts();
        if editor.document.game.is_some() {
            editor.playing = false;
        }
        editor.initialize_workflow();
        editor
    }

    fn is_update_source(&self) -> bool {
        self.is_update_id(&self.current_source)
    }

    fn is_game_source(&self, id: &str) -> bool {
        self.document.game.as_ref().is_some_and(|game| {
            crate::game_host::script_ids(game)
                .iter()
                .any(|source| source == id)
        })
    }

    fn is_update_id(&self, id: &str) -> bool {
        if self
            .document
            .game
            .as_ref()
            .is_some_and(|g| crate::game_host::script_ids(g).iter().any(|s| s == id))
        {
            return true;
        }
        self.document.root.nodes.iter().any(|node| {
            node.module == "custom.rust.update"
                && node
                    .params
                    .get("source_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(id)
        })
    }

    fn compile_connected_scripts(&mut self) {
        let ids = self
            .document
            .root
            .nodes
            .iter()
            .filter(|n| n.module == "custom.rust.update" || n.module == "custom.rust")
            .filter_map(|n| {
                n.params
                    .get("source_id")
                    .and_then(serde_json::Value::as_str)
                    .map(|id| {
                        (
                            id.to_owned(),
                            if n.module == "custom.rust.update" {
                                ScriptKind::Update
                            } else {
                                ScriptKind::Speed
                            },
                        )
                    })
            })
            .collect::<Vec<_>>();
        for (id, kind) in ids {
            if let Some(source) = self.document.custom_sources.get(&id)
                && (kind == ScriptKind::Update || self.scripts.has_active(&id))
                && (self.scripts.active_source(&id) != Some(source.as_str())
                    || !self.scripts.has_active_kind(&id, kind))
            {
                self.scripts.request(&id, source, kind);
            }
        }
    }

    fn flush_history(&mut self) {
        if self.scene_gesture_before.is_some() {
            return;
        }
        if self.history_dirty {
            self.history.record("Изменение проекта", &self.document);
            self.history_dirty = false;
        }
    }

    fn restore_document(&mut self, document: GraphDocument) {
        self.cancel_active_scene_gestures();
        if self.json_dirty || self.game_workbench.has_graph_draft() {
            self.status = "Примените или отмените черновики JSON и Gameplay Systems перед восстановлением версии".into();
            return;
        }
        if let Err(error) = self.validate_candidate(&document, self.can_author_engine()) {
            self.status = error;
            return;
        }
        if let Err(error) = self.validate_asset_restore(&document) {
            self.status = error;
            return;
        }
        self.game_workbench.stop();
        self.game_workbench.invalidate_preview_assets();
        if document.asset_manifest != self.document.asset_manifest {
            self.assets.invalidate();
        } else {
            self.assets.invalidate_usages();
        }
        if document.game.is_some() {
            self.playing = false;
        }
        self.document = document;
        if self.can_author_engine() {
            self.policy_baseline = self.document.clone();
        }
        self.graph_path.clear();
        self.graph_canvas.selected = None;
        self.graph_canvas.pending = None;
        self.code_canvas.selected = None;
        self.code_canvas.pending = None;
        self.program = None;
        self.command_program = None;
        self.inspector_id = None;
        self.json_dirty = false;
        self.json_draft = serde_json::to_string_pretty(&self.document).unwrap_or_default();
        if !self
            .document
            .custom_sources
            .contains_key(&self.current_source)
        {
            self.current_source = self
                .document
                .custom_sources
                .keys()
                .next()
                .cloned()
                .unwrap_or_default();
        }
        self.parse_custom();
        self.restore_code_positions();
        self.rebuild();
        self.compile_connected_scripts();
        self.dirty = true;
    }

    fn undo(&mut self) {
        self.cancel_active_scene_gestures();
        if self.game_workbench.has_graph_draft() {
            self.status = "Примените или отмените черновик Gameplay Systems перед Undo".into();
            return;
        }
        if self.json_dirty {
            self.status = "Примените или отмените JSON-черновик перед Undo".into();
            return;
        }
        self.flush_history();
        if self.asset_history_replay(false) {
            return;
        }
        if let Some(entry) = self
            .history
            .cursor()
            .checked_sub(1)
            .and_then(|cursor| self.history.entries().get(cursor))
            && let Err(error) = self.validate_asset_restore(&entry.document)
        {
            self.status = error;
            return;
        }
        if let Some(document) = self.history.undo() {
            self.restore_document(document);
            self.status = "Изменение отменено".into();
        }
    }

    fn redo(&mut self) {
        self.cancel_active_scene_gestures();
        if self.game_workbench.has_graph_draft() {
            self.status = "Примените или отмените черновик Gameplay Systems перед Redo".into();
            return;
        }
        if self.json_dirty {
            self.status = "Примените или отмените JSON-черновик перед Redo".into();
            return;
        }
        self.flush_history();
        if self.asset_history_replay(true) {
            return;
        }
        if let Some(entry) = self.history.entries().get(self.history.cursor() + 1)
            && let Err(error) = self.validate_asset_restore(&entry.document)
        {
            self.status = error;
            return;
        }
        if let Some(document) = self.history.redo() {
            self.restore_document(document);
            self.status = "Изменение повторено".into();
        }
    }

    fn switch_gpu_path(&mut self, path: RenderPath) {
        if !self.can_author_engine() {
            self.status = "Engine authoring unavailable in this profile/source package".into();
            return;
        }
        if self.json_dirty {
            return;
        }
        self.flush_history();
        let (old_passes, old_resources) = document_render_targets(&self.document);
        let active = active_renderer_node(&self.document).map(|node| node.id.clone());
        if let Some(node) = self.document.root.nodes.iter_mut().find(|node| {
            active.as_ref() == Some(&node.id) && self.registry.is_renderer(&node.module)
        }) {
            node.module = "render.vulkan".into();
            node.label = "Vulkan renderer".into();
            node.params = serde_json::json!({});
            node.subgraph = Some(Box::new(module_engine_vulkan::default_graph(path)));
            let (passes, resources) = document_render_targets(&self.document);
            module_engine_core::cleanup_removed_render_settings(
                &mut self.document,
                &old_passes.difference(&passes).cloned().collect(),
                &old_resources.difference(&resources).cloned().collect(),
            );
            for id in passes.difference(&old_passes) {
                properties::expose_new_gpu_pass(&mut self.document, id);
            }
            self.graph_path.clear();
            self.graph_canvas.home();
            self.graph_canvas.zoom = 0.82;
            self.policy_baseline = self.document.clone();
            self.changed(true);
            self.flush_history();
        }
    }

    fn poll_candidates(&mut self) {
        self.scripts.poll(&self.document);
        if let Err(error) = self.game_workbench.poll_preparation(&self.document) {
            self.game_preparation_error = Some((error.clone(), error.clone()));
            self.runtime_error = Some(error);
        } else if let Some((cause, displayed)) = &self.game_preparation_error
            && self.runtime_error.as_ref() == Some(displayed)
            && !self.game_workbench.has_pending_preparation()
            && !self
                .game_workbench
                .activity_snapshots()
                .iter()
                .any(|activity| activity.error.as_ref() == Some(cause))
        {
            self.runtime_error = None;
            self.game_preparation_error = None;
        }
        let backend_root = self
            .distribution
            .as_ref()
            .map(|distribution| distribution.package_root.clone())
            .unwrap_or_else(project::source_root);
        if let Some(mut runtime) =
            self.preview_builder
                .poll_at(&self.document, &self.registry, &backend_root)
        {
            if let Err(error) = runtime.apply_project_settings(self.time) {
                self.preview_builder.failed_apply(&error);
                self.runtime_error = Some(error);
                return;
            }
            let authored = runtime
                .objects()
                .unwrap_or_default()
                .into_iter()
                .map(|o| o.id)
                .collect();
            let kept = self
                .runtime
                .as_mut()
                .is_some_and(|previous| runtime.adopt_world(previous));
            if !kept {
                self.authored_object_ids = authored;
                self.time = 0.;
            }
            if let Err(error) = runtime.apply_project_settings(self.time) {
                if kept && let Some(previous) = &mut self.runtime {
                    previous.adopt_world(&mut runtime);
                }
                self.preview_builder.failed_apply(&error);
                self.runtime_error = Some(error);
                self.playing = false;
                return;
            }
            self.runtime = Some(runtime);
            self.preview_builder.applied();
            self.runtime_error = None;
            self.last_render = -1.;
            self.status = "Конфигурация рендера применена".into();
        }
        if self.history_dirty && self.last_tick - self.history_edit_time > 0.45 {
            self.flush_history();
        }
        let ready = self
            .rust_edits
            .iter()
            .filter(|(_, time)| self.last_tick - **time > 0.65)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ready {
            self.rust_edits.remove(&id);
            if self.is_game_source(&id)
                && !self.document.root.nodes.iter().any(|node| {
                    node.module.starts_with("custom.rust")
                        && node
                            .params
                            .get("source_id")
                            .and_then(serde_json::Value::as_str)
                            == Some(id.as_str())
                })
            {
                // Game instances use explicit Apply and their own host/state.
                // Do not start a second, invisible legacy preview compilation.
                continue;
            }
            if let Some(source) = self.document.custom_sources.get(&id) {
                let update = self.is_update_id(&id);
                if update || self.scripts.has_active(&id) {
                    self.scripts.request(
                        &id,
                        source,
                        if update {
                            ScriptKind::Update
                        } else {
                            ScriptKind::Speed
                        },
                    );
                }
            }
        }
    }

    fn graph(&self) -> &Graph {
        let mut graph = &self.document.root;
        for &index in &self.graph_path {
            match graph
                .nodes
                .get(index)
                .and_then(|node| node.subgraph.as_deref())
            {
                Some(inner) => graph = inner,
                None => break,
            }
        }
        graph
    }

    fn graph_mut(&mut self) -> &mut Graph {
        fn descend<'a>(graph: &'a mut Graph, path: &[usize]) -> &'a mut Graph {
            if let Some((&index, rest)) = path.split_first()
                && index < graph.nodes.len()
                && graph.nodes[index].subgraph.is_some()
            {
                return descend(graph.nodes[index].subgraph.as_deref_mut().unwrap(), rest);
            }
            graph
        }
        descend(&mut self.document.root, &self.graph_path)
    }

    fn changed(&mut self, rebuild: bool) {
        if self.document.game.is_some()
            && let Some(active_kind) = self.game_workbench.game_world_kind()
            && let Ok(kind) = crate::game_host::selected_world_kind(&self.document, &self.registry)
            && active_kind != kind
        {
            self.game_workbench.stop();
            self.playing = false;
        }
        if self.authoring_intent && self.can_author_engine() {
            self.policy_baseline = self.document.clone();
        }
        self.dirty = true;
        self.history_dirty = true;
        self.history_edit_time = self.last_tick;
        if !self.json_dirty {
            self.json_draft = serde_json::to_string_pretty(&self.document).unwrap_or_default();
        }
        if rebuild {
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        self.diagnostics = validate_graph(&self.document, &self.registry);
        if self
            .document
            .root
            .nodes
            .iter()
            .any(|n| n.module == "render.vulkan")
        {
            let backend_root = self
                .distribution
                .as_ref()
                .map(|distribution| distribution.package_root.clone())
                .unwrap_or_else(project::source_root);
            self.preview_builder
                .request_at(&self.document, &self.registry, &backend_root);
            self.last_render = -1.;
            return;
        }
        match Runtime::from_document(&self.document, &self.registry) {
            Ok(mut runtime) => {
                let authored = runtime
                    .objects()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|o| o.id)
                    .collect();
                let retained = self
                    .runtime
                    .as_mut()
                    .is_some_and(|previous| runtime.adopt_world(previous));
                if let Err(error) = runtime.apply_project_settings(self.time) {
                    self.runtime_error = Some(error);
                    self.playing = false;
                }
                self.runtime = Some(runtime);
                self.runtime_error = None;
                if !retained {
                    self.authored_object_ids = authored;
                    self.time = 0.;
                }
            }
            Err(error) => {
                self.runtime = None;
                self.runtime_error = Some(error.to_string());
            }
        }
        self.last_render = -1.;
    }

    fn parse_custom(&mut self) {
        self.code_canvas.pending = None;
        if let Some(source) = self.document.custom_sources.get(&self.current_source) {
            if self.is_update_source() {
                match CommandProgram::parse(source) {
                    Ok(program) => {
                        self.command_program = Some(program);
                        self.program = None;
                        self.source_error = None
                    }
                    Err(error) => {
                        self.source_error = Some(error.to_string());
                    }
                }
                return;
            }
            self.command_program = None;
            match CustomProgram::parse(source) {
                Ok(program) => {
                    self.program = Some(program);
                    self.source_error = None;
                }
                Err(error) => {
                    self.source_error = Some(error.to_string());
                }
            }
        } else {
            self.source_error = Some("Источник Custom отсутствует".into());
            self.program = None;
            self.command_program = None;
        }
    }

    fn code_layout_keys(&self) -> BTreeMap<String, String> {
        if let Some(program) = &self.command_program {
            return crate::layout::signatures(
                program
                    .nodes()
                    .iter()
                    .map(|n| {
                        (
                            n.id.clone(),
                            serde_json::to_value(&n.kind).expect("serializable node"),
                        )
                    })
                    .collect(),
                program.edges(),
            );
        }
        self.program
            .as_ref()
            .map(|program| {
                crate::layout::signatures(
                    program
                        .nodes()
                        .iter()
                        .map(|n| {
                            (
                                n.id.clone(),
                                serde_json::to_value(&n.kind).expect("serializable node"),
                            )
                        })
                        .collect(),
                    program.edges(),
                )
            })
            .unwrap_or_default()
    }

    fn persist_code_positions(&mut self) {
        fn visit(
            graph: &mut Graph,
            key: &str,
            value: &serde_json::Value,
            keys: &serde_json::Value,
        ) {
            for node in &mut graph.nodes {
                if (node.module == "custom.rust" || node.module == "custom.rust.update")
                    && node
                        .params
                        .get("source_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(key)
                {
                    node.params["code_layout"] = value.clone();
                    node.params["code_layout_keys"] = keys.clone();
                }
                if let Some(inner) = &mut node.subgraph {
                    visit(inner, key, value, keys);
                }
            }
        }
        if let Ok(value) = serde_json::to_value(&self.code_positions) {
            let keys = serde_json::to_value(self.code_layout_keys()).expect("serializable layout");
            visit(&mut self.document.root, &self.current_source, &value, &keys);
        }
    }

    fn restore_code_positions(&mut self) {
        fn find(
            graph: &Graph,
            key: &str,
        ) -> Option<(serde_json::Value, Option<serde_json::Value>)> {
            for node in &graph.nodes {
                if (node.module == "custom.rust" || node.module == "custom.rust.update")
                    && node
                        .params
                        .get("source_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(key)
                {
                    return node
                        .params
                        .get("code_layout")
                        .cloned()
                        .map(|v| (v, node.params.get("code_layout_keys").cloned()));
                }
                if let Some(inner) = &node.subgraph
                    && let Some(value) = find(inner, key)
                {
                    return Some(value);
                }
            }
            None
        }
        self.code_positions = find(&self.document.root, &self.current_source)
            .map(|(positions, keys)| {
                let positions = serde_json::from_value::<BTreeMap<String, [f32; 2]>>(positions)
                    .unwrap_or_default();
                let keys =
                    keys.and_then(|v| serde_json::from_value::<BTreeMap<String, String>>(v).ok());
                keys.map(|keys| crate::layout::remap(&positions, &keys, &self.code_layout_keys()))
                    .unwrap_or(positions)
            })
            .unwrap_or_default();
    }

    fn apply_json(&mut self) -> bool {
        if self.game_workbench.has_graph_draft() {
            self.status =
                "Примените или отмените черновик Gameplay Systems перед применением JSON".into();
            return false;
        }
        match serde_json::from_str::<GraphDocument>(&self.json_draft) {
            Ok(document) => {
                if let Err(error) = self.validate_candidate(&document, self.can_author_engine()) {
                    self.status = error;
                    return false;
                }
                self.apply_validated_json(document);
                self.status = "Файл применён. Проверка графа обновлена.".into();
                true
            }
            Err(error) => {
                self.status = format!("JSON не применён: {error}");
                false
            }
        }
    }

    /// Install a validated candidate after explicit Apply or successful Save As.
    /// Preparing or cancelling a file dialog must never call this state boundary.
    fn apply_validated_json(&mut self, document: GraphDocument) {
        if document.game != self.document.game
            || document.custom_sources != self.document.custom_sources
        {
            self.game_workbench.stop();
            if document.game.is_some() {
                self.playing = false;
            }
        }
        self.document = document;
        if self.can_author_engine() {
            self.policy_baseline = self.document.clone();
        }
        self.graph_path.clear();
        self.graph_canvas.selected = None;
        self.graph_canvas.pending = None;
        self.code_canvas.selected = None;
        self.program = None;
        self.command_program = None;
        self.json_dirty = false;
        self.code_positions.clear();
        self.changed(true);
        self.parse_custom();
        self.restore_code_positions();
        self.compile_connected_scripts();
    }

    fn save(&mut self) -> bool {
        if let Err(error) = self.validate_candidate(&self.document, self.can_author_engine()) {
            self.status = error;
            return false;
        }
        if self.json_dirty && !self.apply_json() {
            return false;
        }
        self.flush_history();
        match project::save(&PathBuf::from(&self.path), &self.document) {
            Ok(()) => {
                self.dirty = false;
                self.status = format!("Сохранено: {}", self.path);
                true
            }
            Err(error) => {
                self.status = error;
                false
            }
        }
    }

    /// The file editor is a transaction: resolve its draft before editing the
    /// shared document in another view, so later saving cannot replace newer work.
    fn finish_tab_change(&mut self, previous: Tab) {
        if previous == Tab::File && self.tab != Tab::File && self.json_dirty && !self.apply_json() {
            self.tab = Tab::File;
        }
    }

    fn discard_json_draft(&mut self) {
        self.json_draft = serde_json::to_string_pretty(&self.document).unwrap_or_default();
        self.json_dirty = false;
        self.status = "JSON-черновик отменён. Показан текущий документ.".into();
    }

    fn load(&mut self) -> bool {
        match project::read(&PathBuf::from(&self.path)) {
            Ok(document) => {
                let requested = self
                    .distribution
                    .as_ref()
                    .filter(|p| p.profile == crate::distribution::DistributionProfile::Team)
                    .map(|p| p.profile);
                let distribution = match crate::distribution::detect(
                    &PathBuf::from(&self.path),
                    &document,
                    requested,
                )
                .and_then(|p| p.validate_document(&document).map(|_| p))
                {
                    Ok(p) => p,
                    Err(error) => {
                        self.status = error;
                        return false;
                    }
                };
                let registry = match host::registry_at(&distribution.package_root) {
                    Ok(registry) => registry,
                    Err(error) => {
                        self.status = error;
                        return false;
                    }
                };
                self.registry = registry;
                self.game_workbench = crate::game_ui::GameWorkbench::default();
                self.game_workbench
                    .set_deferred_preparation(self.interactive_ui);
                self.game_workbench
                    .set_background_game(self.interactive_ui, None);
                self.game_preparation_error = None;
                self.game_execution_error = None;
                if document.game.is_some() {
                    self.playing = false;
                }
                self.authored_object_ids.clear();
                self.distribution = Some(distribution);
                self.policy_baseline = document.clone();
                self.document = document;
                self.load_personal_layouts();
                self.scripts = match self.distribution.as_ref().map(|p| p.compile_root()) {
                    Some(Ok(root)) => Scripts::with_root(root),
                    Some(Err(error)) => Scripts::disabled(error),
                    None => Scripts::disabled("Distribution unavailable".into()),
                };
                if let Err(error) = self.scripts.set_asset_root(&self.project_asset_root()) {
                    self.status = error;
                }
                self.preview_builder = PreviewBuilder::new();
                self.runtime = None;
                self.authored_object_ids.clear();
                self.time = 0.;
                self.rust_edits.clear();
                self.graph_path.clear();
                self.graph_canvas.selected = None;
                self.graph_canvas.pending = None;
                self.code_canvas.selected = None;
                self.program = None;
                self.command_program = None;
                self.code_positions.clear();
                self.json_dirty = false;
                self.json_draft = serde_json::to_string_pretty(&self.document).unwrap_or_default();
                if !self
                    .document
                    .custom_sources
                    .contains_key(&self.current_source)
                {
                    self.current_source = self
                        .document
                        .custom_sources
                        .keys()
                        .next()
                        .cloned()
                        .unwrap_or_default();
                }
                self.rebuild();
                self.parse_custom();
                self.restore_code_positions();
                self.dirty = false;
                self.history = SnapshotHistory::new(self.document.clone(), 200);
                self.history_dirty = false;
                self.compile_connected_scripts();
                self.status = format!("Открыто: {}", self.path);
                true
            }
            Err(error) => {
                self.status = error;
                false
            }
        }
    }

    fn switch_world(&mut self, kind: WorldKind) {
        if !self.can_author_engine() {
            self.status = "Engine authoring unavailable in this profile/source package".into();
            return;
        }
        if self.json_dirty {
            return;
        }
        let module = match kind {
            WorldKind::Objects => "world.objects",
            WorldKind::Ecs => "world.ecs",
            WorldKind::Custom => return,
        };
        let active_id = self.runtime.as_ref().map(|r| r.world_node_id().to_owned());
        let index = active_id
            .as_ref()
            .and_then(|id| self.document.root.nodes.iter().position(|n| &n.id == id))
            .or_else(|| {
                self.document
                    .root
                    .nodes
                    .iter()
                    .position(|n| self.registry.is_world_provider(&n.module))
            });
        if let Some(node) = index.and_then(|i| self.document.root.nodes.get_mut(i)) {
            node.module = module.into();
            self.game_workbench.stop();
            if self.document.game.is_some() {
                self.playing = false;
            }
            self.policy_baseline = self.document.clone();
            self.changed(true);
            self.status = "Изменён модуль мира. Рендер и его внутренний граф сохранены.".into();
        }
    }

    fn add_node(&mut self, module: &str) {
        if !self.can_author_engine() {
            self.status = "Engine authoring unavailable in this profile/source package".into();
            return;
        }
        if self.json_dirty {
            return;
        }
        if !self.module_available_here(module) {
            self.status = "Этот модуль недоступен в выбранном графе.".into();
            return;
        }
        self.flush_history();
        let count = self.graph().nodes.len();
        let mut index = count + 1;
        let id = loop {
            let id = format!("node_{index}");
            if !self.graph().nodes.iter().any(|n| n.id == id) {
                break id;
            }
            index += 1;
        };
        let label = self
            .registry
            .descriptor(module)
            .map(|d| d.label.clone())
            .unwrap_or_else(|| module.into());
        let params = match module {
            "scalar.value" => serde_json::json!({"value":1.0}),
            "gpu.ssao" => serde_json::json!({"strength":0.8,"enabled":true}),
            "gpu.tonemap" => serde_json::json!({"exposure":1.0,"mode":"aces"}),
            "gpu.fxaa" => serde_json::json!({"enabled":true}),
            "custom.rust" | "custom.rust.update" => {
                serde_json::json!({"source_id":unused_source_id(&self.document.custom_sources, &format!("custom_{id}"))})
            }
            "gpu.custom" => {
                serde_json::json!({"source_id":unused_source_id(&self.document.shader_sources, &format!("hlsl_{id}")),"manifest":module_engine_vulkan::custom_compute_manifest(),"gain":1.0})
            }
            "interface.input" => serde_json::json!({"port":"scene","data_type":"Scene3D"}),
            "interface.output" => serde_json::json!({"port":"frame","data_type":"Frame3D"}),
            _ => serde_json::json!({}),
        };
        let mut node = make_node(
            &id,
            module,
            &label,
            [
                120. + (count % 3) as f32 * 225.,
                90. + (count / 3) as f32 * 155.,
            ],
            params,
        );
        let node_size = |node: &module_engine_core::Node| {
            let (inputs, outputs) = node_ports(node, &self.registry).unwrap_or_default();
            Vec2::new(
                canvas::NODE_WIDTH,
                62. + 25. * inputs.len().max(outputs.len()).max(1) as f32,
            )
        };
        let occupied = self
            .graph()
            .nodes
            .iter()
            .map(|node| {
                egui::Rect::from_min_size(node.position.into(), node_size(node)).expand(12.)
            })
            .collect::<Vec<_>>();
        for index in count..count + 1024 {
            let position = [
                120. + (index % 3) as f32 * 225.,
                90. + (index / 3) as f32 * 155.,
            ];
            let rect = egui::Rect::from_min_size(position.into(), node_size(&node));
            if occupied.iter().all(|other| !other.intersects(rect)) {
                node.position = position;
                break;
            }
        }
        if let Some(canvas) = self.graph_canvas.layout.rect {
            let size = node_size(&node) * self.graph_canvas.zoom;
            let position = Vec2::from(node.position) * self.graph_canvas.zoom;
            let screen =
                egui::Rect::from_min_size(canvas.min + self.graph_canvas.pan + position, size);
            if !canvas.shrink(12.).contains_rect(screen) {
                self.graph_canvas.pan = canvas.size() * 0.5 - position - size * 0.5;
                if size.y > canvas.height() - 24. {
                    self.graph_canvas.pan.y = 12. - position.y;
                }
            }
        }
        if module == "render.software3d" {
            node.subgraph = Some(Box::new(software3d_graph()));
        }
        if module == "render.vulkan" {
            node.subgraph = Some(Box::new(module_engine_vulkan::default_graph(
                RenderPath::Deferred,
            )));
        }
        if module == "custom.rust" {
            let key = node.params["source_id"].as_str().unwrap().to_owned();
            self.document.custom_sources.insert(key, demo_source());
        }
        if module == "custom.rust.update" {
            let key = node.params["source_id"].as_str().unwrap().to_owned();
            self.document
                .custom_sources
                .insert(key, module_engine_script::command_demo_source());
        }
        if module == "gpu.custom" {
            let key = node.params["source_id"].as_str().unwrap().to_owned();
            self.document
                .shader_sources
                .insert(key, module_engine_vulkan::demo_custom_compute_source());
        }
        self.graph_mut().nodes.push(node);
        // Pass IDs are local to each renderer. Only the renderer wired to the
        // viewport owns the global Project controls.
        if self.graph_path.len() == 1
            && active_vulkan_node(&self.document).is_some_and(|renderer| {
                self.document
                    .root
                    .nodes
                    .get(self.graph_path[0])
                    .is_some_and(|n| n.id == renderer.id)
            })
        {
            properties::expose_new_gpu_pass(&mut self.document, &id);
        }
        self.graph_canvas.selected = Some(id);
        self.inspector_id = None;
        self.policy_baseline = self.document.clone();
        self.changed(true);
        self.flush_history();
        self.status = "Узел добавлен. Соедините его совместимые пины.".into();
    }

    fn engine_nodes(&self) -> Vec<CanvasNode> {
        self.graph()
            .nodes
            .iter()
            .map(|node| {
                let (inputs, outputs) = node_ports(node, &self.registry).unwrap_or_default();
                CanvasNode {
                    id: node.id.clone(),
                    label: node.label.clone(),
                    subtitle: node.module.clone(),
                    position: node.position,
                    inputs: inputs
                        .into_iter()
                        .map(|p| CanvasPort {
                            name: p.name,
                            data_type: p.data_type,
                        })
                        .collect(),
                    outputs: outputs
                        .into_iter()
                        .map(|p| CanvasPort {
                            name: p.name,
                            data_type: p.data_type,
                        })
                        .collect(),
                    expandable: node.subgraph.is_some()
                        || node.module.starts_with("custom.rust")
                        || node.module == "gpu.custom",
                    opaque: false,
                }
            })
            .collect()
    }

    fn engine_actions(&mut self, actions: Vec<CanvasAction>) {
        if !self.can_author_engine() {
            self.status = "Engine authoring unavailable in this profile/source package".into();
            return;
        }
        if self.json_dirty {
            return;
        }
        for action in actions {
            match action {
                CanvasAction::Select(id) => {
                    self.graph_canvas.selected = Some(id);
                    self.inspector_id = None;
                }
                CanvasAction::Move(id, pos) => {
                    if let Some(node) = self.graph_mut().nodes.iter_mut().find(|n| n.id == id) {
                        node.position = pos;
                        self.changed(false);
                    }
                }
                CanvasAction::Open(id) => {
                    if let Some(index) = self.graph().nodes.iter().position(|n| n.id == id) {
                        let node = &self.graph().nodes[index];
                        if node.subgraph.is_some() {
                            let gpu = node.module == "render.vulkan";
                            self.graph_path.push(index);
                            self.graph_canvas.home();
                            self.graph_canvas.zoom = if gpu { 0.48 } else { 0.9 };
                            self.graph_canvas.selected = None;
                        } else if node.module.starts_with("custom.rust")
                            && let Some(source) = node
                                .params
                                .get("source_id")
                                .and_then(serde_json::Value::as_str)
                        {
                            let next = source.to_owned();
                            if self.current_source != next {
                                self.program = None;
                                self.command_program = None;
                                self.code_canvas.selected = None;
                                self.code_positions.clear();
                            }
                            self.current_source = next;
                            self.tab = Tab::Custom;
                            self.parse_custom();
                            self.restore_code_positions();
                        } else if node.module == "gpu.custom" {
                            self.current_shader = node
                                .params
                                .get("source_id")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_owned();
                            self.tab = Tab::Shader;
                        }
                    }
                }
                CanvasAction::Connect {
                    from_node,
                    from_port,
                    to_node,
                    to_port,
                } => {
                    if let Some(error) = graph_connection_error(
                        self.graph(),
                        &self.registry,
                        &from_node,
                        &from_port,
                        &to_node,
                        &to_port,
                    ) {
                        self.graph_canvas.message = Some(error.clone());
                        self.status = error;
                        continue;
                    }
                    self.flush_history();
                    let before = self.graph().clone();
                    self.graph_mut()
                        .edges
                        .retain(|e| e.to_node != to_node || e.to_port != to_port);
                    self.graph_mut()
                        .edges
                        .push(make_edge(&from_node, &from_port, &to_node, &to_port));
                    let diagnostics = validate_graph(&self.document, &self.registry);
                    if diagnostics
                        .iter()
                        .any(|d| d.severity == Severity::Error && d.message.contains("cycle"))
                    {
                        *self.graph_mut() = before;
                        self.status = "Связь отклонена: она создаёт цикл зависимостей.".into();
                    } else {
                        self.changed(true);
                        self.flush_history();
                        self.status = "Связь обновлена.".into();
                    }
                }
                CanvasAction::DeleteEdge(index) => {
                    if index < self.graph().edges.len() {
                        self.flush_history();
                        self.graph_mut().edges.remove(index);
                        self.changed(true);
                        self.flush_history();
                    }
                }
            }
        }
    }

    fn code_nodes(&self) -> Vec<CanvasNode> {
        if let Some(program) = &self.command_program {
            return program
                .nodes()
                .iter()
                .map(|node| CanvasNode {
                    id: node.id.clone(),
                    label: node.label.clone(),
                    subtitle: match &node.kind {
                        CommandNodeKind::WorldCommand { .. } => "world command",
                        CommandNodeKind::RenderCommand { .. } => "render command",
                        CommandNodeKind::Number { .. } => "Rust numeric literal",
                        CommandNodeKind::Text { .. } => "Rust string",
                        CommandNodeKind::Boolean { .. } => "Rust bool",
                        CommandNodeKind::Opaque { .. } => "Rust fragment (text)",
                    }
                    .into(),
                    position: self
                        .code_positions
                        .get(&node.id)
                        .copied()
                        .unwrap_or(node.position),
                    inputs: program
                        .node_input_types(&node.id)
                        .iter()
                        .enumerate()
                        .map(|(index, kind)| CanvasPort {
                            name: index.to_string(),
                            data_type: kind.clone(),
                        })
                        .collect(),
                    outputs: program
                        .node_output_type(&node.id)
                        .map(|kind| {
                            vec![CanvasPort {
                                name: "value".into(),
                                data_type: kind.into(),
                            }]
                        })
                        .unwrap_or_default(),
                    expandable: false,
                    opaque: matches!(node.kind, CommandNodeKind::Opaque { .. }),
                })
                .collect();
        }
        let Some(program) = &self.program else {
            return vec![];
        };
        program
            .nodes()
            .iter()
            .map(|node| {
                let count = match node.kind {
                    CodeNodeKind::Input { .. } | CodeNodeKind::Number { .. } => 0,
                    CodeNodeKind::Binary { .. } => 2,
                    CodeNodeKind::Output => 1,
                    _ => program
                        .edges()
                        .iter()
                        .filter(|e| e.to == node.id)
                        .map(|e| e.input + 1)
                        .max()
                        .unwrap_or(0),
                };
                let data_type = if program.node_is_scalar(&node.id) {
                    "f64"
                } else {
                    "UnknownRust"
                };
                CanvasNode {
                    id: node.id.clone(),
                    label: node.label.clone(),
                    subtitle: match &node.kind {
                        CodeNodeKind::Input { .. } => "input: f64",
                        CodeNodeKind::Number { .. } => "literal: f64",
                        CodeNodeKind::Binary { .. } => "Rust arithmetic",
                        CodeNodeKind::Call { .. } => "Rust call",
                        CodeNodeKind::Output => "return: f64",
                        CodeNodeKind::Opaque { .. } => "Rust fragment (text)",
                    }
                    .into(),
                    position: self
                        .code_positions
                        .get(&node.id)
                        .copied()
                        .unwrap_or([node.position[0] * 1.25, node.position[1] * 1.8]),
                    inputs: (0..count)
                        .map(|i| CanvasPort {
                            name: i.to_string(),
                            data_type: data_type.into(),
                        })
                        .collect(),
                    outputs: if matches!(node.kind, CodeNodeKind::Output) {
                        vec![]
                    } else {
                        vec![CanvasPort {
                            name: "value".into(),
                            data_type: data_type.into(),
                        }]
                    },
                    expandable: false,
                    opaque: matches!(node.kind, CodeNodeKind::Opaque { .. }),
                }
            })
            .collect()
    }

    fn sync_program_source(&mut self, topology_changed: bool) {
        if !self.can_edit_source(&self.current_source) {
            return;
        }
        if let Some(program) = &self.command_program {
            self.document
                .custom_sources
                .insert(self.current_source.clone(), program.source().into());
            self.source_error = None;
            self.rust_edits
                .insert(self.current_source.clone(), self.last_tick);
            self.changed(false);
            return;
        }
        if let Some(program) = &self.program {
            self.document
                .custom_sources
                .insert(self.current_source.clone(), program.source().into());
            self.source_error = None;
            if topology_changed {
                self.code_canvas.selected = None;
                self.code_canvas.pending = None;
                self.restore_code_positions();
                self.persist_code_positions();
            }
            self.changed(false);
            self.rust_edits
                .insert(self.current_source.clone(), self.last_tick);
        }
    }

    fn code_actions(&mut self, actions: Vec<CanvasAction>) {
        if !self.can_edit_source(&self.current_source) {
            return;
        }
        if self.source_error.is_some() {
            return;
        }
        for action in actions {
            match action {
                CanvasAction::Move(id, pos) => {
                    self.code_positions.insert(id, pos);
                    self.persist_code_positions();
                    self.changed(false);
                }
                CanvasAction::Connect {
                    from_node,
                    to_node,
                    to_port,
                    ..
                } => {
                    if let Some(program) = &mut self.command_program {
                        match program.connect(&from_node, &to_node, to_port.parse().unwrap_or(0)) {
                            Ok(()) => {
                                self.sync_program_source(true);
                                self.status = "Rust-команды обновлены".into();
                            }
                            Err(error) => self.status = error.to_string(),
                        }
                        continue;
                    }
                    if let Some(program) = &mut self.program {
                        match program.connect(&from_node, &to_node, to_port.parse().unwrap_or(0)) {
                            Ok(()) => {
                                self.sync_program_source(true);
                                self.status = "Граф изменён, Rust-исходник обновлён.".into();
                            }
                            Err(error) => self.status = error.to_string(),
                        }
                    }
                }
                CanvasAction::DeleteEdge(_) => self.status =
                    "Удаление входа Rust-выражения требует корректной замены в текстовом режиме."
                        .into(),
                _ => {}
            }
        }
    }

    fn tick_simulation(&mut self, dt: f64) {
        if self.document.game.is_some() {
            // Game may already be open before Play. Refresh its stopped copy
            // from author data before graphics sync can accept that document.
            if self.playing || self.game_workbench.has_game_host() {
                let (root, compile, kind) = self.game_context();
                if let Err(error) =
                    self.game_workbench
                        .ensure(&self.document, &root, compile.as_deref(), kind)
                {
                    self.game_preparation_error = Some((error.clone(), error.clone()));
                    self.game_workbench.suspend_game_execution();
                    self.runtime_error = Some(error);
                    self.playing = false;
                    return;
                }
            }
            if let Err(error) = self.game_workbench.sync_graphics(&self.document) {
                let displayed =
                    format!("Graphics update rejected; previous renderer active: {error}");
                self.game_preparation_error = Some((error, displayed.clone()));
                self.runtime_error = Some(displayed);
            }
            if let Err(error) = self.game_workbench.tick(dt, self.playing) {
                self.game_execution_error = Some(error.clone());
                self.runtime_error = Some(error);
                self.playing = false;
            }
            if self.game_workbench.take_restart_recovered()
                && self
                    .game_execution_error
                    .as_ref()
                    .is_some_and(|error| self.runtime_error.as_ref() == Some(error))
            {
                self.runtime_error = None;
                self.game_execution_error = None;
            }
            return;
        }
        if !self.playing {
            return;
        }
        if let Some(runtime) = &mut self.runtime {
            runtime.begin_frame();
            if let Err(error) = runtime.apply_project_settings(self.time) {
                self.runtime_error = Some(error);
                self.playing = false;
                return;
            }
            let speed_source = runtime.world_speed_source();
            match scalar::world_speed_compiled(
                &self.document,
                speed_source.as_deref(),
                self.time,
                &|id, time, base| self.scripts.speed(id, time, base),
            ) {
                Ok(speed) => {
                    if self.playing {
                        self.runtime_error = None;
                        let mut commands = vec![];
                        let mut script_error = None;
                        for source in runtime.command_sources() {
                            if !self
                                .scripts
                                .has_active_kind(&source.source_id, ScriptKind::Update)
                            {
                                continue;
                            }
                            let context =
                                match crate::script_context(runtime, self.time, dt, source.world) {
                                    Ok(context) => context,
                                    Err(error) => {
                                        script_error =
                                            Some(format!("Custom world query failed: {error}"));
                                        break;
                                    }
                                };
                            match self.scripts.update(&source.source_id, &context) {
                                Ok(result) => {
                                    commands.extend(result.into_iter().filter(|c| match c {
                                        ControlCommand::World(_) => source.world,
                                        ControlCommand::Render(_) => source.render,
                                    }))
                                }
                                Err(error) => {
                                    script_error = Some(error);
                                    break;
                                }
                            }
                        }
                        if let Some(error) =
                            script_error.or_else(|| runtime.apply_project_controls(&commands).err())
                        {
                            let _ = self.scripts.reject_updates();
                            self.runtime_error = Some(error);
                            self.playing = false;
                        } else {
                            if let Err(error) = self.scripts.commit_updates() {
                                self.runtime_error = Some(error);
                                self.playing = false;
                                return;
                            }
                            self.time += dt;
                            runtime.step(dt as f32, speed);
                        }
                    }
                }
                Err(error) => {
                    self.runtime_error = Some(error);
                    self.playing = false;
                }
            }
        }
    }

    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.draw_workspaces(ui);
    }

    pub(super) fn project_asset_root(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf()
    }

    pub(super) fn game_scene_panel(&mut self, ui: &mut egui::Ui) {
        let root = self.project_asset_root();
        let backend = self
            .distribution
            .as_ref()
            .map(|p| p.package_root.clone())
            .unwrap_or_else(project::source_root);
        let compile = self
            .distribution
            .as_ref()
            .and_then(|p| p.compile_root().ok());
        self.game_workbench
            .set_engine_context(compile.as_deref(), &backend);
        let kind = match crate::game_host::selected_world_kind(&self.document, &self.registry) {
            Ok(kind) => kind,
            Err(error) => {
                self.game_workbench.error = Some(error.clone());
                ui.colored_label(Color32::LIGHT_RED, error);
                return;
            }
        };
        let was_playing = self.playing;
        let old = self.document.game.clone();
        if self.game_workbench.scene_ui(
            ui,
            &mut self.document,
            &root,
            compile.as_deref(),
            kind,
            &mut self.playing,
        ) {
            let validation = self.validate_candidate(&self.document, false);
            if let Err(error) = validation {
                self.document.game = old;
                self.game_workbench.error = Some(format!("Scene change rejected: {error}"));
            } else {
                self.changed(false);
            }
        }
        if !was_playing && self.playing {
            self.cancel_active_scene_gestures();
            // Refresh in place if cancelling an unfinished author gesture
            // changed the source after Game's local Play button was pressed.
            if let Err(error) =
                self.game_workbench
                    .ensure(&self.document, &root, compile.as_deref(), kind)
            {
                self.runtime_error = Some(error);
                self.playing = false;
            }
        }
    }
    pub(super) fn game_systems_panel(&mut self, ui: &mut egui::Ui) {
        let root = self.project_asset_root();
        let backend = self
            .distribution
            .as_ref()
            .map(|p| p.package_root.clone())
            .unwrap_or_else(project::source_root);
        let compile = self
            .distribution
            .as_ref()
            .and_then(|p| p.compile_root().ok());
        self.game_workbench
            .set_engine_context(compile.as_deref(), &backend);
        if self
            .game_workbench
            .systems_ui(ui, &mut self.document, &root)
        {
            self.changed(false);
        }
    }
    pub(super) fn stop_game(&mut self) {
        self.cancel_active_scene_gestures();
        self.playing = false;
        self.game_workbench.stop();
        if let Err(error) = self.scripts.reset_game_states() {
            self.status = error;
        }
    }

    fn is_gpu_graph_scope(&self, path: &[usize]) -> bool {
        let mut graph = &self.document.root;
        let mut gpu = false;
        for index in path {
            let Some(node) = graph.nodes.get(*index) else {
                return false;
            };
            let Some(inner) = node.subgraph.as_deref() else {
                return false;
            };
            gpu |= node.module == "render.vulkan";
            graph = inner;
        }
        gpu
    }

    fn module_available_in_scope(&self, module: &str, path: &[usize]) -> bool {
        if self.registry.descriptor(module).is_none() {
            return false;
        }
        let gpu_scope = self.is_gpu_graph_scope(path);
        if gpu_scope {
            return module.starts_with("gpu.") || module.starts_with("interface.");
        }
        if path.is_empty() {
            !module.starts_with("gpu.")
                && !module.starts_with("interface.")
                && module != "render.rasterize"
        } else {
            !module.starts_with("gpu.")
                && !self.registry.is_world_provider(module)
                && !self.registry.is_renderer(module)
                && !matches!(module, "viewport.output" | "scene.extract")
                && !module.starts_with("custom.rust")
        }
    }

    fn module_available_here(&self, module: &str) -> bool {
        self.module_available_in_scope(module, &self.graph_path)
    }

    fn module_choices(&self, path: &[usize]) -> Vec<module_engine_core::ModuleDescriptor> {
        let search = self.module_search.trim().to_lowercase();
        self.registry
            .descriptors()
            .iter()
            .filter(|descriptor| {
                (!descriptor.inputs.is_empty()
                    || !descriptor.outputs.is_empty()
                    || matches!(
                        descriptor.id.as_str(),
                        "interface.input" | "interface.output"
                    ))
                    && self.module_available_in_scope(&descriptor.id, path)
                    && (search.is_empty()
                        || descriptor.label.to_lowercase().contains(&search)
                        || descriptor.id.to_lowercase().contains(&search))
            })
            .cloned()
            .collect()
    }

    fn add_node_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("+ Узел", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.module_search).hint_text("Поиск модулей"));
            let choices = self.module_choices(&self.graph_path);
            egui::ScrollArea::vertical()
                .max_height(300.)
                .show(ui, |ui| {
                    for descriptor in choices {
                        if ui
                            .button(&descriptor.label)
                            .on_hover_text(&descriptor.description)
                            .clicked()
                        {
                            self.add_node(&descriptor.id);
                            ui.close();
                        }
                    }
                });
        });
    }

    fn library(&mut self, ui: &mut egui::Ui) {
        ui.heading("Модули");
        ui.label(
            RichText::new("Добавьте узел и соедините пины одного типа.")
                .size(11.0)
                .color(Color32::from_rgb(144, 158, 181)),
        );
        let target = self.library_graph_context();
        if let Some((id, _, label)) = &target {
            ui.label(format!("Добавление в: {label} · окно {id}"));
        } else {
            ui.label("Откройте Module Graph и выберите граф для добавления.");
        }
        ui.add(egui::TextEdit::singleline(&mut self.module_search).hint_text("Поиск модулей"));
        ui.add_space(6.0);
        let path = target
            .as_ref()
            .map(|(_, path, _)| path.as_slice())
            .unwrap_or_default();
        let descriptors = self.module_choices(path);
        let gpu_scope = self.is_gpu_graph_scope(path);
        egui::ScrollArea::vertical()
            .max_height(if gpu_scope { 360.0 } else { 420.0 })
            .show(ui, |ui| {
                for descriptor in descriptors {
                    let response = ui
                        .add_enabled_ui(
                            !self.json_dirty && target.is_some() && self.can_author_engine(),
                            |ui| {
                                ui.add_sized(
                                    [ui.available_width(), 29.0],
                                    egui::Button::new(format!("+ {}", descriptor.label)),
                                )
                            },
                        )
                        .inner;
                    if response.clicked() {
                        self.add_library_node(&descriptor.id);
                    }
                    response.on_hover_text(&descriptor.description);
                    ui.add_space(3.0);
                }
            });
        ui.separator();
        ui.label(RichText::new("КОНТРАКТЫ ПИНОВ").size(10.0).strong());
        let pins = if gpu_scope {
            vec![
                ("Scene3D", "геометрия сцены"),
                ("GpuColor", "текстура цвета"),
                ("GpuNormals", "текстура нормалей"),
                ("GpuDepth", "текстура глубины"),
                ("Frame3D", "готовое изображение"),
            ]
        } else {
            vec![
                ("World", "состояние мира"),
                ("Scene3D", "данные для рендера"),
                ("Frame3D", "изображение"),
                ("Scalar", "число"),
            ]
        };
        for (kind, meaning) in pins {
            ui.horizontal(|ui| {
                ui.colored_label(canvas::type_color(kind), "●");
                ui.label(RichText::new(kind).monospace().size(11.0));
            });
            ui.label(
                RichText::new(meaning)
                    .size(10.5)
                    .color(Color32::from_rgb(136, 150, 172)),
            );
        }
        ui.add_space(14.0);
        ui.separator();
        ui.label(RichText::new("ПРОЕКТ С ИСХОДНИКАМИ").size(10.0).strong());
        if ui.button("Экспорт проекта…").clicked() {
            self.dispatch_command(editor_workflow::Command::ExportSource, ui.ctx());
        }
        ui.label(
            RichText::new("Экспорт включает workspace, документацию и текущий project.json.")
                .size(10.5)
                .color(Color32::from_rgb(136, 150, 172)),
        );
    }

    fn viewport_panel(&mut self, ui: &mut egui::Ui, now: f64) {
        ui.horizontal(|ui| {
            ui.heading("3D-предпросмотр");
            if ui
                .small_button(if self.playing {
                    "Пауза"
                } else {
                    "Старт"
                })
                .clicked()
            {
                self.playing = !self.playing;
            }
            if ui.small_button("Сброс").clicked() {
                self.runtime = None;
                self.authored_object_ids.clear();
                self.time = 0.;
                self.runtime_error = None;
                self.rebuild();
                self.camera = Camera::default();
            }
        });
        if self.can_author_engine() && self.workspace == WorkspaceKind::Engine {
            ui.add_enabled_ui(!self.json_dirty, |ui| {
                ui.horizontal(|ui| {
                    let kind = self.runtime.as_ref().map(|r| r.world_kind());
                    if ui
                        .selectable_label(kind == Some(WorldKind::Objects), "Объекты")
                        .clicked()
                    {
                        self.switch_world(WorldKind::Objects);
                    }
                    if ui
                        .selectable_label(kind == Some(WorldKind::Ecs), "ECS")
                        .clicked()
                    {
                        self.switch_world(WorldKind::Ecs);
                    }
                    ui.label(
                        RichText::new("один рендер")
                            .size(10.5)
                            .color(Color32::from_rgb(137, 153, 175)),
                    );
                });
            });
            ui.add_enabled_ui(!self.json_dirty, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("GPU Forward").clicked() {
                        self.switch_gpu_path(RenderPath::Forward);
                    }
                    if ui.button("GPU Deferred").clicked() {
                        self.switch_gpu_path(RenderPath::Deferred);
                    }
                });
            });
        }
        if let Some(runtime) = &self.runtime {
            ui.label(
                RichText::new(runtime.backend_name())
                    .size(10.5)
                    .color(Color32::from_rgb(67, 211, 182)),
            );
        }
        if self.preview_builder.is_pending() {
            ui.label("Сборка GPU-кандидата в фоне…");
        }
        if let Some(error) = &self.preview_builder.error {
            ui.colored_label(
                Color32::from_rgb(245, 134, 129),
                "Новая конфигурация не применена",
            );
            ui.label(RichText::new(error).size(10.5));
        }
        if let Some(error) = &self.runtime_error {
            ui.colored_label(Color32::from_rgb(245, 134, 129), "Предпросмотр остановлен");
            ui.label(RichText::new(error).size(10.5));
        }
        if let Some(runtime) = &self.runtime
            && (now - self.last_render > 0.04 || self.last_render < 0.0)
        {
            let camera = Camera {
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
                distance: self.camera.distance,
                target: self.camera.target,
            };
            match runtime.render(600, 390, camera) {
                Ok(frame) => {
                    self.runtime_error = None;
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [frame.width, frame.height],
                        &frame.rgba,
                    );
                    if let Some(texture) = &mut self.texture {
                        texture.set(image, egui::TextureOptions::LINEAR);
                    } else {
                        self.texture = Some(ui.ctx().load_texture(
                            "3d-scene",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    self.stats = format!(
                        "{} объектов · t = {:.2} с",
                        runtime.object_count(),
                        self.time
                    );
                    self.last_render = now;
                }
                Err(error) if error.starts_with("GPU готовит новую конфигурацию;") =>
                    {}
                Err(error) => self.runtime_error = Some(error),
            }
        }
        if let Some(texture) = &self.texture {
            let width = ui.available_width();
            let response = ui.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(Vec2::new(width, width * 0.65))
                    .sense(egui::Sense::drag()),
            );
            if response.dragged() {
                let delta = response.drag_delta();
                self.camera.yaw -= delta.x * 0.009;
                self.camera.pitch = (self.camera.pitch + delta.y * 0.007).clamp(-1.15, 1.15);
                self.last_render = -1.;
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta().y);
                if scroll != 0.0 {
                    self.camera.distance =
                        (self.camera.distance * (-scroll * 0.002).exp()).clamp(2.0, 25.0);
                    self.last_render = -1.;
                }
            }
        }
        ui.label(
            RichText::new(&self.stats)
                .size(11.0)
                .color(Color32::from_rgb(130, 153, 183)),
        );
        ui.label(
            RichText::new("ЛКМ: орбита · колесо: расстояние")
                .size(10.0)
                .color(Color32::from_rgb(126, 140, 163)),
        );
    }

    fn graph_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if !self.graph_path.is_empty() && ui.button("< Внешний граф").clicked() {
                self.graph_path.pop();
                self.graph_canvas.home();
                self.graph_canvas.zoom = 0.82;
                self.graph_canvas.selected = None;
            }
            ui.label(
                RichText::new(if self.graph_path.is_empty() {
                    &self.document.name
                } else {
                    "Внутренний граф модуля"
                })
                .size(17.0)
                .strong(),
            );
            if ui.small_button("Центр").clicked() {
                self.graph_canvas.home();
                self.graph_canvas.zoom = 0.82;
            }
            self.add_node_menu(ui);
        });
        if let Some(message) = &self.graph_canvas.message {
            ui.colored_label(Color32::from_rgb(234, 177, 96), message);
        }
        let nodes = self.engine_nodes();
        let edges = self
            .graph()
            .edges
            .iter()
            .map(|e| CanvasEdge {
                from_node: e.from_node.clone(),
                from_port: e.from_port.clone(),
                to_node: e.to_node.clone(),
                to_port: e.to_port.clone(),
            })
            .collect::<Vec<_>>();
        let actions = canvas::draw(ui, &mut self.graph_canvas, &nodes, &edges);
        self.engine_actions(actions);
    }

    fn file_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Текстовое описание графа");
            if ui.button("Применить файл").clicked() {
                self.apply_json();
            }
            if ui
                .add_enabled(self.json_dirty, egui::Button::new("Отменить черновик"))
                .clicked()
            {
                self.discard_json_draft();
            }
        });
        ui.label(
            "Изменения здесь меняют тот же проект. Сохранение сначала проверяет синтаксис JSON.",
        );
        let response = ui.add_sized(
            ui.available_size(),
            egui::TextEdit::multiline(&mut self.json_draft)
                .font(egui::TextStyle::Monospace)
                .code_editor()
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            self.json_dirty = true;
            self.dirty = true;
        }
    }

    fn custom_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Custom / Rust");
            let previous = self.current_source.clone();
            egui::ComboBox::from_id_salt("source-choice")
                .selected_text(&self.current_source)
                .show_ui(ui, |ui| {
                    for key in self
                        .document
                        .custom_sources
                        .keys()
                        .filter(|id| self.can_edit_source(id))
                        .cloned()
                        .collect::<Vec<_>>()
                    {
                        ui.selectable_value(&mut self.current_source, key.clone(), &key);
                    }
                });
            if previous != self.current_source {
                self.program = None;
                self.command_program = None;
                self.parse_custom();
                self.restore_code_positions();
                self.code_canvas.selected = None;
            }
            ui.separator();
            ui.selectable_value(&mut self.custom_view, CustomView::Text, "Текст");
            ui.selectable_value(&mut self.custom_view, CustomView::Graph, "Граф");
            ui.selectable_value(&mut self.custom_view, CustomView::Both, "Рядом");
            if !self.is_game_source(&self.current_source)
                && ui.button("Применить Rust").clicked()
                && let Some(source) = self.document.custom_sources.get(&self.current_source)
            {
                self.scripts.request(
                    &self.current_source,
                    source,
                    if self.is_update_source() {
                        ScriptKind::Update
                    } else {
                        ScriptKind::Speed
                    },
                );
            }
            if !self.is_game_source(&self.current_source)
                && self.scripts.has_rejected_candidate(&self.current_source)
                && ui.button("Применить со сбросом состояния").clicked()
            {
                match self.scripts.apply_rejected_with_reset(&self.current_source) {
                    Ok(()) => self.status = "Rust применён; состояние этого Custom сброшено".into(),
                    Err(e) => self.status = e,
                }
            }
        });
        if self.is_game_source(&self.current_source) {
            let (root, compile, kind) = self.game_context();
            if let Err(error) =
                self.game_workbench
                    .ensure(&self.document, &root, compile.as_deref(), kind)
            {
                ui.colored_label(Color32::LIGHT_RED, format!("Подготовка Game: {error}"));
            }
            self.game_workbench.custom_editor_ui(
                ui,
                &self.current_source,
                self.document
                    .custom_sources
                    .get(&self.current_source)
                    .map(String::as_str),
                compile.as_deref(),
            );
        } else {
            if let Some(source) = self.scripts.rejected_source(&self.current_source) {
                ui.collapsing(
                    "Несовместимый кандидат: сохранённый исходник",
                    |ui| {
                        ui.label(source);
                    },
                );
            }
            if self.scripts.has_rejected_candidate(&self.current_source) {
                ui.collapsing(
                    "Явная миграция состояния (JSON)",
                    |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.state_migration)
                                .code_editor()
                                .desired_rows(4),
                        );
                        if ui.button("Применить с этим состоянием").clicked()
                        {
                            match serde_json::from_str(&self.state_migration) {
                                Ok(state) => match self
                                    .scripts
                                    .apply_rejected_with_migration(&self.current_source, state)
                                {
                                    Ok(()) => {
                                        self.status =
                                            "Rust применён с явно заданным состоянием".into()
                                    }
                                    Err(e) => self.status = e,
                                },
                                Err(e) => self.status = e.to_string(),
                            }
                        }
                    },
                );
            }
            if self.scripts.pending(&self.current_source) {
                ui.label(if self.scripts.has_active(&self.current_source) {
                    "Компиляция в фоне. Предыдущая версия остаётся активной."
                } else {
                    "Компиляция в фоне. Код начнёт исполняться после успешной сборки."
                });
            }
            if let Some(error) = self.scripts.errors.get(&self.current_source) {
                egui::CollapsingHeader::new("Диагностика компилятора Rust")
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.colored_label(Color32::from_rgb(246, 134, 130), error);
                    });
            }
            if let Some(active) = self.scripts.active_source(&self.current_source) {
                ui.label(
                    if self
                        .document
                        .custom_sources
                        .get(&self.current_source)
                        .is_some_and(|s| s == active)
                    {
                        "Исполняется скомпилированная версия исходника"
                    } else {
                        "Исходник изменён; исполняется предыдущая скомпилированная версия"
                    },
                );
            }
        }
        ui.label(
            RichText::new(
                "Один исходник · граф правит Rust · неподдерживаемые конструкции остаются текстом",
            )
            .size(11.0)
            .color(Color32::from_rgb(144, 161, 185)),
        );
        if let Some(error) = &self.source_error {
            ui.colored_label(Color32::from_rgb(246, 134, 130), error);
            ui.label("Визуальный разбор недоступен; показан последний разобранный граф. Компилятор Rust проверяет исходник отдельно.");
        }
        match self.custom_view {
            CustomView::Text => self.code_text(ui),
            CustomView::Graph => self.code_graph(ui),
            CustomView::Both => {
                let width = ui.available_width();
                let height = ui.available_height();
                ui.horizontal_top(|ui| {
                    ui.scope_builder(
                        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                            ui.cursor().min,
                            Vec2::new(width * 0.37, height),
                        )),
                        |ui| {
                            ui.set_width(width * 0.37);
                            self.code_text(ui);
                        },
                    );
                    ui.separator();
                    ui.scope_builder(
                        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                            ui.cursor().min,
                            Vec2::new(width * 0.61, height),
                        )),
                        |ui| {
                            ui.set_width(width * 0.61);
                            self.code_graph(ui);
                        },
                    );
                });
            }
        }
    }

    fn code_text(&mut self, ui: &mut egui::Ui) {
        if !self.can_edit_source(&self.current_source) {
            ui.label("This source is Engine-owned");
            return;
        }
        let Some(source) = self.document.custom_sources.get_mut(&self.current_source) else {
            ui.label("Добавьте Custom-узел в граф.");
            return;
        };
        let response = ui.add_sized(
            ui.available_size(),
            egui::TextEdit::multiline(source)
                .font(egui::TextStyle::Monospace)
                .code_editor()
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            self.parse_custom();
            if self.source_error.is_none() {
                self.code_canvas.selected = None;
                self.restore_code_positions();
                self.persist_code_positions();
            }
            self.changed(false);
            self.rust_edits
                .insert(self.current_source.clone(), self.last_tick);
        }
    }

    fn code_graph(&mut self, ui: &mut egui::Ui) {
        let nodes = self.code_nodes();
        let mut edges = self
            .program
            .as_ref()
            .map(|p| {
                p.edges()
                    .iter()
                    .map(|e| CanvasEdge {
                        from_node: e.from.clone(),
                        from_port: "value".into(),
                        to_node: e.to.clone(),
                        to_port: e.input.to_string(),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(program) = &self.command_program {
            edges = program
                .edges()
                .iter()
                .map(|e| CanvasEdge {
                    from_node: e.from.clone(),
                    from_port: "value".into(),
                    to_node: e.to.clone(),
                    to_port: e.input.to_string(),
                })
                .collect();
        }
        let actions = canvas::draw(ui, &mut self.code_canvas, &nodes, &edges);
        self.code_actions(actions);
    }

    /// Delete the node and its declared controls as one document/history change.
    /// Settings follow the active viewport renderer, not unrelated graph scopes.
    fn delete_engine_node(&mut self, id: &str) {
        if !self.can_author_engine() || self.json_dirty {
            self.status = "Удаление узлов требует доступного Engine без JSON-черновика".into();
            return;
        }
        if !self.graph().nodes.iter().any(|node| node.id == id) {
            return;
        }
        self.flush_history();
        let (old_passes, old_resources) = document_render_targets(&self.document);
        let restored = module_engine_vulkan::remove_pass_with_bypass(self.graph_mut(), id)
            .expect("selected node exists");
        let (passes, resources) = document_render_targets(&self.document);
        let removed_passes = old_passes.difference(&passes).cloned().collect();
        let removed_resources = old_resources.difference(&resources).cloned().collect();
        let cleanup = module_engine_core::cleanup_removed_render_settings(
            &mut self.document,
            &removed_passes,
            &removed_resources,
        );
        self.graph_canvas.selected = None;
        self.graph_canvas.pending = None;
        self.changed(true);
        self.flush_history();
        self.status = format!(
            "Узел {id} удалён; восстановлено связей: {restored}; обновлено настроек: {}",
            cleanup.total_changes(),
        );
    }

    fn node_inspector(&mut self, ui: &mut egui::Ui) {
        if self.json_dirty {
            ui.label("Примените или отмените JSON-черновик для редактирования узлов.");
            return;
        }
        let selected = self.graph_canvas.selected.clone();
        let Some(node) = selected
            .as_ref()
            .and_then(|id| self.graph().nodes.iter().find(|n| &n.id == id))
            .cloned()
        else {
            ui.label(
                RichText::new("Выберите узел для настройки. Двойной щелчок раскрывает модуль.")
                    .size(11.0)
                    .color(Color32::from_rgb(148, 161, 182)),
            );
            return;
        };
        ui.label(RichText::new(&node.label).strong());
        ui.label(RichText::new(&node.module).monospace().size(10.0));
        if let Some(descriptor) = self.registry.descriptor(&node.module) {
            ui.label(RichText::new(&descriptor.description).size(10.5));
        }
        if node.module.starts_with("gpu.") {
            self.gpu_pass_controls(ui, &node);
        }
        ui.horizontal(|ui| {
            if node.subgraph.is_some() && ui.button("Открыть граф").clicked() {
                self.engine_actions(vec![CanvasAction::Open(node.id.clone())]);
            }
            if node.module.starts_with("custom.rust") && ui.button("Редактировать Rust").clicked()
            {
                self.engine_actions(vec![CanvasAction::Open(node.id.clone())]);
            }
            if ui.button("Удалить").clicked() {
                self.delete_engine_node(&node.id);
            }
        });
        if node.module == "scalar.value" {
            let mut value = node
                .params
                .get("value")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0);
            if ui
                .add(
                    egui::DragValue::new(&mut value)
                        .speed(0.02)
                        .prefix("value = "),
                )
                .changed()
                && let Some(node) = self.graph_mut().nodes.iter_mut().find(|n| n.id == node.id)
            {
                node.params["value"] = serde_json::json!(value);
                self.changed(false);
            }
        }
        if self.inspector_id.as_deref() != Some(&node.id) {
            self.param_draft = serde_json::to_string_pretty(&node.params).unwrap_or_default();
            self.inspector_id = Some(node.id.clone());
        }
        egui::CollapsingHeader::new("Параметры модуля (JSON)").show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.param_draft)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );
            if ui.button("Применить параметры").clicked() {
                match serde_json::from_str::<serde_json::Value>(&self.param_draft) {
                    Ok(params) => {
                        if let Some(target) =
                            self.graph_mut().nodes.iter_mut().find(|n| n.id == node.id)
                        {
                            target.params = params;
                            self.changed(true);
                        }
                    }
                    Err(error) => self.status = format!("Параметры не применены: {error}"),
                }
            }
        });
    }

    fn code_inspector(&mut self, ui: &mut egui::Ui) {
        if self.command_program.is_some() {
            self.command_inspector(ui);
            return;
        }
        let selected = self
            .code_canvas
            .selected
            .as_ref()
            .and_then(|id| self.program.as_ref()?.nodes().iter().find(|n| &n.id == id))
            .cloned();
        let Some(node) = selected else {
            ui.label(
                RichText::new(
                    "Выберите числовой узел или оператор. Изменения появятся в Rust-тексте.",
                )
                .size(11.0),
            );
            return;
        };
        ui.label(RichText::new(&node.label).strong());
        if self.source_error.is_some() {
            ui.label("Графические правки временно отключены: текст содержит ошибку.");
            return;
        }
        let mut result = None;
        match node.kind {
            CodeNodeKind::Number { mut value } => {
                if ui
                    .add(egui::DragValue::new(&mut value).speed(0.02))
                    .changed()
                {
                    result = self.program.as_mut().map(|p| p.set_number(&node.id, value));
                }
            }
            CodeNodeKind::Binary { mut op } => {
                let old = op.clone();
                egui::ComboBox::from_id_salt("operator")
                    .selected_text(&op)
                    .show_ui(ui, |ui| {
                        for symbol in ["+", "-", "*", "/", "%"] {
                            ui.selectable_value(&mut op, symbol.into(), symbol);
                        }
                    });
                if op != old {
                    result = self
                        .program
                        .as_mut()
                        .map(|p| p.set_binary_op(&node.id, &op));
                }
            }
            CodeNodeKind::Opaque { source } => {
                ui.colored_label(Color32::from_rgb(241, 182, 101), "Текстовый Rust-фрагмент");
                ui.label(RichText::new(source).monospace().size(11.0));
                ui.label("Редактируется в текстовом представлении и сохраняется в исходнике.");
            }
            _ => {
                ui.label(
                    RichText::new("Чтобы изменить вход, соедините другой f64-выход с его пином.")
                        .size(11.0),
                );
            }
        }
        if let Some(result) = result {
            match result {
                Ok(()) => {
                    self.sync_program_source(false);
                    self.status = "Rust обновлён графической правкой.".into();
                }
                Err(error) => self.status = error.to_string(),
            }
        }
    }
}

impl eframe::App for Editor {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        self.workflow_capture_input(input);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.interactive_ui = true;
        self.game_workbench.set_deferred_preparation(true);
        self.game_workbench
            .set_background_game(true, Some(ui.ctx()));
        self.draw(ui);
    }
}

pub fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_rgb(23, 29, 42);
    style.visuals.window_fill = Color32::from_rgb(28, 35, 49);
    style.visuals.selection.bg_fill = Color32::from_rgb(38, 113, 112);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(35, 125, 116);
    style.spacing.item_spacing = Vec2::new(8.0, 7.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::ThemePreference::Dark);
}

/// Global render settings address the renderer wired to the active viewport.
/// An unused alternative renderer may reuse local pass IDs without owning them.
fn active_renderer_node(document: &GraphDocument) -> Option<&module_engine_core::Node> {
    let mut outputs = document
        .root
        .nodes
        .iter()
        .filter(|node| node.module == "viewport.output");
    let output = outputs.next()?;
    if outputs.next().is_some() {
        return None;
    }
    let mut sources = document
        .root
        .edges
        .iter()
        .filter(|edge| edge.to_node == output.id && edge.to_port == "frame");
    let source = sources.next()?;
    if sources.next().is_some() {
        return None;
    }
    document
        .root
        .nodes
        .iter()
        .find(|node| node.id == source.from_node)
}

fn active_vulkan_node(document: &GraphDocument) -> Option<&module_engine_core::Node> {
    active_renderer_node(document).filter(|node| node.module == "render.vulkan")
}

fn document_render_targets(document: &GraphDocument) -> (BTreeSet<String>, BTreeSet<String>) {
    active_vulkan_node(document)
        .and_then(|node| node.subgraph.as_deref())
        .map(module_engine_vulkan::render_graph_targets)
        .unwrap_or_default()
}

fn gpu_node_manifest(
    node: &module_engine_core::Node,
) -> Option<module_engine_vulkan::PassManifest> {
    if node.module == "gpu.custom" {
        serde_json::from_value(node.params.get("manifest")?.clone()).ok()
    } else {
        module_engine_vulkan::builtin_manifest(&node.module)
    }
}

fn unused_source_id(sources: &BTreeMap<String, String>, base: &str) -> String {
    if !sources.contains_key(base) {
        return base.into();
    }
    for suffix in 2.. {
        let id = format!("{base}_{suffix}");
        if !sources.contains_key(&id) {
            return id;
        }
    }
    unreachable!()
}

/// A color pin alone is not a sufficient GPU contract: HDR/albedo/LDR share
/// the same palette color but cannot be connected without a conversion pass.
fn graph_connection_error(
    graph: &Graph,
    registry: &ModuleRegistry,
    from_node: &str,
    from_port: &str,
    to_node: &str,
    to_port: &str,
) -> Option<String> {
    let check = || -> Result<(), String> {
        if from_node == to_node {
            return Err("Нельзя соединить узел с самим собой.".into());
        }
        let source = graph
            .nodes
            .iter()
            .find(|node| node.id == from_node)
            .ok_or("Выходной узел удалён.")?;
        let target = graph
            .nodes
            .iter()
            .find(|node| node.id == to_node)
            .ok_or("Входной узел удалён.")?;
        let (_, outputs) =
            node_ports(source, registry).ok_or("Не удалось прочитать выходные пины узла.")?;
        let (inputs, _) =
            node_ports(target, registry).ok_or("Не удалось прочитать входные пины узла.")?;
        let output = outputs
            .iter()
            .find(|port| port.name == from_port)
            .ok_or("Выходной пин не найден.")?;
        let input = inputs
            .iter()
            .find(|port| port.name == to_port)
            .ok_or("Входной пин не найден.")?;
        if output.data_type != input.data_type {
            return Err(format!(
                "Несовместимые пины: {} → {}.",
                output.data_type, input.data_type
            ));
        }
        if let (Some(source), Some(target)) = (gpu_node_manifest(source), gpu_node_manifest(target))
        {
            let output = source
                .outputs
                .iter()
                .find(|spec| spec.name == from_port)
                .ok_or("GPU-выход не объявлен.")?;
            let input = target
                .inputs
                .iter()
                .find(|spec| spec.name == to_port)
                .ok_or("GPU-вход не объявлен.")?;
            if output.format != input.format
                || output.semantic != input.semantic
                || output.extent != input.extent
                || output.samples != input.samples
            {
                return Err(format!(
                    "Несовместимые GPU-ресурсы: {} / {} / {} / {} samples → {} / {} / {} / {} samples.",
                    output.semantic,
                    output.format,
                    output.extent,
                    output.samples,
                    input.semantic,
                    input.format,
                    input.extent,
                    input.samples
                ));
            }
        }
        Ok(())
    };
    check().err()
}

#[cfg(test)]
mod tests {
    /// Native host/Play tests exercise silent fixtures on platforms without
    /// a WAV endpoint. Production templates and audio contract tests retain
    /// their authored requests and explicit unsupported-service diagnostics.
    fn native_test_document() -> GraphDocument {
        let mut document = crate::game_host::demo_document();
        if !module_engine_host_services::WAV_PLAYBACK_SUPPORTED {
            document.game.as_mut().unwrap().audio_cues.clear();
        }
        document
    }

    use super::*;
    use module_engine_core::demo_document;

    #[test]
    fn asset_property_scan_uses_live_ids_and_rejects_a_pending_world_change() {
        let document = demo_document(WorldKind::Objects);
        let mut editor = Editor::new(document, project::source_root().join("project.json"));
        let expected = editor
            .runtime
            .as_ref()
            .unwrap()
            .objects()
            .unwrap()
            .into_iter()
            .map(|object| object.id)
            .collect();
        editor.authored_object_ids = [999_999].into();
        assert_eq!(editor.asset_property_object_ids(), Some(expected));
        editor
            .document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "world")
            .unwrap()
            .module = "world.ecs".into();
        assert_eq!(editor.asset_property_object_ids(), None);
        editor.runtime = None;
        assert_eq!(editor.asset_property_object_ids(), None);
    }

    fn authoring_frame(
        editor: &mut Editor,
        ctx: &egui::Context,
        graph_id: u64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1400., 800.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let graph_rect =
                    egui::Rect::from_min_size(egui::pos2(8., 8.), Vec2::new(1020., 780.));
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt("test-module-graph")
                        .max_rect(graph_rect),
                    |ui| {
                        ui.set_clip_rect(graph_rect);
                        let mut memory = editor.engine_panes.remove(&graph_id).unwrap();
                        memory.rebind_scope(&editor.document);
                        memory.swap(editor);
                        editor.graph_panel(ui);
                        memory.swap(editor);
                        memory.snapshot(&editor.document);
                        editor.engine_panes.insert(graph_id, memory);
                    },
                );
                let library_rect =
                    egui::Rect::from_min_size(egui::pos2(1040., 8.), Vec2::new(350., 780.));
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt("test-library")
                        .max_rect(library_rect),
                    |ui| {
                        ui.set_clip_rect(library_rect);
                        editor.library(ui);
                    },
                );
            },
        )
    }

    fn text_center(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("visible control missing: {label}"))
    }

    fn click_authoring(editor: &mut Editor, ctx: &egui::Context, graph_id: u64, point: egui::Pos2) {
        authoring_frame(
            editor,
            ctx,
            graph_id,
            vec![egui::Event::PointerMoved(point)],
        )
        .drop_without_applying_deltas();
        for pressed in [true, false] {
            authoring_frame(
                editor,
                ctx,
                graph_id,
                vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            )
            .drop_without_applying_deltas();
        }
    }

    fn wait_for_preview(editor: &mut Editor) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
        while editor.preview_builder.is_pending() {
            editor.poll_candidates();
            assert!(
                std::time::Instant::now() < deadline,
                "GPU preview did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn gpu_library_add_wire_render_and_history_use_real_pass_id() {
        let root = crate::project::source_root();
        let mut document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward);
        crate::host::ensure_authoring_configuration(&mut document);
        let mut editor = Editor::new(document, root.join("unused-gpu-authoring.json"));
        editor.playing = false;
        editor.authoring_intent = true;
        editor.graph_path = vec![
            editor
                .document
                .root
                .nodes
                .iter()
                .position(|n| n.module == "render.vulkan")
                .unwrap(),
        ];
        editor.delete_engine_node("aa");
        wait_for_preview(&mut editor);
        assert!(editor.runtime.is_some());
        let without_aa = editor.document.clone();
        let preserved_parameters = editor.document.engine.exposure.parameters.clone();
        let preserved_tonemap = editor.document.engine.exposure.variants["tonemap"].clone();
        let renderer_id = active_vulkan_node(&editor.document).unwrap().id.clone();
        editor.engine_layout = DockLayout::single(EditorType::ModuleGraph);
        let library = editor
            .engine_layout
            .split(1, dock::Axis::Horizontal)
            .unwrap();
        editor
            .engine_layout
            .choose(library, EditorType::ModuleLibrary);
        let graph_id = editor
            .engine_layout
            .panes()
            .into_iter()
            .find(|(_, kind)| *kind == EditorType::ModuleGraph)
            .unwrap()
            .0;
        editor.engine_panes.clear();
        editor.engine_panes.insert(
            graph_id,
            PaneMemory::new(
                panes::PaneSnapshot {
                    graph_scope: vec![renderer_id],
                    graph_zoom: 0.5,
                    ..Default::default()
                },
                &editor.document,
            ),
        );
        editor.graph_path.clear(); // the palette's independent pane is at root
        editor.reconcile_library_graph();
        let ctx = egui::Context::default();
        let output = authoring_frame(&mut editor, &ctx, graph_id, vec![]);
        let add = text_center(&output, "+ FXAA");
        output.drop_without_applying_deltas();
        click_authoring(&mut editor, &ctx, graph_id, add);
        let pass = editor.engine_panes[&graph_id]
            .graph_canvas
            .selected
            .clone()
            .unwrap();
        assert_ne!(pass, "aa");
        let added = editor.document.clone();
        assert!(!added.root.nodes.iter().any(|node| node.id == pass));
        assert!(
            active_vulkan_node(&added)
                .unwrap()
                .subgraph
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.id == pass && node.module == "gpu.fxaa")
        );
        assert_eq!(added.engine.exposure.parameters, preserved_parameters);
        assert_eq!(added.engine.exposure.variants["tonemap"], preserved_tonemap);
        assert_eq!(added.project.variants["antialiasing"], "fxaa");
        for variant in added.engine.exposure.variants["antialiasing"]
            .options
            .values()
        {
            assert!(variant.commands.iter().all(|command| matches!(command,
                module_engine_core::RenderCommand::SetEnabled { pass: target, .. } if target == &pass)));
        }
        // A disconnected draft does not replace the last working renderer.
        wait_for_preview(&mut editor);
        assert!(editor.preview_builder.error.is_some());
        assert!(
            editor
                .runtime
                .as_ref()
                .unwrap()
                .render(64, 48, Camera::default())
                .is_ok()
        );
        editor.undo();
        assert_eq!(editor.document, without_aa);
        editor.redo();
        assert_eq!(editor.document, added);
        authoring_frame(&mut editor, &ctx, graph_id, vec![]).drop_without_applying_deltas();
        for (source, output_pin, target, input_pin) in [
            ("tonemap", "color", pass.as_str(), "color"),
            (pass.as_str(), "color", "encode", "color"),
        ] {
            let layout = &editor.engine_panes[&graph_id].graph_canvas.layout;
            let from = layout.outputs[&(source.into(), output_pin.into())].0;
            let to = layout.inputs[&(target.into(), input_pin.into())].0;
            click_authoring(&mut editor, &ctx, graph_id, from);
            click_authoring(&mut editor, &ctx, graph_id, to);
            assert!(
                active_vulkan_node(&editor.document)
                    .unwrap()
                    .subgraph
                    .as_ref()
                    .unwrap()
                    .edges
                    .contains(&make_edge(source, output_pin, target, input_pin)),
                "pin gesture must create the real connection: {}",
                editor.status
            );
        }
        let wired = editor.document.clone();
        editor.undo();
        assert!(
            !active_vulkan_node(&editor.document)
                .unwrap()
                .subgraph
                .as_ref()
                .unwrap()
                .edges
                .contains(&make_edge(&pass, "color", "encode", "color"))
        );
        editor.redo();
        assert_eq!(editor.document, wired);
        wait_for_preview(&mut editor);
        assert!(editor.runtime_error.is_none(), "{:?}", editor.runtime_error);
        assert!(
            editor.preview_builder.error.is_none(),
            "{:?}",
            editor.preview_builder.error
        );
        let frame = editor
            .runtime
            .as_ref()
            .unwrap()
            .render(96, 64, Camera::default())
            .unwrap();
        assert_eq!(frame.rgba.len(), 96 * 64 * 4);
        assert!(
            frame
                .rgba
                .as_chunks::<4>()
                .0
                .windows(2)
                .any(|pixels| pixels[0] != pixels[1])
        );
        // SSAO needs three resources, not just a color cable. Rebuild it by
        // the same Library/pin gestures and expose its real strength parameter.
        editor.graph_path = vec![
            editor
                .document
                .root
                .nodes
                .iter()
                .position(|node| node.module == "render.vulkan")
                .unwrap(),
        ];
        editor.delete_engine_node("ao");
        editor.graph_path.clear();
        let output = authoring_frame(&mut editor, &ctx, graph_id, vec![]);
        let add = text_center(&output, "+ Screen-space AO");
        output.drop_without_applying_deltas();
        click_authoring(&mut editor, &ctx, graph_id, add);
        let ao = editor.engine_panes[&graph_id]
            .graph_canvas
            .selected
            .clone()
            .unwrap();
        assert_ne!(ao, pass);
        assert_eq!(
            active_vulkan_node(&editor.document)
                .unwrap()
                .subgraph
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .find(|node| node.id == ao)
                .unwrap()
                .module,
            "gpu.ssao"
        );
        // The graph pane is painted before Library. Its pin geometry must be
        // read from the next actual frame, after the newly added node appears.
        authoring_frame(&mut editor, &ctx, graph_id, vec![]).drop_without_applying_deltas();
        for (source, output_pin, target, input_pin) in [
            ("geometry", "color", ao.as_str(), "color"),
            ("geometry", "normals", ao.as_str(), "normals"),
            ("geometry", "depth", ao.as_str(), "depth"),
            (ao.as_str(), "color", "tonemap", "color"),
        ] {
            let layout = &editor.engine_panes[&graph_id].graph_canvas.layout;
            let from = layout.outputs[&(source.into(), output_pin.into())].0;
            let to = layout.inputs[&(target.into(), input_pin.into())].0;
            click_authoring(&mut editor, &ctx, graph_id, from);
            click_authoring(&mut editor, &ctx, graph_id, to);
            assert!(
                active_vulkan_node(&editor.document)
                    .unwrap()
                    .subgraph
                    .as_ref()
                    .unwrap()
                    .edges
                    .contains(&make_edge(source, output_pin, target, input_pin)),
                "{}",
                editor.status
            );
        }
        wait_for_preview(&mut editor);
        assert!(
            editor.preview_builder.error.is_none(),
            "{:?}",
            editor.preview_builder.error
        );
        assert!(
            editor
                .runtime
                .as_ref()
                .unwrap()
                .render(96, 64, Camera::default())
                .is_ok()
        );
        let settings_ctx = egui::Context::default();
        settings_ctx.all_styles_mut(|style| style.animation_time = 0.);
        let settings_frame = |editor: &mut Editor, events| {
            settings_ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1000., 1400.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| editor.engine_settings_panel(ui),
            )
        };
        let output = settings_frame(&mut editor, vec![]);
        let header = text_center(&output, "Expose an existing GPU parameter");
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            settings_frame(
                &mut editor,
                vec![
                    egui::Event::PointerMoved(header),
                    egui::Event::PointerButton {
                        pos: header,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            )
            .drop_without_applying_deltas();
        }
        let output = settings_frame(&mut editor, vec![]);
        let expose = text_center(&output, &format!("Expose {ao}.strength"));
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            settings_frame(
                &mut editor,
                vec![
                    egui::Event::PointerMoved(expose),
                    egui::Event::PointerButton {
                        pos: expose,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            )
            .drop_without_applying_deltas();
        }
        let parameter = &editor.document.engine.exposure.parameters[&format!("{ao}.strength")];
        assert_eq!(parameter.target.pass, ao);
        assert_eq!(parameter.target.name, "strength");
        wait_for_preview(&mut editor);
        assert!(editor.runtime_error.is_none(), "{:?}", editor.runtime_error);
        // Palette insertion also brings the selected node into view when the
        // user had panned far away. Its chosen zoom remains unchanged.
        editor
            .engine_panes
            .get_mut(&graph_id)
            .unwrap()
            .graph_canvas
            .pan = Vec2::new(-4000., 3000.);
        editor
            .engine_panes
            .get_mut(&graph_id)
            .unwrap()
            .graph_canvas
            .zoom = 0.65;
        let output = authoring_frame(&mut editor, &ctx, graph_id, vec![]);
        let add = text_center(&output, "+ FXAA");
        output.drop_without_applying_deltas();
        click_authoring(&mut editor, &ctx, graph_id, add);
        authoring_frame(&mut editor, &ctx, graph_id, vec![]).drop_without_applying_deltas();
        let canvas = &editor.engine_panes[&graph_id].graph_canvas;
        let selected = canvas.selected.as_ref().unwrap();
        assert_ne!(selected, &pass);
        assert_ne!(selected, &ao);
        assert_eq!(canvas.zoom, 0.65);
        assert!(
            canvas
                .layout
                .rect
                .unwrap()
                .contains_rect(canvas.layout.headers[selected])
        );
    }

    #[test]
    fn palette_follows_focused_graph_scope_and_keeps_workspace_targets_separate() {
        let root = crate::project::source_root();
        let mut editor = Editor::new(
            crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward),
            root.join("unused-palette-scopes.json"),
        );
        editor.playing = false;
        editor.authoring_intent = true;
        editor.engine_layout = DockLayout::single(EditorType::ModuleGraph);
        let second = editor
            .engine_layout
            .split(1, dock::Axis::Horizontal)
            .unwrap();
        let library = editor
            .engine_layout
            .split(second, dock::Axis::Vertical)
            .unwrap();
        editor
            .engine_layout
            .choose(library, EditorType::ModuleLibrary);
        let graphs = editor
            .engine_layout
            .panes()
            .into_iter()
            .filter_map(|(id, kind)| (kind == EditorType::ModuleGraph).then_some(id))
            .collect::<Vec<_>>();
        let root_graph = graphs[0];
        let gpu_graph = graphs[1];
        let renderer = active_vulkan_node(&editor.document).unwrap().id.clone();
        editor.engine_panes.clear();
        editor.engine_panes.insert(
            root_graph,
            PaneMemory::new(Default::default(), &editor.document),
        );
        editor.engine_panes.insert(
            gpu_graph,
            PaneMemory::new(
                panes::PaneSnapshot {
                    graph_scope: vec![renderer.clone()],
                    ..Default::default()
                },
                &editor.document,
            ),
        );
        let ctx = egui::Context::default();
        let dock_frame = |editor: &mut Editor, events| {
            let mut panes = vec![];
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1400., 800.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let available = editor.available_editors();
                    panes = editor.draw_window_dock(ui, &available).panes;
                    editor.reconcile_library_graph();
                },
            )
            .drop_without_applying_deltas();
            panes
        };
        let activate = |editor: &mut Editor, id| {
            let panes = dock_frame(editor, vec![]);
            let point = panes
                .iter()
                .find(|pane| pane.id == id)
                .unwrap()
                .rect
                .center();
            for pressed in [true, false] {
                dock_frame(
                    editor,
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
        };
        activate(&mut editor, gpu_graph);
        assert_eq!(editor.library_graph_context().unwrap().0, gpu_graph);
        activate(&mut editor, library);
        assert_eq!(editor.library_graph_context().unwrap().0, gpu_graph);
        assert!(
            editor
                .module_choices(&editor.library_graph_context().unwrap().1)
                .iter()
                .any(|module| module.id == "gpu.ssao")
        );
        // Inserting the root node before the renderer changes its index but not
        // the remembered nested scope's stable IDs.
        editor.document.root.nodes.insert(
            0,
            make_node(
                "unrelated",
                "scalar.value",
                "Scalar",
                [0., 0.],
                serde_json::json!({"value":1.}),
            ),
        );
        let count = active_vulkan_node(&editor.document)
            .unwrap()
            .subgraph
            .as_ref()
            .unwrap()
            .nodes
            .len();
        editor.add_library_node("gpu.ssao");
        assert_eq!(
            active_vulkan_node(&editor.document)
                .unwrap()
                .subgraph
                .as_ref()
                .unwrap()
                .nodes
                .len(),
            count + 1
        );
        assert_eq!(
            editor.engine_panes[&gpu_graph].snapshot.graph_scope,
            vec![renderer]
        );
        assert!(
            editor.engine_panes[&root_graph]
                .graph_canvas
                .selected
                .is_none()
        );
        activate(&mut editor, root_graph);
        assert_eq!(editor.library_graph_context().unwrap().0, root_graph);
        assert!(
            !editor
                .module_choices(&[])
                .iter()
                .any(|module| module.id.starts_with("gpu."))
        );
        editor.add_library_node("scalar.value");
        let selected = editor.engine_panes[&root_graph]
            .graph_canvas
            .selected
            .clone()
            .unwrap();
        assert!(
            editor
                .document
                .root
                .nodes
                .iter()
                .any(|node| node.id == selected && node.module == "scalar.value")
        );
        editor.workspace = WorkspaceKind::Project;
        editor.project_layout = DockLayout::single(EditorType::ModuleGraph);
        editor.project_panes.clear();
        editor.reconcile_library_graph();
        assert_eq!(editor.library_graph_context().unwrap().0, 1);
        assert!(editor.library_graph_context().unwrap().1.is_empty());
        editor.workspace = WorkspaceKind::Engine;
        editor.reconcile_library_graph();
        assert_eq!(editor.library_graph_context().unwrap().0, root_graph);
        // Closing the remembered graph selects a surviving graph, never a
        // Library pane with a coincidentally empty graph_path.
        editor.engine_layout.close(root_graph);
        editor.reconcile_library_graph();
        assert_eq!(editor.library_graph_context().unwrap().0, gpu_graph);
        // Two renderer scopes can have identical local node IDs. Their Custom
        // shader sources are document-wide and must remain independent.
        let renderer_id = active_vulkan_node(&editor.document).unwrap().id.clone();
        let mut alternative = active_vulkan_node(&editor.document).unwrap().clone();
        alternative.id = "alternative".into();
        editor.document.root.nodes.push(alternative);
        editor
            .engine_panes
            .get_mut(&gpu_graph)
            .unwrap()
            .snapshot
            .graph_scope = vec!["alternative".into()];
        editor.add_library_node("gpu.custom");
        let custom = editor.engine_panes[&gpu_graph]
            .graph_canvas
            .selected
            .clone()
            .unwrap();
        let first_source = editor
            .document
            .root
            .nodes
            .iter()
            .find(|node| node.id == "alternative")
            .unwrap()
            .subgraph
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|node| node.id == custom)
            .unwrap()
            .params["source_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let original_source = editor.document.shader_sources[&first_source].clone();
        editor
            .engine_panes
            .get_mut(&gpu_graph)
            .unwrap()
            .snapshot
            .graph_scope = vec![renderer_id];
        editor.add_library_node("gpu.custom");
        let second_source = active_vulkan_node(&editor.document)
            .unwrap()
            .subgraph
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|node| node.id == custom)
            .unwrap()
            .params["source_id"]
            .as_str()
            .unwrap();
        assert_ne!(first_source, second_source);
        assert_eq!(
            editor.document.shader_sources[&first_source],
            original_source
        );
        let before = editor.document.clone();
        editor.distribution = None;
        editor.add_library_node("gpu.ssao");
        assert_eq!(
            editor.document, before,
            "missing Full authorization must block palette edits"
        );
    }

    #[test]
    fn gpu_connections_reject_hdr_to_ldr_without_losing_previous_wire() {
        let root = crate::project::source_root();
        let mut editor = Editor::new(
            crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward),
            root.join("unused-resource-contract.json"),
        );
        editor.authoring_intent = true;
        editor.graph_path = vec![
            editor
                .document
                .root
                .nodes
                .iter()
                .position(|n| n.module == "render.vulkan")
                .unwrap(),
        ];
        let before = editor.document.clone();
        editor.engine_actions(vec![CanvasAction::Connect {
            from_node: "geometry".into(),
            from_port: "color".into(),
            to_node: "aa".into(),
            to_port: "color".into(),
        }]);
        assert_eq!(editor.document, before);
        assert!(editor.status.contains("hdr_linear") && editor.status.contains("ldr_linear"));
        assert!(editor.graph_canvas.message.is_some());
    }

    #[test]
    fn switching_render_path_reconciles_actual_ids_preserves_controls_and_undo() {
        for path in [RenderPath::Forward, RenderPath::Deferred] {
            let root = crate::project::source_root();
            let mut document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward);
            crate::host::ensure_authoring_configuration(&mut document);
            document
                .engine
                .exposure
                .variants
                .get_mut("tonemap")
                .unwrap()
                .label = "Authored tone choices".into();
            document
                .project
                .parameters
                .insert("exposure".into(), serde_json::json!(1.4));
            let mut alternative = active_vulkan_node(&document).unwrap().clone();
            alternative.id = "alternative".into();
            let alternative_graph = alternative.subgraph.clone();
            document.root.nodes.insert(0, alternative);
            document
                .root
                .edges
                .push(make_edge("extract", "scene", "alternative", "scene"));
            let mut editor = Editor::new(document, root.join("unused-switch-path.json"));
            editor.authoring_intent = true;
            editor.graph_path = vec![
                editor
                    .document
                    .root
                    .nodes
                    .iter()
                    .position(|node| node.id == "renderer")
                    .unwrap(),
            ];
            editor.delete_engine_node("aa");
            editor.add_node("gpu.fxaa");
            let replacement = editor.graph_canvas.selected.clone().unwrap();
            editor.engine_actions(vec![
                CanvasAction::Connect {
                    from_node: "tonemap".into(),
                    from_port: "color".into(),
                    to_node: replacement.clone(),
                    to_port: "color".into(),
                },
                CanvasAction::Connect {
                    from_node: replacement.clone(),
                    from_port: "color".into(),
                    to_node: "encode".into(),
                    to_port: "color".into(),
                },
            ]);
            let before = editor.document.clone();
            let tone = before.engine.exposure.variants["tonemap"].clone();
            editor.switch_gpu_path(path);
            let switched = editor.document.clone();
            assert_eq!(switched.engine.exposure.variants["tonemap"], tone);
            assert_eq!(
                switched.project.parameters["exposure"],
                serde_json::json!(1.4)
            );
            assert_eq!(
                switched
                    .root
                    .nodes
                    .iter()
                    .find(|node| node.id == "alternative")
                    .unwrap()
                    .subgraph,
                alternative_graph
            );
            let graph = active_vulkan_node(&switched)
                .unwrap()
                .subgraph
                .as_deref()
                .unwrap();
            assert!(graph.nodes.iter().any(|node| node.module
                == if path == RenderPath::Forward {
                    "gpu.forward"
                } else {
                    "gpu.gbuffer"
                }));
            let validator = module_engine_vulkan::RenderGraphValidator::new(graph).unwrap();
            for group in switched.engine.exposure.variants.values() {
                for variant in group.options.values() {
                    let mut overrides = module_engine_core::RenderOverrides::default();
                    overrides.apply_commands(&variant.commands).unwrap();
                    validator.validate(&overrides).unwrap();
                }
            }
            assert!(
                !switched
                    .engine
                    .exposure
                    .parameters
                    .values()
                    .any(|definition| definition.target.pass == replacement)
            );
            editor.undo();
            assert_eq!(editor.document, before);
            editor.redo();
            assert_eq!(editor.document, switched);
        }
    }

    #[test]
    fn rust_panel_prepares_hidden_game_recovers_bad_source_and_applies_to_game_backend() {
        let root = crate::project::source_root();
        let id = "game-controller";
        let mut document = native_test_document();
        let good = document.custom_sources[id].clone();
        document
            .custom_sources
            .insert(id.into(), "fn update(".into());
        let mut editor = Editor::new(document, root.join("unused-hidden-game-rust.json"));
        editor.playing = false;
        editor.current_source = id.into();
        editor.custom_view = CustomView::Text;
        editor.game_workbench.set_deferred_preparation(true);
        let ctx = egui::Context::default();
        let frame = |editor: &mut Editor, events| {
            editor.poll_candidates();
            editor.tick_simulation(0.);
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1400., 1000.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| editor.custom_panel(ui),
            )
        };
        frame(&mut editor, vec![]).drop_without_applying_deltas();
        assert!(editor.game_workbench.host.is_none());
        assert!(editor.game_workbench.has_pending_preparation());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while editor.game_workbench.has_pending_preparation() {
            frame(&mut editor, vec![]).drop_without_applying_deltas();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(editor.game_workbench.host.is_none());
        let output = frame(&mut editor, vec![]);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().starts_with("Подготовка Game:"))));
        output.drop_without_applying_deltas();
        editor
            .document
            .custom_sources
            .insert(id.into(), good.clone());
        editor.changed(false);
        loop {
            frame(&mut editor, vec![]).drop_without_applying_deltas();
            if editor.game_workbench.host.is_some()
                && !editor.game_workbench.has_pending_preparation()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "correcting the Rust text must retry initial host preparation without opening Game"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let edited = format!("{good}\n// edited and applied from the standalone Rust pane\n");
        editor
            .document
            .custom_sources
            .insert(id.into(), edited.clone());
        editor.changed(false);
        editor.rust_edits.insert(id.into(), 0.);
        editor.last_tick = 2.;
        editor.poll_candidates();
        assert!(
            !editor.scripts.pending(id),
            "a game-only source must not compile into an unused legacy preview backend"
        );
        let output = frame(&mut editor, vec![]);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("предыдущая скомпилированная версия"))));
        let apply = text_center(&output, "Применить Rust");
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            frame(
                &mut editor,
                vec![
                    egui::Event::PointerMoved(apply),
                    egui::Event::PointerButton {
                        pos: apply,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            )
            .drop_without_applying_deltas();
        }
        assert!(
            editor
                .preparation_activities()
                .iter()
                .any(|activity| activity.title == format!("Rust Custom {id}")
                    && activity.state == crate::activity::ActivityState::Running)
        );
        loop {
            frame(&mut editor, vec![]).drop_without_applying_deltas();
            if editor.game_workbench.host.as_ref().unwrap().slots[id]
                .lock()
                .unwrap()
                .source()
                == edited
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Rust Apply must update the Game instances"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!editor.scripts.has_active(id));
        let output = frame(&mut editor, vec![]);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("Game: текущий исходник скомпилирован"))));
        output.drop_without_applying_deltas();
        editor.game_execution_error = Some("Owned previous game execution error".into());
        editor.runtime_error = editor.game_execution_error.clone();
        editor.game_workbench.input.restart = true;
        editor.tick_simulation(0.);
        assert!(
            editor.runtime_error.is_none(),
            "explicit restart while paused must recover its previous game execution error"
        );
        editor.runtime_error = Some("Unrelated GPU diagnostic".into());
        editor.game_workbench.input.restart = true;
        editor.tick_simulation(0.);
        assert_eq!(
            editor.runtime_error.as_deref(),
            Some("Unrelated GPU diagnostic")
        );
    }

    #[test]
    fn deleting_aa_in_inspector_repairs_settings_connections_and_undo_redo_together() {
        let root = crate::project::source_root();
        let mut document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward);
        crate::host::ensure_authoring_configuration(&mut document);
        // Another renderer can reuse local `aa` without keeping the active
        // renderer's deleted settings alive.
        let mut alternative = document
            .root
            .nodes
            .iter()
            .find(|node| node.id == "renderer")
            .unwrap()
            .clone();
        alternative.id = "unused-renderer".into();
        document.root.nodes.push(alternative);
        document
            .root
            .edges
            .push(make_edge("extract", "scene", "unused-renderer", "scene"));
        module_engine_core::set_project_variant(&mut document, "antialiasing", "off").unwrap();
        module_engine_core::set_project_parameter(
            &mut document,
            "exposure",
            serde_json::json!(1.4),
        )
        .unwrap();
        document
            .project
            .rules
            .push(module_engine_core::VariantRule {
                group: "antialiasing".into(),
                variant: "fxaa".into(),
                condition: module_engine_core::RuleCondition::Time {
                    op: module_engine_core::Comparison::Greater,
                    value: 1.,
                },
            });
        let original = document.clone();
        let mut editor = Editor::new(document, root.join("unused-aa-delete.json"));
        editor.authoring_intent = true;
        editor.graph_path = vec![
            editor
                .document
                .root
                .nodes
                .iter()
                .position(|node| node.module == "render.vulkan")
                .unwrap(),
        ];
        editor.graph_canvas.selected = Some("aa".into());
        let ctx = egui::Context::default();
        let draw = |editor: &mut Editor, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(900., 600.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| editor.node_inspector(ui),
            )
        };
        let output = draw(&mut editor, vec![]);
        let click = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Удалить" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("inspector delete button is visible");
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            draw(
                &mut editor,
                vec![
                    egui::Event::PointerMoved(click),
                    egui::Event::PointerButton {
                        pos: click,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            )
            .drop_without_applying_deltas();
        }
        let deleted = editor.document.clone();
        assert!(!editor.graph().nodes.iter().any(|node| node.id == "aa"));
        assert!(
            editor
                .graph()
                .edges
                .contains(&make_edge("tonemap", "color", "encode", "color"))
        );
        assert!(
            !deleted
                .engine
                .exposure
                .variants
                .contains_key("antialiasing")
        );
        assert!(!deleted.project.variants.contains_key("antialiasing"));
        assert!(deleted.project.rules.is_empty());
        assert_eq!(
            deleted.project.parameters["exposure"],
            serde_json::json!(1.4)
        );
        assert_eq!(
            deleted.engine.exposure.parameters,
            original.engine.exposure.parameters
        );
        assert!(
            !validate_graph(&deleted, &editor.registry)
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        let commands = module_engine_core::project_commands(&deleted, 0.).unwrap();
        let render = commands
            .into_iter()
            .filter_map(|command| match command {
                ControlCommand::Render(command) => Some(command),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut overrides = module_engine_core::RenderOverrides::default();
        overrides.apply_commands(&render).unwrap();
        module_engine_vulkan::validate_overrides_at(editor.graph(), &overrides, &root).unwrap();
        editor.undo();
        assert_eq!(editor.document, original);
        editor.redo();
        assert_eq!(editor.document, deleted);
    }

    #[test]
    fn deleting_scoped_names_preserves_other_targets_and_renderer_deletion_cleans_all() {
        let root = crate::project::source_root();
        let mut document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward);
        crate::host::ensure_authoring_configuration(&mut document);
        document.root.nodes.push(make_node(
            "aa",
            "scalar.value",
            "Unrelated scope",
            [0., 0.],
            serde_json::json!({"value":1.}),
        ));
        let mut editor = Editor::new(document, root.join("unused-scope-delete.json"));
        editor.authoring_intent = true;
        let exposure = editor.document.engine.exposure.clone();
        editor.delete_engine_node("aa");
        assert_eq!(editor.document.engine.exposure, exposure);
        assert!(document_render_targets(&editor.document).0.contains("aa"));
        editor.delete_engine_node("renderer");
        assert!(editor.document.engine.exposure.parameters.is_empty());
        assert!(editor.document.engine.exposure.variants.is_empty());
        assert!(document_render_targets(&editor.document).0.is_empty());
    }

    #[test]
    fn aa_checkbox_reports_background_work_and_preserves_the_running_copy() {
        use std::{
            sync::Arc,
            time::{Duration, Instant},
        };
        let root = crate::project::source_root();
        let mut editor = Editor::new(native_test_document(), root.join("unused-aa-status.json"));
        editor.playing = false;
        editor.game_workbench.scene = "arena".into();
        let deadline = Instant::now() + Duration::from_secs(45);
        while editor.preview_builder.is_pending() {
            editor.poll_candidates();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let (asset_root, compile, kind) = editor.game_context();
        editor
            .game_workbench
            .ensure(&editor.document, &asset_root, compile.as_deref(), kind)
            .unwrap();
        editor
            .game_workbench
            .ensure_preview(&editor.document, &asset_root, compile.as_deref(), kind)
            .unwrap();
        editor
            .game_workbench
            .host
            .as_mut()
            .unwrap()
            .session
            .apply_commands(&[module_engine_core::WorldCommand::SetPosition {
                id: 1,
                position: [3., 0., 4.],
            }])
            .unwrap();
        let retained_state = editor
            .game_workbench
            .host
            .as_ref()
            .unwrap()
            .session
            .state()
            .clone();
        let retained_slot = editor
            .game_workbench
            .host
            .as_ref()
            .unwrap()
            .slots
            .values()
            .next()
            .unwrap()
            .clone();
        editor.game_workbench.set_deferred_preparation(true);
        editor.graph_path = vec![
            editor
                .document
                .root
                .nodes
                .iter()
                .position(|node| node.module == "render.vulkan")
                .unwrap(),
        ];
        let ctx = egui::Context::default();
        let draw = |editor: &mut Editor, events| {
            editor.poll_candidates();
            editor.tick_simulation(1. / 60.);
            editor
                .game_workbench
                .ensure_preview(&editor.document, &asset_root, compile.as_deref(), kind)
                .unwrap();
            // Exercise the old device while a replacement compiles, as Game
            // does in native UI. Preparation must not hold its render lock.
            editor
                .game_workbench
                .host
                .as_mut()
                .unwrap()
                .render(160, 90)
                .unwrap();
            let node = editor
                .graph()
                .nodes
                .iter()
                .find(|node| node.module == "gpu.fxaa")
                .unwrap()
                .clone();
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(900., 600.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    editor.gpu_pass_controls(ui, &node);
                    crate::activity::draw_activity(ui, &editor.preparation_activities());
                },
            )
        };
        let output = draw(&mut editor, vec![]);
        let click = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Проход включён" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("AA checkbox is visible");
        output.drop_without_applying_deltas();
        draw(
            &mut editor,
            vec![
                egui::Event::PointerMoved(click),
                egui::Event::PointerButton {
                    pos: click,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        )
        .drop_without_applying_deltas();
        let output = draw(
            &mut editor,
            vec![egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(
            editor
                .graph()
                .nodes
                .iter()
                .find(|node| node.module == "gpu.fxaa")
                .unwrap()
                .params["enabled"],
            false
        );
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("В работе:"))));
        output.drop_without_applying_deltas();
        let mut responsive_frames = 0;
        loop {
            draw(&mut editor, vec![]).drop_without_applying_deltas();
            responsive_frames += 1;
            if !editor.preview_builder.is_pending()
                && !editor.game_workbench.has_pending_preparation()
                && !editor
                    .scripts
                    .activity_snapshots()
                    .iter()
                    .any(|activity| activity.state == crate::activity::ActivityState::Running)
            {
                break;
            }
            assert!(Instant::now() < deadline, "AA preparation did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(responsive_frames > 0);
        assert!(
            editor
                .preparation_activities()
                .iter()
                .all(|activity| activity.state != crate::activity::ActivityState::Running)
        );
        assert_eq!(
            editor.game_workbench.host.as_ref().unwrap().session.state(),
            &retained_state
        );
        assert!(Arc::ptr_eq(
            &retained_slot,
            editor
                .game_workbench
                .host
                .as_ref()
                .unwrap()
                .slots
                .values()
                .next()
                .unwrap()
        ));
        assert!(editor.runtime_error.is_none(), "{:?}", editor.runtime_error);
    }
    #[test]
    fn graph_node_drag_preserves_game_and_scene_hosts() {
        use std::{sync::Arc, time::Instant};
        let count = std::env::var("MODULE_ENGINE_GRAPH_DRAG_FRAMES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(12)
            .clamp(6, 240);
        let mut reports = vec![];
        for nested in [false, true] {
            let root = crate::project::source_root();
            let mut editor =
                Editor::new(native_test_document(), root.join("unused-graph-drag.json"));
            editor.playing = false;
            editor.authoring_intent = true;
            editor.game_workbench.scene = "arena".into();
            let (asset_root, compile, kind) = editor.game_context();
            editor
                .game_workbench
                .ensure(&editor.document, &asset_root, compile.as_deref(), kind)
                .unwrap();
            editor
                .game_workbench
                .ensure_preview(&editor.document, &asset_root, compile.as_deref(), kind)
                .unwrap();
            let game_slot = editor
                .game_workbench
                .host
                .as_ref()
                .unwrap()
                .slots
                .values()
                .next()
                .unwrap()
                .clone();
            let preview_slot = editor
                .game_workbench
                .preview_host
                .as_ref()
                .unwrap()
                .slots
                .values()
                .next()
                .unwrap()
                .clone();
            if nested {
                editor.graph_path.push(
                    editor
                        .document
                        .root
                        .nodes
                        .iter()
                        .position(|node| node.module == "render.vulkan")
                        .unwrap(),
                );
            }
            editor.graph_canvas.zoom = 0.55;
            let id = editor.graph().nodes[0].id.clone();
            let initial_position = editor.graph().nodes[0].position;
            let ctx = egui::Context::default();
            let frame = |editor: &mut Editor, events| {
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            Vec2::new(1200., 720.),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| editor.graph_panel(ui),
                );
                output.drop_without_applying_deltas();
            };
            frame(&mut editor, vec![]);
            let start = editor.graph_canvas.layout.headers[&id].center();
            frame(&mut editor, vec![egui::Event::PointerMoved(start)]);
            frame(
                &mut editor,
                vec![egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
            );
            let mut timings = vec![];
            let mut moved_frames = 0;
            for index in 1..=count {
                let began = Instant::now();
                let pointer = start + Vec2::new(index as f32 * 0.8, index as f32 * 0.25);
                let previous_position = editor.graph().nodes[0].position;
                frame(&mut editor, vec![egui::Event::PointerMoved(pointer)]);
                moved_frames += usize::from(editor.graph().nodes[0].position != previous_position);
                editor.tick_simulation(1. / 60.);
                editor
                    .game_workbench
                    .ensure_preview(&editor.document, &asset_root, compile.as_deref(), kind)
                    .unwrap();
                timings.push(began.elapsed().as_secs_f64() * 1000.);
            }
            frame(
                &mut editor,
                vec![egui::Event::PointerButton {
                    pos: start + Vec2::new(count as f32 * 0.8, count as f32 * 0.25),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            );
            assert_ne!(editor.graph().nodes[0].position, initial_position);
            assert!(editor.runtime_error.is_none(), "{:?}", editor.runtime_error);
            let game_retained = Arc::ptr_eq(
                &game_slot,
                editor
                    .game_workbench
                    .host
                    .as_ref()
                    .unwrap()
                    .slots
                    .values()
                    .next()
                    .unwrap(),
            );
            let preview_retained = Arc::ptr_eq(
                &preview_slot,
                editor
                    .game_workbench
                    .preview_host
                    .as_ref()
                    .unwrap()
                    .slots
                    .values()
                    .next()
                    .unwrap(),
            );
            timings.sort_by(f64::total_cmp);
            reports.push(serde_json::json!({
                "scope":if nested {"gpu"} else {"root"}, "frames":count,
                "moved_frames":moved_frames,
                "median_ms":timings[count / 2], "p95_ms":timings[(count * 95 / 100).min(count - 1)],
                "max_ms":timings[count - 1], "game_host_retained":game_retained,
                "scene_host_retained":preview_retained,
                "timing_scope":"Headless egui graph draw/model edit plus stopped Game tick and Scene ensure; excludes native presentation and viewport rasterization"
            }));
            assert!(
                moved_frames > 0,
                "the benchmark must perform actual model edits"
            );
        }
        eprintln!("{}", serde_json::to_string_pretty(&reports).unwrap());
        if let Ok(path) = std::env::var("MODULE_ENGINE_GRAPH_DRAG_REPORT") {
            std::fs::write(path, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
        }
        assert!(
            reports
                .iter()
                .all(|report| report["game_host_retained"] == true
                    && report["scene_host_retained"] == true)
        );
    }
    #[test]
    fn invalid_custom_keeps_text_and_last_good_graph() {
        let mut editor = Editor::new(
            demo_document(WorldKind::Objects),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json"),
        );
        let before = editor.program.as_ref().unwrap().source().to_owned();
        editor
            .document
            .custom_sources
            .insert("speed".into(), "fn speed(".into());
        editor.parse_custom();
        assert!(editor.source_error.is_some());
        assert_eq!(editor.program.as_ref().unwrap().source(), before);
        assert_eq!(editor.document.custom_sources["speed"], "fn speed(");
    }
    #[test]
    fn world_switch_preserves_renderer_and_custom() {
        let mut editor = Editor::new(
            demo_document(WorldKind::Objects),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json"),
        );
        let renderer = editor
            .document
            .root
            .nodes
            .iter()
            .find(|n| n.module == "render.software3d")
            .unwrap()
            .clone();
        let source = editor.document.custom_sources.clone();
        editor.switch_world(WorldKind::Ecs);
        assert_eq!(
            editor.runtime.as_ref().unwrap().world_kind(),
            WorldKind::Ecs
        );
        assert_eq!(
            *editor
                .document
                .root
                .nodes
                .iter()
                .find(|n| n.module == "render.software3d")
                .unwrap(),
            renderer
        );
        assert_eq!(editor.document.custom_sources, source);
    }
    #[test]
    fn applying_invalid_json_does_not_replace_model_or_draft() {
        let mut editor = Editor::new(
            demo_document(WorldKind::Objects),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json"),
        );
        let before = editor.document.clone();
        editor.json_draft = "{ incomplete".into();
        editor.json_dirty = true;
        assert!(!editor.apply_json());
        assert_eq!(editor.document, before);
        assert_eq!(editor.json_draft, "{ incomplete");
        let mut typo = serde_json::to_value(&before).unwrap();
        typo["root"]["nodes"][0]["param"] = serde_json::json!({"typo":true});
        editor.json_draft = serde_json::to_string(&typo).unwrap();
        assert!(!editor.apply_json());
        assert_eq!(editor.document, before);
    }

    #[test]
    fn file_draft_is_resolved_before_other_views_edit_the_document() {
        let mut editor = Editor::new(
            demo_document(WorldKind::Objects),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json"),
        );
        let mut draft = editor.document.clone();
        draft.name = "From file".into();
        editor.json_draft = serde_json::to_string(&draft).unwrap();
        editor.json_dirty = true;
        let count = editor.document.root.nodes.len();
        editor.add_node("scalar.value");
        assert_eq!(editor.document.root.nodes.len(), count);
        editor.switch_world(WorldKind::Ecs);
        assert_eq!(
            editor.runtime.as_ref().unwrap().world_kind(),
            WorldKind::Objects
        );
        editor.tab = Tab::Graph;
        editor.finish_tab_change(Tab::File);
        assert_eq!(editor.document.name, "From file");
        assert!(!editor.json_dirty);
        editor.switch_world(WorldKind::Ecs);
        assert!(editor.json_draft.contains("world.ecs"));
        assert!(editor.apply_json());
        assert_eq!(
            editor.runtime.as_ref().unwrap().world_kind(),
            WorldKind::Ecs
        );

        editor.json_draft = "invalid JSON".into();
        editor.json_dirty = true;
        editor.tab = Tab::Custom;
        editor.finish_tab_change(Tab::File);
        assert!(editor.tab == Tab::File);
        assert_eq!(editor.json_draft, "invalid JSON");
        editor.discard_json_draft();
        assert!(!editor.json_dirty);
        assert_eq!(
            serde_json::from_str::<GraphDocument>(&editor.json_draft).unwrap(),
            editor.document
        );
    }

    #[test]
    fn custom_layout_and_literal_edits_survive_document_roundtrip() {
        let mut editor = Editor::new(
            demo_document(WorldKind::Objects),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json"),
        );
        let id = editor
            .program
            .as_ref()
            .unwrap()
            .nodes()
            .iter()
            .find(|n| matches!(n.kind, CodeNodeKind::Number { .. }))
            .unwrap()
            .id
            .clone();
        editor.code_actions(vec![CanvasAction::Move(id.clone(), [241.0, 362.0])]);
        editor.code_canvas.selected = Some(id.clone());
        editor
            .program
            .as_mut()
            .unwrap()
            .set_number(&id, 0.45)
            .unwrap();
        editor.sync_program_source(false);
        assert_eq!(editor.code_canvas.selected.as_deref(), Some(id.as_str()));
        assert!(editor.document.custom_sources["speed"].contains("0.45"));
        assert!(Runtime::from_document(&editor.document, &editor.registry).is_ok());
        let reloaded = Editor::new(
            serde_json::from_str(&serde_json::to_string(&editor.document).unwrap()).unwrap(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json"),
        );
        assert_eq!(reloaded.code_positions[&id], [241.0, 362.0]);
        assert_eq!(
            reloaded.program.unwrap().source(),
            editor.document.custom_sources["speed"]
        );
        editor.code_canvas.pending = Some((id, "value".into()));
        editor.document.custom_sources.insert(
            "speed".into(),
            "fn speed(time:f64, base:f64)->f64 { time + base }".into(),
        );
        editor.parse_custom();
        assert!(editor.code_canvas.pending.is_none());
    }

    #[test]
    fn editing_two_sources_then_switching_tabs_compiles_each_correct_source() {
        let mut editor = Editor::new(
            demo_document(WorldKind::Objects),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json"),
        );
        for id in ["a", "b"] {
            editor.document.root.nodes.push(make_node(
                id,
                "custom.rust.update",
                id,
                [0., 0.],
                serde_json::json!({"source_id":id}),
            ));
            editor.document.custom_sources.insert(id.into(),format!("fn update(ctx: &mut ScriptContext) {{ /* {id} edited */ ctx.enable_render_pass(\"ao\", true); }}"));
            editor.rust_edits.insert(id.into(), 0.);
        }
        editor.current_source = "speed".into();
        editor.last_tick = 1.;
        editor.poll_candidates();
        assert!(editor.scripts.pending("a") && editor.scripts.pending("b"));
        assert!(!editor.scripts.pending("speed"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
        while editor.scripts.pending("a") || editor.scripts.pending("b") {
            assert!(
                std::time::Instant::now() < deadline,
                "Compilation did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
            editor.scripts.poll(&editor.document);
        }
        for id in ["a", "b"] {
            assert_eq!(
                editor.scripts.active_source(id),
                Some(editor.document.custom_sources[id].as_str()),
                "{:?}",
                editor.scripts.errors
            );
        }
    }

    #[test]
    fn compiled_scalar_timeout_pauses_simulation_and_is_not_repeated() {
        let mut editor = Editor::new(
            demo_document(WorldKind::Objects),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json"),
        );
        let source = "fn speed(_time:f64,_base:f64)->f64 { loop {} }";
        let root = crate::project::source_root();
        let compiled =
            module_engine_script::ScriptCompiler::new(root.clone(), root.join(".work/scripts"))
                .compile_kind(source, ScriptKind::Speed)
                .unwrap();
        editor
            .document
            .custom_sources
            .insert("speed".into(), source.into());
        editor.scripts.install_for_test("speed", compiled);
        editor.tick_simulation(1. / 60.);
        assert!(!editor.playing);
        assert!(
            editor
                .runtime_error
                .as_deref()
                .unwrap()
                .contains("exceeded 150 ms")
        );
        editor.runtime_error = Some("paused latch".into());
        let before = editor.runtime.as_ref().unwrap().snapshot();
        editor.tick_simulation(1. / 60.);
        assert_eq!(editor.runtime_error.as_deref(), Some("paused latch"));
        assert_eq!(editor.runtime.as_ref().unwrap().snapshot(), before);
    }
}
