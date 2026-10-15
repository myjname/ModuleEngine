//! Shared game composition used by the scene editor and standalone Player.
use crate::activity::{ActivitySnapshot, BuildProgress};
use module_engine_content::ContentLibrary;
use module_engine_core::*;
use module_engine_script::{CompiledScript, ScriptCompiler, ScriptKind, ScriptTick};
use module_engine_vulkan::{RenderGraphValidator, RenderPath, RendererPerformance, VulkanRenderer};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

pub type ScriptSlot = Arc<Mutex<CompiledScript>>;

struct RenderPreparation {
    document: GraphDocument,
    receiver: mpsc::Receiver<Result<VulkanRenderer, String>>,
    progress: BuildProgress,
}

/// Only Vulkan passes/sources and their exposure/Project controls may change.
/// World connections, feature modules, scenes, Rust and assets remain strict.
pub fn graphics_only_change(previous: &GraphDocument, document: &GraphDocument) -> bool {
    let mut normalized = document.clone();
    normalized.shader_sources = previous.shader_sources.clone();
    normalized.engine.exposure = previous.engine.exposure.clone();
    normalized.project = previous.project.clone();
    for (node, old) in normalized.root.nodes.iter_mut().zip(&previous.root.nodes) {
        if node.module == "render.vulkan" && old.module == "render.vulkan" {
            node.subgraph = old.subgraph.clone();
        }
    }
    previous.execution_eq(&normalized)
}
pub fn engine_structure_eq(previous: &Graph, graph: &Graph) -> bool {
    let mut normalized = graph.clone();
    for (node, old) in normalized.nodes.iter_mut().zip(&previous.nodes) {
        if node.module == "render.vulkan" && old.module == "render.vulkan" {
            node.subgraph = old.subgraph.clone();
        }
    }
    previous.execution_eq(&normalized)
}

fn retained_render_controls(
    previous: &Graph,
    graph: &Graph,
    controls: &RenderOverrides,
) -> RenderOverrides {
    let (old_passes, old_resources) = module_engine_vulkan::render_graph_targets(previous);
    let (passes, resources) = module_engine_vulkan::render_graph_targets(graph);
    let removed_pass = |id: &str| old_passes.contains(id) && !passes.contains(id);
    let removed_resource = |id: &str| old_resources.contains(id) && !resources.contains(id);
    let mut retained = controls.clone();
    retained.parameters.retain(|id, _| !removed_pass(id));
    retained.enabled.retain(|id, _| !removed_pass(id));
    retained.resource_commands.retain(|command| match command {
        RenderResourceCommand::ClearResource { name, .. } => !removed_resource(name),
        RenderResourceCommand::CopyResource { from, to } => {
            !removed_resource(from) && !removed_resource(to)
        }
    });
    retained
}

fn effective_render_controls(
    document: &GraphDocument,
    time: f64,
    controls: &RenderOverrides,
) -> Result<RenderOverrides, String> {
    let commands = project_commands(document, time)?
        .into_iter()
        .filter_map(|command| match command {
            ControlCommand::Render(command) => Some(command),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut effective = RenderOverrides::default();
    effective.apply_commands(&commands)?;
    for (pass, parameters) in &controls.parameters {
        effective
            .parameters
            .entry(pass.clone())
            .or_default()
            .extend(parameters.clone());
    }
    effective.enabled.extend(controls.enabled.clone());
    effective
        .resource_commands
        .extend(controls.resource_commands.clone());
    Ok(effective)
}

/// Validate every advertised ready preset, including unselected alternatives,
/// and the current Project settings without compiling shaders or touching GPU.
pub fn validate_render_settings_at(
    document: &GraphDocument,
    graph: &Graph,
    _root: &Path,
) -> Result<(), String> {
    let validator = RenderGraphValidator::new(graph)?;
    validate_render_settings(document, &validator)
}
fn validate_render_settings(
    document: &GraphDocument,
    validator: &RenderGraphValidator,
) -> Result<(), String> {
    let current = effective_render_controls(document, 0., &RenderOverrides::default())?;
    validator.validate(&current)?;
    for group in document.engine.exposure.variants.values() {
        for variant in group.options.values() {
            let mut controls = RenderOverrides::default();
            controls.apply_commands(&variant.commands)?;
            validator.validate(&controls)?;
        }
    }
    Ok(())
}

fn validate_candidate_controls(
    document: &GraphDocument,
    graph: &Graph,
    controls: &RenderOverrides,
    time: f64,
    _root: &Path,
) -> Result<(), String> {
    let validator = RenderGraphValidator::new(graph)?;
    validate_render_settings(document, &validator)?;
    let effective = effective_render_controls(document, time, controls)?;
    validator.validate(&effective)
}

struct RustSystem {
    slot: ScriptSlot,
    document: Arc<Mutex<GraphDocument>>,
    root: PathBuf,
    backend_root: PathBuf,
    overrides: Arc<Mutex<RenderOverrides>>,
    pending: Option<ScriptTick>,
    checkpoint: Option<(Value, RenderOverrides)>,
}
impl ExecutableSystem for RustSystem {
    fn initialize(
        &mut self,
        _ctx: &mut SystemContext<'_>,
        state: &mut Value,
    ) -> Result<(), String> {
        let mut script = self.slot.lock().map_err(|_| "Custom slot poisoned")?;
        if script.asset_root() != self.root {
            *script = script
                .clone()
                .with_asset_root(&self.root)
                .map_err(|e| e.to_string())?;
        }
        script.reset_state().map_err(|e| e.to_string())?;
        *state = script.state_declaration().initial.clone();
        Ok(())
    }
    fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
        self.pending = None;
        self.checkpoint = None;
        let document = self
            .document
            .lock()
            .map_err(|_| "Game configuration poisoned")?
            .clone();
        let objects: Vec<_> = ctx
            .game
            .entities
            .values()
            .map(|e| e.object.clone())
            .collect();
        let mut input = ScriptContext::new(ctx.game.time, ctx.dt as f64, objects.clone());
        input.asset_paths =
            module_engine_project::AssetManifest::from_value(&document.asset_manifest)?
                .asset_paths();
        input.input = serde_json::to_value(ctx.input).map_err(|e| e.to_string())?;
        input.events = ctx
            .events
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
            .collect();
        input.parameters = module_engine_core::resolved_parameters(&document);
        for (id, entity) in &ctx.game.entities {
            for (name, value) in &entity.components {
                if let Some(fields) = value.as_object() {
                    input.properties.components.entry(*id).or_default().insert(
                        name.clone(),
                        fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    );
                }
            }
        }
        let script = self
            .slot
            .lock()
            .map_err(|_| "Custom script slot poisoned")?;
        let tick = script
            .tick(&input, Duration::from_millis(150))
            .map_err(|e| e.to_string())?;
        self.pending = Some(tick.clone());
        validate_project_controls(&document, &tick.commands)?;
        let (world, render) = {
            let mut world = vec![];
            let mut render = vec![];
            for c in &tick.commands {
                match c {
                    ControlCommand::World(c) => world.push(c.clone()),
                    ControlCommand::Render(c) => render.push(c.clone()),
                }
            }
            (world, render)
        };
        // Both targets validate before publishing the candidate script state.
        let mut provider = ObjectsWorld::from_objects(objects)?;
        provider.apply_commands(&world)?;
        let mut overrides = self
            .overrides
            .lock()
            .map_err(|_| "Render control slot poisoned")?;
        let mut candidate = overrides.clone();
        candidate.apply_commands(&render)?;
        if !render.is_empty() {
            module_engine_vulkan::validate_overrides_at(
                &render_graph(&document),
                &candidate,
                &self.backend_root,
            )?;
        }
        let checkpoint = script.state().map_err(|e| e.to_string())?;
        let events = tick
            .events
            .iter()
            .map(|v| {
                serde_json::from_value::<GameEvent>(v.clone())
                    .map_err(|e| format!("Invalid emitted GameEvent: {e}"))
            })
            .collect::<Result<Vec<_>, _>>();
        let events = events?;
        let final_objects = provider.objects()?;
        let mut entities = BTreeMap::new();
        for object in final_objects {
            let mut entity = ctx
                .game
                .entities
                .get(&object.id)
                .cloned()
                .unwrap_or(GameEntity {
                    object: object.clone(),
                    components: BTreeMap::new(),
                    references: BTreeMap::new(),
                });
            entity.object = object;
            entities.insert(entity.object.id, entity);
        }
        self.checkpoint = Some((checkpoint, overrides.clone()));
        ctx.game.entities = entities;
        *overrides = candidate;
        *state = tick.state.clone();
        ctx.events.extend(events);
        self.pending = Some(tick);
        Ok(())
    }
    fn validate_tick(&mut self) -> Result<(), String> {
        if let Some(tick) = &self.pending {
            self.slot
                .lock()
                .map_err(|_| "Custom slot poisoned")?
                .validate_tick(tick)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn commit_tick(&mut self) -> Result<(), String> {
        if let Some(tick) = &self.pending {
            self.slot
                .lock()
                .map_err(|_| "Custom slot poisoned")?
                .commit_tick(tick)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn rollback_tick(&mut self) -> Result<(), String> {
        if let Some((state, overrides)) = self.checkpoint.take() {
            self.slot
                .lock()
                .map_err(|_| "Custom slot poisoned")?
                .migrate_state(state)
                .map_err(|e| e.to_string())?;
            *self.overrides.lock().map_err(|_| "Render slot poisoned")? = overrides;
        } else if let Some(tick) = &self.pending {
            self.slot
                .lock()
                .map_err(|_| "Custom slot poisoned")?
                .reject_tick(tick)
                .map_err(|e| e.to_string())?;
        }
        self.pending = None;
        Ok(())
    }
}

pub fn script_ids(game: &GameProject) -> Vec<String> {
    game.execution
        .stages
        .iter()
        .flat_map(|s| &s.nodes)
        .filter(|n| n.module == "custom.rust.update")
        .filter_map(|n| {
            n.params
                .get("source_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

pub fn game_registry(
    document: &GraphDocument,
    root: &Path,
    slots: &BTreeMap<String, ScriptSlot>,
    overrides: Arc<Mutex<RenderOverrides>>,
) -> Result<SystemRegistry, String> {
    game_registry_shared(
        Arc::new(Mutex::new(document.clone())),
        root,
        root,
        slots,
        overrides,
    )
}

/// Project assets and installed renderer tools have independent locations.
pub fn game_registry_at(
    document: &GraphDocument,
    asset_root: &Path,
    backend_root: &Path,
    slots: &BTreeMap<String, ScriptSlot>,
    overrides: Arc<Mutex<RenderOverrides>>,
) -> Result<SystemRegistry, String> {
    if asset_root == backend_root {
        return game_registry(document, asset_root, slots, overrides);
    }
    game_registry_shared(
        Arc::new(Mutex::new(document.clone())),
        asset_root,
        backend_root,
        slots,
        overrides,
    )
}
fn game_registry_shared(
    document: Arc<Mutex<GraphDocument>>,
    root: &Path,
    backend_root: &Path,
    slots: &BTreeMap<String, ScriptSlot>,
    overrides: Arc<Mutex<RenderOverrides>>,
) -> Result<SystemRegistry, String> {
    let mut registry = SystemRegistry::builtin();
    let slots = slots.clone();
    let root = root.to_path_buf();
    let backend_root = backend_root.to_path_buf();
    registry.register(
        SystemDescriptor {
            id: "custom.rust.update".into(),
            state_schema: "owned-rust-state-v1".into(),
            ports: vec![
                SystemPort {
                    name: "world".into(),
                    data_type: "GameWorld".into(),
                    access: ResourceAccess::ReadWrite,
                },
                SystemPort {
                    name: "input".into(),
                    data_type: "GameInput".into(),
                    access: ResourceAccess::Read,
                },
                SystemPort {
                    name: "events".into(),
                    data_type: "GameEvents".into(),
                    access: ResourceAccess::ReadWrite,
                },
            ],
            requires: vec![],
            components: vec![],
        },
        move |params| {
            let id = params
                .get("source_id")
                .and_then(Value::as_str)
                .ok_or("Rust system needs params.source_id")?;
            let instance = params
                .get("__instance_id")
                .and_then(Value::as_str)
                .unwrap_or(id);
            let slot = slots
                .get(instance)
                .ok_or_else(|| format!("No compiled Rust system '{id}' / '{instance}'"))?
                .clone();
            Ok(Box::new(RustSystem {
                slot,
                document: document.clone(),
                root: root.clone(),
                backend_root: backend_root.clone(),
                overrides: overrides.clone(),
                pending: None,
                checkpoint: None,
            }))
        },
    )?;
    Ok(registry)
}

/// Resolve only the package selected and checked by distribution policy. A
/// relocated authored project supplies assets; it never has to contain DXC or
/// installed backend files. Precompiled Player packages own both roots.
pub fn backend_root_for(asset_root: &Path, compile_root: Option<&Path>) -> Result<PathBuf, String> {
    let Some(compile_root) = compile_root else {
        return asset_root.canonicalize().map_err(|e| e.to_string());
    };
    let compile_root = compile_root.canonicalize().map_err(|e| e.to_string())?;
    // A Full source project can itself be named `.sdk`. Native editor sources
    // distinguish it from the intentionally reduced Team Custom SDK.
    if compile_root.join("crates/editor/src/main.rs").is_file() {
        return Ok(compile_root);
    }
    if compile_root.file_name().is_some_and(|name| name == ".sdk")
        && compile_root.join("crates/engine-core/Cargo.toml").is_file()
        && let Some(package) = compile_root.parent()
    {
        let manifest: crate::distribution::PackageManifest = serde_json::from_slice(
            &std::fs::read(package.join(crate::distribution::MANIFEST_NAME))
                .map_err(|e| format!("Selected Team backend package unavailable: {e}"))?,
        )
        .map_err(|e| format!("Selected Team backend manifest: {e}"))?;
        if manifest.profile == crate::distribution::DistributionProfile::Team
            && manifest.engine_version == env!("CARGO_PKG_VERSION")
        {
            return Ok(package.to_path_buf());
        }
    }
    Err(
        "Selected compile root is neither a Full engine source project nor its checked Team SDK"
            .into(),
    )
}

pub fn render_graph(document: &GraphDocument) -> Graph {
    let selected = document
        .root
        .nodes
        .iter()
        .find(|n| n.module == "viewport.output")
        .and_then(|output| {
            document
                .root
                .edges
                .iter()
                .find(|e| e.to_node == output.id && e.to_port == "frame")
        })
        .and_then(|e| document.root.nodes.iter().find(|n| n.id == e.from_node));
    if let Some(node) = selected.filter(|n| n.module == "render.vulkan")
        && let Some(graph) = node.subgraph.as_deref()
    {
        return graph.clone();
    }
    document
        .root
        .nodes
        .iter()
        .find(|n| n.module == "render.vulkan")
        .and_then(|n| n.subgraph.as_deref())
        .cloned()
        .unwrap_or_else(|| module_engine_vulkan::default_graph(RenderPath::Forward))
}

pub struct GameHost {
    pub session: GameSession,
    pub content: ContentLibrary,
    pub camera: Camera,
    pub slots: BTreeMap<String, ScriptSlot>,
    pub script_sources: BTreeMap<String, String>,
    pub last_step_ms: f64,
    pub last_render_ms: f64,
    pub device: String,
    pub root: PathBuf,
    pub backend_root: PathBuf,
    document: GraphDocument,
    live_document: Arc<Mutex<GraphDocument>>,
    graph: Graph,
    renderer: VulkanRenderer,
    rejected_render: Option<(GraphDocument, String)>,
    overrides: Arc<Mutex<RenderOverrides>>,
    simulation_started: bool,
    pub audio: AudioModule,
    render_preparation: Option<RenderPreparation>,
    render_activity: Option<BuildProgress>,
    rejected_preparation: Option<(GraphDocument, String)>,
    presentation_revision: u64,
}
fn next_presentation_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
impl GameHost {
    /// Unique host/presentation identity. Cosmetic graph edits do not invalidate
    /// the displayed frame; accepted world/GPU changes and simulation do.
    pub fn presentation_revision(&self) -> u64 {
        self.presentation_revision
    }
    pub fn renderer_performance(&self) -> RendererPerformance {
        self.renderer.performance()
    }
    pub fn invalidate_content(&mut self) {
        self.content.invalidate();
        self.presentation_revision = next_presentation_revision();
    }
    pub fn set_detailed_stats(&mut self, enabled: bool) {
        self.renderer.set_detailed_stats(enabled);
    }
    pub fn new(
        document: GraphDocument,
        root: &Path,
        compile_root: Option<&Path>,
        kind: WorldKind,
    ) -> Result<Self, String> {
        let backend_root = backend_root_for(root, compile_root)?;
        Self::new_at(document, root, &backend_root, compile_root, kind)
    }

    /// Explicit composition for an authored project using a checked installed
    /// backend even when native Custom compilation is unavailable.
    pub fn new_at(
        document: GraphDocument,
        root: &Path,
        backend_root: &Path,
        compile_root: Option<&Path>,
        kind: WorldKind,
    ) -> Result<Self, String> {
        Self::new_at_with_progress(document, root, backend_root, compile_root, kind, None)
    }
    pub fn new_at_with_progress(
        document: GraphDocument,
        root: &Path,
        backend_root: &Path,
        compile_root: Option<&Path>,
        kind: WorldKind,
        progress: Option<&BuildProgress>,
    ) -> Result<Self, String> {
        if let Some(progress) = progress {
            progress.stage("Проверка модулей и игровой сцены");
        }
        let mut game = document
            .game
            .clone()
            .ok_or("Project has no authored game")?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let backend_root = backend_root.canonicalize().map_err(|e| e.to_string())?;
        let registry = crate::host::registry_at(&backend_root)?;
        let errors = validate_graph(&document, &registry)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message)
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        validate_render_settings_at(&document, &render_graph(&document), &backend_root)?;
        let outputs = document
            .root
            .nodes
            .iter()
            .filter(|n| n.module == "viewport.output")
            .collect::<Vec<_>>();
        if outputs.len() != 1 {
            return Err("Game blueprint requires one viewport.output".into());
        }
        let edge = document
            .root
            .edges
            .iter()
            .find(|e| e.to_node == outputs[0].id && e.to_port == "frame")
            .ok_or("Game viewport has no renderer")?;
        let render = document
            .root
            .nodes
            .iter()
            .find(|n| n.id == edge.from_node)
            .ok_or("Game renderer is missing")?;
        if render.module != "render.vulkan" || render.subgraph.is_none() {
            return Err("This Player requires the installed Vulkan renderer graph".into());
        }
        if !render.params.is_null() && !render.params.as_object().is_some_and(|p| p.is_empty()) {
            return Err("Vulkan outer parameters must be empty; edit pass parameters".into());
        }
        let mut sources = BTreeMap::new();
        if let Some(progress) = progress {
            progress.stage("Подготовка Rust Custom");
        }
        for id in script_ids(&game) {
            if sources.contains_key(&id) {
                continue;
            }
            let script = if let Some(compile_root) = compile_root {
                let source = document
                    .custom_sources
                    .get(&id)
                    .ok_or_else(|| format!("Missing game Custom '{id}'"))?;
                ScriptCompiler::new(
                    compile_root.to_path_buf(),
                    compile_root.join(".work/game-scripts"),
                )
                .compile_kind(source, ScriptKind::Update)
                .map_err(|e| e.to_string())?
            } else {
                if id.is_empty()
                    || !id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err("Invalid precompiled script ID".into());
                }
                CompiledScript::load_precompiled(
                    &root.join("scripts").join(&id).join("script.json"),
                    &root,
                )
                .map_err(|e| e.to_string())?
            };
            let script = script.with_asset_root(&root).map_err(|e| e.to_string())?;
            sources.insert(id, script);
        }
        let mut slots = BTreeMap::new();
        let mut script_sources = BTreeMap::new();
        for node in game
            .execution
            .stages
            .iter_mut()
            .flat_map(|s| &mut s.nodes)
            .filter(|n| n.module == "custom.rust.update")
        {
            let id = node
                .params
                .get("source_id")
                .and_then(Value::as_str)
                .ok_or("Custom node has no source_id")?
                .to_owned();
            slots.insert(
                node.id.clone(),
                Arc::new(Mutex::new(sources[&id].fork_instance())),
            );
            script_sources.insert(node.id.clone(), id);
            node.params["__instance_id"] = json!(node.id);
        }
        let overrides = Arc::new(Mutex::new(RenderOverrides::default()));
        let live_document = Arc::new(Mutex::new(document.clone()));
        let registry = game_registry_shared(
            live_document.clone(),
            &root,
            &backend_root,
            &slots,
            overrides.clone(),
        )?;
        let session = GameSession::new_with_registry(game.clone(), kind, registry)?;
        let graph = render_graph(&document);
        if let Some(progress) = progress {
            progress.stage("Подготовка Vulkan и шейдеров");
        }
        let mut renderer = VulkanRenderer::new_at(backend_root.clone())?;
        renderer.prepare(&graph, &document.shader_sources)?;
        renderer.set_detailed_stats(false);
        let device = renderer.device_name().to_owned();
        if let Some(progress) = progress {
            progress.stage("Загрузка моделей, текстур и звука");
        }
        let mut content = ContentLibrary::new(&root)?;
        let assets = module_engine_project::AssetManifest::from_value(&document.asset_manifest)?;
        content.set_asset_paths(assets.asset_paths());
        for scene in game.scenes.keys() {
            let resolved = game.resolve_scene(scene)?;
            for e in resolved {
                if let Some(path) = e
                    .components
                    .get("mesh")
                    .and_then(|v| v.get("path"))
                    .and_then(Value::as_str)
                {
                    content.load_model(path)?;
                }
            }
        }
        let audio_active = game
            .execution
            .stages
            .iter()
            .flat_map(|s| &s.nodes)
            .any(|n| n.module == "game.audio");
        let empty_cues = BTreeMap::new();
        let resolved_cues: BTreeMap<String, String> = game
            .audio_cues
            .iter()
            .map(|(cue, reference)| {
                assets
                    .resolve_reference(reference)
                    .map(|path| (cue.clone(), path))
            })
            .collect::<Result<_, _>>()?;
        let audio = AudioModule::new(
            &root,
            if audio_active {
                &resolved_cues
            } else {
                &empty_cues
            },
        )?;
        Ok(Self {
            session,
            content,
            camera: Camera {
                yaw: 0.,
                pitch: 0.95,
                distance: 21.,
                target: [0., 0., 0.],
            },
            slots,
            script_sources,
            last_step_ms: 0.,
            last_render_ms: 0.,
            device,
            root,
            backend_root,
            document,
            live_document,
            graph,
            renderer,
            rejected_render: None,
            overrides,
            simulation_started: false,
            audio,
            render_preparation: None,
            render_activity: None,
            rejected_preparation: None,
            presentation_revision: next_presentation_revision(),
        })
    }
    /// Replace only the authored preview, retaining GPU resources, decoded
    /// content, compiled Custom slots and camera. This is deliberately separate
    /// from a Play update: the caller must stay outside its Play session and
    /// supply the selected preview scene in `game.entry_scene`.
    ///
    /// Engine/execution changes require the existing explicit rebuild/apply
    /// path. A scene/resource validation error keeps the last accepted session
    /// and document; loading a candidate may merely warm the content cache.
    pub fn refresh_author_preview(&mut self, document: &GraphDocument) -> Result<(), String> {
        if self.simulation_started
            || self.session.state().tick != 0
            || self.session.state().time != 0.
        {
            return Err("Authored preview refresh is unavailable during Play; stop first".into());
        }
        if document.execution_eq(&self.document) {
            // Graph gestures update the saved/live source without reconstructing
            // systems, decoding content or disturbing accepted Custom state.
            return self.sync_document(document);
        }
        let previous = self.document.game.as_ref().ok_or("No authored preview")?;
        let mut game = document.game.clone().ok_or("No authored game project")?;
        if !document.root.execution_eq(&self.document.root)
            || document.shader_sources != self.document.shader_sources
            || game.execution != previous.execution
        {
            return Err(
                "Engine or execution structure changed; rebuild the preview explicitly".into(),
            );
        }
        for id in self.script_sources.values() {
            if self.document.custom_sources.contains_key(id)
                && !document.custom_sources.contains_key(id)
            {
                return Err(format!(
                    "Connected Custom source '{id}' was removed; rebuild explicitly"
                ));
            }
        }
        let registry = crate::host::registry_at(&self.backend_root)?;
        let errors = validate_graph(document, &registry)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message)
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        // Transform/resource edits cannot change render contracts. Revalidating
        // every preset on every gizmo sample used to duplicate this work.
        if document.engine.exposure != self.document.engine.exposure
            || document.project != self.document.project
        {
            let controls = self
                .overrides
                .lock()
                .map_err(|_| "Render controls poisoned")?
                .clone();
            validate_candidate_controls(document, &self.graph, &controls, 0., &self.backend_root)?;
        }
        let assets = module_engine_project::AssetManifest::from_value(&document.asset_manifest)?;
        let old_assets =
            module_engine_project::AssetManifest::from_value(&self.document.asset_manifest)?;
        let mut staged_content = if assets.asset_paths() != old_assets.asset_paths() {
            let mut content = ContentLibrary::new(&self.root)?;
            content.set_asset_paths(assets.asset_paths());
            Some(content)
        } else {
            None
        };
        let content = staged_content.as_mut().unwrap_or(&mut self.content);
        for scene in game.scenes.keys() {
            for entity in game.resolve_scene(scene)? {
                if let Some(path) = entity
                    .components
                    .get("mesh")
                    .and_then(|value| value.get("path"))
                    .and_then(Value::as_str)
                {
                    content.load_model(path)?;
                }
                if let Some(material) = entity.components.get("material") {
                    if let Some(color) = material.get("base_color") {
                        serde_json::from_value::<[f32; 4]>(color.clone())
                            .map_err(|error| format!("Material color: {error}"))?;
                    }
                    if let Some(path) = material.get("base_color_texture").and_then(Value::as_str) {
                        content.load_texture(path)?;
                    }
                }
            }
        }
        let audio = if game.audio_cues != previous.audio_cues
            || assets.asset_paths() != old_assets.asset_paths()
        {
            let empty = BTreeMap::new();
            let active = game
                .execution
                .stages
                .iter()
                .flat_map(|stage| &stage.nodes)
                .any(|node| node.module == "game.audio");
            let cues: BTreeMap<String, String> = game
                .audio_cues
                .iter()
                .map(|(cue, reference)| {
                    assets
                        .resolve_reference(reference)
                        .map(|path| (cue.clone(), path))
                })
                .collect::<Result<_, _>>()?;
            let mut audio = AudioModule::new(&self.root, if active { &cues } else { &empty })?;
            // A rejected candidate must not stop the current audio endpoint
            // through AudioModule::drop while it is still being validated.
            audio.enabled = false;
            audio.played = self.audio.played;
            Some(audio)
        } else {
            None
        };
        for node in game
            .execution
            .stages
            .iter_mut()
            .flat_map(|stage| &mut stage.nodes)
            .filter(|node| node.module == "custom.rust.update")
        {
            node.params["__instance_id"] = json!(node.id);
        }
        let registry = game_registry_shared(
            self.live_document.clone(),
            &self.root,
            &self.backend_root,
            &self.slots,
            self.overrides.clone(),
        )?;
        // System construction does not initialize or execute Custom. Existing
        // accepted code/state remain in the retained slots until explicit Play.
        let session = GameSession::new_with_registry(game, self.session.world_kind(), registry)?;
        let mut live = self
            .live_document
            .lock()
            .map_err(|_| "Game configuration poisoned")?;
        self.session = session;
        self.document = document.clone();
        *live = document.clone();
        if let Some(content) = staged_content {
            self.content = content;
        }
        if let Some(mut audio) = audio {
            audio.enabled = self.audio.enabled;
            self.audio = audio;
        }
        self.presentation_revision = next_presentation_revision();
        Ok(())
    }
    pub fn step(&mut self, dt: f64, input: &GameInput) -> Result<usize, String> {
        self.simulation_started = true;
        let started = Instant::now();
        self.overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?
            .begin_frame();
        let ticks = self.session.step(dt, input)?;
        self.presentation_revision = next_presentation_revision();
        self.last_step_ms = started.elapsed().as_secs_f64() * 1000.;
        self.audio.consume(self.session.events())?;
        if let Some(player) = self
            .session
            .state()
            .entities
            .values()
            .find(|e| e.components.contains_key("player"))
        {
            self.camera.target = player.object.transform.translation;
        }
        Ok(ticks)
    }
    /// The exact renderer-neutral geometry/materials displayed by Scene and Game.
    /// Imported primitives retain their authored object IDs for picking.
    pub fn author_snapshot(&mut self) -> Result<SceneSnapshot, String> {
        let mut snapshot = self.session.snapshot();
        self.content
            .enrich_snapshot(&mut snapshot, &self.session.asset_bindings())?;
        for item in &mut snapshot.items {
            if let Some(material) = self
                .session
                .state()
                .entities
                .get(&item.id)
                .and_then(|entity| entity.components.get("material"))
            {
                if let Some(color) = material.get("base_color") {
                    item.material.base_color = serde_json::from_value(color.clone())
                        .map_err(|error| format!("Material color: {error}"))?;
                }
                if let Some(path) = material.get("base_color_texture").and_then(Value::as_str) {
                    item.material.base_color_texture = Some(self.content.load_texture(path)?);
                }
            }
        }
        crate::combat_visuals::apply_hit_flashes(&mut snapshot, self.session.state());
        Ok(snapshot)
    }
    pub fn render(&mut self, width: usize, height: usize) -> Result<Frame, String> {
        let started = Instant::now();
        let snapshot = self.author_snapshot()?;
        let controls = self
            .overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?
            .clone();
        let overrides =
            effective_render_controls(&self.document, self.session.state().time, &controls)?;
        let frame = self.renderer.render(
            &snapshot,
            self.camera,
            width,
            height,
            &self.graph,
            &self.document.shader_sources,
            &overrides,
        )?;
        self.last_render_ms = started.elapsed().as_secs_f64() * 1000.;
        Ok(frame)
    }
    pub fn apply_candidate(
        &mut self,
        id: &str,
        candidate: CompiledScript,
        reset: bool,
    ) -> Result<(), String> {
        let mut replacements = vec![];
        for (instance, source) in &self.script_sources {
            if source == id {
                let slot = &self.slots[instance];
                let active = slot.lock().map_err(|_| "Custom slot poisoned")?;
                let replacement = candidate
                    .fork_instance()
                    .with_asset_root(&self.root)
                    .map_err(|e| e.to_string())?;
                if !reset {
                    replacement
                        .inherit_state_from(&active)
                        .map_err(|e| e.to_string())?;
                }
                replacements.push((slot.clone(), replacement));
            }
        }
        if replacements.is_empty() {
            return Err("Custom is not connected to the game graph".into());
        }
        for (slot, replacement) in replacements {
            *slot.lock().map_err(|_| "Custom slot poisoned")? = replacement;
        }
        Ok(())
    }
    pub fn apply_candidate_migrated(
        &mut self,
        id: &str,
        candidate: &CompiledScript,
        state: Value,
    ) -> Result<(), String> {
        let mut replacements = vec![];
        for (instance, source) in &self.script_sources {
            if source == id {
                let replacement = candidate
                    .fork_instance()
                    .with_asset_root(&self.root)
                    .map_err(|e| e.to_string())?;
                replacement
                    .migrate_state(state.clone())
                    .map_err(|e| e.to_string())?;
                replacements.push((self.slots[instance].clone(), replacement));
            }
        }
        if replacements.is_empty() {
            return Err("Custom is not connected".into());
        }
        for (slot, replacement) in replacements {
            *slot.lock().map_err(|_| "Custom slot poisoned")? = replacement;
        }
        Ok(())
    }
    /// Stage graphics/settings while retaining live scene and Custom state.
    pub fn sync_document(&mut self, document: &GraphDocument) -> Result<(), String> {
        if document == &self.document {
            return Ok(());
        }
        if document.execution_eq(&self.document) {
            return self.accept_document(document);
        }
        let graph = render_graph(document);
        if let Some((failed, error)) = &self.rejected_render
            && failed.execution_eq(document)
        {
            return Err(error.clone());
        }
        let current = self
            .overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?
            .clone();
        let controls = retained_render_controls(&self.graph, &graph, &current);
        let result = validate_candidate_controls(
            document,
            &graph,
            &controls,
            self.session.state().time,
            &self.backend_root,
        )
        .and_then(|_| {
            if !graph.execution_eq(&self.graph)
                || document.shader_sources != self.document.shader_sources
            {
                self.renderer.prepare(&graph, &document.shader_sources)
            } else {
                Ok(())
            }
        });
        if let Err(error) = result {
            self.rejected_render = Some((document.clone(), error.clone()));
            return Err(error);
        }
        self.accept_render_document(document, controls)?;
        self.graph = graph;
        self.rejected_render = None;
        Ok(())
    }
    pub fn preparation_activity(&self) -> Option<ActivitySnapshot> {
        self.render_activity.as_ref().map(BuildProgress::snapshot)
    }
    pub fn has_pending_preparation(&self) -> bool {
        self.render_preparation.is_some()
    }
    pub fn cancel_preparation(&mut self) {
        if let Some(job) = self.render_preparation.take() {
            job.progress
                .succeed("Запрос заменён; текущий рендер сохранён");
            // A completed channel may already own a GPU device. Its destruction
            // waits for Vulkan and must never run on the UI thread.
            std::thread::spawn(move || drop(job));
        }
    }
    /// Editor-only asynchronous alternative; CLI keeps synchronous sync_document.
    /// The old renderer/document remain live until the exact latest target wins.
    pub fn sync_document_deferred(&mut self, document: &GraphDocument) -> Result<(), String> {
        if self
            .rejected_preparation
            .as_ref()
            .is_some_and(|(failed, _)| !failed.execution_eq(document))
        {
            self.rejected_preparation = None;
            self.render_activity = None;
        }
        self.poll_preparation(document)?;
        if document == &self.document {
            return Ok(());
        }
        if document.execution_eq(&self.document) {
            self.cancel_preparation();
            return self.accept_document(document);
        }
        let graph = render_graph(document);
        if graph.execution_eq(&self.graph)
            && document.shader_sources == self.document.shader_sources
        {
            self.cancel_preparation();
            return self.sync_document(document);
        }
        if let Some((failed, error)) = &self.rejected_preparation
            && failed.execution_eq(document)
        {
            return Err(error.clone());
        }
        if self.render_preparation.is_some() {
            return Ok(());
        }
        let candidate = document.clone();
        let root = self.backend_root.clone();
        let sources = candidate.shader_sources.clone();
        let current = self
            .overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?
            .clone();
        let controls = retained_render_controls(&self.graph, &graph, &current);
        let worker_document = candidate.clone();
        let time = self.session.state().time;
        let progress = BuildProgress::new("Рендер Game / Scene");
        let worker_progress = progress.clone();
        let (send, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            worker_progress.stage("Подготовка Vulkan и шейдеров; текущий рендер работает");
            let result = (|| {
                validate_candidate_controls(&worker_document, &graph, &controls, time, &root)?;
                let mut renderer = VulkanRenderer::new_at(root.clone())?;
                renderer.prepare(&graph, &sources)?;
                renderer.set_detailed_stats(false);
                Ok(renderer)
            })();
            let _ = send.send(result);
        });
        self.render_activity = Some(progress.clone());
        self.render_preparation = Some(RenderPreparation {
            document: candidate,
            receiver,
            progress,
        });
        self.rejected_preparation = None;
        Ok(())
    }
    pub fn poll_preparation(&mut self, document: &GraphDocument) -> Result<(), String> {
        if self
            .rejected_preparation
            .as_ref()
            .is_some_and(|(failed, _)| !failed.execution_eq(document))
        {
            self.rejected_preparation = None;
            self.render_activity = None;
        }
        let Some(job) = &self.render_preparation else {
            return Ok(());
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return Ok(()),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Поток подготовки Vulkan завершился без результата".into())
            }
        };
        let job = self.render_preparation.take().unwrap();
        if !job.document.execution_eq(document) {
            job.progress
                .succeed("Устаревший запрос заменён; текущий рендер сохранён");
            std::thread::spawn(move || drop(result));
            return Ok(());
        }
        let candidate = result.and_then(|candidate| {
            let graph = render_graph(document);
            let controls = self
                .overrides
                .lock()
                .map_err(|_| "Render controls poisoned".to_owned())
                .map(|controls| retained_render_controls(&self.graph, &graph, &controls));
            let controls = match controls {
                Ok(controls) => controls,
                Err(error) => {
                    std::thread::spawn(move || drop(candidate));
                    return Err(error);
                }
            };
            let validation = validate_candidate_controls(
                document,
                &graph,
                &controls,
                self.session.state().time,
                &self.backend_root,
            );
            if let Err(error) = validation {
                std::thread::spawn(move || drop(candidate));
                return Err(error);
            }
            // Acquire the configuration lock before committing GPU state.
            if let Err(error) = self.accept_render_document(document, controls) {
                std::thread::spawn(move || drop(candidate));
                return Err(error);
            }
            Ok(candidate)
        });
        match candidate {
            Ok(candidate) => {
                self.device = candidate.device_name().to_owned();
                let old = std::mem::replace(&mut self.renderer, candidate);
                self.graph = render_graph(document);
                self.rejected_render = None;
                self.rejected_preparation = None;
                job.progress
                    .succeed("Новый рендер применён; игровое состояние сохранено");
                std::thread::spawn(move || drop(old));
                Ok(())
            }
            Err(error) => {
                job.progress.fail(error.clone());
                self.rejected_preparation = Some((document.clone(), error.clone()));
                Err(error)
            }
        }
    }
    pub fn accepted_document(&self) -> &GraphDocument {
        &self.document
    }
    fn accept_render_document(
        &mut self,
        document: &GraphDocument,
        controls: RenderOverrides,
    ) -> Result<(), String> {
        let mut live = self
            .live_document
            .lock()
            .map_err(|_| "Game configuration poisoned")?;
        let mut overrides = self
            .overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?;
        self.document = document.clone();
        *live = document.clone();
        *overrides = controls;
        self.presentation_revision = next_presentation_revision();
        Ok(())
    }
    fn accept_document(&mut self, document: &GraphDocument) -> Result<(), String> {
        let mut live = self
            .live_document
            .lock()
            .map_err(|_| "Game configuration poisoned")?;
        self.document = document.clone();
        *live = document.clone();
        Ok(())
    }
}

/// Basic optional single-voice Windows audio endpoint. Scheduling belongs to game.audio.
pub struct AudioModule {
    cues: BTreeMap<String, Vec<u16>>,
    pub played: u64,
    pub enabled: bool,
}
impl AudioModule {
    /// Retire this owner before its GPU/device is destroyed on another thread.
    /// The process-wide WAV endpoint must not later stop a new host's sound.
    pub fn stop_endpoint(&mut self) {
        #[cfg(windows)]
        if self.enabled {
            unsafe {
                PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0);
            }
        }
        self.enabled = false;
    }
    pub fn new(root: &Path, cues: &BTreeMap<String, String>) -> Result<Self, String> {
        let mut paths = BTreeMap::new();
        for (cue, path) in cues {
            let file = module_engine_project::resolve_relative(root, path)?;
            let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
            if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
                return Err(format!("Audio cue '{cue}' needs a WAV file"));
            }
            paths.insert(
                cue.clone(),
                file.to_string_lossy()
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect(),
            );
        }
        Ok(Self {
            cues: paths,
            played: 0,
            enabled: true,
        })
    }
    pub fn consume(&mut self, events: &[GameEvent]) -> Result<(), String> {
        for event in events {
            if let GameEvent::Sound { cue, .. } = event
                && let Some(path) = self.cues.get(cue)
            {
                self.played += 1;
                #[cfg(windows)]
                if self.enabled {
                    unsafe {
                        PlaySoundW(
                            path.as_ptr(),
                            std::ptr::null_mut(),
                            0x00020000 | 0x0001 | 0x0002,
                        );
                    }
                }
                #[cfg(not(windows))]
                let _ = path;
            }
        }
        Ok(())
    }
}
#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32;
}
impl Drop for AudioModule {
    fn drop(&mut self) {
        self.stop_endpoint();
    }
}

/// Cursor ray intersection with the horizontal gameplay plane. Uses the same camera/FOV as Vulkan.
pub fn aim_at(camera: Camera, normalized: [f32; 2], aspect: f32) -> [f32; 3] {
    fn norm(a: [f32; 3]) -> [f32; 3] {
        let l = a.iter().map(|v| v * v).sum::<f32>().sqrt().max(0.000001);
        a.map(|v| v / l)
    }
    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }
    let [sy, cy] = [camera.yaw.sin(), camera.yaw.cos()];
    let [sp, cp] = [camera.pitch.sin(), camera.pitch.cos()];
    let eye = [
        camera.target[0] + sy * cp * camera.distance,
        camera.target[1] + sp * camera.distance,
        camera.target[2] + cy * cp * camera.distance,
    ];
    let forward = norm(std::array::from_fn(|i| camera.target[i] - eye[i]));
    let right = norm(cross(forward, [0., 1., 0.]));
    let up = cross(right, forward);
    let tangent = (55_f32.to_radians() * 0.5).tan();
    let ray: [f32; 3] = std::array::from_fn(|i| {
        forward[i] + right[i] * normalized[0] * aspect * tangent + up[i] * normalized[1] * tangent
    });
    if ray[1].abs() < 0.00001 {
        return camera.target;
    }
    let t = -eye[1] / ray[1];
    if t <= 0. {
        return camera.target;
    }
    [eye[0] + ray[0] * t, 0., eye[2] + ray[2] * t]
}

pub fn demo_document() -> GraphDocument {
    let mut document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward);
    crate::host::ensure_authoring_configuration(&mut document);
    document.root.nodes.retain(|n| {
        !matches!(
            n.module.as_str(),
            "custom.rust" | "clock.time" | "scalar.value"
        )
    });
    let ids = document
        .root
        .nodes
        .iter()
        .map(|n| n.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    document
        .root
        .edges
        .retain(|e| ids.contains(&e.from_node) && ids.contains(&e.to_node));
    document.custom_sources.clear();
    let mut game = GameProject::demo();
    for scene in game.scenes.values_mut() {
        if let Some(player) = scene.objects.iter_mut().find(|o| o.object.id == 1) {
            player.components.insert(
                "mesh".into(),
                json!({"path":"assets/models/demo_crystal.glb"}),
            );
        }
    }
    game.execution.stages.push(SystemStage {
        name: "Custom game logic".into(),
        nodes: vec![SystemNode {
            id: "game-controller".into(),
            module: "custom.rust.update".into(),
            params: json!({"source_id":"game-controller"}),
            bindings: BTreeMap::from([
                ("world".into(), "world".into()),
                ("input".into(), "input".into()),
                ("events".into(), "events".into()),
            ]),
            after: vec!["system-7".into()],
        }],
    });
    document.custom_sources.insert(
        "game-controller".into(),
        r#"// module-engine-state: {"schema":"game.counter.v1","initial":{"ticks":0,"hits":0}}
fn update(ctx: &mut ScriptContext) {
    let ticks = ctx.state["ticks"].as_u64().unwrap_or(0) + 1;
    let hits = ctx.state["hits"].as_u64().unwrap_or(0)
        + ctx.events.iter().filter(|event| event.get("Hit").is_some()).count() as u64;
    ctx.state["ticks"] = serde_json::json!(ticks);
    ctx.state["hits"] = serde_json::json!(hits);
    if ctx.input["interact"].as_bool() == Some(true) {
        ctx.set_color(1, [250, 180, 75, 255]);
        ctx.emit_event(serde_json::json!({"Sound":{"cue":"hit","object":1}}));
    }
}
"#
        .into(),
    );
    document.name = "ModuleEngine / Two arenas".into();
    document.game = Some(game);
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    fn without_aa(mut document: GraphDocument) -> GraphDocument {
        let renderer = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap();
        assert_eq!(
            module_engine_vulkan::remove_pass_with_bypass(
                renderer.subgraph.as_mut().unwrap(),
                "aa"
            ),
            Some(1)
        );
        document.engine.exposure.variants.remove("antialiasing");
        document.project.variants.remove("antialiasing");
        document
            .project
            .rules
            .retain(|rule| rule.group != "antialiasing");
        document
    }
    #[test]
    fn retained_render_controls_drop_only_removed_identities_not_fresh_unknown_commands() {
        let old = module_engine_vulkan::default_graph(RenderPath::Forward);
        let mut graph = old.clone();
        module_engine_vulkan::remove_pass_with_bypass(&mut graph, "aa").unwrap();
        let mut controls = RenderOverrides::default();
        controls.enabled.insert("aa".into(), false);
        controls.enabled.insert("new-unknown".into(), true);
        controls.parameters.insert(
            "tonemap".into(),
            BTreeMap::from([("exposure".into(), json!(1.25))]),
        );
        controls.resource_commands = vec![
            RenderResourceCommand::ClearResource {
                name: "aa.color".into(),
                value: [0.; 4],
            },
            RenderResourceCommand::CopyResource {
                from: "tonemap.color".into(),
                to: "aa.color".into(),
            },
            RenderResourceCommand::ClearResource {
                name: "encode.frame".into(),
                value: [0.; 4],
            },
        ];
        let retained = retained_render_controls(&old, &graph, &controls);
        assert!(!retained.enabled.contains_key("aa"));
        assert!(retained.enabled.contains_key("new-unknown"));
        assert_eq!(retained.parameters["tonemap"]["exposure"], 1.25);
        assert_eq!(retained.resource_commands.len(), 1);
        assert!(
            module_engine_vulkan::validate_overrides(&graph, &retained)
                .unwrap_err()
                .contains("Unknown render pass new-unknown")
        );
        assert!(
            controls.enabled.contains_key("aa"),
            "Staging must not mutate last-good controls"
        );
    }
    #[test]
    fn render_settings_validation_includes_unselected_presets_after_pass_removal() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let mut document = without_aa(demo_document());
        let graph = render_graph(&document);
        validate_render_settings_at(&document, &graph, &root).unwrap();
        document
            .engine
            .exposure
            .variants
            .get_mut("tonemap")
            .unwrap()
            .options
            .insert(
                "unselected-broken".into(),
                ReadyVariant {
                    label: "Invalid unselected AA".into(),
                    commands: vec![RenderCommand::SetEnabled {
                        pass: "aa".into(),
                        enabled: true,
                    }],
                },
            );
        assert!(
            validate_render_settings_at(&document, &graph, &root)
                .unwrap_err()
                .contains("Unknown render pass aa")
        );
        assert!(graphics_only_change(
            &demo_document(),
            &without_aa(demo_document())
        ));
        let mut unrelated = without_aa(demo_document());
        unrelated
            .engine
            .enabled_modules
            .insert("another-feature".into());
        assert!(!graphics_only_change(&demo_document(), &unrelated));
    }
    #[test]
    fn gpu_pass_removal_prunes_controls_only_on_acceptance_and_keeps_live_session() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let document = demo_document();
        let mut host =
            GameHost::new(document.clone(), &root, Some(&root), WorldKind::Objects).unwrap();
        host.audio.enabled = false;
        host.camera.target = [3., 0., 2.];
        host.session
            .apply_commands(&[WorldCommand::SetTransform {
                id: 1,
                transform: Transform {
                    translation: [4., 1., 2.],
                    ..Default::default()
                },
            }])
            .unwrap();
        let state = host.session.state().clone();
        let camera = host.camera;
        let slot = host.slots["game-controller"].clone();
        {
            let mut controls = host.overrides.lock().unwrap();
            controls.enabled.insert("aa".into(), false);
            controls.parameters.insert(
                "tonemap".into(),
                BTreeMap::from([("exposure".into(), json!(1.25))]),
            );
            controls
                .resource_commands
                .push(RenderResourceCommand::ClearResource {
                    name: "aa.color".into(),
                    value: [0.; 4],
                });
        }
        let cleaned = without_aa(document);
        host.sync_document_deferred(&cleaned).unwrap();
        assert!(host.has_pending_preparation());
        assert!(host.overrides.lock().unwrap().enabled.contains_key("aa"));
        assert!(!host.overrides.lock().unwrap().resource_commands.is_empty());
        let deadline = Instant::now() + Duration::from_secs(60);
        while host.has_pending_preparation() {
            assert!(Instant::now() < deadline);
            host.poll_preparation(&cleaned).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(host.session.state(), &state);
        assert_eq!(host.camera.target, camera.target);
        assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
        assert!(!host.overrides.lock().unwrap().enabled.contains_key("aa"));
        assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
        assert_eq!(
            host.overrides.lock().unwrap().parameters["tonemap"]["exposure"],
            1.25
        );
        host.overrides.lock().unwrap().parameters.insert(
            "ao".into(),
            BTreeMap::from([("strength".into(), json!(0.4))]),
        );
        let before = host.render(160, 90).unwrap().rgba;
        let mut invalid = cleaned.clone();
        let graph = invalid
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap();
        graph.nodes.retain(|node| node.id != "ao");
        graph
            .edges
            .retain(|edge| edge.from_node != "ao" && edge.to_node != "ao");
        host.sync_document_deferred(&invalid).unwrap();
        let mut rejected = false;
        while host.has_pending_preparation() {
            assert!(Instant::now() < deadline);
            if host.poll_preparation(&invalid).is_err() {
                rejected = true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(rejected);
        assert!(host.sync_document(&invalid).is_err());
        assert!(host.accepted_document().execution_eq(&cleaned));
        assert_eq!(host.session.state(), &state);
        assert_eq!(
            host.overrides.lock().unwrap().parameters["ao"]["strength"],
            0.4
        );
        assert_eq!(host.render(160, 90).unwrap().rgba, before);
        let mut fresh = host.overrides.lock().unwrap().clone();
        fresh
            .apply_commands(&[RenderCommand::SetEnabled {
                pass: "new-unknown".into(),
                enabled: true,
            }])
            .unwrap();
        let retained = retained_render_controls(&host.graph, &host.graph, &fresh);
        assert!(
            validate_candidate_controls(&cleaned, &host.graph, &retained, 0., &root)
                .unwrap_err()
                .contains("Unknown render pass new-unknown")
        );
        assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
    }
    #[test]
    fn scheduled_rust_declares_typed_world_input_and_event_dependencies() {
        let document = demo_document();
        let registry = game_registry(
            &document,
            Path::new("."),
            &BTreeMap::new(),
            Arc::new(Mutex::new(RenderOverrides::default())),
        )
        .unwrap();
        let mut graph = document.game.unwrap().execution;
        registry.validate(&graph).unwrap();
        let node = graph.stages.last_mut().unwrap().nodes.last_mut().unwrap();
        node.bindings.insert("world".into(), "input".into());
        assert!(registry.validate(&graph).is_err());
    }
    #[test]
    fn center_cursor_targets_ground_below_camera() {
        let c = Camera {
            yaw: 0.,
            pitch: 0.95,
            distance: 21.,
            target: [2., 0., -3.],
        };
        let p = aim_at(c, [0., 0.], 16. / 9.);
        assert!((p[0] - 2.).abs() < 0.0001);
        assert!((p[2] + 3.).abs() < 0.0001);
    }
    #[test]
    fn author_preview_reuses_content_and_custom_and_rejects_failed_edits_transactionally() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut document = demo_document();
            if kind == WorldKind::Ecs {
                document
                    .root
                    .nodes
                    .iter_mut()
                    .find(|node| node.module == "world.objects")
                    .unwrap()
                    .module = "world.ecs".into();
            }
            let mut host = GameHost::new(document.clone(), &root, Some(&root), kind).unwrap();
            host.audio.enabled = false;
            host.camera.target = [1., 0., 1.];
            let camera = host.camera;
            let cached = host
                .content
                .load_model("assets/models/demo_crystal.glb")
                .unwrap();
            let slot = host.slots["game-controller"].clone();
            let executable = slot.lock().unwrap().executable().to_path_buf();
            let owned_state = json!({"ticks":17,"hits":3});
            slot.lock()
                .unwrap()
                .migrate_state(owned_state.clone())
                .unwrap();
            let before_pixels = host.render(240, 135).unwrap().rgba;
            // A graph layout gesture must not reconstruct even a zero-tick
            // session: its retained world may already differ from authored data.
            host.session
                .apply_commands(&[WorldCommand::SetTransform {
                    id: 1,
                    transform: Transform {
                        translation: [8., 1., 3.],
                        ..Transform::default()
                    },
                }])
                .unwrap();
            let retained_state = host.session.state().clone();
            let retained_project = host.session.project().clone();
            let prepared_graph = host.graph.clone();
            let layout_revision = host.presentation_revision();
            for offset in [15., 30., 45.] {
                document.root.nodes[0].position[0] += offset;
                document.root.nodes[0].label.push_str(" moved");
                let inner = document
                    .root
                    .nodes
                    .iter_mut()
                    .find(|node| node.module == "render.vulkan")
                    .unwrap()
                    .subgraph
                    .as_mut()
                    .unwrap();
                inner.nodes[0].position[1] += offset;
                host.refresh_author_preview(&document).unwrap();
                assert_eq!(host.presentation_revision(), layout_revision);
                assert_eq!(host.session.state(), &retained_state);
                assert_eq!(host.session.project(), &retained_project);
                assert_eq!(host.graph, prepared_graph);
                assert_eq!(host.document, document);
                assert_eq!(*host.live_document.lock().unwrap(), document);
                assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
                assert_eq!(slot.lock().unwrap().state().unwrap(), owned_state);
                assert!(Arc::ptr_eq(
                    &cached,
                    &host
                        .content
                        .load_model("assets/models/demo_crystal.glb")
                        .unwrap()
                ));
                assert_eq!(host.camera.target, camera.target);
                assert_eq!(host.camera.distance, camera.distance);
            }
            for x in [1., 2., 3., 4.] {
                let objects = &mut document
                    .game
                    .as_mut()
                    .unwrap()
                    .scenes
                    .get_mut("arena")
                    .unwrap()
                    .objects;
                let player = objects
                    .iter_mut()
                    .find(|object| object.object.id == 1)
                    .unwrap();
                player.object.transform.translation = [x, 1., 1.];
                player.object.transform.rotation = [0., x * 0.1, 0.];
                player.object.transform.scale = [1.3, 1.3, 1.3];
                let expected_transform = player.object.transform;
                let previous_revision = host.presentation_revision();
                host.refresh_author_preview(&document).unwrap();
                assert_ne!(host.presentation_revision(), previous_revision);
                assert_eq!(
                    host.session.state().entities[&1].object.transform,
                    expected_transform
                );
                assert!(Arc::ptr_eq(
                    &cached,
                    &host
                        .content
                        .load_model("assets/models/demo_crystal.glb")
                        .unwrap()
                ));
                assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
                assert_eq!(slot.lock().unwrap().executable(), executable);
                assert_eq!(slot.lock().unwrap().state().unwrap(), owned_state);
                assert_eq!(host.camera.target, camera.target);
                assert_eq!(host.camera.distance, camera.distance);
            }
            let objects = &mut document
                .game
                .as_mut()
                .unwrap()
                .scenes
                .get_mut("arena")
                .unwrap()
                .objects;
            objects.push(AuthoredObject::new(WorldObject {
                id: 5000,
                parent: None,
                transform: Transform {
                    translation: [-2., 0., 0.],
                    scale: [0.1; 3],
                    ..Transform::default()
                },
                color: [100, 100, 100, 255],
            }));
            // Gameplay actors intentionally require root transforms. A mesh
            // without actor/collision components exercises editable hierarchy.
            let mut child = AuthoredObject::new(WorldObject {
                id: 5001,
                parent: None,
                transform: Transform {
                    translation: [1., 1., -1.],
                    scale: [1.5; 3],
                    ..Transform::default()
                },
                color: [240, 140, 80, 255],
            });
            child.components.insert(
                "mesh".into(),
                json!({"path":"assets/models/demo_crystal.glb"}),
            );
            objects.push(child);
            host.refresh_author_preview(&document).unwrap();
            for parent in [Some(5000), None, Some(5000)] {
                document
                    .game
                    .as_mut()
                    .unwrap()
                    .scenes
                    .get_mut("arena")
                    .unwrap()
                    .objects
                    .iter_mut()
                    .find(|object| object.object.id == 5001)
                    .unwrap()
                    .object
                    .parent = parent;
                host.refresh_author_preview(&document).unwrap();
                assert_eq!(host.session.state().entities[&5001].object.parent, parent);
            }
            let edited_scene = document.game.as_ref().unwrap().scenes["arena"].clone();
            document
                .game
                .as_mut()
                .unwrap()
                .scenes
                .insert("after-edit".into(), edited_scene);
            host.refresh_author_preview(&document).unwrap();
            let expected = multiply_matrices(
                document.game.as_ref().unwrap().scenes["arena"]
                    .objects
                    .iter()
                    .find(|object| object.object.id == 5000)
                    .unwrap()
                    .object
                    .transform
                    .matrix(),
                document.game.as_ref().unwrap().scenes["arena"]
                    .objects
                    .iter()
                    .find(|object| object.object.id == 5001)
                    .unwrap()
                    .object
                    .transform
                    .matrix(),
            );
            assert_eq!(
                host.session
                    .snapshot()
                    .items
                    .iter()
                    .find(|item| item.id == 5001)
                    .unwrap()
                    .model,
                expected
            );
            assert_ne!(host.render(240, 135).unwrap().rgba, before_pixels);
            let accepted_state = host.session.state().clone();
            let accepted_project = host.session.project().clone();
            let accepted_document = host.document.clone();
            let mut invalid = document.clone();
            invalid
                .game
                .as_mut()
                .unwrap()
                .scenes
                .get_mut("arena")
                .unwrap()
                .objects
                .iter_mut()
                .find(|object| object.object.id == 5000)
                .unwrap()
                .object
                .parent = Some(5001);
            assert!(host.refresh_author_preview(&invalid).is_err());
            let mut missing = document.clone();
            missing
                .game
                .as_mut()
                .unwrap()
                .scenes
                .get_mut("boss")
                .unwrap()
                .objects
                .iter_mut()
                .find(|object| object.object.id == 1)
                .unwrap()
                .components
                .insert(
                    "mesh".into(),
                    json!({"path":"assets/missing-preview-model.glb"}),
                );
            assert!(host.refresh_author_preview(&missing).is_err());
            let mut structural = document.clone();
            structural
                .game
                .as_mut()
                .unwrap()
                .execution
                .stages
                .last_mut()
                .unwrap()
                .nodes
                .last_mut()
                .unwrap()
                .params["source_id"] = json!("different-controller");
            assert!(host.refresh_author_preview(&structural).is_err());
            assert_eq!(host.session.state(), &accepted_state);
            assert_eq!(host.session.project(), &accepted_project);
            assert_eq!(host.document, accepted_document);
            assert_eq!(*host.live_document.lock().unwrap(), accepted_document);
            assert!(Arc::ptr_eq(
                &cached,
                &host
                    .content
                    .load_model("assets/models/demo_crystal.glb")
                    .unwrap()
            ));
            assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
            assert_eq!(slot.lock().unwrap().state().unwrap(), owned_state);
            host.session
                .apply_commands(&[WorldCommand::SetTransform {
                    id: 1,
                    transform: Transform::default(),
                }])
                .unwrap();
            host.session.restart().unwrap();
            assert_eq!(host.session.state(), &accepted_state);
            host.session.transition("after-edit").unwrap();
            assert_eq!(host.session.state().scene, "after-edit");
            // A reset to the accepted authored entry scene also keeps the latest
            // reparenting and transform, without compiling/replacing Custom.
            host.session.restart().unwrap();
            assert_eq!(host.session.state(), &accepted_state);
            host.step(0., &GameInput::default()).unwrap();
            assert!(
                host.refresh_author_preview(&document)
                    .unwrap_err()
                    .contains("during Play")
            );
        }
    }
    #[test]
    fn relocated_game_uses_project_assets_and_installed_backend_for_rust_render_controls() {
        let backend = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let assets = backend.join(format!(
            ".work/test-temp/game-separated-roots-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(assets.join("assets")).unwrap();
        std::fs::write(
            assets.join("assets/project-marker.txt"),
            "relocated project assets",
        )
        .unwrap();
        let mut document = demo_document();
        let game = document.game.as_ref().unwrap();
        let mut files = game.audio_cues.values().cloned().collect::<Vec<_>>();
        files.push("assets/models/demo_crystal.glb".into());
        for relative in files {
            let target = assets.join(&relative);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(backend.join(relative), target).unwrap();
        }
        document.custom_sources.insert("game-controller".into(), r#"// module-engine-state: {"schema":"project.assets.v1","initial":{"ticks":0,"marker":""}}
fn update(ctx: &mut ScriptContext) {
    ctx.state["ticks"] = serde_json::json!(ctx.state["ticks"].as_u64().unwrap() + 1);
    ctx.state["marker"] = serde_json::json!(std::fs::read_to_string("assets/project-marker.txt").unwrap());
    ctx.set_render_parameter("tonemap", "exposure", 1.25);
}
"#.into());
        assert!(!assets.join("tools/dxc/dxc.exe").exists());
        assert_eq!(backend_root_for(&assets, Some(&backend)).unwrap(), backend);
        let mut host = GameHost::new(
            document.clone(),
            &assets,
            Some(&backend),
            WorldKind::Objects,
        )
        .unwrap();
        host.audio.enabled = false;
        assert_eq!(host.root, assets.canonicalize().unwrap());
        assert_eq!(host.backend_root, backend);
        assert_ne!(host.root, host.backend_root);
        host.step(1. / 60., &GameInput::default()).unwrap();
        let state = host.slots["game-controller"]
            .lock()
            .unwrap()
            .state()
            .unwrap();
        assert_eq!(state["marker"], "relocated project assets");
        assert_eq!(state["ticks"], 1);
        assert_eq!(
            host.overrides.lock().unwrap().parameters["tonemap"]["exposure"],
            1.25
        );
        let frame = host.render(160, 90).unwrap();
        assert_eq!([frame.width, frame.height], [160, 90]);
        assert!(!frame.rgba.is_empty());
        let edited = document.custom_sources["game-controller"].replace(" + 1", " + 2");
        let candidate = ScriptCompiler::new(backend.clone(), backend.join(".work/game-scripts"))
            .compile_kind(&edited, ScriptKind::Update)
            .unwrap();
        host.apply_candidate("game-controller", candidate, false)
            .unwrap();
        host.step(1. / 60., &GameInput::default()).unwrap();
        let state = host.slots["game-controller"]
            .lock()
            .unwrap()
            .state()
            .unwrap();
        assert_eq!(state["marker"], "relocated project assets");
        assert_eq!(state["ticks"], 3);
        assert!(!assets.join("tools").exists());
        drop(host);
        let allowed = backend.join(".work/test-temp").canonicalize().unwrap();
        let assets = assets.canonicalize().unwrap();
        assert!(assets.starts_with(allowed));
        std::fs::remove_dir_all(assets).unwrap();
    }
}
