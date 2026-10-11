//! The barrier scan: the pairs a body's slice accesses separate.
//! See `docs/notes/shared-memory-and-barriers.md` §4.

use std::collections::HashMap;

use crate::{
    Barrier, KernelBin, KernelBody, KernelFragment, KernelInstr, ScalarClass, Slice, ValueDef,
    ValueId,
};

/// A conservative bound on the slot index an expression takes over the lanes.
///
/// # Invariant
/// The index every lane computes lies inside `[low, high]`, so two intervals that do not intersect
/// are two access sets that cannot meet, and a high that reaches a slot's declared count is an
/// access that may leave it. Both readings need the bound to be a superset: a bound that excludes
/// a reachable slot is how a barrier goes missing.
#[derive(Clone, Copy)]
struct Interval {
    low: u64,
    high: u64,
}

impl Interval {
    /// The whole index space: what an expression with no derivable range answers.
    const UNBOUNDED: Interval = Interval {
        low: 0,
        high: u64::MAX,
    };

    fn intersects(self, other: Interval) -> bool {
        self.low <= other.high && other.low <= self.high
    }
}

impl KernelFragment {
    /// The fragment-scope facts a body's own gate cannot see, refused by name.
    ///
    /// # Invariant
    /// A slice declares at least one element: a zero-length `OpTypeArray` has no type to declare and
    /// no index it could bound, so nothing can stage through it
    /// (`docs/notes/shared-memory-and-barriers.md` §9).
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ordinal) = self.slices.iter().position(|slice| slice.count == 0) {
            return Err(format!(
                "the slice {ordinal} declares no element(s): a slice's count is the length of \
                 the array the emitter declares and the bound every access is refused \
                 against, so a zero-length array has no type to declare and no index it could \
                 bound (`docs/notes/shared-memory-and-barriers.md` §9)"
            ));
        }
        Ok(())
    }

    /// The pairs this fragment's slice accesses need a barrier between.
    ///
    /// # Invariant
    /// The extents the body's accesses are bounded by are this fragment's own
    /// [`Self::slices`], so an index that may leave its slice is refused against the length
    /// the emitter declares rather than one a caller supplied.
    pub fn barrier_scan(&self, lane: ValueId) -> Result<Vec<Barrier>, String> {
        self.validate()?;
        self.body.barrier_scan(lane, &self.slices)
    }
}

impl KernelBody {
    /// The pairs this body's slice accesses need a barrier between.
    ///
    /// # Invariant
    /// `lane` is the parameter the invocation index arrives as and `slices` the extents the
    /// fragment declares; a body states neither, so the caller does. A pair is returned when the
    /// read observes the write across lanes, at addresses that may coincide, with no recorded
    /// barrier between them; a contended write, and an access whose index may leave its slice,
    /// are refused.
    pub fn barrier_scan(&self, lane: ValueId, slices: &[Slice]) -> Result<Vec<Barrier>, String> {
        let order = self.walk_order();
        let position: HashMap<ValueId, usize> = order
            .iter()
            .enumerate()
            .map(|(at, &value)| (value, at))
            .collect();
        let incoming = self.incoming_arguments();
        let mut memo = HashMap::new();
        self.refuse_bad_slice_accesses(&order, lane, &incoming, slices, &mut memo)?;
        // **A recorded barrier is already between them**: the scan is a function of the body, not
        // of how often it has run.
        let mut separated: HashMap<u64, usize> = HashMap::new();
        for pair in &self.barriers {
            if let (Some(slice), Some(&at)) =
                (self.slice_ordinal(pair.read), position.get(&pair.read))
            {
                separated
                    .entry(slice)
                    .and_modify(|held| *held = (*held).max(at))
                    .or_insert(at);
            }
        }
        let mut pairs = Vec::new();
        for &read in &order {
            let Some(ValueDef::Instr {
                op: KernelInstr::SliceReadCall(_),
                args,
                ..
            }) = self.values.get(read.0 as usize)
            else {
                continue;
            };
            let (Some(&write), Some(&read_index)) = (args.get(2), args.get(1)) else {
                continue;
            };
            let Some(ValueDef::Instr {
                args: write_args, ..
            }) = self.values.get(write.0 as usize)
            else {
                continue;
            };
            let Some(&write_index) = write_args.get(1) else {
                continue;
            };
            // **The reader is the writing lane**: it observes its own earlier write in program
            // order, whatever the address is.
            if write_index == read_index {
                continue;
            }
            let write_slice = self.slice_ordinal(write);
            let read_slice = self.slice_ordinal(read);
            if matches!(
                (write_slice, read_slice),
                (Some(written), Some(addressed)) if written != addressed
            ) {
                continue;
            }
            let read_at = position.get(&read).copied();
            let settled = match (position.get(&write).copied(), read_at, read_slice) {
                (Some(write_at), Some(this_read_at), Some(slice)) => separated
                    .get(&slice)
                    .is_some_and(|&barrier| write_at < barrier && barrier <= this_read_at),
                _ => false,
            };
            if settled {
                continue;
            }
            if !self
                .interval(write_index, &mut memo)
                .intersects(self.interval(read_index, &mut memo))
            {
                continue;
            }
            pairs.push(Barrier { write, read });
            if let (Some(slice), Some(read_at)) = (read_slice, read_at) {
                separated.insert(slice, read_at);
            }
        }
        Ok(pairs)
    }

    /// The slice accesses a barrier decision cannot be made from, refused by name.
    ///
    /// # Invariant
    /// Run before any pair is recorded, so a contended write is named rather than reported as
    /// an owed barrier and every read this scan pairs observes a real write — the obligation
    /// [`Self::validate`] could not discharge, because it only sees recorded pairs.
    fn refuse_bad_slice_accesses(
        &self,
        order: &[ValueId],
        lane: ValueId,
        incoming: &[Vec<Vec<ValueId>>],
        slices: &[Slice],
        memo: &mut HashMap<ValueId, Interval>,
    ) -> Result<(), String> {
        for &value in order {
            let Some(ValueDef::Instr { op, args, .. }) = self.values.get(value.0 as usize) else {
                continue;
            };
            match op {
                KernelInstr::SliceWriteCall(_) => {
                    self.refuse_escaping_slice(value, "write", args, slices, memo)?;
                    let Some(&index) = args.get(1) else {
                        continue;
                    };
                    if !self.depends_on(index, lane, incoming) {
                        return Err(format!(
                            "the slice write {value:?} is contended: its index {index:?} does \
                             not depend on the lane parameter, so every lane writes the one slice \
                             and no barrier can order them \
                             (`docs/notes/shared-memory-and-barriers.md` §4)"
                        ));
                    }
                }
                KernelInstr::SliceReadCall(_) => {
                    self.refuse_escaping_slice(value, "read", args, slices, memo)?;
                    let observed = args.get(2).copied();
                    let names_a_write = observed.is_some_and(|observed| {
                        matches!(
                            self.values.get(observed.0 as usize),
                            Some(ValueDef::Instr {
                                op: KernelInstr::SliceWriteCall(_),
                                ..
                            })
                        )
                    });
                    if !names_a_write {
                        return Err(format!(
                            "the slice read {value:?} observes no slice write: its version \
                             operand is {observed:?}, and the scan owes the pairing \
                             [`Self::validate`] cannot require \
                             (`docs/notes/shared-memory-and-barriers.md` §9)"
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A slice access whose index interval may leave its slice, refused by name.
    ///
    /// # Invariant
    /// The bound is the count the fragment declares for the slice, and `count` is at least one, so
    /// a reach of `count` or more addresses past the array — a fault no barrier orders
    /// and `spirv-val` cannot see. An access to a slice the table does not reach is not bounded
    /// here; a constant slice ordinal past the set is the emitter's refusal.
    fn refuse_escaping_slice(
        &self,
        value: ValueId,
        what: &str,
        args: &[ValueId],
        slices: &[Slice],
        memo: &mut HashMap<ValueId, Interval>,
    ) -> Result<(), String> {
        let Some(&index) = args.get(1) else {
            return Ok(());
        };
        let Some(count) = self
            .slice_ordinal(value)
            .and_then(|ordinal| slices.get(ordinal as usize))
            .map(|slice| slice.count)
        else {
            return Ok(());
        };
        let reach = self.interval(index, memo).high;
        if reach < u64::from(count) {
            return Ok(());
        }
        Err(format!(
            "the slice {what} {value:?} may address past its slice: the index {index:?} reaches \
             {reach} and the slice holds {count} element(s), so the access can address past the \
             array — a fault no barrier orders and `spirv-val` cannot see \
             (`docs/notes/shared-memory-and-barriers.md` §9)"
        ))
    }

    /// Every instruction this body lists, in the walk's program order.
    ///
    /// # Invariant
    /// Depth first from `entry`, each block's branches taken in the order it names them, so a
    /// dominator is visited before a block it dominates and the order is deterministic.
    fn walk_order(&self) -> Vec<ValueId> {
        let mut order = Vec::new();
        let mut seen = vec![false; self.blocks.len()];
        let mut stack = vec![self.entry];
        while let Some(block) = stack.pop() {
            if block >= self.blocks.len() || std::mem::replace(&mut seen[block], true) {
                continue;
            }
            order.extend(self.blocks[block].instrs.iter().copied());
            for branch in self.blocks[block].branches().into_iter().rev() {
                stack.push(branch.target);
            }
        }
        order
    }

    /// A conservative bound on the index `value` takes, over every lane.
    ///
    /// # Invariant
    /// The value every lane computes lies inside the answer, which is therefore a superset of
    /// the addresses at stake: disjoint answers are disjoint addresses.
    fn interval(&self, value: ValueId, memo: &mut HashMap<ValueId, Interval>) -> Interval {
        if let Some(&known) = memo.get(&value) {
            return known;
        }
        // **A value is bounded before its own cone is walked**: a cone reaching back to it answers
        // the whole space.
        memo.insert(value, Interval::UNBOUNDED);
        let answer = match self.values.get(value.0 as usize) {
            Some(ValueDef::Instr {
                op: KernelInstr::Const(ScalarClass::Int, bits),
                ..
            }) => {
                let bound = *bits as u64;
                Interval {
                    low: bound,
                    high: bound,
                }
            }
            Some(ValueDef::Instr { op, args, .. }) => self.interval_of(op, args, memo),
            _ => Interval::UNBOUNDED,
        };
        memo.insert(value, answer);
        answer
    }

    /// [`Self::interval`]'s arithmetic, over one instruction's operands.
    ///
    /// # Invariant
    /// An operator the IR does not bound — a conversion, a buffer read, a wrapping subtraction
    /// — answers [`Interval::UNBOUNDED`] rather than a guess, because a bound that excludes a
    /// reachable slot is how a barrier goes missing.
    fn interval_of(
        &self,
        op: &KernelInstr,
        args: &[ValueId],
        memo: &mut HashMap<ValueId, Interval>,
    ) -> Interval {
        let operand = |position: usize, memo: &mut HashMap<ValueId, Interval>| {
            args.get(position)
                .map(|&value| self.interval(value, memo))
                .unwrap_or(Interval::UNBOUNDED)
        };
        match *op {
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add) => {
                let (left, right) = (operand(0, memo), operand(1, memo));
                match (
                    left.low.checked_add(right.low),
                    left.high.checked_add(right.high),
                ) {
                    (Some(low), Some(high)) => Interval { low, high },
                    _ => Interval::UNBOUNDED,
                }
            }
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Mul) => {
                let (left, right) = (operand(0, memo), operand(1, memo));
                match (
                    left.low.checked_mul(right.low),
                    left.high.checked_mul(right.high),
                ) {
                    (Some(low), Some(high)) => Interval { low, high },
                    _ => Interval::UNBOUNDED,
                }
            }
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem) => {
                let left = operand(0, memo);
                match self.constant(args.get(1).copied()) {
                    Some(modulus) if modulus > 0 => Interval {
                        low: 0,
                        high: left.high.min(modulus - 1),
                    },
                    _ => Interval {
                        low: 0,
                        high: left.high,
                    },
                }
            }
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Div) => Interval {
                low: 0,
                high: operand(0, memo).high,
            },
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Max) => {
                let (left, right) = (operand(0, memo), operand(1, memo));
                Interval {
                    low: left.low.max(right.low),
                    high: left.high.max(right.high),
                }
            }
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Min) => {
                let (left, right) = (operand(0, memo), operand(1, memo));
                Interval {
                    low: left.low.min(right.low),
                    high: left.high.min(right.high),
                }
            }
            KernelInstr::Bin(ScalarClass::Int, operator) if operator.is_comparison() => {
                Interval { low: 0, high: 1 }
            }
            KernelInstr::I32WrapI64 => Interval { low: 0, high: 1 },
            KernelInstr::Select => {
                let (left, right) = (operand(1, memo), operand(2, memo));
                Interval {
                    low: left.low.min(right.low),
                    high: left.high.max(right.high),
                }
            }
            _ => Interval::UNBOUNDED,
        }
    }
}
