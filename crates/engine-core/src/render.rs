use crate::{SceneSnapshot, transform_point};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Orbit camera in radians. The renderer sanitizes nonfinite values and clamps
/// pitch and distance to keep the camera basis well-defined.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: 0.55,
            pitch: 0.30,
            distance: 8.0,
            target: [0.0, 0.25, 0.0],
        }
    }
}

/// Actual geometry/pixel counts, measured by rendering rather than constants.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderStats {
    /// Number of scene items submitted to the renderer.
    pub objects: usize,
    /// Number of original triangles owning at least one final visible pixel.
    pub triangles: usize,
    /// Original triangles with valid vertices submitted before clipping.
    pub submitted_triangles: usize,
    /// Clipped triangles that passed a depth test for at least one pixel.
    pub rasterized_triangles: usize,
    /// Objects owning at least one final visible pixel.
    pub visible_objects: usize,
    /// Number of final pixels covered by geometry.
    pub shaded_pixels: usize,
}

/// RGBA8 output ready for a native GUI texture or a PNG encoder.
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub stats: RenderStats,
}

const NEAR: f32 = 0.08;
const BACKGROUND: [u8; 4] = [21, 29, 43, 255];

/// Render a world-independent snapshot using perspective, near-plane clipping,
/// flat lighting and a per-pixel depth buffer. Submission order does not decide
/// which surface is visible. Malformed mesh indices are ignored safely.
pub fn render_scene(scene: &SceneSnapshot, width: usize, height: usize, camera: Camera) -> Frame {
    let Some(pixels) = width.checked_mul(height) else {
        return empty_frame();
    };
    // A public API guard; normal viewport sizes are much smaller.
    if pixels > 16_777_216 || pixels.checked_mul(4).is_none() {
        return empty_frame();
    }
    let mut frame = Frame {
        width,
        height,
        rgba: BACKGROUND.repeat(pixels),
        stats: RenderStats {
            objects: scene.items.len(),
            ..RenderStats::default()
        },
    };
    if width == 0 || height == 0 {
        return frame;
    }
    let basis = camera_basis(camera);
    let focal = height as f32 * 0.5 / (55.0_f32.to_radians() * 0.5).tan();
    let mut depth = vec![f32::INFINITY; pixels];
    let mut triangle_owner = vec![usize::MAX; pixels];
    let mut object_owner = vec![usize::MAX; pixels];
    let light = normalize([-0.5, 1.0, 0.7]);
    let mut triangle_id = 0;
    for (object_index, item) in scene.items.iter().enumerate() {
        let vertices: Vec<_> = item
            .mesh
            .vertices
            .iter()
            .map(|v| transform_point(item.model, *v))
            .collect();
        for triangle in &item.mesh.triangles {
            let current_id = triangle_id;
            triangle_id += 1;
            let [Some(a), Some(b), Some(c)] = triangle.map(|index| vertices.get(index).copied())
            else {
                continue;
            };
            if !a.into_iter().chain(b).chain(c).all(f32::is_finite) {
                continue;
            }
            frame.stats.submitted_triangles += 1;
            let normal = normalize(cross(sub(b, a), sub(c, a)));
            // Double-sided diffuse lighting supports author-supplied winding.
            let intensity = 0.30 + 0.70 * dot(normal, light).abs();
            let color = [
                (item.color[0] as f32 * intensity).round() as u8,
                (item.color[1] as f32 * intensity).round() as u8,
                (item.color[2] as f32 * intensity).round() as u8,
                item.color[3],
            ];
            let polygon = clip_near(&[
                to_camera(a, basis),
                to_camera(b, basis),
                to_camera(c, basis),
            ]);
            for index in 1..polygon.len().saturating_sub(1) {
                let camera_triangle = [polygon[0], polygon[index], polygon[index + 1]];
                let projected = camera_triangle.map(|v| {
                    [
                        width as f32 * 0.5 + focal * v[0] / v[2],
                        height as f32 * 0.5 - focal * v[1] / v[2],
                        1.0 / v[2],
                    ]
                });
                if rasterize(
                    projected,
                    color,
                    current_id,
                    object_index,
                    &mut frame,
                    &mut depth,
                    &mut triangle_owner,
                    &mut object_owner,
                ) {
                    frame.stats.rasterized_triangles += 1;
                }
            }
        }
    }
    frame.stats.triangles = triangle_owner
        .iter()
        .copied()
        .filter(|id| *id != usize::MAX)
        .collect::<BTreeSet<_>>()
        .len();
    frame.stats.visible_objects = object_owner
        .iter()
        .copied()
        .filter(|id| *id != usize::MAX)
        .collect::<BTreeSet<_>>()
        .len();
    frame.stats.shaded_pixels = triangle_owner
        .iter()
        .filter(|id| **id != usize::MAX)
        .count();
    frame
}

fn empty_frame() -> Frame {
    Frame {
        width: 0,
        height: 0,
        rgba: vec![],
        stats: RenderStats::default(),
    }
}

#[allow(clippy::too_many_arguments)]
fn rasterize(
    v: [[f32; 3]; 3],
    color: [u8; 4],
    triangle_id: usize,
    object_index: usize,
    frame: &mut Frame,
    depth: &mut [f32],
    triangle_owner: &mut [usize],
    object_owner: &mut [usize],
) -> bool {
    let area = edge(v[0], v[1], v[2][0], v[2][1]);
    if !area.is_finite() || area.abs() < 0.0001 {
        return false;
    }
    let min_x = v
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_x = v
        .iter()
        .map(|p| p[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min((frame.width - 1) as f32)
        .max(0.0) as usize;
    let min_y = v
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let max_y = v
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min((frame.height - 1) as f32)
        .max(0.0) as usize;
    if min_x > max_x || min_y > max_y || min_x >= frame.width || min_y >= frame.height {
        return false;
    }
    let mut touched = false;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let wa = edge(v[1], v[2], px, py) / area;
            let wb = edge(v[2], v[0], px, py) / area;
            let wc = edge(v[0], v[1], px, py) / area;
            if wa < -0.00001 || wb < -0.00001 || wc < -0.00001 {
                continue;
            }
            // Reciprocal camera-space depth is affine in projected coordinates.
            let inverse_depth = wa * v[0][2] + wb * v[1][2] + wc * v[2][2];
            if inverse_depth <= 0.0 || !inverse_depth.is_finite() {
                continue;
            }
            let z = 1.0 / inverse_depth;
            let pixel = y * frame.width + x;
            if z < depth[pixel] {
                depth[pixel] = z;
                frame.rgba[pixel * 4..pixel * 4 + 4].copy_from_slice(&color);
                triangle_owner[pixel] = triangle_id;
                object_owner[pixel] = object_index;
                touched = true;
            }
        }
    }
    touched
}

fn edge(a: [f32; 3], b: [f32; 3], x: f32, y: f32) -> f32 {
    (x - a[0]) * (b[1] - a[1]) - (y - a[1]) * (b[0] - a[0])
}

fn clip_near(vertices: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let mut output = vec![];
    let mut previous = vertices[vertices.len() - 1];
    let mut previous_inside = previous[2] >= NEAR;
    for &current in vertices {
        let current_inside = current[2] >= NEAR;
        if current_inside != previous_inside {
            let t = (NEAR - previous[2]) / (current[2] - previous[2]);
            output.push([
                previous[0] + (current[0] - previous[0]) * t,
                previous[1] + (current[1] - previous[1]) * t,
                NEAR,
            ]);
        }
        if current_inside {
            output.push(current);
        }
        previous = current;
        previous_inside = current_inside;
    }
    output
}

#[derive(Clone, Copy)]
struct CameraBasis {
    eye: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    forward: [f32; 3],
}

fn camera_basis(camera: Camera) -> CameraBasis {
    let defaults = Camera::default();
    let yaw = if camera.yaw.is_finite() {
        camera.yaw
    } else {
        defaults.yaw
    };
    let pitch = if camera.pitch.is_finite() {
        camera.pitch.clamp(-1.50, 1.50)
    } else {
        defaults.pitch
    };
    let distance = if camera.distance.is_finite() {
        camera.distance.clamp(0.15, 200.0)
    } else {
        defaults.distance
    };
    let target = if camera.target.iter().all(|v| v.is_finite()) {
        camera.target
    } else {
        defaults.target
    };
    let offset = [
        yaw.sin() * pitch.cos() * distance,
        pitch.sin() * distance,
        yaw.cos() * pitch.cos() * distance,
    ];
    let eye = std::array::from_fn(|i| target[i] + offset[i]);
    let forward = normalize(sub(target, eye));
    let right = normalize(cross(forward, [0., 1., 0.]));
    let up = cross(right, forward);
    CameraBasis {
        eye,
        right,
        up,
        forward,
    }
}

fn to_camera(position: [f32; 3], basis: CameraBasis) -> [f32; 3] {
    let local = sub(position, basis.eye);
    [
        dot(local, basis.right),
        dot(local, basis.up),
        dot(local, basis.forward),
    ]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = dot(value, value).sqrt();
    if length <= f32::EPSILON {
        [0.; 3]
    } else {
        value.map(|v| v / length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mesh, ObjectsWorld, RenderItem, Transform, WorldProvider, identity_matrix};

    fn triangle(id: u64, z: f32, color: [u8; 4]) -> RenderItem {
        RenderItem {
            id,
            mesh: Mesh {
                texcoords: vec![],
                vertices: vec![[-1., -1., z], [1., -1., z], [0., 1., z]],
                triangles: vec![[0, 1, 2]],
            },
            model: identity_matrix(),
            material: Default::default(),
            color,
        }
    }

    #[test]
    fn real_depth_wins_over_submission_order() {
        let camera = Camera {
            yaw: 0.,
            pitch: 0.,
            distance: 4.,
            target: [0.; 3],
        };
        let near = triangle(1, 1., [230, 20, 20, 255]);
        let far = triangle(2, 0., [20, 20, 230, 255]);
        let frame = render_scene(
            &SceneSnapshot {
                items: vec![near.clone(), far.clone()],
                time: 0.,
            },
            128,
            128,
            camera,
        );
        let reverse = render_scene(
            &SceneSnapshot {
                items: vec![far, near],
                time: 0.,
            },
            128,
            128,
            camera,
        );
        assert_eq!(frame.rgba, reverse.rgba);
        let center = (64 * 128 + 64) * 4;
        assert!(frame.rgba[center] > frame.rgba[center + 2]);
        assert_eq!(frame.stats.visible_objects, 1);
        assert_eq!(frame.stats.triangles, 1);
        assert_eq!(frame.stats.submitted_triangles, 2);
    }

    #[test]
    fn orbit_rotation_and_world_motion_change_real_geometry() {
        let mut world = ObjectsWorld::demo();
        let initial = render_scene(&world.snapshot(), 200, 150, Camera::default());
        assert!(initial.stats.shaded_pixels > 1000);
        assert!(initial.stats.visible_objects >= 3);
        assert!(initial.stats.triangles > 0);
        assert_eq!(initial.stats.submitted_triangles, 48);
        let orbit = render_scene(
            &world.snapshot(),
            200,
            150,
            Camera {
                yaw: 1.2,
                ..Camera::default()
            },
        );
        assert_ne!(initial.rgba, orbit.rgba);
        world.step(0.5, 1.0);
        assert_ne!(
            initial.rgba,
            render_scene(&world.snapshot(), 200, 150, Camera::default()).rgba
        );
    }

    #[test]
    fn triangles_crossing_near_plane_are_clipped_and_behind_camera_are_absent() {
        let camera = Camera {
            yaw: 0.,
            pitch: 0.,
            distance: 1.,
            target: [0.; 3],
        };
        let item = RenderItem {
            material: Default::default(),
            id: 1,
            mesh: Mesh {
                texcoords: vec![],
                vertices: vec![[-0.2, -0.2, 0.4], [0.2, -0.2, 0.4], [0.0, 0.2, 1.2]],
                triangles: vec![[0, 1, 2]],
            },
            model: identity_matrix(),
            color: [255; 4],
        };
        let clipped = render_scene(
            &SceneSnapshot {
                items: vec![item],
                time: 0.,
            },
            100,
            100,
            camera,
        );
        assert!(clipped.stats.shaded_pixels > 0);
        let mut behind = triangle(2, 2.0, [255; 4]);
        behind.model = Transform::default().matrix();
        let frame = render_scene(
            &SceneSnapshot {
                items: vec![behind],
                time: 0.,
            },
            100,
            100,
            camera,
        );
        assert_eq!(frame.stats.shaded_pixels, 0);
    }
}
