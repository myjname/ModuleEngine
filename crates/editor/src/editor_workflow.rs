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
    #[cfg(test)]
    picker_result: Option<Result<Option<PathBuf>, String>>,
    #[cfg(test)]
    picker_requests: Vec<(DialogPurpose, PathBuf)>,
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
            #[cfg(test)]
            picker_result: None,
            #[cfg(test)]
            picker_requests: vec![],
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
        if let Some(root) = &self.diagnostic_personal_root {
            self.workflow.recent_path = Some(root.join("editor-recent-projects.json"));
        } else {
            #[cfg(test)]
            {
                self.workflow.recent_path = Some(
                    self.test_personal_settings
                        .path()
                        .join(".user/editor-recent-projects.json"),
                );
            }
            #[cfg(not(test))]
            {
                self.workflow.recent_path = self
                    .distribution
                    .as_ref()
                    .map(|d| d.package_root.join(".user/editor-recent-projects.json"));
            }
        }
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
        if let Some(path) = &self.workflow.recent_path {
            let result = path
                .parent()
                .ok_or("Recent projects have no parent".to_string())
                .and_then(|p| fs::create_dir_all(p).map_err(|e| e.to_string()))
                .and_then(|_| {
                    serde_json::to_vec_pretty(&self.workflow.recent).map_err(|e| e.to_string())
                })
                .and_then(|bytes| fs::write(path, bytes).map_err(|e| e.to_string()));
            if let Err(error) = result {
                self.status = format!("Project opened; Recent list was not saved: {error}");
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
            return Err("Apply or discard the Gameplay Systems draft before saving. It is not included in the saved project.".into());
        }
        if self.json_dirty && !self.apply_json() {
            return Err(self.status.clone());
        }
        self.validate_candidate(&self.document, self.can_author_engine())
    }
    /// Stage pending JSON for Save As without crossing the authoring-state
    /// boundary. A cancelled picker or failed publication must retain the draft.
    fn file_operation_candidate(&self) -> Result<GraphDocument, String> {
        if self.game_workbench.has_graph_draft() {
            return Err("Apply or discard the Gameplay Systems draft before saving. It is not included in the saved project.".into());
        }
        let candidate = if self.json_dirty {
            serde_json::from_str(&self.json_draft)
                .map_err(|error| format!("JSON not applied: {error}"))?
        } else {
            self.document.clone()
        };
        self.validate_candidate(&candidate, self.can_author_engine())?;
        Ok(candidate)
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
            Action::Check => self.start_cli("Checking project", "--check", None, ctx),
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
                .unwrap_or_else(|_| Err("The operation failed with an internal error".into()));
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
        #[cfg(test)]
        {
            // Exercise real menu dispatch/worker completion without a native
            // modal window. Native COM/Show acceptance is a separate boundary.
            self.workflow
                .picker_requests
                .push((purpose, initial.clone()));
            if let Some(result) = self.workflow.picker_result.take() {
                self.spawn_job("Selecting file / folder…", ctx, move || {
                    result.map(|path| Outcome::Dialog(purpose, path))
                });
                return;
            }
        }
        self.spawn_job("Selecting file / folder…", ctx, move || {
            let directory = initial.parent().unwrap_or(Path::new("."));
            let path = match purpose {
                DialogPurpose::Open => native_dialogs::open_project(directory)?,
                DialogPurpose::SaveAs => native_dialogs::save_project(&initial)?,
                DialogPurpose::New(_) => native_dialogs::pick_folder(
                    "New Project — select a new or empty folder",
                    directory,
                )?,
                DialogPurpose::Export(_) => {
                    native_dialogs::pick_folder("Export — select a new or empty folder", directory)?
                }
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
                "Operation canceled; the current project and its edits remain open".into();
            return;
        };
        let save_candidate = if matches!(purpose, DialogPurpose::SaveAs) {
            match self.file_operation_candidate() {
                Ok(candidate) => Some(candidate),
                Err(error) => {
                    self.report_error(error);
                    return;
                }
            }
        } else {
            None
        };
        if matches!(purpose, DialogPurpose::SaveAs)
            && path
                .canonicalize()
                .ok()
                .zip(Path::new(&self.path).canonicalize().ok())
                .is_some_and(|(a, b)| same_path(&a, &b))
        {
            let candidate = save_candidate.expect("Save As candidate");
            if let Err(error) = project::save(&path, &candidate) {
                self.report_error(error);
                return;
            }
            if self.json_dirty || candidate != self.document {
                self.apply_validated_json(candidate);
            }
            self.flush_history();
            self.workflow.saved = self.document.clone();
            self.dirty = false;
            self.status = format!("Saved: {}", self.path);
            self.remember_project();
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
                    self.report_error("Engine context unavailable".into());
                    return;
                };
                let source = PathBuf::from(&self.path);
                let document = if matches!(purpose, DialogPurpose::SaveAs) {
                    save_candidate.expect("Save As candidate")
                } else if matches!(purpose, DialogPurpose::New(_)) && self.workflow.new_uses_saved {
                    self.workflow.saved.clone()
                } else {
                    self.document.clone()
                };
                self.workflow.new_uses_saved = false;
                let engine_choice = self.workflow.new_engine_choice;
                self.spawn_job("Preparing project and copying assets…", ctx, move || {
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
                                workspace_files::save_as(&source, &path, &document, &distribution)?
                            }
                        }
                        DialogPurpose::New(template) => workspace_files::new_project_with_choice(
                            &source,
                            &path,
                            &document,
                            &distribution,
                            template,
                            engine_choice,
                        )?,
                        _ => unreachable!(),
                    };
                    Ok(Outcome::Saved(Box::new(saved)))
                });
            }
            DialogPurpose::Export(kind) => {
                self.start_cli("Building / exporting…", kind.argument(), Some(path), ctx)
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
                .map_err(|e| format!("Save the project before checking/exporting: {e}"))?;
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
                .map_err(|e| format!("Starting tool: {e}"))?;
            let report = format!(
                "{}\n{}\nLogs: {}",
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
            self.report_error("Stop Play before changing project files".into());
            return;
        }
        let root = self.project_asset_root();
        let protected = self.asset_protected_paths();
        let backend_root = self
            .distribution
            .as_ref()
            .map(|distribution| distribution.package_root.clone())
            .unwrap_or_else(project::source_root);
        let property_object_ids = self.asset_property_object_ids();
        let preparation = crate::host::asset_workspace_at(
            &root,
            protected.clone(),
            &backend_root,
            property_object_ids.clone(),
        )
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
        self.spawn_job("Changing assets…", ctx, move || {
            let workspace = crate::host::asset_workspace_at(
                &root,
                protected.clone(),
                &backend_root,
                property_object_ids,
            )?;
            let manifest = AssetManifest::from_value(&before.asset_manifest)?;
            let operation = match action {
                BrowserAction::NewFolder { path } => workspace.create_folder(&before, &path)?,
                BrowserAction::Rename { id, name } => {
                    if name.is_empty()
                        || name.contains(['/', '\\', ':'])
                        || matches!(name.as_str(), "." | "..")
                    {
                        return Err("Enter a file name without a path".into());
                    }
                    let entry = manifest.assets.get(&id).ok_or("Unknown asset ID")?;
                    let parent = entry.path.rsplit_once('/').ok_or("Asset parent missing")?.0;
                    workspace.move_asset(&before, &id, &format!("{parent}/{name}"))?
                }
                BrowserAction::Move { id, folder } => {
                    let entry = manifest.assets.get(&id).ok_or("Unknown asset ID")?;
                    let name = entry.path.rsplit('/').next().ok_or("Asset name missing")?;
                    workspace.move_asset(&before, &id, &format!("{folder}/{name}"))?
                }
                BrowserAction::Delete {
                    id,
                    replacement,
                    detach,
                } => {
                    let policy = replacement
                        .map(DeletePolicy::ReplaceWith)
                        .unwrap_or(if detach {
                            DeletePolicy::Detach
                        } else {
                            DeletePolicy::RefuseUsed
                        });
                    workspace.delete_asset(&before, &id, policy)?
                }
                BrowserAction::Restore { trash_id } => {
                    workspace.restore_deleted(&before, &trash_id)?
                }
                BrowserAction::Import { folder } => {
                    if protected
                        .iter()
                        .any(|path| folder == *path || folder.starts_with(&format!("{path}/")))
                    {
                        return Err("Installed Engine resources are protected".into());
                    }
                    let Some(source) = native_dialogs::import_asset(&root)? else {
                        return Ok(Outcome::Report("Import canceled".into()));
                    };
                    import_resource_transaction(&root, &source, &folder, |path| {
                        workspace.adopt_import(&before, &[path])
                    })?
                }
                _ => return Err("Unsupported asset transaction".into()),
            };
            let validation = operation
                .document
                .game
                .as_ref()
                .map_or(Ok(()), |game| game.validate_scene_data());
            if let Err(error) =
                validation.and_then(|_| project::save(&project_path, &operation.document))
            {
                let rollback = operation.record.undo(&root, &operation.document);
                return Err(match rollback {
                    Ok(_) => format!("Save failed; files restored: {error}"),
                    Err(rollback) => format!(
                        "Save: {error}; file restoration: {rollback}. Log: .asset-history/{}",
                        operation.record.id
                    ),
                });
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
            self.status = "Stop Play before undoing file changes".into();
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
                    rollback.err().unwrap_or_else(|| "files restored".into())
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
                if forward { "Redone" } else { "Undone" },
                record.label
            ),
            Err(error) => format!("File Undo/Redo did not complete: {error}"),
        };
        true
    }
    fn report_error(&mut self, error: String) {
        self.status = "Operation failed; the current project remains open".into();
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
                                self.status = format!("Project ready: {name}");
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
                                    "{error}; restoration: {}",
                                    rollback
                                        .err()
                                        .unwrap_or_else(|| "files and references restored".into())
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
                            if operation.diagnostics.is_empty() {
                                self.status = label;
                            } else {
                                self.status = format!(
                                    "{label}; {} warnings — see report",
                                    operation.diagnostics.len()
                                );
                                self.workflow.report =
                                    format!("{label}\n\n{}", operation.diagnostics.join("\n\n"));
                                self.workflow.show_report = true;
                            }
                        }
                        Ok(Outcome::Report(report)) => {
                            self.status = "Operation complete".into();
                            self.workflow.report = report;
                            self.workflow.show_report = true;
                        }
                        Err(error) => self.report_error(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.workflow.job = None;
                    self.report_error("The worker thread exited without a result".into());
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
            Command::SaveAs => match self.file_operation_candidate() {
                Ok(_) => self.start_dialog(DialogPurpose::SaveAs, ctx),
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
                ui.set_min_width(440.); ui.heading("Unsaved changes");
                ui.label("Save the current project before continuing?");
                if self.game_workbench.has_graph_draft() { ui.label("There is a Gameplay Systems draft. Click Cancel, apply or discard the draft in the editor, then try saving again."); }
                if !self.status.is_empty() { ui.label(&self.status); }
                ui.horizontal(|ui| {
                    if ui.button("Save and continue").clicked() { Some(0) }
                    else if !matches!(self.workflow.guard, Some(Action::Asset(_))) && ui.button(if matches!(self.workflow.guard, Some(Action::Check | Action::Export(_))) { "Use saved version" } else { "Don't save" }).clicked() { Some(1) }
                    else if ui.button("Cancel").clicked() { Some(2) } else { None }
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
                ui.heading("Operation result");
                egui::ScrollArea::vertical()
                    .max_height(360.)
                    .show(ui, |ui| {
                        ui.label(&self.workflow.report);
                    });
                ui.button("Close").clicked()
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
        return Err("Supported formats: glTF/GLB, PNG/JPEG, and WAV".into());
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
                "{error}; new import cleanup: {cleanup}; {}",
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
    let personal_root = crate::ui_capture::diagnostic_personal_root()?;
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
    let mut editor = Editor::new_with_diagnostic_personal_root(
        copy.document.clone(),
        copy.path.clone(),
        copy_context.clone(),
        personal_root,
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
        owner: PathBuf,
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
            let owner = base.join(format!("editor-workflow-{}-{stamp}", std::process::id()));
            fs::create_dir(&owner).unwrap();
            let owner = owner.canonicalize().unwrap();
            let root = owner.join("project");
            fs::create_dir(&root).unwrap();
            let installation = owner.join("engine");
            crate::distribution::copy_native_fixture(&installation);
            let path = root.join("project.json");
            let mut document = demo_document(WorldKind::Objects);
            if game {
                document.game = Some(module_engine_two_arenas::demo_project());
            }
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
            crate::distribution::write_full_manifest(&installation, &document).unwrap();
            let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
            if let Some(game) = &document.game {
                for path in game.audio_cues.values() {
                    let target = root.join(path);
                    fs::create_dir_all(target.parent().unwrap()).unwrap();
                    fs::copy(checkout.join(path), target).unwrap();
                }
            }
            let distribution = crate::distribution::detect(
                &installation.join("project.json"),
                &document,
                Some(crate::distribution::DistributionProfile::Full),
            )
            .unwrap();
            assert!(
                distribution.can_author_engine(),
                "{:?}",
                distribution.source_status.diagnostics
            );
            assert_eq!(distribution.package_root, installation);
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
            assert!(editor.can_author_engine());
            assert_eq!(
                editor.distribution.as_ref().unwrap().package_root,
                installation
            );
            (Self { owner, root, path }, editor)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only this test's uniquely created directory is eligible for cleanup.
            let allowed = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.work/test-temp")
                .canonicalize()
                .unwrap();
            if let Ok(owner) = self.owner.canonicalize() {
                assert!(owner.starts_with(&allowed));
                assert!(
                    owner
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("editor-workflow-")
                );
                let _ = fs::remove_dir_all(owner);
            }
        }
    }

    #[test]
    fn recent_projects_save_load_is_isolated_per_test_editor() {
        let (fixture, mut first) = Fixture::new(false);
        let first_path = first.workflow.recent_path.clone().unwrap();
        assert!(first_path.starts_with(first.test_personal_settings.path()));
        assert_ne!(
            first_path,
            first
                .distribution
                .as_ref()
                .unwrap()
                .package_root
                .join(".user/editor-recent-projects.json")
        );
        let additional_project = fixture.root.join("another-project.json");
        fs::write(&additional_project, b"{}").unwrap();
        first.path = additional_project.to_string_lossy().into_owned();
        first.remember_project();
        let written: RecentProjects =
            serde_json::from_slice(&fs::read(&first_path).unwrap()).unwrap();
        assert_eq!(written.version, 1);
        assert_eq!(written.paths[0], additional_project.canonicalize().unwrap());
        first.workflow.recent = RecentProjects::default();
        first.path = fixture.path.to_string_lossy().into_owned();
        first.initialize_workflow();
        assert!(
            first
                .workflow
                .recent
                .paths
                .contains(&additional_project.canonicalize().unwrap())
        );
        assert_eq!(
            first.workflow.recent.paths[0],
            fixture.path.canonicalize().unwrap()
        );
        let mut second = Editor::new(first.document.clone(), fixture.path.clone());
        let second_path = second.workflow.recent_path.clone().unwrap();
        assert_ne!(first_path, second_path);
        assert!(second_path.starts_with(second.test_personal_settings.path()));
        second.initialize_workflow();
        assert_eq!(
            second.workflow.recent.paths,
            vec![fixture.path.canonicalize().unwrap()]
        );
        assert!(first_path.is_file() && second_path.is_file());
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

    fn preservation_cases() -> Vec<(&'static str, serde_json::Value, Vec<u8>)> {
        vec![
            ("literal.key", serde_json::Value::Null, vec![]),
            (
                "0",
                serde_json::json!([null, false, {"literal.key": 7}]),
                vec![0, 255, 1, 0],
            ),
            ("", serde_json::json!({}), b"author\r\nbytes\n".to_vec()),
            ("a/b", serde_json::json!([]), vec![0, 255, 1, 0]),
            ("a~b", serde_json::json!(""), vec![]),
            (
                "имя-😀",
                serde_json::json!({"0": null, "nested": {"key": true}}),
                b"author\r\nbytes\n".to_vec(),
            ),
        ]
    }

    fn preservation_archive(key: &str, value: serde_json::Value) -> serde_json::Value {
        let mut payload = serde_json::Map::new();
        payload.insert(key.into(), value);
        payload.insert("foreign-id".into(), serde_json::json!("foreign-stable-id"));
        payload.insert("nested".into(), serde_json::json!({"literal": {"key": 11}}));
        serde_json::Value::Object(payload)
    }

    fn preservation_asset_fixture(
        archive: serde_json::Value,
        bytes: &[u8],
    ) -> (Fixture, Editor, String, String) {
        let (fixture, mut editor) = Fixture::new(false);
        fs::create_dir_all(fixture.root.join("assets")).unwrap();
        fs::write(fixture.root.join("assets/original.bin"), bytes).unwrap();
        fs::write(
            fixture.root.join("assets/untouched.bin"),
            b"unrelated author bytes",
        )
        .unwrap();
        editor
            .document
            .legacy_archive
            .insert("foreign.namespace".into(), archive.clone());
        editor
            .document
            .properties
            .inactive
            .global
            .insert("foreign.namespace".into(), archive);
        let workspace = AssetWorkspace::new(&fixture.root, Vec::<String>::new()).unwrap();
        let infos = workspace.synchronize(&mut editor.document).unwrap();
        let id = infos
            .iter()
            .find(|info| info.path == "assets/original.bin")
            .unwrap()
            .id
            .clone();
        let untouched = infos
            .iter()
            .find(|info| info.path == "assets/untouched.bin")
            .unwrap()
            .id
            .clone();
        editor.changed(false);
        assert!(editor.save_workflow(), "{}", editor.status);
        (fixture, editor, id, untouched)
    }

    #[test]
    fn preservation_save_as_matrix_keeps_source_and_opaque_drafts_on_cancel_conflict_and_copy() {
        for (key, value, bytes) in preservation_cases() {
            for mode in ["cancel", "conflict", "copy"] {
                let label = format!("key={key:?}, mode={mode}");
                let (fixture, mut editor) = Fixture::new(true);
                let ctx = egui::Context::default();
                let archive = preservation_archive(key, value.clone());
                editor
                    .document
                    .legacy_archive
                    .insert("foreign.namespace".into(), archive.clone());
                editor
                    .document
                    .properties
                    .inactive
                    .global
                    .insert("foreign.namespace".into(), archive);
                let mut candidate = stage_save_as_json_draft(&fixture, &mut editor);
                fs::write(fixture.root.join("assets/preservation.bin"), &bytes).unwrap();
                let mut manifest = AssetManifest::from_value(&candidate.asset_manifest).unwrap();
                manifest
                    .register(
                        "preservation-stable-id",
                        "assets/preservation.bin",
                        &fixture.root,
                    )
                    .unwrap();
                candidate.asset_manifest = serde_json::to_value(manifest).unwrap();
                editor.json_draft = serde_json::to_string_pretty(&candidate).unwrap();
                let destination = fixture.root.join("copy-😀").join("project.json");
                if mode == "conflict" {
                    fs::create_dir_all(destination.parent().unwrap().join("assets")).unwrap();
                    fs::write(
                        destination
                            .parent()
                            .unwrap()
                            .join("assets/save-as-note.txt"),
                        b"existing destination resource",
                    )
                    .unwrap();
                    fs::write(destination.parent().unwrap().join("keep.bin"), [8, 0, 255]).unwrap();
                }
                let before = file_command_state(&editor);
                let files = fixture_files(&fixture.root);
                let recent_path = editor.workflow.recent_path.clone().unwrap();
                assert!(
                    !recent_path.starts_with(&fixture.root),
                    "Recent must remain in isolated personal settings"
                );
                let recent = fs::read(&recent_path).unwrap();
                editor.workflow.picker_result =
                    Some(Ok((mode != "cancel").then_some(destination.clone())));
                editor.dispatch_command(Command::SaveAs, &ctx);
                assert_file_command_state(&editor, &before, &format!("{label}: dispatch"));
                drain_file_job(&mut editor, &ctx);
                for (relative, hash) in &files {
                    assert_eq!(
                        module_engine_project::hash_file(&fixture.root.join(relative)).unwrap(),
                        *hash,
                        "{label}: source/target changed: {}",
                        relative.display()
                    );
                }
                if mode == "copy" {
                    assert!(
                        !editor.workflow.show_report,
                        "{label}: {}",
                        editor.workflow.report
                    );
                    assert_eq!(project::read(&destination).unwrap(), candidate, "{label}");
                    assert_eq!(editor.document, candidate, "{label}");
                    assert_eq!(editor.workflow.saved, candidate, "{label}");
                    assert_eq!(Path::new(&editor.path), destination.canonicalize().unwrap());
                    assert_eq!(
                        editor.workflow.recent.paths[0],
                        destination.canonicalize().unwrap()
                    );
                    assert_eq!(
                        fs::read(crate::distribution::project_binding_path(&destination).unwrap())
                            .unwrap(),
                        fs::read(crate::distribution::project_binding_path(&fixture.path).unwrap())
                            .unwrap(),
                        "{label}: copied Engine binding"
                    );
                    assert_eq!(
                        fs::read(
                            destination
                                .parent()
                                .unwrap()
                                .join("assets/preservation.bin")
                        )
                        .unwrap(),
                        bytes,
                        "{label}"
                    );
                    let copied =
                        AssetManifest::from_value(&editor.document.asset_manifest).unwrap();
                    assert_eq!(
                        copied.assets["preservation-stable-id"].path,
                        "assets/preservation.bin"
                    );
                    copied.verify(destination.parent().unwrap()).unwrap();
                    assert!(!editor.json_dirty && !editor.has_unsaved_work());
                } else {
                    assert_file_command_state(&editor, &before, &format!("{label}: completion"));
                    assert_eq!(
                        fixture_files(&fixture.root),
                        files,
                        "{label}: unaccounted files"
                    );
                    assert_eq!(
                        fs::read(&recent_path).unwrap(),
                        recent,
                        "{label}: Recent changed"
                    );
                    assert_eq!(editor.workflow.show_report, mode == "conflict", "{label}");
                    assert!(!destination.exists(), "{label}: candidate published");
                }
            }
        }
    }

    #[test]
    fn preservation_asset_replay_failed_json_publish_rolls_back_undo_and_redo() {
        for operation in ["rename", "delete"] {
            for forward in [false, true] {
                let label = format!("{operation}, forward={forward}");
                let archive =
                    preservation_archive("literal.key", serde_json::json!([null, {"0": false}]));
                let bytes = [0, 255, 1, 0];
                let (fixture, mut editor, id, _) = preservation_asset_fixture(archive, &bytes);
                let ctx = egui::Context::default();
                let original = editor.document.clone();
                let action = if operation == "rename" {
                    BrowserAction::Rename {
                        id,
                        name: "renamed.bin".into(),
                    }
                } else {
                    BrowserAction::Delete {
                        id,
                        replacement: None,
                        detach: false,
                    }
                };
                editor.request_asset_action(action, &ctx);
                complete_asset_job(&mut editor, &ctx);
                let accepted = editor.document.clone();
                if forward {
                    editor.undo();
                    assert_eq!(editor.document, original, "{label}: setup Undo");
                }
                let expected = if forward { &accepted } else { &original };
                let valid_path_state = file_command_state(&editor);
                let blocked = fixture.root.join("blocked-project.json");
                fs::create_dir(&blocked).unwrap();
                editor.path = blocked.to_string_lossy().into_owned();
                let before = file_command_state(&editor);
                let files = fixture_files(&fixture.root);
                let recent_path = editor.workflow.recent_path.clone().unwrap();
                let recent = fs::read(&recent_path).unwrap();
                if forward {
                    editor.redo();
                } else {
                    editor.undo();
                }
                assert!(
                    editor
                        .status
                        .starts_with("File Undo/Redo did not complete:"),
                    "{label}: {}",
                    editor.status
                );
                assert_file_command_state(&editor, &before, &label);
                let mut remaining = fixture_files(&fixture.root);
                for (relative, hash) in &files {
                    assert_eq!(
                        remaining.remove(relative).as_ref(),
                        Some(hash),
                        "{label}: failed replay changed {}",
                        relative.display()
                    );
                }
                // The writer retains its staged JSON after failed replacement.
                // It is recovery data, not permission to mutate any original.
                assert_eq!(
                    remaining.len(),
                    1,
                    "{label}: unexpected recovery: {remaining:?}"
                );
                let recovery = remaining.keys().next().unwrap();
                assert_eq!(recovery.components().count(), 1);
                assert!(
                    recovery
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("blocked-project.tmp-")
                );
                assert_eq!(
                    project::read(&fixture.root.join(recovery)).unwrap(),
                    *expected,
                    "{label}: recovery candidate"
                );
                assert_eq!(
                    fs::read(&recent_path).unwrap(),
                    recent,
                    "{label}: Recent changed"
                );
                editor.path = fixture.path.to_string_lossy().into_owned();
                assert_file_command_state(
                    &editor,
                    &valid_path_state,
                    &format!("{label}: path restored"),
                );
                if forward {
                    editor.redo();
                } else {
                    editor.undo();
                }
                assert_eq!(
                    editor.document, *expected,
                    "{label}: retry must remain usable"
                );
                assert_eq!(
                    project::read(&fixture.path).unwrap(),
                    *expected,
                    "{label}: retry persistence"
                );
                if !forward {
                    assert_eq!(
                        fs::read(fixture.root.join("assets/original.bin")).unwrap(),
                        bytes,
                        "{label}: restored bytes"
                    );
                }
            }
        }
    }

    #[test]
    fn preservation_asset_branch_keeps_ids_bytes_and_opaque_payload_after_move_delete_undo_redo() {
        for (key, value, bytes) in preservation_cases() {
            let archive = preservation_archive(key, value);
            let (fixture, mut editor, id, untouched) =
                preservation_asset_fixture(archive.clone(), &bytes);
            let ctx = egui::Context::default();
            let original = editor.document.clone();
            let binding_path = crate::distribution::project_binding_path(&fixture.path).unwrap();
            let binding = fs::read(&binding_path).unwrap();
            editor.request_asset_action(
                BrowserAction::Rename {
                    id: id.clone(),
                    name: "renamed.bin".into(),
                },
                &ctx,
            );
            complete_asset_job(&mut editor, &ctx);
            let rename_id = editor.history.current_id();
            assert_eq!(
                AssetManifest::from_value(&editor.document.asset_manifest)
                    .unwrap()
                    .assets[&id]
                    .path,
                "assets/renamed.bin"
            );
            assert_eq!(
                fs::read(fixture.root.join("assets/renamed.bin")).unwrap(),
                bytes
            );
            editor.undo();
            assert_eq!(editor.document, original, "{key:?}: rename Undo");
            assert!(editor.history.can_redo());
            editor.request_asset_action(
                BrowserAction::Delete {
                    id: id.clone(),
                    replacement: None,
                    detach: false,
                },
                &ctx,
            );
            complete_asset_job(&mut editor, &ctx);
            let deleted = editor.document.clone();
            let delete_id = editor.history.current_id();
            assert!(delete_id > rename_id, "history identity must not be reused");
            assert!(!editor.history.can_redo());
            assert!(
                !editor.workflow.asset_history.contains_key(&rename_id),
                "discarded branch retained a filesystem record"
            );
            let deleted_manifest = AssetManifest::from_value(&deleted.asset_manifest).unwrap();
            assert!(!deleted_manifest.assets.contains_key(&id));
            assert_eq!(
                deleted_manifest.assets[&untouched].path,
                "assets/untouched.bin"
            );
            let before_unavailable_redo = file_command_state(&editor);
            let files = fixture_files(&fixture.root);
            editor.redo();
            assert_file_command_state(&editor, &before_unavailable_redo, "discarded branch Redo");
            assert_eq!(fixture_files(&fixture.root), files);
            for (forward, expected) in [(false, &original), (true, &deleted), (false, &original)] {
                if forward {
                    editor.redo();
                } else {
                    editor.undo();
                }
                assert_eq!(editor.document, *expected, "{key:?}: forward={forward}");
                assert_eq!(
                    project::read(&fixture.path).unwrap(),
                    *expected,
                    "{key:?}: persisted boundary"
                );
                assert_eq!(
                    editor.document.legacy_archive["foreign.namespace"], archive,
                    "{key:?}: exact archive"
                );
                assert_eq!(
                    editor.document.properties.inactive.global["foreign.namespace"], archive,
                    "{key:?}: inactive payload"
                );
                assert_eq!(
                    fs::read(fixture.root.join("assets/untouched.bin")).unwrap(),
                    b"unrelated author bytes"
                );
                assert!(
                    !fixture.root.join("assets/renamed.bin").exists(),
                    "discarded branch was replayed"
                );
                if !forward {
                    assert_eq!(
                        fs::read(fixture.root.join("assets/original.bin")).unwrap(),
                        bytes
                    );
                    assert_eq!(
                        AssetManifest::from_value(&editor.document.asset_manifest)
                            .unwrap()
                            .assets[&id]
                            .path,
                        "assets/original.bin"
                    );
                } else {
                    assert!(!fixture.root.join("assets/original.bin").exists());
                }
                assert_eq!(fs::read(&binding_path).unwrap(), binding);
            }
        }
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
    fn save_as_cancel_picker_error_and_publish_failure_preserve_dirty_json_and_play() {
        for mode in ["cancel", "picker-error", "copy-error", "same-path-error"] {
            let (fixture, mut editor) = Fixture::new(true);
            let ctx = egui::Context::default();
            let candidate = stage_save_as_json_draft(&fixture, &mut editor);
            let result = match mode {
                "cancel" => Ok(None),
                "picker-error" => Err("Injected picker configuration error".into()),
                _ => {
                    let blocked = fixture.root.join(format!("{mode}.json"));
                    fs::create_dir(&blocked).unwrap();
                    if mode == "same-path-error" {
                        // A directory at the current JSON destination supplies
                        // a deterministic write error on both Windows/Linux.
                        editor.path = blocked.to_string_lossy().into_owned();
                    }
                    Ok(Some(blocked))
                }
            };
            let state = file_command_state(&editor);
            let files = fixture_files(&fixture.root);
            let recent = fs::read(editor.workflow.recent_path.as_ref().unwrap()).unwrap();
            editor.workflow.picker_result = Some(result);
            editor.dispatch_command(Command::SaveAs, &ctx);
            assert_file_command_state(&editor, &state, &format!("{mode}: dispatch"));
            drain_file_job(&mut editor, &ctx);
            assert_file_command_state(&editor, &state, &format!("{mode}: completion"));
            let mut remaining = fixture_files(&fixture.root);
            for (relative, original) in &files {
                assert_eq!(
                    remaining.remove(relative).as_ref(),
                    Some(original),
                    "{mode}: original changed: {}",
                    relative.display()
                );
            }
            if mode == "same-path-error" {
                // The existing project writer deliberately retains its staged
                // JSON when replacing the destination fails. Account for this
                // exact recovery artifact; every original file remains checked.
                assert_eq!(
                    remaining.len(),
                    1,
                    "unexpected recovery files: {remaining:?}"
                );
                let recovery = remaining.keys().next().unwrap();
                assert_eq!(recovery.components().count(), 1);
                assert!(
                    recovery
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("same-path-error.tmp-")
                );
                assert!(
                    project::read(&fixture.root.join(recovery)).unwrap() == candidate,
                    "recovery file differs from staged candidate"
                );
            } else {
                assert!(
                    remaining.is_empty(),
                    "{mode}: unexpected new files: {remaining:?}"
                );
            }
            assert_eq!(
                fs::read(editor.workflow.recent_path.as_ref().unwrap()).unwrap(),
                recent
            );
            assert!(editor.workflow.job.is_none());
            assert_eq!(editor.workflow.show_report, mode != "cancel");
            assert!(matches!(
                editor.workflow.picker_requests.as_slice(),
                [(DialogPurpose::SaveAs, _)]
            ));
        }
    }

    #[test]
    fn save_as_same_path_applies_json_only_after_save_and_preserves_history() {
        let (fixture, mut editor) = Fixture::new(true);
        let ctx = egui::Context::default();
        let candidate = stage_save_as_json_draft(&fixture, &mut editor);
        let state = file_command_state(&editor);
        let history = history_state(&editor);
        let source_binding = crate::distribution::project_binding_path(&fixture.path).unwrap();
        let binding = fs::read(&source_binding).unwrap();
        editor.workflow.picker_result = Some(Ok(Some(fixture.path.clone())));
        editor.dispatch_command(Command::SaveAs, &ctx);
        assert_file_command_state(&editor, &state, "same-path dispatch");
        drain_file_job(&mut editor, &ctx);
        assert!(!editor.workflow.show_report, "{}", editor.workflow.report);
        assert!(
            project::read(&fixture.path).unwrap() == candidate,
            "saved candidate differs"
        );
        assert!(editor.document == candidate, "adopted candidate differs");
        assert!(editor.workflow.saved == candidate, "saved boundary differs");
        assert!(!editor.json_dirty && !editor.dirty);
        assert!(!editor.playing && !editor.game_workbench.is_play_session());
        let after_history = history_state(&editor);
        assert!(
            &after_history[..history.len()] == history.as_slice(),
            "existing history was rewritten"
        );
        assert_eq!(
            after_history.len(),
            history.len() + 1,
            "same-path Save As must keep Undo history"
        );
        assert_eq!(fs::read(source_binding).unwrap(), binding);
        assert_eq!(
            fs::read(fixture.root.join("assets/save-as-note.txt")).unwrap(),
            b"author resource"
        );
    }

    #[test]
    fn save_as_copy_publishes_json_candidate_binding_and_resources_before_adoption() {
        let (fixture, mut editor) = Fixture::new(true);
        let ctx = egui::Context::default();
        let candidate = stage_save_as_json_draft(&fixture, &mut editor);
        let state = file_command_state(&editor);
        let source_json = module_engine_project::hash_file(&fixture.path).unwrap();
        let source_binding = crate::distribution::project_binding_path(&fixture.path).unwrap();
        let binding = fs::read(&source_binding).unwrap();
        let destination = fixture.root.join("Копия-😀").join("игра.json");
        editor.workflow.picker_result = Some(Ok(Some(destination.clone())));
        editor.dispatch_command(Command::SaveAs, &ctx);
        assert_file_command_state(&editor, &state, "copy dispatch");
        assert!(!destination.exists());
        drain_file_job(&mut editor, &ctx);
        assert!(!editor.workflow.show_report, "{}", editor.workflow.report);
        let destination = destination.canonicalize().unwrap();
        assert_eq!(Path::new(&editor.path), destination);
        assert!(
            project::read(&destination).unwrap() == candidate,
            "saved copy candidate differs"
        );
        assert!(editor.document == candidate, "adopted copy differs");
        assert!(
            editor.workflow.saved == candidate,
            "copy saved boundary differs"
        );
        assert!(!editor.json_dirty && !editor.dirty);
        assert_eq!(
            editor.project_asset_root().canonicalize().unwrap(),
            destination.parent().unwrap()
        );
        assert_eq!(
            module_engine_project::hash_file(&fixture.path).unwrap(),
            source_json
        );
        assert_eq!(fs::read(source_binding).unwrap(), binding);
        assert_eq!(
            fs::read(crate::distribution::project_binding_path(&destination).unwrap()).unwrap(),
            binding
        );
        assert_eq!(
            fs::read(
                destination
                    .parent()
                    .unwrap()
                    .join("assets/save-as-note.txt")
            )
            .unwrap(),
            b"author resource"
        );
        AssetManifest::from_value(&candidate.asset_manifest)
            .unwrap()
            .verify(destination.parent().unwrap())
            .unwrap();
        assert_eq!(editor.workflow.recent.paths[0], destination);
        let adopted = file_command_state(&editor);
        editor.workflow.picker_result = Some(Ok(None));
        editor.dispatch_command(Command::SaveAs, &ctx);
        drain_file_job(&mut editor, &ctx);
        assert_file_command_state(&editor, &adopted, "post-copy cancelled Save As");
        assert_eq!(
            editor.workflow.picker_requests.last().unwrap().1,
            destination
        );
    }

    #[test]
    fn save_as_invalid_json_rejects_before_picker_without_applying_or_saving() {
        let (fixture, mut editor) = Fixture::new(true);
        let ctx = egui::Context::default();
        stage_save_as_json_draft(&fixture, &mut editor);
        editor.json_draft = "{ incomplete JSON".into();
        let state = file_command_state(&editor);
        let files = fixture_files(&fixture.root);
        editor.workflow.picker_result = Some(Ok(None));
        editor.dispatch_command(Command::SaveAs, &ctx);
        assert_file_command_state(&editor, &state, "invalid JSON Save As");
        assert_eq!(fixture_files(&fixture.root), files);
        assert!(editor.workflow.picker_requests.is_empty());
        assert!(editor.workflow.picker_result.is_some());
        assert!(editor.workflow.job.is_none() && editor.workflow.show_report);
    }

    #[test]
    fn new_save_as_recent_repeated_dialogs_use_current_unicode_project_path() {
        let (fixture, mut editor) = Fixture::new(true);
        let ctx = egui::Context::default();
        let cwd = std::env::current_dir().unwrap();
        let original_files = fixture_files(&fixture.root);
        let new_folder = fixture.root.join("Новая-😀");
        editor.workflow.picker_result = Some(Ok(Some(new_folder.clone())));
        editor.dispatch_command(Command::NewEmpty, &ctx);
        drain_file_job(&mut editor, &ctx);
        assert!(!editor.workflow.show_report, "{}", editor.workflow.report);
        let new_path = new_folder.join("project.json").canonicalize().unwrap();
        assert_eq!(Path::new(&editor.path), new_path);
        assert_eq!(editor.workflow.recent.paths[0], new_path);
        check_workflow_dialog_folder(&new_path);

        fs::create_dir_all(new_folder.join("assets")).unwrap();
        fs::write(new_folder.join("assets/note.txt"), b"new project resource").unwrap();
        let mut manifest = AssetManifest::from_value(&editor.document.asset_manifest).unwrap();
        manifest
            .register("chain-note", "assets/note.txt", &new_folder)
            .unwrap();
        editor.document.asset_manifest = serde_json::to_value(manifest).unwrap();
        editor.changed(false);
        let copy = fixture.root.join("Копия-😀").join("копия.json");
        editor.workflow.picker_result = Some(Ok(Some(copy.clone())));
        editor.dispatch_command(Command::SaveAs, &ctx);
        drain_file_job(&mut editor, &ctx);
        assert!(!editor.workflow.show_report, "{}", editor.workflow.report);
        let copy = copy.canonicalize().unwrap();
        assert_eq!(Path::new(&editor.path), copy);
        assert_eq!(editor.workflow.recent.paths[0], copy);
        assert_eq!(editor.workflow.picker_requests.last().unwrap().1, new_path);
        assert_eq!(
            fs::read(copy.parent().unwrap().join("assets/note.txt")).unwrap(),
            b"new project resource"
        );
        check_workflow_dialog_folder(&copy);

        // Recent Projects dispatches the same guarded OpenPath action, without
        // choosing a second path through a native picker.
        assert!(editor.workflow.recent.paths.contains(&new_path));
        editor.request_action(Action::OpenPath(new_path.clone()), &ctx);
        assert!(!editor.workflow.show_report, "{}", editor.workflow.report);
        assert_eq!(Path::new(&editor.path), new_path);
        assert_eq!(editor.workflow.recent.paths[0], new_path);
        for command in [
            Command::Open,
            Command::SaveAs,
            Command::NewEmpty,
            Command::ExportSource,
        ] {
            let before = file_command_state(&editor);
            editor.workflow.picker_result = Some(Ok(None));
            editor.dispatch_command(command, &ctx);
            drain_file_job(&mut editor, &ctx);
            assert!(!editor.workflow.show_report, "{}", editor.workflow.report);
            assert_file_command_state(&editor, &before, "post-Recent cancelled picker");
            let (purpose, initial) = editor.workflow.picker_requests.last().unwrap();
            assert_eq!(initial, &new_path);
            assert!(matches!(
                (command, purpose),
                (Command::Open, DialogPurpose::Open)
                    | (Command::SaveAs, DialogPurpose::SaveAs)
                    | (Command::NewEmpty, DialogPurpose::New(_))
                    | (
                        Command::ExportSource,
                        DialogPurpose::Export(ExportKind::Source)
                    )
            ));
            check_workflow_dialog_folder(initial);
        }
        for (relative, expected) in original_files {
            assert_eq!(
                module_engine_project::hash_file(&fixture.root.join(&relative)).unwrap(),
                expected,
                "original changed: {}",
                relative.display()
            );
        }
        assert_eq!(std::env::current_dir().unwrap(), cwd);
    }

    fn check_workflow_dialog_folder(project: &Path) {
        let folder = project.parent().unwrap();
        assert!(folder.is_dir());
        #[cfg(windows)]
        crate::native_dialogs::test_configure_initial_folder(folder).unwrap();
        // This configures actual production COM dialog factories on Windows;
        // no Show/user input or interactive native acceptance is claimed.
    }

    fn stage_save_as_json_draft(fixture: &Fixture, editor: &mut Editor) -> GraphDocument {
        fs::create_dir_all(fixture.root.join("assets")).unwrap();
        fs::write(
            fixture.root.join("assets/save-as-note.txt"),
            b"author resource",
        )
        .unwrap();
        let mut manifest = AssetManifest::from_value(&editor.document.asset_manifest).unwrap();
        manifest
            .register("save-as-note", "assets/save-as-note.txt", &fixture.root)
            .unwrap();
        editor.document.asset_manifest = serde_json::to_value(manifest).unwrap();
        edit_name(editor);
        editor.flush_history();
        let mut candidate = editor.document.clone();
        candidate.name = "Pending JSON candidate".into();
        let game = candidate.game.as_mut().unwrap();
        game.scenes.get_mut(&game.entry_scene.clone()).unwrap().name =
            "Changed only in JSON draft".into();
        editor
            .validate_candidate(&candidate, editor.can_author_engine())
            .unwrap();
        editor.json_draft = serde_json::to_string_pretty(&candidate).unwrap();
        editor.json_dirty = true;
        editor.playing = true;
        // Exercise the public Play lifecycle state without constructing a Host,
        // renderer, compiled Custom or subprocess for this file-command test.
        editor.game_workbench.tick(0., true).unwrap();
        assert!(editor.game_workbench.is_play_session());
        candidate
    }

    fn history_state(editor: &Editor) -> Vec<(u64, String, GraphDocument)> {
        editor
            .history
            .entries()
            .iter()
            .map(|entry| (entry.id, entry.label.clone(), entry.document.clone()))
            .collect()
    }
    fn file_command_state(editor: &Editor) -> serde_json::Value {
        serde_json::json!({
            "document":editor.document,"path":editor.path,"json_draft":editor.json_draft,
            "json_dirty":editor.json_dirty,"dirty":editor.dirty,"history":history_state(editor),
            "history_cursor":editor.history.cursor(),"history_dirty":editor.history_dirty,
            "saved":editor.workflow.saved,"policy":editor.policy_baseline,
            "playing":editor.playing,"play_session":editor.game_workbench.is_play_session(),
            "recent":editor.workflow.recent,"time":editor.time,
        })
    }
    fn assert_file_command_state(editor: &Editor, expected: &serde_json::Value, stage: &str) {
        let actual = file_command_state(editor);
        for (field, value) in expected.as_object().unwrap() {
            assert!(actual.get(field) == Some(value), "{stage}: changed {field}");
        }
    }
    fn fixture_files(root: &Path) -> BTreeMap<PathBuf, String> {
        fn visit(root: &Path, folder: &Path, result: &mut BTreeMap<PathBuf, String>) {
            for entry in fs::read_dir(folder).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(root, &path, result);
                } else {
                    result.insert(
                        path.strip_prefix(root).unwrap().into(),
                        module_engine_project::hash_file(&path).unwrap(),
                    );
                }
            }
        }
        let mut result = BTreeMap::new();
        visit(root, root, &mut result);
        result
    }
    fn drain_file_job(editor: &mut Editor, ctx: &egui::Context) {
        let started = Instant::now();
        while editor.workflow.job.is_some() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(15),
                "file command worker timed out"
            );
            ctx.run_ui(egui::RawInput::default(), |ui| {
                editor.workflow_frame(ui.ctx())
            })
            .drop_without_applying_deltas();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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
        let export = text_point(&output, "Export project…");
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
