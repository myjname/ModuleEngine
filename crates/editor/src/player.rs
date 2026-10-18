//! Editor entry points for the shared standalone Player presentation.
use module_engine_core::{GraphDocument, WorldKind};
pub use module_engine_player::presentations::{draw_hud_for, preview_aim_for, read_input_for};
pub use module_engine_player::ui::{
    presentation_texture_id, update_presented_texture, update_render_diagnostic,
};
use std::path::Path;

pub fn run(
    document: GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
) -> Result<(), String> {
    let host = module_engine_player::ui::prepare_host(|| {
        crate::game_host::new(document, root, compile_root, kind)
    })?;
    module_engine_player::run_host(host)
}
pub fn run_benchmark(
    document: GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
    folder: &Path,
) -> Result<(), String> {
    let host = module_engine_player::ui::prepare_host(|| {
        crate::game_host::new(document, root, compile_root, kind)
    })?;
    module_engine_player::run_benchmark_host(host, folder)
}
