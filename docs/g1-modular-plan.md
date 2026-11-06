# G1: reusable modules and a finished game

[English](g1-modular-plan.md) | [Русский](ru/g1-modular-plan.md) | [Documentation](README.md)

## Agreement and status

The user approved implementation after the twelve-question G1 interview on 26 October 2026. Work has two sequential goals: first prepare and independently review reusable engine modules; then build and independently review the finished game using them. This document records the agreement and implementation boundaries, not a completed feature or acceptance result. Current source work is on `TechArt/g1-foundation`; no publication is authorized by this task.

G1 is a Windows-first, English-language classic crate-pushing puzzle with ten manually authored levels of increasing difficulty. Several crates must reach their goals; no buttons, doors, combat or additional puzzle mechanics are required. Levels unlock in order and completed levels can be replayed. Successful moves count toward a best result, with no hard move limit. Unlimited attempt-local Undo restores positions and count; Restart resets the level. Autosave retains progress, records, settings and the current attempt including its Undo history. Continue restores that attempt.

Presentation is a simple colored 3D diorama viewed by a fixed angled camera with the whole board visible. Arrow/WASD input and four onscreen direction buttons provide equivalent actions. Undo and Restart have buttons and hotkeys. The game needs readable menus, instructions, level selection, victory/final completion feedback, sound effects and background music with separately persisted volumes. A registered editor grid-painting tool authors the levels through normal validated scene/history transactions. Public documentation remains complete in English and Russian.

## Ownership and selection

| Concern | Reusable engine contract | Game-project responsibility |
| --- | --- | --- |
| Input | Named actions, physical edge/held mapping and equivalent UI actions | Bindings, labels, puzzle action names and allowed actions in each screen |
| Grid | Coordinates, bounds, layered occupancy, atomic relocation, grid/world projection | Crate/pawn/goal types, push policy, blocked cells and solved condition |
| Undo | A serializable typed journal with no arbitrary move-count cap | Attempt snapshot and validation; which accepted operation creates a history entry |
| Campaign | Stable ordered level IDs, unlocks, replay and configurable score ordering | Ten layouts, teaching order, lower-move scoring and concrete screens |
| Saves | Explicitly installed namespaced/versioned storage, validated bootstrap and atomic publication | Profile/attempt/settings schema, migration decisions and reconstruction of gameplay data |
| Audio | Explicitly requested service, owned concurrent output, looping and independent bus gains | Original audio content, event-to-cue mappings and music selection |
| Game UI | Registered callbacks return generic actions/values; shared applications dispatch them | Menus, button arrangements, text and visual theme |
| Editor tools | Registered immutable callbacks propose edits; Editor owns validation, permissions and history | Grid palette and painting policy supplied by the game package |
| Native delivery | Composition-root metadata maps installed owners/presentations/services to native features | Selected composition, assets, settings exposed by Engine and executable game rules |

Pure modules in `module-engine-game-kit` are independently enabled Rust features, not fictitious scheduled graph nodes. They must have real behavioral consumers and omission tests. Executable systems remain registered through `SystemRegistry`; services through `HostServiceRegistry`. A helper library is not evidence that arbitrary code can execute from a root graph node. Composition roots may install specific packages; generic runtime, dispatch and export code must not branch on crate/goal/level names or game presentation IDs.

Do not introduce dummy `GameContribution` entries for utility modules: the current contribution carries the primary camera/UI contract. No universal scheduler rewrite, new GPU API, networking, mobile platform, full visual Rust, skeletal animation or full PBR is required by G1.

## Compatibility and data rules

The legacy single-level Puzzle and Two Arenas remain regression consumers. Old authored limits and data must either retain their documented meaning or undergo an explicit source-owned migration; G1's unlimited policy must not silently reinterpret unrelated files. Authored documents, runtime attempts and per-user game saves are distinct. Editor Play must not overwrite the user's shipping profile. Unknown JSON remains opaque under existing contracts.

Storage is opt-in and uses an application-chosen writable root, never an implicit write into a relocated read-only package. Missing optional services perform no IO or endpoint allocation. A corrupt, foreign or incompatible save must produce a diagnostic and preserve previous bytes; an opaque save is not a serialized executor, GPU device or Custom process. Only committed accepted gameplay may be persisted. Utility registration must not change the primary game camera or UI by incidental ordering.

## Verification and exit criteria

Goal 1 requires focused behavioral/failure tests and independent review of modules and their actual integration: a non-puzzle consumer, smaller feature compositions, explicit unsupported/missing capability rejection, UI action dispatch, editor candidate rollback and audio/save lifecycle. Tests cannot substitute a counter or mock for claimed native audio output or Player rendering. Native/GPU/build work is serialized.

Goal 2 requires all ten solvable levels, progress/unlock/replay/record/Undo/Continue flows, real mouse/keyboard and editor grid authoring, save/reopen/export, failure preservation, original included art/audio, and a relocated tools-free Player. Check installation on a clean machine before claiming that coverage; a same-machine relocated package is narrower evidence. Include real visual inspection and complete English/Russian walkthroughs. Recheck affected existing consumers after shared changes.

The agreed Player target is 1920×1080 on the recorded current PC with both mean frame interval and p95 at most 16.67 ms, including the most demanding authored level, movement, Undo, UI and sound. A render callback count is not distinct presented frames. As an explicitly agent-chosen initial memory budget, peak working set must remain within 1 GiB during the reproducible G1 play route; measure rather than infer it. Record startup, package size, device/driver, source digest and artifact hashes. This document does not certify those budgets.

Each goal closes only after a source-bound independent conclusion and repair/recheck of confirmed in-scope issues. Keep failed attempts and platform limits visible. Final delivery includes a work report and an engine assessment: reused modules, project-specific code, real composition evidence, friction, strengths, missing capabilities and remaining unverified environments. Refactoring is driven by observed contract problems, not by removing every old line indiscriminately.
