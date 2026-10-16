<a id="scene-assets-и-отзывчивое-редактирование"></a>

# Scene, Assets, and responsive editing

[English](release-scene-assets.md) | [Русский](ru/release-scene-assets.md) | [Documentation](README.md)

This stage adds a shared authoring context for **Scene / Hierarchy / Inspector / Assets**. **Game** uses a separate project copy for Play. The previous demonstration runtime remains in Engine.

![Verified native Scene and Assets window](media/editor-scene-assets.png)

<a id="поведение"></a>

## Behavior

- Object selection uses the model's actual triangles, including separate glTF primitives; selection is shared across authoring windows.
- Move/Rotate/Scale, World/Local, Snap, a free camera; one completed gesture creates one Undo entry, and Escape cancels the current gesture.
- Assets: folders, search, filter, import, background previews, model and texture drag-and-drop, persistent IDs.
- Rename, move, and delete one file with handling of known references; asset usage is checked before deletion. The trash retains originals.
- Saving is required before a file operation; Undo/Redo restores files and the document together. History does not overwrite externally changed files.
- Old personal layouts migrate; window placement and cameras are saved separately from the game.
- A Game window opened earlier receives the current authored scene before Play. Pause preserves the game copy; Stop restores authored state.

File purposes, API rules, and user actions are described in [Scene and Assets](scene-assets.md), the [asset contract](asset-workspace-api.md), and [history](history-assets.md).

<a id="исправление-задержки"></a>

## Fixing the delay

Previously, a numeric Transform change reset GameHost. The next frame recreated the Vulkan executor, prepared Custom Rust, and decoded content. Now the authoring preview updates the validated scene while retaining the renderer, decoded models, and compiled Custom slots. Assets scanning/hashing runs on open, Refresh, and file operations; Transform does not trigger scanning or usage recalculation.

Assets and execution structure have separate checks: changing a contract may require rebuilding the host; a faulty candidate does not replace the current one. Rendering still synchronously returns a frame to the CPU for egui, so eliminating recreation is not a promise of any particular frame rate.

<a id="воспроизводимая-проверка"></a>

## Reproducible verification

~~~powershell
.\outputs\ModuleEngine.exe --scene-assets-check game-project.json outputs\scene-assets-verification
~~~

The command creates a separate verification directory and does not save the source game JSON. It checks import, IDs, file Undo/Redo, protection of used assets, unlinking, trash, Save As, and Full/Team/Player. The measurement uses actual egui events and 120 drag frames with Vulkan, on one imported object and the arena scene, for Objects and ECS. The report distinguishes frame processing from cold preparation, compositor/vsync, and actual monitor output.

Verified on this PC, Vulkan / NVIDIA GeForce RTX 4080 SUPER, authoring viewport 944×563:

| Content | World | Median | p95 | Maximum |
| --- | --- | --- | --- | --- |
| One imported model | Objects | 3.52 ms | 5.16 ms | 7.34 ms |
| One imported model | ECS | 4.10 ms | 6.12 ms | 6.93 ms |
| Arenas, 29 active scene objects | Objects | 4.65 ms | 5.92 ms | 6.68 ms |
| Arenas, 29 active scene objects | ECS | 4.42 ms | 5.64 ms | 6.51 ms |

Each case confirmed a retained model cache and compiled Custom slot, 122 applied Transform updates (including start/end), one Undo entry, and working Undo/Redo. The measurement excludes other editor windows and monitor output; it is not a measured frame rate for the whole application.

All 230 unit tests passed. `cargo clippy --workspace --all-targets -- -D warnings` and the rustdoc check passed. The CLI check confirmed Engine access in Full, no such access in Team, and actual rendering of an ID-referenced model in Player from ready-made workers/SPIR-V without invoking a compiler. A separate test checks scene edits with Game already open, Play/Pause/Stop, and explicit asset-cache refresh. The source `game-project.json` remained byte-for-byte unchanged. Full numerical results: [scene-assets-report.json](media/scene-assets-report.json).

<a id="границы-этапа"></a>

## Stage boundaries

Work is limited to one selected object; group manipulation is deferred. Navigation and folder creation are available; rename/move operations apply to one file. An unrepresentable rotation/scale combination with a non-uniformly scaled parent is rejected instead of silently distorting it. The model preview shows geometry with a neutral CPU renderer rather than its final Vulkan material. Audio files display metadata.

Current-session history retains file boundaries. Trash and on-disk snapshots persist between launches; automatic recovery of an unfinished operation after a crash is not implemented. A checkpoint or merge with missing/changed assets is rejected before changing the document: restore files through Undo or the trash first.
