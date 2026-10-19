//! The cooperative-matrix product: `C = A · B + C` over one subgroup.
//!
//! # Invariant
//! The module's shape is the device's own advertised combination, so the device test
//! is skipped where no device advertises `M16 N16 K32` `s8`→`s32` at subgroup scope
//! rather than run against a guess, and the validator's absence is a skip the same
//! way. The reference is exact integer arithmetic, so the agreement is bit for bit
//! and not a tolerance.

use ash::vk;
use lichen_compute_gpu::cooperative_matrix::{self, Bindings, K, M, N};

/// The host's reference: `C = A · B + C`, row-major signed bytes and 32-bit integers.
///
/// # Invariant
/// The arithmetic is integer, so the order of the sum is not observable: the device's
/// internal accumulation order cannot disagree with this one.
fn reference(a: &[i8], b: &[i8], c: &mut [i32]) {
    for row in 0..M as usize {
        for column in 0..N as usize {
            let mut sum = c[row * N as usize + column];
            for step in 0..K as usize {
                sum += i32::from(a[row * K as usize + step])
                    * i32::from(b[step * N as usize + column]);
            }
            c[row * N as usize + column] = sum;
        }
    }
}

#[test]
fn the_context_advertises_the_shape_it_can_run() {
    let Some(context) = super::common::context("the_context_advertises_the_shape_it_can_run")
    else {
        return;
    };
    match context.cooperative_matrix() {
        Some(shape) => assert_eq!((shape.m, shape.n, shape.k), (M, N, K)),
        None => eprintln!(
            "SKIPPED the_context_advertises_the_shape_it_can_run: this device advertises              no s8 cooperative matrix"
        ),
    }
}

#[test]
fn the_cooperative_matrix_module_validates() {
    let words = cooperative_matrix::module(Bindings {
        set: 0,
        a: 0,
        b: 1,
        c: 2,
    });
    assert!(
        super::spirv_validation::validate("the cooperative matrix module", &words),
        "the cooperative matrix module did not validate"
    );
}

#[test]
fn the_cooperative_matrix_product_matches_the_host() {
    let entry = unsafe { ash::Entry::load() }.expect("loader");
    let instance = unsafe {
        entry.create_instance(
            &vk::InstanceCreateInfo::default().application_info(
                &vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 3, 0)),
            ),
            None,
        )
    }
    .expect("instance");
    let devices = unsafe { instance.enumerate_physical_devices() }.expect("devices");
    let mut chosen = None;
    for physical in devices {
        let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
            .expect("extensions");
        let cooperative = extensions.iter().any(|property| {
            property
                .extension_name_as_c_str()
                .is_ok_and(|name| name == ash::vk::KHR_COOPERATIVE_MATRIX_NAME)
        });
        let mut twelve = vk::PhysicalDeviceVulkan12Features::default();
        let mut matrix = vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default();
        let mut features = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut twelve)
            .push_next(&mut matrix);
        unsafe { instance.get_physical_device_features2(physical, &mut features) };
        if cooperative && matrix.cooperative_matrix != 0 {
            chosen = Some(physical);
        }
    }
    let Some(physical) = chosen else {
        eprintln!(
            "SKIPPED the_cooperative_matrix_product_matches_the_host: no device advertises \
             VK_KHR_cooperative_matrix"
        );
        return;
    };
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    let family = families
        .iter()
        .position(|queue| queue.queue_flags.contains(vk::QueueFlags::COMPUTE))
        .expect("compute queue");

    // The features the module is written against: int8, 8-bit storage, the matrix.
    let mut twelve = vk::PhysicalDeviceVulkan12Features::default()
        .shader_int8(true)
        .storage_buffer8_bit_access(true);
    let mut matrix =
        vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default().cooperative_matrix(true);
    let priority = [1.0f32];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family as u32)
        .queue_priorities(&priority)];
    let extension_names = [ash::vk::KHR_COOPERATIVE_MATRIX_NAME.as_ptr()];
    let create = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queues)
        .enabled_extension_names(&extension_names)
        .push_next(&mut twelve)
        .push_next(&mut matrix);
    let device = unsafe { instance.create_device(physical, &create, None) }.expect("device");

    let words = cooperative_matrix::module(Bindings {
        set: 0,
        a: 0,
        b: 1,
        c: 2,
    });

    // The three storage buffers are host-visible, so the test needs no staging path.
    let mut make_buffer = |size: u64| {
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&info, None) }.expect("buffer");
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let memory_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(host_visible_type(
                &instance,
                physical,
                requirements.memory_type_bits,
            ));
        let memory = unsafe { device.allocate_memory(&memory_info, None) }.expect("memory");
        unsafe { device.bind_buffer_memory(buffer, memory, 0) }.expect("bind");
        (buffer, memory)
    };
    let (a_buffer, a_memory) = make_buffer(K as u64 * M as u64);
    let (b_buffer, b_memory) = make_buffer(K as u64 * N as u64);
    let (c_buffer, c_memory) = make_buffer(4 * M as u64 * N as u64);

    let a: Vec<i8> = (0..(M * K) as usize)
        .map(|index| (index % 5) as i8 - 2)
        .collect();
    let b: Vec<i8> = (0..(K * N) as usize)
        .map(|index| (index % 7) as i8 - 3)
        .collect();
    let c: Vec<i32> = (0..(M * N) as usize).map(|index| index as i32).collect();
    let mut expected = c.clone();
    reference(&a, &b, &mut expected);

    let a_bytes: Vec<u8> = a.iter().map(|value| *value as u8).collect();
    let b_bytes: Vec<u8> = b.iter().map(|value| *value as u8).collect();
    let c_bytes: Vec<u8> = c.iter().flat_map(|value| value.to_le_bytes()).collect();
    // SAFETY: each region is the whole allocation the memory was made for, and the
    // device is idle.
    unsafe {
        write_memory(&device, a_memory, &a_bytes);
        write_memory(&device, b_memory, &b_bytes);
        write_memory(&device, c_memory, &c_bytes);
    }

    let bindings: Vec<vk::DescriptorSetLayoutBinding> = (0..3)
        .map(|binding| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(binding)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
        })
        .collect();
    let set_layout = unsafe {
        device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
            None,
        )
    }
    .expect("set layout");
    let set_layouts = [set_layout];
    let pipeline_layout = unsafe {
        device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts),
            None,
        )
    }
    .expect("pipeline layout");
    let shader = unsafe {
        device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
    }
    .expect("shader");
    let pipeline = unsafe {
        device.create_compute_pipelines(
            vk::PipelineCache::null(),
            &[vk::ComputePipelineCreateInfo::default()
                .stage(
                    vk::PipelineShaderStageCreateInfo::default()
                        .stage(vk::ShaderStageFlags::COMPUTE)
                        .module(shader)
                        .name(std::ffi::CStr::from_bytes_with_nul(b"main\0").unwrap()),
                )
                .layout(pipeline_layout)],
            None,
        )
    }
    .expect("pipeline")[0];
    let pool = unsafe {
        device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(1)
                .pool_sizes(&[vk::DescriptorPoolSize::default()
                    .ty(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(3)]),
            None,
        )
    }
    .expect("pool");
    let sets = unsafe {
        device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool)
                .set_layouts(&set_layouts),
        )
    }
    .expect("sets");
    let buffers = [a_buffer, b_buffer, c_buffer];
    let infos: Vec<[vk::DescriptorBufferInfo; 1]> = buffers
        .iter()
        .map(|buffer| {
            [vk::DescriptorBufferInfo::default()
                .buffer(*buffer)
                .offset(0)
                .range(vk::WHOLE_SIZE)]
        })
        .collect();
    let writes: Vec<vk::WriteDescriptorSet<'_>> = (0..3)
        .map(|index| {
            vk::WriteDescriptorSet::default()
                .dst_set(sets[0])
                .dst_binding(index as u32)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&infos[index])
        })
        .collect();
    unsafe { device.update_descriptor_sets(&writes, &[]) };

    let command_pool = unsafe {
        device.create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(family as u32),
            None,
        )
    }
    .expect("command pool");
    let command = unsafe {
        device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
    }
    .expect("command buffer")[0];
    // SAFETY: the pipeline and the set outlive the recording, and the submission is
    // waited on before the buffer is read.
    unsafe {
        device
            .begin_command_buffer(
                command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .expect("begin");
        device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, pipeline);
        device.cmd_bind_descriptor_sets(
            command,
            vk::PipelineBindPoint::COMPUTE,
            pipeline_layout,
            0,
            &sets,
            &[],
        );
        device.cmd_dispatch(command, 1, 1, 1);
        device.end_command_buffer(command).expect("end");
        let queue = device.get_device_queue(family as u32, 0);
        let commands = [command];
        let submit = vk::SubmitInfo::default().command_buffers(&commands);
        device
            .queue_submit(queue, &[submit], vk::Fence::null())
            .expect("submit");
        device.queue_wait_idle(queue).expect("wait");
    }

    let result = unsafe { read_memory(&device, c_memory, c_bytes.len()) };
    let result: Vec<i32> = result
        .chunks_exact(4)
        .map(|chunk| i32::from_le_bytes(chunk.try_into().unwrap()))
        .collect();

    // SAFETY: the device is idle, and every object is the one this test created.
    unsafe {
        device.destroy_pipeline(pipeline, None);
        device.destroy_pipeline_layout(pipeline_layout, None);
        device.destroy_descriptor_pool(pool, None);
        device.destroy_descriptor_set_layout(set_layout, None);
        device.destroy_shader_module(shader, None);
        device.destroy_command_pool(command_pool, None);
        for buffer in buffers {
            device.destroy_buffer(buffer, None);
        }
        device.destroy_device(None);
        instance.destroy_instance(None);
    }
    assert_eq!(
        result, expected,
        "the device's product disagrees with the host's reference"
    );
}

/// A memory type that is both in `bits` and host-visible.
///
/// # Invariant
/// The test needs no staging path, so the buffer's memory is mapped directly; a device
/// with no such type has no host-visible storage buffer and the test cannot run here.
fn host_visible_type(instance: &ash::Instance, physical: vk::PhysicalDevice, bits: u32) -> u32 {
    let properties = unsafe { instance.get_physical_device_memory_properties(physical) };
    (0..properties.memory_type_count)
        .find(|index| {
            bits & (1 << index) != 0
                && properties.memory_types[*index as usize]
                    .property_flags
                    .contains(
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )
        })
        .expect("host-visible memory")
}

/// Write `bytes` into the start of `memory`.
///
/// # Invariant
/// The caller holds the allocation entire; a partial write would leave the shader's
/// operands at whatever the allocation held.
unsafe fn write_memory(device: &ash::Device, memory: vk::DeviceMemory, bytes: &[u8]) {
    let pointer =
        unsafe { device.map_memory(memory, 0, bytes.len() as u64, vk::MemoryMapFlags::empty()) }
            .expect("map");
    // SAFETY: the mapping covers `len` bytes and `bytes` is that long.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len()) };
    unsafe { device.unmap_memory(memory) };
}

/// Read the first `len` bytes of `memory`.
///
/// # Invariant
/// The caller holds the allocation entire and the device is idle.
unsafe fn read_memory(device: &ash::Device, memory: vk::DeviceMemory, len: usize) -> Vec<u8> {
    let pointer = unsafe { device.map_memory(memory, 0, len as u64, vk::MemoryMapFlags::empty()) }
        .expect("map");
    let mut bytes = vec![0u8; len];
    // SAFETY: the mapping covers `len` bytes and `bytes` is that long.
    unsafe { std::ptr::copy_nonoverlapping(pointer.cast::<u8>(), bytes.as_mut_ptr(), len) };
    unsafe { device.unmap_memory(memory) };
    bytes
}
