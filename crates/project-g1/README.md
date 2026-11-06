# Crate Garden source package

[English](README.md) | [Русский](README.ru.md) | [Game guide](../../docs/g1-game.md)

`module-engine-g1` owns the Crate Garden game: ten authored push-crate puzzles,
campaign rules, resumable attempts, best move scores, settings, presentation and
a level-painting tool. It is independent of the legacy Puzzle and ARPG packages.

The default crate is a headless source module. Optional `ui` adds egui controls
and the immutable-source authoring palette. Neither path depends on Editor,
Player, Vulkan, a filesystem save implementation, or an audio device. Applications
install its contracts in their selected composition and provide the registered
services requested by the authored project.

The ten authored scenes form a runtime catalog after composition filtering.
The source system switches board projections inside one Core session through
existing ECS commands and presentation-reset boundaries; it does not rewrite the
author's scene blueprint. Catalog/level revisions guard restored attempts and
their complete Undo history. Witness solutions are reproducible solutions,
not claims of minimum move counts.

Normal `GameHost` prepares that catalog in a runtime clone before creating the
rules system. Authored and Team execution parameters remain authored data;
derived caches are rebuilt from the current scenes at launch. A low-level
`GameSession` caller prepares its project explicitly with this registry. See the
[shared Host contract](../../docs/shared-game-host.md).

## Shared contracts and ownership

- Core owns `GameProject`, ECS, system execution and the authored/Play boundary.
- Game Kit supplies bounded grid occupancy, undo history, campaign progression
  and named physical/button action maps. Crate Garden supplies their consumers.
- `g1.rules` owns game phases, legal movement/pushing, the current attempt and
  progress. A blocked push is not a move. Undo restores the preceding attempt
  state; restarting a level preserves unlocked gardens and best scores.
- `g1.profile/1`, `g1.saved/1` and `g1.view/1` are versioned game data. The view is
  immutable UI input, not a second gameplay owner.
- Registered audio and persistence services perform external effects after an
  accepted tick. The module requests them through neutral contracts and has no
  direct file, process or audio-device access.

A valid already-solved authored board settles on attempt activation, including
Play, selection, Continue/resume, Next and Restart. It records a zero-move result
and unlocks the successor without halting the Host or replaying victory on idle
ticks. Ordinary winning moves use that same completion path.

The source declares eight cues: `music`, `move`, `push`, `goal`, `blocked`,
`undo`, `win` and `ui`. A nonfinal goal placement emits `goal`; final victory
takes priority over goal/push/move. Snapshot decoration tints every primitive of
an occupied crate instance green while preserving shared materials, texture
ownership, authored colors and canonical state. A fresh post-Undo snapshot
restores ordinary crate appearance.

The optional registered pose projection applies small procedural static-mesh
poses from finite `GameState.time` before interpolation and content attachment.
Gardener breathing anchors the original model's sole plane, with a small local
heading sway; goal flowers turn about their local vertical axis. These source
poses use SRT instead of shear; the neutral interpolation guards are unchanged.
The early owner model covers all imported primitives, including parent transforms
and primitive-local offsets. The late decorator supplies only crate-goal tint,
so it does not apply the ambient pose twice. Invalid clocks or singular frames
retain ordinary geometry. This is procedural mesh presentation, not
skeletal animation; world/profile/history/command/service state is unchanged.
Native visual and timing checks must use the changed artifact.

## UI and editor integration

`ui::read_input_mode` translates fresh Arrow/WASD/Z/R/Escape events with the
shared `ActionMap` and `ActionState`. Repeats and held movement do not manufacture
turns. Focus loss or a gap in viewport input collection resets held state.
Screen buttons use the same declared action namespace. Level selection and volume
packets retain their values in `action_values` as well as current `values`.

`ui::draw_controls_with_status` receives Core `GameUiStatus` and returns Core
`GameUiResponse`. It displays Menu, Level Select, Settings, Playing, Pause,
Victory and the final ending. Quit is available only when the application grants
`can_close_application`; it cannot close Editor. R emits the ordinary game action
`g1.restart`, never host `GameInput.restart`. Volume drags publish one setting
change at completion. Progress reset requires a separate confirmation.

Before the first fixed step, an absent view at tick zero with no current host
fault displays `Preparing the garden`. A malformed view, a later missing view,
or a current fault retains the diagnostic card. Both states preserve the
standalone-only Quit capability. Editor Stop displays its generic author preview
without invoking this live source HUD; Editor Pause retains the Play copy's HUD.

`grid_ui::draw_tool` uses the immutable stateful module-tool callback. It paints
a local `GridDraft` and returns Core `ModuleToolResponse` with its pending status
and an optional full `GraphDocument` candidate. Core `ModuleToolContext` scopes
one cache to its registered tool, independently of pane IDs. Successful project
adoption creates fresh state even for identical game data; canceled/failed
operations and pane close/reopen preserve the draft. Accepted Apply (including
no-op) and explicit Discard are acknowledged through its revision, so Undo before
the next redraw does not resurrect an accepted edit. A clean cache follows
external Undo/Redo; a dirty cache preserves its stale draft until resolution.
The owning Editor validates permissions, Play state, resources and the whole
candidate, then owns publication, history and Save. A stale draft cannot overwrite
new source data. The tool never changes the input document or writes files.

Pending palette edits participate in Editor's unsaved guard. Apply or Discard
before Save/Save As, Check/export, Play or a project transition. The generic
per-tool `Discard module draft` remains available even for a closed pane or
inactive owner. Rejection preserves the draft; resolving it does not clear
other tools' or Rust/JSON/execution drafts. Draft state is not crash recovery.

Public `GridDraft::into_project` preserves the execution graph, other scenes and
unrelated objects/data. Intermediate paint drafts may have unmatched crates and
goals; publication requires a valid complete board. Goals can remain under
crates. Floor/Erase removes tile contents while preserving the unique gardener.
Each completed Apply is one Editor Undo boundary.

## Checks and limits

The package contains domain and synthetic egui interaction tests. Run
`cargo test -p module-engine-g1 --features ui` in the configured workspace.
Synthetic clicks cover source actions and application-close capability; they do
not certify physical foreground interaction, native audio, exported packages,
frame timing or memory use. Native and release acceptance belongs to the
source-bound delivery report; this README does not substitute for that evidence.
`Editor::register_stateful_module_tool` supplies Core
`ModuleToolContext { storage_id, document_instance, revision }` to this callback;
`ModuleToolResponse { candidate, dirty }` exposes pending work without publication
or IO capability. Legacy `register_module_tool` remains available for immediate
proposals/read-only tools; it cannot report private pending work.

The [1 November verification report](../../docs/g1-verification-20261101.md)
records actual foreground Editor/Player routes, saved continuation/Undo/recovery,
corrected native audio and the 1080p Playing budget on exact retained artifacts.
Source16's warning panel covers expanded diagnostics and bounds long details
inside the viewport. Earlier October results keep their historical scope; final
Goal2 closure follows the report's independent decision. Device completions alone
are neither a real-time cadence test nor a physical listening claim; Menu
timing is separate from the completed Playing route. Windows evidence does not
certify clean-machine installation, hardware Linux or mobile platforms.
