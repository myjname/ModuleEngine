//! Per-area transient views and personal persisted navigation. Simulation is shared.
use super::dock::{DockLayout, EditorType, WorkspaceKind};
use super::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub type ModulePanelFactory = fn(&mut egui::Ui, &GraphDocument, &ModuleRegistry);
pub struct ModulePanelRegistration {
    pub id: u32,
    pub module: String,
    pub label: String,
    pub engine_only: bool,
    pub factory: ModulePanelFactory,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PaneSnapshot {
    pub graph_scope: Vec<String>,
    pub graph_pan: [f32; 2],
    pub graph_zoom: f32,
    pub graph_selected: Option<String>,
    pub code_pan: [f32; 2],
    pub code_zoom: f32,
    pub code_selected: Option<String>,
    pub source: String,
    pub shader: String,
    pub custom_view: CustomView,
    pub camera: Camera,
    pub object: Option<u64>,
    pub object_pinned: bool,
}
impl Default for PaneSnapshot {
    fn default() -> Self {
        Self {
            graph_scope: vec![],
            graph_pan: [28.0, 35.0],
            graph_zoom: 0.5,
            graph_selected: None,
            code_pan: [28.0, 35.0],
            code_zoom: 0.7,
            code_selected: None,
            source: "controller".into(),
            shader: String::new(),
            custom_view: CustomView::Both,
            camera: Camera::default(),
            object: None,
            object_pinned: false,
        }
    }
}
pub struct PaneMemory {
    pub scene_view: crate::scene_view_ui::SceneViewportState,
    pub snapshot: PaneSnapshot,
    pub graph_path: Vec<usize>,
    pub graph_canvas: CanvasState,
    pub code_canvas: CanvasState,
    pub current_source: String,
    pub current_shader: String,
    pub custom_view: CustomView,
    pub program: Option<CustomProgram>,
    pub command_program: Option<CommandProgram>,
    pub source_error: Option<String>,
    pub code_positions: BTreeMap<String, [f32; 2]>,
    pub camera: Camera,
    pub texture: Option<egui::TextureHandle>,
    pub last_render: f64,
    pub stats: String,
    pub param_draft: String,
    pub inspector_id: Option<String>,
    pub parsed_source: Option<String>,
    pub parsed_kind: Option<bool>,
    pub module_search: String,
}
impl PaneMemory {
    pub fn new(snapshot: PaneSnapshot, document: &GraphDocument) -> Self {
        let mut graph_canvas = CanvasState::new("module-graph");
        graph_canvas.pan = Vec2::from(snapshot.graph_pan);
        graph_canvas.zoom = snapshot.graph_zoom.clamp(0.45, 1.65);
        graph_canvas.selected = snapshot.graph_selected.clone();
        let mut code_canvas = CanvasState::new("code-graph");
        code_canvas.pan = Vec2::from(snapshot.code_pan);
        code_canvas.zoom = snapshot.code_zoom.clamp(0.45, 1.65);
        code_canvas.selected = snapshot.code_selected.clone();
        let mut graph = &document.root;
        let mut graph_path = vec![];
        for id in &snapshot.graph_scope {
            let Some(index) = graph
                .nodes
                .iter()
                .position(|n| &n.id == id && n.subgraph.is_some())
            else {
                break;
            };
            graph_path.push(index);
            graph = graph.nodes[index].subgraph.as_deref().unwrap();
        }
        let scene_view = crate::scene_view_ui::SceneViewportState::with_camera(snapshot.camera);
        Self {
            scene_view,
            graph_path,
            graph_canvas,
            code_canvas,
            current_source: snapshot.source.clone(),
            current_shader: snapshot.shader.clone(),
            custom_view: snapshot.custom_view,
            camera: snapshot.camera,
            snapshot,
            program: None,
            command_program: None,
            source_error: None,
            code_positions: BTreeMap::new(),
            texture: None,
            last_render: -1.,
            stats: String::new(),
            param_draft: String::new(),
            inspector_id: None,
            parsed_source: None,
            parsed_kind: None,
            module_search: String::new(),
        }
    }
    pub fn rebind_scope(&mut self, document: &GraphDocument) {
        let mut graph = &document.root;
        let mut path = vec![];
        for id in &self.snapshot.graph_scope {
            let Some(index) = graph
                .nodes
                .iter()
                .position(|n| &n.id == id && n.subgraph.is_some())
            else {
                break;
            };
            path.push(index);
            graph = graph.nodes[index].subgraph.as_deref().unwrap();
        }
        self.graph_path = path;
    }
    pub fn swap(&mut self, editor: &mut Editor) {
        macro_rules! exchange {($($field:ident),*)=>{$(std::mem::swap(&mut self.$field,&mut editor.$field);)*};}
        exchange!(
            graph_path,
            graph_canvas,
            code_canvas,
            current_source,
            current_shader,
            custom_view,
            program,
            command_program,
            source_error,
            code_positions,
            camera,
            texture,
            last_render,
            stats,
            param_draft,
            inspector_id,
            module_search
        );
    }
    pub fn snapshot(&mut self, document: &GraphDocument) -> PaneSnapshot {
        let mut scope = vec![];
        let mut graph = &document.root;
        for index in &self.graph_path {
            let Some(node) = graph.nodes.get(*index) else {
                break;
            };
            let Some(inner) = node.subgraph.as_deref() else {
                break;
            };
            scope.push(node.id.clone());
            graph = inner;
        }
        self.snapshot.graph_scope = scope;
        self.snapshot.graph_pan = [self.graph_canvas.pan.x, self.graph_canvas.pan.y];
        self.snapshot.graph_zoom = self.graph_canvas.zoom;
        self.snapshot.graph_selected = self.graph_canvas.selected.clone();
        self.snapshot.code_pan = [self.code_canvas.pan.x, self.code_canvas.pan.y];
        self.snapshot.code_zoom = self.code_canvas.zoom;
        self.snapshot.code_selected = self.code_canvas.selected.clone();
        self.snapshot.source = self.current_source.clone();
        self.snapshot.shader = self.current_shader.clone();
        self.snapshot.custom_view = self.custom_view;
        self.snapshot.camera = self.camera;
        self.snapshot.clone()
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersonalLayout {
    version: u32,
    layout: DockLayout,
    panes: BTreeMap<u64, PaneSnapshot>,
}
fn migrate_project_layout(layout: &mut DockLayout, old_version: bool) {
    if old_version && layout.panes().len() == 1 && layout.panes()[0].1 == EditorType::Game {
        *layout = DockLayout::default_for(WorkspaceKind::Project);
        return;
    }
    fn walk(area: &mut super::dock::Area) {
        match area {
            super::dock::Area::Pane { editor, .. } => {
                *editor = match *editor {
                    EditorType::Viewport => EditorType::Scene,
                    EditorType::ObjectTree => EditorType::Hierarchy,
                    EditorType::ObjectInspector => EditorType::Inspector,
                    other => other,
                }
            }
            super::dock::Area::Split { first, second, .. } => {
                walk(first);
                walk(second);
            }
        }
    }
    walk(&mut layout.root);
}
pub fn load(
    path: &Path,
    workspace: WorkspaceKind,
    document: &GraphDocument,
) -> Result<(DockLayout, BTreeMap<u64, PaneMemory>), String> {
    if !path.is_file() {
        let layout = DockLayout::default_for(workspace);
        return Ok((layout, BTreeMap::new()));
    }
    let mut saved: PersonalLayout =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if saved.version != 1 && saved.version != 2 {
        return Err("Unsupported personal layout version".into());
    }
    saved.layout.validate()?;
    if workspace == WorkspaceKind::Project {
        migrate_project_layout(&mut saved.layout, saved.version == 1);
    }
    for pane in saved.panes.values() {
        if !pane
            .graph_pan
            .iter()
            .chain(&pane.code_pan)
            .chain(&pane.camera.target)
            .all(|v| v.is_finite())
            || ![
                pane.graph_zoom,
                pane.code_zoom,
                pane.camera.yaw,
                pane.camera.pitch,
                pane.camera.distance,
            ]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err("Personal pane navigation must be finite".into());
        }
    }
    let active: BTreeMap<_, _> = saved.layout.panes().into_iter().collect();
    let panes = saved
        .panes
        .into_iter()
        .filter(|(id, _)| active.contains_key(id))
        .map(|(id, s)| (id, PaneMemory::new(s, document)))
        .collect();
    Ok((saved.layout, panes))
}
pub fn save(
    path: &Path,
    layout: &DockLayout,
    panes: &mut BTreeMap<u64, PaneMemory>,
    document: &GraphDocument,
) -> Result<(), String> {
    layout.validate()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let active: BTreeMap<_, _> = layout.panes().into_iter().collect();
    let panes = panes
        .iter_mut()
        .filter(|(id, _)| active.contains_key(id))
        .map(|(id, p)| (*id, p.snapshot(document)))
        .collect();
    let bytes = serde_json::to_vec_pretty(&PersonalLayout {
        version: 2,
        layout: layout.clone(),
        panes,
    })
    .map_err(|e| e.to_string())?;
    let temporary = path.with_extension("pending");
    std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

impl Editor {
    pub fn register_module_panel(
        &mut self,
        id: u32,
        module: &str,
        label: &str,
        engine_only: bool,
        factory: ModulePanelFactory,
    ) -> Result<(), String> {
        if self.panel_registry.iter().any(|p| p.id == id)
            || self.registry.descriptor(module).is_none()
        {
            return Err("Panel requires a unique ID and an installed owning module".into());
        }
        self.panel_registry.push(ModulePanelRegistration {
            id,
            module: module.into(),
            label: label.into(),
            engine_only,
            factory,
        });
        Ok(())
    }
    pub(super) fn can_author_engine(&self) -> bool {
        self.distribution
            .as_ref()
            .is_some_and(|p| p.can_author_engine())
    }
    pub(super) fn can_edit_source(&self, id: &str) -> bool {
        self.can_author_engine()
            || self
                .distribution
                .as_ref()
                .is_some_and(|p| p.can_edit_custom_source(id, &self.document))
    }
    pub(super) fn validate_candidate(
        &self,
        candidate: &GraphDocument,
        engine: bool,
    ) -> Result<(), String> {
        if engine && !self.can_author_engine() {
            return Err("Engine editor unavailable: Full profile and complete matching source manifest are required".into());
        }
        if let Some(distribution) = &self.distribution {
            distribution.validate_document(candidate)?;
            if !engine {
                distribution.validate_project_document(candidate, &self.policy_baseline)?;
            }
        } else if !engine {
            module_engine_core::validate_project_edit(
                candidate,
                &self.policy_baseline,
                &self.registry,
            )?;
        }
        Ok(())
    }
    pub(super) fn available_editors(&self) -> Vec<EditorDescriptor> {
        self.available_editors_for(self.workspace)
    }
    fn available_editors_for(&self, workspace: WorkspaceKind) -> Vec<EditorDescriptor> {
        let engine = self.can_author_engine();
        let mut available = EditorDescriptor::builtins(engine);
        if workspace == WorkspaceKind::Project {
            available.retain(|descriptor| {
                !matches!(
                    descriptor.editor,
                    EditorType::Viewport | EditorType::ObjectTree | EditorType::ObjectInspector
                )
            });
        }
        let active = module_engine_core::effective_modules(&self.document);
        available.extend(
            self.panel_registry
                .iter()
                .filter(|p| active.contains(&p.module) && (engine || !p.engine_only))
                .map(|p| EditorDescriptor {
                    editor: EditorType::Extension(p.id),
                    label: p.label.clone(),
                    requires_engine: p.engine_only,
                }),
        );
        available
    }
    fn personal_path(&self, workspace: WorkspaceKind) -> PathBuf {
        self.distribution
            .as_ref()
            .map(|p| p.personal_layout_path(workspace.key()))
            .unwrap_or_else(|| {
                PathBuf::from(&self.path)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(".user/unavailable")
                    .join(format!("{}.json", workspace.key()))
            })
    }
    pub(super) fn load_personal_layouts(&mut self) {
        for workspace in [WorkspaceKind::Engine, WorkspaceKind::Project] {
            let available = self.available_editors_for(workspace);
            match load(&self.personal_path(workspace), workspace, &self.document) {
                Ok((mut layout, views)) => {
                    layout.restrict(&available);
                    match workspace {
                        WorkspaceKind::Engine => {
                            self.engine_layout = layout;
                            self.engine_panes = views;
                        }
                        WorkspaceKind::Project => {
                            self.project_layout = layout;
                            self.project_panes = views;
                        }
                    }
                }
                Err(error) => self.status = format!("Personal layout ignored: {error}"),
            }
        }
    }
    pub(super) fn save_personal_layouts(&mut self) {
        let engine = self.personal_path(WorkspaceKind::Engine);
        let project = self.personal_path(WorkspaceKind::Project);
        let result = save(
            &engine,
            &self.engine_layout,
            &mut self.engine_panes,
            &self.document,
        )
        .and_then(|_| {
            save(
                &project,
                &self.project_layout,
                &mut self.project_panes,
                &self.document,
            )
        });
        match result {
            Ok(()) => self.personal_dirty = false,
            Err(error) => self.status = format!("Personal layout: {error}"),
        }
    }
    pub(super) fn reveal_editor(&mut self, kind: EditorType) {
        if kind.requires_engine() && !self.can_author_engine() {
            self.status = "Engine editor unavailable in this distribution".into();
            return;
        }
        let workspace = if kind.requires_engine() {
            WorkspaceKind::Engine
        } else {
            self.workspace
        };
        self.workspace = workspace;
        self.open_window(kind);
    }
    /// Focusing a palette or another tool leaves its last ModuleGraph target
    /// alone. Pane IDs are local to a workspace; scopes use stable node IDs.
    pub(super) fn reconcile_library_graph(&mut self) {
        let (layout, remembered) = match self.workspace {
            WorkspaceKind::Engine => (&self.engine_layout, &mut self.engine_library_graph),
            WorkspaceKind::Project => (&self.project_layout, &mut self.project_library_graph),
        };
        let graphs = layout
            .panes()
            .into_iter()
            .filter_map(|(id, kind)| (kind == EditorType::ModuleGraph).then_some(id))
            .collect::<Vec<_>>();
        let focused = match self.workspace {
            WorkspaceKind::Engine => self.engine_navigation.active(),
            WorkspaceKind::Project => self.project_navigation.active(),
        };
        if let Some(id) = focused.filter(|id| graphs.contains(id)) {
            *remembered = Some(id);
        } else if !remembered.is_some_and(|id| graphs.contains(&id)) {
            *remembered = graphs.first().copied();
        }
    }

    pub(super) fn library_graph_context(&self) -> Option<(u64, Vec<usize>, String)> {
        let (id, memories, layout) = match self.workspace {
            WorkspaceKind::Engine => (
                self.engine_library_graph?,
                &self.engine_panes,
                &self.engine_layout,
            ),
            WorkspaceKind::Project => (
                self.project_library_graph?,
                &self.project_panes,
                &self.project_layout,
            ),
        };
        if !layout.panes().contains(&(id, EditorType::ModuleGraph)) {
            return None;
        }
        let scope = memories
            .get(&id)
            .map(|memory| memory.snapshot.graph_scope.as_slice())
            .unwrap_or_default();
        let mut graph = &self.document.root;
        let mut path = vec![];
        let mut labels = vec![self.document.name.clone()];
        for node_id in scope {
            let index = graph.nodes.iter().position(|node| &node.id == node_id)?;
            let node = &graph.nodes[index];
            graph = node.subgraph.as_deref()?;
            path.push(index);
            labels.push(format!("{} ({})", node.label, node.id));
        }
        Some((id, path, labels.join(" / ")))
    }

    pub(super) fn add_library_node(&mut self, module: &str) {
        let Some((id, _, _)) = self.library_graph_context() else {
            self.status = "Сначала откройте нужный Module Graph.".into();
            return;
        };
        let mut memory = match self.workspace {
            WorkspaceKind::Engine => self.engine_panes.remove(&id),
            WorkspaceKind::Project => self.project_panes.remove(&id),
        }
        .unwrap_or_else(|| PaneMemory::new(PaneSnapshot::default(), &self.document));
        memory.rebind_scope(&self.document);
        memory.swap(self);
        self.add_node(module);
        memory.swap(self);
        memory.snapshot(&self.document);
        match self.workspace {
            WorkspaceKind::Engine => {
                self.engine_panes.insert(id, memory);
            }
            WorkspaceKind::Project => {
                self.project_panes.insert(id, memory);
            }
        }
        self.personal_dirty = true;
    }

    pub(super) fn draw_workspaces(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let frame = ctx.cumulative_frame_nr();
        if self.last_ui_frame != Some(frame) {
            self.last_ui_frame = Some(frame);
            self.workflow_frame(&ctx);
            let dt = if self.last_tick == 0.0 {
                1.0 / 60.0
            } else {
                (now - self.last_tick).clamp(0.0, 0.1)
            };
            self.last_tick = now;
            self.poll_candidates();
            if !self.workflow.blocks_document_edits() {
                self.tick_simulation(dt);
            }
        }
        if !self.can_author_engine() {
            self.workspace = WorkspaceKind::Project;
        }
        egui::Panel::top("workspaces")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(21, 27, 39))
                    .inner_margin(8),
            )
            .show(ui, |ui| self.draw_workflow_toolbar(ui));
        egui::Panel::bottom("work-status")
            .resizable(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&self.status).size(11.0));
                    if let Some(runtime) = &self.runtime {
                        ui.label(format!("t={:.2} · {}", self.time, runtime.backend_name()));
                    }
                });
            });
        let activities = self.preparation_activities();
        if !activities.is_empty() {
            egui::Panel::bottom("preparation-status")
                .resizable(false)
                .show(ui, |ui| {
                    crate::activity::draw_activity(ui, &activities);
                });
        }
        let available = self.available_editors();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(12, 17, 24))
                    .inner_margin(0),
            )
            .show(ui, |ui| {
                if self.workflow.blocks_document_edits() {
                    ui.disable();
                }
                ui.push_id(self.workspace.key(), |ui| {
                    let response = self.draw_window_dock(ui, &available);
                    self.reconcile_library_graph();
                    self.personal_dirty |= response.changed;
                    if response.changed {
                        let ids = match self.workspace {
                            WorkspaceKind::Engine => self.engine_layout.panes(),
                            WorkspaceKind::Project => self.project_layout.panes(),
                        }
                        .into_iter()
                        .map(|(id, _)| id)
                        .collect::<std::collections::BTreeSet<_>>();
                        match self.workspace {
                            WorkspaceKind::Engine => {
                                self.engine_panes.retain(|id, _| ids.contains(id))
                            }
                            WorkspaceKind::Project => {
                                self.project_panes.retain(|id, _| ids.contains(id))
                            }
                        };
                    }
                    for area in response.panes {
                        if area.rect.width() < 20.0 || area.rect.height() < 20.0 {
                            continue;
                        }
                        let mut memory = match self.workspace {
                            WorkspaceKind::Engine => self.engine_panes.remove(&area.id),
                            WorkspaceKind::Project => self.project_panes.remove(&area.id),
                        }
                        .unwrap_or_else(|| {
                            let mut snapshot = PaneSnapshot::default();
                            if area.editor == EditorType::Scene {
                                snapshot.camera =
                                    crate::scene_view_ui::SceneViewportState::default().camera;
                            }
                            PaneMemory::new(snapshot, &self.document)
                        });
                        memory.rebind_scope(&self.document);
                        let before = serde_json::to_value(memory.snapshot(&self.document)).ok();
                        memory.swap(self);
                        let allowed = self.can_edit_source(&self.current_source);
                        if !allowed
                            || !self
                                .document
                                .custom_sources
                                .contains_key(&self.current_source)
                        {
                            self.current_source = self
                                .document
                                .custom_sources
                                .keys()
                                .find(|id| self.can_edit_source(id))
                                .cloned()
                                .unwrap_or_default();
                            self.program = None;
                            self.command_program = None;
                            self.source_error = None;
                        }
                        if area.editor == EditorType::RustCustom
                            && (memory.parsed_source.as_ref()
                                != self.document.custom_sources.get(&self.current_source)
                                || memory.parsed_kind != Some(self.is_update_source())
                                || (self.program.is_none()
                                    && self.command_program.is_none()
                                    && self.source_error.is_none()))
                        {
                            self.parse_custom();
                            self.restore_code_positions();
                        }
                        let previous_tab = match area.editor {
                            EditorType::ModuleGraph => Tab::Graph,
                            EditorType::RustCustom => Tab::Custom,
                            EditorType::Hlsl => Tab::Shader,
                            EditorType::ProjectFile => Tab::File,
                            EditorType::History => Tab::History,
                            _ => self.tab,
                        };
                        self.tab = previous_tab;
                        self.authoring_intent = area.editor.requires_engine()
                            || (area.editor == EditorType::RustCustom
                                && self.can_author_engine()
                                && !self.distribution.as_ref().is_some_and(|p| {
                                    p.can_edit_custom_source(&self.current_source, &self.document)
                                }));
                        let mut selected = memory.snapshot.object;
                        let mut pinned = memory.snapshot.object_pinned;
                        ui.scope_builder(
                            egui::UiBuilder::new()
                                .id_salt(("pane", area.id))
                                .max_rect(area.rect),
                            |ui| {
                                ui.set_clip_rect(area.rect.intersect(area.area_rect));
                                let engine = available
                                    .iter()
                                    .find(|e| e.editor == area.editor)
                                    .is_some_and(|d| d.requires_engine);
                                if engine && !self.can_author_engine() {
                                    ui.label("Engine authoring unavailable");
                                    return;
                                }
                                ui.add_enabled_ui(
                                    !self.json_dirty || area.editor == EditorType::ProjectFile,
                                    |ui| match area.editor {
                                        EditorType::Viewport => self.viewport_panel(ui, now),
                                        EditorType::Game => self.game_scene_panel(ui),
                                        EditorType::Scene => {
                                            memory.scene_view.camera = self.camera;
                                            self.scene_panel(ui, &mut memory.scene_view);
                                            self.camera = memory.scene_view.camera;
                                        }
                                        EditorType::Hierarchy => self.scene_hierarchy_panel(ui),
                                        EditorType::Inspector => self.scene_inspector_panel(ui),
                                        EditorType::Assets => self.assets_panel(ui, area.id),
                                        EditorType::GameSystems => self.game_systems_panel(ui),
                                        EditorType::ObjectTree => {
                                            self.object_tree_panel(ui, &mut selected)
                                        }
                                        EditorType::ObjectInspector => self.object_inspector_panel(
                                            ui,
                                            &mut selected,
                                            &mut pinned,
                                        ),
                                        EditorType::ProjectSettings => {
                                            egui::ScrollArea::vertical()
                                                .show(ui, |ui| self.project_settings_panel(ui))
                                                .inner
                                        }
                                        EditorType::EngineSettings => {
                                            egui::ScrollArea::vertical()
                                                .show(ui, |ui| self.engine_settings_panel(ui))
                                                .inner
                                        }
                                        EditorType::ModuleGraph => self.module_graph_area(ui),
                                        EditorType::RustCustom => self.rust_area(ui),
                                        EditorType::Hlsl => self.shader_panel(ui),
                                        EditorType::ProjectFile => self.file_panel(ui),
                                        EditorType::History => self.history_panel(ui),
                                        EditorType::ModuleLibrary => self.library(ui),
                                        EditorType::Diagnostics => self.diagnostics_panel(ui),
                                        EditorType::Extension(id) => {
                                            if let Some(panel) =
                                                self.panel_registry.iter().find(|p| p.id == id)
                                            {
                                                (panel.factory)(ui, &self.document, &self.registry);
                                            }
                                        }
                                    },
                                );
                            },
                        );
                        self.finish_tab_change(previous_tab);
                        if self.tab != previous_tab {
                            let editor = match self.tab {
                                Tab::Graph => EditorType::ModuleGraph,
                                Tab::Custom => EditorType::RustCustom,
                                Tab::Shader => EditorType::Hlsl,
                                Tab::File => EditorType::ProjectFile,
                                Tab::History => EditorType::History,
                            };
                            if !editor.requires_engine() || self.can_author_engine() {
                                match self.workspace {
                                    WorkspaceKind::Engine => {
                                        self.engine_layout.choose(area.id, editor)
                                    }
                                    WorkspaceKind::Project => {
                                        self.project_layout.choose(area.id, editor)
                                    }
                                };
                                self.personal_dirty = true;
                            }
                        }
                        if area.editor == EditorType::RustCustom {
                            memory.parsed_source = self
                                .document
                                .custom_sources
                                .get(&self.current_source)
                                .cloned();
                            memory.parsed_kind = Some(self.is_update_source());
                        }
                        self.authoring_intent = false;
                        memory.swap(self);
                        memory.snapshot.object = selected;
                        memory.snapshot.object_pinned = pinned;
                        self.personal_dirty |=
                            before != serde_json::to_value(memory.snapshot(&self.document)).ok();
                        match self.workspace {
                            WorkspaceKind::Engine => {
                                self.engine_panes.insert(area.id, memory);
                            }
                            WorkspaceKind::Project => {
                                self.project_panes.insert(area.id, memory);
                            }
                        }
                    }
                });
            });
        self.draw_workflow_modals(&ctx);
        if self.personal_dirty && now - self.personal_save_time > 1.0 {
            self.personal_save_time = now;
            self.save_personal_layouts();
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(
            if self.game_workbench.has_pending_preparation() {
                16
            } else {
                33
            },
        ));
    }
    pub(super) fn preparation_activities(&self) -> Vec<crate::activity::ActivitySnapshot> {
        let mut activities = self.scripts.activity_snapshots();
        activities.extend(self.preview_builder.activity_snapshot());
        activities.extend(self.game_workbench.activity_snapshots());
        activities
    }
    fn diagnostics_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Diagnostics");
        egui::CollapsingHeader::new("Rendering counters (cumulative)").show(ui, |ui| {
            let workbench = self.game_workbench.performance();
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Context resolutions: {}", workbench.context_resolutions));
                ui.label(format!("Normalized document clones: {}", workbench.normalized_document_clones));
                ui.label(format!("Game submissions: {}", workbench.game_render_calls));
                ui.label(format!("Cached frames reused: {}", workbench.game_frame_reuses));
            });
            for (label, host) in [
                ("Game", self.game_workbench.host.as_ref()),
                ("Scene", self.game_workbench.preview_host.as_ref()),
            ] {
                if let Some(host) = host {
                    let counters = host.renderer_performance();
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(label);
                        ui.label(format!("Renders: {}", counters.render_calls));
                        ui.label(format!("Prepare calls: {}", counters.prepare_calls));
                        ui.label(format!("Plans built / reused: {} / {}", counters.plan_builds, counters.plan_cache_hits));
                        ui.label(format!("Pipelines created: {}", counters.pipeline_creations));
                        ui.label(format!("Prepare CPU total: {:.2} ms", counters.prepare_ns as f64 / 1_000_000.));
                    });
                }
            }
            for (workspace, layout, memories) in [
                ("Engine", &self.engine_layout, &self.engine_panes),
                ("Project", &self.project_layout, &self.project_panes),
            ] {
                for (id, kind) in layout.panes() {
                    if kind == EditorType::Scene && let Some(memory) = memories.get(&id) {
                        ui.label(format!("{workspace} Scene area {id}: {} rendered frames", memory.scene_view.render_count()));
                    }
                }
            }
            ui.small("Counts accumulate during this session; renderer counts restart when its host is replaced. Prepare time is CPU time, not GPU duration or FPS.");
        });
        if let Some(distribution) = &self.distribution {
            ui.label(format!("Profile: {:?}", distribution.profile));
            if let Some(root) = &distribution.source_status.root {
                ui.label(format!("Source workspace: {}", root.display()));
            }
            for diagnostic in &distribution.diagnostics {
                ui.label(diagnostic);
            }
            for diagnostic in &distribution.source_status.diagnostics {
                ui.label(diagnostic);
            }
        }
        if let Some(error) = &self.runtime_error {
            ui.colored_label(Color32::from_rgb(246, 134, 130), error);
        }
        if let Some(error) = &self.preview_builder.error {
            ui.colored_label(Color32::from_rgb(246, 134, 130), error);
        }
        for diagnostic in &self.diagnostics {
            ui.label(format!("{:?}: {}", diagnostic.node, diagnostic.message));
        }
        if self.diagnostics.is_empty() {
            ui.label("No graph errors");
        }
    }
}

impl Editor {
    pub(super) fn seed_capture_pane(&mut self) {
        let layout = if self.workspace == WorkspaceKind::Engine {
            &self.engine_layout
        } else {
            &self.project_layout
        };
        let Some(id) = self
            .active_window()
            .or_else(|| layout.panes().first().map(|p| p.0))
        else {
            return;
        };
        let mut scope = vec![];
        let mut graph = &self.document.root;
        for index in &self.graph_path {
            if let Some(node) = graph.nodes.get(*index)
                && let Some(inner) = node.subgraph.as_deref()
            {
                scope.push(node.id.clone());
                graph = inner;
            } else {
                break;
            }
        }
        let snapshot = PaneSnapshot {
            graph_scope: scope,
            graph_pan: [self.graph_canvas.pan.x, self.graph_canvas.pan.y],
            graph_zoom: self.graph_canvas.zoom,
            graph_selected: self.graph_canvas.selected.clone(),
            code_pan: [self.code_canvas.pan.x, self.code_canvas.pan.y],
            code_zoom: self.code_canvas.zoom,
            code_selected: self.code_canvas.selected.clone(),
            source: self.current_source.clone(),
            shader: self.current_shader.clone(),
            custom_view: self.custom_view,
            camera: self.camera,
            object: self.active_object,
            object_pinned: false,
        };
        let memory = PaneMemory::new(snapshot, &self.document);
        match self.workspace {
            WorkspaceKind::Engine => {
                self.engine_panes.insert(id, memory);
            }
            WorkspaceKind::Project => {
                self.project_panes.insert(id, memory);
            }
        }
    }
    fn module_graph_area(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let width = (rect.width() * 0.72)
            .max(120.0)
            .min((rect.width() - 170.0).max(120.0));
        let graph =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + width, rect.bottom()));
        let inspector =
            egui::Rect::from_min_max(egui::pos2(graph.right() + 8.0, rect.top()), rect.max);
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("module-canvas")
                .max_rect(graph),
            |ui| {
                self.graph_panel(ui);
            },
        );
        if inspector.width() > 80.0 {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt("module-inspector")
                    .max_rect(inspector),
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.node_inspector(ui));
                },
            );
        }
    }
    fn rust_area(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let side = (rect.width() * 0.22).clamp(150.0, 260.0);
        let main = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(
                (rect.right() - side - 8.0).max(rect.left() + 100.0),
                rect.bottom(),
            ),
        );
        ui.scope_builder(
            egui::UiBuilder::new().id_salt("rust-view").max_rect(main),
            |ui| self.custom_panel(ui),
        );
        let inspector =
            egui::Rect::from_min_max(egui::pos2(main.right() + 8.0, rect.top()), rect.max);
        if inspector.width() > 80.0 {
            ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt("code-inspector")
                    .max_rect(inspector),
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.code_inspector(ui));
                },
            );
        }
    }
}

pub fn participating_modules_panel(
    ui: &mut egui::Ui,
    document: &GraphDocument,
    registry: &ModuleRegistry,
) {
    ui.heading("Participating Modules");
    for module in module_engine_core::effective_modules(document) {
        ui.push_id(&module, |ui| {
            if let Some(descriptor) = registry.descriptor(&module) {
                ui.collapsing(&descriptor.label, |ui| {
                    ui.monospace(&module);
                    for property in &descriptor.properties {
                        ui.label(format!("{} · {:?}", property.label, property.scope));
                    }
                });
            } else {
                ui.colored_label(
                    Color32::from_rgb(246, 134, 130),
                    format!("Missing module {module}"),
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distribution::{DistributionProfile, detect};
    use module_engine_core::demo_document;
    #[test]
    fn game_defaults_to_scene_view_and_preserves_saved_personal_layout() {
        let document = crate::game_host::demo_document();
        let file = std::env::temp_dir().join(format!(
            "game-layout-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (default, _) = load(&file, WorkspaceKind::Project, &document).unwrap();
        assert_eq!(default, DockLayout::default_for(WorkspaceKind::Project));
        let mut personal = default;
        let second = personal
            .split(1, super::super::dock::Axis::Horizontal)
            .unwrap();
        personal.choose(second, EditorType::RustCustom);
        save(&file, &personal, &mut BTreeMap::new(), &document).unwrap();
        let (restored, _) = load(&file, WorkspaceKind::Project, &document).unwrap();
        assert_eq!(restored, personal);
        std::fs::remove_file(file).unwrap();
    }
    fn frame(ctx: &egui::Context, editor: &mut Editor, time: f64) {
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1200., 800.),
                )),
                time: Some(time),
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.drop_without_applying_deltas();
    }
    #[test]
    fn multiple_viewports_keep_cameras_and_personal_layout_but_only_one_simulation_tick() {
        let mut document = demo_document(WorldKind::Objects);
        host::ensure_authoring_configuration(&mut document);
        let path = crate::project::source_root().join(".work/ui-team-fixture.json");
        let distribution = detect(&path, &document, Some(DistributionProfile::Team)).unwrap();
        let mut editor = Editor::new_with_distribution(document.clone(), path, distribution);
        editor.preview_builder = PreviewBuilder::new();
        editor.runtime = Some(Runtime::from_document(&document, &editor.registry).unwrap());
        editor.project_layout = DockLayout::single(EditorType::Viewport);
        let second = editor
            .project_layout
            .split(1, dock::Axis::Horizontal)
            .unwrap();
        let first_view = PaneSnapshot {
            camera: Camera {
                yaw: 0.2,
                ..Camera::default()
            },
            ..Default::default()
        };
        let second_view = PaneSnapshot {
            camera: Camera {
                yaw: -1.0,
                distance: 12.0,
                ..Camera::default()
            },
            ..Default::default()
        };
        editor
            .project_panes
            .insert(1, PaneMemory::new(first_view, &document));
        editor
            .project_panes
            .insert(second, PaneMemory::new(second_view, &document));
        let ctx = egui::Context::default();
        editor.playing = false;
        frame(&ctx, &mut editor, 1.0);
        assert_eq!(editor.runtime.as_ref().unwrap().snapshot().time, 0.0);
        assert_eq!(editor.project_panes[&1].camera.yaw, 0.2);
        assert_eq!(editor.project_panes[&second].camera.yaw, -1.0);
        editor.playing = true;
        frame(&ctx, &mut editor, 1.05);
        assert!(
            (editor.runtime.as_ref().unwrap().snapshot().time - 0.05).abs() < 1e-6,
            "Multiple views must not multiply ticks"
        );
        editor.playing = false;
        let before = editor.runtime.as_ref().unwrap().snapshot();
        frame(&ctx, &mut editor, 1.1);
        assert_eq!(editor.runtime.as_ref().unwrap().snapshot(), before);
        let file = std::env::temp_dir().join(format!(
            "module-engine-personal-layout-{}.json",
            std::process::id()
        ));
        save(
            &file,
            &editor.project_layout,
            &mut editor.project_panes,
            &document,
        )
        .unwrap();
        let (layout, restored) = load(&file, WorkspaceKind::Project, &document).unwrap();
        assert_eq!(layout, editor.project_layout);
        assert_eq!(restored[&1].camera.yaw, 0.2);
        assert_eq!(restored[&second].camera.distance, 12.0);
        assert_eq!(
            editor.document, document,
            "Personal camera/layout never enters game snapshots"
        );
        assert!(!editor.can_author_engine());
        assert!(
            editor
                .available_editors()
                .iter()
                .all(|p| !p.requires_engine)
        );
    }
    #[test]
    fn stable_scope_ids_survive_shared_document_node_reordering() {
        let mut document = demo_document(WorldKind::Objects);
        let snapshot = PaneSnapshot {
            graph_scope: vec!["renderer".into()],
            ..Default::default()
        };
        let mut pane = PaneMemory::new(snapshot, &document);
        document.root.nodes.reverse();
        pane.rebind_scope(&document);
        assert_eq!(document.root.nodes[pane.graph_path[0]].id, "renderer");
    }
}

#[cfg(test)]
mod editor_choice_regression {
    use super::*;
    #[test]
    fn viewport_to_rust_editor_parses_shared_source_without_a_text_change() {
        let document = module_engine_core::demo_document(WorldKind::Objects);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json");
        let mut editor = Editor::new(document, path);
        editor.preview_builder = PreviewBuilder::new();
        editor.playing = false;
        editor.workspace = WorkspaceKind::Project;
        editor.project_layout = DockLayout::single(EditorType::Viewport);
        let ctx = egui::Context::default();
        for (time, kind) in [(1.0, EditorType::Viewport), (1.1, EditorType::RustCustom)] {
            editor.project_layout.choose(1, kind);
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1200., 800.),
                    )),
                    time: Some(time),
                    ..Default::default()
                },
                |ui| editor.draw(ui),
            );
            output.drop_without_applying_deltas();
        }
        assert!(
            !editor.project_panes[&1]
                .program
                .as_ref()
                .unwrap()
                .nodes()
                .is_empty()
        );
        assert_eq!(
            editor.project_panes[&1].program.as_ref().unwrap().source(),
            editor.document.custom_sources["speed"]
        );
    }
}

#[cfg(test)]
mod authored_object_regression {
    use super::*;
    #[test]
    fn spawned_objects_survive_renderer_rebuild_without_becoming_persistent_property_participants()
    {
        let mut document = module_engine_core::demo_document(WorldKind::Objects);
        host::ensure_authoring_configuration(&mut document);
        document.engine.enabled_modules.insert("game.motion".into());
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../unused.json");
        let mut editor = Editor::new(document, path);
        editor
            .runtime
            .as_mut()
            .unwrap()
            .apply_controls(&[ControlCommand::World(
                module_engine_core::WorldCommand::Spawn {
                    object: module_engine_core::WorldObject {
                        id: 9,
                        parent: None,
                        transform: module_engine_core::Transform::default(),
                        color: [255, 80, 60, 255],
                    },
                },
            )])
            .unwrap();
        editor.runtime.as_mut().unwrap().step(0.2, 1.0);
        let before = editor.runtime.as_ref().unwrap().snapshot();
        editor.document.name = "Cosmetic renderer candidate".into();
        editor.rebuild();
        assert_eq!(editor.runtime.as_ref().unwrap().snapshot(), before);
        assert!(
            editor
                .runtime
                .as_ref()
                .unwrap()
                .objects()
                .unwrap()
                .iter()
                .any(|o| o.id == 9)
        );
        assert!(
            editor.authored_object_ids.contains(&1) && !editor.authored_object_ids.contains(&9)
        );
        let mut candidate = editor.document.clone();
        module_engine_core::attach_component(&mut candidate, &editor.registry, 9, "game.motion")
            .unwrap();
        assert!(!editor.commit_properties(candidate, false));
        assert!(editor.status.contains("ScenePersistence"));
        assert!(
            !editor
                .document
                .properties
                .active
                .components
                .contains_key(&9)
        );
        assert_eq!(editor.runtime.as_ref().unwrap().snapshot(), before);
    }
}
