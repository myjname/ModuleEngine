//! Fixed-step, provider-neutral gameplay. Behaviors are separately registered
//! systems; scene names, encounters and progression are authored data.
use crate::{
    ComponentValidationContext, EcsWorld, GameProject, ObjectsWorld, SceneSnapshot, SystemExecutor,
    SystemRegistry, TickTransaction, WorldCommand, WorldKind, WorldObject, WorldProvider,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Module-defined input values and pressed action names. Restart is a host control.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameInput {
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
    #[serde(default)]
    pub actions: BTreeSet<String>,
    /// Module-selected values captured with a pressed packet. Held values above
    /// remain the latest input when a queued action is finally consumed.
    #[serde(default)]
    pub action_values: BTreeMap<String, Value>,
    #[serde(default)]
    pub restart: bool,
}
impl GameInput {
    pub fn pressed(&self, name: &str) -> bool {
        self.actions.contains(name)
    }
}

/// Portable module event. The externally tagged wire representation is generic:
/// a null payload is a string; other payloads use a single named map entry.
#[derive(Clone, Debug, PartialEq)]
pub struct GameEvent {
    pub name: String,
    pub payload: Value,
}
impl GameEvent {
    /// Request a presentation snap after this tick commits, for example after a
    /// teleport. Does not change simulation, scene identity or the event wire format.
    pub fn presentation_reset() -> Self {
        Self::new(crate::PRESENTATION_RESET_EVENT, Value::Null)
    }
    pub fn new(name: impl Into<String>, payload: Value) -> Self {
        Self {
            name: name.into(),
            payload,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            Err("Event name must be nonempty".into())
        } else {
            Ok(())
        }
    }
}
impl Serialize for GameEvent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        if self.payload.is_null() {
            serializer.serialize_str(&self.name)
        } else {
            serializer.collect_map(std::iter::once((&self.name, &self.payload)))
        }
    }
}
impl<'de> Deserialize<'de> for GameEvent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EventVisitor;
        impl<'de> serde::de::Visitor<'de> for EventVisitor {
            type Value = GameEvent;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an event name string or a single named payload")
            }
            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<GameEvent, E> {
                let event = GameEvent::new(name, Value::Null);
                event.validate().map_err(E::custom)?;
                Ok(event)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<GameEvent, A::Error> {
                let (name, payload) = map
                    .next_entry::<String, Value>()?
                    .ok_or_else(|| serde::de::Error::custom("Event map must have one entry"))?;
                if map.next_entry::<String, Value>()?.is_some() {
                    return Err(serde::de::Error::custom(
                        "Event map must have exactly one entry",
                    ));
                }
                let event = GameEvent::new(name, payload);
                event.validate().map_err(serde::de::Error::custom)?;
                Ok(event)
            }
        }
        deserializer.deserialize_any(EventVisitor)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionControl {
    pub halted: bool,
    pub transition: Option<SceneTransition>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneTransition {
    pub destination: String,
    #[serde(default)]
    pub carry: BTreeSet<u64>,
    /// Replace the local translation of every explicitly carried entity.
    #[serde(default)]
    pub spawn: Option<[f32; 3]>,
}

/// Mutable session data. Module-owned state is opaque to the runtime host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameState {
    pub scene: String,
    pub time: f64,
    pub tick: u64,
    #[serde(rename = "entities", with = "crate::world::entity_dto_map")]
    pub world: EcsWorld,
    #[serde(default)]
    pub module_data: BTreeMap<String, Value>,
    #[serde(default)]
    pub control: SessionControl,
}
impl GameState {
    pub fn apply_commands(&mut self, commands: &[WorldCommand]) -> Result<(), String> {
        self.world.apply_commands(commands)
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
    pending_actions: BTreeSet<String>,
    pending_action_values: BTreeMap<String, Value>,
    world_factory: Option<GameWorldFactory>,
    active_components: BTreeSet<String>,
    validation_modules: BTreeSet<String>,
    asset_paths: Option<BTreeMap<String, String>>,
}

type GameWorldFactory =
    Arc<dyn Fn(Vec<WorldObject>) -> Result<Box<dyn WorldProvider>, String> + Send + Sync>;

struct PreparedSceneTransition {
    state: GameState,
    provider: Option<Box<dyn WorldProvider>>,
    executor: SystemExecutor,
}

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
        validate_game_state(&project, &registry, &state)?;
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
            pending_actions: BTreeSet::new(),
            pending_action_values: BTreeMap::new(),
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
            module_data: BTreeMap::new(),
            control: SessionControl::default(),
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
            &self.registry,
            &self.active_components,
            context,
            &self.project,
            self.world_factory.as_ref(),
            true,
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
        if !dt.is_finite() || dt < 0. {
            return Err("Session dt must be finite and nonnegative".into());
        }
        self.events.clear();
        if input.restart {
            self.restart()?;
            return Ok(0);
        }
        if self.state.control.halted {
            return Ok(0);
        }
        self.pending_actions.extend(input.actions.iter().cloned());
        if !input.actions.is_empty() {
            self.pending_action_values.extend(
                input
                    .action_values
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }
        self.accumulator = (self.accumulator + dt).min(0.25);
        let mut ticks = 0;
        while ticks < max_ticks && self.accumulator + 1e-12 >= 1. / 60. {
            let mut fixed_input = input.clone();
            fixed_input.actions = std::mem::take(&mut self.pending_actions);
            fixed_input.action_values = std::mem::take(&mut self.pending_action_values);
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
                &self.registry,
                &self.active_components,
                context,
                &self.project,
                self.world_factory.as_ref(),
                false,
            ) {
                Ok(transaction) => transaction,
                Err(error) => {
                    self.pending_actions.extend(fixed_input.actions);
                    self.pending_action_values.extend(fixed_input.action_values);
                    return Err(error);
                }
            };
            let mut tick_events = vec![];
            if let Err(error) = self.executor.step_with_transaction(
                &mut self.state,
                &fixed_input,
                1. / 60.,
                &mut tick_events,
                &mut transaction,
            ) {
                self.state = before_tick;
                self.pending_actions.extend(fixed_input.actions);
                self.pending_action_values.extend(fixed_input.action_values);
                return Err(error);
            }
            let transition = transaction.prepared_transition.take();
            drop(transaction);
            self.events.extend(tick_events);
            self.state.tick += 1;
            self.state.time += 1. / 60.;
            self.accumulator -= 1. / 60.;
            ticks += 1;
            if let Some(transition) = transition {
                self.publish_transition(transition)?;
            }
            if self.state.control.halted {
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
    /// Reload a scene without carrying source entities. Modules request explicit
    /// carry IDs through transition_with when their rules require persistence.
    pub fn transition(&mut self, id: &str) -> Result<(), String> {
        self.transition_with(SceneTransition {
            destination: id.into(),
            carry: BTreeSet::new(),
            spawn: None,
        })
    }
    /// Explicit carry IDs retain module component data while destination
    /// parents/transforms/references remain local to the destination scene.
    pub fn transition_with(&mut self, request: SceneTransition) -> Result<(), String> {
        let context = ComponentValidationContext {
            scenes: &self.project.scenes,
            modules: &self.validation_modules,
            asset_paths: self.asset_paths.as_ref(),
            active_components: Some(&self.active_components),
        };
        let transition = Self::prepare_transition(
            &self.project,
            &self.registry,
            self.world_factory.as_ref(),
            &self.active_components,
            &context,
            &self.state,
            &request,
        )?;
        self.publish_transition(transition)
    }
    fn prepare_transition(
        project: &GameProject,
        registry: &SystemRegistry,
        world_factory: Option<&GameWorldFactory>,
        active_components: &BTreeSet<String>,
        context: &ComponentValidationContext<'_>,
        source: &GameState,
        request: &SceneTransition,
    ) -> Result<PreparedSceneTransition, String> {
        validate_scene_transition(request, &source.world, &project.scenes)?;
        let carried = request
            .carry
            .iter()
            .map(|id| {
                source
                    .world
                    .entity(*id)
                    .ok_or_else(|| format!("Cannot carry missing entity {id}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut next = Self::load_state(
            project,
            &request.destination,
            source.time,
            source.tick,
            registry,
            active_components,
            context,
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
            if let Some(pos) = request.spawn {
                entity.object.transform.translation = pos;
            }
            next.world.upsert_entity(entity)?;
        }
        registry.components().validate_entities(
            &next.world.entities_snapshot(),
            active_components,
            context,
        )?;
        next.world.validate_references()?;
        validate_game_state(project, registry, &next)?;
        let provider = world_factory
            .map(|factory| Self::load_provider(factory, &next))
            .transpose()?;
        let executor = SystemExecutor::new(&project.execution, registry)?;
        Ok(PreparedSceneTransition {
            state: next,
            provider,
            executor,
        })
    }
    fn publish_transition(
        &mut self,
        mut transition: PreparedSceneTransition,
    ) -> Result<(), String> {
        let from = self.state.scene.clone();
        // Tick-triggered candidates were validated before source publication.
        // Their new scene starts at the now-committed source tick's clock.
        transition.state.time = self.state.time;
        transition.state.tick = self.state.tick;
        let destination = transition.state.scene.clone();
        // Stop hooks can fail after mutating their context. Keep the visible
        // scene/events until replacement succeeds; external shutdown effects
        // still follow the executor's best-effort, exactly-once contract.
        let mut stopped_state = self.state.clone();
        let mut stopped_events = self.events.clone();
        self.executor
            .stop(&mut stopped_state, &mut stopped_events)?;
        self.state = transition.state;
        self.provider = transition.provider;
        self.executor = transition.executor;
        self.events = stopped_events;
        self.events.push(GameEvent::new(
            "SceneChanged",
            serde_json::json!({"from":from,"to":destination}),
        ));
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

fn validate_game_state(
    project: &GameProject,
    registry: &SystemRegistry,
    state: &GameState,
) -> Result<(), String> {
    for contribution in registry.active_game_contributions(&project.execution) {
        (contribution.validate_state)(state).map_err(|error| {
            format!(
                "Game contribution '{}' state validation: {error}",
                contribution.id
            )
        })?;
    }
    Ok(())
}

fn object_map(objects: &[WorldObject]) -> BTreeMap<u64, WorldObject> {
    objects.iter().map(|o| (o.id, o.clone())).collect()
}
fn validate_scene_transition(
    request: &SceneTransition,
    world: &EcsWorld,
    scenes: &BTreeMap<String, crate::AuthoredScene>,
) -> Result<(), String> {
    if !scenes.contains_key(&request.destination) {
        return Err(format!(
            "Transition destination '{}' is missing",
            request.destination
        ));
    }
    if let Some(id) = request.carry.iter().find(|id| !world.contains(**id)) {
        return Err(format!("Cannot carry missing entity {id}"));
    }
    if request
        .spawn
        .is_some_and(|position| position.iter().any(|value| !value.is_finite()))
    {
        return Err("Transition spawn must be finite".into());
    }
    Ok(())
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
    registry: &'a SystemRegistry,
    active: &'a BTreeSet<String>,
    context: ComponentValidationContext<'a>,
    project: &'a GameProject,
    world_factory: Option<&'a GameWorldFactory>,
    prepared_transition: Option<PreparedSceneTransition>,
    validate_state: bool,
    before: Vec<WorldObject>,
    commands: Vec<WorldCommand>,
    applied: bool,
}
impl<'a> WorldTickTransaction<'a> {
    fn new(
        provider: Option<&'a mut dyn WorldProvider>,
        registry: &'a SystemRegistry,
        active: &'a BTreeSet<String>,
        context: ComponentValidationContext<'a>,
        project: &'a GameProject,
        world_factory: Option<&'a GameWorldFactory>,
        validate_state: bool,
    ) -> Result<Self, String> {
        let before = provider
            .as_ref()
            .map(|provider| provider.objects())
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            provider,
            registry,
            active,
            context,
            project,
            world_factory,
            prepared_transition: None,
            validate_state,
            before,
            commands: vec![],
            applied: false,
        })
    }
}
impl TickTransaction for WorldTickTransaction<'_> {
    fn validate(&mut self, candidate: &GameState) -> Result<(), String> {
        if let Some(request) = &candidate.control.transition {
            validate_scene_transition(request, &candidate.world, self.context.scenes)?;
        }
        candidate.world.validate_references()?;
        let objects = candidate.world.objects_snapshot();
        crate::validate_world_commands(&objects, &[])?;
        self.registry.components().validate_entities(
            &candidate.world.entities_snapshot(),
            self.active,
            &self.context,
        )?;
        // SystemExecutor already guards the same staged state on ordinary
        // ticks. Direct session commands have no executor, so guard them here.
        if self.validate_state {
            validate_game_state(self.project, self.registry, candidate)?;
        }
        if let Some(provider) = self.provider.as_ref() {
            self.commands = world_command_diff(&self.before, &objects)?;
            provider.validate_commands(&self.commands)?;
        }
        self.prepared_transition = candidate
            .control
            .transition
            .as_ref()
            .map(|request| {
                GameSession::prepare_transition(
                    self.project,
                    self.registry,
                    self.world_factory,
                    self.active,
                    &self.context,
                    candidate,
                    request,
                )
            })
            .transpose()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuthoredObject, AuthoredScene, ComponentSchema, ExecutableSystem, PropertySchema,
        PropertyScope, PropertyType, ResourceAccess, SystemContext, SystemDescriptor, SystemNode,
        SystemPort, SystemResource, SystemStage, Transform,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Clone, Default)]
    struct FixtureControl {
        reject: Arc<AtomicBool>,
        invalid: Arc<AtomicBool>,
        invalid_request: Arc<AtomicUsize>,
        reject_stop: Arc<AtomicBool>,
        reject_factory: Arc<AtomicBool>,
        published: Arc<AtomicUsize>,
        constructed: Arc<AtomicUsize>,
        started: Arc<AtomicUsize>,
        stopped: Arc<AtomicUsize>,
    }
    struct FixtureSystem {
        control: FixtureControl,
        before: usize,
        pending: usize,
    }
    impl ExecutableSystem for FixtureSystem {
        fn initialize(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            self.control.started.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            self.before = self.control.published.load(Ordering::SeqCst);
            self.pending = self.before + 1;
            *state = json!(state.as_u64().unwrap_or(0) + 1);
            ctx.game.module_data.insert(
                "fixture.frame/1".into(),
                json!({"tick":state,"actions":ctx.input.actions,"action_values":ctx.input.action_values,"values":ctx.input.values}),
            );
            let counter = ctx.resource("counter")?.as_u64().unwrap_or(0) + 1;
            ctx.set_resource("counter", json!(counter))?;
            if let Some(transform) = ctx.game.world.transform_mut(1) {
                transform.translation[0] += ctx
                    .input
                    .values
                    .get("move")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.) as f32
                    * ctx.dt;
            }
            if ctx.input.pressed("pulse") {
                ctx.events
                    .push(GameEvent::new("Pulse", json!({"object":1})));
            }
            if ctx.input.pressed("halt") {
                ctx.game.control.halted = true;
            }
            if ctx.input.pressed("transition") {
                ctx.game.control.transition = Some(SceneTransition {
                    destination: "destination".into(),
                    carry: BTreeSet::from([1, 2]),
                    spawn: None,
                });
            }
            if ctx.input.pressed("bad-transition") {
                let failure = self.control.invalid_request.load(Ordering::SeqCst);
                ctx.game.control.transition = Some(SceneTransition {
                    destination: if failure == 1 {
                        "missing"
                    } else {
                        "destination"
                    }
                    .into(),
                    carry: if failure == 2 {
                        BTreeSet::from([999])
                    } else {
                        BTreeSet::from([1])
                    },
                    spawn: if failure == 3 {
                        Some([f32::NAN, 0., 0.])
                    } else {
                        None
                    },
                });
            }
            if ctx.input.pressed("invalid") && self.control.invalid.load(Ordering::SeqCst) {
                ctx.game.world.component_mut(1, "fixture.data").unwrap()["amount"] = json!(-1.);
                ctx.game
                    .world
                    .insert_component(1, "scratch", json!({"value":7}))?;
                ctx.game
                    .world
                    .references_mut(1)
                    .unwrap()
                    .insert("self".into(), 1);
                ctx.game.apply_commands(&[
                    WorldCommand::Remove { id: 2 },
                    WorldCommand::Spawn {
                        object: object(2, None).object,
                    },
                ])?;
            }
            Ok(())
        }
        fn commit_tick(&mut self) -> Result<(), String> {
            self.control.published.store(self.pending, Ordering::SeqCst);
            if self.control.reject.load(Ordering::SeqCst) {
                Err("Fixture publication rejected".into())
            } else {
                Ok(())
            }
        }
        fn rollback_tick(&mut self) -> Result<(), String> {
            self.control.published.store(self.before, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            self.control.stopped.fetch_add(1, Ordering::SeqCst);
            if self.control.reject_stop.load(Ordering::SeqCst) {
                ctx.game.scene = "discarded stop edit".into();
                ctx.game.world.clear();
                ctx.events.push(GameEvent::new("Stopped", Value::Null));
                Err("Fixture stop rejected".into())
            } else {
                Ok(())
            }
        }
    }
    fn registry(control: FixtureControl) -> SystemRegistry {
        let mut registry = SystemRegistry::builtin();
        registry.install_module("fixture.owner", "1").unwrap();
        registry
            .components_mut()
            .register(ComponentSchema {
                id: "fixture.data".into(),
                owner_module: "fixture.owner".into(),
                version: "1".into(),
                label: "Fixture data".into(),
                defaults: json!({"amount":10.}),
                fields: vec![PropertySchema {
                    id: "fixture.data.amount".into(),
                    label: "Amount".into(),
                    scope: PropertyScope::Component {
                        component: "fixture.data".into(),
                    },
                    value_type: PropertyType::Number { min: 0., max: 100. },
                    default: json!(10.),
                    requires_modules: vec![],
                    requires_components: vec![],
                }],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        registry
            .register(
                SystemDescriptor {
                    id: "fixture.step".into(),
                    state_schema: "fixture/1".into(),
                    components: vec!["fixture.data".into()],
                    ports: vec![SystemPort {
                        name: "counter".into(),
                        data_type: "Number".into(),
                        access: ResourceAccess::ReadWrite,
                    }],
                    requires: vec![],
                },
                move |_| {
                    control.constructed.fetch_add(1, Ordering::SeqCst);
                    if control.reject_factory.load(Ordering::SeqCst) {
                        return Err("Destination system factory rejected".into());
                    }
                    Ok(Box::new(FixtureSystem {
                        control: control.clone(),
                        before: 0,
                        pending: 0,
                    }))
                },
            )
            .unwrap();
        registry
    }
    fn guarded_registry(control: FixtureControl) -> SystemRegistry {
        let mut registry = registry(control);
        registry
            .register_game_contribution(crate::GameContribution {
                id: "fixture.layout".into(),
                activation_modules: BTreeSet::from(["fixture.step".into()]),
                begin_tick: |_| Ok(()),
                initial_camera: |_| crate::Camera::default(),
                follow_target: |_| None,
                decorate_snapshot: |_, _| {},
                script_input: |input| serde_json::to_value(input).unwrap(),
                validate_event: |_| Ok(()),
                validate_state: |state| {
                    if state.world.entities_snapshot().len() == 2 {
                        Ok(())
                    } else {
                        Err("Fixture layout requires exactly two entities".into())
                    }
                },
                diagnostics: |_| json!({}),
                service_requests: |_| Ok(vec![]),
            })
            .unwrap();
        registry
    }
    fn object(id: u64, parent: Option<u64>) -> AuthoredObject {
        AuthoredObject::new(WorldObject {
            id,
            parent,
            transform: Transform::default(),
            color: [80, 120, 160, 255],
        })
    }
    fn project() -> GameProject {
        let mut first = object(1, None);
        first
            .components
            .insert("fixture.data".into(), json!({"amount":6.}));
        first.references.insert("target".into(), 2);
        let mut destination = object(1, Some(3));
        destination.object.transform.translation = [4., 0., 1.];
        destination
            .components
            .insert("fixture.data".into(), json!({"amount":100.}));
        destination.references.insert("target".into(), 3);
        let mut parent = object(3, None);
        parent.object.transform.translation = [20., 0., 0.];
        GameProject {
            entry_scene: "source".into(),
            scenes: BTreeMap::from([
                (
                    "source".into(),
                    AuthoredScene {
                        name: "Source".into(),
                        objects: vec![first, object(2, None)],
                    },
                ),
                (
                    "destination".into(),
                    AuthoredScene {
                        name: "Destination".into(),
                        objects: vec![parent, destination],
                    },
                ),
            ]),
            prefabs: BTreeMap::new(),
            execution: crate::ExecutionGraph {
                resources: vec![SystemResource {
                    name: "counter".into(),
                    data_type: "Number".into(),
                    external: true,
                    initial: json!(0),
                }],
                stages: vec![SystemStage {
                    name: "Fixture".into(),
                    nodes: vec![SystemNode {
                        id: "step".into(),
                        module: "fixture.step".into(),
                        params: Value::Null,
                        bindings: BTreeMap::from([("counter".into(), "counter".into())]),
                        after: vec![],
                    }],
                }],
            },
            audio_cues: BTreeMap::new(),
        }
    }
    fn session(kind: WorldKind) -> GameSession {
        GameSession::new_with_registry(project(), kind, registry(FixtureControl::default()))
            .unwrap()
    }
    fn input(actions: &[&str], movement: f64) -> GameInput {
        GameInput {
            values: BTreeMap::from([("move".into(), json!(movement))]),
            actions: actions.iter().map(|name| (*name).into()).collect(),
            action_values: BTreeMap::new(),
            restart: false,
        }
    }

    #[test]
    fn generic_events_preserve_external_tags_and_reject_malformed_names_or_maps() {
        for event in [
            GameEvent::new("Ready", Value::Null),
            GameEvent::new("Move", json!({"object":1})),
        ] {
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(serde_json::from_value::<GameEvent>(value).unwrap(), event);
        }
        assert_eq!(
            serde_json::to_value(GameEvent::new("Ready", Value::Null)).unwrap(),
            json!("Ready")
        );
        assert_eq!(
            serde_json::to_value(GameEvent::new("Move", json!({"object":1}))).unwrap(),
            json!({"Move":{"object":1}})
        );
        for malformed in [
            r#""""#,
            r#"{}"#,
            r#"{"a":1,"b":2}"#,
            r#"{"a":1,"a":2}"#,
            r#"{"":1}"#,
            r#"[]"#,
            r#"1"#,
        ] {
            assert!(
                serde_json::from_str::<GameEvent>(malformed).is_err(),
                "accepted {malformed}"
            );
        }
    }
    #[test]
    fn fixed_clock_retains_pressed_actions_and_budgeted_catchup_has_world_parity() {
        let mut a = session(WorldKind::Objects);
        let mut b = session(WorldKind::Ecs);
        assert_eq!(a.step(0.001, &input(&["pulse"], 1.)).unwrap(), 0);
        b.step(0.001, &input(&["pulse"], 1.)).unwrap();
        a.step(1. / 60., &input(&[], 1.)).unwrap();
        b.step(1. / 60., &input(&[], 1.)).unwrap();
        assert_eq!(a.events(), &[GameEvent::new("Pulse", json!({"object":1}))]);
        assert_eq!(a.state(), b.state());
        assert_eq!(a.snapshot(), b.snapshot());
        assert_eq!(a.step_budget(0.25, &input(&[], 1.), 1).unwrap(), 1);
        assert!(a.pending_time() > 0.2);
        while a.pending_time() + 1e-12 >= 1. / 60. {
            a.step_budget(0., &input(&[], 1.), 1).unwrap();
        }
        b.step(0.25, &input(&[], 1.)).unwrap();
        assert_eq!(a.state(), b.state());
        assert_eq!(a.snapshot(), b.snapshot());
    }
    #[test]
    fn pressed_context_survives_short_frames_and_retry_without_replaying_old_held_values() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let control = FixtureControl::default();
            control.reject.store(true, Ordering::SeqCst);
            let mut session =
                GameSession::new_with_registry(project(), kind, registry(control.clone())).unwrap();
            let mut pressed = input(&["pulse"], 1.);
            pressed
                .action_values
                .insert("target".into(), json!([7., 0., 4.]));
            assert_eq!(session.step(0.001, &pressed).unwrap(), 0);
            assert!(session.step(1. / 60., &input(&[], 2.)).is_err());
            assert!(session.state().module_data.is_empty());
            control.reject.store(false, Ordering::SeqCst);
            assert_eq!(session.step_budget(0., &input(&[], 3.), 1).unwrap(), 1);
            let frame = &session.state().module_data["fixture.frame/1"];
            assert_eq!(frame["action_values"]["target"], json!([7., 0., 4.]));
            assert_eq!(frame["values"]["move"], 3.);
            assert!(
                (session
                    .state()
                    .world
                    .object(1)
                    .unwrap()
                    .transform
                    .translation[0]
                    - 0.05)
                    .abs()
                    < 1e-6
            );
            assert_eq!(
                session.events(),
                &[GameEvent::new("Pulse", json!({"object":1}))]
            );
            session.step(1. / 60., &input(&[], 4.)).unwrap();
            assert_eq!(
                session.state().module_data["fixture.frame/1"]["action_values"],
                json!({})
            );
            assert!(session.events().is_empty());
        }
    }
    #[test]
    fn invalid_native_transition_requests_roll_back_world_resources_state_and_events() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            for failure in 1..=3 {
                let control = FixtureControl::default();
                control.invalid_request.store(failure, Ordering::SeqCst);
                let mut session =
                    GameSession::new_with_registry(project(), kind, registry(control.clone()))
                        .unwrap();
                let before = session.state().clone();
                let snapshot = session.snapshot();
                let states = session.system_states();
                let resources = session.executor.resources().clone();
                let lifetime = session.state().world.lifetime(1);
                let error = session
                    .step(1. / 60., &input(&["pulse", "bad-transition"], 1.))
                    .unwrap_err();
                assert!(error.contains(match failure {
                    1 => "destination",
                    2 => "carry",
                    _ => "spawn",
                }));
                assert_eq!(session.state(), &before);
                assert_eq!(session.snapshot(), snapshot);
                assert_eq!(session.system_states(), states);
                assert_eq!(session.executor.resources(), &resources);
                assert!(session.events().is_empty());
                assert_eq!(session.state().world.lifetime(1), lifetime);
                assert_eq!(control.published.load(Ordering::SeqCst), 0);
            }
        }
    }
    #[test]
    fn transition_preflight_rejection_retains_tick_and_retries_with_one_destination_build() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            for failure in ["carried-reference", "world-factory", "system-factory"] {
                let control = FixtureControl::default();
                let mut project = project();
                let mut registry = registry(control.clone());
                let mut active = registry.active_components(&project.execution).unwrap();
                if failure == "carried-reference" {
                    registry
                        .components_mut()
                        .register(ComponentSchema {
                            id: "fixture.link".into(),
                            owner_module: "fixture.owner".into(),
                            version: "1".into(),
                            label: "Scene link".into(),
                            defaults: json!({"target":null}),
                            fields: vec![PropertySchema {
                                id: "fixture.link.target".into(),
                                label: "Target".into(),
                                scope: PropertyScope::Component {
                                    component: "fixture.link".into(),
                                },
                                value_type: PropertyType::ObjectReference {
                                    required_components: vec![],
                                    optional: true,
                                },
                                default: Value::Null,
                                requires_modules: vec![],
                                requires_components: vec![],
                            }],
                            requires_modules: vec![],
                            requires_components: vec![],
                        })
                        .unwrap();
                    active.insert("fixture.link".into());
                    project.scenes.get_mut("source").unwrap().objects[0]
                        .components
                        .insert("fixture.link".into(), json!({"target":2}));
                }
                let modules = registry.installed_modules();
                let context = ComponentValidationContext {
                    scenes: &project.scenes,
                    modules: &modules,
                    asset_paths: None,
                    active_components: Some(&active),
                };
                let factory_calls = Arc::new(AtomicUsize::new(0));
                let reject_factory = Arc::new(AtomicBool::new(false));
                let calls = factory_calls.clone();
                let reject = reject_factory.clone();
                let mut session = GameSession::new_with_world_factory_and_context(
                    project.clone(),
                    kind,
                    registry,
                    move |objects| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        if reject.load(Ordering::SeqCst) {
                            return Err("Destination factory rejected".into());
                        }
                        make_provider(kind, objects)
                    },
                    &context,
                )
                .unwrap();
                reject_factory.store(failure == "world-factory", Ordering::SeqCst);
                control
                    .reject_factory
                    .store(failure == "system-factory", Ordering::SeqCst);
                session
                    .state
                    .module_data
                    .insert("fixture.retained".into(), json!({"keep":7}));
                let before = session.state().clone();
                let snapshot = session.snapshot();
                let states = session.system_states();
                let resources = session.executor.resources().clone();
                let lifetime = session.state().world.lifetime(1);
                let mut pressed = input(&["pulse", "bad-transition"], 1.);
                pressed
                    .action_values
                    .insert("fixture.target".into(), json!(7));
                let error = session.step(1. / 60., &pressed).unwrap_err();
                assert!(
                    error.contains(match failure {
                        "carried-reference" => "references a missing scene object",
                        "world-factory" => "Destination factory rejected",
                        _ => "Destination system factory rejected",
                    }),
                    "{failure}: {error}"
                );
                assert_eq!(session.state(), &before);
                assert_eq!(session.snapshot(), snapshot);
                assert_eq!(session.system_states(), states);
                assert_eq!(session.executor.resources(), &resources);
                assert_eq!(session.state().world.lifetime(1), lifetime);
                assert!(session.events().is_empty());
                assert_eq!(session.pending_actions, pressed.actions);
                assert_eq!(session.pending_action_values, pressed.action_values);
                assert_eq!(control.published.load(Ordering::SeqCst), 0);
                assert_eq!(control.stopped.load(Ordering::SeqCst), 0);
                assert_eq!(session.pending_time(), 1. / 60.);
                if failure == "carried-reference" {
                    // Both scenes were valid independently; only the merged
                    // carry needed this destination-local referent.
                    session
                        .project
                        .scenes
                        .get_mut("destination")
                        .unwrap()
                        .objects
                        .push(object(2, None));
                } else {
                    reject_factory.store(false, Ordering::SeqCst);
                    control.reject_factory.store(false, Ordering::SeqCst);
                }
                let before_retry_calls = factory_calls.load(Ordering::SeqCst);
                let before_retry_constructors = control.constructed.load(Ordering::SeqCst);
                assert_eq!(
                    session.step_budget(0., &GameInput::default(), 1).unwrap(),
                    1
                );
                assert_eq!(
                    factory_calls.load(Ordering::SeqCst),
                    before_retry_calls + 1,
                    "Publish must reuse its validated provider/executor candidate"
                );
                assert_eq!(
                    control.constructed.load(Ordering::SeqCst),
                    before_retry_constructors + 1,
                    "Publish must reuse its validated system instances"
                );
                assert_eq!(session.state().scene, "destination");
                assert_eq!(session.state().tick, before.tick + 1);
                assert_eq!(session.state().time, before.time + 1. / 60.);
                assert_eq!(control.published.load(Ordering::SeqCst), 1);
                assert_eq!(control.stopped.load(Ordering::SeqCst), 1);
                assert!(session.pending_actions.is_empty());
                assert!(session.pending_action_values.is_empty());
                assert_eq!(
                    session.events(),
                    &[
                        GameEvent::new("Pulse", json!({"object":1})),
                        GameEvent::new("SceneChanged", json!({"from":"source","to":"destination"})),
                    ]
                );
            }
        }
    }
    #[test]
    fn failed_publication_restores_all_columns_lifetimes_module_data_events_and_input_edges() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let control = FixtureControl::default();
            control.reject.store(true, Ordering::SeqCst);
            let mut session =
                GameSession::new_with_registry(project(), kind, registry(control.clone())).unwrap();
            let before = session.state().clone();
            let lifetime = before.world.lifetime(2);
            let snapshot = session.snapshot();
            let states = session.system_states();
            assert!(
                session
                    .step(1. / 60., &input(&["pulse"], 1.))
                    .unwrap_err()
                    .contains("publication rejected")
            );
            assert_eq!(session.state(), &before);
            assert_eq!(session.snapshot(), snapshot);
            assert_eq!(session.system_states(), states);
            assert!(session.events().is_empty());
            assert_eq!(control.published.load(Ordering::SeqCst), 0);
            control.invalid.store(true, Ordering::SeqCst);
            assert!(
                session
                    .step_budget(0., &input(&["invalid"], 1.), 1)
                    .unwrap_err()
                    .contains("fixture.data.amount")
            );
            assert_eq!(session.state(), &before);
            assert_eq!(session.state().world.lifetime(2), lifetime);
            assert_eq!(session.system_states(), states);
            assert!(session.events().is_empty());
            control.invalid.store(false, Ordering::SeqCst);
            control.reject.store(false, Ordering::SeqCst);
            assert_eq!(
                session.step_budget(0., &GameInput::default(), 1).unwrap(),
                1
            );
            assert_eq!(
                session.events(),
                &[GameEvent::new("Pulse", json!({"object":1}))]
            );
        }
    }
    #[test]
    fn explicit_carry_preserves_data_and_uses_destination_hierarchy_or_detaches() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut session = session(kind);
            let authored = session.project().clone();
            session
                .transition_with(SceneTransition {
                    destination: "destination".into(),
                    carry: BTreeSet::from([1, 2]),
                    spawn: None,
                })
                .unwrap();
            let carried = session.state().world.entity(1).unwrap();
            assert_eq!(carried.object.parent, Some(3));
            assert_eq!(carried.object.transform.translation, [4., 0., 1.]);
            assert_eq!(carried.components["fixture.data"]["amount"], 6.);
            assert_eq!(carried.references["target"], 3);
            assert_eq!(session.state().world.object(2).unwrap().parent, None);
            assert!(session.state().world.references(2).unwrap().is_empty());
            assert_eq!(
                session
                    .snapshot()
                    .items
                    .iter()
                    .find(|item| item.id == 1)
                    .unwrap()
                    .model[0][3],
                24.
            );
            assert_eq!(session.project(), &authored);
            session.restart().unwrap();
            session.transition("destination").unwrap();
            assert_eq!(
                session.state().world.component(1, "fixture.data").unwrap()["amount"],
                100.
            );
            assert!(!session.state().world.contains(2));
            session.restart().unwrap();
            session
                .transition_with(SceneTransition {
                    destination: "destination".into(),
                    carry: BTreeSet::from([1, 2]),
                    spawn: Some([7., 0., 4.]),
                })
                .unwrap();
            assert_eq!(
                session
                    .state()
                    .world
                    .object(1)
                    .unwrap()
                    .transform
                    .translation,
                [7., 0., 4.]
            );
            assert_eq!(
                session
                    .state()
                    .world
                    .object(2)
                    .unwrap()
                    .transform
                    .translation,
                [7., 0., 4.]
            );
        }
    }
    #[test]
    fn invalid_carry_or_dangling_reference_removal_retains_current_scene() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut session = session(kind);
            let before = session.state().clone();
            let snapshot = session.snapshot();
            assert!(
                session
                    .transition_with(SceneTransition {
                        destination: "destination".into(),
                        carry: BTreeSet::from([999]),
                        spawn: None
                    })
                    .is_err()
            );
            assert!(
                session
                    .apply_commands(&[WorldCommand::Remove { id: 2 }])
                    .unwrap_err()
                    .contains("clear references")
            );
            assert_eq!(session.state(), &before);
            assert_eq!(session.snapshot(), snapshot);
        }
    }
    #[test]
    fn contribution_state_guard_rejects_initial_and_direct_command_publication() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let control = FixtureControl::default();
            let calls = Arc::new(AtomicUsize::new(0));
            let seen = calls.clone();
            let mut invalid = project();
            // This scene is structurally/schema valid; only the source-owned
            // whole-state cardinality contract rejects its publication.
            invalid.scenes.get_mut("source").unwrap().objects.remove(0);
            let error = match GameSession::new_with_world_factory(
                invalid,
                kind,
                guarded_registry(control.clone()),
                move |objects| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    make_provider(kind, objects)
                },
            ) {
                Ok(_) => panic!("Invalid initial source state was published"),
                Err(error) => error,
            };
            assert!(error.contains("Fixture layout requires exactly two entities"));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_eq!(control.constructed.load(Ordering::SeqCst), 0);

            let seen = calls.clone();
            let mut session = GameSession::new_with_world_factory(
                project(),
                kind,
                guarded_registry(control.clone()),
                move |objects| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    make_provider(kind, objects)
                },
            )
            .unwrap();
            session.step(1. / 60., &input(&["pulse"], 1.)).unwrap();
            let before = session.state().clone();
            let snapshot = session.snapshot();
            let events = session.events().to_vec();
            let states = session.system_states();
            let resources = session.executor.resources().clone();
            let lifetime = session.state().world.lifetime(1);
            let transform = Transform {
                translation: [7., 0., 4.],
                ..Default::default()
            };
            let error = session
                .apply_commands(&[
                    WorldCommand::SetTransform { id: 2, transform },
                    WorldCommand::Remove { id: 1 },
                ])
                .unwrap_err();
            assert!(error.contains("Fixture layout requires exactly two entities"));
            assert_eq!(session.state(), &before);
            assert_eq!(session.state().world.lifetime(1), lifetime);
            assert_eq!(session.snapshot(), snapshot);
            assert_eq!(session.events(), events);
            assert_eq!(session.system_states(), states);
            assert_eq!(session.executor.resources(), &resources);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(control.constructed.load(Ordering::SeqCst), 1);
            assert_eq!(control.published.load(Ordering::SeqCst), 1);
            assert_eq!(control.stopped.load(Ordering::SeqCst), 0);
            session
                .apply_commands(&[WorldCommand::SetTransform { id: 2, transform }])
                .unwrap();
            assert_eq!(
                session.state().world.object(2).unwrap().transform,
                transform
            );
        }
    }
    #[test]
    fn contribution_state_guard_rejects_merged_carry_before_destination_construction() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            for tick_transition in [false, true] {
                let control = FixtureControl::default();
                let calls = Arc::new(AtomicUsize::new(0));
                let seen = calls.clone();
                let mut session = GameSession::new_with_world_factory(
                    project(),
                    kind,
                    guarded_registry(control.clone()),
                    move |objects| {
                        seen.fetch_add(1, Ordering::SeqCst);
                        make_provider(kind, objects)
                    },
                )
                .unwrap();
                session.step(1. / 60., &GameInput::default()).unwrap();
                let before = session.state().clone();
                let snapshot = session.snapshot();
                let states = session.system_states();
                let resources = session.executor.resources().clone();
                let lifetime = session.state().world.lifetime(2);
                let request = SceneTransition {
                    destination: "destination".into(),
                    carry: BTreeSet::from([1, 2]),
                    spawn: None,
                };
                // Each authored scene has two entities and passes the guard.
                // Carrying ID 2 adds a third entity only after destination merge.
                let error = if tick_transition {
                    session
                        .step(1. / 60., &input(&["pulse", "transition"], 1.))
                        .unwrap_err()
                } else {
                    session.transition_with(request.clone()).unwrap_err()
                };
                assert!(error.contains("Fixture layout requires exactly two entities"));
                assert_eq!(session.state(), &before);
                assert_eq!(session.state().world.lifetime(2), lifetime);
                assert_eq!(session.snapshot(), snapshot);
                assert_eq!(session.system_states(), states);
                assert_eq!(session.executor.resources(), &resources);
                assert!(session.events().is_empty());
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                assert_eq!(control.constructed.load(Ordering::SeqCst), 1);
                assert_eq!(control.published.load(Ordering::SeqCst), 1);
                assert_eq!(control.stopped.load(Ordering::SeqCst), 0);
                if tick_transition {
                    assert_eq!(
                        session.pending_actions,
                        input(&["pulse", "transition"], 1.).actions
                    );
                    assert_eq!(session.pending_time(), 1. / 60.);
                }
                // An empty authored destination is valid only after the two
                // carried entities are merged; do not guard an intermediate DTO.
                session
                    .project
                    .scenes
                    .get_mut("destination")
                    .unwrap()
                    .objects
                    .clear();
                if tick_transition {
                    assert_eq!(
                        session.step_budget(0., &GameInput::default(), 1).unwrap(),
                        1
                    );
                    assert!(session.pending_actions.is_empty());
                    assert_eq!(session.state().tick, before.tick + 1);
                    assert_eq!(control.published.load(Ordering::SeqCst), 2);
                } else {
                    session.transition_with(request).unwrap();
                    assert_eq!(session.state().tick, before.tick);
                    assert_eq!(control.published.load(Ordering::SeqCst), 1);
                }
                assert_eq!(session.state().scene, "destination");
                assert_eq!(session.state().world.entities_snapshot().len(), 2);
                assert_eq!(calls.load(Ordering::SeqCst), 2);
                assert_eq!(control.constructed.load(Ordering::SeqCst), 2);
                assert_eq!(control.stopped.load(Ordering::SeqCst), 1);
            }
        }
    }
    #[test]
    fn strict_asset_context_and_activity_are_owned_across_transition_and_restart() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut project = project();
            project.scenes.get_mut("source").unwrap().objects[0]
                .components
                .insert("mesh".into(), json!({"path":"asset://model"}));
            let registry = registry(FixtureControl::default());
            let modules = registry.installed_modules();
            let mut paths = BTreeMap::from([("model".into(), "assets/model.glb".into())]);
            let active = registry.active_components(&project.execution).unwrap();
            let context = ComponentValidationContext {
                scenes: &project.scenes,
                modules: &modules,
                asset_paths: Some(&paths),
                active_components: Some(&active),
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
            assert_eq!(session.asset_bindings()[&1], "asset://model");
            let context = ComponentValidationContext {
                scenes: &project.scenes,
                modules: &modules,
                asset_paths: Some(&paths),
                active_components: Some(&active),
            };
            assert!(
                GameSession::new_with_registry_and_context(
                    project.clone(),
                    kind,
                    registry,
                    &context
                )
                .is_err()
            );
        }
    }
    #[test]
    fn inactive_author_data_stays_archived_and_never_enters_runtime_or_prepared_data() {
        let mut project = project();
        project.scenes.get_mut("source").unwrap().objects[0]
            .components
            .insert("uninstalled.archive".into(), json!(17));
        let source = serde_json::to_value(&project).unwrap();
        let registry = registry(FixtureControl::default());
        let session =
            GameSession::new_with_registry(project.clone(), WorldKind::Ecs, registry.clone())
                .unwrap();
        assert!(
            session
                .state()
                .world
                .component(1, "uninstalled.archive")
                .is_none()
        );
        assert!(
            !project.prepare_runtime(&registry).unwrap().scenes["source"].objects[0]
                .components
                .contains_key("uninstalled.archive")
        );
        assert_eq!(serde_json::to_value(&project).unwrap(), source);
    }
    #[test]
    fn canonical_entity_wire_roundtrip_is_semantic_and_rejects_bad_ids_references() {
        let mut session = session(WorldKind::Ecs);
        session.state.module_data.insert(
            "module/1".into(),
            json!({"token":session.state.world.lifetime(1)}),
        );
        let value = serde_json::to_value(session.state()).unwrap();
        assert!(value.get("world").is_none());
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
        let mut dangling = value.clone();
        dangling["entities"]["1"]["references"]["target"] = json!(999);
        assert!(serde_json::from_value::<GameState>(dangling).is_err());
        let entity = serde_json::to_string(&value["entities"]["1"]).unwrap();
        let duplicate = serde_json::to_string(&value).unwrap().replacen(
            "\"entities\":{",
            &format!("\"entities\":{{\"1\":{entity},"),
            1,
        );
        assert!(serde_json::from_str::<GameState>(&duplicate).is_err());
    }
    #[test]
    fn default_ecs_has_no_secondary_provider_and_halt_does_not_mutate_module_data() {
        let mut session = session(WorldKind::Ecs);
        assert!(session.provider.is_none());
        assert!(session.world_factory.is_none());
        session.state.control.halted = true;
        session
            .state
            .module_data
            .insert("app.frame".into(), json!({"value":7}));
        let before = session.state().clone();
        assert_eq!(session.step(0.1, &input(&["pulse"], 1.)).unwrap(), 0);
        assert_eq!(session.state(), &before);
        session.restart().unwrap();
        session.transition("destination").unwrap();
        assert!(session.provider.is_none());
        assert!(session.world_factory.is_none());
    }
    #[test]
    fn successful_tick_processes_generic_transition_and_halt_controls() {
        let mut session = session(WorldKind::Ecs);
        session
            .step(1. / 60., &input(&["pulse", "transition"], 0.))
            .unwrap();
        assert_eq!(session.state().scene, "destination");
        assert_eq!(session.state().tick, 1);
        assert!(session.state().world.contains(2));
        assert!(session.state().module_data.is_empty());
        assert_eq!(
            session.events(),
            &[
                GameEvent::new("Pulse", json!({"object":1})),
                GameEvent::new("SceneChanged", json!({"from":"source","to":"destination"}))
            ]
        );
        session.step(0.25, &input(&["halt"], 0.)).unwrap();
        assert!(session.state().control.halted);
        assert_eq!(session.pending_time(), 0.);
        let frame = session.state().module_data.clone();
        session.step(0.25, &input(&["pulse"], 0.)).unwrap();
        assert_eq!(session.state().module_data, frame);
    }
    #[test]
    fn replacement_factory_and_failed_stop_cannot_publish_visible_scene_edits() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            for restart in [false, true] {
                let control = FixtureControl::default();
                let calls = Arc::new(AtomicUsize::new(0));
                let seen = calls.clone();
                let mut session = GameSession::new_with_world_factory(
                    project(),
                    kind,
                    registry(control.clone()),
                    move |mut objects| {
                        if seen.fetch_add(1, Ordering::SeqCst) > 0 {
                            objects[0].color = [1, 2, 3, 4];
                        }
                        make_provider(kind, objects)
                    },
                )
                .unwrap();
                session.step(1. / 60., &GameInput::default()).unwrap();
                let before = session.state().clone();
                let snapshot = session.snapshot();
                let error = if restart {
                    session.restart()
                } else {
                    session.transition("destination")
                }
                .unwrap_err();
                assert!(error.contains("did not load"));
                assert_eq!(session.state(), &before);
                assert_eq!(session.snapshot(), snapshot);
                assert_eq!(control.stopped.load(Ordering::SeqCst), 0);
                let control = FixtureControl::default();
                let mut session =
                    GameSession::new_with_registry(project(), kind, registry(control.clone()))
                        .unwrap();
                session.step(1. / 60., &GameInput::default()).unwrap();
                control.reject_stop.store(true, Ordering::SeqCst);
                let before = session.state().clone();
                let events = session.events().to_vec();
                let snapshot = session.snapshot();
                assert!(
                    if restart {
                        session.restart()
                    } else {
                        session.transition("destination")
                    }
                    .unwrap_err()
                    .contains("stop rejected")
                );
                assert_eq!(session.state(), &before);
                assert_eq!(session.events(), events);
                assert_eq!(session.snapshot(), snapshot);
            }
        }
    }
    struct CachedWorld {
        inner: ObjectsWorld,
        packets: u32,
        max_x: Option<f32>,
    }
    impl WorldProvider for CachedWorld {
        fn step(&mut self, dt: f32, speed: f32) {
            self.inner.step(dt, speed)
        }
        fn snapshot(&self) -> SceneSnapshot {
            let mut snapshot = self.inner.snapshot();
            snapshot.items[0].color[0] = self.packets as u8;
            snapshot
        }
        fn objects(&self) -> Result<Vec<WorldObject>, String> {
            self.inner.objects()
        }
        fn validate_commands(&self, commands: &[WorldCommand]) -> Result<(), String> {
            let mut candidate = self.inner.clone();
            candidate.apply_commands(commands)?;
            if self.max_x.is_some_and(|max| {
                candidate
                    .objects()
                    .unwrap()
                    .iter()
                    .any(|object| object.id == 1 && object.transform.translation[0] > max)
            }) {
                Err("Third world rejects candidate".into())
            } else {
                Ok(())
            }
        }
        fn apply_commands(&mut self, commands: &[WorldCommand]) -> Result<(), String> {
            self.validate_commands(commands)?;
            self.inner.apply_commands(commands)?;
            self.packets += 1;
            Ok(())
        }
    }
    #[test]
    fn third_world_cache_survives_ticks_and_provider_rejection_rolls_back_publication() {
        let created = Arc::new(AtomicUsize::new(0));
        let seen = created.clone();
        let control = FixtureControl::default();
        let mut session = GameSession::new_with_world_factory(
            project(),
            WorldKind::Custom,
            registry(control.clone()),
            move |objects| {
                seen.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(CachedWorld {
                    inner: ObjectsWorld::from_objects(objects)?,
                    packets: 0,
                    max_x: Some(0.05),
                }))
            },
        )
        .unwrap();
        for _ in 0..2 {
            session.step(1. / 60., &input(&[], 1.)).unwrap();
        }
        assert_eq!(session.snapshot().items[0].color[0], 2);
        assert_eq!(created.load(Ordering::SeqCst), 1);
        let before = session.state().clone();
        let snapshot = session.snapshot();
        let published = control.published.load(Ordering::SeqCst);
        assert!(
            session
                .step(1. / 60., &input(&["pulse"], 10.))
                .unwrap_err()
                .contains("Third world rejects")
        );
        assert_eq!(session.state(), &before);
        assert_eq!(session.snapshot(), snapshot);
        assert_eq!(control.published.load(Ordering::SeqCst), published);
        session
            .apply_commands(&[WorldCommand::SetColor {
                id: 1,
                color: [4, 5, 6, 255],
            }])
            .unwrap();
        assert_eq!(created.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn session_restart_transition_and_drop_stop_initialized_systems_once() {
        let control = FixtureControl::default();
        {
            let mut session = GameSession::new_with_registry(
                project(),
                WorldKind::Ecs,
                registry(control.clone()),
            )
            .unwrap();
            session.step(1. / 60., &GameInput::default()).unwrap();
            session.restart().unwrap();
            assert_eq!(control.stopped.load(Ordering::SeqCst), 1);
            session.step(1. / 60., &GameInput::default()).unwrap();
            session.transition("destination").unwrap();
            assert_eq!(control.stopped.load(Ordering::SeqCst), 2);
            session.step(1. / 60., &GameInput::default()).unwrap();
        }
        assert_eq!(control.started.load(Ordering::SeqCst), 3);
        assert_eq!(control.stopped.load(Ordering::SeqCst), 3);
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
}
