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
