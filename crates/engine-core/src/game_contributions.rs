//! Small source contributions shared by headless hosts and UI applications.
use crate::{Camera, GameEvent, GameInput, GameProject, GameState, SceneSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub type GameTickHook = fn(&mut GameState) -> Result<(), String>;
pub type GameEventValidator = fn(&GameEvent) -> Result<(), String>;
pub type GameStateValidator = fn(&GameState) -> Result<(), String>;

/// A source-owned request for an optional application service. The ID selects
/// an installed native factory; configuration is interpreted only by it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostServiceRequest {
    pub id: String,
    pub config: Value,
}

pub type HostServiceRequests = fn(&GameProject) -> Result<Vec<HostServiceRequest>, String>;

/// Pure host hooks; no graphics API, windowing, or editor types cross this API.
#[derive(Clone)]
pub struct GameContribution {
    pub id: String,
    /// One or more scheduled source modules activate this contribution.
    pub activation_modules: BTreeSet<String>,
    /// Runs inside the tick transaction before any authored initialization or
    /// step. Module-owned transients can be prepared without scheduling a node.
    pub begin_tick: GameTickHook,
    pub initial_camera: fn(&GameProject) -> Camera,
    pub follow_target: fn(&GameState) -> Option<[f32; 3]>,
    pub decorate_snapshot: fn(&mut SceneSnapshot, &GameState),
    pub script_input: fn(&GameInput) -> Value,
    pub validate_event: GameEventValidator,
    /// Check the complete staged world/module state after all authored systems
    /// and before events or external transaction publication.
    pub validate_state: GameStateValidator,
    /// Pure discovery only. The host decides whether to construct services;
    /// disabled/headless hosts need not install or invoke their factories.
    pub service_requests: HostServiceRequests,
    pub diagnostics: fn(&GameState) -> Value,
}

impl GameContribution {
    pub fn is_active(&self, graph: &crate::ExecutionGraph) -> bool {
        graph
            .stages
            .iter()
            .flat_map(|stage| &stage.nodes)
            .any(|node| self.activation_modules.contains(&node.module))
    }
}
