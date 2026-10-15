//! Keep user positions only for structurally unambiguous source nodes.

use module_engine_custom::CodeEdge;
use std::collections::BTreeMap;

pub fn signatures(
    nodes: Vec<(String, serde_json::Value)>,
    edges: &[CodeEdge],
) -> BTreeMap<String, String> {
    let kinds: BTreeMap<_, _> = nodes
        .into_iter()
        .map(|(id, mut kind)| {
            // Values change in-place; input position and operation identify a literal.
            for literal in ["Number", "Text", "Boolean"] {
                if kind.get(literal).is_some() {
                    kind = serde_json::Value::String(literal.into());
                    break;
                }
            }
            (id, kind.to_string())
        })
        .collect();
    kinds
        .iter()
        .map(|(id, kind)| {
            let mut incoming = edges
                .iter()
                .filter(|e| &e.to == id)
                .map(|e| (e.input, kinds.get(&e.from).cloned().unwrap_or_default()))
                .collect::<Vec<_>>();
            let mut outgoing = edges
                .iter()
                .filter(|e| &e.from == id)
                .map(|e| (e.input, kinds.get(&e.to).cloned().unwrap_or_default()))
                .collect::<Vec<_>>();
            incoming.sort();
            outgoing.sort();
            (
                id.clone(),
                serde_json::json!([kind, incoming, outgoing]).to_string(),
            )
        })
        .collect()
}

pub fn remap(
    positions: &BTreeMap<String, [f32; 2]>,
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> BTreeMap<String, [f32; 2]> {
    let mut old_counts = BTreeMap::<&str, usize>::new();
    let mut new_counts = BTreeMap::<&str, usize>::new();
    for key in old.values() {
        *old_counts.entry(key).or_default() += 1;
    }
    for key in new.values() {
        *new_counts.entry(key).or_default() += 1;
    }
    new.iter()
        .filter_map(|(id, key)| {
            if old_counts.get(key.as_str()) != Some(&1) || new_counts.get(key.as_str()) != Some(&1)
            {
                return None;
            }
            let old_id = old.iter().find(|(_, k)| *k == key)?.0;
            positions
                .get(old_id)
                .filter(|p| p.iter().all(|v| v.is_finite()))
                .map(|p| (id.clone(), *p))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_unique_nodes_after_id_shift_and_rejects_ambiguous_matches() {
        let old = BTreeMap::from([
            ("a".into(), "input time".into()),
            ("b".into(), "duplicate".into()),
            ("c".into(), "duplicate".into()),
        ]);
        let new = BTreeMap::from([
            ("new".into(), "input time".into()),
            ("other".into(), "duplicate".into()),
        ]);
        let positions = BTreeMap::from([("a".into(), [11., 22.]), ("b".into(), [33., 44.])]);
        assert_eq!(
            remap(&positions, &old, &new),
            BTreeMap::from([("new".into(), [11., 22.])])
        );
    }
}
