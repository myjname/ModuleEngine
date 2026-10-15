//! Standalone game presentation. Shares gameplay and renderer with editor Play.
use crate::game_host::{GameHost, aim_at};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use module_engine_core::{Camera, GameInput, GameStatus, GraphDocument, WorldKind};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

const BENCHMARK_WARMUP: usize = 30;
const BENCHMARK_FRAMES: usize = 120;
const FRAME_BUDGET_MS: f64 = 1000. / 60.;

pub fn run(
    document: GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
) -> Result<(), String> {
    let mut host = GameHost::new(document, root, compile_root, kind)?;
    // eframe's diagnostic capture exits after two UI frames, which can both
    // precede the first fixed tick. Advance the real simulation once so a
    // capture can verify the HUD for an authored combat fixture.
    if std::env::var_os("EFRAME_SCREENSHOT_TO").is_some() {
        let mut input = GameInput::default();
        if std::env::var_os("MODULE_ENGINE_CAPTURE_ATTACK").is_some()
            && let Some(player) = host.session.state().player()
        {
            let p = player.object.transform.translation;
            if let Some(target) = host
                .session
                .state()
                .entities
                .values()
                .filter(|e| {
                    e.components
                        .get("actor")
                        .and_then(|a| a.get("team"))
                        .and_then(serde_json::Value::as_str)
                        == Some("enemy")
                })
                .min_by(|a, b| {
                    let distance = |v: [f32; 3]| (v[0] - p[0]).powi(2) + (v[2] - p[2]).powi(2);
                    distance(a.object.transform.translation)
                        .total_cmp(&distance(b.object.transform.translation))
                })
            {
                input.attack = true;
                input.aim_world = target.object.transform.translation;
            }
        }
        host.step(1. / 60., &input)?;
        if input.attack {
            input.attack = false;
            for _ in 0..2 {
                host.step(1. / 60., &input)?;
            }
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280., 720.])
            .with_min_inner_size([800., 450.]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "ModuleEngine / Two Arenas",
        options,
        Box::new(move |cc| {
            crate::app::configure_style(&cc.egui_ctx);
            Ok(Box::new(Player {
                host,
                texture: None,
                last: Instant::now(),
                error: None,
                frames: 0,
                last_ui_frame: None,
                benchmark: None,
            }))
        }),
    )
    .map_err(|e| e.to_string())
}

/// Measure the shipping Player path including framework texture upload and
/// presentation cadence. The UI closes itself after 30 + 120 completed frames.
pub fn run_benchmark(
    document: GraphDocument,
    root: &Path,
    compile_root: Option<&Path>,
    kind: WorldKind,
    folder: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let folder = folder.canonicalize().map_err(|e| e.to_string())?;
    let host = GameHost::new(document, root, compile_root, kind)?;
    let outcome = Arc::new(Mutex::new(None));
    let returned = Arc::clone(&outcome);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1920., 1080.])
            .with_min_inner_size([640., 360.])
            .with_decorations(false)
            .with_position([0., 0.]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "ModuleEngine / Native Player benchmark",
        options,
        Box::new(move |cc| {
            crate::app::configure_style(&cc.egui_ctx);
            let window = cc.winit_window().cloned();
            let physical_size: Box<dyn Fn() -> Option<[u32; 2]>> = match window {
                Some(window) => {
                    // NativeOptions sizes are egui points; request physical
                    // 1080p after the actual monitor DPI becomes available.
                    let scale = window.scale_factor() as f32;
                    cc.egui_ctx
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(
                            1920. / scale,
                            1080. / scale,
                        )));
                    Box::new(move || Some(window.inner_size().into()))
                }
                None => Box::new(|| None),
            };
            let monitor = cc
                .winit_window()
                .and_then(|window| window.current_monitor());
            let monitor_size = monitor
                .as_ref()
                .map(|monitor| <[u32; 2]>::from(monitor.size()));
            let refresh_millihertz = monitor
                .as_ref()
                .and_then(|monitor| monitor.refresh_rate_millihertz());
            Ok(Box::new(Player {
                host,
                texture: None,
                last: Instant::now(),
                error: None,
                frames: 0,
                last_ui_frame: None,
                benchmark: Some(NativeBenchmark {
                    folder,
                    outcome,
                    physical_size,
                    monitor_size,
                    refresh_millihertz,
                    started: Instant::now(),
                    previous: None,
                    completed: 0,
                    samples: vec![],
                    finished: false,
                    gl_vendor: cc.gl.as_ref().map(|gl| {
                        use eframe::glow::HasContext;
                        unsafe { gl.get_parameter_string(eframe::glow::VENDOR) }
                    }),
                    gl_renderer: cc.gl.as_ref().map(|gl| {
                        use eframe::glow::HasContext;
                        unsafe { gl.get_parameter_string(eframe::glow::RENDERER) }
                    }),
                }),
            }))
        }),
    )
    .map_err(|e| e.to_string())?;
    returned
        .lock()
        .map_err(|_| "Benchmark result lock poisoned".to_string())?
        .take()
        .unwrap_or_else(|| Err("Native benchmark closed without a report".into()))
}

struct Player {
    host: GameHost,
    texture: Option<egui::TextureHandle>,
    last: Instant,
    error: Option<String>,
    frames: u64,
    last_ui_frame: Option<u64>,
    benchmark: Option<NativeBenchmark>,
}
impl eframe::App for Player {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(benchmark) = &mut self.benchmark {
            if !benchmark.finished && benchmark.started.elapsed().as_secs() >= 45 {
                benchmark.finish(&self.host, self.error.as_deref(), Some("45-second watchdog: window may be hidden, occluded, minimized, or too slow"));
            }
            if benchmark.finished {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, native_frame: &mut eframe::Frame) {
        let now = Instant::now();
        let ui_frame = ui.ctx().cumulative_frame_nr();
        let new_frame = self.last_ui_frame != Some(ui_frame);
        if new_frame {
            self.last_ui_frame = Some(ui_frame);
            if let Some(benchmark) = &mut self.benchmark {
                benchmark.complete_previous(now, native_frame.info().cpu_usage);
                if benchmark.samples.len() == BENCHMARK_FRAMES {
                    benchmark.finish(&self.host, self.error.as_deref(), None);
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
            }
        }
        let mut sample = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click());
                // Native Player has no text/code editors. Widget focus (for
                // example Restart) must not suppress movement indefinitely.
                let mut input = read_input_mode(ui, &response, rect, self.host.camera, false);
                if ui.input(|i| i.focused && i.key_pressed(egui::Key::Escape)) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if new_frame {
                    let dt = now.duration_since(self.last).as_secs_f64().min(0.25);
                    self.last = now;
                    if ui.input(|i| !i.focused) {
                        input = GameInput::default();
                    }
                    let step_started = Instant::now();
                    if let Err(e) = self.host.step(dt, &input) {
                        self.error = Some(e);
                    } else if input.restart {
                        self.error = None;
                    }
                    let step_ms = step_started.elapsed().as_secs_f64() * 1000.;
                    let render_started = Instant::now();
                    let mut conversion_ms = 0.;
                    let mut render_success = false;
                    match self.host.render(1920, 1080) {
                        Ok(frame) => {
                            let conversion_started = Instant::now();
                            update_texture(ui, &mut self.texture, &frame);
                            conversion_ms = conversion_started.elapsed().as_secs_f64() * 1000.;
                            self.frames += 1;
                            render_success = true;
                        }
                        Err(e) => self.error = Some(e),
                    }
                    let total_render_ms = render_started.elapsed().as_secs_f64() * 1000.;
                    if let Some(benchmark) = &self.benchmark {
                        let pixels_per_point = ui.ctx().pixels_per_point();
                        sample = Some(NativeFrameSample {
                            started: now,
                            callback_period_ms: 0.,
                            framework_cpu_ms: None,
                            player_ui_ms: 0.,
                            step_ms,
                            render_ms: (total_render_ms - conversion_ms).max(0.),
                            conversion_enqueue_ms: conversion_ms,
                            hud_ms: 0.,
                            framebuffer_pixels: (benchmark.physical_size)(),
                            canvas_pixels: [
                                (rect.width() * pixels_per_point).round() as u32,
                                (rect.height() * pixels_per_point).round() as u32,
                            ],
                            pixels_per_point,
                            enemies: self.host.session.state().living_enemies(),
                            game_status: self.host.session.state().status,
                            render_success,
                            focused: ui.input(|i| i.focused),
                            window_visible: ui.input(|i| i.viewport().visible()),
                            user_input: input.movement != [0.; 2]
                                || input.attack
                                || input.interact
                                || input.restart,
                            had_error: self.error.is_some(),
                        });
                    }
                }
                if let Some(texture) = &self.texture {
                    ui.painter().image(
                        texture.id(),
                        rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                        Color32::WHITE,
                    );
                }
                let hud_started = Instant::now();
                let aim = (response.hovered() && ui.input(|i| i.focused))
                    .then_some(input.aim_world)
                    .or_else(|| {
                        if std::env::var_os("EFRAME_SCREENSHOT_TO").is_none()
                            || std::env::var_os("MODULE_ENGINE_CAPTURE_ATTACK").is_none()
                        {
                            return None;
                        }
                        self.host
                            .session
                            .state()
                            .attack_visuals
                            .iter()
                            .find(|a| {
                                self.host
                                    .session
                                    .state()
                                    .entities
                                    .get(&a.attacker)
                                    .is_some_and(|e| e.components.contains_key("player"))
                            })
                            .map(|a| {
                                [
                                    a.area.center[0] + a.area.direction[0] * a.area.range,
                                    a.area.center[1],
                                    a.area.center[2] + a.area.direction[1] * a.area.range,
                                ]
                            })
                    });
                draw_hud(ui, rect, &mut self.host, aim);
                if let Some(sample) = &mut sample {
                    sample.hud_ms = hud_started.elapsed().as_secs_f64() * 1000.;
                }
                if let Some(error) = &self.error {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        error,
                        egui::FontId::proportional(18.),
                        Color32::LIGHT_RED,
                    );
                }
            });
        if let Some(mut sample) = sample {
            sample.player_ui_ms = now.elapsed().as_secs_f64() * 1000.;
            if let Some(benchmark) = &mut self.benchmark {
                benchmark.previous = Some(sample);
            }
        }
        ui.ctx().request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(benchmark) = &mut self.benchmark
            && !benchmark.finished
        {
            benchmark.finish(
                &self.host,
                self.error.as_deref(),
                Some("Native Player closed before all measured frames completed"),
            );
        }
    }
}

struct NativeFrameSample {
    started: Instant,
    callback_period_ms: f64,
    framework_cpu_ms: Option<f64>,
    player_ui_ms: f64,
    step_ms: f64,
    render_ms: f64,
    conversion_enqueue_ms: f64,
    hud_ms: f64,
    framebuffer_pixels: Option<[u32; 2]>,
    canvas_pixels: [u32; 2],
    pixels_per_point: f32,
    enemies: usize,
    game_status: GameStatus,
    render_success: bool,
    focused: bool,
    window_visible: Option<bool>,
    user_input: bool,
    had_error: bool,
}

struct NativeBenchmark {
    folder: PathBuf,
    outcome: Arc<Mutex<Option<Result<(), String>>>>,
    physical_size: Box<dyn Fn() -> Option<[u32; 2]>>,
    monitor_size: Option<[u32; 2]>,
    refresh_millihertz: Option<u32>,
    started: Instant,
    previous: Option<NativeFrameSample>,
    completed: usize,
    samples: Vec<NativeFrameSample>,
    finished: bool,
    gl_vendor: Option<String>,
    gl_renderer: Option<String>,
}

#[derive(Serialize)]
struct TimingSummary {
    samples: usize,
    mean_ms: f64,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}
fn timing_summary(mut values: Vec<f64>) -> Option<TimingSummary> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let samples = values.len();
    Some(TimingSummary {
        samples,
        mean_ms: values.iter().sum::<f64>() / samples as f64,
        median_ms: if samples.is_multiple_of(2) {
            (values[samples / 2 - 1] + values[samples / 2]) * 0.5
        } else {
            values[samples / 2]
        },
        p95_ms: values[((samples as f64 * 0.95).ceil() as usize).saturating_sub(1)],
        max_ms: values[samples - 1],
    })
}

impl NativeBenchmark {
    fn complete_previous(&mut self, now: Instant, framework_cpu_seconds: Option<f32>) {
        if let Some(mut previous) = self.previous.take() {
            previous.callback_period_ms =
                now.duration_since(previous.started).as_secs_f64() * 1000.;
            previous.framework_cpu_ms = framework_cpu_seconds.map(|s| s as f64 * 1000.);
            if self.completed >= BENCHMARK_WARMUP {
                self.samples.push(previous);
            }
            self.completed += 1;
        }
    }

    fn finish(&mut self, host: &GameHost, error: Option<&str>, interrupted: Option<&str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let callback = timing_summary(self.samples.iter().map(|s| s.callback_period_ms).collect());
        let framework = timing_summary(
            self.samples
                .iter()
                .filter_map(|s| s.framework_cpu_ms)
                .collect(),
        );
        let complete = self.samples.len() == BENCHMARK_FRAMES && interrupted.is_none();
        let fps = callback.as_ref().map(|s| 1000. / s.mean_ms);
        let physical_resolution_met = complete
            && self.samples.iter().all(|s| {
                s.framebuffer_pixels == Some([1920, 1080]) && s.canvas_pixels == [1920, 1080]
            });
        let workload_met = complete
            && self.samples.iter().all(|s| {
                s.enemies == 20
                    && s.game_status == GameStatus::Playing
                    && s.render_success
                    && !s.had_error
                    && s.window_visible != Some(false)
            });
        let timing_met = callback
            .as_ref()
            .is_some_and(|s| s.mean_ms <= FRAME_BUDGET_MS && s.p95_ms <= FRAME_BUDGET_MS);
        let target_met = physical_resolution_met && workload_met && timing_met && error.is_none();
        let state = host.session.state();
        let player_health = state
            .player()
            .and_then(|p| p.components.get("health"))
            .cloned();
        let models: BTreeSet<_> = host.session.asset_bindings().into_values().collect();
        let mut report = serde_json::json!({
            "schema_version":1,
            "kind":"native-player-window",
            "shipping_game_unmodified":true,
            "input_mode":"normal Player input, no scripted actions and no health/stat overrides",
            "warmup_requested":BENCHMARK_WARMUP,"warmup_completed":self.completed.min(BENCHMARK_WARMUP),
            "measured_requested":BENCHMARK_FRAMES,"measured_completed":self.samples.len(),"complete":complete,
            "target_fps":60,"budget_ms":FRAME_BUDGET_MS,"mean_fps":fps,"target_met":target_met,
            "timing_met":timing_met,"physical_resolution_met":physical_resolution_met,"shipping_twenty_enemy_load_met":workload_met,
            "callback_period":callback,"framework_cpu_including_gl_upload_draw_excluding_vsync":framework
        });
        report.as_object_mut().unwrap().extend(serde_json::json!({
            "player_ui":timing_summary(self.samples.iter().map(|s|s.player_ui_ms).collect()),
            "game_step":timing_summary(self.samples.iter().map(|s|s.step_ms).collect()),
            "vulkan_content_render_readback":timing_summary(self.samples.iter().map(|s|s.render_ms).collect()),
            "rgba_conversion_and_egui_enqueue":timing_summary(self.samples.iter().map(|s|s.conversion_enqueue_ms).collect()),
            "hud_commands":timing_summary(self.samples.iter().map(|s|s.hud_ms).collect()),
            "framebuffer_pixels_first":self.samples.first().and_then(|s|s.framebuffer_pixels),
            "framebuffer_pixels_last":self.samples.last().and_then(|s|s.framebuffer_pixels),
            "canvas_pixels_first":self.samples.first().map(|s|s.canvas_pixels),
            "canvas_pixels_last":self.samples.last().map(|s|s.canvas_pixels),
            "offscreen_render_pixels":[1920,1080],"monitor_pixels":self.monitor_size,"monitor_refresh_millihertz":self.refresh_millihertz,
            "pixels_per_point":self.samples.last().map(|s|s.pixels_per_point),
            "gl_vendor":self.gl_vendor,"gl_renderer":self.gl_renderer,"vulkan_device":host.device
        }).as_object().unwrap().clone());
        report.as_object_mut().unwrap().extend(serde_json::json!({
            "cpu_identifier":std::env::var("PROCESSOR_IDENTIFIER").ok(),
            "native_vsync_requested":true,"model_paths":models,"world_kind":format!("{:?}",host.session.world_kind()),
            "enemy_count_min":self.samples.iter().map(|s|s.enemies).min(),"enemy_count_max":self.samples.iter().map(|s|s.enemies).max(),
            "game_status_final":format!("{:?}",state.status),"player_health_final":player_health,
            "focused_frames":self.samples.iter().filter(|s|s.focused).count(),"user_input_frames":self.samples.iter().filter(|s|s.user_input).count(),
            "error_frames":self.samples.iter().filter(|s|s.had_error).count(),"last_error":error,"interrupted":interrupted,
            "measurement_scope":"Callback start-to-start includes previous simulation, Vulkan/readback, RGBA conversion, egui tessellation, OpenGL texture upload/draw, swap/vsync and OS scheduling. Framework CPU comes from eframe previous-frame cpu_usage and excludes vsync waiting. This measures UI/presentation-loop cadence, not photons/display-scanout timing or a separate GL GPU timestamp."
        }).as_object().unwrap().clone());
        let result = serde_json::to_vec_pretty(&report)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                std::fs::write(self.folder.join("native-frame-report.json"), bytes)
                    .map_err(|e| e.to_string())
            })
            .and_then(|()| {
                if complete {
                    Ok(())
                } else {
                    Err(interrupted
                        .unwrap_or("Native Player benchmark incomplete")
                        .to_owned())
                }
            });
        if let Ok(mut outcome) = self.outcome.lock() {
            *outcome = Some(result);
        }
    }
}
pub fn update_texture(
    ui: &egui::Ui,
    texture: &mut Option<egui::TextureHandle>,
    frame: &module_engine_core::Frame,
) {
    let image = egui::ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
    if let Some(texture) = texture {
        texture.set(image, egui::TextureOptions::LINEAR);
    } else {
        *texture = Some(
            ui.ctx()
                .load_texture("game-frame", image, egui::TextureOptions::LINEAR),
        );
    }
}
pub fn read_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    camera: Camera,
) -> GameInput {
    read_input_mode(ui, response, rect, camera, true)
}

fn read_input_mode(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: Rect,
    camera: Camera,
    respect_editor_focus: bool,
) -> GameInput {
    let mut input = GameInput::default();
    if ui.input(|i| i.focused && i.pointer.any_pressed()) && response.hovered() {
        response.request_focus();
    }
    if ui.input(|i| !i.focused)
        || (respect_editor_focus && ui.ctx().egui_wants_keyboard_input() && !response.has_focus())
        || rect.width() <= 0.
        || rect.height() <= 0.
    {
        return input;
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
    input
}
pub fn draw_hud(ui: &mut egui::Ui, rect: Rect, host: &mut GameHost, aim: Option<[f32; 3]>) {
    let state = host.session.state();
    let painter = ui.painter_at(rect);
    for warning in state
        .telegraphs
        .iter()
        .filter(|_| state.status == GameStatus::Playing)
    {
        if let Some(shape) = telegraph_shape(host.camera, warning, rect) {
            painter.add(shape);
        }
    }
    crate::combat_visuals::draw(ui, rect, host.camera, state, aim);
    let player = state
        .entities
        .values()
        .find(|e| e.components.contains_key("player"));
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
        .entities
        .values()
        .filter(|e| {
            e.components
                .get("actor")
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
        .entities
        .values()
        .filter(|e| e.components.contains_key("boss"))
    {
        let current = entity.components["health"]["current"]
            .as_f64()
            .unwrap_or(0.);
        let maximum = entity.components["health"]["maximum"]
            .as_f64()
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
    if state.status != GameStatus::Playing {
        painter.rect_filled(
            Rect::from_center_size(rect.center(), Vec2::new(370., 130.)),
            12.,
            Color32::from_black_alpha(220),
        );
        painter.text(
            rect.center() - Vec2::new(0., 26.),
            egui::Align2::CENTER_CENTER,
            if state.status == GameStatus::Victory {
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
            let _ = host.session.restart();
        }
    }
}

fn telegraph_shape(
    camera: Camera,
    warning: &module_engine_core::AttackTelegraph,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn input_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        focus_text: bool,
        focused: bool,
    ) -> GameInput {
        let mut result = GameInput::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
            focused,
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            let mut code = String::new();
            let text = ui.put(
                Rect::from_min_max(Pos2::ZERO, Pos2::new(180., 30.)),
                egui::TextEdit::singleline(&mut code).id(egui::Id::new("code")),
            );
            if focus_text {
                text.request_focus();
            }
            let rect = Rect::from_min_max(Pos2::new(0., 40.), Pos2::new(800., 600.));
            let response = ui.interact(rect, egui::Id::new("game-viewport"), Sense::click());
            result = read_input(ui, &response, rect, Camera::default());
        });
        // Headless tests intentionally do not upload the font texture.
        output.textures_delta.clear();
        result
    }

    fn mouse(pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(Pos2::new(400., 300.)),
            egui::Event::PointerButton {
                pos: Pos2::new(400., 300.),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }
    fn key_w(pressed: bool) -> egui::Event {
        egui::Event::Key {
            key: egui::Key::W,
            physical_key: Some(egui::Key::W),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn viewport_focus_keeps_keyboard_and_press_attacks_but_code_focus_blocks_them() {
        let ctx = egui::Context::default();
        input_frame(&ctx, vec![], false, true);
        assert!(input_frame(&ctx, mouse(true), false, true).attack);
        assert!(!input_frame(&ctx, mouse(false), false, true).attack);
        let movement = input_frame(&ctx, vec![key_w(true)], false, true);
        assert_eq!(movement.movement, [0., -1.]);
        let next = input_frame(&ctx, mouse(true), false, true);
        assert!(next.attack);
        assert_eq!(next.movement, [0., -1.]);
        let editing = input_frame(&ctx, mouse(false), true, true);
        assert_eq!(editing, GameInput::default());
        assert_eq!(
            input_frame(&ctx, vec![], false, false),
            GameInput::default()
        );
    }

    #[test]
    fn percentile_summary_counts_slow_frames_without_rounding_to_sixty_fps() {
        let result = timing_summary(vec![10., 20., 30., 40.]).unwrap();
        assert_eq!(result.mean_ms, 25.);
        assert_eq!(result.median_ms, 25.);
        assert_eq!(result.p95_ms, 40.);
        assert_eq!(result.max_ms, 40.);
        assert!(timing_summary(vec![]).is_none());
        assert!(timing_summary(vec![16.8; 120]).unwrap().mean_ms > FRAME_BUDGET_MS);
    }

    #[test]
    fn full_circle_warning_excludes_center_and_wide_sectors_use_explicit_triangles() {
        let camera = Camera {
            yaw: 0.,
            pitch: 0.95,
            distance: 21.,
            target: [0.; 3],
        };
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1920., 1080.));
        let mut warning = module_engine_core::AttackTelegraph {
            attacker: 100,
            center: [0.; 3],
            direction: [1., 0.],
            range: 3.,
            arc: 6.3,
            progress: 0.5,
        };
        let center = screen_point(camera, warning.center, rect).unwrap();
        match telegraph_shape(camera, &warning, rect).unwrap() {
            egui::Shape::Path(path) => {
                assert!(path.closed);
                assert_eq!(path.points.len(), 28);
                assert!(path.points.iter().all(|p| p.distance(center) > 10.));
            }
            _ => panic!("Full-circle telegraph must have one convex boundary"),
        }
        warning.arc = 4.5;
        match telegraph_shape(camera, &warning, rect).unwrap() {
            egui::Shape::Vec(shapes) => match &shapes[0] {
                egui::Shape::Mesh(mesh) => {
                    assert_eq!(mesh.vertices.len(), 30);
                    assert_eq!(mesh.indices.len(), 28 * 3);
                    assert!(
                        mesh.indices
                            .as_chunks::<3>()
                            .0
                            .iter()
                            .all(|triangle| triangle[0] == 0)
                    );
                }
                _ => panic!("Wide sector fill must use a triangle fan"),
            },
            _ => panic!("Wide sector must not enter convex-polygon tessellation"),
        }
    }
}
