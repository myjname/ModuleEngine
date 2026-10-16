<a id="четыре-дополнительных-p2-после-ревью-17fb340"></a>

# Four additional P2 findings after review 17fb340

[English](postfix-p2-fixes-20261016.md) | [Русский](ru/postfix-p2-fixes-20261016.md) | [Documentation](README.md)

Work is performed on branch `codex/postfix-p2-review-fixes` from `17fb340f29be6cf96563948b5a6c2a57c4e4efaf`. The tracked tree was clean before changes. The original report and its reproductions are preserved in `outputs/independent-audit/postfix-review-17fb340/`; new results are stored separately in `outputs/postfix-p2-fixes-17fb340/`.

<a id="границы-и-сохранность"></a>

## Scope and preservation

Work covers only schema-default asset dependencies, partial-prefab Inspector Apply, Windows tool names with extensions and actual Cargo products for source-built Player/Team. Additional P3 findings and architecture changes are excluded. The game, ECS/GameSession, shared Host and optional ARPG are preserved.

`stage0-baseline.json` contains eight protected SHAs: authored JSON, binding, personal layout, SDK metadata and the published EXE. A copy of the latter is saved as `prior-ModuleEngine-17fb340.exe`. The published `outputs/ModuleEngine.exe` is not yet replaced. `.claude/settings.local.json` is preserved; this ignored/untracked metadata file is outside the product changes.

Already installed Rust/Cargo/DXC are used: Windows `.work/cargo-home`, Linux `.tools/linux-x86_64/env.sh`. There are no new installations. Heavy builds and GPU scenarios run sequentially. Windows TEMP is short: `.work/audit-tmp`. OS, drivers, security settings, release profiles and IPC limits are unchanged.

<a id="1-ресурсы-из-schema-defaults"></a>

## 1. Assets from schema defaults

Previously, an attached `{}` component could effectively reference a PNG from defaults, but the usage index saw only existing authored pointers. Move/Delete succeeded, leaving an effective reference to a missing file.

The catalog now stores writable authored references separately from effective schema-default references with provenance. It accounts for exact owner/schema pins, activity, prefab/local shallow merge, defaults and longest-owner overrides. Usage such as `schema-default:<field> at <location>` points to an installed schema rather than an absent authored-JSON field.

An operation that would require changing such an immutable default is rejected before writing project, files, history or trash. The installed schema itself is not rewritten. To make a change, users can explicitly author the corresponding values in project components/templates; the actual default dependency must disappear from every affected authored source. A default of the form `asset://ID` permits a file move with the ID preserved. The candidate is checked separately after Detach: removing an authored value cannot silently expose a default referencing the asset being removed.

Windows focused: `stage1-project-tests.log` — **26 passed**, including four new regressions with real PNGs/file operations. Coverage includes direct/nested-array/prefab/shallow-local defaults, RefuseUsed/Detach/Replace/Move, ID Move+Undo, overrides and inactive/stale pins. An operation regression for allowed explicit null was then added for local, prefab, leaf override and aggregate override inside an array: real Core resolution, Move/Delete/Undo, and schema/authored-data preservation. It passed in the fresh full Windows run. `stage1-project-clippy.log` — strict release all-targets Clippy exit0.

The new default guard guarantees protection for AssetReference fields of active attached `ComponentSchema` in `game.scenes`/`game.prefabs` with compatible exact owner/schema pins. An allowed explicit null is preserved. Defaults from `ModuleDescriptor.properties` / `PropertySchema` — Global, Module, Object and legacy Component — are not resolved by this fix. Runtime computes them separately through `resolved_properties`, including actual live IDs and per-object attachments. A dependency specified only by such a PropertySchema default may therefore still be absent from the usage index. Existing explicit authored references continue to be handled according to active contracts. This is a remaining limitation, not protection of all typed defaults.

<a id="2-apply-для-частичного-prefab"></a>

## 2. Apply for a partial prefab

Previously, schema defaults `stats={speed:1,weight:3}` and prefab `stats={speed:2}` produced draft `{speed:2,weight:3}`, but unchanged Apply wrote local `stats={weight:3}`. Under the existing shallow merge, this replaced the entire prefab aggregate and reverted to speed1.

Inspector compares the draft against the effective prefab + schema defaults. Unchanged Apply creates no unnecessary local/override data. Editing a default-only leaf can be expressed through an ordinary dot override; if local storage is required, the entire top-level aggregate is written with neighboring values preserved. The format's shared shallow-merge contract is unchanged, and `foo`/`foo.bar` ownership is preserved.

The regression through the real egui Apply button checks five cases: unchanged draft, editing a default-only leaf, an unknown nested field, array materialization preserving both indices and optional ObjectReference with explicit null. It includes strict validation, save/reopen and prefab/profile/lock preservation. All cases passed in the full Windows run. The first two appfixture tests failed because of a stale source inventory, not Inspector; the original red log is preserved as `stage2-inspector-stale-inventory.log`. After resealing, both passed in the full run.

<a id="3-короткие-windows-имена-cargorustc"></a>

## 3. Short Windows Cargo/Rustc names

The cache no longer turns `cargo.exe` and `rustc.exe` into `*.exe.exe`: the suffix is added only to an extensionless name on Windows. PATH order, explicit paths and other extensions are preserved. Linux lookup receives no Windows suffix. An unsupported freshness configuration still uses safe ordinary compilation; compiler/probe errors are not masked by a blanket fallback.

Windows focused: `stage3-cache-context.log` — **3 passed / 2 ignored**; `stage3-exe-public-api.log` checked the initial four variants. Fresh `stage5-windows-native-exe-matrix12.log` checks **12** cases: cargo/rustc × bare/suffixed/absolute × stored/uncached. In each case a real worker returns the expected value; a real E0425 remains a Compile error. This is a narrow check of selected tools, not a promise of complete Windows Command lookup/PATHEXT compatibility. An explicit relative `./cargo` does not automatically receive `.exe`.

<a id="4-фактический-cargo-executable-для-playerteam"></a>

## 4. Actual Cargo executable for Player/Team

Previously, the source builder always read `target/release/<player>`, although an explicit native target selects `target/<triple>/release`. On a cold cache, export rejected a successful build; with an old default-layout file present, it could select that instead of the actual result.

Player uses the same bounded Cargo collector and native executable validation as Custom. Exact canonical manifest/package/bin are selected from Cargo metadata, including a virtual workspace without a resolve root. `compiler-artifact.executable`, successful build-finished, HOST=TARGET of the selected native Rustc and PE/ELF are checked before using the product. There is no fallback to the old default filename.

The new std-only `crates/player/build.rs` publishes HOST/TARGET proof and rejects a foreign target. The file is included in the mandatory native source inventory. The source-builder lease still covers build, hash/copy and immutable-snapshot validation. For a large Player build, the collector drains dependency messages without retaining unnecessary dependency facts; Custom freshness retains its previous complete bounded tracking.

`stage4-cargo-products.log` — **9 passed**, including virtual-workspace selection, large dependency stream, stale default sentinel and invalid header. `stage5-windows-final-player-targets.log` — real Cargo on a minimal Player fixture: default/env/config native target, cold/warm rebuild, unchanged build with explicitly checked `compiler-artifact.fresh=true` and foreign-target rejection. `stage5-windows-final-player-snapshots.log` — direct competing-lease test without Cargo before acquisition, then two concurrent feature variants through the production snapshot helper; after completion, both saved executables return their own results. These tests do not replace real Full/source Player/Team export, which is performed separately.

<a id="свежая-windows-проверка"></a>

## Fresh Windows verification

`stage5-windows-workspace-tests.log`: **595 passed / 0 failed / 9 ignored**, offline locked release workspace/all-targets, one test thread. Doc-tests exit0. Original PowerShell5 logs are preserved unchanged; `*.readable.log` are UTF8 derivatives with separately decoded header/footer and UTF16 native redirection, with conversion and SHA recorded in `stage5-windows-readable-logs.json`.

The first strict Clippy run found an unnecessary borrow, `join(&key)`, in the new helper. It was corrected to `join(key)` without a behavior change. `stage5-windows-final-{fmt,clippy,build}.log` — exit0 on the corrected source; both Player native tests and five selected native tests then explicitly passed. Changes after the full suite affect only this borrow and strengthening two ignored Player tests; they were run separately. The full suite is not duplicated without a new reason.

`stage5-windows-native-stop-reuse.log`: after Game, root_artifacts_built=1; after graphics initialization, env key `=C:` appeared, and Scene has root_artifacts_built=2. After Stop/Game it remains 2, root_artifacts_fresh=1, program_hits=1: no new root compilation occurs in the unchanged subsequent context. An additional Cargo/freshness invocation is not presented as a new root compilation. Independent worker PIDs and initial state were checked. The slow UI/hot Apply test also passed. Two other ignored tests — the software-fixture generator and a separate wall-time diagnostic of slow catch-up — did not run in this pass.

Real Windows integration completed: new Full has Engine=true and modified_sources=[]; the env native target was checked with the executable initially absent, then repeated on a warm cache; a private Full `.cargo/config.toml` with a native target also exports Player and Team. Three exported Players have SHAs matching the actual current `target/x86_64-pc-windows-msvc/release/module-engine-player.exe`. The old default-layout binary has a different SHA and is unchanged. PE/machine and every package-file hash were checked. All three Players pass `--verify-package` from another cwd with restricted PATH and unavailable DXC. Team has Engine=false, source_root=null, a working SDK; its `--check`, export through the checked prebuilt Player and compiler-less verify also passed. Summary: `windows-verification-summary.json`.

The first native WGPU/DX12 Editor benchmark preserved a **red** `stage6-windows-editor-dx12/report.json`: both 90-frame series were collected, but cold SDK compilation took about 30s, and post-Stop preparation exceeded the existing 45s watchdog. No successful performance conclusion is drawn from this incomplete report. After preparing the current config, the same benchmark passed with an unchanged watchdog: `stage6-windows-editor-dx12-warm/report.json`, complete=true, measurements_valid=true, Stop actor1→2/time0, errors=[]. GUI is WGPU/DX12, NVIDIA RTX4080SUPER. Editor handler p95 stopped/play=2.89/3.94ms, callback interval p95=7.41/7.66ms; these are callback timings, not scanout measurements or Player60FPS. Game-worker diagnostic also passed. Cold startup within45s remains unproven.

All code fixes are saved in `b81557f`, `2cf04bc`, `7bcaab2`, `c30ff3b`, `566eafb`. Later commits contain documentation and inventory unless a code change is explicitly stated.

Windows GUI evidence boundary: the benchmark lacked input focus (`focused_frames=0`); the visibility API returned unknown (`visible=null`), so `visible_frames=0` proves neither a hidden nor a visible window. Foreground input-to-display and external pointer clicks are unconfirmed. Logs confirm the native WGPU/DX12 event loop, real Game/Scene frames, graph-pan changes and lifecycle; they do not replace manual visual QA/scanout.

<a id="свежая-linux-проверка"></a>

## Fresh Linux verification

Ubuntu/WSL: `linux-final-workspace-tests.log` — **604 passed / 0 failed / 8 ignored**, offline locked release workspace/all-targets. Doc-tests, fmt, strict release workspace/all-targets Clippy and Editor/Player builds — exit0 on source `566eafb`. System components and profiles were unchanged.

The first native Stop/reuse was mistakenly run through `cargo test` and failed at the scene root-count assertion; log `linux-final-stop-reuse.log` and unsuccessful `linux-final-summary.json` are preserved. The test itself prescribes a normal native loader environment. The rerun uses the exact current ELF test executables from `cargo test --no-run --message-format=json`, checking unique target, build-finished, ELF machine and SHA; actual paths/fresh fields are saved in `linux-native-rest-products.json`. It runs directly in the normal environment from existing env.sh, rather than with a Cargo-injected loader/test environment. The ordinary direct run passed without source or verification-condition changes. This does not imply a promise of identical caching behavior in every injected test environment.

`linux-native-rest-status.tsv` contains six exit0 results: actual context, native tools, Stop/reuse, slow UI/hot Apply, Player default/env/config/fresh/foreign modes and concurrent snapshots/lease. In direct Stop/reuse, root_artifacts_built=1 in Game, Scene and Stop/Game; program_hits=2, root_artifacts_fresh=2, three Cargo invocations. State and PIDs are independent. Two remaining ignored tests — fixture emitter and catch-up wall-time diagnostic — were not run.

Real Full/Player/Team/WSLg integration passed: `linux-integration-all-summary.json` all_passed=true. Before the first env-target export, the executable was absent; cold/warm env exports and a private Full config-target export were checked. Native ELF/machine62, executable bits and SHAs of every package file were checked. Four Players, including export from ready Team, have SHAs matching the current Cargo native-target product. The preexisting default-layout executable has a different SHA and remains unchanged. All four packages were moved into new folders, then checked from `/tmp` with PATH=/nonexistent, unavailable DXC and no LD_LIBRARY_PATH. The artifact manifest records paths before relocation; actual new folders have the `-relocated` suffix.

Only the new test copy was changed for this: its audio_cues were explicitly cleared because native WAV is unsupported, with removed cues and the original SHA recorded in `linux-integration-silent-fixture.json`. Authored JSON and scheduled Rust were unchanged. This is a silent fixture, not Linux audio parity. Full has Engine=true, Team has Engine=false/source_root=null; Full/Team `--check` and Team→Player work.

X11 and Wayland have complete=true/measurements_valid=true, 90 stopped/play frames each, verified Play/Stop, errors=[]. GUI and game device are Vulkan / llvmpipe (LLVM20.1.2), meaning software Vulkan in WSLg. X11 handler p95=8.56/11.04ms, callback interval p95=23.75/28.97ms; Wayland handler p95=9.27/10.77ms, callback interval p95=60.81/69.81ms. This is functional native acceptance, not achievement of 60FPS. X11 reports focused/visible=90/90 samples; Wayland focused=90, visibility unknown. Game-worker diagnostic passed. These data do not certify hardware Linux/another glibc/distribution.

<a id="что-считается-завершением"></a>

## What counts as completion

Each P2 must pass meaningful regression tests and current Windows/Linux integration checks. Stop/reuse is checked separately: an unchanged-context root is not rebuilt, and state and worker PIDs are independent. A Cargo/freshness-proof invocation is not a root compilation; Workbench acceptance is not a new desktop GUI/physical-presentation benchmark.

Commands and exit codes are recorded in the listed logs/summaries; source/branch/dirty snapshot and protected SHAs in `stage0-baseline.json`, `stage6-source-snapshot.json` and `final-verification.json`. Old 585/594 test counts and window measurements are not acceptance of the new source. Strict Player60FPS previously failed and remains open without a new corresponding measurement. Linux WSLg/llvmpipe, native audio and dialogs are identified explicitly; hardware Linux certification is outside this stage.

<a id="сборка-исходники-и-оставшиеся-ограничения"></a>

## Build, sources and remaining limitations

Verified new Windows build: `outputs/ModuleEngine-review-fixed.exe`, SHA256 `A5E4F9E85E632AF7E6CA111A040A3DF78282B12684FE1C0C671F41E2CCCA5FBC`, code commit `566eafb`. Original `outputs/ModuleEngine.exe` remains SHA `2477792D79F64A360AE7BEE34DF1BA81DEBEFB949FE6A4E6FA19EF79C5E66BD3`; its recoverable copy is `prior-ModuleEngine-17fb340.exe` in evidence. To return to the previously published editor, simply run the preserved original EXE; the previous branch/commit17fb340 is preserved too. The new build and source inventory do not require overwriting authored projects.

Additional Windows relocation: one Player created here was moved to `windows-player-relocated`, SHA preserved, repeat compiler-less verify exit0; relocation provenance is in `stage6-windows-relocation.json`. The three initial package paths in the early Windows manifest refer to the point before relocation.

The four original P2 findings are fixed within the stated scope; this does not claim the whole engine or every future case is free of defects. Open items are PropertySchema default-only asset dependencies, a separate P3 for selected composition in the Assets browser, strict Player60FPS, cold Editor preparation within45s, native Linux audio/dialogs and hardware Linux/foreground input-to-display QA. Full visual Rust editing, other render APIs and other large subsystems were not added in this pass.

All source changes were made in one Git checkout on branch `codex/postfix-p2-review-fixes`. Full/Team/Player folders are necessary export acceptance fixtures, not new development copies. Dependency targets are shared between builds. ABI, release profiles, GUI/backend selection, benchmark/watchdog/IPC limits, OS and drivers were unchanged. OS background processes of other tasks were not terminated. After final comparison, protected data matches the baseline and the tracked tree is clean.
