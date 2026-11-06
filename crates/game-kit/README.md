# Pure game building modules

[English](README.md) | [Русский](README.ru.md) | [Architecture](../../docs/architecture.md)

`module-engine-game-kit` supplies reusable, independently feature-selected Rust
libraries. It has no dependency on Core, an ECS, an editor, a renderer, native
services, or a specific game. No feature is enabled by default.

```toml
module-engine-game-kit = { path = "../game-kit", default-features = false, features = ["grid", "history", "campaign", "actions"] }
```

These are library modules, not executable graph systems or native services.
Selecting a Cargo feature makes its API available; it does not install a graph
node, create state, start an audio endpoint, read a save, or activate a game.
The consuming source package calls the API from real systems/UI/tools and owns
the corresponding composition, schema, state and service contracts. Do not add
dummy scheduled systems merely to advertise these libraries in a graph.

## Grid

`grid::Cell { x, y }` is an integer coordinate. `GridBounds::new(width, height)`
rejects zero dimensions and dimensions beyond the positive `i32` range.
Deserialization repeats this check. `contains` checks the half-open rectangle
`[0, width) × [0, height)`; `Cell::offset` checks integer overflow.

`Occupancy<Layer, Id>` is a transient validated index. Each ID occurs once, and
each `(Cell, Layer)` has at most one occupant. Distinct layers may overlap;
the library gives no layer a built-in collision or gameplay meaning.

- `insert(cell, layer, id)` rejects duplicates and out-of-bounds positions.
- `get(cell, &layer)`, `position(&id)`, `layer(&id)` and `entries()` inspect it.
- `relocate(&[(id, destination)])` validates a whole batch before publication.
  It permits swaps; unknown/repeated IDs, collisions and outside positions leave
  the original index intact. Empty and unchanged batches are valid.
- `remove(&id)` removes only the named occupant.

Consumers serialize their own authored/save records and rebuild this index. It
is deliberately not another serialized authority for those records. Callers
apply their own terrain, pushing, movement and goal rules before relocation.

`GridProjection::new(bounds, origin, cell_size)` maps centers onto a generic 2D
plane. `origin` is the center of cell `(0, 0)`; a 3D consumer can use this plane
as world XZ and supply its own height. `cell_center` rejects outside cells.
`cell_at` returns the nearest bounded cell, includes its lower edge and excludes
its upper edge, and returns `None` for nonfinite/outside positions. Projection
parameters and extreme output positions must be finite. Output uses `f32` for
UI/renderer compatibility: large floating coordinates are approximate, while
`Cell` remains the exact logical authority.

Tests cover layered board relocation, atomic rejection and a tactical formation
that swaps units in ground/air layers and projects a board for painting.

## History

`history::UndoJournal<T>` stores the current accepted state and its past states,
oldest first. `commit(next)` returns `false` for an equal/no-op candidate;
otherwise it moves the previous current state into history. The consumer must
call it only after accepting its operation. `undo()` restores the previous state
and returns a reference, or `None` when exhausted. A new commit after Undo starts
a new branch; there is no Redo API. `reset(initial)` clears the entire journal.

The journal serializes `current` and `past`, including all Undo states. There is
no fixed history length. Memory and serialized-input limits belong to the
consumer/storage boundary. `validate(callback)` checks every past state and the
current state before a consumer accepts restored data; generic deserialization
cannot validate game-specific payload rules. State may itself be opaque JSON,
which retains any consumer-owned fields.

Tests cover 2,048 accepted moves with round-trip Undo, rejection of an invalid
old state, and an unrelated drawing document branching after Undo.

## Campaign

`campaign::Campaign<Attempt>::new(level_ids, ScoreOrder)` owns an ordered,
nonempty catalog of unique stable IDs, one current level, a sequential unlock
frontier, best `u64` scores, and one opaque current attempt. The first level is
initially unlocked. `LowerIsBetter` supports move/time counts; `HigherIsBetter`
supports points. The module interprets neither level content nor attempt data.

- `select(id)` rejects unknown/locked IDs. Changing level clears its single
  current attempt; selecting the same level preserves that attempt.
- `complete(score)` records a better score and unlocks at most the current
  level's successor. It returns `Completion { new_best, newly_unlocked }`, keeps
  the current selection/attempt, and never navigates automatically. Replaying
  an earlier level cannot skip unlocks or worsen a best score.
- `current`, `levels`, `unlocked_count`, `is_unlocked` and `best` inspect state.
- `set_attempt`, `attempt`, `clear_attempt` manage the opaque current attempt;
  it can contain an `UndoJournal` and game-specific positions/counters.

Deserialization rejects invalid IDs, non-prefix completion records, inconsistent
unlock counts and a locked/outside current selection. Before accepting a save,
the consumer also calls `validate_catalog(authored_ids)` and
`validate_attempt(callback)`. Catalog comparison preserves exact order/IDs;
content changes need an explicit consumer migration. The enclosing save format
owns its version and content/schema identity. A Restart action usually clears
or replaces the attempt while retaining the campaign; the consuming game owns
that decision.

Tests cover move scores, locked selection, replay, attempt persistence and a
racing season using points, including final-stage completion.

## Actions

`actions::ActionMap::new(action_names, bindings)` validates unique nonempty
action names and physical key bindings, and rejects undeclared action targets.
Physical key names are opaque stable strings interpreted by the platform
adapter. One key has one binding; several keys may target the same action.
An action without a physical binding is available to on-screen controls.
Deserialization repeats the same validation.

`Binding { key, action, trigger }` uses `Trigger::Pressed` or `Trigger::Held`.
The platform adapter supplies ordered `PhysicalEvent::Pressed(key)`,
`Released(key)` and `Repeated(key)` records to
`map.evaluate(&mut ActionState, events, focused)`. Pressed records already held
down do not create another edge. OS repeats must use `Repeated`; they can restore
held state after focus returns but never create pressed actions. Unfocused
frames clear state and ignore events. Unknown keys do not grow retained state.
Call `ActionState::reset` when replacing mappings if held keys should reset too.

The resulting `ActionFrame { pressed, held }` preserves edge event order and
deduplicates held action names. Multiple fresh presses may repeat an action in
one frame; the game/host owns queue and one-action-per-tick policy.
`named_press(action)` validates an on-screen button against the same namespace
and creates one pressed action. `ActionFrame::extend` merges UI and physical
results without mutating held physical state. The consumer adapts the result to
Core `GameInput`; this library has no dependency on Core or egui.

Tests cover turn controls/button parity, repeats/event order, malformed bindings,
serialization and an unrelated flight controller with held thrust/focus loss.

## Verification routes

These commands describe crate-level checks; they do not establish native
audio/rendering, editor integration, game completion or measured FPS:

```powershell
cargo test -p module-engine-game-kit --all-features
cargo check -p module-engine-game-kit --no-default-features
cargo check -p module-engine-game-kit --no-default-features --features grid
cargo check -p module-engine-game-kit --no-default-features --features history
cargo check -p module-engine-game-kit --no-default-features --features campaign
cargo check -p module-engine-game-kit --no-default-features --features actions
```
