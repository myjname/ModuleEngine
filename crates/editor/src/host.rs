//! One composition root used by the editor, CLI and project exporter.

use module_engine_core::{
    Camera, Frame, FrameRenderer, Graph, GraphDocument, ModuleDescriptor, ModuleRegistry,
    PortDescriptor, RenderOverrides, SceneSnapshot, WorldKind, demo_document,
};
use module_engine_vulkan::{RenderPath, VulkanRenderer};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

type SharedVulkan = Arc<Mutex<Option<VulkanRenderer>>>;

struct VulkanModule {
    resource_root: std::path::PathBuf,
    backend: SharedVulkan,
    graph: Graph,
    sources: BTreeMap<String, String>,
    name: String,
}

impl FrameRenderer for VulkanModule {
    fn render(
        &mut self,
        scene: &SceneSnapshot,
        camera: Camera,
        width: usize,
        height: usize,
        overrides: &RenderOverrides,
    ) -> Result<Frame, String> {
        // Candidate compilation holds the device lock off the UI thread. Keep
        // the last image visible instead of waiting on that compilation here.
        let mut lock = self
            .backend
            .try_lock()
            .map_err(|_| "GPU готовит новую конфигурацию; показан последний кадр".to_string())?;
        lock.as_mut()
            .ok_or("Vulkan device not initialized")?
            .render(
                scene,
                camera,
                width,
                height,
                &self.graph,
                &self.sources,
                overrides,
            )
    }

    fn backend_name(&self) -> &str {
        &self.name
    }
    fn validate_overrides(&self, overrides: &RenderOverrides) -> Result<(), String> {
        module_engine_vulkan::validate_overrides_at(&self.graph, overrides, &self.resource_root)
    }
}

pub fn registry() -> Result<ModuleRegistry, String> {
    registry_at(&module_engine_vulkan::resource_root())
}
/// The selected package supplies includes, DXC and cache paths even if the
/// executable was launched from another full source checkout.
pub fn registry_at(resource_root: &std::path::Path) -> Result<ModuleRegistry, String> {
    let resource_root = resource_root.to_path_buf();
    let mut registry = ModuleRegistry::builtin();
    for descriptor in module_engine_vulkan::module_descriptors() {
        if descriptor.id != "render.vulkan" {
            registry.register_module(descriptor)?;
        }
    }
    let backend: SharedVulkan = Arc::new(Mutex::new(None));
    registry.register_renderer(ModuleDescriptor {
        id:"render.vulkan".into(), label:"Vulkan renderer".into(),
        description:"Editable Vulkan graphics/compute passes compiled from HLSL. Inspect the nested graph.".into(),
        inputs:vec![PortDescriptor{name:"scene".into(),data_type:"Scene3D".into(),required:true},PortDescriptor{name:"commands".into(),data_type:"RenderCommands".into(),required:false}],
        outputs:vec![PortDescriptor{name:"frame".into(),data_type:"Frame3D".into(),required:false}],properties:vec![],
    }, move |node,document| {
        if !node.params.is_null() && !node.params.as_object().is_some_and(|p|p.is_empty()) {return Err("Vulkan module parameters are declared in its passes; outer parameters must be empty".into());}
        let graph = node.subgraph.as_deref().ok_or("Vulkan renderer needs a pass graph")?.clone();
        let mut lock = backend.lock().map_err(|_| "Vulkan device lock is poisoned")?;
        if lock.is_none() { *lock = Some(VulkanRenderer::new_at(resource_root.clone())?); }
        let gpu = lock.as_mut().unwrap();
        gpu.prepare(&graph,&document.shader_sources)?;
        let name = format!("Vulkan / {}", gpu.device_name());
        drop(lock);
        Ok(Box::new(VulkanModule{backend:backend.clone(),graph,sources:document.shader_sources.clone(),name,resource_root:resource_root.clone()}))
    })?;
    Ok(registry)
}

pub fn gpu_document(kind: WorldKind, path: RenderPath) -> GraphDocument {
    let mut document = demo_document(kind);
    document.name = format!("{:?} / {:?} / Vulkan", kind, path);
    let renderer = document
        .root
        .nodes
        .iter_mut()
        .find(|n| n.id == "renderer")
        .unwrap();
    renderer.module = "render.vulkan".into();
    renderer.label = "Vulkan renderer".into();
    renderer.subgraph = Some(Box::new(module_engine_vulkan::default_graph(path)));
    document
}

/// A legacy graph gains an explicit Project surface before authoring/exporting.
/// Existing initialized declarations always remain authoritative.
pub fn ensure_authoring_configuration(document: &mut GraphDocument) {
    if document.engine.initialized {
        return;
    }
    document.engine.initialized = true;
    if let Some(graph) = document
        .root
        .nodes
        .iter()
        .find(|node| node.module == "render.vulkan")
        .and_then(|node| node.subgraph.as_deref())
    {
        let tone = graph.nodes.iter().find(|node| node.module == "gpu.tonemap");
        let aa = graph.nodes.iter().find(|node| node.module == "gpu.fxaa");
        if let (Some(tone), Some(aa)) = (tone, aa) {
            document.engine.exposure =
                module_engine_core::default_renderer_exposure(&tone.id, &aa.id);
        }
    }
}

pub fn settings_demo_document(kind: WorldKind) -> GraphDocument {
    let mut document = gpu_document(kind, RenderPath::Deferred);
    ensure_authoring_configuration(&mut document);
    document.name = "ModuleEngine / configurable Project + component properties".into();
    document.engine.enabled_modules.insert("game.motion".into());
    document
        .properties
        .active
        .components
        .entry(1)
        .or_default()
        .insert(
            "game.motion".into(),
            BTreeMap::from([("game.motion.speed".into(), serde_json::json!(1.0))]),
        );
    let source = r#"fn update(ctx: &mut ScriptContext) {
    // These fields are supplied only while this object has the active component.
    if let Some(speed) = ctx.component_property(1, "game.motion", "game.motion.speed")
        .and_then(|value| value.as_f64()) {
        let x = 0.35 + (ctx.time * speed).sin() as f32;
        ctx.set_position(1, [x, 0.25, 0.0]);
        ctx.set_color(1, [245, 75, 125, 255]);
    }
}
"#;
    document
        .custom_sources
        .insert("controller".into(), source.into());
    document.root.nodes.push(module_engine_core::make_node(
        "controller",
        "custom.rust.update",
        "Game motion logic",
        [620., 540.],
        serde_json::json!({"source_id":"controller"}),
    ));
    document.root.edges.push(module_engine_core::make_edge(
        "controller",
        "world",
        "world",
        "commands",
    ));
    document.root.edges.push(module_engine_core::make_edge(
        "controller",
        "render",
        "renderer",
        "commands",
    ));
    document
}
