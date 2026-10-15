//! Application creation presets. Concrete game data belongs to its project.
use module_engine_core::{GraphDocument, WorldKind};
use module_engine_vulkan::RenderPath;

fn starter_engine() -> GraphDocument {
    let mut document = crate::host::gpu_document(WorldKind::Ecs, RenderPath::Forward);
    crate::host::ensure_authoring_configuration(&mut document);
    document
}

pub fn demo_document() -> GraphDocument {
    module_engine_two_arenas::document(starter_engine())
}

pub fn puzzle_document() -> GraphDocument {
    let mut document = starter_engine();
    // The starter's scalar demonstration is not part of a selected game.
    document.root.nodes.retain(|node| {
        !matches!(
            node.module.as_str(),
            "custom.rust" | "clock.time" | "scalar.value"
        )
    });
    let ids = document
        .root
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    document
        .root
        .edges
        .retain(|edge| ids.contains(&edge.from_node) && ids.contains(&edge.to_node));
    document.custom_sources.clear();
    document.name = "ModuleEngine / Crate puzzle".into();
    document.game = Some(module_engine_project_puzzle::project());
    document
}
