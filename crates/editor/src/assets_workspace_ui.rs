//! Cached asset service adapter and asynchronous, renderer-independent thumbnails.
//! Files are scanned at explicit synchronization boundaries, never every UI frame.
use crate::assets_ui::{AssetKind, AssetTile, BrowserAction, BrowserState, TrashTile};
use eframe::egui;
use module_engine_content::ContentLibrary;
use module_engine_core::{
    Camera, GraphDocument, RenderItem, SceneSnapshot, multiply_matrices, render_scene,
    transform_point,
};
use module_engine_project::{AssetManifest, AssetWorkspace, asset_reference, resolve_relative};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc,
};

const THUMBNAIL_EDGE: usize = 192;
const MAX_CACHED_THUMBNAILS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CatalogContext {
    backend_root: Option<PathBuf>,
    property_object_ids: Option<BTreeSet<u64>>,
}
impl CatalogContext {
    fn selected(
        backend_root: &Path,
        property_object_ids: Option<BTreeSet<u64>>,
    ) -> Result<Self, String> {
        Ok(Self {
            backend_root: Some(
                backend_root
                    .canonicalize()
                    .map_err(|error| error.to_string())?,
            ),
            property_object_ids,
        })
    }
    #[cfg(test)]
    fn source_only() -> Self {
        Self {
            backend_root: None,
            property_object_ids: None,
        }
    }
    fn service(&self, root: &Path, protected: Vec<String>) -> Result<AssetWorkspace, String> {
        match &self.backend_root {
            Some(backend) => crate::host::asset_workspace_at(
                root,
                protected,
                backend,
                self.property_object_ids.clone(),
            ),
            None => AssetWorkspace::new(root, protected),
        }
    }
}

struct CatalogScanJob {
    root: PathBuf,
    protected: Vec<String>,
    context: CatalogContext,
    document: GraphDocument,
    results: mpsc::Receiver<Result<CatalogScan, String>>,
}
struct CatalogScan {
    manifest: serde_json::Value,
    tiles: Vec<AssetTile>,
    folders: Vec<String>,
    trash: Vec<TrashTile>,
    service: AssetWorkspace,
}

#[derive(Default)]
pub struct AssetsWorkbench {
    browsers: BTreeMap<(String, u64), BrowserState>,
    root: Option<PathBuf>,
    protected: Vec<String>,
    context: Option<CatalogContext>,
    service: Option<AssetWorkspace>,
    pub tiles: Vec<AssetTile>,
    pub folders: Vec<String>,
    pub trash: Vec<TrashTile>,
    textures: BTreeMap<String, egui::TextureHandle>,
    texture_order: VecDeque<String>,
    pending_previews: BTreeSet<String>,
    worker: Option<PreviewWorker>,
    scan: Option<CatalogScanJob>,
    initialized: bool,
    scan_needed: bool,
    usages_dirty: bool,
    pub error: Option<String>,
}
impl AssetsWorkbench {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// Same-project import/file transactions preserve pane navigation and selection.
    pub fn invalidate(&mut self) {
        self.scan_needed = true;
        self.scan = None;
        self.worker = None;
        self.pending_previews.clear();
        self.textures.clear();
        self.texture_order.clear();
        for tile in &mut self.tiles {
            tile.preview = None;
            tile.preview_status = None;
        }
        for browser in self.browsers.values_mut() {
            for tile in &self.tiles {
                browser.invalidate_preview(&tile.id);
            }
        }
    }
    /// Metadata-only refresh after a completed authored change; no filesystem scan.
    pub fn invalidate_usages(&mut self) {
        self.usages_dirty = true;
    }
    #[cfg(test)]
    pub fn workspace(&self) -> Option<&AssetWorkspace> {
        self.service.as_ref()
    }
    #[cfg(test)]
    pub fn browser(&self, workspace: &str, pane_id: u64) -> Option<&BrowserState> {
        self.browsers.get(&(workspace.into(), pane_id))
    }
    #[cfg(test)]
    pub fn needs_synchronization(&self, root: &Path, protected: &[String]) -> bool {
        self.needs_synchronization_with_context(root, protected, &CatalogContext::source_only())
    }
    fn needs_synchronization_with_context(
        &self,
        root: &Path,
        protected: &[String],
        context: &CatalogContext,
    ) -> bool {
        self.scan.is_some()
            || self.scan_needed
            || !self.initialized
            || self.root.as_deref() != Some(root)
            || self.protected != protected
            || self.context.as_ref() != Some(context)
    }
    /// Returns a candidate only when indexing changed the manifest. Caller owns
    /// policy validation, dirty state and the document-history boundary.
    #[cfg(test)]
    pub fn synchronize_cache(
        &mut self,
        document: &GraphDocument,
        root: &Path,
        protected: Vec<String>,
    ) -> Result<Option<GraphDocument>, String> {
        self.synchronize_cache_with_context(
            document,
            root,
            protected,
            CatalogContext::source_only(),
        )
    }
    fn synchronize_cache_with_context(
        &mut self,
        document: &GraphDocument,
        root: &Path,
        protected: Vec<String>,
        context: CatalogContext,
    ) -> Result<Option<GraphDocument>, String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("Папка ресурсов: {error}"))?;
        let service = context.service(&root, protected.clone())?;
        self.synchronize_cache_with_service(document, &root, protected, context, service)
    }
    fn synchronize_cache_with_service(
        &mut self,
        document: &GraphDocument,
        root: &Path,
        protected: Vec<String>,
        context: CatalogContext,
        service: AssetWorkspace,
    ) -> Result<Option<GraphDocument>, String> {
        let mut candidate = document.clone();
        let infos = service.synchronize(&mut candidate)?;
        let mut folders = vec![];
        collect_folders(
            root,
            "assets",
            &protected,
            &mut BTreeSet::new(),
            &mut folders,
        )?;
        folders.sort();
        let mut tiles = vec![];
        let mut all_usages = service.usages_batch(&candidate)?;
        for info in infos {
            let kind = ui_kind(&info.kind);
            let mut metadata = vec![];
            if info.missing {
                metadata.push(("Состояние".into(), "Файл отсутствует".into()));
            }
            if let Some(diagnostic) = &info.diagnostic {
                metadata.push(("Проверка".into(), diagnostic.clone()));
            }
            if !info.dependencies.is_empty() {
                metadata.push(("Зависимостей".into(), info.dependencies.len().to_string()));
            }
            if kind == AssetKind::Audio
                && !info.missing
                && let Ok(path) = resolve_relative(root, &info.path)
            {
                match wav_metadata(&path) {
                    Ok(audio) => metadata.extend(audio),
                    Err(error) => metadata.push(("Проверка звука".into(), error)),
                }
            }
            let usages = all_usages
                .remove(&info.id)
                .unwrap_or_default()
                .into_iter()
                .map(|usage| usage.location)
                .collect();
            tiles.push(AssetTile {
                reference: asset_reference(&info.id),
                id: info.id,
                path: info.path.clone(),
                kind,
                size: info.bytes,
                protected: !info.path.starts_with("assets/"),
                preview: None,
                preview_status: if info.missing {
                    Some("Файл отсутствует".into())
                } else {
                    info.diagnostic
                },
                usages,
                metadata,
            });
        }
        tiles.sort_by_key(|tile| tile.path.to_lowercase());
        let trash = service
            .list_deleted()?
            .into_iter()
            .filter_map(|record| {
                record.deleted_asset.map(|(_, entry)| TrashTile {
                    id: record.id,
                    kind: ui_kind(&module_engine_project::AssetKind::for_path(&entry.path)),
                    path: entry.path,
                    // Durable operation IDs contain a sortable UTC timestamp.
                    deleted_at: "Можно восстановить".into(),
                })
            })
            .collect();
        if self
            .root
            .as_deref()
            .is_some_and(|previous| previous != root)
        {
            self.browsers.clear();
        }
        self.invalidate();
        self.tiles = tiles;
        self.folders = folders;
        self.trash = trash;
        self.root = Some(root.to_path_buf());
        self.protected = protected;
        self.context = Some(context);
        self.service = Some(service);
        self.worker = None;
        self.scan_needed = false;
        self.initialized = true;
        self.usages_dirty = false;
        self.error = None;
        Ok((candidate != *document).then_some(candidate))
    }
    fn clear_stale_usages(&mut self, context: &CatalogContext) {
        if self.context.as_ref() != Some(context) {
            for tile in &mut self.tiles {
                tile.usages.clear();
            }
            self.usages_dirty = true;
        }
    }
    fn refresh_usages(
        &mut self,
        document: &GraphDocument,
        context: &CatalogContext,
    ) -> Result<(), String> {
        self.clear_stale_usages(context);
        if self.context.as_ref() != Some(context) {
            return Ok(());
        }
        if let Some(service) = &self.service {
            let mut all_usages = service.usages_batch(document)?;
            for tile in &mut self.tiles {
                tile.usages = all_usages
                    .remove(&tile.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|usage| usage.location)
                    .collect();
            }
        }
        self.usages_dirty = false;
        Ok(())
    }

    /// Hashing/scanning runs outside the native UI. Only metadata from the
    /// matching project/index request is adopted; concurrent scene edits stay.
    #[cfg(test)]
    fn synchronize_in_background(
        &mut self,
        document: &GraphDocument,
        root: &Path,
        protected: Vec<String>,
        ctx: &egui::Context,
    ) -> Result<Option<GraphDocument>, String> {
        self.synchronize_in_background_with_context(
            document,
            root,
            protected,
            CatalogContext::source_only(),
            ctx,
        )
    }
    fn synchronize_in_background_with_context(
        &mut self,
        document: &GraphDocument,
        root: &Path,
        protected: Vec<String>,
        context: CatalogContext,
        ctx: &egui::Context,
    ) -> Result<Option<GraphDocument>, String> {
        self.clear_stale_usages(&context);
        if self.scan.as_ref().is_some_and(|job| {
            job.root != root || job.protected != protected || job.context != context
        }) {
            self.scan = None;
        }
        if let Some(job) = &self.scan {
            let result = match job.results.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return Ok(None),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scan = None;
                    return Err("Поток индексирования ресурсов завершился без результата".into());
                }
            };
            let job = self.scan.take().unwrap();
            if document.asset_manifest != job.document.asset_manifest {
                self.scan_needed = true;
                return Ok(None);
            }
            let mut catalog = result?;
            let mut candidate = document.clone();
            candidate.asset_manifest = catalog.manifest;
            if document != &job.document {
                let mut usages = catalog.service.usages_batch(&candidate)?;
                for tile in &mut catalog.tiles {
                    tile.usages = usages
                        .remove(&tile.id)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|usage| usage.location)
                        .collect();
                }
            }
            if self
                .root
                .as_deref()
                .is_some_and(|previous| previous != root)
            {
                self.browsers.clear();
            }
            self.invalidate();
            self.tiles = catalog.tiles;
            self.folders = catalog.folders;
            self.trash = catalog.trash;
            self.root = Some(job.root);
            self.protected = job.protected;
            self.context = Some(job.context);
            self.service = Some(catalog.service);
            self.scan_needed = false;
            self.initialized = true;
            self.usages_dirty = false;
            self.error = None;
            return Ok((candidate != *document).then_some(candidate));
        }
        let root = root.to_path_buf();
        let worker_root = root.clone();
        let worker_protected = protected.clone();
        let worker_document = document.clone();
        let worker_context = context.clone();
        let ctx = ctx.clone();
        let (sender, results) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                let mut cache = AssetsWorkbench::default();
                let candidate = cache
                    .synchronize_cache_with_context(
                        &worker_document,
                        &worker_root,
                        worker_protected,
                        worker_context,
                    )?
                    .unwrap_or(worker_document);
                Ok(CatalogScan {
                    manifest: candidate.asset_manifest,
                    tiles: cache.tiles,
                    folders: cache.folders,
                    trash: cache.trash,
                    service: cache.service.ok_or("Asset catalog has no service")?,
                })
            })();
            let _ = sender.send(result);
            ctx.request_repaint();
        });
        self.scan = Some(CatalogScanJob {
            root,
            protected,
            context,
            document: document.clone(),
            results,
        });
        Ok(None)
    }
    fn queue_preview(&mut self, id: &str, document: &GraphDocument) -> Result<(), String> {
        let Some(tile) = self.tiles.iter_mut().find(|tile| tile.id == id) else {
            return Ok(());
        };
        if !matches!(tile.kind, AssetKind::Model | AssetKind::Texture)
            || self.textures.contains_key(id)
            || self.pending_previews.contains(id)
        {
            return Ok(());
        }
        let root = self.root.as_ref().ok_or("Папка ресурсов не выбрана")?;
        if self.worker.is_none() {
            let paths = AssetManifest::from_value(&document.asset_manifest)?.asset_paths();
            self.worker = Some(PreviewWorker::new(root.clone(), paths));
        }
        let worker = self.worker.as_ref().unwrap();
        worker
            .jobs
            .send(PreviewJob {
                id: id.into(),
                reference: tile.reference.clone(),
                kind: tile.kind,
            })
            .map_err(|_| "Загрузка превью остановлена".to_string())?;
        tile.preview_status = Some("Превью загружается…".into());
        self.pending_previews.insert(id.into());
        Ok(())
    }
    fn poll_previews(&mut self, ctx: &egui::Context) {
        let results: Vec<_> = self
            .worker
            .as_ref()
            .map(|worker| worker.results.try_iter().collect())
            .unwrap_or_default();
        for result in results {
            self.pending_previews.remove(&result.id);
            let Some(tile) = self.tiles.iter_mut().find(|tile| tile.id == result.id) else {
                continue;
            };
            match result.image {
                Ok(image) => {
                    let texture = ctx.load_texture(
                        format!("asset-thumbnail:{}", result.id),
                        egui::ColorImage::from_rgba_unmultiplied(
                            [image.width, image.height],
                            &image.rgba,
                        ),
                        egui::TextureOptions::LINEAR,
                    );
                    tile.preview = Some(texture.id());
                    tile.preview_status = image.note;
                    for (name, value) in image.metadata {
                        tile.metadata.retain(|(existing, _)| existing != &name);
                        tile.metadata.push((name, value));
                    }
                    self.textures.insert(result.id.clone(), texture);
                    self.texture_order.retain(|id| id != &result.id);
                    self.texture_order.push_back(result.id);
                }
                Err(error) => {
                    tile.preview_status = Some(format!("Превью недоступно: {error}"));
                }
            }
        }
        while self.textures.len() > MAX_CACHED_THUMBNAILS {
            let Some(id) = self.texture_order.pop_front() else {
                break;
            };
            self.textures.remove(&id);
            if let Some(tile) = self.tiles.iter_mut().find(|tile| tile.id == id) {
                tile.preview = None;
                tile.preview_status = None;
            }
            for browser in self.browsers.values_mut() {
                browser.invalidate_preview(&id);
            }
        }
        if !self.pending_previews.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(35));
        }
    }
}

impl super::Editor {
    /// First-time indexing is metadata migration, not an undoable scene gesture.
    /// The saved baseline remains untouched so newly assigned IDs still need Save.
    pub(super) fn accept_asset_index(&mut self, candidate: GraphDocument) {
        if candidate == self.document {
            return;
        }
        let before = self.document.asset_manifest.clone();
        let mut without_index = candidate.clone();
        without_index.asset_manifest = before.clone();
        let first_index = without_index == self.document
            && !self.workflow.has_asset_history()
            && AssetManifest::from_value(&before)
                .and_then(|previous| {
                    AssetManifest::from_value(&candidate.asset_manifest).map(|indexed| {
                        previous
                            .assets
                            .iter()
                            .all(|(id, entry)| indexed.assets.get(id) == Some(entry))
                    })
                })
                .unwrap_or(false);
        let history_dirty = self.history_dirty;
        let edit_time = self.history_edit_time;
        if first_index {
            if let Some(gesture) = &mut self.scene_gesture_before
                && gesture.asset_manifest == before
            {
                gesture.asset_manifest = candidate.asset_manifest.clone();
            }
            self.history
                .rebase_asset_manifest(&before, &candidate.asset_manifest);
            self.workflow
                .rebase_asset_history(&before, &candidate.asset_manifest);
        }
        self.document = candidate;
        self.changed(false);
        if first_index {
            self.history_dirty = history_dirty;
            self.history_edit_time = edit_time;
        }
    }
    /// Checkpoint/merge restoration changes authored data, not historical files.
    /// Reject stale resource maps instead of installing references to moved files.
    /// The same check also accepts file Undo after its explicit filesystem replay.
    pub(super) fn validate_asset_restore(&self, candidate: &GraphDocument) -> Result<(), String> {
        verify_restore_resources(candidate, &self.project_asset_root()).map_err(|error| {
            format!(
                "Версия не восстановлена: {error}. Сначала отмени файловую операцию через Undo или восстанови нужный ресурс из корзины."
            )
        })
    }
    /// Installed engine shader files are not project-editable resources.
    pub(super) fn asset_protected_paths(&self) -> Vec<String> {
        let root = self.project_asset_root();
        let backend = self
            .distribution
            .as_ref()
            .map(|distribution| distribution.package_root.clone())
            .unwrap_or_else(crate::project::source_root);
        if let (Ok(root), Ok(backend)) = (root.canonicalize(), backend.canonicalize())
            && root == backend
        {
            vec!["assets/gpu".into()]
        } else {
            vec![]
        }
    }
    pub(super) fn assets_panel(&mut self, ui: &mut egui::Ui, pane_id: u64) {
        let root = self.project_asset_root();
        let protected = self.asset_protected_paths();
        let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        let backend_root = self
            .distribution
            .as_ref()
            .map(|distribution| distribution.package_root.clone())
            .unwrap_or_else(crate::project::source_root);
        let context =
            match CatalogContext::selected(&backend_root, self.asset_property_object_ids()) {
                Ok(context) => context,
                Err(error) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, &error);
                    self.assets.error = Some(error);
                    return;
                }
            };
        self.assets.clear_stale_usages(&context);
        let busy = self.playing
            || self.game_workbench.is_play_session()
            || self.workflow.blocks_document_edits()
            || self.json_dirty;
        if !busy
            && self
                .assets
                .needs_synchronization_with_context(&canonical, &protected, &context)
        {
            match self.assets.synchronize_in_background_with_context(
                &self.document,
                &canonical,
                protected.clone(),
                context.clone(),
                ui.ctx(),
            ) {
                Ok(Some(candidate)) => {
                    self.accept_asset_index(candidate);
                }
                Ok(None) => {}
                Err(error) => {
                    self.assets.invalidate();
                    self.assets.tiles.clear();
                    self.assets.folders.clear();
                    self.assets.trash.clear();
                    self.assets.service = None;
                    self.assets.root = Some(canonical.clone());
                    self.assets.protected = protected;
                    self.assets.context = Some(context.clone());
                    self.assets.initialized = true;
                    self.assets.scan_needed = false;
                    self.assets.error = Some(error);
                }
            }
        }
        if self.assets.usages_dirty
            && let Err(error) = self.assets.refresh_usages(&self.document, &context)
        {
            self.assets.error = Some(error);
        }
        self.assets.poll_previews(ui.ctx());
        if self.assets.scan.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Индексирование ресурсов… Редактор остаётся доступен.");
            });
        }
        if let Some(error) = &self.assets.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
            if ui.button("Повторить загрузку ресурсов").clicked() {
                self.assets.invalidate();
                ui.ctx().request_repaint();
            }
            ui.separator();
        }
        if self.playing || self.game_workbench.is_play_session() {
            ui.weak("Для изменения ресурсов останови Play.");
        } else if self.json_dirty {
            ui.weak("Примени или отмени изменения Project File перед работой с ресурсами.");
        }
        let key = (self.workspace.key().into(), pane_id);
        let actions = self.assets.browsers.entry(key).or_default().ui(
            ui,
            &self.assets.tiles,
            &self.assets.folders,
            &self.assets.trash,
            busy || self.assets.scan.is_some(),
        );
        for action in actions {
            match action {
                BrowserAction::Select { .. } => {}
                BrowserAction::Preview { id } => {
                    if let Err(error) = self.assets.queue_preview(&id, &self.document) {
                        self.assets.error = Some(error);
                    }
                }
                action => self.request_asset_action(action, ui.ctx()),
            }
        }
    }
}

fn verify_restore_resources(candidate: &GraphDocument, root: &Path) -> Result<(), String> {
    let manifest = AssetManifest::from_value(&candidate.asset_manifest)?;
    manifest.verify(root)?;
    fn walk(
        value: &serde_json::Value,
        field: &str,
        manifest: &AssetManifest,
        root: &Path,
    ) -> Result<(), String> {
        match value {
            serde_json::Value::String(reference)
                if reference.starts_with("asset://")
                    || reference.starts_with("assets/")
                    || reference.starts_with("assets\\")
                    || matches!(field, "path" | "base_color_texture" | "audio_cues") =>
            {
                let path = manifest.resolve_reference(reference)?;
                resolve_relative(root, &path)?;
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    walk(value, field, manifest, root)?;
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    if !matches!(
                        key.as_str(),
                        "name" | "label" | "id" | "source_id" | "source" | "code"
                    ) {
                        walk(
                            value,
                            if field == "audio_cues" { field } else { key },
                            manifest,
                            root,
                        )?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    // Code strings and installed renderer include paths are not game resources.
    // Legacy manifests may be empty, so active scene references need checking too.
    if let Some(game) = &candidate.game {
        walk(
            &serde_json::to_value(game).map_err(|error| error.to_string())?,
            "game",
            &manifest,
            root,
        )?;
    }
    walk(
        &serde_json::to_value(&candidate.properties).map_err(|error| error.to_string())?,
        "properties",
        &manifest,
        root,
    )
}

fn ui_kind(kind: &module_engine_project::AssetKind) -> AssetKind {
    match kind {
        module_engine_project::AssetKind::Model => AssetKind::Model,
        module_engine_project::AssetKind::Texture => AssetKind::Texture,
        module_engine_project::AssetKind::Audio => AssetKind::Audio,
        module_engine_project::AssetKind::Shader => AssetKind::Shader,
        module_engine_project::AssetKind::Other => AssetKind::Other,
    }
}
fn collect_folders(
    root: &Path,
    relative: &str,
    protected: &[String],
    visited: &mut BTreeSet<PathBuf>,
    result: &mut Vec<String>,
) -> Result<(), String> {
    if protected.iter().any(|path| {
        relative.eq_ignore_ascii_case(path)
            || relative
                .to_ascii_lowercase()
                .starts_with(&(path.to_ascii_lowercase() + "/"))
    }) {
        return Ok(());
    }
    let path = root.join(relative);
    if !path.exists() {
        return Ok(());
    }
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if !path.starts_with(root) || !visited.insert(path.clone()) {
        return Err(format!(
            "Папка ресурсов выходит за проект или повторяется: {relative}"
        ));
    }
    result.push(relative.into());
    for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".module-engine-game-export-")
            {
                continue;
            }
            collect_folders(
                root,
                &format!("{relative}/{}", entry.file_name().to_string_lossy()),
                protected,
                visited,
                result,
            )?;
        }
    }
    Ok(())
}

struct PreviewWorker {
    jobs: mpsc::Sender<PreviewJob>,
    results: mpsc::Receiver<PreviewResult>,
}
impl PreviewWorker {
    fn new(root: PathBuf, paths: BTreeMap<String, String>) -> Self {
        let (jobs, incoming) = mpsc::channel::<PreviewJob>();
        let (outgoing, results) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(job) = incoming.recv() {
                // A lightweight content reader has no runtime/device and owns
                // no unbounded decoded-model cache across thousands of assets.
                let image = ContentLibrary::new(&root).and_then(|mut content| {
                    content.set_asset_paths(paths.clone());
                    make_thumbnail(&mut content, &job.reference, job.kind)
                });
                if outgoing.send(PreviewResult { id: job.id, image }).is_err() {
                    break;
                }
            }
        });
        Self { jobs, results }
    }
}
struct PreviewJob {
    id: String,
    reference: String,
    kind: AssetKind,
}
struct PreviewResult {
    id: String,
    image: Result<Thumbnail, String>,
}
struct Thumbnail {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    metadata: Vec<(String, String)>,
    note: Option<String>,
}
fn make_thumbnail(
    content: &mut ContentLibrary,
    reference: &str,
    kind: AssetKind,
) -> Result<Thumbnail, String> {
    if kind == AssetKind::Texture {
        let image = content.load_texture(reference)?;
        let (width, height, rgba) = resized_rgba(
            image.width as usize,
            image.height as usize,
            &image.rgba8,
            THUMBNAIL_EDGE,
        )?;
        return Ok(Thumbnail {
            width,
            height,
            rgba,
            metadata: vec![(
                "Размер изображения".into(),
                format!("{} × {}", image.width, image.height),
            )],
            note: None,
        });
    }
    if kind != AssetKind::Model {
        return Err("Для этого типа файла превью не предусмотрено".into());
    }
    let model = content.load_model(reference)?;
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut vertices = 0;
    let mut triangles = 0;
    for primitive in &model.primitives {
        vertices += primitive.mesh.vertices.len();
        triangles += primitive.mesh.triangles.len();
        for vertex in &primitive.mesh.vertices {
            let vertex = transform_point(primitive.transform, *vertex);
            if !vertex.into_iter().all(f32::is_finite) {
                continue;
            }
            for axis in 0..3 {
                min[axis] = min[axis].min(vertex[axis]);
                max[axis] = max[axis].max(vertex[axis]);
            }
        }
    }
    if vertices == 0 || triangles == 0 || !min.into_iter().chain(max).all(f32::is_finite) {
        return Err("В модели нет треугольной геометрии".into());
    }
    let center = std::array::from_fn::<_, 3, _>(|axis| min[axis] * 0.5 + max[axis] * 0.5);
    let extent = (0..3)
        .map(|axis| (max[axis] - min[axis]).abs())
        .fold(0.0, f32::max);
    let scale = 2.0 / extent.max(0.0001);
    let normalize = [
        [scale, 0., 0., -center[0] * scale],
        [0., scale, 0., -center[1] * scale],
        [0., 0., scale, -center[2] * scale],
        [0., 0., 0., 1.],
    ];
    let scene = SceneSnapshot {
        time: 0.0,
        items: model
            .primitives
            .iter()
            .enumerate()
            .map(|(index, primitive)| RenderItem {
                id: index as u64 + 1,
                mesh: primitive.mesh.clone(),
                model: multiply_matrices(normalize, primitive.transform),
                color: [
                    base_color_byte(primitive.material.base_color[0]),
                    base_color_byte(primitive.material.base_color[1]),
                    base_color_byte(primitive.material.base_color[2]),
                    255,
                ],
                material: primitive.material.clone(),
            })
            .collect(),
    };
    let frame = render_scene(
        &scene,
        THUMBNAIL_EDGE,
        THUMBNAIL_EDGE,
        Camera {
            yaw: 0.70,
            pitch: 0.35,
            distance: 4.0,
            target: [0.; 3],
        },
    );
    if frame.stats.shaded_pixels == 0 {
        return Err("Геометрия не образует видимой поверхности".into());
    }
    Ok(Thumbnail {
        width: frame.width,
        height: frame.height,
        rgba: frame.rgba,
        metadata: vec![
            ("Частей".into(), model.primitives.len().to_string()),
            ("Вершин".into(), vertices.to_string()),
            ("Треугольников".into(), triangles.to_string()),
        ],
        note: Some("Превью геометрии с базовым цветом материала".into()),
    })
}
fn base_color_byte(linear: f32) -> u8 {
    let linear = if linear.is_finite() {
        linear.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let encoded = if linear <= 0.0031308 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}
fn resized_rgba(
    width: usize,
    height: usize,
    rgba: &[u8],
    max_edge: usize,
) -> Result<(usize, usize, Vec<u8>), String> {
    if width == 0
        || height == 0
        || width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            != Some(rgba.len())
    {
        return Err("Некорректный размер изображения".into());
    }
    let scale = (max_edge as f32 / width.max(height) as f32).min(1.0);
    let target_width = ((width as f32 * scale).round() as usize).max(1);
    let target_height = ((height as f32 * scale).round() as usize).max(1);
    let mut result = vec![0; target_width * target_height * 4];
    for y in 0..target_height {
        for x in 0..target_width {
            let source_x = ((x as f32 + 0.5) / scale).floor() as usize;
            let source_y = ((y as f32 + 0.5) / scale).floor() as usize;
            let source = (source_y.min(height - 1) * width + source_x.min(width - 1)) * 4;
            let target = (y * target_width + x) * 4;
            result[target..target + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    Ok((target_width, target_height, result))
}
fn wav_metadata(path: &Path) -> Result<Vec<(String, String)>, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = vec![];
    file.by_ref()
        .take(65_536)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Ожидается WAV с заголовком RIFF/WAVE".into());
    }
    let mut offset = 12;
    let mut format = None;
    let mut data_bytes = None;
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        if &bytes[offset..offset + 4] == b"fmt " && size >= 16 && body + 16 <= bytes.len() {
            format = Some((
                u16::from_le_bytes(bytes[body + 2..body + 4].try_into().unwrap()),
                u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap()),
                u32::from_le_bytes(bytes[body + 8..body + 12].try_into().unwrap()),
                u16::from_le_bytes(bytes[body + 14..body + 16].try_into().unwrap()),
            ));
        } else if &bytes[offset..offset + 4] == b"data" {
            data_bytes = Some(size);
            break;
        }
        let Some(next) = body
            .checked_add(size)
            .and_then(|value| value.checked_add(size % 2))
        else {
            break;
        };
        offset = next;
    }
    let (channels, rate, bytes_per_second, bits) = format.ok_or("Не найден блок WAV fmt")?;
    let mut result = vec![
        ("Каналов".into(), channels.to_string()),
        ("Частота".into(), format!("{rate} Гц")),
        ("Разрядность".into(), format!("{bits} бит")),
    ];
    if let Some(size) = data_bytes
        && bytes_per_second > 0
    {
        result.push((
            "Длительность".into(),
            format!("{:.2} с", size as f64 / bytes_per_second as f64),
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        base: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.work/test-temp");
            fs::create_dir_all(&base).unwrap();
            let base = base.canonicalize().unwrap();
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = base.join(format!("asset-browser-{stamp}"));
            fs::create_dir_all(root.join("assets")).unwrap();
            Self { root, base }
        }
        fn png(&self, name: &str, color: [u8; 4]) {
            let mut bytes = vec![];
            {
                let mut encoder = png::Encoder::new(&mut bytes, 4, 2);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&color.repeat(8))
                    .unwrap();
            }
            fs::write(self.root.join("assets").join(name), bytes).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Ok(root) = self.root.canonicalize() {
                assert!(root.starts_with(&self.base) && root != self.base);
                let _ = fs::remove_dir_all(root);
            }
        }
    }
    #[test]
    fn catalog_keeps_selected_property_contracts_and_refreshes_default_usages() {
        use module_engine_core::{
            CompositionContext, ModuleDescriptor, ModuleRegistry, PropertySchema, PropertyScope,
            PropertyType, SystemRegistry, WorldKind,
        };
        let fixture = Fixture::new();
        fixture.png("logo.png", [10, 20, 30, 255]);
        let mut document = module_engine_core::demo_document(WorldKind::Objects);
        let mut modules = ModuleRegistry::builtin();
        modules
            .register_module(ModuleDescriptor {
                id: "audit.catalog".into(),
                label: "Catalog contract".into(),
                description: String::new(),
                inputs: vec![],
                outputs: vec![],
                properties: vec![PropertySchema {
                    id: "audit.catalog.image".into(),
                    label: "Logo".into(),
                    scope: PropertyScope::Global,
                    value_type: PropertyType::AssetReference {
                        kinds: vec!["png".into()],
                        optional: true,
                    },
                    default: serde_json::json!("assets/logo.png"),
                    requires_modules: vec![],
                    requires_components: vec![],
                }],
            })
            .unwrap();
        modules.set_module_version("audit.catalog", "1").unwrap();
        document
            .engine
            .enabled_modules
            .insert("audit.catalog".into());
        document
            .module_versions
            .insert("audit.catalog".into(), "1".into());
        let composition = CompositionContext::new(modules, SystemRegistry::builtin()).unwrap();
        let source_only = AssetWorkspace::new(&fixture.root, Vec::<String>::new()).unwrap();
        source_only.synchronize(&mut document).unwrap();
        assert!(
            source_only
                .usages_batch(&document)
                .unwrap()
                .values()
                .all(Vec::is_empty)
        );
        let service = AssetWorkspace::new(&fixture.root, Vec::<String>::new())
            .unwrap()
            .with_composition(composition);
        let context =
            CatalogContext::selected(&crate::project::source_root(), Some(BTreeSet::new()))
                .unwrap();
        let mut cache = AssetsWorkbench::default();
        cache
            .synchronize_cache_with_service(
                &document,
                &fixture.root.canonicalize().unwrap(),
                vec![],
                context.clone(),
                service,
            )
            .unwrap();
        assert_eq!(cache.context.as_ref(), Some(&context));
        assert_eq!(cache.tiles.len(), 1);
        assert_eq!(cache.tiles[0].usages.len(), 1);
        assert!(cache.tiles[0].usages[0].contains("audit.catalog.image"));
        document
            .properties
            .active
            .global
            .insert("audit.catalog.image".into(), serde_json::Value::Null);
        cache.invalidate_usages();
        cache.refresh_usages(&document, &context).unwrap();
        assert!(
            cache.tiles[0].usages.is_empty(),
            "explicit optional null suppresses the installed default"
        );
        assert!(!cache.usages_dirty);
    }

    #[test]
    fn background_catalog_rejects_ready_results_from_other_backend_or_property_objects() {
        let fixture = Fixture::new();
        fixture.png("logo.png", [10, 20, 30, 255]);
        let initial = module_engine_core::demo_document(module_engine_core::WorldKind::Objects);
        let first = CatalogContext {
            backend_root: None,
            property_object_ids: Some(BTreeSet::from([1])),
        };
        let mut cache = AssetsWorkbench::default();
        let document = cache
            .synchronize_cache_with_context(&initial, &fixture.root, vec![], first.clone())
            .unwrap()
            .unwrap();
        let ctx = egui::Context::default();
        cache
            .synchronize_in_background_with_context(
                &document,
                &fixture.root,
                vec![],
                first.clone(),
                &ctx,
            )
            .unwrap();
        for next in [
            CatalogContext {
                backend_root: None,
                property_object_ids: Some(BTreeSet::from([2])),
            },
            CatalogContext {
                backend_root: None,
                property_object_ids: None,
            },
            CatalogContext::selected(&crate::project::source_root(), None).unwrap(),
        ] {
            // Queue an already completed old result: a wrong adoption would be
            // immediate and would publish its stale context rather than submit.
            let prepared = cache
                .scan
                .as_ref()
                .unwrap()
                .results
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let (sender, results) = mpsc::channel();
            sender.send(prepared).unwrap();
            cache.scan.as_mut().unwrap().results = results;
            cache.tiles[0].usages = vec!["old-context-only".into()];
            assert!(cache.needs_synchronization_with_context(&fixture.root, &[], &next));
            assert!(
                cache
                    .synchronize_in_background_with_context(
                        &document,
                        &fixture.root,
                        vec![],
                        next.clone(),
                        &ctx
                    )
                    .unwrap()
                    .is_none()
            );
            assert_eq!(cache.scan.as_ref().unwrap().context, next);
            assert_eq!(
                cache.context.as_ref(),
                Some(&first),
                "stale ready result was not adopted"
            );
            assert!(cache.tiles[0].usages.is_empty());
            cache.refresh_usages(&document, &next).unwrap();
            assert!(
                cache.tiles[0].usages.is_empty(),
                "refresh must not reuse the old service"
            );
            assert!(cache.usages_dirty);
        }
        // Wait for our final metadata worker before deleting its owned fixture.
        let final_result = cache
            .scan
            .take()
            .unwrap()
            .results
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(final_result.is_ok());
    }

    #[test]
    fn background_catalog_keeps_latest_scene_edits_and_rejects_an_obsolete_index() {
        let fixture = Fixture::new();
        fixture.png("test.png", [10, 20, 30, 255]);
        let initial = crate::game_host::demo_document();
        let ctx = egui::Context::default();
        let mut cache = AssetsWorkbench::default();
        assert!(
            cache
                .synchronize_in_background(&initial, &fixture.root, vec![], &ctx)
                .unwrap()
                .is_none()
        );
        assert!(
            cache.scan.is_some(),
            "first UI call submits a job without consuming it"
        );
        let mut edited = initial.clone();
        edited.name = "Scene edited while scan was running".into();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let indexed = loop {
            if let Some(candidate) = cache
                .synchronize_in_background(&edited, &fixture.root, vec![], &ctx)
                .unwrap()
            {
                break candidate;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "catalog worker did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(indexed.name, edited.name);
        assert_eq!(indexed.game, edited.game);
        assert_eq!(cache.tiles.len(), 1);
        let ready_tiles = cache.tiles.clone();
        cache.invalidate();
        cache
            .synchronize_in_background(&indexed, &fixture.root, vec![], &ctx)
            .unwrap();
        let job = cache.scan.as_ref().unwrap();
        // Wait for the background work itself without applying it. Then change
        // the authoritative index before UI adoption, as a file Undo can do.
        let prepared = job
            .results
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let (sender, results) = mpsc::channel();
        sender.send(prepared).unwrap();
        cache.scan.as_mut().unwrap().results = results;
        let mut newer = indexed.clone();
        newer.asset_manifest["assets"][&ready_tiles[0].id]["sha256"] =
            serde_json::json!("external-update");
        assert!(
            cache
                .synchronize_in_background(&newer, &fixture.root, vec![], &ctx)
                .unwrap()
                .is_none()
        );
        assert!(cache.scan.is_none());
        assert!(cache.scan_needed);
        assert_eq!(cache.tiles[0].id, ready_tiles[0].id);
    }

    #[test]
    fn model_thumbnail_renders_loaded_geometry_instead_of_a_generic_icon() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut library = ContentLibrary::new(root).unwrap();
        let thumbnail = make_thumbnail(
            &mut library,
            "assets/models/demo_crystal.glb",
            AssetKind::Model,
        )
        .unwrap();
        assert_eq!(
            (thumbnail.width, thumbnail.height),
            (THUMBNAIL_EDGE, THUMBNAIL_EDGE)
        );
        assert!(thumbnail.metadata.iter().any(|(name, count)| name == "Треугольников" && count.parse::<usize>().unwrap() > 0));
        let colors: BTreeSet<_> = thumbnail.rgba.as_chunks::<4>().0.iter().copied().collect();
        assert!(
            colors.len() > 2,
            "real geometry creates lit surfaces and background"
        );
    }
    #[test]
    fn resizing_keeps_texture_shape_and_actual_pixel_colors() {
        let mut rgba = vec![];
        for y in 0..200 {
            for x in 0..400 {
                rgba.extend_from_slice(if x < 200 && y < 200 {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 255]
                });
            }
        }
        let (width, height, resized) = resized_rgba(400, 200, &rgba, 100).unwrap();
        assert_eq!((width, height), (100, 50));
        assert_eq!(&resized[..4], &[255, 0, 0, 255]);
        assert_eq!(&resized[99 * 4..100 * 4], &[0, 0, 255, 255]);
        assert!(resized_rgba(0, 1, &[], 100).is_err());
    }
    #[test]
    fn audio_details_come_from_wav_header_without_loading_whole_audio() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let details = wav_metadata(&root.join("assets/audio/attack.wav")).unwrap();
        assert!(
            details
                .iter()
                .any(|(name, value)| name == "Частота" && value.ends_with(" Гц"))
        );
        assert!(
            details
                .iter()
                .any(|(name, value)| name == "Длительность" && value.ends_with(" с"))
        );
    }
    #[test]
    fn thumbnail_queue_keeps_all_requests_and_loads_real_texture_pixels() {
        let fixture = Fixture::new();
        fixture.png("red.png", [255, 0, 0, 255]);
        fixture.png("blue.png", [0, 0, 255, 255]);
        let initial = module_engine_core::demo_document(module_engine_core::WorldKind::Objects);
        let mut assets = AssetsWorkbench::default();
        let document = assets
            .synchronize_cache(&initial, &fixture.root, vec![])
            .unwrap()
            .unwrap();
        assert!(assets.workspace().is_some());
        let ids: Vec<_> = assets.tiles.iter().map(|tile| tile.id.clone()).collect();
        for id in &ids {
            assets.queue_preview(id, &document).unwrap();
        }
        assets.queue_preview(&ids[0], &document).unwrap();
        assert_eq!(
            assets.pending_previews.len(),
            2,
            "busy worker must queue both and deduplicate the repeated request"
        );
        let ctx = egui::Context::default();
        let mut pixels = BTreeSet::new();
        for _ in 0..100 {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                assets.poll_previews(ui.ctx())
            });
            for tile in &assets.tiles {
                if let Some(texture) = tile.preview
                    && let Some(deltas) = output.textures_delta.set.get(&texture)
                {
                    for delta in deltas {
                        let egui::ImageData::Color(image) = &delta.image;
                        assert_eq!(image.size, [4, 2]);
                        pixels.insert(image.pixels[0].to_array());
                    }
                }
            }
            output.drop_without_applying_deltas();
            if assets.pending_previews.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(assets.pending_previews.is_empty());
        assert_eq!(assets.textures.len(), 2);
        assert!(pixels.contains(&[255, 0, 0, 255]));
        assert!(pixels.contains(&[0, 0, 255, 255]));
        assert!(!assets.needs_synchronization(&fixture.root.canonicalize().unwrap(), &[]));
    }
    #[test]
    fn project_reset_discards_previous_generation_results_and_navigation() {
        let first = Fixture::new();
        first.png("first.png", [255, 0, 0, 255]);
        let second = Fixture::new();
        second.png("second.png", [0, 0, 255, 255]);
        let initial = module_engine_core::demo_document(module_engine_core::WorldKind::Objects);
        let mut assets = AssetsWorkbench::default();
        let document = assets
            .synchronize_cache(&initial, &first.root, vec![])
            .unwrap()
            .unwrap();
        let old_id = assets.tiles[0].id.clone();
        assets.queue_preview(&old_id, &document).unwrap();
        assets
            .browsers
            .entry(("project".into(), 1))
            .or_default()
            .search = "old search".into();
        assets.reset();
        assert!(assets.pending_previews.is_empty());
        assert!(assets.worker.is_none());
        assert!(assets.browser("project", 1).is_none());
        let document = assets
            .synchronize_cache(&initial, &second.root, vec![])
            .unwrap()
            .unwrap();
        let new_id = assets.tiles[0].id.clone();
        assets.queue_preview(&new_id, &document).unwrap();
        let result = assets
            .worker
            .as_ref()
            .unwrap()
            .results
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(result.id, new_id);
        assert_ne!(result.id, old_id);
        assert_eq!(&result.image.unwrap().rgba[..4], &[0, 0, 255, 255]);
        assert_eq!(assets.tiles.len(), 1);
        assert_eq!(assets.tiles[0].path, "assets/second.png");
    }
    #[test]
    fn stale_checkpoint_paths_and_deleted_ids_are_rejected_before_restoration() {
        let fixture = Fixture::new();
        fixture.png("before.png", [255, 0, 0, 255]);
        let mut document =
            module_engine_core::demo_document(module_engine_core::WorldKind::Objects);
        let mut manifest = AssetManifest::default();
        manifest
            .register("image", "assets/before.png", &fixture.root)
            .unwrap();
        document.asset_manifest = serde_json::to_value(manifest).unwrap();
        assert!(verify_restore_resources(&document, &fixture.root).is_ok());
        fs::rename(
            fixture.root.join("assets/before.png"),
            fixture.root.join("assets/after.png"),
        )
        .unwrap();
        assert!(verify_restore_resources(&document, &fixture.root).is_err());
        document.asset_manifest["assets"]["image"]["path"] = "assets/after.png".into();
        assert!(
            verify_restore_resources(&document, &fixture.root).is_ok(),
            "physical file replay/restored path is accepted"
        );
        document.asset_manifest = serde_json::Value::Null;
        document.game = Some(module_engine_two_arenas::demo_project());
        document.game.as_mut().unwrap().audio_cues.clear();
        document
            .game
            .as_mut()
            .unwrap()
            .audio_cues
            .insert("hit".into(), "asset://deleted".into());
        assert!(verify_restore_resources(&document, &fixture.root).is_err());
        document.game.as_mut().unwrap().audio_cues.clear();
        document
            .custom_sources
            .insert("example".into(), "let s = \"assets/missing.png\";".into());
        assert!(
            verify_restore_resources(&document, &fixture.root).is_ok(),
            "opaque Rust source is not rewritten or interpreted as an asset field"
        );
    }
    #[test]
    fn first_index_keeps_redo_and_stable_ids_and_still_requires_save() {
        let fixture = Fixture::new();
        fixture.png("image.png", [255, 0, 0, 255]);
        let mut initial = module_engine_core::demo_document(module_engine_core::WorldKind::Objects);
        fixture.png("known.png", [0, 0, 255, 255]);
        let mut existing = AssetManifest::default();
        existing
            .register("known-image", "assets/known.png", &fixture.root)
            .unwrap();
        initial.asset_manifest = serde_json::to_value(existing).unwrap();
        initial.game = Some(module_engine_two_arenas::demo_project());
        initial.game.as_mut().unwrap().audio_cues.clear();
        let scene = initial.game.as_ref().unwrap().entry_scene.clone();
        let original = initial.game.as_ref().unwrap().scenes[&scene].objects[0]
            .object
            .transform
            .translation;
        let mut editor =
            super::super::Editor::new_with_policy(initial, fixture.root.join("project.json"), None);
        editor
            .document
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut(&scene)
            .unwrap()
            .objects[0]
            .object
            .transform
            .translation[0] += 5.0;
        editor.changed(false);
        editor.flush_history();
        editor.undo();
        assert!(editor.history.can_redo());
        assert!(!editor.has_unsaved_work());
        let mut assets = AssetsWorkbench::default();
        let indexed = assets
            .synchronize_cache(&editor.document, &fixture.root, vec![])
            .unwrap()
            .unwrap();
        let metadata = indexed.asset_manifest.clone();
        editor.accept_asset_index(indexed);
        assert!(editor.has_unsaved_work(), "new IDs are not silently saved");
        assert!(
            !editor.history_dirty,
            "metadata is not another scene gesture"
        );
        assert!(
            editor.history.can_redo(),
            "index adoption does not discard redo"
        );
        assert_eq!(editor.history.entries().len(), 2);
        let json: GraphDocument = serde_json::from_str(&editor.json_draft).unwrap();
        assert_eq!(json.asset_manifest, metadata);
        editor.redo();
        assert_eq!(
            editor.document.game.as_ref().unwrap().scenes[&scene].objects[0]
                .object
                .transform
                .translation[0],
            original[0] + 5.0
        );
        assert_eq!(editor.document.asset_manifest, metadata);
        editor.undo();
        assert_eq!(
            editor.document.game.as_ref().unwrap().scenes[&scene].objects[0]
                .object
                .transform
                .translation,
            original
        );
        assert_eq!(editor.document.asset_manifest, metadata);
        assert!(!editor.history.can_undo());
        assert!(
            assets
                .synchronize_cache(&editor.document, &fixture.root, vec![])
                .unwrap()
                .is_none(),
            "reopening/scanning after scene Undo does not generate replacement IDs"
        );
    }
    #[test]
    fn first_index_preserves_an_uncommitted_scene_history_change() {
        let fixture = Fixture::new();
        fixture.png("image.png", [255, 0, 0, 255]);
        let initial = module_engine_core::demo_document(module_engine_core::WorldKind::Objects);
        let mut editor =
            super::super::Editor::new_with_policy(initial, fixture.root.join("project.json"), None);
        editor.document.name = "Pending author edit".into();
        editor.changed(false);
        let mut assets = AssetsWorkbench::default();
        let indexed = assets
            .synchronize_cache(&editor.document, &fixture.root, vec![])
            .unwrap()
            .unwrap();
        let metadata = indexed.asset_manifest.clone();
        editor.accept_asset_index(indexed);
        assert!(editor.history_dirty);
        editor.flush_history();
        assert_eq!(editor.history.entries().len(), 2);
        assert_eq!(editor.history.undo().unwrap().asset_manifest, metadata);
        assert_eq!(editor.history.redo().unwrap().name, "Pending author edit");
    }
}
