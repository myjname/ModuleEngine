# Multi-game portfolio: goal and development roadmap

[English](portfolio-roadmap.md) | [Русский](ru/portfolio-roadmap.md) | [Documentation](README.md)

## Accepted goal and scope

The long-term product goal, accepted on 17 October 2026, is to develop **5–10
distinct, small, finished games** with ModuleEngine. Together they must cover
2D, 2.5D and genuinely spatial 3D; pixel art, polygonal/stylized graphics,
realistic graphics and a simple mobile-style presentation; different genres,
controls, game loops and mechanics. The result must demonstrate that one shared
engine can be configured and extended for substantially different projects.

The minimum portfolio is five accepted games covering every required dimension.
Games six through ten should add a new meaningful test of reuse or configuration.
The candidate genres and order below are **proposals**, not separately approved
game designs. A genre change is acceptable if the coverage matrix is preserved.
The goal is finished games with deliberately small scope; a benchmark, a rendered
scene or another skin for the same example does not count as a finished game.

The user's clarification is explicit: **mobile-style means mechanics, interface
and inexpensive graphics running on PC for this portfolio**. Mobile device ports,
touch/device integration and mobile store delivery are future work; specific
Android/iOS targets have not been selected. A PC example
does not establish phone performance or portability. Windows is the first existing
delivery target; Linux support must be stated and verified per game rather than
inferred from Windows or WSLg results.

Five to ten games can demonstrate breadth across these selected cases. They do
not prove suitability for absolutely every possible project. The stronger useful
claim is concrete: distinct projects share documented engine contracts and modules,
select different compositions and produce verified standalone deliveries.

## Starting point

The current 0.4 base already has scenes/prefabs, Assets and scene editing,
SystemRegistry execution, a canonical ECS, a neutral shared Host, Rust/HLSL
candidates, optional gameplay packages and a separate feature-selected Player.
Two Arenas and the independent Crate Puzzle provide working technical baselines.
They are starting material for portfolio games, not automatically accepted entries.
Initially G1–G10 are proposed; G1/G2 have existing functional technical baselines.
See [current status](current-status.md), [architecture](architecture.md),
[Puzzle](puzzle-project.md) and [game-stage scope](next-stage.md).

The [historical P0 report](p0-stabilization-20261017.md), [pre-async continuation](p0-continuation-20261018.md) and [async report](gpu-async-20261018.md) retain separate artifact-bound results. [Current finalization](p0-finalization-20261018.md) records complete Windows/applicable Linux ordinary, native/package, functional delivery and GPU checks, unchanged Windows ARPG/cold gates and scoped foreground observations. Earlier failures and platform/content limits remain explicit. Current P0 closure follows the [external final decision](../outputs/reviews/p0-finalization-20261018/final-review.md), not this roadmap or historical acceptance.

## Proposed minimum five games

| ID | Candidate and distinct mechanics | Presentation and coverage | Main capability to verify |
| --- | --- | --- | --- |
| G1 | Casual puzzle based on Crate Puzzle: a short level set, discrete rules, move limits, level selection and progress | Simple mobile-style graphics and UI on PC; flat board or inexpensive diorama | A finished rules-driven project with its own schemas/UI, save data and optional composition, independent of ARPG |
| G2 | Small ARPG based on Two Arenas: real-time combat, enemy/boss behavior, transitions and progression | 2.5D: gameplay constrained to a plane with an angled 3D camera; stylized polygonal graphics | Reusable action/animation/audio modules, readable feedback and the existing full Player performance criterion |
| G3 | Pixel-art 2D platformer: jumping, platforms, timing and checkpoints | Actual 2D authoring/gameplay, sprites and pixel-aligned presentation | 2D camera, sprite/atlas animation, layers, level authoring and appropriate 2D collisions without mandatory ARPG/3D component fields |
| G4 | Polygonal 3D flight arcade: a short course, obstacles, checkpoints and a time/score outcome | Spatial 3D: movement in height and depth, orientation and a following/cockpit camera | Input/camera/transform and collision contracts for real 3D movement; reuse of common content, UI and delivery |
| G5 | Short realistic first-person exploration game: goals, object interaction and a complete ending | Realistic 3D in a deliberately small authored environment | PBR/material and lighting quality, shadows, texture filtering/mipmaps and bounded content/memory cost in a playable package |

The minimum set guarantees 2D through G3, 2.5D through G2, and spatial 3D through
G4/G5. G1's final camera does not carry an unverified dimensionality claim.
Pixel art is checked in G3; polygonal graphics in G2/G4; realism in G5;
mobile-style mechanics/graphics in G1. Different cameras or post effects alone
do not establish different gameplay. Art direction and actual content are part
of visual acceptance; enabling a PBR switch does not establish realistic graphics.

## Candidate expansion to ten

| ID | Optional additional game | New question it should answer |
| --- | --- | --- |
| G6 | Turn-based tactics | Can grid/path queries, AI, selection and turn rules reuse the Host and authoring contracts? |
| G7 | A small racing game | Can vehicle movement, contacts and camera/UI become optional modules rather than ARPG changes? |
| G8 | Economy or settlement simulation | Can saves, many interacting agents and UI remain manageable at a defined, measured scale? |
| G9 | A 2D rhythm game | Can timed audio events, input and presentation meet an explicitly measured latency/timing budget? |
| G10 | A small procedural/voxel sandbox | Can a different geometry producer, chunk lifetime and resource loading fit the renderer/module contracts? |

Select these after the minimum set exposes actual gaps. Networking, ray tracing,
Nanite-like geometry, a full visual Rust language and dynamic binary plugins are
separate possible directions; none is automatically required before the first
five games. A chosen game that genuinely needs one creates its own scoped task.

## Gaps and their first consumers

| Area | Existing boundary | Planned work and first consumer |
| --- | --- | --- |
| Runtime and presentation | Native Vulkan uses asynchronous production/polling with bounded GPU leases/platform interop; simulation is 60 Hz and eligible presentation targets 120 Hz. [Current finalization](p0-finalization-20261018.md) records Windows/applicable Linux correctness and delivery, unchanged Windows ARPG/cold gates and scoped foreground observations. | Preserve the unchanged source/artifact-bound gates and recheck them in G2. P0 closure is controlled by its external final review before P1. Linux software-GPU functionality does not establish hardware/FPS parity, Linux ARPG/cold or scanout. |
| Game services | Basic input, event-driven Windows WAV with one voice and no long-term game-save contract | Reusable input actions, game UI, multi-voice audio/mixing and versioned progress saves as required by G1/G2; keep game progress separate from authored scenes and Play copies. |
| Animation and content | Static glTF/GLB and opaque base color; no skeletal animation, executed normal/roughness/metallic maps or mipmaps | Minimum character animation needed by G2; independent sprite animation for G3; material/lighting/filtering work for G5. Define content/reference quality and resource budgets before implementation. |
| 2D | Existing board rules and 3D snapshots do not establish a general sprite/2D authoring pipeline | Optional sprite, atlas, camera/layer, alpha blending/masks, tile/level and collision contracts for G3. Low-level GPU infrastructure may be shared without exposing unused 3D settings. |
| Spatial gameplay | Current action collisions use a horizontal plane and rectangular obstacles | Genuine three-axis movement, camera and obstacle contacts for G4/G5; add dynamic bodies or navigation only when the chosen mechanics require them. |
| Authoring and persistence | Single-object tools; no nested prefab hierarchy; scene/execution arrays can conflict as a whole during merge | Expand group editing, reusable templates, schema migration and ID-aware merge where the portfolio's editing workflow needs them; retain exact data and rollback guarantees. |
| Renderer/module breadth | Image/Scene3D GPU pins and a fixed scene vertex ABI; distinct root/runtime and game execution routes; native additions require a build | Extend buffer/geometry/resource-lifetime contracts when a game needs new geometry/streaming. Keep systems/renderers registered through common APIs. Do not assume a scheduler rewrite, alternate ECS or binary ABI is already necessary. |
| Delivery/platforms | Separate Player exists; clean-machine and hardware coverage are limited; Linux lacks native WAV and file dialogs | Relocated and clean-machine Player verification, dependency/resource manifests and authoring recovery. Implement Linux service parity before claiming supported games that require those services. Mobile ports remain future work. |

Detailed current boundaries: [content](content.md), [GPU/HLSL](gpu-hlsl.md),
[Custom Rust](custom-rust.md), [ECS decision](ecs-storage-decision.md),
[scene authoring](scene-assets.md) and [Linux](linux-support.md).

## Work sequence and exit criteria

| Phase | Deliverable | Exit criterion |
| --- | --- | --- |
| P0 — current-version audit and stabilization | Check existing features, register and fix all confirmed current-scope defects and measured performance problems, then repeat verification | No confirmed product defect remains open in the checked scope; the existing ARPG 1080p/20-enemy performance and cold Custom preparation criteria pass unchanged. Required functional, native/manual and delivery checks have source-bound evidence; a baseline or defect list alone is insufficient. |
| P1 — first finished games | Finish G1, then G2 in separate increments; add only the services/content each requires | Both pass the common game gate below; G2 also passes its existing performance/load criteria. G1 success never substitutes for G2. Existing examples remain regression fixtures. |
| P2 — independent 2D profile | Sprite/2D authoring modules and G3 | A complete pixel platformer can be authored, saved and exported through the shared workflow; unused action/3D schemas remain inactive. Pixel scaling and movement/collision behavior are checked in Player. |
| P3 — spatial 3D profile | G4 and the required 3D movement/camera/collision modules | Height/depth gameplay works in a complete course; its module composition and reused services are documented. Earlier games still pass after the shared changes. |
| P4 — realistic profile | G5, required material/lighting/content improvements and agreed visual references | The playable game meets its approved visual and frame/memory budgets on declared hardware, with actual imported content and a standalone package. A still image or renderer-only benchmark is insufficient. |
| P5 — portfolio acceptance and optional expansion | Reuse/configuration matrix for five games; select G6–G10 only for uncovered capabilities | All mandatory coverage is present and every counted game passes on one declared common engine release. Extra games follow the same gates rather than resetting compatibility. |

The [17 October P0 acceptance](p0-stabilization-20261017.md) remains historical. [Current finalization](p0-finalization-20261018.md) records the completed existing-feature matrix and limits; the [external final review](../outputs/reviews/p0-finalization-20261018/final-review.md) determines current source-bound acceptance. The phase criteria above are unchanged.

Performance, authoring, documentation and migration are continuous work across
these phases. P0 is a prerequisite for P1 implementation and new game-driven
subsystems; game briefs and visual references may be prepared in parallel.
No dates or effort estimates are committed before the individual game briefs
and content scope are established.

### P0 scope and completion

The user expanded P0 on 17 October 2026: **fix the current problems first**.
The scope covers all currently implemented features, including optional modules,
within their documented platform support: Editor/graphs/UI, scenes and Assets,
save/load/history, Play/Stop and input, Rust/HLSL apply and recovery, runtime and
rendering, and Full/Team/Player delivery. Check normal use, invalid input,
Cancel/failure preservation and the existing Two Arenas/Puzzle regression cases.
Missing future capabilities such as skeletal animation, a general 2D pipeline,
full PBR or mobile ports remain in their later phases; existing documented
platform limitations are not silently reclassified as supported features.

1. **Audit and reproduce.** Build a feature/check matrix and issue register from
   current contracts and known open problems. Record the trigger, expected and
   actual behavior, affected platform/artifact, evidence and acceptance check.
   Distinguish confirmed product defects, unconfirmed reports, environment
   failures and not-yet-implemented capabilities.
2. **Fix and optimize.** Fix every confirmed defect in that scope, including those
   found during rechecks. Priorities determine the order, not which defects can
   be carried into P1. Profile complete frame/startup paths and fix measured
   stalls, regressions and unmet budgets; do not optimize every costly function
   without evidence of a problem. Keep the existing ARPG mean/p95 16.667 ms at
   1080p/20 active enemies and cold Editor Custom preparation within 45 s unchanged.
3. **Recheck the actual workflows.** Verify each repair and affected regressions,
   then the complete matrix on the final source/packages. Include native/manual
   interaction and package checks where required; a green unit/profile run does
   not substitute for those cases. Update full English/Russian documentation and
   obtain independent review under the existing verification/review contracts.
4. **Close by evidence.** P0 ends only with no open confirmed current-scope product
   defects and no failed or unverified required checks, including the unchanged
   performance/startup criteria. A required case blocked by the environment stays
   open. Record remaining unconfirmed reports and future/platform limitations
   explicitly; they are not fixed results. This is acceptance of the checked
   scope and source, not a guarantee that no undiscovered bug exists. A different
   scope or waived criterion requires an explicit decision, not a silent move to P1.

## Common acceptance gate for every counted game

1. **A finished playable loop.** Player starts into an understandable game with
   controls/instructions, goals, a suitable success/failure or session outcome,
   restart/replay and readable feedback. The authored levels/content and UI form
   a small complete experience. Required progress/settings survive exit and reload.
2. **Independent delivery.** Its selected Player, project, assets, prebuilt Rust
   and shaders run from the package's own relocated root without Editor, the
   original checkout or Rust/Cargo/DXC. Required drivers/platform prerequisites
   are stated. Verify a clean machine for each claimed shipping platform.
3. **Real configuration.** Save the Engine composition/graphs, exposed Project
   controls and exact module/schema/SDK pins. Inactive settings/data/services do
   not activate implicitly. Demonstrate an alternative composition or variant
   with real behavior and clear rejection of an incompatible candidate.
4. **Shared implementation.** Record which modules are reused and which game
   rules are project packages. Native source builds and new optional modules
   are valid. A common engine evolution must have migration and regression
   evidence; an incompatible private Core fork for each game defeats the goal.
   Native-feature/dependency checks support optionality; hiding UI does not prove
   that a native dependency is absent, or that every unused instruction is stripped.
5. **Measured budgets.** Before acceptance, record target OS/CPU/GPU/driver,
   physical and internal resolution, content/load, frame and memory budgets and
   cold/warm conditions. Measure actual distinct Player frames and interaction;
   headless logic, callback FPS and offscreen rendering prove narrower properties.
   The existing ARPG mean/p95 16.667 ms criterion remains unchanged. Other games
   need explicit budgets rather than borrowed results or invented guarantees.
6. **Authoring and failure behavior.** Save/reopen, Play/Stop, permitted hot apply,
   import/reference changes and export work on that game's data. Invalid code,
   missing assets and publication errors preserve authored work/last good state
   with diagnostics. Include manual play and UI QA alongside automated checks.
7. **Bound evidence.** Each game records source HEAD/digest, package hashes,
   features/pins, commands, measured artifact/hardware, passed/failed/unverified
   checks, known limits and a reproducible walkthrough. Public docs have full
   English/Russian counterparts. A later engine change requires rechecking the
   affected games; historical green results do not certify the new release.

The final portfolio ledger must show these fields for each G1–G10 entry and mark
it proposed, in development, functional or accepted. No entry is accepted by this
planning document. Final breadth acceptance counts at least five accepted entries
on the declared common release, with every minimum visual/gameplay dimension
covered and no required per-game gate left open.

## Immediate next increment and later decisions

The changed candidate09 normal EXE has a separate actual build identity. Current-artifact CLI/cold/ARPG revalidation is PASS: [CLI 6/6](../outputs/verification/p0-finalization-20261018/windows-cli-final-09/result.json), [fresh cold](../outputs/verification/p0-finalization-20261018/cold-final-09/launch.json), [current ARPG pair](../outputs/verification/p0-finalization-20261018/arpg-final-09/result.json); source08 matrix/foreground evidence retains its scope. Final source10 build/delivery/data records and the external verdict control closure; unchanged inputs alone do not prove binary equivalence.

Use the [current P0 evidence](p0-finalization-20261018.md) and its external final decision to determine closure before P1. Retain the complete matrix, unchanged Windows ARPG/cold gates, scoped Linux/foreground coverage, preservation and all failed attempts; future changes need renewed applicable evidence.

A short G1 brief may proceed: levels/content, controls/UI, saves, visual reference and budgets. Preserve authored `game-project-v4.json` and use separate verification projects. Asynchronous GPU-resident presentation is implemented within the [documented scope](gpu-async-20261018.md); further producer/interop changes require full frame-path evidence. CPU readback remains an explicit diagnostic/capture facility.

Before each new game, confirm its brief and newly required modules. Still open:
the exact game designs/lengths/assets, which optional entries are worthwhile,
per-game performance/memory budgets, the intended Linux support matrix, and when
later mobile or network projects enter scope.
