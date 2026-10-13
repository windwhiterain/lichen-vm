//! A reduction as a folded body, per lane or tiled.
//! See `docs/notes/shared-memory-and-barriers.md` §5 and §9 step 8.
//!
//! # Invariant
//! Both bodies apply exactly the operator the declaration named, so what licences re-association
//! is the declared associativity and not a property this module proves: splitting a fold's
//! iteration space across lanes **is** re-association, and a combine that is not associative is
//! refused by name rather than tiled.

use std::collections::HashMap;

use lichen_fusion::{BinaryOp, Expr, Reduction, Variable};
use lichen_kernel_ir::{
    Barrier, Br, KernelBin, KernelBody, KernelInstr, KernelMark, MarkField, ScalarClass,
    Terminator, ValueId,
};

use crate::operators::lowered;

/// Whether a fold can combine with `operator`.
///
/// # Invariant
/// The whole licence a re-associated fold rests on, and the one statement of what a [`Folding`]
/// can be built over: the operator must be **associative**, because a tile splits the fold's own
/// iteration space, and it must have a **lowered instruction**, because there is nothing to emit
/// without one. So this is the operator half of [`computes`]' question.
pub fn folds(operator: BinaryOp) -> bool {
    operator.associative() && lowered(operator).is_some()
}

/// Whether a kernel body can be emitted for the datum function `expression`.
///
/// # Invariant
/// [`compile`] is the whole check, so this is that function's own refusal rather than a second
/// reading of the tree: a datum that reads one slot — the datum `d` — and whose every operator
/// has an instruction is computable, and nothing else is.
pub fn computes(expression: &Expr) -> bool {
    compile(expression, ScalarClass::Float).is_some()
}

/// One step of a datum function, over the fold's single read of the datum.
///
/// # Invariant
/// Operands are **earlier** steps, so a function is a straight-line program and a body replays
/// it without re-deciding what it means. Every refusal lives in [`compile`], which runs once when
/// the [`Folding`] is built, so replaying steps cannot fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The datum itself: the buffer read at the loop's carried address.
    Datum,
    /// A literal, as the bits the fold's class carries.
    Constant(i64),
    /// A binary operator over two earlier steps.
    Binary(KernelBin, usize, usize),
}

/// The steps `expression` compiles to, or `None` when no kernel body computes it.
///
/// # Invariant
/// The one check on a datum function, and it is total: a slot other than the datum, a comparison
/// and every unary operator are all `None` rather than a guess, so a body built from the steps
/// computes the function the claim wrote and nothing else.
fn compile(expression: &Expr, class: ScalarClass) -> Option<Box<[Step]>> {
    let mut steps = Vec::new();
    step_of(expression, class, &mut steps).and_then(|()| Some(steps.into_boxed_slice()))
}

/// Whether `expression` contributes a step, and which one it is the last of.
fn step_of(expression: &Expr, class: ScalarClass, steps: &mut Vec<Step>) -> Option<()> {
    match expression {
        Expr::Variable(variable) if *variable == Variable::DATUM => {
            steps.push(Step::Datum);
            Some(())
        }
        // **A comparison narrows to a condition, and a unary operator has none at all.**
        Expr::Variable(_) | Expr::Unary(..) => None,
        Expr::Constant(value) => {
            steps.push(Step::Constant(Identity::of(*value, class).bits()));
            Some(())
        }
        Expr::Binary(operator, left, right) => {
            let operator = lowered(*operator)?;
            step_of(left, class, steps)?;
            step_of(right, class, steps)?;
            let right = steps.len() - 1;
            let left = right - 1;
            steps.push(Step::Binary(operator, left, right));
            Some(())
        }
    }
}

/// One element of a fold: how the value a lane contributes at address `x` is computed.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        /// The slice write every read of this fold observes: every read is ordered after it.
        observed: ValueId,
    },
    /// The element a **function of the datum** contributes.
    ///
    /// # Invariant
    /// The datum the body's own combine computes, carried into the fold in place of the raw
    /// buffer read. The function is read exactly as the claim wrote it and rejected by
    /// [`computes`] before a [`Folding`] holds it, so the fold computes what the body computes
    /// rather than what the combine's top-level operator alone does to the buffer.
    Function {
        /// The buffer the datum is read from, as a position into the fragment's own.
        position: usize,
        /// The function, in the symbolic language the bridge reads.
        function: Box<Expr>,
    },
}

impl Element {
    /// The class an element of this source is computed in before the fold's own class.
    pub fn class(self) -> ScalarClass {
        match self {
            Element::Buffer { .. } => ScalarClass::Int,
            Element::Slice { .. } => ScalarClass::Int,
            Element::Address => ScalarClass::Int,
            Element::Function { .. } => ScalarClass::Int,
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
    /// The element is a function of the datum no kernel body computes.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::UnsupportedCombine`] because it is a distinct question: that
    /// one is about the operator that joins two states, this one about the work done to the datum
    /// before it is joined, which is algebra a fold drops when it reads the raw buffer instead.
    UnsupportedDatum(Expr),
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
            TileRefusal::UnsupportedDatum(expression) => write!(
                formatter,
                "the datum `{expression}` is not a function a kernel body computes: it reads a \
                 value the fold has no slot for, or an operator no lowered instruction carries, so \
                 a fold that applied the raw buffer instead would answer a different sum"
            ),
        }
    }
}

/// One fold's declared combine and identity, and the elements it is a fold over.
///
/// # Invariant
/// **No count is carried here.** The bound a body folds is the launch's — an extent parameter for
/// a tiled fold and the lane's own index for a per-lane one — so a count stored beside the algebra
/// would be a number the body ignores while its caller believes it is the bound.
#[derive(Debug, Clone, PartialEq)]
pub struct Folding {
    combine: KernelBin,
    class: ScalarClass,
    identity: Identity,
    element: Element,
    /// The datum function compiled once, so a body replays it and cannot re-decide it.
    datum: Box<[Step]>,
}

impl Folding {
    /// The fold a declared reduction names over `element`.
    ///
    /// # Invariant
    /// The combine is the declaration's, converted once, and the licence is read from the
    /// declaration: a fold this returns is re-associable, and the fold it refuses has no tiled
    /// form however it is scheduled. An [`Element::Function`] datum is **compiled here and
    /// refused here**, so every body this builds applies a function a kernel can emit and a datum
    /// it cannot is never read raw.
    pub fn new(
        reduction: &Reduction,
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
        let datum = match &element {
            Element::Function { function, .. } => compile(function, class)
                .ok_or_else(|| TileRefusal::UnsupportedDatum((**function).clone()))?,
            _ => Box::default(),
        };
        Ok(Folding {
            combine,
            class,
            identity,
            element,
            datum,
        })
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
        let one = builder.constant(ScalarClass::Int, 1);
        let stop = builder.push(
            entry,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![lane, one],
        );
        let identity = builder.constant(self.class, self.identity.bits());
        let zero = builder.constant(ScalarClass::Int, 0);
        let folded = Fold {
            stop,
            start: zero,
            identity,
            stride: 1,
            modulo: None,
            element: self.element.clone(),
            datum: &self.datum,
            operator: self.combine,
        }
        .run(&mut builder);
        let position = builder.constant(ScalarClass::Int, 0);
        builder.push(
            builder.current(),
            KernelInstr::BufferWriteCall(self.class),
            vec![position, lane, folded],
        );
        let dummy = builder.constant(self.class, self.identity.bits());
        builder.finish(vec![dummy]);
        builder.body
    }

    /// The body that folds the space across the workgroup's lanes, staging into slice `slice`.
    ///
    /// # Invariant
    /// The domain is `(extent, index)` — the launch's count first, the invocation index second —
    /// which a caller's `KernelFragment::param_shape` must match or the dispatch binds one leaf's
    /// value under another. Each lane folds `[lane, lane + lane_width, …]` for `ceil(extent /
    /// lane_width)` steps behind a recorded barrier, and the fold over the slice uses the same
    /// combine.
    pub fn tiled(&self, lane_width: u32, slice: usize) -> Result<Tiled, TileRefusal> {
        if lane_width == 0 {
            return Err(TileRefusal::LaneWidthIsNotAWholeCount { lane_width });
        }
        let mut builder = Builder::new(self.class);
        let entry = builder.body.entry;
        let extent = builder.body.add_param(entry);
        let lane = builder.body.add_param(entry);
        let width = builder.constant(ScalarClass::Int, i64::from(lane_width));
        let slot = builder.push(
            entry,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
            vec![lane, width],
        );
        let identity = builder.constant(self.class, self.identity.bits());
        // **The steps a lane takes are `ceil(extent / lane_width)`**, so the bound is the extent's.
        let bias = builder.constant(ScalarClass::Int, i64::from(lane_width) - 1);
        let biased = builder.push(
            entry,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![extent, bias],
        );
        let steps = builder.push(
            entry,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Div),
            vec![biased, width],
        );
        let lane_slot = builder.constant(ScalarClass::Int, 0);
        let folded = Fold {
            stop: steps,
            start: lane,
            identity,
            stride: u64::from(lane_width),
            modulo: None,
            element: self.element.clone(),
            datum: &self.datum,
            operator: self.combine,
        }
        .run(&mut builder);
        let ordinal = builder.constant(ScalarClass::Int, slice as i64);
        let write = builder.push(
            builder.current(),
            KernelInstr::SliceWriteCall(self.class),
            vec![ordinal, slot, folded],
        );
        let identity = builder.constant(self.class, self.identity.bits());
        let reading = Fold {
            stop: width,
            start: lane_slot,
            identity,
            stride: 1,
            modulo: Some(u64::from(lane_width)),
            element: Element::Slice {
                ordinal: slice,
                observed: write,
            },
            datum: &[],
            operator: self.combine,
        }
        .run(&mut builder);
        let first = builder
            .slice_reads
            .first()
            .copied()
            .expect("a slice fold reads the slice it observed");
        builder.body.barriers.push(Barrier { write, read: first });
        let position = builder.constant(ScalarClass::Int, 0);
        builder.push(
            builder.current(),
            KernelInstr::BufferWriteCall(self.class),
            vec![position, lane, reading],
        );
        let dummy = builder.constant(self.class, self.identity.bits());
        builder.finish(vec![dummy]);
        Ok(Tiled {
            body: builder.body,
            element: self.element.clone(),
            staged: write,
            staged_count: lane_width,
        })
    }
}

/// How many elements a tiled fold's iteration space holds: a whole workgroup tile.
///
/// # Invariant
/// A whole number of tiles, so the last tile has an element for every lane: a lane the tile does
/// not reach reaches the barrier with a slot no lane wrote. The count is the caller's, because the
/// body folds the extent its own launch fixes and carries no count to read this off.
pub fn tiled_count(count: u64, lane_width: u32) -> Result<u64, TileRefusal> {
    if lane_width == 0 {
        return Err(TileRefusal::LaneWidthIsNotAWholeCount { lane_width });
    }
    Ok(count.div_ceil(u64::from(lane_width)) * u64::from(lane_width))
}

/// How many elements past `count` a caller pads a tiled fold's input with.
///
/// # Invariant
/// A caller pads the elements past `count` with the identity of the declared combine, which is
/// the one value the combine is indifferent to (`docs/notes/shared-memory-and-barriers.md` §9).
pub fn tail(count: u64, lane_width: u32) -> Result<u64, TileRefusal> {
    Ok(tiled_count(count, lane_width)? - count)
}

/// A tiled fold's body, the element source it folded, and the value it staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tiled {
    body: KernelBody,
    element: Element,
    staged: ValueId,
    staged_count: u32,
}

impl Tiled {
    /// The body, ready to be a fragment's own.
    pub fn body(&self) -> &KernelBody {
        &self.body
    }

    /// The element source the body folds.
    pub fn element(&self) -> Element {
        self.element.clone()
    }

    /// The slice write's result: the value the body staged into the slice.
    ///
    /// # Invariant
    /// The result of the body's own `SliceWriteCall`, so `KernelBody::staged_slice`
    /// answers that write's slice and a mark on this value names the staging itself
    /// (`docs/notes/graph-fusion.md` §5).
    pub fn staged(&self) -> ValueId {
        self.staged
    }

    /// The element count of the slice the body stages into.
    ///
    /// # Invariant
    /// One element per lane, so the count is the lane width the fold was tiled at. A caller
    /// declares the fragment's slice with **this** count and the plan's space: a slice's count is
    /// the body's and a plan's group width is the plan's, and reading one off the other is right
    /// only in the case where they coincide (`docs/notes/shared-memory-and-barriers.md` §5).
    pub fn staged_count(&self) -> u32 {
        self.staged_count
    }

    /// The marks publishing `name` for the staged value.
    ///
    /// # Invariant
    /// One mark in the shape the channel already uses — a single unnamed field — attached to
    /// the slice write's result, so a plan naming `name` resolves to the staging and never to
    /// a value the fold read (`docs/notes/graph-fusion.md` §5).
    pub fn marks(&self, name: &str) -> Vec<KernelMark> {
        vec![KernelMark {
            value: self.staged,
            payload: vec![MarkField {
                name: None,
                text: name.to_string(),
            }],
        }]
    }

    /// The body, taken.
    pub fn into_body(self) -> KernelBody {
        self.body
    }
}

/// One fold as a body is built: how many steps it takes, and where each step reads.
struct Fold<'a> {
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
    /// The compiled datum function, empty for every element that is not a function of one.
    datum: &'a [Step],
    operator: KernelBin,
}

impl Fold<'_> {
    /// Emit the fold, leaving the builder emitting into the block it exits to, and answer the
    /// value it accumulated.
    ///
    /// # Invariant
    /// The loop carries a **step count and a slot**: the count is what the bound compares and the
    /// slot is where the element is read. The answer is **returned, not left on the builder**,
    /// because the emitter binds a block's parameters per block, so a value read back after a
    /// later block is that block's.
    fn run(&self, builder: &mut Builder) -> ValueId {
        let from = builder.current();
        // **The blocks are allocated in the order a loop body is walked**: header, step, merge.
        let header = builder.body.add_block();
        let counter = builder.body.add_param(header);
        let value = builder.body.add_param(header);
        // **The slot is carried, never derived from the count**.
        let slot = builder.body.add_param(header);
        let step = builder.body.add_block();
        let next = builder.body.add_param(step);
        let current = builder.body.add_param(step);
        let carried_slot = builder.body.add_param(step);
        let merged = builder.body.add_block();
        let result = builder.body.add_param(merged);
        // **The count starts at zero and the slot at `start`**.
        let zero = builder.constant(ScalarClass::Int, 0);
        // **The loop's own instructions land in its header**, which `enter` below selects.
        builder.enter(header);
        // **The element is read in the header**, where a slice read's barrier is uniform.
        let address = match self.modulo {
            Some(modulo) => {
                let modulo = builder.constant(ScalarClass::Int, modulo as i64);
                builder.push(
                    header,
                    KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
                    vec![slot, modulo],
                )
            }
            None => slot,
        };
        let read = builder.element_at(&self.element, address, self.datum);
        let reached = builder.push(
            header,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Lt),
            vec![counter, self.stop],
        );
        builder.body.set_terminator(
            from,
            Terminator::Br(Br {
                target: header,
                args: vec![zero, self.identity, self.start],
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
            KernelInstr::Bin(builder.class, self.operator),
            vec![current, read],
        );
        let one = builder.constant(ScalarClass::Int, 1);
        let advanced = builder.push(
            step,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![next, one],
        );
        let stride = builder.constant(ScalarClass::Int, self.stride as i64);
        let moved = builder.push(
            step,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![carried_slot, stride],
        );
        let moved = match self.modulo {
            Some(modulo) => {
                let modulo = builder.constant(ScalarClass::Int, modulo as i64);
                builder.push(
                    step,
                    KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
                    vec![moved, modulo],
                )
            }
            None => moved,
        };
        builder.body.set_terminator(
            step,
            Terminator::Br(Br {
                target: header,
                args: vec![advanced, combined, moved],
            }),
        );
        builder.enter(merged);
        // **The loop leaves into a block of its own**, so its exit is not the next loop's header.
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
        left
    }
}
/// A body being built, with the block its instructions land in and what it has emitted.
struct Builder {
    body: KernelBody,
    /// The block instructions are emitted into.
    block: usize,
    /// The class a fold's operator is applied in.
    class: ScalarClass,
    /// Every constant this body states, by class and bits: the map is body-global, so its key states
    /// what its entries are.
    constants: HashMap<(ScalarClass, i64), ValueId>,
    /// The slice reads this body emitted, in the order it emitted them.
    slice_reads: Vec<ValueId>,
}

impl Builder {
    fn new(class: ScalarClass) -> Builder {
        let mut body = KernelBody::new();
        // **A body starts with its entry block**: a body with none has nowhere to put the domain's
        // leaves.
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
        self.body
            .set_terminator(block, Terminator::Return { values });
    }

    fn push(&mut self, block: usize, op: KernelInstr, args: Vec<ValueId>) -> ValueId {
        let classes = if op.produces() == 0 {
            Vec::new()
        } else {
            vec![op.own_class().unwrap_or(ScalarClass::Int)]
        };
        self.body.add_op(block, op, args, classes)
    }

    /// The constant `class` and `bits` name, stated once per body.
    ///
    /// # Invariant
    /// It is defined in **`body.entry`**, which dominates every reachable block, because the map
    /// below is keyed by value alone: a constant defined where it was first wanted is one a later
    /// block reads outside its scope, which `KernelBody::validate` refuses.
    fn constant(&mut self, class: ScalarClass, bits: i64) -> ValueId {
        if let Some(&value) = self.constants.get(&(class, bits)) {
            return value;
        }
        let value = self.body.add_const(self.body.entry, class, bits);
        self.constants.insert((class, bits), value);
        value
    }

    /// The value the element at `address` contributes, emitted into the current block.
    ///
    /// # Invariant
    /// The address is the **caller's**, which is the loop's own carried slot: an element that named
    /// its own address would shadow it and every step would read one slot. A datum function is
    /// replayed over the one read it binds, so the value it yields is the last step — which
    /// [`compile`] made the whole function.
    fn element_at(&mut self, element: &Element, address: ValueId, datum: &[Step]) -> ValueId {
        let block = self.current();
        match element {
            Element::Buffer { position } => {
                let position = self.constant(ScalarClass::Int, *position as i64);
                self.push(
                    block,
                    KernelInstr::BufferReadCall(self.class),
                    vec![position, address],
                )
            }
            Element::Slice { ordinal, observed } => {
                let ordinal = self.constant(ScalarClass::Int, *ordinal as i64);
                let read = self.push(
                    block,
                    KernelInstr::SliceReadCall(self.class),
                    vec![ordinal, address, *observed],
                );
                self.slice_reads.push(read);
                read
            }
            Element::Address => address,
            Element::Function { position, .. } => {
                let position = self.constant(ScalarClass::Int, *position as i64);
                let read = self.push(
                    block,
                    KernelInstr::BufferReadCall(self.class),
                    vec![position, address],
                );
                let mut values: Vec<ValueId> = Vec::with_capacity(datum.len());
                for step in datum {
                    let value = match step {
                        Step::Datum => read,
                        Step::Constant(bits) => self.constant(self.class, *bits),
                        Step::Binary(operator, left, right) => {
                            let operands = [values[*left], values[*right]];
                            self.push(
                                block,
                                KernelInstr::Bin(self.class, *operator),
                                operands.to_vec(),
                            )
                        }
                    };
                    values.push(value);
                }
                *values
                    .last()
                    .expect("a datum function compiles to at least one step")
            }
        }
    }
}

/// The fold a declared reduction names, over `element`.
pub fn fold(
    reduction: &Reduction,
    element: Element,
    class: ScalarClass,
) -> Result<Folding, TileRefusal> {
    Folding::new(reduction, element, class)
}
