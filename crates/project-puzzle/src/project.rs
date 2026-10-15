use crate::components::{BOARD, CELL};
use module_engine_core::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Build the complete independent project from a neutral authored starting point.
pub fn project() -> GameProject {
    let mut project = GameProject::empty();
    project.entry_scene = "board".into();
    project.scenes.clear();
    let mut board = AuthoredObject::new(WorldObject {
        id: 1,
        parent: None,
        transform: Transform {
            scale: [2.5, 0.1, 2.5],
            ..Default::default()
        },
        color: [58, 70, 91, 255],
    });
    board.components.insert(BOARD.into(), json!({}));
    let mut objects = vec![board];
    let mut cell = |id, x, z, kind: &str| {
        let (scale, color) = match kind {
            "wall" => ([0.48, 0.5, 0.48], [94, 106, 132, 255]),
            "goal" => ([0.43, 0.025, 0.43], [80, 215, 155, 255]),
            "pawn" => ([0.27, 0.4, 0.27], [90, 175, 250, 255]),
            _ => ([0.36, 0.36, 0.36], [235, 174, 75, 255]),
        };
        let mut entity = AuthoredObject::new(WorldObject {
            id,
            parent: None,
            transform: Transform {
                translation: [
                    x as f32 - 2.,
                    if kind == "goal" { 0.13 } else { 0.5 },
                    z as f32 - 2.,
                ],
                scale,
                ..Default::default()
            },
            color,
        });
        entity
            .components
            .insert(CELL.into(), json!({"x":x,"z":z,"kind":kind}));
        objects.push(entity);
    };
    let mut id = 10;
    for z in 0..5 {
        for x in 0..5 {
            if x == 0 || x == 4 || z == 0 || z == 4 || (x == 1 && z == 1) {
                cell(id, x, z, "wall");
                id += 1;
            }
        }
    }
    cell(100, 1, 3, "pawn");
    cell(101, 2, 2, "crate");
    cell(102, 3, 1, "goal");
    project.scenes.insert(
        "board".into(),
        AuthoredScene {
            name: "Crate Steps".into(),
            objects,
        },
    );
    project.execution = ExecutionGraph {
        resources: vec![
            SystemResource {
                name: "world".into(),
                data_type: "GameWorld".into(),
                external: true,
                initial: Value::Null,
            },
            SystemResource {
                name: "input".into(),
                data_type: "GameInput".into(),
                external: true,
                initial: Value::Null,
            },
            SystemResource {
                name: "events".into(),
                data_type: "GameEvents".into(),
                external: true,
                initial: json!([]),
            },
            SystemResource {
                name: "turn".into(),
                data_type: "PuzzleTurn/1".into(),
                external: false,
                initial: Value::Null,
            },
        ],
        stages: vec![SystemStage {
            name: "Turn".into(),
            nodes: vec![
                SystemNode {
                    id: "turn".into(),
                    module: "puzzle.turn".into(),
                    params: json!({}),
                    bindings: BTreeMap::from([
                        ("world".into(), "world".into()),
                        ("input".into(), "input".into()),
                        ("events".into(), "events".into()),
                        ("turn".into(), "turn".into()),
                    ]),
                    after: vec![],
                },
                SystemNode {
                    id: "result".into(),
                    module: "puzzle.result".into(),
                    params: json!({}),
                    bindings: BTreeMap::from([
                        ("world".into(), "world".into()),
                        ("events".into(), "events".into()),
                        ("turn".into(), "turn".into()),
                    ]),
                    after: vec!["turn".into()],
                },
            ],
        }],
    };
    project
}
