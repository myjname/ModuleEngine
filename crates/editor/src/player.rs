//! Standalone game presentation. Shares gameplay and renderer with editor Play.
use crate::game_host::GameHost;
use crate::game_presentations::{capture_input_for, has_input_for};
pub use crate::game_presentations::{draw_hud_for, preview_aim_for, read_input_for};
use crate::game_worker::{GameSnapshot, GameWorker, WorkerCommand};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Vec2};
use module_engine_core::{GameInput, GraphDocument, WorldKind};
use serde::Serialize;
use std::{
    collections::{BTreeSet, VecDeque},
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
        if std::env::var_os("MODULE_ENGINE_CAPTURE_ATTACK").is_some() {
            input = capture_input_for(host.presentation_id(), host.session.state());
        }
        host.step(1. / 60., &input)?;
        if !input.actions.is_empty() {
            input.actions.clear();
            input.action_values.clear();
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
        "ModuleEngine / Player",
        options,
        Box::new(move |cc| {
            crate::app::configure_style(&cc.egui_ctx);
            let mut player = Player::new(host, &cc.egui_ctx, None);
            if std::env::var_os("EFRAME_SCREENSHOT_TO").is_some() {
                // The two-frame diagnostic must capture actual pixels. Wait
                // only in this explicit startup diagnostic, before UI begins;
                // interactive Player always starts with nonblocking snapshots.
                player.prepare_capture()?;
            }
            Ok(Box::new(player))
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
    let model_paths: BTreeSet<_> = host.session.asset_bindings().into_values().collect();
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
            Ok(Box::new(Player::new(
                host,
                &cc.egui_ctx,
                Some(NativeBenchmark {
                    folder,
                    outcome,
                    physical_size,
                    monitor_size,
                    refresh_millihertz,
                    started: Instant::now(),
                    previous: None,
                    completed: 0,
                    samples: vec![],
                    model_paths,
                    game_frames: vec![],
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
            )))
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
    worker: GameWorker,
    snapshot: Option<Arc<GameSnapshot>>,
    texture: Option<egui::TextureHandle>,
    uploaded: Option<Arc<module_engine_core::Frame>>,
    pending_commands: VecDeque<WorkerCommand>,
    pending_inputs: VecDeque<GameInput>,
    error: Option<String>,
    last_ui_frame: Option<u64>,
    benchmark: Option<NativeBenchmark>,
}
impl Player {
    fn prepare_capture(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            self.dispatch(GameInput::default());
            if let Some(snapshot) = self.worker.latest() {
                if let Some(error) = snapshot.fault.as_ref().or(snapshot.render_error.as_ref()) {
                    return Err(error.clone());
                }
                if snapshot.frame.is_some() {
                    self.snapshot = Some(snapshot);
                    return Ok(());
                }
            }
            if !self.worker.is_alive() || Instant::now() >= deadline {
                return Err("Diagnostic Player did not produce its first game frame".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn new(host: GameHost, ctx: &egui::Context, benchmark: Option<NativeBenchmark>) -> Self {
        Self {
            worker: GameWorker::spawn(host, Some(ctx.clone())),
            snapshot: None,
            texture: None,
            uploaded: None,
            pending_commands: VecDeque::from([
                WorkerCommand::SetViewport {
                    width: 1920,
                    height: 1080,
                },
                WorkerCommand::SetPlaying(true),
            ]),
            pending_inputs: VecDeque::new(),
            error: None,
            last_ui_frame: None,
            benchmark,
        }
    }

    fn dispatch(&mut self, input: GameInput) {
        // Retain edge inputs while the bounded actor mailbox is full. Held
        // movement is always sent after the retained edges with its latest value.
        if input.restart {
            // R cancels the old local queue too. Placing it behind an edge
            // rejected by a full fault-latched mailbox would make recovery
            // unreachable, despite the actor's priority restart protocol.
            self.pending_inputs.clear();
            self.pending_inputs.push_back(input.clone());
        } else if !input.actions.is_empty() {
            if self.pending_inputs.len() < 128 {
                self.pending_inputs.push_back(input.clone());
            } else {
                self.error = Some("Input queue overloaded; wait for the simulation".into());
            }
        }
        while let Some(command) = self.pending_commands.pop_front() {
            if let Err(command) = self.worker.try_command(command) {
                self.pending_commands.push_front(command);
                break;
            }
        }
        while let Some(edge) = self.pending_inputs.pop_front() {
            if let Err(edge) = self.worker.submit_input(edge) {
                self.pending_inputs.push_front(edge);
                break;
            }
        }
        let mut held = input;
        held.actions.clear();
        held.action_values.clear();
        held.restart = false;
        // A contended tiny mailbox is retried on the next UI frame; it never
        // locks the UI behind a running script, renderer or state query.
        let _ = self.worker.submit_input(held);
    }
}
/// Successful pixels clear only the renderer's own previous diagnostic. A
/// newer simulation/Custom error must remain visible in both Player and Game.
pub(crate) fn update_render_diagnostic(
    error: &mut Option<String>,
    render_error: &mut Option<String>,
    result: Result<(), String>,
) {
    match result {
        Err(message) => {
            *render_error = Some(message.clone());
            *error = Some(message);
        }
        Ok(()) => {
            if render_error.is_some() && error.as_ref() == render_error.as_ref() {
                *error = None;
            }
            *render_error = None;
        }
    }
}
impl eframe::App for Player {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.snapshot = self.worker.latest();
        if let Some(benchmark) = &mut self.benchmark {
            if !benchmark.finished && benchmark.started.elapsed().as_secs() >= 45 {
                benchmark.finish(self.snapshot.as_deref(), self.error.as_deref(), Some("45-second watchdog: window may be hidden, occluded, minimized, or too slow"));
            }
            if benchmark.finished {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, native_frame: &mut eframe::Frame) {
        let now = Instant::now();
        let ui_frame = ui.ctx().cumulative_frame_nr();
        let new_frame = self.last_ui_frame != Some(ui_frame);
        let snapshot = self.worker.latest();
        self.snapshot = snapshot.clone();
        if !self.worker.is_alive() {
            self.error = Some("Game worker stopped unexpectedly".into());
        }
        if new_frame {
            self.last_ui_frame = Some(ui_frame);
            if let Some(benchmark) = &mut self.benchmark {
                benchmark.complete_previous(now, native_frame.info().cpu_usage);
                if benchmark.samples.len() >= BENCHMARK_FRAMES
                    && benchmark.game_frames.len() >= BENCHMARK_FRAMES
                {
                    benchmark.finish(snapshot.as_deref(), self.error.as_deref(), None);
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
                let camera = snapshot.as_ref().map(|s| s.camera).unwrap_or_default();
                let presentation = snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.presentation_ids.first())
                    .map(String::as_str);
                let mut input = read_input_for(presentation, ui, &response, rect, camera, false);
                if ui.input(|i| i.focused && i.key_pressed(egui::Key::Escape)) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if new_frame {
                    if ui.input(|i| !i.focused) {
                        input = GameInput::default();
                    }
                    let dispatch_started = Instant::now();
                    self.dispatch(input.clone());
                    let dispatch_ms = dispatch_started.elapsed().as_secs_f64() * 1000.;
                    let mut conversion_ms = 0.;
                    let mut fresh_frame = false;
                    if let Some(snapshot) = &snapshot {
                        if let Some(frame) = &snapshot.frame
                            && self
                                .uploaded
                                .as_ref()
                                .is_none_or(|old| !Arc::ptr_eq(old, frame))
                        {
                            let started = Instant::now();
                            update_texture(ui, &mut self.texture, frame);
                            conversion_ms = started.elapsed().as_secs_f64() * 1000.;
                            self.uploaded = Some(Arc::clone(frame));
                            fresh_frame = true;
                        }
                        self.error = snapshot
                            .fault
                            .clone()
                            .or_else(|| snapshot.preparation_error.clone())
                            .or_else(|| snapshot.render_error.clone())
                            .or_else(|| {
                                (!self.worker.is_alive())
                                    .then(|| "Game worker stopped unexpectedly".to_owned())
                            });
                        if let Some(benchmark) = &mut self.benchmark {
                            benchmark.observe_game_frame(now, snapshot, fresh_frame);
                            let pixels_per_point = ui.ctx().pixels_per_point();
                            sample = Some(NativeFrameSample {
                                started: now,
                                callback_period_ms: 0.,
                                framework_cpu_ms: None,
                                player_ui_ms: 0.,
                                step_ms: snapshot.last_step_ms,
                                render_ms: snapshot.last_render_ms,
                                dispatch_ms,
                                fresh_frame,
                                game_tick: snapshot.state.tick,
                                game_time: snapshot.state.time,
                                conversion_enqueue_ms: conversion_ms,
                                hud_ms: 0.,
                                framebuffer_pixels: (benchmark.physical_size)(),
                                canvas_pixels: [
                                    (rect.width() * pixels_per_point).round() as u32,
                                    (rect.height() * pixels_per_point).round() as u32,
                                ],
                                pixels_per_point,
                                enemies: snapshot
                                    .game_diagnostics
                                    .get("active_enemies")
                                    .and_then(serde_json::Value::as_u64)
                                    .unwrap_or_default()
                                    as usize,
                                game_status: snapshot
                                    .game_diagnostics
                                    .get("status")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                render_success: snapshot.frame.is_some()
                                    && snapshot.render_error.is_none(),
                                focused: ui.input(|i| i.focused),
                                window_visible: ui.input(|i| i.viewport().visible()),
                                user_input: has_input_for(presentation, &input),
                                had_error: self.error.is_some(),
                            });
                        }
                    }
                }
                if let Some(texture) = &self.texture {
                    ui.painter().image(
                        texture.id(),
                        rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                        Color32::WHITE,
                    );
                } else {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Preparing first game frame…",
                        egui::FontId::proportional(18.),
                        Color32::WHITE,
                    );
                }
                let hud_started = Instant::now();
                if let Some(snapshot) = &snapshot {
                    let aim = preview_aim_for(
                        presentation,
                        &snapshot.state,
                        &input,
                        response.hovered() && ui.input(|i| i.focused),
                        std::env::var_os("EFRAME_SCREENSHOT_TO").is_some()
                            && std::env::var_os("MODULE_ENGINE_CAPTURE_ATTACK").is_some(),
                    );
                    if draw_hud_for(
                        presentation,
                        ui,
                        rect,
                        &snapshot.state,
                        snapshot.camera,
                        aim,
                    ) {
                        self.pending_inputs.clear();
                        self.pending_commands
                            .push_back(WorkerCommand::Restart { request: ui_frame });
                    }
                    // Slow game code affects simulation speed, not window event
                    // handling. Show that distinction with the measured tick cost.
                    if snapshot.last_step_ms > FRAME_BUDGET_MS || snapshot.backlog_seconds > 0.05 {
                        ui.painter().text(
                            rect.right_bottom() - Vec2::splat(16.),
                            egui::Align2::RIGHT_BOTTOM,
                            format!(
                                "Simulation: {:.1} ms / tick · backlog {:.0} ms",
                                snapshot.last_step_ms,
                                snapshot.backlog_seconds * 1000.
                            ),
                            egui::FontId::proportional(14.),
                            Color32::YELLOW,
                        );
                    }
                }
                if let Some(sample) = &mut sample {
                    sample.hud_ms = hud_started.elapsed().as_secs_f64() * 1000.;
                }
                if let Some(error) = &self.error {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        if snapshot.as_ref().is_some_and(|s| s.fault.is_some()) {
                            format!("Simulation paused: {error}\nR — restart")
                        } else {
                            error.clone()
                        },
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
        self.worker.shutdown();
        if let Some(benchmark) = &mut self.benchmark
            && !benchmark.finished
        {
            benchmark.finish(
                self.snapshot.as_deref(),
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
    dispatch_ms: f64,
    fresh_frame: bool,
    game_tick: u64,
    game_time: f64,
    conversion_enqueue_ms: f64,
    hud_ms: f64,
    framebuffer_pixels: Option<[u32; 2]>,
    canvas_pixels: [u32; 2],
    pixels_per_point: f32,
    enemies: usize,
    game_status: String,
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
    model_paths: BTreeSet<String>,
    game_frames: Vec<Instant>,
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
    fn observe_game_frame(&mut self, now: Instant, snapshot: &GameSnapshot, fresh: bool) {
        if fresh && self.completed >= BENCHMARK_WARMUP {
            self.game_frames
                .push(snapshot.frame_completed_at.unwrap_or(now));
        }
    }
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

    fn finish(
        &mut self,
        snapshot: Option<&GameSnapshot>,
        error: Option<&str>,
        interrupted: Option<&str>,
    ) {
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
        let complete = self.samples.len() >= BENCHMARK_FRAMES
            && self.game_frames.len() >= BENCHMARK_FRAMES
            && interrupted.is_none();
        let ui_fps = callback.as_ref().map(|s| 1000. / s.mean_ms);
        let game_frame_interval = timing_summary(
            self.game_frames
                .windows(2)
                .map(|v| v[1].duration_since(v[0]).as_secs_f64() * 1000.)
                .collect(),
        );
        let fps = game_frame_interval.as_ref().map(|s| 1000. / s.mean_ms);
        let physical_resolution_met = complete
            && self.samples.iter().all(|s| {
                s.framebuffer_pixels == Some([1920, 1080]) && s.canvas_pixels == [1920, 1080]
            });
        let workload_met = complete
            && self.samples.iter().all(|s| {
                s.enemies == 20
                    && s.game_status == "Playing"
                    && s.render_success
                    && !s.had_error
                    && s.window_visible != Some(false)
            });
        let ui_timing_met = callback
            .as_ref()
            .is_some_and(|s| s.mean_ms <= FRAME_BUDGET_MS && s.p95_ms <= FRAME_BUDGET_MS);
        let timing_met = ui_timing_met
            && game_frame_interval
                .as_ref()
                .is_some_and(|s| s.mean_ms <= FRAME_BUDGET_MS && s.p95_ms <= FRAME_BUDGET_MS);
        let target_met = physical_resolution_met && workload_met && timing_met && error.is_none();
        let diagnostics = snapshot.map(|snapshot| &snapshot.game_diagnostics);
        let player_health = diagnostics
            .and_then(|value| value.get("player_health"))
            .cloned();
        let mut report = serde_json::json!({
            "schema_version":2,
            "kind":"native-player-window",
            "shipping_game_unmodified":true,
            "input_mode":"normal Player input, no scripted actions and no health/stat overrides",
            "warmup_requested":BENCHMARK_WARMUP,"warmup_completed":self.completed.min(BENCHMARK_WARMUP),
            "measured_requested":BENCHMARK_FRAMES,"measured_completed":self.samples.len(),"complete":complete,
            "completion_criterion":"At least 120 UI callback samples and 120 distinct completed Game frames after warmup; watchdog otherwise reports incomplete",
            "target_fps":60,"budget_ms":FRAME_BUDGET_MS,"mean_fps":fps,"target_met":target_met,
            "ui_mean_fps":ui_fps,"unique_game_frame_interval":game_frame_interval,"unique_game_frames":self.game_frames.len(),"ui_timing_met":ui_timing_met,
            "timing_met":timing_met,"physical_resolution_met":physical_resolution_met,"shipping_twenty_enemy_load_met":workload_met,
            "callback_period":callback,"framework_cpu_including_gl_upload_draw_excluding_vsync":framework
        });
        report.as_object_mut().unwrap().extend(serde_json::json!({
            "player_ui":timing_summary(self.samples.iter().map(|s|s.player_ui_ms).collect()),
            "game_step":timing_summary(self.samples.iter().map(|s|s.step_ms).collect()),
            "worker_cost_statistics_scope":"Last completed step/render cost observed at each UI callback; UI-weighted observations, not independent per-worker-frame samples",
            "input_dispatch":timing_summary(self.samples.iter().map(|s|s.dispatch_ms).collect()),
            "fresh_game_frame_callbacks":self.samples.iter().filter(|s|s.fresh_frame).count(),
            "game_tick_first":self.samples.first().map(|s|s.game_tick),"game_tick_last":self.samples.last().map(|s|s.game_tick),
            "game_time_first":self.samples.first().map(|s|s.game_time),"game_time_last":self.samples.last().map(|s|s.game_time),
            "vulkan_content_render_readback":timing_summary(self.samples.iter().map(|s|s.render_ms).collect()),
            "rgba_conversion_and_egui_enqueue":timing_summary(self.samples.iter().map(|s|s.conversion_enqueue_ms).collect()),
            "hud_commands":timing_summary(self.samples.iter().map(|s|s.hud_ms).collect()),
            "framebuffer_pixels_first":self.samples.first().and_then(|s|s.framebuffer_pixels),
            "framebuffer_pixels_last":self.samples.last().and_then(|s|s.framebuffer_pixels),
            "canvas_pixels_first":self.samples.first().map(|s|s.canvas_pixels),
            "canvas_pixels_last":self.samples.last().map(|s|s.canvas_pixels),
            "offscreen_render_pixels":[1920,1080],"monitor_pixels":self.monitor_size,"monitor_refresh_millihertz":self.refresh_millihertz,
            "pixels_per_point":self.samples.last().map(|s|s.pixels_per_point),
            "gl_vendor":self.gl_vendor,"gl_renderer":self.gl_renderer,"vulkan_device":snapshot.map(|s|s.device.as_str())
        }).as_object().unwrap().clone());
        report.as_object_mut().unwrap().extend(serde_json::json!({
            "cpu_identifier":std::env::var("PROCESSOR_IDENTIFIER").ok(),
            "native_vsync_requested":true,"model_paths":self.model_paths,"world_kind":snapshot.map(|s|format!("{:?}",s.world_kind)),
            "enemy_count_min":self.samples.iter().map(|s|s.enemies).min(),"enemy_count_max":self.samples.iter().map(|s|s.enemies).max(),
            "game_status_final":diagnostics.and_then(|value|value.get("status")),"player_health_final":player_health,
            "focused_frames":self.samples.iter().filter(|s|s.focused).count(),"user_input_frames":self.samples.iter().filter(|s|s.user_input).count(),
            "error_frames":self.samples.iter().filter(|s|s.had_error).count(),"last_error":error,"interrupted":interrupted,
            "measurement_scope":"Callback cadence measures UI/presentation including texture upload, draw, swap/vsync and OS scheduling. Simulation and Vulkan/readback run on the Game worker; their last completed costs are reported separately. Unique game frames are counted by changed Frame Arc identity; repeats of an old frame never increase mean_fps. Observed frame intervals are quantized by UI sampling; this is not photons/display scanout or GPU timestamps."
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
#[cfg(test)]
pub use module_engine_arpg::ui::{draw_hud, read_input, screen_point, telegraph_shape};

#[cfg(test)]
mod tests {
    use super::*;
    use module_engine_arpg::{ArpgInput, GameInputExt};
    use module_engine_core::{Camera, GameState};

    #[test]
    fn player_restart_reaches_paused_full_mailbox_and_cancels_unsent_old_actions() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let host = GameHost::new(
            crate::game_host::demo_document(),
            &root,
            Some(&root),
            WorldKind::Objects,
        )
        .unwrap();
        let mut player = Player::new(host, &egui::Context::default(), None);
        player.pending_commands.clear(); // The actor starts paused for this fixture.
        let edge = ArpgInput {
            interact: true,
            ..Default::default()
        }
        .into_input();
        let mut accepted = 0;
        while accepted < 128 {
            if player.worker.submit_input(edge.clone()).is_ok() {
                accepted += 1;
            } else {
                std::thread::yield_now();
            }
        }
        // Reproduce both queues full while the paused/faulted actor cannot
        // consume attacks. R must bypass these obsolete local and remote edges.
        player.pending_inputs = VecDeque::from(vec![edge; 128]);
        player.dispatch(GameInput {
            restart: true,
            ..Default::default()
        });
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            player.dispatch(GameInput::default());
            if let Some(snapshot) = player.worker.latest()
                && snapshot.epoch == 1
            {
                assert_eq!(snapshot.state.tick, 0);
                assert!(snapshot.input_ack > 128);
                assert!(snapshot.cancelled_input_ack >= 128);
                assert!(player.pending_inputs.is_empty());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Player R did not reach the full paused actor"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        player.worker.shutdown();
    }

    #[test]
    fn hud_reads_owned_snapshot_without_requiring_optional_boss_health() {
        let game = crate::game_host::demo_document().game.unwrap();
        let mut boss = game
            .scenes
            .keys()
            .flat_map(|scene| game.resolve_scene(scene).unwrap())
            .find(|entity| entity.components.contains_key("boss"))
            .unwrap();
        boss.components.remove("health");
        let mut state: GameState = serde_json::from_value(serde_json::json!({
            "scene":"test", "time":0., "tick":0, "status":"Playing", "entities":{}, "transition":null
        })).unwrap();
        state.world.upsert_entity(boss).unwrap();
        let original = state.clone();
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.));
            assert!(!draw_hud(ui, rect, &state, Camera::default(), None));
        });
        output.textures_delta.clear();
        assert_eq!(state, original);
    }

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
        assert!(input_frame(&ctx, mouse(true), false, true).attack());
        assert!(!input_frame(&ctx, mouse(false), false, true).attack());
        let movement = input_frame(&ctx, vec![key_w(true)], false, true);
        assert_eq!(movement.movement(), [0., -1.]);
        let next = input_frame(&ctx, mouse(true), false, true);
        assert!(next.attack());
        assert_eq!(next.movement(), [0., -1.]);
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
        let mut warning = module_engine_arpg::AttackTelegraph {
            attacker: 100,
            attacker_lifetime: None,
            attacker_player: None,
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
