//! The Vulkan side: a device, a pipeline per fragment, and a dispatch.
//!
//! # Invariant
//! Every buffer is allocated rounded up to a whole workgroup, and every invocation
//! reaches its write, so only the `count` real elements are ever read back. The
//! tail-lane accounting, where the data lives, and the memory ordering:
//! `docs/notes/lichen-compute-gpu.md`.
//! The Vulkan dispatch backend: device-local buffers, explicit ordering, host pools.
//!
//! # Invariant
//! A launch set that stages a slice is dispatched over a whole number of workgroups
//! only, so no surplus lane writes a slot a real lane of its workgroup reads.
//!
//! # Invariant
//! Buffers are device-local and data reaches them through a host-cached staging buffer: an
//! uncached host-visible allocation is a correct but appallingly slow buffer, so a device with no
//! cached host memory is refused by name. Coherent memory is not ordering, so each point carries
//! its own barrier rather than relying on the submit and the fence.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use ash::vk;
use lichen_kernel_ir::{
    BatchExtent, BatchInput, BatchRefusal, BufferSlot, DeviceExtent, Dispatch, LaunchSet,
    ResidentId, ScalarClass, ScalarData, fragment_digest,
};
use lichen_runtime::{DeviceFacts, DeviceFactsSource};

use crate::spirv::{self, Binding, SpirvRefusal};

/// Why a run could not happen.  Every variant names its own cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// No Vulkan physical device could be used.
    NoDevice { detail: String },
    /// The device cannot represent the fragment's declared integer width.
    MissingInt64 { device: String },
    /// The fragment is outside what this backend emits.
    Emit(SpirvRefusal),
    /// An input buffer is shorter than the run's count; refused rather than zero-filled.
    InputShorterThanCount {
        buffer: usize,
        len: usize,
        count: usize,
    },
    /// A run over no indices: Vulkan has no zero-sized allocation to give it.
    EmptyRun,
    /// A launch that stages a slice at a count that is not a whole number of workgroups.
    ///
    /// # Invariant
    /// A dispatch rounds up to whole workgroups and runs the surplus lanes with zero-padded input,
    /// so a surplus lane of a staged body writes a value a real lane of its workgroup reads and no
    /// diagnostic sees it. Both numbers are named because the pair is what is wrong
    /// (`docs/notes/shared-memory-and-barriers.md` §8).
    StagedLaunchNotAWholeWorkgroup { count: usize, lane_width: u32 },
    /// More storage buffers than the pool has room for; the pool is allocated once.
    TooManyBindings { total: usize, max: usize },
    /// A run asked for a dispatch count one submission cannot record.
    ///
    /// # Invariant
    /// One to `MAX_DISPATCHES_PER_SUBMISSION`, which is what the pool has sets for; zero is
    /// refused too, because a submission with no dispatch is a different request, not a shorter
    /// run. A chain and a graph are one rule and one number, not two bounds.
    TooManyDispatches { wanted: usize, max: usize },
    /// A batch edge naming an output no earlier dispatch produces.
    ///
    /// # Invariant
    /// Named rather than skipped: an edge that resolved to nothing would bind no buffer and
    /// the dispatch would write over uninitialised memory, which is a wrong answer with no
    /// diagnostic in it.
    UnknownBatchOutput { dispatch: usize, ordinal: usize },
    /// A chain needs each link to have one input and one output.
    ChainNotLinear { inputs: usize, outputs: usize },
    /// A chain whose fragment reads one class and writes the other, which a chain cannot carry.
    ChainCrossesClasses {
        input: ScalarClass,
        output: ScalarClass,
    },
    /// A launch whose runtime scalars are not the ones its parameter declares.
    ScalarLeavesMismatch { declared: usize, given: usize },
    /// A fragment whose extent leaf is not an integer.
    ///
    /// # Invariant
    /// The extent's value is the count the dispatch runs over, and there is no float a count could
    /// be read as without inventing one: the ABI calls this leaf a count, so a `Float` one would
    /// push a converted number where the program wrote a whole number.
    ExtentNotAnInteger { class: ScalarClass },
    /// A fragment that reads its extent leaf, dispatched over a number only the device holds.
    ///
    /// # Invariant
    /// An indirect dispatch reads the workgroup count out of a buffer and hands the shader
    /// nothing: the extent leaf is a push constant, fixed when the command is recorded, so a
    /// body that reads it would loop to a number this backend does not have.
    ExtentLeafUnfixed,
    /// An extent that is not a whole number of integers in a buffer this context holds.
    IndirectExtentNotIntegers { len: usize },
    /// An extent whose element has no room for the command read out of it.
    ///
    /// # Invariant
    /// The read is twelve bytes and the elements are eight, so the count cannot be the last one
    /// in the buffer: refused rather than read past what was allocated.
    IndirectExtentOutOfReach { index: usize, len: usize },
    /// A leaf block past what this device offers.
    ///
    /// # Invariant
    /// A device's limit is a floor, not a promise: a fragment may declare more scalar leaves than
    /// `maxPushConstantsSize` covers, and the block would be truncated silently.
    PushConstantsTooLarge { bytes: u32, max: u32 },
    /// A resident id this context is not holding: never issued, or released.
    UnknownResident { id: u64 },
    /// A pool depth of zero, which is not a smaller pool.
    NoSlots { wanted: usize },
    /// A token naming a slot this context does not have.
    ///
    /// # Invariant
    /// A handle that names nothing must say so: waiting on a slot this context does not own would
    /// block on someone else's fence.
    UnknownSlot { slot: u32 },
    /// A fetch asked for more elements than the buffer was allocated for.
    FetchLongerThanBuffer { len: usize, count: usize },
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
                "the device {device:?} does not support `shaderInt64`, and this fragment's values \
                 are 64-bit integers. Narrowing the values would change what the program \
                 computes, so this is refused rather than converted. A fragment whose values are \
                 floats does not need the feature: its integers are 32-bit indices."
            ),
            RunError::EmptyRun => write!(
                f,
                "the run covers no indices, so there is no buffer for it to write: an empty \
                 dispatch is a program with nothing to do, not a smaller run."
            ),
            RunError::StagedLaunchNotAWholeWorkgroup { count, lane_width } => write!(
                f,
                "the launch covers {count} indices, which is not a whole number of workgroups of \
                 {lane_width} lanes, and this launch stages a slice: a dispatch rounds up \
                 and runs the surplus lanes with zero-padded input, so a surplus lane's fabricated \
                 value would reach a real lane's answer with no diagnostic. Launch a multiple of \
                 {lane_width}, or stage nothing (`docs/notes/shared-memory-and-barriers.md` §8)."
            ),
            RunError::TooManyBindings { total, max } => write!(
                f,
                "the run binds {total} storage buffer(s) and this backend's descriptor pool is \
                 sized for {max}, so it is refused here rather than left to fail as pool \
                 exhaustion on the device"
            ),
            RunError::TooManyDispatches { wanted, max } => write!(
                f,
                "a run of {wanted} dispatch(es) cannot be recorded in one submission: the \
                 range is 1 to {max}, which is what the descriptor pool has sets for."
            ),
            RunError::UnknownBatchOutput { dispatch, ordinal } => write!(
                f,
                "this batch reads output {ordinal} of dispatch {dispatch}, which no dispatch \
                 before the one reading it produces."
            ),
            RunError::ChainNotLinear { inputs, outputs } => write!(
                f,
                "a chain needs one input and one output per link so each can be handed the \
                 previous one's result, and this fragment has {inputs} and {outputs}."
            ),
            RunError::ChainCrossesClasses { input, output } => write!(
                f,
                "a chain feeds each link's output back into the next link's input slot, so both \
                 ends of the fragment must be one class, and this one reads {input:?} and \
                 writes {output:?}."
            ),
            RunError::Emit(refusal) => write!(f, "{refusal}"),
            RunError::ScalarLeavesMismatch { declared, given } => write!(
                f,
                "this fragment's parameter declares {declared} runtime scalar(s) beside the \
                 extent and this launch fixes {given}: a leaf is read at the position the \
                 parameter declares, so the two have to agree or a lane would compute from another \
                 leaf's value"
            ),
            RunError::ExtentNotAnInteger { class } => write!(
                f,
                "this fragment's parameter declares its extent leaf as {class:?}, and the extent \
                 is the count this run dispatches over: there is no float a count could be read as \
                 without inventing one, so it is refused rather than converted"
            ),
            RunError::ExtentLeafUnfixed => write!(
                f,
                "this fragment reads its extent leaf, and the number it would read is only on the \
                 device: an indirect dispatch gives the shader no extent to push. It is refused \
                 rather than dispatched over a number the program did not write"
            ),
            RunError::IndirectExtentNotIntegers { len } => write!(
                f,
                "the extent names a buffer holding {len} element(s) that are not integers, so \
                 the workgroup count would be read out of bits that are not a number"
            ),
            RunError::IndirectExtentOutOfReach { index, len } => write!(
                f,
                "the extent names element {index} of a buffer holding {len} element(s): a \
                 dispatch reads a twelve-byte command, so the count and the element after it \
                 must both be inside the buffer"
            ),
            RunError::PushConstantsTooLarge { bytes, max } => write!(
                f,
                "this fragment's scalar leaves fill {bytes} byte(s) of push constants and this \
                 device offers {max}, so it is refused here rather than dispatched with a block \
                 the device would read past"
            ),
            RunError::InputShorterThanCount { buffer, len, count } => write!(
                f,
                "input buffer {buffer} holds {len} element(s) but the run covers {count} \
                 index/indices, so a lane would read past it."
            ),
            RunError::UnknownResident { id } => write!(
                f,
                "buffer {id} is not one this device is holding: it was never issued, or it has \
                 already been released."
            ),
            RunError::NoSlots { wanted } => write!(
                f,
                "a pool depth of {wanted} was asked for, and no submission can be recorded \
                 without a slot to record it into: this is not a smaller pool, it is none."
            ),
            RunError::UnknownSlot { slot } => write!(
                f,
                "submission slot {slot} is not one this device has: the token was issued by \
                 another context, or was never a slot here."
            ),
            RunError::FetchLongerThanBuffer { len, count } => write!(
                f,
                "asked for {count} element(s) of a buffer allocated for {len}, so the read would \
                 run past what the run actually produced."
            ),
            RunError::Vulkan { stage, detail } => write!(f, "Vulkan {stage} failed: {detail}"),
        }
    }
}

impl std::error::Error for RunError {}

/// Where a staged run's index range comes from.
///
/// # Invariant
/// The arms differ in where the number lives and nowhere else, so every step that takes a
/// count is written once. A device-held extent names its **buffer**, not an id: a batch
/// reads a count out of an output of the submission it is still recording, and has issued
/// no id for it.
enum Extent {
    /// `[0, count)`, a number the host holds.
    Host(usize),
    /// `[0, device)`, a number only the device holds, with the run's allocation.
    Device {
        buffer: DeviceBuffer,
        index: usize,
        capacity: usize,
    },
}

impl Extent {
    /// How many elements a run over this extent is allocated for.
    ///
    /// # Invariant
    /// The device arm's `capacity`, because the number deciding how much of those buffers
    /// the shader reaches is not one this host has.
    fn allocated(&self) -> usize {
        match self {
            Extent::Host(count) => *count,
            Extent::Device { capacity, .. } => *capacity,
        }
    }
}

/// Where a recorded dispatch's workgroup count is read from.
///
/// # Invariant
/// `Indirect` names a **command block of the backend's own** and the element the count is
/// copied out of: `vkCmdDispatchIndirect` reads three workgroup counts, and one with none in
/// `y` or `z` runs nothing at all — so the block, not the caller's buffer, carries that shape.
/// The count reaches it on the device, so the host never holds the number it dispatched over.
enum Workgroups {
    /// A number the host already holds.
    Direct(u32),
    /// A number a device buffer holds, read when the submission executes.
    Indirect {
        /// The twelve bytes a dispatch reads its three workgroup counts from.
        block: vk::Buffer,
        /// The buffer the count is in.
        source: vk::Buffer,
        /// Where in it, in bytes.
        offset: vk::DeviceSize,
    },
}

/// One leaf's bytes at its member's offset, at the width the emitter gave that class.
///
/// # Invariant
/// The low bytes of the word, little-endian: a narrower member is the same number read at a
/// smaller width, which is what an integer leaf of a float module is.
fn write_leaf(bytes: &mut [u8], offset: u32, module: ScalarClass, class: ScalarClass, bits: i64) {
    let width = spirv::push_width(module, class) as usize;
    let word = (bits as u64).to_le_bytes();
    bytes[offset as usize..][..width].copy_from_slice(&word[..width]);
}

/// How many elements of `width` a host input's own bytes hold.
///
/// # Invariant
/// Whole elements only: a trailing partial element is not an element, so neither the staging
/// reservation nor the copy counts one. Both ask here, so the two widths cannot drift apart.
fn own_elements(data: &[u8], width: usize) -> usize {
    data.len() / width
}

/// Turn a Vulkan failure into a named `RunError`.
fn check<T>(stage: &'static str, result: Result<T, vk::Result>) -> Result<T, RunError> {
    result.map_err(|code| RunError::Vulkan {
        stage,
        detail: format!("{code:?}"),
    })
}

/// How many workgroups a launch of `count` indices over `launch` is, or why it cannot be one.
///
/// # Invariant
/// A launch set that stages declares `Workgroup` storage for the module, so a surplus lane —
/// which the rounding runs with zero-padded input — can write a slot a real lane of its workgroup
/// reads, and no diagnostic sees it. A staged launch is therefore a whole number of workgroups; the
/// width is the root fragment's own declared fact
/// (`docs/notes/shared-memory-and-barriers.md` §8).
fn launch_workgroups(launch: &LaunchSet<'_>, count: usize) -> Result<usize, RunError> {
    let lane_width = launch.root().target.lane_width.lanes();
    let stages = launch
        .ordered()
        .iter()
        .any(|fragment| !fragment.slices.is_empty());
    if stages && count % lane_width as usize != 0 {
        return Err(RunError::StagedLaunchNotAWholeWorkgroup { count, lane_width });
    }
    Ok(count.div_ceil(lane_width as usize))
}

/// What the chosen device is, as the facts an artifact is checked against.
///
/// # Invariant
/// Each field is the device's **own** answer, not this target's constant: `shaderInt64` for
/// the integer width, `maxComputeWorkGroupInvocations` against `maxComputeWorkGroupSize[0]` for
/// the lanes a row may run and `[1]` for its rows, `subgroupSize` for the subgroup, and the two
/// limits a module declares against.
fn device_facts(
    entry: &ash::Entry,
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    properties: &vk::PhysicalDeviceProperties,
    shader_int64: bool,
) -> DeviceFacts {
    let limits = properties.limits;
    DeviceFacts {
        int_bits: if shader_int64 { 64 } else { 32 },
        max_workgroup_lanes: limits
            .max_compute_work_group_invocations
            .min(limits.max_compute_work_group_size[0]),
        max_workgroup_lanes_y: limits.max_compute_work_group_size[1],
        subgroup_width: subgroup_width(entry, instance, physical),
        max_shared_bytes: limits.max_compute_shared_memory_size,
        max_storage_buffers: limits.max_per_stage_descriptor_storage_buffers,
    }
}

/// The lanes one subgroup runs, or `None` where the loader cannot be asked.
///
/// # Invariant
/// `subgroupSize` is reachable only through `vkGetPhysicalDeviceProperties2`, and the
/// **loader** decides whether that entry point exists: a Vulkan 1.1 loader exports the
/// 1.1 points whatever instance version was asked for, and this crate asks for none.
/// A 1.0 loader therefore leaves the fact unreported rather than guessing a width.
fn subgroup_width(
    entry: &ash::Entry,
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
) -> Option<u32> {
    let loader = unsafe { entry.try_enumerate_instance_version() }
        .ok()
        .flatten()?;
    if loader < vk::API_VERSION_1_1 {
        return None;
    }
    let mut subgroup = vk::PhysicalDeviceSubgroupProperties::default();
    let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut subgroup);
    // SAFETY: `subgroup` is chained into `properties` and outlives the call.
    unsafe { instance.get_physical_device_properties2(physical, &mut properties) };
    Some(subgroup.subgroup_size)
}

/// A device, its compute queue, and the pipelines built on it.
///
/// # Invariant
/// One context owns one device; pipelines are cached on the fragment's content digest, so a
/// fragment run twice compiles once — the same identity the wasm backend's module cache keys on.
pub struct GpuContext {
    entry: ash::Entry,
    instance: ash::Instance,
    device: ash::Device,
    queue: vk::Queue,
    /// Retained so memory-type queries can be answered.
    physical: vk::PhysicalDevice,
    /// Retained for the pipeline cache key and the device name in a refusal.
    name: String,
    /// Whether the device offers `shaderInt64`.
    ///
    /// # Invariant
    /// A property of the fragment decides whether it matters, not of the device: an integer
    /// fragment needs the feature, a float one's integers are 32-bit indices. So it is recorded
    /// here and checked where a fragment is compiled, rather than rejecting the device.
    shader_int64: bool,
    /// The most push-constant bytes this device offers.
    ///
    /// # Invariant
    /// A limit, not a promise: a fragment may declare more scalar leaves than it covers, and the
    /// block would be read past rather than refused, so it is checked where the block is built.
    push_constant_bytes: u32,
    /// What this device is, as the facts a fragment is checked against.
    ///
    /// # Invariant
    /// Probed once, where the device is chosen, and never re-read: a loader compares the
    /// artifact's assumptions against it before anything is dispatched, and a query per load
    /// would be a second answer to a question whose first answer cannot change while the
    /// context lives.
    facts: DeviceFacts,
    /// The pipeline cache, keyed on the **whole set's** digests, not the root's:
    /// two sets sharing a root are two modules.
    pipelines: Mutex<HashMap<(Vec<u64>, usize, usize), vk::Pipeline>>,
    /// Buffers handed out as [`ResidentId`]s and not yet released.
    resident: Mutex<HashMap<ResidentId, DeviceBuffer>>,
    /// Hands out ids. Never reused: a stale id released twice must not name a
    /// live buffer, so ids only ever move forward.
    next_id: AtomicU64,
    /// The submissions' own objects, one set per slot.
    ///
    /// # Invariant
    /// This is what makes more than one submission in flight possible: a single command buffer
    /// cannot be recorded while a previous recording of it runs.
    slots: Mutex<Slots>,
    /// The descriptor set and pipeline layouts for a run with `n` buffers and a
    /// push-constant block of `push` bytes.
    layouts: Mutex<HashMap<(usize, u32), Layouts>>,
    /// Device buffers that have been given back, keyed by their element count and class.
    ///
    /// # Invariant
    /// Reuse is safe **without clearing**: every consumer of a buffer writes all of it —
    /// an output across `[0, padded)` and an input across `[0, len)`, the padding past `len`
    /// filled on the device — so no stale byte is ever read back.
    recycled: Mutex<HashMap<(usize, ScalarClass), Vec<DeviceBuffer>>>,
    /// How many command buffers this context has handed to the queue.
    ///
    /// # Invariant
    /// Counted at the `queue_submit` and nowhere else, because it is the only way to
    /// *observe* that several dispatches went as one: a command buffer's shape is a claim
    /// and this is the count. A read-back is a submission too, so what a caller reads is
    /// every hand-over, not only the dispatches'.
    submissions: AtomicU64,
    /// How many fences this context has waited on.
    ///
    /// # Invariant
    /// One per submission that finished, counted where the wait confirmed it — so "one
    /// submission and one wait" is two numbers read off the device rather than one claim
    /// about a command buffer.
    waits: AtomicU64,
}

/// How deep a pool is when nobody says otherwise.
///
/// # Invariant
/// Two, and not one, because one is not a pool: with a single slot a submission must finish before
/// the next is recorded, so host and device work never overlap.
pub const DEFAULT_SLOT_DEPTH: usize = 2;

/// What a caller may choose about a context.
///
/// # Invariant
/// Rust-side only: a pool depth is a scheduling budget, not a semantic choice, so a language
/// surface for it would make a program's answers depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuConfig {
    /// How many submissions may be in flight at once.
    pub slot_depth: usize,
}

impl Default for GpuConfig {
    fn default() -> Self {
        GpuConfig {
            slot_depth: DEFAULT_SLOT_DEPTH,
        }
    }
}

/// The objects one submission owns, and the claim on them.
///
/// # Invariant
/// Everything a submission reads or rewrites lives here, because they all go invalid when it ends:
/// a command buffer cannot be recorded while a previous recording runs, a staging mapping cannot
/// be written while a copy from it is in flight, and a descriptor pool cannot be reset while a
/// dispatch bound to its sets executes.
struct Slot {
    /// Set from acquisition until a wait has confirmed this slot's fence.
    claimed: bool,
    /// Retained so `Drop` can destroy it: a command pool must outlive its buffers.
    command_pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    /// Mapped host memory this slot's uploads and downloads are staged through.
    staging: Staging,
    /// Descriptor sets for this slot's submission, reset when the slot is acquired.
    ///
    /// # Invariant
    /// A set is read when the submission executes, so one set cannot serve two dispatches and the
    /// pool cannot be reset between them — a reset frees sets an earlier dispatch is bound to,
    /// which is undefined rather than slow. The pool is per slot because "reset at the submission
    /// boundary" stops being a boundary once two are in flight.
    descriptor_pool: vk::DescriptorPool,
}

/// The pool: the slots, and where the next acquisition looks.
struct Slots {
    entries: Vec<Slot>,
    /// Where [`GpuContext::acquire`] looks next: round-robin, so no slot is left behind.
    cursor: usize,
}

impl Slots {
    fn entry(&self, token: Token) -> Result<&Slot, RunError> {
        self.entries
            .get(token.0 as usize)
            .ok_or(RunError::UnknownSlot { slot: token.0 })
    }
}

/// A submitted-but-not-yet-waited-for slot.
///
/// # Invariant
/// It carries nothing but the slot's number, and the wait consumes it: waiting the same
/// submission twice would prove the wrong thing, because by then the slot could be running someone
/// else's submission. Taking the token by value makes the second wait inexpressible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Token(u32);

/// How many freed buffers of one size are kept for reuse.
const RECYCLED_PER_SIZE: usize = 4;

/// The bytes one `Int` element occupies, which is also a transfer's minimum chunk size.
const SCALAR_BYTES: vk::DeviceSize = 8;

/// The bytes one workgroup count occupies: a `u32`, whatever the element class is.
const COUNT_BYTES: vk::DeviceSize = 4;

/// The elements in a dispatch command block: three `u32` counts in sixteen bytes.
///
/// # Invariant
/// Two elements rather than one and a half, because every buffer here is a whole number of
/// elements and a block that size costs the pool nothing to recycle.
const SCALAR_WORDS: usize = 2;

/// The descriptor set and pipeline layouts for a run, keyed by its buffers and its
/// push-constant block.
#[derive(Clone, Copy)]
struct Layouts {
    set: vk::DescriptorSetLayout,
    pipeline: vk::PipelineLayout,
}

/// The most storage-buffer bindings one dispatch may declare.
const MAX_DESCRIPTOR_BINDINGS: usize = 32;

/// The most dispatches one submission may record.
///
/// # Invariant
/// A second descriptor set per dispatch, never a shared one: the pool is allocated once for this
/// many sets, so the limit is the backend's rather than the device's. A submission wanting more is
/// refused by name rather than quietly split.
const MAX_DISPATCHES_PER_SUBMISSION: usize = 64;

impl GpuContext {
    /// Find a device and create a compute context on it, with the default pool.
    pub fn new() -> Result<Self, RunError> {
        Self::with_config(GpuConfig::default())
    }

    /// [`Self::new`], with a chosen pool depth.
    pub fn with_config(config: GpuConfig) -> Result<Self, RunError> {
        if config.slot_depth == 0 {
            return Err(RunError::NoSlots {
                wanted: config.slot_depth,
            });
        }
        let entry = unsafe { ash::Entry::load() }.map_err(|detail| RunError::NoDevice {
            detail: detail.to_string(),
        })?;
        // No layers and no extensions: a compute run needs neither.
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

        let mut chosen: Option<(vk::PhysicalDevice, vk::PhysicalDeviceProperties, u32, bool)> =
            None;
        let seen = devices.len();
        for physical in devices {
            let properties = unsafe { instance.get_physical_device_properties(physical) };
            let features = unsafe { instance.get_physical_device_features(physical) };
            // `shaderInt64` is recorded, not required: a float fragment's module has
            // no 64-bit integer in it.
            let supports_int64 = features.shader_int64 != 0;
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
                Some((_, current, _, current_int64)) => {
                    let current_discrete =
                        current.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
                    match (discrete, current_discrete) {
                        // A discrete device wins outright; among equals the lowest index.
                        (true, false) => true,
                        (false, true) => false,
                        _ => supports_int64 && !current_int64,
                    }
                }
            };
            if better {
                chosen = Some((physical, properties, family as u32, supports_int64));
            }
        }

        let Some((physical, properties, family, shader_int64)) = chosen else {
            return Err(RunError::NoDevice {
                detail: format!("none of the {seen} device(s) offers a compute queue"),
            });
        };
        let name = device_name(&properties);
        let push_constant_bytes = properties.limits.max_push_constants_size;
        let facts = device_facts(&entry, &instance, physical, &properties, shader_int64);

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

        // One slot per permitted in-flight submission, built before the context
        // exists because they need a borrow of the device.

        // A failure part-way destroys the slots already built: their pools are
        // device objects.
        let mut built: Vec<Slot> = Vec::with_capacity(config.slot_depth);
        for _ in 0..config.slot_depth {
            match Slot::new(&device, family) {
                Ok(slot) => built.push(slot),
                Err(error) => {
                    for mut slot in built {
                        slot.destroy(&device);
                    }
                    return Err(error);
                }
            }
        }

        Ok(GpuContext {
            entry,
            instance,
            device,
            queue,
            physical,
            name,
            shader_int64,
            push_constant_bytes,
            facts,
            pipelines: Mutex::new(HashMap::new()),
            resident: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            slots: Mutex::new(Slots {
                entries: built,
                cursor: 0,
            }),
            layouts: Mutex::new(HashMap::new()),
            recycled: Mutex::new(HashMap::new()),
            submissions: AtomicU64::new(0),
            waits: AtomicU64::new(0),
        })
    }

    /// The device this context dispatches on.
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// How many submissions this context may have in flight at once.
    pub fn slot_depth(&self) -> usize {
        self.slots.lock().unwrap().entries.len()
    }

    /// How many command buffers this context has handed to its queue.
    pub fn submissions(&self) -> u64 {
        self.submissions.load(AtomicOrdering::Relaxed)
    }

    /// How many fences this context has waited on.
    pub fn fence_waits(&self) -> u64 {
        self.waits.load(AtomicOrdering::Relaxed)
    }

    /// Take a slot to record one submission into, and the token that will wait for it.
    ///
    /// # Invariant
    /// The lock is held for the segment's whole life, which makes "two threads never record into
    /// one command buffer" a borrow-checked fact rather than a convention. A caller that records
    /// and drops the segment gets its slot straight back; one that submits keeps it until
    /// [`Self::sync`] has seen its fence.
    fn acquire(&self) -> Result<Segment<'_>, RunError> {
        let mut slots = self.slots.lock().unwrap();
        let index = slots.cursor;
        slots.cursor = (index + 1) % slots.entries.len();
        if slots.entries[index].claimed {
            // The slot is running someone else's submission, so its fence is the
            // only thing that makes it reusable.
            self.wait_on(&slots.entries[index])?;
        }
        // Safe exactly here: the fence has signalled or was never claimed, so
        // nothing is reading the sets this frees.
        check("descriptor pool reset", unsafe {
            self.device.reset_descriptor_pool(
                slots.entries[index].descriptor_pool,
                vk::DescriptorPoolResetFlags::empty(),
            )
        })?;
        slots.entries[index].claimed = true;
        Ok(Segment {
            context: self,
            slots,
            index,
            in_flight: false,
        })
    }

    /// Wait for a submitted slot, and release it for reuse.
    ///
    /// # Invariant
    /// The token is consumed, so one submission is waited for exactly once. It need not be called
    /// on the submitting thread: the segment was consumed by `submit` so the host could go and do
    /// something else.
    fn sync(&self, token: Token) -> Result<(), RunError> {
        let mut slots = self.slots.lock().unwrap();
        let slot = slots.entry(token)?;
        self.wait_on(slot)?;
        slots.entries[token.0 as usize].claimed = false;
        Ok(())
    }

    /// Run a launch set's root over the index range `[0, count)`.
    ///
    /// # Invariant
    /// One [`ResidentId`] per declared output, owned until released, and nothing is copied back:
    /// the results are where the shader ran, so a run whose outputs are another run's inputs pays
    /// for no transfer. It is [`Self::submit`] then a wait, sharing [`Self::stage_run`].
    pub fn run(
        &self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, RunError> {
        self.staged_and_waited(launch, inputs, Extent::Host(count))
    }

    /// [`Self::run`] over a number only the device holds, with no read-back in between.
    ///
    /// # Invariant
    /// The count is never on the host: the workgroup count is handed to the queue as a buffer
    /// offset, and the outputs are allocated for the extent's `capacity` rather than for a
    /// number nobody has. What the count *is* decides only how much of those buffers the shader
    /// reaches.
    pub fn run_over_device_extent(
        &self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        extent: DeviceExtent,
    ) -> Result<Vec<ResidentId>, RunError> {
        self.staged_and_waited(launch, inputs, self.held(extent)?)
    }

    /// Stage one run at `extent`, wait for it, and hand back the ids.
    ///
    /// # Invariant
    /// The upload targets are recycled after the wait rather than in a `finally`, so a failed
    /// wait does not leak them.
    fn staged_and_waited(
        &self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        extent: Extent,
    ) -> Result<Vec<ResidentId>, RunError> {
        let staged = self.stage_run(launch, inputs, extent)?;
        let waited = self.sync(staged.token);
        for buffer in staged.scratch {
            self.recycle(buffer);
        }
        waited?;
        Ok(staged.ids)
    }

    /// [`Self::run`], handed back before the device is done.
    ///
    /// # Invariant
    /// The ids name buffers the shader has not been guaranteed to have written: they are for
    /// handing to the next run as [`BufferSlot::Resident`], and a run that needs their values waits
    /// first.
    pub fn submit(
        &self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<GpuPending<'_>, RunError> {
        let staged = self.stage_run(launch, inputs, Extent::Host(count))?;
        Ok(GpuPending {
            context: self,
            token: staged.token,
            scratch: Some(staged.scratch),
            ids: staged.ids,
            finished: false,
        })
    }

    /// Record one run and hand it to the queue, leaving the token, the ids and the
    /// scratch to the caller.
    ///
    /// # Invariant
    /// The scratch travels in the return value because those buffers are not resident: this
    /// submission is the only thing keeping them alive, and the pool must not see them until
    /// someone has waited.
    fn stage_run(
        &self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        extent: Extent,
    ) -> Result<Staged, RunError> {
        // The slot is taken before anything is staged into it: the staging a run writes
        // is the slot's own.
        let mut segment = self.acquire()?;
        let mut scratch = ScratchGuard {
            context: self,
            buffers: Vec::new(),
        };
        let bound = self.bound(inputs)?;
        let staging_bytes = self.staged_bytes(
            launch,
            extent.allocated(),
            bound
                .iter()
                .enumerate()
                .filter_map(|(index, input)| match input {
                    Bound::Host(data) => Some((index, *data)),
                    Bound::Resident(_) => None,
                }),
        )?;
        // The reservation is the whole run's, made before the first byte is staged.
        segment.reserve(self, staging_bytes)?;
        let (dispatched, _) =
            self.dispatch_into(&mut segment, &mut scratch, 0, launch, &bound, extent)?;

        segment.begin()?;
        segment.record(
            dispatched.pipeline,
            &dispatched.descriptors,
            &dispatched.uploads,
            dispatched.workgroups,
            &dispatched.scalars,
        )?;
        // Nothing below can fail, so the bookkeeping comes after the submit: a failed
        // submit means nothing reached the queue.
        let token = segment.submit()?;
        let ids = self.issue(dispatched.outputs);
        // The guard is now empty; what is left is the host upload targets.
        let scratch = std::mem::take(&mut scratch.buffers);
        Ok(Staged {
            token,
            ids,
            scratch,
        })
    }

    /// The buffers a caller's slots name, as this backend's own form of them.
    fn bound<'a>(&self, inputs: &[BufferSlot<'a>]) -> Result<Vec<Bound<'a>>, RunError> {
        inputs
            .iter()
            .map(|input| match input {
                BufferSlot::Host(data) => Ok(Bound::Host(data)),
                BufferSlot::Resident(id) => self.resident_buffer(*id).map(Bound::Resident),
            })
            .collect()
    }

    /// A [`DeviceExtent`] as this backend's own extent: the buffer behind the id.
    fn held(&self, extent: DeviceExtent) -> Result<Extent, RunError> {
        Ok(Extent::Device {
            buffer: self.resident_buffer(extent.count)?,
            index: extent.index,
            capacity: extent.capacity,
        })
    }

    /// Hand out one [`ResidentId`] per buffer, and record what it names.
    fn issue(&self, buffers: Vec<DeviceBuffer>) -> Vec<ResidentId> {
        let mut resident = self.resident.lock().unwrap();
        buffers
            .into_iter()
            .map(|buffer| {
                let id = ResidentId(self.next_id.fetch_add(1, AtomicOrdering::Relaxed));
                resident.insert(id, buffer);
                id
            })
            .collect()
    }

    /// The staging one dispatch's host inputs need, and whether they are long enough.
    ///
    /// # Invariant
    /// The sum of each host input's own `len × byte_width()`, `len` being that slice's own
    /// length in elements — **the width is the buffer's own class**, not one module-wide
    /// answer, and a device input stages nothing and has no length to check. Asked for before
    /// anything is recorded, because that is when the reservation is made, and it is the
    /// width the upload copies.
    fn staged_bytes<'a>(
        &self,
        launch: &LaunchSet<'_>,
        count: usize,
        hosts: impl Iterator<Item = (usize, &'a [u8])>,
    ) -> Result<vk::DeviceSize, RunError> {
        let fragment = launch.root();
        let class = spirv::module_class(fragment).map_err(RunError::Emit)?;
        let mut bytes: vk::DeviceSize = 0;
        for (index, data) in hosts {
            let width = spirv::buffer_class_of(fragment, index, class).byte_width();
            let len = own_elements(data, width);
            if len < count {
                return Err(RunError::InputShorterThanCount {
                    buffer: index,
                    len,
                    count,
                });
            }
            bytes += (len * width) as vk::DeviceSize;
        }
        Ok(bytes)
    }

    /// The staging a whole batch's host inputs need, before any of it is recorded.
    fn batch_staging(&self, batch: &[Dispatch<'_>]) -> Result<vk::DeviceSize, RunError> {
        let mut bytes: vk::DeviceSize = 0;
        for dispatch in batch {
            bytes += self.staged_bytes(
                dispatch.launch,
                dispatch.extent.allocated(),
                dispatch
                    .inputs
                    .iter()
                    .enumerate()
                    .filter_map(|(index, input)| match input {
                        BatchInput::Host(data) => Some((index, *data)),
                        _ => None,
                    }),
            )?;
        }
        Ok(bytes)
    }

    /// [`Self::dispatch_into`], with the staging offset it stopped at.
    ///
    /// # Invariant
    /// Staging is reserved by the caller **before** anything is recorded, and the
    /// offset is the sum of every block staged before this dispatch. Growing a slot's
    /// staging destroys the buffer an earlier recorded transfer names, so a submission
    /// reserves once. Upload targets and indirect blocks go to `scratch`, so a run that
    /// never completes recycles them; the outputs are the caller's.
    fn dispatch_into(
        &self,
        segment: &mut Segment<'_>,
        scratch: &mut ScratchGuard<'_>,
        at: vk::DeviceSize,
        launch: &LaunchSet<'_>,
        inputs: &[Bound<'_>],
        extent: Extent,
    ) -> Result<(Dispatched, vk::DeviceSize), RunError> {
        let fragment = launch.root();
        let class = spirv::module_class(fragment).map_err(RunError::Emit)?;
        // The number the extent leaf is pushed with; a device-held extent has none.
        let (leaf, held) = match extent {
            Extent::Host(count) => (count, None),
            Extent::Device {
                buffer,
                index,
                capacity,
            } => (0, Some((buffer, index, capacity))),
        };
        let count = match held {
            None => leaf,
            Some((_, _, capacity)) => capacity,
        };
        // The block the entry point reads the launch's leaves out of.
        let scalars = self.scalar_block(launch, class, leaf)?;
        let binding = Binding {
            inputs: inputs.len(),
            outputs: fragment.outputs,
        };
        let pipeline = self.pipeline(launch, binding, scalars.len() as u32)?;

        // Round up so the last workgroup's surplus lanes address padding rather
        // than memory past the end; see the module docs.
        let workgroups = launch_workgroups(launch, count)?;
        let padded = workgroups * launch.root().target.lane_width.lanes() as usize;
        if padded == 0 {
            // A zero-element run would ask Vulkan for a zero-sized buffer.
            return Err(RunError::EmptyRun);
        }

        let mut descriptors: Vec<vk::DescriptorBufferInfo> = Vec::with_capacity(binding.total());
        let mut uploads: Vec<Transfer> = Vec::with_capacity(inputs.len());
        // A block begins where the previous one ended: `offset` is the sum of the blocks before it.
        let mut offset: vk::DeviceSize = at;
        for (index, input) in inputs.iter().enumerate() {
            let buffer = match input {
                // Used where it lies: a chained run costs one upload, not two.
                Bound::Resident(buffer) => *buffer,
                Bound::Host(data) => {
                    let buffer_class = spirv::buffer_class_of(fragment, index, class);
                    let element = buffer_class.byte_width() as vk::DeviceSize;
                    // The buffer's own length, not the count: above it there are real elements too.
                    let len = own_elements(data, buffer_class.byte_width()) as vk::DeviceSize;
                    let data_bytes = len * element;
                    // The allocation holds every one of them and still reaches a whole workgroup.
                    let allocated = (padded as vk::DeviceSize).max(len);
                    let padded_bytes = allocated * element;
                    let buffer = self.allocate(allocated as usize, buffer_class)?;
                    // SAFETY: `offset` plus this block is inside the staging `reserve` sized at this
                    // width, and the claimed slot is idle.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            data.as_ptr(),
                            segment.staging().at(offset),
                            data_bytes as usize,
                        );
                    }
                    uploads.push(Transfer {
                        src_offset: offset,
                        dst: buffer.handle,
                        bytes: data_bytes,
                        tail: padded_bytes - data_bytes,
                    });
                    offset += data_bytes;
                    // A host upload is a scratch target: nothing refers to it once
                    // the run is over, so it does not become resident.
                    scratch.buffers.push(buffer);
                    buffer
                }
            };
            descriptors.push(buffer.descriptor());
        }

        // **Nothing is written into the outputs here**: every invocation reaches its write.
        let output_classes: Vec<ScalarClass> = (0..binding.outputs)
            .map(|ordinal| spirv::buffer_class_of(fragment, binding.inputs + ordinal, class))
            .collect();
        // The command block is scratch like an upload target: this submission owns it.
        let indirect = match held {
            None => None,
            Some((buffer, index, _)) => {
                let (command, block) = self.indirect_extent(launch, buffer, index)?;
                scratch.buffers.push(block);
                Some(command)
            }
        };
        // Each output is allocated and recorded at its own class, the width a fetch reads it at.
        let mut outputs = Vec::with_capacity(binding.outputs);
        for buffer_class in &output_classes {
            let buffer = self.allocate(padded, *buffer_class)?;
            descriptors.push(buffer.descriptor());
            outputs.push(DeviceBuffer {
                handle: buffer.handle,
                memory: buffer.memory,
                padded,
                // The class this output's element type was emitted for, so a fetch reads
                // it back at the width it was written at.
                class: *buffer_class,
            });
        }

        Ok((
            Dispatched {
                pipeline,
                descriptors,
                uploads,
                workgroups: match indirect {
                    None => Workgroups::Direct(workgroups as u32),
                    Some(indirect) => indirect,
                },
                scalars,
                outputs,
            },
            offset,
        ))
    }

    /// The buffers one batch dispatch reads, as this backend's own form of them.
    ///
    /// # Invariant
    /// A [`BatchInput::Produced`] is an output of an earlier dispatch **of this batch**, and
    /// the buffer bound is the one that dispatch was allocated: no [`ResidentId`] names it
    /// yet, and none is needed to bind one.
    fn bind<'a>(
        &self,
        inputs: &[BatchInput<'a>],
        produced: &[Vec<DeviceBuffer>],
    ) -> Result<Vec<Bound<'a>>, RunError> {
        inputs
            .iter()
            .map(|input| match input {
                BatchInput::Host(data) => Ok(Bound::Host(data)),
                BatchInput::Resident(id) => self.resident_buffer(*id).map(Bound::Resident),
                BatchInput::Produced { dispatch, ordinal } => produced
                    .get(*dispatch)
                    .and_then(|outputs| outputs.get(*ordinal))
                    .copied()
                    .map(Bound::Resident)
                    .ok_or(RunError::UnknownBatchOutput {
                        dispatch: *dispatch,
                        ordinal: *ordinal,
                    }),
            })
            .collect()
    }

    /// A batch dispatch's extent as this backend's own, with the buffer its count is in.
    fn held_batch(
        &self,
        extent: BatchExtent,
        produced: &[Vec<DeviceBuffer>],
    ) -> Result<Extent, RunError> {
        match extent {
            BatchExtent::Host(count) => Ok(Extent::Host(count)),
            BatchExtent::Resident {
                count,
                index,
                capacity,
            } => self.held(DeviceExtent {
                count,
                index,
                capacity,
            }),
            BatchExtent::Produced {
                dispatch,
                ordinal,
                index,
                capacity,
            } => Ok(Extent::Device {
                buffer: produced
                    .get(dispatch)
                    .and_then(|outputs| outputs.get(ordinal))
                    .copied()
                    .ok_or(RunError::UnknownBatchOutput { dispatch, ordinal })?,
                index,
                capacity,
            }),
        }
    }

    /// Record a graph's dispatches into one command buffer, submit once, wait once.
    ///
    /// # Invariant
    /// The framing is [`Self::run_chain`]'s: one slot, one `begin`, one submit, one wait,
    /// so a graph pays one fence whatever its length. What a graph adds is that a dispatch
    /// may read *any* earlier dispatch's output, and the trailing barrier each recording
    /// already carries is what orders it. Ids are issued **after** the wait: none names a
    /// buffer the device is still writing.
    pub fn run_batch(&self, batch: &[Dispatch<'_>]) -> Result<Vec<Vec<ResidentId>>, RunError> {
        if batch.is_empty() || batch.len() > MAX_DISPATCHES_PER_SUBMISSION {
            return Err(RunError::TooManyDispatches {
                wanted: batch.len(),
                max: MAX_DISPATCHES_PER_SUBMISSION,
            });
        }
        // One slot, and one staging for the whole batch, reserved up
        // front: a dispatch's block begins where the last one ended.
        let mut segment = self.acquire()?;
        segment.reserve(self, self.batch_staging(batch)?)?;
        let mut scratch = ScratchGuard {
            context: self,
            buffers: Vec::new(),
        };
        let mut produced: Vec<Vec<DeviceBuffer>> = Vec::with_capacity(batch.len());

        // Nothing has reached the queue, so every buffer recorded so far
        // is this submission's to give back.
        if let Err(error) = self.record_batch(&mut segment, &mut scratch, batch, &mut produced) {
            for outputs in produced.drain(..) {
                for buffer in outputs {
                    self.recycle(buffer);
                }
            }
            return Err(error);
        }

        segment.submit_and_wait()?;
        // The guard recycles what the wait proved nobody reads; the
        // outputs go to the host as ids.
        Ok(produced
            .into_iter()
            .map(|buffers| self.issue(buffers))
            .collect())
    }

    /// Record every dispatch of a batch into the segment's open command buffer.
    ///
    /// # Invariant
    /// In order, and each dispatch binds the outputs of the ones before it: the barrier a
    /// recording ends with is what orders them, so the only thing this loop owes the next
    /// turn is that the buffer exists. Nothing is submitted here.
    fn record_batch(
        &self,
        segment: &mut Segment<'_>,
        scratch: &mut ScratchGuard<'_>,
        batch: &[Dispatch<'_>],
        produced: &mut Vec<Vec<DeviceBuffer>>,
    ) -> Result<(), RunError> {
        let mut at: vk::DeviceSize = 0;

        // One reset for the whole submission and none inside it: a reset frees sets an
        // earlier dispatch is still bound to.
        segment.begin()?;
        for dispatch in batch {
            let inputs = self.bind(dispatch.inputs, produced)?;
            let extent = self.held_batch(dispatch.extent, produced)?;
            let (dispatched, next) =
                self.dispatch_into(segment, scratch, at, dispatch.launch, &inputs, extent)?;
            segment.record(
                dispatched.pipeline,
                &dispatched.descriptors,
                &dispatched.uploads,
                dispatched.workgroups,
                &dispatched.scalars,
            )?;
            at = next;
            produced.push(dispatched.outputs);
        }
        Ok(())
    }

    /// Record `links` dispatches into one command buffer, submit once, wait once, and hand
    /// back the last link's output.
    ///
    /// # Invariant
    /// Each link consumes the previous link's result over one fragment and one count, with the
    /// first link's input uploaded once — a linear chain, which a graph can fan out beyond. What it
    /// saves is bounded and measured: one submit and one fence wait instead of `links` of each.
    pub fn run_chain(
        &self,
        launch: &LaunchSet<'_>,
        input: &[u8],
        count: usize,
        links: usize,
    ) -> Result<ResidentId, RunError> {
        let fragment = launch.root();
        if links == 0 || links > MAX_DISPATCHES_PER_SUBMISSION {
            return Err(RunError::TooManyDispatches {
                wanted: links,
                max: MAX_DISPATCHES_PER_SUBMISSION,
            });
        }
        // A chain hands one link's output to the next link's input slot, so it needs one buffer in
        // and one out.
        if fragment.param_shape.flat_arity() != 2 || fragment.inputs != 1 || fragment.outputs != 1 {
            return Err(RunError::ChainNotLinear {
                inputs: fragment.inputs,
                outputs: fragment.outputs,
            });
        }
        // Slot 0 is the uploaded input and slot 1 the resident answer; each is its own class.
        let class = spirv::module_class(fragment).map_err(RunError::Emit)?;
        let input_class = spirv::buffer_class_of(fragment, 0, class);
        let output_class = spirv::buffer_class_of(fragment, 1, class);
        // A link reads the previous link's output through its input slot, so both ends must be one class.
        if input_class != output_class {
            return Err(RunError::ChainCrossesClasses {
                input: input_class,
                output: output_class,
            });
        }
        let element = input_class.byte_width();
        let len = own_elements(input, element);
        if len < count {
            return Err(RunError::InputShorterThanCount {
                buffer: 0,
                len,
                count,
            });
        }

        let scalars = self.scalar_block(
            launch,
            spirv::module_class(fragment).map_err(RunError::Emit)?,
            count,
        )?;
        let pipeline = self.pipeline(
            launch,
            Binding {
                inputs: 1,
                outputs: 1,
            },
            scalars.len() as u32,
        )?;
        let workgroups = launch_workgroups(launch, count)?;
        let padded = workgroups * launch.root().target.lane_width.lanes() as usize;
        if padded == 0 {
            return Err(RunError::EmptyRun);
        }
        let element = element as vk::DeviceSize;
        let data_bytes = len as vk::DeviceSize * element;
        // The allocation holds every element the host holds and still reaches a whole workgroup.
        let allocated = padded.max(len);
        let padded_bytes = allocated as vk::DeviceSize * element;

        // One slot, its staging sized for the whole chain up front.
        let mut segment = self.acquire()?;
        segment.reserve(self, data_bytes)?;

        let mut scratch = ScratchGuard {
            context: self,
            buffers: Vec::with_capacity(1),
        };
        let source = self.allocate(allocated, input_class)?;
        scratch.buffers.push(source);
        // SAFETY: `reserve` sized and mapped this slot's staging for `len` elements.
        unsafe {
            std::ptr::copy_nonoverlapping(
                input.as_ptr(),
                segment.staging().at(0),
                data_bytes as usize,
            );
        }
        let upload = Transfer {
            src_offset: 0,
            dst: source.handle,
            bytes: data_bytes,
            tail: padded_bytes - data_bytes,
        };

        // Two output buffers, ping-ponged, not one per link: a link reads what the one
        // before it wrote.

        // One per link is slower, not merely bigger: the pool is empty at every call.

        // It holds because this is a linear chain; a fan-out is the graph's question.
        let first = self.allocate(padded, output_class)?;
        let second = self.allocate(padded, output_class)?;

        // One reset for the whole submission and none inside it: a reset frees sets an
        // earlier dispatch is still bound to.
        segment.begin()?;

        let mut current = source;
        let (mut into, mut onto) = (first, second);
        for step in 0..links {
            let descriptors = [current.descriptor(), into.descriptor()];
            // Only the first link uploads; later links read a buffer an earlier link's
            // shader wrote.
            let uploads: &[Transfer] = if step == 0 {
                std::slice::from_ref(&upload)
            } else {
                &[]
            };
            segment.record(
                pipeline,
                &descriptors,
                uploads,
                Workgroups::Direct(workgroups as u32),
                &scalars,
            )?;
            current = into;
            std::mem::swap(&mut into, &mut onto);
        }

        segment.submit_and_wait()?;

        // The last link's output becomes resident; two buffers go back to the pool.
        if current.handle == first.handle {
            self.recycle(second);
        } else {
            self.recycle(first);
        }
        let id = ResidentId(self.next_id.fetch_add(1, AtomicOrdering::Relaxed));
        self.resident.lock().unwrap().insert(
            id,
            DeviceBuffer {
                handle: current.handle,
                memory: current.memory,
                padded,
                class: output_class,
            },
        );
        Ok(id)
    }

    /// The dispatch command that takes its workgroup count from a device buffer.
    ///
    /// # Invariant
    /// The count is the low `u32` of the named element — an `Int` is a little-endian `i64`, and
    /// its high half is zero for any count a device could schedule. A body reading the extent
    /// leaf is refused rather than pushed a placeholder: that leaf is the entry block's first
    /// parameter, and the number this path reads is not one the host has.
    fn indirect_extent(
        &self,
        launch: &LaunchSet<'_>,
        // The buffer the count is in, named not by id: a batch has none yet.
        buffer: DeviceBuffer,
        index: usize,
    ) -> Result<(Workgroups, DeviceBuffer), RunError> {
        let body = &launch.root().body;
        if let Some(leaf) = body
            .blocks
            .get(body.entry)
            .and_then(|entry| entry.params.first())
            && body.operands().contains(leaf)
        {
            return Err(RunError::ExtentLeafUnfixed);
        }
        let len = buffer.padded;
        if buffer.class != ScalarClass::Int {
            return Err(RunError::IndirectExtentNotIntegers { len });
        }
        if index >= len {
            return Err(RunError::IndirectExtentOutOfReach { index, len });
        }
        let block = self.allocate(SCALAR_WORDS, ScalarClass::Int)?;
        let command = Workgroups::Indirect {
            block: block.handle,
            source: buffer.handle,
            offset: index as vk::DeviceSize * SCALAR_BYTES,
        };
        Ok((command, block))
    }

    /// The push-constant bytes this launch hands the shader.
    ///
    /// # Invariant
    /// **One layout, from one rule**: the bytes sit at the offsets [`spirv::push_layout`]
    /// gave the module's members, each at the width the emitter gave that class.
    ///
    /// Member `0` is the extent, whose value is the `count` this run dispatches over: the
    /// ABI's first scalar **is** that count, so a caller stating it again would be giving
    /// this function two answers to one question.
    fn scalar_block(
        &self,
        launch: &LaunchSet<'_>,
        class: ScalarClass,
        count: usize,
    ) -> Result<Vec<u8>, RunError> {
        let fragment = launch.root();
        let leaves = launch.leaves();
        let declared = fragment.runtime_scalars();
        if leaves.len() != declared {
            return Err(RunError::ScalarLeavesMismatch {
                declared,
                given: leaves.len(),
            });
        }
        let classes = fragment.param_shape.leaf_classes();
        let (offsets, size) = spirv::push_layout(class, &classes[..fragment.scalar_leaves()]);
        if size > self.push_constant_bytes {
            return Err(RunError::PushConstantsTooLarge {
                bytes: size,
                max: self.push_constant_bytes,
            });
        }
        let mut bytes = vec![0u8; size as usize];
        // The extent is a count, at the class the parameter declared.
        let extent = classes.first().copied().unwrap_or(ScalarClass::Int);
        if extent != ScalarClass::Int {
            return Err(RunError::ExtentNotAnInteger { class: extent });
        }
        write_leaf(&mut bytes, offsets[0], class, extent, count as i64);
        for (leaf, offset) in leaves.iter().zip(offsets[1..].iter()) {
            write_leaf(&mut bytes, *offset, class, leaf.class, leaf.bits);
        }
        Ok(bytes)
    }

    /// The device-local buffer behind a resident id.
    fn resident_buffer(&self, id: ResidentId) -> Result<DeviceBuffer, RunError> {
        self.resident
            .lock()
            .unwrap()
            .get(&id)
            .copied()
            .ok_or(RunError::UnknownResident { id: id.0 })
    }

    /// A fresh device-local buffer for `padded` elements of `class`, recycled when one of
    /// that size and class is free.
    ///
    /// # Invariant
    /// Its contents are undefined until something writes them, and every consumer writes all of
    /// it. The class is part of what a buffer is — its byte size and the width a fetch reads it
    /// back at — so the pool is keyed on both.
    fn allocate(&self, padded: usize, class: ScalarClass) -> Result<DeviceBuffer, RunError> {
        if let Some(buffer) = self
            .recycled
            .lock()
            .unwrap()
            .get_mut(&(padded, class))
            .and_then(Vec::pop)
        {
            return Ok(buffer);
        }
        let device = &self.device;
        let bytes = DeviceBuffer::bytes(padded, class);
        let handle = check("buffer creation", unsafe {
            device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(bytes)
                    .usage(
                        vk::BufferUsageFlags::STORAGE_BUFFER
                            | vk::BufferUsageFlags::TRANSFER_DST
                            | vk::BufferUsageFlags::TRANSFER_SRC
                            | vk::BufferUsageFlags::INDIRECT_BUFFER,
                    )
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        })?;
        let requirements = unsafe { device.get_buffer_memory_requirements(handle) };
        let index = memory_type(
            &self.instance,
            self.physical,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let memory = match check("memory allocation", unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(index),
                None,
            )
        }) {
            Ok(memory) => memory,
            Err(error) => {
                // The buffer has no memory bound to it yet, so the allocation
                // failed rather than the binding: only the handle is live.
                unsafe { device.destroy_buffer(handle, None) };
                return Err(error);
            }
        };
        if let Err(error) = check("buffer binding", unsafe {
            device.bind_buffer_memory(handle, memory, 0)
        }) {
            unsafe {
                device.destroy_buffer(handle, None);
                device.free_memory(memory, None);
            }
            return Err(error);
        }
        Ok(DeviceBuffer {
            handle,
            memory,
            padded,
            class,
        })
    }

    /// Hand a device buffer back for reuse, or free it if the pool for its size
    /// and class is already full.
    fn recycle(&self, buffer: DeviceBuffer) {
        let mut recycled = self.recycled.lock().unwrap();
        let pool = recycled.entry((buffer.padded, buffer.class)).or_default();
        if pool.len() < RECYCLED_PER_SIZE {
            pool.push(buffer);
        } else {
            // Past the cap the memory is the caller's again, so it goes back to the driver.
            drop(recycled);
            buffer.destroy(&self.device);
        }
    }

    /// The first `count` elements of a resident buffer, as host data.
    ///
    /// # Invariant
    /// A submit and a wait of its own, which is the point: a run whose results nobody asks for
    /// never pays for moving them. The class is the buffer's own, read off the record the run left,
    /// so a float buffer is decoded as `f32`s — a fetch has no fragment to consult.
    pub fn fetch(&self, id: ResidentId, count: usize) -> Result<ScalarData, RunError> {
        let buffer = self.resident_buffer(id)?;
        if count > buffer.padded {
            return Err(RunError::FetchLongerThanBuffer {
                len: buffer.padded,
                count,
            });
        }
        let bytes = (count * buffer.class.byte_width()) as vk::DeviceSize;
        let device = &self.device;

        // A fetch takes a slot like anything else: a submission needs a command buffer,
        // a fence and staging of its own.
        let mut segment = self.acquire()?;
        segment.reserve(self, bytes)?;

        let source = buffer.handle;
        segment.begin()?;
        let target = segment.staging().handle;
        let command = segment.command();
        unsafe {
            device.cmd_copy_buffer(
                command,
                source,
                target,
                &[vk::BufferCopy {
                    src_offset: 0,
                    dst_offset: 0,
                    size: bytes,
                }],
            );
            // The copy wrote staging on the device; the read below is the host's.
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::HOST_READ)],
                &[],
                &[],
            );
        }

        segment.submit_and_read_back(count, buffer.class)
    }

    /// Give a resident buffer's device memory back.
    ///
    /// # Invariant
    /// Idempotent: the host may hold a block-scoped view and release it more than once, and a
    /// second release is a no-op rather than a double free.
    pub fn release(&self, id: ResidentId) {
        let Some(buffer) = self.resident.lock().unwrap().remove(&id) else {
            return;
        };
        self.recycle(buffer);
    }

    /// Reset `slot`'s command buffer and open it for recording.
    ///
    /// # Invariant
    /// Split from the close so one submission can record more than one dispatch; a chain is still
    /// one submission and one fence, and only several in flight need a pool deeper than one.
    fn begin_recording(&self, slot: &Slot) -> Result<(), RunError> {
        let device = &self.device;
        check("command buffer reset", unsafe {
            device.reset_command_buffer(slot.command, vk::CommandBufferResetFlags::empty())
        })
        .and_then(|()| {
            check("command recording", unsafe {
                device.begin_command_buffer(slot.command, &vk::CommandBufferBeginInfo::default())
            })
        })
    }

    /// Close `slot`'s recording and hand it to the queue, without waiting.
    fn end_and_submit(&self, slot: &Slot) -> Result<(), RunError> {
        let device = &self.device;
        check("command buffer end", unsafe {
            device.end_command_buffer(slot.command)
        })
        .and_then(|()| {
            // The fence is created unsignalled and left signalled by the wait, so it has
            // to be reset or this returns immediately.
            check("fence reset", unsafe { device.reset_fences(&[slot.fence]) })?;
            check("queue submit", unsafe {
                device.queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&[slot.command])],
                    slot.fence,
                )
            })?;
            // After the hand-over: a submission the queue refused is not one.
            self.submissions.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(())
        })
    }

    /// Block until `slot`'s last submission has finished.
    ///
    /// # Invariant
    /// The thirty-second bound is a driver-error bound: a fence unsignalled that long is a device
    /// that stopped, and returning would hand back buffers it is still writing.
    fn wait_on(&self, slot: &Slot) -> Result<(), RunError> {
        check("fence wait", unsafe {
            self.device.wait_for_fences(
                &[slot.fence],
                true,
                std::time::Duration::from_secs(30).as_nanos() as u64,
            )
        })?;
        self.waits.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(())
    }

    /// Record one dispatch into the command buffer that is already open.
    ///
    /// # Invariant
    /// Each call takes its own descriptor set: a set is read when the submission executes, so one
    /// rewritten between two dispatches changes what the first sees. The uploads read `slot`'s own
    /// staging. A chain rests on the trailing barrier, whose destination scope names a shader as
    /// well as a transfer.
    fn record_dispatch(
        &self,
        slot: &Slot,
        pipeline: vk::Pipeline,
        descriptors: &[vk::DescriptorBufferInfo],
        uploads: &[Transfer],
        workgroups: Workgroups,
        scalars: &[u8],
    ) -> Result<(), RunError> {
        let total = descriptors.len();
        if total > MAX_DESCRIPTOR_BINDINGS {
            return Err(RunError::TooManyBindings {
                total,
                max: MAX_DESCRIPTOR_BINDINGS,
            });
        }
        let device = &self.device;
        let staging = slot.staging.handle;

        // A function of `total` and the block alone, built once per shape.
        let layouts = self.layouts(total, scalars.len() as u32)?;

        let set_layout = layouts.set;
        let mut set = vk::DescriptorSet::null();
        check("descriptor set allocation", unsafe {
            device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo {
                // The count defaults to zero, and a zero-count request
                // *succeeds* while allocating nothing — so it is stated.
                descriptor_set_count: 1,
                descriptor_pool: slot.descriptor_pool,
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

        let command = slot.command;
        unsafe {
            // These three exist only for the uploads; a later link's ordering is the
            // trailing barrier below.
            if !uploads.is_empty() {
                // The host wrote staging before this submit, so the copies are the
                // first reader: make that visible.
                device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::HOST,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[vk::MemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::HOST_WRITE)
                        .dst_access_mask(vk::AccessFlags::TRANSFER_READ)],
                    &[],
                    &[],
                );
                for transfer in uploads {
                    device.cmd_copy_buffer(
                        command,
                        staging,
                        transfer.dst,
                        &[vk::BufferCopy {
                            src_offset: transfer.src_offset,
                            dst_offset: 0,
                            size: transfer.bytes,
                        }],
                    );
                    // The padding past the buffer's own length is cleared on the device, so
                    // a surplus lane reads a defined `0`.
                    if transfer.tail > 0 {
                        device.cmd_fill_buffer(
                            command,
                            transfer.dst,
                            transfer.bytes,
                            transfer.tail,
                            0,
                        );
                    }
                }
                // The uploads and the tail fills land in buffers the shader reads, so
                // one barrier covers both.
                device.cmd_pipeline_barrier(
                    command,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[vk::MemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                        .dst_access_mask(vk::AccessFlags::SHADER_READ)],
                    &[],
                    &[],
                );
            }
            device.cmd_bind_pipeline(command, vk::PipelineBindPoint::COMPUTE, pipeline);
            device.cmd_bind_descriptor_sets(
                command,
                vk::PipelineBindPoint::COMPUTE,
                layouts.pipeline,
                0,
                &[set],
                &[],
            );
            // The launch's leaves, uniform across the workgroup: one push serves
            // every lane, and a run with none pushes nothing.
            if !scalars.is_empty() {
                device.cmd_push_constants(
                    command,
                    layouts.pipeline,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    scalars,
                );
            }
            match workgroups {
                Workgroups::Direct(groups) => device.cmd_dispatch(command, groups, 1, 1),
                Workgroups::Indirect {
                    block,
                    source,
                    offset,
                } => {
                    // `y` and `z` are one work each — a dispatch with none runs
                    // nothing — and `x` is copied out of the caller's buffer.
                    device.cmd_pipeline_barrier(
                        command,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::DependencyFlags::empty(),
                        &[vk::MemoryBarrier::default()
                            .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)],
                        &[],
                        &[],
                    );
                    device.cmd_fill_buffer(command, block, 4, 8, 1);
                    device.cmd_copy_buffer(
                        command,
                        source,
                        block,
                        &[vk::BufferCopy {
                            src_offset: offset,
                            dst_offset: 0,
                            size: COUNT_BYTES,
                        }],
                    );
                    device.cmd_pipeline_barrier(
                        command,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::PipelineStageFlags::DRAW_INDIRECT,
                        vk::DependencyFlags::empty(),
                        &[vk::MemoryBarrier::default()
                            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                            .dst_access_mask(vk::AccessFlags::INDIRECT_COMMAND_READ)],
                        &[],
                        &[],
                    );
                    device.cmd_dispatch_indirect(command, block, 0)
                }
            }
            // The trailing barrier names every reader of these results, including a later
            // dispatch in the same command buffer.
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::SHADER_READ)
                    .dst_access_mask(
                        vk::AccessFlags::SHADER_READ
                            | vk::AccessFlags::SHADER_WRITE
                            | vk::AccessFlags::TRANSFER_READ,
                    )],
                &[],
                &[],
            );
        }
        Ok(())
    }

    /// The descriptor set and pipeline layouts for a run binding `total` buffers.
    fn layouts(&self, total: usize, push: u32) -> Result<Layouts, RunError> {
        if let Some(layouts) = self.layouts.lock().unwrap().get(&(total, push)) {
            return Ok(*layouts);
        }
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
        let set = check("descriptor set layout", unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        })?;
        // A zero-length range is not a range: the block is declared only when the launch
        // has leaves to push.
        let ranges = [vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::COMPUTE,
            offset: 0,
            size: push,
        }];
        let pipeline = match check("pipeline layout", unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[set])
                    .push_constant_ranges(if push == 0 { &[] } else { &ranges }),
                None,
            )
        }) {
            Ok(layout) => layout,
            Err(error) => {
                unsafe { device.destroy_descriptor_set_layout(set, None) };
                return Err(error);
            }
        };
        let layouts = Layouts { set, pipeline };
        self.layouts.lock().unwrap().insert((total, push), layouts);
        Ok(layouts)
    }

    /// The pipeline for a launch set, built once per content digest.
    ///
    /// # Invariant
    /// The SPIR-V is emitted only on a cache miss: emitting it on every run puts a whole-module
    /// emission on the critical path of a dispatch whose pipeline is already built. The push-constant
    /// block is a function of the fragment alone — its leaf classes and the module's — so it does not
    /// enter the cache key: the digests already fix it.
    fn pipeline(
        &self,
        launch: &LaunchSet<'_>,
        binding: Binding,
        push: u32,
    ) -> Result<vk::Pipeline, RunError> {
        // **The cache key is the whole set**, not the root: two sets sharing a
        // root and differing in a callee are two modules.
        if spirv::needs_int64(launch).map_err(RunError::Emit)? && !self.shader_int64 {
            return Err(RunError::MissingInt64 {
                device: self.name.clone(),
            });
        }
        let key = (
            launch
                .ordered()
                .iter()
                .map(|fragment| fragment_digest(fragment))
                .collect::<Vec<u64>>(),
            binding.inputs,
            binding.outputs,
        );
        if let Some(pipeline) = self.pipelines.lock().unwrap().get(&key) {
            return Ok(*pipeline);
        }
        let words = spirv::compile(launch, binding).map_err(RunError::Emit)?;
        let module = check("shader module", unsafe {
            // `code` takes the words, not bytes: no repacking needed.
            self.device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
        })?;
        // The same cached pair `dispatch` binds against, so a pipeline and the
        // descriptor sets that use it cannot disagree.
        let layouts = match self.layouts(binding.total(), push) {
            Ok(layouts) => layouts,
            Err(error) => {
                unsafe { self.device.destroy_shader_module(module, None) };
                return Err(error);
            }
        };
        let layout = layouts.pipeline;
        // `create_compute_pipelines` hands back what did build alongside the error.
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
                // Only the module is this call's to destroy; the layouts are
                // cached and shared with every later run of this shape.
                unsafe { self.device.destroy_shader_module(module, None) };
                return Err(RunError::Vulkan {
                    stage: "compute pipeline",
                    detail: format!("{code:?}"),
                });
            }
        };

        // The module goes because the pipeline has copied what it needs; the layouts
        // stay, since the cache points at them.
        unsafe { self.device.destroy_shader_module(module, None) };
        self.pipelines.lock().unwrap().insert(key, pipeline);
        Ok(pipeline)
    }
}

impl Drop for GpuContext {
    fn drop(&mut self) {
        // Nothing may be in flight when a context is dropped: an invariant of every
        // submitting path.

        // Destroying a command buffer under a running submission would be a
        // use-after-free rather than a leak.
        let device = &self.device;
        for (_, pool) in self.recycled.lock().unwrap().drain() {
            for buffer in pool {
                buffer.destroy(device);
            }
        }
        for (_, buffer) in self.resident.lock().unwrap().drain() {
            buffer.destroy(device);
        }
        for slot in &mut self.slots.lock().unwrap().entries {
            slot.destroy(device);
        }
        self.slots = Mutex::new(Slots {
            entries: Vec::new(),
            cursor: 0,
        });
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

impl lichen_kernel_ir::ParallelBackend for GpuContext {
    fn name(&self) -> &'static str {
        "gpu"
    }

    /// Delegates to [`GpuContext::run`], turning a refusal into the string the trait
    /// carries.
    fn run(
        &self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, String> {
        GpuContext::run(self, launch, inputs, count).map_err(|error| error.to_string())
    }

    /// The one backend that overrides the default: its pool hands the next
    /// submission a different slot.
    fn submit<'backend>(
        &'backend self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Box<dyn lichen_kernel_ir::Pending + 'backend>, String> {
        // The inherent form, named explicitly: `self.submit` would be the trait
        // method recursively.
        Ok(Box::new(
            GpuContext::submit(self, launch, inputs, count).map_err(|error| error.to_string())?,
        ))
    }

    /// The one backend that records a whole graph: one command buffer, one submit, one wait.
    fn run_batch(&self, batch: &[Dispatch<'_>]) -> Result<Vec<Vec<ResidentId>>, BatchRefusal> {
        GpuContext::run_batch(self, batch).map_err(|error| BatchRefusal::Refused(error.to_string()))
    }

    /// The one backend that reads no number back: the workgroup count is an indirect
    /// command's, and the host never holds it.
    fn run_over_device_extent(
        &self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        extent: DeviceExtent,
    ) -> Result<Vec<ResidentId>, String> {
        GpuContext::run_over_device_extent(self, launch, inputs, extent)
            .map_err(|error| error.to_string())
    }

    /// The class comes off the resident record, so a float fragment's result is
    /// fetched at the width its module declared.
    fn fetch(&self, id: ResidentId, count: usize) -> Result<ScalarData, String> {
        GpuContext::fetch(self, id, count).map_err(|error| error.to_string())
    }

    fn release(&self, id: ResidentId) {
        GpuContext::release(self, id);
    }
}

/// What this context's device is, for a loader that checks an artifact against it.
///
/// # Invariant
/// **Read, not re-queried**: the context probed the device where it was chosen, so a
/// loader's answer is the same one every later run will be given — and a context that
/// cannot report a subgroup width reports none rather than assuming this target's.
impl DeviceFactsSource for GpuContext {
    fn device_facts(&self) -> DeviceFacts {
        self.facts
    }
}

/// Install a device as the process's compute backend, replacing any previous one.
///
/// # Invariant
/// This is what a host program calls: `lichen-compute` never names this crate, so without this call
/// a program that asked for `"gpu"` is refused by name rather than run on the CPU.
pub fn install(context: GpuContext) {
    lichen_kernel_ir::install_parallel_backend(std::sync::Arc::new(context));
}

/// Open a device and install it, or say why no backend could be installed.
///
/// # Invariant
/// Device creation is eager: a program that cannot dispatch should learn that when it is wired up.
/// The context is not handed back — the registry keeps it alive, and a second handle would only be
/// a second way to be stale.
pub fn install_default() -> Result<(), String> {
    let context = GpuContext::new().map_err(|error| error.to_string())?;
    install(context);
    Ok(())
}

/// Whether a backend is installed, and which.
pub fn installed_backend_name() -> Option<&'static str> {
    lichen_kernel_ir::parallel_backend().map(|backend| backend.name())
}

/// Remove the installed backend, if any.
///
/// # Invariant
/// It drops the context, so every device buffer it held is released at the same moment: a test that
/// left one installed would hand that device — and its memory — to whatever ran next.
pub fn uninstall() {
    lichen_kernel_ir::clear_parallel_backend();
}
/// One input, resolved to what this backend can bind.
///
/// # Invariant
/// The one form [`fn dispatch_into`] binds, so a run and a batch are staged the same way. A
/// device buffer is carried by value rather than named by [`ResidentId`], because a batch
/// binds an output of the submission it is still recording and has issued no id for.
enum Bound<'a> {
    /// Data the host already holds, uploaded when the dispatch is recorded.
    Host(&'a [u8]),
    /// A buffer this backend is already holding.
    Resident(DeviceBuffer),
}

/// Everything one prepared dispatch needs to be recorded, and what it leaves behind.
///
/// # Invariant
/// Its descriptors name buffers this submission owns, so it is recorded once and dropped
/// after: a second recording of the same value would bind a buffer the first has since
/// recycled. The outputs are **not** in `scratch` — they are what the caller keeps.
struct Dispatched {
    pipeline: vk::Pipeline,
    descriptors: Vec<vk::DescriptorBufferInfo>,
    uploads: Vec<Transfer>,
    workgroups: Workgroups,
    scalars: Vec<u8>,
    outputs: Vec<DeviceBuffer>,
}

/// One staging-to-device copy recorded into a dispatch.
struct Transfer {
    /// Where the run's staged bytes sit in the staging buffer.
    src_offset: vk::DeviceSize,
    /// The device-local buffer they are going to.
    dst: vk::Buffer,
    /// The `len` elements the host actually holds.
    bytes: vk::DeviceSize,
    /// The allocation past [`Self::bytes`], which the surplus lanes read.
    ///
    /// # Invariant
    /// Zeroed on the device rather than uploaded, so a run costs `len` elements of traffic and the
    /// surplus lanes still read a defined `0`.
    tail: vk::DeviceSize,
}

/// What one run leaves behind for whoever is going to wait for it.
///
/// # Invariant
/// Consumed by exactly two callers, which is why it is a struct: the buffers in `scratch` are
/// unreachable by name, so they belong to this submission until it is waited for.
struct Staged {
    /// The slot the submission is in, and the only way to wait for it.
    token: Token,
    /// The ids the submission will produce, valid once the token is waited for.
    ids: Vec<ResidentId>,
    /// Buffers a host input was uploaded into; not resident, so nothing else reaches them.
    scratch: Vec<DeviceBuffer>,
}

/// A run this context has recorded and handed to the queue, and not waited for.
///
/// # Invariant
/// It is the implementation of [`lichen_kernel_ir::Pending`], and what makes a graph's "wait only
/// where something needs the data" schedule expressible. Dropping it waits and releases, so a
/// dropped submission is a slowdown rather than a use-after-free.
pub struct GpuPending<'backend> {
    context: &'backend GpuContext,
    token: Token,
    /// `None` once the submission has been waited for: the buffers move out to be
    /// recycled, and this type has a `Drop`.
    scratch: Option<Vec<DeviceBuffer>>,
    ids: Vec<ResidentId>,
    finished: bool,
}

impl GpuPending<'_> {
    /// Wait for the submission and release what it held, if it has not been
    /// waited for already.
    fn finish(&mut self) -> Result<(), RunError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        // The order is fixed: the fence first, then the memory it was reading.
        self.context.sync(self.token)?;
        for buffer in self.scratch.take().into_iter().flatten() {
            self.context.recycle(buffer);
        }
        Ok(())
    }
}

impl lichen_kernel_ir::Pending for GpuPending<'_> {
    fn outputs(&self) -> &[ResidentId] {
        &self.ids
    }

    fn wait(mut self: Box<Self>) -> Result<Vec<ResidentId>, String> {
        self.finish().map_err(|error| error.to_string())?;
        Ok(std::mem::take(&mut self.ids))
    }
}

impl Drop for GpuPending<'_> {
    fn drop(&mut self) {
        // A wait that fails is a fence that did not signal, which `sync` has said.
        let _ = self.finish();
    }
}

/// A device-local buffer of `padded` elements of one class.
///
/// # Invariant
/// Device-local because the shader reads and writes it where the shader runs. The class is part of
/// the record, not of the id: a [`ResidentId`] names a buffer and nothing else, so the width has to
/// travel beside it — it is the class the fragment declared, the one the module's `ArrayStride` was
/// emitted at.
#[derive(Clone, Copy)]
struct DeviceBuffer {
    handle: vk::Buffer,
    memory: vk::DeviceMemory,
    /// The buffer's capacity in elements — the run's *padded* count, not the
    /// count the caller asked about.
    padded: usize,
    /// The class its elements are, and so the width they occupy.
    class: ScalarClass,
}

impl DeviceBuffer {
    fn bytes(padded: usize, class: ScalarClass) -> vk::DeviceSize {
        (padded * class.byte_width()) as vk::DeviceSize
    }

    fn descriptor(&self) -> vk::DescriptorBufferInfo {
        vk::DescriptorBufferInfo {
            buffer: self.handle,
            offset: 0,
            range: Self::bytes(self.padded, self.class),
        }
    }

    fn destroy(&self, device: &ash::Device) {
        unsafe {
            device.destroy_buffer(self.handle, None);
            device.free_memory(self.memory, None);
        }
    }
}

/// Returns device buffers to the context's pool unless the run handed them off.
///
/// # Invariant
/// It owns exactly the scratch buffers: host upload targets on every path, plus the outputs on a
/// path that never completed — a failed run would otherwise leak them with nothing holding their
/// addresses.
struct ScratchGuard<'a> {
    context: &'a GpuContext,
    buffers: Vec<DeviceBuffer>,
}

impl Drop for ScratchGuard<'_> {
    fn drop(&mut self) {
        for buffer in std::mem::take(&mut self.buffers) {
            self.context.recycle(buffer);
        }
    }
}

/// Mapped host memory every upload and download is staged through.
///
/// # Invariant
/// One per slot, reused and grown: a slot's staging is dead the moment its fence signals, and one
/// shared mapping would put this submission's bytes underneath another slot's in-flight copy.
struct Staging {
    handle: vk::Buffer,
    memory: vk::DeviceMemory,
    /// The mapping's base address: a staging byte range, not an element index.
    mapped: *mut u8,
    capacity: usize,
}

// SAFETY: `Staging` is `Send`: every dereference of `mapped` happens while the
// pool's lock is held.

// A slot is not reachable until a fence wait releases it, so no two threads touch
// one mapping.

// The pointer names a fixed offset into host memory the device maps coherently.

// `Sync` is not implemented: the pointer makes a shared `&Staging` unsound.
unsafe impl Send for Staging {}

impl Staging {
    fn empty() -> Self {
        Staging {
            handle: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            mapped: std::ptr::null_mut(),
            capacity: 0,
        }
    }

    /// A pointer `offset` bytes into the mapping.
    fn at(&self, offset: vk::DeviceSize) -> *mut u8 {
        // SAFETY: `offset` is added to the mapping's own base, bounded by what
        // `reserve` was told the run needs.
        unsafe { self.mapped.add(offset as usize) }
    }

    /// Guarantee `bytes` of staging, reallocating only when the mapping is smaller.
    fn reserve(&mut self, context: &GpuContext, bytes: u64) -> Result<(), RunError> {
        if bytes as usize <= self.capacity {
            return Ok(());
        }
        let device = &context.device;
        let handle = check("staging buffer creation", unsafe {
            device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(bytes)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        })?;
        let requirements = unsafe { device.get_buffer_memory_requirements(handle) };
        let index = cached_memory_type(
            &context.instance,
            context.physical,
            requirements.memory_type_bits,
        )?;
        let memory = match check("staging memory allocation", unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(index),
                None,
            )
        }) {
            Ok(memory) => memory,
            Err(error) => {
                unsafe { device.destroy_buffer(handle, None) };
                return Err(error);
            }
        };
        if let Err(error) = check("staging buffer binding", unsafe {
            device.bind_buffer_memory(handle, memory, 0)
        }) {
            unsafe {
                device.destroy_buffer(handle, None);
                device.free_memory(memory, None);
            }
            return Err(error);
        }
        let mapped = check("staging memory mapping", unsafe {
            device.map_memory(memory, 0, bytes, vk::MemoryMapFlags::empty())
        })? as *mut u8;
        // The old mapping is dropped only once the replacement is usable, so a
        // failure above leaves the old one intact.
        self.destroy(context);
        self.handle = handle;
        self.memory = memory;
        self.mapped = mapped;
        self.capacity = bytes as usize;
        Ok(())
    }

    fn destroy(&mut self, context: &GpuContext) {
        if self.capacity == 0 {
            return;
        }
        unsafe {
            context.device.unmap_memory(self.memory);
            context.device.destroy_buffer(self.handle, None);
            context.device.free_memory(self.memory, None);
        }
        *self = Staging::empty();
    }
}

/// A host-visible, coherent and cached memory type, or a refusal.
///
/// # Invariant
/// `HOST_CACHED` is required rather than preferred: on a discrete GPU the uncached type is system
/// RAM over PCIe, so every host touch costs a round trip — a correct and ruinously slow buffer. A
/// device without cached host memory is refused by name.
fn cached_memory_type(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    allowed: u32,
) -> Result<u32, RunError> {
    let properties = unsafe { instance.get_physical_device_memory_properties(physical) };
    let wanted = vk::MemoryPropertyFlags::HOST_VISIBLE
        | vk::MemoryPropertyFlags::HOST_COHERENT
        | vk::MemoryPropertyFlags::HOST_CACHED;
    for index in 0..properties.memory_type_count {
        if allowed & (1u32 << index) == 0 {
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
        stage: "staging memory type selection",
        detail: "this device has no host-visible, host-coherent, host-cached memory, and \
                 staging without a cache turns every host write into a PCIe round trip; see \
                 the module docs"
            .into(),
    })
}

/// One submission's claim on a pool slot, and the recording going into it.
///
/// # Invariant
/// The claim is the pool's lock, held as long as this value lives, which makes "two threads never
/// record into one command buffer" a property the borrow checker sees. The three submit methods
/// differ in what they leave the caller holding: `submit_and_wait` gives the slot back, `submit`
/// keeps it claimed, and `submit_and_read_back` reads the staging while the claim is held.
struct Segment<'a> {
    context: &'a GpuContext,
    /// The pool lock, held for this value's whole life.
    slots: std::sync::MutexGuard<'a, Slots>,
    index: usize,
    /// Whether a submission on this slot may still be running.
    ///
    /// # Invariant
    /// Cleared only by a wait that confirmed the fence, so "nothing is in flight" is a fact about
    /// the device.
    in_flight: bool,
}

impl Segment<'_> {
    fn slot(&self) -> &Slot {
        &self.slots.entries[self.index]
    }

    /// The open command buffer, for recording a command that is not a dispatch.
    fn command(&self) -> vk::CommandBuffer {
        self.slot().command
    }

    /// This slot's staging: the only memory a submission of this segment may
    /// stage its uploads through.
    fn staging(&self) -> &Staging {
        &self.slot().staging
    }

    /// Guarantee this slot has `bytes` of staging, reallocating only if smaller.
    ///
    /// # Invariant
    /// Reallocation unmaps the old memory, so it is only safe while nothing reads it — which the
    /// claim guarantees, since `acquire` waited out the previous holder's submission.
    fn reserve(&mut self, context: &GpuContext, bytes: u64) -> Result<(), RunError> {
        self.slots.entries[self.index]
            .staging
            .reserve(context, bytes)
    }

    fn begin(&mut self) -> Result<(), RunError> {
        self.context.begin_recording(self.slot())
    }

    /// Record one dispatch into the already-open recording.
    fn record(
        &self,
        pipeline: vk::Pipeline,
        descriptors: &[vk::DescriptorBufferInfo],
        uploads: &[Transfer],
        workgroups: Workgroups,
        scalars: &[u8],
    ) -> Result<(), RunError> {
        self.context.record_dispatch(
            self.slot(),
            pipeline,
            descriptors,
            uploads,
            workgroups,
            scalars,
        )
    }

    /// Close the recording and hand it to the queue, without waiting.
    ///
    /// # Invariant
    /// Takes `&mut self` rather than consuming the segment, so a caller that still needs the slot —
    /// to wait on it or read its staging — can have both. Promising the device is done is
    /// [`GpuContext::wait_on`]'s job.
    fn hand_to_queue(&mut self) -> Result<(), RunError> {
        self.context.end_and_submit(self.slot())?;
        self.in_flight = true;
        Ok(())
    }

    /// Close the recording, hand it to the queue, and do not wait.
    ///
    /// # Invariant
    /// The slot stays claimed, so a second acquisition takes a different one and the host is free
    /// for as long as the device takes. Waiting is [`GpuContext::sync`]'s job, a separate call on
    /// purpose: that gap is the whole opportunity the overlapping strategy has.
    fn submit(mut self) -> Result<Token, RunError> {
        self.hand_to_queue()?;
        Ok(Token(self.index as u32))
    }

    /// Record, submit, wait, and release the slot.
    fn submit_and_wait(self) -> Result<(), RunError> {
        // Read before the move: `submit` consumes the segment, and the context
        // reference is what `sync` needs afterwards.
        let context = self.context;
        let token = self.submit()?;
        context.sync(token)
    }

    /// Submit, wait, and take `count` elements of this slot's staging with us.
    ///
    /// # Invariant
    /// The read is inside the claim on purpose: written as `submit`, `sync`, then a read, the next
    /// acquisition could rewrite the very mapping being copied out of — the wrong numbers rather
    /// than a failure. `count` is elements, so the byte range is `count * 4` for a float buffer and
    /// `count * 8` for an integer one.
    fn submit_and_read_back(
        mut self,
        count: usize,
        class: ScalarClass,
    ) -> Result<ScalarData, RunError> {
        self.hand_to_queue()?;
        self.context.wait_on(self.slot())?;
        // The fence has signalled; the claim goes back on drop, not here.
        self.in_flight = false;
        // SAFETY: the slot was reserved and mapped before the submit, and the wait
        // above proves the copy finished.
        let data =
            unsafe { std::slice::from_raw_parts(self.staging().at(0), count * class.byte_width()) };
        Ok(match class {
            ScalarClass::Int => ScalarData::Int(
                data.chunks_exact(8)
                    .map(|bytes| i64::from_le_bytes(bytes.try_into().unwrap_or_default()))
                    .collect(),
            ),
            ScalarClass::Float => ScalarData::Float(
                data.chunks_exact(4)
                    .map(|bytes| {
                        f32::from_bits(u32::from_le_bytes(bytes.try_into().unwrap_or_default()))
                    })
                    .collect(),
            ),
        })
    }
}

impl Drop for Segment<'_> {
    fn drop(&mut self) {
        // Only a path that neither submitted nor waited gives the slot back here.
        if !self.in_flight {
            self.slots.entries[self.index].claimed = false;
        }
    }
}

impl Slot {
    /// A command buffer, a fence, a descriptor pool and a staging buffer, all this
    /// submission's own.
    ///
    /// # Invariant
    /// The fence is created unsignalled and reset before every use, so a wait always waits on this
    /// slot's last submission.
    fn new(device: &ash::Device, family: u32) -> Result<Self, RunError> {
        let command_pool = check("command pool", unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(family),
                None,
            )
        })?;
        let mut command = vk::CommandBuffer::null();
        let allocated = check("command buffer allocation", unsafe {
            device.allocate_command_buffers(&vk::CommandBufferAllocateInfo {
                // Stated for the same reason as the descriptor set count: a
                // zero-count request succeeds while allocating nothing.
                command_buffer_count: 1,
                command_pool,
                ..Default::default()
            })
        })
        .and_then(|buffers| {
            command = *buffers.first().ok_or(RunError::Vulkan {
                stage: "command buffer allocation",
                detail: "Vulkan reported success but returned no command buffer".into(),
            })?;
            Ok(())
        });
        if let Err(error) = allocated {
            unsafe { device.destroy_command_pool(command_pool, None) };
            return Err(error);
        }
        let fence = check("fence", unsafe {
            device.create_fence(&vk::FenceCreateInfo::default(), None)
        })?;
        let descriptor_pool = match check("descriptor pool", unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(MAX_DISPATCHES_PER_SUBMISSION as u32)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::STORAGE_BUFFER,
                        descriptor_count: (MAX_DISPATCHES_PER_SUBMISSION * MAX_DESCRIPTOR_BINDINGS)
                            as u32,
                    }]),
                None,
            )
        }) {
            Ok(pool) => pool,
            Err(error) => {
                unsafe {
                    device.destroy_fence(fence, None);
                    // Destroying the pool frees the command buffer allocated from
                    // it, so there is nothing else to destroy.
                    device.destroy_command_pool(command_pool, None);
                }
                return Err(error);
            }
        };
        Ok(Slot {
            claimed: false,
            command_pool,
            command,
            fence,
            staging: Staging::empty(),
            descriptor_pool,
        })
    }

    /// Release every device object this slot owns.
    ///
    /// # Invariant
    /// Only safe when nothing is in flight: destroying these under a running submission is a
    /// use-after-free, and `GpuContext::drop` relies on every submitting path waiting first.
    fn destroy(&mut self, device: &ash::Device) {
        // The staging needs the context, not the device, to unmap; a never-reserved
        // one holds nothing.
        if self.staging.capacity > 0 {
            unsafe {
                device.unmap_memory(self.staging.memory);
                device.destroy_buffer(self.staging.handle, None);
                device.free_memory(self.staging.memory, None);
            }
            self.staging = Staging::empty();
        }
        unsafe {
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_fence(self.fence, None);
            device.destroy_command_pool(self.command_pool, None);
        }
        self.command = vk::CommandBuffer::null();
        self.fence = vk::Fence::null();
        self.command_pool = vk::CommandPool::null();
        self.descriptor_pool = vk::DescriptorPool::null();
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

/// A memory type index carrying all of `wanted`, or `None` when the device has none.
///
/// # Invariant
/// No fallback to a type without the flags: memory the host cannot reach cannot be read back, so a
/// missing flag is a refusal that names what it wanted rather than empty results.
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
        detail: format!("the device offers no memory type with {wanted:?}"),
    })
}
