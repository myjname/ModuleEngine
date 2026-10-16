<a id="авторские-сцены-и-граф-игровых-систем"></a>

# Authored scenes and the gameplay system graph

[English](editor-game.md) | [Русский](ru/editor-game.md) | [Documentation](README.md)

Open **File → New Project → Empty scene** or **Two arenas example** and choose a separate new/empty folder. The authoring windows are **Scene / Hierarchy / Inspector / Assets**; for Play, open **Windows → Game**. The folder may be inside the engine installation's `projects/` or `outputs/`, but not inside copied assets. A new game project uses the current Engine architecture; choosing `Не сохранять` (Don't Save) for New uses its last saved version. Saving writes `game` into the graph file; game-state changes during Play do not enter that file. See [Scene and Assets workflow](scene-assets.md) and [menus and saving](editor-workflow.md).

Save As transfers the authored scene, prefabs, overrides, disabled data, and the entire `assets/` library with declared dependencies. Confirmed replacement of an existing destination file retains backups of its metadata; conflicting assets are not replaced. Cancelled or failed opening/creation of another project preserves the current work. **Check Project** and export after `Не сохранять` (Don't Save) use the on-disk version, while the unsaved scene and drafts remain in the editor.

On first launch, Project creates the Scene / Hierarchy / Inspector / Assets layout. An old personal layout migrates while retaining splitters and views; former Project Viewport/Tree/Components become authoring windows, and a single old Game becomes the new default layout. Windows activates an open area of the required type or adds it next to the active area. Personal layout is not part of exported architecture. An empty scene uses the existing preview camera and renderer lighting; this template creates no separate authored Camera/Light components.

<a id="сцены-и-объекты"></a>

## Scenes and objects

The scene list in **Hierarchy** selects the authoring preview. **Scene name** and **+ Scene** create an empty scene; **Duplicate** copies the current scene with all objects, local IDs, references, and prefabs. **Set as entry** sets the starting scene for Game and the exported game. The selected authored scene and starting scene may differ.

Select an object in Hierarchy or by clicking geometry in Scene. **+ Object** creates an object with a new nonzero ID. Scene gizmos change its transform; numeric position, rotation in radians, scale, parent, and color are available in Inspector. **Delete** removes an object and its descendants; remaining objects' references to deleted IDs are cleared. A parent cycle or other invalid state is rejected as a whole. Gameplay actors remain roots; the hierarchy supports decorative objects.

**Play** starts a copy. WASD moves the player, left-click attacks toward the cursor, and R restarts the current game example. **Pause** retains and suspends the runtime; **Play** resumes. Authored changes and applying a new graph require **Stop** — pausing does not replace stopping. Stop restores authored positions and objects.

**Restart** and **R** use the shared `GameHost::restart`: the entry scene and Custom states are restored, accumulated render commands/overrides and asset-operation queues are cleared, and current audio stops. The Game camera targets the player again even when restarting while paused; its yaw/pitch/distance are retained. Rust recompilation and GPU recreation are unnecessary. This restart stays inside the game copy; returning to authored editing requires **Stop**.

<a id="поля-компонентов-и-шаблоны"></a>

## Component fields and prefabs

The ordinary inspector uses the same installed module and component-schema catalog as project validation, execution, and export. It displays components of active systems, explicitly registered `SystemNode.params.components`, and mesh/material when a renderer with the `Scene3D → Frame3D` contract is connected. Fields of an absent component do not appear as object settings. **+ Component from active modules** creates a draft from its schema defaults, including in an empty scene without examples or prefabs. Change fields and click **Apply fields**.

The source module's schema defines fields: numbers use its range, bool uses a checkbox, enum uses a list of allowed values, and text uses a length limit. A vector is edited through individual index fields. Object references offer objects in the current scene with required components; asset references offer Assets entries with allowed extensions, including extensions from third-party modules. An optional reference can be cleared to `null`. Fields account for declared dependencies on active modules and attached components. A new schema automatically enters this form; no separate Inspector branch per component is needed. The catalog and forms are created without GPU initialization.

Field names may contain the complete component namespace, such as `example.spin.speed`. Inspector writes actual component fields that the executable system reads after saving and reopening. The source-installed `example.spin` component is an example: its **Radians per second**, **Axis**, and **Enabled** control object rotation.

Inactive data is retained in the authored project. It is hidden from the ordinary inspector and available in the explicit **Advanced: raw components / overrides / references** section. Here, entire maps can be edited and custom fields or object references added. **Apply raw JSON** validates the candidate; malformed JSON or an invalid scene does not replace the saved source.

**Create template from selected** creates a linked prefab from the object's current resolved fields and color. Position, parent, ID, and references remain with the instance. The name field is optional: when empty, the editor chooses an available name. Other objects receive the prefab through the **No template / template name** list.

**Linked template source** edits defaults; **Apply template defaults to instances** updates all linked instances. An individual instance edit is written into `overrides`, for example:

~~~json
{
  "prefab": "small-monster",
  "components": {},
  "overrides": {"health.maximum": 80.0},
  "references": {}
}
~~~

A new prefab maximum health therefore affects the other instances, while this one retains 80. Vector overrides use indexed paths, such as `material.base_color.0`: changing one coordinate preserves inheritance for the rest. **Inherit defaults** removes overrides and local fields of the selected component, restoring prefab defaults. Switching or removing a prefab preserves explicitly specified instance data; on removal, the current resolved appearance becomes ordinary object components.

<a id="свои-ресурсы"></a>

## Your own assets

Open **Windows → Assets**. The browser displays folders and files and supports search, filtering, previews, import, and file management. A model dragged into Scene creates a new object. A texture dragged onto an object's geometry assigns `material.base_color_texture`, preserving tint and other fields. Replacing an existing object's model is an explicit Inspector action.

**File → Import Asset…** or **Import** in Assets opens the native picker for glTF/GLB, PNG/JPEG, or WAV. Models are copied with dependencies. Import does not assign an asset automatically: use drag-and-drop or Inspector. Import refreshes the index and previews. WAV is for game audio cues and is not a mesh/texture.

Import uses a separate folder to avoid overwriting existing assets. New assignments store `asset://ID`; old relative paths are compatible. **Refresh** rereads files and updates caches. Rename/move preserve IDs and known references. Saving, deletion, Undo, and trash rules are in [scene-assets.md](scene-assets.md).

Example data:

~~~json
{
  "mesh": {"path": "assets/models/demo_crystal.glb"},
  "material": {"base_color": [0.8, 1.0, 1.0, 1.0], "base_color_texture": "assets/textures/stone.png"}
}
~~~

Actual glTF, texture, and GPU-material ABI limitations are described in [content.md](content.md) and [gpu-hlsl.md](gpu-hlsl.md).

<a id="порядок-систем-и-typed-pins"></a>

## System order and typed pins

The **Gameplay Systems** window, titled Executable systems, is available to the Engine author. **Stages, order and declared resources** lists stages and nodes in exact execution order. **Earlier / Later** changes order; dragging a node on the canvas changes only its screen position. New stages, stage/node deletion, and typed resources are created here too. Palette adds an actually installed module to the last stage.

`after` consists of separate explicit constraints by instance ID. Moving does not erase them. Correct constraints in JSON or explicitly click **Replace after constraints with displayed linear order**: this command replaces the entire `after` set with a sequential chain. Type checking and checking resource reads before their first producer remain mandatory even after this command.

The canvas displays external resources and actual descriptor ports. A connection assigns a named resource to an input. Input reads the latest preceding value of that resource in authored order; ReadWrite then publishes a new one. This is a graph of named resources rather than a separately saved copy for each line. Create different resources for independent values. Incompatible types, missing ports, and attempts to connect an output to an earlier node are rejected.

Custom Rust participates through required world/input/events: GameWorld ReadWrite, GameInput Read, GameEvents ReadWrite. `params.source_id` selects the source; `params.components` adds component names that this source uses. After successful Apply, their saved fields appear in the ordinary inspector. These connections do not mean all Rust code is already represented as visual operations.

The selected node displays params JSON and lists of compatible resources for each binding. **Update draft params** updates only the draft. The complete `ExecutionGraph` text is under **Text graph: resources / stages / bindings / params**. It exposes `initial`, all bindings, and after. **Load JSON into draft** synchronizes the text with the visual draft. Until modified text is loaded, general Apply is disabled: the editor will not apply the old graph instead of the entered text.

~~~json
{
  "resources": [{"name": "input", "data_type": "GameInput", "external": true, "initial": null},
                {"name": "world", "data_type": "GameWorld", "external": true, "initial": null}],
  "stages": [{"name": "Controllers", "nodes": [{"id": "controller", "module": "game.input",
    "params": {}, "bindings": {"input": "input", "world": "world"}, "after": []}]}]
}
~~~

Changes initially live in a local draft: a required pin may be temporarily disconnected, a resource renamed, or several nodes restructured. **Apply graph draft** validates the full graph, source scenes, and whether an execution candidate can be created. Only a successful candidate replaces the document and runtime. An invalid candidate remains editable; the previous graph continues working. **Discard draft** restores the saved graph. Save/Export requires applying or discarding the draft before proceeding; an unfinished draft is not retained between editor launches. Opening another project and exiting treat it as unsaved work.

Undo/Redo, JSON application, checkpoints, and merge do not replace the document over an unfinished Gameplay Systems draft. First use **Apply graph draft** or **Discard draft**. Save checks for this draft before applying JSON so that another graph cannot destroy unsaved work.

Canvas coordinates are personal settings, written separately into `.user/full/game-systems-positions.json`; they are not part of the game's ExecutionGraph.

<a id="rust-custom-во-время-play"></a>

## Rust Custom during Play

Select a connected `source_id` and click **Apply selected game Rust**, or use ordinary Apply in the Rust editor. Compilation runs in the background; active instances continue working. Compatible state is retained. For an incompatible schema, explicit **Apply rejected version + reset its state** or the **Apply version with migrated state** JSON panel is available. Entered state is applied to all instances of the selected source ID after validating the new version; rejection leaves the previous runtime active.

In interactive Game and Player, a single persistent `GameWorker` owns the actual `GameHost`: it runs systems, Custom, commands, rendering, and GPU readback outside the UI. The editor reads only the latest completed `Arc<GameSnapshot>` with game state, camera, Custom versions/states, and an RGBA frame. The UI does not lock `ScriptSlot`, wait for update completion, or substitute an approximate copy for game state. The retained last successful image has its tick/revision and completion time recorded separately: a new rendering error does not present old pixels as a new frame.

The 150 ms timeout remains the limit for one Custom call. The worker checks commands between fixed updates, running at most one actual tick per iteration; unprocessed time is retained with the previous total cap of 0.25 s. Slow Custom therefore slows game simulation but not editor window handling. Diagnostics expose tick duration and accumulated time. Pause, Restart, and Stop may wait for an already started worker tick; the UI does not wait for them to finish. A tick error retains the last accepted state and stops retries until Restart or an explicit successful application of corrected Custom.

The input queue is bounded: movement/aim retain the latest value, while presses receive sequence numbers and are acknowledged after an accepted tick. A full queue returns the unsubmitted packet to the calling UI for retry; it is not considered accepted. Restart begins a new epoch, explicitly cancels the previous press queue, and rejects compilation results from the previous epoch. A complete engine replacement creates a new actor with a separate snapshot channel; a late result from the old one cannot replace its state. Dropping an actor merely signals shutdown: its thread destroys the GPU and processes without a UI join.

After the native window closes, `main` calls `finish_after_ui`: outside the UI loop, it first signals all still-live owners and then waits for them to finish without holding the registry lock. This lets `GameHost` destroy persistent Custom processes before the main process exits; closing the application during a busy tick does not leave a running child Custom without an owner. Completed owners are removed from the registry, so ordinary editor use does not accumulate handles.

The basic WAV endpoint on Windows is process-wide, so audio start/stop is protected by a separate owner ID. Late shutdown of an old actor stops only its own current voice; it does not mute audio already started by a new Game. Ownership checking is serialized with the short asynchronous `PlaySoundW` call. Stop also sets an atomic retirement flag without UI blocking: an old tick that finishes checks it under the endpoint lock and no longer starts a delayed cue over the new actor's audio.

Direct synchronous `GameHost::step` and `GameSession::step` remain for CLI/tests and still allow 15 catch-up updates. The diagnostic `cargo test -p module-engine-editor audit_slow_custom_catchup_wall_time -- --ignored --nocapture` measures this path with 8/80 ms per update at `dt=0.25` and writes `.work/slow-custom-catchup-audit.json`.

<a id="проверка-авторских-действий"></a>

## Verifying authoring actions

`cargo test --locked -p module-engine-editor game_ui::tests` checks scene duplication, linked-prefab creation, overrides after save/reopen, preservation of inactive data and tint when assigning a texture, restoration of a disconnected typed pin, and rejection of invalid order. A headless egui test draws the actual graph UI with an incomplete draft and checks that the saved document stays unchanged. This verifies UI behavior without claiming visual quality at every window size.
