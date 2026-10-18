<a id="игровой-этап-от-авторской-сцены-до-player"></a>

# Game stage: from authored scene to Player

[English](next-stage.md) | [Русский](ru/next-stage.md) | [Documentation](README.md)

The goal is to verify shared modules in a complete small game. The two arenas are project data: the executor does not know their names, monster counts, or the victory rules of a particular level.

This page describes the 0.4 game path; subsequent changes extracted the shared Host, optional gameplay packages, and standalone Player. Current completed stages and open goals are collected in [current status](current-status.md); each report's results apply only to its own snapshot.

<a id="что-где-находится"></a>

## Where things live

| Task | Implementation | Contract |
| --- | --- | --- |
| Authored scenes and prefabs | [engine-core/src/scenes.rs](../crates/engine-core/src/scenes.rs) | GameProject, AuthoredScene, AuthoredObject, Prefab |
| Subsystem execution | [engine-core/src/systems.rs](../crates/engine-core/src/systems.rs) | SystemRegistry, ExecutionGraph, ExecutableSystem |
| Gameplay rules | [engine-core/src/game.rs](../crates/engine-core/src/game.rs) | GameSession, GameInput, GameEvent; separately registered systems |
| Static models and images | [engine-content/src/lib.rs](../crates/engine-content/src/lib.rs) | ContentLibrary; renderer-neutral Mesh/Material |
| Rust Custom | [runtime-script/src/lib.rs](../crates/runtime-script/src/lib.rs) | Persistent worker, ScriptTick, explicitly declared state |
| Scene and system-graph editor | [editor/src/game_ui.rs](../crates/editor/src/game_ui.rs) | Authored data separated from the Play copy |
| Shared subsystem composition | [game-host/src/game_host.rs](../crates/game-host/src/game_host.rs) | One GameHost for editor and Player |
| Standalone game and export | [Player crate](../crates/player/README.md), [game_package.rs](../crates/editor/src/game_package.rs) | player.json package, native Player, assets, and compiled programs |

<a id="проверяемый-результат"></a>

## Verifiable result

Create/edit a scene, save JSON, and reopen it. Change the monster prefab: the shared parameter changes in every instance while local overrides remain. Play starts a separate GameSession; Stop restores authored values.

Import a static glTF/GLB model with UVs and an opaque material. Connect a PNG/JPEG as its material texture. The same neutral geometry reaches Vulkan from Object and ECS; states, events, and images are compared.

Game example: WASD, an angled overhead camera, and left-click attacks toward the cursor. Each opponent in the area takes damage once per swing. Walls and obstacles block movement. After twenty small monsters, a passage into the second scene opens. The boss has a heavy attack with a warning and a ground area. Player health carries across; defeat and victory appear in the game UI; R restarts.

The host gathers input, and simulation uses a 1/60-second step. System order is defined by graph stages; resource types and dependencies are validated before launch. The interactive actor runs outside the UI thread, processes at most one fixed tick per iteration, then checks commands again; its accumulator is capped at 0.25 seconds. The synchronous CLI route retains its fifteen-step budget. See the [worker contract](game-worker.md).

<a id="состояние-и-применение-изменений"></a>

## State and applying changes

Play actions do not modify the source GameProject. Native-system and Custom state belongs to the live system instance. Scene systems are recreated when a new scene loads. A player with the persistent component carries its component data across; positions are defined by the new scene's entry. Restart creates initial scene states; Stop destroys the Play copy.

Rust Custom declares serializable state with the `module-engine-state` header. Code compiles separately and runs in a persistent process. Compatible application preserves state; an incompatible candidate remains separate until an explicit reset or compatible migration JSON is provided. If commands/state are rejected, the previous working version remains active.

All systems stage changes, then the executor calls validate_tick on each one. The host transaction validates and applies commands to the selected WorldProvider before system commit_tick. An error invokes rollback_tick on systems that actually entered that step and rolls back the host transaction, restoring game/resources/events/owned state. The provider is retained between fixed ticks and direct commands, and recreated on scene entry, transition, or Restart. Its validate_commands must have no side effects and apply_commands must be atomic; after publication, the host can apply an inverse portable batch. Hidden state and arbitrary external native-code actions require their own transactional hooks; rollback errors are reported explicitly.

The render graph and HLSL can be applied to an existing game. The native gameplay-system graph is edited as a candidate; application requires Stop. Rebuilding ordinary native modules remains a separate operation. Details: [Custom state](custom-state.md), [execution](module-execution.md).

<a id="неактивные-возможности"></a>

## Inactive capabilities

Active components are determined by metadata from connected systems and their explicitly declared extensions, including Custom params.components. The ordinary inspector displays only active fields. Inactive authored data is retained for restoration, while runtime and new deliveries do not receive it. This applies to any module, including potential replication; there is no ready-made networking module here.

The personal dock layout is stored separately from the project. An initial layout with Game/Scene is created when no saved personal layout exists; an existing layout is not replaced. The window can be selected manually from the area-type list.

<a id="сборка-игры"></a>

## Building the game

`--export-game` compiles Rust Custom and every selected GPU stage in advance, copies required assets, and verifies the new package from its own root. `Player.exe` on Windows and `Player` on Linux open the game without editor panels. Players need no Rust/Cargo/DXC or native source code. The selected Vulkan renderer requires a compatible driver; native-service availability depends on the platform.

Player is built as a separate [package](../crates/player/README.md) without a production dependency on Editor or Two Arenas. Native features are selected explicitly; the shared GameHost was moved out of the editor, and ARPG and audio are optional. Independent Puzzle and portable Player packages were verified in [stages 5–8](modular-runtime-progress.md). Successful checks do not imply that the performance target has been achieved.

Target: 60 FPS, 1920×1080, twenty active enemies on the current PC with fixed example content. `--game-check` measures GameHost processing. The normal Editor's `--player-benchmark` and standalone Player's `--benchmark` measure their actual native windows. Normal presentation keeps pixels on GPU; explicit CPU/readback diagnostics are separate. Reports distinguish worker completion, UI callback and actual UI-observed unique-frame intervals, with mean and p95/max for an explicit workload; processing FPS does not stand in for presented frame cadence. Windows passes exist for the exact artifacts in [current status](current-status.md); they do not automatically certify a later binary or promise performance with arbitrary assets.

<a id="границы"></a>

## Boundaries

Import is currently static: no skeletal animation, transparency, or full PBR. Collisions use controlled bodies on a horizontal plane and rectangular obstacles; there are no dynamic rigid bodies or navmesh. Audio is short event-driven PCM WAV, one voice; spatial audio and a mixer come later. Progress saving after exit and seamless streaming are deferred. Native Linux builds and bounded WSLg/X11/Wayland checks have been performed; native Linux audio/dialogs, hardware certification and arbitrary ABI/driver portability remain unsupported or unverified as documented. Windows ARPG and target-cold results are source/artifact-bound gates, not universal guarantees; see [current status](current-status.md) for current acceptance and failed attempts. Separate Player and the absence of a mandatory ARPG dependency are already implemented; this does not promise complete removal of every unused piece of native code.

Documents for further development: [scenes](scenes.md), [gameplay systems](gameplay.md), [content](content.md), [game editor](editor-game.md), [Player](player.md).
