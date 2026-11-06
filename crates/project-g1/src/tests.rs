use super::*;
use crate::components::Catalog;
use module_engine_core::*;
use module_engine_game_kit::actions::{ActionState, PhysicalEvent};
use serde_json::json;

fn session_from(project: &GameProject, kind: WorldKind) -> GameSession {
    let registry = registry();
    let prepared = project.prepare_runtime(&registry).unwrap();
    GameSession::new_with_registry(prepared, kind, registry).unwrap()
}
fn session(kind: WorldKind) -> GameSession {
    session_from(&project(), kind)
}
fn tick(session: &mut GameSession, input: &GameInput) {
    assert_eq!(session.step(1. / 60., input).unwrap(), 1);
}
fn action(session: &mut GameSession, name: &str) {
    tick(session, &action_input(name));
}
fn walk(session: &mut GameSession, path: &str) {
    for token in path.chars() {
        tick(
            session,
            &direction_input(match token {
                'N' => Direction::North,
                'E' => Direction::East,
                'S' => Direction::South,
                'W' => Direction::West,
                _ => panic!("Invalid source solution token"),
            }),
        );
    }
}
fn initialize(session: &mut GameSession) {
    tick(session, &GameInput::default());
}
fn volumes_input(music: f64, sfx: f64) -> GameInput {
    let mut input = action_input("g1.set_volumes");
    input.values.insert("g1.music_volume".into(), json!(music));
    input.values.insert("g1.sfx_volume".into(), json!(sfx));
    input.action_values = input.values.clone();
    input
}

fn sound_events(session: &GameSession) -> Vec<String> {
    session
        .events()
        .iter()
        .filter(|event| event.name == "g1.audio")
        .filter_map(|event| {
            event
                .payload
                .get("cue")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}
fn solved_entry_source() -> GameProject {
    let mut source = project();
    for id in ["level01", "level02"] {
        let mut draft = GridDraft::from_project(&source, id).unwrap();
        draft.goals = draft.crates.clone();
        source = draft.into_project(&source).unwrap();
    }
    source
}

#[test]
fn initially_solved_levels_settle_zero_move_records_unlocks_and_all_entry_routes() {
    let source = solved_entry_source();
    let author = source.clone();
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        let mut session = session_from(&source, kind);
        initialize(&mut session);
        action(&mut session, "g1.play");
        assert_eq!(view(session.state()).unwrap().phase, Phase::Victory);
        assert_eq!(view(session.state()).unwrap().moves, 0);
        assert!(!view(session.state()).unwrap().can_undo);
        assert_eq!(
            profile(session.state()).unwrap().campaign.best("level01"),
            Some(0)
        );
        assert!(
            profile(session.state())
                .unwrap()
                .campaign
                .is_unlocked("level02")
        );
        assert_eq!(
            sound_events(&session)
                .iter()
                .filter(|cue| cue.as_str() == "win")
                .count(),
            1
        );
        tick(&mut session, &GameInput::default());
        assert!(sound_events(&session).is_empty());
        tick(&mut session, &volumes_input(0.3, 0.4));
        assert!(!sound_events(&session).iter().any(|cue| cue == "win"));
        action(&mut session, "g1.restart");
        assert_eq!(view(session.state()).unwrap().phase, Phase::Victory);
        assert_eq!(view(session.state()).unwrap().moves, 0);
        assert_eq!(
            profile(session.state()).unwrap().campaign.unlocked_count(),
            2
        );
        action(&mut session, "g1.levels");
        let mut select = action_input("g1.select_level");
        select
            .action_values
            .insert("g1.level_id".into(), json!("level01"));
        tick(&mut session, &select);
        assert_eq!(view(session.state()).unwrap().phase, Phase::Victory);
        action(&mut session, "g1.next");
        assert_eq!(view(session.state()).unwrap().phase, Phase::Victory);
        assert_eq!(
            profile(session.state()).unwrap().campaign.best("level02"),
            Some(0)
        );
        assert_eq!(
            profile(session.state()).unwrap().campaign.unlocked_count(),
            3
        );
        action(&mut session, "g1.next");
        assert_eq!(view(session.state()).unwrap().phase, Phase::Playing);
        assert_eq!(view(session.state()).unwrap().current_level, 2);
        assert!(!session.state().control.halted);
    }
    assert_eq!(source, author);
}

#[test]
fn continue_settles_a_saved_solved_attempt_without_an_existing_completion_record() {
    let source = solved_entry_source();
    let catalog = Catalog::from_project(&source).unwrap();
    let mut first = session_from(&source, WorldKind::Ecs);
    initialize(&mut first);
    let mut saved = profile(first.state()).unwrap();
    saved.campaign.set_attempt(Attempt {
        level_revision: catalog.levels[0].revision.clone(),
        history: UndoJournal::new(catalog.levels[0].initial.clone()),
    });
    assert_eq!(saved.campaign.best("level01"), None);
    for action_name in ["g1.continue", "g1.resume"] {
        let mut resumed = session_from(&source, WorldKind::Ecs);
        resumed
            .seed_module_data(
                [(SAVED_KEY.into(), serde_json::to_value(&saved).unwrap())]
                    .into_iter()
                    .collect(),
            )
            .unwrap();
        initialize(&mut resumed);
        action(&mut resumed, action_name);
        assert_eq!(view(resumed.state()).unwrap().phase, Phase::Victory);
        assert_eq!(
            profile(resumed.state()).unwrap().campaign.best("level01"),
            Some(0)
        );
        assert!(
            profile(resumed.state())
                .unwrap()
                .campaign
                .is_unlocked("level02")
        );
        assert!(resumed.events().iter().any(|event| event.name == "g1.save"));
        assert!(!view(resumed.state()).unwrap().can_undo);
        action(&mut resumed, "g1.next");
        assert_eq!(view(resumed.state()).unwrap().current_level, 1);
    }
}

#[test]
fn partial_goal_has_distinct_sound_and_instance_feedback_undo_clears_it_and_final_win_has_priority()
{
    let source = project();
    let author = source.clone();
    let mut session = session_from(&source, WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    walk(&mut session, "N");
    action(&mut session, "g1.next");
    walk(&mut session, "WN");
    assert_eq!(view(session.state()).unwrap().phase, Phase::Playing);
    let before = session.state().clone();
    let level = crate::components::world_level(&before.world).unwrap();
    let occupied = *level
        .initial
        .crates
        .iter()
        .find(|(_, cell)| level.goals.contains(cell))
        .unwrap()
        .0;
    let other = *level
        .initial
        .crates
        .iter()
        .find(|(_, cell)| !level.goals.contains(cell))
        .unwrap()
        .0;
    let registry = registry();
    let decorate = registry
        .game_contributions()
        .find(|item| item.id == "g1.presentation")
        .unwrap()
        .decorate_snapshot;
    let mut snapshot = session.snapshot();
    let texture = std::sync::Arc::new(TextureImage {
        width: 1,
        height: 1,
        rgba8: vec![180, 120, 60, 255],
    });
    for item in &mut snapshot.items {
        if item.id == occupied || item.id == other {
            item.material.base_color_texture = Some(texture.clone());
        }
    }
    let original_other = snapshot
        .items
        .iter()
        .find(|item| item.id == other)
        .unwrap()
        .clone();
    snapshot.items.push(
        snapshot
            .items
            .iter()
            .find(|item| item.id == occupied)
            .unwrap()
            .clone(),
    );
    decorate(&mut snapshot, &before);
    assert!(
        snapshot
            .items
            .iter()
            .filter(|item| item.id == occupied)
            .all(|item| item.color[1] > item.color[0] && item.color[1] > item.color[2])
    );
    assert_eq!(
        snapshot.items.iter().find(|item| item.id == other),
        Some(&original_other)
    );
    assert!(
        snapshot
            .items
            .iter()
            .filter(|item| item.id == occupied || item.id == other)
            .all(|item| std::sync::Arc::ptr_eq(
                item.material.base_color_texture.as_ref().unwrap(),
                &texture
            ))
    );
    assert_eq!(session.state(), &before);
    assert_eq!(source, author);
    assert_eq!(sound_events(&session), vec!["goal"]);
    action(&mut session, "g1.undo");
    let mut undone = session.snapshot();
    decorate(&mut undone, session.state());
    assert!(
        undone
            .items
            .iter()
            .filter(|item| item.id == occupied || item.id == other)
            .all(|item| item.color == original_other.color)
    );
    walk(&mut session, "NEESEN");
    assert_eq!(view(session.state()).unwrap().phase, Phase::Victory);
    assert_eq!(sound_events(&session), vec!["win"]);
    assert_eq!(
        profile(session.state()).unwrap().campaign.best("level02"),
        Some(7)
    );
    let contribution = registry
        .game_contributions()
        .find(|item| item.id == "g1.presentation")
        .unwrap();
    let requests = (contribution.service_requests)(&source).unwrap();
    let audio = requests
        .iter()
        .find(|request| request.id == "audio.wav")
        .unwrap();
    assert!(audio.config["cues"].get("goal").is_some());
    assert!(audio.asset_references.contains("assets/audio/goal.wav"));
}

#[test]
fn partial_goal_placement_emits_its_own_sound_instead_of_ordinary_push() {
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    walk(&mut session, "N");
    action(&mut session, "g1.next");
    walk(&mut session, "WN");
    assert_eq!(view(session.state()).unwrap().phase, Phase::Playing);
    assert_eq!(sound_events(&session), vec!["goal"]);
}

#[test]
fn ten_original_levels_are_solved_by_real_registered_systems_in_both_worlds() {
    let authored = project();
    let before = authored.clone();
    let mut final_states = Vec::new();
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        let mut session = session_from(&authored, kind);
        initialize(&mut session);
        assert_eq!(view(session.state()).unwrap().phase, Phase::Menu);
        action(&mut session, "g1.play");
        for (index, level) in level_catalog().iter().enumerate() {
            let before_board = view(session.state()).unwrap().board;
            assert_eq!(
                profile(session.state()).unwrap().campaign.current(),
                level.id
            );
            assert_eq!(before_board.crates.len(), before_board.goals.len());
            assert!(!session.asset_bindings().is_empty());
            walk(&mut session, level.solution);
            let view = view(session.state()).unwrap();
            let profile = profile(session.state()).unwrap();
            assert_eq!(
                view.moves,
                level.solution.len() as u64,
                "{kind:?}: {}",
                level.id
            );
            assert_eq!(profile.campaign.best(level.id), Some(view.moves));
            assert_eq!(profile.campaign.unlocked_count(), (index + 2).min(10));
            assert!(
                view.board
                    .crates
                    .iter()
                    .all(|cell| view.board.goals.contains(cell))
            );
            assert_eq!(
                view.phase,
                if index == 9 {
                    Phase::Complete
                } else {
                    Phase::Victory
                }
            );
            assert!(!session.state().control.halted);
            assert!(session.state().control.transition.is_none());
            if index < 9 {
                action(&mut session, "g1.next");
                assert!(
                    session
                        .events()
                        .iter()
                        .any(|event| event.name == PRESENTATION_RESET_EVENT)
                );
                assert_eq!(
                    view.current_level + 1,
                    super::view(session.state()).unwrap().current_level
                );
            }
        }
        final_states.push((
            view(session.state()).unwrap(),
            session.snapshot(),
            session.asset_bindings(),
        ));
    }
    assert_eq!(final_states[0], final_states[1]);
    assert_eq!(authored, before, "Play must not mutate authored content");
}

#[test]
fn blocked_moves_do_not_count_or_save_and_undo_after_win_preserves_best_and_unlock() {
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        let mut session = session(kind);
        initialize(&mut session);
        action(&mut session, "g1.play");
        let before = profile(session.state()).unwrap();
        tick(&mut session, &direction_input(Direction::South));
        assert_eq!(profile(session.state()).unwrap(), before);
        assert!(!session.events().iter().any(|event| event.name == "g1.save"));
        tick(&mut session, &direction_input(Direction::North));
        assert_eq!(view(session.state()).unwrap().phase, Phase::Victory);
        action(&mut session, "g1.undo");
        let current = profile(session.state()).unwrap();
        assert_eq!(
            current.campaign.attempt().unwrap().history.current().moves,
            0
        );
        assert_eq!(current.campaign.best("level01"), Some(1));
        assert!(current.campaign.is_unlocked("level02"));
        assert_eq!(view(session.state()).unwrap().phase, Phase::Playing);
        assert!(!view(session.state()).unwrap().can_undo);
        action(&mut session, "g1.restart");
        assert_eq!(
            profile(session.state()).unwrap().campaign.best("level01"),
            Some(1)
        );
    }
}

#[test]
fn canonical_rules_reject_chain_push_and_do_not_pull_a_crate() {
    let source = project();
    let mut draft = GridDraft::from_project(&source, "level01").unwrap();
    draft.pawn = Cell::new(2, 2);
    draft.crates = [Cell::new(3, 2), Cell::new(4, 2)].into_iter().collect();
    draft.goals = [Cell::new(4, 1), Cell::new(5, 1)].into_iter().collect();
    let source = draft.into_project(&source).unwrap();
    let mut session = session_from(&source, WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    let before = view(session.state()).unwrap().board;
    tick(&mut session, &direction_input(Direction::East));
    assert_eq!(view(session.state()).unwrap().moves, 0);
    assert_eq!(view(session.state()).unwrap().board, before);
    tick(&mut session, &direction_input(Direction::West));
    let moved = view(session.state()).unwrap();
    assert_eq!(moved.moves, 1);
    assert_eq!(moved.board.pawn, Cell::new(1, 2));
    assert_eq!(moved.board.crates, before.crates);
}

#[test]
fn unlimited_journal_accepts_more_than_legacy_turn_limit_and_restores_every_move() {
    let source = project();
    let catalog = Catalog::from_project(&source).unwrap();
    let level = &catalog.levels[0];
    let mut journal = UndoJournal::new(level.initial.clone());
    for index in 0..2048 {
        let direction = if index % 2 == 0 {
            Direction::East
        } else {
            Direction::West
        };
        let (next, _) = crate::rules::move_position(level, journal.current(), direction)
            .unwrap()
            .unwrap();
        assert!(journal.commit(next));
    }
    let mut journal: UndoJournal<Position> =
        serde_json::from_value(serde_json::to_value(journal).unwrap()).unwrap();
    assert_eq!(journal.current().moves, 2048);
    for moves in (0..2048).rev() {
        assert_eq!(journal.undo().unwrap().moves, moves);
    }
    assert_eq!(journal.current(), &level.initial);
}

#[test]
fn resume_restores_selected_level_settings_current_attempt_and_full_undo() {
    let mut original = session(WorldKind::Objects);
    initialize(&mut original);
    action(&mut original, "g1.play");
    walk(&mut original, level_solution("level01").unwrap());
    action(&mut original, "g1.next");
    walk(&mut original, level_solution("level02").unwrap());
    action(&mut original, "g1.next");
    walk(&mut original, &level_solution("level03").unwrap()[..12]);
    tick(&mut original, &volumes_input(0.25, 0.6));
    let saved = profile(original.state()).unwrap();
    let board = view(original.state()).unwrap().board;
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        let mut restored = session(kind);
        restored
            .seed_module_data(
                [(SAVED_KEY.into(), serde_json::to_value(&saved).unwrap())]
                    .into_iter()
                    .collect(),
            )
            .unwrap();
        initialize(&mut restored);
        assert_eq!(view(restored.state()).unwrap().phase, Phase::Menu);
        assert!(view(restored.state()).unwrap().can_continue);
        assert_eq!(profile(restored.state()).unwrap(), saved);
        assert_eq!(view(restored.state()).unwrap().board, board);
        action(&mut restored, "g1.continue");
        assert_eq!(view(restored.state()).unwrap().phase, Phase::Playing);
        assert_eq!(view(restored.state()).unwrap().moves, 12);
        action(&mut restored, "g1.undo");
        assert_eq!(view(restored.state()).unwrap().moves, 11);
        assert_eq!(
            profile(restored.state()).unwrap().campaign.best("level01"),
            Some(1)
        );
        assert!(
            profile(restored.state())
                .unwrap()
                .campaign
                .is_unlocked("level03")
        );
    }
}

#[test]
fn malformed_typed_profile_preserves_bootstrap_and_never_auto_saves_before_reset() {
    let mut good = session(WorldKind::Ecs);
    initialize(&mut good);
    action(&mut good, "g1.play");
    let payload = serde_json::to_value(profile(good.state()).unwrap()).unwrap();
    let mut variants = vec![json!(null), json!({"not":"a profile"})];
    for (key, value) in [
        ("version", json!(2)),
        ("catalog_revision", json!("foreign")),
    ] {
        let mut bad = payload.clone();
        bad[key] = value;
        variants.push(bad);
    }
    let mut broken_history = payload.clone();
    broken_history["campaign"]["attempt"]["history"]["current"]["moves"] = json!(5);
    variants.push(broken_history);
    let mut wrong_revision = payload.clone();
    wrong_revision["campaign"]["attempt"]["level_revision"] = json!("foreign");
    variants.push(wrong_revision);
    for raw in variants {
        let mut restored = session(WorldKind::Ecs);
        restored
            .seed_module_data([(SAVED_KEY.into(), raw.clone())].into_iter().collect())
            .unwrap();
        initialize(&mut restored);
        assert!(view(restored.state()).unwrap().warning.is_some());
        assert_eq!(restored.state().module_data[SAVED_KEY], raw);
        action(&mut restored, "g1.play");
        assert!(
            !restored
                .events()
                .iter()
                .any(|event| event.name == "g1.save")
        );
        tick(&mut restored, &volumes_input(0.1, 0.2));
        assert!(
            !restored
                .events()
                .iter()
                .any(|event| event.name == "g1.save")
        );
        action(&mut restored, "g1.reset_progress");
        assert!(view(restored.state()).unwrap().warning.is_none());
        assert_eq!(
            profile(restored.state()).unwrap().settings,
            Settings {
                music_volume: 0.1,
                sfx_volume: 0.2
            }
        );
        let reset = restored
            .events()
            .iter()
            .position(|event| event.name == "g1.storage.reset")
            .unwrap();
        let save = restored
            .events()
            .iter()
            .position(|event| event.name == "g1.save")
            .unwrap();
        assert!(reset < save);
        assert_eq!(restored.events()[reset].payload, json!({"confirm":true}));
        assert_eq!(restored.state().module_data[SAVED_KEY], raw);
    }
}

#[test]
fn legal_positions_with_illegal_undo_edges_are_not_accepted_as_resume_data() {
    let mut source = session(WorldKind::Ecs);
    initialize(&mut source);
    action(&mut source, "g1.play");
    tick(&mut source, &direction_input(Direction::East));
    let mut payload = serde_json::to_value(profile(source.state()).unwrap()).unwrap();
    payload["campaign"]["attempt"]["history"]["current"]["pawn"] = json!({"x":5,"y":3});
    let mut loaded = session(WorldKind::Ecs);
    loaded
        .seed_module_data([(SAVED_KEY.into(), payload)].into_iter().collect())
        .unwrap();
    initialize(&mut loaded);
    assert!(
        view(loaded.state())
            .unwrap()
            .warning
            .unwrap()
            .contains("disconnected")
    );
}

#[test]
fn captured_values_win_and_rejected_volume_packet_preserves_previous_transaction() {
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    let mut input = volumes_input(0.9, 0.8);
    input
        .action_values
        .insert("g1.music_volume".into(), json!(0.3));
    input
        .action_values
        .insert("g1.sfx_volume".into(), json!(0.4));
    tick(&mut session, &input);
    assert_eq!(
        profile(session.state()).unwrap().settings,
        Settings {
            music_volume: 0.3,
            sfx_volume: 0.4
        }
    );
    let before = session.state().clone();
    assert!(
        session
            .step(1. / 60., &volumes_input(1.00000001, 0.2))
            .is_err()
    );
    assert_eq!(session.state(), &before);
    // Rejected edges retain their fixed-time budget. Correct the captured
    // packet and drain that existing tick without adding more wall time.
    assert_eq!(session.step(0., &volumes_input(0.3, 0.4)).unwrap(), 1);
    assert_eq!(profile(session.state()).unwrap().settings.music_volume, 0.3);
}

#[test]
fn level_selection_replays_unlocked_content_and_rejects_locked_selection_atomically() {
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    walk(&mut session, "N");
    action(&mut session, "g1.next");
    action(&mut session, "g1.levels");
    let mut input = action_input("g1.select_level");
    input
        .action_values
        .insert("g1.level_id".into(), json!("level10"));
    let before = session.state().clone();
    assert!(session.step(1. / 60., &input).is_err());
    assert_eq!(session.state(), &before);
    input
        .action_values
        .insert("g1.level_id".into(), json!("level01"));
    assert_eq!(session.step(0., &input).unwrap(), 1);
    assert_eq!(view(session.state()).unwrap().moves, 0);
    assert_eq!(view(session.state()).unwrap().current_level, 0);
    assert_eq!(
        profile(session.state()).unwrap().campaign.best("level01"),
        Some(1)
    );
    walk(&mut session, "N");
    assert_eq!(view(session.state()).unwrap().phase, Phase::Victory);
}

#[test]
fn pause_and_settings_keep_attempt_and_prevent_direction_packets_from_moving() {
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    action(&mut session, "g1.pause");
    tick(&mut session, &direction_input(Direction::North));
    assert_eq!(view(session.state()).unwrap().phase, Phase::Paused);
    assert_eq!(view(session.state()).unwrap().moves, 0);
    action(&mut session, "g1.settings");
    action(&mut session, "g1.back");
    assert_eq!(view(session.state()).unwrap().phase, Phase::Paused);
    action(&mut session, "g1.pause");
    walk(&mut session, "N");
    assert_eq!(view(session.state()).unwrap().phase, Phase::Victory);
}

#[test]
fn painting_is_a_validated_candidate_keeps_blueprint_and_preserves_unrelated_payloads() {
    let mut source = project();
    let pawn = source
        .scenes
        .get_mut("level01")
        .unwrap()
        .objects
        .iter_mut()
        .find(|object| object.object.id == 900001)
        .unwrap();
    pawn.components.insert(
        "other.namespace".into(),
        json!({"literal.key":null,"unknown":[1,2]}),
    );
    pawn.object.color = [12, 34, 56, 255];
    let original = source.clone();
    let mut draft = GridDraft::from_project(&source, "level01").unwrap();
    assert_eq!(draft.into_project(&source).unwrap(), original);
    draft.paint(Cell::new(1, 2), Brush::Crate).unwrap();
    assert!(draft.into_project(&source).is_err());
    assert_eq!(source, original);
    draft.paint(Cell::new(1, 1), Brush::Goal).unwrap();
    let candidate = draft.into_project(&source).unwrap();
    assert_eq!(candidate.execution, source.execution);
    let pawn = candidate.scenes["level01"]
        .objects
        .iter()
        .find(|object| object.object.id == 900001)
        .unwrap();
    assert_eq!(
        pawn.components["other.namespace"],
        json!({"literal.key":null,"unknown":[1,2]})
    );
    assert_eq!(pawn.object.color, [12, 34, 56, 255]);
    assert_eq!(
        authored_board(&candidate, "level01").unwrap().crates.len(),
        2
    );
    let prepared = candidate.prepare_runtime(&registry()).unwrap();
    assert_ne!(prepared.execution, candidate.execution);
    assert_eq!(candidate.execution.stages[0].nodes[0].params, json!({}));
}

#[test]
fn registered_but_absent_game_does_not_prepare_catalog_or_activate_services() {
    let source = GameProject::empty();
    let registry = registry();
    let prepared = source.prepare_runtime(&registry).unwrap();
    assert_eq!(prepared, source);
    assert!(
        registry
            .active_game_contributions(&prepared.execution)
            .is_empty()
    );
}

#[test]
fn disposable_legacy_catalog_cannot_freeze_edited_scenes_or_override_runtime_derivation() {
    let registry = registry();
    let cached = project().prepare_runtime(&registry).unwrap();
    let frozen = cached.execution.clone();
    let mut draft = GridDraft::from_project(&cached, "level01").unwrap();
    draft.width += 1;
    let edited = draft.into_project(&cached).unwrap();
    assert_eq!(edited.execution, frozen);
    let runtime = edited.prepare_runtime(&registry).unwrap();
    assert_ne!(runtime.execution, frozen);
    assert_eq!(edited.execution, frozen);
    let mut session = GameSession::new_with_registry(runtime, WorldKind::Ecs, registry).unwrap();
    initialize(&mut session);
    assert_eq!(view(session.state()).unwrap().board.width, draft.width);
}

#[test]
fn painting_allocates_around_unrelated_future_grid_id_without_panic_or_data_loss() {
    let mut source = project();
    let mut unrelated = AuthoredObject::new(WorldObject {
        id: 30065,
        parent: None,
        transform: Transform::default(),
        color: [1, 2, 3, 255],
    });
    unrelated.components.insert(
        "other.namespace".into(),
        json!({"literal.key":null,"important":[1,2]}),
    );
    source
        .scenes
        .get_mut("level01")
        .unwrap()
        .objects
        .push(unrelated.clone());
    let before = source.clone();
    let mut draft = GridDraft::from_project(&source, "level01").unwrap();
    assert_eq!(draft.into_project(&source).unwrap(), before);
    draft.paint(Cell::new(1, 2), Brush::Crate).unwrap();
    draft.paint(Cell::new(1, 1), Brush::Goal).unwrap();
    let candidate = draft.into_project(&source).unwrap();
    let created = candidate.scenes["level01"]
        .objects
        .iter()
        .find(|object| {
            object
                .components
                .get(CELL)
                .is_some_and(|cell| cell["kind"] == "crate" && cell["x"] == 1 && cell["z"] == 2)
        })
        .unwrap();
    assert_ne!(created.object.id, 30065);
    assert_eq!(
        candidate.scenes["level01"]
            .objects
            .iter()
            .find(|object| object.object.id == 30065),
        Some(&unrelated)
    );
    assert_eq!(source, before);
    assert_eq!(
        source.scenes["level01"]
            .objects
            .iter()
            .find(|object| object.object.id == 30065),
        Some(&unrelated)
    );
    assert_eq!(
        source.scenes["level01"]
            .objects
            .iter()
            .find(|object| object
                .components
                .get(CELL)
                .is_some_and(|value| value["kind"] == "pawn"))
            .map(|object| object.object.id),
        Some(900001)
    );
}

#[test]
fn painting_preserves_noncanonical_semantic_ids_prefabs_payloads_and_references() {
    let mut source = project();
    let scene = source.scenes.get_mut("level01").unwrap();
    let board = scene
        .objects
        .iter_mut()
        .find(|object| object.components.contains_key(BOARD))
        .unwrap();
    board.object.id = 2;
    board
        .components
        .insert("other.board".into(), json!({"literal.key":null}));
    let crate_object = scene
        .objects
        .iter_mut()
        .find(|object| {
            object
                .components
                .get(CELL)
                .is_some_and(|cell| cell["kind"] == "crate")
        })
        .unwrap();
    crate_object.object.id = 600001;
    crate_object
        .components
        .insert("other.crate".into(), json!({"opaque":[1,2]}));
    let crate_before = crate_object.clone();
    let pawn = scene
        .objects
        .iter_mut()
        .find(|object| {
            object
                .components
                .get(CELL)
                .is_some_and(|cell| cell["kind"] == "pawn")
        })
        .unwrap();
    pawn.object.id = 900002;
    pawn.object.color = [12, 34, 56, 255];
    pawn.components
        .insert("other.pawn".into(), json!({"literal.key":null}));
    pawn.references.insert("board".into(), 2);
    let inherited_cell = pawn.components.remove(CELL).unwrap();
    pawn.prefab = Some("authored-gardener".into());
    let pawn_before = pawn.clone();
    let floor = scene
        .objects
        .iter_mut()
        .find(|object| {
            object
                .components
                .get(CELL)
                .is_some_and(|cell| cell["kind"] == "floor" && cell["x"] == 2 && cell["z"] == 3)
        })
        .unwrap();
    floor.object.id = 400000;
    floor
        .components
        .insert("other.floor".into(), json!({"keep":true}));
    let floor_before = floor.clone();
    let mut unrelated = AuthoredObject::new(WorldObject {
        id: u64::MAX,
        parent: None,
        transform: Transform::default(),
        color: [9, 8, 7, 255],
    });
    unrelated.references = BTreeMap::from([
        ("board".into(), 2),
        ("pawn".into(), 900002),
        ("crate".into(), 600001),
        ("floor".into(), 400000),
    ]);
    scene.objects.push(unrelated.clone());
    source.prefabs.insert(
        "authored-gardener".into(),
        Prefab {
            color: [12, 34, 56, 255],
            components: BTreeMap::from([(CELL.into(), inherited_cell)]),
        },
    );
    source.validate_with_registry(&registry()).unwrap();
    let before = source.clone();
    let mut draft = GridDraft::from_project(&source, "level01").unwrap();
    assert_eq!(draft.into_project(&source).unwrap(), source);
    draft.paint(Cell::new(1, 2), Brush::Wall).unwrap();
    let edited = draft.into_project(&source).unwrap();
    let objects = &edited.scenes["level01"].objects;
    assert_eq!(
        objects
            .iter()
            .find(|object| object.object.id == 2)
            .unwrap()
            .components["other.board"],
        json!({"literal.key":null})
    );
    assert_eq!(
        objects.iter().find(|object| object.object.id == 900002),
        Some(&pawn_before)
    );
    assert_eq!(
        objects.iter().find(|object| object.object.id == 600001),
        Some(&crate_before)
    );
    assert_eq!(
        objects.iter().find(|object| object.object.id == 400000),
        Some(&floor_before)
    );
    assert_eq!(
        objects.iter().find(|object| object.object.id == u64::MAX),
        Some(&unrelated)
    );
    let wall = objects
        .iter()
        .find(|object| {
            object
                .components
                .get(CELL)
                .is_some_and(|cell| cell["kind"] == "wall" && cell["x"] == 1 && cell["z"] == 2)
        })
        .unwrap();
    assert_eq!(
        wall.object.id, 1,
        "Allocate a gap instead of overflowing max_id + 1"
    );
    let mut move_pawn = GridDraft::from_project(&edited, "level01").unwrap();
    move_pawn.paint(Cell::new(2, 3), Brush::Pawn).unwrap();
    let moved = move_pawn.into_project(&edited).unwrap();
    let pawn = moved.scenes["level01"]
        .objects
        .iter()
        .find(|object| object.object.id == 900002)
        .unwrap();
    assert_eq!(pawn.prefab, pawn_before.prefab);
    assert_eq!(pawn.references, pawn_before.references);
    assert_eq!(pawn.object.color, pawn_before.object.color);
    assert_eq!(
        pawn.components["other.pawn"],
        pawn_before.components["other.pawn"]
    );
    assert_eq!(
        authored_board(&moved, "level01").unwrap().pawn,
        Cell::new(2, 3)
    );
    let mut replace_crate = GridDraft::from_project(&source, "level01").unwrap();
    replace_crate.paint(Cell::new(3, 2), Brush::Floor).unwrap();
    replace_crate.paint(Cell::new(1, 2), Brush::Crate).unwrap();
    assert!(
        replace_crate.into_project(&source).is_err(),
        "A removed referenced crate cannot donate its identity to a new crate"
    );
    assert_eq!(source, before);
}

#[test]
fn valid_small_actor_identity_does_not_collide_with_synthetic_wall_indices() {
    let mut source = project();
    source
        .scenes
        .get_mut("level01")
        .unwrap()
        .objects
        .iter_mut()
        .find(|object| {
            object
                .components
                .get(CELL)
                .is_some_and(|cell| cell["kind"] == "crate")
        })
        .unwrap()
        .object
        .id = 3;
    let mut session = session_from(&source, WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    walk(&mut session, "N");
    assert_eq!(view(session.state()).unwrap().phase, Phase::Victory);
}

#[test]
fn screen_and_key_actions_enter_the_same_source_rules_without_host_restart() {
    let map = action_map();
    let mut state = ActionState::default();
    let keyboard = map.evaluate(&mut state, &[PhysicalEvent::Pressed("R".into())], true);
    assert_eq!(keyboard, map.named_press("g1.restart").unwrap());
    assert!(
        map.evaluate(&mut state, &[PhysicalEvent::Repeated("R".into())], true)
            .pressed
            .is_empty()
    );
    let mut input = action_input("g1.restart");
    assert!(!input.restart);
    input.actions.insert("g1.north".into());
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    walk(&mut session, "E");
    tick(&mut session, &input);
    assert_eq!(
        view(session.state()).unwrap().moves,
        0,
        "Restart has priority over a simultaneous move"
    );
}

#[test]
fn direct_world_edit_cannot_bypass_source_grid_transform_guard() {
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        let mut session = session(kind);
        initialize(&mut session);
        action(&mut session, "g1.play");
        let before = session.state().clone();
        let mut transform = before.world.object(900001).unwrap().transform;
        transform.translation[0] += 0.25;
        assert!(
            session
                .apply_commands(&[WorldCommand::SetTransform {
                    id: 900001,
                    transform
                }])
                .is_err()
        );
        assert_eq!(session.state(), &before);
    }
}

/// CPU-only diagnostic, explicitly run in release. This does not measure a
/// rendered frame, native audio/storage IO or the 1080p presentation gate.
#[test]
#[ignore = "bounded release timing diagnostic; run separately from acceptance tests"]
fn diagnostic_long_attempt_and_idle_cpu_cost() {
    use std::time::Instant;
    fn statistics(samples: &[f64]) -> serde_json::Value {
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        json!({"samples":samples.len(),"mean_ms":samples.iter().sum::<f64>()/samples.len() as f64,"p95_ms":sorted[(samples.len()*95).div_ceil(100)-1],"max_ms":sorted.last()})
    }
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    let mut move_cost = Vec::new();
    for index in 0..1000 {
        let input = direction_input(if index % 2 == 0 {
            Direction::East
        } else {
            Direction::West
        });
        let started = Instant::now();
        tick(&mut session, &input);
        move_cost.push(started.elapsed().as_secs_f64() * 1000.);
    }
    let before = profile(session.state()).unwrap();
    let mut idle_cost = Vec::new();
    for _ in 0..300 {
        let started = Instant::now();
        tick(&mut session, &GameInput::default());
        idle_cost.push(started.elapsed().as_secs_f64() * 1000.);
    }
    assert_eq!(profile(session.state()).unwrap(), before);
    assert_eq!(before.campaign.attempt().unwrap().history.undo_len(), 1000);
    println!(
        "{}",
        json!({"diagnostic":"g1-source-cpu-only","debug_assertions":cfg!(debug_assertions),"world":"Ecs","successful_moves":1000,"profile_json_bytes":serde_json::to_vec(&before).unwrap().len(),"moves":statistics(&move_cost),"idle_after_1000_moves":statistics(&idle_cost)})
    );
}

#[test]
fn idle_ticks_preserve_published_payloads_and_do_not_repeat_save_or_audio_events() {
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    for _ in 0..20 {
        walk(&mut session, "EW");
    }
    let profile = session.state().module_data[PROFILE_KEY].clone();
    let view = session.state().module_data[VIEW_KEY].clone();
    let tick_before = session.state().tick;
    for _ in 0..10 {
        tick(&mut session, &GameInput::default());
        tick(&mut session, &action_input("other.module.noop"));
        assert_eq!(session.state().module_data[PROFILE_KEY], profile);
        assert_eq!(session.state().module_data[VIEW_KEY], view);
        assert!(
            !session
                .events()
                .iter()
                .any(|event| event.name.starts_with("g1."))
        );
    }
    assert_eq!(session.state().tick, tick_before + 20);
}

#[test]
fn grid_dimension_and_cell_size_edits_update_owned_geometry_without_losing_other_data() {
    let mut source = project();
    let scene = source.scenes.get_mut("level01").unwrap();
    let board = scene
        .objects
        .iter_mut()
        .find(|object| object.components.contains_key(BOARD))
        .unwrap();
    board.object.transform.translation = [2., 0., -1.];
    board
        .components
        .insert("unrelated.board".into(), json!({"literal.key":null}));
    let pawn = scene
        .objects
        .iter_mut()
        .find(|object| object.object.id == 900001)
        .unwrap();
    pawn.object.color = [12, 34, 56, 255];
    pawn.components
        .insert("unrelated.pawn".into(), json!({"opaque":[1,2,3]}));
    let unrelated = AuthoredObject::new(WorldObject {
        id: 700000,
        parent: None,
        transform: Transform {
            translation: [25., 2., -8.],
            scale: [3., 4., 5.],
            ..Default::default()
        },
        color: [3, 5, 7, 255],
    });
    scene.objects.push(unrelated.clone());
    let before = source.clone();
    let mut draft = GridDraft::from_project(&source, "level01").unwrap();
    assert_eq!(draft.into_project(&source).unwrap(), source);
    draft.width = 9;
    draft.height = 7;
    draft.cell_size = 2.;
    let candidate = draft.into_project(&source).unwrap();
    let scene = &candidate.scenes["level01"];
    let board = scene
        .objects
        .iter()
        .find(|object| object.components.contains_key(BOARD))
        .unwrap();
    assert_eq!(board.object.transform.scale, [9.1, 0.04, 7.1]);
    assert_eq!(board.object.transform.translation, [2., 0., -1.]);
    assert_eq!(
        board.components["unrelated.board"],
        json!({"literal.key":null})
    );
    for cell in scene
        .objects
        .iter()
        .filter(|object| object.components.contains_key(CELL))
    {
        assert_eq!(
            cell.object.transform.scale, [2.; 3],
            "retained and new cell geometry share the edited units"
        );
    }
    let pawn = scene
        .objects
        .iter()
        .find(|object| object.object.id == 900001)
        .unwrap();
    assert_eq!(pawn.object.transform.translation, [0., 0.84, -1.]);
    assert_eq!(pawn.object.color, [12, 34, 56, 255]);
    assert_eq!(pawn.components["unrelated.pawn"], json!({"opaque":[1,2,3]}));
    assert_eq!(
        scene
            .objects
            .iter()
            .find(|object| object.object.id == 700000),
        Some(&unrelated)
    );
    assert_eq!(candidate.execution, source.execution);
    assert_eq!(source, before);
    assert_eq!(
        GridDraft::from_project(&candidate, "level01")
            .unwrap()
            .into_project(&candidate)
            .unwrap(),
        candidate
    );

    // A width-only edit owns platform X and cell placement, not authored model
    // proportions or platform thickness on unchanged axes.
    let mut width_source = source.clone();
    let scene = width_source.scenes.get_mut("level01").unwrap();
    scene
        .objects
        .iter_mut()
        .find(|object| object.components.contains_key(BOARD))
        .unwrap()
        .object
        .transform
        .scale[1] = 0.07;
    scene
        .objects
        .iter_mut()
        .find(|object| object.object.id == 900001)
        .unwrap()
        .object
        .transform
        .scale = [0.8, 1.2, 0.9];
    let mut draft = GridDraft::from_project(&width_source, "level01").unwrap();
    draft.width = 9;
    let candidate = draft.into_project(&width_source).unwrap();
    let scene = &candidate.scenes["level01"];
    assert_eq!(
        scene
            .objects
            .iter()
            .find(|object| object.components.contains_key(BOARD))
            .unwrap()
            .object
            .transform
            .scale,
        [4.6, 0.07, 2.6]
    );
    assert_eq!(
        scene
            .objects
            .iter()
            .find(|object| object.object.id == 900001)
            .unwrap()
            .object
            .transform
            .scale,
        [0.8, 1.2, 0.9]
    );
    let mut invalid = GridDraft::from_project(&source, "level01").unwrap();
    invalid.width = u32::MAX;
    assert!(invalid.into_project(&source).is_err());
    assert_eq!(source, before);
}

// The production content loader remains application-owned. These tests read the
// positions of the actual original generated GLTF assets without adding a native
// importer dependency to the headless game package.
fn ambient_source_mesh(kind: &str) -> Mesh {
    let text = match kind {
        "pawn" => include_str!("../../../examples/crate-garden/assets/models/pawn.gltf"),
        "goal" => include_str!("../../../examples/crate-garden/assets/models/goal.gltf"),
        _ => panic!("Expected animated original asset"),
    };
    let gltf: serde_json::Value = serde_json::from_str(text).unwrap();
    let encoded = gltf["buffers"][0]["uri"]
        .as_str()
        .unwrap()
        .split_once(',')
        .unwrap()
        .1;
    let mut bytes = Vec::new();
    let (mut word, mut bits) = (0_u32, 0_u32);
    for byte in encoded.bytes().take_while(|byte| *byte != b'=') {
        let value = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
            .iter()
            .position(|value| *value == byte)
            .unwrap() as u32;
        word = ((word << 6) | value) & 0x00ff_ffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((word >> bits) as u8);
        }
    }
    let accessor = &gltf["accessors"][0];
    let view = &gltf["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
    let start = view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let count = accessor["count"].as_u64().unwrap() as usize;
    let vertices = bytes[start..start + count * 12]
        .as_chunks::<12>()
        .0
        .iter()
        .map(|point| {
            std::array::from_fn(|axis| {
                f32::from_le_bytes(point[axis * 4..axis * 4 + 4].try_into().unwrap())
            })
        })
        .collect();
    Mesh {
        vertices,
        triangles: vec![],
        texcoords: vec![],
    }
}

fn ambient_decorator() -> fn(&mut SceneSnapshot, &GameState) {
    // Exercise the registered pre-content projection, then independent late
    // color feedback. These fixtures contain raw owner objects, not primitives.
    |snapshot, game| {
        let registry = registry();
        let models = registry.pose_projection("g1.presentation").unwrap()(snapshot, game).unwrap();
        for item in &mut snapshot.items {
            if let Some(model) = models.get(&item.id) {
                item.model = *model;
            }
        }
        (registry
            .game_contributions()
            .find(|item| item.id == "g1.presentation")
            .unwrap()
            .decorate_snapshot)(snapshot, game);
    }
}

fn near_point(left: [f32; 3], right: [f32; 3]) {
    for (left, right) in left.into_iter().zip(right) {
        assert!((left - right).abs() < 0.00003, "{left} != {right}");
    }
}

#[test]
fn garden_poses_reach_session_interpolation_before_content_and_late_tint_does_not_repeat_motion() {
    for kind in [WorldKind::Ecs, WorldKind::Objects] {
        let mut session = session(kind);
        initialize(&mut session);
        action(&mut session, "g1.play");
        let previous = session.presentation_snapshot().unwrap();
        tick(&mut session, &GameInput::default());
        let state = session.state().clone();
        let raw = session.snapshot();
        let current = session.presentation_snapshot().unwrap();
        let identities = state
            .world
            .entity_ids()
            .map(|id| {
                (
                    id,
                    PresentationIdentity {
                        lifetime: state.world.lifetime(id).unwrap(),
                        parent: state.world.object(id).unwrap().parent,
                    },
                )
            })
            .collect();
        let quarter = interpolate_scene_models(&previous, &current, &identities, &identities, 0.25);
        let later = interpolate_scene_models(&previous, &current, &identities, &identities, 0.75);
        for kind in ["pawn", "goal"] {
            let id = state
                .world
                .entities_with(&[CELL])
                .into_iter()
                .find(|id| state.world.component(*id, CELL).unwrap()["kind"] == kind)
                .unwrap();
            let model = |snapshot: &SceneSnapshot| {
                snapshot
                    .items
                    .iter()
                    .find(|item| item.id == id)
                    .unwrap()
                    .model
            };
            assert_ne!(
                model(&quarter),
                model(&later),
                "{kind} needs a genuine interpolated pose"
            );
            assert_ne!(
                model(&later),
                model(&current),
                "{kind} must not snap because of source shear"
            );
        }
        let mut decorated = later.clone();
        (registry()
            .game_contributions()
            .find(|item| item.id == "g1.presentation")
            .unwrap()
            .decorate_snapshot)(&mut decorated, &state);
        assert_eq!(
            decorated
                .items
                .iter()
                .map(|item| item.model)
                .collect::<Vec<_>>(),
            later
                .items
                .iter()
                .map(|item| item.model)
                .collect::<Vec<_>>()
        );
        assert_eq!(session.state(), &state);
        assert_eq!(session.snapshot(), raw);
        assert_eq!(view(session.state()).unwrap().moves, 0);
    }
}

#[test]
fn ambient_original_gardener_breathes_with_an_anchored_sole_plane_and_bounded_heading() {
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    let source_before = session.project().clone();
    let session_before = session.state().clone();
    let events_before = session.events().to_vec();
    let mut game = session_before.clone();
    let pawn = game
        .world
        .entities_with(&[CELL])
        .into_iter()
        .find(|id| game.world.component(*id, CELL).unwrap()["kind"] == "pawn")
        .unwrap();
    let parent = WorldObject {
        id: 999_999,
        parent: None,
        transform: Transform {
            translation: [2., 0.2, -3.],
            rotation: [0.2, -0.3, 0.1],
            scale: [1.2, 0.9, 1.1],
        },
        color: [255; 4],
    };
    game.world
        .apply_commands(&[WorldCommand::Spawn {
            object: parent.clone(),
        }])
        .unwrap();
    let mut entity = game.world.entity(pawn).unwrap();
    entity.object.parent = Some(parent.id);
    entity.object.transform.rotation = [0.1, 0.45, -0.15];
    entity.object.transform.scale = [0.9, 1.1, 0.8];
    let frame = multiply_matrices(parent.transform.matrix(), entity.object.transform.matrix());
    game.world.upsert_entity(entity).unwrap();
    let mesh = ambient_source_mesh("pawn");
    let sole = mesh
        .vertices
        .iter()
        .map(|point| point[1])
        .fold(f32::INFINITY, f32::min);
    assert!(
        (sole + 0.42).abs() < 0.000001,
        "Original model's sole anchor changed"
    );
    let texture = std::sync::Arc::new(TextureImage {
        width: 1,
        height: 1,
        rgba8: vec![75, 160, 170, 255],
    });
    let material = Material {
        base_color: [0.8, 0.7, 0.6, 0.9],
        base_color_texture: Some(texture.clone()),
    };
    let first = RenderItem {
        id: pawn,
        mesh: mesh.clone(),
        model: frame,
        color: [70, 147, 165, 255],
        material: material.clone(),
    };
    let original = SceneSnapshot {
        items: vec![first],
        time: 0.,
    };
    let decorate = ambient_decorator();
    let mut early = original.clone();
    game.time = 0.;
    decorate(&mut early, &game);
    let mut late = original.clone();
    game.time = 1.;
    let game_before = game.clone();
    decorate(&mut late, &game);
    let mut repeat = original.clone();
    decorate(&mut repeat, &game);
    assert_eq!(
        late, repeat,
        "Same simulation time must produce the same pose"
    );
    assert_ne!(early.items[0].model, late.items[0].model);
    let mut largest_motion = 0_f32;
    let anchor = transform_point(frame, [0., sole, 0.]);
    let tangent_x = transform_point(frame, [1., sole, 0.]);
    let tangent_z = transform_point(frame, [0., sole, 1.]);
    let x: [f32; 3] = std::array::from_fn(|i| tangent_x[i] - anchor[i]);
    let z: [f32; 3] = std::array::from_fn(|i| tangent_z[i] - anchor[i]);
    let normal = [
        x[1] * z[2] - x[2] * z[1],
        x[2] * z[0] - x[0] * z[2],
        x[0] * z[1] - x[1] * z[0],
    ];
    for point in &mesh.vertices {
        let before = transform_point(early.items[0].model, *point);
        let after = transform_point(late.items[0].model, *point);
        if (point[1] - sole).abs() < 0.000001 {
            let plane_distance = (0..3)
                .map(|i| (after[i] - anchor[i]) * normal[i])
                .sum::<f32>();
            assert!(
                plane_distance.abs() < 0.00003,
                "Sole left its authored contact plane"
            );
        }
        largest_motion = largest_motion.max(
            before
                .into_iter()
                .zip(after)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                .sqrt(),
        );
    }
    assert!(
        largest_motion > 0.02 && largest_motion < 0.1,
        "Visible bounded original-model pose: {largest_motion}"
    );
    for item in &late.items {
        assert_eq!(item.material, material);
        assert!(std::sync::Arc::ptr_eq(
            item.material.base_color_texture.as_ref().unwrap(),
            &texture
        ));
        assert_eq!(item.color, [70, 147, 165, 255]);
    }
    assert_eq!(game, game_before);
    assert_eq!(session.state(), &session_before);
    assert_eq!(session.events(), events_before.as_slice());
    assert_eq!(session.project(), &source_before);
}

#[test]
fn ambient_original_flower_turns_in_its_local_plane_and_does_not_change_undo_or_goal_tint() {
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    walk(&mut session, "N");
    action(&mut session, "g1.next");
    walk(&mut session, "WN");
    let state = session.state().clone();
    let events = session.events().to_vec();
    let mut game = state.clone();
    let goal = game
        .world
        .entities_with(&[CELL])
        .into_iter()
        .find(|id| game.world.component(*id, CELL).unwrap()["kind"] == "goal")
        .unwrap();
    let frame = game.world.object(goal).unwrap().transform.matrix();
    let mut raw = session.snapshot();
    let item = raw.items.iter_mut().find(|item| item.id == goal).unwrap();
    item.mesh = ambient_source_mesh("goal");
    let geometry = item.mesh.clone();
    let decorate = ambient_decorator();
    let mut start = raw.clone();
    game.time = 0.;
    decorate(&mut start, &game);
    let mut quarter = raw.clone();
    game.time = 3.;
    decorate(&mut quarter, &game);
    let turning = quarter.items.iter().find(|item| item.id == goal).unwrap();
    assert_ne!(
        start
            .items
            .iter()
            .find(|item| item.id == goal)
            .unwrap()
            .model,
        turning.model
    );
    for point in &geometry.vertices {
        let c = std::f32::consts::FRAC_1_SQRT_2;
        near_point(
            transform_point(turning.model, *point),
            transform_point(
                frame,
                [
                    c * (point[0] + point[2]),
                    point[1],
                    c * (point[2] - point[0]),
                ],
            ),
        );
    }
    let mut cycle = raw.clone();
    game.time = 24.;
    decorate(&mut cycle, &game);
    assert_eq!(
        cycle.items.iter().find(|item| item.id == goal).unwrap(),
        start.items.iter().find(|item| item.id == goal).unwrap()
    );
    let level = crate::components::world_level(&game.world).unwrap();
    for (id, cell) in level.initial.crates {
        let before = raw.items.iter().find(|item| item.id == id).unwrap();
        let after = quarter.items.iter().find(|item| item.id == id).unwrap();
        assert_eq!(
            before.model, after.model,
            "Ambient pose must not move crates"
        );
        if level.goals.contains(&cell) {
            assert_eq!(after.color, [80, 245, 130, before.color[3]]);
        } else {
            assert_eq!(after, before);
        }
    }
    assert_eq!(session.state(), &state);
    assert_eq!(session.events(), events.as_slice());
    action(&mut session, "g1.undo");
    let after_undo = session.state().clone();
    let mut snapshot = session.snapshot();
    decorate(&mut snapshot, session.state());
    assert_eq!(session.state(), &after_undo);
    assert_eq!(view(session.state()).unwrap().moves, 1);
}

#[test]
fn ambient_time_is_finite_bounded_and_invalid_clocks_leave_geometry_unchanged() {
    let mut session = session(WorldKind::Ecs);
    initialize(&mut session);
    action(&mut session, "g1.play");
    let raw = session.snapshot();
    let decorate = ambient_decorator();
    let state = session.state().clone();
    for time in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut game = state.clone();
        game.time = time;
        let mut snapshot = raw.clone();
        decorate(&mut snapshot, &game);
        assert_eq!(snapshot, raw, "Invalid clocks retain ordinary geometry");
        assert_eq!(game.world, state.world);
        assert_eq!(game.module_data, state.module_data);
        assert_eq!(game.time.to_bits(), time.to_bits());
    }
    for time in [f64::MAX, -f64::MAX, -1., 1e100] {
        let mut game = state.clone();
        game.time = time;
        let mut snapshot = raw.clone();
        decorate(&mut snapshot, &game);
        let mut again = raw.clone();
        decorate(&mut again, &game);
        assert_eq!(snapshot, again);
        assert!(
            snapshot
                .items
                .iter()
                .flat_map(|item| item.model.iter().flatten())
                .all(|value| value.is_finite())
        );
        assert_eq!(game.world, state.world);
        assert_eq!(game.module_data, state.module_data);
    }
}
