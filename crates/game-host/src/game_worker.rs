//! Persistent owner of the interactive Game host. No simulation, GPU work or
//! script-state lock is performed by the UI-facing methods in this module.
use crate::activity::ActivitySnapshot;
use crate::game_host::{
    GameHost, PendingPresentationIdentity, PresentationProvenance, RenderedPresentation,
};
use crate::game_wait::{DeadlineWait, WakeSignal};
use module_engine_core::RendererPerformance;
use module_engine_core::{Camera, Frame, GameInput, GameState, GraphDocument, WorldKind};
use module_engine_script::CompiledScript;
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const COMMAND_CAPACITY: usize = 32;
const EDGE_CAPACITY: usize = 128;
const REPLY_CAPACITY: usize = 64;
const FIXED_DT: f64 = 1. / 60.;
const PRESENTATION_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 120);
const STOPPED_PRESENTATION_INTERVAL: Duration = Duration::from_millis(250);
const GPU_COMPLETION_POLL_INTERVAL: Duration = Duration::from_millis(1);

fn tick_wait(pending_seconds: f64, elapsed_seconds: f64) -> Duration {
    Duration::from_secs_f64((FIXED_DT - pending_seconds - elapsed_seconds).clamp(0., FIXED_DT))
}

fn presentation_alpha(pending_seconds: f64, elapsed_seconds: f64, playing: bool) -> f64 {
    let pending = pending_seconds + elapsed_seconds;
    if !playing || pending >= FIXED_DT {
        // A slow tick must not extrapolate or stretch presentation through a
        // backlog. Show the latest committed state until the owner catches up.
        1.
    } else {
        (pending / FIXED_DT).clamp(0., 1.)
    }
}

fn next_presentation_deadline(
    previous: Option<Instant>,
    completed: Instant,
    interval: Duration,
) -> Instant {
    match previous {
        Some(deadline) if deadline > completed => deadline,
        Some(deadline) => {
            // Keep the absolute grid, skipping missed slots in one operation.
            // There is no queue of renders to replay after a slow operation.
            let remainder = completed.duration_since(deadline).as_nanos() % interval.as_nanos();
            completed + interval - Duration::from_nanos(remainder as u64)
        }
        None => completed + interval,
    }
}

#[derive(Clone, Copy)]
struct StepBlockedPresentation {
    deadline: Instant,
    epoch: u64,
    tick: u64,
}

#[derive(Clone, Copy)]
struct StepBlockedSubmission {
    identity: PendingPresentationIdentity,
    deadline: Instant,
    epoch: u64,
    advanced_tick: u64,
}

/// A new presentation grid can coincide with the fixed simulation deadline.
/// Correct that observed collision once; ordinary ticks keep the fixed grid.
#[derive(Default)]
struct PresentationPhase {
    armed: Option<(u64, u64)>,
    blocked: Option<StepBlockedPresentation>,
    pending: Option<StepBlockedSubmission>,
}
impl PresentationPhase {
    fn reset(&mut self, epoch: u64, tick: u64, playing: bool) {
        self.armed = playing.then_some((epoch, tick));
        self.blocked = None;
        self.pending = None;
    }

    fn observe_pending_step(
        &mut self,
        epoch: u64,
        ticks: (u64, u64),
        succeeded: bool,
        pending: (
            Option<PendingPresentationIdentity>,
            Option<PendingPresentationIdentity>,
        ),
        deadline: Option<Instant>,
        ended: Instant,
    ) {
        // Any second attempt invalidates the previous observation. Only a
        // submission from the immediately preceding committed tick qualifies.
        self.pending = None;
        let Some((armed_epoch, armed_tick)) = self.armed else {
            return;
        };
        let (Some(before), Some(after), Some(deadline)) = (pending.0, pending.1, deadline) else {
            return;
        };
        if succeeded
            && epoch == armed_epoch
            && ticks.0 >= armed_tick
            && ticks.0.checked_add(1) == Some(ticks.1)
            && before == after
            && before.current_tick == ticks.0
            && deadline <= ended
        {
            self.pending = Some(StepBlockedSubmission {
                identity: before,
                deadline,
                epoch,
                advanced_tick: ticks.1,
            });
        }
    }

    fn after_pending_publication(
        &mut self,
        identity: PendingPresentationIdentity,
        epoch: u64,
        live_tick: u64,
        deadline: Option<Instant>,
        completed: Instant,
        phase_seconds: f64,
    ) -> Option<Instant> {
        let blocked = self.pending.take()?;
        let (armed_epoch, _) = self.armed?;
        if identity != blocked.identity
            || epoch != armed_epoch
            || epoch != blocked.epoch
            || live_tick != blocked.advanced_tick
            || deadline != Some(blocked.deadline)
            || !phase_seconds.is_finite()
            || phase_seconds < 0.
            || phase_seconds >= PRESENTATION_INTERVAL.as_secs_f64()
        {
            return None;
        }
        self.armed = None;
        self.blocked = None;
        Some(completed + PRESENTATION_INTERVAL)
    }

    fn observe_step(
        &mut self,
        epoch: u64,
        ticks: (u64, u64),
        succeeded: bool,
        deadline: Option<Instant>,
        ended: Instant,
    ) {
        self.blocked = if self.armed.is_some() && succeeded && ticks.1 > ticks.0 {
            deadline
                .filter(|deadline| *deadline <= ended)
                .map(|deadline| StepBlockedPresentation {
                    deadline,
                    epoch,
                    tick: ticks.1,
                })
        } else {
            None
        };
    }

    fn after_publication(
        &mut self,
        epoch: u64,
        tick: u64,
        deadline: Option<Instant>,
        completed: Instant,
        phase_seconds: f64,
    ) -> Option<Instant> {
        let (armed_epoch, armed_tick) = self.armed?;
        let blocked = self.blocked?;
        let deadline = deadline?;
        if epoch != armed_epoch
            || blocked.epoch != epoch
            || tick <= armed_tick
            || blocked.tick != tick
            || blocked.deadline != deadline
            || !phase_seconds.is_finite()
            || phase_seconds < 0.
            || phase_seconds >= PRESENTATION_INTERVAL.as_secs_f64()
        {
            return None;
        }
        self.armed = None;
        self.blocked = None;
        self.pending = None;
        Some(completed + PRESENTATION_INTERVAL)
    }
}

fn owner_wait(
    simulation: Option<Duration>,
    presentation: Option<Duration>,
    preparing: bool,
) -> Duration {
    let idle = if preparing {
        Duration::from_millis(10)
    } else {
        Duration::from_millis(100)
    };
    simulation
        .into_iter()
        .chain(presentation)
        .fold(idle, Duration::min)
}

#[derive(Clone, Debug)]
pub enum CustomApplyMode {
    Compatible,
    Reset,
    Migrate(Value),
}

#[derive(Debug)]
pub enum WorkerCommand {
    SetPlaying(bool),
    SetViewport {
        width: usize,
        height: usize,
    },
    Restart {
        request: u64,
    },
    UpdateDocument {
        document: Arc<GraphDocument>,
        authoring: bool,
        request: u64,
    },
    ApplyCustom {
        id: String,
        candidate: Box<CompiledScript>,
        mode: CustomApplyMode,
        request: u64,
        epoch: u64,
    },
    ApplyRejected {
        id: String,
        source: String,
        mode: CustomApplyMode,
        request: u64,
        epoch: u64,
    },
    InvalidateContent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerReplyKind {
    Restart,
    Document,
    Custom(String),
}
#[derive(Clone, Debug)]
pub struct WorkerReply {
    pub request: u64,
    pub kind: WorkerReplyKind,
    pub result: Result<(), String>,
}
#[derive(Clone, Debug)]
pub struct CustomSnapshot {
    /// Opaque identity of the retained slot. Only the owner dereferences it.
    pub identity: u64,
    pub instance: String,
    pub source_id: String,
    pub source: String,
    pub state: Value,
    pub error: Option<String>,
}
#[derive(Clone, Debug)]
pub struct RejectedCustomSnapshot {
    pub source: String,
    pub error: String,
    pub request: u64,
}

/// Provenance belongs to the completed frame, including while a newer actor
/// epoch/state is waiting for pixels or a renderer has rejected its candidate.
#[derive(Clone, Debug, serde::Serialize)]
pub struct FramePresentation {
    pub actor_id: u64,
    pub epoch: u64,
    pub serial: u64,
    pub source_revision: u64,
    pub camera: Camera,
    pub provenance: PresentationProvenance,
}

/// All values come from completed host operations. Retained last-good pixels
/// carry their own tick/revision so a failed render cannot pretend to be fresh.
#[derive(Clone)]
pub struct GameSnapshot {
    pub actor_id: u64,
    pub sequence: u64,
    pub epoch: u64,
    pub state: Arc<GameState>,
    /// Camera of the retained completed pixels, or the initial host camera
    /// before the first frame. Input aiming must use what was actually drawn.
    pub camera: Camera,
    pub frame: Option<Arc<Frame>>,
    pub frame_tick: Option<u64>,
    pub frame_revision: Option<u64>,
    pub frame_completed_at: Option<Instant>,
    pub frame_presentation: Option<FramePresentation>,
    pub deadline_wait_strategy: &'static str,
    pub deadline_wait_error: Option<String>,
    pub presentation_revision: u64,
    pub presentation_ids: Vec<String>,
    pub game_diagnostics: Value,
    pub device: String,
    pub world_kind: WorldKind,
    pub renderer_performance: RendererPerformance,
    pub last_step_ms: f64,
    pub last_render_ms: f64,
    pub backlog_seconds: f64,
    pub playing: bool,
    pub fault: Option<String>,
    pub preparation_error: Option<String>,
    pub render_error: Option<String>,
    pub custom: Vec<CustomSnapshot>,
    pub rejected: BTreeMap<String, RejectedCustomSnapshot>,
    pub results: Vec<WorkerReply>,
    pub accepted_document: Arc<GraphDocument>,
    pub input_ack: u64,
    pub cancelled_input_ack: u64,
    pub activity: Vec<ActivitySnapshot>,
}

struct InputEdge {
    sequence: u64,
    input: GameInput,
}
#[derive(Default)]
struct Mailbox {
    commands: VecDeque<WorkerCommand>,
    edges: VecDeque<InputEdge>,
    held: GameInput,
    next_edge: u64,
    cancelled_through: u64,
}
struct Shared {
    mailbox: Mutex<Mailbox>,
    presentation: Mutex<Option<Arc<GameSnapshot>>>,
    shutdown: AtomicBool,
    retired: Arc<AtomicBool>,
    alive: AtomicBool,
    wake: Arc<WakeSignal>,
}

struct RegisteredWorker {
    actor_id: u64,
    shared: Weak<Shared>,
    thread: thread::Thread,
    owner: thread::JoinHandle<()>,
}
fn worker_registry() -> &'static Mutex<Vec<RegisteredWorker>> {
    static REGISTRY: std::sync::OnceLock<Mutex<Vec<RegisteredWorker>>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(vec![]))
}

/// Call after run_native has returned, outside the UI event loop. UI handles
/// still retire without joining, but process exit must let the actual owners
/// drop their hosts and kill persistent Custom child processes first.
pub fn finish_after_ui() -> Result<(), String> {
    let workers = {
        let mut registry = worker_registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *registry)
    };
    let owners = finish_registered(workers);
    let background = module_engine_script::finish_background_jobs();
    let artifacts = module_engine_script::finish_artifact_cleanup(Duration::from_secs(10));
    owners.and(background).and(artifacts)
}
fn finish_registered(workers: Vec<RegisteredWorker>) -> Result<(), String> {
    for worker in &workers {
        if let Some(shared) = worker.shared.upgrade() {
            shared.retired.store(true, Ordering::Release);
            shared.shutdown.store(true, Ordering::Release);
        }
    }
    for worker in &workers {
        if let Some(shared) = worker.shared.upgrade() {
            shared.wake.notify(&worker.thread);
        }
    }
    let mut panicked = vec![];
    for worker in workers {
        if worker.owner.join().is_err() {
            panicked.push(worker.actor_id);
        }
    }
    if panicked.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Game owner threads panicked during shutdown: {panicked:?}"
        ))
    }
}

/// Dropping a handle only signals shutdown. The host, queued candidates, native
/// resources and script workers are destroyed by their owner thread.
pub struct GameWorker {
    shared: Arc<Shared>,
    thread: thread::Thread,
    cached: Option<Arc<GameSnapshot>>,
}

impl GameWorker {
    pub fn spawn(host: impl Into<GameHost>, repaint: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        Self::spawn_with_presentation_target(host, None, repaint)
    }

    /// Endpoint/device setup belongs to the actor owner, never the calling UI.
    /// Even a failed setup cannot accidentally publish CPU-rendered GPU frames.
    pub fn spawn_with_presentation_target(
        host: impl Into<GameHost>,
        target: Option<Arc<dyn module_engine_core::PresentationTarget>>,
        repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let host = host.into();
        static NEXT_ACTOR: AtomicU64 = AtomicU64::new(1);
        let actor_id = NEXT_ACTOR.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            presentation: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            retired: host.retirement_signal(),
            alive: AtomicBool::new(true),
            wake: Arc::new(WakeSignal::new()),
        });
        let worker_shared = shared.clone();
        let owner = thread::spawn(move || {
            struct Alive {
                shared: Arc<Shared>,
            }
            impl Drop for Alive {
                fn drop(&mut self) {
                    self.shared.alive.store(false, Ordering::Release);
                    // Fields, including the last published GPU frame, may still
                    // be dropping after this flag. Only JoinHandle completion
                    // proves that all owner-thread cleanup has finished.
                }
            }
            let _alive = Alive {
                shared: worker_shared.clone(),
            };
            let mut actor = Actor::new(host, actor_id, worker_shared, repaint);
            actor.presentation_target = target;
            if let Some(target) = &actor.presentation_target
                && let Err(error) = actor.host.set_presentation_target(Some(target.clone()))
            {
                actor.render_error = Some(error);
            }
            // Initial device/pipeline preparation is loading time, not elapsed
            // gameplay. It runs on this owner before the actor starts its clock.
            actor.clock = Instant::now();
            actor.run();
            actor.host.services.stop();
            actor.host.cancel_preparation();
            if let Ok(mut mailbox) = actor.shared.mailbox.lock() {
                mailbox.commands.clear();
                mailbox.edges.clear();
            }
        });
        let thread = owner.thread().clone();
        {
            let mut registry = worker_registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // is_finished includes closure/field destruction. Keep every other
            // handle, even if its Alive guard has already cleared the flag.
            // Weak shared ownership leaves final snapshots on their real owner.
            registry.retain(|worker| !worker.owner.is_finished());
            registry.push(RegisteredWorker {
                actor_id,
                shared: Arc::downgrade(&shared),
                thread: thread.clone(),
                owner,
            });
        }
        Self {
            shared,
            thread,
            cached: None,
        }
    }

    /// Never waits for a tick, GPU readback or script-state mutex. A contended
    /// pointer swap simply returns this handle's previous completed snapshot.
    pub fn latest(&mut self) -> Option<Arc<GameSnapshot>> {
        if let Ok(presentation) = self.shared.presentation.try_lock() {
            self.cached = presentation.clone();
        }
        self.cached.clone()
    }

    /// Latest movement/aim replaces older held input. Each accepted pressed
    /// packet has a distinct sequence and remains queued until a committed tick.
    /// Full/contended returns the original packet for the caller to retain.
    pub fn submit_input(&self, input: GameInput) -> Result<Option<u64>, GameInput> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(input);
        }
        let Ok(mut mailbox) = self.shared.mailbox.try_lock() else {
            return Err(input);
        };
        let pressed = !input.actions.is_empty() || input.restart;
        if pressed && !input.restart && mailbox.edges.len() >= EDGE_CAPACITY {
            return Err(input);
        }
        if input.restart {
            // Restart is an explicit cancellation boundary and must remain
            // reachable even when a fault has stopped draining a full queue.
            if let Some(last) = mailbox.edges.back() {
                mailbox.cancelled_through = mailbox.cancelled_through.max(last.sequence);
            }
            mailbox.edges.clear();
        }
        mailbox.held = GameInput {
            actions: Default::default(),
            action_values: Default::default(),
            restart: false,
            ..input.clone()
        };
        let sequence = if pressed {
            mailbox.next_edge += 1;
            let sequence = mailbox.next_edge;
            mailbox.edges.push_back(InputEdge { sequence, input });
            Some(sequence)
        } else {
            None
        };
        drop(mailbox);
        self.shared.wake.notify(&self.thread);
        Ok(sequence)
    }

    pub fn try_command(&self, command: WorkerCommand) -> Result<(), WorkerCommand> {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(command);
        }
        let Ok(mut mailbox) = self.shared.mailbox.try_lock() else {
            return Err(command);
        };
        // Size is a latest-value setting, not a repeated action. Keep its
        // position at the tail so a restart before it stays ordered.
        if matches!(command, WorkerCommand::SetViewport { .. }) {
            mailbox
                .commands
                .retain(|queued| !matches!(queued, WorkerCommand::SetViewport { .. }));
        }
        if mailbox.commands.len() >= COMMAND_CAPACITY {
            return Err(command);
        }
        mailbox.commands.push_back(command);
        drop(mailbox);
        self.shared.wake.notify(&self.thread);
        Ok(())
    }

    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::Acquire)
    }
    pub fn shutdown(&self) {
        self.shared.retired.store(true, Ordering::Release);
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.wake.notify(&self.thread);
    }
}
impl Drop for GameWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct RejectedCustom {
    candidate: CompiledScript,
    error: String,
    request: u64,
}
struct DocumentRequest {
    document: Arc<GraphDocument>,
    authoring: bool,
    request: u64,
}
struct Actor {
    host: GameHost,
    presentation_target: Option<Arc<dyn module_engine_core::PresentationTarget>>,
    actor_id: u64,
    shared: Arc<Shared>,
    repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    sequence: u64,
    epoch: u64,
    playing: bool,
    size: Option<[usize; 2]>,
    frame: Option<Arc<Frame>>,
    frame_tick: Option<u64>,
    frame_revision: Option<u64>,
    frame_completed_at: Option<Instant>,
    frame_presentation: Option<FramePresentation>,
    attempted_revision: Option<u64>,
    rendered_camera: Option<Camera>,
    rendered_size: Option<[usize; 2]>,
    next_render: Option<Instant>,
    pending_render_deadline: Option<Option<Instant>>,
    presentation_phase: PresentationPhase,
    deadline_wait: DeadlineWait,
    published_wait: Option<(&'static str, Option<String>)>,
    clock: Instant,
    fault: Option<String>,
    preparation_error: Option<String>,
    render_error: Option<String>,
    viewport_error: Option<String>,
    custom_errors: BTreeMap<String, String>,
    rejected: BTreeMap<String, RejectedCustom>,
    replies: VecDeque<WorkerReply>,
    completed: BTreeMap<(u64, String), Result<(), String>>,
    highest_completed: BTreeMap<String, u64>,
    document: Option<DocumentRequest>,
    input_ack: u64,
    cancelled_input_ack: u64,
}

impl Actor {
    fn new(
        host: GameHost,
        actor_id: u64,
        shared: Arc<Shared>,
        repaint: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let deadline_wait = DeadlineWait::new(shared.wake.clone());
        Self {
            host,
            presentation_target: None,
            actor_id,
            shared,
            repaint,
            sequence: 0,
            epoch: 0,
            playing: false,
            size: None,
            frame: None,
            frame_tick: None,
            frame_revision: None,
            frame_completed_at: None,
            frame_presentation: None,
            attempted_revision: None,
            rendered_camera: None,
            rendered_size: None,
            next_render: None,
            pending_render_deadline: None,
            presentation_phase: PresentationPhase::default(),
            deadline_wait,
            published_wait: None,
            clock: Instant::now(),
            fault: None,
            preparation_error: None,
            render_error: None,
            viewport_error: None,
            custom_errors: BTreeMap::new(),
            rejected: BTreeMap::new(),
            replies: VecDeque::new(),
            completed: BTreeMap::new(),
            highest_completed: BTreeMap::new(),
            document: None,
            input_ack: 0,
            cancelled_input_ack: 0,
        }
    }

    fn run(&mut self) {
        self.publish();
        while !self.shared.shutdown.load(Ordering::Acquire) {
            self.publish_wait_transition();
            let shared = self.shared.clone();
            let commands = match shared.mailbox.lock() {
                Ok(mut mailbox) => mailbox.commands.drain(..).collect::<Vec<_>>(),
                Err(_) => {
                    self.fault = Some("Game mailbox poisoned".into());
                    self.publish();
                    return;
                }
            };
            let mut changed = !commands.is_empty();
            for command in commands {
                if self.shared.shutdown.load(Ordering::Acquire) {
                    return;
                }
                self.command(command);
            }
            changed |= self.update_document();
            // R is a host command, including while paused or fault-latched.
            // Ordinary gameplay edges still wait for an actual playing tick.
            changed |= self.process_restart_edge();
            if self.shared.shutdown.load(Ordering::Acquire) {
                return;
            }
            // Observe the submitted pixels before another simulation tick can
            // advance the live inputs. Polling never advances the submit grid.
            if self.poll_rendered() {
                // A ready frame must reach the UI before a potentially slow
                // next tick. Its completion timestamp and source metadata stay
                // those accepted by poll_rendered, independent of live state.
                self.publish();
                // This snapshot also includes preceding command changes. Only
                // subsequent state/render changes require another publication.
                changed = false;
            }
            let mut step_attempted = false;
            if self.playing && self.fault.is_none() {
                let now = Instant::now();
                let elapsed = now.duration_since(self.clock).as_secs_f64();
                if elapsed + self.host.session.pending_time() + 1e-12 >= FIXED_DT {
                    step_attempted = true;
                    self.clock = now;
                    let (mut input, edge) = {
                        let mailbox = self.shared.mailbox.lock().expect("Game mailbox poisoned");
                        let edge = mailbox
                            .edges
                            .front()
                            .map(|edge| (edge.sequence, edge.input.clone()));
                        (mailbox.held.clone(), edge)
                    };
                    if let Some((_, pressed)) = &edge {
                        input.actions.clone_from(&pressed.actions);
                        input.action_values.clone_from(&pressed.action_values);
                        input.restart = pressed.restart;
                    }
                    let before_tick = self.host.session.state().tick;
                    let pending_before_step = self.host.pending_interpolated_identity();
                    let result = if input.restart {
                        // A newly submitted R can arrive after the priority
                        // check above; it still uses the actor epoch boundary.
                        self.restart(edge.as_ref().map(|(sequence, _)| *sequence))
                            .map(|_| 0)
                    } else {
                        self.host.step_budget(elapsed, &input, 1)
                    };
                    let step_ended = Instant::now();
                    let after_tick = self.host.session.state().tick;
                    self.observe_completed_step(
                        (before_tick, after_tick),
                        result.is_ok(),
                        pending_before_step,
                        step_ended,
                    );
                    let committed = after_tick > before_tick;
                    if (committed || (input.restart && result.is_ok()))
                        && let Some((sequence, _)) = &edge
                    {
                        let mut mailbox =
                            self.shared.mailbox.lock().expect("Game mailbox poisoned");
                        if mailbox
                            .edges
                            .front()
                            .is_some_and(|edge| edge.sequence == *sequence)
                        {
                            mailbox.edges.pop_front();
                        }
                        self.input_ack = self.input_ack.max(*sequence);
                    }
                    match result {
                        Ok(_) => {}
                        Err(error) => {
                            self.fault = Some(error);
                        }
                    }
                    changed = true;
                }
            } else {
                // Pause/fault time never turns into a resume catch-up burst.
                self.clock = Instant::now();
            }
            if self.shared.shutdown.load(Ordering::Acquire) {
                return;
            }
            if step_attempted && self.poll_rendered() {
                // A submission can finish while the simulation owner is busy.
                // Reap it now instead of adding another pending-poll wait, and
                // publish before admitting a possible successor render.
                self.publish();
                changed = false;
            }
            changed |= self.render_if_due();
            if changed {
                self.publish();
            }
            let simulation = (self.playing && self.fault.is_none()).then(|| {
                tick_wait(
                    self.host.session.pending_time(),
                    self.clock.elapsed().as_secs_f64(),
                )
            });
            let presentation = self.presentation_wait();
            let wait = owner_wait(
                simulation,
                presentation,
                self.host.has_pending_preparation(),
            );
            if wait.is_zero() {
                // A due tick still yields between bounded owner turns, without
                // adding a millisecond after its deadline. The next turn checks
                // shutdown, commands and restart before processing another tick.
                thread::yield_now();
            } else {
                self.wait_for_work(wait);
            }
        }
    }

    fn wait_for_work(&mut self, duration: Duration) {
        self.deadline_wait.wait(duration);
        self.publish_wait_transition();
    }

    fn publish_wait_transition(&mut self) {
        let state = (self.deadline_wait.strategy(), self.deadline_wait.error());
        if self.published_wait.as_ref() != Some(&state) {
            self.publish();
        }
    }

    fn command(&mut self, command: WorkerCommand) {
        match command {
            WorkerCommand::SetPlaying(playing) => {
                if self.playing != playing {
                    self.playing = playing;
                    self.clock = Instant::now();
                    self.host.reset_presentation_history();
                    self.reset_presentation_grid();
                }
                if !playing && let Ok(mut mailbox) = self.shared.mailbox.lock() {
                    // Resume must not reuse movement held before a modal
                    // workflow/pause. Accepted pressed edges stay queued.
                    mailbox.held = GameInput::default();
                }
            }
            WorkerCommand::SetViewport { width, height } => {
                // Zero disables rendering for a hidden Game view. Validate before
                // touching the GPU; the renderer has the same allocation bound.
                let size = if width == 0 || height == 0 {
                    None
                } else if width
                    .checked_mul(height)
                    .is_some_and(|pixels| pixels <= 16_777_216)
                {
                    Some([width, height])
                } else {
                    self.viewport_error = Some("Game viewport exceeds the pixel limit".into());
                    return;
                };
                // A valid repeated size repairs only the rejected input. It
                // neither invents a new frame nor clears a GPU draw failure.
                self.viewport_error = None;
                if self.size != size {
                    self.size = size;
                    self.host.reset_presentation_history();
                    self.reset_presentation_grid();
                }
            }
            WorkerCommand::Restart { request } => {
                if self.seen(request, "restart") {
                    return;
                }
                let result = self.restart(None);
                self.reply(request, WorkerReplyKind::Restart, result);
            }
            WorkerCommand::UpdateDocument {
                document,
                authoring,
                request,
            } => {
                if self
                    .document
                    .as_ref()
                    .is_some_and(|pending| request == pending.request)
                {
                    return;
                }
                if self
                    .document
                    .as_ref()
                    .is_some_and(|pending| request < pending.request)
                {
                    self.reply(
                        request,
                        WorkerReplyKind::Document,
                        Err(
                            "Obsolete Game document request; a newer request is already pending"
                                .into(),
                        ),
                    );
                    return;
                }
                if self.seen(request, "document") {
                    return;
                }
                if let Some(previous) = self.document.take() {
                    self.reply(
                        previous.request,
                        WorkerReplyKind::Document,
                        Err("Replaced by a newer Game document request".into()),
                    );
                }
                self.document = Some(DocumentRequest {
                    document,
                    authoring,
                    request,
                });
            }
            WorkerCommand::ApplyCustom {
                id,
                candidate,
                mode,
                request,
                epoch,
            } => {
                self.apply_custom(id, *candidate, mode, request, epoch);
            }
            WorkerCommand::ApplyRejected {
                id,
                source,
                mode,
                request,
                epoch,
            } => {
                if self.seen(request, &format!("custom:{id}")) {
                    return;
                }
                let candidate = self
                    .rejected
                    .get(&id)
                    .filter(|rejected| rejected.candidate.source() == source)
                    .map(|rejected| rejected.candidate.clone());
                match candidate {
                    Some(candidate) => self.apply_custom(id, candidate, mode, request, epoch),
                    None => self.reply(
                        request,
                        WorkerReplyKind::Custom(id),
                        Err(
                            "Rejected Custom candidate is absent or belongs to an older source"
                                .into(),
                        ),
                    ),
                }
            }
            WorkerCommand::InvalidateContent => {
                self.host.invalidate_content();
                self.reset_presentation_grid();
            }
        }
    }

    fn process_restart_edge(&mut self) -> bool {
        let restart = self.shared.mailbox.lock().ok().and_then(|mailbox| {
            mailbox
                .edges
                .iter()
                .find(|edge| edge.input.restart)
                .map(|edge| edge.sequence)
        });
        let Some(sequence) = restart else {
            return false;
        };
        match self.restart(Some(sequence)) {
            Ok(()) => self.input_ack = self.input_ack.max(sequence),
            Err(error) => {
                self.fault = Some(error);
                // A failed explicit restart is reported once, not retried on
                // every owner turn. Another R is a new explicit attempt.
                if let Ok(mut mailbox) = self.shared.mailbox.lock() {
                    mailbox.edges.retain(|edge| edge.sequence != sequence);
                }
                self.cancelled_input_ack = self.cancelled_input_ack.max(sequence);
            }
        }
        true
    }

    fn restart(&mut self, through: Option<u64>) -> Result<(), String> {
        let through = through.unwrap_or_else(|| {
            self.shared
                .mailbox
                .lock()
                .ok()
                .and_then(|mailbox| mailbox.edges.back().map(|edge| edge.sequence))
                .unwrap_or(0)
        });
        let result = self.host.restart();
        if let Err(error) = &result {
            self.fault = Some(error.clone());
            return result;
        }
        self.epoch += 1;
        self.presentation_phase
            .reset(self.epoch, self.host.session.state().tick, false);
        self.fault = None;
        self.clock = Instant::now();
        self.next_render = None;
        let mut mailbox = self
            .shared
            .mailbox
            .lock()
            .map_err(|_| "Game mailbox poisoned")?;
        self.cancelled_input_ack = self
            .cancelled_input_ack
            .max(through)
            .max(mailbox.cancelled_through);
        mailbox.edges.retain(|edge| edge.sequence > through);
        mailbox.held.actions.clear();
        mailbox.held.action_values.clear();
        mailbox.held.restart = false;
        drop(mailbox);
        self.reset_presentation_grid();
        Ok(())
    }

    fn apply_custom(
        &mut self,
        id: String,
        candidate: CompiledScript,
        mode: CustomApplyMode,
        request: u64,
        epoch: u64,
    ) {
        if self.seen(request, &format!("custom:{id}")) {
            return;
        }
        if epoch != self.epoch {
            self.reply(
                request,
                WorkerReplyKind::Custom(id),
                Err("Custom candidate belongs to a previous Game epoch".into()),
            );
            return;
        }
        let result = match mode {
            CustomApplyMode::Compatible => self.host.apply_candidate(&id, candidate.clone(), false),
            CustomApplyMode::Reset => self.host.apply_candidate(&id, candidate.clone(), true),
            CustomApplyMode::Migrate(state) => {
                self.host.apply_candidate_migrated(&id, &candidate, state)
            }
        };
        match &result {
            Ok(()) => {
                self.rejected.remove(&id);
                self.custom_errors.remove(&id);
                self.fault = None;
                self.clock = Instant::now();
                self.reset_presentation_grid();
            }
            Err(error) => {
                self.custom_errors.insert(id.clone(), error.clone());
                self.rejected.insert(
                    id.clone(),
                    RejectedCustom {
                        candidate,
                        error: error.clone(),
                        request,
                    },
                );
            }
        }
        self.reply(request, WorkerReplyKind::Custom(id), result);
    }

    fn seen(&self, request: u64, kind: &str) -> bool {
        self.highest_completed
            .get(kind)
            .is_some_and(|previous| request <= *previous)
    }
    fn reply(&mut self, request: u64, kind: WorkerReplyKind, result: Result<(), String>) {
        let key = match &kind {
            WorkerReplyKind::Restart => "restart".into(),
            WorkerReplyKind::Document => "document".into(),
            WorkerReplyKind::Custom(id) => format!("custom:{id}"),
        };
        self.completed.insert((request, key), result.clone());
        let key = match &kind {
            WorkerReplyKind::Restart => "restart".into(),
            WorkerReplyKind::Document => "document".into(),
            WorkerReplyKind::Custom(id) => format!("custom:{id}"),
        };
        self.highest_completed
            .entry(key)
            .and_modify(|previous| *previous = (*previous).max(request))
            .or_insert(request);
        while self.completed.len() > REPLY_CAPACITY {
            self.completed.pop_first();
        }
        self.replies
            .retain(|reply| reply.request != request || reply.kind != kind);
        self.replies.push_back(WorkerReply {
            request,
            kind,
            result,
        });
        while self.replies.len() > REPLY_CAPACITY {
            self.replies.pop_front();
        }
    }

    fn update_document(&mut self) -> bool {
        let Some(target) = self.document.as_ref() else {
            return false;
        };
        let document = target.document.clone();
        let authoring = target.authoring;
        let request = target.request;
        let previous_error = self.preparation_error.clone();
        let revision = self.host.presentation_revision();
        let graphics = self.host.graphics_only_change(&document);
        let result = if authoring && !self.playing && !graphics {
            self.host.refresh_author_preview(&document)
        } else {
            self.host.sync_document_deferred(&document)
        };
        match result {
            Ok(()) if self.host.accepted_document().execution_eq(&document) => {
                self.preparation_error = None;
                self.document = None;
                if authoring && let Some(target) = self.host.frame_target() {
                    self.host.camera.target = target;
                }
                self.reply(request, WorkerReplyKind::Document, Ok(()));
                true
            }
            Ok(()) => {
                previous_error != self.preparation_error
                    || revision != self.host.presentation_revision()
            }
            Err(error) => {
                self.preparation_error = Some(error.clone());
                if authoring && !graphics {
                    self.document = None;
                }
                if self.completed.get(&(request, "document".into())) != Some(&Err(error.clone())) {
                    self.reply(request, WorkerReplyKind::Document, Err(error));
                    true
                } else {
                    false
                }
            }
        }
    }

    fn reset_presentation_grid(&mut self) {
        self.next_render = None;
        self.pending_render_deadline = None;
        self.presentation_phase.reset(
            self.epoch,
            self.host.session.state().tick,
            self.playing && self.fault.is_none() && self.size.is_some(),
        );
    }

    fn observe_completed_step(
        &mut self,
        ticks: (u64, u64),
        succeeded: bool,
        pending_before: Option<PendingPresentationIdentity>,
        ended: Instant,
    ) {
        self.presentation_phase
            .observe_step(self.epoch, ticks, succeeded, self.next_render, ended);
        self.presentation_phase.observe_pending_step(
            self.epoch,
            ticks,
            succeeded,
            (pending_before, self.host.pending_interpolated_identity()),
            self.pending_render_deadline.flatten(),
            ended,
        );
    }

    fn presentation_wait(&self) -> Option<Duration> {
        if self.host.has_pending_presentation() {
            Some(GPU_COMPLETION_POLL_INTERVAL)
        } else if self.size.is_some() && self.host.has_renderer() {
            self.next_render
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
        } else {
            None
        }
    }

    fn render_if_due(&mut self) -> bool {
        if !self.host.has_renderer() || self.host.has_pending_presentation() {
            return false;
        }
        let Some([width, height]) = self.size else {
            return false;
        };
        let revision = self.host.presentation_revision();
        let changed = self.attempted_revision != Some(revision)
            || self.rendered_camera != Some(self.host.camera)
            || self.rendered_size != self.size;
        let playing = self.playing && self.fault.is_none();
        let interval = if playing {
            PRESENTATION_INTERVAL
        } else {
            STOPPED_PRESENTATION_INTERVAL
        };
        // A normal tick changes the source pair, not the presentation clock.
        // Static/repeated samples are filtered by the host's evaluated inputs.
        // Stopped authoring invalidations still request an immediate frame.
        if (playing || !changed)
            && self
                .next_render
                .is_some_and(|deadline| Instant::now() < deadline)
        {
            return false;
        }
        self.attempted_revision = Some(revision);
        self.rendered_camera = Some(self.host.camera);
        self.rendered_size = self.size;
        let alpha = presentation_alpha(
            self.host.session.pending_time(),
            self.clock.elapsed().as_secs_f64(),
            playing,
        );
        let render_deadline = self.next_render;
        let result = self
            .presentation_target
            .as_ref()
            .map_or(Ok(()), |target| {
                self.host.set_presentation_target(Some(target.clone()))
            })
            .and_then(|()| self.host.try_render_interpolated(width, height, alpha));
        if self.host.has_pending_presentation() {
            self.pending_render_deadline = Some(render_deadline);
        }
        self.next_render = Some(next_presentation_deadline(
            self.next_render,
            Instant::now(),
            interval,
        ));
        match result {
            Ok(Some(rendered)) => {
                if let Err(error) = self.accept_rendered(rendered) {
                    self.render_error = Some(error);
                } else if self.presentation_phase.armed.is_some() {
                    self.rephase_after_successful_publication(render_deadline, Instant::now());
                }
                true
            }
            Ok(None) => false,
            Err(error) => {
                let changed = self.render_error.as_ref() != Some(&error);
                self.render_error = Some(error);
                changed
            }
        }
    }

    fn poll_rendered(&mut self) -> bool {
        let pending = self.host.pending_interpolated_identity();
        if self
            .presentation_phase
            .pending
            .is_some_and(|blocked| Some(blocked.identity) != pending)
        {
            self.presentation_phase.pending = None;
        }
        match self.host.poll_interpolated_presentation() {
            Ok(Some(rendered)) => {
                let deadline = self.pending_render_deadline.take().flatten();
                if let Err(error) = self.accept_rendered(rendered) {
                    self.presentation_phase.pending = None;
                    self.render_error = Some(error);
                } else if self.presentation_phase.armed.is_some() {
                    self.rephase_after_pending_publication(pending, deadline, Instant::now());
                    self.rephase_after_successful_publication(deadline, Instant::now());
                }
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.presentation_phase.pending = None;
                self.pending_render_deadline = None;
                let changed = self.render_error.as_ref() != Some(&error);
                self.render_error = Some(error);
                changed
            }
        }
    }

    fn rephase_after_pending_publication(
        &mut self,
        identity: Option<PendingPresentationIdentity>,
        deadline: Option<Instant>,
        now: Instant,
    ) {
        let (Some(identity), Some(frame), Some(completed)) =
            (identity, &self.frame_presentation, self.frame_completed_at)
        else {
            return;
        };
        if !self.playing
            || self.fault.is_some()
            || self.frame.as_ref().is_none_or(|frame| !frame.is_valid())
            || frame.epoch != self.epoch
            || frame.source_revision != identity.source_revision
            || frame.provenance.current_tick != identity.current_tick
            || frame.provenance.generation != identity.generation
        {
            self.presentation_phase.pending = None;
            return;
        }
        let phase_seconds = self.host.session.pending_time()
            + now.saturating_duration_since(self.clock).as_secs_f64();
        if let Some(corrected) = self.presentation_phase.after_pending_publication(
            identity,
            self.epoch,
            self.host.session.state().tick,
            deadline,
            completed,
            phase_seconds,
        ) {
            self.next_render = Some(
                self.next_render
                    .map_or(corrected, |existing| existing.max(corrected)),
            );
        }
    }

    fn rephase_after_successful_publication(&mut self, deadline: Option<Instant>, now: Instant) {
        if !self.playing
            || self.fault.is_some()
            || self.frame.as_ref().is_none_or(|frame| !frame.is_valid())
        {
            return;
        }
        let (Some(frame), Some(completed)) = (&self.frame_presentation, self.frame_completed_at)
        else {
            return;
        };
        if frame.epoch != self.epoch {
            return;
        }
        let phase_seconds = self.host.session.pending_time()
            + now.saturating_duration_since(self.clock).as_secs_f64();
        if let Some(deadline) = self.presentation_phase.after_publication(
            frame.epoch,
            frame.provenance.current_tick,
            deadline,
            completed,
            phase_seconds,
        ) {
            self.next_render = Some(deadline);
        }
    }

    fn accept_rendered(&mut self, rendered: RenderedPresentation) -> Result<(), String> {
        let completed = Instant::now();
        let serial = self
            .frame_presentation
            .as_ref()
            .map_or(Some(1), |frame| frame.serial.checked_add(1))
            .ok_or("Game frame presentation serial exhausted")?;
        self.frame_tick = Some(rendered.provenance.current_tick);
        self.frame_revision = Some(rendered.source_revision);
        self.frame_completed_at = Some(completed);
        self.frame_presentation = Some(FramePresentation {
            actor_id: self.actor_id,
            epoch: self.epoch,
            serial,
            source_revision: rendered.source_revision,
            camera: rendered.camera,
            provenance: rendered.provenance,
        });
        self.frame = Some(rendered.frame);
        self.render_error = None;
        Ok(())
    }

    fn publish(&mut self) {
        let wait_state = (self.deadline_wait.strategy(), self.deadline_wait.error());
        let custom = self
            .host
            .script_sources
            .iter()
            .map(|(instance, source_id)| {
                let active = self
                    .host
                    .slots
                    .get(instance)
                    .ok_or_else(|| "Game Custom slot missing".to_owned())
                    .and_then(|slot| {
                        slot.lock()
                            .map_err(|_| "Game Custom slot poisoned".to_owned())
                    });
                match active {
                    Ok(active) => {
                        let state = active.state().map_err(|error| error.to_string());
                        CustomSnapshot {
                            identity: self
                                .host
                                .slots
                                .get(instance)
                                .map_or(0, |slot| Arc::as_ptr(slot) as usize as u64),
                            instance: instance.clone(),
                            source_id: source_id.clone(),
                            source: active.source().into(),
                            state: state.clone().unwrap_or(Value::Null),
                            error: state
                                .err()
                                .or_else(|| self.custom_errors.get(source_id).cloned()),
                        }
                    }
                    Err(error) => CustomSnapshot {
                        identity: self
                            .host
                            .slots
                            .get(instance)
                            .map_or(0, |slot| Arc::as_ptr(slot) as usize as u64),
                        instance: instance.clone(),
                        source_id: source_id.clone(),
                        source: String::new(),
                        state: Value::Null,
                        error: Some(error),
                    },
                }
            })
            .collect();
        self.sequence += 1;
        let snapshot = Arc::new(GameSnapshot {
            actor_id: self.actor_id,
            sequence: self.sequence,
            epoch: self.epoch,
            state: Arc::new(self.host.session.state().clone()),
            camera: self
                .frame_presentation
                .as_ref()
                .map_or(self.host.camera, |frame| frame.camera),
            frame: self.frame.clone(),
            frame_tick: self.frame_tick,
            frame_revision: self.frame_revision,
            frame_completed_at: self.frame_completed_at,
            frame_presentation: self.frame_presentation.clone(),
            deadline_wait_strategy: wait_state.0,
            deadline_wait_error: wait_state.1.clone(),
            presentation_revision: self.host.presentation_revision(),
            presentation_ids: self.host.presentation_ids(),
            game_diagnostics: self.host.game_diagnostics(),
            device: self.host.device.clone(),
            world_kind: self.host.session.world_kind(),
            renderer_performance: self.host.renderer_performance(),
            last_step_ms: self.host.last_step_ms,
            last_render_ms: self.host.last_render_ms,
            backlog_seconds: self.host.session.pending_time(),
            playing: self.playing,
            fault: self.fault.clone(),
            preparation_error: self.preparation_error.clone(),
            render_error: self
                .viewport_error
                .clone()
                .or_else(|| self.render_error.clone()),
            custom,
            rejected: self
                .rejected
                .iter()
                .map(|(id, rejected)| {
                    (
                        id.clone(),
                        RejectedCustomSnapshot {
                            source: rejected.candidate.source().into(),
                            error: rejected.error.clone(),
                            request: rejected.request,
                        },
                    )
                })
                .collect(),
            results: self.replies.iter().cloned().collect(),
            accepted_document: Arc::new(self.host.accepted_document().clone()),
            input_ack: self.input_ack,
            cancelled_input_ack: self.cancelled_input_ack,
            activity: self.host.activities(),
        });
        if let Ok(mut presentation) = self.shared.presentation.lock() {
            *presentation = Some(snapshot);
            self.published_wait = Some(wait_state);
        }
        if let Some(ctx) = &self.repaint {
            ctx();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::neutral_tests::Fixture;

    fn actor(fixture: &Fixture) -> Actor {
        actor_with_host(fixture.host())
    }

    fn actor_with_host(host: GameHost) -> Actor {
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            presentation: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            retired: host.retirement_signal(),
            alive: AtomicBool::new(true),
            wake: Arc::new(WakeSignal::new()),
        });
        let mut actor = Actor::new(host, 17, shared, None);
        actor.command(WorkerCommand::SetViewport {
            width: 160,
            height: 100,
        });
        actor
    }

    #[test]
    fn pending_gpu_completion_publishes_original_inputs_and_keeps_the_next_submit_turn() {
        let (host, draws, _temp) = crate::game_host::presentation_tests::gpu_host();
        let mut actor = actor_with_host(host);
        let camera = actor.host.camera;
        let revision = actor.host.presentation_revision();
        draws.lock().unwrap().delayed = true;
        assert!(!actor.render_if_due());
        assert!(actor.host.has_pending_presentation());
        assert!(actor.frame.is_none());
        actor.host.camera.yaw += 0.5;
        actor.host.step(FIXED_DT, &GameInput::default()).unwrap();
        let committed = actor.host.session.state().clone();
        let due = Instant::now();
        actor.next_render = Some(due);
        assert!(!actor.render_if_due());
        assert!(!actor.poll_rendered());
        assert_eq!(actor.next_render, Some(due));
        assert_eq!(
            actor.presentation_wait(),
            Some(GPU_COMPLETION_POLL_INTERVAL)
        );
        assert_eq!(
            owner_wait(None, actor.presentation_wait(), false),
            Duration::from_millis(1)
        );
        assert_eq!(draws.lock().unwrap().attempts, 1);

        draws.lock().unwrap().ready = true;
        assert!(actor.poll_rendered());
        let metadata = actor.frame_presentation.clone().unwrap();
        assert_eq!(metadata.camera, camera);
        assert_eq!(metadata.source_revision, revision);
        assert_eq!(metadata.provenance.current_tick, 0);
        assert_ne!(metadata.source_revision, actor.host.presentation_revision());
        assert_eq!(actor.frame_revision, Some(revision));
        assert_eq!(
            actor.next_render,
            Some(due),
            "polling must preserve the submit grid"
        );
        assert!(!actor.render_if_due());
        assert!(actor.host.has_pending_presentation());
        assert_eq!(
            draws.lock().unwrap().attempts,
            2,
            "a completion must not consume the next eligible submission turn"
        );
        assert_eq!(actor.host.session.state(), &committed);
        assert!(actor.poll_rendered());
        assert_eq!(
            actor.frame_presentation.as_ref().unwrap().serial,
            metadata.serial + 1
        );
        assert_eq!(actor.frame_tick, Some(1));
    }

    #[test]
    fn restarting_with_pending_gpu_work_never_assigns_old_pixels_to_the_new_epoch() {
        let (host, draws, _temp) = crate::game_host::presentation_tests::gpu_host();
        let mut actor = actor_with_host(host);
        assert!(actor.render_if_due());
        let previous = actor.frame.clone().unwrap();
        let completed = actor.frame_completed_at;
        let metadata = serde_json::to_value(&actor.frame_presentation).unwrap();
        draws.lock().unwrap().delayed = true;
        actor.host.camera.yaw += 0.5;
        assert!(!actor.render_if_due());
        assert!(actor.host.has_pending_presentation());
        actor.command(WorkerCommand::Restart { request: 1 });
        assert_eq!(actor.epoch, 1);
        assert_eq!(draws.lock().unwrap().discarded, vec![2]);
        draws.lock().unwrap().ready = true;
        assert!(!actor.poll_rendered());
        assert!(Arc::ptr_eq(actor.frame.as_ref().unwrap(), &previous));
        assert_eq!(actor.frame_completed_at, completed);
        assert_eq!(
            serde_json::to_value(&actor.frame_presentation).unwrap(),
            metadata
        );
        assert!(!actor.render_if_due());
        assert!(actor.host.has_pending_presentation());
        assert!(actor.poll_rendered());
        assert_eq!(actor.frame_presentation.as_ref().unwrap().epoch, 1);
        assert_eq!(actor.frame_presentation.as_ref().unwrap().serial, 2);
        assert!(actor.frame_completed_at > completed);
    }

    #[test]
    fn pending_submission_crossing_one_real_step_can_correct_phase_without_relabeling() {
        for preserve_later_deadline in [false, true] {
            let (host, draws, _temp) = crate::game_host::presentation_tests::gpu_host();
            let mut actor = actor_with_host(host);
            assert!(actor.render_if_due());
            actor.command(WorkerCommand::SetPlaying(true));
            actor.host.camera.yaw += 0.5;
            draws.lock().unwrap().delayed = true;
            let due = Instant::now() - Duration::from_millis(1);
            actor.next_render = Some(due);
            assert!(!actor.render_if_due());
            let identity = actor.host.pending_interpolated_identity().unwrap();
            let frozen_deadline = actor.pending_render_deadline.unwrap();
            assert_eq!(frozen_deadline, Some(due));
            assert!(!actor.poll_rendered());
            actor
                .host
                .step_budget(FIXED_DT, &GameInput::default(), 1)
                .unwrap();
            actor.observe_completed_step((0, 1), true, Some(identity), Instant::now());
            assert_eq!(actor.presentation_phase.pending.unwrap().identity, identity);
            draws.lock().unwrap().ready = true;
            let rendered = actor
                .host
                .poll_interpolated_presentation()
                .unwrap()
                .unwrap();
            assert_eq!(rendered.provenance.current_tick, 0);
            actor.accept_rendered(rendered).unwrap();
            let frame = actor.frame.clone().unwrap();
            let metadata = serde_json::to_value(&actor.frame_presentation).unwrap();
            let state = actor.host.session.state().clone();
            let completed = actor.frame_completed_at.unwrap();
            // Deterministic headroom, independent of how long the fixture's
            // actual step took on the test machine. Production supplies now.
            actor.clock = completed;
            let existing = if preserve_later_deadline {
                completed + PRESENTATION_INTERVAL * 2
            } else {
                completed
            };
            actor.next_render = Some(existing);
            actor.rephase_after_successful_publication(
                frozen_deadline,
                completed + Duration::from_millis(1),
            );
            assert_eq!(
                actor.next_render,
                Some(existing),
                "The ordinary current-tick rule cannot match this old-tick frame"
            );
            actor.rephase_after_pending_publication(
                Some(identity),
                frozen_deadline,
                completed + Duration::from_millis(1),
            );
            let corrected = existing.max(completed + PRESENTATION_INTERVAL);
            assert_eq!(actor.next_render, Some(corrected));
            assert!(actor.presentation_phase.armed.is_none());
            assert!(actor.presentation_phase.pending.is_none());
            actor.rephase_after_pending_publication(
                Some(identity),
                frozen_deadline,
                completed + Duration::from_millis(2),
            );
            assert_eq!(actor.next_render, Some(corrected), "Correction is one-shot");
            assert!(Arc::ptr_eq(&frame, actor.frame.as_ref().unwrap()));
            assert_eq!(
                serde_json::to_value(&actor.frame_presentation).unwrap(),
                metadata
            );
            assert_eq!(actor.frame_completed_at, Some(completed));
            assert_eq!(actor.host.session.state(), &state);
            assert_eq!(draws.lock().unwrap().attempts, 2);
        }
    }

    #[test]
    fn pending_phase_rejects_foreign_failed_repeated_reset_and_headroom_observations() {
        let due = Instant::now();
        let key = PendingPresentationIdentity {
            ticket: 2,
            generation: 3,
            source_revision: 4,
            current_tick: 8,
        };
        for case in 0..16 {
            let mut phase = PresentationPhase::default();
            phase.reset(5, 8, true);
            let mut before = key;
            let mut after = key;
            let mut ticks = (8, 9);
            let mut succeeded = true;
            if case == 0 {
                after.ticket += 1;
            }
            if case == 1 {
                before.current_tick = 7;
                after = before;
            }
            if case == 2 {
                succeeded = false;
            }
            if case == 3 {
                ticks = (8, 8);
            }
            phase.observe_pending_step(
                5,
                ticks,
                succeeded,
                (Some(before), Some(after)),
                Some(due),
                due,
            );
            if case == 4 {
                phase.observe_pending_step(
                    5,
                    (9, 10),
                    true,
                    (Some(key), Some(key)),
                    Some(due),
                    due,
                );
            }
            if case == 5 {
                phase.reset(5, 9, true);
            }
            let mut completed_key = key;
            match case {
                6 => completed_key.ticket += 1,
                7 => completed_key.generation += 1,
                8 => completed_key.source_revision += 1,
                9 => completed_key.current_tick += 1,
                _ => {}
            }
            let epoch = if case == 10 { 6 } else { 5 };
            let live_tick = if case == 11 { 10 } else { 9 };
            let deadline = if case == 12 {
                Some(due + Duration::from_nanos(1))
            } else {
                Some(due)
            };
            let seconds = match case {
                13 => PRESENTATION_INTERVAL.as_secs_f64(),
                14 => f64::NAN,
                15 => -0.001,
                _ => 0.001,
            };
            assert!(
                phase
                    .after_pending_publication(
                        completed_key,
                        epoch,
                        live_tick,
                        deadline,
                        due,
                        seconds
                    )
                    .is_none(),
                "case {case}"
            );
            assert!(
                phase.armed.is_some(),
                "Rejected observations must not consume the one-shot arm"
            );
            assert!(phase.pending.is_none());
        }
    }

    #[test]
    fn cancelled_or_failed_host_flight_cannot_rephase_a_later_completion() {
        for cancel in [false, true] {
            let (host, draws, _temp) = crate::game_host::presentation_tests::gpu_host();
            let mut actor = actor_with_host(host);
            assert!(actor.render_if_due());
            actor.command(WorkerCommand::SetPlaying(true));
            actor.host.camera.yaw += 0.5;
            draws.lock().unwrap().delayed = true;
            actor.next_render = Some(Instant::now() - Duration::from_millis(1));
            assert!(!actor.render_if_due());
            let key = actor.host.pending_interpolated_identity();
            actor
                .host
                .step_budget(FIXED_DT, &GameInput::default(), 1)
                .unwrap();
            actor.observe_completed_step((0, 1), true, key, Instant::now());
            assert!(actor.presentation_phase.pending.is_some());
            let deadline = actor.next_render;
            if cancel {
                actor.host.reset_presentation_history();
            } else {
                draws.lock().unwrap().fail_poll = true;
            }
            assert_eq!(actor.poll_rendered(), !cancel);
            assert!(actor.presentation_phase.pending.is_none());
            assert_eq!(actor.next_render, deadline);
            assert!(actor.presentation_phase.armed.is_some());
        }
    }

    #[test]
    fn ready_gpu_frame_reaches_latest_before_the_next_gated_tick_finishes() {
        check_gated_completion_publication(true, false, false);
    }

    #[test]
    fn gpu_frame_completed_during_a_tick_is_published_before_another_poll_wait() {
        for failed_step in [false, true] {
            check_gated_completion_publication(false, failed_step, false);
        }
    }

    #[test]
    fn actual_owner_captures_the_pending_flight_crossing_a_gated_step_for_one_phase_correction() {
        check_gated_completion_publication(false, false, true);
    }

    fn check_gated_completion_publication(
        ready_before_step: bool,
        failed_step: bool,
        capture_pending_phase: bool,
    ) {
        use module_engine_core::{
            ExecutableSystem, GameSession, SystemContext, SystemDescriptor, SystemRegistry,
        };
        use std::sync::{Condvar, mpsc};

        struct GatedStep {
            entered: mpsc::Sender<()>,
            gate: Arc<(Mutex<bool>, Condvar)>,
            finished: Arc<AtomicBool>,
            fail: bool,
        }
        impl ExecutableSystem for GatedStep {
            fn step(
                &mut self,
                context: &mut SystemContext<'_>,
                _: &mut Value,
            ) -> Result<(), String> {
                self.entered.send(()).map_err(|error| error.to_string())?;
                let (released, ready) = &*self.gate;
                let (released, _) = ready
                    .wait_timeout_while(
                        released.lock().map_err(|_| "Tick gate poisoned")?,
                        Duration::from_secs(10),
                        |released| !*released,
                    )
                    .map_err(|_| "Tick gate poisoned")?;
                if !*released {
                    return Err("Gated tick was not released".into());
                }
                context.game.world.transform_mut(1).unwrap().translation[0] += 1.;
                self.finished.store(true, Ordering::Release);
                if self.fail {
                    Err("injected gated tick rejection".into())
                } else {
                    Ok(())
                }
            }
        }
        struct RunningActor {
            shared: Arc<Shared>,
            gates: Vec<Arc<(Mutex<bool>, Condvar)>>,
            owner: Option<thread::JoinHandle<Actor>>,
        }
        impl RunningActor {
            fn stop(&mut self) -> Option<thread::Result<Actor>> {
                let owner = self.owner.take()?;
                self.shared.shutdown.store(true, Ordering::Release);
                for gate in &self.gates {
                    let (released, ready) = &**gate;
                    *released
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                    ready.notify_all();
                }
                self.shared.wake.notify(owner.thread());
                Some(owner.join())
            }
        }
        impl Drop for RunningActor {
            fn drop(&mut self) {
                // A red assertion or a failed handshake still releases the real
                // step, wakes shutdown and joins only this test's owner.
                let _ = self.stop();
            }
        }

        let (mut host, draws, _temp) = crate::game_host::presentation_tests::gpu_host();
        let project = host.accepted_document().game.as_ref().unwrap().clone();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let step_finished = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let mut systems = SystemRegistry::builtin();
        let step_gate = gate.clone();
        let finished = step_finished.clone();
        systems
            .register(
                SystemDescriptor {
                    id: "fixture.presentation_move".into(),
                    state_schema: "fixture.presentation_move/1".into(),
                    ports: vec![],
                    components: vec![],
                    requires: vec![],
                },
                move |_| {
                    Ok(Box::new(GatedStep {
                        entered: entered_tx.clone(),
                        gate: step_gate.clone(),
                        finished: finished.clone(),
                        fail: failed_step,
                    }))
                },
            )
            .unwrap();
        host.session = GameSession::new_with_registry(project, WorldKind::Ecs, systems).unwrap();
        let mut actor = actor_with_host(host);
        assert!(actor.render_if_due());
        actor.publish();
        let previous = actor.shared.presentation.lock().unwrap().clone().unwrap();
        actor.command(WorkerCommand::SetPlaying(true));
        actor.host.camera.yaw += 0.5;
        let submitted_camera = actor.host.camera;
        let submitted_revision = actor.host.presentation_revision();
        draws.lock().unwrap().delayed = true;
        if capture_pending_phase {
            actor.next_render = Some(Instant::now() - Duration::from_millis(1));
        }
        assert!(!actor.render_if_due());
        assert!(actor.host.has_pending_presentation());
        let submitted_identity = actor.host.pending_interpolated_identity();
        let submitted_deadline = actor.pending_render_deadline;
        draws.lock().unwrap().ready = ready_before_step;
        actor.host.camera.yaw += 0.75;
        actor.clock = Instant::now() - Duration::from_secs_f64(FIXED_DT) - Duration::from_millis(1);

        let shared = actor.shared.clone();
        let publication_gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (published_tx, published_rx) = mpsc::channel();
        if !ready_before_step {
            // Hold the first snapshot publication after the actual step. The
            // owner cannot reach another wait/poll while this callback is held.
            // Old ordering publishes the prior frame here; the new poll must
            // have installed the completed frame before this same boundary.
            let publication_gate = publication_gate.clone();
            let notified = AtomicBool::new(false);
            actor.repaint = Some(Arc::new(move || {
                if step_finished.load(Ordering::Acquire) && !notified.swap(true, Ordering::AcqRel) {
                    let _ = published_tx.send(());
                    let (released, ready) = &*publication_gate;
                    drop(
                        ready.wait_timeout_while(
                            released
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner),
                            Duration::from_secs(10),
                            |released| !*released,
                        ),
                    );
                }
            }));
            // This test isolates completion publication rather than successor
            // cadence. No new admission is eligible while its callback is held.
            actor.next_render = Some(Instant::now() + Duration::from_secs(60));
        }
        let next_render = actor.next_render;
        let owner = thread::spawn(move || {
            actor.run();
            actor
        });
        let mut running = RunningActor {
            shared: shared.clone(),
            gates: vec![gate.clone(), publication_gate],
            owner: Some(owner),
        };
        let mut client = GameWorker {
            shared: shared.clone(),
            thread: running.owner.as_ref().unwrap().thread().clone(),
            cached: None,
        };
        // Receipt proves the real simulation step is blocked after the owner's
        // completion poll. Capture latest before releasing it; no elapsed-time
        // assumption decides whether the accepted frame reached the UI.
        let entered = entered_rx.recv_timeout(Duration::from_secs(10));
        let during_tick = client.latest();
        let polls_before_release = draws.lock().unwrap().polls;
        let (published, visible) = if ready_before_step {
            (None, during_tick.clone())
        } else {
            draws.lock().unwrap().ready = !capture_pending_phase;
            {
                let (released, ready) = &*gate;
                *released
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                ready.notify_all();
            }
            let published = published_rx.recv_timeout(Duration::from_secs(10));
            (Some(published), client.latest())
        };
        // Capture while the actual step/publication gate is still held. Once
        // shutdown releases it, a rejected tick may legitimately admit the
        // changed stopped camera before the owner reaches its next loop check.
        let (attempts_at_boundary, polls_at_boundary) = {
            let draws = draws.lock().unwrap();
            (draws.attempts, draws.polls)
        };
        let mut actor = running.stop().unwrap().unwrap();

        entered.expect("Owner did not reach the gated simulation tick");
        if capture_pending_phase {
            published
                .unwrap()
                .expect("Owner did not publish the successful gated step");
            let visible = visible.unwrap();
            assert_eq!(
                visible.frame_presentation.as_ref().unwrap().serial,
                previous.frame_presentation.as_ref().unwrap().serial
            );
            assert_eq!(visible.state.tick, 1);
            assert_eq!(
                actor.host.pending_interpolated_identity(),
                submitted_identity
            );
            let blocked = actor
                .presentation_phase
                .pending
                .expect("The actual owner must retain the flight that crossed its step");
            assert_eq!(Some(blocked.identity), submitted_identity);
            assert_eq!(Some(Some(blocked.deadline)), submitted_deadline);
            assert_eq!(blocked.advanced_tick, 1);
            assert_eq!(actor.next_render, next_render);
            draws.lock().unwrap().ready = true;
            let rendered = actor
                .host
                .poll_interpolated_presentation()
                .unwrap()
                .unwrap();
            actor.accept_rendered(rendered).unwrap();
            let completed = actor.frame_completed_at.unwrap();
            let metadata = serde_json::to_value(&actor.frame_presentation).unwrap();
            actor.clock = completed;
            actor.next_render = Some(completed);
            actor.rephase_after_successful_publication(
                submitted_deadline.flatten(),
                completed + Duration::from_millis(1),
            );
            assert_eq!(
                actor.next_render,
                Some(completed),
                "Source06's ordinary rule cannot correct the old-tick completion"
            );
            actor.rephase_after_pending_publication(
                submitted_identity,
                submitted_deadline.flatten(),
                completed + Duration::from_millis(1),
            );
            assert_eq!(actor.next_render, Some(completed + PRESENTATION_INTERVAL));
            assert!(actor.presentation_phase.armed.is_none());
            assert_eq!(
                serde_json::to_value(&actor.frame_presentation).unwrap(),
                metadata
            );
            assert_eq!(
                actor.frame_presentation.as_ref().unwrap().source_revision,
                submitted_revision
            );
            assert_eq!(
                actor.frame_presentation.as_ref().unwrap().camera,
                submitted_camera
            );
            assert_eq!(
                actor
                    .frame_presentation
                    .as_ref()
                    .unwrap()
                    .provenance
                    .current_tick,
                0
            );
            assert_eq!(actor.host.session.state().tick, 1);
            assert_eq!(attempts_at_boundary, 2);
            return;
        }
        if let Some(published) = published {
            published.expect("Owner did not publish after the gated simulation attempt");
            assert_eq!(
                during_tick
                    .unwrap()
                    .frame_presentation
                    .as_ref()
                    .unwrap()
                    .serial,
                previous.frame_presentation.as_ref().unwrap().serial,
                "Pre-step poll must have observed the request as not ready"
            );
            assert!(polls_before_release > 0);
            assert_eq!(polls_at_boundary, polls_before_release + 1);
        }
        let visible = visible.expect("Owner did not publish a snapshot");
        let metadata = visible.frame_presentation.as_ref().unwrap();
        assert_eq!(
            metadata.serial,
            previous.frame_presentation.as_ref().unwrap().serial + 1
        );
        assert_eq!(metadata.camera, submitted_camera);
        assert_eq!(metadata.source_revision, submitted_revision);
        assert_eq!(metadata.provenance.current_tick, 0);
        assert_eq!(metadata.provenance.previous_tick, 0);
        assert_eq!(metadata.provenance.current_time, 0.);
        assert_eq!(metadata.provenance.visual_time, 0.);
        assert_eq!(
            visible.state.tick,
            if ready_before_step || failed_step {
                0
            } else {
                1
            }
        );
        if failed_step {
            assert!(
                visible
                    .fault
                    .as_deref()
                    .is_some_and(|error| error.contains("injected gated tick rejection"))
            );
        } else {
            assert!(visible.fault.is_none());
        }
        assert_eq!(visible.camera, submitted_camera);
        assert_ne!(visible.camera, actor.host.camera);
        assert_eq!(visible.frame_completed_at, actor.frame_completed_at);
        assert!(visible.frame_completed_at > previous.frame_completed_at);
        assert!(Arc::ptr_eq(
            visible.frame.as_ref().unwrap(),
            actor.frame.as_ref().unwrap()
        ));
        assert_eq!(
            serde_json::to_value(&visible.frame_presentation).unwrap(),
            serde_json::to_value(&actor.frame_presentation).unwrap()
        );
        assert_eq!(
            actor.host.session.state().tick,
            u64::from(!failed_step),
            "Released tick must commit or roll back according to its real result"
        );
        assert_eq!(
            actor
                .host
                .session
                .state()
                .world
                .object(1)
                .unwrap()
                .transform
                .translation[0],
            if failed_step { 0. } else { 1. }
        );
        assert_eq!(
            actor.next_render, next_render,
            "Completion polling must not retime admission"
        );
        assert_eq!(
            attempts_at_boundary, 2,
            "Publication must not submit or fake an extra render"
        );
    }

    #[test]
    fn collided_presentation_grid_reanchors_once_and_then_keeps_absolute_slots() {
        let start = Instant::now();
        let due = start + Duration::from_millis(10);
        let completed = start + Duration::from_millis(15);
        let mut phase = PresentationPhase::default();
        phase.reset(4, 8, true);
        phase.observe_step(
            4,
            (8, 9),
            true,
            Some(due),
            start + Duration::from_millis(14),
        );
        let first = phase
            .after_publication(4, 9, Some(due), completed, 0.005)
            .unwrap();
        assert_eq!(first, completed + PRESENTATION_INTERVAL);
        assert!(phase.armed.is_none());
        phase.observe_step(
            4,
            (9, 10),
            true,
            Some(first),
            first + Duration::from_millis(4),
        );
        assert!(
            phase
                .after_publication(4, 10, Some(first), first + Duration::from_millis(5), 0.005)
                .is_none()
        );
        assert_eq!(
            next_presentation_deadline(Some(first), first, PRESENTATION_INTERVAL),
            first + PRESENTATION_INTERVAL
        );
        assert_eq!(
            next_presentation_deadline(
                Some(first),
                first + PRESENTATION_INTERVAL * 2,
                PRESENTATION_INTERVAL
            ),
            first + PRESENTATION_INTERVAL * 3
        );
    }

    #[test]
    fn presentation_grid_with_deadline_after_step_never_reanchors() {
        let start = Instant::now();
        let due = start + Duration::from_millis(10);
        let mut phase = PresentationPhase::default();
        phase.reset(0, 0, true);
        phase.observe_step(0, (0, 1), true, Some(due), due - Duration::from_millis(1));
        assert!(
            phase
                .after_publication(0, 1, Some(due), due + Duration::from_millis(1), 0.005)
                .is_none()
        );
        assert_eq!(phase.armed, Some((0, 0)));
        assert!(phase.blocked.is_none());
    }

    #[test]
    fn presentation_rephase_rejects_failed_steps_and_missing_or_stale_deadlines() {
        let start = Instant::now();
        let due = start + Duration::from_millis(10);
        for (ticks, succeeded, observed, rendered) in [
            ((0, 1), false, Some(due), Some(due)),
            ((1, 1), true, Some(due), Some(due)),
            ((1, 0), true, Some(due), Some(due)),
            ((0, 1), true, None, None),
            ((0, 1), true, Some(due), None),
            ((0, 1), true, Some(due), Some(due + PRESENTATION_INTERVAL)),
        ] {
            let mut phase = PresentationPhase::default();
            phase.reset(0, 0, true);
            phase.observe_step(
                0,
                ticks,
                succeeded,
                observed,
                due + Duration::from_millis(4),
            );
            assert!(
                phase
                    .after_publication(0, 1, rendered, due + Duration::from_millis(5), 0.005)
                    .is_none()
            );
            assert_eq!(phase.armed, Some((0, 0)));
        }
    }

    #[test]
    fn presentation_rephase_requires_advanced_matching_epoch_tick_and_real_headroom() {
        let start = Instant::now();
        let due = start + Duration::from_millis(10);
        for (epoch, tick, seconds) in [
            (3, 9, 0.005),
            (4, 8, 0.005),
            (4, 10, 0.005),
            (4, 9, -0.001),
            (4, 9, f64::NAN),
            (4, 9, f64::INFINITY),
            (4, 9, PRESENTATION_INTERVAL.as_secs_f64()),
            (4, 9, FIXED_DT),
        ] {
            let mut phase = PresentationPhase::default();
            phase.reset(4, 8, true);
            phase.observe_step(4, (8, 9), true, Some(due), due + Duration::from_millis(4));
            assert!(
                phase
                    .after_publication(
                        epoch,
                        tick,
                        Some(due),
                        due + Duration::from_millis(5),
                        seconds
                    )
                    .is_none()
            );
            assert_eq!(phase.armed, Some((4, 8)));
            assert!(
                phase
                    .after_publication(4, 9, Some(due), due + Duration::from_millis(6), 0.006)
                    .is_some()
            );
        }
    }

    #[test]
    fn rephase_preserves_the_actual_published_frame_camera_and_authoritative_state() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        actor.command(WorkerCommand::SetPlaying(true));
        assert!(actor.render_if_due());
        assert_eq!(actor.host.session.state().tick, 0);
        assert_eq!(actor.presentation_phase.armed, Some((0, 0)));
        actor
            .host
            .step(
                FIXED_DT,
                &GameInput {
                    actions: std::collections::BTreeSet::from(["pulse".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        let revision = actor.host.presentation_revision();
        let rendered = actor
            .host
            .try_render_interpolated(160, 100, 0.25)
            .unwrap()
            .unwrap();
        actor.accept_rendered(rendered).unwrap();
        let frame = actor.frame.clone().unwrap();
        let metadata = serde_json::to_value(&actor.frame_presentation).unwrap();
        let camera = actor.host.camera;
        let state = actor.host.session.state().clone();
        let pending = actor.host.session.pending_time();
        let document = actor.host.accepted_document().clone();
        let calls = actor.host.renderer_performance().render_calls;
        let completed = actor.frame_completed_at.unwrap();
        let due = completed - Duration::from_millis(2);
        actor.clock = completed - Duration::from_millis(1);
        let clock = actor.clock;
        actor.presentation_phase.observe_step(
            0,
            (0, 1),
            true,
            Some(due),
            completed - Duration::from_millis(1),
        );
        actor.next_render = Some(next_presentation_deadline(
            Some(due),
            completed,
            PRESENTATION_INTERVAL,
        ));
        let unchanged_deadline = actor.next_render;
        let mut malformed = (*frame).clone();
        malformed.rgba.pop();
        actor.frame = Some(Arc::new(malformed));
        actor.rephase_after_successful_publication(Some(due), completed + Duration::from_millis(1));
        assert_eq!(actor.next_render, unchanged_deadline);
        assert_eq!(actor.presentation_phase.armed, Some((0, 0)));
        actor.frame = Some(frame.clone());
        actor.fault = Some("injected fault".into());
        actor.rephase_after_successful_publication(Some(due), completed + Duration::from_millis(1));
        assert_eq!(actor.next_render, unchanged_deadline);
        assert_eq!(actor.presentation_phase.armed, Some((0, 0)));
        actor.fault = None;
        actor.rephase_after_successful_publication(Some(due), completed + Duration::from_millis(1));
        assert_eq!(actor.next_render, Some(completed + PRESENTATION_INTERVAL));
        assert!(actor.presentation_phase.armed.is_none());
        assert!(Arc::ptr_eq(&frame, actor.frame.as_ref().unwrap()));
        assert_eq!(
            serde_json::to_value(&actor.frame_presentation).unwrap(),
            metadata
        );
        assert_eq!(actor.frame_completed_at, Some(completed));
        assert_eq!(actor.host.camera, camera);
        assert_eq!(actor.host.session.state(), &state);
        assert_eq!(actor.host.session.pending_time(), pending);
        assert!(actor.host.accepted_document().execution_eq(&document));
        assert_eq!(actor.host.presentation_revision(), revision);
        assert_eq!(actor.host.renderer_performance().render_calls, calls);
        assert_eq!(actor.clock, clock);
    }

    #[test]
    fn meaningful_grid_resets_rearm_while_repeated_invalid_and_hidden_viewports_do_not() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        actor.command(WorkerCommand::SetPlaying(true));
        actor.host.step(FIXED_DT, &GameInput::default()).unwrap();
        let due = Instant::now();
        actor.next_render = Some(due);
        actor
            .presentation_phase
            .observe_step(0, (0, 1), true, Some(due), due);
        actor.command(WorkerCommand::SetPlaying(true));
        actor.command(WorkerCommand::SetViewport {
            width: 160,
            height: 100,
        });
        actor.command(WorkerCommand::SetViewport {
            width: 16_777_217,
            height: 1,
        });
        assert_eq!(actor.next_render, Some(due));
        assert_eq!(actor.presentation_phase.armed, Some((0, 0)));
        assert!(actor.presentation_phase.blocked.is_some());
        actor.command(WorkerCommand::SetViewport {
            width: 161,
            height: 100,
        });
        assert!(actor.next_render.is_none());
        assert_eq!(actor.presentation_phase.armed, Some((0, 1)));
        assert!(actor.presentation_phase.blocked.is_none());
        actor.presentation_phase.armed = None;
        actor.command(WorkerCommand::InvalidateContent);
        assert_eq!(actor.presentation_phase.armed, Some((0, 1)));
        actor.command(WorkerCommand::SetViewport {
            width: 0,
            height: 0,
        });
        assert!(actor.presentation_phase.armed.is_none());
        actor.command(WorkerCommand::SetViewport {
            width: 160,
            height: 100,
        });
        assert_eq!(actor.presentation_phase.armed, Some((0, 1)));
        actor.command(WorkerCommand::SetPlaying(false));
        assert!(actor.presentation_phase.armed.is_none());
        actor.restart(None).unwrap();
        assert!(actor.presentation_phase.armed.is_none());
        actor.command(WorkerCommand::SetPlaying(true));
        assert_eq!(actor.presentation_phase.armed, Some((1, 0)));
        actor.restart(None).unwrap();
        assert_eq!(actor.presentation_phase.armed, Some((2, 0)));
        assert!(actor.presentation_phase.blocked.is_none());
    }

    #[test]
    fn failed_render_preserves_rephase_arm_and_last_good_frame() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        actor.command(WorkerCommand::SetPlaying(true));
        assert!(actor.render_if_due());
        let frame = actor.frame.clone().unwrap();
        let metadata = serde_json::to_value(&actor.frame_presentation).unwrap();
        actor
            .host
            .step(
                FIXED_DT,
                &GameInput {
                    actions: std::collections::BTreeSet::from(["pulse".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        let due = Instant::now() - Duration::from_millis(1);
        actor.next_render = Some(due);
        actor
            .presentation_phase
            .observe_step(0, (0, 1), true, Some(due), Instant::now());
        fixture.set_draw_failure(true);
        assert!(actor.render_if_due());
        assert!(actor.render_error.is_some());
        assert_eq!(actor.presentation_phase.armed, Some((0, 0)));
        assert_ne!(actor.next_render, Some(due));
        assert!(Arc::ptr_eq(&frame, actor.frame.as_ref().unwrap()));
        assert_eq!(
            serde_json::to_value(&actor.frame_presentation).unwrap(),
            metadata
        );
    }

    #[test]
    fn tick_wait_keeps_submillisecond_deadlines_and_adds_no_delay_to_ready_backlog() {
        assert_eq!(tick_wait(0., 0.), Duration::from_secs_f64(FIXED_DT));
        assert_eq!(tick_wait(0., FIXED_DT - 0.0001), Duration::from_micros(100));
        assert_eq!(tick_wait(FIXED_DT - 0.0001, 0.), Duration::from_micros(100));
        assert_eq!(tick_wait(FIXED_DT, 0.), Duration::ZERO);
        assert_eq!(tick_wait(FIXED_DT * 2., 0.004), Duration::ZERO);
    }

    #[test]
    fn presentation_fraction_is_independent_of_rate_and_never_extrapolates_backlog() {
        assert_eq!(presentation_alpha(0., FIXED_DT / 4., true), 0.25);
        assert_eq!(presentation_alpha(FIXED_DT / 4., FIXED_DT / 4., true), 0.5);
        assert_eq!(presentation_alpha(0., FIXED_DT, true), 1.);
        assert_eq!(presentation_alpha(FIXED_DT * 2., 0., true), 1.);
        assert_eq!(presentation_alpha(0., 0., false), 1.);
    }

    #[test]
    fn presentation_deadlines_keep_the_grid_and_skip_missed_slots_without_a_burst() {
        let start = Instant::now();
        let interval = PRESENTATION_INTERVAL;
        let first = next_presentation_deadline(None, start, interval);
        assert_eq!(first, start + interval);
        assert_eq!(
            next_presentation_deadline(Some(first), first, interval),
            start + interval * 2
        );
        let late = start + interval * 4 + interval / 2;
        assert_eq!(
            next_presentation_deadline(Some(first), late, interval),
            start + interval * 5
        );
        assert_eq!(
            next_presentation_deadline(Some(first), start, interval),
            first
        );
    }

    #[test]
    fn owner_wait_tracks_simulation_presentation_and_preparation_without_idle_spinning() {
        assert_eq!(owner_wait(None, None, false), Duration::from_millis(100));
        assert_eq!(owner_wait(None, None, true), Duration::from_millis(10));
        assert_eq!(
            owner_wait(
                Some(Duration::from_millis(15)),
                Some(PRESENTATION_INTERVAL),
                false
            ),
            PRESENTATION_INTERVAL
        );
        assert_eq!(
            owner_wait(Some(Duration::ZERO), Some(PRESENTATION_INTERVAL), true),
            Duration::ZERO
        );
        assert_eq!(
            owner_wait(None, Some(STOPPED_PRESENTATION_INTERVAL), false),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn real_interpolated_frames_keep_their_source_tick_and_advance_only_frame_identity() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        assert!(actor.render_if_due());
        let input = GameInput {
            actions: std::collections::BTreeSet::from(["pulse".into()]),
            ..Default::default()
        };
        actor.host.step(FIXED_DT, &input).unwrap();
        let committed = actor.host.session.state().clone();
        let revision = actor.host.presentation_revision();
        let first = actor
            .host
            .try_render_interpolated(160, 100, 0.25)
            .unwrap()
            .unwrap();
        actor.accept_rendered(first).unwrap();
        let first_frame = actor.frame.clone().unwrap();
        let first_metadata = actor.frame_presentation.clone().unwrap();
        let first_completed = actor.frame_completed_at.unwrap();
        let second = actor
            .host
            .try_render_interpolated(160, 100, 0.75)
            .unwrap()
            .unwrap();
        actor.accept_rendered(second).unwrap();
        let second_metadata = actor.frame_presentation.clone().unwrap();
        assert!(!Arc::ptr_eq(&first_frame, actor.frame.as_ref().unwrap()));
        assert_ne!(first_frame.rgba, actor.frame.as_ref().unwrap().rgba);
        assert_eq!(second_metadata.actor_id, 17);
        assert_eq!(second_metadata.epoch, 0);
        assert_eq!(second_metadata.serial, first_metadata.serial + 1);
        assert_eq!(second_metadata.source_revision, revision);
        assert_eq!(second_metadata.provenance.previous_tick, 0);
        assert_eq!(second_metadata.provenance.current_tick, 1);
        assert_eq!(first_metadata.provenance.alpha, 0.25);
        assert_eq!(second_metadata.provenance.alpha, 0.75);
        assert_eq!(actor.frame_tick, Some(1));
        assert_eq!(actor.frame_revision, Some(revision));
        assert!(actor.frame_completed_at.unwrap() > first_completed);
        assert_eq!(actor.host.session.state(), &committed);
        assert_eq!(actor.host.presentation_revision(), revision);

        let metadata = serde_json::to_value(&actor.frame_presentation).unwrap();
        assert!(
            actor
                .host
                .try_render_interpolated(160, 100, 0.75)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            serde_json::to_value(&actor.frame_presentation).unwrap(),
            metadata
        );
        actor.publish();
        let snapshot = actor.shared.presentation.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.camera, second_metadata.camera);
        assert_eq!(snapshot.frame_completed_at, actor.frame_completed_at);
    }

    #[test]
    fn same_static_between_tick_sample_does_not_publish_another_frame() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        actor.command(WorkerCommand::SetPlaying(true));
        assert!(actor.render_if_due());
        let frame = actor.frame.clone().unwrap();
        let completed = actor.frame_completed_at;
        let metadata = serde_json::to_value(&actor.frame_presentation).unwrap();
        let calls = actor.host.renderer_performance().render_calls;
        actor.next_render = Some(Instant::now());
        assert!(!actor.render_if_due());
        assert!(Arc::ptr_eq(actor.frame.as_ref().unwrap(), &frame));
        assert_eq!(actor.frame_completed_at, completed);
        assert_eq!(
            serde_json::to_value(&actor.frame_presentation).unwrap(),
            metadata
        );
        assert_eq!(actor.host.renderer_performance().render_calls, calls);
        assert_eq!(actor.host.session.state().tick, 0);
        assert_eq!(actor.presentation_phase.armed, Some((0, 0)));
    }

    #[cfg(windows)]
    #[test]
    fn idle_wait_failure_publishes_fallback_without_a_new_frame_or_simulation_step() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        assert!(!actor.playing);
        assert!(actor.render_if_due());
        actor.publish();
        let before = actor.shared.presentation.lock().unwrap().clone().unwrap();
        if before.deadline_wait_strategy == "std-park-fallback" {
            // Older/unsupported runtimes truthfully report construction failure;
            // the transition requires a successfully created native timer.
            assert!(before.deadline_wait_error.is_some());
            return;
        }
        assert_eq!(
            before.deadline_wait_strategy,
            "windows-high-resolution-waitable-timer"
        );
        assert!(before.deadline_wait_error.is_none());
        let calls = actor.host.renderer_performance().render_calls;
        let revision = actor.host.presentation_revision();
        let metadata = serde_json::to_value(&before.frame_presentation).unwrap();

        // The failure already exists BEFORE wait starts: comparing only the
        // before/after wait state would miss the stale published snapshot.
        actor.shared.wake.inject_notification_failure(6);
        actor.shared.wake.notify(&thread::current());
        actor.wait_for_work(Duration::from_secs(10));
        let after = actor.shared.presentation.lock().unwrap().clone().unwrap();
        assert_eq!(after.sequence, before.sequence + 1);
        assert_eq!(after.deadline_wait_strategy, "std-park-fallback");
        assert_eq!(
            after.deadline_wait_error.as_deref(),
            Some("SetEvent: Windows error 6")
        );
        assert!(!after.playing);
        assert_eq!(after.state, before.state);
        assert_eq!(after.accepted_document, before.accepted_document);
        assert_eq!(after.frame_tick, before.frame_tick);
        assert_eq!(after.frame_revision, before.frame_revision);
        assert_eq!(after.frame_completed_at, before.frame_completed_at);
        assert!(Arc::ptr_eq(
            after.frame.as_ref().unwrap(),
            before.frame.as_ref().unwrap()
        ));
        assert_eq!(
            serde_json::to_value(&after.frame_presentation).unwrap(),
            metadata
        );
        assert_eq!(actor.host.presentation_revision(), revision);
        assert_eq!(actor.host.renderer_performance().render_calls, calls);
        actor.publish_wait_transition();
        assert_eq!(
            actor
                .shared
                .presentation
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .sequence,
            after.sequence
        );
    }

    #[test]
    fn valid_repeated_viewport_repairs_input_error_even_when_static_render_is_deduplicated() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        assert!(actor.render_if_due());
        let frame = actor.frame.clone().unwrap();
        let completed = actor.frame_completed_at;
        let metadata = serde_json::to_value(&actor.frame_presentation).unwrap();
        let committed = actor.host.session.state().clone();
        let document = actor.host.accepted_document().clone();
        let revision = actor.host.presentation_revision();
        let calls = actor.host.renderer_performance().render_calls;

        actor.command(WorkerCommand::SetViewport {
            width: 16_777_217,
            height: 1,
        });
        actor.publish();
        let rejected = actor.shared.presentation.lock().unwrap().clone().unwrap();
        assert_eq!(
            rejected.render_error.as_deref(),
            Some("Game viewport exceeds the pixel limit")
        );
        assert_eq!(actor.size, Some([160, 100]));
        actor.command(WorkerCommand::SetViewport {
            width: 160,
            height: 100,
        });
        actor.next_render = Some(Instant::now());
        assert!(!actor.render_if_due());
        actor.publish();
        let recovered = actor.shared.presentation.lock().unwrap().clone().unwrap();
        assert!(recovered.render_error.is_none());
        assert!(Arc::ptr_eq(recovered.frame.as_ref().unwrap(), &frame));
        assert_eq!(recovered.frame_completed_at, completed);
        assert_eq!(
            serde_json::to_value(&recovered.frame_presentation).unwrap(),
            metadata
        );
        assert_eq!(actor.host.renderer_performance().render_calls, calls);
        assert_eq!(actor.host.presentation_revision(), revision);
        assert_eq!(actor.host.session.state(), &committed);
        assert_eq!(actor.host.accepted_document(), &document);
    }

    #[test]
    fn valid_viewport_repairs_only_input_diagnostics_until_the_gpu_actually_recovers() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        fixture.set_draw_failure(true);
        assert!(actor.render_if_due());
        actor.publish();
        let failed = actor.shared.presentation.lock().unwrap().clone().unwrap();
        let gpu_error = failed.render_error.clone().unwrap();
        assert!(gpu_error.contains("Injected CPU draw failure"));
        actor.command(WorkerCommand::SetViewport {
            width: 16_777_217,
            height: 1,
        });
        actor.command(WorkerCommand::SetViewport {
            width: 160,
            height: 100,
        });
        actor.publish();
        let still_failed = actor.shared.presentation.lock().unwrap().clone().unwrap();
        assert_eq!(still_failed.render_error, Some(gpu_error));
        assert!(still_failed.frame.is_none());
        assert!(still_failed.frame_presentation.is_none());
        fixture.set_draw_failure(false);
        actor.next_render = Some(Instant::now());
        assert!(actor.render_if_due());
        actor.publish();
        let recovered = actor.shared.presentation.lock().unwrap().clone().unwrap();
        assert!(recovered.render_error.is_none());
        assert!(recovered.frame.is_some());
        assert!(recovered.frame_presentation.is_some());
    }

    #[test]
    fn renderer_failure_after_restart_retains_the_entire_previous_frame_epoch_and_camera() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        assert!(actor.render_if_due());
        let frame = actor.frame.clone().unwrap();
        let completed = actor.frame_completed_at;
        let frame_tick = actor.frame_tick;
        let frame_revision = actor.frame_revision;
        let metadata = serde_json::to_value(&actor.frame_presentation).unwrap();
        let camera = actor.frame_presentation.as_ref().unwrap().camera;
        fixture.set_draw_failure(true);
        actor.restart(None).unwrap();
        actor.host.camera.target[0] += 4.;
        assert!(actor.render_if_due());
        actor.publish();
        let failed = actor.shared.presentation.lock().unwrap().clone().unwrap();
        assert_eq!(failed.epoch, 1);
        assert_eq!(failed.frame_presentation.as_ref().unwrap().epoch, 0);
        assert_eq!(
            serde_json::to_value(&failed.frame_presentation).unwrap(),
            metadata
        );
        assert_eq!(failed.frame_tick, frame_tick);
        assert_eq!(failed.frame_revision, frame_revision);
        assert_eq!(failed.frame_completed_at, completed);
        assert!(Arc::ptr_eq(failed.frame.as_ref().unwrap(), &frame));
        assert_eq!(failed.camera, camera);
        assert!(failed.render_error.is_some());

        fixture.set_draw_failure(false);
        actor.next_render = Some(Instant::now());
        assert!(actor.render_if_due());
        let recovered = actor.frame_presentation.as_ref().unwrap();
        assert_eq!(recovered.epoch, 1);
        assert_eq!(recovered.serial, 2);
        assert_eq!(recovered.provenance.previous_tick, 0);
        assert_eq!(recovered.provenance.current_tick, 0);
        assert_eq!(recovered.camera, actor.host.camera);
        assert!(actor.frame_completed_at > completed);
        assert!(actor.render_error.is_none());
    }

    #[test]
    fn hidden_viewport_and_pause_keep_fixed_state_and_bound_the_render_schedule() {
        let fixture = Fixture::new(false);
        let mut actor = actor(&fixture);
        actor.command(WorkerCommand::SetPlaying(true));
        actor.host.step(FIXED_DT, &GameInput::default()).unwrap();
        actor.command(WorkerCommand::SetPlaying(false));
        assert!(actor.render_if_due());
        assert_eq!(
            actor.frame_presentation.as_ref().unwrap().provenance.alpha,
            1.
        );
        let frame = actor.frame.clone().unwrap();
        let completed = actor.frame_completed_at;
        let committed = actor.host.session.state().clone();
        actor.command(WorkerCommand::SetViewport {
            width: 0,
            height: 0,
        });
        assert!(actor.next_render.is_none());
        assert!(!actor.render_if_due());
        assert!(Arc::ptr_eq(actor.frame.as_ref().unwrap(), &frame));
        assert_eq!(actor.frame_completed_at, completed);
        actor.command(WorkerCommand::SetViewport {
            width: 160,
            height: 100,
        });
        assert!(actor.render_if_due());
        assert_eq!(actor.host.session.state(), &committed);
        assert_eq!(
            actor.frame_presentation.as_ref().unwrap().provenance.alpha,
            1.
        );
        assert!(actor.next_render.is_some());
    }

    #[test]
    fn post_window_drain_waits_for_the_last_snapshot_field_to_drop() {
        use std::sync::mpsc::{self, RecvTimeoutError};

        #[derive(Debug)]
        struct LastGpuLease {
            dropping: mpsc::Sender<thread::ThreadId>,
            release: Mutex<mpsc::Receiver<()>>,
            dropped: mpsc::Sender<()>,
        }
        impl module_engine_core::GpuFrame for LastGpuLease {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
        impl Drop for LastGpuLease {
            fn drop(&mut self) {
                let _ = self.dropping.send(thread::current().id());
                let _ = self
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5));
                let _ = self.dropped.send(());
            }
        }

        let fixture = Fixture::new(false);
        let (published_tx, published_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let resume_rx = Mutex::new(resume_rx);
        let worker = GameWorker::spawn(
            fixture.host(),
            Some(Arc::new(move || {
                let _ = published_tx.send(());
                let _ = resume_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5));
            })),
        );
        published_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let owner_thread = worker.thread.id();
        let (dropping_tx, dropping_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let actor_id = {
            let mut presentation = worker.shared.presentation.lock().unwrap();
            let snapshot = Arc::make_mut(presentation.as_mut().unwrap());
            snapshot.frame = Some(Arc::new(Frame {
                width: 1,
                height: 1,
                rgba: vec![],
                gpu: Some(Arc::new(LastGpuLease {
                    dropping: dropping_tx,
                    release: Mutex::new(release_rx),
                    dropped: dropped_tx,
                })),
                stats: Default::default(),
            }));
            snapshot.actor_id
        };
        drop(worker);
        let _ = resume_tx.send(());
        // Alive::drop has run, but its Shared field is still destroying the
        // final published lease. The owner must remain joinable at this point.
        let drop_thread = dropping_rx.recv_timeout(Duration::from_secs(5));
        let registration = {
            let mut registry = worker_registry().lock().unwrap();
            registry
                .iter()
                .position(|worker| worker.actor_id == actor_id)
                .map(|index| registry.swap_remove(index))
        };
        let registered = registration.is_some();
        let (finished_tx, finished_rx) = mpsc::channel();
        let drain = thread::spawn(move || {
            let result = finish_registered(registration.into_iter().collect());
            let _ = finished_tx.send(result);
        });
        let early = finished_rx.recv_timeout(Duration::from_millis(50));
        let returned_early = !matches!(&early, Err(RecvTimeoutError::Timeout));
        let _ = release_tx.send(());
        let result = match early {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                finished_rx.recv_timeout(Duration::from_secs(5)).unwrap()
            }
            Err(error) => panic!("Shutdown drain disconnected: {error}"),
        };
        drain.join().unwrap();
        dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(drop_thread.unwrap(), owner_thread);
        assert!(
            registered,
            "Owner was detached before field cleanup finished"
        );
        assert!(
            !returned_early,
            "Shutdown returned during final lease cleanup"
        );
        result.unwrap();
    }

    #[test]
    fn post_window_drain_signals_all_owners_then_waits_for_actual_cleanup() {
        // A local registry exercises the real join route without shutting down
        // other actor tests that may be running concurrently in this process.
        let make_shared = || {
            Arc::new(Shared {
                mailbox: Mutex::new(Mailbox::default()),
                presentation: Mutex::new(None),
                shutdown: AtomicBool::new(false),
                retired: Arc::new(AtomicBool::new(false)),
                alive: AtomicBool::new(true),
                wake: Arc::new(WakeSignal::new()),
            })
        };
        let first = make_shared();
        let second = make_shared();
        let cleaned = [
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        ];
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let mut registrations = vec![];
        for (actor_id, shared, peer, cleaned) in [
            (1, first.clone(), second.clone(), cleaned[0].clone()),
            (2, second.clone(), first.clone(), cleaned[1].clone()),
        ] {
            let owner_shared = shared.clone();
            let ready = ready_tx.clone();
            let owner = thread::spawn(move || {
                let mut wait = DeadlineWait::new(owner_shared.wake.clone());
                let mut reported = false;
                while !owner_shared.shutdown.load(Ordering::Acquire)
                    || !peer.shutdown.load(Ordering::Acquire)
                {
                    if !reported {
                        let _ = ready.send(());
                        reported = true;
                    }
                    wait.wait(Duration::from_secs(10));
                }
                // Represent cleanup after a current transaction has returned;
                // the caller must not merely signal and discard its handle.
                thread::sleep(Duration::from_millis(10));
                cleaned.store(true, Ordering::Release);
                owner_shared.alive.store(false, Ordering::Release);
            });
            registrations.push(RegisteredWorker {
                actor_id,
                shared: Arc::downgrade(&shared),
                thread: owner.thread().clone(),
                owner,
            });
        }
        drop(ready_tx);
        let ready = (0..2).all(|_| ready_rx.recv_timeout(Duration::from_secs(5)).is_ok());
        let started = Instant::now();
        finish_registered(registrations).unwrap();
        assert!(ready, "Owners did not reach their interruptible wait");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            cleaned
                .iter()
                .all(|cleaned| cleaned.load(Ordering::Acquire))
        );
        for shared in [first, second] {
            assert!(shared.retired.load(Ordering::Acquire));
            assert!(shared.shutdown.load(Ordering::Acquire));
            assert!(!shared.alive.load(Ordering::Acquire));
        }
    }
}
