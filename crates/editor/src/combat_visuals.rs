//! Shared world-projected combat feedback for editor Play and standalone Player.
//! Damage and candidate targets come from core's melee rules, not painted shapes.
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use module_engine_core::{Camera, GameState, GameStatus, MeleeArea};

/// Feedback affects only the extracted render snapshot. Actor/material data and
/// authored colors remain unchanged; the next snapshot naturally restores them.
pub fn apply_hit_flashes(snapshot: &mut module_engine_core::SceneSnapshot, state: &GameState) {
    if state.status != GameStatus::Playing || state.hit_visuals.is_empty() {
        return;
    }
    for item in &mut snapshot.items {
        let intensity = state
            .hit_visuals
            .iter()
            .filter(|hit| {
                hit.target == item.id
                    && hit
                        .target_lifetime
                        .is_none_or(|lifetime| state.world.lifetime(item.id) == Some(lifetime))
            })
            .map(|hit| {
                let age = state.time - hit.started_at;
                if age < 0. || state.time >= hit.expires_at {
                    0.
                } else {
                    (1. - age / 0.16).clamp(0., 1.) as f32 * 0.95
                }
            })
            .fold(0_f32, f32::max);
        if intensity <= 0. {
            continue;
        }
        for (channel, white) in [255., 248., 205.].into_iter().enumerate() {
            item.color[channel] =
                (item.color[channel] as f32 * (1. - intensity) + white * intensity).round() as u8;
            item.material.base_color[channel] +=
                (1. - item.material.base_color[channel]) * intensity;
        }
    }
}

pub fn draw(ui: &egui::Ui, rect: Rect, camera: Camera, state: &GameState, aim: Option<[f32; 3]>) {
    if state.status != GameStatus::Playing {
        return;
    }
    let painter = ui.painter_at(rect);
    let player = state.player();
    if let Some(player) = player
        && !state.attack_visuals.iter().any(|a| {
            a.attacker == player.object.id
                && current_incarnation(state, a.attacker, a.attacker_lifetime)
        })
        && !state.telegraphs.iter().any(|a| {
            a.attacker == player.object.id
                && current_incarnation(state, a.attacker, a.attacker_lifetime)
        })
        && let Some(aim) = aim
        && let Some(area) = state.melee_aim(player.object.id, aim)
    {
        paint_area(
            &painter,
            camera,
            rect,
            &area,
            Color32::from_rgba_unmultiplied(75, 200, 255, 24),
            Stroke::new(1.5, Color32::from_rgba_unmultiplied(125, 225, 255, 180)),
        );
        direction_arrow(&painter, camera, rect, &area);
        paint_targets(&painter, camera, rect, state, player.object.id, &area);
    }
    for attack in &state.attack_visuals {
        let friendly = attack
            .attacker_player
            .unwrap_or_else(|| state.world.component(attack.attacker, "player").is_some());
        let color = if friendly {
            [255, 206, 92]
        } else {
            [255, 90, 70]
        };
        let progress = attack.progress.clamp(0., 1.);
        let alpha = (105. * (1. - progress * 0.65)) as u8;
        paint_area(
            &painter,
            camera,
            rect,
            &attack.area,
            Color32::from_rgba_unmultiplied(color[0], color[1], color[2], alpha),
            Stroke::new(2.5, Color32::from_rgb(color[0], color[1], color[2])),
        );
        if friendly && current_incarnation(state, attack.attacker, attack.attacker_lifetime) {
            paint_targets(&painter, camera, rect, state, attack.attacker, &attack.area);
        }
        slash(&painter, camera, rect, &attack.area, progress, friendly);
    }
    for hit in &state.hit_visuals {
        let lifetime = (hit.expires_at - hit.started_at).max(0.001);
        let age = ((state.time - hit.started_at) / lifetime).clamp(0., 1.) as f32;
        if age >= 1. {
            continue;
        }
        let received = hit
            .target_player
            .unwrap_or_else(|| state.world.component(hit.target, "player").is_some());
        let rgb = if received {
            [255, 100, 90]
        } else {
            [255, 235, 145]
        };
        let color =
            Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], (255. * (1. - age)) as u8);
        // Captured before death/removal, so a killing blow remains visible.
        let position = [hit.position[0], hit.position[1] + 0.55, hit.position[2]];
        if let Some(center) = crate::player::screen_point(camera, position, rect) {
            let spread = 5. + age * 18.;
            for ray in 0..7 {
                let angle = ray as f32 * std::f32::consts::TAU / 7. + (hit.target % 7) as f32 * 0.3;
                let axis = Vec2::angled(angle);
                painter.line_segment(
                    [center + axis * spread * 0.5, center + axis * spread],
                    Stroke::new(2., color),
                );
            }
        }
        let label = [
            hit.position[0],
            hit.position[1] + 1.0 + age * 0.8,
            hit.position[2],
        ];
        if let Some(p) = crate::player::screen_point(camera, label, rect) {
            let text = format!("−{:.0}", hit.damage);
            painter.text(
                p + Vec2::new(1., 1.),
                egui::Align2::CENTER_CENTER,
                &text,
                egui::FontId::proportional(19.),
                Color32::from_black_alpha((230. * (1. - age)) as u8),
            );
            painter.text(
                p,
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(19.),
                color,
            );
        }
    }
}

fn current_incarnation(state: &GameState, id: u64, lifetime: Option<u64>) -> bool {
    lifetime.is_none_or(|lifetime| state.world.lifetime(id) == Some(lifetime))
}

fn paint_targets(
    painter: &egui::Painter,
    camera: Camera,
    rect: Rect,
    state: &GameState,
    attacker: u64,
    area: &MeleeArea,
) {
    for id in state.melee_targets(attacker, area) {
        if let Some(target) = state.world.entity(id) {
            let radius = target
                .components
                .get("actor")
                .and_then(|a| a.get("radius"))
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.35) as f32;
            target_ring(
                painter,
                camera,
                rect,
                target.object.transform.translation,
                radius.max(0.2),
                Stroke::new(2.5, Color32::from_rgb(255, 195, 85)),
            );
        }
    }
}

fn paint_area(
    painter: &egui::Painter,
    camera: Camera,
    rect: Rect,
    area: &MeleeArea,
    fill: Color32,
    stroke: Stroke,
) {
    if let Some(shape) = sector_shape(camera, rect, area, fill, stroke) {
        painter.add(shape);
    }
}

/// Perspective-projected footprint. Never use convex tessellation for a wide fan.
pub(super) fn sector_shape(
    camera: Camera,
    rect: Rect,
    area: &MeleeArea,
    fill: Color32,
    stroke: Stroke,
) -> Option<egui::Shape> {
    if !area.range.is_finite()
        || area.range <= 0.
        || !area.arc.is_finite()
        || area.arc <= 0.
        || !area
            .center
            .iter()
            .chain(&area.direction)
            .all(|v| v.is_finite())
    {
        return None;
    }
    let arc = area.arc.min(std::f32::consts::TAU);
    let angle = area.direction[1].atan2(area.direction[0]);
    let mut points = (0..=28)
        .map(|n| {
            let a = angle - arc * 0.5 + arc * n as f32 / 28.;
            crate::player::screen_point(
                camera,
                [
                    area.center[0] + a.cos() * area.range,
                    0.025,
                    area.center[2] + a.sin() * area.range,
                ],
                rect,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    if arc >= std::f32::consts::TAU - 0.001 {
        points.pop();
        return Some(egui::Shape::convex_polygon(points, fill, stroke));
    }
    let center =
        crate::player::screen_point(camera, [area.center[0], 0.025, area.center[2]], rect)?;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, fill);
    for p in &points {
        mesh.colored_vertex(*p, fill);
    }
    for n in 1..points.len() {
        mesh.add_triangle(0, n as u32, n as u32 + 1);
    }
    let mut boundary = vec![center];
    boundary.extend(points);
    Some(egui::Shape::Vec(vec![
        egui::Shape::mesh(mesh),
        egui::Shape::closed_line(boundary, stroke),
    ]))
}

fn direction_arrow(painter: &egui::Painter, camera: Camera, rect: Rect, area: &MeleeArea) {
    let offset = |distance: f32| {
        [
            area.center[0] + area.direction[0] * distance,
            0.04,
            area.center[2] + area.direction[1] * distance,
        ]
    };
    if let (Some(start), Some(end)) = (
        crate::player::screen_point(camera, offset(0.45), rect),
        crate::player::screen_point(camera, offset(area.range * 0.82), rect),
    ) {
        painter.arrow(
            start,
            end - start,
            Stroke::new(2., Color32::from_rgb(155, 235, 255)),
        );
    }
}

fn slash(
    painter: &egui::Painter,
    camera: Camera,
    rect: Rect,
    area: &MeleeArea,
    progress: f32,
    friendly: bool,
) {
    let arc = area.arc.min(std::f32::consts::TAU);
    let middle = area.direction[1].atan2(area.direction[0]);
    let head = middle - arc * 0.5 + arc * progress;
    let tail = (head - arc * 0.32).max(middle - arc * 0.5);
    let point = |angle: f32, radius: f32| {
        crate::player::screen_point(
            camera,
            [
                area.center[0] + angle.cos() * radius,
                area.center[1] + 0.4,
                area.center[2] + angle.sin() * radius,
            ],
            rect,
        )
    };
    let path = (0..=18)
        .filter_map(|i| point(tail + (head - tail) * i as f32 / 18., area.range * 0.82))
        .collect::<Vec<_>>();
    if path.len() > 1 {
        painter.add(egui::Shape::line(
            path.clone(),
            Stroke::new(
                10.,
                if friendly {
                    Color32::from_rgba_unmultiplied(255, 195, 60, 65)
                } else {
                    Color32::from_rgba_unmultiplied(255, 70, 50, 65)
                },
            ),
        ));
        painter.add(egui::Shape::line(
            path,
            Stroke::new(3., Color32::from_rgb(255, 246, 205)),
        ));
    }
    if let (Some(inner), Some(tip)) = (
        point(head, area.range * 0.35),
        point(head, area.range * 0.9),
    ) {
        painter.line_segment([inner, tip], Stroke::new(3.5, Color32::WHITE));
        painter.circle_filled(tip, 3.5, Color32::WHITE);
    }
}

fn target_ring(
    painter: &egui::Painter,
    camera: Camera,
    rect: Rect,
    center: [f32; 3],
    radius: f32,
    stroke: Stroke,
) {
    let points = (0..24)
        .filter_map(|n| {
            let a = n as f32 * std::f32::consts::TAU / 24.;
            crate::player::screen_point(
                camera,
                [
                    center[0] + a.cos() * radius,
                    0.04,
                    center[2] + a.sin() * radius,
                ],
                rect,
            )
        })
        .collect::<Vec<Pos2>>();
    if points.len() == 24 {
        painter.add(egui::Shape::closed_line(points, stroke));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{GameInput, GameProject, GameSession, WorldKind};
    use serde_json::json;

    fn camera() -> Camera {
        Camera {
            yaw: 0.,
            pitch: 0.95,
            distance: 21.,
            target: [0., 0., 4.],
        }
    }
    fn viewport() -> Rect {
        Rect::from_min_size(Pos2::new(40., 70.), Vec2::new(960., 540.))
    }
    fn encounter() -> GameSession {
        let mut project = GameProject::demo();
        let scene = project.scenes.get_mut("arena").unwrap();
        scene
            .objects
            .retain(|e| [1, 10, 11, 12, 13].contains(&e.object.id));
        for entity in &mut scene.objects {
            if entity.object.id == 1 {
                continue;
            }
            entity.object.transform.translation = match entity.object.id {
                // Outside nominal reach, but the target circle overlaps it.
                10 => [0., 0., 2.5],
                11 => [0.2, 0., 3.5],
                // Within reach, but outside the sector's center-angle test.
                12 => [2.1, 0., 4.5],
                _ => [0., 0., 6.5],
            };
            entity
                .components
                .insert("ai".into(), json!({"kind":"chase","aggro_radius":0.0}));
            entity
                .components
                .insert("material".into(), json!({"base_color":[0.2,0.3,0.4,1.0]}));
            if entity.object.id == 10 {
                entity
                    .components
                    .insert("health".into(), json!({"maximum":36.0,"current":1.0}));
            }
        }
        GameSession::new(project, WorldKind::Objects).unwrap()
    }
    fn attacked_encounter() -> GameSession {
        let mut session = encounter();
        session
            .step(
                1. / 60.,
                &GameInput {
                    attack: true,
                    aim_world: [0., 0., 3.],
                    ..GameInput::default()
                },
            )
            .unwrap();
        session
    }
    fn render_shapes(state: &GameState, aim: Option<[f32; 3]>) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1080., 720.))),
                ..Default::default()
            },
            |ui| draw(ui, viewport(), camera(), state, aim),
        );
        output.textures_delta.clear();
        output.shapes
    }
    fn visit_shapes<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::Shape>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    visit_shapes(shape, out);
                }
            }
            _ => out.push(shape),
        }
    }
    fn leaves(shapes: &[egui::epaint::ClippedShape]) -> Vec<&egui::Shape> {
        let mut out = vec![];
        for shape in shapes {
            visit_shapes(&shape.shape, &mut out);
        }
        out
    }
    fn target_rings(shapes: &[egui::epaint::ClippedShape]) -> Vec<&egui::epaint::PathShape> {
        leaves(shapes)
            .into_iter()
            .filter_map(|shape| match shape {
                egui::Shape::Path(path)
                    if path.closed
                        && path.stroke.color
                            == egui::epaint::ColorMode::Solid(Color32::from_rgb(255, 195, 85))
                        && path.stroke.width == 2.5 =>
                {
                    Some(path)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn full_circle_omits_center_and_wide_projected_fan_keeps_its_gap() {
        let area = MeleeArea {
            center: [0., 0., 4.],
            direction: [1., 0.],
            range: 3.,
            arc: std::f32::consts::TAU,
        };
        let center = crate::player::screen_point(camera(), [0., 0.025, 4.], viewport()).unwrap();
        let fill = Color32::from_rgba_unmultiplied(250, 180, 80, 90);
        let stroke = Stroke::new(2., Color32::WHITE);
        let circle = sector_shape(camera(), viewport(), &area, fill, stroke).unwrap();
        let egui::Shape::Path(path) = circle else {
            panic!("Circle requires a convex perimeter");
        };
        assert!(path.closed);
        assert_eq!(path.points.len(), 28);
        assert!(
            path.points
                .iter()
                .all(|p| p.is_finite() && p.distance(center) > 20.)
        );

        let wide = MeleeArea { arc: 4.5, ..area };
        let fan = sector_shape(camera(), viewport(), &wide, fill, stroke).unwrap();
        let egui::Shape::Vec(shapes) = fan else {
            panic!("A wide sector needs an explicit fan");
        };
        let egui::Shape::Mesh(mesh) = &shapes[0] else {
            panic!("A wide sector cannot use convex fill");
        };
        assert_eq!(mesh.vertices[0].pos, center);
        assert_eq!(mesh.indices.len(), 28 * 3);
        assert!(
            mesh.indices
                .as_chunks::<3>()
                .0
                .iter()
                .all(|tri| tri[0] == 0)
        );
        let inside = crate::player::screen_point(camera(), [2., 0.025, 4.], viewport()).unwrap();
        let gap = crate::player::screen_point(camera(), [-2., 0.025, 4.], viewport()).unwrap();
        let in_triangle = |p: Pos2, a: Pos2, b: Pos2, c: Pos2| {
            let side = |a: Pos2, b: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
            let signs = [side(a, b), side(b, c), side(c, a)];
            signs.iter().all(|v| *v >= -0.001) || signs.iter().all(|v| *v <= 0.001)
        };
        let covered = |point| {
            mesh.indices.as_chunks::<3>().0.iter().any(|tri| {
                in_triangle(
                    point,
                    mesh.vertices[tri[0] as usize].pos,
                    mesh.vertices[tri[1] as usize].pos,
                    mesh.vertices[tri[2] as usize].pos,
                )
            })
        };
        assert!(covered(inside));
        assert!(
            !covered(gap),
            "Wide fill must not bridge the excluded angular wedge"
        );
        assert!(
            sector_shape(
                camera(),
                viewport(),
                &MeleeArea { range: 0., ..area },
                fill,
                stroke
            )
            .is_none()
        );
    }

    #[test]
    fn preview_rings_follow_core_targets_and_mouse_direction() {
        let session = encounter();
        let state = session.state();
        let forward = [0., 0., 3.];
        let forward_area = state.melee_aim(1, forward).unwrap();
        assert_eq!(state.melee_targets(1, &forward_area), vec![10, 11]);
        let projected = render_shapes(state, Some(forward));
        let rings = target_rings(&projected);
        assert_eq!(rings.len(), 2);
        // Each highlight belongs to an actual eligible target, including the
        // radial-overlap target that lies beyond the painted nominal range.
        for (ring, id) in rings.iter().zip(state.melee_targets(1, &forward_area)) {
            let entity = state.world.entity(id).unwrap();
            let radius = entity.components["actor"]["radius"].as_f64().unwrap() as f32;
            let expected = crate::player::screen_point(
                camera(),
                [
                    entity.object.transform.translation[0] + radius,
                    0.04,
                    entity.object.transform.translation[2],
                ],
                viewport(),
            )
            .unwrap();
            assert_eq!(ring.points[0], expected);
        }
        let backward = [0., 0., 8.];
        let backward_area = state.melee_aim(1, backward).unwrap();
        assert_eq!(backward_area.direction, [0., 1.]);
        assert_eq!(state.melee_targets(1, &backward_area), vec![13]);
        assert_eq!(target_rings(&render_shapes(state, Some(backward))).len(), 1);
        assert!(target_rings(&render_shapes(state, None)).is_empty());
    }

    #[test]
    fn real_hit_flash_changes_only_extracted_render_data_and_expires() {
        let session = attacked_encounter();
        let before_state = session.state().clone();
        let authored = session.project().clone();
        let fresh = session.snapshot();
        let mut snapshot = fresh.clone();
        // Simulate material enrichment on the extracted item, as GameHost does.
        let surviving = snapshot.items.iter_mut().find(|i| i.id == 11).unwrap();
        surviving.material.base_color = [0.2, 0.3, 0.4, 0.75];
        let before_snapshot = snapshot.clone();
        apply_hit_flashes(&mut snapshot, session.state());
        let changed = snapshot.items.iter().find(|i| i.id == 11).unwrap();
        let original = before_snapshot.items.iter().find(|i| i.id == 11).unwrap();
        assert_ne!(changed.color, original.color);
        assert!(changed.material.base_color[0] > original.material.base_color[0]);
        assert_eq!(
            changed.material.base_color[3],
            original.material.base_color[3]
        );
        assert_eq!(changed.mesh, original.mesh);
        assert_eq!(changed.model, original.model);
        assert_eq!(
            snapshot.items.iter().find(|i| i.id == 12),
            before_snapshot.items.iter().find(|i| i.id == 12)
        );
        assert_eq!(session.state(), &before_state);
        assert_eq!(session.project(), &authored);
        assert_eq!(session.snapshot(), fresh);

        let mut replacement = before_state.clone();
        let object = replacement.world.object(11).unwrap();
        replacement
            .apply_commands(&[
                module_engine_core::WorldCommand::Remove { id: 11 },
                module_engine_core::WorldCommand::Spawn { object },
            ])
            .unwrap();
        assert_ne!(
            replacement.world.lifetime(11),
            before_state.world.lifetime(11)
        );
        let mut replacement_snapshot = before_snapshot.clone();
        apply_hit_flashes(&mut replacement_snapshot, &replacement);
        assert_eq!(replacement_snapshot, before_snapshot);

        let mut expired = before_state.clone();
        expired.time = expired
            .hit_visuals
            .iter()
            .map(|h| h.expires_at)
            .fold(0_f64, f64::max);
        let mut expired_snapshot = before_snapshot.clone();
        apply_hit_flashes(&mut expired_snapshot, &expired);
        assert_eq!(expired_snapshot, before_snapshot);
        for status in [GameStatus::Defeat, GameStatus::Victory] {
            let mut terminal = before_state.clone();
            terminal.status = status;
            let mut terminal_snapshot = before_snapshot.clone();
            apply_hit_flashes(&mut terminal_snapshot, &terminal);
            assert_eq!(terminal_snapshot, before_snapshot);
        }
    }

    #[test]
    fn active_swing_and_lethal_hit_emit_visible_shapes_without_a_gpu() {
        let session = attacked_encounter();
        let active = session.state();
        assert!(!active.world.contains(10));
        assert!(active.hit_visuals.iter().any(|h| h.target == 10));
        let mut blank = active.clone();
        blank.attack_visuals.clear();
        blank.hit_visuals.clear();
        let empty_shapes = render_shapes(&blank, None);
        let active_shapes = render_shapes(active, None);
        assert!(leaves(&active_shapes).len() > leaves(&empty_shapes).len());
        assert!(
            leaves(&active_shapes)
                .iter()
                .any(|shape| matches!(shape, egui::Shape::Mesh(mesh)
            if mesh.vertices.iter().any(|v| v.color.a() > 0)))
        );
        assert!(leaves(&active_shapes).iter().any(
            |shape| matches!(shape, egui::Shape::LineSegment {stroke, ..}
            if stroke.color.r() > 200 && stroke.color.g() > 200 && stroke.color.a() > 0)
        ));

        let mut lethal_only = active.clone();
        lethal_only.attack_visuals.clear();
        lethal_only.hit_visuals.retain(|h| h.target == 10);
        let lethal_shapes = render_shapes(&lethal_only, None);
        assert!(leaves(&lethal_shapes).len() > leaves(&empty_shapes).len());
        assert_eq!(
            leaves(&lethal_shapes)
                .iter()
                .filter(|shape| matches!(shape, egui::Shape::Text(text)
            if text.galley.job.text == "−28"))
                .count(),
            2
        );
        lethal_only.time = lethal_only.hit_visuals[0].expires_at;
        assert_eq!(
            leaves(&render_shapes(&lethal_only, None)).len(),
            leaves(&empty_shapes).len()
        );
        let mut terminal = active.clone();
        terminal.status = GameStatus::Victory;
        assert_eq!(
            leaves(&render_shapes(&terminal, Some([0., 0., 3.]))).len(),
            leaves(&empty_shapes).len()
        );
    }

    #[test]
    fn reused_ids_keep_recorded_roles_without_blocking_new_preview_or_retargeting_old_swings() {
        use module_engine_core::WorldCommand;
        let cyan =
            egui::epaint::ColorMode::Solid(Color32::from_rgba_unmultiplied(125, 225, 255, 180));
        let enemy_stroke = egui::epaint::ColorMode::Solid(Color32::from_rgb(255, 90, 70));
        let player_stroke = egui::epaint::ColorMode::Solid(Color32::from_rgb(255, 206, 92));
        for windup in [0., 0.2] {
            let mut project = encounter().project().clone();
            let scene = project.scenes.get_mut("arena").unwrap();
            scene
                .objects
                .retain(|entity| [1, 11, 13].contains(&entity.object.id));
            let enemy = scene
                .objects
                .iter_mut()
                .find(|entity| entity.object.id == 11)
                .unwrap();
            enemy.components.insert(
                "actor".into(),
                json!({"team":"enemy","radius":0.35,"speed":0.}),
            );
            enemy
                .components
                .insert("ai".into(), json!({"kind":"chase","aggro_radius":100.}));
            enemy.components.insert("melee".into(), json!({"range":4.,"damage":0.,"cooldown":1.,"arc":std::f32::consts::TAU,"duration":1.,"windup":windup}));
            let mut session = GameSession::new(project, WorldKind::Ecs).unwrap();
            session
                .step(
                    1. / 60.,
                    &GameInput {
                        attack: true,
                        aim_world: [0.2, 0., 3.5],
                        ..Default::default()
                    },
                )
                .unwrap();
            let mut state = session.state().clone();
            let old_player_attack = state
                .attack_visuals
                .iter()
                .find(|attack| attack.attacker == 1)
                .unwrap()
                .clone();
            assert!(
                state
                    .hit_visuals
                    .iter()
                    .any(|hit| hit.target == 11 && hit.target_player == Some(false))
            );
            if windup == 0. {
                assert!(
                    state.attack_visuals.iter().any(
                        |attack| attack.attacker == 11 && attack.attacker_player == Some(false)
                    )
                );
            } else {
                assert!(state.telegraphs.iter().any(
                    |warning| warning.attacker == 11 && warning.attacker_player == Some(false)
                ));
            }
            state.attack_visuals.retain(|attack| attack.attacker == 11);
            state.telegraphs.retain(|warning| warning.attacker == 11);
            state.hit_visuals.retain(|hit| hit.target == 11);
            let mut replacement = state.world.entity(11).unwrap();
            replacement.object.transform.translation = [0., 0., 5.];
            replacement.components.insert("player".into(), json!({}));
            replacement.components.get_mut("actor").unwrap()["team"] = json!("player");
            replacement.components.get_mut("melee").unwrap()["range"] = json!(2.2);
            replacement.components.get_mut("melee").unwrap()["arc"] = json!(1.9);
            state
                .apply_commands(&[
                    WorldCommand::Remove { id: 11 },
                    WorldCommand::Spawn {
                        object: replacement.object.clone(),
                    },
                ])
                .unwrap();
            state.world.upsert_entity(replacement).unwrap();
            state.world.remove_component(1, "player");
            assert_eq!(state.player().unwrap().object.id, 11);

            let aim = Some([0., 0., 8.]);
            let mut preview_only = state.clone();
            preview_only.attack_visuals.clear();
            preview_only.telegraphs.clear();
            preview_only.hit_visuals.clear();
            let preview_shapes = render_shapes(&preview_only, aim);
            assert_eq!(target_rings(&preview_shapes).len(), 1);
            let shapes = render_shapes(&state, aim);
            assert!(
                leaves(&shapes).iter().any(
                    |shape| matches!(shape, egui::Shape::Path(path) if path.stroke.color == cyan)
                ),
                "An old incarnation's swing or windup must not suppress the fresh player's aim"
            );
            assert_eq!(
                target_rings(&shapes).len(),
                target_rings(&preview_shapes).len()
            );
            if windup == 0. {
                assert!(leaves(&shapes).iter().any(|shape| matches!(shape, egui::Shape::Path(path) if path.stroke.color == enemy_stroke)),
                    "The recorded enemy swing stays red after its ID becomes a player");
                assert!(!leaves(&shapes).iter().any(|shape| matches!(shape, egui::Shape::Path(path) if path.stroke.color == player_stroke)));
            }
            let hit = &state.hit_visuals[0];
            let age = ((state.time - hit.started_at) / (hit.expires_at - hit.started_at))
                .clamp(0., 1.) as f32;
            let alpha = (255. * (1. - age)) as u8;
            let outgoing = Color32::from_rgba_unmultiplied(255, 235, 145, alpha);
            let incoming = Color32::from_rgba_unmultiplied(255, 100, 90, alpha);
            assert!(leaves(&shapes).iter().any(|shape| matches!(shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == outgoing)));
            assert!(!leaves(&shapes).iter().any(|shape| matches!(shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == incoming)),
                "The recorded outgoing hit must not become incoming damage on a replacement player");

            // Also retain an actual friendly attack after its attacker is
            // replaced by an enemy. Its old gold shape remains, but it must
            // not query the replacement's opposing team for new target rings.
            state.attack_visuals = vec![old_player_attack];
            state.telegraphs.clear();
            state.hit_visuals.clear();
            let mut enemy = state.world.entity(1).unwrap();
            enemy.components.get_mut("actor").unwrap()["team"] = json!("enemy");
            state
                .apply_commands(&[
                    WorldCommand::Remove { id: 1 },
                    WorldCommand::Spawn {
                        object: enemy.object.clone(),
                    },
                ])
                .unwrap();
            state.world.upsert_entity(enemy).unwrap();
            state.world.transform_mut(11).unwrap().translation = [0., 0., 4.];
            assert!(
                !state
                    .melee_targets(1, &state.attack_visuals[0].area)
                    .is_empty()
            );
            let shapes = render_shapes(&state, None);
            assert!(
                target_rings(&shapes).is_empty(),
                "An old swing must not retarget the replacement actor"
            );
            assert!(leaves(&shapes).iter().any(|shape| matches!(shape, egui::Shape::Path(path) if path.stroke.color == player_stroke)));
        }
    }

    #[test]
    fn moving_cursor_during_a_swing_keeps_active_area_and_highlights_locked() {
        let mut session = attacked_encounter();
        let forward_shapes = render_shapes(session.state(), Some([0., 0., 3.]));
        let backward_shapes = render_shapes(session.state(), Some([0., 0., 8.]));
        let forward_rings = target_rings(&forward_shapes);
        let backward_rings = target_rings(&backward_shapes);
        assert_eq!(forward_rings.len(), 1); // Lethal target is already removed.
        assert_eq!(backward_rings.len(), 1);
        assert_eq!(forward_rings[0].points, backward_rings[0].points);
        let cyan =
            egui::epaint::ColorMode::Solid(Color32::from_rgba_unmultiplied(125, 225, 255, 180));
        assert!(
            !leaves(&backward_shapes)
                .iter()
                .any(|shape| matches!(shape, egui::Shape::Path(path)
            if path.stroke.color == cyan)),
            "A mouse preview must not suggest a different active hit area"
        );

        for _ in 0..10 {
            session
                .step(
                    1. / 60.,
                    &GameInput {
                        aim_world: [0., 0., 8.],
                        ..GameInput::default()
                    },
                )
                .unwrap();
        }
        assert!(session.state().attack_visuals.is_empty());
        let next_preview = render_shapes(session.state(), Some([0., 0., 8.]));
        let next_rings = target_rings(&next_preview);
        assert_eq!(next_rings.len(), 1);
        assert_ne!(next_rings[0].points, backward_rings[0].points);
        let target = session.state().world.entity(13).unwrap();
        let radius = target.components["actor"]["radius"].as_f64().unwrap() as f32;
        let expected = crate::player::screen_point(
            camera(),
            [
                target.object.transform.translation[0] + radius,
                0.04,
                target.object.transform.translation[2],
            ],
            viewport(),
        )
        .unwrap();
        assert_eq!(next_rings[0].points[0], expected);
        assert!(
            leaves(&next_preview)
                .iter()
                .any(|shape| matches!(shape, egui::Shape::Path(path)
            if path.stroke.color == cyan))
        );
    }

    #[test]
    fn player_windup_keeps_mouse_preview_hidden_until_captured_attack_finishes() {
        let mut project = encounter().project().clone();
        let player = project
            .scenes
            .get_mut("arena")
            .unwrap()
            .objects
            .iter_mut()
            .find(|e| e.object.id == 1)
            .unwrap();
        player.components.get_mut("melee").unwrap()["windup"] = json!(0.2);
        let mut session = GameSession::new(project, WorldKind::Objects).unwrap();
        session
            .step(
                1. / 60.,
                &GameInput {
                    attack: true,
                    aim_world: [0., 0., 3.],
                    ..GameInput::default()
                },
            )
            .unwrap();
        assert!(session.state().telegraphs.iter().any(|w| w.attacker == 1));
        assert!(session.state().attack_visuals.is_empty());
        let cyan =
            egui::epaint::ColorMode::Solid(Color32::from_rgba_unmultiplied(125, 225, 255, 180));
        let mut windup_frames = 0;
        let mut swing_frames = 0;
        for _ in 0..60 {
            let state = session.state();
            let preparing = state.telegraphs.iter().any(|w| w.attacker == 1);
            let swinging = state.attack_visuals.iter().any(|a| a.attacker == 1);
            if !preparing && !swinging {
                break;
            }
            windup_frames += usize::from(preparing);
            swing_frames += usize::from(swinging);
            for warning in state.telegraphs.iter().filter(|w| w.attacker == 1) {
                assert_eq!(warning.direction, [0., -1.]);
            }
            for attack in state.attack_visuals.iter().filter(|a| a.attacker == 1) {
                assert_eq!(attack.area.direction, [0., -1.]);
            }
            let shapes = render_shapes(state, Some([0., 0., 8.]));
            assert!(
                !leaves(&shapes)
                    .iter()
                    .any(|shape| matches!(shape, egui::Shape::Path(path)
                if path.stroke.color == cyan)),
                "Windup and active swing retain their original aim"
            );
            session
                .step(
                    1. / 60.,
                    &GameInput {
                        aim_world: [0., 0., 8.],
                        ..GameInput::default()
                    },
                )
                .unwrap();
        }
        assert!(windup_frames > 1);
        assert!(swing_frames > 1);
        assert!(!session.state().telegraphs.iter().any(|w| w.attacker == 1));
        assert!(
            !session
                .state()
                .attack_visuals
                .iter()
                .any(|a| a.attacker == 1)
        );
        let shapes = render_shapes(session.state(), Some([0., 0., 8.]));
        assert!(
            leaves(&shapes)
                .iter()
                .any(|shape| matches!(shape, egui::Shape::Path(path)
            if path.stroke.color == cyan))
        );
        assert_eq!(target_rings(&shapes).len(), 1);
        let area = session.state().melee_aim(1, [0., 0., 8.]).unwrap();
        assert_eq!(session.state().melee_targets(1, &area), vec![13]);
    }
}
