# Reusable gameplay modules and the acceptance game

The two-arena demo is data in `GameProject::demo()`, not a scene-name switch in the
executor. `SystemRegistry::builtin()` installs small reusable systems. Removing a
system from the schedule removes that behavior; components configure participating
objects. Unknown component names are retained for separately installed modules.

| System | Responsibility | Object components/resources |
| --- | --- | --- |
| `game.input` | Map host actions to player intentions | `player`, `GameInput` |
| `game.ai` | Choose an opposing target, chase and request attacks | `actor`, `ai`, `melee` |
| `game.movement` | Advance root actors and produce previous positions | `actor`, `Positions` |
| `game.collision` | Prevent movement through obstacles, slide along axes | `actor`, `obstacle`, `Positions` |
| `game.combat` | Cooldowns, windup, attack areas, per-swing hit tracking | `actor`, `health`, `melee` |
| `game.health` | Death, removal, defeat and boss-triggered victory | `health`, `player`, `boss` |
| `game.portals` | Conditional scene loading and persistent player carry | `portal`, `persistent` |
| `game.audio` | Convert gameplay events into configurable sound cues | `GameEvents`, project cue paths |

## Controls and host integration

`GameInput.movement` is X/Z movement from held WASD, normalized if it exceeds unit
length. `aim_world` is the cursor's ground-plane intersection supplied by the
viewport camera. `attack` is the LMB pressed edge. The core never reads an OS
keyboard or uses editor navigation keys; an editor viewport and a standalone
Player provide the same portable actions. `restart` reloads the entry scene.
`interact` is reserved input and currently has no built-in interaction behavior.

Call `session.step(real_frame_seconds, &input)`, inspect `events()` and `state()`,
enrich `snapshot()` with imported assets, then render it. The HUD can query the
player's `health` component, the living enemy count, boss health and the
`Playing`/`Defeat`/`Victory` status. `telegraphs` expose actual attack position,
direction, arc, range and windup progress for visible danger indicators.

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

The host obtains the preview with `GameState::melee_aim()` and candidates with
`melee_targets()`. Candidates need a valid `actor` and `health` component with
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
changes. `crates/editor/src/combat_visuals.rs` draws the shared feedback;
`GameHost::render()` applies hit flashes to the extracted render snapshot after
material overrides, without changing actor colors or material component data.
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
its authored melee values, large health and `boss.victory_on_death`.
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

Events include `Attack`, `Hit`, `Death`, `SceneChanged`, `Victory`, `Sound` and an
extensible `Custom` event carrying JSON. Audio cues map `attack`, `hit`, `death`
and `victory` to authored project-relative WAV files. `game.audio` only produces
sound events; the platform host performs playback. Removing that system removes
those automatic sound events without changing combat.

## Checks and limits

Core tests cover linked overrides, invalid references, invalid execution order and
resource types, independent third-party factory execution, transactional tick
rollback, state-schema replacement, short-frame input retention, Object/ECS
parity, obstacle blocking, once-per-swing group hits, restart, scene health carry
and boss warnings with evasion.

60 FPS at 1920×1080 with up to 20 active enemies is the agreed acceptance target
on the user's current PC and fixed content. These simulation tests alone do not
prove the full exported Player meets that target; Player frame timings include
rendering, content loading, Custom and audio and must be reported separately.
Character skeletal animation, dynamic physics, asynchronous streaming and game
save files are deferred.
