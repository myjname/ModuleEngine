<a id="scene-и-assets"></a>

# Scene and Assets

[English](scene-assets.md) | [Русский](ru/scene-assets.md) | [Documentation](README.md)

Separate **Scene**, **Hierarchy**, **Inspector**, and **Assets** windows are used to arrange an authored scene. The **Game** window runs the game's Play copy; its camera and controls remain game-specific. Windows can be opened from Windows and placed in the editor layout.

![Authoring windows, gizmo, and asset browser](media/editor-scene-assets.png)

<a id="расстановка-объектов"></a>

## Arranging objects

1. Select the authored scene and an object in Hierarchy or by clicking its geometry in Scene. Hierarchy, Inspector, and Scene share selection.
2. Select **Move [G]**, **Rotate [R]**, or **Scale [S]** in Scene. Drag the corresponding handle with the mouse.
3. **World** uses world axes; **Local** uses object axes. **Snap** enables position, angle, or scale increments for the selected tool.
4. Releasing the left button completes one gesture and creates one Undo entry. **Escape** cancels an unfinished gesture. **Ctrl+Z / Ctrl+Y** undo and redo completed changes.
   Save, Open, Undo, and starting Play also cancel an unfinished drag before performing their action; release the handle first to preserve the new position.
5. Numeric transform values and component fields can be edited in Inspector. Save the authored document with Ctrl+S.

The first version selects one object. Simultaneous group changes and a shared group pivot are deferred. G/R/S/F apply to the active Scene; text fields retain their input.

The Scene camera is free for inspection: **middle button** orbits the view, **Shift + middle button** pans, the **wheel** changes distance, and **F** focuses the selected object. This is personal view state rather than a game-camera change.

Authored edits are made after Stop. Changing Play state does not write killed monsters or the player's position into the source scene.

<a id="проводник-ресурсов"></a>

## Asset browser

Assets displays project folders, search, a type filter, sizes, and selected-file details. Models and textures have previews; other files remain accessible as metadata entries. Errors or unsupported content are shown as diagnostics instead of substituting another model.

1. Open **Windows → Assets**. Select a folder on the left; the path bar at the top returns to its parent. In a narrow area, a dropdown replaces the folder list, and asset details appear below the file list.
2. Switch `Плитки / Список` (Tiles / List) and the `Все типы / Модель / Текстура / Звук / Шейдер / Другой файл` filter (All types / Model / Texture / Audio / Shader / Other file). Without search, files in the current folder are displayed; search also checks its subfolders. Each Assets window retains its own folder, search, and asset selection.
3. Click a file: its path, size, details, and the `Копировать ссылку` (Copy reference) button appear on the right. File selection does not change the selected scene object. Models show part, vertex, and triangle counts; WAV files show header information.
4. `Импорт…` (Import…) opens the system file picker and adds an asset to the selected `assets/` folder. `Новая папка…` (New folder…) creates a child folder. `Обновить` (Refresh) rereads files and dependencies after an external change.
5. Use the file context menu or the selected asset's `Переименовать…` (Rename…), `Перенести…` (Move…), `В корзину…` (Move to trash…), and `Использования` (Usages) buttons. A file can also be dragged onto a folder: this requests a move with the same checks and saving. Folder names provide navigation; bulk renaming or folder moves are not performed.

Previews load through a queue on a separate thread. The browser does not create a game host or Vulkan device for each file, and retains loaded thumbnails in a bounded cache. Textures display their pixels with aspect ratio preserved. Models get an image of loaded geometry with the material's base color; textures and every game-rendering pass are not reproduced in that thumbnail. `Превью загружается…` (Preview loading…) changes to an image or an error reason; `Повторить превью` (Retry preview) retries loading. Audio currently has details without a built-in player.

Opening Assets for the first time indexes project files and assigns missing IDs. A manifest change marks the project unsaved; Ctrl+S saves the new IDs. Initial ID assignment is treated as a metadata migration: existing scene Undo/Redo is retained, and undoing a move neither removes nor recreates IDs. Indexing does not repeat on every coordinate change or frame. During Play, including Pause, asset changes require Stop. An unfinished JSON draft must be applied or cancelled first.

File reading, hashing, and catalog construction run on a worker thread. Assets displays an indexing indicator; other editor windows remain available. Browser file operations are locked until scanning completes. A result is accepted only for its original folder and current index, and scene edits made during the scan are preserved. Back references for all files are computed by a shared document traversal rather than a separate traversal per tile.

Import supports static glTF/GLB, PNG/JPEG, and basic WAV. A model is copied together with external buffers and images. Every file receives a persistent ID; new asset assignments use `asset://ID`. Old relative paths remain compatible.

Dragging a model from Assets into Scene creates a new object at the point under the cursor; it does not replace the model of an object already there. The selected object's model can be explicitly replaced through Inspector. Dragging a texture onto an object assigns it to the object's material.

Rename and move change one file and its known references. Folders support navigation and creation; moving an entire folder tree is not this operation. An occupied destination, a path outside `assets/`, a protected backend, or an unresolvable dependency produces an error before assets change.

<a id="удаление-и-восстановление"></a>

## Deletion and restoration

If an asset is in use, the browser shows its usages. To delete it, first select a replacement or explicitly unlink references. Objects remain in the scene. If the file is an internal model or shader dependency, unlinking is forbidden: a suitable replacement asset or prior deletion of the consuming file is required.

The original is retained in the project's trash. Restore returns the file with its previous ID and does not undo later scene edits; a full deletion Undo also restores previous references. Restoration refuses to replace an existing file or an occupied ID.

The file change and references must remain consistent on disk. Therefore the editor saves the current project before renaming, moving, and deleting, then saves the new document. Cancelling saving cancels the operation. A second-save error rolls back files; Undo/Redo of a file operation also saves the corresponding document.

Open the `Корзина` (Trash) tab in Assets and click `Восстановить` (Restore) beside the required file. If the original name is occupied or a required dependency is missing, the editor shows the reason and preserves the current project. **Escape** or `Отмена` (Cancel) closes rename, move, and delete dialogs without a file operation.

Checkpoints and document merging do not move historical files automatically. Before restoration, the editor checks paths, IDs, and hashes of recorded assets. A version with an old path after a move, or with a deleted asset, is rejected before changing the document or history position. First undo the file operation through Undo or restore the asset from trash. File Undo itself returns the files and corresponding references, so after that rollback the check accepts the previous version.

<a id="что-обновляется-автоматически"></a>

## What updates automatically

Stable IDs survive file moves. Known old paths are updated in scenes, prefabs, overrides, and archived data. Relative glTF/GLB URIs and literal HLSL includes are recalculated. Custom source code, source identifiers, and module names are not rewritten as paths.

For Rust Custom, use `ctx.asset_path("ID")`, then pass the resulting path to a file or content API. Arbitrarily constructed Rust paths cannot reliably be corrected automatically. A literal old path or computed file access without the ID API blocks move/delete and identifies the problematic Custom. Macro includes and unknown glTF extensions also require explicit resolution rather than a silent move.

The full contract, snapshot storage, extension API, and checks are described in [asset-workspace-api.md](asset-workspace-api.md). The installed Engine and game folder continue to have separate path bases; see [editor-workflow.md](editor-workflow.md).

<a id="откуда-возникала-задержка-при-изменении-координат"></a>

## Why coordinate changes caused delays

A transform change should pass the existing object's new position while retaining decoded geometry and render executors. Rebuilding the host, reloading models, or running a full Assets hash traversal on every coordinate change makes dragging expensive.

This stage's verification scenario: continuously move/rotate/scale an object, observe feedback in Scene, complete the gesture, undo and redo it, and verify a single history entry and the saved scene. Timing is separate from functional verification: without measuring specific content, a fixed FPS or latency cannot be claimed. Architecture changes, assignment of a different asset, and external changes to its bytes may still require refreshing the corresponding caches.
