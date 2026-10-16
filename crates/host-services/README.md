# Optional host services

[English](README.md) | [Русский](README.ru.md) | [Documentation](../../docs/README.md)

The default crate builds only the generic factory registry and service owner.
It has no renderer, windowing, game-package, or platform playback dependency.
Enable `audio` and call `install_audio` in an application profile to install
the `audio.wav` factory.

Game contributions discover `HostServiceRequest` values from their authored
schedule. A host passes checked asset ID/path metadata, an asset root, and its
retirement atomic to `HostServiceRegistry::build`. Disabled construction creates
zero service instances and invokes no factory, asset resolution, file read, or
platform audio call. A request omitted by the source package also creates no
service. An enabled request for an unavailable factory fails explicitly.

`set_enabled(false)` mutes already constructed services while retaining their
resources and logical diagnostics; it does not undo their initial loading.
Re-enabling a host constructed with services disabled requires rebuilding its
services. Merely toggling the empty owner never performs hidden file IO.

WAV configuration contains `cues` (cue name to raw project-relative path or
`asset://` reference), `event`, and `cue_field`. Its event mapping is configured,
so the service does not know any game's event enum. Matching known cues increase
the logical `played` diagnostic even while muted. Restart/reset clears that
counter. Replacing service configuration constructs a fresh owner and restarts
its diagnostics; counters are not serialized game state.

Windows playback uses the existing single process-wide asynchronous WAV voice.
Only its current owner can stop it, and a retired actor cannot begin a later
play. The ownership check and short native call share one mutex. Retirement is
signaled separately by the host's atomic; dropping an unplayed candidate cannot
retire the accepted host or stop another owner's voice.

`WAV_PLAYBACK_SUPPORTED` is currently true only on Windows. Linux does not have
a native audio adapter in this version. An enabled `audio.wav` request fails
before asset IO with an explicit unsupported-platform message; it cannot report
successful playback through an empty implementation. An omitted request or a
host constructed with services disabled remains valid on Linux. Silent projects
(including Empty and Puzzle) need no audio service. To run a project which has
audio cues without Linux playback, explicitly disable services or use a profile
without those cues; loading a Windows audio dependency does not add Linux sound.

The `played` field remains a logical matched-cue counter, including muted cues;
`platform_playback` describes the adapter capability, not audible device output.
Native playback start errors propagate to the owning host and do not transfer
the process-global voice ownership to a failed candidate.
