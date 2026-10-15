//! The working execution graph shared by Team and standalone Player delivery.
//! Published profiles remain archival contracts; only the delivered working
//! graph and its source maps are projected onto executable dependencies.
use module_engine_core::{Graph, GraphDocument};
use std::{collections::BTreeSet, path::Path};

pub fn prepare(document: &GraphDocument, backend: &Path) -> Result<GraphDocument, String> {
    // Invalid authored contracts must not become valid merely by discarding a
    // fragment. In particular, exporting never repairs pins or republishes a profile.
    crate::engine_profiles::validate(document, backend)?;
    crate::host::validate_document_at(document, backend)?;
    let mut prepared = project(document);
    if prepared.game.is_some() {
        prepared.game = Some(crate::host::prepare_game_at(&prepared, backend)?);
    }
    Ok(prepared)
}

/// Both delivery policies own the transitive includes of their retained HLSL.
/// Asset-library retention is separate: Team keeps declared authoring assets,
/// while Player may select only the assets required by its running game.
pub fn shader_dependencies(
    document: &GraphDocument,
    backend: &Path,
) -> Result<BTreeSet<String>, String> {
    let mut paths = BTreeSet::new();
    for source in document.shader_sources.values() {
        paths.extend(module_engine_project::collect_hlsl_source_dependencies(
            backend,
            "__inline__.hlsl",
            source,
        )?);
    }
    Ok(paths)
}

fn project(document: &GraphDocument) -> GraphDocument {
    let mut prepared = document.clone();
    retain_dependencies(&mut prepared.root, false);
    let (mut rust_ids, mut shader_ids) = (BTreeSet::new(), BTreeSet::new());
    source_ids(&prepared.root, &mut rust_ids, &mut shader_ids);
    // A scheduled system is executable even when it has no graph output. Its
    // order, resource bindings, dependencies and enabled modules are untouched.
    if let Some(game) = &prepared.game {
        rust_ids.extend(crate::game_host::script_ids(game));
    }
    prepared
        .custom_sources
        .retain(|id, _| rust_ids.contains(id));
    prepared
        .shader_sources
        .retain(|id, _| shader_ids.contains(id));
    prepared
}

fn retain_dependencies(graph: &mut Graph, nested: bool) {
    let mut retained = BTreeSet::new();
    let mut pending = graph
        .nodes
        .iter()
        .filter(|node| {
            if nested {
                // Compound contracts require input mappings even when this
                // implementation does not consume one of the inputs.
                matches!(node.module.as_str(), "interface.input" | "interface.output")
            } else {
                node.module == "viewport.output"
            }
        })
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if retained.insert(id.clone()) {
            pending.extend(
                graph
                    .edges
                    .iter()
                    .filter(|edge| edge.to_node == id)
                    .map(|edge| edge.from_node.clone()),
            );
        }
    }
    graph.nodes.retain(|node| retained.contains(&node.id));
    graph
        .edges
        .retain(|edge| retained.contains(&edge.from_node) && retained.contains(&edge.to_node));
    for node in &mut graph.nodes {
        if let Some(inner) = &mut node.subgraph {
            retain_dependencies(inner, true);
        }
    }
}

fn source_ids(graph: &Graph, rust: &mut BTreeSet<String>, hlsl: &mut BTreeSet<String>) {
    for node in &graph.nodes {
        if let Some(id) = node
            .params
            .get("source_id")
            .and_then(serde_json::Value::as_str)
        {
            match node.module.as_str() {
                "gpu.custom" => {
                    hlsl.insert(id.into());
                }
                "custom.rust" | "custom.rust.update" => {
                    rust.insert(id.into());
                }
                _ => {}
            }
        }
        if let Some(inner) = &node.subgraph {
            source_ids(inner, rust, hlsl);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use module_engine_core::*;
    use module_engine_project::{EngineProfile, hash_file};
    use serde_json::json;
    use std::{collections::BTreeMap, fs, path::PathBuf};

    fn backend() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .canonicalize()
            .unwrap()
    }

    pub fn software_with_inactive_vulkan() -> GraphDocument {
        let mut document = demo_document(WorldKind::Ecs);
        document.engine.initialized = true;
        document.game = Some(GameProject::empty());
        let mut alternative = make_node(
            "alternative",
            "render.vulkan",
            "Unused Vulkan",
            [0., 0.],
            json!({}),
        );
        let mut inner =
            module_engine_vulkan::default_graph(module_engine_vulkan::RenderPath::Forward);
        custom_aa(&mut inner, "inactive-shader");
        alternative.subgraph = Some(Box::new(inner));
        document.root.nodes.push(alternative);
        document
            .root
            .edges
            .push(make_edge("extract", "scene", "alternative", "scene"));
        document.shader_sources.insert(
            "inactive-shader".into(),
            "#include \"missing-inactive-only.hlsl\"\n".into(),
        );
        publish(&mut document);
        document
    }

    fn custom_aa(graph: &mut Graph, source: &str) {
        let node = graph.nodes.iter_mut().find(|node| node.id == "aa").unwrap();
        node.module = "gpu.custom".into();
        node.params = json!({"source_id":source,"manifest":module_engine_vulkan::builtin_manifest("gpu.fxaa").unwrap()});
    }

    fn publish(document: &mut GraphDocument) {
        crate::engine_profiles::finish_template(
            document,
            crate::engine_profiles::ProfileKind::Neutral,
            &backend(),
        )
        .unwrap();
    }

    /// A diagnostic fixture emitter, not a product command or an export test.
    /// The final native CLI acceptance consumes the generated author document.
    #[test]
    #[ignore = "requires an explicit new absolute MODULE_ENGINE_STAGE4_FIXTURE_PATH"]
    fn emit_stage4_software_inactive_vulkan_fixture() {
        use std::io::Write;
        let path = PathBuf::from(
            std::env::var_os("MODULE_ENGINE_STAGE4_FIXTURE_PATH")
                .expect("Set MODULE_ENGINE_STAGE4_FIXTURE_PATH to a new absolute JSON file"),
        );
        assert!(path.is_absolute(), "Stage4 fixture path must be absolute");
        assert!(
            path.parent().is_some_and(Path::is_dir),
            "Create the fixture's parent directory explicitly first"
        );
        let document = software_with_inactive_vulkan();
        let bytes = serde_json::to_vec_pretty(&document).unwrap();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("Stage4 fixture must be a new file; existing files are never replaced");
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        println!(
            "Stage4 software-active / inactive-Vulkan author fixture: {}",
            path.display()
        );
    }

    fn write_metadata(root: &Path, document: &GraphDocument, features: &[&str]) {
        fs::write(
            root.join("project.json"),
            serde_json::to_vec_pretty(document).unwrap(),
        )
        .unwrap();
        let manifest = module_engine_player::package::PlayerManifest {
            schema_version: 2,
            engine_version: env!("CARGO_PKG_VERSION").into(),
            project: "project.json".into(),
            files: BTreeMap::from([(
                "project.json".into(),
                hash_file(&root.join("project.json")).unwrap(),
            )]),
            native_features: features.iter().map(|id| (*id).into()).collect(),
            sdk_data_api_fingerprint: SDK_DATA_API_FINGERPRINT.into(),
            native_platform: Some(module_engine_player::package::native_platform()),
        };
        fs::write(
            root.join("player.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn projection_removes_inactive_backend_before_thin_metadata_and_include_checks() {
        let document = software_with_inactive_vulkan();
        let before = serde_json::to_vec(&document).unwrap();
        crate::host::validate_document_at(&document, &backend()).unwrap();
        let prepared = prepare(&document, &backend()).unwrap();
        assert!(
            !prepared
                .root
                .nodes
                .iter()
                .any(|node| node.id == "alternative")
        );
        assert!(prepared.shader_sources.is_empty());
        assert!(
            shader_dependencies(&prepared, &backend())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            crate::game_package::native_feature_key(&prepared, &backend()).unwrap(),
            "ui"
        );
        assert_eq!(prepared.engine_profile, document.engine_profile);
        assert_eq!(prepared.engine_lock, document.engine_lock);
        assert_eq!(prepared.module_versions, document.module_versions);
        assert_eq!(prepared.engine, document.engine);
        assert_eq!(
            prepared.game.as_ref().unwrap().execution,
            document.game.as_ref().unwrap().execution
        );
        assert_eq!(prepare(&prepared, &backend()).unwrap(), prepared);
        assert_eq!(serde_json::to_vec(&document).unwrap(), before);

        let guard = module_engine_script::TestArtifactGuard::new(
            &std::env::temp_dir(),
            "runtime-closure-metadata",
        )
        .unwrap();
        write_metadata(guard.path(), &prepared, &["ui"]);
        assert_eq!(
            module_engine_player::package::validate_package_metadata(guard.path()).unwrap(),
            prepared
        );
        // The loader remains strict. It must not silently project or ignore an
        // unknown node when a package was produced with inconsistent features.
        write_metadata(guard.path(), &document, &["ui"]);
        let error =
            module_engine_player::package::validate_package_metadata(guard.path()).unwrap_err();
        assert!(
            error.contains("render.vulkan") && error.contains("not registered"),
            "{error}"
        );
        assert!(
            crate::game_package::native_feature_key(&document, &backend())
                .unwrap_err()
                .contains("render.vulkan")
        );
    }

    #[test]
    fn recursive_projection_keeps_shared_sources_and_all_scheduled_rust_contracts() {
        let mut document = software_with_inactive_vulkan();
        let nested_alternative = document
            .root
            .nodes
            .iter()
            .find(|node| node.id == "alternative")
            .unwrap()
            .clone();
        let renderer = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "renderer")
            .unwrap();
        let nested = renderer.subgraph.as_mut().unwrap();
        nested.nodes.push(nested_alternative);
        nested
            .edges
            .push(make_edge("scene-in", "value", "alternative", "scene"));
        nested.nodes.extend([
            make_node(
                "dead-clock",
                "clock.time",
                "Unused clock",
                [0., 0.],
                json!({}),
            ),
            make_node(
                "dead-base",
                "scalar.value",
                "Unused value",
                [0., 0.],
                json!({"value":1.}),
            ),
            make_node(
                "dead-rust",
                "custom.rust",
                "Shared source",
                [0., 0.],
                json!({"source_id":"speed"}),
            ),
        ]);
        nested.edges.extend([
            make_edge("dead-clock", "value", "dead-rust", "time"),
            make_edge("dead-base", "value", "dead-rust", "base"),
        ]);
        let game = document.game.as_mut().unwrap();
        game.execution = ExecutionGraph {
            resources: [
                ("world", "GameWorld"),
                ("input", "GameInput"),
                ("events", "GameEvents"),
            ]
            .into_iter()
            .map(|(name, data_type)| SystemResource {
                name: name.into(),
                data_type: data_type.into(),
                external: true,
                initial: serde_json::Value::Null,
            })
            .collect(),
            stages: vec![SystemStage {
                name: "Update".into(),
                nodes: vec![SystemNode {
                    id: "scheduled".into(),
                    module: "custom.rust.update".into(),
                    params: json!({"source_id":"scheduled","components":[]}),
                    bindings: ["world", "input", "events"]
                        .into_iter()
                        .map(|id| (id.into(), id.into()))
                        .collect(),
                    after: vec![],
                }],
            }],
        };
        document.custom_sources.insert(
            "scheduled".into(),
            "fn update(_ctx: &mut ScriptContext) {}".into(),
        );
        document
            .custom_sources
            .insert("unreferenced".into(), "preserve only in author".into());
        publish(&mut document);
        let before = serde_json::to_vec(&document).unwrap();
        let prepared = prepare(&document, &backend()).unwrap();
        let inner = prepared
            .root
            .nodes
            .iter()
            .find(|node| node.id == "renderer")
            .unwrap()
            .subgraph
            .as_ref()
            .unwrap();
        assert_eq!(
            inner
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            ["scene-in", "rasterizer", "frame-out"]
        );
        assert_eq!(
            prepared
                .custom_sources
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["scheduled", "speed"]
        );
        assert!(prepared.shader_sources.is_empty());
        assert_eq!(
            prepared.game.as_ref().unwrap().execution,
            document.game.as_ref().unwrap().execution
        );
        assert_eq!(prepared.engine_lock, document.engine_lock);
        assert_eq!(serde_json::to_vec(&document).unwrap(), before);
        assert_eq!(
            crate::game_package::native_feature_key(&prepared, &backend()).unwrap(),
            "ui"
        );
    }

    #[test]
    fn nested_projection_preserves_unused_input_mappings_and_connected_disabled_nodes() {
        let mut graph = Graph {
            nodes: vec![
                make_node(
                    "input",
                    "interface.input",
                    "Required unused input",
                    [0., 0.],
                    json!({"port":"scene","data_type":"Scene3D"}),
                ),
                make_node(
                    "value",
                    "scalar.value",
                    "Value",
                    [0., 0.],
                    json!({"value":1.,"enabled":false}),
                ),
                make_node(
                    "output",
                    "interface.output",
                    "Output",
                    [0., 0.],
                    json!({"port":"value","data_type":"Scalar"}),
                ),
                make_node(
                    "dead",
                    "scalar.value",
                    "Unused value",
                    [0., 0.],
                    json!({"value":2.}),
                ),
            ],
            edges: vec![make_edge("value", "value", "output", "value")],
        };
        retain_dependencies(&mut graph, true);
        assert_eq!(
            graph
                .nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            ["input", "value", "output"]
        );
        assert_eq!(graph.nodes[1].params["enabled"], false);
    }

    #[test]
    fn active_shader_include_closure_is_transitive_and_missing_active_files_fail() {
        let guard = module_engine_script::TestArtifactGuard::new(
            &std::env::temp_dir(),
            "runtime-shader-closure",
        )
        .unwrap();
        fs::create_dir_all(guard.path().join("assets/gpu/include")).unwrap();
        fs::write(
            guard.path().join("assets/gpu/include/first.hlsl"),
            "#include \"second.hlsl\"\n",
        )
        .unwrap();
        fs::write(
            guard.path().join("assets/gpu/include/second.hlsl"),
            "float helper() { return 1; }\n",
        )
        .unwrap();
        let mut document =
            crate::host::gpu_document(WorldKind::Ecs, module_engine_vulkan::RenderPath::Forward);
        let graph = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "renderer")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap();
        custom_aa(graph, "active-shader");
        document.shader_sources.insert(
            "active-shader".into(),
            "#include \"include/first.hlsl\"\n".into(),
        );
        document.shader_sources.insert(
            "unused-shader".into(),
            "#include \"missing-inactive.hlsl\"\n".into(),
        );
        publish(&mut document);
        let before = serde_json::to_vec(&document).unwrap();
        let prepared = prepare(&document, &backend()).unwrap();
        assert_eq!(
            prepared
                .shader_sources
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["active-shader"]
        );
        assert_eq!(
            shader_dependencies(&prepared, guard.path()).unwrap(),
            BTreeSet::from([
                "assets/gpu/include/first.hlsl".into(),
                "assets/gpu/include/second.hlsl".into()
            ])
        );
        fs::remove_file(guard.path().join("assets/gpu/include/second.hlsl")).unwrap();
        assert!(
            shader_dependencies(&prepared, guard.path())
                .unwrap_err()
                .contains("second.hlsl")
        );
        assert_eq!(serde_json::to_vec(&document).unwrap(), before);
    }

    #[test]
    fn projection_preserves_enabled_modules_and_rejects_invalid_authored_pins() {
        let mut document = software_with_inactive_vulkan();
        document
            .engine
            .enabled_modules
            .insert("render.vulkan".into());
        publish(&mut document);
        let prepared = prepare(&document, &backend()).unwrap();
        assert_eq!(
            prepared.engine.enabled_modules,
            document.engine.enabled_modules
        );
        assert_eq!(
            crate::game_package::native_feature_key(&prepared, &backend()).unwrap(),
            "ui-vulkan"
        );
        assert_eq!(prepared.engine_profile, document.engine_profile);
        assert_eq!(prepared.engine_lock, document.engine_lock);
        document
            .module_versions
            .insert("world.ecs".into(), "foreign-version".into());
        let before = serde_json::to_vec(&document).unwrap();
        assert!(prepare(&document, &backend()).is_err());
        assert_eq!(serde_json::to_vec(&document).unwrap(), before);
    }

    #[test]
    fn projected_team_new_inherits_working_graph_and_keeps_archived_profile_frozen() {
        let document = software_with_inactive_vulkan();
        let prepared = prepare(
            &module_engine_core::prepare_team_document(&document),
            &backend(),
        )
        .unwrap();
        let profile = EngineProfile::from_document(&prepared).unwrap();
        assert!(
            profile
                .blueprint
                .root
                .nodes
                .iter()
                .any(|node| node.module == "render.vulkan")
        );
        assert!(profile.blueprint.shader_sources["inactive-shader"].contains("missing-inactive"));
        let guard =
            module_engine_script::TestArtifactGuard::new(&std::env::temp_dir(), "runtime-team-new")
                .unwrap();
        let team = crate::distribution::detect(
            &guard.path().join("project.json"),
            &prepared,
            Some(crate::distribution::DistributionProfile::Team),
        )
        .unwrap();
        let fresh = crate::workspace_files::template_document(
            &prepared,
            crate::workspace_files::ProjectTemplate::Empty,
            &team,
        )
        .unwrap();
        assert_eq!(fresh.root, prepared.root);
        assert_eq!(fresh.shader_sources, prepared.shader_sources);
        assert_eq!(fresh.custom_sources, prepared.custom_sources);
        assert_eq!(fresh.engine_profile, prepared.engine_profile);
        assert_eq!(fresh.engine_lock, prepared.engine_lock);
        assert_eq!(fresh.module_versions, prepared.module_versions);
        assert_eq!(
            fresh.game.as_ref().unwrap().execution,
            prepared.game.as_ref().unwrap().execution
        );
        team.validate_document(&fresh).unwrap();
        assert!(
            shader_dependencies(&fresh, guard.path())
                .unwrap()
                .is_empty()
        );
        crate::engine_profiles::validate(&fresh, &backend()).unwrap();
    }
}
