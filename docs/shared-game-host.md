# Shared authored-game host

[English](shared-game-host.md) | [Русский](ru/shared-game-host.md) | [Documentation](README.md)

`module-engine-host` owns authored sessions, transactional Rust Custom adapters,
renderer candidates, content caches and the interactive actor. Applications pass
one cloned `CompositionContext`, resolved asset/compilation/backend locations and
`HostOptions`. It has no production dependency on editor UI, Vulkan, ARPG, Puzzle
or Two Arenas. Gameplay packages supply their pure contributions through the
selected schedule. The editor's `game_host` module only resolves application
policy and forwards construction.

## World and renderer selection

`ModuleRegistry::render_route` follows the viewport's actual frame connection,
renderer scene connection and extraction world connection. It validates registered
implementations without constructing them. The host uses that exact world and a
`Box<dyn FrameRenderer>` from its registered renderer factory. No first-installed
renderer fallback exists. The old editor world selector changes the graph's
built-in world node before entering this shared route.

Default ECS gameplay keeps its canonical ECS as the sole mutable owner and never
constructs a second provider. Registered projection providers are populated from
the authored world by remove/spawn commands and checked for exact equality;
their caches survive subsequent ticks. Legacy graph-only `Runtime` uses the same
registry factories and route policy, independently of an authored session.

`RendererAdapter` provides CPU-only configuration identity, dependency revision,
control targets and validation before GPU construction. Vulkan owns the concrete
pass/HLSL/format checks. `FrameRenderer` provides pixels, portable controls,
optional counters and optional synchronous candidate preparation. A factory can
also implement an independent CPU renderer with no Vulkan dependency.

## Candidate publication

A renderer change validates the current controls and all advertised presets.
Only controls whose previously existing targets were removed are pruned; a fresh
unknown target still rejects. Async preparation uses a separate factory instance.
Completion rechecks the latest document, time and controls before acquiring the
publication locks. Rejected or obsolete candidates preserve the accepted document,
renderer, frame identity and game state. Dependency watches compare success and
error states at most once per 250 ms, allowing a repaired external include to retry.

Vulkan synchronous candidates share the live device/cache but own their graph and
source configuration. Warming a pipeline does not select that candidate. Async
factory calls own separate devices, so preparation cannot reconfigure the active
wrapper. Backend counters report actual work; unavailable counters stay zero.

## Headless and optional services

`RenderMode::Headless { world_node }` selects an explicit registered world without
requiring a viewport or invoking any renderer factory. Composition and component
contracts remain validated. Construction skips GPU/tool initialization, eager
model decoding and native host services. The worker publishes `frame=None`; an
explicit render call returns an error before decoding content.

Active contributions can request generic native services. ARPG requests the
optional `audio.wav` service only when its audio system is scheduled and has cues.
Puzzle requests none. `module-engine-host-services` has no audio feature by default;
the WAV implementation is compiled only when selected. An omitted/disabled build
creates zero service instances and performs no audio file reads or endpoint calls.
Muting an already constructed service preserves logical event diagnostics while
suppressing playback. Replacing its configuration starts fresh diagnostics.

Service consumption/reset occurs after a successful game transaction. A service
error is retained as failed host activity; it cannot report a rejected tick or
restart after the game has already committed. Input acknowledgment, restart epoch
and queued-edge cancellation follow the committed operation. Later successful
frames do not silently hide the service failure.

The actor has one generic retirement atomic. A UI handle can signal it without
waiting for a slow Custom call. WAV ownership prevents a retiring actor from
playing over or stopping its successor's voice. See the service crate README.

## UI boundary and shutdown

`GameWorker::spawn` receives a host and an optional thread-safe wake callback.
The editor supplies an egui repaint closure. The host crate has no egui context.
Snapshots retain the actual completed frame tick, presentation revision and time,
separately from newer state or errors. Commands and captured input edges remain
bounded; one interactive tick yields back to the mailbox. Rejected Custom code,
epoch checks, priority restart and last-good state retain the previous protocol.

Dropping a UI handle signals retirement and shutdown without joining. The owning
thread destroys GPU/Custom/native resources. After the event loop,
`finish_after_ui` signals all remaining owners before joining them. Arbitrary
native teardown side effects after a committed operation are not rollbackable.

The actor boundary prevents a slow simulation call from blocking UI methods; it
does not make an intrinsically slow game system meet its frame budget. GPU output
still uses synchronous CPU readback. Performance acceptance is measured separately.
