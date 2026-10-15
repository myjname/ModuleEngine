use super::*;
use crate::ArpgEvent as GameEvent;
use std::sync::Arc;

fn arpg_events(session: &GameSession) -> Vec<GameEvent> {
    session
        .events()
        .iter()
        .filter_map(|e| ArpgEvent::try_from(e).ok())
        .collect()
}
fn health(entity: &GameEntity) -> f32 {
    entity
        .components
        .get("health")
        .and_then(|h| h.get("current"))
        .and_then(Value::as_f64)
        .unwrap_or(1.) as f32
}
fn assert_parity(a: &GameState, b: &GameState) {
    assert_eq!(
        (&a.scene, a.time, a.tick, &a.world, &a.control),
        (&b.scene, b.time, b.tick, &b.world, &b.control)
    );
    assert_eq!(a.arpg(), b.arpg());
}
struct RejectHealthPublication(Arc<std::sync::atomic::AtomicBool>);
impl ExecutableSystem for RejectHealthPublication {
    fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        Ok(())
    }
    fn commit_tick(&mut self) -> Result<(), String> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            Err("Reject health publication".into())
        } else {
            Ok(())
        }
    }
}
fn project_without_enemies() -> GameProject {
    let mut p = GameProject::demo();
    let scene = p.scenes.get_mut("arena").unwrap();
    scene.objects.retain(|e| e.prefab.is_none());
    p
}
fn test_object(id: u64, parent: Option<u64>) -> crate::AuthoredObject {
    crate::AuthoredObject::new(WorldObject {
        id,
        parent,
        transform: crate::Transform::default(),
        color: [80, 120, 160, 255],
    })
}
fn focused_project(objects: Vec<crate::AuthoredObject>, modules: &[&str]) -> GameProject {
    let mut project = GameProject::demo();
    project.entry_scene = "source".into();
    project.scenes = BTreeMap::from([(
        "source".into(),
        crate::AuthoredScene {
            name: "Source".into(),
            objects,
        },
    )]);
    project.prefabs.clear();
    project.audio_cues.clear();
    project.execution.stages.retain_mut(|stage| {
        stage.nodes.retain_mut(|node| {
            node.after.clear();
            modules.contains(&node.module.as_str())
        });
        !stage.nodes.is_empty()
    });
    project
}
fn dead_object(id: u64, parent: Option<u64>) -> crate::AuthoredObject {
    let mut object = test_object(id, parent);
    object
        .components
        .insert("health".into(), json!({"maximum":10.,"current":0.}));
    object
}
fn append_test_system(project: &mut GameProject, id: &str) {
    project.execution.stages.push(crate::SystemStage {
        name: id.into(),
        nodes: vec![crate::SystemNode {
            id: id.into(),
            module: id.into(),
            params: Value::Null,
            bindings: BTreeMap::new(),
            after: vec![],
        }],
    });
}
fn test_system_descriptor(id: &str) -> SystemDescriptor {
    SystemDescriptor {
        id: id.into(),
        state_schema: "1".into(),
        components: vec![],
        ports: vec![],
        requires: vec![],
    }
}
fn persistent_hierarchy_project() -> GameProject {
    let mut carried = test_object(2, Some(1));
    carried.components = BTreeMap::from([
        ("persistent".into(), json!({})),
        ("health".into(), json!({"maximum":10.,"current":6.})),
    ]);
    carried.references.insert("home".into(), 1);
    let mut absent = carried.clone();
    absent.object.id = 4;
    let mut project = focused_project(
        vec![test_object(1, None), carried, absent],
        &["game.health", "game.portals"],
    );
    let mut parent = test_object(3, None);
    parent.object.transform.translation = [20., 0., 0.];
    let mut destination = test_object(2, Some(3));
    destination.object.transform.translation = [4., 0., 1.];
    destination
        .components
        .insert("health".into(), json!({"maximum":10.,"current":10.}));
    destination.references.insert("home".into(), 3);
    project.scenes.insert(
        "destination".into(),
        crate::AuthoredScene {
            name: "Destination".into(),
            objects: vec![parent, destination],
        },
    );
    project
}
#[test]
fn health_subtree_removal_captures_dead_descendant_metadata() {
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        for player in [false, true] {
            let mut child = dead_object(20, Some(10));
            child.components.insert(
                if player { "player" } else { "boss" }.into(),
                if player {
                    json!({})
                } else {
                    json!({"victory_on_death":true})
                },
            );
            let project = focused_project(
                vec![
                    dead_object(10, None),
                    child,
                    test_object(30, Some(20)),
                    dead_object(40, Some(30)),
                ],
                &["game.health"],
            );
            let mut session = crate::session(project, kind).unwrap();
            session.step(1. / 60., &GameInput::default()).unwrap();
            let deaths: Vec<_> = arpg_events(&session)
                .iter()
                .filter_map(|event| {
                    if let GameEvent::Death { object } = event {
                        Some(*object)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(deaths, vec![10, 20, 40]);
            assert_eq!(
                session.state().arpg().status,
                if player {
                    GameStatus::Defeat
                } else {
                    GameStatus::Victory
                }
            );
            assert_eq!(arpg_events(&session).contains(&GameEvent::Victory), !player);
            assert!(session.state().world.is_empty());
            assert!(session.world_objects().is_empty());
            assert!(session.snapshot().items.is_empty());
        }
    }
}
#[test]
fn rejected_health_publication_restores_subtrees_events_and_death_state() {
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        let reject = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let control = reject.clone();
        let mut registry = crate::registry();
        registry
            .register(test_system_descriptor("test.reject-health"), move |_| {
                Ok(Box::new(RejectHealthPublication(control.clone())))
            })
            .unwrap();
        let mut project = focused_project(
            vec![dead_object(10, None), dead_object(20, Some(10))],
            &["game.health"],
        );
        append_test_system(&mut project, "test.reject-health");
        let mut session = GameSession::new_with_registry(project, kind, registry).unwrap();
        let state = session.state().clone();
        let snapshot = session.snapshot();
        let system_states = session.system_states();
        assert!(
            session
                .step(1. / 60., &GameInput::default())
                .unwrap_err()
                .contains("Reject health publication")
        );
        assert_eq!(session.state(), &state);
        assert_eq!(session.snapshot(), snapshot);
        assert_eq!(session.system_states(), system_states);
        assert!(arpg_events(&session).is_empty());
        reject.store(false, std::sync::atomic::Ordering::SeqCst);
        session.step_budget(0., &GameInput::default(), 1).unwrap();
        assert_eq!(
            arpg_events(&session),
            &[
                GameEvent::Death { object: 10 },
                GameEvent::Death { object: 20 }
            ]
        );
        assert!(session.state().world.is_empty());
        assert!(session.world_objects().is_empty());
    }
}
#[test]
fn persistent_entities_use_destination_hierarchy_and_detach_when_absent() {
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        let project = persistent_hierarchy_project();
        let authored = project.clone();
        let destination = project
            .resolve_scene("destination")
            .unwrap()
            .into_iter()
            .find(|e| e.object.id == 2)
            .unwrap();
        let mut session = crate::session(project, kind).unwrap();
        session
            .transition_with(SceneTransition {
                destination: "destination".into(),
                carry: session
                    .state()
                    .world
                    .entities_with(&["persistent"])
                    .into_iter()
                    .collect(),
                spawn: None,
            })
            .unwrap();
        let carried = &session.state().world.entity(2).unwrap();
        assert_eq!(carried.object, destination.object);
        assert_eq!(carried.references, destination.references);
        assert_eq!(health(carried), 6.);
        assert!(carried.components.contains_key("persistent"));
        let absent = &session.state().world.entity(4).unwrap();
        assert_eq!(absent.object.parent, None);
        assert!(absent.references.is_empty());
        assert_eq!(
            session.world_objects(),
            session.state().world.objects_snapshot()
        );
        assert_eq!(
            session
                .snapshot()
                .items
                .iter()
                .find(|item| item.id == 2)
                .unwrap()
                .model[0][3],
            24.
        );
        assert_eq!(session.project(), &authored);
    }
}
#[test]
fn fixed_tick_preserves_short_frame_attack_and_world_parity() {
    let p = GameProject::demo();
    let mut a = crate::session(p.clone(), WorldKind::Objects).unwrap();
    let mut b = crate::session(p, WorldKind::Ecs).unwrap();
    let attack: GameInput = (ArpgInput {
        attack: true,
        aim_world: [0., 0., -4.],
        ..ArpgInput::default()
    })
    .into();
    assert_eq!(a.step(0.001, &attack).unwrap(), 0);
    b.step(0.001, &attack).unwrap();
    let input: GameInput = (ArpgInput {
        movement: [1., 0.],
        aim_world: [0., 0., -4.],
        ..ArpgInput::default()
    })
    .into();
    for _ in 0..30 {
        a.step(1. / 60., &input).unwrap();
        b.step(1. / 60., &input).unwrap();
    }
    assert_eq!(a.world_objects(), b.world_objects());
    assert_eq!(a.snapshot(), b.snapshot());
    assert_eq!(a.state().tick, 30);
}
#[test]
fn budgeted_fixed_ticks_retain_catchup_time_and_match_synchronous_state() {
    let project = GameProject::demo();
    let mut synchronous = crate::session(project.clone(), WorldKind::Objects).unwrap();
    let mut yielding = crate::session(project, WorldKind::Objects).unwrap();
    let pressed: GameInput = (ArpgInput {
        attack: true,
        movement: [0.5, 0.],
        aim_world: [0., 0., -4.],
        ..ArpgInput::default()
    })
    .into();
    assert_eq!(synchronous.step(0.25, &pressed).unwrap(), 15);
    assert_eq!(yielding.step_budget(0.25, &pressed, 1).unwrap(), 1);
    assert!((yielding.pending_time() - 14. / 60.).abs() < 1e-10);
    let mut held = pressed.clone();
    held.actions.remove("attack");
    for _ in 0..14 {
        assert_eq!(yielding.step_budget(0., &held, 1).unwrap(), 1);
    }
    assert!(yielding.pending_time().abs() < 1e-10);
    assert_parity(yielding.state(), synchronous.state());
    assert_eq!(yielding.snapshot(), synchronous.snapshot());
}
#[test]
fn collision_blocks_walls_and_restart_restores_authored_scene() {
    let p = project_without_enemies();
    let authored = p.clone();
    let mut s = crate::session(p, WorldKind::Objects).unwrap();
    let input: GameInput = (ArpgInput {
        movement: [1., 0.],
        ..ArpgInput::default()
    })
    .into();
    for _ in 0..120 {
        s.step(1. / 60., &input).unwrap();
    }
    assert!(s.state().player().unwrap().object.transform.translation[0] <= 7.46);
    assert_eq!(s.project(), &authored);
    s.restart().unwrap();
    assert_eq!(
        s.state().player().unwrap().object.transform.translation,
        [0., 0., 5.]
    );
}
#[test]
fn one_swing_hits_every_target_once() {
    let mut p = GameProject::demo();
    p.scenes
        .get_mut("arena")
        .unwrap()
        .objects
        .retain(|e| e.object.id == 1 || [10, 11].contains(&e.object.id));
    for e in &mut p.scenes.get_mut("arena").unwrap().objects {
        if e.object.id != 1 {
            e.object.transform.translation = [if e.object.id == 10 { -0.2 } else { 0.2 }, 0., 3.5];
            e.components
                .insert("ai".into(), json!({"kind":"chase","aggro_radius":0.0}));
        }
    }
    let mut s = crate::session(p, WorldKind::Objects).unwrap();
    let input: GameInput = (ArpgInput {
        attack: true,
        aim_world: [0., 0., 3.],
        ..ArpgInput::default()
    })
    .into();
    s.step(1. / 60., &input).unwrap();
    assert_eq!(
        arpg_events(&s)
            .iter()
            .filter(|e| matches!(e, GameEvent::Hit { attacker: 1, .. }))
            .count(),
        2
    );
    for _ in 0..5 {
        s.step(1. / 60., &GameInput::default()).unwrap();
        assert!(
            !arpg_events(&s)
                .iter()
                .any(|e| matches!(e, GameEvent::Hit { attacker: 1, .. }))
        );
    }
    assert_eq!(health(&s.state().world.entity(10).unwrap()), 8.);
    assert_eq!(health(&s.state().world.entity(11).unwrap()), 8.);
}
#[test]
fn melee_area_matches_radius_extension_and_center_angle() {
    let area = MeleeArea {
        center: [0., 0., 0.],
        direction: [0., -1.],
        range: 2.,
        arc: std::f32::consts::FRAC_PI_2,
    };
    // A circle can intersect the radial edge, but its center must remain
    // inside the angle. An angular edge is not expanded by target radius.
    assert!(area.contains([0., 100., -2.4], 0.4));
    assert!(!area.contains([0., 0., -2.41], 0.4));
    assert!(area.contains([1., 0., -1.01], 0.2));
    assert!(!area.contains([1., 0., -0.99], 100.));
    assert!(!area.contains([0., 0., 1.], 0.2));
    let circle = MeleeArea {
        arc: std::f32::consts::TAU,
        ..area
    };
    assert!(circle.contains([0., 0., 1.], 0.2));
}
#[test]
fn melee_preview_excludes_actors_without_damageable_health() {
    let mut project = GameProject::demo();
    let scene = project.scenes.get_mut("arena").unwrap();
    scene.objects.retain(|e| [1, 10, 11].contains(&e.object.id));
    for entity in &mut scene.objects {
        if entity.object.id != 1 {
            entity.object.transform.translation =
                [if entity.object.id == 10 { -0.2 } else { 0.2 }, 0., 3.5];
            entity
                .components
                .insert("ai".into(), json!({"kind":"chase","aggro_radius":0.0}));
        }
    }
    // Resolve prefab instances so this fixture can omit only one actor's health.
    let entities = project.resolve_scene("arena").unwrap();
    project.scenes.get_mut("arena").unwrap().objects = entities
        .into_iter()
        .map(|entity| {
            let mut object = AuthoredObject::new(entity.object);
            object.components = entity.components;
            object.references = entity.references;
            if object.object.id == 10 {
                object.components.remove("health");
            }
            object
        })
        .collect();
    let mut session = crate::session(project, WorldKind::Objects).unwrap();
    let aim = [0., 0., 3.];
    let area = session.state().melee_aim(1, aim).unwrap();
    assert!(
        area.contains(
            session
                .state()
                .world
                .entity(10)
                .unwrap()
                .object
                .transform
                .translation,
            0.36
        )
    );
    assert_eq!(session.state().melee_targets(1, &area), vec![11]);
    session
        .step(
            1. / 60.,
            &(ArpgInput {
                attack: true,
                aim_world: aim,
                ..ArpgInput::default()
            })
            .into(),
        )
        .unwrap();
    let targets = arpg_events(&session)
        .iter()
        .filter_map(|event| match event {
            GameEvent::Hit {
                attacker: 1,
                target,
                ..
            } => Some(*target),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(targets, vec![11]);
    assert!(session.state().world.contains(10));
    assert!(
        !session
            .state()
            .arpg()
            .hit_visuals
            .iter()
            .any(|hit| hit.target == 10)
    );
}
#[test]
fn combat_visuals_retain_real_hits_and_expire_with_provider_parity() {
    let mut project = GameProject::demo();
    let arena = project.scenes.get_mut("arena").unwrap();
    arena
        .objects
        .retain(|e| [1, 10, 11, 12].contains(&e.object.id));
    for entity in &mut arena.objects {
        if entity.object.id == 1 {
            continue;
        }
        entity.object.transform.translation = match entity.object.id {
            10 => [-0.2, 0., 3.5],
            11 => [0.2, 0., 3.5],
            _ => [0., 0., 6.5],
        };
        entity
            .components
            .insert("ai".into(), json!({"kind":"chase","aggro_radius":0.0}));
        if entity.object.id == 10 {
            entity
                .components
                .insert("health".into(), json!({"maximum":38.,"current":1.}));
        }
    }
    let input: GameInput = (ArpgInput {
        attack: true,
        aim_world: [0., 0., 3.],
        ..ArpgInput::default()
    })
    .into();
    let mut objects = crate::session(project.clone(), WorldKind::Objects).unwrap();
    let mut ecs = crate::session(project, WorldKind::Ecs).unwrap();
    let preview = objects.state().melee_aim(1, input.aim_world()).unwrap();
    assert_eq!(objects.state().melee_targets(1, &preview), vec![10, 11]);
    for session in [&mut objects, &mut ecs] {
        session.step(1. / 60., &input).unwrap();
        let frame = session.state().arpg();
        let attack = frame
            .attack_visuals
            .iter()
            .find(|v| v.attacker == 1)
            .unwrap();
        assert_eq!(attack.area, preview);
        assert_eq!(attack.progress, 0.);
        assert!(!session.state().world.contains(10));
        let lethal_hit = frame.hit_visuals.iter().find(|h| h.target == 10).unwrap();
        assert_eq!(lethal_hit.position, [-0.2, 0., 3.5]);
        assert_eq!(lethal_hit.damage, 28.);
        assert_eq!(session.state().arpg().hit_visuals.len(), 2);
        // A renderer frame shorter than a tick must not discard feedback.
        let retained = session.state().arpg().hit_visuals.clone();
        session.step(0.001, &GameInput::default()).unwrap();
        assert_eq!(session.state().arpg().hit_visuals, retained);
    }
    assert_parity(objects.state(), ecs.state());
    for tick in 0..30 {
        for session in [&mut objects, &mut ecs] {
            session
                .step(
                    1. / 60.,
                    &(ArpgInput {
                        aim_world: [0., 0., 8.],
                        ..ArpgInput::default()
                    })
                    .into(),
                )
                .unwrap();
            // The cursor can change, but the existing swing remains aimed
            // in its original direction, and cannot hit a target twice.
            for attack in session
                .state()
                .arpg()
                .attack_visuals
                .iter()
                .filter(|v| v.attacker == 1)
            {
                assert_eq!(attack.area.direction, preview.direction);
            }
            assert!(
                !arpg_events(session)
                    .iter()
                    .any(|e| matches!(e, GameEvent::Hit { attacker: 1, .. }))
            );
            if tick < 4 {
                assert!(!session.state().arpg().attack_visuals.is_empty());
                assert_eq!(session.state().arpg().hit_visuals.len(), 2);
            }
            if tick == 10 {
                // Swing ends before hit feedback: these are independent
                // durations rather than a single frame's event list.
                assert!(session.state().arpg().attack_visuals.is_empty());
                assert_eq!(session.state().arpg().hit_visuals.len(), 2);
            }
        }
        assert_parity(objects.state(), ecs.state());
        assert_eq!(objects.snapshot(), ecs.snapshot());
    }
    assert!(objects.state().arpg().attack_visuals.is_empty());
    assert!(objects.state().arpg().hit_visuals.is_empty());
    objects.restart().unwrap();
    objects.step(1. / 60., &input).unwrap();
    assert!(!objects.state().arpg().attack_visuals.is_empty());
    assert!(!objects.state().arpg().hit_visuals.is_empty());
    objects.transition("boss").unwrap();
    assert!(objects.state().arpg().attack_visuals.is_empty());
    assert!(objects.state().arpg().hit_visuals.is_empty());
    objects.restart().unwrap();
    assert!(objects.state().arpg().attack_visuals.is_empty());
    assert!(objects.state().arpg().hit_visuals.is_empty());
    assert!(objects.state().world.contains(10));
}
#[test]
fn portal_transfers_health_and_boss_warns_before_damage() {
    let mut p = project_without_enemies();
    p.scenes
        .get_mut("arena")
        .unwrap()
        .objects
        .iter_mut()
        .find(|o| o.object.id == 1)
        .unwrap()
        .components
        .get_mut("health")
        .unwrap()["current"] = json!(60.);
    let mut s = crate::session(p, WorldKind::Ecs).unwrap();
    s.apply_commands(&[WorldCommand::SetPosition {
        id: 1,
        position: [0., 0., -7.2],
    }])
    .unwrap();
    s.step(1. / 60., &GameInput::default()).unwrap();
    assert_eq!(s.state().scene, "boss");
    assert_eq!(health(&s.state().player().unwrap()), 60.);
    assert!(
        arpg_events(&s)
            .iter()
            .any(|e| matches!(e, GameEvent::SceneChanged { .. }))
    );
    s.apply_commands(&[WorldCommand::SetPosition {
        id: 1,
        position: [0., 0., -1.],
    }])
    .unwrap();
    s.step(1. / 60., &GameInput::default()).unwrap();
    assert!(!s.state().arpg().telegraphs.is_empty());
    assert_eq!(health(&s.state().player().unwrap()), 60.);
    s.apply_commands(&[WorldCommand::SetPosition {
        id: 1,
        position: [7., 0., 5.],
    }])
    .unwrap();
    for _ in 0..70 {
        s.step(1. / 60., &GameInput::default()).unwrap();
    }
    assert_eq!(health(&s.state().player().unwrap()), 60.);
}
