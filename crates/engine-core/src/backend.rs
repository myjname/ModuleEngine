//! Executable renderer boundary. The core never owns a graphics API device.

use crate::{Camera, Frame, GraphDocument, Node, RenderOverrides, SceneSnapshot, render_scene};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Measured cumulative work. Unsupported backend operations remain zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendererPerformance {
    pub render_calls: u64,
    pub prepare_calls: u64,
    pub plan_builds: u64,
    pub plan_cache_hits: u64,
    pub rejected_cache_hits: u64,
    pub pipeline_creations: u64,
    pub pipeline_build_attempts: u64,
    pub prepare_ns: u64,
}

/// Adapter-discovered stable identities, independent of graphics API handles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderControlTargets {
    pub controls: BTreeSet<String>,
    pub resources: BTreeSet<String>,
}

/// Cheap installation metadata available even when device preparation fails.
pub trait RendererAdapter: Send + Sync {
    fn configuration_key(&self, node: &Node, document: &GraphDocument) -> Value {
        json!({"node":renderer_node_configuration(node),"shader_sources":document.shader_sources})
    }
    fn dependency_revision(&self, _node: &Node, _document: &GraphDocument) -> Result<u64, String> {
        Ok(0)
    }
    fn control_targets(
        &self,
        _node: &Node,
        _document: &GraphDocument,
    ) -> Result<Option<RenderControlTargets>, String> {
        Ok(None)
    }
    /// No device creation, shader compilation or live configuration mutation.
    fn validate_controls(
        &self,
        _node: &Node,
        _document: &GraphDocument,
        controls: &RenderOverrides,
    ) -> Result<(), String> {
        reject_unsupported_controls(controls)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultRendererAdapter;
impl RendererAdapter for DefaultRendererAdapter {}

/// Executable node JSON with exactly the graph presentation exclusions used by
/// Graph::execution_eq, shared by source renderer adapters.
pub fn renderer_node_configuration(node: &Node) -> Value {
    fn normalize(node: &mut Node) {
        node.label.clear();
        node.position = [0.; 2];
        if matches!(node.module.as_str(), "custom.rust" | "custom.rust.update")
            && let Some(params) = node.params.as_object_mut()
        {
            params.remove("code_layout");
            params.remove("code_layout_keys");
        }
        if let Some(graph) = &mut node.subgraph {
            for child in &mut graph.nodes {
                normalize(child);
            }
        }
    }
    let mut node = node.clone();
    normalize(&mut node);
    serde_json::to_value(node).expect("Node contains portable JSON")
}

fn reject_unsupported_controls(controls: &RenderOverrides) -> Result<(), String> {
    if controls.parameters.is_empty()
        && controls.enabled.is_empty()
        && controls.resource_commands.is_empty()
    {
        Ok(())
    } else {
        Err("This renderer does not implement portable render controls".into())
    }
}

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
    fn performance(&self) -> RendererPerformance {
        RendererPerformance::default()
    }
    fn set_detailed_stats(&mut self, _enabled: bool) {}
    /// Return a separate configuration sharing reusable caches if supported.
    /// Cache warming must not change the active configuration on rejection.
    /// None asks the host to call its registered isolated factory instead.
    fn prepare_candidate(
        &mut self,
        _node: &Node,
        _document: &GraphDocument,
    ) -> Result<Option<Box<dyn FrameRenderer>>, String> {
        Ok(None)
    }
    fn validate_overrides(&self, overrides: &RenderOverrides) -> Result<(), String> {
        reject_unsupported_controls(overrides)
    }
}

pub struct SoftwareRenderer;

pub(crate) struct SoftwareRendererAdapter;
impl RendererAdapter for SoftwareRendererAdapter {
    fn validate_controls(
        &self,
        node: &Node,
        _document: &GraphDocument,
        controls: &RenderOverrides,
    ) -> Result<(), String> {
        crate::runtime::validate_software_renderer(node).map_err(|error| error.to_string())?;
        reject_unsupported_controls(controls)
    }
}

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
