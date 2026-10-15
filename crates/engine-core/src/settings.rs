//! Module-owned typed data and the boundary between engine authoring and a game
//! project's permitted controls. These checks apply to UI and compiled scripts.

use crate::{
    ControlCommand, Graph, GraphDocument, ModuleDescriptor, ModuleRegistry, RenderCommand,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The storage and applicability of a contributed field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PropertyScope {
    Global,
    Module,
    Object,
    Component { component: String },
}

/// Typed editor values. Numeric bounds are inclusive and must be finite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PropertyType {
    Number {
        min: f64,
        max: f64,
    },
    Boolean,
    Enum {
        choices: Vec<String>,
    },
    Text {
        max_len: usize,
    },
    /// Scene-local stable object ID. Referent checks use the resolved scene.
    ObjectReference {
        required_components: Vec<String>,
        optional: bool,
    },
    /// Portable path or asset:// ID; kinds are allowed filename extensions.
    AssetReference {
        kinds: Vec<String>,
        optional: bool,
    },
}

impl PropertyType {
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        match self {
            Self::Number { min, max } => {
                if !min.is_finite() || !max.is_finite() || min > max {
                    return Err("Numeric schema has invalid bounds".into());
                }
                if !value
                    .as_f64()
                    .is_some_and(|v| v.is_finite() && v >= *min && v <= *max)
                {
                    return Err(format!("Expected finite number in {min}..={max}"));
                }
            }
            Self::Boolean if !value.is_boolean() => return Err("Expected boolean".into()),
            Self::Enum { choices } => {
                if choices.is_empty()
                    || choices.iter().any(|c| c.trim().is_empty())
                    || choices.iter().collect::<BTreeSet<_>>().len() != choices.len()
                {
                    return Err("Enum schema needs unique nonempty choices".into());
                }
                if !value
                    .as_str()
                    .is_some_and(|v| choices.iter().any(|c| c == v))
                {
                    return Err(format!("Expected one of {choices:?}"));
                }
            }
            Self::Text { max_len } if !value.as_str().is_some_and(|s| s.len() <= *max_len) => {
                return Err(format!("Expected text of at most {max_len} bytes"));
            }
            Self::ObjectReference {
                required_components,
                optional,
            } => {
                if required_components.iter().any(|id| id.trim().is_empty())
                    || required_components.iter().collect::<BTreeSet<_>>().len()
                        != required_components.len()
                {
                    return Err("Object reference needs unique nonempty component IDs".into());
                }
                let valid =
                    (*optional && value.is_null()) || value.as_u64().is_some_and(|id| id != 0);
                if !valid {
                    return Err("Expected nonzero object ID or optional null".into());
                }
            }
            Self::AssetReference { kinds, optional } => {
                let normalized = kinds
                    .iter()
                    .map(|kind| kind.to_ascii_lowercase())
                    .collect::<BTreeSet<_>>();
                if normalized.len() != kinds.len()
                    || kinds.iter().any(|kind| {
                        kind.is_empty()
                            || !kind
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    })
                {
                    return Err("Asset kinds need unique filename extensions without dots".into());
                }
                if *optional && value.is_null() {
                    return Ok(());
                }
                validate_asset_reference_syntax(
                    value
                        .as_str()
                        .ok_or("Expected asset reference text or optional null")?,
                )?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Check the portable reference syntax without reading a filesystem or manifest.
pub fn validate_asset_reference_syntax(reference: &str) -> Result<(), String> {
    if let Some(id) = reference.strip_prefix("asset://") {
        if id.trim().is_empty() || id.chars().any(char::is_control) {
            return Err("Asset reference needs a nonempty stable ID".into());
        }
    } else if reference.trim().is_empty()
        || reference.starts_with(['/', '\\'])
        || reference.contains(':')
        || reference.chars().any(char::is_control)
        || reference.split(['/', '\\']).any(|part| part == "..")
    {
        return Err("Asset path must be project-relative without traversal".into());
    }
    Ok(())
}

/// A module contributes stable, namespaced field IDs and its own dependencies.
/// Component requirements are checked against the actual object's attachments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertySchema {
    pub id: String,
    pub label: String,
    pub scope: PropertyScope,
    pub value_type: PropertyType,
    pub default: Value,
    #[serde(default)]
    pub requires_modules: Vec<String>,
    #[serde(default)]
    pub requires_components: Vec<String>,
}

pub type PropertyValues = BTreeMap<String, Value>;
pub type ComponentValues = BTreeMap<String, PropertyValues>;

/// Values belonging to participating fields. Components are attached by presence
/// of their ID, including an empty field map; defaults are resolved at query time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PropertyData {
    pub global: PropertyValues,
    /// Shared configuration of an owning module, keyed by module ID.
    pub modules: BTreeMap<String, PropertyValues>,
    pub objects: BTreeMap<u64, PropertyValues>,
    pub components: BTreeMap<u64, ComponentValues>,
}

/// Inactive values are kept for restoration, but never exposed to a running game.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PropertyStore {
    pub active: PropertyData,
    pub inactive: PropertyData,
    /// Explicitly detached components are not automatically reattached by a
    /// module toggle. Their archived field bytes remain available to attach.
    pub detached_components: BTreeMap<u64, BTreeSet<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineConfiguration {
    /// False retains the pre-settings execution policy for schema 1/2 projects.
    pub initialized: bool,
    /// Included feature/schema modules in addition to the connected engine graph.
    pub enabled_modules: BTreeSet<String>,
    pub exposure: EngineExposure,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineExposure {
    pub parameters: BTreeMap<String, ExposedParameter>,
    pub variants: BTreeMap<String, VariantGroup>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderParameterTarget {
    pub pass: String,
    pub name: String,
}

/// The current renderer parameter boundary supports numeric scalar parameters.
/// Enum/bool pipeline choices use ready variants rather than untyped coercion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposedParameter {
    pub label: String,
    pub value_type: PropertyType,
    pub default: Value,
    pub target: RenderParameterTarget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadyVariant {
    pub label: String,
    pub commands: Vec<RenderCommand>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariantGroup {
    pub label: String,
    pub default: String,
    pub options: BTreeMap<String, ReadyVariant>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfiguration {
    pub parameters: PropertyValues,
    pub variants: BTreeMap<String, String>,
    /// For each group the first matching rule wins; otherwise its selection is used.
    pub rules: Vec<VariantRule>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Comparison {
    Equals,
    NotEquals,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum RuleCondition {
    Parameter {
        parameter: String,
        op: Comparison,
        value: Value,
    },
    Time {
        op: Comparison,
        value: f64,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariantRule {
    pub group: String,
    pub variant: String,
    pub condition: RuleCondition,
}

/// Settings removed when an Engine edit deletes renderer passes or resources.
/// Counts describe individual fields/definitions, not separate Undo operations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderSettingsCleanupReport {
    pub exposed_parameters_removed: usize,
    pub variants_removed: usize,
    pub variant_groups_removed: usize,
    pub defaults_changed: usize,
    pub project_parameters_removed: usize,
    pub project_selections_removed: usize,
    pub project_rules_removed: usize,
}

impl RenderSettingsCleanupReport {
    pub fn total_changes(&self) -> usize {
        self.exposed_parameters_removed
            + self.variants_removed
            + self.variant_groups_removed
            + self.defaults_changed
            + self.project_parameters_removed
            + self.project_selections_removed
            + self.project_rules_removed
    }

    pub fn changed(&self) -> bool {
        self.total_changes() != 0
    }
}

/// Remove settings that depend on explicitly deleted renderer IDs.
///
/// Resource IDs are exact names such as `aa.color`; callers supply the deleted
/// outputs rather than relying on a pass-name prefix. A ready variant is an
/// indivisible preset: one deleted command target removes the entire variant.
/// Surviving groups keep their default unless that variant was removed, then
/// use the first remaining ID in lexical order. Removed Project selections fall
/// back to that group default. Other definitions, values and rule order remain
/// unchanged, including unrelated invalid settings that need their own repair.
/// This mutates document data only, so callers can include it in the same Undo
/// transaction as the graph edit. Custom source text is never rewritten here.
pub fn cleanup_removed_render_settings(
    document: &mut GraphDocument,
    removed_passes: &BTreeSet<String>,
    removed_resources: &BTreeSet<String>,
) -> RenderSettingsCleanupReport {
    let mut report = RenderSettingsCleanupReport::default();
    let mut removed_parameters = BTreeSet::new();
    document.engine.exposure.parameters.retain(|id, parameter| {
        if removed_passes.contains(&parameter.target.pass) {
            removed_parameters.insert(id.clone());
            report.exposed_parameters_removed += 1;
            false
        } else {
            true
        }
    });

    let targets_removed = |command: &RenderCommand| match command {
        RenderCommand::SetParameter { pass, .. } | RenderCommand::SetEnabled { pass, .. } => {
            removed_passes.contains(pass)
        }
        RenderCommand::ClearResource { name, .. } => removed_resources.contains(name),
        RenderCommand::CopyResource { from, to } => {
            removed_resources.contains(from) || removed_resources.contains(to)
        }
    };
    let mut removed_variants = BTreeMap::<String, BTreeSet<String>>::new();
    let mut removed_groups = BTreeSet::new();
    for (id, group) in &mut document.engine.exposure.variants {
        let mut removed = BTreeSet::new();
        group.options.retain(|option, variant| {
            if variant.commands.iter().any(targets_removed) {
                removed.insert(option.clone());
                report.variants_removed += 1;
                false
            } else {
                true
            }
        });
        if removed.is_empty() {
            continue;
        }
        if group.options.is_empty() {
            removed_groups.insert(id.clone());
            report.variant_groups_removed += 1;
        } else if removed.contains(&group.default) {
            group.default = group.options.first_key_value().unwrap().0.clone();
            report.defaults_changed += 1;
        }
        removed_variants.insert(id.clone(), removed);
    }
    document
        .engine
        .exposure
        .variants
        .retain(|id, _| !removed_groups.contains(id));

    document.project.parameters.retain(|id, _| {
        if removed_parameters.contains(id) {
            report.project_parameters_removed += 1;
            false
        } else {
            true
        }
    });
    let variant_removed = |group: &str, variant: &str| {
        removed_groups.contains(group)
            || removed_variants
                .get(group)
                .is_some_and(|options| options.contains(variant))
    };
    document.project.variants.retain(|group, variant| {
        if variant_removed(group, variant) {
            report.project_selections_removed += 1;
            false
        } else {
            true
        }
    });
    document.project.rules.retain(|rule| {
        let parameter_removed = matches!(
            &rule.condition,
            RuleCondition::Parameter { parameter, .. } if removed_parameters.contains(parameter)
        );
        if parameter_removed || variant_removed(&rule.group, &rule.variant) {
            report.project_rules_removed += 1;
            false
        } else {
            true
        }
    });
    report
}

/// Connected modules plus explicitly included features; an installed palette
/// descriptor alone never activates a contribution.
pub fn effective_modules(document: &GraphDocument) -> BTreeSet<String> {
    let mut modules = connected_modules(document);
    modules.extend(document.engine.enabled_modules.iter().cloned());
    modules
}

/// Modules participating in the connected root graph, excluding feature toggles.
pub fn connected_modules(document: &GraphDocument) -> BTreeSet<String> {
    fn visit(graph: &Graph, roots: Vec<String>, modules: &mut BTreeSet<String>) {
        let mut seen = BTreeSet::new();
        let mut pending = roots;
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(node) = graph.nodes.iter().find(|n| n.id == id) {
                modules.insert(node.module.clone());
                if let Some(inner) = &node.subgraph {
                    visit(
                        inner,
                        inner
                            .nodes
                            .iter()
                            .filter(|n| n.module == "interface.output")
                            .map(|n| n.id.clone())
                            .collect(),
                        modules,
                    );
                }
                pending.extend(
                    graph
                        .edges
                        .iter()
                        .filter(|e| e.to_node == id)
                        .map(|e| e.from_node.clone()),
                );
            }
        }
    }
    let mut modules = BTreeSet::new();
    visit(
        &document.root,
        document
            .root
            .nodes
            .iter()
            .filter(|n| n.module == "viewport.output")
            .map(|n| n.id.clone())
            .collect(),
        &mut modules,
    );
    modules
}

fn contributions(registry: &ModuleRegistry) -> Vec<(&str, &PropertySchema)> {
    registry
        .descriptors()
        .iter()
        .flat_map(|m| m.properties.iter().map(move |s| (m.id.as_str(), s)))
        .collect()
}

fn available(
    document: &GraphDocument,
    owner: &str,
    schema: &PropertySchema,
    object: Option<u64>,
) -> bool {
    let modules = effective_modules(document);
    if !modules.contains(owner) || schema.requires_modules.iter().any(|m| !modules.contains(m)) {
        return false;
    }
    let attached = object.and_then(|id| document.properties.active.components.get(&id));
    if schema
        .requires_components
        .iter()
        .any(|c| !attached.is_some_and(|a| a.contains_key(c)))
    {
        return false;
    }
    match &schema.scope {
        PropertyScope::Component { component } => {
            attached.is_some_and(|a| a.contains_key(component))
        }
        PropertyScope::Object => object.is_some(),
        _ => true,
    }
}

/// Schemas available for this scope and actual component participant.
pub fn applicable_properties(
    document: &GraphDocument,
    registry: &ModuleRegistry,
    scope: &PropertyScope,
    object_id: Option<u64>,
) -> Vec<PropertySchema> {
    contributions(registry)
        .into_iter()
        .filter(|(owner, s)| &s.scope == scope && available(document, owner, s, object_id))
        .map(|(_, s)| s.clone())
        .collect()
}

/// Active owner/schema pairs for a generic settings or object inspector.
pub fn visible_property_schemas(
    document: &GraphDocument,
    registry: &ModuleRegistry,
    object_id: Option<u64>,
) -> Vec<(String, PropertySchema)> {
    contributions(registry)
        .into_iter()
        .filter(|(owner, s)| {
            available(document, owner, s, object_id)
                && match s.scope {
                    PropertyScope::Global | PropertyScope::Module => object_id.is_none(),
                    _ => object_id.is_some(),
                }
        })
        .map(|(owner, s)| (owner.into(), s.clone()))
        .collect()
}

pub fn validate_property_schema(schema: &PropertySchema) -> Result<(), String> {
    if schema.id.trim().is_empty() || schema.label.trim().is_empty() {
        return Err("Property schema needs nonempty ID and label".into());
    }
    if matches!(&schema.scope,PropertyScope::Component{component} if component.trim().is_empty()) {
        return Err("Component schema needs a component ID".into());
    }
    if schema
        .requires_modules
        .iter()
        .chain(&schema.requires_components)
        .any(|s| s.trim().is_empty())
    {
        return Err("Property dependencies need nonempty IDs".into());
    }
    schema
        .value_type
        .validate(&schema.default)
        .map_err(|e| format!("Property {} default: {e}", schema.id))
}

fn find_schema<'a>(
    registry: &'a ModuleRegistry,
    scope: &PropertyScope,
    id: &str,
) -> Option<(&'a str, &'a PropertySchema)> {
    contributions(registry)
        .into_iter()
        .find(|(_, s)| &s.scope == scope && s.id == id)
}

fn validate_values(
    document: &GraphDocument,
    registry: &ModuleRegistry,
    scope: &PropertyScope,
    object: Option<u64>,
    owner: Option<&str>,
    values: &PropertyValues,
) -> Result<(), String> {
    for (id, value) in values {
        let (module, schema) = find_schema(registry, scope, id)
            .ok_or_else(|| format!("Unknown property {id} in {scope:?}"))?;
        if owner.is_some_and(|o| o != module) {
            return Err(format!("Property {id} belongs to {module}"));
        }
        if !available(document, module, schema, object) {
            return Err(format!(
                "Property {id} is inactive or its object does not participate; archive it explicitly"
            ));
        }
        schema
            .value_type
            .validate(value)
            .map_err(|e| format!("Property {id}: {e}"))?;
    }
    Ok(())
}

/// Central validation for active fields, exposed values and ready variant rules.
/// Archived data is preserved without reinterpreting an old module's schema.
pub fn validate_settings(
    document: &GraphDocument,
    registry: &ModuleRegistry,
) -> Result<(), String> {
    for module in &document.engine.enabled_modules {
        if registry.descriptor(module).is_none() {
            return Err(format!("Included module {module} is not installed"));
        }
    }
    let schemas = contributions(registry);
    let mut ids = BTreeSet::new();
    for (_, schema) in &schemas {
        validate_property_schema(schema)?;
        if !ids.insert((format!("{:?}", schema.scope), schema.id.clone())) {
            return Err(format!("Duplicate contributed property {}", schema.id));
        }
    }
    validate_exposure(&document.engine.exposure)?;
    validate_project_configuration(&document.engine.exposure, &document.project)?;
    let data = &document.properties.active;
    validate_values(
        document,
        registry,
        &PropertyScope::Global,
        None,
        None,
        &data.global,
    )?;
    for (module, values) in &data.modules {
        if !effective_modules(document).contains(module) || registry.descriptor(module).is_none() {
            return Err(format!("Property module {module} is unknown or inactive"));
        }
        validate_values(
            document,
            registry,
            &PropertyScope::Module,
            None,
            Some(module),
            values,
        )?;
    }
    for (id, values) in &data.objects {
        validate_values(
            document,
            registry,
            &PropertyScope::Object,
            Some(*id),
            None,
            values,
        )?;
    }
    for (id, components) in &data.components {
        for (component, values) in components {
            let scope = PropertyScope::Component {
                component: component.clone(),
            };
            if !schemas
                .iter()
                .any(|(owner, s)| s.scope == scope && available(document, owner, s, Some(*id)))
            {
                return Err(format!(
                    "Component {component} is unknown or inactive for object {id}"
                ));
            }
            validate_values(document, registry, &scope, Some(*id), None, values)?;
        }
    }
    Ok(())
}

fn validate_exposure(exposure: &EngineExposure) -> Result<(), String> {
    let mut targets = BTreeSet::new();
    for (id, p) in &exposure.parameters {
        if id.trim().is_empty()
            || p.target.pass.trim().is_empty()
            || p.target.name.trim().is_empty()
        {
            return Err("Exposed parameter needs IDs and target".into());
        }
        if !matches!(p.value_type, PropertyType::Number { .. }) {
            return Err(format!(
                "Exposed parameter {id} must target a numeric renderer scalar"
            ));
        }
        p.value_type
            .validate(&p.default)
            .map_err(|e| format!("Exposed parameter {id}: {e}"))?;
        if !targets.insert((&p.target.pass, &p.target.name)) {
            return Err("Multiple exposed parameters target the same renderer parameter".into());
        }
    }
    for (id, group) in &exposure.variants {
        if id.trim().is_empty() || !group.options.contains_key(&group.default) {
            return Err(format!(
                "Variant group {id} has missing default {}",
                group.default
            ));
        }
        for (option, variant) in &group.options {
            if option.trim().is_empty() || variant.commands.is_empty() {
                return Err(format!("Ready variant {id}/{option} needs ID and commands"));
            }
            crate::RenderOverrides::default().apply_commands(&variant.commands)?;
        }
    }
    Ok(())
}

fn validate_project_configuration(
    exposure: &EngineExposure,
    project: &ProjectConfiguration,
) -> Result<(), String> {
    for (id, value) in &project.parameters {
        exposure
            .parameters
            .get(id)
            .ok_or_else(|| format!("Parameter {id} is not exposed"))?
            .value_type
            .validate(value)
            .map_err(|e| format!("Project parameter {id}: {e}"))?;
    }
    for (group, variant) in &project.variants {
        validate_variant(exposure, group, variant)?;
    }
    for rule in &project.rules {
        validate_variant(exposure, &rule.group, &rule.variant)?;
        match &rule.condition {
            RuleCondition::Parameter {
                parameter,
                op: _,
                value,
            } => {
                exposure
                    .parameters
                    .get(parameter)
                    .ok_or_else(|| format!("Rule parameter {parameter} is not exposed"))?
                    .value_type
                    .validate(value)?;
            }
            RuleCondition::Time { value, .. } if !value.is_finite() || *value < 0.0 => {
                return Err("Rule time must be finite and nonnegative".into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_variant(exposure: &EngineExposure, group: &str, variant: &str) -> Result<(), String> {
    if !exposure
        .variants
        .get(group)
        .is_some_and(|g| g.options.contains_key(variant))
    {
        return Err(format!(
            "Variant {group}/{variant} is not available; no substitution was made"
        ));
    }
    Ok(())
}

pub fn set_project_parameter(
    document: &mut GraphDocument,
    id: &str,
    value: Value,
) -> Result<(), String> {
    document
        .engine
        .exposure
        .parameters
        .get(id)
        .ok_or_else(|| format!("Parameter {id} is not exposed"))?
        .value_type
        .validate(&value)?;
    document.project.parameters.insert(id.into(), value);
    Ok(())
}

pub fn set_project_variant(
    document: &mut GraphDocument,
    group: &str,
    variant: &str,
) -> Result<(), String> {
    validate_variant(&document.engine.exposure, group, variant)?;
    document
        .project
        .variants
        .insert(group.into(), variant.into());
    Ok(())
}

fn compare(left: &Value, op: Comparison, right: &Value) -> bool {
    if let (Some(a), Some(b)) = (left.as_f64(), right.as_f64()) {
        return match op {
            Comparison::Equals => a == b,
            Comparison::NotEquals => a != b,
            Comparison::Less => a < b,
            Comparison::LessOrEqual => a <= b,
            Comparison::Greater => a > b,
            Comparison::GreaterOrEqual => a >= b,
        };
    }
    match op {
        Comparison::Equals => left == right,
        Comparison::NotEquals => left != right,
        _ => false,
    }
}

pub fn resolved_parameters(document: &GraphDocument) -> PropertyValues {
    document
        .engine
        .exposure
        .parameters
        .iter()
        .map(|(id, p)| {
            (
                id.clone(),
                document
                    .project
                    .parameters
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| p.default.clone()),
            )
        })
        .collect()
}

/// Resolve authored settings to portable commands without changing the graph.
pub fn project_commands(
    document: &GraphDocument,
    time: f64,
) -> Result<Vec<ControlCommand>, String> {
    if !time.is_finite() || time < 0.0 {
        return Err("Project rule time must be finite and nonnegative".into());
    }
    validate_exposure(&document.engine.exposure)?;
    validate_project_configuration(&document.engine.exposure, &document.project)?;
    let values = resolved_parameters(document);
    let mut commands = vec![];
    for (id, p) in &document.engine.exposure.parameters {
        commands.push(ControlCommand::Render(RenderCommand::SetParameter {
            pass: p.target.pass.clone(),
            name: p.target.name.clone(),
            value: values[id].clone(),
        }));
    }
    for (id, group) in &document.engine.exposure.variants {
        let choice = document
            .project
            .rules
            .iter()
            .find(|r| {
                r.group == *id
                    && match &r.condition {
                        RuleCondition::Parameter {
                            parameter,
                            op,
                            value,
                        } => compare(&values[parameter], *op, value),
                        RuleCondition::Time { op, value } => {
                            compare(&json!(time), *op, &json!(value))
                        }
                    }
            })
            .map(|r| r.variant.as_str())
            .or_else(|| document.project.variants.get(id).map(String::as_str))
            .unwrap_or(&group.default);
        commands.extend(
            group.options[choice]
                .commands
                .iter()
                .cloned()
                .map(ControlCommand::Render),
        );
    }
    Ok(commands)
}

fn same_command(a: &RenderCommand, b: &RenderCommand) -> bool {
    match (a, b) {
        (
            RenderCommand::SetParameter {
                pass: a,
                name: an,
                value: av,
            },
            RenderCommand::SetParameter {
                pass: b,
                name: bn,
                value: bv,
            },
        ) => a == b && an == bn && compare(av, Comparison::Equals, bv),
        _ => a == b,
    }
}

/// Project scripts can mutate the world, but renderer controls must satisfy the
/// same range/ready-variant policy as the project inspector.
pub fn validate_project_controls(
    document: &GraphDocument,
    commands: &[ControlCommand],
) -> Result<(), String> {
    if !document.engine.initialized {
        return Ok(());
    }
    for command in commands {
        let ControlCommand::Render(command) = command else {
            continue;
        };
        let exposed = if let RenderCommand::SetParameter { pass, name, value } = command {
            if let Some(p) = document
                .engine
                .exposure
                .parameters
                .values()
                .find(|p| p.target.pass == *pass && p.target.name == *name)
            {
                p.value_type.validate(value)?;
                true
            } else {
                false
            }
        } else {
            false
        };
        if !exposed
            && !document
                .engine
                .exposure
                .variants
                .values()
                .flat_map(|g| g.options.values())
                .flat_map(|v| &v.commands)
                .any(|allowed| same_command(command, allowed))
        {
            return Err(format!(
                "Project renderer command is not exposed: {command:?}"
            ));
        }
    }
    Ok(())
}

/// Ready presets for the existing real tone mapper and spatial AA passes.
/// New stock controls follow the shader ABI: mode 1 is ACES, mode 0 Reinhard.
/// Serialized authored variants remain authoritative and are never rewritten.
pub fn default_renderer_exposure(tonemap_pass: &str, aa_pass: &str) -> EngineExposure {
    EngineExposure {
        parameters: BTreeMap::from([(
            "exposure".into(),
            ExposedParameter {
                label: "Exposure".into(),
                value_type: PropertyType::Number {
                    min: 0.01,
                    max: 8.0,
                },
                default: json!(1.0),
                target: RenderParameterTarget {
                    pass: tonemap_pass.into(),
                    name: "exposure".into(),
                },
            },
        )]),
        variants: BTreeMap::from([
            (
                "tonemap".into(),
                VariantGroup {
                    label: "Tone mapping".into(),
                    default: "aces".into(),
                    options: BTreeMap::from([
                        (
                            "aces".into(),
                            ReadyVariant {
                                label: "ACES".into(),
                                commands: vec![RenderCommand::SetParameter {
                                    pass: tonemap_pass.into(),
                                    name: "mode".into(),
                                    value: json!(1.0),
                                }],
                            },
                        ),
                        (
                            "reinhard".into(),
                            ReadyVariant {
                                label: "Reinhard".into(),
                                commands: vec![RenderCommand::SetParameter {
                                    pass: tonemap_pass.into(),
                                    name: "mode".into(),
                                    value: json!(0.0),
                                }],
                            },
                        ),
                    ]),
                },
            ),
            (
                "antialiasing".into(),
                VariantGroup {
                    label: "Antialiasing".into(),
                    default: "fxaa".into(),
                    options: BTreeMap::from([
                        (
                            "off".into(),
                            ReadyVariant {
                                label: "Off".into(),
                                commands: vec![RenderCommand::SetEnabled {
                                    pass: aa_pass.into(),
                                    enabled: false,
                                }],
                            },
                        ),
                        (
                            "fxaa".into(),
                            ReadyVariant {
                                label: "FXAA".into(),
                                commands: vec![RenderCommand::SetEnabled {
                                    pass: aa_pass.into(),
                                    enabled: true,
                                }],
                            },
                        ),
                    ]),
                },
            ),
        ]),
    }
}

/// A real gameplay-data contribution; its field is read by ordinary Rust Custom.
/// It deliberately provides no fictitious networking execution backend.
pub fn motion_property_module() -> ModuleDescriptor {
    ModuleDescriptor {
        id: "game.motion".into(),
        label: "Gameplay motion".into(),
        description: "Typed per-participant motion data available to Rust Custom".into(),
        inputs: vec![],
        outputs: vec![],
        properties: vec![PropertySchema {
            id: "game.motion.speed".into(),
            label: "Motion speed".into(),
            scope: PropertyScope::Component {
                component: "game.motion".into(),
            },
            value_type: PropertyType::Number { min: 0.0, max: 8.0 },
            default: json!(1.0),
            requires_modules: vec![],
            requires_components: vec![],
        }],
    }
}

fn merge_values(target: &mut PropertyValues, values: PropertyValues) -> Result<(), String> {
    for (id, value) in values {
        if target.get(&id).is_some_and(|old| old != &value) {
            return Err(format!(
                "Conflicting archived property {id}; neither value was discarded"
            ));
        }
        target.insert(id, value);
    }
    Ok(())
}

fn module_allows(document: &GraphDocument, owner: &str, schema: &PropertySchema) -> bool {
    let modules = effective_modules(document);
    modules.contains(owner) && schema.requires_modules.iter().all(|m| modules.contains(m))
}

fn component_known_available(
    document: &GraphDocument,
    registry: &ModuleRegistry,
    component: &str,
) -> bool {
    contributions(registry).iter().any(|(owner, s)| {
        matches!(&s.scope,PropertyScope::Component{component:c} if c==component)
            && module_allows(document, owner, s)
    })
}

fn reconcile_component_presence(
    document: &mut GraphDocument,
    registry: &ModuleRegistry,
) -> Result<(), String> {
    loop {
        let keys = document
            .properties
            .active
            .components
            .iter()
            .flat_map(|(object, c)| c.keys().map(move |id| (*object, id.clone())))
            .collect::<Vec<_>>();
        let mut changed = false;
        for (object, component) in keys {
            let available = contributions(registry).iter().any(|(owner, s)| {
                matches!(&s.scope,PropertyScope::Component{component:c} if c==&component)
                    && available(document, owner, s, Some(object))
            });
            if !available {
                let values = document
                    .properties
                    .active
                    .components
                    .get_mut(&object)
                    .unwrap()
                    .remove(&component)
                    .unwrap();
                merge_values(
                    document
                        .properties
                        .inactive
                        .components
                        .entry(object)
                        .or_default()
                        .entry(component)
                        .or_default(),
                    values,
                )?;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    loop {
        let keys = document
            .properties
            .inactive
            .components
            .iter()
            .flat_map(|(object, c)| c.keys().map(move |id| (*object, id.clone())))
            .collect::<Vec<_>>();
        let mut changed = false;
        for (object, component) in keys {
            if document
                .properties
                .detached_components
                .get(&object)
                .is_some_and(|c| c.contains(&component))
                || document
                    .properties
                    .active
                    .components
                    .get(&object)
                    .is_some_and(|c| c.contains_key(&component))
            {
                continue;
            }
            let attached = document.properties.active.components.get(&object);
            let restorable = contributions(registry).iter().any(|(owner, s)| {
                matches!(&s.scope,PropertyScope::Component{component:c} if c==&component)
                    && module_allows(document, owner, s)
                    && s.requires_components
                        .iter()
                        .all(|id| id == &component || attached.is_some_and(|c| c.contains_key(id)))
            });
            if restorable {
                let values = document
                    .properties
                    .inactive
                    .components
                    .get_mut(&object)
                    .unwrap()
                    .remove(&component)
                    .unwrap();
                document
                    .properties
                    .active
                    .components
                    .entry(object)
                    .or_default()
                    .insert(component, values);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok(())
}

/// Archive unavailable module/component fields and restore known fields when their
/// dependencies return. It is transactional and never removes unknown archive data.
pub fn reconcile_properties(
    document: &mut GraphDocument,
    registry: &ModuleRegistry,
) -> Result<(), String> {
    let mut candidate = document.clone();
    reconcile_component_presence(&mut candidate, registry)?;
    let active = candidate.properties.active.clone();
    for id in active.global.keys() {
        if !find_schema(registry, &PropertyScope::Global, id)
            .is_some_and(|(owner, s)| available(&candidate, owner, s, None))
        {
            let value = candidate.properties.active.global.remove(id).unwrap();
            merge_values(
                &mut candidate.properties.inactive.global,
                BTreeMap::from([(id.clone(), value)]),
            )?;
        }
    }
    for (module, values) in &active.modules {
        for id in values.keys() {
            if !find_schema(registry, &PropertyScope::Module, id)
                .is_some_and(|(owner, s)| owner == module && available(&candidate, owner, s, None))
            {
                let value = candidate
                    .properties
                    .active
                    .modules
                    .get_mut(module)
                    .unwrap()
                    .remove(id)
                    .unwrap();
                merge_values(
                    candidate
                        .properties
                        .inactive
                        .modules
                        .entry(module.clone())
                        .or_default(),
                    BTreeMap::from([(id.clone(), value)]),
                )?;
            }
        }
    }
    for (object, values) in &active.objects {
        for id in values.keys() {
            if !find_schema(registry, &PropertyScope::Object, id)
                .is_some_and(|(owner, s)| available(&candidate, owner, s, Some(*object)))
            {
                let value = candidate
                    .properties
                    .active
                    .objects
                    .get_mut(object)
                    .unwrap()
                    .remove(id)
                    .unwrap();
                merge_values(
                    candidate
                        .properties
                        .inactive
                        .objects
                        .entry(*object)
                        .or_default(),
                    BTreeMap::from([(id.clone(), value)]),
                )?;
            }
        }
    }
    for (object, components) in &active.components {
        for (component, values) in components {
            for id in values.keys() {
                let scope = PropertyScope::Component {
                    component: component.clone(),
                };
                if !find_schema(registry, &scope, id)
                    .is_some_and(|(owner, s)| available(&candidate, owner, s, Some(*object)))
                {
                    let value = candidate
                        .properties
                        .active
                        .components
                        .get_mut(object)
                        .unwrap()
                        .get_mut(component)
                        .unwrap()
                        .remove(id)
                        .unwrap();
                    merge_values(
                        candidate
                            .properties
                            .inactive
                            .components
                            .entry(*object)
                            .or_default()
                            .entry(component.clone())
                            .or_default(),
                        BTreeMap::from([(id.clone(), value)]),
                    )?;
                }
            }
        }
    }
    let inactive = candidate.properties.inactive.clone();
    for (id, value) in inactive.global {
        if find_schema(registry, &PropertyScope::Global, &id)
            .is_some_and(|(owner, s)| available(&candidate, owner, s, None))
        {
            merge_values(
                &mut candidate.properties.active.global,
                BTreeMap::from([(id.clone(), value)]),
            )?;
            candidate.properties.inactive.global.remove(&id);
        }
    }
    for (module, values) in inactive.modules {
        for (id, value) in values {
            if find_schema(registry, &PropertyScope::Module, &id)
                .is_some_and(|(owner, s)| owner == module && available(&candidate, owner, s, None))
            {
                merge_values(
                    candidate
                        .properties
                        .active
                        .modules
                        .entry(module.clone())
                        .or_default(),
                    BTreeMap::from([(id.clone(), value)]),
                )?;
                candidate
                    .properties
                    .inactive
                    .modules
                    .get_mut(&module)
                    .unwrap()
                    .remove(&id);
            }
        }
    }
    for (object, values) in inactive.objects {
        for (id, value) in values {
            if find_schema(registry, &PropertyScope::Object, &id)
                .is_some_and(|(owner, s)| available(&candidate, owner, s, Some(object)))
            {
                merge_values(
                    candidate
                        .properties
                        .active
                        .objects
                        .entry(object)
                        .or_default(),
                    BTreeMap::from([(id.clone(), value)]),
                )?;
                candidate
                    .properties
                    .inactive
                    .objects
                    .get_mut(&object)
                    .unwrap()
                    .remove(&id);
            }
        }
    }
    for (object, components) in inactive.components {
        for (component, values) in components {
            for (id, value) in values {
                let scope = PropertyScope::Component {
                    component: component.clone(),
                };
                if find_schema(registry, &scope, &id)
                    .is_some_and(|(owner, s)| available(&candidate, owner, s, Some(object)))
                {
                    merge_values(
                        candidate
                            .properties
                            .active
                            .components
                            .entry(object)
                            .or_default()
                            .entry(component.clone())
                            .or_default(),
                        BTreeMap::from([(id.clone(), value)]),
                    )?;
                    candidate
                        .properties
                        .inactive
                        .components
                        .get_mut(&object)
                        .unwrap()
                        .get_mut(&component)
                        .unwrap()
                        .remove(&id);
                }
            }
        }
    }
    candidate
        .properties
        .active
        .modules
        .retain(|_, v| !v.is_empty());
    candidate
        .properties
        .active
        .objects
        .retain(|_, v| !v.is_empty());
    candidate
        .properties
        .active
        .components
        .retain(|_, v| !v.is_empty());
    candidate
        .properties
        .inactive
        .modules
        .retain(|_, v| !v.is_empty());
    candidate
        .properties
        .inactive
        .objects
        .retain(|_, v| !v.is_empty());
    candidate
        .properties
        .inactive
        .components
        .retain(|_, v| !v.is_empty());
    validate_settings(&candidate, registry)?;
    *document = candidate;
    Ok(())
}

/// Attach a participating component, optionally restoring its archived values.
pub fn attach_component(
    document: &mut GraphDocument,
    registry: &ModuleRegistry,
    object: u64,
    component: &str,
) -> Result<(), String> {
    if !component_known_available(document, registry, component) {
        return Err(format!(
            "Component {component} is not contributed by an active module"
        ));
    }
    let mut candidate = document.clone();
    if !candidate
        .properties
        .active
        .components
        .get(&object)
        .is_some_and(|v| v.contains_key(component))
    {
        let values = candidate
            .properties
            .inactive
            .components
            .get_mut(&object)
            .and_then(|v| v.remove(component))
            .unwrap_or_default();
        candidate
            .properties
            .active
            .components
            .entry(object)
            .or_default()
            .insert(component.into(), values);
    }
    if let Some(detached) = candidate.properties.detached_components.get_mut(&object) {
        detached.remove(component);
    }
    reconcile_properties(&mut candidate, registry)?;
    *document = candidate;
    Ok(())
}

/// Detach without deleting authoring data. Dependent object fields are archived.
pub fn detach_component(
    document: &mut GraphDocument,
    registry: &ModuleRegistry,
    object: u64,
    component: &str,
) -> Result<(), String> {
    let mut candidate = document.clone();
    candidate
        .properties
        .detached_components
        .entry(object)
        .or_default()
        .insert(component.into());
    if let Some(values) = candidate
        .properties
        .active
        .components
        .get_mut(&object)
        .and_then(|v| v.remove(component))
    {
        merge_values(
            candidate
                .properties
                .inactive
                .components
                .entry(object)
                .or_default()
                .entry(component.into())
                .or_default(),
            values,
        )?;
    }
    // Explicit detach metadata prevents automatic reattachment during reconciliation.
    let values = candidate
        .properties
        .active
        .objects
        .get(&object)
        .cloned()
        .unwrap_or_default();
    for id in values.keys() {
        if !find_schema(registry, &PropertyScope::Object, id)
            .is_some_and(|(owner, s)| available(&candidate, owner, s, Some(object)))
        {
            let value = candidate
                .properties
                .active
                .objects
                .get_mut(&object)
                .unwrap()
                .remove(id)
                .unwrap();
            merge_values(
                candidate
                    .properties
                    .inactive
                    .objects
                    .entry(object)
                    .or_default(),
                BTreeMap::from([(id.clone(), value)]),
            )?;
        }
    }
    reconcile_properties(&mut candidate, registry)?;
    *document = candidate;
    Ok(())
}

/// Return only applicable active values/defaults for actual live object IDs.
pub fn resolved_properties(
    document: &GraphDocument,
    registry: &ModuleRegistry,
    objects: &BTreeSet<u64>,
) -> PropertyData {
    let mut result = PropertyData::default();
    for (owner, schema) in contributions(registry) {
        match &schema.scope {
            PropertyScope::Global if available(document, owner, schema, None) => {
                result.global.insert(
                    schema.id.clone(),
                    document
                        .properties
                        .active
                        .global
                        .get(&schema.id)
                        .cloned()
                        .unwrap_or_else(|| schema.default.clone()),
                );
            }
            PropertyScope::Module if available(document, owner, schema, None) => {
                result.modules.entry(owner.into()).or_default().insert(
                    schema.id.clone(),
                    document
                        .properties
                        .active
                        .modules
                        .get(owner)
                        .and_then(|m| m.get(&schema.id))
                        .cloned()
                        .unwrap_or_else(|| schema.default.clone()),
                );
            }
            PropertyScope::Object | PropertyScope::Component { .. } => {
                for object in objects {
                    if !available(document, owner, schema, Some(*object)) {
                        continue;
                    }
                    match &schema.scope {
                        PropertyScope::Object => {
                            result.objects.entry(*object).or_default().insert(
                                schema.id.clone(),
                                document
                                    .properties
                                    .active
                                    .objects
                                    .get(object)
                                    .and_then(|v| v.get(&schema.id))
                                    .cloned()
                                    .unwrap_or_else(|| schema.default.clone()),
                            );
                        }
                        PropertyScope::Component { component } => {
                            result
                                .components
                                .entry(*object)
                                .or_default()
                                .entry(component.clone())
                                .or_default()
                                .insert(
                                    schema.id.clone(),
                                    document
                                        .properties
                                        .active
                                        .components
                                        .get(object)
                                        .and_then(|c| c.get(component))
                                        .and_then(|v| v.get(&schema.id))
                                        .cloned()
                                        .unwrap_or_else(|| schema.default.clone()),
                                );
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    result
}

/// Team/project mutation validation. Cosmetic layout is not architecture; source
/// bindings and engine shader/configuration remain locked to the supplied baseline.
pub fn validate_project_edit(
    candidate: &GraphDocument,
    baseline: &GraphDocument,
    registry: &ModuleRegistry,
) -> Result<(), String> {
    fn architecture(graph: &Graph) -> Graph {
        let mut result = graph.clone();
        for node in &mut result.nodes {
            node.position = [0.0, 0.0];
            node.label.clear();
            if node.module == "custom.rust.update"
                && let Some(params) = node.params.as_object_mut()
            {
                params.remove("code_layout");
                params.remove("code_layout_keys");
            }
            if let Some(inner) = &mut node.subgraph {
                **inner = architecture(inner);
            }
        }
        result
    }
    if candidate.schema_version != baseline.schema_version
        || architecture(&candidate.root) != architecture(&baseline.root)
        || candidate.engine != baseline.engine
        || candidate.shader_sources != baseline.shader_sources
        || candidate.module_versions != baseline.module_versions
        || candidate.game.as_ref().map(|g| &g.execution)
            != baseline.game.as_ref().map(|g| &g.execution)
    {
        return Err("Project edits cannot change the locked engine architecture, exposure, shader sources or module versions".into());
    }
    if candidate.custom_sources.keys().collect::<Vec<_>>()
        != baseline.custom_sources.keys().collect::<Vec<_>>()
    {
        return Err("Project edits cannot add/remove engine-bound source IDs".into());
    }
    let mut gameplay_sources = baseline
        .root
        .nodes
        .iter()
        .filter(|n| n.module == "custom.rust.update")
        .filter_map(|n| n.params.get("source_id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    if let Some(game) = &baseline.game {
        gameplay_sources.extend(
            game.execution
                .stages
                .iter()
                .flat_map(|s| &s.nodes)
                .filter(|n| n.module == "custom.rust.update")
                .filter_map(|n| n.params.get("source_id").and_then(Value::as_str)),
        );
    }
    for (id, source) in &baseline.custom_sources {
        if !gameplay_sources.contains(id.as_str())
            && candidate.custom_sources.get(id) != Some(source)
        {
            return Err(format!("Source {id} is engine-owned"));
        }
    }
    let errors = crate::validate_graph(candidate, registry)
        .into_iter()
        .filter(|d| d.severity == crate::Severity::Error)
        .map(|d| d.message)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(())
}

/// Prepare active game data for a team packet. Source/SDK packaging is host-owned.
pub fn prepare_team_document(document: &GraphDocument) -> GraphDocument {
    let mut prepared = document.clone();
    prepared.properties.inactive = PropertyData::default();
    prepared.properties.detached_components.clear();
    prepared
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Camera, Frame, FrameRenderer, RenderOverrides, Runtime, SceneSnapshot, ScriptContext,
        SoftwareRenderer, WorldKind, demo_document,
    };

    fn network_fixture() -> ModuleDescriptor {
        let field = |id: &str, scope, value_type, default, requires_components| PropertySchema {
            id: id.into(),
            label: id.into(),
            scope,
            value_type,
            default,
            requires_modules: vec![],
            requires_components,
        };
        ModuleDescriptor {
            id: "fixture.network".into(),
            label: "Network schema fixture".into(),
            description: "No multiplayer implementation".into(),
            inputs: vec![],
            outputs: vec![],
            properties: vec![
                field(
                    "fixture.tick_rate",
                    PropertyScope::Global,
                    PropertyType::Number {
                        min: 1.0,
                        max: 120.0,
                    },
                    json!(30.0),
                    vec![],
                ),
                field(
                    "fixture.replicate",
                    PropertyScope::Object,
                    PropertyType::Boolean,
                    json!(true),
                    vec!["fixture.replicated".into()],
                ),
                field(
                    "fixture.channel",
                    PropertyScope::Component {
                        component: "fixture.replicated".into(),
                    },
                    PropertyType::Enum {
                        choices: vec!["reliable".into(), "unreliable".into()],
                    },
                    json!("reliable"),
                    vec![],
                ),
            ],
        }
    }

    #[test]
    fn contributions_follow_effective_modules_and_actual_participants_without_deleting_data() {
        let mut registry = ModuleRegistry::builtin();
        registry.register_module(network_fixture()).unwrap();
        let mut document = demo_document(WorldKind::Objects);
        assert!(
            applicable_properties(&document, &registry, &PropertyScope::Global, None).is_empty()
        );
        document
            .engine
            .enabled_modules
            .insert("fixture.network".into());
        assert_eq!(
            applicable_properties(&document, &registry, &PropertyScope::Global, None).len(),
            1
        );
        assert!(
            applicable_properties(&document, &registry, &PropertyScope::Object, Some(1)).is_empty()
        );
        attach_component(&mut document, &registry, 1, "fixture.replicated").unwrap();
        assert_eq!(
            applicable_properties(&document, &registry, &PropertyScope::Object, Some(1)).len(),
            1
        );
        assert!(
            applicable_properties(&document, &registry, &PropertyScope::Object, Some(2)).is_empty()
        );
        document
            .properties
            .active
            .global
            .insert("fixture.tick_rate".into(), json!(45.0));
        document
            .properties
            .active
            .objects
            .entry(1)
            .or_default()
            .insert("fixture.replicate".into(), json!(false));
        document
            .properties
            .active
            .components
            .get_mut(&1)
            .unwrap()
            .get_mut("fixture.replicated")
            .unwrap()
            .insert("fixture.channel".into(), json!("unreliable"));
        validate_settings(&document, &registry).unwrap();
        let before = document.properties.active.clone();
        document.engine.enabled_modules.remove("fixture.network");
        reconcile_properties(&mut document, &registry).unwrap();
        assert!(document.properties.active.global.is_empty());
        assert!(document.properties.active.components.is_empty());
        assert_eq!(
            document.properties.inactive.global["fixture.tick_rate"],
            45.0
        );
        let wire = serde_json::to_string(&document).unwrap();
        document = serde_json::from_str(&wire).unwrap();
        assert_eq!(
            document.properties.inactive.components[&1]["fixture.replicated"]["fixture.channel"],
            "unreliable"
        );
        assert!(
            resolved_properties(&document, &registry, &BTreeSet::from([1, 2]))
                .global
                .is_empty()
        );
        document
            .engine
            .enabled_modules
            .insert("fixture.network".into());
        reconcile_properties(&mut document, &registry).unwrap();
        assert_eq!(document.properties.active, before);
        detach_component(&mut document, &registry, 1, "fixture.replicated").unwrap();
        reconcile_properties(&mut document, &registry).unwrap();
        assert!(
            !document
                .properties
                .active
                .components
                .get(&1)
                .is_some_and(|c| c.contains_key("fixture.replicated"))
        );
        attach_component(&mut document, &registry, 1, "fixture.replicated").unwrap();
        assert_eq!(
            document.properties.active.components[&1]["fixture.replicated"]["fixture.channel"],
            "unreliable"
        );
        assert_eq!(
            document.properties.active.objects[&1]["fixture.replicate"],
            false
        );
        document
            .properties
            .inactive
            .global
            .insert("removed.plugin.field".into(), json!({"preserved":true}));
        let team = prepare_team_document(&document);
        assert_eq!(team.properties.inactive, PropertyData::default());
        assert!(team.properties.detached_components.is_empty());
        assert_eq!(
            document.properties.inactive.global["removed.plugin.field"],
            json!({"preserved":true})
        );
    }

    #[test]
    fn dependent_components_archive_and_restore_without_reversing_an_explicit_detach() {
        let mut registry = ModuleRegistry::builtin();
        let mut network = network_fixture();
        network.properties.push(PropertySchema {
            id: "fixture.dependent.rate".into(),
            label: "Dependent rate".into(),
            scope: PropertyScope::Component {
                component: "fixture.dependent".into(),
            },
            value_type: PropertyType::Number {
                min: 0.0,
                max: 10.0,
            },
            default: json!(2.0),
            requires_modules: vec![],
            requires_components: vec!["fixture.replicated".into()],
        });
        registry.register_module(network).unwrap();
        let mut document = demo_document(WorldKind::Objects);
        document
            .engine
            .enabled_modules
            .insert("fixture.network".into());
        attach_component(&mut document, &registry, 1, "fixture.replicated").unwrap();
        attach_component(&mut document, &registry, 1, "fixture.dependent").unwrap();
        document
            .properties
            .active
            .components
            .get_mut(&1)
            .unwrap()
            .get_mut("fixture.dependent")
            .unwrap()
            .insert("fixture.dependent.rate".into(), json!(7.0));
        detach_component(&mut document, &registry, 1, "fixture.replicated").unwrap();
        assert!(
            document
                .properties
                .active
                .components
                .get(&1)
                .is_none_or(|c| c.is_empty())
        );
        assert_eq!(
            document.properties.inactive.components[&1]["fixture.dependent"]["fixture.dependent.rate"],
            7.0
        );
        attach_component(&mut document, &registry, 1, "fixture.replicated").unwrap();
        assert_eq!(
            document.properties.active.components[&1]["fixture.dependent"]["fixture.dependent.rate"],
            7.0
        );
    }

    #[test]
    fn invalid_values_unknown_fields_nonparticipants_and_removed_variants_are_diagnostics() {
        let mut registry = ModuleRegistry::builtin();
        registry.register_module(network_fixture()).unwrap();
        let mut document = demo_document(WorldKind::Objects);
        document
            .engine
            .enabled_modules
            .insert("fixture.network".into());
        document
            .properties
            .active
            .objects
            .entry(2)
            .or_default()
            .insert("fixture.replicate".into(), json!(true));
        assert!(
            validate_settings(&document, &registry)
                .unwrap_err()
                .contains("does not participate")
        );
        document.properties.active.objects.clear();
        document
            .properties
            .active
            .global
            .insert("typo".into(), json!(true));
        assert!(
            validate_settings(&document, &registry)
                .unwrap_err()
                .contains("Unknown property")
        );
        document.properties.active.global.clear();
        document
            .properties
            .active
            .global
            .insert("fixture.tick_rate".into(), json!(121.0));
        assert!(validate_settings(&document, &registry).is_err());
        document.properties.active.global.clear();
        document.engine.initialized = true;
        document.engine.exposure = default_renderer_exposure("tonemap", "aa");
        let original = document.clone();
        assert!(set_project_parameter(&mut document, "exposure", json!(9.0)).is_err());
        assert!(set_project_variant(&mut document, "antialiasing", "smaa").is_err());
        assert_eq!(original, document);
        document
            .project
            .variants
            .insert("antialiasing".into(), "missing".into());
        assert!(
            validate_settings(&document, &registry)
                .unwrap_err()
                .contains("no substitution")
        );
        assert_eq!(document.project.variants["antialiasing"], "missing");
        document.project.variants.clear();
        document.project.rules.push(VariantRule {
            group: "antialiasing".into(),
            variant: "fxaa".into(),
            condition: RuleCondition::Parameter {
                parameter: "secret".into(),
                op: Comparison::Greater,
                value: json!(1.0),
            },
        });
        assert!(validate_settings(&document, &registry).is_err());
        let mut encoded = serde_json::to_value(&original).unwrap();
        encoded["project"]["unknown"] = json!(true);
        assert!(serde_json::from_value::<GraphDocument>(encoded).is_err());
    }

    #[test]
    fn renderer_cleanup_removes_aa_group_and_dependent_project_settings() {
        let mut document = demo_document(WorldKind::Objects);
        document.engine.initialized = true;
        document.engine.exposure = default_renderer_exposure("tonemap", "aa");
        set_project_variant(&mut document, "antialiasing", "off").unwrap();
        document.project.rules.push(VariantRule {
            group: "antialiasing".into(),
            variant: "fxaa".into(),
            condition: RuleCondition::Time {
                op: Comparison::GreaterOrEqual,
                value: 1.0,
            },
        });
        let mut expected = document.clone();
        expected.engine.exposure.variants.remove("antialiasing");
        expected.project.variants.remove("antialiasing");
        expected.project.rules.clear();

        let report = cleanup_removed_render_settings(
            &mut document,
            &BTreeSet::from(["aa".into()]),
            &BTreeSet::from(["aa.color".into()]),
        );
        assert_eq!(document, expected);
        assert_eq!(
            report,
            RenderSettingsCleanupReport {
                variants_removed: 2,
                variant_groups_removed: 1,
                project_selections_removed: 1,
                project_rules_removed: 1,
                ..Default::default()
            }
        );
        assert!(report.changed());
        assert_eq!(report.total_changes(), 5);
        assert!(project_commands(&document, 0.0).is_ok());
        assert!(project_commands(&document, 2.0).is_ok());
    }

    #[test]
    fn renderer_cleanup_removes_whole_variants_and_repairs_defaults_rules_and_resources() {
        let mut document = demo_document(WorldKind::Objects);
        document.engine.initialized = true;
        let parameter = |pass: &str| ExposedParameter {
            label: pass.into(),
            value_type: PropertyType::Number { min: 0.0, max: 8.0 },
            default: json!(1.0),
            target: RenderParameterTarget {
                pass: pass.into(),
                name: "exposure".into(),
            },
        };
        let variant = |commands| ReadyVariant {
            label: "Original label".into(),
            commands,
        };
        let retained = RenderCommand::SetEnabled {
            pass: "alive".into(),
            enabled: true,
        };
        document.engine.exposure.parameters = BTreeMap::from([
            ("gone-value".into(), parameter("gone")),
            ("keep-value".into(), parameter("alive")),
        ]);
        document.engine.exposure.variants.insert(
            "quality".into(),
            VariantGroup {
                label: "Quality".into(),
                default: "bad-enabled".into(),
                options: BTreeMap::from([
                    ("b-keep".into(), variant(vec![retained.clone()])),
                    (
                        "a-keep".into(),
                        variant(vec![RenderCommand::SetParameter {
                            pass: "alive".into(),
                            name: "exposure".into(),
                            value: json!(2.0),
                        }]),
                    ),
                    (
                        "bad-parameter".into(),
                        variant(vec![
                            retained.clone(),
                            RenderCommand::SetParameter {
                                pass: "gone".into(),
                                name: "exposure".into(),
                                value: json!(3.0),
                            },
                        ]),
                    ),
                    (
                        "bad-enabled".into(),
                        variant(vec![RenderCommand::SetEnabled {
                            pass: "gone".into(),
                            enabled: false,
                        }]),
                    ),
                    (
                        "bad-clear".into(),
                        variant(vec![RenderCommand::ClearResource {
                            name: "gone.color".into(),
                            value: [0.0; 4],
                        }]),
                    ),
                    (
                        "bad-copy-from".into(),
                        variant(vec![RenderCommand::CopyResource {
                            from: "gone.color".into(),
                            to: "alive.color".into(),
                        }]),
                    ),
                    (
                        "bad-copy-to".into(),
                        variant(vec![RenderCommand::CopyResource {
                            from: "alive.color".into(),
                            to: "gone.depth".into(),
                        }]),
                    ),
                ]),
            },
        );
        document.engine.exposure.variants.insert(
            "other-quality".into(),
            VariantGroup {
                label: "Unchanged default".into(),
                default: "z-keep".into(),
                options: BTreeMap::from([
                    ("z-keep".into(), variant(vec![retained.clone()])),
                    (
                        "a-remove".into(),
                        variant(vec![RenderCommand::ClearResource {
                            name: "gone.depth".into(),
                            value: [0.0; 4],
                        }]),
                    ),
                ]),
            },
        );
        document.project.parameters = BTreeMap::from([
            ("gone-value".into(), json!(3.0)),
            ("keep-value".into(), json!(4.0)),
        ]);
        document
            .project
            .variants
            .insert("quality".into(), "bad-clear".into());
        document
            .project
            .variants
            .insert("other-quality".into(), "z-keep".into());
        let time_rule = |variant: &str, value| VariantRule {
            group: "quality".into(),
            variant: variant.into(),
            condition: RuleCondition::Time {
                op: Comparison::GreaterOrEqual,
                value,
            },
        };
        let kept_rule = time_rule("b-keep", 2.0);
        document.project.rules = vec![
            time_rule("bad-copy-to", 1.0),
            VariantRule {
                group: "quality".into(),
                variant: "a-keep".into(),
                condition: RuleCondition::Parameter {
                    parameter: "gone-value".into(),
                    op: Comparison::Greater,
                    value: json!(2.0),
                },
            },
            kept_rule.clone(),
        ];
        document
            .custom_sources
            .insert("code".into(), "gone.color".into());
        document
            .shader_sources
            .insert("shader".into(), "gone.color".into());
        document.asset_manifest = json!({"metadata": "gone.color"});
        document
            .properties
            .active
            .global
            .insert("field".into(), json!(true));
        let mut expected = document.clone();
        expected.engine.exposure.parameters.remove("gone-value");
        let expected_group = expected
            .engine
            .exposure
            .variants
            .get_mut("quality")
            .unwrap();
        expected_group
            .options
            .retain(|id, _| matches!(id.as_str(), "a-keep" | "b-keep"));
        expected_group.default = "a-keep".into();
        expected
            .engine
            .exposure
            .variants
            .get_mut("other-quality")
            .unwrap()
            .options
            .remove("a-remove");
        expected.project.parameters.remove("gone-value");
        expected.project.variants.remove("quality");
        expected.project.rules = vec![kept_rule];

        let removed_passes = BTreeSet::from(["gone".into()]);
        let removed_resources = BTreeSet::from(["gone.color".into(), "gone.depth".into()]);
        let report =
            cleanup_removed_render_settings(&mut document, &removed_passes, &removed_resources);
        assert_eq!(document, expected);
        assert_eq!(
            report,
            RenderSettingsCleanupReport {
                exposed_parameters_removed: 1,
                variants_removed: 6,
                defaults_changed: 1,
                project_parameters_removed: 1,
                project_selections_removed: 1,
                project_rules_removed: 2,
                ..Default::default()
            }
        );
        assert!(
            project_commands(&document, 0.0)
                .unwrap()
                .contains(&ControlCommand::Render(RenderCommand::SetParameter {
                    pass: "alive".into(),
                    name: "exposure".into(),
                    value: json!(2.0),
                }))
        );
        assert!(
            project_commands(&document, 3.0)
                .unwrap()
                .contains(&ControlCommand::Render(retained))
        );
        let unchanged = document.clone();
        let repeated =
            cleanup_removed_render_settings(&mut document, &removed_passes, &removed_resources);
        assert_eq!(repeated, RenderSettingsCleanupReport::default());
        assert!(!repeated.changed());
        assert_eq!(document, unchanged);
    }

    #[test]
    fn renderer_cleanup_matches_exact_ids_and_preserves_unrelated_document_fields() {
        let mut document = demo_document(WorldKind::Objects);
        document.engine.initialized = true;
        document.engine.exposure = default_renderer_exposure("tonemap", "aaa");
        document.engine.exposure.variants.insert(
            "resources".into(),
            VariantGroup {
                label: "Unrelated resources".into(),
                default: "keep".into(),
                options: BTreeMap::from([(
                    "keep".into(),
                    ReadyVariant {
                        label: "Keep exact names".into(),
                        commands: vec![
                            RenderCommand::ClearResource {
                                name: "aa.color2".into(),
                                value: [0.1; 4],
                            },
                            RenderCommand::CopyResource {
                                from: "other.aa.color".into(),
                                to: "aa.color.backup".into(),
                            },
                        ],
                    },
                )]),
            },
        );
        // This helper is not a general validator or repair operation.
        document
            .project
            .parameters
            .insert("unknown".into(), json!(2.0));
        document
            .project
            .variants
            .insert("other".into(), "unknown".into());
        document.name = "Unrelated metadata".into();
        document
            .custom_sources
            .insert("code".into(), "aa.color".into());
        document
            .shader_sources
            .insert("shader".into(), "aa.color".into());
        document.asset_manifest = json!({"metadata": "aa.color"});
        document
            .properties
            .active
            .global
            .insert("field".into(), json!(true));
        let original = document.clone();
        let report = cleanup_removed_render_settings(
            &mut document,
            &BTreeSet::from(["aa".into()]),
            &BTreeSet::from(["aa.color".into()]),
        );
        assert_eq!(report, RenderSettingsCleanupReport::default());
        assert_eq!(document, original);
        assert_eq!(
            cleanup_removed_render_settings(&mut document, &BTreeSet::new(), &BTreeSet::new()),
            RenderSettingsCleanupReport::default()
        );
        assert_eq!(document, original);
    }

    #[test]
    fn project_commands_choose_real_ready_presets_and_share_script_ranges() {
        let mut document = demo_document(WorldKind::Objects);
        document.engine.initialized = true;
        document.engine.exposure = default_renderer_exposure("tonemap", "aa");
        set_project_parameter(&mut document, "exposure", json!(2.5)).unwrap();
        set_project_variant(&mut document, "tonemap", "reinhard").unwrap();
        document.project.rules.push(VariantRule {
            group: "antialiasing".into(),
            variant: "off".into(),
            condition: RuleCondition::Time {
                op: Comparison::GreaterOrEqual,
                value: 1.0,
            },
        });
        let before = project_commands(&document, 0.5).unwrap();
        let after = project_commands(&document, 1.5).unwrap();
        assert!(
            before.contains(&ControlCommand::Render(RenderCommand::SetEnabled {
                pass: "aa".into(),
                enabled: true
            }))
        );
        assert!(
            after.contains(&ControlCommand::Render(RenderCommand::SetEnabled {
                pass: "aa".into(),
                enabled: false
            }))
        );
        assert!(
            after.contains(&ControlCommand::Render(RenderCommand::SetParameter {
                pass: "tonemap".into(),
                name: "mode".into(),
                value: json!(0.0)
            }))
        );
        validate_project_controls(&document, &after).unwrap();
        assert!(
            validate_project_controls(
                &document,
                &[ControlCommand::Render(RenderCommand::SetParameter {
                    pass: "tonemap".into(),
                    name: "exposure".into(),
                    value: json!(8.01)
                })]
            )
            .is_err()
        );
        assert!(
            validate_project_controls(
                &document,
                &[ControlCommand::Render(RenderCommand::SetParameter {
                    pass: "tonemap".into(),
                    name: "mode".into(),
                    value: json!(2.0)
                })]
            )
            .is_err()
        );
        assert!(
            validate_project_controls(
                &document,
                &[ControlCommand::Render(RenderCommand::SetEnabled {
                    pass: "geometry".into(),
                    enabled: false
                })]
            )
            .is_err()
        );
    }

    struct PolicyRenderer;
    impl FrameRenderer for PolicyRenderer {
        fn backend_name(&self) -> &str {
            "Policy fixture"
        }
        fn render(
            &mut self,
            scene: &SceneSnapshot,
            camera: Camera,
            width: usize,
            height: usize,
            _: &RenderOverrides,
        ) -> Result<Frame, String> {
            SoftwareRenderer.render(scene, camera, width, height, &RenderOverrides::default())
        }
        fn validate_overrides(&self, overrides: &RenderOverrides) -> Result<(), String> {
            if overrides.parameters.keys().any(|p| p != "tonemap")
                || overrides.enabled.keys().any(|p| p != "aa")
                || !overrides.resource_commands.is_empty()
            {
                return Err("Unsupported renderer target".into());
            }
            for params in overrides.parameters.values() {
                for (name, value) in params {
                    if name != "exposure"
                        && (name != "mode" || !matches!(value.as_f64(), Some(0.0 | 1.0)))
                    {
                        return Err("Unsupported renderer parameter".into());
                    }
                }
            }
            Ok(())
        }
    }

    #[test]
    fn common_runtime_component_data_and_transactional_project_controls_work_for_both_worlds() {
        let mut registry = ModuleRegistry::builtin();
        registry
            .register_renderer(
                ModuleDescriptor {
                    id: "test.policy_renderer".into(),
                    label: "Policy renderer".into(),
                    description: "Portable validation fixture".into(),
                    inputs: vec![crate::PortDescriptor {
                        name: "scene".into(),
                        data_type: "Scene3D".into(),
                        required: true,
                    }],
                    outputs: vec![crate::PortDescriptor {
                        name: "frame".into(),
                        data_type: "Frame3D".into(),
                        required: false,
                    }],
                    properties: vec![],
                },
                |_, _| Ok(Box::new(PolicyRenderer)),
            )
            .unwrap();
        let mut snapshots = vec![];
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let mut document = demo_document(kind);
            let renderer = document
                .root
                .nodes
                .iter_mut()
                .find(|n| n.id == "renderer")
                .unwrap();
            renderer.module = "test.policy_renderer".into();
            renderer.subgraph = None;
            document.engine.initialized = true;
            document.engine.enabled_modules.insert("game.motion".into());
            document.engine.exposure = default_renderer_exposure("tonemap", "aa");
            attach_component(&mut document, &registry, 1, "game.motion").unwrap();
            let mut runtime = Runtime::from_document(&document, &registry).unwrap();
            runtime.apply_project_settings(1.5).unwrap();
            let mut context = ScriptContext::new(0.0, 0.1, runtime.objects().unwrap());
            runtime.fill_script_context(&mut context);
            assert_eq!(
                context.component_property(1, "game.motion", "game.motion.speed"),
                Some(&json!(1.0))
            );
            assert!(
                context
                    .component_property(2, "game.motion", "game.motion.speed")
                    .is_none()
            );
            assert_eq!(context.parameter("exposure"), Some(&json!(1.0)));
            let before = runtime.objects().unwrap();
            assert!(
                runtime
                    .apply_project_controls(&[
                        ControlCommand::World(crate::WorldCommand::Translate {
                            id: 1,
                            delta: [1.0, 0.0, 0.0]
                        }),
                        ControlCommand::Render(RenderCommand::SetParameter {
                            pass: "tonemap".into(),
                            name: "exposure".into(),
                            value: json!(90.0)
                        })
                    ])
                    .is_err()
            );
            assert_eq!(runtime.objects().unwrap(), before);
            let mut properties = document.properties.clone();
            properties
                .active
                .components
                .get_mut(&1)
                .unwrap()
                .get_mut("game.motion")
                .unwrap()
                .insert("game.motion.speed".into(), json!(2.0));
            runtime.set_properties(properties).unwrap();
            runtime.fill_script_context(&mut context);
            assert_eq!(
                context.component_property(1, "game.motion", "game.motion.speed"),
                Some(&json!(2.0))
            );
            let mut invalid = runtime.properties().clone();
            invalid.active.components.insert(
                999,
                BTreeMap::from([("game.motion".into(), BTreeMap::new())]),
            );
            assert!(runtime.set_properties(invalid).is_err());
            assert!(!runtime.properties().active.components.contains_key(&999));
            snapshots.push(runtime.snapshot());
        }
        assert_eq!(snapshots[0], snapshots[1]);
    }

    #[test]
    fn project_architecture_is_locked_but_cosmetic_changes_and_gameplay_source_are_allowed() {
        let registry = ModuleRegistry::builtin();
        let mut baseline = demo_document(WorldKind::Objects);
        baseline.root.nodes.push(crate::make_node(
            "update",
            "custom.rust.update",
            "Gameplay",
            [0.0, 0.0],
            json!({"source_id":"update"}),
        ));
        baseline.custom_sources.insert(
            "update".into(),
            "fn update(ctx:&mut ScriptContext){}".into(),
        );
        let mut candidate = baseline.clone();
        candidate.root.nodes[0].position = [50.0, 50.0];
        candidate.root.nodes[0].label = "New label".into();
        candidate.custom_sources.insert(
            "update".into(),
            "fn update(ctx:&mut ScriptContext){ctx.remove(1);}".into(),
        );
        validate_project_edit(&candidate, &baseline, &registry).unwrap();
        candidate.root.nodes[0].module = "world.ecs".into();
        assert!(validate_project_edit(&candidate, &baseline, &registry).is_err());
        candidate = baseline.clone();
        candidate.custom_sources.insert(
            "speed".into(),
            "fn speed(time:f64,base:f64)->f64{0.0}".into(),
        );
        assert!(validate_project_edit(&candidate, &baseline, &registry).is_err());
        let mut legacy = serde_json::to_value(demo_document(WorldKind::Objects)).unwrap();
        for field in ["engine", "project", "properties"] {
            legacy.as_object_mut().unwrap().remove(field);
        }
        legacy["schema_version"] = json!(2);
        let restored: GraphDocument = serde_json::from_value(legacy).unwrap();
        assert!(!restored.engine.initialized);
        assert_eq!(restored.properties, PropertyStore::default());
        assert!(
            crate::validate_graph(&restored, &registry)
                .iter()
                .all(|d| d.severity != crate::Severity::Error)
        );
    }
}
