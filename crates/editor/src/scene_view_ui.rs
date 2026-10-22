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
    gpu_texture: module_engine_player::gpu_presentation::GpuTexture,
    gpu_bridge:
        Option<std::sync::Arc<module_engine_player::gpu_presentation::GpuPresentationBridge>>,
    displayed_camera: Option<Camera>,
    presentation_owner: u64,
    pub error: Option<String>,
    hovered_axis: Option<Axis>,
    locked: bool,
    drop_hint: Option<(String, bool)>,
    render_revision: Option<u64>,
    rendered_key: Option<SceneRenderKey>,
    displayed_key: Option<SceneRenderKey>,
    presentation_pending: bool,
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

/// Frozen producer metadata travels with its completed pixels. The current
/// viewport camera can already belong to a newer request by the time it arrives.
pub struct PresentedSceneFrame {
    pub frame: Frame,
    pub camera: Camera,
    pub revision: Option<u64>,
    pub pending: bool,
}

fn render_size(rect: Rect, dpi: f32) -> [usize; 2] {
    let physical = rect.size() * dpi;
    let fit = (1280. / physical.x).min(720. / physical.y).min(1.);
    [
        (physical.x * fit).round().max(1.) as usize,
        (physical.y * fit).round().max(1.) as usize,
    ]
}

fn next_presentation_owner() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn scene_owns_press(response: &egui::Response, pointer: Pos2) -> bool {
    let ctx = &response.ctx;
    let local_pointer = ctx
        .layer_transform_from_global(response.layer_id)
        .map_or(pointer, |transform| transform * pointer);
    if !response.enabled()
        || !response.interact_rect.contains(local_pointer)
        || !ctx.memory(|memory| memory.allows_interaction(response.layer_id))
        || ctx.layer_id_at(pointer) != Some(response.layer_id)
    {
        return false;
    }
    // egui's current interaction snapshot uses the final pointer position of
    // the input frame. Check origin ownership against the same previous-pass
    // widget rectangles, without letting a later same-layer control be crossed.
    ctx.viewport(|viewport| {
        let widgets = &viewport.prev_pass.widgets;
        let Some((layer, order)) = widgets.order(response.id) else {
            return false;
        };
        layer == response.layer_id
            && !widgets.get_layer(layer).skip(order + 1).any(|widget| {
                widget.enabled
                    && widget.sense.interactive()
                    && widget.interact_rect.contains(local_pointer)
            })
    })
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
            gpu_texture: Default::default(),
            gpu_bridge: None,
            displayed_camera: None,
            presentation_owner: next_presentation_owner(),
            error: None,
            hovered_axis: None,
            locked: false,
            drop_hint: None,
            render_revision: None,
            rendered_key: None,
            displayed_key: None,
            presentation_pending: false,
            rendered_at: None,
            rendered_frame: None,
            render_count: 0,
            render_failure: None,
        }
    }
}
impl SceneViewportState {
    /// Process-local routing only; personal/authored serialization never stores it.
    pub fn presentation_owner(&self) -> u64 {
        self.presentation_owner
    }
    pub fn request_presentation_refresh(&mut self) {
        self.presentation_pending = true;
    }
    /// The same selected image painted by this viewport, independent of whether
    /// its representation is a software texture or a native GPU display.
    pub fn texture_id(&self) -> Option<egui::TextureId> {
        module_engine_player::ui::presentation_texture_id(&self.texture, &self.gpu_texture)
    }
    pub fn poll_gpu_presentation(&mut self) -> Result<(), String> {
        if let Some(bridge) = &self.gpu_bridge {
            self.gpu_texture.poll(bridge)?;
        }
        Ok(())
    }
    fn visible_camera(&self) -> Camera {
        self.displayed_camera.unwrap_or(self.camera)
    }
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
                ui.label("Play · editing is available after Stop");
            }
        });
        let size = ui.available_size().max(Vec2::splat(1.));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        self.rect = Some(rect);
        // Never test newly edited geometry against a retained older image.
        // An already active gesture keeps using its captured camera/geometry.
        let image_current = self.render_revision.is_none()
            || self.displayed_key.is_some_and(|key| {
                key.revision == self.render_revision
                    && key.size == render_size(rect, ui.ctx().pixels_per_point())
            });
        self.drop_hint = None;
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let primary_pressed =
            ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary));
        // A press and movement can arrive in the same input frame. The latest
        // pointer position is for updating a gesture, not choosing its handle.
        let (primary_press, primary_release) = ui.input(|input| {
            let press = input
                .events
                .iter()
                .enumerate()
                .find_map(|(index, event)| match event {
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        ..
                    } => Some((index, *pos)),
                    _ => None,
                });
            let release_after = if self.gesture.is_some() {
                0
            } else {
                press.map_or(input.events.len(), |(index, _)| index + 1)
            };
            let release = input.events[release_after..]
                .iter()
                .find_map(|event| match event {
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        ..
                    } => Some(*pos),
                    _ => None,
                });
            (press.map(|(_, pos)| pos), release)
        });
        let primary_press = primary_press.filter(|pointer| scene_owns_press(&response, *pointer));
        if primary_press.is_some()
            || (response.contains_pointer()
                && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Middle)))
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
        let geometry = (!locked && image_current)
            .then(|| {
                selected.and_then(|id| {
                    GizmoGeometry::build(
                        objects,
                        id,
                        self.tool,
                        self.space,
                        self.visible_camera(),
                        rect,
                    )
                    .ok()
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
        let pressed_handle = primary_press
            .filter(|pointer| rect.contains(*pointer))
            .and_then(|pointer| {
                geometry
                    .as_ref()
                    .and_then(|geometry| geometry.hit_test(pointer))
                    .map(|axis| (axis, pointer))
            });
        if !locked
            && !handled_gesture
            && self.gesture.is_none()
            && !dragging_asset
            && primary_pressed
            && let (Some(id), Some((axis, pointer))) = (selected, pressed_handle)
        {
            match DragGesture::begin(
                objects,
                id,
                self.tool,
                self.space,
                axis,
                self.visible_camera(),
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
            // Stop at this gesture's first release, even if another press or
            // pointer movement follows it in the same input frame.
            if let Some(pointer) = primary_release.or(pointer) {
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
            if primary_release.is_some()
                || !ui.input(|input| input.pointer.button_down(egui::PointerButton::Primary))
            {
                self.gesture = None;
                actions.push(SceneAction::CommitGesture);
            }
            ui.ctx().request_repaint();
        }
        if !handled_gesture
            && image_current
            && response.clicked_by(egui::PointerButton::Primary)
            && !dragging_asset
            && let Some(pointer) = pointer
        {
            let id = scene_tools::camera_ray(self.visible_camera(), rect, pointer)
                .and_then(|ray| scene_tools::pick(snapshot, ray))
                .map(|hit| hit.id);
            actions.push(SceneAction::Select(id));
        }
        self.asset_drop(ui, &response, rect, snapshot, image_current, &mut actions);
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
        let revision = self.render_revision;
        self.paint_presented(
            ui,
            objects,
            snapshot,
            selected,
            &mut |camera, width, height| {
                render(camera, width, height).map(|frame| {
                    Some(PresentedSceneFrame {
                        frame,
                        camera,
                        revision,
                        pending: false,
                    })
                })
            },
        );
    }

    pub fn set_gpu_presentation(
        &mut self,
        bridge: Option<
            std::sync::Arc<module_engine_player::gpu_presentation::GpuPresentationBridge>,
        >,
    ) {
        self.gpu_bridge = bridge;
    }

    pub fn paint_presented(
        &mut self,
        ui: &mut egui::Ui,
        objects: &[WorldObject],
        snapshot: &SceneSnapshot,
        selected: Option<u64>,
        render: &mut dyn FnMut(Camera, usize, usize) -> Result<Option<PresentedSceneFrame>, String>,
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
        let [width, height] = render_size(rect, dpi);
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
        if self.presentation_pending || self.rendered_key != Some(key) || expired || untracked_frame
        {
            self.rendered_key = Some(key);
            self.rendered_at = Some(now);
            self.rendered_frame = Some(frame_number);
            self.render_count += 1;
            match render(self.camera, width, height) {
                Ok(Some(presented)) if valid_frame(&presented.frame) => {
                    let frame = &presented.frame;
                    self.presentation_pending = presented.pending;
                    match module_engine_player::ui::update_presented_texture(
                        ui,
                        &mut self.texture,
                        &mut self.gpu_texture,
                        self.gpu_bridge.as_deref(),
                        frame,
                    ) {
                        Ok(true) => {
                            self.displayed_camera = Some(presented.camera);
                            let completed_key = SceneRenderKey {
                                revision: presented.revision,
                                camera: presented.camera,
                                size: [frame.width, frame.height],
                            };
                            self.displayed_key = Some(completed_key);
                            self.rendered_key = Some(completed_key);
                            if presented.pending || completed_key != key {
                                ui.ctx().request_repaint();
                            }
                            if self
                                .render_failure
                                .as_ref()
                                .is_some_and(|error| self.error.as_ref() == Some(error))
                            {
                                self.error = None;
                            }
                            self.render_failure = None;
                        }
                        Ok(false) => {
                            self.rendered_key = None;
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(1));
                        }
                        Err(error) => {
                            self.render_failure = Some(error.clone());
                            self.error = Some(error);
                        }
                    }
                }
                Ok(None) => {
                    self.presentation_pending = true;
                    self.rendered_key = None;
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(1));
                }
                Ok(Some(_)) => {
                    self.presentation_pending = false;
                    let error = "Scene renderer returned malformed frame pixels".to_owned();
                    self.render_failure = Some(error.clone());
                    self.error = Some(error);
                }
                Err(error) => {
                    self.presentation_pending = false;
                    self.render_failure = Some(error.clone());
                    self.error = Some(error);
                }
            }
        }
        if self.render_revision.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        if let Some(texture) = self.texture_id() {
            painter.image(
                texture,
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                Color32::WHITE,
            );
        }
        let image_current = self.render_revision.is_none()
            || self.displayed_key.is_some_and(|displayed| {
                displayed.revision == self.render_revision && displayed.size == [width, height]
            });
        if image_current && let Some(id) = selected {
            if let Some(bounds) = object_bounds(snapshot, id) {
                draw_bounds(&painter, self.visible_camera(), rect, bounds);
            }
            if !self.locked
                && let Ok(geometry) = GizmoGeometry::build(
                    objects,
                    id,
                    self.tool,
                    self.space,
                    self.visible_camera(),
                    rect,
                )
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
            "Esc: cancel · release LMB: apply"
        } else {
            "LMB: select · G/R/S: tool · MMB: orbit · Shift+MMB: pan · wheel: zoom · F: focus"
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
        image_current: bool,
        actions: &mut Vec<SceneAction>,
    ) {
        let locked = self.locked;
        if let Some(payload) = response.dnd_hover_payload::<AssetDrag>() {
            let pointer = ui.input(|input| input.pointer.interact_pos());
            let ray = pointer
                .and_then(|pointer| scene_tools::camera_ray(self.visible_camera(), rect, pointer));
            let (hint, allowed) = if locked {
                ("Editing is available after Stop", false)
            } else if !image_current {
                ("Scene image is updating", false)
            } else {
                match payload.kind {
                    AssetKind::Model => (
                        "Drop to create an object with the model",
                        ray.and_then(|ray| scene_tools::placement(snapshot, ray))
                            .is_some(),
                    ),
                    AssetKind::Texture => (
                        "Drop onto an object to assign the texture",
                        ray.and_then(|ray| scene_tools::pick(snapshot, ray))
                            .is_some(),
                    ),
                    _ => ("This asset cannot be assigned through Scene", false),
                }
            };
            self.drop_hint = Some((hint.into(), allowed));
        }
        if let Some(payload) = response.dnd_release_payload::<AssetDrag>() {
            if locked || !image_current {
                return;
            }
            let Some(ray) = ui
                .input(|input| input.pointer.interact_pos())
                .and_then(|pointer| scene_tools::camera_ray(self.visible_camera(), rect, pointer))
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
                            "No surface or ground under the cursor to place the model".into(),
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
                            "Drag the texture onto an existing object".into(),
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
    frame.is_valid()
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
        frame_with_overlay(ctx, state, events, selected, locked, time, None)
    }
    #[derive(Clone, Copy)]
    enum SceneOverlay {
        Foreground,
        SameLayer,
    }
    fn frame_with_overlay(
        ctx: &egui::Context,
        state: &mut SceneViewportState,
        events: Vec<egui::Event>,
        selected: Option<u64>,
        locked: bool,
        time: f64,
        overlay: Option<(SceneOverlay, Rect)>,
    ) -> Vec<SceneAction> {
        let mut actions = vec![];
        let output = ctx.run_ui(input(time, events), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                actions = state.interact(ui, &objects(), &snapshot(), selected, locked);
                if let Some((SceneOverlay::SameLayer, rect)) = overlay {
                    ui.interact(
                        rect,
                        egui::Id::new("scene-press-overlay"),
                        Sense::click_and_drag(),
                    );
                }
            });
            if let Some((SceneOverlay::Foreground, rect)) = overlay {
                egui::Area::new(egui::Id::new("scene-press-overlay-area"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(rect.min)
                    .movable(false)
                    .show(ui.ctx(), |ui| {
                        ui.allocate_exact_size(rect.size(), Sense::click_and_drag());
                    });
            }
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
    #[derive(Clone, Copy)]
    enum DragInputBatching {
        SplitFrames,
        PressAndMove,
        CompleteGesture,
        PressAway,
        ReleaseBeforePress,
        MoveAfterRelease,
        MoveOutsideViewport,
    }
    fn assert_handle_drag_event_batching(batching: DragInputBatching) {
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
        let mut end = handle + (handle - geometry.center).normalized() * 40.;
        if matches!(batching, DragInputBatching::MoveOutsideViewport) {
            end.x = state.rect.unwrap().right() + 2.;
            assert!(input(0., vec![]).screen_rect.unwrap().contains(end));
            assert!(!state.rect.unwrap().contains(end));
        } else {
            assert!(state.rect.unwrap().contains(end));
        }
        assert_eq!(geometry.hit_test(handle), Some(Axis::X));
        assert_eq!(geometry.hit_test(end), None);

        let mut events = vec![egui::Event::PointerMoved(handle), button(handle, true)];
        if matches!(batching, DragInputBatching::ReleaseBeforePress) {
            let preceding = frame(
                &ctx,
                &mut state,
                vec![egui::Event::PointerMoved(end), button(end, true)],
                Some(7),
                false,
                0.05,
            );
            assert!(!preceding.contains(&SceneAction::BeginGesture { id: 7 }));
            events.insert(0, button(end, false));
        }
        if matches!(batching, DragInputBatching::PressAway) {
            let actions = frame(
                &ctx,
                &mut state,
                vec![
                    egui::Event::PointerMoved(end),
                    button(end, true),
                    egui::Event::PointerMoved(handle),
                ],
                Some(7),
                false,
                0.1,
            );
            assert!(!actions.iter().any(|action| matches!(
                action,
                SceneAction::BeginGesture { .. } | SceneAction::SetTransform { .. }
            )));
            assert!(state.gesture.is_none());
            let released = frame(
                &ctx,
                &mut state,
                vec![button(handle, false)],
                Some(7),
                false,
                0.2,
            );
            assert!(!released.contains(&SceneAction::CommitGesture));
            return;
        }
        let mut actions = vec![];
        if matches!(batching, DragInputBatching::SplitFrames) {
            actions.extend(frame(&ctx, &mut state, events, Some(7), false, 0.1));
            events = vec![];
        }
        events.push(egui::Event::PointerMoved(end));
        let released_in_frame = matches!(
            batching,
            DragInputBatching::CompleteGesture | DragInputBatching::MoveAfterRelease
        );
        if released_in_frame {
            events.push(button(end, false));
        }
        if matches!(batching, DragInputBatching::MoveAfterRelease) {
            events.push(egui::Event::PointerMoved(handle));
        }
        actions.extend(frame(&ctx, &mut state, events, Some(7), false, 0.2));
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, SceneAction::BeginGesture { id: 7 }))
                .count(),
            1,
            "The press position must start the gesture even when movement shares its input frame"
        );
        assert!(
            actions.iter().any(|action| matches!(action,
                SceneAction::SetTransform { id: 7, transform }
                    if transform.translation != Transform::default().translation
            )),
            "The gesture must preserve displacement from the actual press position"
        );
        if !released_in_frame {
            assert!(state.gesture.is_some());
            assert!(!actions.contains(&SceneAction::CommitGesture));
            actions.extend(frame(
                &ctx,
                &mut state,
                vec![button(end, false)],
                Some(7),
                false,
                0.3,
            ));
        }
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, SceneAction::CommitGesture))
                .count(),
            1
        );
        assert!(state.gesture.is_none());
        assert!(
            !frame(&ctx, &mut state, vec![], Some(7), false, 0.4)
                .contains(&SceneAction::CommitGesture)
        );
    }
    #[test]
    fn handle_drag_event_batching_split_frames_positive_control() {
        assert_handle_drag_event_batching(DragInputBatching::SplitFrames);
    }
    #[test]
    fn handle_drag_event_batching_coalesced_press_move_preserves_origin() {
        assert_handle_drag_event_batching(DragInputBatching::PressAndMove);
    }
    #[test]
    fn handle_drag_event_batching_complete_gesture_preserves_origin_and_commit() {
        assert_handle_drag_event_batching(DragInputBatching::CompleteGesture);
    }
    #[test]
    fn handle_drag_event_batching_press_away_does_not_capture_handle() {
        assert_handle_drag_event_batching(DragInputBatching::PressAway);
    }
    #[test]
    fn handle_drag_event_batching_release_before_press_does_not_commit_new_gesture() {
        assert_handle_drag_event_batching(DragInputBatching::ReleaseBeforePress);
    }
    #[test]
    fn handle_drag_event_batching_move_after_release_preserves_completed_displacement() {
        assert_handle_drag_event_batching(DragInputBatching::MoveAfterRelease);
    }
    #[test]
    fn handle_drag_event_batching_move_outside_viewport_preserves_origin_ownership() {
        assert_handle_drag_event_batching(DragInputBatching::MoveOutsideViewport);
    }
    fn assert_overlay_press_does_not_start_scene_gesture(overlay: SceneOverlay) {
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
        let end = handle + (handle - geometry.center).normalized() * 40.;
        let overlay_rect = Rect::from_center_size(handle, Vec2::splat(24.));
        assert_eq!(geometry.hit_test(handle), Some(Axis::X));
        assert!(state.rect.unwrap().contains(end));
        assert!(!overlay_rect.contains(end));
        for time in [0.05, 0.1] {
            frame_with_overlay(
                &ctx,
                &mut state,
                vec![],
                Some(7),
                false,
                time,
                Some((overlay, overlay_rect)),
            );
        }
        match overlay {
            SceneOverlay::Foreground => {
                assert_eq!(
                    ctx.layer_id_at(handle).unwrap().order,
                    egui::Order::Foreground
                );
                assert_eq!(ctx.layer_id_at(end).unwrap().order, egui::Order::Background);
            }
            SceneOverlay::SameLayer => {
                let response = ctx
                    .read_response(egui::Id::new("scene-press-overlay"))
                    .unwrap();
                assert!(response.enabled());
                assert!(response.sense.interactive());
                assert!(response.interact_rect.contains(handle));
                assert!(!response.interact_rect.contains(end));
                assert_eq!(ctx.layer_id_at(handle), Some(response.layer_id));
            }
        }
        let actions = frame_with_overlay(
            &ctx,
            &mut state,
            vec![
                egui::Event::PointerMoved(handle),
                button(handle, true),
                egui::Event::PointerMoved(end),
            ],
            Some(7),
            false,
            0.2,
            Some((overlay, overlay_rect)),
        );
        assert!(!actions.iter().any(|action| matches!(
            action,
            SceneAction::BeginGesture { .. }
                | SceneAction::SetTransform { .. }
                | SceneAction::CommitGesture
        )));
        assert!(state.gesture.is_none());
        let released = frame_with_overlay(
            &ctx,
            &mut state,
            vec![button(end, false)],
            Some(7),
            false,
            0.3,
            Some((overlay, overlay_rect)),
        );
        assert!(!released.contains(&SceneAction::CommitGesture));
    }
    #[test]
    fn handle_drag_event_batching_foreground_overlay_owns_press_origin() {
        assert_overlay_press_does_not_start_scene_gesture(SceneOverlay::Foreground);
    }
    #[test]
    fn handle_drag_event_batching_same_layer_overlay_owns_press_origin() {
        assert_overlay_press_does_not_start_scene_gesture(SceneOverlay::SameLayer);
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
                            gpu: None,
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
    fn saturated_presentation_keeps_visible_image_camera_and_retries_without_revision_change() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState {
            rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(160., 90.))),
            ..Default::default()
        };
        let objects = objects();
        let snapshot = snapshot();
        let draw = |state: &mut SceneViewportState, time: f64, busy: bool| {
            ctx.run_ui(input(time, vec![]), |ui| {
                state.paint_presented(
                    ui,
                    &objects,
                    &snapshot,
                    None,
                    &mut |camera, width, height| {
                        Ok((!busy).then(|| PresentedSceneFrame {
                            frame: Frame {
                                width,
                                height,
                                rgba: vec![128; width * height * 4],
                                gpu: None,
                                stats: RenderStats::default(),
                            },
                            camera,
                            revision: Some(1),
                            pending: false,
                        }))
                    },
                );
            })
        };
        state.set_render_revision(1);
        draw(&mut state, 0., false).drop_without_applying_deltas();
        let original_camera = state.visible_camera();
        let texture = state.texture.as_ref().unwrap().id();
        state.camera.yaw += 0.5;
        let requested_camera = state.camera;
        let output = draw(&mut state, 0.01, true);
        assert_eq!(state.visible_camera(), original_camera);
        assert_eq!(state.texture.as_ref().unwrap().id(), texture);
        assert!(state.error.is_none());
        let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
        assert!(primitives.iter().any(|shape| matches!(&shape.primitive,
            egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == texture)));
        output.drop_without_applying_deltas();
        let attempts = state.render_count();
        // Same authored revision/camera, well before the 250 ms idle refresh.
        draw(&mut state, 0.02, false).drop_without_applying_deltas();
        assert_eq!(state.render_count(), attempts + 1);
        assert_eq!(state.visible_camera(), requested_camera);
    }

    #[test]
    fn delayed_completion_keeps_its_camera_and_key_and_polls_successor_before_idle_refresh() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState {
            rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(160., 90.))),
            ..Default::default()
        };
        let authored = objects();
        let snapshot = snapshot();
        let first = state.camera;
        let second = Camera {
            yaw: first.yaw + 0.3,
            ..first
        };
        let third = Camera {
            yaw: first.yaw + 0.6,
            ..first
        };
        let draw = |state: &mut SceneViewportState, time, completed: Option<Camera>, pending| {
            ctx.run_ui(input(time, vec![]), |ui| {
                state.paint_presented(
                    ui,
                    &authored,
                    &snapshot,
                    None,
                    &mut |_requested, width, height| {
                        Ok(completed.map(|camera| PresentedSceneFrame {
                            frame: Frame {
                                width,
                                height,
                                rgba: vec![128; width * height * 4],
                                gpu: None,
                                stats: RenderStats::default(),
                            },
                            camera,
                            revision: Some(1),
                            pending,
                        }))
                    },
                );
            })
            .drop_without_applying_deltas();
        };
        state.set_render_revision(1);
        draw(&mut state, 0., Some(first), false);
        state.camera = second;
        draw(&mut state, 0.01, None, true);
        assert_eq!(state.visible_camera(), first);
        state.camera = third;
        draw(&mut state, 0.02, Some(second), true);
        assert_eq!(state.visible_camera(), second);
        assert_eq!(state.rendered_key.unwrap().camera, second);
        assert!(state.presentation_pending);
        let attempts = state.render_count();
        draw(&mut state, 0.03, Some(third), false);
        assert_eq!(state.render_count(), attempts + 1);
        assert_eq!(state.visible_camera(), third);
        assert!(!state.presentation_pending);
        draw(&mut state, 0.04, None, true);
        assert_eq!(
            state.render_count(),
            attempts + 1,
            "A completed unchanged view returns to its static cache"
        );
    }

    #[test]
    fn retained_old_revision_or_size_cannot_pick_new_geometry() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState::default();
        frame(&ctx, &mut state, vec![], None, false, 0.);
        state.set_render_revision(2);
        let current_size = render_size(state.rect.unwrap(), ctx.pixels_per_point());
        let center =
            scene_tools::project_point(state.camera, state.rect.unwrap(), [0.; 3]).unwrap();
        let click = |state: &mut SceneViewportState, time| {
            frame(
                &ctx,
                state,
                vec![egui::Event::PointerMoved(center), button(center, true)],
                None,
                false,
                time,
            );
            frame(
                &ctx,
                state,
                vec![button(center, false)],
                None,
                false,
                time + 0.01,
            )
        };
        state.displayed_key = Some(SceneRenderKey {
            revision: Some(1),
            camera: state.camera,
            size: current_size,
        });
        let actions = click(&mut state, 0.1);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, SceneAction::Select(_)))
        );
        state.displayed_key = Some(SceneRenderKey {
            revision: Some(2),
            camera: state.camera,
            size: [1, 1],
        });
        let actions = click(&mut state, 0.2);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, SceneAction::Select(_)))
        );
        state.displayed_key = Some(SceneRenderKey {
            revision: Some(2),
            camera: state.camera,
            size: current_size,
        });
        assert!(click(&mut state, 0.3).contains(&SceneAction::Select(Some(7))));
    }

    #[test]
    fn asset_drop_release_uses_the_visible_camera_while_a_new_camera_is_pending() {
        let ctx = egui::Context::default();
        let mut state = SceneViewportState::default();
        frame(&ctx, &mut state, vec![], None, false, 0.);
        let visible = state.camera;
        state.displayed_camera = Some(visible);
        state.camera.target = [100., 0., 0.];
        let center = scene_tools::project_point(visible, state.rect.unwrap(), [0.; 3]).unwrap();
        assert!(
            scene_tools::camera_ray(state.camera, state.rect.unwrap(), center)
                .and_then(|ray| scene_tools::pick(&snapshot(), ray))
                .is_none()
        );
        frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(center), button(center, true)],
            None,
            false,
            0.1,
        );
        egui::DragAndDrop::set_payload(
            &ctx,
            AssetDrag {
                id: "visible-texture".into(),
                reference: "asset://visible-texture".into(),
                path: "assets/visible.png".into(),
                kind: AssetKind::Texture,
            },
        );
        let actions = frame(
            &ctx,
            &mut state,
            vec![button(center, false)],
            None,
            false,
            0.2,
        );
        assert!(actions.contains(&SceneAction::DropTexture {
            asset_ref: "asset://visible-texture".into(),
            id: 7,
        }));
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
                            gpu: None,
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
