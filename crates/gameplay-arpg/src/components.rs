//! ARPG data contracts installed by the optional source package.
use module_engine_core::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Actor {
    pub(crate) team: String,
    pub(crate) radius: f32,
    pub(crate) speed: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Health {
    pub(crate) maximum: f32,
    pub(crate) current: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Ai {
    pub(crate) kind: String,
    pub(crate) aggro_radius: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Melee {
    pub(crate) range: f32,
    pub(crate) damage: f32,
    pub(crate) cooldown: f32,
    pub(crate) arc: f32,
    pub(crate) duration: f32,
    pub(crate) windup: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Obstacle {
    pub(crate) half_extents: [f32; 2],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Portal {
    pub(crate) scene: String,
    pub(crate) radius: f32,
    pub(crate) requires_clear: bool,
    pub(crate) spawn: [f32; 3],
}

/// Register ARPG schemas once; consuming systems declare which contracts run.
pub fn register_arpg_component_schemas(registry: &mut ComponentRegistry) -> Result<(), String> {
    let positive = PropertyType::Number {
        min: f32::from_bits(1) as f64,
        max: f32::MAX as f64,
    };
    let nonnegative = PropertyType::Number {
        min: 0.,
        max: f32::MAX as f64,
    };
    let number = PropertyType::Number {
        min: -(f32::MAX as f64),
        max: f32::MAX as f64,
    };
    fn schema(
        id: &str,
        owner: &str,
        defaults: Value,
        fields: Vec<(&str, PropertyType)>,
    ) -> ComponentSchema {
        ComponentSchema {
            id: id.into(),
            owner_module: owner.into(),
            version: "1".into(),
            label: id.into(),
            fields: fields
                .into_iter()
                .map(|(path, value_type)| PropertySchema {
                    id: format!("{id}.{path}"),
                    label: path.into(),
                    scope: PropertyScope::Component {
                        component: id.into(),
                    },
                    default: component_field(&defaults, path)
                        .expect("builtin default path")
                        .clone(),
                    value_type,
                    requires_modules: vec![],
                    requires_components: vec![],
                })
                .collect(),
            defaults,
            requires_modules: vec![],
            requires_components: vec![],
        }
    }
    for (id, owner) in [("player", "game.input"), ("persistent", "game.portals")] {
        registry.register(schema(id, owner, json!({}), vec![]))?;
    }
    registry.register(schema(
        "boss",
        "game.health",
        json!({"victory_on_death":false}),
        vec![("victory_on_death", PropertyType::Boolean)],
    ))?;
    registry.register_with_validator(
        schema(
            "actor",
            "game.movement",
            json!({"team":"neutral","radius":0.35,"speed":3.0}),
            vec![
                ("team", PropertyType::Text { max_len: 256 }),
                ("radius", positive.clone()),
                ("speed", nonnegative.clone()),
            ],
        ),
        |entity, _| {
            if entity.object.parent.is_some()
                || entity.components["actor"]["team"]
                    .as_str()
                    .is_none_or(str::is_empty)
            {
                return Err(format!(
                    "Object {} actor needs root transform and nonempty team",
                    entity.object.id
                ));
            }
            Ok(())
        },
    )?;
    registry.register_with_validator(
        schema(
            "health",
            "game.health",
            json!({"maximum":100.0,"current":100.0}),
            vec![
                ("maximum", positive.clone()),
                ("current", nonnegative.clone()),
            ],
        ),
        |entity, _| {
            let health = &entity.components["health"];
            if health["current"].as_f64() > health["maximum"].as_f64() {
                return Err(format!(
                    "Object {} health.current exceeds maximum",
                    entity.object.id
                ));
            }
            Ok(())
        },
    )?;
    registry.register(schema(
        "ai",
        "game.ai",
        json!({"kind":"chase","aggro_radius":14.0}),
        vec![
            (
                "kind",
                PropertyType::Enum {
                    choices: vec!["chase".into(), "boss".into()],
                },
            ),
            ("aggro_radius", nonnegative.clone()),
        ],
    ))?;
    registry.register(schema(
        "melee",
        "game.combat",
        json!({"range":1.2,"damage":7.0,"cooldown":1.25,"arc":1.5,"duration":0.15,"windup":0.4}),
        vec![
            ("range", positive.clone()),
            ("damage", nonnegative.clone()),
            ("cooldown", positive.clone()),
            ("arc", positive.clone()),
            ("duration", positive.clone()),
            ("windup", nonnegative),
        ],
    ))?;
    registry.register_with_validator(
        schema(
            "obstacle",
            "game.collision",
            json!({"half_extents":[0.5,0.5]}),
            vec![
                ("half_extents.0", positive.clone()),
                ("half_extents.1", positive.clone()),
            ],
        ),
        |entity, _| {
            if entity.object.parent.is_some()
                || entity.components["obstacle"]["half_extents"]
                    .as_array()
                    .is_none_or(|values| values.len() != 2)
            {
                return Err(format!(
                    "Object {} obstacle needs root transform and two extents",
                    entity.object.id
                ));
            }
            Ok(())
        },
    )?;
    registry.register_with_validator(
        schema(
            "portal",
            "game.portals",
            json!({"scene":"","radius":1.2,"requires_clear":true,"spawn":[0.,0.,0.]}),
            vec![
                ("scene", PropertyType::Text { max_len: 256 }),
                ("radius", positive),
                ("requires_clear", PropertyType::Boolean),
                ("spawn.0", number.clone()),
                ("spawn.1", number.clone()),
                ("spawn.2", number),
            ],
        ),
        |entity, context| {
            let portal = &entity.components["portal"];
            if portal["scene"]
                .as_str()
                .is_none_or(|scene| !context.scenes.contains_key(scene))
                || portal["spawn"]
                    .as_array()
                    .is_none_or(|values| values.len() != 3)
            {
                return Err(format!(
                    "Object {} portal needs an existing scene and three spawn coordinates",
                    entity.object.id
                ));
            }
            Ok(())
        },
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "reusable_contract_tests.rs"]
mod reusable_contract_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn registered_defaults_match_native_component_types() {
        let mut registry = ComponentRegistry::default();
        register_arpg_component_schemas(&mut registry).unwrap();
        let parse = |name| registry.defaults(name).unwrap().clone();
        let _: Actor = serde_json::from_value(parse("actor")).unwrap();
        let _: Health = serde_json::from_value(parse("health")).unwrap();
        let _: Ai = serde_json::from_value(parse("ai")).unwrap();
        let _: Melee = serde_json::from_value(parse("melee")).unwrap();
        let _: Obstacle = serde_json::from_value(parse("obstacle")).unwrap();
        let _: Portal = serde_json::from_value(parse("portal")).unwrap();
        assert_eq!(
            registry.defaults("boss").unwrap()["victory_on_death"],
            json!(false)
        );
    }
    fn entity(id: u64, components: Value) -> GameEntity {
        GameEntity {
            object: WorldObject {
                id,
                parent: None,
                transform: Transform::default(),
                color: [255; 4],
            },
            components: serde_json::from_value(components).unwrap(),
            references: BTreeMap::new(),
        }
    }
    #[test]
    fn registered_arpg_callbacks_enforce_cross_field_and_world_semantics() {
        let mut registry = ComponentRegistry::default();
        register_arpg_component_schemas(&mut registry).unwrap();
        let active = BTreeSet::from(["health".into(), "actor".into(), "portal".into()]);
        let scenes = BTreeMap::from([("entry".into(), AuthoredScene::default())]);
        let modules = BTreeSet::new();
        let context = ComponentValidationContext::source_only(&scenes, &modules);
        let mut value = entity(
            1,
            json!({"health":{"maximum":10.,"current":9.},"actor":{},"portal":{"scene":"entry"}}),
        );
        registry.resolve_defaults(&mut value, &active);
        registry
            .validate_entities(&[value.clone()], &active, &context)
            .unwrap();
        value.components.get_mut("health").unwrap()["current"] = json!(11.);
        assert!(
            registry
                .validate_entities(&[value.clone()], &active, &context)
                .unwrap_err()
                .contains("exceeds maximum")
        );
        value.components.get_mut("health").unwrap()["current"] = json!(9.);
        value.object.parent = Some(2);
        assert!(
            registry
                .validate_entities(&[value.clone()], &active, &context)
                .unwrap_err()
                .contains("root transform")
        );
        value.object.parent = None;
        value.components.get_mut("portal").unwrap()["scene"] = json!("missing");
        assert!(
            registry
                .validate_entities(&[value], &active, &context)
                .unwrap_err()
                .contains("existing scene")
        );
    }
}
