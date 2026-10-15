//! Persisted, renderer-neutral scenes and linked templates. Runtime owns a copy.
use crate::{ComponentValidationContext, ExecutionGraph, SystemRegistry, WorldObject};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Source project; scenes and templates use stable identifiers, never pointers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameProject {
    pub entry_scene: String,
    pub scenes: BTreeMap<String, AuthoredScene>,
    #[serde(default)]
    pub prefabs: BTreeMap<String, Prefab>,
    pub execution: ExecutionGraph,
    /// Archived cue metadata; installed source contracts validate and prepare it.
    #[serde(default)]
    pub audio_cues: BTreeMap<String, String>,
}

/// Saved scene. Object positions are authored, independently of Play state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoredScene {
    pub name: String,
    pub objects: Vec<AuthoredObject>,
}

/// Template component defaults and visual color; instance transforms stay local.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefab {
    pub color: [u8; 4],
    #[serde(default)]
    pub components: BTreeMap<String, Value>,
}

/// Object source, linked template and explicit per-instance field overrides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoredObject {
    pub object: WorldObject,
    #[serde(default)]
    pub prefab: Option<String>,
    #[serde(default)]
    pub components: BTreeMap<String, Value>,
    /// Dot paths such as `source.stats.value`; `color` overrides template color.
    #[serde(default)]
    pub overrides: BTreeMap<String, Value>,
    /// Scene-local object references; missing targets reject the complete scene.
    #[serde(default)]
    pub references: BTreeMap<String, u64>,
}

/// Resolved source object and active component data shared by world stores.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GameEntity {
    pub object: WorldObject,
    pub components: BTreeMap<String, Value>,
    #[serde(default)]
    pub references: BTreeMap<String, u64>,
}

impl AuthoredObject {
    /// Make a scene object without imposing any gameplay component.
    pub fn new(object: WorldObject) -> Self {
        Self {
            object,
            prefab: None,
            components: BTreeMap::new(),
            overrides: BTreeMap::new(),
            references: BTreeMap::new(),
        }
    }
}

impl GameProject {
    /// A neutral authored starting point with no installed gameplay assumptions.
    pub fn empty() -> Self {
        Self {
            entry_scene: "empty".into(),
            scenes: BTreeMap::from([(
                "empty".into(),
                AuthoredScene {
                    name: "Empty".into(),
                    objects: vec![],
                },
            )]),
            prefabs: BTreeMap::new(),
            execution: ExecutionGraph::default(),
            audio_cues: BTreeMap::new(),
        }
    }
    /// Resolve authored structure without requiring installed component schemas.
    /// Unavailable override fields stay archived until active schema resolution.
    pub fn resolve_scene(&self, id: &str) -> Result<Vec<GameEntity>, String> {
        self.resolve_scene_data(id, None, &BTreeSet::new())
    }
    /// Resolve and validate active components with syntax-only asset checks.
    /// Hosts with an asset manifest should use resolve_scene_with_context.
    pub fn resolve_scene_with_registry(
        &self,
        id: &str,
        registry: &SystemRegistry,
    ) -> Result<Vec<GameEntity>, String> {
        let modules = registry.installed_modules();
        let context = ComponentValidationContext::source_only(&self.scenes, &modules);
        self.resolve_scene_with_context(id, registry, &context)
    }
    pub fn resolve_scene_with_context(
        &self,
        id: &str,
        registry: &SystemRegistry,
        context: &ComponentValidationContext<'_>,
    ) -> Result<Vec<GameEntity>, String> {
        let active = self.active_components_with_context(registry, context)?;
        let entities = self.resolve_scene_data(id, Some(registry), &active)?;
        registry
            .components()
            .validate_entities(&entities, &active, context)?;
        Ok(entities)
    }
    pub(crate) fn active_components_with_context(
        &self,
        registry: &SystemRegistry,
        context: &ComponentValidationContext<'_>,
    ) -> Result<BTreeSet<String>, String> {
        registry.validate(&self.execution)?;
        if let Some(active) = context.active_components {
            registry.validate_active_components(active)?;
            Ok(active.clone())
        } else {
            registry.active_components(&self.execution)
        }
    }
    fn resolve_scene_data(
        &self,
        id: &str,
        registry: Option<&SystemRegistry>,
        active: &BTreeSet<String>,
    ) -> Result<Vec<GameEntity>, String> {
        let scene = self
            .scenes
            .get(id)
            .ok_or_else(|| format!("Scene '{id}' is missing"))?;
        let mut result = Vec::with_capacity(scene.objects.len());
        for authored in &scene.objects {
            let mut object = authored.object.clone();
            let mut components = BTreeMap::new();
            if let Some(id) = &authored.prefab {
                let prefab = self
                    .prefabs
                    .get(id)
                    .ok_or_else(|| format!("Prefab '{id}' is missing"))?;
                components = prefab.components.clone();
                object.color = prefab.color;
            }
            for (name, value) in &authored.components {
                if let (Some(Value::Object(base)), Value::Object(fields)) =
                    (components.get_mut(name), value)
                {
                    base.extend(fields.clone());
                } else {
                    components.insert(name.clone(), value.clone());
                }
            }
            let mut entity = GameEntity {
                object,
                components,
                references: authored.references.clone(),
            };
            if let Some(registry) = registry {
                registry.components().resolve_defaults(&mut entity, active);
            }
            for (path, value) in &authored.overrides {
                if path == "color" {
                    entity.object.color = serde_json::from_value(value.clone())
                        .map_err(|e| format!("Invalid color override: {e}"))?;
                    continue;
                }
                let component = entity
                    .components
                    .keys()
                    .filter(|name| {
                        path.strip_prefix(name.as_str())
                            .is_some_and(|rest| rest.starts_with('.'))
                    })
                    .max_by_key(|name| name.len())
                    .cloned()
                    .ok_or_else(|| format!("Override '{path}' targets missing component"))?;
                let fields = &path[component.len() + 1..];
                if fields.split('.').any(str::is_empty) {
                    return Err(format!("Override '{path}' needs component.field"));
                }
                let mut target = entity.components.get_mut(&component);
                for field in fields.split('.') {
                    target = target.and_then(|value| match value {
                        Value::Array(items) => {
                            field.parse::<usize>().ok().and_then(|i| items.get_mut(i))
                        }
                        _ => value.get_mut(field),
                    });
                }
                if let Some(target) = target {
                    *target = value.clone();
                } else if registry.is_some() && active.contains(&component) {
                    return Err(format!("Override '{path}' targets missing field"));
                }
            }
            result.push(entity);
        }
        let ids: BTreeSet<_> = result.iter().map(|e| e.object.id).collect();
        if ids.len() != result.len() || ids.contains(&0) {
            return Err(format!("Scene '{id}' needs unique nonzero object IDs"));
        }
        let world = result.iter().map(|e| e.object.clone()).collect::<Vec<_>>();
        crate::validate_world_commands(&world, &[])?;
        for entity in &result {
            for (name, target) in &entity.references {
                if !ids.contains(target) {
                    return Err(format!(
                        "Object {} reference '{name}' points to missing {target}",
                        entity.object.id
                    ));
                }
            }
        }
        Ok(result)
    }

    /// Validate all scenes and the source-installed execution contracts.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with_registry(&SystemRegistry::builtin())
    }

    pub fn validate_with_registry(&self, registry: &SystemRegistry) -> Result<(), String> {
        let modules = registry.installed_modules();
        let context = ComponentValidationContext::source_only(&self.scenes, &modules);
        self.validate_with_context(registry, &context)
    }
    pub fn validate_with_context(
        &self,
        registry: &SystemRegistry,
        context: &ComponentValidationContext<'_>,
    ) -> Result<(), String> {
        self.validate_scene_data()?;
        self.active_components_with_context(registry, context)?;
        for id in self.scenes.keys() {
            self.resolve_scene_with_context(id, registry, context)?;
        }
        registry.validate_project_contracts(self, context)?;
        Ok(())
    }
    /// Validate persisted data independently of the host's installed factories.
    pub fn validate_scene_data(&self) -> Result<(), String> {
        if !self.scenes.contains_key(&self.entry_scene) {
            return Err("Entry scene is missing".into());
        }
        if self.scenes.len() > 256 {
            return Err("Project exceeds 256 scenes".into());
        }
        for id in self.scenes.keys() {
            self.resolve_scene(id)?;
        }
        Ok(())
    }
    /// Build Player/Team data with only installed active component types.
    /// Authoring data remains an archive in the original project.
    pub fn prepare_runtime(&self, registry: &SystemRegistry) -> Result<Self, String> {
        let modules = registry.installed_modules();
        let context = ComponentValidationContext::source_only(&self.scenes, &modules);
        self.prepare_runtime_with_context(registry, &context)
    }
    pub fn prepare_runtime_with_context(
        &self,
        registry: &SystemRegistry,
        context: &ComponentValidationContext<'_>,
    ) -> Result<Self, String> {
        self.validate_with_context(registry, context)?;
        let active = self.active_components_with_context(registry, context)?;
        let mut result = self.clone();
        for scene in result.scenes.values_mut() {
            for entity in &mut scene.objects {
                // Resolve ownership against the complete authored attachment
                // set before stripping. A shorter active ID must not capture
                // overrides belonging to a longer inactive component ID.
                let component_ids: BTreeSet<_> = entity
                    .components
                    .keys()
                    .chain(
                        entity
                            .prefab
                            .as_ref()
                            .and_then(|id| self.prefabs.get(id))
                            .into_iter()
                            .flat_map(|prefab| prefab.components.keys()),
                    )
                    .cloned()
                    .collect();
                entity.components.retain(|name, _| active.contains(name));
                entity.overrides.retain(|path, _| {
                    path == "color"
                        || component_ids
                            .iter()
                            .filter(|name| {
                                path.strip_prefix(name.as_str())
                                    .is_some_and(|rest| rest.starts_with('.'))
                            })
                            .max_by_key(|name| name.len())
                            .is_some_and(|owner| active.contains(owner))
                });
            }
        }
        for prefab in result.prefabs.values_mut() {
            prefab.components.retain(|name, _| active.contains(name));
        }
        registry.prepare_project_contracts(&mut result)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transform;
    use serde_json::json;
    struct Noop;
    impl crate::ExecutableSystem for Noop {
        fn step(&mut self, _: &mut crate::SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            Ok(())
        }
    }
    fn schema_project() -> (GameProject, SystemRegistry) {
        let project = GameProject {
            entry_scene: "arena".into(),
            scenes: BTreeMap::from([(
                "arena".into(),
                AuthoredScene {
                    name: "Schema fixture".into(),
                    objects: vec![AuthoredObject::new(WorldObject {
                        id: 1,
                        parent: None,
                        transform: Transform::default(),
                        color: [255; 4],
                    })],
                },
            )]),
            prefabs: BTreeMap::new(),
            audio_cues: BTreeMap::new(),
            execution: ExecutionGraph::default(),
        };
        (project, SystemRegistry::builtin())
    }
    fn template_project() -> GameProject {
        let (mut project, _) = schema_project();
        project.prefabs.insert(
            "fixture.template".into(),
            Prefab {
                color: [80, 120, 160, 255],
                components: BTreeMap::from([("fixture.data".into(), json!({"count":36.}))]),
            },
        );
        let objects = &mut project.scenes.get_mut("arena").unwrap().objects;
        objects.clear();
        for id in [10, 11] {
            let mut object = AuthoredObject::new(WorldObject {
                id,
                parent: None,
                transform: Transform {
                    translation: [-5., 0., -5.],
                    ..Transform::default()
                },
                color: [255; 4],
            });
            object.prefab = Some("fixture.template".into());
            objects.push(object);
        }
        project
    }
    #[test]
    fn unclaimed_project_metadata_remains_archived_during_runtime_preparation() {
        let (mut project, registry) = schema_project();
        project
            .audio_cues
            .insert("source.cue".into(), "../archived-source-value".into());
        let source = project.clone();
        project.validate_with_registry(&registry).unwrap();
        assert_eq!(
            project.prepare_runtime(&registry).unwrap().audio_cues,
            source.audio_cues
        );
        assert_eq!(project, source);
    }
    fn activate_fixture(project: &mut GameProject, registry: &mut SystemRegistry) {
        registry
            .register(
                crate::SystemDescriptor {
                    id: "fixture.consumer".into(),
                    state_schema: "1".into(),
                    components: vec!["fixture.motion".into()],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(Noop)),
            )
            .unwrap();
        project.execution.stages.push(crate::SystemStage {
            name: "Fixture".into(),
            nodes: vec![crate::SystemNode {
                id: "fixture".into(),
                module: "fixture.consumer".into(),
                params: Value::Null,
                bindings: BTreeMap::new(),
                after: vec![],
            }],
        });
    }
    fn register_fixture_schema(registry: &mut SystemRegistry) {
        registry.install_module("fixture.owner", "2.0").unwrap();
        registry
            .components_mut()
            .register(crate::ComponentSchema {
                id: "fixture.motion".into(),
                owner_module: "fixture.owner".into(),
                version: "1".into(),
                label: "Motion".into(),
                defaults: json!({"speed":2.,"axis":[1.,2.,3.]}),
                fields: vec![crate::PropertySchema {
                    id: "fixture.motion.speed".into(),
                    label: "Speed".into(),
                    scope: crate::PropertyScope::Component {
                        component: "fixture.motion".into(),
                    },
                    value_type: crate::PropertyType::Number { min: 0., max: 10. },
                    default: json!(2.),
                    requires_modules: vec![],
                    requires_components: vec![],
                }],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
    }
    #[test]
    fn schema_defaults_precede_namespaced_overrides_without_changing_author_data() {
        let (mut project, mut registry) = schema_project();
        activate_fixture(&mut project, &mut registry);
        register_fixture_schema(&mut registry);
        let object = &mut project.scenes.get_mut("arena").unwrap().objects[0];
        object.components.insert("fixture.motion".into(), json!({}));
        object
            .overrides
            .insert("fixture.motion.speed".into(), json!(7.));
        object
            .overrides
            .insert("fixture.motion.axis.1".into(), json!(8.));
        let source = serde_json::to_value(&project).unwrap();
        project.validate_scene_data().unwrap();
        let entity = project
            .resolve_scene_with_registry("arena", &registry)
            .unwrap()
            .remove(0);
        assert_eq!(
            entity.components["fixture.motion"],
            json!({"speed":7.,"axis":[1.,8.,3.]})
        );
        project.validate_with_registry(&registry).unwrap();
        let prepared = project.prepare_runtime(&registry).unwrap();
        assert_eq!(
            prepared
                .resolve_scene_with_registry("arena", &registry)
                .unwrap()[0],
            entity
        );
        assert_eq!(serde_json::to_value(&project).unwrap(), source);
    }
    #[test]
    fn inactive_data_is_preserved_but_activation_needs_a_schema_and_valid_value() {
        let (mut project, mut registry) = schema_project();
        project.scenes.get_mut("arena").unwrap().objects[0]
            .components
            .insert("fixture.motion".into(), json!(17));
        let source = serde_json::to_value(&project).unwrap();
        project.validate_scene_data().unwrap();
        project.validate_with_registry(&registry).unwrap();
        assert!(
            !project.prepare_runtime(&registry).unwrap().scenes["arena"].objects[0]
                .components
                .contains_key("fixture.motion")
        );
        assert_eq!(serde_json::to_value(&project).unwrap(), source);
        activate_fixture(&mut project, &mut registry);
        assert!(
            project
                .validate_with_registry(&registry)
                .unwrap_err()
                .contains("no installed schema")
        );
        register_fixture_schema(&mut registry);
        assert!(
            project
                .validate_with_registry(&registry)
                .unwrap_err()
                .contains("must be an object")
        );
    }
    #[test]
    fn transient_activity_controls_the_same_validation_defaults_and_export_set() {
        let (mut project, registry) = schema_project();
        project.scenes.get_mut("arena").unwrap().objects[0]
            .components
            .insert("mesh".into(), json!({"path":false}));
        let modules = registry.installed_modules();
        let active = BTreeSet::new();
        let paths = BTreeMap::new();
        let context = ComponentValidationContext {
            scenes: &project.scenes,
            modules: &modules,
            asset_paths: Some(&paths),
            active_components: Some(&active),
        };
        project.validate_with_context(&registry, &context).unwrap();
        assert_eq!(
            project
                .resolve_scene_with_context("arena", &registry, &context)
                .unwrap()[0]
                .components["mesh"]["path"],
            json!(false)
        );
        assert!(
            project
                .prepare_runtime_with_context(&registry, &context)
                .unwrap()
                .scenes["arena"]
                .objects[0]
                .components
                .is_empty()
        );
        assert!(project.validate_with_registry(&registry).is_err());
    }
    #[test]
    fn runtime_stripping_keeps_override_owner_across_local_and_prefab_prefix_collisions() {
        let (mut project, mut registry) = schema_project();
        registry.install_module("prefix.owner", "1").unwrap();
        for id in ["foo", "foo.bar"] {
            registry
                .components_mut()
                .register(crate::ComponentSchema {
                    id: id.into(),
                    owner_module: "prefix.owner".into(),
                    version: "1".into(),
                    label: id.into(),
                    defaults: json!({"value":1.}),
                    fields: vec![crate::PropertySchema {
                        id: format!("{id}.value"),
                        label: "Value".into(),
                        scope: crate::PropertyScope::Component {
                            component: id.into(),
                        },
                        value_type: crate::PropertyType::Number { min: 0., max: 100. },
                        default: json!(1.),
                        requires_modules: vec![],
                        requires_components: vec![],
                    }],
                    requires_modules: vec![],
                    requires_components: vec![],
                })
                .unwrap();
        }
        registry
            .register(
                crate::SystemDescriptor {
                    id: "prefix.consumer".into(),
                    state_schema: "1".into(),
                    components: vec!["foo".into()],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(Noop)),
            )
            .unwrap();
        project.execution.stages.push(crate::SystemStage {
            name: "Prefix consumer".into(),
            nodes: vec![crate::SystemNode {
                id: "consume-foo".into(),
                module: "prefix.consumer".into(),
                params: Value::Null,
                bindings: BTreeMap::new(),
                after: vec![],
            }],
        });
        project.prefabs = BTreeMap::from([
            (
                "long".into(),
                Prefab {
                    color: [255; 4],
                    components: BTreeMap::from([("foo.bar".into(), json!({"value":20.}))]),
                },
            ),
            (
                "short".into(),
                Prefab {
                    color: [255; 4],
                    components: BTreeMap::from([("foo".into(), json!({"value":30.}))]),
                },
            ),
        ]);
        let objects = &mut project.scenes.get_mut("arena").unwrap().objects;
        objects[0].prefab = Some("long".into());
        objects[0].components.insert("foo".into(), json!({}));
        objects[0].overrides = BTreeMap::from([
            ("foo.value".into(), json!(7.)),
            ("foo.bar.value".into(), json!(80.)),
        ]);
        let mut second = AuthoredObject::new(WorldObject {
            id: 2,
            parent: None,
            transform: Transform::default(),
            color: [255; 4],
        });
        second.prefab = Some("short".into());
        second
            .components
            .insert("foo.bar".into(), json!({"value":40.}));
        second.overrides = BTreeMap::from([
            ("foo.value".into(), json!(9.)),
            ("foo.bar.value".into(), json!(90.)),
        ]);
        objects.push(second);
        let source = serde_json::to_value(&project).unwrap();
        let prepared = project.prepare_runtime(&registry).unwrap();
        prepared.validate_with_registry(&registry).unwrap();
        let resolved = prepared
            .resolve_scene_with_registry("arena", &registry)
            .unwrap();
        for (entity, expected) in resolved.iter().zip([7., 9.]) {
            assert_eq!(entity.components["foo"]["value"], json!(expected));
            assert!(!entity.components.contains_key("foo.bar"));
        }
        for object in &prepared.scenes["arena"].objects {
            assert_eq!(object.overrides.len(), 1);
            assert!(object.overrides.contains_key("foo.value"));
        }
        assert_eq!(serde_json::to_value(&project).unwrap(), source);
    }
    #[test]
    fn template_edits_propagate_except_overridden_fields() {
        let mut p = template_project();
        let a = &mut p.scenes.get_mut("arena").unwrap().objects;
        a.iter_mut()
            .find(|e| e.object.id == 10)
            .unwrap()
            .overrides
            .insert("fixture.data.count".into(), json!(80.));
        p.prefabs
            .get_mut("fixture.template")
            .unwrap()
            .components
            .get_mut("fixture.data")
            .unwrap()["count"] = json!(60.);
        let resolved = p.resolve_scene("arena").unwrap();
        assert_eq!(
            resolved
                .iter()
                .find(|e| e.object.id == 10)
                .unwrap()
                .components["fixture.data"]["count"],
            80.
        );
        assert_eq!(
            resolved
                .iter()
                .find(|e| e.object.id == 11)
                .unwrap()
                .components["fixture.data"]["count"],
            60.
        );
        assert_eq!(
            resolved
                .iter()
                .find(|e| e.object.id == 10)
                .unwrap()
                .object
                .transform
                .translation,
            [-5., 0., -5.]
        );
    }
    #[test]
    fn missing_reference_and_template_are_rejected() {
        let mut p = template_project();
        p.scenes.get_mut("arena").unwrap().objects[0]
            .references
            .insert("owner".into(), 999999);
        assert!(p.validate().is_err());
        p.scenes.get_mut("arena").unwrap().objects[0]
            .references
            .clear();
        p.prefabs.clear();
        assert!(p.validate().is_err());
    }
}
