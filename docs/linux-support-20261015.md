<a id="подготовка-linux-проверка-15102026"></a>

# Linux preparation: verification on 15.10.2026

[English](linux-support-20261015.md) | [Русский](ru/linux-support-20261015.md) | [Documentation](README.md)

Branch: `codex/linux-native-preparation`. Original commit: `d9eb584f8ee94d152a5a4c0d23a1a4391e32668e`; the initial tree was clean. This is the implementer's report; independent review will be a separate stage.

Linux Editor and exported Player were built natively in the existing Ubuntu/WSL2. Actual Wayland/X11 windows use WGPU/Vulkan. Player, Full, and Team were exported; Player was checked without Rust/DXC. Linux is not fully ready yet: full Editor acceptance with Rust Custom exceeded 45 s during repeated preparation after Stop; native WAV and file/folder pickers are absent, and hardware GPU has not been checked. Windows GUI remains WGPU/DX12 with the previous DWM fix.

<a id="изменения"></a>

## Changes

| Stage | Implemented |
| --- | --- |
| 0 | Source seal, Git snapshot, and SHA-256 hashes of authored graphs, personal layout, and published Windows EXE were retained; the existing Ubuntu/WSL was checked. |
| 1 | Shared GUI policy: Windows WGPU/DX12, Linux WGPU/Vulkan. Target-only Wayland/X11 features, window-system selection, and actual adapter/display diagnostics. No hidden fallback to Glow. |
| 2 | Native names/platform metadata, Unix execute bits, native DXC/shared libraries, separate build caches/lease, and Linux no-replace publication. Explicit audio-capability check. Rust Custom/Cargo use their own Linux process groups. |
| 3 | The read-only probe creates an actual Vulkan instance, enumerates devices, and checks tools/display sockets. Native builds, HLSL compilation, window launches, and package exports were then performed. |
| 4 | Windows code checks, GameWorker, native Editor, and exported Player were checked again. No Glow baseline or driver/system-setting changes were made. |
| 5 | Added `run.sh`, `verify.sh`, tool metadata, and documentation; Full/Team carry the new instructions/scripts. |

Public SDK and graph schemas did not change. The `native_platform` marker is optional for old schema2 Player manifests; new markers are checked before Host creation.

<a id="среда-и-установка"></a>

## Environment and installation

Ubuntu 24.04.1 LTS, WSL2, kernel `5.15.153.1-microsoft-standard-WSL2`, x86_64. The only Vulkan device is **llvmpipe LLVM 17.0.6**, a software renderer. Physical Linux and hardware Linux Vulkan were not checked.

Rust/Cargo, a C compiler, pkg-config, and native DXC were initially absent. After separate authorization, Rust/Cargo 1.99.0 with rustfmt/Clippy, GCC 13.3, pkg-config 1.8.1, the required Wayland/X11/xkbcommon dev libraries, and DXC v1.9.2609 were installed. 68 Ubuntu packages were installed and 11 dependencies updated; 238 other updates were not applied. Updates include libc/dpkg/compiler runtime/X11. Kernel, Mesa/Vulkan/GPU drivers were not updated; shell profiles and global security/environment settings were not changed.

Exact versions, official URLs, and Rust/DXC SHA-256 hashes: `scripts/linux-toolchain.json`. Installation history/package diff: `outputs/linux-support-20261015/provision-summary.json`, `provision-packages-installed-or-updated.tsv`. Metadata does not run an installer automatically.

Local activation: `. .tools/linux-x86_64/env.sh`. Native Cargo/rustup are isolated from Windows; target is on ext4 at `/root/.cache/module-engine-linux-x86_64/target`. The Cargo registry is in a separate project Linux home. Cargo dependencies were downloaded explicitly; existing pinned versions were not updated. Windows DXC was retained.

Child WSLg processes used `XDG_RUNTIME_DIR=/mnt/wslg/runtime-dir`: the original `/run/user/0` did not point to a working Wayland socket. Wayland/X11 were launched separately; global configuration was not edited.

<a id="проверки-linux"></a>

## Linux checks

All logs mentioned here are in `outputs/linux-support-20261015/`. Reproduction commands: [linux-support.md](linux-support.md).

Cargo suite totals across all 13 crates after fixes and targeted reruns: **535 unique tests passed, 0 failed cases in this summary, 1 ignored**. The ignored test is a separate manual Host catch-up diagnostic; the previously ignored GameUI acceptance ran explicitly and passed. This is a union of results by test name, not a successful single initial run. The separate native Editor benchmark is described below and remains failed. The machine-readable summary retains attempt history: `linux-unit-verification.json`.

* `linux-workspace-check.log`, `linux-editor-release-build.log`, `linux-workspace-clippy.log`: Release workspace/all-targets check, Editor/examples build, and Clippy with `-D warnings` passed.
* `linux-runtime-verification.json`: 20 runtime, 6 host-service, 4 compiler-discovery, and 1 native DXC contract test passed. DXC created three new SPIR-V artifacts in a separate folder, without a Windows shader-cache hit. Descendant cleanup, cancel, persistent state/hot apply, and native compiler discovery were checked. Repeating the native-cache case is not counted as another test.
* `linux-binding-and-delivery-tests.log`: 9 distribution tests passed, including Full/Team restrictions and read-only WSL binding.
* `linux-probe-wayland.log`, `linux-probe-x11.log`: actual windows, actual WGPU/Vulkan/llvmpipe, and the corresponding raw display handle. `linux-headless-cpu.log`: self-check without display variables; snapshots/RGBA from both world models matched.
* `linux-full-source-export-wsl-fixed.log`, `linux-player-export.log`, `linux-team-export.log`: Full, native Player, and Team were exported. `linux-team-capabilities.log`: `profile=team`, `engine_available=false`, compile root is `.sdk`.
* `linux-delivery-verification.json`: Player/worker are ELF with mode 0755, platform linux-x86_64; 14 sealed files, no SDK/compiler/sources/cache/symlinks. Team Editor/Player/DXC are executable, `run.sh` is 0755. Full sources do not symlink to the checkout.
* `linux-player-precompiled-verify.log`: native Player passed with `PATH=/nonexistent`, absent DXC, and no `LD_LIBRARY_PATH`; Rust/DXC were not required at runtime.
* `linux-player-relocated-verify.log`: the same package was moved in its entirety into `relocated-player` and passed again from `/tmp`, with the same absent tools. The initial path in `linux-delivery-verification.json` refers to verification before relocation.
* `linux-team-custom-check.log`: Team Editor actually compiled game logic through the supplied `.sdk`; Engine remains unavailable in Team. This checks a working SDK, not merely file presence.
* `linux-player-wayland/native-frame-report.json`: 1920×1080, 120 distinct completed Game frames, 0 errors, actual Wayland/WGPU/Vulkan. **26.81 new game frames/s**, `target_met=false`. This is cadence of completed worker frames, not physical scanout or a hardware GPU benchmark.
* `linux-game-worker/game-worker-report.json`: `passed=true`, source unchanged. An actual slow tick took 86.07 ms; busy client burst 0.065 ms, drop 0.119 ms. Bounded queue, pause/restart, stale-result rejection, and fault latch/recovery were checked. This is the cost of the nonblocking interface, not FPS.
* Final `linux-runtime-placement-tests.log`, `linux-source-modules-placement-retest.log`, `linux-owned-custom-placement-retest.log`, `linux-worker-placement-retest.log`, `linux-slow-ui-placement-retest.log`: 22 runtime tests, source component, failure/cancel, both worker tests, and isolated slow UI passed. Worker pair took 30.68 s, isolated UI 19.18 s. The IPC limit remained 150 ms and Custom delay 80 ms. Latest Clippy/doc-test commands and Editor/examples build also exited 0; doc-test suites contain zero examples.

The first full native unit run stopped in Editor: 188 passed, 6 failed, 1 ignored. Three failures were caused by WAV in Scene fixtures, two by test-package publication on DrvFS, and one by a slow fixture exceeding the IPC limit. After fixture fixes, the entire Scene module passed (7 tests) and the entire package module passed (9 tests). All remaining crates also ran in a separate sequence. The first full-run error and failed slow-worker/UI reruns are retained; successful logs do not replace them. The current result for each test, with duplicates removed, is in `linux-unit-verification.json`.

The initial `linux-editor-x11/report.json` **failed**: 45 s watchdog, 2588 UI callbacks, Game host/frame ready, Scene preparation pending. The repeat `linux-editor-x11-warm/report.json` also failed: Scene was ready, while Game awaited Custom. In the second run, the cache turned out not to be fully prepared: Full SDK Core rebuilt inside the measurement window. Both show Cargo lock/compile waits while UI callbacks continue; Play/Stop and pan measurements had not started yet. This is an actual cold-start limitation rather than a proven Vulkan bug. The early `linux-editor-wayland.png` shows Rust Custom preparation rather than a ready 3D scene.

After a separate sequential `--check` of the exact Full SDK (`linux-editor-full-sdk-prewarm.log`, exit 0), `linux-editor-x11-prepared/report.json` prepared both windows, completed 30/30 warmup and 90/90 actual pan samples. Game time in Play grew from 0.65 to 2.5 s, identities were retained, and there were no errors. However, **the overall acceptance remained red**: final Stop immediately set Pause and released the previous owner, but the new Game started recompiling an unchanged controller. Only 7.83 s remained before the overall watchdog; this build was cancelled without an SDK rebuild or Cargo lock wait. Production does not reuse immutable compiled code between new Hosts. The original 45-second limit was not increased. Mean Editor handler timings for the measured phases were 2.27/3.12 ms; they do not turn unfinished overall acceptance into a success.

A separate diagnostic of the exact controller/harness compared default debug=2 and `line-tables-only`, with separate SDK/dependency prewarming for each profile. One fresh worker: 9.9345 versus 9.3151 s; ELF: 39,669,624 versus 14,978,784 bytes. `addr2line` resolves actual update/invoke lines in both ELFs, and SDKs match. A gain of about 0.62 s in one comparison does not solve the Stop budget; the production profile did not change. This is a negative result for the hypothesis that DWARF was the main factor, not a successful optimization. Full report: `linux-controller-debug-diagnostic-1792100611731523730.json`.

To separate native GUI/lifecycle from compilation, a separate `project-no-custom.json` was created in the same verification Full: only scheduled node `game-controller` was removed; scenes, root graph, shader/code text, assets, SDK, and thresholds were retained. `linux-editor-no-custom-fixture.json` and `.diff` record the change and confirm that the original Full project is unchanged. **This separate benchmark passed**: `linux-editor-x11-no-custom/report.json`, complete/measurements_valid=true, 5.9407 s, actual pan samples 90/90, Play advances simulation, Stop immediately releases the previous owner and creates a new actor 2 at paused/time=0. Errors are empty. This proves native GUI and the Play/Stop cycle without scheduled Custom; the original full Custom acceptance remains red.

Acceptance uses a separate Full export on ext4: `/root/.cache/module-engine-linux-x86_64/acceptance-20261015/full`. Audio cues were explicitly cleared only in its `project.json`; diff/hash: `linux-silent-fixture.json`. The authored project was not changed. The only cache-only symlink leads to the shared native Cargo target; Cargo rebuilt the selected Player from exported sources. This checks export with cache reuse rather than cold restoration on a new machine.

<a id="проверки-windows"></a>

## Windows checks

The initial full Windows unit run stopped on two runtime fixtures: a junction received Access denied in the sandbox and passed natively; the artifact check incorrectly looked for `debug/deps` in Release and now uses the current test executable's directory. Targeted reruns of fixed cases and crates not yet run passed. The Vulkan suite, isolated 80 ms Custom UI test, doc-test commands, strict Clippy, and rustdoc passed separately. Doc-test suites contain zero examples and do not provide substantive documentation verification.

`windows-editor-native/report.json`: 90 Stop + 90 Play pan callbacks, Play/Stop through the shared production dispatcher verified, document unchanged. `windows-player-native/native-frame-report.json`: 120 new frames, 1080p, 0 errors, 59.97 FPS, `target_met=false`. Actual GUI adapter: DX12/RTX 4080 SUPER. Windows launched with a hidden startup window; physical display presentation/latency was not measured.

`windows-game-worker/game-worker-report.json`:passed, tick 83.56 ms, busy client burst 0.104 ms, drop 0.118 ms. Game simulation is already separated from the UI by GameWorker; that move was not repeated.

After the last runtime change, all 21 Windows runtime tests, fmt, workspace/all-targets check, strict Clippy, and Editor Release build were repeated: `windows-post-placement-*.log`, all exit 0. The initial verification PowerShell wrapper attempt stopped on ordinary Cargo stderr; the wrapper was corrected, and Cargo results are assessed by exit code. This is not a runtime-test error.

Published `outputs/ModuleEngine.exe` was deliberately preserved byte for byte as the verified DX12/DWM build; the new Windows source build is in `target/release/module-engine-editor.exe`. Authored graphs, binding, personal layouts, and previous Windows DXC are checked against original SHA-256 hashes in `final-preservation.json`. The source seal `.module-engine-package.json` was updated separately from authored data.

Final `--capabilities game-project-v4.json` confirms Full/Engine available, no source diagnostics or modified sources. The old root `project.json` is expectedly rejected by CLI as format 3 when 4 is required; no automatic migration/rewriting was performed. This source-project limitation is noted in the launch instructions.

<a id="найденные-проблемы-и-исправления"></a>

## Problems found and fixes

1. **Windows binding in the WSL checkout.** The first Full export rejected `\\?\E:\ModuleEngine`. Strict read-only resolution of an absolute drive path through `/usr/bin/wslpath -u` was added, without a shell or sidecar writes. After canonicalize, selected Engine/manifest/SDK/Team checks remain. UNC/device/relative paths are rejected. Repeating the actual export passed; the binding was preserved.
2. **Unit autosave affected personal layout.** The original was restored against the baseline SHA-256 after saving the modified version. The mechanism and content match are confirmed; the exact last writer was not traced. Test Editors now have separate Layout/Recent roots. Explicit save/load, per-instance isolation, and the original autosave regression passed; production paths did not change. Details: `layout-side-effect.json`.
3. **Platform fixtures.** Linux tests explicitly do not request unavailable WAV; Windows audio assertions were retained. The production demo was unchanged, and Linux audio is not simulated. Custom uses an explicitly selected native Cargo home instead of the Windows cache/toolchain.
4. **DrvFS and atomic publication.** A direct test binary confirmed `EINVAL (22)` from `renameat2(RENAME_NOREPLACE)` on shared `/mnt/e`, even for an unoccupied destination; race-rejection checks were already passing. Linux fixtures moved to native `TMPDIR`; production retains no-overwrite behavior and now reports a lack of filesystem/kernel support. Export passes on ext4. Log: `linux-package-publication-drvfs-repro.log`.
5. **Slow ELF startup from DrvFS.** 80 ms Custom exceeded the total IPC limit of 150 ms and retained its fault until Restart. The generic helper then waited another 12 s for ACK, hiding the cause; the error now includes the bounded fault, tick/epoch/ACK, and timings. A controlled matrix of 80 exchanges in 20 fresh processes with the same ELF produced a first response in 137–143 ms on DrvFS and 91–94 ms on ext4. Marker placement contributed about 3 ms; executable placement was the main factor. The measurement matrix had a separately identified 2 s observation limit and does not count as acceptance passing. Log/matrix: `linux-worker-phase-diagnostic.json`.
6. **Linux worker placement and ownership.** The runnable copy is now stored in a unique leaf of the selected native worker target; source/lock/log remain in the project. Final release removes only the candidate-owned executable, after process completion. Also fixed omission of suffixless Cargo ELF files when cleaning owned build outputs. Preservation of shared cache/neighbors, shared leases, and nested target were checked. Windows placement, SDK, the 150 ms IPC limit, and the 80 ms fixture delay did not change. Original red source/failure/build/marker evidence is retained in `worker-80ms-full-suite-fault/`, `worker-80ms-paired-retest-fault/`, and `slow-ui-80ms-isolated-fault/`.
7. **Instructions and probe.** The CLI example now accounts for a configured `CARGO_TARGET_DIR`. `verify.sh` accurately describes its Vulkan/offscreen tests and separately requires DXC; native desktop verification remains separate. The Python probe rejects Windows PE/interop and non-executable DXC before launch. Syntax, negative cases, and actual native `dxc --version` passed without Cargo/GPU: `linux-script-audit-tests.json`.

<a id="остаточные-ограничения"></a>

## Remaining limitations

* Native WAV is absent: an active request/export returns an error. A silent check does not prove audio; a Linux audio backend is needed.
* Native File/Folder dialogs remain Windows-only. CLI open, saving the current file, and CLI export are available; interactive Import/Open/Save As require a native Linux picker.
* Full native Editor acceptance with Rust Custom exceeded the overall 45 s watchdog: a new Host after Stop recompiles the unchanged controller. Both pan phases and Play completed, immediate Stop released the previous owner, but readiness of the new stopped copy was not confirmed in this run. A separate compiled-code reuse mechanism with independent state and source/SDK freshness checks is needed; the overall test is not declared successful.
* Hardware Vulkan, physical Linux, other distributions, and ABI/glibc compatibility were not checked. 60 FPS is also unconfirmed by the current Windows sample; the strict target test remains open.
* Linux process groups clean up inheriting descendants, but deliberate `setsid` can leave the group. Cleanup after a host crash is not guaranteed. Forced timeout of an external verifier/relocation currently guarantees kill/wait only for the direct child. This is lifecycle management, not a sandbox.
* Game simulation/rendering run on one worker: slow logic reduces the rate of new frames. Legacy non-game Engine Runtime still has a synchronous UI path.
* Linux Team currently also carries unused Windows DXC exe/DLL files (~25.6 MB) because the entire `tools/dxc` is copied. Native runs without them; this is excess size. DXC mode 0777 reflects its DrvFS source; separate mode normalization was not added.
* Separate known P2 issues from the previous review — Inspector prefix overrides and inactive-root renderer/subset Player — were not fixed in this stage.

Next work requires faster repeated Custom preparation after Stop and completion of full Editor acceptance, native audio/pickers, and checks on hardware Linux Vulkan. Full commands and limitations are retained alongside the source.
