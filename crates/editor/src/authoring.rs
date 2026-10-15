//! Background candidates and authoring state, separate from simulation state.

use crate::activity::{ActivitySnapshot, BuildProgress};
use crate::game_host::ShaderDependencyWatch;
use module_engine_core::{ControlCommand, GraphDocument, ModuleRegistry, Runtime, ScriptContext};
use module_engine_script::{
    CompiledScript, JobLease, ScriptCompiler, ScriptKind, ScriptTick, spawn_job,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

struct ScriptJob {
    source: String,
    kind: ScriptKind,
    _job: Option<JobLease>,
    receiver: DeferredReceiver<Result<CompiledScript, String>>,
}

// A queued accepted result can own a worker/renderer. Move the receiver itself
// off the UI, so closing a panel never drops that result or joins it there.
struct DeferredReceiver<T: Send + 'static>(Option<mpsc::Receiver<T>>);
impl<T: Send + 'static> From<mpsc::Receiver<T>> for DeferredReceiver<T> {
    fn from(receiver: mpsc::Receiver<T>) -> Self {
        Self(Some(receiver))
    }
}
impl<T: Send + 'static> std::ops::Deref for DeferredReceiver<T> {
    type Target = mpsc::Receiver<T>;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("live background receiver")
    }
}
impl<T: Send + 'static> Drop for DeferredReceiver<T> {
    fn drop(&mut self) {
        if let Some(receiver) = self.0.take() {
            let _ = spawn_job(move |_| drop(receiver));
        }
    }
}

pub struct Scripts {
    compiler: ScriptCompiler,
    asset_root: std::path::PathBuf,
    asset_paths: BTreeMap<String, String>,
    active: BTreeMap<String, CompiledScript>,
    rejected: BTreeMap<String, CompiledScript>,
    staged: RefCell<BTreeMap<String, ScriptTick>>,
    pending: BTreeMap<String, ScriptJob>,
    activities: BTreeMap<String, BuildProgress>,
    pub errors: BTreeMap<String, String>,
    pub status: String,
    availability_error: Option<String>,
}

impl Scripts {
    pub fn with_root(root: std::path::PathBuf) -> Self {
        Self {
            compiler: ScriptCompiler::new(root.clone(), root.join(".work/scripts")),
            asset_root: root,
            asset_paths: BTreeMap::new(),
            active: BTreeMap::new(),
            rejected: BTreeMap::new(),
            staged: RefCell::new(BTreeMap::new()),
            pending: BTreeMap::new(),
            activities: BTreeMap::new(),
            errors: BTreeMap::new(),
            status: String::new(),
            availability_error: None,
        }
    }

    pub fn disabled(error: String) -> Self {
        let mut scripts = Self::with_root(std::path::PathBuf::from(".sdk-unavailable"));
        scripts.availability_error = Some(error.clone());
        scripts.status = error;
        scripts
    }
    /// Compilers belong to Engine; file reads belong to the opened game JSON.
    /// Bind active and pending results to that project without losing accepted
    /// state. Root calls this after selecting a different project directory.
    pub fn set_asset_root(&mut self, root: &std::path::Path) -> Result<(), String> {
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        if root == self.asset_root {
            return Ok(());
        }
        self.reject_updates()?;
        let active = self
            .active
            .iter()
            .map(|(id, script)| {
                Ok((
                    id.clone(),
                    script
                        .clone()
                        .with_asset_root(&root)
                        .map_err(|e| e.to_string())?,
                ))
            })
            .collect::<Result<_, String>>()?;
        let rejected = self
            .rejected
            .iter()
            .map(|(id, script)| {
                Ok((
                    id.clone(),
                    script
                        .clone()
                        .with_asset_root(&root)
                        .map_err(|e| e.to_string())?,
                ))
            })
            .collect::<Result<_, String>>()?;
        self.active = active;
        self.rejected = rejected;
        self.asset_root = root;
        Ok(())
    }
    pub fn request(&mut self, id: &str, source: &str, kind: ScriptKind) {
        if let Some(error) = &self.availability_error {
            self.errors.insert(id.into(), error.clone());
            self.status = error.clone();
            let progress = BuildProgress::new(format!("Rust: {id}"));
            progress.fail(error.clone());
            self.activities.insert(id.into(), progress);
            return;
        }
        if self.pending.contains_key(id) {
            return;
        }
        self.rejected.remove(id);
        let compiler = self.compiler.clone();
        let asset_root = self.asset_root.clone();
        let source = source.to_owned();
        let candidate = source.clone();
        let progress = BuildProgress::new(format!("Rust: {id}"));
        progress.stage("Компиляция Rust");
        self.activities.insert(id.into(), progress.clone());
        let (receiver, job) = spawn_job(move |cancelled| {
            let result = compiler
                .compile_kind_cancellable(&candidate, kind, cancelled)
                .and_then(|script| script.with_asset_root(&asset_root))
                .map_err(|e| e.to_string());
            match &result {
                Ok(_) => progress.stage("Код скомпилирован; ожидание применения"),
                Err(error) => progress.fail(error.clone()),
            }
            result
        });
        self.pending.insert(
            id.to_owned(),
            ScriptJob {
                source,
                kind,
                _job: Some(job),
                receiver: receiver.into(),
            },
        );
        self.errors.remove(id);
        self.status = format!("Rust {id}: компиляция в фоне");
    }

    pub fn poll(&mut self, document: &GraphDocument) {
        match module_engine_project::AssetManifest::from_value(&document.asset_manifest) {
            Ok(manifest) => {
                self.asset_paths = manifest.asset_paths();
                self.errors.remove("asset_manifest");
            }
            Err(error) => {
                self.asset_paths.clear();
                self.errors.insert("asset_manifest".into(), error);
            }
        }
        let completed = self
            .pending
            .iter()
            .filter_map(|(id, job)| match job.receiver.try_recv() {
                Ok(result) => Some((id.clone(), job.source.clone(), job.kind, result)),
                Err(mpsc::TryRecvError::Disconnected) => Some((
                    id.clone(),
                    job.source.clone(),
                    job.kind,
                    Err("Поток компиляции Rust завершился без результата".into()),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            })
            .collect::<Vec<_>>();
        for (id, source, kind, result) in completed {
            self.pending.remove(&id);
            let current_kind = document
                .root
                .nodes
                .iter()
                .find(|n| {
                    n.params
                        .get("source_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(id.as_str())
                        && (n.module == "custom.rust" || n.module == "custom.rust.update")
                })
                .map(|n| {
                    if n.module == "custom.rust.update" {
                        ScriptKind::Update
                    } else {
                        ScriptKind::Speed
                    }
                })
                .unwrap_or(kind);
            if document.custom_sources.get(&id) != Some(&source) || current_kind != kind {
                self.status = format!("Rust {id}: результат устарел, исходник уже изменён");
                if let Some(current) = document.custom_sources.get(&id) {
                    self.request(&id, current, current_kind);
                }
                continue;
            }
            let result = result.and_then(|script| {
                script
                    .with_asset_root(&self.asset_root)
                    .map_err(|e| e.to_string())
            });
            match result {
                Ok(candidate) => {
                    if let Some(active) = self.active.get(&id)
                        && let Err(error) = candidate.inherit_state_from(active)
                    {
                        self.rejected.insert(id.clone(), candidate);
                        self.errors.insert(id.clone(), error.to_string());
                        if let Some(progress) = self.activities.get(&id) {
                            progress.fail(format!(
                                "Несовместимое состояние: {error}. Нужен сброс или миграция"
                            ));
                        }
                        self.status = format!(
                            "Rust {id}: несовместимое состояние; предыдущая версия сохранена. Нужен явный сброс или миграция"
                        );
                        continue;
                    }
                    if let Err(error) = self.reject_staged(&id) {
                        if let Some(progress) = self.activities.get(&id) {
                            progress.fail(error.clone());
                        }
                        self.errors.insert(id.clone(), error);
                        continue;
                    }
                    self.active.insert(id.clone(), candidate);
                    self.rejected.remove(&id);
                    self.errors.remove(&id);
                    self.status = format!("Rust {id}: скомпилированная версия применена");
                    if let Some(progress) = self.activities.get(&id) {
                        progress.succeed("Скомпилированная версия применена");
                    }
                }
                Err(error) => {
                    if let Some(progress) = self.activities.get(&id) {
                        progress.fail(error.clone());
                    }
                    self.errors.insert(id.clone(), error);
                    self.status = if self.has_active(&id) {
                        format!("Rust {id}: ошибка; предыдущая версия сохранена")
                    } else {
                        format!("Rust {id}: ошибка; рабочей скомпилированной версии ещё нет")
                    };
                }
            }
        }
    }

    pub fn pending(&self, id: &str) -> bool {
        self.pending.contains_key(id)
    }
    pub fn activity_snapshots(&self) -> Vec<ActivitySnapshot> {
        self.activities
            .values()
            .map(BuildProgress::snapshot)
            .collect()
    }
    pub fn active_source(&self, id: &str) -> Option<&str> {
        self.active.get(id).map(CompiledScript::source)
    }
    pub fn has_active(&self, id: &str) -> bool {
        self.active.contains_key(id)
    }
    pub fn has_active_kind(&self, id: &str, kind: ScriptKind) -> bool {
        self.active
            .get(id)
            .is_some_and(|script| script.kind() == kind)
    }

    pub fn has_rejected_candidate(&self, id: &str) -> bool {
        self.rejected.contains_key(id)
    }

    /// The UI should compare this to its current source before offering reset.
    pub fn rejected_source(&self, id: &str) -> Option<&str> {
        self.rejected.get(id).map(CompiledScript::source)
    }

    /// Explicit user choice after a successful but incompatible compilation.
    pub fn apply_rejected_with_reset(&mut self, id: &str) -> Result<(), String> {
        let candidate = self
            .rejected
            .get(id)
            .ok_or("Нет отклонённого скомпилированного кандидата")?;
        candidate.reset_state().map_err(|e| e.to_string())?;
        self.reject_staged(id)?;
        let candidate = self.rejected.remove(id).unwrap();
        self.active.insert(id.to_owned(), candidate);
        self.errors.remove(id);
        self.status = format!("Rust {id}: новая версия применена с явным сбросом состояния");
        let progress = BuildProgress::new(format!("Rust: {id}"));
        progress.succeed("Применено со сбросом состояния");
        self.activities.insert(id.into(), progress);
        Ok(())
    }

    /// Explicit migration is supplied by author tooling, rather than guessed.
    pub fn apply_rejected_with_migration(
        &mut self,
        id: &str,
        state: serde_json::Value,
    ) -> Result<(), String> {
        let candidate = self
            .rejected
            .get(id)
            .ok_or("Нет отклонённого скомпилированного кандидата")?;
        candidate.migrate_state(state).map_err(|e| e.to_string())?;
        self.reject_staged(id)?;
        let candidate = self.rejected.remove(id).unwrap();
        self.active.insert(id.to_owned(), candidate);
        self.errors.remove(id);
        self.status = format!("Rust {id}: новая версия применена с явной миграцией состояния");
        let progress = BuildProgress::new(format!("Rust: {id}"));
        progress.succeed("Применено с миграцией состояния");
        self.activities.insert(id.into(), progress);
        Ok(())
    }

    /// Commit only after the whole legacy preview world/render packet succeeds.
    /// No authoring mutation occurs concurrently with this single UI-thread call.
    pub fn commit_updates(&self) -> Result<(), String> {
        let staged = self.staged.take();
        for (id, tick) in &staged {
            let result = self
                .active
                .get(id)
                .ok_or_else(|| format!("Rust {id}: active worker is missing"))
                .and_then(|script| script.validate_tick(tick).map_err(|e| e.to_string()));
            if let Err(error) = result {
                for (id, tick) in &staged {
                    if let Some(script) = self.active.get(id) {
                        let _ = script.reject_tick(tick);
                    }
                }
                return Err(error);
            }
        }
        for (id, tick) in &staged {
            self.active[id]
                .commit_tick(tick)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Reject every prepared source if any command/query/worker in the frame
    /// fails. Committed state stays unchanged and uncommitted globals terminate.
    pub fn reject_updates(&self) -> Result<(), String> {
        let staged = self.staged.take();
        let mut first_error = None;
        for (id, tick) in staged {
            if let Some(script) = self.active.get(&id)
                && let Err(error) = script.reject_tick(&tick)
            {
                first_error.get_or_insert_with(|| error.to_string());
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn reject_staged(&self, id: &str) -> Result<(), String> {
        if let Some(tick) = self.staged.borrow_mut().remove(id)
            && let Some(script) = self.active.get(id)
        {
            script.reject_tick(&tick).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// A new Play session starts from declared defaults. Hot source apply uses
    /// inherit_state_from instead and does not call this method.
    pub fn reset_game_states(&self) -> Result<(), String> {
        self.reject_updates()?;
        for script in self.active.values() {
            script.reset_state().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn install_for_test(&mut self, id: &str, script: CompiledScript) {
        self.active.insert(id.into(), script);
    }

    pub fn speed(&self, id: &str, time: f64, base: f64) -> Option<Result<f64, String>> {
        let script = self.active.get(id)?;
        if script.kind() != ScriptKind::Speed {
            return None;
        }
        let mut context = ScriptContext::new(time, 1.0 / 60.0, vec![]);
        context.base = base;
        context.asset_paths = self.asset_paths.clone();
        Some(
            script
                .tick(&context, Duration::from_millis(150))
                .map_err(|e| e.to_string())
                .and_then(|tick| {
                    let speed = tick.speed.ok_or("Scalar script returned no speed")?;
                    script.commit_tick(&tick).map_err(|e| e.to_string())?;
                    Ok(speed)
                }),
        )
    }

    pub fn update(&self, id: &str, context: &ScriptContext) -> Result<Vec<ControlCommand>, String> {
        let script = self
            .active
            .get(id)
            .ok_or_else(|| format!("Rust {id} ещё не скомпилирован"))?;
        if script.kind() != ScriptKind::Update {
            return Err(format!("Rust {id} должен содержать fn update"));
        }
        if self.staged.borrow().contains_key(id) {
            return Err(format!(
                "Rust {id}: предыдущий результат не принят и не отклонён"
            ));
        }
        let mut context = context.clone();
        context.asset_paths = self.asset_paths.clone();
        let tick = script
            .tick(&context, Duration::from_millis(150))
            .map_err(|e| e.to_string())?;
        if !tick.events.is_empty() {
            script.reject_tick(&tick).map_err(|e| e.to_string())?;
            return Err(format!(
                "Rust {id}: события требуют игрового исполнителя, legacy preview их не принимает"
            ));
        }
        let commands = tick.commands.clone();
        self.staged.borrow_mut().insert(id.to_owned(), tick);
        Ok(commands)
    }
}

struct PreviewJob {
    document: GraphDocument,
    backend_root: PathBuf,
    dependencies: ShaderDependencyWatch,
    _job: Option<JobLease>,
    receiver: DeferredReceiver<Result<Runtime, String>>,
}

struct PreviewAttempt {
    document: GraphDocument,
    backend_root: PathBuf,
    dependencies: ShaderDependencyWatch,
}

pub struct PreviewBuilder {
    pending: Option<PreviewJob>,
    latest: Option<GraphDocument>,
    pub error: Option<String>,
    progress: Option<BuildProgress>,
    awaiting_acceptance: Option<PreviewAttempt>,
    rejected: Option<PreviewAttempt>,
}

impl PreviewBuilder {
    pub fn new() -> Self {
        Self {
            pending: None,
            latest: None,
            error: None,
            progress: None,
            awaiting_acceptance: None,
            rejected: None,
        }
    }
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn activity_snapshot(&self) -> Option<ActivitySnapshot> {
        self.progress.as_ref().map(BuildProgress::snapshot)
    }
    pub fn applied(&mut self) {
        if let Some(progress) = &self.progress {
            progress.succeed("Конфигурация рендера применена");
        }
        self.awaiting_acceptance = None;
        self.rejected = None;
        self.error = None;
    }
    pub fn failed_apply(&mut self, error: &str) {
        if let Some(progress) = &self.progress {
            progress.fail(error);
        }
        if let Some(attempt) = self.awaiting_acceptance.take() {
            self.rejected = Some(attempt);
        }
        self.error = Some(error.into());
    }
    #[allow(dead_code)] // Compatibility wrapper for callers without an explicit backend binding.
    pub fn request(&mut self, document: &GraphDocument, registry: &ModuleRegistry) {
        self.request_at(document, registry, &module_engine_vulkan::resource_root());
    }
    pub fn request_at(
        &mut self,
        document: &GraphDocument,
        registry: &ModuleRegistry,
        backend_root: &Path,
    ) {
        if self.pending.is_some() {
            self.latest = Some(document.clone());
            return;
        }
        let candidate = document.clone();
        // Capture before the worker starts: an include may be fixed before a
        // failed worker result reaches the UI. Capturing after failure misses it.
        let dependencies = ShaderDependencyWatch::new(document, backend_root);
        let registry = registry.clone();
        let artifacts = module_engine_script::retain_test_artifacts(backend_root);
        let progress = BuildProgress::new("Предпросмотр Engine");
        progress.stage("Проверка графа, подготовка Vulkan и шейдеров");
        self.progress = Some(progress.clone());
        let (receiver, job) = spawn_job(move |cancelled| {
            let _artifacts = artifacts;
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err("Подготовка предпросмотра отменена".into());
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Runtime::from_document(&candidate, &registry).map_err(|e| e.to_string())
            }))
            .unwrap_or_else(|_| {
                Err("Подготовка предпросмотра завершилась с внутренней ошибкой".into())
            });
            match &result {
                Ok(_) => progress.stage("GPU-кандидат подготовлен; ожидание применения"),
                Err(error) => progress.fail(error.clone()),
            }
            result
        });
        self.pending = Some(PreviewJob {
            document: document.clone(),
            backend_root: backend_root.to_path_buf(),
            dependencies,
            _job: Some(job),
            receiver: receiver.into(),
        });
        self.awaiting_acceptance = None;
        self.rejected = None;
        self.error = None;
    }

    #[allow(dead_code)] // Compatibility wrapper for callers without an explicit backend binding.
    pub fn poll(&mut self, document: &GraphDocument, registry: &ModuleRegistry) -> Option<Runtime> {
        self.poll_at(document, registry, &module_engine_vulkan::resource_root())
    }
    pub fn poll_at(
        &mut self,
        document: &GraphDocument,
        registry: &ModuleRegistry,
        backend_root: &Path,
    ) -> Option<Runtime> {
        if self.pending.is_none() {
            if self.rejected.as_mut().is_some_and(|attempt| {
                attempt.backend_root != backend_root
                    || !attempt.document.execution_eq(document)
                    || attempt.dependencies.changed(document, backend_root)
            }) {
                self.request_at(document, registry, backend_root);
            }
            return None;
        }
        let job = self.pending.as_ref()?;
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(_) => Err("Поток сборки рендера завершился без результата".into()),
        };
        let candidate = self.pending.take().unwrap();
        if !candidate.document.execution_eq(document) || candidate.backend_root != backend_root {
            let _ = spawn_job(move |_| drop(result));
            self.latest = None;
            self.request_at(document, registry, backend_root);
            return None;
        }
        self.latest = None;
        self.awaiting_acceptance = Some(PreviewAttempt {
            document: candidate.document,
            backend_root: candidate.backend_root,
            dependencies: candidate.dependencies,
        });
        match result {
            Ok(runtime) => {
                self.rejected = None;
                self.error = None;
                Some(runtime)
            }
            Err(error) => {
                self.failed_apply(&error);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{WorldKind, demo_document};
    use std::path::Path;
    use std::thread;

    #[test]
    fn preview_progress_reports_success_only_after_acceptance_and_keeps_failures() {
        let document = demo_document(WorldKind::Objects);
        let registry = ModuleRegistry::builtin();
        let progress = BuildProgress::new("Предпросмотр Engine");
        progress.stage("GPU-кандидат подготовлен; ожидание применения");
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Runtime::from_document(&document, &registry).map_err(|error| error.to_string()))
            .unwrap();
        let mut builder = PreviewBuilder::new();
        builder.progress = Some(progress);
        builder.pending = Some(PreviewJob {
            document: document.clone(),
            backend_root: module_engine_vulkan::resource_root(),
            dependencies: ShaderDependencyWatch::new(
                &document,
                &module_engine_vulkan::resource_root(),
            ),
            _job: None,
            receiver: receiver.into(),
        });
        assert!(builder.poll(&document, &registry).is_some());
        assert_eq!(
            builder.activity_snapshot().unwrap().state,
            crate::activity::ActivityState::Running
        );
        builder.applied();
        assert_eq!(
            builder.activity_snapshot().unwrap().state,
            crate::activity::ActivityState::Succeeded
        );
        builder.failed_apply("Параметр не поддерживается новым проходом");
        assert_eq!(
            builder.activity_snapshot().unwrap().state,
            crate::activity::ActivityState::Failed
        );
        assert_eq!(
            builder.activity_snapshot().unwrap().error.as_deref(),
            Some("Параметр не поддерживается новым проходом")
        );
    }

    fn supply_candidate(scripts: &mut Scripts, id: &str, source: &str, candidate: CompiledScript) {
        let (sender, receiver) = mpsc::channel();
        sender.send(Ok(candidate)).unwrap();
        scripts.pending.insert(
            id.to_owned(),
            ScriptJob {
                source: source.to_owned(),
                kind: ScriptKind::Update,
                _job: None,
                receiver: receiver.into(),
            },
        );
        let progress = BuildProgress::new(format!("Rust: {id}"));
        progress.stage("Код скомпилирован; ожидание применения");
        scripts.activities.insert(id.into(), progress);
    }
    #[test]
    fn completed_preview_survives_layout_edits_but_rejects_runtime_edits() {
        for layout_only in [true, false] {
            let document = demo_document(WorldKind::Objects);
            let registry = ModuleRegistry::builtin();
            let runtime = Runtime::from_document(&document, &registry).unwrap();
            let (sender, receiver) = mpsc::channel();
            sender.send(Ok(runtime)).unwrap();
            let mut builder = PreviewBuilder::new();
            builder.pending = Some(PreviewJob {
                document: document.clone(),
                backend_root: module_engine_vulkan::resource_root(),
                dependencies: ShaderDependencyWatch::new(
                    &document,
                    &module_engine_vulkan::resource_root(),
                ),
                _job: None,
                receiver: receiver.into(),
            });
            let mut edited = document;
            if layout_only {
                edited.root.nodes[0].position = [420., 210.];
                edited.root.nodes[0].label.push_str(" renamed");
            } else {
                edited.root.nodes[0].params["runtime_change"] = serde_json::json!(true);
            }
            assert_eq!(builder.poll(&edited, &registry).is_some(), layout_only);
            assert_eq!(builder.is_pending(), !layout_only);
        }
    }

    #[test]
    fn failed_preview_retries_repaired_external_include_without_json_edit() {
        let artifacts = module_engine_script::TestArtifactGuard::new(
            &std::env::temp_dir(),
            "preview-include-repair",
        )
        .unwrap();
        let root = artifacts.path().to_path_buf();
        std::fs::create_dir_all(root.join("assets/gpu")).unwrap();
        let root = root.canonicalize().unwrap();
        let header = root.join("assets/gpu/repair.hlsli");
        let mut document = crate::game_host::demo_document();
        let renderer = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap();
        let ao = renderer
            .nodes
            .iter_mut()
            .find(|node| node.id == "ao")
            .unwrap();
        ao.module = "gpu.custom".into();
        ao.params = serde_json::json!({"source_id":"repair","manifest":module_engine_vulkan::custom_compute_manifest(),"gain":1.0});
        renderer
            .edges
            .retain(|edge| edge.to_node != "ao" || edge.to_port == "color");
        document.shader_sources.insert(
            "repair".into(),
            format!(
                "#include \"repair.hlsli\"\n{}",
                module_engine_vulkan::demo_custom_compute_source()
            ),
        );
        let original = document.clone();
        // Use the builtin registry to make the worker fail immediately without
        // a GPU/DXC dependency. The production request still captures the real
        // literal include revision at job start, before this result is polled.
        let registry = ModuleRegistry::builtin();
        for repair_before_failure_is_polled in [false, true] {
            std::fs::write(&header, "invalid included HLSL\n").unwrap();
            let mut builder = PreviewBuilder::new();
            builder.request_at(&document, &registry, &root);
            if repair_before_failure_is_polled {
                std::fs::write(&header, "float repaired() { return 0.4; }\n").unwrap();
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while builder.is_pending() {
                builder.poll_at(&document, &registry, &root);
                assert!(std::time::Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            assert!(builder.error.is_some());
            if !repair_before_failure_is_polled {
                builder.poll_at(&document, &registry, &root);
                assert!(
                    !builder.is_pending(),
                    "unchanged failure must not retry each frame"
                );
                std::fs::write(&header, "float repaired() { return 0.4; }\n").unwrap();
            }
            thread::sleep(Duration::from_millis(260));
            builder.poll_at(&document, &registry, &root);
            assert!(
                builder.is_pending(),
                "disk repair must retry without a document change"
            );
            assert!(builder.error.is_none());
            assert_eq!(document, original);
            // Await the retry too; the directory must outlive both workers.
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while builder.is_pending() {
                builder.poll_at(&document, &registry, &root);
                assert!(std::time::Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    #[test]
    fn nongame_custom_resolves_asset_ids_in_selected_project_and_retains_state_on_relocation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let artifacts = module_engine_script::TestArtifactGuard::new(
            &root.join(".work/test-temp"),
            "authoring-asset-context",
        )
        .unwrap();
        let work = artifacts.path();
        let first = work.join("first");
        let second = work.join("second");
        for (folder, data) in [(&first, b"ab".as_slice()), (&second, b"xyz".as_slice())] {
            std::fs::create_dir_all(folder.join("assets/data")).unwrap();
            std::fs::write(folder.join("assets/data/value.txt"), data).unwrap();
        }
        let source = "// module-engine-state: {\"schema\":\"asset-counter.v1\",\"initial\":{\"ticks\":0}}\nfn update(ctx: &mut ScriptContext) { let path=ctx.asset_path(\"stable\").unwrap(); let value=std::fs::read(path).unwrap().len() as f64; ctx.state[\"ticks\"]=serde_json::json!(ctx.state[\"ticks\"].as_u64().unwrap()+1); ctx.set_render_parameter(\"tonemap\",\"exposure\",value); }";
        let compiler = ScriptCompiler::new(root.clone(), work.join("compile"));
        let compiled = compiler.compile(source).unwrap();
        let mut scripts = Scripts::with_root(root);
        scripts.install_for_test("assets", compiled);
        scripts.set_asset_root(&first).unwrap();
        let mut doc = demo_document(WorldKind::Objects);
        let mut manifest = module_engine_project::AssetManifest::default();
        manifest
            .register("stable", "assets/data/value.txt", &first)
            .unwrap();
        doc.asset_manifest = serde_json::to_value(manifest).unwrap();
        scripts.poll(&doc);
        let context = ScriptContext::new(0., 1. / 60., vec![]);
        let commands = scripts.update("assets", &context).unwrap();
        scripts.commit_updates().unwrap();
        assert_eq!(
            commands[0],
            ControlCommand::Render(module_engine_core::RenderCommand::SetParameter {
                pass: "tonemap".into(),
                name: "exposure".into(),
                value: serde_json::json!(2.0)
            })
        );
        assert_eq!(scripts.active["assets"].state().unwrap()["ticks"], 1);
        scripts.set_asset_root(&second).unwrap();
        let mut relocated_manifest = module_engine_project::AssetManifest::default();
        relocated_manifest
            .register("stable", "assets/data/value.txt", &second)
            .unwrap();
        doc.asset_manifest = serde_json::to_value(relocated_manifest).unwrap();
        scripts.poll(&doc);
        assert_eq!(scripts.active["assets"].state().unwrap()["ticks"], 1);
        let commands = scripts.update("assets", &context).unwrap();
        scripts.commit_updates().unwrap();
        assert_eq!(
            commands[0],
            ControlCommand::Render(module_engine_core::RenderCommand::SetParameter {
                pass: "tonemap".into(),
                name: "exposure".into(),
                value: serde_json::json!(3.0)
            })
        );
        assert_eq!(scripts.active["assets"].state().unwrap()["ticks"], 2);
    }

    #[test]
    fn prepared_preview_states_commit_or_reject_together_and_hot_apply_requires_compatible_state() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let artifacts = module_engine_script::TestArtifactGuard::new(
            &root.join(".work/test-temp"),
            "authoring-state-test",
        )
        .unwrap();
        let compiler = ScriptCompiler::new(root.clone(), artifacts.path().to_path_buf());
        let source = "// module-engine-state: {\"schema\":\"preview.v1\",\"initial\":{\"ticks\":0}}\nfn update(ctx: &mut ScriptContext) { ctx.state[\"ticks\"] = serde_json::json!(ctx.state[\"ticks\"].as_u64().unwrap() + 1); ctx.set_render_parameter(\"tonemap\",\"exposure\",1.4); }";
        let compiled = compiler.compile(source).unwrap();
        let mut scripts = Scripts::with_root(root);
        scripts.install_for_test("one", compiled.clone());
        scripts.install_for_test("two", compiled.fork_instance());
        let context = ScriptContext::new(1., 1. / 60., vec![]);
        scripts.update("one", &context).unwrap();
        scripts.update("two", &context).unwrap();
        assert_eq!(scripts.active["one"].state().unwrap()["ticks"], 0);
        assert!(scripts.update("one", &context).is_err());
        scripts.commit_updates().unwrap();
        assert_eq!(scripts.active["one"].state().unwrap()["ticks"], 1);
        assert_eq!(scripts.active["two"].state().unwrap()["ticks"], 1);
        scripts.update("one", &context).unwrap();
        scripts.update("two", &context).unwrap();
        scripts.reject_updates().unwrap();
        assert_eq!(scripts.active["one"].state().unwrap()["ticks"], 1);
        assert_eq!(scripts.active["two"].state().unwrap()["ticks"], 1);
        assert!(scripts.active["one"].worker_process_id().unwrap().is_none());

        let edited = source.replace("unwrap() + 1", "unwrap() + 2");
        let candidate = compiler.compile(&edited).unwrap();
        let mut document = demo_document(WorldKind::Objects);
        document.custom_sources.insert("one".into(), edited.clone());
        supply_candidate(&mut scripts, "one", &edited, candidate);
        scripts.poll(&document);
        assert_eq!(scripts.active_source("one"), Some(edited.as_str()));
        assert_eq!(
            scripts.activity_snapshots()[0].state,
            crate::activity::ActivityState::Succeeded
        );
        assert_eq!(scripts.active["one"].state().unwrap()["ticks"], 1);
        scripts.update("one", &context).unwrap();
        scripts.commit_updates().unwrap();
        assert_eq!(scripts.active["one"].state().unwrap()["ticks"], 3);

        let incompatible_source = edited.replace("preview.v1", "preview.v2");
        let incompatible = compiler.compile(&incompatible_source).unwrap();
        document
            .custom_sources
            .insert("one".into(), incompatible_source.clone());
        supply_candidate(
            &mut scripts,
            "one",
            &incompatible_source,
            incompatible.clone(),
        );
        scripts.poll(&document);
        assert!(scripts.has_rejected_candidate("one"));
        assert_eq!(
            scripts.activity_snapshots()[0].state,
            crate::activity::ActivityState::Failed
        );
        assert_eq!(
            scripts.rejected_source("one"),
            Some(incompatible_source.as_str())
        );
        assert_eq!(scripts.active_source("one"), Some(edited.as_str()));
        assert_eq!(scripts.active["one"].state().unwrap()["ticks"], 3);
        scripts.apply_rejected_with_reset("one").unwrap();
        assert!(!scripts.has_rejected_candidate("one"));
        assert_eq!(
            scripts.activity_snapshots()[0].state,
            crate::activity::ActivityState::Succeeded
        );
        assert_eq!(
            scripts.active_source("one"),
            Some(incompatible_source.as_str())
        );
        assert_eq!(scripts.active["one"].state().unwrap()["ticks"], 0);

        scripts
            .rejected
            .insert("two".into(), incompatible.fork_instance());
        assert!(
            scripts
                .apply_rejected_with_migration("two", serde_json::json!({"ticks":"wrong"}))
                .is_err()
        );
        assert_eq!(scripts.active["two"].state().unwrap()["ticks"], 1);
        scripts
            .apply_rejected_with_migration("two", serde_json::json!({"ticks":30}))
            .unwrap();
        scripts.update("two", &context).unwrap();
        scripts.commit_updates().unwrap();
        assert_eq!(scripts.active["two"].state().unwrap()["ticks"], 32);
        scripts.reset_game_states().unwrap();
        assert_eq!(scripts.active["one"].state().unwrap()["ticks"], 0);
        assert_eq!(scripts.active["two"].state().unwrap()["ticks"], 0);
    }
}
