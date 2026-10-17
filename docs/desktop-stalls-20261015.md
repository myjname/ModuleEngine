<a id="зависания-рабочего-стола-при-запуске-moduleengine--15102026"></a>

# Desktop stalls when launching ModuleEngine — 15.10.2026

[English](desktop-stalls-20261015.md) | [Русский](ru/desktop-stalls-20261015.md) | [Documentation](README.md)

The cause was localized to the OpenGL/Glow window presentation path: the Windows DWM composition thread spends almost an entire core searching for a DX BLT event inside `OpenDxBltEvent`. Switching the GUI to WGPU/DX12 eliminates reproduction of multi-second stalls in primary desktop updates in a controlled comparison. Gameplay ECS, Host, and Vulkan were unchanged.

Branch: `codex/fix-dwm-desktop-stalls`; original HEAD: `4cee54d40469afd9d59bb101d67af0a2176e5ece`. Implementation commit: `c714e9e7521d3203c86dd2b9c7e0574099cfa2bf`; the report and source inventory were saved in the following commit. Compact numerical evidence: [desktop-stalls-20261015.json](evidence/desktop-stalls-20261015.json).

<a id="причина-и-границы-установленного"></a>

## Cause and boundaries of the findings

The original ETW `E:\ModuleEngine\outputs\independent-audit\lag-diagnostics\trace-admin-20261015-102351\engine-lag.etl` is retained: 543162368 bytes, SHA-256 `E407947D4705EDBD72E491DB84CB6AD27A67C2FAA67EE2666B02419CD41A9D01`. There are no lost events/buffers. An xperf warning about one `InvalidEvent` at 22.607578 s in another process's rundown is retained; it does not indicate widespread data loss.

DWM PID 2208, TID 3004 completed `SCHEDULE_PRESENT` in at most 2.823 ms before launch. After launch, 341.434, 2018.662, and 8123.493 ms were recorded. The last interval remained unfinished until the trace ended. Hardware VSync continued at approximately 144 Hz; the engine submitting Present does not mean frames were actually displayed.

Microsoft public PDBs were matched exactly to the GUID/age from the ETL. Stack:

```text
NtQueryDirectoryObject → OpenDxBltEvent
→ CGlobalSurfaceManager::AdvanceFrame → CComposition::PostRender
→ CComposition::ProcessComposition
```

Over 11 seconds, TID 3004 received 10.999 seconds of CPU time; 10727 of 10995 CPU samples (97.56%) pass through `OpenDxBltEvent`. `NtQueryDirectoryObject` alone took 8.706 seconds, and object-name lookup another 1.120 seconds. Disassembly of the corresponding system DLL shows that after a failed `OpenEventW("DwmDxBltEvent_…")`, DWM enumerates `\Sessions\<session>\AppContainerNamedObjects`, attempting to open the event in each subdirectory.

**Why the initial `OpenEventW` fails has not been established.** The available data cannot distinguish a missing event, an access failure, or a specific Windows/driver bug. A later directory read found 2506 objects; their origin and causal role have not been proven. The objects were not changed.

An empty eframe window without a project, GameHost, or Vulkan reproduced the problem: mean DWM load was 94.67% of one core with Glow and 8.05% with DX12, without Desktop Duplication. Gameplay rendering, ECS, and startup compilation are therefore unnecessary for reproduction. RAM/VRAM exhaustion, sustained overall CPU/GPU load, and multi-second DPC/ISR were not supported by the evidence. Causality of MPO and third-party overlays has not been established. In the original ETW, DWM remains busy after close; that trace does not prove immediate recovery.

<a id="изменение-и-сравнение"></a>

## Change and comparison

`E:\ModuleEngine\crates\player\src\ui.rs` sets the shared Editor/Player policy: Windows GUI defaults to WGPU, explicitly selecting DX12. `MODULE_ENGINE_GUI_BACKEND=glow` remains available for diagnostics. The initialized adapter is recorded separately from the requested backend. Editor integration: `E:\ModuleEngine\crates\editor\src\main.rs`; reference empty window: `E:\ModuleEngine\crates\editor\examples\desktop_presentation_probe.rs`. Dependencies were updated in `Cargo.toml`/`Cargo.lock`.

Editor supports native screenshots through `MODULE_ENGINE_UI_SCREENSHOT_TO` (`crates/editor/src/ui_capture.rs`), retaining WGPU. In this report's 15 October 2026 source snapshot, the legacy `EFRAME_SCREENSHOT_TO` selected Glow when no backend was specified and rejected explicitly selected WGPU. That automatic selection is historical: the current Windows/Linux GUI retains WGPU unless `MODULE_ENGINE_GUI_BACKEND=glow` is explicitly requested; otherwise legacy capture returns an error. See the [current GUI and capture policy](linux-support.md). Remove both screenshot variables for ordinary launch.

A/B ran sequentially with one Release EXE, using the same project, layout, observer settings, and RTX 4080 SUPER primary monitor at 3440×1440. Open intervals were defined by QPC from resume to the close request. Desktop Duplication retained metadata only, without the observer application reading/copying pixels.

| During the open interval | Glow, 10.001 s | WGPU/DX12, 10.014 s |
| --- | ---: | ---: |
| Maximum completed update gap | 2437.964 ms | 30.232 ms |
| Unfinished interval at last observation | ≥6406.406 ms | ≥0.063 ms |
| Maximum observed image age | 6406.406 ms | 28.367 ms |
| DWM, weighted load of one core | 94.31% | 16.09% |
| Maximum observer polling interval | 108.034 ms | 30.241 ms |

The 8283.560 ms Glow gap completed at +11.801 s, after close; it is not included among completed gaps within the open interval. Pointer-only events did not reset image age. Continued polling and the absence of accumulated frames in Glow do not support an observer stall as the explanation for the multi-second interruption.

An additional ordinary window with Game view ran for 45 seconds without Desktop Duplication; mean DWM was 12.91% of a core. Computer Use did not receive access confirmation in time: no screenshots or button presses were made in that run. This is not a Play/Stop check.

<a id="проверки-воспроизведение-и-оставшиеся-ограничения"></a>

## Checks, reproduction, and remaining limitations

The Release A/B build, 3 `native_options_tests` tests, and Clippy with `-D warnings` passed:

```powershell
$env:CARGO_HOME='E:\ModuleEngine\.work\cargo-home'
cargo build --release --offline --locked -p module-engine-editor --bin module-engine-editor --example desktop_presentation_probe
cargo test --release --offline --locked -p module-engine-player --features ui native_options_tests -- --test-threads=1
cargo clippy --release --offline --locked -p module-engine-editor -p module-engine-player --all-targets -- -D warnings
```

Repeating the comparison is timer-bounded; directory names must be new:

```powershell
$diag='E:\ModuleEngine\outputs\independent-audit\lag-diagnostics\fix-investigation'
& "$diag\Run-DesktopCase.ps1" -CaseName repeat-glow-01 -Backend glow -ObserveDesktop
& "$diag\Run-DesktopCase.ps1" -CaseName repeat-dx12-01 -Backend wgpu -ObserveDesktop
& "$diag\Analyze-DesktopCases.ps1" -CaseDirectories @("$diag\repeat-glow-01","$diag\repeat-dx12-01") -OutputPath "$diag\repeat-analysis.json"
```

Full local summaries: `$diag\offline-etw-localization.json`, `desktop-case-analysis.json`, `desktop-case-claims.json`; source CSV files are retained in the case directories. The original project and both personal layouts were restored byte for byte: SHA-256 hashes match the start of diagnostics; the old `game-project.json` is also retained.

The final Editor and Player Release build passed with UI/ARPG/Puzzle/Vulkan/audio. The native benchmark in an isolated Full copy passed 90 stopped and 90 gameplay frames: simulation advanced, the Stop command immediately released the Game owner, and ordinary Game-panel preparation then created a stopped worker with `actor_id=2` instead of `1`, at time `0`. `play_stop.verified=true`, `measurements_valid=true`, no errors; the authored document and file did not change. This checks the shared toolbar dispatcher rather than an external click. During the process's 17.87 s lifetime, the maximum desktop update gap was 35.586 ms and weighted DWM was 21.39% of a core. Sources: `$diag\native-editor\report.json` and `native-desktop-analysis.json`.

The final `E:\ModuleEngine\outputs\ModuleEngine.exe` was updated and matches `target\release\module-engine-editor.exe`: SHA-256 `A40ED305456F9AFF3EE868CC91BCF86B34BDDB0111E35C3821C5DF1FD1F0069F`. Its ordinary launch on the original project/layout without a backend override, capture, or Desktop Duplication ran for 12 s: DWM averaged 12.91% of a core, normal exit was 0, there was no forced termination, and no owned child processes remained. This is an additional CPU/cleanup check; presentation intervals without an observer were not measured here. Source: `$diag\final-exe-no-observer\summary.json`. The previous EXE is retained as `$diag\ModuleEngine.before.exe`.

A built-in WGPU screenshot was created and inspected (`$diag\dx12-game.png`): it confirms GUI capture during preparation; completed Game/Scene pixels were confirmed separately by the native benchmark. The source inventory was updated using the standard `--seal-source-project` command; Full Engine is available, `modified_sources=[]`, and diagnostics are empty. Windows GUI now requires a DX12 adapter. Existing exports and prebuilt Player are not updated automatically; their files were not changed.

No new ETW was recorded: additional privileges/UAC were not obtained. Desktop Duplication measures image updates on the selected monitor and can change presentation mode; these are not `SCHEDULE_PRESENT` durations, physical display scanout, or input latency. Limits of ordinary launch and Play/Stop checks are stated separately. Drivers, security settings, MPO, overlays, and global Windows settings were not changed.
