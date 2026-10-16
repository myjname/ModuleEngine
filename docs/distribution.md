<a id="full-team-и-отдельный-player"></a>

# Full, Team and a separate Player

[English](distribution.md) | [Русский](ru/distribution.md) | [Documentation](README.md)

The Full source package is intended for the engine author; Team is for the game author with a selected architecture. Both deliveries contain the editor. Player is a separate runtime export without editor windows, SDK or compilers; its format and launch are described in [player.md](player.md). Source availability is separate from compiler availability.

.module-engine-package.json contains schema_version:1, engine_version:0.4.0, profile, files, module_sources, sdk_files, backend_files and, for Team, gameplay_source_ids/blueprint_hash. files is a relative path → SHA-256 baseline of native sources. Connected nodes and engine.enabled_modules are checked.

Engine is available only to Full with a complete Cargo/native Rust/HLSL inventory and matching workspace.package.version. Two Cargo/lib files do not constitute a source package. Changed sources are marked modified and remain available. Missing DXC/Rust is diagnosed separately; docs/examples/tools do not determine SourceStatus.available. --seal-source-project <folder> deliberately updates the baseline; export does this itself.

The compile-time original workspace does not grant author access. A marker near the executable/project or in the current directory is selected. The Team marker takes priority and rejects --profile full. An .sdk without a marker is rejected without borrowing sources from the original installation.

## Frozen blueprint

The hash pins the Engine graph, bindings/source IDs, exposure/ranges/variants, shaders, module pins, game-system execution graph and Engine Rust. Allowed Rust consists of connected custom.rust.update sources, including game systems; legacy scalar speed belongs to Engine. The source key set and bindings are immutable. Positions/labels of ordinary Graph nodes and gameplay code_layout/code_layout_keys are excluded from the hash. ExecutionGraph has no coordinates: its stages, nodes, parameters, after and bindings are frozen for Team. Authored scenes and active templates remain editable game data.

Project settings, properties and gameplay text are editable within the schema. Core checks ranges, participation and commands; UI/CLI check the blueprint on load/apply/save. A local detach preserves the archive for undo/attach, but runtime cannot see it. Team and Player use a shared runtime projection: they recursively remove unconnected working nodes, preserve required interface mappings of nested graphs and all scheduled game systems. Connected disabled passes remain available for allowed switching. Unused working Rust/HLSL sources are excluded before searching for include files; Team preserves the declared game-asset library, while Player selects resources for its game.

The source document is validated before projection. The published Engine profile, its archived blueprint, enabled modules, module versions and exact locks are preserved without republication. The Team stamp pins the resulting working graph. New Project → Inherit Current inherits that graph and its source maps: archived profile nodes do not return to the new working project. Changing Engine composition remains unavailable to Team.

Backend pins strictly check the executable, tool manifest, DXC/DLL/licenses, assets/gpu and recursive literal HLSL includes, including headers outside assets. Ordinary game assets are not frozen by this map: their AssetManifest defines paths/hashes.

This is a working editing policy, not a security sandbox or a cryptographically signed delivery. A user with file access can modify the program; Custom Rust has ordinary OS privileges.

<a id="экспорт-и-sdk"></a>

## Export and SDK

Full export is the entire workspace with a new manifest. Team contains:

~~~text
ModuleEngine.exe
project.json
.module-engine-package.json
toolchain-manifest.json
run.ps1
scripts/bootstrap.ps1
assets/gpu/...
tools/dxc/... DLL + licenses
.sdk/Cargo.toml
.sdk/Cargo.lock
.sdk/crates/engine-core/...
~~~

.sdk contains the Core contract for compiled Custom, without native editor/Vulkan authoring packages. The SDK lock is normalized from the full Cargo.lock and checked as a subset with the same name/version/source. The compiler takes tools/cache from the package root and sources from .sdk. An incomplete SDK produces an error without falling back to original E. An explicit Team profile on Full also requires its own SDK for compiled Custom.

Export is scoped to the selected Full source/resource root; registered assets are resolved relative to the open JSON. Writing inside copied trees/over a nonempty directory is forbidden. project.json, marker, executable, .sdk and cache paths are reserved. An incomplete result is retained on error for diagnostics.

~~~powershell
.\scripts\bootstrap.ps1 -Json
.\scripts\bootstrap.ps1 -Restore
.\ModuleEngine.exe
~~~

Restore selects Full Cargo.toml or Team .sdk/Cargo.toml. Portable Rust lives in .tools, and the dependency cache in .work/cargo-home. Host MSVC/Windows SDK and a GPU driver are required; the script does not install them. Restore permits network access; there is no complete offline toolchain SDK.




Historically verified in 0.3: automated Full/Team export with a manifest, relocated Team execution and reading a relative game asset through a working Rust worker; real source/profile cases for complete, modified, missing native/tool, Team policy and blueprint rejection. After the first sandbox DNS failure, a repeated network Restore with authorized access downloaded 13 pinned Cargo dependencies into its own new Team cache. The package then built Custom SDK and rendered the scene with CARGO_HOME/CARGO_TARGET_DIR cleared. This checks dependency downloads with Rust/MSVC/driver already installed, not a cold installation of the whole toolchain or a complete offline SDK. Results of the new 0.4 game stage are published separately in [Player](player.md) and [game verification](game-verification.md).
