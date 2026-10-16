<a id="ассеты-идентичность-ссылки-и-файловые-операции"></a>

# Assets: Identity, References, and File Operations

[English](asset-workspace-api.md) | [Русский](ru/asset-workspace-api.md) | [Documentation](README.md)

This document describes the `module-engine-project::AssetWorkspace` contract, rather than a particular panel's design. The browser passes the authored `GraphDocument` and open JSON directory to the service; the service returns an updated document with a file-operation record. The editor needs the record for consistent undo: returning only JSON after renaming a file is insufficient.

<a id="постоянные-id-и-совместимость"></a>

## Persistent IDs and Compatibility

The index continues to use the existing version 1 `asset_manifest`:

```json
{
  "schema_version": 1,
  "assets": {
    "a-example": {
      "path": "assets/models/crystal.glb",
      "sha256": "...64 hex characters...",
      "dependencies": []
    }
  }
}
```

A new resource reference is `asset://a-example`. The ID is created once when the file is registered; moving it or changing its contents does not change the ID. The hash describes current bytes, and the path describes the current location. These are different responsibilities: the hash is not used as asset identity.

Old `assets/models/crystal.glb` references remain supported. Moving updates only `AssetReference` fields confirmed by the active installed contract and saved pins, and explicit built-in resource contracts (mesh, material, audio cues). This applies to scenes, templates, individual overrides, and active property stores, including nested objects and arrays. User fields named `id` or `module` are not excluded: the field type defines meaning. When replacing a deleted resource, the reference can be converted to the replacement ID.

Ordinary `Text`, program sources, and unknown, inactive, or incompatible archived data are not rewritten merely because a string matches a path. `properties.inactive` is preserved in full. An exact match in opaque data is returned in `AssetOperation.diagnostics`; the editor displays a warning and full report. Before reenabling such a contract, the author explicitly checks this data. This is not a promise to automatically repair arbitrary paths computed by code.

Resolution API:

```rust
let manifest = AssetManifest::from_value(&document.asset_manifest)?;
let relative_path = manifest.resolve_reference("asset://a-example")?;
let paths = manifest.asset_paths();
content_library.set_asset_paths(paths.clone());
script_context.asset_paths = paths;
```

`ContentLibrary` resolves IDs before reading a model or standalone texture. An unknown ID produces an error. Changing the path table clears the decoded-resource cache; setting an unchanged table preserves it. After changing bytes at the same path, call `invalidate()`.

Rust Custom obtains a path through the context:

```rust
fn read_asset(ctx: &ScriptContext) -> Result<Vec<u8>, String> {
    let path = ctx.asset_path("a-example")
        .ok_or("Asset is not available")?;
    std::fs::read(path).map_err(|error| error.to_string())
}

fn update(ctx: &mut ScriptContext) {
    match read_asset(ctx) {
        Ok(bytes) => ctx.state["asset_size"] = serde_json::json!(bytes.len()),
        Err(error) => ctx.state["asset_error"] = serde_json::json!(error),
    }
}
```

`update` returns `()`, so `Result` handling remains in the helper and `match`. The example shows ID resolution and reading; in a game, load and cache the resource during initialization rather than reading it again every frame. `asset_path` also accepts `asset://a-example`. The path is relative to the game directory, which is the worker's working directory. The `ScriptContext.asset_paths` field has `serde(default)`: an old packet remains readable and receives an empty table.

<a id="область-владения-и-каталог"></a>

## Ownership Scope and Catalog

`AssetWorkspace::new(root, protected_relative_paths)` canonicalizes the project root. The catalog scans `assets/` and additional files already declared in the manifest. External model and HLSL dependencies are included in the index. Protected files/directories are excluded from the browser; operations cannot change them or rewrite a dependency inside a protected backend. When the game and installed Engine share a directory, the editor passes, for example, `assets/gpu` as a protected area.

By default, preflight checks scene structure. The host passes `workspace.with_composition(composition)` to determine reference types and activity, while `with_validator(|document| ...)` validates the candidate before snapshots, trash, and modified files are written. Types cannot be reconstructed from the validator callback. Without composition, only explicit built-in render contracts are available; source components remain opaque. History replay returns previously recorded document and file boundaries.

`AssetReference` values obtained only from installed defaults are also included in usages with their source identified. This applies to `ComponentSchema` and to `ModuleDescriptor.properties` / `PropertySchema` of all four scopes. If Move/Delete/Replace/Detach would require changing an immutable default, the operation is rejected before files and the log are written. A stable `asset://ID` allows Move with the same ID. An explicitly permitted null suppresses the default; Detach cannot silently reintroduce a dependency through a default.

For Object and legacy Component properties, the host also passes `with_property_object_ids(live_ids)`: these are the actual IDs of the accepted graph Runtime, not authored Game-scene IDs or property-store keys. The service itself does not run world factories for scanning. An empty set means there are no live objects; a missing snapshot with potentially applicable scoped asset contracts produces a context error. Editor checks that the current world source matches the accepted runtime and passes the same snapshot to the catalog and file operation. Backend and snapshot participate in background catalog freshness validation.

The result of `synchronize(&mut document)` is `Vec<AssetInfo>`: ID, path, type, size, missing-file flag, dependency IDs, and a diagnostic message. Unsupported or damaged files remain visible with diagnostics. Unknown extensions are shown as `Other`; the service does not promise to decode their contents.

Scanning computes hashes and reads dependencies. It therefore runs on opening, import, explicit refresh, and file operations; it is not work for every Scene frame or object-drag step. Moving an object changes its transform and does not require reimporting or rescanning resources.

The UI performs `synchronize` in a worker thread and accepts only results for the current index. `usages_batch(document)` is provided to display usages of all files: it returns an ID → `AssetUsage` list table and serializes the document once. `usages(document, id)` remains a convenient single query; do not call it in a loop over tiles.

Inside `assets/`, the scanner skips directories with the reserved `.module-engine-game-export-` prefix: these are unpublished Player packages that may disappear before scanning finishes. Ordinary user and hidden directories continue to be indexed.

Paths normalize to `/`. Absolute paths, escaping the root, external symlinks/junctions, and directory cycles/repeated aliases are rejected. A file-move destination must be inside `assets/`, outside protected areas, and not yet exist. Case-only renaming is not separately supported.

<a id="операции-и-зависимости"></a>

## Operations and Dependencies

| Method | Result |
| --- | --- |
| `move_asset(document, id, destination)` | Moves or renames one file, preserves its ID, updates references and relative dependencies. |
| `delete_asset(document, id, RefuseUsed)` | Deletes only an unused resource; reports reference locations if used. |
| `delete_asset(document, id, Detach)` | Explicitly removes structural references while preserving objects; puts the source in trash. |
| `delete_asset(document, id, ReplaceWith(other_id))` | Replaces references with a resource of the same kind and moves the source to trash. |
| `create_folder(document, path)` | Creates one directory inside an existing asset directory; the first call can also create the `assets/` root. |
| `adopt_import(document, fresh_paths)` | Registers only new files of a unique import, including dependencies, and creates an Undo/Redo record. |
| `usages(document, id)` | Returns structural-reference locations, usages in models/HLSL, and available inline includes. |
| `list_deleted()` / `restore_deleted(document, trash_id)` | Lists trash and restores a file with its previous ID. |

glTF/GLB: literal buffer and image URIs are rewritten. Moving the model itself recalculates relative URIs from the new directory; moving a dependency updates referring models. GLB rebuilds its JSON chunk with alignment and a new size, preserving other binary chunks. Embedded `data:` does not create a file dependency. Percent escapes are parsed before ownership validation, and the updated URI is encoded correctly.

HLSL: literal `#include "..."` and `<...>` are rewritten while preserving the rest of the line. Macro-dependent includes are rejected before file changes. For `"..."`, search starts in the file's directory, then checks `assets/gpu` and the root; for `<...>`, only `assets/gpu` and the root. Inline shader source uses the latter two areas. Inline source can depend on a separately installed backend: an include unavailable in the game directory is not declared game-owned.

Move support does not extend to unknown glTF extensions, remote URIs, dynamic include selection, or arbitrary dependency formats. The service reports an error before changing files. Detach is prohibited for a resource needed inside a model or shader: deleting a URI/buffer/include would damage the file. A compatible replacement or prior removal of the consuming resource is required instead.

Arbitrary Rust cannot be reliably rewritten through string search. If Custom contains a literal old path or computed file access without `asset_path`, moving/deleting is rejected with Custom identified. Mixed code that uses the ID API while also constructing other paths manually still requires author review: a helper's presence does not turn Rust into a fully analyzable dependency description. The service does not modify program text itself.

<a id="документ-файлы-и-undoredo"></a>

## Document, Files, and Undo/Redo

Each file operation returns:

```rust
AssetOperation {
    document: updated_document,
    record: AssetOperationRecord { /* before/after documents + file snapshots */ },
}
```

The record stores exact before/after documents, a list of changed paths, file-snapshot hashes, and created directories. Snapshots and the operation description live under `.asset-history/<operation-id>/`. The service checks paths and all changes in advance, writes backup bytes, applies files, and restores already affected paths on error. If rollback also fails, the error explicitly contains that information; this state cannot be declared successful.

The editor binds `record` to one shared-history boundary. Undo calls `record.undo(root, current_document)`; Redo calls `record.redo(...)`. They check the corresponding document and actual file hashes before application. If a file changed outside the editor, the operation is rejected rather than overwriting the external edit. After successful replay, the restored document is saved by the same editor.

Directory creation does not change JSON: its `before == after`. Ordinary `SnapshotHistory::record`, which discards equal documents, is therefore unsuitable for such a boundary. Shared history must explicitly retain a file record even with identical JSON. Import removes/restores its files but does not recursively remove remaining empty import directories.

The service itself does not save the open game JSON or manage the unsaved-changes prompt. The editor first finishes saving the current project, then invokes the operation and saves the new document. If the second save fails, it uses the record to roll back files. On-disk records make it possible to establish what changed and restore bytes; this API does not claim to implement automatic recovery after a crash midway through a combined file/JSON save.

File replacement first writes and flushes a temporary file, then replaces the original directory entry through rename. The source is not deleted in advance. If adopting a fresh import cannot write snapshots or `record.json`, only the marked log created by that attempt is removed; the calling workflow separately removes its new import directory. Logs of ordinary file operations are preserved for recovery if rollback fails.

<a id="корзина"></a>

## Trash

On deletion, the original is additionally preserved in `.asset-trash/<operation-id>/files/<old-path>` with `record.json`. Restoring through trash returns the file and persistent ID without undoing later authored reference decisions. Full Undo of deletion restores original references as well. An occupied path/ID or missing required dependency blocks restoration.

Trash and file history are outside `assets/` and do not appear in the ordinary resource catalog. Restoration does not use recursive directory deletion or replace existing user files.

<a id="проверки-контракта"></a>

## Contract Checks

`asset_workspace` tests check ID preservation after byte changes, protected and missing resources, moving glTF with external files, GLB binary chunks, URI/include dependencies, templates and archived references, resource replacement, detach, trash, file Undo/Redo, import, first-directory creation, and rejection after external edits. A separate Windows check triggers a write error through a read-only dependency and checks restoration of originals. `engine-content` checks loading through IDs and invalidation of stale cached paths; `engine-core` checks serialized-context and helper compatibility.

```powershell
cargo test -p module-engine-project asset_workspace
cargo test -p module-engine-content stable_ids
cargo test -p module-engine-core stable_asset_paths
```
