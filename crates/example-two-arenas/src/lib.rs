//! The concrete Two Arenas example project, composed from reusable ARPG rules.
use module_engine_core::{GameProject, GraphDocument, SystemNode, SystemStage};
use serde_json::json;
use std::collections::BTreeMap;

pub mod content;

/// Build the example's scenes, templates and goals with the reusable ARPG schedule.
pub fn demo_project() -> GameProject {
    content::build_project(module_engine_arpg::gameplay_schedule())
}

/// Add this project's content to a supplied engine graph and settings.
/// The caller selects/configures the renderer; this package has no Vulkan dependency.
pub fn document(mut document: GraphDocument) -> GraphDocument {
    document.root.nodes.retain(|n| {
        !matches!(
            n.module.as_str(),
            "custom.rust" | "clock.time" | "scalar.value"
        )
    });
    let ids = document
        .root
        .nodes
        .iter()
        .map(|n| n.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    document
        .root
        .edges
        .retain(|e| ids.contains(&e.from_node) && ids.contains(&e.to_node));
    document.custom_sources.clear();
    let mut game = demo_project();
    for scene in game.scenes.values_mut() {
        if let Some(player) = scene.objects.iter_mut().find(|o| o.object.id == 1) {
            player.components.insert(
                "mesh".into(),
                json!({"path":"assets/models/demo_crystal.glb"}),
            );
        }
    }
    game.execution.stages.push(SystemStage {
        name: "Custom game logic".into(),
        nodes: vec![SystemNode {
            id: "game-controller".into(),
            module: "custom.rust.update".into(),
            params: json!({"source_id":"game-controller"}),
            bindings: BTreeMap::from([
                ("world".into(), "world".into()),
                ("input".into(), "input".into()),
                ("events".into(), "events".into()),
            ]),
            after: vec!["system-7".into()],
        }],
    });
    document.custom_sources.insert(
        "game-controller".into(),
        r#"// module-engine-state: {"schema":"game.counter.v1","initial":{"ticks":0,"hits":0}}
fn update(ctx: &mut ScriptContext) {
    let ticks = ctx.state["ticks"].as_u64().unwrap_or(0) + 1;
    let hits = ctx.state["hits"].as_u64().unwrap_or(0)
        + ctx.events.iter().filter(|event| event.get("Hit").is_some()).count() as u64;
    ctx.state["ticks"] = serde_json::json!(ticks);
    ctx.state["hits"] = serde_json::json!(hits);
    if ctx.input["interact"].as_bool() == Some(true) {
        ctx.set_color(1, [250, 180, 75, 255]);
        ctx.emit_event(serde_json::json!({"Sound":{"cue":"hit","object":1}}));
    }
}
"#
        .into(),
    );
    document.name = "ModuleEngine / Two arenas".into();
    document.game = Some(game);
    document
}

/// Convenience for callers explicitly importing this example project.
pub trait ProjectExt {
    fn demo() -> Self;
}
impl ProjectExt for GameProject {
    fn demo() -> Self {
        demo_project()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::ExecutionGraph;

    #[test]
    fn project_document_preserves_engine_settings_and_supplies_its_own_content() {
        let mut base = module_engine_core::demo_document(module_engine_core::WorldKind::Ecs);
        base.engine.initialized = true;
        base.project
            .parameters
            .insert("example.setting".into(), json!(0.7));
        base.shader_sources
            .insert("engine-owned".into(), "retained source".into());
        base.asset_manifest = json!({"engine-owned":"retained metadata"});
        base.module_versions
            .insert("world.ecs".into(), env!("CARGO_PKG_VERSION").into());
        let engine_nodes = base.root.nodes[..4].to_vec();
        let engine_edges = base.root.edges[..3].to_vec();
        let output = document(base.clone());
        assert_eq!(output.root.nodes, engine_nodes);
        assert_eq!(output.root.edges, engine_edges);
        assert_eq!(output.engine, base.engine);
        assert_eq!(output.project, base.project);
        assert_eq!(output.properties, base.properties);
        assert_eq!(output.shader_sources, base.shader_sources);
        assert_eq!(output.asset_manifest, base.asset_manifest);
        assert_eq!(output.module_versions, base.module_versions);
        assert_eq!(output.name, "ModuleEngine / Two arenas");
        assert_eq!(output.custom_sources.len(), 1);
        let source = &output.custom_sources["game-controller"];
        assert!(source.contains("game.counter.v1"));
        assert!(source.contains("ctx.events.iter().filter(|event| event.get(\"Hit\").is_some())"));
        assert!(source.contains("ctx.set_color(1, [250, 180, 75, 255])"));
        let game = output.game.unwrap();
        for scene in game.scenes.values() {
            let player = scene
                .objects
                .iter()
                .find(|entity| entity.object.id == 1)
                .unwrap();
            assert_eq!(
                player.components["mesh"]["path"],
                json!("assets/models/demo_crystal.glb")
            );
        }
        let controller = &game.execution.stages.last().unwrap().nodes[0];
        assert_eq!(controller.id, "game-controller");
        assert_eq!(controller.params["source_id"], json!("game-controller"));
        assert_eq!(controller.after, vec!["system-7"]);
    }

    #[test]
    fn example_content_uses_the_supplied_schedule_and_round_trips() {
        let schedule = ExecutionGraph::default();
        let project = content::build_project(schedule.clone());
        assert_eq!(project.execution, schedule);
        let restored: GameProject =
            serde_json::from_str(&serde_json::to_string(&project).unwrap()).unwrap();
        assert_eq!(restored, project);
    }

    #[test]
    fn complete_example_validates_against_the_reusable_arpg_contracts() {
        let project = demo_project();
        project
            .validate_with_registry(&module_engine_arpg::registry())
            .unwrap();
        let arena = project
            .resolve_scene_with_registry("arena", &module_engine_arpg::registry())
            .unwrap();
        assert_eq!(
            arena
                .iter()
                .filter(|entity| entity.components.contains_key("ai"))
                .count(),
            20
        );
        let boss = project
            .resolve_scene_with_registry("boss", &module_engine_arpg::registry())
            .unwrap();
        assert_eq!(
            boss.iter()
                .filter(
                    |entity| entity.components.get("boss").is_some_and(|settings| {
                        settings
                            .get("victory_on_death")
                            .and_then(|flag| flag.as_bool())
                            == Some(true)
                    })
                )
                .count(),
            1
        );
    }
}
