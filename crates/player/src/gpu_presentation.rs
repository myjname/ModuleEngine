//! GPU-only native presentation. The renderer owns production; this bridge owns
//! platform interop and egui resources. CPU pixels remain an explicit diagnostic
//! and software-renderer contract, never a fallback for an interop failure.

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct GpuPresentationCounters {
    pub gpu_presentations: u64,
    pub busy_skips: u64,
    pub pending_copies: usize,
    pub pending_high_water: usize,
    pub cpu_readback_bytes: u64,
    pub cpu_upload_bytes: u64,
    pub diagnostic_readback_bytes: u64,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct GpuRetirementReport {
    pub registered_jobs: usize,
    pub joined_jobs: usize,
    pub completed_copies: usize,
    pub unconfirmed_copies: usize,
    pub failed_jobs: usize,
    #[cfg(feature = "vulkan")]
    pub producer: module_engine_vulkan::ProducerRetirementReport,
}

#[derive(Debug)]
pub struct GpuRetirementFailure {
    pub report: GpuRetirementReport,
    pub errors: Vec<String>,
}
impl std::fmt::Display for GpuRetirementFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(output, "GPU retirement failed: {}", self.errors.join("; "))
    }
}
impl std::error::Error for GpuRetirementFailure {}

/// Drain only this bridge's retirement threads, after UI and background owners
/// have dropped their panes. Pane destruction itself never waits for the GPU.
pub fn finish_gpu_retirements() -> Result<GpuRetirementReport, GpuRetirementFailure> {
    #[cfg(feature = "vulkan")]
    let (mut report, mut errors) = native::finish_gpu_retirements();
    #[cfg(feature = "vulkan")]
    {
        let (producer, producer_errors) = module_engine_vulkan::finish_producer_retirements();
        report.producer = producer;
        errors.extend(
            producer_errors
                .into_iter()
                .take(8usize.saturating_sub(errors.len())),
        );
    }
    #[cfg(not(feature = "vulkan"))]
    let (report, errors) = (GpuRetirementReport::default(), Vec::<String>::new());
    eprintln!(
        "ModuleEngine GPU retirement: {}",
        serde_json::to_string(&report).expect("serializable GPU retirement report")
    );
    if errors.is_empty() {
        Ok(report)
    } else {
        Err(GpuRetirementFailure { report, errors })
    }
}

#[cfg(feature = "vulkan")]
pub use native::{GPU_ALPHA_CONVERSION_WGSL, GpuPresentationBridge, GpuTexture};

#[cfg(feature = "vulkan")]
mod native {
    use super::{GpuPresentationCounters, GpuRetirementReport};
    use eframe::{egui, egui_wgpu::RenderState, wgpu};
    use module_engine_core::{Frame, GpuFrame, PresentationTarget};
    use module_engine_vulkan::presentation::{
        AdapterIdentity, SharedImageAllocator, SharedImageInfo, VulkanPresentationTarget,
    };
    use std::{
        any::Any,
        sync::{
            Arc, Mutex, OnceLock, Weak,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
    };

    #[derive(Default)]
    struct RetirementOutcome {
        completed_copies: usize,
        unconfirmed_copies: usize,
        errors: Vec<String>,
    }

    struct RetirementJob {
        copies: usize,
        owner: std::thread::JoinHandle<RetirementOutcome>,
    }

    #[derive(Default)]
    struct RetirementRegistry {
        active: Vec<RetirementJob>,
        report: GpuRetirementReport,
        errors: Vec<String>,
    }

    fn retirement_jobs() -> &'static Mutex<RetirementRegistry> {
        static JOBS: OnceLock<Mutex<RetirementRegistry>> = OnceLock::new();
        JOBS.get_or_init(|| Mutex::new(RetirementRegistry::default()))
    }

    fn collect_retirement(
        job: RetirementJob,
        report: &mut GpuRetirementReport,
        errors: &mut Vec<String>,
    ) {
        report.joined_jobs += 1;
        match job.owner.join() {
            Ok(outcome) => {
                report.completed_copies += outcome.completed_copies;
                report.unconfirmed_copies += outcome.unconfirmed_copies;
                report.failed_jobs += usize::from(!outcome.errors.is_empty());
                // Counters are complete; diagnostic samples have a fixed bound.
                errors.extend(outcome.errors.into_iter().take(8 - errors.len()));
            }
            Err(_) => {
                report.failed_jobs += 1;
                report.unconfirmed_copies += job.copies;
                if errors.len() < 8 {
                    errors.push("GPU retirement thread panicked".into());
                }
            }
        }
    }

    fn retire(copies: usize, job: impl FnOnce() -> RetirementOutcome + Send + 'static) {
        let owner = std::thread::spawn(job);
        let mut registry = retirement_jobs()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let registry = &mut *registry;
        let mut index = 0;
        while index < registry.active.len() {
            if registry.active[index].owner.is_finished() {
                let finished = registry.active.swap_remove(index);
                collect_retirement(finished, &mut registry.report, &mut registry.errors);
            } else {
                index += 1;
            }
        }
        registry.report.registered_jobs += 1;
        registry.active.push(RetirementJob { copies, owner });
    }

    pub(super) fn finish_gpu_retirements() -> (GpuRetirementReport, Vec<String>) {
        let mut report = GpuRetirementReport::default();
        let mut errors = Vec::new();
        loop {
            let batch = std::mem::take(
                &mut *retirement_jobs()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            report.registered_jobs += batch.report.registered_jobs;
            report.joined_jobs += batch.report.joined_jobs;
            report.completed_copies += batch.report.completed_copies;
            report.unconfirmed_copies += batch.report.unconfirmed_copies;
            report.failed_jobs += batch.report.failed_jobs;
            errors.extend(batch.errors.into_iter().take(8 - errors.len()));
            if batch.active.is_empty() {
                break;
            }
            for job in batch.active {
                collect_retirement(job, &mut report, &mut errors);
            }
        }
        (report, errors)
    }

    const MAX_PENDING_COPIES: usize = 2;
    // Color32::from_rgba_unmultiplied uses this exact integer rounding. Keep
    // gamma-encoded UNORM bytes: using an Srgb source/view would decode them.
    pub const GPU_ALPHA_CONVERSION_WGSL: &str = r#"
@group(0) @binding(0) var source_image: texture_2d<f32>;
@group(0) @binding(1) var display_image: texture_storage_2d<rgba8unorm, write>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source_image);
    if (id.x >= size.x || id.y >= size.y) { return; }
    let pixel = vec4<u32>(round(textureLoad(source_image, vec2<i32>(id.xy), 0) * 255.0));
    let product = pixel.rgb * pixel.a + vec3<u32>(128u);
    let rgb = (product + (product >> vec3<u32>(8u))) >> vec3<u32>(8u);
    textureStore(display_image, vec2<i32>(id.xy), vec4<f32>(vec4<u32>(rgb, pixel.a)) / 255.0);
}
"#;

    struct BridgeState {
        render: RenderState,
        identity: AdapterIdentity,
        pipeline: wgpu::ComputePipeline,
        counters: Arc<Counters>,
    }

    #[derive(Default)]
    struct Counters {
        presentations: AtomicU64,
        busy_skips: AtomicU64,
        pending: AtomicUsize,
        high_water: AtomicUsize,
        diagnostic_bytes: AtomicU64,
    }

    /// Clone shares one initialized GUI device; each GpuTexture owns its display.
    #[derive(Clone)]
    pub struct GpuPresentationBridge {
        state: Arc<BridgeState>,
        target: Arc<dyn PresentationTarget>,
    }

    impl GpuPresentationBridge {
        pub fn new(render: RenderState) -> Result<Self, String> {
            let identity = platform::identity(&render)?;
            let shader = render
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("ModuleEngine byte-exact egui premultiplication"),
                    source: wgpu::ShaderSource::Wgsl(GPU_ALPHA_CONVERSION_WGSL.into()),
                });
            let pipeline =
                render
                    .device
                    .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                        label: Some("ModuleEngine GPU presentation conversion"),
                        layout: None,
                        module: &shader,
                        entry_point: Some("main"),
                        compilation_options: Default::default(),
                        cache: None,
                    });
            let state = Arc::new(BridgeState {
                render,
                identity,
                pipeline,
                counters: Arc::new(Counters::default()),
            });
            let target = Arc::new(VulkanPresentationTarget {
                identity: state.identity.clone(),
                allocator: state.clone(),
            });
            Ok(Self { state, target })
        }

        pub fn target(&self) -> Arc<dyn PresentationTarget> {
            self.target.clone()
        }

        pub fn counters(&self) -> GpuPresentationCounters {
            let counters = &self.state.counters;
            GpuPresentationCounters {
                gpu_presentations: counters.presentations.load(Ordering::Relaxed),
                busy_skips: counters.busy_skips.load(Ordering::Relaxed),
                pending_copies: counters.pending.load(Ordering::Relaxed),
                pending_high_water: counters.high_water.load(Ordering::Relaxed),
                diagnostic_readback_bytes: counters.diagnostic_bytes.load(Ordering::Relaxed),
                ..Default::default()
            }
        }
    }

    impl SharedImageAllocator for BridgeState {
        fn allocate(&self, width: u32, height: u32) -> Result<SharedImageInfo, String> {
            if width == 0
                || height == 0
                || width > self.render.device.limits().max_texture_dimension_2d
                || height > self.render.device.limits().max_texture_dimension_2d
            {
                return Err("GPU presentation dimensions exceed the GUI device limits".into());
            }
            platform::allocate(&self.render, width, height)
        }
    }

    struct GuiSharedImage {
        width: u32,
        height: u32,
        #[cfg(target_os = "linux")]
        native: Arc<platform::NativeImage>,
        #[cfg(target_os = "windows")]
        native: windows::Win32::Graphics::Direct3D12::ID3D12Resource,
        // Fields drop in declaration order: keep the GUI device pinned until
        // after the native resource has released its last driver reference.
        device: wgpu::Device,
    }

    impl std::fmt::Debug for GuiSharedImage {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("GuiSharedImage")
                .field("width", &self.width)
                .field("height", &self.height)
                .finish_non_exhaustive()
        }
    }
    impl GpuFrame for GuiSharedImage {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct Display {
        state: Arc<BridgeState>,
        texture: wgpu::Texture,
        view: wgpu::TextureView,
        id: egui::TextureId,
        dimensions: [usize; 2],
    }

    impl Drop for Display {
        fn drop(&mut self) {
            // Native registrations contain a bind group, not an owned texture.
            // Wgpu's submitted command references preserve the resource lifetime.
            self.state.render.renderer.write().free_texture(&self.id);
        }
    }

    /// One stable, GUI-owned display image per Scene/Preview/Player view.
    #[derive(Default)]
    pub struct GpuTexture {
        display: Option<Display>,
        pending: Vec<PendingCopy>,
        last_frame: Option<Weak<dyn GpuFrame>>,
        counters: GpuPresentationCounters,
    }

    struct PendingCopy {
        platform: platform::Pending,
        counters: Arc<Counters>,
    }
    impl PendingCopy {
        fn complete(&self) -> Result<bool, String> {
            self.platform.complete()
        }
    }
    impl Drop for PendingCopy {
        fn drop(&mut self) {
            self.counters.pending.fetch_sub(1, Ordering::Relaxed);
        }
    }

    impl GpuTexture {
        /// Retire completed source leases even when the producer has no free
        /// slot/new frame. Call this before attempting another presentation.
        pub fn poll(&mut self, bridge: &GpuPresentationBridge) -> Result<(), String> {
            bridge
                .state
                .render
                .device
                .poll(wgpu::PollType::Poll)
                .map_err(|error| format!("GPU presentation poll: {error}"))?;
            let mut index = 0;
            while index < self.pending.len() {
                if self.pending[index].complete()? {
                    self.pending.swap_remove(index);
                } else {
                    index += 1;
                }
            }
            Ok(())
        }
        pub fn texture_id(&self) -> Option<egui::TextureId> {
            self.display.as_ref().map(|d| d.id)
        }
        pub fn dimensions(&self) -> Option<[usize; 2]> {
            self.display.as_ref().map(|d| d.dimensions)
        }
        pub fn counters(&self) -> GpuPresentationCounters {
            GpuPresentationCounters {
                pending_copies: self.pending.len(),
                ..self.counters
            }
        }

        /// Copy/conversion is explicitly submitted before eframe's paint. A
        /// skipped/minimized surface therefore cannot release an unconsumed slot.
        pub fn update(
            &mut self,
            bridge: &GpuPresentationBridge,
            frame: &Frame,
        ) -> Result<Option<egui::TextureId>, String> {
            self.poll(bridge)?;
            let Some(lease) = &frame.gpu else {
                return Ok(None);
            };
            let image = lease
                .as_any()
                .downcast_ref::<GuiSharedImage>()
                .ok_or("GPU frame does not belong to the native GUI presentation bridge")?;
            if image.device != bridge.state.render.device
                || [image.width as usize, image.height as usize] != [frame.width, frame.height]
            {
                return Err(
                    "GPU frame device or dimensions do not match its presentation target".into(),
                );
            }
            let weak = Arc::downgrade(lease);
            if self
                .last_frame
                .as_ref()
                .is_some_and(|last| Weak::ptr_eq(last, &weak))
            {
                return Ok(None);
            }
            if self.pending.len() >= MAX_PENDING_COPIES {
                self.counters.busy_skips += 1;
                bridge
                    .state
                    .counters
                    .busy_skips
                    .fetch_add(1, Ordering::Relaxed);
                return Ok(None);
            }
            // Build a candidate first; failures leave the previous display intact.
            let candidate = if self.display.as_ref().is_none_or(|d| {
                d.dimensions != [frame.width, frame.height] || !Arc::ptr_eq(&d.state, &bridge.state)
            }) {
                let texture = bridge
                    .state
                    .render
                    .device
                    .create_texture(&texture_descriptor(
                        image.width,
                        image.height,
                        wgpu::TextureUsages::TEXTURE_BINDING
                            | wgpu::TextureUsages::STORAGE_BINDING
                            | wgpu::TextureUsages::COPY_SRC,
                    ));
                let view = texture.create_view(&Default::default());
                let id = bridge
                    .state
                    .render
                    .renderer
                    .write()
                    .register_native_texture(
                        &bridge.state.render.device,
                        &view,
                        wgpu::FilterMode::Linear,
                    );
                Some(Display {
                    state: bridge.state.clone(),
                    texture,
                    view,
                    id,
                    dimensions: [frame.width, frame.height],
                })
            } else {
                None
            };
            let display = candidate
                .as_ref()
                .or(self.display.as_ref())
                .expect("display candidate exists");
            // Wrap only an initialized, completed producer publication. A fresh
            // lightweight wgpu wrapper also resets tracking to the external
            // GENERAL/COMMON state on each use of a recycled native source.
            let source = platform::wrap(&bridge.state.render, image)?;
            let source_view = source.create_view(&Default::default());
            let group = bridge
                .state
                .render
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ModuleEngine presentation copy"),
                    layout: &bridge.state.pipeline.get_bind_group_layout(0),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&source_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&display.view),
                        },
                    ],
                });
            let mut encoder = bridge.state.render.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor {
                    label: Some("ModuleEngine GPU presentation"),
                },
            );
            #[cfg(target_os = "windows")]
            transition(&mut encoder, &source, wgpu::TextureUses::RESOURCE);
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("byte-exact premultiply"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&bridge.state.pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(image.width.div_ceil(8), image.height.div_ceil(8), 1);
            }
            #[cfg(target_os = "windows")]
            transition(&mut encoder, &source, wgpu::TextureUses::PRESENT);
            let pending =
                platform::submit(&bridge.state.render, image, lease.clone(), encoder.finish())?;
            let counters = bridge.state.counters.clone();
            let count = counters.pending.fetch_add(1, Ordering::Relaxed) + 1;
            counters.high_water.fetch_max(count, Ordering::Relaxed);
            counters.presentations.fetch_add(1, Ordering::Relaxed);
            self.pending.push(PendingCopy {
                platform: pending,
                counters,
            });
            if let Some(candidate) = candidate {
                self.display = Some(candidate);
            }
            self.last_frame = Some(weak);
            self.counters.gpu_presentations += 1;
            self.counters.pending_high_water =
                self.counters.pending_high_water.max(self.pending.len());
            Ok(self.texture_id())
        }

        /// Explicit diagnostic only: normal update never maps a pixel buffer.
        pub fn diagnostic_readback(&mut self) -> Result<Vec<u8>, String> {
            let display = self
                .display
                .as_ref()
                .ok_or("No GPU display image is available")?;
            let render = &display.state.render;
            let [width, height] = display.dimensions;
            let stride = (width as u32 * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = render.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("explicit GPU presentation diagnostic readback"),
                size: u64::from(stride) * height as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                render
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("diagnostic readback only"),
                    });
            encoder.copy_texture_to_buffer(
                display.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(stride),
                        rows_per_image: Some(height as u32),
                    },
                },
                wgpu::Extent3d {
                    width: width as u32,
                    height: height as u32,
                    depth_or_array_layers: 1,
                },
            );
            let submission = render.queue.submit([encoder.finish()]);
            let (send, receive) = std::sync::mpsc::sync_channel(1);
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = send.send(result);
                });
            render
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: Some(std::time::Duration::from_secs(30)),
                })
                .map_err(|e| format!("Diagnostic GPU completion: {e}"))?;
            receive
                .recv()
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            let mapped = buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|error| error.to_string())?;
            let mut pixels = Vec::with_capacity(width * height * 4);
            for row in mapped.chunks_exact(stride as usize) {
                pixels.extend_from_slice(&row[..width * 4]);
            }
            drop(mapped);
            buffer.unmap();
            self.counters.diagnostic_readback_bytes += pixels.len() as u64;
            display
                .state
                .counters
                .diagnostic_bytes
                .fetch_add(pixels.len() as u64, Ordering::Relaxed);
            Ok(pixels)
        }
    }

    impl Drop for GpuTexture {
        fn drop(&mut self) {
            if self.pending.is_empty() {
                return;
            }
            // Completion polling continues after pane/worker closure. Do not
            // block the UI thread or destroy another operation's queue/device.
            let mut pending = std::mem::take(&mut self.pending);
            retire(pending.len(), move || {
                let mut outcome = RetirementOutcome::default();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !pending.is_empty() {
                    let mut index = 0;
                    while index < pending.len() {
                        match pending[index].complete() {
                            Ok(false) => index += 1,
                            Ok(true) => {
                                pending.swap_remove(index);
                                outcome.completed_copies += 1;
                            }
                            Err(error) => {
                                // Completion is unknown. Keep the allocation,
                                // source lease and device alive instead of
                                // returning an in-flight slot to its producer.
                                std::mem::forget(pending.swap_remove(index));
                                outcome.unconfirmed_copies += 1;
                                outcome.errors.push(error);
                            }
                        }
                    }
                    if !pending.is_empty() {
                        if std::time::Instant::now() >= deadline {
                            outcome.unconfirmed_copies += pending.len();
                            outcome.errors.push(
                                "GPU retirement completion timed out after 30 seconds".into(),
                            );
                            std::mem::forget(pending);
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
                outcome
            });
        }
    }

    fn texture_descriptor(
        width: u32,
        height: u32,
        usage: wgpu::TextureUsages,
    ) -> wgpu::TextureDescriptor<'static> {
        wgpu::TextureDescriptor {
            label: Some("ModuleEngine native display RGBA bytes"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage,
            view_formats: &[],
        }
    }

    #[cfg(target_os = "windows")]
    fn transition(
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        state: wgpu::TextureUses,
    ) {
        encoder.transition_resources(
            std::iter::empty(),
            std::iter::once(wgpu::TextureTransition {
                texture,
                selector: None,
                state,
            }),
        );
    }

    #[cfg(target_os = "windows")]
    mod platform {
        use super::*;
        use module_engine_vulkan::presentation::SharedMemoryHandle;
        use std::{
            os::windows::io::{FromRawHandle, OwnedHandle},
            sync::atomic::{AtomicBool, Ordering},
        };
        use windows::{
            Win32::Graphics::{Direct3D12::*, Dxgi::Common::*},
            core::PCWSTR,
        };

        pub(super) fn identity(render: &RenderState) -> Result<AdapterIdentity, String> {
            let hal = unsafe { render.device.as_hal::<wgpu::hal::api::Dx12>() }
                .ok_or("Native Windows GPU presentation requires the selected WGPU/DX12 device")?;
            let luid = unsafe { hal.raw_device().GetAdapterLuid() };
            let mut identity = [0; 8];
            identity[..4].copy_from_slice(&luid.LowPart.to_ne_bytes());
            identity[4..].copy_from_slice(&luid.HighPart.to_ne_bytes());
            Ok(AdapterIdentity::WindowsLuid(identity))
        }

        pub(super) fn allocate(
            render: &RenderState,
            width: u32,
            height: u32,
        ) -> Result<SharedImageInfo, String> {
            let hal = unsafe { render.device.as_hal::<wgpu::hal::api::Dx12>() }
                .ok_or("WGPU/DX12 device disappeared")?;
            let device = hal.raw_device();
            let description = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Alignment: 0,
                Width: u64::from(width),
                Height: height,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
            };
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                CreationNodeMask: 1,
                VisibleNodeMask: 1,
                ..Default::default()
            };
            let mut resource: Option<ID3D12Resource> = None;
            unsafe {
                device.CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_SHARED,
                    &description,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut resource,
                )
            }
            .map_err(|e| format!("Create native shared DX12 texture: {e}"))?;
            let resource = resource.ok_or("DX12 returned no shared texture")?;
            let handle =
                unsafe { device.CreateSharedHandle(&resource, None, 0x1000_0000, PCWSTR::null()) }
                    .map_err(|e| format!("Export native shared DX12 texture: {e}"))?;
            let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };
            let allocation = unsafe { device.GetResourceAllocationInfo(0, &[description]) };
            Ok(SharedImageInfo {
                handle: SharedMemoryHandle::Win32(handle),
                allocation_size: allocation.SizeInBytes,
                memory_type_bits: u32::MAX,
                payload: Arc::new(GuiSharedImage {
                    width,
                    height,
                    device: render.device.clone(),
                    native: resource,
                }),
            })
        }

        pub(super) fn wrap(
            render: &RenderState,
            image: &GuiSharedImage,
        ) -> Result<wgpu::Texture, String> {
            let desc = texture_descriptor(
                image.width,
                image.height,
                wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
            );
            let native_texture = unsafe {
                wgpu::hal::dx12::Device::texture_from_raw(
                    image.native.clone(),
                    desc.format,
                    desc.dimension,
                    desc.size,
                    1,
                    1,
                )
            };
            Ok(unsafe {
                render
                    .device
                    .create_texture_from_hal::<wgpu::hal::api::Dx12>(
                        native_texture,
                        &desc,
                        wgpu::TextureUses::PRESENT,
                    )
            })
        }

        pub(super) struct Pending {
            device: wgpu::Device,
            done: Arc<AtomicBool>,
        }
        impl Pending {
            pub(super) fn complete(&self) -> Result<bool, String> {
                self.device
                    .poll(wgpu::PollType::Poll)
                    .map_err(|e| format!("GPU presentation completion: {e}"))?;
                Ok(self.done.load(Ordering::Acquire))
            }
        }
        pub(super) fn submit(
            render: &RenderState,
            _image: &GuiSharedImage,
            lease: Arc<dyn GpuFrame>,
            commands: wgpu::CommandBuffer,
        ) -> Result<Pending, String> {
            render.queue.submit([commands]);
            let done = Arc::new(AtomicBool::new(false));
            let completed = done.clone();
            render.queue.on_submitted_work_done(move || {
                drop(lease);
                completed.store(true, Ordering::Release);
            });
            Ok(Pending {
                device: render.device.clone(),
                done,
            })
        }
    }

    #[cfg(target_os = "linux")]
    mod platform {
        use super::*;
        use ash::vk;
        use module_engine_vulkan::presentation::SharedMemoryHandle;
        use std::os::fd::{FromRawFd, OwnedFd};

        pub(super) struct NativeImage {
            pub(super) device: ash::Device,
            pub(super) image: vk::Image,
            memory: vk::DeviceMemory,
            // The HAL wrapper does not own these allocations; this pin also
            // keeps the native device/instance alive until its final release.
            _gui_device: wgpu::Device,
        }
        impl Drop for NativeImage {
            fn drop(&mut self) {
                unsafe {
                    self.device.destroy_image(self.image, None);
                    self.device.free_memory(self.memory, None);
                }
            }
        }

        pub(super) fn identity(render: &RenderState) -> Result<AdapterIdentity, String> {
            let hal = unsafe { render.device.as_hal::<wgpu::hal::api::Vulkan>() }
                .ok_or("Native Linux GPU presentation requires the selected WGPU/Vulkan device")?;
            let mut ids = vk::PhysicalDeviceIDProperties::default();
            let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
            unsafe {
                hal.shared_instance()
                    .raw_instance()
                    .get_physical_device_properties2(hal.raw_physical_device(), &mut properties);
            }
            Ok(AdapterIdentity::VulkanUuid {
                device: ids.device_uuid,
                driver: ids.driver_uuid,
            })
        }

        pub(super) fn allocate(
            render: &RenderState,
            width: u32,
            height: u32,
        ) -> Result<SharedImageInfo, String> {
            let hal = unsafe { render.device.as_hal::<wgpu::hal::api::Vulkan>() }
                .ok_or("WGPU/Vulkan device disappeared")?;
            // Compiled Vulkan must not impose interop requirements on an active
            // software renderer. Check export support only for a real export.
            if !hal
                .enabled_device_extensions()
                .contains(&ash::khr::external_memory_fd::NAME)
            {
                return Err(
                    "The selected GUI Vulkan device has no VK_KHR_external_memory_fd support"
                        .into(),
                );
            }
            let mut external_format = vk::PhysicalDeviceExternalImageFormatInfo::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let format = vk::PhysicalDeviceImageFormatInfo2::default()
                .format(vk::Format::R8G8B8A8_UNORM)
                .ty(vk::ImageType::TYPE_2D)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(
                    vk::ImageUsageFlags::TRANSFER_SRC
                        | vk::ImageUsageFlags::TRANSFER_DST
                        | vk::ImageUsageFlags::SAMPLED,
                )
                .flags(vk::ImageCreateFlags::empty())
                .push_next(&mut external_format);
            let mut external_properties = vk::ExternalImageFormatProperties::default();
            let max_extent = {
                let mut format_properties =
                    vk::ImageFormatProperties2::default().push_next(&mut external_properties);
                unsafe {
                    hal.shared_instance()
                        .raw_instance()
                        .get_physical_device_image_format_properties2(
                            hal.raw_physical_device(),
                            &format,
                            &mut format_properties,
                        )
                }
                .map_err(|error| {
                    format!("GUI shared RGBA8 export format is unsupported: {error:?}")
                })?;
                format_properties.image_format_properties.max_extent
            };
            if !external_properties
                .external_memory_properties
                .external_memory_features
                .contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE)
                || !external_properties
                    .external_memory_properties
                    .compatible_handle_types
                    .contains(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD)
                || width > max_extent.width
                || height > max_extent.height
            {
                return Err(
                    "GUI Vulkan adapter cannot export the required dedicated shared RGBA8 image"
                        .into(),
                );
            }
            let device = hal.raw_device();
            let mut external = vk::ExternalMemoryImageCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let create = vk::ImageCreateInfo::default()
                .push_next(&mut external)
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(
                    vk::ImageUsageFlags::TRANSFER_SRC
                        | vk::ImageUsageFlags::TRANSFER_DST
                        | vk::ImageUsageFlags::SAMPLED,
                )
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .initial_layout(vk::ImageLayout::UNDEFINED);
            let image = unsafe { device.create_image(&create, None) }
                .map_err(|e| format!("Create GUI export Vulkan image: {e:?}"))?;
            let requirements = unsafe { device.get_image_memory_requirements(image) };
            let properties = unsafe {
                hal.shared_instance()
                    .raw_instance()
                    .get_physical_device_memory_properties(hal.raw_physical_device())
            };
            let Some(memory_index) = (0..properties.memory_type_count).find(|i| {
                requirements.memory_type_bits & (1 << i) != 0
                    && properties.memory_types[*i as usize]
                        .property_flags
                        .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            }) else {
                unsafe {
                    device.destroy_image(image, None);
                }
                return Err("No device-local export memory for GUI Vulkan image".into());
            };
            let mut export = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let memory = match unsafe {
                device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_index)
                        .push_next(&mut export)
                        .push_next(&mut dedicated),
                    None,
                )
            } {
                Ok(memory) => memory,
                Err(error) => {
                    unsafe {
                        device.destroy_image(image, None);
                    }
                    return Err(format!("Allocate GUI export Vulkan memory: {error:?}"));
                }
            };
            if let Err(error) = unsafe { device.bind_image_memory(image, memory, 0) } {
                unsafe {
                    device.destroy_image(image, None);
                    device.free_memory(memory, None);
                }
                return Err(format!("Bind GUI export Vulkan memory: {error:?}"));
            }
            let native = Arc::new(NativeImage {
                device: device.clone(),
                image,
                memory,
                _gui_device: render.device.clone(),
            });
            let extension = ash::khr::external_memory_fd::Device::new(
                hal.shared_instance().raw_instance(),
                device,
            );
            let fd = unsafe {
                extension.get_memory_fd(
                    &vk::MemoryGetFdInfoKHR::default()
                        .memory(memory)
                        .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD),
                )
            }
            .map_err(|e| format!("Export GUI Vulkan memory FD: {e:?}"))?;
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            Ok(SharedImageInfo {
                handle: SharedMemoryHandle::OpaqueFd(fd),
                allocation_size: requirements.size,
                memory_type_bits: 1 << memory_index,
                payload: Arc::new(GuiSharedImage {
                    width,
                    height,
                    device: render.device.clone(),
                    native,
                }),
            })
        }

        pub(super) fn wrap(
            render: &RenderState,
            image: &GuiSharedImage,
        ) -> Result<wgpu::Texture, String> {
            let hal = unsafe { render.device.as_hal::<wgpu::hal::api::Vulkan>() }
                .ok_or("WGPU/Vulkan device disappeared")?;
            let desc = texture_descriptor(
                image.width,
                image.height,
                wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
            );
            let hal_desc = wgpu::hal::TextureDescriptor {
                label: desc.label,
                size: desc.size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: desc.dimension,
                format: desc.format,
                usage: wgpu::TextureUses::RESOURCE
                    | wgpu::TextureUses::COPY_SRC
                    | wgpu::TextureUses::COPY_DST,
                memory_flags: wgpu::hal::MemoryFlags::empty(),
                view_formats: vec![],
            };
            let owner = image.native.clone();
            let hal_texture = unsafe {
                hal.texture_from_raw(
                    image.native.image,
                    &hal_desc,
                    Some(Box::new(move || drop(owner))),
                    wgpu::hal::vulkan::TextureMemory::External,
                )
            };
            drop(hal);
            // The producer published initialized GENERAL contents. In pinned
            // HAL, the legal inclusive COPY_SRC|RESOURCE state maps to GENERAL;
            // the compute pass then records GENERAL -> SHADER_READ_ONLY_OPTIMAL.
            Ok(unsafe {
                render
                    .device
                    .create_texture_from_hal::<wgpu::hal::api::Vulkan>(
                        hal_texture,
                        &desc,
                        wgpu::TextureUses::RESOURCE | wgpu::TextureUses::COPY_SRC,
                    )
            })
        }

        pub(super) struct Pending {
            native: Arc<NativeImage>,
            pool: vk::CommandPool,
            fence: vk::Fence,
            _lease: Arc<dyn GpuFrame>,
        }
        impl Pending {
            pub(super) fn complete(&self) -> Result<bool, String> {
                unsafe { self.native.device.get_fence_status(self.fence) }
                    .map_err(|e| format!("GPU presentation release fence: {e:?}"))
            }
        }
        impl Drop for Pending {
            fn drop(&mut self) {
                unsafe {
                    self.native.device.destroy_fence(self.fence, None);
                    self.native.device.destroy_command_pool(self.pool, None);
                }
            }
        }

        pub(super) fn submit(
            render: &RenderState,
            image: &GuiSharedImage,
            lease: Arc<dyn GpuFrame>,
            commands: wgpu::CommandBuffer,
        ) -> Result<Pending, String> {
            // Only called on the UI thread, adjacent to our own wgpu submit.
            // The worker allocator never touches the GUI queue.
            let hal = unsafe { render.device.as_hal::<wgpu::hal::api::Vulkan>() }
                .ok_or("WGPU/Vulkan device disappeared")?;
            let device = hal.raw_device();
            let pool = unsafe {
                device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(hal.queue_family_index()),
                    None,
                )
            }
            .map_err(|e| format!("Presentation command pool: {e:?}"))?;
            let fence = match unsafe { device.create_fence(&Default::default(), None) } {
                Ok(fence) => fence,
                Err(error) => {
                    unsafe {
                        device.destroy_command_pool(pool, None);
                    }
                    return Err(format!("Presentation release fence: {error:?}"));
                }
            };
            let pending = Pending {
                native: image.native.clone(),
                pool,
                fence,
                _lease: lease,
            };
            let buffers = unsafe {
                device.allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(2),
                )
            }
            .map_err(|e| format!("Presentation command buffers: {e:?}"))?;
            for (index, &command) in buffers.iter().enumerate() {
                unsafe {
                    device.begin_command_buffer(
                        command,
                        &vk::CommandBufferBeginInfo::default()
                            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                    )
                }
                .map_err(|e| format!("Begin presentation barrier: {e:?}"))?;
                let acquire = index == 0;
                let barrier = vk::ImageMemoryBarrier::default()
                    .image(image.native.image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .old_layout(if acquire {
                        vk::ImageLayout::GENERAL
                    } else {
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                    })
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(if acquire {
                        vk::QUEUE_FAMILY_EXTERNAL
                    } else {
                        hal.queue_family_index()
                    })
                    .dst_queue_family_index(if acquire {
                        hal.queue_family_index()
                    } else {
                        vk::QUEUE_FAMILY_EXTERNAL
                    })
                    .src_access_mask(if acquire {
                        vk::AccessFlags::empty()
                    } else {
                        vk::AccessFlags::SHADER_READ
                    })
                    .dst_access_mask(if acquire {
                        vk::AccessFlags::SHADER_READ
                    } else {
                        vk::AccessFlags::empty()
                    });
                unsafe {
                    device.cmd_pipeline_barrier(
                        command,
                        if acquire {
                            vk::PipelineStageFlags::TOP_OF_PIPE
                        } else {
                            vk::PipelineStageFlags::COMPUTE_SHADER
                        },
                        if acquire {
                            vk::PipelineStageFlags::COMPUTE_SHADER
                        } else {
                            vk::PipelineStageFlags::BOTTOM_OF_PIPE
                        },
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &[barrier],
                    );
                    device
                        .end_command_buffer(command)
                        .map_err(|e| format!("End presentation barrier: {e:?}"))?;
                }
            }
            unsafe {
                device.queue_submit(
                    hal.raw_queue(),
                    &[vk::SubmitInfo::default().command_buffers(&buffers[..1])],
                    vk::Fence::null(),
                )
            }
            .map_err(|e| format!("Submit presentation acquire: {e:?}"))?;
            // Release the HAL guard before re-entering wgpu.
            let queue = hal.raw_queue();
            let device = device.clone();
            drop(hal);
            render.queue.submit([commands]);
            if let Err(error) = unsafe {
                device.queue_submit(
                    queue,
                    &[vk::SubmitInfo::default().command_buffers(&buffers[1..])],
                    fence,
                )
            } {
                // A failed release must not return a still-read slot to producer.
                // Keep it pinned until already-submitted work has completed.
                let retirement = pending;
                let gui = render.device.clone();
                retire(1, move || {
                    let completion = gui.poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: Some(std::time::Duration::from_secs(30)),
                    });
                    // A lost ownership release is not a reusable slot. Preserve
                    // its lease permanently rather than return it with an
                    // invalid EXTERNAL state to the bounded producer pool.
                    std::mem::forget(retirement);
                    RetirementOutcome {
                        completed_copies: 0,
                        unconfirmed_copies: 1,
                        errors: vec![match completion {
                            Ok(_) => "GPU presentation ownership release failed; source lease remains pinned".into(),
                            Err(error) => format!("GPU presentation ownership release failed and completion is unconfirmed: {error}"),
                        }],
                    }
                });
                return Err(format!("Submit presentation release: {error:?}"));
            }
            Ok(pending)
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    mod platform {
        use super::*;
        pub(super) fn identity(_: &RenderState) -> Result<AdapterIdentity, String> {
            Err(
                "Native GPU presentation is available only for Windows DX12 and Linux Vulkan"
                    .into(),
            )
        }
        pub(super) fn allocate(_: &RenderState, _: u32, _: u32) -> Result<SharedImageInfo, String> {
            Err("Native GPU presentation platform is unavailable".into())
        }
        pub(super) fn wrap(_: &RenderState, _: &GuiSharedImage) -> Result<wgpu::Texture, String> {
            Err("Native GPU presentation platform is unavailable".into())
        }
        pub(super) struct Pending;
        impl Pending {
            pub(super) fn complete(&self) -> Result<bool, String> {
                Ok(true)
            }
        }
        pub(super) fn submit(
            _: &RenderState,
            _: &GuiSharedImage,
            _: Arc<dyn GpuFrame>,
            _: wgpu::CommandBuffer,
        ) -> Result<Pending, String> {
            Err("Native GPU presentation platform is unavailable".into())
        }
    }
}

#[cfg(not(feature = "vulkan"))]
mod unavailable {
    use eframe::{egui, egui_wgpu::RenderState};
    use module_engine_core::{Frame, PresentationTarget};
    use std::{any::Any, sync::Arc};
    #[derive(Clone)]
    pub struct GpuPresentationBridge;
    impl GpuPresentationBridge {
        pub fn new(_: RenderState) -> Result<Self, String> {
            Err("This Player has no compiled Vulkan presentation module".into())
        }
        pub fn target(&self) -> Arc<dyn PresentationTarget> {
            Arc::new(Unavailable)
        }
        pub fn counters(&self) -> super::GpuPresentationCounters {
            Default::default()
        }
    }
    struct Unavailable;
    impl PresentationTarget for Unavailable {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    #[derive(Default)]
    pub struct GpuTexture;
    impl GpuTexture {
        pub fn poll(&mut self, _: &GpuPresentationBridge) -> Result<(), String> {
            Ok(())
        }
        pub fn update(
            &mut self,
            _: &GpuPresentationBridge,
            _: &Frame,
        ) -> Result<Option<egui::TextureId>, String> {
            Ok(None)
        }
        pub fn texture_id(&self) -> Option<egui::TextureId> {
            None
        }
        pub fn dimensions(&self) -> Option<[usize; 2]> {
            None
        }
        pub fn counters(&self) -> super::GpuPresentationCounters {
            Default::default()
        }
        pub fn diagnostic_readback(&mut self) -> Result<Vec<u8>, String> {
            Err("No GPU presentation module is compiled".into())
        }
    }
}
#[cfg(not(feature = "vulkan"))]
pub use unavailable::{GpuPresentationBridge, GpuTexture};
