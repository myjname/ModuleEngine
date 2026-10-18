# Standalone Player

[English](README.md) | [Русский](README.ru.md) | [Documentation](../../docs/README.md)

This package contains the shared native Player UI and a separate executable. It
has no production dependency on the Editor or the Two Arenas project.

The default build enables no optional implementation. Select native packages
explicitly: `ui`, `arpg`, `puzzle`, `vulkan`, and `audio`. A puzzle window uses
`ui,puzzle,vulkan`; it does not require ARPG or audio. `ui` enables UI callbacks
only for gameplay packages already selected. Installation includes neutral core
contracts, the host-bound Custom contract, and the small generic source rotation
example; installed metadata alone never activates an authored module.

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
Player.exe --verify-package <package-root> <evidence-folder>
Player.exe --headless-check <package-root> <evidence-folder>
Player.exe --benchmark <evidence-folder>
```

Normal launch and native benchmark locate `player.json` only beside the
executable. Verification accepts an explicit package root, advances three real
fixed ticks, and reports actual state and rendering statistics; the rendered
check also saves a 960×540 PNG. Headless verification reports `frame:null` and
constructs no renderer or optional service. Package execution uses no source
checkout locator, compiler root, or Editor distribution policy. Vulkan requires
the package's `.spirv` directory, so a missing artifact fails instead of falling
back to source compilation.

The existing native benchmark retains its 30-frame warmup, 120 UI samples and
120 distinct completed game frames, physical 1920×1080 requirement, 45-second
watchdog, and unmodified twenty-enemy workload criterion. The measured data and
any unmet target are reported directly; a different project does not satisfy
that workload criterion merely by rendering quickly.
