//! Fixed-step, provider-neutral gameplay. Behaviors are separately registered
//! systems; scene names, encounters and progression are authored data.
use crate::{
    EcsWorld, ExecutableSystem, GameEntity, GameProject, ObjectsWorld, ResourceAccess,
    SceneSnapshot, SystemContext, SystemDescriptor, SystemExecutor, SystemPort, SystemRegistry,
    TickTransaction, WorldCommand, WorldKind, WorldObject, WorldProvider,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Host-owned action input; attack/interact/restart are pressed edges.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameInput {
    pub movement: [f32; 2],
    pub aim_world: [f32; 3],
    pub attack: bool,
    pub interact: bool,
    pub restart: bool,
}

/// Portable events read by game UI, audio and custom modules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GameEvent {
    Attack {
        attacker: u64,
    },
    Hit {
        attacker: u64,
        target: u64,
        damage: f32,
    },
    Death {
        object: u64,
    },
    SceneChanged {
        from: String,
        to: String,
    },
    Victory,
    Sound {
        cue: String,
        object: Option<u64>,
    },
    Custom {
        name: String,
        payload: Value,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameStatus {
    #[default]
    Playing,
    Defeat,
    Victory,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameIntent {
    pub movement: [f32; 2],
    pub aim: [f32; 3],
    pub attack: bool,
}

/// World-space telegraph for the UI/renderer. Filled from actual combat state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttackTelegraph {
    pub attacker: u64,
    pub center: [f32; 3],
    pub direction: [f32; 2],
    pub range: f32,
    pub arc: f32,
    pub progress: f32,
}

/// The exact world-space melee region used by combat and an aim preview.
///
/// Target circles extend radial reach by their radius. The angular test uses
/// the target's center; it does not widen the sector by the target's radius.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeleeArea {
    pub center: [f32; 3],
    pub direction: [f32; 2],
    pub range: f32,
    pub arc: f32,
}

impl MeleeArea {
    /// Keep presentation and damage eligibility on the same XZ-plane test.
    pub fn contains(&self, position: [f32; 3], radius: f32) -> bool {
        let delta = normalized([position[0] - self.center[0], position[2] - self.center[2]]);
        let dot = delta[0] * self.direction[0] + delta[1] * self.direction[1];
        distance(self.center, position) <= self.range + radius
            && (self.arc >= std::f32::consts::TAU || dot >= (self.arc * 0.5).cos())
    }
}

/// Presentation of an actual damage-active swing, rather than a pressed edge.
/// Direction is captured when the attack starts and progress spans its duration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttackVisual {
    pub attacker: u64,
    pub area: MeleeArea,
    pub progress: f32,
}

/// A confirmed hit retained for 0.35 simulated seconds, including lethal hits.
/// Position is captured before the health system can remove the target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HitVisual {
    pub attacker: u64,
    pub target: u64,
    pub position: [f32; 3],
    pub damage: f32,
    pub started_at: f64,
    pub expires_at: f64,
}

/// Mutable Play state; none of these values write back to the authored project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameState {
    pub scene: String,
    pub time: f64,
    pub tick: u64,
    pub status: GameStatus,
    pub entities: BTreeMap<u64, GameEntity>,
    #[serde(default)]
    pub intentions: BTreeMap<u64, GameIntent>,
    #[serde(default)]
    pub telegraphs: Vec<AttackTelegraph>,
    #[serde(default)]
    pub attack_visuals: Vec<AttackVisual>,
    #[serde(default)]
    pub hit_visuals: Vec<HitVisual>,
    pub transition: Option<String>,
    #[serde(default)]
    pub transition_spawn: Option<[f32; 3]>,
}

impl GameState {
    /// All-or-none common commands, including subtree deletion and spawn.
    pub fn apply_commands(&mut self, commands: &[WorldCommand]) -> Result<(), String> {
        let mut objects = self
            .entities
            .iter()
            .map(|(id, e)| (*id, e.object.clone()))
            .collect();
        crate::apply_object_commands(&mut objects, commands)?;
        self.entities.retain(|id, _| objects.contains_key(id));
        for (id, object) in objects {
            if let Some(entity) = self.entities.get_mut(&id) {
                entity.object = object;
            } else {
                self.entities.insert(
                    id,
                    GameEntity {
                        object,
                        components: BTreeMap::new(),
                        references: BTreeMap::new(),
                    },
                );
            }
        }
        Ok(())
    }
    pub fn player(&self) -> Option<&GameEntity> {
        self.entities
            .values()
            .find(|e| e.components.contains_key("player"))
    }
    pub fn living_enemies(&self) -> usize {
        self.entities
            .values()
            .filter(|e| {
                component::<Actor>(e, "actor").is_some_and(|a| a.team == "enemy") && health(e) > 0.
            })
            .count()
    }
    /// Prospective attack region, using the same aim fallback as a real swing.
    pub fn melee_aim(&self, attacker: u64, aim_world: [f32; 3]) -> Option<MeleeArea> {
        let entity = self.entities.get(&attacker)?;
        component::<Actor>(entity, "actor")?;
        let melee = component::<Melee>(entity, "melee")?;
        if health(entity) <= 0. {
            return None;
        }
        let center = entity.object.transform.translation;
        Some(MeleeArea {
            center,
            direction: attack_direction(center, aim_world),
            range: melee.range,
            arc: melee.arc,
        })
    }
    /// Living opposing targets eligible for a fresh swing in the supplied area.
    pub fn melee_targets(&self, attacker: u64, area: &MeleeArea) -> Vec<u64> {
        let Some(actor) = self
            .entities
            .get(&attacker)
            .and_then(|e| component::<Actor>(e, "actor"))
        else {
            return vec![];
        };
        self.entities
            .values()
            .filter_map(|target| {
                let other = component::<Actor>(target, "actor")?;
                let hp = component::<Health>(target, "health")?;
                (target.object.id != attacker
                    && other.team != actor.team
                    && hp.current > 0.
                    && area.contains(target.object.transform.translation, other.radius))
                .then_some(target.object.id)
            })
            .collect()
    }
}

/// Independent fixed simulation clock and a selected actual world provider.
pub struct GameSession {
    project: GameProject,
    kind: WorldKind,
    state: GameState,
    executor: SystemExecutor,
    registry: SystemRegistry,
    provider: Box<dyn WorldProvider>,
    events: Vec<GameEvent>,
    accumulator: f64,
    pending_attack: bool,
    pending_interact: bool,
    world_factory: GameWorldFactory,
    active_components: BTreeSet<String>,
}

type GameWorldFactory =
    Arc<dyn Fn(Vec<WorldObject>) -> Result<Box<dyn WorldProvider>, String> + Send + Sync>;

impl Drop for GameSession {
    fn drop(&mut self) {
        // Editor Stop and Player exit drop their host/session. Hooks are
        // idempotently disabled by SystemExecutor::stop; Drop cannot return an
        // error, so shutdown attempts every initialized instance best-effort.
        let _ = self.executor.stop(&mut self.state, &mut self.events);
    }
}

impl GameSession {
    pub fn new(project: GameProject, kind: WorldKind) -> Result<Self, String> {
        Self::new_with_registry(project, kind, SystemRegistry::builtin())
    }
    pub fn new_with_registry(
        project: GameProject,
        kind: WorldKind,
        registry: SystemRegistry,
    ) -> Result<Self, String> {
        Self::build(
            project,
            kind,
            registry,
            Arc::new(move |objects| make_provider(kind, objects)),
        )
    }
    /// Install a third independently stored editable world from application
    /// source. Neither the game systems nor the renderer need backend changes.
    pub fn new_with_world_factory<F>(
        project: GameProject,
        kind: WorldKind,
        registry: SystemRegistry,
        factory: F,
    ) -> Result<Self, String>
    where
        F: Fn(Vec<WorldObject>) -> Result<Box<dyn WorldProvider>, String> + Send + Sync + 'static,
    {
        Self::build(project, kind, registry, Arc::new(factory))
    }
    fn build(
        project: GameProject,
        kind: WorldKind,
        registry: SystemRegistry,
        world_factory: GameWorldFactory,
    ) -> Result<Self, String> {
        project.validate_with_registry(&registry)?;
        let active_components = registry.active_components(&project.execution)?;
        let state = Self::load_state(&project, &project.entry_scene, 0., 0, &active_components)?;
        let provider = Self::load_provider(&world_factory, &state)?;
        let executor = SystemExecutor::new(&project.execution, &registry)?;
        Ok(Self {
            project,
            kind,
            state,
            executor,
            registry,
            provider,
            events: vec![],
            accumulator: 0.,
            pending_attack: false,
            pending_interact: false,
            world_factory,
            active_components,
        })
    }
    fn load_provider(
        world_factory: &GameWorldFactory,
        state: &GameState,
    ) -> Result<Box<dyn WorldProvider>, String> {
        let objects: Vec<_> = state.entities.values().map(|e| e.object.clone()).collect();
        crate::validate_world_commands(&objects, &[])?;
        let provider = world_factory(objects.clone())?;
        let loaded = provider.objects()?;
        crate::validate_world_commands(&loaded, &[])?;
        if object_map(&loaded) != object_map(&objects) {
            return Err("World factory did not load the supplied authored objects".into());
        }
        Ok(provider)
    }
    fn load_state(
        project: &GameProject,
        id: &str,
        time: f64,
        tick: u64,
        active_components: &BTreeSet<String>,
    ) -> Result<GameState, String> {
        Ok(GameState {
            scene: id.into(),
            time,
            tick,
            status: GameStatus::Playing,
            entities: project
                .resolve_scene(id)?
                .into_iter()
                .map(|mut e| {
                    e.components
                        .retain(|name, _| active_components.contains(name));
                    (e.object.id, e)
                })
                .collect(),
            intentions: BTreeMap::new(),
            telegraphs: vec![],
            attack_visuals: vec![],
            hit_visuals: vec![],
            transition: None,
            transition_spawn: None,
        })
    }
    pub fn project(&self) -> &GameProject {
        &self.project
    }
    pub fn state(&self) -> &GameState {
        &self.state
    }
    pub fn events(&self) -> &[GameEvent] {
        &self.events
    }
    pub fn system_states(&self) -> BTreeMap<String, Value> {
        self.executor.states()
    }
    pub fn world_objects(&self) -> Vec<WorldObject> {
        self.state
            .entities
            .values()
            .map(|e| e.object.clone())
            .collect()
    }
    pub fn asset_bindings(&self) -> BTreeMap<u64, String> {
        self.state
            .entities
            .iter()
            .filter_map(|(id, e)| {
                e.components
                    .get("mesh")?
                    .get("path")?
                    .as_str()
                    .map(|p| (*id, p.into()))
            })
            .collect()
    }
    pub fn snapshot(&self) -> SceneSnapshot {
        let mut snapshot = self.provider.snapshot();
        snapshot.time = self.state.time as f32;
        snapshot
    }
    pub fn world_kind(&self) -> WorldKind {
        self.kind
    }
    /// Useful for host Custom packets: components remain with stable object IDs.
    pub fn apply_commands(&mut self, commands: &[WorldCommand]) -> Result<(), String> {
        let mut candidate = self.state.clone();
        candidate.apply_commands(commands)?;
        let mut transaction = WorldTickTransaction::new(self.provider.as_mut())?;
        transaction.validate(&candidate)?;
        if let Err(error) = transaction.commit(&candidate) {
            transaction.rollback()?;
            return Err(error);
        }
        self.state = candidate;
        Ok(())
    }
    /// Advance in 1/60s increments independently of render cadence. Catchup is
    /// capped to 15 ticks per call; excess wall time is discarded explicitly.
    /// Pressed edges are retained across a frame shorter than one fixed tick.
    pub fn step(&mut self, dt: f64, input: &GameInput) -> Result<usize, String> {
        self.step_budget(dt, input, 15)
    }
    /// Interactive owners can yield between fixed ticks without discarding
    /// the remaining accumulator. The existing 0.25 s wall-time cap still
    /// applies; this changes scheduling, not the duration of a simulation tick.
    pub fn step_budget(
        &mut self,
        dt: f64,
        input: &GameInput,
        max_ticks: usize,
    ) -> Result<usize, String> {
        if !dt.is_finite()
            || dt < 0.
            || !input
                .movement
                .iter()
                .chain(&input.aim_world)
                .all(|v| v.is_finite())
        {
            return Err("Game dt/input must be finite and dt nonnegative".into());
        }
        self.events.clear();
        if input.restart {
            self.restart()?;
            return Ok(0);
        }
        if self.state.status != GameStatus::Playing {
            return Ok(0);
        }
        self.pending_attack |= input.attack;
        self.pending_interact |= input.interact;
        self.accumulator = (self.accumulator + dt).min(0.25);
        let mut ticks = 0;
        while ticks < max_ticks && self.accumulator + 1e-12 >= 1. / 60. {
            let mut fixed_input = input.clone();
            fixed_input.attack = self.pending_attack;
            fixed_input.interact = self.pending_interact;
            self.pending_attack = false;
            self.pending_interact = false;
            let before_tick = self.state.clone();
            let mut transaction = match WorldTickTransaction::new(self.provider.as_mut()) {
                Ok(transaction) => transaction,
                Err(error) => {
                    self.pending_attack |= fixed_input.attack;
                    self.pending_interact |= fixed_input.interact;
                    return Err(error);
                }
            };
            self.state.intentions.clear();
            self.state.telegraphs.clear();
            self.state.attack_visuals.clear();
            self.state
                .hit_visuals
                .retain(|hit| hit.expires_at > self.state.time);
            let mut tick_events = vec![];
            if let Err(error) = self.executor.step_with_transaction(
                &mut self.state,
                &fixed_input,
                1. / 60.,
                &mut tick_events,
                &mut transaction,
            ) {
                self.state = before_tick;
                self.pending_attack |= fixed_input.attack;
                self.pending_interact |= fixed_input.interact;
                return Err(error);
            }
            self.events.extend(tick_events);
            self.state.tick += 1;
            self.state.time += 1. / 60.;
            self.accumulator -= 1. / 60.;
            ticks += 1;
            if let Some(id) = self.state.transition.take() {
                self.transition(&id)?;
            }
            if self.state.status != GameStatus::Playing {
                self.accumulator = 0.;
                break;
            }
        }
        Ok(ticks)
    }
    /// Unprocessed fixed time, useful for honest interactive-worker status.
    pub fn pending_time(&self) -> f64 {
        self.accumulator
    }
    /// Explicit scene transition reloads scene-owned state, preserving entities
    /// marked persistent. Destination-authored hierarchy/transforms/references
    /// remain local.
    pub fn transition(&mut self, id: &str) -> Result<(), String> {
        let from = self.state.scene.clone();
        let carried: Vec<_> = self
            .state
            .entities
            .values()
            .filter(|e| e.components.contains_key("persistent"))
            .cloned()
            .collect();
        let spawn = self.state.transition_spawn;
        let mut next = Self::load_state(
            &self.project,
            id,
            self.state.time,
            self.state.tick,
            &self.active_components,
        )?;
        for mut entity in carried {
            if let Some(destination) = next.entities.get(&entity.object.id) {
                entity.object.parent = destination.object.parent;
                entity.object.transform = destination.object.transform;
                entity.references = destination.references.clone();
            } else {
                entity.object.parent = None;
                entity.references.clear();
            }
            if let Some(pos) = spawn {
                entity.object.transform.translation = pos;
            }
            next.entities.insert(entity.object.id, entity);
        }
        let provider = Self::load_provider(&self.world_factory, &next)?;
        let executor = SystemExecutor::new(&self.project.execution, &self.registry)?;
        // Stop hooks can fail after mutating their context. Keep the visible
        // scene/events until replacement succeeds; external shutdown effects
        // still follow the executor's best-effort, exactly-once contract.
        let mut stopped_state = self.state.clone();
        let mut stopped_events = self.events.clone();
        self.executor
            .stop(&mut stopped_state, &mut stopped_events)?;
        self.state = next;
        self.provider = provider;
        self.executor = executor;
        self.events = stopped_events;
        self.events.push(GameEvent::SceneChanged {
            from,
            to: id.into(),
        });
        Ok(())
    }
    pub fn restart(&mut self) -> Result<(), String> {
        let replacement = Self::build(
            self.project.clone(),
            self.kind,
            self.registry.clone(),
            self.world_factory.clone(),
        )?;
        let mut stopped_state = self.state.clone();
        let mut stopped_events = self.events.clone();
        self.executor
            .stop(&mut stopped_state, &mut stopped_events)?;
        *self = replacement;
        Ok(())
    }
    pub fn replace_system(
        &mut self,
        id: &str,
        registry: &SystemRegistry,
        migration: Option<Value>,
        reset: bool,
    ) -> Result<(), String> {
        self.executor.replace(id, registry, migration, reset)
    }
}

fn object_map(objects: &[WorldObject]) -> BTreeMap<u64, WorldObject> {
    objects.iter().map(|o| (o.id, o.clone())).collect()
}

/// Portable all-or-none DTO synchronization. Reparenting rebuilds only affected
/// object subtrees through standard commands, retaining the provider instance.
pub fn world_command_diff(
    before: &[WorldObject],
    after: &[WorldObject],
) -> Result<Vec<WorldCommand>, String> {
    crate::validate_world_commands(before, &[])?;
    crate::validate_world_commands(after, &[])?;
    let before = object_map(before);
    let after = object_map(after);
    let rebuilding: BTreeSet<_> = before
        .iter()
        .filter(|(id, o)| after.get(id).is_none_or(|n| n.parent != o.parent))
        .map(|(id, _)| *id)
        .collect();
    let ancestor_rebuilt = |id: u64, include_self: bool| {
        let mut current = if include_self {
            Some(id)
        } else {
            before.get(&id).and_then(|o| o.parent)
        };
        while let Some(id) = current {
            if rebuilding.contains(&id) {
                return true;
            }
            current = before.get(&id).and_then(|o| o.parent);
        }
        false
    };
    let removed: BTreeSet<_> = before
        .keys()
        .filter(|id| ancestor_rebuilt(**id, true))
        .copied()
        .collect();
    let mut commands: Vec<_> = rebuilding
        .iter()
        .filter(|id| !ancestor_rebuilt(**id, false))
        .map(|id| WorldCommand::Remove { id: *id })
        .collect();
    let mut spawned: Vec<_> = after
        .values()
        .filter(|o| !before.contains_key(&o.id) || removed.contains(&o.id))
        .collect();
    let depth = |object: &WorldObject| {
        let mut count = 0;
        let mut parent = object.parent;
        while let Some(id) = parent {
            count += 1;
            parent = after[&id].parent;
        }
        count
    };
    spawned.sort_by_key(|o| (depth(o), o.id));
    for object in spawned {
        commands.push(WorldCommand::Spawn {
            object: object.clone(),
        });
    }
    for (id, object) in &after {
        if removed.contains(id) {
            continue;
        }
        if let Some(old) = before.get(id) {
            if old.transform != object.transform {
                commands.push(WorldCommand::SetTransform {
                    id: *id,
                    transform: object.transform,
                });
            }
            if old.color != object.color {
                commands.push(WorldCommand::SetColor {
                    id: *id,
                    color: object.color,
                });
            }
        }
    }
    crate::validate_world_commands(&before.values().cloned().collect::<Vec<_>>(), &commands)?;
    Ok(commands)
}

struct WorldTickTransaction<'a> {
    provider: &'a mut dyn WorldProvider,
    before: Vec<WorldObject>,
    commands: Vec<WorldCommand>,
    applied: bool,
}
impl<'a> WorldTickTransaction<'a> {
    fn new(provider: &'a mut dyn WorldProvider) -> Result<Self, String> {
        let before = provider.objects()?;
        Ok(Self {
            provider,
            before,
            commands: vec![],
            applied: false,
        })
    }
}
impl TickTransaction for WorldTickTransaction<'_> {
    fn validate(&mut self, candidate: &GameState) -> Result<(), String> {
        if candidate.entities.iter().any(|(id, e)| *id != e.object.id) {
            return Err("World entity keys must match stable object IDs".into());
        }
        self.commands = world_command_diff(
            &self.before,
            &candidate
                .entities
                .values()
                .map(|e| e.object.clone())
                .collect::<Vec<_>>(),
        )?;
        self.provider.validate_commands(&self.commands)
    }
    fn commit(&mut self, candidate: &GameState) -> Result<(), String> {
        if self.commands.is_empty() {
            return Ok(());
        }
        self.applied = true;
        self.provider.apply_commands(&self.commands)?;
        let expected: Vec<_> = candidate
            .entities
            .values()
            .map(|e| e.object.clone())
            .collect();
        if object_map(&self.provider.objects()?) != object_map(&expected) {
            return Err(
                "World provider did not apply the validated portable command packet".into(),
            );
        }
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), String> {
        if self.applied {
            let commands = world_command_diff(&self.provider.objects()?, &self.before)?;
            self.provider.validate_commands(&commands)?;
            self.provider.apply_commands(&commands)?;
            self.applied = false;
        }
        Ok(())
    }
}

fn make_provider(
    kind: WorldKind,
    objects: Vec<WorldObject>,
) -> Result<Box<dyn WorldProvider>, String> {
    match kind {WorldKind::Objects=>Ok(Box::new(ObjectsWorld::from_objects(objects)?)),WorldKind::Ecs=>Ok(Box::new(EcsWorld::from_objects(objects)?)),WorldKind::Custom=>Err("GameSession needs a selected editable world factory; built-in convenience supports Object and ECS".into())}
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Actor {
    team: String,
    radius: f32,
    speed: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Health {
    maximum: f32,
    current: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Ai {
    kind: String,
    aggro_radius: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Melee {
    range: f32,
    damage: f32,
    cooldown: f32,
    arc: f32,
    duration: f32,
    windup: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Obstacle {
    half_extents: [f32; 2],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Portal {
    scene: String,
    radius: f32,
    requires_clear: bool,
    spawn: [f32; 3],
}
fn component<T: for<'de> Deserialize<'de>>(entity: &GameEntity, name: &str) -> Option<T> {
    serde_json::from_value(entity.components.get(name)?.clone()).ok()
}
fn health(e: &GameEntity) -> f32 {
    component::<Health>(e, "health").map_or(1., |h| h.current)
}
fn normalized(v: [f32; 2]) -> [f32; 2] {
    let length = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if length > 1e-6 {
        [v[0] / length, v[1] / length]
    } else {
        [0., 0.]
    }
}
fn attack_direction(center: [f32; 3], aim: [f32; 3]) -> [f32; 2] {
    let direction = normalized([aim[0] - center[0], aim[2] - center[2]]);
    if direction == [0., 0.] {
        [0., -1.]
    } else {
        direction
    }
}
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Component contracts belong to gameplay modules. Unknown component names are
/// preserved for independently registered systems rather than stripped.
pub(crate) fn validate_game_components(
    e: &GameEntity,
    scenes: &BTreeMap<String, crate::AuthoredScene>,
) -> Result<(), String> {
    let invalid = |name: &str| format!("Object {} has invalid '{name}' component", e.object.id);
    if e.components.contains_key("actor") {
        let a = component::<Actor>(e, "actor").ok_or_else(|| invalid("actor"))?;
        if !a.radius.is_finite()
            || a.radius <= 0.
            || !a.speed.is_finite()
            || a.speed < 0.
            || a.team.is_empty()
            || e.object.parent.is_some()
        {
            return Err(invalid(
                "actor: root object, positive radius and nonnegative speed required",
            ));
        }
    }
    if e.components.contains_key("health") {
        let h = component::<Health>(e, "health").ok_or_else(|| invalid("health"))?;
        if !h.maximum.is_finite()
            || h.maximum <= 0.
            || !h.current.is_finite()
            || h.current < 0.
            || h.current > h.maximum
        {
            return Err(invalid("health"));
        }
    }
    if e.components.contains_key("ai") {
        let a = component::<Ai>(e, "ai").ok_or_else(|| invalid("ai"))?;
        if !["chase", "boss"].contains(&a.kind.as_str())
            || !a.aggro_radius.is_finite()
            || a.aggro_radius < 0.
        {
            return Err(invalid("ai"));
        }
    }
    if e.components.contains_key("melee") {
        let a = component::<Melee>(e, "melee").ok_or_else(|| invalid("melee"))?;
        if ![a.range, a.damage, a.cooldown, a.arc, a.duration, a.windup]
            .iter()
            .all(|n| n.is_finite())
            || a.range <= 0.
            || a.damage < 0.
            || a.cooldown <= 0.
            || a.arc <= 0.
            || a.duration <= 0.
            || a.windup < 0.
        {
            return Err(invalid("melee"));
        }
    }
    if e.components.contains_key("obstacle") {
        let a = component::<Obstacle>(e, "obstacle").ok_or_else(|| invalid("obstacle"))?;
        if !a.half_extents.iter().all(|n| n.is_finite() && *n > 0.) || e.object.parent.is_some() {
            return Err(invalid(
                "obstacle: root object and positive extents required",
            ));
        }
    }
    if e.components.contains_key("portal") {
        let p = component::<Portal>(e, "portal").ok_or_else(|| invalid("portal"))?;
        if !scenes.contains_key(&p.scene)
            || !p.radius.is_finite()
            || p.radius <= 0.
            || !p.spawn.iter().all(|n| n.is_finite())
        {
            return Err(invalid("portal"));
        }
    }
    if let Some(mesh) = e.components.get("mesh") {
        let path = mesh
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("mesh"))?;
        let asset_id = path.strip_prefix("asset://");
        if asset_id.is_some_and(|id| id.trim().is_empty() || id.chars().any(char::is_control))
            || (asset_id.is_none()
                && (path.is_empty()
                    || std::path::Path::new(path).is_absolute()
                    || path.split(['\\', '/']).any(|v| v == "..")
                    || path.contains(':')))
        {
            return Err(invalid("mesh path must be project-relative"));
        }
    }
    Ok(())
}

type TickFn = fn(&mut SystemContext<'_>, &mut Value) -> Result<(), String>;
type BuiltinConfiguration = (
    &'static str,
    TickFn,
    Vec<(&'static str, &'static str, ResourceAccess)>,
    Vec<&'static str>,
);
struct NativeSystem(TickFn);
impl ExecutableSystem for NativeSystem {
    fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
        (self.0)(ctx, state)
    }
}

pub(crate) fn register_game_systems(registry: &mut SystemRegistry) {
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

fn input_system(ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
    for entity in ctx
        .game
        .entities
        .values()
        .filter(|e| e.components.contains_key("player"))
    {
        let mut movement = ctx.input.movement;
        let length = (movement[0] * movement[0] + movement[1] * movement[1]).sqrt();
        if length > 1. {
            movement = normalized(movement);
        }
        ctx.game.intentions.insert(
            entity.object.id,
            GameIntent {
                movement,
                aim: ctx.input.aim_world,
                attack: ctx.input.attack,
            },
        );
    }
    Ok(())
}

fn ai_system(ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
    let actors: Vec<_> = ctx
        .game
        .entities
        .values()
        .filter_map(|e| {
            component::<Actor>(e, "actor")
                .map(|a| (e.object.id, e.object.transform.translation, a, health(e)))
        })
        .collect();
    for e in ctx.game.entities.values() {
        let (Some(ai), Some(actor)) = (component::<Ai>(e, "ai"), component::<Actor>(e, "actor"))
        else {
            continue;
        };
        let pos = e.object.transform.translation;
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
        let melee = component::<Melee>(e, "melee");
        let attack = melee.as_ref().is_some_and(|m| d < m.range * 0.85);
        let movement = if attack {
            [0., 0.]
        } else {
            normalized([target[0] - pos[0], target[2] - pos[2]])
        };
        ctx.game.intentions.insert(
            e.object.id,
            GameIntent {
                movement,
                aim: *target,
                attack,
            },
        );
    }
    Ok(())
}

fn movement_system(ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
    let mut previous = BTreeMap::new();
    for (id, intent) in &ctx.game.intentions {
        let Some(entity) = ctx.game.entities.get_mut(id) else {
            continue;
        };
        let Some(actor) = component::<Actor>(entity, "actor") else {
            continue;
        };
        previous.insert(*id, entity.object.transform.translation);
        entity.object.transform.translation[0] += intent.movement[0] * actor.speed * ctx.dt;
        entity.object.transform.translation[2] += intent.movement[1] * actor.speed * ctx.dt;
    }
    ctx.set_resource(
        "previous",
        serde_json::to_value(previous).map_err(|e| e.to_string())?,
    )
}

fn collision_system(ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
    let previous: BTreeMap<u64, [f32; 3]> =
        serde_json::from_value(ctx.resource("previous")?.clone()).map_err(|e| e.to_string())?;
    let obstacles: Vec<_> = ctx
        .game
        .entities
        .values()
        .filter_map(|e| {
            component::<Obstacle>(e, "obstacle")
                .map(|o| (e.object.transform.translation, o.half_extents))
        })
        .collect();
    for (id, start) in previous {
        let Some(entity) = ctx.game.entities.get_mut(&id) else {
            continue;
        };
        let Some(actor) = component::<Actor>(entity, "actor") else {
            continue;
        };
        let wanted = entity.object.transform.translation;
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
        entity.object.transform.translation = p;
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Swing {
    cooldown: f32,
    remaining: f32,
    windup: f32,
    direction: [f32; 2],
    hit: Vec<u64>,
    active: bool,
}
fn combat_system(ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
    let mut swings: BTreeMap<u64, Swing> = if state.is_null() {
        BTreeMap::new()
    } else {
        serde_json::from_value(state.clone()).map_err(|e| e.to_string())?
    };
    let attackers: Vec<_> = ctx
        .game
        .entities
        .values()
        .filter_map(|e| {
            Some((
                e.object.id,
                e.object.transform.translation,
                component::<Actor>(e, "actor")?,
                component::<Melee>(e, "melee")?,
                health(e),
            ))
        })
        .collect();
    for (id, pos, _actor, melee, hp) in attackers {
        if hp <= 0. {
            continue;
        }
        let swing = swings.entry(id).or_default();
        swing.cooldown = (swing.cooldown - ctx.dt).max(0.);
        let intent = ctx.game.intentions.get(&id).cloned().unwrap_or_default();
        if intent.attack && swing.cooldown <= 0. && !swing.active {
            swing.cooldown = melee.cooldown;
            swing.remaining = melee.duration;
            swing.windup = melee.windup;
            swing.hit.clear();
            swing.active = true;
            swing.direction = attack_direction(pos, intent.aim);
            ctx.events.push(GameEvent::Attack { attacker: id });
        }
        if !swing.active {
            continue;
        }
        // Attack direction is captured at windup start, giving an evasion window.
        if swing.windup > 0. {
            swing.windup = (swing.windup - ctx.dt).max(0.);
            ctx.game.telegraphs.push(AttackTelegraph {
                attacker: id,
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
        ctx.game.attack_visuals.push(AttackVisual {
            attacker: id,
            area,
            progress: (1. - swing.remaining / melee.duration).clamp(0., 1.),
        });
        let targets = ctx.game.melee_targets(id, &area);
        for target in targets {
            if swing.hit.contains(&target) {
                continue;
            }
            let Some(entity) = ctx.game.entities.get_mut(&target) else {
                continue;
            };
            let Some(mut hp) = component::<Health>(entity, "health") else {
                continue;
            };
            hp.current = (hp.current - melee.damage).max(0.);
            entity.components.insert(
                "health".into(),
                serde_json::to_value(hp).map_err(|e| e.to_string())?,
            );
            swing.hit.push(target);
            ctx.game.hit_visuals.push(HitVisual {
                attacker: id,
                target,
                position: entity.object.transform.translation,
                damage: melee.damage,
                started_at: ctx.game.time,
                expires_at: ctx.game.time + 0.35,
            });
            ctx.events.push(GameEvent::Hit {
                attacker: id,
                target,
                damage: melee.damage,
            });
        }
        swing.remaining -= ctx.dt;
        if swing.remaining <= 0. {
            swing.active = false;
        }
    }
    swings.retain(|id, _| ctx.game.entities.contains_key(id));
    *state = serde_json::to_value(swings).map_err(|e| e.to_string())?;
    Ok(())
}

fn health_system(ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
    let mut emitted: Vec<u64> = if state.is_null() {
        vec![]
    } else {
        serde_json::from_value(state.clone()).map_err(|e| e.to_string())?
    };
    // Only retained dead entities need duplicate suppression. Removed IDs can
    // be spawned again, and a revived entity can die again in a later tick.
    emitted.retain(|id| {
        ctx.game
            .entities
            .get(id)
            .is_some_and(|e| e.components.contains_key("health") && health(e) <= 0.)
    });
    let dead: Vec<_> = ctx
        .game
        .entities
        .values()
        .filter(|e| e.components.contains_key("health") && health(e) <= 0.)
        // Capture death metadata before removing a parent can remove another
        // dead entity in this same batch.
        .map(|e| {
            (
                e.object.id,
                e.components.contains_key("player"),
                e.components
                    .get("boss")
                    .and_then(|v| v.get("victory_on_death"))
                    .and_then(Value::as_bool)
                    == Some(true),
            )
        })
        .collect();
    for (id, player, victory) in dead {
        if emitted.contains(&id) {
            continue;
        }
        emitted.push(id);
        ctx.events.push(GameEvent::Death { object: id });
        if player {
            ctx.game.status = GameStatus::Defeat;
        } else {
            if victory {
                ctx.game.status = GameStatus::Victory;
                ctx.events.push(GameEvent::Victory);
            }
            if ctx.game.entities.contains_key(&id) {
                ctx.game.apply_commands(&[WorldCommand::Remove { id }])?;
            }
        }
    }
    emitted.retain(|id| ctx.game.entities.contains_key(id));
    *state = json!(emitted);
    Ok(())
}

fn portal_system(ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
    if ctx.game.status != GameStatus::Playing {
        return Ok(());
    }
    let Some(player) = ctx.game.player() else {
        return Ok(());
    };
    let pos = player.object.transform.translation;
    for entity in ctx.game.entities.values() {
        let Some(portal) = component::<Portal>(entity, "portal") else {
            continue;
        };
        if distance(pos, entity.object.transform.translation) <= portal.radius
            && (!portal.requires_clear || ctx.game.living_enemies() == 0)
        {
            ctx.game.transition = Some(portal.scene);
            ctx.game.transition_spawn = Some(portal.spawn);
            break;
        }
    }
    Ok(())
}

fn audio_system(ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
    let sounds: Vec<_> = ctx
        .events
        .iter()
        .filter_map(|event| match event {
            GameEvent::Attack { attacker } => Some(("attack", Some(*attacker))),
            GameEvent::Hit { target, .. } => Some(("hit", Some(*target))),
            GameEvent::Death { object } => Some(("death", Some(*object))),
            GameEvent::Victory => Some(("victory", None)),
            _ => None,
        })
        .collect();
    for (cue, object) in sounds {
        ctx.events.push(GameEvent::Sound {
            cue: cue.into(),
            object,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_mesh_accepts_opaque_asset_ids_and_rejects_invalid_paths() {
        let mut project = GameProject::demo();
        let object = &mut project.scenes.get_mut("arena").unwrap().objects[0];
        object.components.insert(
            "mesh".into(),
            serde_json::json!({"path":"asset://stable-model-id"}),
        );
        assert!(project.validate_scene_data().is_ok());
        for path in [
            "asset://",
            "../outside.glb",
            "C:/outside.glb",
            "asset://bad\nID",
        ] {
            project.scenes.get_mut("arena").unwrap().objects[0]
                .components
                .insert("mesh".into(), serde_json::json!({"path":path}));
            assert!(project.validate_scene_data().is_err(), "accepted {path:?}");
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
                let mut session = GameSession::new(project, kind).unwrap();
                session.step(1. / 60., &GameInput::default()).unwrap();
                let deaths: Vec<_> = session
                    .events()
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
                    session.state().status,
                    if player {
                        GameStatus::Defeat
                    } else {
                        GameStatus::Victory
                    }
                );
                assert_eq!(session.events().contains(&GameEvent::Victory), !player);
                assert!(session.state().entities.is_empty());
                assert!(session.provider.objects().unwrap().is_empty());
                assert!(session.snapshot().items.is_empty());
            }
        }
    }
    #[test]
    fn health_reused_removed_id_emits_another_death() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let project = focused_project(vec![dead_object(10, None)], &["game.health"]);
            let mut session = GameSession::new(project, kind).unwrap();
            for _ in 0..2 {
                session.step(1. / 60., &GameInput::default()).unwrap();
                assert_eq!(session.events(), &[GameEvent::Death { object: 10 }]);
                assert!(session.state().entities.is_empty());
                assert!(session.provider.objects().unwrap().is_empty());
                session
                    .apply_commands(&[WorldCommand::Spawn {
                        object: test_object(10, None).object,
                    }])
                    .unwrap();
                session
                    .state
                    .entities
                    .get_mut(&10)
                    .unwrap()
                    .components
                    .insert("health".into(), json!({"maximum":10.,"current":0.}));
            }
        }
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
    #[test]
    fn rejected_health_publication_restores_subtrees_events_and_death_state() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let reject = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let control = reject.clone();
            let mut registry = SystemRegistry::builtin();
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
            assert!(session.events().is_empty());
            reject.store(false, std::sync::atomic::Ordering::SeqCst);
            session.step_budget(0., &GameInput::default(), 1).unwrap();
            assert_eq!(
                session.events(),
                &[
                    GameEvent::Death { object: 10 },
                    GameEvent::Death { object: 20 }
                ]
            );
            assert!(session.state().entities.is_empty());
            assert!(session.provider.objects().unwrap().is_empty());
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
            let mut session = GameSession::new(project, kind).unwrap();
            session.transition("destination").unwrap();
            let carried = &session.state().entities[&2];
            assert_eq!(carried.object, destination.object);
            assert_eq!(carried.references, destination.references);
            assert_eq!(health(carried), 6.);
            assert!(carried.components.contains_key("persistent"));
            let absent = &session.state().entities[&4];
            assert_eq!(absent.object.parent, None);
            assert!(absent.references.is_empty());
            assert_eq!(
                object_map(&session.provider.objects().unwrap()),
                object_map(&session.world_objects())
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
    fn replacement_provider_must_load_exact_candidate_before_old_scene_is_changed() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            for restart in [false, true] {
                let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let mut session = GameSession::new_with_world_factory(
                    persistent_hierarchy_project(),
                    kind,
                    SystemRegistry::builtin(),
                    move |mut objects| {
                        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                            objects[0].color = [1, 2, 3, 4];
                        }
                        make_provider(kind, objects)
                    },
                )
                .unwrap();
                let state = session.state().clone();
                let snapshot = session.snapshot();
                let events = session.events().to_vec();
                let error = if restart {
                    session.restart()
                } else {
                    session.transition("destination")
                }
                .unwrap_err();
                assert!(error.contains("did not load the supplied authored objects"));
                assert_eq!(session.state(), &state);
                assert_eq!(session.snapshot(), snapshot);
                assert_eq!(session.events(), events);
            }
        }
    }
    struct MutatingFailedStop;
    impl ExecutableSystem for MutatingFailedStop {
        fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            Ok(())
        }
        fn stop(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            ctx.game.scene = "corrupted by stop".into();
            ctx.game.entities.clear();
            ctx.events.push(GameEvent::Victory);
            Err("Stop failed".into())
        }
    }
    #[test]
    fn failed_replacement_stop_preserves_visible_scene_and_events() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            for restart in [false, true] {
                let mut project = persistent_hierarchy_project();
                append_test_system(&mut project, "test.failed-stop");
                let mut registry = SystemRegistry::builtin();
                registry
                    .register(test_system_descriptor("test.failed-stop"), |_| {
                        Ok(Box::new(MutatingFailedStop))
                    })
                    .unwrap();
                let mut session = GameSession::new_with_registry(project, kind, registry).unwrap();
                session.step(1. / 60., &GameInput::default()).unwrap();
                let state = session.state().clone();
                let snapshot = session.snapshot();
                let events = session.events().to_vec();
                let error = if restart {
                    session.restart()
                } else {
                    session.transition("destination")
                }
                .unwrap_err();
                assert!(error.contains("Stop failed"));
                assert_eq!(session.state(), &state);
                assert_eq!(session.snapshot(), snapshot);
                assert_eq!(session.events(), events);
            }
        }
    }
    #[test]
    fn fixed_tick_preserves_short_frame_attack_and_world_parity() {
        let p = GameProject::demo();
        let mut a = GameSession::new(p.clone(), WorldKind::Objects).unwrap();
        let mut b = GameSession::new(p, WorldKind::Ecs).unwrap();
        let attack = GameInput {
            attack: true,
            aim_world: [0., 0., -4.],
            ..GameInput::default()
        };
        assert_eq!(a.step(0.001, &attack).unwrap(), 0);
        b.step(0.001, &attack).unwrap();
        let input = GameInput {
            movement: [1., 0.],
            aim_world: [0., 0., -4.],
            ..GameInput::default()
        };
        for _ in 0..30 {
            a.step(1. / 60., &input).unwrap();
            b.step(1. / 60., &input).unwrap();
        }
        assert_eq!(a.world_objects(), b.world_objects());
        assert_eq!(a.snapshot(), b.snapshot());
        assert_eq!(a.state.tick, 30);
    }
    #[test]
    fn budgeted_fixed_ticks_retain_catchup_time_and_match_synchronous_state() {
        let project = GameProject::demo();
        let mut synchronous = GameSession::new(project.clone(), WorldKind::Objects).unwrap();
        let mut yielding = GameSession::new(project, WorldKind::Objects).unwrap();
        let pressed = GameInput {
            attack: true,
            movement: [0.5, 0.],
            aim_world: [0., 0., -4.],
            ..GameInput::default()
        };
        assert_eq!(synchronous.step(0.25, &pressed).unwrap(), 15);
        assert_eq!(yielding.step_budget(0.25, &pressed, 1).unwrap(), 1);
        assert!((yielding.pending_time() - 14. / 60.).abs() < 1e-10);
        let held = GameInput {
            attack: false,
            ..pressed
        };
        for _ in 0..14 {
            assert_eq!(yielding.step_budget(0., &held, 1).unwrap(), 1);
        }
        assert!(yielding.pending_time().abs() < 1e-10);
        assert_eq!(yielding.state(), synchronous.state());
        assert_eq!(yielding.snapshot(), synchronous.snapshot());
    }
    #[test]
    fn collision_blocks_walls_and_restart_restores_authored_scene() {
        let p = project_without_enemies();
        let authored = p.clone();
        let mut s = GameSession::new(p, WorldKind::Objects).unwrap();
        let input = GameInput {
            movement: [1., 0.],
            ..GameInput::default()
        };
        for _ in 0..120 {
            s.step(1. / 60., &input).unwrap();
        }
        assert!(s.state.player().unwrap().object.transform.translation[0] <= 7.46);
        assert_eq!(s.project(), &authored);
        s.restart().unwrap();
        assert_eq!(
            s.state.player().unwrap().object.transform.translation,
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
                e.object.transform.translation =
                    [if e.object.id == 10 { -0.2 } else { 0.2 }, 0., 3.5];
                e.components
                    .insert("ai".into(), json!({"kind":"chase","aggro_radius":0.0}));
            }
        }
        let mut s = GameSession::new(p, WorldKind::Objects).unwrap();
        let input = GameInput {
            attack: true,
            aim_world: [0., 0., 3.],
            ..GameInput::default()
        };
        s.step(1. / 60., &input).unwrap();
        assert_eq!(
            s.events
                .iter()
                .filter(|e| matches!(e, GameEvent::Hit { attacker: 1, .. }))
                .count(),
            2
        );
        for _ in 0..5 {
            s.step(1. / 60., &GameInput::default()).unwrap();
            assert!(
                !s.events
                    .iter()
                    .any(|e| matches!(e, GameEvent::Hit { attacker: 1, .. }))
            );
        }
        assert_eq!(health(&s.state.entities[&10]), 8.);
        assert_eq!(health(&s.state.entities[&11]), 8.);
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
        let mut session = GameSession::new(project, WorldKind::Objects).unwrap();
        // A module can supply an actor without a damageable Health component.
        // It remains in the world, but combat has always skipped damaging it.
        session
            .state
            .entities
            .get_mut(&10)
            .unwrap()
            .components
            .remove("health");
        let aim = [0., 0., 3.];
        let area = session.state().melee_aim(1, aim).unwrap();
        assert!(area.contains(
            session.state.entities[&10].object.transform.translation,
            0.36
        ));
        assert_eq!(session.state().melee_targets(1, &area), vec![11]);
        session
            .step(
                1. / 60.,
                &GameInput {
                    attack: true,
                    aim_world: aim,
                    ..GameInput::default()
                },
            )
            .unwrap();
        let targets = session
            .events()
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
        assert!(session.state().entities.contains_key(&10));
        assert!(
            !session
                .state()
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
        let input = GameInput {
            attack: true,
            aim_world: [0., 0., 3.],
            ..GameInput::default()
        };
        let mut objects = GameSession::new(project.clone(), WorldKind::Objects).unwrap();
        let mut ecs = GameSession::new(project, WorldKind::Ecs).unwrap();
        let preview = objects.state().melee_aim(1, input.aim_world).unwrap();
        assert_eq!(objects.state().melee_targets(1, &preview), vec![10, 11]);
        for session in [&mut objects, &mut ecs] {
            session.step(1. / 60., &input).unwrap();
            let attack = session
                .state()
                .attack_visuals
                .iter()
                .find(|v| v.attacker == 1)
                .unwrap();
            assert_eq!(attack.area, preview);
            assert_eq!(attack.progress, 0.);
            assert!(!session.state().entities.contains_key(&10));
            let lethal_hit = session
                .state()
                .hit_visuals
                .iter()
                .find(|h| h.target == 10)
                .unwrap();
            assert_eq!(lethal_hit.position, [-0.2, 0., 3.5]);
            assert_eq!(lethal_hit.damage, 28.);
            assert_eq!(session.state().hit_visuals.len(), 2);
            // A renderer frame shorter than a tick must not discard feedback.
            let retained = session.state().hit_visuals.clone();
            session.step(0.001, &GameInput::default()).unwrap();
            assert_eq!(session.state().hit_visuals, retained);
        }
        assert_eq!(objects.state(), ecs.state());
        for tick in 0..30 {
            for session in [&mut objects, &mut ecs] {
                session
                    .step(
                        1. / 60.,
                        &GameInput {
                            aim_world: [0., 0., 8.],
                            ..GameInput::default()
                        },
                    )
                    .unwrap();
                // The cursor can change, but the existing swing remains aimed
                // in its original direction, and cannot hit a target twice.
                for attack in session
                    .state()
                    .attack_visuals
                    .iter()
                    .filter(|v| v.attacker == 1)
                {
                    assert_eq!(attack.area.direction, preview.direction);
                }
                assert!(
                    !session
                        .events()
                        .iter()
                        .any(|e| matches!(e, GameEvent::Hit { attacker: 1, .. }))
                );
                if tick < 4 {
                    assert!(!session.state().attack_visuals.is_empty());
                    assert_eq!(session.state().hit_visuals.len(), 2);
                }
                if tick == 10 {
                    // Swing ends before hit feedback: these are independent
                    // durations rather than a single frame's event list.
                    assert!(session.state().attack_visuals.is_empty());
                    assert_eq!(session.state().hit_visuals.len(), 2);
                }
            }
            assert_eq!(objects.state(), ecs.state());
            assert_eq!(objects.snapshot(), ecs.snapshot());
        }
        assert!(objects.state().attack_visuals.is_empty());
        assert!(objects.state().hit_visuals.is_empty());
        objects.restart().unwrap();
        objects.step(1. / 60., &input).unwrap();
        assert!(!objects.state().attack_visuals.is_empty());
        assert!(!objects.state().hit_visuals.is_empty());
        objects.transition("boss").unwrap();
        assert!(objects.state().attack_visuals.is_empty());
        assert!(objects.state().hit_visuals.is_empty());
        objects.restart().unwrap();
        assert!(objects.state().attack_visuals.is_empty());
        assert!(objects.state().hit_visuals.is_empty());
        assert!(objects.state().entities.contains_key(&10));
    }
    #[test]
    fn old_game_state_deserializes_without_combat_visuals() {
        let session = GameSession::new(project_without_enemies(), WorldKind::Objects).unwrap();
        let mut legacy = serde_json::to_value(session.state()).unwrap();
        legacy.as_object_mut().unwrap().remove("attack_visuals");
        legacy.as_object_mut().unwrap().remove("hit_visuals");
        let restored: GameState = serde_json::from_value(legacy).unwrap();
        assert!(restored.attack_visuals.is_empty());
        assert!(restored.hit_visuals.is_empty());
    }
    #[test]
    fn portal_transfers_health_and_boss_warns_before_damage() {
        let p = project_without_enemies();
        let mut s = GameSession::new(p, WorldKind::Ecs).unwrap();
        s.state
            .entities
            .get_mut(&1)
            .unwrap()
            .components
            .get_mut("health")
            .unwrap()["current"] = json!(60.);
        s.apply_commands(&[WorldCommand::SetPosition {
            id: 1,
            position: [0., 0., -7.2],
        }])
        .unwrap();
        s.step(1. / 60., &GameInput::default()).unwrap();
        assert_eq!(s.state.scene, "boss");
        assert_eq!(health(s.state.player().unwrap()), 60.);
        assert!(
            s.events
                .iter()
                .any(|e| matches!(e, GameEvent::SceneChanged { .. }))
        );
        s.apply_commands(&[WorldCommand::SetPosition {
            id: 1,
            position: [0., 0., -1.],
        }])
        .unwrap();
        s.step(1. / 60., &GameInput::default()).unwrap();
        assert!(!s.state.telegraphs.is_empty());
        assert_eq!(health(s.state.player().unwrap()), 60.);
        s.apply_commands(&[WorldCommand::SetPosition {
            id: 1,
            position: [7., 0., 5.],
        }])
        .unwrap();
        for _ in 0..70 {
            s.step(1. / 60., &GameInput::default()).unwrap();
        }
        assert_eq!(health(s.state.player().unwrap()), 60.);
    }
    struct DeclaredData;
    impl ExecutableSystem for DeclaredData {
        fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            Ok(())
        }
    }
    #[test]
    fn inactive_components_remain_authored_but_do_not_enter_runtime_or_export() {
        let mut project = GameProject::demo();
        project
            .prefabs
            .get_mut("small-monster")
            .unwrap()
            .components
            .insert("replication".into(), json!({"enabled":true,"channel":7}));
        project
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects
            .iter_mut()
            .find(|e| e.object.id == 10)
            .unwrap()
            .overrides
            .insert("replication.enabled".into(), json!(false));
        let authored = project.clone();
        let registry = SystemRegistry::builtin();
        let session = GameSession::new(project.clone(), WorldKind::Objects).unwrap();
        assert!(
            !session.state().entities[&10]
                .components
                .contains_key("replication")
        );
        assert_eq!(session.project(), &authored);
        let exported = project.prepare_runtime(&registry).unwrap();
        assert!(
            !exported.prefabs["small-monster"]
                .components
                .contains_key("replication")
        );
        assert!(
            !exported.scenes["arena"]
                .objects
                .iter()
                .find(|e| e.object.id == 10)
                .unwrap()
                .overrides
                .contains_key("replication.enabled")
        );
        exported.validate().unwrap();
        assert_eq!(project, authored);
        let mut registry = SystemRegistry::builtin();
        registry
            .register(
                SystemDescriptor {
                    id: "test.declared-data".into(),
                    state_schema: "1".into(),
                    components: vec![],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(DeclaredData)),
            )
            .unwrap();
        project.execution.stages.push(crate::SystemStage {
            name: "Optional data consumer".into(),
            nodes: vec![crate::SystemNode {
                id: "optional-data".into(),
                module: "test.declared-data".into(),
                params: json!({"components":["replication"]}),
                bindings: BTreeMap::new(),
                after: vec![],
            }],
        });
        let restored =
            GameSession::new_with_registry(project.clone(), WorldKind::Ecs, registry.clone())
                .unwrap();
        assert_eq!(
            restored.state().entities[&10].components["replication"]["enabled"],
            json!(false)
        );
        assert!(
            project.prepare_runtime(&registry).unwrap().prefabs["small-monster"]
                .components
                .contains_key("replication")
        );
    }

    struct CachedWorld {
        inner: ObjectsWorld,
        packets: u32,
        max_player_x: Option<f32>,
    }
    impl WorldProvider for CachedWorld {
        fn step(&mut self, dt: f32, speed: f32) {
            self.inner.step(dt, speed);
        }
        fn snapshot(&self) -> SceneSnapshot {
            let mut s = self.inner.snapshot();
            s.items[0].color[0] = (self.packets % 255) as u8;
            s
        }
        fn objects(&self) -> Result<Vec<WorldObject>, String> {
            self.inner.objects()
        }
        fn validate_commands(&self, commands: &[WorldCommand]) -> Result<(), String> {
            let mut candidate = self.inner.clone();
            candidate.apply_commands(commands)?;
            if let Some(max) = self.max_player_x
                && candidate
                    .objects()?
                    .iter()
                    .any(|o| o.id == 1 && o.transform.translation[0] > max)
            {
                return Err("Third world rejects player outside its supported domain".into());
            }
            Ok(())
        }
        fn apply_commands(&mut self, commands: &[WorldCommand]) -> Result<(), String> {
            self.validate_commands(commands)?;
            self.inner.apply_commands(commands)?;
            self.packets += 1;
            Ok(())
        }
    }
    #[test]
    fn third_world_instance_and_private_cache_survive_fixed_ticks_and_direct_commands() {
        let created = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = created.clone();
        let mut session = GameSession::new_with_world_factory(
            GameProject::demo(),
            WorldKind::Custom,
            SystemRegistry::builtin(),
            move |objects| {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Box::new(CachedWorld {
                    inner: ObjectsWorld::from_objects(objects)?,
                    packets: 0,
                    max_player_x: None,
                }))
            },
        )
        .unwrap();
        let input = GameInput {
            movement: [1., 0.],
            ..GameInput::default()
        };
        for _ in 0..4 {
            session.step(1. / 60., &input).unwrap();
        }
        assert_eq!(created.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(session.snapshot().items[0].color[0], 4);
        session
            .apply_commands(&[WorldCommand::SetColor {
                id: 1,
                color: [40, 50, 60, 255],
            }])
            .unwrap();
        assert_eq!(created.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(session.snapshot().items[0].color[0], 5);
    }
    #[test]
    fn portable_diff_reparents_subtrees_and_removes_each_root_once() {
        let before = vec![
            WorldObject {
                id: 1,
                parent: None,
                transform: crate::Transform::default(),
                color: [1; 4],
            },
            WorldObject {
                id: 2,
                parent: Some(1),
                transform: crate::Transform::default(),
                color: [2; 4],
            },
            WorldObject {
                id: 3,
                parent: Some(2),
                transform: crate::Transform::default(),
                color: [3; 4],
            },
            WorldObject {
                id: 4,
                parent: None,
                transform: crate::Transform::default(),
                color: [4; 4],
            },
        ];
        let mut after = before.clone();
        after[1].parent = Some(4);
        after[2].color = [7; 4];
        let commands = world_command_diff(&before, &after).unwrap();
        assert_eq!(
            commands
                .iter()
                .filter(|c| matches!(c, WorldCommand::Remove { .. }))
                .count(),
            1
        );
        let mut provider = EcsWorld::from_objects(before.clone()).unwrap();
        provider.apply_commands(&commands).unwrap();
        assert_eq!(object_map(&provider.objects().unwrap()), object_map(&after));
        let after_removed = vec![after[1].clone(), after[2].clone(), after[3].clone()];
        let commands = world_command_diff(&before, &after_removed).unwrap();
        assert_eq!(
            commands
                .iter()
                .filter(|c| matches!(c, WorldCommand::Remove { .. }))
                .count(),
            1
        );
        let mut provider = ObjectsWorld::from_objects(before).unwrap();
        provider.apply_commands(&commands).unwrap();
        assert_eq!(
            object_map(&provider.objects().unwrap()),
            object_map(&after_removed)
        );
    }
    struct StatePublisher {
        published: Arc<std::sync::atomic::AtomicUsize>,
        before: usize,
        pending: usize,
    }
    impl ExecutableSystem for StatePublisher {
        fn step(&mut self, _: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            self.before = self.published.load(std::sync::atomic::Ordering::SeqCst);
            self.pending = self.before + 1;
            *state = json!(self.pending);
            Ok(())
        }
        fn commit_tick(&mut self) -> Result<(), String> {
            self.published
                .store(self.pending, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn rollback_tick(&mut self) -> Result<(), String> {
            self.published
                .store(self.before, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }
    #[test]
    fn provider_rejection_does_not_publish_external_owned_state() {
        let published = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let target = published.clone();
        let mut registry = SystemRegistry::builtin();
        registry
            .register(
                SystemDescriptor {
                    id: "test.state-publisher".into(),
                    state_schema: "1".into(),
                    components: vec![],
                    ports: vec![],
                    requires: vec![],
                },
                move |_| {
                    Ok(Box::new(StatePublisher {
                        published: target.clone(),
                        before: 0,
                        pending: 0,
                    }))
                },
            )
            .unwrap();
        let mut project = GameProject::demo();
        project.execution.stages.push(crate::SystemStage {
            name: "External state".into(),
            nodes: vec![crate::SystemNode {
                id: "publisher".into(),
                module: "test.state-publisher".into(),
                params: Value::Null,
                bindings: BTreeMap::new(),
                after: vec![],
            }],
        });
        let mut session =
            GameSession::new_with_world_factory(project, WorldKind::Custom, registry, |objects| {
                Ok(Box::new(CachedWorld {
                    inner: ObjectsWorld::from_objects(objects)?,
                    packets: 0,
                    max_player_x: Some(0.05),
                }))
            })
            .unwrap();
        let before = session.world_objects();
        let snapshot = session.snapshot();
        let states = session.system_states();
        let error = session
            .step(
                1. / 60.,
                &GameInput {
                    movement: [1., 0.],
                    ..GameInput::default()
                },
            )
            .unwrap_err();
        assert!(error.contains("Third world rejects"));
        assert_eq!(published.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(session.world_objects(), before);
        assert_eq!(session.snapshot(), snapshot);
        assert_eq!(session.system_states(), states);
        session
            .step(
                1. / 60.,
                &GameInput {
                    movement: [-1., 0.],
                    ..GameInput::default()
                },
            )
            .unwrap();
        assert!(published.load(std::sync::atomic::Ordering::SeqCst) > 0);
    }
    struct LifecycleCounter {
        started: Arc<std::sync::atomic::AtomicUsize>,
        stopped: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl ExecutableSystem for LifecycleCounter {
        fn initialize(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            self.started
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            Ok(())
        }
        fn stop(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            self.stopped
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }
    #[test]
    fn session_drop_stops_initialized_systems_once_after_restart_and_transition() {
        let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stopped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let a = started.clone();
        let b = stopped.clone();
        let mut registry = SystemRegistry::builtin();
        registry
            .register(
                SystemDescriptor {
                    id: "test.lifecycle".into(),
                    state_schema: "1".into(),
                    components: vec![],
                    ports: vec![],
                    requires: vec![],
                },
                move |_| {
                    Ok(Box::new(LifecycleCounter {
                        started: a.clone(),
                        stopped: b.clone(),
                    }))
                },
            )
            .unwrap();
        let mut project = GameProject::demo();
        project.execution.stages.push(crate::SystemStage {
            name: "Lifecycle counter".into(),
            nodes: vec![crate::SystemNode {
                id: "lifecycle".into(),
                module: "test.lifecycle".into(),
                params: Value::Null,
                bindings: BTreeMap::new(),
                after: vec![],
            }],
        });
        {
            let mut session = GameSession::new_with_registry(
                project.clone(),
                WorldKind::Objects,
                registry.clone(),
            )
            .unwrap();
            session.step(1. / 60., &GameInput::default()).unwrap();
        }
        assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(stopped.load(std::sync::atomic::Ordering::SeqCst), 1);
        {
            let mut session =
                GameSession::new_with_registry(project, WorldKind::Ecs, registry).unwrap();
            session.step(1. / 60., &GameInput::default()).unwrap();
            session.restart().unwrap();
            assert_eq!(
                stopped.load(std::sync::atomic::Ordering::SeqCst),
                2,
                "Restart and replacement Drop must not call stop twice"
            );
            session.step(1. / 60., &GameInput::default()).unwrap();
            session.transition("boss").unwrap();
            assert_eq!(stopped.load(std::sync::atomic::Ordering::SeqCst), 3);
            session.step(1. / 60., &GameInput::default()).unwrap();
        }
        assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 4);
        assert_eq!(stopped.load(std::sync::atomic::Ordering::SeqCst), 4);
    }
}
