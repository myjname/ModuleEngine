use crate::{HostService, HostServiceContext, HostServiceRegistry, WAV_SERVICE_ID};
use module_engine_core::GameEvent;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WavConfig {
    cues: BTreeMap<String, String>,
    event: String,
    cue_field: String,
}

/// The current WAV adapter uses WinMM. Installing its factory does not imply
/// playback availability on every host platform; disabled services remain valid.
pub const WAV_PLAYBACK_SUPPORTED: bool = cfg!(windows);

fn ensure_native_playback() -> Result<(), String> {
    if WAV_PLAYBACK_SUPPORTED {
        Ok(())
    } else {
        Err("Native WAV playback is not implemented on this platform. Disable host services/audio cues or select an Engine profile without audio.wav; Linux GUI and silent/headless games remain supported.".into())
    }
}

/// Install explicitly in an application profile which permits native audio.
pub fn install_audio(registry: &mut HostServiceRegistry) -> Result<(), String> {
    registry.register(WAV_SERVICE_ID, |request, context| {
        let config: WavConfig = serde_json::from_value(request.config.clone())
            .map_err(|error| format!("Invalid WAV service configuration: {error}"))?;
        Ok(Box::new(WavService::new(config, context)?))
    })
}

struct WavService {
    cues: BTreeMap<String, Vec<u16>>,
    event: String,
    cue_field: String,
    endpoint_id: u64,
    retired: Arc<AtomicBool>,
    played: u64,
    enabled: bool,
}
impl WavService {
    fn new(config: WavConfig, context: &HostServiceContext<'_>) -> Result<Self, String> {
        // Fail before asset IO rather than constructing a successful silent
        // endpoint and later counting missing Linux playback as delivered sound.
        ensure_native_playback()?;
        if config.event.trim().is_empty() || config.cue_field.trim().is_empty() {
            return Err("WAV service event and cue field must be nonempty".into());
        }
        let mut cues = BTreeMap::new();
        for (cue, reference) in config.cues {
            let relative = if let Some(id) = reference.strip_prefix("asset://") {
                context
                    .asset_paths
                    .get(id)
                    .ok_or_else(|| format!("Audio cue '{cue}' references unknown asset '{id}'"))?
            } else {
                &reference
            };
            let file = module_engine_project::resolve_relative(context.asset_root, relative)?;
            let bytes =
                std::fs::read(&file).map_err(|error| format!("Audio cue '{cue}': {error}"))?;
            if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
                return Err(format!("Audio cue '{cue}' needs a WAV file"));
            }
            cues.insert(
                cue,
                file.to_string_lossy()
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect(),
            );
        }
        static NEXT_ENDPOINT: AtomicU64 = AtomicU64::new(1);
        let endpoint_id = NEXT_ENDPOINT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| "Audio owner ID space exhausted")?;
        Ok(Self {
            cues,
            event: config.event,
            cue_field: config.cue_field,
            endpoint_id,
            retired: context.retired.clone(),
            played: 0,
            enabled: true,
        })
    }
    fn stop_voice(&self) {
        if self.enabled {
            endpoint()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop(self.endpoint_id, native_stop);
        }
    }
}
impl HostService for WavService {
    fn consume(&mut self, events: &[GameEvent]) -> Result<(), String> {
        for event in events {
            if event.name == self.event
                && let Some(cue) = event
                    .payload
                    .get(self.cue_field.as_str())
                    .and_then(Value::as_str)
                && let Some(path) = self.cues.get(cue)
            {
                self.played = self.played.saturating_add(1);
                if self.enabled {
                    endpoint()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .play(self.endpoint_id, &self.retired, || native_play(path))?;
                }
            }
        }
        Ok(())
    }
    fn reset(&mut self) -> Result<(), String> {
        self.stop_voice();
        self.played = 0;
        Ok(())
    }
    fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.stop_voice();
        }
        self.enabled = enabled;
    }
    fn stop(&mut self) {
        self.stop_voice();
        self.enabled = false;
    }
    fn diagnostics(&self) -> Value {
        json!({"played":self.played,"enabled":self.enabled,"cues":self.cues.len(),"platform_playback":WAV_PLAYBACK_SUPPORTED})
    }
}
impl Drop for WavService {
    fn drop(&mut self) {
        self.stop();
    }
}

/// PlaySound is process-global. Ownership and the short async native call share
/// one lock, so a late retired actor cannot play over or stop its successor.
#[derive(Default)]
struct AudioEndpoint {
    owner: Option<u64>,
}
impl AudioEndpoint {
    fn play(
        &mut self,
        owner: u64,
        retired: &AtomicBool,
        play: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if retired.load(Ordering::Acquire) {
            return Ok(());
        }
        play()?;
        self.owner = Some(owner);
        Ok(())
    }
    fn stop(&mut self, owner: u64, stop: impl FnOnce()) {
        if self.owner == Some(owner) {
            stop();
            self.owner = None;
        }
    }
}
fn endpoint() -> &'static Mutex<AudioEndpoint> {
    static ENDPOINT: OnceLock<Mutex<AudioEndpoint>> = OnceLock::new();
    ENDPOINT.get_or_init(|| Mutex::new(AudioEndpoint::default()))
}
fn native_play(path: &[u16]) -> Result<(), String> {
    #[cfg(windows)]
    unsafe {
        if PlaySoundW(
            path.as_ptr(),
            std::ptr::null_mut(),
            0x00020000 | 0x0001 | 0x0002,
        ) == 0
        {
            return Err("WinMM could not start the configured WAV cue".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        ensure_native_playback()
    }
}
fn native_stop() {
    #[cfg(windows)]
    unsafe {
        PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0);
    }
}
#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::HostServiceRequest;
    #[cfg(windows)]
    use std::fs;
    use std::path::Path;

    #[test]
    fn retired_owner_cannot_play_over_or_stop_successor_and_stop_is_idempotent() {
        let mut endpoint = AudioEndpoint::default();
        let old_retired = AtomicBool::new(false);
        let new_retired = AtomicBool::new(false);
        let mut operations = vec![];
        endpoint
            .play(1, &old_retired, || {
                operations.push("old play");
                Ok(())
            })
            .unwrap();
        old_retired.store(true, Ordering::Release);
        endpoint
            .play(2, &new_retired, || {
                operations.push("new play");
                Ok(())
            })
            .unwrap();
        endpoint
            .play(1, &old_retired, || {
                operations.push("retired late play");
                Ok(())
            })
            .unwrap();
        endpoint.stop(1, || operations.push("old stop"));
        assert_eq!(endpoint.owner, Some(2));
        assert_eq!(operations, ["old play", "new play"]);
        endpoint.stop(2, || operations.push("new stop"));
        endpoint.stop(2, || operations.push("duplicate stop"));
        assert_eq!(endpoint.owner, None);
        assert_eq!(operations, ["old play", "new play", "new stop"]);
    }

    #[test]
    fn disabled_audio_does_not_resolve_assets_or_open_files() {
        let mut registry = HostServiceRegistry::new();
        install_audio(&mut registry).unwrap();
        let requests = [HostServiceRequest {
            id: WAV_SERVICE_ID.into(),
            config: json!({"cues":{"hit":"asset://missing"},"event":"Sound","cue_field":"cue"}),
            asset_references: Default::default(),
        }];
        let context = HostServiceContext {
            asset_root: Path::new("missing-root-must-not-be-opened"),
            asset_paths: &BTreeMap::new(),
            retired: Arc::new(AtomicBool::new(false)),
        };
        let services = registry.build(&requests, &context, false).unwrap();
        assert!(services.is_empty());
        assert_eq!(services.diagnostics(), json!({}));
        let error = registry.build(&requests, &context, true).err().unwrap();
        assert!(error.contains(if WAV_PLAYBACK_SUPPORTED {
            "unknown asset"
        } else {
            "not implemented on this platform"
        }));
    }

    #[cfg(windows)]
    #[test]
    fn configured_event_mapping_resolves_manifest_and_counts_muted_cues_then_resets() {
        let root = std::env::temp_dir().join(format!(
            "ModuleEngine service wav {} {}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut wav = vec![0; 44];
        wav[..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        fs::write(root.join("cue.wav"), &wav).unwrap();
        let paths = BTreeMap::from([("tone".into(), "cue.wav".into())]);
        let context = HostServiceContext {
            asset_root: &root,
            asset_paths: &paths,
            retired: Arc::new(AtomicBool::new(false)),
        };
        let mut registry = HostServiceRegistry::new();
        install_audio(&mut registry).unwrap();
        let requests = [HostServiceRequest {
            id: WAV_SERVICE_ID.into(),
            config: json!({"cues":{"hit":"asset://tone"},"event":"independent.cue","cue_field":"name"}),
            asset_references: Default::default(),
        }];
        let mut services = registry.build(&requests, &context, true).unwrap();
        // Never produce platform sound in unit tests.
        services.set_enabled(false);
        services
            .consume(&[
                GameEvent::new("Sound", json!({"cue":"hit"})),
                GameEvent::new("independent.cue", json!({"name":"unknown"})),
                GameEvent::new("independent.cue", json!({"name":"hit"})),
            ])
            .unwrap();
        assert_eq!(services.diagnostics()[WAV_SERVICE_ID]["played"], json!(1));
        assert_eq!(
            services.diagnostics()[WAV_SERVICE_ID]["enabled"],
            json!(false)
        );
        services.reset().unwrap();
        assert_eq!(services.diagnostics()[WAV_SERVICE_ID]["played"], json!(0));
        assert!(!services.enabled());
        drop(services);
        fs::write(root.join("cue.wav"), b"invalid").unwrap();
        assert!(
            registry
                .build(&requests, &context, true)
                .err()
                .unwrap()
                .contains("needs a WAV file")
        );
        let root = root.canonicalize().unwrap();
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        assert!(root.starts_with(&temporary) && root != temporary);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_native_audio_fails_before_asset_io_and_never_reports_playback() {
        let mut registry = HostServiceRegistry::new();
        install_audio(&mut registry).unwrap();
        let request = HostServiceRequest {
            id: WAV_SERVICE_ID.into(),
            config: json!({"cues":{"hit":"missing.wav"},"event":"cue","cue_field":"name"}),
            asset_references: Default::default(),
        };
        let context = HostServiceContext {
            asset_root: Path::new("missing-root-must-not-be-opened"),
            asset_paths: &BTreeMap::new(),
            retired: Arc::new(AtomicBool::new(false)),
        };
        assert!(registry.build(&[], &context, true).unwrap().is_empty());
        assert!(
            registry
                .build(std::slice::from_ref(&request), &context, false)
                .unwrap()
                .is_empty()
        );
        let error = registry.build(&[request], &context, true).err().unwrap();
        assert!(error.contains("Native WAV playback is not implemented"));
        assert!(error.contains("Disable host services/audio cues"));
        assert!(native_play(&[]).is_err());
    }

    #[test]
    fn failing_native_endpoint_does_not_take_ownership() {
        let mut endpoint = AudioEndpoint { owner: Some(1) };
        assert!(
            endpoint
                .play(2, &AtomicBool::new(false), || Err("device error".into()))
                .is_err()
        );
        assert_eq!(endpoint.owner, Some(1));
    }
}
