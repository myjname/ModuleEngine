//! Authored Scene viewport presentation and interaction, independent of runtime.
//!
//! `interact` produces commands before `paint`: the host validates/applies them
//! and obtains a fresh neutral snapshot, so transform feedback renders in the
//! same frame. This window never mutates the world, files, or undo history.
use crate::{
    assets_ui::{AssetDrag, AssetKind},
    scene_tools::{self, Axis, DragGesture, GizmoGeometry, Snap, Space, Tool},
};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use module_engine_core::{Camera, Frame, SceneSnapshot, Transform, WorldObject, transform_point};

#[derive(Clone, Debug, PartialEq)]
pub enum SceneAction {
    Select(Option<u64>),
    BeginGesture { id: u64 },
    SetTransform { id: u64, transform: Transform },
    CommitGesture,
    CancelGesture { id: u64, transform: Transform },
    DropModel { asset_ref: String, point: [f32; 3] },
    DropTexture { asset_ref: String, id: u64 },
    Error(String),
}

pub struct SceneViewportState {
    /// Personal view state: the host can persist this in PaneSnapshot.
    pub camera: Camera,
    pub tool: Tool,
    pub space: Space,
    pub snap: Snap,
    pub gesture: Option<DragGesture>,
    pub rect: Option<Rect>,
    pub texture: Option<egui::TextureHandle>,
    pub error: Option<String>,
    hovered_axis: Option<Axis>,
    locked: bool,
    drop_hint: Option<(String, bool)>,
    render_revision: Option<u64>,
    rendered_key: Option<SceneRenderKey>,
    rendered_at: Option<f64>,
    rendered_frame: Option<u64>,
    render_count: u64,
    render_failure: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
struct SceneRenderKey {
    revision: Option<u64>,
    camera: Camera,
    size: [usize; 2],
}
impl Default for SceneViewportState {
    fn default() -> Self {
        Self {
            camera: Camera {
                yaw: 0.55,
                pitch: 0.65,
                distance: 20.,
                target: [0.; 3],
            },
            tool: Tool::Translate,
            space: Space::World,
            snap: Snap::default(),
            gesture: None,
            rect: None,
            texture: None,
            error: None,
            hovered_axis: None,
            locked: false,
            drop_hint: None,
            render_revision: None,
            rendered_key: None,
            rendered_at: None,
            rendered_frame: None,
            render_count: 0,
            render_failure: None,
        }
    }
}
impl SceneViewportState {
    /// Host revisions cover accepted scene/content/GPU changes. Camera and size
    /// are tracked here so interaction feedback never waits for the idle poll.
    pub fn set_render_revision(&mut self, revision: u64) {
        self.render_revision = Some(revision);
    }

    pub fn render_count(&self) -> u64 {
        self.render_count
    }

    pub fn with_camera(camera: Camera) -> Self {
        Self {
            camera,
            ..Self::default()
        }
    }
    /// Call before a scene switch, Play, Undo, or another document transition.
    pub fn cancel_gesture(&mut self) -> Option<SceneAction> {
        let gesture = self.gesture.take()?;
        self.hovered_axis = None;
        Some(SceneAction::CancelGesture {
            id: gesture.id,
            transform: gesture.cancel(),
        })
    }

    /// Allocate toolbar/canvas and return validated-by-host authoring requests.
    /// Only the focused canvas consumes G/R/S/F; text editors retain their input.
    pub fn interact(
        &mut self,
        ui: &mut egui::Ui,
        objects: &[WorldObject],
        snapshot: &SceneSnapshot,
        selected: Option<u64>,
        locked: bool,
    ) -> Vec<SceneAction> {
        let mut actions = vec![];
        let mut frame_selected = false;
        self.locked = locked;
        ui.horizontal_wrapped(|ui| {
            ui.strong("SCENE");
            ui.add_enabled_ui(!locked && self.gesture.is_none(), |ui| {
                for (tool, label) in [
                    (Tool::Translate, "Move [G]"),
                    (Tool::Rotate, "Rotate [R]"),
                    (Tool::Scale, "Scale [S]"),
                ] {
                    ui.selectable_value(&mut self.tool, tool, label);
                }
                ui.separator();
                ui.selectable_value(&mut self.space, Space::World, "World");
                ui.selectable_value(&mut self.space, Space::Local, "Local");
                ui.separator();
                ui.checkbox(&mut self.snap.enabled, "Snap");
                if self.snap.enabled {
                    match self.tool {
                        Tool::Translate => {
                            ui.add(
                                egui::DragValue::new(&mut self.snap.translation)
                                    .speed(0.05)
                                    .range(0.0001..=100000.)
                                    .prefix("Move: "),
                            );
                        }
                        Tool::Rotate => {
                            ui.add(
                                egui::DragValue::new(&mut self.snap.rotation_degrees)
                                    .speed(1.)
                                    .range(0.1..=180.)
                                    .prefix("Rotate: ")
                                    .suffix("°"),
                            );
                        }
                        Tool::Scale => {
                            ui.add(
                                egui::DragValue::new(&mut self.snap.scale)
                                    .speed(0.01)
                                    .range(0.001..=100.)
                                    .prefix("Scale: "),
                            );
                        }
                    }
                }
            });
            if ui
                .add_enabled(
                    selected.is_some() && self.gesture.is_none(),
                    egui::Button::new("Frame [F]"),
                )
                .clicked()
            {
                frame_selected = true;
            }
            if locked {
                ui.label("Play · редактирование доступно после Stop");
            }
        });
        let size = ui.available_size().max(Vec2::splat(1.));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        self.rect = Some(rect);
        self.drop_hint = None;
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let primary_pressed =
            ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary));
        let primary_released =
            ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary));
        if response.contains_pointer()
            && (primary_pressed
                || ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Middle)))
        {
            response.request_focus();
        }
        let keyboard = response.has_focus() && !ui.ctx().text_edit_focused();
        if keyboard {
            frame_selected |=
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F));
            if !locked && self.gesture.is_none() {
                for (key, tool) in [
                    (egui::Key::G, Tool::Translate),
                    (egui::Key::R, Tool::Rotate),
                    (egui::Key::S, Tool::Scale),
                ] {
                    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key)) {
                        self.tool = tool;
                    }
                }
            }
        }
        let cancel = self.gesture.is_some()
            && (locked
                || !ui.input(|input| input.focused)
                || self.gesture.as_ref().is_some_and(|gesture| {
                    selected != Some(gesture.id)
                        || !objects.iter().any(|object| object.id == gesture.id)
                })
                // egui releases widget focus on Escape before our response is
                // read. An active gesture still owns Escape unless a text
                // editor has explicitly acquired keyboard focus.
                || (!ui.ctx().text_edit_focused()
                    && ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                    })));
        let mut handled_gesture = false;
        if cancel {
            if let Some(action) = self.cancel_gesture() {
                actions.push(action);
            }
            handled_gesture = true;
        }
        if self.gesture.is_none() && !handled_gesture {
            self.camera_input(ui, &response, rect);
            if frame_selected && let Some(id) = selected {
                self.focus_object(objects, snapshot, id, rect);
            }
        }
        let geometry = (!locked)
            .then(|| {
                selected.and_then(|id| {
                    GizmoGeometry::build(objects, id, self.tool, self.space, self.camera, rect).ok()
                })
            })
            .flatten();
        self.hovered_axis = pointer
            .filter(|pointer| rect.contains(*pointer))
            .and_then(|pointer| {
                geometry
                    .as_ref()
                    .and_then(|geometry| geometry.hit_test(pointer))
            });
        if self.hovered_axis.is_some() && !locked && self.gesture.is_none() && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        let dragging_asset = egui::DragAndDrop::has_payload_of_type::<AssetDrag>(ui.ctx());
        if !locked
            && !handled_gesture
            && self.gesture.is_none()
            && !dragging_asset
            && primary_pressed
            && response.contains_pointer()
            && let (Some(id), Some(axis), Some(pointer)) = (selected, self.hovered_axis, pointer)
        {
            match DragGesture::begin(
                objects,
                id,
                self.tool,
                self.space,
                axis,
                self.camera,
                rect,
                pointer,
            ) {
                Ok(gesture) => {
                    self.gesture = Some(gesture);
                    self.error = None;
                    actions.push(SceneAction::BeginGesture { id });
                }
                Err(error) => self.report_error(error, &mut actions),
            }
            handled_gesture = true;
        }
        if let Some(gesture) = &self.gesture {
            handled_gesture = true;
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(pointer) = pointer {
                match gesture.update(pointer, self.snap) {
                    Ok(transform) => {
                        self.error = None;
                        actions.push(SceneAction::SetTransform {
                            id: gesture.id,
                            transform,
                        });
                    }
                    Err(error) => self.report_error(error, &mut actions),
                }
            }
            if primary_released
                || !ui.input(|input| input.pointer.button_down(egui::PointerButton::Primary))
            {
                self.gesture = None;
                actions.push(SceneAction::CommitGesture);
            }
            ui.ctx().request_repaint();
        }
        if !handled_gesture
            && response.clicked_by(egui::PointerButton::Primary)
            && !dragging_asset
            && let Some(pointer) = pointer
        {
            let id = scene_tools::camera_ray(self.camera, rect, pointer)
                .and_then(|ray| scene_tools::pick(snapshot, ray))
                .map(|hit| hit.id);
            actions.push(SceneAction::Select(id));
        }
        self.asset_drop(ui, &response, rect, snapshot, locked, &mut actions);
        actions
    }

    /// Draw only after the host has applied interact() actions and updated its snapshot.
    pub fn paint(
        &mut self,
        ui: &mut egui::Ui,
        objects: &[WorldObject],
        snapshot: &SceneSnapshot,
        selected: Option<u64>,
        render: &mut dyn FnMut(Camera, usize, usize) -> Result<Frame, String>,
    ) {
        let Some(rect) = self
            .rect
            .filter(|rect| rect.width() > 2. && rect.height() > 2.)
        else {
            return;
        };
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(rect, 0., Color32::from_rgb(21, 29, 43));
        let dpi = ui.ctx().pixels_per_point();
        let physical = rect.size() * dpi;
        let fit = (1280. / physical.x).min(720. / physical.y).min(1.);
        let width = (physical.x * fit).round().max(1.) as usize;
        let height = (physical.y * fit).round().max(1.) as usize;
        let now = ui.input(|input| input.time);
        let frame_number = ui.ctx().cumulative_frame_nr();
        let key = SceneRenderKey {
            revision: self.render_revision,
            camera: self.camera,
            size: [width, height],
        };
        let expired = self
            .rendered_at
            .is_none_or(|previous| now < previous || now - previous >= 0.25);
        let untracked_frame =
            self.render_revision.is_none() && self.rendered_frame != Some(frame_number);
        if self.rendered_key != Some(key) || expired || untracked_frame {
            self.rendered_key = Some(key);
            self.rendered_at = Some(now);
            self.rendered_frame = Some(frame_number);
            self.render_count += 1;
            match render(self.camera, width, height) {
                Ok(frame) if valid_frame(&frame) => {
                    if self
                        .render_failure
                        .as_ref()
                        .is_some_and(|error| self.error.as_ref() == Some(error))
                    {
                        self.error = None;
                    }
                    self.render_failure = None;
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [frame.width, frame.height],
                        &frame.rgba,
                    );
                    if let Some(texture) = &mut self.texture {
                        texture.set(image, egui::TextureOptions::LINEAR);
                    } else {
                        self.texture = Some(ui.ctx().load_texture(
                            format!("scene-preview-{:?}", ui.id()),
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                Ok(_) => {
                    let error = "Scene renderer returned malformed frame pixels".to_owned();
                    self.render_failure = Some(error.clone());
                    self.error = Some(error);
                }
                Err(error) => {
                    self.render_failure = Some(error.clone());
                    self.error = Some(error);
                }
            }
        }
        if self.render_revision.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        if let Some(texture) = &self.texture {
            painter.image(
                texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                Color32::WHITE,
            );
        }
        if let Some(id) = selected {
            if let Some(bounds) = object_bounds(snapshot, id) {
                draw_bounds(&painter, self.camera, rect, bounds);
            }
            if !self.locked
                && let Ok(geometry) =
                    GizmoGeometry::build(objects, id, self.tool, self.space, self.camera, rect)
            {
                let active = self.gesture.as_ref().map(|gesture| gesture.axis);
                for axis in &geometry.axes {
                    if !axis.direction.iter().all(|value| value.is_finite()) {
                        continue;
                    }
                    let color = axis_color(axis.axis, active, self.hovered_axis);
                    painter.line_segment([axis.start, axis.end], Stroke::new(3., color));
                    match self.tool {
                        Tool::Scale => {
                            painter.rect_filled(
                                Rect::from_center_size(axis.end, Vec2::splat(9.)),
                                1.,
                                color,
                            );
                        }
                        _ => {
                            let direction = axis.end - axis.start;
                            if direction.length_sq() > 1. {
                                let direction = direction.normalized();
                                let normal = Vec2::new(-direction.y, direction.x);
                                painter.add(egui::Shape::convex_polygon(
                                    vec![
                                        axis.end,
                                        axis.end - direction * 12. + normal * 5.,
                                        axis.end - direction * 12. - normal * 5.,
                                    ],
                                    color,
                                    Stroke::NONE,
                                ));
                            }
                        }
                    }
                }
                for ring in &geometry.rings {
                    let color = axis_color(ring.axis, active, self.hovered_axis);
                    for &segment in &ring.segments {
                        painter.line_segment(
                            segment,
                            Stroke::new(if active == Some(ring.axis) { 3. } else { 2. }, color),
                        );
                    }
                }
                painter.circle_filled(geometry.center, 3., Color32::WHITE);
            }
        }
        if let Some((hint, allowed)) = &self.drop_hint {
            let color = if *allowed {
                Color32::from_rgb(89, 220, 158)
            } else {
                Color32::from_rgb(245, 145, 135)
            };
            painter.rect_stroke(
                rect.shrink(2.),
                0.,
                Stroke::new(2., color),
                egui::StrokeKind::Inside,
            );
            painter.text(
                rect.center_top() + Vec2::new(0., 22.),
                egui::Align2::CENTER_CENTER,
                hint,
                FontId::proportional(13.),
                color,
            );
        }
        let help = if self.gesture.is_some() {
            "Esc: отменить · отпустить ЛКМ: применить"
        } else {
            "ЛКМ: выбрать · G/R/S: инструмент · СКМ: орбита · Shift+СКМ: сдвиг · колесо: масштаб · F: фокус"
        };
        painter.text(
            rect.left_bottom() + Vec2::new(9., -9.),
            egui::Align2::LEFT_BOTTOM,
            help,
            FontId::proportional(11.),
            Color32::from_rgb(187, 201, 216),
        );
        if let Some(error) = &self.error {
            painter.text(
                rect.left_top() + Vec2::splat(9.),
                egui::Align2::LEFT_TOP,
                error,
                FontId::proportional(12.),
                Color32::from_rgb(245, 145, 135),
            );
        }
    }

    fn report_error(&mut self, error: String, actions: &mut Vec<SceneAction>) {
        if self.error.as_ref() != Some(&error) {
            actions.push(SceneAction::Error(error.clone()));
        }
        self.error = Some(error);
    }
    fn camera_input(&mut self, ui: &egui::Ui, response: &egui::Response, rect: Rect) {
        if response.dragged_by(egui::PointerButton::Middle) {
            let delta = ui.input(|input| input.pointer.delta());
            if ui.input(|input| input.modifiers.shift) {
                if let Some(pointer) = ui.input(|input| input.pointer.interact_pos())
                    && let Some(center_ray) =
                        scene_tools::camera_ray(self.camera, rect, rect.center())
                    && let Some(previous) = scene_tools::camera_ray(
                        self.camera,
                        rect,
                        pointer - delta,
                    )
                    .and_then(|ray| {
                        scene_tools::plane_hit(ray, self.camera.target, center_ray.direction)
                    })
                    && let Some(current) = scene_tools::camera_ray(self.camera, rect, pointer)
                        .and_then(|ray| {
                            scene_tools::plane_hit(ray, self.camera.target, center_ray.direction)
                        })
                {
                    for index in 0..3 {
                        self.camera.target[index] += previous[index] - current[index];
                    }
                }
            } else {
                self.camera.yaw -= delta.x * 0.009;
                self.camera.pitch = (self.camera.pitch + delta.y * 0.007).clamp(-1.45, 1.45);
            }
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta().y);
            if scroll != 0. {
                self.camera.distance =
                    (self.camera.distance * (-scroll * 0.002).exp()).clamp(0.2, 1000.);
            }
        }
    }
    fn focus_object(
        &mut self,
        objects: &[WorldObject],
        snapshot: &SceneSnapshot,
        id: u64,
        rect: Rect,
    ) {
        if let Some([min, max]) = object_bounds(snapshot, id) {
            self.camera.target = std::array::from_fn(|index| (min[index] + max[index]) * 0.5);
            let radius = (0..3)
                .map(|index| ((max[index] - min[index]) * 0.5).powi(2))
                .sum::<f32>()
                .sqrt()
                .max(0.1);
            self.camera.distance =
                (radius / (55_f32.to_radians() * 0.5).tan() / rect.aspect_ratio().min(1.) * 1.5)
                    .clamp(0.2, 1000.);
        } else if let Ok(matrices) = scene_tools::object_matrices(objects, id) {
            self.camera.target = std::array::from_fn(|row| matrices.world[row][3]);
        }
    }
    fn asset_drop(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        snapshot: &SceneSnapshot,
        locked: bool,
        actions: &mut Vec<SceneAction>,
    ) {
        if let Some(payload) = response.dnd_hover_payload::<AssetDrag>() {
            let pointer = ui.input(|input| input.pointer.interact_pos());
            let ray =
                pointer.and_then(|pointer| scene_tools::camera_ray(self.camera, rect, pointer));
            let (hint, allowed) = if locked {
                ("Редактирование доступно после Stop", false)
            } else {
                match payload.kind {
                    AssetKind::Model => (
                        "Отпустите: создать объект с моделью",
                        ray.and_then(|ray| scene_tools::placement(snapshot, ray))
                            .is_some(),
                    ),
                    AssetKind::Texture => (
                        "Отпустите над объектом: назначить текстуру",
                        ray.and_then(|ray| scene_tools::pick(snapshot, ray))
                            .is_some(),
                    ),
                    _ => ("Этот ресурс не назначается через Scene", false),
                }
            };
            self.drop_hint = Some((hint.into(), allowed));
        }
        if let Some(payload) = response.dnd_release_payload::<AssetDrag>() {
            if locked {
                return;
            }
            let Some(ray) = ui
                .input(|input| input.pointer.interact_pos())
                .and_then(|pointer| scene_tools::camera_ray(self.camera, rect, pointer))
            else {
                return;
            };
            match payload.kind {
                AssetKind::Model => {
                    if let Some(point) = scene_tools::placement(snapshot, ray) {
                        actions.push(SceneAction::DropModel {
                            asset_ref: payload.reference.clone(),
                            point,
                        });
                    } else {
                        self.report_error(
                            "Под курсором нет поверхности или земли для размещения модели".into(),
                            actions,
                        );
                    }
                }
                AssetKind::Texture => {
                    if let Some(hit) = scene_tools::pick(snapshot, ray) {
                        actions.push(SceneAction::DropTexture {
                            asset_ref: payload.reference.clone(),
                            id: hit.id,
                        });
                    } else {
                        self.report_error(
                            "Перетащите текстуру на существующий объект".into(),
                            actions,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

fn valid_frame(frame: &Frame) -> bool {
    frame.width > 0
        && frame.height > 0
        && frame
            .width
            .checked_mul(frame.height)
            .and_then(|pixels| pixels.checked_mul(4))
            == Some(frame.rgba.len())
}
fn axis_color(axis: Axis, active: Option<Axis>, hovered: Option<Axis>) -> Color32 {
    if active == Some(axis) || hovered == Some(axis) {
        return Color32::from_rgb(255, 219, 79);
    }
    match axis {
        Axis::X => Color32::from_rgb(235, 83, 80),
        Axis::Y => Color32::from_rgb(111, 215, 103),
        Axis::Z => Color32::from_rgb(99, 161, 249),
    }
}
fn object_bounds(snapshot: &SceneSnapshot, id: u64) -> Option<[[f32; 3]; 2]> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut found = false;
    for item in snapshot.items.iter().filter(|item| item.id == id) {
        for &vertex in &item.mesh.vertices {
            let point = transform_point(item.model, vertex);
            if !point.iter().all(|value| value.is_finite()) {
                continue;
            }
            for index in 0..3 {
                min[index] = min[index].min(point[index]);
                max[index] = max[index].max(point[index]);
            }
            found = true;
        }
    }
    found.then_some([min, max])
}
fn draw_bounds(painter: &egui::Painter, camera: Camera, rect: Rect, [min, max]: [[f32; 3]; 2]) {
    let corners: [Option<Pos2>; 8] = std::array::from_fn(|index| {
        scene_tools::project_point(
            camera,
            rect,
            std::array::from_fn(|axis| {
                if index & (1 << axis) == 0 {
                    min[axis]
                } else {
                    max[axis]
                }
            }),
        )
    });
    for index in 0..8 {
        for axis in 0..3 {
            let other = index ^ (1 << axis);
            if index < other
                && let (Some(a), Some(b)) = (corners[index], corners[other])
            {
                painter.line_segment([a, b], Stroke::new(1.5, Color32::from_rgb(255, 185, 63)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_core::{Material, Mesh, RenderItem, RenderStats};
    fn objects() -> Vec<WorldObject> {
        vec![WorldObject {
            id: 7,
            parent: None,
            transform: Transform::default(),
            color: [255; 4],
        }]
    }
    fn snapshot() -> SceneSnapshot {
        SceneSnapshot {
            items: vec![RenderItem {
                id: 7,
                mesh: Mesh::cube(),
                model: Transform::default().matrix(),
                color: [255; 4],
                material: Material::default(),
            }],
            time: 0.,
        }
    }
    fn input(time: f64, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            time: Some(time),
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000., 700.))),
            events,
            ..Default::default()
        }
    }
    fn button(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    fn frame(
        ctx: &egui::Context,
        state: &mut SceneViewportState,
        events: Vec<egui::Event>,
        selected: Option<u64>,
        locked: bool,
        time: f64,
    ) -> Vec<SceneAction> {
        let mut actions = vec![];
        let output = ctx.run_ui(input(time, events), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                actions = state.interact(ui, &objects(), &snapshot(), selected, locked);
            });
        });
        output.drop_without_applying_deltas();
        actions
    }
    #[test]
    fn scene_click_picks_actual_object_and_empty_click_clears_selection() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState::default();
        frame(&ctx, &mut state, vec![], None, false, 0.);
        let center =
            scene_tools::project_point(state.camera, state.rect.unwrap(), [0.; 3]).unwrap();
        frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(center), button(center, true)],
            None,
            false,
            0.1,
        );
        let actions = frame(
            &ctx,
            &mut state,
            vec![button(center, false)],
            None,
            false,
            0.2,
        );
        assert!(actions.contains(&SceneAction::Select(Some(7))));
        let empty = state.rect.unwrap().left_top() + Vec2::splat(20.);
        frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(empty), button(empty, true)],
            Some(7),
            false,
            0.3,
        );
        let actions = frame(
            &ctx,
            &mut state,
            vec![button(empty, false)],
            Some(7),
            false,
            0.4,
        );
        assert!(actions.contains(&SceneAction::Select(None)));
    }
    #[test]
    fn handle_drag_begins_updates_cancels_with_escape_and_release_commits_once() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState::default();
        frame(&ctx, &mut state, vec![], Some(7), false, 0.);
        let geometry = GizmoGeometry::build(
            &objects(),
            7,
            state.tool,
            state.space,
            state.camera,
            state.rect.unwrap(),
        )
        .unwrap();
        let handle = geometry
            .axes
            .iter()
            .find(|axis| axis.axis == Axis::X)
            .unwrap()
            .end;
        let start = frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(handle), button(handle, true)],
            Some(7),
            false,
            0.1,
        );
        assert!(start.contains(&SceneAction::BeginGesture { id: 7 }));
        let updates = frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(handle + Vec2::new(20., 0.))],
            Some(7),
            false,
            0.2,
        );
        assert!(updates.iter().any(|action|matches!(action,SceneAction::SetTransform{id:7,transform} if transform.translation != [0.;3])));
        let canceled = frame(
            &ctx,
            &mut state,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            Some(7),
            false,
            0.3,
        );
        assert!(canceled.contains(&SceneAction::CancelGesture {
            id: 7,
            transform: Transform::default()
        }));
        assert!(state.gesture.is_none());
        frame(
            &ctx,
            &mut state,
            vec![button(handle, false)],
            Some(7),
            false,
            0.4,
        );
        frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(handle), button(handle, true)],
            Some(7),
            false,
            0.5,
        );
        let released = frame(
            &ctx,
            &mut state,
            vec![
                egui::Event::PointerMoved(handle + Vec2::new(25., 0.)),
                button(handle + Vec2::new(25., 0.), false),
            ],
            Some(7),
            false,
            0.6,
        );
        assert_eq!(
            released
                .iter()
                .filter(|action| matches!(action, SceneAction::CommitGesture))
                .count(),
            1
        );
        assert!(state.gesture.is_none());
        assert!(
            !frame(&ctx, &mut state, vec![], Some(7), false, 0.7)
                .contains(&SceneAction::CommitGesture)
        );
    }
    #[test]
    fn play_lock_and_lost_window_focus_restore_gesture_baseline() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState::default();
        frame(&ctx, &mut state, vec![], Some(7), false, 0.);
        let start = state.rect.unwrap().center();
        state.gesture = Some(
            DragGesture::begin(
                &objects(),
                7,
                Tool::Translate,
                Space::World,
                Axis::X,
                state.camera,
                state.rect.unwrap(),
                start,
            )
            .unwrap(),
        );
        let actions = frame(&ctx, &mut state, vec![], Some(7), true, 0.1);
        assert!(actions.contains(&SceneAction::CancelGesture {
            id: 7,
            transform: Transform::default()
        }));
        let geometry = GizmoGeometry::build(
            &objects(),
            7,
            state.tool,
            state.space,
            state.camera,
            state.rect.unwrap(),
        )
        .unwrap();
        let handle = geometry.axes[0].end;
        let actions = frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(handle), button(handle, true)],
            Some(7),
            true,
            0.2,
        );
        assert!(!actions.iter().any(|action| matches!(
            action,
            SceneAction::BeginGesture { .. } | SceneAction::SetTransform { .. }
        )));
        state.gesture = Some(
            DragGesture::begin(
                &objects(),
                7,
                Tool::Translate,
                Space::World,
                Axis::X,
                state.camera,
                state.rect.unwrap(),
                start,
            )
            .unwrap(),
        );
        let mut lost_focus = input(0.3, vec![]);
        lost_focus.focused = false;
        let mut actions = vec![];
        let output = ctx.run_ui(lost_focus, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                actions = state.interact(ui, &objects(), &snapshot(), Some(7), false);
            });
        });
        output.drop_without_applying_deltas();
        assert!(actions.contains(&SceneAction::CancelGesture {
            id: 7,
            transform: Transform::default()
        }));
    }
    #[test]
    fn shortcuts_require_scene_focus_and_text_edit_keeps_its_keyboard() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState::default();
        let key = || egui::Event::Key {
            key: egui::Key::R,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut state, vec![key()], None, false, 0.);
        assert_eq!(state.tool, Tool::Translate);
        let center = state.rect.unwrap().center();
        frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(center), button(center, true)],
            None,
            false,
            0.1,
        );
        frame(
            &ctx,
            &mut state,
            vec![button(center, false)],
            None,
            false,
            0.2,
        );
        frame(&ctx, &mut state, vec![key()], None, false, 0.3);
        assert_eq!(state.tool, Tool::Rotate);
        let mut text = String::new();
        let output = ctx.run_ui(input(0.4, vec![]), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.text_edit_singleline(&mut text).request_focus();
                state.interact(ui, &objects(), &snapshot(), None, false);
            });
        });
        output.drop_without_applying_deltas();
        state.tool = Tool::Translate;
        let output = ctx.run_ui(
            input(0.5, vec![key(), egui::Event::Text("r".into())]),
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.text_edit_singleline(&mut text);
                    state.interact(ui, &objects(), &snapshot(), None, false);
                });
            },
        );
        output.drop_without_applying_deltas();
        assert_eq!(text, "r");
        assert_eq!(state.tool, Tool::Translate);
    }
    #[test]
    fn typed_resource_drop_returns_id_reference_and_play_blocks_authoring() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState::default();
        frame(&ctx, &mut state, vec![], None, false, 0.);
        let center = state.rect.unwrap().center();
        let payload = |kind| AssetDrag {
            id: "same-file".into(),
            reference: "asset://same-file".into(),
            path: "assets/example.glb".into(),
            kind,
        };
        for (index, kind) in [AssetKind::Model, AssetKind::Texture]
            .into_iter()
            .enumerate()
        {
            let time = 0.1 + index as f64 * 0.3;
            frame(
                &ctx,
                &mut state,
                vec![egui::Event::PointerMoved(center), button(center, true)],
                None,
                false,
                time,
            );
            egui::DragAndDrop::set_payload(&ctx, payload(kind));
            let actions = frame(
                &ctx,
                &mut state,
                vec![button(center, false)],
                None,
                false,
                time + 0.1,
            );
            assert!(
                !actions
                    .iter()
                    .any(|action| matches!(action, SceneAction::Select(_)))
            );
            if kind == AssetKind::Model {
                assert!(actions.iter().any(|action|matches!(action,SceneAction::DropModel{asset_ref,..} if asset_ref=="asset://same-file")));
            } else {
                assert!(actions.contains(&SceneAction::DropTexture {
                    asset_ref: "asset://same-file".into(),
                    id: 7
                }));
            }
        }
        frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(center), button(center, true)],
            None,
            true,
            0.8,
        );
        egui::DragAndDrop::set_payload(&ctx, payload(AssetKind::Model));
        let actions = frame(
            &ctx,
            &mut state,
            vec![button(center, false)],
            None,
            true,
            0.9,
        );
        assert!(!actions.iter().any(|action| matches!(
            action,
            SceneAction::DropModel { .. } | SceneAction::DropTexture { .. }
        )));
    }
    #[test]
    fn static_scene_reuses_pixels_but_camera_size_revision_and_idle_poll_refresh() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState {
            rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(160., 90.))),
            ..Default::default()
        };
        state.set_render_revision(1);
        let authored = objects();
        let snapshot = snapshot();
        let draw = |state: &mut SceneViewportState, time, selected, fail| {
            ctx.run_ui(input(time, vec![]), |ui| {
                state.paint(
                    ui,
                    &authored,
                    &snapshot,
                    selected,
                    &mut |_camera, width, height| {
                        if fail {
                            return Err("expected render failure".into());
                        }
                        Ok(Frame {
                            width,
                            height,
                            rgba: vec![128; width * height * 4],
                            stats: RenderStats::default(),
                        })
                    },
                );
            })
            .drop_without_applying_deltas();
        };
        draw(&mut state, 0., None, false);
        let texture = state.texture.as_ref().unwrap().id();
        assert_eq!(state.render_count(), 1);
        for frame in 1..=5 {
            // Selection/gizmo overlays must not trigger a 3D render either.
            draw(&mut state, frame as f64 * 0.01, Some(7), true);
        }
        assert_eq!(state.render_count(), 1);
        assert!(state.error.is_none());
        state.camera.yaw += 0.1;
        draw(&mut state, 0.06, None, false);
        assert_eq!(state.render_count(), 2);
        state.set_render_revision(2);
        draw(&mut state, 0.07, None, false);
        assert_eq!(state.render_count(), 3);
        state.rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(180., 90.)));
        draw(&mut state, 0.08, None, false);
        assert_eq!(state.render_count(), 4);
        draw(&mut state, 0.34, None, false);
        assert_eq!(
            state.render_count(),
            5,
            "Idle polling still observes external HLSL changes"
        );
        state.set_render_revision(3);
        draw(&mut state, 0.35, None, true);
        assert_eq!(state.render_count(), 6);
        assert_eq!(state.texture.as_ref().unwrap().id(), texture);
        assert_eq!(state.error.as_deref(), Some("expected render failure"));
        draw(&mut state, 0.36, None, true);
        assert_eq!(
            state.render_count(),
            6,
            "A failed refresh must not retry for every pan event"
        );
        state.set_render_revision(4);
        draw(&mut state, 0.37, None, false);
        assert_eq!(state.render_count(), 7);
        assert!(state.error.is_none());
        assert_eq!(state.texture.as_ref().unwrap().id(), texture);
    }

    #[test]
    fn painter_preserves_aspect_and_uses_updated_host_snapshot_without_world_mutation() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState::default();
        let authored = objects();
        let before = authored.clone();
        let mut requested = (0, 0);
        let output = ctx.run_ui(input(0., vec![]), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                state.interact(ui, &authored, &snapshot(), Some(7), false);
                state.paint(
                    ui,
                    &authored,
                    &snapshot(),
                    Some(7),
                    &mut |_camera, width, height| {
                        requested = (width, height);
                        Ok(Frame {
                            width,
                            height,
                            rgba: vec![255; width * height * 4],
                            stats: RenderStats::default(),
                        })
                    },
                );
            });
        });
        output.drop_without_applying_deltas();
        assert!(requested.0 > 0 && requested.0 <= 1280 && requested.1 > 0 && requested.1 <= 720);
        assert!(
            (requested.0 as f32 / requested.1 as f32 - state.rect.unwrap().aspect_ratio()).abs()
                < 0.01
        );
        assert_eq!(authored, before);
        assert!(state.texture.is_some());
    }
}
