# End-to-end game acceptance and timing evidence

The game check uses the same `GameHost` as the editor and standalone Player. It
requires the actual Vulkan device. From a full source project:

```powershell
.\outputs\ModuleEngine.exe --game-check .\game-project.json .\outputs\game-verification
```

For an exported runtime package, invoke its `Player.exe` with `--game-check`, the
package's `project.json`, and an output folder. The package path selects the sealed
shader artifacts and precompiled Rust workers; verification does not compile them.
Do not place the output folder inside the sealed package, because additional files
are evidence rather than shipped game dependencies.

## What the check proves

1. The same authored scene and 180 deterministic action frames run through actual
   Object and ECS stores. It compares entities, scene snapshots, emitted events
   and per-system state each tick, including compiled Custom state. Actual Vulkan
   output with imported content must match pixel for pixel.
2. A native validation failure is placed after all Custom steps. The attempted
   tick must restore world objects, executor states, owned worker states and
   renderer controls. Replacing that validator with an accepting implementation
   must let the previous game resume.
3. An unattended player uses movement and attack actions to defeat the original
   20 enemies, enter the real portal and defeat the boss. The test autopilot routes
   around obstacles and moves away during telegraphs. It does not teleport actors,
   remove enemies or issue runtime damage commands. Its authored fixture raises
   player health to 5000 for resilience; enemy damage, attack timings and enemy
   count remain unchanged. This proves progression mechanics, not game balance.
4. A separate authored fixture sets player health to 1. Real enemy attacks must
   emit player death and defeat, and the restart action must restore the scene.
5. Performance uses the supplied shipping document without fixture health edits:
   30 warmup frames followed by 120 measured frames at 1920×1080. The report checks
   that at least 20 living enemies remain and the player is still playing.

Core unit tests independently cover templates and overrides, invalid references,
typed resource dependencies, external commit rollback, inactive component archive
filtering, collision blocking, one-hit-per-target melee and fixed timestep input.

## Performance interpretation

Timing covers the `GameHost` call to simulation, Custom workers, audio consumption,
imported snapshot enrichment, actual Vulkan rendering and GPU readback. It excludes
v-sync, OS/window presentation and editor HUD painting. `mean_processing_fps` is
therefore a processing-rate measurement, not a claim about presented frame rate.

The report includes mean, median, p95 and maximum processing times, simulation and
render timings, over-budget frames, device name, actual enemy counts and the raw
120 timing samples. `target_met` requires the correct workload, mean processing
time no greater than 1000/60 milliseconds, and p95 no greater than that budget. A
missed target is saved as `false`; the test never substitutes a smaller render
resolution or reduced enemy count to make the result pass.

## Evidence files

| File | Purpose |
| --- | --- |
| `game-report.json` | Functional outcomes, fixture disclosures and measured timings |
| `game-parity-objects.png`, `game-parity-ecs.png` | Matching Vulkan geometry/content output |
| `progression-fixture.json` | Exact independently authored high-health test input |
| `game-start.png`, `game-boss-scene.png`, `game-victory-scene.png` | Scene render evidence along progression |
| `boss-warning-state.json` | Actual combat windup and telegraph data |
| `defeat-state.json` | Player death state produced by enemy attacks |
| `game-performance-1080p.png` | Rendered output at the measured resolution |

Rendered PNGs show `GameHost` scene output. They do not include the editor/Player
HUD, so they alone do not prove visible health bars or danger overlays. UI inspection
and a real standalone launch supply that separate evidence. The source project is
unchanged by verification; the fixtures and reports are written only into the
specified output directory.
