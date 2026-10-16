<a id="история-объединение-и-ресурсы"></a>

# History, merging and assets

[English](history-assets.md) | [Русский](ru/history-assets.md) | [Documentation](README.md)

<a id="undoredo-и-контрольные-версии"></a>

## Undo/redo and checkpoints

The History tab works with GraphDocument snapshots: graph, Rust/HLSL, parameters, layout, manifests and authored game scenes/templates/execution. Undo/Redo and Ctrl+Z/Ctrl+Y restore the document. The editor retains up to 200 changes; a new edit after undo discards the redo branch. Pending File JSON must first be applied or cancelled. A local ExecutionGraph draft is not saved history until Apply succeeds.

In-memory history belongs to the currently open document and resets when another file opens. Save checkpoint writes a complete snapshot to .history next to the project JSON. The entry name is unique; an existing version is not overwritten. Restore returns the document and itself becomes an undoable change.

Assets-window operations, including import, folder creation, rename, move, file deletion and restoration, have their own history boundary even without a JSON change. Undo/Redo restore files and document together, checking current file contents before rollback; the result is saved. If an external tool changed a file after the operation, the editor refuses to overwrite it through history. A completed Move/Rotate/Scale gesture also creates one entry; Escape cancels an unfinished gesture.

A checkpoint or merge contains the document, not a complete historical snapshot of all files. Asset paths, IDs, hashes and dependencies are checked before application. A version with a missing file, old path after a move or deleted ID is rejected before document and history changes. Restore required files first through file Undo or the Assets trash. Rust/HLSL sources are not overwritten automatically when restoring a version. Details: [Scene and Assets](scene-assets.md) and [file-transaction API](asset-workspace-api.md).

Time, live object state, GPU resources, owned Custom runtime state and worker globals are not recorded. Restoring an authored document does not rewind Play. Legacy compatible edits may preserve an existing world; changing its contract creates a new one. Changing the authored game scene or shared ExecutionGraph requires Stop; compatible Rust/HLSL hot apply has a separate live-state preservation contract.

<a id="diff-и-трёхсторонний-merge"></a>

## Diff and three-way merge

In the History tab, specify a common original version (base) and another branch's modified JSON (theirs); the current document is ours. Compare with current project shows change categories and paths. Prepare merge builds a MergeResult without writing the source files.

Ordinary Graph nodes are matched by ID within their scope; edges by target node/input. Two different reconnections of the same input therefore conflict, whereas edits to different nodes may merge. Nested Graphs are handled the same way. For game.scenes[*].objects, ExecutionGraph stages/nodes and other ordinary arrays, element-wise ID-aware merge is not yet available: independent edits to the same array in both branches conflict at the whole-array level with base/ours/theirs. Different fields/maps of a scene or template can merge through ordinary object merging. Nonoverlapping Rust/HLSL lines merge; overlapping ones are retained as a source conflict. This does not prove program or scene correctness.

Each conflict stores path, base, ours and theirs. The preliminary document retains the base value; for conflicting new nodes it is absent. Both new versions are preserved in full in the conflict report. Select Keep current, Take other or Restore base. Applying the result becomes available only after every conflict is resolved; ordinary graph validation and compilation then check executability.

Save result with conflicts creates a new merge-*.json with the document and conflicts. This is a MergeResult report, not an ordinary GraphDocument: do not open it as a project. The UI cannot yet reload the report; the library has a serde representation and resolve for a merge-continuation tool. Original files remain available.

Library functions: diff_documents, merge_documents, MergeResult::resolve and ConflictChoice::Custom for a custom value. Duplicate IDs are rejected so normalization cannot lose nodes. After a merge, nodes-array order may change; IDs define connection semantics. Live coediting, locks, server synchronization and automatic asset merging are not implemented.

<a id="ресурсы"></a>

## Assets

AssetId is a stable string manifest key. The path is relative to the project directory, SHA-256 pins the content, and dependencies contains IDs of other registered assets. asset_manifest:null means an empty manifest. Structure:

~~~json
{
  "schema_version": 1,
  "assets": {
    "mesh.main": {
      "path": "resources/model.bin",
      "sha256": "<64 hexadecimal characters>",
      "dependencies": []
    }
  }
}
~~~

Here the hash is a placeholder that must be replaced with the real file's hash. The AssetManifest::register API computes it from the file; verify checks bytes and dependency references. The Assets window indexes project files, assigns missing IDs and supports asset management; new assignments use `asset://ID`, while old relative paths remain compatible. The full manifest is also editable through JSON or the source API. Export additionally gathers transitive resources of game scenes.

Absolute/drive paths and traversal above the root are rejected. Canonical resolution forbids symlinks leading outside the project. Relative .. within the root is allowed. Export does not replace an existing file with different content; changing an asset after registration requires an explicit hash update.

<a id="что-сохраняет-экспорт"></a>

## What export preserves

The full source package contains crates, docs, examples, assets, scripts, tools/dxc with DLLs and licenses, Cargo.toml/Cargo.lock, toolchain-manifest.json, .gitignore and the current project.json. .work and target are not copied. Registered external assets are copied to the same relative paths. HLSL include dependencies are collected recursively and added to the export asset_manifest with ID, relative path and SHA-256; the source manifest is unchanged. One relative path with different bytes produces an error. Cycles terminate; include macros are rejected.

The scoped exporter takes asset_root and package_root separately: registered resources are relative to JSON, native sources/inline HLSL to the selected Full package. Include roots: package/assets/gpu, then package root; a nested quoted include first searches the containing file's directory, while an angle include searches only include roots. Standalone JSON without a package marker does not change compiler context. A source ID is not treated as a filename.

Team export preserves the ready build, allowed gameplay and .sdk Core, removing the inactive archive/unused authored fragments. Backend pins lock Engine HLSL/includes and DXC, leaving ordinary game assets outside the immutable backend inventory. [Distribution](distribution.md) explains the source manifest, profiles and SDK.
Export requires a new or empty folder outside every copied tree: crates/docs/examples/assets/scripts/tools/.sdk/.user/.tools. An incomplete result remains on error for diagnostics; retry requires a new folder or one explicitly cleared by the user. Source files are not deleted.

Saving existing JSON creates a unique .backup-* copy of the original bytes; loading schema 1/2 migrates only in memory. Missing module_versions are filled with installed-registry versions on write/export. A conflicting pin is preserved and produces compatibility diagnostics; the exporter does not fix it by selecting another version.

<a id="инструменты-и-восстановление"></a>

## Tools and restoration

toolchain-manifest.json pins Windows x86_64 MSVC, Rust 1.99.0, rustup 1.29.1 and DXC release v1.9.2609 / executable 1.9.2609.5. Cargo.lock pins Rust dependencies, including the worker. The rustup-init hash from the [official release checksum](https://static.rust-lang.org/rustup/archive/1.29.1/x86_64-pc-windows-msvc/rustup-init.exe.sha256), along with DXC archive and component hashes, is pinned in the manifest. Restore checks downloaded bytes before execution/extraction.

~~~powershell
.\scripts\bootstrap.ps1 -Json
.\scripts\bootstrap.ps1 -Restore
~~~

The first call is a doctor: it checks versions, DXC hashes and Vulkan loader presence without downloading or installing. A missing tool produces JSON with ok:false and exit 1. The second call explicitly restores pinned Rust/DXC to the project's local .tools/tools and runs cargo fetch --locked for the root Cargo.toml or Team .sdk/Cargo.toml with cache .work/cargo-home. Rust Restore includes rustfmt/clippy; local cargo/rustc shims select the pinned toolchain. If local Rust was created, load . .\.tools\env.ps1 in a new PowerShell session.

MSVC build tools/Windows SDK and a GPU driver are host requirements. The script does not install them or prove adapter operation from the presence of vulkan-1.dll; GPU self-check verifies that. A complete offline SDK is absent. In 0.3, after an initial sandbox DNS failure, an authorized Restore retry downloaded 13 pinned dependencies into a new Team cache; subsequent SDK compile/render passed without external CARGO_HOME/CARGO_TARGET_DIR. This check used existing Rust/MSVC/driver. Doctor was checked with existing tools and with missing DXC; this check does not confirm cold installation of Rust/MSVC/driver. A separate ready Player does not restore tools and runs prebuilt shaders/workers.
