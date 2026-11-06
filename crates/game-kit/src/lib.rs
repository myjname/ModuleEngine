//! Independently selected, pure game-building libraries.
//!
//! These modules do not register scheduled systems or native services. A source
//! package calls the selected APIs from its own real consumers and declares its
//! graph/schema/service contracts at the integration boundary.

#[cfg(feature = "actions")]
pub mod actions;
#[cfg(feature = "campaign")]
pub mod campaign;
#[cfg(feature = "grid")]
pub mod grid;
#[cfg(feature = "history")]
pub mod history;
