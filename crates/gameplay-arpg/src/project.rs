//! Reusable ARPG scheduling and source-owned project preparation.
use module_engine_core::*;
use serde_json::{Value, json};
#[cfg(test)]
use std::collections::BTreeMap;

/// Install source-owned project behavior; the core calls all installed hooks.
pub fn install_project_contracts(registry: &mut SystemRegistry) -> Result<(), String> {
    registry.register_project_contract("game.audio", validate_audio_project, prepare_audio_project)
}

fn audio_active(project: &GameProject) -> bool {
    project
        .execution
        .stages
        .iter()
        .flat_map(|stage| &stage.nodes)
        .any(|node| node.module == "game.audio")
}
fn validate_audio_project(
    project: &GameProject,
    context: &ComponentValidationContext<'_>,
) -> Result<(), String> {
    if audio_active(project) {
        for reference in project.audio_cues.values() {
            validate_component_asset_reference(reference, &["wav".into()], context)?;
        }
    }
    Ok(())
}
fn prepare_audio_project(project: &mut GameProject) -> Result<(), String> {
    if !audio_active(project) {
        project.audio_cues.clear();
    }
    Ok(())
}

/// Reusable editable ARPG schedule; projects supply their own scenes and goals.
pub fn gameplay_schedule() -> ExecutionGraph {
    let resources = vec![
        SystemResource {
            name: "world".into(),
            data_type: "GameWorld".into(),
            external: true,
            initial: Value::Null,
        },
        SystemResource {
            name: "input".into(),
            data_type: "GameInput".into(),
            external: true,
            initial: Value::Null,
        },
        SystemResource {
            name: "events".into(),
            data_type: "GameEvents".into(),
            external: true,
            initial: json!([]),
        },
        SystemResource {
            name: "previous".into(),
            data_type: "Positions".into(),
            external: false,
            initial: Value::Null,
        },
    ];
    let configs = [
        ("Controllers", "game.input", vec!["world", "input"]),
        ("AI", "game.ai", vec!["world"]),
        ("Movement", "game.movement", vec!["world", "previous"]),
        ("Collisions", "game.collision", vec!["world", "previous"]),
        ("Combat", "game.combat", vec!["world", "events"]),
        ("Health", "game.health", vec!["world", "events"]),
        ("Scenes", "game.portals", vec!["world", "events"]),
        ("Audio events", "game.audio", vec!["events"]),
    ];
    let stages = configs
        .into_iter()
        .enumerate()
        .map(|(i, (name, module, ports))| SystemStage {
            name: name.into(),
            nodes: vec![SystemNode {
                id: format!("system-{i}"),
                module: module.into(),
                params: json!({}),
                bindings: ports.into_iter().map(|p| (p.into(), p.into())).collect(),
                after: if i == 0 {
                    vec![]
                } else {
                    vec![format!("system-{}", i - 1)]
                },
            }],
        })
        .collect();
    ExecutionGraph { resources, stages }
}

pub trait ExecutionGraphExt {
    fn gameplay() -> Self;
}
impl ExecutionGraphExt for ExecutionGraph {
    fn gameplay() -> Self {
        gameplay_schedule()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo_project;

    #[test]
    fn demo_round_trips_and_validates_through_installed_source_contracts() {
        let project = demo_project();
        let restored: GameProject =
            serde_json::from_str(&serde_json::to_string(&project).unwrap()).unwrap();
        assert_eq!(restored, project);
        restored.validate_with_registry(&crate::registry()).unwrap();
        assert_eq!(restored.scenes.len(), 2);
        assert_eq!(restored.prefabs.len(), 1);
    }

    #[test]
    fn audio_contract_validates_manifest_and_prepares_only_the_runtime_copy() {
        let registry = crate::registry();
        let mut project = demo_project();
        project.audio_cues = BTreeMap::from([("attack".into(), "asset://tone".into())]);
        let modules = registry.installed_modules();
        let mut paths = BTreeMap::from([("tone".into(), "assets/tone.png".into())]);
        {
            let context = ComponentValidationContext {
                scenes: &project.scenes,
                modules: &modules,
                asset_paths: Some(&paths),
                active_components: None,
            };
            assert!(
                project
                    .validate_with_context(&registry, &context)
                    .unwrap_err()
                    .contains("wav")
            );
        }
        paths.insert("tone".into(), "assets/tone.wav".into());
        {
            let context = ComponentValidationContext {
                scenes: &project.scenes,
                modules: &modules,
                asset_paths: Some(&paths),
                active_components: None,
            };
            project.validate_with_context(&registry, &context).unwrap();
        }
        for stage in &mut project.execution.stages {
            stage.nodes.retain(|node| node.module != "game.audio");
        }
        paths.clear();
        let source = project.clone();
        let context = ComponentValidationContext {
            scenes: &project.scenes,
            modules: &modules,
            asset_paths: Some(&paths),
            active_components: None,
        };
        let runtime = project
            .prepare_runtime_with_context(&registry, &context)
            .unwrap();
        assert!(runtime.audio_cues.is_empty());
        assert_eq!(project, source);
    }
}
