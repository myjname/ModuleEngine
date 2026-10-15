use crate::{WorldCommand, WorldObject};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identifies built-in world storage or a separately registered source provider.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorldKind {
    Objects,
    Ecs,
    Custom,
}

/// Renderer-neutral indexed triangle mesh.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[usize; 3]>,
    /// Per-vertex UV set zero. Missing values use `[0, 0]`.
    #[serde(default)]
    pub texcoords: Vec<[f32; 2]>,
}

impl Mesh {
    /// A unit-centered cube with side length two; no renderer-owned mesh handles.
    pub fn cube() -> Self {
        Self {
            vertices: vec![
                [-1., -1., -1.],
                [1., -1., -1.],
                [1., 1., -1.],
                [-1., 1., -1.],
                [-1., -1., 1.],
                [1., -1., 1.],
                [1., 1., 1.],
                [-1., 1., 1.],
            ],
            triangles: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [3, 7, 6],
                [3, 6, 2],
                [0, 4, 7],
                [0, 7, 3],
                [1, 2, 6],
                [1, 6, 5],
            ],
            texcoords: vec![],
        }
    }
}

/// One drawable item. Model matrices use row-major storage and column vectors.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RenderItem {
    pub id: u64,
    pub mesh: Mesh,
    pub model: [[f32; 4]; 4],
    pub color: [u8; 4],
    #[serde(default)]
    pub material: Material,
}

/// Decoded, renderer-neutral RGBA8 base-color image. Shared between snapshots.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TextureImage {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
}

/// Opaque static base-color material. Texture RGB is sRGB; factors are linear.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Material {
    pub base_color: [f32; 4],
    pub base_color_texture: Option<std::sync::Arc<TextureImage>>,
}
impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: [1.; 4],
            base_color_texture: None,
        }
    }
}

/// Extracted immutable scene; no dependency on an ECS or object-tree API.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SceneSnapshot {
    pub items: Vec<RenderItem>,
    pub time: f32,
}

/// Minimal executable source-provider boundary implemented by world authors.
pub trait WorldProvider: Send {
    /// Advance the provider. Runtime supplies finite values and nonnegative dt;
    /// negative speed is valid and reverses motion in the built-in worlds.
    fn step(&mut self, dt: f32, speed: f32);
    /// Produce neutral geometry, sorted/stable as appropriate for this provider.
    fn snapshot(&self) -> SceneSnapshot;
    /// Optional portable editing API. Existing read-only providers keep rendering
    /// unchanged and explicitly reject editing rather than silently doing nothing.
    fn objects(&self) -> Result<Vec<WorldObject>, String> {
        Err("This World provider does not support portable object editing".into())
    }
    /// Apply all operations or none. Built-ins share the same local-transform,
    /// object-ID and subtree-removal semantics with independent persistent stores.
    fn apply_commands(&mut self, _commands: &[WorldCommand]) -> Result<(), String> {
        Err("This World provider does not support portable world commands".into())
    }
    /// Validate without mutation so hosts can stage mixed world/render packets.
    fn validate_commands(&self, commands: &[WorldCommand]) -> Result<(), String> {
        crate::validate_world_commands(&self.objects()?, commands)
    }
}

/// Local translation, Euler rotation in radians and scale.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Transform {
    pub translation: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: [0.; 3],
            rotation: [0.; 3],
            scale: [1.; 3],
        }
    }
}

impl Transform {
    /// Compose T * Rz * Ry * Rx * S using the renderer's matrix convention.
    pub fn matrix(self) -> [[f32; 4]; 4] {
        let [x, y, z] = self.rotation;
        let (sx, cx) = x.sin_cos();
        let (sy, cy) = y.sin_cos();
        let (sz, cz) = z.sin_cos();
        let rx = [
            [1., 0., 0., 0.],
            [0., cx, -sx, 0.],
            [0., sx, cx, 0.],
            [0., 0., 0., 1.],
        ];
        let ry = [
            [cy, 0., sy, 0.],
            [0., 1., 0., 0.],
            [-sy, 0., cy, 0.],
            [0., 0., 0., 1.],
        ];
        let rz = [
            [cz, -sz, 0., 0.],
            [sz, cz, 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ];
        let mut result = multiply_matrices(rz, multiply_matrices(ry, rx));
        for row in result.iter_mut().take(3) {
            for (column, scale) in self.scale.iter().enumerate() {
                row[column] *= scale;
            }
        }
        for (row, translation) in self.translation.iter().enumerate() {
            result[row][3] = *translation;
        }
        result
    }
}

/// Identity model matrix.
pub fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
        [0., 0., 0., 1.],
    ]
}

/// Compose row-major matrices for hierarchical transforms.
pub fn multiply_matrices(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut result = [[0.; 4]; 4];
    for (r, row) in result.iter_mut().enumerate() {
        for (c, value) in row.iter_mut().enumerate() {
            *value = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    result
}

/// Apply an affine model matrix to a position.
pub fn transform_point(matrix: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| {
        matrix[r][0] * p[0] + matrix[r][1] * p[1] + matrix[r][2] * p[2] + matrix[r][3]
    })
}

#[derive(Clone)]
struct Object {
    id: u64,
    local: Transform,
    angular_velocity: [f32; 3],
    color: [u8; 4],
    children: Vec<Object>,
}

/// A recursive owned hierarchy. No entity/component table is hidden underneath.
#[derive(Clone)]
pub struct ObjectsWorld {
    roots: Vec<Object>,
    time: f32,
}

impl ObjectsWorld {
    /// Construct authored objects in a real recursive store, with no demo motion.
    pub fn from_objects(objects: Vec<WorldObject>) -> Result<Self, String> {
        crate::validate_world_commands(&objects, &[])?;
        fn build(
            id: u64,
            objects: &BTreeMap<u64, WorldObject>,
            children: &BTreeMap<Option<u64>, Vec<u64>>,
        ) -> Object {
            let object = &objects[&id];
            Object {
                id,
                local: object.transform,
                color: object.color,
                angular_velocity: [0.; 3],
                children: children
                    .get(&Some(id))
                    .into_iter()
                    .flatten()
                    .map(|id| build(*id, objects, children))
                    .collect(),
            }
        }
        let objects = objects
            .into_iter()
            .map(|o| (o.id, o))
            .collect::<BTreeMap<_, _>>();
        let mut children = BTreeMap::<Option<u64>, Vec<u64>>::new();
        for object in objects.values() {
            children.entry(object.parent).or_default().push(object.id);
        }
        Ok(Self {
            roots: children
                .get(&None)
                .into_iter()
                .flatten()
                .map(|id| build(*id, &objects, &children))
                .collect(),
            time: 0.,
        })
    }
    /// Same initial visible scene as EcsWorld, stored as owned parent/child objects.
    pub fn demo() -> Self {
        Self {
            roots: vec![
                Object {
                    id: 1,
                    local: transform([-1.35, 0.0, 0.0], [0.8; 3]),
                    angular_velocity: [0.18, 0.65, 0.0],
                    color: [80, 175, 245, 255],
                    children: vec![Object {
                        id: 2,
                        local: transform([0.0, 2.0, 0.0], [0.32; 3]),
                        angular_velocity: [0.3, -0.7, 0.0],
                        color: [255, 182, 75, 255],
                        children: vec![],
                    }],
                },
                Object {
                    id: 3,
                    local: transform([1.5, -0.15, 0.2], [0.65; 3]),
                    angular_velocity: [0.25, -0.45, 0.0],
                    color: [189, 128, 240, 255],
                    children: vec![],
                },
                Object {
                    id: 4,
                    local: transform([0.0, -1.0, 0.0], [4.0, 0.08, 3.0]),
                    angular_velocity: [0.; 3],
                    color: [79, 103, 111, 255],
                    children: vec![],
                },
            ],
            time: 0.0,
        }
    }
}

impl WorldProvider for ObjectsWorld {
    fn objects(&self) -> Result<Vec<WorldObject>, String> {
        fn collect(object: &Object, parent: Option<u64>, result: &mut Vec<WorldObject>) {
            result.push(WorldObject {
                id: object.id,
                parent,
                transform: object.local,
                color: object.color,
            });
            for child in &object.children {
                collect(child, Some(object.id), result);
            }
        }
        let mut objects = vec![];
        for root in &self.roots {
            collect(root, None, &mut objects);
        }
        objects.sort_by_key(|o| o.id);
        Ok(objects)
    }

    fn apply_commands(&mut self, commands: &[WorldCommand]) -> Result<(), String> {
        fn velocities(object: &Object, result: &mut BTreeMap<u64, [f32; 3]>) {
            result.insert(object.id, object.angular_velocity);
            for child in &object.children {
                velocities(child, result);
            }
        }
        fn build(
            id: u64,
            objects: &BTreeMap<u64, WorldObject>,
            velocities: &BTreeMap<u64, [f32; 3]>,
            children: &BTreeMap<Option<u64>, Vec<u64>>,
        ) -> Object {
            let object = &objects[&id];
            Object {
                id,
                local: object.transform,
                color: object.color,
                angular_velocity: velocities.get(&id).copied().unwrap_or([0.; 3]),
                children: children
                    .get(&Some(id))
                    .into_iter()
                    .flatten()
                    .map(|id| build(*id, objects, velocities, children))
                    .collect(),
            }
        }
        let mut velocities_map = BTreeMap::new();
        for root in &self.roots {
            velocities(root, &mut velocities_map);
        }
        let mut objects = self.objects()?.into_iter().map(|o| (o.id, o)).collect();
        crate::control::apply_object_commands(&mut objects, commands)?;
        for command in commands {
            if let WorldCommand::Spawn { object } = command {
                velocities_map.remove(&object.id);
            }
        }
        let mut children: BTreeMap<Option<u64>, Vec<u64>> = BTreeMap::new();
        for object in objects.values() {
            children.entry(object.parent).or_default().push(object.id);
        }
        self.roots = children
            .get(&None)
            .into_iter()
            .flatten()
            .map(|id| build(*id, &objects, &velocities_map, &children))
            .collect();
        Ok(())
    }

    fn step(&mut self, dt: f32, speed: f32) {
        fn advance(object: &mut Object, amount: f32) {
            for i in 0..3 {
                object.local.rotation[i] += object.angular_velocity[i] * amount;
            }
            for child in &mut object.children {
                advance(child, amount);
            }
        }
        self.time += dt;
        for root in &mut self.roots {
            advance(root, dt * speed);
        }
    }

    fn snapshot(&self) -> SceneSnapshot {
        fn extract(object: &Object, parent: [[f32; 4]; 4], items: &mut Vec<RenderItem>) {
            let model = multiply_matrices(parent, object.local.matrix());
            items.push(RenderItem {
                id: object.id,
                mesh: Mesh::cube(),
                model,
                color: object.color,
                material: Material::default(),
            });
            for child in &object.children {
                extract(child, model, items);
            }
        }
        let mut items = vec![];
        for root in &self.roots {
            extract(root, identity_matrix(), &mut items);
        }
        items.sort_by_key(|item| item.id);
        SceneSnapshot {
            items,
            time: self.time,
        }
    }
}

/// An ECS-like world with independent component tables keyed by entity ID.
/// It does not wrap or copy ObjectsWorld during construction or extraction.
#[derive(Clone)]
pub struct EcsWorld {
    transforms: BTreeMap<u64, Transform>,
    parents: BTreeMap<u64, u64>,
    colors: BTreeMap<u64, [u8; 4]>,
    angular_velocities: BTreeMap<u64, [f32; 3]>,
    time: f32,
}

impl EcsWorld {
    /// Construct authored entities in independent component tables, without motion.
    pub fn from_objects(objects: Vec<WorldObject>) -> Result<Self, String> {
        crate::validate_world_commands(&objects, &[])?;
        Ok(Self {
            transforms: objects.iter().map(|o| (o.id, o.transform)).collect(),
            parents: objects
                .iter()
                .filter_map(|o| o.parent.map(|p| (o.id, p)))
                .collect(),
            colors: objects.iter().map(|o| (o.id, o.color)).collect(),
            angular_velocities: objects.iter().map(|o| (o.id, [0.; 3])).collect(),
            time: 0.,
        })
    }
    /// Build the demo independently as Transform/Parent/Color/Motion tables.
    pub fn demo() -> Self {
        Self {
            transforms: BTreeMap::from([
                (1, transform([-1.35, 0.0, 0.0], [0.8; 3])),
                (2, transform([0.0, 2.0, 0.0], [0.32; 3])),
                (3, transform([1.5, -0.15, 0.2], [0.65; 3])),
                (4, transform([0.0, -1.0, 0.0], [4.0, 0.08, 3.0])),
            ]),
            parents: BTreeMap::from([(2, 1)]),
            colors: BTreeMap::from([
                (1, [80, 175, 245, 255]),
                (2, [255, 182, 75, 255]),
                (3, [189, 128, 240, 255]),
                (4, [79, 103, 111, 255]),
            ]),
            angular_velocities: BTreeMap::from([
                (1, [0.18, 0.65, 0.0]),
                (2, [0.3, -0.7, 0.0]),
                (3, [0.25, -0.45, 0.0]),
                (4, [0.; 3]),
            ]),
            time: 0.0,
        }
    }
}

impl WorldProvider for EcsWorld {
    fn objects(&self) -> Result<Vec<WorldObject>, String> {
        Ok(self
            .transforms
            .iter()
            .map(|(id, transform)| WorldObject {
                id: *id,
                parent: self.parents.get(id).copied(),
                transform: *transform,
                color: self.colors[id],
            })
            .collect())
    }

    fn apply_commands(&mut self, commands: &[WorldCommand]) -> Result<(), String> {
        let mut objects = self.objects()?.into_iter().map(|o| (o.id, o)).collect();
        crate::control::apply_object_commands(&mut objects, commands)?;
        // DTO validation completed before touching any component table.
        self.transforms = objects.values().map(|o| (o.id, o.transform)).collect();
        self.parents = objects
            .values()
            .filter_map(|o| o.parent.map(|p| (o.id, p)))
            .collect();
        self.colors = objects.values().map(|o| (o.id, o.color)).collect();
        self.angular_velocities
            .retain(|id, _| objects.contains_key(id));
        for command in commands {
            if let WorldCommand::Spawn { object } = command {
                self.angular_velocities.remove(&object.id);
            }
        }
        for id in objects.keys() {
            self.angular_velocities.entry(*id).or_insert([0.; 3]);
        }
        Ok(())
    }

    fn step(&mut self, dt: f32, speed: f32) {
        self.time += dt;
        let amount = dt * speed;
        for (entity, velocity) in &self.angular_velocities {
            if let Some(transform) = self.transforms.get_mut(entity) {
                for (i, component) in velocity.iter().enumerate() {
                    transform.rotation[i] += component * amount;
                }
            }
        }
    }

    fn snapshot(&self) -> SceneSnapshot {
        fn resolve(
            entity: u64,
            transforms: &BTreeMap<u64, Transform>,
            parents: &BTreeMap<u64, u64>,
            cache: &mut BTreeMap<u64, [[f32; 4]; 4]>,
        ) -> [[f32; 4]; 4] {
            if let Some(model) = cache.get(&entity) {
                return *model;
            }
            let parent = parents
                .get(&entity)
                .map_or_else(identity_matrix, |p| resolve(*p, transforms, parents, cache));
            let model = multiply_matrices(parent, transforms[&entity].matrix());
            cache.insert(entity, model);
            model
        }
        let mut cache = BTreeMap::new();
        let items = self
            .transforms
            .keys()
            .map(|entity| RenderItem {
                id: *entity,
                mesh: Mesh::cube(),
                model: resolve(*entity, &self.transforms, &self.parents, &mut cache),
                color: self.colors[entity],
                material: Material::default(),
            })
            .collect();
        SceneSnapshot {
            items,
            time: self.time,
        }
    }
}

fn transform(translation: [f32; 3], scale: [f32; 3]) -> Transform {
    Transform {
        translation,
        scale,
        ..Transform::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_hierarchy_constructs_independent_stores_without_demo_velocity() {
        let data = vec![
            WorldObject {
                id: 20,
                parent: Some(10),
                transform: transform([0., 2., 0.], [1.; 3]),
                color: [255; 4],
            },
            WorldObject {
                id: 10,
                parent: None,
                transform: transform([3., 0., 0.], [2.; 3]),
                color: [200; 4],
            },
        ];
        let mut object = ObjectsWorld::from_objects(data.clone()).unwrap();
        let mut ecs = EcsWorld::from_objects(data).unwrap();
        let before = object.snapshot();
        object.step(2., 10.);
        ecs.step(2., 10.);
        assert_eq!(object.objects().unwrap(), ecs.objects().unwrap());
        assert_eq!(object.snapshot(), ecs.snapshot());
        assert_eq!(before.items, object.snapshot().items);
        let duplicate = vec![
            WorldObject {
                id: 1,
                parent: None,
                transform: Transform::default(),
                color: [255; 4]
            };
            2
        ];
        assert!(ObjectsWorld::from_objects(duplicate.clone()).is_err());
        assert!(EcsWorld::from_objects(duplicate).is_err());
    }

    #[test]
    fn independent_worlds_extract_identical_hierarchical_scene_over_time() {
        let mut objects = ObjectsWorld::demo();
        let mut ecs = EcsWorld::demo();
        assert_eq!(objects.snapshot(), ecs.snapshot());
        let initial_child = objects.snapshot().items[1].model;
        for (dt, speed) in [(0.016, 1.0), (0.031, -0.4), (0.1, 2.0), (0.0, 0.0)] {
            objects.step(dt, speed);
            ecs.step(dt, speed);
            assert_eq!(objects.snapshot(), ecs.snapshot());
        }
        assert_ne!(objects.snapshot().items[1].model, initial_child);
        assert_eq!(objects.snapshot().items.len(), 4);
    }

    #[test]
    fn portable_commands_edit_both_stores_identically_and_roll_back_invalid_batches() {
        let mut objects = ObjectsWorld::demo();
        let mut ecs = EcsWorld::demo();
        let commands = vec![
            WorldCommand::SetPosition {
                id: 1,
                position: [2., 1., 0.],
            },
            WorldCommand::SetColor {
                id: 2,
                color: [10, 220, 80, 255],
            },
            WorldCommand::Spawn {
                object: WorldObject {
                    id: 9,
                    parent: Some(1),
                    transform: Transform {
                        translation: [0., 3., 0.],
                        ..Transform::default()
                    },
                    color: [200; 4],
                },
            },
        ];
        objects.apply_commands(&commands).unwrap();
        ecs.apply_commands(&commands).unwrap();
        objects.step(0.2, 1.);
        ecs.step(0.2, 1.);
        assert_eq!(objects.objects().unwrap(), ecs.objects().unwrap());
        assert_eq!(objects.snapshot(), ecs.snapshot());
        let before = objects.snapshot();
        let bad = [
            WorldCommand::SetColor {
                id: 1,
                color: [0; 4],
            },
            WorldCommand::Translate {
                id: 999,
                delta: [1., 0., 0.],
            },
        ];
        assert!(objects.apply_commands(&bad).is_err());
        assert!(ecs.apply_commands(&bad).is_err());
        assert_eq!(before, objects.snapshot());
        assert_eq!(before, ecs.snapshot());
        objects
            .apply_commands(&[WorldCommand::Remove { id: 1 }])
            .unwrap();
        ecs.apply_commands(&[WorldCommand::Remove { id: 1 }])
            .unwrap();
        assert_eq!(
            objects
                .objects()
                .unwrap()
                .iter()
                .map(|o| o.id)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(objects.snapshot(), ecs.snapshot());
    }

    #[test]
    fn invalid_parent_cycles_and_nonfinite_values_do_not_mutate_worlds() {
        for mut world in [
            Box::new(ObjectsWorld::demo()) as Box<dyn WorldProvider>,
            Box::new(EcsWorld::demo()),
        ] {
            let before = world.snapshot();
            let bad = WorldCommand::Spawn {
                object: WorldObject {
                    id: 10,
                    parent: Some(10),
                    transform: Transform::default(),
                    color: [255; 4],
                },
            };
            assert!(world.apply_commands(&[bad]).is_err());
            assert!(
                world
                    .apply_commands(&[WorldCommand::SetPosition {
                        id: 1,
                        position: [f32::NAN, 0., 0.]
                    }])
                    .is_err()
            );
            assert_eq!(before, world.snapshot());
        }
    }
    #[test]
    fn replacing_a_removed_id_spawns_a_new_object_without_previous_motion() {
        let mut objects = ObjectsWorld::demo();
        let mut ecs = EcsWorld::demo();
        let commands = [
            WorldCommand::Remove { id: 1 },
            WorldCommand::Spawn {
                object: WorldObject {
                    id: 1,
                    parent: None,
                    transform: Transform::default(),
                    color: [255; 4],
                },
            },
        ];
        objects.apply_commands(&commands).unwrap();
        ecs.apply_commands(&commands).unwrap();
        objects.step(1., 1.);
        ecs.step(1., 1.);
        assert_eq!(
            objects.objects().unwrap()[0].transform,
            Transform::default()
        );
        assert_eq!(objects.snapshot(), ecs.snapshot());
    }
}
