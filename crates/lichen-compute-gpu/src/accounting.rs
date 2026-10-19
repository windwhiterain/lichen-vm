//! What the device is holding and why, counted without taking a lock the recorder may already hold.
//!
//! # Invariant
//! Every counter is an atomic, so reading the totals from inside `allocate` is safe: `allocate` runs
//! under the resident lock on paths that release, and a lock taken there deadlocks against itself.
//! A byte is counted once at `vkAllocateMemory` and leaves exactly once at `vkFreeMemory`.

use std::sync::atomic::{AtomicU64, Ordering};

/// What a device-local buffer is for right now, which is what a total is read against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    /// A dispatch's declared output, which is what a resident id is issued for.
    Output,
    /// A host buffer copied in for one dispatch, dead when the submission ends.
    Upload,
    /// The uploaded head of a chain, dead when the chain ends.
    Chain,
    /// The command block of a device-held count.
    Indirect,
}

impl Tag {
    /// Every tag, so a report names a row per purpose rather than the ones that happened.
    pub const ALL: [Tag; 4] = [Tag::Output, Tag::Upload, Tag::Chain, Tag::Indirect];

    /// The row's name in a report.
    pub fn name(self) -> &'static str {
        match self {
            Tag::Output => "dispatch output",
            Tag::Upload => "host upload target",
            Tag::Chain => "chain source",
            Tag::Indirect => "indirect count block",
        }
    }

    fn index(self) -> usize {
        match self {
            Tag::Output => 0,
            Tag::Upload => 1,
            Tag::Chain => 2,
            Tag::Indirect => 3,
        }
    }
}

/// One tag's byte counts, at one instant.
#[derive(Clone, Copy, Debug)]
pub struct TagBytes {
    /// The purpose the row counts.
    pub tag: Tag,
    /// Issued as a resident id and not yet released.
    pub resident: u64,
    /// Sitting in the recycling pool, waiting for a run of the same shape.
    pub pooled: u64,
    /// Every byte this tag has ever allocated, freed ones included.
    pub allocated: u64,
    /// Every byte this tag has had freed.
    pub freed: u64,
}

/// The whole device's counts, device-local bytes and staging apart.
#[derive(Clone, Debug, Default)]
pub struct DeviceReport {
    /// One row per [`Tag`].
    pub rows: Vec<TagBytes>,
    /// Device-local bytes the driver is holding, wherever they are right now.
    pub alive: u64,
    /// Host-visible staging currently mapped, which is not a device-local byte.
    pub staging: u64,
    /// The most device-local bytes alive at one instant, which is what a card has to hold.
    pub peak_alive: u64,
    /// How many pipelines this context has built and retained.
    pub pipelines: usize,
}

impl DeviceReport {
    /// Device-local bytes issued as residents and not yet released.
    pub fn resident(&self) -> u64 {
        self.rows.iter().map(|row| row.resident).sum()
    }

    /// Device-local bytes sitting in the pool.
    pub fn pooled(&self) -> u64 {
        self.rows.iter().map(|row| row.pooled).sum()
    }

    /// The report as one line per row, totals last.
    pub fn render(&self) -> String {
        let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        let mut out = String::new();
        for row in &self.rows {
            out.push_str(&format!(
                "  {:<22} resident {:>9.1} MiB  pooled {:>8.1} MiB  allocated {:>9.1} MiB  \
                 freed {:>9.1} MiB\n",
                row.tag.name(),
                mib(row.resident),
                mib(row.pooled),
                mib(row.allocated),
                mib(row.freed),
            ));
        }
        out.push_str(&format!(
            "  {:<22} {:>9.1} MiB   (peak {:.1} MiB, {:.0} pipelines retained)\n",
            "device-local total",
            mib(self.alive),
            mib(self.peak_alive),
            self.pipelines
        ));
        out.push_str(&format!(
            "  {:<22} {:>9.1} MiB   (host-visible, not device-local)\n",
            "staging", mib(self.staging)
        ));
        out
    }
}

/// Move a counter by `by`, which is negative for a decrement.
fn bump(counter: &AtomicU64, by: i64) {
    if by >= 0 {
        counter.fetch_add(by as u64, Ordering::Relaxed);
    } else {
        counter.fetch_sub((-by) as u64, Ordering::Relaxed);
    }
}

/// The counters.
///
/// # Invariant
/// `alive` is one number, not four: a recycled buffer keeps its allocation and is re-tagged for
/// what it is about to be used for, so a byte has no single tag for life and splitting the total
/// per tag would double-count it. The rows therefore count **where** those bytes are, and a byte
/// is in exactly one place — a resident, the pool, or a submission still holding it.
#[derive(Default)]
pub struct Accounting {
    rows: [Row; 4],
    alive: AtomicU64,
    staging: AtomicU64,
    peak_alive: AtomicU64,
}

#[derive(Default)]
struct Row {
    resident: AtomicU64,
    pooled: AtomicU64,
    allocated: AtomicU64,
    freed: AtomicU64,
}

impl Accounting {
    /// The counters themselves, for a context to hold.
    pub fn new() -> Self {
        Accounting::default()
    }

    fn row(&self, tag: Tag) -> &Row {
        &self.rows[tag.index()]
    }

    /// `vkAllocateMemory` handed the driver `bytes` for `tag`.
    pub fn allocated(&self, tag: Tag, bytes: u64) {
        bump(&self.row(tag).allocated, bytes as i64);
        let alive = self.alive.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.peak_alive.fetch_max(alive, Ordering::Relaxed);
    }

    /// `vkFreeMemory` took `bytes` for `tag` back from the driver.
    pub fn freed(&self, tag: Tag, bytes: u64) {
        bump(&self.row(tag).freed, bytes as i64);
        self.alive.fetch_sub(bytes, Ordering::Relaxed);
    }

    /// A pooled buffer left the pool, to serve as whatever this run needs it for.
    ///
    /// # Invariant
    /// Nothing leaves `alive`: the driver still holds these bytes and only their **purpose**
    /// changes, so the row they sat in loses them and the total does not move.
    pub fn unpooled(&self, tag: Tag, bytes: u64) {
        bump(&self.row(tag).pooled, -(bytes as i64));
    }

    /// A buffer of `tag` was handed to the pool rather than to the driver.
    pub fn pooled(&self, tag: Tag, bytes: u64) {
        bump(&self.row(tag).pooled, bytes as i64);
    }

    /// A buffer of `tag` was issued as a resident id.
    pub fn issued(&self, tag: Tag, bytes: u64) {
        bump(&self.row(tag).resident, bytes as i64);
    }

    /// A resident of `tag` was released, or its record restated at another size.
    pub fn released(&self, tag: Tag, bytes: i64) {
        bump(&self.row(tag).resident, bytes);
    }

    /// The staging mapping grew or shrank by `bytes`.
    pub fn staging(&self, bytes: i64) {
        bump(&self.staging, bytes);
    }

    /// Every counter at one instant, which reads no lock and so may be called from anywhere.
    pub fn report(&self) -> DeviceReport {
        let rows = Tag::ALL
            .iter()
            .map(|tag| {
                let row = self.row(*tag);
                TagBytes {
                    tag: *tag,
                    resident: row.resident.load(Ordering::Relaxed),
                    pooled: row.pooled.load(Ordering::Relaxed),
                    allocated: row.allocated.load(Ordering::Relaxed),
                    freed: row.freed.load(Ordering::Relaxed),
                }
            })
            .collect();
        DeviceReport {
            rows,
            alive: self.alive.load(Ordering::Relaxed),
            staging: self.staging.load(Ordering::Relaxed),
            peak_alive: self.peak_alive.load(Ordering::Relaxed),
            pipelines: 0,
        }
    }
}
