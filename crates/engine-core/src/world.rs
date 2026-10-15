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

/// Canonical entity/component storage. Owned DTOs are extracted at boundaries;
/// systems query component columns directly and never share a second entity map.
#[derive(Clone, Debug)]
pub struct EcsWorld {
    transforms: BTreeMap<u64, Transform>,
    parents: BTreeMap<u64, u64>,
    colors: BTreeMap<u64, [u8; 4]>,
    angular_velocities: BTreeMap<u64, [f32; 3]>,
    components: BTreeMap<String, BTreeMap<u64, serde_json::Value>>,
    references: BTreeMap<u64, BTreeMap<String, u64>>,
    lifetimes: BTreeMap<u64, u64>,
    time: f32,
}

fn new_entity_lifetime() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.try_update(
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
        |value| value.checked_add(1),
    )
    .expect("Entity lifetime token space exhausted")
}

/// Runtime lifetime tokens are private identity bookkeeping, outside the saved
/// semantic DTO state. Clone retains them; deserialization creates new ones.
impl PartialEq for EcsWorld {
    fn eq(&self, other: &Self) -> bool {
        self.transforms == other.transforms
            && self.parents == other.parents
            && self.colors == other.colors
            && self.angular_velocities == other.angular_velocities
            && self.components == other.components
            && self.references == other.references
            && self.time == other.time
    }
}

impl EcsWorld {
    /// Construct authored entities in independent component tables, without motion.
    pub fn from_objects(objects: Vec<WorldObject>) -> Result<Self, String> {
        if objects.iter().any(|object| object.id == 0) {
            return Err("Entity IDs must be nonzero".into());
        }
        crate::validate_world_commands(&objects, &[])?;
        Ok(Self {
            transforms: objects.iter().map(|o| (o.id, o.transform)).collect(),
            parents: objects
                .iter()
                .filter_map(|o| o.parent.map(|p| (o.id, p)))
                .collect(),
            colors: objects.iter().map(|o| (o.id, o.color)).collect(),
            angular_velocities: objects.iter().map(|o| (o.id, [0.; 3])).collect(),
            components: BTreeMap::new(),
            references: objects.iter().map(|o| (o.id, BTreeMap::new())).collect(),
            lifetimes: objects
                .iter()
                .map(|o| (o.id, new_entity_lifetime()))
                .collect(),
            time: 0.,
        })
    }
    pub fn from_entities(entities: Vec<crate::GameEntity>) -> Result<Self, String> {
        if entities.iter().any(|entity| entity.object.id == 0) {
            return Err("Entity IDs must be nonzero".into());
        }
        let mut world = Self::from_objects(
            entities
                .iter()
                .map(|entity| entity.object.clone())
                .collect(),
        )?;
        for entity in entities {
            let id = entity.object.id;
            world.references.insert(id, entity.references);
            for (name, value) in entity.components {
                world.insert_component(id, name, value)?;
            }
        }
        world.validate_references()?;
        Ok(world)
    }
    pub fn entity_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.transforms.keys().copied()
    }
    pub fn contains(&self, id: u64) -> bool {
        self.transforms.contains_key(&id)
    }
    /// Opaque process-local identity for module-owned transient state. Tokens
    /// survive checkpoints/ordinary edits but are fresh after spawn or reload.
    pub fn lifetime(&self, id: u64) -> Option<u64> {
        self.lifetimes.get(&id).copied()
    }
    pub fn len(&self) -> usize {
        self.transforms.len()
    }
    pub fn is_empty(&self) -> bool {
        self.transforms.is_empty()
    }
    /// Query the smallest requested component column, retaining stable ID order.
    pub fn entities_with(&self, required: &[&str]) -> Vec<u64> {
        if required.is_empty() {
            return self.entity_ids().collect();
        }
        let Some(columns) = required
            .iter()
            .map(|name| self.components.get(*name))
            .collect::<Option<Vec<_>>>()
        else {
            return vec![];
        };
        let smallest = columns
            .iter()
            .min_by_key(|column| column.len())
            .expect("Nonempty required columns");
        smallest
            .keys()
            .filter(|id| columns.iter().all(|column| column.contains_key(id)))
            .copied()
            .collect()
    }
    pub fn object(&self, id: u64) -> Option<WorldObject> {
        Some(WorldObject {
            id,
            parent: self.parents.get(&id).copied(),
            transform: *self.transforms.get(&id)?,
            color: self.colors[&id],
        })
    }
    pub fn objects_snapshot(&self) -> Vec<WorldObject> {
        self.entity_ids().filter_map(|id| self.object(id)).collect()
    }
    pub fn transform_mut(&mut self, id: u64) -> Option<&mut Transform> {
        self.transforms.get_mut(&id)
    }
    pub fn component(&self, id: u64, name: &str) -> Option<&serde_json::Value> {
        self.components.get(name)?.get(&id)
    }
    pub fn component_mut(&mut self, id: u64, name: &str) -> Option<&mut serde_json::Value> {
        self.components.get_mut(name)?.get_mut(&id)
    }
    pub fn insert_component(
        &mut self,
        id: u64,
        name: impl Into<String>,
        value: serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = name.into();
        if !self.contains(id) {
            return Err(format!("Entity {id} does not exist"));
        }
        if name.trim().is_empty() {
            return Err("Component ID must be nonempty".into());
        }
        Ok(self.components.entry(name).or_default().insert(id, value))
    }
    pub fn remove_component(&mut self, id: u64, name: &str) -> Option<serde_json::Value> {
        let column = self.components.get_mut(name)?;
        let removed = column.remove(&id);
        if column.is_empty() {
            self.components.remove(name);
        }
        removed
    }
    pub fn references(&self, id: u64) -> Option<&BTreeMap<String, u64>> {
        self.references.get(&id)
    }
    pub fn references_mut(&mut self, id: u64) -> Option<&mut BTreeMap<String, u64>> {
        self.references.get_mut(&id)
    }
    pub fn entity(&self, id: u64) -> Option<crate::GameEntity> {
        Some(crate::GameEntity {
            object: self.object(id)?,
            components: self
                .components
                .iter()
                .filter_map(|(name, column)| {
                    column.get(&id).map(|value| (name.clone(), value.clone()))
                })
                .collect(),
            references: self.references.get(&id).cloned().unwrap_or_default(),
        })
    }
    pub fn entities_snapshot(&self) -> Vec<crate::GameEntity> {
        self.entity_ids().filter_map(|id| self.entity(id)).collect()
    }
    /// Replace one entity's complete DTO without treating the update as a new
    /// lifetime. Structural validation completes before any column is changed.
    /// References may be transiently dangling while systems stage a tick; the
    /// session validates all referents before publication.
    pub fn upsert_entity(&mut self, entity: crate::GameEntity) -> Result<(), String> {
        let id = entity.object.id;
        if id == 0 || entity.components.keys().any(|name| name.trim().is_empty()) {
            return Err("Entity needs a nonzero ID and nonempty component IDs".into());
        }
        let mut objects: BTreeMap<_, _> = self
            .objects_snapshot()
            .into_iter()
            .map(|object| (object.id, object))
            .collect();
        objects.insert(id, entity.object.clone());
        crate::validate_world_commands(&objects.into_values().collect::<Vec<_>>(), &[])?;
        self.transforms.insert(id, entity.object.transform);
        self.colors.insert(id, entity.object.color);
        if let Some(parent) = entity.object.parent {
            self.parents.insert(id, parent);
        } else {
            self.parents.remove(&id);
        }
        self.angular_velocities.entry(id).or_insert([0.; 3]);
        self.lifetimes.entry(id).or_insert_with(new_entity_lifetime);
        for column in self.components.values_mut() {
            column.remove(&id);
        }
        self.components.retain(|_, column| !column.is_empty());
        for (name, value) in entity.components {
            self.components.entry(name).or_default().insert(id, value);
        }
        self.references.insert(id, entity.references);
        Ok(())
    }
    pub fn validate_references(&self) -> Result<(), String> {
        for (id, references) in &self.references {
            for (name, target) in references {
                if !self.contains(*target) {
                    return Err(format!(
                        "Object {id} reference '{name}' points to missing {target}; clear references before removing their targets"
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn clear(&mut self) {
        self.transforms.clear();
        self.parents.clear();
        self.colors.clear();
        self.angular_velocities.clear();
        self.components.clear();
        self.references.clear();
        self.lifetimes.clear();
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
            components: BTreeMap::new(),
            references: (1..=4).map(|id| (id, BTreeMap::new())).collect(),
            lifetimes: (1..=4).map(|id| (id, new_entity_lifetime())).collect(),
            time: 0.0,
        }
    }
}

impl WorldProvider for EcsWorld {
    fn objects(&self) -> Result<Vec<WorldObject>, String> {
        Ok(self.objects_snapshot())
    }

    fn apply_commands(&mut self, commands: &[WorldCommand]) -> Result<(), String> {
        if commands
            .iter()
            .any(|command| matches!(command, WorldCommand::Spawn { object } if object.id == 0))
        {
            return Err("Entity IDs must be nonzero".into());
        }
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
        for column in self.components.values_mut() {
            column.retain(|id, _| objects.contains_key(id));
        }
        self.references.retain(|id, _| objects.contains_key(id));
        self.lifetimes.retain(|id, _| objects.contains_key(id));
        for command in commands {
            if let WorldCommand::Spawn { object } = command {
                self.angular_velocities.remove(&object.id);
                for column in self.components.values_mut() {
                    column.remove(&object.id);
                }
                self.references.remove(&object.id);
                self.lifetimes.remove(&object.id);
            }
        }
        self.components.retain(|_, column| !column.is_empty());
        for id in objects.keys() {
            self.angular_velocities.entry(*id).or_insert([0.; 3]);
            self.references.entry(*id).or_default();
            self.lifetimes
                .entry(*id)
                .or_insert_with(new_entity_lifetime);
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

/// GameState's legacy wire field remains a keyed entity DTO map. Runtime
/// storage is reconstructed only after IDs, hierarchy and references validate.
pub(crate) mod entity_dto_map {
    use super::EcsWorld;
    use crate::GameEntity;
    use serde::{
        Deserializer, Serializer,
        de::{Error, MapAccess, Visitor},
    };
    use std::collections::BTreeSet;

    pub fn serialize<S: Serializer>(world: &EcsWorld, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(
            world
                .entity_ids()
                .map(|id| (id, world.entity(id).expect("Canonical entity ID"))),
        )
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<EcsWorld, D::Error> {
        struct EntityMap;
        impl<'de> Visitor<'de> for EntityMap {
            type Value = EcsWorld;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map of stable IDs to entity DTOs")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut ids = BTreeSet::new();
                let mut entities = vec![];
                while let Some((id, entity)) = map.next_entry::<u64, GameEntity>()? {
                    if id != entity.object.id || !ids.insert(id) {
                        return Err(A::Error::custom(
                            "Entity map keys must match unique stable object IDs",
                        ));
                    }
                    entities.push(entity);
                }
                EcsWorld::from_entities(entities).map_err(A::Error::custom)
            }
        }
        deserializer.deserialize_map(EntityMap)
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
    fn entity(
        id: u64,
        parent: Option<u64>,
        components: &[(&str, serde_json::Value)],
    ) -> crate::GameEntity {
        crate::GameEntity {
            object: WorldObject {
                id,
                parent,
                transform: Transform::default(),
                color: [255; 4],
            },
            components: components
                .iter()
                .map(|(name, value)| ((*name).into(), value.clone()))
                .collect(),
            references: BTreeMap::new(),
        }
    }
    #[test]
    fn component_column_queries_and_owned_dtos_keep_one_mutable_store() {
        let mut world = EcsWorld::from_entities(vec![
            entity(
                3,
                None,
                &[
                    ("a", serde_json::json!({"value":3})),
                    ("b", serde_json::json!({})),
                ],
            ),
            entity(1, None, &[("a", serde_json::json!({"value":1}))]),
            entity(2, None, &[("b", serde_json::json!({}))]),
        ])
        .unwrap();
        assert_eq!(world.entity_ids().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(world.entities_with(&[]), vec![1, 2, 3]);
        assert_eq!(world.entities_with(&["a"]), vec![1, 3]);
        assert_eq!(world.entities_with(&["a", "b"]), vec![3]);
        assert!(world.entities_with(&["missing", "a"]).is_empty());
        world.references_mut(1).unwrap().insert("owner".into(), 3);
        let mut dto = world.entity(1).unwrap();
        dto.components.get_mut("a").unwrap()["value"] = serde_json::json!(99);
        assert_eq!(world.component(1, "a").unwrap()["value"], 1);
        world.component_mut(1, "a").unwrap()["value"] = serde_json::json!(4);
        assert_eq!(world.entity(1).unwrap().components["a"]["value"], 4);
        let lifetime = world.lifetime(3).unwrap();
        world
            .apply_commands(&[WorldCommand::SetTransform {
                id: 3,
                transform: Transform::default(),
            }])
            .unwrap();
        assert_eq!(world.lifetime(3), Some(lifetime));
        let mut child = world.entity(3).unwrap();
        child.object.parent = Some(2);
        world.upsert_entity(child).unwrap();
        assert_eq!(world.lifetime(3), Some(lifetime));
        assert_eq!(world.entities_with(&["a", "b"]), vec![3]);
        assert_eq!(world.references(1).unwrap()["owner"], 3);
        world.remove_component(3, "a");
        world
            .insert_component(2, "a", serde_json::json!({"value":2}))
            .unwrap();
        assert_eq!(world.entities_with(&["a", "b"]), vec![2]);
        assert!(
            world
                .insert_component(999, "a", serde_json::json!({}))
                .is_err()
        );
    }
    #[test]
    fn subtree_reused_ids_clear_all_columns_references_and_motion_in_one_batch() {
        let mut parent = entity(1, None, &[("marker", serde_json::json!({"old":true}))]);
        parent.references.insert("child".into(), 2);
        let mut world = EcsWorld::from_entities(vec![
            parent,
            entity(2, Some(1), &[("marker", serde_json::json!({}))]),
        ])
        .unwrap();
        world.angular_velocities.insert(1, [1., 0., 0.]);
        let old_lifetimes = [world.lifetime(1).unwrap(), world.lifetime(2).unwrap()];
        world
            .apply_commands(&[
                WorldCommand::Remove { id: 1 },
                WorldCommand::Spawn {
                    object: entity(1, None, &[]).object,
                },
                WorldCommand::Spawn {
                    object: entity(2, Some(1), &[]).object,
                },
            ])
            .unwrap();
        assert!(world.entities_with(&["marker"]).is_empty());
        assert!(world.references(1).unwrap().is_empty());
        assert!(world.references(2).unwrap().is_empty());
        assert_ne!(world.lifetime(1), Some(old_lifetimes[0]));
        assert_ne!(world.lifetime(2), Some(old_lifetimes[1]));
        world.step(1., 1.);
        assert_eq!(world.object(1).unwrap().transform.rotation, [0.; 3]);
        world
            .insert_component(2, "fresh", serde_json::json!({"value":5}))
            .unwrap();
        world.references_mut(2).unwrap().insert("parent".into(), 1);
        let before = world.clone();
        let lifetime = world.lifetime(1);
        assert!(
            world
                .apply_commands(&[
                    WorldCommand::Remove { id: 1 },
                    WorldCommand::Spawn {
                        object: entity(3, Some(999), &[]).object
                    }
                ])
                .is_err()
        );
        assert_eq!(world, before);
        assert_eq!(world.lifetime(1), lifetime);
        world.clear();
        assert_eq!(world.lifetime(1), None);
        world
            .apply_commands(&[WorldCommand::Spawn {
                object: entity(1, None, &[]).object,
            }])
            .unwrap();
        assert_ne!(world.lifetime(1), lifetime);
    }

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
