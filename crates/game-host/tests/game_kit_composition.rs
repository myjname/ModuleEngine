//! An independent, non-puzzle consumer through the real registered executor.
use module_engine_core::*;
use module_engine_game_kit::{
    actions::{ActionMap, ActionState, Binding, PhysicalEvent, Trigger},
    campaign::{Campaign, ScoreOrder},
    grid::{Cell, GridBounds, Occupancy},
    history::UndoJournal,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

type Progress = Campaign<UndoJournal<Vec<Cell>>>;
const KEY: &str = "formation.progress/1";
const STEP: &str = "formation.route";
const DT: f64 = 1. / 60.;

fn initial() -> Progress {
    let mut progress = Campaign::new(
        vec!["training".into(), "patrol".into()],
        ScoreOrder::HigherIsBetter,
    )
    .unwrap();
    progress.set_attempt(UndoJournal::new(vec![Cell::new(0, 0), Cell::new(1, 0)]));
    progress
}
struct Formation;
impl ExecutableSystem for Formation {
    fn step(&mut self, context: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        let mut progress: Progress = context.game.module_data.get(KEY).map_or_else(
            || Ok(initial()),
            |v| serde_json::from_value(v.clone()).map_err(|e| e.to_string()),
        )?;
        let mut history = progress
            .attempt()
            .cloned()
            .ok_or("Formation has no attempt")?;
        if context.input.pressed("formation.advance") {
            let mut grid = Occupancy::new(GridBounds::new(4, 2)?);
            for (id, cell) in [41_u64, 42].into_iter().zip(history.current()) {
                grid.insert(*cell, 0_u8, id)?;
            }
            let next = history
                .current()
                .iter()
                .map(|cell| cell.offset(1, 0).ok_or("Coordinate overflow".to_owned()))
                .collect::<Result<Vec<_>, _>>()?;
            // Formation members can atomically enter each other's previous cells.
            grid.relocate(&[(41, next[0]), (42, next[1])])?;
            history.commit(next);
        }
        if context.input.pressed("formation.undo") {
            history.undo();
        }
        if context.input.pressed("formation.finish") {
            progress.complete(75);
        }
        context.apply_commands(
            &[41_u64, 42]
                .into_iter()
                .zip(history.current())
                .map(|(id, cell)| WorldCommand::SetPosition {
                    id,
                    position: [cell.x as f32, 0., cell.y as f32],
                })
                .collect::<Vec<_>>(),
        )?;
        progress.set_attempt(history);
        context.game.module_data.insert(
            KEY.into(),
            serde_json::to_value(progress).map_err(|e| e.to_string())?,
        );
        if context.input.pressed("formation.reject") {
            return Err("Rejected tactical candidate".into());
        }
        Ok(())
    }
}
fn registry() -> SystemRegistry {
    let mut systems = SystemRegistry::builtin();
    systems
        .register(
            SystemDescriptor {
                id: STEP.into(),
                state_schema: "formation-route/1".into(),
                components: vec![],
                requires: vec![],
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
                ],
            },
            |_| Ok(Box::new(Formation)),
        )
        .unwrap();
    systems
}
fn project() -> GameProject {
    let mut game = GameProject::empty();
    game.entry_scene = "training".into();
    game.scenes = ["training", "patrol"]
        .into_iter()
        .map(|id| {
            (
                id.into(),
                AuthoredScene {
                    name: id.into(),
                    objects: [41_u64, 42]
                        .into_iter()
                        .enumerate()
                        .map(|(x, id)| {
                            AuthoredObject::new(WorldObject {
                                id,
                                parent: None,
                                transform: Transform {
                                    translation: [x as f32, 0., 0.],
                                    ..Default::default()
                                },
                                color: [80, 170, 220, 255],
                            })
                        })
                        .collect(),
                },
            )
        })
        .collect();
    game.execution = ExecutionGraph {
        resources: [("world", "GameWorld"), ("input", "GameInput")]
            .into_iter()
            .map(|(name, ty)| SystemResource {
                name: name.into(),
                data_type: ty.into(),
                external: true,
                initial: Value::Null,
            })
            .collect(),
        stages: vec![SystemStage {
            name: "Tactical orders".into(),
            nodes: vec![SystemNode {
                id: "formation".into(),
                module: STEP.into(),
                params: json!({}),
                after: vec![],
                bindings: BTreeMap::from([
                    ("world".into(), "world".into()),
                    ("input".into(), "input".into()),
                ]),
            }],
        }],
    };
    game
}
fn input(action: &str) -> GameInput {
    GameInput {
        actions: BTreeSet::from([action.into()]),
        ..Default::default()
    }
}
fn progress(game: &GameSession) -> Progress {
    serde_json::from_value(game.state().module_data[KEY].clone()).unwrap()
}

#[test]
fn tactical_source_composes_actions_grid_history_and_campaign_in_both_worlds() {
    let actions = ActionMap::new(
        vec!["formation.advance".into()],
        vec![Binding {
            key: "D".into(),
            action: "formation.advance".into(),
            trigger: Trigger::Pressed,
        }],
    )
    .unwrap();
    let source = project();
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        let mut game = GameSession::new_with_registry(source.clone(), kind, registry()).unwrap();
        let mut keys = ActionState::default();
        let pressed = actions.evaluate(&mut keys, &[PhysicalEvent::Pressed("D".into())], true);
        assert_eq!(pressed, actions.named_press("formation.advance").unwrap());
        game.step(
            DT,
            &GameInput {
                actions: pressed.pressed.into_iter().collect(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            game.state().world.object(41).unwrap().transform.translation,
            [1., 0., 0.]
        );
        assert_eq!(
            game.state().world.object(42).unwrap().transform.translation,
            [2., 0., 0.]
        );
        assert_eq!(progress(&game).attempt().unwrap().undo_len(), 1);
        let before = serde_json::to_value(game.state()).unwrap();
        let rejected = GameInput {
            actions: BTreeSet::from(["formation.advance".into(), "formation.reject".into()]),
            ..Default::default()
        };
        assert!(game.step(DT, &rejected).is_err());
        assert_eq!(serde_json::to_value(game.state()).unwrap(), before);
        // The executor retains rejected input for retry; an explicit restart is
        // required to discard that command. A clean session verifies undo/resume.
        let mut resumed = GameSession::new_with_registry(source.clone(), kind, registry()).unwrap();
        resumed
            .seed_module_data(BTreeMap::from([(
                KEY.into(),
                before["module_data"][KEY].clone(),
            )]))
            .unwrap();
        resumed.step(DT, &input("formation.undo")).unwrap();
        assert_eq!(
            resumed
                .state()
                .world
                .object(41)
                .unwrap()
                .transform
                .translation,
            [0., 0., 0.]
        );
        assert_eq!(progress(&resumed).attempt().unwrap().undo_len(), 0);
        resumed.step(DT, &input("formation.finish")).unwrap();
        assert_eq!(progress(&resumed).best("training"), Some(75));
        assert!(progress(&resumed).is_unlocked("patrol"));
    }
    assert_eq!(source, project());
}

#[test]
fn omitting_the_source_module_rejects_the_schedule_without_mutating_authored_data() {
    let source = project();
    let before = serde_json::to_vec(&source).unwrap();
    assert!(
        GameSession::new_with_registry(source.clone(), WorldKind::Ecs, SystemRegistry::builtin())
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&source).unwrap(), before);
}
