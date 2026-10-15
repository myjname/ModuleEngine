//! Authored scenes and staged execution graphs. Play never writes authored data.
use crate::player::update_render_diagnostic;
use crate::{
    activity::{ActivitySnapshot, BuildProgress},
    canvas::{self, CanvasAction, CanvasEdge, CanvasNode, CanvasPort, CanvasState},
    game_host::{self, GameHost},
    game_worker::{CustomApplyMode, GameSnapshot, GameWorker, WorkerCommand},
    player,
};
use eframe::egui::{self, Color32, Vec2};
#[cfg(test)]
use module_engine_arpg::{ArpgInput, ExecutionGraphExt, GameInputExt};
use module_engine_core::*;
use module_engine_script::{CompiledScript, JobLease, ScriptCompiler, ScriptKind};
#[cfg(test)]
use module_engine_two_arenas::ProjectExt;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};
struct HostPreparation {
    document: GraphDocument,
    context: PreparationContext,
    receiver: mpsc::Receiver<Result<GameHost, String>>,
    progress: BuildProgress,
    dependencies: game_host::ShaderDependencyWatch,
    _job: Option<JobLease>,
}
impl HostPreparation {
    fn retire(mut self) {
        self._job.take();
        retire_in_background(self);
    }
}
impl Drop for HostPreparation {
    fn drop(&mut self) {
        self._job.take();
        let receiver = std::mem::replace(&mut self.receiver, mpsc::channel().1);
        retire_in_background(receiver);
    }
}
fn retire_in_background(value: impl Send + 'static) {
    let _ = module_engine_script::spawn_job(move |_| drop(value));
}
#[derive(Clone, PartialEq, Eq)]
struct PreparationContext {
    root: PathBuf,
    backend_root: PathBuf,
    compile_root: Option<PathBuf>,
    kind: WorldKind,
}
/// Requested paths/world paired with their canonical preparation context.
type CachedPreparationContext = (
    PathBuf,
    Option<PathBuf>,
    Option<PathBuf>,
    WorldKind,
    PreparationContext,
);
enum DocumentCandidate<'a> {
    Borrowed(&'a GraphDocument),
    Shared(Arc<GraphDocument>),
}
impl std::ops::Deref for DocumentCandidate<'_> {
    type Target = GraphDocument;
    fn deref(&self) -> &GraphDocument {
        match self {
            Self::Borrowed(document) => document,
            Self::Shared(document) => document,
        }
    }
}
impl DocumentCandidate<'_> {
    fn as_ref(&self) -> &GraphDocument {
        self
    }
    fn into_owned(self) -> GraphDocument {
        match self {
            Self::Borrowed(document) => document.clone(),
            Self::Shared(document) => Arc::unwrap_or_clone(document),
        }
    }
}
#[derive(Default, Clone, Copy, Debug, serde::Serialize)]
pub struct WorkbenchPerformance {
    pub context_resolutions: u64,
    pub normalized_document_clones: u64,
    pub game_render_calls: u64,
    pub game_frame_reuses: u64,
}
#[derive(Clone, Copy, PartialEq)]
struct GameFrameKey {
    revision: u64,
    camera: Camera,
    size: [usize; 2],
    tick: u64,
    time: f64,
}
#[derive(Default)]
struct GameFrameCache {
    key: Option<GameFrameKey>,
    frame: Option<u64>,
    time: Option<f64>,
    next_play_frame: Option<f64>,
}

enum WorkerRequest {
    Document(Arc<GraphDocument>),
    Custom(String),
    Restart,
}
impl GameFrameCache {
    fn should_render(&mut self, key: GameFrameKey, frame: u64, now: f64, playing: bool) -> bool {
        let age = self.time.map_or(f64::INFINITY, |previous| {
            if now < previous {
                f64::INFINITY
            } else {
                now - previous
            }
        });
        if self.frame == Some(frame)
            || (playing
                && self.next_play_frame.is_some_and(|due| now + 0.001 < due)
                && age.is_finite())
            || (self.key == Some(key) && age < 0.25)
        {
            return false;
        }
        self.key = Some(key);
        self.frame = Some(frame);
        self.time = Some(now);
        self.next_play_frame = if playing {
            let due = self
                .next_play_frame
                .filter(|due| (now - due).abs() < 0.25)
                .unwrap_or(now);
            // Advance the 60 Hz clock, not the last submission timestamp. Small
            // vsync jitter must not turn a 60 Hz GUI into a 30 Hz Game viewport.
            let skipped = ((now - due) * 60.).floor().max(0.);
            Some(due + (skipped + 1.) / 60.)
        } else {
            None
        };
        true
    }
}

pub struct GameWorkbench {
    pub host: Option<GameHost>,
    // Accepted authored baseline is separate from GameHost's live graphics
    // document: sync_document intentionally does not replace GameSession.
    game_document: Option<GraphDocument>,
    pub preview_host: Option<GameHost>,
    preview_document: Option<GraphDocument>,
    pub scene: String,
    pub selected: Option<u64>,
    pub input: GameInput,
    texture: Option<egui::TextureHandle>,
    pub error: Option<String>,
    render_error: Option<String>,
    field_key: Option<(String, u64)>,
    component_drafts: BTreeMap<String, Value>,
    advanced: String,
    template_key: Option<String>,
    template_draft: Option<Prefab>,
    template_name: String,
    scene_name: String,
    systems: CanvasState,
    positions: BTreeMap<String, [f32; 2]>,
    positions_root: Option<PathBuf>,
    graph_draft: Option<ExecutionGraph>,
    graph_base: Option<ExecutionGraph>,
    graph_text: String,
    graph_text_dirty: bool,
    params: String,
    params_id: Option<String>,
    pending: Option<(String, mpsc::Receiver<Result<CompiledScript, String>>)>,
    pending_job: Option<JobLease>,
    pending_worker_epoch: Option<(u64, u64)>,
    pub rejected: Option<(String, CompiledScript)>,
    migration: String,
    source_selected: String,
    compile_root: Option<PathBuf>,
    backend_root: Option<PathBuf>,
    world_kind: WorldKind,
    play_session: bool,
    deferred_preparation: bool,
    // Interactive execution has one persistent owner. Deferred preparation is
    // intentionally independent so deterministic headless callers keep the
    // synchronous GameHost contract.
    background_game: bool,
    worker: Option<GameWorker>,
    worker_snapshot: Option<Arc<GameSnapshot>>,
    worker_commands: VecDeque<WorkerCommand>,
    worker_inputs: VecDeque<GameInput>,
    worker_requests: BTreeMap<u64, WorkerRequest>,
    worker_request: u64,
    worker_playing: Option<bool>,
    worker_viewport: Option<[usize; 2]>,
    worker_view_visible: bool,
    worker_frame: Option<Arc<Frame>>,
    worker_rejected_document: Option<Arc<GraphDocument>>,
    worker_deferred_document: Option<(Arc<GraphDocument>, bool)>,
    repaint: Option<egui::Context>,
    game_preparation: Option<HostPreparation>,
    preview_preparation: Option<HostPreparation>,
    game_activity: Option<BuildProgress>,
    preview_activity: Option<BuildProgress>,
    custom_activity: Option<BuildProgress>,
    custom_errors: BTreeMap<String, String>,
    restart_error: Option<String>,
    restart_recovered: bool,
    rejected_game: Option<(GraphDocument, PreparationContext, String)>,
    rejected_preview: Option<(GraphDocument, PreparationContext, String)>,
    rejected_game_dependencies: Option<game_host::ShaderDependencyWatch>,
    rejected_preview_dependencies: Option<game_host::ShaderDependencyWatch>,
    game_context: Option<PreparationContext>,
    preview_context: Option<PreparationContext>,
    accepted_game_context: Option<PreparationContext>,
    accepted_preview_context: Option<PreparationContext>,
    last_preparation_error: Option<String>,
    resolved_context: Option<CachedPreparationContext>,
    performance: WorkbenchPerformance,
    game_frame: GameFrameCache,
    normalized_preview: Option<(GraphDocument, String, Arc<GraphDocument>)>,
}
impl Default for GameWorkbench {
    fn default() -> Self {
        Self {
            host: None,
            game_document: None,
            preview_host: None,
            preview_document: None,
            scene: String::new(),
            selected: Some(1),
            input: GameInput::default(),
            texture: None,
            error: None,
            render_error: None,
            field_key: None,
            component_drafts: BTreeMap::new(),
            advanced: String::new(),
            template_key: None,
            template_draft: None,
            template_name: String::new(),
            scene_name: String::new(),
            systems: {
                let mut canvas = CanvasState::new("game-systems");
                canvas.zoom = 0.64;
                canvas
            },
            positions: BTreeMap::new(),
            positions_root: None,
            graph_draft: None,
            graph_base: None,
            graph_text: String::new(),
            graph_text_dirty: false,
            params: String::new(),
            params_id: None,
            pending: None,
            pending_job: None,
            pending_worker_epoch: None,
            rejected: None,
            migration: "null".into(),
            source_selected: String::new(),
            compile_root: None,
            backend_root: None,
            world_kind: WorldKind::Objects,
            play_session: false,
            deferred_preparation: false,
            background_game: false,
            worker: None,
            worker_snapshot: None,
            worker_commands: VecDeque::new(),
            worker_inputs: VecDeque::new(),
            worker_requests: BTreeMap::new(),
            worker_request: 0,
            worker_playing: None,
            worker_viewport: None,
            worker_view_visible: false,
            worker_frame: None,
            worker_rejected_document: None,
            worker_deferred_document: None,
            repaint: None,
            game_preparation: None,
            preview_preparation: None,
            game_activity: None,
            preview_activity: None,
            custom_activity: None,
            custom_errors: BTreeMap::new(),
            restart_error: None,
            restart_recovered: false,
            rejected_game: None,
            rejected_preview: None,
            rejected_game_dependencies: None,
            rejected_preview_dependencies: None,
            game_context: None,
            preview_context: None,
            accepted_game_context: None,
            accepted_preview_context: None,
            last_preparation_error: None,
            resolved_context: None,
            performance: WorkbenchPerformance::default(),
            game_frame: GameFrameCache::default(),
            normalized_preview: None,
        }
    }
}
impl Drop for GameWorkbench {
    fn drop(&mut self) {
        self.cancel_pending_custom();
        for slot in [&mut self.game_preparation, &mut self.preview_preparation] {
            if let Some(job) = slot.take() {
                job.progress.succeed("Редактор закрыл запрос подготовки");
                job.retire();
            }
        }
        if self.deferred_preparation {
            for slot in [&mut self.host, &mut self.preview_host] {
                if let Some(mut host) = slot.take() {
                    host.services.stop();
                    retire_in_background(host);
                }
            }
        }
    }
}
impl GameWorkbench {
    fn cancel_pending_custom(&mut self) {
        self.pending_job.take();
        if let Some(pending) = self.pending.take() {
            retire_in_background(pending);
        }
    }
    pub fn performance(&self) -> WorkbenchPerformance {
        self.performance
    }
    /// Most views use the authored entry scene and can borrow the document.
    /// Clone only when Scene intentionally previews another authored scene.
    fn preview_candidate<'a>(&mut self, document: &'a GraphDocument) -> DocumentCandidate<'a> {
        if document.game.as_ref().is_none_or(|game| {
            game.entry_scene == self.scene || !game.scenes.contains_key(&self.scene)
        }) {
            return DocumentCandidate::Borrowed(document);
        }
        if let Some((previous, scene, normalized)) = &self.normalized_preview
            && previous == document
            && scene == &self.scene
        {
            return DocumentCandidate::Shared(normalized.clone());
        }
        self.performance.normalized_document_clones += 2;
        let mut candidate = document.clone();
        candidate.game.as_mut().unwrap().entry_scene = self.scene.clone();
        let candidate = Arc::new(candidate);
        self.normalized_preview = Some((document.clone(), self.scene.clone(), candidate.clone()));
        DocumentCandidate::Shared(candidate)
    }
    pub fn set_deferred_preparation(&mut self, enabled: bool) {
        self.deferred_preparation = enabled;
    }
    /// Native Editor execution is asynchronous; headless/CLI callers opt in
    /// explicitly rather than silently changing their deterministic API.
    pub fn set_background_game(&mut self, enabled: bool, context: Option<&egui::Context>) {
        self.background_game = enabled;
        if enabled {
            self.deferred_preparation = true;
        }
        if let Some(context) = context {
            self.repaint = Some(context.clone());
        }
        if enabled
            && self.worker.is_none()
            && let Some(host) = self.host.take()
        {
            self.install_worker(host);
        }
    }
    fn install_worker(&mut self, host: GameHost) {
        self.clear_worker();
        if self.accepted_game_context.is_none() {
            self.accepted_game_context = Some(PreparationContext {
                root: host.root.clone(),
                backend_root: host.backend_root.clone(),
                compile_root: self.compile_root.clone(),
                kind: host.session.world_kind(),
            });
        }
        self.worker = Some(GameWorker::spawn(
            host,
            self.repaint.clone().map(|ctx| {
                std::sync::Arc::new(move || ctx.request_repaint())
                    as std::sync::Arc<dyn Fn() + Send + Sync>
            }),
        ));
    }
    fn clear_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
        self.worker_snapshot = None;
        self.worker_commands.clear();
        self.worker_inputs.clear();
        self.worker_requests.clear();
        self.worker_playing = None;
        self.worker_viewport = None;
        self.worker_view_visible = false;
        self.worker_frame = None;
        self.worker_rejected_document = None;
        self.worker_deferred_document = None;
    }
    pub fn has_game_host(&self) -> bool {
        self.host.is_some() || self.worker.is_some()
    }
    pub fn game_snapshot(&self) -> Option<&Arc<GameSnapshot>> {
        self.worker_snapshot.as_ref()
    }
    pub fn begin_ui_frame(&mut self) {
        self.worker_view_visible = false;
    }
    pub fn finish_ui_frame(&mut self) {
        if self.worker.is_some()
            && !self.worker_view_visible
            && self.worker_viewport != Some([0, 0])
        {
            self.worker_viewport = Some([0, 0]);
            self.queue_worker_command(WorkerCommand::SetViewport {
                width: 0,
                height: 0,
            });
        }
    }
    /// Mirrors a temporarily skipped UI simulation step without waiting for
    /// the owner. Resume is requested by the next normal tick(playing=true).
    pub fn suspend_game_execution(&mut self) {
        self.input = GameInput::default();
        self.worker_inputs.clear();
        if self.worker.is_some() && self.worker_playing != Some(false) {
            self.worker_playing = Some(false);
            self.queue_worker_command(WorkerCommand::SetPlaying(false));
        }
    }
    pub fn game_world_kind(&self) -> Option<WorldKind> {
        self.worker_snapshot
            .as_ref()
            .map(|snapshot| snapshot.world_kind)
            .or_else(|| self.host.as_ref().map(|host| host.session.world_kind()))
    }
    fn next_worker_request(&mut self, request: WorkerRequest) -> u64 {
        self.worker_request = self.worker_request.wrapping_add(1);
        self.worker_requests.insert(self.worker_request, request);
        self.worker_request
    }
    fn queue_worker_command(&mut self, command: WorkerCommand) {
        // Latest-value controls occupy at most one unsent queue slot each.
        self.worker_commands.retain(|queued| {
            !matches!(
                (&command, queued),
                (WorkerCommand::SetPlaying(_), WorkerCommand::SetPlaying(_))
                    | (
                        WorkerCommand::SetViewport { .. },
                        WorkerCommand::SetViewport { .. }
                    )
                    | (
                        WorkerCommand::InvalidateContent,
                        WorkerCommand::InvalidateContent
                    )
            )
        });
        self.worker_commands.push_back(command);
        self.flush_worker_commands();
    }
    fn flush_worker_commands(&mut self) {
        let Some(worker) = &self.worker else { return };
        while let Some(command) = self.worker_commands.pop_front() {
            if let Err(command) = worker.try_command(command) {
                self.worker_commands.push_front(command);
                break;
            }
        }
        while let Some(input) = self.worker_inputs.pop_front() {
            if let Err(input) = worker.submit_input(input) {
                self.worker_inputs.push_front(input);
                break;
            }
        }
    }
    fn queue_worker_document(&mut self, document: &GraphDocument, authoring: bool) {
        if let Some(inflight) = self.worker_requests.values().find_map(|request| {
            if let WorkerRequest::Document(candidate) = request {
                Some(candidate)
            } else {
                None
            }
        }) {
            self.worker_deferred_document =
                (inflight.as_ref() != document).then(|| (Arc::new(document.clone()), authoring));
            return;
        }
        if self.worker_rejected_document.as_deref() == Some(document) {
            return;
        }
        let document = Arc::new(document.clone());
        let request = self.next_worker_request(WorkerRequest::Document(document.clone()));
        self.queue_worker_command(WorkerCommand::UpdateDocument {
            document,
            authoring,
            request,
        });
    }
    /// Only tiny owned snapshots/replies cross this boundary. In particular this
    /// never locks GameHost, Custom slots, waits for a tick or reads GPU state.
    fn poll_worker(&mut self) {
        self.flush_worker_commands();
        let Some(worker) = &mut self.worker else {
            return;
        };
        let Some(snapshot) = worker.latest() else {
            return;
        };
        if self.worker_snapshot.as_ref().is_some_and(|old| {
            old.actor_id == snapshot.actor_id && old.sequence == snapshot.sequence
        }) {
            return;
        }
        if self.worker_snapshot.as_ref().is_some_and(|previous| {
            previous.actor_id == snapshot.actor_id
                && (previous.epoch != snapshot.epoch || previous.fault.is_some())
        }) && snapshot.fault.is_none()
        {
            let _ = self.note_restart_result(Ok(()));
        }
        for reply in &snapshot.results {
            let Some(request) = self.worker_requests.remove(&reply.request) else {
                continue;
            };
            match request {
                WorkerRequest::Document(document) => match &reply.result {
                    Ok(()) => {
                        self.game_document = Some((*document).clone());
                        self.worker_rejected_document = None;
                    }
                    Err(error) => {
                        // A newer queued document owns the visible outcome.
                        if self.worker_deferred_document.is_none()
                            && !self
                                .worker_requests
                                .values()
                                .any(|request| matches!(request, WorkerRequest::Document(_)))
                        {
                            self.worker_rejected_document = Some(document);
                            self.error = Some(error.clone());
                            self.last_preparation_error = Some(error.clone());
                        }
                    }
                },
                WorkerRequest::Restart => {
                    let _ = self.note_restart_result(reply.result.clone());
                }
                WorkerRequest::Custom(id) => match &reply.result {
                    Ok(()) => {
                        let previous = self.custom_errors.remove(&id);
                        if previous.is_some() && self.error == previous {
                            self.error = None;
                        }
                        if let Some(progress) = &self.custom_activity {
                            progress.succeed("Новая версия Game Rust применена");
                        }
                    }
                    Err(error) => {
                        self.custom_errors.insert(id, error.clone());
                        self.error = Some(error.clone());
                        if let Some(progress) = &self.custom_activity {
                            progress.fail(error.clone());
                        }
                    }
                },
            }
        }
        // Deferred GPU preparation can recover an external include without a
        // new UI command. Its accepted snapshot is the authoritative result.
        if let Some(candidate) = &self.worker_rejected_document
            && snapshot.accepted_document.execution_eq(candidate)
            && snapshot.preparation_error.is_none()
        {
            self.game_document = Some((**candidate).clone());
            self.worker_rejected_document = None;
            if self.error == self.last_preparation_error {
                self.error = None;
            }
            self.last_preparation_error = None;
        }
        update_render_diagnostic(
            &mut self.error,
            &mut self.render_error,
            snapshot.render_error.clone().map_or(Ok(()), Err),
        );
        self.worker_snapshot = Some(snapshot);
        if !self
            .worker_requests
            .values()
            .any(|request| matches!(request, WorkerRequest::Document(_)))
            && let Some((document, authoring)) = self.worker_deferred_document.take()
        {
            self.queue_worker_document(&document, authoring);
        }
    }
    pub fn activity_snapshots(&self) -> Vec<ActivitySnapshot> {
        let mut activities = [
            &self.game_activity,
            &self.preview_activity,
            &self.custom_activity,
        ]
        .into_iter()
        .filter_map(|activity| activity.as_ref().map(BuildProgress::snapshot))
        .collect::<Vec<_>>();
        for host in [&self.host, &self.preview_host].into_iter().flatten() {
            if let Some(activity) = host.preparation_activity() {
                activities.push(activity);
            }
        }
        if let Some(snapshot) = &self.worker_snapshot {
            activities.extend(snapshot.activity.iter().cloned());
        }
        activities
    }
    pub fn has_pending_preparation(&self) -> bool {
        self.game_preparation.is_some()
            || self.preview_preparation.is_some()
            || self
                .worker_requests
                .values()
                .any(|request| matches!(request, WorkerRequest::Document(_)))
            || self.worker_snapshot.as_ref().is_some_and(|snapshot| {
                snapshot
                    .activity
                    .iter()
                    .any(|activity| activity.state == crate::activity::ActivityState::Running)
            })
            || self
                .host
                .as_ref()
                .is_some_and(GameHost::has_pending_preparation)
            || self
                .preview_host
                .as_ref()
                .is_some_and(GameHost::has_pending_preparation)
    }
    /// Called once per UI frame, including when the corresponding pane is hidden.
    pub fn poll_preparation(&mut self, document: &GraphDocument) -> Result<(), String> {
        self.poll_worker();
        if !self.deferred_preparation {
            return Ok(());
        }
        let kind = if self.game_preparation.is_some() || self.preview_preparation.is_some() {
            game_host::world_kind(document)?
        } else {
            // No initial host result needs a kind check. Incomplete Engine
            // drafts remain editable while a previous host stays installed.
            self.world_kind
        };
        let mut first_error = self.poll_host_preparation(document, kind, false).err();
        if self
            .rejected_game
            .as_ref()
            .is_some_and(|(failed, _, _)| !failed.execution_eq(document))
        {
            self.rejected_game = None;
            self.rejected_game_dependencies = None;
            self.game_activity = None;
        }
        // Hidden, unopened Scene needs neither normalization nor a source clone.
        let preview_active = self.preview_preparation.is_some()
            || self.preview_host.is_some()
            || self.rejected_preview.is_some();
        let preview = if preview_active {
            self.preview_candidate(document)
        } else {
            DocumentCandidate::Borrowed(document)
        };
        if preview_active && preview.game.is_some() {
            if self
                .rejected_preview
                .as_ref()
                .is_some_and(|(failed, _, _)| !failed.execution_eq(&preview))
            {
                self.rejected_preview = None;
                self.rejected_preview_dependencies = None;
                self.preview_activity = None;
            }
            if let Err(error) = self.poll_host_preparation(&preview, kind, true) {
                first_error.get_or_insert(error);
            }
        }
        if self.game_preparation.is_none()
            && let Some(host) = &mut self.host
            && let Err(error) = host.poll_preparation(document)
        {
            first_error.get_or_insert(error);
        }
        if self.preview_preparation.is_none()
            && let Some(host) = &mut self.preview_host
            && let Err(error) = host.poll_preparation(&preview)
        {
            first_error.get_or_insert(error);
        }
        if let Some(error) = first_error {
            self.error = Some(error.clone());
            self.last_preparation_error = Some(error.clone());
            Err(error)
        } else {
            let failed = [&self.game_activity, &self.preview_activity]
                .into_iter()
                .filter_map(|activity| activity.as_ref())
                .any(|activity| {
                    activity.snapshot().state == crate::activity::ActivityState::Failed
                })
                || [&self.host, &self.preview_host]
                    .into_iter()
                    .flatten()
                    .filter_map(GameHost::preparation_activity)
                    .any(|activity| activity.state == crate::activity::ActivityState::Failed)
                || self
                    .worker_snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.preparation_error.is_some());
            if !failed
                && let Some(error) = self.last_preparation_error.take()
                && self.error.as_ref() == Some(&error)
            {
                self.error = None;
            }
            Ok(())
        }
    }
    pub fn sync_graphics(&mut self, document: &GraphDocument) -> Result<(), String> {
        if self.worker.is_some() {
            if self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_alive())
            {
                return Err(
                    "Игровой исполнитель завершился; нажмите Stop и запустите Play повторно".into(),
                );
            }
            if self.game_preparation.is_none()
                && (self
                    .worker_requests
                    .values()
                    .any(|request| matches!(request, WorkerRequest::Document(_)))
                    || self
                        .game_document
                        .as_ref()
                        .is_some_and(|accepted| accepted != document))
                && (self.play_session
                    || self.game_document.as_ref().is_some_and(|accepted| {
                        accepted.execution_eq(document)
                            || game_host::graphics_only_change(accepted, document)
                    }))
            {
                self.queue_worker_document(document, false);
            }
            return Ok(());
        }
        if self.deferred_preparation {
            if self.game_preparation.is_some() {
                return Ok(());
            }
            // A graphics sync must never acknowledge a newly authored scene
            // before ensure has installed its separate source-world copy.
            if !self.play_session
                && self.game_document.as_ref().is_none_or(|accepted| {
                    !accepted.execution_eq(document)
                        && !game_host::graphics_only_change(accepted, document)
                })
            {
                return Ok(());
            }
            if let Some(host) = &mut self.host {
                host.sync_document_deferred(document)?;
            }
        } else if let Some(host) = &mut self.host {
            host.sync_document(document)?;
        }
        Ok(())
    }
    fn poll_host_preparation(
        &mut self,
        document: &GraphDocument,
        kind: WorldKind,
        preview: bool,
    ) -> Result<(), String> {
        let slot = if preview {
            &mut self.preview_preparation
        } else {
            &mut self.game_preparation
        };
        let Some(job) = slot.as_ref() else {
            return Ok(());
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return Ok(()),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Поток подготовки Game / Scene завершился без результата".into())
            }
        };
        let job = slot.take().unwrap();
        let current_context = if preview {
            &self.preview_context
        } else {
            &self.game_context
        };
        if kind != job.context.kind
            || current_context.as_ref() != Some(&job.context)
            || !job.document.execution_eq(document)
        {
            job.progress
                .succeed("Запрос заменён новой конфигурацией; предыдущий результат сохранён");
            retire_in_background(result);
            return Ok(());
        }
        match result {
            Ok(mut host) => {
                // Metadata may have moved while its execution stayed the same.
                if let Err(error) = host.sync_document(document) {
                    job.progress.fail(error.clone());
                    retire_in_background(host);
                    return Err(error);
                }
                host.services.set_enabled(!preview);
                if !preview {
                    Self::frame_game_start(&mut host);
                }
                let old = if preview {
                    self.preview_document = Some(document.clone());
                    self.accepted_preview_context = Some(job.context.clone());
                    self.rejected_preview = None;
                    self.rejected_preview_dependencies = None;
                    self.preview_host.replace(host)
                } else {
                    self.game_document = Some(document.clone());
                    self.accepted_game_context = Some(job.context.clone());
                    self.rejected_game = None;
                    self.rejected_game_dependencies = None;
                    if self.background_game {
                        self.install_worker(host);
                        self.host.take()
                    } else {
                        self.host.replace(host)
                    }
                };
                if let Some(mut old) = old {
                    old.services.stop();
                    retire_in_background(old);
                }
                job.progress.succeed(if preview {
                    "Scene подготовлена"
                } else {
                    "Game подготовлен"
                });
                self.error = None;
                Ok(())
            }
            Err(error) => {
                job.progress.fail(error.clone());
                let dependencies = job.dependencies.clone();
                let failed = Some((document.clone(), job.context.clone(), error.clone()));
                if preview {
                    self.rejected_preview = failed;
                    self.rejected_preview_dependencies = Some(dependencies);
                } else {
                    self.rejected_game = failed;
                    self.rejected_game_dependencies = Some(dependencies);
                }
                self.error = Some(error.clone());
                Err(error)
            }
        }
    }
    fn request_host_preparation(
        &mut self,
        document: &GraphDocument,
        root: &Path,
        compile_root: Option<&Path>,
        kind: WorldKind,
        preview: bool,
    ) -> Result<(), String> {
        let context = self.preparation_context(root, compile_root, kind)?;
        let failed = if preview {
            &self.rejected_preview
        } else {
            &self.rejected_game
        };
        if let Some((failed, failed_context, error)) = failed
            && failed_context == &context
            && failed.execution_eq(document)
            && !(if preview {
                &mut self.rejected_preview_dependencies
            } else {
                &mut self.rejected_game_dependencies
            })
            .as_mut()
            .is_some_and(|watch| watch.changed(document, &context.backend_root))
        {
            return Err(error.clone());
        }
        let slot = if preview {
            &mut self.preview_preparation
        } else {
            &mut self.game_preparation
        };
        if slot.is_some() {
            return Ok(());
        }
        if let Some(host) = if preview {
            &mut self.preview_host
        } else {
            &mut self.host
        } {
            host.cancel_preparation();
        }
        let root = context.root.clone();
        let compile = context.compile_root.clone();
        let backend = context.backend_root.clone();
        let candidate = document.clone();
        let worker_document = candidate.clone();
        let progress = BuildProgress::new(if preview {
            "Подготовка Scene"
        } else {
            "Подготовка Game"
        });
        let worker_progress = progress.clone();
        let dependencies = game_host::ShaderDependencyWatch::new(document, &context.backend_root);
        let (receiver, job) = module_engine_script::spawn_job(move |cancelled| {
            crate::game_host::new_at_with_progress_cancellable(
                worker_document,
                &root,
                &backend,
                compile.as_deref(),
                kind,
                Some(&worker_progress),
                cancelled,
            )
            .map(|mut host| {
                host.services.set_enabled(false);
                host
            })
        });
        *slot = Some(HostPreparation {
            document: candidate,
            context,
            receiver,
            progress: progress.clone(),
            dependencies,
            _job: Some(job),
        });
        if preview {
            self.preview_activity = Some(progress);
            self.rejected_preview = None;
            self.rejected_preview_dependencies = None;
        } else {
            self.game_activity = Some(progress);
            self.rejected_game = None;
            self.rejected_game_dependencies = None;
            self.suspend_game_execution();
        }
        Ok(())
    }
    fn ensure_deferred(
        &mut self,
        document: &GraphDocument,
        root: &Path,
        compile_root: Option<&Path>,
        kind: WorldKind,
        preview: bool,
    ) -> Result<(), String> {
        let context = self.preparation_context(root, compile_root, kind)?;
        if preview {
            self.preview_context = Some(context.clone());
        } else {
            self.game_context = Some(context.clone());
        }
        if preview {
            let game = document.game.as_ref().ok_or("No authored game project")?;
            self.normalize_selection(game);
        }
        let candidate = if preview {
            self.preview_candidate(document)
        } else {
            DocumentCandidate::Borrowed(document)
        };
        self.poll_host_preparation(&candidate, kind, preview)?;
        if if preview {
            self.preview_preparation.is_some()
        } else {
            self.game_preparation.is_some()
        } {
            return Ok(());
        }
        if !preview && self.worker.is_some() {
            if self.accepted_game_context.as_ref() != Some(&context) {
                return self.request_host_preparation(&candidate, root, compile_root, kind, false);
            }
            if self.game_document.as_ref() == Some(candidate.as_ref()) {
                if self
                    .worker_requests
                    .values()
                    .any(|request| matches!(request, WorkerRequest::Document(_)))
                    || self.worker_rejected_document.is_some()
                {
                    self.queue_worker_document(&candidate, false);
                }
                return Ok(());
            }
            let structure_changed = self.game_document.as_ref().is_some_and(|previous| {
                !game_host::engine_structure_eq(&previous.root, &candidate.root)
                    || previous.game.as_ref().map(|game| &game.execution)
                        != candidate.game.as_ref().map(|game| &game.execution)
            });
            if structure_changed {
                return self.request_host_preparation(&candidate, root, compile_root, kind, false);
            }
            if !self.play_session {
                let authoring = self.game_document.as_ref().is_some_and(|previous| {
                    !previous.execution_eq(&candidate)
                        && !game_host::graphics_only_change(previous, &candidate)
                });
                self.queue_worker_document(&candidate, authoring);
            }
            return Ok(());
        }
        let baseline = if preview {
            &self.preview_document
        } else {
            &self.game_document
        };
        let host = if preview {
            &mut self.preview_host
        } else {
            &mut self.host
        };
        if let Some(host) = host {
            let accepted_context = if preview {
                &mut self.accepted_preview_context
            } else {
                &mut self.accepted_game_context
            };
            if accepted_context.is_none()
                && host.root == context.root
                && host.backend_root == context.backend_root
            {
                *accepted_context = Some(context.clone());
            }
            if accepted_context.as_ref() != Some(&context) {
                return self.request_host_preparation(
                    &candidate,
                    root,
                    compile_root,
                    kind,
                    preview,
                );
            }
            if baseline.as_ref() == Some(candidate.as_ref()) && host.session.world_kind() == kind {
                return Ok(());
            }
            if !preview && self.play_session {
                if baseline.as_ref().is_some_and(|previous| {
                    host.session.world_kind() != kind
                        || !game_host::engine_structure_eq(&previous.root, &candidate.root)
                        || previous.game.as_ref().map(|game| &game.execution)
                            != candidate.game.as_ref().map(|game| &game.execution)
                }) {
                    return self.request_host_preparation(
                        &candidate,
                        root,
                        compile_root,
                        kind,
                        preview,
                    );
                }
                return Ok(());
            }
            host.poll_preparation(&candidate)?;
            if host.session.world_kind() == kind
                && let Some(previous) = baseline
            {
                if previous == candidate.as_ref() {
                    return Ok(());
                }
                if previous.execution_eq(&candidate) {
                    host.sync_document(&candidate)?;
                } else if game_host::graphics_only_change(previous, &candidate) {
                    host.sync_document_deferred(&candidate)?;
                    if !host.accepted_document().execution_eq(&candidate) {
                        return Ok(());
                    }
                } else {
                    let structure_changed = !previous.root.execution_eq(&candidate.root)
                        || previous.shader_sources != candidate.shader_sources
                        || previous.game.as_ref().map(|game| &game.execution)
                            != candidate.game.as_ref().map(|game| &game.execution);
                    if structure_changed {
                        return self.request_host_preparation(
                            &candidate,
                            root,
                            compile_root,
                            kind,
                            preview,
                        );
                    }
                    host.refresh_author_preview(&candidate)?;
                    if !preview {
                        Self::frame_game_start(host);
                    }
                }
                if preview {
                    self.preview_document = Some(candidate.into_owned());
                } else {
                    self.game_document = Some(candidate.into_owned());
                }
                return Ok(());
            }
        }
        self.request_host_preparation(&candidate, root, compile_root, kind, preview)
    }
    fn preparation_context(
        &mut self,
        root: &Path,
        compile: Option<&Path>,
        kind: WorldKind,
    ) -> Result<PreparationContext, String> {
        if let Some((path, compiler, backend, world, context)) = &self.resolved_context
            && path == root
            && compiler.as_deref() == compile
            && backend == &self.backend_root
            && *world == kind
        {
            return Ok(context.clone());
        }
        self.performance.context_resolutions += 1;
        let requested_root = root.to_path_buf();
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let backend_root = self
            .backend_root
            .clone()
            .map(Ok)
            .unwrap_or_else(|| game_host::backend_root_for(&root, compile))?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let context = PreparationContext {
            root,
            backend_root,
            compile_root: compile.map(Path::to_path_buf),
            kind,
        };
        self.resolved_context = Some((
            requested_root,
            compile.map(Path::to_path_buf),
            self.backend_root.clone(),
            kind,
            context.clone(),
        ));
        Ok(context)
    }
    /// The installed Engine supplies GPU tools; author assets live beside the JSON.
    pub fn set_engine_context(&mut self, compile_root: Option<&Path>, backend_root: &Path) {
        if self.compile_root.as_deref() != compile_root
            || self.backend_root.as_deref() != Some(backend_root)
        {
            for slot in [&mut self.game_preparation, &mut self.preview_preparation] {
                if let Some(job) = slot.take() {
                    job.progress
                        .succeed("Контекст Engine изменился; запрос заменён");
                    job.retire();
                }
            }
            for host in [&mut self.host, &mut self.preview_host]
                .into_iter()
                .flatten()
            {
                host.cancel_preparation();
            }
            self.game_context = None;
            self.preview_context = None;
            self.rejected_game = None;
            self.rejected_preview = None;
            self.rejected_game_dependencies = None;
            self.rejected_preview_dependencies = None;
            self.resolved_context = None;
        }
        self.compile_root = compile_root.map(Path::to_path_buf);
        self.backend_root = Some(backend_root.to_path_buf());
    }
    /// Pending execution edits stay local until explicitly applied.
    pub fn has_graph_draft(&self) -> bool {
        self.graph_text_dirty
            || self
                .graph_draft
                .as_ref()
                .zip(self.graph_base.as_ref())
                .is_some_and(|(draft, base)| draft != base)
    }
    pub fn discard_graph_draft(&mut self) {
        self.graph_draft = None;
        self.graph_base = None;
        self.graph_text_dirty = false;
        self.graph_text.clear();
        self.params_id = None;
    }
    pub fn invalidate_assets(&mut self) {
        self.invalidate_preview_assets();
        if !self.play_session && self.worker.is_some() {
            self.queue_worker_command(WorkerCommand::InvalidateContent);
        }
        if !self.play_session
            && let Some(host) = &mut self.host
        {
            host.invalidate_content();
        }
    }
    pub fn stop(&mut self) {
        self.clear_worker();
        if let Some(job) = self.game_preparation.take() {
            job.progress.succeed("Подготовка отменена после Stop");
            job.retire();
        }
        if self.deferred_preparation {
            if let Some(mut host) = self.host.take() {
                host.services.stop();
                host.cancel_preparation();
                retire_in_background(host);
            }
        } else {
            self.host = None;
        }
        self.rejected_game = None;
        self.rejected_game_dependencies = None;
        self.game_context = None;
        self.accepted_game_context = None;
        self.game_document = None;
        self.input = GameInput::default();
        self.texture = None;
        self.game_frame = GameFrameCache::default();
        update_render_diagnostic(&mut self.error, &mut self.render_error, Ok(()));
        self.play_session = false;
        self.field_key = None;
        self.template_key = None;
        self.cancel_pending_custom();
        self.pending_worker_epoch = None;
        self.custom_errors.clear();
        self.restart_error = None;
        self.restart_recovered = false;
        if let Some(progress) = &self.custom_activity {
            progress.succeed("Компиляция отменена после Stop; версия не применена");
        }
        self.rejected = None;
    }
    pub fn ensure(
        &mut self,
        document: &GraphDocument,
        root: &Path,
        compile_root: Option<&Path>,
        kind: WorldKind,
    ) -> Result<(), String> {
        self.compile_root = compile_root.map(Path::to_path_buf);
        self.world_kind = kind;
        if self.deferred_preparation {
            let result = self.ensure_deferred(document, root, compile_root, kind, false);
            if let Err(error) = &result {
                self.last_preparation_error = Some(error.clone());
            }
            return result;
        }
        if let Some(host) = &mut self.host {
            // Pause retains the running copy. Only Stop returns to authoring.
            if self.play_session || self.game_document.as_ref() == Some(document) {
                return Ok(());
            }
            if host.session.world_kind() == kind
                && self
                    .game_document
                    .as_ref()
                    .is_some_and(|previous| previous.execution_eq(document))
            {
                host.sync_document(document)?;
                self.game_document = Some(document.clone());
                return Ok(());
            }
            let structure_changed = host.session.world_kind() != kind
                || self.game_document.as_ref().is_some_and(|previous| {
                    !previous.root.execution_eq(&document.root)
                        || previous.shader_sources != document.shader_sources
                        || previous.game.as_ref().map(|game| &game.execution)
                            != document.game.as_ref().map(|game| &game.execution)
                });
            if !structure_changed {
                match host.refresh_author_preview(document) {
                    Ok(()) => {
                        Self::frame_game_start(host);
                        self.game_document = Some(document.clone());
                        return Ok(());
                    }
                    Err(error) if error.contains("rebuild") => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let candidate = document.clone();
        if candidate.game.is_none() {
            return Err("No game project".into());
        }
        let backend = self
            .backend_root
            .clone()
            .map(Ok)
            .unwrap_or_else(|| game_host::backend_root_for(root, compile_root))?;
        let mut host =
            crate::game_host::new_at(candidate.clone(), root, &backend, compile_root, kind)?;
        Self::frame_game_start(&mut host);
        self.host = Some(host);
        self.game_document = Some(candidate);
        Ok(())
    }
    /// Follow the installed presentation target while retaining Game's orbit.
    fn frame_game_start(host: &mut GameHost) {
        if let Some(target) = host.frame_target() {
            host.camera.target = target;
        }
    }
    /// Compile edited source in the background; current instances keep running.
    pub fn request_custom(&mut self, id: &str, source: &str, compile_root: &Path) {
        if self.pending.is_some()
            || self
                .worker_requests
                .values()
                .any(|request| matches!(request, WorkerRequest::Custom(_)))
        {
            self.error = Some("A Custom compilation is already running".into());
            return;
        }
        self.rejected = None;
        self.pending_worker_epoch = self
            .worker_snapshot
            .as_ref()
            .map(|snapshot| (snapshot.actor_id, snapshot.epoch));
        self.custom_errors.remove(id);
        let root = compile_root.to_path_buf();
        let source = source.to_owned();
        let progress = BuildProgress::new(format!("Rust Custom {id}"));
        let worker_progress = progress.clone();
        let (receive, job) = module_engine_script::spawn_job(move |cancelled| {
            worker_progress.stage("Компиляция Rust; текущая версия продолжает работать");
            let compiler = ScriptCompiler::new(root.clone(), root.join(".work/game-scripts"));
            compiler
                .compile_kind_cancellable(&source, ScriptKind::Update, cancelled)
                .map_err(|e| e.to_string())
        });
        self.pending = Some((id.into(), receive));
        self.pending_job = Some(job);
        self.custom_activity = Some(progress);
    }
    pub fn tick(&mut self, dt: f64, playing: bool) -> Result<(), String> {
        // Pause discards only input the owner has not accepted. Accepted edge
        // packets retain the actor's commit/ack contract until Restart/Stop.
        if !playing {
            self.worker_inputs.clear();
        }
        self.poll_worker();
        // Each viewport contributes to one UI frame. Consuming the full input
        // prevents hidden panes or a newly focused code editor retaining WASD.
        let input = std::mem::take(&mut self.input);
        if input.restart && !playing && self.has_game_host() {
            self.restart_game()?;
        }
        let waiting_for_host = self.deferred_preparation && self.game_preparation.is_some();
        if playing
            && waiting_for_host
            && let Some(job) = &self.game_preparation
        {
            job.progress
                .stage("Пересоздание исполнителя; Play ожидает готовности");
        }
        if playing && !waiting_for_host {
            self.play_session = true;
        }
        if let Some((id, receiver)) = &self.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    let id = id.clone();
                    self.pending_job.take();
                    self.pending = None;
                    match result {
                        Ok(candidate) => {
                            if self.worker.is_some() {
                                let current = self
                                    .worker_snapshot
                                    .as_ref()
                                    .map(|snapshot| (snapshot.actor_id, snapshot.epoch));
                                if let (Some((actor, epoch)), Some((current_actor, _))) =
                                    (self.pending_worker_epoch.take(), current)
                                    && actor == current_actor
                                {
                                    let request =
                                        self.next_worker_request(WorkerRequest::Custom(id.clone()));
                                    self.queue_worker_command(WorkerCommand::ApplyCustom {
                                        id,
                                        candidate: Box::new(candidate),
                                        mode: CustomApplyMode::Compatible,
                                        request,
                                        epoch,
                                    });
                                    if let Some(progress) = &self.custom_activity {
                                        progress
                                            .stage("Ожидание применения между игровыми тактами");
                                    }
                                } else {
                                    let error = "Game заменён после начала компиляции; примените Rust повторно".to_owned();
                                    self.custom_errors.insert(id, error.clone());
                                    self.error = Some(error.clone());
                                    if let Some(progress) = &self.custom_activity {
                                        progress.fail(error);
                                    }
                                }
                            } else if let Some(host) = &mut self.host {
                                match host.apply_candidate(&id, candidate.clone(), false) {
                                    Ok(()) => {
                                        self.custom_errors.remove(&id);
                                        self.error = None;
                                        self.rejected = None;
                                        if let Some(progress) = &self.custom_activity {
                                            progress.succeed("Новая версия Rust применена");
                                        }
                                    }
                                    Err(e) => {
                                        self.custom_errors.insert(id.clone(), e.clone());
                                        if let Some(progress) = &self.custom_activity {
                                            progress.fail(e.clone());
                                        }
                                        self.error = Some(e);
                                        self.rejected = Some((id, candidate));
                                    }
                                }
                            } else {
                                let error =
                                    "Game не открыт; откройте Game и примените Rust повторно"
                                        .to_owned();
                                if let Some(progress) = &self.custom_activity {
                                    progress.fail(error.clone());
                                }
                                self.custom_errors.insert(id, error.clone());
                                self.error = Some(error);
                            }
                        }
                        Err(e) => {
                            self.custom_errors.insert(id, e.clone());
                            if let Some(progress) = &self.custom_activity {
                                progress.fail(e.clone());
                            }
                            self.error = Some(e);
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.custom_errors.insert(
                        id.clone(),
                        "Rust compilation thread ended without a result".into(),
                    );
                    self.pending_job.take();
                    self.pending = None;
                    self.error = Some("Rust compilation thread ended without a result".into());
                    if let Some(progress) = &self.custom_activity {
                        progress.fail("Rust compilation thread ended without a result");
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.worker.is_some() {
            if self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_alive())
            {
                return Err(
                    "Игровой исполнитель завершился; нажмите Stop и запустите Play повторно".into(),
                );
            }
            let active_playing = playing && !waiting_for_host;
            if self.worker_playing != Some(active_playing) {
                self.worker_playing = Some(active_playing);
                self.queue_worker_command(WorkerCommand::SetPlaying(active_playing));
            }
            // Retain every pressed edge until the actor accepts its packet;
            // held movement is coalesced by the worker independently.
            if active_playing {
                let pressed = !input.actions.is_empty() || input.restart;
                if input.restart {
                    self.worker_inputs.clear();
                    self.worker_inputs.push_front(input);
                } else if !pressed
                    && self
                        .worker_inputs
                        .back()
                        .is_some_and(|queued| queued.actions.is_empty() && !queued.restart)
                {
                    *self.worker_inputs.back_mut().unwrap() = input;
                } else if self.worker_inputs.len() < 128 {
                    self.worker_inputs.push_back(input);
                } else {
                    self.queue_worker_command(WorkerCommand::SetPlaying(false));
                    self.worker_playing = Some(false);
                    return Err("Переполнена очередь ввода Game; Play приостановлен. Дождитесь обработки и повторите действие".into());
                }
                self.flush_worker_commands();
            }
            if let Some(snapshot) = &self.worker_snapshot {
                if let Some(error) = &snapshot.fault {
                    return Err(error.clone());
                }
                if let Some(error) = &snapshot.preparation_error {
                    self.last_preparation_error = Some(error.clone());
                    self.error = Some(error.clone());
                }
            }
            return Ok(());
        }
        if playing
            && !waiting_for_host
            && let Some(host) = &mut self.host
        {
            host.step(dt, &input)?;
            if input.restart {
                self.note_restart_result(Ok(()))?;
            }
        }
        Ok(())
    }
    pub fn take_restart_recovered(&mut self) -> bool {
        std::mem::take(&mut self.restart_recovered)
    }
    fn note_restart_result(&mut self, result: Result<(), String>) -> Result<(), String> {
        match &result {
            Ok(()) => {
                if self
                    .restart_error
                    .as_ref()
                    .is_some_and(|error| self.error.as_ref() == Some(error))
                {
                    self.error = None;
                }
                self.restart_error = None;
                self.restart_recovered = true;
                self.game_frame = GameFrameCache::default();
            }
            Err(error) => {
                self.error = Some(error.clone());
                self.restart_error = Some(error.clone());
            }
        }
        result
    }
    fn restart_game(&mut self) -> Result<(), String> {
        if self.worker.is_some() {
            self.worker_inputs.clear();
            self.input = GameInput::default();
            if !self
                .worker_requests
                .values()
                .any(|request| matches!(request, WorkerRequest::Restart))
            {
                let request = self.next_worker_request(WorkerRequest::Restart);
                self.queue_worker_command(WorkerCommand::Restart { request });
            }
            return Ok(());
        }
        let result = self
            .host
            .as_mut()
            .ok_or("Game ещё не подготовлен".to_owned())
            .and_then(GameHost::restart);
        self.note_restart_result(result)
    }
    pub fn selected(&self) -> Option<u64> {
        self.selected
    }
    pub fn select(&mut self, id: Option<u64>) {
        if self.selected != id {
            self.selected = id;
            self.field_key = None;
        }
    }
    pub fn is_play_session(&self) -> bool {
        self.play_session
    }
    pub fn normalize_selection(&mut self, game: &GameProject) {
        if !game.scenes.contains_key(&self.scene) {
            self.scene = game.entry_scene.clone();
            self.field_key = None;
        }
        if self.selected.is_some_and(|id| {
            !game.scenes[&self.scene]
                .objects
                .iter()
                .any(|o| o.object.id == id)
        }) {
            self.select(None);
        }
    }
    pub fn invalidate_preview_assets(&mut self) {
        self.preview_document = None;
        if let Some(host) = &mut self.preview_host {
            host.invalidate_content();
        }
        self.texture = None;
        self.game_frame = GameFrameCache::default();
        self.field_key = None;
    }
    pub fn ensure_preview(
        &mut self,
        document: &GraphDocument,
        root: &Path,
        compile_root: Option<&Path>,
        kind: WorldKind,
    ) -> Result<(), String> {
        if self.deferred_preparation {
            let result = self.ensure_deferred(document, root, compile_root, kind, true);
            if let Err(error) = &result {
                self.last_preparation_error = Some(error.clone());
            }
            return result;
        }
        let mut candidate = document.clone();
        let game = candidate.game.as_mut().ok_or("No authored game project")?;
        self.normalize_selection(game);
        game.entry_scene = self.scene.clone();
        if self.preview_document.as_ref() == Some(&candidate) && self.preview_host.is_some() {
            return Ok(());
        }
        if let Some(host) = &mut self.preview_host {
            match host.refresh_author_preview(&candidate) {
                Ok(()) => {
                    self.preview_document = Some(candidate);
                    return Ok(());
                }
                Err(error) if host.session.world_kind() == kind && !error.contains("rebuild") => {
                    return Err(error);
                }
                Err(_) => {}
            }
        }
        let backend = self
            .backend_root
            .clone()
            .map(Ok)
            .unwrap_or_else(|| game_host::backend_root_for(root, compile_root))?;
        let mut host =
            crate::game_host::new_at(candidate.clone(), root, &backend, compile_root, kind)?;
        host.services.set_enabled(false);
        self.preview_host = Some(host);
        self.preview_document = Some(candidate);
        Ok(())
    }
    pub fn accept_author_candidate(&mut self, document: &GraphDocument) -> Result<(), String> {
        if self.play_session {
            return Err("Stop Play before changing authored scenes".into());
        }
        self.restore_author_candidate(document)
    }
    /// Cancellation restores only the author's preview while its Play copy stays independent.
    pub fn restore_author_candidate(&mut self, document: &GraphDocument) -> Result<(), String> {
        let mut candidate = document.clone();
        let game = candidate.game.as_mut().ok_or("No authored game project")?;
        self.normalize_selection(game);
        game.entry_scene = self.scene.clone();
        if let Some(host) = &mut self.preview_host {
            host.refresh_author_preview(&candidate)?;
            self.preview_document = Some(candidate);
        }
        self.field_key = None;
        Ok(())
    }
    /// Game is an independent Play copy, with the ARPG camera and gameplay HUD.
    pub fn scene_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &mut GraphDocument,
        root: &Path,
        compile_root: Option<&Path>,
        kind: WorldKind,
        playing: &mut bool,
    ) -> bool {
        if document.game.is_none() {
            ui.label("Open or create a game project through File.");
            return false;
        }
        ui.horizontal_wrapped(|ui| {
            ui.strong("GAME");
            if ui.button(if *playing { "Pause" } else { "Play" }).clicked() {
                *playing = !*playing;
            }
            if ui.button("Stop").clicked() {
                *playing = false;
                self.stop();
            }
            if ui
                .add_enabled(self.has_game_host(), egui::Button::new("Restart"))
                .clicked()
            {
                let _ = self.restart_game();
            }
            ui.label("WASD · LMB attack · R restart");
        });
        if let Err(error) = self.ensure(document, root, compile_root, kind) {
            self.error = Some(error);
        }
        self.custom_ui(ui, document, compile_root);
        if let Some(error) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if self.worker.is_some() {
            self.worker_scene_ui(ui, *playing);
            return false;
        }
        if let Some(host) = &mut self.host {
            let size = ui.available_size().max(Vec2::splat(1.));
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            let input = if *playing {
                player::read_input_for(
                    host.presentation_id(),
                    ui,
                    &response,
                    rect,
                    host.camera,
                    true,
                )
            } else {
                GameInput::default()
            };
            let aim = player::preview_aim_for(
                host.presentation_id(),
                host.session.state(),
                &input,
                *playing
                    && response.hovered()
                    && ui.input(|i| i.focused)
                    && (!ui.ctx().egui_wants_keyboard_input() || response.has_focus()),
                false,
            );
            merge_view_input(
                &mut self.input,
                input,
                response.hovered() || response.has_focus(),
            );
            if rect.width() > 2. && rect.height() > 2. {
                let physical = rect.size() * ui.ctx().pixels_per_point();
                let fit = (1280. / physical.x).min(720. / physical.y).min(1.);
                let width = (physical.x * fit).round().max(1.) as usize;
                let height = (physical.y * fit).round().max(1.) as usize;
                let key = GameFrameKey {
                    revision: host.presentation_revision(),
                    camera: host.camera,
                    size: [width, height],
                    tick: host.session.state().tick,
                    time: host.session.state().time,
                };
                let now = ui.input(|input| input.time);
                if self
                    .game_frame
                    .should_render(key, ui.ctx().cumulative_frame_nr(), now, *playing)
                {
                    self.performance.game_render_calls += 1;
                    match host.render(width, height) {
                        Ok(frame) => {
                            player::update_texture(ui, &mut self.texture, &frame);
                            update_render_diagnostic(
                                &mut self.error,
                                &mut self.render_error,
                                Ok(()),
                            );
                        }
                        Err(error) => update_render_diagnostic(
                            &mut self.error,
                            &mut self.render_error,
                            Err(error),
                        ),
                    }
                } else {
                    self.performance.game_frame_reuses += 1;
                }
                // Input collection and HUD above/below always execute, including
                // cached/multipass frames. Rendering is independently paced.
                ui.ctx().request_repaint_after(if *playing {
                    Duration::from_secs_f64(1. / 60.)
                } else {
                    Duration::from_millis(250)
                });
                if let Some(texture) = &self.texture {
                    ui.painter().image(
                        texture.id(),
                        rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1., 1.)),
                        Color32::WHITE,
                    );
                }
                if player::draw_hud_for(
                    host.presentation_id(),
                    ui,
                    rect,
                    host.session.state(),
                    host.camera,
                    aim,
                ) {
                    let _ = self.restart_game();
                }
            }
        } else if self.game_preparation.is_some() {
            ui.spinner();
            ui.label("Game готовится в фоне. Можно продолжать работать с редактором.");
        }
        false
    }
    fn worker_scene_ui(&mut self, ui: &mut egui::Ui, playing: bool) {
        self.worker_view_visible = true;
        let Some(snapshot) = self.worker_snapshot.clone() else {
            ui.spinner();
            ui.label("Game готовится в фоне. Можно продолжать работать с редактором.");
            ui.ctx().request_repaint_after(Duration::from_millis(16));
            return;
        };
        let size = ui.available_size().max(Vec2::splat(1.));
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let input = if playing {
            player::read_input_for(
                snapshot.presentation_ids.first().map(String::as_str),
                ui,
                &response,
                rect,
                snapshot.camera,
                true,
            )
        } else {
            GameInput::default()
        };
        let aim = player::preview_aim_for(
            snapshot.presentation_ids.first().map(String::as_str),
            &snapshot.state,
            &input,
            playing
                && response.hovered()
                && ui.input(|input| input.focused)
                && (!ui.ctx().egui_wants_keyboard_input() || response.has_focus()),
            false,
        );
        merge_view_input(
            &mut self.input,
            input,
            response.hovered() || response.has_focus(),
        );
        if rect.width() <= 2. || rect.height() <= 2. {
            return;
        }
        let physical = rect.size() * ui.ctx().pixels_per_point();
        let fit = (1280. / physical.x).min(720. / physical.y).min(1.);
        let size = [
            (physical.x * fit).round().max(1.) as usize,
            (physical.y * fit).round().max(1.) as usize,
        ];
        if self.worker_viewport != Some(size) {
            self.worker_viewport = Some(size);
            self.queue_worker_command(WorkerCommand::SetViewport {
                width: size[0],
                height: size[1],
            });
        }
        if let Some(frame) = &snapshot.frame {
            if self
                .worker_frame
                .as_ref()
                .is_none_or(|previous| !Arc::ptr_eq(previous, frame))
            {
                player::update_texture(ui, &mut self.texture, frame);
                self.worker_frame = Some(frame.clone());
                self.performance.game_render_calls += 1;
            } else {
                self.performance.game_frame_reuses += 1;
            }
        }
        if let Some(texture) = &self.texture {
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1., 1.)),
                Color32::WHITE,
            );
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Подготовка первого кадра…",
                egui::FontId::proportional(14.),
                Color32::LIGHT_GRAY,
            );
        }
        if player::draw_hud_for(
            snapshot.presentation_ids.first().map(String::as_str),
            ui,
            rect,
            &snapshot.state,
            snapshot.camera,
            aim,
        ) {
            let _ = self.restart_game();
        }
        ui.ctx().request_repaint_after(if playing {
            Duration::from_secs_f64(1. / 60.)
        } else {
            Duration::from_millis(250)
        });
    }
    /// Hierarchy and Inspector share authored selection, independently of Game.
    pub fn hierarchy_ui(&mut self, ui: &mut egui::Ui, document: &mut GraphDocument) -> bool {
        let Some(original) = document.game.clone() else {
            ui.label("Open or create a game project through File.");
            return false;
        };
        let mut game = original.clone();
        self.normalize_selection(&game);
        ui.heading("Hierarchy");
        ui.add_enabled_ui(!self.play_session, |ui| {
            let before = self.scene.clone();
            egui::ComboBox::from_id_salt("authored-scene")
                .selected_text(&self.scene)
                .show_ui(ui, |ui| {
                    for id in game.scenes.keys() {
                        ui.selectable_value(&mut self.scene, id.clone(), id);
                    }
                });
            if self.scene != before {
                self.select(None);
                self.preview_document = None;
            }
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.scene_name)
                        .hint_text("Scene name")
                        .desired_width(140.),
                );
                for (label, duplicate) in [("+ Scene", false), ("Duplicate", true)] {
                    if ui.button(label).clicked() {
                        match create_scene(&mut game, &self.scene, &self.scene_name, duplicate) {
                            Ok(id) => {
                                self.scene = id;
                                self.scene_name.clear();
                                self.select(None);
                            }
                            Err(error) => self.error = Some(error),
                        }
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Set as entry").clicked() {
                    game.entry_scene = self.scene.clone();
                }
                ui.label(format!("Entry: {}", game.entry_scene));
            });
            ui.horizontal(|ui| {
                if ui.button("+ Object").clicked() {
                    let scene = game.scenes.get_mut(&self.scene).unwrap();
                    match next_object_id(scene) {
                        Ok(id) => {
                            scene.objects.push(AuthoredObject::new(WorldObject {
                                id,
                                parent: None,
                                transform: Transform::default(),
                                color: [190, 190, 190, 255],
                            }));
                            self.select(Some(id));
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                if ui
                    .add_enabled(self.selected.is_some(), egui::Button::new("Delete"))
                    .clicked()
                    && let Some(id) = self.selected
                {
                    delete_object(game.scenes.get_mut(&self.scene).unwrap(), id);
                    self.select(None);
                }
            });
        });
        ui.separator();
        fn tree(
            ui: &mut egui::Ui,
            objects: &[AuthoredObject],
            parent: Option<u64>,
            depth: usize,
            selected: &mut Option<u64>,
        ) {
            if depth > 64 {
                return;
            }
            for object in objects
                .iter()
                .filter(|object| object.object.parent == parent)
            {
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 13.);
                    let label = format!(
                        "{} · {}",
                        object.object.id,
                        object.prefab.as_deref().unwrap_or("Object")
                    );
                    if ui
                        .selectable_label(*selected == Some(object.object.id), label)
                        .clicked()
                    {
                        *selected = Some(object.object.id);
                    }
                });
                tree(ui, objects, Some(object.object.id), depth + 1, selected);
            }
        }
        let mut selection = self.selected;
        egui::ScrollArea::vertical().show(ui, |ui| {
            tree(
                ui,
                &game.scenes[&self.scene].objects,
                None,
                0,
                &mut selection,
            )
        });
        self.select(selection);
        if game != original {
            document.game = Some(game);
            true
        } else {
            false
        }
    }
    pub fn inspector_ui(&mut self, ui: &mut egui::Ui, document: &mut GraphDocument) -> bool {
        let composition = match crate::host::composition_at(
            self.backend_root.as_deref().unwrap_or(Path::new(".")),
        ) {
            Ok(composition) => composition,
            Err(error) => {
                ui.colored_label(Color32::LIGHT_RED, &error);
                self.error = Some(error);
                return false;
            }
        };
        self.inspector_ui_with_composition(ui, document, &composition)
    }
    fn inspector_ui_with_composition(
        &mut self,
        ui: &mut egui::Ui,
        document: &mut GraphDocument,
        composition: &CompositionContext,
    ) -> bool {
        let Some(original) = document.game.clone() else {
            ui.label("No authored scene");
            return false;
        };
        let mut game = original.clone();
        self.normalize_selection(&game);
        let activity = match composition.activity(document) {
            Ok(activity) => activity,
            Err(error) => {
                ui.colored_label(Color32::LIGHT_RED, &error);
                self.error = Some(error);
                return false;
            }
        };
        let active = &activity.components;
        let schemas = composition.systems.components();
        let manifest =
            match module_engine_project::AssetManifest::from_value(&document.asset_manifest) {
                Ok(manifest) => manifest,
                Err(error) => {
                    self.error = Some(error);
                    return false;
                }
            };
        let asset_paths = manifest.asset_paths();
        let validation = ComponentValidationContext {
            scenes: &game.scenes,
            modules: &activity.modules,
            asset_paths: Some(&asset_paths),
            active_components: Some(&activity.components),
        };
        let resolved =
            match game.resolve_scene_with_context(&self.scene, &composition.systems, &validation) {
                Ok(resolved) => resolved,
                Err(error) => {
                    ui.colored_label(Color32::LIGHT_RED, &error);
                    self.error = Some(error);
                    return false;
                }
            };
        let fields = ComponentEditorContext {
            modules: &activity.modules,
            components: &activity.components,
            entities: &resolved,
            manifest: &manifest,
        };
        ui.heading("Inspector");
        if self.selected.is_none() {
            ui.label("Select an object in Scene or Hierarchy.");
        }
        if self.play_session {
            ui.label("Play · редактирование доступно после Stop");
        }
        egui::ScrollArea::vertical().show(ui,|ui|{
            if let Some(id)=self.selected && let Some(entity)=resolved.iter().find(|entity|entity.object.id==id).cloned(){
                let templates=game.prefabs.clone();
                let attached=entity.components.keys().cloned().collect::<BTreeSet<_>>();
                let parents=game.scenes[&self.scene].objects.iter().filter(|object|object.object.id!=id).map(|object|object.object.id).collect::<Vec<_>>();
                let object=game.scenes.get_mut(&self.scene).unwrap().objects.iter_mut().find(|object|object.object.id==id).unwrap();
                let prefab=object.prefab.as_ref().and_then(|name|templates.get(name));
                if self.field_key.as_ref()!=Some(&(self.scene.clone(),id)){
                    self.component_drafts=entity.components.iter().map(|(name,value)|(name.clone(),schemas.resolve_component(name,value))).collect();
                    self.advanced=serde_json::to_string_pretty(&json!({"components":object.components,"overrides":object.overrides,"references":object.references})).unwrap_or_default();
                    self.field_key=Some((self.scene.clone(),id));
                }
                ui.add_enabled_ui(!self.play_session,|ui|{
                    ui.strong(format!("Object {id}"));
                    for(name,values)in[("Position",&mut object.object.transform.translation),("Rotation (radians)",&mut object.object.transform.rotation),("Scale",&mut object.object.transform.scale)]{
                        ui.label(name);ui.horizontal(|ui|{for value in values{ui.add(egui::DragValue::new(value).speed(0.05).max_decimals(3));}});
                    }
                    egui::ComboBox::from_id_salt("object-parent").selected_text(object.object.parent.map_or("Root".into(),|parent|parent.to_string())).show_ui(ui,|ui|{
                        ui.selectable_value(&mut object.object.parent,None,"Root");for parent in parents{ui.selectable_value(&mut object.object.parent,Some(parent),parent.to_string());}
                    });
                    let mut color=entity.object.color;
                    if ui.color_edit_button_srgba_unmultiplied(&mut color).changed(){object.object.color=color;if let Some(base)=prefab{if color==base.color{object.overrides.remove("color");}else{object.overrides.insert("color".into(),json!(color));}}}
                    let before=object.prefab.clone();
                    egui::ComboBox::from_id_salt("object-prefab").selected_text(object.prefab.as_deref().unwrap_or("No template")).show_ui(ui,|ui|{
                        ui.selectable_value(&mut object.prefab,None,"No template");for name in templates.keys(){ui.selectable_value(&mut object.prefab,Some(name.clone()),name);}
                    });
                    if object.prefab!=before{link_template(object,&entity,&templates);self.field_key=None;}
                    let visible=self.component_drafts.keys().filter(|name|active.contains(*name)&&schemas.schema(name).is_some()).cloned().collect::<Vec<_>>();
                    for name in visible{let schema=schemas.schema(&name).unwrap();ui.push_id(&name,|ui|{egui::CollapsingHeader::new(&schema.label).default_open(true).show(ui,|ui|{
                        let draft=self.component_drafts.get_mut(&name).unwrap();component_editor(ui,schema,draft,&attached,&fields);let value=draft.clone();
                        ui.horizontal(|ui|{
                            if ui.button("Apply fields").clicked(){apply_component(object,prefab,&name,value);self.field_key=None;}
                            if prefab.is_some_and(|prefab|prefab.components.contains_key(&name)){
                                if ui.button("Inherit defaults").clicked(){reset_component(object,prefab,&name);self.field_key=None;}
                            }else if ui.button("Remove component").clicked(){reset_component(object,prefab,&name);self.component_drafts.remove(&name);self.field_key=None;}
                        });
                    });});}
                    egui::ComboBox::from_id_salt("add-game-component").selected_text("+ Component from active modules").show_ui(ui,|ui|{
                        for name in active{if let Some(schema)=schemas.schema(name)&&!self.component_drafts.contains_key(name)&&ui.button(&schema.label).clicked(){self.component_drafts.insert(name.clone(),schema.defaults.clone());}}
                    });
                    let hidden=entity.components.keys().filter(|name|!active.contains(*name)||schemas.schema(name).is_none()).count();if hidden>0{ui.label(format!("{hidden} inactive or unknown component records preserved; see Advanced"));}
                    ui.collapsing("Advanced: components / overrides / references",|ui|{
                        ui.add(egui::TextEdit::multiline(&mut self.advanced).code_editor().desired_rows(12).desired_width(f32::INFINITY));
                        if ui.button("Apply raw JSON").clicked(){match apply_raw_object(object,&self.advanced){Ok(())=>self.field_key=None,Err(error)=>self.error=Some(error)}}
                    });
                });
                if active.contains("mesh")||active.contains("material"){self.asset_ui(ui,&manifest,object,prefab,!self.play_session);}
                ui.add_enabled_ui(!self.play_session,|ui|{ui.horizontal_wrapped(|ui|{ui.label("New linked template");ui.text_edit_singleline(&mut self.template_name);});});
                if ui.add_enabled(!self.play_session,egui::Button::new("Create template from selected")).clicked(){match create_template(&mut game,&self.scene,&entity,&self.template_name){Ok(_)=>{self.template_name.clear();self.template_key=None;self.field_key=None;},Err(error)=>self.error=Some(error)}}
                let template_name=game.scenes[&self.scene].objects.iter().find(|object|object.object.id==id).and_then(|object|object.prefab.clone());
                if let Some(name)=template_name&&let Some(template)=game.prefabs.get(&name){
                    if self.template_key.as_deref()!=Some(&name){self.template_draft=Some(template.clone());self.template_key=Some(name.clone());}
                    ui.collapsing(format!("Linked template source: {name}"),|ui|{ui.add_enabled_ui(!self.play_session,|ui|{
                        if let Some(draft)=&mut self.template_draft{ui.color_edit_button_srgba_unmultiplied(&mut draft.color);let attached=draft.components.keys().cloned().collect::<BTreeSet<_>>();for(component,value)in&mut draft.components{if active.contains(component)&&let Some(schema)=schemas.schema(component){ui.push_id(format!("template-{component}"),|ui|{ui.collapsing(&schema.label,|ui|{*value=schemas.resolve_component(component,value);component_editor(ui,schema,value,&attached,&fields);});});}}}
                        if ui.button("Apply template defaults to instances").clicked()&&let Some(draft)=&self.template_draft{game.prefabs.insert(name.clone(),draft.clone());self.field_key=None;}
                    });});
                }
            }
        });
        if game != original {
            document.game = Some(game);
            true
        } else {
            false
        }
    }
    /// Shared controls for both the Rust editor and the Game window. Compiled
    /// instances and rejected state belong to Game, independently of preview Scripts.
    pub fn custom_editor_ui(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        source: Option<&str>,
        compile_root: Option<&Path>,
    ) {
        let custom_pending = self.pending.is_some()
            || self
                .worker_requests
                .values()
                .any(|request| matches!(request, WorkerRequest::Custom(_)));
        let instances = if let Some(snapshot) = &self.worker_snapshot {
            snapshot
                .custom
                .iter()
                .filter(|instance| instance.source_id == id)
                .count()
        } else {
            self.host.as_ref().map_or(0, |host| {
                host.script_sources
                    .values()
                    .filter(|source| source.as_str() == id)
                    .count()
            })
        };
        let available = !custom_pending
            && self.game_preparation.is_none()
            && instances > 0
            && compile_root.is_some()
            && source.is_some();
        if ui
            .add_enabled(available, egui::Button::new("Применить Rust"))
            .clicked()
            && let (Some(source), Some(root)) = (source, compile_root)
        {
            self.request_custom(id, source, root);
        }
        if let Some((pending, _)) = &self.pending {
            ui.horizontal(|ui| { ui.spinner(); ui.label(format!("Game Rust {pending}: компиляция в фоне; дождитесь завершения перед следующим Apply.")); });
        } else if self
            .worker_requests
            .values()
            .any(|request| matches!(request, WorkerRequest::Custom(_)))
        {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Game Rust: ожидание применения между игровыми тактами…");
            });
        } else if self.game_preparation.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Подготовка Game для применения Rust…");
            });
        } else if !self.has_game_host() || instances == 0 {
            ui.label("Game Custom ещё не подготовлен. Диагностика подготовки показана ниже и в статусе операций.");
        } else if compile_root.is_none() {
            ui.colored_label(
                Color32::LIGHT_RED,
                "Компилятор Rust недоступен для этого пакета.",
            );
        }
        if let Some(error) = self.custom_errors.get(id) {
            egui::CollapsingHeader::new("Диагностика Game Rust")
                .default_open(true)
                .show(ui, |ui| {
                    ui.colored_label(Color32::LIGHT_RED, error);
                });
        }
        if let Some(snapshot) = &self.worker_snapshot {
            let mut current = 0;
            let mut previous = 0;
            for instance in snapshot
                .custom
                .iter()
                .filter(|instance| instance.source_id == id)
            {
                if Some(instance.source.as_str()) == source {
                    current += 1;
                } else {
                    previous += 1;
                }
                if let Some(error) = &instance.error {
                    ui.colored_label(
                        Color32::LIGHT_RED,
                        format!("Game instance {}: {error}", instance.instance),
                    );
                }
            }
            if current + previous > 0 {
                ui.label(if previous == 0 {
                    format!("Game: текущий исходник скомпилирован в {current} экземплярах.")
                } else {
                    format!("Game: предыдущая скомпилированная версия — {previous} экземпляров, текущая — {current}.")
                });
            }
        } else if let Some(host) = &self.host {
            let mut current = 0;
            let mut previous = 0;
            for (instance, source_id) in &host.script_sources {
                if source_id != id {
                    continue;
                }
                if let Some(slot) = host.slots.get(instance) {
                    match slot.lock() {
                        Ok(active) if Some(active.source()) == source => current += 1,
                        Ok(_) => previous += 1,
                        Err(_) => {
                            ui.colored_label(
                                Color32::LIGHT_RED,
                                format!("Game instance {instance}: Custom slot poisoned"),
                            );
                        }
                    }
                }
            }
            if current + previous > 0 {
                ui.label(if previous == 0 {
                    format!("Game: текущий исходник скомпилирован в {current} экземплярах.")
                } else {
                    format!("Game: предыдущая скомпилированная версия — {previous} экземпляров, текущая — {current}.")
                });
            }
        }
        let rejected = if let Some(snapshot) = &self.worker_snapshot {
            snapshot
                .rejected
                .get(id)
                .map(|candidate| (id.to_owned(), candidate.source.clone()))
        } else {
            self.rejected
                .as_ref()
                .filter(|(rejected_id, _)| rejected_id == id)
                .map(|(id, candidate)| (id.clone(), candidate.source().to_owned()))
        };
        if let Some((rejected_id, candidate_source)) = rejected {
            if let Some(candidate) = self
                .worker_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.rejected.get(id))
            {
                ui.colored_label(
                    Color32::LIGHT_RED,
                    format!(
                        "Game Rust, запрос #{}: {}",
                        candidate.request, candidate.error
                    ),
                );
            }
            let fresh = source == Some(candidate_source.as_str());
            ui.collapsing(
                "Несовместимый кандидат Game: сохранённый исходник",
                |ui| {
                    ui.code(&candidate_source);
                },
            );
            if !fresh {
                ui.colored_label(Color32::LIGHT_RED, "Кандидат относится к предыдущему тексту. Примените текущий Rust перед сбросом или миграцией.");
            }
            ui.add_enabled_ui(fresh && !custom_pending, |ui| {
                if ui.button("Применить со сбросом состояния").clicked()
                {
                    let _ =
                        self.apply_rejected_custom(&rejected_id, source.unwrap_or_default(), None);
                }
                ui.collapsing(
                    "Явная миграция состояния Game (JSON)",
                    |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.migration)
                                .code_editor()
                                .desired_rows(4),
                        );
                        if ui.button("Применить с этим состоянием").clicked()
                        {
                            match serde_json::from_str(&self.migration) {
                                Ok(state) => {
                                    let _ = self.apply_rejected_custom(
                                        &rejected_id,
                                        source.unwrap_or_default(),
                                        Some(state),
                                    );
                                }
                                Err(error) => {
                                    self.custom_errors
                                        .insert(rejected_id.clone(), error.to_string());
                                }
                            }
                        }
                    },
                );
            });
        }
    }

    pub fn apply_rejected_custom(
        &mut self,
        id: &str,
        current_source: &str,
        state: Option<Value>,
    ) -> Result<(), String> {
        if self.worker.is_some() {
            if self.pending.is_some()
                || self
                    .worker_requests
                    .values()
                    .any(|request| matches!(request, WorkerRequest::Custom(_)))
            {
                return Err("Дождитесь завершения применения Game Rust".into());
            }
            let snapshot = self
                .worker_snapshot
                .as_ref()
                .ok_or("Game ещё не подготовлен")?;
            let candidate = snapshot
                .rejected
                .get(id)
                .ok_or("Нет отклонённого кандидата Game Rust")?;
            if candidate.source != current_source {
                return Err(
                    "Кандидат Game Rust устарел; сначала примените текущий исходник".into(),
                );
            }
            let epoch = snapshot.epoch;
            let mode = state.map_or(CustomApplyMode::Reset, CustomApplyMode::Migrate);
            let request = self.next_worker_request(WorkerRequest::Custom(id.into()));
            self.queue_worker_command(WorkerCommand::ApplyRejected {
                id: id.into(),
                source: current_source.into(),
                mode,
                request,
                epoch,
            });
            let progress = BuildProgress::new(format!("Rust Custom {id}: применение состояния"));
            progress.stage("Ожидание применения между игровыми тактами");
            self.custom_activity = Some(progress);
            return Ok(());
        }
        let result = (|| {
            if self.pending.is_some() {
                return Err("Дождитесь завершения компиляции Game Rust".into());
            }
            let (candidate_id, candidate) = self
                .rejected
                .as_ref()
                .ok_or("Нет отклонённого кандидата Game Rust")?;
            if candidate_id != id || candidate.source() != current_source {
                return Err(
                    "Кандидат Game Rust устарел; сначала примените текущий исходник".into(),
                );
            }
            let host = self.host.as_mut().ok_or("Game ещё не подготовлен")?;
            if let Some(state) = state {
                host.apply_candidate_migrated(id, candidate, state)
            } else {
                host.apply_candidate(id, candidate.clone(), true)
            }
        })();
        let progress = BuildProgress::new(format!("Rust Custom {id}: применение состояния"));
        match &result {
            Ok(()) => {
                self.rejected = None;
                self.custom_errors.remove(id);
                self.error = None;
                progress.succeed("Game Rust и состояние применены");
            }
            Err(error) => {
                self.custom_errors.insert(id.into(), error.clone());
                self.error = Some(error.clone());
                progress.fail(error.clone());
            }
        }
        self.custom_activity = Some(progress);
        result
    }

    fn custom_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &GraphDocument,
        compile_root: Option<&Path>,
    ) {
        let ids = document
            .game
            .as_ref()
            .map(game_host::script_ids)
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeSet<_>>();
        if !ids.contains(&self.source_selected) {
            self.source_selected = ids.first().cloned().unwrap_or_default();
        }
        egui::ComboBox::from_id_salt("game-custom-source")
            .selected_text(&self.source_selected)
            .show_ui(ui, |ui| {
                for id in ids {
                    ui.selectable_value(&mut self.source_selected, id.clone(), id);
                }
            });
        let id = self.source_selected.clone();
        self.custom_editor_ui(
            ui,
            &id,
            document.custom_sources.get(&id).map(String::as_str),
            compile_root,
        );
    }
    fn asset_ui(
        &mut self,
        ui: &mut egui::Ui,
        manifest: &module_engine_project::AssetManifest,
        object: &mut AuthoredObject,
        prefab: Option<&Prefab>,
        enabled: bool,
    ) {
        let response = egui::Frame::group(ui.style())
            .show(ui, |ui| {
                ui.label("Model / material resources");
                let mesh = component_value(object, prefab, "mesh");
                let material = component_value(object, prefab, "material");
                if let Some(reference) = mesh.get("path").and_then(Value::as_str) {
                    ui.label(format!("Model: {reference}"));
                }
                if let Some(reference) = material.get("base_color_texture").and_then(Value::as_str)
                {
                    ui.label(format!("Texture: {reference}"));
                }
                ui.label("Перетащите модель или текстуру из Assets сюда для назначения объекту.");
            })
            .response;
        if let Some(payload) = response.dnd_release_payload::<crate::assets_ui::AssetDrag>()
            && enabled
        {
            let result = manifest
                .resolve_reference(&payload.reference)
                .and_then(|path| {
                    let kind = asset_kind(&path).ok_or("Choose a static model or texture")?;
                    attach_resource(object, prefab, &payload.reference, kind)
                });
            match result {
                Ok(()) => self.field_key = None,
                Err(error) => self.error = Some(error),
            }
        }
    }
    pub fn systems_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &mut GraphDocument,
        root: &Path,
    ) -> bool {
        self.world_kind = match game_host::world_kind(document) {
            Ok(kind) => kind,
            Err(error) => {
                ui.colored_label(Color32::LIGHT_RED, error);
                return false;
            }
        };
        let Some(game) = document.game.as_ref() else {
            ui.label("This project has no gameplay graph");
            return false;
        };
        if self.graph_base.as_ref() != Some(&game.execution) {
            self.graph_draft = None;
            self.graph_text_dirty = false;
            self.params_id = None;
        }
        if self.graph_draft.is_none() {
            self.graph_draft = Some(game.execution.clone());
            self.graph_base = Some(game.execution.clone());
            self.graph_text = serde_json::to_string_pretty(&game.execution).unwrap_or_default();
        }
        if self.positions_root.as_deref() != Some(root) {
            let path = positions_path(root);
            self.positions = std::fs::read(&path)
                .ok()
                .filter(|b| b.len() <= 1024 * 1024)
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            self.positions_root = Some(root.into());
        }
        let registry =
            match crate::host::composition_at(self.backend_root.as_deref().unwrap_or(root)) {
                Ok(composition) => composition.systems,
                Err(e) => {
                    ui.colored_label(Color32::LIGHT_RED, e);
                    return false;
                }
            };
        if self.graph_text_dirty {
            ui.label("Load edited JSON into the draft before applying.");
        }
        let mut draft = self.graph_draft.clone().unwrap();
        let before = draft.clone();
        let mut apply = false;
        let mut discard = false;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Executable systems / staged graph");
            apply = ui
                .add_enabled(
                    !self.play_session && !self.graph_text_dirty,
                    egui::Button::new("Apply graph draft"),
                )
                .clicked();
            discard = ui.button("Discard draft").clicked();
            ui.label("Incomplete draft is local; saved graph and runtime keep working.");
        });
        if self.play_session {
            ui.label("Stop Play before applying a new execution graph.");
        }
        egui::CollapsingHeader::new("Stages, order and declared resources").show(ui,|ui|{
            let mut stage_move=None;let mut node_move=None;let mut stage_delete=None;
            for (i,stage) in draft.stages.iter_mut().enumerate(){ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut stage.name).desired_width(120.));if ui.button("Earlier").clicked(){stage_move=Some((i,false));}
if ui.button("Later").clicked(){stage_move=Some((i,true));}
if ui.button("Delete stage").clicked(){stage_delete=Some(i);}});
                for (j,node) in stage.nodes.iter().enumerate(){ui.horizontal(|ui|{if ui.selectable_label(self.systems.selected.as_deref()==Some(&node.id),format!("{} · {}",node.id,node.module)).clicked(){self.systems.selected=Some(node.id.clone());}
if ui.small_button("Earlier").clicked(){node_move=Some((i,j,false));}
if ui.small_button("Later").clicked(){node_move=Some((i,j,true));}});}
            }
            if let Some((i,down))=stage_move{move_item(&mut draft.stages,i,down);}
if let Some((i,j,down))=node_move{move_item(&mut draft.stages[i].nodes,j,down);}
if let Some(i)=stage_delete{draft.stages.remove(i);}
            if ui.button("+ Empty stage").clicked(){let name=unique_name("Stage",draft.stages.iter().map(|s|s.name.as_str()));draft.stages.push(SystemStage{name,nodes:vec![]});}
            if ui.button("Replace after constraints with displayed linear order").clicked(){linear_after(&mut draft);}
            ui.label("Moving nodes does not erase authored after constraints. Adjust them explicitly or use the linear-order action.");
            ui.separator();let mut remove=None;
            for (i,resource) in draft.resources.iter_mut().enumerate(){ui.horizontal(|ui|{ui.add(egui::TextEdit::singleline(&mut resource.name).desired_width(105.));ui.add(egui::TextEdit::singleline(&mut resource.data_type).desired_width(105.));ui.checkbox(&mut resource.external,"external");if ui.small_button("×").clicked(){remove=Some(i);}});}
            if let Some(i)=remove{draft.resources.remove(i);}
if ui.button("+ Resource").clicked(){let name=unique_name("resource",draft.resources.iter().map(|r|r.name.as_str()));draft.resources.push(SystemResource{name,data_type:"Value".into(),external:false,initial:Value::Null});}
            ui.label("Initial values, after constraints and all bindings are editable in the graph JSON below.");
        });
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("system-palette")
                .selected_text("+ Installed module in last stage")
                .show_ui(ui, |ui| {
                    for descriptor in registry.descriptors() {
                        if ui.button(&descriptor.id).clicked() {
                            let id = unique_name(
                                "system",
                                draft
                                    .stages
                                    .iter()
                                    .flat_map(|s| &s.nodes)
                                    .map(|n| n.id.as_str()),
                            );
                            let bindings = descriptor
                                .ports
                                .iter()
                                .filter_map(|p| {
                                    draft
                                        .resources
                                        .iter()
                                        .find(|r| r.data_type == p.data_type)
                                        .map(|r| (p.name.clone(), r.name.clone()))
                                })
                                .collect();
                            let node = SystemNode {
                                id,
                                module: descriptor.id.clone(),
                                params: if descriptor.id == "custom.rust.update" {
                                    json!({"source_id":"game-controller"})
                                } else {
                                    json!({})
                                },
                                bindings,
                                after: vec![],
                            };
                            if let Some(stage) = draft.stages.last_mut() {
                                stage.nodes.push(node);
                            } else {
                                draft.stages.push(SystemStage {
                                    name: "Stage 1".into(),
                                    nodes: vec![node],
                                });
                            }
                        }
                    }
                });
            if ui.button("Delete selected node").clicked()
                && let Some(id) = &self.systems.selected
            {
                for stage in &mut draft.stages {
                    stage.nodes.retain(|n| &n.id != id);
                    for node in &mut stage.nodes {
                        node.after.retain(|a| a != id);
                    }
                }
            }
        });
        if let Some(node) = draft
            .stages
            .iter_mut()
            .flat_map(|s| &mut s.nodes)
            .find(|n| self.systems.selected.as_deref() == Some(&n.id))
        {
            if self.params_id.as_deref() != Some(&node.id) {
                self.params = serde_json::to_string(&node.params).unwrap_or_default();
                self.params_id = Some(node.id.clone());
            }
            ui.horizontal(|ui| {
                ui.label(&node.id);
                ui.text_edit_singleline(&mut self.params);
                if ui.button("Update draft params").clicked() {
                    match serde_json::from_str(&self.params) {
                        Ok(v) => node.params = v,
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
            });
            for port in registry
                .descriptor(&node.module)
                .into_iter()
                .flat_map(|d| &d.ports)
            {
                ui.horizontal(|ui| {
                    ui.label(format!("{} ({})", port.name, port.data_type));
                    let binding = node.bindings.entry(port.name.clone()).or_default();
                    egui::ComboBox::from_id_salt(format!("binding-{}-{}", node.id, port.name))
                        .selected_text(binding.as_str())
                        .show_ui(ui, |ui| {
                            for resource in draft
                                .resources
                                .iter()
                                .filter(|r| r.data_type == port.data_type)
                            {
                                ui.selectable_value(binding, resource.name.clone(), &resource.name);
                            }
                        });
                });
            }
        }
        if draft != before && !self.graph_text_dirty {
            self.graph_text = serde_json::to_string_pretty(&draft).unwrap_or_default();
        }
        ui.collapsing("Text graph: resources / stages / bindings / params", |ui| {
            if ui
                .add(
                    egui::TextEdit::multiline(&mut self.graph_text)
                        .code_editor()
                        .desired_rows(14)
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                self.graph_text_dirty = true;
            }
            if ui.button("Load JSON into draft").clicked() {
                match serde_json::from_str::<ExecutionGraph>(&self.graph_text) {
                    Ok(value) => {
                        draft = value;
                        self.graph_text_dirty = false;
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
        });
        let (nodes, edges) = canvas_data(&draft, &registry, &mut self.positions);
        let mut moved = false;
        for action in canvas::draw(ui, &mut self.systems, &nodes, &edges) {
            match action {
                CanvasAction::Move(id, pos) => {
                    self.positions.insert(id, pos);
                    moved = true;
                }
                CanvasAction::Connect {
                    from_node,
                    from_port,
                    to_node,
                    to_port,
                } => {
                    if let Err(e) = connect_pins(
                        &mut draft, &registry, &from_node, &from_port, &to_node, &to_port,
                    ) {
                        self.error = Some(e);
                    }
                }
                CanvasAction::DeleteEdge(i) => {
                    if let Some(edge) = edges.get(i)
                        && let Some(node) = draft
                            .stages
                            .iter_mut()
                            .flat_map(|s| &mut s.nodes)
                            .find(|n| n.id == edge.to_node)
                    {
                        node.bindings.remove(&edge.to_port);
                    }
                }
                _ => {}
            }
        }
        if moved && let Err(e) = save_positions(root, &self.positions) {
            self.error = Some(e);
        }
        if draft != before && !self.graph_text_dirty {
            self.graph_text = serde_json::to_string_pretty(&draft).unwrap_or_default();
        }
        self.graph_draft = Some(draft.clone());
        if discard {
            self.discard_graph_draft();
            return false;
        }
        if let Some(e) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, e);
        }
        if !apply {
            return false;
        }
        let mut candidate = document.clone();
        candidate.game.as_mut().unwrap().execution = draft;
        if let Err(e) = candidate
            .game
            .as_ref()
            .unwrap()
            .validate_with_registry(&registry)
        {
            self.error = Some(format!("Draft rejected; saved graph unchanged: {e}"));
            return false;
        }
        if self.deferred_preparation {
            let changed = candidate.game != document.game;
            document.game = candidate.game;
            self.graph_draft = None;
            self.graph_text_dirty = false;
            self.field_key = None;
            self.error = None;
            // The accepted document is the requested configuration. Its old
            // executable hosts remain live until ensure's worker accepts it.
            let compile = self.compile_root.clone();
            if let Err(error) =
                self.ensure_deferred(document, root, compile.as_deref(), self.world_kind, true)
            {
                self.error = Some(error);
            }
            return changed;
        }
        let compile = self.compile_root.clone();
        let backend = match self
            .backend_root
            .clone()
            .map(Ok)
            .unwrap_or_else(|| game_host::backend_root_for(root, compile.as_deref()))
        {
            Ok(backend) => backend,
            Err(error) => {
                self.error = Some(error);
                return false;
            }
        };
        let mut runtime_document = candidate.clone();
        if runtime_document
            .game
            .as_ref()
            .is_some_and(|g| g.scenes.contains_key(&self.scene))
        {
            runtime_document.game.as_mut().unwrap().entry_scene = self.scene.clone();
        }
        match crate::game_host::new_at(
            runtime_document.clone(),
            root,
            &backend,
            compile.as_deref(),
            self.world_kind,
        ) {
            Ok(mut host) => {
                let changed = candidate.game != document.game;
                document.game = candidate.game;
                host.services.set_enabled(false);
                self.preview_host = Some(host);
                self.preview_document = Some(runtime_document);
                self.host = None;
                self.texture = None;
                self.graph_draft = None;
                self.graph_text_dirty = false;
                self.field_key = None;
                self.error = None;
                changed
            }
            Err(e) => {
                self.error = Some(format!(
                    "Draft execution rejected; previous runtime retained: {e}"
                ));
                false
            }
        }
    }
}
struct ComponentEditorContext<'a> {
    modules: &'a BTreeSet<String>,
    components: &'a BTreeSet<String>,
    entities: &'a [GameEntity],
    manifest: &'a module_engine_project::AssetManifest,
}

fn component_editor(
    ui: &mut egui::Ui,
    schema: &ComponentSchema,
    component: &mut Value,
    attached: &BTreeSet<String>,
    context: &ComponentEditorContext<'_>,
) {
    if schema.fields.is_empty() {
        ui.label("Marker component (no fields)");
    }
    for field in &schema.fields {
        if !field
            .requires_modules
            .iter()
            .all(|id| context.modules.contains(id))
            || !field
                .requires_components
                .iter()
                .all(|id| attached.contains(id) && context.components.contains(id))
        {
            continue;
        }
        let Some(path) = schema.field_path(field) else {
            continue;
        };
        let mut value = component_field(component, path)
            .unwrap_or(&field.default)
            .clone();
        let changed = reference_editor(ui, field, &mut value, context).unwrap_or_else(|| {
            crate::app::properties::value_editor(
                ui,
                &field.id,
                &field.label,
                &field.value_type,
                &mut value,
            )
        });
        if changed && let Err(error) = set_component_field(component, path, value.clone()) {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        if let Err(error) = field.value_type.validate(&value) {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }
}

fn object_reference_choices(
    entities: &[GameEntity],
    required: &[String],
    active: &BTreeSet<String>,
) -> Vec<u64> {
    if required.iter().any(|id| !active.contains(id)) {
        return vec![];
    }
    entities
        .iter()
        .filter(|entity| required.iter().all(|id| entity.components.contains_key(id)))
        .map(|entity| entity.object.id)
        .collect()
}

fn asset_reference_choices(
    manifest: &module_engine_project::AssetManifest,
    kinds: &[String],
) -> Vec<(String, String)> {
    manifest
        .assets
        .iter()
        .filter(|(_, entry)| {
            kinds.is_empty()
                || Path::new(&entry.path)
                    .extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|extension| {
                        kinds
                            .iter()
                            .any(|kind| kind.eq_ignore_ascii_case(extension))
                    })
        })
        .map(|(id, entry)| (format!("asset://{id}"), entry.path.clone()))
        .collect()
}

fn reference_editor(
    ui: &mut egui::Ui,
    field: &PropertySchema,
    value: &mut Value,
    context: &ComponentEditorContext<'_>,
) -> Option<bool> {
    match &field.value_type {
        PropertyType::ObjectReference {
            required_components,
            optional,
        } => {
            let choices =
                object_reference_choices(context.entities, required_components, context.components);
            let before = value.clone();
            ui.push_id(&field.id, |ui| {
                ui.horizontal(|ui| {
                    ui.label(&field.label);
                    let selected = value.as_u64().map_or_else(
                        || {
                            if *optional {
                                "None".into()
                            } else {
                                "Choose object".into()
                            }
                        },
                        |id| {
                            if choices.contains(&id) {
                                format!("Object {id}")
                            } else {
                                format!("Unavailable object {id}")
                            }
                        },
                    );
                    egui::ComboBox::from_id_salt("reference")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            if *optional {
                                ui.selectable_value(value, Value::Null, "None");
                            }
                            for id in choices {
                                ui.selectable_value(value, json!(id), format!("Object {id}"));
                            }
                        });
                })
            });
            Some(*value != before)
        }
        PropertyType::AssetReference { kinds, optional } => {
            let choices = asset_reference_choices(context.manifest, kinds);
            let before = value.clone();
            let response = ui
                .push_id(&field.id, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(&field.label);
                        let selected = value.as_str().map_or_else(
                            || {
                                if *optional {
                                    "None".into()
                                } else {
                                    "Choose asset".into()
                                }
                            },
                            |reference| {
                                choices
                                    .iter()
                                    .find(|(id, _)| id == reference)
                                    .map_or_else(|| reference.to_owned(), |(_, path)| path.clone())
                            },
                        );
                        egui::ComboBox::from_id_salt("reference")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                if *optional {
                                    ui.selectable_value(value, Value::Null, "None");
                                }
                                for (reference, path) in &choices {
                                    ui.selectable_value(value, json!(reference), path);
                                }
                            });
                    })
                })
                .inner
                .response;
            if let Some(payload) = response.dnd_release_payload::<crate::assets_ui::AssetDrag>()
                && choices
                    .iter()
                    .any(|(reference, _)| reference == &payload.reference)
            {
                *value = json!(payload.reference);
            }
            Some(*value != before)
        }
        _ => None,
    }
}
fn merge_view_input(current: &mut GameInput, next: GameInput, active: bool) {
    if !active {
        return;
    }
    let mut actions = current.actions.clone();
    actions.extend(next.actions.iter().cloned());
    // A second visible pane may update held values after the pane that saw a
    // press. Retain that occurrence's captured context until a new press
    // explicitly supplies replacement context for the same keys.
    let mut action_values = if current.actions.is_empty() {
        BTreeMap::new()
    } else {
        current.action_values.clone()
    };
    if !next.actions.is_empty() {
        action_values.extend(
            next.action_values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    let restart = current.restart || next.restart;
    *current = next;
    current.actions = actions;
    current.action_values = action_values;
    current.restart = restart;
}
fn unique_name<'a>(base: &str, existing: impl IntoIterator<Item = &'a str>) -> String {
    let names = existing.into_iter().collect::<BTreeSet<_>>();
    (1..)
        .map(|i| format!("{base}-{i}"))
        .find(|name| !names.contains(name.as_str()))
        .unwrap()
}
fn create_scene(
    game: &mut GameProject,
    source: &str,
    name: &str,
    duplicate: bool,
) -> Result<String, String> {
    let id = if name.trim().is_empty() {
        unique_name("scene", game.scenes.keys().map(String::as_str))
    } else {
        name.trim().to_owned()
    };
    if game.scenes.contains_key(&id) {
        return Err("Scene name already exists".into());
    }
    let mut scene = if duplicate {
        game.scenes
            .get(source)
            .ok_or("Source scene missing")?
            .clone()
    } else {
        AuthoredScene::default()
    };
    scene.name = id.clone();
    game.scenes.insert(id.clone(), scene);
    Ok(id)
}
fn next_object_id(scene: &AuthoredScene) -> Result<u64, String> {
    scene
        .objects
        .iter()
        .map(|o| o.object.id)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| "Object ID space exhausted".into())
}
fn delete_object(scene: &mut AuthoredScene, id: u64) {
    let mut removed = BTreeSet::from([id]);
    loop {
        let count = removed.len();
        for o in &scene.objects {
            if o.object.parent.is_some_and(|p| removed.contains(&p)) {
                removed.insert(o.object.id);
            }
        }
        if count == removed.len() {
            break;
        }
    }
    scene.objects.retain(|o| !removed.contains(&o.object.id));
    for o in &mut scene.objects {
        o.references.retain(|_, id| !removed.contains(id));
    }
}
fn create_template(
    game: &mut GameProject,
    scene: &str,
    entity: &GameEntity,
    name: &str,
) -> Result<String, String> {
    let id = entity.object.id;
    if !game
        .scenes
        .get(scene)
        .is_some_and(|scene| scene.objects.iter().any(|object| object.object.id == id))
    {
        return Err("Selected object missing".into());
    }
    let name = if name.trim().is_empty() {
        unique_name("template", game.prefabs.keys().map(String::as_str))
    } else {
        name.trim().to_owned()
    };
    if game.prefabs.contains_key(&name) {
        return Err("Template name already exists".into());
    }
    game.prefabs.insert(
        name.clone(),
        Prefab {
            color: entity.object.color,
            components: entity.components.clone(),
        },
    );
    let object = game
        .scenes
        .get_mut(scene)
        .unwrap()
        .objects
        .iter_mut()
        .find(|o| o.object.id == id)
        .unwrap();
    object.prefab = Some(name.clone());
    object.components.clear();
    object.overrides.clear();
    Ok(name)
}
fn component_attachments(object: &AuthoredObject, prefab: Option<&Prefab>) -> BTreeSet<String> {
    object
        .components
        .keys()
        .chain(prefab.into_iter().flat_map(|p| p.components.keys()))
        .cloned()
        .collect()
}
fn reset_component(object: &mut AuthoredObject, prefab: Option<&Prefab>, name: &str) {
    let attachments = component_attachments(object, prefab);
    object.components.remove(name);
    object.overrides.retain(|key, _| {
        component_override_owner(key, attachments.iter().map(String::as_str)) != Some(name)
    });
}
fn link_template(
    object: &mut AuthoredObject,
    resolved: &GameEntity,
    templates: &BTreeMap<String, Prefab>,
) {
    let Some(template) = object.prefab.as_ref().and_then(|n| templates.get(n)) else {
        object.object.color = resolved.object.color;
        object.components = resolved.components.clone();
        object.overrides.clear();
        return;
    };
    // Keep explicit instance data when a new template lacks an overridden field.
    let overrides = object.overrides.clone();
    for (path, value) in overrides {
        if path == "color" {
            continue;
        }
        let Some(component) =
            component_override_owner(&path, resolved.components.keys().map(String::as_str))
        else {
            continue;
        };
        let field = &path[component.len() + 1..];
        let mut base = template
            .components
            .get(component)
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let Some(local) = object.components.get(component) {
            if let (Some(base), Some(local)) = (base.as_object_mut(), local.as_object()) {
                base.extend(local.clone());
            } else {
                base = local.clone();
            }
        }
        let exists = component_field(&base, field).is_some();
        if !exists {
            let local = object
                .components
                .entry(component.to_owned())
                .or_insert_with(|| {
                    resolved
                        .components
                        .get(component)
                        .cloned()
                        .unwrap_or_else(|| json!({}))
                });
            put_field(local, field, value);
            object.overrides.remove(&path);
        }
    }
}
fn component_value(object: &AuthoredObject, prefab: Option<&Prefab>, name: &str) -> Value {
    let mut value = prefab
        .and_then(|p| p.components.get(name))
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(local) = object.components.get(name) {
        if let (Some(base), Some(fields)) = (value.as_object_mut(), local.as_object()) {
            base.extend(fields.clone());
        } else {
            value = local.clone();
        }
    }
    let attachments = component_attachments(object, prefab);
    for (path, field) in &object.overrides {
        if component_override_owner(path, attachments.iter().map(String::as_str)) == Some(name) {
            let path = &path[name.len() + 1..];
            put_field(&mut value, path, field.clone());
        }
    }
    value
}
fn value_leaves(value: &Value, prefix: &str, result: &mut BTreeMap<String, Value>) {
    if let Value::Object(fields) = value
        && !fields.is_empty()
    {
        for (field, v) in fields {
            let path = if prefix.is_empty() {
                field.clone()
            } else {
                format!("{prefix}.{field}")
            };
            value_leaves(v, &path, result);
        }
    } else if let Value::Array(values) = value
        && !values.is_empty()
    {
        for (index, value) in values.iter().enumerate() {
            value_leaves(value, &format!("{prefix}.{index}"), result);
        }
    } else {
        result.insert(prefix.into(), value.clone());
    }
}
fn put_field(value: &mut Value, path: &str, new: Value) {
    let (head, tail) = path
        .split_once('.')
        .map_or((path, None), |(h, t)| (h, Some(t)));
    if let Value::Array(values) = value {
        let Ok(index) = head.parse::<usize>() else {
            return;
        };
        if index > 65535 {
            return;
        }
        values.resize_with(values.len().max(index + 1), || Value::Null);
        if let Some(tail) = tail {
            if values[index].is_null() {
                values[index] = if tail
                    .split('.')
                    .next()
                    .is_some_and(|part| part.parse::<usize>().is_ok())
                {
                    json!([])
                } else {
                    json!({})
                };
            }
            put_field(&mut values[index], tail, new);
        } else {
            values[index] = new;
        }
    } else {
        if !value.is_object() {
            *value = json!({});
        }
        if let Some(tail) = tail {
            let child = value
                .as_object_mut()
                .unwrap()
                .entry(head)
                .or_insert_with(|| {
                    if tail
                        .split('.')
                        .next()
                        .is_some_and(|part| part.parse::<usize>().is_ok())
                    {
                        json!([])
                    } else {
                        json!({})
                    }
                });
            put_field(child, tail, new);
        } else {
            value.as_object_mut().unwrap().insert(head.into(), new);
        }
    }
}
fn apply_component(object: &mut AuthoredObject, prefab: Option<&Prefab>, name: &str, value: Value) {
    reset_component(object, prefab, name);
    let Some(base) = prefab.and_then(|p| p.components.get(name)) else {
        object.components.insert(name.into(), value);
        return;
    };
    let mut defaults = BTreeMap::new();
    let mut edited = BTreeMap::new();
    value_leaves(base, "", &mut defaults);
    value_leaves(&value, "", &mut edited);
    let mut local = json!({});
    for (field, value) in edited {
        if let Some(default) = defaults.get(&field) {
            if &value != default {
                object.overrides.insert(format!("{name}.{field}"), value);
            }
        } else {
            put_field(&mut local, &field, value);
        }
    }
    if local.as_object().is_some_and(|m| !m.is_empty()) {
        object.components.insert(name.into(), local);
    }
}
fn apply_raw_object(object: &mut AuthoredObject, text: &str) -> Result<(), String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let components = serde_json::from_value(value["components"].clone())
        .map_err(|_| "components must be a map")?;
    let overrides = serde_json::from_value(value["overrides"].clone())
        .map_err(|_| "overrides must be a map")?;
    let references = serde_json::from_value(value["references"].clone())
        .map_err(|_| "references must be an ID map")?;
    object.components = components;
    object.overrides = overrides;
    object.references = references;
    Ok(())
}
fn asset_kind(path: &str) -> Option<&'static str> {
    match Path::new(path)
        .extension()?
        .to_str()?
        .to_lowercase()
        .as_str()
    {
        "gltf" | "glb" => Some("model"),
        "png" | "jpg" | "jpeg" => Some("texture"),
        _ => None,
    }
}
#[cfg(test)]
fn attach_asset(
    object: &mut AuthoredObject,
    prefab: Option<&Prefab>,
    path: &str,
) -> Result<(), String> {
    attach_resource(
        object,
        prefab,
        path,
        asset_kind(path).ok_or("Choose model or texture")?,
    )
}
pub(crate) fn attach_resource(
    object: &mut AuthoredObject,
    prefab: Option<&Prefab>,
    path: &str,
    kind: &str,
) -> Result<(), String> {
    match Some(kind) {
        Some("model") => {
            let mut mesh = component_value(object, prefab, "mesh");
            if !mesh.is_object() {
                return Err("Mesh component must be an object".into());
            }
            mesh["path"] = json!(path);
            apply_component(object, prefab, "mesh", mesh);
        }
        Some("texture") => {
            let mut material = component_value(object, prefab, "material");
            if !material.is_object() {
                return Err("Material component must be an object".into());
            }
            if material.get("base_color").is_none() {
                material["base_color"] = json!([1., 1., 1., 1.]);
            }
            material["base_color_texture"] = json!(path);
            apply_component(object, prefab, "material", material);
        }
        _ => return Err("Select a static model or PNG/JPEG texture".into()),
    }
    Ok(())
}
fn move_item<T>(items: &mut [T], index: usize, down: bool) {
    let next = if down {
        index.checked_add(1)
    } else {
        index.checked_sub(1)
    };
    if let Some(next) = next
        && next < items.len()
    {
        items.swap(index, next);
    }
}
fn linear_after(graph: &mut ExecutionGraph) {
    let mut previous = None;
    for node in graph.stages.iter_mut().flat_map(|s| &mut s.nodes) {
        node.after = previous.into_iter().collect();
        previous = Some(node.id.clone());
    }
}
fn positions_path(root: &Path) -> PathBuf {
    root.join(".user/full/game-systems-positions.json")
}
fn save_positions(root: &Path, positions: &BTreeMap<String, [f32; 2]>) -> Result<(), String> {
    let path = positions_path(root);
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    if !parent
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(root.canonicalize().map_err(|e| e.to_string())?)
    {
        return Err("Personal graph layout leaves package root".into());
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(positions).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
fn canvas_data(
    graph: &ExecutionGraph,
    registry: &SystemRegistry,
    positions: &mut BTreeMap<String, [f32; 2]>,
) -> (Vec<CanvasNode>, Vec<CanvasEdge>) {
    let mut nodes = vec![];
    let mut edges = vec![];
    let mut producers = BTreeMap::<String, (String, String)>::new();
    for (i, r) in graph.resources.iter().filter(|r| r.external).enumerate() {
        let id = format!("external-resource::{}", r.name);
        nodes.push(CanvasNode {
            id: id.clone(),
            label: r.name.clone(),
            subtitle: format!("External {}", r.data_type),
            position: *positions
                .entry(id.clone())
                .or_insert([10., i as f32 * 125.]),
            inputs: vec![],
            outputs: vec![CanvasPort {
                name: r.name.clone(),
                data_type: r.data_type.clone(),
            }],
            expandable: false,
            opaque: false,
        });
        producers.insert(r.name.clone(), (id, r.name.clone()));
    }
    for (i, (stage, node)) in graph
        .stages
        .iter()
        .flat_map(|s| s.nodes.iter().map(move |n| (&s.name, n)))
        .enumerate()
    {
        let Some(d) = registry.descriptor(&node.module) else {
            continue;
        };
        let mut inputs = vec![];
        let mut outputs = vec![];
        for port in &d.ports {
            let visual = CanvasPort {
                name: port.name.clone(),
                data_type: port.data_type.clone(),
            };
            if port.access != ResourceAccess::Write {
                inputs.push(visual.clone());
                if let Some(resource) = node.bindings.get(&port.name)
                    && let Some((from, from_port)) = producers.get(resource)
                {
                    edges.push(CanvasEdge {
                        from_node: from.clone(),
                        from_port: from_port.clone(),
                        to_node: node.id.clone(),
                        to_port: port.name.clone(),
                    });
                }
            }
            if port.access != ResourceAccess::Read {
                outputs.push(visual);
                if let Some(resource) = node.bindings.get(&port.name) {
                    producers.insert(resource.clone(), (node.id.clone(), port.name.clone()));
                }
            }
        }
        nodes.push(CanvasNode {
            id: node.id.clone(),
            label: stage.clone(),
            subtitle: node.module.clone(),
            position: *positions
                .entry(node.id.clone())
                .or_insert([250. + (i % 4) as f32 * 245., 30. + (i / 4) as f32 * 230.]),
            inputs,
            outputs,
            expandable: false,
            opaque: false,
        });
    }
    (nodes, edges)
}
fn connect_pins(
    graph: &mut ExecutionGraph,
    registry: &SystemRegistry,
    from: &str,
    output: &str,
    to: &str,
    input: &str,
) -> Result<(), String> {
    if !from.starts_with("external-resource::") {
        let order = graph
            .stages
            .iter()
            .flat_map(|s| &s.nodes)
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>();
        let source = order
            .iter()
            .position(|id| *id == from)
            .ok_or("Missing source node")?;
        let target = order
            .iter()
            .position(|id| *id == to)
            .ok_or("Missing target node")?;
        if source >= target {
            return Err("Output must precede input in the displayed execution order".into());
        }
    }
    let (resource, data_type) = if let Some(name) = from.strip_prefix("external-resource::") {
        let r = graph
            .resources
            .iter()
            .find(|r| r.name == name && r.external)
            .ok_or("Missing external resource")?;
        if output != name {
            return Err("Missing external output pin".into());
        }
        (name.to_owned(), r.data_type.clone())
    } else {
        let n = graph
            .stages
            .iter()
            .flat_map(|s| &s.nodes)
            .find(|n| n.id == from)
            .ok_or("Missing source node")?;
        let p = registry
            .descriptor(&n.module)
            .and_then(|d| {
                d.ports
                    .iter()
                    .find(|p| p.name == output && p.access != ResourceAccess::Read)
            })
            .ok_or("Missing output port")?;
        (
            n.bindings
                .get(output)
                .ok_or("Output port has no resource binding")?
                .clone(),
            p.data_type.clone(),
        )
    };
    let n = graph
        .stages
        .iter_mut()
        .flat_map(|s| &mut s.nodes)
        .find(|n| n.id == to)
        .ok_or("Missing target node")?;
    let p = registry
        .descriptor(&n.module)
        .and_then(|d| {
            d.ports
                .iter()
                .find(|p| p.name == input && p.access != ResourceAccess::Write)
        })
        .ok_or("Missing input port")?;
    if p.data_type != data_type {
        return Err(format!(
            "Pin type mismatch: {data_type} versus {}",
            p.data_type
        ));
    }
    n.bindings.insert(input.into(), resource);
    Ok(())
}
#[cfg(test)]
mod tests {
    /// Native host/Play tests exercise silent fixtures on platforms without
    /// a WAV endpoint. Production templates and audio contract tests retain
    /// their authored requests and explicit unsupported-service diagnostics.
    fn native_test_document() -> GraphDocument {
        let mut document = crate::game_host::demo_document();
        if !module_engine_host_services::WAV_PLAYBACK_SUPPORTED {
            document.game.as_mut().unwrap().audio_cues.clear();
        }
        document
    }

    #[test]
    #[ignore = "latency acceptance: run explicitly without competing Rust/GPU setup"]
    fn interactive_game_ui_keeps_drawing_and_applying_rust_during_slow_ticks() {
        fn diagnostic(workbench: &GameWorkbench, expected_source: &str) -> String {
            let requests: Vec<_> = workbench
                .worker_requests
                .iter()
                .map(|(id, request)| {
                    let kind = match request {
                        WorkerRequest::Document(_) => "document",
                        WorkerRequest::Custom(_) => "custom",
                        WorkerRequest::Restart => "restart",
                    };
                    (*id, kind)
                })
                .collect();
            let snapshot = workbench.game_snapshot().map(|snapshot| {
                (
                    snapshot.actor_id,
                    snapshot.epoch,
                    snapshot.sequence,
                    snapshot.state.tick,
                    snapshot.playing,
                    snapshot.fault.clone(),
                    snapshot.preparation_error.clone(),
                    snapshot
                        .custom
                        .iter()
                        .map(|instance| {
                            (
                                instance.instance.clone(),
                                instance.source_id.clone(),
                                instance.source == expected_source,
                                instance.source.len(),
                                instance.state.clone(),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            });
            format!(
                "pending={:?}, custom_errors={:?}, requests={requests:?}, queued_commands={}, queued_inputs={}, error={:?}, snapshot(actor,epoch,sequence,tick,playing,fault,preparation,instances)={snapshot:?}",
                workbench.pending.as_ref().map(|(id, _)| id),
                workbench.custom_errors,
                workbench.worker_commands.len(),
                workbench.worker_inputs.len(),
                workbench.error
            )
        }
        let root = crate::project::source_root();
        let id = "game-controller";
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let marker = root.join(format!(
            ".work/game-ui-worker-{}-{nonce}.marker",
            std::process::id()
        ));
        let source = format!(
            "// module-engine-state: {{\"schema\":\"async.ui\",\"initial\":{{\"ticks\":0}}}}\nfn update(ctx: &mut ScriptContext) {{ std::fs::write({:?}, b\"tick\").unwrap(); std::thread::sleep(std::time::Duration::from_millis(80)); ctx.state[\"ticks\"] = serde_json::json!(ctx.state[\"ticks\"].as_u64().unwrap()+1); }}",
            marker.to_string_lossy()
        );
        let mut document = native_test_document();
        document.custom_sources.insert(id.into(), source.clone());
        let kind = game_host::world_kind(&document).unwrap();
        let ctx = egui::Context::default();
        let mut workbench = GameWorkbench::default();
        workbench.set_background_game(true, Some(&ctx));
        workbench
            .ensure(&document, &root, Some(&root), kind)
            .unwrap();
        assert_eq!(
            workbench.game_preparation.as_ref().unwrap().context.kind,
            kind
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        while workbench.game_snapshot().is_none() {
            workbench.poll_preparation(&document).unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "real async Game did not prepare: {}",
                diagnostic(&workbench, &source)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            workbench.host.is_none(),
            "interactive UI must not retain the live host"
        );
        let actor = workbench.game_snapshot().unwrap().actor_id;
        let identity = workbench.game_snapshot().unwrap().custom[0].identity;
        let frame = |workbench: &mut GameWorkbench,
                     document: &mut GraphDocument,
                     playing: bool,
                     phase: &str| {
            workbench
                .poll_preparation(document)
                .unwrap_or_else(|error| {
                    panic!(
                        "{phase}: preparation {error}; {}",
                        diagnostic(workbench, &document.custom_sources[id])
                    )
                });
            // This used to synchronously catch up 15 ticks (~1.2 seconds).
            workbench.tick(0.25, playing).unwrap_or_else(|error| {
                panic!(
                    "{phase}: tick {error}; {}",
                    diagnostic(workbench, &document.custom_sources[id])
                )
            });
            let mut playing = playing;
            workbench.begin_ui_frame();
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(640., 480.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    workbench.scene_ui(ui, document, &root, Some(&root), kind, &mut playing);
                },
            )
            .drop_without_applying_deltas();
            workbench.finish_ui_frame();
        };
        frame(&mut workbench, &mut document, false, "warmup");
        workbench.tick(0., true).unwrap();
        let marker_deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !marker.exists() {
            workbench.poll_preparation(&document).unwrap();
            assert!(
                std::time::Instant::now() < marker_deadline,
                "slow Custom never entered its real worker: {}",
                diagnostic(&workbench, &source)
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let started = std::time::Instant::now();
        for _ in 0..8 {
            frame(&mut workbench, &mut document, true, "slow-tick UI latency");
        }
        assert!(
            started.elapsed() < Duration::from_millis(600),
            "UI waited for slow tick/catch-up: {:?}",
            started.elapsed()
        );
        let edited = format!("{source}\n// compatible live edit\n");
        document.custom_sources.insert(id.into(), edited.clone());
        workbench.request_custom(id, &edited, &root);
        assert!(workbench.pending.is_some());
        let apply_deadline = std::time::Instant::now() + Duration::from_secs(90);
        while !workbench
            .game_snapshot()
            .unwrap()
            .custom
            .iter()
            .any(|instance| instance.source == edited)
        {
            frame(&mut workbench, &mut document, true, "compile/apply");
            assert!(
                std::time::Instant::now() < apply_deadline,
                "compiled Game candidate did not reach its actor: {}",
                diagnostic(&workbench, &edited)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let snapshot = workbench.game_snapshot().unwrap();
        assert_eq!(snapshot.actor_id, actor);
        assert_eq!(snapshot.custom[0].identity, identity);
        assert!(snapshot.state.tick > 0);
        assert!(snapshot.custom[0].state["ticks"].as_u64().unwrap() > 0);
        assert!(workbench.custom_errors.is_empty());
        assert!(workbench.pending.is_none());
        assert!(
            !workbench
                .worker_requests
                .values()
                .any(|request| matches!(request, WorkerRequest::Custom(_)))
        );
        let stopped = std::time::Instant::now();
        workbench.stop();
        assert!(
            stopped.elapsed() < Duration::from_millis(200),
            "Stop waited for worker destruction"
        );
        assert!(!workbench.has_game_host());
        assert!(workbench.game_snapshot().is_none());
        assert!(workbench.worker_commands.is_empty());
        assert!(workbench.worker_requests.is_empty());
        std::thread::sleep(Duration::from_millis(100));
        let _ = std::fs::remove_file(marker);
    }
    #[test]
    fn pending_worker_document_coalesces_to_one_latest_candidate() {
        let document = native_test_document();
        let mut workbench = GameWorkbench::default();
        workbench.next_worker_request(WorkerRequest::Document(Arc::new(document.clone())));
        let mut latest = document.clone();
        for index in 0..200 {
            latest.name = format!("latest {index}");
            workbench.queue_worker_document(&latest, false);
        }
        assert_eq!(workbench.worker_requests.len(), 1);
        assert!(workbench.worker_commands.is_empty());
        assert_eq!(
            workbench
                .worker_deferred_document
                .as_ref()
                .unwrap()
                .0
                .as_ref(),
            &latest
        );
        // Returning to the in-flight version cancels a previously queued edit.
        workbench.queue_worker_document(&document, false);
        assert!(workbench.worker_deferred_document.is_none());
    }
    #[test]
    fn interactive_undo_reverts_failed_gpu_target_and_paused_restart_bypasses_input_backlog() {
        let root = crate::project::source_root();
        let document = native_test_document();
        let mut workbench = GameWorkbench::default();
        workbench
            .ensure(&document, &root, Some(&root), WorldKind::Ecs)
            .unwrap();
        workbench.set_background_game(true, None);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while workbench.game_snapshot().is_none() {
            workbench.poll_preparation(&document).unwrap();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let actor = workbench.game_snapshot().unwrap().actor_id;
        let mut invalid = document.clone();
        let aa = invalid
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap()
            .nodes
            .iter_mut()
            .find(|node| node.module == "gpu.fxaa")
            .unwrap();
        aa.module = "gpu.custom".into();
        aa.params = json!({"source_id":"invalid-worker-aa", "manifest":module_engine_vulkan::builtin_manifest("gpu.fxaa").unwrap()});
        invalid.shader_sources.insert(
            "invalid-worker-aa".into(),
            "#error failed_ui_worker_target\n".into(),
        );
        workbench.sync_graphics(&invalid).unwrap();
        while workbench.worker_rejected_document.is_none() {
            workbench.poll_preparation(&invalid).unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "failed GPU target did not report"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            workbench
                .game_snapshot()
                .unwrap()
                .preparation_error
                .is_some()
        );
        // Undo returns exactly to game_document. That equality must not skip
        // delivery of the cancellation/revert to an autonomous owner.
        workbench
            .ensure(&document, &root, Some(&root), WorldKind::Ecs)
            .unwrap();
        assert!(
            workbench
                .worker_requests
                .values()
                .any(|request| matches!(request, WorkerRequest::Document(_)))
        );
        while workbench
            .game_snapshot()
            .unwrap()
            .preparation_error
            .is_some()
            || !workbench.worker_requests.is_empty()
        {
            workbench.poll_preparation(&document).unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "Undo did not restore accepted target"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(workbench.game_snapshot().unwrap().actor_id, actor);
        assert!(
            workbench
                .game_snapshot()
                .unwrap()
                .accepted_document
                .execution_eq(&document)
        );
        workbench.worker_inputs = (0..128)
            .map(|_| {
                ArpgInput {
                    attack: true,
                    ..Default::default()
                }
                .into_input()
            })
            .collect();
        workbench.tick(0., false).unwrap();
        assert!(
            workbench.worker_inputs.is_empty(),
            "Pause must discard only unsent UI input before polling"
        );
        let epoch = workbench.game_snapshot().unwrap().epoch;
        workbench.worker_inputs = (0..128)
            .map(|_| {
                ArpgInput {
                    attack: true,
                    ..Default::default()
                }
                .into_input()
            })
            .collect();
        workbench.input.restart = true;
        workbench.tick(0., false).unwrap();
        assert!(
            workbench.worker_inputs.is_empty(),
            "R must cancel local pending attacks"
        );
        while workbench.game_snapshot().unwrap().epoch == epoch {
            workbench.tick(0., false).unwrap();
            assert!(
                std::time::Instant::now() < deadline,
                "paused R never reached real actor"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(workbench.take_restart_recovered());
        assert_eq!(workbench.game_snapshot().unwrap().state.tick, 0);
        workbench.stop();
    }
    #[test]
    fn rejected_full_host_retries_disk_header_repair_without_inline_edit() {
        let (mut workbench, mut document, root, context) = preparation_fixture();
        let relative = format!(".work/full-host-retry-{}.hlsli", std::process::id());
        let header = root.join(&relative);
        std::fs::write(&header, "float external_gain() { return 0.2; }\n").unwrap();
        let graph = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap();
        let ao = graph.nodes.iter_mut().find(|node| node.id == "ao").unwrap();
        ao.module = "gpu.custom".into();
        ao.params = json!({"source_id":"full-host-retry", "manifest":module_engine_vulkan::custom_compute_manifest(), "gain":1.0});
        graph
            .edges
            .retain(|edge| edge.to_node != "ao" || edge.to_port == "color");
        document.shader_sources.insert(
            "full-host-retry".into(),
            format!(
                "#include \"{relative}\"\n{}",
                module_engine_vulkan::demo_custom_compute_source()
                    .replace("c.rgb*values[0].x", "c.rgb*values[0].x*external_gain()")
            ),
        );
        crate::project::stamp_composition_versions(
            &mut document,
            &crate::host::composition_at(&root).unwrap(),
        )
        .unwrap();
        let unchanged_document = document.clone();
        let (send, _) = fake_host_job(&mut workbench, &document, context);
        // The file is repaired while the worker is returning its older error.
        std::fs::write(&header, "float external_gain() { return 0.8; }\n").unwrap();
        assert!(
            send.send(Err("simulated earlier include failure".into()))
                .is_ok()
        );
        assert!(
            workbench
                .ensure(&document, &root, Some(&root), WorldKind::Objects)
                .is_err()
        );
        assert!(!workbench.has_pending_preparation());
        std::thread::sleep(std::time::Duration::from_millis(260));
        workbench
            .ensure(&document, &root, Some(&root), WorldKind::Objects)
            .unwrap();
        assert!(workbench.has_pending_preparation());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while workbench.has_pending_preparation() {
            assert!(
                std::time::Instant::now() < deadline,
                "repaired host did not complete"
            );
            workbench.poll_preparation(&document).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let host = workbench.host.as_mut().unwrap();
        host.services.set_enabled(false);
        let frame = host.render(96, 64).unwrap();
        assert_eq!(frame.rgba.len(), 96 * 64 * 4);
        assert!(
            frame
                .rgba
                .as_chunks::<4>()
                .0
                .windows(2)
                .any(|pixels| pixels[0] != pixels[1])
        );
        assert_eq!(document, unchanged_document);
        assert!(workbench.rejected_game.is_none());
        drop(workbench);
        std::fs::remove_file(header).unwrap();
    }
    #[test]
    fn recovered_game_frame_clears_only_its_owned_render_error() {
        let mut error = None;
        let mut render_error = None;
        update_render_diagnostic(
            &mut error,
            &mut render_error,
            Err("HLSL include failed".into()),
        );
        assert_eq!(error.as_deref(), Some("HLSL include failed"));
        assert_eq!(error, render_error);
        update_render_diagnostic(&mut error, &mut render_error, Ok(()));
        assert!(error.is_none() && render_error.is_none());
        for newer in [
            "Custom state migration failed",
            "New renderer preparation failed",
        ] {
            update_render_diagnostic(
                &mut error,
                &mut render_error,
                Err("Transient render failure".into()),
            );
            error = Some(newer.into());
            update_render_diagnostic(&mut error, &mut render_error, Ok(()));
            assert_eq!(error.as_deref(), Some(newer));
            assert!(render_error.is_none());
        }
        update_render_diagnostic(
            &mut error,
            &mut render_error,
            Err("First render error".into()),
        );
        update_render_diagnostic(
            &mut error,
            &mut render_error,
            Err("Second render error".into()),
        );
        assert_eq!(error.as_deref(), Some("Second render error"));
        update_render_diagnostic(&mut error, &mut render_error, Ok(()));
        assert!(error.is_none() && render_error.is_none());
    }
    #[test]
    fn game_frame_cache_paces_play_and_preserves_static_and_dirty_frames() {
        let mut cache = GameFrameCache::default();
        let mut key = GameFrameKey {
            revision: 1,
            camera: Camera::default(),
            size: [640, 360],
            tick: 0,
            time: 0.,
        };
        assert!(cache.should_render(key, 0, 0., false));
        key.revision = 2;
        assert!(
            !cache.should_render(key, 0, 0.1, false),
            "egui multipass must reuse the submitted frame"
        );
        assert!(
            cache.should_render(key, 1, 0.1, false),
            "accepted new host/renderer must invalidate static frame"
        );
        assert!(!cache.should_render(key, 2, 0.2, false));
        assert!(
            cache.should_render(key, 3, 0.351, false),
            "idle still periodically checks file-backed HLSL"
        );
        key.camera.yaw += 0.1;
        assert!(cache.should_render(key, 4, 0.36, false));
        key.size = [800, 450];
        assert!(cache.should_render(key, 5, 0.37, false));
        key.tick = 0;
        key.time = 0.;
        let mut cache = GameFrameCache::default();
        let mut renders = 0;
        for frame in 0..120 {
            key.revision += 1;
            key.tick += 1;
            key.time = frame as f64 / 120.;
            renders += usize::from(cache.should_render(key, frame, key.time, true));
            assert!(!cache.should_render(key, frame, key.time, true));
        }
        assert_eq!(
            renders, 60,
            "120 UI frames should submit 60 Game GPU frames"
        );
        let mut cache = GameFrameCache::default();
        let mut renders = 0;
        for frame in 0..60 {
            key.revision += 1;
            let now = frame as f64 / 60. + if frame % 2 == 0 { 0.0002 } else { -0.0002 };
            renders += usize::from(cache.should_render(key, frame, now, true));
        }
        assert_eq!(
            renders, 60,
            "small vsync jitter must retain 60 FPS instead of skipping every other frame"
        );
    }
    #[test]
    fn stable_preparation_context_and_preview_do_not_clone_or_resolve_per_frame() {
        let (mut workbench, mut document, root, context) = preparation_fixture();
        workbench.normalize_selection(document.game.as_ref().unwrap());
        for _ in 0..120 {
            assert!(matches!(
                workbench.preview_candidate(&document),
                DocumentCandidate::Borrowed(_)
            ));
            assert!(
                workbench
                    .preparation_context(&root, Some(&root), WorldKind::Objects)
                    .unwrap()
                    == context
            );
            workbench.poll_preparation(&document).unwrap();
        }
        assert_eq!(workbench.performance.context_resolutions, 1);
        assert_eq!(workbench.performance.normalized_document_clones, 0);
        workbench.scene = "boss".into();
        let first = workbench.preview_candidate(&document);
        assert_eq!(first.game.as_ref().unwrap().entry_scene, "boss");
        let first_game = first.game.clone();
        drop(first);
        for _ in 0..120 {
            let _ = workbench.preview_candidate(&document);
        }
        assert_eq!(workbench.performance.normalized_document_clones, 2);
        document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("boss")
            .unwrap()
            .objects[0]
            .object
            .transform
            .translation[0] += 1.;
        let changed = workbench.preview_candidate(&document);
        assert_ne!(first_game, changed.game);
        assert_eq!(workbench.performance.normalized_document_clones, 4);
        workbench
            .preparation_context(&root, None, WorldKind::Objects)
            .unwrap();
        workbench
            .preparation_context(&root, None, WorldKind::Ecs)
            .unwrap();
        assert_eq!(
            workbench.performance.context_resolutions, 3,
            "SDK/world context changes must invalidate the path cache"
        );
    }
    fn preparation_fixture() -> (GameWorkbench, GraphDocument, PathBuf, PreparationContext) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let mut workbench = GameWorkbench::default();
        workbench.set_engine_context(Some(&root), &root);
        workbench.set_deferred_preparation(true);
        let context = workbench
            .preparation_context(&root, Some(&root), WorldKind::Objects)
            .unwrap();
        workbench.game_context = Some(context.clone());
        // This fixture exercises the legacy Object route explicitly. New game
        // templates now default to ECS; stale-candidate checks compare the
        // selected world with the prepared context.
        let mut document = native_test_document();
        document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "world.ecs")
            .unwrap()
            .module = "world.objects".into();
        (workbench, document, root, context)
    }
    fn fake_host_job(
        workbench: &mut GameWorkbench,
        document: &GraphDocument,
        context: PreparationContext,
    ) -> (mpsc::Sender<Result<GameHost, String>>, BuildProgress) {
        let (send, receiver) = mpsc::channel();
        let progress = BuildProgress::new("Test host candidate");
        workbench.game_activity = Some(progress.clone());
        let dependencies = game_host::ShaderDependencyWatch::new(document, &context.backend_root);
        workbench.game_preparation = Some(HostPreparation {
            document: document.clone(),
            context,
            receiver,
            progress: progress.clone(),
            dependencies,
            _job: None,
        });
        (send, progress)
    }
    #[test]
    fn preparation_poll_uses_the_same_active_kind_as_the_authored_request() {
        for kind in [WorldKind::Objects, WorldKind::Ecs] {
            let (mut workbench, mut document, root, _) = preparation_fixture();
            document
                .root
                .nodes
                .iter_mut()
                .find(|node| node.id == "world")
                .unwrap()
                .module = match kind {
                WorldKind::Objects => "world.objects",
                WorldKind::Ecs => "world.ecs",
                WorldKind::Custom => unreachable!(),
            }
            .into();
            document.root.nodes.push(make_node(
                "inactive-ecs",
                "world.ecs",
                "Inactive",
                [0., 0.],
                json!({}),
            ));
            let selected = game_host::world_kind(&document).unwrap();
            assert_eq!(selected, kind);
            let context = workbench
                .preparation_context(&root, Some(&root), selected)
                .unwrap();
            workbench.game_context = Some(context.clone());
            let (send, _) = fake_host_job(&mut workbench, &document, context);
            send.send(Err("current request diagnostic".into())).unwrap();
            // A matching request's failure is observable. Kind disagreement
            // used to discard it as obsolete and leave no host/error to poll.
            assert!(
                workbench
                    .poll_preparation(&document)
                    .unwrap_err()
                    .contains("current request diagnostic")
            );
            assert_eq!(workbench.rejected_game.as_ref().unwrap().1.kind, kind);
            assert!(!workbench.has_pending_preparation());
        }
    }
    #[test]
    fn deferred_host_failure_is_cached_across_layout_and_reports_terminal_activity() {
        let (mut workbench, mut document, root, context) = preparation_fixture();
        let (send, progress) = fake_host_job(&mut workbench, &document, context);
        assert!(send.send(Err("expected shader failure".into())).is_ok());
        assert!(
            workbench
                .ensure(&document, &root, Some(&root), WorldKind::Objects)
                .unwrap_err()
                .contains("expected shader failure")
        );
        assert_eq!(
            progress.snapshot().state,
            crate::activity::ActivityState::Failed
        );
        let elapsed = progress.snapshot().elapsed;
        document.root.nodes[0].position[0] += 100.;
        for _ in 0..4 {
            assert!(
                workbench
                    .ensure(&document, &root, Some(&root), WorldKind::Objects)
                    .is_err()
            );
            assert!(!workbench.has_pending_preparation());
        }
        assert_eq!(progress.snapshot().elapsed, elapsed);
        assert!(workbench.host.is_none());
    }
    #[test]
    fn deferred_host_ignores_stale_document_and_context_results() {
        let (mut workbench, mut document, _root, context) = preparation_fixture();
        let (send, progress) = fake_host_job(&mut workbench, &document, context.clone());
        document.game.as_mut().unwrap().entry_scene = "boss".into();
        assert!(send.send(Err("obsolete failure".into())).is_ok());
        workbench.poll_preparation(&document).unwrap();
        assert!(workbench.rejected_game.is_none());
        assert!(workbench.error.is_none());
        assert_eq!(
            progress.snapshot().state,
            crate::activity::ActivityState::Succeeded
        );
        let (send, progress) = fake_host_job(&mut workbench, &document, context.clone());
        let mut changed_context = context;
        changed_context.compile_root = None;
        workbench.game_context = Some(changed_context);
        assert!(send.send(Err("old SDK failure".into())).is_ok());
        workbench.poll_preparation(&document).unwrap();
        assert!(workbench.rejected_game.is_none());
        assert_eq!(
            progress.snapshot().state,
            crate::activity::ActivityState::Succeeded
        );
        assert!(workbench.host.is_none());
    }
    #[test]
    fn stop_invalidates_initial_host_request_before_a_late_result() {
        let (mut workbench, document, _root, context) = preparation_fixture();
        let (send, progress) = fake_host_job(&mut workbench, &document, context);
        workbench.tick(0., true).unwrap();
        assert!(
            !workbench.is_play_session(),
            "Play waits for the initial source copy"
        );
        assert!(workbench.has_pending_preparation());
        let (_custom_send, custom_receiver) = mpsc::channel();
        let custom_progress = BuildProgress::new("Pending Rust");
        workbench.custom_activity = Some(custom_progress.clone());
        workbench.pending = Some(("controller".into(), custom_receiver));
        workbench.stop();
        let _ = send.send(Err("late canceled request".into()));
        workbench.poll_preparation(&document).unwrap();
        assert!(!workbench.is_play_session());
        assert!(!workbench.has_pending_preparation());
        assert!(workbench.host.is_none());
        assert_eq!(
            progress.snapshot().state,
            crate::activity::ActivityState::Succeeded
        );
        assert_eq!(
            custom_progress.snapshot().state,
            crate::activity::ActivityState::Succeeded
        );
    }
    #[test]
    fn deferred_gpu_change_preserves_paused_play_and_bad_hlsl_keeps_last_good() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let mut workbench = GameWorkbench::default();
        workbench.set_engine_context(Some(&root), &root);
        let mut document = native_test_document();
        workbench
            .ensure(&document, &root, Some(&root), WorldKind::Ecs)
            .unwrap();
        let host = workbench.host.as_mut().unwrap();
        host.services.set_enabled(false);
        host.camera.yaw = 0.4;
        host.camera.target = [7., 0., 3.];
        host.session
            .apply_commands(&[WorldCommand::SetTransform {
                id: 1,
                transform: Transform {
                    translation: [4., 1., 2.],
                    ..Default::default()
                },
            }])
            .unwrap();
        let retained_state = host.session.state().clone();
        let retained_camera = host.camera;
        let slot = host.slots["game-controller"].clone();
        let host_address = host as *const GameHost;
        workbench.set_deferred_preparation(true);
        workbench.play_session = true; // A paused Play copy still owns its live state.
        let graph = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap();
        graph
            .nodes
            .iter_mut()
            .find(|node| node.module == "gpu.fxaa")
            .unwrap()
            .params["enabled"] = json!(false);
        workbench.sync_graphics(&document).unwrap();
        assert!(workbench.has_pending_preparation());
        assert_eq!(
            workbench.host.as_ref().unwrap() as *const GameHost,
            host_address
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while workbench.has_pending_preparation() {
            assert!(
                std::time::Instant::now() < deadline,
                "GPU preparation timed out"
            );
            workbench.poll_preparation(&document).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let host = workbench.host.as_mut().unwrap();
        assert_eq!(host as *const GameHost, host_address);
        assert_eq!(host.session.state(), &retained_state);
        assert_eq!(host.camera.target, retained_camera.target);
        assert_eq!(host.camera.yaw, retained_camera.yaw);
        assert!(std::sync::Arc::ptr_eq(
            &slot,
            &host.slots["game-controller"]
        ));
        assert!(host.accepted_document().execution_eq(&document));
        let before = host.render(160, 90).unwrap().rgba;
        let accepted = document.clone();
        let graph = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap();
        let aa = graph
            .nodes
            .iter_mut()
            .find(|node| node.module == "gpu.fxaa")
            .unwrap();
        aa.module = "gpu.custom".into();
        aa.params = json!({"source_id":"broken-aa", "manifest": module_engine_vulkan::builtin_manifest("gpu.fxaa").unwrap()});
        document.shader_sources.insert(
            "broken-aa".into(),
            "#error expected_async_hlsl_failure\n".into(),
        );
        crate::project::stamp_composition_versions(
            &mut document,
            &crate::host::composition_at(&root).unwrap(),
        )
        .unwrap();
        workbench.sync_graphics(&document).unwrap();
        let mut failed = false;
        while workbench.has_pending_preparation() {
            assert!(
                std::time::Instant::now() < deadline,
                "Invalid shader preparation timed out"
            );
            if workbench.poll_preparation(&document).is_err() {
                failed = true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(failed);
        for _ in 0..3 {
            assert!(workbench.sync_graphics(&document).is_err());
            assert!(!workbench.has_pending_preparation());
        }
        let host = workbench.host.as_mut().unwrap();
        assert_eq!(host as *const GameHost, host_address);
        assert_eq!(host.session.state(), &retained_state);
        assert!(host.accepted_document().execution_eq(&accepted));
        assert_eq!(host.render(160, 90).unwrap().rgba, before);
        assert!(std::sync::Arc::ptr_eq(
            &slot,
            &host.slots["game-controller"]
        ));
        assert!(workbench.activity_snapshots().iter().any(|activity| {
            activity.state == crate::activity::ActivityState::Failed
                && activity
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("expected_async_hlsl_failure"))
        }));
        workbench.poll_preparation(&accepted).unwrap();
        assert!(
            workbench.error.is_none(),
            "Reverting a rejected target clears its owned error"
        );
        workbench.last_preparation_error = Some("obsolete preparation error".into());
        workbench.error = Some("unrelated Custom failure".into());
        workbench.poll_preparation(&accepted).unwrap();
        assert_eq!(workbench.error.as_deref(), Some("unrelated Custom failure"));
        let context = workbench
            .preparation_context(&root, Some(&root), WorldKind::Ecs)
            .unwrap();
        workbench.game_context = Some(context.clone());
        let (_send, progress) = fake_host_job(&mut workbench, &accepted, context);
        workbench.tick(0.1, true).unwrap();
        assert_eq!(
            workbench.host.as_ref().unwrap().session.state(),
            &retained_state,
            "Already active Play also waits during a full executor rebuild"
        );
        assert!(progress.snapshot().detail.contains("Play ожидает"));
    }
    #[test]
    fn inactive_viewport_cannot_erase_input_and_consumption_releases_movement() {
        let mut workbench = GameWorkbench::default();
        merge_view_input(
            &mut workbench.input,
            ArpgInput {
                movement: [1., 0.],
                attack: true,
                aim_world: [4., 0., 2.],
                ..Default::default()
            }
            .into_input(),
            true,
        );
        merge_view_input(&mut workbench.input, GameInput::default(), false);
        assert!(workbench.input.attack());
        assert_eq!(workbench.input.aim_world(), [4., 0., 2.]);
        assert_eq!(workbench.input.movement(), [1., 0.]);
        workbench.tick(1. / 60., true).unwrap();
        assert_eq!(workbench.input, GameInput::default());
    }
    #[test]
    fn active_pane_held_updates_preserve_pressed_context_until_frame_consumption() {
        let mut workbench = GameWorkbench::default();
        merge_view_input(
            &mut workbench.input,
            ArpgInput {
                attack: true,
                aim_world: [4., 0., 2.],
                ..Default::default()
            }
            .into_input(),
            true,
        );
        merge_view_input(
            &mut workbench.input,
            ArpgInput {
                movement: [1., 0.],
                aim_world: [8., 0., 9.],
                ..Default::default()
            }
            .into_input(),
            true,
        );
        assert!(workbench.input.attack());
        assert_eq!(workbench.input.aim_world(), [4., 0., 2.]);
        assert_eq!(workbench.input.values["aim_world"], json!([8., 0., 9.]));
        assert_eq!(workbench.input.movement(), [1., 0.]);
        // Another module's edge adds its own captured data without erasing
        // the earlier action's context or copying held values into it.
        merge_view_input(
            &mut workbench.input,
            GameInput {
                actions: BTreeSet::from(["puzzle.use".into()]),
                action_values: BTreeMap::from([("puzzle.target".into(), json!(17))]),
                values: BTreeMap::from([("movement".into(), json!([-1., 0.]))]),
                ..Default::default()
            },
            true,
        );
        assert!(workbench.input.attack());
        assert!(workbench.input.pressed("puzzle.use"));
        assert_eq!(
            workbench.input.action_values["aim_world"],
            json!([4., 0., 2.])
        );
        assert_eq!(workbench.input.action_values["puzzle.target"], json!(17));
        assert_eq!(workbench.input.movement(), [-1., 0.]);
        workbench.tick(1. / 60., true).unwrap();
        assert_eq!(workbench.input, GameInput::default());
        workbench.tick(1. / 60., true).unwrap();
        assert!(workbench.input.actions.is_empty());
    }
    #[test]
    fn source_component_defaults_create_real_runtime_fields_in_an_empty_scene() {
        let composition = source_composition();
        let mut document = source_document();
        let defaults = composition
            .systems
            .components()
            .defaults("example.stats")
            .unwrap()
            .clone();
        let object = &mut document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("empty")
            .unwrap()
            .objects[0];
        let mut edited = defaults;
        set_component_field(&mut edited, "speed", json!(5.)).unwrap();
        set_component_field(&mut edited, "mode", json!("run")).unwrap();
        apply_component(object, None, "example.stats", edited);
        assert!(
            composition
                .activity(&document)
                .unwrap()
                .components
                .contains("example.stats")
        );
        assert!(
            !composition
                .validate_document(&document)
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
        );
        let saved = serde_json::to_vec(&document).unwrap();
        let restored: GraphDocument = serde_json::from_slice(&saved).unwrap();
        let session = GameSession::new_with_registry(
            restored.game.unwrap(),
            WorldKind::Ecs,
            composition.systems.clone(),
        )
        .unwrap();
        assert_eq!(
            session.state().world.component(1, "example.stats").unwrap()["speed"],
            json!(5.)
        );
        assert_eq!(
            session.state().world.component(1, "example.stats").unwrap()["mode"],
            json!("run")
        );
        assert_eq!(
            session.state().world.component(1, "example.stats").unwrap()["offset"],
            json!([1., 2., 3.])
        );
        document.game.as_mut().unwrap().execution.stages[0]
            .nodes
            .clear();
        assert!(
            !composition
                .activity(&document)
                .unwrap()
                .components
                .contains("example.stats")
        );
        assert_eq!(
            document.game.as_ref().unwrap().scenes["empty"].objects[0].components["example.stats"]
                ["speed"],
            json!(5.)
        );
    }
    use super::*;

    fn source_schema() -> ComponentSchema {
        let id = "example.stats";
        let defaults = json!({"speed":2.,"mode":"walk","enabled":true,"title":"Scout","offset":[1.,2.,3.],"target":null,"icon":null});
        let fields = vec![
            (
                "speed",
                "Travel speed",
                PropertyType::Number { min: 0., max: 10. },
            ),
            (
                "mode",
                "Movement mode",
                PropertyType::Enum {
                    choices: vec!["walk".into(), "run".into()],
                },
            ),
            ("enabled", "Enabled", PropertyType::Boolean),
            ("title", "Title", PropertyType::Text { max_len: 32 }),
            (
                "offset.0",
                "Offset X",
                PropertyType::Number {
                    min: -10.,
                    max: 10.,
                },
            ),
            (
                "offset.1",
                "Offset Y",
                PropertyType::Number {
                    min: -10.,
                    max: 10.,
                },
            ),
            (
                "offset.2",
                "Offset Z",
                PropertyType::Number {
                    min: -10.,
                    max: 10.,
                },
            ),
            (
                "target",
                "Target",
                PropertyType::ObjectReference {
                    required_components: vec![id.into()],
                    optional: true,
                },
            ),
            (
                "icon",
                "Portrait",
                PropertyType::AssetReference {
                    kinds: vec!["png".into()],
                    optional: true,
                },
            ),
        ]
        .into_iter()
        .map(|(path, label, value_type)| PropertySchema {
            id: format!("{id}.{path}"),
            label: label.into(),
            scope: PropertyScope::Component {
                component: id.into(),
            },
            default: component_field(&defaults, path).unwrap().clone(),
            value_type,
            requires_modules: vec![],
            requires_components: vec![],
        })
        .collect();
        ComponentSchema {
            id: id.into(),
            owner_module: "example.stats.system".into(),
            version: "1".into(),
            label: "Character settings".into(),
            defaults,
            fields,
            requires_modules: vec![],
            requires_components: vec![],
        }
    }

    fn source_composition() -> CompositionContext {
        struct SourceSystem;
        impl ExecutableSystem for SourceSystem {
            fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
                Ok(())
            }
        }
        let mut composition = crate::host::composition_at(Path::new(".")).unwrap();
        composition
            .systems
            .components_mut()
            .register(source_schema())
            .unwrap();
        composition
            .systems
            .register(
                SystemDescriptor {
                    id: "example.stats.system".into(),
                    state_schema: "example.stats/1".into(),
                    components: vec!["example.stats".into()],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(SourceSystem)),
            )
            .unwrap();
        CompositionContext::new(composition.modules, composition.systems).unwrap()
    }

    fn source_document() -> GraphDocument {
        let mut document = demo_document(WorldKind::Objects);
        document.game = Some(GameProject {
            entry_scene: "empty".into(),
            scenes: BTreeMap::from([(
                "empty".into(),
                AuthoredScene {
                    name: "Empty".into(),
                    objects: vec![AuthoredObject::new(WorldObject {
                        id: 1,
                        parent: None,
                        transform: Transform::default(),
                        color: [255; 4],
                    })],
                },
            )]),
            prefabs: BTreeMap::new(),
            audio_cues: BTreeMap::new(),
            execution: ExecutionGraph {
                resources: vec![],
                stages: vec![SystemStage {
                    name: "Source".into(),
                    nodes: vec![SystemNode {
                        id: "source".into(),
                        module: "example.stats.system".into(),
                        params: json!({}),
                        bindings: BTreeMap::new(),
                        after: vec![],
                    }],
                }],
            },
        });
        document
    }

    #[test]
    fn inspector_renders_registered_source_fields_and_apply_changes_the_running_component() {
        let root = crate::project::source_root();
        let composition = crate::host::composition_at(&root).unwrap();
        let mut document = crate::source_modules::tests::document();
        let defaults = composition
            .systems
            .components()
            .defaults("example.spin")
            .unwrap()
            .clone();
        document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("empty")
            .unwrap()
            .objects[0]
            .components
            .insert("example.spin".into(), defaults);
        let mut workbench = GameWorkbench::default();
        workbench.scene.clone_from(&"empty".into());
        workbench.select(Some(1));
        workbench.set_engine_context(None, &root);
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.);
        let frame = |workbench: &mut GameWorkbench, document: &mut GraphDocument, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1200., 1200.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    workbench.inspector_ui(ui, document);
                },
            )
        };
        let output = frame(&mut workbench, &mut document, vec![]);
        assert!(workbench.error.is_none(), "{:?}", workbench.error);
        for label in ["Rotation", "Radians per second", "Axis", "Enabled"] {
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)), "missing schema label {label}");
        }
        let point = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Apply fields" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("source component has the ordinary Apply fields action");
        output.drop_without_applying_deltas();
        set_component_field(
            workbench.component_drafts.get_mut("example.spin").unwrap(),
            "speed",
            json!(3.),
        )
        .unwrap();
        for pressed in [true, false] {
            frame(
                &mut workbench,
                &mut document,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            )
            .drop_without_applying_deltas();
        }
        assert_eq!(
            document.game.as_ref().unwrap().scenes["empty"].objects[0].components["example.spin"]["speed"],
            json!(3.)
        );
        assert!(
            workbench.host.is_none() && workbench.preview_host.is_none(),
            "schema inspection must not initialize rendering"
        );
        let reopened: GraphDocument =
            serde_json::from_slice(&serde_json::to_vec(&document).unwrap()).unwrap();
        let mut session = GameSession::new_with_registry(
            reopened.game.unwrap(),
            WorldKind::Ecs,
            composition.systems,
        )
        .unwrap();
        session.step(1. / 60., &GameInput::default()).unwrap();
        assert!(
            (session.state().world.object(1).unwrap().transform.rotation[1] - 0.05).abs() < 0.00001
        );
    }

    #[test]
    fn inspector_apply_and_inherit_preserve_longer_component_owners_after_save() {
        struct Noop;
        impl ExecutableSystem for Noop {
            fn step(&mut self, _: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
                Ok(())
            }
        }
        let artifacts = module_engine_script::TestArtifactGuard::new(
            &crate::project::source_root().join(".work/test-temp"),
            "inspector-prefix-owners",
        )
        .unwrap();
        for availability in ["active", "inactive", "unknown"] {
            let mut installed = crate::host::composition_at(Path::new(".")).unwrap();
            for (id, owner) in [("foo", "review.foo"), ("foo.bar", "review.bar")] {
                if id == "foo.bar" && availability == "unknown" {
                    continue;
                }
                installed
                    .systems
                    .components_mut()
                    .register(ComponentSchema {
                        id: id.into(),
                        owner_module: owner.into(),
                        version: "1".into(),
                        label: id.into(),
                        defaults: json!({"value":1.}),
                        fields: vec![PropertySchema {
                            id: format!("{id}.value"),
                            label: "Value".into(),
                            scope: PropertyScope::Component {
                                component: id.into(),
                            },
                            value_type: PropertyType::Number { min: 0., max: 20. },
                            default: json!(1.),
                            requires_modules: vec![],
                            requires_components: vec![],
                        }],
                        requires_modules: vec![],
                        requires_components: vec![],
                    })
                    .unwrap();
                installed
                    .systems
                    .register(
                        SystemDescriptor {
                            id: owner.into(),
                            state_schema: format!("{owner}/1"),
                            components: vec![id.into()],
                            ports: vec![],
                            requires: vec![],
                        },
                        |_| Ok(Box::new(Noop)),
                    )
                    .unwrap();
            }
            let composition =
                CompositionContext::new(installed.modules, installed.systems).unwrap();
            for location in ["prefab", "local", "mixed"] {
                for action in ["Apply fields", "Inherit defaults"] {
                    let mut document = source_document();
                    let game = document.game.as_mut().unwrap();
                    game.execution.stages[0].nodes = ["review.foo", "review.bar"]
                        .into_iter()
                        .filter(|owner| *owner == "review.foo" || availability == "active")
                        .map(|owner| SystemNode {
                            id: owner.into(),
                            module: owner.into(),
                            params: json!({}),
                            bindings: BTreeMap::new(),
                            after: vec![],
                        })
                        .collect();
                    let mut prefab = Prefab {
                        color: [255; 4],
                        components: BTreeMap::from([("foo".into(), json!({"value":1.}))]),
                    };
                    if location != "local" {
                        prefab
                            .components
                            .insert("foo.bar".into(), json!({"value":2.,"archive":"preserved"}));
                    }
                    game.prefabs.insert("prefix".into(), prefab);
                    let object = &mut game.scenes.get_mut("empty").unwrap().objects[0];
                    object.prefab = Some("prefix".into());
                    object.components.insert("foo".into(), json!({"value":4.}));
                    if location != "prefab" {
                        object
                            .components
                            .insert("foo.bar".into(), json!({"value":6.,"local_archive":[7,8]}));
                    }
                    object.overrides = BTreeMap::from([
                        ("foo.value".into(), json!(5.)),
                        ("foo.bar.value".into(), json!(9.)),
                        ("color".into(), json!([42, 43, 44, 255])),
                    ]);
                    let sibling_local = object.components.get("foo.bar").cloned();
                    assert_eq!(
                        component_value(object, game.prefabs.get("prefix"), "foo"),
                        json!({"value":5.}),
                        "the longer override must not leak into the shorter draft"
                    );
                    composition.validate_game(&document).unwrap();
                    let mut workbench = GameWorkbench::default();
                    workbench.scene = "empty".into();
                    workbench.select(Some(1));
                    let ctx = egui::Context::default();
                    ctx.all_styles_mut(|style| style.animation_time = 0.);
                    let frame = |workbench: &mut GameWorkbench,
                                 document: &mut GraphDocument,
                                 events| {
                        ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    Vec2::new(1200., 1600.),
                                )),
                                events,
                                ..Default::default()
                            },
                            |ui| {
                                workbench.inspector_ui_with_composition(ui, document, &composition);
                            },
                        )
                    };
                    let output = frame(&mut workbench, &mut document, vec![]);
                    assert!(workbench.error.is_none(), "{:?}", workbench.error);
                    let point = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == action => {
                                Some(text.pos + text.galley.size() * 0.5)
                            }
                            _ => None,
                        })
                        .expect("ordinary Inspector action");
                    output.drop_without_applying_deltas();
                    workbench.component_drafts.get_mut("foo").unwrap()["value"] = json!(3.);
                    for pressed in [true, false] {
                        frame(
                            &mut workbench,
                            &mut document,
                            vec![
                                egui::Event::PointerMoved(point),
                                egui::Event::PointerButton {
                                    pos: point,
                                    button: egui::PointerButton::Primary,
                                    pressed,
                                    modifiers: Default::default(),
                                },
                            ],
                        )
                        .drop_without_applying_deltas();
                    }
                    composition.validate_game(&document).unwrap();
                    let path = artifacts
                        .path()
                        .join(format!("{availability}-{location}-{action}.json"));
                    module_engine_project::save_document_with_backup(&path, &document).unwrap();
                    let reopened = module_engine_project::read_document_strict(&path).unwrap();
                    composition.validate_game(&reopened).unwrap();
                    let game = reopened.game.as_ref().unwrap();
                    let object = &game.scenes["empty"].objects[0];
                    assert_eq!(
                        object.overrides["foo.bar.value"],
                        json!(9.),
                        "{availability} {location} {action}"
                    );
                    assert_eq!(object.overrides["color"], json!([42, 43, 44, 255]));
                    assert_eq!(object.components.get("foo.bar"), sibling_local.as_ref());
                    let entity = game.resolve_scene("empty").unwrap().remove(0);
                    assert_eq!(entity.components["foo.bar"]["value"], json!(9.));
                    assert_eq!(
                        entity.components["foo"]["value"],
                        json!(if action == "Apply fields" { 3. } else { 1. })
                    );
                }
            }
        }
    }

    #[test]
    fn local_component_apply_and_remove_clear_only_their_owned_overrides() {
        let mut object = AuthoredObject::new(WorldObject {
            id: 1,
            parent: None,
            transform: Transform::default(),
            color: [255; 4],
        });
        object.components = BTreeMap::from([
            ("foo".into(), json!({"value":1.})),
            ("foo.bar".into(), json!({"value":2.})),
        ]);
        object.overrides = BTreeMap::from([
            ("foo.value".into(), json!(5.)),
            ("foo.bar.value".into(), json!(9.)),
        ]);
        apply_component(&mut object, None, "foo", json!({"value":3.}));
        assert_eq!(component_value(&object, None, "foo"), json!({"value":3.}));
        reset_component(&mut object, None, "foo");
        assert_eq!(
            object.overrides,
            BTreeMap::from([("foo.bar.value".into(), json!(9.))])
        );
        assert_eq!(
            object.components,
            BTreeMap::from([("foo.bar".into(), json!({"value":2.}))])
        );
    }

    #[test]
    fn namespaced_vector_field_overrides_preserve_other_template_defaults_and_unknown_data() {
        let composition = source_composition();
        let mut document = source_document();
        let game = document.game.as_mut().unwrap();
        let base = Prefab {
            color: [255; 4],
            components: BTreeMap::from([("example.stats".into(), source_schema().defaults)]),
        };
        game.prefabs.insert("stats".into(), base.clone());
        let object = &mut game.scenes.get_mut("empty").unwrap().objects[0];
        object.prefab = Some("stats".into());
        object
            .components
            .insert("future.archive".into(), json!({"keep":[9,8,7]}));
        let mut value = component_value(object, Some(&base), "example.stats");
        value["unregistered"] = json!({"preserve":true});
        set_component_field(&mut value, "offset.1", json!(9.)).unwrap();
        apply_component(object, Some(&base), "example.stats", value);
        assert_eq!(object.overrides["example.stats.offset.1"], json!(9.));
        assert!(!object.overrides.contains_key("example.stats.offset"));
        game.prefabs
            .get_mut("stats")
            .unwrap()
            .components
            .get_mut("example.stats")
            .unwrap()["offset"] = json!([7., 2., 8.]);
        let restored: GraphDocument =
            serde_json::from_slice(&serde_json::to_vec(&document).unwrap()).unwrap();
        let entity = restored
            .game
            .as_ref()
            .unwrap()
            .resolve_scene_with_registry("empty", &composition.systems)
            .unwrap()
            .remove(0);
        assert_eq!(
            entity.components["example.stats"]["offset"],
            json!([7., 9., 8.])
        );
        assert_eq!(
            entity.components["example.stats"]["unregistered"],
            json!({"preserve":true})
        );
        assert_eq!(entity.components["future.archive"], json!({"keep":[9,8,7]}));
        let mut object = restored.game.as_ref().unwrap().scenes["empty"].objects[0].clone();
        reset_component(&mut object, Some(&base), "example.stats");
        assert!(
            !object
                .overrides
                .keys()
                .any(|key| key.starts_with("example.stats."))
        );
        assert_eq!(object.components["future.archive"], json!({"keep":[9,8,7]}));
    }

    #[test]
    fn schema_reference_choices_filter_actual_components_and_declared_file_kinds() {
        let entity = |id, components| GameEntity {
            object: WorldObject {
                id,
                parent: None,
                transform: Transform::default(),
                color: [255; 4],
            },
            components,
            references: BTreeMap::new(),
        };
        let entities = vec![
            entity(1, BTreeMap::new()),
            entity(
                2,
                BTreeMap::from([("example.stats".into(), source_schema().defaults)]),
            ),
        ];
        assert_eq!(
            object_reference_choices(
                &entities,
                &["example.stats".into()],
                &BTreeSet::from(["example.stats".into()])
            ),
            vec![2]
        );
        assert!(
            object_reference_choices(&entities, &["example.stats".into()], &BTreeSet::new())
                .is_empty()
        );
        let mut manifest = module_engine_project::AssetManifest::default();
        for (id, path) in [
            ("portrait", "assets/portrait.PNG"),
            ("sound", "assets/hit.wav"),
            ("other", "assets/data.custom"),
        ] {
            manifest.assets.insert(
                id.into(),
                module_engine_project::AssetEntry {
                    path: path.into(),
                    sha256: "hash".into(),
                    dependencies: vec![],
                },
            );
        }
        assert_eq!(
            asset_reference_choices(&manifest, &["png".into()]),
            vec![("asset://portrait".into(), "assets/portrait.PNG".into())]
        );
        assert_eq!(
            asset_reference_choices(&manifest, &["custom".into()]),
            vec![("asset://other".into(), "assets/data.custom".into())]
        );
        assert_eq!(asset_reference_choices(&manifest, &[]).len(), 3);
    }

    #[test]
    fn typed_schema_editor_renders_declared_fields_and_keeps_unknown_values_unchanged() {
        let mut schema = source_schema();
        let mut hidden = schema.fields[0].clone();
        hidden.id = "example.stats.hidden".into();
        hidden.label = "Unavailable field".into();
        hidden.requires_modules = vec!["not.active".into()];
        schema.fields.push(hidden);
        let mut value = schema.defaults.clone();
        value["unknown"] = json!({"preserve":[true,42]});
        let original = value.clone();
        let modules = BTreeSet::new();
        let attached = BTreeSet::from([schema.id.clone()]);
        let manifest = module_engine_project::AssetManifest::default();
        let fields = ComponentEditorContext {
            modules: &modules,
            components: &attached,
            entities: &[],
            manifest: &manifest,
        };
        let output = egui::Context::default().run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1000., 1000.),
                )),
                ..Default::default()
            },
            |ui| component_editor(ui, &schema, &mut value, &attached, &fields),
        );
        for label in [
            "Travel speed",
            "Movement mode",
            "Enabled",
            "Title",
            "Offset X",
            "Offset Y",
            "Offset Z",
            "Target",
            "Portrait",
        ] {
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)), "missing schema label {label}");
        }
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Unavailable field" || text.galley.text() == "unknown")));
        assert_eq!(value, original);
        output.drop_without_applying_deltas();
    }

    #[test]
    fn game_rust_editor_shows_its_own_errors_and_applies_reset_and_migration_to_game_slots() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let id = "game-controller";
        let first = "// module-engine-state: {\"schema\":\"ui.a\",\"initial\":{\"ticks\":0}}\nfn update(ctx: &mut ScriptContext) { ctx.state[\"ticks\"] = serde_json::json!(ctx.state[\"ticks\"].as_u64().unwrap() + 1); }";
        let second = first.replace("ui.a", "ui.b");
        let third = first.replace("ui.a", "ui.c");
        let mut document = native_test_document();
        document.custom_sources.insert(id.into(), first.into());
        let mut workbench = GameWorkbench::default();
        workbench
            .ensure(&document, &root, Some(&root), WorldKind::Ecs)
            .unwrap();
        workbench.tick(1. / 60., true).unwrap();
        let old_state = workbench.host.as_ref().unwrap().slots[id]
            .lock()
            .unwrap()
            .state()
            .unwrap();
        let compiler = ScriptCompiler::new(root.clone(), root.join(".work/game-ui-custom-state"));
        let candidate = compiler.compile_kind(&second, ScriptKind::Update).unwrap();
        let (send, receiver) = mpsc::channel();
        send.send(Ok(candidate)).unwrap();
        workbench.pending = Some((id.into(), receiver));
        workbench.tick(0., false).unwrap();
        assert!(workbench.rejected.is_some());
        assert_eq!(
            workbench.host.as_ref().unwrap().slots[id]
                .lock()
                .unwrap()
                .state()
                .unwrap(),
            old_state
        );
        assert!(
            workbench
                .apply_rejected_custom(id, &third, None)
                .unwrap_err()
                .contains("устарел")
        );
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.);
        let frame = |workbench: &mut GameWorkbench, source: &str, events| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1100., 900.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| workbench.custom_editor_ui(ui, id, Some(source), Some(&root)),
            )
        };
        let output = frame(&mut workbench, &second, vec![]);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("предыдущая скомпилированная версия"))));
        let point = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "Применить со сбросом состояния" =>
                {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("Game reset is available in the shared Rust UI");
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            frame(
                &mut workbench,
                &second,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            )
            .drop_without_applying_deltas();
        }
        assert!(workbench.rejected.is_none());
        assert_eq!(
            workbench.host.as_ref().unwrap().slots[id]
                .lock()
                .unwrap()
                .source(),
            second
        );
        assert_eq!(
            workbench.host.as_ref().unwrap().slots[id]
                .lock()
                .unwrap()
                .state()
                .unwrap(),
            json!({"ticks":0})
        );
        workbench.rejected = Some((
            id.into(),
            compiler.compile_kind(&third, ScriptKind::Update).unwrap(),
        ));
        assert!(
            workbench
                .apply_rejected_custom(id, &third, Some(json!({"ticks":"wrong"})))
                .is_err()
        );
        assert_eq!(
            workbench.host.as_ref().unwrap().slots[id]
                .lock()
                .unwrap()
                .source(),
            second
        );
        workbench
            .apply_rejected_custom(id, &third, Some(json!({"ticks":37})))
            .unwrap();
        assert_eq!(
            workbench.host.as_ref().unwrap().slots[id]
                .lock()
                .unwrap()
                .state()
                .unwrap(),
            json!({"ticks":37})
        );
        let (send, receiver) = mpsc::channel();
        send.send(Err("Game compiler diagnostic from its own backend".into()))
            .unwrap();
        workbench.pending = Some((id.into(), receiver));
        workbench.tick(0., false).unwrap();
        let output = frame(&mut workbench, &third, vec![]);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Game compiler diagnostic from its own backend")));
        output.drop_without_applying_deltas();
        let (_send, receiver) = mpsc::channel();
        workbench.pending = Some((id.into(), receiver));
        let previous_error = workbench.error.clone();
        let output = frame(&mut workbench, &third, vec![]);
        let apply = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Применить Rust" => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .unwrap();
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            frame(
                &mut workbench,
                &third,
                vec![
                    egui::Event::PointerMoved(apply),
                    egui::Event::PointerButton {
                        pos: apply,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            )
            .drop_without_applying_deltas();
        }
        assert!(workbench.pending.is_some());
        assert_eq!(
            workbench.error, previous_error,
            "disabled Apply must not submit or overwrite the pending operation"
        );
    }
    #[test]
    fn scene_copy_template_creation_and_instance_fields_survive_save_and_template_edits() {
        let mut game = GameProject::demo();
        let copied = create_scene(&mut game, "arena", "another", true).unwrap();
        assert_eq!(game.scenes[&copied].objects, game.scenes["arena"].objects);
        let entity = game
            .resolve_scene("arena")
            .unwrap()
            .into_iter()
            .find(|entity| entity.object.id == 10)
            .unwrap();
        let template = create_template(&mut game, "arena", &entity, "linked").unwrap();
        let base = game.prefabs[&template].clone();
        let object = game
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects
            .iter_mut()
            .find(|o| o.object.id == 10)
            .unwrap();
        apply_component(
            object,
            Some(&base),
            "health",
            json!({"maximum":100.,"current":36.}),
        );
        assert_eq!(object.overrides["health.maximum"], 100.);
        assert!(!object.components.contains_key("health"));
        game.prefabs
            .get_mut(&template)
            .unwrap()
            .components
            .get_mut("health")
            .unwrap()["maximum"] = json!(60.);
        game.scenes
            .get_mut("arena")
            .unwrap()
            .objects
            .iter_mut()
            .find(|o| o.object.id == 11)
            .unwrap()
            .prefab = Some(template.clone());
        let json = serde_json::to_string(&game).unwrap();
        let restored: GameProject = serde_json::from_str(&json).unwrap();
        let resolved = restored.resolve_scene("arena").unwrap();
        assert_eq!(
            resolved
                .iter()
                .find(|e| e.object.id == 10)
                .unwrap()
                .components["health"]["maximum"],
            100.
        );
        assert_eq!(
            resolved
                .iter()
                .find(|e| e.object.id == 11)
                .unwrap()
                .components["health"]["maximum"],
            60.
        );
        restored
            .validate_with_registry(&module_engine_arpg::registry())
            .unwrap();
    }
    #[test]
    fn texture_attachment_and_unlink_preserve_individual_fields_and_inactive_data() {
        let prefab = Prefab {
            color: [255; 4],
            components: BTreeMap::from([
                (
                    "material".into(),
                    json!({"base_color":[1.,1.,1.,1.],"base_color_texture":"assets/base.png"}),
                ),
                ("network".into(), json!({"replication":true})),
            ]),
        };
        let mut object = AuthoredObject::new(WorldObject {
            id: 1,
            parent: None,
            transform: Transform::default(),
            color: [255; 4],
        });
        object.prefab = Some("p".into());
        object
            .overrides
            .insert("material.base_color".into(), json!([0.2, 0.3, 0.4, 1.]));
        attach_asset(&mut object, Some(&prefab), "assets/new.png").unwrap();
        assert_eq!(
            component_value(&object, Some(&prefab), "material")["base_color"],
            json!([0.2, 0.3, 0.4, 1.])
        );
        assert_eq!(object.overrides["material.base_color.0"], json!(0.2));
        assert_eq!(object.overrides["material.base_color.1"], json!(0.3));
        assert_eq!(object.overrides["material.base_color.2"], json!(0.4));
        assert_eq!(
            object.overrides["material.base_color_texture"],
            "assets/new.png"
        );
        let resolved = GameEntity {
            object: object.object.clone(),
            components: BTreeMap::from([
                (
                    "material".into(),
                    component_value(&object, Some(&prefab), "material"),
                ),
                ("network".into(), json!({"replication":true})),
            ]),
            references: BTreeMap::new(),
        };
        object.prefab = None;
        link_template(&mut object, &resolved, &BTreeMap::new());
        assert_eq!(object.components, resolved.components);
        assert!(object.overrides.is_empty());
        let composition = crate::host::composition_at(Path::new(".")).unwrap();
        let mut document = demo_document(WorldKind::Objects);
        document.game = Some(GameProject::demo());
        let visible = composition.activity(&document).unwrap().components;
        assert!(visible.contains("material"));
        assert!(!visible.contains("network"));
        assert_eq!(object.components["network"]["replication"], true);
    }
    #[test]
    fn disconnected_graph_can_be_repaired_and_wrong_pin_or_execution_order_is_rejected() {
        let registry = crate::host::composition_at(Path::new(".")).unwrap().systems;
        let mut graph = ExecutionGraph::gameplay();
        graph.stages[0].nodes[0].bindings.remove("input");
        assert!(registry.validate(&graph).is_err());
        let before = graph.clone();
        assert!(
            connect_pins(
                &mut graph,
                &registry,
                "external-resource::world",
                "world",
                "system-0",
                "input"
            )
            .is_err()
        );
        assert_eq!(graph, before);
        connect_pins(
            &mut graph,
            &registry,
            "external-resource::input",
            "input",
            "system-0",
            "input",
        )
        .unwrap();
        registry.validate(&graph).unwrap();
        assert!(
            connect_pins(
                &mut graph, &registry, "system-1", "world", "system-0", "world"
            )
            .is_err()
        );
        move_item(&mut graph.stages, 2, true);
        assert!(registry.validate(&graph).is_err());
        linear_after(&mut graph);
        assert!(
            registry.validate(&graph).is_err(),
            "Positions still cannot be read before movement produces them"
        );
    }
    #[test]
    fn removing_disconnected_ecs_does_not_replace_playing_object_host() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let mut document = module_engine_core::demo_document(WorldKind::Objects);
        document.game = Some(GameProject::empty());
        document.root.nodes.push(make_node(
            "archived-ecs",
            "world.ecs",
            "Archived ECS",
            [0., 0.],
            json!({}),
        ));
        crate::engine_profiles::finish_template(
            &mut document,
            crate::engine_profiles::ProfileKind::Neutral,
            &root,
        )
        .unwrap();
        let host = GameHost::build(
            document.clone(),
            crate::host::composition_at(&root).unwrap(),
            module_engine_host::HostOptions {
                asset_root: root.clone(),
                backend_root: Some(root.clone()),
                compile_root: None,
                render: module_engine_host::RenderMode::Graph,
                services: module_engine_host_services::HostServiceRegistry::new(),
                services_enabled: false,
            },
            None,
        )
        .unwrap();
        let mut workbench = GameWorkbench::default();
        assert!(workbench.host.replace(host).is_none());
        workbench.game_document = Some(document.clone());
        workbench.set_engine_context(None, &root);
        workbench.set_deferred_preparation(true);
        workbench.tick(1. / 60., true).unwrap();
        let before = workbench.host.as_ref().unwrap().session.state().clone();
        document.root.nodes.retain(|node| node.id != "archived-ecs");
        let kind = game_host::world_kind(&document).unwrap();
        assert_eq!(kind, WorldKind::Objects);
        workbench.ensure(&document, &root, None, kind).unwrap();
        workbench.sync_graphics(&document).unwrap();
        assert!(workbench.game_preparation.is_none());
        let host = workbench.host.as_ref().unwrap();
        assert_eq!(host.session.world_kind(), WorldKind::Objects);
        assert_eq!(host.session.state(), &before);
        assert_eq!(host.accepted_document(), &document);
    }
    #[test]
    fn owned_custom_failure_and_cancel_keep_last_good_instance_running() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let artifacts = module_engine_script::TestArtifactGuard::new(
            &root.join(".work/test-temp"),
            "game-ui-owned-compile",
        )
        .unwrap();
        let id = "game-controller";
        let source = "// module-engine-state: {\"schema\":\"owned.ui/1\",\"initial\":{\"ticks\":0}}\nfn update(ctx: &mut ScriptContext) { ctx.state[\"ticks\"] = serde_json::json!(ctx.state[\"ticks\"].as_u64().unwrap() + 1); }";
        let mut document = native_test_document();
        document.custom_sources.insert(id.into(), source.into());
        let composition = crate::host::composition_at(&root).unwrap();
        let world_node = composition
            .modules
            .render_route(&document)
            .unwrap()
            .world
            .id
            .clone();
        let host = GameHost::build(
            document,
            composition,
            module_engine_host::HostOptions {
                asset_root: root.clone(),
                backend_root: None,
                compile_root: Some(root.clone()),
                render: module_engine_host::RenderMode::Headless { world_node },
                services: module_engine_host_services::HostServiceRegistry::new(),
                services_enabled: false,
            },
            None,
        )
        .unwrap();
        let mut workbench = GameWorkbench::default();
        assert!(workbench.host.replace(host).is_none());
        workbench.tick(1. / 60., true).unwrap();
        let original = workbench.host.as_ref().unwrap().slots[id].clone();
        let before = original.lock().unwrap().state().unwrap();
        workbench.request_custom(
            id,
            "// module-engine-state: not json\nfn update(ctx: &mut ScriptContext) {}",
            &root,
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while workbench.pending.is_some() {
            workbench.tick(0., false).unwrap();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(workbench.custom_errors.contains_key(id));
        assert_eq!(original.lock().unwrap().source(), source);
        assert_eq!(original.lock().unwrap().state().unwrap(), before);

        let compiler = ScriptCompiler::new(root, artifacts.path().join("compile"));
        let source = source.to_owned();
        let (entered, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (observed, result) = mpsc::channel();
        let (receiver, lease) = module_engine_script::spawn_job(move |cancelled| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            let result = compiler
                .compile_kind_cancellable(&source, ScriptKind::Update, cancelled)
                .map_err(|error| error.to_string());
            observed.send(result.as_ref().unwrap_err().clone()).unwrap();
            result
        });
        workbench.pending = Some((id.into(), receiver));
        workbench.pending_job = Some(lease);
        started.recv_timeout(Duration::from_secs(10)).unwrap();
        // Cancellation is the same path used by Stop/Drop. The gated producer
        // proves it returns without joining while the installed version lives.
        workbench.cancel_pending_custom();
        assert!(workbench.pending.is_none());
        release.send(()).unwrap();
        assert!(
            result
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .contains("cancelled")
        );
        assert_eq!(original.lock().unwrap().state().unwrap(), before);
        workbench.tick(1. / 60., true).unwrap();
        assert_eq!(
            original.lock().unwrap().state().unwrap()["ticks"],
            before["ticks"].as_u64().unwrap() + 1
        );
    }
    #[test]
    fn headless_graph_editor_keeps_incomplete_draft_out_of_saved_document() {
        let mut document = demo_document(WorldKind::Objects);
        document.game = Some(GameProject::demo());
        let saved = document.game.clone();
        let mut workbench = GameWorkbench::default();
        let mut draft = document.game.as_ref().unwrap().execution.clone();
        draft.stages[0].nodes[0].bindings.clear();
        workbench.graph_draft = Some(draft);
        workbench.graph_base = Some(document.game.as_ref().unwrap().execution.clone());
        let context = egui::Context::default();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1400., 1000.),
                )),
                ..Default::default()
            },
            |ui| {
                assert!(!workbench.systems_ui(ui, &mut document, &root));
            },
        );
        assert!(!output.shapes.is_empty());
        output.textures_delta.clear();
        assert_eq!(document.game, saved);
        assert!(
            workbench.graph_draft.as_ref().unwrap().stages[0].nodes[0]
                .bindings
                .is_empty()
        );
    }
}
