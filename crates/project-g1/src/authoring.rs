use crate::{
    BOARD, BoardView, CELL, Cell,
    components::{self, BoardConfig, Catalog, CellData, Kind},
};
use module_engine_core::*;
use module_engine_game_kit::grid::GridBounds;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    Floor,
    Wall,
    Goal,
    Crate,
    Pawn,
    Erase,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ObjectRole {
    Board,
    Pawn,
    Tile(Kind, Cell),
}
fn role(entity: &GameEntity) -> Result<Option<ObjectRole>, String> {
    if entity.components.contains_key(BOARD) {
        return Ok(Some(ObjectRole::Board));
    }
    entity
        .components
        .get(CELL)
        .map(|value| {
            let cell: CellData =
                serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
            Ok(if cell.kind == Kind::Pawn {
                ObjectRole::Pawn
            } else {
                ObjectRole::Tile(cell.kind, cell.cell())
            })
        })
        .transpose()
}
fn edit_field(
    object: &mut AuthoredObject,
    component: &str,
    field: &str,
    value: Value,
) -> Result<(), String> {
    object
        .components
        .entry(component.into())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("Grid component must be an object")?
        .insert(field.into(), value);
    object.overrides.remove(&format!("{component}.{field}"));
    Ok(())
}
#[derive(Clone, Debug, PartialEq)]
pub struct GridDraft {
    pub scene_id: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub cell_size: f32,
    pub walls: BTreeSet<Cell>,
    pub goals: BTreeSet<Cell>,
    pub crates: BTreeSet<Cell>,
    pub pawn: Cell,
}
impl GridDraft {
    pub fn from_project(project: &GameProject, scene_id: &str) -> Result<Self, String> {
        let scene = project
            .scenes
            .get(scene_id)
            .ok_or("Unknown authored level")?;
        let level =
            components::from_entities(scene_id, &scene.name, project.resolve_scene(scene_id)?)?;
        Ok(Self {
            scene_id: scene_id.into(),
            title: scene.name.clone(),
            width: level.config.width as u32,
            height: level.config.height as u32,
            cell_size: level.config.cell_size,
            walls: level.walls,
            goals: level.goals,
            crates: level.initial.crates.values().copied().collect(),
            pawn: level.initial.pawn,
        })
    }
    pub fn paint(&mut self, cell: Cell, brush: Brush) -> Result<(), String> {
        if !GridBounds::new(self.width, self.height)?.contains(cell) {
            return Err("Painted cell is outside the board".into());
        }
        match brush {
            Brush::Floor | Brush::Erase => {
                self.walls.remove(&cell);
                self.goals.remove(&cell);
                self.crates.remove(&cell);
            }
            Brush::Wall => {
                if self.pawn == cell {
                    return Err("Move the pawn before painting a wall here".into());
                }
                self.goals.remove(&cell);
                self.crates.remove(&cell);
                self.walls.insert(cell);
            }
            Brush::Goal => {
                self.walls.remove(&cell);
                self.goals.insert(cell);
            }
            Brush::Crate => {
                if self.pawn == cell {
                    return Err("Move the pawn before painting a crate here".into());
                }
                self.walls.remove(&cell);
                self.crates.insert(cell);
            }
            Brush::Pawn => {
                self.walls.remove(&cell);
                self.crates.remove(&cell);
                self.pawn = cell;
            }
        }
        Ok(())
    }
    /// Build a candidate only. The caller owns editor history/publication/Save.
    /// Intermediate drafts may have unequal crate/goal counts; Apply may not.
    pub fn into_project(&self, base: &GameProject) -> Result<GameProject, String> {
        if Self::from_project(base, &self.scene_id)? == *self {
            return Ok(base.clone());
        }
        let old = &base.scenes[&self.scene_id];
        let mut desired = scene_from_draft(self)?;
        let resolved = base
            .resolve_scene(&self.scene_id)?
            .into_iter()
            .map(|entity| (entity.object.id, entity))
            .collect::<BTreeMap<_, _>>();
        let mut by_role = BTreeMap::new();
        for entity in resolved.values() {
            if let Some(role) = role(entity)?
                && by_role.insert(role, entity.object.id).is_some()
            {
                return Err("Grid has ambiguous authored object identities".into());
            }
        }
        let board_id = *by_role
            .get(&ObjectRole::Board)
            .ok_or("Missing authored board")?;
        let origin = resolved[&board_id].object.transform.translation;
        for object in &mut desired.objects {
            if object.components.contains_key(CELL) {
                for (coordinate, offset) in
                    object.object.transform.translation.iter_mut().zip(origin)
                {
                    *coordinate += offset;
                }
            }
        }
        let old_by_id = old
            .objects
            .iter()
            .map(|object| (object.object.id, object))
            .collect::<BTreeMap<_, _>>();
        let related = by_role.values().copied().collect::<BTreeSet<_>>();
        // Reserve even removed IDs: a new object must never inherit references
        // or opaque data belonging to a deleted or unrelated authored object.
        let mut used_ids = old_by_id.keys().copied().collect::<BTreeSet<_>>();
        let mut unused = 1_u64;
        let original = Self::from_project(base, &self.scene_id)?;
        let projection_changed = self.width != original.width
            || self.height != original.height
            || self.cell_size != original.cell_size;
        for object in &mut desired.objects {
            let desired_entity = GameEntity {
                object: object.object.clone(),
                components: object.components.clone(),
                references: object.references.clone(),
            };
            let desired_role =
                role(&desired_entity)?.ok_or("Generated grid object lacks a role")?;
            if let Some(id) = by_role.get(&desired_role) {
                let previous = old_by_id[id];
                let mut retained = (*previous).clone();
                if object.components.contains_key(BOARD) {
                    for name in ["width", "height", "cell_size", "level_id"] {
                        if resolved[id].components[BOARD].get(name)
                            != object.components[BOARD].get(name)
                        {
                            edit_field(
                                &mut retained,
                                BOARD,
                                name,
                                object.components[BOARD][name].clone(),
                            )?;
                        }
                    }
                    if self.width != original.width || self.cell_size != original.cell_size {
                        retained.object.transform.scale[0] = object.object.transform.scale[0];
                    }
                    if self.height != original.height || self.cell_size != original.cell_size {
                        retained.object.transform.scale[2] = object.object.transform.scale[2];
                    }
                    if self.cell_size != original.cell_size {
                        retained.object.transform.scale[1] = object.object.transform.scale[1];
                    }
                } else if object.components.contains_key(CELL) {
                    let next: CellData = serde_json::from_value(object.components[CELL].clone())
                        .map_err(|e| e.to_string())?;
                    let prev: CellData =
                        serde_json::from_value(resolved[id].components[CELL].clone())
                            .map_err(|e| e.to_string())?;
                    if next.x != prev.x {
                        edit_field(&mut retained, CELL, "x", json!(next.x))?;
                    }
                    if next.z != prev.z {
                        edit_field(&mut retained, CELL, "z", json!(next.z))?;
                    }
                    if next != prev || projection_changed {
                        retained.object.transform.translation = object.object.transform.translation;
                    }
                    if self.cell_size != original.cell_size {
                        retained.object.transform.scale = object.object.transform.scale;
                    }
                }
                *object = retained;
            } else {
                while used_ids.contains(&unused) {
                    unused = unused
                        .checked_add(1)
                        .ok_or("Grid object identity space is exhausted")?;
                }
                object.object.id = unused;
                used_ids.insert(unused);
            }
        }
        // Source-independent objects and their payloads are not grid cells.
        desired.objects.extend(
            old.objects
                .iter()
                .filter(|object| !related.contains(&object.object.id))
                .cloned(),
        );
        let mut candidate = base.clone();
        candidate.scenes.insert(self.scene_id.clone(), desired);
        candidate.validate_with_registry(&crate::registry())?;
        Ok(candidate)
    }
}

pub fn authored_board(project: &GameProject, scene_id: &str) -> Result<BoardView, String> {
    let draft = GridDraft::from_project(project, scene_id)?;
    Ok(BoardView {
        width: draft.width,
        height: draft.height,
        walls: draft.walls.into_iter().collect(),
        goals: draft.goals.into_iter().collect(),
        pawn: draft.pawn,
        crates: draft.crates.into_iter().collect(),
    })
}
pub fn level_ids(project: &GameProject) -> Result<Vec<String>, String> {
    Ok(Catalog::from_project(project)?.ids())
}

fn entity_id(kind: Kind, cell: Cell) -> u64 {
    let base = match kind {
        Kind::Floor => 1000,
        Kind::Wall => 10000,
        Kind::Goal => 20000,
        Kind::Crate => 30000,
        Kind::Pawn => return 900001,
    };
    base + cell.x as u64 + cell.y as u64 * 32
}
fn scene_from_draft(draft: &GridDraft) -> Result<AuthoredScene, String> {
    let board = BoardConfig {
        width: draft.width as i32,
        height: draft.height as i32,
        cell_size: draft.cell_size,
        level_id: draft.scene_id.clone(),
    };
    // Validate the source-owned maximum before allocating a tile per cell.
    components::validate_board(&board)?;
    let bounds = GridBounds::new(draft.width, draft.height)?;
    let mut root = AuthoredObject::new(WorldObject {
        id: 1,
        parent: None,
        transform: Transform {
            scale: [
                draft.width as f32 * draft.cell_size * 0.5 + 0.1,
                0.02 * draft.cell_size,
                draft.height as f32 * draft.cell_size * 0.5 + 0.1,
            ],
            ..Default::default()
        },
        color: [74, 92, 68, 255],
    });
    root.components
        .insert(BOARD.into(), serde_json::to_value(&board).unwrap());
    let mut objects = vec![root];
    let mut add = |kind: Kind, cell: Cell| -> Result<(), String> {
        if !bounds.contains(cell) {
            return Err("Grid draft contains an outside object".into());
        }
        let y = match kind {
            Kind::Floor => 0.,
            Kind::Goal => 0.045,
            Kind::Wall => 0.48,
            Kind::Crate => 0.39,
            Kind::Pawn => 0.42,
        };
        let mut object = AuthoredObject::new(WorldObject {
            id: entity_id(kind, cell),
            parent: None,
            transform: Transform {
                translation: [
                    (cell.x as f32 - (draft.width - 1) as f32 * 0.5) * draft.cell_size,
                    y * draft.cell_size,
                    (cell.y as f32 - (draft.height - 1) as f32 * 0.5) * draft.cell_size,
                ],
                scale: [draft.cell_size; 3],
                ..Default::default()
            },
            color: match kind {
                Kind::Floor => [232, 224, 194, 255],
                Kind::Wall => [110, 141, 113, 255],
                Kind::Goal => [247, 191, 78, 255],
                Kind::Crate => [186, 121, 66, 255],
                Kind::Pawn => [70, 147, 165, 255],
            },
        });
        object.components.insert(
            CELL.into(),
            serde_json::to_value(CellData {
                x: cell.x,
                z: cell.y,
                kind,
            })
            .unwrap(),
        );
        object.components.insert(
            "mesh".into(),
            json!({"path":format!("assets/models/{}.gltf", kind.name())}),
        );
        objects.push(object);
        Ok(())
    };
    for y in 0..draft.height as i32 {
        for x in 0..draft.width as i32 {
            add(Kind::Floor, Cell::new(x, y))?;
        }
    }
    for cell in &draft.walls {
        add(Kind::Wall, *cell)?;
    }
    for cell in &draft.goals {
        add(Kind::Goal, *cell)?;
    }
    for cell in &draft.crates {
        add(Kind::Crate, *cell)?;
    }
    add(Kind::Pawn, draft.pawn)?;
    Ok(AuthoredScene {
        name: draft.title.clone(),
        objects,
    })
}

pub fn project() -> GameProject {
    let mut project = GameProject::empty();
    project.entry_scene = "level01".into();
    project.scenes.clear();
    for definition in crate::level_catalog() {
        let mut draft = GridDraft {
            scene_id: definition.id.into(),
            title: definition.title.into(),
            width: definition.rows[0].len() as u32,
            height: definition.rows.len() as u32,
            cell_size: 1.,
            walls: BTreeSet::new(),
            goals: BTreeSet::new(),
            crates: BTreeSet::new(),
            pawn: Cell::default(),
        };
        for (y, row) in definition.rows.iter().enumerate() {
            for (x, token) in row.chars().enumerate() {
                let cell = Cell::new(x as i32, y as i32);
                if token == '#' {
                    draft.walls.insert(cell);
                }
                if ".*+".contains(token) {
                    draft.goals.insert(cell);
                }
                if "$*".contains(token) {
                    draft.crates.insert(cell);
                }
                if "@+".contains(token) {
                    draft.pawn = cell;
                }
            }
        }
        project.scenes.insert(
            definition.id.into(),
            scene_from_draft(&draft).expect("Valid original level"),
        );
    }
    project.audio_cues = crate::rules::CUES
        .iter()
        .copied()
        .map(|cue| (cue.into(), format!("assets/audio/{cue}.wav")))
        .collect();
    project.execution = ExecutionGraph {
        resources: vec![
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
        ],
        stages: vec![SystemStage {
            name: "Garden rules".into(),
            nodes: vec![SystemNode {
                id: "garden".into(),
                module: "g1.rules".into(),
                params: json!({}),
                bindings: BTreeMap::from([
                    ("world".into(), "world".into()),
                    ("input".into(), "input".into()),
                    ("events".into(), "events".into()),
                ]),
                after: vec![],
            }],
        }],
    };
    project
}
