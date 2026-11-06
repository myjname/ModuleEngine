//! Small source contributions shared by headless hosts and UI applications.
use crate::{Camera, GameEvent, GameInput, GameProject, GameState, GraphDocument, SceneSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub type GameTickHook = fn(&mut GameState) -> Result<(), String>;
pub type GameEventValidator = fn(&GameEvent) -> Result<(), String>;
pub type GameStateValidator = fn(&GameState) -> Result<(), String>;

/// Pure source-owned visual models before interpolation and imported primitives.
/// Keys must identify unambiguous items in the raw snapshot; only finite affine
/// models may change. Canonical state, geometry and materials remain untouched.
pub type GamePoseProjection =
    fn(&SceneSnapshot, &GameState) -> Result<BTreeMap<u64, [[f32; 4]; 4]>, String>;

/// A source-owned request for an optional application service. The ID selects
/// an installed native factory; configuration is interpreted only by it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostServiceRequest {
    pub id: String,
    pub config: Value,
    /// Portable references needed by this requested service. Exporters consume
    /// this declaration without interpreting the service's private config JSON.
    #[serde(default)]
    pub asset_references: BTreeSet<String>,
}

pub type HostServiceRequests = fn(&GameProject) -> Result<Vec<HostServiceRequest>, String>;

/// Application-owned presentation context; it is not part of authored data,
/// gameplay input or the Custom wire protocol. Games may display service errors
/// without acquiring filesystem/native service access.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameUiStatus {
    pub can_close_application: bool,
    pub services: Value,
    /// Current host faults, not the historical Activity log. Service-specific
    /// recovery state remains in its explicitly requested diagnostics payload.
    pub errors: Vec<String>,
}

/// Source UI may request an application action separately from game simulation.
/// Applications enforce capability flags; an Editor never closes for a game UI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameUiResponse {
    pub input: GameInput,
    pub close_application: bool,
}

/// Ephemeral authoring-tool identity; never serialized into a project. Storage
/// belongs to one registered tool in one Editor, independently of pane layout.
/// Document changes only after successful project adoption; revision acknowledges
/// that tool's accepted Apply (including no-op) or explicit Discard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleToolContext {
    pub storage_id: u64,
    pub document_instance: u64,
    pub revision: u64,
}

/// A source reports unsaved local work separately from a completed proposal.
/// Only the application may validate, publish, save or acknowledge the proposal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleToolResponse {
    pub candidate: Option<GraphDocument>,
    pub dirty: bool,
}

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
