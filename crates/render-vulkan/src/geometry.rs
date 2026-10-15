use module_engine_core::{Camera, SceneSnapshot, transform_point};
use std::{collections::BTreeMap, sync::Arc};
pub(crate) struct TextureAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub rects: BTreeMap<usize, [f32; 4]>,
    pub _textures: Vec<Arc<module_engine_core::TextureImage>>,
}
pub(crate) fn texture_keys(scene: &SceneSnapshot) -> Vec<usize> {
    let mut keys = scene
        .items
        .iter()
        .filter_map(|i| {
            i.material
                .base_color_texture
                .as_ref()
                .map(|t| Arc::as_ptr(t) as usize)
        })
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    keys
}
pub(crate) fn texture_atlas(scene: &SceneSnapshot) -> Result<TextureAtlas, String> {
    let mut textures = BTreeMap::new();
    for item in &scene.items {
        if let Some(t) = &item.material.base_color_texture {
            textures.entry(Arc::as_ptr(t) as usize).or_insert(t.clone());
        }
    }
    for t in textures.values() {
        let bytes = (t.width as usize)
            .checked_mul(t.height as usize)
            .and_then(|p| p.checked_mul(4));
        if t.width == 0 || t.height == 0 || bytes != Some(t.rgba8.len()) {
            return Err("Malformed neutral texture image".into());
        }
    }
    let width = textures.values().map(|t| t.width).max().unwrap_or(1).max(1);
    let height = textures
        .values()
        .try_fold(1u32, |sum, t| sum.checked_add(t.height))
        .ok_or("Base-color atlas height overflow")?;
    if width > 8192 || height > 8192 {
        return Err("Base-color atlas exceeds 8192 pixels; reduce imported texture sizes".into());
    }
    let mut rgba = vec![255; (width * height * 4) as usize];
    let mut rects = BTreeMap::new();
    let mut y = 1;
    for (key, t) in &textures {
        for row in 0..t.height {
            let source = (row * t.width * 4) as usize;
            let target = ((y + row) * width * 4) as usize;
            rgba[target..target + t.width as usize * 4]
                .copy_from_slice(&t.rgba8[source..source + t.width as usize * 4]);
        }
        rects.insert(*key, [0., y as f32, t.width as f32, t.height as f32]);
        y += t.height;
    }
    Ok(TextureAtlas {
        width,
        height,
        rgba,
        rects,
        _textures: textures.into_values().collect(),
    })
}
pub(crate) fn vertices(
    scene: &SceneSnapshot,
    rects: &BTreeMap<usize, [f32; 4]>,
) -> (Vec<u8>, usize) {
    let mut bytes = Vec::with_capacity(
        scene
            .items
            .iter()
            .map(|i| i.mesh.triangles.len() * 3 * 72)
            .sum(),
    );
    let mut triangle = 0;
    for (object, item) in scene.items.iter().enumerate() {
        for indices in &item.mesh.triangles {
            let positions = indices.map(|index| {
                item.mesh
                    .vertices
                    .get(index)
                    .copied()
                    .map(|p| transform_point(item.model, p))
            });
            let [Some(a), Some(b), Some(c)] = positions else {
                continue;
            };
            if !a.into_iter().chain(b).chain(c).all(f32::is_finite) {
                continue;
            }
            let normal = normalize(cross(sub(b, a), sub(c, a)));
            triangle += 1;
            for (vertex, p) in indices.iter().zip([a, b, c]) {
                let color = std::array::from_fn::<_, 4, _>(|c| {
                    item.color[c] as f32 / 255.0 * item.material.base_color[c]
                });
                let uv = item.mesh.texcoords.get(*vertex).copied().unwrap_or([0.; 2]);
                let rect = item
                    .material
                    .base_color_texture
                    .as_ref()
                    .and_then(|t| rects.get(&(Arc::as_ptr(t) as usize)))
                    .copied()
                    .unwrap_or([0., 0., 1., 1.]);
                let values = [
                    p[0],
                    p[1],
                    p[2],
                    normal[0],
                    normal[1],
                    normal[2],
                    color[0],
                    color[1],
                    color[2],
                    color[3],
                    triangle as f32,
                    (object + 1) as f32,
                    uv[0],
                    uv[1],
                    rect[0],
                    rect[1],
                    rect[2],
                    rect[3],
                ];
                for v in values {
                    bytes.extend_from_slice(&v.to_ne_bytes());
                }
            }
        }
    }
    (bytes, triangle)
}
pub(crate) fn camera_constants(camera: Camera, width: usize, height: usize) -> [f32; 32] {
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
    let eye = [
        target[0] + yaw.sin() * pitch.cos() * distance,
        target[1] + pitch.sin() * distance,
        target[2] + yaw.cos() * pitch.cos() * distance,
    ];
    let forward = normalize(sub(target, eye));
    let right = normalize(cross(forward, [0., 1., 0.]));
    let up = cross(right, forward);
    let mut result = [0.; 32];
    result[..4].copy_from_slice(&[eye[0], eye[1], eye[2], 0.08]);
    result[4..8].copy_from_slice(&[right[0], right[1], right[2], width as f32 / height as f32]);
    result[8..12].copy_from_slice(&[up[0], up[1], up[2], 2000.]);
    result[12..16].copy_from_slice(&[
        forward[0],
        forward[1],
        forward[2],
        1.0 / (55.0f32.to_radians() * 0.5).tan(),
    ]);
    result
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = (v.iter().map(|x| x * x).sum::<f32>()).sqrt();
    if length > 0.00001 {
        v.map(|x| x / length)
    } else {
        [0., 1., 0.]
    }
}
