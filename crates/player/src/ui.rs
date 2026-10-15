//! Standalone game presentation. Shares gameplay and renderer with editor Play.
use crate::presentations::{capture_input_for, has_input_for};
pub use crate::presentations::{draw_hud_for, preview_aim_for, read_input_for};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Vec2};
use module_engine_core::GameInput;
use module_engine_host::GameHost;
use module_engine_host::game_worker::{GameSnapshot, GameWorker, WorkerCommand};
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

/// Shared window presentation policy for Editor, Player, and native diagnostics.
/// The GUI uses WGPU/DX12 on Windows and WGPU/Vulkan on Linux. This does not
/// select the game's renderer. Glow requires an explicit diagnostic request.
/// `MODULE_ENGINE_WINDOW_SYSTEM=auto|wayland|x11` selects Linux window creation
/// independently from the GPU backend.
pub fn native_options(viewport: egui::ViewportBuilder) -> Result<eframe::NativeOptions, String> {
    let policy = requested_gui_policy()?;
    if policy.platform == GuiPlatform::Linux {
        validate_linux_display(
            policy.window_system,
            env_present("WAYLAND_DISPLAY") || env_present("WAYLAND_SOCKET"),
            env_present("DISPLAY"),
        )?;
    }
    eprintln!("ModuleEngine GUI requested: {policy:?}; game renderer is independent");
    Ok(native_options_for(viewport, policy))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuiPlatform {
    Windows,
    Linux,
    Other,
}
impl GuiPlatform {
    fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowSystem {
    Auto,
    Wayland,
    X11,
}
impl WindowSystem {
    fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Wayland => "wayland",
            Self::X11 => "x11",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GuiPolicy {
    platform: GuiPlatform,
    renderer: eframe::Renderer,
    window_system: WindowSystem,
}
impl GuiPolicy {
    fn gpu_backend(self) -> &'static str {
        match (self.renderer, self.platform) {
            (eframe::Renderer::Glow, _) => "OpenGL",
            (eframe::Renderer::Wgpu, GuiPlatform::Windows) => "DX12",
            (eframe::Renderer::Wgpu, GuiPlatform::Linux) => "Vulkan",
            (eframe::Renderer::Wgpu, GuiPlatform::Other) => "platform default",
        }
    }
}

fn env_option(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("{name}: {error}")),
    }
}

fn env_present(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn requested_gui_policy() -> Result<GuiPolicy, String> {
    gui_policy_for(
        GuiPlatform::current(),
        env_option("MODULE_ENGINE_GUI_BACKEND")?.as_deref(),
        env_option("MODULE_ENGINE_WINDOW_SYSTEM")?.as_deref(),
        std::env::var_os("EFRAME_SCREENSHOT_TO").is_some(),
    )
}

fn gui_policy_for(
    platform: GuiPlatform,
    requested_renderer: Option<&str>,
    requested_window_system: Option<&str>,
    legacy_capture: bool,
) -> Result<GuiPolicy, String> {
    let renderer = match requested_renderer {
        Some("glow") => eframe::Renderer::Glow,
        Some("wgpu") => eframe::Renderer::Wgpu,
        Some(_) => return Err("MODULE_ENGINE_GUI_BACKEND must be glow or wgpu".into()),
        None if platform == GuiPlatform::Other => eframe::Renderer::Glow,
        None => eframe::Renderer::Wgpu,
    };
    if legacy_capture && renderer == eframe::Renderer::Wgpu {
        return Err(
            "EFRAME_SCREENSHOT_TO only supports explicit MODULE_ENGINE_GUI_BACKEND=glow diagnostics; it does not change the normal WGPU backend. Unset EFRAME_SCREENSHOT_TO and use the Editor's MODULE_ENGINE_UI_SCREENSHOT_TO for a native WGPU screenshot."
                .into(),
        );
    }
    let window_system = match requested_window_system {
        None | Some("auto") => WindowSystem::Auto,
        Some("wayland") => WindowSystem::Wayland,
        Some("x11") => WindowSystem::X11,
        Some(_) => {
            return Err("MODULE_ENGINE_WINDOW_SYSTEM must be auto, wayland, or x11".into());
        }
    };
    if platform != GuiPlatform::Linux && window_system != WindowSystem::Auto {
        return Err("MODULE_ENGINE_WINDOW_SYSTEM=wayland|x11 is only supported on Linux".into());
    }
    Ok(GuiPolicy {
        platform,
        renderer,
        window_system,
    })
}

fn validate_linux_display(
    window_system: WindowSystem,
    has_wayland: bool,
    has_x11: bool,
) -> Result<(), String> {
    let available = match window_system {
        WindowSystem::Auto => has_wayland || has_x11,
        WindowSystem::Wayland => has_wayland,
        WindowSystem::X11 => has_x11,
    };
    if available {
        return Ok(());
    }
    Err(format!(
        "Linux GUI window-system={} is unavailable: Wayland requires WAYLAND_DISPLAY or WAYLAND_SOCKET; X11 requires DISPLAY. Run in a graphical session (headless package checks do not require a window).",
        window_system.name()
    ))
}

fn native_options_for(viewport: egui::ViewportBuilder, policy: GuiPolicy) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions {
        viewport,
        renderer: policy.renderer,
        ..Default::default()
    };
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        // Restrict GUI presentation to the verified platform path. WGPU_BACKEND
        // must not silently override Windows DX12 or request DX12 on Linux.
        match policy.platform {
            GuiPlatform::Windows => {
                setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
            }
            GuiPlatform::Linux => {
                setup.instance_descriptor.backends = eframe::wgpu::Backends::VULKAN;
            }
            GuiPlatform::Other => {}
        }
    }
    #[cfg(target_os = "linux")]
    {
        use winit::platform::wayland::EventLoopBuilderExtWayland;
        use winit::platform::x11::EventLoopBuilderExtX11;
        options.event_loop_builder = Some(Box::new(move |builder| match policy.window_system {
            WindowSystem::Auto => {}
            WindowSystem::Wayland => {
                builder.with_wayland();
            }
            WindowSystem::X11 => {
                builder.with_x11();
            }
        }));
    }
    options
}

/// Preserve the original framework error with the requested platform policy.
/// This adds useful context when no surface, display server, or GPU is available;
/// it never retries through a different presentation backend.
pub fn gui_startup_error(error: impl std::fmt::Display) -> String {
    let request = requested_gui_policy()
        .map(|policy| {
            format!(
                "renderer={:?}, GPU={}, window-system={}",
                policy.renderer,
                policy.gpu_backend(),
                policy.window_system.name()
            )
        })
        .unwrap_or_else(|error| format!("invalid GUI configuration: {error}"));
    let hint = if cfg!(target_os = "linux") {
        "Check the selected Wayland/X11 session and a native Vulkan-capable GPU driver/ICD."
    } else if cfg!(target_os = "windows") {
        "The default Windows GUI requires a DX12 adapter and driver."
    } else {
        "Check the native graphics adapter and window system."
    };
    format!("ModuleEngine GUI initialization failed ({request}): {error}. {hint}")
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct GuiGraphicsInfo {
    pub renderer: &'static str,
    pub backend: Option<String>,
    pub adapter_name: Option<String>,
    pub window_system: Option<String>,
    pub requested_renderer: Option<String>,
    pub requested_gpu_backend: Option<String>,
    pub requested_window_system: Option<String>,
    pub gl_vendor: Option<String>,
    pub gl_renderer: Option<String>,
}

/// Record the initialized adapter and display handle separately from requests.
pub fn report_graphics(cc: &eframe::CreationContext<'_>) -> GuiGraphicsInfo {
    use eframe::wgpu::rwh::{HasDisplayHandle, RawDisplayHandle};
    let mut graphics = if let Some(state) = &cc.wgpu_render_state {
        let adapter = state.adapter.get_info();
        GuiGraphicsInfo {
            renderer: "Wgpu",
            backend: Some(format!("{:?}", adapter.backend)),
            adapter_name: Some(adapter.name),
            ..Default::default()
        }
    } else if let Some(gl) = &cc.gl {
        use eframe::glow::HasContext;
        let vendor = unsafe { gl.get_parameter_string(eframe::glow::VENDOR) };
        let renderer = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
        GuiGraphicsInfo {
            renderer: "Glow",
            backend: Some("OpenGL".into()),
            adapter_name: Some(renderer.clone()),
            gl_vendor: Some(vendor),
            gl_renderer: Some(renderer),
            ..Default::default()
        }
    } else {
        GuiGraphicsInfo {
            renderer: "Unknown",
            ..Default::default()
        }
    };
    graphics.window_system = cc.display_handle().ok().map(|handle| {
        match handle.as_raw() {
            RawDisplayHandle::Windows(_) => "Windows",
            RawDisplayHandle::Wayland(_) => "Wayland",
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => "X11",
            RawDisplayHandle::AppKit(_) => "AppKit",
            _ => "Other",
        }
        .into()
    });
    if let Ok(policy) = requested_gui_policy() {
        graphics.requested_renderer = Some(format!("{:?}", policy.renderer));
        graphics.requested_gpu_backend = Some(policy.gpu_backend().into());
        graphics.requested_window_system = Some(policy.window_system.name().into());
    }
    eprintln!("ModuleEngine GUI initialized: {graphics:?}");
    graphics
}

pub fn run_host(mut host: GameHost) -> Result<(), String> {
    let options = native_options(
        egui::ViewportBuilder::default()
            .with_inner_size([1280., 720.])
            .with_min_inner_size([800., 450.]),
    )?;
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
    eframe::run_native(
        "ModuleEngine / Player",
        options,
        Box::new(move |cc| {
            report_graphics(cc);
            configure_style(&cc.egui_ctx);
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
    .map_err(gui_startup_error)
}

/// Measure the shipping Player path including framework texture upload and
/// presentation cadence. The UI closes itself after 30 + 120 completed frames.
pub fn run_benchmark_host(host: GameHost, folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let folder = folder.canonicalize().map_err(|e| e.to_string())?;
    let model_paths: BTreeSet<_> = host.session.asset_bindings().into_values().collect();
    let outcome = Arc::new(Mutex::new(None));
    let returned = Arc::clone(&outcome);
    let options = native_options(
        egui::ViewportBuilder::default()
            .with_inner_size([1920., 1080.])
            .with_min_inner_size([640., 360.])
            .with_decorations(false)
            .with_position([0., 0.]),
    )?;
    eframe::run_native(
        "ModuleEngine / Native Player benchmark",
        options,
        Box::new(move |cc| {
            let gui_graphics = report_graphics(cc);
            configure_style(&cc.egui_ctx);
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
                    observed_game_frames: vec![],
                    frame_ages_ms: vec![],
                    missing_completion_timestamps: 0,
                    finished: false,
                    gui_graphics,
                }),
            )))
        }),
    )
    .map_err(gui_startup_error)?;
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
            worker: GameWorker::spawn(
                host,
                Some({
                    let ctx = ctx.clone();
                    std::sync::Arc::new(move || ctx.request_repaint())
                }),
            ),
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
pub fn update_render_diagnostic(
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
        let mut hud_ms = 0.;
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
                    let mut observed_at = None;
                    if let Some(snapshot) = &snapshot {
                        if let Some(frame) = &snapshot.frame
                            && self
                                .uploaded
                                .as_ref()
                                .is_none_or(|old| !Arc::ptr_eq(old, frame))
                        {
                            observed_at = Some(Instant::now());
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
                                snapshot
                                    .activity
                                    .iter()
                                    .rev()
                                    .find(|activity| {
                                        activity.state
                                            == module_engine_host::activity::ActivityState::Failed
                                    })
                                    .and_then(|activity| activity.error.clone())
                            })
                            .or_else(|| {
                                (!self.worker.is_alive())
                                    .then(|| "Game worker stopped unexpectedly".to_owned())
                            });
                        if let Some(benchmark) = &mut self.benchmark {
                            benchmark.observe_game_frame(
                                observed_at.unwrap_or(now),
                                snapshot.frame_completed_at,
                                fresh_frame,
                            );
                            let pixels_per_point = ui.ctx().pixels_per_point();
                            sample = Some(NativeFrameSample {
                                started: now,
                                callback_period_ms: 0.,
                                framework_cpu_ms: None,
                                player_ui_ms: 0.,
                                ui_calls: 1,
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
                hud_ms = hud_started.elapsed().as_secs_f64() * 1000.;
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
        if let Some(benchmark) = &mut self.benchmark {
            benchmark.record_ui_pass(sample, now.elapsed().as_secs_f64() * 1000., hud_ms);
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
    ui_calls: usize,
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
    observed_game_frames: Vec<Instant>,
    frame_ages_ms: Vec<f64>,
    missing_completion_timestamps: usize,
    finished: bool,
    gui_graphics: GuiGraphicsInfo,
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
    fn observe_game_frame(&mut self, now: Instant, completed_at: Option<Instant>, fresh: bool) {
        if fresh && self.completed >= BENCHMARK_WARMUP {
            self.observed_game_frames.push(now);
            if let Some(completed) = completed_at {
                self.game_frames.push(completed);
                self.frame_ages_ms
                    .push(now.saturating_duration_since(completed).as_secs_f64() * 1000.);
            } else {
                self.missing_completion_timestamps += 1;
            }
        }
    }
    fn record_ui_pass(
        &mut self,
        sample: Option<NativeFrameSample>,
        player_ui_ms: f64,
        hud_ms: f64,
    ) {
        if let Some(mut sample) = sample {
            sample.player_ui_ms = player_ui_ms;
            sample.hud_ms = hud_ms;
            self.previous = Some(sample);
        } else if let Some(previous) = &mut self.previous {
            previous.player_ui_ms += player_ui_ms;
            previous.hud_ms += hud_ms;
            previous.ui_calls += 1;
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
        let observed_interval_ms = self
            .observed_game_frames
            .windows(2)
            .map(|frames| frames[1].duration_since(frames[0]).as_secs_f64() * 1000.)
            .collect::<Vec<_>>();
        let observed_interval = timing_summary(observed_interval_ms.clone());
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
        let timing_data_valid = self.missing_completion_timestamps == 0;
        let target_met = physical_resolution_met
            && workload_met
            && timing_met
            && error.is_none()
            && timing_data_valid;
        let diagnostics = snapshot.map(|snapshot| &snapshot.game_diagnostics);
        let player_health = diagnostics
            .and_then(|value| value.get("player_health"))
            .cloned();
        let mut report = serde_json::json!({
            "schema_version":3,
            "kind":"native-player-window",
            "shipping_game_unmodified":true,
            "input_mode":"normal Player input, no scripted actions and no health/stat overrides",
            "warmup_requested":BENCHMARK_WARMUP,"warmup_completed":self.completed.min(BENCHMARK_WARMUP),
            "measured_requested":BENCHMARK_FRAMES,"measured_completed":self.samples.len(),"complete":complete,
            "completion_criterion":"At least 120 UI callback samples and 120 distinct completed Game frames after warmup; watchdog otherwise reports incomplete",
            "target_fps":60,"budget_ms":FRAME_BUDGET_MS,"mean_fps":fps,"target_met":target_met,
            "ui_mean_fps":ui_fps,"unique_game_frame_interval":game_frame_interval,"unique_game_frames":self.game_frames.len(),"ui_timing_met":ui_timing_met,
            "legacy_mean_fps_scope":"Completion cadence of distinct worker frames observed by UI; retained for historical comparison, not display presentation cadence",
            "worker_completion_interval_of_observed_frames":game_frame_interval,
            "ui_observed_unique_frame_interval":observed_interval,
            "ui_observed_unique_mean_fps":observed_interval.as_ref().map(|summary|1000./summary.mean_ms),
            "observed_frame_age":timing_summary(self.frame_ages_ms.clone()),
            "missing_completion_timestamps":self.missing_completion_timestamps,"timing_data_valid":timing_data_valid,
            "timing_met":timing_met,"physical_resolution_met":physical_resolution_met,"shipping_twenty_enemy_load_met":workload_met,
            "callback_period":callback,"framework_cpu_including_gl_upload_draw_excluding_vsync":framework
        });
        report.as_object_mut().unwrap().extend(serde_json::json!({
            "player_ui":timing_summary(self.samples.iter().map(|s|s.player_ui_ms).collect()),
            "player_ui_calls":self.samples.iter().map(|sample|sample.ui_calls).collect::<Vec<_>>(),
            "callback_period_samples_ms":self.samples.iter().map(|sample|sample.callback_period_ms).collect::<Vec<_>>(),
            "worker_completion_interval_samples_ms":self.game_frames.windows(2).map(|frames|frames[1].duration_since(frames[0]).as_secs_f64()*1000.).collect::<Vec<_>>(),
            "ui_observed_unique_frame_interval_samples_ms":observed_interval_ms,
            "observed_frame_age_samples_ms":self.frame_ages_ms,
            "game_step":timing_summary(self.samples.iter().map(|s|s.step_ms).collect()),
            "worker_cost_statistics_scope":"Last completed step/render cost observed at each UI callback; UI-weighted observations, not independent per-worker-frame samples",
            "input_dispatch":timing_summary(self.samples.iter().map(|s|s.dispatch_ms).collect()),
            "fresh_game_frame_callbacks":self.samples.iter().filter(|s|s.fresh_frame).count(),
            "game_tick_first":self.samples.first().map(|s|s.game_tick),"game_tick_last":self.samples.last().map(|s|s.game_tick),
            "game_time_first":self.samples.first().map(|s|s.game_time),"game_time_last":self.samples.last().map(|s|s.game_time),
            "renderer_content_render_readback":timing_summary(self.samples.iter().map(|s|s.render_ms).collect()),
            "vulkan_content_render_readback":timing_summary(self.samples.iter().map(|s|s.render_ms).collect()),
            "rgba_conversion_and_egui_enqueue":timing_summary(self.samples.iter().map(|s|s.conversion_enqueue_ms).collect()),
            "hud_commands":timing_summary(self.samples.iter().map(|s|s.hud_ms).collect()),
            "framebuffer_pixels_first":self.samples.first().and_then(|s|s.framebuffer_pixels),
            "framebuffer_pixels_last":self.samples.last().and_then(|s|s.framebuffer_pixels),
            "canvas_pixels_first":self.samples.first().map(|s|s.canvas_pixels),
            "canvas_pixels_last":self.samples.last().map(|s|s.canvas_pixels),
            "offscreen_render_pixels":[1920,1080],"monitor_pixels":self.monitor_size,"monitor_refresh_millihertz":self.refresh_millihertz,
            "pixels_per_point":self.samples.last().map(|s|s.pixels_per_point),
            "gui_graphics":self.gui_graphics,
            "gl_vendor":self.gui_graphics.gl_vendor,"gl_renderer":self.gui_graphics.gl_renderer,"renderer_device":snapshot.map(|s|s.device.as_str()),"vulkan_device":snapshot.map(|s|s.device.as_str())
        }).as_object().unwrap().clone());
        report.as_object_mut().unwrap().extend(serde_json::json!({
            "cpu_identifier":std::env::var("PROCESSOR_IDENTIFIER").ok(),
            "native_vsync_requested":true,"model_paths":self.model_paths,"world_kind":snapshot.map(|s|format!("{:?}",s.world_kind)),
            "enemy_count_min":self.samples.iter().map(|s|s.enemies).min(),"enemy_count_max":self.samples.iter().map(|s|s.enemies).max(),
            "game_status_final":diagnostics.and_then(|value|value.get("status")),"player_health_final":player_health,
            "focused_frames":self.samples.iter().filter(|s|s.focused).count(),"user_input_frames":self.samples.iter().filter(|s|s.user_input).count(),
            "error_frames":self.samples.iter().filter(|s|s.had_error).count(),"last_error":error,"interrupted":interrupted,
            "measurement_scope":"Callback cadence is UI callback start-to-start, including intervening texture upload, draw, swap/vsync and OS scheduling. Player UI/HUD costs aggregate all egui passes. Legacy mean_fps/unique_game_frame_interval use worker completion timestamps of distinct frames observed by UI; worker frames skipped between UI callbacks are not independently counted. UI-observed unique-frame intervals use callback observation times, and age is observation minus worker completion. These separate series do not measure physical presentation, scanout or GPU timestamps. Simulation and selected-renderer costs are last-completed UI-weighted observations; legacy vulkan-named fields alias those renderer measurements."
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

pub fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_rgb(23, 29, 42);
    style.visuals.window_fill = Color32::from_rgb(28, 35, 49);
    style.visuals.selection.bg_fill = Color32::from_rgb(38, 113, 112);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(35, 125, 116);
    style.spacing.item_spacing = Vec2::new(8.0, 7.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::ThemePreference::Dark);
}

#[cfg(test)]
mod native_options_tests {
    use super::*;

    fn options(platform: GuiPlatform) -> eframe::NativeOptions {
        let policy = gui_policy_for(platform, None, None, false).unwrap();
        native_options_for(egui::ViewportBuilder::default(), policy)
    }

    fn backends(options: eframe::NativeOptions) -> eframe::wgpu::Backends {
        assert_eq!(options.renderer, eframe::Renderer::Wgpu);
        let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = options.wgpu_options.wgpu_setup else {
            panic!("Native GUI must create its own independent wgpu setup");
        };
        setup.instance_descriptor.backends
    }

    #[test]
    fn windows_policy_keeps_dx12_and_linux_policy_uses_only_vulkan() {
        // These are pure cross-platform policy checks, not Linux execution.
        assert_eq!(
            backends(options(GuiPlatform::Windows)),
            eframe::wgpu::Backends::DX12
        );
        assert_eq!(
            backends(options(GuiPlatform::Linux)),
            eframe::wgpu::Backends::VULKAN
        );
    }

    #[test]
    fn legacy_capture_cannot_silently_change_the_default_backend() {
        for platform in [GuiPlatform::Windows, GuiPlatform::Linux] {
            for renderer in [None, Some("wgpu")] {
                let error = gui_policy_for(platform, renderer, None, true).unwrap_err();
                assert!(error.contains("EFRAME_SCREENSHOT_TO"));
                assert!(error.contains("MODULE_ENGINE_UI_SCREENSHOT_TO"));
            }
            let policy = gui_policy_for(platform, Some("glow"), None, true).unwrap();
            assert_eq!(policy.renderer, eframe::Renderer::Glow);
            assert_eq!(policy.gpu_backend(), "OpenGL");
        }
    }

    #[test]
    fn window_system_selection_is_independent_of_the_gpu_policy() {
        for (name, window_system) in [
            ("auto", WindowSystem::Auto),
            ("wayland", WindowSystem::Wayland),
            ("x11", WindowSystem::X11),
        ] {
            let policy = gui_policy_for(GuiPlatform::Linux, None, Some(name), false).unwrap();
            assert_eq!(policy.window_system, window_system);
            assert_eq!(policy.renderer, eframe::Renderer::Wgpu);
            assert_eq!(policy.gpu_backend(), "Vulkan");
            let glow = gui_policy_for(GuiPlatform::Linux, Some("glow"), Some(name), false).unwrap();
            assert_eq!(glow.window_system, window_system);
            assert_eq!(glow.gpu_backend(), "OpenGL");
        }
        assert!(gui_policy_for(GuiPlatform::Windows, None, Some("wayland"), false).is_err());
        assert!(gui_policy_for(GuiPlatform::Other, None, Some("x11"), false).is_err());
    }

    #[test]
    fn missing_linux_display_is_rejected_before_gpu_startup() {
        assert!(validate_linux_display(WindowSystem::Auto, false, false).is_err());
        let error = validate_linux_display(WindowSystem::Wayland, false, true).unwrap_err();
        assert!(error.contains("window-system=wayland"));
        assert!(error.contains("WAYLAND_DISPLAY"));
        assert!(validate_linux_display(WindowSystem::X11, true, false).is_err());
        assert!(validate_linux_display(WindowSystem::Auto, true, false).is_ok());
        assert!(validate_linux_display(WindowSystem::Auto, false, true).is_ok());
        assert!(validate_linux_display(WindowSystem::Wayland, true, false).is_ok());
        assert!(validate_linux_display(WindowSystem::X11, false, true).is_ok());
    }

    #[test]
    fn invalid_renderer_and_window_system_values_are_not_ignored() {
        for renderer in ["invalid", "", "dx12", "vulkan"] {
            assert!(gui_policy_for(GuiPlatform::Windows, Some(renderer), None, false).is_err());
        }
        for window_system in ["invalid", "", "dx12", "vulkan"] {
            assert!(gui_policy_for(GuiPlatform::Linux, None, Some(window_system), false).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_linux_options_install_the_winit_window_system_hook() {
        for requested in ["auto", "wayland", "x11"] {
            let policy = gui_policy_for(GuiPlatform::Linux, None, Some(requested), false).unwrap();
            let options = native_options_for(egui::ViewportBuilder::default(), policy);
            assert!(options.event_loop_builder.is_some());
            assert_eq!(backends(options), eframe::wgpu::Backends::VULKAN);
        }
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;
    use std::time::Duration;

    fn benchmark() -> NativeBenchmark {
        NativeBenchmark {
            folder: PathBuf::new(),
            outcome: Arc::new(Mutex::new(None)),
            physical_size: Box::new(|| Some([1920, 1080])),
            monitor_size: None,
            refresh_millihertz: None,
            started: Instant::now(),
            previous: None,
            completed: BENCHMARK_WARMUP,
            samples: vec![],
            model_paths: BTreeSet::new(),
            game_frames: vec![],
            observed_game_frames: vec![],
            frame_ages_ms: vec![],
            missing_completion_timestamps: 0,
            finished: false,
            gui_graphics: GuiGraphicsInfo::default(),
        }
    }

    fn sample(started: Instant) -> NativeFrameSample {
        NativeFrameSample {
            started,
            callback_period_ms: 0.,
            framework_cpu_ms: None,
            player_ui_ms: 0.,
            ui_calls: 1,
            step_ms: 2.,
            render_ms: 3.,
            dispatch_ms: 0.01,
            fresh_frame: true,
            game_tick: 60,
            game_time: 1.,
            conversion_enqueue_ms: 0.5,
            hud_ms: 0.,
            framebuffer_pixels: Some([1920, 1080]),
            canvas_pixels: [1920, 1080],
            pixels_per_point: 1.,
            enemies: 20,
            game_status: "Playing".into(),
            render_success: true,
            focused: true,
            window_visible: Some(true),
            user_input: false,
            had_error: false,
        }
    }

    #[test]
    fn extra_egui_passes_accumulate_into_one_completed_callback_sample() {
        let mut benchmark = benchmark();
        let started = Instant::now();
        benchmark.record_ui_pass(Some(sample(started)), 1., 0.2);
        benchmark.record_ui_pass(None, 3., 0.5);
        benchmark.complete_previous(started + Duration::from_millis(16), Some(0.008));
        assert_eq!(benchmark.samples.len(), 1);
        let completed = &benchmark.samples[0];
        assert_eq!(completed.ui_calls, 2);
        assert_eq!(completed.player_ui_ms, 4.);
        assert!((completed.hud_ms - 0.7).abs() < 1e-9);
        assert_eq!(completed.callback_period_ms, 16.);
        assert_eq!(
            completed.conversion_enqueue_ms, 0.5,
            "texture conversion is dispatched once per UI frame"
        );
        assert_eq!(
            completed.step_ms, 2.,
            "an additional UI pass cannot multiply worker costs"
        );
    }

    #[test]
    fn worker_completion_observation_and_age_remain_distinct_and_missing_time_is_not_fabricated() {
        let mut benchmark = benchmark();
        let start = Instant::now();
        benchmark.observe_game_frame(start + Duration::from_millis(5), Some(start), true);
        benchmark.observe_game_frame(
            start + Duration::from_millis(25),
            Some(start + Duration::from_millis(10)),
            true,
        );
        benchmark.observe_game_frame(
            start + Duration::from_millis(30),
            Some(start + Duration::from_millis(10)),
            false,
        );
        assert_eq!(benchmark.game_frames.len(), 2);
        assert_eq!(benchmark.observed_game_frames.len(), 2);
        assert_eq!(
            benchmark.game_frames[1].duration_since(benchmark.game_frames[0]),
            Duration::from_millis(10)
        );
        assert_eq!(
            benchmark.observed_game_frames[1].duration_since(benchmark.observed_game_frames[0]),
            Duration::from_millis(20)
        );
        assert_eq!(benchmark.frame_ages_ms, [5., 15.]);
        benchmark.observe_game_frame(start + Duration::from_millis(40), None, true);
        assert_eq!(
            benchmark.game_frames.len(),
            2,
            "UI observation time cannot masquerade as worker completion"
        );
        assert_eq!(benchmark.observed_game_frames.len(), 3);
        assert_eq!(benchmark.missing_completion_timestamps, 1);
    }
}

#[cfg(all(test, feature = "arpg"))]
pub use module_engine_arpg::ui::{draw_hud, read_input, screen_point, telegraph_shape};

#[cfg(all(test, feature = "arpg"))]
mod tests {
    use super::*;
    use module_engine_arpg::{ArpgInput, GameInputExt};
    use module_engine_core::{Camera, GameState};

    fn neutral_host() -> GameHost {
        use module_engine_core::*;
        let root = std::env::temp_dir().canonicalize().unwrap();
        let composition =
            CompositionContext::new(ModuleRegistry::builtin(), SystemRegistry::builtin()).unwrap();
        let mut document = demo_document(WorldKind::Ecs);
        document.custom_sources.clear();
        document.root.nodes.retain(|node| {
            matches!(
                node.id.as_str(),
                "world" | "extract" | "renderer" | "viewport"
            )
        });
        document
            .root
            .edges
            .retain(|edge| matches!(edge.from_node.as_str(), "world" | "extract" | "renderer"));
        document.game = Some(GameProject::empty());
        let profile = module_engine_project::EngineProfile::new(
            "fixture.player-ui",
            "1",
            module_engine_project::EngineBlueprint {
                root: document.root.clone(),
                engine: document.engine.clone(),
                custom_sources: document.custom_sources.clone(),
                shader_sources: document.shader_sources.clone(),
                default_execution: document.game.as_ref().unwrap().execution.clone(),
                available_modules: composition
                    .required_contracts(&document)
                    .unwrap()
                    .module_versions
                    .into_keys()
                    .collect(),
            },
        )
        .unwrap();
        document.engine_profile = Some(profile.document_value().unwrap());
        composition
            .capture_lock(&document, profile.identity, None)
            .unwrap()
            .apply_to(&mut document);
        GameHost::build(
            document,
            composition,
            module_engine_host::HostOptions {
                asset_root: root,
                backend_root: None,
                compile_root: None,
                render: module_engine_host::RenderMode::Graph,
                services: module_engine_host_services::HostServiceRegistry::new(),
                services_enabled: false,
            },
            None,
        )
        .unwrap()
    }

    #[test]
    fn player_restart_reaches_paused_full_mailbox_and_cancels_unsent_old_actions() {
        let host = neutral_host();
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
        let game = module_engine_two_arenas::demo_project();
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
