//! General source-installed executable systems. The graph chooses the schedule;
//! factories choose behavior. Contracts are validated before any lifecycle hook.
use crate::{GameEvent, GameInput, GameState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
            crate::validate_world_commands(&crate::WorldProvider::objects(&candidate.world)?, &[])?;
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
type ProjectValidator = Arc<
    dyn Fn(&crate::GameProject, &crate::ComponentValidationContext<'_>) -> Result<(), String>
        + Send
        + Sync,
>;
type ProjectPreparer = Arc<dyn Fn(&mut crate::GameProject) -> Result<(), String> + Send + Sync>;

#[derive(Clone, Default)]
pub struct SystemRegistry {
    entries: BTreeMap<String, (SystemDescriptor, Factory)>,
    components: crate::ComponentRegistry,
    installed_versions: BTreeMap<String, String>,
    module_dependencies: BTreeMap<String, BTreeSet<String>>,
    project_contracts: BTreeMap<String, (ProjectValidator, ProjectPreparer)>,
    game_contributions: BTreeMap<String, crate::GameContribution>,
}

impl SystemRegistry {
    pub fn register_project_contract<V, P>(
        &mut self,
        owner: &str,
        validate: V,
        prepare: P,
    ) -> Result<(), String>
    where
        V: Fn(&crate::GameProject, &crate::ComponentValidationContext<'_>) -> Result<(), String>
            + Send
            + Sync
            + 'static,
        P: Fn(&mut crate::GameProject) -> Result<(), String> + Send + Sync + 'static,
    {
        if !self.installed_versions.contains_key(owner)
            || self.project_contracts.contains_key(owner)
        {
            return Err("Project contract needs a unique installed owner".into());
        }
        self.project_contracts
            .insert(owner.into(), (Arc::new(validate), Arc::new(prepare)));
        Ok(())
    }
    pub fn validate_project_contracts(
        &self,
        project: &crate::GameProject,
        context: &crate::ComponentValidationContext<'_>,
    ) -> Result<(), String> {
        for (validate, _) in self.project_contracts.values() {
            validate(project, context)?;
        }
        Ok(())
    }
    pub fn prepare_project_contracts(
        &self,
        project: &mut crate::GameProject,
    ) -> Result<(), String> {
        for (_, prepare) in self.project_contracts.values() {
            prepare(project)?;
        }
        Ok(())
    }
    pub fn register_game_contribution(
        &mut self,
        contribution: crate::GameContribution,
    ) -> Result<(), String> {
        if contribution.id.trim().is_empty()
            || contribution.activation_modules.is_empty()
            || self.game_contributions.contains_key(&contribution.id)
            || contribution
                .activation_modules
                .iter()
                .any(|id| !self.installed_versions.contains_key(id))
        {
            return Err(
                "Game contribution needs a unique ID and installed activating modules".into(),
            );
        }
        self.game_contributions
            .insert(contribution.id.clone(), contribution);
        Ok(())
    }
    pub fn active_game_contributions(
        &self,
        graph: &ExecutionGraph,
    ) -> Vec<&crate::GameContribution> {
        let scheduled: BTreeSet<_> = graph
            .stages
            .iter()
            .flat_map(|stage| &stage.nodes)
            .map(|node| node.module.as_str())
            .collect();
        self.game_contributions
            .values()
            .filter(|contribution| {
                contribution
                    .activation_modules
                    .iter()
                    .any(|id| scheduled.contains(id.as_str()))
            })
            .collect()
    }
    pub fn game_contributions(&self) -> impl Iterator<Item = &crate::GameContribution> {
        self.game_contributions.values()
    }
    pub fn builtin() -> Self {
        let mut result = Self::default();
        crate::register_builtin_component_schemas(result.components_mut())
            .expect("Valid built-in component schemas");
        result
            .install_module("scene.extract", env!("CARGO_PKG_VERSION"))
            .expect("Valid built-in scene module");
        result
    }
    pub fn components(&self) -> &crate::ComponentRegistry {
        &self.components
    }
    pub fn components_mut(&mut self) -> &mut crate::ComponentRegistry {
        &mut self.components
    }
    /// Install a schema/service owner independently of executable consumers.
    pub fn install_module(&mut self, id: &str, version: &str) -> Result<(), String> {
        if id.trim().is_empty() || version.trim().is_empty() {
            return Err("Installed module needs a nonempty ID and implementation version".into());
        }
        if let Some(installed) = self.installed_versions.get(id) {
            if installed != version {
                return Err(format!(
                    "Module '{id}' is already installed at version '{installed}'"
                ));
            }
        } else {
            self.installed_versions.insert(id.into(), version.into());
        }
        Ok(())
    }
    pub fn module_version(&self, id: &str) -> Option<&str> {
        self.installed_versions.get(id).map(String::as_str)
    }
    pub fn module_versions(&self) -> &BTreeMap<String, String> {
        &self.installed_versions
    }
    pub fn installed_modules(&self) -> BTreeSet<String> {
        self.installed_versions.keys().cloned().collect()
    }
    /// Installation/activity dependency, independent of execution order.
    /// Root-graph modules may own these declarations without system factories.
    pub fn require_module(&mut self, owner: &str, dependency: &str) -> Result<(), String> {
        if owner.trim().is_empty() || dependency.trim().is_empty() || owner == dependency {
            return Err(
                "Module dependency needs distinct nonempty owner and dependency IDs".into(),
            );
        }
        self.module_dependencies
            .entry(owner.into())
            .or_default()
            .insert(dependency.into());
        Ok(())
    }
    pub fn module_dependencies(&self) -> &BTreeMap<String, BTreeSet<String>> {
        &self.module_dependencies
    }
    pub fn set_module_version(&mut self, id: &str, version: &str) -> Result<(), String> {
        if !self.installed_versions.contains_key(id) || version.trim().is_empty() {
            return Err("Version needs an installed module and a nonempty value".into());
        }
        self.installed_versions.insert(id.into(), version.into());
        Ok(())
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
        self.installed_versions
            .entry(descriptor.id.clone())
            .or_insert_with(|| env!("CARGO_PKG_VERSION").into());
        self.entries
            .insert(descriptor.id.clone(), (descriptor, Arc::new(factory)));
        Ok(())
    }
    /// Bind a host implementation to an installed contract without replacing
    /// its ports, components, state schema or implementation version.
    pub fn bind_factory<F>(&mut self, id: &str, factory: F) -> Result<(), String>
    where
        F: Fn(&Value) -> Result<Box<dyn ExecutableSystem>, String> + Send + Sync + 'static,
    {
        let (_, installed) = self
            .entries
            .get_mut(id)
            .ok_or_else(|| format!("Cannot bind factory for unregistered system '{id}'"))?;
        *installed = Arc::new(factory);
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
        let mut result: BTreeSet<String> = ["mesh", "material"]
            .into_iter()
            .filter(|name| self.components.schema(name).is_some())
            .map(str::to_owned)
            .collect();
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
        self.validate_active_components(&result)?;
        Ok(result)
    }
    pub fn validate_active_components(&self, active: &BTreeSet<String>) -> Result<(), String> {
        for name in active {
            let schema = self
                .components
                .schema(name)
                .ok_or_else(|| format!("Active component '{name}' has no installed schema"))?;
            if self.module_version(&schema.owner_module).is_none() {
                return Err(format!(
                    "Component '{name}' owner '{}' is not installed",
                    schema.owner_module
                ));
            }
        }
        Ok(())
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

struct PreparedReplacement {
    descriptor: SystemDescriptor,
    implementation: Box<dyn ExecutableSystem>,
    /// None uses initialize's defaults; Some retains explicitly portable state.
    state: Option<Value>,
}

impl PreparedReplacement {
    fn cleanup(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Vec<String> {
        let mut errors = vec![];
        if let Err(error) = self.implementation.rollback_tick() {
            errors.push(format!("rollback: {error}"));
        }
        if let Err(error) = self.implementation.stop(ctx, state) {
            errors.push(format!("stop: {error}"));
        }
        errors
    }
}

struct LiveSystem {
    node: SystemNode,
    descriptor: SystemDescriptor,
    implementation: Box<dyn ExecutableSystem>,
    state: Value,
    initialized: bool,
    preserve_on_initialize: bool,
    pending: Option<PreparedReplacement>,
}

struct TickCheckpoint {
    states: Vec<(Value, bool)>,
    entered: usize,
}

/// Executor of arbitrary registered systems; it does not inspect module IDs.
pub struct SystemExecutor {
    systems: Vec<LiveSystem>,
    resources: ResourceStore,
    event_validators: Vec<crate::GameEventValidator>,
    begin_tick_hooks: Vec<crate::GameTickHook>,
}

impl SystemExecutor {
    pub fn new(graph: &ExecutionGraph, registry: &SystemRegistry) -> Result<Self, String> {
        registry.validate(graph)?;
        let mut systems = vec![];
        for node in graph.stages.iter().flat_map(|s| &s.nodes) {
            let (d, f) = &registry.entries[&node.module];
            systems.push(LiveSystem {
                node: node.clone(),
                descriptor: d.clone(),
                implementation: f(&node.params)?,
                state: Value::Null,
                initialized: false,
                preserve_on_initialize: false,
                pending: None,
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
        let event_validators = registry
            .active_game_contributions(graph)
            .iter()
            .map(|contribution| contribution.validate_event)
            .collect();
        let begin_tick_hooks = registry
            .active_game_contributions(graph)
            .iter()
            .map(|contribution| contribution.begin_tick)
            .collect();
        Ok(Self {
            systems,
            resources,
            event_validators,
            begin_tick_hooks,
        })
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
        let mut checkpoint = TickCheckpoint {
            states: self
                .systems
                .iter()
                .map(|s| (s.state.clone(), s.initialized))
                .collect(),
            entered: 0,
        };
        let mut result = self.run_tick(game, input, dt, events, &mut checkpoint, transaction);
        if result.is_err() {
            let mut rollback_errors = vec![];
            for system in self.systems.iter_mut().take(checkpoint.entered).rev() {
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
            for (system, (state, initialized)) in self.systems.iter_mut().zip(checkpoint.states) {
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
    /// Activation needs a real tick context. Initialization writes join this
    /// tick's transaction; teardown writes are discarded with its scratch DTOs.
    fn activate_replacement(
        &mut self,
        index: usize,
        game: &mut GameState,
        input: &GameInput,
        dt: f32,
        events: &mut Vec<GameEvent>,
        rollback_state: &mut (Value, bool),
    ) -> Result<(), String> {
        let system = &mut self.systems[index];
        let Some(mut candidate) = system.pending.take() else {
            return Ok(());
        };
        let mut old_game = game.clone();
        let mut old_resources = self.resources.clone();
        let mut old_events = events.clone();
        let before_initialize = candidate.state.clone().unwrap_or(Value::Null);
        let mut candidate_state = before_initialize.clone();
        let initialization = {
            let mut ctx = SystemContext {
                game,
                resources: &mut self.resources,
                events,
                dt,
                input,
                bindings: &system.node.bindings,
            };
            candidate
                .implementation
                .initialize(&mut ctx, &mut candidate_state)
        };
        let mut teardown = SystemContext {
            game: &mut old_game,
            resources: &mut old_resources,
            events: &mut old_events,
            dt: 0.,
            input,
            bindings: &system.node.bindings,
        };
        if let Err(error) = initialization {
            let cleanup = candidate.cleanup(&mut teardown, &mut candidate_state);
            let detail = if cleanup.is_empty() {
                String::new()
            } else {
                format!("; replacement cleanup failed: {}", cleanup.join("; "))
            };
            return Err(format!(
                "System '{}' replacement initialize: {error}{detail}",
                system.node.id
            ));
        }
        if system.initialized {
            // A stop hook can have irreversible private effects. Attempt it
            // once, retaining portable state and requiring reinitialization
            // if shutdown rejects the replacement.
            system.initialized = false;
            system.preserve_on_initialize = true;
            rollback_state.1 = false;
            let mut old_state = system.state.clone();
            if let Err(error) = system.implementation.stop(&mut teardown, &mut old_state) {
                let cleanup = candidate.cleanup(&mut teardown, &mut candidate_state);
                let detail = if cleanup.is_empty() {
                    String::new()
                } else {
                    format!("; replacement cleanup failed: {}", cleanup.join("; "))
                };
                return Err(format!(
                    "System '{}' replacement stop: {error}{detail}",
                    system.node.id
                ));
            }
        }
        // initialize always sets up the new implementation, but an explicit
        // retain/migrate choice wins over any JSON defaults it writes.
        if let Some(state) = &candidate.state {
            candidate_state = state.clone();
        }
        *rollback_state = (before_initialize, false);
        system.preserve_on_initialize = candidate.state.is_some();
        system.descriptor = candidate.descriptor;
        system.implementation = candidate.implementation;
        system.state = candidate_state;
        system.initialized = true;
        Ok(())
    }
    fn refresh_service_resources(
        resources: &mut ResourceStore,
        input: &GameInput,
        events: &[GameEvent],
    ) -> Result<(), String> {
        for resource in resources.values_mut() {
            resource.value = match resource.data_type.as_str() {
                "GameInput" => serde_json::to_value(input).map_err(|e| e.to_string())?,
                "GameEvents" => serde_json::to_value(events).map_err(|e| e.to_string())?,
                // GameWorld uses the typed capability, without copying entities.
                _ => resource.value.clone(),
            };
        }
        Ok(())
    }
    fn run_tick(
        &mut self,
        game: &mut GameState,
        input: &GameInput,
        dt: f32,
        events: &mut Vec<GameEvent>,
        checkpoint: &mut TickCheckpoint,
        transaction: &mut dyn TickTransaction,
    ) -> Result<(), String> {
        // step_with_transaction has already checkpointed all owned values.
        // A failed preparation follows the same rollback path as a failed
        // authored system and cannot initialize or retire a replacement.
        for begin_tick in &self.begin_tick_hooks {
            begin_tick(game).map_err(|error| format!("Game contribution begin_tick: {error}"))?;
        }
        for index in 0..self.systems.len() {
            Self::refresh_service_resources(&mut self.resources, input, events)?;
            self.activate_replacement(
                index,
                game,
                input,
                dt,
                events,
                &mut checkpoint.states[index],
            )?;
            checkpoint.entered = index + 1;
            let system = &mut self.systems[index];
            let mut ctx = SystemContext {
                game,
                resources: &mut self.resources,
                events,
                dt,
                input,
                bindings: &system.node.bindings,
            };
            if !system.initialized {
                let retained = system.preserve_on_initialize.then(|| system.state.clone());
                system
                    .implementation
                    .initialize(&mut ctx, &mut system.state)?;
                if let Some(state) = retained {
                    system.state = state;
                }
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
        for event in events.iter() {
            event.validate()?;
            for validate in &self.event_validators {
                validate(event)?;
            }
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
            // Prepared candidates never received initialize and own no active
            // lifecycle. Shutdown cancels them without invoking stop.
            system.pending = None;
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
            .map(|s| {
                let state = s.pending.as_ref().map_or_else(
                    || s.state.clone(),
                    |candidate| candidate.state.clone().unwrap_or(Value::Null),
                );
                (s.node.id.clone(), state)
            })
            .collect()
    }
    pub fn resources(&self) -> &ResourceStore {
        &self.resources
    }
    /// Prepare a replacement without changing the active implementation. The
    /// next tick initializes it before retiring the old instance. Its scheduling
    /// and component contracts must remain unchanged; a different state schema
    /// requires explicit migration or reset. `states` exposes the prepared state.
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
        let current = live
            .pending
            .as_ref()
            .map_or(&live.descriptor, |p| &p.descriptor);
        let current_ports: BTreeMap<_, _> = current.ports.iter().map(|p| (&p.name, p)).collect();
        let replacement_ports: BTreeMap<_, _> = d.ports.iter().map(|p| (&p.name, p)).collect();
        if current_ports != replacement_ports
            || current.components.iter().collect::<BTreeSet<_>>() != d.components.iter().collect()
            || current.requires.iter().collect::<BTreeSet<_>>() != d.requires.iter().collect()
        {
            return Err("Incompatible execution/component contract: ports, components and requires must remain unchanged; previous implementation retained".into());
        }
        if current.state_schema != d.state_schema && !reset && migrated.is_none() {
            return Err("Incompatible state schema: explicit reset or migration required; previous implementation retained".into());
        }
        let replacement = f(&live.node.params)?;
        let state = if reset {
            None
        } else if let Some(state) = migrated {
            Some(state)
        } else if let Some(candidate) = &live.pending {
            candidate.state.clone()
        } else if live.initialized || !live.state.is_null() || live.preserve_on_initialize {
            Some(live.state.clone())
        } else {
            None
        };
        live.pending = Some(PreparedReplacement {
            descriptor: d.clone(),
            implementation: replacement,
            state,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn empty_project() -> crate::GameProject {
        crate::GameProject {
            entry_scene: "empty".into(),
            scenes: BTreeMap::from([("empty".into(), crate::AuthoredScene::default())]),
            prefabs: BTreeMap::new(),
            execution: ExecutionGraph::default(),
            audio_cues: BTreeMap::new(),
        }
    }
    struct BoundNoop;
    impl ExecutableSystem for BoundNoop {
        fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            Ok(())
        }
    }
    #[test]
    fn installed_schema_owner_is_independent_from_consumers_and_factory_binding() {
        let mut registry = SystemRegistry::default();
        let descriptor = SystemDescriptor {
            id: "fixture.consumer".into(),
            state_schema: "fixture-state/2".into(),
            components: vec!["fixture.component".into()],
            ports: vec![],
            requires: vec![],
        };
        registry
            .register(descriptor.clone(), |_| Ok(Box::new(BoundNoop)))
            .unwrap();
        registry
            .set_module_version("fixture.consumer", "4.2")
            .unwrap();
        registry
            .components_mut()
            .register(crate::ComponentSchema {
                id: "fixture.component".into(),
                owner_module: "fixture.owner".into(),
                version: "2".into(),
                label: "Fixture".into(),
                defaults: json!({}),
                fields: vec![],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        let graph = ExecutionGraph {
            resources: vec![],
            stages: vec![SystemStage {
                name: "Fixture".into(),
                nodes: vec![SystemNode {
                    id: "consumer".into(),
                    module: "fixture.consumer".into(),
                    params: Value::Null,
                    bindings: BTreeMap::new(),
                    after: vec![],
                }],
            }],
        };
        assert!(
            registry
                .active_components(&graph)
                .unwrap_err()
                .contains("owner")
        );
        registry.install_module("fixture.owner", "7.0").unwrap();
        registry
            .require_module("root.fixture", "fixture.owner")
            .unwrap();
        assert_eq!(
            registry.module_dependencies()["root.fixture"],
            BTreeSet::from(["fixture.owner".into()])
        );
        assert!(
            registry
                .require_module("fixture.owner", "fixture.owner")
                .is_err()
        );
        assert!(registry.require_module("", "fixture.owner").is_err());
        assert_eq!(
            registry.active_components(&graph).unwrap(),
            BTreeSet::from(["fixture.component".into()])
        );
        let bound = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let created = bound.clone();
        registry
            .bind_factory("fixture.consumer", move |_| {
                created.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Box::new(BoundNoop))
            })
            .unwrap();
        assert_eq!(registry.descriptor("fixture.consumer"), Some(&descriptor));
        assert_eq!(registry.module_version("fixture.consumer"), Some("4.2"));
        SystemExecutor::new(&graph, &registry).unwrap();
        assert!(bound.load(std::sync::atomic::Ordering::SeqCst));
        assert!(
            registry
                .bind_factory("fixture.missing", |_| Ok(Box::new(BoundNoop)))
                .is_err()
        );
    }
    #[test]
    fn wrong_order_and_resource_type_are_rejected() {
        let (original, r) = graph_registry(false, "1");
        let mut g = original.clone();
        r.validate(&g).unwrap();
        g.stages[0].nodes.swap(0, 1);
        assert!(r.validate(&g).is_err());
        g = original.clone();
        g.resources[0].data_type = "Texture".into();
        assert!(r.validate(&g).unwrap_err().contains("Type mismatch"));
        g = original;
        g.stages[0].nodes[0].module = "custom.missing".into();
        assert!(r.validate(&g).unwrap_err().contains("factory"));
    }

    #[derive(Clone)]
    struct LifecycleConfig {
        label: &'static str,
        default_state: u64,
        reject_initialize: bool,
        reject_step: bool,
        reject_stop: bool,
        initialize_outputs: bool,
    }

    impl LifecycleConfig {
        fn new(label: &'static str, default_state: u64) -> Self {
            Self {
                label,
                default_state,
                reject_initialize: false,
                reject_step: false,
                reject_stop: false,
                initialize_outputs: false,
            }
        }
    }

    struct LifecycleProbe {
        config: LifecycleConfig,
        initialized: bool,
        calls: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl ExecutableSystem for LifecycleProbe {
        fn initialize(
            &mut self,
            ctx: &mut SystemContext<'_>,
            state: &mut Value,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}:initialize:{state}", self.config.label));
            self.initialized = true;
            *state = json!(self.config.default_state);
            if self.config.initialize_outputs {
                ctx.game.time += 50.;
                ctx.set_resource("value", json!(77))?;
                ctx.events.push(GameEvent::new(
                    "Custom",
                    json!({"name":self.config.label,"payload":"initialized"}),
                ));
            }
            if self.config.reject_initialize {
                Err("initialize rejected".into())
            } else {
                Ok(())
            }
        }

        fn step(&mut self, _: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}:step:{state}", self.config.label));
            if !self.initialized {
                return Err("step without initialize".into());
            }
            *state = json!(state.as_u64().unwrap() + 1);
            if self.config.reject_step {
                Err("step rejected".into())
            } else {
                Ok(())
            }
        }

        fn rollback_tick(&mut self) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}:rollback", self.config.label));
            Ok(())
        }

        fn stop(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}:stop", self.config.label));
            self.initialized = false;
            // Teardown is allowed to mutate its context, but these writes must
            // not replace the active portable state during hot replacement.
            *state = json!(999);
            ctx.game.time = 999.;
            ctx.set_resource("value", json!(999))?;
            ctx.events.push(GameEvent::new(
                "Custom",
                json!({"name":"teardown","payload":null}),
            ));
            if self.config.reject_stop {
                Err("stop rejected".into())
            } else {
                Ok(())
            }
        }
    }

    fn lifecycle_descriptor(schema: &str) -> SystemDescriptor {
        SystemDescriptor {
            id: "test.lifecycle".into(),
            state_schema: schema.into(),
            components: vec!["test-component".into()],
            ports: vec![SystemPort {
                name: "value".into(),
                data_type: "Number".into(),
                access: ResourceAccess::ReadWrite,
            }],
            requires: vec![],
        }
    }

    fn lifecycle_registry(
        schema: &str,
        config: LifecycleConfig,
        calls: &Arc<std::sync::Mutex<Vec<String>>>,
    ) -> SystemRegistry {
        let mut registry = SystemRegistry::default();
        let calls = calls.clone();
        registry
            .register(lifecycle_descriptor(schema), move |_| {
                Ok(Box::new(LifecycleProbe {
                    config: config.clone(),
                    initialized: false,
                    calls: calls.clone(),
                }))
            })
            .unwrap();
        registry
    }

    fn lifecycle_fixture(registry: &SystemRegistry) -> (SystemExecutor, GameState) {
        let graph = ExecutionGraph {
            resources: vec![SystemResource {
                name: "counter".into(),
                data_type: "Number".into(),
                external: true,
                initial: json!(0),
            }],
            stages: vec![SystemStage {
                name: "Lifecycle".into(),
                nodes: vec![SystemNode {
                    id: "probe".into(),
                    module: "test.lifecycle".into(),
                    params: Value::Null,
                    bindings: BTreeMap::from([("value".into(), "counter".into())]),
                    after: vec![],
                }],
            }],
        };
        let game = crate::GameSession::new(empty_project(), crate::WorldKind::Objects)
            .unwrap()
            .state()
            .clone();
        (SystemExecutor::new(&graph, registry).unwrap(), game)
    }

    fn call_count(calls: &Arc<std::sync::Mutex<Vec<String>>>, name: &str) -> usize {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.starts_with(name))
            .count()
    }

    #[test]
    fn replacement_initializes_retains_state_and_stops_each_live_implementation_once() {
        let calls = Arc::new(std::sync::Mutex::new(vec![]));
        let old = lifecycle_registry("1", LifecycleConfig::new("old", 10), &calls);
        let new = lifecycle_registry("1", LifecycleConfig::new("new", 100), &calls);
        let (mut executor, mut game) = lifecycle_fixture(&old);
        let mut events = vec![];
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        assert_eq!(executor.states()["probe"], json!(11));
        executor.replace("probe", &new, None, false).unwrap();
        assert_eq!(executor.states()["probe"], json!(11));
        assert_eq!(
            call_count(&calls, "old:stop"),
            0,
            "activation needs a tick context"
        );
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        assert_eq!(
            executor.states()["probe"],
            json!(12),
            "initializer defaults must not overwrite retained state"
        );
        assert_eq!(call_count(&calls, "new:initialize:11"), 1);
        assert_eq!(call_count(&calls, "old:stop"), 1);
        assert_eq!(
            game.time, 0.,
            "old teardown must not publish context writes"
        );
        assert_eq!(executor.resources()["counter"].value, json!(0));
        assert!(events.is_empty());
        executor.stop(&mut game, &mut events).unwrap();
        executor.stop(&mut game, &mut events).unwrap();
        assert_eq!(call_count(&calls, "old:stop"), 1);
        assert_eq!(call_count(&calls, "new:stop"), 1);
    }

    #[test]
    fn replacement_before_first_tick_can_be_superseded_migrated_and_reset() {
        let calls = Arc::new(std::sync::Mutex::new(vec![]));
        let old = lifecycle_registry("1", LifecycleConfig::new("old", 10), &calls);
        let superseded = lifecycle_registry("2", LifecycleConfig::new("superseded", 20), &calls);
        let new = lifecycle_registry("2", LifecycleConfig::new("new", 30), &calls);
        let reset = lifecycle_registry("3", LifecycleConfig::new("reset", 40), &calls);
        let (mut executor, mut game) = lifecycle_fixture(&old);
        let mut events = vec![];
        executor
            .replace("probe", &superseded, Some(json!(7)), false)
            .unwrap();
        executor.replace("probe", &new, None, false).unwrap();
        assert_eq!(executor.states()["probe"], json!(7));
        let mut broken = SystemRegistry::default();
        broken
            .register(
                lifecycle_descriptor("2"),
                |_| Err("factory rejected".into()),
            )
            .unwrap();
        assert!(
            executor
                .replace("probe", &broken, None, false)
                .unwrap_err()
                .contains("factory rejected")
        );
        assert_eq!(
            executor.states()["probe"],
            json!(7),
            "a rejected factory must retain the prepared candidate"
        );
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        assert_eq!(executor.states()["probe"], json!(8));
        assert_eq!(call_count(&calls, "old:"), 0);
        assert_eq!(call_count(&calls, "superseded:"), 0);
        assert_eq!(call_count(&calls, "new:initialize:7"), 1);
        executor.replace("probe", &reset, None, true).unwrap();
        assert!(executor.states()["probe"].is_null());
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        assert_eq!(
            executor.states()["probe"],
            json!(41),
            "reset uses initialization defaults"
        );
        assert_eq!(call_count(&calls, "new:stop"), 1);
        assert_eq!(call_count(&calls, "reset:initialize:null"), 1);
    }

    #[test]
    fn replacement_initialize_failure_discards_candidate_and_retains_old_context() {
        let calls = Arc::new(std::sync::Mutex::new(vec![]));
        let old = lifecycle_registry("1", LifecycleConfig::new("old", 10), &calls);
        let mut config = LifecycleConfig::new("rejected", 100);
        config.reject_initialize = true;
        config.initialize_outputs = true;
        let rejected = lifecycle_registry("1", config, &calls);
        let (mut executor, mut game) = lifecycle_fixture(&old);
        let mut events = vec![];
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        let original_game = game.clone();
        let original_resources = executor.resources().clone();
        executor.replace("probe", &rejected, None, false).unwrap();
        assert!(
            executor
                .step(&mut game, &GameInput::default(), 0.01, &mut events)
                .unwrap_err()
                .contains("replacement initialize")
        );
        assert_eq!(game, original_game);
        assert_eq!(executor.resources(), &original_resources);
        assert!(events.is_empty());
        assert_eq!(executor.states()["probe"], json!(11));
        assert_eq!(call_count(&calls, "old:stop"), 0);
        assert_eq!(call_count(&calls, "rejected:rollback"), 1);
        assert_eq!(call_count(&calls, "rejected:stop"), 1);
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        assert_eq!(executor.states()["probe"], json!(12));
        assert_eq!(call_count(&calls, "old:initialize"), 1);
        assert_eq!(call_count(&calls, "rejected:initialize"), 1);
    }

    #[test]
    fn replacement_stop_failure_retains_portable_state_and_reinitializes_old() {
        let calls = Arc::new(std::sync::Mutex::new(vec![]));
        let mut config = LifecycleConfig::new("old", 10);
        config.reject_stop = true;
        let old = lifecycle_registry("1", config, &calls);
        let mut config = LifecycleConfig::new("new", 100);
        config.initialize_outputs = true;
        let new = lifecycle_registry("1", config, &calls);
        let (mut executor, mut game) = lifecycle_fixture(&old);
        let mut events = vec![];
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        let original_game = game.clone();
        let original_resources = executor.resources().clone();
        executor.replace("probe", &new, None, false).unwrap();
        assert!(
            executor
                .step(&mut game, &GameInput::default(), 0.01, &mut events)
                .unwrap_err()
                .contains("replacement stop")
        );
        assert_eq!(game, original_game);
        assert_eq!(executor.resources(), &original_resources);
        assert!(events.is_empty());
        assert_eq!(executor.states()["probe"], json!(11));
        assert_eq!(call_count(&calls, "old:stop"), 1);
        assert_eq!(call_count(&calls, "new:stop"), 1);
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        assert_eq!(executor.states()["probe"], json!(12));
        assert_eq!(call_count(&calls, "old:stop"), 1);
        assert_eq!(call_count(&calls, "old:initialize:11"), 1);
        assert_eq!(call_count(&calls, "new:initialize"), 1);
    }

    #[derive(Default)]
    struct InitializationTransaction {
        reject: bool,
        validated_times: Vec<f64>,
        commits: usize,
        rollbacks: usize,
    }

    impl TickTransaction for InitializationTransaction {
        fn validate(&mut self, candidate: &GameState) -> Result<(), String> {
            self.validated_times.push(candidate.time);
            if self.reject {
                Err("host rejected".into())
            } else {
                Ok(())
            }
        }

        fn commit(&mut self, _: &GameState) -> Result<(), String> {
            self.commits += 1;
            Ok(())
        }

        fn rollback(&mut self) -> Result<(), String> {
            self.rollbacks += 1;
            Ok(())
        }
    }

    #[test]
    fn replacement_initialize_outputs_publish_with_tick_and_retry_after_host_rejection() {
        let calls = Arc::new(std::sync::Mutex::new(vec![]));
        let old = lifecycle_registry("1", LifecycleConfig::new("old", 10), &calls);
        let mut config = LifecycleConfig::new("new", 100);
        config.initialize_outputs = true;
        let new = lifecycle_registry("1", config, &calls);
        let (mut executor, mut game) = lifecycle_fixture(&old);
        let mut events = vec![];
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        let original_game = game.clone();
        let original_resources = executor.resources().clone();
        executor.replace("probe", &new, None, false).unwrap();
        let mut transaction = InitializationTransaction {
            reject: true,
            ..Default::default()
        };
        assert!(
            executor
                .step_with_transaction(
                    &mut game,
                    &GameInput::default(),
                    0.01,
                    &mut events,
                    &mut transaction
                )
                .is_err()
        );
        assert_eq!(game, original_game);
        assert_eq!(executor.resources(), &original_resources);
        assert!(events.is_empty());
        assert_eq!(executor.states()["probe"], json!(11));
        assert_eq!(call_count(&calls, "old:stop"), 1);
        assert_eq!(call_count(&calls, "new:rollback"), 1);
        transaction.reject = false;
        executor
            .step_with_transaction(
                &mut game,
                &GameInput::default(),
                0.01,
                &mut events,
                &mut transaction,
            )
            .unwrap();
        assert_eq!(game.time, 50.);
        assert_eq!(executor.resources()["counter"].value, json!(77));
        assert_eq!(
            events,
            vec![GameEvent::new(
                "Custom",
                json!({"name":"new","payload":"initialized"})
            )]
        );
        assert_eq!(executor.states()["probe"], json!(12));
        assert_eq!(call_count(&calls, "new:initialize:11"), 2);
        assert_eq!(call_count(&calls, "old:stop"), 1);
        assert_eq!(transaction.validated_times, vec![50., 50.]);
        assert_eq!(transaction.commits, 1);
        assert_eq!(transaction.rollbacks, 1);
    }

    #[test]
    fn replacement_initialize_reads_resources_after_authored_predecessors_run() {
        struct InitializedConsumer;
        impl ExecutableSystem for InitializedConsumer {
            fn initialize(
                &mut self,
                ctx: &mut SystemContext<'_>,
                state: &mut Value,
            ) -> Result<(), String> {
                let produced = ctx
                    .resource("value")?
                    .as_u64()
                    .ok_or("predecessor has not produced its resource")?;
                *state = json!(produced);
                Ok(())
            }
            fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
                Ok(())
            }
        }
        let (graph, registry) = graph_registry(false, "1");
        let mut replacement = SystemRegistry::default();
        replacement
            .register(
                registry.descriptor("third.consumer").unwrap().clone(),
                |_| Ok(Box::new(InitializedConsumer)),
            )
            .unwrap();
        let mut executor = SystemExecutor::new(&graph, &registry).unwrap();
        let mut game = crate::GameSession::new(empty_project(), crate::WorldKind::Objects)
            .unwrap()
            .state()
            .clone();
        executor.replace("b", &replacement, None, false).unwrap();
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut vec![])
            .unwrap();
        assert_eq!(executor.states()["a"], json!(1));
        assert_eq!(executor.states()["b"], json!(1));
    }

    #[test]
    fn replacement_contract_changes_are_rejected_before_factory_or_state_mutation() {
        let calls = Arc::new(std::sync::Mutex::new(vec![]));
        let old = lifecycle_registry("1", LifecycleConfig::new("old", 10), &calls);
        let (mut executor, mut game) = lifecycle_fixture(&old);
        let mut events = vec![];
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        let factories = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut contracts = vec![];
        let mut descriptor = lifecycle_descriptor("2");
        descriptor.components.clear();
        contracts.push(descriptor);
        let mut descriptor = lifecycle_descriptor("2");
        descriptor.ports[0].name = "renamed".into();
        contracts.push(descriptor);
        let mut descriptor = lifecycle_descriptor("2");
        descriptor.ports[0].data_type = "Text".into();
        contracts.push(descriptor);
        let mut descriptor = lifecycle_descriptor("2");
        descriptor.ports[0].access = ResourceAccess::Read;
        contracts.push(descriptor);
        let mut descriptor = lifecycle_descriptor("2");
        descriptor.ports.clear();
        contracts.push(descriptor);
        let mut descriptor = lifecycle_descriptor("2");
        descriptor.requires.push("missing.predecessor".into());
        contracts.push(descriptor);
        for descriptor in contracts {
            let factories = factories.clone();
            let mut registry = SystemRegistry::default();
            registry
                .register(descriptor, move |_| {
                    factories.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err("must not construct an incompatible replacement".into())
                })
                .unwrap();
            assert!(
                executor
                    .replace("probe", &registry, Some(json!(700)), true)
                    .unwrap_err()
                    .contains("contract")
            );
            assert_eq!(executor.states()["probe"], json!(11));
        }
        assert_eq!(factories.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(call_count(&calls, "old:stop"), 0);
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        assert_eq!(executor.states()["probe"], json!(12));
    }

    #[test]
    fn shutdown_cancels_a_prepared_replacement_without_initializing_it() {
        let calls = Arc::new(std::sync::Mutex::new(vec![]));
        let old = lifecycle_registry("1", LifecycleConfig::new("old", 10), &calls);
        let new = lifecycle_registry("1", LifecycleConfig::new("new", 100), &calls);
        let (mut executor, mut game) = lifecycle_fixture(&old);
        let mut events = vec![];
        executor
            .step(&mut game, &GameInput::default(), 0.01, &mut events)
            .unwrap();
        executor
            .replace("probe", &new, Some(json!(55)), false)
            .unwrap();
        executor.stop(&mut game, &mut events).unwrap();
        executor.stop(&mut game, &mut events).unwrap();
        assert_eq!(call_count(&calls, "old:stop"), 1);
        assert_eq!(call_count(&calls, "new:"), 0);
        assert_eq!(
            executor.states()["probe"],
            json!(999),
            "shutdown cancels the prepared state"
        );
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
        let mut project = empty_project();
        project.execution = g.clone();
        let session = crate::GameSession::new(project, crate::WorldKind::Objects);
        assert!(session.is_err());
        let mut project = empty_project();
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
        let mut game = crate::GameSession::new(empty_project(), crate::WorldKind::Objects)
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

    #[test]
    fn source_project_hooks_validate_strict_context_and_prepare_only_copies() {
        let mut registry = SystemRegistry::default();
        assert!(
            registry
                .register_project_contract("fixture.project", |_, _| Ok(()), |_| Ok(()))
                .is_err()
        );
        registry.install_module("fixture.project", "2").unwrap();
        registry
            .register_project_contract(
                "fixture.project",
                |project, context| {
                    if !context.modules.contains("fixture.project") {
                        return Err("Source project owner is unavailable".into());
                    }
                    crate::validate_component_asset_reference(
                        project
                            .audio_cues
                            .get("fixture.config")
                            .ok_or("Missing source configuration")?,
                        &["json".into()],
                        context,
                    )
                },
                |project| {
                    project.audio_cues.remove("fixture.editor-only");
                    if project
                        .audio_cues
                        .contains_key("fixture.reject-preparation")
                    {
                        return Err("Source preparation rejected".into());
                    }
                    Ok(())
                },
            )
            .unwrap();
        assert!(
            registry
                .register_project_contract("fixture.project", |_, _| Ok(()), |_| Ok(()))
                .is_err()
        );
        let mut project = crate::GameProject::empty();
        project.audio_cues = BTreeMap::from([
            ("fixture.config".into(), "asset://config".into()),
            (
                "fixture.editor-only".into(),
                "source authoring metadata".into(),
            ),
        ]);
        let source = project.clone();
        let modules = registry.installed_modules();
        let mut assets = BTreeMap::new();
        {
            let context = crate::ComponentValidationContext {
                scenes: &project.scenes,
                modules: &modules,
                asset_paths: Some(&assets),
                active_components: None,
            };
            assert!(
                project
                    .validate_with_context(&registry, &context)
                    .unwrap_err()
                    .contains("Unknown asset ID")
            );
        }
        assets.insert("config".into(), "assets/config.json".into());
        {
            let context = crate::ComponentValidationContext {
                scenes: &project.scenes,
                modules: &modules,
                asset_paths: Some(&assets),
                active_components: None,
            };
            let runtime = project
                .prepare_runtime_with_context(&registry.clone(), &context)
                .unwrap();
            assert_eq!(
                runtime.audio_cues,
                BTreeMap::from([("fixture.config".into(), "asset://config".into())])
            );
        }
        assert_eq!(project, source);
        project
            .audio_cues
            .insert("fixture.reject-preparation".into(), "reject".into());
        let rejected_source = project.clone();
        let context = crate::ComponentValidationContext {
            scenes: &project.scenes,
            modules: &modules,
            asset_paths: Some(&assets),
            active_components: None,
        };
        assert!(
            project
                .prepare_runtime_with_context(&registry, &context)
                .unwrap_err()
                .contains("Source preparation rejected")
        );
        assert_eq!(
            project, rejected_source,
            "Failed preparation must discard its partially changed copy"
        );
    }

    #[derive(Default)]
    struct ContributionPublication {
        value: u64,
        commits: usize,
        rollbacks: usize,
    }
    struct ContributionEmitter {
        private_counter: u64,
        before: u64,
        publication: Arc<std::sync::Mutex<ContributionPublication>>,
    }
    impl ExecutableSystem for ContributionEmitter {
        fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value) -> Result<(), String> {
            self.before = self.private_counter;
            self.private_counter += 1;
            *state = json!(self.private_counter);
            ctx.game.time += 1.;
            ctx.game.world.transform_mut(1).unwrap().translation[0] += 1.;
            ctx.game
                .module_data
                .insert("fixture.counter".into(), json!(self.private_counter));
            ctx.set_resource("value", json!(self.private_counter))?;
            ctx.events.push(GameEvent::new(
                "fixture.changed",
                json!({
                    "count": self.private_counter,
                    "accepted": ctx.input.pressed("accept"),
                }),
            ));
            Ok(())
        }
        fn commit_tick(&mut self) -> Result<(), String> {
            let mut publication = self.publication.lock().unwrap();
            publication.value = self.private_counter;
            publication.commits += 1;
            Ok(())
        }
        fn rollback_tick(&mut self) -> Result<(), String> {
            self.private_counter = self.before;
            self.publication.lock().unwrap().rollbacks += 1;
            Ok(())
        }
    }
    fn neutral_contribution() -> crate::GameContribution {
        crate::GameContribution {
            begin_tick: |_| Ok(()),
            id: "fixture.presentation".into(),
            activation_modules: BTreeSet::from(["fixture.optional".into()]),
            initial_camera: |_| crate::Camera::default(),
            follow_target: |_| None,
            decorate_snapshot: |_, _| {},
            script_input: |input| serde_json::to_value(input).unwrap(),
            validate_event: |event| {
                if event.name == "fixture.changed" && event.payload["accepted"] != json!(true) {
                    return Err("Source event contract rejected".into());
                }
                Ok(())
            },
            diagnostics: |_| json!({}),
        }
    }

    #[test]
    fn scheduled_contribution_event_rejection_rolls_back_all_staged_outputs() {
        let publication = Arc::new(std::sync::Mutex::new(ContributionPublication::default()));
        let mut registry = SystemRegistry::default();
        let shared = publication.clone();
        registry
            .register(
                SystemDescriptor {
                    id: "fixture.emitter".into(),
                    state_schema: "counter/1".into(),
                    components: vec![],
                    ports: vec![SystemPort {
                        name: "value".into(),
                        data_type: "Number".into(),
                        access: ResourceAccess::ReadWrite,
                    }],
                    requires: vec![],
                },
                move |_| {
                    Ok(Box::new(ContributionEmitter {
                        private_counter: 0,
                        before: 0,
                        publication: shared.clone(),
                    }))
                },
            )
            .unwrap();
        let contribution = neutral_contribution();
        assert!(
            registry
                .register_game_contribution(contribution.clone())
                .is_err()
        );
        registry
            .register(
                SystemDescriptor {
                    id: "fixture.optional".into(),
                    state_schema: "1".into(),
                    components: vec![],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(BoundNoop)),
            )
            .unwrap();
        registry
            .register_game_contribution(contribution.clone())
            .unwrap();
        assert!(
            registry
                .register_game_contribution(contribution.clone())
                .is_err()
        );
        let mut graph = ExecutionGraph {
            resources: vec![SystemResource {
                name: "counter".into(),
                data_type: "Number".into(),
                external: true,
                initial: json!(0),
            }],
            stages: vec![SystemStage {
                name: "Source".into(),
                nodes: vec![SystemNode {
                    id: "emitter".into(),
                    module: "fixture.emitter".into(),
                    params: Value::Null,
                    bindings: BTreeMap::from([("value".into(), "counter".into())]),
                    after: vec![],
                }],
            }],
        };
        let original_game = GameState {
            scene: "empty".into(),
            time: 0.,
            tick: 0,
            world: crate::EcsWorld::from_objects(vec![crate::WorldObject {
                id: 1,
                parent: None,
                transform: crate::Transform::default(),
                color: [255; 4],
            }])
            .unwrap(),
            module_data: BTreeMap::from([("fixture.untouched".into(), json!({"keep":[1,2,3]}))]),
            control: crate::SessionControl::default(),
        };
        assert!(!contribution.is_active(&graph));
        assert!(registry.active_game_contributions(&graph).is_empty());
        let mut inactive = SystemExecutor::new(&graph, &registry).unwrap();
        let mut inactive_game = original_game.clone();
        inactive
            .step(&mut inactive_game, &GameInput::default(), 0.01, &mut vec![])
            .unwrap();
        assert_eq!(
            publication.lock().unwrap().value,
            1,
            "Installation alone must not enable the event validator"
        );

        graph.stages[0].nodes.push(SystemNode {
            id: "optional".into(),
            module: "fixture.optional".into(),
            params: Value::Null,
            bindings: BTreeMap::new(),
            after: vec!["emitter".into()],
        });
        assert!(contribution.is_active(&graph));
        assert_eq!(
            registry
                .active_game_contributions(&graph)
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["fixture.presentation"]
        );
        let mut executor = SystemExecutor::new(&graph, &registry).unwrap();
        let mut game = original_game;
        let mut events = vec![GameEvent::new("fixture.previous", json!({"keep":true}))];
        let accepted = GameInput {
            actions: BTreeSet::from(["accept".into()]),
            ..Default::default()
        };
        executor
            .step(&mut game, &accepted, 0.01, &mut events)
            .unwrap();
        let before_game = game.clone();
        let before_states = executor.states();
        let before_resources = executor.resources().clone();
        let before_events = events.clone();
        let before_commits = publication.lock().unwrap().commits;
        let mut transaction = InitializationTransaction::default();
        let error = executor
            .step_with_transaction(
                &mut game,
                &GameInput::default(),
                0.01,
                &mut events,
                &mut transaction,
            )
            .unwrap_err();
        assert!(error.contains("Source event contract rejected"));
        assert_eq!(game, before_game);
        assert_eq!(executor.states(), before_states);
        assert_eq!(executor.resources(), &before_resources);
        assert_eq!(events, before_events);
        assert_eq!(publication.lock().unwrap().value, 1);
        assert_eq!(publication.lock().unwrap().commits, before_commits);
        assert_eq!(publication.lock().unwrap().rollbacks, 1);
        assert!(transaction.validated_times.is_empty());
        assert_eq!(transaction.commits, 0);
        assert_eq!(transaction.rollbacks, 1);
        executor
            .step(&mut game, &accepted, 0.01, &mut events)
            .unwrap();
        assert_eq!(
            executor.states()["emitter"],
            json!(2),
            "Rejected ticks must restore implementation-private counters before retry"
        );
        assert_eq!(executor.resources()["counter"].value, json!(2));
        assert_eq!(publication.lock().unwrap().value, 2);
        assert_eq!(game.world.object(1).unwrap().transform.translation[0], 2.);
        assert_eq!(
            game.module_data["fixture.untouched"],
            json!({"keep":[1,2,3]})
        );
        assert_eq!(events.len(), before_events.len() + 1);
    }
}
