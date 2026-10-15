//! Persisted, renderer-neutral scenes and linked templates. Runtime owns a copy.
use crate::{ExecutionGraph, SystemRegistry, Transform, WorldObject};
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
    /// Resolve current template defaults without modifying authored instances.
    pub fn resolve_scene(&self, id: &str) -> Result<Vec<GameEntity>, String> {
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
            for (path, value) in &authored.overrides {
                if path == "color" {
                    object.color = serde_json::from_value(value.clone())
                        .map_err(|e| format!("Invalid color override: {e}"))?;
                    continue;
                }
                let (component, fields) = path
                    .split_once('.')
                    .ok_or_else(|| format!("Override '{path}' needs component.field"))?;
                let mut target = components
                    .get_mut(component)
                    .ok_or_else(|| format!("Override '{path}' targets missing component"))?;
                for field in fields.split('.') {
                    target = target
                        .get_mut(field)
                        .ok_or_else(|| format!("Override '{path}' targets missing field"))?;
                }
                *target = value.clone();
            }
            result.push(GameEntity {
                object,
                components,
                references: authored.references.clone(),
            });
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
            crate::validate_game_components(entity, &self.scenes)?;
        }
        Ok(result)
    }

    /// Validate all scenes and the source-installed execution contracts.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with_registry(&SystemRegistry::builtin())
    }

    pub fn validate_with_registry(&self, registry: &SystemRegistry) -> Result<(), String> {
        self.validate_scene_data()?;
        registry.validate(&self.execution)
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
        for path in self.audio_cues.values() {
            if path.is_empty()
                || std::path::Path::new(path).is_absolute()
                || path.contains(':')
                || path.split(['\\', '/']).any(|p| p == "..")
            {
                return Err("Audio paths must be project-relative".into());
            }
        }
        Ok(())
    }
    /// Build Player/Team data with only installed active component types.
    /// Authoring data remains an archive in the original project.
    pub fn prepare_runtime(&self, registry: &SystemRegistry) -> Result<Self, String> {
        self.validate_with_registry(registry)?;
        let active = registry.active_components(&self.execution)?;
        let mut result = self.clone();
        for scene in result.scenes.values_mut() {
            for entity in &mut scene.objects {
                entity.components.retain(|name, _| active.contains(name));
                entity.overrides.retain(|path, _| {
                    path == "color"
                        || path
                            .split_once('.')
                            .is_some_and(|(name, _)| active.contains(name))
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
