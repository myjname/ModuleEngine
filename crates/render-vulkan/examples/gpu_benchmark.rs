//! Renderer-only diagnostic, not a substitute for the full Player acceptance run.
use module_engine_core::{
    Camera, ObjectsWorld, RenderOverrides, Transform, WorldObject, WorldProvider,
};
use module_engine_vulkan::{RenderPath, VulkanRenderer, default_graph};
use std::{collections::BTreeMap, time::Instant};
fn main() -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut objects = (0..20)
        .map(|i| WorldObject {
            id: i + 1,
            parent: None,
            transform: Transform {
                translation: [(i % 5) as f32 * 3. - 6., 0., (i / 5) as f32 * 3. - 4.5],
                ..Default::default()
            },
            color: [255; 4],
        })
        .collect::<Vec<_>>();
    objects.push(WorldObject {
        id: 21,
        parent: None,
        transform: Transform {
            translation: [0., -1.3, 0.],
            scale: [10., 0.1, 10.],
            ..Default::default()
        },
        color: [90, 110, 130, 255],
    });
    let mut scene = ObjectsWorld::from_objects(objects)?.snapshot();
    let mut library = module_engine_content::ContentLibrary::new(&root)?;
    library.enrich_snapshot(
        &mut scene,
        &(1..=20)
            .map(|i| (i, "assets/models/demo_crystal.glb".into()))
            .collect(),
    )?;
    let mut renderer = VulkanRenderer::new_at(root.clone())?;
    renderer.set_detailed_stats(false);
    let camera = Camera {
        pitch: 0.85,
        distance: 25.,
        ..Default::default()
    };
    let sources = BTreeMap::new();
    let overrides = RenderOverrides::default();
    let graph = default_graph(RenderPath::Forward);
    for _ in 0..10 {
        renderer.render(&scene, camera, 1920, 1080, &graph, &sources, &overrides)?;
    }
    let mut samples = vec![];
    for frame in 0..120 {
        scene.items[0].model[0][3] = -6. + (frame as f32 * 0.06).sin();
        let start = Instant::now();
        renderer.render(&scene, camera, 1920, 1080, &graph, &sources, &overrides)?;
        samples.push(start.elapsed().as_secs_f64() * 1000.);
    }
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let report = serde_json::json!({"scope":"renderer only; no player input/audio/logic/display upload","adapter":renderer.device_name(),"resolution":[1920,1080],"imported_textured_objects":20,"frames":samples.len(),"mean_ms":mean,"p50_ms":samples[60],"p95_ms":samples[114],"mean_fps":1000./mean,"target_mean_60":mean<=1000./60.});
    let path = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join(".work/gpu-benchmark.json"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&report).unwrap();
    std::fs::write(path, &text).map_err(|e| e.to_string())?;
    println!("{text}");
    Ok(())
}
