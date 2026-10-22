<a id="состояние-rust-custom-и-самостоятельные-workers"></a>

# Rust Custom State and Standalone Workers

[English](custom-state.md) | [Русский](ru/custom-state.md) | [Documentation](README.md)

Purpose: Custom timers, counters, and ongoing actions must live between frames, and code replacement must not accidentally reset the game. State belongs to the host. The worker receives a copy and returns the next copy with commands and events. Executing Rust and accepting the result are separate operations.

<a id="объявление-и-пример"></a>

## Declaration and Example

At the start of the source, before the first Rust item or attribute, one declaration comment specifies JSON on a single line. Blank lines, ordinary single-line comments, and a shebang are allowed before it; text inside a Rust body or string literal is not a declaration. Inner attributes can follow the declaration:

```rust
// module-engine-state: {"schema":"attack.v1","initial":{"cooldown":0.0,"swings":0}}
fn update(ctx: &mut ScriptContext) {
    let cooldown = ctx.state["cooldown"].as_f64().unwrap_or(0.0);
    let ready = (cooldown - ctx.dt).max(0.0);
    ctx.state["cooldown"] = serde_json::json!(ready);
    if ready == 0.0 && ctx.input["attack"].as_bool().unwrap_or(false) {
        let count = ctx.state["swings"].as_u64().unwrap_or(0);
        ctx.state["swings"] = serde_json::json!(count + 1);
        ctx.state["cooldown"] = serde_json::json!(0.4);
        ctx.emit_event(serde_json::json!({"Custom":{"name":"swing","payload":{"count":count + 1}}}));
    }
}
```

`schema` is a nonempty name and version, up to 256 bytes. `initial` is an object with initial values. Without a declaration, state is `null`: old programs continue working. Each Custom program has its own state. `ctx.state_schema` contains the declared version. `ctx.input` and `ctx.events` are serialized contracts of the connected game module; the host supplies input and events, while `ctx.emit_event(value)` adds new events for subsequent host validation.

The `initial` structure defines fixed object keys and JSON value types. Numbers of the same JSON type can change value, and strings can change text. New fields, field deletion, or replacing a number with a string are rejected. Arrays can change length: a nonempty initial array defines element structure through its first element. An empty array means an opaque list; the author of the `schema` version controls its content compatibility. `null` remains `null`. The maximum state and each input/output packet is 1 MiB; commands and new events are limited to 4096.

<a id="применение-и-миграция"></a>

## Application and Migration

The **Custom / Rust** window chooses the executor by the selected source. A game `source_id` from Gameplay Systems uses Game instances; legacy Custom in the external graph uses a separate preview executor. Active-version status, errors, reset, and migration in this window apply to the selected executor. For a game source, the counts of instances with the current and previous compiled text are shown: one source can connect to multiple nodes with independent state.

Opening the Game window before applying game Rust is unnecessary: the Rust window starts normal background Game preparation, including for a stopped game. Until the executor is ready and during ongoing compilation, **Apply Rust** is unavailable, with the reason shown beside it. After completion, the next version can be applied. If initial preparation is rejected because of erroneous source, correcting it permits a new attempt. For game-only sources, the editor does not start an additional hidden legacy preview compilation while typing.

An incompatible candidate is bound to specific compiled text. If the source changes after rejection, reset and migration of the old candidate become unavailable: the current Rust must be applied first. This prevents installing a version that differs from the open text. The normal Game UI uses the same operations and checks as the Rust window.

In the native editor, the live GameHost belongs to the persistent game thread. Game, Custom / Rust, and Diagnostics windows read the latest completed snapshots: they do not wait for a game tick or lock the active Custom mutex. A snapshot contains each instance's source and state, so the “current version” label appears after the executor accepts the code. After compilation, status changes to waiting for application between ticks; sending a command alone does not mean application succeeded.

Application, reset, and migration are sent to the game thread with a request number and session epoch. Restart changes the epoch: a candidate whose compilation started before Restart cannot later replace the new session's code. After Stop or Game replacement, old responses are not installed in a new executor. Reset and migration also recheck the rejected candidate's source. On incompatibility, the previous version continues working, and the rejection reason and request number are visible in the Rust window. Synchronous GameHost remains available for CLI and deterministic checks; background execution is described in [GameWorker](game-worker.md).

Compatibility requires the same `schema` and the same initial JSON structure. Changing the initial `cooldown` value from 0 to 1 preserves the current timer. Renaming a field or changing `schema` is incompatible. A candidate can compile successfully without replacing the active program: the previous version continues working until reset or migration is explicitly selected.

The host uses this order:

1. `compiler.compile_kind(source, kind)` builds the candidate in the background.
2. `candidate.inherit_state_from(&active)` transfers compatible state. On error, the previous executable and state are preserved.
3. For an intentional incompatible change: `candidate.reset_state()` or `candidate.migrate_state(transformed_json)`; the host then explicitly installs the candidate. Migration validates the new JSON without automatically guessing fields.
4. `active.tick(&context, timeout)` returns `ScriptTick {commands, speed, state, events, ...}`. This operation does not save state.
5. The host validates all world/render commands and events before changing subsystems. After successful application, it calls `active.commit_tick(&tick)`.
6. If validation or application is rejected, `active.reject_tick(&tick)` preserves previous state, stops the worker, and makes the old response inapplicable.

A response is bound to a specific program and state revision. Repeated commit, commit after reset, or commit from another Custom is rejected. Stop/new Play must reset state through `reset_state()`. `stop_worker()` stops the process while preserving accepted JSON. Compatible hot apply starts a new worker, transferring only declared state; globals are not transferred.

<a id="протокол-и-ограничения"></a>

## Protocol and Limitations

The editor's `Scripts` stores legacy preview results until the shared world/render packet is accepted. `update` itself does not save state; `commit_updates()` saves all prepared sources' states after success, and `reject_updates()` rejects them after an error. A pending result is not overwritten by a second call. Gameplay uses separate instances for nodes of its graph.

On successful background compilation, `poll` transfers compatible state. An incompatible compiled candidate remains available separately, the previous program continues working, and diagnostics explain the need for reset/migration. `apply_rejected_with_reset(id)` explicitly applies new defaults; `apply_rejected_with_migration(id, json)` is the API for an explicitly prepared transformation. The UI must compare `rejected_source(id)` with the current source before application. New compilation removes the old rejected candidate. `reset_game_states()` is used for a new Play session, but not ordinary code edits.

The host maintains one worker per live Custom instance. Two SystemNodes with the same source_id receive separate fork_instance instances and states; ordinary clones of one CompiledScript share that instance. Input and response are JSON lines with `request_id`; the response has a service marker. Queues are bounded, and pipe writes happen outside the calling thread. Timeout, panic, EOF, excess volume, or an invalid ID stops the worker. The next tick restarts it with the last accepted state. Releasing the last clone of an instance terminates the child process.

stderr diagnostics are limited to 8 KiB; stdout that is not a response is limited to 1 MiB per request. The host does not save every output line. Ordinary Rust has OS access: writing a file or another external effect is not rolled back by the transaction. For reproducible game logic, use declared state and commands.

<a id="экспорт-без-компилятора"></a>

## Export Without a Compiler

`compiled.export_precompiled(package/scripts/id)` writes `worker.exe` and `script.json`. The manifest includes the protocol version, entry-point mode, source for identification, state declaration, and pinned dependency versions. Executable paths are relative; traversal and escaping the manifest directory are rejected during loading.

Player uses `CompiledScript::load_precompiled(&manifest, &package_root)`. It does not open Cargo.toml, SDK, or Rust compiler and does not build. The worker searches for relative game resources in the supplied package root. The package can be moved to another directory. Player's initial state comes from the declaration, not the editor's current Play session. Saving game progress between launches is a separate contract.

<a id="проверка"></a>

## Verification

runtime-script tests check real compiled programs, PID reuse, accumulation and rollback of JSON state, hot apply, incompatibility, explicit reset/migration, timeout/panic, invalid response format, and moving a precompiled package that reads its own asset. For a full game result, the host must also validate the world/render and event transaction: successful worker execution alone does not mean commands were accepted.
