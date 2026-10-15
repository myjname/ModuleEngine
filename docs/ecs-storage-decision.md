# Gameplay storage decision

Status: implemented and checked in stage 3. `GameState.world` owns gameplay; the old entity map remains only as a serialized DTO representation. See [the API and lifetime contract](ecs-world.md) and the stage evidence log.

## Requirements

Source modules register dynamic component IDs and JSON schemas. A saved object ID
must survive scene/prefab resolution, commands, exports and the Rust IPC protocol.
The complete world, component values, resources and events must support an owned
checkpoint for failed tick publication. Queries must select component membership
without scanning every object's JSON map. Renderer extraction remains neutral.

## Compared approaches

| Approach | Fit | Additional work |
| --- | --- | --- |
| Extend the existing `EcsWorld` component tables | Existing stable IDs, deterministic ordering and owned cloning; dynamic schema IDs can address columns directly | Component columns, membership queries, reference policy, checkpoint validation and consumer migration |
| Adopt `hecs` | Typed component queries and contiguous archetype storage, independent of our scheduler | Stable ID mapping, schema/native codecs, deterministic extraction and explicit snapshot/restore of all installed component types |

The [hecs World documentation](https://docs.rs/hecs/latest/hecs/struct.World.html)
describes typed queries and archetype storage. Its
[serialization documentation](https://docs.rs/hecs/latest/hecs/serialize/index.html)
requires application-defined procedures because arbitrary components need not
implement serialization. These are useful capabilities, but they do not remove
this project's dynamic schema or transactional requirements.

Choose the smaller extension of `EcsWorld` for this iteration. This is a contract
and migration-cost decision, not a measured claim that it outperforms hecs.
Dynamic components belong in component-indexed columns, with transform, parent,
color and references owned by that same world. A relocated entity map with an ECS
name does not satisfy this decision. Native gameplay reads/updates those columns;
entity-shaped records are temporary serialization/UI/IPC DTOs.

The Object provider remains a compatibility adapter. Its independent hierarchy
continues to serve legacy examples, but it does not gain a second production
gameplay implementation. Stable object IDs remain independent of storage handles.

## Acceptance and limits

Verify dynamic membership/intersection queries, full component rollback after
validation/commit failure, remove/reuse without inherited data, hierarchy and
references, deterministic serialization, prefab compatibility, scene extraction
and ARPG behavior. The ECS route must have one mutable owner; the compatibility
projection is validated before publication. No cache-locality, parallel-query or
large-world performance guarantee follows from choosing component columns.
