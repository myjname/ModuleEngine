//! Native Vulkan renderer. All shading, postprocessing, and Custom passes execute
//! on the GPU. Scene vertex preparation and final frame readback are CPU work.
mod compiler;
mod geometry;
mod graph;
mod vulkan;
pub use compiler::shader_dependency_revision_at;
pub use compiler::{
    ShaderArtifact, ShaderPerformance, compile_hlsl, compile_hlsl_at, dxc_path,
    precompile_graph_at, resource_root, shader_performance,
};
pub use graph::builtin_hlsl_source;
pub use graph::{
    PassManifest, RenderPath, ResourceSpec, builtin_manifest, custom_compute_manifest,
    custom_raster_manifest, default_graph, demo_custom_compute_source, demo_custom_raster_source,
    module_descriptors, port_type, remove_pass_with_bypass, render_graph_targets,
};
pub use graph::{custom_fullscreen_manifest, demo_custom_fullscreen_source};
pub use vulkan::{
    RenderGraphValidator, RendererPerformance, VulkanRenderer, validate_overrides,
    validate_overrides_at,
};
