//! Persistent owner of the interactive Game host. No simulation, GPU work or
//! script-state lock is performed by the UI-facing methods in this module.
use crate::activity::ActivitySnapshot;
use crate::game_host::GameHost;
use eframe::egui;
use module_engine_core::{Camera, Frame, GameInput, GameState, GraphDocument, WorldKind};
use module_engine_script::CompiledScript;
use module_engine_vulkan::RendererPerformance;
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const COMMAND_CAPACITY: usize = 32;
const EDGE_CAPACITY: usize = 128;
const REPLY_CAPACITY: usize = 64;
const FIXED_DT: f64 = 1. / 60.;

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

/// All values come from completed host operations. Retained last-good pixels
/// carry their own tick/revision so a failed render cannot pretend to be fresh.
#[derive(Clone)]
pub struct GameSnapshot {
    pub actor_id: u64,
    pub sequence: u64,
    pub epoch: u64,
    pub state: Arc<GameState>,
    pub camera: Camera,
    pub frame: Option<Arc<Frame>>,
    pub frame_tick: Option<u64>,
    pub frame_revision: Option<u64>,
    pub frame_completed_at: Option<Instant>,
    pub presentation_revision: u64,
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
    audio_retired: Arc<AtomicBool>,
    alive: AtomicBool,
}

struct RegisteredWorker {
    actor_id: u64,
    shared: Arc<Shared>,
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
    finish_registered(workers)
}
fn finish_registered(workers: Vec<RegisteredWorker>) -> Result<(), String> {
    for worker in &workers {
        worker.shared.audio_retired.store(true, Ordering::Release);
        worker.shared.shutdown.store(true, Ordering::Release);
        worker.thread.unpark();
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
    pub fn spawn(host: GameHost, repaint: Option<egui::Context>) -> Self {
        static NEXT_ACTOR: AtomicU64 = AtomicU64::new(1);
        let actor_id = NEXT_ACTOR.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            presentation: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            audio_retired: host.audio.retirement_signal(),
            alive: AtomicBool::new(true),
        });
        let worker_shared = shared.clone();
        let owner = thread::spawn(move || {
            struct Alive {
                shared: Arc<Shared>,
                actor_id: u64,
            }
            impl Drop for Alive {
                fn drop(&mut self) {
                    self.shared.alive.store(false, Ordering::Release);
                    // This guard was created before Actor, so its host/slots
                    // and queued candidates have already been dropped here.
                    worker_registry()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .retain(|worker| worker.actor_id != self.actor_id);
                }
            }
            let _alive = Alive {
                shared: worker_shared.clone(),
                actor_id,
            };
            let mut actor = Actor::new(host, actor_id, worker_shared, repaint);
            actor.run();
            actor.host.audio.stop_endpoint();
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
            // Completed handles need no UI join. Live entries alone retain the
            // resources needed by finish_after_ui's post-window shutdown.
            registry.retain(|worker| !worker.owner.is_finished());
            if shared.alive.load(Ordering::Acquire) {
                registry.push(RegisteredWorker {
                    actor_id,
                    shared: shared.clone(),
                    thread: thread.clone(),
                    owner,
                });
            }
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
        let pressed = input.attack || input.interact || input.restart;
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
            attack: false,
            interact: false,
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
        self.thread.unpark();
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
        self.thread.unpark();
        Ok(())
    }

    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::Acquire)
    }
    pub fn shutdown(&self) {
        self.shared.audio_retired.store(true, Ordering::Release);
        self.shared.shutdown.store(true, Ordering::Release);
        self.thread.unpark();
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
    actor_id: u64,
    shared: Arc<Shared>,
    repaint: Option<egui::Context>,
    sequence: u64,
    epoch: u64,
    playing: bool,
    size: Option<[usize; 2]>,
    frame: Option<Arc<Frame>>,
    frame_tick: Option<u64>,
    frame_revision: Option<u64>,
    frame_completed_at: Option<Instant>,
    attempted_revision: Option<u64>,
    rendered_camera: Option<Camera>,
    rendered_size: Option<[usize; 2]>,
    last_render: Option<Instant>,
    clock: Instant,
    fault: Option<String>,
    preparation_error: Option<String>,
    render_error: Option<String>,
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
        repaint: Option<egui::Context>,
    ) -> Self {
        Self {
            host,
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
            attempted_revision: None,
            rendered_camera: None,
            rendered_size: None,
            last_render: None,
            clock: Instant::now(),
            fault: None,
            preparation_error: None,
            render_error: None,
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
            if self.playing && self.fault.is_none() {
                let elapsed = self.clock.elapsed().as_secs_f64();
                if elapsed + self.host.session.pending_time() + 1e-12 >= FIXED_DT {
                    self.clock = Instant::now();
                    let (mut input, edge) = {
                        let mailbox = self.shared.mailbox.lock().expect("Game mailbox poisoned");
                        let edge = mailbox
                            .edges
                            .front()
                            .map(|edge| (edge.sequence, edge.input.clone()));
                        (mailbox.held.clone(), edge)
                    };
                    if let Some((_, pressed)) = &edge {
                        input.attack = pressed.attack;
                        input.interact = pressed.interact;
                        input.restart = pressed.restart;
                        input.aim_world = pressed.aim_world;
                    }
                    let before_tick = self.host.session.state().tick;
                    let result = if input.restart {
                        // A newly submitted R can arrive after the priority
                        // check above; it still uses the actor epoch boundary.
                        self.restart(edge.as_ref().map(|(sequence, _)| *sequence))
                            .map(|_| 0)
                    } else {
                        self.host.step_budget(elapsed, &input, 1)
                    };
                    let committed = self.host.session.state().tick > before_tick;
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
            changed |= self.render_if_due();
            if changed {
                self.publish();
            }
            let wait = if self.playing && self.fault.is_none() {
                Duration::from_secs_f64(
                    (FIXED_DT
                        - self.host.session.pending_time()
                        - self.clock.elapsed().as_secs_f64())
                    .clamp(0.001, FIXED_DT),
                )
            } else if self.host.has_pending_preparation() {
                Duration::from_millis(10)
            } else {
                Duration::from_millis(100)
            };
            thread::park_timeout(wait);
        }
    }

    fn command(&mut self, command: WorkerCommand) {
        match command {
            WorkerCommand::SetPlaying(playing) => {
                if self.playing != playing {
                    self.playing = playing;
                    self.clock = Instant::now();
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
                if width == 0 || height == 0 {
                    self.size = None;
                } else if width
                    .checked_mul(height)
                    .is_some_and(|pixels| pixels <= 16_777_216)
                {
                    self.size = Some([width, height]);
                } else {
                    self.render_error = Some("Game viewport exceeds the pixel limit".into());
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
        self.fault = None;
        self.clock = Instant::now();
        self.last_render = None;
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
        mailbox.held.attack = false;
        mailbox.held.interact = false;
        mailbox.held.restart = false;
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
                self.last_render = None;
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
        let graphics =
            crate::game_host::graphics_only_change(self.host.accepted_document(), &document);
        let result = if authoring && !self.playing && !graphics {
            self.host.refresh_author_preview(&document)
        } else {
            self.host.sync_document_deferred(&document)
        };
        match result {
            Ok(()) if self.host.accepted_document().execution_eq(&document) => {
                self.preparation_error = None;
                self.document = None;
                if authoring && let Some(player) = self.host.session.state().player() {
                    self.host.camera.target = player.object.transform.translation;
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

    fn render_if_due(&mut self) -> bool {
        let Some([width, height]) = self.size else {
            return false;
        };
        let revision = self.host.presentation_revision();
        let changed = self.attempted_revision != Some(revision)
            || self.rendered_camera != Some(self.host.camera)
            || self.rendered_size != self.size;
        let interval = if self.playing && self.fault.is_none() {
            Duration::from_secs_f64(FIXED_DT)
        } else {
            Duration::from_millis(250)
        };
        if !changed
            && self
                .last_render
                .is_some_and(|previous| previous.elapsed() < interval)
        {
            return false;
        }
        self.last_render = Some(Instant::now());
        self.attempted_revision = Some(revision);
        self.rendered_camera = Some(self.host.camera);
        self.rendered_size = self.size;
        match self.host.render(width, height) {
            Ok(frame) => {
                self.frame = Some(Arc::new(frame));
                self.frame_tick = Some(self.host.session.state().tick);
                self.frame_revision = Some(revision);
                self.frame_completed_at = Some(Instant::now());
                self.render_error = None;
            }
            Err(error) => {
                self.render_error = Some(error);
            }
        }
        true
    }

    fn publish(&mut self) {
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
            camera: self.host.camera,
            frame: self.frame.clone(),
            frame_tick: self.frame_tick,
            frame_revision: self.frame_revision,
            frame_completed_at: self.frame_completed_at,
            presentation_revision: self.host.presentation_revision(),
            device: self.host.device.clone(),
            world_kind: self.host.session.world_kind(),
            renderer_performance: self.host.renderer_performance(),
            last_step_ms: self.host.last_step_ms,
            last_render_ms: self.host.last_render_ms,
            backlog_seconds: self.host.session.pending_time(),
            playing: self.playing,
            fault: self.fault.clone(),
            preparation_error: self.preparation_error.clone(),
            render_error: self.render_error.clone(),
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
            activity: self.host.preparation_activity().into_iter().collect(),
        });
        if let Ok(mut presentation) = self.shared.presentation.lock() {
            *presentation = Some(snapshot);
        }
        if let Some(ctx) = &self.repaint {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_window_drain_signals_all_owners_then_waits_for_actual_cleanup() {
        // A local registry exercises the real join route without shutting down
        // other actor tests that may be running concurrently in this process.
        let make_shared = || {
            Arc::new(Shared {
                mailbox: Mutex::new(Mailbox::default()),
                presentation: Mutex::new(None),
                shutdown: AtomicBool::new(false),
                audio_retired: Arc::new(AtomicBool::new(false)),
                alive: AtomicBool::new(true),
            })
        };
        let first = make_shared();
        let second = make_shared();
        let cleaned = [
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        ];
        let mut registrations = vec![];
        for (actor_id, shared, peer, cleaned) in [
            (1, first.clone(), second.clone(), cleaned[0].clone()),
            (2, second.clone(), first.clone(), cleaned[1].clone()),
        ] {
            let owner_shared = shared.clone();
            let owner = thread::spawn(move || {
                while !owner_shared.shutdown.load(Ordering::Acquire)
                    || !peer.shutdown.load(Ordering::Acquire)
                {
                    thread::park_timeout(Duration::from_millis(1));
                }
                // Represent cleanup after a current transaction has returned;
                // the caller must not merely signal and discard its handle.
                thread::sleep(Duration::from_millis(10));
                cleaned.store(true, Ordering::Release);
                owner_shared.alive.store(false, Ordering::Release);
            });
            registrations.push(RegisteredWorker {
                actor_id,
                shared,
                thread: owner.thread().clone(),
                owner,
            });
        }
        finish_registered(registrations).unwrap();
        assert!(
            cleaned
                .iter()
                .all(|cleaned| cleaned.load(Ordering::Acquire))
        );
        for shared in [first, second] {
            assert!(shared.audio_retired.load(Ordering::Acquire));
            assert!(shared.shutdown.load(Ordering::Acquire));
            assert!(!shared.alive.load(Ordering::Acquire));
        }
    }
}
