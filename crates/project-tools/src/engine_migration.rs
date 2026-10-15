//! Explicit, in-memory adapters for known format 1/2/3 documents. Preparing a
//! candidate performs no IO, simulation, compilation, or publication. A caller
//! must attach `legacy_archive` to the format-4 document before saving the copy,
//! then check package assets and compile active Custom against the selected SDK.
//!
//! Supported fields use the shared typed-record shapes known across formats
//! 1/2/3, rather than a historical field-introduction table for every format.
//! Unknown names are archived; malformed values under a supported name reject.

use module_engine_core::{
    CompositionContext, GraphDocument, ProfileIdentity, RequiredContracts, SchemaPin, SdkIdentity,
    Severity,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const TARGET_FORMAT: u32 = 4;

/// An audited, shipped migration path's target contracts. Do not construct this
/// from the mutable installed catalog merely to make its comparisons succeed.
/// Only required contracts are compared: unrelated inactive pins remain archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownMigrationTarget {
    pub module_versions: BTreeMap<String, String>,
    pub component_schemas: BTreeMap<String, SchemaPin>,
    pub system_state_schemas: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationChange {
    /// RFC 6901 JSON pointer in the source, or the newly added target field.
    pub path: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationReport {
    pub source_schema: u32,
    pub target_schema: u32,
    /// Hash of the original bytes, including any UTF-8 BOM and trailing newline.
    pub source_sha256: String,
    pub changes: Vec<MigrationChange>,
}

#[derive(Clone, Debug)]
pub struct MigrationCandidate {
    pub document: GraphDocument,
    /// Unknown names at typed record boundaries, keyed by source JSON pointer.
    /// Values in components/params/other Value-backed fields stay in the document.
    /// Publication must retain this archive in GraphDocument.legacy_archive.
    pub legacy_archive: BTreeMap<String, Value>,
    pub report: MigrationReport,
}

/// Inspect known legacy fields solely to resolve the source package/locator and
/// its policy. Unknown typed-record names are omitted from this temporary view;
/// preparing the actual migration must reread the original raw bytes so those
/// values are preserved in the candidate archive. No format, pin or boss data
/// is migrated, and no installation/contract compatibility is inferred here.
pub fn inspect_legacy_document(raw: &[u8]) -> Result<GraphDocument, String> {
    let mut value = parse_source(raw)?;
    legacy_source_schema(&value)?;
    archive_record(&mut value, Record::Document, "", &mut BTreeMap::new())?;
    serde_json::from_value(value)
        .map_err(|error| format!("Malformed legacy document inspection: {error}"))
}

fn legacy_source_schema(value: &Value) -> Result<u32, String> {
    let source_schema = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or("Legacy document needs an integer schema_version")?;
    if !(1..=3).contains(&source_schema) {
        return Err(format!(
            "Explicit legacy migration supports formats 1, 2 and 3; found {source_schema}"
        ));
    }
    Ok(source_schema)
}

pub fn prepare_legacy_migration(
    raw: &[u8],
    composition: &CompositionContext,
    profile: ProfileIdentity,
    sdk: SdkIdentity,
    target: &KnownMigrationTarget,
) -> Result<MigrationCandidate, String> {
    let mut value = parse_source(raw)?;
    let source_schema = legacy_source_schema(&value)?;
    profile.validate()?;
    sdk.validate()?;
    let mut report = MigrationReport {
        source_schema,
        target_schema: TARGET_FORMAT,
        source_sha256: format!("{:x}", Sha256::digest(raw)),
        changes: vec![],
    };
    let mut legacy_archive = BTreeMap::new();
    archive_record(&mut value, Record::Document, "", &mut legacy_archive)?;
    for path in legacy_archive.keys() {
        change(
            &mut report,
            path,
            "Preserved unknown legacy field in legacy_archive",
        );
    }
    // The source field names/types are checked before any semantic adapter. In
    // particular, malformed known fields cannot be hidden in the archive.
    let source_document: GraphDocument = serde_json::from_value(value.clone())
        .map_err(|error| format!("Malformed legacy document: {error}"))?;
    let required = composition.required_contracts(&source_document)?;
    validate_source_contracts(source_schema, &source_document, &required, &mut report)?;
    validate_target_contracts(&required, target)?;
    let known_boss_source = source_schema == 3
        && source_document
            .module_versions
            .get("game.health")
            .is_none_or(|version| version == "0.4.0");
    if known_boss_source {
        migrate_boss_defaults(&mut value, source_schema, &mut report)?;
    } else if source_document.game.as_ref().is_some_and(|game| {
        game.prefabs
            .values()
            .any(|prefab| prefab.components.contains_key("boss"))
            || game
                .scenes
                .values()
                .flat_map(|scene| &scene.objects)
                .any(|object| {
                    object.components.contains_key("boss")
                        || object
                            .overrides
                            .keys()
                            .any(|path| path.starts_with("boss."))
                })
    }) {
        // An active unknown owner was already rejected above. Inactive data
        // under an unknown source contract must remain opaque, even when its
        // component ID happens to match the supported ARPG contract's ID.
        change(
            &mut report,
            "/game",
            "Preserved inactive boss data without interpretation; no known source game.health contract",
        );
    }
    let mut document: GraphDocument = serde_json::from_value(value)
        .map_err(|error| format!("Malformed migrated document: {error}"))?;

    document.schema_version = TARGET_FORMAT;
    change(
        &mut report,
        "/schema_version",
        &format!("Explicitly migrated format {source_schema} to {TARGET_FORMAT}"),
    );
    let before_pins = document.module_versions.clone();
    composition
        .capture_lock(&document, profile, Some(sdk.clone()))?
        .apply_to(&mut document);
    for (id, version) in &required.module_versions {
        if before_pins.get(id) != Some(version) {
            change(
                &mut report,
                &pointer("/module_versions", id),
                &format!("Applied audited target implementation pin '{version}'"),
            );
        }
    }
    change(
        &mut report,
        "/engine_lock",
        "Captured selected profile, active schema/state contracts and exact SDK identity",
    );
    composition.validate_lock(&document, Some(&sdk))?;
    let errors = composition
        .validate_document(&document)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.message)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(format!(
            "Migrated candidate is invalid: {}",
            errors.join("; ")
        ));
    }
    Ok(MigrationCandidate {
        document,
        legacy_archive,
        report,
    })
}

fn validate_target_contracts(
    required: &RequiredContracts,
    target: &KnownMigrationTarget,
) -> Result<(), String> {
    for (id, installed) in &required.module_versions {
        if target.module_versions.get(id) != Some(installed) {
            return Err(format!(
                "No audited migration target for active module '{id}' at '{installed}'"
            ));
        }
    }
    for (id, installed) in &required.component_schemas {
        if target.component_schemas.get(id) != Some(installed) {
            return Err(format!(
                "Active component '{id}' differs from the audited target owner/version/contract"
            ));
        }
    }
    for (id, installed) in &required.system_state_schemas {
        if target.system_state_schemas.get(id) != Some(installed) {
            return Err(format!(
                "Active system '{id}' differs from the audited target state schema"
            ));
        }
    }
    Ok(())
}

fn validate_source_contracts(
    source: u32,
    document: &GraphDocument,
    required: &RequiredContracts,
    report: &mut MigrationReport,
) -> Result<(), String> {
    for id in required.module_versions.keys() {
        let core = matches!(
            id.as_str(),
            "world.objects"
                | "world.ecs"
                | "scene.extract"
                | "viewport.output"
                | "render.software3d"
                | "render.rasterize"
                | "clock.time"
                | "scalar.value"
                | "custom.rust"
                | "custom.rust.update"
                | "interface.input"
                | "interface.output"
                | "game.motion"
        );
        let gpu = matches!(
            id.as_str(),
            "render.vulkan"
                | "gpu.forward"
                | "gpu.gbuffer"
                | "gpu.depth_normal"
                | "gpu.deferred"
                | "gpu.ssao"
                | "gpu.tonemap"
                | "gpu.fxaa"
                | "gpu.color_encode"
                | "gpu.custom"
        );
        let game = matches!(
            id.as_str(),
            "game.input"
                | "game.ai"
                | "game.movement"
                | "game.collision"
                | "game.combat"
                | "game.health"
                | "game.portals"
                | "game.audio"
                | "puzzle.turn"
                | "puzzle.result"
                | "example.spin"
        );
        if !(core || (source >= 2 && gpu) || (source == 3 && game)) {
            return Err(format!(
                "No known format-{source} source migration contract for active module '{id}'"
            ));
        }
        if let Some(version) = document.module_versions.get(id) {
            let known = match source {
                // Format 1 shipped without an implementation pin catalog.
                1 => false,
                2 => version == "0.2.0",
                3 if core || gpu => matches!(version.as_str(), "0.3.0" | "0.4.0"),
                3 => version == "0.4.0",
                _ => false,
            };
            if !known {
                return Err(format!(
                    "No known format-{source} migration for active module '{id}' version '{version}'"
                ));
            }
        } else {
            change(
                report,
                &pointer("/module_versions", id),
                &format!("Used the explicitly supported unpinned format-{source} source baseline"),
            );
        }
    }
    for id in required.component_schemas.keys() {
        let known = matches!(id.as_str(), "mesh" | "material")
            || (source == 3
                && matches!(
                    id.as_str(),
                    "player"
                        | "persistent"
                        | "boss"
                        | "actor"
                        | "health"
                        | "ai"
                        | "melee"
                        | "obstacle"
                        | "portal"
                        | "puzzle.board"
                        | "puzzle.cell"
                        | "example.spin"
                ));
        if !known {
            return Err(format!(
                "No known format-{source} source component contract for '{id}'"
            ));
        }
    }
    Ok(())
}

/// Legacy boss data has one known boolean field. Materialize missing defaults
/// first in templates, then only on objects without an inherited boss component.
/// Explicit local values and dot-field overrides remain authoritative.
fn migrate_boss_defaults(
    document: &mut Value,
    source: u32,
    report: &mut MigrationReport,
) -> Result<(), String> {
    let Some(game) = document.get_mut("game").filter(|game| !game.is_null()) else {
        return Ok(());
    };
    let mut prefab_bosses = BTreeMap::new();
    let mut prefab_components = BTreeMap::<String, Vec<String>>::new();
    if let Some(prefabs) = game.get_mut("prefabs").and_then(Value::as_object_mut) {
        for (id, prefab) in prefabs {
            if let Some(components) = prefab.get("components").and_then(Value::as_object) {
                prefab_components.insert(id.clone(), components.keys().cloned().collect());
            }
            let path = format!("{}/components/boss", pointer("/game/prefabs", id));
            if let Some(boss) = prefab.get_mut("components").and_then(|v| v.get_mut("boss")) {
                validate_legacy_boss(boss, &path, source)?;
                materialize_boss(boss, &path, report);
                prefab_bosses.insert(id.clone(), boss.clone());
            }
        }
    }
    if let Some(scenes) = game.get_mut("scenes").and_then(Value::as_object_mut) {
        for (scene_id, scene) in scenes {
            if let Some(objects) = scene.get_mut("objects").and_then(Value::as_array_mut) {
                for (index, object) in objects.iter_mut().enumerate() {
                    let base = format!("{}/objects/{index}", pointer("/game/scenes", scene_id));
                    let inherited = object
                        .get("prefab")
                        .and_then(Value::as_str)
                        .is_some_and(|id| prefab_bosses.contains_key(id));
                    if let Some(boss) = object.get_mut("components").and_then(|v| v.get_mut("boss"))
                    {
                        let path = format!("{base}/components/boss");
                        validate_legacy_boss(boss, &path, source)?;
                        if !inherited {
                            materialize_boss(boss, &path, report);
                        }
                    }
                    if let Some(overrides) = object.get("overrides").and_then(Value::as_object) {
                        // A longer attached component ID such as boss.extra owns
                        // its own overrides. Never reinterpret it as boss data.
                        for (path, value) in overrides {
                            if !path.starts_with("boss.") {
                                continue;
                            }
                            let local_owner = object
                                .get("components")
                                .and_then(Value::as_object)
                                .into_iter()
                                .flat_map(|components| components.keys());
                            let prefab_owner = object
                                .get("prefab")
                                .and_then(Value::as_str)
                                .and_then(|id| prefab_components.get(id));
                            let longer =
                                local_owner
                                    .chain(prefab_owner.into_iter().flatten())
                                    .any(|id| {
                                        id.len() > "boss".len()
                                            && path
                                                .strip_prefix(id.as_str())
                                                .is_some_and(|tail| tail.starts_with('.'))
                                    });
                            if !longer && (path != "boss.victory_on_death" || !value.is_boolean()) {
                                return Err(format!(
                                    "Unknown or malformed legacy boss override at {}",
                                    pointer(&format!("{base}/overrides"), path)
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_legacy_boss(value: &Value, path: &str, source: u32) -> Result<(), String> {
    if source != 3 {
        return Err(format!("No known format-{source} boss contract at {path}"));
    }
    let fields = value
        .as_object()
        .ok_or_else(|| format!("Legacy boss must be an object at {path}"))?;
    for (field, value) in fields {
        if field != "victory_on_death" || !value.is_boolean() {
            return Err(format!(
                "Unknown or malformed legacy boss field at {}",
                pointer(path, field)
            ));
        }
    }
    Ok(())
}

fn materialize_boss(value: &mut Value, path: &str, report: &mut MigrationReport) {
    let fields = value.as_object_mut().expect("validated legacy boss");
    if !fields.contains_key("victory_on_death") {
        fields.insert("victory_on_death".into(), json!(true));
        change(
            report,
            &format!("{path}/victory_on_death"),
            "Materialized legacy effective boss victory_on_death=true; reusable schema defaults to false",
        );
    }
}

fn change(report: &mut MigrationReport, path: &str, description: &str) {
    report.changes.push(MigrationChange {
        path: path.into(),
        description: description.into(),
    });
}

fn pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

// Record shapes deliberately stop at opaque JSON payloads. New source fields
// cannot be mistaken for supported configuration, nor silently disappear.
#[derive(Clone, Copy)]
enum Record {
    Document,
    Graph,
    Node,
    Edge,
    Engine,
    Exposure,
    Exposed,
    Target,
    Variant,
    Ready,
    Project,
    Rule,
    PropertyStore,
    PropertyData,
    Game,
    Scene,
    Prefab,
    Authored,
    Object,
    Transform,
    Execution,
    Resource,
    Stage,
    System,
}
#[derive(Clone, Copy)]
enum Shape {
    Opaque,
    Record(Record),
    Map(Record),
    Array(Record),
    PropertyType,
    Condition,
    Commands,
}

fn fields(record: Record) -> &'static [(&'static str, Shape)] {
    use Record::*;
    use Shape::{Array as A, Map as M, Opaque as O, Record as R};
    match record {
        Document => &[
            ("schema_version", O),
            ("name", O),
            ("root", R(Graph)),
            ("custom_sources", O),
            ("shader_sources", O),
            ("asset_manifest", O),
            ("module_versions", O),
            ("engine", R(Engine)),
            ("project", R(Project)),
            ("properties", R(PropertyStore)),
            ("game", R(Game)),
        ],
        Graph => &[("nodes", A(Node)), ("edges", A(Edge))],
        Node => &[
            ("id", O),
            ("module", O),
            ("label", O),
            ("position", O),
            ("params", O),
            ("subgraph", R(Graph)),
        ],
        Edge => &[
            ("from_node", O),
            ("from_port", O),
            ("to_node", O),
            ("to_port", O),
        ],
        Engine => &[
            ("initialized", O),
            ("enabled_modules", O),
            ("exposure", R(Exposure)),
        ],
        Exposure => &[("parameters", M(Exposed)), ("variants", M(Variant))],
        Exposed => &[
            ("label", O),
            ("value_type", Shape::PropertyType),
            ("default", O),
            ("target", R(Target)),
        ],
        Target => &[("pass", O), ("name", O)],
        Variant => &[("label", O), ("default", O), ("options", M(Ready))],
        Ready => &[("label", O), ("commands", Shape::Commands)],
        Project => &[("parameters", O), ("variants", O), ("rules", A(Rule))],
        Rule => &[
            ("group", O),
            ("variant", O),
            ("condition", Shape::Condition),
        ],
        PropertyStore => &[
            ("active", R(PropertyData)),
            ("inactive", R(PropertyData)),
            ("detached_components", O),
        ],
        PropertyData => &[
            ("global", O),
            ("modules", O),
            ("objects", O),
            ("components", O),
        ],
        Game => &[
            ("entry_scene", O),
            ("scenes", M(Scene)),
            ("prefabs", M(Prefab)),
            ("execution", R(Execution)),
            ("audio_cues", O),
        ],
        Scene => &[("name", O), ("objects", A(Authored))],
        Prefab => &[("color", O), ("components", O)],
        Authored => &[
            ("object", R(Object)),
            ("prefab", O),
            ("components", O),
            ("overrides", O),
            ("references", O),
        ],
        Object => &[
            ("id", O),
            ("parent", O),
            ("transform", R(Transform)),
            ("color", O),
        ],
        Transform => &[("translation", O), ("rotation", O), ("scale", O)],
        Execution => &[("resources", A(Resource)), ("stages", A(Stage))],
        Resource => &[
            ("name", O),
            ("data_type", O),
            ("external", O),
            ("initial", O),
        ],
        Stage => &[("name", O), ("nodes", A(System))],
        System => &[
            ("id", O),
            ("module", O),
            ("params", O),
            ("bindings", O),
            ("after", O),
        ],
    }
}

fn archive_record(
    value: &mut Value,
    record: Record,
    path: &str,
    archive: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    archive_fields(value, fields(record), path, archive)
}
fn archive_fields(
    value: &mut Value,
    fields: &[(&str, Shape)],
    path: &str,
    archive: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    let unknown = object
        .keys()
        .filter(|key| !fields.iter().any(|(name, _)| *name == key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for key in unknown {
        archive.insert(
            pointer(path, &key),
            object.remove(&key).expect("existing field"),
        );
    }
    for (key, shape) in fields {
        if let Some(value) = object.get_mut(*key) {
            archive_shape(value, *shape, &pointer(path, key), archive)?;
        }
    }
    Ok(())
}
fn archive_shape(
    value: &mut Value,
    shape: Shape,
    path: &str,
    archive: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    match shape {
        Shape::Opaque => {}
        Shape::Record(record) => archive_record(value, record, path, archive)?,
        Shape::Map(record) => {
            if let Some(values) = value.as_object_mut() {
                for (id, value) in values {
                    archive_record(value, record, &pointer(path, id), archive)?;
                }
            }
        }
        Shape::Array(record) => {
            if let Some(values) = value.as_array_mut() {
                for (i, value) in values.iter_mut().enumerate() {
                    archive_record(value, record, &format!("{path}/{i}"), archive)?;
                }
            }
        }
        Shape::PropertyType => archive_tagged(
            value,
            path,
            archive,
            &[
                ("Number", &["min", "max"]),
                ("Boolean", &[]),
                ("Enum", &["choices"]),
                ("Text", &["max_len"]),
                ("ObjectReference", &["required_components", "optional"]),
                ("AssetReference", &["kinds", "optional"]),
            ],
        )?,
        Shape::Condition => archive_tagged(
            value,
            path,
            archive,
            &[
                ("Parameter", &["parameter", "op", "value"]),
                ("Time", &["op", "value"]),
            ],
        )?,
        Shape::Commands => {
            if let Some(values) = value.as_array_mut() {
                for (i, value) in values.iter_mut().enumerate() {
                    archive_tagged(
                        value,
                        &format!("{path}/{i}"),
                        archive,
                        &[
                            ("SetParameter", &["pass", "name", "value"]),
                            ("SetEnabled", &["pass", "enabled"]),
                            ("ClearResource", &["name", "value"]),
                            ("CopyResource", &["from", "to"]),
                        ],
                    )?;
                }
            }
        }
    }
    Ok(())
}
fn archive_tagged(
    value: &mut Value,
    path: &str,
    archive: &mut BTreeMap<String, Value>,
    variants: &[(&str, &[&str])],
) -> Result<(), String> {
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };
    if object.len() != 1 {
        return Err(format!("Malformed known tagged value at {path}"));
    }
    let (tag, body) = object.iter_mut().next().expect("one variant");
    let Some((_, keys)) = variants.iter().find(|(name, _)| *name == tag.as_str()) else {
        return Err(format!("Unknown known-field variant '{tag}' at {path}"));
    };
    let fields = keys
        .iter()
        .map(|key| (*key, Shape::Opaque))
        .collect::<Vec<_>>();
    archive_fields(body, &fields, &pointer(path, tag), archive)
}

/// Reject duplicate keys rather than silently choosing one meaning. A BOM is
/// accepted for inspection; it remains part of the report's original-byte hash.
pub(crate) fn parse_source(raw: &[u8]) -> Result<Value, String> {
    struct Unique(Value);
    impl<'de> Deserialize<'de> for Unique {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Unique;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("JSON without duplicate keys")
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Unique, E> {
                    Ok(Unique(Value::Null))
                }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Unique, E> {
                    Ok(Unique(Value::Bool(v)))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Unique, E> {
                    serde_json::Number::from_f64(v)
                        .map(|v| Unique(v.into()))
                        .ok_or_else(|| E::custom("Non-finite JSON number"))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<Unique, A::Error> {
                    let mut values = vec![];
                    while let Some(Unique(value)) = seq.next_element()? {
                        values.push(value);
                    }
                    Ok(Unique(Value::Array(values)))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Unique, A::Error> {
                    let mut values = Map::new();
                    while let Some(key) = map.next_key::<String>()? {
                        if values.contains_key(&key) {
                            return Err(serde::de::Error::custom(format!(
                                "Duplicate JSON key '{key}'"
                            )));
                        }
                        let Unique(value) = map.next_value()?;
                        values.insert(key, value);
                    }
                    Ok(Unique(Value::Object(values)))
                }
            }
            deserializer.deserialize_any(Visitor)
        }
    }
    let input = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
    serde_json::from_slice::<Unique>(input)
        .map(|v| v.0)
        .map_err(|error| format!("Project JSON: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{
        AuthoredObject, ComponentSchema, ExecutableSystem, ExecutionGraph, GameProject,
        ModuleRegistry, Prefab, PropertySchema, PropertyScope, PropertyType, SystemContext,
        SystemDescriptor, SystemNode, SystemRegistry, SystemStage, Transform, WorldKind,
        WorldObject, demo_document,
    };

    struct Noop;
    impl ExecutableSystem for Noop {
        fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
            Ok(())
        }
    }

    fn boss_schema() -> ComponentSchema {
        ComponentSchema {
            id: "boss".into(),
            owner_module: "game.health".into(),
            version: "1".into(),
            label: "Boss fixture".into(),
            defaults: json!({"victory_on_death":false}),
            fields: vec![PropertySchema {
                id: "boss.victory_on_death".into(),
                label: "Fixture outcome rule".into(),
                scope: PropertyScope::Component {
                    component: "boss".into(),
                },
                value_type: PropertyType::Boolean,
                default: json!(false),
                requires_modules: vec![],
                requires_components: vec![],
            }],
            requires_modules: vec![],
            requires_components: vec![],
        }
    }

    fn composition() -> CompositionContext {
        let mut systems = SystemRegistry::builtin();
        systems.components_mut().register(boss_schema()).unwrap();
        systems
            .register(
                SystemDescriptor {
                    id: "game.health".into(),
                    state_schema: "game.health/1".into(),
                    components: vec!["boss".into()],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(Noop)),
            )
            .unwrap();
        CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap()
    }

    fn profile() -> ProfileIdentity {
        ProfileIdentity::from_payload("migration.fixture", "1", &json!({"engine":"software"}))
            .unwrap()
    }

    // This catalog belongs to a fixed synthetic source fixture, not production
    // discovery. Production must supply its audited published profile catalog.
    fn fixture_target(
        composition: &CompositionContext,
        document: &GraphDocument,
    ) -> KnownMigrationTarget {
        let required = composition.required_contracts(document).unwrap();
        KnownMigrationTarget {
            module_versions: required.module_versions,
            component_schemas: required.component_schemas,
            system_state_schemas: required.system_state_schemas,
        }
    }

    fn legacy(source: u32) -> GraphDocument {
        let mut document = demo_document(WorldKind::Ecs);
        document.schema_version = source;
        document.module_versions.clear();
        document.engine_lock = None;
        document.engine_profile = None;
        document.legacy_archive.clear();
        document
    }

    fn object(id: u64, boss: Option<Value>, prefab: Option<&str>) -> AuthoredObject {
        let mut object = AuthoredObject::new(WorldObject {
            id,
            parent: None,
            transform: Transform::default(),
            color: [255; 4],
        });
        object.prefab = prefab.map(str::to_owned);
        if let Some(boss) = boss {
            object.components.insert("boss".into(), boss);
        }
        object
    }

    fn boss_project() -> GraphDocument {
        let mut document = legacy(3);
        let mut game = GameProject::empty();
        game.execution = ExecutionGraph {
            resources: vec![],
            stages: vec![SystemStage {
                name: "Explicit source fixture".into(),
                nodes: vec![SystemNode {
                    id: "health".into(),
                    module: "game.health".into(),
                    params: json!({}),
                    bindings: BTreeMap::new(),
                    after: vec![],
                }],
            }],
        };
        game.prefabs.insert(
            "old default".into(),
            Prefab {
                color: [255; 4],
                components: BTreeMap::from([("boss".into(), json!({}))]),
            },
        );
        game.prefabs.insert(
            "explicit false".into(),
            Prefab {
                color: [255; 4],
                components: BTreeMap::from([("boss".into(), json!({"victory_on_death":false}))]),
            },
        );
        let mut override_false = object(4, None, Some("old default"));
        override_false
            .overrides
            .insert("boss.victory_on_death".into(), json!(false));
        let mut override_true = object(5, Some(json!({})), Some("explicit false"));
        override_true
            .overrides
            .insert("boss.victory_on_death".into(), json!(true));
        game.scenes.get_mut("empty").unwrap().objects = vec![
            object(1, Some(json!({})), None),
            object(2, None, Some("old default")),
            object(3, Some(json!({})), Some("explicit false")),
            override_false,
            override_true,
        ];
        document.game = Some(game);
        document
    }

    fn prepare(document: &GraphDocument) -> Result<MigrationCandidate, String> {
        let composition = composition();
        prepare_legacy_migration(
            &serde_json::to_vec(document).unwrap(),
            &composition,
            profile(),
            SdkIdentity::current(),
            &fixture_target(&composition, document),
        )
    }

    #[test]
    fn explicit_supported_formats_capture_locks_without_touching_source_bytes() {
        let composition = composition();
        for source in 1..=3 {
            let mut document = legacy(source);
            if source > 1 {
                let version = if source == 2 { "0.2.0" } else { "0.3.0" };
                document.module_versions = composition
                    .required_contracts(&document)
                    .unwrap()
                    .module_versions
                    .into_keys()
                    .map(|id| (id, version.into()))
                    .collect();
            }
            document
                .module_versions
                .insert("archived.unknown".into(), "ancient".into());
            document
                .custom_sources
                .insert("unused".into(), "// Keep exact unused source\n".into());
            document
                .shader_sources
                .insert("unused.hlsl".into(), "// Untouched shader\n".into());
            let mut raw = vec![0xef, 0xbb, 0xbf];
            raw.extend(serde_json::to_vec_pretty(&document).unwrap());
            raw.extend(b"\r\n");
            let original = raw.clone();
            let candidate = prepare_legacy_migration(
                &raw,
                &composition,
                profile(),
                SdkIdentity::current(),
                &fixture_target(&composition, &document),
            )
            .unwrap();
            assert_eq!(raw, original);
            assert_eq!(
                candidate.report.source_sha256,
                format!("{:x}", Sha256::digest(&original))
            );
            assert_eq!(candidate.report.source_schema, source);
            assert_eq!(candidate.document.schema_version, 4);
            assert_eq!(
                candidate.document.module_versions["archived.unknown"],
                "ancient"
            );
            assert_eq!(candidate.document.custom_sources, document.custom_sources);
            assert_eq!(candidate.document.shader_sources, document.shader_sources);
            composition
                .validate_lock(&candidate.document, Some(&SdkIdentity::current()))
                .unwrap();
        }
    }

    #[test]
    fn legacy_inspection_preserves_source_format_pins_and_bytes_without_semantic_migration() {
        let mut document = boss_project();
        document
            .module_versions
            .insert("world.ecs".into(), "source-only-pin".into());
        let mut value = serde_json::to_value(&document).unwrap();
        value["unknown_source_record"] = json!({"keep":"original bytes"});
        value["game"]["scenes"]["empty"]["unknown_scene_field"] = json!([1, 2]);
        let mut raw = vec![0xef, 0xbb, 0xbf];
        raw.extend(serde_json::to_vec_pretty(&value).unwrap());
        raw.extend(b"\r\n");
        let original = raw.clone();
        let inspected = inspect_legacy_document(&raw).unwrap();
        assert_eq!(raw, original);
        assert_eq!(inspected.schema_version, 3);
        assert_eq!(inspected.module_versions, document.module_versions);
        assert!(inspected.engine_lock.is_none());
        assert!(inspected.engine_profile.is_none());
        assert_eq!(
            inspected.game.as_ref().unwrap().prefabs["old default"].components["boss"],
            json!({})
        );
        let mut malformed = value;
        malformed["game"]["scenes"]["empty"]["objects"][0]["object"]["transform"]["translation"] =
            json!(null);
        assert!(
            inspect_legacy_document(&serde_json::to_vec(&malformed).unwrap())
                .unwrap_err()
                .contains("Malformed legacy document inspection")
        );
        for source in [0, 4, 5] {
            malformed["schema_version"] = json!(source);
            assert!(
                inspect_legacy_document(&serde_json::to_vec(&malformed).unwrap())
                    .unwrap_err()
                    .contains("supports formats 1, 2 and 3")
            );
        }
    }

    #[test]
    fn boss_defaults_preserve_local_prefab_inheritance_and_both_field_overrides() {
        let candidate = prepare(&boss_project()).unwrap();
        let game = candidate.document.game.unwrap();
        let resolved = game.resolve_scene("empty").unwrap();
        let values = resolved
            .iter()
            .map(|entity| {
                entity.components["boss"]["victory_on_death"]
                    .as_bool()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(values, [true, true, false, false, true]);
        assert_eq!(
            game.scenes["empty"].objects[2].components["boss"],
            json!({})
        );
        assert_eq!(
            game.prefabs["old default"].components["boss"],
            json!({"victory_on_death":true})
        );
        assert_eq!(
            candidate
                .report
                .changes
                .iter()
                .filter(|change| change
                    .description
                    .contains("Materialized legacy effective boss"))
                .count(),
            2
        );
    }

    #[test]
    fn malformed_and_unknown_boss_contracts_reject_including_unused_prefabs() {
        for boss in [
            Value::Null,
            json!([]),
            json!(true),
            json!({"victory_on_death":null}),
            json!({"victory_on_death":1}),
            json!({"unknown":true}),
        ] {
            let mut document = boss_project();
            document.game.as_mut().unwrap().prefabs.insert(
                "unused".into(),
                Prefab {
                    color: [255; 4],
                    components: BTreeMap::from([("boss".into(), boss)]),
                },
            );
            let raw = serde_json::to_vec(&document).unwrap();
            let before = raw.clone();
            let composition = composition();
            assert!(
                prepare_legacy_migration(
                    &raw,
                    &composition,
                    profile(),
                    SdkIdentity::current(),
                    &fixture_target(&composition, &document)
                )
                .unwrap_err()
                .contains("boss")
            );
            assert_eq!(raw, before);
        }
        for (path, value) in [
            ("boss.victory_on_death", Value::Null),
            ("boss.unknown", json!(false)),
        ] {
            let mut document = boss_project();
            document
                .game
                .as_mut()
                .unwrap()
                .scenes
                .get_mut("empty")
                .unwrap()
                .objects[0]
                .overrides
                .insert(path.into(), value);
            assert!(prepare(&document).unwrap_err().contains("boss override"));
        }
    }

    #[test]
    fn unknown_inactive_boss_owner_preserves_every_payload_and_override_without_interpretation() {
        for source in [1, 3] {
            let mut document = boss_project();
            document.schema_version = source;
            document
                .module_versions
                .insert("game.health".into(), "unknown-archived-version".into());
            let game = document.game.as_mut().unwrap();
            game.execution = ExecutionGraph::default();
            game.prefabs
                .get_mut("old default")
                .unwrap()
                .components
                .insert("boss".into(), json!({"future_rule":null}));
            game.scenes.get_mut("empty").unwrap().objects[0]
                .components
                .insert("boss".into(), Value::Null);
            game.scenes.get_mut("empty").unwrap().objects[0]
                .overrides
                .insert("boss.unknown".into(), json!({"opaque":[1,null,"keep"]}));
            let before = document.game.clone();
            let candidate = prepare(&document).unwrap();
            assert_eq!(candidate.document.game, before);
            assert_eq!(
                candidate.document.module_versions["game.health"],
                "unknown-archived-version"
            );
            assert!(
                !candidate
                    .document
                    .engine_lock
                    .as_ref()
                    .unwrap()
                    .component_schemas
                    .contains_key("boss")
            );
            assert!(!candidate.report.changes.iter().any(|change| {
                change
                    .description
                    .contains("Materialized legacy effective boss")
            }));
            assert!(candidate.report.changes.iter().any(|change| {
                change
                    .description
                    .contains("Preserved inactive boss data without interpretation")
            }));
        }
    }

    #[test]
    fn unknown_typed_fields_are_archived_by_pointer_and_component_payloads_stay_authored() {
        let document = boss_project();
        let composition = composition();
        let mut source = serde_json::to_value(&document).unwrap();
        source["future/root~field"] = json!({"preserve":[1,null,{"text":"source"}]});
        source["engine"]["future_engine"] = json!({"mode":"archive"});
        source["game"]["scenes"]["empty"]["future_scene"] = json!(17);
        source["game"]["scenes"]["empty"]["objects"][0]["object"]["transform"]["future_transform"] =
            json!([8, 9]);
        source["game"]["scenes"]["empty"]["objects"][0]["components"]["unknown.component"] =
            json!({"opaque":{"null":null,"array":[1,2]}});
        source["game"]["scenes"]["empty"]["objects"][0]["overrides"]["unknown.component.opaque.null"] =
            json!("keep override");
        let raw = serde_json::to_vec(&source).unwrap();
        let candidate = prepare_legacy_migration(
            &raw,
            &composition,
            profile(),
            SdkIdentity::current(),
            &fixture_target(&composition, &document),
        )
        .unwrap();
        assert_eq!(candidate.legacy_archive.len(), 4);
        assert_eq!(
            candidate.legacy_archive["/future~1root~0field"],
            source["future/root~field"]
        );
        assert_eq!(
            candidate.legacy_archive["/engine/future_engine"],
            source["engine"]["future_engine"]
        );
        assert_eq!(
            candidate.legacy_archive["/game/scenes/empty/future_scene"],
            json!(17)
        );
        assert_eq!(
            candidate.legacy_archive["/game/scenes/empty/objects/0/object/transform/future_transform"],
            json!([8, 9])
        );
        let object = &candidate.document.game.as_ref().unwrap().scenes["empty"].objects[0];
        assert_eq!(
            object.components["unknown.component"],
            source["game"]["scenes"]["empty"]["objects"][0]["components"]["unknown.component"]
        );
        assert_eq!(
            object.overrides["unknown.component.opaque.null"],
            json!("keep override")
        );
    }

    #[test]
    fn known_field_type_errors_are_not_hidden_in_the_archive() {
        let document = legacy(3);
        let composition = composition();
        let target = fixture_target(&composition, &document);
        for path in [
            "/engine/enabled_modules",
            "/root/nodes/0/position",
            "/custom_sources",
        ] {
            let mut source = serde_json::to_value(&document).unwrap();
            *source.pointer_mut(path).unwrap() = json!({"future":null});
            let error = prepare_legacy_migration(
                &serde_json::to_vec(&source).unwrap(),
                &composition,
                profile(),
                SdkIdentity::current(),
                &target,
            )
            .unwrap_err();
            assert!(error.contains("Malformed legacy document"), "{error}");
        }
    }

    #[test]
    fn active_unknown_source_versions_and_catalogs_reject_without_repinning() {
        let mut document = legacy(3);
        document
            .module_versions
            .insert("world.ecs".into(), "0.4.0-mutated".into());
        let composition = composition();
        let target = fixture_target(&composition, &document);
        let raw = serde_json::to_vec(&document).unwrap();
        let original = raw.clone();
        assert!(
            prepare_legacy_migration(
                &raw,
                &composition,
                profile(),
                SdkIdentity::current(),
                &target
            )
            .unwrap_err()
            .contains("0.4.0-mutated")
        );
        assert_eq!(raw, original);
        let mut systems = SystemRegistry::builtin();
        systems.install_module("unknown.active", "0.4.0").unwrap();
        let composition = CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap();
        let mut document = legacy(3);
        document
            .engine
            .enabled_modules
            .insert("unknown.active".into());
        assert!(
            prepare_legacy_migration(
                &serde_json::to_vec(&document).unwrap(),
                &composition,
                profile(),
                SdkIdentity::current(),
                &fixture_target(&composition, &document)
            )
            .unwrap_err()
            .contains("No known format-3 source migration contract")
        );
    }

    #[test]
    fn audited_target_detects_module_schema_and_state_contract_drift() {
        let document = boss_project();
        let composition = composition();
        let target = fixture_target(&composition, &document);
        let raw = serde_json::to_vec(&document).unwrap();
        let mut module = target.clone();
        module
            .module_versions
            .insert("game.health".into(), "old".into());
        let mut schema = target.clone();
        schema
            .component_schemas
            .get_mut("boss")
            .unwrap()
            .contract_hash = "0".repeat(64);
        let mut owner = target.clone();
        owner
            .component_schemas
            .get_mut("boss")
            .unwrap()
            .owner_module = "another.owner".into();
        let mut state = target.clone();
        state
            .system_state_schemas
            .insert("game.health".into(), "game.health/0".into());
        for target in [module, schema, owner, state] {
            assert!(
                prepare_legacy_migration(
                    &raw,
                    &composition,
                    profile(),
                    SdkIdentity::current(),
                    &target
                )
                .unwrap_err()
                .contains("audited")
            );
        }
    }

    #[test]
    fn namespace_prefix_collision_does_not_reinterpret_an_archived_override_as_boss() {
        let mut document = boss_project();
        let object = &mut document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("empty")
            .unwrap()
            .objects[0];
        object
            .components
            .insert("boss.extra".into(), json!({"setting":1}));
        object
            .overrides
            .insert("boss.extra.setting".into(), json!(2));
        let candidate = prepare(&document).unwrap();
        let object = &candidate.document.game.as_ref().unwrap().scenes["empty"].objects[0];
        assert_eq!(object.components["boss.extra"], json!({"setting":1}));
        assert_eq!(object.overrides["boss.extra.setting"], json!(2));
    }

    #[test]
    fn duplicate_json_keys_and_nonlegacy_formats_reject_before_candidate_creation() {
        assert!(
            parse_source(br#"{"schema_version":3,"schema_version":1}"#)
                .unwrap_err()
                .contains("Duplicate JSON key")
        );
        let composition = composition();
        for source in [0, 4, 5] {
            let document = legacy(source);
            assert!(
                prepare_legacy_migration(
                    &serde_json::to_vec(&document).unwrap(),
                    &composition,
                    profile(),
                    SdkIdentity::current(),
                    &fixture_target(&composition, &document)
                )
                .unwrap_err()
                .contains("supports formats 1, 2 and 3")
            );
        }
    }
}
