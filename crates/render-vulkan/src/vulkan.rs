#[cfg(test)]
use crate::{compiler::compile_hlsl, graph::build_plan};
use crate::{
    compiler::{compile_hlsl_at, verify_shader},
    geometry,
    graph::{Pass, Plan, build_metadata_plan, build_plan_at},
};
use ash::{Entry, vk};
use module_engine_core::{
    Camera, Frame, Graph, RenderOverrides, RenderResourceCommand, RenderStats, RendererPerformance,
    SceneSnapshot,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString},
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

const INCLUDE_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Debug)]
enum AllocationFault {
    BufferHandle,
    BufferMemory,
    ImageHandle,
    ImageMemory,
    ImageView,
}
#[cfg(test)]
#[derive(Default)]
struct AllocationCounts {
    buffers: std::sync::atomic::AtomicUsize,
    images: std::sync::atomic::AtomicUsize,
    views: std::sync::atomic::AtomicUsize,
    memory: std::sync::atomic::AtomicUsize,
}
#[cfg(test)]
impl AllocationCounts {
    fn snapshot(&self) -> [usize; 4] {
        use std::sync::atomic::Ordering::Relaxed;
        [
            self.buffers.load(Relaxed),
            self.images.load(Relaxed),
            self.views.load(Relaxed),
            self.memory.load(Relaxed),
        ]
    }
}
struct RejectedPreparation {
    graph: Graph,
    sources: BTreeMap<String, String>,
    signature: Option<Vec<String>>,
    checked: Instant,
    error: String,
}
impl RejectedPreparation {
    fn matches(&self, graph: &Graph, sources: &BTreeMap<String, String>) -> bool {
        self.graph.execution_eq(graph) && &self.sources == sources
    }
}

struct Image {
    device: ash::Device,
    image: vk::Image,
    view: vk::ImageView,
    memory: vk::DeviceMemory,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
    layout: vk::ImageLayout,
    stage: vk::PipelineStageFlags,
    access: vk::AccessFlags,
    #[cfg(test)]
    allocations: std::sync::Arc<AllocationCounts>,
}
impl Drop for Image {
    fn drop(&mut self) {
        unsafe {
            if self.view != vk::ImageView::null() {
                self.device.destroy_image_view(self.view, None);
                #[cfg(test)]
                self.allocations
                    .views
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
            if self.image != vk::Image::null() {
                self.device.destroy_image(self.image, None);
                #[cfg(test)]
                self.allocations
                    .images
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
                #[cfg(test)]
                self.allocations
                    .memory
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}
struct Buffer {
    device: ash::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: u64,
    #[cfg(test)]
    allocations: std::sync::Arc<AllocationCounts>,
}
impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            if self.buffer != vk::Buffer::null() {
                self.device.destroy_buffer(self.buffer, None);
                #[cfg(test)]
                self.allocations
                    .buffers
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
            if self.memory != vk::DeviceMemory::null() {
                self.device.free_memory(self.memory, None);
                #[cfg(test)]
                self.allocations
                    .memory
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}
struct Pipeline {
    device: ash::Device,
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    compute: bool,
}
struct FrameResources {
    key: FrameResourceKey,
    images: BTreeMap<String, Image>,
    vertex: Buffer,
    staging: Buffer,
}
/// Allocation identity is independent from shader/pipeline identity. Reusing a
/// pipeline under a new pass ID must still allocate its newly named resources.
#[derive(Debug, PartialEq, Eq)]
struct FrameResourceKey {
    size: [usize; 2],
    images: Vec<(String, String, bool)>,
    readback: Vec<(String, usize, usize)>,
}
fn frame_readback(
    plan: &Plan,
    width: usize,
    height: usize,
    detailed: bool,
) -> Vec<(String, usize, usize)> {
    let mut readback = vec![(plan.output.clone(), 0, width * height * 4)];
    if !detailed {
        return readback;
    }
    if let Some(pass) = plan
        .passes
        .iter()
        .find(|pass| pass.geometry() && pass.module != "gpu.custom")
    {
        for (name, kind, bytes) in [("depth", 1, 4), ("normals", 2, 16)] {
            if let Some(spec) = pass.manifest.outputs.iter().find(|spec| spec.name == name) {
                readback.push((pass.resource(&spec.name), kind, width * height * bytes));
            }
        }
        if let Some(spec) = pass
            .manifest
            .outputs
            .iter()
            .find(|spec| spec.name == "color" || spec.name == "albedo")
        {
            readback.push((pass.resource(&spec.name), 3, width * height * 16));
        }
    } else if let Some(pass) = plan.passes.iter().find(|pass| pass.geometry()) {
        readback.push((
            format!("{}.__hardware_depth", pass.id),
            4,
            width * height * 4,
        ));
    }
    readback
}
fn frame_resource_key(
    plan: &Plan,
    width: usize,
    height: usize,
    readback: &[(String, usize, usize)],
) -> FrameResourceKey {
    let mut images = Vec::new();
    for pass in &plan.passes {
        images.extend(
            pass.manifest
                .outputs
                .iter()
                .map(|spec| (pass.resource(&spec.name), spec.format.clone(), false)),
        );
        if pass.geometry() {
            images.push((
                format!("{}.__hardware_depth", pass.id),
                "d32float".into(),
                true,
            ));
        }
    }
    FrameResourceKey {
        size: [width, height],
        images,
        readback: readback.to_vec(),
    }
}
fn validate_frame_images(
    plan: &Plan,
    available: &BTreeSet<&str>,
    readback: &[(String, usize, usize)],
) -> Result<(), String> {
    let mut declared = BTreeSet::new();
    for pass in &plan.passes {
        for (_, resource) in pass
            .inputs
            .iter()
            .filter(|(spec, _)| spec.format != "scene")
        {
            if !available.contains(resource.as_str()) {
                return Err(format!(
                    "GPU pass {} input resource {resource} has no allocated image",
                    pass.id
                ));
            }
        }
        for spec in &pass.manifest.outputs {
            let resource = pass.resource(&spec.name);
            if !declared.insert(resource.clone()) {
                return Err(format!(
                    "GPU output resource identity collision: {resource}"
                ));
            }
            if !available.contains(resource.as_str()) {
                return Err(format!(
                    "GPU pass {} output resource {resource} has no allocated image",
                    pass.id
                ));
            }
        }
        if pass.geometry() {
            let resource = format!("{}.__hardware_depth", pass.id);
            if !declared.insert(resource.clone()) {
                return Err(format!("GPU depth resource identity collision: {resource}"));
            }
            if !available.contains(resource.as_str()) {
                return Err(format!(
                    "GPU pass {} hardware depth has no allocated image",
                    pass.id
                ));
            }
        }
    }
    for (resource, _, _) in readback {
        if !available.contains(resource.as_str()) {
            return Err(format!(
                "GPU readback resource {resource} has no allocated image"
            ));
        }
    }
    Ok(())
}
struct AtlasGpu {
    keys: Vec<usize>,
    atlas: geometry::TextureAtlas,
    image: Image,
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device
                .destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}
pub struct VulkanRenderer {
    resource_root: std::path::PathBuf,
    _entry: Entry,
    instance: ash::Instance,
    device: ash::Device,
    physical: vk::PhysicalDevice,
    memory: vk::PhysicalDeviceMemoryProperties,
    queue: vk::Queue,
    pool: vk::CommandPool,
    command: vk::CommandBuffer,
    descriptor_pool: vk::DescriptorPool,
    name: String,
    pipelines: BTreeMap<String, Pipeline>,
    prepared: Option<Plan>,
    diagnostics: Vec<String>,
    frame_resources: Option<FrameResources>,
    atlas: Option<AtlasGpu>,
    detailed_stats: bool,
    prepared_inputs: Option<(Graph, BTreeMap<String, String>, Instant)>,
    rejected_inputs: Option<RejectedPreparation>,
    performance: RendererPerformance,
    #[cfg(test)]
    allocation_fault: Option<AllocationFault>,
    #[cfg(test)]
    allocations: std::sync::Arc<AllocationCounts>,
}
impl VulkanRenderer {
    pub fn new() -> Result<Self, String> {
        Self::new_at(crate::compiler::resource_root())
    }
    /// Bind this backend's shader compilation to one explicit package root.
    pub fn new_at(resource_root: std::path::PathBuf) -> Result<Self, String> {
        unsafe {
            let entry = Entry::load().map_err(|e| format!("Vulkan loader: {e}"))?;
            let app = CString::new("ModuleEngine GPU graph").unwrap();
            let info = vk::ApplicationInfo::default()
                .application_name(&app)
                .api_version(vk::API_VERSION_1_3);
            let instance = entry
                .create_instance(
                    &vk::InstanceCreateInfo::default().application_info(&info),
                    None,
                )
                .map_err(|e| format!("Vulkan 1.3 instance: {e:?}"))?;
            let candidates = instance
                .enumerate_physical_devices()
                .map_err(|e| format!("Vulkan adapters: {e:?}"))?;
            let selected = candidates
                .into_iter()
                .filter_map(|physical| {
                    let properties = instance.get_physical_device_properties(physical);
                    if properties.api_version < vk::API_VERSION_1_3 {
                        return None;
                    }
                    let family = instance
                        .get_physical_device_queue_family_properties(physical)
                        .iter()
                        .position(|p| {
                            p.queue_flags
                                .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                        })?;
                    let mut rendering = vk::PhysicalDeviceDynamicRenderingFeatures::default();
                    let mut features =
                        vk::PhysicalDeviceFeatures2::default().push_next(&mut rendering);
                    instance.get_physical_device_features2(physical, &mut features);
                    if rendering.dynamic_rendering == 0 {
                        return None;
                    }
                    Some((physical, family as u32, properties))
                })
                .max_by_key(|(_, _, p)| {
                    if p.device_type == vk::PhysicalDeviceType::DISCRETE_GPU {
                        2
                    } else {
                        1
                    }
                });
            let Some((physical, family, properties)) = selected else {
                instance.destroy_instance(None);
                return Err("No Vulkan 1.3 graphics+compute adapter with dynamic rendering; a compatible GPU driver is required".into());
            };
            let name = CStr::from_ptr(properties.device_name.as_ptr())
                .to_string_lossy()
                .into_owned();
            let priorities = [1.];
            let queues = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(family)
                .queue_priorities(&priorities)];
            let mut rendering =
                vk::PhysicalDeviceDynamicRenderingFeatures::default().dynamic_rendering(true);
            let device = instance
                .create_device(
                    physical,
                    &vk::DeviceCreateInfo::default()
                        .queue_create_infos(&queues)
                        .push_next(&mut rendering),
                    None,
                )
                .map_err(|e| format!("Vulkan device {name}: {e:?}"))?;
            let queue = device.get_device_queue(family, 0);
            let memory = instance.get_physical_device_memory_properties(physical);
            let pool = device
                .create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(family)
                        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                    None,
                )
                .map_err(vk_error)?;
            let command = device
                .allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
                .map_err(vk_error)?[0];
            let sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::SAMPLED_IMAGE,
                    descriptor_count: 1024,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::STORAGE_IMAGE,
                    descriptor_count: 1024,
                },
            ];
            let descriptor_pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(128)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(vk_error)?;
            Ok(Self {
                _entry: entry,
                instance,
                device,
                physical,
                memory,
                queue,
                pool,
                command,
                descriptor_pool,
                name,
                pipelines: BTreeMap::new(),
                prepared: None,
                diagnostics: vec![],
                resource_root,
                frame_resources: None,
                atlas: None,
                detailed_stats: true,
                prepared_inputs: None,
                rejected_inputs: None,
                performance: RendererPerformance::default(),
                #[cfg(test)]
                allocation_fault: None,
                #[cfg(test)]
                allocations: std::sync::Arc::default(),
            })
        }
    }
    pub fn device_name(&self) -> &str {
        &self.name
    }
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
    /// Player mode avoids reading depth/normal/color diagnostics every frame.
    /// Geometry submission counts remain available; visibility counts are omitted.
    pub fn set_detailed_stats(&mut self, enabled: bool) {
        self.detailed_stats = enabled;
    }
    pub fn performance(&self) -> RendererPerformance {
        self.performance
    }
    /// Explicit preparation always fingerprints current include contents. Only
    /// ordinary render calls coalesce unchanged-source dependency scans.
    pub fn prepare(
        &mut self,
        graph: &Graph,
        sources: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        self.prepare_checked(graph, sources, false)
    }
    fn reject_preparation(
        &mut self,
        graph: &Graph,
        sources: &BTreeMap<String, String>,
        signature: Option<Vec<String>>,
        error: String,
    ) -> Result<(), String> {
        self.diagnostics = vec![error.clone()];
        self.rejected_inputs = Some(RejectedPreparation {
            graph: graph.clone(),
            sources: sources.clone(),
            signature,
            checked: Instant::now(),
            error: error.clone(),
        });
        Err(error)
    }
    fn prepare_checked(
        &mut self,
        graph: &Graph,
        sources: &BTreeMap<String, String>,
        reuse_rejection: bool,
    ) -> Result<(), String> {
        self.performance.prepare_calls += 1;
        self.performance.plan_builds += 1;
        let started = Instant::now();
        let plan = match build_plan_at(graph, sources, &self.resource_root) {
            Ok(plan) => plan,
            Err(error) => {
                return self.reject_preparation(graph, sources, None, error);
            }
        };
        let signature = plan.passes.iter().map(pipeline_key).collect::<Vec<_>>();
        if reuse_rejection
            && let Some(rejected) = &mut self.rejected_inputs
            && rejected.matches(graph, sources)
            && rejected.signature.as_ref() == Some(&signature)
        {
            rejected.checked = Instant::now();
            self.performance.rejected_cache_hits += 1;
            return Err(rejected.error.clone());
        }
        if let Err(error) = self.prepare_plan(plan) {
            return self.reject_preparation(graph, sources, Some(signature), error);
        }
        self.prepared_inputs = Some((graph.clone(), sources.clone(), Instant::now()));
        self.rejected_inputs = None;
        self.performance.prepare_ns += started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.diagnostics.clear();
        Ok(())
    }
    fn prepare_plan(&mut self, plan: Plan) -> Result<(), String> {
        if let Err(error) = schedule_overrides(&plan, &RenderOverrides::default()) {
            self.diagnostics = vec![error.clone()];
            return Err(error);
        }
        if plan.passes.len() > 64 {
            return Err("GPU graph exceeds 64 passes".into());
        }
        if self.pipelines.len() > 256 {
            let mut keep: BTreeSet<_> = plan.passes.iter().map(pipeline_key).collect();
            if let Some(active) = &self.prepared {
                keep.extend(active.passes.iter().map(pipeline_key));
            }
            self.pipelines.retain(|key, _| keep.contains(key));
        }
        for pass in &plan.passes {
            let key = pipeline_key(pass);
            if !self.pipelines.contains_key(&key) {
                self.performance.pipeline_build_attempts += 1;
                match self.create_pipeline(pass) {
                    Ok(pipeline) => {
                        if crate::compiler::dependency_fingerprint_at(
                            &self.resource_root,
                            &pass.source,
                        )? != pass.dependency_hash
                        {
                            return Err(format!(
                                "Pass {}: HLSL dependencies changed during pipeline preparation",
                                pass.id
                            ));
                        }
                        self.pipelines.insert(key, pipeline);
                        self.performance.pipeline_creations += 1;
                    }
                    Err(error) => {
                        self.diagnostics = vec![format!("Pass {}: {error}", pass.id)];
                        return Err(self.diagnostics[0].clone());
                    }
                }
            }
        }
        self.prepared = Some(plan);
        Ok(())
    }
    fn prepare_for_render(
        &mut self,
        graph: &Graph,
        sources: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        if let Some(rejected) = &self.rejected_inputs
            && rejected.matches(graph, sources)
        {
            if rejected.checked.elapsed() < INCLUDE_REFRESH_INTERVAL {
                self.performance.rejected_cache_hits += 1;
                return Err(rejected.error.clone());
            }
            return self.prepare_checked(graph, sources, true);
        }
        if self
            .prepared_inputs
            .as_ref()
            .is_some_and(|(previous, text, checked)| {
                checked.elapsed() < INCLUDE_REFRESH_INTERVAL
                    && previous.execution_eq(graph)
                    && text == sources
            })
        {
            self.performance.plan_cache_hits += 1;
            return Ok(());
        }
        self.prepare_checked(graph, sources, true)
    }
    fn memory_type(&self, bits: u32, required: vk::MemoryPropertyFlags) -> Result<u32, String> {
        (0..self.memory.memory_type_count)
            .find(|i| {
                bits & (1 << i) != 0
                    && self.memory.memory_types[*i as usize]
                        .property_flags
                        .contains(required)
            })
            .ok_or_else(|| format!("Vulkan memory type {required:?} unavailable"))
    }
    fn buffer(&self, size: u64, usage: vk::BufferUsageFlags) -> Result<Buffer, String> {
        unsafe {
            let size = size.max(4);
            let buffer = self
                .device
                .create_buffer(
                    &vk::BufferCreateInfo::default()
                        .size(size)
                        .usage(usage)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .map_err(vk_error)?;
            #[cfg(test)]
            self.allocations
                .buffers
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut allocated = Buffer {
                device: self.device.clone(),
                buffer,
                memory: vk::DeviceMemory::null(),
                size,
                #[cfg(test)]
                allocations: self.allocations.clone(),
            };
            #[cfg(test)]
            self.allocation_checkpoint(AllocationFault::BufferHandle)?;
            let requirements = self.device.get_buffer_memory_requirements(buffer);
            let host =
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            // CPU frame reads must prefer cached system memory. On a discrete
            // adapter the first HOST_VISIBLE type can be an uncached BAR mapping,
            // whose read bandwidth dominates a 1080p frame despite fast GPU work.
            let memory_type = if usage.contains(vk::BufferUsageFlags::TRANSFER_DST) {
                self.memory_type(
                    requirements.memory_type_bits,
                    host | vk::MemoryPropertyFlags::HOST_CACHED,
                )
                .or_else(|_| self.memory_type(requirements.memory_type_bits, host))?
            } else {
                self.memory_type(requirements.memory_type_bits, host)?
            };
            let allocation = vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type);
            let memory = self
                .device
                .allocate_memory(&allocation, None)
                .map_err(vk_error)?;
            allocated.memory = memory;
            #[cfg(test)]
            self.allocations
                .memory
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            #[cfg(test)]
            self.allocation_checkpoint(AllocationFault::BufferMemory)?;
            self.device
                .bind_buffer_memory(buffer, memory, 0)
                .map_err(vk_error)?;
            Ok(allocated)
        }
    }
    fn image(
        &self,
        width: u32,
        height: u32,
        format: vk::Format,
        depth: bool,
    ) -> Result<Image, String> {
        unsafe {
            let props = self
                .instance
                .get_physical_device_format_properties(self.physical, format);
            let required = if depth {
                vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT
                    | vk::FormatFeatureFlags::TRANSFER_SRC
            } else {
                vk::FormatFeatureFlags::COLOR_ATTACHMENT
                    | vk::FormatFeatureFlags::STORAGE_IMAGE
                    | vk::FormatFeatureFlags::SAMPLED_IMAGE
                    | vk::FormatFeatureFlags::TRANSFER_SRC
                    | vk::FormatFeatureFlags::TRANSFER_DST
            };
            if !props.optimal_tiling_features.contains(required) {
                return Err(format!(
                    "Adapter {} lacks required format capabilities {format:?}: {required:?}",
                    self.name
                ));
            }
            let usage = if depth {
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC
            } else {
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST
            };
            let info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(usage)
                .initial_layout(vk::ImageLayout::UNDEFINED);
            let image = self.device.create_image(&info, None).map_err(vk_error)?;
            #[cfg(test)]
            self.allocations
                .images
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let aspect = if depth {
                vk::ImageAspectFlags::DEPTH
            } else {
                vk::ImageAspectFlags::COLOR
            };
            let mut allocated = Image {
                device: self.device.clone(),
                image,
                view: vk::ImageView::null(),
                memory: vk::DeviceMemory::null(),
                format,
                aspect,
                layout: vk::ImageLayout::UNDEFINED,
                stage: vk::PipelineStageFlags::TOP_OF_PIPE,
                access: vk::AccessFlags::empty(),
                #[cfg(test)]
                allocations: self.allocations.clone(),
            };
            #[cfg(test)]
            self.allocation_checkpoint(AllocationFault::ImageHandle)?;
            let requirements = self.device.get_image_memory_requirements(image);
            let memory = self
                .device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(self.memory_type(
                            requirements.memory_type_bits,
                            vk::MemoryPropertyFlags::DEVICE_LOCAL,
                        )?),
                    None,
                )
                .map_err(vk_error)?;
            allocated.memory = memory;
            #[cfg(test)]
            self.allocations
                .memory
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            #[cfg(test)]
            self.allocation_checkpoint(AllocationFault::ImageMemory)?;
            self.device
                .bind_image_memory(image, memory, 0)
                .map_err(vk_error)?;
            let view = self
                .device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(range(aspect)),
                    None,
                )
                .map_err(vk_error)?;
            allocated.view = view;
            #[cfg(test)]
            self.allocations
                .views
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            #[cfg(test)]
            self.allocation_checkpoint(AllocationFault::ImageView)?;
            Ok(allocated)
        }
    }
    #[cfg(test)]
    fn allocation_checkpoint(&self, stage: AllocationFault) -> Result<(), String> {
        if self.allocation_fault == Some(stage) {
            Err(format!("Injected Vulkan allocation failure: {stage:?}"))
        } else {
            Ok(())
        }
    }
    fn create_pipeline(&self, pass: &Pass) -> Result<Pipeline, String> {
        unsafe {
            let compute = pass.manifest.stage == "compute";
            let profile = if compute { "cs_6_0" } else { "ps_6_0" };
            let shader = compile_hlsl_at(
                &self.resource_root,
                &pass.source,
                &pass.manifest.entry,
                profile,
            )?;
            verify_shader(pass, &shader.words, false)?;
            // Compile and validate BOTH stages before allocating Vulkan handles.
            // A rejected vertex candidate cannot leak the fragment/layout handles.
            let vertex_artifact = if compute {
                None
            } else {
                let vertex_source = if pass.manifest.vertex_entry.is_some() {
                    pass.source.as_str()
                } else if pass.geometry() {
                    include_str!("../../../assets/gpu/forward.hlsl")
                } else {
                    include_str!("../../../assets/gpu/fullscreen.hlsl")
                };
                let entry = pass.manifest.vertex_entry.as_deref().unwrap_or("mainVS");
                let artifact =
                    compile_hlsl_at(&self.resource_root, vertex_source, entry, "vs_6_0")?;
                verify_shader(pass, &artifact.words, true)?;
                Some(artifact)
            };
            let inputs = pass
                .manifest
                .inputs
                .iter()
                .filter(|s| s.format != "scene")
                .count()
                + usize::from(pass.manifest.scene_materials);
            let descriptor_count = inputs
                + if compute {
                    pass.manifest.outputs.len()
                } else {
                    0
                };
            let bindings: Vec<_> = (0..descriptor_count)
                .map(|i| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(i as u32)
                        .descriptor_type(if i < inputs {
                            vk::DescriptorType::SAMPLED_IMAGE
                        } else {
                            vk::DescriptorType::STORAGE_IMAGE
                        })
                        .descriptor_count(1)
                        .stage_flags(if compute {
                            vk::ShaderStageFlags::COMPUTE
                        } else {
                            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT
                        })
                })
                .collect();
            let set_layout = self
                .device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(vk_error)?;
            let layouts = [set_layout];
            let push = [vk::PushConstantRange {
                stage_flags: vk::ShaderStageFlags::VERTEX
                    | vk::ShaderStageFlags::FRAGMENT
                    | vk::ShaderStageFlags::COMPUTE,
                offset: 0,
                size: 128,
            }];
            let layout = self
                .device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&layouts)
                        .push_constant_ranges(&push),
                    None,
                )
                .map_err(vk_error)?;
            let module = self
                .device
                .create_shader_module(
                    &vk::ShaderModuleCreateInfo::default().code(&shader.words),
                    None,
                )
                .map_err(vk_error)?;
            let entry = CString::new(pass.manifest.entry.as_str())
                .map_err(|_| "Shader entry contains NUL")?;
            let stage = vk::PipelineShaderStageCreateInfo::default()
                .stage(if compute {
                    vk::ShaderStageFlags::COMPUTE
                } else {
                    vk::ShaderStageFlags::FRAGMENT
                })
                .module(module)
                .name(&entry);
            let result = if compute {
                self.device
                    .create_compute_pipelines(
                        vk::PipelineCache::null(),
                        &[vk::ComputePipelineCreateInfo::default()
                            .stage(stage)
                            .layout(layout)],
                        None,
                    )
                    .map(|p| p[0])
                    .map_err(|(_, e)| vk_error(e))
            } else {
                let vertex_entry = pass.manifest.vertex_entry.as_deref().unwrap_or("mainVS");
                let vertex = vertex_artifact.as_ref().unwrap();
                let vertex_module = self
                    .device
                    .create_shader_module(
                        &vk::ShaderModuleCreateInfo::default().code(&vertex.words),
                        None,
                    )
                    .map_err(vk_error)?;
                let ve = CString::new(vertex_entry).map_err(|_| "Vertex entry contains NUL")?;
                let stages = [
                    vk::PipelineShaderStageCreateInfo::default()
                        .stage(vk::ShaderStageFlags::VERTEX)
                        .module(vertex_module)
                        .name(&ve),
                    stage,
                ];
                let vertex_bindings = [vk::VertexInputBindingDescription {
                    binding: 0,
                    stride: 72,
                    input_rate: vk::VertexInputRate::VERTEX,
                }];
                let attributes = [
                    vk::VertexInputAttributeDescription {
                        location: 0,
                        binding: 0,
                        format: vk::Format::R32G32B32_SFLOAT,
                        offset: 0,
                    },
                    vk::VertexInputAttributeDescription {
                        location: 1,
                        binding: 0,
                        format: vk::Format::R32G32B32_SFLOAT,
                        offset: 12,
                    },
                    vk::VertexInputAttributeDescription {
                        location: 2,
                        binding: 0,
                        format: vk::Format::R32G32B32A32_SFLOAT,
                        offset: 24,
                    },
                    vk::VertexInputAttributeDescription {
                        location: 3,
                        binding: 0,
                        format: vk::Format::R32G32_SFLOAT,
                        offset: 40,
                    },
                    vk::VertexInputAttributeDescription {
                        location: 4,
                        binding: 0,
                        format: vk::Format::R32G32_SFLOAT,
                        offset: 48,
                    },
                    vk::VertexInputAttributeDescription {
                        location: 5,
                        binding: 0,
                        format: vk::Format::R32G32B32A32_SFLOAT,
                        offset: 56,
                    },
                ];
                let vertex_input = if pass.geometry() {
                    vk::PipelineVertexInputStateCreateInfo::default()
                        .vertex_binding_descriptions(&vertex_bindings)
                        .vertex_attribute_descriptions(&attributes)
                } else {
                    vk::PipelineVertexInputStateCreateInfo::default()
                };
                let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                    .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
                let viewport = vk::PipelineViewportStateCreateInfo::default()
                    .viewport_count(1)
                    .scissor_count(1);
                let raster = vk::PipelineRasterizationStateCreateInfo::default()
                    .polygon_mode(vk::PolygonMode::FILL)
                    .cull_mode(vk::CullModeFlags::NONE)
                    .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                    .line_width(1.);
                let multi = vk::PipelineMultisampleStateCreateInfo::default()
                    .rasterization_samples(vk::SampleCountFlags::TYPE_1);
                let depth = vk::PipelineDepthStencilStateCreateInfo::default()
                    .depth_test_enable(pass.geometry())
                    .depth_write_enable(pass.geometry())
                    .depth_compare_op(vk::CompareOp::LESS);
                let attachments: Vec<_> = pass
                    .manifest
                    .outputs
                    .iter()
                    .map(|_| {
                        vk::PipelineColorBlendAttachmentState::default()
                            .color_write_mask(vk::ColorComponentFlags::RGBA)
                    })
                    .collect();
                let blend =
                    vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
                let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
                let dynamic =
                    vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
                let formats: Vec<_> = pass
                    .manifest
                    .outputs
                    .iter()
                    .map(|s| format(&s.format))
                    .collect::<Result<_, _>>()?;
                let mut rendering = vk::PipelineRenderingCreateInfo::default()
                    .color_attachment_formats(&formats)
                    .depth_attachment_format(if pass.geometry() {
                        vk::Format::D32_SFLOAT
                    } else {
                        vk::Format::UNDEFINED
                    });
                let info = vk::GraphicsPipelineCreateInfo::default()
                    .stages(&stages)
                    .vertex_input_state(&vertex_input)
                    .input_assembly_state(&assembly)
                    .viewport_state(&viewport)
                    .rasterization_state(&raster)
                    .multisample_state(&multi)
                    .depth_stencil_state(&depth)
                    .color_blend_state(&blend)
                    .dynamic_state(&dynamic)
                    .layout(layout)
                    .push_next(&mut rendering);
                let result = self
                    .device
                    .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
                    .map(|p| p[0])
                    .map_err(|(_, e)| vk_error(e));
                self.device.destroy_shader_module(vertex_module, None);
                result
            };
            self.device.destroy_shader_module(module, None);
            match result {
                Ok(pipeline) => Ok(Pipeline {
                    device: self.device.clone(),
                    pipeline,
                    layout,
                    set_layout,
                    compute,
                }),
                Err(error) => {
                    self.device.destroy_pipeline_layout(layout, None);
                    self.device.destroy_descriptor_set_layout(set_layout, None);
                    Err(error)
                }
            }
        }
    }
    #[allow(clippy::too_many_arguments)] // Public integration boundary fixed by the shared renderer contract.
    pub fn render(
        &mut self,
        scene: &SceneSnapshot,
        camera: Camera,
        width: usize,
        height: usize,
        graph: &Graph,
        sources: &BTreeMap<String, String>,
        overrides: &RenderOverrides,
    ) -> Result<Frame, String> {
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || width.checked_mul(height).is_none_or(|p| p > 4_194_304)
        {
            return Err("GPU viewport must be nonzero, <=4096 per axis and <=4M pixels".into());
        }
        self.performance.render_calls += 1;
        self.prepare_for_render(graph, sources)?;
        let plan = self.prepared.as_ref().unwrap().clone();
        let operations = schedule_overrides(&plan, overrides)?;
        let texture_keys = geometry::texture_keys(scene);
        let mut atlas = self.atlas.take().filter(|a| a.keys == texture_keys);
        let upload = atlas.is_none();
        if atlas.is_none() {
            let decoded = geometry::texture_atlas(scene)?;
            let image = self.image(
                decoded.width,
                decoded.height,
                vk::Format::R8G8B8A8_UNORM,
                false,
            )?;
            atlas = Some(AtlasGpu {
                keys: texture_keys,
                atlas: decoded,
                image,
            });
        }
        let mut atlas = atlas.unwrap();
        let (vertex_bytes, submitted) = geometry::vertices(scene, &atlas.atlas.rects);
        let readback = frame_readback(&plan, width, height, self.detailed_stats);
        let resource_key = frame_resource_key(&plan, width, height, &readback);
        let mut reused = self
            .frame_resources
            .take()
            .filter(|r| r.key == resource_key);
        let new_vertex = if let Some(resources) = reused.as_mut() {
            if resources.vertex.size < vertex_bytes.len() as u64 {
                resources.vertex = self.buffer(
                    vertex_bytes.len() as u64 * 2,
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                )?;
            }
            None
        } else {
            Some(self.buffer(
                vertex_bytes.len() as u64 * 2,
                vk::BufferUsageFlags::VERTEX_BUFFER,
            )?)
        };
        let mut images = reused
            .as_mut()
            .map(|r| std::mem::take(&mut r.images))
            .unwrap_or_default();
        let vertex_buffer = new_vertex
            .as_ref()
            .unwrap_or_else(|| &reused.as_ref().unwrap().vertex);
        unsafe {
            let mapped = self
                .device
                .map_memory(
                    vertex_buffer.memory,
                    0,
                    vertex_buffer.size,
                    vk::MemoryMapFlags::empty(),
                )
                .map_err(vk_error)?;
            std::ptr::copy_nonoverlapping(
                vertex_bytes.as_ptr(),
                mapped.cast::<u8>(),
                vertex_bytes.len(),
            );
            self.device.unmap_memory(vertex_buffer.memory);
        }
        if images.is_empty() {
            for pass in &plan.passes {
                for spec in &pass.manifest.outputs {
                    images.insert(
                        pass.resource(&spec.name),
                        self.image(width as u32, height as u32, format(&spec.format)?, false)?,
                    );
                }
                if pass.geometry() {
                    images.insert(
                        format!("{}.__hardware_depth", pass.id),
                        self.image(width as u32, height as u32, vk::Format::D32_SFLOAT, true)?,
                    );
                }
            }
        }
        validate_frame_images(
            &plan,
            &images.keys().map(String::as_str).collect(),
            &readback,
        )?;
        let new_staging = if reused.is_none() {
            Some(self.buffer(
                readback.iter().map(|(_, _, size)| *size as u64).sum(),
                vk::BufferUsageFlags::TRANSFER_DST,
            )?)
        } else {
            None
        };
        let staging = new_staging
            .as_ref()
            .unwrap_or_else(|| &reused.as_ref().unwrap().staging);
        let atlas_upload = if upload {
            Some(self.buffer(
                atlas.atlas.rgba.len() as u64,
                vk::BufferUsageFlags::TRANSFER_SRC,
            )?)
        } else {
            None
        };
        if let Some(buffer) = &atlas_upload {
            unsafe {
                let mapped = self
                    .device
                    .map_memory(buffer.memory, 0, buffer.size, vk::MemoryMapFlags::empty())
                    .map_err(vk_error)?;
                std::ptr::copy_nonoverlapping(
                    atlas.atlas.rgba.as_ptr(),
                    mapped.cast::<u8>(),
                    atlas.atlas.rgba.len(),
                );
                self.device.unmap_memory(buffer.memory);
            }
        }
        unsafe {
            self.device
                .reset_command_pool(self.pool, vk::CommandPoolResetFlags::empty())
                .map_err(vk_error)?;
            self.device
                .reset_descriptor_pool(self.descriptor_pool, vk::DescriptorPoolResetFlags::empty())
                .map_err(vk_error)?;
            self.device
                .begin_command_buffer(
                    self.command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(vk_error)?;
        }
        if let Some(buffer) = &atlas_upload {
            transition(
                &self.device,
                self.command,
                &mut atlas.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::TRANSFER_WRITE,
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width: atlas.atlas.width,
                    height: atlas.atlas.height,
                    depth: 1,
                });
            unsafe {
                self.device.cmd_copy_buffer_to_image(
                    self.command,
                    buffer.buffer,
                    atlas.image.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
        }
        transition(
            &self.device,
            self.command,
            &mut atlas.image,
            vk::ImageLayout::GENERAL,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
        );
        for (index, pass) in plan.passes.iter().enumerate() {
            let pipeline = &self.pipelines[&pipeline_key(pass)];
            let compute = pipeline.compute;
            let stage = if compute {
                vk::PipelineStageFlags::COMPUTE_SHADER
            } else {
                vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::VERTEX_SHADER
            };
            if !pass_enabled(pass, overrides) {
                let from = &pass
                    .inputs
                    .iter()
                    .find(|(s, _)| s.format != "scene")
                    .unwrap()
                    .1;
                let to = pass.resource(&pass.manifest.outputs[0].name);
                copy_image(
                    &self.device,
                    self.command,
                    &mut images,
                    from,
                    &to,
                    width as u32,
                    height as u32,
                )?;
            } else {
                for (_, resource) in pass.inputs.iter().filter(|(s, _)| s.format != "scene") {
                    transition(
                        &self.device,
                        self.command,
                        images.get_mut(resource).unwrap(),
                        vk::ImageLayout::GENERAL,
                        stage,
                        vk::AccessFlags::SHADER_READ,
                    );
                }
                for spec in &pass.manifest.outputs {
                    let image = images.get_mut(&pass.resource(&spec.name)).unwrap();
                    transition(
                        &self.device,
                        self.command,
                        image,
                        if compute {
                            vk::ImageLayout::GENERAL
                        } else {
                            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
                        },
                        if compute {
                            vk::PipelineStageFlags::COMPUTE_SHADER
                        } else {
                            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        },
                        if compute {
                            vk::AccessFlags::SHADER_WRITE
                        } else {
                            vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        },
                    );
                }
                let material_input = pass.manifest.scene_materials;
                let descriptor_images: Vec<_> = pass
                    .inputs
                    .iter()
                    .filter(|(s, _)| s.format != "scene")
                    .map(|(_, name)| name.clone())
                    .chain(if material_input {
                        vec!["__material_atlas".into()]
                    } else {
                        vec![]
                    })
                    .chain(if compute {
                        pass.manifest
                            .outputs
                            .iter()
                            .map(|s| pass.resource(&s.name))
                            .collect::<Vec<_>>()
                    } else {
                        vec![]
                    })
                    .collect();
                let set = unsafe {
                    self.device
                        .allocate_descriptor_sets(
                            &vk::DescriptorSetAllocateInfo::default()
                                .descriptor_pool(self.descriptor_pool)
                                .set_layouts(&[pipeline.set_layout]),
                        )
                        .map_err(vk_error)?[0]
                };
                let image_infos: Vec<_> = descriptor_images
                    .iter()
                    .map(|name| {
                        vk::DescriptorImageInfo::default()
                            .image_view(if name == "__material_atlas" {
                                atlas.image.view
                            } else {
                                images[name].view
                            })
                            .image_layout(vk::ImageLayout::GENERAL)
                    })
                    .collect();
                let input_count = pass
                    .inputs
                    .iter()
                    .filter(|(s, _)| s.format != "scene")
                    .count()
                    + usize::from(material_input);
                let writes: Vec<_> = image_infos
                    .iter()
                    .enumerate()
                    .map(|(i, info)| {
                        vk::WriteDescriptorSet::default()
                            .dst_set(set)
                            .dst_binding(i as u32)
                            .descriptor_type(if i < input_count {
                                vk::DescriptorType::SAMPLED_IMAGE
                            } else {
                                vk::DescriptorType::STORAGE_IMAGE
                            })
                            .image_info(std::slice::from_ref(info))
                    })
                    .collect();
                let mut constants = geometry::camera_constants(camera, width, height);
                for (i, name) in pass.manifest.parameters.iter().enumerate() {
                    let configured = overrides
                        .parameters
                        .get(&pass.id)
                        .and_then(|p| p.get(name))
                        .or_else(|| pass.params.get(name));
                    constants[16 + i] = if name == "mode" {
                        match configured.and_then(|v| v.as_str()) {
                            Some("aces") => 1.,
                            Some("reinhard") => 0.,
                            None => configured.and_then(|v| v.as_f64()).unwrap_or(1.) as f32,
                            Some(other) => {
                                return Err(format!("Unknown tone mapping mode {other}"));
                            }
                        }
                    } else {
                        configured
                            .and_then(|v| v.as_f64())
                            .unwrap_or(if name == "shift" { 0. } else { 1. })
                            as f32
                    };
                }
                let mut push_bytes = Vec::with_capacity(128);
                for value in constants {
                    push_bytes.extend_from_slice(&value.to_ne_bytes());
                }
                let bind_point = if compute {
                    vk::PipelineBindPoint::COMPUTE
                } else {
                    vk::PipelineBindPoint::GRAPHICS
                };
                unsafe {
                    self.device.update_descriptor_sets(&writes, &[]);
                    self.device
                        .cmd_bind_pipeline(self.command, bind_point, pipeline.pipeline);
                    self.device.cmd_bind_descriptor_sets(
                        self.command,
                        bind_point,
                        pipeline.layout,
                        0,
                        &[set],
                        &[],
                    );
                    self.device.cmd_push_constants(
                        self.command,
                        pipeline.layout,
                        vk::ShaderStageFlags::VERTEX
                            | vk::ShaderStageFlags::FRAGMENT
                            | vk::ShaderStageFlags::COMPUTE,
                        0,
                        &push_bytes,
                    );
                    if compute {
                        self.device.cmd_dispatch(
                            self.command,
                            (width as u32).div_ceil(pass.manifest.workgroup[0]),
                            (height as u32).div_ceil(pass.manifest.workgroup[1]),
                            1,
                        );
                    } else {
                        let attachments: Vec<_> = pass
                            .manifest
                            .outputs
                            .iter()
                            .map(|spec| {
                                vk::RenderingAttachmentInfo::default()
                                    .image_view(images[&pass.resource(&spec.name)].view)
                                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                                    .load_op(vk::AttachmentLoadOp::CLEAR)
                                    .store_op(vk::AttachmentStoreOp::STORE)
                                    .clear_value(vk::ClearValue {
                                        color: vk::ClearColorValue {
                                            float32: if spec.semantic == "hdr_linear"
                                                || spec.semantic == "albedo_linear"
                                            {
                                                [0.02, 0.03, 0.055, 0.]
                                            } else {
                                                [0.; 4]
                                            },
                                        },
                                    })
                            })
                            .collect();
                        let mut rendering = vk::RenderingInfo::default()
                            .render_area(vk::Rect2D {
                                offset: vk::Offset2D { x: 0, y: 0 },
                                extent: vk::Extent2D {
                                    width: width as u32,
                                    height: height as u32,
                                },
                            })
                            .layer_count(1)
                            .color_attachments(&attachments);
                        let depth_attachment;
                        if pass.geometry() {
                            let depth = images
                                .get_mut(&format!("{}.__hardware_depth", pass.id))
                                .unwrap();
                            transition(
                                &self.device,
                                self.command,
                                depth,
                                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                                vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                                vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                            );
                            depth_attachment = vk::RenderingAttachmentInfo::default()
                                .image_view(depth.view)
                                .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
                                .load_op(vk::AttachmentLoadOp::CLEAR)
                                .store_op(vk::AttachmentStoreOp::STORE)
                                .clear_value(vk::ClearValue {
                                    depth_stencil: vk::ClearDepthStencilValue {
                                        depth: 1.,
                                        stencil: 0,
                                    },
                                });
                            rendering = rendering.depth_attachment(&depth_attachment);
                        }
                        self.device.cmd_begin_rendering(self.command, &rendering);
                        self.device.cmd_set_viewport(
                            self.command,
                            0,
                            &[vk::Viewport {
                                x: 0.,
                                y: 0.,
                                width: width as f32,
                                height: height as f32,
                                min_depth: 0.,
                                max_depth: 1.,
                            }],
                        );
                        self.device.cmd_set_scissor(
                            self.command,
                            0,
                            &[vk::Rect2D {
                                offset: vk::Offset2D { x: 0, y: 0 },
                                extent: vk::Extent2D {
                                    width: width as u32,
                                    height: height as u32,
                                },
                            }],
                        );
                        if pass.geometry() {
                            self.device.cmd_bind_vertex_buffers(
                                self.command,
                                0,
                                &[vertex_buffer.buffer],
                                &[0],
                            );
                            self.device
                                .cmd_draw(self.command, (submitted * 3) as u32, 1, 0, 0);
                        } else {
                            self.device.cmd_draw(self.command, 3, 1, 0, 0);
                        }
                        self.device.cmd_end_rendering(self.command);
                    }
                }
            }
            if let Some(commands) = operations.get(&index) {
                for operation in commands {
                    match operation {
                        RenderResourceCommand::ClearResource { name, value } => {
                            let image = images.get_mut(name).unwrap();
                            transition(
                                &self.device,
                                self.command,
                                image,
                                vk::ImageLayout::GENERAL,
                                vk::PipelineStageFlags::TRANSFER,
                                vk::AccessFlags::TRANSFER_WRITE,
                            );
                            unsafe {
                                self.device.cmd_clear_color_image(
                                    self.command,
                                    image.image,
                                    vk::ImageLayout::GENERAL,
                                    &vk::ClearColorValue { float32: *value },
                                    &[range(image.aspect)],
                                );
                            }
                        }
                        RenderResourceCommand::CopyResource { from, to } => copy_image(
                            &self.device,
                            self.command,
                            &mut images,
                            from,
                            to,
                            width as u32,
                            height as u32,
                        )?,
                    }
                }
            }
        }
        let mut offset = 0u64;
        for (name, _, size) in &readback {
            let image = images.get_mut(name).unwrap();
            transition(
                &self.device,
                self.command,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::TRANSFER_READ,
            );
            let region = vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: image.aspect,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width: width as u32,
                    height: height as u32,
                    depth: 1,
                });
            unsafe {
                self.device.cmd_copy_image_to_buffer(
                    self.command,
                    image.image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    staging.buffer,
                    &[region],
                );
            }
            offset += *size as u64;
        }
        unsafe {
            // Make transfer writes available and visible to host reads. Waiting
            // for the queue alone is not the graph's memory dependency contract.
            let host_read = vk::BufferMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(staging.buffer)
                .offset(0)
                .size(staging.size);
            self.device.cmd_pipeline_barrier(
                self.command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &[host_read],
                &[],
            );
            self.device
                .end_command_buffer(self.command)
                .map_err(vk_error)?;
            self.device
                .queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&[self.command])],
                    vk::Fence::null(),
                )
                .map_err(vk_error)?;
            self.device.queue_wait_idle(self.queue).map_err(vk_error)?;
        }
        let mut data = vec![0u8; staging.size as usize];
        unsafe {
            let mapped = self
                .device
                .map_memory(staging.memory, 0, staging.size, vk::MemoryMapFlags::empty())
                .map_err(vk_error)?;
            std::ptr::copy_nonoverlapping(mapped.cast::<u8>(), data.as_mut_ptr(), data.len());
            self.device.unmap_memory(staging.memory);
        }
        let mut stats = RenderStats {
            objects: scene.items.len(),
            submitted_triangles: submitted,
            ..Default::default()
        };
        let mut triangles = BTreeSet::new();
        let mut objects = BTreeSet::new();
        let mut offset = 0;
        for (_, kind, size) in &readback {
            let bytes = &data[offset..offset + size];
            match kind {
                1 => {
                    stats.shaded_pixels = bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .filter(|v| f32::from_ne_bytes(**v) > 0.)
                        .count()
                }
                4 => {
                    stats.shaded_pixels = bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .filter(|v| f32::from_ne_bytes(**v) < 1.)
                        .count()
                }
                2 | 3 => {
                    for pixel in bytes.as_chunks::<16>().0 {
                        let id = f32::from_ne_bytes(pixel[12..16].try_into().unwrap()) as u32;
                        if id > 0 {
                            if *kind == 2 {
                                objects.insert(id);
                            } else {
                                triangles.insert(id);
                            }
                        }
                    }
                }
                _ => {}
            }
            offset += size;
        }
        stats.triangles = triangles.len();
        stats.rasterized_triangles = triangles.len();
        stats.visible_objects = objects.len();
        // Reuse the readback allocation as output instead of copying RGBA twice.
        data.truncate(width * height * 4);
        let frame = Frame {
            width,
            height,
            rgba: data,
            stats,
        };
        let resources = if let Some(mut resources) = reused {
            resources.images = images;
            resources
        } else {
            FrameResources {
                key: resource_key,
                images,
                vertex: new_vertex.unwrap(),
                staging: new_staging.unwrap(),
            }
        };
        self.frame_resources = Some(resources);
        self.atlas = Some(atlas);
        Ok(frame)
    }
}
impl Drop for VulkanRenderer {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.pipelines.clear();
            self.frame_resources = None;
            self.atlas = None;
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}
#[cfg(test)]
mod content_player_tests {
    use super::*;
    use crate::{RenderPath, default_graph, precompile_graph_at};
    use module_engine_core::{ObjectsWorld, WorldProvider};
    #[test]
    fn actual_imported_uv_texture_changes_forward_and_deferred_gpu_pixels_and_cache_is_reused() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut content = module_engine_content::ContentLibrary::new(&root).unwrap();
        let mut scene = ObjectsWorld::demo().snapshot();
        scene.items[0].color = [255; 4];
        content
            .enrich_snapshot(
                &mut scene,
                &BTreeMap::from([(1, "assets/models/demo_crystal.glb".into())]),
            )
            .unwrap();
        let mut renderer = VulkanRenderer::new_at(root).unwrap();
        for path in [RenderPath::Forward, RenderPath::Deferred] {
            let graph = default_graph(path);
            let textured = renderer
                .render(
                    &scene,
                    Camera::default(),
                    160,
                    120,
                    &graph,
                    &BTreeMap::new(),
                    &RenderOverrides::default(),
                )
                .unwrap();
            let repeat = renderer
                .render(
                    &scene,
                    Camera::default(),
                    160,
                    120,
                    &graph,
                    &BTreeMap::new(),
                    &RenderOverrides::default(),
                )
                .unwrap();
            assert_eq!(textured.rgba, repeat.rgba);
            assert!(textured.stats.shaded_pixels > 100);
            let mut plain = scene.clone();
            plain.items[0].material.base_color_texture = None;
            let untextured = renderer
                .render(
                    &plain,
                    Camera::default(),
                    160,
                    120,
                    &graph,
                    &BTreeMap::new(),
                    &RenderOverrides::default(),
                )
                .unwrap();
            assert!(
                textured
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(untextured.rgba.as_chunks::<4>().0.iter())
                    .filter(|(a, b)| a != b)
                    .count()
                    > 100,
                "Material UV/image must affect actual GPU output"
            );
        }
    }
    #[test]
    fn sealed_player_renders_from_exported_spirv_without_dxc_and_missing_artifact_fails() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let package = root.join(format!(
            ".work/precompiled-player-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&package).unwrap();
        let graph = default_graph(RenderPath::Forward);
        let sources = BTreeMap::new();
        let scene = ObjectsWorld::demo().snapshot();
        let count = precompile_graph_at(&graph, &sources, &root, &package).unwrap();
        assert!(count > 3);
        assert!(!package.join("tools/dxc/dxc.exe").exists());
        let mut renderer = VulkanRenderer::new_at(package.clone()).unwrap();
        renderer.set_detailed_stats(false);
        let frame = renderer
            .render(
                &scene,
                Camera::default(),
                96,
                72,
                &graph,
                &sources,
                &RenderOverrides::default(),
            )
            .unwrap();
        assert_eq!(frame.rgba.len(), 96 * 72 * 4);
        drop(renderer);
        let artifact = std::fs::read_dir(package.join(".spirv"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::remove_file(artifact).unwrap();
        assert!(
            VulkanRenderer::new_at(package.clone())
                .unwrap()
                .prepare(&graph, &sources)
                .is_err()
        );
        std::fs::remove_dir_all(package).unwrap();
    }
}
fn pipeline_key(pass: &Pass) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    pass.source.hash(&mut h);
    pass.dependency_hash.hash(&mut h);
    serde_json::to_string(&pass.manifest).unwrap().hash(&mut h);
    format!("{:016x}", h.finish())
}
fn vk_error(error: vk::Result) -> String {
    format!("Vulkan: {error:?}")
}
fn format(name: &str) -> Result<vk::Format, String> {
    match name {
        "rgba32float" => Ok(vk::Format::R32G32B32A32_SFLOAT),
        "r32float" => Ok(vk::Format::R32_SFLOAT),
        "rgba8unorm" => Ok(vk::Format::R8G8B8A8_UNORM),
        _ => Err(format!("Unknown GPU format {name}")),
    }
}
fn range(aspect: vk::ImageAspectFlags) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: aspect,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}
fn transition(
    device: &ash::Device,
    command: vk::CommandBuffer,
    image: &mut Image,
    layout: vk::ImageLayout,
    stage: vk::PipelineStageFlags,
    access: vk::AccessFlags,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .src_access_mask(image.access)
        .dst_access_mask(access)
        .old_layout(image.layout)
        .new_layout(layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image.image)
        .subresource_range(range(image.aspect));
    unsafe {
        device.cmd_pipeline_barrier(
            command,
            image.stage,
            stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
    image.layout = layout;
    image.stage = stage;
    image.access = access;
}
fn copy_image(
    device: &ash::Device,
    command: vk::CommandBuffer,
    images: &mut BTreeMap<String, Image>,
    from: &str,
    to: &str,
    width: u32,
    height: u32,
) -> Result<(), String> {
    if from == to {
        return Err("Resource copy cannot alias itself".into());
    }
    let source = images
        .get_mut(from)
        .ok_or_else(|| format!("Unknown GPU resource {from}"))?;
    transition(
        device,
        command,
        source,
        vk::ImageLayout::GENERAL,
        vk::PipelineStageFlags::TRANSFER,
        vk::AccessFlags::TRANSFER_READ,
    );
    let image = source.image;
    let format = source.format;
    let target = images
        .get_mut(to)
        .ok_or_else(|| format!("Unknown GPU resource {to}"))?;
    if target.format != format {
        return Err("Resource copy requires identical image formats".into());
    }
    transition(
        device,
        command,
        target,
        vk::ImageLayout::GENERAL,
        vk::PipelineStageFlags::TRANSFER,
        vk::AccessFlags::TRANSFER_WRITE,
    );
    let layer = vk::ImageSubresourceLayers {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        mip_level: 0,
        base_array_layer: 0,
        layer_count: 1,
    };
    let region = vk::ImageCopy::default()
        .src_subresource(layer)
        .dst_subresource(layer)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        });
    unsafe {
        device.cmd_copy_image(
            command,
            image,
            vk::ImageLayout::GENERAL,
            target.image,
            vk::ImageLayout::GENERAL,
            &[region],
        );
    }
    Ok(())
}
fn schedule_overrides(
    plan: &Plan,
    overrides: &RenderOverrides,
) -> Result<BTreeMap<usize, Vec<RenderResourceCommand>>, String> {
    for (id, params) in &overrides.parameters {
        let pass = plan
            .passes
            .iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| format!("Unknown render pass {id}"))?;
        for (name, value) in params {
            if !pass.manifest.parameters.contains(name)
                || !value.as_f64().is_some_and(|v| {
                    v.is_finite()
                        && v.abs() <= f32::MAX as f64
                        && (name != "mode" || v == 0. || v == 1.)
                })
            {
                return Err(format!(
                    "Pass {id}: unsupported/nonfinite scalar parameter {name}"
                ));
            }
        }
    }
    for id in overrides.enabled.keys() {
        if !plan.passes.iter().any(|pass| &pass.id == id) {
            return Err(format!("Unknown render pass {id}"));
        }
    }
    for pass in &plan.passes {
        let id = &pass.id;
        if !pass_enabled(pass, overrides) {
            let input = pass
                .inputs
                .iter()
                .find(|(s, _)| s.format != "scene")
                .map(|(s, _)| s);
            let output = pass.manifest.outputs.first();
            if pass.manifest.outputs.len() != 1
                || !input
                    .zip(output)
                    .is_some_and(|(a, b)| a.format == b.format && a.semantic == b.semantic)
            {
                return Err(format!(
                    "Pass {id} cannot be disabled: no type-preserving single-image bypass"
                ));
            }
        }
    }
    let mut producers = BTreeMap::new();
    let mut consumers = BTreeMap::<String, usize>::new();
    for (index, pass) in plan.passes.iter().enumerate() {
        for output in &pass.manifest.outputs {
            producers.insert(pass.resource(&output.name), (index, output));
        }
        for (_, input) in &pass.inputs {
            consumers.entry(input.clone()).or_insert(index);
        }
    }
    let mut schedule = BTreeMap::<usize, Vec<RenderResourceCommand>>::new();
    let mut last = 0;
    for command in &overrides.resource_commands {
        let (needed, destination) = match command {
            RenderResourceCommand::ClearResource { name, value } => {
                if !value.iter().all(|v| v.is_finite()) {
                    return Err("Resource clear requires finite values".into());
                }
                (
                    producers
                        .get(name)
                        .ok_or_else(|| format!("Unknown resource {name}"))?
                        .0,
                    name,
                )
            }
            RenderResourceCommand::CopyResource { from, to } => {
                if from == to {
                    return Err("Resource copy must use distinct images".into());
                }
                let a = producers
                    .get(from)
                    .ok_or_else(|| format!("Unknown resource {from}"))?;
                let b = producers
                    .get(to)
                    .ok_or_else(|| format!("Unknown resource {to}"))?;
                if a.1.format != b.1.format || a.1.semantic != b.1.semantic {
                    return Err("Resource copy requires matching format and semantic".into());
                }
                (a.0.max(b.0), to)
            }
        };
        last = last.max(needed);
        if consumers
            .get(destination)
            .is_some_and(|first| *first <= last)
        {
            return Err(
                "Ordered resource operation would happen after its destination was consumed".into(),
            );
        }
        schedule.entry(last).or_default().push(command.clone());
    }
    Ok(schedule)
}

/// Validate portable render controls using only graph resource/parameter metadata.
/// No shader compilation, world mutation, Vulkan work, or source replacement occurs.
pub fn validate_overrides(graph: &Graph, overrides: &RenderOverrides) -> Result<(), String> {
    RenderGraphValidator::new(graph)?.validate(overrides)
}
pub fn validate_overrides_at(
    graph: &Graph,
    overrides: &RenderOverrides,
    _resource_root: &std::path::Path,
) -> Result<(), String> {
    RenderGraphValidator::new(graph)?.validate(overrides)
}

/// Reuse one metadata plan to validate current settings, every advertised
/// preset, and live Rust controls. No sources, includes or GPU are consulted.
pub struct RenderGraphValidator {
    plan: Plan,
}
impl RenderGraphValidator {
    pub fn new(graph: &Graph) -> Result<Self, String> {
        Ok(Self {
            plan: build_metadata_plan(graph)?,
        })
    }
    pub fn validate(&self, overrides: &RenderOverrides) -> Result<(), String> {
        schedule_overrides(&self.plan, overrides).map(|_| ())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RenderPath, custom_compute_manifest, default_graph, demo_custom_compute_source};
    use module_engine_core::{ObjectsWorld, WorldProvider, make_edge};
    #[test]
    fn failed_partial_native_allocations_release_owned_handles_and_device_recovers() {
        let mut renderer = VulkanRenderer::new().unwrap();
        let counts = renderer.allocations.clone();
        let initial = counts.snapshot();
        for fault in [
            AllocationFault::BufferHandle,
            AllocationFault::BufferMemory,
            AllocationFault::ImageHandle,
            AllocationFault::ImageMemory,
            AllocationFault::ImageView,
        ] {
            renderer.allocation_fault = Some(fault);
            let error = match fault {
                AllocationFault::BufferHandle | AllocationFault::BufferMemory => renderer
                    .buffer(4096, vk::BufferUsageFlags::VERTEX_BUFFER)
                    .err(),
                _ => renderer
                    .image(32, 32, vk::Format::R8G8B8A8_UNORM, false)
                    .err(),
            }
            .expect("injected allocation point must reject the partial object");
            assert!(
                error.contains("Injected Vulkan allocation failure"),
                "{error}"
            );
            assert_eq!(
                counts.snapshot(),
                initial,
                "no handle or device-memory allocation may survive {fault:?}"
            );
        }
        renderer.allocation_fault = None;
        let frame = renderer
            .render(
                &ObjectsWorld::demo().snapshot(),
                Camera::default(),
                96,
                64,
                &default_graph(RenderPath::Forward),
                &BTreeMap::new(),
                &RenderOverrides::default(),
            )
            .unwrap();
        assert_eq!(frame.rgba.len(), 96 * 64 * 4);
        assert!(counts.snapshot().into_iter().all(|count| count > 0));
        drop(renderer);
        assert_eq!(
            counts.snapshot(),
            [0; 4],
            "successful retained resources also release exactly once"
        );
    }
    fn rename_pass(graph: &mut Graph, previous: &str, next: &str) {
        graph
            .nodes
            .iter_mut()
            .find(|node| node.id == previous)
            .unwrap()
            .id = next.into();
        for edge in &mut graph.edges {
            if edge.from_node == previous {
                edge.from_node = next.into();
            }
            if edge.to_node == previous {
                edge.to_node = next.into();
            }
        }
    }
    #[test]
    fn frame_allocation_identity_includes_resource_ids_readback_and_viewport() {
        let graph = default_graph(RenderPath::Forward);
        let plan = build_metadata_plan(&graph).unwrap();
        let readback = frame_readback(&plan, 96, 64, true);
        let key = frame_resource_key(&plan, 96, 64, &readback);
        let mut renamed = graph.clone();
        rename_pass(&mut renamed, "ao", "gpu_ssao_added");
        let renamed_plan = build_metadata_plan(&renamed).unwrap();
        assert_eq!(
            plan.passes.iter().map(pipeline_key).collect::<Vec<_>>(),
            renamed_plan
                .passes
                .iter()
                .map(pipeline_key)
                .collect::<Vec<_>>(),
            "this was the insufficient old resource cache key"
        );
        let renamed_readback = frame_readback(&renamed_plan, 96, 64, true);
        assert_ne!(
            key,
            frame_resource_key(&renamed_plan, 96, 64, &renamed_readback)
        );
        let available = key
            .images
            .iter()
            .map(|(name, _, _)| name.as_str())
            .collect();
        let error =
            validate_frame_images(&renamed_plan, &available, &renamed_readback).unwrap_err();
        assert!(
            error.contains("gpu_ssao_added.color") && error.contains("allocated image"),
            "{error}"
        );
        assert_ne!(
            key,
            frame_resource_key(&plan, 96, 64, &frame_readback(&plan, 96, 64, false))
        );
        assert_ne!(
            key,
            frame_resource_key(&plan, 128, 72, &frame_readback(&plan, 128, 72, true))
        );
        let mut same_allocations = plan.clone();
        same_allocations
            .passes
            .iter_mut()
            .find(|pass| pass.id == "tonemap")
            .unwrap()
            .source
            .push_str("\n// shader-only edit\n");
        assert_eq!(
            key,
            frame_resource_key(&same_allocations, 96, 64, &readback),
            "a new shader must not force new image allocations"
        );
    }
    #[test]
    fn gpu_resource_cache_renames_ssao_geometry_and_output_without_stale_images() {
        let scene = ObjectsWorld::demo().snapshot();
        let camera = Camera::default();
        for path in [RenderPath::Forward, RenderPath::Deferred] {
            let mut renderer = VulkanRenderer::new().unwrap();
            let mut graph = default_graph(path);
            let sources = BTreeMap::new();
            let controls = RenderOverrides::default();
            let original = renderer
                .render(&scene, camera, 96, 64, &graph, &sources, &controls)
                .unwrap();
            let pipeline_count = renderer.performance.pipeline_creations;
            for (previous, next) in [
                ("ao", "gpu_ssao_added"),
                ("geometry", "geometry_added"),
                ("encode", "encode_added"),
            ] {
                rename_pass(&mut graph, previous, next);
                let frame = renderer
                    .render(&scene, camera, 96, 64, &graph, &sources, &controls)
                    .unwrap();
                assert_eq!(
                    frame.rgba, original.rgba,
                    "resource rename must preserve actual GPU pixels"
                );
                assert_eq!(
                    renderer.performance.pipeline_creations, pipeline_count,
                    "resource IDs change while pipeline identities remain reusable"
                );
                let resources = renderer.frame_resources.as_ref().unwrap();
                assert!(
                    resources
                        .images
                        .keys()
                        .any(|resource| resource.starts_with(&format!("{next}.")))
                );
                assert!(
                    !resources
                        .images
                        .keys()
                        .any(|resource| resource.starts_with(&format!("{previous}.")))
                );
                assert_eq!(
                    resources.staging.size,
                    resources
                        .key
                        .readback
                        .iter()
                        .map(|(_, _, bytes)| *bytes as u64)
                        .sum::<u64>()
                );
            }
            renderer.set_detailed_stats(false);
            let resized = renderer
                .render(&scene, camera, 128, 72, &graph, &sources, &controls)
                .unwrap();
            assert_eq!(resized.rgba.len(), 128 * 72 * 4);
            assert_eq!(
                renderer.frame_resources.as_ref().unwrap().staging.size,
                128 * 72 * 4
            );
        }
    }
    #[test]
    fn metadata_validator_reuses_declared_contracts_without_any_shader_root() {
        let absent = std::env::temp_dir().join("module-engine-no-root-for-render-controls");
        let (graph, _) = custom_graph();
        let validator = RenderGraphValidator::new(&graph).unwrap();
        for _ in 0..120 {
            validator.validate(&RenderOverrides::default()).unwrap();
            validate_overrides_at(&graph, &RenderOverrides::default(), &absent).unwrap();
        }
        let mut controls = RenderOverrides::default();
        controls.enabled.insert("missing".into(), false);
        assert!(
            validator
                .validate(&controls)
                .unwrap_err()
                .contains("Unknown render pass")
        );
    }
    #[test]
    fn prepared_plan_reuse_skips_layout_rebuilds_but_keeps_real_edits_and_explicit_freshness() {
        let mut renderer = VulkanRenderer::new().unwrap();
        let mut graph = default_graph(RenderPath::Forward);
        let mut sources = BTreeMap::new();
        renderer.prepare(&graph, &sources).unwrap();
        let initial = renderer.performance();
        for index in 0..120 {
            graph.nodes[0].position[0] = index as f32;
            graph.nodes[0].label = format!("Layout {index}");
            renderer.prepared_inputs.as_mut().unwrap().2 = Instant::now();
            renderer.prepare_for_render(&graph, &sources).unwrap();
        }
        let stable = renderer.performance();
        assert_eq!(stable.plan_builds, initial.plan_builds);
        assert_eq!(stable.plan_cache_hits - initial.plan_cache_hits, 120);
        assert_eq!(stable.pipeline_creations, initial.pipeline_creations);
        graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "tonemap")
            .unwrap()
            .params["exposure"] = serde_json::json!(1.25);
        renderer.prepare_for_render(&graph, &sources).unwrap();
        assert_eq!(renderer.performance().plan_builds, stable.plan_builds + 1);
        sources.insert("new-inline-source".into(), "float gain = 1.;".into());
        renderer.prepare_for_render(&graph, &sources).unwrap();
        assert_eq!(renderer.performance().plan_builds, stable.plan_builds + 2);
        renderer.prepared_inputs.as_mut().unwrap().2 = Instant::now() - INCLUDE_REFRESH_INTERVAL;
        renderer.prepare_for_render(&graph, &sources).unwrap();
        assert_eq!(renderer.performance().plan_builds, stable.plan_builds + 3);
        renderer.prepare(&graph, &sources).unwrap();
        assert_eq!(renderer.performance().plan_builds, stable.plan_builds + 4);
    }
    fn custom_graph() -> (Graph, BTreeMap<String, String>) {
        let mut graph = default_graph(RenderPath::Forward);
        let node = graph.nodes.iter_mut().find(|n| n.id == "ao").unwrap();
        node.module = "gpu.custom".into();
        node.params = serde_json::json!({"source_id":"custom","manifest":custom_compute_manifest(),"gain":0.5});
        graph
            .edges
            .retain(|e| e.to_node != "ao" || e.to_port == "color");
        (
            graph,
            BTreeMap::from([("custom".into(), demo_custom_compute_source())]),
        )
    }
    #[test]
    fn metadata_rejects_cycles_semantics_formats_and_missing_resources() {
        let mut graph = default_graph(RenderPath::Forward);
        graph.edges.retain(|e| e.to_node != "tonemap");
        graph
            .edges
            .push(make_edge("aa", "color", "tonemap", "color"));
        assert!(
            build_plan(&graph, &BTreeMap::new())
                .unwrap_err()
                .contains("cycle")
        );
        let (mut graph, sources) = custom_graph();
        graph
            .nodes
            .iter_mut()
            .find(|n| n.id == "ao")
            .unwrap()
            .params["manifest"]["outputs"][0]["semantic"] = serde_json::json!("ldr_linear");
        assert!(
            build_plan(&graph, &sources)
                .unwrap_err()
                .contains("expects")
        );
        let mut overrides = RenderOverrides::default();
        overrides.parameters.insert(
            "unknown".into(),
            BTreeMap::from([("exposure".into(), serde_json::json!(1.))]),
        );
        assert!(validate_overrides(&default_graph(RenderPath::Forward), &overrides).is_err());
        overrides = RenderOverrides::default();
        overrides.enabled.insert("tonemap".into(), false);
        assert!(
            validate_overrides(&default_graph(RenderPath::Forward), &overrides)
                .unwrap_err()
                .contains("type-preserving")
        );
        overrides = RenderOverrides::default();
        overrides
            .resource_commands
            .push(RenderResourceCommand::CopyResource {
                from: "geometry.normals".into(),
                to: "ao.color".into(),
            });
        assert!(
            validate_overrides(&default_graph(RenderPath::Forward), &overrides)
                .unwrap_err()
                .contains("semantic")
        );
    }
    #[test]
    fn compiled_hlsl_must_match_manifest_bindings_workgroup_and_storage_format() {
        let (graph, sources) = custom_graph();
        let plan = build_plan(&graph, &sources).unwrap();
        let pass = plan.passes.iter().find(|p| p.id == "ao").unwrap();
        for (source, expected) in [
            (
                pass.source
                    .replace("numthreads(8,8,1)", "numthreads(4,4,1)"),
                "workgroup",
            ),
            (
                pass.source.replace("binding(1,0)", "binding(9,0)"),
                "outside",
            ),
            (
                pass.source
                    .replace("image_format(\"rgba32f\")", "image_format(\"rgba8\")"),
                "format decoration",
            ),
        ] {
            let shader = compile_hlsl(&source, "main", "cs_6_0").unwrap();
            assert!(
                verify_shader(pass, &shader.words, false)
                    .unwrap_err()
                    .contains(expected)
            );
        }
    }
    #[test]
    fn actual_gpu_bypass_and_copy_agree_and_invalid_candidate_retains_prepared_graph() {
        let mut renderer = VulkanRenderer::new().expect("GPU regression requires Vulkan1.3 driver");
        let scene = ObjectsWorld::demo().snapshot();
        let graph = default_graph(RenderPath::Forward);
        let sources = BTreeMap::new();
        let base = renderer
            .render(
                &scene,
                Camera::default(),
                160,
                120,
                &graph,
                &sources,
                &RenderOverrides::default(),
            )
            .unwrap();
        assert!(base.stats.shaded_pixels > 500);
        let mut bypass = RenderOverrides::default();
        bypass.enabled.insert("ao".into(), false);
        let no_ao = renderer
            .render(
                &scene,
                Camera::default(),
                160,
                120,
                &graph,
                &sources,
                &bypass,
            )
            .unwrap();
        assert_ne!(
            base.rgba, no_ao.rgba,
            "SSAO executes on GPU and must affect actual pixels"
        );
        let mut copying = RenderOverrides::default();
        copying
            .resource_commands
            .push(RenderResourceCommand::CopyResource {
                from: "geometry.color".into(),
                to: "ao.color".into(),
            });
        let copied = renderer
            .render(
                &scene,
                Camera::default(),
                160,
                120,
                &graph,
                &sources,
                &copying,
            )
            .unwrap();
        assert_eq!(
            copied.rgba, no_ao.rgba,
            "vkCmdCopyImage must match explicitly bypassing AO"
        );
        let (custom_graph, mut custom_sources) = custom_graph();
        renderer.prepare(&custom_graph, &custom_sources).unwrap();
        let previous = renderer.prepared.clone().unwrap();
        custom_sources.insert("custom".into(), "not HLSL".into());
        assert!(renderer.prepare(&custom_graph, &custom_sources).is_err());
        assert_eq!(renderer.prepared.as_ref().unwrap().output, previous.output);
        assert_eq!(
            renderer
                .prepared
                .as_ref()
                .unwrap()
                .passes
                .last()
                .unwrap()
                .id,
            previous.passes.last().unwrap().id
        );
    }
}

#[cfg(test)]
mod fullscreen_tests {
    use super::*;
    use crate::{
        RenderPath, custom_compute_manifest, custom_fullscreen_manifest, default_graph,
        demo_custom_compute_source, demo_custom_fullscreen_source,
    };
    use module_engine_core::{ObjectsWorld, WorldProvider};
    #[test]
    fn independent_fullscreen_raster_and_compute_execute_equivalent_shader_math() {
        let mut renderer = VulkanRenderer::new().unwrap();
        let scene = ObjectsWorld::demo().snapshot();
        let mut graph = default_graph(RenderPath::Forward);
        let node = graph.nodes.iter_mut().find(|n| n.id == "ao").unwrap();
        node.module = "gpu.custom".into();
        node.params = serde_json::json!({"source_id":"custom","manifest":custom_compute_manifest(),"gain":0.4});
        graph
            .edges
            .retain(|e| e.to_node != "ao" || e.to_port == "color");
        let mut sources = BTreeMap::from([("custom".into(), demo_custom_compute_source())]);
        let compute = renderer
            .render(
                &scene,
                Camera::default(),
                128,
                96,
                &graph,
                &sources,
                &RenderOverrides::default(),
            )
            .unwrap();
        graph
            .nodes
            .iter_mut()
            .find(|n| n.id == "ao")
            .unwrap()
            .params["manifest"] = serde_json::to_value(custom_fullscreen_manifest()).unwrap();
        sources.insert("custom".into(), demo_custom_fullscreen_source());
        let raster = renderer
            .render(
                &scene,
                Camera::default(),
                128,
                96,
                &graph,
                &sources,
                &RenderOverrides::default(),
            )
            .unwrap();
        assert_eq!(compute.rgba, raster.rgba);
    }
}
#[cfg(test)]
mod conversion_tests {
    use super::*;
    use crate::{RenderPath, builtin_hlsl_source, builtin_manifest, default_graph};
    use module_engine_core::{ObjectsWorld, WorldProvider};
    #[test]
    fn builtins_including_fxaa_and_color_encode_become_editable_custom_without_pixel_changes() {
        let mut renderer = VulkanRenderer::new().unwrap();
        let scene = ObjectsWorld::demo().snapshot();
        for path in [RenderPath::Forward, RenderPath::Deferred] {
            let mut graph = default_graph(path);
            let mut sources = BTreeMap::new();
            let before = renderer
                .render(
                    &scene,
                    Camera::default(),
                    128,
                    96,
                    &graph,
                    &sources,
                    &RenderOverrides::default(),
                )
                .unwrap();
            for node in graph
                .nodes
                .iter_mut()
                .filter(|n| n.module.starts_with("gpu."))
            {
                let mut manifest = builtin_manifest(&node.module).unwrap();
                if manifest.stage == "raster" {
                    manifest.vertex_entry = Some("mainVS".into());
                }
                let key = format!("editable_{}", node.id);
                sources.insert(
                    key.clone(),
                    builtin_hlsl_source(&node.module).unwrap().into(),
                );
                node.module = "gpu.custom".into();
                node.params["source_id"] = serde_json::json!(key);
                node.params["manifest"] = serde_json::to_value(manifest).unwrap();
            }
            let after = renderer
                .render(
                    &scene,
                    Camera::default(),
                    128,
                    96,
                    &graph,
                    &sources,
                    &RenderOverrides::default(),
                )
                .unwrap();
            assert_eq!(
                before.rgba, after.rgba,
                "Converting shaders to editable HLSL must preserve the complete pass graph behavior"
            );
        }
    }
}

fn pass_enabled(pass: &Pass, overrides: &RenderOverrides) -> bool {
    overrides
        .enabled
        .get(&pass.id)
        .copied()
        .or_else(|| pass.params.get("enabled").and_then(|v| v.as_bool()))
        .unwrap_or(true)
}
#[cfg(test)]
mod authored_configuration_tests {
    use super::*;
    use crate::{RenderPath, custom_compute_manifest, default_graph};
    use module_engine_core::{ObjectsWorld, WorldProvider};
    #[test]
    fn authored_config_typos_are_rejected_before_execution_and_disable_has_explicit_precedence() {
        let mut graph = default_graph(RenderPath::Forward);
        graph
            .nodes
            .iter_mut()
            .find(|n| n.id == "tonemap")
            .unwrap()
            .params["exposue"] = serde_json::json!(2.0);
        assert!(
            validate_overrides(&graph, &RenderOverrides::default())
                .unwrap_err()
                .contains("unknown parameter exposue")
        );
        let mut value = serde_json::to_value(custom_compute_manifest()).unwrap();
        value["entri"] = serde_json::json!("main");
        assert!(
            serde_json::from_value::<crate::PassManifest>(value)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
        let mut value = serde_json::to_value(custom_compute_manifest()).unwrap();
        value["inputs"][0]["samplez"] = serde_json::json!(4);
        assert!(
            serde_json::from_value::<crate::PassManifest>(value)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );
        graph = default_graph(RenderPath::Forward);
        graph
            .nodes
            .iter_mut()
            .find(|n| n.id == "tonemap")
            .unwrap()
            .params["enabled"] = serde_json::json!(false);
        assert!(
            validate_overrides(&graph, &RenderOverrides::default())
                .unwrap_err()
                .contains("cannot be disabled")
        );
        graph = default_graph(RenderPath::Forward);
        graph
            .nodes
            .iter_mut()
            .find(|n| n.id == "ao")
            .unwrap()
            .params["enabled"] = serde_json::json!(false);
        validate_overrides(&graph, &RenderOverrides::default()).unwrap();
        let mut renderer = VulkanRenderer::new().unwrap();
        let scene = ObjectsWorld::demo().snapshot();
        let sources = BTreeMap::new();
        let authored = renderer
            .render(
                &scene,
                Camera::default(),
                128,
                96,
                &graph,
                &sources,
                &RenderOverrides::default(),
            )
            .unwrap();
        let mut explicit = RenderOverrides::default();
        explicit.enabled.insert("ao".into(), false);
        let baseline_graph = default_graph(RenderPath::Forward);
        let controlled = renderer
            .render(
                &scene,
                Camera::default(),
                128,
                96,
                &baseline_graph,
                &sources,
                &explicit,
            )
            .unwrap();
        assert_eq!(
            authored.rgba, controlled.rgba,
            "Authored enabled:false must execute the same native bypass as a portable command"
        );
        explicit.enabled.insert("ao".into(), true);
        let restored = renderer
            .render(
                &scene,
                Camera::default(),
                128,
                96,
                &graph,
                &sources,
                &explicit,
            )
            .unwrap();
        let baseline = renderer
            .render(
                &scene,
                Camera::default(),
                128,
                96,
                &baseline_graph,
                &sources,
                &RenderOverrides::default(),
            )
            .unwrap();
        assert_eq!(
            restored.rgba, baseline.rgba,
            "Explicit command override takes precedence over authored disabled state"
        );
        assert_ne!(baseline.rgba, authored.rgba);
    }
}

#[cfg(test)]
mod include_regression {
    use super::*;
    use crate::{
        RenderPath, compile_hlsl, custom_compute_manifest, default_graph,
        demo_custom_compute_source,
    };
    use module_engine_core::{ObjectsWorld, WorldProvider};
    #[test]
    fn actual_nested_include_mutation_recompiles_unchanged_inline_shader_and_gpu_pipeline() {
        let mut renderer =
            VulkanRenderer::new().expect("Real Vulkan adapter required for GPU regression");
        let name = format!("include-regression-{}", std::process::id());
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory = workspace.join(".work").join(&name);
        std::fs::create_dir_all(directory.join("child")).unwrap();
        std::fs::write(
            directory.join("main.hlsli"),
            "#ifndef TEST_MAIN\n#define TEST_MAIN\n#include \"child/gain.hlsli\"\n#endif\n",
        )
        .unwrap();
        let child = directory.join("child/gain.hlsli");
        let child_source = "#ifndef TEST_GAIN\n#define TEST_GAIN\n#include \"../main.hlsli\"\nfloat header_gain() {return 0.2;}\n#endif\n";
        std::fs::write(&child, child_source).unwrap();
        let source = format!(
            "#include \".work/{name}/main.hlsli\"\n{}",
            demo_custom_compute_source()
                .replace("c.rgb*values[0].x", "c.rgb*values[0].x*header_gain()")
        );
        let mut graph = default_graph(RenderPath::Forward);
        let node = graph.nodes.iter_mut().find(|n| n.id == "ao").unwrap();
        node.module = "gpu.custom".into();
        node.params = serde_json::json!({"source_id":"nested","manifest":custom_compute_manifest(),"gain":1.0});
        graph
            .edges
            .retain(|e| e.to_node != "ao" || e.to_port == "color");
        let sources = BTreeMap::from([("nested".into(), source.clone())]);
        let scene = ObjectsWorld::demo().snapshot();
        let first_spirv = compile_hlsl(&source, "main", "cs_6_0").unwrap().words;
        let first = renderer
            .render(
                &scene,
                Camera::default(),
                160,
                120,
                &graph,
                &sources,
                &RenderOverrides::default(),
            )
            .unwrap();
        let pipeline_count = renderer.pipelines.len();
        let key = pipeline_key(
            renderer
                .prepared
                .as_ref()
                .unwrap()
                .passes
                .iter()
                .find(|p| p.id == "ao")
                .unwrap(),
        );
        renderer.prepare(&graph, &sources).unwrap();
        assert_eq!(
            renderer.pipelines.len(),
            pipeline_count,
            "Unchanged dependencies must reuse GPU pipelines"
        );
        std::fs::write(&child, child_source.replace("return 0.2", "return 0.8")).unwrap();
        let second_spirv = compile_hlsl(&source, "main", "cs_6_0").unwrap().words;
        assert_ne!(
            first_spirv, second_spirv,
            "DXC must consume changed nested include with identical inline text"
        );
        renderer.prepare(&graph, &sources).unwrap();
        let changed_key = pipeline_key(
            renderer
                .prepared
                .as_ref()
                .unwrap()
                .passes
                .iter()
                .find(|p| p.id == "ao")
                .unwrap(),
        );
        assert_ne!(key, changed_key);
        assert_eq!(
            renderer.pipelines.len(),
            pipeline_count + 1,
            "Changed include needs a new compiled pipeline"
        );
        let second = renderer
            .render(
                &scene,
                Camera::default(),
                160,
                120,
                &graph,
                &sources,
                &RenderOverrides::default(),
            )
            .unwrap();
        let pixels = first
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(second.rgba.as_chunks::<4>().0.iter())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            pixels > 100,
            "Changed nested include must change actual GPU pixels"
        );
        assert_eq!(
            sources["nested"], source,
            "Inline authored source remains byte-identical"
        );
        std::fs::write(&child, "this is invalid included HLSL\n").unwrap();
        assert!(
            renderer
                .prepare(&graph, &sources)
                .unwrap_err()
                .contains("HLSL compilation failed")
        );
        let rejected = renderer.performance();
        for _ in 0..120 {
            renderer.rejected_inputs.as_mut().unwrap().checked = Instant::now();
            assert!(
                renderer
                    .render(
                        &scene,
                        Camera::default(),
                        160,
                        120,
                        &graph,
                        &sources,
                        &RenderOverrides::default()
                    )
                    .unwrap_err()
                    .contains("HLSL compilation failed")
            );
        }
        assert_eq!(
            renderer.performance().prepare_calls,
            rejected.prepare_calls,
            "failing hot reload must not compile on every UI frame"
        );
        assert_eq!(
            renderer.performance().pipeline_build_attempts,
            rejected.pipeline_build_attempts
        );
        renderer.rejected_inputs.as_mut().unwrap().checked =
            Instant::now() - INCLUDE_REFRESH_INTERVAL;
        assert!(
            renderer
                .render(
                    &scene,
                    Camera::default(),
                    160,
                    120,
                    &graph,
                    &sources,
                    &RenderOverrides::default()
                )
                .is_err()
        );
        assert_eq!(
            renderer.performance().pipeline_build_attempts,
            rejected.pipeline_build_attempts,
            "unchanged failed include bytes must not invoke DXC again after polling"
        );
        assert_eq!(
            pipeline_key(
                renderer
                    .prepared
                    .as_ref()
                    .unwrap()
                    .passes
                    .iter()
                    .find(|pass| pass.id == "ao")
                    .unwrap()
            ),
            changed_key,
            "last good GPU plan survives every rejected reload"
        );
        std::fs::write(&child, child_source.replace("return 0.2", "return 0.8")).unwrap();
        renderer.rejected_inputs.as_mut().unwrap().checked =
            Instant::now() - INCLUDE_REFRESH_INTERVAL;
        assert_eq!(
            renderer
                .render(
                    &scene,
                    Camera::default(),
                    160,
                    120,
                    &graph,
                    &sources,
                    &RenderOverrides::default()
                )
                .unwrap()
                .rgba,
            second.rgba,
            "Failed include candidate must retain valid pipeline behavior"
        );
        assert!(renderer.rejected_inputs.is_none());
        assert!(renderer.diagnostics().is_empty());
        std::fs::write(&child, child_source.replace("return 0.2", "return 0.6")).unwrap();
        renderer.prepared_inputs.as_mut().unwrap().2 = Instant::now() - INCLUDE_REFRESH_INTERVAL;
        let third = renderer
            .render(
                &scene,
                Camera::default(),
                160,
                120,
                &graph,
                &sources,
                &RenderOverrides::default(),
            )
            .unwrap();
        assert_ne!(
            third.rgba, second.rgba,
            "disk-only repair/change must reach real pixels without an inline edit"
        );
        assert!(renderer.performance().pipeline_build_attempts > rejected.pipeline_build_attempts);
    }
}
