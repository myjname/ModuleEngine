# Reusable gameplay modules and the acceptance game

[English](gameplay.md) | [Русский](ru/gameplay.md) | [Documentation](README.md)

The actual Two Arenas project is data in
`module_engine_two_arenas::demo_project()`, in `crates/example-two-arenas`.
Its `ProjectExt::demo()` is an example-owned convenience adapter. The optional
`crates/gameplay-arpg` package installs its schemas, systems and presentation
through `module_engine_arpg::install(&mut registry)`. Core's
`SystemRegistry::builtin()` contains generic contracts. Removing an ARPG system
from the authored schedule removes that behavior; components configure
participating objects. Unknown and inactive authored component JSON is retained.
Stage 4 checks are pending; the current editor/Player still installs this package.
See [optional gameplay contracts](optional-gameplay.md) for the ownership boundary.

Engine/SDK contracts, reusable ARPG mechanics and concrete project content are
separate. ARPG supplies `gameplay_schedule()` and source-owned hooks; Two Arenas
supplies scene IDs, templates, enemy placements/counts, tuning, cue paths and
chosen goals. ARPG's existing example-driven tests include the example's
core-only content file under `cfg(test)`; production ARPG has no dependency on
the example package.

The example's `document(base)` additionally supplies its crystal model reference,
Custom controller source and counter state, schedule instance and project name.
The caller supplies the configured engine/renderer document; these project
choices do not live in the editor host or require an example-to-Vulkan dependency.

| System | Responsibility | Object components/resources |
| --- | --- | --- |
| `game.input` | Map host actions to player intentions | `player`, `GameInput` |
| `game.ai` | Choose an opposing target, chase and request attacks | `actor`, `ai`, `melee` |
| `game.movement` | Advance root actors and produce previous positions | `actor`, `Positions` |
| `game.collision` | Prevent movement through obstacles, slide along axes | `actor`, `obstacle`, `Positions` |
| `game.combat` | Cooldowns, windup, attack areas, per-swing hit tracking | `actor`, `health`, `melee` |
| `game.health` | Death, removal, defeat and opt-in death-triggered victory | `health`, `player`, `boss` |
| `game.portals` | Conditional scene loading and persistent player carry | `portal`, `persistent` |
| `game.audio` | Convert gameplay events into configurable sound cues | `GameEvents`, project cue paths |

## Controls and host integration

`ArpgInput.movement` is X/Z movement from held WASD, normalized if it exceeds unit
length. `aim_world` is the cursor's ground-plane intersection supplied by the
viewport camera. `attack` is the LMB pressed edge. `into_input()` writes held
values into generic `GameInput.values`, action names into `actions` and captures
pressed-action aim in `action_values`. Thus a queued attack retains its aim while
movement uses the latest held axes. The core never reads an OS keyboard; the
package's optional UI callbacks collect these controls for editor Play and
Player. `restart` reloads the entry scene. `interact` is reserved input and
currently has no built-in interaction behavior.

Call `session.step(real_frame_seconds, &input)`, inspect `events()` and `state()`,
enrich `snapshot()` with imported assets, then render it. The HUD can query the
player's `health` component, the living enemy count, boss health and the
`Playing`/`Defeat`/`Victory` status through the package's `GameStateExt` and decoded
`ArpgFrame`. The frame is stored under `GameState.module_data["arpg.frame/1"]`.
Its `telegraphs` expose actual attack position, direction, arc, range and windup
progress for visible danger indicators. Core session halt is a separate control.
The active contribution's `begin_tick` hook prepares this frame before all
scheduled systems, including source controllers, without changing their authored
order.

## Melee feedback

Editor Play and the standalone Player use the same combat presentation. While the
cursor is over the active game viewport and no player swing or windup is active, a pale
blue ground sector and arrow show the prospective melee area and aim. Gold rings
mark every eligible living opposing actor. During an active player swing, the
blue preview is hidden: the gold sector, moving slash and target rings all use
the swing's captured `attack.area`, even if the cursor moves. After the swing,
the blue preview follows the cursor again. A configured player windup also hides
the cursor preview, because the attack direction is already captured. Opposing swings and their windup
warnings use red. Confirmed hits briefly lighten the target's rendered model
for 0.16 simulated seconds and produce sparks and a rising damage label:
incoming hits on the player are red, while outgoing hits on opponents are gold.
Sparks and labels remain visible when the health system has already removed the
target.

The package's `GameStateExt` supplies `melee_aim()` and `melee_targets()` for
preview and candidates. Candidates need a valid `actor` and `health` component with
`current > 0`; an actor without valid health is not a damage target. Both
presentation and combat use `MeleeArea::contains()` on the X/Z plane: radial
reach is `range + target.radius`, while the angular test uses the target's
center. A target's center can therefore be slightly outside the drawn range
boundary and still be marked as eligible. The preview describes targets at
their current positions; it does not bypass cooldowns or guarantee that a
moving target will remain in the sector when damage is applied. Rings during
an active swing describe its geometric candidates; the combat instance's hit
list still prevents repeated damage to one target during that swing.

`attack_visuals` describe damage-active swings, using the direction captured at
attack start and progress through the configured duration. `hit_visuals` capture
the target position, attacker, target ID, damage and simulated timestamps before
death/removal. They remain for 0.35 seconds of game time. Pausing stops this clock;
Stop, Restart and scene loading create fresh presentation state. Combat overlays
are hidden on Victory/Defeat. These are transient Play values, not authored scene
changes. `crates/gameplay-arpg/src/ui/combat_visuals.rs` draws the shared feedback;
the package's pure snapshot contribution applies hit flashes after material
overrides, without changing actor colors or material component data.
Ground sectors, rings, slash trails and labels are projected HUD shapes on top
of the scene image; they do not provide skeletal animation or depth-tested
particle effects.

## Components

```json
{
  "player": {},
  "persistent": {},
  "actor": { "team": "player", "radius": 0.35, "speed": 6.0 },
  "health": { "maximum": 100.0, "current": 100.0 },
  "melee": {
    "range": 2.2, "damage": 28.0, "cooldown": 0.35,
    "arc": 1.9, "duration": 0.12, "windup": 0.0
  }
}
```

Distances/speeds use scene units; attack arc is in radians; timers are seconds.
Actors and obstacles must currently be root objects. `ai` adds `kind: "chase"`
or `"boss"` and `aggro_radius`. Both use shared chase logic; a boss differs through
its authored melee values, large health and an optional
`boss.victory_on_death`. That flag defaults to `false`; Two Arenas explicitly
sets it to `true` on its chosen goal object. Other projects can keep it false
and register their own outcome rules.
`obstacle.half_extents` contains axis-aligned X/Z half extents. Movement collision
is circle versus rectangle with bounded substeps and axis sliding. It does not
provide rigid body dynamics, actor-versus-actor separation, rotated collision
shapes, navmesh routing or obstacle-aware pathfinding.

A melee swing captures the aim direction at its start. After its configured
windup, it damages every opposing actor in the sector once. Repeated frames of
the same swing cannot deal repeated damage to one target. Boss attacks keep their
captured direction during windup so the player can evade by movement. Cooldowns,
windup and hit target lists are explicit persistent state owned by the combat
instance, independent of the chosen world store.

## Demo progression

The first scene contains 20 small enemies linked to one template, blocking arena
walls, a pillar and a passage trigger. The passage becomes usable after those
enemies die. The destination scene contains a boss with a one-second warning
before a heavy attack. Health carries between scenes. Killing the boss emits
victory; player health reaching zero emits defeat. Restart restores the entry
scene and original health/monsters.

The package's typed `ArpgEvent` includes `Attack`, `Hit`, `Death`, `SceneChanged`,
`Victory`, `Sound` and `Custom`; it converts to the core's opaque named JSON
event without changing the existing wire representation. Other modules can use
their own event names directly. Audio cues map `attack`, `hit`, `death`
and `victory` to authored project-relative WAV files. `game.audio` only produces
sound events; the platform host performs playback. Removing that system removes
those automatic sound events without changing combat.

## Checks and limits

Core regressions cover linked overrides, invalid references, invalid execution
order and resource types, independent third-party factory execution,
transactional tick rollback, state-schema replacement, short-frame action/context
retention and generic restart/transition controls. ARPG owns Object/ECS combat
parity, obstacle blocking, once-per-swing group hits, health carry, boss warnings
with evasion and the projected-feedback regressions. Stage 4 execution results
are pending; [migration progress](modular-runtime-progress.md) distinguishes them
from checks passed before this package extraction.

60 FPS at 1920×1080 with up to 20 active enemies is the agreed acceptance target
on the user's current PC and fixed content. These simulation tests alone do not
prove the full exported Player meets that target; Player frame timings include
rendering, content loading, Custom and audio and must be reported separately.
Character skeletal animation, dynamic physics, asynchronous streaming and game
save files are deferred.
