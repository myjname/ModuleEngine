//! Acceptance evidence for the actual Project policy, module data and GPU output.
use module_engine_core::{
    Camera, Comparison, ControlCommand, Frame, PropertyData, RuleCondition, Runtime, ScriptContext,
    VariantRule, WorldKind, attach_component, detach_component, prepare_team_document,
    reconcile_properties, set_project_parameter, set_project_variant, validate_project_edit,
    visible_property_schemas,
};
use module_engine_script::{ScriptCompiler, ScriptKind};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};

fn check(ok: bool, message: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message.into()) }
}

fn changed(a: &Frame, b: &Frame) -> usize {
    a.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0.iter())
        .filter(|(a, b)| a != b)
        .count()
}

pub fn run(output: &Path) -> Result<Value, String> {
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let registry = crate::host::registry()?;
    let mut document = crate::host::settings_demo_document(WorldKind::Objects);
    let ecs_document = crate::host::settings_demo_document(WorldKind::Ecs);
    let mut objects = Runtime::from_document(&document, &registry).map_err(|e| e.to_string())?;
    let mut ecs = Runtime::from_document(&ecs_document, &registry).map_err(|e| e.to_string())?;
    crate::advance(&document, &mut objects, 30)?;
    crate::advance(&ecs_document, &mut ecs, 30)?;
    check(
        objects.snapshot() == ecs.snapshot(),
        "Component-driven Rust differs between Object/ECS",
    )?;
    let frame = objects.render(320, 240, Camera::default())?;
    let ecs_frame = ecs.render(320, 240, Camera::default())?;
    check(
        frame.rgba == ecs_frame.rgba && frame.stats.shaded_pixels > 1000,
        "Component-driven Vulkan frames differ or contain no geometry",
    )?;
    crate::write_png(&output.join("project-objects.png"), &frame)?;
    crate::write_png(&output.join("project-ecs.png"), &ecs_frame)?;

    let participating = visible_property_schemas(&document, &registry, Some(1));
    let unrelated = visible_property_schemas(&document, &registry, Some(2));
    check(
        participating
            .iter()
            .any(|(_, s)| s.id == "game.motion.speed")
            && !unrelated.iter().any(|(_, s)| s.id == "game.motion.speed"),
        "Component fields leaked to a non-participating object",
    )?;
    let before = objects.snapshot();
    let mut context = ScriptContext::new(0.8, 1.0 / 60.0, objects.objects()?);
    objects.fill_script_context(&mut context);
    check(
        context.component_property(1, "game.motion", "game.motion.speed") == Some(&json!(1.0))
            && context
                .component_property(2, "game.motion", "game.motion.speed")
                .is_none(),
        "Compiled worker context contains wrong participating data",
    )?;

    let root = crate::project::source_root();
    let compiler = ScriptCompiler::new(root.clone(), root.join(".work/scripts"));
    let forbidden = compiler.compile_kind(
        "fn update(ctx:&mut ScriptContext) { ctx.set_color(1,[1,2,3,255]); ctx.set_render_parameter(\"tonemap\",\"exposure\",999.0); }",
        ScriptKind::Update).map_err(|e| e.to_string())?;
    let bad_tick = forbidden
        .tick(&context, Duration::from_millis(150))
        .map_err(|e| e.to_string())?;
    let policy_error = objects
        .apply_project_controls(&bad_tick.commands)
        .err()
        .ok_or("Compiled Rust bypassed Project range")?;
    check(
        objects.snapshot() == before,
        "Rejected render batch partially changed the world",
    )?;
    let allowed = compiler.compile_kind(
        "fn update(ctx:&mut ScriptContext) { if ctx.parameter(\"exposure\").is_some() { ctx.set_render_parameter(\"tonemap\",\"exposure\",1.7); } }",
        ScriptKind::Update).map_err(|e| e.to_string())?;
    let tick = allowed
        .tick(&context, Duration::from_millis(150))
        .map_err(|e| e.to_string())?;
    check(
        matches!(tick.commands.as_slice(), [ControlCommand::Render(_)]),
        "Allowed Rust parameter query did not execute",
    )?;
    objects.apply_project_controls(&tick.commands)?;
    let exposed = objects.render(320, 240, Camera::default())?;
    let exposure_pixels = changed(&frame, &exposed);
    check(
        exposure_pixels > 100,
        "Allowed compiled Rust parameter did not affect GPU pixels",
    )?;
    crate::write_png(&output.join("project-exposure.png"), &exposed)?;

    set_project_parameter(&mut document, "exposure", json!(1.0))?;
    set_project_variant(&mut document, "antialiasing", "off")?;
    document.project.rules.push(VariantRule {
        group: "tonemap".into(),
        variant: "reinhard".into(),
        condition: RuleCondition::Time {
            op: Comparison::GreaterOrEqual,
            value: 0.25,
        },
    });
    objects.set_project_configuration(document.project.clone())?;
    objects.begin_frame();
    objects.apply_project_settings(0.0)?;
    let initial = objects.render(320, 240, Camera::default())?;
    objects.begin_frame();
    objects.apply_project_settings(0.5)?;
    let conditional = objects.render(320, 240, Camera::default())?;
    let conditional_pixels = changed(&initial, &conditional);
    check(
        conditional_pixels > 100,
        "Ready-variant rule did not affect real GPU pixels",
    )?;
    check(
        objects.snapshot() == before,
        "Project parameter/variant edit reset world state",
    )?;
    crate::write_png(
        &output.join("project-conditional-reinhard.png"),
        &conditional,
    )?;
    let mut invalid = document.clone();
    check(
        set_project_variant(&mut invalid, "antialiasing", "dlaa").is_err()
            && set_project_parameter(&mut invalid, "exposure", json!(99.0)).is_err(),
        "Unshipped variant or out-of-range UI value was accepted",
    )?;
    let mut altered = document.clone();
    altered.root.edges.pop();
    check(
        validate_project_edit(&altered, &document, &registry).is_err(),
        "Project topology edit was accepted",
    )?;
    let mut gameplay = document.clone();
    gameplay
        .custom_sources
        .get_mut("controller")
        .unwrap()
        .push_str("\n// Editable gameplay source\n");
    validate_project_edit(&gameplay, &document, &registry)?;

    let mut inactive = document.clone();
    inactive.engine.enabled_modules.remove("game.motion");
    reconcile_properties(&mut inactive, &registry)?;
    check(
        visible_property_schemas(&inactive, &registry, Some(1)).is_empty()
            && inactive
                .properties
                .active
                .components
                .get(&1)
                .is_none_or(|c| !c.contains_key("game.motion"))
            && inactive.properties.inactive.components[&1]["game.motion"]["game.motion.speed"]
                == json!(1.0),
        "Disabled module did not archive participating values",
    )?;
    objects.set_properties(document.properties.clone())?;
    inactive.engine.enabled_modules.insert("game.motion".into());
    reconcile_properties(&mut inactive, &registry)?;
    check(
        inactive.properties.active.components[&1]["game.motion"]["game.motion.speed"] == json!(1.0),
        "Re-enabled module lost authored component value",
    )?;
    detach_component(&mut inactive, &registry, 1, "game.motion")?;
    reconcile_properties(&mut inactive, &registry)?;
    check(
        visible_property_schemas(&inactive, &registry, Some(1)).is_empty(),
        "Detached component was automatically reattached",
    )?;
    let packaged = prepare_team_document(&inactive);
    check(
        packaged.properties.inactive == PropertyData::default()
            && packaged.properties.detached_components.is_empty(),
        "Team project contains authoring archive",
    )?;
    attach_component(&mut inactive, &registry, 1, "game.motion")?;
    check(
        inactive.properties.active.components[&1]["game.motion"]["game.motion.speed"] == json!(1.0),
        "Explicit component reattachment lost archived value",
    )?;
    let mut faster = document.clone();
    faster
        .properties
        .active
        .components
        .get_mut(&1)
        .unwrap()
        .get_mut("game.motion")
        .unwrap()
        .insert("game.motion.speed".into(), json!(2.0));
    let mut faster_runtime =
        Runtime::from_document(&faster, &registry).map_err(|e| e.to_string())?;
    crate::advance(&faster, &mut faster_runtime, 30)?;
    check(
        faster_runtime.snapshot() != before,
        "Edited component did not affect actual compiled Rust movement",
    )?;
    crate::project::save(&output.join("project-settings.json"), &document)?;
    crate::project::save(&output.join("project-settings-ecs.json"), &ecs_document)?;
    let report = json!({
        "version":env!("CARGO_PKG_VERSION"), "backend":objects.backend_name(),
        "object_ecs_component_snapshots_equal":true, "object_ecs_gpu_frames_equal":true,
        "component_fields_only_on_participants":true,"component_edit_changes_compiled_rust":true,
        "disabled_data_archived_and_restored":true,"explicit_detach_preserved":true,
        "team_archive_excluded":true,"project_topology_rejected":true,"gameplay_source_edit_allowed":true,
        "unshipped_dlaa_rejected":true,"compiled_rust_range_rejected_transactionally":true,
        "range_diagnostic":policy_error,"exposure_changed_pixels":exposure_pixels,
        "conditional_tonemap_changed_pixels":conditional_pixels,"world_state_preserved":true,
        "geometry_pixels":frame.stats.shaded_pixels,"source":"Real Vulkan, compiled Rust workers and shared Project validator"
    });
    fs::write(
        output.join("report.json"),
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}
