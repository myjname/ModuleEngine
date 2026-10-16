<a id="самостоятельная-игра-и-проверки"></a>

# Standalone Game and Verification

[English](player.md) | [Русский](ru/player.md) | [Documentation](README.md)

Project working directory: `E:\ModuleEngine`. The authored example is stored in `game-project-v4.json` after [explicit migration into a copy](engine-profiles.md); the original `game-project.json` and previous `project.json` are preserved separately.

<a id="запуск"></a>

## Launch

Game execution runs in a [persistent background GameWorker](game-worker.md). The window displays completed snapshots, processes input, and does not wait for slow Rust Custom. Under overload, tick cost and simulation lag are shown separately; the speed of the game itself depends on its code.

```powershell
.\outputs\ModuleEngine.exe game-project-v4.json
# Editor window: select Game / Scene. Engine → Gameplay Systems edits execution.
.\outputs\ModuleEngine.exe --play game-project-v4.json
# Game window without editor panels.
```

WASD moves the player. Left mouse button attacks toward the point under the cursor on the horizontal plane. R restarts the game; Esc closes Player. E demonstrates a Rust Custom action: changing the player color and sending an audio event. Dodging uses movement; there is no separate dash yet.

The first scene contains twenty monsters. Defeat them and enter the green passage at the far side of the arena. The second scene contains the boss; the red area shows attack preparation. Scene objects, components, and behavior parameters are edited as ordinary data.

<a id="как-читать-удар"></a>

## Reading an Attack

If the author configures attack preparation (`melee.windup`), the cyan preview is hidden during it: the attack direction is fixed when pressed. The normal example has zero player windup.

When the cursor is over the active game window and a swing is not yet active, a pale cyan sector and arrow show the next attack's area and direction. Gold rings highlight enemies currently passing the same hit test used by the combat system. Left mouse button starts an attack: the cyan preview sector hides, while the gold active area, swing trail, and target rings use the fixed area of this attack. After the swing, the cyan sector follows the cursor again. Enemy attacks and windup warnings are red.

A confirmed hit brightens the target model for 0,16 seconds and shows sparks with a floating damage number. Damage received by the player is red; damage dealt to enemies is gold. Sparks and the number remain visible after a killed enemy is removed.

A target must have valid `actor` and `health` components with `current > 0`: an object without health does not become a target merely because it has `actor`. Enemy radius extends the attack's radial reach, while the angle is checked against its center. A ring can therefore highlight a target whose center lies slightly beyond the sector arc. Highlighting refers to the current position and does not cancel cooldown. The direction of a started swing is fixed: later cursor movement does not rotate that attack. Rings during a swing show targets in its area, and an enemy already damaged does not take damage again: each enemy takes damage once per swing.

Feedback is identical in editor Play and Player. Hit data for sparks and numbers is stored for 0,35 seconds of game time; its time freezes on Pause. Stop, Restart, and scene loading clear this temporary data; combat overlays hide on victory or defeat. The flash changes only the rendering snapshot, preserving object colors and materials. Sectors and trails are drawn over the scene image rather than as weapon animation or depth-aware particles. They help show attack area, direction, and result.

<a id="экспорт"></a>

## Export

```powershell
.\outputs\ModuleEngine.exe --export-game game-project-v4.json outputs\my-game
.\outputs\my-game\Player.exe
```

Player builds as a separate package with selected native features; the editor exe is not copied into it. `player.json` v2 pins the exact feature set and SDK API. See [profiles and compatibility](engine-profiles.md).

Player and Team share working-graph preparation. Before selecting native features and dependencies, export validates the authored profile and pins, then retains nodes leading to outputs, recursively processes nested graphs, and retains used Rust/HLSL sources. Scheduled game systems are retained regardless of graphical output. A connected but disabled pass is also retained: the game may enable it through a permitted setting. Unused archived HLSL does not force the distribution to look for its include files.

The Engine profile, exact locks, and explicitly selected enabled modules are not rewritten. Explicitly enabled Vulkan may therefore remain a native capability with an active software renderer; that capability alone does not trigger Vulkan compilation of the software-renderer graph. The package is checked with the actual selected native feature set before building; runtime does not repair an incomplete distribution during loading.

The destination directory must be new or empty. Export creates Player.exe, project.json, player.json with checksums, model/material/audio dependencies, and `.spirv` for the selected graphs. Rust workers are in scripts/<source-id>. After copying the folder to another Windows machine, launching does not use the original working directory, SDK, or compilers.

The build is prepared in its own temporary directory beside the destination. Copying, Rust/HLSL compilation, package verification, and a trial render must finish before publication. On error, the temporary directory is removed, and the selected empty directory remains usable for another attempt. Publication on Windows does not replace a file or nonempty directory that appeared at the destination during the build. If temporary-directory cleanup also fails, the error reports its path; export does not attempt to delete other content at the destination.

Game resources are resolved relative to the selected JSON project; GPU sources and HLSL include roots come from the selected backend package, and the Rust/Core SDK from its compile root. A project with its own models in another directory therefore does not borrow same-named resources from the engine directory. Before copying package files, normalized paths, Windows case conflicts, and matching bytes for an asset and GPU file sharing a path are checked. `Player.exe`, `project.json`, `player.json`, `README.txt`, `.spirv`, and selected Rust worker directories are reserved: declaring an asset at these paths is rejected without silent overwriting.

The existing Team distribution remains a team editor. The Player game distribution is a separate launch mode and separate export. Full/Team Engine restrictions are preserved; game-system order belongs to Engine, while scenes and permitted game logic belong to Project.

<a id="воспроизводимая-проверка"></a>

## Reproducible Verification

```powershell
.\outputs\ModuleEngine.exe --game-check game-project-v4.json outputs\game-verification
.\outputs\my-game\Player.exe --verify-package outputs\my-game outputs\player-verification
```

Checks cover Object/ECS states/events/images, rollback of rejected Custom, death and Restart, transition, and victory. Automatic completion uses a separate saved verification copy with increased player health; enemy behavior, attacks, and transitions run through real systems. It checks the execution route, not the difficulty balance of the shipped game.

The GameHost measurement includes simulation, Rust IPC, collisions, audio, and Vulkan/readback. The run is warmed up, then 120 frames at 1920×1080 are measured. The report contains GPU, load, mean, median, p95, and max; window presentation is measured separately. Disabling an inactive audio module excludes loading its sounds, but the authored archive remains.

<a id="изменение-rust-во-время-play"></a>

## Changing Rust During Play

In Custom / Rust, select game-controller, edit update, and click Apply Rust (`Применить Rust`); compilation runs in the background. State is declared on the first source line. When the schema changes, the application preserves previous code and offers explicit reset/migration. Switching text/graph shows supported operations and preserves other constructs as text.

JSON-packet protection and timeouts limit process errors, but the worker has normal user permissions. Use code owned by the project. The contract and examples are described in [Custom state](custom-state.md).

<a id="измерение-настоящего-окна-player"></a>

## Measuring the Real Player Window

```powershell
.\outputs\ModuleEngine.exe --player-benchmark game-project-v4.json outputs\native-window-verification
.\outputs\my-game\Player.exe --benchmark outputs\my-game-native-window-verification
```

The measurement uses the same Player, GPU rendering at 1920×1080, RGBA→egui conversion, image transfer to the GUI renderer (WGPU/DX12 on Windows and WGPU/Vulkan on Linux by default), game UI, and window loop. It does not increase health or replace scenes or behavior. Input is ordinary, without synthetic completion: twenty enemies continue acting. After 30 warmup UI frames, at least 120 callbacks and 120 distinct completed game frames are collected. The window then closes; the report is saved to `native-frame-report.json`.

In report schema 3, callback start-to-start measures the window loop: conversion, egui tessellation, GUI texture upload/draw, swap/vsync, and OS scheduling. Simulation, Rust IPC, and Vulkan/readback run on a separate worker. `mean_fps` uses distinct `Frame` values and their completion times; `ui_mean_fps` uses window callbacks. Repeating an old image does not increase game FPS. Completion intervals of observed worker frames, intervals of their first UI observation, and snapshot age are recorded separately; frames skipped between observations are not counted as measured. All egui passes are included in UI/HUD costs. Step/render cost fields contain the latest completed values observed on UI frames, so their distributions cannot be treated as an independent sample of worker frames.

Additionally, `eframe::Frame.info().cpu_usage` measures CPU work of the UI and native presentation for the previous frame, excluding vsync waiting. Separate conversion/enqueue figures describe texture preparation; the framework performs the actual GUI texture upload later, and it is included in framework CPU and the overall period. This measures the window loop, not the time photons appear on the monitor or a separate GPU timestamp of the GUI renderer.

The window requests a client area of 1920×1080 physical pixels accounting for DPI. The report contains actual framebuffer and canvas area, game Vulkan renderer GPU and GUI backend, monitor/refresh rate, CPU identifier, models, living-enemy count, and player status/health. If the window manager reduced the window, `physical_resolution_met` shows this; offscreen render size stays 1920×1080.

`target_met=true` requires a complete measurement, physical 1920×1080, twenty living enemies and Playing in every measured frame, no errors, and mean and p95 for both window and game periods no greater than 16.667 ms. Monitor refresh rate, vsync, and OS scheduling may limit the result even with a fast GameHost. If the stationary player dies, the measurement does not claim to meet the game-load criterion. Closing or hanging the window does not turn into success; a 45-second watchdog saves an incomplete report. Unknown window visibility is marked separately and does not prove actual display on screen.

In Player, focus on Restart or the game viewport does not disable WASD. In the editor, input remains available after clicking the game viewport, and text-editor focus does not move the character. Left mouse button counts as an attack on press, not release.
