use crate::{HostService, HostServiceContext, HostServiceRegistry, WAV_SERVICE_ID};
use crate::{audio_output::AudioOutput, wav::DecodedWav};
use module_engine_core::GameEvent;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) const OUTPUT_RATE: u32 = 48_000;
const MAX_TOTAL_SAMPLES: usize = 64 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Bus {
    #[default]
    Sfx,
    Music,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum CueConfig {
    Path(String),
    Settings(CueSettings),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CueSettings {
    path: String,
    #[serde(default)]
    bus: Bus,
    #[serde(default = "one")]
    volume: f32,
    #[serde(default)]
    looping: bool,
}
fn one() -> f32 {
    1.
}
fn default_voices() -> usize {
    32
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WavConfig {
    cues: BTreeMap<String, CueConfig>,
    event: String,
    cue_field: String,
    #[serde(default)]
    control_event: Option<String>,
    #[serde(default = "one")]
    master_volume: f32,
    #[serde(default = "one")]
    music_volume: f32,
    #[serde(default = "one")]
    sfx_volume: f32,
    #[serde(default = "default_voices")]
    max_voices: usize,
}
#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
enum AudioControl {
    Volume {
        bus: VolumeBus,
        volume: f32,
    },
    Stop {
        #[serde(default)]
        bus: Option<Bus>,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum VolumeBus {
    Master,
    Music,
    Sfx,
}

/// Native playback remains explicit: there is no successful silent adapter.
pub const WAV_PLAYBACK_SUPPORTED: bool = cfg!(windows);
fn ensure_native_playback() -> Result<(), String> {
    if WAV_PLAYBACK_SUPPORTED {
        Ok(())
    } else {
        Err("Native WAV playback is not implemented on this platform. Disable host services/audio cues or select an Engine profile without audio.wav; Linux GUI and silent/headless games remain supported.".into())
    }
}
pub fn install_audio(registry: &mut HostServiceRegistry) -> Result<(), String> {
    registry.register(WAV_SERVICE_ID, |request, context| {
        let config: WavConfig = serde_json::from_value(request.config.clone())
            .map_err(|error| format!("Invalid WAV service configuration: {error}"))?;
        Ok(Box::new(WavService::new(config, context)?))
    })
}
#[derive(Clone)]
struct Cue {
    wave: Arc<DecodedWav>,
    bus: Bus,
    volume: f32,
    looping: bool,
}
struct Voice {
    cue: Cue,
    position: f64,
}
pub(crate) struct Mixer {
    voices: Vec<Voice>,
    master: f32,
    music: f32,
    sfx: f32,
    max_voices: usize,
    stolen: u64,
    pub(crate) submitted: u64,
    pub(crate) completed: u64,
    pub(crate) fault: Option<String>,
}
impl Mixer {
    fn queue(&mut self, cue: Cue) {
        if cue.bus == Bus::Music {
            self.voices.retain(|voice| voice.cue.bus != Bus::Music);
        }
        if self.voices.len() >= self.max_voices {
            // Bounded oldest-SFX stealing preserves the music during bursts.
            let oldest = self
                .voices
                .iter()
                .position(|voice| voice.cue.bus == Bus::Sfx)
                .unwrap_or(0);
            self.voices.remove(oldest);
            self.stolen = self.stolen.saturating_add(1);
        }
        self.voices.push(Voice { cue, position: 0. });
    }
    fn stop(&mut self, bus: Option<Bus>) {
        self.voices
            .retain(|voice| bus.is_some_and(|bus| voice.cue.bus != bus));
    }
    #[cfg_attr(
        all(not(windows), not(test)),
        expect(
            dead_code,
            reason = "Only the Windows endpoint calls mixing outside CPU tests"
        )
    )]
    pub(crate) fn mix(&mut self, output: &mut [i16]) -> bool {
        if self.voices.is_empty() {
            return false;
        }
        // Admitted IEEE float32 samples may use their entire finite range.
        // f64 keeps opposite-sign interpolation and the bounded 128-voice sum
        // finite until the one final PCM clamp.
        let mut mixed = vec![0_f64; output.len()];
        let master = self.master;
        let music = self.music;
        let sfx = self.sfx;
        self.voices.retain_mut(|voice| {
            let frames = voice.cue.wave.samples.len() / 2;
            let gain = f64::from(master)
                * f64::from(voice.cue.volume)
                * f64::from(if voice.cue.bus == Bus::Music {
                    music
                } else {
                    sfx
                });
            for frame in mixed.as_chunks_mut::<2>().0 {
                if voice.position >= frames as f64 {
                    if !voice.cue.looping {
                        break;
                    }
                    voice.position %= frames as f64;
                }
                let index = voice.position as usize;
                let next = if index + 1 < frames {
                    index + 1
                } else if voice.cue.looping {
                    0
                } else {
                    index
                };
                let fraction = voice.position - index as f64;
                for (channel, sample) in frame.iter_mut().enumerate() {
                    let a = f64::from(voice.cue.wave.samples[index * 2 + channel]);
                    let b = f64::from(voice.cue.wave.samples[next * 2 + channel]);
                    *sample += (a + (b - a) * fraction) * gain;
                }
                voice.position += voice.cue.wave.sample_rate as f64 / OUTPUT_RATE as f64;
            }
            voice.cue.looping || voice.position < frames as f64
        });
        for (sample, value) in output.iter_mut().zip(mixed) {
            *sample = (value.clamp(-1., 1.) * i16::MAX as f64).round() as i16;
        }
        true
    }
}
struct WavService {
    cues: BTreeMap<String, Cue>,
    event: String,
    cue_field: String,
    control_event: Option<String>,
    mixer: Arc<Mutex<Mixer>>,
    output: Option<AudioOutput>,
    start_error: Option<String>,
    retired: Arc<AtomicBool>,
    played: u64,
    enabled: bool,
}
fn volume(value: f32) -> Result<f32, String> {
    if value.is_finite() && (0. ..=1.).contains(&value) {
        Ok(value)
    } else {
        Err("Audio volume must be finite and between 0 and 1".into())
    }
}
impl WavService {
    fn new(config: WavConfig, context: &HostServiceContext<'_>) -> Result<Self, String> {
        ensure_native_playback()?;
        if config.event.trim().is_empty()
            || config.cue_field.trim().is_empty()
            || config
                .control_event
                .as_ref()
                .is_some_and(|event| event.trim().is_empty() || event == &config.event)
        {
            return Err(
                "WAV cue event/field must be nonempty and control event must be distinct".into(),
            );
        }
        if !(2..=128).contains(&config.max_voices) || config.cues.len() > 256 {
            return Err("WAV service needs 2..=128 voices and at most 256 cues".into());
        }
        let mut cues = BTreeMap::new();
        let mut total_samples = 0_usize;
        for (name, cue) in config.cues {
            if name.trim().is_empty() {
                return Err("WAV cue names must be nonempty".into());
            }
            let settings = match cue {
                CueConfig::Path(path) => CueSettings {
                    path,
                    bus: Bus::Sfx,
                    volume: 1.,
                    looping: false,
                },
                CueConfig::Settings(settings) => settings,
            };
            let gain = volume(settings.volume)?;
            let relative = if let Some(id) = settings.path.strip_prefix("asset://") {
                context
                    .asset_paths
                    .get(id)
                    .ok_or_else(|| format!("Audio cue '{name}' references unknown asset '{id}'"))?
            } else {
                &settings.path
            };
            let file = module_engine_project::resolve_relative(context.asset_root, relative)?;
            let wave =
                DecodedWav::read(&file).map_err(|error| format!("Audio cue '{name}': {error}"))?;
            total_samples = total_samples
                .checked_add(wave.samples.len())
                .ok_or("Audio sample count overflow")?;
            if total_samples > MAX_TOTAL_SAMPLES {
                return Err("Audio cues exceed the decoded sample budget".into());
            }
            cues.insert(
                name,
                Cue {
                    wave: Arc::new(wave),
                    bus: settings.bus,
                    volume: gain,
                    looping: settings.looping,
                },
            );
        }
        Ok(Self {
            cues,
            event: config.event,
            cue_field: config.cue_field,
            control_event: config.control_event,
            mixer: Arc::new(Mutex::new(Mixer {
                voices: vec![],
                master: volume(config.master_volume)?,
                music: volume(config.music_volume)?,
                sfx: volume(config.sfx_volume)?,
                max_voices: config.max_voices,
                stolen: 0,
                submitted: 0,
                completed: 0,
                fault: None,
            })),
            output: None,
            start_error: None,
            retired: context.retired.clone(),
            played: 0,
            enabled: true,
        })
    }
    fn stop_output(&mut self) {
        if let Some(output) = self.output.take() {
            output.stop();
        }
        self.mixer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop(None);
    }
}
impl WavService {
    // A single production adapter path also permits failure injection without
    // opening, disabling or reconfiguring an actual device in ordinary tests.
    fn consume_with_start(
        &mut self,
        events: &[GameEvent],
        mut start: impl FnMut(Arc<Mutex<Mixer>>, Arc<AtomicBool>) -> Result<AudioOutput, String>,
    ) -> Result<(), String> {
        if let Some(error) = self
            .mixer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fault
            .clone()
        {
            return Err(error);
        }
        for event in events {
            if self.control_event.as_ref() == Some(&event.name) {
                let command: AudioControl = serde_json::from_value(event.payload.clone())
                    .map_err(|error| format!("Invalid audio control: {error}"))?;
                let mut mixer = self
                    .mixer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match command {
                    AudioControl::Volume { bus, volume: gain } => {
                        let gain = volume(gain)?;
                        match bus {
                            VolumeBus::Master => mixer.master = gain,
                            VolumeBus::Music => mixer.music = gain,
                            VolumeBus::Sfx => mixer.sfx = gain,
                        }
                    }
                    AudioControl::Stop { bus } => mixer.stop(bus),
                }
            } else if event.name == self.event
                && let Some(name) = event
                    .payload
                    .get(self.cue_field.as_str())
                    .and_then(Value::as_str)
                && let Some(cue) = self.cues.get(name)
            {
                self.played = self.played.saturating_add(1);
                if self.enabled && !self.retired.load(Ordering::Acquire) {
                    if self.output.is_none() {
                        match start(self.mixer.clone(), self.retired.clone()) {
                            Ok(output) => {
                                self.output = Some(output);
                                // A worker may already have reported its own
                                // terminal fault; clear only the start latch.
                                self.start_error = None;
                            }
                            Err(error) => {
                                self.start_error = Some(error.clone());
                                return Err(error);
                            }
                        }
                    }
                    // The worker checks retirement before each device submission.
                    self.mixer
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .queue(cue.clone());
                    if let Some(output) = &self.output {
                        output.wake();
                    }
                }
            }
        }
        Ok(())
    }
}
impl HostService for WavService {
    fn consume(&mut self, events: &[GameEvent]) -> Result<(), String> {
        self.consume_with_start(events, AudioOutput::start)
    }
    fn reset(&mut self) -> Result<(), String> {
        self.stop_output();
        self.played = 0;
        let mut mixer = self
            .mixer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        mixer.stolen = 0;
        mixer.submitted = 0;
        mixer.completed = 0;
        mixer
            .fault
            .take()
            .or(self.start_error.take())
            .map_or(Ok(()), Err)
    }
    fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.stop_output();
        }
        self.enabled = enabled;
    }
    fn stop(&mut self) {
        self.stop_output();
        self.enabled = false;
    }
    fn diagnostics(&self) -> Value {
        let mixer = self
            .mixer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        json!({"played":self.played,"enabled":self.enabled,"cues":self.cues.len(),"platform_playback":WAV_PLAYBACK_SUPPORTED,
            "active_voices":mixer.voices.len(),"stolen_voices":mixer.stolen,"master_volume":mixer.master,
            "music_volume":mixer.music,"sfx_volume":mixer.sfx,"output_error":mixer.fault.as_ref().or(self.start_error.as_ref()),
            "device_submitted_buffers":mixer.submitted,"device_completed_buffers":mixer.completed})
    }
}
impl Drop for WavService {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::HostServiceRequest;
    use std::path::Path;
    fn mixer() -> Mixer {
        Mixer {
            voices: vec![],
            master: 1.,
            music: 1.,
            sfx: 1.,
            max_voices: 2,
            stolen: 0,
            submitted: 0,
            completed: 0,
            fault: None,
        }
    }
    fn cue(bus: Bus, value: f32, frames: usize, looping: bool) -> Cue {
        Cue {
            wave: Arc::new(DecodedWav {
                samples: vec![value; frames * 2],
                sample_rate: OUTPUT_RATE,
            }),
            bus,
            volume: 1.,
            looping,
        }
    }
    fn adapter_fixture() -> WavService {
        WavService {
            cues: BTreeMap::from([("tone".into(), cue(Bus::Sfx, 0.1, 10, false))]),
            event: "fixture.sound".into(),
            cue_field: "cue".into(),
            control_event: Some("fixture.audio-control".into()),
            mixer: Arc::new(Mutex::new(mixer())),
            output: None,
            start_error: None,
            retired: Arc::new(AtomicBool::new(false)),
            played: 0,
            enabled: true,
        }
    }
    #[test]
    fn service_start_failure_is_visible_until_reset_and_idle_control_mute_do_not_clear_it() {
        let mut service = adapter_fixture();
        let sound = GameEvent::new("fixture.sound", json!({"cue":"tone"}));
        let error = service
            .consume_with_start(std::slice::from_ref(&sound), |_, _| {
                Err("Injected endpoint open failure".into())
            })
            .unwrap_err();
        assert_eq!(error, "Injected endpoint open failure");
        assert_eq!(service.diagnostics()["output_error"], json!(error));
        assert!(service.output.is_none());
        assert!(service.mixer.lock().unwrap().fault.is_none());
        service.consume(&[]).unwrap();
        service
            .consume_with_start(
                &[GameEvent::new(
                    "fixture.audio-control",
                    json!({"command":"volume","bus":"master","volume":0.5}),
                )],
                |_, _| panic!("Control must not open an endpoint"),
            )
            .unwrap();
        assert_eq!(service.diagnostics()["output_error"], json!(error));
        assert_eq!(service.diagnostics()["master_volume"], json!(0.5));
        service.set_enabled(false);
        service
            .consume_with_start(std::slice::from_ref(&sound), |_, _| {
                panic!("Muted audio must not open an endpoint")
            })
            .unwrap();
        service.set_enabled(true);
        service.retired.store(true, Ordering::Release);
        service
            .consume_with_start(&[sound], |_, _| {
                panic!("Retired audio must not open an endpoint")
            })
            .unwrap();
        assert_eq!(service.diagnostics()["output_error"], json!(error));
        assert_eq!(service.diagnostics()["active_voices"], json!(0));
        assert_eq!(service.reset().unwrap_err(), error);
        assert!(service.diagnostics()["output_error"].is_null());
        assert_eq!(service.diagnostics()["master_volume"], json!(0.5));
    }
    #[test]
    fn asynchronous_fault_remains_terminal_and_has_priority_over_a_previous_start_failure() {
        let mut service = adapter_fixture();
        service.start_error = Some("Previous start failure".into());
        service.mixer.lock().unwrap().fault = Some("Injected asynchronous output failure".into());
        let error = service
            .consume_with_start(
                &[GameEvent::new("fixture.sound", json!({"cue":"tone"}))],
                |_, _| panic!("An asynchronous fault needs explicit reset"),
            )
            .unwrap_err();
        assert_eq!(error, "Injected asynchronous output failure");
        assert_eq!(service.diagnostics()["output_error"], json!(error));
        assert_eq!(service.reset().unwrap_err(), error);
        assert!(service.start_error.is_none() && service.mixer.lock().unwrap().fault.is_none());
    }
    #[test]
    fn music_and_effects_mix_with_independent_volumes_loop_and_bounded_stealing() {
        let mut mixer = mixer();
        mixer.music = 0.5;
        mixer.sfx = 0.25;
        let mut music = cue(Bus::Music, 0.4, 1, true);
        music.volume = 0.5;
        mixer.queue(music);
        mixer.queue(cue(Bus::Sfx, 0.4, 2, false));
        let mut output = [0; 4];
        assert!(mixer.mix(&mut output));
        assert!(output.iter().all(|sample| (*sample - 6553).abs() <= 1));
        assert_eq!(mixer.voices.len(), 1);
        mixer.queue(cue(Bus::Sfx, 0.1, 10, false));
        mixer.queue(cue(Bus::Sfx, 0.2, 10, false));
        assert_eq!(mixer.voices.len(), 2);
        assert_eq!(mixer.stolen, 1);
        assert!(mixer.voices.iter().any(|voice| voice.cue.bus == Bus::Music));
        mixer.queue(cue(Bus::Music, 0.8, 1, true));
        assert_eq!(mixer.voices.len(), 2);
        mixer.stop(Some(Bus::Sfx));
        assert_eq!(mixer.voices.len(), 1);
        mixer.stop(None);
        assert!(!mixer.mix(&mut output));
    }
    #[test]
    fn mixer_clamps_resamples_and_keeps_owner_state_independent() {
        let mut a = mixer();
        let mut b = mixer();
        a.queue(cue(Bus::Sfx, 0.8, 4, false));
        a.queue(cue(Bus::Sfx, 0.8, 4, false));
        let mut output = [0; 2];
        a.mix(&mut output);
        assert_eq!(output, [i16::MAX; 2]);
        assert!(!b.mix(&mut output));
        a.stop(None);
        let slow = Cue {
            wave: Arc::new(DecodedWav {
                samples: vec![0., 0., 1., 1.],
                sample_rate: 24_000,
            }),
            bus: Bus::Sfx,
            volume: 1.,
            looping: false,
        };
        a.queue(slow);
        let mut output = [0; 6];
        a.mix(&mut output);
        assert_eq!(output[0], 0);
        assert!((output[2] - 16384).abs() <= 1);
        assert_eq!(output[4], i16::MAX);
        assert!(volume(f32::NAN).is_err());
        assert!(volume(1.01).is_err());
    }
    #[test]
    fn finite_float_extremes_interpolate_and_accumulate_128_voices_before_final_clamp() {
        let maximum = f32::from_bits(0x7f7f_ffff);
        let mut subject = mixer();
        subject.queue(Cue {
            wave: Arc::new(DecodedWav {
                samples: vec![maximum, maximum, -maximum, -maximum],
                sample_rate: 24_000,
            }),
            bus: Bus::Sfx,
            volume: 1.,
            looping: false,
        });
        let mut interpolated = [42_i16; 8];
        assert!(subject.mix(&mut interpolated));
        assert_eq!(
            interpolated,
            [
                i16::MAX,
                i16::MAX,
                0,
                0,
                -i16::MAX,
                -i16::MAX,
                -i16::MAX,
                -i16::MAX
            ]
        );
        subject.stop(None);
        subject.max_voices = 128;
        for _ in 0..128 {
            subject.queue(cue(Bus::Sfx, maximum, 2, false));
        }
        let mut summed = [42_i16; 2];
        assert!(subject.mix(&mut summed));
        assert_eq!(summed, [i16::MAX; 2]);
        subject.stop(None);
        for _ in 0..64 {
            subject.queue(cue(Bus::Sfx, maximum, 2, false));
        }
        for _ in 0..64 {
            subject.queue(cue(Bus::Sfx, -maximum, 2, false));
        }
        assert!(subject.mix(&mut summed));
        assert_eq!(summed, [0; 2]);
        assert_eq!(subject.stolen, 0);
    }
    #[test]
    fn legacy_cue_configuration_remains_valid_and_controls_are_strict() {
        let config: WavConfig = serde_json::from_value(json!({"cues":{"hit":"asset://tone","music":{"path":"music.wav","bus":"music","volume":0.4,"looping":true}},"event":"Sound","cue_field":"cue"})).unwrap();
        assert_eq!(config.max_voices, 32);
        assert!(
            serde_json::from_value::<AudioControl>(
                json!({"command":"volume","bus":"music","volume":0.5})
            )
            .is_ok()
        );
        assert!(
            serde_json::from_value::<AudioControl>(json!({"command":"stop","unknown":1})).is_err()
        );
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
        assert!(
            registry
                .build(&requests, &context, false)
                .unwrap()
                .is_empty()
        );
        let error = registry.build(&requests, &context, true).err().unwrap();
        assert!(error.contains(if WAV_PLAYBACK_SUPPORTED {
            "unknown asset"
        } else {
            "not implemented on this platform"
        }));
    }
    #[cfg(windows)]
    #[test]
    fn configured_event_mapping_counts_muted_cues_without_opening_a_device_then_resets() {
        let root = std::env::temp_dir().join(format!(
            "ModuleEngine-audio-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("cue.wav"),
            crate::wav::tests::pcm16(&[100, 200], 1, OUTPUT_RATE),
        )
        .unwrap();
        let context = HostServiceContext {
            asset_root: &root,
            asset_paths: &BTreeMap::from([("tone".into(), "cue.wav".into())]),
            retired: Arc::new(AtomicBool::new(false)),
        };
        let mut registry = HostServiceRegistry::new();
        install_audio(&mut registry).unwrap();
        let request = HostServiceRequest {
            id: WAV_SERVICE_ID.into(),
            config: json!({"cues":{"hit":"asset://tone"},"event":"independent.cue","cue_field":"name"}),
            asset_references: Default::default(),
        };
        let mut services = registry.build(&[request], &context, true).unwrap();
        services.set_enabled(false);
        services
            .consume(&[
                GameEvent::new("other", json!({"name":"hit"})),
                GameEvent::new("independent.cue", json!({"name":"unknown"})),
                GameEvent::new("independent.cue", json!({"name":"hit"})),
            ])
            .unwrap();
        assert_eq!(services.diagnostics()[WAV_SERVICE_ID]["played"], json!(1));
        assert_eq!(
            services.diagnostics()[WAV_SERVICE_ID]["active_voices"],
            json!(0)
        );
        services.reset().unwrap();
        assert_eq!(services.diagnostics()[WAV_SERVICE_ID]["played"], json!(0));
        drop(services);
        let absolute = root.canonicalize().unwrap();
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        assert!(absolute.starts_with(&temporary) && absolute != temporary);
        std::fs::remove_dir_all(absolute).unwrap();
    }
    #[cfg(not(windows))]
    #[test]
    fn unsupported_audio_fails_before_asset_io_without_successful_silent_output() {
        assert!(
            ensure_native_playback()
                .unwrap_err()
                .contains("Native WAV playback is not implemented")
        );
        assert!(
            AudioOutput::start(
                Arc::new(Mutex::new(mixer())),
                Arc::new(AtomicBool::new(false))
            )
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires a real Windows audio endpoint and an uncontended sustained playback clock check"]
    fn native_output_advances_source_audio_at_realtime_speed() {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        let sample_rate = 24_000;
        let shared = Arc::new(Mutex::new(mixer()));
        shared.lock().unwrap().queue(Cue {
            wave: Arc::new(DecodedWav {
                samples: vec![0.; sample_rate as usize * 12 * 2],
                sample_rate,
            }),
            bus: Bus::Music,
            volume: 1.,
            looping: false,
        });
        let output = AudioOutput::start(shared.clone(), Arc::new(AtomicBool::new(false)))
            .expect("Owned native audio clock probe");
        thread::sleep(Duration::from_millis(500));
        let begin = Instant::now();
        let start = shared.lock().unwrap().voices[0].position;
        let mut observations = Vec::new();
        for _ in 0..10 {
            thread::sleep(Duration::from_millis(500));
            let mixer = shared.lock().unwrap();
            assert!(mixer.fault.is_none(), "{:?}", mixer.fault);
            observations.push(json!({"wall_seconds":begin.elapsed().as_secs_f64(),"source_seconds":(mixer.voices[0].position-start)/f64::from(sample_rate),"submitted":mixer.submitted,"completed":mixer.completed}));
        }
        output.stop();
        let last = observations.last().unwrap();
        let wall = last["wall_seconds"].as_f64().unwrap();
        let source = last["source_seconds"].as_f64().unwrap();
        println!(
            "{}",
            json!({"case":"native_audio_source_clock","source_rate":sample_rate,"output_rate":OUTPUT_RATE,"silent_probe":true,"observations":observations,"source_to_wall_ratio":source/wall,"physical_audibility_verified":false})
        );
        assert!(
            (source - wall).abs() <= 0.10,
            "Audio source advanced {source:.3}s during {wall:.3}s wall time"
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires a real Windows audio endpoint; run without competing native/GPU checks"]
    fn native_registered_service_decodes_legacy_pcm8_cue_and_completes_then_stops_owned_output() {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        let fixture_root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../work/host-service-tests");
        let root = fixture_root.join(format!(
            "native-registered-audio-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let samples = (0..22_050)
            .map(|index| {
                let phase = std::f32::consts::TAU * 440. * index as f32 / 22_050.;
                (128_i16 + (phase.sin() * 3.).round() as i16) as u8
            })
            .collect::<Vec<_>>();
        std::fs::write(
            root.join("legacy-pcm8.wav"),
            crate::wav::tests::pcm8(&samples, 1, 22_050),
        )
        .unwrap();
        let paths = BTreeMap::from([("legacy-tone".into(), "legacy-pcm8.wav".into())]);
        let context = HostServiceContext {
            asset_root: &root,
            asset_paths: &paths,
            retired: Arc::new(AtomicBool::new(false)),
        };
        let mut registry = HostServiceRegistry::new();
        install_audio(&mut registry).unwrap();
        let request = HostServiceRequest {
            id: WAV_SERVICE_ID.into(),
            config: json!({"cues":{"tone":"asset://legacy-tone"},"event":"registered.sound","cue_field":"cue"}),
            asset_references: ["asset://legacy-tone".into()].into_iter().collect(),
        };
        let mut services = registry.build(&[request], &context, true).unwrap();
        assert_eq!(
            services.diagnostics()[WAV_SERVICE_ID]["device_submitted_buffers"],
            json!(0)
        );
        services
            .consume(&[GameEvent::new("registered.sound", json!({"cue":"tone"}))])
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            services.consume(&[]).unwrap();
            if services.diagnostics()[WAV_SERVICE_ID]["device_completed_buffers"]
                .as_u64()
                .unwrap()
                >= 3
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Registered audio.wav service did not complete owned device buffers"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let before_stop = services.diagnostics()[WAV_SERVICE_ID].clone();
        assert_eq!(before_stop["played"], json!(1));
        services.stop();
        let stopped = services.diagnostics()[WAV_SERVICE_ID].clone();
        assert_eq!(stopped["enabled"], json!(false));
        assert_eq!(stopped["active_voices"], json!(0));
        assert!(stopped["output_error"].is_null());
        thread::sleep(Duration::from_millis(20));
        assert_eq!(
            services.diagnostics()[WAV_SERVICE_ID]["device_submitted_buffers"],
            stopped["device_submitted_buffers"]
        );
        println!(
            "{}",
            json!({"case":"registered_service_pcm8","before_stop":before_stop,"after_stop":stopped,"owned_stop_stable":true,"physical_audibility_verified":false})
        );
        drop(services);
        let absolute = root.canonicalize().unwrap();
        let fixtures = fixture_root.canonicalize().unwrap();
        assert!(absolute.starts_with(&fixtures) && absolute != fixtures);
        std::fs::remove_dir_all(absolute).unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires a real Windows audio endpoint; run without competing native/GPU checks"]
    fn native_service_retries_a_start_error_and_clears_only_after_real_endpoint_success() {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        let mut service = adapter_fixture();
        service.cues.get_mut("tone").unwrap().wave = Arc::new(DecodedWav {
            samples: (0..OUTPUT_RATE / 5)
                .flat_map(|index| {
                    let value = (std::f32::consts::TAU * 440. * index as f32 / OUTPUT_RATE as f32)
                        .sin()
                        * 0.025;
                    [value, value]
                })
                .collect(),
            sample_rate: OUTPUT_RATE,
        });
        let sound = GameEvent::new("fixture.sound", json!({"cue":"tone"}));
        service
            .consume_with_start(std::slice::from_ref(&sound), |_, _| {
                Err("Injected pre-device start failure".into())
            })
            .unwrap_err();
        assert_eq!(
            service.diagnostics()["output_error"],
            json!("Injected pre-device start failure")
        );
        service
            .consume(&[sound])
            .expect("Retry must start a real owned endpoint");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            service.consume(&[]).unwrap();
            if service.diagnostics()["device_completed_buffers"]
                .as_u64()
                .unwrap()
                >= 3
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Retried service did not complete real device buffers"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(service.start_error.is_none() && service.diagnostics()["output_error"].is_null());
        service.stop();
        println!(
            "{}",
            json!({"case":"service_start_error_native_retry","diagnostics":service.diagnostics(),"physical_audibility_verified":false})
        );
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "Requires a real Windows audio endpoint; run without competing native/GPU checks"]
    fn native_two_owned_mixers_complete_buffers_and_retiring_one_preserves_the_other() {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        fn tone(bus: Bus, frequency: f32, looping: bool) -> Cue {
            let samples = (0..OUTPUT_RATE / 5)
                .flat_map(|index| {
                    let value = (std::f32::consts::TAU * frequency * index as f32
                        / OUTPUT_RATE as f32)
                        .sin()
                        * 0.025;
                    [value, value]
                })
                .collect();
            Cue {
                wave: Arc::new(DecodedWav {
                    samples,
                    sample_rate: OUTPUT_RATE,
                }),
                bus,
                volume: 1.,
                looping,
            }
        }
        fn wait(shared: &Arc<Mutex<Mixer>>, minimum: u64) {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                {
                    let mixer = shared.lock().unwrap();
                    assert!(mixer.fault.is_none(), "{:?}", mixer.fault);
                    if mixer.completed >= minimum {
                        return;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "Native mixer did not complete {minimum} submitted buffers"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        let a = Arc::new(Mutex::new(mixer()));
        let b = Arc::new(Mutex::new(mixer()));
        {
            let mut mixer = a.lock().unwrap();
            mixer.max_voices = 4;
            mixer.queue(tone(Bus::Music, 220., true));
            for _ in 0..3 {
                mixer.queue(tone(Bus::Sfx, 880., false));
            }
        }
        b.lock().unwrap().queue(tone(Bus::Music, 330., true));
        let retired_a = Arc::new(AtomicBool::new(false));
        let retired_b = Arc::new(AtomicBool::new(false));
        let output_a = AudioOutput::start(a.clone(), retired_a.clone())
            .expect("First owned Windows audio endpoint");
        let output_b = AudioOutput::start(b.clone(), retired_b)
            .expect("Second independent Windows audio endpoint");
        wait(&a, 3);
        wait(&b, 3);
        retired_a.store(true, Ordering::Release);
        output_a.stop();
        let a_submitted = a.lock().unwrap().submitted;
        let b_completed = b.lock().unwrap().completed;
        a.lock().unwrap().queue(tone(Bus::Sfx, 990., false));
        wait(&b, b_completed + 3);
        assert_eq!(a.lock().unwrap().submitted, a_submitted);
        output_b.stop();
        let a = a.lock().unwrap();
        let b = b.lock().unwrap();
        assert!(
            a.fault.is_none() && b.fault.is_none(),
            "{:?} / {:?}",
            a.fault,
            b.fault
        );
        println!(
            "{}",
            json!({"owner_a_submitted":a.submitted,"owner_a_completed":a.completed,"owner_b_submitted":b.submitted,"owner_b_completed":b.completed,"retired_owner_stable":true,"other_owner_continued":true,"physical_audibility_verified":false})
        );
    }
}
