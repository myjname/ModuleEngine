use crate::{
    EcsWorld, FrameRenderer, GraphDocument, Node, ObjectsWorld, SoftwareRenderer, WorldProvider,
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

/// Installed source modules and their executable world factories.
///
/// A third world is registered in the application composition root through
/// [`Self::register_world`]; no change to graph validation or rendering is needed.
/// Other executable extension points are intentionally not implemented yet.
#[derive(Clone)]
pub struct ModuleRegistry {
    descriptors: Vec<ModuleDescriptor>,
    world_factories: BTreeMap<String, WorldFactory>,
    renderer_factories: BTreeMap<String, RendererFactory>,
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
            renderer_factories: BTreeMap::new(),
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
            .register_world(
                world_descriptor(
                    "world.objects",
                    "Object tree",
                    "Owned recursive object hierarchy with per-object transforms and children.",
                ),
                |params| {
                    require_empty_world_params(params)?;
                    Ok(Box::new(ObjectsWorld::demo()))
                },
            )
            .expect("unique built-in Object world");
        registry
            .register_world(
                world_descriptor(
                    "world.ecs",
                    "ECS tables",
                    "Entity IDs and separate Transform, Parent, Color and Motion component tables.",
                ),
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
        self.versions
            .insert(descriptor.id.clone(), env!("CARGO_PKG_VERSION").into());
        self.descriptors.push(descriptor);
        Ok(())
    }

    /// Whether a contract has an installed executable world implementation.
    pub fn is_world_provider(&self, id: &str) -> bool {
        self.world_factories.contains_key(id)
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
        self.renderer_factories.insert(id, Arc::new(factory));
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

    pub(crate) fn create_renderer(
        &self,
        node: &Node,
        document: &GraphDocument,
    ) -> Result<Box<dyn FrameRenderer>, String> {
        self.renderer_factories
            .get(&node.module)
            .ok_or_else(|| format!("No renderer registered for '{}'", node.module))?(
            node, document
        )
    }

    pub(crate) fn create_world(
        &self,
        id: &str,
        params: &Value,
    ) -> Result<Box<dyn WorldProvider>, String> {
        self.world_factories
            .get(id)
            .ok_or_else(|| format!("No executable provider registered for '{id}'"))?(params)
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
