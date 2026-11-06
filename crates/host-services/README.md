# Optional host services

[English](README.md) | [Русский](README.ru.md) | [Documentation](../../docs/README.md)

The default crate builds only the generic factory registry and service owner. It has no renderer, windowing, game-package or platform-playback dependency. Enable `audio` and call `install_audio` for `audio.wav`; enable `persistence` and call `install_persistence_at(&mut registry, state_root)` for `persistence.json`. Installing either factory performs no file or device access.

Game contributions discover `HostServiceRequest` values from their authored schedule. The host supplies checked asset ID/path metadata, the asset root and its retirement atomic. Omitted or disabled requests construct no instances and perform no factory calls, asset resolution, file reads, directory creation or device calls. An enabled request for a missing factory fails explicitly. Headless hosts keep services disabled by default.

`HostServices::initial_module_data()` merges source-owned bootstrap keys and rejects empty/duplicate keys. A new `GameSession` accepts these opaque values through `seed_module_data` before its first simulation operation, validating the entire candidate before publication. Exact keys, unknown payload fields and explicit null survive. This does not restore the ECS, clock, executor, Custom worker, input edges or service internals. The source package reads its own namespace during initialization and validates/reconstructs its resume and undo data. An authored-preview replacement seeds its fresh candidate before publication; a rejected candidate preserves the previous document/session. Restart starts a fresh session and does not automatically reload the save.

Service effects happen after committed game operations, including an accepted prefix when a later fixed tick in the same catchup call rejects. Rejected tick events are not delivered, and a no-commit error does not replay earlier events. A service failure remains visible as host activity and does not pretend that the committed tick was rolled back. Every service receives the committed events and reset independently; failures are aggregated so an audio error cannot suppress a save. Interpolated presentation invokes no service. `set_enabled(false)` suppresses external effects while retaining loaded resources and logical diagnostics; enabling an owner built with services disabled cannot create omitted services or perform hidden IO. Stop disables effects and releases only the owner's resources.

## Versioned JSON persistence

The application chooses a writable state root separately from authored content and packaged assets. A source-owned request configures:

```json
{
  "namespace": "example-game/player-profile",
  "data_key": "example.resume/1",
  "write_event": "example.save",
  "version": 1,
  "slot": "state",
  "payload_field": "data",
  "invalid_save": "reject",
  "reset_event": null
}
```

`slot` and `payload_field` default to `state` and `data`; `invalid_save` defaults to `reject`, and `reset_event` is absent by default. Namespace and slot identities become hex-encoded path segments below the application root, preserving case/Unicode identity without treating the identity as a path. Different games/profiles use different namespaces. The save envelope stores format version 1, the configured namespace/data key/payload version, and opaque JSON `data`. A missing file seeds no key; saved null seeds an explicit null. With the unchanged default policy, corrupt, oversized, unknown-format, mismatched identity or mismatched payload-version saves and load IO failures reject construction and remain unchanged. Payload migration is not automatic; a later source contract must authorize it.

An explicitly selected `invalid_save:"read_only"` policy keeps an enabled service usable after a load failure, with no bootstrap value and `write_blocked:true`. `last_error` is latched, and `error_kind` distinguishes `invalid_save` from `io` (including unreadable paths/permissions). Configuration errors still reject construction. This policy never silently repairs, replaces or discards a save; a game may continue without saving while its UI reports the blocked storage. The game still validates its typed payload: a structurally valid envelope is opaque to the service, even if its payload is invalid for the source-owned game schema.

Set an optional, distinct `reset_event`, for example `example.reset-storage`, to permit an explicit `GameEvent::new("example.reset-storage", json!({"confirm":true}))`. A confirmed enabled reset reserves a unique recovery sibling and atomically moves only the existing regular save file into it, preserving its bytes; it then clears the loaded value/error and unblocks writes. Recovery files are retained and reported by `recovery_file`. It never moves a directory or follows a symlink as a save. A reset followed by the normal fresh-save event in the same committed event batch is supported, including when the old envelope was valid but the source rejected its typed payload. If quarantine fails, the original stays in place, writes remain blocked and the reset error is latched. If a subsequent fresh save fails, the old recovery file remains intact. Missing/false `confirm`, disabled services, retired owners and unconfigured reset events cannot discard the published save. Application confirmation and the choice to reset belong to the source/UI; ordinary host restart is not this event.

The configured event, for example `GameEvent::new("example.save", json!({"data": payload}))`, requests a write. The source package owns the full payload, including any resume/history schema. Only matching enabled events perform persistence IO; idle frames do not. An identical published payload is a no-op. Reads and serialized envelopes are bounded to 8 MiB. A write flushes a uniquely created sibling candidate before atomically replacing the published entry. Serialization/write/flush/publication failure preserves the previous published save; owned temporary files are removed. The retired flag is rechecked before publication. A live host also revalidates an existing envelope before replacement so it cannot silently overwrite an externally corrupted or future-version save.

Ordinary host reset clears persistence counters without deleting the save, clearing a latched error or unblocking storage. `requests` counts matching writes, including disabled ones; `writes` counts actual replacements. `reset_requests` and `resets` track explicit reset requests and successful resets. `loaded` distinguishes a loaded/saved value from absence. `write_blocked`, `last_error`, `error_kind` and `recovery_file` are current UI diagnostics; idle consumption does not clear errors. A successful requested write (including an already-identical published value) or confirmed reset clears its current error. Write/publication failures remain retryable. With the opt-in policy, a newly invalid/unreadable existing envelope blocks replacement until reset; the default reject policy fails that operation and revalidates on later retries. Publication is serialized between services in this process; it is not a cross-process merge, compare-and-swap or locking protocol. The flushed candidate and atomic replacement protect ordinary publication failure, not a guarantee against every power-loss/filesystem failure. Disk work occurs on the host owner thread for explicit save/reset events; measure save latency when setting a game's frame-time criteria.

## Concurrent WAV music and effects

Gain multiplication, interpolation and voice accumulation use float64 before the final PCM16 clamp. This keeps the full admitted finite float32 range valid, including opposite-sign neighbours and the bounded 128-voice sum; admission is not narrowed to a nominal amplitude range.

The WAV event mapping remains configured, independent of any game event enum:

```json
{
  "event": "example.sound",
  "cue_field": "cue",
  "control_event": "example.audio-control",
  "master_volume": 1.0,
  "music_volume": 0.5,
  "sfx_volume": 0.8,
  "max_voices": 32,
  "cues": {
    "hit": "asset://hit-wav",
    "theme": {"path": "asset://theme-wav", "bus": "music", "volume": 0.6, "looping": true}
  }
}
```

Legacy string cue values remain valid: each selects a non-looping SFX at volume 1. Rich cues specify `path`, optional `bus` (`sfx` by default), `volume` (1 by default) and `looping` (false by default). References are project-relative paths or checked `asset://` IDs. Declare every cue reference in the service request's `asset_references` so export includes it without interpreting private service JSON.

Each owner has a software mixer and its own WinMM `waveOut` handle. SFX overlap. Starting music replaces only that owner's previous music voice; it preserves SFX and other owners. The bounded 2..=128 voice limit defaults to 32; a full mixer steals the oldest SFX. Per-cue, master and bus volumes multiply; each finite gain must be in 0..=1, and the final mixed samples are clamped. The configured control event accepts `{"command":"volume","bus":"master|music|sfx","volume":0.5}` or `{"command":"stop","bus":"music|sfx"}`; omitting the stop bus stops both. Bus and volume values are literals, not pipe-separated alternatives in actual JSON. Changes affect future mixed buffers; up to four 10 ms buffers (40 ms of generated audio) can already be queued.

The decoder accepts nonempty mono/stereo RIFF WAV at 8000..=192000 Hz: unsigned PCM8, signed little-endian PCM16/24/32 and finite IEEE float32. Standard `WAVEFORMATEXTENSIBLE` PCM/float GUIDs are accepted: PCM precision must be positive and no wider than its byte-aligned container, with valid bits left-aligned and unused low bits zero; float precision/container must both be 32 bits. Mono masks may be unspecified, front-left, front-right or front-center; stereo may be unspecified or front-left+front-right. Explicit mono left/right stays on that output channel; unspecified/center mono is duplicated to stereo. Other channel layouts fail explicitly. [Microsoft's extensible-format contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ksmedia/ns-ksmedia-waveformatextensible) defines the container, precision and placement fields.

The decoder validates sample rate, block alignment, byte rate, extension size and complete RIFF chunks, including odd-byte padding. Malformed, duplicate or truncated format/data chunks, unsupported compression/precision and nonfinite float samples fail explicitly. Finite float headroom is preserved until the final mixer clamp. Input is bounded to 128 MiB per WAV and 64 Mi interleaved float samples per decode and across retained cues; there are at most 256 cues. A linear resampler mixes into 48 kHz stereo PCM16. A `.wav` extension does not imply arbitrary codec support: compressed WAV, compressed streaming, spatial audio and crossfade are outside this implementation.

The native device opens lazily on the first enabled known cue. A muted service counts logical `played` matches without queuing voices or opening a device. Muting/reset/stop clears voices and stops only the owned output. Reset clears cue/steal counters and preserves bus-volume preferences. The worker checks retirement before submissions and retains output errors for host diagnostics. Endpoint-open or worker-spawn failures are separately latched in the same current output_error diagnostic; idle events, control changes, muting and retirement do not clear them. The next enabled known cue retries startup, and a successful start clears only that start error. Asynchronous worker faults take diagnostic priority and require explicit host reset; reset clears both error latches while reporting the previous error to the host. Reset returns submitted buffers before release; unconfirmed native cleanup failures retain their handle/buffer allocations rather than freeing device-owned memory.

`WAV_PLAYBACK_SUPPORTED` is true only on Windows. Linux has no native adapter: an enabled WAV request fails before asset IO, and an omitted/disabled request remains valid. `device_submitted_buffers` counts successful owned-device submissions; `device_completed_buffers` counts their normal WinMM completion flags before teardown reset. Logical counters and mixer sample assertions do not establish audible device output. The ignored Windows test `native_output_advances_source_audio_at_realtime_speed` compares the production mixer source cursor with monotonic wall time after warmup, using a silent 24 kHz source through the real 48 kHz output. It detects sustained feed slowdown; it does not certify device pitch or physical audibility. The ignored Windows test `native_two_owned_mixers_complete_buffers_and_retiring_one_preserves_the_other` checks real submissions/completions and ownership retirement with a bounded wait. `native_registered_service_decodes_legacy_pcm8_cue_and_completes_then_stops_owned_output` exercises the installed registry, an asset ID, a legacy string cue, PCM8 decoding, configured event delivery and owned stop through `audio.wav`. Run native checks sequentially with `--ignored --nocapture --test-threads=1`. Native output and listening require their own separately reported checks; buffer completion is not proof of physical speaker output.
