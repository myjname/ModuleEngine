//! Application policy and checked roots for the shared host.
pub use crate::game_templates::{demo_document, puzzle_document};
use module_engine_core::*;
pub use module_engine_host::{GameHost, ScriptSlot, script_ids};
use module_engine_host::{HostOptions, RenderMode};
use module_engine_host_services::{HostServiceRegistry, install_audio};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

pub fn new(
    document: GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
) -> Result<GameHost, String> {
    let backend = backend_root_for(root, compile_root)?;
    new_at(document, root, &backend, compile_root, kind)
}
pub fn new_at(
    document: GraphDocument,
    root: &Path,
    backend_root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
) -> Result<GameHost, String> {
    new_at_with_progress(document, root, backend_root, compile_root, kind, None)
}
pub fn new_at_with_progress(
    document: GraphDocument,
    root: &Path,
    backend_root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
    progress: Option<&crate::activity::BuildProgress>,
) -> Result<GameHost, String> {
    new_at_with_progress_cancellable(
        document,
        root,
        backend_root,
        compile_root,
        kind,
        progress,
        &AtomicBool::new(false),
    )
}
pub fn new_at_with_progress_cancellable(
    mut document: GraphDocument,
    root: &Path,
    backend_root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
    progress: Option<&crate::activity::BuildProgress>,
    cancelled: &AtomicBool,
) -> Result<GameHost, String> {
    // Compatibility for the existing world selector: make its selection an
    // explicit graph change before entering the shared host.
    if kind != WorldKind::Custom {
        let modules = crate::host::composition_at(backend_root)?.modules;
        let world_id = modules.render_route(&document)?.world.id.clone();
        let world = document
            .root
            .nodes
            .iter_mut()
            .find(|n| n.id == world_id)
            .unwrap();
        if matches!(world.module.as_str(), "world.objects" | "world.ecs") {
            world.module = match kind {
                WorldKind::Objects => "world.objects",
                _ => "world.ecs",
            }
            .into();
        }
    }
    let composition = crate::host::composition_at(backend_root)?;
    let mut services = HostServiceRegistry::new();
    install_audio(&mut services)?;
    GameHost::build_cancellable(
        document,
        composition,
        HostOptions {
            asset_root: root.into(),
            backend_root: Some(backend_root.into()),
            compile_root: compile_root.map(Path::to_path_buf),
            render: RenderMode::Graph,
            services,
            services_enabled: true,
        },
        progress,
        cancelled,
    )
}

pub fn graphics_only_change(previous: &GraphDocument, document: &GraphDocument) -> bool {
    comparison_registry().is_some_and(|modules| {
        let mut previous = previous.clone();
        let mut document = document.clone();
        previous.root = connected_root(&previous.root);
        document.root = connected_root(&document.root);
        module_engine_host::graphics_only_change(modules, &previous, &document)
    })
}
pub fn engine_structure_eq(previous: &Graph, graph: &Graph) -> bool {
    if previous.execution_eq(graph) {
        return true;
    }
    comparison_registry().is_some_and(|modules| {
        module_engine_host::engine_structure_eq(
            modules,
            &connected_root(previous),
            &connected_root(graph),
        )
    })
}
fn connected_root(graph: &Graph) -> Graph {
    let mut active = std::collections::BTreeSet::new();
    let mut pending = graph
        .nodes
        .iter()
        .filter(|node| node.module == "viewport.output")
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if active.insert(id.clone()) {
            pending.extend(
                graph
                    .edges
                    .iter()
                    .filter(|edge| edge.to_node == id)
                    .map(|edge| edge.from_node.clone()),
            );
        }
    }
    let mut connected = graph.clone();
    connected.nodes.retain(|node| active.contains(&node.id));
    connected
        .edges
        .retain(|edge| active.contains(&edge.from_node) && active.contains(&edge.to_node));
    connected
}
fn comparison_registry() -> Option<&'static ModuleRegistry> {
    static MODULES: std::sync::OnceLock<Result<ModuleRegistry, String>> =
        std::sync::OnceLock::new();
    MODULES.get_or_init(crate::host::registry).as_ref().ok()
}
/// Selection follows the connected renderer route. Archived canvas nodes do
/// not choose storage, and native providers retain their registered kind.
pub fn selected_world_kind(
    document: &GraphDocument,
    modules: &ModuleRegistry,
) -> Result<WorldKind, String> {
    let world = modules.render_route(document)?.world;
    modules
        .world_metadata(&world.module)
        .map(|metadata| metadata.kind)
        .ok_or_else(|| format!("Missing selected world metadata: {}", world.module))
}
pub fn world_kind(document: &GraphDocument) -> Result<WorldKind, String> {
    selected_world_kind(
        document,
        comparison_registry().ok_or("Installed world catalog is unavailable")?,
    )
}
#[derive(Clone)]
pub struct ShaderDependencyWatch {
    modules: ModuleRegistry,
    inner: module_engine_host::RendererDependencyWatch,
}
impl ShaderDependencyWatch {
    pub fn new(document: &GraphDocument, root: &Path) -> Self {
        let modules = crate::host::composition_at(root)
            .map(|c| c.modules)
            .unwrap_or_default();
        Self {
            inner: module_engine_host::RendererDependencyWatch::new(document, &modules),
            modules,
        }
    }
    pub fn changed(&mut self, document: &GraphDocument, _root: &Path) -> bool {
        self.inner.changed(document, &self.modules)
    }
}
pub fn render_graph(document: &GraphDocument) -> Graph {
    let Ok(composition) = crate::host::composition_at(&module_engine_vulkan::resource_root())
    else {
        return Graph::default();
    };
    composition
        .modules
        .render_route(document)
        .ok()
        .and_then(|r| r.renderer.subgraph.as_deref())
        .cloned()
        .unwrap_or_default()
}
pub fn game_registry_at(
    document: &GraphDocument,
    root: &Path,
    backend: &Path,
    slots: &BTreeMap<String, ScriptSlot>,
    overrides: Arc<Mutex<RenderOverrides>>,
) -> Result<SystemRegistry, String> {
    module_engine_host::bind_game_registry(
        Arc::new(Mutex::new(document.clone())),
        root,
        &crate::host::composition_at(backend)?,
        slots,
        overrides,
    )
}
/// Resolve only the package selected and checked by distribution policy. A
/// relocated authored project supplies assets; it never has to contain DXC or
/// installed backend files. Precompiled Player packages own both roots.
pub fn backend_root_for(asset_root: &Path, compile_root: Option<&Path>) -> Result<PathBuf, String> {
    let Some(compile_root) = compile_root else {
        return asset_root.canonicalize().map_err(|e| e.to_string());
    };
    let compile_root = compile_root.canonicalize().map_err(|e| e.to_string())?;
    // A Full source project can itself be named `.sdk`. Native editor sources
    // distinguish it from the intentionally reduced Team Custom SDK.
    if compile_root.join("crates/editor/src/main.rs").is_file() {
        return Ok(compile_root);
    }
    if compile_root.file_name().is_some_and(|name| name == ".sdk")
        && compile_root.join("crates/engine-core/Cargo.toml").is_file()
        && let Some(package) = compile_root.parent()
    {
        let manifest: crate::distribution::PackageManifest = serde_json::from_slice(
            &std::fs::read(package.join(crate::distribution::MANIFEST_NAME))
                .map_err(|e| format!("Selected Team backend package unavailable: {e}"))?,
        )
        .map_err(|e| format!("Selected Team backend manifest: {e}"))?;
        if manifest.profile == crate::distribution::DistributionProfile::Team
            && manifest.engine_version == env!("CARGO_PKG_VERSION")
        {
            return Ok(package.to_path_buf());
        }
    }
    Err(
        "Selected compile root is neither a Full engine source project nor its checked Team SDK"
            .into(),
    )
}

#[cfg(test)]
mod world_selection_tests {
    use super::*;
    #[test]
    fn selection_uses_active_world_metadata_and_ignores_disconnected_ecs() {
        let mut modules = ModuleRegistry::builtin();
        modules
            .register_world(
                world_descriptor(
                    "world.native_fixture",
                    "Native fixture",
                    "Not a built-in store",
                ),
                |_| Err("Selection must not construct a world".into()),
            )
            .unwrap();
        let mut document = module_engine_core::demo_document(WorldKind::Objects);
        document.root.nodes.push(make_node(
            "archived",
            "world.ecs",
            "Archived",
            [0., 0.],
            serde_json::json!({}),
        ));
        assert_eq!(
            selected_world_kind(&document, &modules).unwrap(),
            WorldKind::Objects
        );
        let mut active_only = document.clone();
        active_only.root.nodes.retain(|node| node.id != "archived");
        assert!(engine_structure_eq(&document.root, &active_only.root));
        assert!(graphics_only_change(&document, &active_only));
        document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == "world")
            .unwrap()
            .module = "world.native_fixture".into();
        assert_eq!(
            selected_world_kind(&document, &modules).unwrap(),
            WorldKind::Custom
        );
        document.root.nodes.retain(|node| node.id != "archived");
        assert_eq!(
            selected_world_kind(&document, &modules).unwrap(),
            WorldKind::Custom
        );
    }
}
