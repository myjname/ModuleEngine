use module_engine_core::GraphDocument;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DocumentChange {
    pub path: String,
    pub kind: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MergeConflict {
    pub path: String,
    pub base: Option<Value>,
    pub ours: Option<Value>,
    pub theirs: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeResult {
    pub document: GraphDocument,
    pub conflicts: Vec<MergeConflict>,
}
pub enum ConflictChoice {
    Base,
    Ours,
    Theirs,
    Custom(Option<Value>),
}
impl MergeResult {
    pub fn is_resolved(&self) -> bool {
        self.conflicts.is_empty()
    }
    /// Persist both sides; existing conflict reports are never overwritten.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        crate::history::write_new_json(path, self)
    }
    pub fn resolve(&mut self, index: usize, choice: ConflictChoice) -> Result<(), String> {
        let conflict = self.conflicts.get(index).ok_or("Unknown conflict")?;
        let value = match choice {
            ConflictChoice::Base => conflict.base.clone(),
            ConflictChoice::Ours => conflict.ours.clone(),
            ConflictChoice::Theirs => conflict.theirs.clone(),
            ConflictChoice::Custom(v) => v,
        };
        let mut normalized = normalize(&self.document)?;
        let parts: Vec<String> = conflict
            .path
            .split('/')
            .skip(1)
            .map(|p| p.replace("~1", "/").replace("~0", "~"))
            .collect();
        let (last, parents) = parts.split_last().ok_or("Invalid conflict path")?;
        let mut target = &mut normalized;
        for part in parents {
            target = target
                .get_mut(part)
                .ok_or("Conflict parent was removed by another resolution")?;
        }
        let object = target
            .as_object_mut()
            .ok_or("Conflict parent is not an object")?;
        if let Some(v) = value {
            object.insert(last.clone(), v);
        } else {
            object.remove(last);
        }
        let document = denormalize(normalized)?;
        self.document = document;
        self.conflicts.remove(index);
        Ok(())
    }
}
pub fn diff_documents(
    base: &GraphDocument,
    changed: &GraphDocument,
) -> Result<Vec<DocumentChange>, String> {
    let (a, b) = (normalize(base)?, normalize(changed)?);
    let mut result = vec![];
    diff_value("", Some(&a), Some(&b), &mut result);
    Ok(result)
}
pub fn merge_documents(
    base: &GraphDocument,
    ours: &GraphDocument,
    theirs: &GraphDocument,
) -> Result<MergeResult, String> {
    let (base, ours, theirs) = (normalize(base)?, normalize(ours)?, normalize(theirs)?);
    let mut conflicts = vec![];
    let result = merge_value("", Some(&base), Some(&ours), Some(&theirs), &mut conflicts)
        .ok_or("Merge removed the document")?;
    Ok(MergeResult {
        document: denormalize(result)?,
        conflicts,
    })
}
fn segment(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}
fn path_join(path: &str, key: &str) -> String {
    format!("{path}/{}", segment(key))
}
fn kind(path: &str) -> String {
    if path.starts_with("/custom_sources/") || path.starts_with("/shader_sources/") {
        "source"
    } else if path.contains("/edges/") {
        "connection"
    } else if path.contains("/nodes/") {
        "node"
    } else {
        "project"
    }
    .into()
}
fn keys(values: &[Option<&Value>]) -> BTreeSet<String> {
    values
        .iter()
        .filter_map(|v| v.and_then(Value::as_object))
        .flat_map(|m| m.keys().cloned())
        .collect()
}
fn diff_value(path: &str, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<DocumentChange>) {
    if a == b {
        return;
    }
    if a.is_some_and(Value::is_object) && b.is_some_and(Value::is_object) {
        for key in keys(&[a, b]) {
            diff_value(
                &path_join(path, &key),
                a.and_then(|v| v.get(&key)),
                b.and_then(|v| v.get(&key)),
                out,
            );
        }
        return;
    }
    out.push(DocumentChange {
        path: path.into(),
        kind: kind(path),
        before: a.cloned(),
        after: b.cloned(),
    });
}
fn merge_value(
    path: &str,
    base: Option<&Value>,
    ours: Option<&Value>,
    theirs: Option<&Value>,
    conflicts: &mut Vec<MergeConflict>,
) -> Option<Value> {
    if ours == theirs {
        return ours.cloned();
    }
    if ours == base {
        return theirs.cloned();
    }
    if theirs == base {
        return ours.cloned();
    }
    if ours.is_some_and(Value::is_object)
        && theirs.is_some_and(Value::is_object)
        && base.is_some_and(Value::is_object)
    {
        let mut map = Map::new();
        for key in keys(&[base, ours, theirs]) {
            if let Some(value) = merge_value(
                &path_join(path, &key),
                base.and_then(|v| v.get(&key)),
                ours.and_then(|v| v.get(&key)),
                theirs.and_then(|v| v.get(&key)),
                conflicts,
            ) {
                map.insert(key, value);
            }
        }
        return Some(Value::Object(map));
    }
    let merged_source = if kind(path) == "source" {
        base.and_then(Value::as_str)
            .zip(ours.and_then(Value::as_str))
            .zip(theirs.and_then(Value::as_str))
            .and_then(|((a, b), c)| merge_text(a, b, c))
    } else {
        None
    };
    if let Some(text) = merged_source {
        return Some(Value::String(text));
    }
    conflicts.push(MergeConflict {
        path: path.into(),
        base: base.cloned(),
        ours: ours.cloned(),
        theirs: theirs.cloned(),
    });
    base.cloned()
}
fn normalize(document: &GraphDocument) -> Result<Value, String> {
    let mut value = serde_json::to_value(document).map_err(|e| e.to_string())?;
    normalize_graph(value.get_mut("root").ok_or("Missing root")?)?;
    Ok(value)
}
fn normalize_graph(graph: &mut Value) -> Result<(), String> {
    let nodes = graph
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .ok_or("Missing nodes")?;
    let mut map = Map::new();
    for mut node in std::mem::take(nodes) {
        let id = node["id"].as_str().ok_or("Missing node ID")?.to_owned();
        if map.contains_key(&id) {
            return Err(format!(
                "Duplicate node ID {id}: merge cannot identify nodes safely"
            ));
        }
        if let Some(inner) = node.get_mut("subgraph").filter(|v| !v.is_null()) {
            normalize_graph(inner)?;
        }
        map.insert(id, node);
    }
    graph["nodes"] = Value::Object(map);
    let edges = graph
        .get_mut("edges")
        .and_then(Value::as_array_mut)
        .ok_or("Missing edges")?;
    let mut map: Map<String, Value> = Map::new();
    for edge in std::mem::take(edges) {
        let key = serde_json::to_string(&[
            edge["to_node"].as_str().ok_or("Missing edge target")?,
            edge["to_port"].as_str().ok_or("Missing target port")?,
        ])
        .map_err(|e| e.to_string())?;
        map.entry(key)
            .or_insert_with(|| Value::Array(vec![]))
            .as_array_mut()
            .unwrap()
            .push(edge);
    }
    graph["edges"] = Value::Object(map);
    Ok(())
}
fn denormalize(mut value: Value) -> Result<GraphDocument, String> {
    denormalize_graph(value.get_mut("root").ok_or("Missing root")?)?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}
fn denormalize_graph(graph: &mut Value) -> Result<(), String> {
    let nodes = graph
        .get_mut("nodes")
        .and_then(Value::as_object_mut)
        .ok_or("Missing normalized nodes")?;
    let mut array = vec![];
    for (_, mut node) in std::mem::take(nodes) {
        if let Some(inner) = node.get_mut("subgraph").filter(|v| !v.is_null()) {
            denormalize_graph(inner)?;
        }
        array.push(node);
    }
    graph["nodes"] = Value::Array(array);
    let edges = graph
        .get_mut("edges")
        .and_then(Value::as_object_mut)
        .ok_or("Missing normalized edges")?;
    let mut array = vec![];
    for (_, value) in std::mem::take(edges) {
        array.extend(
            value
                .as_array()
                .ok_or("Invalid edge group")?
                .iter()
                .cloned(),
        );
    }
    graph["edges"] = Value::Array(array);
    Ok(())
}
#[derive(Clone, PartialEq)]
struct Edit {
    start: usize,
    end: usize,
    lines: Vec<String>,
}
fn line_edits(base: &[&str], changed: &[&str]) -> Option<Vec<Edit>> {
    let n = base.len();
    let m = changed.len();
    if (n + 1).checked_mul(m + 1)? > 4_000_000 {
        return None;
    }
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if base[i] == changed[j] {
                1 + lcs[i + 1][j + 1]
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut edits = vec![];
    while i < n || j < m {
        if i < n && j < m && base[i] == changed[j] {
            i += 1;
            j += 1;
            continue;
        }
        let start = i;
        let mut lines = vec![];
        while i < n || j < m {
            if i < n && j < m && base[i] == changed[j] {
                break;
            }
            if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
                lines.push(changed[j].into());
                j += 1;
            } else {
                i += 1;
            }
        }
        edits.push(Edit {
            start,
            end: i,
            lines,
        });
    }
    Some(edits)
}
fn overlapping(a: &Edit, b: &Edit) -> bool {
    if a.start == a.end && b.start == b.end {
        return a.start == b.start;
    }
    if a.start == a.end {
        return a.start >= b.start && a.start <= b.end;
    }
    if b.start == b.end {
        return b.start >= a.start && b.start <= a.end;
    }
    a.start < b.end && b.start < a.end
}
fn merge_text(base: &str, ours: &str, theirs: &str) -> Option<String> {
    let lines: Vec<_> = base.split_inclusive('\n').collect();
    let mut edits = line_edits(&lines, &ours.split_inclusive('\n').collect::<Vec<_>>())?;
    for edit in line_edits(&lines, &theirs.split_inclusive('\n').collect::<Vec<_>>())? {
        if edits.contains(&edit) {
            continue;
        }
        if edits.iter().any(|other| overlapping(other, &edit)) {
            return None;
        }
        edits.push(edit);
    }
    edits.sort_by_key(|e| (e.start, e.end));
    let mut result: String = String::new();
    let mut cursor = 0;
    for edit in edits {
        result.extend(lines[cursor..edit.start].iter().copied());
        result.extend(edit.lines);
        cursor = edit.end;
    }
    result.extend(lines[cursor..].iter().copied());
    Some(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{WorldKind, demo_document, make_edge};
    #[test]
    fn merges_independent_nested_fields_and_separate_source_lines() {
        let base = demo_document(WorldKind::Objects);
        let mut a = base.clone();
        let mut b = base.clone();
        a.root
            .nodes
            .iter_mut()
            .find(|n| n.id == "renderer")
            .unwrap()
            .subgraph
            .as_mut()
            .unwrap()
            .nodes[1]
            .label = "Alternative".into();
        b.root
            .nodes
            .iter_mut()
            .find(|n| n.id == "base")
            .unwrap()
            .params["value"] = 2.0.into();
        let source = "fn speed(time:f64, base:f64)->f64 {\n let x = time * 2.0;\n base + x\n}\n";
        let mut base = base;
        base.custom_sources.insert("speed".into(), source.into());
        a.custom_sources
            .insert("speed".into(), source.replace("2.0", "3.0"));
        b.custom_sources
            .insert("speed".into(), source.replace("base + x", "base - x"));
        let result = merge_documents(&base, &a, &b).unwrap();
        assert!(result.is_resolved());
        let source = &result.document.custom_sources["speed"];
        assert!(source.contains("3.0"));
        assert!(source.contains("base - x"));
        let changes = diff_documents(&base, &result.document).unwrap();
        assert!(changes.iter().any(|c| c.path.contains("rasterizer/label")));
        assert!(changes.iter().any(|c| c.kind == "source"));
    }
    #[test]
    fn same_input_rewire_and_source_conflicts_preserve_every_side_until_choice() {
        let base = demo_document(WorldKind::Objects);
        let mut a = base.clone();
        let mut b = base.clone();
        for d in [&mut a, &mut b] {
            d.root
                .edges
                .retain(|e| !(e.to_node == "world" && e.to_port == "speed"));
        }
        a.root
            .edges
            .push(make_edge("base", "value", "world", "speed"));
        b.root
            .edges
            .push(make_edge("clock", "value", "world", "speed"));
        a.custom_sources
            .insert("speed".into(), "fn ours() {}".into());
        b.custom_sources
            .insert("speed".into(), "fn theirs() {}".into());
        let mut result = merge_documents(&base, &a, &b).unwrap();
        assert_eq!(result.conflicts.len(), 2);
        assert_eq!(result.document.custom_sources, base.custom_sources);
        let index = result
            .conflicts
            .iter()
            .position(|c| c.path.starts_with("/custom_sources"))
            .unwrap();
        assert_eq!(
            result.conflicts[index].ours.as_ref().unwrap().as_str(),
            Some("fn ours() {}")
        );
        result.resolve(index, ConflictChoice::Theirs).unwrap();
        assert_eq!(result.document.custom_sources["speed"], "fn theirs() {}");
        let report =
            std::env::temp_dir().join(format!("merge-{}.json", crate::history::unique_id()));
        result.save(&report).unwrap();
        let recovered: MergeResult =
            serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
        assert_eq!(recovered.conflicts, result.conflicts);
        std::fs::remove_file(report).unwrap();
    }
    #[test]
    fn deletion_against_edit_is_conflict_not_data_loss() {
        let base = demo_document(WorldKind::Ecs);
        let mut a = base.clone();
        let mut b = base.clone();
        a.root.nodes.retain(|n| n.id != "base");
        b.root
            .nodes
            .iter_mut()
            .find(|n| n.id == "base")
            .unwrap()
            .label = "Keep me".into();
        let result = merge_documents(&base, &a, &b).unwrap();
        assert_eq!(result.conflicts.len(), 1);
        assert!(result.conflicts[0].ours.is_none());
        assert!(result.document.root.nodes.iter().any(|n| n.id == "base"));
    }
    #[test]
    fn divergent_new_node_keeps_whole_conflict_and_can_be_resolved() {
        let base = demo_document(WorldKind::Objects);
        let mut ours = base.clone();
        let mut theirs = base.clone();
        ours.root.nodes.push(module_engine_core::make_node(
            "new",
            "scalar.value",
            "Ours",
            [0., 0.],
            serde_json::json!({"value":2.0}),
        ));
        theirs.root.nodes.push(module_engine_core::make_node(
            "new",
            "clock.time",
            "Theirs",
            [0., 0.],
            serde_json::json!({}),
        ));
        let mut merged = merge_documents(&base, &ours, &theirs).unwrap();
        assert_eq!(merged.conflicts.len(), 1);
        assert_eq!(merged.conflicts[0].path, "/root/nodes/new");
        assert!(merged.conflicts[0].base.is_none());
        assert_eq!(
            merged.conflicts[0].ours.as_ref().unwrap()["module"],
            "scalar.value"
        );
        assert_eq!(
            merged.conflicts[0].theirs.as_ref().unwrap()["module"],
            "clock.time"
        );
        assert!(!merged.document.root.nodes.iter().any(|n| n.id == "new"));
        merged.resolve(0, ConflictChoice::Ours).unwrap();
        assert!(merged.is_resolved());
        assert_eq!(
            merged
                .document
                .root
                .nodes
                .iter()
                .find(|n| n.id == "new")
                .unwrap()
                .module,
            "scalar.value"
        );
    }
}
