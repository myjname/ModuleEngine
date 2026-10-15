use module_engine_core::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const BOARD: &str = "puzzle.board";
pub(crate) const CELL: &str = "puzzle.cell";

fn whole<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i32, D::Error> {
    let value = f64::deserialize(deserializer)?;
    if value.is_finite()
        && value.fract() == 0.
        && value >= i32::MIN as f64
        && value <= i32::MAX as f64
    {
        Ok(value as i32)
    } else {
        Err(serde::de::Error::custom(
            "Expected a whole cell/count number",
        ))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Board {
    #[serde(deserialize_with = "whole")]
    pub width: i32,
    #[serde(deserialize_with = "whole")]
    pub height: i32,
    pub cell_size: f32,
    #[serde(deserialize_with = "whole")]
    pub max_turns: i32,
}
impl Default for Board {
    fn default() -> Self {
        Self {
            width: 5,
            height: 5,
            cell_size: 1.,
            max_turns: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    #[default]
    Wall,
    Goal,
    Pawn,
    Crate,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Cell {
    #[serde(deserialize_with = "whole")]
    pub x: i32,
    #[serde(deserialize_with = "whole")]
    pub z: i32,
    pub kind: Kind,
}

fn schema(id: &str, defaults: Value, fields: Vec<(&str, PropertyType)>) -> ComponentSchema {
    ComponentSchema {
        id: id.into(),
        owner_module: "puzzle.turn".into(),
        version: "1".into(),
        label: id.into(),
        fields: fields
            .into_iter()
            .map(|(field, value_type)| PropertySchema {
                id: format!("{id}.{field}"),
                label: field.into(),
                scope: PropertyScope::Component {
                    component: id.into(),
                },
                default: defaults[field].clone(),
                value_type,
                requires_modules: vec![],
                requires_components: vec![],
            })
            .collect(),
        defaults,
        requires_modules: vec![],
        requires_components: vec![],
    }
}

pub(crate) fn install(registry: &mut ComponentRegistry) -> Result<(), String> {
    registry.register_with_validator(
        schema(
            BOARD,
            serde_json::to_value(Board::default()).unwrap(),
            vec![
                ("width", PropertyType::Number { min: 3., max: 64. }),
                ("height", PropertyType::Number { min: 3., max: 64. }),
                ("cell_size", PropertyType::Number { min: 0.1, max: 10. }),
                (
                    "max_turns",
                    PropertyType::Number {
                        min: 1.,
                        max: 1024.,
                    },
                ),
            ],
        ),
        |entity, _| {
            let board: Board = serde_json::from_value(entity.components[BOARD].clone())
                .map_err(|error| error.to_string())?;
            validate_board(&board)?;
            root(entity)
        },
    )?;
    registry.register_with_validator(
        schema(
            CELL,
            serde_json::to_value(Cell::default()).unwrap(),
            vec![
                ("x", PropertyType::Number { min: 0., max: 63. }),
                ("z", PropertyType::Number { min: 0., max: 63. }),
                (
                    "kind",
                    PropertyType::Enum {
                        choices: ["wall", "goal", "pawn", "crate"]
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                    },
                ),
            ],
        ),
        |entity, _| {
            let _: Cell = serde_json::from_value(entity.components[CELL].clone())
                .map_err(|error| error.to_string())?;
            root(entity)
        },
    )
}
fn root(entity: &GameEntity) -> Result<(), String> {
    if entity.object.parent.is_some() {
        Err("Puzzle board/cells must be root objects".into())
    } else {
        Ok(())
    }
}
fn validate_board(board: &Board) -> Result<(), String> {
    if !(3..=64).contains(&board.width)
        || !(3..=64).contains(&board.height)
        || !(1..=1024).contains(&board.max_turns)
        || !board.cell_size.is_finite()
        || !(0.1..=10.).contains(&board.cell_size)
    {
        Err("Invalid puzzle board dimensions, cell size or turn limit".into())
    } else {
        Ok(())
    }
}

pub(crate) struct Layout {
    pub board: Board,
    pub origin: [f32; 3],
    pub cells: BTreeMap<u64, Cell>,
    pub pawn: u64,
    pub crate_id: u64,
    pub goal: (i32, i32),
    pub walls: BTreeSet<(i32, i32)>,
}
impl Layout {
    pub fn free(&self, point: (i32, i32)) -> bool {
        point.0 >= 0
            && point.1 >= 0
            && point.0 < self.board.width
            && point.1 < self.board.height
            && !self.walls.contains(&point)
    }
    pub fn translation(&self, cell: &Cell) -> [f32; 3] {
        [
            self.origin[0]
                + (cell.x as f32 - (self.board.width - 1) as f32 * 0.5) * self.board.cell_size,
            self.origin[1] + if cell.kind == Kind::Goal { 0.13 } else { 0.5 },
            self.origin[2]
                + (cell.z as f32 - (self.board.height - 1) as f32 * 0.5) * self.board.cell_size,
        ]
    }
}
pub(crate) fn world_layout(world: &EcsWorld) -> Result<Layout, String> {
    let ids: BTreeSet<_> = world
        .entities_with(&[BOARD])
        .into_iter()
        .chain(world.entities_with(&[CELL]))
        .collect();
    layout(
        &ids.into_iter()
            .filter_map(|id| world.entity(id))
            .collect::<Vec<_>>(),
    )
}
pub(crate) fn project_layout(project: &GameProject, scene: &str) -> Result<Layout, String> {
    let mut entities = project.resolve_scene(scene)?;
    // Core validation resolves schema defaults before overrides. Structural
    // reads deliberately do not, so normalize this package's scalar defaults
    // and reapply its authored overrides for the independent layout hook.
    for entity in &mut entities {
        for name in [BOARD, CELL] {
            if let Some(value) = entity.components.get_mut(name) {
                *value = if name == BOARD {
                    serde_json::to_value(
                        serde_json::from_value::<Board>(value.clone())
                            .map_err(|error| error.to_string())?,
                    )
                    .unwrap()
                } else {
                    serde_json::to_value(
                        serde_json::from_value::<Cell>(value.clone())
                            .map_err(|error| error.to_string())?,
                    )
                    .unwrap()
                };
                if let Some(authored) = project.scenes[scene]
                    .objects
                    .iter()
                    .find(|object| object.object.id == entity.object.id)
                {
                    for (path, override_value) in &authored.overrides {
                        if let Some(field) = path.strip_prefix(&format!("{name}."))
                            && value.get(field).is_some()
                        {
                            value[field] = override_value.clone();
                        }
                    }
                }
            }
        }
    }
    layout(&entities)
}
fn layout(entities: &[GameEntity]) -> Result<Layout, String> {
    let boards: Vec<_> = entities
        .iter()
        .filter(|entity| entity.components.contains_key(BOARD))
        .collect();
    if boards.len() != 1 {
        return Err("Puzzle scene needs exactly one board".into());
    }
    let owner = boards[0];
    root(owner)?;
    if owner.components.contains_key(CELL) || owner.object.transform.rotation != [0.; 3] {
        return Err("Puzzle board cannot also be a cell or be rotated".into());
    }
    let board: Board = serde_json::from_value(owner.components[BOARD].clone())
        .map_err(|error| error.to_string())?;
    validate_board(&board)?;
    let mut cells = BTreeMap::new();
    let mut walls = BTreeSet::new();
    let mut pawn = None;
    let mut crate_id = None;
    let mut goal = None;
    for entity in entities {
        let Some(value) = entity.components.get(CELL) else {
            continue;
        };
        root(entity)?;
        let cell: Cell =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        if cell.x < 0 || cell.z < 0 || cell.x >= board.width || cell.z >= board.height {
            return Err("Puzzle cell is outside board bounds".into());
        }
        let duplicate = match cell.kind {
            Kind::Wall => !walls.insert((cell.x, cell.z)),
            Kind::Goal => goal.replace((cell.x, cell.z)).is_some(),
            Kind::Pawn => pawn.replace(entity.object.id).is_some(),
            Kind::Crate => crate_id.replace(entity.object.id).is_some(),
        };
        if duplicate {
            return Err("Duplicate puzzle pawn, crate, goal or wall".into());
        }
        cells.insert(entity.object.id, cell);
    }
    let (pawn, crate_id, goal) = (
        pawn.ok_or("Puzzle needs one pawn")?,
        crate_id.ok_or("Puzzle needs one crate")?,
        goal.ok_or("Puzzle needs one goal")?,
    );
    let pos = |id| {
        let cell = &cells[&id];
        (cell.x, cell.z)
    };
    if pos(pawn) == pos(crate_id)
        || walls.contains(&pos(pawn))
        || walls.contains(&pos(crate_id))
        || walls.contains(&goal)
    {
        return Err("Puzzle cells have a forbidden overlap".into());
    }
    Ok(Layout {
        board,
        origin: owner.object.transform.translation,
        cells,
        pawn,
        crate_id,
        goal,
        walls,
    })
}
