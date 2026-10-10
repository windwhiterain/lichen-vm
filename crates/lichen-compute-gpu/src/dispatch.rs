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
    BufferSlot, LaunchSet, ResidentId, ScalarClass, ScalarData, fragment_digest,
};

use crate::spirv::{self, Binding, LOCAL_SIZE_X, SpirvRefusal};

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
    /// More storage buffers than the pool has room for; the pool is allocated once.
    TooManyBindings { total: usize, max: usize },
    /// A chain asked for a dispatch count one submission cannot record.
    ///
    /// # Invariant
    /// One to `MAX_DISPATCHES_PER_SUBMISSION`, which is what the pool has sets for; zero is
    /// refused too, because a submission with no dispatch is a different request, not a shorter
    /// chain.
    ChainLength { wanted: usize, max: usize },
    /// A chain needs each link to have one input and one output.
    ChainNotLinear { inputs: usize, outputs: usize },
    /// A chain whose fragment reads one class and writes the other, which a chain cannot carry.
    ChainCrossesClasses {
        input: ScalarClass,
        output: ScalarClass,
    },
    /// A fragment whose body reads a parameter beside the extent.
    ///
    /// # Invariant
    /// The dispatch carries no leaf but the index, so a second read leaf has nowhere to go; a leaf
    /// the body never reads is not this, because nothing is missing from it.
    ScalarsNotPushed { leaves: usize },
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
            RunError::TooManyBindings { total, max } => write!(
                f,
                "the run binds {total} storage buffer(s) and this backend's descriptor pool is \
                 sized for {max}, so it is refused here rather than left to fail as pool \
                 exhaustion on the device"
            ),
            RunError::ChainLength { wanted, max } => write!(
                f,
                "a chain of {wanted} dispatch(es) cannot be recorded in one submission: the \
                 range is 1 to {max}, which is what the descriptor pool has sets for."
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
            RunError::ScalarsNotPushed { leaves } => write!(
                f,
                "this fragment's parameter declares {leaves} leaf/leaves (the launch extent, \
                 any runtime scalar, and the index), and a dispatch pushes the extent alone: \
                 a runtime scalar needs the leaf list the CPU path passes, so this is refused \
                 rather than dispatched with the argument missing"
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

/// Turn a Vulkan failure into a named `RunError`.
fn check<T>(stage: &'static str, result: Result<T, vk::Result>) -> Result<T, RunError> {
    result.map_err(|code| RunError::Vulkan {
        stage,
        detail: format!("{code:?}"),
    })
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
    /// The descriptor set and pipeline layouts for a run with `n` buffers.
    layouts: Mutex<HashMap<usize, Layouts>>,
    /// Device buffers that have been given back, keyed by their element count and class.
    ///
    /// # Invariant
    /// Reuse is safe **without clearing**: every consumer of a buffer writes all of it —
    /// an output across `[0, padded)` and an input across `[0, count)` — so no stale byte
    /// is ever read back.
    recycled: Mutex<HashMap<(usize, ScalarClass), Vec<DeviceBuffer>>>,
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

/// The two layouts a dispatch binds, which depend only on how many buffers the
/// run has in total.
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
            pipelines: Mutex::new(HashMap::new()),
            resident: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            slots: Mutex::new(Slots {
                entries: built,
                cursor: 0,
            }),
            layouts: Mutex::new(HashMap::new()),
            recycled: Mutex::new(HashMap::new()),
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
        let staged = self.stage_run(launch, inputs, count)?;
        // The upload targets are recycled after the wait rather than in a `finally`, so
        // a failed wait does not leak them.
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
        let staged = self.stage_run(launch, inputs, count)?;
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
        count: usize,
    ) -> Result<Staged, RunError> {
        let fragment = launch.root();
        // **The device path pushes no leaf at all**, so a parameter the body *reads* is
        // refused by name.
        let leaves = fragment.param_shape.flat_arity();
        let index = spirv::index_local(fragment);
        // A declared leaf the body never reads is not missing; the emitter draws the same
        // line.
        let index_param = index.and_then(|index| {
            fragment.body.blocks[fragment.body.entry]
                .params
                .get(index as usize)
        });
        let reads_a_parameter = fragment
            .body
            .operands()
            .iter()
            .any(|value| fragment.body.is_a_parameter(*value) && Some(value) != index_param);
        if reads_a_parameter {
            return Err(RunError::ScalarsNotPushed { leaves });
        }
        let class = spirv::module_class(fragment).map_err(RunError::Emit)?;
        let binding = Binding {
            inputs: inputs.len(),
            outputs: fragment.outputs,
        };
        for (index, slot) in inputs.iter().enumerate() {
            // **The width is this buffer's own class**, not one module-wide answer.
            let width = spirv::buffer_class_of(fragment, index, class).byte_width();
            if let BufferSlot::Host(data) = slot
                && data.len() < count * width
            {
                return Err(RunError::InputShorterThanCount {
                    buffer: index,
                    len: data.len() / width,
                    count,
                });
            }
        }
        let pipeline = self.pipeline(launch, binding)?;

        // Round up so the last workgroup's surplus lanes address padding rather
        // than memory past the end; see the module docs.
        let padded = count.div_ceil(LOCAL_SIZE_X as usize) * LOCAL_SIZE_X as usize;
        if padded == 0 {
            // A zero-element run would ask Vulkan for a zero-sized buffer.
            return Err(RunError::EmptyRun);
        }
        // The slot is taken before anything is staged into it: the staging a run writes
        // is the slot's own.
        let mut segment = self.acquire()?;
        // The reservation is the sum of each host input's own `count × byte_width()`.
        let staging_bytes: vk::DeviceSize = inputs
            .iter()
            .enumerate()
            .map(|(index, slot)| match slot {
                BufferSlot::Host(_) => {
                    count as vk::DeviceSize
                        * spirv::buffer_class_of(fragment, index, class).byte_width()
                            as vk::DeviceSize
                }
                BufferSlot::Resident(_) => 0,
            })
            .sum();
        segment.reserve(self, staging_bytes)?;

        let mut scratch = ScratchGuard {
            context: self,
            buffers: Vec::new(),
        };
        let mut descriptors: Vec<vk::DescriptorBufferInfo> = Vec::with_capacity(binding.total());
        let mut uploads: Vec<Transfer> = Vec::with_capacity(inputs.len());
        // A block begins where the previous one ended: `offset` is the sum of the blocks before it.
        let mut offset: vk::DeviceSize = 0;
        for (index, slot) in inputs.iter().enumerate() {
            let buffer = match slot {
                // Used where it lies: a chained run costs one upload, not two.
                BufferSlot::Resident(id) => self.resident_buffer(*id)?,
                BufferSlot::Host(data) => {
                    let buffer_class = spirv::buffer_class_of(fragment, index, class);
                    let element = buffer_class.byte_width() as vk::DeviceSize;
                    let data_bytes = count as vk::DeviceSize * element;
                    let padded_bytes = padded as vk::DeviceSize * element;
                    let buffer = self.allocate(padded, buffer_class)?;
                    // SAFETY: `offset` plus this block is inside the staging `reserve` sized per buffer, and the claimed slot is idle.
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
        // Each output is allocated and recorded at its own class, the width a fetch reads it at.
        for buffer_class in &output_classes {
            let buffer = self.allocate(padded, *buffer_class)?;
            descriptors.push(buffer.descriptor());
            scratch.buffers.push(buffer);
        }

        segment.begin()?;
        segment.record(pipeline, &descriptors, &uploads, count)?;
        // Nothing below can fail, so the bookkeeping comes after the submit: a failed
        // submit means nothing reached the queue.
        let token = segment.submit()?;
        let mut resident = self.resident.lock().unwrap();
        let mut ids = Vec::with_capacity(binding.outputs);
        for (ordinal, buffer) in scratch
            .buffers
            .split_off(scratch.buffers.len() - binding.outputs)
            .into_iter()
            .enumerate()
        {
            let id = ResidentId(self.next_id.fetch_add(1, AtomicOrdering::Relaxed));
            resident.insert(
                id,
                DeviceBuffer {
                    handle: buffer.handle,
                    memory: buffer.memory,
                    padded,
                    // The class this output's element type was emitted for, so a fetch reads
                    // it back at the width it was written at.
                    class: output_classes[ordinal],
                },
            );
            ids.push(id);
        }
        // The guard is now empty; what is left is the host upload targets.
        let scratch = std::mem::take(&mut scratch.buffers);
        drop(resident);
        Ok(Staged {
            token,
            ids,
            scratch,
        })
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
            return Err(RunError::ChainLength {
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
        if input.len() < count * element {
            return Err(RunError::InputShorterThanCount {
                buffer: 0,
                len: input.len() / element,
                count,
            });
        }

        let pipeline = self.pipeline(
            launch,
            Binding {
                inputs: 1,
                outputs: 1,
            },
        )?;
        let padded = count.div_ceil(LOCAL_SIZE_X as usize) * LOCAL_SIZE_X as usize;
        if padded == 0 {
            return Err(RunError::EmptyRun);
        }
        let element = element as vk::DeviceSize;
        let data_bytes = count as vk::DeviceSize * element;
        let padded_bytes = padded as vk::DeviceSize * element;

        // One slot, its staging sized for the whole chain up front.
        let mut segment = self.acquire()?;
        segment.reserve(self, data_bytes)?;

        let mut scratch = ScratchGuard {
            context: self,
            buffers: Vec::with_capacity(1),
        };
        let source = self.allocate(padded, input_class)?;
        scratch.buffers.push(source);
        // SAFETY: `reserve` sized and mapped this slot's staging for `count` elements.
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
            segment.record(pipeline, &descriptors, uploads, count)?;
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
                            | vk::BufferUsageFlags::TRANSFER_SRC,
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
            })
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
        })
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
        count: usize,
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

        // The two layouts are a function of `total` alone, built once per shape, and
        // `pipeline` builds the same pair.
        let layouts = self.layouts(total)?;

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
                    // The surplus lanes read past `count`, so the tail is cleared on the
                    // device rather than uploaded as zeroes.
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
            device.cmd_dispatch(command, count.div_ceil(LOCAL_SIZE_X as usize) as u32, 1, 1);
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
    fn layouts(&self, total: usize) -> Result<Layouts, RunError> {
        if let Some(layouts) = self.layouts.lock().unwrap().get(&total) {
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
        let pipeline = match check("pipeline layout", unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[set])
                    .push_constant_ranges(&[]),
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
        self.layouts.lock().unwrap().insert(total, layouts);
        Ok(layouts)
    }

    /// The pipeline for a launch set, built once per content digest.
    ///
    /// # Invariant
    /// The SPIR-V is emitted only on a cache miss: emitting it on every run puts a whole-module
    /// emission on the critical path of a dispatch whose pipeline is already built.
    fn pipeline(&self, launch: &LaunchSet<'_>, binding: Binding) -> Result<vk::Pipeline, RunError> {
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
        let layouts = match self.layouts(binding.total()) {
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

    /// The class comes off the resident record, so a float fragment's result is
    /// fetched at the width its module declared.
    fn fetch(&self, id: ResidentId, count: usize) -> Result<ScalarData, String> {
        GpuContext::fetch(self, id, count).map_err(|error| error.to_string())
    }

    fn release(&self, id: ResidentId) {
        GpuContext::release(self, id);
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
/// One staging-to-device copy recorded into a dispatch.
struct Transfer {
    /// Where the run's staged bytes sit in the staging buffer.
    src_offset: vk::DeviceSize,
    /// The device-local buffer they are going to.
    dst: vk::Buffer,
    /// The `count` elements the host actually holds.
    bytes: vk::DeviceSize,
    /// The allocation past [`Self::bytes`], which the surplus lanes read.
    ///
    /// # Invariant
    /// Zeroed on the device rather than uploaded, so a run costs `count` elements of traffic and the
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
        count: usize,
    ) -> Result<(), RunError> {
        self.context
            .record_dispatch(self.slot(), pipeline, descriptors, uploads, count)
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
