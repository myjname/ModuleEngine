# Modular runtime migration

This is an implementation record, not a claim that the planned architecture is
already available. Baseline: `b6194b385687692058542c9cc3131e806cd60d13`.
Run evidence: `outputs/modular-ecs-20261015-41613750/`.

## Order and acceptance

| Stage | Work | Required evidence | Status |
| --- | --- | --- | --- |
| 0 | Source/tool/content inventory and independent audit reproduction | SHA-256 inventory, baseline tests and repro logs | Complete |
| 1 | Hierarchical death, scene transfer, system replacement, tool doctor | Regression tests; unchanged failure candidates | Complete |
| 2 | Shared composition and component schemas | Source module defaults/Inspector/validation/save/runtime/export share contracts | Complete |
| 3 | Canonical ECS gameplay ownership | Dynamic component queries, stable IDs, hierarchy and complete rollback; DTOs are snapshots | Complete |
| 4 | Optional ARPG module | No combat contracts or presentation required by core execution | Complete |
| 5 | Independent turn-based 3D puzzle | Empty authoring, edit/save/reopen/run/export without ARPG | Complete |
| 6 | Common host outside editor | Editor/Player use composition, headless has no GPU, omitted audio has no service | Pending |
| 7 | Profiles, exact versions, separate Player | Safe checked migration and relocated exports for both games | Pending |
| 8 | Acceptance, performance and author documentation | Serial tests, isolated UI/worker/export checks and scoped performance reports | Pending |

Each stage must build and pass its relevant checks before dependent stages.
Module dependencies do not change the authored execution order. The registry is
a shared installation context, not one scheduler for data, simulation and GPU
graphs. Existing GameWorker, candidate publication and authoring/Play separation
remain boundaries throughout the migration.

## Baseline findings

The independent audit and source review agree on three reproducible runtime
defects: deleting a dead hierarchy can index an already removed child; persistent
scene transfer keeps the source parent; replacement skips initialization of a
new implementation. The doctor also ignores missing required Rust components
when the compiler version matches. The current host has these tools installed;
the doctor defect is not evidence of a missing tool on this machine.

Baseline core tests: 49 passed, zero ignored. The existing independent audit
records 309 workspace library/binary tests, one separately run slow UI test and
Clippy success for this exact commit and executable. Its reported native Player
performance criterion remains **not met**; mean FPS does not establish frame
stability. No clean-machine or physical-display validation is claimed.

At baseline, the `GameState.entities` map owned gameplay mutations; the selected world
provider was synchronized from it. That ECS provider therefore did not
make gameplay ECS based. The baseline Player package copied the editor binary.
ECS ownership is addressed in stage 3 below; the separate Player remains stage 7.

## Stage 1 result

Health now snapshots death metadata before subtree removal, emits a death for
each dead entity once and permits a removed/revived ID to die again. Persistent
transfer uses the destination hierarchy, transform and references; every world
factory candidate must load the exact supplied objects before replacing the old
scene. Failed stop hooks cannot change the retained visible scene/events, though
private native lifecycle effects remain module-owned.

System replacement prepares a candidate, checks its execution/component
contract, initializes it at its authored schedule position and preserves the
explicit retained/migrated state. Initialization outputs join the host tick
transaction. The documented boundary distinguishes candidate rejection from an
ordinary failed tick after activation. Scene transitions/restarts still create
instances from the installed registry; per-instance replacement does not update
that registry.

Doctor checks Cargo, rustfmt, cargo-fmt, cargo-clippy and clippy-driver separately
from rustc, and restores missing components even when rustc matches. Portable
environment selection reads the manifest.

Checks: 63 core tests passed (49 baseline + 14 regressions); workspace/all-targets
Clippy with warnings denied passed. Twelve mocked bootstrap scenarios passed in
PowerShell 7 and Windows PowerShell 5.1; the real read-only doctor passed. Logs
are in the run directory under `stage1-*`. The authored game file hash is
unchanged. These checks do not replace the final end-to-end acceptance stage.

## Preservation rules

Unknown and inactive author data remain available; runtime/export receive only
the selected composition. Persistent object IDs must not become storage handles.
Failed code, schema, world or package candidates retain the last accepted state.
The authored `game-project.json` (including removed AA/AO passes) is not silently
rewritten. Full/Team authoring policy remains explicit and is not a native-code
security sandbox. No project copy is used as source version control; Git records
accepted stages.

## Stage 2 result

The native composition now shares root/system registries, component schemas,
implementation versions and derived activity. Schema owners contribute data
without being scheduled as systems; their availability dependencies and pins are
checked separately from authored execution order. Root-only schema owners share
their existing implementation version with system metadata.

Inspector creates fields/defaults from registered schemas, including typed object
and asset references. Prefab overrides support namespaced component IDs and
numeric array paths. Active values are validated before runtime publication;
unknown/inactive author records remain archived. A headless composition does not
activate mesh/material merely because a renderer is installed or enabled.

`example.spin` is registered once from a separate source file. Actual Inspector
Apply, save/reopen, execution and Full export checks verify it without a named
panel branch. Export collects registered resource fields, including defaults;
inactive raw mesh/audio records do not drive the active-content collector.

AssetWorkspace accepts a host validation callback before file publication. Its
standalone default checks structure only; the editor supplies the shared
composition. Required source semantic data and rejected import policies are
tested before file/trash/journal changes. Builtin defaults may deliberately make
a detached field valid again; tests use explicit required source contracts for
the rejection case.

Checks across scoped runs: core 83, editor 190, content 4, visual Rust 15,
project-tools 28, script 8 and Vulkan 22 passed. The 11 editor workflow cases were
rerun after callback integration. Workspace/all-targets Clippy passed. Earlier
full-run failures (unfinished graph editing, preserving incompatible author pins,
and source-specific file preflight) led to fixes; original logs are retained.
The two ignored wall-clock/isolated-UI cases are not included in these counts.
Final end-to-end checks and performance remain stage 8. Persisted component-data
version locks/migrations remain stage 7; registered schema versions alone are not
claimed as that feature. The authored game file is unchanged.

## Stage 3 result

`GameState.world` is the canonical ECS. Transform, hierarchy, color, references
and dynamic components share that owner; native systems use membership queries
and columns. Rust receives a boundary DTO and applies commands to an ECS
candidate. Default new game graphs select ECS; a default ECS session creates no
secondary world. Object and explicitly registered third-party providers remain
validated compatibility projections and keep their private caches between ticks.

The serialized entity-map adapter preserves the old wire shape and rejects
invalid/duplicate/zero IDs. Complete checkpoints include all columns and runtime
incarnation tokens. Ordinary edits/upserts retain a token; remove/spawn, clear
and deserialization create a new incarnation. Failed publication restores the
old token. Commands, intentions, collision start positions, combat cooldowns,
hit suppression and presentation distinguish a fresh entity from a reused ID.
Recorded old action shapes keep their captured role without controlling the
replacement's preview or target queries. Legacy state/position records remain
readable. See `ecs-world.md` and `ecs-storage-decision.md`.

Checks: a complete serial workspace run passed 359 tests (core 92, editor 190,
content 4, visual Rust 15, project-tools 28, script 8, Vulkan 22). Two old fixture
assumptions about the default world were corrected; original failures remain in
the run directory. Independent code review then found more per-tick incarnation
leaks. After those fixes, all 96 core tests, seven headless combat-presentation
tests and five Player UI tests passed; workspace/all-targets Clippy passed with
warnings denied. These scoped rechecks are recorded separately from the earlier
full run. The two isolated/diagnostic tests remain excluded until stage 8.
No release performance or complete migration acceptance is claimed here. The
authored game file remains byte-identical.

## Stage 4 result

Core session state now contains the canonical world, opaque module JSON and
generic halt/explicit scene-transition controls. Input separates latest held
values, pressed action names and captured action context; event envelopes retain
the externally tagged wire shape without genre enum branches. Native systems,
ARPG schemas, frame/outcome types, queries and optional egui feedback
now live in `crates/gameplay-arpg`. The concrete Two Arenas scenes, prefab,
placements, resources and Rust controller live in `crates/example-two-arenas`.
Production ARPG does not depend on that project. Source-installed project hooks and pure
contributions connect package validation, camera/snapshot behavior, script input,
events and diagnostics to the shared composition. See `optional-gameplay.md`.

Core tests use neutral synthetic systems; ARPG owns the transferred combat
and visual regressions, with explicit test-only example data. Independent source
rules demonstrate renamed scenes, different opponent counts and a project-owned
outcome. The default boss policy no longer implicitly declares victory; the
example explicitly opts in.

Review caught and fixed two extraction regressions: multiple UI panes could lose
captured action context, and ARPG transients could clear a source module's freshly
written intent. Active package tick hooks now run inside the checkpoint before
every authored initialization/step. Scene transitions prepare the merged target,
references, provider and system constructors before committing the source tick;
rejection retains input, world, resources, events and private system state.
An error from source `stop` after a committed tick is a separate lifecycle
boundary: the destination is not published, the request remains pending, and
native teardown effects and that already committed tick are not rewound.

Checks: the final full workspace run passed 394 tests (core 82, ARPG including
UI 46, editor 187, Two Arenas 2, and unchanged supporting suites); two dedicated
diagnostic/isolated UI tests remain for stage 8. After moving the last example
document constructor, scoped project and host checks supplement that full run.
Workspace/all-targets Clippy passed with warnings denied. Logs are under
`stage4-*` in the run directory. No release performance claim is made.

The editor still installs ARPG, `GameHost` remains editor-owned and
constructs Vulkan, and Player is not yet an independently built profile that can
omit ARPG. Those are later-stage requirements, not results of this extraction.

## Stage 5 result

`crates/project-puzzle` is a separate project module built from
`GameProject::empty()`. Its board/cell schemas, pressed direction actions,
turn/result resources, outcome and UI use the public source contracts. Its
default build has no ARPG, Vulkan, egui or audio dependency; optional UI adds
egui. Fifteen package/UI regressions passed. The editor installs this package
through the same composition and presentation registries.

The real Inspector numeric drag and Apply, Save As, save/reopen, Object/ECS
simulation, failure/restart and relocated Full-export/Vulkan workflow passed.
Both world adapters and the exported package produced identical solved state
and RGBA. The check leaves source and saved author bytes unchanged and writes
its PNGs/state/report to `stage5-workflow-final/` in the run directory.

Independent review found a late source node could publish grid overlap and a
restored counter could overflow. Package state guards now run before all public
session publication paths, including direct commands and carry-merged scene
transitions; failed candidates preserve world, lifetimes, events, resources,
private state and input retry. Counter increments are checked. Initial camera
resolves prefab data and frames the grid's configured span. No grid-specific
branch was added to core or Inspector. Grid coordinates determine starting
Play positions; the project has no dedicated grid editor.

Checks: core 85, ARPG with UI 46, Puzzle with UI 15 and the focused real
Inspector/persistence test passed. The final CLI workflow and workspace/all-targets
Clippy with warnings denied passed; a standalone default-feature Puzzle build
and dependency tree confirm its source dependency boundary. Logs are `stage5-*`.
This stage used scoped tests after the stage 4 full run; full acceptance remains
stage 8.

The current common host has not yet been extracted, and the editor executable
still contains its installed packages. An independently compiled Puzzle Player
and omitted host services remain stages 6–7; package dependency isolation is not
claimed as proof of that native Player boundary.
