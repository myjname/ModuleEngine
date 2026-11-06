# Standalone Player

[English](README.md) | [Русский](README.ru.md) | [Documentation](../../docs/README.md)

This package contains the shared native Player UI and a separate executable. It
has no production dependency on the Editor or the Two Arenas project.

The default build enables no optional implementation. Select native packages
explicitly: `ui`, `arpg`, `puzzle`, `g1`, `vulkan`, `audio`, and `persistence`. A puzzle window uses
`ui,puzzle,vulkan`; it does not require ARPG or audio. `ui` enables UI callbacks
only for gameplay packages already selected. Installation includes neutral core
contracts, the host-bound Custom contract, and the small generic source rotation
example; installed metadata alone never activates an authored module.
`g1` selects the independent [Crate Garden](../project-g1/README.md) source
package; its finished-game UI is enabled when `ui` is also selected. Requested
audio and persistence still require their own native features.

Applications share `composition_at`, `composition_with`, `service_registry`,
and the UI's `run_host`/`run_benchmark_host`. The Editor can add source contracts
through the composition callback before metadata is synchronized. The Player's
ordinary dispatch paths select source UI callbacks by contribution ID.

Construct a GUI-bound host through `ui::prepare_host`, then pass the returned
host to `run_host` or `run_benchmark_host`. On Windows, preparation runs on a
joined scoped thread: MSVC discovery may initialize COM as MTA, while winit's
window thread requires STA/OLE. Preparation errors are returned unchanged;
an unwinding preparation panic becomes a startup error after owned resources
are dropped. The window keeps drag-and-drop support. On Linux the factory runs
directly. Editor play/benchmark and standalone play/benchmark use this boundary;
headless package checks do not create a window.
Ordinary interactive Player uses the accepted authored document's game name as its
window title. Status-aware source UI receives current service/error diagnostics
and can request a capability-checked application close; the same UI embedded in
Editor receives no permission to close Editor. See the [UI contract](../../docs/game-ui-tools.md).

An exported package contains `player.json` schema 2, its authored format-4
project, the selected native executable, required assets, precompiled Custom
workers, and sealed Vulkan artifacts when used. The manifest records exact
native features, the SDK data/API fingerprint, and SHA-256 file hashes. Launch
requires an exact compiled feature match and the project's exact active
module/schema/state/SDK lock. Normal reads perform no migration and never fill
missing pins. The Editor's metadata validator can inspect a smaller installation
without pretending that its all-feature binary is that selected Player.

## Native GPU presentation

The normal native Vulkan viewport publishes an opaque GPU frame lease instead of CPU pixels. Windows bridges the separate Vulkan producer to WGPU/DX12 through shared D3D12 resources; Linux bridges separate Vulkan devices through dedicated OPAQUE_FD memory and requires exact device and driver UUIDs. The GUI bridge copies and premultiplies RGBA8 on the GPU into its own native `Rgba8Unorm` texture, preserving the byte rounding of the previous egui conversion without sRGB decoding. Normal presentation performs no pixel readback to CPU and no CPU texture upload. Unsupported or failed interop reports an error and preserves the last good image; it does not silently select CPU transfer.

The native producer admits at most one GPU submission at a time, with four leased output slots. `render_presented` returns `RenderSubmission::Pending(ticket)` after admission; `poll_presented(ticket)` checks completion without waiting and publishes that exact request once. `Busy` admits no work. `Ready(Frame)` supports immediate CPU/software output. Until completion, the previous image, identity, camera and provenance remain current; pending work is not a fresh frame. Scene uploads, atlas images, command resources and pipelines remain owned until their GPU use completes. The GUI separately retains each published lease until actual consumer GPU completion. Dropping a CPU reference or skipping a minimized surface does not complete either side. Explicit CPU/software rendering, diagnostic Glow and PNG/capture retain CPU pixels and may synchronously wait for readback; a conflicting CPU render is rejected while native producer work is pending. There is no game-renderer swapchain or multi-submission producer pipeline.

Cancellation discards publication without waiting or freeing busy GPU resources. Reaping cancelled work may be deferred until the next submission or backend destruction; absence of publication does not permit early reuse. Dropping a renderer transfers unfinished producer work to an owned retirement job; only that background job may wait up to 30 seconds for its own fence. Application shutdown joins these jobs after renderer owners have stopped. `finish_gpu_retirements` reports consumer retirement plus nested `producer` registered/joined/completed/unconfirmed/failed counts. A timeout, device error, panic or failed retirement-thread start preserves unconfirmed resources and their device pin instead of reusing potentially busy memory; it is reported as an error, not successful completion. Linux consumer ownership-release failure retains its existing quarantine behavior. Historical successful or synthetic failure checks do not certify new async sources or real hardware loss.

New async sources require separately bound checks and an independent conclusion in the [async report](../../docs/gpu-async-20261018.md); passing individual current gates does not establish no regression or final acceptance. The [historical GPU report](../../docs/gpu-presentation-20261018.md) retains Windows source15/Linux source11 results and earlier failures, including the cold gate open at that time. The [18 October continuation](../../docs/p0-continuation-20261018.md) separately records the later normal cold PASS 40.906/39.9128549 s, Windows22/16 and WSL Linux23/16 and scoped native/package/lifecycle checks for its own sources. These snapshots do not verify the new async implementation or certify hardware Linux, real device loss or physical scanout.

```powershell
Player.exe
Player.exe --state-root <writable-profile-folder>
Player.exe --verify-package <package-root> <evidence-folder>
Player.exe --headless-check <package-root> <evidence-folder>
Player.exe --benchmark <evidence-folder> [--state-root <writable-profile-folder>]
```

Interactive play and native benchmark accept the optional `--state-root` once
with an explicit writable profile location. Without it, requested persistence
uses the application's per-user data location; there is no fallback to writing
beside the package or into its assets. This selects external state, not another
package root, compiler root or authored document. Package verification and
headless checks reject `--state-root` and do not use it to alter verification.

Automatic lookup tries `LOCALAPPDATA`, `XDG_DATA_HOME`, then `HOME`; empty or
nonabsolute values are skipped. The first two use `ModuleEngine/profiles`,
while `HOME` uses `.local/share/ModuleEngine/profiles`. If no absolute candidate
is configured, persistence reports a configuration error instead of using the current
directory. This does not change explicitly chosen `--state-root` paths or
guarantee write permission; actual storage errors remain visible.

Normal launch and native benchmark locate `player.json` only beside the
executable. Verification accepts an explicit package root, advances three real
fixed ticks, and reports actual state and rendering statistics; the rendered
check also saves a 960×540 PNG. Headless verification reports `frame:null` and
constructs no renderer or optional service. Package execution uses no source
checkout locator, compiler root, or Editor distribution policy. Vulkan requires
the package's `.spirv` directory, so a missing artifact fails instead of falling
back to source compilation.

The existing native benchmark retains its 30-frame warmup, by default 120 UI samples and
120 distinct completed game frames, physical 1920×1080 requirement, 45-second
watchdog, and unmodified twenty-enemy workload criterion. The measured data and
any unmet target are reported directly; a different project does not satisfy
that workload criterion merely by rendering quickly.

For a longer real-input diagnostic, set `MODULE_ENGINE_BENCHMARK_FRAMES` to an
integer from 120 to 2400. Both UI and distinct-frame counts use that value;
warmup and the watchdog remain unchanged. Reports include per-sample game
status/input flags and final source-owned diagnostics. A different game needs
its own workload assessment from those records; the historical ARPG
`target_met` field is not a G1 acceptance gate.

For a game that opens into a menu, set `MODULE_ENGINE_BENCHMARK_ARM=manual`.
The ordinary game and controls remain active with a visible F9 waiting notice;
no benchmark warmup, samples or completion counters advance before a fresh F9
press. F9 arms once: collected callback/frame/timestamp/provenance history is
cleared, the measurement clock starts, and the unchanged 30-frame warmup begins.
A later F9 cannot reset a running measurement. Frames completed before arming
cannot enter its measured series. No game actions, state changes or workload
filters are injected by arming. The operator must establish the intended game
and workload with normal controls before pressing F9.

Unarmed waiting has its own 120-second deadline and produces an explicit
incomplete report. After arming, the existing 45-second measurement watchdog
applies. Reports include `arming_mode`, `armed`, `arm_wait_ms`,
`manual_arm_deadline_seconds` and `measurement_watchdog_seconds`. Omission of
the variable, or `automatic`, retains automatic historical counting; any other
value is rejected. Actual renderer/bridge counters remain cumulative owner
counters reported at the first/last measured observations; arming resets only
benchmark collections, not gameplay, services or GPU state.

This opt-in variable is read only by `--benchmark`; ordinary interactive play
has no benchmark F9 binding or waiting notice. `arm_wait_ms` begins at benchmark
window initialization and is not a process startup measurement. Keep an external
process-start timer for cold-start evidence. Raw `render_success_samples`,
`error_samples` and `window_visible_samples` preserve each measured callback's
flags independently of the historical twenty-enemy workload decision.

Native game poses may come from an active source-owned Core pose projection,
before Host interpolation and content expansion. They are genuine model changes,
separate from late color/material decoration. Their presence does not certify
frame rate: unchanged poses remain suppressed, unsupported transforms snap, and
render/projection failures retain the last-good frame. `service_diagnostics_final`
preserves final diagnostics from this Player instance, independently of another
Editor or CLI instance's audio/persistence results.
