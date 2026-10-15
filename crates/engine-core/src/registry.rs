use crate::{
    DefaultRendererAdapter, EcsWorld, FrameRenderer, GraphDocument, Node, ObjectsWorld,
    RendererAdapter, SoftwareRenderer, WorldCommand, WorldKind, WorldObject, WorldProvider,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A named pin contract; type names are compared exactly, without implicit casts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortDescriptor {
    pub name: String,
    pub data_type: String,
    pub required: bool,
}

/// The public, serializable contract of a registered source module.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModuleDescriptor {
    pub id: String,
    pub label: String,
    pub description: String,
    pub inputs: Vec<PortDescriptor>,
    pub outputs: Vec<PortDescriptor>,
    #[serde(default)]
    pub properties: Vec<crate::PropertySchema>,
}

type WorldFactory = Arc<dyn Fn(&Value) -> Result<Box<dyn WorldProvider>, String> + Send + Sync>;
type RendererFactory =
    Arc<dyn Fn(&Node, &GraphDocument) -> Result<Box<dyn FrameRenderer>, String> + Send + Sync>;

/// Authored gameplay keeps its own ECS; compatible providers are projections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameWorldPolicy {
    CanonicalEcs,
    Projection,
}

#[derive(Clone, Copy, Debug)]
pub struct WorldMetadata {
    pub kind: WorldKind,
    pub game_policy: GameWorldPolicy,
    pub validate_params: fn(&Value) -> Result<(), String>,
}
impl Default for WorldMetadata {
    fn default() -> Self {
        Self {
            kind: WorldKind::Custom,
            game_policy: GameWorldPolicy::Projection,
            validate_params: |_| Ok(()),
        }
    }
}

/// Installed source modules and their executable world factories.
///
/// A third world is registered in the application composition root through
/// [`Self::register_world`]; no change to graph validation or rendering is needed.
/// Other executable extension points are intentionally not implemented yet.
#[derive(Clone)]
pub struct ModuleRegistry {
    descriptors: Vec<ModuleDescriptor>,
    world_factories: BTreeMap<String, WorldFactory>,
    world_metadata: BTreeMap<String, WorldMetadata>,
    renderer_factories: BTreeMap<String, RendererFactory>,
    renderer_adapters: BTreeMap<String, Arc<dyn RendererAdapter>>,
    versions: BTreeMap<String, String>,
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

impl ModuleRegistry {
    /// Create the installed source registry for this prototype.
    pub fn builtin() -> Self {
        let mut registry = Self {
            descriptors: vec![],
            world_factories: BTreeMap::new(),
            world_metadata: BTreeMap::new(),
            renderer_factories: BTreeMap::new(),
            renderer_adapters: BTreeMap::new(),
            versions: BTreeMap::new(),
        };
        for descriptor in [
            module(
                "scene.extract",
                "Scene extraction",
                "Extract neutral meshes and transforms from the selected World provider.",
                vec![pin("world", "World", true)],
                vec![pin("scene", "Scene3D", false)],
            ),
            module(
                "render.software3d",
                "Software 3D",
                "Compound perspective renderer: inspect its actual rasterizer graph.",
                vec![pin("scene", "Scene3D", true)],
                vec![pin("frame", "Frame3D", false)],
            ),
            module(
                "render.rasterize",
                "Depth rasterizer",
                "CPU triangle rasterization with perspective projection and a depth buffer.",
                vec![pin("scene", "Scene3D", true)],
                vec![pin("frame", "Frame3D", false)],
            ),
            module(
                "viewport.output",
                "Viewport",
                "Present the connected rendered frame.",
                vec![pin("frame", "Frame3D", true)],
                vec![],
            ),
            module(
                "scalar.value",
                "Scalar",
                "A numeric source. Parameter: value.",
                vec![],
                vec![pin("value", "Scalar", false)],
            ),
            module(
                "clock.time",
                "Time",
                "Host simulation time in seconds, evaluated by the scalar host.",
                vec![],
                vec![pin("value", "Scalar", false)],
            ),
            module(
                "custom.rust",
                "Custom Rust",
                "Host-evaluated supported Rust expressions; unsupported source remains explicit.",
                vec![pin("time", "Scalar", false), pin("base", "Scalar", false)],
                vec![pin("value", "Scalar", false)],
            ),
            module(
                "interface.input",
                "Interface input",
                "Nested input. Parameters: parent port, data_type; output pin is value.",
                vec![],
                vec![],
            ),
            module(
                "interface.output",
                "Interface output",
                "Nested output. Parameters: parent port, data_type; input pin is value.",
                vec![],
                vec![],
            ),
        ] {
            registry.descriptors.push(descriptor);
        }
        registry.renderer_factories.insert(
            "render.software3d".into(),
            Arc::new(|_, _| Ok(Box::new(SoftwareRenderer))),
        );
        registry.renderer_adapters.insert(
            "render.software3d".into(),
            Arc::new(crate::backend::SoftwareRendererAdapter),
        );
        registry.descriptors.push(module(
            "custom.rust.update",
            "Custom update",
            "Compiled Rust produces portable world and renderer commands for the current frame.",
            vec![],
            vec![
                pin("world", "WorldCommands", false),
                pin("render", "RenderCommands", false),
            ],
        ));
        registry
            .register_module(crate::motion_property_module())
            .expect("unique gameplay schema module");
        registry
            .register_world_with_metadata(
                world_descriptor(
                    "world.objects",
                    "Object tree",
                    "Owned recursive object hierarchy with per-object transforms and children.",
                ),
                WorldMetadata {
                    kind: WorldKind::Objects,
                    game_policy: GameWorldPolicy::Projection,
                    validate_params: require_empty_world_params,
                },
                |params| {
                    require_empty_world_params(params)?;
                    Ok(Box::new(ObjectsWorld::demo()))
                },
            )
            .expect("unique built-in Object world");
        registry
            .register_world_with_metadata(
                world_descriptor(
                    "world.ecs",
                    "ECS tables",
                    "Entity IDs and separate Transform, Parent, Color and Motion component tables.",
                ),
                WorldMetadata {
                    kind: WorldKind::Ecs,
                    game_policy: GameWorldPolicy::CanonicalEcs,
                    validate_params: require_empty_world_params,
                },
                |params| {
                    require_empty_world_params(params)?;
                    Ok(Box::new(EcsWorld::demo()))
                },
            )
            .expect("unique built-in ECS world");
        for descriptor in &registry.descriptors {
            registry
                .versions
                .entry(descriptor.id.clone())
                .or_insert_with(|| env!("CARGO_PKG_VERSION").into());
        }
        registry
    }

    /// Contracts in stable insertion order for palette and documentation.
    pub fn descriptors(&self) -> &[ModuleDescriptor] {
        &self.descriptors
    }

    /// Look up the contract of an installed module.
    pub fn descriptor(&self, id: &str) -> Option<&ModuleDescriptor> {
        self.descriptors.iter().find(|d| d.id == id)
    }

    pub fn module_version(&self, id: &str) -> Option<&str> {
        self.versions.get(id).map(String::as_str)
    }
    pub fn set_module_version(&mut self, id: &str, version: &str) -> Result<(), String> {
        if self.descriptor(id).is_none() || version.trim().is_empty() {
            return Err("Version needs an installed module and a nonempty value".into());
        }
        self.versions.insert(id.into(), version.into());
        Ok(())
    }

    /// Register a compiled-in provider from application or plugin source.
    ///
    /// The descriptor must expose exactly one output named world of type World;
    /// inputs may only contain the optional Scalar speed pin used by the host.
    /// The factory receives the node's JSON parameters. Duplicate IDs are errors.
    /// Dynamic binary loading and arbitrary renderer replacement are future work.
    pub fn register_world<F>(
        &mut self,
        descriptor: ModuleDescriptor,
        factory: F,
    ) -> Result<(), String>
    where
        F: Fn(&Value) -> Result<Box<dyn WorldProvider>, String> + Send + Sync + 'static,
    {
        self.register_world_with_metadata(descriptor, WorldMetadata::default(), factory)
    }

    pub fn register_world_with_metadata<F>(
        &mut self,
        descriptor: ModuleDescriptor,
        metadata: WorldMetadata,
        factory: F,
    ) -> Result<(), String>
    where
        F: Fn(&Value) -> Result<Box<dyn WorldProvider>, String> + Send + Sync + 'static,
    {
        if metadata.game_policy == GameWorldPolicy::CanonicalEcs && metadata.kind != WorldKind::Ecs
        {
            return Err("Canonical ECS policy requires ECS world kind".into());
        }
        for schema in &descriptor.properties {
            crate::validate_property_schema(schema)?;
        }
        if descriptor.id.trim().is_empty() || self.descriptor(&descriptor.id).is_some() {
            return Err(format!(
                "Module '{}' is empty or already registered",
                descriptor.id
            ));
        }
        if descriptor.outputs.len() != 1
            || descriptor.outputs[0].name != "world"
            || descriptor.outputs[0].data_type != "World"
        {
            return Err("World provider must expose one world:World output".into());
        }
        if descriptor.inputs.iter().any(|p| {
            !((p.name == "speed" && p.data_type == "Scalar")
                || (p.name == "commands" && p.data_type == "WorldCommands"))
                || p.required
        }) || descriptor.inputs.len() > 2
        {
            return Err(
                "World provider supports optional speed:Scalar and commands:WorldCommands inputs"
                    .into(),
            );
        }
        self.world_factories
            .insert(descriptor.id.clone(), Arc::new(factory));
        self.world_metadata.insert(descriptor.id.clone(), metadata);
        self.versions
            .insert(descriptor.id.clone(), env!("CARGO_PKG_VERSION").into());
        self.descriptors.push(descriptor);
        Ok(())
    }

    /// Whether a contract has an installed executable world implementation.
    pub fn is_world_provider(&self, id: &str) -> bool {
        self.world_factories.contains_key(id)
    }
    pub fn world_metadata(&self, id: &str) -> Option<&WorldMetadata> {
        self.world_metadata.get(id)
    }

    /// Register an executable source renderer and its visible external contract.
    pub fn register_renderer<F>(
        &mut self,
        descriptor: ModuleDescriptor,
        factory: F,
    ) -> Result<(), String>
    where
        F: Fn(&Node, &GraphDocument) -> Result<Box<dyn FrameRenderer>, String>
            + Send
            + Sync
            + 'static,
    {
        self.register_renderer_with_adapter(descriptor, DefaultRendererAdapter, factory)
    }

    pub fn register_renderer_with_adapter<A, F>(
        &mut self,
        descriptor: ModuleDescriptor,
        adapter: A,
        factory: F,
    ) -> Result<(), String>
    where
        A: RendererAdapter + 'static,
        F: Fn(&Node, &GraphDocument) -> Result<Box<dyn FrameRenderer>, String>
            + Send
            + Sync
            + 'static,
    {
        if descriptor.outputs.len() != 1
            || descriptor.outputs[0].name != "frame"
            || descriptor.outputs[0].data_type != "Frame3D"
            || !descriptor
                .inputs
                .iter()
                .any(|p| p.name == "scene" && p.data_type == "Scene3D" && p.required)
        {
            return Err("Renderer must expose scene:Scene3D and frame:Frame3D".into());
        }
        let id = descriptor.id.clone();
        self.register_module(descriptor)?;
        self.renderer_factories
            .insert(id.clone(), Arc::new(factory));
        self.renderer_adapters.insert(id, Arc::new(adapter));
        Ok(())
    }

    /// Register a pass contract. Its parent renderer owns execution.
    pub fn register_module(&mut self, descriptor: ModuleDescriptor) -> Result<(), String> {
        for schema in &descriptor.properties {
            crate::validate_property_schema(schema)?;
        }
        if descriptor.id.trim().is_empty() || self.descriptor(&descriptor.id).is_some() {
            return Err(format!(
                "Module '{}' is empty or already registered",
                descriptor.id
            ));
        }
        self.versions
            .insert(descriptor.id.clone(), env!("CARGO_PKG_VERSION").into());
        self.descriptors.push(descriptor);
        Ok(())
    }

    pub fn is_renderer(&self, id: &str) -> bool {
        self.renderer_factories.contains_key(id)
    }

    pub fn renderer_adapter(&self, id: &str) -> Option<&dyn RendererAdapter> {
        self.renderer_adapters
            .get(id)
            .map(|adapter| adapter.as_ref())
    }

    pub fn create_renderer(
        &self,
        node: &Node,
        document: &GraphDocument,
    ) -> Result<Box<dyn FrameRenderer>, String> {
        self.renderer_adapter(&node.module)
            .ok_or_else(|| format!("No renderer registered for '{}'", node.module))?
            .validate_controls(node, document, &crate::RenderOverrides::default())?;
        self.renderer_factories
            .get(&node.module)
            .ok_or_else(|| format!("No renderer registered for '{}'", node.module))?(
            node, document
        )
    }

    pub fn create_world(&self, id: &str, params: &Value) -> Result<Box<dyn WorldProvider>, String> {
        (self
            .world_metadata(id)
            .ok_or_else(|| format!("No executable provider registered for '{id}'"))?
            .validate_params)(params)?;
        self.world_factories
            .get(id)
            .ok_or_else(|| format!("No executable provider registered for '{id}'"))?(params)
    }

    /// Build a fresh authored compatibility projection, replacing factory demo
    /// objects atomically and checking the exact scene before acceptance.
    pub fn create_game_world(
        &self,
        node: &Node,
        objects: Vec<WorldObject>,
    ) -> Result<Box<dyn WorldProvider>, String> {
        if self
            .world_metadata(&node.module)
            .ok_or("No registered world policy")?
            .game_policy
            == GameWorldPolicy::CanonicalEcs
        {
            return Err("Canonical ECS gameplay does not create a secondary provider".into());
        }
        if node.subgraph.is_some() || objects.iter().any(|object| object.id == 0) {
            return Err("Authored world needs a flat module and nonzero object IDs".into());
        }
        crate::validate_world_commands(&objects, &[])?;
        let mut provider = self.create_world(&node.module, &node.params)?;
        let previous = provider.objects()?;
        crate::validate_world_commands(&previous, &[])?;
        let mut commands: Vec<_> = previous
            .iter()
            .filter(|object| object.parent.is_none())
            .map(|object| WorldCommand::Remove { id: object.id })
            .collect();
        commands.extend(
            objects
                .iter()
                .cloned()
                .map(|object| WorldCommand::Spawn { object }),
        );
        provider.validate_commands(&commands)?;
        provider.apply_commands(&commands)?;
        let loaded = provider.objects()?;
        crate::validate_world_commands(&loaded, &[])?;
        let as_map = |objects: Vec<WorldObject>| {
            objects
                .into_iter()
                .map(|object| (object.id, object))
                .collect::<BTreeMap<_, _>>()
        };
        if loaded.len() != objects.len() || as_map(loaded) != as_map(objects) {
            return Err("World factory did not load the exact authored objects".into());
        }
        Ok(provider)
    }
}

/// Standard contract for a source world module; labels/descriptions remain yours.
pub fn world_descriptor(id: &str, label: &str, description: &str) -> ModuleDescriptor {
    module(
        id,
        label,
        description,
        vec![
            pin("speed", "Scalar", false),
            pin("commands", "WorldCommands", false),
        ],
        vec![pin("world", "World", false)],
    )
}

fn require_empty_world_params(params: &Value) -> Result<(), String> {
    if params.is_null() || params.as_object().is_some_and(|o| o.is_empty()) {
        Ok(())
    } else {
        Err("Built-in worlds do not support parameters yet; expected {}".into())
    }
}

fn pin(name: &str, data_type: &str, required: bool) -> PortDescriptor {
    PortDescriptor {
        name: name.into(),
        data_type: data_type.into(),
        required,
    }
}

fn module(
    id: &str,
    label: &str,
    description: &str,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
) -> ModuleDescriptor {
    ModuleDescriptor {
        id: id.into(),
        label: label.into(),
        description: description.into(),
        inputs,
        outputs,
        properties: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Camera, DefaultRendererAdapter, GameWorldPolicy, Graph, RenderControlTargets,
        RenderOverrides, RendererPerformance, Runtime, SceneSnapshot, Transform, demo_document,
        make_node, renderer_node_configuration,
    };
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn render_descriptor(id: &str) -> ModuleDescriptor {
        module(
            id,
            "Source CPU renderer",
            "Independent source renderer",
            vec![pin("scene", "Scene3D", true)],
            vec![pin("frame", "Frame3D", false)],
        )
    }
    struct CpuRenderer {
        tint: u8,
        renders: u64,
    }
    impl FrameRenderer for CpuRenderer {
        fn render(
            &mut self,
            scene: &SceneSnapshot,
            camera: Camera,
            width: usize,
            height: usize,
            _: &RenderOverrides,
        ) -> Result<crate::Frame, String> {
            self.renders += 1;
            let mut frame = crate::render_scene(scene, width, height, camera);
            for pixel in frame.rgba.as_chunks_mut::<4>().0 {
                pixel[0] = pixel[0].saturating_add(self.tint);
            }
            Ok(frame)
        }
        fn backend_name(&self) -> &str {
            "Source CPU"
        }
        fn performance(&self) -> RendererPerformance {
            RendererPerformance {
                render_calls: self.renders,
                ..Default::default()
            }
        }
    }
    struct Metadata {
        revision: Arc<AtomicU64>,
    }
    impl RendererAdapter for Metadata {
        fn dependency_revision(&self, _: &Node, _: &GraphDocument) -> Result<u64, String> {
            let revision = self.revision.load(Ordering::SeqCst);
            if revision == 0 {
                Err("Missing source dependency".into())
            } else {
                Ok(revision)
            }
        }
        fn control_targets(
            &self,
            _: &Node,
            _: &GraphDocument,
        ) -> Result<Option<RenderControlTargets>, String> {
            Ok(Some(RenderControlTargets {
                controls: BTreeSet::from(["tint".into()]),
                resources: BTreeSet::from(["frame".into()]),
            }))
        }
    }

    #[test]
    fn failed_factory_keeps_metadata_and_dependency_repair_without_creating_a_device() {
        let mut registry = ModuleRegistry::builtin();
        let revisions = Arc::new(AtomicU64::new(0));
        let factories = Arc::new(AtomicU64::new(0));
        let built = factories.clone();
        registry
            .register_renderer_with_adapter(
                render_descriptor("render.source"),
                Metadata {
                    revision: revisions.clone(),
                },
                move |_, _| {
                    built.fetch_add(1, Ordering::SeqCst);
                    Err("Injected device preparation failure".into())
                },
            )
            .unwrap();
        let document = demo_document(WorldKind::Ecs);
        let node = make_node("source", "render.source", "Source", [1., 2.], json!({}));
        let adapter = registry.renderer_adapter(&node.module).unwrap();
        let key = adapter.configuration_key(&node, &document);
        assert!(adapter.dependency_revision(&node, &document).is_err());
        assert_eq!(factories.load(Ordering::SeqCst), 0);
        assert!(registry.create_renderer(&node, &document).is_err());
        assert_eq!(factories.load(Ordering::SeqCst), 1);
        revisions.store(7, Ordering::SeqCst);
        assert_eq!(adapter.dependency_revision(&node, &document).unwrap(), 7);
        assert_eq!(adapter.configuration_key(&node, &document), key);
        assert!(
            adapter
                .control_targets(&node, &document)
                .unwrap()
                .unwrap()
                .controls
                .contains("tint")
        );
        assert_eq!(factories.load(Ordering::SeqCst), 1);
        let mut controls = RenderOverrides::default();
        controls.enabled.insert("unsupported".into(), true);
        assert!(
            adapter
                .validate_controls(&node, &document, &controls)
                .is_err()
        );
        assert!(
            DefaultRendererAdapter
                .validate_controls(&node, &document, &controls)
                .is_err()
        );
    }

    #[test]
    fn executable_configuration_keys_ignore_layout_but_keep_nested_parameters_and_sources() {
        let mut document = demo_document(WorldKind::Ecs);
        let mut node = make_node(
            "renderer",
            "render.source",
            "Old label",
            [1., 2.],
            json!({"quality":2}),
        );
        node.subgraph = Some(Box::new(Graph {
            nodes: vec![make_node(
                "custom",
                "custom.rust",
                "Custom",
                [8., 9.],
                json!({"source_id":"effect","code_layout":{"x":4},"code_layout_keys":["x"]}),
            )],
            edges: vec![],
        }));
        let original = renderer_node_configuration(&node);
        let original_key = DefaultRendererAdapter.configuration_key(&node, &document);
        node.label = "Moved".into();
        node.position = [99., 100.];
        let child = &mut node.subgraph.as_mut().unwrap().nodes[0];
        child.label = "Renamed".into();
        child.position = [-4., 8.];
        child.params["code_layout"] = json!({"x":99});
        assert_eq!(renderer_node_configuration(&node), original);
        assert_eq!(
            DefaultRendererAdapter.configuration_key(&node, &document),
            original_key
        );
        document
            .shader_sources
            .insert("effect".into(), "changed executable source".into());
        assert_ne!(
            DefaultRendererAdapter.configuration_key(&node, &document),
            original_key
        );
        node.subgraph.as_mut().unwrap().nodes[0].params["source_id"] = json!("different");
        assert_ne!(renderer_node_configuration(&node), original);
    }

    #[test]
    fn explicit_routes_and_headless_world_selection_never_call_registered_factories() {
        let mut registry = ModuleRegistry::builtin();
        let calls = Arc::new(AtomicU64::new(0));
        let built = calls.clone();
        registry
            .register_renderer(render_descriptor("render.source"), move |_, _| {
                built.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(CpuRenderer {
                    tint: 0,
                    renders: 0,
                }))
            })
            .unwrap();
        let built = calls.clone();
        registry
            .register_world(
                world_descriptor("world.source", "Source", "Independent world"),
                move |_| {
                    built.fetch_add(1, Ordering::SeqCst);
                    Ok(Box::new(ObjectsWorld::demo()))
                },
            )
            .unwrap();
        let mut document = demo_document(WorldKind::Ecs);
        document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "world")
            .unwrap()
            .module = "world.source".into();
        let renderer = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "renderer")
            .unwrap();
        renderer.module = "render.source".into();
        renderer.subgraph = None;
        document.root.nodes.insert(
            0,
            make_node("unused", "render.software3d", "Unused", [0.; 2], json!({})),
        );
        let route = registry.render_route(&document).unwrap();
        assert_eq!(route.renderer.id, "renderer");
        assert_eq!(route.world.module, "world.source");
        document.root.nodes.rotate_left(2);
        assert_eq!(
            registry.render_route(&document).unwrap().renderer.module,
            "render.source"
        );
        let mut headless = document.clone();
        headless.root.nodes.retain(|node| node.id == "world");
        headless.root.edges.clear();
        assert_eq!(
            registry.world_node(&headless, "world").unwrap().module,
            "world.source"
        );
        assert!(registry.render_route(&headless).is_err());
        document
            .root
            .edges
            .retain(|edge| edge.to_node != "viewport");
        assert!(registry.render_route(&document).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let mut duplicated = demo_document(WorldKind::Ecs);
        duplicated.root.edges.push(duplicated.root.edges[2].clone());
        assert!(registry.render_route(&duplicated).is_err());
    }

    #[test]
    fn authored_projection_resets_factory_content_checks_exact_objects_and_keeps_canonical_policy()
    {
        let mut registry = ModuleRegistry::builtin();
        registry
            .register_world(
                world_descriptor("world.source", "Source", "Independent editable projection"),
                |_| Ok(Box::new(ObjectsWorld::demo())),
            )
            .unwrap();
        let node = make_node("world", "world.source", "Source", [0.; 2], json!({}));
        let objects = vec![
            WorldObject {
                id: 2,
                parent: Some(1),
                transform: Transform {
                    translation: [0., 2., 0.],
                    ..Default::default()
                },
                color: [2; 4],
            },
            WorldObject {
                id: 1,
                parent: None,
                transform: Transform::default(),
                color: [1; 4],
            },
        ];
        let mut loaded = registry
            .create_game_world(&node, objects.clone())
            .unwrap()
            .objects()
            .unwrap();
        loaded.sort_by_key(|object| object.id);
        assert_eq!(loaded, vec![objects[1].clone(), objects[0].clone()]);
        assert_eq!(
            registry.world_metadata("world.source").unwrap().kind,
            WorldKind::Custom
        );
        assert_eq!(
            registry
                .world_metadata("world.objects")
                .unwrap()
                .game_policy,
            GameWorldPolicy::Projection
        );
        let canonical = make_node("world", "world.ecs", "Canonical", [0.; 2], json!({}));
        assert_eq!(
            registry.world_metadata("world.ecs").unwrap().game_policy,
            GameWorldPolicy::CanonicalEcs
        );
        assert!(
            registry
                .create_game_world(&canonical, objects.clone())
                .is_err()
        );
        let mut invalid = objects.clone();
        invalid[0].parent = Some(999);
        assert!(registry.create_game_world(&node, invalid).is_err());
        struct IgnoresEdits;
        impl WorldProvider for IgnoresEdits {
            fn step(&mut self, _: f32, _: f32) {}
            fn snapshot(&self) -> SceneSnapshot {
                SceneSnapshot::default()
            }
            fn objects(&self) -> Result<Vec<WorldObject>, String> {
                Ok(vec![])
            }
            fn apply_commands(&mut self, _: &[WorldCommand]) -> Result<(), String> {
                Ok(())
            }
        }
        registry
            .register_world(
                world_descriptor("world.broken", "Broken", "Ignores object loading"),
                |_| Ok(Box::new(IgnoresEdits)),
            )
            .unwrap();
        let broken = make_node("world", "world.broken", "Broken", [0.; 2], json!({}));
        assert!(registry.create_game_world(&broken, objects).is_err());
    }

    #[test]
    fn third_renderer_runs_real_pixels_through_legacy_runtime_and_neutral_defaults() {
        let mut registry = ModuleRegistry::builtin();
        let factories = Arc::new(AtomicU64::new(0));
        let built = factories.clone();
        registry
            .register_renderer(render_descriptor("render.source"), move |node, _| {
                built.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(CpuRenderer {
                    tint: node.params["tint"].as_u64().unwrap_or_default() as u8,
                    renders: 0,
                }))
            })
            .unwrap();
        let mut document = demo_document(WorldKind::Ecs);
        let renderer = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "renderer")
            .unwrap();
        renderer.module = "render.source".into();
        renderer.params = json!({"tint":37});
        renderer.subgraph = None;
        let runtime = Runtime::from_document(&document, &registry).unwrap();
        assert_eq!(runtime.backend_name(), "Source CPU");
        let frame = runtime.render(64, 48, Camera::default()).unwrap();
        assert_eq!(frame.rgba.len(), 64 * 48 * 4);
        assert!(
            frame
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[0] >= 37)
        );
        assert_eq!(factories.load(Ordering::SeqCst), 1);
        let node = registry.render_route(&document).unwrap().renderer;
        let mut live = registry.create_renderer(node, &document).unwrap();
        live.set_detailed_stats(false);
        assert!(live.prepare_candidate(node, &document).unwrap().is_none());
        assert_eq!(live.performance().render_calls, 0);
        live.render(
            &runtime.snapshot(),
            Camera::default(),
            32,
            24,
            &RenderOverrides::default(),
        )
        .unwrap();
        assert_eq!(live.performance().render_calls, 1);
        assert_eq!(live.performance().pipeline_creations, 0);
    }
}
