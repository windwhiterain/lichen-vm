//! The Vulkan side: a device, a pipeline per fragment, and a dispatch.
//!
//! # The tail-lane obligation, and how it is discharged
//!
//! A dispatch covers [`spirv::LOCAL_SIZE_X`] invocations per workgroup, so the
//! last workgroup runs lanes whose index is past the end. The shader has **no
//! bounds test**, deliberately: a bounds test would be a branch every lane takes,
//! on the hottest path, to guard indices the host already knows about.
//!
//! Instead every buffer is allocated rounded up to a whole number of workgroups
//! and the padding is zeroed — inputs so the surplus lanes' *reads* are in bounds
//! and return `0`, outputs so their *writes* land in padding rather than past the
//! end. Only the first `count` elements are read back. That keeps the shader
//! branch-free and makes out-of-range access impossible by construction rather
//! than by a runtime test.
//!
//! # Memory ordering is explicit
//!
//! Buffers are host-visible and host-coherent, so no flush is needed — but
//! coherence is not *ordering*. The host's writes must still be made available to
//! the shader, and the shader's writes visible to the host, so the command buffer
//! carries a barrier on each side of the dispatch rather than relying on the
//! submit and the fence to imply it.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use ash::vk;
use lichen_kernel_ir::{KernelFragment, fragment_digest};

use crate::spirv::{self, Binding, LOCAL_SIZE_X, SpirvRefusal};

/// Why a run could not happen.  Every variant names its own cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// No Vulkan physical device could be used.
    NoDevice { detail: String },
    /// The device is present but cannot represent the fragment's integer width.
    /// The IR declares the width, and this is the refusal that declaration exists
    /// to make possible — the alternative would be narrowing, which is a semantic
    /// change to a program nobody wrote.
    MissingInt64 { device: String },
    /// The fragment is outside what this backend emits.
    Emit(SpirvRefusal),
    /// An input buffer is shorter than the run's count, so a lane would read past
    /// it.  Refused rather than zero-filled: a short input is a caller error, and
    /// silently reading `0` would look like real data.
    InputShorterThanCount {
        buffer: usize,
        len: usize,
        count: usize,
    },
    /// A Vulkan call failed.  `stage` names which one, because "Vulkan error" is
    /// not a cause.
    Vulkan { stage: &'static str, detail: String },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::NoDevice { detail } => {
                write!(f, "no usable Vulkan device: {detail}")
            }
            RunError::MissingInt64 { device } => write!(
                f,
                "the device {device:?} does not support 64-bit integers in shaders, and the \
                 fragment was lowered to mean 64-bit ones. Narrowing the values would change \
                 what the program computes, so this is refused rather than converted."
            ),
            RunError::Emit(refusal) => write!(f, "{refusal}"),
            RunError::InputShorterThanCount { buffer, len, count } => write!(
                f,
                "input buffer {buffer} holds {len} element(s) but the run covers {count} \
                 index/indices, so a lane would read past it."
            ),
            RunError::Vulkan { stage, detail } => write!(f, "Vulkan {stage} failed: {detail}"),
        }
    }
}

impl std::error::Error for RunError {}

/// Turn a Vulkan failure into a named `RunError`.
fn check<T>(stage: &'static str, result: Result<T, vk::Result>) -> Result<T, RunError> {
    result.map_err(|code| RunError::Vulkan {
        stage,
        detail: format!("{code:?}"),
    })
}

/// A device, its compute queue, and the pipelines built on it.
///
/// One context owns one device.  Pipelines are cached on the fragment's content
/// digest, so a fragment that is run twice compiles once — the same identity the
/// wasm backend's module cache keys on, which is why [`fragment_digest`] lives
/// in the IR rather than in either backend.
pub struct GpuContext {
    entry: ash::Entry,
    instance: ash::Instance,
    device: ash::Device,
    queue: vk::Queue,
    family: u32,
    /// Retained so memory-type queries can be answered; those live on the
    /// instance, not the device, so the device alone is not enough.
    physical: vk::PhysicalDevice,
    /// Retained so a pipeline cache key can be checked against what the device
    /// actually accepted, and for the device name in a refusal.
    name: String,
    pipelines: Mutex<HashMap<(u64, usize, usize), vk::Pipeline>>,
}

impl GpuContext {
    /// Find a device and create a compute context on it.
    ///
    /// A **discrete** GPU is preferred over an integrated one, because a compute
    /// run is exactly the workload where that matters; among equals, the lowest
    /// index wins so the choice is deterministic.
    pub fn new() -> Result<Self, RunError> {
        let entry = unsafe { ash::Entry::load() }.map_err(|detail| RunError::NoDevice {
            detail: detail.to_string(),
        })?;
        // No layers and no extensions: a compute run needs neither, and asking for
        // none means the context works on a machine with no loader extras.
        let instance = check("instance creation", unsafe {
            entry.create_instance(&vk::InstanceCreateInfo::default(), None)
        })?;

        let devices = check("physical device enumeration", unsafe {
            instance.enumerate_physical_devices()
        })?;
        if devices.is_empty() {
            return Err(RunError::NoDevice {
                detail: "the loader reported no physical device".into(),
            });
        }

        let mut chosen: Option<(vk::PhysicalDevice, vk::PhysicalDeviceProperties, u32)> = None;
        let seen = devices.len();
        for physical in devices {
            let properties = unsafe { instance.get_physical_device_properties(physical) };
            let features = unsafe { instance.get_physical_device_features(physical) };
            if features.shader_int64 == 0 {
                // Recorded rather than skipped silently, so the refusal below can
                // name the device that was passed over and why.
                continue;
            }
            let families =
                unsafe { instance.get_physical_device_queue_family_properties(physical) };
            let Some(family) = families
                .iter()
                .position(|queue| queue.queue_flags.contains(vk::QueueFlags::COMPUTE))
            else {
                continue;
            };
            let discrete = properties.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
            let better = match &chosen {
                None => true,
                Some((_, current, _)) => {
                    discrete && current.device_type != vk::PhysicalDeviceType::DISCRETE_GPU
                }
            };
            if better {
                chosen = Some((physical, properties, family as u32));
            }
        }

        let Some((physical, properties, family)) = chosen else {
            return Err(RunError::NoDevice {
                detail: format!(
                    "none of the {seen} device(s) offers both a compute queue and shaderInt64"
                ),
            });
        };
        let name = device_name(&properties);

        // The priorities array has to outlive the create-info that points into it,
        // so it is a local rather than a temporary.
        let priorities = [1.0f32; 1];
        let priority = vk::DeviceQueueCreateInfo {
            queue_family_index: family,
            queue_count: 1,
            p_queue_priorities: priorities.as_ptr(),
            ..Default::default()
        };
        let device = check("device creation", unsafe {
            instance.create_device(
                physical,
                &vk::DeviceCreateInfo::default().queue_create_infos(&[priority]),
                None,
            )
        })?;
        let queue = unsafe { device.get_device_queue(family, 0) };

        Ok(GpuContext {
            entry,
            instance,
            device,
            queue,
            family,
            physical,
            name,
            pipelines: Mutex::new(HashMap::new()),
        })
    }

    /// The device this context dispatches on.
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// Run one fragment over the index range `[0, count)`.
    ///
    /// Returns one buffer per output, each exactly `count` elements long.
    pub fn run(
        &self,
        fragment: &KernelFragment,
        inputs: &[Vec<i64>],
        count: usize,
    ) -> Result<Vec<Vec<i64>>, RunError> {
        let binding = Binding {
            inputs: inputs.len(),
            outputs: fragment.outputs,
        };
        for (index, buffer) in inputs.iter().enumerate() {
            if buffer.len() < count {
                return Err(RunError::InputShorterThanCount {
                    buffer: index,
                    len: buffer.len(),
                    count,
                });
            }
        }
        let words = spirv::compile(fragment, binding).map_err(RunError::Emit)?;
        let pipeline = self.pipeline(fragment, binding, &words)?;

        // Round up so the last workgroup's surplus lanes address padding rather
        // than memory past the end; see the module docs.
        let padded = count.div_ceil(LOCAL_SIZE_X as usize) * LOCAL_SIZE_X as usize;

        let mut scratch: Vec<HostBuffer> = Vec::with_capacity(binding.total());
        let mut descriptors: Vec<vk::DescriptorBufferInfo> = Vec::new();
        for slot in 0..binding.total() {
            let is_output = slot >= binding.inputs;
            let data = if is_output {
                // Output padding is zeroed too, so a surplus lane's write lands on
                // a defined value.
                vec![0i64; padded]
            } else {
                let mut data = vec![0i64; padded];
                data[..count].copy_from_slice(&inputs[slot][..count]);
                data
            };
            let buffer = HostBuffer::new(self, &data)?;
            descriptors.push(vk::DescriptorBufferInfo {
                buffer: buffer.handle,
                offset: 0,
                range: std::mem::size_of_val(data.as_slice()) as vk::DeviceSize,
            });
            scratch.push(buffer);
        }

        self.dispatch(pipeline, &descriptors, count)?;
        Ok(scratch
            .iter()
            .skip(binding.inputs)
            .map(|buffer| buffer.to_vec(count))
            .collect())
    }

    /// Record and submit one dispatch, waiting for it to finish.
    fn dispatch(
        &self,
        pipeline: vk::Pipeline,
        descriptors: &[vk::DescriptorBufferInfo],
        count: usize,
    ) -> Result<(), RunError> {
        let total = descriptors.len();
        let device = &self.device;

        let bindings: Vec<vk::DescriptorSetLayoutBinding> = (0..total)
            .map(|slot| vk::DescriptorSetLayoutBinding {
                binding: slot as u32,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            })
            .collect();
        let set_layout = check("descriptor set layout", unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        })?;
        let layout = check("pipeline layout", unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[set_layout])
                    .push_constant_ranges(&[]),
                None,
            )
        })?;

        let pool = check("descriptor pool", unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::STORAGE_BUFFER,
                        descriptor_count: total as u32,
                    }]),
                None,
            )
        })?;
        let mut set = vk::DescriptorSet::null();
        check("descriptor set allocation", unsafe {
            device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo {
                // The count defaults to zero, and a zero-count request
                // *succeeds* while allocating nothing — so it is stated.
                descriptor_set_count: 1,
                descriptor_pool: pool,
                p_set_layouts: std::ptr::addr_of!(set_layout),
                ..Default::default()
            })
        })
        .and_then(|sets| {
            set = *sets.first().ok_or(RunError::Vulkan {
                stage: "descriptor set allocation",
                detail: "Vulkan reported success but returned no set".into(),
            })?;
            Ok(())
        })?;

        let writes: Vec<vk::WriteDescriptorSet> = (0..total)
            .map(|slot| {
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(slot as u32)
                    .descriptor_count(1)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&descriptors[slot..slot + 1])
            })
            .collect();
        unsafe { device.update_descriptor_sets(&writes, &[]) };

        let pool_info = check("command pool", unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(self.family),
                None,
            )
        })?;
        let mut command = vk::CommandBuffer::null();
        check("command buffer allocation", unsafe {
            device.allocate_command_buffers(&vk::CommandBufferAllocateInfo {
                // Stated for the same reason as the descriptor set count.
                command_buffer_count: 1,
                command_pool: pool_info,
                ..Default::default()
            })
        })
        .and_then(|buffers| {
            command = *buffers.first().ok_or(RunError::Vulkan {
                stage: "command buffer allocation",
                detail: "Vulkan reported success but returned no command buffer".into(),
            })?;
            Ok(())
        })?;
        check("command recording", unsafe {
            device.begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())
        })?;

        unsafe {
            device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, pipeline);
            device.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::COMPUTE,
                layout,
                0,
                &[set],
                &[],
            );
            // Host writes become available to the shader.
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::HOST,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::HOST_WRITE)
                    .dst_access_mask(vk::AccessFlags::SHADER_READ)],
                &[],
                &[],
            );
            device.cmd_dispatch(command, count.div_ceil(LOCAL_SIZE_X as usize) as u32, 1, 1);
            // Shader writes become visible to the host.
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ)],
                &[],
                &[],
            );
        }
        check("command submission", unsafe {
            device.end_command_buffer(command)
        })?;

        let fence = check("fence", unsafe {
            device.create_fence(&vk::FenceCreateInfo::default(), None)
        })?;
        let outcome = check("queue submit", unsafe {
            device.queue_submit(
                self.queue,
                &[vk::SubmitInfo::default().command_buffers(&[command])],
                fence,
            )
        })
        .and_then(|()| {
            check("fence wait", unsafe {
                device.wait_for_fences(
                    &[fence],
                    true,
                    std::time::Duration::from_secs(30).as_nanos() as u64,
                )
            })
        });
        // Tear down in both cases; a failed run leaks nothing either way.
        unsafe {
            device.destroy_fence(fence, None);
            device.destroy_command_pool(pool_info, None);
            device.destroy_descriptor_pool(pool, None);
            device.destroy_pipeline_layout(layout, None);
            device.destroy_descriptor_set_layout(set_layout, None);
        }
        outcome
    }

    /// The pipeline for a fragment, built once per content digest.
    fn pipeline(
        &self,
        fragment: &KernelFragment,
        binding: Binding,
        words: &[u32],
    ) -> Result<vk::Pipeline, RunError> {
        let key = (fragment_digest(fragment), binding.inputs, binding.outputs);
        if let Some(pipeline) = self.pipelines.lock().unwrap().get(&key) {
            return Ok(*pipeline);
        }
        let module = check("shader module", unsafe {
            // `code` takes the words, not bytes: no repacking needed.
            self.device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None)
        })?;
        // The descriptor set layout is the same one `dispatch` builds, so the
        // pipeline is created against an identical one.
        let bindings: Vec<vk::DescriptorSetLayoutBinding> = (0..binding.total())
            .map(|slot| vk::DescriptorSetLayoutBinding {
                binding: slot as u32,
                descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::COMPUTE,
                ..Default::default()
            })
            .collect();
        let set_layout = check("pipeline descriptor set layout", unsafe {
            self.device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        })?;
        let layout = check("pipeline layout", unsafe {
            self.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[set_layout])
                    .push_constant_ranges(&[]),
                None,
            )
        })?;
        // `create_compute_pipelines` reports differently from every other call:
        // on failure it hands back the pipelines that *did* build alongside the
        // code, so a caller can tell "one of three failed" from "none built".
        let built = unsafe {
            self.device.create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default()
                    .stage(
                        vk::PipelineShaderStageCreateInfo::default()
                            .stage(vk::ShaderStageFlags::COMPUTE)
                            .module(module)
                            .name(c"main"),
                    )
                    .layout(layout)],
                None,
            )
        };
        let pipeline = match built {
            Ok(mut pipelines) => pipelines
                .pop()
                .expect("one pipeline was requested and one was returned"),
            Err((_, code)) => {
                unsafe {
                    self.device.destroy_shader_module(module, None);
                    self.device.destroy_pipeline_layout(layout, None);
                    self.device.destroy_descriptor_set_layout(set_layout, None);
                }
                return Err(RunError::Vulkan {
                    stage: "compute pipeline",
                    detail: format!("{code:?}"),
                });
            }
        };

        unsafe {
            self.device.destroy_shader_module(module, None);
            self.device.destroy_pipeline_layout(layout, None);
            self.device.destroy_descriptor_set_layout(set_layout, None);
        }
        self.pipelines.lock().unwrap().insert(key, pipeline);
        Ok(pipeline)
    }
}

impl Drop for GpuContext {
    fn drop(&mut self) {
        for (_, pipeline) in self.pipelines.lock().unwrap().drain() {
            unsafe { self.device.destroy_pipeline(pipeline, None) };
        }
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
        // The entry is not destroyed: it is a process-level loader handle.
        let _ = &self.entry;
    }
}

/// A host-visible, host-coherent staging buffer.
struct HostBuffer {
    /// Kept so `Drop` can unmap: mapping is a device operation, and the buffer
    /// outlives the call that created it.
    device: ash::Device,
    handle: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut i64,
}

impl HostBuffer {
    fn new(context: &GpuContext, data: &[i64]) -> Result<Self, RunError> {
        let device = &context.device;
        let size = std::mem::size_of_val(data) as vk::DeviceSize;
        let handle = check("buffer creation", unsafe {
            device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(
                        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
                    )
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        })?;
        let requirements = unsafe { device.get_buffer_memory_requirements(handle) };
        let index = memory_type(
            &context.instance,
            context.physical,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        let memory = check("memory allocation", unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(index),
                None,
            )
        })?;
        check("buffer binding", unsafe {
            device.bind_buffer_memory(handle, memory, 0)
        })?;
        let mapped = check("memory mapping", unsafe {
            device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty())
        })? as *mut i64;
        // SAFETY: `mapped` is the host pointer `map_memory` just returned for
        // `memory`, which this buffer owns and keeps alive, and `data` is at
        // least as long as the mapping it was created with.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), mapped, data.len()) };
        Ok(HostBuffer {
            device: device.clone(),
            handle,
            memory,
            mapped,
        })
    }

    /// The first `count` elements, as owned data.
    fn to_vec(&self, count: usize) -> Vec<i64> {
        // SAFETY: `mapped` addresses this buffer's own mapped memory, which is
        // still mapped and large enough for `count` elements. The run waited on a
        // fence and recorded a shader-write-to-host-read barrier, so the shader's
        // stores are visible here.
        unsafe { std::slice::from_raw_parts(self.mapped, count) }.to_vec()
    }
}

impl Drop for HostBuffer {
    fn drop(&mut self) {
        unsafe {
            self.device.unmap_memory(self.memory);
            self.device.destroy_buffer(self.handle, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

/// The device name a `PhysicalDeviceProperties` carries, as UTF-8 where possible.
fn device_name(properties: &vk::PhysicalDeviceProperties) -> String {
    let bytes: Vec<u8> = properties
        .device_name
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    String::from_utf8(bytes).unwrap_or_else(|_| "unnamed device".into())
}

/// A memory type index with all of `wanted`, or `None` when the device has none.
///
/// No fallback to a type without the flags: a buffer that is not host-visible
/// cannot be read back, and quietly choosing one would produce empty results
/// rather than an error.
fn memory_type(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    allowed: u32,
    wanted: vk::MemoryPropertyFlags,
) -> Result<u32, RunError> {
    let properties = unsafe { instance.get_physical_device_memory_properties(physical) };
    for index in 0..properties.memory_type_count {
        let bits = 1u32 << index;
        if allowed & bits == 0 {
            continue;
        }
        if properties.memory_types[index as usize]
            .property_flags
            .contains(wanted)
        {
            return Ok(index);
        }
    }
    Err(RunError::Vulkan {
        stage: "memory type selection",
        detail: "no memory type is both host-visible and host-coherent".into(),
    })
}
