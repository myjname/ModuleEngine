//! Per-area transient views and personal persisted navigation. Simulation is shared.
use super::dock::{DockLayout, EditorType, WorkspaceKind};
use super::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub type ModulePanelFactory = fn(&mut egui::Ui, &GraphDocument, &ModuleRegistry);
/// A completed authoring gesture proposes a candidate; Editor owns publication.
pub type ModuleToolFactory =
    fn(&mut egui::Ui, &GraphDocument, &ModuleRegistry) -> Result<Option<GraphDocument>, String>;
pub type StatefulModuleToolFactory = fn(
    &mut egui::Ui,
    &GraphDocument,
    &ModuleRegistry,
    &module_engine_core::ModuleToolContext,
) -> Result<module_engine_core::ModuleToolResponse, String>;
#[derive(Clone, Copy)]
pub enum ModuleToolCallback {
    Legacy(ModuleToolFactory),
    Stateful(StatefulModuleToolFactory),
}
pub(super) struct ModuleToolState {
    context: module_engine_core::ModuleToolContext,
    label: String,
    dirty: bool,
}
pub(super) fn next_tool_identity() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.try_update(
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
        |id| id.checked_add(1),
    )
    .expect("Module tool identity exhausted")
}
pub struct ModulePanelRegistration {
    pub id: u32,
    pub module: String,
    pub label: String,
    pub engine_only: bool,
    pub factory: ModulePanelFactory,
    pub tool: Option<ModuleToolCallback>,
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
    pub gpu_texture: module_engine_player::gpu_presentation::GpuTexture,
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
            gpu_texture: Default::default(),
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
            gpu_texture,
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
    fn active_module_owners(&self) -> Result<BTreeSet<String>, String> {
        let root = self
            .distribution
            .as_ref()
            .map(|distribution| distribution.package_root.clone())
            .unwrap_or_else(crate::project::source_root);
        Ok(crate::host::composition_at(&root)?
            .activity(&self.document)?
            .modules)
    }

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
            tool: None,
        });
        Ok(())
    }
    // Public source-extension seam; selected packages may install no tools.
    #[allow(dead_code)]
    pub fn register_module_tool(
        &mut self,
        id: u32,
        module: &str,
        label: &str,
        engine_only: bool,
        factory: ModuleToolFactory,
    ) -> Result<(), String> {
        self.register_module_panel(id, module, label, engine_only, |_, _, _| {})?;
        self.panel_registry.last_mut().unwrap().tool = Some(ModuleToolCallback::Legacy(factory));
        Ok(())
    }

    pub fn register_stateful_module_tool(
        &mut self,
        id: u32,
        module: &str,
        label: &str,
        engine_only: bool,
        factory: StatefulModuleToolFactory,
    ) -> Result<(), String> {
        self.register_module_panel(id, module, label, engine_only, |_, _, _| {})?;
        self.panel_registry.last_mut().unwrap().tool = Some(ModuleToolCallback::Stateful(factory));
        self.module_tool_states.insert(
            id,
            ModuleToolState {
                context: module_engine_core::ModuleToolContext {
                    storage_id: next_tool_identity(),
                    document_instance: self.module_document_instance,
                    revision: 0,
                },
                label: label.into(),
                dirty: false,
            },
        );
        Ok(())
    }

    pub(super) fn has_module_tool_drafts(&self) -> bool {
        self.module_tool_states.values().any(|state| state.dirty)
    }
    pub(super) fn require_resolved_module_tools(&self, operation: &str) -> Result<(), String> {
        let labels = self
            .module_tool_states
            .values()
            .filter(|state| state.dirty)
            .map(|state| state.label.as_str())
            .collect::<Vec<_>>();
        if labels.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Apply or discard module tool drafts before {operation}: {}. These drafts are not in the saved project.",
                labels.join(", ")
            ))
        }
    }
    pub(super) fn resolve_module_tool_draft(&mut self, id: u32) {
        if let Some(state) = self.module_tool_states.get_mut(&id) {
            state.context.revision = state
                .context
                .revision
                .checked_add(1)
                .expect("Module tool revision exhausted");
            state.dirty = false;
        }
    }
    pub(super) fn adopt_module_tool_document(&mut self) {
        self.module_document_instance = next_tool_identity();
        for state in self.module_tool_states.values_mut() {
            state.context.document_instance = self.module_document_instance;
            state.context.revision = 0;
            state.dirty = false;
        }
    }
    pub(super) fn module_tool_draft_resolutions_ui(&mut self, ui: &mut egui::Ui) {
        let pending = self
            .module_tool_states
            .iter()
            .filter(|(_, state)| state.dirty)
            .map(|(id, state)| (*id, state.label.clone()))
            .collect::<Vec<_>>();
        if !pending.is_empty() {
            ui.label("Module tool drafts need Apply or explicit Discard. Cancel to return to an available tool; Discard also works for hidden or inactive tools.");
        }
        for (id, label) in pending {
            ui.push_id(("module-draft-resolution", id), |ui| {
                ui.horizontal(|ui| {
                    ui.label(label);
                    if ui.button("Discard module draft").clicked() {
                        self.resolve_module_tool_draft(id);
                    }
                });
            });
        }
    }

    fn apply_module_tool_candidate(
        &mut self,
        id: u32,
        candidate: GraphDocument,
    ) -> Result<bool, String> {
        let panel = self
            .panel_registry
            .iter()
            .find(|panel| panel.id == id && panel.tool.is_some())
            .ok_or("Module tool is not registered")?;
        if self.registry.descriptor(&panel.module).is_none()
            || !self.active_module_owners()?.contains(&panel.module)
        {
            return Err("Module tool owner is not active and installed".into());
        }
        if panel.engine_only && !self.can_author_engine() {
            return Err("Module tool requires Full source authoring".into());
        }
        if self.playing || self.game_workbench.is_play_session() {
            return Err("Stop Play before applying a module tool".into());
        }
        if self.json_dirty
            || self.game_workbench.has_graph_draft()
            || self.scene_gesture_before.is_some()
        {
            return Err(
                "Finish or discard active authoring drafts before applying a module tool".into(),
            );
        }
        if candidate == self.document {
            return Ok(false);
        }
        let label = panel.label.clone();
        let engine = panel.engine_only;
        self.validate_candidate(&candidate, engine)?;
        let before = self.document.clone();
        if candidate.game.is_some() {
            // Reuse scene/resource/preview validation and the same publication
            // boundary as Inspector and placement, with this tool's explicit
            // Engine/Project policy. Full permission never bypasses resources.
            self.commit_scene_document_with_mode(candidate, &label, engine)?;
        } else {
            self.apply_validated_json(candidate);
        }
        self.history.record("Before module tool", &before);
        self.history.record(&label, &self.document);
        self.history_dirty = false;
        self.status = label;
        Ok(true)
    }

    fn module_extension_panel(&mut self, ui: &mut egui::Ui, id: u32) {
        let Some(panel) = self.panel_registry.iter().find(|panel| panel.id == id) else {
            return;
        };
        if !self
            .active_module_owners()
            .is_ok_and(|active| active.contains(&panel.module))
            || self.registry.descriptor(&panel.module).is_none()
            || (panel.engine_only && !self.can_author_engine())
        {
            ui.label("Module tool owner or authoring permission is unavailable");
            return;
        }
        if let Some(tool) = panel.tool {
            let allowed = !self.playing
                && !self.game_workbench.is_play_session()
                && !self.json_dirty
                && !self.game_workbench.has_graph_draft();
            let context = self.module_tool_states.get(&id).map(|state| state.context);
            let result = ui
                .add_enabled_ui(allowed, |ui| match tool {
                    ModuleToolCallback::Legacy(tool) => tool(ui, &self.document, &self.registry)
                        .map(|candidate| module_engine_core::ModuleToolResponse {
                            candidate,
                            dirty: false,
                        }),
                    ModuleToolCallback::Stateful(tool) => tool(
                        ui,
                        &self.document,
                        &self.registry,
                        &context.expect("Stateful tool context registered"),
                    ),
                })
                .inner;
            let result = result.and_then(|response| {
                let was_dirty = self
                    .module_tool_states
                    .get(&id)
                    .is_some_and(|state| state.dirty);
                if let Some(state) = self.module_tool_states.get_mut(&id) {
                    state.dirty = response.dirty || response.candidate.is_some();
                }
                if let Some(candidate) = response.candidate {
                    let changed = self.apply_module_tool_candidate(id, candidate)?;
                    self.resolve_module_tool_draft(id);
                    Ok(changed)
                } else {
                    if was_dirty && !response.dirty {
                        self.resolve_module_tool_draft(id);
                    }
                    Ok(false)
                }
            });
            if let Err(error) = result {
                self.status = error.clone();
                ui.colored_label(Color32::LIGHT_RED, error);
            }
        } else {
            (panel.factory)(ui, &self.document, &self.registry);
        }
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
        let backend_root = self
            .distribution
            .as_ref()
            .map(|distribution| distribution.package_root.clone())
            .unwrap_or_else(crate::project::source_root);
        crate::host::validate_game_at(candidate, &backend_root)?;
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
        let active = self.active_module_owners().unwrap_or_default();
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
        if let Some(root) = &self.diagnostic_personal_root {
            let profile = match self.distribution.as_ref().map(|d| d.profile) {
                Some(crate::distribution::DistributionProfile::Full) => "full",
                Some(crate::distribution::DistributionProfile::Team) => "team",
                None => "unavailable",
            };
            return root.join(profile).join(format!("{}.json", workspace.key()));
        }
        #[cfg(test)]
        {
            let profile = match self.distribution.as_ref().map(|d| d.profile) {
                Some(crate::distribution::DistributionProfile::Full) => "full",
                Some(crate::distribution::DistributionProfile::Team) => "team",
                None => "unavailable",
            };
            self.test_personal_settings
                .path()
                .join(".user")
                .join(profile)
                .join(format!("{}.json", workspace.key()))
        }
        #[cfg(not(test))]
        {
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
            self.status = "Open the target Module Graph first.".into();
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
            } else {
                self.game_workbench.suspend_game_execution();
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
        self.game_workbench.begin_ui_frame();
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
                                            self.module_extension_panel(ui, id);
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
        self.game_workbench.finish_ui_frame();
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
            for (label, counters) in [
                ("Game", self.game_workbench.game_snapshot().map(|snapshot| snapshot.renderer_performance)
                    .or_else(|| self.game_workbench.host.as_ref().map(|host| host.renderer_performance()))),
                ("Scene", self.game_workbench.preview_host.as_ref().map(|host| host.renderer_performance())),
            ] {
                if let Some(counters) = counters {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(label);
                        ui.label(format!("Renders: {}", counters.render_calls));
                        ui.label(format!("Prepare calls: {}", counters.prepare_calls));
                        ui.label(format!("Plans built / reused: {} / {}", counters.plan_builds, counters.plan_cache_hits));
                        ui.label(format!("Pipelines created: {}", counters.pipeline_creations));
                        ui.label(format!("Pipeline build attempts: {}", counters.pipeline_build_attempts));
                        ui.label(format!("Rejected shader retries avoided: {}", counters.rejected_cache_hits));
                        ui.label(format!("Prepare CPU total: {:.2} ms", counters.prepare_ns as f64 / 1_000_000.));
                    });
                }
            }
            if let Some(snapshot) = self.game_workbench.game_snapshot() {
                ui.label(format!("Game worker {} · epoch {} · tick {} · queued time {:.3} s · input ack {}",
                    snapshot.actor_id, snapshot.epoch, snapshot.state.tick, snapshot.backlog_seconds, snapshot.input_ack));
                ui.small(format!("Last completed simulation {:.2} ms · render {:.2} ms", snapshot.last_step_ms, snapshot.last_render_ms));
                ui.small(format!("State revision {} · last pixels tick {:?}, revision {:?}",
                    snapshot.presentation_revision, snapshot.frame_tick, snapshot.frame_revision));
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

    fn module_tool_document() -> GraphDocument {
        let mut document = demo_document(WorldKind::Objects);
        document.game = Some(module_engine_core::GameProject {
            entry_scene: "independent".into(),
            scenes: [(
                "independent".into(),
                module_engine_core::AuthoredScene {
                    name: "Independent module fixture".into(),
                    objects: vec![module_engine_core::AuthoredObject::new(
                        module_engine_core::WorldObject {
                            id: 41,
                            parent: None,
                            transform: module_engine_core::Transform::default(),
                            color: [255; 4],
                        },
                    )],
                },
            )]
            .into(),
            prefabs: BTreeMap::new(),
            audio_cues: BTreeMap::new(),
            execution: Default::default(),
        });
        host::ensure_authoring_configuration(&mut document);
        document
    }

    fn independent_scene_tool(
        _: &mut egui::Ui,
        source: &GraphDocument,
        _: &ModuleRegistry,
    ) -> Result<Option<GraphDocument>, String> {
        let mut candidate = source.clone();
        candidate
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("independent")
            .unwrap()
            .objects[0]
            .object
            .transform
            .translation[0] += 3.;
        Ok(Some(candidate))
    }

    #[derive(Clone)]
    struct FixtureDraft {
        document: u64,
        revision: u64,
        dirty: bool,
        calls: usize,
    }
    fn independent_staged_tool(
        ui: &mut egui::Ui,
        source: &GraphDocument,
        _: &ModuleRegistry,
        context: &module_engine_core::ModuleToolContext,
    ) -> Result<module_engine_core::ModuleToolResponse, String> {
        let key = egui::Id::new(("fixture.module-draft", context.storage_id));
        let mut state = ui
            .ctx()
            .data(|data| data.get_temp::<FixtureDraft>(key))
            .filter(|state| {
                state.document == context.document_instance && state.revision == context.revision
            })
            .unwrap_or(FixtureDraft {
                document: context.document_instance,
                revision: context.revision,
                dirty: false,
                calls: 0,
            });
        state.calls += 1;
        if ui.button("Stage architecture change").clicked() {
            state.dirty = true;
        }
        let mut candidate = None;
        if ui.button("Apply staged change").clicked() {
            let mut changed = source.clone();
            let execution = &mut changed.game.as_mut().unwrap().execution;
            if !execution
                .stages
                .iter()
                .any(|stage| stage.name == "Source tool stage")
            {
                execution.stages.push(module_engine_core::SystemStage {
                    name: "Source tool stage".into(),
                    nodes: vec![],
                });
            }
            candidate = Some(changed);
        }
        if ui.button("Apply unchanged").clicked() {
            candidate = Some(source.clone());
        }
        if ui.button("Apply invalid proposal").clicked() {
            let mut changed = source.clone();
            changed.game.as_mut().unwrap().entry_scene = "missing".into();
            candidate = Some(changed);
        }
        let fail = ui.button("Fail callback").clicked();
        let dirty = state.dirty;
        ui.ctx().data_mut(|data| data.insert_temp(key, state));
        if fail {
            Err("source tool callback failed".into())
        } else {
            Ok(module_engine_core::ModuleToolResponse { candidate, dirty })
        }
    }
    fn tool_test_frame(
        editor: &mut Editor,
        context: &egui::Context,
        id: Option<u32>,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 760.),
                )),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| {
                if let Some(id) = id {
                    ui.push_id(id, |ui| editor.module_extension_panel(ui, id));
                } else {
                    editor.draw_workflow_modals(ui.ctx());
                }
            },
        );
        let labels = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().into(),
                    text.pos + text.galley.size() * 0.5,
                )),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        labels
    }
    fn tool_test_click(editor: &mut Editor, context: &egui::Context, id: Option<u32>, label: &str) {
        // Modal Areas perform an invisible sizing/fade pass on first use.
        // Observe real settled paint before deriving a pointer hit position.
        if id.is_none() {
            for _ in 0..3 {
                tool_test_frame(editor, context, id, vec![]);
            }
        }
        let mut labels = tool_test_frame(editor, context, id, vec![]);
        for _ in 0..6 {
            if labels.iter().any(|(text, _)| text == label) {
                break;
            }
            labels = tool_test_frame(editor, context, id, vec![]);
        }
        let at = labels
            .iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("Missing actual tool/modal button {label}: {labels:?}"))
            .1;
        tool_test_frame(
            editor,
            context,
            id,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        tool_test_frame(
            editor,
            context,
            id,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    fn play_test_frame(
        editor: &mut Editor,
        context: &egui::Context,
        engine: bool,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640., 480.),
                )),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| {
                if engine {
                    editor.viewport_panel(ui, 0.);
                } else {
                    editor.game_scene_panel(ui);
                }
            },
        );
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().into(),
                    text.pos + text.galley.size() * 0.5,
                )),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        text
    }

    #[test]
    fn stateful_tools_resolve_only_their_own_proposals_and_inactive_discard_needs_no_factory() {
        let document = module_tool_document();
        let fixture = full_source_fixture(&document);
        let mut editor = Editor::new(document.clone(), fixture.path().join("tool-lifecycle.json"));
        editor.playing = false;
        editor
            .register_stateful_module_tool(
                620,
                "world.objects",
                "Tool A",
                false,
                independent_staged_tool,
            )
            .unwrap();
        editor
            .register_stateful_module_tool(
                621,
                "scene.extract",
                "Tool B",
                true,
                independent_staged_tool,
            )
            .unwrap();
        let context = egui::Context::default();
        for id in [620, 621] {
            tool_test_click(&mut editor, &context, Some(id), "Stage architecture change");
        }
        let a = editor.module_tool_states[&620].context;
        let b = editor.module_tool_states[&621].context;
        assert!(editor.module_tool_states[&620].dirty && editor.module_tool_states[&621].dirty);
        use super::super::editor_workflow::Command;
        for command in [
            Command::Open,
            Command::NewG1,
            Command::Quit,
            Command::Check,
            Command::ExportPlayer,
        ] {
            editor.dispatch_command(command, &context);
            assert!(editor.workflow.blocks_document_edits());
            let unsaved = if matches!(command, Command::Check | Command::ExportPlayer) {
                "Use saved version"
            } else {
                "Don't save"
            };
            tool_test_click(&mut editor, &context, None, unsaved);
            assert!(
                editor.workflow.blocks_document_edits(),
                "Don't-save cannot bypass pending tool data"
            );
            tool_test_click(&mut editor, &context, None, "Cancel");
            assert!(!editor.workflow.blocks_document_edits());
            assert_eq!(editor.module_tool_states[&620].context, a);
            assert_eq!(editor.module_tool_states[&621].context, b);
            assert_eq!(editor.document, document);
        }
        editor.state_migration = "unrelated Rust migration".into();
        editor.json_draft = "{ unresolved JSON draft".into();
        editor.json_dirty = true;
        for command in [Command::Save, Command::SaveAs] {
            editor.dispatch_command(command, &context);
            tool_test_click(&mut editor, &context, None, "Close");
            assert_eq!(editor.json_draft, "{ unresolved JSON draft");
            assert!(editor.json_dirty);
            assert_eq!(editor.module_tool_states[&620].context, a);
            assert_eq!(editor.document, document);
        }
        editor.json_dirty = false;
        assert!(
            !fixture.path().join("tool-lifecycle.json").exists(),
            "guarded operations never publish a file"
        );
        for engine in [false, true] {
            let label = if engine { "Start" } else { "Play" };
            let at = play_test_frame(&mut editor, &context, engine, vec![])
                .into_iter()
                .find(|(text, _)| text == label)
                .unwrap()
                .1;
            play_test_frame(
                &mut editor,
                &context,
                engine,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            play_test_frame(
                &mut editor,
                &context,
                engine,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(!editor.playing && !editor.game_workbench.is_play_session());
            assert!(
                editor
                    .status
                    .contains("Apply or discard module tool drafts")
            );
        }
        editor.playing = true; // Defensive guard for a legacy/direct start path.
        editor.tick_simulation(1. / 60.);
        assert!(!editor.playing && !editor.game_workbench.is_play_session());
        assert_eq!(
            editor
                .game_workbench
                .host
                .as_ref()
                .unwrap()
                .session
                .state()
                .tick,
            0
        );
        assert!(editor.module_tool_states[&620].dirty && editor.module_tool_states[&621].dirty);
        tool_test_click(&mut editor, &context, Some(620), "Apply staged change");
        assert_eq!(
            editor.document, document,
            "Project tool cannot change the Engine blueprint"
        );
        assert_eq!(editor.module_tool_states[&620].context, a);
        assert!(editor.module_tool_states[&620].dirty);
        tool_test_click(&mut editor, &context, Some(621), "Apply staged change");
        assert!(!editor.module_tool_states[&621].dirty);
        assert_ne!(editor.module_tool_states[&621].context.revision, b.revision);
        tool_test_frame(&mut editor, &context, Some(620), vec![]);
        assert!(
            editor.module_tool_states[&620].dirty,
            "another tool adopting the same proposal is not A's acknowledgement"
        );
        assert_eq!(editor.module_tool_states[&620].context, a);
        let history = editor.history.entries().len();
        tool_test_click(&mut editor, &context, Some(620), "Apply unchanged");
        assert!(!editor.module_tool_states[&620].dirty);
        assert_ne!(editor.module_tool_states[&620].context.revision, a.revision);
        assert_eq!(
            editor.history.entries().len(),
            history,
            "accepted no-op adds no history"
        );
        for id in [620, 621] {
            tool_test_click(&mut editor, &context, Some(id), "Stage architecture change");
        }
        let pending = editor.module_tool_states[&620].context;
        tool_test_click(&mut editor, &context, Some(620), "Fail callback");
        assert!(editor.module_tool_states[&620].dirty);
        assert_eq!(editor.module_tool_states[&620].context, pending);
        tool_test_click(&mut editor, &context, Some(620), "Apply invalid proposal");
        assert!(editor.module_tool_states[&620].dirty);
        assert_eq!(editor.module_tool_states[&620].context, pending);
        editor.switch_world(WorldKind::Ecs); // Installed world.objects owner becomes inactive.
        let key = egui::Id::new(("fixture.module-draft", pending.storage_id));
        let calls = context.data(|data| data.get_temp::<FixtureDraft>(key).unwrap().calls);
        let inactive = tool_test_frame(&mut editor, &context, Some(620), vec![]);
        assert!(inactive.iter().any(|(text,_)|text == "Module tool owner or authoring permission is unavailable"));
        assert_eq!(
            context.data(|data| data.get_temp::<FixtureDraft>(key).unwrap().calls),
            calls
        );
        editor.json_draft = "{ isolated JSON draft".into();
        editor.json_dirty = true;
        editor.dispatch_command(Command::PlayPause, &context);
        assert!(!editor.playing && !editor.game_workbench.is_play_session());
        tool_test_click(&mut editor, &context, None, "Discard module draft"); // First row is Tool A, even inactive.
        assert!(!editor.module_tool_states[&620].dirty);
        assert!(editor.module_tool_states[&621].dirty);
        tool_test_click(&mut editor, &context, None, "Close");
        assert_eq!(editor.state_migration, "unrelated Rust migration");
        assert_eq!(editor.json_draft, "{ isolated JSON draft");
        assert!(editor.json_dirty);
        editor.json_dirty = false;
        editor.switch_world(WorldKind::Objects);
        tool_test_frame(&mut editor, &context, Some(620), vec![]);
        assert!(
            !editor.module_tool_states[&620].dirty,
            "next draw uses the explicit Discard revision"
        );
        assert!(editor.module_tool_states[&621].dirty);
    }

    #[test]
    fn module_tool_publishes_valid_scene_candidate_with_one_undo_and_no_direct_source_access() {
        let document = module_tool_document();
        let fixture = full_source_fixture(&document);
        let mut editor = Editor::new(document.clone(), fixture.path().join("unused-tool.json"));
        editor.playing = false;
        assert!(editor.can_author_engine());
        editor
            .register_module_tool(
                601,
                "scene.extract",
                "Independent source tool",
                false,
                independent_scene_tool,
            )
            .unwrap();
        let history_before = editor.history.entries().len();
        let context = egui::Context::default();
        context
            .run_ui(egui::RawInput::default(), |ui| {
                editor.module_extension_panel(ui, 601)
            })
            .drop_without_applying_deltas();
        assert_eq!(
            editor.document.game.as_ref().unwrap().scenes["independent"].objects[0]
                .object
                .transform
                .translation[0],
            3.
        );
        assert_eq!(editor.history.entries().len(), history_before + 1);
        assert!(editor.dirty);
        assert!(
            !fixture.path().join("unused-tool.json").exists(),
            "publication stays in memory until Editor Save"
        );
        editor.undo();
        assert_eq!(editor.document, document);
    }

    fn g1_palette_frame(
        editor: &mut Editor,
        context: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (egui::Rect, egui::Pos2) {
        let board = module_engine_g1::GridDraft::from_project(
            editor.document.game.as_ref().unwrap(),
            "level01",
        )
        .unwrap();
        let output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900., 760.),
                )),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| editor.module_extension_panel(ui, 110),
        );
        let grid = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect)
                if rect.fill == Color32::from_rgb(239, 233, 210)
                    && (rect.rect.width() - board.width as f32 * 34.).abs() < 0.1
                    && (rect.rect.height() - board.height as f32 * 34.).abs() < 0.1 =>
            {
                Some(rect.rect)
            }
            _ => None,
        });
        let apply = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "Apply level" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        });
        output.drop_without_applying_deltas();
        (
            grid.expect("registered G1 palette must draw its actual grid"),
            apply.expect("registered G1 palette must draw Apply level"),
        )
    }
    fn g1_palette_click(
        editor: &mut Editor,
        context: &egui::Context,
        at: egui::Pos2,
    ) -> (egui::Rect, egui::Pos2) {
        g1_palette_frame(
            editor,
            context,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        g1_palette_frame(
            editor,
            context,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
    }

    #[test]
    fn g1_registered_palette_pointer_apply_undo_redo_and_save_reopen_preserve_authored_data() {
        fn copy_assets(source: &Path, destination: &Path) {
            std::fs::create_dir_all(destination).unwrap();
            for entry in std::fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let target = destination.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_assets(&entry.path(), &target);
                } else {
                    std::fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        let current = crate::game_templates::g1_document();
        let fixture = full_source_fixture(&current);
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let source_root = fixture.path().join("examples/crate-garden");
        copy_assets(
            &checkout.join("examples/crate-garden/assets"),
            &source_root.join("assets"),
        );
        let source_path = fixture.path().join("author.json");
        crate::project::save(&source_path, &current).unwrap();
        let full = crate::distribution::detect(&source_path, &current, None).unwrap();
        assert!(full.can_author_engine());
        let saved = crate::workspace_files::new_project_file_with_choice(
            &source_path,
            &fixture.path().join("projects/palette/project.json"),
            &current,
            &full,
            crate::workspace_files::ProjectTemplate::G1,
            crate::engine_profiles::EngineChoice::Selected(crate::engine_profiles::ProfileKind::G1),
        )
        .unwrap();
        let mut authored = saved.document;
        let scene = authored
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("level01")
            .unwrap();
        let goal = scene
            .objects
            .iter()
            .find(|object| {
                object
                    .components
                    .get(module_engine_g1::CELL)
                    .is_some_and(|cell| cell["kind"] == "goal")
            })
            .unwrap()
            .object
            .id;
        let pawn = scene
            .objects
            .iter_mut()
            .find(|object| {
                object
                    .components
                    .get(module_engine_g1::CELL)
                    .is_some_and(|cell| cell["kind"] == "pawn")
            })
            .unwrap();
        pawn.components.insert(
            "foreign.palette-test".into(),
            serde_json::json!({"literal.key":null,"opaque":[false,3]}),
        );
        pawn.references.insert("foreign-peer".into(), goal);
        let retained = pawn.clone();
        crate::project::save(&saved.path, &authored).unwrap();
        let original_bytes = std::fs::read(&saved.path).unwrap();
        let mut editor = Editor::new(authored.clone(), saved.path.clone());
        editor.playing = false;
        assert!(editor.can_author_engine());
        let registration = editor
            .panel_registry
            .iter()
            .find(|panel| panel.id == 110)
            .unwrap();
        assert_eq!(registration.module, "g1.rules");
        assert!(registration.tool.is_some());
        let history = editor.history.entries().len();
        let cursor = editor.history.cursor();
        let context = egui::Context::default();
        let (grid, apply) = g1_palette_frame(&mut editor, &context, vec![]);
        g1_palette_click(&mut editor, &context, apply); // Actual no-op Apply, not a forged candidate.
        assert_eq!(editor.document, authored);
        assert_eq!(editor.history.entries().len(), history);
        // This wall does not intersect level01's N solution or any actor/goal.
        let at = grid.min + egui::vec2(1.5, 1.5) * 34.;
        let (_, apply) = g1_palette_click(&mut editor, &context, at);
        assert_eq!(editor.document, authored, "painting is a local tool draft");
        assert_eq!(editor.history.entries().len(), history);
        assert_eq!(std::fs::read(&saved.path).unwrap(), original_bytes);
        g1_palette_click(&mut editor, &context, apply);
        let accepted = editor.document.clone();
        assert_ne!(
            accepted, authored,
            "real Apply must publish the painted candidate: {}",
            editor.status
        );
        let game = accepted.game.as_ref().unwrap();
        assert!(
            module_engine_g1::GridDraft::from_project(game, "level01")
                .unwrap()
                .walls
                .contains(&module_engine_g1::Cell::new(1, 1))
        );
        assert_eq!(game.execution, authored.game.as_ref().unwrap().execution);
        assert_eq!(
            game.scenes["level02"],
            authored.game.as_ref().unwrap().scenes["level02"]
        );
        assert_eq!(
            game.scenes["level01"]
                .objects
                .iter()
                .find(|object| object.object.id == retained.object.id)
                .unwrap(),
            &retained
        );
        assert_eq!(editor.history.entries().len(), history + 1);
        assert_eq!(editor.history.cursor(), cursor + 1);
        assert_eq!(
            std::fs::read(&saved.path).unwrap(),
            original_bytes,
            "publication is in memory until Save"
        );
        let acknowledged = editor.module_tool_states[&110].context;
        assert!(!editor.has_module_tool_drafts());
        editor.undo(); // No tool redraw between acceptance and this Undo.
        assert_eq!(editor.document, authored);
        g1_palette_frame(&mut editor, &context, vec![]);
        assert!(
            !editor.has_module_tool_drafts(),
            "accepted paint must not resurrect after Undo"
        );
        assert_eq!(editor.module_tool_states[&110].context, acknowledged);
        editor.redo();
        g1_palette_frame(&mut editor, &context, vec![]);
        assert!(
            !editor.has_module_tool_drafts(),
            "a clean cache follows Redo"
        );
        g1_palette_click(&mut editor, &context, apply); // Accepted-source rebase is also a no-op.
        assert_eq!(editor.document, accepted);
        assert_eq!(editor.history.entries().len(), history + 1);
        editor.undo();
        assert_eq!(editor.document, authored);
        editor.redo();
        assert_eq!(editor.document, accepted);
        assert!(editor.save(), "{}", editor.status);
        let reopened = crate::project::read(&saved.path).unwrap();
        assert_eq!(reopened, accepted);
        module_engine_project::AssetManifest::from_value(&reopened.asset_manifest)
            .unwrap()
            .verify(saved.path.parent().unwrap())
            .unwrap();
        // An unaccepted draft must instead survive an external author Undo as
        // stale work; it cannot silently overwrite that reverted document.
        let (grid, apply) = g1_palette_frame(&mut editor, &context, vec![]);
        g1_palette_click(&mut editor, &context, grid.min + egui::vec2(2.5, 1.5) * 34.);
        assert!(editor.has_module_tool_drafts());
        editor.undo();
        assert_eq!(editor.document, authored);
        g1_palette_click(&mut editor, &context, apply); // The stale Apply button is disabled.
        assert_eq!(editor.document, authored);
        assert!(editor.has_module_tool_drafts());
        tool_test_click(&mut editor, &context, Some(110), "Discard draft");
        assert!(!editor.has_module_tool_drafts());
        editor.redo();
        g1_palette_frame(&mut editor, &context, vec![]);
        assert_eq!(editor.document, accepted);
        assert!(!editor.has_module_tool_drafts());
    }

    #[test]
    #[ignore = "requires a genuinely exported Team delivery in MODULE_ENGINE_TEST_TEAM_ROOT; no native window is launched"]
    fn g1_team_registered_palette_pointer_apply_history_save_preserves_frozen_execution() {
        fn files(root: &Path, current: &Path, hashes: &mut BTreeMap<String, String>) {
            for entry in std::fs::read_dir(current).unwrap() {
                let entry = entry.unwrap();
                let kind = entry.file_type().unwrap();
                assert!(
                    !kind.is_symlink(),
                    "Team fixture must contain ordinary package files"
                );
                if kind.is_dir() {
                    files(root, &entry.path(), hashes);
                } else {
                    hashes.insert(
                        entry
                            .path()
                            .strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                        module_engine_project::hash_file(&entry.path()).unwrap(),
                    );
                }
            }
        }
        fn copy(source: &Path, target: &Path) {
            std::fs::create_dir_all(target).unwrap();
            for entry in std::fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    copy(&entry.path(), &target.join(entry.file_name()));
                } else {
                    std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
                }
            }
        }
        let source = std::path::PathBuf::from(
            std::env::var_os("MODULE_ENGINE_TEST_TEAM_ROOT")
                .expect("explicit validated Team delivery required"),
        )
        .canonicalize()
        .unwrap();
        let original = crate::project::read(&source.join("project.json")).unwrap();
        let capability =
            crate::distribution::detect(&source.join("project.json"), &original, None).unwrap();
        assert_eq!(
            capability.profile,
            crate::distribution::DistributionProfile::Team
        );
        assert!(!capability.can_author_engine());
        assert!(capability.compile_root().unwrap().is_dir());
        let mut sealed = BTreeMap::new();
        files(&source, &source, &mut sealed);
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.work/test-temp");
        let fixture =
            module_engine_script::TestArtifactGuard::new(&parent, "actual-team-palette").unwrap();
        assert!(!fixture.path().starts_with(&source));
        copy(&source, fixture.path());
        let mut copied = BTreeMap::new();
        files(fixture.path(), fixture.path(), &mut copied);
        assert_eq!(
            sealed, copied,
            "copy retains the original manifest, SDK, backend and prebuilt file tree"
        );
        let path = fixture.path().join("project.json");
        let mut authored = crate::project::read(&path).unwrap();
        let scene = authored
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("level01")
            .unwrap();
        let pawn = scene
            .objects
            .iter_mut()
            .find(|object| {
                object
                    .components
                    .get(module_engine_g1::CELL)
                    .is_some_and(|cell| cell["kind"] == "pawn")
            })
            .unwrap();
        pawn.components.insert(
            "foreign.team-palette".into(),
            serde_json::json!({"literal.key":null,"opaque":[1,false]}),
        );
        let retained = pawn.clone();
        crate::project::save(&path, &authored).unwrap(); // Normal permitted Team scene editing, never reseal.
        let checked = crate::distribution::detect(&path, &authored, None).unwrap();
        checked.validate_document(&authored).unwrap();
        let mut editor = Editor::new_with_distribution(authored.clone(), path.clone(), checked);
        editor.playing = false;
        assert_eq!(
            editor.distribution.as_ref().unwrap().profile,
            crate::distribution::DistributionProfile::Team
        );
        assert!(!editor.can_author_engine());
        let context = egui::Context::default();
        let history = editor.history.entries().len();
        let (grid, apply) = g1_palette_frame(&mut editor, &context, vec![]);
        g1_palette_click(&mut editor, &context, apply);
        assert_eq!(editor.document, authored);
        let (_, apply) =
            g1_palette_click(&mut editor, &context, grid.min + egui::vec2(1.5, 1.5) * 34.);
        assert_eq!(editor.document, authored);
        g1_palette_click(&mut editor, &context, apply);
        let accepted = editor.document.clone();
        assert_ne!(accepted, authored, "{}", editor.status);
        assert_eq!(editor.history.entries().len(), history + 1);
        assert_eq!(
            accepted.game.as_ref().unwrap().execution,
            authored.game.as_ref().unwrap().execution
        );
        assert_eq!(
            accepted.game.as_ref().unwrap().scenes["level02"],
            authored.game.as_ref().unwrap().scenes["level02"]
        );
        assert_eq!(
            accepted.game.as_ref().unwrap().scenes["level01"]
                .objects
                .iter()
                .find(|object| object.object.id == retained.object.id)
                .unwrap(),
            &retained
        );
        let mut forbidden = accepted.clone();
        forbidden
            .game
            .as_mut()
            .unwrap()
            .execution
            .stages
            .push(module_engine_core::SystemStage {
                name: "forbidden".into(),
                nodes: vec![],
            });
        assert!(editor.apply_module_tool_candidate(110, forbidden).is_err());
        assert_eq!(editor.document, accepted);
        assert_eq!(editor.history.entries().len(), history + 1);
        editor.undo();
        assert_eq!(editor.document, authored);
        editor.redo();
        assert_eq!(editor.document, accepted);
        assert!(editor.save(), "{}", editor.status);
        assert_eq!(crate::project::read(&path).unwrap(), accepted);
        module_engine_project::AssetManifest::from_value(&accepted.asset_manifest)
            .unwrap()
            .verify(fixture.path())
            .unwrap();
        assert!(!editor.can_author_engine());
        let mut after = BTreeMap::new();
        files(&source, &source, &mut after);
        assert_eq!(
            after, sealed,
            "source delivery remains byte-exact; only disposable copy is edited"
        );
        println!(
            "Team input {} files={} manifest={} editor={}",
            source.display(),
            sealed.len(),
            sealed[".module-engine-package.json"],
            sealed[crate::distribution::EDITOR_EXECUTABLE]
        );
    }

    #[test]
    fn g1_unapplied_palette_draft_cannot_cross_a_successful_identical_project_open() {
        fn copy_assets(source: &Path, destination: &Path) {
            std::fs::create_dir_all(destination).unwrap();
            for entry in std::fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let target = destination.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_assets(&entry.path(), &target);
                } else {
                    std::fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        fn frame(
            editor: &mut Editor,
            context: &egui::Context,
            events: Vec<egui::Event>,
        ) -> (egui::Rect, Vec<String>) {
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 760.),
                    )),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| {
                    let pane = editor
                        .project_layout
                        .panes()
                        .into_iter()
                        .find(|(_, kind)| *kind == EditorType::Extension(110))
                        .map_or(0, |(id, _)| id);
                    ui.push_id(("pane", pane), |ui| editor.module_extension_panel(ui, 110));
                },
            );
            let grid = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.fill == Color32::from_rgb(239, 233, 210)
                        && (rect.rect.width() - 238.).abs() < 0.1
                        && (rect.rect.height() - 170.).abs() < 0.1 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            });
            let text = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
            output.drop_without_applying_deltas();
            (grid.expect("real G1 tool110 must draw level01"), text)
        }
        let source = crate::game_templates::g1_document();
        let fixture = full_source_fixture(&source);
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        copy_assets(
            &checkout.join("examples/crate-garden/assets"),
            &fixture.path().join("assets"),
        );
        let a = fixture.path().join("A.json");
        let b = fixture.path().join("B.json");
        crate::project::save(&a, &source).unwrap();
        crate::project::save(&b, &source).unwrap();
        let mut editor = Editor::new(source.clone(), a);
        editor.playing = false;
        assert!(editor.can_author_engine());
        editor.workspace = WorkspaceKind::Project;
        editor.reveal_editor(EditorType::Extension(110));
        let context = egui::Context::default();
        let (grid, _) = frame(&mut editor, &context, vec![]);
        let at = grid.min + egui::vec2(1.5, 1.5) * 34.;
        frame(
            &mut editor,
            &context,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let (_, painted) = frame(
            &mut editor,
            &context,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(
            painted
                .iter()
                .any(|text| text == "Draft has unapplied changes")
        );
        assert_eq!(editor.document, source, "A's paint remains unapplied");
        assert!(editor.has_unsaved_work());
        let previous_context = editor.module_tool_states[&110].context;
        let old_pane = editor
            .project_layout
            .panes()
            .into_iter()
            .find(|(_, kind)| *kind == EditorType::Extension(110))
            .unwrap()
            .0;
        assert!(editor.project_layout.close(old_pane));
        editor.reveal_editor(EditorType::Extension(110));
        assert!(
            frame(&mut editor, &context, vec![])
                .1
                .iter()
                .any(|text| text == "Draft has unapplied changes"),
            "closing/reopening a pane retains its draft"
        );
        assert_eq!(editor.module_tool_states[&110].context, previous_context);
        let old_path = editor.path.clone();
        editor.path = fixture
            .path()
            .join("missing.json")
            .to_string_lossy()
            .into_owned();
        assert!(!editor.load());
        editor.path = old_path;
        assert_eq!(editor.module_tool_states[&110].context, previous_context);
        assert!(
            frame(&mut editor, &context, vec![])
                .1
                .iter()
                .any(|text| text == "Draft has unapplied changes")
        );
        editor.path = b.to_string_lossy().into_owned();
        assert!(editor.load(), "{}", editor.status);
        assert_eq!(
            editor.document, source,
            "B intentionally has byte-identical game data"
        );
        let (_, reopened) = frame(&mut editor, &context, vec![]);
        assert!(
            reopened
                .iter()
                .any(|text| text == "Level matches authored source"),
            "B inherited A's tool-local draft: {reopened:?}"
        );
        assert!(
            !reopened
                .iter()
                .any(|text| text == "Draft has unapplied changes")
        );
        let opened = editor.module_tool_states[&110].context;
        assert_eq!(opened.storage_id, previous_context.storage_id);
        assert_ne!(opened.document_instance, previous_context.document_instance);
        assert!(editor.load(), "same-file reload is a new project adoption");
        assert_ne!(
            editor.module_tool_states[&110].context.document_instance,
            opened.document_instance
        );
        assert!(!editor.has_module_tool_drafts());
    }

    #[test]
    fn module_tool_noop_errors_team_play_and_inactive_owner_preserve_document_and_history() {
        let document = module_tool_document();
        let fixture = full_source_fixture(&document);
        let mut editor = Editor::new(document, fixture.path().join("unused-tool-rejection.json"));
        editor.playing = false;
        editor
            .register_module_tool(
                602,
                "scene.extract",
                "Full tool",
                true,
                independent_scene_tool,
            )
            .unwrap();
        editor
            .register_module_tool(
                603,
                "scene.extract",
                "Rejected source tool",
                false,
                |_, _, _| Err("source callback rejected gesture".into()),
            )
            .unwrap();
        let before = editor.document.clone();
        let history = editor.history.entries().len();
        assert!(
            !editor
                .apply_module_tool_candidate(602, before.clone())
                .unwrap()
        );
        let mut invalid = before.clone();
        invalid.game.as_mut().unwrap().entry_scene = "missing".into();
        assert!(editor.apply_module_tool_candidate(602, invalid).is_err());
        let context = egui::Context::default();
        context
            .run_ui(egui::RawInput::default(), |ui| {
                editor.module_extension_panel(ui, 603)
            })
            .drop_without_applying_deltas();
        assert!(editor.status.contains("source callback rejected gesture"));
        let mut changed = before.clone();
        changed
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("independent")
            .unwrap()
            .name = "Changed".into();
        editor.playing = true;
        assert!(
            editor
                .apply_module_tool_candidate(602, changed.clone())
                .unwrap_err()
                .contains("Stop Play")
        );
        editor.playing = false;
        editor.distribution.as_mut().unwrap().profile =
            crate::distribution::DistributionProfile::Team;
        assert!(
            editor
                .apply_module_tool_candidate(602, changed.clone())
                .unwrap_err()
                .contains("Full")
        );
        editor.distribution.as_mut().unwrap().profile =
            crate::distribution::DistributionProfile::Full;
        editor
            .register_module_tool(
                604,
                "custom.rust.update",
                "Inactive owner",
                false,
                independent_scene_tool,
            )
            .unwrap();
        assert!(
            editor
                .apply_module_tool_candidate(604, changed)
                .unwrap_err()
                .contains("active and installed")
        );
        assert_eq!(editor.document, before);
        assert_eq!(editor.history.entries().len(), history);
    }

    #[test]
    fn scheduled_source_system_owner_exposes_tool_without_root_node_or_engine_toggle() {
        let mut document = module_tool_document();
        document.game.as_mut().unwrap().execution = module_engine_core::ExecutionGraph {
            resources: vec![module_engine_core::SystemResource {
                name: "authored-world".into(),
                data_type: "GameWorld".into(),
                external: true,
                initial: serde_json::Value::Null,
            }],
            stages: vec![module_engine_core::SystemStage {
                name: "Independent source".into(),
                nodes: vec![module_engine_core::SystemNode {
                    id: "independent-spin".into(),
                    module: "example.spin".into(),
                    params: serde_json::json!({}),
                    bindings: [("world".into(), "authored-world".into())].into(),
                    after: vec![],
                }],
            }],
        };
        assert!(!module_engine_core::effective_modules(&document).contains("example.spin"));
        let fixture = full_source_fixture(&document);
        let mut editor = Editor::new(document, fixture.path().join("unused-scheduled-tool.json"));
        editor.playing = false;
        editor
            .register_module_tool(
                605,
                "example.spin",
                "Scheduled module tool",
                false,
                independent_scene_tool,
            )
            .unwrap();
        assert!(
            editor
                .available_editors_for(WorkspaceKind::Project)
                .iter()
                .any(|editor| editor.editor == EditorType::Extension(605))
        );
        let context = egui::Context::default();
        context
            .run_ui(egui::RawInput::default(), |ui| {
                editor.module_extension_panel(ui, 605)
            })
            .drop_without_applying_deltas();
        assert_eq!(
            editor.document.game.as_ref().unwrap().scenes["independent"].objects[0]
                .object
                .transform
                .translation[0],
            3.
        );
        editor.document.game.as_mut().unwrap().execution = Default::default();
        assert!(
            !editor
                .available_editors_for(WorkspaceKind::Project)
                .iter()
                .any(|editor| editor.editor == EditorType::Extension(605))
        );
        let before = editor.document.clone();
        let mut candidate = before.clone();
        candidate
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("independent")
            .unwrap()
            .name = "Unavailable edit".into();
        let history = editor.history.entries().len();
        assert!(
            editor
                .apply_module_tool_candidate(605, candidate)
                .unwrap_err()
                .contains("active and installed")
        );
        assert_eq!(editor.document, before);
        assert_eq!(editor.history.entries().len(), history);
    }

    #[test]
    fn full_engine_tool_can_change_game_blueprint_while_project_and_team_tools_cannot() {
        let document = module_tool_document();
        let fixture = full_source_fixture(&document);
        let mut editor = Editor::new(document, fixture.path().join("unused-engine-tool.json"));
        editor.playing = false;
        editor
            .register_module_tool(
                606,
                "scene.extract",
                "Engine blueprint tool",
                true,
                independent_scene_tool,
            )
            .unwrap();
        editor
            .register_module_tool(
                607,
                "scene.extract",
                "Project tool",
                false,
                independent_scene_tool,
            )
            .unwrap();
        let before = editor.document.clone();
        let history = editor.history.entries().len();
        let mut candidate = before.clone();
        candidate
            .game
            .as_mut()
            .unwrap()
            .execution
            .stages
            .push(module_engine_core::SystemStage {
                name: "Explicit engine stage".into(),
                nodes: vec![],
            });
        assert!(
            editor
                .apply_module_tool_candidate(607, candidate.clone())
                .is_err()
        );
        assert_eq!(editor.document, before);
        assert_eq!(editor.history.entries().len(), history);
        assert!(
            editor
                .apply_module_tool_candidate(606, candidate.clone())
                .unwrap()
        );
        assert_eq!(editor.document, candidate);
        assert_eq!(
            editor.policy_baseline, candidate,
            "accepted Engine edit establishes the Project policy baseline"
        );
        assert_eq!(editor.history.entries().len(), history + 1);
        editor.distribution.as_mut().unwrap().profile =
            crate::distribution::DistributionProfile::Team;
        assert!(
            editor
                .apply_module_tool_candidate(606, before)
                .unwrap_err()
                .contains("Full")
        );
        assert_eq!(editor.document, candidate);
        assert_eq!(editor.history.entries().len(), history + 1);
    }

    fn full_source_fixture(document: &GraphDocument) -> module_engine_script::TestArtifactGuard {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.work/test-temp");
        let guard =
            module_engine_script::TestArtifactGuard::new(&base, "pane-full-source").unwrap();
        crate::distribution::copy_native_fixture(guard.path());
        crate::distribution::write_full_manifest(guard.path(), document).unwrap();
        guard
    }

    #[test]
    fn diagnostic_personal_settings_preserve_other_editor_state_and_source_permissions() {
        let document = demo_document(WorldKind::Objects);
        let sources = full_source_fixture(&document);
        let root = sources.path();
        let path = root.join("unused-diagnostic-settings-isolation.json");
        let first = Editor::new(document.clone(), path.clone());
        let distribution = first.distribution.as_ref().unwrap().clone();
        assert!(distribution.can_author_engine());
        assert_eq!(distribution.package_root, root);
        let compile_root = distribution.compile_root().unwrap();
        let ordinary_recent = first
            .test_personal_settings
            .path()
            .join(".user/editor-recent-projects.json");
        let recent_before = std::fs::read(&ordinary_recent).unwrap();
        let ordinary_layout = first.personal_path(WorkspaceKind::Project);
        let isolated = module_engine_script::TestArtifactGuard::new(
            &std::env::temp_dir().join("module-engine-diagnostic-settings"),
            "editor-isolation",
        )
        .unwrap();
        let mut diagnostic = Editor::new_with_diagnostic_personal_root(
            document.clone(),
            path.clone(),
            distribution.clone(),
            Some(isolated.path().into()),
        );
        assert!(
            isolated
                .path()
                .join("editor-recent-projects.json")
                .is_file()
        );
        assert_eq!(
            diagnostic.personal_path(WorkspaceKind::Project),
            isolated.path().join("full/project.json")
        );
        diagnostic.project_layout = DockLayout::single(EditorType::History);
        diagnostic.personal_dirty = true;
        diagnostic.save_personal_layouts();
        assert!(isolated.path().join("full/project.json").is_file());
        assert!(!ordinary_layout.exists());
        assert_eq!(std::fs::read(ordinary_recent).unwrap(), recent_before);
        assert_eq!(diagnostic.document, document);
        let actual = diagnostic.distribution.as_ref().unwrap();
        assert_eq!(actual.package_root, distribution.package_root);
        assert_eq!(actual.profile, distribution.profile);
        assert_eq!(actual.can_author_engine(), distribution.can_author_engine());
        assert_eq!(actual.compile_root().unwrap(), compile_root);
    }

    #[test]
    fn editor_personal_layouts_are_isolated_and_explicit_save_load_still_roundtrips() {
        let document = demo_document(WorldKind::Objects);
        let sources = full_source_fixture(&document);
        let root = sources.path();
        let path = root.join("unused-personal-settings-isolation.json");
        let mut first = Editor::new(document.clone(), path.clone());
        let mut second = Editor::new(document.clone(), path);
        let distribution = first.distribution.as_ref().unwrap();
        assert!(distribution.can_author_engine());
        assert_eq!(distribution.package_root, root);
        let package_root = distribution.package_root.clone();
        let compile_root = distribution.compile_root().unwrap();
        let production_path = distribution.personal_layout_path("project");
        assert_eq!(
            production_path,
            package_root.join(".user/full/project.json")
        );
        let first_path = first.personal_path(WorkspaceKind::Project);
        let second_path = second.personal_path(WorkspaceKind::Project);
        assert!(first_path.starts_with(first.test_personal_settings.path()));
        assert!(second_path.starts_with(second.test_personal_settings.path()));
        assert_ne!(first_path, production_path);
        assert_ne!(first_path, second_path);
        first.project_layout = DockLayout::single(EditorType::RustCustom);
        first.project_panes.insert(
            1,
            PaneMemory::new(
                PaneSnapshot {
                    graph_pan: [123., 456.],
                    graph_zoom: 0.61,
                    source: "speed".into(),
                    ..Default::default()
                },
                &document,
            ),
        );
        first.personal_dirty = true;
        first.save_personal_layouts();
        assert!(!first.personal_dirty);
        assert!(first_path.is_file());
        assert!(!second_path.exists());
        first.project_layout = DockLayout::default_for(WorkspaceKind::Project);
        first.project_panes.clear();
        first.load_personal_layouts();
        assert_eq!(
            first.project_layout,
            DockLayout::single(EditorType::RustCustom)
        );
        let restored = first.project_panes.get_mut(&1).unwrap().snapshot(&document);
        assert_eq!(restored.graph_pan, [123., 456.]);
        assert_eq!(restored.graph_zoom, 0.61);
        second.load_personal_layouts();
        assert_eq!(
            second.project_layout,
            DockLayout::default_for(WorkspaceKind::Project)
        );
        assert_eq!(first.document, document);
        assert_eq!(
            first.distribution.as_ref().unwrap().package_root,
            package_root
        );
        assert_eq!(
            first.distribution.as_ref().unwrap().compile_root().unwrap(),
            compile_root
        );
    }

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
    fn rust_toolbar_remains_separate_from_inspector_in_narrow_panes() {
        let document = crate::game_host::demo_document();
        let (_sources, mut editor) = super::super::tests::isolated_editor(document);
        editor.preview_builder = PreviewBuilder::new();
        editor.playing = false;
        editor.current_source = "game-controller".into();
        editor.custom_view = CustomView::Both;
        editor.parse_custom();
        editor.game_workbench.set_deferred_preparation(true);
        assert!(editor.command_program.is_some());
        let before = editor.document.clone();
        for width in [440., 560., 800.] {
            let ctx = egui::Context::default();
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(width, 600.),
                    )),
                    ..Default::default()
                },
                |ui| editor.rust_area(ui),
            );
            let text_rect = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => {
                            Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing text at pane width {width}: {label}"))
            };
            let help = text_rect(
                "Select a command parameter: number, string, or flag. A visual edit changes Rust and starts candidate compilation.",
            );
            for label in [
                "Custom / Rust",
                "game-controller",
                "Text",
                "Graph",
                "Side by side",
            ] {
                let control = text_rect(label);
                assert!(
                    control.right() <= help.left(),
                    "{label} overlaps the inspector column at pane width {width}: {control:?}, {help:?}"
                );
            }
            output.drop_without_applying_deltas();
        }
        assert_eq!(editor.document, before);
    }

    #[test]
    fn viewport_to_rust_editor_parses_shared_source_without_a_text_change() {
        let document = module_engine_core::demo_document(WorldKind::Objects);
        let (_sources, mut editor) = super::super::tests::isolated_editor(document);
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
        crate::engine_profiles::finish_template(
            &mut document,
            crate::engine_profiles::ProfileKind::Neutral,
            &crate::project::source_root(),
        )
        .unwrap();
        let (_sources, mut editor) = super::super::tests::isolated_editor(document);
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
