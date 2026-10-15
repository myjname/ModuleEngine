//! Optional native host services, independent of renderers and game packages.
use module_engine_core::{GameEvent, HostServiceRequest};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

#[cfg(feature = "audio")]
mod audio;
#[cfg(feature = "audio")]
pub use audio::{WAV_PLAYBACK_SUPPORTED, install_audio};

pub const WAV_SERVICE_ID: &str = "audio.wav";

pub struct HostServiceContext<'a> {
    pub asset_root: &'a Path,
    /// Checked portable asset IDs. A service resolves only references it uses.
    pub asset_paths: &'a BTreeMap<String, String>,
    /// A host/actor may retire all its services with one nonblocking store.
    pub retired: Arc<AtomicBool>,
}

pub trait HostService: Send {
    fn consume(&mut self, events: &[GameEvent]) -> Result<(), String>;
    fn reset(&mut self) -> Result<(), String>;
    /// Disable external effects while retaining already loaded resources and
    /// logical diagnostics. Enabling never creates an omitted service.
    fn set_enabled(&mut self, enabled: bool);
    fn stop(&mut self);
    fn diagnostics(&self) -> Value;
}

type Factory = Arc<
    dyn for<'a> Fn(
            &HostServiceRequest,
            &HostServiceContext<'a>,
        ) -> Result<Box<dyn HostService>, String>
        + Send
        + Sync,
>;

#[derive(Clone, Default)]
pub struct HostServiceRegistry {
    factories: BTreeMap<String, Factory>,
}
impl HostServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register<F>(&mut self, id: impl Into<String>, factory: F) -> Result<(), String>
    where
        F: for<'a> Fn(
                &HostServiceRequest,
                &HostServiceContext<'a>,
            ) -> Result<Box<dyn HostService>, String>
            + Send
            + Sync
            + 'static,
    {
        let id = id.into();
        if id.trim().is_empty() || self.factories.contains_key(&id) {
            return Err("Host service needs a unique nonempty factory ID".into());
        }
        self.factories.insert(id, Arc::new(factory));
        Ok(())
    }
    pub fn contains(&self, id: &str) -> bool {
        self.factories.contains_key(id)
    }
    pub fn build(
        &self,
        requests: &[HostServiceRequest],
        context: &HostServiceContext<'_>,
        enabled: bool,
    ) -> Result<HostServices, String> {
        let mut ids = BTreeSet::new();
        for request in requests {
            if request.id.trim().is_empty() || !ids.insert(request.id.clone()) {
                return Err("Host service requests need unique nonempty IDs".into());
            }
        }
        let mut services = HostServices {
            services: BTreeMap::new(),
            enabled,
        };
        // In particular, do not look up optional factories, resolve assets,
        // read WAVs, or create endpoint owners for a disabled host.
        if !enabled {
            return Ok(services);
        }
        for request in requests {
            let factory = self
                .factories
                .get(&request.id)
                .ok_or_else(|| format!("Host service '{}' is not installed", request.id))?;
            let mut service = factory(request, context)
                .map_err(|error| format!("Host service '{}': {error}", request.id))?;
            service.set_enabled(true);
            services.services.insert(request.id.clone(), service);
        }
        Ok(services)
    }
}

#[derive(Default)]
pub struct HostServices {
    services: BTreeMap<String, Box<dyn HostService>>,
    enabled: bool,
}
impl HostServices {
    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }
    pub fn len(&self) -> usize {
        self.services.len()
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        for service in self.services.values_mut() {
            service.set_enabled(enabled);
        }
        self.enabled = enabled;
    }
    pub fn consume(&mut self, events: &[GameEvent]) -> Result<(), String> {
        for (id, service) in &mut self.services {
            service
                .consume(events)
                .map_err(|error| format!("Host service '{id}': {error}"))?;
        }
        Ok(())
    }
    pub fn reset(&mut self) -> Result<(), String> {
        for (id, service) in &mut self.services {
            service
                .reset()
                .map_err(|error| format!("Host service '{id}' reset: {error}"))?;
        }
        Ok(())
    }
    pub fn stop(&mut self) {
        for service in self.services.values_mut() {
            service.stop();
        }
        self.enabled = false;
    }
    pub fn diagnostics(&self) -> Value {
        Value::Object(
            self.services
                .iter()
                .map(|(id, service)| (id.clone(), service.diagnostics()))
                .collect(),
        )
    }
}
impl Drop for HostServices {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn disabled_or_omitted_requests_never_invoke_installed_factories() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let mut registry = HostServiceRegistry::new();
        registry
            .register("fixture.service", move |_, _| {
                seen.fetch_add(1, Ordering::SeqCst);
                Err("Factory must not run".into())
            })
            .unwrap();
        let context = HostServiceContext {
            asset_root: Path::new("missing-root"),
            asset_paths: &BTreeMap::new(),
            retired: Arc::new(AtomicBool::new(false)),
        };
        let request = HostServiceRequest {
            id: "fixture.service".into(),
            config: Value::Null,
            asset_references: BTreeSet::new(),
        };
        assert!(registry.build(&[], &context, true).unwrap().is_empty());
        let mut disabled = registry
            .build(std::slice::from_ref(&request), &context, false)
            .unwrap();
        disabled.set_enabled(true);
        disabled
            .consume(&[GameEvent::new("fixture.event", Value::Null)])
            .unwrap();
        disabled.reset().unwrap();
        disabled.stop();
        assert_eq!(disabled.diagnostics(), json!({}));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(
            registry
                .build(&[request], &context, true)
                .err()
                .unwrap()
                .contains("Factory must not run")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let unknown = HostServiceRequest {
            id: "optional.not-installed".into(),
            config: Value::Null,
            asset_references: BTreeSet::new(),
        };
        assert!(
            HostServiceRegistry::new()
                .build(std::slice::from_ref(&unknown), &context, false)
                .unwrap()
                .is_empty()
        );
        assert!(
            HostServiceRegistry::new()
                .build(&[unknown], &context, true)
                .is_err()
        );
    }

    #[test]
    fn duplicate_requests_are_rejected_before_any_factory_construction() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let mut registry = HostServiceRegistry::new();
        registry
            .register("fixture.service", move |_, _| {
                seen.fetch_add(1, Ordering::SeqCst);
                Err("not reached".into())
            })
            .unwrap();
        let request = HostServiceRequest {
            id: "fixture.service".into(),
            config: Value::Null,
            asset_references: BTreeSet::new(),
        };
        let context = HostServiceContext {
            asset_root: Path::new("missing-root"),
            asset_paths: &BTreeMap::new(),
            retired: Arc::new(AtomicBool::new(false)),
        };
        assert!(
            registry
                .build(&[request.clone(), request], &context, true)
                .err()
                .unwrap()
                .contains("unique")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
