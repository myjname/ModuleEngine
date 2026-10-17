//! Optional native-app screenshot diagnostics, after real UI frames settle.
//! Normal editor runs never request screenshots or additional repaints here.
use eframe::egui;
use std::{
    ffi::OsStr,
    fs,
    io::BufWriter,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Personal settings are the only redirected paths. This never substitutes
/// package/source roots, Engine bindings, SDKs or authored scene data.
pub(crate) fn diagnostic_personal_root() -> Result<Option<PathBuf>, String> {
    parse_diagnostic_personal_root(
        std::env::var_os("MODULE_ENGINE_DIAGNOSTIC_USER_ROOT").as_deref(),
    )
}

fn parse_diagnostic_personal_root(value: Option<&OsStr>) -> Result<Option<PathBuf>, String> {
    let Some(value) = value else { return Ok(None) };
    let path = Path::new(value);
    if !path.is_absolute() || !path.is_dir() {
        return Err(
            "MODULE_ENGINE_DIAGNOSTIC_USER_ROOT requires an existing absolute directory".into(),
        );
    }
    path.canonicalize()
        .map(Some)
        .map_err(|error| format!("Diagnostic personal directory: {error}"))
}

pub struct Capture {
    destination: Option<PathBuf>,
    last_frame: Option<u64>,
    frames: u32,
    requested: bool,
    started: Option<Instant>,
    settled_since: Option<Instant>,
    done: bool,
}

impl Default for Capture {
    fn default() -> Self {
        Self::new()
    }
}

impl Capture {
    pub fn new() -> Self {
        Self {
            destination: std::env::var_os("MODULE_ENGINE_UI_SCREENSHOT_TO")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            last_frame: None,
            frames: 0,
            requested: false,
            started: None,
            settled_since: None,
            done: false,
        }
    }

    /// Returns true only after the requested native screenshot was saved.
    /// The caller can then allow its diagnostic window to close normally.
    pub fn tick(&mut self, ctx: &egui::Context) -> Result<bool, String> {
        let Some(destination) = &self.destination else {
            return Ok(false);
        };
        if self.done {
            return Ok(true);
        }
        if self.requested {
            let viewport = ctx.viewport_id();
            let screenshot = ctx.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Screenshot {
                        viewport_id, image, ..
                    } if *viewport_id == viewport => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = screenshot {
                write_png(destination, &image)?;
                self.done = true;
                return Ok(true);
            }
        }
        let started = self.started.get_or_insert_with(Instant::now);
        if started.elapsed() > Duration::from_secs(45) {
            return Err("Native UI capture timed out before a screenshot arrived".into());
        }
        let frame = ctx.cumulative_frame_nr();
        if self.last_frame != Some(frame) {
            self.last_frame = Some(frame);
            self.frames += 1;
        }
        // Count frames rather than layout passes. A sizing pass can run twice
        // in one frame and must not capture a partly laid-out menu or toolbar.
        if !self.requested && self.frames >= 8 {
            // Initial host/shader preparation can occupy the first frame. Start
            // the animation wait only once subsequent frames have settled.
            let settled = self.settled_since.get_or_insert_with(Instant::now);
            if settled.elapsed() >= Duration::from_millis(400) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.requested = true;
            }
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        Ok(false)
    }
}

fn write_png(path: &Path, image: &egui::ColorImage) -> Result<(), String> {
    let [width, height] = image.size;
    if width == 0 || height == 0 || width.checked_mul(height) != Some(image.pixels.len()) {
        return Err("Native screenshot has invalid pixel dimensions".into());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|e| format!("UI capture folder: {e}"))?;
    }
    let width = u32::try_from(width).map_err(|_| "UI capture width exceeds PNG limits")?;
    let height = u32::try_from(height).map_err(|_| "UI capture height exceeds PNG limits")?;
    let file = fs::File::create(path).map_err(|e| format!("UI capture file: {e}"))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("UI capture PNG: {e}"))?;
    let rgba = image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_srgba_unmultiplied())
        .collect::<Vec<_>>();
    writer
        .write_image_data(&rgba)
        .map_err(|e| format!("UI capture pixels: {e}"))?;
    writer
        .finish()
        .map_err(|e| format!("UI capture finish: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_personal_root_requires_an_explicit_existing_absolute_directory() {
        assert_eq!(parse_diagnostic_personal_root(None).unwrap(), None);
        for value in ["", "relative/personal", "."] {
            assert!(parse_diagnostic_personal_root(Some(OsStr::new(value))).is_err());
        }
        let guard = module_engine_script::TestArtifactGuard::new(
            &std::env::temp_dir().join("module-engine-diagnostic-settings"),
            "personal-root",
        )
        .unwrap();
        let root = guard.path().canonicalize().unwrap();
        assert_eq!(
            parse_diagnostic_personal_root(Some(root.as_os_str())).unwrap(),
            Some(root.clone())
        );
        assert!(parse_diagnostic_personal_root(Some(root.join("missing").as_os_str())).is_err());
        let file = root.join("not-a-directory");
        fs::write(&file, b"keep").unwrap();
        assert!(parse_diagnostic_personal_root(Some(file.as_os_str())).is_err());
        assert_eq!(fs::read(file).unwrap(), b"keep");
    }
}
