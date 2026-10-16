<a id="исправления-ревью-и-повторной-подготовки-custom--16102026"></a>

# Review fixes and Custom repreparation — 16.10.2026

[English](review-fixes-20261016.md) | [Русский](ru/review-fixes-20261016.md) | [Documentation](README.md)

Four review defects and recompilation of unchanged Rust Custom after Stop are fixed and checked on Windows/Linux. The final code checkpoint is `3233497c45f2e00ce4c853980c9a01ec53122005`, the base is `b4f099b51a2cb37bc007a678b355d775c34935e3`, and the branch is `codex/review-fixes-and-custom-reuse`. The ready `outputs/ModuleEngine.exe` is updated. Functional checks of the fixes are complete; the strict Player 60 FPS target remains unmet on both tested configurations.

<a id="что-изменилось-и-как-пользоваться"></a>

## What changed and how to use it

- Asset moves, deletions and replacements handle typed references. Text fields and unknown archived data are preserved; an operation with ambiguous references returns diagnostics.
- Inspector Apply/Inherit change overrides of the selected component. Components with similar IDs, such as `foo` and `foo.bar`, retain their own data independently.
- Rust Custom launches the actual Cargo product after validating the native target. Build failures show compiler diagnostics; cleanup does not affect foreign products or products still in use.
- Player and Team compute one working graph before selecting features and sources. An unconnected alternative Vulkan renderer does not add Vulkan to a software-only package.
- Game, Scene and live Rust application use a shared bounded store of immutable programs. Stop releases the game host; the next launch creates a separate worker with initial state, using verified ready code. Users need to enable no new setting.

Reuse does not mean skipping input checks: changes to source, SDK, dependencies, compiler context or build products are checked. An unsupported context is safely compiled separately. A cold build, new context or corrupt cache can still require compilation. Limits and miss reasons are described in [custom-program-store.md](custom-program-store.md).

<a id="финальная-приёмка-implementation-checkpoint"></a>

## Final acceptance of the implementation checkpoint

All files below are in `outputs/review-fixes-20261016/`. The store namespace remains v3; final `final-*` results refer to checkpoint `3233497c45f2e00ce4c853980c9a01ec53122005`. Earlier results are preserved in `pre-v3/`, `pre-linux-clippy-fix/` and the stage log below; they do not substitute for final verification.

| Check | Windows | Ubuntu / WSLg |
| --- | --- | --- |
| Workspace, all targets, offline/locked/release | 585 passed, 6 ignored | 594 passed, 6 ignored |
| Doctests, fmt, Clippy `-D warnings`, normal Editor/Player build | Passed | Passed |
| Native Workbench: independent state/PIDs and repeat after Stop | Passed, 18.71 s | Passed, 10.74 s |
| Real Custom80ms, egui and hot apply | Passed, 13.56 s | Passed, 7.44 s |
| Original full Editor benchmark, watchdog45s, 90+90 frames | Passed, DX12, 19.484 s | Passed, Vulkan: X11 13.122 s / Wayland 20.254 s |
| Full export, SDK prewarm, GameWorker acceptance | Passed | Passed |
| Player/Team export and strict verification of relocated Player | Passed | Passed |
| Software-active / Vulkan-inactive Player and Team | Passed | Passed |
| Standalone Player: strict 60FPS / 1080p / 20-enemy target | **Not met** | **Not met** |

Native Editor acceptance checks real WGPU windows and frames, Play/Stop command execution, a new actor after Stop, time0 and document/file preservation. The automated scenario uses the shared button dispatcher; it does not check external mouse clicks. Both Linux backends were checked through **llvmpipe software Vulkan**: this proves Wayland/X11 functionality, not Linux hardware performance.

Windows Player: WGPU/DX12 on RTX 4080 SUPER, actual 1920×1080, 20 live enemies, 120 unique frames and 0 error frames. Results were **59.9644 FPS**, mean interval **16.6766 ms**, p95 **18.3717 ms** against a **16.6667 ms** budget. `target_met=false` remains in `final-windows-player-dx12/native-frame-report.json`, although UI timing and workload predicates are met. Functional benchmark completion and UI timing do not replace this criterion. The previous checkpoint also missed the strict 60 FPS target; its result is archived.

Linux Player: actual 1920×1080, WGPU/Vulkan on llvmpipe, **15.5078 FPS** by distinct completed game frames, with mean interval **64.4837 ms**, p95 **91.0565 ms**. UI callback mean 15.4944 FPS / p95 72.1309 ms is reported separately. `target_met=false`, `ui_timing_met=false`, `shipping_twenty_enemy_load_met=false`: the long run without manual input reached Defeat/health0. The benchmark completed, with 120 unique frames and 0 error frames, but this is **not successful acceptance of a 20-enemy / 60 FPS load**. This software result cannot be extrapolated to a hardware Linux GPU.

The Linux Full fixture was explicitly created without `game.audio_cues`, because native audio remains an existing Linux limitation. The authored `game-project-v4.json` was unchanged. Further work on game-frame interval stability and hardware Linux verification remain open.

The six ignored cases in each OS's standard suite are disclosed: native Workbench, slow-UI/hot-apply and two runtime checks of the installed toolchain/context were run as separate native acceptances. All four passed on each OS's final source. The old diagnostic synchronous catch-up test and fixture emitter are not new performance criteria; the emitter ran in stage4.

Primary evidence: `final-{windows,linux}-{workspace-tests,doc-tests,fmt,clippy,build}.log`, `stage5-windows-final-*`, `stage5-linux-v3-*`, `stage5-linux-final-summary.json`, `final-{windows,linux}-editor-*/report.json`, `final-*-game-worker/game-worker-report.json`, `final-*-software-metadata.json`. The final aggregator reports functional readiness, the Player60FPS target and source-project preservation separately.

Final strict summary: `functional_state=passed`, `player_60fps_state=failed`, pending0. The only two failed requirements are Windows/Linux Player60FPS. Overall `state=failed` and exit1 are preserved: successful functional checks do not turn unmet performance into a green result. The source seal contains 155 main files against 152 required native files; all 107 backend/document/tool entries were also checked by SHA. Final JSON is `final-summary.json`; the preliminary version before the docs commit is saved separately.

<a id="готовый-результат-и-сохранность"></a>

## Ready result and preservation

Run the updated Windows Editor from `outputs/ModuleEngine.exe`. Its SHA-256 is `2477792D79F64A360AE7BEE34DF1BA81DEBEFB949FE6A4E6FA19EF79C5E66BD3`: it matches the final normal build, both checked Team EXEs and their package pins. Build time precedes all concluding native/package scenarios. An intermediate normal build had another SHA; the publication guard rejected its old expected value before replacing the file. After comparison, the final EXE actually verified was published.

Native Linux Editor SHA-256 is `f66a2fb317a507d15615621d2378f77dbf7cd2a251b6f1b6ab6feed9f7a91b19`. Delivered Players have their own package manifests: the shared Cargo target binary changes when building different features and does not identify all packages at once.

Seven protected source JSON/layout/binding/SDK files retained baseline SHAs. The only intentional replacement from the baseline list is the ready Windows EXE; the previous binary is saved in `outputs/review-fixes-20261016/prior-ModuleEngine-DX12.exe` with its original SHA. New Full/Team/Player directories are real verification exports; the working branch and sources remain in one checkout. No residual owned native Windows/Linux processes remain after acceptance; `final-process-preservation.json` and a Linux idle snapshot are saved beside the final JSON. No unrelated process was stopped.

Tools were installed in the preexisting Ubuntu with explicit authorization. Linux uses `.tools/linux-x86_64/env.sh`, native Rust/Cargo1.99.0, GCC13.3, pkg-config and DXC1.9.2609.5. The exact toolchain/registry/DXC copy on ext4 was verified by content, mode and symlinks; full content proof improved from 102.04 to 2.26s. OS, GPU drivers, release profiles, IPC limits and benchmark criteria were unchanged. Windows GUI retained WGPU/DX12, Linux WGPU/Vulkan with X11 and Wayland; Glow was not used.

<a id="журнал-этапов"></a>

## Stage log

The following sections preserve intermediate findings and red checks. Phrases such as “still required” refer to the time of a particular stage; current status is shown in the final acceptance table above.

<a id="границы-и-порядок"></a>

## Scope and order

1. Asset mutation: only typed AssetReference and built-in resource contracts; Text and unknown archived payloads are preserved. Real move/delete/relink, validation, save and rollback are checked.
2. Inspector: the same longest-owner rule for prefab/local/unknown attachments; one component's Apply/Inherit preserves neighboring namespace data. The production edit path and reopen are checked.
3. Custom build: actual Cargo executable and native target/config; Windows/Linux default, explicit native env and config, foreign-target rejection, cleanup/cancel/leases.
4. Player/Team: one runtime closure before selecting features, sources and resources; software-active/Vulkan-inactive export, explicit enabled modules, active Vulkan, transitive includes and Player relocation.
5. Custom reuse: bounded retention of immutable compiled code, separate instance/state/worker. Freshness accounts for source/SDK/dependencies/lock/toolchain/target/build configuration; uncertainty means a safe miss. Primary acceptance is the complete original native Editor benchmark with Custom and the previous overall 45 s watchdog.
6. General checks of final source, real Windows DX12/Linux Vulkan windows, ready Windows EXE update, preservation, Git and report.

Implementation stages and focused checks run sequentially. Independent reading of subsequent stages is allowed. Heavy builds and GPU/performance checks do not compete for resources during measurements. Historical 535/197/158 tests are not treated as verification of new fixes.

Windows GUI remains WGPU/DX12; Linux GUI WGPU/Vulkan with Wayland/X11. Glow baseline, drivers and system settings are unchanged. ECS/GameSession/shared Host, frozen Object compatibility, optional ARPG and separate Two Arenas/Puzzle are preserved. Native audio/pickers, hardware Linux/ABI and additional P3 review findings are outside this task.

## Baseline

`outputs/review-fixes-20261016/stage0-baseline.json` contains HEAD/branch/status, SHA-256 of authored JSON, layout, binding, published Windows EXE and SDK metadata, Windows toolchain and free disk space. The previous working EXE is saved separately as `prior-ModuleEngine-DX12.exe`; this is one ready artifact, not a copy of the source project.

Linux uses the already installed native environment `.tools/linux-x86_64/env.sh` and ext4 target `/root/.cache/module-engine-linux-x86_64/target`. Native baseline probes are saved separately from new functional acceptances. New tools/OS/drivers are not installed automatically.

<a id="результаты-этапов"></a>

## Stage results

<a id="1-типизированные-операции-с-ассетами"></a>

### 1. Typed asset operations

`AssetWorkspace` receives shared composition through `with_composition`; callback validation is no longer considered a type source. Move/delete/relink/usages use explicit `AssetReference` and builtin resource contracts. Text is preserved. Unknown/inactive/stale payloads, including the entire inactive property store, remain archived; ambiguous matches return diagnostics displayed by the editor. With an existing lock, a missing or incompatible schema pin prevents data rewriting. Detach removes only an actually detached empty known mesh, without global opaque-payload cleanup.

Windows: `cargo test --offline --locked --release -p module-engine-project --lib` — **50 passed**, log `stage1-project-final.log`. Project strict Clippy and Editor release all-targets check passed (`stage1-project-clippy-final.log`, `stage1-editor-check.log`). The original public audit repro, with its code unchanged, now returns `before=assets/a.png`, `after=assets/a.png` after a successful move (`stage1-public-text-repro.log`). Coverage includes real typed `id`/`module` fields, nested arrays, prefab/local/overrides, stable IDs, disk save/reopen and rejection before file changes.

Intermediate red logs are preserved: a Clippy finding in the new test was fixed; after adding a schema, the source fixture required real `capture_lock`, and the production pin guard was not weakened (`stage1-project-missing-fixture-pins.log`). Linux and the combined final source have not yet been rechecked; success of the whole work stage is not yet claimed.

<a id="2-владение-overrides-в-inspector"></a>

### 2. Inspector override ownership

The shared Core helper `component_override_owner` is used by scene resolution, runtime stripping, asset mutation and Inspector. Apply/Inherit/Remove compute ownership from the complete prefab+local set before removing a component. `component_value` also no longer mixes a long-ID override into a short-ID draft. Applying a local component clears its previous overrides so they cannot override newly entered values. Prefix-related IDs remain allowed.

Windows: the production Inspector test clicks real Apply/Inherit buttons in all **18 combinations** of prefab/local/mixed and active/inactive/unknown long component; it checks validation, disk save/reopen, original `foo.bar.value`, color and opaque local payload. `stage2-inspector.log` — 5 tests passed; separate local Apply/Remove and namespaced vector tests passed (`stage2-local.log`, `stage2-namespaced.log`). Core **101** and Project **50** passed together (`stage2-core-project.log`). After helper reuse, Clippy flagged a now-redundant closure; the fix changes no semantics, and intermediate logs are preserved. The final strict Clippy result is in `stage2-clippy.log`.

Subsequent targeted QA found two refinements. An existing lock also requires the owner's implementation pin, not only the exact SchemaPin. When generating new overrides, Apply checks ownership again: an unknown nested `foo.bar.value` field inside aggregate `foo` stays local if the dot path belongs to a separately attached `foo.bar`. Otherwise reset would preserve a foreign override, but subsequent Apply would overwrite it again. The GUI matrix was extended with precisely such aggregate data and uses an Engine profile published through production helpers and real exact locks; the complete document/profile/SDK validator runs before/after Apply and after disk reopen.

The refinements were checked after stage3 completed: Project **51 passed** (`stage12-qa-project.log`), Inspector group **5 passed**, including 18 combinations (`stage12-qa-inspector.log`), strict Editor all-targets Clippy exit0 (`stage12-qa-clippy.log`). The package resource-search fixture initially also lacked a pin for the new module owner; it was fixed with real `capture_lock`, not by weakening the guard. The red log is preserved as `stage12-qa-project-missing-fixture-owner-pin.log`. Original green stage2 logs are not replaced by later results.

<a id="3-фактические-cargo-artifacts-и-native-target"></a>

### 3. Actual Cargo artifacts and native target

The worker path comes from the owned package/manifest/bin's `compiler-artifact`, rather than constructing `target/debug/name`. A successful exit, unambiguous executable and `build-finished.success` are required. The generated std-only build script publishes HOST/TARGET; before the SDK probe, the selected native Rustc host, HOST=TARGET equality and PE/ELF machine/header are checked. Cargo environment and target configuration remain effective.

Cargo JSON is parsed by a separate streaming collector: the long overall stream is drained, while line length, number/size of retained products/proofs and rendered diagnostics are bounded. The worker protocol packet limit is not increased. Rust errors return readable rendered text; the original stream is retained in a bounded build log.

Cleanup understands default and explicit-native profile scopes, as well as verified owned build-script products. Canonical path guards and exact package/Cargo suffix matching preserve shared dependencies, neighboring names and active runnable leases. Linux executable placement on the native target filesystem from b4 is preserved. Unverified external build directories are not deleted on assumptions.

Full Linux runtime run before the last narrow refinements: **32 passed**, 511.43 s (`stage3-linux-runtime.log`). These are functional cold-compilation checks, not performance acceptance. First full Windows run: 28/29 passed; the only failure was sandbox denial of `mklink /J` before checked cleanup itself. The exact test passed with authorized native access (`stage3-windows-junction-native.log`); the original red log is preserved.

Subsequent checks of all affected cleanup/native/foreign/collector/diagnostic/stream scenarios passed on both systems, as did strict runtime Clippy. Total distinct stage coverage is **31 Windows / 33 native Linux cases**; these are original full runs plus targeted reruns, not one full run of the final source. `stage3-summary.json` contains results of 11 concluding commands, SHA-256 of three implementation files and a check for no remaining Cargo/rustc/test-workers. Another red Windows fixture log is preserved: an uppercase hash in the test matched its own lowercase name on the case-insensitive filesystem; the fixture was replaced with a separate name, without weakening the production matcher. The combined final source will be checked separately after stages 4/5.

<a id="4-общий-рабочий-граф-playerteam"></a>

### 4. Shared Player/Team working graph

The new `runtime_delivery` first validates the authored document/profile, then computes a working dependency copy before selecting native features, Rust/HLSL and include files. Root traversal starts from viewport outputs, while nested traversal preserves interface contracts. The order and bindings of scheduled game systems are not trimmed. Enabled modules, pins and the published profile are preserved; unconnected Vulkan does not become an active renderer merely because of a native feature. Feature-subset checking occurs before the native build, and strict ready-Player verification is preserved.

New Project → Inherit Current uses the working graph and source maps rather than restoring archived published-profile nodes. Team continues preserving the declared authored asset library; Player applies its active-game resource policy.

Targeted Windows checks: projection group **8/8**, existing package group **10/10** (two checks overlap), owner-feature mapping **1/1**, Team audio variants **1/1**, strict Editor all-targets Clippy exit0. Initially the collision fixture declared an include only in an unused source; after correctly removing that source, export naturally succeeded. Three such fixtures now bind the include to a real retained Custom HLSL pass, preserving the meaning of collision/resource-owner checks. The red log is preserved. Native Linux checks and real new export/relocation are not yet complete; unit coverage is not presented as native package acceptance.

The first full targeted Linux run also passed: projection **8**, package **11**, owner-feature **1**, Team/audio **1**, strict Clippy. The existing package test actually exported Vulkan Player and checked the copied native binary with offscreen rendering; this suite opens no GUI windows. The reported 163.10 s for the package group includes native Player building and is not a game-FPS measurement. Separate software-active/inactive-Vulkan export and relocation remain final acceptance steps.

After adding the required-file inventory and ignored fixture emitter, the final Windows rerun passed: projection **8**, package **10**, distribution inventory **8**, Clippy and emitter. The initial inventory filter selected 0 tests; this was corrected with an exact filter. Then 3 inventory tests received AccessDenied on atomic rename in virtual TEMP sandbox; rerunning with child-only TEMP/TMP in the owned `.work/stage4-windows-temp` passed 8/8 without production-code changes. Original logs are preserved. Native rerun of the final source is still in progress.

Final native rerun: projection **8 passed** (emitter separately ignored), package **11 passed** in 42.91 s with a prewarmed build cache, distribution inventory **9 passed**, strict Clippy exit0. Native and Windows emitters use an absolute destination and `create_new`, do not export packages inside the test and do not overwrite existing files. Their JSON is saved as a separate input for subsequent real software/inactive-Vulkan export. Required native inventory was extended with three new compilation modules from stages 1/3/4.

<a id="5-неизменяемый-compiled-code-и-отдельные-запуски"></a>

### 5. Immutable compiled code and separate runs

Implementation follows stage 4 commit `59937fb`. Workbench owns one `Arc<CompiledProgramStore>` for Game, Scene and hot-apply preparation; Stop retains only this handle, releasing the game host/actor. Shared GameHost receives an optional store through a new constructor, preserving previous APIs. CLI Editor wrappers create a local store; a persistent Cargo dependency namespace should allow prewarming the same SDK before native window verification.

A production Workbench regression was written: Game changes state and records a PID, Scene begins with initial state, and Stop creates another independent state/worker without rebuilding the unchanged root. The old slot is intentionally kept alive by the test to check independence and the lease; this does not claim that ordinary Stop must preserve the old process. The native Editor report receives only a counters snapshot; the 45 s watchdog, 90+90 measured frames, real Game/Scene and actor/identity criteria are unchanged. Final native checks remain mandatory.

Intermediate Windows integration of the real store passed: `stage5-windows-store-reuse-explicit-build-input.log`, **1 passed, 23.42 s**. It checked concurrent request coalescing, one root build and real Cargo fresh, separate state/PIDs, eviction, export and the lease until the last instance releases it. This is a runtime check, not yet a native Editor benchmark or full suite of the final source.

Real checks found incorrect application of Linux loader policy on Windows, loss of Rustup argv[0] when canonicalizing a Linux symlink, differing Windows path spellings and a verbatim target spelling unusable by the linker. These were fixed without profile/timeout changes. Another rebuild cause: a generated build script without `rerun-if-changed` treated build.log and the marker as whole-package inputs. Its sole file input is now explicitly declared as `build.rs`; Rust/Cargo continue tracking other compile inputs. Original red logs are preserved separately from the green rerun.

The real native resolver was confirmed on both OSes: Windows **586 files / 5 ordered searches**, Linux **26 / 86**. This proves discovery, not final performance or the whole workflow. The first complete Windows Editor all-targets check also passed without warnings in 10.38 s (`stage5-windows-editor-initial-check.log`); it precedes the last narrow proof fixes and does not replace final tests/Clippy.

After final narrow fixes, Windows proof group passed **10/10**, native resolver group **8/8**, including the explicit installed-tool test (`stage5-windows-proof-final.log`, `stage5-windows-native-final.log`). `cargo fmt --all -- --check`, strict Editor all-targets Clippy and strict runtime all-targets Clippy passed. Initial Clippy logs contained five findings on complex tuple signatures/argument count and one on a test clone; aliases, a context struct and test comparison were fixed without lint allows. Functional Workbench checks, the full workspace and native Editor benchmark remain separate mandatory acceptances.

The complete Linux production context was also correct, but took **102.04 s** with native Rust/Cargo originally placed on DrvFS (`stage5-linux-context-direct.log`). That result is not an optimization and exceeds the native Editor watchdog. The authorized installation of the same pinned toolchain/registry/DXC is being placed on Ubuntu ext4 with content, mode and symlink-target checks. This changes the local native environment; input integrity, compiler flags, IPC limit and benchmark watchdog are preserved. Relocation requires a new measurement and all subsequent Linux acceptance.

The first Windows Workbench regression confirmed independent state/PIDs/time and document preservation, but failed on an excessive requirement of exactly one build over Game → Scene → Stop → Game. Diagnostics (`stage5-windows-workbench-context-diagnostic.log`) showed the only environment change after the first Game preparation: hidden Windows key `=C:`. Counters: Game built1, Scene built2, Stop/Game hit1 and real root fresh1; rejected/repair/uncacheable — 0. Such per-drive entries are passed to the child environment (local source `std/sys/process/windows.rs`, `make_envp` was checked), so removing them from the key is unsafe. The test contract was corrected: Game/Scene require a hit in an unchanged environment; a new environment permits one new root. Scene → Stop → Game must preserve the environment and root-build count, increase hits and create a new state/worker. Production freshness was not weakened; original red logs are preserved. A rerun of the corrected check is still required.

An exact copy of the installed Linux toolchain/registry/DXC is activated on ext4 at `/root/.cache/module-engine-linux-x86_64/tools/native-20261016-7d95c48d`. Contents, modes and symlinks were checked; 201 existing registry archives match the lock, and 141 other lock rows were absent before relocation too. Old tool homes are preserved. `stage5-ext4-tools-migration.json` and `stage5-ext4-tools-validation.json` confirm installation but do not themselves prove a speedup; the new measurement is separate.

The new native Linux measurement on ext4 passed: resolver **0.21 s** (26 files / 86 ordered searches), full supported context **2.26 s** instead of 102.04 s. The real check of coalescing, separate state/PIDs, eviction, export and the last lease passed in **26.97 s**. The first native-test-binary rebuild after changing tool-home paths took a separate 55.55 s and is excluded from these test times. All three exit0 results are recorded in `stage5-linux-ext4-initial-summary.json`. This confirms the specific proof speedup, but is not yet a native Editor/Player GUI result or full suite of the final source.

Independent reading of freshness found incomplete Cargo-config checking with an unspecified default home or relative `CARGO_HOME`. Such contexts are now explicitly uncached; no home guessing or changes to the real compiler environment were added. Positive fixtures use a separate absolute owned home; a new regression checks None, relative and Windows drive-relative cases. The final proof/build rerun follows this refinement.

After the CargoHome guard, Windows proof group passed **11/11** (`stage5-windows-home-guard-proof-retest.log`). The latest normal Editor/Player and Editor test binary were rebuilt. The corrected production Workbench regression passed in **28.23 s** (`stage5-windows-workbench-home-guard-native.log`): the first Game/Scene needed two different root contexts because of `=C:`, and subsequent Stop/Game produced a real hit/fresh without a new build. State/PIDs/time and unchanged authored document passed. The first targeted command invocation used an incorrect package name and ran no tests; that red log is preserved, and the result counted is the rerun with actual `module-engine-script`.

Windows Full export, capabilities and SDK prewarm passed. The full **unchanged 45-second native Editor benchmark** passed: internal elapsed **20.67 s**, wall **21.51 s**, warmup 30+30, measured frames 90+90; `complete`, `measurements_valid`, document/file preservation and Play/Stop are true. Actual GUI is **WGPU/DX12, RTX 4080 SUPER**. Here the store produced root built1/fresh2, hits2, coalesced wait1, dependency built0/fresh93, without rejection/repair. UI p95: Stop 3.14 ms / Play 3.36 ms; callback interval p95 7.54 / 7.87 ms. Visibility/focus reports are 0 frames; this is native window/render/callback proof, not measurement of display latency or standalone-game FPS. `stage5-windows-native-gate-summary.json` and `final-windows-editor-dx12/report.json` contain details. Separate actual GameWorker acceptance with Custom80ms passed (`final-windows-game-worker/game-worker-report.json`, wall23.36s); its client-call timings are not FPS.

The first Windows GUI invocation was rejected before opening a window: PowerShell/.NET converted an attempted null environment value into a present empty screenshot variable. The wrapper now actually removes the child env entry through `Remove-Item Env:` and checks its absence; the production benchmark guard was not weakened. The original rejection is preserved as `stage5-windows-gui-rejected-empty-screenshot-env.log`. Independent QA also removed the possibility of false wrapper success with a nonexistent EXE/inherited LASTEXITCODE; mandatory Phase and launch checking were tested in isolation in `script-wrapper-qa/`.

The latest normal native Linux Editor/Player/examples were built (Cargo5m47s cold after registry-path changes), and the Editor all-targets test binary took 1m38s. Raw native Workbench regression passed in **23.18 s**: root built1/fresh2/hits2, independent state/PIDs and time0 after Stop, document unchanged, without environment changes (`stage5-linux-workbench-native.log`). Linux source seal, new Full export, capabilities and exact Full SDK prewarm passed. Windowed X11/Wayland and worker acceptance are separate; the entire workspace, corruption/include tests and ready-package relocation remain open.

The unchanged full Linux native Editor benchmark now passed **in both window systems**: X11 **13.802 s**, Wayland **13.913 s** with the previous watchdog45s. Each report has complete/measurements_valid true, measured90+90, warmup30+30, actual pan changes, Play/Stop verified with a new actor and time0, document/project file unchanged, errors[]. Store in each case: root built1/fresh2/hits2, dependency built0/fresh93; full content proof totaled about 7.5/8.0s. Actual GUI is WGPU/Vulkan, **llvmpipe software**, not hardware Vulkan. Native worker acceptance also passed with unchanged source. Evidence: `stage5-linux-original45-summary.json`, `stage5-linux-native-gate-summary.json` and `final-linux-editor-{x11,wayland}/report.json`. Native processes exited after the gate. This eliminates the original reproducible post-Stop benchmark failure; full tests and real package/relocation checks are still running.

The first full Windows workspace run afterward reached runtime: **59 passed / 1 failed / 2 ignored** there. The sole failure was not broken cache logic but a real MSVC `LNK1104` in the **first** isolated build: the build-script executable output path reached 269 characters. Key segment `sdk-v2/<64hex>` made the path unnecessarily long. The namespace is now `sdk-v3/<16hex>`, while the full key remains in marker/receipt and memory identity. A separate named regression checks two different full keys with the same locator and preservation of the first owner. Profiles, assertions, IPC and watchdog are unchanged.

Rerunning the original exact real repair test passed in **86.31 s** (`stage5-windows-v3-real-repair.log`), including root-only repair preserving verified SDK dependencies, SDK corruption with old mtime in a new store/source, a missing SDK product, a real subsequent hit and last-lease cleanup. The previous full red log is preserved as `stage5-windows-full-pre-v3-red.log`. Old window reports/logs were moved to `pre-v3/`: they confirm the previous implementation, but final checks of the new layout must run again. New Full fixtures `windows-full-v3` and native `review-acceptance-20261016-v3` are created for them; previous fixtures are unchanged. The full Windows suite was restarted on the final layout.

The complete Windows v3 rerun passed: workspace/all-targets **585 passed / 6 ignored**, including runtime **61 passed / 2 ignored**, with actual corruption/include/coalescing tests performed; whole-workspace wall time **9m39s**. Doctests, `fmt --check`, workspace all-targets Clippy `-D warnings` and normal Editor/Player build also exit0 (`final-windows-{workspace-tests,doc-tests,fmt,clippy,build}.log`). Explicit native Workbench v3 passed in **33.69s**; both ignored runtime checks of installed context and native tools were run separately — **1+1 passed** in 2.41/0.36s. New Windows CLI SHA-256 is `6896247F63E8E409F214A1630E38792F79E432373F34F346D36C3B4B98FA88A2`; it is not yet published to `outputs/ModuleEngine.exe`. New Full/native/package acceptance and the full Linux workspace remain mandatory.

At code checkpoint `c44c2654e96824e8631b72abb831a9371a40dd62`, Windows v3 raw slow-UI/hot-apply acceptance with real Custom80ms passed in **14.66s**. New Full-v3 export, capabilities, SDK prewarm, unchanged DX12 Editor benchmark and GameWorker acceptance passed (`final-windows-*`). Player and Team were actually exported; Team capabilities/Custom check exit0. The relocated Player passed strict package/render verification from TEMP with PATH limited to Windows/System32 and DXC deliberately absent.

Standalone Windows Player benchmark completed functionally: physical 1920×1080, WGPU/DX12 RTX4080SUPER, workload/render without errors, UI timing met. **The strict 60FPS target was not met**: cadence of distinct completed game frames **59.9469FPS**, mean interval16.6814ms, p95 **19.2432ms** against a 16.6667ms budget. `target_met=false` remains in `final-windows-player-dx12/native-frame-report.json`; the result is not rounded to successful 60FPS or replaced with GUI callback FPS. This remains a performance limitation, although original Editor45s and functional checks of the fixes pass.

Real software-active/inactive-Vulkan Windows Player/Team exports also passed. Both working graph and shader map were stripped of alternative/unused sources, published engine_profile/engine_lock were preserved; Player native features are only `["ui"]`. Team check and relocated software Player strict verification without Rust/DXC passed. `final-windows-software-metadata.json` records the original fixture SHA and projection/profile checks. Linux v3 rerun and its full workspace/package stage are still running.

At checkpoint `c44c2654`, the full Linux workspace completed with **594 passed / 0 failed / 6 ignored** in 19m21s, including real corruption/include/coalescing checks; doctests passed. Strict Clippy then found one `collapsible_if` in the Linux-only `-l` library-resolution check. Commit `3233497c45f2e00ce4c853980c9a01ec53122005` combines nested conditions into a let-chain without changing the check or adding a lint allow. Windows normal rebuild exit0, but its SHA changed to `BD4199BFC0159764D6A580BCD6E7E1847DA2AB83A9EF26C2725AEEDD27C6D726`, so final suites/native/package acceptance is repeated on both platforms. Previous logs/reports are preserved in `pre-linux-clippy-fix/`, Windows artifacts in its `windows/`; previous Full fixtures were not fixed in place. New Full roots are `windows-full-final` and native `review-acceptance-20261016-final`.
