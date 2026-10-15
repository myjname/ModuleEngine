//! Published engine blueprints shared by authoring and Player applications.
//! The saved profile defines identity and availability. A Full project's local
//! graph/settings may evolve independently; exact active locks still apply.
//! Frozen Team architecture and package/SDK file verification remain caller policy.

use module_engine_core::{
    CompositionContext, EngineConfiguration, ExecutionGraph, Graph, GraphDocument, ProfileIdentity,
    SdkIdentity,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineBlueprint {
    pub root: Graph,
    pub engine: EngineConfiguration,
    /// Engine-owned sources only. Content templates do not supply or replace
    /// these IDs, and inheriting an Engine does not copy project controllers.
    pub custom_sources: BTreeMap<String, String>,
    pub shader_sources: BTreeMap<String, String>,
    /// Explicit schedule for New/Empty; absence of a previous game is not a
    /// reason to select a genre. Existing projects retain their own schedule.
    pub default_execution: ExecutionGraph,
    /// Permitted source modules, not an activity list or another version map.
    /// Graph/settings/scheduled systems determine which contracts require pins.
    pub available_modules: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineProfile {
    pub identity: ProfileIdentity,
    pub blueprint: EngineBlueprint,
}

impl EngineProfile {
    pub fn new(
        id: impl Into<String>,
        version: impl Into<String>,
        blueprint: EngineBlueprint,
    ) -> Result<Self, String> {
        let identity = ProfileIdentity::from_payload(id, version, &blueprint)?;
        let profile = Self {
            identity,
            blueprint,
        };
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.identity.validate_payload(&self.blueprint)?;
        if self
            .blueprint
            .available_modules
            .iter()
            .any(|id| id.trim().is_empty() || id.chars().any(char::is_control))
        {
            return Err("Engine profile availability needs nonempty module IDs".into());
        }
        for node in self
            .blueprint
            .default_execution
            .stages
            .iter()
            .flat_map(|stage| &stage.nodes)
        {
            if !self.blueprint.available_modules.contains(&node.module) {
                return Err(format!(
                    "Engine profile default schedule requires unavailable module '{}'",
                    node.module
                ));
            }
        }
        Ok(())
    }

    pub fn document_value(&self) -> Result<Value, String> {
        self.validate()?;
        serde_json::to_value(self).map_err(|error| format!("Engine profile JSON: {error}"))
    }

    /// Read the retained profile, never a mutable catalog fallback or a profile
    /// inferred from the current game's schedule/installed modules.
    pub fn from_document(document: &GraphDocument) -> Result<Self, String> {
        let value = document.engine_profile.as_ref().ok_or(
            "Project has no selected Engine profile; choose one through New or explicit migration",
        )?;
        let profile: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("Saved Engine profile: {error}"))?;
        profile.validate()?;
        Ok(profile)
    }

    /// Validate the saved published payload, selection and exact active pins.
    /// This deliberately does not compare the current root/settings to the base
    /// blueprint. Actual installed SDK identity is checked by validate_with_sdk
    /// or CompositionContext::validate_lock at the host/compile boundary.
    pub fn validate_document(
        &self,
        document: &GraphDocument,
        composition: &CompositionContext,
    ) -> Result<(), String> {
        self.validate()?;
        let saved = Self::from_document(document)?;
        if &saved != self {
            return Err("Saved Engine profile differs from the selected published profile".into());
        }
        if document.schema_version != 4 {
            return Err("Engine profiles require explicit format 4 migration".into());
        }
        let lock = document
            .engine_lock
            .as_ref()
            .ok_or("Selected Engine profile requires an exact engine lock")?;
        lock.validate_profile(&self.identity)?;
        lock.validate_integrity(&document.module_versions)?;
        let required = composition.required_contracts(document)?;
        for id in required.module_versions.keys() {
            if !self.blueprint.available_modules.contains(id) {
                return Err(format!(
                    "Selected Engine profile '{}' does not provide required module '{id}'",
                    self.identity.id
                ));
            }
        }
        require_pins(
            "module",
            &required.module_versions,
            &document.module_versions,
        )?;
        require_pins(
            "component schema",
            &required.component_schemas,
            &lock.component_schemas,
        )?;
        require_pins(
            "system state schema",
            &required.system_state_schemas,
            &lock.system_state_schemas,
        )?;
        if required.requires_sdk {
            lock.sdk
                .as_ref()
                .ok_or("Active Custom requires a saved exact SDK identity")?
                .validate()?;
        }
        Ok(())
    }

    /// Callers with checked SDK evidence can perform the complete portable
    /// profile+lock check together. Native-only projects need no SDK installation.
    pub fn validate_with_sdk(
        &self,
        document: &GraphDocument,
        composition: &CompositionContext,
        installed_sdk: Option<&SdkIdentity>,
    ) -> Result<(), String> {
        self.validate_document(document, composition)?;
        composition.validate_lock(document, installed_sdk)
    }
}

fn require_pins<T: PartialEq>(
    kind: &str,
    required: &BTreeMap<String, T>,
    pins: &BTreeMap<String, T>,
) -> Result<(), String> {
    for (id, installed) in required {
        match pins.get(id) {
            None => return Err(format!("Required {kind} '{id}' has no exact pin")),
            Some(pinned) if pinned != installed => {
                return Err(format!("Required {kind} '{id}' differs from its exact pin"));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{
        GameProject, ModuleRegistry, SystemRegistry, WorldKind, demo_document,
    };
    use serde_json::json;

    fn composition() -> CompositionContext {
        let mut systems = SystemRegistry::builtin();
        systems.install_module("fixture.extra", "1").unwrap();
        CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap()
    }

    fn blueprint(composition: &CompositionContext) -> EngineBlueprint {
        let document = demo_document(WorldKind::Objects);
        let mut available_modules = composition
            .required_contracts(&document)
            .unwrap()
            .module_versions
            .into_keys()
            .collect::<BTreeSet<_>>();
        available_modules.insert("world.ecs".into());
        EngineBlueprint {
            root: document.root,
            engine: document.engine,
            custom_sources: document.custom_sources,
            shader_sources: document.shader_sources,
            default_execution: ExecutionGraph::default(),
            available_modules,
        }
    }

    fn profile(composition: &CompositionContext) -> EngineProfile {
        EngineProfile::new("fixture.engine", "1", blueprint(composition)).unwrap()
    }

    fn document(profile: &EngineProfile, composition: &CompositionContext) -> GraphDocument {
        let mut document = demo_document(WorldKind::Objects);
        document.schema_version = 4;
        document.engine_profile = Some(profile.document_value().unwrap());
        document.game = Some(GameProject::empty());
        composition
            .capture_lock(
                &document,
                profile.identity.clone(),
                Some(SdkIdentity::current()),
            )
            .unwrap()
            .apply_to(&mut document);
        document
    }

    #[test]
    fn identity_covers_sources_settings_default_schedule_and_availability() {
        let composition = composition();
        let original = profile(&composition);
        let changes: [fn(&mut EngineBlueprint); 5] = [
            |blueprint| blueprint.root.nodes[0].module = "world.ecs".into(),
            |blueprint| blueprint.engine.initialized = true,
            |blueprint| {
                blueprint
                    .custom_sources
                    .insert("engine source".into(), "text".into());
            },
            |blueprint| {
                blueprint
                    .shader_sources
                    .insert("engine shader".into(), "text".into());
            },
            |blueprint| {
                blueprint.available_modules.insert("fixture.extra".into());
            },
        ];
        for change in changes {
            let mut changed = original.clone();
            change(&mut changed.blueprint);
            assert!(changed.validate().unwrap_err().contains("fingerprint"));
        }
        let mut changed = original;
        changed
            .blueprint
            .default_execution
            .resources
            .push(module_engine_core::SystemResource {
                name: "explicit resource".into(),
                data_type: "Fixture/1".into(),
                external: false,
                initial: Value::Null,
            });
        assert!(changed.validate().is_err());
    }

    #[test]
    fn retained_profile_roundtrips_and_missing_or_malformed_selection_has_no_fallback() {
        let composition = composition();
        let profile = profile(&composition);
        let mut document = document(&profile, &composition);
        let encoded = serde_json::to_vec(&document).unwrap();
        let reopened: GraphDocument = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(EngineProfile::from_document(&reopened).unwrap(), profile);
        profile
            .validate_with_sdk(&reopened, &composition, Some(&SdkIdentity::current()))
            .unwrap();
        document.engine_profile = None;
        assert!(
            EngineProfile::from_document(&document)
                .unwrap_err()
                .contains("no selected Engine profile")
        );
        document.engine_profile =
            Some(json!({"identity":profile.identity,"blueprint":profile.blueprint,"unknown":true}));
        assert!(
            EngineProfile::from_document(&document)
                .unwrap_err()
                .contains("unknown field")
        );
    }

    #[test]
    fn local_full_engine_graph_and_settings_can_differ_from_published_base() {
        let composition = composition();
        let profile = profile(&composition);
        let mut document = document(&profile, &composition);
        document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "world")
            .unwrap()
            .module = "world.ecs".into();
        document.engine.initialized = true;
        composition
            .capture_lock(
                &document,
                profile.identity.clone(),
                Some(SdkIdentity::current()),
            )
            .unwrap()
            .apply_to(&mut document);
        assert_ne!(document.root, profile.blueprint.root);
        assert_ne!(document.engine, profile.blueprint.engine);
        profile.validate_document(&document, &composition).unwrap();
        assert_eq!(EngineProfile::from_document(&document).unwrap(), profile);
    }

    #[test]
    fn installed_module_outside_selected_availability_rejects_even_with_fresh_lock() {
        let composition = composition();
        let profile = profile(&composition);
        let mut document = document(&profile, &composition);
        document
            .engine
            .enabled_modules
            .insert("fixture.extra".into());
        composition
            .capture_lock(
                &document,
                profile.identity.clone(),
                Some(SdkIdentity::current()),
            )
            .unwrap()
            .apply_to(&mut document);
        assert!(
            profile
                .validate_document(&document, &composition)
                .unwrap_err()
                .contains("does not provide required module 'fixture.extra'")
        );
    }

    #[test]
    fn profile_selection_and_exact_pins_cannot_be_silently_replaced() {
        let composition = composition();
        let profile = profile(&composition);
        let original = document(&profile, &composition);
        let mut selected = original.clone();
        let another = EngineProfile::new("another.engine", "1", profile.blueprint.clone()).unwrap();
        composition
            .capture_lock(&selected, another.identity, Some(SdkIdentity::current()))
            .unwrap()
            .apply_to(&mut selected);
        assert!(
            profile
                .validate_document(&selected, &composition)
                .unwrap_err()
                .contains("selected exact profile")
        );
        let mut pin = original.clone();
        pin.module_versions.remove("world.objects");
        assert!(profile.validate_document(&pin, &composition).is_err());
        let mut payload = original;
        payload.engine_profile.as_mut().unwrap()["blueprint"]["available_modules"] = json!([]);
        assert!(
            EngineProfile::from_document(&payload)
                .unwrap_err()
                .contains("fingerprint")
        );
    }

    #[test]
    fn inactive_catalog_versions_are_archived_without_invalidating_profile() {
        let composition = composition();
        let profile = profile(&composition);
        let mut document = document(&profile, &composition);
        document
            .module_versions
            .insert("fixture.extra".into(), "unrelated old version".into());
        composition
            .capture_lock(
                &document,
                profile.identity.clone(),
                Some(SdkIdentity::current()),
            )
            .unwrap()
            .apply_to(&mut document);
        profile.validate_document(&document, &composition).unwrap();
        assert_eq!(
            document.module_versions["fixture.extra"],
            "unrelated old version"
        );
        assert!(profile.blueprint.default_execution.stages.is_empty());
    }
}
