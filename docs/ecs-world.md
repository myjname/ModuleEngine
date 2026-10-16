# Gameplay ECS storage

[English](ecs-world.md) | [Русский](ru/ecs-world.md) | [Documentation](README.md)

`GameState.world: EcsWorld` is the mutable gameplay world. Stable author IDs index
Transform, Parent, Color, dynamic component columns and named references. There is
no independently mutable entity map to synchronize with it. This stage is under
verification; see `modular-runtime-progress.md` for acceptance status.

## Module access

Native systems select membership before reading values:

```rust,ignore
let world = &context.world("world")?.world;
for id in world.entities_with(&["example.spin"]) {
    let settings = world.component(id, "example.spin").unwrap();
    let object = world.object(id).unwrap();
    // Stage portable WorldCommands, or edit a component in this tick's world.
}
```

`entities_with` intersects component columns, starting with the smallest column,
and returns stable ID order. An empty requirement selects every entity. Dynamic
values remain JSON described by registered component schemas; this is not a
typed archetype ECS or a parallel query scheduler.

`component_mut`, `insert_component`, `remove_component`, `transform_mut` and
`references_mut` edit the canonical world. A native system may stage intermediate
values, but the whole tick must pass structural, reference and active schema
validation before publication. `upsert_entity` replaces a complete boundary DTO
without beginning a new object lifetime; its references are checked at the
publication boundary, allowing a packet to create mutually referring objects.

`entities_snapshot` and `entity` return owned `GameEntity` DTOs for Inspector,
serialization, IPC and validation. Changing such a DTO does not change the world.
Apply it explicitly or issue commands. Native hot loops use column queries.

## Lifetime and transactions

Subtree removal clears every removed object's columns and outgoing references.
Surviving references to removed targets reject publication until explicitly
cleared. Removing and spawning the same ID starts a fresh lifetime: old components,
references and demo angular velocity cannot leak into the replacement.

`lifetime(id)` identifies the current runtime incarnation separately from the
saved ID. Cloning a checkpoint preserves it; spawning, rebuilding from a DTO or
clearing and recreating an object gets a fresh token. Module caches use this
token to discard per-object cooldowns and death markers on ID reuse. Tokens are
process-local bookkeeping, excluded from semantic world equality and the saved
entity DTO format; they are not author-facing handles.

Per-tick intentions and collision start positions also carry the incarnation.
A source system inserted between authored stages may replace an object, so these
transient records cannot safely use its saved ID alone. Captured combat visuals
retain their original role/style, and target queries or hit flashes do not bind
an earlier action to a replacement entity.

The executor checkpoints the complete world together with portable system state,
resources and events. Custom Rust receives an IPC DTO and returns commands; those
commands apply to an ECS candidate before the renderer controls and worker state
are committed. Failed candidates retain the previously accepted state.

The default ECS session has no secondary provider. Object and explicitly
registered third-party worlds remain compatibility projections of portable object
data. Their validation/commit/rollback still participates in publication, and
their private caches survive ordinary ticks. New gameplay capabilities belong in
the canonical ECS.

## Saved compatibility

The `GameState` wire representation still uses the `entities` map of stable IDs
to DTOs. Deserialization checks duplicate IDs, map-key/object-ID agreement,
hierarchy, finite transforms and named references before constructing columns.
Authored scenes and prefab records retain their existing format. This snapshot
adapter does not create a second live gameplay owner.
