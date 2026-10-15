//! Stable, serializable area trees. Area chrome owns no project/runtime state.
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceKind {
    Engine,
    Project,
}
impl WorkspaceKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Project => "project",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditorType {
    Viewport,
    Game,
    Scene,
    Hierarchy,
    Inspector,
    Assets,
    GameSystems,
    ObjectTree,
    ObjectInspector,
    ProjectSettings,
    EngineSettings,
    ModuleGraph,
    RustCustom,
    Hlsl,
    ProjectFile,
    History,
    ModuleLibrary,
    Diagnostics,
    Extension(u32),
}
impl EditorType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Viewport => "3D Viewport",
            Self::Game => "Game",
            Self::Scene => "Scene",
            Self::Hierarchy => "Hierarchy",
            Self::Inspector => "Inspector",
            Self::Assets => "Assets",
            Self::GameSystems => "Gameplay Systems",
            Self::ObjectTree => "Object Tree",
            Self::ObjectInspector => "Object / Components",
            Self::ProjectSettings => "Project Settings",
            Self::EngineSettings => "Engine Exposure",
            Self::ModuleGraph => "Module Blueprint",
            Self::RustCustom => "Custom / Rust",
            Self::Hlsl => "Custom / HLSL",
            Self::ProjectFile => "Project File",
            Self::History => "History / Merge",
            Self::ModuleLibrary => "Module Library",
            Self::Diagnostics => "Diagnostics",
            Self::Extension(_) => "Module Panel",
        }
    }
    pub fn requires_engine(self) -> bool {
        matches!(
            self,
            Self::Viewport
                | Self::ObjectTree
                | Self::ObjectInspector
                | Self::EngineSettings
                | Self::GameSystems
                | Self::ModuleGraph
                | Self::Hlsl
                | Self::ProjectFile
                | Self::ModuleLibrary
        )
    }
}
#[derive(Clone)]
pub struct EditorDescriptor {
    pub editor: EditorType,
    pub label: String,
    pub requires_engine: bool,
}
impl EditorDescriptor {
    pub fn builtins(engine: bool) -> Vec<Self> {
        [
            EditorType::Viewport,
            EditorType::Game,
            EditorType::Scene,
            EditorType::Hierarchy,
            EditorType::Inspector,
            EditorType::Assets,
            EditorType::GameSystems,
            EditorType::ObjectTree,
            EditorType::ObjectInspector,
            EditorType::ProjectSettings,
            EditorType::EngineSettings,
            EditorType::ModuleGraph,
            EditorType::RustCustom,
            EditorType::Hlsl,
            EditorType::ProjectFile,
            EditorType::History,
            EditorType::ModuleLibrary,
            EditorType::Diagnostics,
        ]
        .into_iter()
        .filter(|editor| engine || !editor.requires_engine())
        .map(|editor| Self {
            editor,
            label: editor.label().into(),
            requires_engine: editor.requires_engine(),
        })
        .collect()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    Horizontal,
    Vertical,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Area {
    Pane {
        id: u64,
        editor: EditorType,
    },
    Split {
        id: u64,
        axis: Axis,
        ratio: f32,
        first: Box<Area>,
        second: Box<Area>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockLayout {
    pub root: Area,
    pub next_id: u64,
}
#[derive(Clone)]
pub struct PaneRect {
    pub id: u64,
    pub editor: EditorType,
    /// Whole area, including its type selector and split/close controls.
    pub area_rect: Rect,
    pub rect: Rect,
}
#[derive(Default)]
pub struct DockResponse {
    pub panes: Vec<PaneRect>,
    pub activated_pane: Option<u64>,
    pub changed: bool,
    pub split_buttons: BTreeMap<(u64, &'static str), Rect>,
    pub choices: BTreeMap<u64, Rect>,
    pub options: BTreeMap<(u64, &'static str), Rect>,
    pub dividers: BTreeMap<u64, Rect>,
}
enum Action {
    Split(u64, Axis),
    Close(u64),
    Choose(u64, EditorType),
}
struct DrawOptions<'a> {
    available: &'a [EditorDescriptor],
    active_pane: Option<u64>,
    closable: bool,
}
impl DockLayout {
    pub fn single(editor: EditorType) -> Self {
        Self {
            root: Area::Pane { id: 1, editor },
            next_id: 2,
        }
    }
    pub fn default_for(workspace: WorkspaceKind) -> Self {
        if workspace == WorkspaceKind::Project {
            let mut layout = Self::single(EditorType::Scene);
            let hierarchy = layout.split(1, Axis::Horizontal).unwrap();
            layout.choose(hierarchy, EditorType::Hierarchy);
            let inspector = layout.split(hierarchy, Axis::Vertical).unwrap();
            layout.choose(inspector, EditorType::Inspector);
            let assets = layout.split(1, Axis::Vertical).unwrap();
            layout.choose(assets, EditorType::Assets);
            if let Area::Split {
                ratio,
                first,
                second,
                ..
            } = &mut layout.root
            {
                *ratio = 0.72;
                if let Area::Split { ratio, .. } = first.as_mut() {
                    *ratio = 0.60;
                }
                if let Area::Split { ratio, .. } = second.as_mut() {
                    *ratio = 0.36;
                }
            }
            return layout;
        }
        let mut layout = Self::single(if workspace == WorkspaceKind::Project {
            EditorType::Viewport
        } else {
            EditorType::ModuleGraph
        });
        layout.split(1, Axis::Horizontal).unwrap();
        layout.choose(
            3,
            if workspace == WorkspaceKind::Project {
                EditorType::ObjectTree
            } else {
                EditorType::ModuleLibrary
            },
        );
        layout.split(3, Axis::Vertical).unwrap();
        layout.choose(
            5,
            if workspace == WorkspaceKind::Project {
                EditorType::ObjectInspector
            } else {
                EditorType::Viewport
            },
        );
        if let Area::Split { ratio, .. } = &mut layout.root {
            *ratio = 0.68;
        }
        layout
    }
    pub fn panes(&self) -> Vec<(u64, EditorType)> {
        fn walk(area: &Area, result: &mut Vec<(u64, EditorType)>) {
            match area {
                Area::Pane { id, editor } => result.push((*id, *editor)),
                Area::Split { first, second, .. } => {
                    walk(first, result);
                    walk(second, result);
                }
            }
        }
        let mut result = vec![];
        walk(&self.root, &mut result);
        result
    }
    pub fn validate(&self) -> Result<(), String> {
        fn walk(area: &Area, depth: usize, ids: &mut BTreeSet<u64>) -> Result<(), String> {
            if depth > 12 || ids.len() > 64 {
                return Err("Personal layout exceeds area/depth limits".into());
            }
            let id = match area {
                Area::Pane { id, .. } | Area::Split { id, .. } => *id,
            };
            if id == 0 || !ids.insert(id) {
                return Err("Personal layout area IDs must be unique and nonzero".into());
            }
            if let Area::Split {
                ratio,
                first,
                second,
                ..
            } = area
            {
                if !ratio.is_finite() || !(0.08..=0.92).contains(ratio) {
                    return Err("Personal split ratio is invalid".into());
                }
                walk(first, depth + 1, ids)?;
                walk(second, depth + 1, ids)?;
            }
            Ok(())
        }
        let mut ids = BTreeSet::new();
        walk(&self.root, 0, &mut ids)?;
        if self.next_id <= ids.last().copied().unwrap_or(0) || self.next_id == u64::MAX {
            return Err("Personal layout ID sequence is invalid".into());
        }
        Ok(())
    }
    pub fn restrict(&mut self, available: &[EditorDescriptor]) -> bool {
        fn walk(area: &mut Area, available: &[EditorDescriptor]) -> bool {
            match area {
                Area::Pane { editor, .. } => {
                    if !available.iter().any(|d| d.editor == *editor) {
                        *editor = available
                            .iter()
                            .find(|descriptor| descriptor.editor == EditorType::Scene)
                            .or_else(|| available.first())
                            .map(|descriptor| descriptor.editor)
                            .unwrap_or(EditorType::Scene);
                        true
                    } else {
                        false
                    }
                }
                Area::Split { first, second, .. } => {
                    let a = walk(first, available);
                    let b = walk(second, available);
                    a || b
                }
            }
        }
        walk(&mut self.root, available)
    }
    pub fn split(&mut self, pane: u64, axis: Axis) -> Result<u64, String> {
        if self.panes().len() >= 24 {
            return Err("At most 24 areas per workspace".into());
        }
        fn find(area: &mut Area, pane: u64) -> Option<&mut Area> {
            match area {
                Area::Pane { id, .. } if *id == pane => Some(area),
                Area::Split { first, second, .. } => {
                    find(first, pane).or_else(|| find(second, pane))
                }
                _ => None,
            }
        }
        // Build first, then validate. A deep user layout must not be made
        // unsaveable by splitting its deepest leaf or exhausting its IDs.
        let mut candidate = self.clone();
        let target = find(&mut candidate.root, pane).ok_or("Area not found")?;
        let editor = match target {
            Area::Pane { editor, .. } => *editor,
            _ => unreachable!(),
        };
        let split = candidate.next_id;
        let added = split.checked_add(1).ok_or("Area ID exhausted")?;
        candidate.next_id = added.checked_add(1).ok_or("Area ID exhausted")?;
        *target = Area::Split {
            id: split,
            axis,
            ratio: 0.5,
            first: Box::new(Area::Pane { id: pane, editor }),
            second: Box::new(Area::Pane { id: added, editor }),
        };
        candidate.validate()?;
        *self = candidate;
        Ok(added)
    }
    pub fn choose(&mut self, pane: u64, editor: EditorType) -> bool {
        fn walk(area: &mut Area, pane: u64, editor: EditorType) -> bool {
            match area {
                Area::Pane {
                    id,
                    editor: current,
                } if *id == pane => {
                    let changed = *current != editor;
                    *current = editor;
                    changed
                }
                Area::Split { first, second, .. } => {
                    walk(first, pane, editor) || walk(second, pane, editor)
                }
                _ => false,
            }
        }
        walk(&mut self.root, pane, editor)
    }
    pub fn close(&mut self, pane: u64) -> bool {
        fn walk(area: &mut Area, pane: u64) -> bool {
            let Area::Split { first, second, .. } = area else {
                return false;
            };
            if matches!(first.as_ref(),Area::Pane{id,..} if *id==pane) {
                *area = *second.clone();
                return true;
            }
            if matches!(second.as_ref(),Area::Pane{id,..} if *id==pane) {
                *area = *first.clone();
                return true;
            }
            walk(first, pane) || walk(second, pane)
        }
        walk(&mut self.root, pane)
    }
    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        available: &[EditorDescriptor],
        active_pane: Option<u64>,
    ) -> DockResponse {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        let mut response = DockResponse::default();
        let mut actions = vec![];
        let options = DrawOptions {
            available,
            active_pane,
            closable: self.panes().len() > 1,
        };
        fn draw(
            area: &mut Area,
            rect: Rect,
            ui: &mut egui::Ui,
            options: &DrawOptions<'_>,
            response: &mut DockResponse,
            actions: &mut Vec<Action>,
        ) {
            match area {
                Area::Pane { id, editor } => {
                    if let Some(pointer) = ui.input(|i| {
                        i.pointer
                            .any_pressed()
                            .then(|| i.pointer.interact_pos())
                            .flatten()
                    }) && rect.contains(pointer)
                        && ui.ctx().layer_id_at(pointer) == Some(ui.layer_id())
                    {
                        response.activated_pane = Some(*id);
                    }
                    let header = Rect::from_min_max(
                        rect.min,
                        Pos2::new(rect.right(), (rect.top() + 30.0).min(rect.bottom())),
                    );
                    ui.painter()
                        .rect_filled(rect, 0.0, Color32::from_rgb(18, 24, 34));
                    ui.painter().rect_filled(
                        header,
                        0.0,
                        if options.active_pane == Some(*id) {
                            Color32::from_rgb(36, 61, 69)
                        } else {
                            Color32::from_rgb(30, 38, 52)
                        },
                    );
                    if options.active_pane == Some(*id) {
                        ui.painter().rect_stroke(
                            rect.shrink(0.5),
                            0.0,
                            egui::Stroke::new(1.0, Color32::from_rgb(67, 168, 156)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .id_salt(("area-header", *id))
                            .max_rect(header.shrink2(Vec2::new(4.0, 2.0))),
                        |ui| {
                            ui.horizontal(|ui| {
                                let choice = egui::ComboBox::from_id_salt("editor-type")
                                    .width((header.width() - 106.0).clamp(60.0, 180.0))
                                    .selected_text(editor.label())
                                    .show_ui(ui, |ui| {
                                        for descriptor in options.available {
                                            let option = ui.selectable_label(
                                                *editor == descriptor.editor,
                                                &descriptor.label,
                                            );
                                            response.options.insert(
                                                (*id, descriptor.editor.label()),
                                                option.rect,
                                            );
                                            if option.clicked() {
                                                actions
                                                    .push(Action::Choose(*id, descriptor.editor));
                                            }
                                        }
                                    });
                                response.choices.insert(*id, choice.response.rect);
                                for (label, axis) in
                                    [("H", Axis::Horizontal), ("V", Axis::Vertical)]
                                {
                                    let button =
                                        ui.small_button(label).on_hover_text(if label == "H" {
                                            "Split side by side"
                                        } else {
                                            "Split top / bottom"
                                        });
                                    response.split_buttons.insert((*id, label), button.rect);
                                    if button.clicked() {
                                        actions.push(Action::Split(*id, axis));
                                    }
                                }
                                let close = ui
                                    .add_enabled(options.closable, egui::Button::new("×").small())
                                    .on_hover_text("Close this area and join its sibling");
                                response.split_buttons.insert((*id, "close"), close.rect);
                                if close.clicked() {
                                    actions.push(Action::Close(*id));
                                }
                            });
                        },
                    );
                    let body = Rect::from_min_max(
                        Pos2::new(
                            rect.left() + 5.0,
                            (header.bottom() + 5.0).min(rect.bottom()),
                        ),
                        Pos2::new(
                            (rect.right() - 5.0).max(rect.left() + 5.0),
                            (rect.bottom() - 5.0).max(header.bottom() + 5.0),
                        ),
                    );
                    response.panes.push(PaneRect {
                        id: *id,
                        editor: *editor,
                        area_rect: rect,
                        rect: body.intersect(rect),
                    });
                }
                Area::Split {
                    id,
                    axis,
                    ratio,
                    first,
                    second,
                } => {
                    let span = if *axis == Axis::Horizontal {
                        rect.width()
                    } else {
                        rect.height()
                    };
                    let boundary = if *axis == Axis::Horizontal {
                        rect.left() + span * *ratio
                    } else {
                        rect.top() + span * *ratio
                    };
                    let divider = if *axis == Axis::Horizontal {
                        Rect::from_min_max(
                            Pos2::new(boundary - 3.0, rect.top()),
                            Pos2::new(boundary + 3.0, rect.bottom()),
                        )
                    } else {
                        Rect::from_min_max(
                            Pos2::new(rect.left(), boundary - 3.0),
                            Pos2::new(rect.right(), boundary + 3.0),
                        )
                    };
                    let gesture = ui
                        .interact(
                            divider,
                            ui.make_persistent_id(("area-divider", *id)),
                            Sense::drag(),
                        )
                        .on_hover_cursor(if *axis == Axis::Horizontal {
                            CursorIcon::ResizeHorizontal
                        } else {
                            CursorIcon::ResizeVertical
                        });
                    if gesture.dragged()
                        && let Some(pointer) = gesture.interact_pointer_pos()
                    {
                        let offset = if *axis == Axis::Horizontal {
                            pointer.x - rect.left()
                        } else {
                            pointer.y - rect.top()
                        };
                        let minimum = (70.0 / span.max(1.0)).clamp(0.08, 0.45);
                        let candidate = (offset / span.max(1.0)).clamp(minimum, 1.0 - minimum);
                        if candidate != *ratio {
                            *ratio = candidate;
                            response.changed = true;
                        }
                    }
                    response.dividers.insert(*id, divider);
                    ui.painter().rect_filled(
                        divider,
                        0.0,
                        if gesture.hovered() || gesture.dragged() {
                            Color32::from_rgb(70, 147, 169)
                        } else {
                            Color32::from_rgb(43, 54, 72)
                        },
                    );
                    let boundary = if *axis == Axis::Horizontal {
                        rect.left() + span * *ratio
                    } else {
                        rect.top() + span * *ratio
                    };
                    let (a, b) = if *axis == Axis::Horizontal {
                        (
                            Rect::from_min_max(rect.min, Pos2::new(boundary - 3.0, rect.bottom())),
                            Rect::from_min_max(Pos2::new(boundary + 3.0, rect.top()), rect.max),
                        )
                    } else {
                        (
                            Rect::from_min_max(rect.min, Pos2::new(rect.right(), boundary - 3.0)),
                            Rect::from_min_max(Pos2::new(rect.left(), boundary + 3.0), rect.max),
                        )
                    };
                    draw(first, a, ui, options, response, actions);
                    draw(second, b, ui, options, response, actions);
                }
            }
        }
        draw(
            &mut self.root,
            rect,
            ui,
            &options,
            &mut response,
            &mut actions,
        );
        for action in actions {
            response.changed |= match action {
                Action::Split(id, axis) => match self.split(id, axis) {
                    Ok(added) => {
                        response.activated_pane = Some(added);
                        true
                    }
                    Err(_) => false,
                },
                Action::Close(id) => self.close(id),
                Action::Choose(id, editor) => {
                    response.activated_pane = Some(id);
                    self.choose(id, editor)
                }
            };
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(
        ctx: &egui::Context,
        layout: &mut DockLayout,
        time: f64,
        events: Vec<egui::Event>,
    ) -> DockResponse {
        let mut result = None;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                result = Some(layout.draw(ui, &EditorDescriptor::builtins(true), Some(1)));
            },
        );
        output.drop_without_applying_deltas();
        result.unwrap()
    }
    fn click(ctx: &egui::Context, layout: &mut DockLayout, point: Pos2, time: f64) -> DockResponse {
        frame(
            ctx,
            layout,
            time,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            ctx,
            layout,
            time + 0.02,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
    }
    #[test]
    fn actual_pointer_split_resize_editor_choice_and_join_preserve_pane_ids() {
        let ctx = egui::Context::default();
        let mut layout = DockLayout::single(EditorType::Viewport);
        let first = frame(&ctx, &mut layout, 0.0, vec![]);
        let button = first.split_buttons[&(1, "H")].center();
        click(&ctx, &mut layout, button, 0.1);
        let split = frame(&ctx, &mut layout, 0.15, vec![]);
        assert_eq!(layout.panes().len(), 2);
        assert_eq!(layout.panes()[0].0, 1);
        let (divider, rect) = split
            .dividers
            .iter()
            .next()
            .map(|(id, rect)| (*id, *rect))
            .unwrap();
        let start = rect.center();
        frame(
            &ctx,
            &mut layout,
            0.2,
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let moved = start + Vec2::new(100.0, 0.0);
        frame(
            &ctx,
            &mut layout,
            0.23,
            vec![egui::Event::PointerMoved(moved)],
        );
        frame(
            &ctx,
            &mut layout,
            0.26,
            vec![egui::Event::PointerButton {
                pos: moved,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(matches!(layout.root,Area::Split{id,ratio,..} if id==divider && ratio>0.6));
        let state = frame(&ctx, &mut layout, 0.3, vec![]);
        click(&ctx, &mut layout, state.choices[&1].center(), 0.32);
        let menu = frame(&ctx, &mut layout, 0.36, vec![]);
        let option = menu.options[&(1, EditorType::ObjectTree.label())].center();
        click(&ctx, &mut layout, option, 0.38);
        assert_eq!(layout.panes()[0], (1, EditorType::ObjectTree));
        let state = frame(&ctx, &mut layout, 0.42, vec![]);
        let other = layout.panes()[1].0;
        click(
            &ctx,
            &mut layout,
            state.split_buttons[&(other, "close")].center(),
            0.44,
        );
        assert_eq!(layout.panes(), vec![(1, EditorType::ObjectTree)]);
        let state = frame(&ctx, &mut layout, 0.5, vec![]);
        click(
            &ctx,
            &mut layout,
            state.split_buttons[&(1, "V")].center(),
            0.52,
        );
        assert!(matches!(
            layout.root,
            Area::Split {
                axis: Axis::Vertical,
                ..
            }
        ));
        layout.validate().unwrap();
        let restored: DockLayout =
            serde_json::from_str(&serde_json::to_string(&layout).unwrap()).unwrap();
        assert_eq!(restored, layout);
    }
    #[test]
    fn profile_restriction_and_corrupt_layout_are_enforced() {
        let mut layout = DockLayout::default_for(WorkspaceKind::Engine);
        assert!(layout.restrict(&EditorDescriptor::builtins(false)));
        assert!(
            layout
                .panes()
                .iter()
                .all(|(_, editor)| !editor.requires_engine())
        );
        let invalid = DockLayout {
            root: Area::Split {
                id: 2,
                axis: Axis::Horizontal,
                ratio: 0.5,
                first: Box::new(Area::Pane {
                    id: 1,
                    editor: EditorType::Viewport,
                }),
                second: Box::new(Area::Pane {
                    id: 1,
                    editor: EditorType::Viewport,
                }),
            },
            next_id: 3,
        };
        assert!(invalid.validate().is_err());
    }
    #[test]
    fn area_activation_includes_header_and_content_and_split_selects_new_area() {
        let ctx = egui::Context::default();
        let mut layout = DockLayout::default_for(WorkspaceKind::Project);
        let state = frame(&ctx, &mut layout, 0.0, vec![]);
        let area = &state.panes[1];
        let id = area.id;
        // Stay inside header chrome, away from the splitter's expanded drag
        // hit area. The four-pane Project layout has a divider above Assets.
        let header = Pos2::new(area.area_rect.right() - 12.0, area.area_rect.top() + 15.0);
        assert!(area.area_rect.contains(header));
        assert!(!area.rect.contains(header));
        let pressed = frame(
            &ctx,
            &mut layout,
            0.1,
            vec![
                egui::Event::PointerMoved(header),
                egui::Event::PointerButton {
                    pos: header,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(pressed.activated_pane, Some(id));
        frame(
            &ctx,
            &mut layout,
            0.12,
            vec![egui::Event::PointerButton {
                pos: header,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let body = state.panes[2].rect.center();
        let body_id = state.panes[2].id;
        let pressed = frame(
            &ctx,
            &mut layout,
            0.2,
            vec![
                egui::Event::PointerMoved(body),
                egui::Event::PointerButton {
                    pos: body,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(pressed.activated_pane, Some(body_id));
        frame(
            &ctx,
            &mut layout,
            0.22,
            vec![egui::Event::PointerButton {
                pos: body,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let next = layout.next_id + 1;
        let split = click(
            &ctx,
            &mut layout,
            state.split_buttons[&(body_id, "H")].center(),
            0.3,
        );
        assert_eq!(split.activated_pane, Some(next));
        assert!(layout.panes().iter().any(|(id, _)| *id == next));
    }
}
