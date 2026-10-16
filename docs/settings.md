<a id="настройки-engine-и-project"></a>

# Engine and Project settings

[English](settings.md) | [Русский](ru/settings.md) | [Documentation](README.md)

EngineConfiguration enables feature modules and defines the allowed Project controls. ProjectConfiguration stores values, ready variants and conditions. PropertyStore stores module-contributed data separately from the architecture graph.

In Project Settings, change exposure and select ACES/Reinhard or FXAA/off. These are real GPU variants. Time/parameter conditions select a ready variant; the first matching rule in a group takes priority, otherwise selection/default is used. A missing variant produces an error without a silent fallback.

In the previous demo project.json, select object 1 in Object Tree. The Legacy Object Inspector shows game.motion.speed of the game.motion component. Its compiled controller reads the saved value; this is a separate typed PropertyStore example, not the game controller of the new game-project.json:

~~~rust
if let Some(speed) = ctx.component_property(1, "game.motion", "game.motion.speed")
    .and_then(|value| value.as_f64())
{
    ctx.set_position(1, [(ctx.time * speed).sin() as f32, 0.4, 0.0]);
}
~~~

Other objects do not receive the field merely because a descriptor is installed. Attach adds participation; Detach archives values. Disabling a feature module removes its contribution from active views/runtime while preserving data for restoration. The archive is included in Full save/export and local history, and excluded from a new Team delivery.

In the legacy PropertyStore, persistent values are bound to the demo's initial objects, while Transform/Color in the old Object Inspector change the live preview. For saved game scenes, use Game / Scene: Transform/Color, components, references, prefab and overrides are written to game.scenes. New objects are created in this authoring window; runtime spawns during Play are not transferred to the source automatically. Actor/health/melee settings in the new example belong to those components, without the legacy Scalar chain.

PropertySchema contains a namespaced id, label, scope, value_type, default, requires_modules/components. Scope: Global, Module, Object, Component {component}. Types: finite Number with inclusive min/max, Boolean, Enum and Text with a UTF-8 byte limit. Visibility depends on the actual active modules/component participation.

Global/Module fields are edited in Project Settings; Object/Component fields are filtered by the selected object. resolved_properties supplies defaults; the archive is not passed to ScriptContext. ctx.parameter/property/component_property and ctx.properties.global/modules read active resolved values.

Engine Exposure binds an exposed numeric scalar to a pass ID/name/range. VariantGroup contains ready variants of allowed RenderCommand. Engine defines the contents/order of variants; Project selects variants/conditions. The current renderer parameter boundary is numeric; Boolean/Enum pipeline choices are represented by ready variants.

Compiled commands are checked against the same policy: an undeclared target, out-of-range value or forbidden pass is rejected before state changes. Legacy schema1/2 is read; GUI/Team initializes the new policy. Team requires initialized:true.

The Networking schema exists only in tests. This mechanism lets a module contribute its own fields; it does not mean a networking subsystem is ready.

<a id="компоненты-игрового-маршрута-04"></a>

## Components in the 0.4 game route

SystemDescriptor.components declares the contribution of a connected game system. The normal inspector filters by active module and whether the object has the component; a separate Advanced JSON view shows the saved authored archive. The game receives only active components, and Team/Player export prepares the same copy. The current form's field structure comes from JSON values: complete enum/range/help schemas for game components are not yet provided by every native module.

A template stores defaults; an individual edit to an inherited field is recorded as a separate dot-path override. Editing defaults affects other instances. System order, bindings, after and the list of connected modules belong to Engine and are frozen for Team. Details and buttons: [editor-game.md](editor-game.md); data format: [scenes.md](scenes.md).
