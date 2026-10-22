<a id="меню-редактора-и-работа-с-проектом"></a>

# Editor Menus and Project Workflow

[English](editor-workflow.md) | [Русский](ru/editor-workflow.md) | [Documentation](README.md)

The top **File / Edit / Tools / Windows** bar brings together project, build, and layout commands. Beneath it remain **Project / Engine**, **Play / Pause**, **Stop**, **Save**, and the project name. Hovering over the name shows the path to the open JSON; **● unsaved** means authored data or a draft has not yet been saved. Changes to the Play copy of the scene do not count as authored edits.

Menus and shortcuts invoke the same commands. This preserves identical project and Full / Team restriction checks regardless of how an action is started.

<a id="ui-language"></a>

## UI Language

Engine-owned labels, help, status, progress, and error messages in Editor, Player, and CLI tools are English. Authored names and content remain unchanged, including existing project, scene, object, node, and history labels. Opaque external compiler and OS diagnostics retain their original text; OS-owned native dialog controls retain the system language. This contract adds no locale selector and does not migrate authored data.

<a id="первый-рабочий-маршрут"></a>

## First Working Route

1. Choose **File → New Project → Empty scene** or **Two arenas example**. In the system dialog, select a new or empty project directory. It may be, for example, in `projects/` or `outputs/` of the installed engine, but not inside the resource library being copied. It will contain `project.json`, resources, and a binding to the installed Engine; after successful creation, the editor switches to the new project.
2. Open **Windows → Scene**, **Hierarchy**, **Inspector**, and **Assets**. Import glTF/GLB or PNG/JPEG through **File → Import Asset…** or **Import…** in Assets. Drag a model into Scene to create an object; select it by clicking or through Hierarchy, and change its transform with Move/Rotate/Scale handles. Drag a texture onto an object or assign it through Inspector. WAV import adds an audio file for subsequent use by game events. Keep the Game window for Play.
3. Press **Ctrl+S**. For a separate copy, choose **File → Save As…**, another directory, and a JSON name. After completion, work continues with the copy. The original stays in place. If an existing destination file is selected, Windows asks for confirmation before replacing it.
4. Through **File → Open Project…** or **Recent Projects**, reopen the saved JSON. Its scenes, templates, Custom sources, and settings will be read together with resources from that directory.
5. Run **Tools → Check Project**, then **Tools → Build Windows Player…**. Choose a new or empty build directory. After successful export, launch `Player.exe` from it. Player uses prebuilt Rust workers and SPIR-V; Rust/Cargo/DXC are not needed on the player's machine. Windows GUI presentation uses WGPU/DX12 and requires DX12 support; a project using the Vulkan game renderer additionally needs a compatible Vulkan driver.

Placement and browser work are described in [Scene and Assets](scene-assets.md), gameplay and Play in the [game editor](editor-game.md); build requirements and design are in [Player](player.md).

<a id="file-создать-открыть-сохранить"></a>

## File: Create, Open, Save

| Command | What it does |
| --- | --- |
| New Project → Empty scene | Creates one empty authored scene with the selected Engine architecture. |
| New Project → Two arenas example | Creates a game example with monsters, a transition, and a boss on the selected Engine architecture. |
| Open Project… | Selects an existing JSON in a native Windows dialog. |
| Recent Projects | Shows up to 12 successfully opened/created/saved projects, most recent first. Hover reveals the full path. |
| Save Project | Saves the current authored document to the open path. |
| Save As… | Saves an authored copy to the selected path and switches the editor to it; replacing an existing file requires native confirmation. |
| Import Asset… | Imports glTF/GLB, PNG/JPEG, or WAV through file selection. |
| Export → Windows Player… | Builds a standalone game. |
| Export → Team editor… | Creates a team editor with a frozen architecture. |
| Export → Full source project… | Exports the full engine and game source project. |
| Exit | Closes the editor after checking for unsaved changes. |

**New Project** preserves the module graph, source identifiers, HLSL, Engine settings, and system execution graph of the current project. Permitted game Rust `update` sources are replaced with empty logic, and the previous game's object settings are cleared. This allows a new game to start on an already configured engine. If the architecture must change only for the new game, first make a copy and edit Engine in it with Full access.

The empty scene uses the current renderer's existing preview camera and lighting. The new stage does not add separate authored Camera/Light components or a lighting-schema editor. Import and geometry creation are described in [editor-game.md](editor-game.md).

Recent is stored separately from JSON, in `.user/editor-recent-projects.json` of the installed engine package. It does not enter the game export. If a project file is moved or deleted, opening it shows an error; the already open project remains in the editor.

<a id="save-as-авторская-копия-и-установленный-engine"></a>

## Save As: Authored Copy and Installed Engine

When moving to another directory, Save As preserves the entire authored document: scenes, linked templates and individual overrides, graphs, embedded Rust/HLSL, settings, and archived data of disabled modules. It transfers **all `assets/` files**, including unused ones, as well as resources declared in the manifest and scene data, glTF dependencies, and HLSL includes. Disabled data is not discarded for a runtime build.

Copying is prepared and checked first; the editor switches to the copy only after a successful write. An existing destination JSON can be replaced after confirmation in the native dialog. Replaced JSON and binding files receive exact backups of their previous bytes beside them, with a `.backup-<id>` suffix. Selecting the currently open JSON performs an ordinary Save.

In an existing directory, identical resources can be reused, but a file with the same relative path and different contents causes an error **before the old JSON changes**. Confirmation to replace a project does not permit overwriting conflicting resources. For an independent copy, use a separate directory. It may be inside the engine installation, for example in `projects/` or `outputs/`; saving inside `assets/` or other resource trees copied by the operation itself is prohibited. Saving another JSON in the same directory makes both documents share that directory's resource library.

A separate binding file is written beside each new JSON, for example:

~~~text
project.json
project.json.module-engine-project.json
assets/
~~~

The binding contains a version, path to the installed package, and Full / Team profile. Team also preserves the frozen blueprint. Engine permissions are determined by the verified manifest and actual installed-package sources; the binding grants no additional permissions and does not substitute an SDK.

Save As makes an independent **copy of authored game data**, but does not copy the entire installed engine. Opening and execution require an available compatible Engine package with a backend; Rust Custom compilation uses its Full sources or Team SDK and installed tools. HLSL and building also use package and machine tools. When moving to another computer, use **Full source project**, **Team editor**, or **Windows Player**, depending on the required distribution type. Their contents and requirements differ: see [distribution.md](distribution.md) and [player.md](player.md).

<a id="папка-игры-и-папка-инструментов"></a>

### Game Directory and Tool Directory

| Path base | What it is used for |
| --- | --- |
| Open JSON directory | Game models, textures, audio, and other relative resource paths; the working directory of running Rust Custom. |
| Installed Engine package | Renderer backend, DXC, engine shader includes, and GPU cache. |
| Full sources or Team `.sdk` of the installed package | Rust Custom compilation with compatible engine contracts. |

For example, `std::fs::read("assets/config.json")` inside Rust Custom reads a file beside the current game JSON rather than from the original Engine installation. After Save As and during hot apply, Custom's working directory remains the open project's directory. This allows a game with a separate resource library to work correctly even if the engine is installed elsewhere.

DXC, backend, and GPU cache need not be copied again on every Save As: the editor obtains them from the installed compatible Engine. Authored resource dependencies are transferred with the game. In Team, the project's local `assets/` do not replace the installed backend's frozen shader includes; Engine restrictions continue to be checked.

**Full source project** transfers Engine sources, its dependencies, and game data as a separate distribution. To distribute a finished game, use **Windows Player**: its package contains its own backend resources and precompiled workers/shaders. Simply copying the game directory with a binding file does not replace these export types.

<a id="несохранённые-изменения-и-черновики"></a>

## Unsaved Changes and Drafts

Before Open, Recent, New, Exit, Check Project, or export, the editor checks authored changes and shows a dialog with three choices:

- **Save and continue** writes the current document and performs the selected command only after saving succeeds.
- **Don't save**, or **Use saved version** for checking and export, continues without writing current edits. Checking and export use the on-disk project version; current edits remain open in memory.
- **Cancel** keeps the current work open and cancels the transition or build.

On Open and New, the old document, history, and drafts remain available until the replacement is successfully opened or created. Canceling the system dialog and errors do not discard them even after choosing **Don't save**. In that case, New uses the last saved Engine configuration. After a successful transition, the editor works with the new project; exiting without saving closes unsaved work.

Invalid JSON does not replace the document or permit continuation through saving. The **File** window's JSON draft is validated and applied before Save after completing the Gameplay Systems draft. Correct it or explicitly discard the draft if the changes are unnecessary.

**Save As** first validates the JSON draft as a separate candidate. Canceling selection, a system-dialog error, or a copy-write error leaves the original document, draft, history, Recent Projects, and current Play unchanged. The candidate becomes the open document only after successfully writing JSON, the Engine binding, and resources. Saving to the same path keeps history available; game state changes only when edits requiring its recreation are successfully applied.

**Gameplay Systems** has its own graph draft. Before saving, finish it through **Apply graph draft** or **Discard draft**. If the protection dialog is already open, click **Cancel**, return to Gameplay Systems, and finish the draft, then repeat Save or the required command. An unfinished draft is not presented as a saved graph and is not included in export. Typed resource and application details are in [editor-game.md](editor-game.md).

While the Gameplay Systems draft is unfinished, document Undo/Redo, applying another JSON, checkpoint, and merge do not replace its base. Apply or discard the draft first, then perform these commands. If both JSON and Gameplay Systems have changed, explicitly discard the JSON draft to edit Gameplay Systems again; Save does not apply one draft over another or silently erase it.

<a id="edit-и-горячие-клавиши"></a>

## Edit and Shortcuts

**Edit → Undo / Redo** works with authored-document history. Assets file operations, including import, rename, move, delete, and restore, create separate history boundaries: Undo/Redo restores files and references together and saves the result. One completed move, rotate, or scale gesture is one history entry. History does not rewind game runtime or roll back personal layout.

**History / Merge** opens history, checkpoints, comparison, and merging. Restoring a document checks resource existence and hashes; a stale path or deleted ID does not replace the working version. First restore files through Undo of the file operation or Assets trash. See [scene-assets.md](scene-assets.md), [history-assets.md](history-assets.md), and [asset-workspace-api.md](asset-workspace-api.md).

| Keys | Command |
| --- | --- |
| Ctrl+N | New Project → Empty scene |
| Ctrl+O | Open Project |
| Ctrl+S | Save Project |
| Ctrl+Shift+S | Save As |
| Ctrl+Z | Document Undo outside a text field |
| Ctrl+Y / Ctrl+Shift+Z | Document Redo outside a text field |

When a Rust, HLSL, JSON, or ordinary text field has focus, Ctrl+Z/Redo remain with the text editor and do not undo the entire document. Focus on a viewport, button, or other nontext widget does not disable document Undo/Redo. File commands, including Ctrl+S, are available while typing; Save accounts for the JSON draft and does not bypass an unfinished Gameplay Systems graph.

<a id="tools-и-фоновые-операции"></a>

## Tools and Background Operations

**Check Project** checks the saved project through the same CLI route as `--check`. **Build Windows Player…** runs `--export-game`. **Diagnostics** opens profile, source, and runtime/preview error information; **Project Settings** opens parameters permitted by Engine. **Last operation report** shows the latest check or export result again.

Native dialogs, import, copy preparation, checks, and export run in a worker thread. The editor continues repainting and shows the operation name, waiting indicator, and elapsed time; this is neither a completion percentage nor an estimate. One operation runs at a time. Document-changing commands are locked during an operation so that the result applies to the selected project version. Closing the editor is blocked while an operation runs; after it finishes, the window can be closed.

Errors appear in the **Operation result** window. While the report is open, global menu and shortcut commands are locked; close the report to continue working on the project. For CLI checks and builds, full stdout/stderr is written to `.work/editor-jobs/<id>/` beside the open JSON; the path is included in the report. Native-dialog, import, or copying errors are shown directly. System cancellation of file/folder selection returns control to the current project.

Native dialogs are available on Windows. The initial directory is an optional hint: the editor normalizes relative paths, `/`, `.`/`..`, and ordinary extended-length paths into a form understood by Windows Shell, separately from working project paths. If the directory disappeared, Shell does not support its path, or rejects the hint, the dialog opens in its default directory. Errors in the dialog itself and reading the selected result still appear to the user. Selected paths, Recent Projects, bindings, and file I/O are not rewritten for the hint.

On Linux, system file-selection dialogs currently return an explicit unsupported message. Editor/Player launch and CLI path operations are independent of this limitation. Dialog verification and fix boundaries are described in the [report](dialog-path-fixes-20261016.md).

The **Export project…** command in Module Library invokes the same asynchronous Full export command as the File menu. It uses system folder selection and checks unsaved changes and the Gameplay Systems draft. Copying runs outside library painting; results and errors appear in the shared operation report.

Import reserves a separate new directory for one file set. If copying, dependency parsing, or adoption into the log is rejected, that directory is removed; existing assets are preserved. If cleanup itself fails, the report identifies the remaining path. Successful import preserves shared Undo for files and references.

<a id="windows-и-личная-раскладка"></a>

## Windows and Personal Layout

Selecting an area type through **Windows** searches for it in the current **Engine** or **Project**. If several such areas exist, the most recently used one is activated; if none exist, the active area splits into two adjacent areas and the selected editor opens in the new one. An already open window is not duplicated, and the neighboring area's type is preserved. Clicking an area changes which is active; the active area has a highlighted border.

Available types come from installed active modules and the distribution profile. Engine areas and the Engine tab itself are available only to Full with a complete compatible source inventory. Team stays within Project, permitted parameters, and game Rust; New, Save As, and menus do not unfreeze architecture.

**Scene**, **Hierarchy**, **Inspector**, and **Assets** are available in both layouts. Scene/Hierarchy/Inspector share authored-object selection; each Scene retains its own viewing camera. Each Assets retains its own directory and filters. These windows can sit beside Game; authored edits are made after Stop. See [scene-assets.md](scene-assets.md) for details.

Any area's header can select another available type, split it with **H / V**, or close it with **×**. Engine and Project layouts are independent. **Save Layout** saves personal layouts, **Load Saved Layout** rereads them, and **Reset Layout** resets the current workspace to its initial view. Changes are also saved automatically. Personal settings are in `.user/<full|team>/<engine|project>.json` of the installed package and are outside scenes and document history.

In graphs, **Ctrl + wheel** zooms content under the cursor, including over a node or pin. Graph position compensates for zoom, so the point under the cursor stays in place. Right or middle mouse button pans the view. Only the graph under the cursor is scaled; other windows and interface size stay unchanged. This is a shared handler for module, GPU-pass, Rust, and Gameplay Systems graphs.

The handler is in `canvas.rs` and uses `InputState::zoom_delta()`: egui already converts Ctrl + wheel into a zoom factor, leaving ordinary scrolling at zero. `canvas::tests` send real wheel events into egui and check both directions over background/node/pin, preservation of the point under the cursor, a neighboring graph, zoom limits, and no response to ordinary scrolling or a disabled area.

Dragging a node changes its position in the diagram and is saved in the document, history, and export. It must not recreate Game/Scene, the GPU executor, compiled Custom, or game state. For this, `Graph::execution_eq` recursively excludes node coordinates and labels, and for `custom.rust` / `custom.rust.update` also `code_layout` and `code_layout_keys`. All other parameters, connections, order, sources, assets, and scenes continue participating in comparison. Authored-document and background-preview updates use this comparison; ordinary `PartialEq` and serialization still preserve the full layout.

The `app::tests::graph_node_drag_preserves_game_and_scene_hosts` regression performs real egui dragging over the external and nested GPU graph with Game and Scene already open. It checks node movement and executor preservation; the frame count can be increased with `MODULE_ENGINE_GRAPH_DRAG_FRAMES` (up to 240). If `MODULE_ENGINE_GRAPH_DRAG_REPORT` is set, the result is written to the specified JSON. The measurement includes graph processing and synchronization of stopped Game/Scene; native presentation and 3D-view rasterization are excluded. Before the fix, this scenario recreated executors and recorded individual delays around 2,3 seconds in a debug build.

After the fix, 120 input frames were checked for each graph, including 113 actual movements: Game/Scene preserve executors, with maxima of 7,04 ms for the external graph and 8,07 ms for the GPU graph in the same debug scenario. The [before/after report](media/graph-drag-report.json) states measurement boundaries and the difference in sample sizes; these numbers are not FPS of the whole editor.

<a id="сборка-gpu-графа-и-возврат-удалённых-проходов"></a>

## Building a GPU Graph and Restoring Deleted Passes

In **Engine**, double-click the Vulkan renderer. Add passes through the **+ Node** button above the graph itself or through **Windows → Module Library**. The library supports search by module name and ID; the **Adding to:** line shows the graph and window number that will receive the node. The library follows the last active Module Graph in the current workspace. To choose another graph window, click inside it. Engine and Project contexts are preserved separately; architectural changes require Full with sources.

To restore antialiasing, add **FXAA** and connect `tonemap.color → FXAA.color → encode.color`. The last connection replaces the old direct tonemapper-to-encoding connection. To restore **Screen-space AO**, connect HDR color, normals, and depth to its inputs, then its `color` output to the tonemapper. In Forward, color comes from Geometry; in Deferred, from lighting, while normals and depth come from Geometry.

The [native window with the GPU-pass library](media/gpu-graph-library.png) shows a graph without AO/AA: add them from the list or find them through search. If a pass appears outside the current view, the editor moves the view to it while retaining the selected zoom.

New FXAA receives Project enable/disable variants targeting the actual ID of the new pass. A new tonemapper receives parameters and mode variants; existing authored settings are not overwritten. For SSAO, configure `strength` in the Inspector or open **Engine Settings → Expose an existing GPU parameter** and expose the parameter to Project. New standard tonemapper presets use the shader contract `ACES = 1`, `Reinhard = 0`; saved authored commands do not migrate automatically.

The **GPU Forward / GPU Deferred** buttons in the 3D preview replace the internal graph of the renderer connected to Viewport. Settings for disappeared passes and resources are cleared with Project references; definitions for surviving targets remain. New standard passes receive their own settings. Switching and settings changes are undone with one Undo. This command does not replace an unused alternative renderer.

Connections are made by clicking an output pin, then an input pin. Beyond the common pin type, texture format, data meaning, size, and sample count are checked. For example, HDR cannot connect directly to FXAA expecting LDR. A rejected connection does not remove the previous one. Each addition and connection step has its own Undo/Redo boundary, including created settings. Adding GPU Custom in different graphs creates independent HLSL source IDs.

Until all required inputs and outputs of a new pass are connected, the graph is incomplete. Editing the document can continue; the previous working render is preserved. Preparation status reports the rejection reason and confirms application when the chain becomes valid.

<a id="удаление-gpu-прохода-и-связанных-настроек"></a>

## Deleting a GPU Pass and Related Settings

In **Engine**, expand the Vulkan renderer, select a pass, and delete it through the node Inspector. Architectural changes are available to Full with compatible sources. The **Pass enabled** flag preserves the node and its settings; the delete command removes the pass itself from the graph.

Deletion reconciles the graph with permitted Engine and Project settings in one document change:

- Parameters targeting the deleted pass are removed from Engine Exposure.
- If at least one ready-variant command refers to the deleted pass or its resource, the **entire variant** is removed. Remaining commands in that variant do not become a new partially working preset.
- If a group's default is deleted, one of the remaining available variants becomes the default. A group with no variants is removed.
- Project values for deleted parameters and selections referring to disappeared groups or variants are cleared. Rules requiring a deleted parameter, group, or variant are removed; valid selections and rules are preserved. If an individual selection no longer exists, the group's available default applies.

For example, deleting FXAA from the graph removes ready variants controlling that pass and related Project rules. If no other variants remain in the antialiasing group, the group itself disappears from settings. **Undo** restores the deleted node, connections, Engine Exposure, and corresponding Project values, selections, and rules together; Redo repeats the deletion.

[Example of a working graph after AA removal](media/editor-aa-removal.png).

Standard **SSAO** and **FXAA** support connecting the preceding color output to consumers of the deleted node's color output when contracts are compatible. For example, after deleting FXAA, the `tonemapper → FXAA → color encoding` chain continues passing finished color from the tonemapper to encoding. This bypass is also possible for Custom with an unambiguous contract.

For automatic reconnection, the deleted pass must have a single output and exactly one input with the same `format`, `semantic`, `extent`, and `samples`. Exactly one connection must lead to that input. Pin names need not be `color`: the decision is based on matching contracts. A pass with multiple outputs or an ambiguous suitable input is not automatically bypassed.

Deleting geometry does not create geometry and a G-buffer; deleting the tonemapper does not convert HDR to LDR; deleting encoding does not replace color-space conversion. Such changes require manually connecting compatible stages or installing a suitable replacement. Pin types, formats, and resource meaning continue to be checked; an available default alone does not repair an incompatible render chain. The preparation error remains visible in status, and the previous working version is used until successful application.

Runtime parameter and pass-enable overrides, and resource commands targeting removed destinations, are cleared **after successful application** of the new renderer. A rejected candidate does not change overrides of the previous working configuration. This cleans up existing assignments: a new Custom command targeting an unknown pass or resource is still rejected with an error.

Settings reconciliation processes declared Engine Exposure and ProjectConfiguration references. Settings targets are determined by the renderer connected to Viewport; a same-named pass in an unused alternative renderer does not preserve settings for a deleted active pass. Arbitrary Rust/HLSL sources are not rewritten: the author updates Custom commands, pass names, and resources embedded in code when needed. To check the saved result, run **Tools → Check Project**: current settings and all declared ready variants, including unselected ones, are checked. Parameter and ready-variant design is described in [settings.md](settings.md).

<a id="подготовка-и-применение-изменений"></a>

## Preparing and Applying Changes

When the GPU graph changes, for example when switching the **Pass enabled** flag for AA, the bottom bar shows ongoing operations, the current stage, and elapsed time. A spinner indicates work without estimating remaining time. Click the bar or arrow to expand individual Engine, Game, Scene, and Rust tasks; a long error can be read in full and copied. Success and errors remain after the operation ends.

Vulkan, shader, and new Game/Scene executor preparation runs in the background. While a candidate is prepared, the previous working version or latest frame is used; the first window opening shows a preparation wait. With rapid repeated changes, the current configuration is applied and stale results skipped. For a successful build, **Done** status appears after the editor accepts the result, not immediately after the compiler finishes. Canceled and superseded requests also finish: their details explicitly state cancellation or replacement rather than application.

Replacing only rendering preserves the game session, Rust state, camera, and loaded resources. Changing the world model itself or executable-system graph requires preparing a new executor: Play steps pause temporarily, and accepting the new executor recreates game state. If preparation is rejected, the error remains visible and the previous working version is preserved. Authored edits remain in the document: they can be corrected or undone through Undo. Saving JSON and applying it in preview are separate actions; preparation status remains visible after Save.

This queue handles graph and embedded HLSL-text changes. Watching external HLSL files during rendering currently uses the previous compilation route; moving it into the background remains a separate task.

After failed Engine, Game, or Scene preparation, correcting an external literal HLSL include permits retry without changing JSON. Dependency state is captured before the task starts and checked after failure no more often than once per 250 ms against the selected backend. This also accounts for a file corrected before the worker error arrives. An unchanged error does not trigger compilation every frame; this is not a general project file watcher.

Regressions check a real AA click while drawing the old GPU during preparation, preservation of the game copy and Rust state, erroneous HLSL, Stop, and stale results. Initial Scene waiting and expansion of a long error were checked separately. Native-window examples: [successful application](media/editor-preparation-ready.png), [HLSL error](media/editor-preparation-error.png).

The shared `activity.rs` model sends worker stages through `BuildProgress`; the UI reads snapshots and continues repainting. `PreviewBuilder`, Rust compilation, and Game/Scene preparation report stages and results through one interface. Ordinary CLI checks retain synchronous behavior and do not depend on the editor window.

In native Editor, after preparation, GameHost is transferred to a persistent game thread. It performs simulation, game Rust application, graphics updates, and Game rendering. The UI sends input and commands without waiting and shows the latest completed frame and state. Slow Custom therefore slows game progression but does not force the window to execute a series of catch-up ticks. The game owner processes commands between individual ticks; running Rust is not interrupted midway through a tick. Pause and Restart take effect at this boundary, Stop detaches the executor, and its processes and GPU resources are released outside the UI. Authored Scene remains a separate preview.

Graphics changes are coalesced into the latest pending document: the UI has at most one submitted and one next candidate at a time. The old working frame remains visible until the new one is accepted. Attack/interact/restart presses are preserved separately from the latest held direction; overflow of the bounded input queue produces an explicit error and pauses Play. A hidden Game window stops requesting GPU frames while retaining simulation and Rust-window functionality. Diagnostics separately shows the current state revision and tick/revision of the latest ready pixels: a retained frame on error is not presented as new.

This model is enabled by real native Editor and native diagnostics; background preparation mode alone does not change the synchronous headless-check contract. See [GameWorker](game-worker.md) and [Custom state](custom-state.md).

<a id="перерисовка-и-измерение-производительности"></a>

## Repainting and Performance Measurement

With a stationary Scene, repeated events in other windows use the ready texture. Camera, size, accepted-scene, resource, or GPU-configuration changes update it immediately; a verification repaint remains every 250 ms for external HLSL files. Object selection and handles are drawn over the image every UI frame. Game separately reads input and draws the HUD. Simulation remains fixed at 60 Hz; the worker schedules eligible presentation samples toward 120 Hz, interpolating supported poses with approximately one fixed tick of visual delay. Static or clamped samples retain the completed frame rather than manufacturing extra frames. This scheduling target does not guarantee measured 120 FPS. Stop/Pause uses the ready image until a change or periodic check. See the [worker contract](game-worker.md) and [Host interpolation boundaries](shared-game-host.md#fixed-simulation-and-interpolated-presentation).

The ready GPU plan is reused between unchanged frames. Explicit candidate preparation still checks fresh HLSL include files; during ordinary rendering, repeated checks are coalesced at a 250 ms interval. Contract and permitted-settings validation works from manifests without reading shader files. The graph background grid is drawn as one mesh instead of thousands of individual shapes. **Tools → Diagnostics** provides preparation, render, and frame-reuse counters; these count work, not FPS.

For repeatable whole-editor measurement, use `--editor-benchmark game-project.json output-folder`. Run it on a separate Full copy: the editor constructor writes Recent to that copy's `.user`. Diagnostics opens a real Editor with Graph, Game, and Scene, warms up preparation, and measures 90 real graph movements each at Stop and Play. `report.json` separately records Editor UI time, the previous eframe CPU frame, inter-frame interval, visibility/focus limitations, and document preservation. These metrics do not replace screen-latency or standalone Player measurements.

Release-build comparison from 15.10.2026: the same project without AA/AO, Graph/Game/Scene, 20 living enemies, a 1920×1080 native window on a 144 Hz monitor; 90 measured graph movements in each phase. Both windows had focus in every measured frame. The table gives **median / p95 in ms**; lower is better.

| Measurement | Before | After |
| --- | --- | --- |
| Stop: Editor UI processing | 4,77 / 5,56 | 0,70 / 1,86 |
| Play: Editor UI processing | 7,14 / 9,77 | 2,03 / 4,12 |
| Stop: interval between UI frames | 6,86 / 8,12 | 6,94 / 7,22 |
| Play: interval between UI frames | 9,08 / 14,33 | 6,94 / 7,27 |

The [comparison report](media/editor-native-performance-comparison.json) contains additional CPU metrics, maxima, and limitations. Document/file invariance, Custom-executor preservation, and Play progression were checked. Input is supplied programmatically to a real window; physical mouse and display latency is not measured, and native visibility remains unknown. These are two sequential runs on one PC, not a speed guarantee for other projects. The first unfocused run of the old build is preserved separately and excluded from this table.

A separate Player check on the same packaged content with 20 enemies at 1920×1080: median game-frame processing 5,22 → 4,44 ms, p95 5,70 → 5,23 ms. It includes simulation, audio, and Vulkan with readback, but excludes the window, vsync, and HUD. Functional checks of both world models, defeat, arena transition, victory, and Rust rollback also passed. Severe standalone Player degradation was not reproduced on this fixed example; the larger confirmed cost was in the editor.

<a id="где-находится-реализация"></a>

## Implementation Locations

| File | Purpose |
| --- | --- |
| `crates/editor/src/editor_workflow.rs` | Shared menu/shortcut commands, Recent, change protection, background jobs, and reports. |
| `crates/editor/src/activity.rs` | Shared preparation progress, time, result, and expandable diagnostics. |
| `crates/editor/src/native_dialogs.rs` | Windows COM file/folder dialogs without a separate helper process. |
| `crates/editor/src/workspace_files.rs` | Project templates and verified copying of authored data and resources. |
| `crates/editor/src/window_navigation.rs` | Finding existing areas, activation, and adjacent addition. |
| `crates/editor/src/editor_benchmark.rs` | Native Editor measurement with Graph/Game/Scene, programmatic input, and project-preservation checks. |
| `crates/editor/src/distribution.rs` | Project-to-Engine binding, source/SDK checks, and Full / Team policy. |
| `crates/editor/src/assets_ui.rs` | Directories, filters, selection, dialogs, and browser commands without file changes inside the UI. |
| `crates/editor/src/assets_workspace_ui.rs` | Resource-index cache, information, preview queue, and validation of resources for a restored version. |
| `crates/project-tools/src/asset_workspace.rs` | Persistent IDs, dependencies, file transactions, trash, and rollback with the document. |
| `crates/editor/src/scene_view_ui.rs`, `scene_tools.rs`, `scene_workspace.rs` | Visual tools, geometry selection, viewing camera, and applying gestures to the authored scene. |

The UI uses existing check and export commands. Separating the installed engine from the game directory allows saving new projects without copying compiler/backend on every Save As, while still checking which Engine actually serves the project.

In the file service, `save_as` does not overwrite an existing JSON by default. The UI invokes a separate `save_as_confirmed` only after replacement is confirmed in the native dialog; both functions check resources and retain profile restrictions.
