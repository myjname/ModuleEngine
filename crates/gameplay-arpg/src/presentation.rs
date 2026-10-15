use crate::{GameStateExt, GameStatus, legacy_input};
use module_engine_core::{Camera, GameContribution, GameProject, GameState};
use std::collections::BTreeSet;

pub fn host_contribution() -> GameContribution {
    GameContribution {
        id: "arpg.presentation".into(),
        activation_modules: [
            "game.input",
            "game.ai",
            "game.movement",
            "game.collision",
            "game.combat",
            "game.health",
            "game.portals",
            "game.audio",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>(),
        begin_tick: crate::state::begin_tick,
        initial_camera,
        follow_target,
        decorate_snapshot: apply_hit_flashes,
        script_input: legacy_input,
        validate_event: crate::validate_event,
        diagnostics,
    }
}
pub fn initial_camera(_: &GameProject) -> Camera {
    Camera {
        yaw: 0.,
        pitch: 0.95,
        distance: 21.,
        target: [0.; 3],
    }
}
pub fn follow_target(state: &GameState) -> Option<[f32; 3]> {
    state
        .player()
        .map(|player| player.object.transform.translation)
}
pub fn diagnostics(state: &GameState) -> serde_json::Value {
    serde_json::json!({
        "status": format!("{:?}", state.arpg().status),
        "active_enemies": state.living_enemies(),
        "player_health":state.player().and_then(|player| player.components.get("health").and_then(|value|value.get("current")).and_then(serde_json::Value::as_f64))
    })
}

pub fn apply_hit_flashes(snapshot: &mut module_engine_core::SceneSnapshot, state: &GameState) {
    let frame = state.arpg();
    if frame.status != GameStatus::Playing || frame.hit_visuals.is_empty() {
        return;
    }
    for item in &mut snapshot.items {
        let intensity = frame
            .hit_visuals
            .iter()
            .filter(|hit| {
                hit.target == item.id
                    && hit
                        .target_lifetime
                        .is_none_or(|lifetime| state.world.lifetime(item.id) == Some(lifetime))
            })
            .map(|hit| {
                let age = state.time - hit.started_at;
                if age < 0. || state.time >= hit.expires_at {
                    0.
                } else {
                    (1. - age / 0.16).clamp(0., 1.) as f32 * 0.95
                }
            })
            .fold(0_f32, f32::max);
        if intensity <= 0. {
            continue;
        }
        for (channel, white) in [255., 248., 205.].into_iter().enumerate() {
            item.color[channel] =
                (item.color[channel] as f32 * (1. - intensity) + white * intensity).round() as u8;
            item.material.base_color[channel] +=
                (1. - item.material.base_color[channel]) * intensity;
        }
    }
}
