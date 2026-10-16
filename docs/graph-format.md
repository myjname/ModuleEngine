<a id="формат-проекта-04"></a>

# Project Format 0.4

[English](graph-format.md) | [Русский](ru/graph-format.md) | [Documentation](README.md)

GraphDocument is UTF-8 JSON. The current schema_version is 3; versions 1/2 are read with defaults for new fields. Loading migrates the document in memory; saving an existing file leaves an exact backup. Unknown versions are rejected.

<a id="документ"></a>

## Document

| Field | Contents |
| --- | --- |
| schema_version | Document format version. |
| name | Project name. |
| root | Root Graph. |
| custom_sources | Map of source_id → full Rust text. |
| shader_sources | Map of source_id → full HLSL text. |
| asset_manifest | null or a resource manifest with relative paths and SHA-256. |
| module_versions | Map of module ID → exact version of the installed implementation. A mismatch or missing declared module produces an error; this is an implementation pin, not an independent ABI version. Automatic selection of compatible versions is absent. |
| engine / project / properties | Authored module/exposure settings, permitted Project controls, and active/inactive PropertyStore. |
| game | null or GameProject: entry_scene, scenes, prefabs, execution, and audio_cues. Old graph projects are read with game:null. |

An ordinary Graph has nodes and edges arrays. A Node contains id, module, label, position:[x,y], params, and an optional subgraph. A node identifier is unique within its Graph; nested areas have their own IDs. An Edge specifies from_node/from_port/to_node/to_port. Editor geometry does not define legacy/render execution order: connections determine it. A separate game ExecutionGraph defines stages/nodes in explicit order; its after and typed resource bindings are checked against this order, while canvas positions are saved separately.

Unknown fields in fixed document, graph, node, and edge objects are rejected so that typos do not disappear on resave. params stores arbitrary JSON; the executor of a particular module restricts its permitted keys.

<a id="настройки-и-поля"></a>

## Settings and Fields

engine contains initialized, enabled_modules, and exposure {parameters, variants}. It belongs to Engine. project contains parameters, variants, and rules; these are permitted Project values/conditions. properties contains active, inactive, and detached_components. active/inactive have global, modules, objects, and components; the presence of a component map means the object participates. Inactive data is not passed to runtime. See [settings](settings.md) for details.

The personal area layout is outside GraphDocument: it is saved in .user/<profile>/<workspace>.json. params.code_layout/code_layout_keys remain the authored layout of the Custom graph itself.

<a id="контракты-и-проверка"></a>

## Contracts and Validation

The ModuleRegistry contains ModuleDescriptor: ID, name, description, inputs, and outputs. PortDescriptor defines name, data_type, and required. Root port compatibility is based on exact type matching. An input receives at most one connection; required inputs must be connected. Unknown modules/ports, duplicate IDs, cycles, types, and nested interfaces are checked. An unknown module remains in the document with diagnostics.

A composite module declares external ports and internal interface.input/interface.output. Their params specify port and type. Interfaces must match the external contract. Vulkan additionally checks format, semantic, extent, samples, access, and shader implementation. A typed connection alone does not prove the existence of an executor for an arbitrary architecture.

<a id="legacy-и-gpu-маршруты"></a>

## Legacy and GPU Routes

The retained legacy Runtime requires exactly one viewport.output. Its frame must come from a registered renderer; the renderer's scene must come from scene.extract, and the adapter's world from a registered WorldProvider. Built-in worlds: world.objects and world.ecs. The external API allows a third world or another renderer to be added through source code. The current Player GameHost validates this external blueprint and requires an installed render.vulkan with a nested graph; its game simulation runs separately through game.execution.

The scalar graph supports scalar.value, clock.time, and custom.rust. custom.rust contains params.source_id and optional params.code_layout/code_layout_keys. custom.rust.update uses the source_id parameter and emits commands for connected world and renderer inputs. One source_id cannot be used by both speed and update nodes; a mode conflict produces an error. An unconnected branch does not execute; warnings report such fragments.

render.software3d executes only the established internal input → render.rasterize → output route. render.vulkan executes supported gpu.* passes in a nested subgraph. The complete pass contract and an example params.manifest are in [GPU/HLSL](gpu-hlsl.md).

<a id="авторские-сцены-и-общее-исполнение"></a>

## Authored Scenes and Shared Execution

game.scenes contains named AuthoredScene entries with AuthoredObject: object (ID/parent/local Transform/color), prefab, components, overrides, and references. game.prefabs contains color/component defaults; field overrides use dot paths, such as health.maximum. Play uses a resolved copy with active components without changing the source. The complete format is in [scenes.md](scenes.md).

game.execution contains resources {name,data_type,external,initial} and stages {name,nodes}. SystemNode contains id, module, params, bindings, and after. This uses SystemRegistry with real ExecutableSystem factories and a state schema; stages execute in saved order. The new game-project.json has no legacy scalar/value/clock controllers: actor fields and game systems define movement. Contract examples: [module-execution.md](module-execution.md); the UI draft behavior is described in [editor-game.md](editor-game.md).

<a id="сохранение-и-совместимость"></a>

## Saving and Compatibility

The file API can save a syntactically valid document with a nonexecutable graph or erroneous Rust for later repair; saving does not promise executability. Ordinary Graph/File edits the document itself. The graphical game execution editor has a separate incomplete draft: it does not enter Save/Export until Apply succeeds. The authoring UI rejects an invalid scene in full; erroneous Rust text is saved separately from the last working runtime.

In schema 1, missing shader_sources, module_versions, and asset_manifest, and in schema 1/2 engine/project/properties, take default values. Migration does not automatically convert the software renderer to Vulkan or change an old project's architecture. A backup on explicit save allows returning to the original bytes.

When loading schema1/2, known first-party pins 0.1.0/0.2.0, and for schema3, first-party pin 0.3.0, are converted to the current installed version 0.4.0 through an explicit list of known modules. The file is not rewritten. Arbitrary third-party IDs and other incompatible versions are preserved. Saving/export adds only missing versions, including feature modules; separate package/SDK versions must also be compatible.

A new incompatible version requires explicit migration or a new ID. A third-party version resolver and dynamic plugin installation are not implemented. Team separately validates the frozen [distribution](distribution.md) blueprint, so saving erroneous gameplay text does not permit rewriting the architecture.
