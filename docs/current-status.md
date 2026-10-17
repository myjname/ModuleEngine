<a id="текущее-состояние-moduleengine"></a>

# Current ModuleEngine Status

[English](current-status.md) | [Русский](ru/current-status.md) | [Documentation](README.md)

Summary as of 17 October 2026; the source baseline for the previous documentation stage is `c64bc34ffd28419cfd851838c16a2a48c9c2fcf8`. This is an index of current contracts and accumulated evidence, not a new acceptance of HEAD or a finished EXE. Test counts and measurements apply only to the snapshot of each report.

<a id="цели-и-границы"></a>

## Goals and Boundaries

Shared modules and typed contracts must work independently of genre and specific examples. Authored data is separate from Play; rejecting a change preserves the working version and the user's work. Engine owns the architecture; Project changes what is permitted. Player is a separate native distribution with explicitly selected features, without Editor, Rust/Cargo/DXC on the player's machine.

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

- **The strict Player target of 60 FPS, 1080p, 20 active enemies was not reached** in the latest relevant measurements. Callback FPS and functional completion do not replace the mean/p95 cadence of unique game frames. See [final results](review-fixes-20261016.md).
- **The previous 45-second target for cold Editor preparation with Custom remains open.** A successful prewarmed Editor and a small Puzzle scenario do not confirm it. See [authoring-stage boundaries](remaining-authoring-fixes-20261016.md) and [dialog smoke scope](dialog-path-fixes-20261016.md).
- **Native Linux WAV and File/Folder dialogs are absent.** A verification project without audio and CLI operations do not confirm these services; production data must not silently lose sound. See [Linux services](linux-support.md#optional-services).
- **Hardware Linux, portability across arbitrary ABIs/drivers, and manual foreground input-to-display QA are not certified.** WSLg software Vulkan and offscreen package verification cover a different scope.
- The Custom worker manages process lifetimes, but **is not a sandbox for untrusted Rust**. Synchronous GPU readback, the limited visual Rust subset, and other functional boundaries remain documented in their respective contracts.

<a id="как-выбирать-доказательства"></a>

## Choosing Evidence

The latest dialog-fix code snapshot is `0fbfb9ca74a139253b4e9d611b2b8877cfb4166d`: its [report](dialog-path-fixes-20261016.md) covers three changed Rust files, Editor tests, and the listed native scenarios. The full Windows/Linux workspace run from the [previous authoring stage](remaining-authoring-fixes-20261016.md) applies to `fb63b3ddae7cad52532761932855b755c61fb2c7`; its counts cannot be presented as a new full run of the current sources. The performance status from [review-fixes](review-fixes-20261016.md) did not change as a result of subsequent successful functional checks.

The finished `outputs/ModuleEngine.exe`, `ModuleEngine-review-fixed.exe`, `ModuleEngine-authoring-fixed.exe`, and `ModuleEngine-dialogs-fixed.exe` correspond to different source snapshots. Choose an artifact by its report and SHA; the date of this page does not update the EXE or source inventory.

<a id="маршруты-проверок"></a>

## Verification Routes

- Windows: root [verify.ps1](../verify.ps1); the optional `-NativeBenchmarks` branch requires a desktop and an isolated measurement.
- Linux: root [verify.sh](../verify.sh); native toolchain/DXC/Vulkan requirements are in the [Linux guide](linux-support.md). The script includes offscreen checks and does not complete desktop/export acceptance.
- The [registry and stdlib launcher](verification.md) add explicit `quick`, `contracts`, `native`, `native-tools`, `package` profiles; `--plan`/`--list` read configuration without Cargo. Native tests run directly from the CargoJSON-selected executable with the normal loader environment. Windows-only checks are marked `skipped` on Linux. `package` checks minimal Cargo product/snapshot fixtures, not a full Full/Team/Player export.
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

[GitHub CI](ci.md) runs Windows/Linux contracts and selected native/package
routes; public `myjname/ModuleEngine` requires `CI required` for `main`,
including administrators. New versions are assessed by the actual CI for their SHA,
not an old green run. A negative service PR confirmed merge blocking.

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
