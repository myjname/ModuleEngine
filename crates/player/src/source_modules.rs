//! A source-installed extension used to exercise the public module contract.
//! Installation is called once from the application composition root.
use module_engine_core::{
    ComponentSchema, ExecutableSystem, PropertySchema, PropertyScope, PropertyType, ResourceAccess,
    SystemContext, SystemDescriptor, SystemPort, SystemRegistry, WorldCommand,
};
use serde_json::{Value, json};

const SPIN: &str = "example.spin";

pub fn install(registry: &mut SystemRegistry) -> Result<(), String> {
    let field = |path: &str, label: &str, value_type, default| PropertySchema {
        id: format!("{SPIN}.{path}"),
        label: label.into(),
        scope: PropertyScope::Component {
            component: SPIN.into(),
        },
        value_type,
        default,
        requires_modules: vec![],
        requires_components: vec![],
    };
    registry.components_mut().register(ComponentSchema {
        id: SPIN.into(),
        owner_module: SPIN.into(),
        version: "1".into(),
        label: "Rotation".into(),
        defaults: json!({"speed":1.,"axis":"y","enabled":true}),
        fields: vec![
            field(
                "speed",
                "Radians per second",
                PropertyType::Number {
                    min: -20.,
                    max: 20.,
                },
                json!(1.),
            ),
            field(
                "axis",
                "Axis",
                PropertyType::Enum {
                    choices: vec!["x".into(), "y".into(), "z".into()],
                },
                json!("y"),
            ),
            field("enabled", "Enabled", PropertyType::Boolean, json!(true)),
        ],
        requires_modules: vec![],
        requires_components: vec![],
    })?;
    registry.register(
        SystemDescriptor {
            id: SPIN.into(),
            state_schema: "example.spin/1".into(),
            components: vec![SPIN.into()],
            ports: vec![SystemPort {
                name: "world".into(),
                data_type: "GameWorld".into(),
                access: ResourceAccess::ReadWrite,
            }],
            requires: vec![],
        },
        |params| {
            if !params.is_null() && !params.as_object().is_some_and(|map| map.is_empty()) {
                return Err("Rotation configuration belongs to its component fields".into());
            }
            Ok(Box::new(Spin))
        },
    )
}

struct Spin;
impl ExecutableSystem for Spin {
    fn step(&mut self, context: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        let world = &context.world("world")?.world;
        let commands = world
            .entities_with(&[SPIN])
            .into_iter()
            .filter_map(|id| {
                let settings = world.component(id, SPIN)?;
                if !settings.get("enabled")?.as_bool()? {
                    return None;
                }
                let mut transform = world.object(id)?.transform;
                let axis = match settings.get("axis")?.as_str()? {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    _ => return None,
                };
                transform.rotation[axis] += settings.get("speed")?.as_f64()? as f32 * context.dt;
                Some(WorldCommand::SetTransform { id, transform })
            })
            .collect::<Vec<_>>();
        context.apply_commands(&commands)
    }
}
