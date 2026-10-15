//! Shared authored-game execution. Applications supply the installed composition.
#[cfg(test)]
#[path = "compat_tests.rs"]
mod compat_tests;
#[cfg(test)]
#[path = "test_support.rs"]
mod test_support;
use crate::activity::{ActivitySnapshot, BuildProgress};
use module_engine_content::ContentLibrary;
use module_engine_core::*;
use module_engine_host_services::{HostServiceContext, HostServiceRegistry, HostServices};
use module_engine_script::{CompiledScript, ScriptCompiler, ScriptKind, ScriptTick};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub enum RenderMode {
    Graph,
    /// The explicitly selected registered world is usable without a viewport.
    Headless {
        world_node: String,
    },
}
#[derive(Clone)]
pub struct HostOptions {
    pub asset_root: PathBuf,
    pub backend_root: Option<PathBuf>,
    pub compile_root: Option<PathBuf>,
    pub render: RenderMode,
    pub services: HostServiceRegistry,
    pub services_enabled: bool,
}
pub type ScriptSlot = Arc<Mutex<CompiledScript>>;
struct RenderPreparation {
    document: GraphDocument,
    receiver: mpsc::Receiver<Result<Box<dyn FrameRenderer>, String>>,
    progress: BuildProgress,
    dependencies: RendererDependencyWatch,
}

/// A bounded-frequency adapter dependency watch also retains failed states.
pub struct RendererDependencyWatch {
    revision: Result<u64, String>,
    checked: Instant,
}
impl RendererDependencyWatch {
    pub fn new(document: &GraphDocument, modules: &ModuleRegistry) -> Self {
        Self {
            revision: Self::revision(document, modules),
            checked: Instant::now(),
        }
    }
    fn revision(document: &GraphDocument, modules: &ModuleRegistry) -> Result<u64, String> {
        let route = modules.render_route(document)?;
        modules
            .renderer_adapter(&route.renderer.module)
            .ok_or("Missing renderer adapter")?
            .dependency_revision(route.renderer, document)
    }
    pub fn changed(&mut self, document: &GraphDocument, modules: &ModuleRegistry) -> bool {
        if self.checked.elapsed() < Duration::from_millis(250) {
            return false;
        }
        self.checked = Instant::now();
        let revision = Self::revision(document, modules);
        let changed = self.revision != revision;
        self.revision = revision;
        changed
    }
}
fn renderer_key(modules: &ModuleRegistry, document: &GraphDocument) -> Result<Value, String> {
    let route = modules.render_route(document)?;
    let adapter = modules
        .renderer_adapter(&route.renderer.module)
        .ok_or("Missing renderer adapter")?;
    Ok(json!([
        route.renderer.id,
        route.renderer.module,
        adapter.configuration_key(route.renderer, document)
    ]))
}
pub fn graphics_only_change(
    modules: &ModuleRegistry,
    previous: &GraphDocument,
    document: &GraphDocument,
) -> bool {
    let mut normalized = document.clone();
    normalized.shader_sources = previous.shader_sources.clone();
    normalized.engine.exposure = previous.engine.exposure.clone();
    normalized.project = previous.project.clone();
    normalize_renderer_nodes(modules, &previous.root, &mut normalized.root);
    previous.execution_eq(&normalized)
}
fn normalize_renderer_nodes(modules: &ModuleRegistry, previous: &Graph, graph: &mut Graph) {
    for (node, old) in graph.nodes.iter_mut().zip(&previous.nodes) {
        if node.module == old.module && modules.is_renderer(&node.module) {
            node.params = old.params.clone();
            node.subgraph = old.subgraph.clone();
        }
    }
}
pub fn engine_structure_eq(modules: &ModuleRegistry, previous: &Graph, graph: &Graph) -> bool {
    let mut normalized = graph.clone();
    normalize_renderer_nodes(modules, previous, &mut normalized);
    previous.execution_eq(&normalized)
}
fn retained_render_controls(
    modules: &ModuleRegistry,
    previous: &GraphDocument,
    document: &GraphDocument,
    controls: &RenderOverrides,
) -> Result<RenderOverrides, String> {
    let old = modules.render_route(previous)?;
    let new = modules.render_route(document)?;
    let old_targets = modules
        .renderer_adapter(&old.renderer.module)
        .ok_or("Missing renderer adapter")?
        .control_targets(old.renderer, previous)?;
    let new_targets = modules
        .renderer_adapter(&new.renderer.module)
        .ok_or("Missing renderer adapter")?
        .control_targets(new.renderer, document)?;
    let (Some(old), Some(new)) = (old_targets, new_targets) else {
        return Ok(controls.clone());
    };
    let removed_pass = |id: &str| old.controls.contains(id) && !new.controls.contains(id);
    let removed_resource = |id: &str| old.resources.contains(id) && !new.resources.contains(id);
    let mut retained = controls.clone();
    retained.parameters.retain(|id, _| !removed_pass(id));
    retained.enabled.retain(|id, _| !removed_pass(id));
    retained.resource_commands.retain(|command| match command {
        RenderResourceCommand::ClearResource { name, .. } => !removed_resource(name),
        RenderResourceCommand::CopyResource { from, to } => {
            !removed_resource(from) && !removed_resource(to)
        }
    });
    Ok(retained)
}
fn effective_render_controls(
    document: &GraphDocument,
    time: f64,
    controls: &RenderOverrides,
) -> Result<RenderOverrides, String> {
    let commands = project_commands(document, time)?
        .into_iter()
        .filter_map(|command| match command {
            ControlCommand::Render(c) => Some(c),
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
pub fn validate_render_settings(
    modules: &ModuleRegistry,
    document: &GraphDocument,
) -> Result<(), String> {
    let route = modules.render_route(document)?;
    let adapter = modules
        .renderer_adapter(&route.renderer.module)
        .ok_or("Missing renderer adapter")?;
    adapter.validate_controls(
        route.renderer,
        document,
        &effective_render_controls(document, 0., &RenderOverrides::default())?,
    )?;
    for group in document.engine.exposure.variants.values() {
        for variant in group.options.values() {
            let mut controls = RenderOverrides::default();
            controls.apply_commands(&variant.commands)?;
            adapter.validate_controls(route.renderer, document, &controls)?;
        }
    }
    Ok(())
}
fn validate_candidate_controls(
    modules: &ModuleRegistry,
    document: &GraphDocument,
    controls: &RenderOverrides,
    time: f64,
) -> Result<(), String> {
    validate_render_settings(modules, document)?;
    let route = modules.render_route(document)?;
    modules
        .renderer_adapter(&route.renderer.module)
        .ok_or("Missing renderer adapter")?
        .validate_controls(
            route.renderer,
            document,
            &effective_render_controls(document, time, controls)?,
        )
}
fn service_requests(
    contributions: &[GameContribution],
    game: &GameProject,
) -> Result<Vec<HostServiceRequest>, String> {
    contributions.iter().try_fold(vec![], |mut result, c| {
        result.extend((c.service_requests)(game)?);
        Ok(result)
    })
}
fn build_session(
    game: GameProject,
    registry: SystemRegistry,
    context: &ComponentValidationContext<'_>,
    modules: &ModuleRegistry,
    world: &Node,
) -> Result<GameSession, String> {
    let metadata = modules
        .world_metadata(&world.module)
        .ok_or("Missing world metadata")?;
    (metadata.validate_params)(&world.params)?;
    match metadata.game_policy {
        GameWorldPolicy::CanonicalEcs => {
            GameSession::new_with_registry_and_context(game, metadata.kind, registry, context)
        }
        GameWorldPolicy::Projection => {
            let modules = modules.clone();
            let world = world.clone();
            GameSession::new_with_world_factory_and_context(
                game,
                metadata.kind,
                registry,
                move |objects| modules.create_game_world(&world, objects),
                context,
            )
        }
    }
}
struct RustSystem {
    slot: ScriptSlot,
    document: Arc<Mutex<GraphDocument>>,
    root: PathBuf,
    modules: ModuleRegistry,
    overrides: Arc<Mutex<RenderOverrides>>,
    pending: Option<ScriptTick>,
    checkpoint: Option<(Value, RenderOverrides)>,
    input_contributions: Vec<GameContribution>,
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
        let entities = ctx.game.world.entities_snapshot();
        let objects: Vec<_> = entities.iter().map(|e| e.object.clone()).collect();
        let mut input = ScriptContext::new(ctx.game.time, ctx.dt as f64, objects.clone());
        input.asset_paths =
            module_engine_project::AssetManifest::from_value(&document.asset_manifest)?
                .asset_paths();
        let script_input = document
            .game
            .as_ref()
            .and_then(|game| {
                self.input_contributions
                    .iter()
                    .find(|contribution| contribution.is_active(&game.execution))
            })
            .map_or(
                generic_script_input as fn(&GameInput) -> Value,
                |contribution| contribution.script_input,
            );
        input.input = script_input(ctx.input);
        input.events = ctx
            .events
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
            .collect();
        input.parameters = module_engine_core::resolved_parameters(&document);
        for entity in &entities {
            for (name, value) in &entity.components {
                if let Some(fields) = value.as_object() {
                    input
                        .properties
                        .components
                        .entry(entity.object.id)
                        .or_default()
                        .insert(
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
        let mut candidate_world = ctx.game.world.clone();
        candidate_world.apply_commands(&world)?;
        let mut overrides = self
            .overrides
            .lock()
            .map_err(|_| "Render control slot poisoned")?;
        let mut candidate = overrides.clone();
        candidate.apply_commands(&render)?;
        if !render.is_empty() {
            validate_candidate_controls(&self.modules, &document, &candidate, ctx.game.time)?;
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
        self.checkpoint = Some((checkpoint, overrides.clone()));
        ctx.game.world = candidate_world;
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

pub fn bind_game_registry(
    document: Arc<Mutex<GraphDocument>>,
    root: &Path,
    composition: &CompositionContext,
    slots: &BTreeMap<String, ScriptSlot>,
    overrides: Arc<Mutex<RenderOverrides>>,
) -> Result<SystemRegistry, String> {
    let mut registry = composition.systems.clone();
    if registry.descriptor("custom.rust.update").is_none() {
        if !slots.is_empty() {
            return Err("Compiled Custom slots require an installed Custom contract".into());
        }
        return Ok(registry);
    }
    let modules = composition.modules.clone();
    let input_contributions = registry.game_contributions().cloned().collect::<Vec<_>>();
    let slots = slots.clone();
    let root = root.to_path_buf();

    registry.bind_factory("custom.rust.update", move |params| {
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
            modules: modules.clone(),
            overrides: overrides.clone(),
            pending: None,
            checkpoint: None,
            input_contributions: input_contributions.clone(),
        }))
    })?;
    Ok(registry)
}

fn generic_script_input(input: &GameInput) -> Value {
    serde_json::to_value(input).expect("Portable input contains JSON values")
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
    renderer: Option<Box<dyn FrameRenderer>>,
    composition: CompositionContext,
    options: HostOptions,
    retired: Arc<std::sync::atomic::AtomicBool>,
    rejected_render: Option<(GraphDocument, String)>,
    rejected_render_dependencies: Option<RendererDependencyWatch>,
    overrides: Arc<Mutex<RenderOverrides>>,
    simulation_started: bool,
    pub services: HostServices,
    render_preparation: Option<RenderPreparation>,
    render_activity: Option<BuildProgress>,
    service_activity: Option<BuildProgress>,
    rejected_preparation: Option<(GraphDocument, String)>,
    rejected_preparation_dependencies: Option<RendererDependencyWatch>,
    presentation_revision: u64,
    contributions: Vec<GameContribution>,
}
fn next_presentation_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
impl GameHost {
    pub fn presentation_ids(&self) -> Vec<String> {
        self.contributions
            .iter()
            .map(|contribution| contribution.id.clone())
            .collect()
    }
    pub fn presentation_id(&self) -> Option<&str> {
        self.contributions
            .first()
            .map(|contribution| contribution.id.as_str())
    }
    pub fn frame_target(&self) -> Option<[f32; 3]> {
        self.contributions
            .iter()
            .find_map(|contribution| (contribution.follow_target)(self.session.state()))
    }
    pub fn game_diagnostics(&self) -> Value {
        self.contributions.first().map_or_else(
            || json!({}),
            |contribution| (contribution.diagnostics)(self.session.state()),
        )
    }
    fn follow_camera(&mut self) {
        for contribution in &self.contributions {
            if let Some(target) = (contribution.follow_target)(self.session.state()) {
                self.camera.target = target;
            }
        }
    }
    /// Unique host/presentation identity. Cosmetic graph edits do not invalidate
    /// the displayed frame; accepted world/GPU changes and simulation do.
    pub fn presentation_revision(&self) -> u64 {
        self.presentation_revision
    }
    pub fn renderer_performance(&self) -> RendererPerformance {
        self.renderer
            .as_ref()
            .map_or_else(RendererPerformance::default, |renderer| {
                renderer.performance()
            })
    }
    pub fn invalidate_content(&mut self) {
        self.content.invalidate();
        self.presentation_revision = next_presentation_revision();
    }
    pub fn set_detailed_stats(&mut self, enabled: bool) {
        if let Some(renderer) = &mut self.renderer {
            renderer.set_detailed_stats(enabled);
        }
    }
    pub fn build(
        document: GraphDocument,
        composition: CompositionContext,
        options: HostOptions,
        progress: Option<&BuildProgress>,
    ) -> Result<Self, String> {
        let root = options
            .asset_root
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let backend_root = match options.render {
            RenderMode::Graph => options
                .backend_root
                .as_deref()
                .unwrap_or(&root)
                .canonicalize()
                .map_err(|error| error.to_string())?,
            RenderMode::Headless { .. } => root.clone(),
        };
        let compile_root = options.compile_root.as_deref();
        if let Some(progress) = progress {
            progress.stage("Проверка модулей и игровой сцены");
        }
        let mut game = document
            .game
            .clone()
            .ok_or("Project has no authored game")?;

        let errors = composition
            .validate_document(&document)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message)
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        let world = match &options.render {
            RenderMode::Graph => composition.modules.render_route(&document)?.world,
            RenderMode::Headless { world_node } => {
                composition.modules.world_node(&document, world_node)?
            }
        }
        .clone();
        let metadata = composition
            .modules
            .world_metadata(&world.module)
            .ok_or("Missing world metadata")?;
        (metadata.validate_params)(&world.params)?;
        if matches!(options.render, RenderMode::Graph) {
            validate_render_settings(&composition.modules, &document)?;
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
        let registry = bind_game_registry(
            live_document.clone(),
            &root,
            &composition,
            &slots,
            overrides.clone(),
        )?;
        let activity = composition.activity(&document)?;
        let asset_paths =
            module_engine_project::AssetManifest::from_value(&document.asset_manifest)?
                .asset_paths();
        let context = ComponentValidationContext {
            scenes: &game.scenes,
            modules: &activity.modules,
            asset_paths: Some(&asset_paths),
            active_components: Some(&activity.components),
        };
        let session = build_session(
            game.clone(),
            registry.clone(),
            &context,
            &composition.modules,
            &world,
        )?;
        let contributions = registry
            .active_game_contributions(&game.execution)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if contributions.len() > 1 {
            return Err("Composition selects multiple game control presentations".into());
        }
        let camera = contributions
            .first()
            .map_or_else(Camera::default, |contribution| {
                (contribution.initial_camera)(&game)
            });
        let mut renderer = match options.render {
            RenderMode::Graph => {
                if let Some(progress) = progress {
                    progress.stage("Подготовка выбранного рендера");
                }
                let route = composition.modules.render_route(&document)?;
                Some(
                    composition
                        .modules
                        .create_renderer(route.renderer, &document)?,
                )
            }
            RenderMode::Headless { .. } => None,
        };
        if let Some(renderer) = &mut renderer {
            renderer.set_detailed_stats(false);
        }
        let device = renderer
            .as_ref()
            .map_or_else(|| "Headless".into(), |r| r.backend_name().to_owned());
        if let Some(progress) = progress {
            progress.stage("Загрузка моделей, текстур и звука");
        }
        let mut content = ContentLibrary::new(&root)?;
        let assets = module_engine_project::AssetManifest::from_value(&document.asset_manifest)?;
        content.set_asset_paths(assets.asset_paths());
        if renderer.is_some() {
            for scene in game.scenes.keys() {
                let resolved = game.resolve_scene_with_context(scene, &registry, &context)?;
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
        }
        let retired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let requests = service_requests(&contributions, &game)?;
        let services = options.services.build(
            &requests,
            &HostServiceContext {
                asset_root: &root,
                asset_paths: &asset_paths,
                retired: retired.clone(),
            },
            options.services_enabled && renderer.is_some(),
        )?;
        Ok(Self {
            session,
            content,
            camera,
            slots,
            script_sources,
            last_step_ms: 0.,
            last_render_ms: 0.,
            device,
            root,
            backend_root,
            document,
            contributions,
            live_document,
            renderer,
            composition,
            options,
            retired,
            rejected_render: None,
            rejected_render_dependencies: None,
            overrides,
            simulation_started: false,
            services,
            render_preparation: None,
            render_activity: None,
            service_activity: None,
            rejected_preparation: None,
            rejected_preparation_dependencies: None,
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
        let composition = self.composition.clone();
        let errors = composition
            .validate_document(document)
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
            if self.renderer.is_some() {
                validate_candidate_controls(&composition.modules, document, &controls, 0.)?;
            }
        }
        let assets = module_engine_project::AssetManifest::from_value(&document.asset_manifest)?;
        let asset_paths = assets.asset_paths();
        let activity = composition.activity(document)?;
        let context = ComponentValidationContext {
            scenes: &game.scenes,
            modules: &activity.modules,
            asset_paths: Some(&asset_paths),
            active_components: Some(&activity.components),
        };
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
        if self.renderer.is_some() {
            for scene in game.scenes.keys() {
                for entity in
                    game.resolve_scene_with_context(scene, &composition.systems, &context)?
                {
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
                        if let Some(path) =
                            material.get("base_color_texture").and_then(Value::as_str)
                        {
                            content.load_texture(path)?;
                        }
                    }
                }
            }
        }
        let requests = service_requests(&self.contributions, &game)?;
        let old_requests = service_requests(&self.contributions, previous)?;
        let services =
            if requests != old_requests || assets.asset_paths() != old_assets.asset_paths() {
                let mut services = self.options.services.build(
                    &requests,
                    &HostServiceContext {
                        asset_root: &self.root,
                        asset_paths: &asset_paths,
                        retired: self.retired.clone(),
                    },
                    self.options.services_enabled && self.renderer.is_some(),
                )?;
                services.set_enabled(self.services.enabled());
                Some(services)
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
        let registry = bind_game_registry(
            self.live_document.clone(),
            &self.root,
            &composition,
            &self.slots,
            self.overrides.clone(),
        )?;
        // System construction does not initialize or execute Custom. Existing
        // accepted code/state remain in the retained slots until explicit Play.
        let world = match &self.options.render {
            RenderMode::Graph => composition.modules.render_route(document)?.world,
            RenderMode::Headless { world_node } => {
                composition.modules.world_node(document, world_node)?
            }
        };
        let session = build_session(
            game.clone(),
            registry.clone(),
            &context,
            &composition.modules,
            world,
        )?;
        let contributions = registry
            .active_game_contributions(&game.execution)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if contributions.len() > 1 {
            return Err("Composition selects multiple game control presentations".into());
        }
        let mut live = self
            .live_document
            .lock()
            .map_err(|_| "Game configuration poisoned")?;
        self.session = session;
        self.contributions = contributions;
        self.document = document.clone();
        *live = document.clone();
        if let Some(content) = staged_content {
            self.content = content;
        }
        if let Some(services) = services {
            self.services = services;
        }
        self.presentation_revision = next_presentation_revision();
        Ok(())
    }
    pub fn step(&mut self, dt: f64, input: &GameInput) -> Result<usize, String> {
        self.step_budget(dt, input, 15)
    }
    /// Yield to the interactive owner's mailbox after a bounded number of
    /// actual fixed ticks. The synchronous CLI path keeps its 15-tick budget.
    pub fn step_budget(
        &mut self,
        dt: f64,
        input: &GameInput,
        max_ticks: usize,
    ) -> Result<usize, String> {
        if input.restart {
            if !dt.is_finite() || dt < 0. {
                return Err("Game dt/input must be finite and dt nonnegative".into());
            }
            self.simulation_started = true;
            self.restart()?;
            return Ok(0);
        }
        self.simulation_started = true;
        let started = Instant::now();
        self.overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?
            .begin_frame();
        let ticks = self.session.step_budget(dt, input, max_ticks)?;
        self.presentation_revision = next_presentation_revision();
        self.last_step_ms = started.elapsed().as_secs_f64() * 1000.;
        if let Err(error) = self.services.consume(self.session.events()) {
            self.record_service_error("step", error);
        }
        self.follow_camera();
        Ok(ticks)
    }
    /// Restart the whole hosted game, including state held outside GameSession.
    /// Retain compiled code, GPU resources and personal camera orbit.
    pub fn restart(&mut self) -> Result<(), String> {
        let mut slots = self
            .slots
            .values()
            .map(|slot| slot.lock().map_err(|_| "Custom slot poisoned".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let replacements = slots
            .iter()
            .map(|script| script.fork_instance())
            .collect::<Vec<_>>();
        let mut controls = self
            .overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?;
        self.session.restart()?;
        for (slot, replacement) in slots.iter_mut().zip(replacements) {
            **slot = replacement;
        }
        *controls = RenderOverrides::default();
        drop(controls);
        drop(slots);
        if let Err(error) = self.services.reset() {
            self.record_service_error("restart", error);
        }
        self.follow_camera();
        self.last_step_ms = 0.;
        self.last_render_ms = 0.;
        self.presentation_revision = next_presentation_revision();
        Ok(())
    }
    /// The exact renderer-neutral geometry/materials displayed by Scene and Game.
    /// Imported primitives retain their authored object IDs for picking.
    pub fn author_snapshot(&mut self) -> Result<SceneSnapshot, String> {
        let mut snapshot = self.session.snapshot();
        self.content
            .enrich_snapshot(&mut snapshot, &self.session.asset_bindings())?;
        for item in &mut snapshot.items {
            if let Some(material) = self.session.state().world.component(item.id, "material") {
                if let Some(color) = material.get("base_color") {
                    item.material.base_color = serde_json::from_value(color.clone())
                        .map_err(|error| format!("Material color: {error}"))?;
                }
                if let Some(path) = material.get("base_color_texture").and_then(Value::as_str) {
                    item.material.base_color_texture = Some(self.content.load_texture(path)?);
                }
            }
        }
        for contribution in &self.contributions {
            (contribution.decorate_snapshot)(&mut snapshot, self.session.state());
        }
        Ok(snapshot)
    }
    pub fn render(&mut self, width: usize, height: usize) -> Result<Frame, String> {
        if self.renderer.is_none() {
            return Err("Headless host has no renderer".into());
        }
        let started = Instant::now();
        let snapshot = self.author_snapshot()?;
        let controls = self
            .overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?
            .clone();
        let overrides =
            effective_render_controls(&self.document, self.session.state().time, &controls)?;
        let frame = self
            .renderer
            .as_mut()
            .ok_or("Headless host has no renderer")?
            .render(&snapshot, self.camera, width, height, &overrides)?;
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
    pub fn has_renderer(&self) -> bool {
        self.renderer.is_some()
    }
    pub fn retirement_signal(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.retired.clone()
    }
    pub fn graphics_only_change(&self, document: &GraphDocument) -> bool {
        graphics_only_change(&self.composition.modules, &self.document, document)
    }
    pub fn sync_document(&mut self, document: &GraphDocument) -> Result<(), String> {
        if document == &self.document {
            return Ok(());
        }
        if document.execution_eq(&self.document) {
            return self.accept_document(document);
        }
        if self.renderer.is_none() {
            if !document.root.execution_eq(&self.document.root) {
                return Err("Headless engine structure changed; rebuild explicitly".into());
            }
            return self.accept_document(document);
        }
        if let Some((failed, error)) = &self.rejected_render
            && failed.execution_eq(document)
            && !self
                .rejected_render_dependencies
                .as_mut()
                .is_some_and(|watch| watch.changed(document, &self.composition.modules))
        {
            return Err(error.clone());
        }
        let modules = &self.composition.modules;
        let current = self
            .overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?
            .clone();
        let controls = retained_render_controls(modules, &self.document, document, &current)?;
        let changed = renderer_key(modules, document)? != renderer_key(modules, &self.document)?;
        let dependencies = RendererDependencyWatch::new(document, modules);
        let result: Result<Option<Box<dyn FrameRenderer>>, String> = (|| {
            validate_candidate_controls(modules, document, &controls, self.session.state().time)?;
            if changed {
                let node = modules.render_route(document)?.renderer;
                let previous = modules.render_route(&self.document)?.renderer;
                let candidate = if previous.module == node.module {
                    self.renderer
                        .as_mut()
                        .unwrap()
                        .prepare_candidate(node, document)?
                } else {
                    None
                };
                Ok(Some(match candidate {
                    Some(candidate) => candidate,
                    None => modules.create_renderer(node, document)?,
                }))
            } else {
                Ok(None)
            }
        })();
        let candidate = match result {
            Ok(candidate) => candidate,
            Err(error) => {
                self.rejected_render = Some((document.clone(), error.clone()));
                self.rejected_render_dependencies = Some(dependencies);
                return Err(error);
            }
        };
        self.accept_render_document(document, controls)?;
        if let Some(candidate) = candidate {
            self.device = candidate.backend_name().into();
            self.renderer = Some(candidate);
        }
        self.rejected_render = None;
        self.rejected_render_dependencies = None;
        Ok(())
    }
    pub fn preparation_activity(&self) -> Option<ActivitySnapshot> {
        self.render_activity.as_ref().map(BuildProgress::snapshot)
    }
    /// Service errors remain visible after later successful operations. External
    /// effects run after game publication and cannot reject that committed tick
    /// or restart; callers must still acknowledge input and advance the epoch.
    pub fn activities(&self) -> Vec<ActivitySnapshot> {
        self.render_activity
            .iter()
            .chain(self.service_activity.iter())
            .map(BuildProgress::snapshot)
            .collect()
    }
    fn record_service_error(&mut self, operation: &str, error: String) {
        let activity = self.service_activity.get_or_insert_with(|| {
            let activity = BuildProgress::new("Game services");
            activity.stage("Game state accepted; service reported an error");
            activity
        });
        activity.fail(format!("After committed {operation}: {error}"));
    }
    pub fn has_pending_preparation(&self) -> bool {
        self.render_preparation.is_some()
    }
    pub fn cancel_preparation(&mut self) {
        if let Some(job) = self.render_preparation.take() {
            job.progress
                .succeed("Запрос заменён; текущий рендер сохранён");
            std::thread::spawn(move || drop(job));
        }
    }
    pub fn sync_document_deferred(&mut self, document: &GraphDocument) -> Result<(), String> {
        if self
            .rejected_preparation
            .as_ref()
            .is_some_and(|(failed, _)| !failed.execution_eq(document))
        {
            self.rejected_preparation = None;
            self.rejected_preparation_dependencies = None;
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
        if self.renderer.is_none()
            || renderer_key(&self.composition.modules, document)?
                == renderer_key(&self.composition.modules, &self.document)?
        {
            self.cancel_preparation();
            return self.sync_document(document);
        }
        if let Some((failed, error)) = &self.rejected_preparation
            && failed.execution_eq(document)
            && !self
                .rejected_preparation_dependencies
                .as_mut()
                .is_some_and(|watch| watch.changed(document, &self.composition.modules))
        {
            return Err(error.clone());
        }
        if self.render_preparation.is_some() {
            return Ok(());
        }
        let candidate = document.clone();
        let modules = self.composition.modules.clone();
        let current = self
            .overrides
            .lock()
            .map_err(|_| "Render controls poisoned")?
            .clone();
        let controls = retained_render_controls(&modules, &self.document, document, &current)?;
        let worker_document = candidate.clone();
        let time = self.session.state().time;
        let progress = BuildProgress::new("Рендер Game / Scene");
        let worker_progress = progress.clone();
        let dependencies = RendererDependencyWatch::new(document, &modules);
        let (send, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            worker_progress.stage("Подготовка выбранного рендера; текущий кадр доступен");
            let result = (|| {
                validate_candidate_controls(&modules, &worker_document, &controls, time)?;
                let node = modules.render_route(&worker_document)?.renderer;
                let mut renderer = modules.create_renderer(node, &worker_document)?;
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
            dependencies,
        });
        self.rejected_preparation = None;
        self.rejected_preparation_dependencies = None;
        Ok(())
    }
    pub fn poll_preparation(&mut self, document: &GraphDocument) -> Result<(), String> {
        if self
            .rejected_preparation
            .as_ref()
            .is_some_and(|(failed, _)| !failed.execution_eq(document))
        {
            self.rejected_preparation = None;
            self.rejected_preparation_dependencies = None;
            self.render_activity = None;
        }
        let Some(job) = &self.render_preparation else {
            return Ok(());
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return Ok(()),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Поток подготовки рендера завершился без результата".into())
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
            let validation = (|| {
                let current = self
                    .overrides
                    .lock()
                    .map_err(|_| "Render controls poisoned")?
                    .clone();
                let controls = retained_render_controls(
                    &self.composition.modules,
                    &self.document,
                    document,
                    &current,
                )?;
                validate_candidate_controls(
                    &self.composition.modules,
                    document,
                    &controls,
                    self.session.state().time,
                )?;
                self.accept_render_document(document, controls)
            })();
            if let Err(error) = validation {
                std::thread::spawn(move || drop(candidate));
                return Err(error);
            }
            Ok(candidate)
        });
        match candidate {
            Ok(candidate) => {
                self.device = candidate.backend_name().into();
                let old = self.renderer.replace(candidate);
                self.rejected_render = None;
                self.rejected_render_dependencies = None;
                self.rejected_preparation = None;
                self.rejected_preparation_dependencies = None;
                job.progress
                    .succeed("Новый рендер применён; игровое состояние сохранено");
                std::thread::spawn(move || drop(old));
                Ok(())
            }
            Err(error) => {
                job.progress.fail(error.clone());
                self.rejected_preparation = Some((document.clone(), error.clone()));
                self.rejected_preparation_dependencies = Some(job.dependencies);
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
