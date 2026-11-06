//! Crate Garden: source-owned Sokoban rules, authored levels and progress.
use module_engine_core::{GameInput, GameState};
pub use module_engine_game_kit::grid::Cell;
use module_engine_game_kit::{campaign::Campaign, history::UndoJournal};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod authoring;
mod components;
#[cfg(feature = "ui")]
pub mod grid_ui;
mod levels;
mod rules;
#[cfg(test)]
mod tests;
#[cfg(feature = "ui")]
pub mod ui;

pub use authoring::{Brush, GridDraft, authored_board, level_ids, project};
pub use levels::{LevelDefinition, level_catalog, level_solution};
pub use rules::{action_map, install, profile, registry, view};

pub const PROFILE_KEY: &str = "g1.profile/1";
pub const SAVED_KEY: &str = "g1.saved/1";
pub const VIEW_KEY: &str = "g1.view/1";
pub const BOARD: &str = "g1.board";
pub const CELL: &str = "g1.cell";

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
            Self::North => "g1.north",
            Self::East => "g1.east",
            Self::South => "g1.south",
            Self::West => "g1.west",
        }
    }
    pub fn delta(self) -> (i32, i32) {
        match self {
            Self::North => (0, -1),
            Self::East => (1, 0),
            Self::South => (0, 1),
            Self::West => (-1, 0),
        }
    }
}
pub fn action_input(action: &str) -> GameInput {
    GameInput {
        actions: [action.to_owned()].into_iter().collect(),
        ..Default::default()
    }
}
pub fn direction_input(direction: Direction) -> GameInput {
    action_input(direction.action())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub pawn: Cell,
    pub crates: BTreeMap<u64, Cell>,
    pub moves: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub level_revision: String,
    pub history: UndoJournal<Position>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub music_volume: f32,
    pub sfx_volume: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            music_volume: 0.45,
            sfx_volume: 0.75,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub version: u32,
    pub catalog_revision: String,
    pub campaign: Campaign<Attempt>,
    pub settings: Settings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    #[default]
    Menu,
    LevelSelect,
    Settings,
    Playing,
    Paused,
    Victory,
    Complete,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelSummary {
    pub id: String,
    pub name: String,
    pub index: usize,
    pub unlocked: bool,
    pub best_moves: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoardView {
    pub width: u32,
    pub height: u32,
    pub walls: Vec<Cell>,
    pub goals: Vec<Cell>,
    pub pawn: Cell,
    pub crates: Vec<Cell>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub phase: Phase,
    pub current_level: usize,
    pub levels: Vec<LevelSummary>,
    pub moves: u64,
    pub can_undo: bool,
    pub can_continue: bool,
    pub settings: Settings,
    pub board: BoardView,
    pub warning: Option<String>,
}

pub fn diagnostics(game: &GameState) -> serde_json::Value {
    view(game).map_or_else(|error| serde_json::json!({"error":error}), |view| serde_json::json!({
        "game":"Crate Garden","phase":view.phase,"status":view.phase,"level":view.current_level+1,"moves":view.moves,
        "can_undo":view.can_undo,"warning":view.warning
    }))
}
