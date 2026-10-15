use super::*;
use crate::components::{Actor, Ai, Health, Melee, Obstacle, Portal};
use crate::queries::{attack_direction, distance, normalized};
use crate::state::read_frame;

type TickFn = fn(&mut SystemContext<'_>, &mut ArpgFrame, &mut Value) -> Result<(), String>;
type BuiltinConfiguration = (
    &'static str,
    TickFn,
    Vec<(&'static str, &'static str, ResourceAccess)>,
    Vec<&'static str>,
);
struct NativeSystem(TickFn);
impl ExecutableSystem for NativeSystem {
    fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
        let mut frame = read_frame(ctx.game)?;
        frame.ensure_tick(ctx.game);
        (self.0)(ctx, &mut frame, state)?;
        ctx.game.set_arpg(frame)
    }
}

pub(crate) fn register_arpg_systems(registry: &mut SystemRegistry) {
    use ResourceAccess::{Read, ReadWrite, Write};
    let configs: [BuiltinConfiguration; 8] = [
        (
            "game.input",
            input_system,
            vec![
                ("world", "GameWorld", ReadWrite),
                ("input", "GameInput", Read),
            ],
            vec![],
        ),
        (
            "game.ai",
            ai_system,
            vec![("world", "GameWorld", ReadWrite)],
            vec![],
        ),
        (
            "game.movement",
            movement_system,
            vec![
                ("world", "GameWorld", ReadWrite),
                ("previous", "Positions", Write),
            ],
            vec![],
        ),
        (
            "game.collision",
            collision_system,
            vec![
                ("world", "GameWorld", ReadWrite),
                ("previous", "Positions", Read),
            ],
            vec![],
        ),
        (
            "game.combat",
            combat_system,
            vec![
                ("world", "GameWorld", ReadWrite),
                ("events", "GameEvents", ReadWrite),
            ],
            vec![],
        ),
        (
            "game.health",
            health_system,
            vec![
                ("world", "GameWorld", ReadWrite),
                ("events", "GameEvents", ReadWrite),
            ],
            vec![],
        ),
        (
            "game.portals",
            portal_system,
            vec![
                ("world", "GameWorld", ReadWrite),
                ("events", "GameEvents", ReadWrite),
            ],
            vec![],
        ),
        (
            "game.audio",
            audio_system,
            vec![("events", "GameEvents", ReadWrite)],
            vec![],
        ),
    ];
    for (id, step, ports, requires) in configs {
        let descriptor = SystemDescriptor {
            id: id.into(),
            state_schema: format!("{id}/1"),
            components: match id {
                "game.input" => vec!["player"],
                "game.ai" => vec!["actor", "ai", "health", "melee"],
                "game.movement" => vec!["actor"],
                "game.collision" => vec!["actor", "obstacle"],
                "game.combat" => vec!["actor", "health", "melee"],
                "game.health" => vec!["health", "player", "boss"],
                "game.portals" => vec!["portal", "persistent", "player", "actor", "health"],
                _ => vec![],
            }
            .into_iter()
            .map(Into::into)
            .collect(),
            ports: ports
                .into_iter()
                .map(|(name, data_type, access)| SystemPort {
                    name: name.into(),
                    data_type: data_type.into(),
                    access,
                })
                .collect(),
            requires: requires.into_iter().map(Into::into).collect(),
        };
        registry.register(descriptor,move|params|{
            if !params.is_null()&&params.as_object().is_none_or(|o|!o.is_empty()){return Err("Built-in system configuration belongs to object components; params must be empty".into());}
            Ok(Box::new(NativeSystem(step)))
        }).expect("Unique built-in system module");
    }
}

fn input_system(
    ctx: &mut SystemContext<'_>,
    frame: &mut ArpgFrame,
    _: &mut Value,
) -> Result<(), String> {
    let mut movement = ctx.input.movement();
    let length = (movement[0] * movement[0] + movement[1] * movement[1]).sqrt();
    if length > 1. {
        movement = normalized(movement);
    }
    for id in ctx.game.world.entities_with(&["player"]) {
        let Some(lifetime) = ctx.game.world.lifetime(id) else {
            continue;
        };
        frame.intentions.insert(
            id,
            GameIntent {
                movement,
                aim: ctx.input.aim_world(),
                attack: ctx.input.attack(),
                lifetime: Some(lifetime),
            },
        );
    }
    Ok(())
}

fn column_component<T: for<'de> Deserialize<'de>>(
    world: &EcsWorld,
    id: u64,
    name: &str,
) -> Option<T> {
    serde_json::from_value(world.component(id, name)?.clone()).ok()
}

fn column_health(world: &EcsWorld, id: u64) -> f32 {
    column_component::<Health>(world, id, "health").map_or(1., |health| health.current)
}

fn ai_system(
    ctx: &mut SystemContext<'_>,
    frame: &mut ArpgFrame,
    _: &mut Value,
) -> Result<(), String> {
    let actors: Vec<_> = ctx
        .game
        .world
        .entities_with(&["actor"])
        .into_iter()
        .filter_map(|id| {
            Some((
                id,
                ctx.game.world.object(id)?.transform.translation,
                column_component::<Actor>(&ctx.game.world, id, "actor")?,
                column_health(&ctx.game.world, id),
            ))
        })
        .collect();
    for id in ctx.game.world.entities_with(&["ai", "actor"]) {
        let (Some(ai), Some(actor)) = (
            column_component::<Ai>(&ctx.game.world, id, "ai"),
            column_component::<Actor>(&ctx.game.world, id, "actor"),
        ) else {
            continue;
        };
        let Some(object) = ctx.game.world.object(id) else {
            continue;
        };
        let pos = object.transform.translation;
        let target = actors
            .iter()
            .filter(|(_, _, a, h)| a.team != actor.team && *h > 0.)
            .min_by(|a, b| distance(pos, a.1).total_cmp(&distance(pos, b.1)));
        let Some((_, target, _, _)) = target else {
            continue;
        };
        let d = distance(pos, *target);
        if d > ai.aggro_radius {
            continue;
        }
        let melee = column_component::<Melee>(&ctx.game.world, id, "melee");
        let attack = melee.as_ref().is_some_and(|m| d < m.range * 0.85);
        let movement = if attack {
            [0., 0.]
        } else {
            normalized([target[0] - pos[0], target[2] - pos[2]])
        };
        let Some(lifetime) = ctx.game.world.lifetime(id) else {
            continue;
        };
        frame.intentions.insert(
            id,
            GameIntent {
                movement,
                aim: *target,
                attack,
                lifetime: Some(lifetime),
            },
        );
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum PreviousPosition {
    /// Previously published Positions resources contained only coordinate arrays.
    Legacy([f32; 3]),
    Current {
        position: [f32; 3],
        lifetime: u64,
    },
}

fn movement_system(
    ctx: &mut SystemContext<'_>,
    frame: &mut ArpgFrame,
    _: &mut Value,
) -> Result<(), String> {
    let mut previous = BTreeMap::new();
    for (id, intent) in &frame.intentions {
        let Some(lifetime) = ctx.game.world.lifetime(*id) else {
            continue;
        };
        if intent.lifetime.is_some_and(|previous| previous != lifetime) {
            continue;
        }
        let Some(actor) = column_component::<Actor>(&ctx.game.world, *id, "actor") else {
            continue;
        };
        let Some(transform) = ctx.game.world.transform_mut(*id) else {
            continue;
        };
        previous.insert(
            *id,
            PreviousPosition::Current {
                position: transform.translation,
                lifetime,
            },
        );
        transform.translation[0] += intent.movement[0] * actor.speed * ctx.dt;
        transform.translation[2] += intent.movement[1] * actor.speed * ctx.dt;
    }
    ctx.set_resource(
        "previous",
        serde_json::to_value(previous).map_err(|e| e.to_string())?,
    )
}

fn collision_system(
    ctx: &mut SystemContext<'_>,
    _frame: &mut ArpgFrame,
    _: &mut Value,
) -> Result<(), String> {
    let previous: BTreeMap<u64, PreviousPosition> =
        serde_json::from_value(ctx.resource("previous")?.clone()).map_err(|e| e.to_string())?;
    let obstacles: Vec<_> = ctx
        .game
        .world
        .entities_with(&["obstacle"])
        .into_iter()
        .filter_map(|id| {
            Some((
                ctx.game.world.object(id)?.transform.translation,
                column_component::<Obstacle>(&ctx.game.world, id, "obstacle")?.half_extents,
            ))
        })
        .collect();
    for (id, previous) in previous {
        let start = match previous {
            PreviousPosition::Legacy(position) => position,
            PreviousPosition::Current { position, lifetime } => {
                if ctx.game.world.lifetime(id) != Some(lifetime) {
                    continue;
                }
                position
            }
        };
        let Some(actor) = column_component::<Actor>(&ctx.game.world, id, "actor") else {
            continue;
        };
        let Some(transform) = ctx.game.world.transform_mut(id) else {
            continue;
        };
        let wanted = transform.translation;
        let mut p = start;
        // Bound movement increments to the body radius to prevent tunnelling,
        // and resolve axes separately so actors slide along blocking geometry.
        let count =
            ((distance(start, wanted) / (actor.radius * 0.5)).ceil() as usize).clamp(1, 128);
        let delta = [
            (wanted[0] - start[0]) / count as f32,
            (wanted[2] - start[2]) / count as f32,
        ];
        let blocks = |p: [f32; 3]| {
            obstacles.iter().any(|(center, extents)| {
                let dx = (p[0] - center[0]).abs() - extents[0];
                let dz = (p[2] - center[2]).abs() - extents[1];
                dx.max(0.).powi(2) + dz.max(0.).powi(2) < actor.radius * actor.radius
            })
        };
        for _ in 0..count {
            let mut next = p;
            next[0] += delta[0];
            if !blocks(next) {
                p = next;
            }
            next = p;
            next[2] += delta[1];
            if !blocks(next) {
                p = next;
            }
        }
        transform.translation = p;
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Swing {
    /// Missing in legacy state; bind it to the current entity once on load.
    #[serde(default)]
    lifetime: Option<u64>,
    cooldown: f32,
    remaining: f32,
    windup: f32,
    direction: [f32; 2],
    hit: Vec<u64>,
    #[serde(default)]
    hit_lifetimes: BTreeMap<u64, u64>,
    active: bool,
}
fn combat_system(
    ctx: &mut SystemContext<'_>,
    frame: &mut ArpgFrame,
    state: &mut Value,
) -> Result<(), String> {
    let mut swings: BTreeMap<u64, Swing> = if state.is_null() {
        BTreeMap::new()
    } else {
        serde_json::from_value(state.clone()).map_err(|e| e.to_string())?
    };
    // Capture eligibility before applying this tick's damage. Simultaneous
    // attackers retain their swing even if an earlier attacker kills them.
    let attackers: Vec<_> = ctx
        .game
        .world
        .entities_with(&["actor", "melee"])
        .into_iter()
        .filter_map(|id| {
            column_component::<Actor>(&ctx.game.world, id, "actor")?;
            Some((
                id,
                ctx.game.world.object(id)?.transform.translation,
                column_component::<Melee>(&ctx.game.world, id, "melee")?,
                column_health(&ctx.game.world, id),
            ))
        })
        .collect();
    for (id, pos, melee, hp) in attackers {
        if hp <= 0. {
            continue;
        }
        let Some(lifetime) = ctx.game.world.lifetime(id) else {
            continue;
        };
        let swing = swings.entry(id).or_default();
        if swing.lifetime.is_some_and(|previous| previous != lifetime) {
            *swing = Swing::default();
        }
        swing.lifetime = Some(lifetime);
        let hit_lifetimes = &mut swing.hit_lifetimes;
        swing.hit.retain(|target| {
            let Some(current) = ctx.game.world.lifetime(*target) else {
                return false;
            };
            *hit_lifetimes.entry(*target).or_insert(current) == current
        });
        swing
            .hit_lifetimes
            .retain(|target, _| swing.hit.contains(target));
        swing.cooldown = (swing.cooldown - ctx.dt).max(0.);
        let intent = frame
            .intentions
            .get(&id)
            .filter(|intent| intent.lifetime.is_none_or(|previous| previous == lifetime))
            .cloned()
            .unwrap_or_default();
        if intent.attack && swing.cooldown <= 0. && !swing.active {
            swing.cooldown = melee.cooldown;
            swing.remaining = melee.duration;
            swing.windup = melee.windup;
            swing.hit.clear();
            swing.hit_lifetimes.clear();
            swing.active = true;
            swing.direction = attack_direction(pos, intent.aim);
            ctx.events.push(ArpgEvent::Attack { attacker: id }.into());
        }
        if !swing.active {
            continue;
        }
        // Attack direction is captured at windup start, giving an evasion window.
        if swing.windup > 0. {
            swing.windup = (swing.windup - ctx.dt).max(0.);
            frame.telegraphs.push(AttackTelegraph {
                attacker: id,
                attacker_lifetime: Some(lifetime),
                attacker_player: Some(ctx.game.world.component(id, "player").is_some()),
                center: pos,
                direction: swing.direction,
                range: melee.range,
                arc: melee.arc,
                progress: 1. - swing.windup / melee.windup.max(1e-6),
            });
            continue;
        }
        let area = MeleeArea {
            center: pos,
            direction: swing.direction,
            range: melee.range,
            arc: melee.arc,
        };
        frame.attack_visuals.push(AttackVisual {
            attacker: id,
            attacker_lifetime: Some(lifetime),
            attacker_player: Some(ctx.game.world.component(id, "player").is_some()),
            area,
            progress: (1. - swing.remaining / melee.duration).clamp(0., 1.),
        });
        let targets = ctx.game.melee_targets(id, &area);
        for target in targets {
            if swing.hit.contains(&target) {
                continue;
            }
            let Some(lifetime) = ctx.game.world.lifetime(target) else {
                continue;
            };
            let Some(position) = ctx
                .game
                .world
                .object(target)
                .map(|object| object.transform.translation)
            else {
                continue;
            };
            let Some(mut hp) = column_component::<Health>(&ctx.game.world, target, "health") else {
                continue;
            };
            hp.current = (hp.current - melee.damage).max(0.);
            let Some(health) = ctx.game.world.component_mut(target, "health") else {
                continue;
            };
            *health = serde_json::to_value(hp).map_err(|e| e.to_string())?;
            swing.hit.push(target);
            swing.hit_lifetimes.insert(target, lifetime);
            frame.hit_visuals.push(HitVisual {
                attacker: id,
                target,
                target_lifetime: Some(lifetime),
                target_player: Some(ctx.game.world.component(target, "player").is_some()),
                position,
                damage: melee.damage,
                started_at: ctx.game.time,
                expires_at: ctx.game.time + 0.35,
            });
            ctx.events.push(
                ArpgEvent::Hit {
                    attacker: id,
                    target,
                    damage: melee.damage,
                }
                .into(),
            );
        }
        swing.remaining -= ctx.dt;
        if swing.remaining <= 0. {
            swing.active = false;
        }
    }
    swings.retain(|id, _| ctx.game.world.contains(*id));
    *state = serde_json::to_value(swings).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum DeathEmission {
    // Existing game.health/1 state remains readable; rewrite it with the
    // current lifetime so subsequent ID reuse can be distinguished.
    Legacy(u64),
    Current { object: u64, lifetime: u64 },
}

/// Verification uses the same private state readers as the native systems.
/// Preserve every logical field and any extra combat data; remove only the
/// process-local incarnation metadata after checking its expected shape.
pub(crate) fn portable_native_state(module: &str, value: &Value) -> Result<Value, String> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    match module {
        "game.combat" => {
            let _: BTreeMap<u64, Swing> = serde_json::from_value(value.clone())
                .map_err(|error| format!("Invalid {module} state: {error}"))?;
            let mut portable = value.clone();
            for swing in portable
                .as_object_mut()
                .expect("Validated swing map")
                .values_mut()
            {
                let swing = swing.as_object_mut().expect("Validated swing record");
                swing.remove("lifetime");
                swing.remove("hit_lifetimes");
            }
            Ok(portable)
        }
        "game.health" => {
            let records: Vec<DeathEmission> = serde_json::from_value(value.clone())
                .map_err(|error| format!("Invalid {module} state: {error}"))?;
            for record in value.as_array().expect("Validated death records") {
                if let Some(record) = record.as_object()
                    && record
                        .keys()
                        .any(|key| key != "object" && key != "lifetime")
                {
                    return Err("Invalid game.health state: unknown death record field".into());
                }
            }
            Ok(Value::Array(
                records
                    .into_iter()
                    .map(|record| {
                        let object = match record {
                            DeathEmission::Legacy(object)
                            | DeathEmission::Current { object, .. } => object,
                        };
                        json!(object)
                    })
                    .collect(),
            ))
        }
        _ => Ok(value.clone()),
    }
}

fn health_system(
    ctx: &mut SystemContext<'_>,
    frame: &mut ArpgFrame,
    state: &mut Value,
) -> Result<(), String> {
    let records: Vec<DeathEmission> = if state.is_null() {
        vec![]
    } else {
        serde_json::from_value(state.clone()).map_err(|e| e.to_string())?
    };
    let mut emitted: BTreeMap<u64, u64> = records
        .into_iter()
        .filter_map(|record| match record {
            DeathEmission::Legacy(object) => ctx
                .game
                .world
                .lifetime(object)
                .map(|lifetime| (object, lifetime)),
            DeathEmission::Current { object, lifetime } => Some((object, lifetime)),
        })
        .collect();
    // Only retained dead entities need duplicate suppression. Removed IDs can
    // be spawned again, and a revived entity can die again in a later tick.
    emitted.retain(|id, lifetime| {
        ctx.game.world.component(*id, "health").is_some()
            && column_health(&ctx.game.world, *id) <= 0.
            && ctx.game.world.lifetime(*id) == Some(*lifetime)
    });
    let dead: Vec<_> = ctx
        .game
        .world
        .entities_with(&["health"])
        .into_iter()
        .filter(|id| column_health(&ctx.game.world, *id) <= 0.)
        // Capture death metadata before removing a parent can remove another
        // dead entity in this same batch.
        .filter_map(|id| {
            Some((
                id,
                ctx.game.world.lifetime(id)?,
                ctx.game.world.component(id, "player").is_some(),
                ctx.game
                    .world
                    .component(id, "boss")
                    .and_then(|v| v.get("victory_on_death"))
                    .and_then(Value::as_bool)
                    == Some(true),
            ))
        })
        .collect();
    for (id, lifetime, player, victory) in dead {
        if emitted.get(&id) == Some(&lifetime) {
            continue;
        }
        emitted.insert(id, lifetime);
        ctx.events.push(ArpgEvent::Death { object: id }.into());
        if player {
            frame.status = GameStatus::Defeat;
            ctx.game.control.halted = true;
        } else {
            if victory {
                frame.status = GameStatus::Victory;
                ctx.game.control.halted = true;
                ctx.events.push(ArpgEvent::Victory.into());
            }
            if ctx.game.world.contains(id) {
                ctx.game.apply_commands(&[WorldCommand::Remove { id }])?;
            }
        }
    }
    emitted.retain(|id, lifetime| ctx.game.world.lifetime(*id) == Some(*lifetime));
    *state = serde_json::to_value(
        emitted
            .into_iter()
            .map(|(object, lifetime)| DeathEmission::Current { object, lifetime })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn portal_system(
    ctx: &mut SystemContext<'_>,
    frame: &mut ArpgFrame,
    _: &mut Value,
) -> Result<(), String> {
    if frame.status != GameStatus::Playing {
        return Ok(());
    }
    let Some(player) = ctx.game.world.entities_with(&["player"]).into_iter().next() else {
        return Ok(());
    };
    let Some(pos) = ctx
        .game
        .world
        .object(player)
        .map(|object| object.transform.translation)
    else {
        return Ok(());
    };
    for id in ctx.game.world.entities_with(&["portal"]) {
        let Some(portal) = column_component::<Portal>(&ctx.game.world, id, "portal") else {
            continue;
        };
        let Some(target) = ctx
            .game
            .world
            .object(id)
            .map(|object| object.transform.translation)
        else {
            continue;
        };
        if distance(pos, target) <= portal.radius
            && (!portal.requires_clear || ctx.game.living_enemies() == 0)
        {
            ctx.game.control.transition = Some(SceneTransition {
                destination: portal.scene,
                carry: ctx
                    .game
                    .world
                    .entities_with(&["persistent"])
                    .into_iter()
                    .collect(),
                spawn: Some(portal.spawn),
            });
            break;
        }
    }
    Ok(())
}

fn audio_system(
    ctx: &mut SystemContext<'_>,
    _frame: &mut ArpgFrame,
    _: &mut Value,
) -> Result<(), String> {
    let sounds: Vec<_> = ctx
        .events
        .iter()
        .filter_map(|event| match ArpgEvent::try_from(event).ok()? {
            ArpgEvent::Attack { attacker } => Some(("attack", Some(attacker))),
            ArpgEvent::Hit { target, .. } => Some(("hit", Some(target))),
            ArpgEvent::Death { object } => Some(("death", Some(object))),
            ArpgEvent::Victory => Some(("victory", None)),
            _ => None,
        })
        .collect();
    for (cue, object) in sounds {
        ctx.events.push(
            ArpgEvent::Sound {
                cue: cue.into(),
                object,
            }
            .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "regressions.rs"]
mod regressions;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArpgEvent as GameEvent;

    fn arpg_events(session: &GameSession) -> Vec<GameEvent> {
        session
            .events()
            .iter()
            .filter_map(|e| ArpgEvent::try_from(e).ok())
            .collect()
    }

    fn actor(id: u64, team: &str, position: [f32; 3], speed: f32) -> crate::AuthoredObject {
        let mut entity = crate::AuthoredObject::new(WorldObject {
            id,
            parent: None,
            transform: crate::Transform {
                translation: position,
                ..Default::default()
            },
            color: [255; 4],
        });
        entity.components.insert(
            "actor".into(),
            json!({"team":team,"radius":0.35,"speed":speed}),
        );
        entity
    }

    fn native_state(objects: Vec<crate::AuthoredObject>) -> GameState {
        let mut project = GameProject::demo();
        project.scenes.get_mut("arena").unwrap().objects = objects;
        crate::session(project, WorldKind::Ecs)
            .unwrap()
            .state()
            .clone()
    }

    fn native_tick(game: &mut GameState, state: &mut Value, tick: TickFn) -> Vec<GameEvent> {
        let mut resources = crate::ResourceStore::new();
        let mut events = vec![];
        let bindings = BTreeMap::new();
        let input = GameInput::default();
        let mut context = SystemContext {
            game,
            resources: &mut resources,
            events: &mut events,
            dt: 1. / 60.,
            input: &input,
            bindings: &bindings,
        };
        let mut frame = context.game.arpg();
        tick(&mut context, &mut frame, state).unwrap();
        context.game.set_arpg(frame).unwrap();
        events
            .into_iter()
            .map(|event| ArpgEvent::try_from(event).unwrap())
            .collect()
    }

    fn reuse_in_one_packet(game: &mut GameState, entity: GameEntity) {
        let id = entity.object.id;
        let previous = game.world.lifetime(id).unwrap();
        game.apply_commands(&[
            WorldCommand::Remove { id },
            WorldCommand::Spawn {
                object: entity.object.clone(),
            },
        ])
        .unwrap();
        game.world.upsert_entity(entity).unwrap();
        assert_ne!(game.world.lifetime(id), Some(previous));
    }

    fn attacking_player() -> crate::AuthoredObject {
        let mut player = actor(1, "player", [0.; 3], 0.);
        player.components.insert("player".into(), json!({}));
        player.components.insert("melee".into(), json!({"range":2.,"damage":7.,"cooldown":1.,"arc":std::f32::consts::TAU,"duration":1.,"windup":0.}));
        player
    }

    fn attack_intent(game: &mut GameState) {
        let mut frame = game.arpg();
        frame.intentions.insert(
            1,
            GameIntent {
                attack: true,
                aim: [0., 0., 1.],
                ..Default::default()
            },
        );
        game.set_arpg(frame).unwrap();
    }

    struct Reincarnate(crate::GameEntity);
    impl ExecutableSystem for Reincarnate {
        fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            if state.as_bool() == Some(true) {
                return Ok(());
            }
            ctx.game.apply_commands(&[
                WorldCommand::Remove {
                    id: self.0.object.id,
                },
                WorldCommand::Spawn {
                    object: self.0.object.clone(),
                },
            ])?;
            ctx.game.world.upsert_entity(self.0.clone())?;
            *state = json!(true);
            Ok(())
        }
    }

    fn session_with_reincarnation(
        kind: WorldKind,
        at_stage: usize,
        objects: Vec<crate::AuthoredObject>,
        fresh: crate::AuthoredObject,
    ) -> GameSession {
        let mut project = GameProject::demo();
        project.scenes.get_mut("arena").unwrap().objects = objects;
        let entity = GameEntity {
            object: fresh.object,
            components: fresh.components,
            references: fresh.references,
        };
        let mut registry = crate::registry();
        registry
            .register(
                SystemDescriptor {
                    id: "test.reincarnate".into(),
                    state_schema: "1".into(),
                    components: vec![],
                    ports: vec![crate::SystemPort {
                        name: "world".into(),
                        data_type: "GameWorld".into(),
                        access: ResourceAccess::ReadWrite,
                    }],
                    requires: vec![],
                },
                move |_| Ok(Box::new(Reincarnate(entity.clone()))),
            )
            .unwrap();
        project.execution.stages.insert(
            at_stage,
            crate::SystemStage {
                name: "Reincarnate".into(),
                nodes: vec![crate::SystemNode {
                    id: "reincarnate".into(),
                    module: "test.reincarnate".into(),
                    params: json!({}),
                    bindings: BTreeMap::from([("world".into(), "world".into())]),
                    after: vec![],
                }],
            },
        );
        GameSession::new_with_registry(project, kind, registry).unwrap()
    }

    #[test]
    fn authored_reincarnation_after_input_cannot_move_or_attack_using_the_removed_players_intent() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut player = attacking_player();
            player.components.get_mut("actor").unwrap()["speed"] = json!(3.);
            let mut fresh = player.clone();
            fresh.components.remove("player");
            fresh.object.transform.translation = [9., 0., 4.];
            let mut session = session_with_reincarnation(kind, 1, vec![player], fresh);
            session
                .step(
                    1. / 60.,
                    &(ArpgInput {
                        movement: [1., 0.],
                        attack: true,
                        aim_world: [20., 0., 4.],
                        ..Default::default()
                    })
                    .into(),
                )
                .unwrap();
            assert_eq!(
                session
                    .state()
                    .world
                    .object(1)
                    .unwrap()
                    .transform
                    .translation,
                [9., 0., 4.]
            );
            assert!(session.state().world.component(1, "player").is_none());
            assert!(
                arpg_events(&session)
                    .iter()
                    .all(|event| !matches!(event, GameEvent::Attack { .. }))
            );
            assert!(session.state().arpg().attack_visuals.is_empty());
            assert_ne!(
                session.state().arpg().intentions[&1].lifetime,
                session.state().world.lifetime(1)
            );
        }
    }

    #[test]
    fn authored_reincarnation_after_ai_cannot_consume_the_removed_actors_intent() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut npc = attacking_player();
            npc.components.remove("player");
            npc.components.get_mut("actor").unwrap()["team"] = json!("enemy");
            npc.components.get_mut("actor").unwrap()["speed"] = json!(3.);
            npc.components.insert("ai".into(), json!({}));
            let target = actor(2, "player", [3., 0., 0.], 0.);
            let mut fresh = npc.clone();
            fresh.components.remove("ai");
            fresh.object.transform.translation = [9., 0., 4.];
            let mut session = session_with_reincarnation(kind, 2, vec![npc, target], fresh);
            session.step(1. / 60., &GameInput::default()).unwrap();
            assert_eq!(
                session
                    .state()
                    .world
                    .object(1)
                    .unwrap()
                    .transform
                    .translation,
                [9., 0., 4.]
            );
            assert_ne!(
                session.state().arpg().intentions[&1].lifetime,
                session.state().world.lifetime(1)
            );
        }
    }

    #[test]
    fn authored_reincarnation_after_movement_does_not_collide_along_the_removed_actors_path() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut player = attacking_player();
            player.components.get_mut("actor").unwrap()["speed"] = json!(3.);
            let mut fresh = player.clone();
            fresh.components.remove("player");
            fresh.object.transform.translation = [2., 0., 0.];
            let mut obstacle = crate::AuthoredObject::new(WorldObject {
                id: 2,
                parent: None,
                transform: crate::Transform {
                    translation: [1., 0., 0.],
                    ..Default::default()
                },
                color: [255; 4],
            });
            obstacle
                .components
                .insert("obstacle".into(), json!({"half_extents":[0.25,0.25]}));
            let mut session = session_with_reincarnation(kind, 3, vec![player, obstacle], fresh);
            session
                .step(
                    1. / 60.,
                    &(ArpgInput {
                        movement: [1., 0.],
                        ..Default::default()
                    })
                    .into(),
                )
                .unwrap();
            assert_eq!(
                session
                    .state()
                    .world
                    .object(1)
                    .unwrap()
                    .transform
                    .translation,
                [2., 0., 0.]
            );
        }
    }

    #[test]
    fn collision_reads_legacy_positions_and_movement_accepts_legacy_unstamped_intentions() {
        let player = actor(1, "player", [2., 0., 0.], 3.);
        let mut obstacle = crate::AuthoredObject::new(WorldObject {
            id: 2,
            parent: None,
            transform: crate::Transform {
                translation: [1., 0., 0.],
                ..Default::default()
            },
            color: [255; 4],
        });
        obstacle
            .components
            .insert("obstacle".into(), json!({"half_extents":[0.25,0.25]}));
        let mut game = native_state(vec![player, obstacle]);
        let mut resources = crate::ResourceStore::from([(
            "positions".into(),
            crate::ResourceValue {
                data_type: "Positions".into(),
                value: json!({"1":[0.,0.,0.]}),
            },
        )]);
        let bindings = BTreeMap::from([("previous".into(), "positions".into())]);
        let input = GameInput::default();
        let mut events = vec![];
        let mut frame = game.arpg();
        collision_system(
            &mut SystemContext {
                game: &mut game,
                resources: &mut resources,
                events: &mut events,
                dt: 1. / 60.,
                input: &input,
                bindings: &bindings,
            },
            &mut frame,
            &mut Value::Null,
        )
        .unwrap();
        let stopped = game.world.object(1).unwrap().transform.translation[0];
        assert!(stopped < 1.);
        frame.intentions.insert(
            1,
            GameIntent {
                movement: [1., 0.],
                ..Default::default()
            },
        );
        movement_system(
            &mut SystemContext {
                game: &mut game,
                resources: &mut resources,
                events: &mut events,
                dt: 1. / 60.,
                input: &input,
                bindings: &bindings,
            },
            &mut frame,
            &mut Value::Null,
        )
        .unwrap();
        assert!(
            (game.world.object(1).unwrap().transform.translation[0] - stopped - 0.05).abs()
                < 0.000001
        );
        assert_eq!(
            resources["positions"].value["1"]["lifetime"].as_u64(),
            game.world.lifetime(1)
        );
    }

    #[test]
    fn combat_same_packet_reuse_resets_cooldown_but_transform_upsert_and_legacy_state_do_not() {
        let mut game = native_state(vec![attacking_player()]);
        attack_intent(&mut game);
        let mut state = Value::Null;
        assert!(
            native_tick(&mut game, &mut state, combat_system)
                .contains(&GameEvent::Attack { attacker: 1 })
        );
        let lifetime = game.world.lifetime(1);
        game.world.transform_mut(1).unwrap().translation[0] = 0.25;
        let entity = game.world.entity(1).unwrap();
        game.world.upsert_entity(entity.clone()).unwrap();
        assert_eq!(game.world.lifetime(1), lifetime);
        // The previous implementation did not write lifetime bookkeeping.
        state["1"].as_object_mut().unwrap().remove("lifetime");
        state["1"].as_object_mut().unwrap().remove("hit_lifetimes");
        assert!(
            !native_tick(&mut game, &mut state, combat_system)
                .contains(&GameEvent::Attack { attacker: 1 })
        );
        reuse_in_one_packet(&mut game, entity);
        assert!(
            native_tick(&mut game, &mut state, combat_system)
                .contains(&GameEvent::Attack { attacker: 1 })
        );
    }

    #[test]
    fn active_swing_can_hit_a_fresh_target_reusing_an_already_hit_id() {
        let mut target = actor(2, "enemy", [0., 0., 1.], 0.);
        target
            .components
            .insert("health".into(), json!({"maximum":100.,"current":100.}));
        let mut game = native_state(vec![attacking_player(), target]);
        let fresh_target = game.world.entity(2).unwrap();
        let previous_lifetime = game.world.lifetime(2);
        attack_intent(&mut game);
        let mut state = Value::Null;
        let hit = GameEvent::Hit {
            attacker: 1,
            target: 2,
            damage: 7.,
        };
        assert!(native_tick(&mut game, &mut state, combat_system).contains(&hit));
        assert_eq!(
            game.arpg().hit_visuals[0].target_lifetime,
            previous_lifetime
        );
        let same_target = game.world.entity(2).unwrap();
        game.world.upsert_entity(same_target).unwrap();
        assert!(!native_tick(&mut game, &mut state, combat_system).contains(&hit));
        reuse_in_one_packet(&mut game, fresh_target);
        assert!(native_tick(&mut game, &mut state, combat_system).contains(&hit));
        assert_ne!(game.world.lifetime(2), previous_lifetime);
        assert_eq!(
            game.arpg().hit_visuals.last().unwrap().target_lifetime,
            game.world.lifetime(2)
        );
        assert_eq!(
            game.world.component(2, "health").unwrap()["current"],
            json!(93.)
        );
    }

    #[test]
    fn health_same_packet_reuse_emits_again_and_legacy_dead_ids_remain_suppressed() {
        let mut player = attacking_player();
        player
            .components
            .insert("health".into(), json!({"maximum":1.,"current":0.}));
        let mut game = native_state(vec![player]);
        let mut state = Value::Null;
        let death = GameEvent::Death { object: 1 };
        assert_eq!(
            native_tick(&mut game, &mut state, health_system),
            vec![death.clone()]
        );
        assert!(native_tick(&mut game, &mut state, health_system).is_empty());
        let entity = game.world.entity(1).unwrap();
        let lifetime = game.world.lifetime(1);
        game.world.upsert_entity(entity.clone()).unwrap();
        assert_eq!(game.world.lifetime(1), lifetime);
        state = json!([1]);
        assert!(native_tick(&mut game, &mut state, health_system).is_empty());
        assert_eq!(state[0]["object"], json!(1));
        assert_eq!(state[0]["lifetime"], json!(lifetime.unwrap()));
        reuse_in_one_packet(&mut game, entity);
        assert_eq!(
            native_tick(&mut game, &mut state, health_system),
            vec![death]
        );
        assert!(native_tick(&mut game, &mut state, health_system).is_empty());
    }

    #[test]
    fn sparse_actor_queries_preserve_ai_and_movement_without_health_components() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut player = actor(1, "player", [0., 0., 0.], 3.);
            player.components.insert("player".into(), json!({}));
            let mut enemy = actor(2, "enemy", [3., 0., 0.], 1.);
            enemy.components.insert("ai".into(), json!({}));
            let decoration = crate::AuthoredObject::new(WorldObject {
                id: 9,
                parent: None,
                transform: crate::Transform {
                    translation: [10., 0., 0.],
                    ..Default::default()
                },
                color: [255; 4],
            });
            let mut project = GameProject::demo();
            project.scenes.get_mut("arena").unwrap().objects = vec![player, enemy, decoration];
            let mut session = crate::session(project, kind).unwrap();
            session
                .step(
                    1. / 60.,
                    &(ArpgInput {
                        movement: [1., 0.],
                        ..Default::default()
                    })
                    .into(),
                )
                .unwrap();
            let state = session.state();
            assert_eq!(state.world.entities_with(&["actor"]), vec![1, 2]);
            assert!(state.world.entities_with(&["actor", "health"]).is_empty());
            assert!(
                (state.world.object(1).unwrap().transform.translation[0] - 0.05).abs() < 0.000001
            );
            assert!(
                (state.world.object(2).unwrap().transform.translation[0] - (3. - 1. / 60.)).abs()
                    < 0.000001
            );
            assert_eq!(
                state.world.object(9).unwrap().transform.translation,
                [10., 0., 0.]
            );
            assert_eq!(
                state.arpg().intentions.keys().copied().collect::<Vec<_>>(),
                vec![1, 2]
            );
        }
    }

    #[test]
    fn combat_column_updates_preserve_simultaneous_lethal_swing_eligibility() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut player = actor(1, "player", [0., 0., 0.], 0.);
            player.components.insert("player".into(), json!({}));
            let mut enemy = actor(2, "enemy", [0., 0., 1.], 0.);
            enemy.components.insert("ai".into(), json!({}));
            for entity in [&mut player, &mut enemy] {
                entity
                    .components
                    .insert("health".into(), json!({"maximum":1.,"current":1.}));
                entity.components.insert("melee".into(), json!({
                    "range":2.,"damage":2.,"cooldown":1.,"arc":std::f32::consts::TAU,"duration":0.1,"windup":0.,
                }));
            }
            let mut project = GameProject::demo();
            project.scenes.get_mut("arena").unwrap().objects = vec![player, enemy];
            let mut session = crate::session(project, kind).unwrap();
            session
                .step(
                    1. / 60.,
                    &(ArpgInput {
                        aim_world: [0., 0., 1.],
                        attack: true,
                        ..Default::default()
                    })
                    .into(),
                )
                .unwrap();
            assert_eq!(session.state().arpg().status, GameStatus::Defeat);
            assert_eq!(
                session.state().world.component(1, "health").unwrap()["current"],
                json!(0.)
            );
            assert!(!session.state().world.contains(2));
            for (attacker, target) in [(1, 2), (2, 1)] {
                assert!(arpg_events(&session).iter().any(|event| matches!(event, GameEvent::Hit { attacker: actual_attacker, target: actual_target, .. } if (*actual_attacker, *actual_target) == (attacker, target))));
                assert!(arpg_events(&session).contains(&GameEvent::Death { object: target }));
                let frame = session.state().arpg();
                let visual = frame
                    .attack_visuals
                    .iter()
                    .find(|visual| visual.attacker == attacker)
                    .unwrap();
                assert_eq!(visual.attacker_player, Some(attacker == 1));
                assert!(visual.attacker_lifetime.is_some());
                let hit = frame
                    .hit_visuals
                    .iter()
                    .find(|visual| visual.target == target)
                    .unwrap();
                assert_eq!(hit.target_player, Some(target == 1));
            }
        }
    }

    struct SourceControllerBeforeMovement;
    impl ExecutableSystem for SourceControllerBeforeMovement {
        fn initialize(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            if !ctx.game.module_data.contains_key(FRAME_KEY) {
                return Err("ARPG preparation must precede source initialization".into());
            }
            Ok(())
        }
        fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            let mut frame = ctx.game.arpg();
            if !frame.intentions.is_empty() {
                return Err("Source controller observed previous-tick intentions".into());
            }
            frame.intentions.insert(
                1,
                GameIntent {
                    movement: [1., 0.],
                    lifetime: ctx.game.world.lifetime(1),
                    ..Default::default()
                },
            );
            *state = json!(state.as_u64().unwrap_or_default() + 1);
            ctx.game.set_arpg(frame)
        }
    }

    #[test]
    fn source_controller_before_movement_keeps_fresh_intent_and_sees_prepared_frame() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut project = demo_project();
            project.scenes.get_mut("arena").unwrap().objects =
                vec![actor(1, "source", [0.; 3], 3.)];
            project.execution.stages.retain_mut(|stage| {
                stage.nodes.retain_mut(|node| {
                    node.after.clear();
                    node.module == "game.movement"
                });
                !stage.nodes.is_empty()
            });
            project.execution.stages.insert(
                0,
                SystemStage {
                    name: "Source controller".into(),
                    nodes: vec![SystemNode {
                        id: "source.controller".into(),
                        module: "source.controller".into(),
                        params: Value::Null,
                        bindings: BTreeMap::from([("world".into(), "world".into())]),
                        after: vec![],
                    }],
                },
            );
            let mut registry = crate::registry();
            registry
                .register(
                    SystemDescriptor {
                        id: "source.controller".into(),
                        state_schema: "source.controller/1".into(),
                        components: vec!["actor".into()],
                        ports: vec![SystemPort {
                            name: "world".into(),
                            data_type: "GameWorld".into(),
                            access: ResourceAccess::ReadWrite,
                        }],
                        requires: vec![],
                    },
                    |_| Ok(Box::new(SourceControllerBeforeMovement)),
                )
                .unwrap();
            let mut session = GameSession::new_with_registry(project, kind, registry).unwrap();
            session.step(1. / 60., &GameInput::default()).unwrap();
            session.step(1. / 60., &GameInput::default()).unwrap();
            assert!(
                (session
                    .state()
                    .world
                    .object(1)
                    .unwrap()
                    .transform
                    .translation[0]
                    - 0.1)
                    .abs()
                    < 0.000001
            );
            assert_eq!(session.system_states()["source.controller"], json!(2));
        }
    }

    struct BeginTickProbe(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl ExecutableSystem for BeginTickProbe {
        fn initialize(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *state = json!(state.as_u64().unwrap_or_default() + 1);
            let value = ctx.resource("scratch")?.as_u64().unwrap();
            ctx.set_resource("scratch", json!(value + 1))?;
            ctx.events.push(module_engine_core::GameEvent::new(
                "source.probed",
                json!({"count":value+1}),
            ));
            Ok(())
        }
    }

    #[test]
    fn failed_begin_tick_rolls_back_before_entering_initialized_systems() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let factory_calls = calls.clone();
        let mut registry = SystemRegistry::builtin();
        registry
            .register(
                SystemDescriptor {
                    id: "source.probe".into(),
                    state_schema: "source.probe/1".into(),
                    components: vec![],
                    ports: vec![SystemPort {
                        name: "scratch".into(),
                        data_type: "Json".into(),
                        access: ResourceAccess::ReadWrite,
                    }],
                    requires: vec![],
                },
                move |_| Ok(Box::new(BeginTickProbe(factory_calls.clone()))),
            )
            .unwrap();
        let mut contribution = host_contribution();
        contribution.id = "source.preparation".into();
        contribution.activation_modules = BTreeSet::from(["source.probe".into()]);
        contribution.begin_tick = |game| {
            crate::state::begin_tick(game)?;
            if game.module_data.get("fail_begin") == Some(&json!(true)) {
                game.world.clear();
                game.module_data.insert("corruption".into(), json!(true));
                return Err("Source preparation failed".into());
            }
            Ok(())
        };
        registry.register_game_contribution(contribution).unwrap();
        let graph = ExecutionGraph {
            resources: vec![SystemResource {
                name: "scratch".into(),
                data_type: "Json".into(),
                external: true,
                initial: json!(0),
            }],
            stages: vec![SystemStage {
                name: "Probe".into(),
                nodes: vec![SystemNode {
                    id: "probe".into(),
                    module: "source.probe".into(),
                    params: Value::Null,
                    bindings: BTreeMap::from([("scratch".into(), "scratch".into())]),
                    after: vec![],
                }],
            }],
        };
        let mut executor = SystemExecutor::new(&graph, &registry).unwrap();
        let mut game = native_state(vec![actor(1, "source", [0.; 3], 3.)]);
        let mut events = vec![];
        executor
            .step(&mut game, &GameInput::default(), 1. / 60., &mut events)
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        game.module_data.insert("fail_begin".into(), json!(true));
        let before = game.clone();
        let lifetime = game.world.lifetime(1);
        let states = executor.states();
        let resources = executor.resources().clone();
        let before_events = events.clone();
        assert!(
            executor
                .step(&mut game, &GameInput::default(), 1. / 60., &mut events)
                .unwrap_err()
                .contains("Source preparation failed")
        );
        assert_eq!(game, before);
        assert_eq!(game.world.lifetime(1), lifetime);
        assert_eq!(executor.states(), states);
        assert_eq!(executor.resources(), &resources);
        assert_eq!(events, before_events);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        game.module_data.insert("fail_begin".into(), json!(false));
        executor
            .step(&mut game, &GameInput::default(), 1. / 60., &mut events)
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "Preparation failure reinitialized a retained implementation"
        );
    }
}
