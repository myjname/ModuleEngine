<a id="текущее-состояние-moduleengine"></a>

# Current ModuleEngine Status

[English](current-status.md) | [Русский](ru/current-status.md) | [Documentation](README.md)

Summary as of 18 October 2026: current async runtime evidence is separately bound to Windows08/LinuxL01 in the new report; earlier results retain their own source/artifact snapshots. This is an index of current contracts and accumulated evidence, not a new acceptance of HEAD or a finished EXE. Test counts and measurements apply only to the snapshot of each report.

<a id="цели-и-границы"></a>

## Goals and Boundaries

The PR preparation also closes a Full-source completeness gap: `crates/render-vulkan/src/retirement.rs` is now mandatory in the source inventory. Omitting it rejects Engine authoring; the source-omission regression covers this additional packaging guard separately from the historical async runtime measurements.

Workflow and personal-layout tests now use [isolated, normally validated source installations](verification.md#isolated-source-fixtures), so a stale checkout manifest cannot stop their CI scenarios before the intended assertions. This changes test setup; runtime acceptance remains bound to the reports below.

Shared modules and typed contracts must work independently of genre and specific examples. Authored data is separate from Play; rejecting a change preserves the working version and the user's work. Engine owns the architecture; Project changes what is permitted. Player is a separate native distribution with explicitly selected features, without Editor, Rust/Cargo/DXC on the player's machine.

The accepted long-term goal is a [portfolio of 5–10 distinct finished games](portfolio-roadmap.md)
covering 2D/2.5D/spatial 3D, pixel/polygonal/realistic/mobile-style graphics and
different mechanics. Mobile-style currently means a PC game; mobile ports are
future work, with specific platform targets still open. The roadmap proposes coverage, capability phases and acceptance
gates; its game designs are not individually approved and no portfolio entry is
accepted by writing the plan. Two Arenas and Puzzle remain technical baselines
until they meet those gates. This adds a product direction, not new runtime features.

P0 now requires an audit and stabilization of the existing version before P1:
fix all confirmed current-scope defects and measured performance problems, repeat
required checks, and pass the unchanged ARPG and cold Custom preparation criteria.
Future missing subsystems remain separate work. This is an accepted plan change;
recording it does not itself close the evidence gates.

[Decisions](decisions.md) retain the accepted scope and assumptions; [open questions](questions.md) list future options. Historical statements such as “remains the next stage” in older reports do not reopen work already completed. Skeletal animation, full PBR, navmesh, networking, and other large subsystems require a separate task and criteria.

<a id="реализовано"></a>

## Implemented

| Area | Current contract | Evidence and description |
| --- | --- | --- |
| Shared Host and ECS | Shared GameHost outside Editor; Core is neutral to ARPG, and the mutable world belongs to ECS. | [Completed stages 2–8](modular-runtime-progress.md), [Host](shared-game-host.md), [ECS](ecs-world.md). |
| Game packages and Player | ARPG is optional; Two Arenas and the independent Puzzle are separate projects. Player has a separate executable and explicitly selected native features. | [Package boundaries](optional-gameplay.md), [Player crate](../crates/player/README.md), stages 5–7 in the [log](modular-runtime-progress.md). |
| Custom after Stop | With an unchanged supported compile context, ready code is reused; the freshness proof may invoke Cargo without a new root compilation. Instance state and worker PIDs are independent. | [Compiled-program store](custom-program-store.md), [subsequent Windows/Linux verification](remaining-authoring-fixes-20261016.md). |
| Defaults and Inspector | PropertySchema asset defaults account for actual live IDs; exact JSON keys are preserved; Apply without changes preserves prefab inheritance. | [Fixes and regressions](remaining-authoring-fixes-20261016.md), code snapshot `fb63b3ddae7cad52532761932855b755c61fb2c7`. |
| Windows dialogs and Save As | The initial folder is a validated optional hint. Save As publishes the candidate before accepting it in Editor; Cancel/error preserves the original work. A recovery file is permitted when writing the current file fails. | [Dialog and transaction verification](dialog-path-fixes-20261016.md), code snapshot `0fbfb9ca74a139253b4e9d611b2b8877cfb4166d`. |
| Linux | Native Editor, Player, and Full/Team exports were verified in the existing Ubuntu/WSL2 environment; X11/Wayland windows use WSLg/llvmpipe. This is limited functional support. | [Launch requirements](linux-support.md), [authoring acceptance](remaining-authoring-fixes-20261016.md), [latest dialog stage](dialog-path-fixes-20261016.md). |

<a id="открыто-и-не-подтверждено"></a>

## Open and Unconfirmed

- **Native Linux WAV and File/Folder dialogs are absent.** A verification project without audio and CLI operations do not confirm these services; production data must not silently lose sound. See [Linux services](linux-support.md#optional-services).
- **Hardware Linux, portability across arbitrary ABIs/drivers, and manual foreground input-to-display QA are not certified.** WSLg software Vulkan and offscreen package verification cover a different scope.
- The Custom worker manages process lifetimes, but **is not a sandbox for untrusted Rust**. Explicit diagnostic GPU readback, bounded background GPU-retirement waiting, the limited visual Rust subset, and other functional boundaries remain documented in their respective contracts.

### Async GPU producer: scoped runtime gates PASS — 18 October 2026

Normal native Vulkan now submits one pending request and polls its exact ticket without ordinary producer GPU waiting or CPU pixel transfer. Four leases and actual consumer completion bound reuse; submitted camera/revision/tick/provenance and Scene owners stay attached to their pixels. Explicit diagnostics retain synchronous readback; only owned background teardown may wait up to30 s on its own fence, pinning unconfirmed resources/device on errors. Fixed 60 Hz simulation and the120 Hz presentation target are unchanged.

The [async GPU report](gpu-async-20261018.md) binds Windows08 `318ae8c06c406c959aad706ecf16efbe45e368b648d62f2ebf8176ba5316d8fe`: current27/16 GPU cases/exact images,307 ordinary/3 ignored scoped tests plus two separately executed ignored native UI tests, strict Clippy, warm whole-Editor Graph/Scene/Game+Play/Stop, selected18-file tools-free Player/relocation and AB03 all 12 processes. Editor A/B means the native ARPG window launched by the normal Editor artifact; standalone Player is separate. Accepted supplemental audits reconstruct actual UI/callback/worker series, correcting F4 helper labels without changing original reports. Median worker FPS Editor 120.0422→119.0027/Player 119.0875→120.0249; worker p95 increases0.3543/0.3731 ms. Actual UI p9514.0170→14.0375/14.0813→13.9872 ms. Unchanged scoped gates pass; no blanket no-regression or universal improvement claim.

Linux L01 `73a77fe8e06bd1a10fcc99f876c4dc7b7e612b396ab00c6c23c9942cfc142066` separately binds actual28/16 offscreen, normal Editor, strict Clippy, X11 resize1280×800→1440×900/Graph/Scene/Game/PlayStop/natural exit and ordinaryWM_DELETEempty-fixture close with all owned groups absent and no forced-cleanup success. Scoped coverage is a nonoverlapping303 ordinary/3 ignored logical union across failed v1's successful Core/Host and v2's five remaining suites, not one full-workspace run. JSON-only proof shows150 native/163 complete input maps equal Windows08; only an opaque settings entry is additionally inventoried by WSL. The initial pre-Cargo guard failure and DrvFS executable-bit fixture failure remain failed with explicit receipts. WSLg/llvmpipe is bounded functional evidence; native Linux audio/dialog, hardware/ABI/real device-loss, physical scanout and a newcold certification remain outside it.

The report links the external final source inventory and independent decision; their recorded scope controls interpretation. Runtime hashes remain Windows Editor `cebd1391…` / Linux ELF `e7ad6a75…`; the planned source-complete Full and separately retained normal EXE have their own later delivery receipt. No new whole-P0/whole-workspace acceptance is claimed. Historical17 October P0 below remains unchanged.

### Historical pre-async GPU stage — 18 October 2026

The [ME-01 receipt](../outputs/verification/p0-continuation-20261018/me01-receipt.json) maps the inherited source snapshot to the exact Editor/Player packages and recorded results. ME-01 reconciles documentation; it is not a native rerun or closure of the cold gate.

Normal native pixels remain on GPU through opaque Core leases and platform interop; four leases and actual consumer completion bound reuse. In that pre-async snapshot, producer completion waiting remained synchronous, and explicit CPU/software/Glow/PNG diagnostics retain readback. The [continuation report](p0-continuation-20261018.md) binds the restored normal Windows source `cedf173e70f531c8f1784a14ec02830a4aea905f7ff0ebd3aea5a0a07484a6cb` and its exact artifacts: first eligible fresh cold attempt **40.906 s wall / 39.9128549 s internal**, unchanged current ARPG gates (Editor 119.05/Player 119.00 FPS), Windows22/16 and separately bound current WSL Linux23/16 offscreen cases/exact comparisons, and scoped Windows authoring/export/relocated Player 120-frame evidence. This single cold attempt does not guarantee startup time; earlier failed cold attempts remain failed and their cause unknown. Diagnostic traces and admission-limit1 were removed; production default2 and original runtime remain.

In that historical run, WSLg/X11 resized Editor and clean ordinary WM_DELETE close pass. A synthetic Linux pre-submit release-failure pair verifies the expected production error and quarantine without CPU fallback; it does not simulate actual hardware loss or establish hardware Linux support. The earlier close timeout showed an Unsaved Changes migration guard in a generated fixture; failed fixture/cleanup receipts remain preserved. Native Linux audio/dialog services, physical input/scanout and arbitrary ABI/driver portability remain outside these results. The [original GPU report](gpu-presentation-20261018.md) retains its historical source15 A/B, source11 Linux and cold failures. Final source binding and independent conclusion belong to the continuation's external records; full P0 or whole-workspace acceptance is not claimed. Historical17 October P0 below is unchanged.

### P0: accepted within the documented scope — 17 October 2026

**P0 verification and independent review are complete within the documented scope.** The [final evidence report](p0-stabilization-20261017.md) binds Windows runtime `b260549403e638653b75e880598b507d7965164b11096399cb18006eadbdab2c` and Linux Scene tests `94c60e636ca77ae19609e723913a745205da950305210236a60d8033e62e8c70` to actual artifacts. Normal Editor SHA256 is `de7ae73680f0c86d1ae1b33fa7de313ecffab1589906b72551debea593e2a9e9`; own standalone Player is `27b8ca7c7486a24b7f2df8c1e0927eb61b0a52881c23b77d078f990709c47427`.

The unchanged 1080p/20-living-enemy/Playing gate passes: Editor 119.922177561 FPS, worker mean 8.338741176/p95 9.3295 ms; Player 117.082020875 FPS, worker mean 8.541021008/p95 9.2637 ms. Callback p95 is 7.5085/7.6059 ms and UI-observed unique p95 is 13.8867/14.0195 ms, separate series. Each report has 120 distinct completed presentations; origins 60/60 and 61/59 committed/interpolated, game/wall ratios 1.00535937/0.99658117, zero frame/wait errors. Focused-frame counts 145/146 with user-input frames zero do not prove held input or physical scanout latency. Simulation stays fixed at 60 Hz; eligible presentation targets 120 Hz without a guaranteed measured cadence.

The single-attempt target-cold run passes both <=45 s clocks: 37.14 s wall/36.1642576 s internal, with a new process, fresh Full03 and previously absent empty `work/pcB260-03`. Existing tools/offline registry and uncontrolled OS caches remain. Actual [native/relocation](../outputs/p0-stabilization-20261017/scene-input-fixed-arpg-relocation-01/result.json), [cold](../outputs/p0-stabilization-20261017/final-cold-editor-03/launch.json) and [FG04](../outputs/p0-stabilization-20261017/foreground-current-04/result.json) receipts preserve source/protected data.

Scene input's original 1-pass/2-fail red is retained; its corrected native set passes all 16 tests on Windows/Linux. Actual FG04 Move/one Undo/Rotate/Scale/Save changes exactly two selected transform leaves (Y rotation `0.9277253`, Y scale `1.0593712`); Editor closes with exit 0 and an explicitly author-transcribed tool receipt. Six captures have JPEG bytes despite `.png` names. Earlier scoped FG02 supports Unicode SaveAs, menus/Cancel/Assets/Play guards/history. Generic graph/DragValue drag, held keys/release, MMB/Shift+MMB, Ctrl-wheel and mid-held Escape were not exercised physically. Editor opens only for short specific UI tests and closes immediately afterward.

Applicable functional/native/package evidence, six normal CLI cases, strict Windows checks/Rustdoc, supported Full stage and scoped Linux WSLg/X11 software-Vulkan Puzzle/Team/relocated Player are recorded in the full report with each source identity. Linux functional `target_met=false` is not Linux ARPG FPS/cold/hardware/Wayland acceptance. Default coverage is a mixed-snapshot logical union: **Windows 666 ordinary/9 ignored, Linux 664/8**; the [unique-name ledger](../work/verification/p0-stabilization-20261017/composite-test-inventory-scene.json) adds only nine new Scene names/platform, not duplicate 16 passes or all-feature overlap. It is not one successful current full-workspace invocation. Failed guards/cache fixtures/race/observers remain failed reports with bounded supplements/reruns; all 33 protected files and initial user data are preserved.

The [independent final decision record](../outputs/reviews/p0-final-20261017/review.md) binds the final documentation/source snapshot separately from the unchanged native inputs. Native Linux WAV/dialog gaps, hardware/ABI support limits, physical input/scanout certification and untrusted Rust without a sandbox remain explicit limits; future portfolio systems are separate work.

<a id="как-выбирать-доказательства"></a>

## Choosing Evidence

The latest dialog-fix code snapshot is `0fbfb9ca74a139253b4e9d611b2b8877cfb4166d`: its [report](dialog-path-fixes-20261016.md) covers three changed Rust files, Editor tests, and the listed native scenarios. The full Windows/Linux workspace run from the [previous authoring stage](remaining-authoring-fixes-20261016.md) applies to `fb63b3ddae7cad52532761932855b755c61fb2c7`; its counts cannot be presented as a new full run of the current sources. The performance results in [review-fixes](review-fixes-20261016.md) remain historical; the current P0 report records later source-bound measurements.

The finished `outputs/ModuleEngine.exe`, `ModuleEngine-review-fixed.exe`, `ModuleEngine-authoring-fixed.exe`, and `ModuleEngine-dialogs-fixed.exe` correspond to different source snapshots. Choose an artifact by its report and SHA; the date of this page does not update the EXE or source inventory.

<a id="маршруты-проверок"></a>

## Verification Routes

- Windows: root [verify.ps1](../verify.ps1); the optional `-NativeBenchmarks` branch requires a desktop and an isolated measurement.
- Linux: root [verify.sh](../verify.sh); native toolchain/DXC/Vulkan requirements are in the [Linux guide](linux-support.md). The script includes offscreen checks and does not complete desktop/export acceptance.
- The [registry and stdlib launcher](verification.md) add explicit `quick`, `contracts`, `clippy`, `native`, `native-tools`, `package` profiles; `--plan`/`--list` read configuration without Cargo. Native tests run directly from the CargoJSON-selected executable with the normal loader environment. Windows-only checks are marked `skipped` on Linux. `package` checks minimal Cargo product/snapshot fixtures, not a full Full/Team/Player export.
- The scope of legacy invocations without arguments is preserved; Windows `-NativeBenchmarks` selects the actual normal Editor from CargoJSON and still does not require Python. The new launcher requires an already installed Python 3.9+ without third-party dependencies. Performance remains a separate opt-in legacy/targeted route with its previous limitations.

The launcher and its unit fixtures are implemented separately from Rust. Unit checks and planning are not a new engine/native/package acceptance; real profiles require their own reports on the current snapshot. Work and data-preservation rules: [AGENTS.md](../AGENTS.md).

Three [Inspector property checks](verification.md#inspector-property-checks) add 99 finite scenarios through real Apply/save/reopen: exact keys and null, opaque local payload, future prefab/default inheritance, and intentional local edits. This strengthens tests of existing behavior; no new dependencies or product changes were required. The sensitivity check actually detected a temporarily reintroduced bug that parsed `literal.key` as a dot path; the correct implementation was then restored byte for byte.

At this stage, `contracts` passed on Windows and Ubuntu/WSL (178 tests each; platform-specific sets differ); Windows `native`/`package` were verified separately during the routing stage. The [rollout results and boundaries](verification.md#rollout-verification-on-16-october-2026) do not replace full engine acceptance or acceptance of published EXEs.

On 17 October, `contracts` gained four existing Save As tests and three
[workflow preservation checks](verification.md#save-as-and-file-history-preservation)
with 28 finite scenarios. Windows and Ubuntu/WSL each passed 185 Rust tests on source
`editor_workflow.rs` SHA-256 `88e8f1d12cc01ce369d5e016e2107fdeb42b5706ab93078047b56b4a8a4aa1cf`.
Temporarily disabling rollback caused a real failure due to changed files;
the source was restored byte for byte, and the targeted test then passed. This strengthens tests,
without changing production behavior or Cargo dependencies; previous performance
and delivery limitations remain.

[GitHub CI](ci.md) runs separate Windows/Linux `contracts` and `clippy` checks, including
documentation PRs, and selected native/package
routes; public `myjname/ModuleEngine` requires `CI required` for `main`,
including administrators. New versions are assessed by the actual CI for their SHA,
not an old green run. A negative service PR confirmed merge blocking.

On 17 October, an explicit all-workspace/all-targets/all-features `clippy` profile
was added with warnings denied and pinned Rust release/native-target validation.
It does not add formatting, tests, or runtime checks. The profile passed on
Windows (Clippy 3.359 s; preflight 0.219 s) and Ubuntu/WSL (Clippy 27.856 s;
preflight 0.592 s), using existing warm shared caches. Reports:
[Windows](../outputs/ci-editor-builds-20261017/verification/20261017T015811Z-985d823c/result.json),
[Ubuntu/WSL](../outputs/ci-editor-builds-20261017/verification/20261017T015921Z-6ef11385/result.json).
These are not cold measurements or hardware acceptance. The
[stage-1/2 hosted run](https://github.com/myjname/ModuleEngine/actions/runs/38994338494)
passed Windows/Linux Clippy, contracts, selected native/package routes and the aggregate;
previous acceptance boundaries remain.

The [Editor diagnostics route](editor-diagnostics.md) uses a supplied normal executable,
generated fixtures and isolated Recent/layout. Its
[Windows both run](../outputs/ci-editor-builds-20261017/diagnostics/20261017T020938Z-e9e189aad466/result.json)
passed workflow and native WGPU/Dx12 capture; unsupported window-system and timeout
checks rejected their cases with preservation and owned-process cleanup evidence.
Final Windows `contracts` passed 187 Rust tests in 16 steps, Clippy passed, and the
full Python suite passed with 66 tests and two explicit platform skips; reports are
linked in the guide. The both-run native Scene pane was still preparing; separate
workflow PNGs verify the imported model, and the Puzzle capture shows its board.
Linux native capture remains unconfirmed; the stage-1/2 hosted run passed the
listed CI routes. These results do not establish Scene readiness, cold-start,
compositor responsiveness or full delivery.

[Build/verification measurements and cache policy](build-verification-performance.md)
separate target-cold compilation from repeat tests: both local `contracts` cycles
passed 187 tests with unchanged source/protected data. Whole-cycle time was
182.375/89.906 s; this does not establish hosted benefit or Editor cold-start.
The conservative CI Cargo cache preserves every verification step; actual hosted
baseline/miss/hit outcomes belong with their reports, including transfer/save costs.

[Independent review](review.md) is required for substantial changes under AGENTS.
Another agent/person examines the diff, goal, actual paths, and evidence. The stdlib
`review_snapshot.py` binds the conclusion to HEAD and tracked/untracked source bytes;
a successful comparison is not itself approval. The [template](review-template.md)
records findings, fixes, rechecks, and unverified boundaries.

## Assistant workflow

[Communication and context guidance](assistant-workflow.md), a repository debugging
skill and adaptive subagent settings were added on 17 October. New subagents default
to `gpt-6.1-sol` / `medium`; explicit launches select supported effort by task.
This does not change an active parent turn or the user's global `ultra` setting.
The [read-only Serena pilot](assistant-efficiency-20261017.md) resolved nine checked
caller references after workspace-loading configuration was corrected. Overall speed
and account-cost gains remain unmeasured; no always-on MCP registration was added.
These changes concern assistant tooling and documentation, not engine/native acceptance.
