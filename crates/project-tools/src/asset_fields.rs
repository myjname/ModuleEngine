//! Asset semantics come from installed contracts, never from a JSON key name.
use module_engine_core::{
    ComponentRegistry, ComponentSchema, CompositionActivity, CompositionContext, GraphDocument,
    PropertyData, PropertyScope, PropertyType, SchemaPin, component_override_owner,
    register_builtin_component_schemas,
};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct AssetFields {
    pub references: BTreeSet<String>,
    known: BTreeSet<String>,
    activity: Option<CompositionActivity>,
}
fn token(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
fn child(base: &str, value: &str) -> String {
    format!("{base}/{}", token(value))
}
impl AssetFields {
    pub fn collect(
        document: &GraphDocument,
        composition: Option<&CompositionContext>,
    ) -> Result<Self, String> {
        let mut builtin = ComponentRegistry::default();
        register_builtin_component_schemas(&mut builtin)?;
        let activity = composition
            .map(|context| context.activity(document))
            .transpose()?;
        let mut fields = Self {
            activity: activity.clone(),
            ..Self::default()
        };
        let schema = |id: &str| -> Option<&ComponentSchema> {
            if let Some(context) = composition {
                let schema = context.systems.components().schema(id)?;
                if !owner_matches(document, context, &schema.owner_module)
                    || !activity.as_ref()?.components.contains(id)
                    || !schema_matches(document, schema)
                {
                    return None;
                }
                Some(schema)
            } else {
                // Source-only callers have only these explicit, renderer-neutral
                // contracts. Custom components require with_composition.
                let schema = builtin.schema(id)?;
                if !schema_matches(document, schema) {
                    return None;
                }
                Some(schema)
            }
        };
        if let Some(context) = composition {
            fields.properties(
                &document.properties.active,
                "/properties/active",
                document,
                context,
                activity.as_ref().expect("composition activity"),
                &schema,
            );
            // Inactive property payload is archived author data. Its paths are
            // deliberately opaque until the contract participates again.
        }
        if let Some(game) = &document.game {
            for (id, prefab) in &game.prefabs {
                fields.components(
                    &prefab.components,
                    &format!("/game/prefabs/{}/components", token(id)),
                    &schema,
                );
            }
            for (id, scene) in &game.scenes {
                for (index, object) in scene.objects.iter().enumerate() {
                    let base = format!("/game/scenes/{}/objects/{index}", token(id));
                    fields.components(&object.components, &format!("{base}/components"), &schema);
                    let inherited = object.prefab.as_ref().and_then(|id| game.prefabs.get(id));
                    for field in object.overrides.keys() {
                        // Include every attachment, even opaque ones: a shorter
                        // known ID must not claim a longer unknown owner's data.
                        let owner = component_override_owner(
                            field,
                            object
                                .components
                                .keys()
                                .chain(inherited.into_iter().flat_map(|p| p.components.keys()))
                                .map(String::as_str),
                        );
                        if let Some(registered) = owner.and_then(schema).and_then(|schema| {
                            schema
                                .fields
                                .iter()
                                .find(|registered| registered.id == *field)
                        }) {
                            fields.add(child(&format!("{base}/overrides"), field), registered);
                        }
                    }
                }
            }
            for cue in game.audio_cues.keys() {
                fields.references.insert(child("/game/audio_cues", cue));
            }
        }
        Ok(fields)
    }
    fn add(&mut self, path: String, field: &module_engine_core::PropertySchema) {
        if self.activity.as_ref().is_some_and(|activity| {
            field
                .requires_modules
                .iter()
                .any(|id| !activity.modules.contains(id))
                || field
                    .requires_components
                    .iter()
                    .any(|id| !activity.components.contains(id))
        }) {
            return;
        }
        self.known.insert(path.clone());
        if matches!(field.value_type, PropertyType::AssetReference { .. }) {
            self.references.insert(path);
        }
    }
    fn components<'a>(
        &mut self,
        components: &std::collections::BTreeMap<String, Value>,
        base: &str,
        schema: &impl Fn(&str) -> Option<&'a ComponentSchema>,
    ) {
        for id in components.keys() {
            if let Some(schema) = schema(id) {
                for field in &schema.fields {
                    if let Some(path) = schema.field_path(field) {
                        let path = path
                            .split('.')
                            .fold(child(base, id), |base, part| child(&base, part));
                        self.add(path, field);
                    }
                }
            }
        }
    }
    fn properties<'a>(
        &mut self,
        data: &PropertyData,
        base: &str,
        document: &GraphDocument,
        composition: &CompositionContext,
        activity: &CompositionActivity,
        schema: &impl Fn(&str) -> Option<&'a ComponentSchema>,
    ) {
        for module in composition.modules.descriptors() {
            if !owner_matches(document, composition, &module.id)
                || !activity.modules.contains(&module.id)
            {
                continue;
            }
            for field in &module.properties {
                if let PropertyScope::Component { component } = &field.scope
                    && !activity.components.contains(component)
                {
                    continue;
                }
                let paths: Vec<_> = match &field.scope {
                    PropertyScope::Global => vec![child(&format!("{base}/global"), &field.id)],
                    PropertyScope::Module => vec![child(
                        &child(&format!("{base}/modules"), &module.id),
                        &field.id,
                    )],
                    PropertyScope::Object => data
                        .objects
                        .keys()
                        .map(|id| child(&format!("{base}/objects/{id}"), &field.id))
                        .collect(),
                    PropertyScope::Component { component } => data
                        .components
                        .keys()
                        .map(|id| {
                            child(
                                &child(&format!("{base}/components/{id}"), component),
                                &field.id,
                            )
                        })
                        .collect(),
                };
                for path in paths {
                    self.add(path, field);
                }
            }
        }
        // Legacy property stores use flat registered field IDs, not aggregate
        // component JSON. Both supported encodings have explicit contracts.
        for (object, components) in &data.components {
            for (id, values) in components {
                if let Some(schema) = schema(id) {
                    for field in &schema.fields {
                        for name in [Some(field.id.as_str()), schema.field_path(field)]
                            .into_iter()
                            .flatten()
                        {
                            if values.contains_key(name) {
                                self.add(
                                    child(&child(&format!("{base}/components/{object}"), id), name),
                                    field,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    /// Warn on an exact old reference in opaque data; never infer writable
    /// semantics from its spelling. Known Text fields are not dependencies.
    pub fn ambiguous(&self, value: &Value, matches: impl Fn(&str) -> bool) -> Vec<String> {
        let mut result = vec![];
        for base in [
            "/properties",
            "/game",
            "/root",
            "/legacy_archive",
            "/engine_profile",
        ] {
            if let Some(value) = value.pointer(base) {
                self.walk_ambiguous(value, base, &matches, &mut result);
            }
        }
        result
    }
    fn walk_ambiguous(
        &self,
        value: &Value,
        path: &str,
        matches: &impl Fn(&str) -> bool,
        result: &mut Vec<String>,
    ) {
        match value {
            Value::String(text)
                if !self.known.contains(path)
                    && !self.references.contains(path)
                    && matches(text) =>
            {
                result.push(format!("Opaque author data at {path} matches the old asset reference; preserved unchanged. Verify or update it explicitly before reactivating its contract."));
            }
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    self.walk_ambiguous(value, &format!("{path}/{index}"), matches, result);
                }
            }
            Value::Object(values) => {
                for (key, value) in values {
                    self.walk_ambiguous(value, &child(path, key), matches, result);
                }
            }
            _ => {}
        }
    }
}
fn owner_matches(document: &GraphDocument, composition: &CompositionContext, owner: &str) -> bool {
    composition.module_version(owner).is_some_and(|installed| {
        document
            .module_versions
            .get(owner)
            .is_none_or(|saved| saved == installed)
    })
}
fn schema_matches(document: &GraphDocument, schema: &ComponentSchema) -> bool {
    document.engine_lock.as_ref().is_none_or(|lock| {
        lock.component_schemas
            .get(&schema.id)
            .is_some_and(|saved| SchemaPin::capture(schema).ok().as_ref() == Some(saved))
    })
}

pub(crate) fn remove_field(value: &mut Value, pointer: &str) {
    let Some((parent, name)) = pointer.rsplit_once('/') else {
        return;
    };
    if let Some(parent) = value.pointer_mut(parent) {
        match parent {
            Value::Object(values) => {
                values.remove(&name.replace("~1", "/").replace("~0", "~"));
            }
            Value::Array(values) => {
                if let Ok(index) = name.parse::<usize>()
                    && let Some(value) = values.get_mut(index)
                {
                    *value = Value::Null;
                }
            }
            _ => {}
        }
    }
}
