# Crate Garden

[English](g1-game.md) | [Русский](ru/g1-game.md) | [Documentation](README.md)

Crate Garden is a small PC puzzle game about finding a home for every crate.
Push wooden crates onto flower-marked goals in ten gardens, from First Seed to
Garden Master. Crates can be pushed one at a time; they cannot be pulled or
pushed through walls or another crate. There is no countdown. A blocked attempt
does not add a move, and Undo is available when a move needs another thought.

## Create, open and run

Use a verified Full or compatible Team installation containing the G1 modules. In Full, open **File → New Project**, select **Choose Crate Garden
(G1) Engine**, then **Crate Garden (G1)** and a new/empty project directory.
**Inherit current Engine** instead preserves the current architecture and must
pass the G1 template compatibility checks; it does not enable missing modules.
Team cannot select a different Engine and must use its compatible frozen composition.

In Full, the CLI alternative from the verified installation is
`ModuleEngine.exe --new-g1 crate-garden/project.json`. Open the resulting
`project.json` through **File → Open Project…**, select **Project**, open
**Windows → Game** and press **Play**. Stop returns to the authored preview;
Pause retains the live game copy. With Play stopped, open **Windows → Crate
Garden Level Palette** to paint/apply a level, then use normal **Save**.
The template copies resources through the ordinary project/asset-manifest route.

For standalone play, run the retained selected package's `Player.exe`; the
[delivery report](g1-delivery-20261027.md) distinguishes the exact current package
from historical exports. To edit a retained verification project, keep its
`project.json`, resources and binding and use its associated Full Editor, rather
than an old published EXE from another installation. Creation changes no
existing authored project in place.

## Playing

The menu offers Play for a fresh start or Continue for a saved attempt. New game
starts from garden one while retaining unlocks and best scores. Choose a
Garden shows ten levels, locked gardens and best move counts. Completing one
garden unlocks the next. Restarting an attempt preserves unlocked levels and
best scores. Victory offers the next garden, replay or level selection; finishing
the last garden leads to the ending screen rather than another unfinished level.
An authored garden whose crates already occupy every goal completes when its
attempt is activated, records zero moves and unlocks its successor. Loading or
restarting that garden follows the same rule; idle frames do not repeat victory.

| Control | Action |
| --- | --- |
| Arrow keys or WASD | One move per fresh press |
| On-screen direction buttons | The same four movement actions |
| Z, including Ctrl+Z | Undo one accepted move |
| R | Restart this garden; retain campaign progress |
| Escape | Pause/resume gameplay; return from a menu page |
| Undo / Restart / Pause buttons | The corresponding keyboard actions |

The game uses a fixed view of the whole board with cream, teal and amber UI.
The Playing header shows the garden, current moves, best result and attempt
controls. Instructions remain concise and visible below the board. Keyboard
repeat and simply holding a direction do not generate extra turns.

The gardener gently breathes and sways with planted feet, while flower goal
markers turn slowly. These procedural poses use the simulation clock and the
same normal presentation in Editor Play and standalone Player; grid positions
and move counts stay unchanged.

A crate on a flower goal gains a green tint. Moving it away or undoing that push
restores its ordinary appearance. A partial goal placement has its own chime;
the final placement plays the victory sound. The eight cues cover music, move,
push, goal placement, blocked movement, Undo, victory and UI actions.

Settings has independent music and sound-effect volumes. A slider drag commits
one change when completed. Reset progress opens a separate confirmation before
clearing unlocked levels and best scores. The Quit button appears in standalone
Player only; an embedded game cannot close Editor.

## Progress and service errors

Game progress is versioned source-module data. Registered persistence stores
attempts, campaign unlocks, best move counts and settings; registered audio
handles music and sound effects. The game's source package does not write files
or open an audio device itself. Service configuration and supported platforms
belong to the delivered package.

If the save service cannot accept a file or cannot read its location, the UI
shows a persistent warning and the game can continue without overwriting it.
Open the warning for diagnostic detail. Recovery is an explicit Reset progress
operation from Settings; a replaced save is retained as a recovery copy. A
permissions or storage error can still prevent recovery or saving and remains
visible. A warning disappearing after successful recovery/save is distinct from
silently deleting or replacing damaged data.

Ordinary save-publication failures and audio-device output errors also appear in
this warning, even when the save is not write-blocked and the host has no current
simulation fault. Expanded details show the current requested service diagnostic.
The banner clears when current service diagnostics report a successful repair;
historical Activity failures do not keep it visible after recovery.

## Authoring a garden

The Crate Garden level palette is supplied by the active `g1.rules` source
module. It is available for a scheduled owner without a redundant root-graph
node or Engine switch. Stop Play before editing authored data, including a
paused Play session. The palette receives an immutable source document.

Choose a garden, adjust dimensions/cell size and paint Floor, Wall, Flower/goal,
Crate, Gardener or Erase. A goal can be painted under an existing crate.
Gardener relocates the one pawn; Floor and Erase clear tile contents. Dragging
paints a local draft, which can temporarily contain unmatched crates and goals.
Apply validates the complete board and proposes one document candidate; one
completed Apply is one Undo step. Discard draft restores the current source.
Changing the source elsewhere makes an older draft stale and blocks application.

The palette uses one stateful cache per registered tool, independently of its
pane. Successful Open/New/reload or different-file Save As starts a fresh
document instance even for identical game data; cancellation/failure and pane
close/reopen preserve the current draft. Accepted/no-op Apply and explicit
Discard acknowledge only its revision. Undo before redraw cannot resurrect an
accepted draft; clean caches follow external Undo/Redo, while dirty changes stay
stale until resolved. Rejection or callback failure preserves pending work.

Apply or Discard before Save/Save As, Check/export, Play, Open/New or Close.
Don't save cannot bypass a pending module draft. The generic `Discard module
draft` remains available for a hidden/inactive tool without clearing other
tools' or JSON/Rust/execution drafts. These guards prevent the first gameplay
tick/effects; existing stopped-preview preparation reads remain. Drafts are not
crash recovery. See the [shared tool contract](game-ui-tools.md).

Publication preserves the game execution graph, unrelated objects, other levels,
asset data and references. Resizing does not silently discard objects outside the
new bounds: repair the draft or discard it. Team uses the ordinary frozen Engine
policy. The tool has no direct Save, file or Editor-internals access; normal
Editor validation, history, Save and Save As own publication.

Existing Board/Gardener objects and unchanged cells retain their stable IDs,
prefabs, overrides and opaque attached data, including imported noncanonical
IDs. Moving the Gardener preserves its identity. New cells receive unused IDs
without taking an unrelated object's ID. Deleting an object still referenced by
authored data rejects the candidate instead of silently breaking the reference.

Changing an authored board changes its revision. An incompatible saved attempt
is diagnosed rather than silently replayed against different rules or cells;
its opaque bootstrap data remains preserved until an explicit reset. A supplied
solution witness proves the shipped original board is solvable, not that its
move count is optimal or that an arbitrary painted replacement is solvable.

## Composition and evidence

The [source package](../crates/project-g1/README.md) owns gameplay, content/UI and
the authoring palette. It consumes all four reusable Game Kit libraries through
real game features and neutral registered audio/persistence services. Core,
Host, Editor and Player do not interpret Crate Garden action names. The legacy
Puzzle remains a separate technical baseline.

Domain and synthetic egui tests cover source behavior. Actual normal Windows
Full/Team/selected Player checks verify all ten levels, saved continuation/Undo,
recovery, selected resources and relocation. Real Editor input covers painting,
Apply/Undo/Redo, Save/reopen and Play/Stop; standalone input verifies controls,
menus, persisted settings and recovery. The user confirmed normal sound after
the common audio feeding fix. Computer Use is actual OS input injection, not a
physical human keyboard or input-to-display latency test.

The current 1920×1080 Player route includes movement, keyboard Undo, onscreen
controls, audio, UI and saves on Garden Master. Mean and p95 distinct-frame
intervals pass 16.67 ms; all source/artifact/measurement boundaries are in the
[1 November report](g1-verification-20261101.md). The [October report](g1-delivery-20261027.md)
retains historical scopes and failures; the linked independent final decision
governs acceptance. Windows evidence does not certify a clean machine, hardware
Linux, native Linux audio or phones.
