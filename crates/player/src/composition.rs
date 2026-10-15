use module_engine_core::{
    CompositionContext, ModuleRegistry, ResourceAccess, SystemDescriptor, SystemPort,
    SystemRegistry,
};
use module_engine_host_services::HostServiceRegistry;
use std::{collections::BTreeSet, path::Path};

pub fn compiled_features() -> BTreeSet<String> {
    [
        ("ui", cfg!(feature = "ui")),
        ("arpg", cfg!(feature = "arpg")),
        ("puzzle", cfg!(feature = "puzzle")),
        ("vulkan", cfg!(feature = "vulkan")),
        ("audio", cfg!(feature = "audio")),
    ]
    .into_iter()
    .filter(|(_, enabled)| *enabled)
    .map(|(id, _)| id.into())
    .collect()
}

pub fn composition_at(root: &Path) -> Result<CompositionContext, String> {
    composition_with(root, |_, _| Ok(()))
}

/// Native applications can install additional source contracts in this same
/// catalog before module/schema metadata is synchronized. This does not change
/// document activity or the authored execution order.
pub fn composition_with(
    root: &Path,
    install: impl FnOnce(&mut ModuleRegistry, &mut SystemRegistry) -> Result<(), String>,
) -> Result<CompositionContext, String> {
    let (mut modules, mut systems) = catalogs_at(root, &compiled_features())?;
    install(&mut modules, &mut systems)?;
    CompositionContext::new(modules, systems)
}

/// Editor metadata checks can model a smaller compiled Player using only the
/// source packages actually available in this binary. This is installation
/// metadata, never a second document activation list.
pub fn composition_for_features(
    root: &Path,
    features: &BTreeSet<String>,
) -> Result<CompositionContext, String> {
    let (modules, systems) = catalogs_at(root, features)?;
    CompositionContext::new(modules, systems)
}

fn catalogs_at(
    root: &Path,
    features: &BTreeSet<String>,
) -> Result<(ModuleRegistry, SystemRegistry), String> {
    if !features.is_subset(&compiled_features()) {
        return Err("Player package requests native features absent from this build".into());
    }
    let mut modules = ModuleRegistry::builtin();
    #[cfg(feature = "vulkan")]
    if features.contains("vulkan") {
        module_engine_vulkan::install(&mut modules, root)?;
    }
    #[cfg(not(feature = "vulkan"))]
    let _ = (root, &mut modules);
    let mut systems = SystemRegistry::builtin();
    crate::source_modules::install(&mut systems)?;
    #[cfg(feature = "arpg")]
    if features.contains("arpg") {
        module_engine_arpg::install(&mut systems)?;
    }
    #[cfg(feature = "puzzle")]
    if features.contains("puzzle") {
        module_engine_project_puzzle::install(&mut systems)?;
    }
    systems.register(custom_system_descriptor(), |_| {
        Err("Rust Custom requires a host-bound compiled instance".into())
    })?;
    Ok((modules, systems))
}

pub fn service_registry() -> Result<HostServiceRegistry, String> {
    let mut services = HostServiceRegistry::new();
    #[cfg(feature = "audio")]
    module_engine_host_services::install_audio(&mut services)?;
    #[cfg(not(feature = "audio"))]
    let _ = &mut services;
    Ok(services)
}

fn custom_system_descriptor() -> SystemDescriptor {
    SystemDescriptor {
        id: "custom.rust.update".into(),
        state_schema: "owned-rust-state-v1".into(),
        components: vec![],
        requires: vec![],
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
    }
}
