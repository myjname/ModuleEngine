# Crate puzzle project

[English](puzzle-project.md) | [Русский](ru/puzzle-project.md) | [Documentation](README.md)

`module-engine-project-puzzle` is a small authored game built from
`GameProject::empty()`. It depends on the neutral engine contracts and optionally
egui for its controls and HUD. It has no dependency on the ARPG package or the
Two Arenas project.

The project supplies two component schemas, two scheduled systems, and a
presentation contribution. The editor installs that source package through the
shared composition catalog. The ordinary Hierarchy, Inspector, Scene, project
persistence, and runtime paths consume those registrations; their panels do not
contain puzzle-specific field editors.

## Create and play

From a complete matching Full source package:

```powershell
target/debug/module-engine-editor.exe --new-puzzle puzzle-project.json
target/debug/module-engine-editor.exe puzzle-project.json
```

The board has one pawn, one crate, walls, and a green goal. Arrow keys or WASD
make one move per fresh key press. A move into the crate pushes it if the next
cell is free. Walls block movement without consuming a turn. Push the crate
onto the goal within eight moves. A solved game or an exhausted turn limit
halts further moves. Press R or the HUD's Restart button to restore the authored
board and clear the module's turn counter and outcome.

The supplied board's solution is **North, East, South, East, North**. The game
emits named `puzzle.blocked`, `puzzle.moved`, `puzzle.solved`, and `puzzle.failed`
events through the engine's generic event contract.

## Edit the authored data

Select object 1 in scene `board`. Its `puzzle.board` component starts as `{}`;
the registered schema resolves these defaults:

| Field | Default | Constraint |
| --- | --- | --- |
| `width` | 5 | Whole number, 3–64 |
| `height` | 5 | Whole number, 3–64 |
| `cell_size` | 1 | Number, 0.1–10 |
| `max_turns` | 8 | Whole number, 1–1024 |

Use the Inspector's numeric fields and **Apply fields**. Changing `cell_size`
changes the grid's runtime projection without changing its logical solution.
Other objects have `puzzle.cell` fields `x`, `z`, and `kind` (`wall`, `goal`,
`pawn`, or `crate`). Source-owned validation checks whole coordinates, bounds,
root object ownership, unique occupied cells, and the required board/pawn/crate/
goal layout. The same package guard rejects invalid final worlds before a tick,
direct command packet or merged scene transition can publish. A late source
system cannot bypass this check. Turn increments reject overflow, and restored
frame counters must match the authored turn limit.

Grid coordinates and cell size determine starting positions when the first
Play tick initializes the source systems. Edit-mode Scene still shows authored
Transforms and supports its normal gizmos; there is no dedicated grid editor.
Moving a cell's authored Transform alone does not change its grid logic, and
Play initialization projects that cell from its `puzzle.cell` coordinates.

The authored schedule runs `puzzle.turn` before `puzzle.result`. Its declared
component activity contains `puzzle.board` and `puzzle.cell`; the renderer also
contributes neutral mesh/material schemas. Health, AI, combat, ARPG frame data,
and authored audio cues are absent. Puzzle state lives in `puzzle.frame/1` in
the generic module data map.

Save and reopen through the normal project commands. A project may keep its
resources separately from its selected Engine package; the standard Save As
binding preserves that relationship.

## Reproduce the authoring and runtime proof

```powershell
target/debug/module-engine-editor.exe --puzzle-check puzzle-project.json outputs/puzzle-check
```

This command uses the supplied file to locate its checked Engine and resource
context, creates a fresh puzzle preset in a separate case folder, and leaves
the supplied file unchanged. It performs these checks:

- Renders the real generic Inspector, drags `puzzle.board.cell_size`, clicks
  its actual **Apply fields** button, and checks that the remaining schema
  defaults survive. Inspecting fields must not create a runtime host.
- Uses the public Save As, Save, and read functions to reopen the edited
  project, preserving its authored values and selected Engine binding.
- Runs real `GameHost` sessions with Objects and ECS worlds, checks blocked
  movement, the five-move solution, turn-limit failure, ignored moves while
  halted, and restart after both outcomes. The Inspector edit must affect the
  initialized world.
- Writes actual 960×540 Vulkan frames for the initial, solved, and restarted
  board. Both world implementations must produce equal logical solved states
  and RGBA output; restart must restore the initial frame.
- Uses the existing Full project exporter, reopens the result as an editable
  source package, then runs and renders it with the exported folder supplied
  explicitly as both its resource and Engine root.
- Checks the source file hash and saved author file bytes after execution.

The case contains PNGs, solved state snapshots, the saved author project, the
Full export, and `puzzle-report.json`. The output folder also receives the latest
report. The focused editor unit test
`puzzle_verification::tests::generic_inspector_save_reopen_and_puzzle_only_runtime_preserve_authored_project`
covers the real Inspector/save/reopen workflow and then solves and restarts
using a registry that installs only neutral builtins and the Puzzle package.

This proves the Full editable source export path. That export includes the
Engine's complete source tree and can contain other installed modules and
unused assets. It does not establish a minimal standalone Player package or
the physical absence of ARPG sources. Empty authored audio cues are checked;
audio service allocation and endpoint absence are outside this proof.
