//! Optional egui controls and HUD for the Puzzle source project.
use crate::{Direction, PuzzleOutcome, direction_input, frame};
use egui::{self, Color32, Pos2, Rect, Vec2};
use module_engine_core::{Camera, GameInput, GameState};

pub fn read_input_mode(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    _camera: Camera,
    respect_editor_focus: bool,
) -> GameInput {
    if ui.input(|input| input.focused && input.pointer.any_pressed()) && response.hovered() {
        response.request_focus();
    }
    if !ui.input(|input| input.focused)
        || (respect_editor_focus && ui.ctx().egui_wants_keyboard_input() && !response.has_focus())
        || rect.width() <= 0.
        || rect.height() <= 0.
    {
        return GameInput::default();
    }
    let (direction, restart) = ui.input(|input| {
        let mut direction = None;
        let mut restart = false;
        for event in &input.events {
            let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                ..
            } = event
            else {
                continue;
            };
            if *key == egui::Key::R {
                restart = true;
            }
            // Preserve event order when two directions are pressed together.
            // Repeated or merely held keys never create another turn.
            if direction.is_none() {
                direction = match key {
                    egui::Key::ArrowUp | egui::Key::W => Some(Direction::North),
                    egui::Key::ArrowRight | egui::Key::D => Some(Direction::East),
                    egui::Key::ArrowDown | egui::Key::S => Some(Direction::South),
                    egui::Key::ArrowLeft | egui::Key::A => Some(Direction::West),
                    _ => None,
                };
            }
        }
        (direction, restart)
    });
    if restart {
        return GameInput {
            restart: true,
            ..Default::default()
        };
    }
    direction.map_or_else(GameInput::default, direction_input)
}

pub fn has_input(input: &GameInput) -> bool {
    !input.actions.is_empty() || input.restart
}

pub fn draw_hud(
    ui: &mut egui::Ui,
    rect: Rect,
    state: &GameState,
    _camera: Camera,
    _aim: Option<[f32; 3]>,
) -> bool {
    if rect.width() <= 0. || rect.height() <= 0. {
        return false;
    }
    let painter = ui.painter_at(rect);
    let panel = Rect::from_min_max(
        rect.left_top() + Vec2::splat(16.),
        rect.left_top() + Vec2::new(rect.width() - 16., 112.),
    );
    painter.rect_filled(panel, 6., Color32::from_black_alpha(190));
    let left = panel.left_top() + Vec2::new(12., 10.);
    painter.text(
        left,
        egui::Align2::LEFT_TOP,
        "Puzzle",
        egui::FontId::proportional(20.),
        Color32::WHITE,
    );
    match frame(state) {
        Ok(frame) => {
            let (outcome, color) = match frame.outcome {
                PuzzleOutcome::Playing => ("Playing", Color32::WHITE),
                PuzzleOutcome::Solved => ("Solved", Color32::from_rgb(115, 225, 145)),
                PuzzleOutcome::Failed => ("Failed", Color32::LIGHT_RED),
            };
            painter.text(
                left + Vec2::new(0., 29.),
                egui::Align2::LEFT_TOP,
                format!("Turns: {}", frame.turns),
                egui::FontId::proportional(16.),
                Color32::WHITE,
            );
            painter.text(
                left + Vec2::new(126., 29.),
                egui::Align2::LEFT_TOP,
                format!("Outcome: {outcome}"),
                egui::FontId::proportional(16.),
                color,
            );
        }
        Err(error) => {
            painter.text(
                left + Vec2::new(0., 29.),
                egui::Align2::LEFT_TOP,
                format!("Puzzle state unavailable: {error}"),
                egui::FontId::proportional(14.),
                Color32::LIGHT_RED,
            );
        }
    }
    painter.text(
        left + Vec2::new(0., 59.),
        egui::Align2::LEFT_TOP,
        "Arrow keys / WASD: one move per press · R: restart",
        egui::FontId::proportional(14.),
        Color32::LIGHT_GRAY,
    );
    ui.put(restart_rect(rect), egui::Button::new("Restart"))
        .clicked()
}

fn restart_rect(rect: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(rect.right() - 140., rect.top() + 26.),
        Vec2::new(112., 32.),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FRAME_KEY, PuzzleFrame};
    use module_engine_core::{GameSession, WorldKind};

    fn key(key: egui::Key, pressed: bool, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn input_frame(
        context: &egui::Context,
        events: Vec<egui::Event>,
        focus_text: bool,
        focused: bool,
    ) -> GameInput {
        let mut result = GameInput::default();
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                events,
                focused,
                ..Default::default()
            },
            |ui| {
                let mut source = String::new();
                let text = ui.put(
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(180., 30.)),
                    egui::TextEdit::singleline(&mut source).id(egui::Id::new("puzzle-source")),
                );
                if focus_text {
                    text.request_focus();
                }
                let rect = Rect::from_min_max(Pos2::new(0., 40.), Pos2::new(800., 600.));
                let response =
                    ui.interact(rect, egui::Id::new("puzzle-viewport"), egui::Sense::click());
                result = read_input_mode(ui, &response, rect, Camera::default(), true);
            },
        );
        output.textures_delta.clear();
        result
    }

    #[test]
    fn arrows_and_wasd_generate_one_direction_for_each_fresh_press() {
        for (key_code, direction) in [
            (egui::Key::ArrowUp, Direction::North),
            (egui::Key::W, Direction::North),
            (egui::Key::ArrowRight, Direction::East),
            (egui::Key::D, Direction::East),
            (egui::Key::ArrowDown, Direction::South),
            (egui::Key::S, Direction::South),
            (egui::Key::ArrowLeft, Direction::West),
            (egui::Key::A, Direction::West),
        ] {
            let context = egui::Context::default();
            input_frame(&context, vec![], false, true);
            assert_eq!(
                input_frame(&context, vec![key(key_code, true, false)], false, true),
                direction_input(direction)
            );
            assert_eq!(
                input_frame(&context, vec![], false, true),
                GameInput::default(),
                "held key replayed"
            );
            assert_eq!(
                input_frame(&context, vec![key(key_code, true, true)], false, true),
                GameInput::default(),
                "OS repeat replayed"
            );
            assert_eq!(
                input_frame(&context, vec![key(key_code, false, false)], false, true),
                GameInput::default()
            );
            assert_eq!(
                input_frame(&context, vec![key(key_code, true, false)], false, true),
                direction_input(direction)
            );
        }
    }

    #[test]
    fn editor_text_focus_blocks_controls_until_the_viewport_is_clicked() {
        let context = egui::Context::default();
        input_frame(&context, vec![], true, true);
        assert_eq!(
            input_frame(&context, vec![key(egui::Key::W, true, false)], true, true),
            GameInput::default()
        );
        input_frame(&context, vec![key(egui::Key::W, false, false)], true, true);
        let position = Pos2::new(400., 300.);
        let input = input_frame(
            &context,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                key(egui::Key::W, true, false),
            ],
            false,
            true,
        );
        assert_eq!(input, direction_input(Direction::North));
        assert_eq!(
            input_frame(&context, vec![key(egui::Key::D, true, false)], false, false),
            GameInput::default()
        );
    }

    #[test]
    fn first_direction_is_deterministic_and_restart_cancels_directional_input() {
        let context = egui::Context::default();
        assert_eq!(
            input_frame(
                &context,
                vec![
                    key(egui::Key::ArrowLeft, true, false),
                    key(egui::Key::W, true, false)
                ],
                false,
                true
            ),
            direction_input(Direction::West)
        );
        let input = input_frame(
            &context,
            vec![
                key(egui::Key::D, true, false),
                key(egui::Key::R, true, false),
            ],
            false,
            true,
        );
        assert!(input.restart);
        assert!(input.actions.is_empty());
        assert!(has_input(&input));
        assert!(!has_input(&GameInput::default()));
    }

    fn state(outcome: PuzzleOutcome, turns: u64) -> GameState {
        let session =
            GameSession::new_with_registry(crate::project(), WorldKind::Ecs, crate::registry())
                .unwrap();
        let mut state = session.state().clone();
        state.module_data.insert(
            FRAME_KEY.into(),
            serde_json::to_value(PuzzleFrame { outcome, turns }).unwrap(),
        );
        state
    }

    #[test]
    fn hud_displays_owned_turns_outcomes_and_restart_without_mutating_state() {
        for (outcome, label) in [
            (PuzzleOutcome::Playing, "Playing"),
            (PuzzleOutcome::Solved, "Solved"),
            (PuzzleOutcome::Failed, "Failed"),
        ] {
            let context = egui::Context::default();
            let state = state(outcome, 7);
            let before = state.clone();
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.));
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                assert!(!draw_hud(ui, rect, &state, Camera::default(), None));
            });
            for expected in [
                "Puzzle".to_owned(),
                "Turns: 7".to_owned(),
                format!("Outcome: {label}"),
                "Restart".to_owned(),
            ] {
                assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text()==expected)),"missing HUD label {expected}");
            }
            output.textures_delta.clear();
            assert_eq!(state, before);
            let position = restart_rect(rect).center();
            let mut clicked = false;
            for pressed in [true, false] {
                let mut output = context.run_ui(
                    egui::RawInput {
                        events: vec![
                            egui::Event::PointerMoved(position),
                            egui::Event::PointerButton {
                                pos: position,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |ui| {
                        clicked |= draw_hud(ui, rect, &state, Camera::default(), None);
                    },
                );
                output.textures_delta.clear();
            }
            assert!(clicked, "Restart button did not respond");
        }
    }
}
