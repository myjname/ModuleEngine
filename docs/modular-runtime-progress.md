# Modular runtime migration

This is an implementation record, not a claim that the planned architecture is
already available. Baseline: `b6194b385687692058542c9cc3131e806cd60d13`.
Run evidence: `outputs/modular-ecs-20261015-41613750/`.

## Order and acceptance

| Stage | Work | Required evidence | Status |
| --- | --- | --- | --- |
| 0 | Source/tool/content inventory and independent audit reproduction | SHA-256 inventory, baseline tests and repro logs | Complete |
| 1 | Hierarchical death, scene transfer, system replacement, tool doctor | Regression tests; unchanged failure candidates | Complete |
| 2 | Shared composition and component schemas | Source module defaults/Inspector/validation/save/runtime/export share contracts | Pending |
| 3 | Canonical ECS gameplay ownership | Dynamic component queries, stable IDs, hierarchy and complete rollback; DTOs are snapshots | Pending |
| 4 | Optional ARPG module | No combat contracts or presentation required by core execution | Pending |
| 5 | Independent turn-based 3D puzzle | Empty authoring, edit/save/reopen/run/export without ARPG | Pending |
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

The current `GameState.entities` map owns gameplay mutations; the selected world
provider is synchronized from it. The present ECS provider therefore does not
make gameplay ECS based. The current Player package copies the editor binary.
Both are migration requirements, not completed features.

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
