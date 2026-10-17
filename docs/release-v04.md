<a id="результат-игрового-этапа-04"></a>

# Results of the 0.4 Game Stage

[English](release-v04.md) | [Русский](ru/release-v04.md) | [Documentation](README.md)

The stage was implemented in `E:\ModuleEngine`: authored scenes and templates, an executable-system graph, persistent Rust Custom state, static import, and standalone Windows Player. The previous `project.json` is retained for the demonstration route; the new game is in `game-project.json`.

<a id="что-запускать"></a>

## What to Run

| Result | Path relative to the project root |
| --- | --- |
| Editor 0.4 | `outputs/ModuleEngine.exe` |
| Game without editor windows | `outputs/game-player-v04/Player.exe` |
| Fully editable workspace | `outputs/source-project-v04` |
| Team editor with the selected architecture | `outputs/team-build-v04/ModuleEngine.exe` |
| Verification results | `outputs/verification-v04` |

```powershell
.\outputs\ModuleEngine.exe
# Game / Scene → Play; edit the authored scene after Stop.
.\outputs\game-player-v04\Player.exe
```

WASD moves; left mouse button performs one attack toward the cursor, hitting each enemy in the area once; R restarts. After the first arena, enter the green passage and defeat the boss. Its heavy attack displays a red area in advance. E demonstrates Rust Custom. Controls, export, and measurements are described in detail in [Player](player.md).

<a id="почему-появились-эти-части"></a>

## Why These Parts Were Added

The scene is now saved data, and Play works with a copy. Templates propagate shared changes to instances while preserving individual overrides. Components of connected modules define ordinary Inspector fields; the inactive-component archive is preserved for the author and excluded from runtime/new distributions.

The system graph defines order and typed dependencies; the registry connects implementations through factories. Object and ECS really use different storage. The provider instance is retained between frames. World validation happens before system/Custom state publication; rejection rolls the frame back. Stop, scene transition, and closing call lifecycle hooks. [Contracts and extension](module-execution.md).

Rust Custom runs as a persistent process with explicitly declared state. Compatible application preserves state; an incompatible change requires explicit reset or migration. Export prebuilds workers and GPU SPIR-V so that players need no compilers. [State and errors](custom-state.md), [resource format](content.md).

<a id="проверки-14-октября-2026"></a>

## Verification on 14 October 2026

The initial 0.4 delivery results are preserved below. Current builds at the same paths have gained melee visuals; their design, screenshot, and separate verification results are described in [combat-feedback](combat-feedback.md). Previous builds are preserved in `outputs/update-backup-v04-combat-*`, initial reports in `outputs/verification-v04`.

- **126 unit tests passed**, including real Rust processes, GPU, dependent resources, state, UI input, scenes/templates, packaging, and lifecycle.
- Workspace Clippy and rustdoc generation passed with `-D warnings`; formatting was checked.
- Previous CPU, Vulkan/Rust/source-export integration, and Project settings were checked separately.
- Object/ECS execute the same 180 actions: world, events, state, and actual Vulkan pixels with the imported model match.
- The autopilot completed two scenes: 20 monsters, transition, boss, and victory. Test health was increased to 5000; other mechanics were preserved; this checks completion, not balance. Death was checked in a separate scene with 1 HP, then Restart restored the authored scene.
- Player was moved to a separate folder. During verification, Rust/Cargo were absent from PATH, and `MODULE_ENGINE_DXC` pointed to a missing file; prebuilt workers/SPIR-V were used. The package contains no native Rust sources, SDK, or compilers. An attempt to invoke an editor command from Player was rejected.
- Full export confirmed Engine availability. The moved Team distribution compiled Custom through its own `.sdk` and verified the authored game; Engine remained unavailable, and forced `--profile full` was rejected. Installed Rust/MSVC and an existing cache of pinned third-party dependencies were used; fresh tool installation was not checked.

<a id="производительность"></a>

### Performance

Reference for the 14 October 2026 measurement: RTX 4080 SUPER, Windows, Vulkan + egui/OpenGL window presentation, 3440×1440 144 Hz monitor. Actual framebuffer and game canvas were **1920×1080**. The scene and health were normal; all 120 measured frames retained 20 living enemies and Playing status. Measurement followed 30 warmup frames.

The OpenGL setup and results below are historical evidence. The [current GUI policy](linux-support.md) is WGPU/DX12 on Windows and WGPU/Vulkan on Linux; these measurements do not establish results for those GUI backends or the current build.

| Measurement | Mean | p95 | Maximum |
| --- | --- | --- | --- |
| GameHost: simulation, Custom, audio, Vulkan/readback | 4,78 ms | 5,18 ms | 5,37 ms |
| Full Player window loop | 9,68 ms | 11,00 ms | 11,60 ms |

Window loop: **103,35 FPS**, the 60 FPS target was met. This is callback start-to-start with processing, presentation, vsync, and OS scheduling; the time when the image physically appeared on screen was not measured separately. The window was unfocused, with no user input during measurement; game systems and enemies continued running. The report itself validates size, load, and status. The result applies to this PC and the fixed example.

Source data: `outputs/verification-v04/player-native/native-frame-report.json`, `player-final-functional/game-report.json`; `release-evidence.json` contains executable checksums and verification conditions. The full methodology is in [game-verification.md](game-verification.md).

<a id="проверка-интерфейса"></a>

## Interface Verification

![Authored scene and Inspector](media/editor-game-v04.png)

![Game-system graph with resource connections](media/editor-systems-v04.png)

![Game HUD and boss warning](media/player-boss-v04.png)

The last screenshot uses a separate authored fixture: starting in the second scene with the character beside the boss. The diagnostic two-frame screenshot actually executes one fixed tick. The screenshot checks visibility of the HUD/attack area; it is not proof of completing the game. This fixture did not change the shipped project.

<a id="что-остаётся-дальше"></a>

## What Remains

This is a working small engine and game example. Skeletal animation, full PBR/transparency, dynamic physics/navmesh, mixer and spatial audio, progress saving after exit, seamless streaming, and Linux are not yet present. Audio is currently single-voice PCM WAV. The GPU frame still returns to the CPU; a native swapchain and multiple frames in flight are the next scaling route.

Visual Rust supports a limited subset and stores the remaining code as text. Native modules require a rebuild. Player uses the same host executable in game mode: physical exclusion of editor code from the binary has not been done separately. A fresh Windows/tool installation on another machine was not checked. Further decisions are recorded in [questions.md](questions.md).
