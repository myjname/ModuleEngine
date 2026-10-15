//! Independently authored projects exercise reusable contracts without a demo.
use crate::{ArpgEvent, GameStateExt};
use module_engine_core::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const PLAYER: u64 = 94_117;

fn object(id: u64, components: Value) -> AuthoredObject {
    let mut object = AuthoredObject::new(WorldObject {
        id,
        parent: None,
        transform: Transform::default(),
        color: [137, 91, 208, 255],
    });
    object.components = serde_json::from_value(components).unwrap();
    object
}

fn project(scene: &str, opponents: usize) -> GameProject {
    let mut project = GameProject::empty();
    let mut objects = vec![object(
        PLAYER,
        json!({
            "player":{}, "actor":{"team":"player"}, "health":{"current":9.,"maximum":9.}
        }),
    )];
    for index in 0..opponents {
        objects.push(object(173 + index as u64 * 31, json!({
            "actor":{"team":"enemy"}, "health":{"current":0.,"maximum":7.},
            // An omitted flag and an explicit false must have identical policy.
            "boss":if index.is_multiple_of(2) { json!({}) } else { json!({"victory_on_death":false}) }
        })));
    }
    project.entry_scene = scene.into();
    project.scenes = BTreeMap::from([
        (
            scene.into(),
            AuthoredScene {
                name: format!("Source {scene}"),
                objects,
            },
        ),
        (
            format!("{scene}_annex"),
            AuthoredScene {
                name: "Unused authored scene".into(),
                objects: vec![],
            },
        ),
    ]);
    project.execution = ExecutionGraph {
        resources: vec![
            SystemResource {
                name: "objects".into(),
                data_type: "GameWorld".into(),
                external: true,
                initial: Value::Null,
            },
            SystemResource {
                name: "outcomes".into(),
                data_type: "GameEvents".into(),
                external: true,
                initial: json!([]),
            },
        ],
        stages: vec![
            SystemStage {
                name: "Authored movement decisions".into(),
                nodes: vec![SystemNode {
                    id: "consider".into(),
                    module: "game.ai".into(),
                    params: Value::Null,
                    bindings: BTreeMap::from([("world".into(), "objects".into())]),
                    after: vec![],
                }],
            },
            SystemStage {
                name: "Authored damage resolution".into(),
                nodes: vec![SystemNode {
                    id: "resolve".into(),
                    module: "game.health".into(),
                    params: Value::Null,
                    bindings: BTreeMap::from([
                        ("world".into(), "objects".into()),
                        ("events".into(), "outcomes".into()),
                    ]),
                    after: vec!["consider".into()],
                }],
            },
        ],
    };
    project
}

#[test]
fn renamed_scenes_and_different_boss_counts_never_imply_victory() {
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        for (scene, count) in [
            ("copper_garden", 1),
            ("snow_library", 4),
            ("lantern_archive", 9),
        ] {
            let mut session = crate::session(project(scene, count), kind).unwrap();
            for id in session.state().world.entities_with(&["boss"]) {
                assert_eq!(
                    session.state().world.component(id, "boss").unwrap()["victory_on_death"],
                    json!(false)
                );
            }
            session.step(1. / 60., &GameInput::default()).unwrap();
            assert_eq!(session.state().scene, scene);
            assert_eq!(session.state().living_enemies(), 0);
            assert_eq!(session.state().arpg().status, crate::GameStatus::Playing);
            assert!(!session.state().control.halted);
            assert_eq!(
                session.state().world.entity_ids().collect::<Vec<_>>(),
                vec![PLAYER]
            );
            let events: Vec<_> = session
                .events()
                .iter()
                .filter_map(|event| ArpgEvent::try_from(event).ok())
                .collect();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, ArpgEvent::Death { .. }))
                    .count(),
                count
            );
            assert!(!events.contains(&ArpgEvent::Victory));
            session.step(1. / 60., &GameInput::default()).unwrap();
            assert!(session.events().is_empty());
            assert!(!session.state().control.halted);
        }
    }
}

struct AuthoredOutcome {
    selected: bool,
    required_deaths: usize,
    result: String,
}
impl ExecutableSystem for AuthoredOutcome {
    fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
        let deaths = ctx
            .events
            .iter()
            .filter(|event| matches!(ArpgEvent::try_from(*event), Ok(ArpgEvent::Death { .. })))
            .count();
        if self.selected && deaths == self.required_deaths && !state.as_bool().unwrap_or_default() {
            let data = json!({"scene":ctx.game.scene,"result":self.result,"deaths":deaths});
            ctx.game
                .module_data
                .insert("garden.outcome/1".into(), data.clone());
            ctx.events.push(GameEvent::new("garden.harvested", data));
            ctx.game.control.halted = true;
            *state = json!(true);
        }
        Ok(())
    }
}

fn register_authored_rule(registry: &mut SystemRegistry) {
    registry
        .register(
            SystemDescriptor {
                id: "garden.outcome".into(),
                state_schema: "garden.outcome/1".into(),
                components: vec![],
                ports: vec![
                    SystemPort {
                        name: "world".into(),
                        data_type: "GameWorld".into(),
                        access: ResourceAccess::ReadWrite,
                    },
                    SystemPort {
                        name: "events".into(),
                        data_type: "GameEvents".into(),
                        access: ResourceAccess::ReadWrite,
                    },
                ],
                requires: vec![],
            },
            |params| {
                Ok(Box::new(AuthoredOutcome {
                    selected: params["selected"]
                        .as_bool()
                        .ok_or("Rule selection must be explicit")?,
                    required_deaths: params["required_deaths"]
                        .as_u64()
                        .ok_or("Rule needs an authored threshold")?
                        as usize,
                    result: params["result"]
                        .as_str()
                        .ok_or("Rule needs an authored result")?
                        .into(),
                }))
            },
        )
        .unwrap();
}

#[test]
fn installed_project_rule_chooses_its_own_outcome_without_changing_arpg() {
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        for (scene, count) in [("moss_orchard", 2), ("glass_courtyard", 6)] {
            for selected in [false, true] {
                let mut project = project(scene, count);
                project.execution.stages.push(SystemStage {
                    name:"Source project outcome".into(),nodes:vec![SystemNode {
                        id:"garden_result".into(),module:"garden.outcome".into(),
                        params:json!({"selected":selected,"required_deaths":count,"result":"harvest stored"}),
                        bindings:BTreeMap::from([("world".into(),"objects".into()),("events".into(),"outcomes".into())]),after:vec!["resolve".into()],
                    }],
                });
                let mut registry = crate::registry();
                register_authored_rule(&mut registry);
                let mut session = GameSession::new_with_registry(project, kind, registry).unwrap();
                session.step(1. / 60., &GameInput::default()).unwrap();
                assert_eq!(session.state().control.halted, selected);
                assert_eq!(session.state().arpg().status, crate::GameStatus::Playing);
                assert_eq!(
                    session.state().module_data.contains_key("garden.outcome/1"),
                    selected
                );
                let outcome = session
                    .events()
                    .iter()
                    .find(|event| event.name == "garden.harvested");
                assert_eq!(outcome.is_some(), selected);
                assert!(session.events().iter().all(|event| event.name != "Victory"));
                if let Some(outcome) = outcome {
                    assert_eq!(
                        outcome.payload,
                        json!({"scene":scene,"result":"harvest stored","deaths":count})
                    );
                    assert_eq!(
                        session.state().module_data["garden.outcome/1"],
                        outcome.payload
                    );
                }
            }
        }
    }
}

#[test]
fn an_authored_boss_can_explicitly_opt_into_the_victory_policy() {
    let mut project = project("cedar_observatory", 1);
    project.scenes.get_mut("cedar_observatory").unwrap().objects[1]
        .components
        .get_mut("boss")
        .unwrap()["victory_on_death"] = json!(true);
    let mut session = crate::session(project, WorldKind::Ecs).unwrap();
    session.step(1. / 60., &GameInput::default()).unwrap();
    assert!(session.state().control.halted);
    assert_eq!(session.state().arpg().status, crate::GameStatus::Victory);
    assert!(session.events().iter().any(|event| event.name == "Victory"));
}
