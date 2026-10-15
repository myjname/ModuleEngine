//! Owned, portable command packets shared by world implementations and compiled
//! Rust workers. Commands use stable object/pass/resource IDs, not Rust pointers.

use crate::Transform;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Editable local object state. Parent IDs define the same hierarchy in both
/// built-in stores; transforms are local to that parent, never renderer matrices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldObject {
    pub id: u64,
    pub parent: Option<u64>,
    pub transform: Transform,
    pub color: [u8; 4],
}

/// Supported world mutations. Remove deletes the whole descendant subtree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WorldCommand {
    SetPosition { id: u64, position: [f32; 3] },
    Translate { id: u64, delta: [f32; 3] },
    SetTransform { id: u64, transform: Transform },
    SetColor { id: u64, color: [u8; 4] },
    Spawn { object: WorldObject },
    Remove { id: u64 },
}

/// Explicit named-resource operations, in submission order. The renderer must
/// reject unknown resources and incompatible copy/clear operations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RenderResourceCommand {
    ClearResource { name: String, value: [f32; 4] },
    CopyResource { from: String, to: String },
}

/// Portable render controls. Parameter compatibility is validated by the renderer
/// owning the selected pass contract; core never silently invents a GPU operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RenderCommand {
    SetParameter {
        pass: String,
        name: String,
        value: Value,
    },
    SetEnabled {
        pass: String,
        enabled: bool,
    },
    ClearResource {
        name: String,
        value: [f32; 4],
    },
    CopyResource {
        from: String,
        to: String,
    },
}

/// One serialized command; batches can be validated before mutating the world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ControlCommand {
    World(WorldCommand),
    Render(RenderCommand),
}

/// Persistent pass overrides and one-frame named resource operations.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RenderOverrides {
    pub parameters: BTreeMap<String, BTreeMap<String, Value>>,
    pub enabled: BTreeMap<String, bool>,
    pub resource_commands: Vec<RenderResourceCommand>,
}

impl RenderOverrides {
    /// Clear previous one-frame operations while retaining parameters/enabled.
    pub fn begin_frame(&mut self) {
        self.resource_commands.clear();
    }

    /// Validate and stage a complete batch; errors leave these overrides intact.
    /// Resource existence and pass-specific ranges belong to the GPU executor.
    pub fn apply_commands(&mut self, commands: &[RenderCommand]) -> Result<(), String> {
        if commands.len() > 4096 {
            return Err("Render command batch exceeds 4096 operations".into());
        }
        let mut candidate = self.clone();
        for command in commands {
            match command {
                RenderCommand::SetParameter { pass, name, value } => {
                    validate_name(pass)?;
                    validate_name(name)?;
                    if !value.as_f64().is_some_and(f64::is_finite) {
                        return Err("Render parameters must be finite numeric scalars".into());
                    }
                    if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > 65536 {
                        return Err("Render parameter exceeds 64 KiB".into());
                    }
                    candidate
                        .parameters
                        .entry(pass.clone())
                        .or_default()
                        .insert(name.clone(), value.clone());
                }
                RenderCommand::SetEnabled { pass, enabled } => {
                    validate_name(pass)?;
                    candidate.enabled.insert(pass.clone(), *enabled);
                }
                RenderCommand::ClearResource { name, value } => {
                    validate_name(name)?;
                    if !value.iter().all(|v| v.is_finite()) {
                        return Err("Clear values must be finite".into());
                    }
                    candidate
                        .resource_commands
                        .push(RenderResourceCommand::ClearResource {
                            name: name.clone(),
                            value: *value,
                        });
                }
                RenderCommand::CopyResource { from, to } => {
                    validate_name(from)?;
                    validate_name(to)?;
                    if from == to {
                        return Err("Resource copy needs distinct source and destination".into());
                    }
                    candidate
                        .resource_commands
                        .push(RenderResourceCommand::CopyResource {
                            from: from.clone(),
                            to: to.clone(),
                        });
                }
            }
        }
        if candidate.resource_commands.len() > 4096 {
            return Err("Pending resource operations exceed 4096".into());
        }
        *self = candidate;
        Ok(())
    }
}

/// Owned worker input and accumulated output commands. Editing the objects list
/// alone does not mutate the engine: emit commands through the helper methods.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptContext {
    pub time: f64,
    pub dt: f64,
    #[serde(default = "default_base")]
    pub base: f64,
    /// Whether the host could query the selected provider's local object state.
    /// False distinguishes unsupported queries from an available, empty world.
    /// Older packets without this field do not claim that capability.
    #[serde(default)]
    pub world_queries_available: bool,
    pub objects: Vec<WorldObject>,
    /// Project asset IDs resolve to portable paths beneath the worker's project
    /// working directory. Older hosts and packets supply an empty map.
    #[serde(default)]
    pub asset_paths: BTreeMap<String, String>,
    /// Validated exposed parameter values for this Project invocation.
    #[serde(default)]
    pub parameters: crate::PropertyValues,
    /// Applicable active properties/defaults; the inactive archive is excluded.
    #[serde(default)]
    pub properties: crate::PropertyData,
    #[serde(default)]
    pub commands: Vec<ControlCommand>,
    /// Host-owned, explicitly declared Custom state. Changes are returned as a
    /// candidate and committed only after the host accepts every command.
    #[serde(default)]
    pub state: Value,
    #[serde(default)]
    pub state_schema: String,
    /// Serialized input contract of the active gameplay module. Native hosts
    /// currently supply GameInput, while extensions may add their own fields.
    #[serde(default)]
    pub input: Value,
    /// Serialized gameplay events supplied for this invocation.
    #[serde(default)]
    pub events: Vec<Value>,
    /// New events are output, rather than mutations of the input event list.
    #[serde(default)]
    pub emitted_events: Vec<Value>,
}

fn default_base() -> f64 {
    1.0
}

impl ScriptContext {
    /// Create the input packet for one script invocation.
    pub fn new(time: f64, dt: f64, objects: Vec<WorldObject>) -> Self {
        Self {
            time,
            dt,
            base: 1.0,
            world_queries_available: true,
            objects,
            asset_paths: BTreeMap::new(),
            parameters: BTreeMap::new(),
            properties: crate::PropertyData::default(),
            commands: vec![],
            state: Value::Null,
            state_schema: String::new(),
            input: Value::Null,
            events: vec![],
            emitted_events: vec![],
        }
    }
    /// Create a render-only input when the provider does not support queries.
    /// The empty list is unavailable state, rather than evidence of no objects.
    pub fn without_world_queries(time: f64, dt: f64) -> Self {
        Self {
            world_queries_available: false,
            ..Self::new(time, dt, vec![])
        }
    }
    /// Read provider-neutral local object state by stable ID.
    /// Check world_queries_available to distinguish an unavailable query from a
    /// missing object; both otherwise produce None.
    pub fn object(&self, id: u64) -> Option<&WorldObject> {
        self.objects.iter().find(|o| o.id == id)
    }
    /// Resolve a stable asset ID (or asset://ID) for filesystem/content access.
    pub fn asset_path(&self, id: &str) -> Option<&str> {
        self.asset_paths
            .get(id.strip_prefix("asset://").unwrap_or(id))
            .map(String::as_str)
    }
    pub fn parameter(&self, id: &str) -> Option<&Value> {
        self.parameters.get(id)
    }
    pub fn property(&self, object: u64, id: &str) -> Option<&Value> {
        self.properties.objects.get(&object)?.get(id)
    }
    pub fn component_property(&self, object: u64, component: &str, id: &str) -> Option<&Value> {
        self.properties
            .components
            .get(&object)?
            .get(component)?
            .get(id)
    }
    /// Append an owned event for validation and dispatch by the host.
    pub fn emit_event(&mut self, event: Value) {
        self.emitted_events.push(event);
    }
    pub fn set_position(&mut self, id: u64, position: [f32; 3]) {
        self.commands
            .push(ControlCommand::World(WorldCommand::SetPosition {
                id,
                position,
            }));
    }
    pub fn translate(&mut self, id: u64, delta: [f32; 3]) {
        self.commands
            .push(ControlCommand::World(WorldCommand::Translate { id, delta }));
    }
    pub fn set_transform(&mut self, id: u64, transform: Transform) {
        self.commands
            .push(ControlCommand::World(WorldCommand::SetTransform {
                id,
                transform,
            }));
    }
    pub fn set_color(&mut self, id: u64, color: [u8; 4]) {
        self.commands
            .push(ControlCommand::World(WorldCommand::SetColor { id, color }));
    }
    pub fn spawn(&mut self, object: WorldObject) {
        self.commands
            .push(ControlCommand::World(WorldCommand::Spawn { object }));
    }
    pub fn remove(&mut self, id: u64) {
        self.commands
            .push(ControlCommand::World(WorldCommand::Remove { id }));
    }
    pub fn set_render_parameter(&mut self, pass: &str, name: &str, value: f64) {
        self.commands
            .push(ControlCommand::Render(RenderCommand::SetParameter {
                pass: pass.into(),
                name: name.into(),
                value: Value::from(value),
            }));
    }
    pub fn enable_render_pass(&mut self, pass: &str, enabled: bool) {
        self.commands
            .push(ControlCommand::Render(RenderCommand::SetEnabled {
                pass: pass.into(),
                enabled,
            }));
    }
    pub fn clear_resource(&mut self, name: &str, value: [f32; 4]) {
        self.commands
            .push(ControlCommand::Render(RenderCommand::ClearResource {
                name: name.into(),
                value,
            }));
    }
    pub fn copy_resource(&mut self, from: &str, to: &str) {
        self.commands
            .push(ControlCommand::Render(RenderCommand::CopyResource {
                from: from.into(),
                to: to.into(),
            }));
    }

    /// Separate the owned command packet without losing command order within
    /// either target. Cross-target validation is performed before host commit.
    pub fn split_commands(&self) -> (Vec<WorldCommand>, Vec<RenderCommand>) {
        let mut world = vec![];
        let mut render = vec![];
        for command in &self.commands {
            match command {
                ControlCommand::World(c) => world.push(c.clone()),
                ControlCommand::Render(c) => render.push(c.clone()),
            }
        }
        (world, render)
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.len() > 256 {
        Err("Pass/parameter/resource names need 1..256 bytes".into())
    } else {
        Ok(())
    }
}

fn valid_transform(transform: &Transform) -> bool {
    transform
        .translation
        .iter()
        .chain(&transform.rotation)
        .chain(&transform.scale)
        .all(|v| v.is_finite())
}

/// Common transactional semantics, using temporary DTOs rather than dictating
/// either provider's persistent storage representation.
pub(crate) fn apply_object_commands(
    objects: &mut BTreeMap<u64, WorldObject>,
    commands: &[WorldCommand],
) -> Result<(), String> {
    if commands.len() > 4096 {
        return Err("World command batch exceeds 4096 operations".into());
    }
    let mut candidate = objects.clone();
    for command in commands {
        match command {
            WorldCommand::SetPosition { id, position } => {
                if !position.iter().all(|v| v.is_finite()) {
                    return Err("Position must be finite".into());
                }
                candidate
                    .get_mut(id)
                    .ok_or_else(|| format!("Object {id} does not exist"))?
                    .transform
                    .translation = *position;
            }
            WorldCommand::Translate { id, delta } => {
                if !delta.iter().all(|v| v.is_finite()) {
                    return Err("Translation delta must be finite".into());
                }
                let object = candidate
                    .get_mut(id)
                    .ok_or_else(|| format!("Object {id} does not exist"))?;
                for (value, delta) in object.transform.translation.iter_mut().zip(delta) {
                    *value += delta;
                }
            }
            WorldCommand::SetTransform { id, transform } => {
                if !valid_transform(transform) {
                    return Err("Transform must be finite".into());
                }
                candidate
                    .get_mut(id)
                    .ok_or_else(|| format!("Object {id} does not exist"))?
                    .transform = *transform;
            }
            WorldCommand::SetColor { id, color } => {
                candidate
                    .get_mut(id)
                    .ok_or_else(|| format!("Object {id} does not exist"))?
                    .color = *color
            }
            WorldCommand::Spawn { object } => {
                if candidate.contains_key(&object.id) {
                    return Err(format!("Object {} already exists", object.id));
                }
                candidate.insert(object.id, object.clone());
            }
            WorldCommand::Remove { id } => {
                if !candidate.contains_key(id) {
                    return Err(format!("Object {id} does not exist"));
                }
                let mut removed = BTreeSet::from([*id]);
                loop {
                    let children: Vec<_> = candidate
                        .values()
                        .filter(|o| {
                            o.parent.is_some_and(|p| removed.contains(&p))
                                && !removed.contains(&o.id)
                        })
                        .map(|o| o.id)
                        .collect();
                    if children.is_empty() {
                        break;
                    }
                    removed.extend(children);
                }
                candidate.retain(|id, _| !removed.contains(id));
            }
        }
    }
    if candidate.len() > 65536 {
        return Err("Built-in editable world exceeds 65536 objects".into());
    }
    for object in candidate.values() {
        if !valid_transform(&object.transform) {
            return Err(format!("Object {} has a nonfinite transform", object.id));
        }
        let mut visited = BTreeSet::from([object.id]);
        let mut parent = object.parent;
        while let Some(id) = parent {
            if !visited.insert(id) {
                return Err(format!("Object {} creates a parent cycle", object.id));
            }
            if visited.len() > 128 {
                return Err("Built-in editable hierarchy exceeds depth 128".into());
            }
            parent = candidate
                .get(&id)
                .ok_or_else(|| format!("Parent {id} does not exist"))?
                .parent;
        }
    }
    *objects = candidate;
    Ok(())
}

/// Dry-run portable world validation. Hosts can stage renderer controls first,
/// then validate this batch before committing either side of a script packet.
pub fn validate_world_commands(
    objects: &[WorldObject],
    commands: &[WorldCommand],
) -> Result<(), String> {
    let mut map: BTreeMap<_, _> = objects.iter().cloned().map(|o| (o.id, o)).collect();
    if map.len() != objects.len() {
        return Err("Object packet has duplicate IDs".into());
    }
    apply_object_commands(&mut map, commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn script_context_distinguishes_empty_world_from_unavailable_queries_on_the_wire() {
        let available = ScriptContext::new(1.0, 0.016, vec![]);
        assert!(available.world_queries_available);
        assert!(available.object(1).is_none());
        let unavailable = ScriptContext::without_world_queries(1.0, 0.016);
        assert!(!unavailable.world_queries_available);
        assert!(unavailable.objects.is_empty());
        let json = serde_json::to_string(&unavailable).unwrap();
        assert_eq!(
            serde_json::from_str::<ScriptContext>(&json).unwrap(),
            unavailable
        );
        let legacy = serde_json::json!({"time":1.0,"dt":0.016,"objects":[]});
        let legacy: ScriptContext = serde_json::from_value(legacy).unwrap();
        assert!(!legacy.world_queries_available);
        assert_eq!(legacy.base, 1.0);
        assert!(legacy.commands.is_empty());
        assert!(legacy.asset_paths.is_empty());
    }
    #[test]
    fn stable_asset_paths_resolve_in_script_context_and_survive_wire_roundtrip() {
        let mut packet = ScriptContext::new(0., 0., vec![]);
        packet
            .asset_paths
            .insert("stable-model".into(), "assets/models/new-name.glb".into());
        assert_eq!(
            packet.asset_path("stable-model"),
            Some("assets/models/new-name.glb")
        );
        assert_eq!(
            packet.asset_path("asset://stable-model"),
            Some("assets/models/new-name.glb")
        );
        assert!(packet.asset_path("missing").is_none());
        let restored: ScriptContext =
            serde_json::from_value(serde_json::to_value(&packet).unwrap()).unwrap();
        assert_eq!(packet, restored);
    }
    #[test]
    fn render_override_batch_is_transactional_and_resource_operations_are_ordered() {
        let mut overrides = RenderOverrides::default();
        overrides
            .apply_commands(&[RenderCommand::SetParameter {
                pass: "tone".into(),
                name: "exposure".into(),
                value: Value::from(2.0),
            }])
            .unwrap();
        let before = overrides.clone();
        assert!(
            overrides
                .apply_commands(&[
                    RenderCommand::SetEnabled {
                        pass: "ssao".into(),
                        enabled: false
                    },
                    RenderCommand::CopyResource {
                        from: "same".into(),
                        to: "same".into()
                    }
                ])
                .is_err()
        );
        assert_eq!(before, overrides);
        overrides
            .apply_commands(&[
                RenderCommand::ClearResource {
                    name: "a.color".into(),
                    value: [0.; 4],
                },
                RenderCommand::CopyResource {
                    from: "a.color".into(),
                    to: "b.color".into(),
                },
            ])
            .unwrap();
        assert_eq!(overrides.resource_commands.len(), 2);
        overrides.begin_frame();
        assert!(overrides.resource_commands.is_empty());
        assert_eq!(overrides.parameters["tone"]["exposure"], 2.0);
    }
}
