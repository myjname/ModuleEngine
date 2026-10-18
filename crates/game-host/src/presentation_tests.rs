//! CPU presentation contracts plus an explicitly named real Custom-worker case.
use super::*;
use std::collections::BTreeSet;

#[derive(Default)]
struct Draws {
    fail: bool,
    snapshots: Vec<SceneSnapshot>,
    batches: Vec<Vec<RenderResourceCommand>>,
}
struct Recorder(Arc<Mutex<Draws>>);
impl FrameRenderer for Recorder {
    fn render(
        &mut self,
        scene: &SceneSnapshot,
        camera: Camera,
        w: usize,
        h: usize,
        controls: &RenderOverrides,
    ) -> Result<Frame, String> {
        let mut draws = self.0.lock().unwrap();
        if draws.fail {
            return Err("injected presentation failure".into());
        }
        draws.snapshots.push(scene.clone());
        draws.batches.push(controls.resource_commands.clone());
        Ok(render_scene(scene, w, h, camera))
    }
    fn backend_name(&self) -> &str {
        "presentation CPU recorder"
    }
}
struct Move;
impl ExecutableSystem for Move {
    fn step(&mut self, ctx: &mut SystemContext<'_>, _: &mut Value) -> Result<(), String> {
        if ctx.input.pressed("move") {
            ctx.game.world.transform_mut(1).unwrap().translation[0] += 1.;
        }
        if ctx.input.pressed("respawn") {
            ctx.game.apply_commands(&[
                WorldCommand::Remove { id: 1 },
                WorldCommand::Spawn {
                    object: WorldObject {
                        id: 1,
                        parent: None,
                        transform: Transform {
                            translation: [8., 0., 0.],
                            ..Default::default()
                        },
                        color: [200, 120, 50, 255],
                    },
                },
            ])?;
        }
        if ctx.input.pressed("reset") {
            ctx.events.push(GameEvent::presentation_reset());
        }
        if ctx.input.pressed("reject") {
            return Err("injected tick rejection".into());
        }
        if ctx.input.pressed("bad_material") {
            ctx.game.world.insert_component(
                1,
                "material",
                json!({
                    "base_color": [1., 1., 1., 1.],
                    "base_color_texture": "assets/missing-successor-texture.png"
                }),
            )?;
        }
        Ok(())
    }
}
fn input(actions: &[&str]) -> GameInput {
    GameInput {
        actions: actions.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}
fn host() -> (
    GameHost,
    Arc<Mutex<Draws>>,
    module_engine_script::TestArtifactGuard,
) {
    let temp = module_engine_script::TestArtifactGuard::new(&std::env::temp_dir(), "presentation")
        .unwrap();
    let mut systems = SystemRegistry::builtin();
    systems
        .register(
            SystemDescriptor {
                id: "fixture.presentation_move".into(),
                state_schema: "fixture.presentation_move/1".into(),
                ports: vec![],
                components: vec![],
                requires: vec![],
            },
            |_| Ok(Box::new(Move)),
        )
        .unwrap();
    let composition = CompositionContext::new(ModuleRegistry::builtin(), systems).unwrap();
    let mut document = demo_document(WorldKind::Ecs);
    document.custom_sources.clear();
    document
        .root
        .nodes
        .retain(|n| matches!(n.id.as_str(), "world" | "extract" | "renderer" | "viewport"));
    document
        .root
        .edges
        .retain(|e| matches!(e.from_node.as_str(), "world" | "extract" | "renderer"));
    let mut game = GameProject::empty();
    game.scenes
        .get_mut("empty")
        .unwrap()
        .objects
        .push(AuthoredObject::new(WorldObject {
            id: 1,
            parent: None,
            transform: Transform::default(),
            color: [200, 120, 50, 255],
        }));
    game.execution.stages.push(SystemStage {
        name: "move".into(),
        nodes: vec![SystemNode {
            id: "move".into(),
            module: "fixture.presentation_move".into(),
            params: json!({}),
            bindings: BTreeMap::new(),
            after: vec![],
        }],
    });
    document.game = Some(game);
    composition
        .capture_lock(
            &document,
            ProfileIdentity::from_payload("fixture.presentation", "1", &json!({})).unwrap(),
            Some(SdkIdentity::current()),
        )
        .unwrap()
        .apply_to(&mut document);
    let mut host = GameHost::build(
        document,
        composition,
        HostOptions {
            asset_root: temp.path().into(),
            backend_root: None,
            compile_root: None,
            render: RenderMode::Graph,
            services: HostServiceRegistry::new(),
            services_enabled: false,
        },
        None,
    )
    .unwrap();
    let draws = Arc::new(Mutex::new(Draws::default()));
    host.renderer = Some(Box::new(Recorder(draws.clone())));
    (host, draws, temp)
}

#[test]
fn interpolation_renders_real_midpoints_without_advancing_gameplay_and_deduplicates() {
    let (mut host, draws, _temp) = host();
    host.step_budget(1. / 60., &input(&["move"]), 1).unwrap();
    let state = serde_json::to_value(host.session.state()).unwrap();
    let events = host.session.events().to_vec();
    let a = host
        .try_render_interpolated(160, 90, 0.25)
        .unwrap()
        .unwrap();
    let b = host
        .try_render_interpolated(160, 90, 0.75)
        .unwrap()
        .unwrap();
    assert_eq!(b.provenance.origin, PresentationOrigin::InterpolatedPose);
    assert_eq!(
        (b.provenance.previous_tick, b.provenance.current_tick),
        (0, 1)
    );
    assert_ne!(
        a.frame.rgba, b.frame.rgba,
        "Real CPU pixels must move between actual poses"
    );
    {
        let draws = draws.lock().unwrap();
        assert!((draws.snapshots[0].items[0].model[0][3] - 0.25).abs() < 1e-5);
        assert!((draws.snapshots[1].items[0].model[0][3] - 0.75).abs() < 1e-5);
    }
    assert_eq!(serde_json::to_value(host.session.state()).unwrap(), state);
    assert_eq!(host.session.events(), events);
    assert_eq!(host.author_snapshot().unwrap().items[0].model[0][3], 1.);
    assert!(
        host.try_render_interpolated(160, 90, 0.75)
            .unwrap()
            .is_none()
    );
    assert!(host.try_render_interpolated(160, 90, 2.).unwrap().is_some());
    assert!(host.try_render_interpolated(160, 90, 3.).unwrap().is_none());
}

// These fixtures exercise only the neutral Host contract. A GPU lease marker is
// deliberately not a native image or evidence of any graphics API execution.
#[derive(Debug)]
struct FixtureGpuLease {
    serial: u64,
    drops: Arc<std::sync::atomic::AtomicUsize>,
}
impl GpuFrame for FixtureGpuLease {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
impl Drop for FixtureGpuLease {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
struct FixturePresentationTarget(u64);
impl PresentationTarget for FixturePresentationTarget {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
#[derive(Default)]
pub(crate) struct GpuDraws {
    busy: bool,
    reject_target: Option<u64>,
    accepted_target: Option<Arc<dyn PresentationTarget>>,
    target_attempts: usize,
    cpu_calls: usize,
    pub(crate) attempts: usize,
    completed: Vec<(Camera, Vec<RenderResourceCommand>)>,
    lease_drops: Arc<std::sync::atomic::AtomicUsize>,
    enqueue_after_success: Option<(Arc<Mutex<RenderOverrides>>, RenderResourceCommand)>,
    pub(crate) delayed: bool,
    pub(crate) ready: bool,
    pub(crate) fail_poll: bool,
    fail_submit: bool,
    pub(crate) polls: usize,
    pub(crate) discarded: Vec<u64>,
    pending: Option<(u64, Frame)>,
}
struct GpuRecorder(Arc<Mutex<GpuDraws>>);
impl FrameRenderer for GpuRecorder {
    fn render(
        &mut self,
        _: &SceneSnapshot,
        _: Camera,
        _: usize,
        _: usize,
        _: &RenderOverrides,
    ) -> Result<Frame, String> {
        self.0.lock().unwrap().cpu_calls += 1;
        Err("fixture GPU renderer forbids a silent CPU fallback".into())
    }
    fn set_presentation_target(
        &mut self,
        target: Option<Arc<dyn PresentationTarget>>,
    ) -> Result<(), String> {
        let mut draws = self.0.lock().unwrap();
        draws.target_attempts += 1;
        if let Some(target) = &target {
            let target = target
                .as_any()
                .downcast_ref::<FixturePresentationTarget>()
                .ok_or("fixture target type")?;
            if draws.reject_target == Some(target.0) {
                return Err("injected target rejection".into());
            }
        }
        draws.accepted_target = target;
        Ok(())
    }
    fn render_presented(
        &mut self,
        _: &SceneSnapshot,
        camera: Camera,
        width: usize,
        height: usize,
        controls: &RenderOverrides,
    ) -> Result<RenderSubmission, String> {
        let mut draws = self.0.lock().unwrap();
        draws.attempts += 1;
        if draws.fail_submit {
            return Err("injected successor admission failure".into());
        }
        if draws.busy || draws.pending.is_some() {
            return Ok(RenderSubmission::Busy);
        }
        draws
            .completed
            .push((camera, controls.resource_commands.clone()));
        let serial = draws.completed.len() as u64;
        let drops = draws.lease_drops.clone();
        let enqueue = draws.enqueue_after_success.take();
        let delayed = draws.delayed;
        drop(draws);
        if let Some((overrides, command)) = enqueue {
            overrides.lock().unwrap().resource_commands.push(command);
        }
        let frame = Frame {
            width,
            height,
            rgba: Vec::new(),
            stats: RenderStats::default(),
            gpu: Some(Arc::new(FixtureGpuLease { serial, drops })),
        };
        if delayed {
            self.0.lock().unwrap().pending = Some((serial, frame));
            Ok(RenderSubmission::Pending(serial))
        } else {
            Ok(RenderSubmission::Ready(frame))
        }
    }
    fn poll_presented(&mut self, ticket: u64) -> Result<Option<Frame>, String> {
        let mut draws = self.0.lock().unwrap();
        draws.polls += 1;
        if draws.fail_poll {
            return Err("injected asynchronous completion failure".into());
        }
        if draws.pending.as_ref().map(|(id, _)| *id) != Some(ticket) {
            return Err("fixture unknown presentation ticket".into());
        }
        if !draws.ready {
            return Ok(None);
        }
        Ok(draws.pending.take().map(|(_, frame)| frame))
    }
    fn discard_presented(&mut self, ticket: u64) -> Result<(), String> {
        let mut draws = self.0.lock().unwrap();
        draws.discarded.push(ticket);
        if draws.pending.as_ref().map(|(id, _)| *id) == Some(ticket) {
            draws.pending = None;
        }
        Ok(())
    }
    fn backend_name(&self) -> &str {
        "fixture GPU lease recorder"
    }
}

pub(crate) fn gpu_host() -> (
    GameHost,
    Arc<Mutex<GpuDraws>>,
    module_engine_script::TestArtifactGuard,
) {
    let (mut host, _, temp) = host();
    let draws = Arc::new(Mutex::new(GpuDraws::default()));
    host.renderer = Some(Box::new(GpuRecorder(draws.clone())));
    host.set_presentation_target(Some(Arc::new(FixturePresentationTarget(1))))
        .unwrap();
    (host, draws, temp)
}

fn completed_evaluation(host: &GameHost) -> Value {
    let evaluation = host.presentation_evaluation.as_ref().unwrap();
    json!({"generation":evaluation.generation,"tick":evaluation.tick,"revision":evaluation.revision,
        "size":evaluation.size,"source_camera":evaluation.source_camera,"camera":evaluation.camera,
        "models":evaluation.models,"controls":evaluation.controls})
}

fn queue_gpu_resource_batch(host: &GameHost) -> Vec<RenderResourceCommand> {
    host.overrides
        .lock()
        .unwrap()
        .apply_commands(&[
            RenderCommand::ClearResource {
                name: "source.color".into(),
                value: [0.25, 0.5, 0.75, 1.],
            },
            RenderCommand::CopyResource {
                from: "source.color".into(),
                to: "output.color".into(),
            },
        ])
        .unwrap();
    host.overrides.lock().unwrap().resource_commands.clone()
}

#[test]
fn gpu_backpressure_preserves_interpolated_evaluation_and_commands_until_real_lease() {
    let (mut host, draws, _temp) = gpu_host();
    host.contributions.push(follow_contribution());
    host.camera.target = host.frame_target().unwrap();
    host.step_budget(1. / 60., &input(&["move"]), 1).unwrap();
    let first = host.try_render_interpolated(80, 45, 0.25).unwrap().unwrap();
    assert!(first.frame.is_valid());
    assert!(first.frame.rgba.is_empty());
    assert_eq!(first.camera.target, [0.25, 0., 0.]);
    let evaluation = completed_evaluation(&host);
    let provenance = serde_json::to_value(&first.provenance).unwrap();
    let state = serde_json::to_value(host.session.state()).unwrap();
    let source_camera = host.camera;
    let commands = queue_gpu_resource_batch(&host);
    draws.lock().unwrap().busy = true;
    host.last_render_ms = 123.;
    assert!(
        host.try_render_interpolated(80, 45, 0.75)
            .unwrap()
            .is_none()
    );
    assert_eq!(completed_evaluation(&host), evaluation);
    assert_eq!(host.overrides.lock().unwrap().resource_commands, commands);
    assert_eq!(host.last_render_ms, 123.);
    assert_eq!(host.camera, source_camera);
    assert_eq!(serde_json::to_value(host.session.state()).unwrap(), state);
    assert_eq!(serde_json::to_value(&first.provenance).unwrap(), provenance);
    assert_eq!(first.camera.target, [0.25, 0., 0.]);
    assert_eq!(draws.lock().unwrap().lease_drops.load(Ordering::SeqCst), 0);

    draws.lock().unwrap().busy = false;
    let next = host.try_render_interpolated(80, 45, 0.75).unwrap().unwrap();
    assert_eq!(next.camera.target, [0.75, 0., 0.]);
    assert_eq!(next.provenance.alpha, 0.75);
    assert_eq!(next.provenance.origin, PresentationOrigin::Invalidated);
    assert_eq!(next.provenance.current_tick, first.provenance.current_tick);
    assert_eq!(next.provenance.generation, first.provenance.generation);
    assert_ne!(completed_evaluation(&host), evaluation);
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    assert!(next.frame.is_valid());
    assert!(next.frame.rgba.is_empty());
    assert_eq!(
        next.frame
            .gpu
            .as_ref()
            .unwrap()
            .as_any()
            .downcast_ref::<FixtureGpuLease>()
            .unwrap()
            .serial,
        2
    );
    assert!(
        host.try_render_interpolated(80, 45, 0.75)
            .unwrap()
            .is_none()
    );
    {
        let draws = draws.lock().unwrap();
        assert_eq!(draws.cpu_calls, 0);
        assert_eq!(
            draws.attempts, 3,
            "deduplication must not submit a duplicate GPU frame"
        );
        assert_eq!(draws.completed.len(), 2);
        assert_eq!(draws.completed[1].1, commands);
    }
    let consumer = next.frame.gpu.clone();
    drop(next);
    assert_eq!(draws.lock().unwrap().lease_drops.load(Ordering::SeqCst), 0);
    drop(consumer);
    assert_eq!(draws.lock().unwrap().lease_drops.load(Ordering::SeqCst), 1);
    drop(first);
    assert_eq!(draws.lock().unwrap().lease_drops.load(Ordering::SeqCst), 2);
}

#[test]
fn gpu_authored_view_backpressure_consumes_each_resource_batch_once() {
    let (mut host, draws, _temp) = gpu_host();
    let first_batch = queue_gpu_resource_batch(&host);
    draws.lock().unwrap().busy = true;
    host.last_render_ms = 321.;
    assert!(host.render_presented(96, 54).unwrap().is_none());
    assert_eq!(
        host.overrides.lock().unwrap().resource_commands,
        first_batch
    );
    assert_eq!(host.last_render_ms, 321.);
    let later = RenderResourceCommand::ClearResource {
        name: "later.color".into(),
        value: [1.; 4],
    };
    {
        let mut draws = draws.lock().unwrap();
        draws.busy = false;
        draws.enqueue_after_success = Some((host.overrides.clone(), later.clone()));
    }
    let frame = host.render_presented(96, 54).unwrap().unwrap();
    assert!(frame.is_valid());
    assert!(frame.rgba.is_empty());
    assert_eq!(
        host.overrides.lock().unwrap().resource_commands,
        vec![later.clone()],
        "a command accepted after the submitted prefix belongs to the next frame"
    );
    let next = host.render_presented(96, 54).unwrap().unwrap();
    assert!(next.is_valid());
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    host.render_presented(96, 54).unwrap().unwrap();
    let draws = draws.lock().unwrap();
    assert_eq!(draws.cpu_calls, 0);
    assert_eq!(draws.attempts, 4);
    assert_eq!(
        draws
            .completed
            .iter()
            .map(|(_, commands)| commands.clone())
            .collect::<Vec<_>>(),
        vec![first_batch, vec![later], vec![]]
    );
}

#[test]
fn pending_authored_frame_keeps_camera_and_submits_changed_inputs_on_completion_turn() {
    let (mut host, draws, _temp) = gpu_host();
    let camera = host.camera;
    let revision = host.presentation_revision();
    let first_batch = queue_gpu_resource_batch(&host);
    draws.lock().unwrap().delayed = true;
    host.last_render_ms = 321.;
    assert!(host.render_presented(80, 45).unwrap().is_none());
    assert!(host.has_pending_presentation());
    assert_eq!(host.last_presented_camera(), None);
    assert_eq!(host.last_render_ms, 321.);
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    let later = RenderResourceCommand::ClearResource {
        name: "later.color".into(),
        value: [0.5; 4],
    };
    host.overrides
        .lock()
        .unwrap()
        .resource_commands
        .push(later.clone());
    host.camera.yaw += 0.75;
    assert!(host.render_presented(81, 46).unwrap().is_none());
    assert_eq!(draws.lock().unwrap().attempts, 1);
    assert_eq!(
        host.overrides.lock().unwrap().resource_commands,
        vec![later.clone()]
    );

    draws.lock().unwrap().ready = true;
    host.poll_authored_presentation().unwrap();
    assert!(!host.has_pending_presentation());
    assert!(host.has_ready_authored_presentation());
    assert_eq!(
        host.last_presented_camera(),
        None,
        "hidden polling must not label the retained display"
    );
    let polls = draws.lock().unwrap().polls;
    for _ in 0..4 {
        host.poll_authored_presentation().unwrap();
    }
    assert_eq!(
        draws.lock().unwrap().polls,
        polls,
        "buffered completion must not keep polling the GPU"
    );
    assert_eq!(draws.lock().unwrap().attempts, 1);
    let old = host.render_presented(81, 46).unwrap().unwrap();
    assert_eq!([old.width, old.height], [80, 45]);
    assert_eq!(host.last_presented_camera(), Some(camera));
    assert_eq!(host.last_presented_revision(), Some(revision));
    assert!(
        host.has_pending_presentation(),
        "completion must not cost a separate submission turn"
    );
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    {
        let draws = draws.lock().unwrap();
        assert_eq!(draws.attempts, 2);
        assert_eq!(draws.completed[0].1, first_batch);
        assert_eq!(draws.completed[1].1, vec![later]);
    }
    let current = host.render_presented(81, 46).unwrap().unwrap();
    assert_eq!([current.width, current.height], [81, 46]);
    assert_eq!(host.last_presented_camera(), Some(host.camera));
    assert!(!host.has_pending_presentation());
    assert_eq!(
        draws.lock().unwrap().attempts,
        2,
        "unchanged completion inputs must not be immediately resubmitted"
    );
    assert!(host.render_presented(81, 46).unwrap().is_none());
    assert!(host.has_pending_presentation());
    assert_eq!(
        draws.lock().unwrap().attempts,
        3,
        "a later explicit refresh must still check renderer dependencies"
    );
}

#[test]
fn authored_completion_survives_successor_snapshot_and_admission_errors_with_its_metadata() {
    for snapshot_failure in [false, true] {
        let (mut host, draws, _temp) = gpu_host();
        host.render_presented(80, 45).unwrap().unwrap();
        let original_camera = host.last_presented_camera();
        host.camera.yaw += 0.25;
        let completed_camera = host.camera;
        let completed_revision = host.presentation_revision();
        queue_gpu_resource_batch(&host);
        draws.lock().unwrap().delayed = true;
        assert!(host.render_presented(80, 45).unwrap().is_none());
        assert_eq!(host.last_presented_camera(), original_camera);
        let later = RenderResourceCommand::ClearResource {
            name: "later.color".into(),
            value: [0.5; 4],
        };
        host.overrides
            .lock()
            .unwrap()
            .resource_commands
            .push(later.clone());
        host.camera.yaw += 0.75;
        if snapshot_failure {
            host.step_budget(1. / 60., &input(&["bad_material"]), 1)
                .unwrap();
            let error = host
                .author_snapshot()
                .expect_err("fixture must fail before successor admission");
            assert!(
                error.contains("assets/missing-successor-texture.png"),
                "unexpected snapshot failure: {error}"
            );
        } else {
            draws.lock().unwrap().fail_submit = true;
        }
        draws.lock().unwrap().ready = true;
        let completed = host.render_presented(81, 46).unwrap().unwrap();
        assert_eq!([completed.width, completed.height], [80, 45]);
        assert_eq!(
            completed
                .gpu
                .as_ref()
                .unwrap()
                .as_any()
                .downcast_ref::<FixtureGpuLease>()
                .unwrap()
                .serial,
            2
        );
        assert_eq!(host.last_presented_camera(), Some(completed_camera));
        assert_eq!(host.last_presented_revision(), Some(completed_revision));
        assert!(!host.has_pending_presentation());
        assert!(!host.has_ready_authored_presentation());
        assert_eq!(
            host.overrides.lock().unwrap().resource_commands,
            vec![later.clone()]
        );
        assert_eq!(
            draws.lock().unwrap().completed.len(),
            2,
            "failed successor must not accept any commands"
        );

        assert!(
            host.render_presented(81, 46).is_err(),
            "the next attempt must report the successor's own error"
        );
        assert_eq!(host.last_presented_camera(), Some(completed_camera));
        assert_eq!(host.last_presented_revision(), Some(completed_revision));
        assert_eq!(
            host.overrides.lock().unwrap().resource_commands,
            vec![later]
        );
    }
}

#[test]
fn authored_owner_mailboxes_keep_distinct_cameras_and_fifo_prevents_a_moving_pane_starving_another()
{
    let (mut host, draws, _temp) = gpu_host();
    draws.lock().unwrap().delayed = true;
    let owner_a = 11;
    let owner_b = 22;
    let camera_a = host.camera;
    let mut camera_b = camera_a;
    camera_b.yaw += 1.;
    assert!(
        host.render_presented_for(owner_a, 80, 45)
            .unwrap()
            .is_none()
    );
    host.camera = camera_b;
    assert!(
        host.render_presented_for(owner_b, 81, 46)
            .unwrap()
            .is_none()
    );
    assert!(host.has_pending_authored_presentation_for(owner_a));
    assert!(host.has_pending_authored_presentation_for(owner_b));
    draws.lock().unwrap().ready = true;

    host.camera = camera_a;
    host.camera.yaw += 0.25;
    let completed_a = host.render_presented_for(owner_a, 80, 45).unwrap().unwrap();
    assert_eq!([completed_a.width, completed_a.height], [80, 45]);
    assert_eq!(host.last_presented_camera(), Some(camera_a));
    assert!(
        !host.has_pending_presentation(),
        "A's changing camera must yield to B's queued demand"
    );
    assert_eq!(draws.lock().unwrap().attempts, 1);
    host.camera = camera_b;
    assert!(
        host.render_presented_for(owner_b, 81, 46)
            .unwrap()
            .is_none()
    );
    assert_eq!(draws.lock().unwrap().attempts, 2);

    host.camera = camera_a;
    host.camera.yaw += 0.5;
    let next_camera_a = host.camera;
    assert!(
        host.render_presented_for(owner_a, 80, 45)
            .unwrap()
            .is_none(),
        "A must not receive B's completed pixels"
    );
    assert!(host.has_ready_authored_presentation_for(owner_b));
    assert!(!host.has_ready_authored_presentation_for(owner_a));
    assert_eq!(host.last_presented_camera(), Some(camera_a));
    assert_eq!(draws.lock().unwrap().attempts, 3);

    draws.lock().unwrap().ready = false;
    host.camera = camera_b;
    let completed_b = host.render_presented_for(owner_b, 81, 46).unwrap().unwrap();
    assert_eq!([completed_b.width, completed_b.height], [81, 46]);
    assert_eq!(
        completed_b
            .gpu
            .as_ref()
            .unwrap()
            .as_any()
            .downcast_ref::<FixtureGpuLease>()
            .unwrap()
            .serial,
        2
    );
    assert_eq!(host.last_presented_camera(), Some(camera_b));
    assert!(
        !host.has_pending_authored_presentation_for(owner_b),
        "consuming B's unchanged completion must not queue a redundant render"
    );
    assert!(host.has_pending_presentation());
    draws.lock().unwrap().ready = true;
    host.camera = next_camera_a;
    let next_a = host.render_presented_for(owner_a, 80, 45).unwrap().unwrap();
    assert_eq!(
        next_a
            .gpu
            .as_ref()
            .unwrap()
            .as_any()
            .downcast_ref::<FixtureGpuLease>()
            .unwrap()
            .serial,
        3
    );
    assert_eq!(host.last_presented_camera(), Some(next_camera_a));
    assert!(!host.has_pending_presentation());
    assert_eq!(draws.lock().unwrap().attempts, 3);
}

#[test]
fn hidden_authored_owners_release_ready_leases_and_cancel_pending_tickets_and_demand() {
    let (mut host, draws, _temp) = gpu_host();
    draws.lock().unwrap().delayed = true;
    let owner_a = 11;
    let owner_b = 22;
    assert!(
        host.render_presented_for(owner_a, 80, 45)
            .unwrap()
            .is_none()
    );
    assert!(
        host.render_presented_for(owner_b, 81, 46)
            .unwrap()
            .is_none()
    );
    draws.lock().unwrap().ready = true;
    host.camera.yaw += 0.25;
    let _completed_a = host.render_presented_for(owner_a, 80, 45).unwrap().unwrap();
    assert!(
        host.render_presented_for(owner_b, 81, 46)
            .unwrap()
            .is_none()
    );
    host.camera.yaw += 0.5;
    assert!(
        host.render_presented_for(owner_a, 80, 45)
            .unwrap()
            .is_none()
    );
    assert!(host.has_ready_authored_presentation_for(owner_b));
    assert!(host.has_pending_presentation());

    host.retain_authored_presentation_owners(&BTreeSet::from([owner_a]));
    assert!(!host.has_ready_authored_presentation_for(owner_b));
    assert!(!host.has_pending_authored_presentation_for(owner_b));
    assert_eq!(draws.lock().unwrap().lease_drops.load(Ordering::SeqCst), 1);
    assert!(host.has_pending_presentation());
    host.retain_authored_presentation_owners(&BTreeSet::new());
    assert!(!host.has_pending_presentation());
    assert_eq!(draws.lock().unwrap().discarded, vec![3]);
    assert_eq!(host.authored_presentation_owners, BTreeSet::from([0]));
    assert!(host.authored_presentation_requests.is_empty());
    assert!(host.authored_presentation_waiters.is_empty());
    assert!(!host.has_ready_authored_presentation());
    assert_eq!(draws.lock().unwrap().lease_drops.load(Ordering::SeqCst), 2);
}

#[test]
fn delayed_interpolated_completion_keeps_submitted_tick_camera_and_ordered_commands() {
    let (mut host, draws, _temp) = gpu_host();
    host.contributions.push(follow_contribution());
    host.camera.target = host.frame_target().unwrap();
    host.step_budget(1. / 60., &input(&["move"]), 1).unwrap();
    let revision = host.presentation_revision();
    let batch = queue_gpu_resource_batch(&host);
    draws.lock().unwrap().delayed = true;
    assert!(
        host.try_render_interpolated(80, 45, 0.25)
            .unwrap()
            .is_none()
    );
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    let later = RenderResourceCommand::ClearResource {
        name: "next.color".into(),
        value: [0.; 4],
    };
    host.overrides
        .lock()
        .unwrap()
        .resource_commands
        .push(later.clone());
    host.step_budget(1. / 60., &input(&["move"]), 1).unwrap();
    host.camera.yaw += 1.;
    let current_camera = host.camera;
    let current_state = host.session.state().clone();
    assert!(host.poll_interpolated_presentation().unwrap().is_none());
    assert_eq!(
        host.overrides.lock().unwrap().resource_commands,
        vec![later.clone()]
    );
    draws.lock().unwrap().ready = true;
    let completed = host.poll_interpolated_presentation().unwrap().unwrap();
    assert_eq!(completed.source_revision, revision);
    assert_ne!(completed.source_revision, host.presentation_revision());
    assert_eq!(completed.provenance.current_tick, 1);
    assert_eq!(completed.provenance.previous_tick, 0);
    assert_eq!(completed.provenance.alpha, 0.25);
    assert_eq!(completed.camera.target, [0.25, 0., 0.]);
    assert_ne!(completed.camera.yaw, current_camera.yaw);
    assert_eq!(host.camera, current_camera);
    assert_eq!(host.session.state(), &current_state);
    assert_eq!(
        host.overrides.lock().unwrap().resource_commands,
        vec![later.clone()]
    );
    assert!(
        host.try_render_interpolated(80, 45, 0.75)
            .unwrap()
            .is_none()
    );
    assert!(host.has_pending_presentation());
    let next = host.poll_interpolated_presentation().unwrap().unwrap();
    assert_eq!(next.provenance.current_tick, 2);
    assert_eq!(next.camera.target, [1.75, 0., 0.]);
    let draws = draws.lock().unwrap();
    assert_eq!(draws.attempts, 2);
    assert_eq!(draws.completed[0].1, batch);
    assert_eq!(draws.completed[1].1, vec![later]);
}

#[test]
fn restart_and_host_drop_discard_publication_without_replaying_accepted_batch() {
    let (mut host, draws, _temp) = gpu_host();
    draws.lock().unwrap().delayed = true;
    queue_gpu_resource_batch(&host);
    assert!(host.try_render_interpolated(80, 45, 1.).unwrap().is_none());
    let old_generation = host.presentation_generation;
    host.restart().unwrap();
    assert!(!host.has_pending_presentation());
    assert_eq!(draws.lock().unwrap().discarded, vec![1]);
    assert!(host.poll_interpolated_presentation().unwrap().is_none());
    let fresh = queue_gpu_resource_batch(&host);
    assert!(host.try_render_interpolated(80, 45, 1.).unwrap().is_none());
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    draws.lock().unwrap().ready = true;
    let completed = host.poll_interpolated_presentation().unwrap().unwrap();
    assert_ne!(completed.provenance.generation, old_generation);
    assert_eq!(completed.provenance.current_tick, 0);
    assert_eq!(draws.lock().unwrap().completed[1].1, fresh);
    host.camera.yaw += 0.5;
    assert!(host.try_render_interpolated(80, 45, 1.).unwrap().is_none());
    drop(host);
    assert_eq!(draws.lock().unwrap().discarded, vec![1, 3]);
}

#[test]
fn asynchronous_gpu_error_preserves_completed_evaluation_and_never_replays_accepted_one_shots() {
    let (mut host, draws, _temp) = gpu_host();
    let last_good = host.try_render_interpolated(80, 45, 1.).unwrap().unwrap();
    let evaluation = completed_evaluation(&host);
    let accepted = queue_gpu_resource_batch(&host);
    draws.lock().unwrap().delayed = true;
    assert!(host.try_render_interpolated(80, 45, 1.).unwrap().is_none());
    host.last_render_ms = 123.;
    let later = RenderResourceCommand::ClearResource {
        name: "later.color".into(),
        value: [1.; 4],
    };
    host.overrides
        .lock()
        .unwrap()
        .resource_commands
        .push(later.clone());
    draws.lock().unwrap().fail_poll = true;
    assert_eq!(
        host.poll_interpolated_presentation().err().unwrap(),
        "injected asynchronous completion failure"
    );
    assert_eq!(completed_evaluation(&host), evaluation);
    assert_eq!(host.last_render_ms, 123.);
    assert_eq!(
        host.overrides.lock().unwrap().resource_commands,
        vec![later.clone()]
    );
    assert!(last_good.frame.is_valid());
    {
        let mut draws = draws.lock().unwrap();
        draws.fail_poll = false;
        draws.delayed = false;
    }
    host.try_render_interpolated(80, 45, 1.).unwrap().unwrap();
    let draws = draws.lock().unwrap();
    assert_eq!(draws.completed[1].1, accepted);
    assert_eq!(draws.completed[2].1, vec![later]);
}

#[test]
fn rejected_gpu_presentation_target_keeps_renderer_target_document_and_completed_pose() {
    let (mut host, draws, _temp) = gpu_host();
    let completed = host.try_render_interpolated(80, 45, 1.).unwrap().unwrap();
    let old_target = host.presentation_target.clone().unwrap();
    let old_renderer = host.renderer.as_deref().unwrap() as *const dyn FrameRenderer as *const ();
    let document = serde_json::to_value(&host.document).unwrap();
    let evaluation = completed_evaluation(&host);
    let generation = host.presentation_generation;
    let camera = host.camera;
    let backend = host.device.clone();
    let commands = queue_gpu_resource_batch(&host);
    draws.lock().unwrap().reject_target = Some(2);
    let error = host
        .set_presentation_target(Some(Arc::new(FixturePresentationTarget(2))))
        .unwrap_err();
    assert_eq!(error, "injected target rejection");
    assert!(Arc::ptr_eq(
        host.presentation_target.as_ref().unwrap(),
        &old_target
    ));
    assert_eq!(
        host.renderer.as_deref().unwrap() as *const dyn FrameRenderer as *const (),
        old_renderer
    );
    assert_eq!(host.presentation_generation, generation);
    assert_eq!(host.camera, camera);
    assert_eq!(host.device, backend);
    assert_eq!(serde_json::to_value(&host.document).unwrap(), document);
    assert_eq!(completed_evaluation(&host), evaluation);
    assert_eq!(host.overrides.lock().unwrap().resource_commands, commands);
    assert!(completed.frame.is_valid());
    {
        let draws = draws.lock().unwrap();
        assert_eq!(draws.cpu_calls, 0);
        assert_eq!(draws.target_attempts, 2);
        assert!(Arc::ptr_eq(
            draws.accepted_target.as_ref().unwrap(),
            &old_target
        ));
    }
    let retry = host.try_render_interpolated(80, 45, 1.).unwrap().unwrap();
    assert_eq!(retry.provenance.generation, completed.provenance.generation);
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    assert_eq!(draws.lock().unwrap().completed.last().unwrap().1, commands);
}

#[test]
fn static_tick_has_one_baseline_frame_and_reset_events_snap_only_after_commit() {
    let (mut host, _, _temp) = host();
    host.step_budget(1. / 60., &input(&[]), 1).unwrap();
    host.try_render_interpolated(80, 45, 0.1).unwrap().unwrap();
    assert!(host.try_render_interpolated(80, 45, 0.9).unwrap().is_none());
    host.step_budget(1. / 60., &input(&[]), 1).unwrap();
    let committed = host.try_render_interpolated(80, 45, 0.1).unwrap().unwrap();
    assert_eq!(
        committed.provenance.origin,
        PresentationOrigin::CommittedState
    );
    let generation = committed.provenance.generation;
    assert!(
        host.step_budget(1. / 60., &input(&["reset", "reject"]), 1)
            .is_err()
    );
    assert!(host.try_render_interpolated(80, 45, 0.5).unwrap().is_none());
    host.restart().unwrap();
    host.step_budget(1. / 60., &input(&["move", "reset"]), 1)
        .unwrap();
    let reset = host.try_render_interpolated(80, 45, 0.1).unwrap().unwrap();
    assert_ne!(reset.provenance.generation, generation);
    assert_eq!(
        reset.provenance.previous_tick,
        reset.provenance.current_tick
    );
    assert_eq!(reset.provenance.alpha, 1.);
}

#[test]
fn render_failure_keeps_eligibility_and_ordered_one_shots_until_one_success() {
    let (mut host, draws, _temp) = host();
    host.try_render_interpolated(80, 45, 1.).unwrap().unwrap();
    let clear = |name: &str| RenderCommand::ClearResource {
        name: name.into(),
        value: [0.; 4],
    };
    host.overrides
        .lock()
        .unwrap()
        .apply_commands(&[clear("first")])
        .unwrap();
    draws.lock().unwrap().fail = true;
    assert!(host.try_render_interpolated(80, 45, 1.).is_err());
    assert_eq!(host.overrides.lock().unwrap().resource_commands.len(), 1);
    host.step_budget(1. / 60., &input(&[]), 1).unwrap();
    host.overrides
        .lock()
        .unwrap()
        .apply_commands(&[clear("second")])
        .unwrap();
    host.step_budget(1. / 60., &input(&[]), 1).unwrap();
    draws.lock().unwrap().fail = false;
    host.try_render_interpolated(80, 45, 1.).unwrap().unwrap();
    assert_eq!(
        draws.lock().unwrap().batches.last().unwrap(),
        &vec![
            RenderResourceCommand::ClearResource {
                name: "first".into(),
                value: [0.; 4]
            },
            RenderResourceCommand::ClearResource {
                name: "second".into(),
                value: [0.; 4]
            },
        ]
    );
    assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
    assert!(
        host.try_render_interpolated(80, 45, 1.).unwrap().is_none(),
        "Consuming a batch must not itself request another frame"
    );
    host.step_budget(1. / 60., &input(&[]), 1).unwrap();
    host.try_render_interpolated(80, 45, 1.).unwrap().unwrap();
    assert!(draws.lock().unwrap().batches.last().unwrap().is_empty());
}

#[test]
fn headless_discards_bounded_resource_batches_and_failed_enqueue_preserves_pending() {
    let (mut host, _, _temp) = host();
    let command = RenderCommand::ClearResource {
        name: "owned".into(),
        value: [0.; 4],
    };
    host.overrides
        .lock()
        .unwrap()
        .apply_commands(&vec![command.clone(); 4096])
        .unwrap();
    assert!(
        host.overrides
            .lock()
            .unwrap()
            .apply_commands(std::slice::from_ref(&command))
            .is_err()
    );
    assert_eq!(host.overrides.lock().unwrap().resource_commands.len(), 4096);
    host.renderer = None;
    for _ in 0..8 {
        host.overrides.lock().unwrap().apply_commands(&[]).unwrap();
        host.step_budget(1. / 60., &input(&[]), 1).unwrap();
        assert!(host.overrides.lock().unwrap().resource_commands.is_empty());
        host.overrides
            .lock()
            .unwrap()
            .apply_commands(std::slice::from_ref(&command))
            .unwrap();
    }
}

fn follow_contribution() -> GameContribution {
    GameContribution {
        id: "fixture.follow".into(),
        activation_modules: BTreeSet::new(),
        begin_tick: |_| Ok(()),
        initial_camera: |_| Camera::default(),
        follow_target: |state| Some(state.world.object(1).unwrap().transform.translation),
        decorate_snapshot: |_, _| {},
        script_input: |_| Value::Null,
        validate_event: |_| Ok(()),
        validate_state: |_| Ok(()),
        service_requests: |_| Ok(vec![]),
        diagnostics: |_| Value::Null,
    }
}

#[test]
fn direct_same_tick_mutation_and_camera_follow_use_actual_rendered_camera() {
    let (mut host, draws, _temp) = host();
    host.step_budget(1. / 60., &input(&["move"]), 1).unwrap();
    host.presentation_previous.as_mut().unwrap().target = Some([0.; 3]);
    host.presentation_current.as_mut().unwrap().target = Some([1., 0., 0.]);
    // Fixture has no follow contribution; add a pure neutral one for capture.
    host.contributions.push(follow_contribution());
    host.camera.target = [1., 0., 0.];
    let presented = host.try_render_interpolated(80, 45, 0.5).unwrap().unwrap();
    assert_eq!(presented.camera.target, [0.5, 0., 0.]);
    assert_eq!(host.camera.target, [1., 0., 0.]);
    host.session
        .apply_commands(&[WorldCommand::SetPosition {
            id: 1,
            position: [9., 0., 0.],
        }])
        .unwrap();
    let snap = host.try_render_interpolated(80, 45, 0.1).unwrap().unwrap();
    assert_eq!(snap.provenance.alpha, 1.);
    assert_eq!(
        draws.lock().unwrap().snapshots.last().unwrap().items[0].model[0][3],
        9.
    );
}

#[test]
fn following_respawn_snaps_camera_with_geometry_and_cannot_create_extra_pose_frames() {
    let (mut host, draws, _temp) = host();
    host.contributions.push(follow_contribution());
    host.camera.target = host.frame_target().unwrap();
    host.step_budget(1. / 60., &input(&["move"]), 1).unwrap();
    let moving = host.try_render_interpolated(80, 45, 0.5).unwrap().unwrap();
    assert_eq!(moving.camera.target, [0.5, 0., 0.]);
    host.step_budget(1. / 60., &input(&["respawn"]), 1).unwrap();
    let respawn = host.try_render_interpolated(80, 45, 0.5).unwrap().unwrap();
    assert_eq!(respawn.camera.target, [8., 0., 0.]);
    assert_eq!(respawn.provenance.alpha, 1.);
    assert_eq!(
        draws.lock().unwrap().snapshots.last().unwrap().items[0].model[0][3],
        8.
    );
    let renders = draws.lock().unwrap().snapshots.len();
    assert!(
        host.try_render_interpolated(80, 45, 0.75)
            .unwrap()
            .is_none()
    );
    assert_eq!(draws.lock().unwrap().snapshots.len(), renders);
}

#[test]
fn imported_primitives_receive_interpolated_object_pose_before_attachment() {
    let (mut host, draws, _temp) = host();
    std::fs::create_dir_all(host.root.join("assets/models")).unwrap();
    std::fs::write(
        host.root.join("assets/models/crystal.glb"),
        include_bytes!("../../../assets/models/demo_crystal.glb"),
    )
    .unwrap();
    let mut document = host.document.clone();
    document
        .game
        .as_mut()
        .unwrap()
        .scenes
        .get_mut("empty")
        .unwrap()
        .objects[0]
        .components
        .insert("mesh".into(), json!({"path":"assets/models/crystal.glb"}));
    host.refresh_author_preview(&document).unwrap();
    let model = host
        .content
        .load_model("assets/models/crystal.glb")
        .unwrap();
    assert!(!model.primitives.is_empty());
    host.step_budget(1. / 60., &input(&["move"]), 1).unwrap();
    host.try_render_interpolated(80, 45, 0.5).unwrap().unwrap();
    let draws = draws.lock().unwrap();
    let rendered = &draws.snapshots.last().unwrap().items;
    assert_eq!(rendered.len(), model.primitives.len());
    for (item, primitive) in rendered.iter().zip(&model.primitives) {
        assert_eq!(item.id, 1);
        let mut expected = primitive.transform;
        expected[0][3] += 0.5;
        for (actual, expected) in item.model.iter().flatten().zip(expected.iter().flatten()) {
            assert!((actual - expected).abs() < 1e-5);
        }
    }
}

#[test]
fn respawn_same_id_and_actual_same_scene_transition_never_blend_unrelated_lifetimes() {
    let (mut host, draws, _temp) = host();
    let original_lifetime = host.session.state().world.lifetime(1);
    host.step_budget(1. / 60., &input(&["respawn"]), 1).unwrap();
    assert_ne!(host.session.state().world.lifetime(1), original_lifetime);
    let respawn = host.try_render_interpolated(80, 45, 0.5).unwrap().unwrap();
    assert_eq!(respawn.provenance.alpha, 1.);
    assert_eq!(
        draws.lock().unwrap().snapshots.last().unwrap().items[0].model[0][3],
        8.
    );
    let before = host.presentation_generation;
    host.session.transition("empty").unwrap();
    let reset = host.try_render_interpolated(80, 45, 0.5).unwrap().unwrap();
    assert_ne!(reset.provenance.generation, before);
    assert_eq!(reset.provenance.alpha, 1.);
    assert_eq!(
        draws.lock().unwrap().snapshots.last().unwrap().items[0].model[0][3],
        0.
    );
}

#[test]
fn compiled_custom_string_reset_snaps_only_committed_teleport_and_rejection_keeps_history() {
    // Reuse the existing validated native fixture/compiler path. Only the final
    // rendering is recorded on CPU; the script really compiles and executes IPC.
    let root = super::test_support::workspace_root();
    let mut document = super::test_support::demo_document();
    document.custom_sources.insert(
        "game-controller".into(),
        r#"// module-engine-state: {"schema":"presentation.reset.v1","initial":{"mode":"move","ticks":0}}
fn update(ctx: &mut ScriptContext) {
    let mode = ctx.state["mode"].as_str().unwrap_or("move").to_owned();
    let position = ctx.object(1).unwrap().transform.translation;
    if mode == "move" {
        ctx.set_position(1, [position[0] + 1.0, position[1], position[2]]);
    } else {
        ctx.set_position(1, [9.0, position[1], position[2]]);
        ctx.emit_event(serde_json::json!("PresentationReset"));
        if mode == "reject" {
            ctx.set_position(999999, [0.0, 0.0, 0.0]);
        }
    }
    ctx.state["ticks"] = serde_json::json!(ctx.state["ticks"].as_u64().unwrap() + 1);
}
"#
        .into(),
    );
    let mut host =
        super::test_support::new_host(document, &root, Some(&root), WorldKind::Objects).unwrap();
    host.services.set_enabled(false);
    let draws = Arc::new(Mutex::new(Draws::default()));
    host.renderer = Some(Box::new(Recorder(draws.clone())));
    let original_x = host
        .session
        .state()
        .world
        .object(1)
        .unwrap()
        .transform
        .translation[0];
    host.step_budget(1. / 60., &GameInput::default(), 1)
        .unwrap();
    let first = host.try_render_interpolated(160, 90, 0.5).unwrap().unwrap();
    let current_x = host
        .session
        .state()
        .world
        .object(1)
        .unwrap()
        .transform
        .translation[0];
    assert!((current_x - original_x - 1.).abs() < 1e-5);
    assert_eq!(first.provenance.alpha, 0.5);
    let generation = host.presentation_generation;
    let previous = host.presentation_previous.clone();
    let current = host.presentation_current.clone();
    let state = serde_json::to_value(host.session.state()).unwrap();
    let rendered = draws.lock().unwrap().snapshots.len();
    let slot = host.slots["game-controller"].clone();
    slot.lock()
        .unwrap()
        .migrate_state(json!({"mode":"reject","ticks":1}))
        .unwrap();
    assert!(
        host.step_budget(1. / 60., &GameInput::default(), 1)
            .is_err()
    );
    assert_eq!(serde_json::to_value(host.session.state()).unwrap(), state);
    assert_eq!(host.presentation_generation, generation);
    assert!(host.presentation_previous == previous);
    assert!(host.presentation_current == current);
    assert_eq!(slot.lock().unwrap().state().unwrap()["ticks"], json!(1));
    assert!(
        host.try_render_interpolated(160, 90, 0.5)
            .unwrap()
            .is_none()
    );
    assert_eq!(draws.lock().unwrap().snapshots.len(), rendered);

    slot.lock()
        .unwrap()
        .migrate_state(json!({"mode":"teleport","ticks":1}))
        .unwrap();
    // The rejected tick retained its accumulator; retry it without adding time.
    assert_eq!(host.step_budget(0., &GameInput::default(), 1).unwrap(), 1);
    assert_eq!(
        host.session
            .events()
            .iter()
            .filter(|event| event.name == PRESENTATION_RESET_EVENT && event.payload.is_null())
            .count(),
        1
    );
    let reset = host.try_render_interpolated(160, 90, 0.1).unwrap().unwrap();
    assert_ne!(reset.provenance.generation, generation);
    assert_eq!(reset.provenance.alpha, 1.);
    assert_eq!(
        reset.provenance.previous_tick,
        reset.provenance.current_tick
    );
    assert_eq!(
        host.session
            .state()
            .world
            .object(1)
            .unwrap()
            .transform
            .translation[0],
        9.
    );
    assert_eq!(slot.lock().unwrap().state().unwrap()["ticks"], json!(2));
    let tick = host.session.state().tick;
    assert!(
        host.try_render_interpolated(160, 90, 0.9)
            .unwrap()
            .is_none()
    );
    assert_eq!(host.session.state().tick, tick);
    assert_eq!(slot.lock().unwrap().state().unwrap()["ticks"], json!(2));
}
