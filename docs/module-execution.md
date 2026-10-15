# Executable systems and the authored schedule

The original render graph describes the selected world, extraction and renderer.
The optional `GraphDocument.game.execution` describes simulation. Its stages and
nodes execute in saved order; the executor never guesses an order from module
names. Changing this schedule is Engine authoring, while scene/component values
are Project authoring.

## Why this boundary exists

Gameplay previously required the host to recognize special nodes. `SystemRegistry`
now installs a `SystemDescriptor` and an executable factory. `SystemExecutor`
creates every configured instance from its factory and invokes its lifecycle. A
new module does not require a new branch in the executor. The tests register a
counter producer and consumer that are absent from the built-in registry and
verify their execution and rollback.

Each descriptor has a module ID, explicit state schema, typed resource ports and
optional preceding module requirements. `components` declares the component
types whose fields this installed system contributes to the inspector; inactive
system contributions are absent from runtime entities without deleting authored
values. A node's `params.components` can explicitly declare additional component
types consumed by Custom Rust. Each saved `SystemNode` has a stable
instance ID, module ID, parameters, pin-to-resource bindings and explicit `after`
constraints. A stage contains its ordered nodes. Validation rejects:

- Missing factories, duplicate stages/instances/resources and unknown bindings.
- Resource types that do not match the declared port type.
- Reading a resource before an earlier node has produced it, unless external.
- A required predecessor appearing later or missing entirely.

External resources are host-owned initial values. Current gameplay declares
`GameWorld`, `GameInput`, `GameEvents` and produced `Positions`. `GameWorld` is a
typed capability exposed as `SystemContext.game`; it is not copied into a large
JSON blob for every node. `world(port)`/`world_mut(port)` check a declared service
binding and return the portable world; `resource(port)` explicitly rejects a
GameWorld service rather than returning its null envelope. Input and events also
have typed context access through `input_resource`, `events_resource` and
`events_resource_mut`. Their JSON resource envelopes are refreshed before each
module: the input is the current host input, and events contain earlier modules'
events. Within a module the typed events access is canonical; its JSON envelope
is an entry-time snapshot. Custom
resource types use `ResourceStore` JSON envelopes and exact type-name comparison.
They do not yet provide a general language-independent binary ABI or schema
checker for arbitrary custom JSON payloads. Native source is trusted and can use
the public context directly even without a pin; metadata declares scheduling and
editor contracts, not a security boundary.

## Installing a source module

```rust
use module_engine_core::{ExecutableSystem, SystemContext, SystemDescriptor,
    SystemRegistry};
use serde_json::Value;

struct ClockCounter;
impl ExecutableSystem for ClockCounter {
    fn step(&mut self, ctx: &mut SystemContext<'_>, state: &mut Value)
        -> Result<(), String> {
        *state = Value::from(state.as_u64().unwrap_or(0) + 1);
        ctx.events.push(module_engine_core::GameEvent::Custom {
            name: "counter".into(), payload: state.clone(),
        });
        Ok(())
    }
}

fn install_counter() -> Result<SystemRegistry, String> {
    let mut registry = SystemRegistry::builtin();
    registry.register(SystemDescriptor {
        id: "example.counter".into(), state_schema: "counter/1".into(),
        components: vec![], ports: vec![], requires: vec![],
    }, |_| Ok(Box::new(ClockCounter)))?;
    Ok(registry)
}
```

Add its `SystemNode` to the saved execution graph, then construct
`GameSession::new_with_registry(project, world_kind, registry)`. Native source
modules currently require rebuilding the application, as in the earlier stage.
The host may register a compiled Rust Custom adapter through this same boundary.

The installed `custom.rust.update` adapter declares `world: GameWorld` ReadWrite,
`input: GameInput` Read and `events: GameEvents` ReadWrite. Its saved node must bind
all three services to matching declared resources, just like a native gameplay
module; the canvas shows these ports and validates their connections. The node's
`params.source_id` selects code from `custom_sources`, and optional
`params.components` declares extra component names that this code consumes.

## Lifecycle, state and errors

`initialize` runs before an instance's first tick. `step` receives the fixed tick,
typed input, mutable game world, events, resource bindings, and that instance's
owned JSON state. `stop` runs in reverse order when a scene is replaced/restarted
and on session shutdown (Editor Stop or Player exit). Each initialized instance
is stopped once; shutdown tries all hooks even if one fails. Drop performs
best-effort cleanup because destructor errors cannot be returned to the caller.
Put persistent state in the supplied state value instead of an unobservable
process-global variable. The built-in combat module stores cooldowns, windup,
attack direction and already-hit targets in that state.

A failed tick restores the game state, owned system states, resource values and
events to their pre-tick values. Systems stage external outputs during `step`.
`SystemExecutor::step_with_transaction` runs every system step, every
`validate_tick`, then the host `TickTransaction::validate` and `commit`, and only
then the system `commit_tick` hooks. In `GameSession`, that host transaction
validates and applies the portable command packet to the selected World provider.
Thus a provider rejection happens before script state or renderer overrides are
published. The plain `SystemExecutor::step` uses a no-op host transaction.

A failure at any step, validation or publication invokes `rollback_tick` on
entered systems in reverse order, rolls back the host transaction, then restores
owned values. The compiled Rust adapter uses these hooks to retain the previous
script state and renderer overrides. A custom host target must implement
`TickTransaction::rollback` for changes outside the executor's owned data. Native
module effects such as file writes, OS calls and hidden mutable fields require
their own rollback; they are not made reversible by the JSON snapshot. A rollback
failure is reported explicitly. Source modules are trusted code, not sandboxed
plugins.

`replace_system(instance, registry, migration, reset)` constructs a replacement
first. Matching schemas retain state. A different schema requires an explicitly
supplied migrated value or `reset = true`; otherwise the previous implementation
continues to be installed. This is an executable system boundary, independent of
the compiler-specific Custom source editor.

## Simulation and rendering

`GameSession::step(real_dt, input)` advances at exactly 1/60 second per tick. It
retains an attack edge until a tick occurs and consumes that edge once when
catching up multiple ticks. Movement remains held. Rendering consumes the most
recent immutable scene snapshot independently of the number of simulation ticks.
Catchup is capped at 15 ticks per call; excess wall time is discarded, preventing
an unbounded backlog after a pause. Render interpolation and selectable simulation
rates remain future work.

`GameSession::new_with_world_factory` permits a separately implemented editable
world store. The default constructors use the actual independent Object and ECS
stores, not a single dictionary disguised as two providers. Both consume the same
portable objects and produce equivalent snapshots.

The session retains one World provider across fixed ticks and direct
`GameSession::apply_commands` calls. It creates a replacement only when entering a
scene, transitioning or restarting; provider-specific storage and caches are not
rebuilt every tick. A provider must expose stable portable objects and implement
side-effect-free `validate_commands` plus atomic, all-or-none `apply_commands`.
After publication the host checks that the provider's portable objects match the
candidate game state. If a later system commit fails, the host computes and
applies the inverse portable packet. Private provider state or external effects
must honor this rollback contract themselves; the host cannot reconstruct them
from `WorldObject` records.
