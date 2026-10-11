//! A declared reduction as a folded body: per lane, or tiled across the workgroup.
//! See `docs/notes/shared-memory-and-barriers.md` §5 and §9 step 8.
//!
//! # Invariant
//! The fold is the declaration's and nothing else is: [`Folding`] carries the `combine` and the
//! `identity` the reduction declared, and both bodies below apply exactly the operator the
//! declaration named. What licences re-association is therefore the declared associativity and
//! not a property this module proves, and a combine that is not associative is refused by name
//! rather than tiled — splitting a fold's iteration space across lanes **is** re-association.

use std::collections::HashMap;

use lichen_fusion::{BinaryOp, Reduction};
use lichen_kernel_ir::{
    Barrier, Br, KernelBin, KernelBody, KernelInstr, ScalarClass, Terminator, ValueId,
};

use crate::operators::lowered;

/// One element of a fold: how the value a lane contributes at address `x` is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    /// The element one buffer holds at `x`, read at the fold's class.
    Buffer {
        /// The buffer the fold reads, as a position into the fragment's own.
        position: usize,
    },
    /// The address itself, converted to the fold's class.
    Address,
    /// The element one slice holds at `x`: the second half of a tiled fold.
    Slice {
        /// The slice, as an ordinal into the fragment's slice table.
        ordinal: usize,
        /// The slice write every read of this fold observes, so every read is ordered after it
        /// without a chain of versions no one reads.
        observed: ValueId,
    },
}

impl Element {
    /// The class an element of this source is computed in before the fold's own class.
    pub const fn class(self) -> ScalarClass {
        match self {
            Element::Buffer { .. } => ScalarClass::Int,
            Element::Slice { .. } => ScalarClass::Int,
            Element::Address => ScalarClass::Int,
        }
    }
}

/// The value a fold starts from, as the declaration states it.
///
/// # Invariant
/// An identity is a number of one class, so it is carried as bits and read at that class — the
/// encoding [`KernelInstr::Const`] already uses, because an `f64` identity lowered to `Float`
/// is an `f32` and a second conversion site is how the two spellings drift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Identity {
    /// An integer identity — `0` for a sum, `1` for a product.
    Int(i64),
    /// A float identity, as the `f64` the declaration holds.
    Float(f64),
}

impl Identity {
    /// The class the identity is stated in.
    pub const fn class(self) -> ScalarClass {
        match self {
            Identity::Int(_) => ScalarClass::Int,
            Identity::Float(_) => ScalarClass::Float,
        }
    }

    /// The bits [`KernelInstr::Const`] carries for this identity, at its own class.
    pub fn bits(self) -> i64 {
        match self {
            Identity::Int(bits) => bits,
            Identity::Float(value) => (value as f32).to_bits() as i64,
        }
    }

    /// The identity a declared value is, at the class the fold computes in.
    pub fn of(value: f64, class: ScalarClass) -> Identity {
        match class {
            ScalarClass::Int => Identity::Int(value as i64),
            ScalarClass::Float => Identity::Float(value),
        }
    }
}

/// Why a declared reduction was not folded.
#[derive(Debug, Clone, PartialEq)]
pub enum TileRefusal {
    /// The declaration's combine has no lowered instruction.
    UnsupportedCombine(BinaryOp),
    /// The declaration's combine is not associative, so a tiled fold is a different answer.
    CombineIsNotAssociative(BinaryOp),
    /// The declaration's `combine` and `identity` are stated in two classes.
    ClassesDisagree {
        /// The class the combine is applied in.
        combine: ScalarClass,
        /// The class the identity is stated in.
        identity: ScalarClass,
    },
    /// A workgroup declares no lane, so its tile holds no element.
    LaneWidthIsNotAWholeCount {
        /// The width the caller declared.
        lane_width: u32,
    },
}

impl std::fmt::Display for TileRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TileRefusal::UnsupportedCombine(operator) => write!(
                formatter,
                "the reduction's combine `{}` is not a lowered instruction, so there is no \
                 operator to fold with",
                operator.name()
            ),
            TileRefusal::CombineIsNotAssociative(operator) => write!(
                formatter,
                "the reduction's combine `{}` is not associative: tiling a fold splits its \
                 iteration space across lanes and combines the partials, which is exactly \
                 re-association, so the declared algebra does not licence it \
                 (`docs/notes/shared-memory-and-barriers.md` §5)",
                operator.name()
            ),
            TileRefusal::ClassesDisagree { combine, identity } => write!(
                formatter,
                "the reduction's combine is in {combine:?} and its identity in {identity:?}, and a \
                 fold's identity is the value its combine is applied to"
            ),
            TileRefusal::LaneWidthIsNotAWholeCount { lane_width } => write!(
                formatter,
                "a workgroup runs {lane_width} lane(s), and a tiled fold needs one lane to fold a \
                 tile"
            ),
        }
    }
}

/// One fold's declared combine and identity, and the elements it is a fold over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Folding {
    combine: KernelBin,
    class: ScalarClass,
    identity: Identity,
    count: u64,
    element: Element,
}

impl Folding {
    /// The fold a declared reduction names over `count` elements of `element`.
    ///
    /// # Invariant
    /// The combine is the declaration's, converted once, and the licence is read from the
    /// declaration: a fold this returns is re-associable, and the fold it refuses has no tiled
    /// form however it is scheduled.
    pub fn new(
        reduction: &Reduction,
        count: u64,
        element: Element,
        class: ScalarClass,
    ) -> Result<Folding, TileRefusal> {
        if !reduction.combine.associative() {
            return Err(TileRefusal::CombineIsNotAssociative(reduction.combine));
        }
        let Some(combine) = lowered(reduction.combine) else {
            return Err(TileRefusal::UnsupportedCombine(reduction.combine));
        };
        let identity = Identity::of(reduction.identity, class);
        if identity.class() != class {
            return Err(TileRefusal::ClassesDisagree {
                combine: class,
                identity: identity.class(),
            });
        }
        Ok(Folding {
            combine,
            class,
            identity,
            count,
            element,
        })
    }

    /// How many elements the fold is over.
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// How many elements a tiled fold's iteration space holds: a whole workgroup tile.
    ///
    /// # Invariant
    /// A whole number of tiles, so the last tile has an element for every lane: a lane the tile
    /// does not reach reaches the barrier with a slot no lane wrote.
    pub fn tiled_count(&self, lane_width: u32) -> Result<u64, TileRefusal> {
        if lane_width == 0 {
            return Err(TileRefusal::LaneWidthIsNotAWholeCount { lane_width });
        }
        Ok(self.count.div_ceil(u64::from(lane_width)) * u64::from(lane_width))
    }

    /// How many elements past [`Self::count`] a caller pads a tiled fold's input with.
    ///
    /// # Invariant
    /// The tail is the caller's, because the fold's own iteration space is a whole workgroup
    /// tile — `lane_width × ceil(count / lane_width)` — and a lane that folds no element of its
    /// own reaches the barrier with a slot no lane wrote. A caller pads the elements past
    /// [`Self::count`] with the identity of the declared combine, which is the one value the
    /// combine is indifferent to (`docs/notes/shared-memory-and-barriers.md` §9).
    pub fn tail(&self, lane_width: u32) -> Result<u64, TileRefusal> {
        Ok(self.tiled_count(lane_width)? - self.count)
    }

    /// The body that folds the whole space once, in one lane.
    ///
    /// # Invariant
    /// The domain is `[extent, lane]` and the codomain one output buffer: lane `i` writes the
    /// fold over `[0, i + 1]` at `out[i]`, so `out[count - 1]` is the fold over the declared
    /// count — the per-lane implementation a tiled body is differenced against.
    pub fn per_lane(&self) -> KernelBody {
        let mut builder = Builder::new(self.class);
        let entry = builder.body.entry;
        builder.body.add_param(entry);
        let lane = builder.body.add_param(entry);
        let one = builder.constant(entry, ScalarClass::Int, 1);
        let stop = builder.push(
            entry,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![lane, one],
        );
        eprintln!("DEBUG per_lane: lane {lane:?} stop {stop:?}");
        let identity = builder.constant(entry, self.class, self.identity.bits());
        let zero = builder.constant(entry, ScalarClass::Int, 0);
        let folded = Loop {
            builder,
            stop,
            start: zero,
            identity,
            stride: 1,
            modulo: None,
            element: self.element,
            operator: self.combine,
        }
        .run();
        let mut builder = folded.builder;
        let position = builder.constant(builder.body.entry, ScalarClass::Int, 0);
        builder.push(
            builder.current(),
            KernelInstr::BufferWriteCall(self.class),
            vec![position, lane, folded.result],
        );
        let dummy = builder.constant(builder.body.entry, self.class, self.identity.bits());
        builder.finish(vec![dummy]);
        builder.body
    }

    /// The body that folds the space across the workgroup's lanes.
    ///
    /// # Invariant
    /// Every lane folds the strided subset `[lane, lane + lane_width, …]` into the slot its own
    /// index names, a recorded barrier separates those writes from the reads, and the fold then
    /// continues over the slice's elements with the same declared combine. `slice` is the ordinal
    /// of a slice the fragment declares with `lane_width` elements.
    pub fn tiled(&self, lane_width: u32, slice: usize) -> Result<Tiled, TileRefusal> {
        if lane_width == 0 {
            return Err(TileRefusal::LaneWidthIsNotAWholeCount { lane_width });
        }
        let count = self.tiled_count(lane_width)?;
        let mut builder = Builder::new(self.class);
        let entry = builder.body.entry;
        builder.body.add_param(entry);
        let lane = builder.body.add_param(entry);
        let width = builder.constant(entry, ScalarClass::Int, i64::from(lane_width));
        let slot = builder.push(
            entry,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
            vec![lane, width],
        );
        let identity = builder.constant(entry, self.class, self.identity.bits());
        // **The steps a lane takes are per lane**, not the elements the whole tile holds: the
        // stride hands each lane the elements it owns, so the bound is the tile divided by it.
        let steps = count / u64::from(lane_width);
        let stop = builder.constant(entry, ScalarClass::Int, steps as i64);
        let lane_slot = builder.constant(entry, ScalarClass::Int, 0);
        let folded = Loop {
            builder,
            stop,
            start: lane,
            identity,
            stride: u64::from(lane_width),
            modulo: None,
            element: self.element,
            operator: self.combine,
        }
        .run();
        let mut builder = folded.builder;
        let ordinal = builder.constant(builder.body.entry, ScalarClass::Int, slice as i64);
        let write = builder.push(
            builder.current(),
            KernelInstr::SliceWriteCall(self.class),
            vec![ordinal, slot, folded.result],
        );
        let identity = builder.constant(builder.body.entry, self.class, self.identity.bits());
        let reading = Loop {
            builder,
            stop: width,
            start: lane_slot,
            identity,
            stride: 1,
            modulo: Some(u64::from(lane_width)),
            element: Element::Slice {
                ordinal: slice,
                observed: write,
            },            operator: self.combine,
        }
        .run();
        let mut builder = reading.builder;
        let first = builder
            .slice_reads
            .first()
            .copied()
            .expect("a slice fold reads the slice it observed");
        builder.body.barriers.push(Barrier { write, read: first });
        let position = builder.constant(builder.body.entry, ScalarClass::Int, 0);
        builder.push(
            builder.current(),
            KernelInstr::BufferWriteCall(self.class),
            vec![position, lane, folded.result],
        );
        let dummy = builder.constant(builder.body.entry, self.class, self.identity.bits());
        builder.finish(vec![dummy]);
        return Ok(Tiled {
            body: builder.body,
            element: self.element,
        });
    }
}

/// A tiled fold's body, with the element source it folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tiled {
    body: KernelBody,
    element: Element,
}

impl Tiled {
    /// The body, ready to be a fragment's own.
    pub fn body(&self) -> &KernelBody {
        &self.body
    }

    /// The element source the body folds.
    pub fn element(&self) -> Element {
        self.element
    }

    /// The body, taken.
    pub fn into_body(self) -> KernelBody {
        self.body
    }
}

/// One fold as a body is built: how many steps it takes, and where each step reads.
struct Loop {
    builder: Builder,
    /// The value the loop leaves at: the number of steps one lane takes.
    stop: ValueId,
    /// The address the first step reads.
    start: ValueId,
    /// The value the fold starts from, which is what an empty space folds to.
    identity: ValueId,
    /// How far each step advances the address, so one lane folds a strided subset.
    stride: u64,
    /// The width the address folds back into before it is read, when the element is a slice's.
    ///
    /// # Invariant
    /// A slice holds one element per lane, so a fold that reads it addresses `[0, lane_width)` —
    /// the bound is the slice's own count. Without it the address would leave the slice, which
    /// `KernelFragment::barrier_scan` refuses against the length the emitter declares.
    modulo: Option<u64>,
    element: Element,
    operator: KernelBin,
}

/// A loop's own carried values and the block it emitted into.
struct Folded {
    builder: Builder,
    /// The value the loop merged its accumulator as.
    result: ValueId,
}

impl Loop {
    /// Emit the loop, leaving the builder emitting into the block it exits to.
    ///
    /// # Invariant
    /// The loop is the body's structured control flow: the header is the test, the step is the
    /// fold, and both hand the whole carried state over — so a value the fold accumulates is a
    /// block parameter and survives the backedge. It carries a **step count and a slot**: the count
    /// is what the bound compares, and the slot is where the element is read, so a fold over a
    /// strided buffer and a fold over a slice's slots are the same loop and a lane's slot advances
    /// by its stride while the count advances by one.
    fn run(self) -> Folded {
        let Loop {
            mut builder,
            stop,
            start,
            identity,
            stride,
            modulo,
            element,
            operator,
        } = self;
        let from = builder.current();
        // **The blocks are allocated in the order a loop body is walked**: the header, its step,
        // and the merge it exits to last, which is the order `Lower`'s own nests give the emitter.
        let header = builder.body.add_block();
        let counter = builder.body.add_param(header);
        let value = builder.body.add_param(header);
        // **The slot is carried, never derived from the count**: a slot the count derived would have
        // to be re-derived from the same count the step advances, and the backedge would then carry
        // the read's *index* for the next iteration's *value*.
        let slot = builder.body.add_param(header);
        let step = builder.body.add_block();
        let next = builder.body.add_param(step);
        let current = builder.body.add_param(step);
        let carried_slot = builder.body.add_param(step);
        let merged = builder.body.add_block();
        let result = builder.body.add_param(merged);
        // **The count starts at zero and the slot at `start`**: the steps a lane takes are `stop`
        // and the slot it reads is its own, so a count that began at `start` would take
        // `stop - start` steps and a strided fold would read past its own space.
        let zero = builder.constant(builder.body.entry, ScalarClass::Int, 0);
        // **The loop's own instructions land in its header**: the slot, the read and the test all
        // read the header's parameters, so the builder has to be emitting into it and not into the
        // block the loop was entered from.
        builder.enter(header);
        // **The element is read in the header**: a slice read carries the barrier that separates
        // it from the write, and a read in the loop's **step** block is one no barrier can serve —
        // the step is a block a lane skips when its own bound is reached, so a barrier under it is
        // in divergent control flow, which is invalid rather than imprecise. The step folds this
        // value directly: the header dominates it, and a value carried only to be folded again is
        // a second name for the same read. A slice is addressed **through its own count**, because
        // every lane must read every slot and `KernelFragment::barrier_scan` bounds an access by
        // the length the emitter declares.
        let address = match modulo {
            Some(modulo) => {
                let modulo = builder.constant(builder.body.entry, ScalarClass::Int, modulo as i64);
                builder.push(
                    header,
                    KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
                    vec![slot, modulo],
                )
            }
            None => slot,
        };
        let read = builder.element_at(element, address);
        let reached = builder.push(
            header,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Lt),
            vec![counter, stop],
        );
        builder.body.set_terminator(
            from,
            Terminator::Br(Br {
                target: header,
                args: vec![zero, identity, start],
            }),
        );
        builder.body.set_terminator(
            header,
            Terminator::CondBr {
                cond: reached,
                if_true: Br {
                    target: step,
                    args: vec![counter, value, slot],
                },
                if_false: Br {
                    target: merged,
                    args: vec![value],
                },
            },
        );
        builder.enter(step);
        let combined = builder.push(
            step,
            KernelInstr::Bin(builder.class, operator),
            vec![current, read],
        );
        let one = builder.constant(step, ScalarClass::Int, 1);
        let advanced = builder.push(
            step,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![next, one],
        );
        let stride = builder.constant(step, ScalarClass::Int, stride as i64);
        let moved = builder.push(
            step,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![carried_slot, stride],
        );
        let moved = match modulo {
            Some(modulo) => {
                let modulo = builder.constant(step, ScalarClass::Int, modulo as i64);
                builder.push(
                    step,
                    KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
                    vec![moved, modulo],
                )
            }
            None => moved,
        };        builder.body.set_terminator(
            step,
            Terminator::Br(Br {
                target: header,
                args: vec![advanced, combined, moved],
            }),
        );
        builder.enter(merged);
        // **The loop leaves into a block of its own**: the emitter plans a merge node whose exit is
        // the block the header names, and an exit that is also the next loop's header is two
        // structures meeting at one block.
        let after = builder.body.add_block();
        let left = builder.body.add_param(after);
        builder.body.set_terminator(
            merged,
            Terminator::Br(Br {
                target: after,
                args: vec![result],
            }),
        );
        builder.enter(after);
        Folded {
            builder,
            result: left,
        }
    }
}

/// A body being built, with the block its instructions land in and what it has emitted.
struct Builder {
    body: KernelBody,
    /// The block instructions are emitted into.
    block: usize,
    /// The class a fold's operator is applied in.
    class: ScalarClass,
    /// Every constant emitted so far, by class and bits, so a body states one per value.
    constants: HashMap<(ScalarClass, i64), ValueId>,
    /// The slice reads this body emitted, in the order it emitted them.
    slice_reads: Vec<ValueId>,
}

impl Builder {
    fn new(class: ScalarClass) -> Builder {
        let mut body = KernelBody::new();
        // **A body starts with its entry block**: every parameter and constant below is added
        // to the block `KernelBody::entry` names, and a body that has none has nowhere to put
        // the domain's leaves.
        let entry = body.add_block();
        debug_assert_eq!(entry, body.entry);
        Builder {
            body,
            block: entry,
            class,
            constants: HashMap::new(),
            slice_reads: Vec::new(),
        }
    }

    fn current(&self) -> usize {
        self.block
    }

    fn enter(&mut self, block: usize) {
        self.block = block;
    }

    /// The terminator of a body whose walk has ended.
    fn finish(&mut self, values: Vec<ValueId>) {
        let block = self.block;
        self.body.set_terminator(block, Terminator::Return { values });
    }

    fn push(&mut self, block: usize, op: KernelInstr, args: Vec<ValueId>) -> ValueId {
        let classes = if op.produces() == 0 {
            Vec::new()
        } else {
            vec![op.own_class().unwrap_or(ScalarClass::Int)]
        };
        self.body.add_op(block, op, args, classes)
    }

    fn constant(&mut self, block: usize, class: ScalarClass, bits: i64) -> ValueId {
        if let Some(&value) = self.constants.get(&(class, bits)) {
            return value;
        }
        let value = self.body.add_const(block, class, bits);
        self.constants.insert((class, bits), value);
        value
    }

    /// The value the element at `address` contributes, emitted into the current block.
    ///
    /// # Invariant
    /// The address is the **caller's**, which is the loop's own carried slot: an element that named
    /// its own address would shadow it, and the read would then observe one slot for every step.
    fn element_at(&mut self, element: Element, address: ValueId) -> ValueId {
        let block = self.current();
        match element {
            Element::Buffer { position } => {
                let position = self.constant(block, ScalarClass::Int, position as i64);
                self.push(
                    block,
                    KernelInstr::BufferReadCall(self.class),
                    vec![position, address],
                )
            }
            Element::Slice { ordinal, observed } => {
                let ordinal = self.constant(block, ScalarClass::Int, ordinal as i64);
                let read = self.push(
                    block,
                    KernelInstr::SliceReadCall(self.class),
                    vec![ordinal, address, observed],
                );
                self.slice_reads.push(read);
                read
            }
            Element::Address => address,
        }
    }
}

/// The fold a declared reduction names, over `count` elements of `element`.
pub fn fold(
    reduction: &Reduction,
    count: u64,
    element: Element,
    class: ScalarClass,
) -> Result<Folding, TileRefusal> {
    Folding::new(reduction, count, element, class)
}
