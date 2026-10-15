//! Renderer-neutral selection and transform gestures for authored scenes.
//!
//! Picking consumes the same final meshes/model matrices as rendering. Gestures
//! keep an immutable start transform: every update is absolute, cancellation
//! restores that transform, and the UI can record one undo entry on release.
//! World rotation/scaling is rejected when its result cannot be stored as the
//! project's local TRS (for example shear under a nonuniformly scaled parent).
use eframe::egui::{Pos2, Rect, Vec2};
use module_engine_core::{
    Camera, SceneSnapshot, Transform, WorldObject, identity_matrix, multiply_matrices,
    transform_point,
};
use std::collections::{BTreeMap, BTreeSet};

pub type Matrix = [[f32; 4]; 4];
const EPSILON: f32 = 1.0e-6;
const NEAR: f32 = 0.08;
const FAR: f32 = 2000.;
const HANDLE_PIXELS: f32 = 84.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Translate,
    Rotate,
    Scale,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}
impl Axis {
    pub const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];
    pub fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
    pub fn direction(self) -> [f32; 3] {
        let mut result = [0.; 3];
        result[self.index()] = 1.;
        result
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Space {
    #[default]
    World,
    Local,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub enabled: bool,
    /// Translation step in world units along the chosen axis.
    pub translation: f32,
    pub rotation_degrees: f32,
    /// Scale factor step: 0.1 changes a factor of 1 to 1.1, 1.2, etc.
    pub scale: f32,
}
impl Default for Snap {
    fn default() -> Self {
        Self {
            enabled: false,
            translation: 0.5,
            rotation_degrees: 15.,
            scale: 0.1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: [f32; 3],
    pub direction: [f32; 3],
    pub min_distance: f32,
    pub max_distance: f32,
}
impl Ray {
    /// An unclipped ray; camera_ray supplies the renderer's near/far planes.
    pub fn new(origin: [f32; 3], direction: [f32; 3]) -> Option<Self> {
        if !finite(origin) {
            return None;
        }
        Some(Self {
            origin,
            direction: normalize(direction)?,
            min_distance: 0.,
            max_distance: f32::INFINITY,
        })
    }
    pub fn at(self, distance: f32) -> [f32; 3] {
        add(self.origin, mul(self.direction, distance))
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickHit {
    pub id: u64,
    /// Distance from the camera eye along its normalized ray.
    pub distance: f32,
    pub point: [f32; 3],
}

#[derive(Clone, Copy)]
struct CameraBasis {
    eye: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    forward: [f32; 3],
    aspect: f32,
    tangent: f32,
}
fn camera_basis(camera: Camera, rect: Rect) -> Option<CameraBasis> {
    if !rect.is_finite() || rect.width() <= 0. || rect.height() <= 0. {
        return None;
    }
    // These are the established GPU projection constants, not a Vulkan handle.
    let target = camera.target.map(|v| if v.is_finite() { v } else { 0. });
    let yaw = if camera.yaw.is_finite() {
        camera.yaw
    } else {
        0.55
    };
    let pitch = if camera.pitch.is_finite() {
        camera.pitch.clamp(-1.45, 1.45)
    } else {
        0.3
    };
    let distance = if camera.distance.is_finite() {
        camera.distance.clamp(0.2, 1000.)
    } else {
        8.
    };
    let eye = add(
        target,
        [
            yaw.sin() * pitch.cos() * distance,
            pitch.sin() * distance,
            yaw.cos() * pitch.cos() * distance,
        ],
    );
    let forward = normalize(sub(target, eye))?;
    let right = normalize(cross(forward, [0., 1., 0.]))?;
    let up = cross(right, forward);
    Some(CameraBasis {
        eye,
        right,
        up,
        forward,
        aspect: rect.aspect_ratio(),
        tangent: (55_f32.to_radians() * 0.5).tan(),
    })
}

/// Project a world point using the renderer's 55-degree perspective camera.
/// Points outside the area retain valid coordinates; near/behind points do not.
pub fn project_point(camera: Camera, rect: Rect, point: [f32; 3]) -> Option<Pos2> {
    let basis = camera_basis(camera, rect)?;
    if !finite(point) {
        return None;
    }
    let delta = sub(point, basis.eye);
    let depth = dot(delta, basis.forward);
    if !(NEAR..=FAR).contains(&depth) {
        return None;
    }
    let x = dot(delta, basis.right) / (depth * basis.tangent * basis.aspect);
    let y = dot(delta, basis.up) / (depth * basis.tangent);
    let result = Pos2::new(
        rect.center().x + x * rect.width() * 0.5,
        rect.center().y - y * rect.height() * 0.5,
    );
    result.is_finite().then_some(result)
}

pub fn camera_ray(camera: Camera, rect: Rect, cursor: Pos2) -> Option<Ray> {
    if !cursor.is_finite() {
        return None;
    }
    let basis = camera_basis(camera, rect)?;
    let x = 2. * (cursor.x - rect.center().x) / rect.width();
    let y = -2. * (cursor.y - rect.center().y) / rect.height();
    let ray = Ray::new(
        basis.eye,
        add(
            basis.forward,
            add(
                mul(basis.right, x * basis.aspect * basis.tangent),
                mul(basis.up, y * basis.tangent),
            ),
        ),
    )?;
    let plane_rate = dot(ray.direction, basis.forward);
    Some(Ray {
        min_distance: NEAR / plane_rate,
        max_distance: FAR / plane_rate,
        ..ray
    })
}

/// Nearest visible triangle, including imported multi-primitive geometry.
/// Triangle tests are double-sided, so selection does not depend on mesh winding.
pub fn pick(scene: &SceneSnapshot, ray: Ray) -> Option<PickHit> {
    if !finite(ray.origin) || !finite(ray.direction) {
        return None;
    }
    let mut nearest: Option<PickHit> = None;
    for item in &scene.items {
        for triangle in &item.mesh.triangles {
            let [Some(a), Some(b), Some(c)] = triangle.map(|index| item.mesh.vertices.get(index))
            else {
                continue;
            };
            let [a, b, c] = [*a, *b, *c].map(|p| transform_point(item.model, p));
            if let Some(distance) = ray_triangle(ray, a, b, c)
                && nearest.as_ref().is_none_or(|hit| distance < hit.distance)
            {
                nearest = Some(PickHit {
                    id: item.id,
                    distance,
                    point: ray.at(distance),
                });
            }
        }
    }
    nearest
}
fn ray_triangle(ray: Ray, a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<f32> {
    if !finite(a) || !finite(b) || !finite(c) {
        return None;
    }
    let ab = sub(b, a);
    let ac = sub(c, a);
    let h = cross(ray.direction, ac);
    let determinant = dot(ab, h);
    if determinant.abs() < EPSILON {
        return None;
    }
    let relative = sub(ray.origin, a);
    let u = dot(relative, h) / determinant;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(relative, ab);
    let v = dot(ray.direction, q) / determinant;
    if v < 0. || u + v > 1. {
        return None;
    }
    let distance = dot(ac, q) / determinant;
    (distance.is_finite() && distance >= ray.min_distance.max(0.) && distance <= ray.max_distance)
        .then_some(distance)
}

/// Intersect an arbitrary plane; returns no result when parallel or behind eye.
pub fn plane_hit(ray: Ray, point: [f32; 3], normal: [f32; 3]) -> Option<[f32; 3]> {
    let denominator = dot(ray.direction, normal);
    if denominator.abs() < EPSILON {
        return None;
    }
    let distance = dot(sub(point, ray.origin), normal) / denominator;
    (distance.is_finite() && distance >= ray.min_distance.max(0.) && distance <= ray.max_distance)
        .then(|| ray.at(distance))
}
/// Asset placement uses the nearest surface, or the horizontal ground plane.
pub fn placement(scene: &SceneSnapshot, ray: Ray) -> Option<[f32; 3]> {
    pick(scene, ray)
        .map(|hit| hit.point)
        .or_else(|| plane_hit(ray, [0.; 3], [0., 1., 0.]))
}

#[derive(Clone, Copy, Debug)]
pub struct ObjectMatrices {
    pub world: Matrix,
    pub parent: Matrix,
}
pub fn object_matrices(objects: &[WorldObject], id: u64) -> Result<ObjectMatrices, String> {
    let by_id = objects
        .iter()
        .map(|o| (o.id, o))
        .collect::<BTreeMap<_, _>>();
    if by_id.len() != objects.len() {
        return Err("Scene has duplicate object IDs".into());
    }
    let object = by_id.get(&id).ok_or("Selected object is missing")?;
    let mut chain = vec![*object];
    let mut visited = BTreeSet::from([id]);
    let mut parent = object.parent;
    while let Some(id) = parent {
        if !visited.insert(id) {
            return Err("Scene hierarchy contains a cycle".into());
        }
        let object = by_id.get(&id).ok_or("Scene parent is missing")?;
        chain.push(*object);
        parent = object.parent;
    }
    let mut parent = identity_matrix();
    for ancestor in chain.iter().skip(1).rev() {
        if !valid_transform(ancestor.transform) {
            return Err("Scene transform is not finite".into());
        }
        parent = multiply_matrices(parent, ancestor.transform.matrix());
    }
    if !valid_transform(object.transform) {
        return Err("Scene transform is not finite".into());
    }
    Ok(ObjectMatrices {
        world: multiply_matrices(parent, object.transform.matrix()),
        parent,
    })
}

/// Invert an affine parent matrix. A zero/singular parent cannot map world edits.
pub fn inverse_affine(matrix: Matrix) -> Result<Matrix, String> {
    if !matrix.iter().flatten().all(|v| v.is_finite()) {
        return Err("Parent matrix is not finite".into());
    }
    if matrix[3][..3].iter().any(|v| v.abs() > EPSILON) || (matrix[3][3] - 1.).abs() > EPSILON {
        return Err("Parent matrix must be affine".into());
    }
    let [a, b, c] = [column(matrix, 0), column(matrix, 1), column(matrix, 2)];
    let determinant = dot(a, cross(b, c));
    let size = length(a) * length(b) * length(c);
    if !size.is_finite() || size <= f32::MIN_POSITIVE || determinant.abs() <= size * EPSILON {
        return Err("Parent transform is singular; this gesture cannot be applied".into());
    }
    let rows = [cross(b, c), cross(c, a), cross(a, b)].map(|row| mul(row, 1. / determinant));
    let translation = origin(matrix);
    let mut inverse = identity_matrix();
    for row in 0..3 {
        inverse[row][..3].copy_from_slice(&rows[row]);
        inverse[row][3] = -dot(rows[row], translation);
    }
    Ok(inverse)
}

/// Convert a desired world matrix to exact local T*Rz*Ry*Rx*S storage.
pub fn world_to_local(parent: Matrix, desired: Matrix) -> Result<Transform, String> {
    decompose_trs(multiply_matrices(inverse_affine(parent)?, desired))
}
fn decompose_trs(matrix: Matrix) -> Result<Transform, String> {
    if matrix[3][..3].iter().any(|v| v.abs() > EPSILON) || (matrix[3][3] - 1.).abs() > EPSILON {
        return Err("Desired world transform must be affine".into());
    }
    let columns = [column(matrix, 0), column(matrix, 1), column(matrix, 2)];
    let mut scale = columns.map(length);
    if !matrix.iter().flatten().all(|v| v.is_finite()) || scale.iter().any(|s| *s <= EPSILON) {
        return Err("Gesture would create a zero or invalid scale".into());
    }
    let determinant = dot(columns[0], cross(columns[1], columns[2]));
    // A reflection is representable; keep it in one signed scale component.
    if determinant < 0. {
        scale[0] = -scale[0];
    }
    let normalized: [[f32; 3]; 3] = std::array::from_fn(|i| mul(columns[i], 1. / scale[i]));
    if dot(normalized[0], normalized[1]).abs() > 0.0002
        || dot(normalized[0], normalized[2]).abs() > 0.0002
        || dot(normalized[1], normalized[2]).abs() > 0.0002
    {
        return Err(
            "World gesture would produce shear; use Local axes or change the parent scale".into(),
        );
    }
    // Rz * Ry * Rx, matching Transform::matrix. At gimbal lock choose z=0.
    let sy = (-normalized[0][2]).clamp(-1., 1.);
    let y = sy.asin();
    let (x, z) = if y.cos().abs() > 0.0001 {
        (
            normalized[1][2].atan2(normalized[2][2]),
            normalized[0][1].atan2(normalized[0][0]),
        )
    } else {
        ((sy * normalized[1][0]).atan2(normalized[1][1]), 0.)
    };
    let result = Transform {
        translation: origin(matrix),
        rotation: [x, y, z],
        scale,
    };
    let rebuilt = result.matrix();
    let magnitude = matrix.iter().flatten().map(|v| v.abs()).fold(1., f32::max);
    if matrix
        .iter()
        .flatten()
        .zip(rebuilt.iter().flatten())
        .any(|(a, b)| (a - b).abs() > 0.0005 * magnitude)
    {
        return Err("World gesture cannot be represented by the local transform".into());
    }
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct GizmoAxis {
    pub axis: Axis,
    pub start: Pos2,
    pub end: Pos2,
    pub direction: [f32; 3],
}
#[derive(Clone, Debug)]
pub struct GizmoRing {
    pub axis: Axis,
    /// Consecutive projected points; each pair is a visible line segment.
    pub segments: Vec<[Pos2; 2]>,
}
#[derive(Clone, Debug)]
pub struct GizmoGeometry {
    pub center: Pos2,
    pub axes: Vec<GizmoAxis>,
    pub rings: Vec<GizmoRing>,
}
impl GizmoGeometry {
    pub fn build(
        objects: &[WorldObject],
        id: u64,
        tool: Tool,
        space: Space,
        camera: Camera,
        rect: Rect,
    ) -> Result<Self, String> {
        let matrices = object_matrices(objects, id)?;
        let pivot = origin(matrices.world);
        let center =
            project_point(camera, rect, pivot).ok_or("Selected object is outside camera depth")?;
        let basis = camera_basis(camera, rect).ok_or("Scene area has no valid size")?;
        let depth = dot(sub(pivot, basis.eye), basis.forward);
        let world_radius = 2. * depth * basis.tangent * HANDLE_PIXELS / rect.height();
        let mut result = Self {
            center,
            axes: vec![],
            rings: vec![],
        };
        for axis in Axis::ALL {
            let direction = axis_direction(matrices.world, space, axis)?;
            if tool == Tool::Rotate {
                let reference = if direction[1].abs() < 0.8 {
                    [0., 1., 0.]
                } else {
                    [1., 0., 0.]
                };
                let u = normalize(cross(direction, reference)).ok_or("Invalid rotation axis")?;
                let v = cross(direction, u);
                let points = (0..=64)
                    .map(|i| {
                        let angle = i as f32 * std::f32::consts::TAU / 64.;
                        project_point(
                            camera,
                            rect,
                            add(
                                pivot,
                                mul(add(mul(u, angle.cos()), mul(v, angle.sin())), world_radius),
                            ),
                        )
                    })
                    .collect::<Vec<_>>();
                result.rings.push(GizmoRing {
                    axis,
                    segments: points
                        .windows(2)
                        .filter_map(|pair| Some([pair[0]?, pair[1]?]))
                        .collect(),
                });
            } else if let Some(end) =
                project_point(camera, rect, add(pivot, mul(direction, world_radius)))
            {
                result.axes.push(GizmoAxis {
                    axis,
                    start: center,
                    end,
                    direction,
                });
            }
        }
        Ok(result)
    }
    /// Screen-space hover tolerance stays independent of scene units/zoom.
    pub fn hit_test(&self, pointer: Pos2) -> Option<Axis> {
        let mut nearest = None;
        let mut best = 9.;
        for axis in &self.axes {
            let length = axis.start.distance(axis.end);
            if length < 15. {
                continue;
            }
            // Leave a small center zone so axes do not all compete at the pivot.
            let start = axis.start + (axis.end - axis.start) * (12. / length);
            let distance = segment_distance(pointer, start, axis.end);
            if distance < best {
                best = distance;
                nearest = Some(axis.axis);
            }
        }
        for ring in &self.rings {
            for &[a, b] in &ring.segments {
                let distance = segment_distance(pointer, a, b);
                if distance < best {
                    best = distance;
                    nearest = Some(ring.axis);
                }
            }
        }
        nearest
    }
}

/// Immutable gesture baseline. The UI owns commit/undo and can use cancel on Esc.
#[derive(Clone, Debug)]
pub struct DragGesture {
    pub id: u64,
    pub tool: Tool,
    pub space: Space,
    pub axis: Axis,
    initial: Transform,
    matrices: ObjectMatrices,
    camera: Camera,
    rect: Rect,
    start: Pos2,
    pivot: [f32; 3],
    direction: [f32; 3],
    screen_center: Pos2,
    screen_direction: Vec2,
    units_per_pixel: f32,
    start_axis: Option<f32>,
    start_rotation: Option<[f32; 3]>,
}
impl DragGesture {
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        objects: &[WorldObject],
        id: u64,
        tool: Tool,
        space: Space,
        axis: Axis,
        camera: Camera,
        rect: Rect,
        start: Pos2,
    ) -> Result<Self, String> {
        let initial = objects
            .iter()
            .find(|o| o.id == id)
            .ok_or("Selected object is missing")?
            .transform;
        let matrices = object_matrices(objects, id)?;
        // Translate and world R/S need this inverse. Reject at gesture start.
        inverse_affine(matrices.parent)?;
        let pivot = origin(matrices.world);
        let direction = axis_direction(matrices.world, space, axis)?;
        let screen_center =
            project_point(camera, rect, pivot).ok_or("Selected object is outside camera depth")?;
        let basis = camera_basis(camera, rect).ok_or("Scene area has no valid size")?;
        let depth = dot(sub(pivot, basis.eye), basis.forward);
        let units_per_pixel = 2. * depth * basis.tangent / rect.height();
        let projected_axis = project_point(
            camera,
            rect,
            add(pivot, mul(direction, units_per_pixel * HANDLE_PIXELS)),
        )
        .map(|end| end - screen_center)
        .unwrap_or(Vec2::ZERO);
        let screen_direction = if projected_axis.length_sq() > 1. {
            projected_axis.normalized()
        } else {
            Vec2::new(0., -1.)
        };
        let ray = camera_ray(camera, rect, start).ok_or("Pointer has no camera ray")?;
        let start_axis = axis_parameter(ray, pivot, direction);
        let start_rotation =
            plane_hit(ray, pivot, direction).and_then(|point| normalize(sub(point, pivot)));
        Ok(Self {
            id,
            tool,
            space,
            axis,
            initial,
            matrices,
            camera,
            rect,
            start,
            pivot,
            direction,
            screen_center,
            screen_direction,
            units_per_pixel,
            start_axis,
            start_rotation,
        })
    }
    pub fn cancel(&self) -> Transform {
        self.initial
    }
    pub fn update(&self, cursor: Pos2, snap: Snap) -> Result<Transform, String> {
        let ray = camera_ray(self.camera, self.rect, cursor).ok_or("Pointer has no camera ray")?;
        let mut result = self.initial;
        match self.tool {
            Tool::Translate => {
                let value = match (
                    self.start_axis,
                    axis_parameter(ray, self.pivot, self.direction),
                ) {
                    (Some(start), Some(current)) => current - start,
                    _ => (cursor - self.start).dot(self.screen_direction) * self.units_per_pixel,
                };
                let value = snapped(value, snap.enabled, snap.translation);
                let desired_position = add(self.pivot, mul(self.direction, value));
                result.translation =
                    transform_point(inverse_affine(self.matrices.parent)?, desired_position);
            }
            Tool::Rotate => {
                let current_rotation = plane_hit(ray, self.pivot, self.direction)
                    .and_then(|point| normalize(sub(point, self.pivot)));
                let angle = match (self.start_rotation, current_rotation) {
                    (Some(start), Some(current)) => {
                        dot(self.direction, cross(start, current)).atan2(dot(start, current))
                    }
                    _ => {
                        let a = self.start - self.screen_center;
                        let b = cursor - self.screen_center;
                        if a.length_sq() > 25. && b.length_sq() > 25. {
                            (a.x * b.y - a.y * b.x).atan2(a.dot(b))
                        } else {
                            (cursor.x - self.start.x) * 0.01
                        }
                    }
                };
                let angle = snapped(angle, snap.enabled, snap.rotation_degrees.to_radians());
                result = self.rotated(angle)?;
            }
            Tool::Scale => {
                let delta = (cursor - self.start).dot(self.screen_direction) / HANDLE_PIXELS;
                let factor = 1. + snapped(delta, snap.enabled, snap.scale);
                if factor <= 0.001 || !factor.is_finite() {
                    return Err("Scale gesture must keep a positive nonzero factor".into());
                }
                result = self.scaled(factor)?;
            }
        }
        if !valid_transform(result) {
            return Err("Gesture produced a nonfinite transform".into());
        }
        Ok(result)
    }
    fn rotated(&self, angle: f32) -> Result<Transform, String> {
        if angle.abs() < EPSILON {
            return Ok(self.initial);
        }
        if self.space == Space::Local {
            let orientation = Transform {
                scale: [1.; 3],
                translation: [0.; 3],
                ..self.initial
            }
            .matrix();
            let mut result = decompose_trs(multiply_matrices(
                orientation,
                rotation_matrix(self.axis.direction(), angle),
            ))?;
            result.translation = self.initial.translation;
            result.scale = self.initial.scale;
            Ok(result)
        } else {
            let desired = linear_about_pivot(
                rotation_matrix(self.direction, angle),
                self.pivot,
                self.matrices.world,
            );
            world_to_local(self.matrices.parent, desired)
        }
    }
    fn scaled(&self, factor: f32) -> Result<Transform, String> {
        if self.space == Space::Local {
            let mut result = self.initial;
            result.scale[self.axis.index()] *= factor;
            Ok(result)
        } else {
            let mut linear = identity_matrix();
            for (row, values) in linear.iter_mut().enumerate().take(3) {
                for (column, value) in values.iter_mut().enumerate().take(3) {
                    *value += (factor - 1.) * self.direction[row] * self.direction[column];
                }
            }
            world_to_local(
                self.matrices.parent,
                linear_about_pivot(linear, self.pivot, self.matrices.world),
            )
        }
    }
}

fn linear_about_pivot(mut linear: Matrix, pivot: [f32; 3], original: Matrix) -> Matrix {
    let displaced = transform_point(linear, pivot);
    for row in 0..3 {
        linear[row][3] = pivot[row] - displaced[row];
    }
    multiply_matrices(linear, original)
}
fn rotation_matrix(axis: [f32; 3], angle: f32) -> Matrix {
    let [x, y, z] = axis;
    let (s, c) = angle.sin_cos();
    let t = 1. - c;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0.],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0.],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0.],
        [0., 0., 0., 1.],
    ]
}
fn axis_direction(matrix: Matrix, space: Space, axis: Axis) -> Result<[f32; 3], String> {
    if space == Space::World {
        return Ok(axis.direction());
    }
    normalize(column(matrix, axis.index())).ok_or_else(|| "Local axis has zero scale".into())
}
fn axis_parameter(ray: Ray, pivot: [f32; 3], axis: [f32; 3]) -> Option<f32> {
    let cosine = dot(ray.direction, axis);
    let denominator = 1. - cosine * cosine;
    if denominator < 0.0025 {
        return None;
    }
    let offset = sub(ray.origin, pivot);
    Some((dot(axis, offset) - cosine * dot(ray.direction, offset)) / denominator)
}
fn snapped(value: f32, enabled: bool, step: f32) -> f32 {
    if enabled && step.is_finite() && step > EPSILON {
        (value / step).round() * step
    } else {
        value
    }
}
fn segment_distance(point: Pos2, a: Pos2, b: Pos2) -> f32 {
    let segment = b - a;
    let length = segment.length_sq();
    if length <= EPSILON {
        return point.distance(a);
    }
    let t = ((point - a).dot(segment) / length).clamp(0., 1.);
    point.distance(a + segment * t)
}
fn column(matrix: Matrix, index: usize) -> [f32; 3] {
    std::array::from_fn(|row| matrix[row][index])
}
fn origin(matrix: Matrix) -> [f32; 3] {
    column(matrix, 3)
}
fn valid_transform(value: Transform) -> bool {
    value
        .translation
        .iter()
        .chain(&value.rotation)
        .chain(&value.scale)
        .all(|v| v.is_finite())
}
fn finite(value: [f32; 3]) -> bool {
    value.iter().all(|v| v.is_finite())
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn mul(a: [f32; 3], scalar: f32) -> [f32; 3] {
    a.map(|v| v * scalar)
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let length = length(a);
    (length.is_finite() && length > EPSILON).then(|| mul(a, 1. / length))
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{Material, Mesh, RenderItem};

    fn area() -> Rect {
        Rect::from_min_size(Pos2::new(100., 50.), Vec2::new(800., 600.))
    }
    fn camera() -> Camera {
        Camera {
            yaw: 0.,
            pitch: 0.35,
            distance: 10.,
            target: [0.; 3],
        }
    }
    fn object(id: u64, parent: Option<u64>, transform: Transform) -> WorldObject {
        WorldObject {
            id,
            parent,
            transform,
            color: [255; 4],
        }
    }
    fn close(a: f32, b: f32) {
        assert!((a - b).abs() < 0.001, "{a} != {b}");
    }
    fn matrix_close(a: Matrix, b: Matrix) {
        for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
            close(*a, *b);
        }
    }
    fn cube(id: u64, position: [f32; 3]) -> RenderItem {
        RenderItem {
            id,
            mesh: Mesh::cube(),
            model: Transform {
                translation: position,
                ..Transform::default()
            }
            .matrix(),
            color: [255; 4],
            material: Material::default(),
        }
    }

    #[test]
    fn projection_ray_agree_at_offset_viewport_and_clip_camera_depth() {
        let rect = area();
        let point = [1.1, 0.8, -0.6];
        let screen = project_point(camera(), rect, point).unwrap();
        let ray = camera_ray(camera(), rect, screen).unwrap();
        let delta = sub(point, ray.origin);
        close(length(cross(delta, ray.direction)), 0.);
        let basis = camera_basis(camera(), rect).unwrap();
        assert!(project_point(camera(), rect, add(basis.eye, mul(basis.forward, 0.04))).is_none());
        assert!(project_point(camera(), rect, sub(basis.eye, basis.forward)).is_none());
        let center = camera_ray(camera(), rect, rect.center()).unwrap();
        close(dot(center.direction, basis.forward), 1.);
        close(center.min_distance, 0.08);
    }
    #[test]
    fn triangle_pick_selects_nearest_actual_mesh_and_retains_duplicate_primitive_id() {
        let ray = Ray::new([0., 0., 10.], [0., 0., -1.]).unwrap();
        let scene = SceneSnapshot {
            items: vec![
                cube(4, [0., 0., 0.]),
                cube(9, [0., 0., 4.]),
                cube(9, [3., 0., 4.]),
            ],
            time: 0.,
        };
        let hit = pick(&scene, ray).unwrap();
        assert_eq!(hit.id, 9);
        close(hit.distance, 5.);
        close(hit.point[2], 5.);
        let hole = SceneSnapshot {
            items: vec![RenderItem {
                mesh: Mesh {
                    vertices: vec![[2., 2., 0.], [3., 2., 0.], [2., 3., 0.]],
                    triangles: vec![[0, 1, 2]],
                    texcoords: vec![],
                },
                ..cube(7, [0.; 3])
            }],
            time: 0.,
        };
        assert!(
            pick(&hole, ray).is_none(),
            "Pick must test triangles rather than oversized bounds"
        );
        assert!(
            pick(
                &scene,
                Ray {
                    min_distance: 7.1,
                    ..ray
                }
            )
            .is_some_and(|h| h.id == 4)
        );
    }
    #[test]
    fn placement_prefers_surface_then_ground_and_rejects_parallel_ray() {
        let ray = Ray::new([0., 10., 0.], [0., -1., 0.]).unwrap();
        let scene = SceneSnapshot {
            items: vec![cube(1, [0.; 3])],
            time: 0.,
        };
        close(placement(&scene, ray).unwrap()[1], 1.);
        close(placement(&SceneSnapshot::default(), ray).unwrap()[1], 0.);
        assert!(
            placement(
                &SceneSnapshot::default(),
                Ray::new([0., 2., 0.], [1., 0., 0.]).unwrap()
            )
            .is_none()
        );
    }
    #[test]
    fn world_local_mapping_respects_rotated_scaled_hierarchy_and_reflection() {
        let parent = Transform {
            translation: [3., 2., -1.],
            rotation: [0.2, 0.7, -0.15],
            scale: [2., 0.7, 1.5],
        };
        let local = Transform {
            translation: [1., 2., 3.],
            rotation: [0.4, -0.2, 0.8],
            scale: [-0.4, 2., 1.1],
        };
        let objects = [object(1, None, parent), object(2, Some(1), local)];
        let matrices = object_matrices(&objects, 2).unwrap();
        matrix_close(
            matrices.world,
            multiply_matrices(parent.matrix(), local.matrix()),
        );
        let restored = world_to_local(matrices.parent, matrices.world).unwrap();
        matrix_close(restored.matrix(), local.matrix());
        let mut desired = matrices.world;
        desired[0][3] += 2.;
        let changed = world_to_local(matrices.parent, desired).unwrap();
        matrix_close(
            multiply_matrices(matrices.parent, changed.matrix()),
            desired,
        );
    }
    #[test]
    fn decomposition_gimbal_lock_preserves_both_pitch_poles() {
        for y in [std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_2] {
            let value = Transform {
                translation: [2., 4., 6.],
                rotation: [0.8, y, -0.4],
                scale: [2., 3., 4.],
            };
            matrix_close(
                decompose_trs(value.matrix()).unwrap().matrix(),
                value.matrix(),
            );
        }
    }
    #[test]
    fn shear_singular_parent_and_hierarchy_errors_are_explicit() {
        let mut shear = identity_matrix();
        shear[0][1] = 0.5;
        assert!(
            world_to_local(identity_matrix(), shear)
                .unwrap_err()
                .contains("shear")
        );
        let singular = Transform {
            scale: [0., 1., 1.],
            ..Transform::default()
        }
        .matrix();
        assert!(
            world_to_local(singular, identity_matrix())
                .unwrap_err()
                .contains("singular")
        );
        let objects = [
            object(1, Some(2), Transform::default()),
            object(2, Some(1), Transform::default()),
        ];
        assert!(object_matrices(&objects, 1).unwrap_err().contains("cycle"));
    }
    #[test]
    fn small_import_unit_parent_is_invertible_and_non_affine_input_is_rejected() {
        let small = Transform {
            translation: [3., 0., -2.],
            rotation: [0., 0.5, 0.],
            scale: [0.01; 3],
        }
        .matrix();
        let inverse = inverse_affine(small).unwrap();
        matrix_close(multiply_matrices(inverse, small), identity_matrix());
        let mut perspective = identity_matrix();
        perspective[3][2] = 0.1;
        assert!(inverse_affine(perspective).unwrap_err().contains("affine"));
        assert!(
            world_to_local(identity_matrix(), perspective)
                .unwrap_err()
                .contains("affine")
        );
    }
    #[test]
    fn translation_drag_snaps_without_accumulation_and_cancel_restores_baseline() {
        let objects = [object(
            1,
            None,
            Transform {
                translation: [0.2, 0., 0.],
                ..Transform::default()
            },
        )];
        let rect = area();
        let start = project_point(camera(), rect, [0.2, 0., 0.]).unwrap();
        let drag = DragGesture::begin(
            &objects,
            1,
            Tool::Translate,
            Space::World,
            Axis::X,
            camera(),
            rect,
            start,
        )
        .unwrap();
        let cursor = project_point(camera(), rect, [1.42, 0., 0.]).unwrap();
        let snapped = drag
            .update(
                cursor,
                Snap {
                    enabled: true,
                    translation: 0.5,
                    ..Snap::default()
                },
            )
            .unwrap();
        close(snapped.translation[0], 1.2);
        let continuous = drag.update(cursor, Snap::default()).unwrap();
        close(continuous.translation[0], 1.42);
        assert_eq!(
            drag.update(start, Snap::default()).unwrap(),
            objects[0].transform
        );
        assert_eq!(drag.cancel(), objects[0].transform);
        assert_eq!(
            objects[0].transform.translation,
            [0.2, 0., 0.],
            "Math never mutates authored data"
        );
    }
    #[test]
    fn world_translation_under_parent_maps_to_correct_local_axes() {
        let objects = [
            object(
                1,
                None,
                Transform {
                    rotation: [0., 0., std::f32::consts::FRAC_PI_2],
                    scale: [2.; 3],
                    ..Transform::default()
                },
            ),
            object(2, Some(1), Transform::default()),
        ];
        let rect = area();
        let start = rect.center();
        let drag = DragGesture::begin(
            &objects,
            2,
            Tool::Translate,
            Space::World,
            Axis::X,
            camera(),
            rect,
            start,
        )
        .unwrap();
        let cursor = project_point(camera(), rect, [1., 0., 0.]).unwrap();
        let transform = drag.update(cursor, Snap::default()).unwrap();
        close(transform.translation[0], 0.);
        close(transform.translation[1], -0.5);
        close(transform.translation[2], 0.);
    }
    #[test]
    fn local_rotation_and_scale_keep_exact_trs_world_shear_is_rejected() {
        let objects = [
            object(
                1,
                None,
                Transform {
                    scale: [3., 1., 1.],
                    ..Transform::default()
                },
            ),
            object(
                2,
                Some(1),
                Transform {
                    rotation: [0., 0., 0.5],
                    ..Transform::default()
                },
            ),
        ];
        let rect = area();
        let local = DragGesture::begin(
            &objects,
            2,
            Tool::Rotate,
            Space::Local,
            Axis::Z,
            camera(),
            rect,
            rect.center() + Vec2::new(70., 0.),
        )
        .unwrap();
        let rotated = local.rotated(0.2).unwrap();
        close(rotated.rotation[2], 0.7);
        let scale = DragGesture::begin(
            &objects,
            2,
            Tool::Scale,
            Space::Local,
            Axis::X,
            camera(),
            rect,
            rect.center(),
        )
        .unwrap()
        .scaled(1.5)
        .unwrap();
        assert_eq!(scale.scale, [1.5, 1., 1.]);
        let world = DragGesture::begin(
            &objects,
            2,
            Tool::Rotate,
            Space::World,
            Axis::Z,
            camera(),
            rect,
            rect.center(),
        )
        .unwrap();
        assert!(world.rotated(0.2).unwrap_err().contains("shear"));
        let world_scale = DragGesture::begin(
            &objects,
            2,
            Tool::Scale,
            Space::World,
            Axis::X,
            camera(),
            rect,
            rect.center(),
        )
        .unwrap();
        assert!(world_scale.scaled(1.5).unwrap_err().contains("shear"));
    }
    #[test]
    fn rotation_and_scale_pointer_updates_use_configured_snap_and_cancel() {
        let objects = [object(1, None, Transform::default())];
        let rect = area();
        let start = project_point(camera(), rect, [1., 0., 0.]).unwrap();
        let angle = 33_f32.to_radians();
        let cursor = project_point(camera(), rect, [angle.cos(), angle.sin(), 0.]).unwrap();
        let rotate = DragGesture::begin(
            &objects,
            1,
            Tool::Rotate,
            Space::Local,
            Axis::Z,
            camera(),
            rect,
            start,
        )
        .unwrap();
        let rotated = rotate
            .update(
                cursor,
                Snap {
                    enabled: true,
                    rotation_degrees: 15.,
                    ..Snap::default()
                },
            )
            .unwrap();
        close(rotated.rotation[2], 30_f32.to_radians());
        close(
            rotate.update(cursor, Snap::default()).unwrap().rotation[2],
            angle,
        );
        assert_eq!(rotate.cancel(), Transform::default());
        let scale = DragGesture::begin(
            &objects,
            1,
            Tool::Scale,
            Space::Local,
            Axis::X,
            camera(),
            rect,
            start,
        )
        .unwrap();
        let cursor = start + scale.screen_direction * HANDLE_PIXELS * 0.28;
        let scaled = scale
            .update(
                cursor,
                Snap {
                    enabled: true,
                    ..Snap::default()
                },
            )
            .unwrap();
        close(scaled.scale[0], 1.3);
        close(scaled.scale[1], 1.);
        assert_eq!(scale.cancel(), Transform::default());
        assert!(
            scale
                .update(
                    start - scale.screen_direction * HANDLE_PIXELS * 1.2,
                    Snap::default()
                )
                .is_err()
        );
    }
    #[test]
    fn projected_gizmo_hit_test_uses_own_axis_and_rotation_ring() {
        let objects = [object(1, None, Transform::default())];
        let camera = Camera {
            yaw: 0.45,
            pitch: 0.45,
            ..camera()
        };
        let geometry =
            GizmoGeometry::build(&objects, 1, Tool::Translate, Space::World, camera, area())
                .unwrap();
        let x = geometry.axes.iter().find(|a| a.axis == Axis::X).unwrap();
        assert_eq!(geometry.hit_test(x.end), Some(Axis::X));
        assert!(geometry.hit_test(geometry.center).is_none());
        let rings =
            GizmoGeometry::build(&objects, 1, Tool::Rotate, Space::Local, camera, area()).unwrap();
        assert_eq!(rings.rings.len(), 3);
        assert!(rings.rings.iter().all(|r| r.segments.len() == 64));
        assert!(rings.hit_test(Pos2::new(1., 1.)).is_none());
    }
}
