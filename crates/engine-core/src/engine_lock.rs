//! Exact portable contract locks. Activity comes from the authored document,
//! never from the saved pin maps. Package files and SDK sources are checked by
//! the caller; this module performs no IO and activates no implementation.

use crate::{ComponentSchema, CompositionContext, GraphDocument};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const ENGINE_LOCK_DOCUMENT_VERSION: u32 = 4;
/// Shared data/API identity, not a Rust memory ABI or compiler compatibility claim.
pub const SDK_DATA_API_FINGERPRINT: &str = "module-engine-data-api/v2";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileIdentity {
    pub id: String,
    pub version: String,
    /// Hash of the published profile payload, independent of installed catalogs.
    pub fingerprint: String,
}

impl ProfileIdentity {
    /// The caller supplies its published blueprint/contract payload, excluding
    /// the identity fingerprint. Local locators and example assets do not belong
    /// in that payload. Retain it with the selected profile/package for checking.
    pub fn from_payload<T: Serialize>(
        id: impl Into<String>,
        version: impl Into<String>,
        payload: &T,
    ) -> Result<Self, String> {
        let id = id.into();
        let version = version.into();
        if id.trim().is_empty() || version.trim().is_empty() {
            return Err("Engine profile needs nonempty identity and exact version".into());
        }
        let payload = serde_json::to_value(payload)
            .map_err(|error| format!("Profile payload JSON: {error}"))?;
        let fingerprint = canonical_contract_hash(&json!({
            "id":id,"version":version,"payload":payload,
        }))?;
        Ok(Self {
            id,
            version,
            fingerprint,
        })
    }

    pub fn validate_payload<T: Serialize>(&self, payload: &T) -> Result<(), String> {
        self.validate()?;
        let expected = Self::from_payload(&self.id, &self.version, payload)?;
        if self != &expected {
            return Err(format!(
                "Engine profile '{}' published fingerprint differs",
                self.id
            ));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty()
            || self.version.trim().is_empty()
            || !is_sha256(&self.fingerprint)
        {
            return Err(
                "Engine profile needs an ID, exact version and lowercase SHA-256 fingerprint"
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkIdentity {
    pub version: String,
    pub data_api_fingerprint: String,
}

impl SdkIdentity {
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").into(),
            data_api_fingerprint: SDK_DATA_API_FINGERPRINT.into(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version.trim().is_empty() || self.data_api_fingerprint != SDK_DATA_API_FINGERPRINT {
            return Err(format!(
                "SDK requires exact data/API identity '{SDK_DATA_API_FINGERPRINT}' and a version"
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaPin {
    pub owner_module: String,
    pub version: String,
    pub contract_hash: String,
}

impl SchemaPin {
    pub fn capture(schema: &ComponentSchema) -> Result<Self, String> {
        Ok(Self {
            owner_module: schema.owner_module.clone(),
            version: schema.version.clone(),
            contract_hash: canonical_contract_hash(schema)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineLock {
    pub profile: ProfileIdentity,
    /// Integrity of the captured payload, including GraphDocument.module_versions.
    /// No second authoritative module-version or activity map is persisted here.
    pub lock_fingerprint: String,
    pub component_schemas: BTreeMap<String, SchemaPin>,
    pub system_state_schemas: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<SdkIdentity>,
}

impl EngineLock {
    fn fingerprint(&self, module_versions: &BTreeMap<String, String>) -> Result<String, String> {
        canonical_contract_hash(&json!({
            "profile":self.profile,
            "module_versions":module_versions,
            "component_schemas":self.component_schemas,
            "system_state_schemas":self.system_state_schemas,
            "sdk":self.sdk,
        }))
    }

    pub fn validate_integrity(
        &self,
        module_versions: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        self.profile.validate()?;
        if !is_sha256(&self.lock_fingerprint)
            || self.lock_fingerprint != self.fingerprint(module_versions)?
        {
            return Err("Engine lock fingerprint differs from its captured contract payload; use an explicit lock/profile update".into());
        }
        Ok(())
    }

    /// Resolve a checked retained profile/package identity outside core. An
    /// unrelated mutable catalog entry must not replace this saved selection.
    pub fn validate_profile(&self, selected: &ProfileIdentity) -> Result<(), String> {
        selected.validate()?;
        if &self.profile != selected {
            return Err(format!(
                "Engine profile '{}' does not match the selected exact profile identity",
                self.profile.id
            ));
        }
        Ok(())
    }
}

/// Transient derivation. It is never a persisted enable/disable list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredContracts {
    pub module_versions: BTreeMap<String, String>,
    pub component_schemas: BTreeMap<String, SchemaPin>,
    pub system_state_schemas: BTreeMap<String, String>,
    pub requires_sdk: bool,
}

/// Publish these two records together only during New, explicit profile updates
/// or migration. Normal open/save/run/export must validate without capturing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractLockSnapshot {
    pub module_versions: BTreeMap<String, String>,
    pub engine_lock: EngineLock,
}

impl ContractLockSnapshot {
    pub fn apply_to(self, document: &mut GraphDocument) {
        document.module_versions = self.module_versions;
        document.engine_lock = Some(self.engine_lock);
    }
}

impl CompositionContext {
    /// Derive required owner, component and state contracts before inspecting
    /// saved locks, including schemas consumed by systems with no instances yet.
    pub fn required_contracts(
        &self,
        document: &GraphDocument,
    ) -> Result<RequiredContracts, String> {
        let activity = self.activity(document)?;
        let module_versions = activity
            .modules
            .iter()
            .map(|id| {
                self.module_version(id)
                    .map(|version| (id.clone(), version.into()))
                    .ok_or_else(|| format!("Required module '{id}' is not installed"))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let component_schemas = activity
            .components
            .iter()
            .map(|id| {
                let schema =
                    self.systems.components().schema(id).ok_or_else(|| {
                        format!("Required component '{id}' has no installed schema")
                    })?;
                Ok((id.clone(), SchemaPin::capture(schema)?))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let mut system_state_schemas = BTreeMap::new();
        if let Some(game) = &document.game {
            for node in game.execution.stages.iter().flat_map(|stage| &stage.nodes) {
                let descriptor = self
                    .systems
                    .descriptor(&node.module)
                    .ok_or_else(|| format!("Required system '{}' is not installed", node.module))?;
                system_state_schemas.insert(node.module.clone(), descriptor.state_schema.clone());
            }
        }
        let requires_sdk = activity
            .modules
            .iter()
            .any(|id| matches!(id.as_str(), "custom.rust" | "custom.rust.update"));
        Ok(RequiredContracts {
            module_versions,
            component_schemas,
            system_state_schemas,
            requires_sdk,
        })
    }

    pub fn capture_lock(
        &self,
        document: &GraphDocument,
        profile: ProfileIdentity,
        sdk: Option<SdkIdentity>,
    ) -> Result<ContractLockSnapshot, String> {
        profile.validate()?;
        let required = self.required_contracts(document)?;
        if required.requires_sdk {
            sdk.as_ref()
                .ok_or("Active Custom requires an explicit SDK identity")?
                .validate()?;
        }
        let mut module_versions = document.module_versions.clone();
        module_versions.extend(required.module_versions);
        let archive = document.engine_lock.as_ref();
        let mut component_schemas = archive
            .map(|lock| lock.component_schemas.clone())
            .unwrap_or_default();
        component_schemas.extend(required.component_schemas);
        let mut system_state_schemas = archive
            .map(|lock| lock.system_state_schemas.clone())
            .unwrap_or_default();
        system_state_schemas.extend(required.system_state_schemas);
        let sdk = sdk.or_else(|| archive.and_then(|lock| lock.sdk.clone()));
        let mut engine_lock = EngineLock {
            profile,
            lock_fingerprint: String::new(),
            component_schemas,
            system_state_schemas,
            sdk,
        };
        engine_lock.lock_fingerprint = engine_lock.fingerprint(&module_versions)?;
        Ok(ContractLockSnapshot {
            module_versions,
            engine_lock,
        })
    }

    pub fn validate_lock(
        &self,
        document: &GraphDocument,
        installed_sdk: Option<&SdkIdentity>,
    ) -> Result<(), String> {
        self.validate_lock_snapshot(document, document.engine_lock.as_ref(), installed_sdk)
    }

    pub fn validate_lock_snapshot(
        &self,
        document: &GraphDocument,
        lock: Option<&EngineLock>,
        installed_sdk: Option<&SdkIdentity>,
    ) -> Result<(), String> {
        if document.schema_version != ENGINE_LOCK_DOCUMENT_VERSION {
            return Err(format!(
                "Migration required: document format {}, expected exact format {ENGINE_LOCK_DOCUMENT_VERSION}",
                document.schema_version
            ));
        }
        let lock = lock.ok_or(
            "Format 4 requires an explicit engine lock; normal reads never capture missing pins",
        )?;
        let required = self.required_contracts(document)?;
        for (id, installed) in &required.module_versions {
            match document.module_versions.get(id) {
                None => return Err(format!("Required module '{id}' has no exact version pin")),
                Some(pinned) if pinned != installed => {
                    return Err(format!(
                        "Required module '{id}' pins '{pinned}', installed '{installed}'"
                    ));
                }
                _ => {}
            }
        }
        for (id, installed) in &required.component_schemas {
            match lock.component_schemas.get(id) {
                None => return Err(format!("Required component '{id}' has no schema pin")),
                Some(pinned) if pinned != installed => {
                    return Err(format!(
                        "Required component '{id}' schema owner/version/contract differs from its exact pin"
                    ));
                }
                _ => {}
            }
        }
        for (id, installed) in &required.system_state_schemas {
            match lock.system_state_schemas.get(id) {
                None => return Err(format!("Required system '{id}' has no state schema pin")),
                Some(pinned) if pinned != installed => {
                    return Err(format!(
                        "Required system '{id}' state schema pins '{pinned}', installed '{installed}'"
                    ));
                }
                _ => {}
            }
        }
        if required.requires_sdk {
            let pinned = lock
                .sdk
                .as_ref()
                .ok_or("Active Custom requires an exact SDK identity pin")?;
            pinned.validate()?;
            let installed =
                installed_sdk.ok_or("Active Custom requires a checked installed SDK identity")?;
            installed.validate()?;
            if pinned != installed {
                return Err("Active Custom SDK version/data API differs from its exact pin".into());
            }
        }
        lock.validate_integrity(&document.module_versions)
    }
}

/// SHA-256 of compact JSON with recursively sorted object keys. Array order,
/// defaults, constraints, typed references and every other serialized field are
/// retained. This is integrity, not authentication or a package file inventory.
pub fn canonical_contract_hash<T: Serialize>(payload: &T) -> Result<String, String> {
    fn normalize(value: Value) -> Value {
        match value {
            Value::Object(object) => {
                let sorted = object
                    .into_iter()
                    .map(|(key, value)| (key, normalize(value)))
                    .collect::<BTreeMap<_, _>>();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(values) => Value::Array(values.into_iter().map(normalize).collect()),
            other => other,
        }
    }
    let value = serde_json::to_value(payload).map_err(|error| format!("Contract JSON: {error}"))?;
    let bytes =
        serde_json::to_vec(&normalize(value)).map_err(|error| format!("Contract JSON: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ExecutableSystem, ExecutionGraph, GameProject, ModuleRegistry, PropertySchema,
        PropertyScope, PropertyType, SystemContext, SystemDescriptor, SystemNode, SystemRegistry,
        SystemStage, WorldKind, demo_document,
    };

    struct Noop;
    impl ExecutableSystem for Noop {
        fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            Ok(())
        }
    }

    fn composition(
        default: bool,
        state_schema: &str,
        unrelated_version: &str,
    ) -> CompositionContext {
        let mut systems = SystemRegistry::builtin();
        systems.install_module("fixture.owner", "7.0").unwrap();
        systems
            .install_module("unrelated", unrelated_version)
            .unwrap();
        systems
            .components_mut()
            .register(ComponentSchema {
                id: "fixture.flag".into(),
                owner_module: "fixture.owner".into(),
                version: "data/3".into(),
                label: "Fixture flag".into(),
                defaults: json!({"enabled":default}),
                fields: vec![PropertySchema {
                    id: "fixture.flag.enabled".into(),
                    label: "Enabled".into(),
                    scope: PropertyScope::Component {
                        component: "fixture.flag".into(),
                    },
                    value_type: PropertyType::Boolean,
                    default: json!(default),
                    requires_modules: vec![],
                    requires_components: vec![],
                }],
                requires_modules: vec![],
                requires_components: vec![],
            })
            .unwrap();
        systems
            .register(
                SystemDescriptor {
                    id: "fixture.consumer".into(),
                    state_schema: state_schema.into(),
                    components: vec!["fixture.flag".into()],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(Noop)),
            )
            .unwrap();
        systems
            .set_module_version("fixture.consumer", "4.2")
            .unwrap();
        CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap()
    }

    fn document() -> GraphDocument {
        let mut document = demo_document(WorldKind::Ecs);
        document.schema_version = ENGINE_LOCK_DOCUMENT_VERSION;
        document.root.nodes.retain(|node| node.id == "world");
        document.root.edges.clear();
        let mut game = GameProject::empty();
        game.execution = ExecutionGraph {
            resources: vec![],
            stages: vec![SystemStage {
                name: "Fixture".into(),
                nodes: vec![SystemNode {
                    id: "consumer".into(),
                    module: "fixture.consumer".into(),
                    params: json!({}),
                    bindings: BTreeMap::new(),
                    after: vec![],
                }],
            }],
        };
        document.game = Some(game);
        document
    }

    fn profile() -> ProfileIdentity {
        ProfileIdentity::from_payload("fixture.profile", "1", &json!({"blueprint":"world.ecs"}))
            .unwrap()
    }

    fn sealed(composition: &CompositionContext) -> GraphDocument {
        let mut document = document();
        composition
            .capture_lock(&document, profile(), None)
            .unwrap()
            .apply_to(&mut document);
        document
    }

    fn reseal(document: &mut GraphDocument) {
        let lock = document.engine_lock.as_mut().unwrap();
        lock.lock_fingerprint = lock.fingerprint(&document.module_versions).unwrap();
    }

    #[test]
    fn canonical_sha256_is_sorted_recursive_and_preserves_array_contract_order() {
        assert_eq!(
            canonical_contract_hash(&json!({})).unwrap(),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        let left: Value =
            serde_json::from_str(r#"{"z":{"second":2,"first":1},"a":[1,2]}"#).unwrap();
        let right: Value =
            serde_json::from_str(r#"{"a":[1,2],"z":{"first":1,"second":2}}"#).unwrap();
        assert_eq!(
            canonical_contract_hash(&left).unwrap(),
            canonical_contract_hash(&right).unwrap()
        );
        assert_ne!(
            canonical_contract_hash(&left).unwrap(),
            canonical_contract_hash(&json!({"z":{"first":1,"second":2},"a":[2,1]})).unwrap()
        );
    }

    #[test]
    fn published_profile_fingerprint_requires_its_exact_payload_and_identity() {
        let identity = profile();
        identity
            .validate_payload(&json!({"blueprint":"world.ecs"}))
            .unwrap();
        assert!(
            identity
                .validate_payload(&json!({"blueprint":"world.objects"}))
                .is_err()
        );
        assert_ne!(
            identity,
            ProfileIdentity::from_payload(
                "fixture.profile",
                "2",
                &json!({"blueprint":"world.ecs"})
            )
            .unwrap()
        );
    }

    #[test]
    fn required_contracts_include_unattached_consumed_schema_owner_and_system_state() {
        let catalog = composition(false, "state/1", "1");
        let document = sealed(&catalog);
        assert!(
            document.game.as_ref().unwrap().scenes["empty"]
                .objects
                .is_empty()
        );
        let required = catalog.required_contracts(&document).unwrap();
        assert_eq!(required.module_versions["fixture.owner"], "7.0");
        assert_eq!(required.module_versions["fixture.consumer"], "4.2");
        assert_eq!(required.component_schemas["fixture.flag"].version, "data/3");
        assert_eq!(required.system_state_schemas["fixture.consumer"], "state/1");
        assert!(!required.requires_sdk);
        catalog.validate_lock(&document, None).unwrap();
        assert!(
            composition(true, "state/1", "1")
                .validate_lock(&document, None)
                .is_err()
        );
        assert!(
            composition(false, "state/2", "1")
                .validate_lock(&document, None)
                .is_err()
        );
    }

    #[test]
    fn missing_active_module_component_and_state_pins_reject_without_filling() {
        let composition = composition(false, "state/1", "1");
        let document = sealed(&composition);
        let mut missing = document.clone();
        missing.module_versions.remove("fixture.owner");
        assert!(
            composition
                .validate_lock(&missing, None)
                .unwrap_err()
                .contains("fixture.owner")
        );
        assert!(!missing.module_versions.contains_key("fixture.owner"));
        let mut missing = document.clone();
        missing
            .engine_lock
            .as_mut()
            .unwrap()
            .component_schemas
            .remove("fixture.flag");
        assert!(
            composition
                .validate_lock(&missing, None)
                .unwrap_err()
                .contains("no schema pin")
        );
        let mut missing = document;
        missing
            .engine_lock
            .as_mut()
            .unwrap()
            .system_state_schemas
            .clear();
        assert!(
            composition
                .validate_lock(&missing, None)
                .unwrap_err()
                .contains("no state schema pin")
        );
    }

    #[test]
    fn inactive_archives_survive_catalog_drift_removal_and_explicit_recapture() {
        let first = composition(false, "state/1", "1");
        let mut document = document();
        document
            .module_versions
            .insert("unrelated".into(), "old version".into());
        document
            .module_versions
            .insert("not-installed.archive".into(), "99".into());
        first
            .capture_lock(&document, profile(), None)
            .unwrap()
            .apply_to(&mut document);
        document
            .engine_lock
            .as_mut()
            .unwrap()
            .component_schemas
            .insert(
                "archived.unknown".into(),
                SchemaPin {
                    owner_module: "gone.owner".into(),
                    version: "old".into(),
                    contract_hash: "legacy archived bytes".into(),
                },
            );
        reseal(&mut document);
        composition(false, "state/1", "2")
            .validate_lock(&document, None)
            .unwrap();
        document.game.as_mut().unwrap().execution = ExecutionGraph::default();
        first.validate_lock(&document, None).unwrap();
        let archive = document.engine_lock.as_ref().unwrap().clone();
        first
            .capture_lock(&document, profile(), None)
            .unwrap()
            .apply_to(&mut document);
        assert_eq!(
            document.engine_lock.as_ref().unwrap().component_schemas,
            archive.component_schemas
        );
        assert_eq!(
            document.engine_lock.as_ref().unwrap().system_state_schemas,
            archive.system_state_schemas
        );
        assert_eq!(document.module_versions["not-installed.archive"], "99");
        assert_eq!(document.module_versions["unrelated"], "old version");
    }

    #[test]
    fn ordinary_serialization_preserves_old_contracts_and_only_explicit_update_replaces_them() {
        let original_catalog = composition(false, "state/1", "1");
        let mut original = document();
        original
            .module_versions
            .insert("unknown.archived".into(), "previous".into());
        original
            .legacy_archive
            .insert("unknown.owner".into(), json!({"authored":[null,1,false]}));
        original_catalog
            .capture_lock(&original, profile(), None)
            .unwrap()
            .apply_to(&mut original);
        let saved = serde_json::to_vec(&original).unwrap();
        let reopened: GraphDocument = serde_json::from_slice(&saved).unwrap();
        assert_eq!(reopened.engine_lock, original.engine_lock);
        assert_eq!(reopened.module_versions, original.module_versions);

        let mut updated_catalog = composition(true, "state/2", "2");
        updated_catalog
            .modules
            .set_module_version("fixture.owner", "8.0")
            .unwrap();
        updated_catalog
            .systems
            .set_module_version("fixture.owner", "8.0")
            .unwrap();
        assert!(
            updated_catalog
                .validate_lock(&reopened, None)
                .unwrap_err()
                .contains("fixture.owner")
        );
        assert_eq!(
            serde_json::to_vec(&reopened).unwrap(),
            saved,
            "ordinary read/validation/serialization cannot silently capture installed contracts"
        );
        let replacement_profile = ProfileIdentity::from_payload(
            "fixture.profile",
            "2",
            &json!({"blueprint":"world.ecs"}),
        )
        .unwrap();
        let candidate = updated_catalog
            .capture_lock(&reopened, replacement_profile.clone(), None)
            .unwrap();
        assert_eq!(
            serde_json::to_vec(&reopened).unwrap(),
            saved,
            "preparing an explicit update must remain reviewable before publication"
        );
        let mut updated = reopened;
        candidate.apply_to(&mut updated);
        updated_catalog.validate_lock(&updated, None).unwrap();
        assert_eq!(updated.module_versions["fixture.owner"], "8.0");
        assert_eq!(
            updated.engine_lock.as_ref().unwrap().profile,
            replacement_profile
        );
        assert_eq!(
            updated.engine_lock.as_ref().unwrap().system_state_schemas["fixture.consumer"],
            "state/2"
        );
        assert_ne!(
            updated.engine_lock.as_ref().unwrap().component_schemas["fixture.flag"],
            original.engine_lock.as_ref().unwrap().component_schemas["fixture.flag"]
        );
        assert_eq!(updated.module_versions["unknown.archived"], "previous");
        assert_eq!(updated.legacy_archive, original.legacy_archive);
        assert_eq!(
            updated.game, original.game,
            "lock capture is metadata publication, not a semantic data migration"
        );
        let roundtrip: GraphDocument =
            serde_json::from_slice(&serde_json::to_vec(&updated).unwrap()).unwrap();
        updated_catalog.validate_lock(&roundtrip, None).unwrap();
    }

    #[test]
    fn removed_consumers_archive_schema_state_and_owner_pins_until_reactivated() {
        let original_catalog = composition(false, "state/1", "1");
        let mut document = sealed(&original_catalog);
        let execution = document.game.as_ref().unwrap().execution.clone();
        let captured = document.engine_lock.clone();
        let captured_versions = document.module_versions.clone();
        document.game.as_mut().unwrap().execution = ExecutionGraph::default();
        let mut updated_catalog = composition(true, "state/2", "2");
        updated_catalog
            .modules
            .set_module_version("fixture.owner", "8.0")
            .unwrap();
        updated_catalog
            .systems
            .set_module_version("fixture.owner", "8.0")
            .unwrap();
        updated_catalog.validate_lock(&document, None).unwrap();
        assert_eq!(document.engine_lock, captured);
        assert_eq!(document.module_versions, captured_versions);
        document.game.as_mut().unwrap().execution = execution;
        assert!(updated_catalog.validate_lock(&document, None).is_err());
        assert_eq!(
            document.engine_lock, captured,
            "activation rejects stale contracts without repinning them"
        );
        assert_eq!(document.module_versions, captured_versions);
    }

    #[test]
    fn activation_requires_exact_saved_contracts_and_does_not_reseal_payload_changes() {
        let composition = composition(false, "state/1", "1");
        let mut document = sealed(&composition);
        document.engine.enabled_modules.insert("unrelated".into());
        assert!(
            composition
                .validate_lock(&document, None)
                .unwrap_err()
                .contains("unrelated")
        );
        document.engine.enabled_modules.clear();
        document
            .module_versions
            .insert("new.inactive.archive".into(), "1".into());
        assert!(
            composition
                .validate_lock(&document, None)
                .unwrap_err()
                .contains("fingerprint")
        );
    }

    #[test]
    fn exact_v4_and_required_sdk_identity_are_enforced_before_execution() {
        let composition =
            CompositionContext::new(ModuleRegistry::builtin(), SystemRegistry::builtin()).unwrap();
        let mut document = demo_document(WorldKind::Ecs);
        document.schema_version = ENGINE_LOCK_DOCUMENT_VERSION;
        assert!(
            composition
                .required_contracts(&document)
                .unwrap()
                .requires_sdk
        );
        let sdk = SdkIdentity::current();
        composition
            .capture_lock(&document, profile(), Some(sdk.clone()))
            .unwrap()
            .apply_to(&mut document);
        composition.validate_lock(&document, Some(&sdk)).unwrap();
        assert!(
            composition
                .validate_lock(&document, None)
                .unwrap_err()
                .contains("installed SDK")
        );
        let mut wrong = sdk.clone();
        wrong.data_api_fingerprint = "module-engine-data-api/v1".into();
        assert!(composition.validate_lock(&document, Some(&wrong)).is_err());
        document.engine_lock.as_mut().unwrap().sdk = None;
        assert!(
            composition
                .validate_lock(&document, Some(&sdk))
                .unwrap_err()
                .contains("SDK identity pin")
        );
        document.schema_version = 3;
        assert!(
            composition
                .validate_lock(&document, Some(&sdk))
                .unwrap_err()
                .contains("Migration required")
        );
        document.schema_version = 4;
        document.engine_lock = None;
        assert!(
            composition
                .validate_lock(&document, Some(&sdk))
                .unwrap_err()
                .contains("requires an explicit engine lock")
        );
    }
}
