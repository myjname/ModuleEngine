use crate::{BOARD, BoardView, CELL, Cell, Position};
use module_engine_core::*;
use module_engine_game_kit::grid::{GridBounds, GridProjection, Occupancy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn whole<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i32, D::Error> {
    let number = f64::deserialize(deserializer)?;
    if number.is_finite()
        && number.fract() == 0.
        && number >= i32::MIN as f64
        && number <= i32::MAX as f64
    {
        Ok(number as i32)
    } else {
        Err(serde::de::Error::custom("Expected a whole grid coordinate"))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct BoardConfig {
    #[serde(deserialize_with = "whole")]
    pub width: i32,
    #[serde(deserialize_with = "whole")]
    pub height: i32,
    pub cell_size: f32,
    pub level_id: String,
}
impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            width: 7,
            height: 5,
            cell_size: 1.,
            level_id: "level01".into(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    #[default]
    Floor,
    Wall,
    Goal,
    Crate,
    Pawn,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Floor => "floor",
            Self::Wall => "wall",
            Self::Goal => "goal",
            Self::Crate => "crate",
            Self::Pawn => "pawn",
        }
    }
    fn layer(self) -> &'static str {
        match self {
            Self::Pawn | Self::Crate => "actor",
            other => other.name(),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CellData {
    #[serde(deserialize_with = "whole")]
    pub x: i32,
    #[serde(deserialize_with = "whole")]
    pub z: i32,
    pub kind: Kind,
}
impl CellData {
    pub fn cell(&self) -> Cell {
        Cell::new(self.x, self.z)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Level {
    pub id: String,
    pub title: String,
    pub config: BoardConfig,
    pub origin: [f32; 3],
    pub walls: BTreeSet<Cell>,
    pub goals: BTreeSet<Cell>,
    pub initial: Position,
    pub pawn_id: u64,
    pub revision: String,
    pub entities: Vec<GameEntity>,
}
impl Level {
    pub fn bounds(&self) -> Result<GridBounds, String> {
        GridBounds::new(self.config.width as u32, self.config.height as u32)
    }
    pub fn projection(&self) -> Result<GridProjection, String> {
        GridProjection::new(
            self.bounds()?,
            [
                self.origin[0] - (self.config.width - 1) as f32 * self.config.cell_size * 0.5,
                self.origin[2] - (self.config.height - 1) as f32 * self.config.cell_size * 0.5,
            ],
            self.config.cell_size,
        )
    }
    pub fn translation(&self, cell: Cell, kind: Kind) -> Result<[f32; 3], String> {
        let [x, z] = self.projection()?.cell_center(cell)?;
        let height = match kind {
            Kind::Floor => 0.,
            Kind::Goal => 0.045,
            Kind::Wall => 0.48,
            Kind::Crate => 0.39,
            Kind::Pawn => 0.42,
        };
        Ok([x, self.origin[1] + height * self.config.cell_size, z])
    }
    pub fn view(&self, position: &Position) -> BoardView {
        BoardView {
            width: self.config.width as u32,
            height: self.config.height as u32,
            walls: self.walls.iter().copied().collect(),
            goals: self.goals.iter().copied().collect(),
            pawn: position.pawn,
            crates: position.crates.values().copied().collect(),
        }
    }
    pub fn validate_position(&self, position: &Position) -> Result<(), String> {
        if position.crates.keys().ne(self.initial.crates.keys()) {
            return Err("Saved attempt crate identities differ from the authored level".into());
        }
        let mut occupancy = Occupancy::new(self.bounds()?);
        occupancy.insert(position.pawn, "actor", self.pawn_id)?;
        for (id, cell) in &position.crates {
            occupancy.insert(*cell, "actor", *id)?;
        }
        if self.walls.contains(&position.pawn)
            || position
                .crates
                .values()
                .any(|cell| self.walls.contains(cell))
        {
            return Err("Attempt overlaps a wall".into());
        }
        Ok(())
    }
    pub fn solved(&self, position: &Position) -> bool {
        position
            .crates
            .values()
            .all(|cell| self.goals.contains(cell))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Catalog {
    pub levels: Vec<Level>,
    pub revision: String,
}
impl Catalog {
    pub fn from_project(project: &GameProject) -> Result<Self, String> {
        let mut levels = Vec::new();
        for definition in crate::level_catalog() {
            let scene = project
                .scenes
                .get(definition.id)
                .ok_or_else(|| format!("Missing Crate Garden scene '{}'", definition.id))?;
            levels.push(from_entities(
                definition.id,
                &scene.name,
                project.resolve_scene(definition.id)?,
            )?);
        }
        if project.scenes.len() != levels.len() {
            return Err("Crate Garden requires its ten authored level scenes".into());
        }
        let revision = canonical_contract_hash(
            &levels
                .iter()
                .map(|level| (&level.id, &level.revision))
                .collect::<Vec<_>>(),
        )?;
        Ok(Self { levels, revision })
    }
    pub fn level(&self, id: &str) -> Result<&Level, String> {
        self.levels
            .iter()
            .find(|level| level.id == id)
            .ok_or_else(|| format!("Unknown Crate Garden level '{id}'"))
    }
    pub fn ids(&self) -> Vec<String> {
        self.levels.iter().map(|level| level.id.clone()).collect()
    }
}

pub(crate) fn from_entities(
    id: &str,
    title: &str,
    entities: Vec<GameEntity>,
) -> Result<Level, String> {
    let boards = entities
        .iter()
        .filter(|entity| entity.components.contains_key(BOARD))
        .collect::<Vec<_>>();
    if boards.len() != 1 {
        return Err("A Crate Garden scene needs exactly one board".into());
    }
    let board = boards[0];
    let config: BoardConfig =
        serde_json::from_value(board.components[BOARD].clone()).map_err(|e| e.to_string())?;
    validate_board(&config)?;
    if config.level_id != id
        || board.object.parent.is_some()
        || board.object.transform.rotation != [0.; 3]
    {
        return Err("Board identity/root rotation differs from its level scene".into());
    }
    let mut index = Occupancy::new(GridBounds::new(config.width as u32, config.height as u32)?);
    let mut walls = BTreeSet::new();
    let mut goals = BTreeSet::new();
    let mut crates = BTreeMap::new();
    let mut pawn = None;
    for entity in &entities {
        if let Some(raw) = entity.components.get(CELL) {
            if entity.object.parent.is_some() || entity.components.contains_key(BOARD) {
                return Err("Crate Garden cells must be independent root objects".into());
            }
            let data: CellData = serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
            index.insert(data.cell(), data.kind.layer(), entity.object.id)?;
            match data.kind {
                Kind::Wall => {
                    walls.insert(data.cell());
                }
                Kind::Goal => {
                    goals.insert(data.cell());
                }
                Kind::Crate => {
                    crates.insert(entity.object.id, data.cell());
                }
                Kind::Pawn => {
                    if pawn.replace((entity.object.id, data.cell())).is_some() {
                        return Err("A level needs exactly one pawn".into());
                    }
                }
                Kind::Floor => {}
            }
        }
    }
    let (pawn_id, pawn) = pawn.ok_or("A level needs one pawn")?;
    if crates.is_empty()
        || crates.len() != goals.len()
        || goals.iter().any(|cell| walls.contains(cell))
    {
        return Err("A level needs equal nonzero crate/goal counts without wall overlap".into());
    }
    let initial = Position {
        pawn,
        crates,
        moves: 0,
    };
    let revision = canonical_contract_hash(
        &json!({"config":config,"origin":board.object.transform.translation,"walls":walls,"goals":goals,"initial":initial}),
    )?;
    let result = Level {
        id: id.into(),
        title: title.into(),
        config,
        origin: board.object.transform.translation,
        walls,
        goals,
        initial,
        pawn_id,
        revision,
        entities,
    };
    result.validate_position(&result.initial)?;
    Ok(result)
}

pub(crate) fn world_level(world: &EcsWorld) -> Result<Level, String> {
    let board_id = world
        .entities_with(&[BOARD])
        .first()
        .copied()
        .ok_or("Missing Crate Garden board")?;
    let config: BoardConfig = serde_json::from_value(
        world
            .component(board_id, BOARD)
            .ok_or("Missing board component")?
            .clone(),
    )
    .map_err(|e| e.to_string())?;
    from_entities(
        &config.level_id,
        &config.level_id,
        world.entities_snapshot(),
    )
}

pub(crate) fn validate_board(board: &BoardConfig) -> Result<(), String> {
    if !(3..=32).contains(&board.width)
        || !(3..=32).contains(&board.height)
        || !board.cell_size.is_finite()
        || !(0.1..=4.).contains(&board.cell_size)
        || !crate::level_catalog()
            .iter()
            .any(|level| level.id == board.level_id)
    {
        Err("Invalid Crate Garden board dimensions, scale or identity".into())
    } else {
        Ok(())
    }
}
fn schema(id: &str, defaults: Value, fields: Vec<(&str, PropertyType)>) -> ComponentSchema {
    ComponentSchema {
        id: id.into(),
        owner_module: "g1.rules".into(),
        version: "1".into(),
        label: id.into(),
        fields: fields
            .into_iter()
            .map(|(name, value_type)| PropertySchema {
                id: format!("{id}.{name}"),
                label: name.into(),
                scope: PropertyScope::Component {
                    component: id.into(),
                },
                default: defaults[name].clone(),
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
            serde_json::to_value(BoardConfig::default()).unwrap(),
            vec![
                ("width", PropertyType::Number { min: 3., max: 32. }),
                ("height", PropertyType::Number { min: 3., max: 32. }),
                ("cell_size", PropertyType::Number { min: 0.1, max: 4. }),
                (
                    "level_id",
                    PropertyType::Enum {
                        choices: crate::level_catalog()
                            .iter()
                            .map(|level| level.id.into())
                            .collect(),
                    },
                ),
            ],
        ),
        |entity, _| {
            let board: BoardConfig = serde_json::from_value(entity.components[BOARD].clone())
                .map_err(|e| e.to_string())?;
            validate_board(&board)
        },
    )?;
    registry.register_with_validator(
        schema(
            CELL,
            serde_json::to_value(CellData::default()).unwrap(),
            vec![
                ("x", PropertyType::Number { min: 0., max: 31. }),
                ("z", PropertyType::Number { min: 0., max: 31. }),
                (
                    "kind",
                    PropertyType::Enum {
                        choices: ["floor", "wall", "goal", "crate", "pawn"]
                            .map(str::to_owned)
                            .to_vec(),
                    },
                ),
            ],
        ),
        |entity, _| {
            let cell: CellData = serde_json::from_value(entity.components[CELL].clone())
                .map_err(|e| e.to_string())?;
            if (0..32).contains(&cell.x) && (0..32).contains(&cell.z) {
                Ok(())
            } else {
                Err("Cell coordinates are outside supported board range".into())
            }
        },
    )
}
