//! Executable renderer boundary. The core never owns a graphics API device.

use crate::{Camera, Frame, RenderOverrides, SceneSnapshot, render_scene};

/// A compiled-in rendering module with an API-independent scene/control input.
/// A host may share one device between candidates, but each candidate owns its
/// graph configuration. Returning an error allows the host to retain the last
/// working configuration rather than silently display a fabricated frame.
pub trait FrameRenderer: Send {
    fn render(
        &mut self,
        scene: &SceneSnapshot,
        camera: Camera,
        width: usize,
        height: usize,
        overrides: &RenderOverrides,
    ) -> Result<Frame, String>;

    fn backend_name(&self) -> &str;
    fn validate_overrides(&self, overrides: &RenderOverrides) -> Result<(), String> {
        if overrides.parameters.is_empty()
            && overrides.enabled.is_empty()
            && overrides.resource_commands.is_empty()
        {
            Ok(())
        } else {
            Err("This renderer does not implement portable render controls".into())
        }
    }
}

pub struct SoftwareRenderer;

impl FrameRenderer for SoftwareRenderer {
    fn render(
        &mut self,
        scene: &SceneSnapshot,
        camera: Camera,
        width: usize,
        height: usize,
        _overrides: &RenderOverrides,
    ) -> Result<Frame, String> {
        Ok(render_scene(scene, width, height, camera))
    }

    fn backend_name(&self) -> &str {
        "CPU / Software3D"
    }
}
