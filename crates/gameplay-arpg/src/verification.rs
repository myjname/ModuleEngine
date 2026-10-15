use super::*;

/// Normalize only this package's process-local incarnation bookkeeping.
/// Node IDs are resolved through the authored graph so Custom state remains
/// exact, even when it uses similarly named fields.
pub fn portable_system_states(
    graph: &ExecutionGraph,
    states: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let mut modules = BTreeMap::new();
    for node in graph.stages.iter().flat_map(|stage| &stage.nodes) {
        if modules
            .insert(node.id.as_str(), node.module.as_str())
            .is_some()
        {
            return Err(format!("Duplicate system node ID {}", node.id));
        }
    }
    states
        .iter()
        .map(|(id, value)| {
            let module = modules.get(id.as_str()).copied().unwrap_or_default();
            Ok((id.clone(), systems::portable_native_state(module, value)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (ExecutionGraph, BTreeMap<String, Value>) {
        let mut graph = gameplay_schedule();
        graph.stages.push(SystemStage {
            name: "Source state".into(),
            nodes: vec![SystemNode {
                id: "source".into(),
                module: "example.spin".into(),
                params: Value::Null,
                bindings: BTreeMap::new(),
                after: vec![],
            }],
        });
        let states = BTreeMap::from([
            (
                "system-4".into(),
                json!({"1":{
                    "lifetime":17,"cooldown":0.4,"remaining":0.1,"windup":0.,
                    "direction":[0.,-1.],"hit":[10],"hit_lifetimes":{"10":19},"active":true
                }}),
            ),
            ("system-5".into(), json!([{"object":10,"lifetime":19}])),
            (
                "source".into(),
                json!({"lifetime":17,"hit_lifetimes":{"10":19},"cooldown":4}),
            ),
        ]);
        (graph, states)
    }

    #[test]
    fn parity_ignores_generations_and_preserves_every_logical_system_field() {
        let (graph, left) = fixture();
        let mut right = left.clone();
        right.get_mut("system-4").unwrap()["1"]["lifetime"] = json!(88);
        right.get_mut("system-4").unwrap()["1"]["hit_lifetimes"]["10"] = json!(89);
        right.get_mut("system-5").unwrap()[0]["lifetime"] = json!(89);
        let portable = portable_system_states(&graph, &left).unwrap();
        assert_ne!(left, right);
        assert_eq!(portable, portable_system_states(&graph, &right).unwrap());
        right.get_mut("system-5").unwrap()[0] = json!(10);
        assert_eq!(portable, portable_system_states(&graph, &right).unwrap());
        for (node, field, value) in [
            ("system-4", "cooldown", json!(0.5)),
            ("system-4", "remaining", json!(0.2)),
            ("system-4", "windup", json!(0.1)),
            ("system-4", "direction", json!([1., 0.])),
            ("system-4", "hit", json!([11])),
            ("system-4", "active", json!(false)),
        ] {
            let mut changed = left.clone();
            changed.get_mut(node).unwrap()["1"][field] = value;
            assert_ne!(
                portable,
                portable_system_states(&graph, &changed).unwrap(),
                "lost {field}"
            );
        }
        let mut changed = left.clone();
        changed.get_mut("system-5").unwrap()[0]["object"] = json!(11);
        assert_ne!(portable, portable_system_states(&graph, &changed).unwrap());
        let mut changed = left.clone();
        changed.get_mut("source").unwrap()["lifetime"] = json!(88);
        assert_ne!(portable, portable_system_states(&graph, &changed).unwrap());
        let mut changed = left.clone();
        changed.get_mut("system-4").unwrap()["1"]["source_extra"] = json!(7);
        assert_ne!(portable, portable_system_states(&graph, &changed).unwrap());
    }

    #[test]
    fn malformed_known_state_is_rejected_before_normalization() {
        let (graph, states) = fixture();
        for (node, value) in [
            ("system-4", json!({"1":{"lifetime":2}})),
            (
                "system-4",
                json!({"1":{"lifetime":"invalid","cooldown":0.,"remaining":0.,"windup":0.,"direction":[0.,1.],"hit":[],"active":false}}),
            ),
            ("system-5", json!([{"object":10,"lifetime":"invalid"}])),
            ("system-5", json!([{"object":10,"lifetime":2,"custom":7}])),
        ] {
            let mut invalid = states.clone();
            invalid.insert(node.into(), value);
            assert!(portable_system_states(&graph, &invalid).is_err());
        }
    }

    #[test]
    fn malformed_frames_never_compare_equal_to_default_frames() {
        let session = crate::session(demo_project(), WorldKind::Ecs).unwrap();
        let valid = session.state().clone();
        let mut invalid = valid.clone();
        invalid
            .module_data
            .insert(FRAME_KEY.into(), json!("malformed"));
        assert!(!states_equivalent(&invalid, &valid));
        assert!(!states_equivalent(&invalid, &invalid));
        let mut changed = valid.clone();
        changed
            .module_data
            .insert("source.extra/1".into(), json!(7));
        assert!(!states_equivalent(&valid, &changed));
    }

    #[test]
    fn versioned_frame_rejects_unknown_fields_at_every_nested_boundary() {
        let session = crate::session(demo_project(), WorldKind::Ecs).unwrap();
        let valid = session.state().clone();
        for field in [
            "unknown",
            "intentions",
            "telegraphs",
            "attack_visuals",
            "hit_visuals",
        ] {
            let mut frame = serde_json::to_value(ArpgFrame::default()).unwrap();
            match field {
                "intentions" => {
                    frame[field] = json!({"1":{"movement":[0.,0.],"aim":[0.,0.,0.],"attack":false,"unknown":7}})
                }
                "telegraphs" => {
                    frame[field] = json!([{"attacker":1,"center":[0.,0.,0.],"direction":[0.,1.],"range":2.,"arc":1.,"progress":0.,"unknown":7}])
                }
                "attack_visuals" => {
                    frame[field] = json!([{"attacker":1,"area":{"center":[0.,0.,0.],"direction":[0.,1.],"range":2.,"arc":1.},"progress":0.,"unknown":7}])
                }
                "hit_visuals" => {
                    frame[field] = json!([{"attacker":1,"target":2,"position":[0.,0.,0.],"damage":1.,"started_at":0.,"expires_at":0.35,"unknown":7}])
                }
                _ => frame[field] = json!(7),
            }
            let mut invalid = valid.clone();
            invalid.module_data.insert(FRAME_KEY.into(), frame);
            assert!(!states_equivalent(&valid, &invalid), "ignored {field}");
            assert!(state::read_frame(&invalid).is_err());
        }
        let area = json!({"center":[0.,0.,0.],"direction":[0.,1.],"range":2.,"arc":1.,"unknown":7});
        assert!(serde_json::from_value::<MeleeArea>(area).is_err());
    }
}
