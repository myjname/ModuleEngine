//! Source-installed UI callbacks selected by the host's active contribution ID.
use eframe::egui::{self, Rect};
use module_engine_core::{Camera, GameInput, GameState, GameUiResponse, GameUiStatus};
use std::collections::BTreeMap;
use std::sync::OnceLock;

type InputCallback = fn(&egui::Ui, &egui::Response, Rect, Camera, bool) -> GameInput;
type HudCallback = fn(&mut egui::Ui, Rect, &GameState, Camera, Option<[f32; 3]>) -> bool;
pub type ControlsCallback =
    fn(&mut egui::Ui, Rect, &GameState, Camera, Option<[f32; 3]>) -> GameInput;
pub type StatusControlsCallback =
    fn(&mut egui::Ui, Rect, &GameState, Camera, Option<[f32; 3]>, &GameUiStatus) -> GameUiResponse;
type AimCallback = fn(&GameState, &GameInput, bool, bool) -> Option<[f32; 3]>;

pub struct UiContribution {
    pub id: &'static str,
    pub input: InputCallback,
    pub hud: HudCallback,
    /// Replaces the legacy restart-only HUD when present.
    pub controls: Option<ControlsCallback>,
    /// Status-aware source UI; replaces older controls without changing GameInput.
    pub controls_with_status: Option<StatusControlsCallback>,
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
        #[cfg(feature = "arpg")]
        registry
            .register(UiContribution {
                id: "arpg.presentation",
                input: module_engine_arpg::ui::read_input_mode,
                hud: module_engine_arpg::ui::draw_hud,
                controls: None,
                controls_with_status: None,
                preview_aim: Some(module_engine_arpg::ui::preview_aim),
                has_input: Some(module_engine_arpg::ui::has_input),
                capture_input: Some(module_engine_arpg::ui::capture_input),
            })
            .expect("Unique installed UI contribution");
        #[cfg(feature = "puzzle")]
        registry
            .register(UiContribution {
                id: "puzzle.presentation",
                input: module_engine_project_puzzle::ui::read_input_mode,
                hud: module_engine_project_puzzle::ui::draw_hud,
                controls: None,
                controls_with_status: None,
                preview_aim: None,
                has_input: Some(module_engine_project_puzzle::ui::has_input),
                capture_input: None,
            })
            .expect("Unique installed puzzle UI contribution");
        #[cfg(feature = "g1")]
        registry
            .register(UiContribution {
                id: "g1.presentation",
                input: module_engine_g1::ui::read_input_mode,
                hud: module_engine_g1::ui::draw_hud,
                controls: None,
                controls_with_status: Some(module_engine_g1::ui::draw_controls_with_status),
                preview_aim: None,
                has_input: Some(module_engine_g1::ui::has_input),
                capture_input: None,
            })
            .expect("Unique installed G1 UI contribution");
        #[cfg(not(any(feature = "arpg", feature = "puzzle", feature = "g1")))]
        let _ = &mut registry;
        registry
    })
}

/// Status-aware UI owns application shortcuts (for example Escape to pause).
/// Legacy presentations retain the application's historical Escape-to-close.
pub fn source_owns_window_controls(id: Option<&str>) -> bool {
    installed()
        .get(id)
        .is_some_and(|source| source.controls_with_status.is_some())
}

pub fn read_input_for(
    id: Option<&str>,
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    camera: Camera,
    respect_editor_focus: bool,
) -> GameInput {
    read_contribution_input(
        installed().get(id),
        ui,
        response,
        rect,
        camera,
        respect_editor_focus,
    )
}

fn read_contribution_input(
    contribution: Option<&UiContribution>,
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    camera: Camera,
    respect_editor_focus: bool,
) -> GameInput {
    // Egui resolves widget ownership from the previous pass. Do not send a
    // pointer gesture belonging to an interactive HUD widget to world controls.
    let pointer_widgets = ui.ctx().interaction_snapshot(|snapshot| {
        snapshot
            .hovered
            .iter()
            .copied()
            .chain(snapshot.dragged)
            .chain(snapshot.clicked)
            .collect::<Vec<_>>()
    });
    let controls_own_pointer = ui
        .input(|state| state.pointer.any_down() || state.pointer.any_released())
        && pointer_widgets.into_iter().any(|id| {
            id != response.id
                && ui
                    .ctx()
                    .read_response(id)
                    .is_some_and(|widget| widget.sense.interactive())
        });
    let mut input = contribution
        .filter(|_| !controls_own_pointer)
        .map_or_else(GameInput::default, |contribution| {
            (contribution.input)(ui, response, rect, camera, respect_editor_focus)
        });
    // Restart is a common session control even without a gameplay UI module.
    let keyboard_allowed = ui.input(|state| state.focused)
        && (!respect_editor_focus || !ui.ctx().egui_wants_keyboard_input() || response.has_focus());
    input.restart |= contribution.is_none()
        && keyboard_allowed
        && ui.input(|state| state.key_pressed(egui::Key::R));
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
    draw_controls_for(id, ui, rect, state, camera, aim).restart
}

impl UiContribution {
    pub fn draw_controls_with_status(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        state: &GameState,
        camera: Camera,
        aim: Option<[f32; 3]>,
        status: &GameUiStatus,
    ) -> GameUiResponse {
        if let Some(callback) = self.controls_with_status {
            let mut response = callback(ui, rect, state, camera, aim, status);
            response.close_application &= status.can_close_application;
            response
        } else {
            GameUiResponse {
                input: self.draw_controls(ui, rect, state, camera, aim),
                close_application: false,
            }
        }
    }
    pub fn draw_controls(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        state: &GameState,
        camera: Camera,
        aim: Option<[f32; 3]>,
    ) -> GameInput {
        if let Some(callback) = self.controls_with_status {
            callback(ui, rect, state, camera, aim, &GameUiStatus::default()).input
        } else if let Some(callback) = self.controls {
            callback(ui, rect, state, camera, aim)
        } else {
            GameInput {
                restart: (self.hud)(ui, rect, state, camera, aim),
                ..Default::default()
            }
        }
    }
}

pub fn draw_controls_for(
    id: Option<&str>,
    ui: &mut egui::Ui,
    rect: Rect,
    state: &GameState,
    camera: Camera,
    aim: Option<[f32; 3]>,
) -> GameInput {
    installed()
        .get(id)
        .map_or_else(GameInput::default, |contribution| {
            contribution.draw_controls(ui, rect, state, camera, aim)
        })
}

/// Context for one controls-first input collection. `aim` is presentation-only
/// and may come from the previous accepted packet; source input is read once.
pub struct InputCollection<'a> {
    pub id: Option<&'a str>,
    pub rect: Rect,
    pub state: &'a GameState,
    pub camera: Camera,
    pub aim: Option<[f32; 3]>,
    pub respect_editor_focus: bool,
    pub world_enabled: bool,
}

pub struct CollectedInput {
    pub input: GameInput,
    pub controls_active: bool,
    pub close_application: bool,
}

pub fn collect_input_for(
    ui: &mut egui::Ui,
    response: &egui::Response,
    collection: InputCollection<'_>,
) -> CollectedInput {
    collect_contribution_input(installed().get(collection.id), ui, response, collection)
}

pub fn collect_input_with_status_for(
    ui: &mut egui::Ui,
    response: &egui::Response,
    collection: InputCollection<'_>,
    status: &GameUiStatus,
) -> CollectedInput {
    collect_contribution_input_with_status(
        installed().get(collection.id),
        ui,
        response,
        collection,
        status,
    )
}

fn collect_contribution_input(
    contribution: Option<&UiContribution>,
    ui: &mut egui::Ui,
    response: &egui::Response,
    collection: InputCollection<'_>,
) -> CollectedInput {
    collect_contribution_input_with_status(
        contribution,
        ui,
        response,
        collection,
        &GameUiStatus::default(),
    )
}

fn collect_contribution_input_with_status(
    contribution: Option<&UiContribution>,
    ui: &mut egui::Ui,
    response: &egui::Response,
    collection: InputCollection<'_>,
    status: &GameUiStatus,
) -> CollectedInput {
    let controls = contribution.map_or_else(GameUiResponse::default, |contribution| {
        contribution.draw_controls_with_status(
            ui,
            collection.rect,
            collection.state,
            collection.camera,
            collection.aim,
            status,
        )
    });
    let close_application = controls.close_application;
    let controls = controls.input;
    let controls_active = controls != GameInput::default() || close_application;
    let pointer_control = controls_active
        && ui.input(|input| input.pointer.any_down() || input.pointer.any_released());
    let world_active =
        !collection.respect_editor_focus || response.hovered() || response.has_focus();
    let world = if collection.world_enabled
        && world_active
        && !pointer_control
        && !controls.restart
        && !close_application
    {
        read_contribution_input(
            contribution,
            ui,
            response,
            collection.rect,
            collection.camera,
            collection.respect_editor_focus,
        )
    } else {
        GameInput::default()
    };
    CollectedInput {
        input: if close_application {
            GameInput::default()
        } else {
            merge_controls_input(world, controls)
        },
        controls_active,
        close_application,
    }
}

/// Controls override matching held values and occurrence context. Restart cancels
/// gameplay packets from the same gesture, just as it cancels older queued edges.
pub fn merge_controls_input(mut world: GameInput, controls: GameInput) -> GameInput {
    if world.restart || controls.restart {
        return GameInput {
            restart: true,
            ..Default::default()
        };
    }
    world.values.extend(controls.values);
    world.actions.extend(controls.actions);
    world.action_values.extend(controls.action_values);
    world
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
        .map_or_else(
            || {
                !input.actions.is_empty()
                    || !input.values.is_empty()
                    || !input.action_values.is_empty()
                    || input.restart
            },
            |callback| callback(input),
        )
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
    fn status_controls_observe_services_and_cannot_close_an_editor_or_enter_gameplay_on_quit() {
        fn controls(
            _: &mut egui::Ui,
            _: Rect,
            _: &GameState,
            _: Camera,
            _: Option<[f32; 3]>,
            status: &GameUiStatus,
        ) -> GameUiResponse {
            assert_eq!(
                status.services["independent.store"]["write_blocked"],
                serde_json::json!(true)
            );
            assert_eq!(status.errors, vec!["Save unavailable"]);
            GameUiResponse {
                input: GameInput {
                    actions: ["independent.choose".into()].into(),
                    ..Default::default()
                },
                close_application: true,
            }
        }
        let context = egui::Context::default();
        let state = module_engine_core::GameSession::new(
            module_engine_core::GameProject::empty(),
            module_engine_core::WorldKind::Ecs,
        )
        .unwrap()
        .state()
        .clone();
        let before = serde_json::to_value(&state).unwrap();
        for standalone in [false, true] {
            let mut collected = None;
            context
                .run_ui(egui::RawInput::default(), |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(500., 300.), egui::Sense::click());
                    let mut source = synthetic_contribution(None);
                    source.controls_with_status = Some(controls);
                    let status = GameUiStatus {
                        can_close_application: standalone,
                        services: serde_json::json!({"independent.store":{"write_blocked":true}}),
                        errors: vec!["Save unavailable".into()],
                    };
                    collected = Some(collect_contribution_input_with_status(
                        Some(&source),
                        ui,
                        &response,
                        InputCollection {
                            id: Some(source.id),
                            rect,
                            state: &state,
                            camera: Camera::default(),
                            aim: None,
                            respect_editor_focus: true,
                            world_enabled: true,
                        },
                        &status,
                    ));
                })
                .drop_without_applying_deltas();
            let response = collected.unwrap();
            assert_eq!(response.close_application, standalone);
            if standalone {
                assert_eq!(response.input, GameInput::default());
            } else {
                assert!(response.input.pressed("independent.choose"));
                assert!(!response.input.restart);
            }
        }
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        assert!(!source_owns_window_controls(None));
    }

    fn synthetic_input(_: &egui::Ui, _: &egui::Response, _: Rect, _: Camera, _: bool) -> GameInput {
        GameInput {
            actions: ["independent.world".into()].into(),
            ..Default::default()
        }
    }

    fn legacy_restart(
        _: &mut egui::Ui,
        _: Rect,
        _: &GameState,
        _: Camera,
        _: Option<[f32; 3]>,
    ) -> bool {
        true
    }

    fn synthetic_controls(
        _: &mut egui::Ui,
        _: Rect,
        _: &GameState,
        _: Camera,
        _: Option<[f32; 3]>,
    ) -> GameInput {
        GameInput {
            actions: ["independent.choose".into()].into(),
            values: [("independent.slider".into(), serde_json::json!(0.75))].into(),
            action_values: [("independent.choice".into(), serde_json::json!(7))].into(),
            ..Default::default()
        }
    }

    fn synthetic_contribution(controls: Option<ControlsCallback>) -> UiContribution {
        UiContribution {
            id: "independent.presentation",
            input: synthetic_input,
            hud: legacy_restart,
            controls,
            controls_with_status: None,
            preview_aim: None,
            has_input: None,
            capture_input: None,
        }
    }

    #[test]
    fn controls_first_collection_calls_consuming_source_once() {
        fn consume_input(
            ui: &egui::Ui,
            _: &egui::Response,
            _: Rect,
            _: Camera,
            _: bool,
        ) -> GameInput {
            ui.ctx().data_mut(|data| {
                let id = egui::Id::new("independent.input.callback-count");
                let count = data.get_temp::<u32>(id).unwrap_or_default();
                data.insert_temp(id, count + 1);
            });
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::R)) {
                GameInput {
                    actions: ["independent.reset-level".into()].into(),
                    ..Default::default()
                }
            } else {
                GameInput::default()
            }
        }
        fn hud(_: &mut egui::Ui, _: Rect, _: &GameState, _: Camera, _: Option<[f32; 3]>) -> bool {
            false
        }
        // Both standalone and editor-focus policy use the production collector.
        for respect_editor_focus in [false, true] {
            let context = egui::Context::default();
            let mut accepted = GameInput::default();
            context
                .run_ui(
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
                            ui.allocate_exact_size(egui::vec2(640., 480.), egui::Sense::click());
                        if respect_editor_focus {
                            response.request_focus();
                        }
                        let state: GameState = serde_json::from_value(serde_json::json!({
                            "scene":"empty","time":0.,"tick":0,"entities":{}
                        }))
                        .unwrap();
                        let contribution = UiContribution {
                            input: consume_input,
                            hud,
                            ..synthetic_contribution(None)
                        };
                        accepted = collect_contribution_input(
                            Some(&contribution),
                            ui,
                            &response,
                            InputCollection {
                                id: Some(contribution.id),
                                rect,
                                state: &state,
                                camera: Camera::default(),
                                aim: None,
                                respect_editor_focus,
                                world_enabled: true,
                            },
                        )
                        .input;
                    },
                )
                .drop_without_applying_deltas();
            assert_eq!(
                context
                    .data(|data| data
                        .get_temp::<u32>(egui::Id::new("independent.input.callback-count"))),
                Some(1)
            );
            assert!(accepted.pressed("independent.reset-level"));
            assert!(
                !accepted.restart,
                "installed source owns R binding; host fallback must not replace it"
            );
        }
    }

    #[test]
    fn installed_source_can_bind_r_to_namespaced_action_without_host_restart() {
        fn input(ui: &egui::Ui, _: &egui::Response, _: Rect, _: Camera, _: bool) -> GameInput {
            if ui.input(|input| input.key_pressed(egui::Key::R)) {
                GameInput {
                    actions: ["independent.reset-level".into()].into(),
                    ..Default::default()
                }
            } else {
                GameInput::default()
            }
        }
        let context = egui::Context::default();
        let mut accepted = GameInput::default();
        context
            .run_ui(
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
                        ui.allocate_exact_size(egui::vec2(640., 480.), egui::Sense::click());
                    let contribution = UiContribution {
                        input,
                        ..synthetic_contribution(None)
                    };
                    accepted = read_contribution_input(
                        Some(&contribution),
                        ui,
                        &response,
                        rect,
                        Camera::default(),
                        false,
                    );
                },
            )
            .drop_without_applying_deltas();
        assert!(accepted.pressed("independent.reset-level"));
        assert!(!accepted.restart);
    }

    #[test]
    fn controls_callback_returns_full_packet_and_legacy_restart_adapter_remains() {
        let context = egui::Context::default();
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            let state: GameState = serde_json::from_value(serde_json::json!({
                "scene":"empty","time":0.,"tick":0,"entities":{}
            }))
            .unwrap();
            let rect = ui.max_rect();
            let controls = synthetic_contribution(Some(synthetic_controls)).draw_controls(
                ui,
                rect,
                &state,
                Camera::default(),
                None,
            );
            assert!(!controls.restart, "new controls replace the legacy HUD");
            assert!(controls.pressed("independent.choose"));
            assert_eq!(
                controls.values["independent.slider"],
                serde_json::json!(0.75)
            );
            assert_eq!(
                controls.action_values["independent.choice"],
                serde_json::json!(7)
            );
            let merged = merge_controls_input(
                GameInput {
                    actions: ["independent.world".into()].into(),
                    ..Default::default()
                },
                controls,
            );
            assert_eq!(merged.actions.len(), 2);
            let restart = synthetic_contribution(None).draw_controls(
                ui,
                rect,
                &state,
                Camera::default(),
                None,
            );
            let merged = merge_controls_input(merged, restart);
            assert_eq!(
                merged,
                GameInput {
                    restart: true,
                    ..Default::default()
                }
            );
        });
        output.drop_without_applying_deltas();
        assert!(has_input_for(
            None,
            &GameInput {
                values: [("independent.slider".into(), serde_json::json!(0.5))].into(),
                ..Default::default()
            }
        ));
    }

    #[test]
    fn hud_pointer_gesture_does_not_reach_world_callback() {
        let context = egui::Context::default();
        let mut packet = GameInput::default();
        for pressed in [false, true] {
            let output = context.run_ui(
                egui::RawInput {
                    focused: true,
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640., 480.),
                    )),
                    events: if pressed {
                        vec![
                            egui::Event::PointerMoved(egui::pos2(50., 50.)),
                            egui::Event::PointerButton {
                                pos: egui::pos2(50., 50.),
                                button: egui::PointerButton::Primary,
                                pressed: true,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(600., 400.), egui::Sense::click());
                    let _ = ui.put(
                        Rect::from_min_size(egui::pos2(20., 20.), egui::vec2(100., 60.)),
                        egui::Button::new("HUD control"),
                    );
                    packet = read_contribution_input(
                        Some(&synthetic_contribution(None)),
                        ui,
                        &response,
                        rect,
                        Camera::default(),
                        false,
                    );
                },
            );
            output.drop_without_applying_deltas();
            assert_eq!(packet.pressed("independent.world"), !pressed);
        }
    }

    #[cfg(feature = "arpg")]
    #[test]
    fn installed_input_predicate_can_distinguish_neutral_held_values() {
        let input = module_engine_arpg::ArpgInput::default().into_input();
        assert!(!input.values.is_empty());
        assert!(!has_input_for(Some("arpg.presentation"), &input));
    }

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
                controls: None,
                controls_with_status: None,
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
