//! What a device is, as the facts a fragment's correctness depends on.
//! See `docs/notes/taking-a-kernel-to-a-server.md`.

use lichen_kernel_ir::{KernelFragment, Plan, Space};

/// The facts a device reports, which are what a fragment is checked against.
///
/// # Invariant
/// **Every member is something a fragment can require and this device could fail to
/// provide**, and nothing else: a fact no fragment is refused for would be a number this
/// crate collects and never reads. Each answers one assumption
/// [`lichen_kernel_ir::TargetFacts`] carries as data rather than taking from a backend's
/// constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceFacts {
    /// The widest integer a shader on this device represents natively.
    ///
    /// # Invariant
    /// `IntWidth::I64` needs 64 and anything narrower needs no feature at all, so this
    /// is a floor rather than a choice: a device answering 32 has no 64-bit integer for
    /// a fragment declaring one, and nothing else in the module can stand in for it.
    pub int_bits: u32,
    /// The most lanes one workgroup may run.
    ///
    /// # Invariant
    /// A ceiling, not the width a dispatch uses: `TargetFacts::lane_width` is the host
    /// compiler's choice and a device that cannot schedule that many lanes has no way to
    /// honour it. Refused rather than rounded down, which would run a different program.
    pub max_workgroup_lanes: u32,
    /// The most rows one workgroup may run.
    ///
    /// # Invariant
    /// The second axis of the same question, read rather than left out: a device reports
    /// `maxComputeWorkGroupSize` as three numbers, and reading `[0]` alone gives a height
    /// no ceiling at all, so `TargetFacts::lane_height` would be refused against nothing
    /// (`docs/notes/two-dimensional-workgroup.md` §1.2).
    pub max_workgroup_lanes_y: u32,
    /// The lanes one subgroup runs, or `None` where the device reported none.
    ///
    /// # Invariant
    /// **The one fact a module cannot carry**: `OpExecutionMode SubgroupSize` needs
    /// Vulkan 1.3 and this target emits SPIR-V 1.3, so a `Space::Warp` slice's width is
    /// an assumption about the device — and a narrower subgroup answers **undefined**
    /// rather than wrong, which `spirv-val` cannot see (`docs/notes/shared-memory-and-
    /// barriers.md` §5). `None` is its own case.
    pub subgroup_width: Option<u32>,
    /// The bytes one workgroup's shared memory may hold.
    ///
    /// # Invariant
    /// A `Space::Shared` slice is `Workgroup` storage the module declares, and nothing
    /// reading the module can see whether the declaration fits: the driver finds out at
    /// the dispatch, which is why the check belongs before it.
    pub max_shared_bytes: u32,
    /// The storage buffers one stage of this device may bind.
    ///
    /// # Invariant
    /// Every buffer a fragment reads or writes is one `StorageBuffer` binding in a single
    /// descriptor set, so `inputs + outputs` is a number this limit answers, and a
    /// fragment asking for more is a fragment no pipeline on this device can be built for.
    pub max_storage_buffers: u32,
}

/// Where a device's facts come from.
///
/// # Invariant
/// **Declared here rather than on `ParallelBackend`**: what a device *is* is not part of
/// dispatching a fragment, and a device query on that contract would make every backend
/// answer a question the CPU path has no device for. A source is anything a host has.
pub trait DeviceFactsSource {
    /// What this device is.
    fn device_facts(&self) -> DeviceFacts;
}

/// Why a device cannot honour what a fragment assumes.
///
/// # Invariant
/// One variant per way the two disagree, and each names **both numbers**: an artifact and
/// a device are two facts that only mean something against each other, so a refusal
/// stating one of them leaves the caller to guess which was wrong. The one exception is a
/// fact **no device can change**, which names the value this build supports instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceRefusal {
    /// The device has no integer of the width the fragment was lowered to mean.
    IntWidthNotRepresentable {
        /// The width the fragment declared.
        asked: u32,
        /// The widest integer this device represents.
        offered: u32,
    },
    /// The device cannot schedule one workgroup of this many lanes.
    LaneWidthTooWide {
        /// The lane count one row of a workgroup runs.
        asked: u32,
        /// The most lanes one row of a workgroup may run here.
        widest: u32,
    },
    /// The device cannot schedule one workgroup of this many rows.
    ///
    /// # Invariant
    /// The `y` axis of the ceiling a device already reports: read before the row count is
    /// refused for being more than one, so a height past the device names the device.
    LaneHeightTooWide {
        /// The row count one workgroup runs.
        asked: u32,
        /// The most rows one workgroup may run here.
        tallest: u32,
    },
    /// The workgroup is taller than one row, and nothing in this build schedules one.
    ///
    /// # Invariant
    /// **A fact about this build rather than about a device**, so the second number is the
    /// one row every backend here runs: a rectangle is a legal module, but the launch
    /// rounds the count up by the `x` axis alone, so the rows no lane covers are indices
    /// nothing writes, with no error and no report line
    /// (`docs/notes/two-dimensional-workgroup.md` §4).
    WorkgroupTallerThanOneRow {
        /// The row count one workgroup runs.
        asked: u32,
        /// The row count a workgroup runs wherever this build schedules one.
        one_row: u32,
    },
    /// A `Warp` slice is not one element per lane of this device's subgroup.
    ///
    /// # Invariant
    /// Both directions are refused: a wider slice names a lane the reader cannot reach
    /// and a narrower one a lane its siblings also staged, so neither is a slower program
    /// but a different one (`docs/notes/shared-memory-and-barriers.md` §5).
    SubgroupWidthMismatch {
        /// The slice's element count.
        count: u32,
        /// The lanes one subgroup runs here.
        subgroup_width: u32,
    },
    /// The device did not report a subgroup width, so a `Warp` slice cannot be honoured.
    SubgroupWidthNotReported,
    /// The fragment's `Workgroup` arrays do not fit this device's shared memory.
    SharedMemoryTooLarge {
        /// The bytes the fragment's slices ask for.
        asked: u64,
        /// The bytes one workgroup may hold here.
        offered: u32,
    },
    /// More buffers than one stage of this device binds.
    TooManyBuffers {
        /// The bindings the fragment reads and writes.
        asked: usize,
        /// The storage buffers one stage offers.
        max: u32,
    },
    /// A plan staged a `Warp` value at a width this device's subgroups do not run.
    StagingWidthNotASubgroup {
        /// The name the plan used.
        name: String,
        /// The group width the plan asked for.
        asked: u32,
        /// The lanes one subgroup runs here.
        subgroup_width: u32,
    },
}

impl DeviceRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// The refusal's name, for a report that has to say which check refused an artifact.
    ///
    /// # Invariant
    /// One variant, one name, as `PlanRefusal`'s is: a report printing the sentence alone
    /// could not be matched against the variant a caller matches on, so the two would drift.
    pub fn name(&self) -> &'static str {
        match self {
            DeviceRefusal::IntWidthNotRepresentable { .. } => "IntWidthNotRepresentable",
            DeviceRefusal::LaneWidthTooWide { .. } => "LaneWidthTooWide",
            DeviceRefusal::LaneHeightTooWide { .. } => "LaneHeightTooWide",
            DeviceRefusal::WorkgroupTallerThanOneRow { .. } => "WorkgroupTallerThanOneRow",
            DeviceRefusal::SubgroupWidthMismatch { .. } => "SubgroupWidthMismatch",
            DeviceRefusal::SubgroupWidthNotReported => "SubgroupWidthNotReported",
            DeviceRefusal::SharedMemoryTooLarge { .. } => "SharedMemoryTooLarge",
            DeviceRefusal::TooManyBuffers { .. } => "TooManyBuffers",
            DeviceRefusal::StagingWidthNotASubgroup { .. } => "StagingWidthNotASubgroup",
        }
    }
}

impl std::fmt::Display for DeviceRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceRefusal::IntWidthNotRepresentable { asked, offered } => write!(
                formatter,
                "the fragment declares {asked}-bit integers and this device's widest is {offered}, \
                 so the module's own integer type has no representation here: a fragment is \
                 narrowed to fit rather than refused and it computes a different program \
                 (`docs/notes/taking-a-kernel-to-a-server.md`)"
            ),
            DeviceRefusal::LaneWidthTooWide { asked, widest } => write!(
                formatter,
                "one row of a workgroup runs {asked} lane(s) and this device schedules at most \
                 {widest}, so the fragment's `LocalSize` is one no workgroup here can be: it is \
                 refused rather than rounded down, which would run a different program \
                 (`docs/notes/taking-a-kernel-to-a-server.md`)"
            ),
            DeviceRefusal::LaneHeightTooWide { asked, tallest } => write!(
                formatter,
                "one workgroup runs {asked} row(s) and this device schedules at most {tallest}, \
                 so the fragment's `LocalSize` is one no workgroup here can be: it is refused \
                 rather than rounded down, which would run a different program \
                 (`docs/notes/taking-a-kernel-to-a-server.md`)"
            ),
            DeviceRefusal::WorkgroupTallerThanOneRow { asked, one_row } => write!(
                formatter,
                "one workgroup runs {asked} row(s) and this build schedules {one_row} row(s) per \
                 workgroup. A rectangle is a legal module — `OpExecutionModeLocalSize` takes three \
                 literals — but the dispatch rounds the count up by the `x` axis alone, so the \
                 rows no lane covers are indices nothing writes: no error, no refusal and no \
                 report line (`docs/notes/two-dimensional-workgroup.md` §4)"
            ),
            DeviceRefusal::SubgroupWidthMismatch {
                count,
                subgroup_width,
            } => write!(
                formatter,
                "a `Warp` slice stages {count} element(s) and one subgroup of this device runs \
                 {subgroup_width} lane(s). The exchange reads the value a named lane of the \
                 reader's own subgroup staged, so a wider slice names a lane the reader cannot \
                 reach and a narrower one a lane its siblings also staged — undefined rather \
                 than merely wrong, in a way `spirv-val` cannot see \
                 (`docs/notes/shared-memory-and-barriers.md` §5)"
            ),
            DeviceRefusal::SubgroupWidthNotReported => write!(
                formatter,
                "a `Warp` slice needs the width one subgroup runs, and this device did not \
                 report one. The module cannot state it — `OpExecutionMode SubgroupSize` needs \
                 Vulkan 1.3 — so a width nobody was told is a width nobody may assume \
                 (`docs/notes/shared-memory-and-barriers.md` §5)"
            ),
            DeviceRefusal::SharedMemoryTooLarge { asked, offered } => write!(
                formatter,
                "the fragment's `Shared` slices ask for {asked} byte(s) of workgroup memory and \
                 this device offers {offered}. The arrays are declared by the module, which \
                 nothing reading it can check; the driver finds out at the dispatch, so it is \
                 refused here instead (`docs/notes/taking-a-kernel-to-a-server.md`)"
            ),
            DeviceRefusal::TooManyBuffers { asked, max } => write!(
                formatter,
                "the fragment binds {asked} storage buffer(s) in one descriptor set and one \
                 stage of this device offers {max}, so no pipeline for it can be built here \
                 (`docs/notes/taking-a-kernel-to-a-server.md`)"
            ),
            DeviceRefusal::StagingWidthNotASubgroup {
                name,
                asked,
                subgroup_width,
            } => write!(
                formatter,
                "the plan stages `{name}` in a `Warp` group of {asked} lane(s) and one subgroup \
                 of this device runs {subgroup_width}: the plan checked its width against the \
                 fragment's workgroup, and the subgroup is the narrower group this placement \
                 actually exchanges in (`docs/notes/shared-memory-and-barriers.md` §5)"
            ),
        }
    }
}

impl DeviceFacts {
    /// Whether this device can run a fragment under `plan`, or the first fact it cannot honour.
    ///
    /// # Invariant
    /// The fragment's own gates have already run: `plan` is a proposal `check_plan`
    /// accepted, so each entry names a staging and the space it states is the fragment's
    /// own. The order is cheapest first, so the number a refusal carries is the first
    /// disagreement rather than whichever walk finished last.
    pub fn check(&self, fragment: &KernelFragment, plan: &Plan) -> Result<(), DeviceRefusal> {
        let asked = fragment.target.int_width.bits();
        if asked > self.int_bits {
            return Err(DeviceRefusal::IntWidthNotRepresentable {
                asked,
                offered: self.int_bits,
            });
        }
        let lanes = fragment.target.lane_width.lanes();
        if lanes > self.max_workgroup_lanes {
            return Err(DeviceRefusal::LaneWidthTooWide {
                asked: lanes,
                widest: self.max_workgroup_lanes,
            });
        }
        let rows = fragment.target.lane_height.rows();
        if rows > self.max_workgroup_lanes_y {
            return Err(DeviceRefusal::LaneHeightTooWide {
                asked: rows,
                tallest: self.max_workgroup_lanes_y,
            });
        }
        if rows != 1 {
            return Err(DeviceRefusal::WorkgroupTallerThanOneRow {
                asked: rows,
                one_row: 1,
            });
        }
        let buffers = fragment.inputs + fragment.outputs;
        if buffers > self.max_storage_buffers as usize {
            return Err(DeviceRefusal::TooManyBuffers {
                asked: buffers,
                max: self.max_storage_buffers,
            });
        }
        let shared: u64 = fragment
            .slices
            .iter()
            .filter(|slice| slice.space == Space::Shared)
            .map(|slice| u64::from(slice.count) * slice.class.byte_width() as u64)
            .sum();
        if shared > u64::from(self.max_shared_bytes) {
            return Err(DeviceRefusal::SharedMemoryTooLarge {
                asked: shared,
                offered: self.max_shared_bytes,
            });
        }
        for slice in fragment.slices.iter().filter(|s| s.space == Space::Warp) {
            let subgroup_width = self.subgroup()?;
            if slice.count != subgroup_width {
                return Err(DeviceRefusal::SubgroupWidthMismatch {
                    count: slice.count,
                    subgroup_width,
                });
            }
        }
        for entry in &plan.staging {
            if entry.space != Space::Warp {
                continue;
            }
            let subgroup_width = self.subgroup()?;
            if entry.group_width != subgroup_width {
                return Err(DeviceRefusal::StagingWidthNotASubgroup {
                    name: entry.name.clone(),
                    asked: entry.group_width,
                    subgroup_width,
                });
            }
        }
        Ok(())
    }

    /// The width one subgroup runs here, or the refusal that nobody reported one.
    ///
    /// # Invariant
    /// One read, so the `Space::Warp` gates cannot disagree about whether this device
    /// reported a width at all: the two sit on either side of a plan walk, and two copies
    /// of the `None` test are two chances to name the case differently.
    fn subgroup(&self) -> Result<u32, DeviceRefusal> {
        self.subgroup_width
            .ok_or(DeviceRefusal::SubgroupWidthNotReported)
    }
}
