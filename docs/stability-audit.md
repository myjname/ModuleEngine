<a id="функциональность-и-стабильность-04--15-октября-2026"></a>

# Functionality and stability 0.4 — October 15, 2026

[English](stability-audit.md) | [Русский](ru/stability-audit.md) | [Documentation](README.md)

This audit checks the existing Windows engine: Full/Team Editor, Vulkan, authored scenes and assets, Custom Rust/HLSL, and standalone Player. Following slow-Custom measurements, the user additionally approved moving simulation out of the UI and rechecking lifecycle behavior. Passing unit tests are not treated as proof that every window works: file operations, actual GPU output, packaging, and native window loops are checked as well.

<a id="исправления"></a>

## Fixes

| Problem | Change and verifiable behavior |
| --- | --- |
| Custom builds could replace main Cargo workspace artifacts before rustdoc. | Workers use a separate `target/custom-workers`, including when `CARGO_TARGET_DIR` is set. Actual compilation checks that outer-build artifacts survive. |
| Slow Custom ran on the UI, and catch-up ticks multiplied window latency. | Native Editor/Player hand GameHost to a persistent worker; the UI reads completed snapshots without waiting. One tick per iteration allows commands to be handled between updates. Input, Restart, hot apply, and obsolete-candidate rejection are checked separately. |
| The Rust window showed the legacy executor instead of the game executor, and Apply could not finish without a Game panel. | The window prepares the game host in the background and displays its instances, errors, and applied versions. Reset/migration targets the actual game candidate; obsolete text is not applied through an old rejection. |
| Library export bypassed draft checks and ran on the UI thread. | The shared asynchronous workflow is used with a guard, progress, and a report. |
| Replacing Forward/Deferred left settings for removed custom pass IDs. | Disappeared targets are cleared; retained authored controls survive. Undo restores the graph and settings together. |
| Faulty external HLSL repeatedly invoked DXC; fixing an include did not clear preparation rejection. | Failures are cached by dependencies; repaired files trigger a retry. The watch is captured before worker startup, including a repair during compilation. The last accepted runtime is preserved. |
| An include change during compilation could publish SPIR-V under another fingerprint. | Dependencies are rechecked before artifact publication and after pipeline creation; temporary files have RAII ownership. |
| Restart reset GameSession while retaining renderer overrides and the old camera target. | R, HUD, and toolbar use one Host restart: world, Custom state, render commands, audio, and camera target remain consistent. |
| A game-tick error repeated every Player UI frame. | Simulation stops until an explicit Restart; the last frame and window handling remain available. Successful rendering clears only its own previous error. |
| Assets hashed the directory synchronously; reference lookup serialized the entire document again for each tile. | Scanning runs in a worker, and back references are computed in one document traversal. An obsolete index is discarded, and new scene edits are not replaced by an old snapshot. |
| Writing an asset file first deleted the original. | A prepared and flushed file replaces the original through rename, without a preceding unlink. This does not add general project crash recovery. |
| Failed import left files and a partial journal without Undo. | Only its own fresh directory and journal are rolled back; existing files/history are not deleted. |
| Failed Player export left a partial package and prevented a retry into the same folder. | Building, package verification, and a trial GPU frame run in staging. Windows publication does not replace a concurrently created destination. Internal staging does not enter the Assets index. |
| Old standard examples confused ACES/Reinhard. | Three files in `examples` were corrected: ACES=1, Reinhard=0. Saved authored variants and the user's `game-project.json` are not automatically rewritten. |

Exact implementations and limitations are described in [GameWorker](game-worker.md), [workflow](editor-workflow.md), [Custom state](custom-state.md), [Assets API](asset-workspace-api.md), [Vulkan](gpu-hlsl.md), and [Player](player.md).

<a id="покрытие"></a>

## Coverage

| Capability | Verification |
| --- | --- |
| Object/ECS, module contracts, system order, lifecycle, and rollback | Core/unit, CPU self-check, integration, Game parity across 180 actions and GPU pixels |
| Graph addition, connection, deletion, nesting, pan/zoom, and history | Actual egui events, runtime rendering, host/Custom preservation while moving nodes |
| Forward/Deferred, SSAO, tone mapping, FXAA, encoding, Custom raster/compute | Vulkan unit/integration, shader reflection, actual pixels, invalid candidate, and include recovery |
| Rust text/graph, compilation, last-good code, schema reset/migration | Custom/unit, game slots, UI Apply without an open Game, runtime/Player precompiled workers |
| File/New/Open/Save/Save As/Recent, edit protection, shortcuts, windows | Workflow unit + `--editor-ui-check`, moved assets, and export |
| Scene/Hierarchy/Inspector, selection and G/R/S, snapping, world/local, Undo/Escape | Scene unit + `--scene-assets-check`, actual pointer gestures, and Vulkan |
| Assets, import, IDs, references, move/rename/delete, trash | Project-tools/unit + acceptance, file Undo/Redo, rejection, and restoration |
| Scenes, prefabs/overrides, Play/Pause/Stop, input/combat/boss warning | Core/Game/UI unit + two arenas, defeat, Restart, and victory through GameHost |
| Audio and game events | WAV/events/consumption through GameHost; subjective audibility and device settings are not measured |
| Full/Team restrictions and separate Player | Source inventory, frozen blueprint, SDK, actual exports, and precompiled payload launch |
| Responsiveness and speed | Native Editor/Player benchmarks, separate CPU/GPU-readback and UI-cadence figures; no FPS promise for other content |

<a id="результаты-этого-прогона"></a>

## Results of this run

Verification ran on Windows, NVIDIA GeForce RTX 4080 SUPER, and a 144 Hz monitor. The authored `game-project.json` was preserved byte for byte: SHA-256 `BF2D5D86FFB51241CFF34E9247C51890145FC0EFD6418A6EB5DED2BDE2B2DC6B`.

* The shared sequential run: **308 unit tests**, no errors. An additional isolated UI check with 80 ms Custom and a new owner-completion test after window close also passed: **310 distinct unit scenarios** in total. A focused repeat of three worker/lifecycle tests passed too. Diagnostics of the former synchronous catch-up path run separately and are not counted as a mandatory unit scenario.
* All six doc-test suites, workspace/all-targets Clippy with `-D warnings`, rustdoc with `-D warnings`, and the release build passed.
* CPU, integration, settings, editor workflow, Scene/Assets, Game, and GameWorker CLI acceptance passed on the final EXE. All 12 standard JSON examples were checked. Full grants Engine access; Team does not; packaged Player passed `--game-check` with an empty `PATH` and DXC deliberately absent, using precompiled workers/SPIR-V.
* The previous synchronous diagnostic `step(0.25)` with 80 ms Custom took **1281.28 ms** for 15 ticks. In the final worker check, 256 snapshot reads and three input submissions during actual slow Custom took **0.0499 ms** in total. A single `latest` had p95 **0.0007 ms**, input **0.0043 ms**; nonblocking Drop took **0.0774 ms**. These figures describe the UI/worker bridge, not FPS or acceleration of the algorithm itself.
* Native Editor: 90 actual pan frames each in Stop/Play; the document and Custom identities were preserved, simulation advanced in Play, and there were no errors. In Play, UI handler median was **1.07 ms**, p95 **1.86 ms**, max **3.46 ms**. In Stop: median **0.90 ms**, p95 **1.31 ms**. Unlike the old baseline, the new process was unfocused; no percentage speedup is claimed. OS visibility was undetermined, and actual physical mouse/screen latency was not measured.
* Native packaged Player: **120 distinct completed frames**, 1080p, 20 living enemies, no errors; **60.05 new game frames/s** and **141.00 UI callbacks/s**. Game-period p95 was **18.18 ms**, max **20.80 ms**. The strict `mean/p95 ≤ 16.667 ms` criterion was **not met** (`target_met=false`): a nominal rate near 60 Hz was reached, while frame consistency remains a limitation. The old synchronous Player's 105 frames/s and the new UI FPS cannot be compared directly: game execution is now limited by fixed updates, and repeating old pixels does not count as a new frame.

Logs and JSON are in the working project's `outputs/full-functional-audit/`: `serial-workspace-tests.log`, `isolated-slow-ui.log`, `final-worker-lifecycle-tests.log`, `final-doc-tests.log`, `final-clippy.log`, `final-rustdoc.log`, `final-*.log`, `final-worker/game-worker-report.json`, `editor-native-after/report.json`, `player-native-after/native-frame-report.json`. These are results for specific content and a specific PC, without a performance promise for arbitrary models or Rust code.

<a id="повторение"></a>

## Reproduction

```powershell
.\verify.ps1
# Also open real windows and measure the window loop:
.\verify.ps1 -NativeBenchmarks
```

The script uses pinned installed tools and an offline Cargo cache. Tool and dependency restoration runs separately through `scripts/bootstrap.ps1 -Restore`. Shared unit tests run sequentially: several scenarios use a shared Cargo cache, GPU, and strict execution deadlines. The UI check with 80 ms Custom runs as a mandatory separate stage with `--ignored --exact`, retaining Play, hot apply, and latency checks. During parallel preparation, slow fixtures exceeded the working Custom timeout of 150 ms because of OS scheduling; production timeout was not increased to obtain a green test. A separate timeout scenario checks stopping and recovery.

Doc-tests, Clippy, and rustdoc are checked separately; then CPU, integration, settings, game, editor workflow, Scene/Assets, and GameWorker acceptance run. Native measurements require an available desktop. They may write Recent into the selected Full package's personal settings.

The audit uses one Git workspace. The original version is retained in commit `6c16533c8d5f2531ad7f4aa5eec5f579e57c785b`; fixes follow in subsequent local commits. Build/cache/reports and downloadable DXC binaries are excluded from Git; pinned versions and licenses are retained. Test fixtures and Full/Team/Player exports verify functionality and are not backups of the source project.

<a id="границы-вывода"></a>

## Limits of the conclusions

This run does not prove the absence of every possible error. It checks a specific PC and a finite set of scenarios. Native Windows dialogs have ABI/cancellation and workflow checks, but a human selecting each file in an actual COM window is not automatically checked. A fresh Windows/driver/tool installation on another machine, Linux, and subjective screen latency were not checked.

Previously stated limitations remain: synchronous GPU readback and external HLSL compilation on update, single-voice PCM WAV, a limited visual subset of Rust, no general file-transaction recovery after a crash, and no sandbox for untrusted Custom. A cancelled read-only scan may finish in the background; its obsolete result is not accepted. These are limitations of the current implementation, not features completed by this audit.
