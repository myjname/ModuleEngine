//! Game-owned presentation and physical-input adapter. No Editor or Player APIs.
use crate::{Phase, View};
use egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};
use module_engine_core::{Camera, GameInput, GameState, GameUiResponse, GameUiStatus};
use module_engine_game_kit::actions::{ActionState, PhysicalEvent};

const CREAM: Color32 = Color32::from_rgb(249, 244, 227);
const PAPER: Color32 = Color32::from_rgb(255, 252, 242);
const TEAL: Color32 = Color32::from_rgb(31, 91, 78);
const INK: Color32 = Color32::from_rgb(42, 60, 49);
const MUTED: Color32 = Color32::from_rgb(112, 122, 103);
const AMBER: Color32 = Color32::from_rgb(224, 167, 72);
const LINE: Color32 = Color32::from_rgb(218, 222, 198);

#[derive(Clone, Default)]
struct InputMemory {
    actions: ActionState,
    last_frame: Option<u64>,
}

#[derive(Clone)]
struct VolumeDraft {
    source: crate::Settings,
    value: crate::Settings,
    dirty: bool,
}

#[derive(Clone, Copy)]
struct Canvas {
    origin: Pos2,
    scale: f32,
}
impl Canvas {
    fn new(rect: Rect) -> Self {
        let scale = (rect.width() / 1280.).min(rect.height() / 720.).max(0.1);
        Self {
            origin: rect.center() - Vec2::new(640., 360.) * scale,
            scale,
        }
    }
    fn rect(self, x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect::from_min_size(
            self.origin + Vec2::new(x, y) * self.scale,
            Vec2::new(width, height) * self.scale,
        )
    }
    fn text(self, ui: &egui::Ui, x: f32, y: f32, size: f32, color: Color32, text: impl AsRef<str>) {
        ui.painter().text(
            self.origin + Vec2::new(x, y) * self.scale,
            Align2::LEFT_TOP,
            text.as_ref(),
            FontId::proportional(size * self.scale),
            color,
        );
    }
    fn card(self, ui: &egui::Ui, rect: Rect) {
        ui.painter().rect_filled(
            rect.translate(Vec2::new(0., 8.) * self.scale),
            22.,
            Color32::from_black_alpha(24),
        );
        ui.painter().rect_filled(rect, 22., PAPER);
        ui.painter().rect_stroke(
            rect,
            22.,
            Stroke::new(self.scale, LINE),
            egui::StrokeKind::Inside,
        );
    }
    fn button(self, ui: &mut egui::Ui, rect: Rect, label: &str, primary: bool) -> bool {
        self.button_response(ui, rect, label, primary).clicked()
    }
    fn button_response(
        self,
        ui: &mut egui::Ui,
        rect: Rect,
        label: &str,
        primary: bool,
    ) -> egui::Response {
        ui.put(
            rect,
            egui::Button::new(
                RichText::new(label)
                    .size(18. * self.scale)
                    .color(if primary { PAPER } else { TEAL }),
            )
            .fill(if primary { TEAL } else { CREAM })
            .stroke(Stroke::new(self.scale, if primary { TEAL } else { LINE }))
            .corner_radius(12.),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

fn press(input: &mut GameInput, action: &str) {
    if let Ok(frame) = crate::action_map().named_press(action) {
        input.actions.extend(frame.pressed);
    }
}
fn captured(input: &mut GameInput, key: &str, value: serde_json::Value) {
    input.values.insert(key.into(), value.clone());
    input.action_values.insert(key.into(), value);
}

/// One fresh physical press makes one named action. Focus loss resets held keys.
pub fn read_input_mode(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    _: Camera,
    respect_editor_focus: bool,
) -> GameInput {
    if response.hovered() && ui.input(|input| input.pointer.any_pressed()) {
        response.request_focus();
    }
    let focused = ui.input(|input| input.focused)
        && rect.is_positive()
        && (!respect_editor_focus || response.hovered() || response.has_focus())
        && (!ui.ctx().egui_wants_keyboard_input() || response.has_focus());
    let events = ui.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| {
                let egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } = event
                else {
                    return None;
                };
                if *pressed
                    && (modifiers.alt
                        || modifiers.mac_cmd
                        || ((modifiers.ctrl || modifiers.command) && *key != egui::Key::Z))
                {
                    return None;
                }
                let name = match key {
                    egui::Key::ArrowUp => "ArrowUp",
                    egui::Key::ArrowRight => "ArrowRight",
                    egui::Key::ArrowDown => "ArrowDown",
                    egui::Key::ArrowLeft => "ArrowLeft",
                    egui::Key::W => "W",
                    egui::Key::A => "A",
                    egui::Key::S => "S",
                    egui::Key::D => "D",
                    egui::Key::Z => "Z",
                    egui::Key::R => "R",
                    egui::Key::Escape => "Escape",
                    _ => return None,
                }
                .to_string();
                Some(if !pressed {
                    PhysicalEvent::Released(name)
                } else if *repeat {
                    PhysicalEvent::Repeated(name)
                } else {
                    PhysicalEvent::Pressed(name)
                })
            })
            .collect::<Vec<_>>()
    });
    let id = response.id.with("g1.action-state");
    let frame_number = ui.ctx().cumulative_frame_nr();
    let frame = ui.ctx().data_mut(|data| {
        let mut state = data.get_temp::<InputMemory>(id).unwrap_or_default();
        if state
            .last_frame
            .is_some_and(|last| frame_number > last.saturating_add(1))
        {
            state.actions.reset();
        }
        let frame = crate::action_map().evaluate(&mut state.actions, &events, focused);
        state.last_frame = Some(frame_number);
        data.insert_temp(id, state);
        frame
    });
    GameInput {
        actions: frame.pressed.into_iter().chain(frame.held).collect(),
        ..Default::default()
    }
}

pub fn has_input(input: &GameInput) -> bool {
    !input.actions.is_empty() || !input.values.is_empty()
}

/// Compatibility callback; the installed contribution uses the status-aware API.
pub fn draw_hud(_: &mut egui::Ui, _: Rect, _: &GameState, _: Camera, _: Option<[f32; 3]>) -> bool {
    false
}

pub fn draw_controls_with_status(
    ui: &mut egui::Ui,
    rect: Rect,
    state: &GameState,
    _: Camera,
    _: Option<[f32; 3]>,
    status: &GameUiStatus,
) -> GameUiResponse {
    let mut response = GameUiResponse::default();
    let canvas = Canvas::new(rect);
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("crate-garden-controls")
            .max_rect(rect),
        |ui| {
            ui.set_clip_rect(rect);
            let style = ui.style_mut();
            style.visuals.override_text_color = Some(INK);
            style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(232, 224, 197);
            style.visuals.widgets.active.bg_fill = AMBER;
            style.visuals.selection.bg_fill = TEAL;
            match crate::view(state) {
                Ok(view) => draw_view(ui, canvas, rect, &view, status, &mut response),
                Err(error) => {
                    let preparing = state.tick == 0
                        && !state.module_data.contains_key(crate::VIEW_KEY)
                        && status.errors.is_empty();
                    veil(ui, rect);
                    canvas.card(ui, canvas.rect(350., 220., 580., 290.));
                    canvas.text(
                        ui,
                        390.,
                        254.,
                        30.,
                        INK,
                        if preparing {
                            "Preparing the garden"
                        } else {
                            "The garden is not ready"
                        },
                    );
                    canvas.text(
                        ui,
                        390.,
                        304.,
                        17.,
                        MUTED,
                        if preparing {
                            "The first game step is preparing the controls."
                        } else if status.can_close_application {
                            "The game could not prepare its state."
                        } else {
                            "Stop Play and check the project diagnostics."
                        },
                    );
                    if !preparing {
                        ui.put(
                            canvas.rect(390., 337., 500., 52.),
                            egui::Label::new(error).wrap(),
                        );
                    }
                    if status.can_close_application
                        && canvas.button(ui, canvas.rect(390., 420., 200., 48.), "Quit", true)
                    {
                        response.close_application = true;
                    }
                }
            }
        },
    );
    response
}

fn veil(ui: &egui::Ui, rect: Rect) {
    ui.painter().rect_filled(
        rect,
        0.,
        Color32::from_rgba_unmultiplied(249, 244, 227, 224),
    );
}

fn service_error<'a>(status: &'a GameUiStatus, service: &str, field: &str) -> Option<&'a str> {
    status
        .services
        .get(service)?
        .get(field)?
        .as_str()
        .filter(|error| !error.trim().is_empty())
}

fn draw_view(
    ui: &mut egui::Ui,
    c: Canvas,
    rect: Rect,
    view: &View,
    status: &GameUiStatus,
    output: &mut GameUiResponse,
) {
    let confirmation_id = ui.id().with("reset-progress-confirmation");
    let confirmation = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(confirmation_id))
        .unwrap_or(false);
    if confirmation {
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            ui.ctx()
                .data_mut(|data| data.insert_temp(confirmation_id, false));
            return;
        }
        veil(ui, rect);
        c.card(ui, c.rect(350., 182., 580., 356.));
        c.text(ui, 394., 224., 32., INK, "A fresh start?");
        c.text(
            ui,
            394.,
            286.,
            17.,
            MUTED,
            "Unlocked gardens and best scores will be cleared.",
        );
        c.text(
            ui,
            394.,
            318.,
            17.,
            MUTED,
            "A damaged save is kept as a recovery copy.",
        );
        if c.button(ui, c.rect(394., 394., 234., 56.), "Keep my progress", false) {
            ui.ctx()
                .data_mut(|data| data.insert_temp(confirmation_id, false));
        }
        if c.button(ui, c.rect(650., 394., 234., 56.), "Reset progress", true) {
            press(&mut output.input, "g1.reset_progress");
            ui.ctx()
                .data_mut(|data| data.insert_temp(confirmation_id, false));
        }
        return;
    }
    if view.phase != Phase::Settings {
        ui.ctx()
            .data_mut(|data| data.remove::<VolumeDraft>(ui.id().with("g1.volume-draft")));
    }
    match view.phase {
        Phase::Menu => menu(ui, c, rect, view, status, output),
        Phase::LevelSelect => levels(ui, c, rect, view, &mut output.input),
        Phase::Settings => settings(ui, c, rect, view, confirmation_id, &mut output.input),
        Phase::Playing => playing(ui, c, view, &mut output.input),
        Phase::Paused => pause(ui, c, rect, &mut output.input),
        Phase::Victory => victory(ui, c, rect, view, &mut output.input),
        Phase::Complete => complete(ui, c, rect, view, &mut output.input),
    }
    let save_blocked = status
        .services
        .get("persistence.json")
        .and_then(|service| service.get("write_blocked"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    // These requested services publish current operational errors separately
    // from host faults. Never retain their historical Activity failures here.
    let save_error = service_error(status, "persistence.json", "last_error");
    let audio_error = service_error(status, "audio.wav", "output_error");
    if save_blocked
        || save_error.is_some()
        || audio_error.is_some()
        || view.warning.is_some()
        || !status.errors.is_empty()
    {
        let message = if save_blocked {
            "Progress is read-only. Open Settings to recover your save."
        } else if view.warning.is_some() {
            "Your garden needs attention."
        } else if save_error.is_some() && audio_error.is_some() {
            "Audio and progress need attention."
        } else if save_error.is_some() {
            "Progress could not be saved. Your game is still running."
        } else if audio_error.is_some() {
            "Sound is unavailable. Your game is still running."
        } else {
            "An audio or save service needs attention."
        };
        let alert = c.rect(
            300.,
            if view.phase == Phase::Playing {
                128.
            } else {
                10.
            },
            680.,
            48.,
        );
        let padding = 8. * c.scale;
        // Reserve the background before drawing text, then size it from the
        // completed layout so expanded diagnostics stay on the warning panel.
        let background = ui.painter().add(egui::Shape::Noop);
        let content = ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt("garden-service-message")
                .max_rect(Rect::from_min_max(
                    alert.min + Vec2::splat(padding),
                    egui::pos2(alert.right() - padding, rect.bottom() - padding),
                )),
            |ui| {
                ui.set_min_size(alert.size() - Vec2::splat(2. * padding));
                ui.set_clip_rect(rect);
                egui::CollapsingHeader::new(RichText::new(message).color(INK).size(14. * c.scale))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                                if let Some(warning) = &view.warning {
                                    ui.label(warning);
                                }
                                for error in &status.errors {
                                    ui.label(error);
                                }
                                if let Some(error) = save_error {
                                    ui.label(format!("Save: {error}"));
                                }
                                if let Some(error) = audio_error {
                                    ui.label(format!("Audio: {error}"));
                                }
                            });
                    });
            },
        );
        ui.painter().set(
            background,
            egui::Shape::rect_filled(
                alert
                    .union(content.response.rect.expand(padding))
                    .intersect(rect),
                10.,
                Color32::from_rgb(255, 225, 176),
            ),
        );
    }
}

fn menu(
    ui: &mut egui::Ui,
    c: Canvas,
    _rect: Rect,
    view: &View,
    status: &GameUiStatus,
    output: &mut GameUiResponse,
) {
    ui.painter().rect_filled(
        c.rect(0., 0., 470., 720.),
        0.,
        Color32::from_rgba_unmultiplied(249, 244, 227, 244),
    );
    c.text(ui, 64., 108., 13., TEAL, "A LITTLE SPACE TO THINK");
    c.text(ui, 60., 154., 66., INK, "Crate");
    c.text(ui, 60., 224., 66., TEAL, "Garden");
    ui.painter().line_segment(
        [
            c.rect(64., 309., 42., 0.).min,
            c.rect(64., 309., 42., 0.).max,
        ],
        Stroke::new(4. * c.scale, AMBER),
    );
    c.text(ui, 64., 330., 18., MUTED, "Find a home for every crate.");
    let (label, action) = if view.can_continue {
        ("Continue", "g1.continue")
    } else {
        ("Play", "g1.play")
    };
    if c.button(ui, c.rect(64., 395., 310., 58.), label, true) {
        press(&mut output.input, action);
    }
    if c.button(ui, c.rect(64., 467., 310., 50.), "Choose a garden", false) {
        press(&mut output.input, "g1.levels");
    }
    if c.button(ui, c.rect(64., 531., 310., 50.), "Settings", false) {
        press(&mut output.input, "g1.settings");
    }
    if view.can_continue && c.button(ui, c.rect(64., 595., 146., 44.), "New game", false) {
        press(&mut output.input, "g1.play");
    }
    if status.can_close_application
        && c.button(
            ui,
            c.rect(if view.can_continue { 228. } else { 64. }, 595., 146., 44.),
            "Quit",
            false,
        )
    {
        output.close_application = true;
    }
    if view.can_continue {
        c.text(
            ui,
            64.,
            648.,
            12.,
            MUTED,
            "New game keeps unlocked gardens and best scores.",
        );
    }
    c.text(
        ui,
        64.,
        674.,
        13.,
        MUTED,
        "Ten small gardens. One thoughtful move at a time.",
    );
}

fn levels(ui: &mut egui::Ui, c: Canvas, rect: Rect, view: &View, input: &mut GameInput) {
    veil(ui, rect);
    c.text(ui, 88., 74., 42., INK, "Choose your garden");
    c.text(
        ui,
        90.,
        132.,
        17.,
        MUTED,
        "Each little puzzle opens the next. Your best score stays with you.",
    );
    for level in &view.levels {
        let x = 90. + (level.index % 5) as f32 * 222.;
        let y = 202. + (level.index / 5) as f32 * 170.;
        let card = c.rect(x, y, 202., 146.);
        ui.painter().rect_filled(
            card.translate(Vec2::new(0., 5.) * c.scale),
            16.,
            Color32::from_black_alpha(12),
        );
        let clicked = ui
            .add_enabled_ui(level.unlocked, |ui| c.button(ui, card, "", false))
            .inner;
        c.text(
            ui,
            x + 18.,
            y + 15.,
            36.,
            if level.unlocked { TEAL } else { MUTED },
            format!("{:02}", level.index + 1),
        );
        c.text(ui, x + 18., y + 68., 16., INK, &level.name);
        c.text(
            ui,
            x + 18.,
            y + 108.,
            13.,
            MUTED,
            if !level.unlocked {
                "Finish the previous garden".into()
            } else {
                level.best_moves.map_or("Ready to explore".into(), |moves| {
                    format!("Best · {moves} moves")
                })
            },
        );
        if clicked {
            press(input, "g1.select_level");
            captured(input, "g1.level_id", serde_json::json!(level.id));
        }
    }
    if c.button(ui, c.rect(90., 606., 220., 52.), "Back", false) {
        press(input, "g1.back");
    }
}

fn settings(
    ui: &mut egui::Ui,
    c: Canvas,
    rect: Rect,
    view: &View,
    confirmation_id: egui::Id,
    input: &mut GameInput,
) {
    veil(ui, rect);
    c.card(ui, c.rect(330., 112., 620., 510.));
    c.text(ui, 378., 151., 38., INK, "Make yourself at home");
    c.text(
        ui,
        380.,
        212.,
        17.,
        MUTED,
        "A quieter garden, or a little more music.",
    );
    let volume_id = ui.id().with("g1.volume-draft");
    let mut volume = ui
        .ctx()
        .data(|data| data.get_temp::<VolumeDraft>(volume_id))
        .unwrap_or(VolumeDraft {
            source: view.settings,
            value: view.settings,
            dirty: false,
        });
    if !volume.dirty && volume.source != view.settings {
        volume = VolumeDraft {
            source: view.settings,
            value: view.settings,
            dirty: false,
        };
    }
    let mut music = volume.value.music_volume;
    let mut sfx = volume.value.sfx_volume;
    c.text(ui, 380., 272., 19., INK, "Music");
    c.text(ui, 380., 358., 19., INK, "Sound effects");
    ui.spacing_mut().slider_width = 430. * c.scale;
    let music_response = ui.put(
        c.rect(380., 305., 430., 32.),
        egui::Slider::new(&mut music, 0.0..=1.0).show_value(false),
    );
    let sfx_response = ui.put(
        c.rect(380., 391., 430., 32.),
        egui::Slider::new(&mut sfx, 0.0..=1.0).show_value(false),
    );
    c.text(
        ui,
        838.,
        305.,
        17.,
        TEAL,
        format!("{}%", (music * 100.).round() as u32),
    );
    c.text(
        ui,
        838.,
        391.,
        17.,
        TEAL,
        format!("{}%", (sfx * 100.).round() as u32),
    );
    let changed = music_response.changed() || sfx_response.changed();
    volume.dirty |= changed;
    volume.value = crate::Settings {
        music_volume: music,
        sfx_volume: sfx,
    };
    let done = c.button(ui, c.rect(380., 491., 230., 54.), "Done", true);
    // Persist a completed slider gesture, not one file write for every pointer
    // sample. Keyboard slider edits commit immediately as discrete gestures.
    let commit = volume.dirty
        && (done
            || ui.input(|input| input.pointer.any_released())
            || music_response.drag_stopped()
            || sfx_response.drag_stopped()
            || (changed && !ui.input(|input| input.pointer.any_down())));
    if commit {
        press(input, "g1.set_volumes");
        captured(input, "g1.music_volume", serde_json::json!(music));
        captured(input, "g1.sfx_volume", serde_json::json!(sfx));
        volume.dirty = false;
    }
    ui.ctx()
        .data_mut(|data| data.insert_temp(volume_id, volume));
    if done {
        press(input, "g1.back");
    }
    if c.button(ui, c.rect(638., 491., 260., 54.), "Reset progress", false) {
        ui.ctx()
            .data_mut(|data| data.insert_temp(confirmation_id, true));
    }
    c.text(
        ui,
        380.,
        573.,
        13.,
        MUTED,
        "Music and sound effects have independent volume controls.",
    );
}

fn playing(ui: &mut egui::Ui, c: Canvas, view: &View, input: &mut GameInput) {
    c.card(ui, c.rect(28., 24., 1224., 92.));
    let level = view.levels.get(view.current_level);
    c.text(
        ui,
        54.,
        43.,
        12.,
        MUTED,
        format!(
            "GARDEN {:02} / {:02}",
            view.current_level + 1,
            view.levels.len()
        ),
    );
    c.text(
        ui,
        54.,
        66.,
        25.,
        TEAL,
        level.map_or("Your garden", |level| level.name.as_str()),
    );
    c.text(ui, 452., 42., 12., MUTED, "MOVES");
    c.text(ui, 452., 64., 30., INK, view.moves.to_string());
    c.text(ui, 574., 42., 12., MUTED, "BEST");
    c.text(
        ui,
        574.,
        64.,
        30.,
        INK,
        level
            .and_then(|level| level.best_moves)
            .map_or("—".into(), |best| best.to_string()),
    );
    if ui
        .add_enabled_ui(view.can_undo, |ui| {
            c.button(ui, c.rect(808., 43., 120., 48.), "Undo · Z", false)
        })
        .inner
    {
        press(input, "g1.undo");
    }
    if c.button(ui, c.rect(942., 43., 136., 48.), "Restart · R", false) {
        press(input, "g1.restart");
    }
    if c.button(ui, c.rect(1092., 43., 136., 48.), "Pause", true) {
        press(input, "g1.pause");
    }
    let help = c.rect(34., 642., 730., 48.);
    ui.painter().rect_filled(
        help,
        12.,
        Color32::from_rgba_unmultiplied(255, 252, 242, 238),
    );
    c.text(
        ui,
        54.,
        658.,
        14.,
        INK,
        "WASD / arrows to move · Push each crate onto a flower · Esc to pause",
    );
    for (x, y, direction, label, action) in [
        (1106., 514., Vec2::new(0., -1.), "Move up", "g1.north"),
        (1164., 572., Vec2::new(1., 0.), "Move right", "g1.east"),
        (1106., 630., Vec2::new(0., 1.), "Move down", "g1.south"),
        (1048., 572., Vec2::new(-1., 0.), "Move left", "g1.west"),
    ] {
        let button = c.rect(x, y, 52., 52.);
        let response = c.button_response(ui, button, "", true);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        if response.clicked() {
            press(input, action);
        }
        // Direction controls must not depend on the platform font's arrow glyphs.
        ui.painter().arrow(
            button.center() - direction * (10. * c.scale),
            direction * (20. * c.scale),
            Stroke::new(2.5 * c.scale, PAPER),
        );
    }
}

fn pause(ui: &mut egui::Ui, c: Canvas, rect: Rect, input: &mut GameInput) {
    veil(ui, rect);
    c.card(ui, c.rect(404., 130., 472., 474.));
    c.text(ui, 454., 173., 36., INK, "A little breather.");
    c.text(
        ui,
        454.,
        230.,
        17.,
        MUTED,
        "Your garden will be right here.",
    );
    for (y, label, action, primary) in [
        (286., "Resume", "g1.resume", true),
        (358., "Settings", "g1.settings", false),
        (430., "Back to menu", "g1.menu", false),
    ] {
        if c.button(ui, c.rect(454., y, 372., 56.), label, primary) {
            press(input, action);
        }
    }
    c.text(
        ui,
        454.,
        535.,
        14.,
        MUTED,
        "Esc to return · Every move can be undone",
    );
}

fn victory(ui: &mut egui::Ui, c: Canvas, rect: Rect, view: &View, input: &mut GameInput) {
    veil(ui, rect);
    c.card(ui, c.rect(350., 128., 580., 476.));
    c.text(ui, 398., 170., 13., TEAL, "EVERY CRATE FOUND ITS PLACE");
    c.text(ui, 394., 210., 40., INK, "Garden complete!");
    c.text(
        ui,
        398.,
        281.,
        22.,
        TEAL,
        format!(
            "{} thoughtful {}",
            view.moves,
            if view.moves == 1 { "move" } else { "moves" }
        ),
    );
    c.text(ui, 398., 323., 17., MUTED, "Small steps. A lovely result.");
    let final_level = view.current_level + 1 >= view.levels.len();
    if c.button(
        ui,
        c.rect(398., 384., 484., 60.),
        if final_level {
            "Finish the journey"
        } else {
            "Next garden"
        },
        true,
    ) {
        press(input, "g1.next");
    }
    if c.button(ui, c.rect(398., 463., 230., 50.), "Play again", false) {
        press(input, "g1.restart");
    }
    if c.button(ui, c.rect(650., 463., 232., 50.), "Choose a garden", false) {
        press(input, "g1.levels");
    }
}

fn complete(ui: &mut egui::Ui, c: Canvas, rect: Rect, view: &View, input: &mut GameInput) {
    veil(ui, rect);
    c.card(ui, c.rect(298., 132., 684., 468.));
    c.text(
        ui,
        350.,
        178.,
        13.,
        TEAL,
        "A LITTLE PATIENCE GOES A LONG WAY",
    );
    c.text(ui, 346., 220., 42., INK, "A garden well tended.");
    c.text(
        ui,
        350.,
        290.,
        20.,
        MUTED,
        format!("You've completed all {} gardens.", view.levels.len()),
    );
    c.text(
        ui,
        350.,
        330.,
        18.,
        MUTED,
        "Come back whenever you need a little space to think.",
    );
    if c.button(ui, c.rect(350., 411., 280., 58.), "Visit the gardens", true) {
        press(input, "g1.levels");
    }
    if c.button(ui, c.rect(654., 411., 274., 58.), "Back to menu", false) {
        press(input, "g1.menu");
    }
    c.text(
        ui,
        350.,
        532.,
        15.,
        TEAL,
        "Thank you for playing Crate Garden.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(phase: Phase) -> View {
        View {
            phase,
            current_level: 0,
            moves: 3,
            can_undo: true,
            can_continue: false,
            settings: Default::default(),
            warning: None,
            board: crate::BoardView {
                width: 7,
                height: 5,
                walls: vec![],
                goals: vec![],
                pawn: crate::Cell::new(3, 3),
                crates: vec![],
            },
            levels: (0..10)
                .map(|index| crate::LevelSummary {
                    id: format!("level{:02}", index + 1),
                    name: format!("Garden {}", index + 1),
                    index,
                    unlocked: index == 0,
                    best_moves: None,
                })
                .collect(),
        }
    }
    fn frame(
        context: &egui::Context,
        view: &View,
        status: &GameUiStatus,
        events: Vec<egui::Event>,
    ) -> GameUiResponse {
        let mut result = GameUiResponse::default();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280., 720.));
        context
            .run_ui(
                egui::RawInput {
                    focused: true,
                    screen_rect: Some(rect),
                    events,
                    ..Default::default()
                },
                |ui| {
                    draw_view(ui, Canvas::new(rect), rect, view, status, &mut result);
                },
            )
            .drop_without_applying_deltas();
        result
    }

    #[test]
    fn current_save_and_audio_errors_render_details_and_clear_after_repair() {
        fn text_in(shape: &egui::Shape, text: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(shape) => text.push(shape.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text_in(shape, text);
                    }
                }
                _ => {}
            }
        }
        fn render(
            context: &egui::Context,
            status: &GameUiStatus,
            time: f64,
            events: Vec<egui::Event>,
        ) -> Vec<String> {
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280., 720.));
            let output = context.run_ui(
                egui::RawInput {
                    focused: true,
                    screen_rect: Some(rect),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    draw_view(
                        ui,
                        Canvas::new(rect),
                        rect,
                        &view(Phase::Playing),
                        status,
                        &mut GameUiResponse::default(),
                    );
                },
            );
            let mut text = vec![];
            for shape in &output.shapes {
                text_in(&shape.shape, &mut text);
            }
            output.drop_without_applying_deltas();
            text
        }
        for (services, headline, detail) in [
            (
                serde_json::json!({"persistence.json":{"write_blocked":false,"last_error":"atomic save publication failed"}}),
                "Progress could not be saved. Your game is still running.",
                "Save: atomic save publication failed",
            ),
            (
                serde_json::json!({"audio.wav":{"output_error":"audio output device was disconnected"}}),
                "Sound is unavailable. Your game is still running.",
                "Audio: audio output device was disconnected",
            ),
        ] {
            let context = egui::Context::default();
            let failed = GameUiStatus {
                services,
                ..Default::default()
            };
            assert!(failed.errors.is_empty());
            assert!(
                render(&context, &failed, 0., vec![])
                    .iter()
                    .any(|text| text == headline)
            );
            let pointer = egui::pos2(450., 148.);
            let _ = render(
                &context,
                &failed,
                0.1,
                vec![
                    egui::Event::PointerMoved(pointer),
                    egui::Event::PointerButton {
                        pos: pointer,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            let _ = render(
                &context,
                &failed,
                0.2,
                vec![egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(
                render(&context, &failed, 1., vec![])
                    .iter()
                    .any(|text| text == detail),
                "expanded current diagnostic was not painted"
            );
            let repaired = GameUiStatus {
                services: serde_json::json!({
                    "persistence.json":{"write_blocked":false,"last_error":null},
                    "audio.wav":{"output_error":null}
                }),
                ..Default::default()
            };
            let after = render(&context, &repaired, 2., vec![]);
            assert!(
                !after.iter().any(|text| text == headline || text == detail),
                "repaired status retained historical service diagnostics"
            );
        }
    }
    #[test]
    fn expanded_service_diagnostics_stay_inside_warning_panel_and_viewport() {
        let save_error = format!(
            "Invalid persistence save 'E:\\ModuleEngine\\outputs\\{}profile\\slot.json': key must be a string; original preserved",
            "verification-with-a-long-native-directory\\".repeat(8),
        );
        let audio_error = "audio output device was disconnected";
        let status = GameUiStatus {
            services: serde_json::json!({
                "persistence.json":{"write_blocked":true,"last_error":save_error},
                "audio.wav":{"output_error":audio_error}
            }),
            ..Default::default()
        };
        for size in [Vec2::new(1280., 720.), Vec2::new(800., 600.)] {
            for phase in [Phase::Menu, Phase::Playing] {
                let context = egui::Context::default();
                let rect = Rect::from_min_size(Pos2::ZERO, size);
                let canvas = Canvas::new(rect);
                let render = |time, events| {
                    context.run_ui(
                        egui::RawInput {
                            focused: true,
                            screen_rect: Some(rect),
                            time: Some(time),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            draw_view(
                                ui,
                                canvas,
                                rect,
                                &view(phase),
                                &status,
                                &mut GameUiResponse::default(),
                            );
                        },
                    )
                };
                let y = if phase == Phase::Playing { 148. } else { 30. };
                let at = canvas.rect(450., y, 0., 0.).min;
                render(0., vec![]).drop_without_applying_deltas();
                render(
                    0.1,
                    vec![
                        egui::Event::PointerMoved(at),
                        egui::Event::PointerButton {
                            pos: at,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                )
                .drop_without_applying_deltas();
                render(
                    0.2,
                    vec![egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                )
                .drop_without_applying_deltas();
                let output = render(1., vec![]);
                let panel = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(panel)
                            if panel.fill == Color32::from_rgb(255, 225, 176) =>
                        {
                            Some(panel.rect)
                        }
                        _ => None,
                    })
                    .expect("warning panel was not painted");
                assert!(rect.contains_rect(panel));
                for expected in [
                    format!("Save: {save_error}"),
                    format!("Audio: {audio_error}"),
                ] {
                    let diagnostic = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == expected => Some((
                                Rect::from_min_size(text.pos, text.galley.size()),
                                shape.clip_rect,
                            )),
                            _ => None,
                        })
                        .expect("expanded diagnostic was not painted");
                    let visible = diagnostic.0.intersect(diagnostic.1);
                    assert!(visible.is_positive());
                    assert!(panel.contains_rect(visible), "diagnostic escaped its panel");
                    assert!(
                        rect.contains_rect(visible),
                        "diagnostic escaped the viewport"
                    );
                }
                output.drop_without_applying_deltas();
            }
        }
    }

    fn click(
        context: &egui::Context,
        view: &View,
        status: &GameUiStatus,
        at: Pos2,
    ) -> GameUiResponse {
        let _ = frame(context, view, status, vec![]);
        let _ = frame(
            context,
            view,
            status,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            context,
            view,
            status,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
    }

    #[test]
    fn main_menu_buttons_and_close_capability_use_real_widget_clicks() {
        let context = egui::Context::default();
        let mut menu = view(Phase::Menu);
        let status = GameUiStatus::default();
        assert!(
            click(&context, &menu, &status, egui::pos2(210., 420.))
                .input
                .pressed("g1.play")
        );
        menu.can_continue = true;
        assert!(
            click(&context, &menu, &status, egui::pos2(210., 420.))
                .input
                .pressed("g1.continue")
        );
        assert!(
            click(&context, &menu, &status, egui::pos2(120., 614.))
                .input
                .pressed("g1.play")
        );
        assert!(!click(&context, &menu, &status, egui::pos2(300., 614.)).close_application);
        let standalone = GameUiStatus {
            can_close_application: true,
            ..Default::default()
        };
        assert!(click(&context, &menu, &standalone, egui::pos2(300., 614.)).close_application);
    }

    #[test]
    fn unavailable_view_retains_a_standalone_exit_without_closing_editor() {
        let state: GameState = serde_json::from_value(serde_json::json!({
            "scene":"empty","time":0.,"tick":0,"entities":{}
        }))
        .unwrap();
        for can_close_application in [false, true] {
            let context = egui::Context::default();
            let status = GameUiStatus {
                can_close_application,
                ..Default::default()
            };
            let mut result = GameUiResponse::default();
            let at = egui::pos2(490., 444.);
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280., 720.));
            for events in [
                vec![],
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            ] {
                context
                    .run_ui(
                        egui::RawInput {
                            focused: true,
                            screen_rect: Some(rect),
                            events,
                            ..Default::default()
                        },
                        |ui| {
                            result = draw_controls_with_status(
                                ui,
                                rect,
                                &state,
                                Camera::default(),
                                None,
                                &status,
                            );
                        },
                    )
                    .drop_without_applying_deltas();
            }
            assert_eq!(result.close_application, can_close_application);
            assert!(result.input.actions.is_empty());
        }
    }

    #[test]
    fn game_pane_initial_state_uses_loading_without_masking_corrupt_or_failed_live_state() {
        use module_engine_core::{GameSession, WorldKind};
        let registry = crate::registry();
        let project = crate::project().prepare_runtime(&registry).unwrap();
        let mut session =
            GameSession::new_with_registry(project, WorldKind::Ecs, registry).unwrap();
        let initial = session.state().clone();
        assert_eq!(initial.tick, 0);
        assert!(!initial.module_data.contains_key(crate::VIEW_KEY));
        let context = egui::Context::default();
        let draw = |state: &GameState, status: &GameUiStatus| {
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280., 720.));
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    draw_controls_with_status(ui, rect, state, Camera::default(), None, status);
                },
            );
            let text = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            output.drop_without_applying_deltas();
            text
        };
        let text = draw(&initial, &GameUiStatus::default());
        assert!(text.iter().any(|text| text == "Preparing the garden"));
        assert!(!text.iter().any(|text| text == "The garden is not ready"));
        assert_eq!(session.state(), &initial);
        for mode in 0..3 {
            let mut state = initial.clone();
            let mut status = GameUiStatus::default();
            match mode {
                0 => {
                    state
                        .module_data
                        .insert(crate::VIEW_KEY.into(), serde_json::Value::Null);
                }
                1 => status.errors.push("Actual host failure".into()),
                _ => state.tick = 1,
            }
            let text = draw(&state, &status);
            assert!(text.iter().any(|text| text == "The garden is not ready"));
            assert!(!text.iter().any(|text| text == "Preparing the garden"));
        }
        session.step(1. / 60., &GameInput::default()).unwrap();
        assert!(
            draw(session.state(), &GameUiStatus::default())
                .iter()
                .any(|text| text == "A LITTLE SPACE TO THINK")
        );
    }

    #[test]
    fn level_lock_and_selection_keep_the_selected_id_in_action_context() {
        let context = egui::Context::default();
        let levels = view(Phase::LevelSelect);
        let status = GameUiStatus::default();
        assert!(
            click(&context, &levels, &status, egui::pos2(400., 270.))
                .input
                .actions
                .is_empty()
        );
        let selected = click(&context, &levels, &status, egui::pos2(180., 270.));
        assert!(selected.input.pressed("g1.select_level"));
        assert_eq!(
            selected.input.action_values["g1.level_id"],
            serde_json::json!("level01")
        );
    }

    #[test]
    fn board_buttons_emit_named_actions_and_level_restart_never_restarts_host() {
        let context = egui::Context::default();
        let playing = view(Phase::Playing);
        let status = GameUiStatus::default();
        for (at, action) in [
            (egui::pos2(1132., 540.), "g1.north"),
            (egui::pos2(1190., 598.), "g1.east"),
            (egui::pos2(1132., 656.), "g1.south"),
            (egui::pos2(1074., 598.), "g1.west"),
        ] {
            let input = click(&context, &playing, &status, at).input;
            assert!(input.pressed(action));
            assert_eq!(input.actions.len(), 1);
        }
        let restart = click(&context, &playing, &status, egui::pos2(1000., 65.));
        assert!(restart.input.pressed("g1.restart"));
        assert!(!restart.input.restart);
        let mut no_undo = playing.clone();
        no_undo.can_undo = false;
        assert!(
            !click(&context, &no_undo, &status, egui::pos2(860., 65.))
                .input
                .pressed("g1.undo")
        );
        assert!(
            click(&context, &playing, &status, egui::pos2(860., 65.))
                .input
                .pressed("g1.undo")
        );
    }

    #[test]
    fn direction_icons_are_visible_vector_arrows_at_multiple_canvas_scales() {
        fn segments(shape: &egui::Shape, lines: &mut Vec<[Pos2; 2]>) {
            match shape {
                egui::Shape::LineSegment { points, .. } => lines.push(*points),
                egui::Shape::Path(path) => {
                    lines.extend(path.points.windows(2).map(|points| [points[0], points[1]]));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        segments(shape, lines);
                    }
                }
                _ => {}
            }
        }
        for size in [Vec2::new(1280., 720.), Vec2::new(800., 600.)] {
            let context = egui::Context::default();
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            let canvas = Canvas::new(rect);
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect),
                    ..Default::default()
                },
                |ui| {
                    playing(ui, canvas, &view(Phase::Playing), &mut GameInput::default());
                },
            );
            let mut lines = vec![];
            for shape in &output.shapes {
                segments(&shape.shape, &mut lines);
            }
            for (x, y, direction) in [
                (1106., 514., Vec2::new(0., -1.)),
                (1164., 572., Vec2::new(1., 0.)),
                (1106., 630., Vec2::new(0., 1.)),
                (1048., 572., Vec2::new(-1., 0.)),
            ] {
                let button = canvas.rect(x, y, 52., 52.);
                let icon = lines
                    .iter()
                    .filter(|line| line.iter().all(|point| button.contains(*point)))
                    .collect::<Vec<_>>();
                assert_eq!(icon.len(), 3, "each direction needs a shaft and two wings");
                let points = icon
                    .iter()
                    .flat_map(|line| line.iter().copied())
                    .collect::<Vec<_>>();
                let tip = points
                    .iter()
                    .max_by(|a, b| {
                        ((**a - button.center()).dot(direction))
                            .total_cmp(&((**b - button.center()).dot(direction)))
                    })
                    .unwrap();
                assert!((*tip - button.center()).dot(direction) > 5. * canvas.scale);
                assert_eq!(points.iter().filter(|point| *point == tip).count(), 3);
                assert!(points.iter().any(|point| {
                    (*point - button.center()).dot(direction) < -5. * canvas.scale
                }));
            }
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn victory_move_count_uses_singular_only_for_one() {
        for (moves, expected) in [
            (0, "0 thoughtful moves"),
            (1, "1 thoughtful move"),
            (2, "2 thoughtful moves"),
        ] {
            let context = egui::Context::default();
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280., 720.));
            let mut view = view(Phase::Victory);
            view.moves = moves;
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                victory(
                    ui,
                    Canvas::new(rect),
                    rect,
                    &view,
                    &mut GameInput::default(),
                );
            });
            assert!(output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == expected)
            }));
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn progress_reset_requires_a_separate_confirmation_click() {
        let context = egui::Context::default();
        let settings = view(Phase::Settings);
        let status = GameUiStatus::default();
        assert!(
            click(&context, &settings, &status, egui::pos2(760., 515.))
                .input
                .actions
                .is_empty()
        );
        assert!(
            click(&context, &settings, &status, egui::pos2(760., 420.))
                .input
                .pressed("g1.reset_progress")
        );
    }

    #[test]
    fn volume_drag_commits_once_on_release_with_independent_captured_channels() {
        let context = egui::Context::default();
        let settings = view(Phase::Settings);
        let status = GameUiStatus::default();
        let _ = frame(&context, &settings, &status, vec![]);
        let at = egui::pos2(740., 315.);
        let down = frame(
            &context,
            &settings,
            &status,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(down.input.actions.is_empty());
        assert!(
            frame(
                &context,
                &settings,
                &status,
                vec![egui::Event::PointerMoved(egui::pos2(760., 315.))]
            )
            .input
            .actions
            .is_empty()
        );
        let released = frame(
            &context,
            &settings,
            &status,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(760., 315.),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(released.input.pressed("g1.set_volumes"));
        assert!(
            released.input.action_values["g1.music_volume"]
                .as_f64()
                .unwrap()
                > 0.6
        );
        assert_eq!(
            released.input.action_values["g1.sfx_volume"],
            serde_json::json!(settings.settings.sfx_volume)
        );
        assert!(
            frame(&context, &settings, &status, vec![])
                .input
                .actions
                .is_empty()
        );
    }

    #[test]
    fn pause_victory_and_ending_have_complete_action_routes() {
        let context = egui::Context::default();
        let status = GameUiStatus::default();
        assert!(
            click(
                &context,
                &view(Phase::Paused),
                &status,
                egui::pos2(630., 312.)
            )
            .input
            .pressed("g1.resume")
        );
        assert!(
            click(
                &context,
                &view(Phase::Victory),
                &status,
                egui::pos2(630., 410.)
            )
            .input
            .pressed("g1.next")
        );
        assert!(
            click(
                &context,
                &view(Phase::Complete),
                &status,
                egui::pos2(490., 435.)
            )
            .input
            .pressed("g1.levels")
        );
    }

    fn keyboard(context: &egui::Context, events: Vec<egui::Event>, focused: bool) -> GameInput {
        let mut result = GameInput::default();
        context
            .run_ui(
                egui::RawInput {
                    focused,
                    events,
                    ..Default::default()
                },
                |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(800., 600.), egui::Sense::click());
                    response.request_focus();
                    result = read_input_mode(ui, &response, rect, Camera::default(), true);
                },
            )
            .drop_without_applying_deltas();
        result
    }
    fn key(key: egui::Key, pressed: bool, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat,
            modifiers: egui::Modifiers::NONE,
        }
    }
    #[test]
    fn physical_press_repeat_release_and_focus_loss_follow_action_map() {
        let context = egui::Context::default();
        assert!(keyboard(&context, vec![key(egui::Key::W, true, false)], true).pressed("g1.north"));
        assert!(
            keyboard(&context, vec![key(egui::Key::W, true, true)], true)
                .actions
                .is_empty()
        );
        assert!(keyboard(&context, vec![], true).actions.is_empty());
        let _ = keyboard(&context, vec![key(egui::Key::W, false, false)], true);
        assert!(keyboard(&context, vec![key(egui::Key::W, true, false)], true).pressed("g1.north"));
        assert!(keyboard(&context, vec![], false).actions.is_empty());
        let restart = keyboard(&context, vec![key(egui::Key::R, true, false)], true);
        assert!(restart.pressed("g1.restart"));
        assert!(!restart.restart);
        let modified = |key| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            },
        };
        let shortcuts = keyboard(
            &context,
            vec![modified(egui::Key::Z), modified(egui::Key::S)],
            true,
        );
        assert!(shortcuts.pressed("g1.undo"));
        assert!(
            !shortcuts.pressed("g1.south"),
            "Editor save shortcut must not also move the gardener"
        );
    }
}
