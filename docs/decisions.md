<a id="решения-и-рабочие-допущения"></a>

# Decisions and Working Assumptions

[English](decisions.md) | [Русский](ru/decisions.md) | [Documentation](README.md)

This log separates the accepted product scope from selected implementation details. A capability is considered implemented only together with its limitations and verification.

<a id="принято-для-02"></a>

## Accepted for 0.2

| Decision | Consequence |
| --- | --- |
| The first GPU backend for game rendering is Vulkan. | Real raster/compute passes and resources, forward/deferred graphs. Other game-rendering graphics APIs come later. |
| Custom HLSL is ordinary text for a standalone pass. | The manifest describes the stage, entry points, and resources; code is available in the project. |
| Rust Custom really compiles and applies without restarting the editor. | rustc diagnostics, a separate candidate, and preservation of the previous working version. |
| Rust controls execution through portable typed commands. | The same logic works with both worlds and a registered renderer; direct Vulkan calls from ScriptContext are not provided. |
| Visual Rust support expands gradually. | Unsupported constructs are preserved as opaque text and can compile as ordinary Rust. |
| Local undo/redo, checkpoints, diff, and three-way merge are needed now. | Conflicts are preserved explicitly; live coediting is deferred. |
| The first AA is current-frame FXAA. | Temporal AA and temporal resources are deferred. |
| Export includes files, tool versions, and restoration with downloads. | A full offline SDK is not packaged; internet access during restoration is permitted. |
| The full source project remains editable. | Role-based distributions were deferred until 0.3; the implemented Full/Team scope is described below. |

<a id="выбранные-детали-реализации"></a>

## Selected Implementation Details

- Native Vulkan presentation uses opaque Core GPU leases and explicit application-supplied targets to remove the GPU→CPU→GPU pixel roundtrip while keeping Host/Core independent of native APIs. Windows uses shared D3D12 resources with WGPU/DX12; Linux uses dedicated OPAQUE_FD memory with WGPU/Vulkan and exact device/driver UUIDs. Four producer leases and actual consumer GPU completion bound reuse. Normal native presentation admits one `RenderSubmission::Pending(ticket)` at a time, polls that exact request without waiting, and reports `Busy` without accepting work; `Ready(Frame)` supports immediate CPU/software output. Completed pixels retain their submitted metadata. Pending work, cancellation and background retirement preserve native resource ownership until real GPU completion. Interop failures preserve the last image and report an error without implicit readback fallback. Explicit CPU/software/diagnostic rendering and its synchronous readback remain available; conflicting CPU rendering is rejected while native work is pending. Cancellation drops publication without waiting; renderer destruction transfers unfinished work to an owned background retirement job with a bounded fence wait and explicit failed/unconfirmed reporting. A game-renderer swapchain and several simultaneous producer submissions remain separate work. The [async GPU stage](gpu-async-20261018.md) requires its own final source-bound evidence and independent conclusion; no acceptance follows from this decision alone. [Historical source-bound results](gpu-presentation-20261018.md) record Windows source15 21 cases/16 exact-image checks, Host 64 ordinary/one ignored, selected 18-file Player export/relocation and all 12 normal A/B processes passing unchanged gates with preserved data. Linux 21/16interop evidence remains separately bound to source11/WSL llvmpipe; it is not a current15Linux build or hardware certification. Source11 strict p95/shutdown failures remain historical failures. Small current samples confirm the agreed normal criteria, not uniform improvement, statistical non-inferiority or all-frame maximum latency. Cold ≤45 s remains OPEN; final source/documentation binding and independent conclusion are recorded separately in the report. The small observed-collision/reset-only phase policy is defined in [GameWorker](game-worker.md); diagnostic14 is not production-fixture acceptance.
- Rust compiles into a worker executable and exchanges owned JSON packets without a Rust DLL ABI. In 0.2/0.3, a worker was created on every tick. In 0.4, the process is persistent, state is explicitly declared and owned by the host; a timeout stops the worker, and the next tick restores the last accepted state. This does not isolate OS access or promise migration of globals.
- The root Cargo.lock is authoritative for worker dependencies as well. The generated worker build checks package versions and sources before an offline --locked build; its own package has the technical version 0.0.0.
- Tools are pinned in toolchain-manifest.json: Rust 1.99.0, DXC 1.9.2609.5. Doctor does not modify the system; explicit Restore installs tools into the project directory and verifies DXC hashes.
- History stores document snapshots, and merge normalizes nodes by ID and connections by input. A conflict leaves the base value in the preview document and both alternatives in the report.
- Resources use AssetId, a relative path, and SHA-256. HLSL include dependencies are exported recursively; a macro in place of an include name is rejected.
- The 0.1 software renderer is retained as a separate module.

<a id="рабочие-допущения"></a>

## Working Assumptions

The following rules were adopted by the team for implementation; the user did not select every detail individually:

1. Changing the renderer and sources preserves the world/time when the world's ID, type, and parameters match. Explicitly replacing the world resets it.
2. An invalid candidate preserves its text and the last working version, and diagnostics show its rejection.
3. Migration does not overwrite the source during loading; writing an existing file creates a backup.
4. Module IDs and their ports must remain stable. An incompatible change will require explicit migration.
5. Rust node positions can be transferred only when matching is unambiguous. Current limitations are described in Custom; full persistent AST IDs are not yet promised.

Further product decisions are in [open questions](questions.md).

<a id="принято-для-03-ответы-2428"></a>

## Accepted for 0.3 (Answers 24–28)

| Decision | Implementation and boundary |
| --- | --- |
| Active modules contribute their own typed fields/settings. | Descriptor.properties, active participation, and defaults; physical removal of all unused code from the binary comes later. |
| Object/Component fields belong to actual participants. | Selected object and attach/detach; Global/Module fields are in Project Settings. |
| Independent Blender-like Engine/Project areas. | Split/join/resize/type selection, personal persistence, separate cameras/navigation with a shared runtime. |
| Engine owns architecture and exposure. | Project changes declared ranges/ready variants/conditions; new/reordered passes are prohibited. |
| Graph/HLSL/Custom Rust hot apply is available now. | Native world/renderer/UI Rust requires a build; hot native modules come later. |
| Team never includes Engine. | Explicit package marker, frozen blueprint and gameplay whitelist, .sdk for compiled Custom. |
| Full authoring requires a complete compatible source inventory. | Version/required files/active module paths are checked; legitimate source edits are marked modified. Tool availability is separate. |

Working details: .module-engine-package.json; personal .user/<profile>/<workspace>.json outside the document; the gameplay whitelist covers only active custom.rust.update; the SDK lock is a subset of the full lock by name/version/source. Team backend pins cover the HLSL include closure and tools; ordinary game assets remain game data. The mode is policy, not an adversarial security boundary.

Historical 0.3 boundary: the inactive archive was preserved by Full/local history and excluded from runtime/Team; Networking was a schema test fixture; FXAA/off and ACES/Reinhard worked. Transform/Color in the old Object Inspector changed the preview; authored game scenes and an importer did not yet exist. In 0.4, new game scenes are saved separately; the previous windows remain as legacy. Known first-party pins from older stages are updated through an explicit compatibility list; third-party conflicts are preserved; current migration details are in [graph-format.md](graph-format.md).

<a id="принято-для-игрового-этапа-04"></a>

## Accepted for the 0.4 Game Stage

| Decision | Implementation and boundary |
| --- | --- |
| The verification game has a 3D ARPG view, WASD, and mouse controls. | Two separate arenas with a short transition: 20 small enemies, then a boss. Angled overhead camera, one melee attack toward the cursor, each enemy takes damage once per swing. |
| The boss has a telegraphed heavy attack. | Windup, a marked area, and dodging through movement; no additional phases or dash. |
| The graph defines stages and subsystem order. | Shared ExecutableSystem factories, typed resource contracts, and before/after/production checks; a new module requires no branch in the executor. Native registrations require a build. |
| Play uses a copy of the authored scene. | Pause preserves the copy; Stop destroys it; runtime spawn/death is not automatically written into the source. |
| Linked templates have separate instance overrides. | Default changes propagate; local Transform/ID/references and explicitly overridden fields are preserved. |
| Custom static models, textures, and simple materials are needed now. | glTF/GLB, PNG/JPEG, and opaque base-color on the GPU; skeletons, transparency, and full PBR are deferred. |
| Collisions block movement; attacks use areas. | Controlled bodies/rectangles, without dynamic rigid bodies or navmesh. |
| Basic audio plays in response to events. | A separate audio system and Windows WAV endpoint, one voice; spatial audio/mixer later. |
| Rust Custom preserves explicitly declared state. | Persistent worker and transactional JSON packets; compatible hot apply preserves state, while an incompatible change requires explicit reset/migration. |
| The standalone game targets Windows first. | A Player package with prebuilt workers/SPIR-V, without SDK/Rust/DXC on the player's machine. Native editor code is not yet physically excluded from the shared executable. |
| The criterion is 60 FPS, 1080p, up to 20 active enemies on the current PC and fixed content. | Processing and real window measurements publish load, mean/p95/max, and disclose scope; unit tests or renderer-only FPS do not substitute for verification of the finished game. |
| Documentation accompanies changes. | API/contracts, the purpose of each module, data, actual editor actions, checks, and boundaries are described in docs. |

Working assumption: long-term game saves are outside this stage. Linux, seamless streaming, networking, temporal AA, and physical exclusion of unused native code remain future tasks. The glTF/GLB and PNG/JPEG assumption was chosen for implementation; the user did not specify particular formats individually. [next-stage.md](next-stage.md) connects the decisions to the implemented subsystems.

## Accepted long-term portfolio goal — 17 October 2026

| User decision | Consequence |
| --- | --- |
| Develop 5–10 distinct finished games to demonstrate engine configurability. | Use several playable projects as the product-level proof of reuse; existing technical examples do not automatically satisfy finished-game acceptance. |
| Cover 2D, 2.5D and 3D, pixel art, polygonal, realistic and simple mobile-style graphics, with different genres and mechanics. | Track visual, spatial and gameplay coverage explicitly; variants of one example are insufficient. |
| Mobile-style currently concerns the game, mechanics and graphics on PC. | Mobile ports are future work, with their OS targets still to be chosen; this portfolio does not require current device ports and does not claim phone support. |
| Fix the current problems before starting development of the portfolio games. | P0 includes audit, repair of all confirmed defects and measured performance problems in existing features, and final rechecks. P1 implementation starts after P0 passes; unchanged ARPG/cold-start criteria and required checks cannot remain failed or unverified. Missing future subsystems stay in later phases. |

The [portfolio roadmap](portfolio-roadmap.md) proposes a minimum five-game matrix,
up to five additional candidates, capability dependencies and common acceptance
gates. Those exact genres, game briefs, order and budgets are planning proposals,
not additional user selections. The finite portfolio demonstrates the named
cases, not universal suitability for every possible project. Each implementation
increment still needs a bounded goal and checks; this decision does not reopen all
future subsystems in unrelated tasks or mark existing performance targets passed.

## Accepted EN-01 language scope — 22 October 2026

Before portfolio-game implementation, all engine-owned shipped Editor, Player and CLI display text is English. This includes menus, panes, tooltips, confirmations, progress, errors, schema/module display metadata, dynamic subsystem diagnostics and the display defaults of newly created templates. Public documentation retains complete separate English and Russian versions.

Existing authored names and content, paths, stable IDs, API/JSON keys and module/schema/SDK pins are preserved. Raw external compiler/OS diagnostics and OS-owned native file-dialog controls retain their original language. Unicode regression fixtures and opaque source comments are not translated. No localization framework, locale selector, dependency, format migration or runtime redesign is introduced. Existing projects are not rewritten merely to translate their authored text.

Acceptance requires a classified inventory of product text, applicable tests and actual current Editor/Player/CLI workflows, rendered-label checks and independent source/artifact-bound review. Historical P0 evidence is retained under its original scope; a translation alone does not recertify timing, hardware Linux or physical input-to-display behavior. See the [UI language contract](editor-workflow.md#ui-language).

## Accepted P0 presentation change — 17 October 2026

The user approved separating fixed 60 Hz simulation from more frequent, genuine
interpolated presentation to address the measured Player cadence failure.
Conventional interpolation between completed states adds about one fixed tick
of visual delay. It must preserve authoritative gameplay, input/event/Custom
execution, authored data and the existing mean/p95/load criteria. Additional
sub-tick frames must use changed evaluated poses or camera; repeating the same
presentation or changing counters/timestamps cannot establish a performance pass.
History requires explicit lifetime/discontinuity boundaries, including a module
signal for teleports, scene changes and accepted code/configuration replacements.
Implementation and final native verification belong to P0; this decision alone
does not close the failed gate. See the [shared Host](shared-game-host.md).
