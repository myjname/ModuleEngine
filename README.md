# ModuleEngine 0.4

[English](README.md) | [Русский](README.ru.md) | [Documentation](docs/README.md)

Start with the [current status](docs/current-status.md): implemented contracts, open goals, and the scope of recent checks. The [project working rules](AGENTS.md) define data preservation and check selection. Earlier stage reports linked below apply to their own source snapshots.

[Verification routes](docs/verification.md): explicit quick/contracts/native/package profiles, safe planning, and compatibility with previous verify calls.

A modular Rust engine with two workspaces. Engine defines the architecture; Project configures the game within the boundaries permitted by the engine. The native editor provides typed nested graphs, a real Vulkan 3D scene, and editable Rust/HLSL.

The mutable game world lives in ECS; Object remains a replaceable view through shared scene and command contracts. Forward/deferred paths execute geometry, lighting, SSAO, ACES/Reinhard, FXAA, and color encoding. Custom Rust is actually compiled; constructs the visual editor does not recognize remain plain text.

Version 0.4 adds a gameplay path: saved scenes and linked prefabs, a shared execution graph for systems, input/collisions/combat/events/audio, static glTF/GLB and textured materials, persistent Rust Custom state, and a separate Windows Player. The example contains two arenas, 20 monsters, and a boss. See the [stage goals and design](docs/next-stage.md), [game editor](docs/editor-game.md), and [Player launch and export](docs/player.md).

~~~powershell
.\outputs\ModuleEngine.exe game-project-v4.json
.\outputs\ModuleEngine.exe --play game-project-v4.json
.\outputs\ModuleEngine.exe --export-game game-project-v4.json outputs\my-game
~~~

![Project 0.4: Scene, Hierarchy, Inspector, and Assets](docs/media/editor-scene-assets.png)

[Stage 0.4 results: verification, performance, and remaining limitations](docs/release-v04.md).

[Functional audit and stability fixes](docs/stability-audit.md): reproducible scenarios, failure handling, Git history, and a complete rerun through `verify.ps1`.

[Additional authored-data fixes](docs/remaining-authoring-fixes-20261016.md): PropertySchema defaults in asset references, exact Inspector JSON keys, and preserved prefab inheritance.

[Subsequent Windows path and Save As fixes](docs/dialog-path-fixes-20261016.md): directory hint normalization, draft preservation on cancellation, and the scope of Windows/Linux checks.

[Linux preparation](docs/linux-support.md): WGPU/Vulkan, Wayland/X11, native DXC, and packages. Windows retains WGPU/DX12. The [current status](docs/current-status.md) records established and open boundaries with links to subsequent native checks; the [first Linux report](docs/linux-support-20261015.md) is retained as history.

[Component schemas and the shared registry](docs/component-schemas.md): Inspector fields, defaults, references, dependencies, and source module installation. [Game-world storage and object lifetimes](docs/ecs-world.md). [Architecture migration progress and verified boundaries](docs/modular-runtime-progress.md).

Melee combat now displays the aiming sector, targets, swing, hit flash, and damage numbers in Play and Player. See the [attack visual design](docs/combat-feedback.md).

The editor has **File / Edit / Tools / Windows** menus: new projects on the current Engine, native dialogs, Recent, Save As with assets, unsaved-change protection, Player verification and builds, window search, and personal layouts. Start with **File → New Project → Empty scene** or **Two arenas example**. See the [workflow and command purposes](docs/editor-workflow.md) and [UI stage results and checks](docs/release-ui.md).

Authoring is now split into **Scene / Hierarchy / Inspector / Assets**, with shared object selection. Scene supports click selection of geometry, translation/rotation/scale handles, World/Local, and Snap. Assets provides folders, search, import, previews, drag and drop, persistent IDs, rename/move, and managed deletion with a trash folder. **Game** runs a separate Play copy. See [using Scene and Assets](docs/scene-assets.md) and [assets and file history](docs/asset-workspace-api.md).

<a id="запуск"></a>

## Running

The primary Windows distribution runs from E:\ModuleEngine:

~~~powershell
.\outputs\ModuleEngine.exe
~~~

Building and editable Rust Custom require Rust/Cargo 1.99.0, MSVC C++ build tools, and the Windows SDK; HLSL compilation requires DXC. The GPU requires a compatible Vulkan 1.3 adapter/driver. An exported Player uses prebuilt workers and SPIR-V and does not require Rust/Cargo/DXC. Exact requirements are in [GPU/HLSL](docs/gpu-hlsl.md).

~~~powershell
.\scripts\bootstrap.ps1 -Json
cargo run --locked -p module-engine-editor
~~~

Doctor checks tools without downloads. An explicit `.\scripts\bootstrap.ps1 -Restore` restores pinned Rust/DXC and Cargo dependencies locally in the project. After creating the local Rust installation, source `. .\.tools\env.ps1` in a new PowerShell session. MSVC/SDK and the driver remain host requirements; a cold Rust installation has not been verified separately.

Formats 1–3 open through an explicit migration to another file: `ModuleEngine.exe --migrate-copy game-project.json game-project-v4.json arpg`; the original is preserved. See [Engine profiles](docs/engine-profiles.md).

Without an argument, the editor prefers the created game-project-v4.json copy, then game-project.json in the selected package/folder; if absent, it opens project.json. Older files require explicit migration, including the previous project.json with cubes and a floor. File → New Project creates a new JSON and assets on an explicitly selected or inherited Engine. In a runtime package, Player.exe without arguments opens the game from player.json. The editor CLI accepts a path and --profile full|team.

<a id="engine-и-project"></a>

## Engine and Project

| Distribution | Access |
| --- | --- |
| Full, complete compatible source manifest | Engine architecture, source, settings, and Project fields. |
| Full, incomplete/incompatible source | Project and diagnostics; Engine is unavailable. |
| Team | Project and permitted gameplay Update Rust. Engine is always unavailable, even alongside complete source. |

.module-engine-package.json checks the version, required Cargo/native Rust/HLSL files, and the inventory of active modules. Editing source Rust marks it as modified and preserves authoring access. A missing compiler is diagnosed separately and does not hide Engine. Native world/renderer/UI changes require rebuilding; hot apply covers graphs, HLSL, and Custom Rust.

Engine declares parameters, ranges, and predefined variants. Project changes permitted values and selection conditions for existing variants; it does not add or reorder passes. The demo provides exposure, ACES/Reinhard, and FXAA/off. Conditions compare a parameter or time; the first matching rule in a group selects the variant.

Modules provide Number/Boolean/Enum/Text fields. General settings appear in Project Settings; Object/Component fields appear only for valid members of active modules. In the previous project.json example, object 1's game.motion provides game.motion.speed for the compiled controller. The new gameplay path uses authored-scene components and metadata from connected SystemDescriptor entries; the scalar demo controller does not control its actor. Disabled authored fields are retained for restoration but are not passed to runtime or a new Team/Player distribution.

Scene and Inspector save Transform/Color, components, references, and prefabs in the authored project. Game uses a separate Play copy; authored changes are available after Stop. The old 3D Viewport and Object Tree/Components are in Engine and show its demonstration runtime. Model/texture import is available in Assets. Networking exists as a schema test fixture; multiplayer, DLAA, and SMAA are not implemented.

<a id="области-редактора"></a>

## Editor areas

Engine and Project have separate personal layouts. Select an editor in the area's header. H splits an area side by side, V splits it vertically; drag the divider, or use × to close an area and merge it with its neighbor. Full can mix available editors in both workspaces.

The Windows menu activates an existing area of the selected type in the current workspace or adds one beside the active area. It also provides Save Layout / Load Saved Layout / Reset Layout. File and Tools group open, save, import, diagnostics, and export operations; operations show progress and errors. Save As to another folder preserves all authored data and assets; confirmed replacement of an existing JSON creates metadata backups, while asset conflicts are rejected. The installed Engine binding is stored in a separate file beside the JSON. Use a separate Full/Team export to move the engine itself. Cancel or failure during Open/New preserves current edits; verification/export without saving reads the JSON on disk and leaves edits open. See the [details](docs/editor-workflow.md).

Scene, Hierarchy, Inspector, and Assets operate on the authored project; Game shows the gameplay copy. Each Scene area has its own free camera. Legacy Preview in Engine has an independent camera with a shared demonstration Runtime. Gameplay Systems defines the Engine author's execution graph. Game simulation uses a fixed tick of 1/60 independently of UI frames. Layouts are stored separately in .user/<full|team>/<engine|project>.json; gameplay canvas positions are in .user/full/game-systems-positions.json.

| Action | Controls |
| --- | --- |
| Add a module/pass | `+ Узел` (Add node) in the relevant graph or **Module Library** with search; the `Добавление в` (Adding to) line identifies the recipient. |
| Move a node | Drag its header. |
| Connect | Output on the right → compatible input on the left. |
| Delete a connection / open a nested graph | Double-click the line / header. |
| Graph: pan/zoom | Right/middle mouse button; Ctrl + wheel. |
| Legacy 3D Preview: camera | Left mouse button on the image; wheel. |
| Scene: tools | G/R/S — translate/rotate/scale; Escape cancels the current gesture. |
| Scene: camera | Middle mouse button — orbit; Shift + middle — pan; wheel — zoom; F — selected object. |
| Game and Player | WASD — move, left mouse button — attack toward the cursor, R — Restart. |
| New / open / save / Save As | Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+S. |
| Undo/redo | Edit / History, Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z outside a text field. |
| Cancel an unfinished connection | Esc. |

Open GPU renderer in Engine. `Заменить на редактируемый Custom HLSL` (Replace with editable Custom HLSL) copies a pass into the project; `Применить HLSL` (Apply HLSL) activates a verified candidate. Custom Rust supports text editing and visual editing of recognized fragments; `Применить Rust` (Apply Rust) compiles in the background.

Deleted FXAA/AO passes can be restored from the library and reconnected. Connect FXAA between the tone mapper and color encoding; AO uses HDR color, normals, and depth. New pass settings use their actual IDs; an incompatible connection preserves the previous link. See the [step-by-step graph assembly and performance diagnostics](docs/editor-workflow.md#building-a-gpu-graph-and-restoring-deleted-passes).

An error preserves the text, diagnostics, and last working version. A compatible renderer replacement preserves the world/time; changing the world resets simulation. A JSON File draft is applied when leaving; a syntax error keeps the editor open. Team validates the frozen blueprint when loading, saving, and applying the document. History stores the document, checkpoints, diffs, and merges with explicit conflicts; it does not rewind simulation.

<a id="cli-и-экспорт"></a>

## CLI and export

~~~powershell
.\outputs\ModuleEngine.exe --check project.json
.\outputs\ModuleEngine.exe --render project.json outputs/scene.png
.\outputs\ModuleEngine.exe --export project.json exports/my-engine
.\outputs\ModuleEngine.exe --export-team project.json exports/my-game
.\outputs\ModuleEngine.exe --capabilities project.json
~~~

--check compiles connected Rust and verifies the legacy path; when game is present, it creates a real GameHost and checks authored scenes/execution. --render runs 90 steps of 1/60 with a bounded Rust tick and writes a 960×640 PNG; for game, it uses the same gameplay copy with neutral input and no HUD. The source JSON is not replaced with an image. --game-check and --player-benchmark check the gameplay path and real window separately; see [game verification](docs/game-verification.md) and [Player](docs/player.md).

A Full export contains the entire source workspace, graph/Rust/HLSL, docs/examples, assets/includes, DXC/DLL/licenses, scripts, Cargo.lock, toolchain manifest, and a fresh source manifest. It is built separately from source.

A Team export contains a ready-to-run ModuleEngine.exe, project.json, frozen blueprint/exposure/Engine Rust, permitted gameplay, assets/backend/DXC, and .sdk with Core source/lock for compiled Custom. The native editor/Vulkan workspace is absent. Run .\ModuleEngine.exe or .\run.ps1; Restore uses .sdk/Cargo.toml and local tools/cache. The profile is an authoring policy, not a security sandbox.

Full/Team editor exports require Full and a complete source inventory. The separate --export-game creates a runtime Player with prebuilt logic and shaders. The destination must be a new/empty directory; .work/target/.user/.tools are not packaged. Source/includes come from the selected package, and registered assets are resolved relative to the opened JSON. [Distributions](docs/distribution.md) and [Player](docs/player.md) describe the different exports.

<a id="проверка"></a>

## Verification

~~~powershell
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo run --locked -p module-engine-editor -- --self-check outputs/verification-v03/cpu
cargo run --locked -p module-engine-editor -- --integration-check outputs/verification-v03/integration
cargo run --locked -p module-engine-editor -- --settings-check outputs/verification-v03/settings
~~~

Integration checks Vulkan forward/deferred, Custom compute/vertex/fragment, Rust commands, invalid candidates, and history/merge. Settings-check covers fields, archives, ranges, variants, gameplay Rust, and the prohibition on architectural changes. GPU checks require an adapter and Rust/DXC.

In 0.3, RTX 4080 SUPER checks covered equality of Object/ECS snapshots/GPU, actual movement from game.motion.speed, the effect of exposure/variants on pixels, and transactional rejection of an out-of-range value. Full/Team checks confirmed modified source, missing native files/compiler, forced Full rejection in Team, and blueprint tamper rejection. All 85 workspace tests passed: core 21, custom 15, editor 29, project 9, script 3, GPU 8; strict Clippy and rustdoc with -D warnings were clean. Full/Team exports were created with their own manifests without manual sealing and relocated to separate directories. A relocated Team worker read a game asset relative to the package and rendered 90 steps: 171723 geometry pixels. Native Project and Engine windows were inspected. Final reports are in outputs/verification-v03. In a new Team directory, Restore downloaded 13 pinned dependencies into its own cache; the Team executable then compiled the Custom SDK and rendered the scene with CARGO_HOME/CARGO_TARGET_DIR cleared. The check used installed Rust/MSVC and a driver; cold installation of these tools was not tested.

The GPU uses synchronous offscreen readback for egui with resource reuse and cached memory; there is no swapchain or frames in flight. The worker stays running, and explicitly declared JSON state is preserved transactionally between frames and across compatible hot apply. It has ordinary OS permissions. Visual Rust is limited to recognized expressions/commands. Skeletal animation, transparency/full PBR, dynamic rigid bodies, game saves, GPU buffer/geometry ports, virtual geometry, live coediting, and dynamic binary plugins are not implemented yet. GameHost performance and actual Player window performance are measured separately; the earlier 0.3 results above are not results for the new gameplay stage.

<a id="документация"></a>

## Documentation

| Topic | Document |
| --- | --- |
| Gameplay stage goals and accepted scope | [next-stage](docs/next-stage.md), [decisions](docs/decisions.md), [open questions](docs/questions.md) |
| Shared architecture and JSON | [architecture](docs/architecture.md), [graph-format](docs/graph-format.md) |
| Executable modules and game rules | [module-execution](docs/module-execution.md), [gameplay](docs/gameplay.md), [game rule packages](docs/optional-gameplay.md) |
| Independent puzzle project | [puzzle-project](docs/puzzle-project.md) |
| Menus, files, windows, and workflow | [editor-workflow](docs/editor-workflow.md) |
| Scenes, prefabs, and editor authoring | [scenes](docs/scenes.md), [editor-game](docs/editor-game.md) |
| Content and GPU | [content](docs/content.md), [gpu-hlsl](docs/gpu-hlsl.md) |
| Rust, state, and hot apply | [custom-rust](docs/custom-rust.md), [custom-state](docs/custom-state.md) |
| Settings, extensions, and history | [settings](docs/settings.md), [extending](docs/extending.md), [history-assets](docs/history-assets.md) |
| Editor distributions and standalone games | [distribution](docs/distribution.md), [player](docs/player.md) |
| Verification and measurement boundaries | [game-verification](docs/game-verification.md) |

The shared execution and renderer/service boundaries are documented in [shared-game-host.md](docs/shared-game-host.md).
