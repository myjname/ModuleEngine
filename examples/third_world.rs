//! Register a third world from application source without editing engine-core or
//! the renderer. Run: cargo run -p module-engine-core --example third_world.
//! Optional --write-examples writes the two built-in graph documents beside this
//! source; normal execution exercises the independently implemented provider.

use module_engine_core::{
    Camera, Mesh, ModuleRegistry, RenderItem, Runtime, SceneSnapshot, Transform, WorldKind,
    WorldProvider, demo_document, world_descriptor,
};
use std::error::Error;

/// This provider owns only its own angle/time; it wraps neither built-in world.
struct PyramidWorld {
    angle: f32,
    time: f32,
}

impl WorldProvider for PyramidWorld {
    fn step(&mut self, dt: f32, speed: f32) {
        self.time += dt;
        self.angle += dt * speed;
    }

    fn snapshot(&self) -> SceneSnapshot {
        SceneSnapshot {
            time: self.time,
            items: vec![RenderItem {
                material: Default::default(),
                id: 100,
                mesh: Mesh {
                    texcoords: vec![],
                    vertices: vec![
                        [0.0, 1.5, 0.0],
                        [-1.0, -0.7, -1.0],
                        [1.0, -0.7, -1.0],
                        [1.0, -0.7, 1.0],
                        [-1.0, -0.7, 1.0],
                    ],
                    triangles: vec![
                        [0, 2, 1],
                        [0, 3, 2],
                        [0, 4, 3],
                        [0, 1, 4],
                        [1, 2, 3],
                        [1, 3, 4],
                    ],
                },
                model: Transform {
                    rotation: [0.0, self.angle, 0.0],
                    ..Transform::default()
                }
                .matrix(),
                color: [238, 175, 66, 255],
            }],
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().any(|arg| arg == "--write-examples") {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        for (name, kind) in [
            ("objects-world.json", WorldKind::Objects),
            ("ecs-world.json", WorldKind::Ecs),
        ] {
            std::fs::write(
                directory.join(name),
                serde_json::to_string_pretty(&demo_document(kind))? + "\n",
            )?;
            println!("Wrote {}", directory.join(name).display());
        }
        return Ok(());
    }

    let mut registry = ModuleRegistry::builtin();
    registry.register_world(
        world_descriptor(
            "world.pyramid",
            "Pyramid",
            "Independent source provider with a rotating triangle mesh",
        ),
        |_params| {
            Ok(Box::new(PyramidWorld {
                angle: 0.0,
                time: 0.0,
            }))
        },
    )?;
    let mut document = demo_document(WorldKind::Objects);
    document.name = "Third source provider / unchanged renderer".into();
    document
        .root
        .nodes
        .iter_mut()
        .find(|node| node.id == "world")
        .unwrap()
        .module = "world.pyramid".into();

    // Use a direct Scalar source in this minimal example. Unlike the full editor,
    // it has no Custom parser; it follows the actual connected scalar node.
    document
        .root
        .edges
        .retain(|edge| !(edge.to_node == "world" && edge.to_port == "speed"));
    document.root.edges.push(module_engine_core::make_edge(
        "base", "value", "world", "speed",
    ));
    let mut runtime = Runtime::from_document(&document, &registry)?;
    let source_id = runtime
        .world_speed_source()
        .expect("connected speed source");
    let source = document
        .root
        .nodes
        .iter()
        .find(|node| node.id == source_id)
        .unwrap();
    let speed = source.params["value"].as_f64().unwrap() as f32;
    runtime.step(0.75, speed);
    let frame = runtime
        .render(320, 240, Camera::default())
        .expect("render frame");
    println!(
        "world={}, kind={:?}",
        runtime.world_module_id(),
        runtime.world_kind()
    );
    println!("{}", serde_json::to_string_pretty(&frame.stats)?);
    assert_eq!(frame.stats.objects, 1);
    assert!(frame.stats.shaded_pixels > 0);
    Ok(())
}
