//! Persisted, renderer-neutral scenes and linked templates. Runtime owns a copy.
use crate::{ComponentValidationContext, ExecutionGraph, SystemRegistry, Transform, WorldObject};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
    /// Optional audio module cue names map to project-relative PCM WAV files.
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
    /// Dot paths such as `health.maximum`; `color` overrides template color.
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
        if self
            .execution
            .stages
            .iter()
            .flat_map(|stage| &stage.nodes)
            .any(|node| node.module == "game.audio")
        {
            for reference in self.audio_cues.values() {
                crate::validate_component_asset_reference(reference, &["wav".into()], context)?;
            }
        }
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
        if !self
            .execution
            .stages
            .iter()
            .flat_map(|s| &s.nodes)
            .any(|n| n.module == "game.audio")
        {
            result.audio_cues.clear();
        }
        Ok(result)
    }

    /// The acceptance game is authored data using the same reusable systems.
    pub fn demo() -> Self {
        let prefab = Prefab {
            color: [190, 80, 85, 255],
            components: BTreeMap::from([
                (
                    "actor".into(),
                    json!({"team":"enemy","radius":0.36,"speed":2.1}),
                ),
                ("health".into(), json!({"maximum":36.0,"current":36.0})),
                ("ai".into(), json!({"kind":"chase","aggro_radius":14.0})),
                (
                    "melee".into(),
                    json!({"range":1.2,"damage":7.0,"cooldown":1.25,"arc":1.5,"duration":0.15,"windup":0.4}),
                ),
            ]),
        };
        fn object(id: u64, position: [f32; 3], scale: [f32; 3], color: [u8; 4]) -> AuthoredObject {
            AuthoredObject::new(WorldObject {
                id,
                parent: None,
                transform: Transform {
                    translation: position,
                    scale,
                    ..Transform::default()
                },
                color,
            })
        }
        fn arena(name: &str) -> AuthoredScene {
            let mut objects = vec![object(
                1000,
                [0., -0.5, 0.],
                [8., 0.2, 8.],
                [60, 78, 86, 255],
            )];
            for (id, pos, scale) in [
                (1001, [-8., 0.5, 0.], [0.2, 1., 8.]),
                (1002, [8., 0.5, 0.], [0.2, 1., 8.]),
                (1003, [0., 0.5, 8.], [8., 1., 0.2]),
                (1004, [-4.75, 0.5, -8.], [3.25, 1., 0.2]),
                (1005, [4.75, 0.5, -8.], [3.25, 1., 0.2]),
                (1006, [-3., 0.4, 1.], [0.8, 0.8, 0.8]),
            ] {
                let mut wall = object(id, pos, scale, [95, 104, 119, 255]);
                wall.components.insert(
                    "obstacle".into(),
                    json!({"half_extents":[scale[0],scale[2]]}),
                );
                objects.push(wall);
            }
            let mut player = object(1, [0., 0., 5.], [0.35, 0.7, 0.35], [70, 170, 245, 255]);
            player.components = BTreeMap::from([
                ("player".into(), json!({})),
                ("persistent".into(), json!({})),
                (
                    "actor".into(),
                    json!({"team":"player","radius":0.35,"speed":6.0}),
                ),
                ("health".into(), json!({"maximum":100.0,"current":100.0})),
                (
                    "melee".into(),
                    json!({"range":2.2,"damage":28.0,"cooldown":0.35,"arc":1.9,"duration":0.12,"windup":0.0}),
                ),
            ]);
            objects.push(player);
            AuthoredScene {
                name: name.into(),
                objects,
            }
        }
        let mut first = arena("Small monsters");
        for index in 0..20_u64 {
            let mut monster = object(
                index + 10,
                [
                    -5. + (index % 5) as f32 * 2.4,
                    0.,
                    -5. + (index / 5) as f32 * 1.6,
                ],
                [0.3, 0.45, 0.3],
                [190, 80, 85, 255],
            );
            monster.prefab = Some("small-monster".into());
            first.objects.push(monster);
        }
        let mut passage = object(1100, [0., 0., -7.2], [1.4, 0.08, 0.5], [60, 190, 140, 255]);
        passage.components.insert(
            "portal".into(),
            json!({"scene":"boss","radius":1.1,"requires_clear":true,"spawn":[0.,0.,5.]}),
        );
        first.objects.push(passage);
        let mut second = arena("Boss arena");
        let mut boss = object(100, [0., 0., -3.], [0.9, 1.2, 0.9], [150, 65, 210, 255]);
        boss.components = BTreeMap::from([
            (
                "actor".into(),
                json!({"team":"enemy","radius":0.9,"speed":1.6}),
            ),
            ("health".into(), json!({"maximum":400.0,"current":400.0})),
            ("ai".into(), json!({"kind":"boss","aggro_radius":20.0})),
            ("boss".into(), json!({"victory_on_death":true})),
            (
                "melee".into(),
                json!({"range":3.0,"damage":30.0,"cooldown":2.2,"arc":6.3,"duration":0.1,"windup":1.0}),
            ),
        ]);
        second.objects.push(boss);
        Self {
            entry_scene: "arena".into(),
            scenes: BTreeMap::from([("arena".into(), first), ("boss".into(), second)]),
            prefabs: BTreeMap::from([("small-monster".into(), prefab)]),
            execution: ExecutionGraph::gameplay(),
            audio_cues: ["attack", "hit", "death", "victory"]
                .into_iter()
                .map(|cue| (cue.into(), format!("assets/audio/{cue}.wav")))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Noop;
    impl crate::ExecutableSystem for Noop {
        fn step(&mut self, _: &mut crate::SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            Ok(())
        }
    }
    fn schema_project() -> (GameProject, SystemRegistry) {
        let mut project = GameProject::demo();
        project.scenes = BTreeMap::from([(
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
        )]);
        project.prefabs.clear();
        project.audio_cues.clear();
        project.execution = ExecutionGraph::default();
        (project, SystemRegistry::builtin())
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
        let mut p = GameProject::demo();
        let a = &mut p.scenes.get_mut("arena").unwrap().objects;
        a.iter_mut()
            .find(|e| e.object.id == 10)
            .unwrap()
            .overrides
            .insert("health.maximum".into(), json!(80.));
        p.prefabs
            .get_mut("small-monster")
            .unwrap()
            .components
            .get_mut("health")
            .unwrap()["maximum"] = json!(60.);
        let resolved = p.resolve_scene("arena").unwrap();
        assert_eq!(
            resolved
                .iter()
                .find(|e| e.object.id == 10)
                .unwrap()
                .components["health"]["maximum"],
            80.
        );
        assert_eq!(
            resolved
                .iter()
                .find(|e| e.object.id == 11)
                .unwrap()
                .components["health"]["maximum"],
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
        let mut p = GameProject::demo();
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
