//! Concrete Two Arenas authored data. This file depends only on core contracts.
use module_engine_core::*;
use serde_json::json;
use std::collections::BTreeMap;

/// Concrete Two Arenas content, supplied with an explicitly selected schedule.
pub fn build_project(execution: ExecutionGraph) -> GameProject {
    let prefab = Prefab {
        color: [190, 80, 85, 255],
        components: BTreeMap::from([
            (
                "actor".into(),
                json!({"team":"enemy","radius":0.36,"speed":2.1}),
            ),
            ("health".into(), json!({"maximum":36.0,"current":36.0})),
            ("ai".into(), json!({"kind":"chase","aggro_radius":14.0})),
            (
                "melee".into(),
                json!({"range":1.2,"damage":7.0,"cooldown":1.25,"arc":1.5,"duration":0.15,"windup":0.4}),
            ),
        ]),
    };
    fn object(id: u64, position: [f32; 3], scale: [f32; 3], color: [u8; 4]) -> AuthoredObject {
        AuthoredObject::new(WorldObject {
            id,
            parent: None,
            transform: Transform {
                translation: position,
                scale,
                ..Transform::default()
            },
            color,
        })
    }
    fn arena(name: &str) -> AuthoredScene {
        let mut objects = vec![object(
            1000,
            [0., -0.5, 0.],
            [8., 0.2, 8.],
            [60, 78, 86, 255],
        )];
        for (id, pos, scale) in [
            (1001, [-8., 0.5, 0.], [0.2, 1., 8.]),
            (1002, [8., 0.5, 0.], [0.2, 1., 8.]),
            (1003, [0., 0.5, 8.], [8., 1., 0.2]),
            (1004, [-4.75, 0.5, -8.], [3.25, 1., 0.2]),
            (1005, [4.75, 0.5, -8.], [3.25, 1., 0.2]),
            (1006, [-3., 0.4, 1.], [0.8, 0.8, 0.8]),
        ] {
            let mut wall = object(id, pos, scale, [95, 104, 119, 255]);
            wall.components.insert(
                "obstacle".into(),
                json!({"half_extents":[scale[0],scale[2]]}),
            );
            objects.push(wall);
        }
        let mut player = object(1, [0., 0., 5.], [0.35, 0.7, 0.35], [70, 170, 245, 255]);
        player.components = BTreeMap::from([
            ("player".into(), json!({})),
            ("persistent".into(), json!({})),
            (
                "actor".into(),
                json!({"team":"player","radius":0.35,"speed":6.0}),
            ),
            ("health".into(), json!({"maximum":100.0,"current":100.0})),
            (
                "melee".into(),
                json!({"range":2.2,"damage":28.0,"cooldown":0.35,"arc":1.9,"duration":0.12,"windup":0.0}),
            ),
        ]);
        objects.push(player);
        AuthoredScene {
            name: name.into(),
            objects,
        }
    }
    let mut first = arena("Small monsters");
    for index in 0..20_u64 {
        let mut monster = object(
            index + 10,
            [
                -5. + (index % 5) as f32 * 2.4,
                0.,
                -5. + (index / 5) as f32 * 1.6,
            ],
            [0.3, 0.45, 0.3],
            [190, 80, 85, 255],
        );
        monster.prefab = Some("small-monster".into());
        first.objects.push(monster);
    }
    let mut passage = object(1100, [0., 0., -7.2], [1.4, 0.08, 0.5], [60, 190, 140, 255]);
    passage.components.insert(
        "portal".into(),
        json!({"scene":"boss","radius":1.1,"requires_clear":true,"spawn":[0.,0.,5.]}),
    );
    first.objects.push(passage);
    let mut second = arena("Boss arena");
    let mut boss = object(100, [0., 0., -3.], [0.9, 1.2, 0.9], [150, 65, 210, 255]);
    boss.components = BTreeMap::from([
        (
            "actor".into(),
            json!({"team":"enemy","radius":0.9,"speed":1.6}),
        ),
        ("health".into(), json!({"maximum":400.0,"current":400.0})),
        ("ai".into(), json!({"kind":"boss","aggro_radius":20.0})),
        ("boss".into(), json!({"victory_on_death":true})),
        (
            "melee".into(),
            json!({"range":3.0,"damage":30.0,"cooldown":2.2,"arc":6.3,"duration":0.1,"windup":1.0}),
        ),
    ]);
    second.objects.push(boss);
    GameProject {
        entry_scene: "arena".into(),
        scenes: BTreeMap::from([("arena".into(), first), ("boss".into(), second)]),
        prefabs: BTreeMap::from([("small-monster".into(), prefab)]),
        execution,
        audio_cues: ["attack", "hit", "death", "victory"]
            .into_iter()
            .map(|cue| (cue.into(), format!("assets/audio/{cue}.wav")))
            .collect(),
    }
}
