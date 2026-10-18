//! Renderer-neutral interpolation; gameplay storage and clocks are never modified.
use crate::{SceneSnapshot, multiply_matrices};
use std::collections::{BTreeMap, BTreeSet};

/// Emit this null-payload event in a committed tick that teleports objects.
/// Hosts snap presentation history; ordinary movement is never classified by distance.
pub const PRESENTATION_RESET_EVENT: &str = "PresentationReset";

/// Process-local identity and hierarchy used only to match adjacent presentation poses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationIdentity {
    pub lifetime: u64,
    pub parent: Option<u64>,
}

/// Blend safe affine SRT models before imported primitives are attached. Current
/// geometry, materials, colors and item ordering remain authoritative. Duplicate
/// IDs, changed lifetimes/ancestors, singular, reflected-axis changes and sheared
/// matrices snap to current. Unknown providers should not call this without a
/// proven identity map. Alpha is clamped; nonfinite alpha snaps to current.
pub fn interpolate_scene_models(
    previous: &SceneSnapshot,
    current: &SceneSnapshot,
    previous_ids: &BTreeMap<u64, PresentationIdentity>,
    current_ids: &BTreeMap<u64, PresentationIdentity>,
    alpha: f64,
) -> SceneSnapshot {
    let alpha = if alpha.is_finite() {
        alpha.clamp(0., 1.) as f32
    } else {
        1.
    };
    let mut result = current.clone();
    let mut old = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    for item in &previous.items {
        if old.insert(item.id, item).is_some() {
            duplicates.insert(item.id);
        }
    }
    let mut seen = BTreeSet::new();
    for item in &current.items {
        if !seen.insert(item.id) {
            duplicates.insert(item.id);
        }
    }
    for item in &mut result.items {
        let mut id = Some(item.id);
        let mut compatible = true;
        let mut remaining = current_ids.len() + 1;
        while let Some(entity) = id {
            remaining = remaining.saturating_sub(1);
            match (previous_ids.get(&entity), current_ids.get(&entity)) {
                (Some(a), Some(b)) if a == b && remaining > 0 => id = b.parent,
                _ => {
                    compatible = false;
                    break;
                }
            }
        }
        if compatible
            && !duplicates.contains(&item.id)
            && let Some(old) = old.get(&item.id)
            && let Some(model) = interpolate_model(old.model, item.model, alpha)
        {
            item.model = model;
        }
    }
    result
}

type Matrix = [[f32; 4]; 4];
type Quaternion = [f32; 4];
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn decompose(m: Matrix) -> Option<([f32; 3], Quaternion, [f32; 3])> {
    if m.iter().flatten().any(|v| !v.is_finite())
        || (0..4).any(|i| (m[3][i] - if i == 3 { 1. } else { 0. }).abs() > 1e-5)
    {
        return None;
    }
    let mut cols: [[f32; 3]; 3] = std::array::from_fn(|c| std::array::from_fn(|r| m[r][c]));
    let mut scale = cols.map(|v| dot(v, v).sqrt());
    if scale.iter().any(|s| *s < 1e-7) {
        return None;
    }
    for (column, length) in cols.iter_mut().zip(scale) {
        for value in column {
            *value /= length;
        }
    }
    if dot(cols[0], cols[1]).abs() > 1e-4
        || dot(cols[0], cols[2]).abs() > 1e-4
        || dot(cols[1], cols[2]).abs() > 1e-4
    {
        return None;
    }
    let cross = [
        cols[0][1] * cols[1][2] - cols[0][2] * cols[1][1],
        cols[0][2] * cols[1][0] - cols[0][0] * cols[1][2],
        cols[0][0] * cols[1][1] - cols[0][1] * cols[1][0],
    ];
    if dot(cross, cols[2]) < 0. {
        scale[0] = -scale[0];
        cols[0] = cols[0].map(|v| -v);
    }
    let r: [[f32; 3]; 3] = std::array::from_fn(|row| std::array::from_fn(|col| cols[col][row]));
    let trace = r[0][0] + r[1][1] + r[2][2];
    let q = if trace > 0. {
        let s = (trace + 1.).sqrt() * 2.;
        [
            (r[2][1] - r[1][2]) / s,
            (r[0][2] - r[2][0]) / s,
            (r[1][0] - r[0][1]) / s,
            s / 4.,
        ]
    } else {
        let i = if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
            0
        } else if r[1][1] > r[2][2] {
            1
        } else {
            2
        };
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        let s = (1. + r[i][i] - r[j][j] - r[k][k]).max(0.).sqrt() * 2.;
        if s < 1e-7 {
            return None;
        }
        let mut q = [0.; 4];
        q[i] = s / 4.;
        q[j] = (r[j][i] + r[i][j]) / s;
        q[k] = (r[k][i] + r[i][k]) / s;
        q[3] = (r[k][j] - r[j][k]) / s;
        q
    };
    Some(([m[0][3], m[1][3], m[2][3]], q, scale))
}
fn interpolate_model(a: Matrix, b: Matrix, alpha: f32) -> Option<Matrix> {
    if a == b {
        return Some(b);
    }
    let (at, aq, as_) = decompose(a)?;
    let (bt, mut bq, bs) = decompose(b)?;
    if as_[0].is_sign_negative() != bs[0].is_sign_negative() {
        return None;
    }
    if alpha == 0. {
        return Some(a);
    }
    if alpha == 1. {
        return Some(b);
    }
    let mut cosine: f32 = aq.iter().zip(bq).map(|(a, b)| a * b).sum();
    if cosine < 0. {
        bq = bq.map(|v| -v);
        cosine = -cosine;
    }
    let q = if cosine > 0.9995 {
        let q: [f32; 4] = std::array::from_fn(|i| aq[i] + alpha * (bq[i] - aq[i]));
        let length = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        q.map(|v| v / length)
    } else {
        let theta = cosine.clamp(-1., 1.).acos();
        let sine = theta.sin();
        let x = ((1. - alpha) * theta).sin() / sine;
        let y = (alpha * theta).sin() / sine;
        std::array::from_fn(|i| x * aq[i] + y * bq[i])
    };
    let [x, y, z, w] = q;
    let rotation = [
        [
            1. - 2. * (y * y + z * z),
            2. * (x * y - z * w),
            2. * (x * z + y * w),
            0.,
        ],
        [
            2. * (x * y + z * w),
            1. - 2. * (x * x + z * z),
            2. * (y * z - x * w),
            0.,
        ],
        [
            2. * (x * z - y * w),
            2. * (y * z + x * w),
            1. - 2. * (x * x + y * y),
            0.,
        ],
        [0., 0., 0., 1.],
    ];
    let scale: [f32; 3] = std::array::from_fn(|i| as_[i] + alpha * (bs[i] - as_[i]));
    let mut result = multiply_matrices(
        rotation,
        [
            [scale[0], 0., 0., 0.],
            [0., scale[1], 0., 0.],
            [0., 0., scale[2], 0.],
            [0., 0., 0., 1.],
        ],
    );
    for (i, row) in result.iter_mut().take(3).enumerate() {
        row[3] = at[i] + alpha * (bt[i] - at[i]);
    }
    result
        .iter()
        .flatten()
        .all(|v| v.is_finite())
        .then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Material, Mesh, RenderItem, Transform, transform_point};
    fn snapshot(transform: Transform) -> SceneSnapshot {
        SceneSnapshot {
            items: vec![RenderItem {
                id: 1,
                mesh: Mesh::cube(),
                model: transform.matrix(),
                color: [255; 4],
                material: Material::default(),
            }],
            time: 0.,
        }
    }
    fn ids() -> BTreeMap<u64, PresentationIdentity> {
        BTreeMap::from([(
            1,
            PresentationIdentity {
                lifetime: 7,
                parent: None,
            },
        )])
    }
    #[test]
    fn srt_endpoints_midpoint_and_shortest_quaternion_rotation_preserve_scale() {
        let a = snapshot(Transform {
            rotation: [0., 0., 170f32.to_radians()],
            scale: [-2., 3., 4.],
            ..Default::default()
        });
        let b = snapshot(Transform {
            translation: [4., 6., 8.],
            rotation: [0., 0., -170f32.to_radians()],
            scale: [-4., 5., 6.],
        });
        assert_eq!(
            interpolate_scene_models(&a, &b, &ids(), &ids(), 0.).items[0].model,
            a.items[0].model
        );
        assert_eq!(
            interpolate_scene_models(&a, &b, &ids(), &ids(), 1.).items[0].model,
            b.items[0].model
        );
        let middle = interpolate_scene_models(&a, &b, &ids(), &ids(), 0.5);
        let m = middle.items[0].model;
        for (row, value) in [2., 3., 4.].into_iter().enumerate() {
            assert!((m[row][3] - value).abs() < 1e-5);
        }
        let p = transform_point(m, [1., 0., 0.]);
        assert!(
            (p[0] - 5.).abs() < 1e-4,
            "Negative scale and shortest rotation pass through180deg"
        );
        assert!((p[1] - 3.).abs() < 1e-4);
    }
    #[test]
    fn changed_lifetimes_ancestors_duplicates_and_invalid_srt_snap_current() {
        let a = snapshot(Transform::default());
        let b = snapshot(Transform {
            translation: [8., 0., 0.],
            ..Default::default()
        });
        let mut changed = ids();
        changed.get_mut(&1).unwrap().lifetime = 8;
        assert_eq!(interpolate_scene_models(&a, &b, &ids(), &changed, 0.5), b);
        changed = ids();
        changed.get_mut(&1).unwrap().parent = Some(2);
        assert_eq!(interpolate_scene_models(&a, &b, &ids(), &changed, 0.5), b);
        assert_eq!(
            interpolate_scene_models(&a, &b, &BTreeMap::new(), &BTreeMap::new(), 0.5),
            b
        );
        let mut duplicate = b.clone();
        duplicate.items.push(b.items[0].clone());
        assert_eq!(
            interpolate_scene_models(&a, &duplicate, &ids(), &ids(), 0.5),
            duplicate
        );
        for model in [
            [
                [1., 0.5, 0., 8.],
                [0., 1., 0., 0.],
                [0., 0., 1., 0.],
                [0., 0., 0., 1.],
            ],
            Transform {
                scale: [0., 1., 1.],
                ..Default::default()
            }
            .matrix(),
            Transform {
                scale: [-1., 1., 1.],
                ..Default::default()
            }
            .matrix(),
        ] {
            let mut invalid = b.clone();
            invalid.items[0].model = model;
            assert_eq!(
                interpolate_scene_models(&a, &invalid, &ids(), &ids(), 0.5),
                invalid
            );
        }
        assert_eq!(
            interpolate_scene_models(&a, &b, &ids(), &ids(), f64::NAN),
            b
        );
        let mut invalid = b.clone();
        invalid.items[0].model[0][0] = f32::INFINITY;
        assert_eq!(
            interpolate_scene_models(&a, &invalid, &ids(), &ids(), 0.5),
            invalid
        );
    }
}
