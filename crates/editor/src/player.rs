//! Editor entry points for the shared standalone Player presentation.
use module_engine_core::{GraphDocument, WorldKind};
pub use module_engine_player::presentations::{draw_hud_for, preview_aim_for, read_input_for};
pub use module_engine_player::ui::{update_render_diagnostic, update_texture};
use std::path::Path;

pub fn run(
    document: GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
) -> Result<(), String> {
    module_engine_player::run_host(crate::game_host::new(document, root, compile_root, kind)?)
}
pub fn run_benchmark(
    document: GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
    folder: &Path,
) -> Result<(), String> {
    module_engine_player::run_benchmark_host(
        crate::game_host::new(document, root, compile_root, kind)?,
        folder,
    )
}
