use module_engine_core::{
    Camera, EcsWorld, ObjectsWorld, RenderOverrides, RenderResourceCommand, WorldProvider,
};
use module_engine_vulkan::{
    RenderPath, VulkanRenderer, custom_compute_manifest, custom_raster_manifest, default_graph,
    demo_custom_compute_source, demo_custom_raster_source,
};
use std::{collections::BTreeMap, path::Path};
fn save(directory: &Path, name: &str, frame: &module_engine_core::Frame) -> Result<(), String> {
    let file =
        std::fs::File::create(directory.join(format!("{name}.png"))).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(file, frame.width as u32, frame.height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .map_err(|e| e.to_string())?
        .write_image_data(&frame.rgba)
        .map_err(|e| e.to_string())
}
fn main() -> Result<(), String> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or(".work/gpu-selfcheck".into());
    let directory = Path::new(&directory);
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut renderer = VulkanRenderer::new()?;
    println!("Actual Vulkan adapter: {}", renderer.device_name());
    let mut objects = ObjectsWorld::demo();
    let mut ecs = EcsWorld::demo();
    for _ in 0..30 {
        objects.step(1. / 60., 1.);
        ecs.step(1. / 60., 1.);
    }
    let scene = objects.snapshot();
    let camera = Camera::default();
    let mut graph = default_graph(RenderPath::Forward);
    let mut sources = BTreeMap::new();
    let overrides = RenderOverrides::default();
    let forward = renderer.render(&scene, camera, 320, 240, &graph, &sources, &overrides)?;
    save(directory, "forward-aces", &forward)?;
    assert!(
        forward.stats.shaded_pixels > 1000
            && forward.stats.visible_objects >= 3
            && forward.stats.triangles > 0,
        "GPU must show actual depth-tested geometry"
    );
    let ecs_frame = renderer.render(
        &ecs.snapshot(),
        camera,
        320,
        240,
        &graph,
        &sources,
        &overrides,
    )?;
    assert_eq!(
        forward.rgba, ecs_frame.rgba,
        "Same neutral snapshot must render identically for both world stores"
    );
    let deferred = renderer.render(
        &scene,
        camera,
        320,
        240,
        &default_graph(RenderPath::Deferred),
        &sources,
        &overrides,
    )?;
    save(directory, "deferred", &deferred)?;
    assert_ne!(
        forward.rgba, deferred.rgba,
        "Deferred lighting pass must affect pixels"
    );
    graph
        .nodes
        .iter_mut()
        .find(|n| n.id == "tonemap")
        .unwrap()
        .params["mode"] = serde_json::json!("reinhard");
    let reinhard = renderer.render(&scene, camera, 320, 240, &graph, &sources, &overrides)?;
    save(directory, "forward-reinhard", &reinhard)?;
    assert_ne!(
        forward.rgba, reinhard.rgba,
        "Tone mapper replacement must affect pixels"
    );
    graph = default_graph(RenderPath::Forward);
    let custom = graph.nodes.iter_mut().find(|n| n.id == "ao").unwrap();
    custom.module = "gpu.custom".into();
    custom.params = serde_json::json!({"source_id":"custom_compute","gain":0.25,"manifest":custom_compute_manifest()});
    graph
        .edges
        .retain(|e| e.to_node != "ao" || e.to_port == "color");
    sources.insert("custom_compute".into(), demo_custom_compute_source());
    let compute = renderer.render(&scene, camera, 320, 240, &graph, &sources, &overrides)?;
    save(directory, "custom-compute", &compute)?;
    assert_ne!(forward.rgba, compute.rgba);
    let mut bad = sources.clone();
    bad.insert("custom_compute".into(), "this is invalid HLSL".into());
    assert!(
        renderer
            .prepare(&graph, &bad)
            .unwrap_err()
            .contains("HLSL compilation failed")
    );
    let retained = renderer.render(&scene, camera, 320, 240, &graph, &sources, &overrides)?;
    assert_eq!(
        compute.rgba, retained.rgba,
        "Failed candidate must retain valid pipelines and exact source"
    );
    graph = default_graph(RenderPath::Forward);
    graph.nodes.retain(|n| n.id != "ao");
    graph
        .edges
        .retain(|e| e.from_node != "ao" && e.to_node != "ao");
    graph.edges.push(module_engine_core::make_edge(
        "geometry", "color", "tonemap", "color",
    ));
    let custom = graph.nodes.iter_mut().find(|n| n.id == "geometry").unwrap();
    custom.module = "gpu.custom".into();
    custom.params = serde_json::json!({"source_id":"custom_scene","shift":0.0,"manifest":custom_raster_manifest()});
    sources.insert("custom_scene".into(), demo_custom_raster_source());
    let raster = renderer.render(&scene, camera, 320, 240, &graph, &sources, &overrides)?;
    save(directory, "custom-raster", &raster)?;
    assert!(raster.stats.shaded_pixels > 1000);
    graph
        .nodes
        .iter_mut()
        .find(|n| n.id == "geometry")
        .unwrap()
        .params["shift"] = serde_json::json!(1.2);
    let shifted = renderer.render(&scene, camera, 320, 240, &graph, &sources, &overrides)?;
    save(directory, "custom-vertex-shift", &shifted)?;
    assert_ne!(
        raster.rgba, shifted.rgba,
        "Custom vertex shader must change actual rasterized geometry"
    );
    let mut clearing = RenderOverrides::default();
    clearing
        .resource_commands
        .push(RenderResourceCommand::ClearResource {
            name: "encode.frame".into(),
            value: [1., 0., 0., 1.],
        });
    let cleared = renderer.render(&scene, camera, 320, 240, &graph, &sources, &clearing)?;
    assert!(
        cleared
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 0, 0, 255]),
        "Explicit clear must execute vkCmdClearColorImage"
    );
    let report = serde_json::json!({"adapter":renderer.device_name(),"worlds_identical":true,"forward_deferred_changed":true,"tonemappers_changed":true,"custom_compute_changed":true,"custom_vertex_changed":true,"invalid_candidate_retained":true,"native_resource_clear":true,"stats":forward.stats});
    std::fs::write(
        directory.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("{report}");
    Ok(())
}
