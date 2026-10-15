# Source component schemas and composition

`CompositionContext` holds the installed root `ModuleRegistry` and gameplay
`SystemRegistry`. The system registry owns a cloneable `ComponentRegistry`.
Register a source extension once in the host composition root, with its component
schemas, implementation versions and executable system factories. Inspector,
document validation, Play, CLI and export consume that same context.

`ComponentSchema` declares a stable component ID, one owning source module,
schema version, label, aggregate defaults, typed `PropertySchema` fields and
module/component requirements. The schema version describes the data contract;
the owning module's implementation version is a separate document pin. A shared
component has one schema owner and may be consumed by many systems. Scheduling
any consumer activates its component types; the schema owner's system does not
have to execute. The owner must be installed and participates in active module
availability as a data contribution, including its module dependencies. This
does not add a scheduled node. Root-only schema owners share their existing
implementation version with the system catalog without a duplicate descriptor
or synthetic factory.

Field IDs begin with the complete component ID, followed by a dot and field
path: `example.spin.speed`. Object keys and numeric array indices are supported,
such as `material.base_color.0`. The field scope must name that component.
Prefix-related component IDs are allowed when their declared fields remain
unambiguous: `foo.speed` and component `foo.bar` can coexist, but schema `foo`
cannot declare `foo.bar.x` alongside component `foo.bar`. Registration rejects
that collision in either order because dot overrides select the longest
component ID.
Inspector Apply/Inherit/Remove and resource mutation use the same
`component_override_owner` rule as resolution and runtime stripping. Ownership
uses all inherited and local attachments before removal, including unavailable
schemas. Editing `foo` therefore preserves `foo.bar.value` when `foo.bar` is
attached; its visibility or activation does not transfer ownership to `foo`.
When an unregistered nested aggregate field in `foo` has that same spelling,
Apply retains it in the local `foo` payload instead of generating a dot override
that would address `foo.bar`.
Aggregate defaults preserve nested object/array structure and must agree with
each field default. Resolution fills missing values in attached active components
without changing authored records or overwriting present values. Prefab values,
local values and field overrides retain precedence. Unknown/inactive records are
preserved in authoring and omitted from stripped runtime data.

Numbers, booleans, enums and bounded text reuse the legacy property types.
`ObjectReference { required_components, optional }` stores a nonzero scene-local
object ID; the resolved target must exist and carry the named active components.
`AssetReference { kinds, optional }` stores a project-relative path or stable
`asset://id`. Kinds are allowed filename extensions without dots, compared without
case (`["gltf", "glb"]`, `["png", "jpg", "jpeg"]`, or an extension defined by an
external module). Optional references use JSON null, not an empty string.
Absolute paths, traversal and malformed stable IDs reject.

`ComponentValidationContext` supplies scene metadata, derived active modules,
the derived active component set and optional asset ID/path metadata. Production
uses `Some(map)`, including an empty map: unknown IDs and mismatched file kinds
reject. `source_only` explicitly omits asset referent checks for core convenience
constructors. File existence, hashes and imported dependency checks remain the
host/project-tools responsibility; core does not depend on project-tools.

`register_with_validator` also installs a source semantic callback over the
resolved entity and validation context. Builtin callbacks enforce constraints
such as health.current not exceeding health.maximum, root-only actor/obstacle
transforms and existing portal scenes. They run only for active components.
Inactive malformed component values do not prevent opening the author archive.

Activity is derived from the existing root graph/configuration and authored
execution graph; there is no second persistent activation list. Render component
participation follows the connected renderer. Component module requirements are
availability contracts. They do not reorder execution: `SystemDescriptor.requires`
still means a preceding scheduled module, and `SystemNode.after` names a preceding
instance. Stages and nodes execute in their saved order.

`SystemRegistry::require_module(owner, dependency)` declares native module
availability separately from preceding-system requirements. Composition checks
that both are installed and that an active owner has its dependency selected.
A dependency may be a schema/service module with no executable node. This check
does not move nodes or rewrite `after`.

Root/GPU canvases can temporarily contain incomplete connections while being
edited. Scene/execution edits use the same active schema checks independently of
those incomplete pins. Runtime publication and export additionally require full
graph validation; the last accepted runtime remains available on rejection.

`ComponentRegistry::asset_references` walks registered active asset fields,
including nested and defaulted values, for generic export planning. A new source
component can therefore add fields and resource dependencies without a named
Inspector panel or exporter branch. The existing mesh/model importer remains a
specific render-content workflow.

The shipped source example is `crates/player/src/source_modules.rs`. Its single
`install` call in the shared Player composition registers `example.spin` with speed,
axis and enabled fields and a system that issues real transform commands. Add
its system to Gameplay Systems, bind `world` to the GameWorld resource and apply
the execution draft while stopped. Inspector can then attach Rotation to an
object even in an otherwise empty scene. Change radians/second and axis, apply
fields, save and run. The tests exercise the actual Inspector Apply button,
save/reopen, simulation and Full source export without a special panel branch.

Format4 persists schema owner/version/contract hashes alongside module and
system-state pins. Ordinary loading and saving validate these exact contracts;
Full profile publication and known legacy migration are explicit operations.
See [Engine profiles](engine-profiles.md). The editor source_modules test module
exercises the same shared registration rather than installing a second catalog.
