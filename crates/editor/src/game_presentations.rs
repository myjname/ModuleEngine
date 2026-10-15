//! Source-installed UI callbacks selected by the host's active contribution ID.
use eframe::egui::{self, Rect};
use module_engine_core::{Camera, GameInput, GameState};
use std::collections::BTreeMap;
use std::sync::OnceLock;

type InputCallback = fn(&egui::Ui, &egui::Response, Rect, Camera, bool) -> GameInput;
type HudCallback = fn(&mut egui::Ui, Rect, &GameState, Camera, Option<[f32; 3]>) -> bool;
type AimCallback = fn(&GameState, &GameInput, bool, bool) -> Option<[f32; 3]>;

pub struct UiContribution {
    pub id: &'static str,
    pub input: InputCallback,
    pub hud: HudCallback,
    pub preview_aim: Option<AimCallback>,
    pub has_input: Option<fn(&GameInput) -> bool>,
    pub capture_input: Option<fn(&GameState) -> GameInput>,
}

#[derive(Default)]
pub struct UiRegistry {
    contributions: BTreeMap<&'static str, UiContribution>,
}
impl UiRegistry {
    pub fn register(&mut self, contribution: UiContribution) -> Result<(), String> {
        if contribution.id.is_empty() || self.contributions.contains_key(contribution.id) {
            return Err(format!(
                "Invalid or duplicate UI contribution {}",
                contribution.id
            ));
        }
        self.contributions.insert(contribution.id, contribution);
        Ok(())
    }
    pub fn get(&self, id: Option<&str>) -> Option<&UiContribution> {
        self.contributions.get(id?)
    }
}

fn installed() -> &'static UiRegistry {
    static REGISTRY: OnceLock<UiRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = UiRegistry::default();
        // This is the UI composition root. Each installed package supplies its
        // callbacks; the Player dispatch paths below contain no game rules.
        registry
            .register(UiContribution {
                id: "arpg.presentation",
                input: module_engine_arpg::ui::read_input_mode,
                hud: module_engine_arpg::ui::draw_hud,
                preview_aim: Some(module_engine_arpg::ui::preview_aim),
                has_input: Some(module_engine_arpg::ui::has_input),
                capture_input: Some(module_engine_arpg::ui::capture_input),
            })
            .expect("Unique installed UI contribution");
        registry
    })
}

pub fn read_input_for(
    id: Option<&str>,
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    camera: Camera,
    respect_editor_focus: bool,
) -> GameInput {
    let mut input = installed()
        .get(id)
        .map_or_else(GameInput::default, |contribution| {
            (contribution.input)(ui, response, rect, camera, respect_editor_focus)
        });
    // Restart is a common session control even without a gameplay UI module.
    let keyboard_allowed = ui.input(|state| state.focused)
        && (!respect_editor_focus || !ui.ctx().egui_wants_keyboard_input() || response.has_focus());
    input.restart |= keyboard_allowed && ui.input(|state| state.key_pressed(egui::Key::R));
    input
}

pub fn draw_hud_for(
    id: Option<&str>,
    ui: &mut egui::Ui,
    rect: Rect,
    state: &GameState,
    camera: Camera,
    aim: Option<[f32; 3]>,
) -> bool {
    installed()
        .get(id)
        .is_some_and(|contribution| (contribution.hud)(ui, rect, state, camera, aim))
}

pub fn preview_aim_for(
    id: Option<&str>,
    state: &GameState,
    input: &GameInput,
    pointer_active: bool,
    capture_attack: bool,
) -> Option<[f32; 3]> {
    installed()
        .get(id)?
        .preview_aim
        .and_then(|callback| callback(state, input, pointer_active, capture_attack))
}

pub fn has_input_for(id: Option<&str>, input: &GameInput) -> bool {
    installed()
        .get(id)
        .and_then(|contribution| contribution.has_input)
        .map_or(!input.actions.is_empty() || input.restart, |callback| {
            callback(input)
        })
}

pub fn capture_input_for(id: Option<&str>, state: &GameState) -> GameInput {
    installed()
        .get(id)
        .and_then(|contribution| contribution.capture_input)
        .map_or_else(GameInput::default, |callback| callback(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_presentation_has_no_game_controls_and_keeps_generic_restart() {
        let context = egui::Context::default();
        let mut input = GameInput::default();
        let mut output = context.run_ui(
            egui::RawInput {
                focused: true,
                events: vec![egui::Event::Key {
                    key: egui::Key::R,
                    physical_key: Some(egui::Key::R),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(egui::Vec2::new(800., 600.), egui::Sense::click());
                input = read_input_for(None, ui, &response, rect, Camera::default(), false);
                let state: GameState = serde_json::from_value(serde_json::json!({
                    "scene":"empty","time":0.,"tick":0,"entities":{}
                }))
                .unwrap();
                assert!(!draw_hud_for(
                    None,
                    ui,
                    rect,
                    &state,
                    Camera::default(),
                    None
                ));
                assert!(preview_aim_for(None, &state, &input, true, true).is_none());
                assert_eq!(capture_input_for(None, &state), GameInput::default());
            },
        );
        output.textures_delta.clear();
        assert!(input.restart);
        assert!(input.values.is_empty());
        assert!(input.actions.is_empty());
    }

    #[test]
    fn independently_installed_ui_uses_its_own_id_and_rejects_duplicates() {
        fn input(_: &egui::Ui, _: &egui::Response, _: Rect, _: Camera, _: bool) -> GameInput {
            GameInput {
                actions: ["puzzle.use".into()].into_iter().collect(),
                ..Default::default()
            }
        }
        fn hud(_: &mut egui::Ui, _: Rect, _: &GameState, _: Camera, _: Option<[f32; 3]>) -> bool {
            true
        }
        fn contribution() -> UiContribution {
            UiContribution {
                id: "puzzle.presentation",
                input,
                hud,
                preview_aim: None,
                has_input: None,
                capture_input: None,
            }
        }
        let mut registry = UiRegistry::default();
        registry.register(contribution()).unwrap();
        assert!(registry.get(Some("puzzle.presentation")).is_some());
        assert!(registry.get(Some("arpg.presentation")).is_none());
        assert!(registry.register(contribution()).is_err());
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::Vec2::new(800., 600.), egui::Sense::click());
            let contribution = registry.get(Some("puzzle.presentation")).unwrap();
            let input = (contribution.input)(ui, &response, rect, Camera::default(), false);
            assert!(input.pressed("puzzle.use"));
            let state: GameState = serde_json::from_value(serde_json::json!({
                "scene":"empty","time":0.,"tick":0,"entities":{}
            }))
            .unwrap();
            assert!((contribution.hud)(
                ui,
                rect,
                &state,
                Camera::default(),
                None
            ));
        });
        output.textures_delta.clear();
    }
}
