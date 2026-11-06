//! Bounded integer grids, layered occupancy and a two-dimensional projection.
//! No layer has built-in collision, pushing, terrain or goal semantics.
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
}
impl Cell {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
    pub fn offset(self, x: i32, y: i32) -> Option<Self> {
        Some(Self {
            x: self.x.checked_add(x)?,
            y: self.y.checked_add(y)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct GridBounds {
    width: u32,
    height: u32,
}

impl<'de> Deserialize<'de> for GridBounds {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Data {
            width: u32,
            height: u32,
        }
        let data = Data::deserialize(deserializer)?;
        Self::new(data.width, data.height).map_err(serde::de::Error::custom)
    }
}

impl GridBounds {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err("Grid dimensions must be nonzero and fit positive i32 coordinates".into());
        }
        Ok(Self { width, height })
    }
    pub fn width(self) -> u32 {
        self.width
    }
    pub fn height(self) -> u32 {
        self.height
    }
    pub fn contains(self, cell: Cell) -> bool {
        cell.x >= 0 && cell.y >= 0 && (cell.x as u32) < self.width && (cell.y as u32) < self.height
    }
}

/// Each `(cell, layer)` has at most one occupant, and each occupant ID occurs
/// once. Different layers may overlap. Consumers decide which layers interact.
/// This transient validated index is rebuilt from consumer-owned authored/save
/// data; it is deliberately not a second serialized authority for that data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Occupancy<L: Ord, I: Ord> {
    bounds: GridBounds,
    by_id: BTreeMap<I, (Cell, L)>,
    by_cell: BTreeMap<(Cell, L), I>,
}
impl<L: Ord + Clone, I: Ord + Clone> Occupancy<L, I> {
    pub fn new(bounds: GridBounds) -> Self {
        Self {
            bounds,
            by_id: BTreeMap::new(),
            by_cell: BTreeMap::new(),
        }
    }
    pub fn bounds(&self) -> GridBounds {
        self.bounds
    }
    pub fn len(&self) -> usize {
        self.by_id.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
    pub fn get(&self, cell: Cell, layer: &L) -> Option<&I> {
        self.by_cell.get(&(cell, layer.clone()))
    }
    pub fn position(&self, id: &I) -> Option<Cell> {
        self.by_id.get(id).map(|(cell, _)| *cell)
    }
    pub fn layer(&self, id: &I) -> Option<&L> {
        self.by_id.get(id).map(|(_, layer)| layer)
    }
    pub fn entries(&self) -> impl Iterator<Item = (&I, Cell, &L)> {
        self.by_id
            .iter()
            .map(|(id, (cell, layer))| (id, *cell, layer))
    }
    pub fn insert(&mut self, cell: Cell, layer: L, id: I) -> Result<(), String> {
        if !self.bounds.contains(cell) {
            return Err("Occupant is outside grid bounds".into());
        }
        if self.by_id.contains_key(&id) {
            return Err("Occupant ID is already present".into());
        }
        if self.by_cell.contains_key(&(cell, layer.clone())) {
            return Err("Grid cell/layer is already occupied".into());
        }
        self.by_id.insert(id.clone(), (cell, layer.clone()));
        self.by_cell.insert((cell, layer), id);
        Ok(())
    }
    pub fn remove(&mut self, id: &I) -> Option<(Cell, L)> {
        let position = self.by_id.remove(id)?;
        self.by_cell.remove(&position);
        Some(position)
    }
    /// Validate the whole relocation batch before publication. Occupants may
    /// swap cells. Unknown/repeated IDs, out-of-bounds targets and same-layer
    /// collisions reject the entire batch without changing either index.
    pub fn relocate(&mut self, moves: &[(I, Cell)]) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        let mut candidate = self.by_cell.clone();
        for (id, cell) in moves {
            if !ids.insert(id) {
                return Err("Relocation repeats an occupant ID".into());
            }
            if !self.bounds.contains(*cell) {
                return Err("Relocation is outside grid bounds".into());
            }
            let old = self
                .by_id
                .get(id)
                .ok_or("Relocation names an unknown occupant")?;
            candidate.remove(old);
        }
        for (id, cell) in moves {
            let layer = &self.by_id[id].1;
            if candidate
                .insert((*cell, layer.clone()), id.clone())
                .is_some()
            {
                return Err("Relocation collides with an occupied cell/layer".into());
            }
        }
        self.by_cell = candidate;
        for (id, cell) in moves {
            self.by_id.get_mut(id).expect("Validated occupant").0 = *cell;
        }
        Ok(())
    }
}

/// Projects cell centers onto any 2D plane (for example screen XY or world XZ).
/// `origin` is the center of cell (0, 0). Coordinates are f32 for renderer/UI
/// compatibility; consumers needing exact large-coordinate arithmetic retain
/// `Cell` as their logical authority.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridProjection {
    bounds: GridBounds,
    origin: [f32; 2],
    cell_size: f32,
}
impl GridProjection {
    pub fn new(bounds: GridBounds, origin: [f32; 2], cell_size: f32) -> Result<Self, String> {
        if !origin.iter().all(|n| n.is_finite()) || !cell_size.is_finite() || cell_size <= 0. {
            return Err("Grid projection needs finite origin and positive finite cell size".into());
        }
        for (axis, span) in [bounds.width, bounds.height].into_iter().enumerate() {
            let far = origin[axis] as f64 + (span - 1) as f64 * cell_size as f64;
            if far > f32::MAX as f64 || far < -(f32::MAX as f64) {
                return Err("Grid projection exceeds finite f32 positions".into());
            }
        }
        Ok(Self {
            bounds,
            origin,
            cell_size,
        })
    }
    pub fn cell_center(self, cell: Cell) -> Result<[f32; 2], String> {
        if !self.bounds.contains(cell) {
            return Err("Cannot project a cell outside grid bounds".into());
        }
        Ok([
            (self.origin[0] as f64 + cell.x as f64 * self.cell_size as f64) as f32,
            (self.origin[1] as f64 + cell.y as f64 * self.cell_size as f64) as f32,
        ])
    }
    /// Nearest center, with half-open cell edges: the lower edge is included,
    /// and the upper edge belongs to the next cell. Nonfinite/outside is None.
    pub fn cell_at(self, position: [f32; 2]) -> Option<Cell> {
        if !position.iter().all(|n| n.is_finite()) {
            return None;
        }
        let x =
            ((position[0] as f64 - self.origin[0] as f64) / self.cell_size as f64 + 0.5).floor();
        let y =
            ((position[1] as f64 - self.origin[1] as f64) / self.cell_size as f64 + 0.5).floor();
        if x < 0. || y < 0. || x >= self.bounds.width as f64 || y >= self.bounds.height as f64 {
            return None;
        }
        Some(Cell::new(x as i32, y as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layered_board_moves_are_atomic_and_do_not_assign_game_rules_to_layers() {
        let mut cells = Occupancy::new(GridBounds::new(5, 5).unwrap());
        cells.insert(Cell::new(2, 2), "marker", 10).unwrap();
        cells.insert(Cell::new(2, 2), "actor", 20).unwrap();
        cells.insert(Cell::new(3, 2), "actor", 21).unwrap();
        let before = cells.clone();
        assert!(cells.relocate(&[(20, Cell::new(3, 2))]).is_err());
        assert_eq!(cells, before);
        cells
            .relocate(&[(20, Cell::new(3, 2)), (21, Cell::new(4, 2))])
            .unwrap();
        assert_eq!(cells.get(Cell::new(2, 2), &"marker"), Some(&10));
        assert_eq!(cells.position(&20), Some(Cell::new(3, 2)));
        let before = cells.clone();
        assert!(
            cells
                .relocate(&[(20, Cell::new(4, 4)), (21, Cell::new(5, 4))])
                .is_err()
        );
        assert!(
            cells
                .relocate(&[(20, Cell::new(1, 1)), (20, Cell::new(2, 1))])
                .is_err()
        );
        assert!(cells.relocate(&[(99, Cell::new(1, 1))]).is_err());
        assert_eq!(cells, before);
    }

    #[test]
    fn tactical_formation_consumer_swaps_units_and_projects_board_for_painting() {
        let bounds = GridBounds::new(8, 6).unwrap();
        let mut formation = Occupancy::new(bounds);
        formation
            .insert(Cell::new(1, 2), "ground", "archer")
            .unwrap();
        formation
            .insert(Cell::new(2, 2), "ground", "knight")
            .unwrap();
        formation.insert(Cell::new(1, 2), "air", "scout").unwrap();
        formation
            .relocate(&[("archer", Cell::new(2, 2)), ("knight", Cell::new(1, 2))])
            .unwrap();
        assert_eq!(formation.remove(&"scout"), Some((Cell::new(1, 2), "air")));
        assert_eq!(formation.len(), 2);
        let projection = GridProjection::new(bounds, [24., 40.], 16.).unwrap();
        for x in 0..8 {
            for y in 0..6 {
                let cell = Cell::new(x, y);
                assert_eq!(
                    projection.cell_at(projection.cell_center(cell).unwrap()),
                    Some(cell)
                );
            }
        }
        assert_eq!(projection.cell_at([16., 32.]), Some(Cell::new(0, 0)));
        assert_eq!(projection.cell_at([15., 32.]), None);
        assert_eq!(projection.cell_at([144., 40.]), None);
        assert_eq!(projection.cell_at([f32::NAN, 40.]), None);
    }

    #[test]
    fn malformed_dimensions_and_insertion_never_create_invalid_indexes() {
        assert!(GridBounds::new(0, 2).is_err());
        assert!(serde_json::from_str::<GridBounds>(r#"{"width":0,"height":2}"#).is_err());
        let bounds = GridBounds::new(2, 2).unwrap();
        assert!(GridProjection::new(bounds, [0., 0.], 0.).is_err());
        assert!(GridProjection::new(bounds, [f32::MAX, 0.], f32::MAX).is_err());
        let mut occupancy = Occupancy::new(bounds);
        occupancy.insert(Cell::new(0, 0), 0, 1).unwrap();
        let before = occupancy.clone();
        assert!(occupancy.insert(Cell::new(-1, 0), 0, 2).is_err());
        assert!(occupancy.insert(Cell::new(0, 0), 0, 2).is_err());
        assert!(occupancy.insert(Cell::new(1, 0), 0, 1).is_err());
        assert_eq!(occupancy, before);
        assert_eq!(Cell::new(i32::MAX, 0).offset(1, 0), None);
    }
}
