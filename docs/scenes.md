# Authored scenes, templates and Play copies

`GraphDocument.game` is optional. Existing graph-only files load with no game.
When present it contains a `GameProject`: named scenes, entry scene, named linked
templates, audio cue paths and the authored execution graph. The normal project
file saves these values alongside graph nodes, Rust sources and shaders.

## Object identity and data

`AuthoredScene.objects` contains `AuthoredObject` records. Each holds a portable
`WorldObject` (stable nonzero ID, optional parent, local transform and color), an
optional template ID, component values, field overrides and named object
references. IDs are unique within a scene. Scene loading rejects missing parents,
cycles, nonfinite transforms, missing templates, invalid known gameplay components
and references to absent scene objects. Scene names and portal targets are also
validated before Play starts.

Rendering remains independent of the authoring format. `GameEntity` is the
resolved object plus components and references. `GameSession.world_objects()`
returns provider-neutral object queries. `GameSession.snapshot()` obtains actual
geometry from the selected world provider. An imported model component fragment is:

```json
{
  "components": {
    "mesh": { "path": "assets/models/demo_crystal.glb" }
  }
}
```

Paths must be project-relative and cannot traverse above the project. The content
loader enriches the provider's snapshot from `GameSession.asset_bindings()`.

Active runtime components come from the installed execution nodes' descriptor
contributions. Source Custom nodes can additionally declare component names in
`params.components`. Renderer-neutral `mesh` and `material` remain supported.
Inactive values stay in the authored project but are absent from runtime entities
and script component queries. `prepare_runtime(registry)` strips inactive scene
and template components and their overrides for Player/Team data. It also omits
audio cue paths when the audio module is inactive. Re-enabling an optional module
in the original author project restores its preserved data; an exported stripped
runtime package does not contain that archive. This is data modularity, not native
code removal from the executable.

## Linked templates and instance overrides

A `Prefab` provides component defaults and color. Resolving a linked instance
reads the latest template each time. Instance components merge their immediate
fields over the template's component fields. `overrides` apply explicit dot paths
such as `health.maximum`, `melee.damage` or `color`:

```json
{
  "prefab": "small-monster",
  "components": {},
  "overrides": { "health.maximum": 80.0 }
}
```

The complete record also contains its object and references. Unknown override
paths reject the scene rather than silently creating misspelled settings.
Changing the template's melee damage updates every linked instance. The example
health override remains 80. Position/rotation/scale and parent are instance-local,
so template changes never rearrange placed monsters. This first format does not
yet provide nested prefab hierarchies or template-owned transform defaults.

## Edit and Play

`GameSession::new` takes an owned project copy. Loading and playing resolve that
copy; attacks, movement, spawns/removals, cooldowns and scene changes never modify
the editor's authored project. Stopping Play discards the session. Restart loads
the copied entry scene again. Saving the editor project therefore saves authored
values rather than dead monsters or the player's temporary position.

World command packets have all-or-none validation with identical semantics in
Object and ECS. `GameSession` retains the same provider across fixed ticks and
direct command application; it rebuilds the provider on scene entry, transition
or restart. Removing an object deletes its descendant subtree. New runtime
objects do not automatically become authored objects. Applying runtime changes
back into the authored scene is not implemented implicitly.

During a tick, all systems first stage and validate their changes. The host then
validates and atomically applies the resulting portable world packet before
system commit hooks publish script state or other outputs. A failure restores
the pre-tick owned values and invokes rollback hooks, including an inverse world
packet if publication was attempted. Third-party providers must supply
side-effect-free validation and atomic command application; private storage or
external side effects need their own rollback semantics. See
[module execution](module-execution.md) for the lifecycle contract.

## Scene transitions

`portal` components declare destination scene, trigger radius, destination spawn
and whether all opposing enemies must be dead. Entering an enabled trigger loads
the other scene. Entities with `persistent` components retain their runtime
components (including health), while destination-authored parents, local
transforms and references replace their source-scene values. A carried ID absent
from the destination becomes a root and clears its source-scene references. The
portal's spawn overrides the carried entity position. Other
scene objects and per-system state are unloaded and recreated. A transition emits
`GameEvent::SceneChanged`.

The candidate provider must expose exactly the supplied destination objects
before the current scene is stopped. A rejected load retains the current scene.
If a stop hook fails, its edits to the visible scene and events are discarded;
shutdown itself remains best-effort and exactly-once, so external stop effects
and executor lifecycle flags cannot be rolled back.

This is synchronous scene replacement with a permitted short loading pause. It
does not yet stream scene chunks or save game progress to disk. Authoring data and
gameplay persistence are separate responsibilities.
