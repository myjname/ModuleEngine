<a id="архитектура-04"></a>

# Architecture 0.4

[English](architecture.md) | [Русский](ru/architecture.md) | [Documentation](README.md)

The project stores its architecture, authored scenes, and source code in editable JSON. `ModuleRegistry` describes root world/render modules; `SystemRegistry` installs executable gameplay subsystems. A neutral snapshot separates world storage from rendering, while owned commands and state separate Rust Custom from specific storage implementations and Vulkan.

<a id="пакеты-и-границы"></a>

## Packages and boundaries

| Package | Purpose |
| --- | --- |
| module-engine-core | GraphDocument, registries, Object/ECS, WorldObject/commands, SceneSnapshot/Material, authored scenes and prefabs, SystemExecutor, and GameSession. Does not depend on Vulkan, UI, image import, or rustc. |
| module-engine-content | Static glTF/GLB, PNG/JPEG, decoded cache, material/geometry, and asset dependencies of the selected project. |
| module-engine-vulkan | Raster/compute GPU graph, resource contracts, HLSL/DXC and SPIR-V validation, actual textured Forward/Deferred passes. |
| module-engine-custom | Partial visual representations of Rust, source-span edits, and preservation of opaque text. |
| module-engine-script | Rust worker compilation, persistent process, bounded JSON-lines IPC, and explicitly declared state with validate/commit/reject. |
| module-engine-project | Document history, checkpoints, diff/merge, migration, asset manifests, and dependency export. |
| module-engine-editor | Native egui/eframe UI, GameHost, scene/execution editor, Full/Team policies, and a separate Player mode. |

<a id="два-маршрута-исполнения"></a>

## Two execution paths

The retained demonstration `Runtime` path serves `World → scene.extract → registered renderer → viewport.output`. Additional inputs accept WorldCommands/RenderCommands; an optional Scalar graph controls speed. It serves one viewport and a limited set of routes. An ordinary root Node descriptor does not, by itself, create arbitrary subsystem behavior. This path is used by the old `project.json`, 3D Preview windows, and Object Tree/Inspector.

The game path uses `GraphDocument.game`: GameProject, scenes/prefabs, and ExecutionGraph. `SystemRegistry` registers a descriptor together with an `ExecutableSystem` factory. `SystemExecutor` creates instances and executes saved stages/nodes in the specified order. Resource types, required ports, `requires`, and `after` are validated before execution; the executor does not gain a branch for each new module name. `GameSession` owns the Play copy, a fixed 1/60 clock, and the canonical `GameState.world: EcsWorld`. The ECS path has no second provider; Object and explicitly connected third-party providers serve as compatible projections. `module-engine-host` connects the session, content, persistent Custom, a registered `FrameRenderer`, and explicitly requested services. It is shared by the Game / Scene window and Windows Player; the shared crate does not depend on the editor, Vulkan, or gameplay packages.

Game order lives in `game.execution`; it is not inferred from node positions or the legacy Scalar chain. In the new `game-project.json`, scene components define actor speed; the old scalar controller is not its game loop. A shared native-system API does not mean that a universal scheduler already executes all root Graph modules. The application registers Vulkan through its adapter; the shared host has also been verified with an independent CPU renderer and in headless mode. Other GPU APIs and a dynamic native binary ABI are not implemented yet. See the [shared host](shared-game-host.md).

<a id="мир-и-контент"></a>

## World and content

In the old Runtime, Object stores an actual recursive hierarchy, while ECS uses independent component tables. In the game path, mutable transforms, parents, colors, references, and dynamic components belong to one ECS. Native systems use column queries, and object DTOs are extracted for UI, IPC, and checks; see the [contract and lifetime](ecs-world.md). `from_objects` creates both stores from the resolved authored scene without demo angular velocities. Their commands and snapshots are equivalent for identical data. The renderer receives vertices/UVs, matrices, color, and Material; it does not query ECS or object storage. Matrices are row-major, and vectors are column vectors.

ContentLibrary attaches imported primitives by stable object ID. glTF local matrices compose with the object matrix; TextureImage is shared between snapshots through Arc. Import and export are restricted to the selected project/package root. Opaque base-color material runs on the GPU; skeletons, transparency, full PBR, mipmaps, and streaming are not implemented yet.

<a id="авторство-и-сохранённые-данные"></a>

## Authoring and saved data

EngineConfiguration defines active feature modules, exposed ranges, and ready-made rendering variants. ProjectConfiguration stores permitted values/conditions. Legacy PropertySchema filters fields by the actual participation of an active module/component. In Game / Scene, component schemas are registered separately from the systems that consume them. Membership is computed from the selected composition, including explicitly declared Custom `params.components`; the connected renderer activates mesh/material. Fields, defaults, constraints, and references come from the shared schema rather than an arbitrary prefab. Inactive authored values remain in the author's document; the runtime and newly stripped Team/Player data do not receive them.

Full Engine requires a complete compatible native source manifest. Team is always Project-only: its blueprint pins the architecture, game ExecutionGraph, bindings/source IDs, shaders, and versions; scenes, active prefabs, and permitted gameplay Rust remain game data. Modified Full sources remain editable. Team's `.sdk` is required for compiled Custom; the package tools/assets root is separate from the SDK source root. Standalone Player has ready-made workers and `.spirv` and does not require SDK/Rust/DXC. These are delivery policies, not a sandbox for arbitrary native/Rust code.

Personal Engine/Project dock layouts are stored outside the game. The initial Game layout is created only when no saved personal layout exists; an existing layout is retained. Game/Scene can be selected manually from its window list. Legacy Preview areas have independent cameras/selections while sharing a Runtime step. The game scene viewport displays a separate Play copy. Pause preserves the runtime; Stop destroys it and restores authored data. The execution graph is edited in a local draft; Apply requires Stop and validates the entire candidate. Its canvas coordinates are stored separately in `.user/full/game-systems-positions.json`.

<a id="gpu-и-измерения"></a>

## GPU and measurements

`render.vulkan` has an external Scene3D → Frame3D contract. Its subgraph connects real geometry, Forward/G-buffer/Deferred, SSAO, tone mapping, FXAA, and encoding. ResourceSpec defines format, semantic, extent, samples, and access; DXC/SPIR-V reflection validates bindings and actual image formats. Custom scene raster can explicitly enable the material ABI through `scene_materials`.

The game renderer runs offscreen with synchronous readback; egui presents the resulting frames through WGPU/DX12 on Windows and WGPU/Vulkan on Linux. The game renderer remains a separate Vulkan module; the GUI backend does not select the game graph's format. Glow is available only as an explicitly selected diagnostic mode. The game renderer has no Vulkan swapchain or support for multiple frames in flight. Pass images, vertex/readback buffers, and the GPU atlas are reused; CPU reading prefers cached coherent memory. Player returns only RGBA8, without diagnostic depth/normal ownership readback on every frame. This is the current game's actual execution and measurement path; it is not an estimate of the maximum performance of any future engine.

The renderer-only benchmark measures passes and readback. The GameHost check includes simulation, Rust IPC, audio, content, and rendering. The actual Player window benchmark additionally includes texture conversion/upload, HUD, egui/WGPU presentation, vsync, and OS scheduling. Their results and criteria differ: [GPU/HLSL](gpu-hlsl.md), [game verification](game-verification.md), [Player](player.md). Figures for a new full window measurement are published after measurement, not inferred from renderer-only FPS.

<a id="кандидаты-состояние-и-отказ"></a>

## Candidates, state, and failure

Rust compiles in the background. A compatible version transfers explicitly declared state; an incompatible one remains a candidate until reset or migration JSON is provided. Legacy Preview and game instances have separate owned state. A persistent worker eliminates process startup on every tick; after a timeout/panic/invalid packet, it restarts with the last accepted JSON. Globals are not a persistence/migration contract and are not portable between versions.

The game executor runs initialize/step for all systems and validates all staged outputs; the host transaction then validates and applies the world command batch, and only then are system commit hooks run. An error invokes rollback in the participating systems and host transaction, restoring world/resources/events/owned state. GameSession retains one ECS between fixed ticks and direct commands. If a compatible external WorldProvider is selected, its instance is retained too; a new instance is created on scene entry, transition, or Restart. The provider must implement validate_commands without side effects and apply_commands atomically. To roll back after publication, the host applies an inverse portable batch; hidden state and external native effects require their own transactional hooks. Writing a file does not automatically become reversible, and rollback failure is reported explicitly. After a scene transition, scene systems are recreated; the persistent player carries components across while using the destination's local transforms/references.

Applying a compatible render graph/HLSL or permitted Project controls preserves the live game. Changing the native ExecutionGraph requires Stop; ordinary native world/renderer/UI source code requires a build. Legacy Runtime separately carries its provider across when world ID/type/parameters match, and clears incompatible renderer controls. These mechanisms should not be confused with saving game progress to disk.

<a id="история-и-объединение"></a>

## History and merging

Undo/redo and checkpoints preserve GraphDocument, including scenes, prefabs, graphs, source code, and parameters. They do not preserve live objects, timers, GPU resources, or Custom runtime state. Restoring a document does not rewind Play.

Merge normalizes ordinary Graph nodes by ID and edges by input. Scene object arrays and ExecutionGraph stage/node arrays do not yet have element-by-element ID-aware merging: overlapping changes to such an array produce a whole-array conflict with base/ours/theirs. On conflict, preview retains the base. This is a boundary of the current algorithm, not automatic merging of two scenes.

More detail: [format](graph-format.md), [module execution](module-execution.md), [scenes](scenes.md), [game editor](editor-game.md), [content](content.md), [Custom state](custom-state.md), [history](history-assets.md).

The precise format 4 contracts, explicit profile publication, migration into a copy, and separate feature-selected Players are described in [Engine profiles](engine-profiles.md). The shared Host does not depend on the editor; Player and the editor install the same source contracts through composition.
