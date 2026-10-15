use crate::{ArpgInput, AttackTelegraph, GameInputExt, GameStateExt, GameStatus};
use egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use module_engine_core::{Camera, GameInput, GameState};
pub mod combat_visuals;

pub fn has_input(input: &GameInput) -> bool {
    input.movement() != [0., 0.] || input.attack() || input.interact() || input.restart
}

pub fn capture_input(state: &GameState) -> GameInput {
    let Some(player) = state.player() else {
        return GameInput::default();
    };
    let position = player.object.transform.translation;
    let distance =
        |target: [f32; 3]| (target[0] - position[0]).powi(2) + (target[2] - position[2]).powi(2);
    let target = state
        .world
        .entities_with(&["actor"])
        .into_iter()
        .filter(|id| {
            state
                .world
                .component(*id, "actor")
                .and_then(|actor| actor.get("team"))
                .and_then(serde_json::Value::as_str)
                == Some("enemy")
        })
        .filter_map(|id| state.world.object(id))
        .min_by(|a, b| {
            distance(a.transform.translation).total_cmp(&distance(b.transform.translation))
        });
    target.map_or_else(GameInput::default, |target| {
        ArpgInput {
            attack: true,
            aim_world: target.transform.translation,
            ..Default::default()
        }
        .into_input()
    })
}

pub fn preview_aim(
    state: &GameState,
    input: &GameInput,
    pointer_active: bool,
    capture_attack: bool,
) -> Option<[f32; 3]> {
    if pointer_active {
        return Some(input.aim_world());
    }
    if !capture_attack {
        return None;
    }
    state
        .arpg()
        .attack_visuals
        .iter()
        .find(|attack| state.world.component(attack.attacker, "player").is_some())
        .map(|attack| {
            [
                attack.area.center[0] + attack.area.direction[0] * attack.area.range,
                attack.area.center[1],
                attack.area.center[2] + attack.area.direction[1] * attack.area.range,
            ]
        })
}

pub fn read_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    camera: Camera,
) -> GameInput {
    read_input_mode(ui, response, rect, camera, true)
}

pub fn read_input_mode(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    camera: Camera,
    respect_editor_focus: bool,
) -> GameInput {
    let mut input = ArpgInput::default();
    if ui.input(|i| i.focused && i.pointer.any_pressed()) && response.hovered() {
        response.request_focus();
    }
    if ui.input(|i| !i.focused)
        || (respect_editor_focus && ui.ctx().egui_wants_keyboard_input() && !response.has_focus())
        || rect.width() <= 0.
        || rect.height() <= 0.
    {
        return GameInput::default();
    }
    ui.input(|i| {
        input.movement = [
            i.key_down(egui::Key::D) as u8 as f32 - i.key_down(egui::Key::A) as u8 as f32,
            i.key_down(egui::Key::S) as u8 as f32 - i.key_down(egui::Key::W) as u8 as f32,
        ];
        input.attack = response.hovered() && i.pointer.button_pressed(egui::PointerButton::Primary);
        input.interact = i.key_pressed(egui::Key::E);
        input.restart = i.key_pressed(egui::Key::R);
        if let Some(p) = i.pointer.hover_pos() {
            input.aim_world = aim_at(
                camera,
                [
                    (p.x - rect.center().x) / (rect.width() * 0.5),
                    (rect.center().y - p.y) / (rect.height() * 0.5),
                ],
                rect.aspect_ratio(),
            );
        }
    });
    input.into_input()
}
pub fn draw_hud(
    ui: &mut egui::Ui,
    rect: Rect,
    state: &GameState,
    camera: Camera,
    aim: Option<[f32; 3]>,
) -> bool {
    let frame = state.arpg();
    let painter = ui.painter_at(rect);
    for warning in frame
        .telegraphs
        .iter()
        .filter(|_| frame.status == GameStatus::Playing)
    {
        if let Some(shape) = telegraph_shape(camera, warning, rect) {
            painter.add(shape);
        }
    }
    combat_visuals::draw(ui, rect, camera, state, aim);
    let player = state.player();
    if let Some(player) = player {
        let health = player.components.get("health");
        let current = health
            .and_then(|h| h.get("current"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.);
        let maximum = health
            .and_then(|h| h.get("maximum"))
            .and_then(|v| v.as_f64())
            .unwrap_or(100.);
        let bar = Rect::from_min_size(rect.left_top() + Vec2::new(22., 22.), Vec2::new(230., 24.));
        painter.rect_filled(bar, 5., Color32::from_black_alpha(180));
        painter.rect_filled(
            Rect::from_min_size(
                bar.min,
                Vec2::new(
                    bar.width() * (current / maximum.max(1.)).clamp(0., 1.) as f32,
                    bar.height(),
                ),
            ),
            5.,
            Color32::from_rgb(80, 195, 140),
        );
        painter.text(
            bar.center(),
            egui::Align2::CENTER_CENTER,
            format!("HP {current:.0} / {maximum:.0}"),
            egui::FontId::proportional(15.),
            Color32::WHITE,
        );
    }
    let enemies = state
        .world
        .entities_with(&["actor"])
        .into_iter()
        .filter(|id| {
            state
                .world
                .component(*id, "actor")
                .and_then(|a| a.get("team"))
                .and_then(|t| t.as_str())
                == Some("enemy")
        })
        .count();
    painter.text(
        rect.left_top() + Vec2::new(22., 60.),
        egui::Align2::LEFT_TOP,
        format!(
            "{}  ·  Enemies {enemies}\nWASD move · LMB attack · R restart",
            state.scene
        ),
        egui::FontId::proportional(15.),
        Color32::WHITE,
    );
    for entity in state
        .world
        .entities_with(&["boss"])
        .into_iter()
        .filter_map(|id| state.world.entity(id))
    {
        let health = entity.components.get("health");
        let current = health
            .and_then(|h| h.get("current"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.);
        let maximum = health
            .and_then(|h| h.get("maximum"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.);
        let bar = Rect::from_min_size(
            Pos2::new(rect.center().x - 200., rect.top() + 24.),
            Vec2::new(400., 18.),
        );
        painter.rect_filled(bar, 4., Color32::from_black_alpha(180));
        painter.rect_filled(
            Rect::from_min_size(
                bar.min,
                Vec2::new(
                    bar.width() * (current / maximum.max(1.)) as f32,
                    bar.height(),
                ),
            ),
            4.,
            Color32::from_rgb(200, 70, 100),
        );
        painter.text(
            bar.center(),
            egui::Align2::CENTER_CENTER,
            format!("BOSS {current:.0} / {maximum:.0}"),
            egui::FontId::proportional(14.),
            Color32::WHITE,
        );
    }
    if frame.status != GameStatus::Playing {
        painter.rect_filled(
            Rect::from_center_size(rect.center(), Vec2::new(370., 130.)),
            12.,
            Color32::from_black_alpha(220),
        );
        painter.text(
            rect.center() - Vec2::new(0., 26.),
            egui::Align2::CENTER_CENTER,
            if frame.status == GameStatus::Victory {
                "VICTORY"
            } else {
                "DEFEAT"
            },
            egui::FontId::proportional(32.),
            Color32::WHITE,
        );
        if ui
            .put(
                Rect::from_center_size(rect.center() + Vec2::new(0., 27.), Vec2::new(150., 32.)),
                egui::Button::new("Restart / R"),
            )
            .clicked()
        {
            return true;
        }
    }
    false
}

pub fn telegraph_shape(
    camera: Camera,
    warning: &AttackTelegraph,
    rect: Rect,
) -> Option<egui::Shape> {
    if !warning.range.is_finite()
        || warning.range <= 0.
        || !warning.arc.is_finite()
        || warning.arc <= 0.
        || !warning
            .center
            .iter()
            .chain(&warning.direction)
            .all(|v| v.is_finite())
    {
        return None;
    }
    let arc = warning.arc.min(std::f32::consts::TAU);
    let angle = warning.direction[1].atan2(warning.direction[0]);
    let mut points = (0..=28)
        .map(|n| {
            let a = angle - arc * 0.5 + arc * n as f32 / 28.;
            screen_point(
                camera,
                [
                    warning.center[0] + a.cos() * warning.range,
                    0.02,
                    warning.center[2] + a.sin() * warning.range,
                ],
                rect,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    let fill = Color32::from_rgba_unmultiplied(
        245,
        60,
        55,
        65 + (warning.progress.clamp(0., 1.) * 95.) as u8,
    );
    let stroke = Stroke::new(2., Color32::LIGHT_RED);
    if arc >= std::f32::consts::TAU - 0.001 {
        // A full circle has no center in its boundary. Perspective projects it
        // into a convex ellipse; including its center breaks convex tessellation.
        points.pop();
        return Some(egui::Shape::convex_polygon(points, fill, stroke));
    }
    let center = screen_point(camera, warning.center, rect)?;
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, fill);
    for point in &points {
        mesh.colored_vertex(*point, fill);
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
pub fn screen_point(camera: Camera, p: [f32; 3], rect: Rect) -> Option<Pos2> {
    let yaw = camera.yaw;
    let pitch = camera.pitch;
    let offset = [
        yaw.sin() * pitch.cos() * camera.distance,
        pitch.sin() * camera.distance,
        yaw.cos() * pitch.cos() * camera.distance,
    ];
    let eye: [f32; 3] = std::array::from_fn(|i| camera.target[i] + offset[i]);
    let delta: [f32; 3] = std::array::from_fn(|i| p[i] - eye[i]);
    let forward = [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        -yaw.cos() * pitch.cos(),
    ];
    let right = [yaw.cos(), 0., -yaw.sin()];
    let up = [
        -yaw.sin() * pitch.sin(),
        pitch.cos(),
        -yaw.cos() * pitch.sin(),
    ];
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a.iter().zip(b).map(|(a, b)| a * b).sum()
    }
    let depth = dot(delta, forward);
    if depth < 0.08 {
        return None;
    }
    let f = rect.height() * 0.5 / (55_f32.to_radians() * 0.5).tan();
    Some(Pos2::new(
        rect.center().x + dot(delta, right) * f / depth,
        rect.center().y - dot(delta, up) * f / depth,
    ))
}

pub fn aim_at(camera: Camera, normalized: [f32; 2], aspect: f32) -> [f32; 3] {
    fn norm(a: [f32; 3]) -> [f32; 3] {
        let l = a.iter().map(|v| v * v).sum::<f32>().sqrt().max(0.000001);
        a.map(|v| v / l)
    }
    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }
    let [sy, cy] = [camera.yaw.sin(), camera.yaw.cos()];
    let [sp, cp] = [camera.pitch.sin(), camera.pitch.cos()];
    let eye = [
        camera.target[0] + sy * cp * camera.distance,
        camera.target[1] + sp * camera.distance,
        camera.target[2] + cy * cp * camera.distance,
    ];
    let forward = norm(std::array::from_fn(|i| camera.target[i] - eye[i]));
    let right = norm(cross(forward, [0., 1., 0.]));
    let up = cross(right, forward);
    let tangent = (55_f32.to_radians() * 0.5).tan();
    let ray: [f32; 3] = std::array::from_fn(|i| {
        forward[i] + right[i] * normalized[0] * aspect * tangent + up[i] * normalized[1] * tangent
    });
    if ray[1].abs() < 0.00001 {
        return camera.target;
    }
    let t = -eye[1] / ray[1];
    if t <= 0. {
        return camera.target;
    }
    [eye[0] + ray[0] * t, 0., eye[2] + ray[2] * t]
}
