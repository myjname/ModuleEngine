//! Configurable engine prototype: typed graphs, independently stored worlds, and
//! a renderer that consumes a neutral scene snapshot.
//!
//! The host evaluates Scalar inputs (including Custom Rust) and passes the value
//! connected to the selected world's speed pin to [`Runtime::step`]. This crate
//! does not interpret arbitrary Rust or silently approximate unsupported routes.

mod backend;
mod component_schema;
mod composition;
mod control;
mod engine_lock;
mod game;
mod game_contributions;
mod graph;
mod registry;
mod render;
mod runtime;
mod scenes;
mod settings;
mod systems;
mod world;

pub use backend::*;
pub use component_schema::*;
pub use composition::*;
pub use control::*;
pub use engine_lock::*;
pub use game::*;
pub use game_contributions::*;
pub use graph::*;
pub use registry::*;
pub use render::*;
pub use runtime::*;
pub use scenes::*;
pub use settings::*;
pub use systems::*;
pub use world::*;
