//! Authored scenes and staged execution graphs. Play never writes authored data.
use crate::{
    activity::{ActivitySnapshot, BuildProgress},
    canvas::{self, CanvasAction, CanvasEdge, CanvasNode, CanvasPort, CanvasState},
    game_host::{self, GameHost},
    player,
};
use eframe::egui::{self, Color32, Vec2};
use module_engine_core::*;
use module_engine_script::{CompiledScript, ScriptCompiler, ScriptKind};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};
struct HostPreparation {
    document: GraphDocument,
    context: PreparationContext,
    receiver: mpsc::Receiver<Result<GameHost, String>>,
    progress: BuildProgress,
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
/// A successful frame may retire its own previous error, but must preserve a
/// newer diagnostic from Custom, authoring or asynchronous preparation.
fn update_render_diagnostic(
    error: &mut Option<String>,
    render_error: &mut Option<String>,
    result: Result<(), String>,
) {
    match result {
        Err(message) => {
            *render_error = Some(message.clone());
            *error = Some(message);
        }
        Ok(()) => {
            if render_error.is_some() && error.as_ref() == render_error.as_ref() {
                *error = None;
            }
            *render_error = None;
        }
    }
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
    pub rejected: Option<(String, CompiledScript)>,
    migration: String,
    source_selected: String,
    compile_root: Option<PathBuf>,
    backend_root: Option<PathBuf>,
    world_kind: WorldKind,
    play_session: bool,
    deferred_preparation: bool,
    game_preparation: Option<HostPreparation>,
    preview_preparation: Option<HostPreparation>,
    game_activity: Option<BuildProgress>,
    preview_activity: Option<BuildProgress>,
    custom_activity: Option<BuildProgress>,
    rejected_game: Option<(GraphDocument, PreparationContext, String)>,
    rejected_preview: Option<(GraphDocument, PreparationContext, String)>,
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
            rejected: None,
            migration: "null".into(),
            source_selected: String::new(),
            compile_root: None,
            backend_root: None,
            world_kind: WorldKind::Objects,
            play_session: false,
            deferred_preparation: false,
            game_preparation: None,
            preview_preparation: None,
            game_activity: None,
            preview_activity: None,
            custom_activity: None,
            rejected_game: None,
            rejected_preview: None,
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
        for slot in [&mut self.game_preparation, &mut self.preview_preparation] {
            if let Some(job) = slot.take() {
                job.progress.succeed("Редактор закрыл запрос подготовки");
                std::thread::spawn(move || drop(job));
            }
        }
        if self.deferred_preparation {
            for slot in [&mut self.host, &mut self.preview_host] {
                if let Some(mut host) = slot.take() {
                    host.audio.stop_endpoint();
                    std::thread::spawn(move || drop(host));
                }
            }
        }
    }
}
impl GameWorkbench {
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
        activities
    }
    pub fn has_pending_preparation(&self) -> bool {
        self.game_preparation.is_some()
            || self.preview_preparation.is_some()
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
        if !self.deferred_preparation {
            return Ok(());
        }
        let kind = if document
            .root
            .nodes
            .iter()
            .any(|node| node.module == "world.ecs")
        {
            WorldKind::Ecs
        } else {
            WorldKind::Objects
        };
        let mut first_error = self.poll_host_preparation(document, kind, false).err();
        if self
            .rejected_game
            .as_ref()
            .is_some_and(|(failed, _, _)| !failed.execution_eq(document))
        {
            self.rejected_game = None;
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
                    .any(|activity| activity.state == crate::activity::ActivityState::Failed);
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
            std::thread::spawn(move || drop(result));
            return Ok(());
        }
        match result {
            Ok(mut host) => {
                // Metadata may have moved while its execution stayed the same.
                if let Err(error) = host.sync_document(document) {
                    job.progress.fail(error.clone());
                    std::thread::spawn(move || drop(host));
                    return Err(error);
                }
                host.audio.enabled = !preview;
                if !preview {
                    Self::frame_game_start(&mut host);
                }
                let old = if preview {
                    self.preview_document = Some(document.clone());
                    self.accepted_preview_context = Some(job.context.clone());
                    self.rejected_preview = None;
                    self.preview_host.replace(host)
                } else {
                    self.game_document = Some(document.clone());
                    self.accepted_game_context = Some(job.context.clone());
                    self.rejected_game = None;
                    self.host.replace(host)
                };
                if let Some(mut old) = old {
                    old.audio.stop_endpoint();
                    std::thread::spawn(move || drop(old));
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
                let failed = Some((document.clone(), job.context, error.clone()));
                if preview {
                    self.rejected_preview = failed;
                } else {
                    self.rejected_game = failed;
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
        let (send, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = GameHost::new_at_with_progress(
                worker_document,
                &root,
                &backend,
                compile.as_deref(),
                kind,
                Some(&worker_progress),
            )
            .map(|mut host| {
                host.audio.enabled = false;
                host
            });
            let _ = send.send(result);
        });
        *slot = Some(HostPreparation {
            document: candidate,
            context,
            receiver,
            progress: progress.clone(),
        });
        if preview {
            self.preview_activity = Some(progress);
            self.rejected_preview = None;
        } else {
            self.game_activity = Some(progress);
            self.rejected_game = None;
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
                    std::thread::spawn(move || drop(job));
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
        if !self.play_session
            && let Some(host) = &mut self.host
        {
            host.invalidate_content();
        }
    }
    pub fn stop(&mut self) {
        if let Some(job) = self.game_preparation.take() {
            job.progress.succeed("Подготовка отменена после Stop");
            std::thread::spawn(move || drop(job));
        }
        if self.deferred_preparation {
            if let Some(mut host) = self.host.take() {
                host.audio.stop_endpoint();
                host.cancel_preparation();
                std::thread::spawn(move || drop(host));
            }
        } else {
            self.host = None;
        }
        self.rejected_game = None;
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
        self.pending = None;
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
        let mut host = GameHost::new_at(candidate.clone(), root, &backend, compile_root, kind)?;
        Self::frame_game_start(&mut host);
        self.host = Some(host);
        self.game_document = Some(candidate);
        Ok(())
    }
    /// Game follows its authored starting player while retaining its own orbit.
    fn frame_game_start(host: &mut GameHost) {
        if let Some(player) = host
            .session
            .state()
            .entities
            .values()
            .find(|entity| entity.components.contains_key("player"))
        {
            host.camera.target = player.object.transform.translation;
        }
    }
    /// Compile edited source in the background; current instances keep running.
    pub fn request_custom(&mut self, id: &str, source: &str, compile_root: &Path) {
        if self.pending.is_some() {
            self.error = Some("A Custom compilation is already running".into());
            return;
        }
        let root = compile_root.to_path_buf();
        let source = source.to_owned();
        let progress = BuildProgress::new(format!("Rust Custom {id}"));
        let worker_progress = progress.clone();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            worker_progress.stage("Компиляция Rust; текущая версия продолжает работать");
            let compiler = ScriptCompiler::new(root.clone(), root.join(".work/game-scripts"));
            let _ = send.send(
                compiler
                    .compile_kind(&source, ScriptKind::Update)
                    .map_err(|e| e.to_string()),
            );
        });
        self.pending = Some((id.into(), receive));
        self.custom_activity = Some(progress);
    }
    pub fn tick(&mut self, dt: f64, playing: bool) -> Result<(), String> {
        // Each viewport contributes to one UI frame. Consuming the full input
        // prevents hidden panes or a newly focused code editor retaining WASD.
        let input = std::mem::take(&mut self.input);
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
                    self.pending = None;
                    match result {
                        Ok(candidate) => {
                            if let Some(host) = &mut self.host {
                                match host.apply_candidate(&id, candidate.clone(), false) {
                                    Ok(()) => {
                                        self.error = None;
                                        self.rejected = None;
                                        if let Some(progress) = &self.custom_activity {
                                            progress.succeed("Новая версия Rust применена");
                                        }
                                    }
                                    Err(e) => {
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
                                self.error = Some(error);
                            }
                        }
                        Err(e) => {
                            if let Some(progress) = &self.custom_activity {
                                progress.fail(e.clone());
                            }
                            self.error = Some(e);
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.error = Some("Rust compilation thread ended without a result".into());
                    if let Some(progress) = &self.custom_activity {
                        progress.fail("Rust compilation thread ended without a result");
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if playing
            && !waiting_for_host
            && let Some(host) = &mut self.host
        {
            host.step(dt, &input)?;
        }
        Ok(())
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
        let mut host = GameHost::new_at(candidate.clone(), root, &backend, compile_root, kind)?;
        host.audio.enabled = false;
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
            if ui.button("Restart").clicked()
                && let Some(host) = &mut self.host
                && let Err(error) = host.session.restart()
            {
                self.error = Some(error);
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
        if let Some(host) = &mut self.host {
            let size = ui.available_size().max(Vec2::splat(1.));
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            let input = if *playing {
                player::read_input(ui, &response, rect, host.camera)
            } else {
                GameInput::default()
            };
            let aim = (*playing
                && response.hovered()
                && ui.input(|i| i.focused)
                && (!ui.ctx().egui_wants_keyboard_input() || response.has_focus()))
            .then_some(input.aim_world);
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
                player::draw_hud(ui, rect, host, aim);
            }
        } else if self.game_preparation.is_some() {
            ui.spinner();
            ui.label("Game готовится в фоне. Можно продолжать работать с редактором.");
        }
        false
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
        let Some(original) = document.game.clone() else {
            ui.label("No authored scene");
            return false;
        };
        let mut game = original.clone();
        self.normalize_selection(&game);
        let resolved = game.resolve_scene(&self.scene).unwrap_or_default();
        let active = active_components(
            &game.execution,
            &SystemRegistry::builtin(),
            renderer_active(
                document,
                self.backend_root.as_deref().unwrap_or(Path::new(".")),
            ),
        );
        let manifest =
            match module_engine_project::AssetManifest::from_value(&document.asset_manifest) {
                Ok(manifest) => manifest,
                Err(error) => {
                    self.error = Some(error);
                    return false;
                }
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
                let templates=game.prefabs.clone();let defaults=component_examples(&game);
                let parents=game.scenes[&self.scene].objects.iter().filter(|object|object.object.id!=id).map(|object|object.object.id).collect::<Vec<_>>();
                let object=game.scenes.get_mut(&self.scene).unwrap().objects.iter_mut().find(|object|object.object.id==id).unwrap();
                let prefab=object.prefab.as_ref().and_then(|name|templates.get(name));
                if self.field_key.as_ref()!=Some(&(self.scene.clone(),id)){
                    self.component_drafts=entity.components.clone();
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
                    let visible=self.component_drafts.keys().filter(|name|active.contains(*name)).cloned().collect::<Vec<_>>();
                    for name in visible{ui.push_id(&name,|ui|{ui.collapsing(&name,|ui|{
                        let draft=self.component_drafts.get_mut(&name).unwrap();edit_value(ui,draft);let value=draft.clone();
                        ui.horizontal(|ui|{
                            if ui.button("Apply fields").clicked(){apply_component(object,prefab,&name,value);self.field_key=None;}
                            if prefab.is_some_and(|prefab|prefab.components.contains_key(&name)){
                                if ui.button("Inherit defaults").clicked(){reset_component(object,&name);self.field_key=None;}
                            }else if ui.button("Remove component").clicked(){object.components.remove(&name);self.component_drafts.remove(&name);self.field_key=None;}
                        });
                    });});}
                    egui::ComboBox::from_id_salt("add-game-component").selected_text("+ Component from active modules").show_ui(ui,|ui|{
                        for name in &active{if !self.component_drafts.contains_key(name)&&ui.button(name).clicked(){self.component_drafts.insert(name.clone(),defaults.get(name).cloned().unwrap_or_else(||match name.as_str(){"mesh"=>json!({"path":""}),"material"=>json!({"base_color":[1.,1.,1.,1.]}),_=>json!({})}));}}
                    });
                    let hidden=entity.components.keys().filter(|name|!active.contains(*name)).count();if hidden>0{ui.label(format!("{hidden} inactive component records preserved; see Advanced"));}
                    ui.collapsing("Advanced: components / overrides / references",|ui|{
                        ui.add(egui::TextEdit::multiline(&mut self.advanced).code_editor().desired_rows(12).desired_width(f32::INFINITY));
                        if ui.button("Apply raw JSON").clicked(){match apply_raw_object(object,&self.advanced){Ok(())=>self.field_key=None,Err(error)=>self.error=Some(error)}}
                    });
                });
                if active.contains("mesh")||active.contains("material"){self.asset_ui(ui,&manifest,object,prefab,!self.play_session);}
                ui.add_enabled_ui(!self.play_session,|ui|{ui.horizontal_wrapped(|ui|{ui.label("New linked template");ui.text_edit_singleline(&mut self.template_name);});});
                if ui.add_enabled(!self.play_session,egui::Button::new("Create template from selected")).clicked(){match create_template(&mut game,&self.scene,id,&self.template_name){Ok(_)=>{self.template_name.clear();self.template_key=None;self.field_key=None;},Err(error)=>self.error=Some(error)}}
                let template_name=game.scenes[&self.scene].objects.iter().find(|object|object.object.id==id).and_then(|object|object.prefab.clone());
                if let Some(name)=template_name&&let Some(template)=game.prefabs.get(&name){
                    if self.template_key.as_deref()!=Some(&name){self.template_draft=Some(template.clone());self.template_key=Some(name.clone());}
                    ui.collapsing(format!("Linked template source: {name}"),|ui|{ui.add_enabled_ui(!self.play_session,|ui|{
                        if let Some(draft)=&mut self.template_draft{ui.color_edit_button_srgba_unmultiplied(&mut draft.color);for(component,value)in&mut draft.components{if active.contains(component){ui.push_id(format!("template-{component}"),|ui|{ui.collapsing(component,|ui|edit_value(ui,value));});}}}
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
    fn custom_ui(
        &mut self,
        ui: &mut egui::Ui,
        document: &GraphDocument,
        compile_root: Option<&Path>,
    ) {
        if let Some((id, _)) = &self.pending {
            ui.label(format!("Compiling {id}; active code continues"));
        }
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
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("game-custom-source")
                .selected_text(&self.source_selected)
                .show_ui(ui, |ui| {
                    for id in ids {
                        ui.selectable_value(&mut self.source_selected, id.clone(), id);
                    }
                });
            if ui
                .add_enabled(
                    self.pending.is_none() && compile_root.is_some(),
                    egui::Button::new("Apply selected game Rust"),
                )
                .clicked()
                && let Some(root) = compile_root
                && let Some(source) = document.custom_sources.get(&self.source_selected)
            {
                let id = self.source_selected.clone();
                self.request_custom(&id, source, root);
            }
        });
        if let Some((id, candidate)) = self.rejected.clone() {
            ui.collapsing("Rejected state schema: explicit reset or migration", |ui| {
                if ui
                    .button("Apply rejected version + reset its state")
                    .clicked()
                    && let Some(host) = &mut self.host
                {
                    let progress = BuildProgress::new(format!("Rust {id}: сброс состояния"));
                    progress.stage("Применение версии с явным сбросом состояния");
                    match host.apply_candidate(&id, candidate.clone(), true) {
                        Ok(()) => {
                            self.rejected = None;
                            self.error = None;
                            progress.succeed("Версия применена, состояние сброшено");
                        }
                        Err(e) => {
                            progress.fail(e.clone());
                            self.error = Some(e);
                        }
                    }
                    self.custom_activity = Some(progress);
                }
                ui.label("Replacement state JSON (all instances of this source)");
                ui.add(
                    egui::TextEdit::multiline(&mut self.migration)
                        .code_editor()
                        .desired_rows(4),
                );
                if ui.button("Apply version with migrated state").clicked()
                    && let Some(host) = &mut self.host
                {
                    let progress = BuildProgress::new(format!("Rust {id}: миграция состояния"));
                    progress.stage("Проверка и применение указанного состояния");
                    match serde_json::from_str(&self.migration)
                        .map_err(|e| e.to_string())
                        .and_then(|state| host.apply_candidate_migrated(&id, &candidate, state))
                    {
                        Ok(()) => {
                            self.rejected = None;
                            self.error = None;
                            progress.succeed("Версия и миграция применены");
                        }
                        Err(e) => {
                            progress.fail(e.clone());
                            self.error = Some(e);
                        }
                    }
                    self.custom_activity = Some(progress);
                }
            });
        }
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
        self.world_kind = if document
            .root
            .nodes
            .iter()
            .any(|node| node.module == "world.ecs")
        {
            WorldKind::Ecs
        } else {
            WorldKind::Objects
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
        let mut copy = document.clone();
        copy.game = None;
        let slots = self
            .host
            .as_ref()
            .map(|h| h.slots.clone())
            .unwrap_or_default();
        let registry = match game_host::game_registry_at(
            &copy,
            root,
            self.backend_root.as_deref().unwrap_or(root),
            &slots,
            std::sync::Arc::new(std::sync::Mutex::new(RenderOverrides::default())),
        ) {
            Ok(r) => r,
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
        match GameHost::new_at(
            runtime_document.clone(),
            root,
            &backend,
            compile.as_deref(),
            self.world_kind,
        ) {
            Ok(mut host) => {
                let changed = candidate.game != document.game;
                document.game = candidate.game;
                host.audio.enabled = false;
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
fn renderer_active(document: &GraphDocument, root: &Path) -> bool {
    let Ok(registry) = crate::host::registry_at(root) else {
        return false;
    };
    document
        .root
        .nodes
        .iter()
        .filter_map(|n| registry.descriptor(&n.module))
        .any(|d| {
            d.inputs.iter().any(|p| p.data_type == "Scene3D")
                && d.outputs.iter().any(|p| p.data_type == "Frame3D")
        })
}
fn active_components(
    graph: &ExecutionGraph,
    registry: &SystemRegistry,
    renderer: bool,
) -> BTreeSet<String> {
    let mut names = graph
        .stages
        .iter()
        .flat_map(|s| &s.nodes)
        .flat_map(|n| {
            registry
                .descriptor(&n.module)
                .into_iter()
                .flat_map(|d| d.components.iter().cloned())
                .chain(
                    n.params
                        .get("components")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned),
                )
        })
        .collect::<BTreeSet<_>>();
    if renderer {
        names.extend(["mesh".into(), "material".into()]);
    }
    names
}
fn merge_view_input(current: &mut GameInput, next: GameInput, active: bool) {
    if !active {
        return;
    }
    let attack = current.attack || next.attack;
    let interact = current.interact || next.interact;
    let restart = current.restart || next.restart;
    *current = next;
    current.attack = attack;
    current.interact = interact;
    current.restart = restart;
}
fn component_examples(game: &GameProject) -> BTreeMap<String, Value> {
    let mut values = BTreeMap::new();
    for components in game.prefabs.values().map(|p| &p.components).chain(
        game.scenes
            .values()
            .flat_map(|s| &s.objects)
            .map(|o| &o.components),
    ) {
        for (name, value) in components {
            values.entry(name.clone()).or_insert(value.clone());
        }
    }
    values
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
    id: u64,
    name: &str,
) -> Result<String, String> {
    let entity = game
        .resolve_scene(scene)?
        .into_iter()
        .find(|e| e.object.id == id)
        .ok_or("Selected object missing")?;
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
            components: entity.components,
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
fn reset_component(object: &mut AuthoredObject, name: &str) {
    object.components.remove(name);
    object
        .overrides
        .retain(|key, _| !key.starts_with(&format!("{name}.")));
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
        let Some((component, field)) = path.split_once('.') else {
            continue;
        };
        let base = component_value(object, Some(template), component);
        let exists = field
            .split('.')
            .try_fold(&base, |v, key| v.get(key))
            .is_some();
        if !exists {
            let local = object
                .components
                .entry(component.into())
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
    for (path, field) in &object.overrides {
        if let Some(path) = path.strip_prefix(&format!("{name}.")) {
            put_field(&mut value, path, field.clone());
        }
    }
    value
}
fn value_leaves(value: &Value, prefix: &str, result: &mut BTreeMap<String, Value>) {
    if let Value::Object(fields) = value {
        for (field, v) in fields {
            let path = if prefix.is_empty() {
                field.clone()
            } else {
                format!("{prefix}.{field}")
            };
            value_leaves(v, &path, result);
        }
    } else {
        result.insert(prefix.into(), value.clone());
    }
}
fn put_field(value: &mut Value, path: &str, new: Value) {
    let (head, tail) = path
        .split_once('.')
        .map_or((path, None), |(h, t)| (h, Some(t)));
    if !value.is_object() {
        *value = json!({});
    }
    if let Some(tail) = tail {
        put_field(
            value
                .as_object_mut()
                .unwrap()
                .entry(head)
                .or_insert_with(|| json!({})),
            tail,
            new,
        );
    } else {
        value.as_object_mut().unwrap().insert(head.into(), new);
    }
}
fn apply_component(object: &mut AuthoredObject, prefab: Option<&Prefab>, name: &str, value: Value) {
    let Some(base) = prefab.and_then(|p| p.components.get(name)) else {
        object.components.insert(name.into(), value);
        return;
    };
    let mut defaults = BTreeMap::new();
    let mut edited = BTreeMap::new();
    value_leaves(base, "", &mut defaults);
    value_leaves(&value, "", &mut edited);
    reset_component(object, name);
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
fn edit_value(ui: &mut egui::Ui, value: &mut Value) {
    match value {
        Value::Object(fields) => {
            if fields.is_empty() {
                ui.label("Marker component (no fields)");
            }
            for (name, value) in fields {
                ui.push_id(name, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        edit_value(ui, value);
                    });
                });
            }
        }
        Value::Array(values) => {
            ui.horizontal_wrapped(|ui| {
                for (i, value) in values.iter_mut().enumerate() {
                    ui.push_id(i, |ui| edit_value(ui, value));
                }
            });
        }
        Value::Number(n) => {
            let mut v = n.as_f64().unwrap_or(0.);
            if ui
                .add(egui::DragValue::new(&mut v).speed(0.1).max_decimals(3))
                .changed()
            {
                *value = json!(v);
            }
        }
        Value::Bool(v) => {
            ui.checkbox(v, "");
        }
        Value::String(v) => {
            ui.add(egui::TextEdit::singleline(v).desired_width(150.));
        }
        Value::Null => {
            ui.label("null (edit type in Advanced JSON)");
        }
    }
}

#[cfg(test)]
mod tests {
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
        (workbench, game_host::demo_document(), root, context)
    }
    fn fake_host_job(
        workbench: &mut GameWorkbench,
        document: &GraphDocument,
        context: PreparationContext,
    ) -> (mpsc::Sender<Result<GameHost, String>>, BuildProgress) {
        let (send, receiver) = mpsc::channel();
        let progress = BuildProgress::new("Test host candidate");
        workbench.game_activity = Some(progress.clone());
        workbench.game_preparation = Some(HostPreparation {
            document: document.clone(),
            context,
            receiver,
            progress: progress.clone(),
        });
        (send, progress)
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
        let mut document = game_host::demo_document();
        workbench
            .ensure(&document, &root, Some(&root), WorldKind::Objects)
            .unwrap();
        let host = workbench.host.as_mut().unwrap();
        host.audio.enabled = false;
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
            .preparation_context(&root, Some(&root), WorldKind::Objects)
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
            GameInput {
                movement: [1., 0.],
                attack: true,
                aim_world: [4., 0., 2.],
                ..Default::default()
            },
            true,
        );
        merge_view_input(&mut workbench.input, GameInput::default(), false);
        assert!(workbench.input.attack);
        assert_eq!(workbench.input.aim_world, [4., 0., 2.]);
        assert_eq!(workbench.input.movement, [1., 0.]);
        workbench.tick(1. / 60., true).unwrap();
        assert_eq!(workbench.input, GameInput::default());
    }
    #[test]
    fn custom_declared_components_are_visible_only_while_the_module_is_active() {
        let mut graph = ExecutionGraph::gameplay();
        graph.stages[0].nodes[0].params["components"] = json!(["inventory", "quest", 42]);
        let registry = SystemRegistry::builtin();
        let visible = active_components(&graph, &registry, false);
        assert!(visible.contains("inventory"));
        assert!(visible.contains("quest"));
        graph.stages[0].nodes[0]
            .params
            .as_object_mut()
            .unwrap()
            .remove("components");
        assert!(!active_components(&graph, &registry, false).contains("inventory"));
    }
    use super::*;
    #[test]
    fn scene_copy_template_creation_and_instance_fields_survive_save_and_template_edits() {
        let mut game = GameProject::demo();
        let copied = create_scene(&mut game, "arena", "another", true).unwrap();
        assert_eq!(game.scenes[&copied].objects, game.scenes["arena"].objects);
        let template = create_template(&mut game, "arena", 10, "linked").unwrap();
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
        restored.validate().unwrap();
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
            object.overrides["material.base_color"],
            json!([0.2, 0.3, 0.4, 1.])
        );
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
        let visible = active_components(
            &ExecutionGraph::gameplay(),
            &SystemRegistry::builtin(),
            true,
        );
        assert!(visible.contains("material"));
        assert!(!visible.contains("network"));
        assert_eq!(object.components["network"]["replication"], true);
    }
    #[test]
    fn disconnected_graph_can_be_repaired_and_wrong_pin_or_execution_order_is_rejected() {
        let registry = SystemRegistry::builtin();
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
