//! Fixed-step, provider-neutral gameplay. Behaviors are separately registered
//! systems; scene names, encounters and progression are authored data.
use crate::{
    ComponentValidationContext, EcsWorld, ExecutableSystem, GameEntity, GameProject, ObjectsWorld,
    ResourceAccess, SceneSnapshot, SystemContext, SystemDescriptor, SystemExecutor, SystemPort,
    SystemRegistry, TickTransaction, WorldCommand, WorldKind, WorldObject, WorldProvider,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GameIntent {
    pub movement: [f32; 2],
    pub aim: [f32; 3],
    pub attack: bool,
    #[serde(default)]
    pub lifetime: Option<u64>,
}

impl PartialEq for GameIntent {
    fn eq(&self, other: &Self) -> bool {
        self.movement == other.movement && self.aim == other.aim && self.attack == other.attack
    }
}

/// World-space telegraph for the UI/renderer. Filled from actual combat state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttackTelegraph {
    pub attacker: u64,
    #[serde(default)]
    pub attacker_lifetime: Option<u64>,
    #[serde(default)]
    pub attacker_player: Option<bool>,
    pub center: [f32; 3],
    pub direction: [f32; 2],
    pub range: f32,
    pub arc: f32,
    pub progress: f32,
}

impl PartialEq for AttackTelegraph {
    fn eq(&self, other: &Self) -> bool {
        self.attacker == other.attacker
            && self.attacker_player == other.attacker_player
            && self.center == other.center
            && self.direction == other.direction
            && self.range == other.range
            && self.arc == other.arc
            && self.progress == other.progress
    }
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttackVisual {
    pub attacker: u64,
    #[serde(default)]
    pub attacker_lifetime: Option<u64>,
    #[serde(default)]
    pub attacker_player: Option<bool>,
    pub area: MeleeArea,
    pub progress: f32,
}

impl PartialEq for AttackVisual {
    fn eq(&self, other: &Self) -> bool {
        self.attacker == other.attacker
            && self.attacker_player == other.attacker_player
            && self.area == other.area
            && self.progress == other.progress
    }
}

/// A confirmed hit retained for 0.35 simulated seconds, including lethal hits.
/// Position is captured before the health system can remove the target.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HitVisual {
    pub attacker: u64,
    pub target: u64,
    /// Runtime incarnation prevents an old hit flashing a replacement object.
    #[serde(default)]
    pub target_lifetime: Option<u64>,
    #[serde(default)]
    pub target_player: Option<bool>,
    pub position: [f32; 3],
    pub damage: f32,
    pub started_at: f64,
    pub expires_at: f64,
}

impl PartialEq for HitVisual {
    fn eq(&self, other: &Self) -> bool {
        self.attacker == other.attacker
            && self.target == other.target
            && self.target_player == other.target_player
            && self.position == other.position
            && self.damage == other.damage
            && self.started_at == other.started_at
            && self.expires_at == other.expires_at
    }
}

/// Mutable Play state; none of these values write back to the authored project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameState {
    pub scene: String,
    pub time: f64,
    pub tick: u64,
    pub status: GameStatus,
    #[serde(rename = "entities", with = "crate::world::entity_dto_map")]
    pub world: EcsWorld,
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
        self.world.apply_commands(commands)
    }
    pub fn player(&self) -> Option<GameEntity> {
        self.world
            .entities_with(&["player"])
            .first()
            .and_then(|id| self.world.entity(*id))
    }
    pub fn living_enemies(&self) -> usize {
        self.world
            .entities_with(&["actor"])
            .into_iter()
            .filter(|id| {
                world_component::<Actor>(&self.world, *id, "actor")
                    .is_some_and(|a| a.team == "enemy")
                    && world_component::<Health>(&self.world, *id, "health")
                        .is_none_or(|health| health.current > 0.)
            })
            .count()
    }
    /// Prospective attack region, using the same aim fallback as a real swing.
    pub fn melee_aim(&self, attacker: u64, aim_world: [f32; 3]) -> Option<MeleeArea> {
        world_component::<Actor>(&self.world, attacker, "actor")?;
        let melee = world_component::<Melee>(&self.world, attacker, "melee")?;
        if world_component::<Health>(&self.world, attacker, "health")
            .is_some_and(|health| health.current <= 0.)
        {
            return None;
        }
        let center = self.world.object(attacker)?.transform.translation;
        Some(MeleeArea {
            center,
            direction: attack_direction(center, aim_world),
            range: melee.range,
            arc: melee.arc,
        })
    }
    /// Living opposing targets eligible for a fresh swing in the supplied area.
    pub fn melee_targets(&self, attacker: u64, area: &MeleeArea) -> Vec<u64> {
        let Some(actor) = world_component::<Actor>(&self.world, attacker, "actor") else {
            return vec![];
        };
        self.world
            .entities_with(&["actor", "health"])
            .into_iter()
            .filter_map(|id| {
                let other = world_component::<Actor>(&self.world, id, "actor")?;
                let hp = world_component::<Health>(&self.world, id, "health")?;
                (id != attacker
                    && other.team != actor.team
                    && hp.current > 0.
                    && area.contains(self.world.object(id)?.transform.translation, other.radius))
                .then_some(id)
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
    provider: Option<Box<dyn WorldProvider>>,
    events: Vec<GameEvent>,
    accumulator: f64,
    pending_attack: bool,
    pending_interact: bool,
    world_factory: Option<GameWorldFactory>,
    active_components: BTreeSet<String>,
    validation_modules: BTreeSet<String>,
    asset_paths: Option<BTreeMap<String, String>>,
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
        let modules = registry.installed_modules();
        Self::build(
            project,
            kind,
            registry,
            compatibility_factory(kind)?,
            modules,
            None,
            None,
        )
    }
    /// Project-aware schema validation; copy reference context for later loads.
    pub fn new_with_registry_and_context(
        project: GameProject,
        kind: WorldKind,
        registry: SystemRegistry,
        context: &ComponentValidationContext<'_>,
    ) -> Result<Self, String> {
        Self::build(
            project,
            kind,
            registry,
            compatibility_factory(kind)?,
            context.modules.clone(),
            context.asset_paths.cloned(),
            context.active_components.cloned(),
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
        let modules = registry.installed_modules();
        Self::build(
            project,
            kind,
            registry,
            Some(Arc::new(factory)),
            modules,
            None,
            None,
        )
    }
    pub fn new_with_world_factory_and_context<F>(
        project: GameProject,
        kind: WorldKind,
        registry: SystemRegistry,
        factory: F,
        context: &ComponentValidationContext<'_>,
    ) -> Result<Self, String>
    where
        F: Fn(Vec<WorldObject>) -> Result<Box<dyn WorldProvider>, String> + Send + Sync + 'static,
    {
        Self::build(
            project,
            kind,
            registry,
            Some(Arc::new(factory)),
            context.modules.clone(),
            context.asset_paths.cloned(),
            context.active_components.cloned(),
        )
    }
    fn build(
        project: GameProject,
        kind: WorldKind,
        registry: SystemRegistry,
        world_factory: Option<GameWorldFactory>,
        validation_modules: BTreeSet<String>,
        asset_paths: Option<BTreeMap<String, String>>,
        active_override: Option<BTreeSet<String>>,
    ) -> Result<Self, String> {
        let context = ComponentValidationContext {
            scenes: &project.scenes,
            modules: &validation_modules,
            asset_paths: asset_paths.as_ref(),
            active_components: active_override.as_ref(),
        };
        project.validate_with_context(&registry, &context)?;
        let active_components = project.active_components_with_context(&registry, &context)?;
        let state = Self::load_state(
            &project,
            &project.entry_scene,
            0.,
            0,
            &registry,
            &active_components,
            &context,
        )?;
        let provider = world_factory
            .as_ref()
            .map(|factory| Self::load_provider(factory, &state))
            .transpose()?;
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
            validation_modules,
            asset_paths,
        })
    }
    fn load_provider(
        world_factory: &GameWorldFactory,
        state: &GameState,
    ) -> Result<Box<dyn WorldProvider>, String> {
        let objects = state.world.objects_snapshot();
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
        registry: &SystemRegistry,
        active_components: &BTreeSet<String>,
        context: &ComponentValidationContext<'_>,
    ) -> Result<GameState, String> {
        Ok(GameState {
            scene: id.into(),
            time,
            tick,
            status: GameStatus::Playing,
            world: EcsWorld::from_entities(
                project
                    .resolve_scene_with_context(id, registry, context)?
                    .into_iter()
                    .map(|mut e| {
                        e.components
                            .retain(|name, _| active_components.contains(name));
                        e
                    })
                    .collect(),
            )?,
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
        self.state.world.objects_snapshot()
    }
    pub fn asset_bindings(&self) -> BTreeMap<u64, String> {
        self.state
            .world
            .entities_with(&["mesh"])
            .into_iter()
            .filter_map(|id| {
                self.state
                    .world
                    .component(id, "mesh")?
                    .get("path")?
                    .as_str()
                    .map(|p| (id, p.into()))
            })
            .collect()
    }
    pub fn snapshot(&self) -> SceneSnapshot {
        let mut snapshot = self.provider.as_ref().map_or_else(
            || self.state.world.snapshot(),
            |provider| provider.snapshot(),
        );
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
        let context = ComponentValidationContext {
            scenes: &self.project.scenes,
            modules: &self.validation_modules,
            asset_paths: self.asset_paths.as_ref(),
            active_components: Some(&self.active_components),
        };
        let mut transaction = WorldTickTransaction::new(
            self.provider
                .as_mut()
                .map(|provider| provider.as_mut() as &mut dyn WorldProvider),
            self.registry.components(),
            &self.active_components,
            context,
        )?;
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
            let context = ComponentValidationContext {
                scenes: &self.project.scenes,
                modules: &self.validation_modules,
                asset_paths: self.asset_paths.as_ref(),
                active_components: Some(&self.active_components),
            };
            let mut transaction = match WorldTickTransaction::new(
                self.provider
                    .as_mut()
                    .map(|provider| provider.as_mut() as &mut dyn WorldProvider),
                self.registry.components(),
                &self.active_components,
                context,
            ) {
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
            .world
            .entities_with(&["persistent"])
            .into_iter()
            .filter_map(|id| self.state.world.entity(id))
            .collect();
        let spawn = self.state.transition_spawn;
        let context = ComponentValidationContext {
            scenes: &self.project.scenes,
            modules: &self.validation_modules,
            asset_paths: self.asset_paths.as_ref(),
            active_components: Some(&self.active_components),
        };
        let mut next = Self::load_state(
            &self.project,
            id,
            self.state.time,
            self.state.tick,
            &self.registry,
            &self.active_components,
            &context,
        )?;
        for mut entity in carried {
            if let Some(destination) = next.world.entity(entity.object.id) {
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
            next.world.upsert_entity(entity)?;
        }
        self.registry.components().validate_entities(
            &next.world.entities_snapshot(),
            &self.active_components,
            &context,
        )?;
        next.world.validate_references()?;
        let provider = self
            .world_factory
            .as_ref()
            .map(|factory| Self::load_provider(factory, &next))
            .transpose()?;
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
            self.validation_modules.clone(),
            self.asset_paths.clone(),
            Some(self.active_components.clone()),
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
    provider: Option<&'a mut dyn WorldProvider>,
    components: &'a crate::ComponentRegistry,
    active: &'a BTreeSet<String>,
    context: ComponentValidationContext<'a>,
    before: Vec<WorldObject>,
    commands: Vec<WorldCommand>,
    applied: bool,
}
impl<'a> WorldTickTransaction<'a> {
    fn new(
        provider: Option<&'a mut dyn WorldProvider>,
        components: &'a crate::ComponentRegistry,
        active: &'a BTreeSet<String>,
        context: ComponentValidationContext<'a>,
    ) -> Result<Self, String> {
        let before = provider
            .as_ref()
            .map(|provider| provider.objects())
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            provider,
            components,
            active,
            context,
            before,
            commands: vec![],
            applied: false,
        })
    }
}
impl TickTransaction for WorldTickTransaction<'_> {
    fn validate(&mut self, candidate: &GameState) -> Result<(), String> {
        candidate.world.validate_references()?;
        let objects = candidate.world.objects_snapshot();
        crate::validate_world_commands(&objects, &[])?;
        self.components.validate_entities(
            &candidate.world.entities_snapshot(),
            self.active,
            &self.context,
        )?;
        if let Some(provider) = self.provider.as_ref() {
            self.commands = world_command_diff(&self.before, &objects)?;
            provider.validate_commands(&self.commands)?;
        }
        Ok(())
    }
    fn commit(&mut self, candidate: &GameState) -> Result<(), String> {
        if self.commands.is_empty() {
            return Ok(());
        }
        let Some(provider) = self.provider.as_deref_mut() else {
            return Ok(());
        };
        self.applied = true;
        provider.apply_commands(&self.commands)?;
        let expected = candidate.world.objects_snapshot();
        if object_map(&provider.objects()?) != object_map(&expected) {
            return Err(
                "World provider did not apply the validated portable command packet".into(),
            );
        }
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), String> {
        if self.applied
            && let Some(provider) = self.provider.as_deref_mut()
        {
            let commands = world_command_diff(&provider.objects()?, &self.before)?;
            provider.validate_commands(&commands)?;
            provider.apply_commands(&commands)?;
            self.applied = false;
        }
        Ok(())
    }
}

fn compatibility_factory(kind: WorldKind) -> Result<Option<GameWorldFactory>, String> {
    match kind {
        WorldKind::Ecs => Ok(None),
        WorldKind::Objects => Ok(Some(Arc::new(|objects| {
            make_provider(WorldKind::Objects, objects)
        }))),
        WorldKind::Custom => Err("Custom worlds need an explicit compatibility factory".into()),
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
#[cfg(test)]
fn component<T: for<'de> Deserialize<'de>>(entity: &GameEntity, name: &str) -> Option<T> {
    serde_json::from_value(entity.components.get(name)?.clone()).ok()
}
fn world_component<T: for<'de> Deserialize<'de>>(
    world: &EcsWorld,
    id: u64,
    name: &str,
) -> Option<T> {
    serde_json::from_value(world.component(id, name)?.clone()).ok()
}
#[cfg(test)]
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

#[path = "builtin_game_systems.rs"]
mod builtin;
pub(crate) use builtin::register_game_systems;

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
        assert!(project.validate().is_ok());
        for path in [
            "asset://",
            "../outside.glb",
            "C:/outside.glb",
            "asset://bad\nID",
        ] {
            project.scenes.get_mut("arena").unwrap().objects[0]
                .components
                .insert("mesh".into(), serde_json::json!({"path":path}));
            assert!(project.validate().is_err(), "accepted {path:?}");
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
                assert!(session.state().world.is_empty());
                assert!(
                    session
                        .provider
                        .as_ref()
                        .map(|provider| provider.objects().unwrap())
                        .unwrap_or_else(|| session.state().world.objects_snapshot())
                        .is_empty()
                );
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
                assert!(session.state().world.is_empty());
                assert!(
                    session
                        .provider
                        .as_ref()
                        .map(|provider| provider.objects().unwrap())
                        .unwrap_or_else(|| session.state().world.objects_snapshot())
                        .is_empty()
                );
                session
                    .apply_commands(&[WorldCommand::Spawn {
                        object: test_object(10, None).object,
                    }])
                    .unwrap();
                session
                    .state
                    .world
                    .insert_component(10, "health", json!({"maximum":10.,"current":0.}))
                    .unwrap();
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
            assert!(session.state().world.is_empty());
            assert!(
                session
                    .provider
                    .as_ref()
                    .map(|provider| provider.objects().unwrap())
                    .unwrap_or_else(|| session.state().world.objects_snapshot())
                    .is_empty()
            );
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
            let carried = &session.state().world.entity(2).unwrap();
            assert_eq!(carried.object, destination.object);
            assert_eq!(carried.references, destination.references);
            assert_eq!(health(carried), 6.);
            assert!(carried.components.contains_key("persistent"));
            let absent = &session.state().world.entity(4).unwrap();
            assert_eq!(absent.object.parent, None);
            assert!(absent.references.is_empty());
            assert_eq!(
                object_map(
                    &session
                        .provider
                        .as_ref()
                        .map(|provider| provider.objects().unwrap())
                        .unwrap_or_else(|| session.state().world.objects_snapshot())
                ),
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
    fn strict_session_asset_context_is_owned_across_transition_and_restart() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut project = persistent_hierarchy_project();
            project.scenes.get_mut("source").unwrap().objects[0]
                .components
                .insert("mesh".into(), json!({"path":"asset://model"}));
            let registry = SystemRegistry::builtin();
            let modules = registry.installed_modules();
            let mut paths = BTreeMap::from([("model".into(), "assets/model.glb".into())]);
            let context = ComponentValidationContext {
                scenes: &project.scenes,
                modules: &modules,
                asset_paths: Some(&paths),
                active_components: None,
            };
            let mut session = GameSession::new_with_registry_and_context(
                project.clone(),
                kind,
                registry.clone(),
                &context,
            )
            .unwrap();
            paths.clear();
            session.transition("destination").unwrap();
            session.restart().unwrap();
            assert_eq!(session.state().scene, "source");
            assert_eq!(session.asset_bindings()[&1], "asset://model");
            let empty_context = ComponentValidationContext {
                scenes: &project.scenes,
                modules: &modules,
                asset_paths: Some(&paths),
                active_components: None,
            };
            let error = match GameSession::new_with_registry_and_context(
                project.clone(),
                kind,
                registry,
                &empty_context,
            ) {
                Ok(_) => panic!("Strict empty manifest accepted an unknown asset ID"),
                Err(error) => error,
            };
            assert!(error.contains("Unknown asset ID"));
        }
    }
    struct InvalidHealthWriter;
    impl ExecutableSystem for InvalidHealthWriter {
        fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            ctx.game.world.component_mut(10, "health").unwrap()["current"] = json!(-1.);
            ctx.game.world.transform_mut(10).unwrap().translation = [9., 0., 0.];
            ctx.game.world.remove_component(10, "material");
            ctx.game
                .world
                .insert_component(10, "scratch", json!({"value":7}))
                .unwrap();
            ctx.game
                .world
                .references_mut(10)
                .unwrap()
                .insert("self".into(), 10);
            ctx.game.apply_commands(&[
                WorldCommand::Remove { id: 20 },
                WorldCommand::Spawn {
                    object: test_object(20, None).object,
                },
            ])?;
            ctx.events.push(GameEvent::Victory);
            *state = json!("invalid candidate");
            Ok(())
        }
    }
    #[test]
    fn schema_invalid_tick_and_dangling_runtime_references_do_not_publish() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut registry = SystemRegistry::builtin();
            let mut descriptor = test_system_descriptor("test.invalid-health");
            descriptor.components.push("health".into());
            registry
                .register(descriptor, |_| Ok(Box::new(InvalidHealthWriter)))
                .unwrap();
            let mut original = dead_object(10, None);
            original.components.insert("material".into(), json!({}));
            original.references.insert("target".into(), 20);
            let mut project = focused_project(vec![original, test_object(20, None)], &[]);
            append_test_system(&mut project, "test.invalid-health");
            let mut session = GameSession::new_with_registry(project, kind, registry).unwrap();
            let before = session.state().clone();
            let lifetime = session.state().world.lifetime(20);
            let snapshot = session.snapshot();
            let systems = session.system_states();
            assert!(
                session
                    .step(1. / 60., &GameInput::default())
                    .unwrap_err()
                    .contains("health.current")
            );
            assert_eq!(session.state(), &before);
            assert_eq!(session.state().world.lifetime(20), lifetime);
            assert_eq!(session.snapshot(), snapshot);
            assert_eq!(session.system_states(), systems);
            assert!(session.events().is_empty());

            let mut survivor = test_object(20, None);
            survivor.references.insert("target".into(), 10);
            let project = focused_project(vec![test_object(10, None), survivor], &[]);
            let mut session = GameSession::new(project, kind).unwrap();
            let before = session.state().clone();
            let snapshot = session.snapshot();
            assert!(
                session
                    .apply_commands(&[WorldCommand::Remove { id: 10 }])
                    .unwrap_err()
                    .contains("clear references")
            );
            assert_eq!(session.state(), &before);
            assert_eq!(session.snapshot(), snapshot);
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
            ctx.game.world.clear();
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
        assert_eq!(health(&s.state.world.entity(10).unwrap()), 8.);
        assert_eq!(health(&s.state.world.entity(11).unwrap()), 8.);
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
        session.state.world.remove_component(10, "health");
        let aim = [0., 0., 3.];
        let area = session.state().melee_aim(1, aim).unwrap();
        assert!(
            area.contains(
                session
                    .state
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
        assert!(session.state().world.contains(10));
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
            assert!(!session.state().world.contains(10));
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
        assert!(objects.state().world.contains(10));
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
    fn canonical_world_preserves_entity_wire_shape_and_rejects_malformed_ids() {
        let session = GameSession::new(project_without_enemies(), WorldKind::Ecs).unwrap();
        let value = serde_json::to_value(session.state()).unwrap();
        assert!(value.get("world").is_none());
        assert_eq!(
            value["entities"].as_object().unwrap().len(),
            session.state().world.len()
        );
        let restored: GameState = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(&restored, session.state());
        assert_ne!(
            restored.world.lifetime(1),
            session.state().world.lifetime(1)
        );
        let mut mismatch = value.clone();
        mismatch["entities"]["1"]["object"]["id"] = json!(999);
        assert!(serde_json::from_value::<GameState>(mismatch).is_err());
        let mut zero = value.clone();
        let mut entity = zero["entities"]
            .as_object_mut()
            .unwrap()
            .remove("1")
            .unwrap();
        entity["object"]["id"] = json!(0);
        zero["entities"]["0"] = entity;
        assert!(serde_json::from_value::<GameState>(zero).is_err());
        let entity = serde_json::to_string(&value["entities"]["1"]).unwrap();
        let duplicate = serde_json::to_string(&value).unwrap().replacen(
            "\"entities\":{",
            &format!("\"entities\":{{\"1\":{entity},"),
            1,
        );
        assert!(serde_json::from_str::<GameState>(&duplicate).is_err());
    }
    #[test]
    fn default_ecs_session_keeps_no_secondary_provider_through_scene_lifecycle() {
        let mut session = GameSession::new(project_without_enemies(), WorldKind::Ecs).unwrap();
        assert!(session.provider.is_none());
        assert!(session.world_factory.is_none());
        session.step(1. / 60., &GameInput::default()).unwrap();
        session.transition("boss").unwrap();
        session.restart().unwrap();
        assert!(session.provider.is_none());
        assert!(session.world_factory.is_none());
        let mut expected = session.state().world.snapshot();
        expected.time = session.state().time as f32;
        assert_eq!(session.snapshot(), expected);
    }
    #[test]
    fn portal_transfers_health_and_boss_warns_before_damage() {
        let p = project_without_enemies();
        let mut s = GameSession::new(p, WorldKind::Ecs).unwrap();
        s.state.world.component_mut(1, "health").unwrap()["current"] = json!(60.);
        s.apply_commands(&[WorldCommand::SetPosition {
            id: 1,
            position: [0., 0., -7.2],
        }])
        .unwrap();
        s.step(1. / 60., &GameInput::default()).unwrap();
        assert_eq!(s.state.scene, "boss");
        assert_eq!(health(&s.state.player().unwrap()), 60.);
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
        assert_eq!(health(&s.state.player().unwrap()), 60.);
        s.apply_commands(&[WorldCommand::SetPosition {
            id: 1,
            position: [7., 0., 5.],
        }])
        .unwrap();
        for _ in 0..70 {
            s.step(1. / 60., &GameInput::default()).unwrap();
        }
        assert_eq!(health(&s.state.player().unwrap()), 60.);
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
            !session
                .state()
                .world
                .entity(10)
                .unwrap()
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
        registry
            .components_mut()
            .register(crate::ComponentSchema {
                id: "replication".into(),
                owner_module: "test.declared-data".into(),
                version: "1".into(),
                label: "Replication".into(),
                defaults: json!({"enabled":true,"channel":0}),
                fields: vec![],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        let restored =
            GameSession::new_with_registry(project.clone(), WorldKind::Ecs, registry.clone())
                .unwrap();
        assert_eq!(
            restored.state().world.entity(10).unwrap().components["replication"]["enabled"],
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
