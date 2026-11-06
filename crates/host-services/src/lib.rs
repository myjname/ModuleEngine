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
mod audio_output;
#[cfg(feature = "audio")]
mod wav;
#[cfg(feature = "audio")]
pub use audio::{WAV_PLAYBACK_SUPPORTED, install_audio};
#[cfg(feature = "persistence")]
mod persistence;
#[cfg(feature = "persistence")]
pub use persistence::{InvalidSavePolicy, PersistenceConfig, install_persistence_at};

pub const WAV_SERVICE_ID: &str = "audio.wav";
pub const PERSISTENCE_SERVICE_ID: &str = "persistence.json";

pub struct HostServiceContext<'a> {
    pub asset_root: &'a Path,
    /// Checked portable asset IDs. A service resolves only references it uses.
    pub asset_paths: &'a BTreeMap<String, String>,
    /// A host/actor may retire all its services with one nonblocking store.
    pub retired: Arc<AtomicBool>,
}

pub trait HostService: Send {
    /// Opaque, source-owned data offered only when constructing a new session.
    /// Services must not mutate the authored project or restore executor state.
    fn initial_module_data(&self) -> BTreeMap<String, Value> {
        BTreeMap::new()
    }
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
    /// Reject conflicting service namespaces before publishing any bootstrap.
    pub fn initial_module_data(&self) -> Result<BTreeMap<String, Value>, String> {
        let mut result = BTreeMap::new();
        for (id, service) in &self.services {
            for (key, value) in service.initial_module_data() {
                if key.trim().is_empty() || result.insert(key.clone(), value).is_some() {
                    return Err(format!(
                        "Host service '{id}' has an empty or duplicate bootstrap key '{key}'"
                    ));
                }
            }
        }
        Ok(result)
    }
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
        let mut errors = vec![];
        for (id, service) in &mut self.services {
            if let Err(error) = service.consume(events) {
                errors.push(format!("Host service '{id}': {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
    pub fn reset(&mut self) -> Result<(), String> {
        let mut errors = vec![];
        for (id, service) in &mut self.services {
            if let Err(error) = service.reset() {
                errors.push(format!("Host service '{id}' reset: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
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

    struct SeedService;
    struct EffectService {
        failing: bool,
        calls: Arc<AtomicUsize>,
        resets: Arc<AtomicUsize>,
        saved: Arc<std::sync::Mutex<Vec<GameEvent>>>,
    }
    impl HostService for EffectService {
        fn consume(&mut self, events: &[GameEvent]) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.failing {
                Err("fixture consume failure".into())
            } else {
                self.saved.lock().unwrap().extend_from_slice(events);
                Ok(())
            }
        }
        fn reset(&mut self) -> Result<(), String> {
            self.resets.fetch_add(1, Ordering::SeqCst);
            if self.failing {
                Err("fixture reset failure".into())
            } else {
                Ok(())
            }
        }
        fn set_enabled(&mut self, _: bool) {}
        fn stop(&mut self) {}
        fn diagnostics(&self) -> Value {
            Value::Null
        }
    }
    #[test]
    fn failed_audio_cannot_starve_committed_persistence_events_or_other_service_resets() {
        let mut registry = HostServiceRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let resets = Arc::new(AtomicUsize::new(0));
        let saved = Arc::new(std::sync::Mutex::new(vec![]));
        let mut requests = vec![];
        for (id, failing) in [
            (WAV_SERVICE_ID, true),
            (PERSISTENCE_SERVICE_ID, false),
            ("z.failure", true),
        ] {
            let calls = calls.clone();
            let resets = resets.clone();
            let saved = saved.clone();
            registry
                .register(id, move |_, _| {
                    Ok(Box::new(EffectService {
                        failing,
                        calls: calls.clone(),
                        resets: resets.clone(),
                        saved: saved.clone(),
                    }))
                })
                .unwrap();
            requests.push(HostServiceRequest {
                id: id.into(),
                config: Value::Null,
                asset_references: Default::default(),
            });
        }
        let context = HostServiceContext {
            asset_root: Path::new("missing"),
            asset_paths: &BTreeMap::new(),
            retired: Arc::new(AtomicBool::new(false)),
        };
        let mut services = registry.build(&requests, &context, true).unwrap();
        let error = services
            .consume(&[GameEvent::new("fixture.save", json!({"data":7}))])
            .unwrap_err();
        assert!(error.contains("audio.wav") && error.contains("z.failure"));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            *saved.lock().unwrap(),
            vec![GameEvent::new("fixture.save", json!({"data":7}))]
        );
        let error = services.reset().unwrap_err();
        assert!(error.contains("audio.wav") && error.contains("z.failure"));
        assert_eq!(resets.load(Ordering::SeqCst), 3);
    }
    impl HostService for SeedService {
        fn initial_module_data(&self) -> BTreeMap<String, Value> {
            BTreeMap::from([("fixture.seed".into(), Value::Null)])
        }
        fn consume(&mut self, _: &[GameEvent]) -> Result<(), String> {
            Ok(())
        }
        fn reset(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn set_enabled(&mut self, _: bool) {}
        fn stop(&mut self) {}
        fn diagnostics(&self) -> Value {
            Value::Null
        }
    }
    #[test]
    fn duplicate_bootstrap_namespaces_are_rejected_and_disabled_owner_has_no_seed() {
        let mut registry = HostServiceRegistry::new();
        registry
            .register("a", |_, _| Ok(Box::new(SeedService)))
            .unwrap();
        registry
            .register("b", |_, _| Ok(Box::new(SeedService)))
            .unwrap();
        let request = |id: &str| HostServiceRequest {
            id: id.into(),
            config: Value::Null,
            asset_references: Default::default(),
        };
        let context = HostServiceContext {
            asset_root: Path::new("missing"),
            asset_paths: &BTreeMap::new(),
            retired: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(
            registry
                .build(&[request("a")], &context, true)
                .unwrap()
                .initial_module_data()
                .unwrap()["fixture.seed"],
            Value::Null
        );
        assert!(
            registry
                .build(&[request("a"), request("b")], &context, true)
                .unwrap()
                .initial_module_data()
                .unwrap_err()
                .contains("duplicate bootstrap key")
        );
        assert!(
            registry
                .build(&[request("a"), request("b")], &context, false)
                .unwrap()
                .initial_module_data()
                .unwrap()
                .is_empty()
        );
    }

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
