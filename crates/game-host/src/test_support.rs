//! Test-only installation/templates for the preserved host regressions.
use super::*;
pub(super) use module_engine_arpg::ui::aim_at;
use module_engine_vulkan::RenderPath;

pub(super) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
pub(super) fn modules(root: &Path) -> Result<ModuleRegistry, String> {
    let mut modules = ModuleRegistry::builtin();
    module_engine_vulkan::install(&mut modules, root)?;
    Ok(modules)
}
fn composition(root: &Path) -> Result<CompositionContext, String> {
    let mut systems = SystemRegistry::builtin();
    module_engine_arpg::install(&mut systems)?;
    module_engine_project_puzzle::install(&mut systems)?;
    systems.register(
        SystemDescriptor {
            id: "custom.rust.update".into(),
            state_schema: "owned-rust-state-v1".into(),
            ports: vec![
                SystemPort {
                    name: "world".into(),
                    data_type: "GameWorld".into(),
                    access: ResourceAccess::ReadWrite,
                },
                SystemPort {
                    name: "input".into(),
                    data_type: "GameInput".into(),
                    access: ResourceAccess::Read,
                },
                SystemPort {
                    name: "events".into(),
                    data_type: "GameEvents".into(),
                    access: ResourceAccess::ReadWrite,
                },
            ],
            requires: vec![],
            components: vec![],
        },
        |_| Err("Custom needs a host-bound compiled instance".into()),
    )?;
    CompositionContext::new(modules(root)?, systems)
}

pub(super) fn demo_document() -> GraphDocument {
    // These compatibility cases explicitly selected Object storage through the
    // old constructor; the test document now declares that same actual route.
    let mut document = module_engine_core::demo_document(WorldKind::Objects);
    let renderer = document
        .root
        .nodes
        .iter_mut()
        .find(|node| node.id == "renderer")
        .unwrap();
    renderer.module = "render.vulkan".into();
    renderer.subgraph = Some(Box::new(module_engine_vulkan::default_graph(
        RenderPath::Forward,
    )));
    document.engine.initialized = true;
    document.engine.exposure = default_renderer_exposure("tonemap", "aa");
    let mut document = module_engine_two_arenas::document(document);
    approve_fixture(&mut document).unwrap();
    document
}

/// Explicit New/profile-update step for a fixture. Production host paths only
/// validate these records and never call this test-only capture helper.
pub(super) fn approve_fixture(document: &mut GraphDocument) -> Result<(), String> {
    let composition = composition(&workspace_root())?;
    let profile = document
        .engine_lock
        .as_ref()
        .map(|lock| lock.profile.clone())
        .unwrap_or(ProfileIdentity::from_payload(
            "host.fixture",
            "1",
            &json!({"fixture":"host"}),
        )?);
    composition
        .capture_lock(document, profile, Some(SdkIdentity::current()))?
        .apply_to(document);
    Ok(())
}

pub(super) fn new_host(
    document: GraphDocument,
    asset_root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
) -> Result<GameHost, String> {
    new_host_at(
        document,
        asset_root,
        compile_root.unwrap_or(asset_root),
        compile_root,
        kind,
    )
}
pub(super) fn new_host_at(
    mut document: GraphDocument,
    asset_root: &Path,
    backend_root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
) -> Result<GameHost, String> {
    let composition = composition(backend_root)?;
    let world_id = composition
        .modules
        .render_route(&document)?
        .world
        .id
        .clone();
    if kind != WorldKind::Custom {
        let world = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.id == world_id)
            .ok_or("Missing selected world")?;
        world.module = match kind {
            WorldKind::Objects => "world.objects",
            WorldKind::Ecs => "world.ecs",
            WorldKind::Custom => unreachable!(),
        }
        .into();
        world.params = json!({});
    }
    approve_fixture(&mut document)?;
    let mut services = HostServiceRegistry::new();
    module_engine_host_services::install_audio(&mut services)?;
    GameHost::build(
        document,
        composition,
        HostOptions {
            asset_root: asset_root.to_path_buf(),
            backend_root: Some(backend_root.to_path_buf()),
            compile_root: compile_root.map(Path::to_path_buf),
            render: RenderMode::Graph,
            services,
            // These compatibility fixtures opt into native endpoint tests only
            // where playback is implemented; Linux exercises the same host
            // without claiming a silent stub is working audio.
            services_enabled: module_engine_host_services::WAV_PLAYBACK_SUPPORTED,
        },
        None,
    )
}

pub(super) fn selected_render_graph(document: &GraphDocument) -> Result<&Graph, String> {
    let modules = modules(&workspace_root())?;
    modules
        .render_route(document)?
        .renderer
        .subgraph
        .as_deref()
        .ok_or_else(|| "Active renderer has no GPU graph".into())
}
pub(super) fn render_graph(document: &GraphDocument) -> Graph {
    selected_render_graph(document).cloned().unwrap_or_default()
}
pub(super) fn active_graph(host: &GameHost) -> &Graph {
    host.composition
        .modules
        .render_route(host.accepted_document())
        .unwrap()
        .renderer
        .subgraph
        .as_deref()
        .unwrap()
}
pub(super) fn retained_render_controls(
    previous: &Graph,
    graph: &Graph,
    controls: &RenderOverrides,
) -> RenderOverrides {
    let mut old = demo_document();
    let mut next = old.clone();
    old.root
        .nodes
        .iter_mut()
        .find(|node| node.module == "render.vulkan")
        .unwrap()
        .subgraph = Some(Box::new(previous.clone()));
    next.root
        .nodes
        .iter_mut()
        .find(|node| node.module == "render.vulkan")
        .unwrap()
        .subgraph = Some(Box::new(graph.clone()));
    super::retained_render_controls(&modules(&workspace_root()).unwrap(), &old, &next, controls)
        .unwrap()
}
pub(super) fn validate_render_settings_at(
    document: &GraphDocument,
    graph: &Graph,
    root: &Path,
) -> Result<(), String> {
    assert_eq!(selected_render_graph(document)?, graph);
    super::validate_render_settings(&modules(root)?, document)
}
pub(super) fn validate_candidate_controls(
    document: &GraphDocument,
    graph: &Graph,
    controls: &RenderOverrides,
    time: f64,
    root: &Path,
) -> Result<(), String> {
    assert_eq!(selected_render_graph(document)?, graph);
    super::validate_candidate_controls(&modules(root)?, document, controls, time)
}
pub(super) fn graphics_only_change(previous: &GraphDocument, document: &GraphDocument) -> bool {
    super::graphics_only_change(&modules(&workspace_root()).unwrap(), previous, document)
}
pub(super) fn game_registry(
    document: &GraphDocument,
    root: &Path,
    slots: &BTreeMap<String, ScriptSlot>,
    overrides: Arc<Mutex<RenderOverrides>>,
) -> Result<SystemRegistry, String> {
    bind_game_registry(
        Arc::new(Mutex::new(document.clone())),
        root,
        &composition(&workspace_root())?,
        slots,
        overrides,
    )
}
#[cfg(windows)]
pub(super) fn audio_played(host: &GameHost) -> u64 {
    host.services.diagnostics()[module_engine_host_services::WAV_SERVICE_ID]["played"]
        .as_u64()
        .expect("Installed WAV service has a logical event counter")
}
