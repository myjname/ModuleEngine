//! Stable project assets and reversible filesystem transactions.
//!
//! Files beneath `assets/` and explicitly registered extra files belong to the
//! project. Installed backend resources can be excluded by their relative paths.
//! A transaction validates all source/target files before writing, stores durable
//! snapshots, and rolls back touched files on failure. Its record must accompany
//! the editor's corresponding document-history boundary.
use crate::asset_fields::{AssetFields, remove_field};
use crate::{
    AssetEntry, AssetManifest, hash_bytes, hash_file, normalize_relative, resolve_relative,
};
use module_engine_core::{CompositionContext, GraphDocument};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

pub fn asset_reference(id: &str) -> String {
    format!("asset://{id}")
}

#[cfg(test)]
thread_local! {
    static USAGE_DOCUMENT_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetKind {
    Model,
    Texture,
    Audio,
    Shader,
    Other,
}
impl AssetKind {
    pub fn for_path(path: &str) -> Self {
        match Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "gltf" | "glb" => Self::Model,
            "png" | "jpg" | "jpeg" => Self::Texture,
            "wav" => Self::Audio,
            "hlsl" | "hlsli" => Self::Shader,
            _ => Self::Other,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetInfo {
    pub id: String,
    pub path: String,
    pub kind: AssetKind,
    pub bytes: u64,
    pub missing: bool,
    pub dependencies: Vec<String>,
    /// Invalid/unsupported content is still visible in the browser. Mutating
    /// operations independently revalidate dependency closure before writing.
    pub diagnostic: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetUsage {
    pub location: String,
    pub reference: String,
}

#[derive(Clone, Debug)]
pub enum DeletePolicy {
    RefuseUsed,
    /// Remove matching resource fields/cues. Objects and unrelated fields remain.
    Detach,
    ReplaceWith(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetFileSnapshot {
    pub snapshot_path: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetFileDelta {
    pub path: String,
    pub before: Option<AssetFileSnapshot>,
    pub after: Option<AssetFileSnapshot>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetOperationRecord {
    pub id: String,
    pub label: String,
    pub before: GraphDocument,
    pub after: GraphDocument,
    pub files: Vec<AssetFileDelta>,
    /// Delete transactions also expose their originals in the project trash.
    pub deleted_asset: Option<(String, AssetEntry)>,
    #[serde(default)]
    pub created_directories: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct AssetOperation {
    pub document: GraphDocument,
    pub record: AssetOperationRecord,
    /// Exact old references in opaque archived data were preserved rather than
    /// guessed. Hosts must display these diagnostics after a successful edit.
    pub diagnostics: Vec<String>,
}

impl AssetOperationRecord {
    pub fn undo(
        &self,
        project_root: &Path,
        current: &GraphDocument,
    ) -> Result<GraphDocument, String> {
        self.replay(project_root, current, false)
    }
    pub fn redo(
        &self,
        project_root: &Path,
        current: &GraphDocument,
    ) -> Result<GraphDocument, String> {
        self.replay(project_root, current, true)
    }
    fn replay(
        &self,
        root: &Path,
        current: &GraphDocument,
        forward: bool,
    ) -> Result<GraphDocument, String> {
        let (expected, result) = if forward {
            (&self.before, &self.after)
        } else {
            (&self.after, &self.before)
        };
        if current != expected {
            return Err("Asset history document boundary differs; replay was not applied".into());
        }
        let workspace = AssetWorkspace::new(root, Vec::<String>::new())?;
        let mut changes = BTreeMap::new();
        for file in &self.files {
            let (source, target) = if forward {
                (&file.before, &file.after)
            } else {
                (&file.after, &file.before)
            };
            workspace.check_snapshot_state(&file.path, source)?;
            changes.insert(file.path.clone(), workspace.read_snapshot(target)?);
        }
        for directory in &self.created_directories {
            if directory != "assets" {
                workspace.editable(directory)?;
            }
            let path = workspace.checked_target(directory)?;
            if forward {
                if path.exists() {
                    return Err("Asset folder already exists; redo was not applied".into());
                }
            } else {
                if !path.is_dir() {
                    return Err("Asset folder is missing; undo was not applied".into());
                }
                for entry in fs::read_dir(&path).map_err(|e| e.to_string())? {
                    let entry = entry.map_err(|e| e.to_string())?;
                    let relative = entry
                        .path()
                        .strip_prefix(&workspace.root)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .replace('\\', "/");
                    if !self.created_directories.contains(&relative) {
                        return Err("Asset folder contains files; undo was not applied".into());
                    }
                }
            }
        }
        workspace.write_changes(&changes)?;
        let directories: Vec<_> = if forward {
            self.created_directories.iter().collect()
        } else {
            self.created_directories.iter().rev().collect()
        };
        for directory in directories {
            let path = workspace.checked_target(directory)?;
            if forward {
                fs::create_dir_all(path).map_err(|e| e.to_string())?;
            } else {
                fs::remove_dir(path).map_err(|e| e.to_string())?;
            }
        }
        Ok(result.clone())
    }
}

pub struct AssetWorkspace {
    root: PathBuf,
    protected: Vec<String>,
    validator: Option<DocumentValidator>,
    composition: Option<CompositionContext>,
}
type DocumentValidator = Arc<dyn Fn(&GraphDocument) -> Result<(), String> + Send + Sync + 'static>;
fn reject_schema_default_rewrites(
    fields: &AssetFields,
    matches: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let dependencies: Vec<_> = fields
        .default_references
        .iter()
        .filter(|(_, reference)| matches(reference))
        .map(|(location, _)| location.as_str())
        .collect();
    if !dependencies.is_empty() {
        return Err(format!(
            "Asset is supplied by an immutable installed schema default; author explicit component values before moving, replacing or detaching it: {}",
            dependencies.join(", ")
        ));
    }
    Ok(())
}
impl AssetWorkspace {
    /// Construct a workspace with structural scene preflight and builtin render
    /// resource contracts only. Hosts must provide with_composition for source
    /// fields and with_validator for their installed semantic preflight.
    pub fn new(
        root: impl AsRef<Path>,
        protected: impl IntoIterator<Item = String>,
    ) -> Result<Self, String> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|e| format!("Project asset root: {e}"))?;
        if !root.is_dir() {
            return Err("Project asset root must be a directory".into());
        }
        let protected = protected
            .into_iter()
            .map(|p| normalize_relative(&p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            root,
            protected,
            validator: None,
            composition: None,
        })
    }
    /// Add the host's installed schema/policy validation to every candidate
    /// preflight, before transaction snapshots, trash or asset files are written.
    /// The callback is retained by this workspace across its operations.
    pub fn with_validator<F>(mut self, validator: F) -> Self
    where
        F: Fn(&GraphDocument) -> Result<(), String> + Send + Sync + 'static,
    {
        self.validator = Some(Arc::new(validator));
        self
    }
    /// Supply installed, pinned source contracts explicitly. A validation
    /// callback alone cannot establish which arbitrary strings are resources.
    pub fn with_composition(mut self, composition: CompositionContext) -> Self {
        self.composition = Some(composition);
        self
    }
    fn validate_candidate(&self, document: &GraphDocument) -> Result<(), String> {
        validate_candidate(document)?;
        if let Some(validator) = &self.validator {
            validator(document)?;
        }
        Ok(())
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Import worker primitive for a fresh project-relative file. Ownership is
    /// checked against existing ancestors before creating directories; create_new
    /// refuses an occupied destination. Follow with adopt_import for history.
    pub fn write_new_file(&self, relative: &str, bytes: &[u8]) -> Result<String, String> {
        let relative = self.editable(relative)?;
        let target = self.checked_target(&relative)?;
        write_new(&target, bytes)?;
        Ok(relative)
    }
    pub fn create_folder(
        &self,
        document: &GraphDocument,
        path: &str,
    ) -> Result<AssetOperation, String> {
        let path = self.editable(path)?;
        let target = self.checked_target(&path)?;
        self.checked_target(".asset-history/.preflight")?;
        if target.exists() {
            return Err("Asset folder already exists".into());
        }
        let parent = target.parent().ok_or("Asset folder has no parent")?;
        let create_assets = parent == self.root.join("assets") && !parent.exists();
        if !parent.is_dir() {
            if create_assets {
                fs::create_dir(parent).map_err(|e| e.to_string())?;
            } else {
                return Err("Choose an existing parent asset folder".into());
            }
        }
        if let Err(error) = fs::create_dir(&target) {
            if create_assets {
                let _ = fs::remove_dir(parent);
            }
            return Err(error.to_string());
        }
        let record = AssetOperationRecord {
            id: crate::history::unique_id(),
            label: format!("New asset folder: {path}"),
            before: document.clone(),
            after: document.clone(),
            files: vec![],
            deleted_asset: None,
            created_directories: if create_assets {
                vec!["assets".into(), path]
            } else {
                vec![path]
            },
        };
        if let Err(error) = write_new(
            &self.checked_target(&format!(".asset-history/{}/record.json", record.id))?,
            &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
        ) {
            for directory in record.created_directories.iter().rev() {
                let _ = fs::remove_dir(self.root.join(directory));
            }
            return Err(error);
        }
        Ok(AssetOperation {
            document: document.clone(),
            record,
            diagnostics: vec![],
        })
    }
    /// Adopt files freshly created by the import worker. Caller must pass only
    /// that import's new files (its unique destination prevents overwrites).
    /// Recording them makes Undo remove files and metadata together.
    pub fn adopt_import(
        &self,
        document: &GraphDocument,
        files: &[String],
    ) -> Result<AssetOperation, String> {
        let manifest = AssetManifest::from_value(&document.asset_manifest)?;
        let mut paths: BTreeSet<String> = files
            .iter()
            .map(|p| self.editable(p))
            .collect::<Result<_, _>>()?;
        let mut pending: Vec<_> = paths.iter().cloned().collect();
        while let Some(path) = pending.pop() {
            let bytes =
                fs::read(resolve_relative(&self.root, &path)?).map_err(|e| e.to_string())?;
            for link in file_links(&self.root, &path, &bytes)? {
                if !self.protected(&link.target) && paths.insert(link.target.clone()) {
                    pending.push(self.editable(&link.target)?);
                }
            }
        }
        if paths
            .iter()
            .any(|p| manifest.assets.values().any(|e| same_path(&e.path, p)))
        {
            return Err("Import adoption cannot include previously indexed files".into());
        }
        let mut candidate = document.clone();
        self.synchronize(&mut candidate)?;
        self.validate_candidate(&candidate)?;
        let id = crate::history::unique_id();
        self.with_import_journal(&id, || {
            let mut deltas = vec![];
            for (index, path) in paths.iter().enumerate() {
                let bytes =
                    fs::read(resolve_relative(&self.root, path)?).map_err(|e| e.to_string())?;
                let snapshot_path = format!(".asset-history/{id}/after/{index}.bin");
                write_new(&self.checked_target(&snapshot_path)?, &bytes)?;
                deltas.push(AssetFileDelta {
                    path: path.clone(),
                    before: None,
                    after: Some(AssetFileSnapshot {
                        snapshot_path,
                        sha256: hash_bytes(&bytes),
                    }),
                });
            }
            let record = AssetOperationRecord {
                id: id.clone(),
                label: "Import asset".into(),
                before: document.clone(),
                after: candidate.clone(),
                files: deltas,
                deleted_asset: None,
                created_directories: vec![],
            };
            write_new(
                &self.checked_target(&format!(".asset-history/{}/record.json", record.id))?,
                &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
            )?;
            Ok(AssetOperation {
                document: candidate,
                record,
                diagnostics: vec![],
            })
        })
    }
    /// Adoption has not mutated live files yet. A failed snapshot/record write
    /// may therefore remove only its freshly created, marked journal directory.
    /// Normal file-operation failures retain recovery snapshots instead.
    fn with_import_journal<T>(
        &self,
        id: &str,
        write: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("Invalid import journal ID".into());
        }
        let parent = self.checked_target(".asset-history")?;
        fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
        let parent = parent.canonicalize().map_err(|error| error.to_string())?;
        if !parent.starts_with(&self.root) {
            return Err("Import journal leaves the project".into());
        }
        let journal = parent.join(id);
        fs::create_dir(&journal).map_err(|error| error.to_string())?;
        let journal = journal.canonicalize().map_err(|error| error.to_string())?;
        if journal.parent() != Some(parent.as_path()) {
            return Err("Import journal ownership check failed".into());
        }
        // The directory was created exclusively above; this marker permits
        // cleanup to verify ownership again immediately before recursive removal.
        let marker = journal.join(".import-owner");
        if let Err(error) = write_new(&marker, id.as_bytes()) {
            // No callback ran and remove_dir cannot delete pre-existing contents.
            let _ = fs::remove_file(&marker);
            let _ = fs::remove_dir(&journal);
            return Err(error);
        }
        let result = write();
        if let Err(error) = &result {
            let owned = journal.canonicalize().ok().as_ref() == Some(&journal)
                && journal.parent() == Some(parent.as_path())
                && journal.starts_with(&self.root)
                && fs::read(&marker).ok().as_deref() == Some(id.as_bytes());
            if !owned {
                return Err(format!(
                    "{error}; import journal retained after failed ownership check: {}",
                    journal.display()
                ));
            }
            fs::remove_dir_all(&journal).map_err(|cleanup| {
                format!(
                    "{error}; import journal retained at {}: {cleanup}",
                    journal.display()
                )
            })?;
        }
        result
    }
    fn protected(&self, path: &str) -> bool {
        let path = path.to_ascii_lowercase();
        self.protected.iter().any(|p| {
            let p = p.to_ascii_lowercase();
            path == p || path.starts_with(&(p + "/"))
        })
    }
    fn editable(&self, path: &str) -> Result<String, String> {
        let path = normalize_relative(path)?;
        if self.protected(&path) || path.starts_with(".asset-") || !path.starts_with("assets/") {
            return Err(format!(
                "Asset operation cannot change protected/non-assets path: {path}"
            ));
        }
        self.checked_target(&path)?;
        Ok(path)
    }
    /// Scan without modifying source files. Existing IDs survive content edits;
    /// hashes/dependencies refresh, and missing entries remain visibly missing.
    pub fn synchronize(&self, document: &mut GraphDocument) -> Result<Vec<AssetInfo>, String> {
        let mut manifest = AssetManifest::from_value(&document.asset_manifest)?;
        let mut paths = BTreeSet::new();
        let mut visited = BTreeSet::new();
        if self.root.join("assets").exists() {
            self.scan_directory("assets", &mut paths, &mut visited)?;
        }
        paths.extend(
            manifest
                .assets
                .values()
                .filter(|e| !self.protected(&e.path))
                .map(|e| e.path.clone()),
        );
        let mut known = BTreeMap::new();
        for (id, entry) in &manifest.assets {
            let path = normalize_relative(&entry.path)?;
            if known
                .insert(path.to_ascii_lowercase(), id.clone())
                .is_some()
            {
                return Err(format!("Duplicate indexed asset path: {path}"));
            }
        }
        let mut pending: Vec<_> = paths.iter().cloned().collect();
        let mut analyzed = BTreeSet::new();
        let mut diagnostics = BTreeMap::new();
        while let Some(path) = pending.pop() {
            if !analyzed.insert(path.clone()) {
                continue;
            }
            if let Ok(file) = resolve_relative(&self.root, &path) {
                match file_links(
                    &self.root,
                    &path,
                    &fs::read(file).map_err(|e| e.to_string())?,
                ) {
                    Ok(links) => {
                        for link in links {
                            if paths.insert(link.target.clone()) {
                                pending.push(link.target);
                            }
                        }
                    }
                    Err(error) => {
                        diagnostics.insert(path, error);
                    }
                }
            }
        }
        for path in &paths {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                known.entry(path.to_ascii_lowercase())
            {
                let id = unique_asset_id();
                manifest.register(&id, path, &self.root)?;
                entry.insert(id);
            }
        }
        // Resolve dependencies against the entire discovered index, not only
        // files already encountered in directory traversal.
        for (id, entry) in &mut manifest.assets {
            if self.protected(&entry.path) {
                continue;
            }
            if let Ok(file) = resolve_relative(&self.root, &entry.path) {
                entry.sha256 = hash_file(&file)?;
                let bytes = fs::read(file).map_err(|e| e.to_string())?;
                if let Ok(links) = file_links(&self.root, &entry.path, &bytes) {
                    entry.dependencies = links
                        .iter()
                        .map(|l| {
                            known
                                .get(&l.target.to_ascii_lowercase())
                                .cloned()
                                .ok_or_else(|| {
                                    format!("Asset {id}: dependency is not indexed: {}", l.target)
                                })
                        })
                        .collect::<Result<BTreeSet<_>, _>>()?
                        .into_iter()
                        .collect();
                }
            }
        }
        let info = manifest
            .assets
            .iter()
            .filter(|(_, e)| !self.protected(&e.path))
            .map(|(id, e)| {
                let metadata = resolve_relative(&self.root, &e.path)
                    .ok()
                    .and_then(|p| fs::metadata(p).ok());
                AssetInfo {
                    id: id.clone(),
                    path: e.path.clone(),
                    kind: AssetKind::for_path(&e.path),
                    bytes: metadata.as_ref().map_or(0, |m| m.len()),
                    missing: metadata.is_none(),
                    dependencies: e.dependencies.clone(),
                    diagnostic: diagnostics.get(&e.path).cloned(),
                }
            })
            .collect();
        document.asset_manifest = serde_json::to_value(manifest).map_err(|e| e.to_string())?;
        Ok(info)
    }
    fn scan_directory(
        &self,
        relative: &str,
        files: &mut BTreeSet<String>,
        visited: &mut BTreeSet<PathBuf>,
    ) -> Result<(), String> {
        if self.protected(relative) {
            return Ok(());
        }
        let dir = self
            .root
            .join(relative)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if !dir.starts_with(&self.root) {
            return Err(format!("Asset directory leaves project: {relative}"));
        }
        if !visited.insert(dir.clone()) {
            return Err(format!("Asset directory cycle/alias: {relative}"));
        }
        for item in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let item = item.map_err(|e| e.to_string())?;
            let path = format!("{relative}/{}", item.file_name().to_string_lossy());
            if self.protected(&path) {
                continue;
            }
            // Player export owns this reserved prefix for its unpublished
            // sibling directories. They can disappear while a scan is running.
            if item
                .file_name()
                .to_string_lossy()
                .starts_with(".module-engine-game-export-")
                && item
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
            {
                continue;
            }
            let resolved = item.path().canonicalize().map_err(|e| e.to_string())?;
            if !resolved.starts_with(&self.root) {
                return Err(format!("Asset link leaves project: {path}"));
            }
            if resolved.is_dir() {
                self.scan_directory(&path, files, visited)?;
            } else if resolved.is_file() {
                files.insert(normalize_relative(&path)?);
            }
        }
        Ok(())
    }
    pub fn usages(&self, document: &GraphDocument, id: &str) -> Result<Vec<AssetUsage>, String> {
        self.usages_batch(document)?
            .remove(id)
            .ok_or_else(|| format!("Unknown asset ID: {id}"))
    }

    /// Build the complete reverse-reference index with one document walk and
    /// one include traversal per inline shader. Browser refreshes must use this
    /// batch instead of serializing the document independently for every tile.
    pub fn usages_batch(
        &self,
        document: &GraphDocument,
    ) -> Result<BTreeMap<String, Vec<AssetUsage>>, String> {
        let manifest = AssetManifest::from_value(&document.asset_manifest)?;
        let paths = manifest
            .assets
            .iter()
            .map(|(id, entry)| {
                normalize_relative(&entry.path).map(|path| (path.to_ascii_lowercase(), id.clone()))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let mut result: BTreeMap<String, Vec<AssetUsage>> = manifest
            .assets
            .keys()
            .map(|id| (id.clone(), vec![]))
            .collect();
        #[cfg(test)]
        USAGE_DOCUMENT_WALKS.with(|walks| walks.set(walks.get() + 1));
        let value = serde_json::to_value(document).map_err(|e| e.to_string())?;
        let fields = AssetFields::collect(document, self.composition.as_ref())?;
        let mut add_usage = |location: String, text: &str| {
            let id = if let Some(id) = text.strip_prefix("asset://") {
                manifest.assets.contains_key(id).then(|| id.to_owned())
            } else {
                normalize_relative(text)
                    .ok()
                    .and_then(|path| paths.get(&path.to_ascii_lowercase()).cloned())
            };
            if let Some(usages) = id.and_then(|id| result.get_mut(&id)) {
                usages.push(AssetUsage {
                    location,
                    reference: text.into(),
                });
            }
        };
        for location in &fields.references {
            if let Some(text) = value.pointer(location).and_then(Value::as_str) {
                add_usage(location.clone(), text);
            }
        }
        for (location, reference) in &fields.default_references {
            add_usage(location.clone(), reference);
        }
        for (owner, asset) in &manifest.assets {
            for dependency in asset.dependencies.iter().collect::<BTreeSet<_>>() {
                if dependency != owner
                    && let Some(usages) = result.get_mut(dependency)
                {
                    usages.push(AssetUsage {
                        location: format!("file:{}", asset.path),
                        reference: manifest.assets[dependency].path.clone(),
                    });
                }
            }
        }
        for (source_id, source) in &document.shader_sources {
            if let Ok(links) = hlsl_links(&self.root, "inline.hlsl", source, false) {
                let linked: BTreeSet<_> = links
                    .into_iter()
                    .map(|link| link.target.to_ascii_lowercase())
                    .collect();
                for path in linked {
                    if let Some(id) = paths.get(&path)
                        && let Some(usages) = result.get_mut(id)
                    {
                        usages.push(AssetUsage {
                            location: format!("shader_source:{source_id}"),
                            reference: manifest.assets[id].path.clone(),
                        });
                    }
                }
            }
        }
        Ok(result)
    }
    pub fn move_asset(
        &self,
        document: &GraphDocument,
        id: &str,
        destination: &str,
    ) -> Result<AssetOperation, String> {
        let manifest = AssetManifest::from_value(&document.asset_manifest)?;
        let entry = manifest
            .assets
            .get(id)
            .ok_or_else(|| format!("Unknown asset ID: {id}"))?;
        let old = self.editable(&entry.path)?;
        let new = self.editable(destination)?;
        if old.eq_ignore_ascii_case(&new) {
            return Err("Asset destination must differ, including letter case".into());
        }
        if self.checked_target(&new)?.exists()
            || manifest
                .assets
                .values()
                .any(|e| e.path.eq_ignore_ascii_case(&new))
        {
            return Err(format!("Asset destination already exists: {new}"));
        }
        reject_hardcoded_source(document, &old)?;
        self.validate_mutation_dependencies(document, &manifest)?;
        if manifest
            .assets
            .values()
            .any(|e| self.protected(&e.path) && e.dependencies.iter().any(|d| d == id))
        {
            return Err(
                "Asset is referenced by protected backend files; mutation is forbidden".into(),
            );
        }
        let (mut candidate, diagnostics) =
            self.rewrite_references(document, &old, id, Some(&new), false)?;
        let mut updated = manifest.clone();
        updated.assets.get_mut(id).unwrap().path = new.clone();
        let mut changes = BTreeMap::new();
        changes.insert(old.clone(), None);
        for asset in manifest
            .assets
            .values()
            .filter(|e| !self.protected(&e.path))
        {
            let before =
                fs::read(resolve_relative(&self.root, &asset.path)?).map_err(|e| e.to_string())?;
            let output_path = if same_path(&asset.path, &old) {
                &new
            } else {
                &asset.path
            };
            let rewritten =
                rewrite_file_links(&self.root, &asset.path, output_path, &before, &old, &new)?;
            if output_path != &asset.path || rewritten != before {
                if self.protected(output_path) {
                    return Err(format!(
                        "Dependency belongs to protected backend: {output_path}"
                    ));
                }
                updated
                    .assets
                    .iter_mut()
                    .filter(|(_, e)| same_path(&e.path, output_path))
                    .for_each(|(_, e)| e.sha256 = hash_bytes(&rewritten));
                changes.insert(output_path.clone(), Some(rewritten));
            }
        }
        rewrite_inline_includes(document, &mut candidate, &self.root, &old, &new)?;
        candidate.asset_manifest = serde_json::to_value(updated).map_err(|e| e.to_string())?;
        let mut operation = self.commit(
            document,
            candidate,
            changes,
            format!("Move asset: {old} → {new}"),
            None,
        )?;
        operation.diagnostics = diagnostics;
        Ok(operation)
    }
    pub fn delete_asset(
        &self,
        document: &GraphDocument,
        id: &str,
        policy: DeletePolicy,
    ) -> Result<AssetOperation, String> {
        let mut manifest = AssetManifest::from_value(&document.asset_manifest)?;
        let entry = manifest
            .assets
            .get(id)
            .ok_or_else(|| format!("Unknown asset ID: {id}"))?
            .clone();
        let path = self.editable(&entry.path)?;
        reject_hardcoded_source(document, &path)?;
        self.validate_mutation_dependencies(document, &manifest)?;
        if manifest
            .assets
            .values()
            .any(|e| self.protected(&e.path) && e.dependencies.iter().any(|d| d == id))
        {
            return Err(
                "Asset is referenced by protected backend files; mutation is forbidden".into(),
            );
        }
        let uses = self.usages(document, id)?;
        if matches!(policy, DeletePolicy::RefuseUsed) && !uses.is_empty() {
            return Err(format!(
                "Asset is used; replace/detach first: {}",
                uses.iter()
                    .map(|u| u.location.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // Embedded file dependencies cannot be 'detached' without corrupting a
        // model/shader. Replacement rewrites them; detach must first remove the
        // referring model/shader or choose a compatible replacement.
        if matches!(policy, DeletePolicy::Detach)
            && uses.iter().any(|u| {
                u.location.starts_with("file:") || u.location.starts_with("shader_source:")
            })
        {
            return Err(
                "Asset is required by a model/shader file; replace its dependency before deletion"
                    .into(),
            );
        }
        let replacement = if let DeletePolicy::ReplaceWith(other) = &policy {
            if other == id {
                return Err("Replacement must be a different asset".into());
            }
            let replacement = manifest
                .assets
                .get(other)
                .ok_or("Replacement asset does not exist")?;
            resolve_relative(&self.root, &replacement.path)?;
            if AssetKind::for_path(&path) != AssetKind::for_path(&replacement.path) {
                return Err("Replacement must have the same resource kind".into());
            }
            Some((other.clone(), replacement.path.clone()))
        } else {
            None
        };
        let (mut candidate, diagnostics) = self.rewrite_references(
            document,
            &path,
            id,
            replacement
                .as_ref()
                .map(|(other, _)| asset_reference(other))
                .as_deref(),
            true,
        )?;
        let mut changes = BTreeMap::from([(path.clone(), None)]);
        if let Some((_, target)) = &replacement {
            let file_assets: Vec<_> = manifest
                .assets
                .values()
                .filter(|e| !self.protected(&e.path) && !same_path(&e.path, &path))
                .cloned()
                .collect();
            for asset in file_assets {
                let before = fs::read(resolve_relative(&self.root, &asset.path)?)
                    .map_err(|e| e.to_string())?;
                let after = rewrite_file_links(
                    &self.root,
                    &asset.path,
                    &asset.path,
                    &before,
                    &path,
                    target,
                )?;
                if before != after {
                    manifest
                        .assets
                        .iter_mut()
                        .filter(|(_, e)| same_path(&e.path, &asset.path))
                        .for_each(|(_, e)| e.sha256 = hash_bytes(&after));
                    changes.insert(asset.path.clone(), Some(after));
                }
            }
            rewrite_inline_includes(document, &mut candidate, &self.root, &path, target)?;
        }
        manifest.assets.remove(id);
        for asset in manifest.assets.values_mut() {
            let used = asset.dependencies.iter().any(|d| d == id);
            asset.dependencies.retain(|d| d != id);
            if let Some((other, _)) = &replacement
                && used
                && !asset.dependencies.contains(other)
            {
                asset.dependencies.push(other.clone());
            }
        }
        candidate.asset_manifest = serde_json::to_value(manifest).map_err(|e| e.to_string())?;
        let mut operation = self.commit(
            document,
            candidate,
            changes,
            format!("Delete asset: {path}"),
            Some((id.into(), entry)),
        )?;
        operation.diagnostics = diagnostics;
        Ok(operation)
    }
    /// Trash records survive restarts. Restore recovers the original ID/file;
    /// document links intentionally remain as currently authored. Undo restores
    /// the original complete links as well as the file.
    pub fn list_deleted(&self) -> Result<Vec<AssetOperationRecord>, String> {
        let directory = self.root.join(".asset-trash");
        if !directory.exists() {
            return Ok(vec![]);
        }
        let mut result = vec![];
        for item in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let path = item.map_err(|e| e.to_string())?.path().join("record.json");
            if path.is_file() {
                let record: AssetOperationRecord =
                    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                if let Some((_, asset)) = &record.deleted_asset
                    && !self.root.join(&asset.path).exists()
                {
                    result.push(record);
                }
            }
        }
        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }
    pub fn restore_deleted(
        &self,
        document: &GraphDocument,
        trash_id: &str,
    ) -> Result<AssetOperation, String> {
        if !trash_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err("Invalid trash ID".into());
        }
        let path = self.checked_target(&format!(".asset-trash/{trash_id}/record.json"))?;
        let record: AssetOperationRecord =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let (id, entry) = record
            .deleted_asset
            .ok_or("Trash record has no deleted asset")?;
        self.editable(&entry.path)?;
        let mut manifest = AssetManifest::from_value(&document.asset_manifest)?;
        if manifest.assets.contains_key(&id) || self.root.join(&entry.path).exists() {
            return Err("Original asset ID/path is already occupied".into());
        }
        for dependency in &entry.dependencies {
            if !manifest.assets.contains_key(dependency) {
                return Err(format!("Restore requires missing dependency: {dependency}"));
            }
        }
        let file = record
            .files
            .iter()
            .find(|f| same_path(&f.path, &entry.path))
            .ok_or("Trash snapshot missing")?;
        let bytes = self
            .read_snapshot(&file.before)?
            .ok_or("Trash original missing")?;
        manifest.assets.insert(id, entry.clone());
        let mut candidate = document.clone();
        candidate.asset_manifest = serde_json::to_value(manifest).map_err(|e| e.to_string())?;
        self.commit(
            document,
            candidate,
            BTreeMap::from([(entry.path.clone(), Some(bytes))]),
            format!("Restore asset: {}", entry.path),
            None,
        )
    }
    fn commit(
        &self,
        before: &GraphDocument,
        after: GraphDocument,
        changes: BTreeMap<String, Option<Vec<u8>>>,
        label: String,
        deleted: Option<(String, AssetEntry)>,
    ) -> Result<AssetOperation, String> {
        self.validate_candidate(&after)?;
        let id = crate::history::unique_id();
        let mut deltas = vec![];
        for (index, (path, bytes)) in changes.iter().enumerate() {
            let target = self.checked_target(path)?;
            if self.protected(path) {
                return Err(format!("Protected asset cannot change: {path}"));
            }
            let original = if target.exists() {
                Some(fs::read(target).map_err(|e| e.to_string())?)
            } else {
                None
            };
            let store =
                |side: &str, data: &Option<Vec<u8>>| -> Result<Option<AssetFileSnapshot>, String> {
                    let Some(data) = data else { return Ok(None) };
                    let snapshot_path = format!(".asset-history/{id}/{side}/{index}.bin");
                    let target = self.checked_target(&snapshot_path)?;
                    write_new(&target, data)?;
                    Ok(Some(AssetFileSnapshot {
                        snapshot_path,
                        sha256: hash_bytes(data),
                    }))
                };
            deltas.push(AssetFileDelta {
                path: path.clone(),
                before: store("before", &original)?,
                after: store("after", bytes)?,
            });
        }
        let record = AssetOperationRecord {
            id: id.clone(),
            label,
            before: before.clone(),
            after: after.clone(),
            files: deltas,
            deleted_asset: deleted,
            created_directories: vec![],
        };
        if let Some((_, asset)) = &record.deleted_asset {
            let source = record
                .files
                .iter()
                .find(|f| same_path(&f.path, &asset.path))
                .and_then(|f| f.before.as_ref())
                .ok_or("Delete original snapshot missing")?;
            let original = self.read_snapshot(&Some(source.clone()))?.unwrap();
            write_new(
                &self.checked_target(&format!(".asset-trash/{id}/files/{}", asset.path))?,
                &original,
            )?;
            write_new(
                &self.checked_target(&format!(".asset-trash/{id}/record.json"))?,
                &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
            )?;
        }
        write_new(
            &self.checked_target(&format!(".asset-history/{id}/record.json"))?,
            &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
        )?;
        for file in &record.files {
            self.check_snapshot_state(&file.path, &file.before)?;
        }
        self.write_changes(&changes)?;
        Ok(AssetOperation {
            document: after,
            record,
            diagnostics: vec![],
        })
    }
    fn rewrite_references(
        &self,
        document: &GraphDocument,
        old: &str,
        id: &str,
        replacement: Option<&str>,
        rewrite_id: bool,
    ) -> Result<(GraphDocument, Vec<String>), String> {
        let fields = AssetFields::collect(document, self.composition.as_ref())?;
        let mut value = serde_json::to_value(document).map_err(|error| error.to_string())?;
        let matches =
            |text: &str| same_path(text, old) || (rewrite_id && text == asset_reference(id));
        reject_schema_default_rewrites(&fields, matches)?;
        let diagnostics = fields.ambiguous(&value, matches);
        for path in &fields.references {
            if value
                .pointer(path)
                .and_then(Value::as_str)
                .is_some_and(matches)
            {
                if let Some(replacement) = replacement {
                    *value.pointer_mut(path).expect("existing asset field") =
                        Value::String(replacement.into());
                } else {
                    remove_field(&mut value, path);
                    // The builtin mesh has only an optional path. Drop its
                    // now-empty attachment, but only when this exact typed
                    // field was detached; opaque/inactive mesh payloads and
                    // unrelated metadata must never be swept globally.
                    if path.ends_with("/components/mesh/path") {
                        let parent = path.strip_suffix("/path").expect("mesh path");
                        if value
                            .pointer(parent)
                            .and_then(Value::as_object)
                            .is_some_and(|fields| fields.is_empty())
                        {
                            remove_field(&mut value, parent);
                        }
                    }
                }
            }
        }
        let candidate = serde_json::from_value(value).map_err(|error| error.to_string())?;
        // Detaching an authored value may expose the same deleted asset from
        // its schema default. Preflight this effective dependency as well, so
        // neither a missing pointer nor field removal can bypass the guard.
        let candidate_fields = AssetFields::collect(&candidate, self.composition.as_ref())?;
        reject_schema_default_rewrites(&candidate_fields, matches)?;
        Ok((candidate, diagnostics))
    }
    fn validate_mutation_dependencies(
        &self,
        document: &GraphDocument,
        manifest: &AssetManifest,
    ) -> Result<(), String> {
        for entry in manifest
            .assets
            .values()
            .filter(|e| !self.protected(&e.path))
        {
            let bytes =
                fs::read(resolve_relative(&self.root, &entry.path)?).map_err(|e| e.to_string())?;
            file_links(&self.root, &entry.path, &bytes)?;
        }
        for (id, source) in &document.shader_sources {
            match hlsl_links(&self.root, "inline.hlsl", source, false) {
                Ok(_) => {}
                Err(error) if error.contains("missing dependency") => {}
                Err(error) => return Err(format!("Shader {id}: {error}")),
            }
        }
        Ok(())
    }
    fn checked_target(&self, path: &str) -> Result<PathBuf, String> {
        let relative = normalize_relative(path)?;
        let target = self.root.join(relative);
        let mut existing = target.as_path();
        while !existing.exists() {
            existing = existing.parent().ok_or("No project ancestor")?;
        }
        let canonical = existing.canonicalize().map_err(|e| e.to_string())?;
        if !canonical.starts_with(&self.root)
            || (target.exists() && !target.is_file() && target != existing)
        {
            return Err("Asset target leaves project or is not a file".into());
        }
        Ok(target)
    }
    fn read_snapshot(
        &self,
        snapshot: &Option<AssetFileSnapshot>,
    ) -> Result<Option<Vec<u8>>, String> {
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };
        if !snapshot.snapshot_path.starts_with(".asset-history/") {
            return Err("Invalid asset history snapshot".into());
        }
        let bytes =
            fs::read(self.checked_target(&snapshot.snapshot_path)?).map_err(|e| e.to_string())?;
        if hash_bytes(&bytes) != snapshot.sha256 {
            return Err("Asset history snapshot hash mismatch".into());
        }
        Ok(Some(bytes))
    }
    fn check_snapshot_state(
        &self,
        path: &str,
        expected: &Option<AssetFileSnapshot>,
    ) -> Result<(), String> {
        let target = self.checked_target(path)?;
        match expected {
            Some(snapshot) if target.is_file() && hash_file(&target)? == snapshot.sha256 => Ok(()),
            None if !target.exists() => Ok(()),
            _ => Err(format!(
                "Asset changed outside history; refusing to overwrite: {path}"
            )),
        }
    }
    fn write_changes(&self, changes: &BTreeMap<String, Option<Vec<u8>>>) -> Result<(), String> {
        let originals = changes
            .keys()
            .map(|p| {
                let target = self.checked_target(p)?;
                if target.exists() && !target.is_file() {
                    return Err(format!("Asset target is a directory: {p}"));
                }
                Ok((
                    p.clone(),
                    if target.exists() {
                        Some(fs::read(target).map_err(|e| e.to_string())?)
                    } else {
                        None
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        let mut touched = vec![];
        for (path, bytes) in changes {
            touched.push(path);
            if let Err(error) = write_current(&self.checked_target(path)?, bytes.as_deref()) {
                let mut rollback = vec![];
                for p in touched.into_iter().rev() {
                    let target = self.checked_target(p)?;
                    let original = originals[p].as_deref();
                    if original.is_none() && !target.exists() {
                        continue;
                    }
                    if original.is_some() && fs::read(&target).ok().as_deref() == original {
                        continue;
                    }
                    if let Err(e) = write_current(&target, original) {
                        rollback.push(e);
                    }
                }
                return Err(if rollback.is_empty() {
                    error
                } else {
                    format!("{error}; rollback errors: {}", rollback.join("; "))
                });
            }
        }
        Ok(())
    }
}

fn unique_asset_id() -> String {
    format!("a-{}", crate::history::unique_id())
}
fn validate_candidate(document: &GraphDocument) -> Result<(), String> {
    if let Some(game) = &document.game {
        game.validate_scene_data()?;
    }
    Ok(())
}
fn same_path(a: &str, b: &str) -> bool {
    normalize_relative(a)
        .ok()
        .is_some_and(|a| a.eq_ignore_ascii_case(b))
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("Missing file parent")?).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn write_current(path: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
    if let Some(bytes) = bytes {
        fs::create_dir_all(path.parent().ok_or("Missing asset parent")?)
            .map_err(|e| e.to_string())?;
        let temporary =
            path.with_file_name(format!(".asset-write-{}", crate::history::unique_id()));
        if let Err(error) = write_new(&temporary, bytes) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        // Rust's Windows rename uses replace-existing semantics. Keep the old
        // directory entry until the flushed candidate replaces it atomically;
        // unlinking it first created a process-crash window with no live file.
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        Ok(())
    } else if path.exists() {
        fs::remove_file(path).map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}
fn reject_hardcoded_source(document: &GraphDocument, path: &str) -> Result<(), String> {
    for (id, source) in &document.custom_sources {
        if source.contains(path) || source.contains(&path.replace('/', "\\\\")) {
            return Err(format!(
                "Rust Custom '{id}' contains a literal asset path; use ScriptContext::asset_path(ID) or update code before moving/deleting {path}"
            ));
        }
        if ["fs::", "File::", "Path::", "include_bytes!", "include_str!"]
            .iter()
            .any(|call| source.contains(call))
            && !source.contains("asset_path")
        {
            return Err(format!(
                "Rust Custom '{id}' accesses files without the asset ID API; computed paths cannot be analyzed safely before moving/deleting {path}. Use ScriptContext::asset_path(ID), or remove/update this dependency first."
            ));
        }
    }
    Ok(())
}

#[derive(Clone)]
struct FileLink {
    target: String,
    slot: LinkSlot,
}
#[derive(Clone)]
enum LinkSlot {
    Gltf {
        collection: String,
        index: usize,
    },
    Hlsl {
        line: usize,
        start: usize,
        end: usize,
    },
}
fn file_links(root: &Path, path: &str, bytes: &[u8]) -> Result<Vec<FileLink>, String> {
    match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "gltf" | "glb" => {
            let (json, _) = model_json(path, bytes)?;
            let mut links = vec![];
            for collection in ["buffers", "images"] {
                for (index, item) in json
                    .get(collection)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    if let Some(uri) = item.get("uri").and_then(Value::as_str) {
                        if uri.starts_with("data:") {
                            continue;
                        }
                        let target = relative_link(path, &decode_uri(uri)?)?;
                        resolve_relative(root, &target)?;
                        links.push(FileLink {
                            target,
                            slot: LinkSlot::Gltf {
                                collection: collection.into(),
                                index,
                            },
                        });
                    }
                }
            }
            if ["extensionsRequired", "extensionsUsed"]
                .iter()
                .any(|field| {
                    json.get(field)
                        .and_then(Value::as_array)
                        .is_some_and(|e| !e.is_empty())
                })
            {
                return Err(format!(
                    "{path}: glTF extensions are unsupported by safe asset relocation"
                ));
            }
            Ok(links)
        }
        "hlsl" | "hlsli" => hlsl_links(
            root,
            path,
            std::str::from_utf8(bytes).map_err(|e| e.to_string())?,
            true,
        ),
        _ => Ok(vec![]),
    }
}
fn relative_link(path: &str, uri: &str) -> Result<String, String> {
    let parent = Path::new(path).parent().unwrap_or(Path::new(""));
    normalize_relative(&parent.join(uri).to_string_lossy())
}
fn decode_uri(uri: &str) -> Result<String, String> {
    if uri.contains(':') || uri.starts_with('/') || uri.contains('?') || uri.contains('#') {
        return Err(format!("Unsupported external/remote dependency URI: {uri}"));
    }
    let mut result = vec![];
    let bytes = uri.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3).ok_or("Invalid URI escape")?)
                .map_err(|e| e.to_string())?;
            result.push(u8::from_str_radix(hex, 16).map_err(|e| e.to_string())?);
            i += 3;
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(result).map_err(|e| e.to_string())
}
fn encode_uri(path: &str) -> String {
    path.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn relative_from_file(file: &str, target: &str) -> String {
    let parent: Vec<_> = file
        .rsplit_once('/')
        .map_or("", |p| p.0)
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    let target: Vec<_> = target.split('/').collect();
    let common = parent
        .iter()
        .zip(&target)
        .take_while(|(a, b)| a == b)
        .count();
    std::iter::repeat_n("..", parent.len() - common)
        .chain(target[common..].iter().copied())
        .collect::<Vec<_>>()
        .join("/")
}
fn model_json(path: &str, bytes: &[u8]) -> Result<(Value, Option<Vec<u8>>), String> {
    if !path.to_ascii_lowercase().ends_with(".glb") {
        return serde_json::from_slice(bytes)
            .map(|j| (j, None))
            .map_err(|e| format!("{path}: {e}"));
    }
    if bytes.len() < 20
        || &bytes[..4] != b"glTF"
        || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != 2
        || u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize != bytes.len()
        || &bytes[16..20] != b"JSON"
    {
        return Err(format!("{path}: invalid GLB header"));
    }
    let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let end = 20usize
        .checked_add(length)
        .filter(|n| *n <= bytes.len())
        .ok_or("Invalid GLB JSON length")?;
    serde_json::from_slice(&bytes[20..end])
        .map(|j| (j, Some(bytes[end..].to_vec())))
        .map_err(|e| e.to_string())
}
fn encode_model(json: &Value, tail: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(json).map_err(|e| e.to_string())?;
    let Some(tail) = tail else { return Ok(bytes) };
    while bytes.len() % 4 != 0 {
        bytes.push(b' ');
    }
    let length = bytes.len() + tail.len() + 20;
    let length = u32::try_from(length).map_err(|_| "GLB exceeds 4 GiB")?;
    let mut result = b"glTF".to_vec();
    result.extend(2u32.to_le_bytes());
    result.extend(length.to_le_bytes());
    result.extend((bytes.len() as u32).to_le_bytes());
    result.extend(b"JSON");
    result.extend(bytes);
    result.extend(tail);
    Ok(result)
}
fn hlsl_links(
    root: &Path,
    path: &str,
    source: &str,
    use_parent: bool,
) -> Result<Vec<FileLink>, String> {
    let mut result = vec![];
    let mut block = false;
    for (line, text) in source.lines().enumerate() {
        // Preserve offsets while removing comments; escaped quotes stay intact.
        let mut visible = text.as_bytes().to_vec();
        let mut i = 0;
        let mut quoted = false;
        while i < visible.len() {
            if block {
                if visible.get(i..i + 2) == Some(b"*/") {
                    visible[i] = b' ';
                    visible[i + 1] = b' ';
                    i += 2;
                    block = false;
                } else {
                    visible[i] = b' ';
                    i += 1;
                }
            } else if !quoted && visible.get(i..i + 2) == Some(b"/*") {
                visible[i] = b' ';
                visible[i + 1] = b' ';
                i += 2;
                block = true;
            } else if !quoted && visible.get(i..i + 2) == Some(b"//") {
                visible[i..].fill(b' ');
                break;
            } else {
                if visible[i] == b'"' && (i == 0 || visible[i - 1] != b'\\') {
                    quoted = !quoted;
                }
                i += 1;
            }
        }
        let visible = String::from_utf8(visible).map_err(|e| e.to_string())?;
        let trimmed = visible.trim_start_matches([' ', '\t', '\u{feff}']);
        let Some(directive) = trimmed.strip_prefix('#') else {
            continue;
        };
        let Some(rest) = directive.trim_start().strip_prefix("include") else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) && !rest.starts_with(['"', '<']) {
            continue;
        }
        let rest = rest.trim_start();
        let close = match rest.as_bytes().first() {
            Some(b'"') => '"',
            Some(b'<') => '>',
            _ => return Err(format!("{path}: macro include cannot be relocated safely")),
        };
        let end = rest[1..].find(close).ok_or("Malformed HLSL include")? + 1;
        let name = rest[1..end].replace('\\', "/");
        if name.starts_with('/') || name.contains(':') {
            return Err(format!("{path}: absolute HLSL include is unsupported"));
        }
        let parent = Path::new(path).parent().unwrap_or(Path::new(""));
        let target = [
            (use_parent && close == '"').then_some(parent),
            Some(Path::new("assets/gpu")),
            Some(Path::new("")),
        ]
        .into_iter()
        .flatten()
        .filter_map(|p| normalize_relative(&p.join(&name).to_string_lossy()).ok())
        .find(|p| resolve_relative(root, p).is_ok())
        .ok_or_else(|| format!("{path}: missing dependency {name}"))?;
        let start = visible.len() - rest.len();
        result.push(FileLink {
            target,
            slot: LinkSlot::Hlsl {
                line,
                start,
                end: start + end + 1,
            },
        });
    }
    Ok(result)
}
fn rewrite_file_links(
    root: &Path,
    old_file: &str,
    new_file: &str,
    bytes: &[u8],
    old_target: &str,
    new_target: &str,
) -> Result<Vec<u8>, String> {
    let links = file_links(root, old_file, bytes)?;
    if links.is_empty() {
        return Ok(bytes.to_vec());
    }
    let model = matches!(AssetKind::for_path(old_file), AssetKind::Model);
    if model {
        let (mut json, tail) = model_json(old_file, bytes)?;
        let mut changed = false;
        for link in links {
            if let LinkSlot::Gltf { collection, index } = link.slot {
                let target = if same_path(&link.target, old_target) {
                    new_target
                } else {
                    &link.target
                };
                if old_file != new_file || target != link.target {
                    json[collection][index]["uri"] =
                        Value::String(encode_uri(&relative_from_file(new_file, target)));
                    changed = true;
                }
            }
        }
        if changed {
            encode_model(&json, tail)
        } else {
            Ok(bytes.to_vec())
        }
    } else {
        let source = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
        let mut lines: Vec<String> = source.split_inclusive('\n').map(str::to_owned).collect();
        let mut changed = false;
        for link in links {
            if let LinkSlot::Hlsl { line, start, end } = link.slot {
                let target = if same_path(&link.target, old_target) {
                    new_target
                } else {
                    &link.target
                };
                if old_file != new_file || target != link.target {
                    lines[line].replace_range(
                        start..end,
                        &format!("\"{}\"", relative_from_file(new_file, target)),
                    );
                    changed = true;
                }
            }
        }
        Ok(if changed {
            lines.concat().into_bytes()
        } else {
            bytes.to_vec()
        })
    }
}
fn rewrite_inline_includes(
    before: &GraphDocument,
    after: &mut GraphDocument,
    root: &Path,
    old: &str,
    new: &str,
) -> Result<(), String> {
    for (id, source) in &before.shader_sources {
        // Inline engine sources can depend on separately installed backend files.
        // Only project-resolvable literal includes participate in asset moves;
        // source IDs and other strings are never mistaken for filenames.
        let links = match hlsl_links(root, "inline.hlsl", source, false) {
            Ok(l) => l,
            Err(e) if e.contains("missing dependency") => continue,
            Err(e) => return Err(format!("Shader {id}: {e}")),
        };
        let mut lines: Vec<String> = source.split_inclusive('\n').map(str::to_owned).collect();
        let mut changed = false;
        for link in links {
            if same_path(&link.target, old)
                && let LinkSlot::Hlsl { line, start, end } = link.slot
            {
                lines[line].replace_range(start..end, &format!("<{new}>"));
                changed = true;
            }
        }
        if changed {
            after.shader_sources.insert(id.clone(), lines.concat());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (PathBuf, AssetWorkspace, GraphDocument) {
        use module_engine_core::{
            AuthoredObject, AuthoredScene, GameProject, Prefab, Transform, WorldObject,
        };
        let root =
            std::env::temp_dir().join(format!("asset-workspace-{}", crate::history::unique_id()));
        fs::create_dir_all(root.join("assets/models")).unwrap();
        fs::create_dir_all(root.join("assets/textures")).unwrap();
        let workspace = AssetWorkspace::new(&root, vec!["assets/gpu".into()]).unwrap();
        let mut doc = module_engine_core::demo_document(module_engine_core::WorldKind::Objects);
        let mut project = GameProject::empty();
        project.entry_scene = "arena".into();
        let mut objects = vec![];
        for id in [1, 2] {
            let mut object = AuthoredObject::new(WorldObject {
                id,
                parent: None,
                transform: Transform::default(),
                color: [255; 4],
            });
            if id == 2 {
                object.prefab = Some("asset-template".into());
            }
            objects.push(object);
        }
        project.scenes = BTreeMap::from([(
            "arena".into(),
            AuthoredScene {
                name: "Asset fixture".into(),
                objects,
            },
        )]);
        project.prefabs.insert(
            "asset-template".into(),
            Prefab {
                color: [80, 120, 160, 255],
                components: BTreeMap::from([("fixture.data".into(), json!({"value":1}))]),
            },
        );
        doc.game = Some(project);
        (root, workspace, doc)
    }
    fn id(doc: &GraphDocument, path: &str) -> String {
        AssetManifest::from_value(&doc.asset_manifest)
            .unwrap()
            .assets
            .into_iter()
            .find(|(_, e)| e.path == path)
            .unwrap()
            .0
    }
    fn write(root: &Path, path: &str, bytes: impl AsRef<[u8]>) {
        fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        fs::write(root.join(path), bytes).unwrap();
    }
    struct SourceContract;
    impl module_engine_core::ExecutableSystem for SourceContract {
        fn step(
            &mut self,
            _: &mut module_engine_core::SystemContext<'_>,
            _: &mut Value,
        ) -> Result<(), String> {
            Ok(())
        }
    }
    fn with_source_contract<F>(
        workspace: AssetWorkspace,
        document: &mut GraphDocument,
        schema: module_engine_core::ComponentSchema,
        validator: F,
    ) -> AssetWorkspace
    where
        F: Fn(
                &module_engine_core::GameEntity,
                &module_engine_core::ComponentValidationContext<'_>,
            ) -> Result<(), String>
            + Send
            + Sync
            + 'static,
    {
        use module_engine_core::{
            ComponentValidationContext, ExecutionGraph, SystemDescriptor, SystemNode,
            SystemRegistry, SystemStage,
        };
        let owner = schema.owner_module.clone();
        let component = schema.id.clone();
        let mut registry = SystemRegistry::builtin();
        registry
            .components_mut()
            .register_with_validator(schema, validator)
            .unwrap();
        registry
            .register(
                SystemDescriptor {
                    id: owner.clone(),
                    state_schema: "fixture/1".into(),
                    components: vec![component],
                    ports: vec![],
                    requires: vec![],
                },
                |_| Ok(Box::new(SourceContract)),
            )
            .unwrap();
        document.game.as_mut().unwrap().execution = ExecutionGraph {
            resources: vec![],
            stages: vec![SystemStage {
                name: "Source contract".into(),
                nodes: vec![SystemNode {
                    id: "source-contract".into(),
                    module: owner,
                    params: Value::Null,
                    bindings: BTreeMap::new(),
                    after: vec![],
                }],
            }],
        };
        let composition = module_engine_core::CompositionContext::new(
            module_engine_core::ModuleRegistry::builtin(),
            registry.clone(),
        )
        .unwrap();
        let snapshot = composition
            .capture_lock(
                document,
                document.engine_lock.as_ref().unwrap().profile.clone(),
                Some(module_engine_core::SdkIdentity::current()),
            )
            .unwrap();
        document.module_versions = snapshot.module_versions;
        document.engine_lock = Some(snapshot.engine_lock);
        workspace
            .with_composition(composition)
            .with_validator(move |candidate| {
                if let Some(game) = &candidate.game {
                    let modules = registry.installed_modules();
                    let active = registry.active_components(&game.execution)?;
                    let assets =
                        AssetManifest::from_value(&candidate.asset_manifest)?.asset_paths();
                    game.validate_with_context(
                        &registry,
                        &ComponentValidationContext {
                            scenes: &game.scenes,
                            modules: &modules,
                            asset_paths: Some(&assets),
                            active_components: Some(&active),
                        },
                    )?;
                }
                Ok(())
            })
    }
    #[test]
    fn batched_usages_walk_document_once_and_keep_all_references() {
        let (root, workspace, mut doc) = fixture();
        for index in 0..128 {
            write(&root, &format!("assets/textures/{index}.bin"), b"asset");
        }
        workspace.synchronize(&mut doc).unwrap();
        let manifest = AssetManifest::from_value(&doc.asset_manifest).unwrap();
        let mut registry = module_engine_core::ModuleRegistry::builtin();
        let mut descriptor = module_engine_core::ModuleDescriptor {
            id: "fixture.resources".into(),
            label: "Resources".into(),
            description: String::new(),
            inputs: vec![],
            outputs: vec![],
            properties: vec![],
        };
        for (index, (id, entry)) in manifest.assets.iter().enumerate() {
            descriptor
                .properties
                .push(module_engine_core::PropertySchema {
                    id: format!("resource-{index}"),
                    label: "Resource".into(),
                    scope: module_engine_core::PropertyScope::Global,
                    value_type: module_engine_core::PropertyType::AssetReference {
                        kinds: vec![],
                        optional: true,
                    },
                    default: Value::Null,
                    requires_modules: vec![],
                    requires_components: vec![],
                });
            doc.properties.active.global.insert(
                format!("resource-{index}"),
                json!(if index.is_multiple_of(2) {
                    asset_reference(id)
                } else {
                    entry.path.clone()
                }),
            );
        }
        registry.register_module(descriptor).unwrap();
        doc.engine
            .enabled_modules
            .insert("fixture.resources".into());
        let composition = module_engine_core::CompositionContext::new(
            registry,
            module_engine_core::SystemRegistry::builtin(),
        )
        .unwrap();
        // Enabling a newly installed source contribution also requires its
        // exact implementation pin in this already locked author document.
        composition
            .capture_lock(
                &doc,
                doc.engine_lock.as_ref().unwrap().profile.clone(),
                Some(module_engine_core::SdkIdentity::current()),
            )
            .unwrap()
            .apply_to(&mut doc);
        let workspace = workspace.with_composition(composition);
        USAGE_DOCUMENT_WALKS.with(|walks| walks.set(0));
        let started = std::time::Instant::now();
        let batched = workspace.usages_batch(&doc).unwrap();
        let batched_time = started.elapsed();
        assert_eq!(USAGE_DOCUMENT_WALKS.with(|walks| walks.get()), 1);
        assert_eq!(batched.len(), 128);
        assert!(batched.values().all(|usages| usages.len() == 1));
        USAGE_DOCUMENT_WALKS.with(|walks| walks.set(0));
        let started = std::time::Instant::now();
        for id in manifest.assets.keys() {
            assert_eq!(workspace.usages(&doc, id).unwrap(), batched[id]);
        }
        let repeated_time = started.elapsed();
        assert_eq!(USAGE_DOCUMENT_WALKS.with(|walks| walks.get()), 128);
        eprintln!(
            "128 asset reverse references: one batch {batched_time:?}, per-tile rebuilds {repeated_time:?}; full document walks 1 vs 128"
        );
        crate::history::remove_test_directory(&root);
    }

    fn typed_links_workspace(workspace: AssetWorkspace, doc: &mut GraphDocument) -> AssetWorkspace {
        use module_engine_core::{ComponentSchema, PropertySchema, PropertyScope, PropertyType};
        let defaults = json!({"id":null,"module":null,"slots":[{"path":null}],"archive":{"path":null},"text":""});
        let fields = ["id", "module", "slots.0.path", "archive.path", "text"]
            .into_iter()
            .map(|path| PropertySchema {
                id: format!("audit.links.{path}"),
                label: path.into(),
                scope: PropertyScope::Component {
                    component: "audit.links".into(),
                },
                value_type: if path == "text" {
                    PropertyType::Text { max_len: 1024 }
                } else {
                    PropertyType::AssetReference {
                        kinds: vec!["png".into()],
                        optional: true,
                    }
                },
                default: module_engine_core::component_field(&defaults, path)
                    .unwrap()
                    .clone(),
                requires_modules: vec![],
                requires_components: vec![],
            })
            .collect();
        with_source_contract(
            workspace,
            doc,
            ComponentSchema {
                id: "audit.links".into(),
                owner_module: "audit.links.owner".into(),
                version: "1".into(),
                label: "Typed asset links".into(),
                defaults,
                fields,
                requires_modules: vec![],
                requires_components: vec![],
            },
            |_, _| Ok(()),
        )
    }

    const DEFAULT_ASSET_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 11, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4, 0, 9,
        251, 3, 253, 251, 94, 107, 43, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    fn schema_default_workspace(
        workspace: AssetWorkspace,
        doc: &mut GraphDocument,
        defaults: Value,
        field_path: &str,
    ) -> AssetWorkspace {
        schema_default_workspace_with_optional(workspace, doc, defaults, field_path, false)
    }
    fn schema_default_workspace_with_optional(
        workspace: AssetWorkspace,
        doc: &mut GraphDocument,
        defaults: Value,
        field_path: &str,
        optional: bool,
    ) -> AssetWorkspace {
        use module_engine_core::{ComponentSchema, PropertySchema, PropertyScope, PropertyType};
        let field = PropertySchema {
            id: format!("audit.defaults.{field_path}"),
            label: "Default asset".into(),
            scope: PropertyScope::Component {
                component: "audit.defaults".into(),
            },
            value_type: PropertyType::AssetReference {
                kinds: vec!["png".into()],
                optional,
            },
            default: module_engine_core::component_field(&defaults, field_path)
                .unwrap()
                .clone(),
            requires_modules: vec![],
            requires_components: vec![],
        };
        with_source_contract(
            workspace,
            doc,
            ComponentSchema {
                id: "audit.defaults".into(),
                owner_module: "audit.defaults.owner".into(),
                version: "1".into(),
                label: "Schema default assets".into(),
                defaults,
                fields: vec![field],
                requires_modules: vec![],
                requires_components: vec![],
            },
            |_, _| Ok(()),
        )
    }

    #[test]
    fn schema_default_asset_usages_refuse_file_mutations_before_commit() {
        for source in ["local", "array", "prefab", "shallow-local"] {
            let (root, workspace, mut doc) = fixture();
            write(&root, "assets/textures/a.png", DEFAULT_ASSET_PNG);
            write(&root, "assets/textures/b.png", DEFAULT_ASSET_PNG);
            workspace.synchronize(&mut doc).unwrap();
            let old = id(&doc, "assets/textures/a.png");
            let replacement = id(&doc, "assets/textures/b.png");
            let (defaults, path, authored) = match source {
                "local" => (json!({"path":"assets/textures/a.png"}), "path", json!({})),
                "array" => (
                    json!({"slots":[{"path":"assets/textures/a.png"}]}),
                    "slots.0.path",
                    json!({"slots":[]}),
                ),
                _ => (
                    json!({"material":{"path":"assets/textures/a.png"}}),
                    "material.path",
                    json!({"material":{}}),
                ),
            };
            let game = doc.game.as_mut().unwrap();
            if source == "prefab" || source == "shallow-local" {
                game.prefabs
                    .get_mut("asset-template")
                    .unwrap()
                    .components
                    .insert(
                        "audit.defaults".into(),
                        if source == "shallow-local" {
                            json!({"material":{"path":"assets/textures/b.png"}})
                        } else {
                            authored.clone()
                        },
                    );
                if source == "shallow-local" {
                    game.scenes.get_mut("arena").unwrap().objects[1]
                        .components
                        .insert("audit.defaults".into(), authored);
                }
            } else {
                game.scenes.get_mut("arena").unwrap().objects[0]
                    .components
                    .insert("audit.defaults".into(), authored);
            }
            let workspace = schema_default_workspace(workspace, &mut doc, defaults, path);
            workspace.validate_candidate(&doc).unwrap();
            AssetManifest::from_value(&doc.asset_manifest)
                .unwrap()
                .verify(&root)
                .unwrap();
            let usages = workspace.usages(&doc, &old).unwrap();
            assert!(!usages.is_empty(), "{source}");
            assert!(
                usages.iter().all(|usage| {
                    usage.location.starts_with("schema-default:audit.defaults.")
                        && usage.reference == "assets/textures/a.png"
                }),
                "{source}: {usages:?}"
            );
            if source == "shallow-local" {
                assert_eq!(usages.len(), 1);
                assert!(usages[0].location.contains("/objects/1/components/"));
            }
            let project_path = root.join("project.json");
            crate::save_document_with_backup(&project_path, &doc).unwrap();
            let project_bytes = fs::read(&project_path).unwrap();
            let schema_before = workspace
                .composition
                .as_ref()
                .unwrap()
                .systems
                .components()
                .schema("audit.defaults")
                .unwrap()
                .clone();
            let moved = workspace
                .move_asset(&doc, &old, "assets/textures/moved.png")
                .unwrap_err();
            assert!(
                moved.contains("immutable installed schema default"),
                "{source}: {moved}"
            );
            for policy in [
                DeletePolicy::RefuseUsed,
                DeletePolicy::Detach,
                DeletePolicy::ReplaceWith(replacement.clone()),
            ] {
                let error = workspace.delete_asset(&doc, &old, policy).unwrap_err();
                assert!(
                    error.contains("schema-default:audit.defaults."),
                    "{source}: {error}"
                );
            }
            assert_eq!(
                fs::read(root.join("assets/textures/a.png")).unwrap(),
                DEFAULT_ASSET_PNG
            );
            assert_eq!(
                fs::read(root.join("assets/textures/b.png")).unwrap(),
                DEFAULT_ASSET_PNG
            );
            assert!(!root.join("assets/textures/moved.png").exists());
            assert!(!root.join(".asset-history").exists());
            assert!(!root.join(".asset-trash").exists());
            assert_eq!(fs::read(project_path).unwrap(), project_bytes);
            assert_eq!(
                workspace
                    .composition
                    .as_ref()
                    .unwrap()
                    .systems
                    .components()
                    .schema("audit.defaults")
                    .unwrap(),
                &schema_before
            );
            workspace.validate_candidate(&doc).unwrap();
            AssetManifest::from_value(&doc.asset_manifest)
                .unwrap()
                .verify(&root)
                .unwrap();
            crate::history::remove_test_directory(&root);
        }
    }

    #[test]
    fn schema_default_stable_id_survives_move_but_cannot_be_deleted_or_relinked() {
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/textures/a.png", DEFAULT_ASSET_PNG);
        write(&root, "assets/textures/b.png", DEFAULT_ASSET_PNG);
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/textures/a.png");
        let replacement = id(&doc, "assets/textures/b.png");
        doc.game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .insert("audit.defaults".into(), json!({}));
        let workspace = schema_default_workspace(
            workspace,
            &mut doc,
            json!({"path":asset_reference(&asset)}),
            "path",
        );
        workspace.validate_candidate(&doc).unwrap();
        assert_eq!(workspace.usages(&doc, &asset).unwrap().len(), 1);
        let operation = workspace
            .move_asset(&doc, &asset, "assets/textures/moved.png")
            .unwrap();
        workspace.validate_candidate(&operation.document).unwrap();
        assert_eq!(
            workspace.usages(&operation.document, &asset).unwrap()[0].reference,
            asset_reference(&asset)
        );
        assert_eq!(operation.document.game, doc.game);
        for policy in [
            DeletePolicy::RefuseUsed,
            DeletePolicy::Detach,
            DeletePolicy::ReplaceWith(replacement),
        ] {
            assert!(
                workspace
                    .delete_asset(&operation.document, &asset, policy)
                    .unwrap_err()
                    .contains("schema-default:audit.defaults.path")
            );
        }
        assert!(root.join("assets/textures/moved.png").exists());
        assert_eq!(
            operation.record.undo(&root, &operation.document).unwrap(),
            doc
        );
        crate::history::remove_test_directory(&root);
    }

    #[test]
    fn explicit_optional_null_suppresses_schema_default_during_move_and_delete() {
        for source in ["local", "prefab", "leaf-override", "aggregate-override"] {
            let (root, workspace, mut doc) = fixture();
            write(&root, "assets/textures/a.png", DEFAULT_ASSET_PNG);
            workspace.synchronize(&mut doc).unwrap();
            let asset = id(&doc, "assets/textures/a.png");
            let game = doc.game.as_mut().unwrap();
            let object_index = usize::from(source == "prefab");
            if source == "prefab" {
                game.prefabs
                    .get_mut("asset-template")
                    .unwrap()
                    .components
                    .insert("audit.defaults".into(), json!({"slots":[{"path":null}]}));
            } else {
                let object = &mut game.scenes.get_mut("arena").unwrap().objects[object_index];
                object.components.insert(
                    "audit.defaults".into(),
                    if source == "local" {
                        json!({"slots":[{"path":null}]})
                    } else {
                        json!({})
                    },
                );
                if source == "leaf-override" {
                    object
                        .overrides
                        .insert("audit.defaults.slots.0.path".into(), Value::Null);
                } else if source == "aggregate-override" {
                    object
                        .overrides
                        .insert("audit.defaults.slots.0".into(), json!({"path":null}));
                }
            }
            let workspace = schema_default_workspace_with_optional(
                workspace,
                &mut doc,
                json!({"slots":[{"path":"assets/textures/a.png"}]}),
                "slots.0.path",
                true,
            );
            let assert_resolved_null = |candidate: &GraphDocument| {
                workspace.validate_candidate(candidate).unwrap();
                let composition = workspace.composition.as_ref().unwrap();
                let activity = composition.activity(candidate).unwrap();
                let manifest = AssetManifest::from_value(&candidate.asset_manifest).unwrap();
                let paths = manifest.asset_paths();
                let game = candidate.game.as_ref().unwrap();
                let entities = game
                    .resolve_scene_with_context(
                        "arena",
                        &composition.systems,
                        &module_engine_core::ComponentValidationContext {
                            scenes: &game.scenes,
                            modules: &activity.modules,
                            asset_paths: Some(&paths),
                            active_components: Some(&activity.components),
                        },
                    )
                    .unwrap();
                assert_eq!(
                    module_engine_core::component_field(
                        &entities[object_index].components["audit.defaults"],
                        "slots.0.path",
                    ),
                    Some(&Value::Null),
                    "{source}"
                );
                manifest.verify(&root).unwrap();
            };
            assert_resolved_null(&doc);
            assert!(
                workspace.usages(&doc, &asset).unwrap().is_empty(),
                "{source}"
            );
            let schema_before = workspace
                .composition
                .as_ref()
                .unwrap()
                .systems
                .components()
                .schema("audit.defaults")
                .unwrap()
                .clone();
            let moved = workspace
                .move_asset(&doc, &asset, "assets/textures/moved.png")
                .unwrap();
            assert_resolved_null(&moved.document);
            assert!(
                workspace
                    .usages(&moved.document, &asset)
                    .unwrap()
                    .is_empty(),
                "{source}"
            );
            assert!(!root.join("assets/textures/a.png").exists());
            assert_eq!(
                fs::read(root.join("assets/textures/moved.png")).unwrap(),
                DEFAULT_ASSET_PNG
            );
            let deleted = workspace
                .delete_asset(&moved.document, &asset, DeletePolicy::RefuseUsed)
                .unwrap();
            assert_resolved_null(&deleted.document);
            assert!(!root.join("assets/textures/moved.png").exists());
            for candidate in [&moved.document, &deleted.document] {
                assert_eq!(candidate.game, doc.game);
                assert_eq!(candidate.engine_lock, doc.engine_lock);
                assert_eq!(candidate.engine_profile, doc.engine_profile);
                assert_eq!(candidate.module_versions, doc.module_versions);
            }
            assert_eq!(
                workspace
                    .composition
                    .as_ref()
                    .unwrap()
                    .systems
                    .components()
                    .schema("audit.defaults")
                    .unwrap(),
                &schema_before
            );
            let restored = deleted.record.undo(&root, &deleted.document).unwrap();
            assert_eq!(restored, moved.document);
            assert_resolved_null(&restored);
            assert_eq!(moved.record.undo(&root, &restored).unwrap(), doc);
            crate::history::remove_test_directory(&root);
        }
    }

    #[test]
    fn authored_and_override_asset_values_suppress_defaults_but_detach_cannot_reveal_deleted_asset()
    {
        for source in ["local", "prefab", "leaf-override", "aggregate-override"] {
            let (root, workspace, mut doc) = fixture();
            write(&root, "assets/textures/a.png", DEFAULT_ASSET_PNG);
            workspace.synchronize(&mut doc).unwrap();
            let asset = id(&doc, "assets/textures/a.png");
            let game = doc.game.as_mut().unwrap();
            if source == "prefab" {
                game.prefabs
                    .get_mut("asset-template")
                    .unwrap()
                    .components
                    .insert(
                        "audit.defaults".into(),
                        json!({"material":{"path":"assets/textures/a.png"}}),
                    );
            } else {
                let object = &mut game.scenes.get_mut("arena").unwrap().objects[0];
                object.components.insert(
                    "audit.defaults".into(),
                    if source == "local" {
                        json!({"material":{"path":"assets/textures/a.png"}})
                    } else {
                        json!({})
                    },
                );
                if source == "leaf-override" {
                    object.overrides.insert(
                        "audit.defaults.material.path".into(),
                        json!("assets/textures/a.png"),
                    );
                } else if source == "aggregate-override" {
                    object.overrides.insert(
                        "audit.defaults.material".into(),
                        json!({"path":"assets/textures/a.png"}),
                    );
                }
            }
            let workspace = schema_default_workspace(
                workspace,
                &mut doc,
                json!({"material":{"path":"assets/textures/a.png"}}),
                "material.path",
            );
            workspace.validate_candidate(&doc).unwrap();
            let usages = workspace.usages(&doc, &asset).unwrap();
            assert_eq!(usages.len(), 1, "{source}: {usages:?}");
            assert!(!usages[0].location.starts_with("schema-default:"));
            let error = workspace
                .delete_asset(&doc, &asset, DeletePolicy::Detach)
                .unwrap_err();
            if source == "aggregate-override" {
                // Scene resolution fills defaults before whole-aggregate
                // overrides. An empty override therefore hides the field;
                // semantic preflight must reject that required missing value.
                assert!(error.contains("audit.defaults.material.path"), "{error}");
            } else {
                assert!(
                    error.contains("immutable installed schema default"),
                    "{source}: {error}"
                );
            }
            assert!(root.join("assets/textures/a.png").exists());
            assert!(!root.join(".asset-history").exists());
            assert!(!root.join(".asset-trash").exists());
            let operation = workspace
                .move_asset(&doc, &asset, "assets/textures/moved.png")
                .unwrap();
            workspace.validate_candidate(&operation.document).unwrap();
            assert_eq!(
                workspace.usages(&operation.document, &asset).unwrap().len(),
                1
            );
            assert!(!root.join("assets/textures/a.png").exists());
            assert_eq!(
                operation.record.undo(&root, &operation.document).unwrap(),
                doc
            );
            crate::history::remove_test_directory(&root);
        }
    }

    #[test]
    fn inactive_or_unpinned_schema_defaults_do_not_claim_archived_attachments() {
        for mode in [
            "inactive",
            "missing-owner",
            "stale-owner",
            "missing-schema",
            "stale-schema",
        ] {
            let (root, workspace, mut doc) = fixture();
            write(&root, "assets/textures/a.png", DEFAULT_ASSET_PNG);
            workspace.synchronize(&mut doc).unwrap();
            let asset = id(&doc, "assets/textures/a.png");
            doc.game
                .as_mut()
                .unwrap()
                .scenes
                .get_mut("arena")
                .unwrap()
                .objects[0]
                .components
                .insert("audit.defaults".into(), json!({}));
            let workspace = schema_default_workspace(
                workspace,
                &mut doc,
                json!({"path":"assets/textures/a.png"}),
                "path",
            );
            match mode {
                "inactive" => doc.game.as_mut().unwrap().execution.stages.clear(),
                "missing-owner" => {
                    doc.module_versions.remove("audit.defaults.owner");
                }
                "stale-owner" => {
                    doc.module_versions
                        .insert("audit.defaults.owner".into(), "stale".into());
                }
                "missing-schema" => {
                    doc.engine_lock
                        .as_mut()
                        .unwrap()
                        .component_schemas
                        .remove("audit.defaults");
                }
                _ => {
                    doc.engine_lock
                        .as_mut()
                        .unwrap()
                        .component_schemas
                        .get_mut("audit.defaults")
                        .unwrap()
                        .version = "stale".into();
                }
            }
            assert!(workspace.usages(&doc, &asset).unwrap().is_empty(), "{mode}");
            // A source-only structural caller does not pretend the inactive or
            // mismatched schema supplied an effective reference.
            let workspace = AssetWorkspace {
                validator: None,
                ..workspace
            };
            let operation = workspace
                .move_asset(&doc, &asset, "assets/textures/moved.png")
                .unwrap();
            assert_eq!(operation.document.game, doc.game);
            assert_eq!(operation.document.engine_lock, doc.engine_lock);
            assert_eq!(operation.document.module_versions, doc.module_versions);
            crate::history::remove_test_directory(&root);
        }
    }

    #[test]
    fn typed_move_keeps_text_and_opaque_archives_and_persists_true_references() {
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/textures/a.png", b"a");
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/textures/a.png");
        let game = doc.game.as_mut().unwrap();
        game.prefabs.get_mut("asset-template").unwrap().components.insert("audit.links".into(), json!({
            "id":"assets/textures/a.png", "module":asset_reference(&asset), "slots":[{"path":"assets/textures/a.png"}], "text":"assets/textures/a.png"
        }));
        let object = &mut game.scenes.get_mut("arena").unwrap().objects[0];
        object.components.insert(
            "audit.links".into(),
            json!({"module":"assets/textures/a.png","text":"assets/textures/a.png"}),
        );
        object.components.insert(
            "unknown.payload".into(),
            json!({"id":"assets/textures/a.png","nested":[{"path":"assets/textures/a.png"}]}),
        );
        object
            .overrides
            .insert("audit.links.id".into(), json!("assets/textures/a.png"));
        object.overrides.insert(
            "unknown.payload.path".into(),
            json!("assets/textures/a.png"),
        );
        doc.properties
            .inactive
            .global
            .insert("audit.links.id".into(), json!("assets/textures/a.png"));
        doc.legacy_archive
            .insert("opaque".into(), json!({"module":"assets/textures/a.png"}));
        let workspace = typed_links_workspace(workspace, &mut doc);
        workspace.validate_candidate(&doc).unwrap();
        assert_eq!(workspace.usages(&doc, &asset).unwrap().len(), 5);
        let op = workspace
            .move_asset(&doc, &asset, "assets/textures/moved.png")
            .unwrap();
        workspace.validate_candidate(&op.document).unwrap();
        let game = op.document.game.as_ref().unwrap();
        let prefab = &game.prefabs["asset-template"].components["audit.links"];
        assert_eq!(prefab["id"], "assets/textures/moved.png");
        assert_eq!(prefab["module"], asset_reference(&asset));
        assert_eq!(prefab["slots"][0]["path"], "assets/textures/moved.png");
        assert_eq!(prefab["text"], "assets/textures/a.png");
        let object = &game.scenes["arena"].objects[0];
        assert_eq!(
            object.components["audit.links"]["module"],
            "assets/textures/moved.png"
        );
        assert_eq!(
            object.components["audit.links"]["text"],
            "assets/textures/a.png"
        );
        assert_eq!(
            object.overrides["audit.links.id"],
            "assets/textures/moved.png"
        );
        assert_eq!(
            object.components["unknown.payload"],
            doc.game.as_ref().unwrap().scenes["arena"].objects[0].components["unknown.payload"]
        );
        assert_eq!(
            object.overrides["unknown.payload.path"],
            "assets/textures/a.png"
        );
        assert_eq!(op.document.properties.inactive, doc.properties.inactive);
        assert_eq!(op.document.legacy_archive, doc.legacy_archive);
        assert_eq!(op.diagnostics.len(), 5);
        assert!(
            op.diagnostics
                .iter()
                .all(|message| !message.contains("/audit.links/text"))
        );
        let path = root.join("project.json");
        crate::save_document_with_backup(&path, &op.document).unwrap();
        let reopened = crate::read_document_strict(&path).unwrap();
        workspace.validate_candidate(&reopened).unwrap();
        assert_eq!(reopened, op.document);
        assert_eq!(op.record.undo(&root, &reopened).unwrap(), doc);
        crate::history::remove_test_directory(&root);
    }

    #[test]
    fn typed_delete_relinks_prefab_local_overrides_and_stable_ids_without_text_changes() {
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/textures/a.png", b"a");
        write(&root, "assets/textures/b.png", b"b");
        workspace.synchronize(&mut doc).unwrap();
        let old = id(&doc, "assets/textures/a.png");
        let new = id(&doc, "assets/textures/b.png");
        let game = doc.game.as_mut().unwrap();
        game.prefabs
            .get_mut("asset-template")
            .unwrap()
            .components
            .insert(
                "audit.links".into(),
                json!({"id":asset_reference(&old),"text":"assets/textures/a.png"}),
            );
        let object = &mut game.scenes.get_mut("arena").unwrap().objects[0];
        object.components.insert(
            "audit.links".into(),
            json!({"module":"assets/textures/a.png","slots":[{"path":asset_reference(&old)}]}),
        );
        object
            .overrides
            .insert("audit.links.id".into(), json!(asset_reference(&old)));
        let workspace = typed_links_workspace(workspace, &mut doc);
        assert!(
            workspace
                .delete_asset(&doc, &old, DeletePolicy::RefuseUsed)
                .unwrap_err()
                .contains("Asset is used")
        );
        let op = workspace
            .delete_asset(&doc, &old, DeletePolicy::ReplaceWith(new.clone()))
            .unwrap();
        workspace.validate_candidate(&op.document).unwrap();
        let game = op.document.game.as_ref().unwrap();
        assert_eq!(
            game.prefabs["asset-template"].components["audit.links"]["id"],
            asset_reference(&new)
        );
        assert_eq!(
            game.prefabs["asset-template"].components["audit.links"]["text"],
            "assets/textures/a.png"
        );
        let object = &game.scenes["arena"].objects[0];
        assert_eq!(
            object.components["audit.links"]["module"],
            asset_reference(&new)
        );
        assert_eq!(
            object.components["audit.links"]["slots"][0]["path"],
            asset_reference(&new)
        );
        assert_eq!(object.overrides["audit.links.id"], asset_reference(&new));
        assert!(op.diagnostics.is_empty());
        crate::save_document_with_backup(&root.join("project.json"), &op.document).unwrap();
        assert_eq!(
            crate::read_document_strict(&root.join("project.json")).unwrap(),
            op.document
        );
        let original = op.record.undo(&root, &op.document).unwrap();
        assert_eq!(original, doc);
        let detached = workspace
            .delete_asset(&original, &old, DeletePolicy::Detach)
            .unwrap();
        workspace.validate_candidate(&detached.document).unwrap();
        assert!(detached.document.game.as_ref().unwrap().scenes["arena"].objects[0].components["audit.links"]["slots"][0].get("path").is_none());
        assert!(
            !detached.document.game.as_ref().unwrap().scenes["arena"].objects[0]
                .overrides
                .contains_key("audit.links.id")
        );
        crate::history::remove_test_directory(&root);
    }

    #[test]
    fn known_but_inactive_or_stale_components_are_preserved_and_reported() {
        for stale in [false, true] {
            let (root, workspace, mut doc) = fixture();
            write(&root, "assets/textures/a.png", b"a");
            workspace.synchronize(&mut doc).unwrap();
            let asset = id(&doc, "assets/textures/a.png");
            doc.game
                .as_mut()
                .unwrap()
                .scenes
                .get_mut("arena")
                .unwrap()
                .objects[0]
                .components
                .insert("audit.links".into(), json!({"id":"assets/textures/a.png"}));
            let workspace = typed_links_workspace(workspace, &mut doc);
            if stale {
                doc.module_versions
                    .insert("audit.links.owner".into(), "archived-version".into());
            }
            doc.game.as_mut().unwrap().execution.stages.clear();
            workspace.validate_candidate(&doc).unwrap();
            let moved = workspace
                .move_asset(&doc, &asset, "assets/textures/new.png")
                .unwrap();
            assert_eq!(
                moved.document.game.as_ref().unwrap().scenes["arena"].objects[0].components["audit.links"]
                    ["id"],
                "assets/textures/a.png"
            );
            assert_eq!(moved.diagnostics.len(), 1);
            assert!(
                workspace
                    .usages(&moved.document, &asset)
                    .unwrap()
                    .is_empty()
            );
            crate::history::remove_test_directory(&root);
        }
    }

    #[test]
    fn source_api_preserves_missing_or_stale_implementation_pins_and_keeps_legacy_compatibility() {
        for pin in ["missing", "stale", "legacy-unpinned"] {
            let (root, workspace, mut doc) = fixture();
            write(&root, "assets/textures/a.png", b"a");
            workspace.synchronize(&mut doc).unwrap();
            let asset = id(&doc, "assets/textures/a.png");
            doc.game
                .as_mut()
                .unwrap()
                .scenes
                .get_mut("arena")
                .unwrap()
                .objects[0]
                .components
                .insert("audit.links".into(), json!({"id":"assets/textures/a.png"}));
            let workspace = typed_links_workspace(workspace, &mut doc);
            // The installed schema pin remains exact. The independent module
            // implementation pin must still be present in a locked document.
            let schema = workspace
                .composition
                .as_ref()
                .unwrap()
                .systems
                .components()
                .schema("audit.links")
                .unwrap();
            assert_eq!(
                doc.engine_lock.as_ref().unwrap().component_schemas["audit.links"],
                module_engine_core::SchemaPin::capture(schema).unwrap()
            );
            if pin == "stale" {
                doc.module_versions
                    .insert("audit.links.owner".into(), "archived-implementation".into());
            } else {
                doc.module_versions.remove("audit.links.owner");
            }
            if pin == "legacy-unpinned" {
                doc.engine_lock = None;
            }
            // The caller's game-only preflight deliberately does not validate
            // implementation pins; AssetWorkspace must protect the payload.
            workspace.validate_candidate(&doc).unwrap();
            let moved = workspace
                .move_asset(&doc, &asset, "assets/textures/new.png")
                .unwrap();
            let expected = if pin == "legacy-unpinned" {
                "assets/textures/new.png"
            } else {
                "assets/textures/a.png"
            };
            assert_eq!(
                moved.document.game.as_ref().unwrap().scenes["arena"].objects[0].components["audit.links"]
                    ["id"],
                expected,
                "{pin}"
            );
            assert_eq!(
                moved.diagnostics.len(),
                usize::from(pin != "legacy-unpinned")
            );
            crate::save_document_with_backup(&root.join("project.json"), &moved.document).unwrap();
            assert_eq!(
                crate::read_document_strict(&root.join("project.json")).unwrap(),
                moved.document
            );
            assert_eq!(moved.record.undo(&root, &moved.document).unwrap(), doc);
            crate::history::remove_test_directory(&root);
        }
    }

    #[test]
    fn opaque_longer_attachment_owns_override_before_shorter_registered_asset_field() {
        for inherited in [false, true] {
            let (root, workspace, mut doc) = fixture();
            write(&root, "assets/textures/a.png", b"a");
            workspace.synchronize(&mut doc).unwrap();
            let asset = id(&doc, "assets/textures/a.png");
            let game = doc.game.as_mut().unwrap();
            if inherited {
                game.prefabs
                    .get_mut("asset-template")
                    .unwrap()
                    .components
                    .insert(
                        "audit.links.archive".into(),
                        json!({"path":"assets/textures/a.png"}),
                    );
            }
            let object = &mut game.scenes.get_mut("arena").unwrap().objects[0];
            object
                .components
                .insert("audit.links".into(), json!({"id":"assets/textures/a.png"}));
            object.prefab = Some("asset-template".into());
            if !inherited {
                object.components.insert(
                    "audit.links.archive".into(),
                    json!({"path":"assets/textures/a.png"}),
                );
            }
            object.overrides.insert(
                "audit.links.archive.path".into(),
                json!("assets/textures/a.png"),
            );
            let workspace = typed_links_workspace(workspace, &mut doc);
            workspace.validate_candidate(&doc).unwrap();
            let moved = workspace
                .move_asset(&doc, &asset, "assets/textures/new.png")
                .unwrap();
            let object = &moved.document.game.as_ref().unwrap().scenes["arena"].objects[0];
            assert_eq!(
                object.components["audit.links"]["id"],
                "assets/textures/new.png"
            );
            assert_eq!(
                object.overrides["audit.links.archive.path"],
                "assets/textures/a.png"
            );
            assert!(
                moved
                    .diagnostics
                    .iter()
                    .any(|message| message.contains("/overrides/audit.links.archive.path"))
            );
            crate::history::remove_test_directory(&root);
        }
    }

    #[test]
    fn inactive_component_property_and_stale_mesh_are_not_mutated_by_delete() {
        use module_engine_core::{
            CompositionContext, ModuleDescriptor, ModuleRegistry, PropertySchema, PropertyScope,
            PropertyType, SystemRegistry,
        };
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/textures/a.png", b"a");
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/textures/a.png");
        let mut modules = ModuleRegistry::builtin();
        modules
            .register_module(ModuleDescriptor {
                id: "fixture.property".into(),
                label: "Property".into(),
                description: String::new(),
                inputs: vec![],
                outputs: vec![],
                properties: vec![PropertySchema {
                    id: "fixture.path".into(),
                    label: "Path".into(),
                    scope: PropertyScope::Component {
                        component: "inactive.component".into(),
                    },
                    value_type: PropertyType::AssetReference {
                        kinds: vec!["png".into()],
                        optional: true,
                    },
                    default: Value::Null,
                    requires_modules: vec![],
                    requires_components: vec![],
                }],
            })
            .unwrap();
        doc.engine.enabled_modules.insert("fixture.property".into());
        doc.module_versions
            .insert("scene.extract".into(), "opaque-old-version".into());
        doc.properties
            .active
            .components
            .entry(1)
            .or_default()
            .insert(
                "inactive.component".into(),
                BTreeMap::from([("fixture.path".into(), json!("assets/textures/a.png"))]),
            );
        doc.game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .insert("mesh".into(), json!({"opaque":"saved payload"}));
        let workspace = workspace
            .with_composition(CompositionContext::new(modules, SystemRegistry::builtin()).unwrap());
        let deleted = workspace
            .delete_asset(&doc, &asset, DeletePolicy::Detach)
            .unwrap();
        assert_eq!(deleted.document.properties, doc.properties);
        assert_eq!(
            deleted.document.game.as_ref().unwrap().scenes["arena"].objects[0].components["mesh"],
            json!({"opaque":"saved payload"})
        );
        assert_eq!(deleted.diagnostics.len(), 1);
        assert!(deleted.diagnostics[0].contains("/inactive.component/fixture.path"));
        crate::save_document_with_backup(&root.join("project.json"), &deleted.document).unwrap();
        assert_eq!(
            crate::read_document_strict(&root.join("project.json")).unwrap(),
            deleted.document
        );
        crate::history::remove_test_directory(&root);
    }

    #[test]
    fn replacing_asset_file_keeps_the_old_entry_until_publish_and_refuses_bad_target() {
        let (root, _, _) = fixture();
        let path = root.join("assets/file.bin");
        fs::write(&path, b"old").unwrap();
        write_current(&path, Some(b"new")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        let directory = root.join("assets/directory");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("owned.bin"), b"keep").unwrap();
        assert!(write_current(&directory, Some(b"new")).is_err());
        assert_eq!(fs::read(directory.join("owned.bin")).unwrap(), b"keep");
        assert!(!fs::read_dir(root.join("assets")).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".asset-write-")
        }));
        crate::history::remove_test_directory(&root);
    }

    #[test]
    fn failed_import_journal_cleans_only_its_own_partial_snapshots() {
        let (root, workspace, _) = fixture();
        write(&root, ".asset-history/other/record.json", b"keep");
        let error = workspace
            .with_import_journal("failed-import", || -> Result<(), String> {
                write_new(
                    &workspace.checked_target(".asset-history/failed-import/after/0.bin")?,
                    b"partial snapshot",
                )?;
                Err("injected second snapshot failure".into())
            })
            .unwrap_err();
        assert!(error.contains("injected second snapshot failure"));
        assert!(!root.join(".asset-history/failed-import").exists());
        assert_eq!(
            fs::read(root.join(".asset-history/other/record.json")).unwrap(),
            b"keep"
        );
        assert!(
            workspace
                .with_import_journal("other", || -> Result<(), String> {
                    panic!("existing journal must never be adopted")
                })
                .is_err()
        );
        assert_eq!(
            fs::read(root.join(".asset-history/other/record.json")).unwrap(),
            b"keep"
        );
        workspace
            .with_import_journal("successful-import", || {
                write_new(
                    &workspace.checked_target(".asset-history/successful-import/record.json")?,
                    b"saved",
                )
            })
            .unwrap();
        assert_eq!(
            fs::read(root.join(".asset-history/successful-import/record.json")).unwrap(),
            b"saved"
        );
        crate::history::remove_test_directory(&root);
    }

    #[test]
    fn asset_scan_ignores_only_reserved_export_staging_directories() {
        let (root, workspace, mut document) = fixture();
        write(
            &root,
            "assets/.module-engine-game-export-1-2/partial.bin",
            b"temporary",
        );
        write(
            &root,
            "assets/.module-engine-game-export-not-a-directory",
            b"user file",
        );
        write(
            &root,
            "assets/module-engine-game-export-user/keep.bin",
            b"user folder",
        );
        write(
            &root,
            "assets/.other-hidden-folder/keep.bin",
            b"hidden folder",
        );
        let assets = workspace.synchronize(&mut document).unwrap();
        let paths: BTreeSet<_> = assets.iter().map(|asset| asset.path.as_str()).collect();
        assert!(!paths.contains("assets/.module-engine-game-export-1-2/partial.bin"));
        assert!(paths.contains("assets/.module-engine-game-export-not-a-directory"));
        assert!(paths.contains("assets/module-engine-game-export-user/keep.bin"));
        assert!(paths.contains("assets/.other-hidden-folder/keep.bin"));
        fs::remove_file(root.join("assets/.module-engine-game-export-1-2/partial.bin")).unwrap();
        fs::remove_dir(root.join("assets/.module-engine-game-export-1-2")).unwrap();
        assert!(workspace.synchronize(&mut document).is_ok());
        crate::history::remove_test_directory(&root);
    }

    #[test]
    fn stable_ids_refresh_content_protected_files_and_missing_entries_are_explicit() {
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/textures/a.png", b"first");
        write(&root, "assets/gpu/engine.hlsl", b"#include MACRO");
        let info = workspace.synchronize(&mut doc).unwrap();
        assert_eq!(info.len(), 1);
        let asset = id(&doc, "assets/textures/a.png");
        let before = doc.asset_manifest.clone();
        write(&root, "assets/textures/a.png", b"changed");
        workspace.synchronize(&mut doc).unwrap();
        assert_eq!(asset, id(&doc, "assets/textures/a.png"));
        assert_ne!(before, doc.asset_manifest);
        let manifest = AssetManifest::from_value(&doc.asset_manifest).unwrap();
        assert_eq!(
            manifest
                .resolve_reference(&asset_reference(&asset))
                .unwrap(),
            "assets/textures/a.png"
        );
        assert_eq!(
            manifest.resolve_reference("assets/textures/a.png").unwrap(),
            "assets/textures/a.png"
        );
        fs::remove_file(root.join("assets/textures/a.png")).unwrap();
        assert!(workspace.synchronize(&mut doc).unwrap()[0].missing);
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn model_moves_preserve_external_dependencies_scene_prefab_and_archived_paths() {
        let (root, workspace, mut doc) = fixture();
        write(&root,"assets/models/a.gltf",serde_json::to_vec(&json!({"asset":{"version":"2.0"},"buffers":[{"uri":"buffer.bin","byteLength":3}],"images":[{"uri":"../textures/a.png"}]})).unwrap());
        write(&root, "assets/models/buffer.bin", b"abc");
        write(&root, "assets/textures/a.png", b"image");
        doc.game
            .as_mut()
            .unwrap()
            .prefabs
            .get_mut("asset-template")
            .unwrap()
            .components
            .insert("mesh".into(), json!({"path":"assets/models/a.gltf"}));
        doc.properties
            .inactive
            .global
            .insert("archived.resource".into(), json!("assets/models/a.gltf"));
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/models/a.gltf");
        let op = workspace
            .move_asset(&doc, &asset, "assets/renamed/model.gltf")
            .unwrap();
        assert!(!root.join("assets/models/a.gltf").exists());
        let json: Value =
            serde_json::from_slice(&fs::read(root.join("assets/renamed/model.gltf")).unwrap())
                .unwrap();
        assert_eq!(json["buffers"][0]["uri"], "../models/buffer.bin");
        assert_eq!(json["images"][0]["uri"], "../textures/a.png");
        assert_eq!(
            op.document.properties.inactive.global["archived.resource"],
            "assets/models/a.gltf"
        );
        assert!(
            op.diagnostics
                .iter()
                .any(|message| message.contains("archived.resource"))
        );
        assert_eq!(id(&op.document, "assets/renamed/model.gltf"), asset);
        let game = op.document.game.as_ref().unwrap();
        assert_eq!(
            game.prefabs["asset-template"].components["mesh"]["path"],
            "assets/renamed/model.gltf"
        );
        assert_eq!(
            game.resolve_scene("arena")
                .unwrap()
                .iter()
                .find(|entity| entity.object.id == 2)
                .unwrap()
                .components["mesh"]["path"],
            "assets/renamed/model.gltf"
        );
        let undone = op.record.undo(&root, &op.document).unwrap();
        assert_eq!(undone, doc);
        assert!(root.join("assets/models/a.gltf").exists());
        assert!(!root.join("assets/renamed/model.gltf").exists());
        let redone = op.record.redo(&root, &undone).unwrap();
        assert_eq!(redone, op.document);
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn moving_dependency_rewrites_percent_uri_hlsl_and_keeps_ids() {
        let (root, workspace, mut doc) = fixture();
        write(
            &root,
            "assets/models/a.gltf",
            serde_json::to_vec(
                &json!({"asset":{"version":"2.0"},"images":[{"uri":"../textures/a%20b.png"}]}),
            )
            .unwrap(),
        );
        write(&root, "assets/textures/a b.png", b"image");
        write(&root, "assets/shaders/header.hlsli", b"float x;");
        write(
            &root,
            "assets/shaders/filter.hlsl",
            b"// ignore #include MACRO\n# include \"header.hlsli\" // preserve comment\n",
        );
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/textures/a b.png");
        let op = workspace
            .move_asset(&doc, &asset, "assets/textures/changed.png")
            .unwrap();
        let model: Value =
            serde_json::from_slice(&fs::read(root.join("assets/models/a.gltf")).unwrap()).unwrap();
        assert_eq!(model["images"][0]["uri"], "../textures/changed.png");
        let header = id(&op.document, "assets/shaders/header.hlsli");
        let next = workspace
            .move_asset(&op.document, &header, "assets/headers/shared.hlsli")
            .unwrap();
        assert!(
            fs::read_to_string(root.join("assets/shaders/filter.hlsl"))
                .unwrap()
                .contains("\"../headers/shared.hlsli\" // preserve comment")
        );
        AssetManifest::from_value(&next.document.asset_manifest)
            .unwrap()
            .verify(&root)
            .unwrap();
        let restored = next.record.undo(&root, &next.document).unwrap();
        assert_eq!(restored, op.document);
        op.record.undo(&root, &restored).unwrap();
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn used_delete_requires_explicit_detach_and_trash_restore_preserves_id() {
        let (root, workspace, mut doc) = fixture();
        write(
            &root,
            "assets/models/a.glb",
            include_bytes!("../../../assets/models/demo_crystal.glb"),
        );
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/models/a.glb");
        doc.game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .insert("mesh".into(), json!({"path":asset_reference(&asset)}));
        assert_eq!(workspace.usages(&doc, &asset).unwrap().len(), 1);
        assert!(
            workspace
                .delete_asset(&doc, &asset, DeletePolicy::RefuseUsed)
                .is_err()
        );
        let op = workspace
            .delete_asset(&doc, &asset, DeletePolicy::Detach)
            .unwrap();
        assert!(!root.join("assets/models/a.glb").exists());
        assert!(
            !op.document.game.as_ref().unwrap().scenes["arena"].objects[0]
                .components
                .contains_key("mesh")
        );
        assert_eq!(workspace.list_deleted().unwrap().len(), 1);
        let restored = workspace
            .restore_deleted(&op.document, &op.record.id)
            .unwrap();
        assert_eq!(id(&restored.document, "assets/models/a.glb"), asset);
        assert!(workspace.list_deleted().unwrap().is_empty());
        restored.record.undo(&root, &restored.document).unwrap();
        assert_eq!(op.record.undo(&root, &op.document).unwrap(), doc);
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn replacements_rewrite_file_dependencies_and_document_ids_undo_together() {
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/textures/a.png", b"a");
        write(&root, "assets/textures/b.png", b"b");
        write(
            &root,
            "assets/models/a.gltf",
            serde_json::to_vec(
                &json!({"asset":{"version":"2.0"},"images":[{"uri":"../textures/a.png"}]}),
            )
            .unwrap(),
        );
        workspace.synchronize(&mut doc).unwrap();
        let old = id(&doc, "assets/textures/a.png");
        let new = id(&doc, "assets/textures/b.png");
        doc.game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .insert(
                "material".into(),
                json!({"base_color":[1.,1.,1.,1.],"base_color_texture":asset_reference(&old)}),
            );
        assert!(
            workspace
                .delete_asset(&doc, &old, DeletePolicy::Detach)
                .is_err()
        );
        let op = workspace
            .delete_asset(&doc, &old, DeletePolicy::ReplaceWith(new.clone()))
            .unwrap();
        assert_eq!(
            op.document.game.as_ref().unwrap().scenes["arena"].objects[0].components["material"]["base_color_texture"],
            asset_reference(&new)
        );
        let model: Value =
            serde_json::from_slice(&fs::read(root.join("assets/models/a.gltf")).unwrap()).unwrap();
        assert_eq!(model["images"][0]["uri"], "../textures/b.png");
        let manifest = AssetManifest::from_value(&op.document.asset_manifest).unwrap();
        assert!(
            manifest.assets[&id(&op.document, "assets/models/a.gltf")]
                .dependencies
                .contains(&new)
        );
        manifest.verify(&root).unwrap();
        op.record.undo(&root, &op.document).unwrap();
        assert_eq!(fs::read(root.join("assets/textures/a.png")).unwrap(), b"a");
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn unsafe_dependency_occupied_target_custom_literal_and_external_changes_do_not_overwrite() {
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/textures/a.png", b"a");
        write(&root, "assets/textures/b.png", b"b");
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/textures/a.png");
        assert!(
            workspace
                .move_asset(&doc, &asset, "assets/textures/b.png")
                .is_err()
        );
        assert!(
            workspace
                .move_asset(&doc, &asset, "../outside.png")
                .is_err()
        );
        doc.custom_sources.insert(
            "literal".into(),
            "std::fs::read(\"assets/textures/a.png\")".into(),
        );
        assert!(
            workspace
                .move_asset(&doc, &asset, "assets/new.png")
                .is_err()
        );
        doc.custom_sources.clear();
        write(
            &root,
            "assets/shaders/unsafe.hlsl",
            b"#include CHOSEN_HEADER",
        );
        let info = workspace.synchronize(&mut doc).unwrap();
        assert!(info.iter().any(|i| i.diagnostic.is_some()));
        assert!(
            workspace
                .move_asset(&doc, &asset, "assets/new.png")
                .is_err()
        );
        fs::remove_file(root.join("assets/shaders/unsafe.hlsl")).unwrap();
        let mut manifest = AssetManifest::from_value(&doc.asset_manifest).unwrap();
        manifest
            .assets
            .retain(|_, e| e.path != "assets/shaders/unsafe.hlsl");
        doc.asset_manifest = serde_json::to_value(manifest).unwrap();
        let op = workspace
            .move_asset(&doc, &asset, "assets/new.png")
            .unwrap();
        write(&root, "assets/new.png", b"external");
        assert!(op.record.undo(&root, &op.document).is_err());
        assert_eq!(fs::read(root.join("assets/new.png")).unwrap(), b"external");
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn import_and_new_folder_records_restore_files_even_without_document_change() {
        let (root, workspace, doc) = fixture();
        let folder = workspace.create_folder(&doc, "assets/new").unwrap();
        assert_eq!(folder.record.before, folder.record.after);
        folder.record.undo(&root, &folder.document).unwrap();
        assert!(!root.join("assets/new").exists());
        folder.record.redo(&root, &doc).unwrap();
        write(
            &root,
            "assets/new/a.glb",
            include_bytes!("../../../assets/models/demo_crystal.glb"),
        );
        let import = workspace
            .adopt_import(&doc, &["assets/new/a.glb".into()])
            .unwrap();
        let asset = id(&import.document, "assets/new/a.glb");
        let before = import.record.undo(&root, &import.document).unwrap();
        assert_eq!(before, doc);
        assert!(!root.join("assets/new/a.glb").exists());
        let after = import.record.redo(&root, &before).unwrap();
        assert_eq!(id(&after, "assets/new/a.glb"), asset);
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn glb_json_relocation_preserves_binary_tail_and_extension_dependencies_are_explicit() {
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/textures/picture.png", b"picture");
        let json = json!({"asset":{"version":"2.0"},"images":[{"uri":"../textures/picture.png"}]});
        let tail = [4u32.to_le_bytes().as_slice(), b"BIN\0", b"data"].concat();
        let original = encode_model(&json, Some(tail.clone())).unwrap();
        write(&root, "assets/models/external.glb", &original);
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/models/external.glb");
        let op = workspace
            .move_asset(&doc, &asset, "assets/elsewhere/deep/external.glb")
            .unwrap();
        let bytes = fs::read(root.join("assets/elsewhere/deep/external.glb")).unwrap();
        let (moved, actual_tail) = model_json("moved.glb", &bytes).unwrap();
        assert_eq!(moved["images"][0]["uri"], "../../textures/picture.png");
        assert_eq!(actual_tail, Some(tail));
        op.record.undo(&root, &op.document).unwrap();
        assert_eq!(
            fs::read(root.join("assets/models/external.glb")).unwrap(),
            original
        );
        write(
            &root,
            "assets/models/extension.gltf",
            serde_json::to_vec(
                &json!({"asset":{"version":"2.0"},"extensionsUsed":["CUSTOM_external_data"]}),
            )
            .unwrap(),
        );
        let info = workspace.synchronize(&mut doc).unwrap();
        assert!(
            info.iter()
                .any(|i| i.path.ends_with("extension.gltf") && i.diagnostic.is_some())
        );
        assert!(
            workspace
                .move_asset(&doc, &asset, "assets/renamed.glb")
                .is_err()
        );
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn creating_first_asset_folder_undo_removes_only_its_own_empty_directories() {
        let root = std::env::temp_dir().join(format!(
            "first-asset-folder-{}",
            crate::history::unique_id()
        ));
        fs::create_dir_all(&root).unwrap();
        let workspace = AssetWorkspace::new(&root, Vec::<String>::new()).unwrap();
        let doc = module_engine_core::demo_document(module_engine_core::WorldKind::Objects);
        let op = workspace.create_folder(&doc, "assets/new").unwrap();
        op.record.undo(&root, &op.document).unwrap();
        assert!(!root.join("assets").exists());
        op.record.redo(&root, &doc).unwrap();
        write(&root, "assets/new/external.bin", b"external");
        assert!(op.record.undo(&root, &op.document).is_err());
        assert_eq!(
            fs::read(root.join("assets/new/external.bin")).unwrap(),
            b"external"
        );
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn source_required_resource_contract_rejects_detach_before_files_change() {
        use module_engine_core::{ComponentSchema, PropertySchema, PropertyScope, PropertyType};
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/textures/a.png", b"image");
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/textures/a.png");
        let object = doc
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects
            .iter_mut()
            .find(|object| object.object.id == 1)
            .unwrap();
        object.components.insert(
            "source.required-resource".into(),
            json!({"path":"assets/textures/a.png"}),
        );
        // This source-owned contract requires a nonempty value after defaults;
        // detaching its resource must reject before any file/journal changes.
        let schema = ComponentSchema {
            id: "source.required-resource".into(),
            owner_module: "source.required-resource-owner".into(),
            version: "required-resource/1".into(),
            label: "Required resource".into(),
            defaults: json!({"path":null}),
            fields: vec![PropertySchema {
                id: "source.required-resource.path".into(),
                label: "Path".into(),
                scope: PropertyScope::Component {
                    component: "source.required-resource".into(),
                },
                value_type: PropertyType::AssetReference {
                    kinds: vec!["png".into()],
                    optional: true,
                },
                default: Value::Null,
                requires_modules: vec![],
                requires_components: vec![],
            }],
            requires_modules: vec![],
            requires_components: vec![],
        };
        let workspace = with_source_contract(workspace, &mut doc, schema, |entity, _| {
            if entity.components["source.required-resource"]["path"]
                .as_str()
                .is_none_or(str::is_empty)
            {
                return Err("Source resource path must be nonempty".into());
            }
            Ok(())
        });
        doc.game.as_ref().unwrap().validate_scene_data().unwrap();
        workspace.validate_candidate(&doc).unwrap();
        let error = workspace
            .delete_asset(&doc, &asset, DeletePolicy::Detach)
            .unwrap_err();
        assert!(error.contains("Source resource path must be nonempty"));
        assert!(root.join("assets/textures/a.png").is_file());
        assert_eq!(
            fs::read(root.join("assets/textures/a.png")).unwrap(),
            b"image"
        );
        assert!(!root.join(".asset-history").exists());
        assert!(workspace.list_deleted().unwrap().is_empty());
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn external_source_validator_rejects_import_and_remains_on_later_operations() {
        use module_engine_core::{ComponentSchema, PropertySchema, PropertyScope, PropertyType};
        let (root, workspace, mut doc) = fixture();
        doc.game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .insert("external.import-policy".into(), json!({}));
        let workspace = with_source_contract(
            workspace,
            &mut doc,
            ComponentSchema {
                id: "external.import-policy".into(),
                owner_module: "external.asset-policy".into(),
                version: "1".into(),
                label: "Source import approval".into(),
                defaults: json!({"approved":false}),
                fields: vec![PropertySchema {
                    id: "external.import-policy.approved".into(),
                    label: "Approved".into(),
                    scope: PropertyScope::Component {
                        component: "external.import-policy".into(),
                    },
                    value_type: PropertyType::Boolean,
                    default: json!(false),
                    requires_modules: vec![],
                    requires_components: vec![],
                }],
                requires_modules: vec![],
                requires_components: vec![],
            },
            |entity, context| {
                if context.asset_paths.is_some_and(|assets| !assets.is_empty())
                    && entity.components["external.import-policy"]["approved"] != json!(true)
                {
                    return Err("External source requires asset approval".into());
                }
                Ok(())
            },
        );
        workspace.validate_candidate(&doc).unwrap();
        write(&root, "assets/imported.bin", b"imported");
        let files = vec!["assets/imported.bin".into()];
        let error = workspace.adopt_import(&doc, &files).unwrap_err();
        assert!(error.contains("External source requires asset approval"));
        assert_eq!(fs::read(root.join(&files[0])).unwrap(), b"imported");
        assert!(!root.join(".asset-history").exists());
        assert!(workspace.list_deleted().unwrap().is_empty());

        doc.game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .get_mut("external.import-policy")
            .unwrap()["approved"] = json!(true);
        let mut accepted = workspace.adopt_import(&doc, &files).unwrap().document;
        let asset = id(&accepted, &files[0]);
        accepted
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects[0]
            .components
            .get_mut("external.import-policy")
            .unwrap()["approved"] = json!(false);
        let error = workspace
            .move_asset(&accepted, &asset, "assets/moved.bin")
            .unwrap_err();
        assert!(error.contains("External source requires asset approval"));
        assert_eq!(fs::read(root.join(&files[0])).unwrap(), b"imported");
        assert!(!root.join("assets/moved.bin").exists());
        assert_eq!(
            fs::read_dir(root.join(".asset-history")).unwrap().count(),
            1
        );
        assert!(workspace.list_deleted().unwrap().is_empty());
        crate::history::remove_test_directory(&root);
    }
    #[cfg(windows)]
    #[test]
    fn dependency_write_failure_rolls_back_created_destination_and_keeps_originals() {
        let (root, workspace, mut doc) = fixture();
        write(&root, "assets/shaders/header.hlsli", b"float x;\n");
        let filter = b"#include \"header.hlsli\"\n";
        write(&root, "assets/shaders/filter.hlsl", filter);
        workspace.synchronize(&mut doc).unwrap();
        let asset = id(&doc, "assets/shaders/header.hlsli");
        let file = root.join("assets/shaders/filter.hlsl");
        let original_permissions = fs::metadata(&file).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions).unwrap();
        assert!(
            workspace
                .move_asset(&doc, &asset, "assets/headers/new.hlsli")
                .is_err()
        );
        assert!(root.join("assets/shaders/header.hlsli").is_file());
        assert!(!root.join("assets/headers/new.hlsli").exists());
        assert_eq!(fs::read(&file).unwrap(), filter);
        fs::set_permissions(file, original_permissions).unwrap();
        AssetManifest::from_value(&doc.asset_manifest)
            .unwrap()
            .verify(&root)
            .unwrap();
        crate::history::remove_test_directory(&root);
    }
}
