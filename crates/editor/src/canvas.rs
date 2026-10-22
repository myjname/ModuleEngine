//! Shared visual canvas for module graphs and parsed Rust expression graphs.

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::BTreeMap;

pub const NODE_WIDTH: f32 = 190.0;
const HEADER: f32 = 34.0;
const ROW: f32 = 25.0;

#[derive(Clone)]
pub struct CanvasPort {
    pub name: String,
    pub data_type: String,
}

#[derive(Clone)]
pub struct CanvasNode {
    pub id: String,
    pub label: String,
    pub subtitle: String,
    pub position: [f32; 2],
    pub inputs: Vec<CanvasPort>,
    pub outputs: Vec<CanvasPort>,
    pub expandable: bool,
    pub opaque: bool,
}

#[derive(Clone)]
pub struct CanvasEdge {
    pub from_node: String,
    pub from_port: String,
    pub to_node: String,
    pub to_port: String,
}

#[derive(Debug)]
pub enum CanvasAction {
    Select(String),
    Move(String, [f32; 2]),
    Open(String),
    Connect {
        from_node: String,
        from_port: String,
        to_node: String,
        to_port: String,
    },
    DeleteEdge(usize),
}

#[derive(Default)]
pub struct CanvasLayout {
    pub rect: Option<Rect>,
    pub headers: BTreeMap<String, Rect>,
    pub inputs: BTreeMap<(String, String), (Pos2, String)>,
    pub outputs: BTreeMap<(String, String), (Pos2, String)>,
}

pub struct CanvasState {
    pub pan: Vec2,
    pub zoom: f32,
    pub selected: Option<String>,
    pub pending: Option<(String, String)>,
    pub layout: CanvasLayout,
    pub message: Option<String>,
    id: &'static str,
}

impl CanvasState {
    pub fn new(id: &'static str) -> Self {
        Self {
            pan: Vec2::new(28.0, 35.0),
            zoom: 1.0,
            selected: None,
            pending: None,
            layout: CanvasLayout::default(),
            message: None,
            id,
        }
    }
    pub fn home(&mut self) {
        self.pan = Vec2::new(28.0, 35.0);
        self.zoom = 1.0;
        self.pending = None;
        self.message = None;
    }
}

pub fn type_color(kind: &str) -> Color32 {
    match kind {
        "World" => Color32::from_rgb(176, 135, 249),
        "Scene3D" => Color32::from_rgb(61, 206, 183),
        "Frame3D" => Color32::from_rgb(245, 170, 88),
        "Scalar" | "f64" => Color32::from_rgb(99, 174, 252),
        "GpuColor" => Color32::from_rgb(246, 159, 79),
        "GpuNormals" => Color32::from_rgb(131, 203, 248),
        "GpuDepth" => Color32::from_rgb(196, 206, 220),
        "WorldCommands" => Color32::from_rgb(199, 144, 238),
        "RenderCommands" => Color32::from_rgb(237, 182, 96),
        "f32" | "u8" | "ObjectId" => Color32::from_rgb(99, 174, 252),
        "String" => Color32::from_rgb(127, 212, 155),
        "bool" => Color32::from_rgb(225, 130, 157),
        _ => Color32::from_rgb(163, 172, 193),
    }
}

fn compact(text: &str, len: usize) -> String {
    let text = text.lines().next().unwrap_or(text);
    if text.chars().count() > len {
        format!("{}…", text.chars().take(len).collect::<String>())
    } else {
        text.to_owned()
    }
}

fn curve(a: Pos2, b: Pos2) -> Vec<Pos2> {
    let bend = ((b.x - a.x).abs() * 0.48).clamp(40.0, 140.0);
    let c = a + Vec2::new(bend, 0.0);
    let d = b - Vec2::new(bend, 0.0);
    (0..=28)
        .map(|i| {
            let t = i as f32 / 28.0;
            let u = 1.0 - t;
            Pos2::new(
                u * u * u * a.x + 3.0 * u * u * t * c.x + 3.0 * u * t * t * d.x + t * t * t * b.x,
                u * u * u * a.y + 3.0 * u * u * t * c.y + 3.0 * u * t * t * d.y + t * t * t * b.y,
            )
        })
        .collect()
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let line = b - a;
    let t = if line.length_sq() > 0.0 {
        ((p - a).dot(line) / line.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + line * t)
}

/// Draw one canvas and return edits. Applying edits belongs to the model, so UI
/// connection gestures cannot bypass the graph's validation rules.
pub fn draw(
    ui: &mut egui::Ui,
    state: &mut CanvasState,
    nodes: &[CanvasNode],
    edges: &[CanvasEdge],
) -> Vec<CanvasAction> {
    let mut actions = Vec::new();
    let size = ui.available_size().max(Vec2::new(100.0, 100.0));
    let (canvas, background) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(canvas);
    painter.rect_filled(canvas, 8.0, Color32::from_rgb(16, 21, 31));

    if background.dragged_by(egui::PointerButton::Middle)
        || background.dragged_by(egui::PointerButton::Secondary)
    {
        state.pan += background.drag_delta();
    }
    // egui routes Ctrl/Cmd+wheel to zoom_delta, not smooth_scroll_delta.
    // Test the canvas region so nodes and pins do not steal its zoom gesture.
    if ui.is_enabled()
        && ui.rect_contains_pointer(canvas)
        && let Some(pointer) = ui.input(|input| input.pointer.hover_pos())
    {
        let factor = ui.input(|input| input.zoom_delta());
        if factor.is_finite() && factor > 0.0 && factor != 1.0 {
            let zoom = (state.zoom * factor).clamp(0.45, 1.65);
            if zoom != state.zoom {
                let anchor = pointer - canvas.min;
                state.pan = anchor - (anchor - state.pan) * (zoom / state.zoom);
                state.zoom = zoom;
            }
        }
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.pending = None;
    }

    let grid = 24.0 * state.zoom;
    let first_x = canvas.left() + state.pan.x.rem_euclid(grid);
    let first_y = canvas.top() + state.pan.y.rem_euclid(grid);
    // Batch the tiny grid marks. Thousands of separate circle shapes used to
    // be rebuilt/tessellated for every pointer event when zoomed out.
    let mut grid_mesh = egui::Mesh::default();
    let mut x = first_x;
    while x < canvas.right() {
        let mut y = first_y;
        while y < canvas.bottom() {
            grid_mesh.add_colored_rect(
                Rect::from_center_size(Pos2::new(x, y), Vec2::splat(1.6)),
                Color32::from_rgb(35, 42, 57),
            );
            y += grid;
        }
        x += grid;
    }
    painter.add(egui::Shape::mesh(grid_mesh));

    let origin = canvas.min + state.pan;
    state.layout = CanvasLayout {
        rect: Some(canvas),
        ..CanvasLayout::default()
    };
    let mut rects = BTreeMap::new();
    for node in nodes {
        let p = origin + Vec2::new(node.position[0], node.position[1]) * state.zoom;
        let rows = node.inputs.len().max(node.outputs.len()).max(1) as f32;
        let rect = Rect::from_min_size(
            p,
            Vec2::new(NODE_WIDTH, HEADER + rows * ROW + 28.0) * state.zoom,
        );
        state.layout.headers.insert(
            node.id.clone(),
            Rect::from_min_size(p, Vec2::new(NODE_WIDTH, HEADER) * state.zoom),
        );
        for (i, port) in node.inputs.iter().enumerate() {
            state.layout.inputs.insert(
                (node.id.clone(), port.name.clone()),
                (
                    Pos2::new(
                        rect.left(),
                        p.y + (HEADER + 17.0 + i as f32 * ROW) * state.zoom,
                    ),
                    port.data_type.clone(),
                ),
            );
        }
        for (i, port) in node.outputs.iter().enumerate() {
            state.layout.outputs.insert(
                (node.id.clone(), port.name.clone()),
                (
                    Pos2::new(
                        rect.right(),
                        p.y + (HEADER + 17.0 + i as f32 * ROW) * state.zoom,
                    ),
                    port.data_type.clone(),
                ),
            );
        }
        rects.insert(node.id.clone(), rect);
    }

    for (index, edge) in edges.iter().enumerate() {
        if let (Some((a, kind)), Some((b, _))) = (
            state
                .layout
                .outputs
                .get(&(edge.from_node.clone(), edge.from_port.clone())),
            state
                .layout
                .inputs
                .get(&(edge.to_node.clone(), edge.to_port.clone())),
        ) {
            let points = curve(*a, *b);
            let hot = background.hovered()
                && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
                    points
                        .windows(2)
                        .any(|w| distance_to_segment(p, w[0], w[1]) < 6.0)
                });
            painter.add(egui::Shape::line(
                points,
                Stroke::new(if hot { 3.8 } else { 2.2 }, type_color(kind)),
            ));
            if hot && background.double_clicked() {
                actions.push(CanvasAction::DeleteEdge(index));
            }
        }
    }

    let mut connect_target = None;
    let mut clicked_output = false;
    for node in nodes {
        let rect = rects[&node.id];
        if !canvas.intersects(rect) {
            continue;
        }
        let accent = if node.opaque {
            Color32::from_rgb(236, 171, 95)
        } else {
            node.outputs
                .first()
                .map(|p| type_color(&p.data_type))
                .unwrap_or(Color32::from_rgb(118, 137, 167))
        };
        painter.rect_filled(
            rect,
            7.0,
            if state.selected.as_deref() == Some(&node.id) {
                Color32::from_rgb(38, 49, 69)
            } else {
                Color32::from_rgb(27, 34, 48)
            },
        );
        painter.rect_stroke(
            rect,
            7.0,
            Stroke::new(
                if state.selected.as_deref() == Some(&node.id) {
                    1.7
                } else {
                    1.0
                },
                if state.selected.as_deref() == Some(&node.id) {
                    accent
                } else {
                    Color32::from_rgb(61, 73, 96)
                },
            ),
            egui::StrokeKind::Inside,
        );
        painter.line_segment(
            [
                rect.left_top() + Vec2::new(8.0, 2.0),
                rect.right_top() - Vec2::new(8.0, -2.0),
            ],
            Stroke::new(2.0, accent),
        );
        let header = state.layout.headers[&node.id];
        let response = ui.interact(
            header,
            ui.make_persistent_id((state.id, &node.id, "header")),
            Sense::click_and_drag(),
        );
        if response.clicked() || response.drag_started() {
            state.selected = Some(node.id.clone());
            actions.push(CanvasAction::Select(node.id.clone()));
        }
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = response.drag_delta() / state.zoom;
            actions.push(CanvasAction::Move(
                node.id.clone(),
                [node.position[0] + delta.x, node.position[1] + delta.y],
            ));
        }
        if response.double_clicked() && node.expandable {
            actions.push(CanvasAction::Open(node.id.clone()));
        }
        response.on_hover_text(if node.expandable {
            "Drag the header. Double-click to open the internal graph."
        } else {
            "Drag the header. The selected node's parameters are on the right."
        });
        painter.text(
            header.left_top() + Vec2::new(11.0, 10.0) * state.zoom,
            egui::Align2::LEFT_TOP,
            compact(&node.label, 24),
            FontId::proportional(13.0 * state.zoom),
            Color32::from_rgb(232, 237, 247),
        );
        painter.text(
            rect.left_bottom() + Vec2::new(10.0, -13.0) * state.zoom,
            egui::Align2::LEFT_CENTER,
            compact(&node.subtitle, 29),
            FontId::monospace(9.5 * state.zoom),
            Color32::from_rgb(127, 143, 167),
        );

        for port in &node.inputs {
            let (p, kind) = state.layout.inputs[&(node.id.clone(), port.name.clone())].clone();
            let response = ui.interact(
                Rect::from_center_size(p, Vec2::splat(24.0)),
                ui.make_persistent_id((state.id, &node.id, &port.name, "input")),
                Sense::click(),
            );
            let ready = state
                .pending
                .as_ref()
                .and_then(|from| state.layout.outputs.get(from))
                .is_some_and(|(_, out_kind)| out_kind == &kind);
            painter.circle_filled(p, if ready { 5.7 } else { 4.5 }, type_color(&kind));
            painter.text(
                p + Vec2::new(12.0, 0.0) * state.zoom,
                egui::Align2::LEFT_CENTER,
                compact(&port.name, 12),
                FontId::monospace(10.0 * state.zoom),
                Color32::from_rgb(179, 190, 207),
            );
            if response.clicked()
                || (response.hovered()
                    && ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary)))
            {
                connect_target = Some((node.id.clone(), port.name.clone(), kind.clone()));
            }
            response.on_hover_text(format!("{}: {}", port.name, kind));
        }
        for port in &node.outputs {
            let (p, kind) = state.layout.outputs[&(node.id.clone(), port.name.clone())].clone();
            let response = ui.interact(
                Rect::from_center_size(p, Vec2::splat(24.0)),
                ui.make_persistent_id((state.id, &node.id, &port.name, "output")),
                Sense::click_and_drag(),
            );
            painter.circle_filled(p, 4.5, type_color(&kind));
            painter.text(
                p - Vec2::new(12.0, 0.0) * state.zoom,
                egui::Align2::RIGHT_CENTER,
                compact(&port.name, 12),
                FontId::monospace(10.0 * state.zoom),
                Color32::from_rgb(179, 190, 207),
            );
            if response.clicked() || response.drag_started() {
                state.pending = Some((node.id.clone(), port.name.clone()));
                clicked_output = true;
            }
            response.on_hover_text(format!(
                "{}: {} — select an input of the same type",
                port.name, kind
            ));
        }
    }

    if let Some((to_node, to_port, kind)) = connect_target
        && let Some((from_node, from_port)) = state.pending.clone()
    {
        let from_kind = state
            .layout
            .outputs
            .get(&(from_node.clone(), from_port.clone()))
            .map(|(_, k)| k);
        if from_kind == Some(&kind) && from_node != to_node {
            actions.push(CanvasAction::Connect {
                from_node,
                from_port,
                to_node,
                to_port,
            });
            state.pending = None;
            state.message = None;
        } else {
            state.message =
                Some("Connection rejected: pin types must match and nodes must differ.".into());
        }
    }
    if let Some(from) = &state.pending
        && let (Some((a, kind)), Some(b)) = (
            state.layout.outputs.get(from),
            ui.input(|i| i.pointer.hover_pos()),
        )
    {
        painter.add(egui::Shape::line(
            curve(*a, b),
            Stroke::new(2.0, type_color(kind)),
        ));
    }
    if background.clicked() && !clicked_output && state.pending.is_none() {
        state.selected = None;
    }
    painter.text(
        canvas.left_bottom() + Vec2::new(14.0, -14.0),
        egui::Align2::LEFT_BOTTOM,
        "Connect: output to input · double-click a connection to delete it · RMB: pan · Ctrl+wheel: zoom",
        FontId::proportional(10.5),
        Color32::from_rgb(111, 126, 149),
    );
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoomed_out_grid_is_batched_in_one_mesh_instead_of_thousands_of_shapes() {
        let ctx = egui::Context::default();
        let mut state = CanvasState::new("grid-batch-test");
        state.zoom = 0.45;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600., 900.))),
                ..Default::default()
            },
            |ui| {
                assert!(draw(ui, &mut state, &[], &[]).is_empty());
            },
        );
        assert!(
            output.shapes.len() < 20,
            "Background dots must not create per-dot shapes"
        );
        let dense_meshes = output.shapes.iter().filter(|shape| {
            matches!(&shape.shape, egui::Shape::Mesh(mesh) if mesh.vertices.len() > 1000)
        }).count();
        assert_eq!(dense_meshes, 1);
        output.drop_without_applying_deltas();
    }
    fn nodes() -> Vec<CanvasNode> {
        vec![
            CanvasNode {
                id: "a".into(),
                label: "Input".into(),
                subtitle: "".into(),
                position: [20., 40.],
                inputs: vec![],
                outputs: vec![CanvasPort {
                    name: "value".into(),
                    data_type: "Scalar".into(),
                }],
                expandable: false,
                opaque: false,
            },
            CanvasNode {
                id: "b".into(),
                label: "Output".into(),
                subtitle: "".into(),
                position: [290., 40.],
                inputs: vec![CanvasPort {
                    name: "base".into(),
                    data_type: "Scalar".into(),
                }],
                outputs: vec![],
                expandable: false,
                opaque: false,
            },
        ]
    }
    fn frame(
        ctx: &egui::Context,
        state: &mut CanvasState,
        nodes: &[CanvasNode],
        events: Vec<egui::Event>,
    ) -> Vec<CanvasAction> {
        frame_enabled(ctx, state, nodes, events, true)
    }
    fn frame_enabled(
        ctx: &egui::Context,
        state: &mut CanvasState,
        nodes: &[CanvasNode],
        events: Vec<egui::Event>,
        enabled: bool,
    ) -> Vec<CanvasAction> {
        let mut actions = vec![];
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 400.))),
                events,
                ..Default::default()
            },
            |ui| {
                if !enabled {
                    ui.disable();
                }
                actions = draw(ui, state, nodes, &[]);
            },
        );
        output.drop_without_applying_deltas();
        actions
    }
    fn click(
        ctx: &egui::Context,
        state: &mut CanvasState,
        nodes: &[CanvasNode],
        p: Pos2,
    ) -> Vec<CanvasAction> {
        frame(
            ctx,
            state,
            nodes,
            vec![
                egui::Event::PointerMoved(p),
                egui::Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        frame(
            ctx,
            state,
            nodes,
            vec![egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        )
    }
    #[test]
    fn ctrl_wheel_zooms_in_and_out_around_pointer_over_background_and_nodes() {
        for target in ["background", "header", "port"] {
            for direction in [1.0, -1.0] {
                let ctx = egui::Context::default();
                let mut state = CanvasState::new("wheel-zoom");
                let ns = nodes();
                frame(&ctx, &mut state, &ns, vec![]);
                let pointer = match target {
                    "header" => state.layout.headers["a"].center(),
                    "port" => state.layout.outputs[&("a".into(), "value".into())].0,
                    _ => Pos2::new(650.0, 300.0),
                };
                frame(
                    &ctx,
                    &mut state,
                    &ns,
                    vec![egui::Event::PointerMoved(pointer)],
                );
                let before_zoom = state.zoom;
                let before_anchor = (pointer - state.layout.headers["a"].min) / state.zoom;
                let modifiers = egui::Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                };
                let actions = frame(
                    &ctx,
                    &mut state,
                    &ns,
                    vec![
                        egui::Event::ModifiersChanged(modifiers),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Line,
                            delta: Vec2::new(0.0, direction * 3.0),
                            phase: egui::TouchPhase::Move,
                            modifiers,
                        },
                    ],
                );
                assert_eq!(ctx.input(|input| input.smooth_scroll_delta()), Vec2::ZERO);
                assert!(
                    (state.zoom - before_zoom) * direction > 0.0,
                    "Ctrl+wheel must zoom {direction} over {target}"
                );
                let after_anchor = (pointer - state.layout.headers["a"].min) / state.zoom;
                assert!((before_anchor - after_anchor).length() < 0.001);
                assert!(actions.is_empty(), "zoom must not modify the graph");
                assert_eq!(ctx.zoom_factor(), 1.0, "editor UI scale stays unchanged");
            }
        }
    }
    #[test]
    fn ordinary_wheel_and_zoom_outside_or_disabled_canvas_leave_view_unchanged() {
        for (pointer, enabled, modifiers) in [
            (Pos2::new(650.0, 300.0), true, egui::Modifiers::NONE),
            (
                Pos2::new(900.0, 300.0),
                true,
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            ),
            (
                Pos2::new(650.0, 300.0),
                false,
                egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            ),
        ] {
            let ctx = egui::Context::default();
            let mut state = CanvasState::new("wheel-focus");
            let ns = nodes();
            frame_enabled(&ctx, &mut state, &ns, vec![], enabled);
            frame_enabled(
                &ctx,
                &mut state,
                &ns,
                vec![egui::Event::PointerMoved(pointer)],
                enabled,
            );
            let before = (state.zoom, state.pan);
            frame_enabled(
                &ctx,
                &mut state,
                &ns,
                vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: Vec2::new(0.0, 3.0),
                        phase: egui::TouchPhase::Move,
                        modifiers,
                    },
                ],
                enabled,
            );
            assert_eq!((state.zoom, state.pan), before);
        }
    }
    #[test]
    fn wheel_zooms_only_the_graph_under_the_pointer() {
        let ctx = egui::Context::default();
        let mut left = CanvasState::new("left");
        let mut right = CanvasState::new("right");
        let ns = nodes();
        for events in [
            vec![],
            vec![egui::Event::PointerMoved(Pos2::new(650.0, 300.0))],
            vec![egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: Vec2::new(0.0, 3.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            }],
        ] {
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 400.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.columns(2, |columns| {
                        assert!(draw(&mut columns[0], &mut left, &ns, &[]).is_empty());
                        assert!(draw(&mut columns[1], &mut right, &ns, &[]).is_empty());
                    });
                },
            );
            output.drop_without_applying_deltas();
        }
        assert_eq!(left.zoom, 1.0);
        assert!(right.zoom > 1.0);
    }
    #[test]
    fn zoom_limits_keep_the_pointer_anchor_without_drift() {
        let ctx = egui::Context::default();
        let mut state = CanvasState::new("zoom-limits");
        let ns = nodes();
        let pointer = Pos2::new(650.0, 300.0);
        frame(
            &ctx,
            &mut state,
            &ns,
            vec![egui::Event::PointerMoved(pointer)],
        );
        let anchor = (pointer - state.layout.headers["a"].min) / state.zoom;
        for (factor, expected) in [(100.0, 1.65), (100.0, 1.65), (0.001, 0.45), (0.001, 0.45)] {
            frame(&ctx, &mut state, &ns, vec![egui::Event::Zoom(factor)]);
            assert_eq!(state.zoom, expected);
            let current = (pointer - state.layout.headers["a"].min) / state.zoom;
            assert!((anchor - current).length() < 0.001);
        }
    }
    #[test]
    fn pointer_gestures_connect_compatible_ports() {
        let ctx = egui::Context::default();
        let mut state = CanvasState::new("test");
        let ns = nodes();
        frame(&ctx, &mut state, &ns, vec![]);
        let out = state.layout.outputs[&("a".into(), "value".into())].0;
        let input = state.layout.inputs[&("b".into(), "base".into())].0;
        click(&ctx, &mut state, &ns, out);
        assert!(state.pending.is_some());
        let actions = click(&ctx, &mut state, &ns, input);
        assert!(actions.iter().any(|a|matches!(a,CanvasAction::Connect{from_node,to_node,..} if from_node=="a" && to_node=="b")));
    }
    #[test]
    fn pointer_gestures_reject_incompatible_types() {
        let ctx = egui::Context::default();
        let mut state = CanvasState::new("reject");
        let mut ns = nodes();
        ns[1].inputs[0].data_type = "Scene3D".into();
        frame(&ctx, &mut state, &ns, vec![]);
        let out = state.layout.outputs[&("a".into(), "value".into())].0;
        let input = state.layout.inputs[&("b".into(), "base".into())].0;
        click(&ctx, &mut state, &ns, out);
        let actions = click(&ctx, &mut state, &ns, input);
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, CanvasAction::Connect { .. }))
        );
        assert!(state.message.is_some());
    }

    #[test]
    fn header_drag_respects_zoom_and_double_click_opens_expandable_node() {
        let ctx = egui::Context::default();
        let mut state = CanvasState::new("header-gestures");
        state.zoom = 0.5;
        let mut ns = nodes();
        ns[0].expandable = true;
        frame(&ctx, &mut state, &ns, vec![]);
        let start = state.layout.headers["a"].center();
        frame(
            &ctx,
            &mut state,
            &ns,
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );

        // Apply each returned model edit, as Editor does between frames. This
        // also catches accidentally accumulating the full drag distance twice.
        for (pointer, expected) in [
            (start + Vec2::new(40.0, 20.0), [100.0, 80.0]),
            (start + Vec2::new(55.0, 15.0), [130.0, 70.0]),
        ] {
            let actions = frame(
                &ctx,
                &mut state,
                &ns,
                vec![egui::Event::PointerMoved(pointer)],
            );
            let position = actions
                .iter()
                .find_map(|action| match action {
                    CanvasAction::Move(id, position) if id == "a" => Some(*position),
                    _ => None,
                })
                .expect("dragging the header must emit a Move action");
            assert!((position[0] - expected[0]).abs() < 0.001);
            assert!((position[1] - expected[1]).abs() < 0.001);
            assert!(
                !actions
                    .iter()
                    .any(|action| matches!(action, CanvasAction::Open(_)))
            );
            ns[0].position = position;
        }
        assert_eq!(state.selected.as_deref(), Some("a"));
        frame(
            &ctx,
            &mut state,
            &ns,
            vec![egui::Event::PointerButton {
                pos: start + Vec2::new(55.0, 15.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );

        let header = state.layout.headers["a"].center();
        let first = click(&ctx, &mut state, &ns, header);
        assert!(
            !first
                .iter()
                .any(|action| matches!(action, CanvasAction::Open(_)))
        );
        let second = click(&ctx, &mut state, &ns, header);
        assert_eq!(
            second
                .iter()
                .filter(|action| matches!(action, CanvasAction::Open(id) if id == "a"))
                .count(),
            1
        );

        // The same gesture must not expand an ordinary leaf module.
        let leaf = state.layout.headers["b"].center();
        click(&ctx, &mut state, &ns, leaf);
        let actions = click(&ctx, &mut state, &ns, leaf);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, CanvasAction::Open(_)))
        );
    }
}
