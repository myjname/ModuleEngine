//! Asset semantics come from installed contracts, never from a JSON key name.
use module_engine_core::{
    ComponentRegistry, ComponentSchema, CompositionActivity, CompositionContext, GraphDocument,
    PropertyData, PropertyScope, PropertyType, SchemaPin, component_field,
    component_override_owner, merge_component_defaults, register_builtin_component_schemas,
    visible_property_schemas,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(crate) struct AssetFields {
    pub references: BTreeSet<String>,
    /// Effective references supplied by installed schemas have no writable
    /// authored pointer. Keep their value and provenance separate: inventing a
    /// pointer to an absent field silently loses this dependency during edits.
    pub default_references: BTreeMap<String, String>,
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
        property_objects: Option<&BTreeSet<u64>>,
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
                property_objects,
                &schema,
            )?;
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
                fields.defaults(
                    &prefab.components,
                    &format!("/game/prefabs/{}/components", token(id)),
                    None,
                    &schema,
                );
            }
            for (id, scene) in &game.scenes {
                for (index, object) in scene.objects.iter().enumerate() {
                    let base = format!("/game/scenes/{}/objects/{index}", token(id));
                    fields.components(&object.components, &format!("{base}/components"), &schema);
                    let inherited = object.prefab.as_ref().and_then(|id| game.prefabs.get(id));
                    let mut effective = inherited
                        .map(|prefab| prefab.components.clone())
                        .unwrap_or_default();
                    // Match scene resolution's shallow prefab/local merge;
                    // defaults are filled only after this authored merge.
                    for (id, value) in &object.components {
                        if let (Some(Value::Object(base)), Value::Object(local)) =
                            (effective.get_mut(id), value)
                        {
                            base.extend(local.clone());
                        } else {
                            effective.insert(id.clone(), value.clone());
                        }
                    }
                    fields.defaults(
                        &effective,
                        &format!("{base}/components"),
                        Some((&object.overrides, &format!("{base}/overrides"))),
                        &schema,
                    );
                }
            }
            for cue in game.audio_cues.keys() {
                fields.references.insert(child("/game/audio_cues", cue));
            }
        }
        Ok(fields)
    }
    fn add(&mut self, path: String, field: &module_engine_core::PropertySchema) {
        if !self.field_active(field) {
            return;
        }
        self.known.insert(path.clone());
        if matches!(field.value_type, PropertyType::AssetReference { .. }) {
            self.references.insert(path);
        }
    }
    fn field_active(&self, field: &module_engine_core::PropertySchema) -> bool {
        !self.activity.as_ref().is_some_and(|activity| {
            field
                .requires_modules
                .iter()
                .any(|id| !activity.modules.contains(id))
                || field
                    .requires_components
                    .iter()
                    .any(|id| !activity.components.contains(id))
        })
    }
    fn defaults<'a>(
        &mut self,
        components: &BTreeMap<String, Value>,
        base: &str,
        overrides: Option<(&BTreeMap<String, Value>, &str)>,
        schema: &impl Fn(&str) -> Option<&'a ComponentSchema>,
    ) {
        for (id, authored) in components {
            let Some(schema) = schema(id) else {
                continue;
            };
            let mut effective = authored.clone();
            merge_component_defaults(&mut effective, &schema.defaults);
            for field in &schema.fields {
                if !self.field_active(field) {
                    continue;
                }
                let Some(path) = schema.field_path(field) else {
                    continue;
                };
                let mut overridden = false;
                if let Some((overrides, override_base)) = overrides {
                    for key in overrides.keys() {
                        // Opaque longer attachment IDs still own their whole
                        // namespace; an active shorter schema cannot claim it.
                        if component_override_owner(key, components.keys().map(String::as_str))
                            != Some(id.as_str())
                        {
                            continue;
                        }
                        let tail = &key[id.len() + 1..];
                        let remainder = if path == tail {
                            Some("")
                        } else {
                            path.strip_prefix(tail)
                                .and_then(|rest| rest.strip_prefix('.'))
                        };
                        if let Some(remainder) = remainder {
                            overridden = true;
                            let pointer = remainder
                                .split('.')
                                .filter(|part| !part.is_empty())
                                .fold(child(override_base, key), |base, part| child(&base, part));
                            self.add(pointer, field);
                        }
                    }
                }
                if !overridden
                    && matches!(field.value_type, PropertyType::AssetReference { .. })
                    && component_field(authored, path).is_none()
                    && let Some(reference) =
                        component_field(&effective, path).and_then(Value::as_str)
                {
                    let location = format!(
                        "schema-default:{} at {}",
                        field.id,
                        path.split('.')
                            .fold(child(base, id), |base, part| child(&base, part))
                    );
                    self.default_references.insert(location, reference.into());
                }
            }
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
        property_objects: Option<&BTreeSet<u64>>,
        schema: &impl Fn(&str) -> Option<&'a ComponentSchema>,
    ) -> Result<(), String> {
        let mut contexts = vec![None];
        if let Some(objects) = property_objects {
            contexts.extend(objects.iter().copied().map(Some));
        } else {
            // Availability for unrestricted Object properties only needs a
            // Some(object); attachment-dependent fields also need the stored
            // per-object attachment map. These candidates establish whether
            // a live-ID snapshot is necessary, never actual dependencies.
            let candidates: BTreeSet<_> = data.components.keys().copied().chain([0]).collect();
            for object in candidates {
                if let Some((owner, field)) =
                    visible_property_schemas(document, &composition.modules, Some(object))
                        .into_iter()
                        .find(|(owner, field)| {
                            owner_matches(document, composition, owner)
                                && matches!(field.value_type, PropertyType::AssetReference { .. })
                        })
                {
                    return Err(format!(
                        "Asset dependency collection needs a current live property-object ID snapshot for {owner}/{}; provide with_property_object_ids before scanning or changing assets",
                        field.id
                    ));
                }
            }
        }
        for object in contexts {
            for (owner, field) in visible_property_schemas(document, &composition.modules, object) {
                if !owner_matches(document, composition, &owner) {
                    continue;
                }
                let (path, authored) = match &field.scope {
                    PropertyScope::Global => (
                        child(&format!("{base}/global"), &field.id),
                        data.global.get(&field.id),
                    ),
                    PropertyScope::Module => (
                        child(&child(&format!("{base}/modules"), &owner), &field.id),
                        data.modules
                            .get(&owner)
                            .and_then(|values| values.get(&field.id)),
                    ),
                    PropertyScope::Object => {
                        let id = object.expect("Core returned an object-scoped property");
                        (
                            child(&format!("{base}/objects/{id}"), &field.id),
                            data.objects
                                .get(&id)
                                .and_then(|values| values.get(&field.id)),
                        )
                    }
                    PropertyScope::Component { component } => {
                        let id = object.expect("Core returned a component-scoped property");
                        (
                            child(
                                &child(&format!("{base}/components/{id}"), component),
                                &field.id,
                            ),
                            data.components
                                .get(&id)
                                .and_then(|components| components.get(component))
                                .and_then(|values| values.get(&field.id)),
                        )
                    }
                };
                // Core already applied root module/per-object requirements;
                // CompositionActivity's game ComponentSchema set is a separate
                // contract and must not suppress legacy property attachments.
                self.known.insert(path.clone());
                if matches!(field.value_type, PropertyType::AssetReference { .. }) {
                    self.references.insert(path.clone());
                    if authored.is_none()
                        && let Some(reference) = field.default.as_str()
                    {
                        self.default_references.insert(
                            format!("property-default:{owner}/{} at {path}", field.id),
                            reference.into(),
                        );
                    }
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
        Ok(())
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
        match document.module_versions.get(owner) {
            Some(saved) => saved == installed,
            None => document.engine_lock.is_none(),
        }
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
