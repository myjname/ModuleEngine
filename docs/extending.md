<a id="добавление-модуля"></a>

# Adding a module

[English](extending.md) | [Русский](ru/extending.md) | [Documentation](README.md)

Native extensions in 0.4 are compiled with the source project. For the old world/render graph, use ModuleRegistry and its factories; for shared game subsystems, use SystemRegistry with real ExecutableSystem factories. A descriptor for an ordinary Graph can display a node but does not create its behavior. Programmable HLSL/Rust candidates have a separate hot apply without a native rebuild.

<a id="общая-исполняемая-подсистема"></a>

## Shared executable subsystem

Implement ExecutableSystem::step and, when needed, initialize/stop/validate_tick/commit_tick/rollback_tick. Register SystemDescriptor together with a factory through SystemRegistry::register. The descriptor defines state_schema, typed ports, preceding requires and components consumed by the system. Add a SystemNode instance to game.execution with explicit bindings/after and authored stage order. The executor itself does not check the new module's name in a special branch.

Instance state resides in the supplied JSON, and access to world/input/events is checked by the corresponding SystemContext helpers. Other data uses typed named ResourceStore envelopes. Connect the registry to GameSession::new_with_registry; Player/editor receive their native registrations through the composition root. GameSession::new_with_world_factory is provided for a third independent authored-game store. Compilable example, lifecycle and transaction boundaries: [module-execution.md](module-execution.md); scene format: [scenes.md](scenes.md).

<a id="третий-мир"></a>

## A third world

A ready standalone example is examples/third_world.rs:

~~~powershell
cargo run --locked -p module-engine-core --example third_world
~~~

The example registers world.pyramid and replaces the world node's module in the legacy demo_document. The renderer receives the same SceneSnapshot: a new storage model does not require changing the software renderer. A saved game needs an editable world store with portable WorldObject/commands; a read-only demo snapshot alone is not yet an authored GameSession.

For your own world:

1. Implement WorldProvider::step(dt, speed) and snapshot(). The provider owns the state and storage model; the renderer receives only a snapshot.
2. Create a descriptor through world_descriptor(id, label, description) and call registry.register_world(descriptor, factory). The factory receives node.params.
3. Replace the existing World node's module, preserving its external connections.
4. To support Custom update, implement WorldObject reading and transactional WorldCommand application. The default implementation explicitly rejects editing while retaining rendering support.
5. Assign an implementation version through set_module_version for an independent release and check old projects.

Standard boundary: optional speed:Scalar/commands:WorldCommands → world:World. Object IDs must be stable; their local Transform and parent retain a shared meaning for both world models.

<a id="другой-renderer"></a>

## Another renderer

FrameRenderer::render takes SceneSnapshot, Camera, dimensions and RenderOverrides, and returns Frame. backend_name describes the active backend; validate_overrides checks supported commands. Frame is an RGBA8 image with measured RenderStats.

ModuleRegistry::register_renderer(descriptor, factory) takes a factory Fn(&Node, &GraphDocument) → Result<Box<dyn FrameRenderer>, String>. The descriptor must have a required scene:Scene3D and a single frame:Frame3D output. Optional commands:RenderCommands can be declared for control. The factory receives the nested graph and the sources of the entire document.

The Editor composition root is in crates/editor/src/host.rs. Register the renderer there, along with internal-pass descriptors through register_module. CLI, UI and exporter must receive the same registry. Update the example project and checks; an incompatible old module_versions pin must not be changed silently.

<a id="новый-gpu-проход"></a>

## A new GPU pass

For a custom effect, gpu.custom is usually sufficient, without a new Rust package:

1. Copy a similar shipped HLSL template into shader_sources under a new source_id.
2. Declare PassManifest: stage, entry, inputs/outputs, parameters and, when needed, vertex_entry/raster_mode/workgroup.
3. Connect only matching format/semantic/extent/samples. For example, an HDR filter must precede tone map, and an LDR filter must follow it.
4. Follow the descriptor/push constant/vertex ABI and reflect any new resources in the manifest.
5. Apply HLSL, check the effect in the frame and a compilation error that preserves the last working version.
6. Register external files/includes, export and rebuild the package.

For a permanent built-in pass, add the source to assets/gpu, the descriptor and builtin_manifest/source to module-engine-vulkan, then scheduler execution/validation. A contract change without a new ID requires explicit graph migration.

<a id="новый-вид-визуального-rust"></a>

## A new visual Rust construct

Add support in module-engine-custom only together with exact source-range mapping, type/allowed-edit checking and preservation of all other text. An unrecognized fragment needs Opaque, not invented execution. The command graph and scalar graph have different limitations; ordinary rustc compilation does not imply visual support for all constructs.

Useful extension checks: effects on real snapshots/pixels, rejection of an invalid contract before state changes, text/position preservation and export without dependence on the original absolute path. General tests do not replace this verification.

<a id="вклад-typed-настроек-модуля"></a>

## Contributing typed module settings

ModuleDescriptor.properties contains PropertySchema. The real legacy example motion_property_module in core/settings.rs provides game.motion.speed: Number 0..=8, default 1, scope Component {component:"game.motion"}. The compiled controller of the previous project.json reads it. In the new authored game, components are stored separately in game.scenes/templates, and game-system descriptors declare active participation. Both examples execute data; these are different storage/editing boundaries.

For your own contribution:

1. Define stable namespaced module/field/component IDs, PropertyType, default and scope. Number bounds are inclusive; Text is limited by UTF-8 bytes.
2. Declare requires_modules/components. Do not tie visibility merely to a descriptor's presence in the palette: a field needs an actual active module and participant.
3. Register the descriptor through register_module; a field-only feature may have empty inputs/outputs. Provide a real implementation for runtime behavior; the descriptor does not create it itself.
4. Add the feature to engine.enabled_modules or connect an executable module. attach_component(document,registry,object_id,component_id) adds participation; reconcile_properties moves disabled values into the archive and restores compatible ones.
5. Read ctx.parameter, ctx.property, ctx.component_property or ctx.properties.global/modules. Runtime provides resolved active values/defaults; inactive data is not passed.
6. For Project renderer control, the Engine author defines ExposedParameter target/range and VariantGroup. Check compiled Rust commands against the same policy, instead of only limiting the widget.
7. Verify the actual gameplay/GPU effect, absence of the field for a nonparticipant, detach/restore, bad value refusal and Team export without inactive data. Rebuild native sources and deliberately update the source manifest/implementation pin.

The dynamic Number/Boolean/Enum/Text UI is shared and needs no separate panel for each field. The Networking descriptor in tests checks participation/dependencies; it is not a networking implementation.
