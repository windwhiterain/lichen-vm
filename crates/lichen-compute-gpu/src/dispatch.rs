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
//! - **Outputs** are not initialised at all. The emitter produces straight-line
//!   code — one `OpLabel`, no branches — so every invocation reaches its write
//!   and the dispatch covers `[0, padded)` in full. A zero-fill would be a second
//!   pass over memory the shader is about to overwrite completely. The surplus
//!   lanes' *writes* land in the padding rather than past the end of it.
//!
//! Only the first `count` elements are ever read back. This keeps the shader
//! branch-free and makes out-of-range access impossible by construction rather
//! than by a runtime test.
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
use lichen_kernel_ir::{BufferSlot, KernelFragment, ResidentId, fragment_digest};

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
    /// A run binds more storage buffers than the descriptor pool is sized for.
    /// The pool is allocated once, so this is a limit of the backend rather than
    /// of the device — refused by name instead of surfacing as pool exhaustion.
    TooManyBindings { total: usize, max: usize },
    /// A resident id this context is not holding — never issued, or already
    /// released.  Refused rather than read as empty: an id is a handle, and using
    /// a dead one means the host lost track of its own buffers, which reporting
    /// `0`s would hide.
    UnknownResident { id: u64 },
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
                "the device {device:?} does not support 64-bit integers in shaders, and the \
                 fragment was lowered to mean 64-bit ones. Narrowing the values would change \
                 what the program computes, so this is refused rather than converted."
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
            RunError::Emit(refusal) => write!(f, "{refusal}"),
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
    pipelines: Mutex<HashMap<(u64, usize, usize), vk::Pipeline>>,
    /// Buffers handed out as [`ResidentId`]s and not yet released. The value is
    /// the device-local allocation behind the id, so the id is the only handle
    /// the host ever holds to device memory.
    resident: Mutex<HashMap<ResidentId, DeviceBuffer>>,
    /// Hands out ids. Never reused: a stale id released twice must not name a
    /// live buffer, so ids only ever move forward.
    next_id: AtomicU64,
    /// Mapped host memory the uploads and downloads are staged through. One per
    /// context and reused, because a run's staging is dead the moment its fence
    /// signals.
    staging: Mutex<Staging>,
    /// One command pool, one command buffer and one fence, reused for every
    /// submission.
    ///
    /// Reuse is safe because submissions are **already serialised**: `run` holds
    /// the staging lock across its dispatch and `fetch` holds it across its copy,
    /// so two submissions can never overlap. The mutex is belt to that braces —
    /// it makes the exclusion local instead of resting on a reader having to
    /// trace two methods to convince themselves the lock is the same one.
    submit: Mutex<Submit>,
    /// A **second** command buffer and fence, for a dispatch whose completion the
    /// caller has not waited for yet — see [`Segment`].
    ///
    /// It is a separate pair rather than a mode of `submit` because the two can
    /// never be in flight at once and for a reason worth stating: a recorded-but-
    /// unsubmitted command buffer is clobbered by the next `record_and_submit`,
    /// so a submission that has not been waited for must not share a command
    /// buffer with one that is about to be recorded. `fetch` records into
    /// `submit`; a [`Segment`] records into this.
    detached: Mutex<Submit>,
    /// The descriptor set layout and pipeline layout for a run with `n` buffers
    /// in total. They are a function of `n` alone — the same two objects the old
    /// per-run code built and dropped for every dispatch.
    layouts: Mutex<HashMap<usize, Layouts>>,
    /// Sized once for [`MAX_DESCRIPTOR_BINDINGS`] and **reset before each run**,
    /// which is what makes one set per dispatch safe without a free: the reset
    /// invalidates the previous run's set, and that run's fence has already
    /// signalled.
    descriptor_pool: vk::DescriptorPool,
    /// Device buffers that have been given back, keyed by their element count.
    ///
    /// Reuse is safe **without clearing**, and that is worth spelling out because
    /// it is what makes this a saving rather than a trade: every consumer of a
    /// buffer writes all of it. An output is written by the shader across
    /// `[0, padded)` — see `spirv`'s straight-line invariant — and an input is
    /// uploaded across `[0, count)` with the tail filled on the device, so no
    /// stale byte is ever read back.
    ///
    /// Keyed by exact count, so a chain at a fixed size is served from here
    /// forever and a different size simply allocates. The cap is what keeps this
    /// from turning a release into a permanent VRAM reservation.
    recycled: Mutex<HashMap<usize, Vec<DeviceBuffer>>>,
}

/// How many freed buffers of one size are kept for reuse.
///
/// Four is enough for a chain to ping-pong two buffers plus a host input and a
/// host output without allocating, and small enough that idling here is not worth
/// measuring against a device's whole memory.
const RECYCLED_PER_SIZE: usize = 4;

/// The reusable objects one submission needs.
struct Submit {
    command: vk::CommandBuffer,
    fence: vk::Fence,
}

/// The two layouts a dispatch binds, which depend only on how many buffers the
/// run has in total.
#[derive(Clone, Copy)]
struct Layouts {
    set: vk::DescriptorSetLayout,
    pipeline: vk::PipelineLayout,
}

/// The most storage-buffer bindings one run may declare.
///
/// The descriptor pool is sized once for this, so a run that binds more is
/// **refused by name** rather than left to fail as a driver-side pool exhaustion
/// that would read like a device problem.
const MAX_DESCRIPTOR_BINDINGS: usize = 32;

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
        let descriptor_pool = check("descriptor pool", unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::STORAGE_BUFFER,
                        descriptor_count: MAX_DESCRIPTOR_BINDINGS as u32,
                    }]),
                None,
            )
        })?;

        // Built before the context exists, because it needs a borrow of the
        // device that the struct literal below is about to move.
        let submit = Submit::new(&device, family)?;
        let detached = Submit::new(&device, family)?;

        Ok(GpuContext {
            entry,
            instance,
            device,
            queue,
            physical,
            name,
            pipelines: Mutex::new(HashMap::new()),
            resident: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            staging: Mutex::new(Staging::empty()),
            submit: Mutex::new(submit),
            detached: Mutex::new(detached),
            layouts: Mutex::new(HashMap::new()),
            descriptor_pool,
            recycled: Mutex::new(HashMap::new()),
        })
    }

    /// The device this context dispatches on.
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// Run one fragment over the index range `[0, count)`.
    ///
    /// Returns one [`ResidentId`] per declared output, owned by the caller until
    /// it releases them.  **Nothing is copied back here:** the results are where
    /// the shader ran, and [`Self::fetch`] is what brings them home.  A run whose
    /// outputs are fed to another run — or never read at all — pays for no
    /// transfer down.
    pub fn run(
        &self,
        fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, RunError> {
        let binding = Binding {
            inputs: inputs.len(),
            outputs: fragment.outputs,
        };
        for (index, slot) in inputs.iter().enumerate() {
            // Only a host slot can be too short; a resident one already holds what
            // an earlier run put there, and its length is that run's business.
            if let BufferSlot::Host(data) = slot
                && data.len() < count
            {
                return Err(RunError::InputShorterThanCount {
                    buffer: index,
                    len: data.len(),
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
        let element = std::mem::size_of::<i64>() as vk::DeviceSize;
        let data_bytes = count as vk::DeviceSize * element;
        let padded_bytes = padded as vk::DeviceSize * element;

        // Staging is held for the whole run: it carries the uploads out and the
        // dispatch itself, and a run's staging is dead the moment its fence
        // signals, so nothing else may write it in between.
        let mut staging = self.staging.lock().unwrap();
        let host_inputs: Vec<&[i64]> = inputs
            .iter()
            .filter_map(|slot| match slot {
                BufferSlot::Host(data) => Some(*data),
                BufferSlot::Resident(_) => None,
            })
            .collect();
        staging.reserve(self, host_inputs.len() as u64 * data_bytes)?;

        let mut scratch = ScratchGuard {
            context: self,
            buffers: Vec::new(),
        };
        let mut descriptors: Vec<vk::DescriptorBufferInfo> = Vec::with_capacity(binding.total());
        let mut uploads: Vec<Transfer> = Vec::with_capacity(host_inputs.len());
        for slot in inputs {
            let buffer = match slot {
                // Used where it lies: this is the whole point of a resident id,
                // and it is why chaining two runs costs one upload, not two.
                BufferSlot::Resident(id) => self.resident_buffer(*id)?,
                BufferSlot::Host(data) => {
                    let buffer = self.allocate(padded)?;
                    let offset = uploads.len() as u64 * data_bytes;
                    // SAFETY: `reserve` sized staging for every host input's `count`
                    // elements, and `offset` counts whole such blocks that come
                    // before this one, so this block lies inside the mapping.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            data.as_ptr(),
                            staging.at(offset) as *mut i64,
                            count,
                        );
                    }
                    uploads.push(Transfer {
                        src_offset: offset,
                        dst: buffer.handle,
                        bytes: data_bytes,
                        tail: padded_bytes - data_bytes,
                    });
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
        // **Nothing is written into them here.**  The emitter produces
        // straight-line code — one `OpLabel`, no branches — so every invocation
        // reaches its `BufferWriteCall` and stores, and the dispatch covers
        // `[0, padded)`.  A zero-fill would be a second pass over memory the
        // shader is about to overwrite in full; `spirv`'s module docs carry this
        // invariant, and a branch in the emitted body would break it.
        for _ in 0..binding.outputs {
            let buffer = self.allocate(padded)?;
            descriptors.push(buffer.descriptor());
            scratch.buffers.push(buffer);
        }

        let outcome = {
            // The submit lock is taken here rather than inside `dispatch` so that
            // the command buffer stays reserved for exactly as long as this
            // submission is in flight, which is what makes a `wait: false`
            // dispatch on the same target safe to hand back to a caller.
            let submit = self.submit.lock().unwrap();
            self.dispatch(
                &submit,
                true,
                pipeline,
                &descriptors,
                staging.handle,
                &uploads,
                count,
            )
        };
        drop(staging);
        outcome?;

        // Success: the output buffers stop being scratch and become the ids the
        // caller is handed.  Nothing can fail from here, and the guard owns
        // whatever is left in `buffers` — the host upload targets — either way.
        let mut resident = self.resident.lock().unwrap();
        let mut ids = Vec::with_capacity(binding.outputs);
        for buffer in scratch
            .buffers
            .split_off(scratch.buffers.len() - binding.outputs)
        {
            let id = ResidentId(self.next_id.fetch_add(1, AtomicOrdering::Relaxed));
            resident.insert(
                id,
                DeviceBuffer {
                    handle: buffer.handle,
                    memory: buffer.memory,
                    padded,
                },
            );
            ids.push(id);
        }
        Ok(ids)
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

    /// A fresh device-local buffer for `padded` elements, from the recycled pool
    /// when one of that size is free.
    ///
    /// Its contents are undefined until something writes them, and every consumer
    /// writes all of it — see [`GpuContext::recycled`]. A run fills its outputs
    /// before the dispatch, and an upload's staging is copied over the whole real
    /// range with the tail cleared on the device.
    fn allocate(&self, padded: usize) -> Result<DeviceBuffer, RunError> {
        if let Some(buffer) = self
            .recycled
            .lock()
            .unwrap()
            .get_mut(&padded)
            .and_then(Vec::pop)
        {
            return Ok(buffer);
        }
        let device = &self.device;
        let bytes = DeviceBuffer::bytes(padded);
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
        })
    }

    /// Hand a device buffer back for reuse, or free it if the pool for its size
    /// is already full.
    fn recycle(&self, buffer: DeviceBuffer) {
        let mut recycled = self.recycled.lock().unwrap();
        let pool = recycled.entry(buffer.padded).or_default();
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
    pub fn fetch(&self, id: ResidentId, count: usize) -> Result<Vec<i64>, RunError> {
        let buffer = self.resident_buffer(id)?;
        if count > buffer.padded {
            return Err(RunError::FetchLongerThanBuffer {
                len: buffer.padded,
                count,
            });
        }
        let bytes = (count * std::mem::size_of::<i64>()) as vk::DeviceSize;
        let device = &self.device;

        let mut staging = self.staging.lock().unwrap();
        staging.reserve(self, bytes)?;

        let source = buffer.handle;
        let target = staging.handle;
        self.record_and_wait(|command| unsafe {
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
        })?;

        // SAFETY: staging was reserved for at least `bytes` and mapped before the
        // submit; the fence above waited for the copy that filled it, and the
        // barrier made the write host-visible. `count` elements are read from a
        // mapping that long.
        let data = unsafe { std::slice::from_raw_parts(staging.at(0) as *const i64, count) };
        Ok(data.to_vec())
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

    /// Record one command buffer, submit it, and wait for it to finish.
    ///
    /// The command buffer and fence belong to the context and are reset here, so
    /// a dispatch pays two reset calls rather than three object create/destroy
    /// pairs.  Every call is serialised by [`Self::submit`], so reusing both is
    /// sound: there is never a second submission in flight to trample.
    fn record_and_wait(&self, record: impl FnOnce(vk::CommandBuffer)) -> Result<(), RunError> {
        let submit = self.submit.lock().unwrap();
        self.record_and_submit(&submit, record)?;
        self.wait_on(&submit)
    }

    /// Reset `target`'s command buffer, record into it, and submit it — **without
    /// waiting for the fence**.
    ///
    /// Split from [`Self::wait_on`] so a caller can hold the gap open: a dispatch
    /// that is submitted and not waited for is *in flight*, and the host is free
    /// for that whole window. Whether the host actually gets anything back from
    /// that window is a property of the device, not of this function, which is why
    /// it is the caller's measurement to make.
    ///
    /// The caller must hold whatever locks make the recorded commands valid until
    /// the wait: the staging mapping the uploads read, and the command buffer
    /// itself, which the next `record_and_submit` into the same target would
    /// reset out from under this submission.
    fn record_and_submit(
        &self,
        target: &Submit,
        record: impl FnOnce(vk::CommandBuffer),
    ) -> Result<(), RunError> {
        let device = &self.device;
        let command = target.command;
        let fence = target.fence;
        check("command buffer reset", unsafe {
            device.reset_command_buffer(command, vk::CommandBufferResetFlags::empty())
        })
        .and_then(|()| {
            check("command recording", unsafe {
                device.begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())
            })
        })?;

        record(command);

        check("command buffer end", unsafe {
            device.end_command_buffer(command)
        })
        .and_then(|()| {
            // The fence is created unsignalled and left signalled by the wait,
            // so it has to be reset or this would return immediately.
            check("fence reset", unsafe { device.reset_fences(&[fence]) })?;
            check("queue submit", unsafe {
                device.queue_submit(
                    self.queue,
                    &[vk::SubmitInfo::default().command_buffers(&[command])],
                    fence,
                )
            })
        })
    }

    /// Block until `target`'s last submission has finished.
    ///
    /// The thirty-second bound is a driver-error bound, not a timeout anyone
    /// chose: a fence that has not signalled that long is a device that has
    /// stopped making progress, and returning would hand back buffers the device
    /// is still writing.
    fn wait_on(&self, target: &Submit) -> Result<(), RunError> {
        check("fence wait", unsafe {
            self.device.wait_for_fences(
                &[target.fence],
                true,
                std::time::Duration::from_secs(30).as_nanos() as u64,
            )
        })
    }

    /// Record and submit one dispatch into `target`, waiting for it to finish
    /// only when `wait` says so.
    ///
    /// `staging` is the caller's already-locked staging buffer: the uploads read
    /// from it, so this cannot take the lock itself.  The same is true of
    /// `target` — the caller holds it, so that a `wait: false` dispatch keeps its
    /// command buffer reserved for as long as the submission is in flight.
    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &self,
        target: &Submit,
        wait: bool,
        pipeline: vk::Pipeline,
        descriptors: &[vk::DescriptorBufferInfo],
        staging: vk::Buffer,
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

        // The two layouts are a function of `total` alone, so they are built once
        // per shape rather than per run.  `pipeline` builds the *same* pair, so a
        // pipeline and the sets that bind against it always agree — which is the
        // invariant the old per-run code got for free by building both every time.
        let layouts = self.layouts(total)?;

        // The pool is reset rather than recreated: it only ever holds the one set
        // of the run in flight, and that run's set is dead the moment its fence
        // signalled.
        check("descriptor pool reset", unsafe {
            device
                .reset_descriptor_pool(self.descriptor_pool, vk::DescriptorPoolResetFlags::empty())
        })?;
        let set_layout = layouts.set;
        let mut set = vk::DescriptorSet::null();
        check("descriptor set allocation", unsafe {
            device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo {
                // The count defaults to zero, and a zero-count request
                // *succeeds* while allocating nothing — so it is stated.
                descriptor_set_count: 1,
                descriptor_pool: self.descriptor_pool,
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

        self.record_and_submit(target, |command| unsafe {
            // The host wrote staging before this submit, so the copies below are
            // the first reader of it: make that write visible to them.
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
                // The surplus lanes read past `count`, and that read has to land
                // on a defined value — so the tail is cleared here, on the
                // device, rather than uploaded as zeroes the host already had
                // copies of.
                if transfer.tail > 0 {
                    device.cmd_fill_buffer(command, transfer.dst, transfer.bytes, transfer.tail, 0);
                }
            }
            // Both the uploads and the tail fills land in buffers the shader is
            // about to read, so one barrier after them covers both.
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
            // host — it makes them visible to the `fetch` that may read them
            // later, which is a separate submission.
            //
            // **`TRANSFER_READ` is load-bearing, and it follows from "a separate
            // submission".**  It is the right scope mask only while every dispatch
            // in a command buffer is followed by a `fetch` in another one.
            // Recording a *second* dispatch into this same command buffer would
            // read these results as a shader, and the mask above would then be the
            // wrong scope for that read: a consumer inside one submission wants
            // `SHADER_READ` and needs a barrier that says so.  Nothing records two
            // dispatches into one command buffer today, so nothing exercises that
            // barrier, and `spirv`'s single-`OpLabel` invariant does not imply it.
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
        })?;
        if wait {
            self.wait_on(target)?;
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
        for (_, pipeline) in self.pipelines.lock().unwrap().drain() {
            unsafe { self.device.destroy_pipeline(pipeline, None) };
        }
        // Buffers the host never released: the context is going away, so the
        // device memory goes with it.  Their ids die with the registry, which is
        // why a stale id can never name them afterwards.
        for (_, buffer) in self.resident.lock().unwrap().drain() {
            buffer.destroy(&self.device);
        }
        for (_, pool) in self.recycled.lock().unwrap().drain() {
            for buffer in pool {
                buffer.destroy(&self.device);
            }
        }
        self.staging.lock().unwrap().destroy(self);
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

    fn fetch(&self, id: ResidentId, count: usize) -> Result<Vec<i64>, String> {
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

/// A device-local buffer of `i64` elements.
///
/// Device-local because the shader reads and writes it where the shader runs. A
/// host-visible buffer would put every one of those accesses on the path between
/// the CPU and the GPU, which is the cost this whole shape exists to avoid.
#[derive(Clone, Copy)]
struct DeviceBuffer {
    handle: vk::Buffer,
    memory: vk::DeviceMemory,
    /// The buffer's capacity in elements — the run's *padded* count, not the
    /// count the caller asked about.
    padded: usize,
}

impl DeviceBuffer {
    fn bytes(padded: usize) -> vk::DeviceSize {
        (padded * std::mem::size_of::<i64>()) as vk::DeviceSize
    }

    fn descriptor(&self) -> vk::DescriptorBufferInfo {
        vk::DescriptorBufferInfo {
            buffer: self.handle,
            offset: 0,
            range: Self::bytes(self.padded),
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
/// One per context, reused and grown: a run's staging is dead the moment its
/// fence signals, so a fresh mapping per run would put an allocation and a
/// `map_memory` on the critical path of every dispatch.
struct Staging {
    handle: vk::Buffer,
    memory: vk::DeviceMemory,
    /// The mapping's base address. Bytes rather than `i64`: this is a staging
    /// byte range, and the alignment Vulkan hands back is what it is.
    mapped: *mut u8,
    capacity: usize,
}

// SAFETY: `Staging` is `Send` because every dereference of `mapped` happens
// while the context's staging mutex is held — `run` and `fetch` each take it for
// the whole span in which they read or write the mapping, so no two threads can
// touch it at once.  The memory itself is host memory the device maps
// coherently, and the pointer names a fixed offset into it, so moving the
// pointer between threads changes nothing about what it addresses.  `Sync` is
// deliberately not implemented: the pointer makes a shared `&Staging`
// unsound on its own, and `Mutex<Staging>` is `Sync` from `Staging: Send`
// alone, which is all the context needs.
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

impl Submit {
    /// A command buffer and a fence, both created unsignalled and reset before
    /// every use, so a submission always waits on *this* submission.
    fn new(device: &ash::Device, family: u32) -> Result<Self, RunError> {
        let pool = check("command pool", unsafe {
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
                command_pool: pool,
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
            unsafe { device.destroy_command_pool(pool, None) };
            return Err(error);
        }
        // The pool is kept alive by the command buffer's allocation, which is
        // freed with the device; the handle is not needed again.
        let fence = check("fence", unsafe {
            device.create_fence(&vk::FenceCreateInfo::default(), None)
        })?;
        Ok(Submit { command, fence })
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
