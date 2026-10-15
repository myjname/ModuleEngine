//! Source-owned component contracts, shared by authoring, runtime and export.
use crate::{AuthoredScene, GameEntity, PropertySchema, PropertyScope, PropertyType};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// One schema owner can have many consuming systems. Version identifies this
/// component's data contract, independently of the owner's implementation pin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentSchema {
    pub id: String,
    pub owner_module: String,
    pub version: String,
    pub label: String,
    /// Aggregate defaults retain array/object shape for dot-index field paths.
    pub defaults: Value,
    pub fields: Vec<PropertySchema>,
    #[serde(default)]
    pub requires_modules: Vec<String>,
    #[serde(default)]
    pub requires_components: Vec<String>,
}

impl ComponentSchema {
    /// Field IDs include the complete component ID, even when it contains dots.
    pub fn field_path<'a>(&self, field: &'a PropertySchema) -> Option<&'a str> {
        field
            .id
            .strip_prefix(&format!("{}.", self.id))
            .filter(|path| !path.is_empty())
    }
}

/// Per-project context. Some(empty_map) is strict: unknown asset IDs reject.
/// None is explicitly source-only and never claims asset referents were checked.
pub struct ComponentValidationContext<'a> {
    pub scenes: &'a BTreeMap<String, AuthoredScene>,
    pub modules: &'a BTreeSet<String>,
    pub asset_paths: Option<&'a BTreeMap<String, String>>,
    /// Derived once from the complete document; None is the source-only graph
    /// convenience with renderer-neutral component support.
    pub active_components: Option<&'a BTreeSet<String>>,
}
impl<'a> ComponentValidationContext<'a> {
    pub fn source_only(
        scenes: &'a BTreeMap<String, AuthoredScene>,
        modules: &'a BTreeSet<String>,
    ) -> Self {
        Self {
            scenes,
            modules,
            asset_paths: None,
            active_components: None,
        }
    }
}

type SemanticValidator =
    Arc<dyn Fn(&GameEntity, &ComponentValidationContext<'_>) -> Result<(), String> + Send + Sync>;

#[derive(Clone, Default)]
pub struct ComponentRegistry {
    entries: BTreeMap<String, (ComponentSchema, Option<SemanticValidator>)>,
}
impl ComponentRegistry {
    pub fn register(&mut self, schema: ComponentSchema) -> Result<(), String> {
        self.insert(schema, None)
    }
    pub fn register_with_validator<F>(
        &mut self,
        schema: ComponentSchema,
        validator: F,
    ) -> Result<(), String>
    where
        F: Fn(&GameEntity, &ComponentValidationContext<'_>) -> Result<(), String>
            + Send
            + Sync
            + 'static,
    {
        self.insert(schema, Some(Arc::new(validator)))
    }
    fn insert(
        &mut self,
        schema: ComponentSchema,
        validator: Option<SemanticValidator>,
    ) -> Result<(), String> {
        validate_component_schema(&schema)?;
        if self.entries.contains_key(&schema.id) {
            return Err(format!(
                "Component '{}' already has a schema owner",
                schema.id
            ));
        }
        for existing in self.schemas() {
            for (owner, other) in [(&schema, existing), (existing, &schema)] {
                if other.id.len() > owner.id.len()
                    && let Some(field) = owner.fields.iter().find(|field| {
                        field
                            .id
                            .strip_prefix(&other.id)
                            .is_some_and(|rest| rest.starts_with('.'))
                    })
                {
                    return Err(format!(
                        "Field '{}' conflicts with longer component ID '{}'",
                        field.id, other.id
                    ));
                }
            }
        }
        self.entries.insert(schema.id.clone(), (schema, validator));
        Ok(())
    }
    pub fn schema(&self, id: &str) -> Option<&ComponentSchema> {
        self.entries.get(id).map(|(schema, _)| schema)
    }
    pub fn schemas(&self) -> impl Iterator<Item = &ComponentSchema> {
        self.entries.values().map(|(schema, _)| schema)
    }
    pub fn defaults(&self, id: &str) -> Option<&Value> {
        self.schema(id).map(|schema| &schema.defaults)
    }
    pub fn resolve_component(&self, id: &str, value: &Value) -> Value {
        let mut result = value.clone();
        if let Some(defaults) = self.defaults(id) {
            merge_component_defaults(&mut result, defaults);
        }
        result
    }
    /// Fill only attached, active components. Inactive/unknown bytes stay exact.
    pub fn resolve_defaults(&self, entity: &mut GameEntity, active: &BTreeSet<String>) {
        for (id, value) in &mut entity.components {
            if active.contains(id)
                && let Some(defaults) = self.defaults(id)
            {
                merge_component_defaults(value, defaults);
            }
        }
    }
    pub fn validate_entities(
        &self,
        entities: &[GameEntity],
        active: &BTreeSet<String>,
        context: &ComponentValidationContext<'_>,
    ) -> Result<(), String> {
        for entity in entities {
            for (id, value) in &entity.components {
                if !active.contains(id) {
                    continue;
                }
                let Some((schema, validator)) = self.entries.get(id) else {
                    return Err(format!("Active component '{id}' has no installed schema"));
                };
                if !value.is_object() {
                    return Err(format!(
                        "Object {} component '{id}' must be an object",
                        entity.object.id
                    ));
                }
                validate_dependencies(
                    entity,
                    active,
                    context,
                    &schema.requires_modules,
                    &schema.requires_components,
                )?;
                for field in &schema.fields {
                    validate_dependencies(
                        entity,
                        active,
                        context,
                        &field.requires_modules,
                        &field.requires_components,
                    )?;
                    let path = schema
                        .field_path(field)
                        .ok_or("Invalid registered field path")?;
                    let value = component_field(value, path).ok_or_else(|| {
                        format!(
                            "Object {} is missing field '{}'",
                            entity.object.id, field.id
                        )
                    })?;
                    field.value_type.validate(value).map_err(|error| {
                        format!("Object {} field '{}': {error}", entity.object.id, field.id)
                    })?;
                    match &field.value_type {
                        PropertyType::ObjectReference {
                            required_components,
                            ..
                        } if !value.is_null() => {
                            let target = entities
                                .iter()
                                .find(|target| Some(target.object.id) == value.as_u64())
                                .ok_or_else(|| {
                                    format!(
                                        "Object {} field '{}' references a missing scene object",
                                        entity.object.id, field.id
                                    )
                                })?;
                            if required_components.iter().any(|name| {
                                !active.contains(name) || !target.components.contains_key(name)
                            }) {
                                return Err(format!(
                                    "Object {} field '{}' target lacks required active components",
                                    entity.object.id, field.id
                                ));
                            }
                        }
                        PropertyType::AssetReference { kinds, .. } if !value.is_null() => {
                            validate_component_asset_reference(
                                value.as_str().ok_or("Invalid asset reference")?,
                                kinds,
                                context,
                            )
                            .map_err(|error| {
                                format!("Object {} field '{}': {error}", entity.object.id, field.id)
                            })?;
                        }
                        _ => {}
                    }
                }
                if let Some(validator) = validator {
                    validator(entity, context)?;
                }
            }
        }
        Ok(())
    }

    /// Registered active asset fields, for export/dependency planning.
    pub fn asset_references(
        &self,
        entities: &[GameEntity],
        active: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, String> {
        let mut result = BTreeSet::new();
        for entity in entities {
            for (id, value) in &entity.components {
                if !active.contains(id) {
                    continue;
                }
                let schema = self
                    .schema(id)
                    .ok_or_else(|| format!("Active component '{id}' has no installed schema"))?;
                let resolved = self.resolve_component(id, value);
                for field in &schema.fields {
                    if matches!(field.value_type, PropertyType::AssetReference { .. })
                        && let Some(reference) = schema
                            .field_path(field)
                            .and_then(|path| component_field(&resolved, path))
                            .and_then(Value::as_str)
                    {
                        result.insert(reference.into());
                    }
                }
            }
        }
        Ok(result)
    }
}

fn validate_dependencies(
    entity: &GameEntity,
    active: &BTreeSet<String>,
    context: &ComponentValidationContext<'_>,
    modules: &[String],
    components: &[String],
) -> Result<(), String> {
    if let Some(module) = modules
        .iter()
        .find(|module| !context.modules.contains(*module))
    {
        return Err(format!(
            "Object {} component requires active module '{module}'",
            entity.object.id
        ));
    }
    if let Some(component) = components
        .iter()
        .find(|name| !active.contains(*name) || !entity.components.contains_key(*name))
    {
        return Err(format!(
            "Object {} component requires attached active component '{component}'",
            entity.object.id
        ));
    }
    Ok(())
}

pub fn validate_component_schema(schema: &ComponentSchema) -> Result<(), String> {
    if [
        &schema.id,
        &schema.owner_module,
        &schema.version,
        &schema.label,
    ]
    .iter()
    .any(|value| value.trim().is_empty())
        || !schema.defaults.is_object()
    {
        return Err(
            "Component schema needs nonempty ID/owner/version/label and object defaults".into(),
        );
    }
    if schema
        .requires_modules
        .iter()
        .chain(&schema.requires_components)
        .any(|id| id.trim().is_empty())
    {
        return Err("Component dependencies need nonempty IDs".into());
    }
    let mut paths = BTreeSet::new();
    for field in &schema.fields {
        crate::validate_property_schema(field)?;
        if field.scope
            != (PropertyScope::Component {
                component: schema.id.clone(),
            })
        {
            return Err(format!(
                "Field '{}' has a different component scope",
                field.id
            ));
        }
        let path = schema
            .field_path(field)
            .ok_or_else(|| format!("Field '{}' needs component-prefixed ID", field.id))?;
        if path.split('.').any(str::is_empty) || !paths.insert(path) {
            return Err(format!(
                "Component '{}' has invalid or duplicate field path",
                schema.id
            ));
        }
        let default = component_field(&schema.defaults, path)
            .ok_or_else(|| format!("Field '{}' is missing aggregate default", field.id))?;
        if default != &field.default {
            return Err(format!(
                "Field '{}' has conflicting aggregate/field defaults",
                field.id
            ));
        }
    }
    Ok(())
}

/// A missing member takes its default; present scalar/null values are preserved
/// for validation. Array defaults fill missing indices without truncating data.
pub fn merge_component_defaults(value: &mut Value, defaults: &Value) {
    match (value, defaults) {
        (Value::Object(value), Value::Object(defaults)) => {
            for (key, default) in defaults {
                match value.get_mut(key) {
                    Some(value) => merge_component_defaults(value, default),
                    None => {
                        value.insert(key.clone(), default.clone());
                    }
                }
            }
        }
        (Value::Array(value), Value::Array(defaults)) => {
            for (index, default) in defaults.iter().enumerate() {
                if let Some(value) = value.get_mut(index) {
                    merge_component_defaults(value, default);
                } else {
                    value.push(default.clone());
                }
            }
        }
        _ => {}
    }
}
pub fn component_field<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |value, segment| match value {
            Value::Array(values) => values.get(segment.parse::<usize>().ok()?),
            Value::Object(values) => values.get(segment),
            _ => None,
        })
}
pub fn component_field_mut<'a>(value: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    let (head, tail) = path
        .split_once('.')
        .map_or((path, None), |(head, tail)| (head, Some(tail)));
    let child = match value {
        Value::Array(values) => values.get_mut(head.parse::<usize>().ok()?)?,
        Value::Object(values) => values.get_mut(head)?,
        _ => return None,
    };
    match tail {
        Some(tail) => component_field_mut(child, tail),
        None => Some(child),
    }
}
pub fn set_component_field(value: &mut Value, path: &str, field: Value) -> Result<(), String> {
    *component_field_mut(value, path)
        .ok_or_else(|| format!("Unknown component field path '{path}'"))? = field;
    Ok(())
}

pub fn validate_component_asset_reference(
    reference: &str,
    kinds: &[String],
    context: &ComponentValidationContext<'_>,
) -> Result<(), String> {
    crate::validate_asset_reference_syntax(reference)?;
    let path = if let Some(id) = reference.strip_prefix("asset://") {
        match context.asset_paths {
            Some(paths) => paths
                .get(id)
                .ok_or_else(|| format!("Unknown asset ID '{id}'"))?
                .as_str(),
            None => return Ok(()),
        }
    } else {
        reference
    };
    crate::validate_asset_reference_syntax(path)?;
    if path.starts_with("asset://") {
        return Err("Asset manifest path must be project-relative".into());
    }
    if !kinds.is_empty() {
        let extension = std::path::Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("");
        if !kinds
            .iter()
            .any(|kind| kind.eq_ignore_ascii_case(extension))
        {
            return Err(format!(
                "Asset '{reference}' needs one of the file kinds {kinds:?}"
            ));
        }
    }
    Ok(())
}

/// Register reusable builtins once; consumers are declared by SystemDescriptor.
pub fn register_builtin_component_schemas(registry: &mut ComponentRegistry) -> Result<(), String> {
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
    registry.register(schema(
        "mesh",
        "scene.extract",
        json!({"path":null}),
        vec![(
            "path",
            PropertyType::AssetReference {
                kinds: vec!["gltf".into(), "glb".into()],
                optional: true,
            },
        )],
    ))?;
    registry.register_with_validator(
        schema(
            "material",
            "scene.extract",
            json!({"base_color":[1.,1.,1.,1.],"base_color_texture":null}),
            vec![
                ("base_color.0", PropertyType::Number { min: 0., max: 1. }),
                ("base_color.1", PropertyType::Number { min: 0., max: 1. }),
                ("base_color.2", PropertyType::Number { min: 0., max: 1. }),
                ("base_color.3", PropertyType::Number { min: 0., max: 1. }),
                (
                    "base_color_texture",
                    PropertyType::AssetReference {
                        kinds: vec!["png".into(), "jpg".into(), "jpeg".into()],
                        optional: true,
                    },
                ),
            ],
        ),
        |entity, _| {
            if entity.components["material"]["base_color"]
                .as_array()
                .is_none_or(|values| values.len() != 4)
            {
                return Err(format!(
                    "Object {} material needs four color channels",
                    entity.object.id
                ));
            }
            Ok(())
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Transform, WorldObject};

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
    fn schema() -> ComponentSchema {
        ComponentSchema {
            id: "external.settings".into(),
            owner_module: "source.owner".into(),
            version: "settings/2".into(),
            label: "External settings".into(),
            defaults: json!({"speed":2.0,"direction":[0.,1.]}),
            fields: vec![
                PropertySchema {
                    id: "external.settings.speed".into(),
                    label: "Speed".into(),
                    scope: PropertyScope::Component {
                        component: "external.settings".into(),
                    },
                    value_type: PropertyType::Number { min: 0., max: 8. },
                    default: json!(2.),
                    requires_modules: vec![],
                    requires_components: vec![],
                },
                PropertySchema {
                    id: "external.settings.direction.0".into(),
                    label: "X".into(),
                    scope: PropertyScope::Component {
                        component: "external.settings".into(),
                    },
                    value_type: PropertyType::Number { min: -1., max: 1. },
                    default: json!(0.),
                    requires_modules: vec![],
                    requires_components: vec![],
                },
                PropertySchema {
                    id: "external.settings.direction.1".into(),
                    label: "Y".into(),
                    scope: PropertyScope::Component {
                        component: "external.settings".into(),
                    },
                    value_type: PropertyType::Number { min: -1., max: 1. },
                    default: json!(1.),
                    requires_modules: vec![],
                    requires_components: vec![],
                },
            ],
            requires_modules: vec![],
            requires_components: vec![],
        }
    }
    fn active() -> BTreeSet<String> {
        BTreeSet::from(["external.settings".into()])
    }

    #[test]
    fn schema_registration_rejects_ambiguous_owners_paths_and_defaults() {
        let mut registry = ComponentRegistry::default();
        registry.register(schema()).unwrap();
        let mut duplicate = schema();
        duplicate.owner_module = "another.owner".into();
        assert!(registry.register(duplicate).is_err());
        let mut invalid = schema();
        invalid.version.clear();
        assert!(validate_component_schema(&invalid).is_err());
        let mut invalid = schema();
        invalid.fields[0].id = "other.speed".into();
        assert!(validate_component_schema(&invalid).is_err());
        let mut invalid = schema();
        invalid.fields[0].default = json!(3.);
        assert!(validate_component_schema(&invalid).is_err());
        let mut invalid = schema();
        invalid.fields.push(invalid.fields[0].clone());
        assert!(validate_component_schema(&invalid).is_err());
        let mut invalid = schema();
        invalid.fields[0].value_type = PropertyType::Number { min: 9., max: 1. };
        assert!(validate_component_schema(&invalid).is_err());
    }
    #[test]
    fn prefix_component_ids_are_allowed_unless_a_declared_field_is_ambiguous() {
        let component = |id: &str, path: &str, defaults: Value| ComponentSchema {
            id: id.into(),
            owner_module: "source.owner".into(),
            version: "1".into(),
            label: id.into(),
            defaults,
            fields: vec![PropertySchema {
                id: format!("{id}.{path}"),
                label: path.into(),
                scope: PropertyScope::Component {
                    component: id.into(),
                },
                value_type: PropertyType::Number { min: 0., max: 8. },
                default: json!(1.),
                requires_modules: vec![],
                requires_components: vec![],
            }],
            requires_modules: vec![],
            requires_components: vec![],
        };
        let short = component("foo", "speed", json!({"speed":1.}));
        let long = component("foo.bar", "value", json!({"value":1.}));
        for schemas in [[short.clone(), long.clone()], [long.clone(), short]] {
            let mut registry = ComponentRegistry::default();
            for schema in schemas {
                registry.register(schema).unwrap();
            }
            assert_eq!(registry.schemas().count(), 2);
        }
        let ambiguous = component("foo", "bar.x", json!({"bar":{"x":1.}}));
        for schemas in [[ambiguous.clone(), long.clone()], [long, ambiguous]] {
            let mut registry = ComponentRegistry::default();
            registry.register(schemas[0].clone()).unwrap();
            let error = registry.register(schemas[1].clone()).unwrap_err();
            assert!(error.contains("foo.bar.x") && error.contains("foo.bar"));
            assert_eq!(
                registry.schemas().count(),
                1,
                "rejection must leave the catalog intact"
            );
        }
    }
    #[test]
    fn defaults_fill_nested_arrays_preserve_authored_values_and_inactive_bytes() {
        let mut registry = ComponentRegistry::default();
        registry.register(schema()).unwrap();
        let original = entity(
            1,
            json!({"external.settings":{"speed":5.,"direction":[-0.5]},"archived":{"broken":null}}),
        );
        let mut resolved = original.clone();
        registry.resolve_defaults(&mut resolved, &active());
        assert_eq!(
            resolved.components["external.settings"],
            json!({"speed":5.,"direction":[-0.5,1.]})
        );
        assert_eq!(
            resolved.components["archived"],
            original.components["archived"]
        );
        assert_eq!(
            original.components["external.settings"]["direction"],
            json!([-0.5])
        );
        set_component_field(
            resolved.components.get_mut("external.settings").unwrap(),
            "direction.1",
            json!(-1.),
        )
        .unwrap();
        assert_eq!(
            component_field(&resolved.components["external.settings"], "direction.1"),
            Some(&json!(-1.))
        );
        assert!(
            set_component_field(
                resolved.components.get_mut("external.settings").unwrap(),
                "direction.8",
                json!(0.)
            )
            .is_err()
        );
        let scenes = BTreeMap::new();
        let modules = BTreeSet::new();
        registry
            .validate_entities(
                &[resolved],
                &active(),
                &ComponentValidationContext::source_only(&scenes, &modules),
            )
            .unwrap();
        let wrong = registry.resolve_component("external.settings", &json!({"speed":null}));
        assert!(
            wrong["speed"].is_null(),
            "explicit bad types must remain visible to validation"
        );
    }
    #[test]
    fn constraints_and_component_module_dependencies_apply_only_to_active_data() {
        let mut definition = schema();
        definition.requires_modules.push("feature.enabled".into());
        definition.requires_components.push("marker".into());
        let mut registry = ComponentRegistry::default();
        registry.register(definition).unwrap();
        let scenes = BTreeMap::new();
        let mut modules = BTreeSet::new();
        let mut value = entity(1, json!({"external.settings":{"speed":99.},"marker":{}}));
        registry.resolve_defaults(&mut value, &active());
        registry
            .validate_entities(
                &[value.clone()],
                &BTreeSet::new(),
                &ComponentValidationContext::source_only(&scenes, &modules),
            )
            .unwrap();
        assert!(
            registry
                .validate_entities(
                    &[value.clone()],
                    &active(),
                    &ComponentValidationContext::source_only(&scenes, &modules)
                )
                .is_err()
        );
        modules.insert("feature.enabled".into());
        let active = BTreeSet::from(["external.settings".into(), "marker".into()]);
        registry
            .register(ComponentSchema {
                id: "marker".into(),
                owner_module: "source.owner".into(),
                version: "1".into(),
                label: "Marker".into(),
                defaults: json!({}),
                fields: vec![],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        assert!(
            registry
                .validate_entities(
                    &[value.clone()],
                    &active,
                    &ComponentValidationContext::source_only(&scenes, &modules)
                )
                .unwrap_err()
                .contains("finite number")
        );
        value.components.get_mut("external.settings").unwrap()["speed"] = json!(6.);
        registry
            .validate_entities(
                &[value.clone()],
                &active,
                &ComponentValidationContext::source_only(&scenes, &modules),
            )
            .unwrap();
        value.components.remove("marker");
        assert!(
            registry
                .validate_entities(
                    &[value],
                    &active,
                    &ComponentValidationContext::source_only(&scenes, &modules)
                )
                .unwrap_err()
                .contains("attached active component")
        );
    }
    #[test]
    fn typed_references_check_scene_targets_asset_ids_and_file_kinds() {
        let mut definition = schema();
        definition.defaults = json!({"target":2,"config":"asset://config"});
        definition.fields.truncate(2);
        definition.fields[0].id = "external.settings.target".into();
        definition.fields[0].default = json!(2);
        definition.fields[0].value_type = PropertyType::ObjectReference {
            required_components: vec!["external.settings".into()],
            optional: false,
        };
        definition.fields[1].id = "external.settings.config".into();
        definition.fields[1].default = json!("asset://config");
        definition.fields[1].value_type = PropertyType::AssetReference {
            kinds: vec!["json".into()],
            optional: false,
        };
        let mut registry = ComponentRegistry::default();
        registry.register(definition).unwrap();
        let mut entities = vec![
            entity(1, json!({"external.settings":{}})),
            entity(2, json!({"external.settings":{}})),
        ];
        for entity in &mut entities {
            registry.resolve_defaults(entity, &active());
        }
        let scenes = BTreeMap::new();
        let modules = BTreeSet::new();
        let mut assets = BTreeMap::new();
        let source = ComponentValidationContext::source_only(&scenes, &modules);
        registry
            .validate_entities(&entities, &active(), &source)
            .unwrap();
        let strict = ComponentValidationContext {
            scenes: &scenes,
            modules: &modules,
            asset_paths: Some(&assets),
            active_components: Some(&active()),
        };
        assert!(
            registry
                .validate_entities(&entities, &active(), &strict)
                .unwrap_err()
                .contains("Unknown asset ID")
        );
        assets.insert("config".into(), "data/config.JSON".into());
        let strict = ComponentValidationContext {
            scenes: &scenes,
            modules: &modules,
            asset_paths: Some(&assets),
            active_components: None,
        };
        registry
            .validate_entities(&entities, &active(), &strict)
            .unwrap();
        assert_eq!(
            registry.asset_references(&entities, &active()).unwrap(),
            BTreeSet::from(["asset://config".into()])
        );
        entities[0].components.get_mut("external.settings").unwrap()["target"] = json!(3);
        assert!(
            registry
                .validate_entities(&entities, &active(), &strict)
                .unwrap_err()
                .contains("missing scene object")
        );
        entities[0].components.get_mut("external.settings").unwrap()["target"] = json!(2);
        entities[1].components.clear();
        assert!(
            registry
                .validate_entities(&entities, &active(), &strict)
                .unwrap_err()
                .contains("lacks required")
        );
        assert!(
            validate_component_asset_reference("image.png", &["json".into()], &strict).is_err()
        );
        for invalid in [
            "../outside.json",
            "C:\\outside.json",
            "asset://",
            "/absolute.json",
        ] {
            assert!(
                PropertyType::AssetReference {
                    kinds: vec![],
                    optional: true
                }
                .validate(&json!(invalid))
                .is_err()
            );
        }
        assert!(
            PropertyType::ObjectReference {
                required_components: vec![],
                optional: false
            }
            .validate(&json!(0))
            .is_err()
        );
    }
    #[test]
    fn registered_builtin_callbacks_enforce_cross_field_and_world_semantics() {
        let mut registry = ComponentRegistry::default();
        register_builtin_component_schemas(&mut registry).unwrap();
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
