//! Execute the scalar portion actually connected to the active world.
//! Rust preview is delegated to the Custom module, not embedded in engine core.

use module_engine_core::GraphDocument;
use module_engine_custom::CustomProgram;
use std::collections::{BTreeMap, BTreeSet};

type CompiledScalar<'a> = dyn Fn(&str, f64, f64) -> Option<Result<f64, String>> + 'a;

#[cfg(test)]
pub fn world_speed(
    document: &GraphDocument,
    source: Option<&str>,
    time: f64,
) -> Result<f32, String> {
    world_speed_compiled(document, source, time, &|_, _, _| None)
}

pub fn world_speed_compiled(
    document: &GraphDocument,
    source: Option<&str>,
    time: f64,
    compiled: &CompiledScalar<'_>,
) -> Result<f32, String> {
    let value = match source {
        Some(source) => evaluate(document, source, time, &mut BTreeSet::new(), compiled)?,
        None => 1.0,
    };
    if !value.is_finite() || value.abs() > 100.0 {
        return Err("Speed must be a finite number within −100…100 for preview.".into());
    }
    Ok(value as f32)
}

fn input(
    document: &GraphDocument,
    node: &str,
    port: &str,
    time: f64,
    seen: &mut BTreeSet<String>,
    default: f64,
    compiled: &CompiledScalar<'_>,
) -> Result<f64, String> {
    match document
        .root
        .edges
        .iter()
        .find(|e| e.to_node == node && e.to_port == port)
    {
        Some(edge) => evaluate(document, &edge.from_node, time, seen, compiled),
        None => Ok(default),
    }
}

fn evaluate(
    document: &GraphDocument,
    id: &str,
    time: f64,
    seen: &mut BTreeSet<String>,
    compiled: &CompiledScalar<'_>,
) -> Result<f64, String> {
    if !seen.insert(id.to_owned()) {
        return Err(format!("Scalar dependency cycle: {id}"));
    }
    let node = document
        .root
        .nodes
        .iter()
        .find(|n| n.id == id)
        .ok_or_else(|| format!("Node {id} is missing"))?;
    let result = match node.module.as_str() {
        "scalar.value" => node
            .params
            .get("value")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| format!("Node {id}: a numeric value parameter is required")),
        "clock.time" => Ok(time),
        "custom.rust" => {
            let source_id = node
                .params
                .get("source_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("Custom {id}: source_id is missing"))?;
            let source = document
                .custom_sources
                .get(source_id)
                .ok_or_else(|| format!("Source {source_id} is missing"))?;
            let base = input(document, id, "base", time, seen, 1.0, compiled)?;
            let connected_time = input(document, id, "time", time, seen, time, compiled)?;
            if let Some(result) = compiled(source_id, connected_time, base) {
                result
            } else {
                CustomProgram::parse(source)
                    .map_err(|e| e.to_string())?
                    .evaluate(&BTreeMap::from([
                        ("base".into(), base),
                        ("time".into(), connected_time),
                    ]))
                    .map_err(|e| e.to_string())
            }
        }
        other => Err(format!(
            "Node {id}: scalar execution for module {other} is not registered"
        )),
    };
    seen.remove(id);
    result.and_then(|value| {
        if value.is_finite() {
            Ok(value)
        } else {
            Err(format!("Node {id} returned a non-finite number"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{WorldKind, demo_document, make_edge};
    #[test]
    fn actual_wiring_controls_the_value() {
        let mut d = demo_document(WorldKind::Objects);
        d.custom_sources.insert(
            "speed".into(),
            "fn speed(time:f64,base:f64)->f64{base*3.0+time}".into(),
        );
        assert_eq!(world_speed(&d, Some("speed"), 2.0).unwrap(), 5.0);
        d.root
            .nodes
            .iter_mut()
            .find(|n| n.id == "base")
            .unwrap()
            .params["value"] = serde_json::json!(2.0);
        assert_eq!(world_speed(&d, Some("speed"), 2.0).unwrap(), 8.0);
        d.root
            .edges
            .retain(|e| e.to_node != "speed" || e.to_port != "base");
        d.root
            .edges
            .push(make_edge("clock", "value", "speed", "base"));
        assert_eq!(world_speed(&d, Some("speed"), 2.0).unwrap(), 8.0);
        assert_eq!(world_speed(&d, None, 2.0).unwrap(), 1.0);
    }
    #[test]
    fn unsupported_code_stops_preview_without_losing_source() {
        let mut d = demo_document(WorldKind::Ecs);
        let source = "fn speed(time:f64,base:f64)->f64{let _x=vec![1,2];base+time}";
        d.custom_sources.insert("speed".into(), source.into());
        assert!(
            world_speed(&d, Some("speed"), 0.0)
                .unwrap_err()
                .contains("Unsupported")
        );
        assert_eq!(d.custom_sources["speed"], source);
    }
}
