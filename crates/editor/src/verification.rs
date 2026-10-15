//! End-to-end checks through the editor composition root and portable Runtime.
//! This command requires a real Vulkan adapter and the pinned local Rust/DXC tools.
use module_engine_core::{
    Camera, ControlCommand, Frame, Graph, GraphDocument, ModuleRegistry, RenderCommand, Runtime,
    ScriptContext, WorldCommand, WorldKind, make_edge, make_node,
};
use module_engine_custom::{CommandNodeKind, CommandProgram};
use module_engine_project::{CheckpointStore, SnapshotHistory, diff_documents, merge_documents};
use module_engine_script::{ScriptCompiler, ScriptHost};
use module_engine_vulkan::{
    RenderPath, custom_compute_manifest, custom_raster_manifest, demo_custom_compute_source,
    demo_custom_raster_source,
};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const FRAMES: usize = 30;
const DT: f32 = 1.0 / 60.0;
const CONTROLLER: &str = r#"fn update(ctx: &mut ScriptContext) {
    // Пример: source comments and opaque arithmetic survive visual literal edits.
    let x = 0.35 + (ctx.time * 0.7).sin() as f32;
    ctx.set_position(1, [x, 0.25, 0.0]);
    ctx.set_color(1, [245, 75, 125, 255]);
    ctx.set_render_parameter("tonemap", "exposure", 1.4);
}
"#;

/// Execute acceptance checks through the same registry/renderer host as the UI.
/// Generated examples and evidence are confined to the requested output folder.
pub fn integration_self_check(output: &Path) -> Result<Value, String> {
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let registry = crate::host::registry()?;
    let mut forward_document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward);
    let mut deferred_document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Deferred);
    crate::project::stamp_module_versions(&mut forward_document, &registry);
    crate::project::stamp_module_versions(&mut deferred_document, &registry);
    save_document(output, "gpu-forward.json", &forward_document)?;
    save_document(output, "gpu-deferred.json", &deferred_document)?;

    let mut objects = create_runtime(&forward_document, &registry)?;
    let mut ecs = create_runtime(
        &crate::host::gpu_document(WorldKind::Ecs, RenderPath::Forward),
        &registry,
    )?;
    advance(&mut objects);
    advance(&mut ecs);
    check(
        objects.snapshot() == ecs.snapshot(),
        "World stores differ before GPU rendering",
    )?;
    let forward = render(&objects)?;
    let forward_ecs = render(&ecs)?;
    check(
        forward.rgba == forward_ecs.rgba,
        "Forward renderer differs between Object/ECS worlds",
    )?;
    check(
        forward.stats.shaded_pixels > 1000
            && forward.stats.visible_objects >= 3
            && forward.stats.triangles > 0,
        "Registered GPU renderer did not draw depth-tested geometry",
    )?;
    crate::write_png(&output.join("forward-objects.png"), &forward)?;
    crate::write_png(&output.join("forward-ecs.png"), &forward_ecs)?;

    let mut deferred_objects = create_runtime(&deferred_document, &registry)?;
    let mut deferred_ecs = create_runtime(
        &crate::host::gpu_document(WorldKind::Ecs, RenderPath::Deferred),
        &registry,
    )?;
    advance(&mut deferred_objects);
    advance(&mut deferred_ecs);
    let deferred = render(&deferred_objects)?;
    let deferred_ecs_frame = render(&deferred_ecs)?;
    check(
        deferred_objects.snapshot() == deferred_ecs.snapshot()
            && deferred.rgba == deferred_ecs_frame.rgba,
        "Deferred renderer differs between Object/ECS worlds",
    )?;
    let deferred_pixels = changed_pixels(&forward, &deferred)?;
    check(
        deferred_pixels > 100,
        "Forward/deferred executable pipelines produced no meaningful difference",
    )?;
    crate::write_png(&output.join("deferred-objects.png"), &deferred)?;
    crate::write_png(&output.join("deferred-ecs.png"), &deferred_ecs_frame)?;

    // A real graph edit must retain the already advanced provider, including time.
    let before = objects.snapshot();
    let before_objects = objects.objects()?;
    check(
        before.time > 0.0,
        "Preservation check needs an advanced world",
    )?;
    let mut tone_document = forward_document.clone();
    pass_graph(&mut tone_document)?
        .nodes
        .iter_mut()
        .find(|n| n.id == "tonemap")
        .ok_or("Missing tone pass")?
        .params["mode"] = json!("reinhard");
    check(
        objects
            .reconfigure(&tone_document)
            .map_err(|e| e.to_string())?,
        "Renderer edit reset the world",
    )?;
    check(
        objects.snapshot() == before && objects.objects()? == before_objects,
        "Renderer graph edit changed time or object state",
    )?;
    let reinhard = render(&objects)?;
    let tone_pixels = changed_pixels(&forward, &reinhard)?;
    check(
        tone_pixels > 100,
        "Tone mapper graph edit did not change GPU pixels",
    )?;
    crate::write_png(&output.join("forward-reinhard.png"), &reinhard)?;

    let compute_document = custom_compute_document(&forward_document)?;
    check(
        objects
            .reconfigure(&compute_document)
            .map_err(|e| e.to_string())?,
        "Custom compute activation reset the world",
    )?;
    check(
        objects.snapshot() == before,
        "Custom compute activation changed world time/state",
    )?;
    let compute = render(&objects)?;
    let compute_pixels = changed_pixels(&forward, &compute)?;
    check(
        compute_pixels > 100,
        "Custom compute dispatch did not affect pixels",
    )?;
    save_document(output, "gpu-custom-compute.json", &compute_document)?;
    crate::write_png(&output.join("custom-compute.png"), &compute)?;

    // Reject the complete Runtime candidate. The authored invalid text remains
    // available as a project, while the previous renderer and provider stay live.
    let mut bad_shader_document = compute_document.clone();
    let invalid_hlsl = "// Retained invalid shader candidate\nthis is invalid HLSL\n";
    bad_shader_document
        .shader_sources
        .insert("custom_compute".into(), invalid_hlsl.into());
    let shader_error = match objects.reconfigure(&bad_shader_document) {
        Ok(_) => return Err("Invalid HLSL Runtime candidate was accepted".into()),
        Err(error) => error.to_string(),
    };
    check(
        shader_error.contains("HLSL compilation failed"),
        "Invalid shader lost the real DXC diagnostic",
    )?;
    check(
        objects.snapshot() == before && render(&objects)?.rgba == compute.rgba,
        "Invalid shader candidate changed active state or output",
    )?;
    save_document(
        output,
        "rejected-shader-candidate.json",
        &bad_shader_document,
    )?;
    fs::write(output.join("rejected-shader-diagnostic.txt"), &shader_error)
        .map_err(|e| e.to_string())?;

    let mut raster_document = custom_raster_document(&forward_document)?;
    check(
        objects
            .reconfigure(&raster_document)
            .map_err(|e| e.to_string())?,
        "Custom scene raster activation reset world",
    )?;
    let raster = render(&objects)?;
    check(
        raster.stats.shaded_pixels > 1000,
        "Custom scene raster did not draw geometry",
    )?;
    save_document(output, "gpu-custom-raster.json", &raster_document)?;
    crate::write_png(&output.join("custom-raster.png"), &raster)?;
    pass_graph(&mut raster_document)?
        .nodes
        .iter_mut()
        .find(|n| n.id == "geometry")
        .ok_or("Missing geometry pass")?
        .params["shift"] = json!(1.2);
    objects
        .reconfigure(&raster_document)
        .map_err(|e| e.to_string())?;
    let shifted = render(&objects)?;
    let vertex_pixels = changed_pixels(&raster, &shifted)?;
    check(
        vertex_pixels > 100,
        "Custom vertex shader did not displace actual GPU geometry",
    )?;
    crate::write_png(&output.join("custom-vertex-shift.png"), &shifted)?;
    let original = raster_document
        .shader_sources
        .get("custom_scene")
        .ok_or("Missing raster source")?;
    let edited = original.replace("float3(0.4,1.0,0.6)", "float3(1.0,0.2,0.2)");
    check(
        edited != *original,
        "Fragment source proof did not actually edit HLSL",
    )?;
    raster_document
        .shader_sources
        .insert("custom_scene".into(), edited);
    objects
        .reconfigure(&raster_document)
        .map_err(|e| e.to_string())?;
    let recolored = render(&objects)?;
    let fragment_pixels = changed_pixels(&shifted, &recolored)?;
    check(
        fragment_pixels > 100,
        "Edited fragment HLSL compiled but did not change GPU output",
    )?;
    check(
        objects.snapshot() == before,
        "HLSL edits changed simulation time/state",
    )?;
    crate::write_png(&output.join("custom-fragment-edit.png"), &recolored)?;
    save_document(output, "gpu-custom-raster-edited.json", &raster_document)?;

    // Exercise both native resource operations through Runtime control dispatch.
    objects
        .reconfigure(&forward_document)
        .map_err(|e| e.to_string())?;
    objects.apply_controls(&[ControlCommand::Render(RenderCommand::SetEnabled {
        pass: "ao".into(),
        enabled: false,
    })])?;
    let ao_bypass = render(&objects)?;
    objects.apply_controls(&[
        ControlCommand::Render(RenderCommand::SetEnabled {
            pass: "ao".into(),
            enabled: true,
        }),
        ControlCommand::Render(RenderCommand::CopyResource {
            from: "geometry.color".into(),
            to: "ao.color".into(),
        }),
    ])?;
    check(
        render(&objects)?.rgba == ao_bypass.rgba,
        "Native resource copy differs from equivalent AO bypass",
    )?;
    objects.begin_frame();
    check(
        render(&objects)?.rgba == forward.rgba,
        "begin_frame failed to clear transient resource commands",
    )?;
    objects.apply_controls(&[ControlCommand::Render(RenderCommand::ClearResource {
        name: "encode.frame".into(),
        value: [1.0, 0.0, 0.0, 1.0],
    })])?;
    let cleared = render(&objects)?;
    check(
        cleared
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 0, 0, 255]),
        "Native resource clear did not reach the final GPU frame",
    )?;
    objects.begin_frame();
    check(
        render(&objects)?.rgba == forward.rgba,
        "Transient clear remained active in next frame",
    )?;

    // A queued resource operation belongs to the old frame, even when all pass
    // IDs survive a candidate commit. Compatible persistent controls survive.
    let override_state = objects.snapshot();
    objects.apply_controls(&[exposure(0.45)])?;
    let compatible_control_frame = render(&objects)?;
    objects.apply_controls(&[ControlCommand::Render(RenderCommand::ClearResource {
        name: "encode.frame".into(),
        value: [1.0, 0.0, 0.0, 1.0],
    })])?;
    check(
        objects
            .reconfigure(&forward_document)
            .map_err(|e| e.to_string())?,
        "Compatible render-control adoption reset world",
    )?;
    check(
        objects.snapshot() == override_state
            && render(&objects)?.rgba == compatible_control_frame.rgba,
        "Candidate commit lost compatible exposure or replayed old resource commands",
    )?;

    // Removing a controlled pass must not install stale overrides which make
    // the replacement renderer fail after the candidate was already accepted.
    objects.apply_controls(&[
        ControlCommand::Render(RenderCommand::SetParameter {
            pass: "ao".into(),
            name: "strength".into(),
            value: json!(0.1),
        }),
        ControlCommand::Render(RenderCommand::ClearResource {
            name: "encode.frame".into(),
            value: [1.0, 0.0, 0.0, 1.0],
        }),
    ])?;
    let pending_clear = render(&objects)?;
    check(
        pending_clear
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 0, 0, 255]),
        "Stale-override regression did not actually queue a resource action",
    )?;
    let replacement = custom_raster_document(&forward_document)?;
    check(
        objects
            .reconfigure(&replacement)
            .map_err(|e| e.to_string())?,
        "Removing a controlled pass reset world",
    )?;
    check(
        objects.snapshot() == override_state && render(&objects)?.rgba == raster.rgba,
        "Replacement graph retained removed-pass overrides or previous resource actions",
    )?;

    // The graph edit changes only a literal span; Rust remains ordinary source.
    let mut program = CommandProgram::parse(CONTROLLER).map_err(|e| e.to_string())?;
    let exposure_node = program
        .nodes()
        .iter()
        .find_map(|n| match n.kind {
            CommandNodeKind::Number { value: 1.4 } => Some(n.id.clone()),
            _ => None,
        })
        .ok_or("No editable exposure literal in command graph")?;
    let expected_source = CONTROLLER.replacen("exposure\", 1.4", "exposure\", 0.45", 1);
    program
        .set_number(&exposure_node, 0.45)
        .map_err(|e| e.to_string())?;
    check(
        program.source() == expected_source,
        "Visual command edit lost comments or opaque Rust",
    )?;
    let source = program.source().to_owned();
    let mut controller_document = controller_document(&forward_document, &source)?;
    let controller = controller_document
        .root
        .nodes
        .iter_mut()
        .find(|n| n.id == "controller")
        .ok_or("Missing controller node")?;
    controller.params["code_layout"] = json!({exposure_node: [123.0, 456.0]});
    crate::project::stamp_module_versions(&mut controller_document, &registry);
    save_document(output, "gpu-controller.json", &controller_document)?;
    let mut controller_ecs_document = controller_document.clone();
    controller_ecs_document
        .root
        .nodes
        .iter_mut()
        .find(|n| n.id == "world")
        .ok_or("Missing ECS world")?
        .module = "world.ecs".into();
    save_document(output, "gpu-controller-ecs.json", &controller_ecs_document)?;
    let mut controlled_objects = create_runtime(&controller_document, &registry)?;
    let mut controlled_ecs = create_runtime(&controller_ecs_document, &registry)?;
    for runtime in [&controlled_objects, &controlled_ecs] {
        let routes = runtime.command_sources();
        check(
            routes.len() == 1
                && routes[0].source_id == "controller"
                && routes[0].world
                && routes[0].render,
            "One connected Rust source must feed both command routes exactly once",
        )?;
    }
    advance(&mut controlled_objects);
    advance(&mut controlled_ecs);
    let script_time = controlled_objects.snapshot().time;
    let original_objects = controlled_objects.objects()?;
    let compiler = ScriptCompiler::new(
        crate::project::source_root(),
        crate::project::source_root().join(".work/integration-scripts"),
    );
    let mut script_host = ScriptHost::new(compiler);
    script_host
        .compile_and_apply(&source)
        .map_err(|e| e.to_string())?;
    let active = script_host
        .active()
        .ok_or("Compiled Rust worker not activated")?;
    check(
        active.executable().is_file() && active.source() == source,
        "Rust source was not retained as an actual compiled worker",
    )?;
    let worker = active.executable().display().to_string();
    let dependency_versions = active.dependency_versions().to_vec();
    let packet = ScriptContext::new(
        f64::from(script_time),
        f64::from(DT),
        original_objects.clone(),
    );
    let tick = script_host
        .tick(&packet, Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let ecs_tick = script_host
        .tick(
            &ScriptContext::new(
                f64::from(controlled_ecs.snapshot().time),
                f64::from(DT),
                controlled_ecs.objects()?,
            ),
            Duration::from_secs(5),
        )
        .map_err(|e| e.to_string())?;
    check(
        tick.commands == ecs_tick.commands && tick.commands.len() == 3,
        "Compiled Rust update did not produce identical world/render commands",
    )?;
    controlled_objects.apply_controls(&tick.commands)?;
    controlled_ecs.apply_controls(&ecs_tick.commands)?;
    let object = controlled_objects
        .objects()?
        .into_iter()
        .find(|o| o.id == 1)
        .ok_or("Script target disappeared")?;
    check(
        object.color == [245, 75, 125, 255]
            && (object.transform.translation[0] - (0.35 + (script_time * 0.7).sin())).abs() < 1e-6,
        "Compiled Rust worker did not set the actual provider transform/color",
    )?;
    check(
        controlled_objects.objects()? != original_objects
            && controlled_objects.snapshot() == controlled_ecs.snapshot()
            && controlled_objects.snapshot().time == script_time,
        "Rust world controls reset time or differed between stores",
    )?;
    let controlled = render(&controlled_objects)?;
    let controlled_ecs_frame = render(&controlled_ecs)?;
    check(
        controlled.rgba == controlled_ecs_frame.rgba,
        "Same compiled script produced different GPU frames between worlds",
    )?;
    check(
        changed_pixels(&forward, &controlled)? > 100,
        "Rust controls did not change rendered output",
    )?;
    crate::write_png(&output.join("rust-controller-objects.png"), &controlled)?;
    crate::write_png(
        &output.join("rust-controller-ecs.png"),
        &controlled_ecs_frame,
    )?;
    controlled_objects.apply_controls(&[exposure(1.0)])?;
    let bright = render(&controlled_objects)?;
    let exposure_pixels = changed_pixels(&controlled, &bright)?;
    check(
        exposure_pixels > 100,
        "Compiled Rust exposure command did not change GPU pixels independently of world controls",
    )?;
    controlled_objects.apply_controls(&[exposure(0.45)])?;
    check(
        render(&controlled_objects)?.rgba == controlled.rgba,
        "Restoring exposure failed to restore exact GPU output",
    )?;

    let invalid_rust = "fn update(ctx: &mut ScriptContext) { let value: f64 = \"type error\"; ctx.set_render_parameter(\"tonemap\", \"exposure\", value); }\n";
    let rust_error = match script_host.compile_and_apply(invalid_rust) {
        Ok(()) => return Err("Rust type-error candidate compiled successfully".into()),
        Err(error) => error.to_string(),
    };
    check(
        rust_error.contains("mismatched types")
            && script_host.active_source() == Some(source.as_str()),
        "Failed Rust candidate lost diagnostics or replaced the active source",
    )?;
    check(
        script_host
            .tick(&packet, Duration::from_secs(5))
            .map_err(|e| e.to_string())?
            .commands
            == tick.commands,
        "Failed Rust candidate discarded the executable worker",
    )?;
    fs::write(output.join("rejected-rust-source.rs"), invalid_rust).map_err(|e| e.to_string())?;
    fs::write(output.join("rejected-rust-diagnostic.txt"), &rust_error)
        .map_err(|e| e.to_string())?;

    // A mixed packet is all-or-nothing, even when its world half is valid.
    let before_invalid = controlled_objects.snapshot();
    let before_invalid_objects = controlled_objects.objects()?;
    let invalid_batch_error = controlled_objects
        .apply_controls(&[
            ControlCommand::World(WorldCommand::Translate {
                id: 1,
                delta: [50.0, 0.0, 0.0],
            }),
            ControlCommand::Render(RenderCommand::SetParameter {
                pass: "missing_pass".into(),
                name: "exposure".into(),
                value: json!(0.1),
            }),
        ])
        .err()
        .ok_or("Invalid mixed world/render packet was accepted")?;
    check(
        controlled_objects.snapshot() == before_invalid
            && controlled_objects.objects()? == before_invalid_objects
            && render(&controlled_objects)?.rgba == controlled.rgba,
        "Rejected mixed packet partially mutated world or active render controls",
    )?;

    // Runtime::adopt_world is the UI candidate-commit path; check the direct API
    // after live Rust edits, rather than only before world controls exist.
    let mut next = create_runtime(&tone_document, &registry)?;
    check(
        next.adopt_world(&mut controlled_objects),
        "Candidate commit did not adopt unchanged world provider",
    )?;
    check(
        next.snapshot() == before_invalid && next.objects()? == before_invalid_objects,
        "Candidate commit discarded script-modified state or simulation time",
    )?;
    let adopted = render(&next)?;
    check(
        changed_pixels(&controlled, &adopted)? > 100,
        "Adopted world did not execute the new tone graph",
    )?;

    let persistence =
        persistence_checks(output, &registry, &controller_document, &compute_document)?;
    let report = json!({
        "backend": objects.backend_name(), "frame_size": [WIDTH, HEIGHT], "simulation_time": before.time,
        "registered_runtime": true, "objects_ecs_forward_equal": true, "objects_ecs_deferred_equal": true,
        "world_state_and_time_preserved": true, "invalid_hlsl_candidate_retained": true,
        "custom_compute_dispatch": true, "custom_geometry_vertex_and_fragment": true,
        "native_resource_copy_and_clear": true, "resource_actions_one_frame": true,
        "compiled_rust_tick_world_and_render": true, "rust_worlds_and_gpu_frames_equal": true,
        "visual_literal_edit_source_preserved": true, "invalid_rust_candidate_retained": true,
        "invalid_mixed_batch_atomic": true, "candidate_adopt_world_preserved": true, "adoption_retains_compatible_controls":true, "removed_pass_overrides_and_resource_actions_cleared":true,
        "changed_pixels": {"forward_deferred":deferred_pixels,"tone_modes":tone_pixels,"custom_compute":compute_pixels,
            "custom_vertex":vertex_pixels,"custom_fragment":fragment_pixels,"rust_exposure":exposure_pixels},
        "worker":worker, "worker_dependencies":dependency_versions, "gpu_stats":forward.stats,
        "persistence":persistence, "shader_rejection":shader_error, "mixed_batch_rejection":invalid_batch_error,
        "examples":["gpu-forward.json","gpu-deferred.json","gpu-controller.json","gpu-controller-ecs.json",
            "gpu-custom-compute.json","gpu-custom-raster.json","gpu-custom-raster-edited.json"]
    });
    fs::write(
        output.join("integration-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}

fn check(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn create_runtime(document: &GraphDocument, registry: &ModuleRegistry) -> Result<Runtime, String> {
    Runtime::from_document(document, registry).map_err(|e| e.to_string())
}
fn advance(runtime: &mut Runtime) {
    for _ in 0..FRAMES {
        runtime.step(DT, 1.0);
    }
}
fn render(runtime: &Runtime) -> Result<Frame, String> {
    runtime.render(WIDTH, HEIGHT, Camera::default())
}
fn changed_pixels(a: &Frame, b: &Frame) -> Result<usize, String> {
    check(
        a.width == b.width && a.height == b.height && a.rgba.len() == b.rgba.len(),
        "Cannot compare differently sized GPU frames",
    )?;
    Ok(a.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0.iter())
        .filter(|(a, b)| a != b)
        .count())
}
fn pass_graph(document: &mut GraphDocument) -> Result<&mut Graph, String> {
    document
        .root
        .nodes
        .iter_mut()
        .find(|n| n.id == "renderer")
        .and_then(|n| n.subgraph.as_deref_mut())
        .ok_or_else(|| "Vulkan pass graph missing".into())
}
fn custom_compute_document(base: &GraphDocument) -> Result<GraphDocument, String> {
    let mut document = base.clone();
    document.name = "Custom HLSL compute / Vulkan".into();
    let graph = pass_graph(&mut document)?;
    let node = graph
        .nodes
        .iter_mut()
        .find(|n| n.id == "ao")
        .ok_or("AO pass missing")?;
    node.module = "gpu.custom".into();
    node.label = "Editable Custom compute".into();
    node.params =
        json!({"source_id":"custom_compute","gain":0.25,"manifest":custom_compute_manifest()});
    graph
        .edges
        .retain(|e| e.to_node != "ao" || e.to_port == "color");
    document
        .shader_sources
        .insert("custom_compute".into(), demo_custom_compute_source());
    Ok(document)
}
fn custom_raster_document(base: &GraphDocument) -> Result<GraphDocument, String> {
    let mut document = base.clone();
    document.name = "Custom HLSL scene raster / Vulkan".into();
    let graph = pass_graph(&mut document)?;
    graph.nodes.retain(|n| n.id != "ao");
    graph
        .edges
        .retain(|e| e.from_node != "ao" && e.to_node != "ao");
    graph
        .edges
        .push(make_edge("geometry", "color", "tonemap", "color"));
    let node = graph
        .nodes
        .iter_mut()
        .find(|n| n.id == "geometry")
        .ok_or("Geometry pass missing")?;
    node.module = "gpu.custom".into();
    node.label = "Editable Custom scene raster".into();
    node.params =
        json!({"source_id":"custom_scene","shift":0.0,"manifest":custom_raster_manifest()});
    document
        .shader_sources
        .insert("custom_scene".into(), demo_custom_raster_source());
    Ok(document)
}
fn controller_document(base: &GraphDocument, source: &str) -> Result<GraphDocument, String> {
    let mut document = base.clone();
    document.name = "Compiled Rust world + GPU controller".into();
    document
        .custom_sources
        .insert("controller".into(), source.into());
    document.root.nodes.push(make_node(
        "controller",
        "custom.rust.update",
        "Compiled Rust controller",
        [150.0, 470.0],
        json!({"source_id":"controller"}),
    ));
    document
        .root
        .edges
        .push(make_edge("controller", "world", "world", "commands"));
    document
        .root
        .edges
        .push(make_edge("controller", "render", "renderer", "commands"));
    Ok(document)
}
fn exposure(value: f64) -> ControlCommand {
    ControlCommand::Render(RenderCommand::SetParameter {
        pass: "tonemap".into(),
        name: "exposure".into(),
        value: json!(value),
    })
}
fn save_document(output: &Path, name: &str, document: &GraphDocument) -> Result<(), String> {
    crate::project::save(&output.join(name), document)
}
fn persistence_checks(
    output: &Path,
    registry: &ModuleRegistry,
    controller: &GraphDocument,
    compute: &GraphDocument,
) -> Result<Value, String> {
    let mut base = controller.clone();
    base.shader_sources = compute.shader_sources.clone();
    crate::project::stamp_module_versions(&mut base, registry);
    let mut edited = base.clone();
    edited
        .custom_sources
        .get_mut("controller")
        .ok_or("Missing command source")?
        .push_str("// saved draft: 世界\n");
    edited
        .shader_sources
        .get_mut("custom_compute")
        .ok_or("Missing shader source")?
        .push_str("// saved HLSL comment\n");
    edited
        .root
        .nodes
        .iter_mut()
        .find(|n| n.id == "controller")
        .ok_or("Missing command node")?
        .params["code_layout"]["extra_saved_position"] = json!([777.0, 888.0]);
    let mut history = SnapshotHistory::new(base.clone(), 8);
    check(
        history.record("Rust/HLSL draft and code position", &edited),
        "History ignored an actual source/layout edit",
    )?;
    check(
        history.undo() == Some(base.clone()) && history.redo() == Some(edited.clone()),
        "Undo/redo did not preserve complete source/layout documents",
    )?;
    let path = output.join("source-roundtrip.json");
    save_document(output, "source-roundtrip.json", &edited)?;
    let loaded = crate::project::read(&path)?;
    check(
        loaded == edited,
        "Project save/load lost source, metadata, versions or code positions",
    )?;
    let checkpoints = CheckpointStore::new(output.join("checkpoints"));
    let checkpoint = checkpoints.save("Integration verified Rust/HLSL/layout snapshot", &edited)?;
    check(
        checkpoints.load(&checkpoint.id)? == edited,
        "Checkpoint lost authored source/layout",
    )?;
    let mut ours = base.clone();
    let mut theirs = base.clone();
    ours.name = "Locally renamed controller".into();
    ours.custom_sources
        .get_mut("controller")
        .unwrap()
        .push_str("// ours\n");
    theirs
        .root
        .nodes
        .iter_mut()
        .find(|n| n.id == "controller")
        .unwrap()
        .position = [901.0, 902.0];
    theirs
        .custom_sources
        .get_mut("controller")
        .unwrap()
        .push_str("// theirs\n");
    let merged = merge_documents(&base, &ours, &theirs)?;
    check(
        merged.conflicts.len() == 1
            && merged.conflicts[0].ours == Some(json!(ours.custom_sources["controller"]))
            && merged.conflicts[0].theirs == Some(json!(theirs.custom_sources["controller"]))
            && merged.document.name == ours.name
            && merged
                .document
                .root
                .nodes
                .iter()
                .find(|n| n.id == "controller")
                .unwrap()
                .position
                == [901.0, 902.0],
        "Three-way merge lost nonconflicting edits or either conflicting source",
    )?;
    let changes = diff_documents(&base, &edited)?;
    check(
        changes.len() >= 3,
        "Document diff missed Rust, HLSL or code-layout changes",
    )?;
    fs::write(
        output.join("source-conflicts.json"),
        serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(
        json!({"undo_redo":true,"source_and_layout_roundtrip":true,"checkpoint":checkpoint.id,
        "diff_changes":changes.len(),"merge_conflicts_preserved":merged.conflicts.len()}),
    )
}
