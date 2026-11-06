//! Immutable-source level palette. Apply proposes a document; Editor publishes it.
use crate::{Brush, Cell, GridDraft};
use egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use module_engine_core::{
    GameProject, GraphDocument, ModuleRegistry, ModuleToolContext, ModuleToolResponse,
};

#[derive(Clone)]
struct DraftState {
    source: GameProject,
    draft: GridDraft,
    base: GridDraft,
    document_instance: u64,
    revision: u64,
    brush: Brush,
    error: Option<String>,
    last_cell: Option<Cell>,
}

/// Source-installed module tool: no mutable Editor, filesystem or history access.
pub fn draw_tool(
    ui: &mut egui::Ui,
    document: &GraphDocument,
    registry: &ModuleRegistry,
    context: &ModuleToolContext,
) -> Result<ModuleToolResponse, String> {
    if registry.descriptor("g1.rules").is_none() {
        return Err("Crate Garden source tools are not installed".into());
    }
    let game = document
        .game
        .as_ref()
        .ok_or("Open a Crate Garden game project")?;
    if !game
        .execution
        .stages
        .iter()
        .flat_map(|stage| &stage.nodes)
        .any(|node| node.module == "g1.rules")
    {
        return Err("Crate Garden rules are not active in this project".into());
    }
    let scenes = crate::level_ids(game)?;
    let first = scenes
        .first()
        .ok_or("Crate Garden has no authored levels")?;
    let id = egui::Id::new(("g1.grid.draft", context.storage_id));
    let mut state = if let Some(state) = ui.ctx().data(|data| data.get_temp::<DraftState>(id)) {
        state
    } else {
        let draft = GridDraft::from_project(game, first)?;
        DraftState {
            source: game.clone(),
            base: draft.clone(),
            draft,
            document_instance: context.document_instance,
            revision: context.revision,
            brush: Brush::Wall,
            error: None,
            last_cell: None,
        }
    };
    // Only an explicit application acknowledgement can resolve a dirty draft.
    // Clean caches follow ordinary Undo/Redo; dirty caches retain a stale draft.
    let new_document = state.document_instance != context.document_instance;
    if new_document
        || state.revision != context.revision
        || (&state.source != game && state.draft == state.base)
    {
        let scene = if !new_document && scenes.contains(&state.draft.scene_id) {
            &state.draft.scene_id
        } else {
            first
        };
        let draft = GridDraft::from_project(game, scene)?;
        state.source = game.clone();
        state.draft = draft;
        state.base = state.draft.clone();
        state.document_instance = context.document_instance;
        state.revision = context.revision;
        state.error = None;
    }
    let dirty = state.draft != state.base;
    let stale = &state.source != game;
    let mut candidate = None;
    ui.heading("Crate Garden · Level palette");
    ui.label("Paint a draft, then Apply. One Apply creates one Undo step.");
    ui.add_space(8.);
    ui.horizontal(|ui| {
        ui.label("Garden");
        ui.add_enabled_ui(!dirty, |ui| {
            egui::ComboBox::from_id_salt("g1.grid.scene")
                .selected_text(&state.draft.title)
                .show_ui(ui, |ui| {
                    for scene in &scenes {
                        let title = game
                            .scenes
                            .get(scene)
                            .map_or(scene.as_str(), |scene| scene.name.as_str());
                        if ui
                            .selectable_label(scene == &state.draft.scene_id, title)
                            .clicked()
                        {
                            match GridDraft::from_project(game, scene) {
                                Ok(draft) => {
                                    state.draft = draft;
                                    state.base = state.draft.clone();
                                    state.source = game.clone();
                                    state.error = None;
                                }
                                Err(error) => state.error = Some(error),
                            }
                        }
                    }
                });
        });
    });
    ui.horizontal(|ui| {
        ui.label("Width");
        ui.add(egui::DragValue::new(&mut state.draft.width).range(3..=32));
        ui.label("Height");
        ui.add(egui::DragValue::new(&mut state.draft.height).range(3..=32));
        ui.label("Cell size");
        ui.add(
            egui::DragValue::new(&mut state.draft.cell_size)
                .range(0.1..=4.0)
                .speed(0.05),
        );
    });
    ui.horizontal_wrapped(|ui| {
        for (brush, label) in [
            (Brush::Floor, "Floor"),
            (Brush::Wall, "Wall"),
            (Brush::Goal, "Flower / goal"),
            (Brush::Crate, "Crate"),
            (Brush::Pawn, "Gardener"),
            (Brush::Erase, "Erase"),
        ] {
            if ui.selectable_label(state.brush == brush, label).clicked() {
                state.brush = brush;
            }
        }
    });
    ui.small("Drag to paint. A goal can sit under a crate. Use Erase to clear a cell.");
    ui.add_space(8.);
    egui::ScrollArea::both()
        .max_height(540.)
        .show(ui, |ui| draw_grid(ui, &mut state));
    ui.add_space(8.);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!stale, egui::Button::new("Apply level"))
            .clicked()
        {
            match state.draft.into_project(game) {
                Ok(edited) => {
                    let mut proposed = document.clone();
                    proposed.game = Some(edited);
                    candidate = Some(proposed);
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
        }
        if ui.button("Discard draft").clicked() {
            state.source = game.clone();
            match GridDraft::from_project(game, &state.draft.scene_id) {
                Ok(draft) => {
                    state.draft = draft;
                    state.base = state.draft.clone();
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
        }
        ui.label(if stale {
            "Source changed. Discard this draft before continuing."
        } else if dirty {
            "Draft has unapplied changes"
        } else {
            "Level matches authored source"
        });
    });
    if let Some(error) = &state.error {
        ui.colored_label(Color32::from_rgb(186, 83, 47), error);
    }
    ui.small("Apply validates the whole board. Keep one gardener and matching crates/goals. Resize is checked without silently removing objects.");
    let dirty = state.draft != state.base;
    ui.ctx().data_mut(|data| data.insert_temp(id, state));
    Ok(ModuleToolResponse { candidate, dirty })
}

fn draw_grid(ui: &mut egui::Ui, state: &mut DraftState) -> Rect {
    let cell_size = 34.;
    let size = Vec2::new(state.draft.width as f32, state.draft.height as f32) * cell_size;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8., Color32::from_rgb(239, 233, 210));
    for y in 0..state.draft.height {
        for x in 0..state.draft.width {
            let cell = Cell::new(x as i32, y as i32);
            let tile = Rect::from_min_size(
                rect.min + Vec2::new(x as f32, y as f32) * cell_size,
                Vec2::splat(cell_size),
            )
            .shrink(1.);
            painter.rect_stroke(
                tile,
                3.,
                Stroke::new(1., Color32::from_rgb(216, 213, 190)),
                egui::StrokeKind::Inside,
            );
            if state.draft.walls.contains(&cell) {
                painter.rect_filled(tile.shrink(2.), 5., Color32::from_rgb(115, 124, 90));
            }
            if state.draft.goals.contains(&cell) {
                painter.circle_filled(tile.center(), 11., Color32::from_rgb(229, 178, 91));
                for offset in [
                    Vec2::new(-6., 0.),
                    Vec2::new(6., 0.),
                    Vec2::new(0., -6.),
                    Vec2::new(0., 6.),
                ] {
                    painter.circle_filled(
                        tile.center() + offset,
                        4.,
                        Color32::from_rgb(255, 243, 198),
                    );
                }
            }
            if state.draft.crates.contains(&cell) {
                painter.rect_filled(tile.shrink(5.), 4., Color32::from_rgb(186, 126, 61));
                painter.line_segment(
                    [tile.min + Vec2::splat(9.), tile.max - Vec2::splat(9.)],
                    Stroke::new(2., Color32::from_rgb(242, 196, 122)),
                );
                painter.line_segment(
                    [
                        Pos2::new(tile.right() - 9., tile.top() + 9.),
                        Pos2::new(tile.left() + 9., tile.bottom() - 9.),
                    ],
                    Stroke::new(2., Color32::from_rgb(242, 196, 122)),
                );
                if state.draft.goals.contains(&cell) {
                    painter.circle_filled(
                        tile.right_top() + Vec2::new(-5., 5.),
                        4.,
                        Color32::from_rgb(241, 221, 123),
                    );
                }
            }
            if state.draft.pawn == cell {
                painter.circle_filled(tile.center(), 10., Color32::from_rgb(37, 103, 85));
                painter.circle_filled(
                    tile.center() + Vec2::new(2., -4.),
                    4.,
                    Color32::from_rgb(160, 195, 115),
                );
            }
        }
    }
    let painting = response.clicked()
        || (ui.input(|input| input.pointer.primary_down())
            && (response.dragged() || response.is_pointer_button_down_on()));
    if painting && let Some(pointer) = response.interact_pointer_pos() {
        let relative = (pointer - rect.min) / cell_size;
        let cell = Cell::new(relative.x.floor() as i32, relative.y.floor() as i32);
        if rect.contains(pointer) && state.last_cell != Some(cell) {
            if let Err(error) = state.draft.paint(cell, state.brush) {
                state.error = Some(error);
            } else {
                state.error = None;
            }
            state.last_cell = Some(cell);
        }
    }
    if !ui.input(|input| input.pointer.primary_down()) {
        state.last_cell = None;
    }
    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grid_frame(
        context: &egui::Context,
        state: &mut DraftState,
        events: Vec<egui::Event>,
    ) -> Rect {
        let mut rect = Rect::NOTHING;
        context
            .run_ui(
                egui::RawInput {
                    focused: true,
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    rect = draw_grid(ui, state);
                },
            )
            .drop_without_applying_deltas();
        rect
    }

    #[test]
    fn actual_canvas_stroke_only_changes_local_draft_until_candidate_is_requested() {
        let source = crate::project();
        let draft = GridDraft::from_project(&source, "level01").unwrap();
        let mut state = DraftState {
            source: source.clone(),
            base: draft.clone(),
            draft,
            document_instance: 0,
            revision: 0,
            brush: Brush::Wall,
            error: None,
            last_cell: None,
        };
        let context = egui::Context::default();
        let rect = grid_frame(&context, &mut state, vec![]);
        let first = rect.min + Vec2::new(2.5, 1.5) * 34.;
        let second = rect.min + Vec2::new(2.5, 2.5) * 34.;
        grid_frame(
            &context,
            &mut state,
            vec![
                egui::Event::PointerMoved(first),
                egui::Event::PointerButton {
                    pos: first,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        grid_frame(
            &context,
            &mut state,
            vec![egui::Event::PointerMoved(second)],
        );
        grid_frame(
            &context,
            &mut state,
            vec![egui::Event::PointerButton {
                pos: second,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(state.draft.walls.contains(&Cell::new(2, 1)));
        assert!(state.draft.walls.contains(&Cell::new(2, 2)));
        assert_eq!(state.source, source);
        let candidate = state.draft.into_project(&source).unwrap();
        assert_eq!(candidate.execution, source.execution);
        assert_eq!(candidate.scenes["level02"], source.scenes["level02"]);
        assert_ne!(candidate.scenes["level01"], source.scenes["level01"]);
    }
}
