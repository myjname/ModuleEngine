use module_engine_core::{GraphDocument, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    /// Stable within this history, including after capacity eviction or branching.
    pub id: u64,
    pub label: String,
    pub document: GraphDocument,
}
/// Authoring snapshots; one record is one completed gesture/transaction.
pub struct SnapshotHistory {
    entries: Vec<HistoryEntry>,
    cursor: usize,
    capacity: usize,
    next_id: u64,
}
impl SnapshotHistory {
    pub fn new(initial: GraphDocument, capacity: usize) -> Self {
        Self {
            entries: vec![HistoryEntry {
                id: 0,
                label: "Initial".into(),
                document: initial,
            }],
            cursor: 0,
            capacity: capacity.max(1),
            next_id: 1,
        }
    }
    pub fn record(&mut self, label: impl Into<String>, document: &GraphDocument) -> bool {
        if self.current() == document {
            return false;
        }
        self.record_transaction(label, document);
        true
    }
    /// File transactions may change the filesystem without changing JSON.
    /// They still need an explicit Undo boundary with a stable identity.
    pub fn record_transaction(
        &mut self,
        label: impl Into<String>,
        document: &GraphDocument,
    ) -> u64 {
        self.entries.truncate(self.cursor + 1);
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(HistoryEntry {
            id,
            label: label.into(),
            document: document.clone(),
        });
        if self.entries.len() > self.capacity + 1 {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len() - 1;
        id
    }
    pub fn current_id(&self) -> u64 {
        self.entries[self.cursor].id
    }
    pub fn redo_id(&self) -> Option<u64> {
        self.entries.get(self.cursor + 1).map(|entry| entry.id)
    }
    pub fn current(&self) -> &GraphDocument {
        &self.entries[self.cursor].document
    }
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }
    /// Adopt first-time resource metadata without making Undo remove stable IDs.
    /// Only snapshots with the exact old map are rebased; scene/code edits,
    /// record identities, cursor and redo branch remain unchanged.
    pub fn rebase_asset_manifest(&mut self, before: &serde_json::Value, after: &serde_json::Value) {
        for entry in &mut self.entries {
            if entry.document.asset_manifest == *before {
                entry.document.asset_manifest = after.clone();
            }
        }
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }
    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }
    pub fn undo(&mut self) -> Option<GraphDocument> {
        if !self.can_undo() {
            return None;
        }
        self.cursor -= 1;
        Some(self.current().clone())
    }
    pub fn redo(&mut self) -> Option<GraphDocument> {
        if !self.can_redo() {
            return None;
        }
        self.cursor += 1;
        Some(self.current().clone())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub id: String,
    pub label: String,
    pub created_ms: u64,
    pub document: GraphDocument,
}
pub struct CheckpointStore {
    root: PathBuf,
}
impl CheckpointStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn save(
        &self,
        label: impl Into<String>,
        document: &GraphDocument,
    ) -> Result<Checkpoint, String> {
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let checkpoint = Checkpoint {
            id: unique_id(),
            label: label.into(),
            created_ms: now_ms(),
            document: document.clone(),
        };
        write_new_json(
            &self.root.join(format!("{}.json", checkpoint.id)),
            &checkpoint,
        )?;
        Ok(checkpoint)
    }
    pub fn list(&self) -> Result<Vec<Checkpoint>, String> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut result = vec![];
        for entry in fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|x| x == "json") {
                result.push(read_json::<Checkpoint>(&path)?);
            }
        }
        result.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
        Ok(result)
    }
    pub fn load(&self, id: &str) -> Result<GraphDocument, String> {
        if id.is_empty()
            || !id
                .bytes()
                .all(|x| x.is_ascii_alphanumeric() || x == b'-' || x == b'_')
        {
            return Err("Invalid checkpoint identifier".into());
        }
        Ok(read_json::<Checkpoint>(&self.root.join(format!("{id}.json")))?.document)
    }
}
#[derive(Clone, Debug)]
pub struct MigratedDocument {
    pub document: GraphDocument,
    pub source_schema: u32,
    pub changed: bool,
}
/// Migration changes only the in-memory document; explicit save keeps a backup.
pub fn load_document_migrating(path: &Path) -> Result<MigratedDocument, String> {
    let mut document: GraphDocument = read_json(path)?;
    let source_schema = document.schema_version;
    if !(1..=SCHEMA_VERSION).contains(&source_schema) {
        return Err(format!("Unsupported schema {source_schema}"));
    }
    document.schema_version = SCHEMA_VERSION;
    Ok(MigratedDocument {
        document,
        source_schema,
        changed: source_schema != SCHEMA_VERSION,
    })
}
/// Original survives serialization/write failures; replacing an existing file keeps a unique backup.
pub fn save_document_with_backup(
    path: &Path,
    document: &GraphDocument,
) -> Result<Option<PathBuf>, String> {
    let bytes = serde_json::to_vec_pretty(document).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = path.with_extension(format!("tmp-{}", unique_id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    let backup = if path.exists() {
        let backup = path.with_extension(format!("backup-{}", unique_id()));
        fs::copy(path, &backup).map_err(|e| e.to_string())?;
        Some(backup)
    } else {
        None
    };
    fs::rename(&temporary, path)
        .map_err(|e| format!("{e}; new document retained at {}", temporary.display()))?;
    Ok(backup)
}
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}
pub(crate) fn write_new_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
pub(crate) fn unique_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        now_ms(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
pub(crate) fn remove_test_directory(path: &Path) {
    let temporary = std::env::temp_dir().canonicalize().unwrap();
    let target = path.canonicalize().unwrap();
    assert!(target.starts_with(&temporary) && target != temporary);
    fs::remove_dir_all(target).unwrap();
}
#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{WorldKind, demo_document};
    #[test]
    fn undo_redo_restores_nested_graph_and_source_and_truncates_branch() {
        let initial = demo_document(WorldKind::Objects);
        let mut history = SnapshotHistory::new(initial.clone(), 3);
        let mut a = initial.clone();
        a.custom_sources.insert("speed".into(), "fn broken(".into());
        a.root.nodes[2].subgraph.as_mut().unwrap().nodes[1].label = "Changed pass".into();
        assert!(history.record("edit source + nested pass", &a));
        assert!(!history.record("same", &a));
        assert_eq!(history.undo(), Some(initial.clone()));
        assert_eq!(history.redo(), Some(a));
        history.undo();
        let mut b = initial;
        b.name = "other branch".into();
        history.record("branch", &b);
        assert!(!history.can_redo());
        assert_eq!(history.current(), &b);
    }
    #[test]
    fn checkpoints_and_migration_preserve_original_bytes() {
        let root = std::env::temp_dir().join(format!("module-project-{}", unique_id()));
        fs::create_dir_all(&root).unwrap();
        let mut original = demo_document(WorldKind::Ecs);
        original.schema_version = 1;
        let path = root.join("project.json");
        let bytes = serde_json::to_vec(&original).unwrap();
        fs::write(&path, &bytes).unwrap();
        let migrated = load_document_migrating(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let backup = save_document_with_backup(&path, &migrated.document)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(backup).unwrap(), bytes);
        let store = CheckpointStore::new(root.join("history"));
        let a = store.save("before", &original).unwrap();
        let mut edited = original.clone();
        edited.name = "after".into();
        store.save("after", &edited).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
        assert_eq!(store.load(&a.id).unwrap(), original);
        assert!(store.load("../project").is_err());
        remove_test_directory(&root);
    }
    #[test]
    fn identical_document_file_transactions_keep_identity_across_eviction_and_branches() {
        let document = demo_document(WorldKind::Objects);
        let mut history = SnapshotHistory::new(document.clone(), 2);
        let first = history.record_transaction("folder", &document);
        let second = history.record_transaction("second folder", &document);
        let third = history.record_transaction("third folder", &document);
        assert_eq!(history.current_id(), third);
        assert_eq!(history.entries()[0].id, first);
        assert_eq!(history.undo(), Some(document.clone()));
        assert_eq!(history.current_id(), second);
        assert_eq!(history.redo_id(), Some(third));
        let branch = history.record_transaction("other folder", &document);
        assert!(branch > third);
        assert!(!history.can_redo());
        assert!(!history.entries().iter().any(|entry| entry.id == third));
    }
    #[test]
    fn initial_asset_metadata_rebase_preserves_scene_undo_and_redo_without_new_entry() {
        let mut initial = demo_document(WorldKind::Objects);
        initial.game = Some(module_engine_core::GameProject::demo());
        let scene = initial.game.as_ref().unwrap().entry_scene.clone();
        let original_position = initial.game.as_ref().unwrap().scenes[&scene].objects[0]
            .object
            .transform
            .translation;
        let mut history = SnapshotHistory::new(initial.clone(), 10);
        let mut moved = initial.clone();
        moved
            .game
            .as_mut()
            .unwrap()
            .scenes
            .get_mut(&scene)
            .unwrap()
            .objects[0]
            .object
            .transform
            .translation[0] += 3.0;
        history.record("Move object", &moved);
        let move_id = history.current_id();
        history.undo();
        let cursor = history.cursor();
        let manifest = serde_json::json!({"schema_version":1,"assets":{"stable":{"path":"assets/model.glb","sha256":"pin","dependencies":[]}}});
        history.rebase_asset_manifest(&initial.asset_manifest, &manifest);
        assert_eq!(history.entries().len(), 2);
        assert_eq!(history.cursor(), cursor);
        assert_eq!(history.redo_id(), Some(move_id));
        let redone = history.redo().unwrap();
        assert_eq!(redone.asset_manifest, manifest);
        assert_eq!(
            redone.game.unwrap().scenes[&scene].objects[0]
                .object
                .transform
                .translation[0],
            original_position[0] + 3.0
        );
        let undone = history.undo().unwrap();
        assert_eq!(undone.asset_manifest, manifest);
        assert_eq!(
            undone.game.unwrap().scenes[&scene].objects[0]
                .object
                .transform
                .translation,
            original_position
        );
        assert!(
            !history.can_undo(),
            "initial metadata is not another user gesture"
        );
    }
    #[test]
    fn metadata_rebase_does_not_replace_a_different_indexed_snapshot() {
        let initial = demo_document(WorldKind::Objects);
        let mut history = SnapshotHistory::new(initial.clone(), 10);
        let other =
            serde_json::json!({"schema_version":1,"assets":{"other":{"path":"assets/other.glb"}}});
        let mut indexed = initial.clone();
        indexed.asset_manifest = other.clone();
        history.record("different index", &indexed);
        let manifest =
            serde_json::json!({"schema_version":1,"assets":{"stable":{"path":"assets/model.glb"}}});
        history.rebase_asset_manifest(&initial.asset_manifest, &manifest);
        assert_eq!(history.current().asset_manifest, other);
        assert_eq!(history.undo().unwrap().asset_manifest, manifest);
    }
}
