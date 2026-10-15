use super::*;
use crate::components::{BOARD, CELL};
use module_engine_core::*;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const DT: f64 = 1. / 60.;
const SOLUTION: [Direction; 5] = [
    Direction::North,
    Direction::East,
    Direction::South,
    Direction::East,
    Direction::North,
];
fn new_session(project: GameProject, kind: WorldKind) -> GameSession {
    GameSession::new_with_registry(project, kind, registry()).unwrap()
}
fn position(game: &GameState, id: u64) -> (i64, i64) {
    let value = game.world.component(id, CELL).unwrap();
    (value["x"].as_i64().unwrap(), value["z"].as_i64().unwrap())
}
fn board_mut(project: &mut GameProject) -> &mut AuthoredObject {
    project
        .scenes
        .get_mut("board")
        .unwrap()
        .objects
        .iter_mut()
        .find(|object| object.object.id == 1)
        .unwrap()
}

#[test]
fn authored_defaults_prefab_overrides_and_edit_save_reopen_use_the_same_contracts() {
    let mut authored = project();
    authored.prefabs.insert(
        "box".into(),
        Prefab {
            color: [235, 174, 75, 255],
            components: BTreeMap::from([(CELL.into(), json!({"kind":"crate","x":2,"z":2}))]),
        },
    );
    let crate_object = authored
        .scenes
        .get_mut("board")
        .unwrap()
        .objects
        .iter_mut()
        .find(|object| object.object.id == 101)
        .unwrap();
    crate_object.prefab = Some("box".into());
    crate_object
        .components
        .insert(CELL.into(), json!({"z":2.0}));
    crate_object
        .overrides
        .insert("puzzle.cell.x".into(), json!(2.0));
    let board = board_mut(&mut authored);
    board
        .overrides
        .insert("puzzle.board.max_turns".into(), json!(9.0));
    board
        .overrides
        .insert("puzzle.board.cell_size".into(), json!(1.25));
    board
        .components
        .insert("future.record".into(), json!({"opaque":[1,2,3]}));
    let saved = serde_json::to_string(&authored).unwrap();
    let reopened: GameProject = serde_json::from_str(&saved).unwrap();
    assert_eq!(reopened, authored);
    let registry = registry();
    reopened.validate_with_registry(&registry).unwrap();
    assert_eq!(
        registry
            .module_versions()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["puzzle.result", "puzzle.turn", "scene.extract"]
    );
    assert!(reopened.audio_cues.is_empty());
    for kind in [WorldKind::Ecs, WorldKind::Objects] {
        let mut session = new_session(reopened.clone(), kind);
        assert_eq!(frame(session.state()).unwrap(), PuzzleFrame::default());
        session.step(DT, &GameInput::default()).unwrap();
        let board = session.state().world.component(1, BOARD).unwrap();
        assert_eq!(board["width"], json!(5));
        assert_eq!(board["max_turns"], json!(9.0));
        assert!(
            session
                .state()
                .world
                .component(1, "future.record")
                .is_none()
        );
        assert_eq!(
            session
                .state()
                .world
                .object(100)
                .unwrap()
                .transform
                .translation,
            [-1.25, 0.5, 1.25]
        );
        assert_eq!(position(session.state(), 101), (2, 2));
        assert_eq!(session.project(), &reopened);
    }
    assert_eq!(serde_json::to_string(&authored).unwrap(), saved);
}

#[test]
fn pressed_turns_push_once_idle_blocked_and_ambiguous_packets_do_not_move() {
    for kind in [WorldKind::Ecs, WorldKind::Objects] {
        let mut session = new_session(project(), kind);
        session.step(DT, &direction_input(Direction::West)).unwrap();
        assert_eq!(frame(session.state()).unwrap().turns, 0);
        let mut ambiguous = direction_input(Direction::North);
        ambiguous.actions.insert(Direction::East.action().into());
        session.step(DT, &ambiguous).unwrap();
        assert_eq!(position(session.state(), 100), (1, 3));
        session
            .step(1. / 240., &direction_input(Direction::North))
            .unwrap();
        assert_eq!(frame(session.state()).unwrap().turns, 0);
        session.step(1. / 80., &GameInput::default()).unwrap();
        assert_eq!(position(session.state(), 100), (1, 2));
        session.step(DT, &direction_input(Direction::East)).unwrap();
        assert_eq!(position(session.state(), 101), (3, 2));
        session.step(DT, &direction_input(Direction::East)).unwrap();
        assert_eq!(position(session.state(), 101), (3, 2));
        assert_eq!(frame(session.state()).unwrap().turns, 2);
        for _ in 0..40 {
            session.step(DT, &GameInput::default()).unwrap();
        }
        assert_eq!(position(session.state(), 100), (2, 2));
        assert_eq!(frame(session.state()).unwrap().turns, 2);
        assert!(session.events().is_empty());
    }
}

#[test]
fn five_turn_solution_wins_at_the_limit_halts_and_restart_restores_both_worlds() {
    for kind in [WorldKind::Ecs, WorldKind::Objects] {
        let mut source = project();
        board_mut(&mut source).components.get_mut(BOARD).unwrap()["max_turns"] = json!(5);
        let mut session = new_session(source.clone(), kind);
        let initial = session.state().world.clone();
        let lifetime = session.state().world.lifetime(100);
        for direction in SOLUTION {
            session.step(DT, &direction_input(direction)).unwrap();
        }
        assert_eq!(
            frame(session.state()).unwrap(),
            PuzzleFrame {
                outcome: PuzzleOutcome::Solved,
                turns: 5
            }
        );
        assert_eq!(position(session.state(), 101), (3, 1));
        assert!(session.state().control.halted);
        assert_eq!(
            session
                .events()
                .iter()
                .filter(|event| event.name == "puzzle.solved")
                .count(),
            1
        );
        let tick = session.state().tick;
        session
            .step(1., &direction_input(Direction::South))
            .unwrap();
        assert_eq!(session.state().tick, tick);
        assert!(session.events().is_empty());
        session
            .step(
                0.,
                &GameInput {
                    restart: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(session.state().world, initial);
        assert_ne!(session.state().world.lifetime(100), lifetime);
        assert_eq!(frame(session.state()).unwrap(), PuzzleFrame::default());
        assert!(!session.state().control.halted);
        assert_eq!(session.state().tick, 0);
        assert_eq!(session.project(), &source);
        let mut failed_source = project();
        board_mut(&mut failed_source)
            .components
            .get_mut(BOARD)
            .unwrap()["max_turns"] = json!(2.0);
        let mut failed = new_session(failed_source, kind);
        for direction in [Direction::North, Direction::East] {
            failed.step(DT, &direction_input(direction)).unwrap();
        }
        assert_eq!(
            frame(failed.state()).unwrap(),
            PuzzleFrame {
                outcome: PuzzleOutcome::Failed,
                turns: 2
            }
        );
        assert!(
            failed
                .events()
                .iter()
                .any(|event| event.name == "puzzle.failed")
        );
        failed.restart().unwrap();
        assert_eq!(frame(failed.state()).unwrap(), PuzzleFrame::default());
        assert_eq!(position(failed.state(), 101), (2, 2));
    }
}

#[test]
fn invalid_layouts_fractional_fields_and_malformed_frames_reject_without_rewriting_source() {
    let registry = registry();
    for change in 0..5 {
        let mut source = project();
        match change {
            0 => {
                board_mut(&mut source).components.get_mut(BOARD).unwrap()["width"] = json!(4.5);
            }
            1 => {
                source
                    .scenes
                    .get_mut("board")
                    .unwrap()
                    .objects
                    .iter_mut()
                    .find(|object| object.object.id == 101)
                    .unwrap()
                    .components
                    .get_mut(CELL)
                    .unwrap()["x"] = json!(10);
            }
            2 => {
                let extra = board_mut(&mut source).clone();
                let mut extra = extra;
                extra.object.id = 500;
                source.scenes.get_mut("board").unwrap().objects.push(extra);
            }
            3 => {
                *source
                    .scenes
                    .get_mut("board")
                    .unwrap()
                    .objects
                    .iter_mut()
                    .find(|object| object.object.id == 101)
                    .unwrap()
                    .components
                    .get_mut(CELL)
                    .unwrap() = json!({"x":1,"z":3,"kind":"crate"});
            }
            _ => {
                source
                    .scenes
                    .get_mut("board")
                    .unwrap()
                    .objects
                    .retain(|object| object.object.id != 102);
            }
        }
        let before = source.clone();
        assert!(source.validate_with_registry(&registry).is_err());
        assert_eq!(source, before);
        for kind in [WorldKind::Ecs, WorldKind::Objects] {
            assert!(
                GameSession::new_with_registry(source.clone(), kind, registry.clone()).is_err()
            );
        }
    }
    let state = new_session(project(), WorldKind::Ecs).state().clone();
    for invalid in [
        json!("bad"),
        json!({"outcome":"unknown","turns":1}),
        json!({"outcome":"Playing","turns":0,"extra":1}),
    ] {
        let mut invalid_state = state.clone();
        invalid_state.module_data.insert(FRAME_KEY.into(), invalid);
        assert!(frame(&invalid_state).is_err());
    }
}

struct Reject {
    fail: Arc<AtomicBool>,
}
impl ExecutableSystem for Reject {
    fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
        if self.fail.load(Ordering::SeqCst) {
            *state = json!({"uncommitted":true});
            ctx.set_resource("turn", json!({"broken":true}))?;
            ctx.game
                .module_data
                .insert("fixture.uncommitted".into(), json!([1, 2, 3]));
            ctx.events
                .push(GameEvent::new("fixture.uncommitted", Value::Null));
            return Err("Injected publication rejection".into());
        }
        Ok(())
    }
}
fn failure_fixture(fail: Arc<AtomicBool>) -> (GameProject, SystemRegistry) {
    let mut project = project();
    let mut registry = registry();
    registry
        .register(
            SystemDescriptor {
                id: "fixture.reject".into(),
                state_schema: "fixture.reject/1".into(),
                components: vec![],
                requires: vec!["puzzle.result".into()],
                ports: vec![SystemPort {
                    name: "turn".into(),
                    data_type: "PuzzleTurn/1".into(),
                    access: ResourceAccess::ReadWrite,
                }],
            },
            move |_| Ok(Box::new(Reject { fail: fail.clone() })),
        )
        .unwrap();
    project.execution.stages[0].nodes.push(SystemNode {
        id: "reject".into(),
        module: "fixture.reject".into(),
        params: Value::Null,
        bindings: BTreeMap::from([("turn".into(), "turn".into())]),
        after: vec!["result".into()],
    });
    (project, registry)
}

#[test]
fn rejected_push_restores_world_frame_lifetimes_resources_and_retains_one_retry() {
    for kind in [WorldKind::Ecs, WorldKind::Objects] {
        let fail = Arc::new(AtomicBool::new(false));
        let (project, registry) = failure_fixture(fail.clone());
        let mut session =
            GameSession::new_with_registry(project.clone(), kind, registry.clone()).unwrap();
        session
            .step(DT, &direction_input(Direction::North))
            .unwrap();
        let before = session.state().clone();
        let states = session.system_states();
        let lifetime = before.world.lifetime(101);
        fail.store(true, Ordering::SeqCst);
        assert!(session.step(DT, &direction_input(Direction::East)).is_err());
        assert_eq!(session.state(), &before);
        assert_eq!(session.state().world.lifetime(101), lifetime);
        assert_eq!(session.system_states(), states);
        assert!(session.events().is_empty());
        fail.store(false, Ordering::SeqCst);
        session.step(0., &GameInput::default()).unwrap();
        assert_eq!(position(session.state(), 101), (3, 2));
        assert_eq!(frame(session.state()).unwrap().turns, 2);
        session.step(DT, &GameInput::default()).unwrap();
        assert_eq!(frame(session.state()).unwrap().turns, 2);

        let mut executor = SystemExecutor::new(&project.execution, &registry).unwrap();
        let mut state = GameSession::new_with_registry(project.clone(), kind, registry.clone())
            .unwrap()
            .state()
            .clone();
        let mut events = vec![];
        executor
            .step(
                &mut state,
                &direction_input(Direction::North),
                DT as f32,
                &mut events,
            )
            .unwrap();
        let before = state.clone();
        let resources = executor.resources().clone();
        let events_before = events.clone();
        fail.store(true, Ordering::SeqCst);
        assert!(
            executor
                .step(
                    &mut state,
                    &direction_input(Direction::East),
                    DT as f32,
                    &mut events
                )
                .is_err()
        );
        assert_eq!(state, before);
        assert_eq!(executor.resources(), &resources);
        assert_eq!(events, events_before);
    }
}

struct LateOverlap {
    fail: Arc<AtomicBool>,
    counter: u64,
    before: u64,
}
impl ExecutableSystem for LateOverlap {
    fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
        self.before = self.counter;
        self.counter += 1;
        *state = json!(self.counter);
        if self.fail.load(Ordering::SeqCst) {
            let pawn = ctx.game.world.component_mut(100, CELL).unwrap();
            pawn["x"] = json!(0);
            pawn["z"] = json!(2);
            ctx.set_resource("turn", json!({"uncommitted":true}))?;
            ctx.game
                .module_data
                .insert("fixture.late".into(), json!(true));
            ctx.events.push(GameEvent::new(
                "fixture.late",
                json!({"count":self.counter}),
            ));
        }
        Ok(())
    }
    fn rollback_tick(&mut self) -> Result<(), String> {
        self.counter = self.before;
        Ok(())
    }
}

fn late_overlap_fixture(fail: Arc<AtomicBool>) -> (GameProject, SystemRegistry) {
    let mut project = project();
    let mut registry = registry();
    registry
        .register(
            SystemDescriptor {
                id: "fixture.late".into(),
                state_schema: "fixture.late/1".into(),
                components: vec![],
                ports: vec![SystemPort {
                    name: "turn".into(),
                    data_type: "PuzzleTurn/1".into(),
                    access: ResourceAccess::ReadWrite,
                }],
                requires: vec!["puzzle.result".into()],
            },
            move |_| {
                Ok(Box::new(LateOverlap {
                    fail: fail.clone(),
                    counter: 0,
                    before: 0,
                }))
            },
        )
        .unwrap();
    project.execution.stages[0].nodes.push(SystemNode {
        id: "late".into(),
        module: "fixture.late".into(),
        params: Value::Null,
        bindings: BTreeMap::from([("turn".into(), "turn".into())]),
        after: vec!["result".into()],
    });
    (project, registry)
}

#[test]
fn source_mutation_after_result_cannot_publish_invalid_grid_and_retries_one_input() {
    for kind in [WorldKind::Ecs, WorldKind::Objects] {
        let fail = Arc::new(AtomicBool::new(false));
        let (project, registry) = late_overlap_fixture(fail.clone());
        let mut session =
            GameSession::new_with_registry(project.clone(), kind, registry.clone()).unwrap();
        session
            .step(DT, &direction_input(Direction::North))
            .unwrap();
        let before = session.state().clone();
        let states = session.system_states();
        let lifetime = session.state().world.lifetime(100);
        fail.store(true, Ordering::SeqCst);
        let error = session
            .step(DT, &direction_input(Direction::East))
            .unwrap_err();
        assert!(error.contains("forbidden overlap"), "{error}");
        assert_eq!(session.state(), &before);
        assert_eq!(session.state().world.lifetime(100), lifetime);
        assert_eq!(session.system_states(), states);
        assert!(session.events().is_empty());
        fail.store(false, Ordering::SeqCst);
        session.step(0., &GameInput::default()).unwrap();
        assert_eq!(position(session.state(), 101), (3, 2));
        assert_eq!(frame(session.state()).unwrap().turns, 2);
        assert_eq!(
            session.system_states()["late"],
            json!(2),
            "Private counter was not restored before retry"
        );
        session.step(DT, &GameInput::default()).unwrap();
        assert_eq!(frame(session.state()).unwrap().turns, 2);
        assert!(session.events().is_empty());

        let mut executor = SystemExecutor::new(&project.execution, &registry).unwrap();
        let mut game = GameSession::new_with_registry(project.clone(), kind, registry.clone())
            .unwrap()
            .state()
            .clone();
        let mut events = vec![];
        executor
            .step(
                &mut game,
                &direction_input(Direction::North),
                DT as f32,
                &mut events,
            )
            .unwrap();
        let before = game.clone();
        let states = executor.states();
        let resources = executor.resources().clone();
        let before_events = events.clone();
        fail.store(true, Ordering::SeqCst);
        assert!(
            executor
                .step(
                    &mut game,
                    &direction_input(Direction::East),
                    DT as f32,
                    &mut events
                )
                .unwrap_err()
                .contains("forbidden overlap")
        );
        assert_eq!(game, before);
        assert_eq!(executor.states(), states);
        assert_eq!(executor.resources(), &resources);
        assert_eq!(events, before_events);
        fail.store(false, Ordering::SeqCst);
        executor
            .step(
                &mut game,
                &direction_input(Direction::East),
                DT as f32,
                &mut events,
            )
            .unwrap();
        assert_eq!(executor.states()["late"], json!(2));
        assert_eq!(frame(&game).unwrap().turns, 2);
    }
}

struct OverflowFrame;
impl ExecutableSystem for OverflowFrame {
    fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
        *state = json!("uncommitted");
        store_frame(
            ctx.game,
            PuzzleFrame {
                outcome: PuzzleOutcome::Playing,
                turns: u64::MAX,
            },
        )
    }
}

#[test]
fn same_tick_source_counter_overflow_returns_error_and_restores_every_owned_value() {
    for kind in [WorldKind::Ecs, WorldKind::Objects] {
        let mut project = project();
        let mut registry = registry();
        registry
            .register(
                SystemDescriptor {
                    id: "fixture.overflow".into(),
                    state_schema: "fixture.overflow/1".into(),
                    components: vec![],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(OverflowFrame)),
            )
            .unwrap();
        project.execution.stages[0].nodes.insert(
            0,
            SystemNode {
                id: "overflow".into(),
                module: "fixture.overflow".into(),
                params: Value::Null,
                bindings: BTreeMap::new(),
                after: vec![],
            },
        );
        let mut executor = SystemExecutor::new(&project.execution, &registry).unwrap();
        let mut game = GameSession::new_with_registry(project, kind, registry)
            .unwrap()
            .state()
            .clone();
        let before = game.clone();
        let lifetime = game.world.lifetime(100);
        let states = executor.states();
        let resources = executor.resources().clone();
        let mut events = vec![GameEvent::new("fixture.before", json!({"keep":true}))];
        let before_events = events.clone();
        let error = executor
            .step(
                &mut game,
                &direction_input(Direction::North),
                DT as f32,
                &mut events,
            )
            .unwrap_err();
        assert!(error.contains("counter overflow"), "{error}");
        assert_eq!(game, before);
        assert_eq!(game.world.lifetime(100), lifetime);
        assert_eq!(executor.states(), states);
        assert_eq!(executor.resources(), &resources);
        assert_eq!(events, before_events);
    }
}

#[test]
fn restored_outcome_counters_are_checked_against_the_authored_limit_before_initialization() {
    let project = project();
    let registry = registry();
    for current in [
        PuzzleFrame {
            outcome: PuzzleOutcome::Playing,
            turns: 8,
        },
        PuzzleFrame {
            outcome: PuzzleOutcome::Playing,
            turns: u64::MAX,
        },
        PuzzleFrame {
            outcome: PuzzleOutcome::Solved,
            turns: 9,
        },
        PuzzleFrame {
            outcome: PuzzleOutcome::Failed,
            turns: 9,
        },
    ] {
        let mut game = new_session(project.clone(), WorldKind::Ecs).state().clone();
        store_frame(&mut game, current).unwrap();
        let before = game.clone();
        let mut executor = SystemExecutor::new(&project.execution, &registry).unwrap();
        let states = executor.states();
        let resources = executor.resources().clone();
        let mut events = vec![];
        let error = executor
            .step(&mut game, &GameInput::default(), DT as f32, &mut events)
            .unwrap_err();
        assert!(error.contains("turn count"), "{error}");
        assert_eq!(game, before);
        assert_eq!(executor.states(), states);
        assert_eq!(executor.resources(), &resources);
        assert!(events.is_empty());
    }
}

#[test]
fn initial_camera_uses_prefab_board_origin_and_resolved_grid_span() {
    let mut source = project();
    let registry = registry();
    let baseline = registry.active_game_contributions(&source.execution)[0];
    assert_eq!((baseline.initial_camera)(&source).distance, 9.5);
    source.prefabs.insert(
        "board_template".into(),
        Prefab {
            color: [58, 70, 91, 255],
            components: BTreeMap::from([(BOARD.into(), json!({}))]),
        },
    );
    let board = board_mut(&mut source);
    board.components.remove(BOARD);
    board.prefab = Some("board_template".into());
    board.object.transform.translation = [17., 0., -9.];
    board
        .overrides
        .insert("puzzle.board.cell_size".into(), json!(2.));
    source.validate_with_registry(&registry).unwrap();
    let camera = (baseline.initial_camera)(&source);
    assert_eq!(camera.target, [17., 0., -9.]);
    assert_eq!(camera.distance, 19.);
}

#[test]
fn direct_removal_of_required_crate_rejects_without_publishing_any_state() {
    for kind in [WorldKind::Ecs, WorldKind::Objects] {
        let mut session = new_session(project(), kind);
        session
            .step(DT, &direction_input(Direction::North))
            .unwrap();
        let before = session.state().clone();
        let lifetime = session.state().world.lifetime(101);
        let states = session.system_states();
        let events = session.events().to_vec();
        let snapshot = session.snapshot();
        let authored = session.project().clone();
        let error = session
            .apply_commands(&[WorldCommand::Remove { id: 101 }])
            .unwrap_err();
        assert!(error.contains("one crate"), "{error}");
        assert_eq!(session.state(), &before);
        assert_eq!(session.state().world.lifetime(101), lifetime);
        assert_eq!(session.system_states(), states);
        assert_eq!(session.events(), events);
        assert_eq!(session.snapshot(), snapshot);
        assert_eq!(session.project(), &authored);
    }
}

#[test]
fn carried_crate_overlapping_destination_pawn_rejects_before_scene_publication() {
    let mut source = project();
    let mut destination = source.scenes["board"].clone();
    destination.name = "Different valid puzzle layout".into();
    for object in &mut destination.objects {
        if let Some(cell) = object.components.get_mut(CELL) {
            match object.object.id {
                100 => {
                    cell["x"] = json!(2);
                    cell["z"] = json!(2);
                }
                101 => {
                    cell["x"] = json!(3);
                    cell["z"] = json!(2);
                }
                _ => {}
            }
        }
    }
    source.scenes.insert("copper_annex".into(), destination);
    // Each authored scene is valid by itself. Only replacing the destination
    // crate's cell with the explicitly carried source cell creates overlap.
    let registry = registry();
    source.validate_with_registry(&registry).unwrap();
    for kind in [WorldKind::Ecs, WorldKind::Objects] {
        let mut session =
            GameSession::new_with_registry(source.clone(), kind, registry.clone()).unwrap();
        session
            .step(DT, &direction_input(Direction::North))
            .unwrap();
        let before = session.state().clone();
        let lifetimes = [
            session.state().world.lifetime(100),
            session.state().world.lifetime(101),
        ];
        let states = session.system_states();
        let events = session.events().to_vec();
        let snapshot = session.snapshot();
        let error = session
            .transition_with(SceneTransition {
                destination: "copper_annex".into(),
                carry: BTreeSet::from([101]),
                spawn: None,
            })
            .unwrap_err();
        assert!(error.contains("forbidden overlap"), "{error}");
        assert_eq!(session.state(), &before);
        assert_eq!(
            [
                session.state().world.lifetime(100),
                session.state().world.lifetime(101)
            ],
            lifetimes
        );
        assert_eq!(session.system_states(), states);
        assert_eq!(session.events(), events);
        assert_eq!(session.snapshot(), snapshot);
        assert_eq!(session.project(), &source);
        assert_eq!(session.state().scene, "board");
    }
}
