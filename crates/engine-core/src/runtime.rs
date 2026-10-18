use crate::{
    Camera, ControlCommand, Diagnostic, Frame, FrameRenderer, Graph, GraphDocument, ModuleRegistry,
    Node, RenderOverrides, RenderSubmission, SceneSnapshot, Severity, WorldKind, WorldObject,
    WorldProvider, validate_graph,
};
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct CommandSource {
    pub source_id: String,
    pub world: bool,
    pub render: bool,
}

/// A rejected graph or executable route. Type-valid graphs are not automatically
/// promises of installed execution support.
#[derive(Clone, Debug)]
pub struct RuntimeError {
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// Explicit connection-only selection. Resolving this route creates no provider
/// or renderer and is safe during authoring/headless installation preflight.
#[derive(Clone, Copy, Debug)]
pub struct RenderRoute<'a> {
    pub viewport: &'a Node,
    pub renderer: &'a Node,
    pub extract: &'a Node,
    pub world: &'a Node,
}

impl ModuleRegistry {
    /// Select a declared world by node identity, with no renderer requirement.
    pub fn world_node<'a>(
        &self,
        document: &'a GraphDocument,
        node_id: &str,
    ) -> Result<&'a Node, String> {
        let mut nodes = document.root.nodes.iter().filter(|node| node.id == node_id);
        let node = nodes
            .next()
            .ok_or_else(|| format!("World node '{node_id}' is missing"))?;
        if nodes.next().is_some() {
            return Err(format!("World node '{node_id}' is duplicated"));
        }
        let metadata = self
            .world_metadata(&node.module)
            .ok_or_else(|| format!("Module '{}' has no installed World provider", node.module))?;
        if node.subgraph.is_some() {
            return Err("World modules do not execute nested graphs".into());
        }
        (metadata.validate_params)(&node.params)?;
        Ok(node)
    }

    pub fn render_route<'a>(&self, document: &'a GraphDocument) -> Result<RenderRoute<'a>, String> {
        let graph = &document.root;
        let mut ids = BTreeSet::new();
        if graph
            .nodes
            .iter()
            .any(|node| node.id.trim().is_empty() || !ids.insert(node.id.as_str()))
        {
            return Err("Render route has empty or duplicate node IDs".into());
        }
        let mut outputs = graph
            .nodes
            .iter()
            .filter(|node| node.module == "viewport.output");
        let viewport = outputs
            .next()
            .ok_or("Runtime requires exactly one viewport.output")?;
        if outputs.next().is_some() {
            return Err("Runtime requires exactly one viewport.output".into());
        }
        require_no_params(viewport).map_err(|error| error.to_string())?;
        let renderer =
            upstream(graph, viewport, "frame", "frame").map_err(|error| error.to_string())?;
        if !self.is_renderer(&renderer.module) {
            return Err("Viewport needs a registered executable renderer module".into());
        }
        let extract =
            upstream(graph, renderer, "scene", "scene").map_err(|error| error.to_string())?;
        if extract.module != "scene.extract" {
            return Err("Renderer needs the installed scene.extract adapter".into());
        }
        require_no_params(extract).map_err(|error| error.to_string())?;
        let source =
            upstream(graph, extract, "world", "world").map_err(|error| error.to_string())?;
        let world = self.world_node(document, &source.id)?;
        if extract.subgraph.is_some() || viewport.subgraph.is_some() {
            return Err("Extraction and viewport do not execute nested graphs".into());
        }
        Ok(RenderRoute {
            viewport,
            renderer,
            extract,
            world,
        })
    }
}

/// An executable route built from a validated graph, not from a world dropdown.
///
/// Scalar graph evaluation belongs to the host. Use world_speed_source to follow
/// the actual connected source, evaluate that Scalar subgraph, then call step.
/// The renderer consumes the selected provider's snapshot through scene.extract.
pub struct Runtime {
    provider: Box<dyn WorldProvider>,
    kind: WorldKind,
    world_node_id: String,
    world_module_id: String,
    speed_source: Option<String>,
    document: GraphDocument,
    registry: ModuleRegistry,
    renderer: Mutex<Box<dyn FrameRenderer>>,
    renderer_node_id: String,
    overrides: RenderOverrides,
    scene_revision: u64,
    presentation: Mutex<RuntimePresentation>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RuntimePresentationKey {
    revision: u64,
    camera: Camera,
    size: [usize; 2],
}

struct PendingRuntimePresentation {
    ticket: u64,
    key: RuntimePresentationKey,
}

#[derive(Default)]
struct RuntimePresentation {
    pending: Option<PendingRuntimePresentation>,
    ready: Option<(RuntimePresentationKey, Frame)>,
    completed: Option<RuntimePresentationKey>,
    target: Option<std::sync::Arc<dyn crate::PresentationTarget>>,
    consumed_resource_commands: usize,
}

impl RuntimePresentation {
    fn poll(&mut self, renderer: &mut dyn FrameRenderer) -> Result<(), String> {
        if self.ready.is_some() {
            return Ok(());
        }
        if let Some(pending) = self.pending.take() {
            match renderer.poll_presented(pending.ticket) {
                Ok(Some(frame)) => self.ready = Some((pending.key, frame)),
                Ok(None) => self.pending = Some(pending),
                Err(error) => {
                    // An error does not prove that GPU resources are idle.
                    self.pending = Some(pending);
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

impl Runtime {
    /// Construct one viewport -> compound Software3D -> scene extraction -> World
    /// route. Missing connections and unsupported edits are reported as errors.
    pub fn from_document(
        document: &GraphDocument,
        registry: &ModuleRegistry,
    ) -> Result<Self, RuntimeError> {
        let diagnostics = validate_graph(document, registry);
        if diagnostics.iter().any(|d| d.severity == Severity::Error) {
            let message = diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(RuntimeError {
                message,
                diagnostics,
            });
        }
        let graph = &document.root;
        let route = registry.render_route(document).map_err(route_error)?;
        let viewport = route.viewport;
        let renderer = route.renderer;
        let world = route.world;
        let speed_source = graph
            .edges
            .iter()
            .find(|e| e.to_node == world.id && e.to_port == "speed")
            .map(|e| e.from_node.clone());
        let mut active = BTreeSet::new();
        let mut pending = vec![viewport.id.clone()];
        while let Some(id) = pending.pop() {
            if active.insert(id.clone()) {
                pending.extend(
                    graph
                        .edges
                        .iter()
                        .filter(|e| e.to_node == id)
                        .map(|e| e.from_node.clone()),
                );
            }
        }
        for node in graph.nodes.iter().filter(|n| active.contains(&n.id)) {
            if registry.is_world_provider(&node.module) || registry.is_renderer(&node.module) {
                continue;
            }
            match node.module.as_str() {
                "scene.extract" | "viewport.output" => {}
                "custom.rust.update" => {
                    require_parameter_keys(
                        node,
                        &["source_id", "code_layout", "code_layout_keys"],
                    )?;
                    if node.subgraph.is_some() {
                        return Err(route_error(
                            "Rust update uses source, not a module subgraph",
                        ));
                    }
                }
                "scalar.value" | "clock.time" | "custom.rust" => {
                    if node.subgraph.is_some() {
                        return Err(route_error(format!(
                            "Scalar module '{}' does not execute nested graphs",
                            node.id
                        )));
                    }
                    if node.module == "clock.time" {
                        require_no_params(node)?;
                    }
                    if node.module == "scalar.value" {
                        require_parameter_keys(node, &["value"])?;
                    }
                    if node.module == "custom.rust" {
                        require_parameter_keys(
                            node,
                            &["source_id", "code_layout", "code_layout_keys"],
                        )?;
                    }
                }
                _ => {
                    return Err(route_error(format!(
                        "Active module '{}' has no supported execution route",
                        node.module
                    )));
                }
            }
        }
        let provider = registry
            .create_world(&world.module, &world.params)
            .map_err(route_error)?;
        let executable_renderer = registry
            .create_renderer(renderer, document)
            .map_err(route_error)?;
        validate_live_properties(document, provider.as_ref()).map_err(route_error)?;
        if document.engine.initialized {
            // A permitted preset must be executable before the engine exposes it.
            let mut groups = document.engine.exposure.variants.values();
            let mut checks = vec![crate::project_commands(document, 0.0).map_err(route_error)?];
            for group in &mut groups {
                for variant in group.options.values() {
                    checks.push(
                        variant
                            .commands
                            .iter()
                            .cloned()
                            .map(ControlCommand::Render)
                            .collect(),
                    );
                }
            }
            for commands in checks {
                let render = commands
                    .into_iter()
                    .filter_map(|c| match c {
                        ControlCommand::Render(c) => Some(c),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let mut overrides = RenderOverrides::default();
                overrides.apply_commands(&render).map_err(route_error)?;
                executable_renderer
                    .validate_overrides(&overrides)
                    .map_err(route_error)?;
            }
        }
        let kind = registry
            .world_metadata(&world.module)
            .expect("Selected registered world")
            .kind;
        Ok(Self {
            provider,
            kind,
            world_node_id: world.id.clone(),
            world_module_id: world.module.clone(),
            speed_source,
            document: document.clone(),
            registry: registry.clone(),
            renderer: Mutex::new(executable_renderer),
            renderer_node_id: renderer.id.clone(),
            overrides: RenderOverrides::default(),
            scene_revision: 0,
            presentation: Mutex::new(RuntimePresentation::default()),
        })
    }

    /// Advance with a host-evaluated Scalar value. Invalid numeric values are
    /// ignored; dt is clamped to zero on negative inputs, without truncating long
    /// valid steps. The host owns wall-clock scheduling.
    pub fn step(&mut self, dt: f32, speed: f32) {
        if dt.is_finite() && speed.is_finite() {
            self.provider.step(dt.max(0.0), speed);
            self.scene_revision = self.scene_revision.wrapping_add(1);
        }
    }

    /// Execute the supported extraction/rasterization route into RGBA8.
    pub fn render(&self, width: usize, height: usize, camera: Camera) -> Result<Frame, String> {
        let mut renderer = self
            .renderer
            .lock()
            .map_err(|_| "Renderer lock is poisoned".to_string())?;
        let mut presentation = self
            .presentation
            .lock()
            .map_err(|_| "Presentation lock is poisoned".to_string())?;
        if presentation.pending.is_some() {
            return Err(
                "Diagnostic readback is unavailable while a presentation is pending".into(),
            );
        }
        let mut overrides = self.overrides.clone();
        let consumed = presentation
            .consumed_resource_commands
            .min(overrides.resource_commands.len());
        overrides.resource_commands.drain(..consumed);
        let frame =
            renderer.render(&self.provider.snapshot(), camera, width, height, &overrides)?;
        presentation.consumed_resource_commands = self.overrides.resource_commands.len();
        Ok(frame)
    }

    pub fn backend_name(&self) -> String {
        self.renderer
            .lock()
            .map(|r| r.backend_name().to_owned())
            .unwrap_or_else(|_| "Unavailable renderer".into())
    }

    pub fn set_presentation_target(
        &self,
        target: Option<std::sync::Arc<dyn crate::PresentationTarget>>,
    ) -> Result<(), String> {
        let mut renderer = self
            .renderer
            .lock()
            .map_err(|_| "Renderer lock is poisoned".to_string())?;
        let mut presentation = self
            .presentation
            .lock()
            .map_err(|_| "Presentation lock is poisoned".to_string())?;
        let unchanged = match (&presentation.target, &target) {
            (None, None) => true,
            (Some(previous), Some(candidate)) => std::sync::Arc::ptr_eq(previous, candidate),
            _ => false,
        };
        if unchanged {
            return Ok(());
        }
        if presentation.pending.is_some() || presentation.ready.is_some() {
            return Err("Cannot replace presentation target while a frame is pending".into());
        }
        renderer.set_presentation_target(target.clone())?;
        presentation.target = target;
        Ok(())
    }

    pub fn has_pending_presentation(&self) -> bool {
        self.presentation
            .lock()
            .is_ok_and(|state| state.pending.is_some())
    }

    pub fn has_ready_authored_presentation(&self) -> bool {
        self.presentation
            .lock()
            .is_ok_and(|state| state.ready.is_some())
    }

    /// Hidden panes still reap producer work. Retain the frozen completion until
    /// their next render call, without submitting a new request or changing its
    /// displayed camera metadata.
    pub fn poll_presentation(&self) -> Result<(), String> {
        let mut renderer = self
            .renderer
            .lock()
            .map_err(|_| "Renderer lock is poisoned".to_string())?;
        self.presentation
            .lock()
            .map_err(|_| "Presentation lock is poisoned".to_string())?
            .poll(renderer.as_mut())
    }

    /// Metadata belongs to the last completed frame, never to a newer request.
    pub fn last_presented_camera(&self) -> Option<Camera> {
        self.presentation
            .lock()
            .ok()?
            .completed
            .map(|key| key.camera)
    }

    pub fn last_presented_revision(&self) -> Option<u64> {
        self.presentation
            .lock()
            .ok()?
            .completed
            .map(|key| key.revision)
    }

    /// Native presentation retains the CPU-only `render` route for diagnostics.
    /// Polling never waits. Admit changed input in the completion call so a
    /// continuously moving view does not alternate completion and submission.
    pub fn render_presented(
        &self,
        width: usize,
        height: usize,
        camera: Camera,
    ) -> Result<Option<Frame>, String> {
        let mut renderer = self
            .renderer
            .lock()
            .map_err(|_| "Renderer lock is poisoned".to_string())?;
        let mut presentation = self
            .presentation
            .lock()
            .map_err(|_| "Presentation lock is poisoned".to_string())?;
        let key = RuntimePresentationKey {
            revision: self.scene_revision,
            camera,
            size: [width, height],
        };
        presentation.poll(renderer.as_mut())?;
        let mut completed = None;
        if let Some((completed_key, frame)) = presentation.ready.take() {
            presentation.completed = Some(completed_key);
            completed = Some(frame);
            if completed_key == key {
                return Ok(completed);
            }
        }
        if presentation.pending.is_some() {
            return Ok(completed);
        }
        let mut overrides = self.overrides.clone();
        let consumed = presentation
            .consumed_resource_commands
            .min(overrides.resource_commands.len());
        overrides.resource_commands.drain(..consumed);
        match renderer.render_presented(
            &self.provider.snapshot(),
            camera,
            width,
            height,
            &overrides,
        ) {
            Ok(RenderSubmission::Ready(frame)) => {
                presentation.consumed_resource_commands = self.overrides.resource_commands.len();
                presentation.completed = Some(key);
                Ok(Some(frame))
            }
            Ok(RenderSubmission::Pending(ticket)) => {
                presentation.consumed_resource_commands = self.overrides.resource_commands.len();
                presentation.pending = Some(PendingRuntimePresentation { ticket, key });
                Ok(completed)
            }
            Ok(RenderSubmission::Busy) => Ok(completed),
            // Publish a completed sample even if admitting its successor fails.
            // A retry of that newer sample can report its own error.
            Err(_) if completed.is_some() => Ok(completed),
            Err(error) => Err(error),
        }
    }

    pub fn objects(&self) -> Result<Vec<WorldObject>, String> {
        self.provider.objects()
    }

    /// Whether this provider still belongs to the author's selected world source.
    /// This metadata-only check never constructs a provider or renderer. Hosts
    /// must not use an old provider's IDs while a different world is preparing.
    pub fn world_source_matches(&self, document: &GraphDocument) -> bool {
        let Ok(route) = self.registry.render_route(document) else {
            return false;
        };
        route.world.id == self.world_node_id
            && route.world.module == self.world_module_id
            && self
                .document
                .root
                .nodes
                .iter()
                .find(|node| node.id == self.world_node_id)
                .is_some_and(|accepted| accepted.params == route.world.params)
    }

    /// The project boundary uses the same gate for native UI and compiled Rust.
    /// Engine-authoring apply_controls remains available for trusted tooling.
    pub fn apply_project_controls(&mut self, commands: &[ControlCommand]) -> Result<(), String> {
        crate::validate_project_controls(&self.document, commands)?;
        self.apply_controls(commands)
    }

    /// Apply exposed values and the first matching ready-variant rule per group.
    /// The host calls this once per tick before project script commands.
    pub fn apply_project_settings(&mut self, time: f64) -> Result<(), String> {
        if !self.document.engine.initialized {
            return Ok(());
        }
        let commands = crate::project_commands(&self.document, time)?;
        self.apply_project_controls(&commands)
    }

    /// Commit validated active fields for actual live objects, independent of the
    /// selected Object/ECS storage. The archive is never sent to a script.
    pub fn set_properties(&mut self, properties: crate::PropertyStore) -> Result<(), String> {
        let mut candidate = self.document.clone();
        candidate.properties = properties;
        crate::validate_settings(&candidate, &self.registry)?;
        validate_live_properties(&candidate, self.provider.as_ref())?;
        self.document.properties = candidate.properties;
        Ok(())
    }

    /// Update project values/rules without allowing architecture edits.
    pub fn set_project_configuration(
        &mut self,
        project: crate::ProjectConfiguration,
    ) -> Result<(), String> {
        let mut candidate = self.document.clone();
        candidate.project = project;
        crate::validate_settings(&candidate, &self.registry)?;
        self.document.project = candidate.project;
        Ok(())
    }

    /// Fill the portable owned packet with typed settings and participating data.
    pub fn fill_script_context(&self, context: &mut crate::ScriptContext) {
        context.parameters = crate::resolved_parameters(&self.document);
        let objects = self
            .provider
            .objects()
            .map(|v| v.into_iter().map(|o| o.id).collect())
            .unwrap_or_else(|_| {
                self.provider
                    .snapshot()
                    .items
                    .into_iter()
                    .map(|o| o.id)
                    .collect()
            });
        context.properties = crate::resolved_properties(&self.document, &self.registry, &objects);
    }

    pub fn properties(&self) -> &crate::PropertyStore {
        &self.document.properties
    }

    /// Validate the render batch before changing the world. Each provider applies
    /// its world batch transactionally. Resource commands are per-frame actions.
    pub fn apply_controls(&mut self, commands: &[ControlCommand]) -> Result<(), String> {
        let world = commands
            .iter()
            .filter_map(|c| match c {
                ControlCommand::World(c) => Some(c.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let render = commands
            .iter()
            .filter_map(|c| match c {
                ControlCommand::Render(c) => Some(c.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut candidate = self.overrides.clone();
        candidate.apply_commands(&render)?;
        if !world.is_empty() {
            self.provider.validate_commands(&world)?;
        }
        self.renderer
            .lock()
            .map_err(|_| "Renderer lock is poisoned".to_string())?
            .validate_overrides(&candidate)?;
        if !world.is_empty() {
            self.provider.apply_commands(&world)?;
        }
        if !world.is_empty() || self.overrides != candidate {
            self.scene_revision = self.scene_revision.wrapping_add(1);
        }
        self.overrides = candidate;
        Ok(())
    }

    pub fn begin_frame(&mut self) {
        // Retain one-shot operations until the renderer admits their request.
        if let Ok(mut presentation) = self.presentation.lock() {
            let consumed = presentation
                .consumed_resource_commands
                .min(self.overrides.resource_commands.len());
            self.overrides.resource_commands.drain(..consumed);
            presentation.consumed_resource_commands = 0;
        }
    }

    /// Keep owned world state when only renderer/source/layout changes. Explicit
    /// world module or creation-parameter changes construct a fresh provider.
    pub fn adopt_world(&mut self, previous: &mut Self) -> bool {
        let same_params = self
            .document
            .root
            .nodes
            .iter()
            .find(|n| n.id == self.world_node_id)
            .map(|n| &n.params)
            == previous
                .document
                .root
                .nodes
                .iter()
                .find(|n| n.id == previous.world_node_id)
                .map(|n| &n.params);
        if self.world_module_id == previous.world_module_id
            && self.world_node_id == previous.world_node_id
            && same_params
        {
            std::mem::swap(&mut self.provider, &mut previous.provider);
            let mut retained = previous.overrides.clone();
            retained.begin_frame();
            let mut retained_commands = vec![];
            for (pass, parameters) in &retained.parameters {
                for (name, value) in parameters {
                    retained_commands.push(ControlCommand::Render(
                        crate::RenderCommand::SetParameter {
                            pass: pass.clone(),
                            name: name.clone(),
                            value: value.clone(),
                        },
                    ));
                }
            }
            for (pass, enabled) in &retained.enabled {
                retained_commands.push(ControlCommand::Render(crate::RenderCommand::SetEnabled {
                    pass: pass.clone(),
                    enabled: *enabled,
                }));
            }
            // Pass/resource identities may change with a replacement render graph.
            // Retain controls only when their contract still validates.
            if crate::validate_project_controls(&self.document, &retained_commands).is_ok()
                && self
                    .renderer
                    .lock()
                    .is_ok_and(|renderer| renderer.validate_overrides(&retained).is_ok())
            {
                self.overrides = retained;
            }
            true
        } else {
            false
        }
    }

    pub fn reconfigure(&mut self, document: &GraphDocument) -> Result<bool, RuntimeError> {
        let mut candidate = Self::from_document(document, &self.registry)?;
        let target = self
            .presentation
            .lock()
            .map_err(|_| route_error("Presentation lock is poisoned"))?
            .target
            .clone();
        candidate
            .set_presentation_target(target)
            .map_err(route_error)?;
        let kept = candidate.adopt_world(self);
        *self = candidate;
        Ok(kept)
    }

    /// Only connected command outputs are executed. One compiled source can feed
    /// both outputs without evaluating the user's update function twice.
    pub fn command_sources(&self) -> Vec<CommandSource> {
        let mut sources = std::collections::BTreeMap::<String, CommandSource>::new();
        for edge in &self.document.root.edges {
            let world = edge.to_node == self.world_node_id && edge.to_port == "commands";
            let render = edge.to_node == self.renderer_node_id && edge.to_port == "commands";
            if !(world || render) {
                continue;
            }
            let Some(node) = self
                .document
                .root
                .nodes
                .iter()
                .find(|n| n.id == edge.from_node && n.module == "custom.rust.update")
            else {
                continue;
            };
            let Some(source_id) = node
                .params
                .get("source_id")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let source = sources
                .entry(source_id.to_owned())
                .or_insert(CommandSource {
                    source_id: source_id.to_owned(),
                    world: false,
                    render: false,
                });
            source.world |= world;
            source.render |= render;
        }
        sources.into_values().collect()
    }

    /// The selected built-in storage type, or Custom for a registered provider.
    pub fn world_kind(&self) -> WorldKind {
        self.kind
    }

    /// Registered executable source ID selected by the actual graph route.
    pub fn world_module_id(&self) -> &str {
        &self.world_module_id
    }

    /// Selected world node ID; useful for graph-driven host scalar evaluation.
    pub fn world_node_id(&self) -> &str {
        &self.world_node_id
    }

    /// Upstream node connected to speed, or None if the host should use its
    /// documented unconnected default. Pin type has already been validated.
    pub fn world_speed_source(&self) -> Option<String> {
        self.speed_source.clone()
    }

    /// Current neutral scene count rather than a hard-coded demo count.
    pub fn object_count(&self) -> usize {
        self.provider.snapshot().items.len()
    }

    /// Deterministic neutral scene for verification, exporting and rendering.
    pub fn snapshot(&self) -> SceneSnapshot {
        self.provider.snapshot()
    }

    /// Switch between installed built-in worlds and reset their simulation state.
    /// Custom leaves the current provider unchanged; a custom module is selected
    /// by changing its graph module ID and rebuilding with from_document.
    pub fn set_world_kind(&mut self, kind: WorldKind) {
        let module = match kind {
            WorldKind::Objects => "world.objects",
            WorldKind::Ecs => "world.ecs",
            WorldKind::Custom => return,
        };
        let mut document = self.document.clone();
        if let Some(world) = document
            .root
            .nodes
            .iter_mut()
            .find(|n| n.id == self.world_node_id)
        {
            world.module = module.into();
            world.params = serde_json::json!({});
        }
        let _ = self.reconfigure(&document);
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Ok(state) = self.presentation.get_mut()
            && let Some(pending) = state.pending.take()
            && let Ok(renderer) = self.renderer.get_mut()
        {
            // The backend retains execution resources until real completion.
            let _ = renderer.discard_presented(pending.ticket);
        }
    }
}

fn validate_live_properties(
    document: &GraphDocument,
    provider: &dyn WorldProvider,
) -> Result<(), String> {
    let ids: BTreeSet<_> = provider
        .objects()
        .map(|objects| objects.into_iter().map(|o| o.id).collect())
        .unwrap_or_else(|_| {
            provider
                .snapshot()
                .items
                .into_iter()
                .map(|o| o.id)
                .collect()
        });
    for id in document
        .properties
        .active
        .objects
        .keys()
        .chain(document.properties.active.components.keys())
    {
        if !ids.contains(id) {
            return Err(format!(
                "Active properties refer to missing world object {id}"
            ));
        }
    }
    Ok(())
}

fn route_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError {
        message: message.into(),
        diagnostics: vec![],
    }
}

fn upstream<'a>(
    graph: &'a Graph,
    node: &Node,
    input: &str,
    expected_output: &str,
) -> Result<&'a Node, RuntimeError> {
    let mut edges = graph
        .edges
        .iter()
        .filter(|e| e.to_node == node.id && e.to_port == input);
    let edge = edges
        .next()
        .ok_or_else(|| route_error(format!("Missing input {}.{}", node.id, input)))?;
    if edges.next().is_some() {
        return Err(route_error(format!(
            "Duplicate input {}.{}",
            node.id, input
        )));
    }
    if edge.from_port != expected_output {
        return Err(route_error(format!(
            "Unsupported source output '{}' for {}.{}",
            edge.from_port, node.id, input
        )));
    }
    let mut sources = graph.nodes.iter().filter(|n| n.id == edge.from_node);
    let source = sources
        .next()
        .ok_or_else(|| route_error("Source node disappeared"))?;
    if sources.next().is_some() {
        return Err(route_error("Source node is duplicated"));
    }
    Ok(source)
}

fn require_parameter_keys(node: &Node, supported: &[&str]) -> Result<(), RuntimeError> {
    if node.params.is_null() {
        return Ok(());
    }
    let object = node
        .params
        .as_object()
        .ok_or_else(|| route_error(format!("Parameters of '{}' must be an object", node.id)))?;
    if let Some(key) = object.keys().find(|k| !supported.contains(&k.as_str())) {
        return Err(route_error(format!(
            "Module '{}' does not execute parameter '{}'",
            node.module, key
        )));
    }
    Ok(())
}

fn require_no_params(node: &Node) -> Result<(), RuntimeError> {
    require_parameter_keys(node, &[])
}

pub(crate) fn validate_software_renderer(node: &Node) -> Result<(), RuntimeError> {
    require_no_params(node)?;
    let graph = node
        .subgraph
        .as_deref()
        .ok_or_else(|| route_error("Software3D needs its internal graph"))?;
    if graph.nodes.len() != 3 || graph.edges.len() != 2 {
        return Err(route_error(
            "Software3D currently executes exactly input -> rasterize -> output; edited passes are not implemented",
        ));
    }
    let output = graph
        .nodes
        .iter()
        .find(|n| n.module == "interface.output")
        .ok_or_else(|| route_error("Renderer has no output interface"))?;
    let rasterizer = upstream(graph, output, "value", "frame")?;
    if rasterizer.module != "render.rasterize" {
        return Err(route_error(
            "Software3D output must come from render.rasterize",
        ));
    }
    require_no_params(rasterizer)?;
    if rasterizer.subgraph.is_some() {
        return Err(route_error(
            "Nested rasterizer execution is not implemented",
        ));
    }
    let input = upstream(graph, rasterizer, "scene", "value")?;
    if input.module != "interface.input" || input.subgraph.is_some() || output.subgraph.is_some() {
        return Err(route_error(
            "Software3D needs a direct input interface -> rasterizer -> output interface route",
        ));
    }
    require_parameter_keys(input, &["port", "data_type"])?;
    require_parameter_keys(output, &["port", "data_type"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Mesh, RenderItem, Transform, demo_document, make_edge, make_node, world_descriptor,
    };
    use std::{collections::BTreeMap, sync::Arc};

    #[derive(Default)]
    struct AsyncRendererState {
        ready: bool,
        busy: bool,
        submitted: Vec<(Camera, [usize; 2], usize)>,
        frames: BTreeMap<u64, Frame>,
        discarded: Vec<u64>,
    }

    struct AsyncRenderer(Arc<Mutex<AsyncRendererState>>);

    impl FrameRenderer for AsyncRenderer {
        fn render(
            &mut self,
            scene: &SceneSnapshot,
            camera: Camera,
            width: usize,
            height: usize,
            _: &RenderOverrides,
        ) -> Result<Frame, String> {
            Ok(crate::render_scene(scene, width, height, camera))
        }

        fn render_presented(
            &mut self,
            _: &SceneSnapshot,
            camera: Camera,
            width: usize,
            height: usize,
            overrides: &RenderOverrides,
        ) -> Result<RenderSubmission, String> {
            let mut state = self.0.lock().unwrap();
            if state.busy {
                return Ok(RenderSubmission::Busy);
            }
            state
                .submitted
                .push((camera, [width, height], overrides.resource_commands.len()));
            let ticket = state.submitted.len() as u64;
            state.frames.insert(
                ticket,
                Frame {
                    width,
                    height,
                    rgba: vec![ticket as u8; width * height * 4],
                    gpu: None,
                    stats: Default::default(),
                },
            );
            Ok(RenderSubmission::Pending(ticket))
        }

        fn poll_presented(&mut self, ticket: u64) -> Result<Option<Frame>, String> {
            let mut state = self.0.lock().unwrap();
            Ok(if state.ready {
                state.frames.remove(&ticket)
            } else {
                None
            })
        }

        fn discard_presented(&mut self, ticket: u64) -> Result<(), String> {
            self.0.lock().unwrap().discarded.push(ticket);
            Ok(())
        }

        fn backend_name(&self) -> &str {
            "Deterministic asynchronous test renderer"
        }
        fn validate_overrides(&self, _: &RenderOverrides) -> Result<(), String> {
            Ok(())
        }
    }

    fn async_runtime() -> (Runtime, Arc<Mutex<AsyncRendererState>>) {
        let mut runtime = Runtime::from_document(
            &demo_document(WorldKind::Objects),
            &ModuleRegistry::builtin(),
        )
        .unwrap();
        let state = Arc::new(Mutex::new(AsyncRendererState::default()));
        runtime.renderer = Mutex::new(Box::new(AsyncRenderer(state.clone())));
        (runtime, state)
    }

    #[test]
    fn asynchronous_runtime_keeps_submitted_camera_and_revision_and_admits_next_on_completion() {
        let (mut runtime, state) = async_runtime();
        let first = Camera::default();
        let second = Camera {
            yaw: first.yaw + 0.4,
            ..first
        };
        assert!(runtime.render_presented(32, 24, first).unwrap().is_none());
        assert!(runtime.has_pending_presentation());
        assert_eq!(runtime.last_presented_camera(), None);
        runtime.step(0.1, 1.);
        assert!(runtime.render_presented(40, 24, second).unwrap().is_none());
        assert_eq!(state.lock().unwrap().submitted.len(), 1);
        state.lock().unwrap().ready = true;
        let old = runtime.render_presented(40, 24, second).unwrap().unwrap();
        assert_eq!((old.width, old.rgba[0]), (32, 1));
        assert_eq!(runtime.last_presented_camera(), Some(first));
        assert_eq!(runtime.last_presented_revision(), Some(0));
        assert_eq!(state.lock().unwrap().submitted.len(), 2);
        assert!(runtime.has_pending_presentation());
        let current = runtime.render_presented(40, 24, second).unwrap().unwrap();
        assert_eq!((current.width, current.rgba[0]), (40, 2));
        assert_eq!(runtime.last_presented_camera(), Some(second));
        assert_eq!(runtime.last_presented_revision(), Some(1));
        assert!(!runtime.has_pending_presentation());
        assert_eq!(
            state.lock().unwrap().submitted.len(),
            2,
            "Completing unchanged static input must not create a perpetual submit loop"
        );
    }

    #[test]
    fn asynchronous_runtime_retains_unadmitted_commands_and_consumes_each_prefix_once() {
        let (mut runtime, state) = async_runtime();
        let clear = |name: &str| {
            ControlCommand::Render(crate::RenderCommand::ClearResource {
                name: name.into(),
                value: [0.; 4],
            })
        };
        runtime.apply_controls(&[clear("first")]).unwrap();
        state.lock().unwrap().busy = true;
        assert!(
            runtime
                .render_presented(16, 12, Camera::default())
                .unwrap()
                .is_none()
        );
        runtime.begin_frame();
        assert_eq!(runtime.overrides.resource_commands.len(), 1);
        state.lock().unwrap().busy = false;
        runtime.render_presented(16, 12, Camera::default()).unwrap();
        runtime.apply_controls(&[clear("second")]).unwrap();
        runtime.begin_frame();
        assert_eq!(runtime.overrides.resource_commands.len(), 1);
        assert!(
            runtime
                .render_presented(16, 12, Camera::default())
                .unwrap()
                .is_none()
        );
        runtime.begin_frame();
        assert_eq!(runtime.overrides.resource_commands.len(), 1);
        state.lock().unwrap().ready = true;
        runtime
            .render_presented(16, 12, Camera::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            state
                .lock()
                .unwrap()
                .submitted
                .iter()
                .map(|entry| entry.2)
                .collect::<Vec<_>>(),
            vec![1, 1]
        );
        runtime.begin_frame();
        assert!(runtime.overrides.resource_commands.is_empty());
        runtime
            .render_presented(16, 12, Camera::default())
            .unwrap()
            .unwrap();
        assert_eq!(state.lock().unwrap().submitted.len(), 2);
    }

    #[test]
    fn asynchronous_runtime_hidden_poll_buffers_completion_without_new_work_or_camera_relabeling() {
        let (runtime, state) = async_runtime();
        let camera = Camera::default();
        runtime.render_presented(16, 12, camera).unwrap();
        runtime.poll_presentation().unwrap();
        assert!(runtime.has_pending_presentation());
        state.lock().unwrap().ready = true;
        runtime.poll_presentation().unwrap();
        assert!(!runtime.has_pending_presentation());
        assert!(runtime.has_ready_authored_presentation());
        assert_eq!(runtime.last_presented_camera(), None);
        for _ in 0..3 {
            runtime.poll_presentation().unwrap();
        }
        assert_eq!(state.lock().unwrap().submitted.len(), 1);
        let frame = runtime.render_presented(16, 12, camera).unwrap().unwrap();
        assert_eq!(frame.rgba[0], 1);
        assert!(!runtime.has_ready_authored_presentation());
        assert_eq!(runtime.last_presented_camera(), Some(camera));
        assert_eq!(state.lock().unwrap().submitted.len(), 1);
        // A later explicit idle refresh must still enter the renderer so its
        // ordinary external HLSL dependency check remains reachable.
        assert!(runtime.render_presented(16, 12, camera).unwrap().is_none());
        assert_eq!(state.lock().unwrap().submitted.len(), 2);
    }

    struct TestPresentationTarget;
    impl crate::PresentationTarget for TestPresentationTarget {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[test]
    fn asynchronous_runtime_rejects_target_or_readback_conflicts_and_discards_replaced_ticket() {
        let (mut runtime, state) = async_runtime();
        let target: Arc<dyn crate::PresentationTarget> = Arc::new(TestPresentationTarget);
        runtime
            .set_presentation_target(Some(target.clone()))
            .unwrap();
        runtime.render_presented(16, 12, Camera::default()).unwrap();
        runtime
            .set_presentation_target(Some(target.clone()))
            .unwrap();
        assert!(
            runtime
                .set_presentation_target(None)
                .unwrap_err()
                .contains("pending")
        );
        assert!(
            runtime
                .render(16, 12, Camera::default())
                .unwrap_err()
                .contains("pending")
        );
        let mut rejected = demo_document(WorldKind::Objects);
        rejected.root.edges.clear();
        assert!(runtime.reconfigure(&rejected).is_err());
        assert!(runtime.has_pending_presentation());
        assert!(state.lock().unwrap().discarded.is_empty());
        runtime
            .reconfigure(&demo_document(WorldKind::Objects))
            .unwrap();
        assert_eq!(state.lock().unwrap().discarded, vec![1]);
        assert!(!runtime.has_pending_presentation());
        assert_eq!(runtime.last_presented_camera(), None);
        let retained = runtime.presentation.lock().unwrap().target.clone().unwrap();
        assert!(Arc::ptr_eq(&retained, &target));
    }

    #[test]
    fn live_property_ids_are_bound_to_the_accepted_world_source() {
        let registry = ModuleRegistry::builtin();
        let document = demo_document(WorldKind::Objects);
        let runtime = Runtime::from_document(&document, &registry).unwrap();
        let mut candidate = document.clone();
        candidate.name = "Renamed project".into();
        candidate.root.nodes[0].position = [100., 200.];
        candidate
            .properties
            .active
            .global
            .insert("unrelated".into(), serde_json::json!(9));
        assert!(runtime.world_source_matches(&candidate));
        let world = candidate
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "world")
            .unwrap();
        world.module = "world.ecs".into();
        assert!(!runtime.world_source_matches(&candidate));
        candidate = document.clone();
        candidate
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "world")
            .unwrap()
            .params = serde_json::json!({"changed": true});
        assert!(!runtime.world_source_matches(&candidate));
        candidate = document;
        candidate
            .root
            .edges
            .retain(|edge| edge.to_node != "extract");
        assert!(!runtime.world_source_matches(&candidate));
    }

    #[test]
    fn graph_selects_world_and_speed_source_and_renders_identically() {
        let registry = ModuleRegistry::builtin();
        let mut objects =
            Runtime::from_document(&demo_document(WorldKind::Objects), &registry).unwrap();
        let mut ecs = Runtime::from_document(&demo_document(WorldKind::Ecs), &registry).unwrap();
        assert_eq!(objects.world_kind(), WorldKind::Objects);
        assert_eq!(ecs.world_kind(), WorldKind::Ecs);
        assert_eq!(objects.world_speed_source().as_deref(), Some("speed"));
        objects.step(0.2, 0.7);
        ecs.step(0.2, 0.7);
        assert_eq!(objects.snapshot(), ecs.snapshot());
        assert_eq!(
            objects.render(160, 120, Camera::default()).unwrap().rgba,
            ecs.render(160, 120, Camera::default()).unwrap().rgba
        );
        let mut d = demo_document(WorldKind::Objects);
        d.root
            .edges
            .retain(|e| !(e.to_node == "world" && e.to_port == "speed"));
        d.root
            .edges
            .push(make_edge("base", "value", "world", "speed"));
        let runtime = Runtime::from_document(&d, &registry).unwrap();
        assert_eq!(runtime.world_speed_source().as_deref(), Some("base"));
    }

    #[test]
    fn a_third_source_provider_works_without_core_or_renderer_changes() {
        struct SingleCube;
        impl WorldProvider for SingleCube {
            fn step(&mut self, _: f32, _: f32) {}
            fn snapshot(&self) -> SceneSnapshot {
                SceneSnapshot {
                    items: vec![RenderItem {
                        material: Default::default(),
                        id: 90,
                        mesh: Mesh::cube(),
                        model: Transform::default().matrix(),
                        color: [250, 140, 60, 255],
                    }],
                    time: 0.0,
                }
            }
        }
        let mut registry = ModuleRegistry::builtin();
        registry
            .register_world(
                world_descriptor("test.single", "Single cube", "Independent test provider"),
                |_| Ok(Box::new(SingleCube)),
            )
            .unwrap();
        let mut d = demo_document(WorldKind::Objects);
        d.root
            .nodes
            .iter_mut()
            .find(|n| n.id == "world")
            .unwrap()
            .module = "test.single".into();
        d.module_versions.insert(
            "test.single".into(),
            registry.module_version("test.single").unwrap().into(),
        );
        let runtime = Runtime::from_document(&d, &registry).unwrap();
        assert_eq!(runtime.world_kind(), WorldKind::Custom);
        assert_eq!(runtime.world_module_id(), "test.single");
        assert_eq!(runtime.object_count(), 1);
        assert!(
            runtime
                .render(128, 128, Camera::default())
                .unwrap()
                .stats
                .shaded_pixels
                > 100
        );
    }

    #[test]
    fn valid_but_unsupported_renderer_edits_are_rejected() {
        let registry = ModuleRegistry::builtin();
        let mut d = demo_document(WorldKind::Objects);
        let internal = d
            .root
            .nodes
            .iter_mut()
            .find(|n| n.id == "renderer")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap();
        internal.nodes.push(make_node(
            "unused-scalar",
            "scalar.value",
            "Extra pass",
            [0., 0.],
            serde_json::json!({"value":2.0}),
        ));
        assert!(
            !validate_graph(&d, &registry)
                .iter()
                .any(|d| d.severity == Severity::Error)
        );
        let Err(error) = Runtime::from_document(&d, &registry) else {
            panic!("expected runtime rejection");
        };
        assert!(error.message.contains("edited passes"));
    }

    #[test]
    fn incompatible_module_pin_and_unsupported_render_batch_are_explicit_errors() {
        let registry = ModuleRegistry::builtin();
        let mut document = demo_document(WorldKind::Objects);
        document
            .module_versions
            .insert("world.objects".into(), "999.0.0".into());
        assert!(
            Runtime::from_document(&document, &registry)
                .err()
                .unwrap()
                .message
                .contains("exact version pin")
        );
        document.module_versions.clear();
        assert!(Runtime::from_document(&document, &registry).is_err());
        let document = demo_document(WorldKind::Objects);
        let mut runtime = Runtime::from_document(&document, &registry).unwrap();
        let before = runtime.objects().unwrap();
        let commands = vec![
            ControlCommand::World(crate::WorldCommand::Translate {
                id: 1,
                delta: [1., 0., 0.],
            }),
            ControlCommand::Render(crate::RenderCommand::SetEnabled {
                pass: "nonexistent".into(),
                enabled: false,
            }),
        ];
        assert!(runtime.apply_controls(&commands).is_err());
        assert_eq!(runtime.objects().unwrap(), before);
    }
}
