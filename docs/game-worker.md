<a id="исполнение-game-вне-ui"></a>

# Running Game outside the UI

[English](game-worker.md) | [Русский](ru/game-worker.md) | [Documentation](README.md)

Native Editor and Player pass `GameHost` to a persistent background executor, `GameWorker`. It owns the simulation, Rust Custom instances, game audio, rendering and change preparation. The UI receives the latest completed `Arc<GameSnapshot>` and sends commands. Verification CLI scenarios retain a synchronous `GameHost` to reproduce a specified number of actions; a separate `--game-worker-check` tests the asynchronous route itself.

<a id="зачем-это-нужно"></a>

## Why this is needed

Before the move, one UI frame could perform up to 15 catch-up fixed updates. In diagnostics, Custom with an 80 ms delay took about 1.28 seconds in total: the window processed no events during that time. Simply limiting catch-up would have shortened the freeze but left every slow call on the window thread.

The UI now performs no `step`, Vulkan readback or Custom state requests. The `latest`, `submit_input`, `try_command` methods use only brief `try_lock` calls on their own mailboxes. If these are busy, reads return the previous snapshot, and sends return the original command for retry. These methods do not wait for a tick to finish or for a ScriptSlot lock.

A slow algorithm still slows down the game. Simulation and game rendering belong to one executor: during a long tick, the window displays the latest completed frame. This keeps the editor responsive but does not turn an 80 ms algorithm into a 60 Hz simulation. Tick cost and accumulated lag are displayed separately.

<a id="обновления-и-ввод"></a>

## Updates and input

The fixed step remains 1/60 second. The executor performs at most one tick per iteration, then checks commands again. Unprocessed time is retained in an accumulator capped at 0.25 seconds; there is no infinite queue of catch-up ticks. Pause does not accumulate time for a later jump.

Held movement and aim direction are replaced by the latest input. Attack/Interact/Restart presses have sequence numbers and a separate bounded queue. An accepted press is acknowledged only after the operation finishes; redisplaying a snapshot does not repeat an attack. Restart explicitly cancels the old action queue. The UI retains unsent actions and retries without waiting for queue space.

Control commands have request IDs, responses and a bounded history. Matching repeated requests are not applied twice. Viewport size is a latest-value setting. Graphics changes are prepared separately; the previous renderer runs until the candidate is accepted. A snapshot contains the camera, accepted document, diagnostics and preparation state.

<a id="интерполированный-показ"></a>

## Interpolated presentation

The interactive owner schedules presentation on a separate 120 Hz deadline grid;
simulation stays fixed at 60 Hz. This is a scheduling target, not a guarantee of
output/display frequency. Missed render slots are skipped without a catch-up
queue. Bounded owner turns still check commands/shutdown. Waiting uses the nearest
simulation, presentation or preparation deadline and the existing interruptible
wake semantics. No global Windows timer setting or busy-wait loop is introduced.

Windows uses an owner-held high-resolution waitable timer and an auto-reset wake
event for precise, interruptible deadlines. Input, commands, retirement and final
drain signal the same wake path; a signal before waiting remains pending. Native
handle/timer/wait failure falls back to bounded standard thread parking without
claiming high-resolution support. Linux uses standard parking/unparking. An owned
coalescing notification flag is stored before either native or standard wake;
blocking libraries such as Custom response channels cannot consume that flag
with the thread's shared park token. Standard waiting rechecks it after every
spurious or stale-token wake against one monotonic elapsed-time budget.
Snapshots expose `deadline_wait_strategy` and `deadline_wait_error`; Player reports
the strategies/errors actually observed. Platform-specific verification and
native performance evidence, with their limits, are recorded in the
[P0 report](p0-stabilization-20261017.md).

The coarse Windows wait timeout is a failure watchdog, not the presentation
deadline. Each wait arms the native timer once and keeps one monotonic watchdog
of the requested duration plus 100 ms. An early coarse timeout retries that same
armed timer. Retries use a positive blocking timeout with a 100 ms floor; the
last retry can overshoot the logical watchdog by that floor plus OS clock and
scheduling delay. Healthy timer/event signals return immediately without this
grace. If the wake primitive itself fails, immediate interruption cannot be
guaranteed; the timer/watchdog bounds recovery while the UI remains independent.

Alpha uses measured pending/elapsed time and never exceeds 1. Overdue simulation
shows committed state instead of extrapolating through backlog. Pause/fault uses
current state without a 120 Hz loop. Host returns `None` for unchanged evaluated
presentation, preserving the old frame/timestamp. The [Host contract](shared-game-host.md#fixed-simulation-and-interpolated-presentation)
defines safe interpolation, discontinuities and the roughly one-tick visual delay.
A slow Custom call still delays this owner; extra presentation never runs game
ticks or hides simulation lag.

`GameSnapshot.frame_presentation` belongs to completed pixels: actor ID, epoch,
successful serial, source revision, actual camera and Host provenance.
`frame_tick`/`frame_revision` retain their source meaning. `sequence` counts status
publications, not frames. Success alone advances identity and samples
`frame_completed_at`; `None`/failure preserves the whole old tuple even when
snapshot state/epoch advances. `camera` uses the completed frame's camera for input
projection. Player reports provenance and simulation progression separately;
counter changes cannot establish cadence. For the 17 October 2026 P0 snapshot, the source-bound Windows measurements passed the unchanged native ARPG and target-cold gates; Linux extras in that same historical scope passed their applicable native/package and functional Puzzle/Team/relocated-Player checks on WSLg/X11 software Vulkan. The Linux result is not ARPG FPS, cold, hardware or Wayland acceptance. The corrected Scene-input contract has targeted Windows/Linux green evidence; the three Scene manipulators are independently observed in a foreground session. See the [P0 report](p0-stabilization-20261017.md) for exact snapshots, artifacts, failed runs, evidence limits and the final independent decision status.

## GPU leases and backpressure

Normal native snapshots may carry `Frame.gpu` with empty `rgba`. The renderer admits one pending producer submission and four leased output slots, without waiting for normal GPU completion or reading pixels to CPU. Ready/Pending acceptance consumes only the frozen one-shot command prefix; Busy and pre-submit failure preserve it. A later GPU error does not replay accepted commands. Only exact-ticket completion advances the completed frame tuple; pending work, Busy, repeated images and failures preserve the previous frame/provenance/completion time/camera.

The owner polls pending completion before simulation and independently of presentation deadlines, including paused/static/hidden views. A ready result is published to the latest snapshot immediately before a potentially slow next step. After every actual step attempt, including success or rollback, completion is polled again and published before admitting a successor render; a completed image is not held behind another tick or poll wait. An actual pending ticket adds a bounded 1 ms wake deadline; a buffered ready frame alone does not run that poll loop. Completion polling itself neither submits a successor nor advances the submission grid; admission retains the existing absolute-deadline/skip-missed-slots schedule. Completion uses the submitted camera, source revision, tick and provenance, rather than newer mutable state. Restart, Play/pause history reset and invalidating scene/document/content/target changes discard obsolete pending publication; UI detachment/Stop never waits for a producer fence. Shared Scene panes route ready frames by owner and prune hidden/closed owners as described in the [Host contract](shared-game-host.md#native-frame-ownership). The GUI bridge separately owns actual consumer completion; explicit diagnostics retain synchronous readback.

New async sources require separately bound checks and an independent conclusion in the [async report](gpu-async-20261018.md); passing individual current gates does not establish no regression or final acceptance. The [historical GPU report](gpu-presentation-20261018.md) retains Windows source15/Linux source11 results and earlier failures, including the cold gate open at that time. The [18 October continuation](p0-continuation-20261018.md) separately records the later normal cold PASS 40.906/39.9128549 s, Windows22/16 and WSL Linux23/16 and scoped native/package/lifecycle checks for its own sources. These snapshots do not verify the new async implementation or certify hardware Linux, real device loss or physical scanout.

`PresentationPhase` retains the existing one-shot correction for an observed successful advancing simulation step that reaches/crosses its render deadline; ordinary noncolliding steps retain the absolute presentation grid. A synchronous successful publication still requires matching epoch/current tick and finite nonnegative phase below the existing 8.333333 ms interval.

For an asynchronous flight that crosses a step, the owner keeps a crate-private identity `(ticket, generation, source_revision, current_tick)` from the submitted envelope, plus the actor epoch and frozen render deadline. It records a collision only when the same exact flight exists before and after one successful step, that step advances exactly one tick from the submitted current tick, and the frozen deadline has been reached by step end. A second step attempt invalidates the observation; resets, errors, cancellation, or a missing/foreign/replaced flight cannot inherit it. A later valid completion must match that identity, submitted provenance, epoch, the recorded single advanced live tick and the same frozen deadline, while Playing without a fault. Finite phase in `[0, 8.333333 ms)` supplies the required headroom. Only then may this armed correction move the next deadline once to `max(existing next deadline, completion + interval)`, preserving an already later deadline. Mere Pending/Busy/empty polls or a failed step never trigger it.

Only the existing grid resets rearm correction: Play/resume, actual valid viewport change, accepted apply/invalidate and successful playing Restart; Pause disables it. The policy changes neither submitted/completed frame metadata nor simulation clocks/state/camera/provenance, and keeps fixed60Hz simulation and the120Hz presentation scheduling target. There is no new cap, forced phase or continuous completion-based reanchoring; final source-bound tests and performance belong to the async report.

<a id="play-stop-и-custom"></a>

## Play, Stop and Custom

Pause changes the executor's state after the current operation. The UI keeps running while that operation finishes. Stop detaches the current executor and returns the authored scene; future snapshots from the old executor are no longer accepted. Closing and replacing GameWorker only signal a stop and do not call `join` on the UI. Its owning thread releases its Host, GPU and child processes.

After returning from the window loop, `main` calls `finish_after_ui`: it first signals all surviving owners, then waits for them to finish outside the UI. This prevents the application process from exiting before persistent Rust processes are destroyed. Removing an executor remains nonblocking while the editor is running.

Host preparation, Custom compilation and the authored preview are registered as cancellable background tasks. After the game owners finish, application shutdown cancels and waits for these tasks, then cleans up their artifacts. On Windows, Cargo and Custom belong to separate Job Objects: cancellation directly terminates the process tree owned by the application and waits for its exit before releasing files. Closing the last handle when an owner exits abnormally also terminates its child processes. `taskkill` is not used for this. This manages process lifetimes; it is not a security sandbox for user code.

Restart uses the shared `GameHost::restart`: it recreates world and Custom state and resets renderer commands, audio and the camera target. It increments the session epoch. Compiled Custom started for an old executor or epoch is rejected. This prevents a late build from being applied after Restart or project replacement.

Ordinary Apply preserves compatible state. An incompatible candidate stays with the executor; the UI receives the source and rejection reason, and sends Reset/Migration as a separate explicit command. Text edited after rejection is not applied through the old candidate. A tick error stops retries until an explicit Restart or successful Custom apply; window repainting alone does not rerun faulty code.

<a id="как-проверять"></a>

## How to verify

```powershell
.\outputs\ModuleEngine.exe --game-worker-check game-project-v4.json outputs\verification-v04\game-worker
.\verify.ps1 -NativeBenchmarks
```

Worker acceptance uses real compiled Rust Custom and checks mailboxes while it runs, delivery of presses, session control and stale candidates. Native benchmarks measure the UI and completion of new game frames separately. Displaying the previous `Frame` again does not count as a new game frame. Overall audit results and limitations are in [stability-audit.md](stability-audit.md).
