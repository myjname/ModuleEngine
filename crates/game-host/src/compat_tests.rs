//! Original GameHost regressions against the extracted generic host.
use super::test_support::{
    active_graph, aim_at, audio_played, demo_document, game_registry, graphics_only_change,
    modules, new_host, new_host_at, render_graph, retained_render_controls, selected_render_graph,
    validate_candidate_controls, validate_render_settings_at,
};
use super::*;
use module_engine_vulkan::{RenderPath, VulkanRenderer};
#[test]
fn invalid_active_renderer_route_never_falls_back_to_unused_vulkan() {
    let original = demo_document();
    let mut document = original.clone();
    let mut unused = document
        .root
        .nodes
        .iter()
        .find(|node| node.module == "render.vulkan")
        .unwrap()
        .clone();
    unused.id = "unused-renderer".into();
    document.root.nodes.insert(0, unused);
    let output = document
        .root
        .nodes
        .iter()
        .find(|node| node.module == "viewport.output")
        .unwrap()
        .id
        .clone();
    document.root.edges.retain(|edge| edge.to_node != output);
    assert!(selected_render_graph(&document).is_err());
    assert!(render_graph(&document).nodes.is_empty());
    document = original.clone();
    document
        .root
        .nodes
        .iter_mut()
        .find(|node| node.module == "render.vulkan")
        .unwrap()
        .subgraph = None;
    assert!(
        selected_render_graph(&document)
            .unwrap_err()
            .contains("no GPU graph")
    );
    document = original.clone();
    let mut duplicate = document
        .root
        .nodes
        .iter()
        .find(|node| node.module == "viewport.output")
        .unwrap()
        .clone();
    duplicate.id = "another-view".into();
    document.root.nodes.push(duplicate);
    assert!(
        selected_render_graph(&document)
            .unwrap_err()
            .contains("exactly one")
    );
    document = original.clone();
    let edge = document
        .root
        .edges
        .iter()
        .find(|edge| edge.to_node == output && edge.to_port == "frame")
        .unwrap()
        .clone();
    document.root.edges.push(edge);
    assert!(selected_render_graph(&document).is_err());
    document = original;
    document
        .root
        .edges
        .iter_mut()
        .find(|edge| edge.to_node == output && edge.to_port == "frame")
        .unwrap()
        .from_port = "wrong".into();
    assert!(selected_render_graph(&document).is_err());
}
#[test]
fn game_scene_and_dependency_watch_follow_wired_renderer_across_unused_reordering() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut document = demo_document();
    let active = selected_render_graph(&document).unwrap().clone();
    let active_id = document
        .root
        .nodes
        .iter()
        .find(|node| node.module == "render.vulkan")
        .unwrap()
        .id
        .clone();
    let mut alternative = document
        .root
        .nodes
        .iter()
        .find(|node| node.id == active_id)
        .unwrap()
        .clone();
    alternative.id = "unused-renderer".into();
    let alternate = alternative.subgraph.as_mut().unwrap();
    let ao = alternate
        .nodes
        .iter_mut()
        .find(|node| node.id == "ao")
        .unwrap();
    ao.module = "gpu.custom".into();
    ao.params = json!({"source_id":"unused-render-color", "manifest":module_engine_vulkan::custom_compute_manifest(), "gain":0.2});
    alternate
        .edges
        .retain(|edge| edge.to_node != "ao" || edge.to_port == "color");
    let alternate_graph = alternate.as_ref().clone();
    document.shader_sources.insert(
        "unused-render-color".into(),
        module_engine_vulkan::demo_custom_compute_source(),
    );
    let mut scene_edge = document
        .root
        .edges
        .iter()
        .find(|edge| edge.to_node == active_id && edge.to_port == "scene")
        .unwrap()
        .clone();
    scene_edge.to_node = alternative.id.clone();
    document.root.edges.push(scene_edge);
    document.root.nodes.insert(0, alternative);
    assert_eq!(render_graph(&document), active);
    let mut game = new_host(document.clone(), &root, Some(&root), WorldKind::Objects).unwrap();
    game.services.set_enabled(false);
    let game_pixels = game.render(160, 90).unwrap().rgba;
    let snapshot = game.author_snapshot().unwrap();
    let mut alternate_renderer = VulkanRenderer::new_at(root.clone()).unwrap();
    alternate_renderer.set_detailed_stats(false);
    let alternative_pixels = alternate_renderer
        .render(
            &snapshot,
            game.camera,
            160,
            90,
            &alternate_graph,
            &document.shader_sources,
            &RenderOverrides::default(),
        )
        .unwrap()
        .rgba;
    assert_ne!(
        game_pixels, alternative_pixels,
        "the inactive graph must produce visibly different pixels"
    );
    let mut preview_document = document.clone();
    preview_document.game.as_mut().unwrap().entry_scene = "boss".into();
    let mut preview = new_host(
        preview_document.clone(),
        &root,
        Some(&root),
        WorldKind::Objects,
    )
    .unwrap();
    preview.services.set_enabled(false);
    let preview_pixels = preview.render(160, 90).unwrap().rgba;
    assert_eq!(active_graph(&preview), &active);
    let mut watch = RendererDependencyWatch::new(&document, &modules(&root).unwrap());
    document.root.nodes.rotate_left(1); // move the inactive renderer behind the active route
    game.sync_document(&document).unwrap();
    assert_eq!(game.render(160, 90).unwrap().rgba, game_pixels);
    preview_document.root = document.root.clone();
    preview.sync_document(&preview_document).unwrap();
    assert_eq!(preview.render(160, 90).unwrap().rgba, preview_pixels);
    watch.checked = Instant::now() - Duration::from_millis(250);
    assert!(!watch.changed(&document, &modules(&root).unwrap()));
    document.shader_sources.insert(
        "unused-render-color".into(),
        "#include \"nonexistent-inactive.hlsli\"\n".into(),
    );
    watch.checked = Instant::now() - Duration::from_millis(250);
    assert!(
        !watch.changed(&document, &modules(&root).unwrap()),
        "inactive shader dependencies must not own the live watch"
    );
    assert_eq!(render_graph(&document), active);
}
#[test]
#[ignore = "wall-time diagnostic: explicitly measure slow Custom catchup without imposing a new simulation policy"]
fn audit_slow_custom_catchup_wall_time() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut document = demo_document();
    document.custom_sources.insert(
        "game-controller".into(),
        r#"// module-engine-state: {"schema":"audit.delay.v1","initial":{"delay_ms":0,"ticks":0}}
fn update(ctx: &mut ScriptContext) {
    let delay = ctx.state["delay_ms"].as_u64().unwrap_or(0);
    std::thread::sleep(std::time::Duration::from_millis(delay));
    ctx.state["ticks"] = serde_json::json!(ctx.state["ticks"].as_u64().unwrap_or(0) + 1);
}
"#
        .into(),
    );
    let mut host = new_host(document, &root, Some(&root), WorldKind::Objects).unwrap();
    host.services.set_enabled(false);
    host.step(1. / 60., &GameInput::default()).unwrap();
    let mut samples = Vec::new();
    for delay in [8_u64, 80] {
        host.slots["game-controller"]
            .lock()
            .unwrap()
            .migrate_state(json!({"delay_ms":delay,"ticks":0}))
            .unwrap();
        host.step(1. / 60., &GameInput::default()).unwrap(); // start the new worker before timing
        let started = Instant::now();
        let ticks = host.step(0.25, &GameInput::default()).unwrap();
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.;
        assert_eq!(ticks, 15);
        assert!(elapsed_ms >= delay as f64 * ticks as f64);
        samples.push(json!({"custom_delay_ms":delay,"dt":0.25,"fixed_ticks":ticks,"host_step_ms":elapsed_ms,"per_tick_timeout_ms":150}));
    }
    let report = json!({"kind":"synchronous-custom-catchup-diagnostic","samples":samples,"policy":"current fixed-step/catchup semantics retained; no frame-wall-time budget"});
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    std::fs::write(
        root.join(".work/slow-custom-catchup-audit.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}
#[test]
fn host_restart_restores_world_script_render_audio_and_camera_without_recompiling() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut host = new_host(demo_document(), &root, Some(&root), WorldKind::Objects).unwrap();
    host.services.set_enabled(false);
    host.camera.yaw = 0.4;
    host.camera.pitch = 0.8;
    host.camera.distance = 17.;
    host.restart().unwrap();
    let initial = host.session.state().clone();
    let camera = host.camera;
    let pixels = host.render(160, 90).unwrap().rgba;
    let slot = host.slots["game-controller"].clone();
    let executable = slot.lock().unwrap().executable().to_path_buf();
    host.step(
        1. / 60.,
        &module_engine_arpg::ArpgInput {
            movement: [1., 0.],
            ..Default::default()
        }
        .into_input(),
    )
    .unwrap();
    assert!(
        slot.lock().unwrap().state().unwrap()["ticks"]
            .as_u64()
            .unwrap()
            > 0
    );
    host.camera.target = [12., 0., 8.];
    host.services
        .consume(&vec![
            GameEvent::new(
                "Sound",
                json!({"cue":"hit","object":1})
            );
            4
        ])
        .unwrap();
    assert_eq!(audio_played(&host), 4);
    let frame_interface = active_graph(&host)
        .nodes
        .iter()
        .find(|node| node.module == "interface.output")
        .unwrap();
    let frame_edge = active_graph(&host)
        .edges
        .iter()
        .find(|edge| edge.to_node == frame_interface.id && edge.to_port == "value")
        .unwrap();
    let frame_resource = format!("{}.{}", frame_edge.from_node, frame_edge.from_port);
    {
        let mut controls = host.overrides.lock().unwrap();
        controls.enabled.insert("aa".into(), false);
        controls.parameters.insert(
            "tonemap".into(),
            BTreeMap::from([("exposure".into(), json!(0.2))]),
        );
        controls
            .resource_commands
            .push(RenderResourceCommand::ClearResource {
                name: frame_resource,
                value: [0.; 4],
            });
    }
    assert_ne!(host.render(160, 90).unwrap().rgba, pixels);
    let pipelines = host.renderer_performance().pipeline_creations;
    let revision = host.presentation_revision();
    host.restart().unwrap();
    assert_ne!(host.presentation_revision(), revision);
    assert_eq!(host.session.state(), &initial);
    assert_eq!(host.camera, camera);
    assert_eq!(audio_played(&host), 0);
    assert!(!host.services.enabled());
    assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
    assert_eq!(slot.lock().unwrap().executable(), executable);
    let (state, declaration) = {
        let script = slot.lock().unwrap();
        (
            script.state().unwrap(),
            script.state_declaration().initial.clone(),
        )
    };
    assert_eq!(state, declaration);
    assert!(host.overrides.lock().unwrap().enabled.is_empty());
    assert!(host.overrides.lock().unwrap().parameters.is_empty());
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    assert_eq!(host.render(160, 90).unwrap().rgba, pixels);
    assert_eq!(host.renderer_performance().pipeline_creations, pipelines);
    host.overrides
        .lock()
        .unwrap()
        .enabled
        .insert("aa".into(), false);
    assert_eq!(
        host.step(
            0.,
            &GameInput {
                restart: true,
                ..Default::default()
            }
        )
        .unwrap(),
        0
    );
    assert!(host.overrides.lock().unwrap().enabled.is_empty());
    assert_eq!(host.session.state(), &initial);
}
#[test]
fn rejected_shader_dependency_watch_tracks_disk_repairs_without_document_edits() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let relative = format!(".work/shader-rejection-watch-{}.hlsli", std::process::id());
    let header = root.join(&relative);
    std::fs::write(&header, "float gain() { return 0.2; }\n").unwrap();
    let mut document = demo_document();
    let renderer = document
        .root
        .nodes
        .iter_mut()
        .find(|node| node.module == "render.vulkan")
        .unwrap()
        .subgraph
        .as_mut()
        .unwrap();
    let ao = renderer
        .nodes
        .iter_mut()
        .find(|node| node.id == "ao")
        .unwrap();
    ao.module = "gpu.custom".into();
    ao.params = json!({"source_id":"watch","manifest":module_engine_vulkan::custom_compute_manifest(),"gain":1.0});
    renderer
        .edges
        .retain(|edge| edge.to_node != "ao" || edge.to_port == "color");
    document.shader_sources.insert(
        "watch".into(),
        format!(
            "#include \"{relative}\"\n{}",
            module_engine_vulkan::demo_custom_compute_source()
        ),
    );
    let mut watch = RendererDependencyWatch::new(&document, &modules(&root).unwrap());
    std::fs::write(&header, "invalid included code\n").unwrap();
    watch.checked = Instant::now();
    assert!(
        !watch.changed(&document, &modules(&root).unwrap()),
        "no filesystem walk on every UI frame"
    );
    watch.checked = Instant::now() - Duration::from_millis(250);
    assert!(watch.changed(&document, &modules(&root).unwrap()));
    watch.checked = Instant::now() - Duration::from_millis(250);
    assert!(!watch.changed(&document, &modules(&root).unwrap()));
    std::fs::remove_file(&header).unwrap();
    watch.checked = Instant::now() - Duration::from_millis(250);
    assert!(
        watch.changed(&document, &modules(&root).unwrap()),
        "missing include is a distinct dependency state"
    );
    std::fs::write(&header, "float gain() { return 0.8; }\n").unwrap();
    watch.checked = Instant::now() - Duration::from_millis(250);
    assert!(
        watch.changed(&document, &modules(&root).unwrap()),
        "repair must permit retry without changing inline code"
    );
    std::fs::remove_file(&header).unwrap();
}
fn without_aa(mut document: GraphDocument) -> GraphDocument {
    let renderer = document
        .root
        .nodes
        .iter_mut()
        .find(|node| node.module == "render.vulkan")
        .unwrap();
    assert_eq!(
        module_engine_vulkan::remove_pass_with_bypass(renderer.subgraph.as_mut().unwrap(), "aa"),
        Some(1)
    );
    document.engine.exposure.variants.remove("antialiasing");
    document.project.variants.remove("antialiasing");
    document
        .project
        .rules
        .retain(|rule| rule.group != "antialiasing");
    document
}
#[test]
fn retained_render_controls_drop_only_removed_identities_not_fresh_unknown_commands() {
    let old = module_engine_vulkan::default_graph(RenderPath::Forward);
    let mut graph = old.clone();
    module_engine_vulkan::remove_pass_with_bypass(&mut graph, "aa").unwrap();
    let mut controls = RenderOverrides::default();
    controls.enabled.insert("aa".into(), false);
    controls.enabled.insert("new-unknown".into(), true);
    controls.parameters.insert(
        "tonemap".into(),
        BTreeMap::from([("exposure".into(), json!(1.25))]),
    );
    controls.resource_commands = vec![
        RenderResourceCommand::ClearResource {
            name: "aa.color".into(),
            value: [0.; 4],
        },
        RenderResourceCommand::CopyResource {
            from: "tonemap.color".into(),
            to: "aa.color".into(),
        },
        RenderResourceCommand::ClearResource {
            name: "encode.frame".into(),
            value: [0.; 4],
        },
    ];
    let retained = retained_render_controls(&old, &graph, &controls);
    assert!(!retained.enabled.contains_key("aa"));
    assert!(retained.enabled.contains_key("new-unknown"));
    assert_eq!(retained.parameters["tonemap"]["exposure"], 1.25);
    assert_eq!(retained.resource_commands.len(), 1);
    assert!(
        module_engine_vulkan::validate_overrides(&graph, &retained)
            .unwrap_err()
            .contains("Unknown render pass new-unknown")
    );
    assert!(
        controls.enabled.contains_key("aa"),
        "Staging must not mutate last-good controls"
    );
}
#[test]
fn render_settings_validation_includes_unselected_presets_after_pass_removal() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut document = without_aa(demo_document());
    let graph = render_graph(&document);
    validate_render_settings_at(&document, &graph, &root).unwrap();
    document
        .engine
        .exposure
        .variants
        .get_mut("tonemap")
        .unwrap()
        .options
        .insert(
            "unselected-broken".into(),
            ReadyVariant {
                label: "Invalid unselected AA".into(),
                commands: vec![RenderCommand::SetEnabled {
                    pass: "aa".into(),
                    enabled: true,
                }],
            },
        );
    assert!(
        validate_render_settings_at(&document, &graph, &root)
            .unwrap_err()
            .contains("Unknown render pass aa")
    );
    assert!(graphics_only_change(
        &demo_document(),
        &without_aa(demo_document())
    ));
    let mut unrelated = without_aa(demo_document());
    unrelated
        .engine
        .enabled_modules
        .insert("another-feature".into());
    assert!(!graphics_only_change(&demo_document(), &unrelated));
}
#[test]
fn gpu_pass_removal_prunes_controls_only_on_acceptance_and_keeps_live_session() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let document = demo_document();
    let mut host = new_host(document.clone(), &root, Some(&root), WorldKind::Objects).unwrap();
    host.services.set_enabled(false);
    host.camera.target = [3., 0., 2.];
    host.session
        .apply_commands(&[WorldCommand::SetTransform {
            id: 1,
            transform: Transform {
                translation: [4., 1., 2.],
                ..Default::default()
            },
        }])
        .unwrap();
    let state = host.session.state().clone();
    let camera = host.camera;
    let slot = host.slots["game-controller"].clone();
    {
        let mut controls = host.overrides.lock().unwrap();
        controls.enabled.insert("aa".into(), false);
        controls.parameters.insert(
            "tonemap".into(),
            BTreeMap::from([("exposure".into(), json!(1.25))]),
        );
        controls
            .resource_commands
            .push(RenderResourceCommand::ClearResource {
                name: "aa.color".into(),
                value: [0.; 4],
            });
    }
    let cleaned = without_aa(document);
    host.sync_document_deferred(&cleaned).unwrap();
    assert!(host.has_pending_preparation());
    assert!(host.overrides.lock().unwrap().enabled.contains_key("aa"));
    assert!(!host.overrides.lock().unwrap().resource_commands.is_empty());
    let deadline = Instant::now() + Duration::from_secs(60);
    while host.has_pending_preparation() {
        assert!(Instant::now() < deadline);
        host.poll_preparation(&cleaned).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.session.state(), &state);
    assert_eq!(host.camera.target, camera.target);
    assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
    assert!(!host.overrides.lock().unwrap().enabled.contains_key("aa"));
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    assert_eq!(
        host.overrides.lock().unwrap().parameters["tonemap"]["exposure"],
        1.25
    );
    host.overrides.lock().unwrap().parameters.insert(
        "ao".into(),
        BTreeMap::from([("strength".into(), json!(0.4))]),
    );
    let before = host.render(160, 90).unwrap().rgba;
    let mut invalid = cleaned.clone();
    let graph = invalid
        .root
        .nodes
        .iter_mut()
        .find(|node| node.module == "render.vulkan")
        .unwrap()
        .subgraph
        .as_mut()
        .unwrap();
    graph.nodes.retain(|node| node.id != "ao");
    graph
        .edges
        .retain(|edge| edge.from_node != "ao" && edge.to_node != "ao");
    host.sync_document_deferred(&invalid).unwrap();
    let mut rejected = false;
    while host.has_pending_preparation() {
        assert!(Instant::now() < deadline);
        if host.poll_preparation(&invalid).is_err() {
            rejected = true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(rejected);
    assert!(host.sync_document(&invalid).is_err());
    assert!(host.accepted_document().execution_eq(&cleaned));
    assert_eq!(host.session.state(), &state);
    assert_eq!(
        host.overrides.lock().unwrap().parameters["ao"]["strength"],
        0.4
    );
    assert_eq!(host.render(160, 90).unwrap().rgba, before);
    let mut fresh = host.overrides.lock().unwrap().clone();
    fresh
        .apply_commands(&[RenderCommand::SetEnabled {
            pass: "new-unknown".into(),
            enabled: true,
        }])
        .unwrap();
    let retained = retained_render_controls(active_graph(&host), active_graph(&host), &fresh);
    assert!(
        validate_candidate_controls(&cleaned, active_graph(&host), &retained, 0., &root)
            .unwrap_err()
            .contains("Unknown render pass new-unknown")
    );
    assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
}
#[test]
fn scheduled_rust_declares_typed_world_input_and_event_dependencies() {
    let document = demo_document();
    let registry = game_registry(
        &document,
        Path::new("."),
        &BTreeMap::new(),
        Arc::new(Mutex::new(RenderOverrides::default())),
    )
    .unwrap();
    let mut graph = document.game.unwrap().execution;
    registry.validate(&graph).unwrap();
    let node = graph.stages.last_mut().unwrap().nodes.last_mut().unwrap();
    node.bindings.insert("world".into(), "input".into());
    assert!(registry.validate(&graph).is_err());
}
#[test]
fn center_cursor_targets_ground_below_camera() {
    let c = Camera {
        yaw: 0.,
        pitch: 0.95,
        distance: 21.,
        target: [2., 0., -3.],
    };
    let p = aim_at(c, [0., 0.], 16. / 9.);
    assert!((p[0] - 2.).abs() < 0.0001);
    assert!((p[2] + 3.).abs() < 0.0001);
}
#[test]
fn author_preview_reuses_content_and_custom_and_rejects_failed_edits_transactionally() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    for kind in [WorldKind::Objects, WorldKind::Ecs] {
        let mut document = demo_document();
        document
            .root
            .nodes
            .iter_mut()
            .find(|node| matches!(node.module.as_str(), "world.objects" | "world.ecs"))
            .unwrap()
            .module = match kind {
            WorldKind::Objects => "world.objects",
            WorldKind::Ecs => "world.ecs",
            WorldKind::Custom => unreachable!(),
        }
        .into();
        let mut host = new_host(document.clone(), &root, Some(&root), kind).unwrap();
        host.services.set_enabled(false);
        host.camera.target = [1., 0., 1.];
        let camera = host.camera;
        let cached = host
            .content
            .load_model("assets/models/demo_crystal.glb")
            .unwrap();
        let slot = host.slots["game-controller"].clone();
        let executable = slot.lock().unwrap().executable().to_path_buf();
        let owned_state = json!({"ticks":17,"hits":3});
        slot.lock()
            .unwrap()
            .migrate_state(owned_state.clone())
            .unwrap();
        let before_pixels = host.render(240, 135).unwrap().rgba;
        // A graph layout gesture must not reconstruct even a zero-tick
        // session: its retained world may already differ from authored data.
        host.session
            .apply_commands(&[WorldCommand::SetTransform {
                id: 1,
                transform: Transform {
                    translation: [8., 1., 3.],
                    ..Transform::default()
                },
            }])
            .unwrap();
        let retained_state = host.session.state().clone();
        let retained_project = host.session.project().clone();
        let prepared_graph = active_graph(&host).clone();
        let prepared_performance = host.renderer_performance();
        let layout_revision = host.presentation_revision();
        for offset in [15., 30., 45.] {
            document.root.nodes[0].position[0] += offset;
            document.root.nodes[0].label.push_str(" moved");
            let inner = document
                .root
                .nodes
                .iter_mut()
                .find(|node| node.module == "render.vulkan")
                .unwrap()
                .subgraph
                .as_mut()
                .unwrap();
            inner.nodes[0].position[1] += offset;
            host.refresh_author_preview(&document).unwrap();
            assert_eq!(host.presentation_revision(), layout_revision);
            assert_eq!(host.session.state(), &retained_state);
            assert_eq!(host.session.project(), &retained_project);
            assert!(active_graph(&host).execution_eq(&prepared_graph));
            assert_eq!(
                host.renderer_performance().prepare_calls,
                prepared_performance.prepare_calls
            );
            assert_eq!(
                host.renderer_performance().pipeline_creations,
                prepared_performance.pipeline_creations
            );
            assert_eq!(host.document, document);
            assert_eq!(*host.live_document.lock().unwrap(), document);
            assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
            assert_eq!(slot.lock().unwrap().state().unwrap(), owned_state);
            assert!(Arc::ptr_eq(
                &cached,
                &host
                    .content
                    .load_model("assets/models/demo_crystal.glb")
                    .unwrap()
            ));
            assert_eq!(host.camera.target, camera.target);
            assert_eq!(host.camera.distance, camera.distance);
        }
        for x in [1., 2., 3., 4.] {
            let objects = &mut document
                .game
                .as_mut()
                .unwrap()
                .scenes
                .get_mut("arena")
                .unwrap()
                .objects;
            let player = objects
                .iter_mut()
                .find(|object| object.object.id == 1)
                .unwrap();
            player.object.transform.translation = [x, 1., 1.];
            player.object.transform.rotation = [0., x * 0.1, 0.];
            player.object.transform.scale = [1.3, 1.3, 1.3];
            let expected_transform = player.object.transform;
            let previous_revision = host.presentation_revision();
            host.refresh_author_preview(&document).unwrap();
            assert_ne!(host.presentation_revision(), previous_revision);
            assert_eq!(
                host.session.state().world.object(1).unwrap().transform,
                expected_transform
            );
            assert!(Arc::ptr_eq(
                &cached,
                &host
                    .content
                    .load_model("assets/models/demo_crystal.glb")
                    .unwrap()
            ));
            assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
            assert_eq!(slot.lock().unwrap().executable(), executable);
            assert_eq!(slot.lock().unwrap().state().unwrap(), owned_state);
            assert_eq!(host.camera.target, camera.target);
            assert_eq!(host.camera.distance, camera.distance);
        }
        let objects = &mut document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects;
        objects.push(AuthoredObject::new(WorldObject {
            id: 5000,
            parent: None,
            transform: Transform {
                translation: [-2., 0., 0.],
                scale: [0.1; 3],
                ..Transform::default()
            },
            color: [100, 100, 100, 255],
        }));
        // Gameplay actors intentionally require root transforms. A mesh
        // without actor/collision components exercises editable hierarchy.
        let mut child = AuthoredObject::new(WorldObject {
            id: 5001,
            parent: None,
            transform: Transform {
                translation: [1., 1., -1.],
                scale: [1.5; 3],
                ..Transform::default()
            },
            color: [240, 140, 80, 255],
        });
        child.components.insert(
            "mesh".into(),
            json!({"path":"assets/models/demo_crystal.glb"}),
        );
        objects.push(child);
        host.refresh_author_preview(&document).unwrap();
        for parent in [Some(5000), None, Some(5000)] {
            document
                .game
                .as_mut()
                .unwrap()
                .scenes
                .get_mut("arena")
                .unwrap()
                .objects
                .iter_mut()
                .find(|object| object.object.id == 5001)
                .unwrap()
                .object
                .parent = parent;
            host.refresh_author_preview(&document).unwrap();
            assert_eq!(
                host.session.state().world.object(5001).unwrap().parent,
                parent
            );
        }
        let edited_scene = document.game.as_ref().unwrap().scenes["arena"].clone();
        document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .insert("after-edit".into(), edited_scene);
        host.refresh_author_preview(&document).unwrap();
        let expected = multiply_matrices(
            document.game.as_ref().unwrap().scenes["arena"]
                .objects
                .iter()
                .find(|object| object.object.id == 5000)
                .unwrap()
                .object
                .transform
                .matrix(),
            document.game.as_ref().unwrap().scenes["arena"]
                .objects
                .iter()
                .find(|object| object.object.id == 5001)
                .unwrap()
                .object
                .transform
                .matrix(),
        );
        assert_eq!(
            host.session
                .snapshot()
                .items
                .iter()
                .find(|item| item.id == 5001)
                .unwrap()
                .model,
            expected
        );
        assert_ne!(host.render(240, 135).unwrap().rgba, before_pixels);
        let accepted_state = host.session.state().clone();
        let accepted_project = host.session.project().clone();
        let accepted_document = host.document.clone();
        let mut invalid = document.clone();
        invalid
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects
            .iter_mut()
            .find(|object| object.object.id == 5000)
            .unwrap()
            .object
            .parent = Some(5001);
        assert!(host.refresh_author_preview(&invalid).is_err());
        let mut missing = document.clone();
        missing
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("boss")
            .unwrap()
            .objects
            .iter_mut()
            .find(|object| object.object.id == 1)
            .unwrap()
            .components
            .insert(
                "mesh".into(),
                json!({"path":"assets/missing-preview-model.glb"}),
            );
        assert!(host.refresh_author_preview(&missing).is_err());
        let mut structural = document.clone();
        structural
            .game
            .as_mut()
            .unwrap()
            .execution
            .stages
            .last_mut()
            .unwrap()
            .nodes
            .last_mut()
            .unwrap()
            .params["source_id"] = json!("different-controller");
        assert!(host.refresh_author_preview(&structural).is_err());
        assert_eq!(host.session.state(), &accepted_state);
        assert_eq!(host.session.project(), &accepted_project);
        assert_eq!(host.document, accepted_document);
        assert_eq!(*host.live_document.lock().unwrap(), accepted_document);
        assert!(Arc::ptr_eq(
            &cached,
            &host
                .content
                .load_model("assets/models/demo_crystal.glb")
                .unwrap()
        ));
        assert!(Arc::ptr_eq(&slot, &host.slots["game-controller"]));
        assert_eq!(slot.lock().unwrap().state().unwrap(), owned_state);
        host.session
            .apply_commands(&[WorldCommand::SetTransform {
                id: 1,
                transform: Transform::default(),
            }])
            .unwrap();
        host.session.restart().unwrap();
        assert_eq!(host.session.state(), &accepted_state);
        host.session.transition("after-edit").unwrap();
        assert_eq!(host.session.state().scene, "after-edit");
        // A reset to the accepted authored entry scene also keeps the latest
        // reparenting and transform, without compiling/replacing Custom.
        host.session.restart().unwrap();
        assert_eq!(host.session.state(), &accepted_state);
        host.step(0., &GameInput::default()).unwrap();
        assert!(
            host.refresh_author_preview(&document)
                .unwrap_err()
                .contains("during Play")
        );
    }
}
#[test]
fn relocated_game_uses_project_assets_and_installed_backend_for_rust_render_controls() {
    let backend = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let assets = backend.join(format!(
        ".work/test-temp/game-separated-roots-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(assets.join("assets")).unwrap();
    std::fs::write(
        assets.join("assets/project-marker.txt"),
        "relocated project assets",
    )
    .unwrap();
    let mut document = demo_document();
    let game = document.game.as_ref().unwrap();
    let mut files = game.audio_cues.values().cloned().collect::<Vec<_>>();
    files.push("assets/models/demo_crystal.glb".into());
    for relative in files {
        let target = assets.join(&relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(backend.join(relative), target).unwrap();
    }
    document.custom_sources.insert("game-controller".into(), r#"// module-engine-state: {"schema":"project.assets.v1","initial":{"ticks":0,"marker":""}}
fn update(ctx: &mut ScriptContext) {
    ctx.state["ticks"] = serde_json::json!(ctx.state["ticks"].as_u64().unwrap() + 1);
    ctx.state["marker"] = serde_json::json!(std::fs::read_to_string("assets/project-marker.txt").unwrap());
    ctx.set_render_parameter("tonemap", "exposure", 1.25);
}
"#.into());
    assert!(!assets.join("tools/dxc/dxc.exe").exists());

    let mut host = new_host_at(
        document.clone(),
        &assets,
        &backend,
        Some(&backend),
        WorldKind::Objects,
    )
    .unwrap();
    host.services.set_enabled(false);
    assert_eq!(host.root, assets.canonicalize().unwrap());
    assert_eq!(host.backend_root, backend);
    assert_ne!(host.root, host.backend_root);
    host.step(1. / 60., &GameInput::default()).unwrap();
    let state = host.slots["game-controller"]
        .lock()
        .unwrap()
        .state()
        .unwrap();
    assert_eq!(state["marker"], "relocated project assets");
    assert_eq!(state["ticks"], 1);
    assert_eq!(
        host.overrides.lock().unwrap().parameters["tonemap"]["exposure"],
        1.25
    );
    let frame = host.render(160, 90).unwrap();
    assert_eq!([frame.width, frame.height], [160, 90]);
    assert!(!frame.rgba.is_empty());
    let edited = document.custom_sources["game-controller"].replace(" + 1", " + 2");
    let candidate = ScriptCompiler::new(backend.clone(), backend.join(".work/game-scripts"))
        .compile_kind(&edited, ScriptKind::Update)
        .unwrap();
    host.apply_candidate("game-controller", candidate, false)
        .unwrap();
    host.step(1. / 60., &GameInput::default()).unwrap();
    let state = host.slots["game-controller"]
        .lock()
        .unwrap()
        .state()
        .unwrap();
    assert_eq!(state["marker"], "relocated project assets");
    assert_eq!(state["ticks"], 3);
    assert!(!assets.join("tools").exists());
    drop(host);
    let allowed = backend.join(".work/test-temp").canonicalize().unwrap();
    let assets = assets.canonicalize().unwrap();
    assert!(assets.starts_with(allowed));
    std::fs::remove_dir_all(assets).unwrap();
}
