//! The Vulkan side: a device, a pipeline per fragment, and a dispatch.
//!
//! # The tail-lane obligation, and how it is discharged
//!
//! A dispatch covers [`spirv::LOCAL_SIZE_X`] invocations per workgroup, so the
//! last workgroup runs lanes whose index is past the end. The shader has **no
//! bounds test**, deliberately: a bounds test would be a branch every lane takes,
//! on the hottest path, to guard indices the host already knows about.
//!
//! Instead every buffer is allocated rounded up to a whole number of workgroups,
//! and the padding is handled by allocation rather than by data:
//!
//! - **Inputs** have their padding zeroed **on the device**, by a
//!   `cmdFillBuffer` recorded next to the upload. Only the `count` real elements
//!   cross the bus; the tail is cleared where it already lives rather than
//!   uploaded as zeroes the host had copies of anyway. The surplus lanes'
//!   *reads* are therefore in bounds and return `0`.
//! - **Outputs** are not initialised at all. The emitter emits a write only where
//!   every invocation reaches it — a selection arm is reached by exactly the lanes
//!   that take it, and a write in a loop body is refused by name — so every
//!   invocation reaches its write and the dispatch covers `[0, padded)` in full.
//!   A zero-fill would be a second pass over memory the shader is about to
//!   overwrite completely. The surplus lanes' *writes* land in the padding rather
//!   than past the end of it.
//!
//! Only the first `count` elements are ever read back.
//!
//! # Where the data lives
//!
//! Buffers are **device-local**: the shader reads and writes them where the
//! shader runs. Data reaches them through a separate **staging** buffer that the
//! host maps, and reaches the host back the same way — so a round trip is a
//! `vkCmdCopyBuffer` against mapped system memory, not a per-element transfer
//! on the shader's path.
//!
//! The staging buffer is `HOST_CACHED`, and that is required rather than
//! preferred. A host-visible-but-uncached allocation on a discrete GPU is system
//! RAM reached over PCIe with no cache behind it, and every host touch to it
//! costs a round trip — which is a *correct* buffer and an appallingly slow one.
//! Choosing it silently would put a backend on the same footing as the CPU it is
//! meant to beat while reporting success, so a device with no cached host memory
//! is refused by name instead.
//!
//! # Memory ordering is explicit
//!
//! Coherent memory is not *ordering*. Host writes to staging must be made
//! readable by the transfer that uploads them, the upload must be visible to the
//! shader, and the shader's writes must be visible to the download that reads
//! them back — so the command buffer carries a barrier at each of those points
//! rather than relying on the submit and the fence to imply it.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use ash::vk;
use lichen_kernel_ir::{
    BufferSlot, KernelFragment, ResidentId, ScalarClass, ScalarData, fragment_digest,
};

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
    /// A run over no indices at all.  The buffer such a run would need is a
    /// zero-sized allocation, which Vulkan does not have; refused here so the
    /// reason is the program's rather than the driver's.
    EmptyRun,
    /// A dispatch binds more storage buffers than the descriptor pool has room
    /// for.  The pool is allocated once, so this is a limit of the backend
    /// rather than of the device — refused by name instead of surfacing as pool
    /// exhaustion.
    TooManyBindings { total: usize, max: usize },
    /// A chain asked for a number of dispatches that one submission cannot
    /// record. The range is one to `MAX_DISPATCHES_PER_SUBMISSION`, which is
    /// what the descriptor pool has sets for; over it, the chain would exhaust
    /// the pool on the device instead of being refused here. Zero is refused
    /// too, and for a different reason worth stating: a submission with no
    /// dispatch in it is not a shorter chain, it is a different request.
    ChainLength { wanted: usize, max: usize },
    /// A chain needs each link to have one input and one output, so that link
    /// `n + 1` can be handed link `n`'s result without the caller naming
    /// buffers. A wider shape is a real requirement and a real graph node, but
    /// it is not a linear chain.
    ChainNotLinear { inputs: usize, outputs: usize },
    /// A chain whose fragment reads one class and writes the other, which a chain cannot carry.
    ChainCrossesClasses {
        input: ScalarClass,
        output: ScalarClass,
    },
    /// A fragment whose body **reads** a parameter beside the extent: the
    /// dispatch carries no leaf but the index, so a second read leaf has nowhere
    /// to go.  Refused by name rather than dispatched with the argument missing,
    /// which would compute every lane from the wrong value — the CPU path passes
    /// the whole leaf list (`docs/notes/compute-runtime-scalars.md` §3).  A leaf
    /// the body never reads is *not* this: nothing is missing from it.
    ScalarsNotPushed { leaves: usize },
    /// A resident id this context is not holding — never issued, or already
    /// released.  Refused rather than read as empty: an id is a handle, and using
    /// a dead one means the host lost track of its own buffers, which reporting
    /// `0`s would hide.
    UnknownResident { id: u64 },
    /// A pool depth of zero, which is not a smaller pool.  There is nothing to
    /// acquire and therefore no submission any shape can be recorded into, so it
    /// is refused here rather than as an empty acquire later.
    NoSlots { wanted: usize },
    /// A token naming a slot this context does not have — issued by a different
    /// context, or a number that was never a slot at all.  The same reasoning as
    /// [`Self::UnknownResident`]: a handle that names nothing must say so, and
    /// waiting on a slot this context does not own would block on someone else's
    /// fence.
    UnknownSlot { slot: u32 },
    /// A fetch asked for more elements than the buffer was allocated for. The
    /// buffer is sized to the run's padded count, so this means the host is
    /// asking for a run that did not produce this buffer.
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
/// One context owns one device.  Pipelines are cached on the fragment's content
/// digest, so a fragment that is run twice compiles once — the same identity the
/// wasm backend's module cache keys on, which is why [`fragment_digest`] lives
/// in the IR rather than in either backend.
pub struct GpuContext {
    entry: ash::Entry,
    instance: ash::Instance,
    device: ash::Device,
    queue: vk::Queue,
    /// Retained so memory-type queries can be answered; those live on the
    /// instance, not the device, so the device alone is not enough.
    physical: vk::PhysicalDevice,
    /// Retained so a pipeline cache key can be checked against what the device
    /// actually accepted, and for the device name in a refusal.
    name: String,
    /// Whether the device offers `shaderInt64`.
    ///
    /// **A property of the fragment decides whether it matters**, not a property
    /// of the device: an integer fragment's module declares a 64-bit integer and
    /// needs the feature, while a float one's integers are 32-bit indices and
    /// its scalar is core `Float32` ([`spirv::needs_int64`]). So the feature is
    /// recorded here and checked where a fragment is compiled, rather than used
    /// to reject the device at selection time — which would leave a float kernel
    /// unable to run on a device for a reason that no longer applies.
    shader_int64: bool,
    pipelines: Mutex<HashMap<(u64, usize, usize), vk::Pipeline>>,
    /// Buffers handed out as [`ResidentId`]s and not yet released. The value is
    /// the device-local allocation behind the id, so the id is the only handle
    /// the host ever holds to device memory.
    resident: Mutex<HashMap<ResidentId, DeviceBuffer>>,
    /// Hands out ids. Never reused: a stale id released twice must not name a
    /// live buffer, so ids only ever move forward.
    next_id: AtomicU64,
    /// The submissions' own objects, one set per slot — see [`Slots`].
    ///
    /// This is what makes more than one submission in flight possible at all. A
    /// single command buffer cannot be recorded while a previous recording of it
    /// is still running, so one command buffer means one submission at a time,
    /// which is the batch strategy and not the overlapping one.
    slots: Mutex<Slots>,
    /// The descriptor set layout and pipeline layout for a run with `n` buffers
    /// in total. They are a function of `n` alone — the same two objects the old
    /// per-run code built and dropped for every dispatch.
    layouts: Mutex<HashMap<usize, Layouts>>,
    /// Device buffers that have been given back, keyed by their element count.
    ///
    /// Reuse is safe **without clearing**, and that is worth spelling out because
    /// it is what makes this a saving rather than a trade: every consumer of a
    /// buffer writes all of it. An output is written by the shader across
    /// `[0, padded)` — see `spirv`'s write-reachability invariant — and an input
    /// is uploaded across `[0, count)` with the tail filled on the device, so no
    /// stale byte is ever read back.
    ///
    /// Keyed by exact element count **and class**, so a chain at a fixed size and
    /// class is served from here forever and a different size — or the same size
    /// in the other class, whose elements are half as wide — simply allocates.
    /// The cap is what keeps this from turning a release into a permanent VRAM
    /// reservation.
    recycled: Mutex<HashMap<(usize, ScalarClass), Vec<DeviceBuffer>>>,
}

/// How deep a pool is when nobody says otherwise.
///
/// Two, and not one, because one is not a pool: with a single slot a submission
/// must finish before the next can be recorded, so the host's work and the
/// device's work never overlap. Two is the smallest depth that lets one
/// submission be recording while the one before it is still running, which is
/// the whole difference between the batch strategy and the overlapping one.
pub const DEFAULT_SLOT_DEPTH: usize = 2;

/// What a caller may choose about a context.
///
/// Rust-side only, and deliberately so. A pool depth is a scheduling budget
/// rather than a semantic choice: a program that computed different answers at
/// depth 1 and depth 2 would make it part of the language, and the depth would
/// then need a surface, a spelling and a compatibility promise. Keeping it here
/// means it can move without any of that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuConfig {
    /// How many submissions may be in flight at once, each with a command
    /// buffer, a fence, a staging buffer and a descriptor pool of its own.
    ///
    /// One means the device is never behind the host by more than one
    /// submission. Larger values let the host record ahead of the device by that
    /// many submissions, at the cost of that many command buffers, staging
    /// buffers and descriptor pools resident in VRAM.
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
/// **Everything a submission reads or rewrites lives here, together, for one
/// reason: they all go invalid or get rewritten when the submission ends.** A
/// command buffer cannot be recorded while a previous recording of it is
/// running. A staging mapping cannot be written while a copy out of it is still
/// in flight. A descriptor pool cannot be reset while a dispatch bound to its
/// sets is still executing. Splitting them across a context and a "one at a
/// time" lock is how a second in-flight submission gets recorded into the first
/// one's command buffer; keeping them in one struct makes that mistake
/// unrepresentable instead of merely discouraged.
struct Slot {
    /// Set from the moment this slot is acquired until a wait has confirmed its
    /// fence. While it is set, the slot's objects belong to the device.
    claimed: bool,
    /// Retained so `Drop` can destroy it. It is not needed again while the slot
    /// lives: a command buffer is freed by destroying the pool it came from, and
    /// the pool has to outlive every buffer allocated from it.
    command_pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    /// Mapped host memory this slot's uploads and downloads are staged through.
    /// Per slot rather than per context precisely so that staging one submission
    /// cannot land under a copy the device has not made yet.
    staging: Staging,
    /// Descriptor sets for this slot's submission, reset when the slot is
    /// acquired and at no point while it is held.
    ///
    /// The rule is not a preference. A set is read when the submission
    /// *executes*, so one set cannot serve two dispatches — rewriting it between
    /// them changes what the first one sees. The pool cannot be reset between
    /// them either: a reset frees every set, including ones an earlier dispatch
    /// in the same command buffer is still bound to, and that is undefined
    /// rather than slow.
    ///
    /// **Per slot rather than one for the context** because "reset at the
    /// submission boundary" stops being a boundary once there are two
    /// submissions in flight: the next submission starts while this one is still
    /// running, and a reset would free sets this submission's dispatches are
    /// bound to. Each slot therefore has a pool nobody else can reach, and the
    /// reset happens where a fence wait has just proved that slot is idle —
    /// see [`GpuContext::acquire`].
    descriptor_pool: vk::DescriptorPool,
}

/// The pool: the slots, and where the next acquisition looks.
struct Slots {
    entries: Vec<Slot>,
    /// Where [`GpuContext::acquire`] looks next. Round-robin rather than "first
    /// free", so a stream of acquisitions cannot keep returning to slot zero
    /// while the rest go untouched: round-robin bounds how long any slot can be
    /// left waiting behind the cursor.
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
/// [`GpuContext::acquire`] hands one out with a [`Segment`], and
/// [`GpuContext::sync`] consumes it. It is `Copy` and carries nothing but the
/// slot's number, which is the same bargain [`ResidentId`] makes and for the
/// same reason: a token means nothing without the context that issued it, so it
/// is never compared across contexts or persisted.
///
/// **It is consumed by the wait, not clonable into a second wait.** That is the
/// point. Waiting the same submission twice would not be a no-op — by the time
/// the second wait ran, the slot could be running *someone else's* submission,
/// and the wait would return having proved the wrong thing. Taking the token by
/// value makes the second wait inexpressible rather than merely wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Token(u32);

/// How many freed buffers of one size are kept for reuse.
///
/// Four is enough for a chain to ping-pong two buffers plus a host input and a
/// host output without allocating, and small enough that idling here is not worth
/// measuring against a device's whole memory.
const RECYCLED_PER_SIZE: usize = 4;

/// The two layouts a dispatch binds, which depend only on how many buffers the
/// run has in total.
#[derive(Clone, Copy)]
struct Layouts {
    set: vk::DescriptorSetLayout,
    pipeline: vk::PipelineLayout,
}

/// The most storage-buffer bindings **one dispatch** may declare.
///
/// Refused by name rather than left to fail as a driver-side pool exhaustion,
/// which would read like a device problem.
const MAX_DESCRIPTOR_BINDINGS: usize = 32;

/// The most dispatches **one submission** may record.
///
/// A second descriptor set per dispatch, and never a shared one — see
/// [`GpuContext::descriptor_pool`]. The pool is allocated once for this many
/// sets, so this is a limit of the backend rather than of the device.
///
/// Sixty-four is a placeholder, picked to keep the pool in the low hundreds of
/// kilobytes and deliberately not raised further: the graph-level node set that
/// ought to own this number does not exist yet, and a larger one here would be
/// headroom for a program shape nobody has written. A submission wanting more
/// is refused by name rather than quietly split into two.
const MAX_DISPATCHES_PER_SUBMISSION: usize = 64;

impl GpuContext {
    /// Find a device and create a compute context on it, with the default pool.
    ///
    /// A **discrete** GPU is preferred over an integrated one, because a compute
    /// run is exactly the workload where that matters; among equals, the lowest
    /// index wins so the choice is deterministic.
    pub fn new() -> Result<Self, RunError> {
        Self::with_config(GpuConfig::default())
    }

    /// [`Self::new`], with a chosen pool depth.
    ///
    /// Everything else about the context is the same either way: the device
    /// choice, the pipeline cache, the buffer pool. The depth is the only knob,
    /// and it is on this side of the boundary because it is a scheduling budget
    /// rather than anything a program says — see [`GpuConfig`].
    pub fn with_config(config: GpuConfig) -> Result<Self, RunError> {
        if config.slot_depth == 0 {
            return Err(RunError::NoSlots {
                wanted: config.slot_depth,
            });
        }
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

        let mut chosen: Option<(vk::PhysicalDevice, vk::PhysicalDeviceProperties, u32, bool)> =
            None;
        let seen = devices.len();
        for physical in devices {
            let properties = unsafe { instance.get_physical_device_properties(physical) };
            let features = unsafe { instance.get_physical_device_features(physical) };
            // `shaderInt64` is recorded, not required: a float fragment's module
            // has no 64-bit integer in it, so refusing the device here would
            // refuse a kernel this device can run. It is still *preferred* below,
            // because an integer fragment needs it and the common case is one.
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
                        // A discrete device wins outright, and among equals the
                        // lowest index still wins unless the incumbent cannot run
                        // an integer fragment and this one can.
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

        // One slot per permitted in-flight submission, all built before the
        // context exists because they need a borrow of the device that the
        // struct literal below is about to move.  A failure part-way leaves the
        // slots already built alive, so they are destroyed rather than dropped:
        // their command pools and descriptor pools are device objects, and the
        // device outlives this function.
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

    /// Take a slot to record one submission into, and the token that will wait
    /// for it.
    ///
    /// This is the whole scheduling surface. A caller that records, submits and
    /// then drops the segment without submitting gets its slot straight back; a
    /// caller that submits gets a [`Token`] and the slot stays claimed until
    /// [`Self::sync`] has seen its fence. Between those two points the host is
    /// free — that window is what the overlapping strategy is made of, and it
    /// exists whether or not the host spends it.
    ///
    /// The lock is held for the segment's whole life, and that is what makes
    /// "two threads never record into one command buffer" a borrow-checked fact
    /// rather than a convention. The cost is that a second thread cannot acquire
    /// while the first is *recording*, which is a few hundred microseconds of
    /// host time; the win is that it also cannot acquire while the first is
    /// **waiting**, which is the whole point.
    fn acquire(&self) -> Result<Segment<'_>, RunError> {
        let mut slots = self.slots.lock().unwrap();
        let index = slots.cursor;
        slots.cursor = (index + 1) % slots.entries.len();
        if slots.entries[index].claimed {
            // This is where a depth above one stops being bookkeeping.  The slot
            // is running someone else's submission, so its command buffer cannot
            // be reset, its staging cannot be written and its descriptor pool
            // cannot be touched — and the only thing that makes it reusable is
            // its fence, so that is what is waited on.
            self.wait_on(&slots.entries[index])?;
        }
        // Safe exactly where it is, and only here: the fence above has either
        // signalled or was never claimed, so nothing is reading the sets this
        // frees.  See [`Slot::descriptor_pool`] for why there is no other place
        // a reset could go.
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
    /// Consumes the token, so one submission is waited for exactly once. See
    /// [`Token`] for why the second wait has to be inexpressible rather than
    /// merely discouraged.
    ///
    /// This does not need to be called on the thread that submitted, and does
    /// not need the [`Segment`] that submitted — the segment was consumed by
    /// [`Segment::submit`] precisely so the host could go and do something else
    /// in between.
    fn sync(&self, token: Token) -> Result<(), RunError> {
        let mut slots = self.slots.lock().unwrap();
        let slot = slots.entry(token)?;
        self.wait_on(slot)?;
        slots.entries[token.0 as usize].claimed = false;
        Ok(())
    }

    /// Run one fragment over the index range `[0, count)`.
    ///
    /// Returns one [`ResidentId`] per declared output, owned by the caller until
    /// it releases them.  **Nothing is copied back here:** the results are where
    /// the shader ran, and [`Self::fetch`] is what brings them home.  A run whose
    /// outputs are fed to another run — or never read at all — pays for no
    /// transfer down.
    ///
    /// Literally [`Self::submit`] then a wait, and the shared half is
    /// [`Self::stage_run`] so the two cannot drift apart.
    pub fn run(
        &self,
        fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, RunError> {
        let staged = self.stage_run(fragment, inputs, count)?;
        // The upload targets are recycled after the wait rather than in a
        // `finally`, so a wait that fails does not leave them unreachable — and
        // a fence that has not signalled in thirty seconds is a device that has
        // stopped, so handing its memory back is the only honest thing left.
        let waited = self.sync(staged.token);
        for buffer in staged.scratch {
            self.recycle(buffer);
        }
        waited?;
        Ok(staged.ids)
    }

    /// [`Self::run`], handed back **before the device is done**.
    ///
    /// This is the one place the host can be ahead of the device, and it is only
    /// a place: whether the device is still busy when the next thing is recorded
    /// is a property of the device and of how much work the host does in
    /// between, which is the caller's measurement to make and not this
    /// function's promise.
    ///
    /// The returned ids name buffers the shader **has not been guaranteed to
    /// have written**. They are for handing to the next run as
    /// [`BufferSlot::Resident`], and a run that needs their values waits first —
    /// see [`GpuPending`].
    pub fn submit(
        &self,
        fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<GpuPending<'_>, RunError> {
        let staged = self.stage_run(fragment, inputs, count)?;
        Ok(GpuPending {
            context: self,
            token: staged.token,
            scratch: Some(staged.scratch),
            ids: staged.ids,
            finished: false,
        })
    }

    /// Record one run and hand it to the queue, leaving three things for the
    /// caller to finish: the token that waits for it, the ids it will produce,
    /// and the scratch its host inputs were staged through.
    ///
    /// The scratch is the reason this is a separate method. Those buffers are
    /// not resident, so nothing else can name them and **this submission is the
    /// only thing keeping them alive** — the device is copying out of one of
    /// them right now. A caller that waits has to hold them across the wait, and
    /// a caller that does not has to hold them until someone does. Neither
    /// [`Self::run`] nor [`Self::submit`] can let the buffer pool see them before
    /// then, so they travel in the return value.
    fn stage_run(
        &self,
        fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Staged, RunError> {
        // A buffer's width is that buffer's own class, read by [`spirv::buffer_class_of`].
        //
        // **The device path pushes no leaf at all**, so a parameter a body
        // *reads* has nowhere for its value to arrive: refused by name rather
        // than dispatched with a leaf missing, which would compute every lane
        // from the wrong value (`docs/notes/compute-runtime-scalars.md` §3).
        //
        // **A leaf the body never reads is not missing**, and that is the whole
        // of the condition: a parameter declares its leaves whether or not the
        // body names one, so an input group's filler — or any other declared leaf
        // the body does not read — is a leaf nothing demands
        // (`docs/notes/compute-buffer-wrapper.md`).  The emitter draws the same
        // line at [`spirv::SpirvRefusal::NonIndexParameter`].
        let leaves = fragment.param_shape.flat_arity();
        let index = spirv::index_local(fragment);
        // A parameter read is an operand naming a **block parameter**, so a
        // dispatch pushes the extent and the index and nothing else: any
        // operand naming a *different* entry parameter is a leaf with no
        // value to arrive.
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
            // Only a host slot can be too short; a resident one already holds what
            // an earlier run put there, and its length is that run's business.
            //
            // **The width is this buffer's own class**, so a fragment that reads an
            // `Int` buffer and a `Float` one checks each against its own width
            // rather than against one module-wide answer.
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
        let pipeline = self.pipeline(fragment, binding)?;

        // Round up so the last workgroup's surplus lanes address padding rather
        // than memory past the end; see the module docs.
        let padded = count.div_ceil(LOCAL_SIZE_X as usize) * LOCAL_SIZE_X as usize;
        if padded == 0 {
            // A zero-element run would ask Vulkan for a zero-sized buffer, which
            // is not a buffer.  Refused by name: "no indices" is a program that
            // has nothing to dispatch, and saying so beats a driver error.
            return Err(RunError::EmptyRun);
        }
        // The slot is taken before anything is staged into it, and the order is
        // the point: the staging a run writes is the *slot's* staging, so a run
        // that staged first and acquired second would be writing into a mapping
        // some other submission is still copying out of.  Acquiring first makes
        // "this run's upload is somewhere the device is not reading" true by
        // construction rather than by the caller remembering it.
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
                // Used where it lies: this is the whole point of a resident id,
                // and it is why chaining two runs costs one upload, not two.
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

        // The outputs are what the caller keeps, so they are the buffers that
        // become resident.  They go into the same descriptor list: the binding
        // layout is inputs-then-outputs in one set, so a run's outputs occupy the
        // slots after its inputs rather than a second set.
        //
        // **Nothing is written into them here.**  The emitter emits a write only
        // where every invocation reaches it — a loop body's write is refused by
        // name — so every invocation reaches its `BufferWriteCall` and stores, and
        // the dispatch covers `[0, padded)`.  A zero-fill would be a second pass
        // over memory the shader is about to overwrite in full; `spirv`'s module
        // docs carry this invariant.
        // Each output is allocated and recorded at its own class, the width a fetch reads it back at.
        let output_classes: Vec<ScalarClass> = (0..binding.outputs)
            .map(|ordinal| spirv::buffer_class_of(fragment, binding.inputs + ordinal, class))
            .collect();
        for buffer_class in &output_classes {
            let buffer = self.allocate(padded, *buffer_class)?;
            descriptors.push(buffer.descriptor());
            scratch.buffers.push(buffer);
        }

        segment.begin()?;
        segment.record(pipeline, &descriptors, &uploads, count)?;
        // Nothing below this can fail, which is why the buffer bookkeeping comes
        // after the submit rather than before it: a failed submit means nothing
        // reached the queue, and the guard still owns every buffer, so they all
        // go back to the pool.  After it, the outputs become ids and the upload
        // targets become the submission's to hold.
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
                    // The class this output's element type was emitted for, so a
                    // fetch reads it back at the width it was written at rather
                    // than at another buffer's.
                    class: output_classes[ordinal],
                },
            );
            ids.push(id);
        }
        // The guard is now empty and drops to nothing; whatever is left is the
        // host upload targets, which the device is reading through.
        let scratch = std::mem::take(&mut scratch.buffers);
        drop(resident);
        Ok(Staged {
            token,
            ids,
            scratch,
        })
    }

    /// Record `links` dispatches into **one** command buffer, submit once, wait
    /// once, and hand back the last link's output.
    ///
    /// Each link consumes the previous link's result, over one `fragment` and
    /// one `count`, with the first link's input uploaded once from the host. So
    /// the shape is a linear chain, and that is a real restriction: a graph
    /// names every node's inputs and may fan out, which this cannot express.
    /// What it is for is the thing a graph would sit on top of — proving that
    /// several dispatches can share a submission, which is the first time the
    /// one-set-per-dispatch rule and the widened trailing barrier are actually
    /// executed rather than merely written down.
    ///
    /// **What it saves is bounded, and the bound is measured**: one submit and
    /// one fence wait instead of `links` of each, worth 65% to 81% of a 16-link
    /// chain at counts up to 65 536 and 10.7% at a million. See
    /// [lichen-compute-gpu.md](../../docs/notes/lichen-compute-gpu.md).
    pub fn run_chain(
        &self,
        fragment: &KernelFragment,
        input: &[u8],
        count: usize,
        links: usize,
    ) -> Result<ResidentId, RunError> {
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
            fragment,
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

        // One slot, and its staging is sized for the **whole chain** up front.
        // Growing it after recording has begun would be a use-after-write on the
        // mapping: the device reads staging during the submission, and
        // reallocating the buffer unmaps the memory a recorded copy still points
        // at.  Acquiring the slot first is what makes that the only thing that can
        // happen — the slot is not released until the fence has signalled.
        let mut segment = self.acquire()?;
        segment.reserve(self, data_bytes)?;

        let mut scratch = ScratchGuard {
            context: self,
            buffers: Vec::with_capacity(1),
        };
        let source = self.allocate(padded, input_class)?;
        scratch.buffers.push(source);
        // SAFETY: `reserve` sized and mapped this slot's staging for `count`
        // elements before anything was recorded, and the copy below writes
        // exactly that many bytes. Nothing is in flight on this slot.
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

        // **Two output buffers, ping-ponged, not one per link.** A link reads
        // the buffer the link before it wrote, so the one before *that* is dead.
        // For a chain recorded into one command buffer that is the difference
        // between two buffers and sixteen, and the one-per-link version is not
        // merely bigger, it is **slower**: a pool that has to hold a buffer per
        // link is empty at the start of every call, so every call allocates the
        // lot and throws most of it straight away again. Measured with the cap
        // raised to cover it, the one-per-link chain cost 2.226 ms against
        // 0.115 ms — three times *slower* than not fusing at all, entirely in
        // `vkAllocateMemory`.
        //
        // What this costs is a hazard the other version does not have: link `i`
        // reads the buffer link `i + 2` writes. That is write-after-read, and the
        // trailing barrier in `record_dispatch` is widened to order it.
        //
        // It holds **because this is a linear chain**. A fan-out needs two live
        // buffers at once, and then which can be shared is a question about
        // liveness, which is the graph's to answer — see the memory note in the
        // graph design.
        let first = self.allocate(padded, output_class)?;
        let second = self.allocate(padded, output_class)?;

        // One reset for the whole submission and **none** inside it, and it
        // happened in `acquire` rather than here: a reset frees every set,
        // including the ones an earlier dispatch in this same command buffer is
        // still bound to, and those reads happen at execution time, long after
        // the recording.
        segment.begin()?;

        let mut current = source;
        let (mut into, mut onto) = (first, second);
        for step in 0..links {
            let descriptors = [current.descriptor(), into.descriptor()];
            // Only the first link uploads. Later links read a buffer an earlier
            // link's shader wrote, and the barrier between them is the trailing
            // one in `record_dispatch`, not a transfer barrier.
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

        // The last link's output is the answer and becomes resident; the uploaded
        // source is scratch and so is whichever ping-pong buffer the answer did
        // not land in. Two buffers go back to the pool, not one per link.
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

    /// A fresh device-local buffer for `padded` elements of `class`, from the
    /// recycled pool when one of that size and class is free.
    ///
    /// Its contents are undefined until something writes them, and every consumer
    /// writes all of it — see [`GpuContext::recycled`]. A run fills its outputs
    /// before the dispatch, and an upload's staging is copied over the whole real
    /// range with the tail cleared on the device.
    ///
    /// The class is part of what a buffer *is* — its byte size and the width a
    /// fetch reads it back at — so the pool is keyed on both and a float run never
    /// draws an integer-sized allocation.
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
            // Past the cap the memory is genuinely the caller's again, so it goes
            // back to the driver rather than sitting here for a run that may
            // never come.
            drop(recycled);
            buffer.destroy(&self.device);
        }
    }

    /// The first `count` elements of a resident buffer, as host data.
    ///
    /// A submit and a wait of its own, which is the point of the method: a run
    /// whose results nobody asks for never pays for moving them.
    ///
    /// **The class is the buffer's own**, read off the record the run left, so
    /// the copy below is sized at [`ScalarClass::byte_width`] and the elements
    /// come back as the class's own [`ScalarData`] — a float buffer is decoded as
    /// `f32`s rather than handed over as the bits of one.  A fetch has no
    /// fragment to consult, which is why the class travels on the resident record
    /// (`docs/notes/floating-point.md` §3.8, §4.4).
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

        // A fetch takes a slot like anything else, and for the same reason: it is
        // a submission, and a submission needs a command buffer, a fence and a
        // staging buffer of its own.  Reading the staging back out is the part
        // that constrains it — see `submit_and_read_back`.
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
    /// Idempotent, because the host may hold a block-scoped view of a buffer and
    /// release it more than once on some paths; a second release is a no-op rather
    /// than a double free.
    pub fn release(&self, id: ResidentId) {
        let Some(buffer) = self.resident.lock().unwrap().remove(&id) else {
            return;
        };
        self.recycle(buffer);
    }

    /// Reset `slot`'s command buffer and open it for recording.
    ///
    /// Split from the close so one submission can record **more than one**
    /// dispatch: a command buffer is a single recording however many dispatches
    /// go into it, and a chain is exactly that.  A chain is still **one**
    /// submission and one fence: recording several dispatches here is not the
    /// same as having several in flight, and only the second one needs a pool
    /// deeper than one.
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
            // The fence is created unsignalled and left signalled by the wait,
            // so it has to be reset or this would return immediately.  A fence
            // with nobody waiting on it may still be signalled from a submission
            // that has been waited for, so this reset is not optional.
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
    /// The thirty-second bound is a driver-error bound, not a timeout anyone
    /// chose: a fence that has not signalled that long is a device that has
    /// stopped making progress, and returning would hand back buffers the device
    /// is still writing.
    fn wait_on(&self, slot: &Slot) -> Result<(), RunError> {
        check("fence wait", unsafe {
            self.device.wait_for_fences(
                &[slot.fence],
                true,
                std::time::Duration::from_secs(30).as_nanos() as u64,
            )
        })
    }

    /// Record one dispatch into the command buffer that is **already open**.
    ///
    /// There is deliberately no begin and no end here. The caller opened the
    /// recording and will close it, which is what lets a chain put several
    /// dispatches into one submission. Each call still takes **its own**
    /// descriptor set, and that is not tidiness: a set is read when the
    /// submission executes, so one set rewritten between two dispatches would
    /// change what the first one sees — see [`Slot::descriptor_pool`].
    ///
    /// The uploads read out of `slot`'s own staging and nothing else, which is
    /// why the staging handle is not a parameter: passing one separately would
    /// be an invitation to record a copy out of a mapping the device is reading.
    ///
    /// The correctness of a chain rests on the trailing barrier below, whose
    /// destination scope names a shader as well as a transfer. Narrow it to the
    /// transfer alone and a consumer inside the same submission reads undefined
    /// data — not stale data, and not a crash.
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

        // The two layouts are a function of `total` alone, so they are built once
        // per shape rather than per run.  `pipeline` builds the *same* pair, so a
        // pipeline and the sets that bind against it always agree — which is the
        // invariant the old per-run code got for free by building both every time.
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
            // These three only exist for the uploads. A later link of a chain
            // reads a buffer an earlier link's *shader* wrote, and the ordering
            // for that is the trailing barrier below — a transfer barrier with
            // no transfer in it is not just wasted recording, it is a claim
            // about work that did not happen.
            if !uploads.is_empty() {
                // The host wrote staging before this submit, so the copies below
                // are the first reader of it: make that write visible to them.
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
                    // The surplus lanes read past `count`, and that read has to
                    // land on a defined value — so the tail is cleared here, on
                    // the device, rather than uploaded as zeroes the host already
                    // had copies of.
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
                // Both the uploads and the tail fills land in buffers the shader
                // is about to read, so one barrier after them covers both.
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
            // The results stay on the device, so this does not hand them to the
            // host; it makes them visible to whoever reads them next.
            //
            // **The scope is wide on purpose, and it names three readers/writers
            // rather than the two this path was born needing.** A `fetch` in a
            // later submission reads the results as a transfer. A dispatch
            // recorded after this one *in the same command buffer* reads them as
            // a shader. And a chain that hands the same buffer round again —
            // `run_chain` ping-pongs two buffers rather than allocating one per
            // link — has this dispatch **reading** a buffer a later dispatch
            // **writes**, and that is a write-after-read hazard the other two
            // scopes do not order at all.
            //
            // Each of the three is a case where leaving it out does not slow
            // anything down, it makes the chain read undefined data: not stale
            // data, and not a crash. So the scope carries all three, which
            // over-covers the single-dispatch case that is what the `run` path
            // records and under-covers nothing.
            //
            // That over-coverage is a cost, so the first widening was measured
            // rather than assumed: 16 links at 1 048 576 elements cost 7.79 ms
            // before it and 7.17 ms after, and an empty dispatch 0.046 against
            // 0.048, both inside the run-to-run spread of the example that
            // produced them. The reading was that the widening is not
            // measurable — not that it is free.
            //
            // **The write-after-read half above has not been measured that way.**
            // It is only needed by a chain that reuses a buffer, so nothing on
            // the `run` path depends on it, and the runs of the example on this
            // machine span a range wider than any effect it could plausibly
            // have. It is paid here on the strength of the specification, not of
            // a number, and that is worth knowing before anyone treats the
            // barrier above as a cost that was justified by measurement.
            //
            // Note what does *not* imply this barrier is needed: `spirv`'s
            // write-reachability invariant says every invocation reaches its
            // write. It says nothing about ordering between dispatches.
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

    /// The descriptor set layout and pipeline layout for a run binding `total`
    /// storage buffers, built on first use and reused after that.
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

    /// The pipeline for a fragment, built once per content digest.
    ///
    /// The SPIR-V is emitted **only on a cache miss**.  It used to be emitted on
    /// every run, which put a whole-module emission on the critical path of a
    /// dispatch whose pipeline was already built and waiting — pure repeated work
    /// that a cache hit was supposed to have removed.
    fn pipeline(
        &self,
        fragment: &KernelFragment,
        binding: Binding,
    ) -> Result<vk::Pipeline, RunError> {
        // The device gate is derived from the *fragment*, not applied when the
        // device was chosen: an integer fragment's module declares a 64-bit
        // integer and needs `shaderInt64`, and a float one's does not. Checked
        // before the cache so a cached integer pipeline is not served on a device
        // that could never have built it.
        if spirv::needs_int64(fragment).map_err(RunError::Emit)? && !self.shader_int64 {
            return Err(RunError::MissingInt64 {
                device: self.name.clone(),
            });
        }
        let key = (fragment_digest(fragment), binding.inputs, binding.outputs);
        if let Some(pipeline) = self.pipelines.lock().unwrap().get(&key) {
            return Ok(*pipeline);
        }
        let words = spirv::compile(fragment, binding).map_err(RunError::Emit)?;
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
                // Only the module is this call's to destroy; the layouts are
                // cached and shared with every later run of this shape.
                unsafe { self.device.destroy_shader_module(module, None) };
                return Err(RunError::Vulkan {
                    stage: "compute pipeline",
                    detail: format!("{code:?}"),
                });
            }
        };

        // The module goes here because the pipeline has copied what it needs from
        // it.  The layouts stay: they are cached, and destroying them would leave
        // the cache pointing at freed objects.
        unsafe { self.device.destroy_shader_module(module, None) };
        self.pipelines.lock().unwrap().insert(key, pipeline);
        Ok(pipeline)
    }
}

impl Drop for GpuContext {
    fn drop(&mut self) {
        // Nothing may be in flight by the time a context is dropped, and that is
        // an invariant of every path that submits rather than something checked
        // here: `run`, `run_chain` and `fetch` all wait before they return, and
        // the segments that do not are consumed by a wait of their own. Destroying
        // a command buffer or unmapping a buffer under a running submission would
        // be a use-after-free rather than a leak, so this is the one place in
        // `Drop` that has to be believed rather than verified.
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

    /// Delegates to the inherent [`GpuContext::run`], turning a refusal into the
    /// string the trait carries. The inherent method is the same code either way —
    /// this impl exists so the *host program* can install a backend, not so a
    /// second path can run a kernel.
    fn run(
        &self,
        fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, String> {
        GpuContext::run(self, fragment, inputs, count).map_err(|error| error.to_string())
    }

    /// The only backend here that overrides the default, and the reason it can:
    /// it has a pool, so a submission handed back is a **different slot** from
    /// the one the next submission will take, and the host's work in between is
    /// the device's work overlapping rather than waiting.
    fn submit<'backend>(
        &'backend self,
        fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Box<dyn lichen_kernel_ir::Pending + 'backend>, String> {
        // The inherent form, named explicitly: `self.submit` would be the trait
        // method recursively.
        Ok(Box::new(
            GpuContext::submit(self, fragment, inputs, count).map_err(|error| error.to_string())?,
        ))
    }

    /// The class comes off the resident record the run left, so a float
    /// fragment's result is fetched at the width its module declared — and this
    /// is the half that used to be missing: a float module's `ArrayStride` was
    /// emitted at four bytes while everything this path staged, allocated and
    /// read back was sized at eight, so a float fragment could not be run at all
    /// (`docs/notes/floating-point.md` §3.8, §4.4).
    fn fetch(&self, id: ResidentId, count: usize) -> Result<ScalarData, String> {
        GpuContext::fetch(self, id, count).map_err(|error| error.to_string())
    }

    fn release(&self, id: ResidentId) {
        GpuContext::release(self, id);
    }
}

/// Install a device as the process's compute backend, replacing any previous one.
///
/// This is what a **host program** calls — the composition that links both
/// backends together. `lichen-compute` never names this crate, so without this
/// call a program that asked for `"gpu"` is refused by name, which is the honest
/// outcome rather than a silent run on the CPU.
pub fn install(context: GpuContext) {
    lichen_kernel_ir::install_parallel_backend(std::sync::Arc::new(context));
}

/// Open a device and install it, or say why no backend could be installed.
///
/// Device creation is eager on purpose: a program that cannot dispatch at all
/// should learn that when it is wired up, not on its first hot loop. The
/// context itself is not handed back — the registry keeps it alive for as long as
/// it is installed, and a second handle would only be a second way to be stale.
pub fn install_default() -> Result<(), String> {
    let context = GpuContext::new().map_err(|error| error.to_string())?;
    install(context);
    Ok(())
}

/// Whether a backend is installed, and which.
///
/// A host program can print this to report what a run will actually use, rather
/// than leaving it to be inferred from a program's source.
pub fn installed_backend_name() -> Option<&'static str> {
    lichen_kernel_ir::parallel_backend().map(|backend| backend.name())
}

/// Remove the installed backend, if any.
///
/// The counterpart to [`install`], and not merely a convenience: uninstalling
/// **drops the context**, so every device buffer it was holding is released at the
/// same moment.  A test that installs a device and leaves it installed would hand
/// that device — and the memory on it — to whatever ran next.
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
    /// The allocation past [`Self::bytes`], which the surplus lanes read.  It is
    /// zeroed **on the device** rather than uploaded, so a run costs `count`
    /// elements of bus traffic and not `padded` — and the surplus lanes still read
    /// a defined `0` rather than whatever was in the allocation.
    tail: vk::DeviceSize,
}

/// What one run leaves behind for whoever is going to wait for it.
///
/// Produced by [`GpuContext::stage_run`] and consumed by exactly two callers,
/// which is the whole reason it is a struct: the buffers in `scratch` are
/// unreachable by name, so the only correct way to think about them is "they
/// belong to this submission until it is waited for".
struct Staged {
    /// The slot the submission is in, and the only way to wait for it.
    token: Token,
    /// The ids the submission will produce, valid once the token is waited for.
    ids: Vec<ResidentId>,
    /// Buffers a host input was uploaded into. Not resident, so nothing else can
    /// reach them while the copy out of them is in flight.
    scratch: Vec<DeviceBuffer>,
}

/// A run this context has recorded and handed to the queue, and has not waited
/// for.
///
/// It is the implementation of [`lichen_kernel_ir::Pending`] for this backend,
/// and it is what makes a graph's "only wait where something actually needs the
/// data" schedule expressible: between [`GpuContext::submit`] and
/// [`Pending::wait`] the host is doing whatever else the run needs to do, and
/// the device is not being held up waiting to be asked.
///
/// # Dropping it waits
///
/// A dropped submission would leave a claimed slot, a staging buffer the device
/// may be copying out of, and upload targets nothing else can name. So the
/// `Drop` here waits and releases, which makes the mistake a slowdown rather
/// than a use-after-free. It is a backstop and not a mechanism: a run submits
/// and waits inside one call, so a pending that is merely dropped should not
/// exist, and if one does the wait is the wrong-but-safe answer.
pub struct GpuPending<'backend> {
    context: &'backend GpuContext,
    token: Token,
    /// `None` once the submission has been waited for. An `Option` rather than a
    /// flag because the buffers have to be *moved out* to be recycled, and this
    /// type has a `Drop`.
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
        // Recycling an upload target while the copy out of it is in flight would
        // hand the buffer to the next run to write into.
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
        // A wait that fails is a fence that did not signal, which `sync` has
        // already said out loud. Saying it again here would only bury it.
        let _ = self.finish();
    }
}

/// A device-local buffer of `padded` elements of one class.
///
/// Device-local because the shader reads and writes it where the shader runs. A
/// host-visible buffer would put every one of those accesses on the path between
/// the CPU and the GPU, which is the cost this whole shape exists to avoid.
///
/// **The class is part of the record, not of the id.**  A [`ResidentId`] names a
/// buffer and nothing else, so the width a buffer's elements were written at has
/// to travel beside it or a fetch of a float buffer is a fetch at the integer
/// width — the wrong numbers, not a failed shape.  It is the class the fragment
/// declared, so it is the class the module's `ArrayStride` was emitted at
/// ([`spirv::element_stride`]) and the two cannot disagree.
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
/// A run that fails after allocating buffers for its outputs would otherwise leak
/// them with nothing left holding their addresses, and a run that succeeds has
/// already moved its outputs out of here into the ids it hands back.  So this
/// owns exactly the scratch buffers: host upload targets on every path, plus the
/// outputs on the path that never completed.
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
/// One per **slot**, reused and grown: a slot's staging is dead the moment its
/// fence signals, so a fresh mapping per submission would put an allocation and
/// a `map_memory` on the critical path of every dispatch. Per slot rather than
/// per context because "dead the moment its fence signals" is no longer the
/// whole rule — the device may be reading *another* slot's staging right now,
/// and one shared mapping would put this submission's bytes underneath it.
struct Staging {
    handle: vk::Buffer,
    memory: vk::DeviceMemory,
    /// The mapping's base address. Bytes rather than `i64`: this is a staging
    /// byte range, and the alignment Vulkan hands back is what it is.
    mapped: *mut u8,
    capacity: usize,
}

// SAFETY: `Staging` is `Send` because every dereference of `mapped` happens
// while the pool's lock is held — a `Segment` carries that lock for its whole
// life, and a slot is not reachable again until a fence wait has released it —
// so no two threads can touch one mapping at once.  The memory itself is host
// memory the device maps coherently, and the pointer names a fixed offset into
// it, so moving the pointer between threads changes nothing about what it
// addresses.  `Sync` is deliberately not implemented: the pointer makes a shared
// `&Staging` unsound on its own, and `Mutex<Slots>` is `Sync` from
// `Slot: Send` alone, which is all the context needs.
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
        // SAFETY: `offset` is added to the mapping's own base. Every caller
        // bounds it by what `reserve` was told the run needs, and `at` is only
        // reached after a `reserve` that covered it.
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

/// A host-visible, host-coherent **and host-cached** memory type, or a refusal.
///
/// `HOST_CACHED` is required here rather than preferred, and the reason is
/// measured rather than theoretical: on a discrete GPU the uncached
/// host-visible type is system RAM reached over PCIe with no cache behind it, so
/// every host touch on staging costs a round trip.  That is a *correct* buffer
/// and a ruinously slow one, and picking it silently would leave this backend
/// reporting success while losing to the CPU it exists to beat — so a device
/// without cached host memory is refused by name instead.
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
/// The claim is the pool's lock, held for as long as this value lives. That is
/// what makes "two threads never record into one command buffer" a property the
/// borrow checker sees rather than a property the comments promise: a second
/// thread wanting the same slot cannot get past [`GpuContext::acquire`] until
/// this one is gone.
///
/// The three submit methods are the same two-step decision spelled three ways,
/// because the interesting difference is not what they record but **what they
/// leave the caller holding**:
///
/// - [`Self::submit_and_wait`] records, submits, waits, and gives the slot back.
///   This is a run and a `run_chain`: the device is the only thing that matters
///   and the host wants its answer before it does anything else.
/// - [`Self::submit`] records and submits, and **keeps the slot claimed** while
///   handing back a [`Token`]. This is the only path that leaves the device
///   behind the host, and the token is what says which submission is still out.
/// - [`Self::submit_and_read_back`] does the download's own read of the staging
///   while the claim is still held. It cannot be written as `submit` plus
///   [`GpuContext::sync`] followed by a read, because `sync` gives the slot away
///   and the next acquisition of it would be free to overwrite the very bytes
///   being read.
struct Segment<'a> {
    context: &'a GpuContext,
    /// The pool lock, held for this value's whole life.
    slots: std::sync::MutexGuard<'a, Slots>,
    index: usize,
    /// Whether a submission on this slot may still be running.
    ///
    /// Set by [`Self::submit`] on the way out and cleared only by a wait that has
    /// confirmed the fence, so "nothing is in flight" is a fact about the device
    /// and never an assumption about the host.
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
    /// Reallocation unmaps the old memory, so this is only safe while nothing is
    /// reading it. The claim is what guarantees that: the slot was claimed in
    /// `acquire`, which waited out any submission the previous holder left in
    /// flight.
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
    /// Takes `&mut self` rather than consuming the segment, so a caller that
    /// still needs the slot afterwards — to wait on it, or to read the staging it
    /// just filled — can have both. What it cannot do is promise the device is
    /// done; that is [`GpuContext::wait_on`] and nothing else.
    fn hand_to_queue(&mut self) -> Result<(), RunError> {
        self.context.end_and_submit(self.slot())?;
        self.in_flight = true;
        Ok(())
    }

    /// Close the recording, hand it to the queue, and **do not wait**.
    ///
    /// The slot stays claimed, so a second acquisition takes a different one and
    /// the host is free for as long as the device takes.  Waiting is
    /// [`GpuContext::sync`]'s job, and it is a separate call on purpose: the gap
    /// between the two is the entire opportunity the overlapping strategy has,
    /// and an `if wait` parameter would be a way to forget it exists.
    fn submit(mut self) -> Result<Token, RunError> {
        self.hand_to_queue()?;
        Ok(Token(self.index as u32))
    }

    /// Record, submit, wait, and release the slot.
    ///
    /// Literally [`Self::submit`] then [`GpuContext::sync`], which is the shape
    /// every non-overlapping path has: one submission, waited for before the
    /// call returns.
    fn submit_and_wait(self) -> Result<(), RunError> {
        // Read before the move: `submit` consumes the segment, and the context
        // reference is what `sync` needs afterwards.
        let context = self.context;
        let token = self.submit()?;
        context.sync(token)
    }

    /// Submit, wait, and take `count` elements of this slot's staging with us.
    ///
    /// The read is inside the claim on purpose. This cannot be written as
    /// `submit` then [`GpuContext::sync`] followed by a read, because `sync` gives
    /// the slot away and the next acquisition of it would be free to resize or
    /// rewrite the very mapping being copied out of — which returns the *wrong
    /// numbers* rather than failing, so it has to be structurally impossible
    /// rather than merely discouraged.
    ///
    /// `count` is a count of **elements**, so the byte range read is `count * 4`
    /// for a float buffer and `count * 8` for an integer one: the copy above moved
    /// exactly that many bytes, and reading a word per element here would read the
    /// bytes of the two elements after this one.
    fn submit_and_read_back(
        mut self,
        count: usize,
        class: ScalarClass,
    ) -> Result<ScalarData, RunError> {
        self.hand_to_queue()?;
        self.context.wait_on(self.slot())?;
        // The fence has signalled, so nothing is in flight and the claim can go
        // back when this value drops. It is deliberately **not** released here:
        // the read below still reads this slot's memory.
        self.in_flight = false;
        // SAFETY: the slot was reserved for at least this many bytes and mapped
        // before the submit, the wait above is the one that proves the copy
        // filling it has finished, and the copy's own barrier made the write
        // host-visible. The claim is still held, so no other thread can be
        // writing this mapping.
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
        // A segment that submitted leaves the claim alone: the submission may
        // still be running, and the next acquisition of this slot waits its
        // fence — which is exactly what `GpuContext::acquire` is for.
        if !self.in_flight {
            self.slots.entries[self.index].claimed = false;
        }
    }
}

impl Slot {
    /// A command buffer, a fence, a descriptor pool and an unmapped staging
    /// buffer, all of them this submission's own.
    ///
    /// The fence is created unsignalled and reset before every use, so a wait
    /// always waits on *this* slot's last submission and never on a previous
    /// one's.
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
    /// **Only safe when nothing is in flight on it.** A command buffer, a fence
    /// and a mapped buffer all destroyed under a running submission is a
    /// use-after-free, and `GpuContext::drop` relies on the invariant that every
    /// path that submits also waits before returning.
    fn destroy(&mut self, device: &ash::Device) {
        // The staging needs the context, not the device, to unmap; when it has
        // never been reserved it holds nothing and there is nothing to unmap, so
        // the null memory is skipped rather than passed to `unmap_memory`.
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
/// No fallback to a type without the flags.  A buffer in memory the host cannot
/// reach cannot be read back, and quietly choosing one would produce empty
/// results rather than an error, so a missing flag is a refusal that names the
/// flags it wanted.
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
