# Optional gameplay packages

[English](optional-gameplay.md) | [Русский](ru/optional-gameplay.md) | [Documentation](README.md)

Engine contracts, reusable ARPG rules and the concrete Two Arenas project have
separate source boundaries. The shared Host has been extracted from the Editor;
ARPG and audio are optional native packages. An independent Puzzle and a separate,
feature-selected Player without ARPG have passed their recorded acceptance.
The Editor installs its authoring packages; this does not require every exported
Player to install them.

See the completed stages 4–8 in the [migration record](modular-runtime-progress.md)
and the [Player package](../crates/player/README.md). Those results belong to the
snapshots stated in their reports. The [current status](current-status.md) tracks
remaining performance, Linux service and hardware-validation limits; this page
is not a new acceptance run.

| Boundary | Source responsibility |
| --- | --- |
| Engine/SDK | Generic composition, schemas, ECS, session/executor and portable host contracts. |
| `module-engine-arpg` | Reusable mechanics, schemas, an editable default schedule and optional presentation. |
| `module-engine-two-arenas` | The actual example project's scenes, prefabs, encounter counts, tuning, cue paths and chosen goals. |

## Core ownership

`module-engine-core` owns authored scenes, composition/schema contracts, the
fixed-step session, transactional system execution and the canonical
`GameState.world: EcsWorld`. It has no player, combat, health, victory or HUD
types. `SystemRegistry::builtin()` installs generic contracts; it does not
install the ARPG systems or construct the two-arena demo.

`GameState` contains the scene ID, simulation time/tick, world, opaque
`module_data: BTreeMap<String, Value>` and `SessionControl`. A module can decode
its own namespaced/versioned JSON record, update its typed data and store the
record again. Core checkpoints preserve the entire map alongside every world
column and entity lifetime. The core neither clears genre fields each tick nor
interprets module JSON for equality. A package can provide its own semantic
comparison when process-local lifetime tokens appear in that JSON.

The world still serializes under the existing `entities` field as entity DTOs.
That compatibility shape does not create a second mutable entity owner. See
[ECS ownership](ecs-world.md).

## Input and event packets

`GameInput` has four fields:

| Field | Meaning |
| --- | --- |
| `values` | Latest held module-defined values, such as movement axes or cursor position. |
| `actions` | A set of pressed action names; `pressed(name)` checks membership. |
| `action_values` | Module-selected context captured with the pressed packet, such as the aim at attack time. |
| `restart` | Common host/session control. |

For example, an attack can retain its captured aim while a queued packet uses
the latest movement axes. The interactive worker queues distinct accepted
pressed packets; it copies their actions/context onto the latest held values
when they are consumed. Held input contains neither pressed actions nor their
captured context. Short frames and failed ticks retain pending actions/context;
a successful fixed tick consumes them. A direct session's pending action names
form a set, so repeated equal names before that tick coalesce. The worker's
separate packet queue preserves accepted occurrences. Restart is an explicit
cancellation boundary.

`GameEvent { name, payload }` carries opaque JSON. A null payload serializes as a
string (`"Victory"`); another payload serializes as one named entry
(`{"Hit":{"attacker":1,"target":2,"damage":8.0}}`). Empty names, malformed
maps and multiple entries are rejected. New modules can emit their own names
directly, without adding enum branches or wrapping every event in `Custom`.
The core validates this envelope before publication. Active source contributions
can additionally validate the event shapes they own; unrelated event names
remain available to other modules.

## Session controls

`SessionControl.halted` stops further simulation ticks. A module's outcome data,
such as ARPG victory/defeat, belongs to its own frame record. Host pause remains
a separate worker control.

`SessionControl.transition` carries a `SceneTransition` with a destination,
explicit entity IDs to carry and an optional spawn translation. The destination,
carried IDs and finite spawn values are checked before tick publication.
`transition(id)` carries nothing; `transition_with(request)` expresses module
persistence rules. Destination hierarchy, transforms and references retain their
scene-local meaning. When supplied, `spawn` replaces the local translation of
**every** explicitly carried entity. The core does not select a `persistent`
component or a player on a module's behalf.

Preflight rejection retains the accepted source state and the pending action. Transition/restart
lifecycle hooks have a separate boundary: arbitrary native/process effects of a
failing source `stop` hook cannot be undone. A failure after a committed tick is
not a promise to rewind that earlier tick or all external lifecycle effects.

## Source installation

An application installs package contracts into the shared `SystemRegistry`:

```rust
let mut systems = module_engine_core::SystemRegistry::builtin();
module_engine_arpg::install(&mut systems)?;
```

Installation registers component schemas/defaults/semantic validators, native
system factories, project validation/preparation hooks and a `GameContribution`.
Installed schema owners can contribute data without being scheduled consumers.
Availability/version metadata does not reorder the authored execution graph.
Unknown and inactive authored JSON remains preserved; runtime preparation uses
the composition's exact active component set.

Project hooks are registered against an installed owner. The core calls the
installed hooks; each package checks whether its behavior is active. ARPG's
audio hook validates WAV references when `game.audio` is scheduled and removes
inactive cue data from the prepared runtime copy, leaving the authored project
unchanged.

`GameContribution` supplies `begin_tick`/`validate_state` hooks and pure callbacks for
initial/follow camera targets, snapshot decoration, script-input adaptation,
event validation and diagnostics.
One or more scheduled activation modules select the contribution. Installation
alone does not activate it. These callbacks use core data and have no egui,
window or graphics-device types. The current UI registry separately associates
the active contribution ID with optional input/HUD callbacks.

The executor runs active `begin_tick` hooks inside the checkpoint before **all**
scheduled systems, including independently installed controllers. ARPG prepares
its transient frame there, so an earlier source controller's current-tick
intentions survive until the consuming ARPG system. This preparation does not
change the authored system order and rolls back with a failed tick.

`validate_state` checks the complete candidate world and module data after all
authored systems and their staged checks, before publication. Initial sessions,
direct session command packets and merged scene transitions use the same active
package guards. They are read-only validation, must accept the package's valid
initial state before system initialization, and must not publish external
effects. This lets a package reject cross-object invariants, such as overlapping
puzzle cells, even when a later source system produced individually valid
components. A rejected candidate retains the previous world and pending action.

## ARPG ownership

`crates/gameplay-arpg` owns the eight `game.*` systems, their component schemas,
typed `ArpgInput`/`ArpgEvent`, `ArpgFrame`, combat queries, transient feedback,
audio project rules and the reusable `gameplay_schedule()`/`ExecutionGraphExt`.
`GameStateExt::arpg()`
decodes its `arpg.frame/1` record; `set_arpg(frame)` stores an updated record.
`ArpgInput::into_input()` and `GameInputExt` bridge its controls to generic
packets, including captured aim for pressed actions. `ArpgEvent` preserves the
established externally tagged event representation at this package boundary.

`boss.victory_on_death` is an optional declarative mechanic with a default of
`false`. A project can opt in to that outcome or install its own outcome system.
The reusable package does not select the Two Arenas boss ID, enemy count, scene
route or goal value.

The crate's default features are empty. Its optional `ui` feature adds egui for
input collection, HUD and projected combat shapes. Pure hit-flash decoration
remains usable without egui and only changes an extracted render snapshot.
Core sessions do not require these presentation types or a GPU. The shared
`module-engine-host` uses registered renderer interfaces and supports an explicit
headless world. Optional service factories are installed by the application;
omitted audio creates no endpoint. See `shared-game-host.md`.

## Actual example project

`crates/example-two-arenas` depends on core and ARPG. Its
`module_engine_two_arenas::demo_project()` combines the reusable schedule with
`content::build_project(execution)`. The content file imports only core contracts
and contains both arenas, the small-monster prefab and placements, boss tuning,
the explicit victory flag, passage destination and cue paths. The example's
`ProjectExt::demo()` belongs to this project package.

`module_engine_two_arenas::document(base)` adds the example's concrete model path,
Custom controller source/state, controller schedule instance and project name to
a supplied engine document. The application selects the engine graph and its
renderer/settings before calling this function; the example does not depend on
Vulkan. It replaces the base demonstration's scalar Custom source and removes
its speed-generator nodes/edges, while preserving the supplied engine route and
non-content settings.

ARPG has no production dependency on this example and no production
`demo_project()` or `ProjectExt::demo()` API. Existing ARPG regressions explicitly
include the example's core-only content file under `cfg(test)` and use
crate-private test conveniences. That fixture inclusion preserves regression
coverage without introducing a production package cycle.

Core regressions now use neutral installed fixtures for clocks, controls,
events, schemas, lifecycle and rollback. ARPG owns reusable combat/parity
regressions, including entity-ID reuse and the seven projected visual tests;
their example-driven cases use the explicit test fixture. The example package
also checks its own content round trip and compatibility with ARPG contracts.
Stage 4 workspace and scoped execution results are recorded in
[migration progress](modular-runtime-progress.md). Headless host and independent
Player builds remain later acceptance stages.
