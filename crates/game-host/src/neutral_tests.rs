//! Shared-host contracts exercised with native source CPU/world/service fixtures.
//! No test in this module constructs Vulkan, a window, or platform audio.
use crate::{
    activity::ActivityState,
    game_host::{GameHost, HostOptions, RenderMode},
    game_worker::{GameSnapshot, GameWorker, WorkerCommand},
};
use module_engine_core::*;
use module_engine_host_services::{HostService, HostServiceRegistry};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(10);

struct Control {
    factories: AtomicUsize,
    candidates: AtomicUsize,
    world_factories: AtomicUsize,
    service_factories: AtomicUsize,
    reject_generation: AtomicU64,
    delayed_generation: AtomicU64,
    fail_render: AtomicBool,
    dependency: Mutex<Result<u64, String>>,
    started: Mutex<BTreeSet<u64>>,
    released: Mutex<bool>,
    gate: Condvar,
    dropped: Mutex<Vec<(u64, thread::ThreadId)>>,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            factories: AtomicUsize::new(0),
            candidates: AtomicUsize::new(0),
            world_factories: AtomicUsize::new(0),
            service_factories: AtomicUsize::new(0),
            reject_generation: AtomicU64::new(u64::MAX),
            delayed_generation: AtomicU64::new(u64::MAX),
            fail_render: AtomicBool::new(false),
            dependency: Mutex::new(Ok(0)),
            started: Mutex::new(BTreeSet::new()),
            released: Mutex::new(false),
            gate: Condvar::new(),
            dropped: Mutex::new(vec![]),
        }
    }
}
impl Control {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.gate.notify_all();
    }
}

struct CpuAdapter(Arc<Control>);
fn cpu_config(node: &Node) -> Result<(u64, [u8; 3]), String> {
    Ok((
        node.params["generation"]
            .as_u64()
            .ok_or("CPU generation must be an integer")?,
        serde_json::from_value(node.params["tint"].clone())
            .map_err(|error| format!("CPU tint: {error}"))?,
    ))
}
fn cpu_controls(controls: &RenderOverrides) -> Result<(), String> {
    if !controls.enabled.is_empty()
        || !controls.resource_commands.is_empty()
        || controls.parameters.iter().any(|(pass, values)| {
            pass != "tint"
                || values.iter().any(|(name, value)| {
                    name != "red"
                        || !value
                            .as_f64()
                            .is_some_and(|value| (0.0..=1.0).contains(&value))
                })
        })
    {
        return Err("CPU source rejected render control".into());
    }
    Ok(())
}
impl RendererAdapter for CpuAdapter {
    fn dependency_revision(&self, _: &Node, _: &GraphDocument) -> Result<u64, String> {
        self.0.dependency.lock().unwrap().clone()
    }
    fn control_targets(
        &self,
        _: &Node,
        _: &GraphDocument,
    ) -> Result<Option<RenderControlTargets>, String> {
        Ok(Some(RenderControlTargets {
            controls: BTreeSet::from(["tint".into()]),
            resources: BTreeSet::new(),
        }))
    }
    fn validate_controls(
        &self,
        node: &Node,
        _: &GraphDocument,
        controls: &RenderOverrides,
    ) -> Result<(), String> {
        cpu_config(node)?;
        cpu_controls(controls)
    }
}
struct CpuRenderer {
    control: Arc<Control>,
    generation: u64,
    tint: [u8; 3],
    renders: u64,
}
impl CpuRenderer {
    fn candidate(node: &Node, control: Arc<Control>) -> Result<Self, String> {
        let (generation, tint) = cpu_config(node)?;
        if control.reject_generation.load(Ordering::SeqCst) == generation {
            return Err("Injected CPU candidate rejection".into());
        }
        Ok(Self {
            control,
            generation,
            tint,
            renders: 0,
        })
    }
}
impl FrameRenderer for CpuRenderer {
    fn render(
        &mut self,
        scene: &SceneSnapshot,
        camera: Camera,
        width: usize,
        height: usize,
        controls: &RenderOverrides,
    ) -> Result<Frame, String> {
        cpu_controls(controls)?;
        if self.control.fail_render.load(Ordering::SeqCst) {
            return Err("Injected CPU draw failure".into());
        }
        let mut scene = scene.clone();
        let red = controls
            .parameters
            .get("tint")
            .and_then(|values| values.get("red"))
            .and_then(Value::as_f64)
            .unwrap_or(1.) as f32;
        for item in &mut scene.items {
            for (channel, tint) in self.tint.into_iter().enumerate() {
                item.color[channel] = ((item.color[channel] as f32 * tint as f32 / 255.)
                    * if channel == 0 { red } else { 1. })
                .round() as u8;
            }
        }
        self.renders += 1;
        Ok(render_scene(&scene, width, height, camera))
    }
    fn backend_name(&self) -> &str {
        "Fixture CPU 3D"
    }
    fn performance(&self) -> RendererPerformance {
        RendererPerformance {
            render_calls: self.renders,
            prepare_calls: self.control.candidates.load(Ordering::SeqCst) as u64,
            ..Default::default()
        }
    }
    fn prepare_candidate(
        &mut self,
        node: &Node,
        _: &GraphDocument,
    ) -> Result<Option<Box<dyn FrameRenderer>>, String> {
        self.control.candidates.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Box::new(Self::candidate(node, self.control.clone())?)))
    }
    fn validate_overrides(&self, overrides: &RenderOverrides) -> Result<(), String> {
        cpu_controls(overrides)
    }
}
impl Drop for CpuRenderer {
    fn drop(&mut self) {
        self.control
            .dropped
            .lock()
            .unwrap()
            .push((self.generation, thread::current().id()));
    }
}

/// Independently stored source provider. Shared validation checks its portable
/// transaction; this BTreeMap owns and applies every persistent object field.
struct SourceWorld(BTreeMap<u64, WorldObject>);
impl WorldProvider for SourceWorld {
    fn step(&mut self, _: f32, _: f32) {}
    fn objects(&self) -> Result<Vec<WorldObject>, String> {
        Ok(self.0.values().cloned().collect())
    }
    fn snapshot(&self) -> SceneSnapshot {
        let mut items = vec![];
        for object in self.0.values() {
            let mut model = object.transform.matrix();
            let mut parent = object.parent;
            while let Some(id) = parent {
                let object = &self.0[&id];
                model = multiply_matrices(object.transform.matrix(), model);
                parent = object.parent;
            }
            items.push(RenderItem {
                id: object.id,
                mesh: Mesh::cube(),
                model,
                color: object.color,
                material: Material::default(),
            });
        }
        SceneSnapshot { items, time: 0. }
    }
    fn apply_commands(&mut self, commands: &[WorldCommand]) -> Result<(), String> {
        validate_world_commands(&self.objects()?, commands)?;
        let mut next = self.0.clone();
        for command in commands {
            match command {
                WorldCommand::Spawn { object } => {
                    next.insert(object.id, object.clone());
                }
                WorldCommand::Remove { id } => {
                    let mut removed = BTreeSet::from([*id]);
                    loop {
                        let previous = removed.len();
                        for object in next.values() {
                            if object.parent.is_some_and(|id| removed.contains(&id)) {
                                removed.insert(object.id);
                            }
                        }
                        if removed.len() == previous {
                            break;
                        }
                    }
                    next.retain(|id, _| !removed.contains(id));
                }
                WorldCommand::SetPosition { id, position } => {
                    next.get_mut(id).unwrap().transform.translation = *position
                }
                WorldCommand::Translate { id, delta } => {
                    let position = &mut next.get_mut(id).unwrap().transform.translation;
                    for (position, delta) in position.iter_mut().zip(delta) {
                        *position += *delta;
                    }
                }
                WorldCommand::SetTransform { id, transform } => {
                    next.get_mut(id).unwrap().transform = *transform
                }
                WorldCommand::SetColor { id, color } => next.get_mut(id).unwrap().color = *color,
            }
        }
        self.0 = next;
        Ok(())
    }
}
struct Step;
impl ExecutableSystem for Step {
    fn step(&mut self, context: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
        *state = json!(state.as_u64().unwrap_or(0) + 1);
        context
            .game
            .module_data
            .insert("fixture.steps".into(), state.clone());
        if context.input.pressed("pulse") {
            let pulses = context
                .game
                .module_data
                .get("fixture.pulses")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + 1;
            context
                .game
                .module_data
                .insert("fixture.pulses".into(), json!(pulses));
            context.game.world.transform_mut(1).unwrap().translation[0] += 0.25;
            context
                .events
                .push(GameEvent::new("fixture.pulse", json!({"count":pulses})));
        }
        Ok(())
    }
}

struct Fixture {
    root: PathBuf,
    document: GraphDocument,
    composition: CompositionContext,
    control: Arc<Control>,
}
impl Fixture {
    fn new(custom_world: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "ModuleEngine neutral host {} {}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let control = Arc::new(Control::default());
        let mut modules = ModuleRegistry::builtin();
        let built = control.clone();
        modules
            .register_renderer_with_adapter(
                ModuleDescriptor {
                    id: "render.test_cpu".into(),
                    label: "Independent CPU renderer".into(),
                    description: "Native test source".into(),
                    properties: vec![],
                    inputs: vec![PortDescriptor {
                        name: "scene".into(),
                        data_type: "Scene3D".into(),
                        required: true,
                    }],
                    outputs: vec![PortDescriptor {
                        name: "frame".into(),
                        data_type: "Frame3D".into(),
                        required: false,
                    }],
                },
                CpuAdapter(control.clone()),
                move |node, _| {
                    let (generation, _) = cpu_config(node)?;
                    built.factories.fetch_add(1, Ordering::SeqCst);
                    built.started.lock().unwrap().insert(generation);
                    if built.delayed_generation.load(Ordering::SeqCst) == generation {
                        let released = built.released.lock().unwrap();
                        let (released, _) = built
                            .gate
                            .wait_timeout_while(released, WAIT, |released| !*released)
                            .unwrap();
                        if !*released {
                            return Err("CPU factory gate timed out".into());
                        }
                    }
                    Ok(Box::new(CpuRenderer::candidate(node, built.clone())?))
                },
            )
            .unwrap();
        if custom_world {
            let built = control.clone();
            modules
                .register_world_with_metadata(
                    ModuleDescriptor {
                        id: "world.test_source".into(),
                        label: "Independent map world".into(),
                        description: "BTreeMap persistent store".into(),
                        properties: vec![],
                        inputs: vec![],
                        outputs: vec![PortDescriptor {
                            name: "world".into(),
                            data_type: "World".into(),
                            required: false,
                        }],
                    },
                    WorldMetadata::default(),
                    move |_| {
                        built.world_factories.fetch_add(1, Ordering::SeqCst);
                        let object = WorldObject {
                            id: 9999,
                            parent: None,
                            transform: Transform::default(),
                            color: [255; 4],
                        };
                        Ok(Box::new(SourceWorld(BTreeMap::from([(object.id, object)]))))
                    },
                )
                .unwrap();
        }
        let mut systems = SystemRegistry::builtin();
        systems
            .register(
                SystemDescriptor {
                    id: "fixture.step".into(),
                    state_schema: "fixture.step/1".into(),
                    components: vec![],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(Step)),
            )
            .unwrap();
        systems
            .register_game_contribution(GameContribution {
                id: "fixture.presentation".into(),
                activation_modules: BTreeSet::from(["fixture.step".into()]),
                begin_tick: |_| Ok(()),
                initial_camera: |_| Camera::default(),
                follow_target: |_| None,
                decorate_snapshot: |_, _| {},
                script_input: |input| serde_json::to_value(input).unwrap(),
                validate_event: |_| Ok(()),
                validate_state: |_| Ok(()),
                diagnostics: |_| json!({}),
                service_requests: |project| {
                    Ok(
                        if project
                            .execution
                            .stages
                            .iter()
                            .flat_map(|stage| &stage.nodes)
                            .any(|node| {
                                node.module == "fixture.step"
                                    && node.params["service"] == json!(true)
                            })
                        {
                            vec![HostServiceRequest {
                                id: "fixture.service".into(),
                                config: Value::Null,
                                asset_references: BTreeSet::new(),
                            }]
                        } else {
                            vec![]
                        },
                    )
                },
            })
            .unwrap();
        let composition = CompositionContext::new(modules, systems).unwrap();
        let mut document = demo_document(WorldKind::Ecs);
        document.name = "Neutral source host fixture".into();
        document.custom_sources.clear();
        document.root.nodes.retain(|node| {
            matches!(
                node.id.as_str(),
                "world" | "extract" | "renderer" | "viewport"
            )
        });
        document
            .root
            .edges
            .retain(|edge| matches!(edge.from_node.as_str(), "world" | "extract" | "renderer"));
        let node = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "renderer")
            .unwrap();
        node.module = "render.test_cpu".into();
        node.params = json!({"generation":0,"tint":[255,255,255]});
        node.subgraph = None;
        if custom_world {
            document
                .root
                .nodes
                .iter_mut()
                .find(|node| node.id == "world")
                .unwrap()
                .module = "world.test_source".into();
        }
        let mut game = GameProject::empty();
        game.scenes.get_mut("empty").unwrap().objects = vec![
            AuthoredObject::new(WorldObject {
                id: 1,
                parent: None,
                transform: Transform::default(),
                color: [210, 170, 110, 255],
            }),
            AuthoredObject::new(WorldObject {
                id: 2,
                parent: Some(1),
                transform: Transform {
                    translation: [1.5, 0., 0.],
                    scale: [0.35; 3],
                    ..Default::default()
                },
                color: [110, 190, 220, 255],
            }),
        ];
        game.execution.stages.push(SystemStage {
            name: "Source step".into(),
            nodes: vec![SystemNode {
                id: "step".into(),
                module: "fixture.step".into(),
                params: json!({}),
                bindings: BTreeMap::new(),
                after: vec![],
            }],
        });
        document.game = Some(game);
        approve_fixture(&composition, &mut document);
        Self {
            root,
            document,
            composition,
            control,
        }
    }
    fn options(
        &self,
        render: RenderMode,
        services: HostServiceRegistry,
        enabled: bool,
    ) -> HostOptions {
        HostOptions {
            asset_root: self.root.clone(),
            backend_root: None,
            compile_root: None,
            render,
            services,
            services_enabled: enabled,
        }
    }
    fn host(&self) -> GameHost {
        GameHost::build(
            self.document.clone(),
            self.composition.clone(),
            self.options(RenderMode::Graph, HostServiceRegistry::new(), false),
            None,
        )
        .unwrap()
    }
    fn candidate(&self, generation: u64, tint: [u8; 3]) -> GraphDocument {
        let mut document = self.document.clone();
        document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "renderer")
            .unwrap()
            .params = json!({"generation":generation,"tint":tint});
        document
    }
}
/// Explicit approval while constructing a new source fixture/profile choice.
fn approve_fixture(composition: &CompositionContext, document: &mut GraphDocument) {
    let profile = document.engine_lock.as_ref().unwrap().profile.clone();
    composition
        .capture_lock(document, profile, Some(SdkIdentity::current()))
        .unwrap()
        .apply_to(document);
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.control.release();
        let root = self.root.canonicalize().unwrap();
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        assert!(root.starts_with(&temporary) && root != temporary);
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ModuleEngine neutral host ")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for source fixture"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn complete(host: &mut GameHost, document: &GraphDocument) -> Result<(), String> {
    let deadline = Instant::now() + WAIT;
    while host.has_pending_preparation() {
        host.poll_preparation(document)?;
        assert!(Instant::now() < deadline, "CPU candidate did not complete");
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
fn send(worker: &GameWorker, mut command: WorkerCommand) {
    let deadline = Instant::now() + WAIT;
    loop {
        match worker.try_command(command) {
            Ok(()) => return,
            Err(returned) => command = returned,
        }
        assert!(
            Instant::now() < deadline,
            "Worker command queue remained unavailable"
        );
        thread::sleep(Duration::from_millis(1));
    }
}
fn press(worker: &GameWorker) -> u64 {
    let mut input = GameInput {
        actions: BTreeSet::from(["pulse".into()]),
        ..Default::default()
    };
    let deadline = Instant::now() + WAIT;
    loop {
        match worker.submit_input(input) {
            Ok(Some(sequence)) => return sequence,
            Ok(None) => panic!("Pressed input was not sequenced"),
            Err(returned) => input = returned,
        }
        assert!(
            Instant::now() < deadline,
            "Worker input queue remained unavailable"
        );
        thread::sleep(Duration::from_millis(1));
    }
}
fn snapshot(
    worker: &mut GameWorker,
    mut accepted: impl FnMut(&GameSnapshot) -> bool,
) -> Arc<GameSnapshot> {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(snapshot) = worker.latest()
            && accepted(&snapshot)
        {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "Worker did not publish the requested state"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn stop(worker: &GameWorker) {
    worker.shutdown();
    wait_until(|| !worker.is_alive());
}

#[test]
fn switching_renderer_module_uses_the_target_factory_instead_of_live_preparation() {
    let mut fixture = Fixture::new(false);
    let mut descriptor = fixture
        .composition
        .modules
        .descriptor("render.test_cpu")
        .unwrap()
        .clone();
    descriptor.id = "render.second_cpu".into();
    let control = fixture.control.clone();
    fixture
        .composition
        .modules
        .register_renderer_with_adapter(descriptor, CpuAdapter(control.clone()), move |node, _| {
            control.factories.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(CpuRenderer::candidate(node, control.clone())?))
        })
        .unwrap();
    let mut host = fixture.host();
    let original = host.render(160, 100).unwrap();
    let mut candidate = fixture.candidate(1, [255, 30, 30]);
    candidate
        .root
        .nodes
        .iter_mut()
        .find(|n| n.id == "renderer")
        .unwrap()
        .module = "render.second_cpu".into();
    approve_fixture(&fixture.composition, &mut candidate);
    host.sync_document(&candidate).unwrap();
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 0);
    assert_ne!(host.render(160, 100).unwrap().rgba, original.rgba);
    assert_eq!(host.accepted_document(), &candidate);
}

#[test]
fn registered_cpu_renderer_reuses_unchanged_configuration_and_validates_all_presets() {
    let fixture = Fixture::new(false);
    let mut host = fixture.host();
    let initial = host.render(160, 100).unwrap();
    assert!(initial.stats.shaded_pixels > 0 && initial.stats.visible_objects > 0);
    assert_eq!(host.device, "Fixture CPU 3D");
    let state = host.session.state().clone();
    host.sync_document(&fixture.document).unwrap();
    let mut layout = fixture.document.clone();
    layout.root.nodes[2].label = "Cosmetic".into();
    layout.root.nodes[2].position = [123., 456.];
    host.sync_document(&layout).unwrap();
    assert_eq!(host.render(160, 100).unwrap().rgba, initial.rgba);
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 0);
    let tinted = fixture.candidate(1, [255, 40, 40]);
    host.sync_document(&tinted).unwrap();
    let accepted = host.render(160, 100).unwrap();
    assert_ne!(accepted.rgba, initial.rgba);
    assert_eq!(host.session.state(), &state);
    assert_eq!(
        fixture.control.factories.load(Ordering::SeqCst),
        1,
        "Synchronous preparation should use the live renderer's candidate API"
    );
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 1);
    let mut invalid = tinted.clone();
    invalid.engine.exposure.variants.insert(
        "quality".into(),
        VariantGroup {
            label: "All preset preflight".into(),
            default: "good".into(),
            options: BTreeMap::from([
                (
                    "good".into(),
                    ReadyVariant {
                        label: "Good".into(),
                        commands: vec![RenderCommand::SetParameter {
                            pass: "tint".into(),
                            name: "red".into(),
                            value: json!(1.0),
                        }],
                    },
                ),
                (
                    "bad".into(),
                    ReadyVariant {
                        label: "Rejected unselected preset".into(),
                        commands: vec![RenderCommand::SetParameter {
                            pass: "unknown".into(),
                            name: "red".into(),
                            value: json!(0.5),
                        }],
                    },
                ),
            ]),
        },
    );
    let error = host.sync_document(&invalid).unwrap_err();
    assert!(
        error.contains("CPU source rejected render control"),
        "{error}"
    );
    assert_eq!(host.accepted_document(), &tinted);
    assert_eq!(host.render(160, 100).unwrap().rgba, accepted.rgba);
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 1);
    let mut unknown = tinted.clone();
    unknown
        .root
        .nodes
        .iter_mut()
        .find(|node| node.id == "renderer")
        .unwrap()
        .params["future_executable_parameter"] = json!(7);
    host.sync_document(&unknown).unwrap();
    assert_eq!(
        fixture.control.candidates.load(Ordering::SeqCst),
        2,
        "Unknown executable fields must remain in the conservative key"
    );
}

#[test]
fn exact_locks_reject_before_factories_and_all_candidate_paths_preserve_last_good() {
    let fixture = Fixture::new(false);
    let build = |document: GraphDocument| {
        GameHost::build(
            document,
            fixture.composition.clone(),
            fixture.options(RenderMode::Graph, HostServiceRegistry::new(), false),
            None,
        )
    };
    let mut missing = fixture.document.clone();
    missing.engine_lock = None;
    assert!(
        build(missing.clone())
            .err()
            .unwrap()
            .contains("explicit engine lock")
    );
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 0);
    let mut missing_pin = fixture.document.clone();
    missing_pin.module_versions.remove("fixture.step");
    assert!(build(missing_pin).err().unwrap().contains("fixture.step"));
    let mut missing_state = fixture.document.clone();
    missing_state
        .engine_lock
        .as_mut()
        .unwrap()
        .system_state_schemas
        .remove("fixture.step");
    assert!(
        build(missing_state)
            .err()
            .unwrap()
            .contains("state schema pin")
    );
    let mut missing_schema = fixture.document.clone();
    missing_schema
        .engine_lock
        .as_mut()
        .unwrap()
        .component_schemas
        .remove("mesh");
    assert!(build(missing_schema).err().unwrap().contains("schema pin"));
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 0);

    let mut host = fixture.host();
    let state = host.session.state().clone();
    let accepted = host.accepted_document().clone();
    let revision = host.presentation_revision();
    assert!(host.sync_document(&missing).is_err());
    assert!(host.sync_document_deferred(&missing).is_err());
    assert!(host.poll_preparation(&missing).is_err());
    assert!(host.refresh_author_preview(&missing).is_err());
    assert_eq!(host.session.state(), &state);
    assert_eq!(host.accepted_document(), &accepted);
    assert_eq!(host.presentation_revision(), revision);
    assert!(!host.has_pending_preparation());
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 0);

    let mut new_active = accepted.clone();
    new_active
        .engine
        .enabled_modules
        .insert("game.motion".into());
    assert!(
        host.sync_document(&new_active)
            .unwrap_err()
            .contains("game.motion")
    );
    assert!(!new_active.module_versions.contains_key("game.motion"));
    assert_eq!(host.accepted_document(), &accepted);
}

#[test]
fn worker_rejects_unpinned_activity_after_roundtrip_then_accepts_explicit_lock_update() {
    let fixture = Fixture::new(false);
    let mut worker = GameWorker::spawn(fixture.host(), None);
    send(
        &worker,
        WorkerCommand::SetViewport {
            width: 160,
            height: 100,
        },
    );
    let initial = snapshot(&mut worker, |status| status.frame.is_some());
    let mut candidate = fixture.document.clone();
    candidate
        .engine
        .enabled_modules
        .insert("game.motion".into());
    let ordinary_saved = serde_json::to_vec(&candidate).unwrap();
    let reopened: GraphDocument = serde_json::from_slice(&ordinary_saved).unwrap();
    assert!(!reopened.module_versions.contains_key("game.motion"));
    send(
        &worker,
        WorkerCommand::UpdateDocument {
            document: Arc::new(reopened.clone()),
            authoring: false,
            request: 70,
        },
    );
    let rejected = snapshot(&mut worker, |status| {
        status
            .results
            .iter()
            .any(|reply| reply.request == 70 && reply.result.is_err())
    });
    assert_eq!(
        rejected.accepted_document.as_ref(),
        initial.accepted_document.as_ref()
    );
    assert_eq!(rejected.state.as_ref(), initial.state.as_ref());
    assert_eq!(
        rejected.frame.as_ref().unwrap().rgba,
        initial.frame.as_ref().unwrap().rgba
    );
    assert_eq!(rejected.epoch, initial.epoch);
    assert!(
        rejected
            .preparation_error
            .as_ref()
            .unwrap()
            .contains("game.motion")
    );
    assert_eq!(
        serde_json::to_vec(&reopened).unwrap(),
        ordinary_saved,
        "rejection cannot rewrite the edited source document"
    );
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 0);

    let selected_profile = ProfileIdentity::from_payload(
        "fixture.explicit-update",
        "2",
        &json!({"allows":"game.motion"}),
    )
    .unwrap();
    let update = fixture
        .composition
        .capture_lock(
            &reopened,
            selected_profile.clone(),
            Some(SdkIdentity::current()),
        )
        .unwrap();
    assert_eq!(serde_json::to_vec(&reopened).unwrap(), ordinary_saved);
    let mut approved = reopened;
    update.apply_to(&mut approved);
    assert_eq!(
        approved.engine_lock.as_ref().unwrap().profile,
        selected_profile
    );
    send(
        &worker,
        WorkerCommand::UpdateDocument {
            document: Arc::new(approved.clone()),
            authoring: false,
            request: 71,
        },
    );
    let accepted = snapshot(&mut worker, |status| {
        status
            .results
            .iter()
            .any(|reply| reply.request == 71 && reply.result.is_ok())
    });
    assert_eq!(accepted.accepted_document.as_ref(), &approved);
    assert!(accepted.preparation_error.is_none());
    assert_eq!(accepted.state.as_ref(), initial.state.as_ref());
    assert_eq!(accepted.epoch, initial.epoch);
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 0);
    stop(&worker);
}

#[test]
fn compatible_locks_cannot_publish_a_new_execution_or_world_without_replacing_session() {
    let fixture = Fixture::new(false);
    let mut host = fixture.host();
    let accepted = host.accepted_document().clone();
    let state = host.session.state().clone();
    let project = host.session.project().clone();
    let revision = host.presentation_revision();
    let frame = host.render(160, 100).unwrap();
    let mut removed_schedule = accepted.clone();
    removed_schedule.game.as_mut().unwrap().execution = ExecutionGraph::default();
    approve_fixture(&fixture.composition, &mut removed_schedule);
    let mut changed_parameters = accepted.clone();
    changed_parameters.game.as_mut().unwrap().execution.stages[0].nodes[0].params["configuration"] =
        json!("changed");
    approve_fixture(&fixture.composition, &mut changed_parameters);
    let mut changed_world = accepted.clone();
    changed_world
        .root
        .nodes
        .iter_mut()
        .find(|node| node.id == "world")
        .unwrap()
        .module = "world.objects".into();
    approve_fixture(&fixture.composition, &mut changed_world);
    let mut changed_world_identity = accepted.clone();
    changed_world_identity
        .root
        .nodes
        .iter_mut()
        .find(|node| node.id == "world")
        .unwrap()
        .id = "another-world".into();
    for edge in &mut changed_world_identity.root.edges {
        if edge.from_node == "world" {
            edge.from_node = "another-world".into();
        }
    }
    for rejected in [
        &removed_schedule,
        &changed_parameters,
        &changed_world,
        &changed_world_identity,
    ] {
        fixture
            .composition
            .validate_lock(rejected, Some(&SdkIdentity::current()))
            .unwrap();
        assert!(
            host.sync_document(rejected)
                .unwrap_err()
                .contains("complete host")
        );
        assert!(
            host.sync_document_deferred(rejected)
                .unwrap_err()
                .contains("complete host")
        );
        assert!(
            host.poll_preparation(rejected)
                .unwrap_err()
                .contains("complete host")
        );
        assert!(!host.has_pending_preparation());
        assert_eq!(host.accepted_document(), &accepted);
        assert_eq!(host.session.project(), &project);
        assert_eq!(host.session.state(), &state);
        assert_eq!(host.presentation_revision(), revision);
        assert_eq!(host.render(160, 100).unwrap().rgba, frame.rgba);
    }
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 0);
    let renderer_only = fixture.candidate(4, [255, 30, 30]);
    host.sync_document(&renderer_only).unwrap();
    assert_eq!(host.accepted_document(), &renderer_only);
    assert_eq!(host.session.project(), &project);
    assert_eq!(host.session.state(), &state);
    assert_ne!(host.render(160, 100).unwrap().rgba, frame.rgba);
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 1);
    let source = Fixture::new(true);
    let mut source_host = source.host();
    let mut parameters = source.document.clone();
    parameters
        .root
        .nodes
        .iter_mut()
        .find(|node| node.id == "world")
        .unwrap()
        .params = json!({"seed":7});
    source
        .composition
        .validate_lock(&parameters, Some(&SdkIdentity::current()))
        .unwrap();
    assert!(
        source_host
            .sync_document(&parameters)
            .unwrap_err()
            .contains("complete host")
    );
    assert!(
        source_host
            .sync_document_deferred(&parameters)
            .unwrap_err()
            .contains("complete host")
    );
    assert_eq!(source_host.accepted_document(), &source.document);
    assert_eq!(source.control.world_factories.load(Ordering::SeqCst), 1);
}

#[test]
fn optional_profile_payload_and_active_sdk_identity_are_checked_before_device_creation() {
    let fixture = Fixture::new(false);
    let build = |document: GraphDocument| {
        GameHost::build(
            document,
            fixture.composition.clone(),
            fixture.options(RenderMode::Graph, HostServiceRegistry::new(), false),
            None,
        )
    };
    let mut malformed = fixture.document.clone();
    malformed.engine_profile = Some(json!({"invalid":"profile"}));
    assert!(
        build(malformed)
            .err()
            .unwrap()
            .contains("Saved Engine profile")
    );

    let mut selected = fixture.document.clone();
    let profile = module_engine_project::EngineProfile::new(
        "fixture.selected",
        "1",
        module_engine_project::EngineBlueprint {
            root: selected.root.clone(),
            engine: selected.engine.clone(),
            custom_sources: selected.custom_sources.clone(),
            shader_sources: selected.shader_sources.clone(),
            default_execution: selected.game.as_ref().unwrap().execution.clone(),
            available_modules: fixture.composition.module_versions().into_keys().collect(),
        },
    )
    .unwrap();
    selected.engine_profile = Some(profile.document_value().unwrap());
    fixture
        .composition
        .capture_lock(
            &selected,
            profile.identity.clone(),
            Some(SdkIdentity::current()),
        )
        .unwrap()
        .apply_to(&mut selected);
    let mut tampered = selected.clone();
    tampered.engine_profile.as_mut().unwrap()["blueprint"]["custom_sources"] =
        json!({"added":"source"});
    assert!(build(tampered).err().unwrap().contains("fingerprint"));
    let mut wrong_sdk = fixture.document.clone();
    wrong_sdk
        .engine
        .enabled_modules
        .insert("custom.rust".into());
    approve_fixture(&fixture.composition, &mut wrong_sdk);
    wrong_sdk
        .engine_lock
        .as_mut()
        .unwrap()
        .sdk
        .as_mut()
        .unwrap()
        .data_api_fingerprint = "module-engine-data-api/v1".into();
    assert!(
        build(wrong_sdk)
            .err()
            .unwrap()
            .contains("data/API identity")
    );
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 0);
    let mut host = build(selected).unwrap();
    let accepted = host.accepted_document().clone();
    let mut invalid = accepted.clone();
    invalid.engine_profile.as_mut().unwrap()["identity"]["version"] = json!("different");
    assert!(host.sync_document(&invalid).is_err());
    assert_eq!(host.accepted_document(), &accepted);
}

#[test]
fn headless_selection_requires_world_activity_and_explicit_pin_without_creating_backends() {
    let fixture = Fixture::new(false);
    let mut document = fixture.document.clone();
    document.root.nodes.retain(|node| node.id == "world");
    document.root.edges.clear();
    approve_fixture(&fixture.composition, &mut document);
    let error = GameHost::build(
        document.clone(),
        fixture.composition.clone(),
        fixture.options(
            RenderMode::Headless {
                world_node: "world".into(),
            },
            HostServiceRegistry::new(),
            false,
        ),
        None,
    )
    .err()
    .unwrap();
    assert!(error.contains("engine.enabled_modules"), "{error}");
    document.engine.enabled_modules.insert("world.ecs".into());
    approve_fixture(&fixture.composition, &mut document);
    document.module_versions.remove("world.ecs");
    assert!(
        GameHost::build(
            document,
            fixture.composition.clone(),
            fixture.options(
                RenderMode::Headless {
                    world_node: "world".into()
                },
                HostServiceRegistry::new(),
                false
            ),
            None,
        )
        .err()
        .unwrap()
        .contains("world.ecs")
    );
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.control.world_factories.load(Ordering::SeqCst), 0);
}

#[test]
fn deferred_cpu_factory_keeps_live_frames_discards_obsolete_results_and_rejects_candidates() {
    let fixture = Fixture::new(false);
    let mut host = fixture.host();
    let initial = host.render(160, 100).unwrap();
    fixture
        .control
        .delayed_generation
        .store(1, Ordering::SeqCst);
    let delayed = fixture.candidate(1, [40, 255, 40]);
    host.sync_document_deferred(&delayed).unwrap();
    wait_until(|| fixture.control.started.lock().unwrap().contains(&1));
    assert!(host.has_pending_preparation());
    assert_eq!(host.accepted_document(), &fixture.document);
    assert_eq!(host.render(160, 100).unwrap().rgba, initial.rgba);
    let newer = fixture.candidate(2, [40, 40, 255]);
    fixture.control.release();
    complete(&mut host, &newer).unwrap();
    assert_eq!(
        host.accepted_document(),
        &fixture.document,
        "Obsolete completion must not select its renderer"
    );
    let polling_thread = thread::current().id();
    wait_until(|| {
        fixture
            .control
            .dropped
            .lock()
            .unwrap()
            .iter()
            .any(|(generation, owner)| *generation == 1 && *owner != polling_thread)
    });
    host.sync_document_deferred(&newer).unwrap();
    complete(&mut host, &newer).unwrap();
    let accepted = host.render(160, 100).unwrap();
    assert_ne!(accepted.rgba, initial.rgba);
    assert_eq!(host.accepted_document(), &newer);
    fixture.control.reject_generation.store(3, Ordering::SeqCst);
    let rejected = fixture.candidate(3, [255, 255, 40]);
    host.sync_document_deferred(&rejected).unwrap();
    assert!(
        complete(&mut host, &rejected)
            .unwrap_err()
            .contains("Injected CPU candidate rejection")
    );
    assert_eq!(host.accepted_document(), &newer);
    assert_eq!(host.render(160, 100).unwrap().rgba, accepted.rgba);
    let factories = fixture.control.factories.load(Ordering::SeqCst);
    assert!(host.sync_document_deferred(&rejected).is_err());
    assert_eq!(
        fixture.control.factories.load(Ordering::SeqCst),
        factories,
        "An unchanged failed candidate is retained without repeated preparation"
    );
}

#[test]
fn failed_renderer_candidate_retries_when_dependency_error_is_repaired() {
    let fixture = Fixture::new(false);
    let mut host = fixture.host();
    let candidate = fixture.candidate(1, [40, 255, 40]);
    *fixture.control.dependency.lock().unwrap() = Err("Missing CPU source dependency".into());
    fixture.control.reject_generation.store(1, Ordering::SeqCst);
    assert!(host.sync_document(&candidate).is_err());
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 1);
    fixture
        .control
        .reject_generation
        .store(u64::MAX, Ordering::SeqCst);
    assert!(host.sync_document(&candidate).is_err());
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 1);
    *fixture.control.dependency.lock().unwrap() = Ok(7);
    thread::sleep(Duration::from_millis(260));
    host.sync_document(&candidate).unwrap();
    assert_eq!(host.accepted_document(), &candidate);
    assert_eq!(fixture.control.candidates.load(Ordering::SeqCst), 2);
}

#[test]
fn independently_registered_world_projects_exact_authored_data_and_commands() {
    let fixture = Fixture::new(true);
    let mut host = fixture.host();
    assert_eq!(host.session.world_kind(), WorldKind::Custom);
    assert_eq!(fixture.control.world_factories.load(Ordering::SeqCst), 1);
    assert_eq!(
        host.session
            .world_objects()
            .iter()
            .map(|object| object.id)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(
        !host
            .session
            .snapshot()
            .items
            .iter()
            .any(|item| item.id == 9999)
    );
    host.session
        .apply_commands(&[
            WorldCommand::SetPosition {
                id: 1,
                position: [2., 0., 0.],
            },
            WorldCommand::SetColor {
                id: 2,
                color: [220, 30, 20, 255],
            },
        ])
        .unwrap();
    let projected = host.session.snapshot();
    assert_eq!(
        projected
            .items
            .iter()
            .find(|item| item.id == 2)
            .unwrap()
            .model[0][3],
        3.5
    );
    assert_eq!(
        projected
            .items
            .iter()
            .find(|item| item.id == 2)
            .unwrap()
            .color,
        [220, 30, 20, 255]
    );
    host.step(
        1. / 60.,
        &GameInput {
            actions: BTreeSet::from(["pulse".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        host.session
            .state()
            .world
            .object(1)
            .unwrap()
            .transform
            .translation[0],
        2.25
    );
    assert_eq!(
        host.session
            .snapshot()
            .items
            .iter()
            .find(|item| item.id == 2)
            .unwrap()
            .model[0][3],
        3.75
    );
    assert!(host.render(160, 100).unwrap().stats.shaded_pixels > 0);
    host.restart().unwrap();
    assert_eq!(fixture.control.world_factories.load(Ordering::SeqCst), 2);
    assert_eq!(
        host.session
            .state()
            .world
            .object(1)
            .unwrap()
            .transform
            .translation,
        [0.; 3]
    );
}

fn request_service(document: &mut GraphDocument) {
    document.game.as_mut().unwrap().execution.stages[0].nodes[0].params["service"] = json!(true);
}
fn rejecting_service_registry(control: &Arc<Control>) -> HostServiceRegistry {
    let mut services = HostServiceRegistry::new();
    let control = control.clone();
    services
        .register("fixture.service", move |_, _| {
            control.service_factories.fetch_add(1, Ordering::SeqCst);
            Err("Optional service factory must not run".into())
        })
        .unwrap();
    services
}
#[test]
fn headless_and_omitted_services_skip_all_renderer_and_optional_service_factories() {
    let fixture = Fixture::new(false);
    let mut document = fixture.document.clone();
    document.root.nodes.retain(|node| node.id == "world");
    document.root.edges.clear();
    document.engine.enabled_modules.insert("world.ecs".into());
    request_service(&mut document);
    document
        .game
        .as_mut()
        .unwrap()
        .scenes
        .get_mut("empty")
        .unwrap()
        .objects[0]
        .components
        .insert("mesh".into(), json!({"path":"assets/does-not-exist.glb"}));
    approve_fixture(&fixture.composition, &mut document);
    let mut host = GameHost::build(
        document,
        fixture.composition.clone(),
        fixture.options(
            RenderMode::Headless {
                world_node: "world".into(),
            },
            rejecting_service_registry(&fixture.control),
            true,
        ),
        None,
    )
    .unwrap();
    assert!(!host.has_renderer() && host.services.is_empty());
    assert!(host.session.state().world.component(1, "mesh").is_none());
    assert_eq!(host.renderer_performance(), RendererPerformance::default());
    assert!(
        host.render(160, 100)
            .unwrap_err()
            .contains("Headless host has no renderer")
    );
    host.step(1. / 60., &GameInput::default()).unwrap();
    assert_eq!(host.session.state().tick, 1);
    assert_eq!(fixture.control.factories.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.control.service_factories.load(Ordering::SeqCst), 0);
    let mut invalid = host.accepted_document().clone();
    invalid.game.as_mut().unwrap().execution.stages[0].nodes[0].module =
        "unknown.active-source".into();
    assert!(
        GameHost::build(
            invalid,
            fixture.composition.clone(),
            fixture.options(
                RenderMode::Headless {
                    world_node: "world".into()
                },
                HostServiceRegistry::new(),
                false
            ),
            None
        )
        .err()
        .unwrap()
        .contains("unknown.active-source")
    );
    let mut omitted = fixture.document.clone();
    omitted.game.as_mut().unwrap().execution.stages.clear();
    let omitted = GameHost::build(
        omitted,
        fixture.composition.clone(),
        fixture.options(
            RenderMode::Graph,
            rejecting_service_registry(&fixture.control),
            true,
        ),
        None,
    )
    .unwrap();
    assert!(omitted.services.is_empty());
    assert_eq!(fixture.control.service_factories.load(Ordering::SeqCst), 0);
    let mut worker = GameWorker::spawn(host, None);
    send(
        &worker,
        WorkerCommand::SetViewport {
            width: 160,
            height: 100,
        },
    );
    send(&worker, WorkerCommand::SetPlaying(true));
    let status = snapshot(&mut worker, |status| status.state.tick > 1);
    assert!(
        status.frame.is_none()
            && status.frame_tick.is_none()
            && status.frame_revision.is_none()
            && status.frame_completed_at.is_none()
            && status.render_error.is_none()
    );
    stop(&worker);
}

#[test]
fn worker_retains_last_good_cpu_frame_metadata_on_draw_and_candidate_failure() {
    let fixture = Fixture::new(false);
    let host = fixture.host();
    let retired = host.retirement_signal();
    let wakes = Arc::new(AtomicUsize::new(0));
    let callback = wakes.clone();
    let mut worker = GameWorker::spawn(
        host,
        Some(Arc::new(move || {
            callback.fetch_add(1, Ordering::SeqCst);
        })),
    );
    send(
        &worker,
        WorkerCommand::SetViewport {
            width: 160,
            height: 100,
        },
    );
    let initial = snapshot(&mut worker, |status| status.frame.is_some());
    assert!(initial.frame.as_ref().unwrap().stats.shaded_pixels > 0);
    fixture.control.fail_render.store(true, Ordering::SeqCst);
    send(&worker, WorkerCommand::SetPlaying(true));
    let failed = snapshot(&mut worker, |status| {
        status.state.tick > 0 && status.render_error.is_some()
    });
    assert_eq!(
        failed.frame.as_ref().unwrap().rgba,
        initial.frame.as_ref().unwrap().rgba
    );
    assert_eq!(failed.frame_tick, initial.frame_tick);
    assert_eq!(failed.frame_revision, initial.frame_revision);
    assert_eq!(failed.frame_completed_at, initial.frame_completed_at);
    assert_ne!(failed.presentation_revision, initial.presentation_revision);
    assert!(failed.fault.is_none());
    send(&worker, WorkerCommand::SetPlaying(false));
    snapshot(&mut worker, |status| !status.playing);
    fixture.control.fail_render.store(false, Ordering::SeqCst);
    send(&worker, WorkerCommand::InvalidateContent);
    let recovered = snapshot(&mut worker, |status| {
        status.render_error.is_none() && status.frame_revision == Some(status.presentation_revision)
    });
    fixture.control.reject_generation.store(4, Ordering::SeqCst);
    let rejected = fixture.candidate(4, [40, 255, 40]);
    send(
        &worker,
        WorkerCommand::UpdateDocument {
            document: Arc::new(rejected),
            authoring: false,
            request: 1,
        },
    );
    let rejected = snapshot(&mut worker, |status| {
        status
            .results
            .iter()
            .any(|reply| reply.request == 1 && reply.result.is_err())
    });
    assert_eq!(rejected.accepted_document.as_ref(), &fixture.document);
    assert_eq!(
        rejected.frame.as_ref().unwrap().rgba,
        recovered.frame.as_ref().unwrap().rgba
    );
    assert_eq!(rejected.frame_revision, recovered.frame_revision);
    assert!(
        rejected
            .preparation_error
            .as_ref()
            .unwrap()
            .contains("Injected CPU candidate rejection")
    );
    let accepted = fixture.candidate(5, [40, 40, 255]);
    send(
        &worker,
        WorkerCommand::UpdateDocument {
            document: Arc::new(accepted.clone()),
            authoring: false,
            request: 2,
        },
    );
    let updated = snapshot(&mut worker, |status| {
        status.accepted_document.as_ref() == &accepted
            && status.frame_revision == Some(status.presentation_revision)
    });
    assert!(updated.preparation_error.is_none() && updated.render_error.is_none());
    assert_ne!(
        updated.frame.as_ref().unwrap().rgba,
        initial.frame.as_ref().unwrap().rgba
    );
    assert!(wakes.load(Ordering::SeqCst) > 0);
    stop(&worker);
    assert!(retired.load(Ordering::Acquire));
}

#[test]
fn cancelled_renderer_preparation_retires_result_off_caller_and_preserves_last_good() {
    let fixture = Fixture::new(false);
    let mut host = fixture.host();
    let before = host.render(80, 60).unwrap();
    let state = host.session.state().clone();
    let document = host.accepted_document().clone();
    let candidate = fixture.candidate(17, [0, 100, 200]);
    fixture
        .control
        .delayed_generation
        .store(17, Ordering::SeqCst);
    host.sync_document_deferred(&candidate).unwrap();
    wait_until(|| fixture.control.started.lock().unwrap().contains(&17));
    // The factory is deliberately blocked. Cancellation must return before
    // release, so a UI-thread join here would deadlock this regression.
    host.cancel_preparation();
    assert!(!host.has_pending_preparation());
    assert_eq!(host.accepted_document(), &document);
    assert_eq!(host.session.state(), &state);
    fixture.control.release();
    wait_until(|| {
        fixture
            .control
            .dropped
            .lock()
            .unwrap()
            .iter()
            .any(|(generation, _)| *generation == 17)
    });
    assert!(
        fixture
            .control
            .dropped
            .lock()
            .unwrap()
            .iter()
            .filter(|(generation, _)| *generation == 17)
            .all(|(_, owner)| *owner != thread::current().id())
    );
    assert_eq!(host.render(80, 60).unwrap().rgba, before.rgba);
    assert_eq!(host.accepted_document(), &document);
}

struct FailingService;
impl HostService for FailingService {
    fn consume(&mut self, _: &[GameEvent]) -> Result<(), String> {
        Err("Injected service consume failure".into())
    }
    fn reset(&mut self) -> Result<(), String> {
        Err("Injected service reset failure".into())
    }
    fn set_enabled(&mut self, _: bool) {}
    fn stop(&mut self) {}
    fn diagnostics(&self) -> Value {
        json!({})
    }
}
#[test]
fn optional_service_failures_report_activity_without_rejecting_committed_tick_or_restart() {
    let fixture = Fixture::new(false);
    let mut document = fixture.document.clone();
    request_service(&mut document);
    let mut registry = HostServiceRegistry::new();
    registry
        .register("fixture.service", |_, _| Ok(Box::new(FailingService)))
        .unwrap();
    let mut host = GameHost::build(
        document,
        fixture.composition.clone(),
        fixture.options(RenderMode::Graph, registry, true),
        None,
    )
    .unwrap();
    assert_eq!(host.step(1. / 60., &GameInput::default()).unwrap(), 1);
    assert_eq!(host.session.state().tick, 1);
    assert!(host.activities().iter().any(|activity| {
        activity.state == ActivityState::Failed
            && activity
                .error
                .as_ref()
                .is_some_and(|error| error.contains("service consume failure"))
    }));
    host.restart().unwrap();
    assert_eq!(host.session.state().tick, 0);
    assert!(host.session.state().module_data.is_empty());
    assert!(host.activities().iter().any(|activity| {
        activity.state == ActivityState::Failed
            && activity
                .error
                .as_ref()
                .is_some_and(|error| error.contains("service reset failure"))
    }));
    let mut worker = GameWorker::spawn(host, None);
    send(&worker, WorkerCommand::SetPlaying(true));
    let sequence = press(&worker);
    let committed = snapshot(&mut worker, |status| status.input_ack >= sequence);
    assert!(committed.state.tick > 0 && committed.fault.is_none());
    assert_eq!(committed.state.module_data["fixture.pulses"], json!(1));
    assert!(
        committed
            .activity
            .iter()
            .any(|activity| activity.state == ActivityState::Failed)
    );
    send(&worker, WorkerCommand::SetPlaying(false));
    let paused = snapshot(&mut worker, |status| !status.playing);
    let queued = press(&worker);
    send(&worker, WorkerCommand::Restart { request: 7 });
    let restarted = snapshot(&mut worker, |status| {
        status
            .results
            .iter()
            .any(|reply| reply.request == 7 && reply.result.is_ok())
    });
    assert_eq!(restarted.epoch, paused.epoch + 1);
    assert_eq!(restarted.state.tick, 0);
    assert!(restarted.state.module_data.is_empty() && restarted.fault.is_none());
    assert!(restarted.cancelled_input_ack >= queued);
    assert!(restarted.activity.iter().any(|activity| {
        activity.state == ActivityState::Failed
            && activity
                .error
                .as_ref()
                .is_some_and(|error| error.contains("service reset failure"))
    }));
    stop(&worker);
}
