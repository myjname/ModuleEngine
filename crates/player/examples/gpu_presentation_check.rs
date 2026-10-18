//! Explicit offscreen native verification, never an interactive fallback.
//! Usage: gpu_presentation_check <report.json> [--png]
use eframe::{egui, egui_wgpu, wgpu};
use module_engine_core::{
    Camera, Frame, FrameRenderer, Graph, GraphDocument, ModuleRegistry, ObjectsWorld,
    RenderOverrides, RenderSubmission, SceneSnapshot, TextureImage, WorldKind, WorldProvider,
    demo_document,
};
use module_engine_player::gpu_presentation::{
    GpuPresentationBridge, GpuTexture, finish_gpu_retirements,
};
use module_engine_vulkan::presentation::{AdapterIdentity, VulkanPresentationTarget};
use module_engine_vulkan::{
    RenderPath, builtin_hlsl_source, builtin_manifest, default_graph, remove_pass_with_bypass,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

const WATCHDOG_SECONDS: u64 = 180;

struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park_timeout(Duration::from_millis(2)),
        }
    }
}
fn ensure(condition: bool, message: impl Into<String>) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

// Submit exactly once. Polling must never admit a replacement request, even
// when a very fast device has already completed the accepted submission.
fn finish_submission(
    renderer: &mut dyn FrameRenderer,
    submission: RenderSubmission,
) -> Result<Frame, String> {
    let ticket = match submission {
        RenderSubmission::Ready(_) => {
            return Err("Native fixture unexpectedly completed synchronously".into());
        }
        RenderSubmission::Busy => {
            return Err("Native fixture unexpectedly rejected admission".into());
        }
        RenderSubmission::Pending(ticket) => ticket,
    };
    finish_ticket(renderer, ticket)
}

fn finish_ticket(renderer: &mut dyn FrameRenderer, ticket: u64) -> Result<Frame, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(frame) = renderer.poll_presented(ticket)? {
            ensure(
                renderer.poll_presented(ticket)?.is_none(),
                "Consumed ticket published a duplicate frame",
            )?;
            return Ok(frame);
        }
        ensure(
            Instant::now() < deadline,
            "Producer completion poll exceeded 10 seconds",
        )?;
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn presented(
    renderer: &mut dyn FrameRenderer,
    scene: &SceneSnapshot,
    camera: Camera,
    size: [usize; 2],
    controls: &RenderOverrides,
) -> Result<Frame, String> {
    let before = renderer.performance();
    let submission = renderer.render_presented(scene, camera, size[0], size[1], controls)?;
    ensure(
        renderer.performance().gpu_submissions == before.gpu_submissions + 1
            && renderer.performance().gpu_presentation_frames == before.gpu_presentation_frames,
        "Admission counters did not distinguish submitted and completed work",
    )?;
    let frame = finish_submission(renderer, submission)?;
    ensure(
        renderer.performance().gpu_submissions == before.gpu_submissions + 1
            && renderer.performance().gpu_completion_polls > before.gpu_completion_polls
            && renderer.performance().gpu_presentation_frames == before.gpu_presentation_frames + 1,
        "Polling resubmitted work or failed to count real completion",
    )?;
    Ok(frame)
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(report_path) = args.first().map(PathBuf::from) else {
        eprintln!("Usage: gpu_presentation_check <report.json> [--png]");
        std::process::exit(2);
    };
    let png = args.iter().skip(1).any(|arg| arg == "--png");
    if let Some(parent) = report_path.parent().filter(|p| !p.as_os_str().is_empty())
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("Cannot create report directory: {error}");
        std::process::exit(2);
    }
    let report = Arc::new(Mutex::new(json!({
        "passed": false, "fixture": "production_gpu_presentation_offscreen",
        "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "watchdog_seconds": WATCHDOG_SECONDS,
        "scope": "Production Vulkan FrameRenderer -> native GUI bridge -> GUI-owned UNORM display; explicit diagnostic readback. No window, surface, eframe paint, vsync, compositor or hardware portability acceptance.",
        "cases": [],
    })));
    let (finished, watchdog) = std::sync::mpsc::sync_channel::<()>(1);
    let timeout_report = report.clone();
    let timeout_path = report_path.clone();
    let watchdog_thread = std::thread::spawn(move || {
        if watchdog
            .recv_timeout(Duration::from_secs(WATCHDOG_SECONDS))
            .is_err()
        {
            if let Ok(mut report) = timeout_report.lock() {
                report["error"] = json!("Native verification watchdog expired");
                report["timeout"] = json!(true);
                let _ = std::fs::write(
                    timeout_path,
                    serde_json::to_vec_pretty(&*report).unwrap_or_default(),
                );
            }
            std::process::exit(124);
        }
    });
    let started = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&report, &report_path, png)
    }))
    .unwrap_or_else(|_| Err("Native verification panicked; see process stderr".into()));
    // run's fixture/panes are gone, including on failure. A zero pending-copy
    // counter alone does not establish that the retirement thread has exited.
    let retirement = finish_gpu_retirements();
    let retirement_report = match &retirement {
        Ok(report) => report,
        Err(error) => &error.report,
    };
    report.lock().expect("report mutex")["gpu_retirement"] = json!(retirement_report);
    let producer = &retirement_report.producer;
    let producer_retired = producer.registered_jobs > 0
        && producer.joined_jobs == producer.registered_jobs
        && producer.completed_submissions == producer.registered_jobs
        && producer.unconfirmed_submissions == 0
        && producer.failed_jobs == 0;
    let result = result.and_then(|orphan_frame| {
        // All run-local GUI owners and all registered completion jobs are gone.
        // This frame now owns the final device pin through its native payload.
        if let Err(error) = retirement {
            drop(orphan_frame);
            return Err(error.to_string());
        }
        ensure(producer_retired, "Pending producer destruction was not fully retired at shutdown")?;
        ensure(orphan_frame.gpu.is_some(), "Orphan lifetime fixture requires a GPU frame")?;
        drop(orphan_frame);
        record(
            &report,
            json!({"case":"final_published_frame_outlives_gui_owners","passed":true,"gui_owners_dropped":true,"retirement_jobs_joined_before_frame_drop":true,"orphan_frame_dropped":true}),
        );
        Ok(())
    });
    let mut output = report.lock().expect("report mutex");
    output["wall_seconds"] = json!(started.elapsed().as_secs_f64());
    output["passed"] = json!(result.is_ok());
    if let Err(error) = &result {
        output["error"] = json!(error);
    }
    let write = std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&*output).expect("serializable report"),
    );
    drop(output);
    let _ = finished.send(());
    let _ = watchdog_thread.join();
    if let Err(error) = write {
        eprintln!("Write verification report: {error}");
        std::process::exit(2);
    }
    match result {
        Ok(()) => println!(
            "GPU presentation verification passed: {}",
            report_path.display()
        ),
        Err(error) => {
            eprintln!("GPU presentation verification failed: {error}");
            std::process::exit(1);
        }
    }
}

struct Fixture<'a> {
    registry: ModuleRegistry,
    scene: SceneSnapshot,
    bridge: GpuPresentationBridge,
    display: GpuTexture,
    report: &'a Arc<Mutex<Value>>,
    png_folder: Option<PathBuf>,
}
fn record(report: &Arc<Mutex<Value>>, case: Value) {
    report.lock().expect("report mutex")["cases"]
        .as_array_mut()
        .expect("case list")
        .push(case);
}

fn run(report: &Arc<Mutex<Value>>, report_path: &Path, png: bool) -> Result<Frame, String> {
    let mut config = egui_wgpu::WgpuConfiguration::default();
    let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut config.wgpu_setup else {
        return Err("Expected independent native WGPU setup".into());
    };
    setup.instance_descriptor.backends = if cfg!(target_os = "windows") {
        wgpu::Backends::DX12
    } else {
        wgpu::Backends::VULKAN
    };
    let instance = block_on(config.wgpu_setup.new_instance());
    let render_state = block_on(egui_wgpu::RenderState::create(
        &config,
        &instance,
        None,
        egui_wgpu::RendererOptions::default(),
    ))
    .map_err(|error| format!("Create actual offscreen GUI device: {error}"))?;
    let adapter = render_state.adapter.get_info();
    let expected_backend = if cfg!(target_os = "windows") {
        wgpu::Backend::Dx12
    } else {
        wgpu::Backend::Vulkan
    };
    ensure(
        adapter.backend == expected_backend,
        "GUI selected a different native backend",
    )?;
    report.lock().expect("report mutex")["gui"] = json!({"backend":format!("{:?}", adapter.backend),"adapter":adapter.name,"driver":adapter.driver,"driver_info":adapter.driver_info,"surface":false});
    let bridge = GpuPresentationBridge::new(render_state.clone())?;
    ensure(
        Arc::ptr_eq(&bridge.target(), &bridge.target()),
        "Bridge target identity is unstable",
    )?;
    let mut registry = ModuleRegistry::builtin();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    module_engine_vulkan::install(&mut registry, &root)?;
    let png_folder = png.then(|| {
        report_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("diagnostic-png")
    });
    if let Some(folder) = &png_folder {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    let mut fixture = Fixture {
        registry,
        scene: ObjectsWorld::demo().snapshot(),
        bridge,
        display: GpuTexture::default(),
        report,
        png_folder,
    };
    for (label, path) in [
        ("forward", RenderPath::Forward),
        ("deferred", RenderPath::Deferred),
    ] {
        let initial_document = document(default_graph(path), BTreeMap::new());
        let mut renderer = fixture.renderer(&initial_document)?;
        fixture.compare(
            &format!("{label}_default"),
            &mut *renderer,
            [96, 64],
            &RenderOverrides::default(),
            false,
        )?;
        // Native-window acceptance exposed a size-dependent cross-API import
        // rejection that the small fixtures could not detect. Compare the exact
        // required framebuffer and a neighboring nonaligned extent on both paths.
        for (extent_label, size) in [
            ("full_hd", [1920, 1080]),
            ("full_hd_unaligned", [1919, 1079]),
        ] {
            fixture.compare(
                &format!("{label}_{extent_label}"),
                &mut *renderer,
                size,
                &RenderOverrides::default(),
                false,
            )?;
        }
        let mut disabled = RenderOverrides::default();
        disabled.enabled.insert("ao".into(), false);
        disabled.enabled.insert("aa".into(), false);
        fixture.compare(
            &format!("{label}_ao_aa_disabled"),
            &mut *renderer,
            [96, 64],
            &disabled,
            false,
        )?;
        let mut graph = default_graph(path);
        ensure(
            remove_pass_with_bypass(&mut graph, "ao") == Some(1),
            "AO bypass did not preserve one color consumer",
        )?;
        ensure(
            remove_pass_with_bypass(&mut graph, "aa") == Some(1),
            "AA bypass did not preserve one color consumer",
        )?;
        let replacement = document(graph, BTreeMap::new());
        renderer = renderer
            .prepare_candidate(renderer_node(&replacement)?, &replacement)?
            .ok_or("Vulkan candidate did not reuse production renderer")?;
        fixture.compare(
            &format!("{label}_ao_aa_removed_candidate"),
            &mut *renderer,
            [97, 63],
            &RenderOverrides::default(),
            false,
        )?;
    }
    let mut alpha_document = custom_alpha_document(RenderPath::Forward)?;
    let mut renderer = fixture.renderer(&alpha_document)?;
    fixture.compare(
        "custom_final_alpha",
        &mut *renderer,
        [97, 63],
        &RenderOverrides::default(),
        true,
    )?;
    fixture.compare(
        "resize_up",
        &mut *renderer,
        [163, 101],
        &RenderOverrides::default(),
        true,
    )?;
    fixture.compare(
        "resize_down",
        &mut *renderer,
        [97, 63],
        &RenderOverrides::default(),
        true,
    )?;
    let previous_id = fixture.display.texture_id();
    let previous_pixels = fixture.display.diagnostic_readback()?;
    let mut invalid = alpha_document.clone();
    invalid.shader_sources.insert(
        "fixture-final-alpha".into(),
        "invalid HLSL fixture candidate".into(),
    );
    let rejection = match renderer.prepare_candidate(renderer_node(&invalid)?, &invalid) {
        Err(error) => error,
        Ok(_) => return Err("Invalid shader candidate was accepted".into()),
    };
    ensure(
        fixture.display.texture_id() == previous_id
            && fixture.display.diagnostic_readback()? == previous_pixels,
        "Rejected graph changed last good GUI display",
    )?;
    fixture.compare(
        "last_good_after_rejected_shader",
        &mut *renderer,
        [97, 63],
        &RenderOverrides::default(),
        true,
    )?;
    record(
        report,
        json!({"case":"rejected_shader_preserves_display","passed":true,"diagnostic":rejection}),
    );

    let leased = presented(
        &mut *renderer,
        &fixture.scene,
        Camera::default(),
        [97, 63],
        &RenderOverrides::default(),
    )?;
    let other_bridge = GpuPresentationBridge::new(render_state.clone())?;
    let target_error = renderer
        .set_presentation_target(Some(other_bridge.target()))
        .err()
        .ok_or("Different target accepted while source lease was live")?;
    renderer.set_presentation_target(Some(fixture.bridge.target()))?;
    ensure(
        fixture.display.texture_id() == previous_id
            && fixture.display.diagnostic_readback()? == previous_pixels,
        "Rejected target changed last good GUI display",
    )?;
    drop(leased);
    record(
        report,
        json!({"case":"rejected_target_preserves_endpoint_and_display","passed":true,"diagnostic":target_error}),
    );

    fixture.adapter_mismatch_check(&mut *renderer)?;

    fixture.pool_check(&alpha_document)?;
    fixture.async_check(&alpha_document, &render_state)?;
    // Publishing a replacement shares the actual backend; replacing authored
    // configuration never substitutes synthetic pixels for graph output.
    alpha_document = custom_alpha_document(RenderPath::Deferred)?;
    renderer = renderer
        .prepare_candidate(renderer_node(&alpha_document)?, &alpha_document)?
        .ok_or("Deferred replacement candidate missing")?;
    fixture.compare(
        "deferred_custom_graph_replacement",
        &mut *renderer,
        [97, 63],
        &RenderOverrides::default(),
        true,
    )?;
    drop(renderer);
    fixture.drop_check(&alpha_document)?;
    let mut recreated = fixture.renderer(&alpha_document)?;
    fixture.compare(
        "renderer_recreated_after_stop",
        &mut *recreated,
        [97, 63],
        &RenderOverrides::default(),
        true,
    )?;
    report.lock().expect("report mutex")["bridge_counters"] =
        serde_json::to_value(fixture.bridge.counters()).map_err(|e| e.to_string())?;
    let counters = fixture.bridge.counters();
    ensure(
        counters.cpu_readback_bytes == 0 && counters.cpu_upload_bytes == 0,
        "Normal bridge path transferred CPU pixels",
    )?;
    ensure(
        counters.pending_copies == 0,
        "Verification ended with pending consumer copies",
    )?;
    // Return only this final published frame. The renderer, fixture/display,
    // bridge, RenderState and GUI instance all leave scope before its Drop.
    presented(
        &mut *recreated,
        &fixture.scene,
        Camera::default(),
        [97, 63],
        &RenderOverrides::default(),
    )
}

fn document(graph: Graph, sources: BTreeMap<String, String>) -> GraphDocument {
    let mut document = demo_document(WorldKind::Objects);
    let node = document
        .root
        .nodes
        .iter_mut()
        .find(|n| n.id == "renderer")
        .expect("demo renderer");
    node.module = "render.vulkan".into();
    node.params = json!({});
    node.subgraph = Some(Box::new(graph));
    document.shader_sources = sources;
    document
}
fn renderer_node(document: &GraphDocument) -> Result<&module_engine_core::Node, String> {
    document
        .root
        .nodes
        .iter()
        .find(|node| node.id == "renderer")
        .ok_or_else(|| "Fixture renderer node missing".into())
}
fn custom_alpha_document(path: RenderPath) -> Result<GraphDocument, String> {
    let mut graph = default_graph(path);
    let encode = graph
        .nodes
        .iter_mut()
        .find(|node| node.id == "encode")
        .ok_or("Encode node missing")?;
    encode.module = "gpu.custom".into();
    encode.params = json!({"source_id":"fixture-final-alpha","manifest":builtin_manifest("gpu.color_encode").ok_or("Color encode manifest missing")?});
    let original = builtin_hlsl_source("gpu.color_encode").ok_or("Color encode HLSL missing")?;
    let store = "frame[tid.xy]=float4(encoded,1.0);";
    ensure(
        original.contains(store),
        "Fixture must adapt the actual builtin final store",
    )?;
    let source = original.replace(store, "float alpha = tid.x % 3 == 0 ? 0.0 : (tid.x % 3 == 1 ? 128.0 / 255.0 : 1.0);\n    frame[tid.xy] = float4(max(encoded, float3(0.25,0.5,0.75)), alpha);");
    Ok(document(
        graph,
        BTreeMap::from([("fixture-final-alpha".into(), source)]),
    ))
}

impl Fixture<'_> {
    fn async_check(
        &self,
        document: &GraphDocument,
        render_state: &egui_wgpu::RenderState,
    ) -> Result<(), String> {
        let controls = RenderOverrides::default();
        let mut scene_a = self.scene.clone();
        let texture_a = Arc::new(TextureImage {
            width: 2,
            height: 2,
            rgba8: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        });
        for item in &mut scene_a.items {
            item.material.base_color_texture = Some(texture_a.clone());
        }
        let mut scene_b = scene_a.clone();
        let texture_b = Arc::new(TextureImage {
            width: 3,
            height: 1,
            rgba8: vec![0, 255, 255, 255, 255, 0, 255, 255, 32, 32, 32, 255],
        });
        for item in &mut scene_b.items {
            item.material.base_color_texture = Some(texture_b.clone());
            item.color = [255, 32, 64, 255];
            item.model[0][3] += 1.0;
        }
        scene_b.time += 3.0;
        // Keep diagnostic reference uploads on another producer. The native
        // producer below must upload its own atlas in the pending submission.
        let mut reference_renderer = self.renderer(document)?;
        let reference_a =
            reference_renderer.render(&scene_a, Camera::default(), 97, 63, &controls)?;
        let expected_a: Vec<u8> =
            egui::ColorImage::from_rgba_unmultiplied([97, 63], &reference_a.rgba)
                .pixels
                .iter()
                .flat_map(|p| p.to_array())
                .collect();
        drop(reference_renderer);
        let mut renderer = self.renderer(document)?;
        let before = renderer.performance();
        let ticket =
            match renderer.render_presented(&scene_a, Camera::default(), 97, 63, &controls)? {
                RenderSubmission::Pending(ticket) => ticket,
                _ => {
                    return Err(
                        "Async isolation fixture did not accept a pending native request".into(),
                    );
                }
            };
        let admitted = renderer.performance();
        ensure(
            admitted.gpu_submissions == before.gpu_submissions + 1
                && admitted.gpu_presentation_frames == before.gpu_presentation_frames,
            "Pending submission counters did not distinguish admission and publication",
        )?;
        ensure(
            matches!(
                renderer.render_presented(&scene_b, Camera::default(), 131, 79, &controls)?,
                RenderSubmission::Busy
            ),
            "Second request overwrote pending scene/atlas/extent",
        )?;
        ensure(
            renderer.performance().gpu_submissions == admitted.gpu_submissions
                && renderer.performance().gpu_busy_skips == admitted.gpu_busy_skips + 1,
            "Busy request submitted GPU work or failed to record backpressure",
        )?;
        ensure(
            renderer
                .render(&scene_b, Camera::default(), 131, 79, &controls)
                .is_err(),
            "CPU render overwrote pending GPU upload or resized its target",
        )?;
        let same_target_before = renderer.performance();
        renderer.set_presentation_target(Some(self.bridge.target()))?;
        ensure(
            renderer.performance() == same_target_before,
            "Same target reconfigured an in-flight renderer",
        )?;
        let other = GpuPresentationBridge::new(render_state.clone())?;
        let rejection = renderer
            .set_presentation_target(Some(other.target()))
            .err()
            .ok_or("Target replaced while producer submission was pending")?;
        // No diagnostic readback or consumer submission has run since native
        // admission. These checks exercise logical flight even on a fast GPU.
        record(
            self.report,
            json!({"case":"async_pending_busy_and_target_preservation","passed":true,"native_pending":true,"second_submission_busy":true,"cpu_render_rejected":true,"same_target_idempotent":true,"different_target_rejected":true,"diagnostic":rejection,"readback_before_completion":false}),
        );

        let mut invalid = document.clone();
        invalid.shader_sources.insert(
            "fixture-final-alpha".into(),
            "invalid HLSL pending candidate".into(),
        );
        ensure(
            renderer
                .prepare_candidate(renderer_node(&invalid)?, &invalid)
                .is_err(),
            "Invalid pending shader candidate was accepted",
        )?;
        let candidate_document = custom_alpha_document(RenderPath::Deferred)?;
        let candidate = renderer
            .prepare_candidate(renderer_node(&candidate_document)?, &candidate_document)?
            .ok_or("Valid graph candidate could not be prepared during producer flight")?;
        let frame_a = finish_ticket(&mut *renderer, ticket)?;
        ensure(
            frame_a.width == 97
                && frame_a.height == 63
                && frame_a.gpu.is_some()
                && frame_a.rgba.is_empty(),
            "Pending request lost original dimensions or GPU representation",
        )?;
        ensure(
            renderer.performance().gpu_presentation_frames == before.gpu_presentation_frames + 1
                && renderer.performance().cpu_readback_bytes == before.cpu_readback_bytes,
            "Pending request publication count or normal readback changed",
        )?;
        let mut display = GpuTexture::default();
        ensure(
            display.update(&self.bridge, &frame_a)?.is_some(),
            "Pending A display copy missing",
        )?;
        ensure(
            display.diagnostic_readback()? == expected_a,
            "Scene/atlas/resize request or rejected candidate corrupted in-flight A pixels",
        )?;
        self.reap(&mut display)?;
        drop(frame_a);
        record(
            self.report,
            json!({"case":"async_candidates_preserve_inflight_pixels","passed":true,"invalid_candidate_rejected":true,"valid_candidate_prepared":true,"differing_bytes":0,"exact_ticket_completed_once":true,"normal_cpu_readback_bytes":renderer.performance().cpu_readback_bytes-before.cpu_readback_bytes}),
        );
        let changed_scene = renderer.render(&scene_b, Camera::default(), 97, 63, &controls)?;
        let changed_scene_pixels: Vec<u8> =
            egui::ColorImage::from_rgba_unmultiplied([97, 63], &changed_scene.rgba)
                .pixels
                .iter()
                .flat_map(|p| p.to_array())
                .collect();
        ensure(
            expected_a != changed_scene_pixels,
            "Scene/material positive control did not change same-size pixels",
        )?;
        renderer = candidate;

        let reference_b = renderer.render(&scene_b, Camera::default(), 131, 79, &controls)?;
        let expected_b: Vec<u8> =
            egui::ColorImage::from_rgba_unmultiplied([131, 79], &reference_b.rgba)
                .pixels
                .iter()
                .flat_map(|p| p.to_array())
                .collect();
        ensure(
            expected_a != expected_b,
            "Scene/atlas/extent isolation control is unchanged",
        )?;
        let before_b = renderer.performance();
        let frame_b = presented(
            &mut *renderer,
            &scene_b,
            Camera::default(),
            [131, 79],
            &controls,
        )?;
        ensure(
            frame_b.width == 131 && frame_b.height == 79,
            "Next async request retained previous extent",
        )?;
        ensure(
            display.update(&self.bridge, &frame_b)?.is_some()
                && display.diagnostic_readback()? == expected_b,
            "Next scene/atlas/resize did not match its own reference",
        )?;
        self.reap(&mut display)?;
        drop(frame_b);
        ensure(
            renderer.performance().cpu_readback_bytes == before_b.cpu_readback_bytes
                && renderer.performance().gpu_presentation_frames
                    == before_b.gpu_presentation_frames + 1,
            "Scene B publication used CPU pixels or duplicated output",
        )?;
        record(
            self.report,
            json!({"case":"async_scene_atlas_resize_upload_isolation","passed":true,"previous_dimensions":[97,63],"next_dimensions":[131,79],"scene_material_and_geometry_changed":true,"differing_bytes":0,"published_frames":2,"normal_cpu_readback_bytes":0}),
        );

        let ticket =
            match renderer.render_presented(&scene_a, Camera::default(), 97, 63, &controls)? {
                RenderSubmission::Pending(ticket) => ticket,
                _ => return Err("Discard fixture did not start pending GPU work".into()),
            };
        let before_discard = renderer.performance();
        renderer.discard_presented(ticket)?;
        ensure(
            renderer.poll_presented(ticket)?.is_none(),
            "Discarded request published a frame",
        )?;
        ensure(
            renderer.performance().gpu_presentation_frames
                == before_discard.gpu_presentation_frames,
            "Discarded request counted as publication",
        )?;
        // A discarded request still owns the backend until real completion.
        // Polling its known ticket may retire resources but cannot publish.
        let deadline = Instant::now() + Duration::from_secs(10);
        let next = loop {
            ensure(
                renderer.poll_presented(ticket)?.is_none(),
                "Discarded request later published a frame",
            )?;
            match renderer.render_presented(&scene_b, Camera::default(), 131, 79, &controls)? {
                RenderSubmission::Busy => {
                    ensure(
                        Instant::now() < deadline,
                        "Discarded producer work never retired",
                    )?;
                    std::thread::sleep(Duration::from_millis(1));
                }
                submission => break submission,
            }
        };
        let frame = finish_submission(&mut *renderer, next)?;
        ensure(
            display.update(&self.bridge, &frame)?.is_some()
                && display.diagnostic_readback()? == expected_b,
            "Discard corrupted the next publication",
        )?;
        self.reap(&mut display)?;
        drop(frame);
        record(
            self.report,
            json!({"case":"async_discard_retires_without_publication","passed":true,"discarded_ticket_never_published":true,"next_ticket_exact_pixels":true,"differing_bytes":0}),
        );

        let submission =
            renderer.render_presented(&scene_a, Camera::default(), 97, 63, &controls)?;
        ensure(
            matches!(submission, RenderSubmission::Pending(_)),
            "Drop fixture did not own pending producer work",
        )?;
        let drop_started = Instant::now();
        drop(renderer);
        record(
            self.report,
            json!({"case":"async_drop_pending_producer_retirement","passed":true,"producer_pending_at_drop":true,"owner_drop_ms":drop_started.elapsed().as_secs_f64()*1000.0,"retirement_join_required_at_fixture_shutdown":true,"scope":"Actual pending FrameRenderer destruction; aggregate retirement join is validated at fixture shutdown, not Editor Stop UI. Drop timing is diagnostic, without a timing acceptance threshold."}),
        );
        Ok(())
    }

    fn renderer(&self, document: &GraphDocument) -> Result<Box<dyn FrameRenderer>, String> {
        let mut renderer = self
            .registry
            .create_renderer(renderer_node(document)?, document)?;
        renderer.set_detailed_stats(false);
        renderer.set_presentation_target(Some(self.bridge.target()))?;
        Ok(renderer)
    }
    fn reap(&self, display: &mut GpuTexture) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while display.counters().pending_copies != 0 {
            display.poll(&self.bridge)?;
            ensure(
                Instant::now() < deadline,
                "Consumer GPU completion did not retire within 10 seconds",
            )?;
            if display.counters().pending_copies != 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        Ok(())
    }
    fn compare(
        &mut self,
        name: &str,
        renderer: &mut dyn FrameRenderer,
        size: [usize; 2],
        controls: &RenderOverrides,
        alpha: bool,
    ) -> Result<Frame, String> {
        let before_reference = renderer.performance();
        let reference =
            renderer.render(&self.scene, Camera::default(), size[0], size[1], controls)?;
        ensure(
            reference.gpu.is_none() && reference.rgba.len() == size[0] * size[1] * 4,
            format!("{name}: explicit CPU reference is malformed"),
        )?;
        let after_reference = renderer.performance();
        let converted = egui::ColorImage::from_rgba_unmultiplied(size, &reference.rgba);
        let expected: Vec<u8> = converted
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect();
        if alpha {
            for alpha in [0, 128, 255] {
                ensure(
                    reference
                        .rgba
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|p| p[3] == alpha),
                    format!("{name}: source alpha {alpha} missing"),
                )?;
            }
            ensure(
                reference
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] == 0 && p[..3].iter().any(|v| *v != 0)),
                "Transparent source RGB fixture was optimized away",
            )?;
        }
        let before_native = renderer.performance();
        let frame = presented(renderer, &self.scene, Camera::default(), size, controls)?;
        let after_native = renderer.performance();
        ensure(
            frame.gpu.is_some() && frame.rgba.is_empty() && frame.is_valid(),
            format!("{name}: native renderer returned CPU or malformed pixels"),
        )?;
        ensure(
            after_native.cpu_readback_bytes == before_native.cpu_readback_bytes,
            format!("{name}: native renderer read GPU pixels to CPU"),
        )?;
        ensure(
            after_native.gpu_presentation_frames == before_native.gpu_presentation_frames + 1,
            format!("{name}: producer GPU frame counter did not advance exactly once"),
        )?;
        ensure(
            after_native.gpu_presentation_bytes
                == before_native.gpu_presentation_bytes + (size[0] * size[1] * 4) as u64,
            format!("{name}: producer GPU byte counter mismatch"),
        )?;
        let id = self
            .display
            .update(&self.bridge, &frame)?
            .ok_or_else(|| format!("{name}: display conversion not submitted"))?;
        ensure(
            self.display.dimensions() == Some(size),
            "GUI display dimensions mismatch",
        )?;
        let actual = self.display.diagnostic_readback()?;
        ensure(
            actual.len() == expected.len(),
            format!("{name}: diagnostic display size mismatch"),
        )?;
        let max_error = actual
            .iter()
            .zip(&expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        let differing_bytes = actual.iter().zip(&expected).filter(|(a, b)| a != b).count();
        // Repeat has no submission and must leave frame identity/display stable.
        ensure(
            self.display.update(&self.bridge, &frame)?.is_none()
                && self.display.texture_id() == Some(id),
            "Repeated GPU publication was incorrectly marked fresh",
        )?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.display.counters().pending_copies != 0 {
            self.display.poll(&self.bridge)?;
            ensure(
                Instant::now() < deadline,
                "Display release fence did not complete",
            )?;
            std::thread::sleep(Duration::from_millis(1));
        }
        if let Some(folder) = &self.png_folder {
            write_png(&folder.join(format!("{name}.png")), size, &actual)?;
        }
        record(
            self.report,
            json!({
                "case":name, "passed":max_error == 0, "dimensions":size,
                "pixels":size[0]*size[1], "max_channel_error":max_error,"differing_bytes":differing_bytes,
                "reference_cpu_readback_bytes":after_reference.cpu_readback_bytes-before_reference.cpu_readback_bytes,
                "presented_cpu_readback_bytes":after_native.cpu_readback_bytes-before_native.cpu_readback_bytes,
                "presented_gpu_frames":after_native.gpu_presentation_frames-before_native.gpu_presentation_frames,
                "presented_gpu_bytes":after_native.gpu_presentation_bytes-before_native.gpu_presentation_bytes,
                "display_explicit_diagnostic_readback_bytes":actual.len(),"display_counters":self.display.counters(),
                "alpha_fixture":alpha,
            }),
        );
        ensure(
            max_error == 0,
            format!(
                "{name}: GPU display differs from actual ColorImage conversion: {differing_bytes} bytes, max {max_error}"
            ),
        )?;
        Ok(reference)
    }

    fn adapter_mismatch_check(&mut self, renderer: &mut dyn FrameRenderer) -> Result<(), String> {
        // All ordinary copies and the preceding endpoint-rejection lease have
        // retired. Otherwise a busy-pool rejection would mask adapter matching.
        ensure(
            self.display.counters().pending_copies == 0,
            "Adapter mismatch fixture requires no pending consumer copies",
        )?;
        let accepted = self.bridge.target();
        let native = accepted
            .as_any()
            .downcast_ref::<VulkanPresentationTarget>()
            .ok_or("GUI bridge did not supply a VulkanPresentationTarget")?;
        let identities = match native.identity.clone() {
            AdapterIdentity::VulkanUuid { device, driver } => {
                ensure(
                    cfg!(target_os = "linux"),
                    "Vulkan UUID fixture requires Linux",
                )?;
                let mut wrong_device = device;
                wrong_device[0] ^= 1;
                let mut wrong_driver = driver;
                wrong_driver[0] ^= 1;
                vec![
                    (
                        "rejected_device_uuid_preserves_endpoint_and_display",
                        AdapterIdentity::VulkanUuid {
                            device: wrong_device,
                            driver,
                        },
                    ),
                    (
                        "rejected_driver_uuid_preserves_endpoint_and_display",
                        AdapterIdentity::VulkanUuid {
                            device,
                            driver: wrong_driver,
                        },
                    ),
                ]
            }
            AdapterIdentity::WindowsLuid(mut luid) => {
                ensure(cfg!(target_os = "windows"), "LUID fixture requires Windows")?;
                luid[0] ^= 1;
                vec![(
                    "rejected_adapter_luid_preserves_endpoint_and_display",
                    AdapterIdentity::WindowsLuid(luid),
                )]
            }
        };
        let previous_id = self.display.texture_id();
        let previous_pixels = self.display.diagnostic_readback()?;
        let [width, height] = self
            .display
            .dimensions()
            .ok_or("Missing accepted display")?;
        let previous_backend = renderer.backend_name().to_owned();
        for (name, identity) in identities {
            let before = renderer.performance();
            let candidate = Arc::new(VulkanPresentationTarget {
                identity,
                allocator: native.allocator.clone(),
            });
            let rejection = renderer
                .set_presentation_target(Some(candidate))
                .err()
                .ok_or_else(|| format!("{name}: mismatched adapter was accepted"))?;
            ensure(
                rejection
                    == "No matching GUI adapter with Vulkan 1.3 dynamic rendering and external-memory import",
                format!("{name}: adapter matching was not reached: {rejection}"),
            )?;
            ensure(
                renderer.performance() == before && renderer.backend_name() == previous_backend,
                format!("{name}: rejected target changed accepted renderer state"),
            )?;
            ensure(
                self.display.texture_id() == previous_id
                    && self.display.diagnostic_readback()? == previous_pixels,
                format!("{name}: rejection changed last good display"),
            )?;
            // Do not restore the endpoint: this actual publication must use the
            // endpoint retained by the failed target-candidate transaction.
            let frame = presented(
                renderer,
                &self.scene,
                Camera::default(),
                [width, height],
                &RenderOverrides::default(),
            )?;
            let after = renderer.performance();
            ensure(
                frame.gpu.is_some()
                    && frame.rgba.is_empty()
                    && frame.is_valid()
                    && after.cpu_readback_bytes == before.cpu_readback_bytes
                    && after.gpu_presentation_frames == before.gpu_presentation_frames + 1
                    && after.gpu_presentation_bytes
                        == before.gpu_presentation_bytes + (width * height * 4) as u64,
                format!("{name}: retained endpoint used CPU fallback or missed GPU publication"),
            )?;
            // A live lease forbids replacing a different endpoint. Reaccepting
            // this exact endpoint must be a no-op while the new frame is held.
            renderer.set_presentation_target(Some(accepted.clone()))?;
            ensure(
                renderer.performance() == after,
                format!("{name}: retained endpoint was rebuilt"),
            )?;
            ensure(
                self.display.update(&self.bridge, &frame)? == previous_id
                    && self.display.texture_id() == previous_id
                    && self.display.diagnostic_readback()? == previous_pixels,
                format!("{name}: last good actual GPU image changed"),
            )?;
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.display.counters().pending_copies != 0 {
                self.display.poll(&self.bridge)?;
                ensure(
                    Instant::now() < deadline,
                    "Adapter mismatch copy did not retire",
                )?;
                std::thread::sleep(Duration::from_millis(1));
            }
            drop(frame);
            let counters = self.bridge.counters();
            ensure(
                counters.cpu_readback_bytes == 0 && counters.cpu_upload_bytes == 0,
                format!("{name}: normal bridge used CPU pixel transfer"),
            )?;
            record(
                self.report,
                json!({
                    "case": name, "passed": true, "diagnostic": rejection,
                    "only_identity_changed": true, "allocator_preserved": true,
                    "last_good_display_unchanged": true, "retained_endpoint_gpu_publication": true,
                    "dimensions": [width, height], "differing_bytes": 0,
                    "presented_gpu_frames": after.gpu_presentation_frames-before.gpu_presentation_frames,
                    "presented_cpu_readback_bytes": after.cpu_readback_bytes-before.cpu_readback_bytes,
                    "explicit_diagnostic_readback": true,
                }),
            );
        }
        Ok(())
    }

    fn pool_check(&self, document: &GraphDocument) -> Result<(), String> {
        let mut renderer = self.renderer(document)?;
        let before = renderer.performance();
        let mut frames = Vec::new();
        for _ in 0..4 {
            frames.push(presented(
                &mut *renderer,
                &self.scene,
                Camera::default(),
                [97, 63],
                &RenderOverrides::default(),
            )?);
        }
        ensure(
            matches!(
                renderer.render_presented(
                    &self.scene,
                    Camera::default(),
                    97,
                    63,
                    &RenderOverrides::default()
                )?,
                RenderSubmission::Busy
            ),
            "Fifth held lease bypassed the bounded producer pool",
        )?;
        let mut display = GpuTexture::default();
        let first = frames.remove(0);
        ensure(
            display.update(&self.bridge, &first)?.is_some(),
            "Pool retirement copy not submitted",
        )?;
        drop(first);
        // No completion poll has released the consumer's source lease yet.
        ensure(
            matches!(
                renderer.render_presented(
                    &self.scene,
                    Camera::default(),
                    97,
                    63,
                    &RenderOverrides::default()
                )?,
                RenderSubmission::Busy
            ),
            "Source lease was released before consumer completion observation",
        )?;
        // Recover exclusively through production nonblocking poll, without a
        // new frame or diagnostic readback providing an incidental GPU wait.
        self.reap(&mut display)?;
        let recycled = presented(
            &mut *renderer,
            &self.scene,
            Camera::default(),
            [97, 63],
            &RenderOverrides::default(),
        )?;
        ensure(
            recycled.gpu.is_some() && recycled.rgba.is_empty(),
            "Recycled native slot returned CPU pixels",
        )?;
        let after = renderer.performance();
        ensure(
            after.cpu_readback_bytes == before.cpu_readback_bytes
                && after.gpu_presentation_frames == before.gpu_presentation_frames + 5,
            "Pool saturation performed a hidden readback/submission",
        )?;
        let diagnostic = display.diagnostic_readback()?;
        drop(recycled);
        drop(frames);
        record(
            self.report,
            json!({"case":"four_leases_backpressure_consumer_retirement","passed":true,"held_leases":4,"fifth_skipped":true,"leased_until_completion_poll":true,"recycled_after_gpu_completion":true,"gpu_frames":after.gpu_presentation_frames-before.gpu_presentation_frames,"normal_cpu_readback_bytes":after.cpu_readback_bytes-before.cpu_readback_bytes,"explicit_diagnostic_bytes":diagnostic.len(),"display_counters":display.counters()}),
        );
        Ok(())
    }

    fn drop_check(&self, document: &GraphDocument) -> Result<(), String> {
        let mut renderer = self.renderer(document)?;
        let reference = renderer.render(
            &self.scene,
            Camera::default(),
            97,
            63,
            &RenderOverrides::default(),
        )?;
        let expected: Vec<u8> = egui::ColorImage::from_rgba_unmultiplied([97, 63], &reference.rgba)
            .pixels
            .iter()
            .flat_map(|p| p.to_array())
            .collect();
        let frame = presented(
            &mut *renderer,
            &self.scene,
            Camera::default(),
            [97, 63],
            &RenderOverrides::default(),
        )?;
        drop(renderer);
        let mut display = GpuTexture::default();
        ensure(
            display.update(&self.bridge, &frame)?.is_some(),
            "Frame could not be consumed after producer renderer drop",
        )?;
        let actual = display.diagnostic_readback()?;
        ensure(
            actual == expected,
            "Renderer drop invalidated published GPU pixels",
        )?;
        self.reap(&mut display)?;
        drop(frame);
        // Exercise pane closure with an actual outstanding copy and retain no
        // producer frame outside the production bridge's completion machinery.
        let mut renderer = self.renderer(document)?;
        let frame = presented(
            &mut *renderer,
            &self.scene,
            Camera::default(),
            [97, 63],
            &RenderOverrides::default(),
        )?;
        ensure(
            display.update(&self.bridge, &frame)?.is_some(),
            "Pane-drop copy not submitted",
        )?;
        drop(frame);
        drop(renderer);
        drop(display);
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.bridge.counters().pending_copies != 0 {
            ensure(
                Instant::now() < deadline,
                "Closed-pane completion reaper retained GPU leases",
            )?;
            std::thread::sleep(Duration::from_millis(2));
        }
        record(
            self.report,
            json!({"case":"producer_drop_and_pane_drop_completion_lifetime","passed":true,"published_frame_consumed_after_renderer_drop":true,"closed_pane_pending_copies":0,"explicit_diagnostic_bytes":actual.len(),"scope":"FrameRenderer and GpuTexture destruction/recreation; not the Editor Stop command UI"}),
        );
        Ok(())
    }
}

fn write_png(path: &Path, size: [usize; 2], pixels: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        size[0] as u32,
        size[1] as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .map_err(|e| e.to_string())?
        .write_image_data(pixels)
        .map_err(|e| e.to_string())
}
