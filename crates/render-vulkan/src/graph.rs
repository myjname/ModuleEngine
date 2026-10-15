use module_engine_core::{Graph, ModuleDescriptor, PortDescriptor, make_edge, make_node};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderPath {
    Forward,
    Deferred,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResourceSpec {
    pub name: String,
    pub format: String,
    pub semantic: String,
    #[serde(default = "viewport")]
    pub extent: String,
    #[serde(default = "one")]
    pub samples: u32,
    pub access: String,
}
fn viewport() -> String {
    "viewport".into()
}
fn one() -> u32 {
    1
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassManifest {
    pub stage: String,
    #[serde(default = "main_entry")]
    pub entry: String,
    #[serde(default)]
    pub vertex_entry: Option<String>,
    #[serde(default = "fullscreen")]
    pub raster_mode: String,
    /// Scene raster pass accepts the native UV/base-color image atlas ABI.
    #[serde(default)]
    pub scene_materials: bool,
    #[serde(default = "groups")]
    pub workgroup: [u32; 3],
    #[serde(default)]
    pub inputs: Vec<ResourceSpec>,
    pub outputs: Vec<ResourceSpec>,
    #[serde(default)]
    pub parameters: Vec<String>,
}
fn main_entry() -> String {
    "main".into()
}
fn fullscreen() -> String {
    "fullscreen".into()
}
fn groups() -> [u32; 3] {
    [8, 8, 1]
}
#[derive(Clone, Debug)]
pub(crate) struct Pass {
    pub id: String,
    pub module: String,
    pub manifest: PassManifest,
    pub inputs: Vec<(ResourceSpec, String)>,
    pub source: String,
    pub dependency_hash: u64,
    pub params: serde_json::Value,
}
impl Pass {
    pub fn resource(&self, name: &str) -> String {
        format!("{}.{}", self.id, name)
    }
    pub fn geometry(&self) -> bool {
        self.manifest.stage == "raster" && self.manifest.raster_mode == "scene"
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    pub passes: Vec<Pass>,
    pub output: String,
}
fn resource(name: &str, format: &str, semantic: &str, access: &str) -> ResourceSpec {
    ResourceSpec {
        name: name.into(),
        format: format.into(),
        semantic: semantic.into(),
        extent: viewport(),
        samples: 1,
        access: access.into(),
    }
}
fn manifest(
    stage: &str,
    inputs: Vec<ResourceSpec>,
    outputs: Vec<ResourceSpec>,
    parameters: &[&str],
) -> PassManifest {
    PassManifest {
        stage: stage.into(),
        entry: "main".into(),
        vertex_entry: None,
        raster_mode: if stage == "raster" {
            "scene"
        } else {
            "fullscreen"
        }
        .into(),
        scene_materials: false,
        workgroup: groups(),
        inputs,
        outputs,
        parameters: parameters.iter().map(|s| (*s).into()).collect(),
    }
}
pub fn builtin_manifest(module: &str) -> Option<PassManifest> {
    let hdr = || resource("color", "rgba32float", "hdr_linear", "sampled_read");
    let ldr = || resource("color", "rgba32float", "ldr_linear", "sampled_read");
    let normal = || resource("normals", "rgba32float", "world_normals", "sampled_read");
    let depth = || resource("depth", "r32float", "camera_linear_depth", "sampled_read");
    let output = |semantic: &str| resource("color", "rgba32float", semantic, "storage_write");
    Some(match module {
        "gpu.forward" | "gpu.gbuffer" | "gpu.depth_normal" => {
            let color_name = if module == "gpu.gbuffer" {
                "albedo"
            } else {
                "color"
            };
            let semantic = if module == "gpu.gbuffer" {
                "albedo_linear"
            } else {
                "hdr_linear"
            };
            let mut outputs = vec![
                resource(color_name, "rgba32float", semantic, "color_write"),
                resource("normals", "rgba32float", "world_normals", "color_write"),
                resource("depth", "r32float", "camera_linear_depth", "color_write"),
            ];
            if module == "gpu.depth_normal" {
                outputs.remove(0);
            }
            let mut result = manifest(
                "raster",
                vec![resource("scene", "scene", "scene3d", "vertex_read")],
                outputs,
                &[],
            );
            result.scene_materials = module != "gpu.depth_normal";
            result
        }
        "gpu.deferred" => manifest(
            "compute",
            vec![
                resource("albedo", "rgba32float", "albedo_linear", "sampled_read"),
                normal(),
                depth(),
            ],
            vec![output("hdr_linear")],
            &[],
        ),
        "gpu.ssao" => manifest(
            "compute",
            vec![hdr(), normal(), depth()],
            vec![output("hdr_linear")],
            &["strength"],
        ),
        "gpu.tonemap" => manifest(
            "compute",
            vec![hdr()],
            vec![output("ldr_linear")],
            &["exposure", "mode"],
        ),
        "gpu.fxaa" => manifest("compute", vec![ldr()], vec![output("ldr_linear")], &[]),
        "gpu.color_encode" => manifest(
            "compute",
            vec![ldr()],
            vec![resource(
                "frame",
                "rgba8unorm",
                "srgb_display",
                "storage_write",
            )],
            &[],
        ),
        _ => return None,
    })
}
pub fn port_type(spec: &ResourceSpec) -> &'static str {
    match spec.semantic.as_str() {
        "scene3d" => "Scene3D",
        "world_normals" => "GpuNormals",
        "camera_linear_depth" => "GpuDepth",
        "srgb_display" => "Frame3D",
        _ => "GpuColor",
    }
}

fn node_manifest(node: &module_engine_core::Node) -> Option<PassManifest> {
    let manifest = if node.module == "gpu.custom" {
        serde_json::from_value(node.params.get("manifest")?.clone()).ok()?
    } else {
        builtin_manifest(&node.module)?
    };
    validate_manifest(&manifest).ok()?;
    Some(manifest)
}

/// Stable executable pass/output identities, without compiling shaders. Invalid
/// pass manifests retain their pass ID but do not claim valid output resources.
pub fn render_graph_targets(graph: &Graph) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut passes = BTreeSet::new();
    let mut resources = BTreeSet::new();
    for node in &graph.nodes {
        if matches!(node.module.as_str(), "interface.input" | "interface.output") {
            continue;
        }
        passes.insert(node.id.clone());
        if let Some(manifest) = node_manifest(node) {
            resources.extend(
                manifest
                    .outputs
                    .into_iter()
                    .map(|output| format!("{}.{}", node.id, output.name)),
            );
        }
    }
    (passes, resources)
}

/// Delete a pass and incident links. Preserve its consumers only when its sole
/// output has one unambiguous, type-identical input with one incoming link.
/// Returns None when absent, otherwise the number of replacement connections.
pub fn remove_pass_with_bypass(graph: &mut Graph, id: &str) -> Option<usize> {
    let node = graph.nodes.iter().find(|node| node.id == id)?;
    let bypass = (|| {
        if graph.nodes.iter().filter(|node| node.id == id).count() != 1 {
            return None;
        }
        let manifest = node_manifest(node)?;
        let [output] = manifest.outputs.as_slice() else {
            return None;
        };
        let mut matching = manifest.inputs.iter().filter(|input| {
            input.format == output.format
                && input.semantic == output.semantic
                && input.extent == output.extent
                && input.samples == output.samples
        });
        let input = matching.next()?;
        if matching.next().is_some() {
            return None;
        }
        let mut incoming = graph
            .edges
            .iter()
            .filter(|edge| edge.to_node == id && edge.to_port == input.name);
        let source = incoming.next()?;
        if incoming.next().is_some()
            || source.from_node == id
            || !graph.nodes.iter().any(|node| node.id == source.from_node)
        {
            return None;
        }
        let outgoing = graph
            .edges
            .iter()
            .filter(|edge| edge.from_node == id)
            .collect::<Vec<_>>();
        if outgoing
            .iter()
            .any(|edge| edge.from_port != output.name || edge.to_node == id)
        {
            return None;
        }
        Some(
            outgoing
                .into_iter()
                .map(|edge| {
                    make_edge(
                        &source.from_node,
                        &source.from_port,
                        &edge.to_node,
                        &edge.to_port,
                    )
                })
                .collect::<Vec<_>>(),
        )
    })();
    graph.nodes.retain(|node| node.id != id);
    graph
        .edges
        .retain(|edge| edge.from_node != id && edge.to_node != id);
    let replacement = bypass.unwrap_or_default();
    let count = replacement.len();
    graph.edges.extend(replacement);
    Some(count)
}
pub fn module_descriptors() -> Vec<ModuleDescriptor> {
    let mut result = Vec::new();
    for (id, label) in [
        ("gpu.forward", "Vulkan Forward"),
        ("gpu.gbuffer", "Vulkan G-buffer"),
        ("gpu.deferred", "Deferred lighting"),
        ("gpu.depth_normal", "Depth and normals"),
        ("gpu.ssao", "Screen-space AO"),
        ("gpu.tonemap", "Tone mapping"),
        ("gpu.fxaa", "FXAA"),
        ("gpu.color_encode", "sRGB encoding"),
    ] {
        let m = builtin_manifest(id).unwrap();
        result.push(ModuleDescriptor{id:id.into(),label:label.into(),description:"Native Vulkan GPU pass; resource format/semantic/access validated before activation.".into(),inputs:m.inputs.iter().map(|s|PortDescriptor{name:s.name.clone(),data_type:port_type(s).into(),required:true}).collect(),outputs:m.outputs.iter().map(|s|PortDescriptor{name:s.name.clone(),data_type:port_type(s).into(),required:false}).collect(),properties:vec![]});
    }
    result.push(ModuleDescriptor{id:"gpu.custom".into(),label:"Custom HLSL".into(),description:"Independent compute or raster pass. Named resources and optional scene vertex input are declared in params.manifest.".into(),inputs:vec![],outputs:vec![],properties:vec![]});
    result
}
pub fn default_graph(path: RenderPath) -> Graph {
    let mut graph = Graph::default();
    graph.nodes.push(make_node(
        "scene",
        "interface.input",
        "Scene input",
        [20., 90.],
        json!({"port":"scene","data_type":"Scene3D"}),
    ));
    let first = match path {
        RenderPath::Forward => "gpu.forward",
        RenderPath::Deferred => "gpu.gbuffer",
    };
    graph.nodes.push(make_node(
        "geometry",
        first,
        "Geometry GPU pass",
        [245., 90.],
        json!({}),
    ));
    graph
        .edges
        .push(make_edge("scene", "value", "geometry", "scene"));
    let color = if path == RenderPath::Deferred {
        graph.nodes.push(make_node(
            "lighting",
            "gpu.deferred",
            "Deferred lighting",
            [470., 90.],
            json!({}),
        ));
        for pin in ["albedo", "normals", "depth"] {
            graph
                .edges
                .push(make_edge("geometry", pin, "lighting", pin));
        }
        "lighting"
    } else {
        "geometry"
    };
    for (id, module, x, params) in [
        ("ao", "gpu.ssao", 695., json!({"strength":0.8})),
        (
            "tonemap",
            "gpu.tonemap",
            920.,
            json!({"exposure":1.0,"mode":"aces"}),
        ),
        ("aa", "gpu.fxaa", 1145., json!({})),
        ("encode", "gpu.color_encode", 1370., json!({})),
    ] {
        graph
            .nodes
            .push(make_node(id, module, id, [x, 90.], params));
    }
    graph.edges.extend([
        make_edge(color, "color", "ao", "color"),
        make_edge("geometry", "normals", "ao", "normals"),
        make_edge("geometry", "depth", "ao", "depth"),
        make_edge("ao", "color", "tonemap", "color"),
        make_edge("tonemap", "color", "aa", "color"),
        make_edge("aa", "color", "encode", "color"),
    ]);
    graph.nodes.push(make_node(
        "frame",
        "interface.output",
        "Frame output",
        [1595., 90.],
        json!({"port":"frame","data_type":"Frame3D"}),
    ));
    graph
        .edges
        .push(make_edge("encode", "frame", "frame", "value"));
    graph
}
pub fn custom_compute_manifest() -> PassManifest {
    manifest(
        "compute",
        vec![resource(
            "color",
            "rgba32float",
            "hdr_linear",
            "sampled_read",
        )],
        vec![resource(
            "color",
            "rgba32float",
            "hdr_linear",
            "storage_write",
        )],
        &["gain"],
    )
}
pub fn custom_raster_manifest() -> PassManifest {
    let mut result = manifest(
        "raster",
        vec![resource("scene", "scene", "scene3d", "vertex_read")],
        vec![resource(
            "color",
            "rgba32float",
            "hdr_linear",
            "color_write",
        )],
        &["shift"],
    );
    result.entry = "customPS".into();
    result.vertex_entry = Some("customVS".into());
    result
}
#[cfg(test)]
pub(crate) fn build_plan(
    graph: &Graph,
    sources: &BTreeMap<String, String>,
) -> Result<Plan, String> {
    build_plan_at(graph, sources, &crate::compiler::resource_root())
}
pub(crate) fn build_plan_at(
    graph: &Graph,
    sources: &BTreeMap<String, String>,
    resource_root: &std::path::Path,
) -> Result<Plan, String> {
    build_plan_impl(graph, sources, Some(resource_root))
}

/// Validate only declared resource/parameter contracts. Shader dependencies are
/// irrelevant to these contracts and must not perform filesystem work here.
pub(crate) fn build_metadata_plan(graph: &Graph) -> Result<Plan, String> {
    let sources = graph
        .nodes
        .iter()
        .filter_map(|node| node.params.get("source_id").and_then(|v| v.as_str()))
        .map(|id| (id.to_string(), String::new()))
        .collect();
    build_plan_impl(graph, &sources, None)
}
fn build_plan_impl(
    graph: &Graph,
    sources: &BTreeMap<String, String>,
    resource_root: Option<&std::path::Path>,
) -> Result<Plan, String> {
    let nodes: BTreeMap<_, _> = graph.nodes.iter().map(|n| (n.id.clone(), n)).collect();
    if nodes.len() != graph.nodes.len() {
        return Err("GPU graph contains duplicate node IDs".into());
    }
    let outputs: Vec<_> = graph
        .nodes
        .iter()
        .filter(|n| n.module == "interface.output")
        .collect();
    if outputs.len() != 1 || outputs[0].params.get("port").and_then(|v| v.as_str()) != Some("frame")
    {
        return Err("GPU graph needs exactly one interface.output(frame)".into());
    }
    if graph.nodes.iter().any(|n| n.id.is_empty()) {
        return Err("GPU node IDs must be nonempty".into());
    }
    let mut incoming = BTreeMap::<(String, String), (String, String)>::new();
    for edge in &graph.edges {
        if !nodes.contains_key(&edge.from_node) || !nodes.contains_key(&edge.to_node) {
            return Err("GPU edge references missing node".into());
        }
        if incoming
            .insert(
                (edge.to_node.clone(), edge.to_port.clone()),
                (edge.from_node.clone(), edge.from_port.clone()),
            )
            .is_some()
        {
            return Err("GPU input has multiple connections".into());
        }
    }
    let mut order = vec![];
    let mut remaining: BTreeSet<_> = nodes.keys().cloned().collect();
    while !remaining.is_empty() {
        let ready: Vec<_> = remaining
            .iter()
            .filter(|id| {
                !graph
                    .edges
                    .iter()
                    .any(|e| &e.to_node == *id && remaining.contains(&e.from_node))
            })
            .cloned()
            .collect();
        if ready.is_empty() {
            return Err("GPU graph dependency cycle".into());
        }
        for id in ready {
            remaining.remove(&id);
            order.push(id);
        }
    }
    let mut resources = BTreeMap::<(String, String), ResourceSpec>::new();
    let mut passes = vec![];
    let mut output = None;
    for id in order {
        let node = nodes[&id];
        if node.module == "interface.input" {
            if incoming.keys().any(|(to, _)| to == &id) {
                return Err("GPU interface.input cannot have incoming edges".into());
            }
            if node.params.get("port").and_then(|v| v.as_str()) != Some("scene") {
                return Err("Only scene GPU interface input is supported".into());
            }
            resources.insert(
                (id, "value".into()),
                resource("value", "scene", "scene3d", "vertex_read"),
            );
            continue;
        }
        if node.module == "interface.output" {
            if incoming.keys().any(|(to, pin)| to == &id && pin != "value") {
                return Err("GPU interface.output only accepts value".into());
            }
            let from = incoming
                .get(&(id.clone(), "value".into()))
                .ok_or("GPU frame output has no connection")?;
            let spec = resources
                .get(from)
                .ok_or("GPU frame output references nonexistent resource pin")?;
            if spec.format != "rgba8unorm" || spec.semantic != "srgb_display" {
                return Err(
                    "Frame output requires color encoding to rgba8unorm/srgb_display".into(),
                );
            }
            output = Some(format!("{}.{}", from.0, from.1));
            continue;
        }
        let m = if node.module == "gpu.custom" {
            serde_json::from_value::<PassManifest>(
                node.params
                    .get("manifest")
                    .cloned()
                    .ok_or("Custom HLSL needs params.manifest")?,
            )
            .map_err(|e| format!("Custom {} manifest: {e}", id))?
        } else {
            builtin_manifest(&node.module)
                .ok_or_else(|| format!("GPU module {} is not executable", node.module))?
        };
        validate_manifest(&m).map_err(|e| format!("GPU pass {id}: {e}"))?;
        if !node.params.is_null() && !node.params.is_object() {
            return Err(format!("GPU pass {id}: params must be an object"));
        }
        if let Some(params) = node.params.as_object() {
            for (key, value) in params {
                let custom_metadata =
                    node.module == "gpu.custom" && (key == "source_id" || key == "manifest");
                if key == "enabled" {
                    if !value.is_boolean() {
                        return Err(format!("GPU pass {id}: enabled must be boolean"));
                    }
                } else if !custom_metadata && !m.parameters.contains(key) {
                    return Err(format!("GPU pass {id}: unknown parameter {key}"));
                }
            }
        }
        for name in &m.parameters {
            if let Some(value) = node.params.get(name) {
                let valid = if name == "mode" {
                    value
                        .as_str()
                        .is_some_and(|v| v == "aces" || v == "reinhard")
                        || value.as_f64().is_some_and(|v| v == 0. || v == 1.)
                } else {
                    value
                        .as_f64()
                        .is_some_and(|v| v.is_finite() && v.abs() <= f32::MAX as f64)
                };
                if !valid {
                    return Err(format!(
                        "GPU pass {id}: parameter {name} violates scalar contract"
                    ));
                }
            }
        }
        let mut bindings = vec![];
        for spec in &m.inputs {
            let from = incoming
                .get(&(id.clone(), spec.name.clone()))
                .ok_or_else(|| format!("GPU pass {id} missing input {}", spec.name))?;
            let produced = resources.get(from).ok_or_else(|| {
                format!(
                    "GPU pass {id} input {} references nonexistent resource",
                    spec.name
                )
            })?;
            if produced.format != spec.format
                || produced.semantic != spec.semantic
                || produced.samples != spec.samples
                || produced.extent != spec.extent
            {
                return Err(format!(
                    "GPU pass {id} input {} expects {}/{}/{} samples, received {}/{}/{}",
                    spec.name,
                    spec.format,
                    spec.semantic,
                    spec.samples,
                    produced.format,
                    produced.semantic,
                    produced.samples
                ));
            }
            bindings.push((spec.clone(), format!("{}.{}", from.0, from.1)));
        }
        for (to, pin) in incoming.keys().filter(|(to, _)| to == &id) {
            if !m.inputs.iter().any(|s| &s.name == pin) {
                return Err(format!("GPU pass {to} has unknown input {pin}"));
            }
        }
        for spec in &m.outputs {
            resources.insert((id.clone(), spec.name.clone()), spec.clone());
        }
        let source = if node.module == "gpu.custom" {
            let key = node
                .params
                .get("source_id")
                .and_then(|v| v.as_str())
                .ok_or("Custom HLSL needs source_id")?;
            sources
                .get(key)
                .cloned()
                .ok_or_else(|| format!("HLSL source {key} missing"))?
        } else {
            builtin_source(&node.module).into()
        };
        let dependency_hash = match resource_root {
            Some(root) => crate::compiler::dependency_fingerprint_at(root, &source)?,
            None => 0,
        };
        passes.push(Pass {
            id,
            module: node.module.clone(),
            manifest: m,
            inputs: bindings,
            source,
            dependency_hash,
            params: node.params.clone(),
        });
    }
    let output = output.ok_or("GPU frame output missing")?;
    let mut reachable = BTreeSet::new();
    let mut todo = vec![outputs[0].id.clone()];
    while let Some(id) = todo.pop() {
        if reachable.insert(id.clone()) {
            todo.extend(
                graph
                    .edges
                    .iter()
                    .filter(|e| e.to_node == id)
                    .map(|e| e.from_node.clone()),
            );
        }
    }
    if passes.iter().any(|p| !reachable.contains(&p.id)) {
        return Err(
            "Disconnected GPU passes are not executed; connect or remove them before activation"
                .into(),
        );
    }
    Ok(Plan { passes, output })
}
fn validate_manifest(m: &PassManifest) -> Result<(), String> {
    if !["compute", "raster"].contains(&m.stage.as_str()) {
        return Err("stage must be compute or raster".into());
    }
    if m.inputs.len() > 16 || m.outputs.is_empty() || m.outputs.len() > 3 || m.parameters.len() > 16
    {
        return Err("one to three outputs and at most 16 scalar parameters are supported".into());
    }
    if m.stage == "raster" && !(["scene", "fullscreen"].contains(&m.raster_mode.as_str())) {
        return Err("raster_mode must be scene or fullscreen".into());
    }
    if m.scene_materials && (m.stage != "raster" || m.raster_mode != "scene") {
        return Err("scene_materials is available only for scene raster passes".into());
    }
    if m.stage == "compute"
        && (m.workgroup[0] == 0
            || m.workgroup[1] == 0
            || m.workgroup[2] != 1
            || m.workgroup[0]
                .checked_mul(m.workgroup[1])
                .is_none_or(|v| v > 256))
    {
        return Err("compute workgroup must be 1..256 invocations, z=1".into());
    }
    for list in [&m.inputs, &m.outputs] {
        let mut names = BTreeSet::new();
        for s in list {
            if s.name.is_empty() || !names.insert(&s.name) {
                return Err("resource names must be unique and nonempty per direction".into());
            }
            if s.extent != "viewport" || s.samples != 1 {
                return Err("0.2 supports viewport-sized single-sample resources; MSAA requires a future explicit resolve contract".into());
            }
            if s.format == "scene" {
                if s.semantic != "scene3d"
                    || m.stage != "raster"
                    || m.raster_mode != "scene"
                    || s.access != "vertex_read"
                {
                    return Err("Scene3D is only a geometry raster input".into());
                }
            } else if !["rgba32float", "r32float", "rgba8unorm"].contains(&s.format.as_str()) {
                return Err(format!("unsupported image format {}", s.format));
            }
        }
    }
    if m.outputs.iter().any(|s| {
        s.access
            != if m.stage == "compute" {
                "storage_write"
            } else {
                "color_write"
            }
    }) {
        return Err("output access does not match pass stage".into());
    }
    if m.inputs
        .iter()
        .any(|s| s.format != "scene" && s.access != "sampled_read")
    {
        return Err("texture inputs require sampled_read access".into());
    }
    let scenes = m.inputs.iter().filter(|s| s.format == "scene").count();
    if scenes != usize::from(m.stage == "raster" && m.raster_mode == "scene") {
        return Err("scene raster needs exactly one Scene3D input; fullscreen/compute do not consume Scene3D".into());
    }
    Ok(())
}
pub(crate) fn builtin_source(module: &str) -> &'static str {
    match module {
        "gpu.forward" => include_str!("../../../assets/gpu/forward.hlsl"),
        "gpu.gbuffer" => include_str!("../../../assets/gpu/gbuffer.hlsl"),
        "gpu.depth_normal" => include_str!("../../../assets/gpu/depth_normal.hlsl"),
        "gpu.deferred" => include_str!("../../../assets/gpu/deferred.hlsl"),
        "gpu.ssao" => include_str!("../../../assets/gpu/ssao.hlsl"),
        "gpu.tonemap" => include_str!("../../../assets/gpu/tonemap.hlsl"),
        "gpu.fxaa" => include_str!("../../../assets/gpu/fxaa.hlsl"),
        "gpu.color_encode" => include_str!("../../../assets/gpu/color_encode.hlsl"),
        _ => "",
    }
}
pub fn demo_custom_compute_source() -> String {
    include_str!("../../../assets/gpu/custom_compute.hlsl").into()
}
pub fn demo_custom_raster_source() -> String {
    include_str!("../../../assets/gpu/custom_raster.hlsl").into()
}

pub fn custom_fullscreen_manifest() -> PassManifest {
    let mut manifest = custom_compute_manifest();
    manifest.stage = "raster".into();
    manifest.raster_mode = "fullscreen".into();
    manifest.outputs[0].access = "color_write".into();
    manifest
}
pub fn demo_custom_fullscreen_source() -> String {
    include_str!("../../../assets/gpu/custom_fullscreen.hlsl").into()
}
/// Return the exact shipped HLSL so an installed pass can be converted into an
/// editable Custom instance without changing its resource contract or graph ID.
pub fn builtin_hlsl_source(module: &str) -> Option<&'static str> {
    builtin_manifest(module).map(|_| builtin_source(module))
}

#[cfg(test)]
mod removal_tests {
    use super::*;

    #[test]
    fn aa_and_ao_removal_preserve_executable_image_chains() {
        for (id, source, target) in [("aa", "tonemap", "encode"), ("ao", "geometry", "tonemap")] {
            let mut graph = default_graph(RenderPath::Forward);
            assert_eq!(remove_pass_with_bypass(&mut graph, id), Some(1));
            assert!(!graph.nodes.iter().any(|node| node.id == id));
            assert!(
                !graph
                    .edges
                    .iter()
                    .any(|edge| edge.from_node == id || edge.to_node == id)
            );
            assert!(
                graph
                    .edges
                    .contains(&make_edge(source, "color", target, "color"))
            );
            build_plan(&graph, &BTreeMap::new()).unwrap();
        }
    }

    #[test]
    fn semantic_or_format_conversions_and_geometry_are_not_guessed() {
        for id in ["tonemap", "encode", "geometry"] {
            let mut graph = default_graph(RenderPath::Forward);
            assert_eq!(remove_pass_with_bypass(&mut graph, id), Some(0));
            assert!(
                !graph
                    .edges
                    .iter()
                    .any(|edge| edge.from_node == id || edge.to_node == id)
            );
        }
    }

    fn custom_aa(graph: &mut Graph, change: impl FnOnce(&mut PassManifest)) {
        let mut manifest = builtin_manifest("gpu.fxaa").unwrap();
        change(&mut manifest);
        let node = graph.nodes.iter_mut().find(|node| node.id == "aa").unwrap();
        node.module = "gpu.custom".into();
        node.params = json!({"source_id":"custom-aa", "manifest":manifest});
    }

    #[test]
    fn custom_manifest_requires_one_matching_input_and_one_output() {
        let mut graph = default_graph(RenderPath::Forward);
        custom_aa(&mut graph, |_| {});
        assert_eq!(remove_pass_with_bypass(&mut graph, "aa"), Some(1));
        let mut graph = default_graph(RenderPath::Forward);
        custom_aa(&mut graph, |manifest| {
            let mut other = manifest.outputs[0].clone();
            other.name = "other".into();
            manifest.outputs.push(other);
        });
        assert_eq!(remove_pass_with_bypass(&mut graph, "aa"), Some(0));
        let mut graph = default_graph(RenderPath::Forward);
        custom_aa(&mut graph, |manifest| {
            let mut other = manifest.inputs[0].clone();
            other.name = "alternate".into();
            manifest.inputs.push(other);
        });
        assert_eq!(remove_pass_with_bypass(&mut graph, "aa"), Some(0));
    }

    #[test]
    fn ambiguous_or_missing_source_and_unknown_output_do_not_reconnect() {
        let mut graph = default_graph(RenderPath::Forward);
        graph.edges.push(make_edge("ao", "color", "aa", "color"));
        assert_eq!(remove_pass_with_bypass(&mut graph, "aa"), Some(0));
        let mut graph = default_graph(RenderPath::Forward);
        graph.edges.retain(|edge| edge.to_node != "aa");
        assert_eq!(remove_pass_with_bypass(&mut graph, "aa"), Some(0));
        let mut graph = default_graph(RenderPath::Forward);
        graph
            .edges
            .iter_mut()
            .find(|edge| edge.from_node == "aa")
            .unwrap()
            .from_port = "misspelled".into();
        assert_eq!(remove_pass_with_bypass(&mut graph, "aa"), Some(0));
    }

    #[test]
    fn invalid_manifest_is_removed_without_inventing_contracts() {
        let mut graph = default_graph(RenderPath::Forward);
        custom_aa(&mut graph, |manifest| manifest.inputs[0].samples = 2);
        assert_eq!(remove_pass_with_bypass(&mut graph, "aa"), Some(0));
        let mut graph = default_graph(RenderPath::Forward);
        let node = graph.nodes.iter_mut().find(|node| node.id == "aa").unwrap();
        node.module = "gpu.custom".into();
        node.params = json!({"manifest":{"stage":17}});
        let (passes, resources) = render_graph_targets(&graph);
        assert!(passes.contains("aa"));
        assert!(!resources.contains("aa.color"));
        assert_eq!(remove_pass_with_bypass(&mut graph, "aa"), Some(0));
    }

    #[test]
    fn absent_and_unused_passes_are_distinguished_and_targets_are_exact() {
        let mut graph = default_graph(RenderPath::Forward);
        let before = graph.clone();
        assert_eq!(remove_pass_with_bypass(&mut graph, "missing"), None);
        assert_eq!(graph, before);
        let (passes, resources) = render_graph_targets(&graph);
        assert!(passes.contains("aa"));
        assert!(!passes.contains("scene"));
        assert!(!passes.contains("frame"));
        assert!(resources.contains("aa.color"));
        assert!(resources.contains("geometry.depth"));
        assert!(!resources.contains("aa.unknown"));
        graph.edges.retain(|edge| edge.from_node != "aa");
        assert_eq!(remove_pass_with_bypass(&mut graph, "aa"), Some(0));
    }
}
