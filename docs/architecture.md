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

Interactive `GameWorker` now has separate simulation and presentation schedules:
fixed 60 Hz gameplay and a 120 Hz render target using genuine interpolated poses
between completed states. Core provides safe renderer-neutral pose interpolation;
Host owns two bounded raw snapshots and applies content afterwards. This leaves
authored/state data discrete, requires explicit lifetime/discontinuity boundaries,
and adds about one tick of visual delay. Extra renders never execute gameplay or
Custom. Static sub-tick samples are suppressed. Frame provenance and the actual
camera stay attached to completed pixels. Native evidence is source- and
scope-bound; a render target is not measured FPS. See [current GPU results](gpu-presentation-20261018.md). See [Host](shared-game-host.md)
and [worker](game-worker.md#interpolated-presentation).

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

The normal native Vulkan viewport publishes an opaque GPU frame lease instead of CPU pixels. Windows bridges the separate Vulkan producer to WGPU/DX12 through shared D3D12 resources; Linux bridges separate Vulkan devices through dedicated OPAQUE_FD memory and requires exact device and driver UUIDs. The GUI bridge copies and premultiplies RGBA8 on the GPU into its own native `Rgba8Unorm` texture, preserving the byte rounding of the previous egui conversion without sRGB decoding. Normal presentation performs no pixel readback to CPU and no CPU texture upload. Unsupported or failed interop reports an error and preserves the last good image; it does not silently select CPU transfer.

The native producer admits at most one GPU submission at a time, with four leased output slots. `render_presented` returns `RenderSubmission::Pending(ticket)` after admission; `poll_presented(ticket)` checks completion without waiting and publishes that exact request once. `Busy` admits no work. `Ready(Frame)` supports immediate CPU/software output. Until completion, the previous image, identity, camera and provenance remain current; pending work is not a fresh frame. Scene uploads, atlas images, command resources and pipelines remain owned until their GPU use completes. The GUI separately retains each published lease until actual consumer GPU completion. Dropping a CPU reference or skipping a minimized surface does not complete either side. Explicit CPU/software rendering, diagnostic Glow and PNG/capture retain CPU pixels and may synchronously wait for readback; a conflicting CPU render is rejected while native producer work is pending. There is no game-renderer swapchain or multi-submission producer pipeline.

Cancellation discards publication without waiting or freeing busy GPU resources. Reaping cancelled work may be deferred until the next submission or backend destruction; absence of publication does not permit early reuse. Dropping a renderer transfers unfinished producer work to an owned retirement job; only that background job may wait up to 30 seconds for its own fence. Application shutdown joins these jobs after renderer owners have stopped. `finish_gpu_retirements` reports consumer retirement plus nested `producer` registered/joined/completed/unconfirmed/failed counts. A timeout, device error, panic or failed retirement-thread start preserves unconfirmed resources and their device pin instead of reusing potentially busy memory; it is reported as an error, not successful completion. Linux consumer ownership-release failure retains its existing quarantine behavior. Historical successful or synthetic failure checks do not certify new async sources or real hardware loss.

The renderer-only diagnostic benchmark measures passes and explicit readback; Host and actual windows include different costs. The native window adds GPU interop/copy/conversion, HUD, egui/WGPU, vsync and OS scheduling. [GPU/HLSL](gpu-hlsl.md), [game verification](game-verification.md) and [Player](player.md) retain separate measurement criteria. New async sources require separately bound checks and an independent conclusion in the [async report](gpu-async-20261018.md); passing individual current gates does not establish no regression or final acceptance. The [historical GPU report](gpu-presentation-20261018.md) retains Windows source15/Linux source11 results and earlier failures, including the cold gate open at that time. The [18 October continuation](p0-continuation-20261018.md) separately records the later normal cold PASS 40.906/39.9128549 s, Windows22/16 and WSL Linux23/16 and scoped native/package/lifecycle checks for its own sources. These snapshots do not verify the new async implementation or certify hardware Linux, real device loss or physical scanout.

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
