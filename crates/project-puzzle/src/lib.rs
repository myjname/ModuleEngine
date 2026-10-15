//! An independent authored turn-based crate puzzle using portable engine contracts.
use module_engine_core::{
    Camera, GameContribution, GameInput, GameProject, GameState, SystemRegistry,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

mod components;
mod project;
mod systems;
pub use project::project;

#[cfg(test)]
mod tests;
#[cfg(feature = "ui")]
pub mod ui;

pub const FRAME_KEY: &str = "puzzle.frame/1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PuzzleOutcome {
    #[default]
    Playing,
    Solved,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PuzzleFrame {
    pub outcome: PuzzleOutcome,
    pub turns: u64,
}

pub fn frame(game: &GameState) -> Result<PuzzleFrame, String> {
    game.module_data.get(FRAME_KEY).map_or_else(
        || Ok(PuzzleFrame::default()),
        |value| {
            serde_json::from_value(value.clone())
                .map_err(|error| format!("Invalid {FRAME_KEY}: {error}"))
        },
    )
}

fn store_frame(game: &mut GameState, frame: PuzzleFrame) -> Result<(), String> {
    game.module_data.insert(
        FRAME_KEY.into(),
        serde_json::to_value(frame).map_err(|error| error.to_string())?,
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    North,
    East,
    South,
    West,
}
impl Direction {
    pub fn action(self) -> &'static str {
        match self {
            Self::North => "puzzle.north",
            Self::East => "puzzle.east",
            Self::South => "puzzle.south",
            Self::West => "puzzle.west",
        }
    }
    fn delta(self) -> (i32, i32) {
        match self {
            Self::North => (0, -1),
            Self::East => (1, 0),
            Self::South => (0, 1),
            Self::West => (-1, 0),
        }
    }
}

pub fn direction_input(direction: Direction) -> GameInput {
    GameInput {
        actions: BTreeSet::from([direction.action().into()]),
        ..Default::default()
    }
}

pub fn install(registry: &mut SystemRegistry) -> Result<(), String> {
    components::install(registry.components_mut())?;
    systems::install(registry)?;
    registry.register_project_contract(
        "puzzle.turn",
        |project, _| {
            if project
                .execution
                .stages
                .iter()
                .flat_map(|stage| &stage.nodes)
                .any(|node| matches!(node.module.as_str(), "puzzle.turn" | "puzzle.result"))
            {
                for scene in project.scenes.keys() {
                    components::project_layout(project, scene)?;
                }
            }
            Ok(())
        },
        |_| Ok(()),
    )?;
    registry.register_game_contribution(GameContribution {
        id: "puzzle.presentation".into(),
        activation_modules: BTreeSet::from(["puzzle.turn".into(), "puzzle.result".into()]),
        begin_tick: |game| validate_state(game),
        initial_camera,
        follow_target,
        decorate_snapshot: |_, _| {},
        script_input: |input| serde_json::to_value(input).expect("Portable input"),
        validate_event: systems::validate_event,
        validate_state,
        diagnostics,
    })
}

fn initial_camera(project: &GameProject) -> Camera {
    let (target, distance) = components::project_layout(project, &project.entry_scene).map_or(
        ([0.; 3], 9.5),
        |layout| {
            let span = layout.board.width.max(layout.board.height) as f32 * layout.board.cell_size;
            (layout.origin, 9.5 * span / 5.)
        },
    );
    Camera {
        yaw: 0.45,
        pitch: 1.,
        distance,
        target,
    }
}
fn validate_state(game: &GameState) -> Result<(), String> {
    let layout = components::world_layout(&game.world)?;
    let current = frame(game)?;
    let limit = layout.board.max_turns as u64;
    if current.turns > limit
        || (current.outcome == PuzzleOutcome::Playing && current.turns >= limit)
    {
        return Err("Puzzle frame turn count exceeds the authored playing limit".into());
    }
    Ok(())
}
fn follow_target(game: &GameState) -> Option<[f32; 3]> {
    game.world
        .entities_with(&[components::BOARD])
        .first()
        .and_then(|id| game.world.object(*id))
        .map(|object| object.transform.translation)
}
fn diagnostics(game: &GameState) -> serde_json::Value {
    match frame(game) {
        Ok(frame) => {
            json!({"status":format!("{:?}",frame.outcome),"outcome":format!("{:?}",frame.outcome),"turns":frame.turns})
        }
        Err(error) => json!({"error":error}),
    }
}

pub fn registry() -> SystemRegistry {
    let mut registry = SystemRegistry::builtin();
    install(&mut registry).expect("Valid puzzle source contracts");
    registry
}
