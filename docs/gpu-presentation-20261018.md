# Native GPU presentation: normal criteria pass; cold-start gate open

[English](gpu-presentation-20261018.md) | [Русский](ru/gpu-presentation-20261018.md)

Historical GPU-stage snapshot: the original source15 normal and cold conclusions below retain their scope. The later [P0 continuation](p0-continuation-20261018.md) records the first eligible restored-normal cold PASS, current Windows/Linux cases, scoped window/authoring and synthetic fault evidence. Earlier failures remain failed; the cause of the old cold timeouts remains unknown.

**Current ordinary native requirements PASS on source15; cold ≤45 s remains OPEN. Final source binding and the independent conclusion are recorded in the external records below.** Pixels stay on GPU in normal presentation. Current Windows correctness/lifetime checks, selected Player export/relocation and all 12 normal A/B processes pass unchanged gates. This is a small source-bound measurement, not universal improvement or statistical non-inferiority. Source11's earlier p95 failure and all failed cold/shutdown attempts remain evidence. Matching old/new cold attempts miss final Stop readiness after both measured phases; their cause is not attributed specifically to GPU transport.

## Implemented contract

Core `Frame` contains either complete CPU `rgba` or an opaque `Arc<dyn GpuFrame>` with empty `rgba`. Core/Host know no graphics handles. The application explicitly supplies `PresentationTarget`; `render_presented` returns `None` when its bounded pool is busy, preserving the last image, identity, completion time, provenance, aiming camera and pending one-shot commands. Rejected targets/candidates preserve accepted state. An interop error reports diagnostics without silently enabling CPU transfer or activating an absent optional Vulkan module.

The Vulkan producer shares D3D12 resources with Windows WGPU/DX12. Linux uses dedicated OPAQUE_FD memory between separate Vulkan devices with exact device/driver UUID matching. The GUI performs copy and byte-exact premultiplication on the GPU into its own native `Rgba8Unorm` texture, retaining previous egui rounding without sRGB decoding. Four producer leases bound outstanding output. Actual consumer GPU completion controls reuse; CPU reference drops, hidden panes and skipped surfaces are not completion. Shutdown stops producers, then joins tracked GPU retirements before final teardown.

The producer's synchronous GPU-completion wait remains. There is no new game-renderer swapchain or general asynchronous multi-frame pipeline. Explicit CPU/software rendering, diagnostic Glow, CLI/PNG capture and comparison readbacks remain supported. Linux fatal ownership-release submission failure quarantines the affected lease/device until process exit; normal successful tests do not validate that exceptional path.

## Exact source and delivery

These results bind to Windows source15 `cea62e8ffdc73821e0e0242bb495dd2d97d3d58b6e18b6e98a36e47771254139` and HEAD `cbe97de7a974d08e391b4e3b21a18656d08ff6f7`:

| Actual artifact/evidence | SHA256 or binding |
| --- | --- |
| Normal release Editor, `editor-receipt-12.json` | `fd56c4b90c9944723ddb10d9ad59dd30a5a7230c58a4c8c43a9bf8e0dde93ba7` |
| Windows fixture06 executable | `4fddfa8d39cb7705a9935630302d63a16cd0e9388fc2dcc7caa527d40c0762c7` |
| Selected relocated Player, ARPG04 | `85effaa215fc9b69497df9c7900f0123fdcce33f422d63372a1b3f03ecd91ff2` |
| Linux fixture02 ELF | `56d44fda0feb0a55a8c4056d1b0239c744bc399e0faecb03fd9bfde93367d64b` |
| Actual Linux source inventory | `5b6a6ac55f407b3e1788db745c1a111a0873df6c0c87f2a74458e7b396bf7eb8` |

[Linux inventory binding](../outputs/gpu-direct-20261018/linux-source-inventory-binding-02.json) proves identical common/native inputs for the earlier source11 Windows/Linux checks; only unrelated `.claude/settings.local.json` appears additionally because Git global ignores differ. The configuration is preserved. CargoJSON binds actual native executables/features; current Full05/stage06 passes with 162 native/contract/shader inputs.

[ARPG04](../outputs/gpu-direct-20261018/arpg-relocation-04/result.json) passes all five commands: normal Editor native, supported export, relocated package verification, headless verification and standalone native. Player has exactly `arpg,audio,ui,vulkan`, 18 sealed files and exact-byte relocation. Actual Player verifies package hashes, exact locks, unchanged authored bytes and no source compilation. Its stripped environment removes compiler/tool controls and restricts PATH to Windows/System32; installed tools elsewhere remain, so this is not clean-OS acceptance.

## Pixels, lifetime and actual UI

[Windows fixture06](../outputs/gpu-direct-20261018/windows-gpu-fixture-06/result.json) and [Linux fixture02](../outputs/gpu-direct-20261018/linux-gpu-fixture-02/result.json) each pass **21 cases, including 16 comparisons with zero differing bytes and zero channel error**. Forward/Deferred cover 1920×1080 and unaligned 1919×1079; other cases cover AO/AA disabled/removed, custom final alpha, resize, rejected shader/target preservation, graph replacement, Stop-style renderer recreation, four-lease backpressure and producer/pane destruction. The new final orphan frame outlives GUI owners and is destroyed safely. Each fixture records retirement registered/joined/completed=1/1/1, unconfirmed/failed=0/0.

Normal bridge readback/upload bytes are zero; comparison readbacks are explicitly diagnostic (33,611,092 bytes each), not fallback. Source/protected/dependency/executable after-checks pass. Windows uses RTX 4080 SUPER/DX12; Linux uses Ubuntu/WSL llvmpipe Vulkan. Linux remains source11-bound rather than a source15 scheduling run. Both are offscreen, without surface, eframe paint, vsync or compositor. They do not certify hardware Linux, arbitrary drivers/ABIs, physical scanout or cross-platform image equality.

[Three normal-window shutdown probes](../outputs/gpu-direct-20261018/shutdown-probe-02/result.json) exit 0 with 120 distinct GPU-only frames each, zero normal CPU transfer and zero frame errors. They use a preserved compatible Full03 and prove the shutdown regression scope, not current package performance by themselves. [Current Host tests](../outputs/gpu-direct-20261018/scoped-tests-05-host/result.json) pass64 ordinary/one ignored, including seven deterministic phase-policy tests counted once; earlier viewport/endpoint checks remain separately source-bound. UI-only release CargoJSON has Player features `["ui"]` and no game `module-engine-vulkan` artifact (33.76 s): compile separation, not runtime certification.

[Warm Editor UI](../outputs/gpu-direct-20261018/warm-editor-probe-01/launch.json) passes in 33.2727902 s internal: real Graph/Scene/Game panes, 30 warmup+90 measured callbacks per stopped/Playing phase, 90+90 changed pans, progressing simulation and command-dispatch Play/Stop returning a new actor at time zero. Two GPU retirements are registered/joined/completed, none unconfirmed/failed. Injected native pointer moves are not manual foreground QA; warm dependencies do not prove cold startup. Texture presence is not one fresh render per callback.

## Historical source11 A/B: all pairs retained

A is retained P0 `b260…`, Editor `de7ae736…`, Player `27b8ca7c…`; B is source11 in those historical runs. [Original three pairs](../outputs/gpu-direct-20261018/native-ab-01/result.json) and [preregistered Editor extension](../outputs/gpu-direct-20261018/native-editor-phase-01/result.json) preserve six Editor/three Player pairs and all 18 process exits 0. Each run requires actual 1080p, 20 living enemies, `Playing`, unmodified game, 30 warmup, ≥120 distinct completed frames and unchanged mean/p95 ≤1000/60 ms gates. Producer completion, UI observation and callback intervals are distinct; none measures scanout.

| Product/pair | Order | FPS A→B | Worker mean/p95 A ms | Worker mean/p95 B ms | Callback p95 A→B ms | Gate |
| --- | --- | --- | --- | --- | --- | --- |
| editor 1 | A/B | 107.4506→105.0623 | 9.3066/15.4514 | 9.5182/16.7622 | 11.1785→7.5246 | A PASS; **B FAIL** |
| editor 2 | B/A | 109.6666→114.1750 | 9.1185/13.4483 | 8.7585/13.8611 | 11.5241→7.6299 | A/B PASS |
| editor 3 | A/B | 108.1341→117.0929 | 9.2478/13.4795 | 8.5402/10.1371 | 12.0031→7.7473 | A/B PASS |
| editor 4 | B/A | 120.3239→119.0131 | 8.3109/10.0909 | 8.4024/9.8741 | 8.2429→7.6704 | A/B PASS |
| editor 5 | A/B | 104.2889→120.0555 | 9.5888/16.6201 | 8.3295/9.3019 | 11.4256→7.6904 | A/B PASS |
| editor 6 | B/A | 116.0210→118.9883 | 8.6191/10.7550 | 8.4042/9.1949 | 9.5689→7.8034 | A/B PASS |
| player 1 | A/B | 100.4623→117.8345 | 9.9540/16.3913 | 8.4865/10.3285 | 7.9032→7.8867 | A/B PASS |
| player 2 | B/A | 105.9910→111.1679 | 9.4348/14.3334 | 8.9954/15.4195 | 10.6725→8.0787 | A/B PASS |
| player 3 | A/B | 106.1692→118.9422 | 9.4189/13.9826 | 8.4074/10.9116 | 8.3644→7.5711 | A/B PASS |

In that historical source11 set, only Editor B1 fails: worker and observed-completion p95 **16.7622 > 16.666666666666668 ms**. All six A Editor runs, five other B Editor runs and all Player runs pass. That combined Editor result remains **FAILED**; it is not relabeled by current source15 success. Favorable medians or subsequent successes cannot remove that row or establish statistical non-inferiority. Original-three median FPS is Editor 108.134→114.175 and Player 105.991→117.834; descriptive only. Six original candidate stderr summaries show no unconfirmed/failed retirements; old baseline has no marker, not inferred zero.

## Current source15 normal requirements — PASS

[Actual ARPG04](../outputs/gpu-direct-20261018/arpg-relocation-04/result.json) passes all five commands and four checks with unchanged native inputs/protected data. Editor: 120.0921177FPS, worker p95 8.8188 ms, callback p95 7.494 ms. Player: 115.066776FPS, worker p95 9.0583 ms, callback p95 7.6144 ms; actual maximum worker interval 28.9109 ms remains in the report, so not every frame is within budget. Each window has 120 distinct GPU-only frames, zero normal CPU pixels/errors and exit 0.

[Fixed current A/B02](../outputs/gpu-direct-20261018/native-ab-02/result.json) passes all 12processes and unchanged gates; source/artifact/package preservation passes. A remains the retained P0 baseline; B is clean source15.

| Product/pair | FPS A→B | Worker p95 A→B ms | Callback p95 A→B ms | Gate |
| --- | --- | --- | --- | --- |
| editor 1 | 119.0277→119.9778 | 9.3893→9.0601 | 8.2947→7.4191 | PASS |
| editor 2 | 119.9997→119.9514 | 8.9876→9.6275 | 7.5173→7.5728 | PASS |
| editor 3 | 120.0710→120.0002 | 9.0323→9.5804 | 7.5289→7.4151 | PASS |
| player 1 | 114.8764→120.0052 | 10.2935→9.4250 | 8.9162→7.6754 | PASS |
| player 2 | 117.8019→120.0428 | 9.9849→8.7673 | 7.7213→7.3446 | PASS |
| player 3 | 117.0103→119.9201 | 9.1828→8.7545 | 7.6059→7.3385 | PASS |

Editor FPS is approximately 120-capped, not a uniform gain: B2/B3 worker p95 rises by 0.6399/0.5481 ms, and B2 callback p95 by 0.0555 ms; all remain below 16.666667 ms. Player improves in these three pairs. Median-of-three per-run mean costs A→B are: renderer-owner interval Editor 2.6235→1.1468ms/Player 2.6115→0.9637ms; conversion/enqueue 2.7147→0.1602/2.7072→0.1357ms; Game-view UI handler 3.1850→0.3000/3.1993→0.2705ms; framework CPU 4.5298→0.6792/4.5700→0.5886ms. These actual component series support lower observed UI CPU cost; render-owner time includes rendering/waits, fields overlap and must not be summed. These elapsed UI-weighted observations are not independent worker samples, pure CPU instruction/GPU execution time or whole-Editor UI latency. Editor here is the Editor executable’s Player-window mode; the separate warm multipane run covers Graph/Scene/Game. Small samples establish these agreed gates on these artifacts, not universal improvement/statistical non-inferiority.

## Cold ≤45 s remains open

Historical status at this snapshot; the later exact normal-attempt result is in the [continuation](p0-continuation-20261018.md). Original failed rows below are unchanged.

Every cold run uses a new process, supported fresh Full and previously absent Custom target, retaining existing tools/offline registry and uncontrolled OS caches. Both process wall and internal elapsed clocks must pass. No cache was cleared to obtain acceptance.

| Preserved attempt | Wall / internal seconds | Actual result |
| --- | --- | --- |
| [Cold01](../outputs/gpu-direct-20261018/cold-editor-01/launch.json) | 46.140 /45.002885 | FAILED: Script setup `Access is denied. (os error 5)`, before Scene GPU-target attachment |
| [Cold02](../outputs/gpu-direct-20261018/cold-editor-02/launch.json) | 46.172 /45.005879 | FAILED: no errors; both 90 phases complete, final Stop replacement Game preparation still in flight |
| [Matched retained A](../outputs/gpu-direct-20261018/cold-baseline-01/launch.json) | 46.078 /45.002080 | FAILED: both 90 phases complete, final Stop replacement not ready |
| [Matched source11 B03](../outputs/gpu-direct-20261018/cold-editor-03/launch.json) | 46.125 /45.001474 | FAILED: same final Stop boundary, no errors |

Cold02 and matched A/B complete both 30+90 phases and immediate Stop pause/owner release, but not the new stopped actor/readiness. Cold02 records build 20.430732/proof 24.675499 s; historical P0 records 14.289326/22.186638 s and internal 36.164258 s. Stage counters are not wall time. Matched old/new failures prevent attributing today's miss specifically to GPU transport; they do not satisfy the gate. `ScriptCompiler::make_command` already sets child `CARGO_INCREMENTAL=0`, so parent-variable differences are not a demonstrated cause.

## Diagnosis and still-provisional scheduling work

[Source12 trace](../outputs/gpu-direct-20261018/timing-trace-01/result.json), diagnostic source `4a6cdc9b…`, brackets a committed publication's retained availability at about **3.4458 ms between UI lookups**: a due presentation was postponed by step-first work, published, then replaced before the next lookup. This demonstrates the observed scheduling/capture mechanism, not scanout latency or equivalence of every missed frame. Trace instrumentation changes the source/artifact and is not shipping evidence.

[Source13 control](../outputs/gpu-direct-20261018/rephase-trace-01/result.json) uses free initial phases, Off/On/On/Off. All four runs pass, but no benefit is demonstrated; unconditional/provisional rephasing must not ship from these results. [Controlled source14](../outputs/gpu-direct-20261018/collision-rephase-trace-01/result.json), source `db0b5c13b4aa79475b0cf257e29fd0a690a603e410bc8b315f66217a06ecc2f1`, runs exactly Off/On/On/Off under the same adversarial initial phase. Its diagnostic result passes; the raw envelope retains FAILED because OFF1/4 fail unchanged strict gates, while ON2/3 pass. This isolates a mechanism under that fixture, not universal shipping performance.

| Controlled mode/run | Unobserved measured-window publications | Committed retained median ms | Render deadline relative to step | Strict gate |
| --- | --- | --- | --- | --- |
| OFF1 | 21 | 4.9582 | All 70 already due before step | FAIL |
| ON2 | 0 | 8.25445 | 60 after step | PASS |
| ON3 | 2 | 8.2134 | 59 after step, 2 inside | PASS |
| OFF4 | 23 | 4.63245 | All 72 already due before step | FAIL |

The clean source15 implementation uses small `PresentationPhase` state. It observes only a successful advancing step that reaches/crosses the actual render deadline. A valid publication with matching epoch/current tick and finite nonnegative phase below the existing 8.333333 ms interval may shift the next deadline once to completion+interval. Noncolliding ticks keep the absolute grid; `None`, failure, stale/missing deadline or insufficient headroom cannot shift it. Only existing grid resets rearm it: Play/resume, actual valid viewport change, accepted apply/invalidate, successful playing Restart; Pause disables it. No simulation reset or frame/camera/provenance change is added. Seven deterministic tests cover these guards and preservation. All trace/environment/forced-phase fixture code is removed; diagnostic source14 is not the production fixture.

## Preserved failures, protections and final decision

| Earlier evidence | Boundary retained |
| --- | --- |
| [ARPG01](../outputs/gpu-direct-20261018/arpg-relocation-01/result.json) | Actual D3D12 import-size error; Windows wrongly compared a Vulkan requirement that has no D3D12_RESOURCE meaning; Linux size checks retained |
| [ARPG02](../outputs/gpu-direct-20261018/arpg-relocation-02/result.json), crash/shutdown probes | Complete timing reports followed by exit `0xC0000005`; never performance passes |
| [Lifetime trace](../outputs/gpu-direct-20261018/shutdown-trace-probe-01/result.json) | Raw D3D12 resource `Release` followed last device-pin destruction; native-before-device field order fixes source11 scope |
| Helper preflights fixture01/stage01 | Wrong release-profile/receipt target; no native command selected/executed; not product failures |

The lifetime diagnosis combines actual destruction trace, source order and independent review with [Microsoft's D3D12 weak-device lifetime contract](https://learn.microsoft.com/en-us/windows/win32/direct3d12/important-changes-from-directx-11-to-directx-12). Instrumented binaries remain diagnostic. Original failed reports/logs stay intact.

The unchanged protected33, authored JSON/assets/layout/Recent/SDK data, unrelated configuration and published EXEs are preserved by source/byte after-checks. No unrelated subsystem/dependency, renderer backend switch or wider Linux support is accepted. Optional direct Vulkan interop bindings reuse already pinned ash 0.38/windows 0.62 versions under the selected Vulkan feature. Historical P0 acceptance remains tied to its own snapshots.

Final source binding and the independent conclusion are recorded in the [final source inventory](../outputs/gpu-direct-20261018/final-source.json) and [independent decision](../outputs/reviews/gpu-direct-20261018/review.md). Completed native results retain their source15 build identities; later Markdown-only Full06 must prove identical native inputs rather than relabel receipts. Cold ≤45 s stays OPEN. Preserve prior failed runs; no universal, all-frame maximum, whole-workspace or cold acceptance is claimed.
