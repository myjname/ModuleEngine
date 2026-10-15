//! Real actor acceptance. Client-call timings are not presentation/FPS metrics.
use crate::{
    game_host::GameHost,
    game_worker::{CustomApplyMode, GameSnapshot, GameWorker, WorkerCommand, WorkerReplyKind},
};
use module_engine_arpg::ArpgInput;
use module_engine_core::{GameInput, GraphDocument, WorldKind};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SLOW_CUSTOM_MS: u64 = 80;
const WAIT: Duration = Duration::from_secs(12);

#[derive(Default)]
struct ClientTimings {
    poll: Vec<f64>,
    input: Vec<f64>,
    command: Vec<f64>,
}

#[derive(Serialize)]
struct Statistics {
    count: usize,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}
fn statistics(values: &[f64]) -> Statistics {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| {
        sorted
            .get(((sorted.len() as f64 * fraction).ceil() as usize).saturating_sub(1))
            .copied()
            .unwrap_or(0.)
    };
    Statistics {
        count: sorted.len(),
        median_ms: percentile(0.5),
        p95_ms: percentile(0.95),
        max_ms: sorted.last().copied().unwrap_or(0.),
    }
}
fn ensure(condition: bool, message: impl Into<String>) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn millis(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.
}
fn poll(worker: &mut GameWorker, timings: &mut ClientTimings) -> Option<Arc<GameSnapshot>> {
    let started = Instant::now();
    let result = worker.latest();
    timings.poll.push(millis(started));
    result
}
fn command(
    worker: &GameWorker,
    mut command: WorkerCommand,
    timings: &mut ClientTimings,
) -> Result<(), String> {
    let deadline = Instant::now() + WAIT;
    loop {
        let started = Instant::now();
        let result = worker.try_command(command);
        timings.command.push(millis(started));
        match result {
            Ok(()) => return Ok(()),
            Err(returned) => command = returned,
        }
        ensure(
            worker.is_alive(),
            "Game worker stopped before accepting a command",
        )?;
        ensure(
            Instant::now() < deadline,
            "Game worker command queue did not drain",
        )?;
        thread::sleep(Duration::from_millis(1));
    }
}
fn input(
    worker: &GameWorker,
    mut input: GameInput,
    timings: &mut ClientTimings,
) -> Result<Option<u64>, String> {
    let deadline = Instant::now() + WAIT;
    loop {
        let started = Instant::now();
        let result = worker.submit_input(input);
        timings.input.push(millis(started));
        match result {
            Ok(sequence) => return Ok(sequence),
            Err(returned) => input = returned,
        }
        ensure(
            worker.is_alive(),
            "Game worker stopped before accepting input",
        )?;
        ensure(
            Instant::now() < deadline,
            "Game worker input queue did not drain",
        )?;
        thread::sleep(Duration::from_millis(1));
    }
}
fn wait_snapshot(
    worker: &mut GameWorker,
    timings: &mut ClientTimings,
    reason: &str,
    mut ready: impl FnMut(&GameSnapshot) -> bool,
) -> Result<Arc<GameSnapshot>, String> {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(snapshot) = poll(worker, timings) {
            if ready(&snapshot) {
                return Ok(snapshot);
            }
            if !worker.is_alive() {
                return Err(format!(
                    "{reason}: worker stopped; fault={:?}",
                    snapshot.fault
                ));
            }
        }
        ensure(Instant::now() < deadline, format!("Timed out: {reason}"))?;
        thread::sleep(Duration::from_millis(1));
    }
}
fn wait_reply(
    worker: &mut GameWorker,
    timings: &mut ClientTimings,
    request: u64,
) -> Result<Arc<GameSnapshot>, String> {
    wait_snapshot(worker, timings, "worker command reply", |snapshot| {
        snapshot
            .results
            .iter()
            .any(|reply| reply.request == request)
    })
}
fn script_state(snapshot: &GameSnapshot) -> Result<&Value, String> {
    snapshot
        .custom
        .iter()
        .find(|script| script.source_id == "game-controller")
        .map(|script| &script.state)
        .ok_or("Probe Custom snapshot missing".into())
}
fn counter(path: &Path) -> u64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}
fn wait_counter(path: &Path, previous: u64) -> Result<u64, String> {
    let deadline = Instant::now() + WAIT;
    loop {
        let current = counter(path);
        if current > previous {
            return Ok(current);
        }
        ensure(Instant::now() < deadline, "Probe Custom did not start")?;
        thread::sleep(Duration::from_millis(1));
    }
}
fn case_folder(folder: &Path, label: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let case = folder.join(format!(
        "{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos()
    ));
    fs::create_dir(&case).map_err(|error| error.to_string())?;
    case.canonicalize().map_err(|error| error.to_string())
}
fn probe_document(delay_ms: u64) -> GraphDocument {
    let mut document = crate::game_host::demo_document();
    let game = document.game.as_mut().unwrap();
    game.audio_cues.clear();
    for scene in game.scenes.values_mut() {
        scene.objects.retain(|entity| {
            entity
                .components
                .get("actor")
                .and_then(|actor| actor.get("team"))
                .and_then(Value::as_str)
                != Some("enemy")
        });
        for entity in &mut scene.objects {
            entity.components.remove("mesh");
        }
    }
    document.asset_manifest = Value::Null;
    let source = r#"// module-engine-state: {"schema":"worker.probe.v1","initial":{"ticks":0,"interacts":0,"aims":[]}}
fn update(ctx: &mut ScriptContext) {
    let attempts = std::fs::read_to_string("worker-started.txt").ok()
        .and_then(|text| text.parse::<u64>().ok()).unwrap_or(0) + 1;
    std::fs::write("worker-started.txt", attempts.to_string()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(DELAY));
    ctx.state["ticks"] = serde_json::json!(ctx.state["ticks"].as_u64().unwrap() + 1);
    if ctx.input["interact"].as_bool() == Some(true) {
        ctx.state["interacts"] = serde_json::json!(ctx.state["interacts"].as_u64().unwrap() + 1);
        ctx.state["aims"].as_array_mut().unwrap().push(ctx.input["aim_world"][0].clone());
    }
}
"#.replace("DELAY", &delay_ms.to_string());
    document
        .custom_sources
        .insert("game-controller".into(), source);
    document
}
fn host(
    document: GraphDocument,
    root: &Path,
    backend: &Path,
    compile: &Path,
) -> Result<GameHost, String> {
    let mut host =
        crate::game_host::new_at(document, root, backend, Some(compile), WorldKind::Objects)?;
    host.services.set_enabled(false);
    Ok(host)
}
fn configure(worker: &GameWorker, timings: &mut ClientTimings) -> Result<(), String> {
    command(
        worker,
        WorkerCommand::SetViewport {
            width: 160,
            height: 90,
        },
        timings,
    )?;
    command(worker, WorkerCommand::SetPlaying(false), timings)
}

fn lifecycle(backend: &Path, compile: &Path, output: &Path) -> Result<Value, String> {
    let folder = case_folder(output, "slow-custom")?;
    let marker = folder.join("worker-started.txt");
    let document = probe_document(SLOW_CUSTOM_MS);
    let initial_host = host(document.clone(), &folder, backend, compile)?;
    let stale_custom = initial_host.slots["game-controller"]
        .lock()
        .map_err(|_| "Probe Custom slot poisoned")?
        .fork_instance();
    let mut worker = GameWorker::spawn(initial_host, None);
    let mut timings = ClientTimings::default();
    configure(&worker, &mut timings)?;
    let initial = wait_snapshot(
        &mut worker,
        &mut timings,
        "initial real GPU frame",
        |snapshot| snapshot.frame.is_some() && !snapshot.playing,
    )?;
    ensure(
        initial.state.tick == 0,
        "Paused worker advanced the initial world",
    )?;
    let initial_epoch = initial.epoch;
    let actor_id = initial.actor_id;
    command(&worker, WorkerCommand::SetPlaying(true), &mut timings)?;
    wait_counter(&marker, 0)?;
    // This call burst happens after the actual script entered its 80 ms sleep.
    // No Host, ScriptSlot or GPU lock is touched by the measured client path.
    let burst_started = Instant::now();
    let mut edges = vec![];
    for aim in [101., 102., 103.] {
        edges.push(
            input(
                &worker,
                ArpgInput {
                    interact: true,
                    aim_world: [aim, 0., 0.],
                    ..Default::default()
                }
                .into_input(),
                &mut timings,
            )?
            .ok_or("Pressed edge did not receive a sequence")?,
        );
    }
    for _ in 0..256 {
        let _ = poll(&mut worker, &mut timings);
    }
    let busy_client_burst_ms = millis(burst_started);
    ensure(
        busy_client_burst_ms < SLOW_CUSTOM_MS as f64 * 0.75,
        format!("UI bridge waited for slow Custom: {busy_client_burst_ms:.3} ms"),
    )?;
    let mut newest_document = document.clone();
    newest_document.name = "Newest actor document".into();
    let mut late_document = document.clone();
    late_document.name = "Late stale actor document".into();
    command(
        &worker,
        WorkerCommand::UpdateDocument {
            document: Arc::new(newest_document),
            authoring: false,
            request: 802,
        },
        &mut timings,
    )?;
    command(
        &worker,
        WorkerCommand::UpdateDocument {
            document: Arc::new(late_document),
            authoring: false,
            request: 801,
        },
        &mut timings,
    )?;
    let newest = wait_reply(&mut worker, &mut timings, 802)?;
    ensure(
        newest
            .results
            .iter()
            .find(|reply| reply.request == 802)
            .unwrap()
            .result
            .is_ok()
            && newest.accepted_document.name == "Newest actor document",
        "A late older document request replaced the newer pending request",
    )?;
    let obsolete_document = wait_reply(&mut worker, &mut timings, 801)?;
    ensure(
        obsolete_document
            .results
            .iter()
            .find(|reply| reply.request == 801)
            .unwrap()
            .result
            .is_err(),
        "Obsolete document token did not receive an explicit rejection",
    )?;
    let last_edge = *edges.last().unwrap();
    let accepted = wait_snapshot(
        &mut worker,
        &mut timings,
        "ordered input edges committed",
        |snapshot| snapshot.input_ack >= last_edge,
    )?;
    ensure(
        accepted.fault.is_none(),
        format!("Probe simulation failed: {:?}", accepted.fault),
    )?;
    let state = script_state(&accepted)?;
    ensure(
        state["interacts"] == 3,
        format!("Lost/duplicated input edges: {state}"),
    )?;
    ensure(
        state["aims"] == json!([101., 102., 103.]),
        format!("Edge aim/order changed: {state}"),
    )?;
    ensure(
        accepted.last_step_ms >= SLOW_CUSTOM_MS as f64 * 0.75,
        "Slow Custom did not contribute to the measured worker step",
    )?;
    command(&worker, WorkerCommand::SetPlaying(false), &mut timings)?;
    let paused = wait_snapshot(
        &mut worker,
        &mut timings,
        "Pause acknowledgement",
        |snapshot| !snapshot.playing,
    )?;
    thread::sleep(Duration::from_millis(130));
    let still_paused = poll(&mut worker, &mut timings).ok_or("Paused snapshot disappeared")?;
    ensure(
        still_paused.state.tick == paused.state.tick,
        "Pause allowed another simulation tick",
    )?;
    ensure(
        script_state(&still_paused)?["interacts"] == 3,
        "An input edge was replayed while paused",
    )?;

    command(
        &worker,
        WorkerCommand::Restart { request: 700 },
        &mut timings,
    )?;
    let restarted = wait_reply(&mut worker, &mut timings, 700)?;
    let reply = restarted
        .results
        .iter()
        .find(|reply| reply.request == 700)
        .unwrap();
    ensure(
        reply.kind == WorkerReplyKind::Restart && reply.result.is_ok(),
        "Restart failed",
    )?;
    ensure(
        restarted.epoch > initial_epoch && restarted.state.tick == 0,
        "Restart did not create a fresh epoch/world",
    )?;
    ensure(
        script_state(&restarted)?["ticks"] == 0 && script_state(&restarted)?["aims"] == json!([]),
        "Restart retained Custom state",
    )?;

    command(
        &worker,
        WorkerCommand::ApplyCustom {
            id: "game-controller".into(),
            candidate: Box::new(stale_custom),
            mode: CustomApplyMode::Compatible,
            request: 701,
            epoch: initial_epoch,
        },
        &mut timings,
    )?;
    let stale = wait_reply(&mut worker, &mut timings, 701)?;
    ensure(
        stale
            .results
            .iter()
            .find(|reply| reply.request == 701)
            .unwrap()
            .result
            .is_err(),
        "Old-epoch Custom candidate was applied after Restart",
    )?;
    ensure(
        stale.epoch == restarted.epoch && script_state(&stale)?["ticks"] == 0,
        "Rejected stale candidate mutated the new session",
    )?;

    // With Play paused, input can fill the bounded edge queue without being
    // consumed. Rejected input stays owned by the caller, then retries once.
    let mut pending = ArpgInput {
        interact: true,
        aim_world: [404., 0., 0.],
        ..Default::default()
    }
    .into_input();
    let mut queued = vec![];
    // The protocol reserves 128 pressed packets. Retry transient pointer-lock
    // contention, then confirm that the 129th packet is returned intact.
    for _ in 0..128 {
        queued.push(
            input(&worker, pending.clone(), &mut timings)?
                .ok_or("An interact edge was accepted without a sequence")?,
        );
    }
    let started = Instant::now();
    let full = worker.submit_input(pending);
    timings.input.push(millis(started));
    pending = full
        .err()
        .ok_or("Pressed-edge queue exceeded its capacity")?;
    let full_queue_restart = input(
        &worker,
        GameInput {
            restart: true,
            ..Default::default()
        },
        &mut timings,
    )?
    .ok_or("R must remain reachable when the paused input queue is full")?;
    let cancelled = wait_snapshot(
        &mut worker,
        &mut timings,
        "R cancels a full paused input queue",
        |snapshot| snapshot.epoch > restarted.epoch && snapshot.input_ack >= full_queue_restart,
    )?;
    ensure(
        cancelled.cancelled_input_ack >= *queued.last().unwrap(),
        "Restart did not acknowledge cancelled old inputs",
    )?;
    let retry = input(&worker, pending, &mut timings)?.ok_or("Retried edge missing sequence")?;
    command(&worker, WorkerCommand::SetPlaying(true), &mut timings)?;
    let retried = wait_snapshot(
        &mut worker,
        &mut timings,
        "retained input retry",
        |snapshot| snapshot.input_ack >= retry,
    )?;
    ensure(
        script_state(&retried)?["interacts"] == 1
            && script_state(&retried)?["aims"] == json!([404.]),
        "Cancelled inputs leaked, or retained edge was lost/duplicated",
    )?;
    command(&worker, WorkerCommand::SetPlaying(false), &mut timings)?;
    wait_snapshot(&mut worker, &mut timings, "second Pause", |snapshot| {
        !snapshot.playing
    })?;

    let before_restart_burst = counter(&marker);
    command(&worker, WorkerCommand::SetPlaying(true), &mut timings)?;
    wait_counter(&marker, before_restart_burst)?;
    let obsolete_edge = input(
        &worker,
        ArpgInput {
            interact: true,
            aim_world: [111., 0., 0.],
            ..Default::default()
        }
        .into_input(),
        &mut timings,
    )?
    .ok_or("Pre-R interact missing sequence")?;
    let restart_edge = input(
        &worker,
        GameInput {
            restart: true,
            ..Default::default()
        },
        &mut timings,
    )?
    .ok_or("R input missing sequence")?;
    let after_restart_edge = input(
        &worker,
        ArpgInput {
            interact: true,
            aim_world: [555., 0., 0.],
            ..Default::default()
        }
        .into_input(),
        &mut timings,
    )?
    .ok_or("Post-R interact missing sequence")?;
    let after_input_restart = wait_snapshot(
        &mut worker,
        &mut timings,
        "R preserves later accepted input",
        |snapshot| snapshot.epoch > retried.epoch && snapshot.input_ack >= after_restart_edge,
    )?;
    ensure(
        after_input_restart.cancelled_input_ack >= obsolete_edge
            && after_input_restart.cancelled_input_ack < after_restart_edge,
        "R cancellation swallowed input accepted after the restart edge",
    )?;
    ensure(
        script_state(&after_input_restart)?["interacts"] == 1
            && script_state(&after_input_restart)?["aims"] == json!([555.]),
        "R replayed a cancelled edge or lost/duplicated a later edge",
    )?;
    command(&worker, WorkerCommand::SetPlaying(false), &mut timings)?;
    wait_snapshot(&mut worker, &mut timings, "Pause after R", |snapshot| {
        !snapshot.playing
    })?;

    let replacement_host = host(document, &folder, backend, compile)?;
    let before_busy = counter(&marker);
    command(&worker, WorkerCommand::SetPlaying(true), &mut timings)?;
    let active_attempt = wait_counter(&marker, before_busy)?;
    let dropped = Instant::now();
    drop(worker);
    let busy_drop_ms = millis(dropped);
    ensure(
        busy_drop_ms < SLOW_CUSTOM_MS as f64 * 0.75,
        format!("UI Drop waited for active simulation: {busy_drop_ms:.3} ms"),
    )?;
    let mut replacement = GameWorker::spawn(replacement_host, None);
    configure(&replacement, &mut timings)?;
    let fresh = wait_snapshot(
        &mut replacement,
        &mut timings,
        "replacement actor frame",
        |snapshot| snapshot.frame.is_some() && !snapshot.playing,
    )?;
    ensure(
        fresh.actor_id != actor_id && fresh.state.tick == 0,
        "Replacement accepted the retired actor's state",
    )?;
    let fresh_actor = fresh.actor_id;
    let until = Instant::now() + Duration::from_millis(200);
    while Instant::now() < until {
        if let Some(snapshot) = poll(&mut replacement, &mut timings) {
            ensure(
                snapshot.actor_id == fresh_actor && snapshot.state.tick == 0,
                "A stale actor presentation reached the new actor",
            )?;
        }
        thread::sleep(Duration::from_millis(1));
    }
    ensure(
        counter(&marker) == active_attempt,
        "Retired actor started another Custom tick after Drop",
    )?;
    let shutdown = Instant::now();
    replacement.shutdown();
    let shutdown_dispatch_ms = millis(shutdown);
    let deadline = Instant::now() + WAIT;
    while replacement.is_alive() {
        ensure(Instant::now() < deadline, "Worker did not finish shutdown")?;
        thread::sleep(Duration::from_millis(1));
    }
    Ok(
        json!({"passed":true,"slow_custom_ms":SLOW_CUSTOM_MS,"actual_worker_step_ms":accepted.last_step_ms,
        "busy_client_burst_ms":busy_client_burst_ms,"busy_drop_ms":busy_drop_ms,"shutdown_dispatch_ms":shutdown_dispatch_ms,
        "poll":statistics(&timings.poll),"input":statistics(&timings.input),"command":statistics(&timings.command),
        "ordered_edges":edges,"bounded_queue_accepted":queued.len(),"retry_sequence":retry,"restart_input_sequence":restart_edge,"full_queue_restart_sequence":full_queue_restart,
        "initial_actor_id":actor_id,"replacement_actor_id":fresh_actor,"initial_epoch":initial_epoch,"restart_epoch":restarted.epoch,
        "pause_preserved_tick":paused.state.tick,"old_epoch_candidate_rejected":true,"old_inputs_cancelled":true,
        "replacement_rejects_old_presentation":true,"late_document_token_rejected":true,"input_after_R_preserved":true,"worker_shutdown_completed":true}),
    )
}

fn fault_latch(backend: &Path, compile: &Path, output: &Path) -> Result<Value, String> {
    let folder = case_folder(output, "fault-latch")?;
    let marker = folder.join("worker-started.txt");
    let document = probe_document(240); // Host's actual Custom deadline is 150 ms.
    let mut worker = GameWorker::spawn(host(document, &folder, backend, compile)?, None);
    let mut timings = ClientTimings::default();
    configure(&worker, &mut timings)?;
    let good = wait_snapshot(
        &mut worker,
        &mut timings,
        "fault fixture initial frame",
        |snapshot| snapshot.frame.is_some(),
    )?;
    command(&worker, WorkerCommand::SetPlaying(true), &mut timings)?;
    let failed = wait_snapshot(
        &mut worker,
        &mut timings,
        "actual Custom timeout",
        |snapshot| snapshot.fault.is_some(),
    )?;
    ensure(worker.is_alive(), "Custom failure killed the actor")?;
    ensure(
        failed
            .frame
            .as_ref()
            .zip(good.frame.as_ref())
            .is_some_and(|(actual, initial)| actual.rgba == initial.rgba),
        "Simulation fault discarded/replaced the last-good frame",
    )?;
    let attempts = counter(&marker);
    ensure(
        attempts == 1,
        "Timeout fixture ran more than once before its failure was latched",
    )?;
    command(&worker, WorkerCommand::SetPlaying(false), &mut timings)?;
    command(&worker, WorkerCommand::SetPlaying(true), &mut timings)?;
    let until = Instant::now() + Duration::from_millis(350);
    while Instant::now() < until {
        let _ = poll(&mut worker, &mut timings);
        let _ = input(&worker, GameInput::default(), &mut timings)?;
        thread::sleep(Duration::from_millis(1));
    }
    ensure(
        counter(&marker) == attempts && worker.is_alive(),
        "Repaint/SetPlaying/input retried a latched failing Custom",
    )?;
    command(
        &worker,
        WorkerCommand::Restart { request: 900 },
        &mut timings,
    )?;
    let restarted = wait_reply(&mut worker, &mut timings, 900)?;
    ensure(
        restarted
            .results
            .iter()
            .find(|reply| reply.request == 900)
            .unwrap()
            .result
            .is_ok(),
        "Faulted actor Restart failed",
    )?;
    command(&worker, WorkerCommand::SetPlaying(true), &mut timings)?;
    let retried = wait_snapshot(
        &mut worker,
        &mut timings,
        "explicit Restart retries fault",
        |snapshot| snapshot.epoch > failed.epoch && snapshot.fault.is_some(),
    )?;
    ensure(
        counter(&marker) == attempts + 1,
        "Explicit Restart did not retry exactly once",
    )?;
    ensure(
        retried.state.tick == 0,
        "Failed retry committed simulation state",
    )?;
    command(&worker, WorkerCommand::SetPlaying(false), &mut timings)?;
    wait_snapshot(
        &mut worker,
        &mut timings,
        "Pause while faulted",
        |snapshot| !snapshot.playing,
    )?;
    let restart_input = input(
        &worker,
        GameInput {
            restart: true,
            ..Default::default()
        },
        &mut timings,
    )?
    .ok_or("Fault recovery R edge missing sequence")?;
    let recovered_by_input = wait_snapshot(
        &mut worker,
        &mut timings,
        "R recovers paused fault",
        |snapshot| {
            snapshot.epoch > retried.epoch
                && snapshot.input_ack >= restart_input
                && snapshot.fault.is_none()
                && !snapshot.playing
        },
    )?;
    ensure(
        recovered_by_input.state.tick == 0 && script_state(&recovered_by_input)?["ticks"] == 0,
        "R did not reset failed Custom state while paused",
    )?;
    command(&worker, WorkerCommand::SetPlaying(true), &mut timings)?;
    wait_snapshot(
        &mut worker,
        &mut timings,
        "R permits exactly one fault retry",
        |snapshot| snapshot.epoch == recovered_by_input.epoch && snapshot.fault.is_some(),
    )?;
    ensure(
        counter(&marker) == attempts + 2,
        "R recovery did not retry the failed code exactly once",
    )?;
    worker.shutdown();
    let deadline = Instant::now() + WAIT;
    while worker.is_alive() {
        ensure(
            Instant::now() < deadline,
            "Faulted worker did not shut down",
        )?;
        thread::sleep(Duration::from_millis(1));
    }
    Ok(
        json!({"passed":true,"custom_delay_ms":240,"host_timeout_ms":150,"failure":failed.fault,
        "attempts_before_restart":attempts,"attempts_after_command_restart":attempts+1,"attempts_after_input_restart":counter(&marker),"last_good_frame_retained":true,
        "paused_R_recovered_fault":true,"repaint_and_set_playing_did_not_retry":true,"actor_survived_fault":true,"poll":statistics(&timings.poll)}),
    )
}

/// Compile/run isolated probe game data against the selected Full backend.
/// The source project and its authored scripts/assets are never overwritten.
pub fn run(source: &Path, output: &Path) -> Result<Value, String> {
    let source_hash = module_engine_project::hash_file(source)?;
    let document = crate::project::read(source)?;
    let distribution = crate::distribution::detect(source, &document, None)?;
    ensure(
        distribution.can_author_engine(),
        "Worker verification requires a complete Full source package",
    )?;
    let compile = distribution.compile_root()?;
    let started = Instant::now();
    let performance = lifecycle(&distribution.package_root, &compile, output)?;
    let fault = fault_latch(&distribution.package_root, &compile, output)?;
    ensure(
        module_engine_project::hash_file(source)? == source_hash,
        "Worker verification changed the supplied project",
    )?;
    let report = json!({"passed":true,"lifecycle_and_client_performance":performance,"fault_latch":fault,
        "elapsed_ms":millis(started),"source_project_unchanged":true,
        "measurement_scope":"Actual GameWorker/GameHost, real 80 ms Custom, Vulkan frames 160x90. Timings measure dispatch/poll client calls while the owner thread is busy. Host construction, Rust/DXC compilation, eframe drawing, display latency and vsync are outside client-call timings; this is not an FPS measurement.",
        "fixture":"Separate folders with player/obstacles from the demo, no enemies/audio/imported meshes; disposable scripts only write their own attempt marker. Interactive game content is unchanged."});
    fs::create_dir_all(output).map_err(|error| error.to_string())?;
    fs::write(
        output.join("game-worker-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn backend() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
    }
    #[test]
    fn worker_slow_custom_client_stays_nonblocking_and_preserves_edges_lifecycle() {
        let backend = backend();
        let report = lifecycle(&backend, &backend, &backend.join(".work/worker-tests")).unwrap();
        eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
    }
    #[test]
    fn worker_fault_latches_without_retry_until_explicit_restart() {
        let backend = backend();
        let report = fault_latch(&backend, &backend, &backend.join(".work/worker-tests")).unwrap();
        eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
    }
}
