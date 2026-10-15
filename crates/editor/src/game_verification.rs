//! Acceptance through the same GameHost as the editor and standalone Player.
//! Automated progression changes only authored fixture health, never teleports
//! or removes runtime actors; benchmark runs the supplied shipping document.
use crate::game_host::{GameHost, game_registry_at, script_ids};
use module_engine_core::{
    ExecutableSystem, GameEntity, GameEvent, GameInput, GameSession, GameState, GameStatus,
    GraphDocument, RenderOverrides, SystemContext, SystemDescriptor, SystemNode, SystemRegistry,
    SystemStage, WorldKind,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};

const DT: f64 = 1. / 60.;
const WARM_FRAMES: usize = 30;
const MEASURED_FRAMES: usize = 120;
const WIDTH: usize = 1920;
const HEIGHT: usize = 1080;

/// Persist honest evidence including a failed performance target. Functional
/// acceptance errors fail the command; performance shortfalls remain report data.
pub fn run(
    document: &GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    folder: &Path,
) -> Result<Value, String> {
    fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let original = document.clone();
    let parity = parity(document, root, compile_root, folder)?;
    let custom_transaction = custom_transaction(document, root, compile_root)?;
    let progression = progression(document, root, compile_root, folder)?;
    let defeat = defeat_fixture(document, root, compile_root, folder)?;
    let performance = performance(document, root, compile_root, folder)?;
    if document != &original {
        return Err("Verification mutated the supplied authored document".into());
    }
    let report = json!({"functional_checks_passed":true,"parity":parity,"custom_transaction":custom_transaction,"progression":progression,"defeat":defeat,"performance":performance,
        "scope":"Real GameHost simulation, Vulkan rendering, imported content, configured Rust and audio. Presentation/vsync and editor HUD drawing are outside processing timings.",
        "script_loading":if compile_root.is_some(){"author source compilation"}else{"packaged precompiled workers; no compiler invoked"},
        "authored_document_unchanged":true});
    fs::write(
        folder.join("game-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}

struct TransactionValidator {
    reject: bool,
}
impl ExecutableSystem for TransactionValidator {
    fn step(&mut self, context: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        if let Some(entity) = context.game.entities.values_mut().next() {
            entity.object.color = [251, 3, 4, 255];
        }
        Ok(())
    }
    fn validate_tick(&mut self) -> Result<(), String> {
        if self.reject {
            Err("Acceptance fixture rejects after Custom".into())
        } else {
            Ok(())
        }
    }
}
fn validator(registry: &mut SystemRegistry, reject: bool) -> Result<(), String> {
    registry.register(
        SystemDescriptor {
            id: "verification.transaction-validator".into(),
            state_schema: "validator/1".into(),
            components: vec![],
            ports: vec![],
            requires: vec![],
        },
        move |_| Ok(Box::new(TransactionValidator { reject })),
    )
}
fn custom_transaction(
    document: &GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
) -> Result<Value, String> {
    let host = GameHost::new(document.clone(), root, compile_root, WorldKind::Objects)?;
    if host.slots.is_empty() {
        return Ok(
            json!({"exercised":false,"reason":"Supplied graph contains no Custom system; native rollback is covered by core tests."}),
        );
    }
    let overrides = Arc::new(Mutex::new(RenderOverrides::default()));
    let mut game = host.session.project().clone();
    let after = game
        .execution
        .stages
        .iter()
        .flat_map(|s| &s.nodes)
        .last()
        .map(|n| n.id.clone())
        .into_iter()
        .collect();
    game.execution.stages.push(SystemStage {
        name: "Verification rejects staged outputs".into(),
        nodes: vec![SystemNode {
            id: "verification-validator".into(),
            module: "verification.transaction-validator".into(),
            params: Value::Null,
            bindings: BTreeMap::new(),
            after,
        }],
    });
    let before_slots = host
        .slots
        .iter()
        .map(|(id, slot)| {
            Ok((
                id.clone(),
                slot.lock()
                    .map_err(|_| "Script slot poisoned")?
                    .state()
                    .map_err(|e| e.to_string())?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let mut registry = game_registry_at(
        document,
        &host.root,
        &host.backend_root,
        &host.slots,
        overrides.clone(),
    )?;
    validator(&mut registry, true)?;
    let mut session = GameSession::new_with_registry(game, WorldKind::Objects, registry)?;
    let before_world = session.world_objects();
    let before_systems = session.system_states();
    let rejected = match session.step(DT, &GameInput::default()) {
        Err(error) => error,
        Ok(_) => return Err("Native validator unexpectedly accepted staged tick".into()),
    };
    if !rejected.contains("Acceptance fixture rejects") {
        return Err(format!("Unexpected transaction test error: {rejected}"));
    }
    let after_slots = host
        .slots
        .iter()
        .map(|(id, slot)| {
            Ok((
                id.clone(),
                slot.lock()
                    .map_err(|_| "Script slot poisoned")?
                    .state()
                    .map_err(|e| e.to_string())?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    if session.world_objects() != before_world
        || session.system_states() != before_systems
        || before_slots != after_slots
        || *overrides.lock().map_err(|_| "Render slot poisoned")? != RenderOverrides::default()
    {
        return Err(
            "A later system rejection leaked world, owned Custom state or render controls".into(),
        );
    }
    let mut accepting = game_registry_at(
        document,
        &host.root,
        &host.backend_root,
        &host.slots,
        overrides.clone(),
    )?;
    validator(&mut accepting, false)?;
    session.replace_system("verification-validator", &accepting, None, false)?;
    session.step(DT, &GameInput::default())?;
    if session.state().tick == 0 {
        return Err("Last-good game did not resume after rejected tick".into());
    }
    Ok(
        json!({"exercised":true,"passed":true,"custom_instances":host.slots.keys().collect::<Vec<_>>(),"world_restored":true,"owned_script_state_restored":true,"system_state_restored":true,"render_controls_restored":true,"last_good_resumed":true,"failure_stage":"native validate_tick after every Custom step"}),
    )
}

fn defeat_fixture(
    document: &GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    folder: &Path,
) -> Result<Value, String> {
    let mut fixture = document.clone();
    for scene in fixture.game.as_mut().ok_or("No game")?.scenes.values_mut() {
        for entity in &mut scene.objects {
            if entity.components.contains_key("player") {
                entity
                    .components
                    .insert("health".into(), json!({"maximum":1.,"current":1.}));
            }
        }
    }
    let mut host = GameHost::new(fixture, root, compile_root, WorldKind::Objects)?;
    let enemies = host.session.state().living_enemies();
    let mut death_event = false;
    for _ in 0..360 {
        host.step(DT, &GameInput::default())?;
        death_event |= host
            .session
            .events()
            .iter()
            .any(|e| matches!(e, GameEvent::Death { object: 1 }));
        if host.session.state().status == GameStatus::Defeat {
            break;
        }
    }
    if host.session.state().status != GameStatus::Defeat || !death_event {
        return Err("Enemy attacks did not produce player death/defeat in the low-health acceptance fixture".into());
    }
    fs::write(
        folder.join("defeat-state.json"),
        serde_json::to_vec_pretty(host.session.state()).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    host.step(
        DT,
        &GameInput {
            restart: true,
            ..GameInput::default()
        },
    )?;
    if host.session.state().status != GameStatus::Playing
        || host.session.state().living_enemies() != enemies
    {
        return Err("Restart action failed after defeat".into());
    }
    Ok(
        json!({"passed":true,"fixture":"Player maximum/current health=1; real enemy attacks cause death; no runtime kill commands.","death_event":true,"defeat_status":true,"restart_input_restores_authored_scene":true,"restored_enemies":enemies}),
    )
}

fn parity(
    document: &GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    folder: &Path,
) -> Result<Value, String> {
    let mut objects = GameHost::new(document.clone(), root, compile_root, WorldKind::Objects)?;
    let mut ecs = GameHost::new(document.clone(), root, compile_root, WorldKind::Ecs)?;
    let mut event_count = 0;
    for index in 0_usize..180 {
        let movement = match index / 60 {
            0 => [0.7, 0.],
            1 => [0., -0.7],
            _ => [-0.7, 0.],
        };
        let input = GameInput {
            movement,
            aim_world: [0., 0., -5.],
            attack: index.is_multiple_of(23),
            ..GameInput::default()
        };
        objects.step(DT, &input)?;
        ecs.step(DT, &input)?;
        if objects.session.state() != ecs.session.state()
            || objects.session.snapshot() != ecs.session.snapshot()
            || objects.session.events() != ecs.session.events()
            || objects.session.system_states() != ecs.session.system_states()
        {
            return Err(format!(
                "Object/ECS gameplay, event or Custom state parity failed at tick {index}"
            ));
        }
        event_count += objects.session.events().len();
    }
    let a = objects.render(960, 540)?;
    let b = ecs.render(960, 540)?;
    if a.rgba != b.rgba {
        return Err("Object/ECS imported scene differs after actual Vulkan rendering".into());
    }
    crate::write_png(&folder.join("game-parity-objects.png"), &a)?;
    crate::write_png(&folder.join("game-parity-ecs.png"), &b)?;
    Ok(
        json!({"passed":true,"fixed_ticks":objects.session.state().tick,"input_frames":180,"events_compared":event_count,
        "world_queries_equal":true,"scene_snapshots_equal":true,"vulkan_pixels_equal":true,"system_states_equal":true,
        "custom_sources":script_ids(document.game.as_ref().ok_or("No game")?),"audio_cues_consumed":objects.audio.played,
        "render_size":[960,540],"device":objects.device}),
    )
}

fn progression(
    document: &GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    folder: &Path,
) -> Result<Value, String> {
    let mut fixture = document.clone();
    let game = fixture.game.as_mut().ok_or("No game")?;
    for scene in game.scenes.values_mut() {
        for entity in &mut scene.objects {
            if entity.components.contains_key("player") {
                entity
                    .components
                    .insert("health".into(), json!({"maximum":5000.,"current":5000.}));
            }
        }
    }
    fs::write(
        folder.join("progression-fixture.json"),
        serde_json::to_vec_pretty(&fixture).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut host = GameHost::new(fixture, root, compile_root, WorldKind::Objects)?;
    crate::write_png(&folder.join("game-start.png"), &host.render(960, 540)?)?;
    let initial_enemies = host.session.state().living_enemies();
    let initial_scene = host.session.state().scene.clone();
    let mut events = BTreeMap::<String, u64>::new();
    let mut scenes = vec![initial_scene];
    let mut warning_seen = false;
    let mut boss_hits = 0;
    let mut attacks = 0;
    let mut hits = 0;
    let mut last_frame = 0;
    let mut warning_image_saved = false;
    for index in 0..9000 {
        let input = autopilot(host.session.state(), index)?;
        if !host.session.state().telegraphs.is_empty() {
            if host
                .session
                .state()
                .entities
                .values()
                .any(|e| e.components.contains_key("boss"))
            {
                warning_seen = true;
            }
            if !warning_image_saved && warning_seen {
                crate::write_png(&folder.join("game-boss-scene.png"), &host.render(960, 540)?)?;
                fs::write(
                    folder.join("boss-warning-state.json"),
                    serde_json::to_vec_pretty(host.session.state()).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                warning_image_saved = true;
            }
        }
        host.step(DT, &input)?;
        last_frame = index;
        for event in host.session.events() {
            let key = match event {
                GameEvent::Attack { .. } => {
                    attacks += 1;
                    "Attack"
                }
                GameEvent::Hit {
                    attacker, target, ..
                } => {
                    hits += 1;
                    if *target == 1
                        && host
                            .session
                            .state()
                            .entities
                            .get(attacker)
                            .is_some_and(|e| e.components.contains_key("boss"))
                    {
                        boss_hits += 1;
                    }
                    "Hit"
                }
                GameEvent::Death { .. } => "Death",
                GameEvent::SceneChanged { to, .. } => {
                    scenes.push(to.clone());
                    "SceneChanged"
                }
                GameEvent::Victory => "Victory",
                GameEvent::Sound { .. } => "Sound",
                GameEvent::Custom { .. } => "Custom",
            };
            *events.entry(key.into()).or_default() += 1;
        }
        if host.session.state().status == GameStatus::Victory {
            break;
        }
        if host.session.state().status == GameStatus::Defeat {
            return Err("Unattended progression fixture player died".into());
        }
    }
    if host.session.state().status != GameStatus::Victory {
        return Err(format!(
            "Autopilot did not complete the two-scene fixture after {} input frames; scene={}, enemies={}, player={:?}",
            last_frame + 1,
            host.session.state().scene,
            host.session.state().living_enemies(),
            host.session
                .state()
                .player()
                .map(|e| e.object.transform.translation)
        ));
    }
    if scenes.len() < 2 || !warning_seen || attacks == 0 || hits == 0 {
        return Err("Progression did not exercise scenes, real attacks and boss warnings".into());
    }
    crate::write_png(
        &folder.join("game-victory-scene.png"),
        &host.render(960, 540)?,
    )?;
    let play_time = host.session.state().time;
    let played = host.audio.played;
    host.session.restart()?;
    if host.session.state().living_enemies() != initial_enemies
        || host.session.state().status != GameStatus::Playing
    {
        return Err("Restart failed to restore authored enemies and status".into());
    }
    Ok(
        json!({"passed":true,"fixture":"Authored clone with player maximum/current health=5000; original enemy count, damage, cooldowns and boss warning retained.",
        "runtime_teleports_or_kill_commands":false,"controls":"Portable movement and attack edges only; obstacle-aware test autopilot.",
        "initial_enemies":initial_enemies,"input_frames":last_frame+1,"simulated_seconds":play_time,"scenes":scenes,
        "events":events,"boss_warning_seen":warning_seen,"boss_hits_on_player":boss_hits,"audio_cues_consumed":played,
        "victory_reached":true,"restart_restored_entry_scene":true,
        "limits":"This fixture verifies mechanics/progression, not the shipping game's difficulty or HUD presentation."}),
    )
}

fn performance(
    document: &GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    folder: &Path,
) -> Result<Value, String> {
    let mut host = GameHost::new(document.clone(), root, compile_root, WorldKind::Objects)?;
    let initial_enemies = host.session.state().living_enemies();
    let input = GameInput {
        aim_world: [0., 0., -4.],
        ..GameInput::default()
    };
    for _ in 0..WARM_FRAMES {
        host.step(DT, &input)?;
        host.render(WIDTH, HEIGHT)?;
    }
    let mut total_ms = vec![];
    let mut step_ms = vec![];
    let mut render_ms = vec![];
    let mut last = None;
    let mut minimum_enemies = initial_enemies;
    for _ in 0..MEASURED_FRAMES {
        let start = Instant::now();
        host.step(DT, &input)?;
        let frame = host.render(WIDTH, HEIGHT)?;
        total_ms.push(start.elapsed().as_secs_f64() * 1000.);
        step_ms.push(host.last_step_ms);
        render_ms.push(host.last_render_ms);
        minimum_enemies = minimum_enemies.min(host.session.state().living_enemies());
        last = Some(frame);
    }
    crate::write_png(
        &folder.join("game-performance-1080p.png"),
        &last.ok_or("No measured frame")?,
    )?;
    let mean_ms = total_ms.iter().sum::<f64>() / total_ms.len() as f64;
    let p95 = percentile(&total_ms, 0.95);
    let budget = 1000. / 60.;
    let actual_workload =
        minimum_enemies >= 20 && host.session.state().status == GameStatus::Playing;
    Ok(
        json!({"device":host.device,"resolution":[WIDTH,HEIGHT],"warmup_frames":WARM_FRAMES,"measured_frames":MEASURED_FRAMES,
        "fixed_step_seconds":DT,"initial_enemies":initial_enemies,"minimum_living_enemies":minimum_enemies,
        "player_status_after_measurement":host.session.state().status,"target_60_fps_20_enemies_workload":actual_workload,
        "frame_processing_ms":{"mean":mean_ms,"median":percentile(&total_ms,0.5),"p95":p95,"max":percentile(&total_ms,1.)},
        "simulation_ms":{"median":percentile(&step_ms,0.5),"p95":percentile(&step_ms,0.95)},
        "render_ms":{"median":percentile(&render_ms,0.5),"p95":percentile(&render_ms,0.95)},
        "mean_processing_fps":1000./mean_ms,"budget_ms":budget,"frames_over_budget":total_ms.iter().filter(|v|**v>budget).count(),
        "target_met":actual_workload&&mean_ms<=budget&&p95<=budget,"custom_sources":script_ids(document.game.as_ref().ok_or("No game")?),
        "audio_cues_consumed":host.audio.played,"frame_samples_ms":total_ms,
        "measurement_scope":"GameHost step + audio consume + imported snapshot enrichment + actual Vulkan render/readback; no vsync, window presentation or editor HUD."}),
    )
}

fn percentile(values: &[f64], fraction: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return 0.;
    }
    if fraction == 0.5 {
        return (sorted[(sorted.len() - 1) / 2] + sorted[sorted.len() / 2]) * 0.5;
    }
    let index = (sorted.len() as f64 * fraction).ceil() as usize;
    sorted[index.saturating_sub(1).min(sorted.len() - 1)]
}

fn horizontal_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
fn direction(from: [f32; 3], to: [f32; 3]) -> [f32; 2] {
    let d = horizontal_distance(from, to);
    if d > 0.001 {
        [(to[0] - from[0]) / d, (to[2] - from[2]) / d]
    } else {
        [0., 0.]
    }
}
fn number(e: &GameEntity, component: &str, field: &str, default: f32) -> f32 {
    e.components
        .get(component)
        .and_then(|v| v.get(field))
        .and_then(Value::as_f64)
        .map_or(default, |n| n as f32)
}

fn autopilot(state: &GameState, index: usize) -> Result<GameInput, String> {
    let player = state.player().ok_or("Acceptance game has no player")?;
    let p = player.object.transform.translation;
    let radius = number(player, "actor", "radius", 0.35);
    let obstacles: Vec<_> = state
        .entities
        .values()
        .filter_map(|e| {
            let half = e
                .components
                .get("obstacle")?
                .get("half_extents")?
                .as_array()?;
            Some((
                e.object.transform.translation,
                [
                    half.first()?.as_f64()? as f32,
                    half.get(1)?.as_f64()? as f32,
                ],
            ))
        })
        .collect();
    let target = state
        .entities
        .values()
        .filter(|e| {
            e.components
                .get("actor")
                .and_then(|a| a.get("team"))
                .and_then(Value::as_str)
                == Some("enemy")
                && number(e, "health", "current", 1.) > 0.
        })
        .min_by(|a, b| {
            horizontal_distance(p, a.object.transform.translation)
                .total_cmp(&horizontal_distance(p, b.object.transform.translation))
        });
    if let Some(target) = target {
        let goal = target.object.transform.translation;
        let d = horizontal_distance(p, goal);
        let warning = state
            .telegraphs
            .iter()
            .find(|t| t.attacker == target.object.id);
        if let Some(warning) = warning {
            if d < warning.range + radius + 1.0 {
                let away = direction(goal, p);
                let escape = [p[0] + away[0] * 2., p[1], p[2] + away[1] * 2.];
                return Ok(GameInput {
                    movement: navigate(p, escape, radius, &obstacles),
                    aim_world: goal,
                    attack: index.is_multiple_of(23),
                    ..GameInput::default()
                });
            }
            return Ok(GameInput {
                aim_world: goal,
                ..GameInput::default()
            });
        }
        let target_radius = number(target, "actor", "radius", 0.4);
        let desired = (number(player, "melee", "range", 2.2) + target_radius - 0.4).max(0.8);
        let movement = if d > desired {
            navigate(p, goal, radius, &obstacles)
        } else {
            [0., 0.]
        };
        return Ok(GameInput {
            movement,
            aim_world: goal,
            attack: index.is_multiple_of(23),
            ..GameInput::default()
        });
    }
    let portal = state
        .entities
        .values()
        .find(|e| e.components.contains_key("portal"))
        .ok_or("No living target and no exit portal")?;
    let goal = portal.object.transform.translation;
    Ok(GameInput {
        movement: navigate(p, goal, radius, &obstacles),
        aim_world: goal,
        ..GameInput::default()
    })
}

type Obstacle = ([f32; 3], [f32; 2]);
fn clear(p: [f32; 3], radius: f32, obstacles: &[Obstacle]) -> bool {
    obstacles.iter().all(|(center, half)| {
        let dx = ((p[0] - center[0]).abs() - half[0]).max(0.);
        let dz = ((p[2] - center[2]).abs() - half[1]).max(0.);
        dx * dx + dz * dz >= (radius + 0.04).powi(2)
    })
}
fn navigate(from: [f32; 3], to: [f32; 3], radius: f32, obstacles: &[Obstacle]) -> [f32; 2] {
    let d = horizontal_distance(from, to);
    let steps = ((d / 0.15).ceil() as usize).max(1);
    if (1..=steps).all(|i| {
        let t = i as f32 / steps as f32;
        clear(
            [
                from[0] + (to[0] - from[0]) * t,
                from[1],
                from[2] + (to[2] - from[2]) * t,
            ],
            radius,
            obstacles,
        )
    }) {
        return direction(from, to);
    }
    const N: i32 = 41;
    const CELL: f32 = 0.4;
    const OFFSET: f32 = -8.;
    let grid = |p: [f32; 3]| {
        [
            ((p[0] - OFFSET) / CELL).round() as i32,
            ((p[2] - OFFSET) / CELL).round() as i32,
        ]
        .map(|n| n.clamp(0, N - 1))
    };
    let point = |p: [i32; 2]| {
        [
            OFFSET + p[0] as f32 * CELL,
            from[1],
            OFFSET + p[1] as f32 * CELL,
        ]
    };
    let start = grid(from);
    let mut goal = grid(to);
    if !clear(point(goal), radius, obstacles) {
        let mut nearest = None;
        for x in 0..N {
            for z in 0..N {
                let node = [x, z];
                if clear(point(node), radius, obstacles) {
                    let d = horizontal_distance(point(node), to);
                    if nearest.is_none_or(|(_, best)| d < best) {
                        nearest = Some((node, d));
                    }
                }
            }
        }
        if let Some((node, _)) = nearest {
            goal = node;
        } else {
            return [0., 0.];
        }
    }
    let mut frontier = VecDeque::from([start]);
    let mut previous = BTreeMap::from([(start, start)]);
    while let Some(current) = frontier.pop_front() {
        if current == goal {
            break;
        }
        for delta in [[-1, 0], [1, 0], [0, -1], [0, 1]] {
            let next = [current[0] + delta[0], current[1] + delta[1]];
            if next.iter().any(|n| *n < 0 || *n >= N)
                || previous.contains_key(&next)
                || !clear(point(next), radius, obstacles)
            {
                continue;
            }
            previous.insert(next, current);
            frontier.push_back(next);
        }
    }
    if !previous.contains_key(&goal) {
        return [0., 0.];
    }
    let mut next = goal;
    while previous[&next] != start && previous[&next] != next {
        next = previous[&next];
    }
    direction(from, point(next))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_routes_around_the_actual_blocking_shape() {
        let wall = ([0., 0., 0.], [0.8, 0.8]);
        let input = navigate([-1.2, 0., 0.], [2., 0., 0.], 0.35, &[wall]);
        assert!(input[1].abs() > 0.01);
        assert!(clear([-2., 0., 0.], 0.35, &[wall]));
        assert!(!clear([0., 0., 0.], 0.35, &[wall]));
    }
    #[test]
    fn timings_use_nearest_rank_p95_and_max() {
        let v = [2., 1., 4., 3.];
        assert_eq!(percentile(&v, 0.5), 2.5);
        assert_eq!(percentile(&v, 0.95), 4.);
        assert_eq!(percentile(&v, 1.), 4.);
    }
    #[test]
    fn unattended_input_fixture_completes_both_scenes_without_world_commands() {
        let mut game = module_engine_core::GameProject::demo();
        for scene in game.scenes.values_mut() {
            for entity in &mut scene.objects {
                if entity.components.contains_key("player") {
                    entity
                        .components
                        .insert("health".into(), json!({"maximum":5000.,"current":5000.}));
                }
            }
        }
        let mut session = module_engine_core::GameSession::new(game, WorldKind::Objects).unwrap();
        let mut warning = false;
        let mut transitioned = false;
        for index in 0..9000 {
            let input = autopilot(session.state(), index).unwrap();
            session.step(DT, &input).unwrap();
            warning |= !session.state().telegraphs.is_empty() && session.state().scene == "boss";
            transitioned |= session
                .events()
                .iter()
                .any(|e| matches!(e, GameEvent::SceneChanged { .. }));
            if session.state().status == GameStatus::Victory {
                break;
            }
        }
        assert_eq!(
            session.state().status,
            GameStatus::Victory,
            "scene={} enemies={} player={:?}",
            session.state().scene,
            session.state().living_enemies(),
            session
                .state()
                .player()
                .map(|e| e.object.transform.translation)
        );
        assert!(warning && transitioned);
    }
}
