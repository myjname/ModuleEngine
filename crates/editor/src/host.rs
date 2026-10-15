//! One composition root used by the editor, CLI and project exporter.

use module_engine_core::{
    CompositionContext, GraphDocument, ModuleRegistry, ResourceAccess, SystemDescriptor,
    SystemPort, SystemRegistry, WorldKind, demo_document,
};
use module_engine_vulkan::RenderPath;
use std::collections::BTreeMap;

pub fn registry() -> Result<ModuleRegistry, String> {
    registry_at(&module_engine_vulkan::resource_root())
}
/// The selected package supplies includes, DXC and cache paths even if the
/// executable was launched from another full source checkout.
pub fn registry_at(resource_root: &std::path::Path) -> Result<ModuleRegistry, String> {
    Ok(composition_at(resource_root)?.modules)
}

/// The installed native catalog is shared by palettes, authoring validation,
/// runtime preparation and exporters. Host-bound adapters only replace factories
/// of these same contracts; they do not construct a separate builtin catalog.
pub fn composition_at(resource_root: &std::path::Path) -> Result<CompositionContext, String> {
    let modules = module_registry_at(resource_root)?;
    let mut systems = SystemRegistry::builtin();
    module_engine_arpg::install(&mut systems)?;
    module_engine_project_puzzle::install(&mut systems)?;
    crate::source_modules::install(&mut systems)?;
    systems.register(custom_system_descriptor(), |_| {
        Err("Rust Custom requires a host-bound compiled instance".into())
    })?;
    CompositionContext::new(modules, systems)
}

pub fn validate_document_at(
    document: &GraphDocument,
    resource_root: &std::path::Path,
) -> Result<(), String> {
    let composition = composition_at(resource_root)?;
    let errors = composition
        .validate_document(document)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == module_engine_core::Severity::Error)
        .map(|diagnostic| diagnostic.message)
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn validate_game_at(
    document: &GraphDocument,
    resource_root: &std::path::Path,
) -> Result<(), String> {
    composition_at(resource_root)?.validate_game(document)
}

pub fn asset_workspace_at(
    asset_root: &std::path::Path,
    protected: Vec<String>,
    resource_root: &std::path::Path,
) -> Result<module_engine_project::AssetWorkspace, String> {
    let composition = composition_at(resource_root)?;
    Ok(
        module_engine_project::AssetWorkspace::new(asset_root, protected)?
            .with_validator(move |candidate| composition.validate_game(candidate)),
    )
}

/// Resolve the selected game's active data using the same schemas as the
/// Inspector and runtime, retaining the author's original document separately.
pub fn prepare_game_at(
    document: &GraphDocument,
    resource_root: &std::path::Path,
) -> Result<module_engine_core::GameProject, String> {
    let composition = composition_at(resource_root)?;
    let game = document
        .game
        .as_ref()
        .ok_or("Project has no authored game")?;
    let activity = composition.activity(document)?;
    let asset_paths =
        module_engine_project::AssetManifest::from_value(&document.asset_manifest)?.asset_paths();
    let context = module_engine_core::ComponentValidationContext {
        scenes: &game.scenes,
        modules: &activity.modules,
        asset_paths: Some(&asset_paths),
        active_components: Some(&activity.components),
    };
    game.prepare_runtime_with_context(&composition.systems, &context)
}

pub fn component_asset_references(
    document: &GraphDocument,
    resource_root: &std::path::Path,
) -> Result<std::collections::BTreeSet<String>, String> {
    let Some(game) = &document.game else {
        return Ok(Default::default());
    };
    let composition = composition_at(resource_root)?;
    let activity = composition.activity(document)?;
    let asset_paths =
        module_engine_project::AssetManifest::from_value(&document.asset_manifest)?.asset_paths();
    let context = module_engine_core::ComponentValidationContext {
        scenes: &game.scenes,
        modules: &activity.modules,
        asset_paths: Some(&asset_paths),
        active_components: Some(&activity.components),
    };
    let mut references = std::collections::BTreeSet::new();
    for id in game.scenes.keys() {
        let entities = game.resolve_scene_with_context(id, &composition.systems, &context)?;
        references.extend(
            composition
                .systems
                .components()
                .asset_references(&entities, &activity.components)?,
        );
    }
    Ok(references)
}

pub fn active_game_entities_at(
    document: &GraphDocument,
    resource_root: &std::path::Path,
) -> Result<Vec<module_engine_core::GameEntity>, String> {
    let Some(game) = &document.game else {
        return Ok(vec![]);
    };
    let composition = composition_at(resource_root)?;
    let activity = composition.activity(document)?;
    let asset_paths =
        module_engine_project::AssetManifest::from_value(&document.asset_manifest)?.asset_paths();
    let context = module_engine_core::ComponentValidationContext {
        scenes: &game.scenes,
        modules: &activity.modules,
        asset_paths: Some(&asset_paths),
        active_components: Some(&activity.components),
    };
    let mut result = vec![];
    for id in game.scenes.keys() {
        for mut entity in game.resolve_scene_with_context(id, &composition.systems, &context)? {
            entity
                .components
                .retain(|name, _| activity.components.contains(name));
            result.push(entity);
        }
    }
    Ok(result)
}

fn custom_system_descriptor() -> SystemDescriptor {
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
    }
}

fn module_registry_at(resource_root: &std::path::Path) -> Result<ModuleRegistry, String> {
    let mut registry = ModuleRegistry::builtin();
    module_engine_vulkan::install(&mut registry, resource_root)?;
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
