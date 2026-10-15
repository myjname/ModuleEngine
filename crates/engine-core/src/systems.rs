//! General source-installed executable systems. The graph chooses the schedule;
//! factories choose behavior. Contracts are validated before any lifecycle hook.
use crate::{GameEvent, GameInput, GameState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// An explicitly typed resource. External values are supplied by the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemResource {
    pub name: String,
    pub data_type: String,
    #[serde(default)]
    pub external: bool,
    #[serde(default)]
    pub initial: Value,
}

/// Resource access declares dependencies without choosing a storage backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceAccess {
    Read,
    Write,
    ReadWrite,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPort {
    pub name: String,
    pub data_type: String,
    pub access: ResourceAccess,
}

/// Installed executable contract and state schema for one source module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemDescriptor {
    pub id: String,
    pub state_schema: String,
    /// Component names whose fields this installed module contributes to UI.
    #[serde(default)]
    pub components: Vec<String>,
    pub ports: Vec<SystemPort>,
    /// Required preceding module kinds. Graph author still chooses their order.
    #[serde(default)]
    pub requires: Vec<String>,
}

/// One system instance. `after` names instance IDs; bindings map pins to resources.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemNode {
    pub id: String,
    pub module: String,
    #[serde(default)]
    pub params: Value,
    pub bindings: BTreeMap<String, String>,
    #[serde(default)]
    pub after: Vec<String>,
}

/// Stages and nodes execute in the exact authored order, never topological guesswork.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemStage {
    pub name: String,
    pub nodes: Vec<SystemNode>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionGraph {
    pub resources: Vec<SystemResource>,
    pub stages: Vec<SystemStage>,
}

/// Typed envelopes owned by the executor; module payloads remain normal JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceValue {
    pub data_type: String,
    pub value: Value,
}
pub type ResourceStore = BTreeMap<String, ResourceValue>;

/// One fixed simulation tick; helpers resolve this instance's bound resource
/// names. Native modules are trusted and metadata is a scheduling contract.
pub struct SystemContext<'a> {
    pub game: &'a mut GameState,
    pub resources: &'a mut ResourceStore,
    pub events: &'a mut Vec<GameEvent>,
    pub dt: f32,
    pub input: &'a GameInput,
    pub bindings: &'a BTreeMap<String, String>,
}

impl SystemContext<'_> {
    pub fn resource(&self, port: &str) -> Result<&Value, String> {
        let id = self
            .bindings
            .get(port)
            .ok_or_else(|| format!("Unbound resource port '{port}'"))?;
        let resource = self
            .resources
            .get(id)
            .ok_or_else(|| format!("Missing resource '{id}'"))?;
        if resource.data_type == "GameWorld" {
            return Err(
                "GameWorld is a typed service resource; use context.world(port) or context.game"
                    .into(),
            );
        }
        Ok(&resource.value)
    }
    /// Resolve an explicit GameWorld service binding, without serializing the
    /// whole scene. Platform service pins provide this shared portable interface.
    pub fn world(&self, port: &str) -> Result<&GameState, String> {
        self.check_service(port, "GameWorld")?;
        Ok(self.game)
    }
    pub fn world_mut(&mut self, port: &str) -> Result<&mut GameState, String> {
        self.check_service(port, "GameWorld")?;
        Ok(self.game)
    }
    pub fn input_resource(&self, port: &str) -> Result<&GameInput, String> {
        self.check_service(port, "GameInput")?;
        Ok(self.input)
    }
    pub fn events_resource(&self, port: &str) -> Result<&[GameEvent], String> {
        self.check_service(port, "GameEvents")?;
        Ok(self.events)
    }
    pub fn events_resource_mut(&mut self, port: &str) -> Result<&mut Vec<GameEvent>, String> {
        self.check_service(port, "GameEvents")?;
        Ok(self.events)
    }
    fn check_service(&self, port: &str, expected: &str) -> Result<(), String> {
        let id = self
            .bindings
            .get(port)
            .ok_or_else(|| format!("Unbound resource port '{port}'"))?;
        if self
            .resources
            .get(id)
            .is_some_and(|r| r.data_type == expected)
        {
            Ok(())
        } else {
            Err(format!("Port '{port}' does not bind a {expected} service"))
        }
    }
    pub fn set_resource(&mut self, port: &str, value: Value) -> Result<(), String> {
        let id = self
            .bindings
            .get(port)
            .ok_or_else(|| format!("Unbound resource port '{port}'"))?;
        let resource = self
            .resources
            .get_mut(id)
            .ok_or_else(|| format!("Missing resource '{id}'"))?;
        // These platform resource types have typed canonical context access.
        // Generic user resource types remain opaque envelopes.
        if resource.data_type == "GameInput" {
            return Err("GameInput is read-only host input".into());
        }
        if resource.data_type == "GameEvents" {
            *self.events = serde_json::from_value(value.clone())
                .map_err(|e| format!("Invalid GameEvents: {e}"))?;
        }
        if resource.data_type == "GameWorld" {
            let candidate: GameState = serde_json::from_value(value.clone())
                .map_err(|e| format!("Invalid GameWorld: {e}"))?;
            crate::validate_world_commands(
                &candidate
                    .entities
                    .values()
                    .map(|e| e.object.clone())
                    .collect::<Vec<_>>(),
                &[],
            )?;
            *self.game = candidate;
        }
        resource.value = value;
        Ok(())
    }
    /// Portable world edits preserve component state and subtree semantics.
    pub fn apply_commands(&mut self, commands: &[crate::WorldCommand]) -> Result<(), String> {
        self.game.apply_commands(commands)
    }
}

/// A compiled source module. State is explicit, owned by the executor, and can
/// survive a compatible implementation replacement without restarting Play.
pub trait ExecutableSystem: Send {
    fn initialize(
        &mut self,
        _context: &mut SystemContext<'_>,
        _state: &mut Value,
    ) -> Result<(), String> {
        Ok(())
    }
    fn step(&mut self, context: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String>;
    /// Check a staged external output after every system has completed its step.
    fn validate_tick(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Publish a previously validated external output. Implement rollback_tick
    /// when publication changes state outside the executor's owned JSON values.
    fn commit_tick(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Undo staged/published external state after any step/validate/commit error.
    fn rollback_tick(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&mut self, _context: &mut SystemContext<'_>, _state: &mut Value) -> Result<(), String> {
        Ok(())
    }
}

/// A host-owned transactional target, such as the selected World provider.
/// Validation runs after all systems stage outputs; target publication runs
/// before system publication so any rejection retains last-good script state.
pub trait TickTransaction {
    fn validate(&mut self, _candidate: &GameState) -> Result<(), String> {
        Ok(())
    }
    fn commit(&mut self, _candidate: &GameState) -> Result<(), String> {
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), String> {
        Ok(())
    }
}
struct NoTickTransaction;
impl TickTransaction for NoTickTransaction {}

type Factory = Arc<dyn Fn(&Value) -> Result<Box<dyn ExecutableSystem>, String> + Send + Sync>;

#[derive(Clone, Default)]
pub struct SystemRegistry {
    entries: BTreeMap<String, (SystemDescriptor, Factory)>,
}

impl SystemRegistry {
    pub fn builtin() -> Self {
        let mut result = Self::default();
        crate::register_game_systems(&mut result);
        result
    }
    pub fn register<F>(&mut self, descriptor: SystemDescriptor, factory: F) -> Result<(), String>
    where
        F: Fn(&Value) -> Result<Box<dyn ExecutableSystem>, String> + Send + Sync + 'static,
    {
        if descriptor.id.trim().is_empty()
            || descriptor.state_schema.trim().is_empty()
            || self.entries.contains_key(&descriptor.id)
        {
            return Err("System needs unique module ID and explicit state schema".into());
        }
        let mut names = BTreeSet::new();
        for port in &descriptor.ports {
            if port.name.trim().is_empty()
                || port.data_type.trim().is_empty()
                || !names.insert(&port.name)
            {
                return Err("System has invalid or duplicate resource ports".into());
            }
        }
        self.entries
            .insert(descriptor.id.clone(), (descriptor, Arc::new(factory)));
        Ok(())
    }
    pub fn descriptors(&self) -> impl Iterator<Item = &SystemDescriptor> {
        self.entries.values().map(|(d, _)| d)
    }
    pub fn descriptor(&self, id: &str) -> Option<&SystemDescriptor> {
        self.entries.get(id).map(|(d, _)| d)
    }
    /// Active component contributions. Source Custom nodes can explicitly
    /// declare additional component types through params.components.
    pub fn active_components(&self, graph: &ExecutionGraph) -> Result<BTreeSet<String>, String> {
        self.validate(graph)?;
        let mut result = BTreeSet::from(["mesh".into(), "material".into()]);
        for node in graph.stages.iter().flat_map(|s| &s.nodes) {
            result.extend(
                self.descriptor(&node.module)
                    .ok_or("Missing system descriptor")?
                    .components
                    .iter()
                    .cloned(),
            );
            if let Some(types) = node.params.get("components") {
                let types = types
                    .as_array()
                    .ok_or("System params.components must be an array of names")?;
                for name in types {
                    let name = name
                        .as_str()
                        .filter(|s| !s.trim().is_empty() && s.len() <= 256)
                        .ok_or("Declared component names need 1..256 bytes")?;
                    result.insert(name.into());
                }
            }
        }
        Ok(result)
    }

    /// Reject missing modules/resources, wrong types, use-before-production and
    /// constraints that conflict with the explicitly authored stage order.
    pub fn validate(&self, graph: &ExecutionGraph) -> Result<(), String> {
        let mut resources = BTreeMap::new();
        let mut available = BTreeSet::new();
        for r in &graph.resources {
            if r.name.trim().is_empty()
                || r.data_type.trim().is_empty()
                || resources.insert(&r.name, r).is_some()
            {
                return Err("Resources need unique names and explicit types".into());
            }
            if r.external {
                available.insert(r.name.clone());
            }
        }
        let mut stages = BTreeSet::new();
        let mut nodes = BTreeSet::new();
        let mut modules = BTreeSet::new();
        for stage in &graph.stages {
            if stage.name.trim().is_empty() || !stages.insert(&stage.name) {
                return Err("Stages need unique nonempty names".into());
            }
            for node in &stage.nodes {
                if node.id.trim().is_empty() || nodes.contains(&node.id) {
                    return Err("System instances need unique nonempty IDs".into());
                }
                let d = self.descriptor(&node.module).ok_or_else(|| {
                    format!("No executable factory installed for '{}'", node.module)
                })?;
                for id in &node.after {
                    if !nodes.contains(id) {
                        return Err(format!("System '{}' must run after '{id}'", node.id));
                    }
                }
                for module in &d.requires {
                    if !modules.contains(module) {
                        return Err(format!(
                            "System '{}' requires preceding module '{module}'",
                            node.id
                        ));
                    }
                }
                if node.bindings.len() != d.ports.len() {
                    return Err(format!(
                        "System '{}' has missing or extra resource bindings",
                        node.id
                    ));
                }
                for port in &d.ports {
                    let id = node
                        .bindings
                        .get(&port.name)
                        .ok_or_else(|| format!("System '{}' needs pin '{}'", node.id, port.name))?;
                    let r = resources
                        .get(id)
                        .ok_or_else(|| format!("Resource '{id}' is not declared"))?;
                    if r.data_type != port.data_type {
                        return Err(format!(
                            "Type mismatch on '{}.{}': {} versus {}",
                            node.id, port.name, r.data_type, port.data_type
                        ));
                    }
                    if port.access != ResourceAccess::Write && !available.contains(id) {
                        return Err(format!(
                            "Resource '{id}' is read before production by '{}'",
                            node.id
                        ));
                    }
                }
                for port in &d.ports {
                    if port.access != ResourceAccess::Read {
                        available.insert(node.bindings[&port.name].clone());
                    }
                }
                nodes.insert(node.id.clone());
                modules.insert(node.module.clone());
            }
        }
        Ok(())
    }
}

struct LiveSystem {
    node: SystemNode,
    schema: String,
    implementation: Box<dyn ExecutableSystem>,
    state: Value,
    initialized: bool,
}

/// Executor of arbitrary registered systems; it does not inspect module IDs.
pub struct SystemExecutor {
    systems: Vec<LiveSystem>,
    resources: ResourceStore,
}

impl SystemExecutor {
    pub fn new(graph: &ExecutionGraph, registry: &SystemRegistry) -> Result<Self, String> {
        registry.validate(graph)?;
        let mut systems = vec![];
        for node in graph.stages.iter().flat_map(|s| &s.nodes) {
            let (d, f) = &registry.entries[&node.module];
            systems.push(LiveSystem {
                node: node.clone(),
                schema: d.state_schema.clone(),
                implementation: f(&node.params)?,
                state: Value::Null,
                initialized: false,
            });
        }
        let resources = graph
            .resources
            .iter()
            .map(|r| {
                (
                    r.name.clone(),
                    ResourceValue {
                        data_type: r.data_type.clone(),
                        value: r.initial.clone(),
                    },
                )
            })
            .collect();
        Ok(Self { systems, resources })
    }
    pub fn step(
        &mut self,
        game: &mut GameState,
        input: &GameInput,
        dt: f32,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), String> {
        self.step_with_transaction(game, input, dt, events, &mut NoTickTransaction)
    }
    /// Include an external target in the same all-or-none system tick.
    pub fn step_with_transaction(
        &mut self,
        game: &mut GameState,
        input: &GameInput,
        dt: f32,
        events: &mut Vec<GameEvent>,
        transaction: &mut dyn TickTransaction,
    ) -> Result<(), String> {
        let original_game = game.clone();
        let original_resources = self.resources.clone();
        let original_events = events.clone();
        let original_states: Vec<_> = self
            .systems
            .iter()
            .map(|s| (s.state.clone(), s.initialized))
            .collect();
        let mut entered = 0;
        let mut result = self.run_tick(game, input, dt, events, &mut entered, transaction);
        if result.is_err() {
            let mut rollback_errors = vec![];
            for system in self.systems.iter_mut().take(entered).rev() {
                if let Err(error) = system.implementation.rollback_tick() {
                    rollback_errors.push(format!("{}: {error}", system.node.id));
                }
            }
            if let Err(error) = transaction.rollback() {
                rollback_errors.push(format!("Host transaction: {error}"));
            }
            *game = original_game;
            self.resources = original_resources;
            *events = original_events;
            for (system, (state, initialized)) in self.systems.iter_mut().zip(original_states) {
                system.state = state;
                system.initialized = initialized;
            }
            if !rollback_errors.is_empty() {
                result = Err(format!(
                    "{}; external rollback failed: {}",
                    result.unwrap_err(),
                    rollback_errors.join("; ")
                ));
            }
        }
        result
    }
    fn run_tick(
        &mut self,
        game: &mut GameState,
        input: &GameInput,
        dt: f32,
        events: &mut Vec<GameEvent>,
        entered: &mut usize,
        transaction: &mut dyn TickTransaction,
    ) -> Result<(), String> {
        for (index, system) in self.systems.iter_mut().enumerate() {
            *entered = index + 1;
            for resource in self.resources.values_mut() {
                resource.value = match resource.data_type.as_str() {
                    "GameInput" => serde_json::to_value(input).map_err(|e| e.to_string())?,
                    "GameEvents" => serde_json::to_value(&*events).map_err(|e| e.to_string())?,
                    // GameWorld uses the typed game capability; duplicating all
                    // entities into JSON before every system is unnecessary.
                    _ => resource.value.clone(),
                };
            }
            let mut ctx = SystemContext {
                game,
                resources: &mut self.resources,
                events,
                dt,
                input,
                bindings: &system.node.bindings,
            };
            if !system.initialized {
                system
                    .implementation
                    .initialize(&mut ctx, &mut system.state)?;
                system.initialized = true;
            }
            system
                .implementation
                .step(&mut ctx, &mut system.state)
                .map_err(|e| format!("System '{}': {e}", system.node.id))?;
        }
        for system in &mut self.systems {
            system
                .implementation
                .validate_tick()
                .map_err(|e| format!("System '{}' staged output: {e}", system.node.id))?;
        }
        transaction
            .validate(game)
            .map_err(|e| format!("Host transaction validation: {e}"))?;
        transaction
            .commit(game)
            .map_err(|e| format!("Host transaction publication: {e}"))?;
        for system in &mut self.systems {
            system
                .implementation
                .commit_tick()
                .map_err(|e| format!("System '{}' commit: {e}", system.node.id))?;
        }
        Ok(())
    }
    pub fn stop(
        &mut self,
        game: &mut GameState,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), String> {
        let input = GameInput::default();
        let mut errors = vec![];
        for system in self.systems.iter_mut().rev() {
            if system.initialized {
                // Mark first so a later best-effort shutdown cannot invoke a
                // failing stop hook twice; other instances still receive stop.
                system.initialized = false;
                let mut ctx = SystemContext {
                    game,
                    resources: &mut self.resources,
                    events,
                    dt: 0.,
                    input: &input,
                    bindings: &system.node.bindings,
                };
                if let Err(error) = system.implementation.stop(&mut ctx, &mut system.state) {
                    errors.push(format!("System '{}': {error}", system.node.id));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
    pub fn states(&self) -> BTreeMap<String, Value> {
        self.systems
            .iter()
            .map(|s| (s.node.id.clone(), s.state.clone()))
            .collect()
    }
    pub fn resources(&self) -> &ResourceStore {
        &self.resources
    }
    /// Stage a replacement only when the state contract matches. A caller can
    /// explicitly migrate state or request a reset instead of losing it silently.
    pub fn replace(
        &mut self,
        id: &str,
        registry: &SystemRegistry,
        migrated: Option<Value>,
        reset: bool,
    ) -> Result<(), String> {
        let live = self
            .systems
            .iter_mut()
            .find(|s| s.node.id == id)
            .ok_or_else(|| "Unknown system instance".to_string())?;
        let (d, f) = registry
            .entries
            .get(&live.node.module)
            .ok_or_else(|| "Replacement module is missing".to_string())?;
        if live.schema != d.state_schema && !reset && migrated.is_none() {
            return Err("Incompatible state schema: explicit reset or migration required; previous implementation retained".into());
        }
        let replacement = f(&live.node.params)?;
        live.implementation = replacement;
        live.schema = d.state_schema.clone();
        if reset {
            live.state = Value::Null;
            live.initialized = false;
        } else if let Some(state) = migrated {
            live.state = state;
        }
        Ok(())
    }
}

impl ExecutionGraph {
    /// Editable demonstration schedule; the executor itself has no game route.
    pub fn gameplay() -> Self {
        let resources = vec![
            SystemResource {
                name: "world".into(),
                data_type: "GameWorld".into(),
                external: true,
                initial: Value::Null,
            },
            SystemResource {
                name: "input".into(),
                data_type: "GameInput".into(),
                external: true,
                initial: Value::Null,
            },
            SystemResource {
                name: "events".into(),
                data_type: "GameEvents".into(),
                external: true,
                initial: json!([]),
            },
            SystemResource {
                name: "previous".into(),
                data_type: "Positions".into(),
                external: false,
                initial: Value::Null,
            },
        ];
        let configs = [
            ("Controllers", "game.input", vec!["world", "input"]),
            ("AI", "game.ai", vec!["world"]),
            ("Movement", "game.movement", vec!["world", "previous"]),
            ("Collisions", "game.collision", vec!["world", "previous"]),
            ("Combat", "game.combat", vec!["world", "events"]),
            ("Health", "game.health", vec!["world", "events"]),
            ("Scenes", "game.portals", vec!["world", "events"]),
            ("Audio events", "game.audio", vec!["events"]),
        ];
        let stages = configs
            .into_iter()
            .enumerate()
            .map(|(i, (name, module, ports))| SystemStage {
                name: name.into(),
                nodes: vec![SystemNode {
                    id: format!("system-{i}"),
                    module: module.into(),
                    params: json!({}),
                    bindings: ports.into_iter().map(|p| (p.into(), p.into())).collect(),
                    after: if i == 0 {
                        vec![]
                    } else {
                        vec![format!("system-{}", i - 1)]
                    },
                }],
            })
            .collect();
        Self { resources, stages }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrong_order_and_resource_type_are_rejected() {
        let r = SystemRegistry::builtin();
        let mut g = ExecutionGraph::gameplay();
        r.validate(&g).unwrap();
        g.stages.swap(2, 3);
        assert!(r.validate(&g).is_err());
        g = ExecutionGraph::gameplay();
        g.resources[3].data_type = "Texture".into();
        assert!(r.validate(&g).unwrap_err().contains("Type mismatch"));
        g = ExecutionGraph::gameplay();
        g.stages[0].nodes[0].module = "custom.missing".into();
        assert!(r.validate(&g).unwrap_err().contains("factory"));
    }
    struct Producer;
    impl ExecutableSystem for Producer {
        fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            let n = state.as_u64().unwrap_or(0) + 1;
            *state = json!(n);
            ctx.set_resource("result", json!(n))
        }
    }
    struct Consumer {
        reject: bool,
    }
    impl ExecutableSystem for Consumer {
        fn step(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            ctx.game.time = ctx.resource("value")?.as_f64().unwrap();
            if self.reject {
                Err("Rejected candidate".into())
            } else {
                Ok(())
            }
        }
    }
    fn graph_registry(reject: bool, schema: &str) -> (ExecutionGraph, SystemRegistry) {
        let mut registry = SystemRegistry::default();
        registry
            .register(
                SystemDescriptor {
                    id: "third.counter".into(),
                    state_schema: schema.into(),
                    components: vec![],
                    ports: vec![SystemPort {
                        name: "result".into(),
                        data_type: "Number".into(),
                        access: ResourceAccess::Write,
                    }],
                    requires: vec![],
                },
                |_| Ok(Box::new(Producer)),
            )
            .unwrap();
        registry
            .register(
                SystemDescriptor {
                    id: "third.consumer".into(),
                    state_schema: "1".into(),
                    components: vec![],
                    ports: vec![SystemPort {
                        name: "value".into(),
                        data_type: "Number".into(),
                        access: ResourceAccess::Read,
                    }],
                    requires: vec![],
                },
                move |_| Ok(Box::new(Consumer { reject })),
            )
            .unwrap();
        let graph = ExecutionGraph {
            resources: vec![SystemResource {
                name: "counter".into(),
                data_type: "Number".into(),
                external: false,
                initial: Value::Null,
            }],
            stages: vec![SystemStage {
                name: "Third-party systems".into(),
                nodes: vec![
                    SystemNode {
                        id: "a".into(),
                        module: "third.counter".into(),
                        params: Value::Null,
                        bindings: BTreeMap::from([("result".into(), "counter".into())]),
                        after: vec![],
                    },
                    SystemNode {
                        id: "b".into(),
                        module: "third.consumer".into(),
                        params: Value::Null,
                        bindings: BTreeMap::from([("value".into(), "counter".into())]),
                        after: vec!["a".into()],
                    },
                ],
            }],
        };
        (graph, registry)
    }
    #[test]
    fn third_party_factories_execute_and_tick_failure_rolls_back_state() {
        let (g, r) = graph_registry(false, "1");
        let mut project = crate::GameProject::demo();
        project.execution = g.clone();
        let session = crate::GameSession::new(project, crate::WorldKind::Objects);
        assert!(session.is_err());
        let mut project = crate::GameProject::demo();
        project.execution = g.clone();
        let mut game =
            crate::GameSession::new_with_registry(project, crate::WorldKind::Objects, r.clone())
                .unwrap()
                .state()
                .clone();
        let mut executor = SystemExecutor::new(&g, &r).unwrap();
        let mut events = vec![];
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        assert_eq!(game.time, 1.);
        assert_eq!(executor.states()["a"], json!(1));
        let (_, reject) = graph_registry(true, "1");
        executor.replace("b", &reject, None, false).unwrap();
        let original = game.clone();
        assert!(
            executor
                .step(&mut game, &GameInput::default(), 0.01, &mut events)
                .is_err()
        );
        assert_eq!(game, original);
        assert_eq!(executor.states()["a"], json!(1));
        let (_, incompatible) = graph_registry(false, "2");
        assert!(executor.replace("a", &incompatible, None, false).is_err());
        executor
            .replace("a", &incompatible, Some(json!(9)), false)
            .unwrap();
        assert_eq!(executor.states()["a"], json!(9));
    }

    struct ExternalCounter {
        value: Arc<std::sync::Mutex<u64>>,
        before: u64,
        pending: u64,
        fail_step: bool,
        fail_commit: bool,
    }
    impl ExecutableSystem for ExternalCounter {
        fn step(&mut self, _: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            self.before = *self.value.lock().unwrap();
            self.pending = self.before + 1;
            *state = json!(self.pending);
            if self.fail_step {
                Err("Early failure".into())
            } else {
                Ok(())
            }
        }
        fn commit_tick(&mut self) -> Result<(), String> {
            *self.value.lock().unwrap() = self.pending;
            if self.fail_commit {
                Err("Late commit failure".into())
            } else {
                Ok(())
            }
        }
        fn rollback_tick(&mut self) -> Result<(), String> {
            *self.value.lock().unwrap() = self.before;
            Ok(())
        }
    }
    fn external_registry(
        values: &[Arc<std::sync::Mutex<u64>>; 2],
        early: bool,
        late: bool,
    ) -> SystemRegistry {
        let mut registry = SystemRegistry::default();
        for (i, id) in ["external.a", "external.b"].into_iter().enumerate() {
            let value = values[i].clone();
            registry
                .register(
                    SystemDescriptor {
                        id: id.into(),
                        state_schema: "1".into(),
                        components: vec![],
                        ports: vec![],
                        requires: vec![],
                    },
                    move |_| {
                        Ok(Box::new(ExternalCounter {
                            value: value.clone(),
                            before: 0,
                            pending: 0,
                            fail_step: early && i == 0,
                            fail_commit: late && i == 1,
                        }))
                    },
                )
                .unwrap();
        }
        registry
    }
    #[test]
    fn external_commit_failure_rolls_back_and_untouched_system_keeps_previous_tick() {
        let values = [
            Arc::new(std::sync::Mutex::new(0)),
            Arc::new(std::sync::Mutex::new(0)),
        ];
        let registry = external_registry(&values, false, false);
        let graph = ExecutionGraph {
            resources: vec![],
            stages: vec![SystemStage {
                name: "external".into(),
                nodes: ["external.a", "external.b"]
                    .into_iter()
                    .map(|id| SystemNode {
                        id: id.into(),
                        module: id.into(),
                        params: Value::Null,
                        bindings: BTreeMap::new(),
                        after: vec![],
                    })
                    .collect(),
            }],
        };
        let mut executor = SystemExecutor::new(&graph, &registry).unwrap();
        let mut game =
            crate::GameSession::new(crate::GameProject::demo(), crate::WorldKind::Objects)
                .unwrap()
                .state()
                .clone();
        let mut events = vec![];
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        assert_eq!(*values[0].lock().unwrap(), 1);
        assert_eq!(*values[1].lock().unwrap(), 1);
        let before = executor.states();
        let early = external_registry(&values, true, false);
        executor.replace("external.a", &early, None, false).unwrap();
        assert!(
            executor
                .step(&mut game, &GameInput::default(), 0.01, &mut events)
                .is_err()
        );
        assert_eq!(*values[0].lock().unwrap(), 1);
        assert_eq!(
            *values[1].lock().unwrap(),
            1,
            "Untouched later system must retain previous successful publication"
        );
        assert_eq!(executor.states(), before);
        let late = external_registry(&values, false, true);
        executor.replace("external.a", &late, None, false).unwrap();
        executor.replace("external.b", &late, None, false).unwrap();
        assert!(
            executor
                .step(&mut game, &GameInput::default(), 0.01, &mut events)
                .is_err()
        );
        assert_eq!(
            *values[0].lock().unwrap(),
            1,
            "Earlier commit must rollback after later commit rejects"
        );
        assert_eq!(*values[1].lock().unwrap(), 1);
        assert_eq!(executor.states(), before);
    }
}
