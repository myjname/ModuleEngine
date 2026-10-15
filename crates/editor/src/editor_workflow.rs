//! Menu commands, guarded project transitions and asynchronous file operations.
//! The same dispatch path serves menu items, toolbar controls and shortcuts.
use super::*;
use crate::assets_ui::BrowserAction;
use crate::{native_dialogs, workspace_files};
use eframe::egui::{Key, Modifiers};
use module_engine_project::{
    AssetManifest, AssetOperation, AssetOperationRecord, AssetWorkspace, DeletePolicy,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    process::{Command as ProcessCommand, Stdio},
    sync::mpsc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    NewEmpty,
    NewArenas,
    NewPuzzle,
    UpdateEngineContracts,
    Open,
    Save,
    SaveAs,
    Import,
    ExportPlayer,
    ExportTeam,
    ExportSource,
    Check,
    Undo,
    Redo,
    History,
    Diagnostics,
    ProjectSettings,
    SaveLayout,
    LoadLayout,
    ResetLayout,
    PlayPause,
    Stop,
    Quit,
}
#[derive(Clone, Copy, Debug)]
enum ExportKind {
    Player,
    Team,
    Source,
}
impl ExportKind {
    fn argument(self) -> &'static str {
        match self {
            Self::Player => "--export-game",
            Self::Team => "--export-team",
            Self::Source => "--export",
        }
    }
}
#[derive(Clone, Debug)]
enum Action {
    Asset(BrowserAction),
    Open,
    OpenPath(PathBuf),
    New(workspace_files::ProjectTemplate),
    Quit,
    Export(ExportKind),
    Check,
}
#[derive(Clone, Copy, Debug)]
enum DialogPurpose {
    Open,
    SaveAs,
    New(workspace_files::ProjectTemplate),
    Export(ExportKind),
}
enum Outcome {
    Asset(Box<AssetOperation>),
    Dialog(DialogPurpose, Option<PathBuf>),
    Saved(Box<workspace_files::SavedProject>),
    Report(String),
}
struct Job {
    title: String,
    started: Instant,
    receiver: mpsc::Receiver<Result<Outcome, String>>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct RecentProjects {
    version: u32,
    paths: Vec<PathBuf>,
}
impl RecentProjects {
    fn remember(&mut self, path: &Path) {
        let Ok(path) = path.canonicalize() else {
            return;
        };
        self.version = 1;
        self.paths.retain(|p| !same_path(p, &path));
        self.paths.insert(0, path);
        self.paths.truncate(12);
    }
}
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

pub struct Workflow {
    saved: GraphDocument,
    recent: RecentProjects,
    recent_path: Option<PathBuf>,
    guard: Option<Action>,
    job: Option<Job>,
    allow_close: bool,
    new_uses_saved: bool,
    new_engine_choice: crate::engine_profiles::EngineChoice,
    report: String,
    show_report: bool,
    menu_rects: BTreeMap<String, egui::Rect>,
    capture_menu: Option<String>,
    asset_history: BTreeMap<u64, AssetOperationRecord>,
}
impl Workflow {
    pub fn new(document: GraphDocument) -> Self {
        Self {
            saved: document,
            recent: RecentProjects::default(),
            recent_path: None,
            guard: None,
            job: None,
            allow_close: false,
            new_uses_saved: false,
            new_engine_choice: Default::default(),
            report: String::new(),
            show_report: false,
            menu_rects: BTreeMap::new(),
            capture_menu: if std::env::var_os("EFRAME_SCREENSHOT_TO").is_some()
                || std::env::var_os("MODULE_ENGINE_UI_SCREENSHOT_TO").is_some()
            {
                std::env::var("MODULE_ENGINE_CAPTURE_MENU").ok()
            } else {
                None
            },
            asset_history: BTreeMap::new(),
        }
    }
    pub fn blocks_document_edits(&self) -> bool {
        self.job.is_some() || self.guard.is_some() || self.show_report
    }
    pub(super) fn has_asset_history(&self) -> bool {
        !self.asset_history.is_empty()
    }
    pub(super) fn rebase_asset_history(
        &mut self,
        before: &serde_json::Value,
        after: &serde_json::Value,
    ) {
        for record in self.asset_history.values_mut() {
            if record.before.asset_manifest == *before {
                record.before.asset_manifest = after.clone();
            }
            if record.after.asset_manifest == *before {
                record.after.asset_manifest = after.clone();
            }
        }
    }
}

impl Editor {
    pub(super) fn initialize_workflow(&mut self) {
        self.workflow.recent_path = self
            .distribution
            .as_ref()
            .map(|d| d.package_root.join(".user/editor-recent-projects.json"));
        if let Some(path) = &self.workflow.recent_path
            && let Ok(bytes) = fs::read(path)
            && let Ok(mut recent) = serde_json::from_slice::<RecentProjects>(&bytes)
            && recent.version == 1
        {
            recent.paths.truncate(12);
            self.workflow.recent = recent;
        }
        self.remember_project();
    }
    fn remember_project(&mut self) {
        self.workflow.recent.remember(Path::new(&self.path));
        if !cfg!(test)
            && let Some(path) = &self.workflow.recent_path
        {
            let result = path
                .parent()
                .ok_or("Recent projects have no parent".to_string())
                .and_then(|p| fs::create_dir_all(p).map_err(|e| e.to_string()))
                .and_then(|_| {
                    serde_json::to_vec_pretty(&self.workflow.recent).map_err(|e| e.to_string())
                })
                .and_then(|bytes| fs::write(path, bytes).map_err(|e| e.to_string()));
            if let Err(error) = result {
                self.status = format!("Проект открыт; список Recent не сохранён: {error}");
            }
        }
    }
    pub(super) fn has_unsaved_work(&self) -> bool {
        self.json_dirty
            || self.game_workbench.has_graph_draft()
            || self.document != self.workflow.saved
    }
    fn prepare_file_operation(&mut self) -> Result<(), String> {
        if self.game_workbench.has_graph_draft() {
            return Err("Примените или отмените черновик Gameplay Systems перед сохранением. Он не входит в сохранённый проект.".into());
        }
        if self.json_dirty && !self.apply_json() {
            return Err(self.status.clone());
        }
        self.validate_candidate(&self.document, self.can_author_engine())
    }
    fn save_workflow(&mut self) -> bool {
        if let Err(error) = self.prepare_file_operation() {
            self.status = error;
            return false;
        }
        if !self.save() {
            return false;
        }
        self.workflow.saved = self.document.clone();
        self.remember_project();
        true
    }
    fn load_workflow(&mut self, path: PathBuf) -> bool {
        let original_path = self.path.clone();
        self.path = path.to_string_lossy().into_owned();
        if !self.load() {
            self.path = original_path;
            return false;
        }
        self.workflow.saved = self.document.clone();
        self.workflow.asset_history.clear();
        self.assets.reset();
        self.remember_project();
        true
    }
    fn request_action(&mut self, action: Action, ctx: &egui::Context) {
        if self.workflow.job.is_some() || self.workflow.guard.is_some() {
            return;
        }
        if matches!(action, Action::New(_)) {
            self.workflow.new_uses_saved = false;
        }
        if self.has_unsaved_work() {
            self.workflow.guard = Some(action);
        } else {
            self.perform_action(action, ctx);
        }
    }
    fn perform_action(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Asset(action) => self.perform_asset_action(action, ctx),
            Action::Open => self.start_dialog(DialogPurpose::Open, ctx),
            Action::OpenPath(path) => {
                if !self.load_workflow(path) {
                    self.report_error(self.status.clone());
                }
            }
            Action::New(empty) => self.start_dialog(DialogPurpose::New(empty), ctx),
            Action::Quit => {
                self.workflow.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Action::Export(kind) => self.start_dialog(DialogPurpose::Export(kind), ctx),
            Action::Check => self.start_cli("Проверка проекта", "--check", None, ctx),
        }
    }
    fn spawn_job(
        &mut self,
        title: &str,
        ctx: &egui::Context,
        work: impl FnOnce() -> Result<Outcome, String> + Send + 'static,
    ) {
        if self.workflow.job.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .unwrap_or_else(|_| Err("Операция завершилась с внутренней ошибкой".into()));
            let _ = sender.send(result);
            ctx.request_repaint();
        });
        self.workflow.job = Some(Job {
            title: title.into(),
            started: Instant::now(),
            receiver,
        });
        self.status = title.into();
    }
    fn start_dialog(&mut self, purpose: DialogPurpose, ctx: &egui::Context) {
        let initial = PathBuf::from(&self.path);
        self.spawn_job("Выбор файла / папки…", ctx, move || {
            let directory = initial.parent().unwrap_or(Path::new("."));
            let path = match purpose {
                DialogPurpose::Open => native_dialogs::open_project(directory)?,
                DialogPurpose::SaveAs => native_dialogs::save_project(&initial)?,
                DialogPurpose::New(_) => native_dialogs::pick_folder(
                    "New Project — выберите новую или пустую папку",
                    directory,
                )?,
                DialogPurpose::Export(_) => native_dialogs::pick_folder(
                    "Export — выберите новую или пустую папку",
                    directory,
                )?,
            };
            Ok(Outcome::Dialog(purpose, path))
        });
    }
    fn complete_dialog(
        &mut self,
        purpose: DialogPurpose,
        path: Option<PathBuf>,
        ctx: &egui::Context,
    ) {
        let Some(path) = path else {
            self.workflow.new_uses_saved = false;
            self.status =
                "Операция отменена; текущий проект и его правки остаются открытыми".into();
            return;
        };
        if matches!(purpose, DialogPurpose::SaveAs)
            && path
                .canonicalize()
                .ok()
                .zip(Path::new(&self.path).canonicalize().ok())
                .is_some_and(|(a, b)| same_path(&a, &b))
        {
            if !self.save_workflow() {
                self.report_error(self.status.clone());
            }
            return;
        }
        match purpose {
            DialogPurpose::Open => {
                if !self.load_workflow(path) {
                    self.report_error(self.status.clone());
                }
            }
            DialogPurpose::SaveAs | DialogPurpose::New(_) => {
                let Some(distribution) = self.distribution.clone() else {
                    self.report_error("Контекст движка недоступен".into());
                    return;
                };
                let source = PathBuf::from(&self.path);
                let document =
                    if matches!(purpose, DialogPurpose::New(_)) && self.workflow.new_uses_saved {
                        self.workflow.saved.clone()
                    } else {
                        self.document.clone()
                    };
                self.workflow.new_uses_saved = false;
                let engine_choice = self.workflow.new_engine_choice;
                self.spawn_job(
                    "Подготовка проекта и копирование ресурсов…",
                    ctx,
                    move || {
                        let saved = match purpose {
                            DialogPurpose::SaveAs => {
                                if path.exists() {
                                    workspace_files::save_as_confirmed(
                                        &source,
                                        &path,
                                        &document,
                                        &distribution,
                                    )?
                                } else {
                                    workspace_files::save_as(
                                        &source,
                                        &path,
                                        &document,
                                        &distribution,
                                    )?
                                }
                            }
                            DialogPurpose::New(template) => {
                                workspace_files::new_project_with_choice(
                                    &source,
                                    &path,
                                    &document,
                                    &distribution,
                                    template,
                                    engine_choice,
                                )?
                            }
                            _ => unreachable!(),
                        };
                        Ok(Outcome::Saved(Box::new(saved)))
                    },
                );
            }
            DialogPurpose::Export(kind) => {
                self.start_cli("Сборка / экспорт…", kind.argument(), Some(path), ctx)
            }
        }
    }
    fn start_cli(
        &mut self,
        title: &str,
        argument: &str,
        destination: Option<PathBuf>,
        ctx: &egui::Context,
    ) {
        let source = PathBuf::from(&self.path);
        let argument = argument.to_owned();
        self.spawn_job(title, ctx, move || {
            let source = source
                .canonicalize()
                .map_err(|e| format!("Сохраните проект перед проверкой/экспортом: {e}"))?;
            let root = source.parent().ok_or("Project directory missing")?;
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos();
            let logs = root.join(format!(".work/editor-jobs/{id}"));
            fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
            let stdout = logs.join("stdout.log");
            let stderr = logs.join("stderr.log");
            let mut command =
                ProcessCommand::new(std::env::current_exe().map_err(|e| e.to_string())?);
            command
                .arg(&argument)
                .arg(&source)
                .current_dir(root)
                .stdout(Stdio::from(
                    fs::File::create(&stdout).map_err(|e| e.to_string())?,
                ))
                .stderr(Stdio::from(
                    fs::File::create(&stderr).map_err(|e| e.to_string())?,
                ));
            if let Some(destination) = destination {
                command.arg(destination);
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            let status = command
                .status()
                .map_err(|e| format!("Запуск инструмента: {e}"))?;
            let report = format!(
                "{}\n{}\nЛоги: {}",
                read_tail(&stdout),
                read_tail(&stderr),
                logs.display()
            );
            if status.success() {
                Ok(Outcome::Report(report))
            } else {
                Err(report)
            }
        });
    }
    pub(super) fn request_asset_action(&mut self, action: BrowserAction, ctx: &egui::Context) {
        if self.game_workbench.is_play_session()
            || self.playing
            || self.workflow.blocks_document_edits()
        {
            return;
        }
        if matches!(action, BrowserAction::Refresh) {
            self.assets.invalidate();
            self.game_workbench.invalidate_assets();
            return;
        }
        if matches!(
            action,
            BrowserAction::Preview { .. } | BrowserAction::Select { .. }
        ) {
            return;
        }
        self.cancel_active_scene_gestures();
        self.request_action(Action::Asset(action), ctx);
    }
    fn perform_asset_action(&mut self, action: BrowserAction, ctx: &egui::Context) {
        if self.game_workbench.is_play_session() || self.playing {
            self.report_error("Остановите Play перед изменением файлов проекта".into());
            return;
        }
        let root = self.project_asset_root();
        let protected = self.asset_protected_paths();
        let backend_root = self
            .distribution
            .as_ref()
            .map(|distribution| distribution.package_root.clone())
            .unwrap_or_else(project::source_root);
        let preparation = crate::host::asset_workspace_at(&root, protected.clone(), &backend_root)
            .and_then(|workspace| {
                let mut candidate = self.document.clone();
                workspace.synchronize(&mut candidate)?;
                self.validate_candidate(&candidate, self.can_author_engine())?;
                if candidate != self.document {
                    self.accept_asset_index(candidate);
                }
                if !self.save_workflow() {
                    return Err(self.status.clone());
                }
                Ok(())
            });
        if let Err(error) = preparation {
            self.report_error(error);
            return;
        }
        self.flush_history();
        let before = self.document.clone();
        let project_path = PathBuf::from(&self.path);
        self.spawn_job("Изменение ресурсов…", ctx, move || {
            let workspace = crate::host::asset_workspace_at(&root, protected.clone(), &backend_root)?;
            let manifest = AssetManifest::from_value(&before.asset_manifest)?;
            let operation = match action {
                BrowserAction::NewFolder { path } => workspace.create_folder(&before, &path)?,
                BrowserAction::Rename { id, name } => {
                    if name.is_empty() || name.contains(['/', '\\', ':']) || matches!(name.as_str(), "." | "..") { return Err("Введите имя файла без пути".into()); }
                    let entry = manifest.assets.get(&id).ok_or("Unknown asset ID")?;
                    let parent = entry.path.rsplit_once('/').ok_or("Asset parent missing")?.0;
                    workspace.move_asset(&before, &id, &format!("{parent}/{name}"))?
                }
                BrowserAction::Move { id, folder } => {
                    let entry = manifest.assets.get(&id).ok_or("Unknown asset ID")?;
                    let name = entry.path.rsplit('/').next().ok_or("Asset name missing")?;
                    workspace.move_asset(&before, &id, &format!("{folder}/{name}"))?
                }
                BrowserAction::Delete { id, replacement, detach } => {
                    let policy = replacement.map(DeletePolicy::ReplaceWith).unwrap_or(if detach {DeletePolicy::Detach} else {DeletePolicy::RefuseUsed});
                    workspace.delete_asset(&before, &id, policy)?
                }
                BrowserAction::Restore { trash_id } => workspace.restore_deleted(&before, &trash_id)?,
                BrowserAction::Import { folder } => {
                    if protected.iter().any(|path| folder == *path || folder.starts_with(&format!("{path}/"))) { return Err("Installed Engine resources are protected".into()); }
                    let Some(source) = native_dialogs::import_asset(&root)? else { return Ok(Outcome::Report("Импорт отменён".into())); };
                    import_resource_transaction(&root, &source, &folder, |path| workspace.adopt_import(&before, &[path]))?
                }
                _ => return Err("Unsupported asset transaction".into()),
            };
            let validation = operation.document.game.as_ref().map_or(Ok(()), |game| game.validate_scene_data());
            if let Err(error) = validation.and_then(|_| project::save(&project_path, &operation.document)) {
                let rollback = operation.record.undo(&root, &operation.document);
                return Err(match rollback { Ok(_) => format!("Сохранение не выполнено; файлы восстановлены: {error}"), Err(rollback) => format!("Сохранение: {error}; восстановление файлов: {rollback}. Журнал: .asset-history/{}", operation.record.id) });
            }
            Ok(Outcome::Asset(Box::new(operation)))
        });
    }
    /// Returns true when a filesystem boundary was handled (including rejection).
    pub(super) fn asset_history_replay(&mut self, forward: bool) -> bool {
        let id = if forward {
            self.history.redo_id()
        } else {
            Some(self.history.current_id())
        };
        let Some(record) = id
            .and_then(|id| self.workflow.asset_history.get(&id))
            .cloned()
        else {
            return false;
        };
        if self.game_workbench.is_play_session() || self.playing {
            self.status = "Остановите Play перед Undo файлов".into();
            return true;
        }
        let root = self.project_asset_root();
        let result = (|| {
            let document = if forward {
                record.redo(&root, &self.document)?
            } else {
                record.undo(&root, &self.document)?
            };
            if let Err(error) = self
                .validate_candidate(&document, self.can_author_engine())
                .and_then(|_| self.validate_asset_restore(&document))
                .and_then(|_| project::save(Path::new(&self.path), &document))
            {
                let rollback = if forward {
                    record.undo(&root, &document)
                } else {
                    record.redo(&root, &document)
                };
                return Err(format!(
                    "{error}; rollback: {}",
                    rollback
                        .err()
                        .unwrap_or_else(|| "файлы восстановлены".into())
                ));
            }
            if forward {
                self.history.redo();
            } else {
                self.history.undo();
            }
            self.restore_document(document);
            self.workflow.saved = self.document.clone();
            self.history_dirty = false;
            self.dirty = false;
            self.assets.invalidate();
            self.game_workbench.invalidate_assets();
            Ok(())
        })();
        self.status = match result {
            Ok(()) => format!(
                "{}: {}",
                if forward {
                    "Повторено"
                } else {
                    "Отменено"
                },
                record.label
            ),
            Err(error) => format!("Undo/Redo файлов не завершён: {error}"),
        };
        true
    }
    fn report_error(&mut self, error: String) {
        self.status = "Операция не выполнена; текущий проект остаётся открытым".into();
        self.workflow.report = error;
        self.workflow.show_report = true;
    }
    pub(super) fn workflow_frame(&mut self, ctx: &egui::Context) {
        match self.ui_capture.tick(ctx) {
            Ok(true) => {
                self.workflow.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(error) => {
                eprintln!("{error}");
                self.workflow.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Ok(false) => {}
        }
        let outcome = self
            .workflow
            .job
            .as_ref()
            .map(|job| job.receiver.try_recv());
        if let Some(result) = outcome {
            match result {
                Ok(result) => {
                    self.workflow.job = None;
                    match result {
                        Ok(Outcome::Dialog(purpose, path)) => {
                            self.complete_dialog(purpose, path, ctx)
                        }
                        Ok(Outcome::Saved(saved)) => {
                            let saved = *saved;
                            let name = saved.document.name;
                            if !self.load_workflow(saved.path) {
                                self.report_error(self.status.clone());
                            } else {
                                self.status = format!("Проект готов: {name}");
                            }
                        }
                        Ok(Outcome::Asset(operation)) => {
                            let operation = *operation;
                            if let Err(error) = self
                                .validate_candidate(&operation.document, self.can_author_engine())
                                .and_then(|_| self.validate_asset_restore(&operation.document))
                            {
                                let root = self.project_asset_root();
                                let rollback =
                                    operation.record.undo(&root, &operation.document).and_then(
                                        |before| project::save(Path::new(&self.path), &before),
                                    );
                                self.report_error(format!(
                                    "{error}; восстановление: {}",
                                    rollback
                                        .err()
                                        .unwrap_or_else(|| "файлы и ссылки восстановлены".into())
                                ));
                                return;
                            }
                            let label = operation.record.label.clone();
                            self.restore_document(operation.document.clone());
                            self.history_dirty = false;
                            let entry =
                                self.history.record_transaction(&label, &operation.document);
                            self.workflow.asset_history.insert(entry, operation.record);
                            let retained: std::collections::BTreeSet<_> =
                                self.history.entries().iter().map(|e| e.id).collect();
                            self.workflow
                                .asset_history
                                .retain(|id, _| retained.contains(id));
                            self.workflow.saved = self.document.clone();
                            self.dirty = false;
                            self.assets.invalidate();
                            self.game_workbench.invalidate_assets();
                            self.status = label;
                        }
                        Ok(Outcome::Report(report)) => {
                            self.status = "Операция завершена".into();
                            self.workflow.report = report;
                            self.workflow.show_report = true;
                        }
                        Err(error) => self.report_error(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.workflow.job = None;
                    self.report_error("Рабочий поток завершился без результата".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.workflow.allow_close {
            if self.workflow.job.is_some() || self.workflow.guard.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self.has_unsaved_work() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.workflow.guard = Some(Action::Quit);
            }
        }
        if !self.workflow.blocks_document_edits() {
            let text = ctx.text_edit_focused();
            let command = ctx.input_mut(|i| {
                if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
                    Some(Command::SaveAs)
                } else if i.consume_key(Modifiers::COMMAND, Key::S) {
                    Some(Command::Save)
                } else if i.consume_key(Modifiers::COMMAND, Key::O) {
                    Some(Command::Open)
                } else if i.consume_key(Modifiers::COMMAND, Key::N) {
                    Some(Command::NewEmpty)
                } else if !text
                    && (i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                        || i.consume_key(Modifiers::COMMAND, Key::Y))
                {
                    Some(Command::Redo)
                } else if !text && i.consume_key(Modifiers::COMMAND, Key::Z) {
                    Some(Command::Undo)
                } else {
                    None
                }
            });
            if let Some(command) = command {
                self.dispatch_command(command, ctx);
            }
        }
    }
    fn command_available(&self, command: Command) -> bool {
        if self.workflow.blocks_document_edits() {
            return false;
        }
        match command {
            Command::ExportSource | Command::ExportTeam | Command::UpdateEngineContracts => {
                self.can_author_engine()
            }
            Command::NewEmpty | Command::NewArenas | Command::NewPuzzle => {
                self.document.game.is_some() || self.can_author_engine()
            }
            Command::ExportPlayer => self.document.game.is_some(),
            Command::Undo => self.history_dirty || self.history.can_undo(),
            Command::Redo => self.history.can_redo(),
            Command::Stop => self.document.game.is_some(),
            _ => true,
        }
    }
    pub(super) fn dispatch_command(&mut self, command: Command, ctx: &egui::Context) {
        if !self.command_available(command) {
            return;
        }
        self.cancel_active_scene_gestures();
        match command {
            Command::NewEmpty => {
                self.request_action(Action::New(workspace_files::ProjectTemplate::Empty), ctx)
            }
            Command::NewArenas => self.request_action(
                Action::New(workspace_files::ProjectTemplate::TwoArenas),
                ctx,
            ),
            Command::NewPuzzle => {
                self.request_action(Action::New(workspace_files::ProjectTemplate::Puzzle), ctx)
            }
            Command::UpdateEngineContracts => {
                let backend = self
                    .distribution
                    .as_ref()
                    .map(|d| d.package_root.clone())
                    .unwrap_or_else(crate::project::source_root);
                match crate::engine_profiles::update_contracts(&self.document, &backend) {
                    Ok(candidate) => {
                        self.history
                            .record("Publish Engine profile and exact contracts", &candidate);
                        self.document = candidate;
                        self.authoring_intent = true;
                        self.changed(true);
                        self.status="Engine profile published locally; exact active contracts updated. Save to persist.".into();
                    }
                    Err(error) => self.status = error,
                }
            }
            Command::Open => self.request_action(Action::Open, ctx),
            Command::Save => {
                if !self.save_workflow() {
                    self.workflow.report = self.status.clone();
                    self.workflow.show_report = true;
                }
            }
            Command::SaveAs => match self.prepare_file_operation() {
                Ok(()) => self.start_dialog(DialogPurpose::SaveAs, ctx),
                Err(e) => self.report_error(e),
            },
            Command::Import => self.request_asset_action(
                BrowserAction::Import {
                    folder: "assets/imported".into(),
                },
                ctx,
            ),
            Command::ExportPlayer => self.request_action(Action::Export(ExportKind::Player), ctx),
            Command::ExportTeam => self.request_action(Action::Export(ExportKind::Team), ctx),
            Command::ExportSource => self.request_action(Action::Export(ExportKind::Source), ctx),
            Command::Check => self.request_action(Action::Check, ctx),
            Command::Quit => self.request_action(Action::Quit, ctx),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::History => self.open_window(EditorType::History),
            Command::Diagnostics => self.open_window(EditorType::Diagnostics),
            Command::ProjectSettings => self.open_window(EditorType::ProjectSettings),
            Command::SaveLayout => self.save_personal_layouts(),
            Command::LoadLayout => self.load_personal_layouts(),
            Command::ResetLayout => {
                let layout = DockLayout::default_for(self.workspace);
                match self.workspace {
                    WorkspaceKind::Engine => {
                        self.engine_layout = layout;
                        self.engine_panes.clear();
                    }
                    WorkspaceKind::Project => {
                        self.project_layout = layout;
                        self.project_panes.clear();
                    }
                }
                self.personal_dirty = true;
            }
            Command::PlayPause => self.playing = !self.playing,
            Command::Stop => self.stop_game(),
        }
    }
    fn menu_item(&self, ui: &mut egui::Ui, command: Command, label: &str, shortcut: &str) -> bool {
        let clicked = ui
            .add_enabled(
                self.command_available(command),
                egui::Button::new(label).shortcut_text(shortcut),
            )
            .clicked();
        if clicked {
            ui.close();
        }
        clicked
    }
    pub(super) fn draw_workflow_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut commands = vec![];
        let mut recent = None;
        let mut window = None;
        egui::MenuBar::new().ui(ui, |ui| {
            let menu = ui.menu_button("File", |ui| {
                ui.menu_button("New Project", |ui| {
                    use crate::engine_profiles::{EngineChoice, ProfileKind};
                    let selected =
                        module_engine_project::EngineProfile::from_document(&self.document)
                            .ok()
                            .map(|p| p.identity.id)
                            .unwrap_or_else(|| "unresolved".into());
                    ui.label(format!("Engine for New: {selected}"));
                    ui.add_enabled_ui(!self.workflow.blocks_document_edits(), |ui| {
                        ui.radio_value(
                            &mut self.workflow.new_engine_choice,
                            EngineChoice::InheritCurrent,
                            "Inherit current Engine",
                        );
                        ui.add_enabled_ui(self.can_author_engine(), |ui| {
                            for kind in
                                [ProfileKind::Neutral, ProfileKind::Arpg, ProfileKind::Puzzle]
                            {
                                ui.radio_value(
                                    &mut self.workflow.new_engine_choice,
                                    EngineChoice::Selected(kind),
                                    format!("Choose {} Engine", kind.label()),
                                );
                            }
                        });
                    });
                    ui.separator();
                    if self.menu_item(ui, Command::NewEmpty, "Empty scene", "Ctrl+N") {
                        commands.push(Command::NewEmpty);
                    }
                    if self.menu_item(ui, Command::NewArenas, "Two arenas example", "") {
                        commands.push(Command::NewArenas);
                    }
                    if self.menu_item(ui, Command::NewPuzzle, "Crate puzzle example", "") {
                        commands.push(Command::NewPuzzle);
                    }
                });
                if self.menu_item(ui, Command::Open, "Open Project…", "Ctrl+O") {
                    commands.push(Command::Open);
                }
                ui.menu_button("Recent Projects", |ui| {
                    if self.workflow.recent.paths.is_empty() {
                        ui.label("No recent projects");
                    }
                    for path in &self.workflow.recent.paths {
                        let label = format!(
                            "{} / {}",
                            path.parent()
                                .and_then(Path::file_name)
                                .unwrap_or_default()
                                .to_string_lossy(),
                            path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        if ui
                            .add_enabled(
                                !self.workflow.blocks_document_edits(),
                                egui::Button::new(label),
                            )
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            recent = Some(path.clone());
                            ui.close();
                        }
                    }
                });
                ui.separator();
                for (command, label, shortcut) in [
                    (Command::Save, "Save Project", "Ctrl+S"),
                    (Command::SaveAs, "Save As…", "Ctrl+Shift+S"),
                    (Command::Import, "Import Asset…", ""),
                ] {
                    if self.menu_item(ui, command, label, shortcut) {
                        commands.push(command);
                    }
                }
                ui.menu_button("Export", |ui| {
                    for (command, label) in [
                        (Command::ExportPlayer, "Windows Player…"),
                        (Command::ExportTeam, "Team editor…"),
                        (Command::ExportSource, "Full source project…"),
                    ] {
                        if self.menu_item(ui, command, label, "") {
                            commands.push(command);
                        }
                    }
                });
                ui.separator();
                if self.menu_item(ui, Command::Quit, "Exit", "") {
                    commands.push(Command::Quit);
                }
            });
            self.workflow
                .menu_rects
                .insert("File".into(), menu.response.rect);
            let menu = ui.menu_button("Edit", |ui| {
                for (command, label, shortcut) in [
                    (Command::Undo, "Undo", "Ctrl+Z"),
                    (Command::Redo, "Redo", "Ctrl+Y / Ctrl+Shift+Z"),
                    (Command::History, "History / Merge", ""),
                ] {
                    if self.menu_item(ui, command, label, shortcut) {
                        commands.push(command);
                    }
                }
            });
            self.workflow
                .menu_rects
                .insert("Edit".into(), menu.response.rect);
            let menu = ui.menu_button("Tools", |ui| {
                for (command, label) in [
                    (Command::Check, "Check Project"),
                    (
                        Command::UpdateEngineContracts,
                        "Publish Engine profile / update contracts",
                    ),
                    (Command::ExportPlayer, "Build Windows Player…"),
                    (Command::Diagnostics, "Diagnostics"),
                    (Command::ProjectSettings, "Project Settings"),
                ] {
                    if self.menu_item(ui, command, label, "") {
                        commands.push(command);
                    }
                }
                if ui
                    .add_enabled(
                        !self.workflow.report.is_empty(),
                        egui::Button::new("Last operation report"),
                    )
                    .clicked()
                {
                    self.workflow.show_report = true;
                    ui.close();
                }
            });
            self.workflow
                .menu_rects
                .insert("Tools".into(), menu.response.rect);
            let menu = ui.menu_button("Windows", |ui| {
                for descriptor in self.available_editors() {
                    if ui
                        .add_enabled(
                            !self.workflow.blocks_document_edits(),
                            egui::Button::new(descriptor.label),
                        )
                        .clicked()
                    {
                        window = Some(descriptor.editor);
                        ui.close();
                    }
                }
                ui.separator();
                for (command, label) in [
                    (Command::SaveLayout, "Save Layout"),
                    (Command::LoadLayout, "Load Saved Layout"),
                    (Command::ResetLayout, "Reset Layout"),
                ] {
                    if self.menu_item(ui, command, label, "") {
                        commands.push(command);
                    }
                }
            });
            self.workflow
                .menu_rects
                .insert("Windows".into(), menu.response.rect);
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.strong("MODULE ENGINE / 0.4");
            ui.separator();
            ui.add_enabled_ui(!self.workflow.blocks_document_edits(), |ui| {
                ui.selectable_value(&mut self.workspace, WorkspaceKind::Project, "Project");
                if self.can_author_engine() {
                    ui.selectable_value(&mut self.workspace, WorkspaceKind::Engine, "Engine");
                }
            });
            ui.separator();
            for (command, label) in [
                (
                    Command::PlayPause,
                    if self.playing { "Pause" } else { "Play" },
                ),
                (Command::Stop, "Stop"),
                (Command::Save, "Save"),
            ] {
                if ui
                    .add_enabled(self.command_available(command), egui::Button::new(label))
                    .clicked()
                {
                    commands.push(command);
                }
            }
            ui.separator();
            ui.label(&self.document.name).on_hover_text(&self.path);
            if self.has_unsaved_work() {
                ui.colored_label(Color32::from_rgb(240, 183, 92), "● unsaved");
            }
            if let Some(job) = &self.workflow.job {
                ui.spinner();
                ui.label(format!(
                    "{} {:.1}s",
                    job.title,
                    job.started.elapsed().as_secs_f32()
                ));
            }
        });
        let ctx = ui.ctx().clone();
        if let Some(path) = recent {
            self.request_action(Action::OpenPath(path), &ctx);
        }
        if let Some(kind) = window {
            self.open_window(kind);
        }
        for command in commands {
            self.dispatch_command(command, &ctx);
        }
    }
    /// Explicit screenshot diagnostics click a real menu after its first layout.
    /// Ordinary input is untouched; only the calling application's egui stream
    /// receives these events, never other desktop windows.
    pub(super) fn workflow_capture_input(&mut self, input: &mut egui::RawInput) {
        let Some(menu) = &self.workflow.capture_menu else {
            return;
        };
        let Some(rect) = self.workflow.menu_rects.get(menu) else {
            return;
        };
        let pos = rect.center();
        input.focused = true;
        input.events.extend([
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]);
        self.workflow.capture_menu = None;
    }
    pub(super) fn draw_workflow_modals(&mut self, ctx: &egui::Context) {
        if self.workflow.guard.is_some() {
            let response = egui::Modal::new(egui::Id::new("unsaved-project")).show(ctx,|ui| {
                ui.set_min_width(440.); ui.heading("Несохранённые изменения");
                ui.label("Сохранить текущий проект перед продолжением?");
                if self.game_workbench.has_graph_draft() { ui.label("Есть черновик Gameplay Systems. Нажмите «Отмена», примените или отмените черновик в редакторе и повторите сохранение."); }
                if !self.status.is_empty() { ui.label(&self.status); }
                ui.horizontal(|ui| {
                    if ui.button("Сохранить и продолжить").clicked() { Some(0) }
                    else if !matches!(self.workflow.guard, Some(Action::Asset(_))) && ui.button(if matches!(self.workflow.guard, Some(Action::Check | Action::Export(_))) { "Использовать сохранённую версию" } else { "Не сохранять" }).clicked() { Some(1) }
                    else if ui.button("Отмена").clicked() { Some(2) } else { None }
                }).inner
            });
            if let Some(choice) = response.inner {
                self.resolve_guard(choice, ctx);
            } else if response.should_close() {
                self.workflow.guard = None;
            }
        }
        if self.workflow.show_report {
            let response = egui::Modal::new(egui::Id::new("operation-report")).show(ctx, |ui| {
                ui.set_min_width(560.);
                ui.heading("Результат операции");
                egui::ScrollArea::vertical()
                    .max_height(360.)
                    .show(ui, |ui| {
                        ui.label(&self.workflow.report);
                    });
                ui.button("Закрыть").clicked()
            });
            if response.inner || response.should_close() {
                self.workflow.show_report = false;
            }
        }
    }
    fn resolve_guard(&mut self, choice: u8, ctx: &egui::Context) {
        match choice {
            0 => {
                if self.save_workflow()
                    && let Some(action) = self.workflow.guard.take()
                {
                    self.perform_action(action, ctx);
                }
            }
            1 => {
                if matches!(self.workflow.guard, Some(Action::Asset(_))) {
                    return;
                }
                if let Some(action) = self.workflow.guard.take() {
                    self.workflow.new_uses_saved = matches!(action, Action::New(_));
                    self.perform_action(action, ctx);
                }
            }
            _ => self.workflow.guard = None,
        }
    }
}
fn read_tail(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => {
            String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(32_768)..]).into_owned()
        }
        Err(e) => e.to_string(),
    }
}
pub(super) fn import_resource(root: &Path, source: &Path) -> Result<String, String> {
    import_resource_into(root, source, "assets/imported/files")
}
fn import_resource_into(root: &Path, source: &Path, folder: &str) -> Result<String, String> {
    import_resource_transaction(root, source, folder, Ok)
}

/// One import owns one fresh nonce directory. Adoption may still reject links
/// or fail to persist history; neither failure may leave unindexed source files.
fn import_resource_transaction<T>(
    root: &Path,
    source: &Path,
    folder: &str,
    commit: impl FnOnce(String) -> Result<T, String>,
) -> Result<T, String> {
    let folder = module_engine_project::normalize_relative(folder)?;
    if folder != "assets" && !folder.starts_with("assets/") {
        return Err("Import folder must belong to assets/".into());
    }
    let bytes = fs::read(source).map_err(|error| format!("Import: {error}"))?;
    let extension = source
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "glb" | "gltf" | "png" | "jpg" | "jpeg" | "wav"
    ) {
        return Err("Поддержаны glTF/GLB, PNG/JPEG и WAV".into());
    }
    if extension == "wav"
        && (bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE")
    {
        return Err("Expected a RIFF WAV file".into());
    }
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let parent = root.join(&folder);
    let mut ancestor = parent.as_path();
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or("Import folder has no project ancestor")?;
    }
    if !ancestor
        .canonicalize()
        .map_err(|error| error.to_string())?
        .starts_with(&root)
    {
        return Err("Import destination leaves project".into());
    }
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let parent = parent.canonicalize().map_err(|error| error.to_string())?;
    if !parent.starts_with(&root) {
        return Err("Import destination leaves project".into());
    }
    let hash = module_engine_project::hash_bytes(&bytes);
    let import_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let name = format!("{}-{import_id}", &hash[..16]);
    let directory = format!("{folder}/{name}");
    let owned = parent.join(&name);
    // create_dir is atomic: an occupied directory is never eligible for cleanup.
    fs::create_dir(&owned).map_err(|error| format!("Reserve import directory: {error}"))?;
    let result = (|| {
        let name = source
            .file_name()
            .ok_or("Source has no filename")?
            .to_string_lossy();
        let mut library = module_engine_content::ContentLibrary::new(&root)?;
        let imported = match extension.as_str() {
            "glb" | "gltf" => library.import_model(source, &directory),
            "png" | "jpg" | "jpeg" => {
                library.import_texture(source, &format!("{directory}/{name}"))
            }
            "wav" => AssetWorkspace::new(&root, Vec::<String>::new())?
                .write_new_file(&format!("{directory}/{name}"), &bytes),
            _ => unreachable!(),
        }?;
        commit(imported)
    })();
    if let Err(error) = result {
        let cleanup = (|| {
            let resolved = owned.canonicalize().map_err(|error| error.to_string())?;
            if resolved != owned || !resolved.starts_with(&root) || resolved == root {
                return Err("Import cleanup path changed or leaves project".into());
            }
            fs::remove_dir_all(&resolved).map_err(|error| error.to_string())
        })();
        return Err(match cleanup {
            Ok(()) => error,
            Err(cleanup) => format!(
                "{error}; очистка нового импорта: {cleanup}; {}",
                owned.display()
            ),
        });
    }
    result
}

/// Reproduce the editor's project/file services and real menu interaction, then
/// render/export the relocated imported model. Native dialog cancellation/ABI
/// and guarded command transitions have independent unit tests.
pub(crate) fn verify_workflow(source: &Path, output: &Path) -> Result<serde_json::Value, String> {
    use module_engine_core::{AuthoredObject, Transform, WorldObject};
    use workspace_files::ProjectTemplate;
    let source = source.canonicalize().map_err(|e| e.to_string())?;
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let output = output.canonicalize().map_err(|e| e.to_string())?;
    let original_hash = module_engine_project::hash_file(&source)?;
    let document = project::read(&source)?;
    let distribution = crate::distribution::detect(&source, &document, None)?;
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let case = output.join(format!("case-{id}"));
    fs::create_dir(&case).map_err(|e| e.to_string())?;
    let empty = workspace_files::new_project(
        &source,
        &case.join("empty"),
        &document,
        &distribution,
        ProjectTemplate::Empty,
    )?;
    let arenas = workspace_files::new_project(
        &source,
        &case.join("arenas"),
        &document,
        &distribution,
        ProjectTemplate::TwoArenas,
    )?;
    if empty.document.root != document.root
        || arenas.document.root != document.root
        || empty.document.engine != document.engine
        || arenas.document.engine != document.engine
    {
        return Err("New Project replaced the selected Engine".into());
    }
    let empty_root = empty.path.parent().ok_or("Empty project root missing")?;
    let imported = import_resource(
        empty_root,
        &distribution
            .package_root
            .join("assets/models/demo_crystal.glb"),
    )?;
    let mut edited = empty.document;
    let mut object = AuthoredObject::new(WorldObject {
        id: 1,
        parent: None,
        transform: Transform {
            scale: [2.; 3],
            ..Transform::default()
        },
        color: [255; 4],
    });
    object
        .components
        .insert("mesh".into(), serde_json::json!({"path":imported}));
    let game = edited.game.as_mut().ok_or("New project has no scene")?;
    game.scenes
        .get_mut(&game.entry_scene)
        .ok_or("Entry scene missing")?
        .objects
        .push(object);
    let unassigned = empty_root.join("assets/unassigned-author-note.txt");
    fs::write(
        &unassigned,
        b"not assigned to any scene; preserve on Save As",
    )
    .map_err(|e| e.to_string())?;
    project::save(&empty.path, &edited)?;
    let edited = project::read(&empty.path)?;
    let context = crate::distribution::detect(&empty.path, &edited, None)?;
    let copy = workspace_files::save_as(
        &empty.path,
        &case.join("copy/project.json"),
        &edited,
        &context,
    )?;
    if copy.document != edited {
        return Err("Save As changed author document".into());
    }
    let copy_root = copy.path.parent().ok_or("Copied root missing")?;
    if fs::read(copy_root.join("assets/unassigned-author-note.txt")).map_err(|e| e.to_string())?
        != fs::read(&unassigned).map_err(|e| e.to_string())?
    {
        return Err("Save As lost unassigned author asset".into());
    }
    let mut content = module_engine_content::ContentLibrary::new(copy_root)?;
    let model = content.load_model(&imported)?;
    let copy_context = crate::distribution::detect(&copy.path, &copy.document, None)?;
    let mut host = crate::game_host::new(
        copy.document.clone(),
        copy_root,
        Some(&copy_context.compile_root()?),
        WorldKind::Objects,
    )?;
    host.services.set_enabled(false);
    host.step(1. / 60., &module_engine_core::GameInput::default())?;
    let frame = host.render(640, 480)?;
    // Player rendering omits diagnostic depth readbacks. Compare actual output
    // against the same frame without geometry instead of relying on those stats.
    host.session
        .apply_commands(&[module_engine_core::WorldCommand::Remove { id: 1 }])?;
    let without_model = host.render(640, 480)?;
    let model_pixels = frame
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(without_model.rgba.as_chunks::<4>().0.iter())
        .filter(|(model, empty)| model != empty)
        .count();
    if model_pixels == 0 {
        return Err("Imported relocated model did not render".into());
    }
    crate::write_png(&case.join("relocated-model.png"), &frame)?;
    drop(host);
    let mut editor = Editor::new_with_distribution(
        copy.document.clone(),
        copy.path.clone(),
        copy_context.clone(),
    );
    editor.workspace = WorkspaceKind::Project;
    editor.playing = false;
    let mut menus = BTreeMap::new();
    for (menu, expected) in [
        ("File", "Open Project"),
        ("Edit", "History / Merge"),
        ("Tools", "Check Project"),
        ("Windows", "Save Layout"),
    ] {
        let ctx = egui::Context::default();
        configure_style(&ctx);
        let raw = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1500., 900.),
            )),
            focused: true,
            ..Default::default()
        };
        ctx.run_ui(raw(), |ui| editor.draw_workflow_toolbar(ui))
            .drop_without_applying_deltas();
        ctx.run_ui(raw(), |ui| editor.draw_workflow_toolbar(ui))
            .drop_without_applying_deltas();
        let pos = editor
            .workflow
            .menu_rects
            .get(menu)
            .ok_or("Menu rectangle missing")?
            .center();
        let mut input = raw();
        input.events = vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ];
        ctx.run_ui(input, |ui| editor.draw_workflow_toolbar(ui))
            .drop_without_applying_deltas();
        // A newly opened popup needs a sizing frame before its text is painted.
        ctx.run_ui(raw(), |ui| editor.draw_workflow_toolbar(ui))
            .drop_without_applying_deltas();
        let result = ctx.run_ui(raw(), |ui| editor.draw_workflow_toolbar(ui));
        let visible = result.shapes.iter().any(|shape| matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text().contains(expected)));
        result.drop_without_applying_deltas();
        if !visible {
            return Err(format!("Actual {menu} menu did not display {expected}"));
        }
        menus.insert(menu, true);
    }
    let before = editor.project_layout.panes().len();
    editor.open_window(EditorType::History);
    let opened = editor.project_layout.panes().len();
    editor.open_window(EditorType::History);
    if editor.workspace != WorkspaceKind::Project
        || editor.project_layout.panes().len() != opened
        || opened < before
    {
        return Err("Windows duplicated a pane or changed workspace".into());
    }
    let player = crate::game_package::export(
        &copy.document,
        copy_root,
        &copy_context.package_root,
        &copy_context.compile_root()?,
        &case.join("player"),
    )?;
    let player_doc = crate::game_package::validate_package(&player)?;
    let mut player_host = crate::game_host::new(player_doc, &player, None, WorldKind::Objects)?;
    player_host.services.set_enabled(false);
    player_host.step(1. / 60., &module_engine_core::GameInput::default())?;
    let player_frame = player_host.render(640, 480)?;
    if player_frame.rgba != frame.rgba {
        return Err("Exported player changed relocated model pixels".into());
    }
    crate::write_png(&case.join("player-model.png"), &player_frame)?;
    if module_engine_project::hash_file(&source)? != original_hash {
        return Err("Workflow changed original project".into());
    }
    let report = serde_json::json!({"passed":true,"menus_clicked":menus,"new_templates_preserve_engine":true,
        "copy_retains_author_document":true,"unassigned_asset_copied":true,"imported_model":imported,
        "model_primitives":model.primitives.len(),"model_pixels_changed":model_pixels,
        "windows_reuses_current_workspace":true,"exported_player_pixels_equal":true,"original_project_unchanged":true,
        "case_folder":case,"copy_project":copy.path,"player_folder":player,
        "scope":"Real project services, actual egui menu clicks, Vulkan imported model, precompiled Player. Native picker selection is not automated here."});
    fs::write(
        output.join("workflow-report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(report)
}

#[cfg(test)]
mod workflow_tests {
    use super::*;
    use eframe::egui::{Event, Id, Pos2, Rect, Sense, Vec2};
    use module_engine_core::{WorldKind, demo_document};

    struct Fixture {
        root: PathBuf,
        path: PathBuf,
    }
    impl Fixture {
        fn new(game: bool) -> (Self, Editor) {
            let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.work/test-temp");
            fs::create_dir_all(&base).unwrap();
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = base.join(format!("editor-workflow-{}-{stamp}", std::process::id()));
            fs::create_dir(&root).unwrap();
            let path = root.join("project.json");
            let mut document = demo_document(WorldKind::Objects);
            if game {
                document.game = Some(module_engine_two_arenas::demo_project());
            }
            let installation = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
            crate::engine_profiles::finish_template(
                &mut document,
                if game {
                    crate::engine_profiles::ProfileKind::Arpg
                } else {
                    crate::engine_profiles::ProfileKind::Neutral
                },
                &installation,
            )
            .unwrap();
            if let Some(game) = &document.game {
                for path in game.audio_cues.values() {
                    let target = root.join(path);
                    fs::create_dir_all(target.parent().unwrap()).unwrap();
                    fs::copy(installation.join(path), target).unwrap();
                }
            }
            let distribution = crate::distribution::detect(
                &installation.join("project.json"),
                &document,
                Some(crate::distribution::DistributionProfile::Full),
            )
            .unwrap();
            assert!(distribution.can_author_engine());
            fs::write(
                crate::distribution::project_binding_path(&path).unwrap(),
                serde_json::to_vec_pretty(
                    &crate::distribution::ProjectEngineBinding::from_distribution(&distribution)
                        .unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            crate::project::save(&path, &document).unwrap();
            let document = crate::project::read(&path).unwrap();
            let mut editor = Editor::new(document, path.clone());
            editor.playing = false;
            (Self { root, path }, editor)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only this test's uniquely created directory is eligible for cleanup.
            let allowed = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.work/test-temp")
                .canonicalize()
                .unwrap();
            if let Ok(root) = self.root.canonicalize() {
                assert!(root.starts_with(&allowed));
                assert!(
                    root.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("editor-workflow-")
                );
                let _ = fs::remove_dir_all(root);
            }
        }
    }

    fn edit_name(editor: &mut Editor) {
        editor.document.name = "Unsaved working scene".into();
        editor.changed(false);
    }
    fn complete_asset_job(editor: &mut Editor, ctx: &egui::Context) {
        let started = Instant::now();
        while editor.workflow.job.is_some() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(15),
                "asset job timed out"
            );
            let result = ctx.run_ui(egui::RawInput::default(), |ui| {
                editor.workflow_frame(ui.ctx())
            });
            result.drop_without_applying_deltas();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!editor.workflow.show_report, "{}", editor.workflow.report);
    }
    #[test]
    fn asset_save_guard_cancel_and_file_undo_redo_share_one_boundary() {
        let (fixture, mut editor) = Fixture::new(true);
        let ctx = egui::Context::default();
        fs::create_dir_all(fixture.root.join("assets")).unwrap();
        fs::write(fixture.root.join("assets/note.txt"), b"author note").unwrap();
        let service = AssetWorkspace::new(&fixture.root, Vec::<String>::new()).unwrap();
        let infos = service.synchronize(&mut editor.document).unwrap();
        let id = infos
            .iter()
            .find(|i| i.path == "assets/note.txt")
            .unwrap()
            .id
            .clone();
        assert!(editor.save_workflow());
        edit_name(&mut editor);
        let action = BrowserAction::Rename {
            id,
            name: "renamed.txt".into(),
        };
        editor.request_asset_action(action.clone(), &ctx);
        assert!(matches!(editor.workflow.guard, Some(Action::Asset(_))));
        editor.resolve_guard(2, &ctx);
        assert!(fixture.root.join("assets/note.txt").is_file());
        assert!(!fixture.root.join("assets/renamed.txt").exists());
        editor.request_asset_action(action, &ctx);
        editor.resolve_guard(0, &ctx);
        complete_asset_job(&mut editor, &ctx);
        assert!(fixture.root.join("assets/renamed.txt").is_file());
        assert!(!fixture.root.join("assets/note.txt").exists());
        assert_eq!(project::read(&fixture.path).unwrap(), editor.document);
        let accepted = editor.document.clone();
        assert!(editor.save_workflow()); // Save preserves the filesystem journal.
        editor.undo();
        assert!(fixture.root.join("assets/note.txt").is_file());
        assert!(!fixture.root.join("assets/renamed.txt").exists());
        assert_eq!(project::read(&fixture.path).unwrap(), editor.document);
        editor.redo();
        assert_eq!(editor.document, accepted);
        assert!(fixture.root.join("assets/renamed.txt").is_file());
        fs::write(fixture.root.join("assets/renamed.txt"), b"external edit").unwrap();
        let cursor = editor.history.cursor();
        editor.undo();
        assert_eq!(editor.history.cursor(), cursor);
        assert_eq!(editor.document, accepted);
        assert_eq!(
            fs::read(fixture.root.join("assets/renamed.txt")).unwrap(),
            b"external edit"
        );
    }
    #[test]
    fn folder_transaction_has_undo_even_when_project_json_is_identical() {
        let (fixture, mut editor) = Fixture::new(true);
        let ctx = egui::Context::default();
        fs::create_dir_all(fixture.root.join("assets")).unwrap();
        AssetWorkspace::new(&fixture.root, Vec::<String>::new())
            .unwrap()
            .synchronize(&mut editor.document)
            .unwrap();
        assert!(editor.save_workflow());
        let before = editor.document.clone();
        editor.request_asset_action(
            BrowserAction::NewFolder {
                path: "assets/models".into(),
            },
            &ctx,
        );
        complete_asset_job(&mut editor, &ctx);
        assert_eq!(editor.document, before);
        assert!(fixture.root.join("assets/models").is_dir());
        editor.undo();
        assert!(!fixture.root.join("assets/models").exists());
        assert_eq!(editor.document, before);
        editor.redo();
        assert!(fixture.root.join("assets/models").is_dir());
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn keyboard_frame(
        ctx: &egui::Context,
        editor: &mut Editor,
        text: &mut String,
        text_focus: bool,
        time: f64,
        mut events: Vec<Event>,
    ) {
        let modifiers = events
            .iter()
            .find_map(|event| {
                if let Event::Key { modifiers, .. } = event {
                    Some(*modifiers)
                } else {
                    None
                }
            })
            .unwrap_or_default();
        events.insert(0, Event::ModifiersChanged(modifiers));
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900., 650.))),
                time: Some(time),
                focused: true,
                events,
                ..Default::default()
            },
            |ui| {
                // Match the real editor order: shortcuts run before pane widgets.
                editor.workflow_frame(ui.ctx());
                let edit = ui.add(egui::TextEdit::singleline(text).id(Id::new("workflow-text")));
                let viewport = ui.interact(
                    Rect::from_min_max(Pos2::new(0., 50.), Pos2::new(900., 650.)),
                    Id::new("workflow-viewport"),
                    Sense::click(),
                );
                if text_focus {
                    edit.request_focus();
                } else {
                    viewport.request_focus();
                }
            },
        );
        output.drop_without_applying_deltas();
    }

    #[test]
    fn unsaved_project_guard_cancel_preserves_work_and_no_save_quit_keeps_document() {
        let (_fixture, mut editor) = Fixture::new(false);
        let ctx = egui::Context::default();
        edit_name(&mut editor);
        let working = editor.document.clone();
        editor.dispatch_command(Command::Quit, &ctx);
        assert!(matches!(editor.workflow.guard, Some(Action::Quit)));
        assert!(!editor.workflow.allow_close);
        editor.resolve_guard(2, &ctx);
        assert!(editor.workflow.guard.is_none());
        assert_eq!(editor.document, working);
        assert!(editor.has_unsaved_work());
        editor.dispatch_command(Command::Quit, &ctx);
        editor.resolve_guard(1, &ctx);
        assert!(editor.workflow.allow_close);
        assert_eq!(editor.document, working);
        assert!(editor.has_unsaved_work());
    }

    #[test]
    fn invalid_json_cannot_save_and_continue_or_silently_replace_working_model() {
        let (fixture, mut editor) = Fixture::new(false);
        let ctx = egui::Context::default();
        let model = editor.document.clone();
        let on_disk = fs::read(&fixture.path).unwrap();
        editor.json_draft = "{ unfinished JSON".into();
        editor.json_dirty = true;
        editor.dispatch_command(Command::Quit, &ctx);
        assert!(matches!(editor.workflow.guard, Some(Action::Quit)));
        editor.resolve_guard(0, &ctx);
        assert!(matches!(editor.workflow.guard, Some(Action::Quit)));
        assert!(!editor.workflow.allow_close);
        assert_eq!(editor.document, model);
        assert_eq!(editor.json_draft, "{ unfinished JSON");
        assert!(editor.json_dirty);
        assert_eq!(fs::read(&fixture.path).unwrap(), on_disk);
    }

    #[test]
    fn failed_save_does_not_advance_guard_or_clear_dirty_state() {
        let (fixture, mut editor) = Fixture::new(false);
        let ctx = egui::Context::default();
        edit_name(&mut editor);
        let working = editor.document.clone();
        let saved = editor.workflow.saved.clone();
        let blocked_target = fixture.root.join("folder-instead-of-project.json");
        fs::create_dir(&blocked_target).unwrap();
        editor.path = blocked_target.to_string_lossy().into_owned();
        editor.dispatch_command(Command::Quit, &ctx);
        editor.resolve_guard(0, &ctx);
        assert!(matches!(editor.workflow.guard, Some(Action::Quit)));
        assert!(!editor.workflow.allow_close);
        assert!(editor.dirty);
        assert!(editor.has_unsaved_work());
        assert_eq!(editor.document, working);
        assert_eq!(editor.workflow.saved, saved);
        assert_eq!(crate::project::read(&fixture.path).unwrap(), saved);
    }

    #[test]
    fn canceled_picker_and_failed_open_preserve_working_path_document_and_history() {
        let (fixture, mut editor) = Fixture::new(false);
        let ctx = egui::Context::default();
        edit_name(&mut editor);
        editor.flush_history();
        let working = editor.document.clone();
        let path = editor.path.clone();
        let history_len = editor.history.entries().len();
        editor.complete_dialog(DialogPurpose::Open, None, &ctx);
        editor.complete_dialog(
            DialogPurpose::New(workspace_files::ProjectTemplate::Empty),
            None,
            &ctx,
        );
        assert_eq!(editor.path, path);
        assert_eq!(editor.document, working);
        assert_eq!(editor.history.entries().len(), history_len);
        assert!(editor.has_unsaved_work());
        // No native picker is spawned: use the same guarded path with a missing
        // Recent entry. Declining Save must not discard edits before load succeeds.
        editor.request_action(Action::OpenPath(fixture.root.join("missing.json")), &ctx);
        assert!(editor.workflow.guard.is_some());
        editor.resolve_guard(1, &ctx);
        assert_eq!(editor.path, path);
        assert_eq!(editor.document, working);
        assert_eq!(editor.history.entries().len(), history_len);
        assert!(editor.has_unsaved_work());
        assert!(editor.workflow.show_report);
        assert!(editor.workflow.job.is_none());
    }

    #[test]
    fn project_undo_works_with_viewport_focus_but_text_undo_stays_in_text_edit() {
        let (_fixture, mut editor) = Fixture::new(false);
        let ctx = egui::Context::default();
        let original_name = editor.document.name.clone();
        let mut text = "seed".to_owned();
        edit_name(&mut editor);
        keyboard_frame(&ctx, &mut editor, &mut text, false, 0., vec![]);
        assert!(ctx.egui_wants_keyboard_input());
        assert!(!ctx.text_edit_focused());
        keyboard_frame(
            &ctx,
            &mut editor,
            &mut text,
            false,
            0.1,
            vec![key(Key::Z, Modifiers::COMMAND)],
        );
        assert_eq!(editor.document.name, original_name);

        edit_name(&mut editor);
        let working = editor.document.clone();
        keyboard_frame(&ctx, &mut editor, &mut text, true, 1., vec![]);
        assert!(ctx.text_edit_focused());
        keyboard_frame(
            &ctx,
            &mut editor,
            &mut text,
            true,
            1.1,
            vec![Event::Text("a".into())],
        );
        keyboard_frame(&ctx, &mut editor, &mut text, true, 2.1, vec![]);
        let first_edit = text.clone();
        keyboard_frame(
            &ctx,
            &mut editor,
            &mut text,
            true,
            2.2,
            vec![Event::Text("b".into())],
        );
        keyboard_frame(&ctx, &mut editor, &mut text, true, 3.2, vec![]);
        assert_ne!(text, first_edit);
        let before_undo = text.clone();
        keyboard_frame(
            &ctx,
            &mut editor,
            &mut text,
            true,
            3.3,
            vec![key(Key::Z, Modifiers::COMMAND)],
        );
        assert_ne!(
            text, before_undo,
            "TextEdit must receive the unconsumed Undo key"
        );
        assert_eq!(editor.document, working);
        assert!(editor.history_dirty);
    }

    #[test]
    fn operation_report_blocks_file_shortcuts_and_project_undo() {
        let (fixture, mut editor) = Fixture::new(false);
        let ctx = egui::Context::default();
        edit_name(&mut editor);
        let working = editor.document.clone();
        let on_disk = fs::read(&fixture.path).unwrap();
        let mut text = String::new();
        editor.workflow.report = "A completed operation report".into();
        editor.workflow.show_report = true;
        for (index, command) in [Key::O, Key::N, Key::S, Key::Z].into_iter().enumerate() {
            keyboard_frame(
                &ctx,
                &mut editor,
                &mut text,
                false,
                index as f64,
                vec![key(command, Modifiers::COMMAND)],
            );
        }
        assert_eq!(editor.document, working);
        assert_eq!(fs::read(&fixture.path).unwrap(), on_disk);
        assert!(editor.has_unsaved_work());
        assert!(editor.workflow.guard.is_none());
        assert!(editor.workflow.job.is_none());
        assert!(!editor.workflow.allow_close);
    }

    #[test]
    fn windows_commands_stay_in_selected_workspace_and_capture_seeds_active_area() {
        let (_fixture, mut editor) = Fixture::new(false);
        editor.workspace = WorkspaceKind::Project;
        editor.project_layout = DockLayout::single(EditorType::Scene);
        editor.project_panes.clear();
        editor.engine_layout = DockLayout::single(EditorType::ModuleGraph);
        let engine_before = editor.engine_layout.clone();
        let document_before = editor.document.clone();
        editor.open_window(EditorType::ModuleGraph);
        assert_eq!(editor.workspace, WorkspaceKind::Project);
        assert_eq!(editor.engine_layout, engine_before);
        assert_eq!(
            editor.project_layout.panes().first(),
            Some(&(1, EditorType::Scene))
        );
        let active = editor.active_window().unwrap();
        assert_eq!(editor.project_layout.panes().len(), 2);
        editor.open_window(EditorType::ModuleGraph);
        assert_eq!(editor.active_window(), Some(active));
        assert_eq!(editor.project_layout.panes().len(), 2);
        editor.graph_canvas.zoom = 0.83;
        editor.seed_capture_pane();
        assert!(editor.project_panes.contains_key(&active));
        assert!(!editor.project_panes.contains_key(&1));
        assert_eq!(editor.project_panes[&active].graph_canvas.zoom, 0.83);
        assert_eq!(editor.document, document_before);

        editor.workspace = WorkspaceKind::Engine;
        editor.open_window(EditorType::ModuleGraph);
        assert_eq!(editor.workspace, WorkspaceKind::Engine);
        assert_eq!(editor.engine_layout, engine_before);
        assert_eq!(editor.active_window(), Some(1));
        assert_eq!(editor.project_layout.panes().len(), 2);
    }

    #[test]
    fn local_gameplay_graph_draft_requires_resolution_before_save_and_continue() {
        let (fixture, mut editor) = Fixture::new(true);
        let ctx = egui::Context::default();
        // Give Undo a real execution change that would invalidate a pending draft.
        editor.document.game.as_mut().unwrap().execution.stages[0]
            .name
            .push_str(" applied");
        editor.changed(false);
        editor.flush_history();
        project::save(&fixture.path, &editor.document).unwrap();
        editor.workflow.saved = editor.document.clone();
        let original = editor.document.clone();
        let stage_name = original.game.as_ref().unwrap().execution.stages[0]
            .name
            .clone();
        fn frame(
            ctx: &egui::Context,
            editor: &mut Editor,
            root: &Path,
            time: f64,
            events: Vec<Event>,
        ) -> egui::FullOutput {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600., 2000.))),
                    time: Some(time),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ui| {
                    assert!(
                        !editor
                            .game_workbench
                            .systems_ui(ui, &mut editor.document, root)
                    );
                },
            )
        }
        fn text_point(output: &egui::FullOutput, label: &str) -> Pos2 {
            output
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape
                        && text.galley.text() == label
                        && shape
                            .clip_rect
                            .contains(text.pos + text.galley.size() * 0.5)
                    {
                        Some(text.pos + text.galley.size() * 0.5)
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| panic!("Visible UI text not found: {label}"))
        }
        fn pointer(point: Pos2, pressed: bool) -> Vec<Event> {
            vec![
                Event::PointerMoved(point),
                Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]
        }
        frame(&ctx, &mut editor, &fixture.root, 0., vec![]).drop_without_applying_deltas();
        let output = frame(&ctx, &mut editor, &fixture.root, 0.1, vec![]);
        let header = text_point(&output, "Stages, order and declared resources");
        output.drop_without_applying_deltas();
        // Open the actual collapsing header; guessing its private auto-ID would
        // accidentally target the canvas node also labelled "Input".
        frame(&ctx, &mut editor, &fixture.root, 0.2, pointer(header, true))
            .drop_without_applying_deltas();
        frame(
            &ctx,
            &mut editor,
            &fixture.root,
            0.3,
            pointer(header, false),
        )
        .drop_without_applying_deltas();
        let output = frame(&ctx, &mut editor, &fixture.root, 1., vec![]);
        let stage = text_point(&output, &stage_name);
        output.drop_without_applying_deltas();
        frame(&ctx, &mut editor, &fixture.root, 1.1, pointer(stage, true))
            .drop_without_applying_deltas();
        frame(&ctx, &mut editor, &fixture.root, 1.2, pointer(stage, false))
            .drop_without_applying_deltas();
        assert!(
            ctx.text_edit_focused(),
            "Stage-name TextEdit must receive the pointer gesture"
        );
        frame(
            &ctx,
            &mut editor,
            &fixture.root,
            1.3,
            vec![Event::Text(" changed locally".into())],
        )
        .drop_without_applying_deltas();
        assert!(editor.game_workbench.has_graph_draft());
        assert_eq!(editor.document, original);
        assert!(editor.has_unsaved_work());
        let history_cursor = editor.history.cursor();
        assert!(editor.history.can_undo());
        editor.dispatch_command(Command::Undo, &ctx);
        editor.undo();
        editor.redo();
        assert_eq!(editor.history.cursor(), history_cursor);
        assert_eq!(editor.document, original);
        assert!(editor.game_workbench.has_graph_draft());
        let mut json_candidate = original.clone();
        json_candidate.game.as_mut().unwrap().execution.stages[0].name = "JSON replacement".into();
        editor.json_draft = serde_json::to_string_pretty(&json_candidate).unwrap();
        editor.json_dirty = true;
        assert!(!editor.save_workflow());
        assert!(!editor.apply_json());
        assert!(editor.json_dirty);
        assert_eq!(editor.document, original);
        assert!(editor.game_workbench.has_graph_draft());
        editor.discard_json_draft();
        editor.dispatch_command(Command::Quit, &ctx);
        editor.resolve_guard(0, &ctx);
        assert!(matches!(editor.workflow.guard, Some(Action::Quit)));
        assert!(!editor.workflow.allow_close);
        assert!(editor.game_workbench.has_graph_draft());
        assert_eq!(editor.document, original);
        assert_eq!(crate::project::read(&fixture.path).unwrap(), original);
        editor.resolve_guard(2, &ctx);
        assert!(
            editor.game_workbench.has_graph_draft(),
            "Cancel only dismisses the guard"
        );
        let library_ctx = egui::Context::default();
        let library_frame = |editor: &mut Editor, events| {
            library_ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900., 1800.))),
                    events,
                    ..Default::default()
                },
                |ui| editor.library(ui),
            )
        };
        let output = library_frame(&mut editor, vec![]);
        let export = text_point(&output, "Экспорт проекта…");
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            library_frame(&mut editor, pointer(export, pressed)).drop_without_applying_deltas();
        }
        assert!(matches!(
            editor.workflow.guard,
            Some(Action::Export(ExportKind::Source))
        ));
        assert!(
            editor.workflow.job.is_none(),
            "Library must not export synchronously or open a picker before resolving the draft"
        );
        editor.resolve_guard(0, &library_ctx);
        assert!(matches!(
            editor.workflow.guard,
            Some(Action::Export(ExportKind::Source))
        ));
        assert!(editor.game_workbench.has_graph_draft());
        assert_eq!(crate::project::read(&fixture.path).unwrap(), original);
        editor.resolve_guard(2, &library_ctx);
    }

    #[test]
    fn failed_import_or_adoption_removes_only_the_fresh_import_directory() {
        let (fixture, mut editor) = Fixture::new(false);
        fs::create_dir_all(fixture.root.join("assets")).unwrap();
        fs::write(fixture.root.join("assets/keep.txt"), b"author bytes").unwrap();
        let workspace = AssetWorkspace::new(&fixture.root, Vec::<String>::new()).unwrap();
        workspace.synchronize(&mut editor.document).unwrap();
        let original = editor.document.clone();
        let glb = include_bytes!("../../../assets/models/demo_crystal.glb");
        let length = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let mut json: serde_json::Value = serde_json::from_slice(&glb[20..20 + length]).unwrap();
        json["extensionsUsed"] = serde_json::json!(["CUSTOM_optional_extension"]);
        let mut json = serde_json::to_vec(&json).unwrap();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let binary = &glb[20 + length..];
        let total = 20 + json.len() + binary.len();
        let mut encoded = b"glTF".to_vec();
        encoded.extend(2u32.to_le_bytes());
        encoded.extend((total as u32).to_le_bytes());
        encoded.extend((json.len() as u32).to_le_bytes());
        encoded.extend(b"JSON");
        encoded.extend(json);
        encoded.extend(binary);
        let source = fixture.root.join("optional.glb");
        fs::write(&source, encoded).unwrap();
        let Err(error) = import_resource_transaction(&fixture.root, &source, "assets", |path| {
            assert!(
                fixture.root.join(&path).is_file(),
                "the content importer must have succeeded before adoption rejects its extension"
            );
            workspace.adopt_import(&original, &[path])
        }) else {
            panic!("unsupported relocation semantics must reject adoption");
        };
        assert!(error.to_lowercase().contains("extension"), "{error}");
        let files = || {
            fs::read_dir(fixture.root.join("assets"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>()
        };
        assert_eq!(files(), vec![std::ffi::OsString::from("keep.txt")]);
        let wav = crate::project::source_root().join("assets/audio/attack.wav");
        let error = import_resource_transaction::<()>(&fixture.root, &wav, "assets", |_| {
            Err("Injected persistence failure".into())
        })
        .unwrap_err();
        assert_eq!(error, "Injected persistence failure");
        assert_eq!(files(), vec![std::ffi::OsString::from("keep.txt")]);
        fs::write(&source, b"not glb").unwrap();
        assert!(
            import_resource_transaction::<()>(&fixture.root, &source, "assets", |_| panic!(
                "invalid content cannot reach adoption"
            ))
            .is_err()
        );
        assert_eq!(files(), vec![std::ffi::OsString::from("keep.txt")]);
        assert_eq!(
            fs::read(fixture.root.join("assets/keep.txt")).unwrap(),
            b"author bytes"
        );
        assert_eq!(editor.document, original);
    }
}
