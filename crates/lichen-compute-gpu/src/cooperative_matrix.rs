//! The cooperative-matrix product: `C = A · B + C` over one subgroup.
//!
//! # Invariant
//! The shape is the device's advertised combination, not a guess: `M16 N16 K32`
//! `s8`→`s32` is what this target's device reports. The components are integer, so
//! the arithmetic is exact and the CPU path agrees bit for bit; the floating shapes
//! are not used because their accumulation order is the hardware's.

/// The reduction length, the hardware's `K`.
pub use lichen_kernel_ir::COOPERATIVE_MATRIX_K as K;
/// The product's rows, the hardware's `M`.
pub use lichen_kernel_ir::COOPERATIVE_MATRIX_M as M;
/// The product's columns, the hardware's `N`.
pub use lichen_kernel_ir::COOPERATIVE_MATRIX_N as N;
/// The workgroup's lanes: one subgroup, the scope every advertised combination uses.
pub const LANES: u32 = 32;

/// The storage buffers one cooperative-matrix product binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bindings {
    /// The descriptor set the three buffers live in.
    pub set: u32,
    /// `A`: `M × K` signed bytes, row-major, stride `K`.
    pub a: u32,
    /// `B`: `K × N` signed bytes, row-major, stride `N`.
    pub b: u32,
    /// `C`: `M × N` signed 32-bit integers, row-major, stride `N`.
    pub c: u32,
}

/// SPIR-V's storage class for the three buffers.
const STORAGE_BUFFER: u32 = 12;

/// `SpvCapabilityVulkanMemoryModel`.
///
/// # Invariant
/// `SPV_KHR_cooperative_matrix` requires it wherever `Shader` and `CooperativeMatrixKHR` are
/// declared; `spirv-val` refuses the module without it. Its own extension carries it: the module
/// is SPIR-V 1.3, where the capability is only an extension's.
const CAPABILITY_VULKAN_MEMORY_MODEL: u32 = 5345;

/// `SpvMemoryModelVulkan`: the memory model the Vulkan capability requires.
const MEMORY_MODEL_VULKAN: u32 = 3;

/// The module: one entry point that loads, multiplies and stores, every lane of one
/// subgroup executing together.
///
/// # Invariant
/// The types name `Subgroup` scope, so `LocalSize` is one subgroup ([`LANES`]) and
/// every lane reaches the same collective op: a wider workgroup would run the product
/// once per subgroup and store `C` twice.
pub fn module(bindings: Bindings) -> Vec<u32> {
    let mut ids = Ids { next: 1 };
    let void = ids.id();
    let fn_type = ids.id();
    let s8 = ids.id();
    let s32 = ids.id();
    let u32_type = ids.id();
    let scope = ids.id();
    let rows = ids.id();
    let columns = ids.id();
    let reduction = ids.id();
    let row_major = ids.id();
    let stride_a = ids.id();
    let stride_b = ids.id();
    let stride_c = ids.id();
    let use_a = ids.id();
    let use_b = ids.id();
    let use_c = ids.id();
    let array_a = ids.id();
    let array_c = ids.id();
    let block_a = ids.id();
    let block_c = ids.id();
    let pointer_s8 = ids.id();
    let pointer_s32 = ids.id();
    let pointer_block_a = ids.id();
    let pointer_block_c = ids.id();
    let matrix_a = ids.id();
    let matrix_b = ids.id();
    let matrix_c = ids.id();
    let zero = ids.id();
    let buffer_a = ids.id();
    let buffer_b = ids.id();
    let buffer_c = ids.id();
    let function = ids.id();
    let label = ids.id();
    let at_a = ids.id();
    let at_b = ids.id();
    let at_c = ids.id();
    let value_a = ids.id();
    let value_b = ids.id();
    let value_c = ids.id();
    let product = ids.id();
    let bound = ids.next;

    let mut words: Vec<u32> = Vec::new();
    words.extend([0x0723_0203, 0x0001_0300, 0, bound, 0]);
    let mut push = |op: u16, operands: &[u32]| {
        words.push(((operands.len() as u32 + 1) << 16) | u32::from(op));
        words.extend_from_slice(operands);
    };
    // Capabilities: the shader, its 8-bit components, the cooperative matrix, and the Vulkan
    // memory model.
    push(op::CAPABILITY, &[1]);
    push(op::CAPABILITY, &[39]);
    push(op::CAPABILITY, &[4448]);
    push(op::CAPABILITY, &[6022]);
    // `VulkanMemoryModel`: required beside the cooperative matrix, and it requires the model below.
    push(op::CAPABILITY, &[CAPABILITY_VULKAN_MEMORY_MODEL]);
    for extension in [
        b"SPV_KHR_8bit_storage\0".as_slice(),
        b"SPV_KHR_cooperative_matrix\0",
        // The capability above is an extension's at SPIR-V 1.3.
        b"SPV_KHR_vulkan_memory_model\0",
    ] {
        let mut operands = Vec::new();
        for chunk in extension.chunks(4) {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            operands.push(u32::from_le_bytes(word));
        }
        push(op::EXTENSION, &operands);
    }
    // Logical addressing and the Vulkan memory model: the model the capability above names.
    push(op::MEMORY_MODEL, &[0, MEMORY_MODEL_VULKAN]);
    let mut entry = vec![5, function];
    entry.extend(spv_string(b"main\0"));
    push(op::ENTRY_POINT, &entry);
    push(op::EXECUTION_MODE, &[function, 17, LANES, 1, 1]);
    // Annotations: the three bindings, the block layouts and the element strides.
    for (buffer, binding) in [
        (buffer_a, bindings.a),
        (buffer_b, bindings.b),
        (buffer_c, bindings.c),
    ] {
        push(op::DECORATE, &[buffer, 34, bindings.set]);
        push(op::DECORATE, &[buffer, 33, binding]);
    }
    push(op::DECORATE, &[array_a, 6, 1]);
    push(op::DECORATE, &[array_c, 6, 4]);
    for block in [block_a, block_c] {
        push(op::DECORATE, &[block, 2]);
        push(op::MEMBER_DECORATE, &[block, 0, 35, 0]);
    }
    // Types and constants.
    push(op::TYPE_VOID, &[void]);
    push(op::TYPE_FUNCTION, &[fn_type, void]);
    push(op::TYPE_INT, &[s8, 8, 1]);
    push(op::TYPE_INT, &[s32, 32, 1]);
    push(op::TYPE_INT, &[u32_type, 32, 0]);
    for (constant, value) in [
        (scope, 3u32),
        (rows, M),
        (columns, N),
        (reduction, K),
        (row_major, 0),
        (stride_a, K),
        (stride_b, N),
        (stride_c, N),
        (use_a, 0),
        (use_b, 1),
        (use_c, 2),
        (zero, 0),
    ] {
        push(op::CONSTANT, &[u32_type, constant, value]);
    }
    push(op::TYPE_RUNTIME_ARRAY, &[array_a, s8]);
    push(op::TYPE_RUNTIME_ARRAY, &[array_c, s32]);
    push(op::TYPE_STRUCT, &[block_a, array_a]);
    push(op::TYPE_STRUCT, &[block_c, array_c]);
    push(op::TYPE_POINTER, &[pointer_s8, STORAGE_BUFFER, s8]);
    push(op::TYPE_POINTER, &[pointer_s32, STORAGE_BUFFER, s32]);
    push(
        op::TYPE_POINTER,
        &[pointer_block_a, STORAGE_BUFFER, block_a],
    );
    push(
        op::TYPE_POINTER,
        &[pointer_block_c, STORAGE_BUFFER, block_c],
    );
    push(
        op::TYPE_COOPERATIVE_MATRIX,
        &[matrix_a, s8, scope, rows, reduction, use_a],
    );
    push(
        op::TYPE_COOPERATIVE_MATRIX,
        &[matrix_b, s8, scope, reduction, columns, use_b],
    );
    push(
        op::TYPE_COOPERATIVE_MATRIX,
        &[matrix_c, s32, scope, rows, columns, use_c],
    );
    push(op::VARIABLE, &[pointer_block_a, buffer_a, STORAGE_BUFFER]);
    push(op::VARIABLE, &[pointer_block_a, buffer_b, STORAGE_BUFFER]);
    push(op::VARIABLE, &[pointer_block_c, buffer_c, STORAGE_BUFFER]);
    // The entry point: the collective product, every lane executing.
    push(op::FUNCTION, &[void, function, 0, fn_type]);
    push(op::LABEL, &[label]);
    push(op::ACCESS_CHAIN, &[pointer_s8, at_a, buffer_a, zero, zero]);
    push(op::ACCESS_CHAIN, &[pointer_s8, at_b, buffer_b, zero, zero]);
    push(op::ACCESS_CHAIN, &[pointer_s32, at_c, buffer_c, zero, zero]);
    push(
        op::COOPERATIVE_MATRIX_LOAD,
        &[matrix_a, value_a, at_a, row_major, stride_a],
    );
    push(
        op::COOPERATIVE_MATRIX_LOAD,
        &[matrix_b, value_b, at_b, row_major, stride_b],
    );
    push(
        op::COOPERATIVE_MATRIX_LOAD,
        &[matrix_c, value_c, at_c, row_major, stride_c],
    );
    push(
        op::COOPERATIVE_MATRIX_MUL_ADD,
        &[matrix_c, product, value_a, value_b, value_c, 0xf],
    );
    push(
        op::COOPERATIVE_MATRIX_STORE,
        &[at_c, product, row_major, stride_c],
    );
    push(op::RETURN, &[]);
    push(op::FUNCTION_END, &[]);
    words
}

/// The ids the module hands out, in the order it first needs them.
struct Ids {
    next: u32,
}

impl Ids {
    fn id(&mut self) -> u32 {
        let id = self.next;
        self.next += 1;
        id
    }
}

/// A NUL-terminated string as the words a `LiteralString` operand is.
fn spv_string(text: &[u8]) -> Vec<u32> {
    let mut operands = Vec::new();
    for chunk in text.chunks(4) {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        operands.push(u32::from_le_bytes(word));
    }
    operands
}

/// The module's opcodes, from the Khronos `spirv.h`.
mod op {
    pub const CAPABILITY: u16 = 17;
    pub const EXTENSION: u16 = 10;
    pub const MEMORY_MODEL: u16 = 14;
    pub const ENTRY_POINT: u16 = 15;
    pub const EXECUTION_MODE: u16 = 16;
    pub const DECORATE: u16 = 71;
    pub const MEMBER_DECORATE: u16 = 72;
    pub const TYPE_VOID: u16 = 19;
    pub const TYPE_INT: u16 = 21;
    pub const TYPE_STRUCT: u16 = 30;
    pub const TYPE_RUNTIME_ARRAY: u16 = 29;
    pub const TYPE_POINTER: u16 = 32;
    pub const TYPE_FUNCTION: u16 = 33;
    pub const CONSTANT: u16 = 43;
    pub const VARIABLE: u16 = 59;
    pub const ACCESS_CHAIN: u16 = 65;
    pub const FUNCTION: u16 = 54;
    pub const LABEL: u16 = 248;
    pub const RETURN: u16 = 253;
    pub const FUNCTION_END: u16 = 56;
    pub const TYPE_COOPERATIVE_MATRIX: u16 = 4456;
    pub const COOPERATIVE_MATRIX_LOAD: u16 = 4457;
    pub const COOPERATIVE_MATRIX_STORE: u16 = 4458;
    pub const COOPERATIVE_MATRIX_MUL_ADD: u16 = 4459;
}

/// The integer shape a device advertises, or `None` where it advertises none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Advertised {
    /// The product's rows.
    pub m: u32,
    /// The product's columns.
    pub n: u32,
    /// The reduction length.
    pub k: u32,
}

/// The device's own answer: the extension, the feature, and an `s8`→`s32` composition
/// at subgroup scope.
///
/// # Invariant
/// The extension's presence is not the feature's: a device may list
/// `VK_KHR_cooperative_matrix` and answer `cooperativeMatrix` false, and the module is
/// then refused at pipeline creation. Both are asked, and the combination must be the
/// one the module is written for.
pub fn advertised(
    entry: &ash::Entry,
    instance: &ash::Instance,
    physical: ash::vk::PhysicalDevice,
) -> Option<Advertised> {
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }.ok()?;
    let present = extensions.iter().any(|property| {
        property
            .extension_name_as_c_str()
            .is_ok_and(|name| name == ash::vk::KHR_COOPERATIVE_MATRIX_NAME)
    });
    if !present {
        return None;
    }
    let mut features = ash::vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default();
    let mut features2 = ash::vk::PhysicalDeviceFeatures2::default().push_next(&mut features);
    // SAFETY: `features` is chained into `features2` and outlives the call.
    unsafe { instance.get_physical_device_features2(physical, &mut features2) };
    if features.cooperative_matrix == 0 {
        return None;
    }
    let cooperative = ash::khr::cooperative_matrix::Instance::new(entry, instance);
    let combinations =
        unsafe { cooperative.get_physical_device_cooperative_matrix_properties(physical) }.ok()?;
    combinations
        .iter()
        .find(|combination| {
            combination.m_size == M
                && combination.n_size == N
                && combination.k_size == K
                && combination.a_type == ash::vk::ComponentTypeKHR::SINT8
                && combination.b_type == ash::vk::ComponentTypeKHR::SINT8
                && combination.c_type == ash::vk::ComponentTypeKHR::SINT32
                && combination.result_type == ash::vk::ComponentTypeKHR::SINT32
                && combination.scope == ash::vk::ScopeKHR::SUBGROUP
        })
        .map(|combination| Advertised {
            m: combination.m_size,
            n: combination.n_size,
            k: combination.k_size,
        })
}
