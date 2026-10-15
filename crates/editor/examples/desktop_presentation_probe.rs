//! Bounded control for the editor's native GUI presentation path.
//!
//! This deliberately creates no project, GameHost, Vulkan device, or layout.
//! `MODULE_ENGINE_PROBE_SECONDS` sets the lifetime (default: 10 seconds).
//! `MODULE_ENGINE_PROBE_VSYNC=0` changes only the OpenGL swap interval.
//! `MODULE_ENGINE_GUI_BACKEND=glow|wgpu` selects the shared presentation backend.

use eframe::egui;
use std::time::{Duration, Instant};

struct DesktopPresentationProbe {
    started: Instant,
    lifetime: Duration,
    vsync: bool,
    renderer: &'static str,
}

impl eframe::App for DesktopPresentationProbe {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let elapsed = self.started.elapsed();
        ui.heading("ModuleEngine desktop presentation probe");
        ui.label(format!("Standalone eframe / {} control", self.renderer));
        if self.renderer == "Glow" {
            ui.label(format!("OpenGL vsync: {}", self.vsync));
        }
        ui.label(format!("Frame: {}", ui.ctx().cumulative_frame_nr()));
        ui.label(format!("Elapsed: {:.3} seconds", elapsed.as_secs_f64()));
        if elapsed >= self.lifetime {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
    }
}

fn main() -> Result<(), String> {
    let seconds = std::env::var("MODULE_ENGINE_PROBE_SECONDS")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("MODULE_ENGINE_PROBE_SECONDS: {error}"))
        })
        .unwrap_or(Ok(10))?;
    let vsync = std::env::var("MODULE_ENGINE_PROBE_VSYNC").as_deref() != Ok("0");
    let mut options = module_engine_player::ui::native_options(
        egui::ViewportBuilder::default()
            .with_inner_size([1580.0, 940.0])
            .with_min_inner_size([1100.0, 720.0]),
    )?;
    options.glow_options.vsync = vsync;
    eprintln!(
        "Desktop presentation probe: pid={}, renderer={:?}, glow_vsync={vsync}, seconds={seconds}",
        std::process::id(),
        options.renderer
    );
    eframe::run_native(
        "ModuleEngine / Desktop presentation probe",
        options,
        Box::new(move |cc| {
            let graphics = module_engine_player::ui::report_graphics(cc);
            Ok(Box::new(DesktopPresentationProbe {
                started: Instant::now(),
                lifetime: Duration::from_secs(seconds),
                vsync,
                renderer: graphics.renderer,
            }))
        }),
    )
    .map_err(|error| error.to_string())
}
