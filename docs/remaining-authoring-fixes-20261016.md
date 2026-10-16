<a id="propertyschema-assets-и-сохранность-inspector-после-ревью-5e3f064"></a>

# PropertySchema assets and Inspector preservation after review 5e3f064

[English](remaining-authoring-fixes-20261016.md) | [Русский](ru/remaining-authoring-fixes-20261016.md) | [Documentation](README.md)

Work began from `5e3f0649d0b67bf41ddae330700385daa164f26c` on branch `codex/property-defaults-inspector-keys`. Independent reproductions: `outputs/independent-audit/four-p2-5e3f064/`; new logs and results: `outputs/remaining-authoring-fixes-5e3f064/`. This continues an audit of three specific defects, not a new acceptance of the entire engine.

<a id="исходные-ошибки-и-проверка-до-исправлений"></a>

## Original defects and checks before fixes

`red-property-defaults.log`: a real PNG is used only through `ModuleDescriptor.properties` / `PropertySchema.default`, but the usage index returns zero. The previous stage's `ComponentSchema` protection did not cover this other property mechanism.

`red-inspector-keys.log`: the real egui Apply button loses the literal JSON key `future.key` and conflates it with nested `future → key`. JSON allows both simultaneously. A registered dot-path override is not a universal address for arbitrary JSON keys.

`red-inspector-inheritance-direct.log`: Apply without edits creates a local aggregate on a prefab instance and prevents subsequent inheritance. An already built baseline test executable checks it. The initial Cargo run of the third test encountered temporarily incompatible parallel caller-API edits; its compile-error log is also saved separately and is not considered a defect reproduction.

<a id="propertyschema-и-граница-live-ids"></a>

## PropertySchema and the live-ID boundary

Property availability is determined by the same Core API as runtime: effective modules, owner, requires_modules, requires_components and legacy attachments of the specific object. The asset service then separately checks the exact owner module pin. Global and Module require no object; Object and legacy Component are resolved only for the supplied set of actual live IDs.

`AssetWorkspace::with_property_object_ids` passes a snapshot of IDs rather than creating a world. Editor takes it from the already accepted runtime. `Runtime::world_source_matches` compares the connected world source, ID, module and creation parameters without launching factories. If the author switched worlds, old IDs are not presented as IDs of the new world. Authored scene IDs and property-store keys do not substitute for runtime IDs.

Without a known ID set, potentially applicable Object/legacy Component asset schemas produce an explicit context error before a file operation. Global/Module and projects without such scoped asset contracts keep working. A library client that knows the real set must pass it explicitly; an empty set differs from an unknown one.

Authored references remain editable. References coming only from an installed default are tracked separately with provenance. Moving a path reference, Delete, Replace and Detach are rejected before committing files, project, history or trash if the operation would require changing an immutable schema. Stable `asset://ID` allows Move while preserving the ID. An explicit allowed null suppresses the default. Candidate revalidation after Detach prevents a removed dependency from being restored by a default. Project format, schema pins and installed schemas are not changed to enable an operation.

The Assets browser receives the selected backend and the same ID context as file operations. Context participates in background-scan freshness: a result for a different backend or ID set cannot be accepted as current.

<a id="inspector-точные-ключи-и-наследование"></a>

## Inspector: exact keys and inheritance

Internal JSON traversal distinguishes an exact object key from an array index. Literal `future.key`, nested `future → key`, numeric keys, empty strings, `/`, `~`, arrays and null retain their original structure. The persisted format is unchanged.

A dot override is created only for a path unambiguously representable in the existing grammar and belonging to the required component under the longest-owner rule. If an opaque edit requires local storage, the entire top-level aggregate is transferred with its exact original key and neighboring values.

Comparison against the effective prefab happens before owner-conflict handling. An unchanged inherited `foo.bar` therefore does not become a local aggregate merely because a separate `foo.bar` component is attached. A real opaque-field edit still materializes the aggregate; its neighbors and the separate component are preserved.

Each such top-level aggregate is copied once per Apply. Traversing a large opaque array does not copy the whole array again for every element.

<a id="проверки-и-ограничения"></a>

## Checks and limitations

The verified code commit is `fb63b3ddae7cad52532761932855b755c61fb2c7`. Fresh results refer to it; old test counts are not reused. Only documentation and its source inventory change afterward.

| Check | Windows | Ubuntu/WSL2 |
| --- | --- | --- |
| Offline locked release workspace/all-targets | 605 passed, 0 failed, 9 ignored | 614 passed, 0 failed, 8 ignored |
| Separately selected native parent tests | 7 passed | 6 passed |
| fmt, strict all-targets Clippy, doc-tests, Editor/Player build | exit0 | exit0 |
| Fresh Full → Player, Full → Team → Player | passed | passed |
| Relocated Player, PATH without Cargo/Rust/DXC | 2 passed | 2 passed |
| Real Editor window, 90 stopped + 90 Play samples | WGPU/DX12, passed | WGPU/Vulkan X11 and Wayland, passed |
| Game-worker: slow Custom, lifecycle, fault latch | passed | passed |

Logs: `windows-workspace-tests.log`, `linux-workspace-tests.log`, `windows-native-*.log`, `linux-native-*.log`, `windows-integration-*.log`, `linux-integration-*.log`. Summary results: `windows-verification-summary.json`, `linux-verification-summary.json`, `linux-integration-all-summary.json`. They verify each export's own immutable snapshots, hashes of all package files, SDK identity and exact contract lock. An early export need not match the latest shared Cargo executable after later builds; both saved snapshots were checked separately. Full is the source project run by the built Editor; the executable Editor is shipped with Team.

Native tests run using actual test executables from fresh Cargo JSON, with a normal loader environment. Stop/reuse on Linux: `root_artifacts_built` 1 → 1 → 1; on Windows 1 → 2 → 2 with the explicitly recorded appearance of environment key `=C:` before Scene. Subsequent Stop/Game uses a fresh artifact; state and worker PIDs are independent. Three Cargo calls for freshness proof do not mean three root compilations. The Windows tool matrix includes 12 Cargo/Rustc name/path variants and preservation of a real Rust error.

All three original defects have regression tests; Inspector is checked through the real egui Apply button with save/reopen and strict profile/lock validation. PropertySchema tests use real PNGs and file operations. For Object/legacy Component, custom Runtime provides ID17/29, different from authored Scene1/2; a factory counter confirms that scanning does not recreate the world.

Failed intermediate attempts are preserved. The first focused project run had 30 passed / 1 failed because of an invalid fixture: attachment29 had no available property. An independent Boolean property was added; the gated AssetReference remains available only to object17. The corrected test passed in both full runs. Notes `windows-verification-attempt-note.json` and `linux-collector-attempt-note.json` describe corrected report-collector assumptions, without product changes. Original logs are preserved; final checks ran afresh.

<a id="оконные-измерения-и-оставшиеся-границы"></a>

## Window measurements and remaining boundaries

Windows on RTX 4080 SUPER: the prewarmed Editor finished in 18.74 s, handler p95 was 2.33/2.59 ms, and callback interval 7.89/7.46 ms. Focus=false, visibility=null: this is not a manual foreground input-to-display check.

Linux is WSLg software Vulkan `llvmpipe (LLVM 20.1.2)`. X11/Wayland finished in 13.19/22.32 s, both complete=true, measurements_valid=true, errors=[]. X11 handler p95 was 9.18/10.10 ms, callback 25.55/27.23 ms; Wayland was 10.48/10.58 ms and 107.34/69.27 ms. X11 focus/visible were confirmed in all 90 samples of each phase; Wayland focus was confirmed, visibility unknown. These data do not confirm 60 FPS.

The Linux fixture explicitly removes audio cues only in the fresh Full because native WAV is currently unsupported. Removed cues, SHA of the capability source and original JSON are recorded in `linux-integration-audio-fixture.json`. Authored data and scheduled Rust are unchanged. Compiler-less Player verify runs offscreen960×540 with native services disabled; it does not check sound, a real presentation window or user input.

This stage excludes cold Editor within45s, strict Player60FPS, native Linux audio/dialogs, hardware Linux certification and manual foreground input-to-display QA. Their previous open status is unchanged by successful authoring regression tests.

<a id="сборка-и-сохранность"></a>

## Build and preservation

New Windows build: `outputs/ModuleEngine-authoring-fixed.exe`, SHA256 `D9DEB3C70D36432D2BD0C9AF39C8D4A910E2ABF2836D832F05E902EFC8AC195C`. Launch:

```powershell
.\outputs\ModuleEngine-authoring-fixed.exe game-project-v4.json
```

Previous `outputs/ModuleEngine.exe` (SHA2477792D…) and `outputs/ModuleEngine-review-fixed.exe` (SHAA5E4F9E8…) are preserved. Nine protected files and ignored metadata were compared against `stage0-baseline.json`. Authored projects, SDK metadata, binding and personal layouts were not replaced. No native processes of these checks remained after completion; unrelated processes were not stopped.

Work was performed in one Git checkout. Full/Team/Player folders are export acceptance fixtures, not additional development copies; dependency targets are reused. OS, drivers, security settings, ABI/SDK identity, release profiles, GUI/backend policy, watchdog and IPC limits were unchanged. Existing installed tools were used.

Repeat basic checks after loading the existing tool environment:

```text
cargo fmt --all -- --check
cargo clippy --offline --locked --release --workspace --all-targets -- -D warnings
cargo test --offline --locked --release --workspace --all-targets -- --test-threads=1
cargo test --offline --locked --release --workspace --doc
cargo build --offline --locked --release -p module-engine-editor -p module-engine-player
```

Native scenarios need real Rust/Cargo/DXC, Vulkan and the selected window system. Exact executables, commands, hashes and normal-loader environment are saved in the current evidence. API documentation: [AssetWorkspace](asset-workspace-api.md).
