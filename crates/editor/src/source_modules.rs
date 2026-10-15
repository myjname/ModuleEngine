//! Editor authoring regressions for the shared native source extension.

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};
    const SPIN: &str = "example.spin";
    use module_engine_core::*;
    use std::collections::BTreeMap;

    pub(crate) fn document() -> GraphDocument {
        let mut document =
            crate::host::gpu_document(WorldKind::Ecs, module_engine_vulkan::RenderPath::Forward);
        document.root.nodes.retain(|node| {
            !matches!(
                node.module.as_str(),
                "custom.rust" | "custom.rust.update" | "clock.time" | "scalar.value"
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
        let object = AuthoredObject::new(WorldObject {
            id: 1,
            parent: None,
            transform: Transform::default(),
            color: [180, 150, 90, 255],
        });
        document.game = Some(GameProject {
            entry_scene: "empty".into(),
            scenes: BTreeMap::from([(
                "empty".into(),
                AuthoredScene {
                    name: "Empty".into(),
                    objects: vec![object],
                },
            )]),
            prefabs: BTreeMap::new(),
            execution: ExecutionGraph {
                resources: vec![SystemResource {
                    name: "world".into(),
                    data_type: "GameWorld".into(),
                    external: true,
                    initial: Value::Null,
                }],
                stages: vec![SystemStage {
                    name: "Update".into(),
                    nodes: vec![SystemNode {
                        id: "rotate".into(),
                        module: SPIN.into(),
                        params: json!({}),
                        bindings: BTreeMap::from([("world".into(), "world".into())]),
                        after: vec![],
                    }],
                }],
            },
            audio_cues: BTreeMap::new(),
        });
        crate::engine_profiles::finish_template(
            &mut document,
            crate::engine_profiles::ProfileKind::Neutral,
            &module_engine_vulkan::resource_root(),
        )
        .expect("Valid explicit source-extension fixture");
        document
    }

    #[test]
    fn registered_source_component_defaults_survive_save_reopen_and_execute() {
        let root = crate::project::source_root();
        let composition = crate::host::composition_at(&root).unwrap();
        let mut document = document();
        let defaults = composition
            .systems
            .components()
            .defaults(SPIN)
            .unwrap()
            .clone();
        let entity = &mut document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("empty")
            .unwrap()
            .objects[0];
        entity.components.insert(SPIN.into(), defaults);
        entity.components.get_mut(SPIN).unwrap()["speed"] = json!(3.);
        crate::project::stamp_module_versions(&mut document, &composition.modules);
        let folder = root.join(".work").join(format!(
            "source-module-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("game.json");
        crate::project::save(&path, &document).unwrap();
        let reopened = crate::project::read(&path).unwrap();
        crate::host::validate_document_at(&reopened, &root).unwrap();
        let prepared = crate::host::prepare_game_at(&reopened, &root).unwrap();
        let mut session =
            GameSession::new_with_registry(prepared, WorldKind::Ecs, composition.systems).unwrap();
        session.step(1. / 60., &GameInput::default()).unwrap();
        assert!(
            (session.state().world.object(1).unwrap().transform.rotation[1] - 0.05).abs() < 0.00001
        );
        assert_eq!(
            reopened.game.as_ref().unwrap().scenes["empty"].objects[0].components[SPIN]["speed"],
            json!(3.)
        );
        let exported = crate::project::export_with_package_root(
            &folder.join("export"),
            &reopened,
            &folder,
            &root,
        )
        .unwrap();
        assert!(
            exported
                .join("crates/editor/src/source_modules.rs")
                .is_file()
        );
        let exported_document = crate::project::read(&exported.join("project.json")).unwrap();
        crate::host::validate_document_at(&exported_document, &exported).unwrap();
        assert_eq!(
            exported_document.game.as_ref().unwrap().scenes["empty"].objects[0].components[SPIN]["speed"],
            json!(3.)
        );
        let owned_root = root.join(".work").canonicalize().unwrap();
        let folder = folder.canonicalize().unwrap();
        assert!(folder.starts_with(&owned_root) && folder != owned_root);
        std::fs::remove_dir_all(folder).unwrap();
    }
}
