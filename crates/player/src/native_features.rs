//! Composition-root metadata for native package selection. No game rules live here.
use module_engine_core::{CompositionContext, GraphDocument, ModuleRegistry, SystemRegistry};
use std::collections::{BTreeMap, BTreeSet};

pub const KNOWN_NATIVE_FEATURES: &[&str] = &[
    "ui",
    "arpg",
    "puzzle",
    "g1",
    "vulkan",
    "audio",
    "persistence",
];

/// A source installer declares which native feature owns its contracts. A base
/// contract is always compiled; a missing mapping is an error, never a default.
#[derive(Clone, Default)]
pub struct NativeFeatureCatalog {
    modules: BTreeMap<String, Option<String>>,
    presentations: BTreeMap<String, String>,
    services: BTreeMap<String, String>,
}
impl NativeFeatureCatalog {
    /// Filter a composition's already-installed contracts by known source-game
    /// ownership. The composition proves availability; this catalog supplies
    /// native-package ownership, which need not cover separately installed
    /// source extensions. Export still rejects any unmapped native owner.
    pub fn module_available_in_profile(&self, id: &str, selected: Option<&str>) -> bool {
        match self.modules.get(id) {
            Some(Some(feature)) if self.presentations.values().any(|owner| owner == feature) => {
                selected == Some(feature.as_str())
            }
            _ => true,
        }
    }
    pub fn register_base(
        &mut self,
        modules: impl IntoIterator<Item = String>,
    ) -> Result<(), String> {
        let mut candidate = self.clone();
        for id in modules {
            if id.trim().is_empty() || candidate.modules.get(&id).is_some_and(Option::is_some) {
                return Err(format!(
                    "Native base contract has invalid or conflicting owner: {id}"
                ));
            }
            candidate.modules.insert(id, None);
        }
        *self = candidate;
        Ok(())
    }
    pub fn register(
        &mut self,
        feature: &str,
        modules: impl IntoIterator<Item = String>,
        presentations: impl IntoIterator<Item = String>,
        services: impl IntoIterator<Item = String>,
    ) -> Result<(), String> {
        if feature.is_empty()
            || !feature
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err("Native feature needs a portable lowercase identifier".into());
        }
        let mut candidate = self.clone();
        for (kind, ids) in [
            ("module", modules.into_iter().collect::<Vec<_>>()),
            ("presentation", presentations.into_iter().collect()),
            ("service", services.into_iter().collect()),
        ] {
            for id in ids {
                if id.trim().is_empty() {
                    return Err(format!("Native {kind} needs a nonempty contract ID"));
                }
                let conflict = match kind {
                    "module" => candidate
                        .modules
                        .insert(id.clone(), Some(feature.into()))
                        .is_some(),
                    "presentation" => candidate
                        .presentations
                        .insert(id.clone(), feature.into())
                        .is_some(),
                    _ => candidate
                        .services
                        .insert(id.clone(), feature.into())
                        .is_some(),
                };
                if conflict {
                    return Err(format!("Native {kind} has multiple owners: {id}"));
                }
            }
        }
        *self = candidate;
        Ok(())
    }
    pub fn select(
        &self,
        modules: impl IntoIterator<Item = String>,
        presentations: impl IntoIterator<Item = String>,
        services: impl IntoIterator<Item = String>,
    ) -> Result<BTreeSet<String>, String> {
        let mut result = BTreeSet::from(["ui".into()]);
        for id in modules {
            match self.modules.get(&id) {
                Some(Some(feature)) => {
                    result.insert(feature.clone());
                }
                Some(None) => {}
                None => {
                    return Err(format!(
                        "Player has no compiled feature mapping for module '{id}'"
                    ));
                }
            }
        }
        for (kind, ids, mapping) in [
            (
                "presentation",
                presentations.into_iter().collect::<Vec<_>>(),
                &self.presentations,
            ),
            ("service", services.into_iter().collect(), &self.services),
        ] {
            for id in ids {
                result.insert(
                    mapping
                        .get(&id)
                        .ok_or_else(|| {
                            format!("Player has no compiled feature mapping for {kind} '{id}'")
                        })?
                        .clone(),
                );
            }
        }
        Ok(result)
    }
    pub fn select_document(
        &self,
        document: &GraphDocument,
        composition: &CompositionContext,
    ) -> Result<BTreeSet<String>, String> {
        let required = composition.required_contracts(document)?;
        let mut presentations = Vec::new();
        let mut services = Vec::new();
        if let Some(game) = &document.game {
            for contribution in composition
                .systems
                .active_game_contributions(&game.execution)
            {
                presentations.push(contribution.id.clone());
                services.extend(
                    (contribution.service_requests)(game)?
                        .into_iter()
                        .map(|request| request.id),
                );
            }
        }
        self.select(
            required.module_versions.into_keys(),
            presentations,
            services,
        )
    }
}

/// Specific source packages are installed here, rather than in generic exporters.
pub fn installed_native_features() -> Result<NativeFeatureCatalog, String> {
    let mut catalog = NativeFeatureCatalog::default();
    let mut base = SystemRegistry::builtin();
    crate::source_modules::install(&mut base)?;
    catalog.register_base(base.module_versions().keys().cloned())?;
    catalog.register_base(
        ModuleRegistry::builtin()
            .descriptors()
            .iter()
            .map(|m| m.id.clone()),
    )?;
    catalog.register_base(["custom.rust.update".into()])?;
    let builtin = SystemRegistry::builtin();
    #[cfg(feature = "arpg")]
    catalog.register(
        "arpg",
        module_engine_arpg::registry()
            .module_versions()
            .keys()
            .filter(|id| builtin.module_version(id).is_none())
            .cloned(),
        ["arpg.presentation".into()],
        [],
    )?;
    #[cfg(feature = "puzzle")]
    catalog.register(
        "puzzle",
        module_engine_project_puzzle::registry()
            .module_versions()
            .keys()
            .filter(|id| builtin.module_version(id).is_none())
            .cloned(),
        ["puzzle.presentation".into()],
        [],
    )?;
    #[cfg(feature = "g1")]
    catalog.register(
        "g1",
        module_engine_g1::registry()
            .module_versions()
            .keys()
            .filter(|id| builtin.module_version(id).is_none())
            .cloned(),
        ["g1.presentation".into()],
        [],
    )?;
    #[cfg(not(any(feature = "arpg", feature = "puzzle", feature = "g1")))]
    let _ = builtin;
    #[cfg(feature = "vulkan")]
    catalog.register(
        "vulkan",
        std::iter::once("render.vulkan".into()).chain(
            module_engine_vulkan::module_descriptors()
                .into_iter()
                .map(|descriptor| descriptor.id),
        ),
        [],
        [],
    )?;
    #[cfg(feature = "audio")]
    catalog.register(
        "audio",
        [],
        [],
        [module_engine_host_services::WAV_SERVICE_ID.into()],
    )?;
    #[cfg(feature = "persistence")]
    catalog.register(
        "persistence",
        [],
        [],
        [module_engine_host_services::PERSISTENCE_SERVICE_ID.into()],
    )?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_profile_filters_by_registered_ownership_instead_of_game_names() {
        let mut catalog = NativeFeatureCatalog::default();
        catalog.register_base(["world.ecs".into()]).unwrap();
        catalog
            .register(
                "strategy",
                ["tactics.orders".into()],
                ["tactics.hud".into()],
                [],
            )
            .unwrap();
        catalog
            .register("renderer", ["renderer.forward".into()], [], [])
            .unwrap();
        assert!(catalog.module_available_in_profile("world.ecs", None));
        assert!(catalog.module_available_in_profile("renderer.forward", Some("strategy")));
        assert!(catalog.module_available_in_profile("tactics.orders", Some("strategy")));
        assert!(!catalog.module_available_in_profile("tactics.orders", None));
        assert!(!catalog.module_available_in_profile("tactics.orders", Some("other")));
        assert!(catalog.module_available_in_profile("additional.installed.source", None));
        assert!(
            catalog
                .select(["additional.installed.source".into()], [], [])
                .is_err()
        );
    }
    #[cfg(feature = "g1")]
    #[test]
    fn g1_selects_only_its_explicit_source_and_requested_services() {
        let catalog = installed_native_features().unwrap();
        assert_eq!(
            catalog
                .select(["g1.rules".into()], ["g1.presentation".into()], [])
                .unwrap(),
            BTreeSet::from(["ui".into(), "g1".into()])
        );
    }
    #[cfg(not(feature = "g1"))]
    #[test]
    fn omitted_g1_cannot_activate_by_contract_id() {
        assert!(
            installed_native_features()
                .unwrap()
                .select(["g1.rules".into()], [], [])
                .is_err()
        );
    }
    #[cfg(feature = "vulkan")]
    #[test]
    fn nested_gpu_pass_contracts_select_their_registered_renderer_package() {
        let catalog = installed_native_features().unwrap();
        assert_eq!(
            catalog.select(["gpu.color_encode".into()], [], []).unwrap(),
            BTreeSet::from(["ui".into(), "vulkan".into()])
        );
    }
    #[cfg(not(feature = "vulkan"))]
    #[test]
    fn absent_renderer_package_cannot_activate_nested_passes_implicitly() {
        assert!(
            installed_native_features()
                .unwrap()
                .select(["gpu.color_encode".into()], [], [])
                .is_err()
        );
    }
    #[test]
    fn independent_racing_package_and_services_select_only_their_owners() {
        let mut catalog = NativeFeatureCatalog::default();
        catalog.register_base(["world".into()]).unwrap();
        catalog
            .register("racing", ["race.laps".into()], ["race.hud".into()], [])
            .unwrap();
        catalog
            .register("storage", [], [], ["profile.store".into()])
            .unwrap();
        assert_eq!(
            catalog.select(["world".into()], [], []).unwrap(),
            BTreeSet::from(["ui".into()])
        );
        assert_eq!(
            catalog
                .select(
                    ["race.laps".into()],
                    ["race.hud".into()],
                    ["profile.store".into()]
                )
                .unwrap(),
            BTreeSet::from(["ui".into(), "racing".into(), "storage".into()])
        );
        assert!(catalog.select([], [], ["missing".into()]).is_err());
        assert!(catalog.select(["unmapped".into()], [], []).is_err());
    }
    #[test]
    fn conflicting_registration_is_atomic_and_cannot_steal_contracts() {
        let mut catalog = NativeFeatureCatalog::default();
        catalog.register("one", ["owned".into()], [], []).unwrap();
        assert!(
            catalog
                .register("two", ["new".into(), "owned".into()], [], [])
                .is_err()
        );
        assert!(catalog.select(["new".into()], [], []).is_err());
        assert_eq!(
            catalog.select(["owned".into()], [], []).unwrap(),
            BTreeSet::from(["ui".into(), "one".into()])
        );
    }
}
