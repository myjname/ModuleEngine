//! Application creation presets. Concrete game data belongs to its project.
use module_engine_core::{GameProject, GraphDocument, WorldKind};
use module_engine_vulkan::RenderPath;

fn starter_engine() -> GraphDocument {
    let mut document = crate::host::gpu_document(WorldKind::Ecs, RenderPath::Forward);
    crate::host::ensure_authoring_configuration(&mut document);
    document
}

pub fn demo_document() -> GraphDocument {
    let mut document = module_engine_two_arenas::document(starter_engine());
    crate::engine_profiles::finish_template(
        &mut document,
        crate::engine_profiles::ProfileKind::Arpg,
        &module_engine_vulkan::resource_root(),
    )
    .expect("Installed ARPG template contracts");
    document
}

pub fn puzzle_document() -> GraphDocument {
    source_game_document(
        module_engine_project_puzzle::project(),
        "ModuleEngine / Crate puzzle",
        crate::engine_profiles::ProfileKind::Puzzle,
    )
}
pub fn g1_document() -> GraphDocument {
    source_game_document(
        module_engine_g1::project(),
        "Crate Garden",
        crate::engine_profiles::ProfileKind::G1,
    )
}
fn source_game_document(
    game: GameProject,
    name: &str,
    profile: crate::engine_profiles::ProfileKind,
) -> GraphDocument {
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
    document.name = name.into();
    document.game = Some(game);
    crate::engine_profiles::finish_template(
        &mut document,
        profile,
        &module_engine_vulkan::resource_root(),
    )
    .expect("Installed source game template contracts");
    document
}
