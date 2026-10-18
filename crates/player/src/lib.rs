//! Source-composed standalone Player, independent of the Editor application.
mod composition;
pub mod package;
pub mod source_modules;

#[cfg(feature = "ui")]
pub mod gpu_presentation;
#[cfg(feature = "ui")]
pub mod presentations;
#[cfg(feature = "ui")]
pub mod ui;

pub use composition::{
    compiled_features, composition_at, composition_for_features, composition_with, service_registry,
};
#[cfg(feature = "ui")]
pub use ui::{configure_style, run_benchmark_host, run_host};

/// Export a real rendered frame without introducing a windowing dependency.
pub fn write_png(path: &std::path::Path, frame: &module_engine_core::Frame) -> Result<(), String> {
    if frame.gpu.is_some() || !frame.is_valid() {
        return Err("PNG export requires an explicit complete CPU-rendered frame".into());
    }
    let file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        frame.width as u32,
        frame.height as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .map_err(|error| error.to_string())?
        .write_image_data(&frame.rgba)
        .map_err(|error| error.to_string())
}
