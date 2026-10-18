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
pass/HLSL/format checks. `FrameRenderer` provides CPU pixels or an opaque GPU lease, portable controls,
optional counters and optional synchronous candidate preparation. A factory can
also implement an independent CPU renderer with no Vulkan dependency.

## Native frame ownership

Core `Frame` has exactly one complete representation: CPU `rgba`, or `gpu: Some(Arc<dyn GpuFrame>)` with empty `rgba`. Core interprets no native handles. The application supplies a `PresentationTarget` through `FrameRenderer::set_presentation_target`. `render_presented` returns `RenderSubmission::Ready(Frame)`, `Pending(ticket)` or `Busy`. Ready is immediate output; Pending accepts one frozen request and publishes nothing yet; Busy accepts no work. Native Vulkan permits one producer submission at a time and four output leases. The explicit `render` contract remains CPU-readable and may synchronously wait for diagnostic readback; it rejects a conflicting pending native submission. Rejected targets cannot silently enable CPU transfer. Reaccepting the same endpoint is idempotent; a different endpoint cannot replace pending work or a live source lease. Renderer candidates retain the accepted endpoint.

Host freezes the accepted request's scene, dimensions, camera, source revision, tick and presentation provenance. Exact-ticket nonblocking polling publishes that envelope once, even if authoritative state has since advanced. Pending and Busy preserve the last completed frame and its metadata. Ready/Pending acceptance consumes only the submitted one-shot command prefix; Busy or a pre-submit error preserves it. Commands accepted by a pending submission are not replayed after a later GPU error. Restart, presentation-history reset, pause or incompatible document/content/target changes discard obsolete publication and cached completion without treating cancellation as GPU completion.

Authored Scene panes use stable process-local owner IDs, separate from authored JSON. Owner 0 preserves the legacy caller; nonzero IDs isolate pane completions. Per-owner ready mailboxes, request deduplication and FIFO demand route each completed frame only to its requesting pane and prevent a moving pane from starving an earlier waiting pane. The bounded owner registry holds at most 64 entries including the legacy owner, rather than an unbounded full-request queue. UI traversal retains visible owners even on display-cache hits; after traversal it removes hidden/closed owners' ready frames and demand, and cancels their pending publication without waiting. Runtime uses the same pending/ready contract. The backend continues owning discarded native resources until actual GPU completion; reaping may wait until the next submission or backend destruction. Scene ray/gizmo/drop projection uses the displayed request camera; new hit/drop geometry is gated until displayed revision and dimensions match current data, while an active gesture retains its captured camera.

This async contract requires separately source-bound verification. Historical reports below retain their original snapshots and do not establish acceptance of the new implementation.

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

## Fixed simulation and interpolated presentation

Interactive gameplay keeps its authoritative fixed 1/60-second step. Host retains
at most the previous and current completed raw scene, their ECS lifetimes and
follow-camera targets. `try_render_interpolated(width, height, alpha)` evaluates
an immutable presentation without advancing systems, Custom, input, events or
audio. The synchronous `render`/CLI route continues to use current state.
This implementation is undergoing P0 verification; its presence is not a passed
performance gate.

Core interpolates safe affine translation/scale and shortest-path quaternion
rotation before `ContentLibrary` expands imported primitives. Matching requires
an unambiguous raw object ID and unchanged lifetime. New/reused IDs, duplicate
IDs, unknown providers and unsupported singular/sheared transforms snap current.
Geometry, materials, components and gameplay remain current discrete values.
The mutable world is never interpolated. Camera follow uses the same state pair;
the frame carries the camera actually used for rendering and aiming. Conventional
previous/current interpolation adds roughly one fixed tick of visual delay.
The point-only follow-target API has no target identity: camera interpolation is
conservatively disabled when the identity/parent maps change, including birth,
death, reparenting or respawn. A module switching targets without changing that
map must signal a camera cut through the same discontinuity event.

Restart, accepted code/content/configuration replacement and scene transitions
discard old endpoints. `SceneChanged` also resets history for the same scene name.
Native systems signal discontinuous motion with `GameEvent::presentation_reset()`;
existing Rust Custom uses `ctx.emit_event(serde_json::json!("PresentationReset"))`.
External owners use `reset_presentation_history()`. No distance heuristic reliably
distinguishes motion from teleportation: teleporting modules must send the signal.
Rejected transactions/candidates do not publish a reset.

`RenderedPresentation` contains the actual frame, camera and
`PresentationProvenance`: history generation, source tick/time pair, alpha,
visual time and origin (`CommittedState`, `InterpolatedPose`, `Invalidated`).
One actual render remains eligible per new committed tick, including static
pixels. Extra sub-tick renders require changed evaluated poses or camera;
alpha or visual time alone cannot qualify a static pair. `None` retains the last
completed frame; failure cannot replace its identity or completion time.
Time-only shader animation has no independent high-frequency capability here.

Extra renders never invoke Custom. Pending Custom-emitted one-shot clear/copy
operations retain order across ticks/failures until a successful render consumes
its submitted batch once. The existing 4096-operation bound applies. Separately,
selected `ReadyVariant`/`project_commands` recipes remain active on each actual
render, including interpolated poses, and use committed simulation time; they do
not run additional Custom or game calls. Headless execution discards GPU operations
at each step call without GPU work or an unbounded command queue.

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

The actor boundary prevents a slow simulation call from blocking UI methods; it does not make slow systems meet their budget. Native output uses shared leases without normal CPU pixel transfer; the producer checks GPU completion through nonblocking exact-ticket polling. GUI submissions/retirement remain bridge-owned; explicit diagnostics retain readback. New async sources require separately bound checks and an independent conclusion in the [async report](gpu-async-20261018.md); passing individual current gates does not establish no regression or final acceptance. The [historical GPU report](gpu-presentation-20261018.md) retains Windows source15/Linux source11 results and earlier failures, including the cold gate open at that time. The [18 October continuation](p0-continuation-20261018.md) separately records the later normal cold PASS 40.906/39.9128549 s, Windows22/16 and WSL Linux23/16 and scoped native/package/lifecycle checks for its own sources. These snapshots do not verify the new async implementation or certify hardware Linux, real device loss or physical scanout.