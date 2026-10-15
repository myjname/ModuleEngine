use crate::{ModuleRegistry, PortDescriptor, WorldKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Current serialized graph schema; newer versions are rejected explicitly.
pub const SCHEMA_VERSION: u32 = 3;

/// Human-editable project graph and source text owned by Custom nodes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GraphDocument {
    pub schema_version: u32,
    pub name: String,
    pub root: Graph,
    #[serde(default)]
    pub custom_sources: BTreeMap<String, String>,
    #[serde(default)]
    pub shader_sources: BTreeMap<String, String>,
    #[serde(default)]
    pub asset_manifest: Value,
    #[serde(default)]
    pub module_versions: BTreeMap<String, String>,
    #[serde(default)]
    pub engine: crate::EngineConfiguration,
    #[serde(default)]
    pub project: crate::ProjectConfiguration,
    #[serde(default)]
    pub properties: crate::PropertyStore,
    /// Optional authored game; legacy graph-only projects remain valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<crate::GameProject>,
}

/// A graph scope. Node identifiers are unique within this scope.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// An installed module instance with optional, independently validated internals.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub module: String,
    pub label: String,
    pub position: [f32; 2],
    #[serde(default)]
    pub params: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subgraph: Option<Box<Graph>>,
}

/// A directed connection from one output to exactly one input.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from_node: String,
    pub from_port: String,
    pub to_node: String,
    pub to_port: String,
}

impl Graph {
    /// Compare executable connections and parameters, excluding only editor
    /// positions, labels and the two Rust nodes' visual-code layout metadata.
    /// Node/edge order is retained, as are unknown parameters and nested scopes.
    pub fn execution_eq(&self, other: &Self) -> bool {
        self.edges == other.edges
            && self.nodes.len() == other.nodes.len()
            && self.nodes.iter().zip(&other.nodes).all(|(left, right)| {
                left.id == right.id
                    && left.module == right.module
                    && params_execution_eq(left, right)
                    && match (&left.subgraph, &right.subgraph) {
                        (Some(left), Some(right)) => left.execution_eq(right),
                        (None, None) => true,
                        _ => false,
                    }
            })
    }
}

fn params_execution_eq(left: &Node, right: &Node) -> bool {
    if !matches!(left.module.as_str(), "custom.rust" | "custom.rust.update") {
        return left.params == right.params;
    }
    let (Some(left), Some(right)) = (left.params.as_object(), right.params.as_object()) else {
        return left.params == right.params;
    };
    let is_layout = |key: &str| matches!(key, "code_layout" | "code_layout_keys");
    left.keys().filter(|key| !is_layout(key)).count()
        == right.keys().filter(|key| !is_layout(key)).count()
        && left
            .iter()
            .filter(|(key, _)| !is_layout(key))
            .all(|(key, value)| right.get(key) == Some(value))
}

impl GraphDocument {
    /// A conservative document comparison that ignores graph presentation only.
    /// Sources, assets, scenes, settings and every other document field must match.
    pub fn execution_eq(&self, other: &Self) -> bool {
        // Keep this exhaustive so adding a document field also requires deciding
        // whether that field may bypass execution/author-scene reconstruction.
        let Self {
            schema_version,
            name,
            root,
            custom_sources,
            shader_sources,
            asset_manifest,
            module_versions,
            engine,
            project,
            properties,
            game,
        } = self;
        *schema_version == other.schema_version
            && name == &other.name
            && root.execution_eq(&other.root)
            && custom_sources == &other.custom_sources
            && shader_sources == &other.shader_sources
            && asset_manifest == &other.asset_manifest
            && module_versions == &other.module_versions
            && engine == &other.engine
            && project == &other.project
            && properties == &other.properties
            && game == &other.game
    }
}

/// A validation diagnostic suitable for display by the editor or CLI.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub node: Option<String>,
}

/// Errors prevent reconstruction; warnings identify unexecuted graph fragments.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// Validate schema, identifiers, pins, types, required connections, cycles and
/// nested interfaces. Validation does not claim every typed graph is executable.
pub fn validate_graph(document: &GraphDocument, registry: &ModuleRegistry) -> Vec<Diagnostic> {
    let mut result = Vec::new();
    if let Some(game) = &document.game
        && let Err(error) = game.validate_scene_data()
    {
        emit(&mut result, Severity::Error, None, error);
    }
    if let Err(error) = crate::validate_settings(document, registry) {
        emit(&mut result, Severity::Error, None, error);
    }
    if !(1..=SCHEMA_VERSION).contains(&document.schema_version) {
        emit(
            &mut result,
            Severity::Error,
            None,
            format!(
                "Unsupported schema version {}; expected {}",
                document.schema_version, SCHEMA_VERSION
            ),
        );
    }
    for (module, pinned) in &document.module_versions {
        if registry.module_version(module) != Some(pinned.as_str()) {
            emit(
                &mut result,
                Severity::Error,
                None,
                format!(
                    "Module '{module}' requires version '{pinned}', installed: {}",
                    registry.module_version(module).unwrap_or("missing")
                ),
            );
        }
    }
    validate_scope(&document.root, registry, "", None, &mut result);
    let mut source_modes = BTreeMap::new();
    for node in &document.root.nodes {
        if node.module == "custom.rust" || node.module == "custom.rust.update" {
            let key = node.params.get("source_id").and_then(Value::as_str);
            if let Some(key) = key
                && let Some(previous) = source_modes.insert(key, node.module.as_str())
                && previous != node.module
            {
                emit(
                    &mut result,
                    Severity::Error,
                    Some(node.id.clone()),
                    format!(
                        "Rust source '{key}' cannot share speed and update modes; use separate source IDs"
                    ),
                );
            }
            if key.is_none()
                || !document
                    .custom_sources
                    .contains_key(key.unwrap_or_default())
            {
                emit(
                    &mut result,
                    Severity::Error,
                    Some(node.id.clone()),
                    "Custom node needs source_id referring to custom_sources".into(),
                );
            }
        }
    }
    // Report disconnected nodes rather than executing unrelated fragments.
    let outputs: Vec<_> = document
        .root
        .nodes
        .iter()
        .filter(|n| n.module == "viewport.output")
        .collect();
    let mut active = BTreeSet::new();
    let mut pending: Vec<String> = outputs.iter().map(|n| n.id.clone()).collect();
    while let Some(id) = pending.pop() {
        if active.insert(id.clone()) {
            pending.extend(
                document
                    .root
                    .edges
                    .iter()
                    .filter(|e| e.to_node == id)
                    .map(|e| e.from_node.clone()),
            );
        }
    }
    for node in &document.root.nodes {
        if !active.contains(&node.id) {
            emit(
                &mut result,
                Severity::Warning,
                Some(node.id.clone()),
                "Node is not connected to a viewport and is not executed".into(),
            );
        }
    }
    result
}

fn emit(out: &mut Vec<Diagnostic>, severity: Severity, node: Option<String>, message: String) {
    out.push(Diagnostic {
        severity,
        message,
        node,
    });
}

fn scoped(scope: &str, id: &str) -> String {
    if scope.is_empty() {
        id.into()
    } else {
        format!("{scope}/{id}")
    }
}

/// Resolve dynamic pins of a nested interface; regular pins come from registry.
pub fn node_ports(
    node: &Node,
    registry: &ModuleRegistry,
) -> Option<(Vec<PortDescriptor>, Vec<PortDescriptor>)> {
    let descriptor = registry.descriptor(&node.module)?;
    if node.module == "interface.input" || node.module == "interface.output" {
        let data_type = node.params.get("data_type")?.as_str()?.to_owned();
        let port = PortDescriptor {
            name: "value".into(),
            data_type,
            required: node.module == "interface.output",
        };
        return Some(if node.module == "interface.input" {
            (vec![], vec![port])
        } else {
            (vec![port], vec![])
        });
    }
    if node.module == "gpu.custom" {
        fn pins(value: Option<&Value>, inputs: bool) -> Option<Vec<PortDescriptor>> {
            let Some(value) = value else {
                return Some(vec![]);
            };
            value
                .as_array()?
                .iter()
                .map(|spec| {
                    let name = spec.get("name")?.as_str()?.to_owned();
                    let semantic = spec
                        .get("semantic")
                        .and_then(Value::as_str)
                        .unwrap_or("color")
                        .to_ascii_lowercase();
                    let explicit = spec
                        .get("data_type")
                        .or_else(|| spec.get("type"))
                        .and_then(Value::as_str);
                    let data_type = explicit
                        .unwrap_or(
                            if name == "scene" || semantic == "scene" || semantic == "scene3d" {
                                "Scene3D"
                            } else if semantic.contains("normal") {
                                "GpuNormals"
                            } else if semantic.contains("depth") {
                                "GpuDepth"
                            } else if name == "frame" || semantic == "srgb_display" {
                                "Frame3D"
                            } else {
                                "GpuColor"
                            },
                        )
                        .to_owned();
                    Some(PortDescriptor {
                        name,
                        data_type,
                        required: inputs
                            && spec
                                .get("required")
                                .and_then(Value::as_bool)
                                .unwrap_or(true),
                    })
                })
                .collect()
        }
        let manifest = node.params.get("manifest").unwrap_or(&node.params);
        return Some((
            pins(manifest.get("inputs"), true)?,
            pins(manifest.get("outputs"), false)?,
        ));
    }
    Some((descriptor.inputs.clone(), descriptor.outputs.clone()))
}

fn validate_scope(
    graph: &Graph,
    registry: &ModuleRegistry,
    scope: &str,
    parent: Option<&Node>,
    out: &mut Vec<Diagnostic>,
) {
    let mut nodes = BTreeMap::new();
    for node in &graph.nodes {
        let id = scoped(scope, &node.id);
        if node.id.trim().is_empty() || nodes.insert(node.id.as_str(), node).is_some() {
            emit(
                out,
                Severity::Error,
                Some(id.clone()),
                "Node identifier is empty or duplicated".into(),
            );
        }
        if registry.descriptor(&node.module).is_none() {
            emit(
                out,
                Severity::Error,
                Some(id.clone()),
                format!("Module '{}' is not registered", node.module),
            );
        }
        if !node.position.iter().all(|v| v.is_finite()) {
            emit(
                out,
                Severity::Error,
                Some(id.clone()),
                "Node position must be finite".into(),
            );
        }
        if node.module == "scalar.value"
            && node.params.get("value").and_then(Value::as_f64).is_none()
        {
            emit(
                out,
                Severity::Error,
                Some(id.clone()),
                "Scalar value needs a numeric value parameter".into(),
            );
        }
        if node.module.starts_with("interface.") {
            if parent.is_none() {
                emit(
                    out,
                    Severity::Error,
                    Some(id.clone()),
                    "Interface nodes belong inside a compound module".into(),
                );
            }
            if node.params.get("port").and_then(Value::as_str).is_none()
                || node_ports(node, registry).is_none()
            {
                emit(
                    out,
                    Severity::Error,
                    Some(id.clone()),
                    "Interface needs port and data_type parameters".into(),
                );
            }
        }
        if node.module == "render.software3d" && node.subgraph.is_none() {
            emit(
                out,
                Severity::Error,
                Some(id.clone()),
                "Software3D requires an internal graph".into(),
            );
        }
        if let Some(inner) = &node.subgraph {
            validate_scope(inner, registry, &id, Some(node), out);
        }
    }
    let mut connected = BTreeSet::new();
    for edge in &graph.edges {
        let Some(from) = nodes.get(edge.from_node.as_str()) else {
            emit(
                out,
                Severity::Error,
                None,
                format!("Edge source '{}' does not exist", edge.from_node),
            );
            continue;
        };
        let Some(to) = nodes.get(edge.to_node.as_str()) else {
            emit(
                out,
                Severity::Error,
                None,
                format!("Edge destination '{}' does not exist", edge.to_node),
            );
            continue;
        };
        if !connected.insert((edge.to_node.clone(), edge.to_port.clone())) {
            emit(
                out,
                Severity::Error,
                Some(scoped(scope, &to.id)),
                format!("Input '{}' has multiple connections", edge.to_port),
            );
        }
        let output = node_ports(from, registry)
            .and_then(|(_, p)| p.into_iter().find(|p| p.name == edge.from_port));
        let input = node_ports(to, registry)
            .and_then(|(p, _)| p.into_iter().find(|p| p.name == edge.to_port));
        match (output, input) {
            (Some(a), Some(b)) if a.data_type != b.data_type => emit(
                out,
                Severity::Error,
                Some(scoped(scope, &to.id)),
                format!("Type mismatch: {} -> {}", a.data_type, b.data_type),
            ),
            (None, _) => emit(
                out,
                Severity::Error,
                Some(scoped(scope, &from.id)),
                format!("Unknown output '{}'", edge.from_port),
            ),
            (_, None) => emit(
                out,
                Severity::Error,
                Some(scoped(scope, &to.id)),
                format!("Unknown input '{}'", edge.to_port),
            ),
            _ => {}
        }
    }
    for node in &graph.nodes {
        if let Some((inputs, _)) = node_ports(node, registry) {
            for pin in inputs.iter().filter(|p| p.required) {
                if !connected.contains(&(node.id.clone(), pin.name.clone())) {
                    emit(
                        out,
                        Severity::Error,
                        Some(scoped(scope, &node.id)),
                        format!("Required input '{}' is disconnected", pin.name),
                    );
                }
            }
        }
    }
    // Kahn's algorithm counts every edge, including parallel output consumers.
    let mut indegree: BTreeMap<&str, usize> = nodes.keys().map(|id| (*id, 0)).collect();
    for edge in &graph.edges {
        if nodes.contains_key(edge.from_node.as_str())
            && let Some(n) = indegree.get_mut(edge.to_node.as_str())
        {
            *n += 1;
        }
    }
    let mut queue: Vec<&str> = indegree
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut visited = 0;
    while let Some(id) = queue.pop() {
        visited += 1;
        for edge in graph.edges.iter().filter(|e| e.from_node == id) {
            if let Some(n) = indegree.get_mut(edge.to_node.as_str()) {
                *n -= 1;
                if *n == 0 {
                    queue.push(edge.to_node.as_str());
                }
            }
        }
    }
    if visited != nodes.len() {
        emit(
            out,
            Severity::Error,
            parent
                .map(|_| scope.to_owned())
                .or_else(|| graph.nodes.first().map(|n| n.id.clone())),
            "Graph contains a dependency cycle".into(),
        );
    }
    if let Some(parent) = parent
        && let Some(descriptor) = registry.descriptor(&parent.module)
    {
        for (module, pins) in [
            ("interface.input", &descriptor.inputs),
            ("interface.output", &descriptor.outputs),
        ] {
            for pin in pins {
                let matches: Vec<_> = graph
                    .nodes
                    .iter()
                    .filter(|n| {
                        n.module == module
                            && n.params.get("port").and_then(Value::as_str)
                                == Some(pin.name.as_str())
                    })
                    .collect();
                // Optional host controls need not be forwarded into the nested
                // resource graph. Required data inputs and all outputs do.
                if module == "interface.input" && !pin.required && matches.is_empty() {
                    continue;
                }
                if matches.len() != 1
                    || matches[0].params.get("data_type").and_then(Value::as_str)
                        != Some(pin.data_type.as_str())
                {
                    emit(
                        out,
                        Severity::Error,
                        Some(scope.into()),
                        format!(
                            "Nested {module} must map '{}' exactly once with type {}",
                            pin.name, pin.data_type
                        ),
                    );
                }
            }
            for node in graph.nodes.iter().filter(|n| n.module == module) {
                if !pins.iter().any(|p| {
                    node.params.get("port").and_then(Value::as_str) == Some(p.name.as_str())
                }) {
                    emit(
                        out,
                        Severity::Error,
                        Some(scoped(scope, &node.id)),
                        "Nested interface maps an unknown parent pin".into(),
                    );
                }
            }
        }
    }
}

/// Internal graph executed by the prototype Software3D implementation.
pub fn software3d_graph() -> Graph {
    Graph {
        nodes: vec![
            make_node(
                "scene-in",
                "interface.input",
                "Scene input",
                [35.0, 70.0],
                json!({"port":"scene","data_type":"Scene3D"}),
            ),
            make_node(
                "rasterizer",
                "render.rasterize",
                "Depth rasterizer",
                [270.0, 70.0],
                json!({}),
            ),
            make_node(
                "frame-out",
                "interface.output",
                "Frame output",
                [505.0, 70.0],
                json!({"port":"frame","data_type":"Frame3D"}),
            ),
        ],
        edges: vec![
            make_edge("scene-in", "value", "rasterizer", "scene"),
            make_edge("rasterizer", "frame", "frame-out", "value"),
        ],
    }
}

/// Deterministic example with a real nested renderer and connected Custom speed.
pub fn demo_document(kind: WorldKind) -> GraphDocument {
    let mut renderer = make_node(
        "renderer",
        "render.software3d",
        "Software 3D",
        [580.0, 120.0],
        json!({}),
    );
    renderer.subgraph = Some(Box::new(software3d_graph()));
    let module = match kind {
        WorldKind::Objects => "world.objects",
        WorldKind::Ecs => "world.ecs",
        WorldKind::Custom => "world.objects",
    };
    GraphDocument {
        schema_version: SCHEMA_VERSION,
        name: format!("{} world / same 3D renderer", if kind == WorldKind::Ecs { "ECS" } else { "Objects" }),
        root: Graph {
            nodes: vec![
                make_node("world", module, "World", [70.0, 120.0], json!({})),
                make_node("extract", "scene.extract", "Extract scene", [325.0, 120.0], json!({})),
                renderer,
                make_node("viewport", "viewport.output", "Viewport", [840.0, 120.0], json!({})),
                make_node("clock", "clock.time", "Time", [40.0, 375.0], json!({})),
                make_node("base", "scalar.value", "Base speed", [40.0, 540.0], json!({"value":1.0})),
                make_node("speed", "custom.rust", "Custom speed", [315.0, 400.0], json!({"source_id":"speed"})),
            ],
            edges: vec![
                make_edge("world", "world", "extract", "world"),
                make_edge("extract", "scene", "renderer", "scene"),
                make_edge("renderer", "frame", "viewport", "frame"),
                make_edge("clock", "value", "speed", "time"),
                make_edge("base", "value", "speed", "base"),
                make_edge("speed", "value", "world", "speed"),
            ],
        },
        custom_sources: BTreeMap::from([("speed".into(), "fn speed(time: f64, base: f64) -> f64 {\n    let wave = time.sin();\n    base + wave * 0.25\n}\n".into())]),
        shader_sources: BTreeMap::new(),
        asset_manifest: Value::Null,
        module_versions: BTreeMap::new(),
        engine: crate::EngineConfiguration::default(),
        project: crate::ProjectConfiguration::default(),
        properties: crate::PropertyStore::default(),
        game: None,
    }
}

/// Convenience constructor for an ordinary module node.
pub fn make_node(id: &str, module: &str, label: &str, position: [f32; 2], params: Value) -> Node {
    Node {
        id: id.into(),
        module: module.into(),
        label: label.into(),
        position,
        params,
        subgraph: None,
    }
}

/// Convenience constructor for a connection.
pub fn make_edge(from_node: &str, from_port: &str, to_node: &str, to_port: &str) -> Edge {
    Edge {
        from_node: from_node.into(),
        from_port: from_port.into(),
        to_node: to_node.into(),
        to_port: to_port.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Runtime;

    #[test]
    fn execution_equality_ignores_nested_positions_labels_and_only_rust_layout_keys() {
        let mut original = demo_document(WorldKind::Objects);
        original.root.nodes.push(make_node(
            "update",
            "custom.rust.update",
            "Update",
            [1., 2.],
            json!({"source_id":"update"}),
        ));
        let mut moved = original.clone();
        fn change_layout(graph: &mut Graph) {
            for node in &mut graph.nodes {
                node.position[0] += 50.;
                node.label.push_str(" edited label");
                if matches!(node.module.as_str(), "custom.rust" | "custom.rust.update") {
                    node.params["code_layout"] = json!({"statement": [30., 40.]});
                    node.params["code_layout_keys"] = json!(["statement"]);
                }
                if let Some(inner) = &mut node.subgraph {
                    change_layout(inner);
                }
            }
        }
        change_layout(&mut moved.root);
        assert_ne!(original, moved);
        assert!(original.execution_eq(&moved));
        assert!(moved.execution_eq(&original));
        // The same spelling on another module can be a real module parameter.
        moved.root.nodes[0].params["code_layout"] = json!({});
        assert!(!original.execution_eq(&moved));
    }

    #[test]
    fn execution_equality_retains_parameters_bindings_order_and_nested_structure() {
        let original = demo_document(WorldKind::Objects).root;
        let changes: [fn(&mut Graph); 10] = [
            |g| g.nodes[0].id.push_str(" changed"),
            |g| g.nodes[0].module.push_str(" changed"),
            |g| g.nodes[0].params["speed"] = json!(2.),
            |g| {
                g.nodes
                    .iter_mut()
                    .find(|n| n.module == "custom.rust")
                    .unwrap()
                    .params["source_id"] = json!("another source")
            },
            |g| g.edges[0].to_port.push_str(" changed"),
            |g| {
                g.edges.pop();
            },
            |g| g.nodes.swap(0, 1),
            |g| g.edges.swap(0, 1),
            |g| {
                g.nodes[2].subgraph.as_mut().unwrap().nodes[0].params["gain"] = json!(2.);
            },
            |g| {
                g.nodes[2].subgraph = None;
            },
        ];
        for change in changes {
            let mut changed = original.clone();
            change(&mut changed);
            assert!(!original.execution_eq(&changed), "{changed:?}");
            assert!(!changed.execution_eq(&original));
        }
        let mut malformed = original.clone();
        malformed
            .nodes
            .iter_mut()
            .find(|n| n.module == "custom.rust")
            .unwrap()
            .params = Value::Null;
        assert!(!original.execution_eq(&malformed));
    }

    #[test]
    fn document_execution_equality_checks_all_non_graph_fields() {
        let original = demo_document(WorldKind::Objects);
        let changes: [fn(&mut GraphDocument); 11] = [
            |d| d.schema_version += 1,
            |d| d.name.push_str(" changed"),
            |d| {
                d.custom_sources
                    .get_mut("speed")
                    .unwrap()
                    .push_str("// changed")
            },
            |d| {
                d.shader_sources.insert("new".into(), "shader".into());
            },
            |d| d.asset_manifest = json!({"assets":[]}),
            |d| {
                d.module_versions
                    .insert("world.objects".into(), "other".into());
            },
            |d| d.engine.initialized = !d.engine.initialized,
            |d| {
                d.project.parameters.insert("gain".into(), json!(2.));
            },
            |d| {
                d.properties
                    .active
                    .global
                    .insert("setting".into(), json!(true));
            },
            |d| d.game = Some(crate::GameProject::demo()),
            |d| d.root.nodes[0].params["value"] = json!(2.),
        ];
        for change in changes {
            let mut changed = original.clone();
            change(&mut changed);
            assert!(!original.execution_eq(&changed));
            assert!(!changed.execution_eq(&original));
        }
        let mut authored = original;
        authored.game = Some(crate::GameProject::demo());
        let mut changed = authored.clone();
        changed
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .object
            .transform
            .translation[0] += 1.;
        assert!(!authored.execution_eq(&changed));
        changed = authored.clone();
        changed.game.as_mut().unwrap().execution.stages[0].nodes[0].params["enabled"] =
            json!(false);
        assert!(!authored.execution_eq(&changed));
    }

    #[test]
    fn persistent_nested_document_and_required_edges() {
        let registry = ModuleRegistry::builtin();
        let mut document = demo_document(WorldKind::Objects);
        let text = serde_json::to_string_pretty(&document).unwrap();
        let recovered: GraphDocument = serde_json::from_str(&text).unwrap();
        assert_eq!(document, recovered);
        assert!(validate_graph(&document, &registry).is_empty());
        document.root.edges.retain(|e| e.to_node != "extract");
        assert!(
            validate_graph(&document, &registry)
                .iter()
                .any(|d| d.severity == Severity::Error && d.message.contains("Required input"))
        );
        assert!(Runtime::from_document(&document, &registry).is_err());
    }

    #[test]
    fn rejects_wrong_types_cycles_missing_modules_and_broken_internals() {
        let registry = ModuleRegistry::builtin();
        let mut d = demo_document(WorldKind::Ecs);
        d.root.edges[0].from_node = "base".into();
        d.root.edges[0].from_port = "value".into();
        assert!(
            validate_graph(&d, &registry)
                .iter()
                .any(|d| d.message.contains("Type mismatch"))
        );
        d = demo_document(WorldKind::Ecs);
        d.root
            .edges
            .push(make_edge("speed", "value", "speed", "base"));
        assert!(
            validate_graph(&d, &registry)
                .iter()
                .any(|d| d.message.contains("cycle"))
        );
        d = demo_document(WorldKind::Ecs);
        d.root.nodes[0].module = "world.missing".into();
        assert!(Runtime::from_document(&d, &registry).is_err());
        d = demo_document(WorldKind::Ecs);
        d.root.nodes[2].subgraph.as_mut().unwrap().edges.pop();
        assert!(Runtime::from_document(&d, &registry).is_err());
    }
}
