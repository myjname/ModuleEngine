//! Shared installed catalogs and activity derived from the authored blueprint.
use crate::{
    ComponentValidationContext, Diagnostic, GraphDocument, ModuleDescriptor, ModuleRegistry,
    Severity, SystemRegistry,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct CompositionContext {
    pub modules: ModuleRegistry,
    pub systems: SystemRegistry,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompositionActivity {
    pub modules: BTreeSet<String>,
    pub components: BTreeSet<String>,
}
impl CompositionContext {
    /// Install source metadata once. The root module catalog also carries pins
    /// for systems/schema owners, so graph validation and saves share one view.
    pub fn new(mut modules: ModuleRegistry, mut systems: SystemRegistry) -> Result<Self, String> {
        for (id, version) in systems.module_versions() {
            if let Some(installed) = modules.module_version(id) {
                if installed != version.as_str() {
                    return Err(format!(
                        "Module '{id}' has conflicting installed versions '{installed}' and '{version}'"
                    ));
                }
            } else {
                modules.register_module(ModuleDescriptor {
                    id: id.clone(),
                    label: id.clone(),
                    description: "Installed source system or component owner".into(),
                    inputs: vec![],
                    outputs: vec![],
                    properties: vec![],
                })?;
                modules.set_module_version(id, version)?;
            }
        }
        // A component can be owned by a root world/render source module without
        // an executable gameplay system. Mirror its installed version only;
        // do not create another descriptor or a synthetic system factory.
        let owners = systems
            .components()
            .schemas()
            .map(|schema| schema.owner_module.clone())
            .collect::<BTreeSet<_>>();
        for owner in owners {
            if systems.module_version(&owner).is_none()
                && let Some(version) = modules.module_version(&owner)
            {
                systems.install_module(&owner, version)?;
            }
        }
        let result = Self { modules, systems };
        for (owner, dependencies) in result.systems.module_dependencies() {
            if result.module_version(owner).is_none() {
                return Err(format!(
                    "Module dependency owner '{owner}' is not installed"
                ));
            }
            for dependency in dependencies {
                if result.module_version(dependency).is_none() {
                    return Err(format!(
                        "Module '{owner}' requires uninstalled module '{dependency}'"
                    ));
                }
            }
        }
        for schema in result.systems.components().schemas() {
            if result.module_version(&schema.owner_module).is_none() {
                return Err(format!(
                    "Component '{}' owner '{}' is not installed",
                    schema.id, schema.owner_module
                ));
            }
            for module in schema.requires_modules.iter().chain(
                schema
                    .fields
                    .iter()
                    .flat_map(|field| &field.requires_modules),
            ) {
                if result.module_version(module).is_none() {
                    return Err(format!(
                        "Component '{}' requires uninstalled module '{module}'",
                        schema.id
                    ));
                }
            }
            for component in schema.requires_components.iter().chain(
                schema
                    .fields
                    .iter()
                    .flat_map(|field| &field.requires_components),
            ) {
                if result.systems.components().schema(component).is_none() {
                    return Err(format!(
                        "Component '{}' requires unknown component '{component}'",
                        schema.id
                    ));
                }
            }
            for field in &schema.fields {
                if let crate::PropertyType::ObjectReference {
                    required_components,
                    ..
                } = &field.value_type
                {
                    for component in required_components {
                        if result.systems.components().schema(component).is_none() {
                            return Err(format!(
                                "Field '{}' requires unknown target component '{component}'",
                                field.id
                            ));
                        }
                    }
                }
            }
        }
        Ok(result)
    }
    pub fn module_version(&self, id: &str) -> Option<&str> {
        self.modules.module_version(id)
    }
    pub fn module_versions(&self) -> BTreeMap<String, String> {
        self.modules
            .descriptors()
            .iter()
            .filter_map(|module| {
                self.module_version(&module.id)
                    .map(|version| (module.id.clone(), version.into()))
            })
            .collect()
    }
    pub fn activity(&self, document: &GraphDocument) -> Result<CompositionActivity, String> {
        let mut modules = crate::effective_modules(document);
        let mut components = BTreeSet::new();
        if let Some(game) = &document.game {
            components = self.systems.active_components(&game.execution)?;
            modules.extend(
                game.execution
                    .stages
                    .iter()
                    .flat_map(|stage| &stage.nodes)
                    .map(|node| node.module.clone()),
            );
        }
        let renderer = crate::connected_modules(document)
            .iter()
            .filter_map(|id| self.modules.descriptor(id))
            .any(|descriptor| {
                descriptor
                    .inputs
                    .iter()
                    .any(|pin| pin.data_type == "Scene3D")
                    && descriptor
                        .outputs
                        .iter()
                        .any(|pin| pin.data_type == "Frame3D")
            });
        if renderer {
            components.extend(["mesh".into(), "material".into()]);
        } else {
            for component in ["mesh", "material"] {
                let consumed = document.game.as_ref().is_some_and(|game| {
                    game.execution
                        .stages
                        .iter()
                        .flat_map(|stage| &stage.nodes)
                        .any(|node| {
                            self.systems
                                .descriptor(&node.module)
                                .is_some_and(|descriptor| {
                                    descriptor.components.iter().any(|name| name == component)
                                })
                                || node
                                    .params
                                    .get("components")
                                    .and_then(Value::as_array)
                                    .is_some_and(|names| {
                                        names.iter().any(|name| name.as_str() == Some(component))
                                    })
                        })
                });
                if !consumed {
                    components.remove(component);
                }
            }
        }
        // Consuming a component activates its owner's data contribution, not
        // its executable system. Availability dependencies include these owners
        // even when the authored schedule contains only another consumer.
        for component in &components {
            let schema =
                self.systems.components().schema(component).ok_or_else(|| {
                    format!("Active component '{component}' has no installed schema")
                })?;
            modules.insert(schema.owner_module.clone());
        }
        for module in &modules {
            if self.module_version(module).is_none() {
                return Err(format!("Active module '{module}' is not installed"));
            }
            if let Some(dependencies) = self.systems.module_dependencies().get(module)
                && let Some(dependency) = dependencies
                    .iter()
                    .find(|dependency| !modules.contains(*dependency))
            {
                return Err(format!(
                    "Active module '{module}' requires active module '{dependency}'"
                ));
            }
        }
        for component in &components {
            let schema =
                self.systems.components().schema(component).ok_or_else(|| {
                    format!("Active component '{component}' has no installed schema")
                })?;
            if self.module_version(&schema.owner_module).is_none() {
                return Err(format!("Component '{component}' owner is not installed"));
            }
            if let Some(required) = schema
                .requires_modules
                .iter()
                .find(|required| !modules.contains(*required))
            {
                return Err(format!(
                    "Active component '{component}' requires active module '{required}'"
                ));
            }
            if let Some(required) = schema
                .requires_components
                .iter()
                .find(|required| !components.contains(*required))
            {
                return Err(format!(
                    "Active component '{component}' requires active component '{required}'"
                ));
            }
        }
        Ok(CompositionActivity {
            modules,
            components,
        })
    }
    /// Production document validation always supplies a strict manifest map.
    pub fn validate_document(&self, document: &GraphDocument) -> Vec<Diagnostic> {
        let mut diagnostics = crate::validate_graph(document, &self.modules);
        let result = self.validate_game(document);
        if let Err(message) = result {
            diagnostics.push(Diagnostic {
                severity: Severity::Error,
                message,
                node: None,
            });
        }
        diagnostics
    }

    /// Scene/execution edits validate active component contracts independently
    /// of an unfinished root/GPU canvas. Full publication still validates graphs.
    pub fn validate_game(&self, document: &GraphDocument) -> Result<(), String> {
        let activity = self.activity(document)?;
        let assets = component_asset_paths(&document.asset_manifest)?;
        if let Some(game) = &document.game {
            let context = ComponentValidationContext {
                scenes: &game.scenes,
                modules: &activity.modules,
                asset_paths: Some(&assets),
                active_components: Some(&activity.components),
            };
            game.validate_with_context(&self.systems, &context)?;
        }
        Ok(())
    }
}

/// Read only portable ID/path metadata; file hashes and IO remain project-tools'
/// responsibility. Null is an empty strict manifest, never a source-only mode.
pub fn component_asset_paths(manifest: &Value) -> Result<BTreeMap<String, String>, String> {
    if manifest.is_null() {
        return Ok(BTreeMap::new());
    }
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("Unsupported asset manifest version".into());
    }
    let assets = manifest
        .get("assets")
        .and_then(Value::as_object)
        .ok_or("Asset manifest needs assets map")?;
    assets
        .iter()
        .map(|(id, entry)| {
            let path = entry
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("Asset '{id}' needs a relative path"))?;
            crate::validate_asset_reference_syntax(path)?;
            if id.trim().is_empty()
                || id.chars().any(char::is_control)
                || path.starts_with("asset://")
            {
                return Err("Invalid asset manifest ID/path".into());
            }
            Ok((id.clone(), path.into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuthoredScene, ComponentSchema, ExecutableSystem, ExecutionGraph, GameProject,
        PropertySchema, PropertyScope, PropertyType, SystemContext, SystemDescriptor, SystemNode,
        SystemStage, WorldKind,
    };
    use serde_json::json;

    struct Noop;
    impl ExecutableSystem for Noop {
        fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            Ok(())
        }
    }
    fn install_system(registry: &mut SystemRegistry, id: &str, components: Vec<String>) {
        registry
            .register(
                SystemDescriptor {
                    id: id.into(),
                    state_schema: "state/1".into(),
                    components,
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(Noop)),
            )
            .unwrap();
    }
    fn node(id: &str) -> SystemNode {
        SystemNode {
            id: format!("{id}-instance"),
            module: id.into(),
            params: json!({}),
            bindings: BTreeMap::new(),
            after: vec![],
        }
    }
    fn document(nodes: Vec<SystemNode>) -> GraphDocument {
        let mut document = crate::demo_document(WorldKind::Objects);
        document.game = Some(GameProject {
            entry_scene: "empty".into(),
            scenes: BTreeMap::from([("empty".into(), AuthoredScene::default())]),
            prefabs: BTreeMap::new(),
            execution: ExecutionGraph {
                resources: vec![],
                stages: vec![SystemStage {
                    name: "authored".into(),
                    nodes,
                }],
            },
            audio_cues: BTreeMap::new(),
        });
        document
    }
    #[test]
    fn owner_installation_version_and_system_consumption_are_separate() {
        let mut systems = SystemRegistry::builtin();
        systems.install_module("external.owner", "9.2.1").unwrap();
        install_system(
            &mut systems,
            "external.consumer",
            vec!["external.component".into()],
        );
        systems
            .components_mut()
            .register(ComponentSchema {
                id: "external.component".into(),
                owner_module: "external.owner".into(),
                version: "data/3".into(),
                label: "External".into(),
                defaults: json!({}),
                fields: vec![],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        let composition = CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap();
        let mut document = document(vec![node("external.consumer")]);
        document
            .module_versions
            .insert("external.owner".into(), "9.2.1".into());
        let activity = composition.activity(&document).unwrap();
        assert!(activity.components.contains("external.component"));
        assert!(activity.modules.contains("external.owner"));
        assert_eq!(
            document.game.as_ref().unwrap().execution.stages[0].nodes,
            vec![node("external.consumer")],
            "a schema contribution must not schedule its owning system"
        );
        assert_eq!(composition.module_version("external.owner"), Some("9.2.1"));
        let profile = crate::ProfileIdentity::from_payload(
            "external.fixture",
            "1",
            &json!({"owner":"external.owner"}),
        )
        .unwrap();
        composition
            .capture_lock(&document, profile, Some(crate::SdkIdentity::current()))
            .unwrap()
            .apply_to(&mut document);
        assert!(
            composition
                .validate_document(&document)
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error)
        );
        document
            .module_versions
            .insert("external.owner".into(), "data/3".into());
        assert!(
            composition
                .validate_lock(&document, Some(&crate::SdkIdentity::current()))
                .unwrap_err()
                .contains("external.owner")
        );
    }
    #[test]
    fn schema_owner_availability_dependencies_apply_without_owner_execution() {
        let mut systems = SystemRegistry::builtin();
        systems.install_module("external.owner", "9.2.1").unwrap();
        systems
            .install_module("external.data-library", "3.0")
            .unwrap();
        systems
            .require_module("external.owner", "external.data-library")
            .unwrap();
        install_system(
            &mut systems,
            "external.consumer",
            vec!["external.component".into()],
        );
        systems
            .components_mut()
            .register(ComponentSchema {
                id: "external.component".into(),
                owner_module: "external.owner".into(),
                version: "1".into(),
                label: "External".into(),
                defaults: json!({}),
                fields: vec![],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        let composition = CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap();
        let mut document = document(vec![node("external.consumer")]);
        let authored = document.game.as_ref().unwrap().execution.clone();
        let error = composition.activity(&document).unwrap_err();
        assert!(error.contains("external.owner") && error.contains("external.data-library"));
        document
            .engine
            .enabled_modules
            .insert("external.data-library".into());
        let activity = composition.activity(&document).unwrap();
        assert!(activity.modules.contains("external.owner"));
        assert!(activity.modules.contains("external.data-library"));
        assert!(
            composition
                .systems
                .descriptor("external.data-library")
                .is_none()
        );
        assert_eq!(document.game.as_ref().unwrap().execution, authored);
    }
    #[test]
    fn root_only_schema_owner_is_mirrored_without_duplicate_descriptors_or_systems() {
        let mut modules = ModuleRegistry::builtin();
        modules
            .register_module(ModuleDescriptor {
                id: "external.root-owner".into(),
                label: "Root source".into(),
                description: "Root-owned data contract".into(),
                inputs: vec![],
                outputs: vec![],
                properties: vec![],
            })
            .unwrap();
        modules
            .set_module_version("external.root-owner", "4.2.0")
            .unwrap();
        let mut systems = SystemRegistry::builtin();
        install_system(
            &mut systems,
            "external.consumer",
            vec!["external.root-component".into()],
        );
        systems
            .components_mut()
            .register(ComponentSchema {
                id: "external.root-component".into(),
                owner_module: "external.root-owner".into(),
                version: "1".into(),
                label: "Root data".into(),
                defaults: json!({"value":4.}),
                fields: vec![PropertySchema {
                    id: "external.root-component.value".into(),
                    label: "Value".into(),
                    scope: PropertyScope::Component {
                        component: "external.root-component".into(),
                    },
                    value_type: PropertyType::Number { min: 0., max: 8. },
                    default: json!(4.),
                    requires_modules: vec!["external.root-owner".into()],
                    requires_components: vec![],
                }],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        assert!(systems.module_version("external.root-owner").is_none());
        let composition = CompositionContext::new(modules, systems).unwrap();
        assert_eq!(
            composition.systems.module_version("external.root-owner"),
            Some("4.2.0")
        );
        assert_eq!(
            composition
                .modules
                .descriptors()
                .iter()
                .filter(|descriptor| descriptor.id == "external.root-owner")
                .count(),
            1
        );
        assert!(
            composition
                .systems
                .descriptor("external.root-owner")
                .is_none()
        );
        let mut document = document(vec![node("external.consumer")]);
        let mut object = crate::AuthoredObject::new(crate::WorldObject {
            id: 1,
            parent: None,
            transform: crate::Transform::default(),
            color: [255; 4],
        });
        object
            .components
            .insert("external.root-component".into(), json!({}));
        document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("empty")
            .unwrap()
            .objects
            .push(object);
        let authored = document.game.as_ref().unwrap().execution.clone();
        composition.validate_game(&document).unwrap();
        assert!(
            composition
                .activity(&document)
                .unwrap()
                .modules
                .contains("external.root-owner")
        );
        assert_eq!(document.game.as_ref().unwrap().execution, authored);
    }
    #[test]
    fn module_availability_dependency_does_not_impose_system_order() {
        let mut systems = SystemRegistry::builtin();
        install_system(&mut systems, "external.consumer", vec![]);
        install_system(&mut systems, "external.helper", vec![]);
        systems
            .require_module("external.consumer", "external.helper")
            .unwrap();
        let composition = CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap();
        let first = node("external.consumer");
        let mut later = node("external.helper");
        later.after.push(first.id.clone());
        let mut document = document(vec![first, later]);
        let authored = document.game.as_ref().unwrap().execution.clone();
        composition.activity(&document).unwrap();
        assert_eq!(
            document.game.as_ref().unwrap().execution,
            authored,
            "availability validation must never reorder the schedule"
        );
        document.game.as_mut().unwrap().execution.stages[0]
            .nodes
            .pop();
        assert!(
            composition
                .activity(&document)
                .unwrap_err()
                .contains("requires active module")
        );
        document
            .engine
            .enabled_modules
            .insert("external.helper".into());
        composition.activity(&document).unwrap();
        assert_eq!(
            document.game.as_ref().unwrap().execution.stages[0]
                .nodes
                .len(),
            1,
            "an explicit feature can satisfy availability without a scheduled helper"
        );
    }
    #[test]
    fn constructor_rejects_missing_owner_and_dependency_installations() {
        let mut systems = SystemRegistry::builtin();
        systems
            .components_mut()
            .register(ComponentSchema {
                id: "unowned".into(),
                owner_module: "missing.owner".into(),
                version: "1".into(),
                label: "Unowned".into(),
                defaults: json!({}),
                fields: vec![],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        assert!(
            CompositionContext::new(ModuleRegistry::builtin(), systems)
                .err()
                .unwrap()
                .contains("owner")
        );
        let mut systems = SystemRegistry::builtin();
        systems
            .require_module("scene.extract", "missing.dependency")
            .unwrap();
        assert!(
            CompositionContext::new(ModuleRegistry::builtin(), systems)
                .err()
                .unwrap()
                .contains("uninstalled")
        );
    }
    #[test]
    fn render_components_follow_connected_rendering_or_explicit_consumption() {
        let mut systems = SystemRegistry::builtin();
        install_system(&mut systems, "external.geometry", vec!["mesh".into()]);
        let composition = CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap();
        let mut document = document(vec![]);
        document
            .root
            .edges
            .retain(|edge| edge.to_node != "viewport");
        // Explicitly enabling a renderer does not connect its frame output.
        document
            .engine
            .enabled_modules
            .insert("render.software3d".into());
        let activity = composition.activity(&document).unwrap();
        assert!(!activity.components.contains("mesh"));
        assert!(!activity.components.contains("material"));
        document.game.as_mut().unwrap().execution.stages[0]
            .nodes
            .push(node("external.geometry"));
        let activity = composition.activity(&document).unwrap();
        assert!(
            activity.components.contains("mesh"),
            "a source consumer can use geometry without rendering"
        );
        assert!(!activity.components.contains("material"));
    }
    #[test]
    fn strict_document_validation_checks_external_schema_asset_defaults() {
        let mut systems = SystemRegistry::builtin();
        install_system(
            &mut systems,
            "external.data",
            vec!["external.settings".into()],
        );
        systems
            .components_mut()
            .register(ComponentSchema {
                id: "external.settings".into(),
                owner_module: "external.data".into(),
                version: "1".into(),
                label: "External".into(),
                defaults: json!({"config":"asset://settings"}),
                fields: vec![PropertySchema {
                    id: "external.settings.config".into(),
                    label: "Config".into(),
                    scope: PropertyScope::Component {
                        component: "external.settings".into(),
                    },
                    value_type: PropertyType::AssetReference {
                        kinds: vec!["json".into()],
                        optional: false,
                    },
                    default: json!("asset://settings"),
                    requires_modules: vec![],
                    requires_components: vec![],
                }],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        let composition = CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap();
        let mut document = document(vec![node("external.data")]);
        let mut object = crate::AuthoredObject::new(crate::WorldObject {
            id: 1,
            parent: None,
            transform: crate::Transform::default(),
            color: [255; 4],
        });
        object
            .components
            .insert("external.settings".into(), json!({}));
        document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("empty")
            .unwrap()
            .objects
            .push(object);
        assert!(
            composition
                .validate_document(&document)
                .iter()
                .any(|diagnostic| diagnostic.message.contains("Unknown asset ID"))
        );
        document.asset_manifest = json!({"schema_version":1,"assets":{"settings":{"path":"data/settings.json","sha256":"fixture"}}});
        assert!(
            composition
                .validate_document(&document)
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error)
        );
    }
}
