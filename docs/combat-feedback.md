<a id="визуальная-обратная-связь-ближнего-боя"></a>

# Melee combat visual feedback

[English](combat-feedback.md) | [Русский](ru/combat-feedback.md) | [Documentation](README.md)

The game example already calculated damage, but an ordinary attack did not show its area or result. This change adds shared visuals for editor Play and standalone Windows Player while preserving combat rules and authored scenes.

<a id="что-видно-в-игре"></a>

## What appears in the game

- Before an attack, a pale blue sector and arrow show the direction under the cursor. Gold rings highlight living opponents that currently meet the hit-test criteria.
- During an attack, the area turns gold and a moving swing trail appears. Direction locks at the start of the attack, including the configurable windup; the current mouse position controls the preview again after it ends.
- A confirmed hit brightens the model for 0.16 seconds and creates sparks and a floating damage number. The sparks and number persist for 0.35 seconds, including lethal hits after the target is removed.
- Enemy attacks and damage received by the player are shown in red.

Radius, angle, damage, cooldown, and one hit per target per swing are unchanged. The shipped example uses a range of 2.2, an angle of 1.9 radians, damage of 28, and a swing duration of 0.12 seconds. The preview tests current geometry rather than cooldown readiness. Target radius counts toward range, while angle is tested at its center: a target may be reachable at the very edge of the drawn sector.

<a id="где-это-устроено"></a>

## Implementation locations

| Part | Purpose |
| --- | --- |
| `crates/gameplay-arpg/src/queries.rs` | `MeleeArea::contains` is the shared area test for combat and highlighting; `melee_aim`/`melee_targets` are preview queries; `attack_visuals`/`hit_visuals` are transient data for actual attacks and hits. |
| `crates/gameplay-arpg/src/ui/combat_visuals.rs` | Shared projection of sectors, targets, trails, sparks, and numbers; flash in the extracted scene snapshot. |
| `crates/editor/src/player.rs` | The game HUD calls the shared visuals; standalone Player displays the same effects. |
| `crates/editor/src/game_ui.rs` | Supplies aiming from the active editor game window. |
| `crates/game-host/src/game_host.rs` | Applies flashes after materials to the snapshot for the GPU, preserving the objects' original colors and components. |

Transient data follows the game clock: Pause freezes the effect; Stop/Restart/scene loading clears it. Victory/Defeat hides combat overlays. Sectors and trails are projected over the scene image; skeletal animation and depth-tested particles are outside this scope.

Controls are described in [Player](player.md); the combat contract is in [gameplay](gameplay.md).

<a id="проверка-этой-правки"></a>

## Verification of this change

On October 14, 2026, 136 workspace unit tests, formatting checks, Clippy for all targets, and rustdoc generation with `-D warnings` passed. New checks cover sector boundaries, targets without health, identical Object/ECS results, direction while moving the cursor during swing and windup, lethal hits, effect expiry, and preservation of authored colors/materials.

The final exported Player was checked without Rust/Cargo in PATH and with DXC deliberately absent: it uses ready-made workers and SPIR-V. Object/ECS comparison, Custom rejection/rollback, death/Restart, and completion of both arenas passed. As before, automated completion uses a separate copy with increased health; the shipped game is unchanged by it.

A separate measurement of the actual Player window on RTX 4080 SUPER used 1920×1080, 20 living enemies, 30 warmup frames, and 120 measured frames. Mean frame period was 11.13 ms (89.83 FPS), p95 was 13.47 ms; there were no errors, and the 60 FPS target was met. The measurement used the ordinary scene, without user input or window focus. It is not a separate stress test of many simultaneously drawn attacks. Original 0.4 figures are retained separately in [release-v04](release-v04.md).

![An actual attack in Windows Player: area, swing, target, and damage](media/player-attack-feedback.png)

The image was captured with native Player's own screenshot mode. A separate authored fixture places the character 1.4 units from a monster; three actual fixed ticks run before the window opens, with an attack action on the first. Damage, range, and angle remain ordinary. Effects come from combat state; the image was not painted over. The fixture does not replace `game-project.json`.

Reports and exact image inputs are in `outputs/combat-feedback-verification`: `player-final-functional/game-report.json`, `player-final-native/native-frame-report.json`, `capture-project.json`, `capture-fixture.json`, `release-evidence.json`. `outputs/ModuleEngine.exe`, `outputs/game-player-v04`, `outputs/source-project-v04`, and `outputs/team-build-v04` were updated. The backup folder for previous builds is recorded in `backup-path.txt` next to the reports.
