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
        ("g1", cfg!(feature = "g1")),
        ("vulkan", cfg!(feature = "vulkan")),
        ("audio", cfg!(feature = "audio")),
        ("persistence", cfg!(feature = "persistence")),
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
    #[cfg(feature = "g1")]
    if features.contains("g1") {
        module_engine_g1::install(&mut systems)?;
    }
    systems.register(custom_system_descriptor(), |_| {
        Err("Rust Custom requires a host-bound compiled instance".into())
    })?;
    Ok((modules, systems))
}

pub fn service_registry() -> Result<HostServiceRegistry, String> {
    let services = audio_services()?;
    #[cfg(feature = "persistence")]
    let services = {
        let mut services = services;
        match default_state_root() {
            Ok(root) => module_engine_host_services::install_persistence_at(&mut services, root)?,
            Err(error) => services.register(
                module_engine_host_services::PERSISTENCE_SERVICE_ID,
                move |_, _| Err(error.clone()),
            )?,
        }
        services
    };
    Ok(services)
}

/// The application selects a writable user-data root. Registering a factory
/// performs no IO; only an explicitly requested service can open that root.
pub fn service_registry_at(state_root: std::path::PathBuf) -> Result<HostServiceRegistry, String> {
    let services = audio_services()?;
    #[cfg(feature = "persistence")]
    let services = {
        let mut services = services;
        module_engine_host_services::install_persistence_at(&mut services, state_root)?;
        services
    };
    #[cfg(not(feature = "persistence"))]
    let _ = state_root;
    Ok(services)
}

fn audio_services() -> Result<HostServiceRegistry, String> {
    let services = HostServiceRegistry::new();
    #[cfg(feature = "audio")]
    let services = {
        let mut services = services;
        module_engine_host_services::install_audio(&mut services)?;
        services
    };
    Ok(services)
}

#[cfg(feature = "persistence")]
fn default_state_root() -> Result<std::path::PathBuf, String> {
    state_root_from_environment(
        std::env::var_os("LOCALAPPDATA"),
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("HOME"),
    )
}

#[cfg(feature = "persistence")]
fn state_root_from_environment(
    local: Option<std::ffi::OsString>,
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<std::path::PathBuf, String> {
    // No package/cwd fallback: a tools-free package may be read-only.
    let absolute = |value: Option<std::ffi::OsString>| {
        value
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if let Some(path) = absolute(local).or_else(|| absolute(xdg)) {
        return Ok(path.join("ModuleEngine").join("profiles"));
    }
    if let Some(path) = absolute(home) {
        return Ok(path.join(".local/share/ModuleEngine/profiles"));
    }
    Err(
        "A writable per-user state root is required; no absolute user-data directory is configured"
            .into(),
    )
}

#[cfg(all(test, feature = "persistence"))]
mod state_root_tests {
    use super::state_root_from_environment;
    use std::{ffi::OsString, path::PathBuf};

    fn absolute(name: &str) -> PathBuf {
        let root = if cfg!(windows) {
            PathBuf::from(r"E:\g1-state-root-test")
        } else {
            PathBuf::from("/g1-state-root-test")
        };
        root.join(name)
    }

    #[test]
    fn default_state_root_preserves_absolute_precedence_and_home_suffix() {
        let local = absolute("local");
        let xdg = absolute("xdg");
        let home = absolute("home");
        assert_eq!(
            state_root_from_environment(
                Some(local.clone().into_os_string()),
                Some(xdg.clone().into_os_string()),
                Some(home.clone().into_os_string()),
            )
            .unwrap(),
            local.join("ModuleEngine/profiles")
        );
        assert_eq!(
            state_root_from_environment(None, Some(xdg.clone().into_os_string()), None).unwrap(),
            xdg.join("ModuleEngine/profiles")
        );
        assert_eq!(
            state_root_from_environment(None, None, Some(home.clone().into_os_string())).unwrap(),
            home.join(".local/share/ModuleEngine/profiles")
        );
    }

    #[test]
    fn default_state_root_skips_empty_relative_and_drive_relative_candidates() {
        let xdg = absolute("xdg");
        let home = absolute("home");
        for invalid in [
            "",
            "relative-data",
            r"E:relative-data",
            r"\rooted-without-drive",
        ] {
            assert_eq!(
                state_root_from_environment(
                    Some(OsString::from(invalid)),
                    Some(xdg.clone().into_os_string()),
                    None,
                )
                .unwrap(),
                xdg.join("ModuleEngine/profiles"),
                "LOCALAPPDATA={invalid:?}"
            );
            assert_eq!(
                state_root_from_environment(
                    Some(OsString::from(invalid)),
                    Some(OsString::from("relative-xdg")),
                    Some(home.clone().into_os_string()),
                )
                .unwrap(),
                home.join(".local/share/ModuleEngine/profiles"),
                "LOCALAPPDATA={invalid:?}, relative XDG_DATA_HOME"
            );
        }
    }

    #[test]
    fn default_state_root_rejects_missing_or_all_invalid_candidates() {
        assert!(state_root_from_environment(None, None, None).is_err());
        assert!(
            state_root_from_environment(
                Some(OsString::new()),
                Some(OsString::from("relative-xdg")),
                Some(OsString::from("relative-home")),
            )
            .is_err()
        );
    }
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
