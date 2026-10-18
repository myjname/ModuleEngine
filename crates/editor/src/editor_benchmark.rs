//! Opt-in native Editor diagnostic. Kept as an app child so the same harness can
//! be compiled against older source packages without product instrumentation.
use super::editor_workflow::Command;
use super::{Editor, EditorType, WorkspaceKind, dock, panes};
use crate::distribution::Distribution;
use eframe::{App, egui};
use module_engine_core::GraphDocument;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

const WARMUP_FRAMES: usize = 30;
const MEASURED_FRAMES: usize = 90;
const GRAPH_PANE: u64 = 1;
const GAME_PANE: u64 = 3;
const SCENE_PANE: u64 = 5;

/// Opens the actual native Editor with a fixed Graph/Game/Scene layout. The
/// caller must supply an isolated Full project/package: Editor construction
/// records Recent Projects in that package before this wrapper takes control.
/// No project Save or personal-layout Save is performed by the benchmark.
pub fn run(
    document: GraphDocument,
    path: PathBuf,
    distribution: Distribution,
    folder: &Path,
) -> Result<(), String> {
    if !distribution.can_author_engine() || document.game.is_none() {
        return Err("Native Editor benchmark requires a Full package and an authored game".into());
    }
    for variable in ["EFRAME_SCREENSHOT_TO", "MODULE_ENGINE_UI_SCREENSHOT_TO"] {
        if std::env::var_os(variable).is_some() {
            return Err(format!(
                "Unset {variable} before the native Editor benchmark"
            ));
        }
    }
    let project_bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let folder = folder.canonicalize().map_err(|error| error.to_string())?;
    let outcome = Arc::new(Mutex::new(None));
    let returned = Arc::clone(&outcome);
    let options = module_engine_player::ui::native_options(
        egui::ViewportBuilder::default()
            .with_inner_size([1920., 1080.])
            .with_min_inner_size([1200., 720.])
            .with_position([0., 0.]),
    )?;
    eframe::run_native(
        "ModuleEngine / Native Editor benchmark",
        options,
        Box::new(move |cc| {
            let gui_graphics = module_engine_player::ui::report_graphics(cc);
            super::configure_style(&cc.egui_ctx);
            let bridge = module_engine_player::ui::presentation_bridge(cc)?;
            let mut editor = Editor::new_native_with_diagnostic_personal_root(
                document.clone(),
                path.clone(),
                distribution,
                None,
                bridge,
            );
            editor.workspace = WorkspaceKind::Engine;
            editor.engine_layout = fixed_layout();
            editor.engine_panes.clear();
            editor.engine_navigation = Default::default();
            editor.playing = false;
            editor.personal_dirty = false;
            editor.personal_save_time = f64::MAX;
            for (id, editor_type) in editor.engine_layout.panes() {
                let mut snapshot = panes::PaneSnapshot::default();
                if editor_type == EditorType::Scene {
                    snapshot.camera = crate::scene_view_ui::SceneViewportState::default().camera;
                }
                editor
                    .engine_panes
                    .insert(id, panes::PaneMemory::new(snapshot, &editor.document));
            }
            let native_window = cc.winit_window().cloned();
            let physical_size: Box<dyn Fn() -> Option<[u32; 2]>> = match native_window {
                Some(window) => {
                    let scale = window.scale_factor() as f32;
                    cc.egui_ctx
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
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
            let monitor_pixels = monitor
                .as_ref()
                .map(|monitor| <[u32; 2]>::from(monitor.size()));
            let refresh_millihertz = monitor
                .as_ref()
                .and_then(|monitor| monitor.refresh_rate_millihertz());
            Ok(Box::new(NativeEditorBenchmark {
                editor,
                original: document,
                project_path: path,
                project_bytes,
                folder,
                outcome,
                started: Instant::now(),
                phase: Phase::WarmupStopped,
                warmup: 0,
                warmup_completed: [0; 2],
                measured: [0; 2],
                previous: None,
                samples: vec![],
                last_ui_frame: None,
                total_ui_frames: 0,
                input_phase: None,
                input_step: false,
                pointer: None,
                drag_held: false,
                drag_steps: 0,
                drag_attempts: [0; 2],
                graph_pan_changes: [0; 2],
                identities: None,
                identities_preserved: true,
                play_command_started: false,
                play_actor_id: None,
                stop_command_dispatched: false,
                stop_immediate_paused: false,
                stop_immediate_owner_released: false,
                errors: BTreeSet::new(),
                finished: false,
                physical_size,
                monitor_pixels,
                refresh_millihertz,
                gui_graphics,
            }))
        }),
    )
    .map_err(module_engine_player::ui::gui_startup_error)?;
    returned
        .lock()
        .map_err(|_| "Native Editor benchmark result lock poisoned".to_string())?
        .take()
        .unwrap_or_else(|| Err("Native Editor benchmark closed without a report".into()))
}

fn fixed_layout() -> dock::DockLayout {
    use dock::{Area, Axis};
    dock::DockLayout {
        root: Area::Split {
            id: 2,
            axis: Axis::Horizontal,
            ratio: 0.56,
            first: Box::new(Area::Pane {
                id: GRAPH_PANE,
                editor: EditorType::ModuleGraph,
            }),
            second: Box::new(Area::Split {
                id: 4,
                axis: Axis::Vertical,
                ratio: 0.5,
                first: Box::new(Area::Pane {
                    id: GAME_PANE,
                    editor: EditorType::Game,
                }),
                second: Box::new(Area::Pane {
                    id: SCENE_PANE,
                    editor: EditorType::Scene,
                }),
            }),
        },
        next_id: 6,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    WarmupStopped,
    StoppedPan,
    WarmupPlay,
    PlayPan,
    StoppedAfterPlay,
}
impl Phase {
    fn measured_index(self) -> Option<usize> {
        match self {
            Self::StoppedPan => Some(0),
            Self::PlayPan => Some(1),
            _ => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::WarmupStopped => "stopped-warmup",
            Self::StoppedPan => "stopped-pan",
            Self::WarmupPlay => "play-warmup",
            Self::PlayPan => "play-pan",
            Self::StoppedAfterPlay => "stopped-after-play",
        }
    }
}

#[derive(Serialize)]
struct FrameSample {
    #[serde(skip)]
    started: Option<Instant>,
    phase: &'static str,
    editor_handler_ms: f64,
    editor_ui_calls: usize,
    frame_interval_ms: f64,
    framework_cpu_ms: Option<f64>,
    pan_before: [f32; 2],
    pan_after: [f32; 2],
    focused: bool,
    visible: Option<bool>,
    game_time: Option<f64>,
    game_status: Option<String>,
    living_enemies: Option<usize>,
    game_render_ms_last: Option<f64>,
    scene_render_ms_last: Option<f64>,
    game_frame_ready: bool,
    scene_texture_present: bool,
}

type HostIdentities = BTreeMap<String, usize>;
type Outcome = Arc<Mutex<Option<Result<(), String>>>>;

#[derive(Serialize)]
struct Readiness {
    engine_runtime: bool,
    engine_background_complete: bool,
    game_scene_background_complete: bool,
    game_host: bool,
    scene_host: bool,
    activities_finished: bool,
    game_frame_ready: bool,
    scene_texture_present: bool,
}
impl Readiness {
    fn ready(&self) -> bool {
        self.engine_runtime
            && self.engine_background_complete
            && self.game_scene_background_complete
            && self.game_host
            && self.scene_host
            && self.activities_finished
            && self.game_frame_ready
            && self.scene_texture_present
    }
}

struct NativeEditorBenchmark {
    editor: Editor,
    original: GraphDocument,
    project_path: PathBuf,
    project_bytes: Vec<u8>,
    folder: PathBuf,
    outcome: Outcome,
    started: Instant,
    phase: Phase,
    warmup: usize,
    warmup_completed: [usize; 2],
    measured: [usize; 2],
    previous: Option<FrameSample>,
    samples: Vec<FrameSample>,
    last_ui_frame: Option<u64>,
    total_ui_frames: usize,
    input_phase: Option<Phase>,
    input_step: bool,
    pointer: Option<egui::Pos2>,
    drag_held: bool,
    drag_steps: usize,
    drag_attempts: [usize; 2],
    graph_pan_changes: [usize; 2],
    identities: Option<HostIdentities>,
    identities_preserved: bool,
    play_command_started: bool,
    play_actor_id: Option<u64>,
    stop_command_dispatched: bool,
    stop_immediate_paused: bool,
    stop_immediate_owner_released: bool,
    errors: BTreeSet<String>,
    finished: bool,
    physical_size: Box<dyn Fn() -> Option<[u32; 2]>>,
    monitor_pixels: Option<[u32; 2]>,
    refresh_millihertz: Option<u32>,
    gui_graphics: module_engine_player::ui::GuiGraphicsInfo,
}

impl NativeEditorBenchmark {
    fn graph_pan(&self) -> [f32; 2] {
        self.editor
            .engine_panes
            .get(&GRAPH_PANE)
            .map_or([0.; 2], |pane| {
                [pane.graph_canvas.pan.x, pane.graph_canvas.pan.y]
            })
    }

    fn host_identities(&self) -> HostIdentities {
        let mut identities = BTreeMap::new();
        if let Some(snapshot) = self.editor.game_workbench.game_snapshot() {
            identities.insert("game/worker".into(), snapshot.actor_id as usize);
            for instance in &snapshot.custom {
                identities.insert(
                    format!("game/{}", instance.instance),
                    instance.identity as usize,
                );
            }
        }
        for (name, host) in [
            ("game", &self.editor.game_workbench.host),
            ("scene", &self.editor.game_workbench.preview_host),
        ] {
            if let Some(host) = host {
                for (id, slot) in &host.slots {
                    identities.insert(format!("{name}/{id}"), Arc::as_ptr(slot) as usize);
                }
            }
        }
        identities
    }

    fn readiness(&self) -> Readiness {
        Readiness {
            engine_runtime: self.editor.runtime.is_some(),
            engine_background_complete: !self.editor.preview_builder.is_pending(),
            game_scene_background_complete: !self.editor.game_workbench.has_pending_preparation(),
            game_host: self.editor.game_workbench.has_game_host(),
            scene_host: self.editor.game_workbench.preview_host.is_some(),
            activities_finished: self
                .editor
                .preparation_activities()
                .iter()
                .all(|activity| activity.state != crate::activity::ActivityState::Running),
            // GameWorkbench owns its private texture; PaneMemory.texture is
            // used by the legacy viewport. A successful real GameHost render
            // sets this duration, including in the older comparison package.
            game_frame_ready: self
                .editor
                .game_workbench
                .game_snapshot()
                .is_some_and(|snapshot| snapshot.frame.is_some() && snapshot.last_render_ms > 0.),
            scene_texture_present: self
                .editor
                .engine_panes
                .get(&SCENE_PANE)
                .is_some_and(|pane| pane.scene_view.texture_id().is_some()),
        }
    }

    fn ready(&self) -> bool {
        self.readiness().ready()
    }

    fn stopped_after_play_ready(&self) -> bool {
        self.phase == Phase::StoppedAfterPlay
            && self.ready()
            && !self.editor.playing
            && self
                .play_actor_id
                .zip(self.editor.game_workbench.game_snapshot())
                .is_some_and(|(play_actor, snapshot)| {
                    snapshot.state.time == 0. && snapshot.actor_id != play_actor
                })
    }

    fn collect_errors(&mut self) {
        if let Some(error) = &self.editor.runtime_error {
            self.errors.insert(error.clone());
        }
        if let Some(error) = &self.editor.game_workbench.error {
            self.errors.insert(error.clone());
        }
        for activity in self.editor.preparation_activities() {
            if let Some(error) = activity.error {
                self.errors.insert(error);
            }
        }
        for pane in self.editor.engine_panes.values() {
            if let Some(error) = &pane.scene_view.error {
                self.errors.insert(error.clone());
            }
        }
    }

    fn complete_previous(&mut self, now: Instant, cpu: Option<f32>) {
        if let Some(mut previous) = self.previous.take() {
            previous.frame_interval_ms = now
                .duration_since(previous.started.take().unwrap())
                .as_secs_f64()
                * 1000.;
            previous.framework_cpu_ms = cpu.map(|seconds| seconds as f64 * 1000.);
            let index = usize::from(previous.phase == "play-pan");
            self.measured[index] += 1;
            self.samples.push(previous);
        }
    }

    fn finish(&mut self, interrupted: Option<&str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let document_unchanged = self.editor.document == self.original;
        let execution_unchanged = self.editor.document.execution_eq(&self.original);
        let project_file_unchanged =
            std::fs::read(&self.project_path).is_ok_and(|bytes| bytes == self.project_bytes);
        let complete = self.measured == [MEASURED_FRAMES; 2] && interrupted.is_none();
        let play_times: Vec<_> = self
            .samples
            .iter()
            .filter(|sample| sample.phase == "play-pan")
            .filter_map(|sample| sample.game_time)
            .collect();
        let simulation_advanced = play_times
            .first()
            .zip(play_times.last())
            .is_some_and(|(first, last)| last > first);
        let stop_verified = self.play_command_started
            && self.stop_command_dispatched
            && self.stop_immediate_paused
            && self.stop_immediate_owner_released
            && self.stopped_after_play_ready();
        let measurements_valid = complete
            && self.errors.is_empty()
            && document_unchanged
            && execution_unchanged
            && project_file_unchanged
            && self.identities_preserved
            && simulation_advanced
            && stop_verified
            && self.samples.iter().all(|sample| {
                sample.visible != Some(false)
                    && sample.game_frame_ready
                    && sample.scene_texture_present
            });
        let summaries: Vec<_> = ["stopped-pan", "play-pan"].into_iter().map(|phase| {
            let samples: Vec<_> = self.samples.iter().filter(|sample| sample.phase == phase).collect();
            serde_json::json!({
                "phase":phase,
                "frames":samples.len(),
                "editor_handler_ms":summary(samples.iter().map(|sample|sample.editor_handler_ms).collect()),
                "frame_interval_ms":summary(samples.iter().map(|sample|sample.frame_interval_ms).collect()),
                "framework_cpu_ms":summary(samples.iter().filter_map(|sample|sample.framework_cpu_ms).collect()),
                "game_time_first":samples.first().and_then(|sample|sample.game_time),
                "game_time_last":samples.last().and_then(|sample|sample.game_time),
                "game_status_first":samples.first().and_then(|sample|sample.game_status.as_deref()),
                "game_status_last":samples.last().and_then(|sample|sample.game_status.as_deref()),
                "playing_game_status_frames":samples.iter().filter(|sample|sample.game_status.as_deref() == Some("Playing")).count(),
                "living_enemies_min":samples.iter().filter_map(|sample|sample.living_enemies).min(),
                "living_enemies_max":samples.iter().filter_map(|sample|sample.living_enemies).max(),
                "focused_frames":samples.iter().filter(|sample|sample.focused).count(),
                "visible_frames":samples.iter().filter(|sample|sample.visible == Some(true)).count()
            })
        }).collect();
        let report = serde_json::json!({
            "schema_version":1,"kind":"native-editor-graph-pan","complete":complete,"measurements_valid":measurements_valid,
            "document_name":self.original.name,"project_path":self.project_path,"project_saved":false,
            "document_unchanged":document_unchanged,"document_execution_unchanged":execution_unchanged,
            "project_file_unchanged":project_file_unchanged,"personal_layout_autosave_disabled":true,
            "requested_frames_per_phase":MEASURED_FRAMES,"completed_frames":self.measured,
            "warmup_requested_per_phase":WARMUP_FRAMES,"warmup_completed":self.warmup_completed,
            "total_ui_frames":self.total_ui_frames,"readiness":self.readiness(),
            "game_frame_evidence":"Completed worker snapshot contains real Game pixels and last_render_ms > 0; GameWorkbench uploads that snapshot without waiting for simulation",
            "pan_input_attempts":self.drag_attempts,"actual_pan_changes":self.graph_pan_changes,
            "host_custom_slot_arc_identities_preserved":self.identities_preserved,
            "custom_program_store":self.editor.game_workbench.program_store_stats(),
            "game_simulation_advanced_during_play_samples":simulation_advanced,
            "play_stop":{
                "verified":stop_verified,
                "play_command_started":self.play_command_started,
                "play_actor_id":self.play_actor_id,
                "stop_command_dispatched":self.stop_command_dispatched,
                "stop_immediate_paused":self.stop_immediate_paused,
                "stop_immediate_owner_released":self.stop_immediate_owner_released,
                "stopped_host_ready":self.phase == Phase::StoppedAfterPlay && self.ready(),
                "stopped_paused":self.phase == Phase::StoppedAfterPlay && !self.editor.playing,
                "stopped_actor_id":self.editor.game_workbench.game_snapshot().map(|snapshot|snapshot.actor_id),
                "stopped_time":self.editor.game_workbench.game_snapshot().map(|snapshot|snapshot.state.time),
                "scope":"Native Editor invokes the same PlayPause and Stop command dispatcher as the toolbar; this does not verify external pointer clicks. Stop must release the old Game owner and prepare a new stopped worker at time zero."
            },
            "host_identity_slots":self.identities.as_ref().map(BTreeMap::len),
            "host_identity_scope":"During stopped/play measurements, Game worker actor_id and opaque Custom slot Arc identities published by the worker, plus Scene Custom slot Arc identities; UI never locks a live Game Custom slot. The intentional final Stop releases the Game owner, so the new stopped worker must have a different actor_id.",
            "layout":{"graph":GRAPH_PANE,"game":GAME_PANE,"scene":SCENE_PANE,"graph_width_ratio":0.56,"game_scene_split":0.5},
            "framebuffer_pixels_final":(self.physical_size)(),"monitor_pixels":self.monitor_pixels,
            "monitor_refresh_millihertz":self.refresh_millihertz,"vsync_requested":true,
            "gui_graphics":self.gui_graphics,
            "vulkan_device":self.editor.game_workbench.game_snapshot().map(|snapshot|snapshot.device.clone()),
            "errors":self.errors,"interrupted":interrupted,"elapsed_seconds":self.started.elapsed().as_secs_f64(),
            "phase_summaries":summaries,"frames":self.samples,
            "measurement_scope":"editor_handler_ms times the real Editor::ui call, all open native editor panes and completed-frame texture enqueue. Game simulation and Game Vulkan rendering run on their persistent worker; the independent authored Scene viewport still renders on demand. Frame interval is Editor callback start-to-start, including previous egui texture upload/draw, present/vsync and scheduling. framework_cpu_ms is eframe previous-frame CPU, including GUI rendering but excluding vsync waiting. No headless/FPS substitute or display scanout timing. Only actual middle-button pointer moves that changed graph pan are sampled; preparation, warmup and drag acquisition are excluded. Last render durations/texture presence are diagnostics, not proof of one fresh GPU render per sample."
        });
        let result = serde_json::to_vec_pretty(&report).map_err(|error|error.to_string())
            .and_then(|bytes|std::fs::write(self.folder.join("report.json"),bytes).map_err(|error|error.to_string()))
            .and_then(|()|if measurements_valid { Ok(()) } else { Err(interrupted.unwrap_or("Native Editor benchmark reported incomplete/invalid measurements; inspect report.json").into()) });
        if let Ok(mut outcome) = self.outcome.lock() {
            *outcome = Some(result);
        }
    }
}

impl App for NativeEditorBenchmark {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.finished && self.started.elapsed().as_secs() >= 45 {
            self.finish(Some(
                "45-second watchdog: preparation or native frames did not complete",
            ));
        }
        if self.finished {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        // Ignore external clicks/keys during this scripted diagnostic so no
        // project commands, text edits or layout changes can be triggered.
        input.events.clear();
        self.input_step = false;
        self.input_phase = None;
        if let Some(index) = self.phase.measured_index().filter(|_| !self.finished) {
            if let Some(base) = self.pointer {
                let offset = if self.drag_held {
                    self.drag_steps += 1;
                    egui::vec2(
                        (self.drag_steps % 40) as f32 * 4.,
                        ((self.drag_steps / 40) % 2) as f32 * 4.,
                    )
                } else {
                    egui::Vec2::ZERO
                };
                let pos = base + offset;
                input.events.push(egui::Event::PointerMoved(pos));
                if !self.drag_held {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Middle,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    });
                    self.drag_held = true;
                } else {
                    self.input_step = true;
                    self.input_phase = Some(self.phase);
                    self.drag_attempts[index] += 1;
                }
            }
        } else if self.drag_held {
            if let Some(pos) = self.pointer {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Middle,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            self.drag_held = false;
            self.drag_steps = 0;
        }
        self.editor.raw_input_hook(ctx, input);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.finished {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let start = Instant::now();
        let ui_frame = ui.ctx().cumulative_frame_nr();
        let new_frame = self.last_ui_frame != Some(ui_frame);
        if new_frame {
            self.last_ui_frame = Some(ui_frame);
            self.total_ui_frames += 1;
            self.complete_previous(start, frame.info().cpu_usage);
            if self.phase == Phase::StoppedPan && self.measured[0] == MEASURED_FRAMES {
                self.phase = Phase::WarmupPlay;
                self.editor.dispatch_command(Command::PlayPause, ui.ctx());
                self.play_command_started = self.editor.playing;
                if !self.play_command_started {
                    self.errors
                        .insert("Play command did not start the native game session".into());
                }
                self.warmup = 0;
            } else if self.phase == Phase::PlayPan && self.measured[1] == MEASURED_FRAMES {
                self.play_actor_id = self
                    .editor
                    .game_workbench
                    .game_snapshot()
                    .map(|snapshot| snapshot.actor_id);
                if self.play_actor_id.is_none() {
                    self.errors
                        .insert("Play completed without a game worker snapshot".into());
                }
                self.editor.dispatch_command(Command::Stop, ui.ctx());
                self.stop_command_dispatched = true;
                self.stop_immediate_paused = !self.editor.playing;
                self.stop_immediate_owner_released = !self.editor.game_workbench.has_game_host();
                if !self.stop_immediate_paused {
                    self.errors
                        .insert("Stop command did not pause Editor immediately".into());
                }
                if !self.stop_immediate_owner_released {
                    self.errors
                        .insert("Stop command retained the Game host owner".into());
                }
                self.phase = Phase::StoppedAfterPlay;
            }
        }
        // No personal state is written even though actual canvas panning marks
        // navigation dirty inside the unmodified Editor callback.
        self.editor.personal_dirty = false;
        self.editor.personal_save_time = f64::MAX;
        let rect = ui.available_rect_before_wrap();
        self.pointer = Some(egui::pos2(rect.left() + 64., rect.bottom() - 100.));
        let before = self.graph_pan();
        let handler_started = Instant::now();
        self.editor.ui(ui, frame);
        let handler_ms = handler_started.elapsed().as_secs_f64() * 1000.;
        self.editor.personal_dirty = false;
        let after = self.graph_pan();
        if !new_frame {
            if let Some(previous) = &mut self.previous {
                previous.editor_handler_ms += handler_ms;
                previous.editor_ui_calls += 1;
                previous.pan_after = after;
            }
            return;
        }
        self.collect_errors();
        if self.original != self.editor.document {
            self.errors
                .insert("Benchmark unexpectedly changed the authored document".into());
        }
        if self.phase == Phase::StoppedAfterPlay {
            if self.stopped_after_play_ready() {
                self.finish(None);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else if self.ready() {
                self.errors.insert(
                    "Stopped Game must remain paused and publish a new worker at time zero".into(),
                );
            }
            ui.ctx().request_repaint();
            return;
        }
        if let Some(identities) = &self.identities {
            self.identities_preserved &= *identities == self.host_identities();
        }
        if self.phase.measured_index().is_none() {
            if self.ready() {
                self.warmup += 1;
                let index = usize::from(self.phase == Phase::WarmupPlay);
                self.warmup_completed[index] = self.warmup;
                if self.warmup >= WARMUP_FRAMES {
                    if self.identities.is_none() {
                        self.identities = Some(self.host_identities());
                    }
                    self.phase = if index == 0 {
                        Phase::StoppedPan
                    } else {
                        Phase::PlayPan
                    };
                }
            } else {
                self.warmup = 0;
            }
        } else if self.input_step && self.input_phase == Some(self.phase) && before != after {
            let index = self.phase.measured_index().unwrap();
            self.graph_pan_changes[index] += 1;
            let game_frame_ready = self
                .editor
                .game_workbench
                .game_snapshot()
                .is_some_and(|snapshot| snapshot.frame.is_some() && snapshot.last_render_ms > 0.);
            let scene_texture_present = self
                .editor
                .engine_panes
                .get(&SCENE_PANE)
                .is_some_and(|pane| pane.scene_view.texture_id().is_some());
            self.previous =
                Some(FrameSample {
                    started: Some(start),
                    phase: self.phase.name(),
                    editor_handler_ms: handler_ms,
                    editor_ui_calls: 1,
                    frame_interval_ms: 0.,
                    framework_cpu_ms: None,
                    pan_before: before,
                    pan_after: after,
                    focused: ui.input(|input| input.focused),
                    visible: ui.input(|input| input.viewport().visible()),
                    game_time: self
                        .editor
                        .game_workbench
                        .game_snapshot()
                        .map(|snapshot| snapshot.state.time),
                    game_status: self
                        .editor
                        .game_workbench
                        .game_snapshot()
                        .and_then(|snapshot| {
                            snapshot
                                .game_diagnostics
                                .get("status")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        }),
                    living_enemies: self.editor.game_workbench.game_snapshot().and_then(
                        |snapshot| {
                            snapshot
                                .game_diagnostics
                                .get("active_enemies")
                                .and_then(serde_json::Value::as_u64)
                                .map(|value| value as usize)
                        },
                    ),
                    game_render_ms_last: self
                        .editor
                        .game_workbench
                        .game_snapshot()
                        .map(|snapshot| snapshot.last_render_ms),
                    scene_render_ms_last: self
                        .editor
                        .game_workbench
                        .preview_host
                        .as_ref()
                        .map(|host| host.last_render_ms),
                    game_frame_ready,
                    scene_texture_present,
                });
        }
        ui.ctx().request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.finished {
            self.finish(Some(
                "Native Editor window closed before all measured frames completed",
            ));
        }
    }
}

#[derive(Serialize)]
struct TimingSummary {
    samples: usize,
    mean_ms: f64,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}
fn summary(mut values: Vec<f64>) -> Option<TimingSummary> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let count = values.len();
    Some(TimingSummary {
        samples: count,
        mean_ms: values.iter().sum::<f64>() / count as f64,
        median_ms: if count.is_multiple_of(2) {
            (values[count / 2 - 1] + values[count / 2]) * 0.5
        } else {
            values[count / 2]
        },
        p95_ms: values[((count as f64 * 0.95).ceil() as usize).saturating_sub(1)],
        max_ms: values[count - 1],
    })
}
