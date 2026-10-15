//! Explicit Engine selection, contract publication and legacy copy preparation.
use module_engine_core::*;
use module_engine_project::{EngineBlueprint, EngineProfile};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProfileKind {
    #[default]
    Neutral,
    Arpg,
    Puzzle,
}
impl ProfileKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Neutral => "module-engine.neutral",
            Self::Arpg => "module-engine.arpg",
            Self::Puzzle => "module-engine.puzzle",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Neutral => "Neutral",
            Self::Arpg => "ARPG",
            Self::Puzzle => "Puzzle",
        }
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "neutral" => Ok(Self::Neutral),
            "arpg" => Ok(Self::Arpg),
            "puzzle" => Ok(Self::Puzzle),
            _ => Err("Engine profile must be neutral, arpg or puzzle".into()),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineChoice {
    #[default]
    InheritCurrent,
    Selected(ProfileKind),
}

fn root_source_ids(graph: &Graph, ids: &mut BTreeSet<String>) {
    for node in &graph.nodes {
        if matches!(node.module.as_str(), "custom.rust" | "custom.rust.update")
            && let Some(id) = node.params.get("source_id").and_then(Value::as_str)
        {
            ids.insert(id.into());
        }
        if let Some(graph) = &node.subgraph {
            root_source_ids(graph, ids);
        }
    }
}
pub fn engine_sources(document: &GraphDocument) -> BTreeMap<String, String> {
    let mut ids = BTreeSet::new();
    root_source_ids(&document.root, &mut ids);
    document
        .custom_sources
        .iter()
        .filter(|(id, _)| ids.contains(*id))
        .map(|(id, source)| (id.clone(), source.clone()))
        .collect()
}
fn package_modules(registry: SystemRegistry) -> BTreeSet<String> {
    let builtin = SystemRegistry::builtin();
    registry
        .module_versions()
        .iter()
        .filter(|(id, _)| builtin.module_version(id).is_none())
        .map(|(id, _)| id.clone())
        .collect()
}
pub fn from_blueprint(
    document: &GraphDocument,
    composition: &CompositionContext,
    kind: ProfileKind,
) -> Result<EngineProfile, String> {
    let arpg = package_modules(module_engine_arpg::registry());
    let puzzle = package_modules(module_engine_project_puzzle::registry());
    let available_modules = composition
        .module_versions()
        .into_keys()
        .filter(|id| {
            (kind == ProfileKind::Arpg || !arpg.contains(id))
                && (kind == ProfileKind::Puzzle || !puzzle.contains(id))
        })
        .collect();
    let mut ids = BTreeSet::new();
    root_source_ids(&document.root, &mut ids);
    let custom_sources = document
        .custom_sources
        .iter()
        .filter(|(id, _)| ids.contains(*id))
        .map(|(id, source)| (id.clone(), source.clone()))
        .collect();
    EngineProfile::new(
        kind.id(),
        "1",
        EngineBlueprint {
            root: document.root.clone(),
            engine: document.engine.clone(),
            custom_sources,
            shader_sources: document.shader_sources.clone(),
            // Genre rules belong to the chosen project. Empty always has an
            // explicitly published neutral schedule, including with an ARPG SDK.
            default_execution: GameProject::empty().execution,
            available_modules,
        },
    )
}
pub fn attach(
    document: &mut GraphDocument,
    composition: &CompositionContext,
    profile: &EngineProfile,
) -> Result<(), String> {
    // New/profile publication explicitly approves the installed available module
    // library. Connecting an already offered GPU pass needs no hidden repinning.
    for id in &profile.blueprint.available_modules {
        if let Some(version) = composition.module_version(id) {
            document
                .module_versions
                .entry(id.clone())
                .or_insert_with(|| version.into());
        }
    }
    document.schema_version = SCHEMA_VERSION;
    document.engine_profile = Some(profile.document_value()?);
    composition
        .capture_lock(
            document,
            profile.identity.clone(),
            Some(SdkIdentity::current()),
        )?
        .apply_to(document);
    profile.validate_with_sdk(document, composition, Some(&SdkIdentity::current()))
}
pub fn finish_template(
    document: &mut GraphDocument,
    kind: ProfileKind,
    backend: &Path,
) -> Result<(), String> {
    let composition = crate::host::composition_at(backend)?;
    let profile = from_blueprint(document, &composition, kind)?;
    attach(document, &composition, &profile)
}
pub fn select(kind: ProfileKind, backend: &Path) -> Result<EngineProfile, String> {
    let mut document =
        crate::host::gpu_document(WorldKind::Ecs, module_engine_vulkan::RenderPath::Forward);
    document.root.nodes.retain(|node| {
        !matches!(
            node.module.as_str(),
            "custom.rust" | "custom.rust.update" | "scalar.value" | "clock.time"
        )
    });
    let ids = document
        .root
        .nodes
        .iter()
        .map(|n| n.id.clone())
        .collect::<BTreeSet<_>>();
    document
        .root
        .edges
        .retain(|e| ids.contains(&e.from_node) && ids.contains(&e.to_node));
    document.custom_sources.clear();
    crate::host::ensure_authoring_configuration(&mut document);
    from_blueprint(&document, &crate::host::composition_at(backend)?, kind)
}
/// Explicit user operation, separate from ordinary Save/Play/export. Existing
/// inactive pins survive; only required installed contracts are approved anew.
pub fn update_contracts(document: &GraphDocument, backend: &Path) -> Result<GraphDocument, String> {
    let composition = crate::host::composition_at(backend)?;
    let previous = EngineProfile::from_document(document)?;
    let mut blueprint = previous.blueprint;
    blueprint.root = document.root.clone();
    blueprint.engine = document.engine.clone();
    blueprint.custom_sources = engine_sources(document);
    blueprint.shader_sources = document.shader_sources.clone();
    blueprint.available_modules.extend(
        composition
            .required_contracts(document)?
            .module_versions
            .into_keys(),
    );
    let version = previous
        .identity
        .version
        .parse::<u64>()
        .ok()
        .and_then(|v| v.checked_add(1))
        .map_or_else(
            || format!("{}.1", previous.identity.version),
            |v| v.to_string(),
        );
    let profile = EngineProfile::new(previous.identity.id, version, blueprint)?;
    let mut candidate = document.clone();
    attach(&mut candidate, &composition, &profile)?;
    Ok(candidate)
}
pub fn validate(document: &GraphDocument, backend: &Path) -> Result<(), String> {
    let composition = crate::host::composition_at(backend)?;
    EngineProfile::from_document(document)?.validate_with_sdk(
        document,
        &composition,
        Some(&SdkIdentity::current()),
    )
}

/// A checked snapshot for the explicitly supported source→target adapters.
/// It is generated once during engine development and shipped as reviewed data.
pub fn known_migration_target() -> Result<module_engine_project::KnownMigrationTarget, String> {
    serde_json::from_str(include_str!("../../../assets/engine-contracts/v0.4.0.json"))
        .map_err(|e| format!("Shipped migration contracts: {e}"))
}
pub fn migration_candidate(
    raw: &[u8],
    kind: ProfileKind,
    backend: &Path,
) -> Result<module_engine_project::MigrationCandidate, String> {
    let composition = crate::host::composition_at(backend)?;
    let profile = select(kind, backend)?;
    let mut candidate = module_engine_project::prepare_legacy_migration(
        raw,
        &composition,
        profile.identity.clone(),
        SdkIdentity::current(),
        &known_migration_target()?,
    )?;
    candidate
        .document
        .legacy_archive
        .extend(candidate.legacy_archive.clone());
    let target = known_migration_target()?;
    for id in &profile.blueprint.available_modules {
        if !candidate.document.module_versions.contains_key(id)
            && let Some(version) = target.module_versions.get(id)
            && composition.module_version(id) == Some(version.as_str())
        {
            candidate
                .document
                .module_versions
                .insert(id.clone(), version.clone());
            candidate.report.changes.push(module_engine_project::MigrationChange {path:format!("/module_versions/{id}"),description:"Explicit migration approves an available shipped library module at the audited target version".into()});
        }
    }
    composition
        .capture_lock(
            &candidate.document,
            profile.identity.clone(),
            Some(SdkIdentity::current()),
        )?
        .apply_to(&mut candidate.document);
    candidate.document.engine_profile = Some(profile.document_value()?);
    profile.validate_with_sdk(
        &candidate.document,
        &composition,
        Some(&SdkIdentity::current()),
    )?;
    Ok(candidate)
}

pub fn inspected_contracts(
    backend: &Path,
) -> Result<module_engine_project::KnownMigrationTarget, String> {
    let composition = crate::host::composition_at(backend)?;
    let module_versions = composition.module_versions();
    let component_schemas = composition
        .systems
        .components()
        .schemas()
        .map(|s| Ok((s.id.clone(), SchemaPin::capture(s)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let system_state_schemas = composition
        .systems
        .descriptors()
        .map(|s| (s.id.clone(), s.state_schema.clone()))
        .collect();
    Ok(module_engine_project::KnownMigrationTarget {
        module_versions,
        component_schemas,
        system_state_schemas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn backend() -> std::path::PathBuf {
        crate::project::source_root()
    }
    #[test]
    fn inherited_empty_has_explicit_neutral_schedule_without_arpg_controller() {
        let base = crate::game_templates::demo_document();
        let distribution =
            crate::distribution::detect(&backend().join("project.json"), &base, None).unwrap();
        let empty = crate::workspace_files::template_document_with_choice(
            &base,
            crate::workspace_files::ProjectTemplate::Empty,
            &distribution,
            EngineChoice::InheritCurrent,
        )
        .unwrap();
        assert!(
            empty
                .game
                .as_ref()
                .unwrap()
                .scenes
                .values()
                .all(|scene| scene.objects.is_empty())
        );
        assert!(
            empty
                .custom_sources
                .keys()
                .all(|id| id != "game-controller")
        );
        assert_eq!(
            EngineProfile::from_document(&empty).unwrap().identity,
            EngineProfile::from_document(&base).unwrap().identity
        );
        let activity = crate::host::composition_at(&backend())
            .unwrap()
            .activity(&empty)
            .unwrap();
        assert!(activity.modules.iter().all(|id| !id.starts_with("game.")));
        validate(&empty, &backend()).unwrap();
    }
    #[test]
    fn explicitly_selected_puzzle_engine_runs_its_content_and_rejects_arpg() {
        let base = crate::game_templates::demo_document();
        let distribution =
            crate::distribution::detect(&backend().join("project.json"), &base, None).unwrap();
        let puzzle = crate::workspace_files::template_document_with_choice(
            &base,
            crate::workspace_files::ProjectTemplate::Puzzle,
            &distribution,
            EngineChoice::Selected(ProfileKind::Puzzle),
        )
        .unwrap();
        assert_eq!(
            EngineProfile::from_document(&puzzle).unwrap().identity.id,
            "module-engine.puzzle"
        );
        validate(&puzzle, &backend()).unwrap();
        assert!(
            crate::workspace_files::template_document_with_choice(
                &base,
                crate::workspace_files::ProjectTemplate::TwoArenas,
                &distribution,
                EngineChoice::Selected(ProfileKind::Puzzle)
            )
            .is_err()
        );
    }
    #[test]
    fn normal_validation_cannot_repair_pins_or_publish_profile() {
        let mut document = crate::game_templates::demo_document();
        document.module_versions.remove("world.ecs");
        let before = serde_json::to_vec(&document).unwrap();
        assert!(validate(&document, &backend()).is_err());
        assert_eq!(serde_json::to_vec(&document).unwrap(), before);
        let published = update_contracts(&document, &backend()).unwrap();
        assert_ne!(
            EngineProfile::from_document(&published).unwrap().identity,
            EngineProfile::from_document(&document).unwrap().identity
        );
        validate(&published, &backend()).unwrap();
        assert_eq!(serde_json::to_vec(&document).unwrap(), before);
    }
    #[test]
    fn wrong_profile_payload_rejects_before_save_without_touching_original_bytes() {
        let mut document = crate::game_templates::demo_document();
        document.engine_profile.as_mut().unwrap()["blueprint"]["available_modules"] =
            serde_json::json!([]);
        let root = backend().join(".work/profile-rejection-test");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("author.json");
        std::fs::write(&path, b"original author bytes").unwrap();
        assert!(crate::project::save(&path, &document).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"original author bytes");
    }
}
