//! Installation metadata and the core renderer boundary for Vulkan.

use crate::{
    RenderGraphValidator, VulkanRenderer, module_descriptors, render_graph_targets,
    shader_dependency_revision_at,
};
use module_engine_core::{
    Camera, Frame, FrameRenderer, Graph, GraphDocument, ModuleDescriptor, ModuleRegistry, Node,
    PortDescriptor, RenderControlTargets, RenderOverrides, RendererAdapter, RendererPerformance,
    SceneSnapshot,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Install contracts and factories without opening a device or loading tools.
/// Each factory invocation owns a fresh backend, so background preparation
/// cannot block or reconfigure a previously accepted renderer.
pub fn install(registry: &mut ModuleRegistry, resource_root: &Path) -> Result<(), String> {
    for descriptor in module_descriptors() {
        registry.register_module(descriptor)?;
    }
    let resource_root = resource_root.to_path_buf();
    registry.register_renderer_with_adapter(
        renderer_descriptor(),
        VulkanAdapter::new(resource_root.clone()),
        move |node, document| {
            Ok(Box::new(VulkanFrameRenderer::new(
                &resource_root,
                node,
                document,
            )?))
        },
    )
}

fn renderer_descriptor() -> ModuleDescriptor {
    ModuleDescriptor {
        id: "render.vulkan".into(),
        label: "Vulkan renderer".into(),
        description:
            "Editable Vulkan graphics/compute passes compiled from HLSL. Inspect the nested graph."
                .into(),
        inputs: vec![
            PortDescriptor {
                name: "scene".into(),
                data_type: "Scene3D".into(),
                required: true,
            },
            PortDescriptor {
                name: "commands".into(),
                data_type: "RenderCommands".into(),
                required: false,
            },
        ],
        outputs: vec![PortDescriptor {
            name: "frame".into(),
            data_type: "Frame3D".into(),
            required: false,
        }],
        properties: vec![],
    }
}

fn renderer_graph(node: &Node) -> Result<&Graph, String> {
    if node.module != "render.vulkan" {
        return Err(format!("Expected Vulkan renderer, got '{}'", node.module));
    }
    if !node.params.is_null()
        && !node
            .params
            .as_object()
            .is_some_and(|params| params.is_empty())
    {
        return Err(
            "Vulkan module parameters are declared in its passes; outer parameters must be empty"
                .into(),
        );
    }
    node.subgraph
        .as_deref()
        .ok_or_else(|| "Vulkan renderer needs a pass graph".into())
}

/// Cheap metadata remains available even when the first shader candidate fails.
#[derive(Clone, Debug)]
pub struct VulkanAdapter {
    resource_root: PathBuf,
}

impl VulkanAdapter {
    pub fn new(resource_root: PathBuf) -> Self {
        Self { resource_root }
    }
}

impl RendererAdapter for VulkanAdapter {
    fn dependency_revision(&self, node: &Node, document: &GraphDocument) -> Result<u64, String> {
        shader_dependency_revision_at(
            renderer_graph(node)?,
            &document.shader_sources,
            &self.resource_root,
        )
    }

    fn control_targets(
        &self,
        node: &Node,
        _document: &GraphDocument,
    ) -> Result<Option<RenderControlTargets>, String> {
        let (controls, resources) = render_graph_targets(renderer_graph(node)?);
        Ok(Some(RenderControlTargets {
            controls,
            resources,
        }))
    }

    fn validate_controls(
        &self,
        node: &Node,
        _document: &GraphDocument,
        controls: &RenderOverrides,
    ) -> Result<(), String> {
        RenderGraphValidator::new(renderer_graph(node)?)?.validate(controls)
    }
}

/// Configuration belongs to the wrapper; reusable pipelines belong to its
/// backend. A synchronous candidate may share the latter without selecting the
/// candidate graph before the host publishes it.
struct VulkanFrameRenderer {
    backend: Arc<Mutex<VulkanRenderer>>,
    graph: Graph,
    sources: BTreeMap<String, String>,
    name: String,
}

impl VulkanFrameRenderer {
    fn new(resource_root: &Path, node: &Node, document: &GraphDocument) -> Result<Self, String> {
        let graph = renderer_graph(node)?;
        RenderGraphValidator::new(graph)?.validate(&RenderOverrides::default())?;
        let mut backend = VulkanRenderer::new_at(resource_root.to_path_buf())?;
        backend.prepare(graph, &document.shader_sources)?;
        let name = format!("Vulkan / {}", backend.device_name());
        Ok(Self {
            backend: Arc::new(Mutex::new(backend)),
            graph: graph.clone(),
            sources: document.shader_sources.clone(),
            name,
        })
    }
}

impl FrameRenderer for VulkanFrameRenderer {
    fn render(
        &mut self,
        scene: &SceneSnapshot,
        camera: Camera,
        width: usize,
        height: usize,
        overrides: &RenderOverrides,
    ) -> Result<Frame, String> {
        let mut backend = self
            .backend
            .try_lock()
            .map_err(|_| "GPU готовит новую конфигурацию; показан последний кадр".to_string())?;
        backend.render(
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

    fn performance(&self) -> RendererPerformance {
        self.backend
            .lock()
            .map(|backend| backend.performance())
            .unwrap_or_default()
    }

    fn set_detailed_stats(&mut self, enabled: bool) {
        if let Ok(mut backend) = self.backend.lock() {
            backend.set_detailed_stats(enabled);
        }
    }

    fn prepare_candidate(
        &mut self,
        node: &Node,
        document: &GraphDocument,
    ) -> Result<Option<Box<dyn FrameRenderer>>, String> {
        let graph = renderer_graph(node)?;
        RenderGraphValidator::new(graph)?.validate(&RenderOverrides::default())?;
        self.backend
            .lock()
            .map_err(|_| "Vulkan device lock is poisoned".to_string())?
            .prepare(graph, &document.shader_sources)?;
        Ok(Some(Box::new(Self {
            backend: self.backend.clone(),
            graph: graph.clone(),
            sources: document.shader_sources.clone(),
            name: self.name.clone(),
        })))
    }

    fn validate_overrides(&self, overrides: &RenderOverrides) -> Result<(), String> {
        RenderGraphValidator::new(&self.graph)?.validate(overrides)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RenderPath, custom_compute_manifest, default_graph, demo_custom_compute_source};
    use module_engine_core::{ObjectsWorld, WorldKind, WorldProvider, demo_document, make_node};
    use serde_json::json;

    fn document() -> GraphDocument {
        let mut document = demo_document(WorldKind::Ecs);
        let renderer = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "renderer")
            .unwrap();
        renderer.module = "render.vulkan".into();
        renderer.subgraph = Some(Box::new(default_graph(RenderPath::Forward)));
        document
    }

    fn node(document: &GraphDocument) -> &Node {
        document
            .root
            .nodes
            .iter()
            .find(|node| node.id == "renderer")
            .unwrap()
    }

    fn custom_document(source: String) -> GraphDocument {
        let mut document = document();
        let graph = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "renderer")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap();
        let ao = graph.nodes.iter_mut().find(|node| node.id == "ao").unwrap();
        ao.module = "gpu.custom".into();
        ao.params = json!({
            "source_id":"adapter-test",
            "manifest":custom_compute_manifest(),
            "gain":1.0,
        });
        graph
            .edges
            .retain(|edge| edge.to_node != "ao" || edge.to_port == "color");
        document
            .shader_sources
            .insert("adapter-test".into(), source);
        document
    }

    #[test]
    fn installation_metadata_and_invalid_factory_inputs_need_no_gpu_or_tool_root() {
        let absent = std::env::temp_dir().join("module-engine-absent-vulkan-adapter-root");
        let mut registry = ModuleRegistry::builtin();
        install(&mut registry, &absent).unwrap();
        let document = custom_document("#include \"missing.hlsli\"\ninvalid HLSL".into());
        let adapter = registry.renderer_adapter("render.vulkan").unwrap();
        adapter
            .validate_controls(node(&document), &document, &RenderOverrides::default())
            .unwrap();
        assert!(
            adapter
                .dependency_revision(node(&document), &document)
                .is_err()
        );
        let targets = adapter
            .control_targets(node(&document), &document)
            .unwrap()
            .unwrap();
        assert!(targets.controls.contains("ao"));
        assert!(targets.resources.contains("ao.color"));

        let mut unknown = RenderOverrides::default();
        unknown.enabled.insert("fresh-unknown-pass".into(), false);
        assert!(
            adapter
                .validate_controls(node(&document), &document, &unknown)
                .is_err()
        );
        let mut bad_node = node(&document).clone();
        bad_node.params = json!({"unrecognized":1});
        let error = registry
            .create_renderer(&bad_node, &document)
            .err()
            .unwrap();
        assert!(error.contains("outer parameters must be empty"), "{error}");
        bad_node.params = json!({});
        bad_node.subgraph = None;
        let error = registry
            .create_renderer(&bad_node, &document)
            .err()
            .unwrap();
        assert!(error.contains("needs a pass graph"), "{error}");
    }

    #[test]
    fn configuration_key_ignores_only_presentation_and_tracks_all_consumed_sources() {
        let adapter = VulkanAdapter::new(PathBuf::from("unused"));
        let document = document();
        let original = node(&document);
        let key = adapter.configuration_key(original, &document);
        let mut layout = original.clone();
        layout.label = "renamed".into();
        layout.position = [888., 42.];
        let pass = &mut layout.subgraph.as_mut().unwrap().nodes[1];
        pass.label = "changed pass label".into();
        pass.position = [-99., 60.];
        assert_eq!(key, adapter.configuration_key(&layout, &document));
        pass_parameter(&mut layout, "tonemap", "exposure", json!(0.2));
        assert_ne!(key, adapter.configuration_key(&layout, &document));
        let mut unknown_parameter = original.clone();
        unknown_parameter.params = json!({"unknown":1});
        assert_ne!(
            key,
            adapter.configuration_key(&unknown_parameter, &document)
        );
        let mut sources = document.clone();
        sources
            .shader_sources
            .insert("new-source".into(), "different HLSL".into());
        assert_ne!(key, adapter.configuration_key(original, &sources));

        let mut custom = original.clone();
        custom.subgraph.as_mut().unwrap().nodes.push(make_node(
            "custom",
            "custom.rust",
            "Custom",
            [0., 0.],
            json!({"code":"fn update() {}", "code_layout":{"x":1}}),
        ));
        let before = adapter.configuration_key(&custom, &document);
        custom
            .subgraph
            .as_mut()
            .unwrap()
            .nodes
            .last_mut()
            .unwrap()
            .params["code_layout"] = json!({"x":2});
        assert_eq!(before, adapter.configuration_key(&custom, &document));
        custom
            .subgraph
            .as_mut()
            .unwrap()
            .nodes
            .last_mut()
            .unwrap()
            .params["code"] = json!("fn update() { changed(); }");
        assert_ne!(before, adapter.configuration_key(&custom, &document));
    }

    fn pass_parameter(node: &mut Node, pass: &str, name: &str, value: serde_json::Value) {
        node.subgraph
            .as_mut()
            .unwrap()
            .nodes
            .iter_mut()
            .find(|node| node.id == pass)
            .unwrap()
            .params[name] = value;
    }

    #[test]
    fn dependency_metadata_observes_missing_repaired_and_changed_include_bytes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let relative = format!(
            ".work/vulkan-adapter-dependency-{}.hlsli",
            std::process::id()
        );
        std::fs::create_dir_all(root.join(".work")).unwrap();
        let header = root.join(&relative);
        if header.exists() {
            std::fs::remove_file(&header).unwrap();
        }
        let adapter = VulkanAdapter::new(root);
        let document = custom_document(format!(
            "#include \"{relative}\"\n{}",
            demo_custom_compute_source()
        ));
        assert!(
            adapter
                .dependency_revision(node(&document), &document)
                .is_err()
        );
        std::fs::write(&header, "float adapter_gain() { return 0.2; }\n").unwrap();
        let repaired = adapter
            .dependency_revision(node(&document), &document)
            .unwrap();
        assert_eq!(
            repaired,
            adapter
                .dependency_revision(node(&document), &document)
                .unwrap()
        );
        std::fs::write(&header, "float adapter_gain() { return 0.8; }\n").unwrap();
        assert_ne!(
            repaired,
            adapter
                .dependency_revision(node(&document), &document)
                .unwrap()
        );
        std::fs::remove_file(&header).unwrap();
        assert!(
            adapter
                .dependency_revision(node(&document), &document)
                .is_err()
        );
    }

    #[test]
    fn factories_isolate_devices_and_sync_candidates_share_cache_without_selecting_configuration() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let document = document();
        let mut registry = ModuleRegistry::builtin();
        install(&mut registry, &root).unwrap();
        let mut accepted = registry
            .create_renderer(node(&document), &document)
            .unwrap();
        let independent = registry
            .create_renderer(node(&document), &document)
            .unwrap();
        let scene = ObjectsWorld::demo().snapshot();
        let render = |renderer: &mut dyn FrameRenderer| {
            renderer
                .render(
                    &scene,
                    Camera::default(),
                    96,
                    64,
                    &RenderOverrides::default(),
                )
                .unwrap()
        };
        let baseline = render(accepted.as_mut());
        assert_eq!(accepted.performance().render_calls, 1);
        assert_eq!(
            independent.performance().render_calls,
            0,
            "registered factories cannot share a backend"
        );
        drop(independent);
        let before = accepted.performance();
        let mut changed = node(&document).clone();
        pass_parameter(&mut changed, "tonemap", "exposure", json!(0.2));
        let mut candidate = accepted
            .prepare_candidate(&changed, &document)
            .unwrap()
            .unwrap();
        assert_eq!(
            accepted.performance().pipeline_creations,
            before.pipeline_creations,
            "parameter-only candidate reuses compiled pipelines"
        );
        let changed_frame = render(candidate.as_mut());
        assert_ne!(baseline.rgba, changed_frame.rgba);
        assert_eq!(
            accepted.performance().render_calls,
            candidate.performance().render_calls,
            "synchronous candidate shares actual cache counters"
        );
        assert_eq!(
            baseline.rgba,
            render(accepted.as_mut()).rgba,
            "unpublished candidate cannot select the accepted graph"
        );
        let rejected = custom_document("invalid HLSL which cannot compile".into());
        assert!(
            accepted
                .prepare_candidate(node(&rejected), &rejected)
                .is_err()
        );
        assert_eq!(
            baseline.rgba,
            render(accepted.as_mut()).rgba,
            "failed candidate retains accepted configuration"
        );
    }
}
