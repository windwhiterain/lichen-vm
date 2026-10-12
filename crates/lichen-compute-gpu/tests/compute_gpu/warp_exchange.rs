//! A cross-lane value confined to one subgroup: the shuffle exchange.
//!
//! # Invariant
//! The risk in a placement this narrow is a **silent** one: a read naming a lane the reader cannot
//! reach answers whatever the device holds. So each case states the answer it would give if the
//! gate were wrong — the differential pins one number through both placements, and the negative
//! case names what a cross-warp read would have cost
//! (`docs/notes/shared-memory-and-barriers.md` §5).

use super::common;

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_compute_gpu::{LOCAL_SIZE_X, SUBGROUP_WIDTH};
use lichen_kernel_ir::{
    Barrier, BufferSlot, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaneWidth, LaunchSet, ScalarClass, Slice, Space, TargetFacts, Terminator,
    ValueDef, ValueId,
};

/// `OpCapability` (17), `OpExecutionMode` (16) and `OpControlBarrier` (224), the specification's
/// own numbers.
const VALIDATOR: &str = "spirv-val";
const CAPABILITY: u16 = 17;
const EXECUTION_MODE: u16 = 16;
const CONTROL_BARRIER: u16 = 224;
/// `OpGroupNonUniformShuffle` (345).
const GROUP_NON_UNIFORM_SHUFFLE: u16 = 345;
/// `SpvCapabilityGroupNonUniformShuffle` (65) and `SpvExecutionModeLocalSize` (17).
const GROUP_NON_UNIFORM_SHUFFLE_CAPABILITY: u32 = 65;
const EXECUTION_MODE_LOCAL_SIZE: u32 = 17;
/// `SpvScopeWorkgroup` and `SpvScopeSubgroup`.
const SCOPE_WORKGROUP: u32 = 2;
const SCOPE_SUBGROUP: u32 = 3;

/// `OpConstant` (43), the specification's own number.
const CONSTANT: u16 = 43;

/// Every instruction the module carries, as its opcode and its operands.
///
/// # Invariant
/// `spirv-val` reads the module's shape and cannot see a **missing** barrier or a **wrong** scope,
/// so what the device is handed has to be read back off the words: a constant's own value is in
/// the operands the instruction carries, so this is the module the device runs rather than a
/// description of it.
fn instructions(words: &[u32]) -> Vec<(u16, Vec<u32>)> {
    let mut at = 5;
    let mut found = Vec::new();
    while at < words.len() {
        let word = words[at];
        let (count, opcode) = ((word >> 16) as usize, (word & 0xffff) as u16);
        found.push((opcode, words[at + 1..at + count].to_vec()));
        at += count;
    }
    found
}

/// What each constant id names.
///
/// # Invariant
/// A barrier's scope is an **id**, not a literal — at SPIR-V 1.3 the operands name an
/// `OpConstant` — so reading the scope back means resolving the id, or every scope would read as
/// the number the constant happens to sit at.
fn constants(words: &[u32]) -> HashMap<u32, u32> {
    instructions(words)
        .into_iter()
        .filter(|(opcode, _)| *opcode == CONSTANT)
        .filter_map(|(_, operands)| Some((operands[1], *operands.get(2)?)))
        .collect()
}

/// The scope every `OpControlBarrier` in the module is emitted at.
///
/// # Invariant
/// Both operands name one id, so a single answer is the whole barrier — and the module carries no
/// other barrier, which is what makes "a workgroup sync and nothing else" a statement about the
/// module rather than about one instruction in it.
fn barrier_scopes(words: &[u32]) -> Vec<u32> {
    let values = constants(words);
    instructions(words)
        .into_iter()
        .filter(|(opcode, _)| *opcode == CONTROL_BARRIER)
        .map(|(_, operands)| values[&operands[0]])
        .collect()
}

/// `slice[i] = in[i]` for `i = lane % count`, then `out[lane] = Σ_j slice[j]`.
///
/// # Invariant
/// **Every lane writes the same value into every slot**, so the shared path has no write to race
/// on and the two placements owe the same number: lane `L` of a subgroup holds `in[j]` in its own
/// register, and a read at index `j` names exactly that lane. That is what makes this a
/// differential rather than two answers to compare by eye.
fn sums_a_staged_tile(space: Space, count: u32) -> KernelFragment {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    body.add_param(entry);
    let lane = body.add_param(entry);
    let width = body.add_const(entry, ScalarClass::Int, i64::from(count));
    let ordinal = body.add_const(entry, ScalarClass::Int, 0);
    let position = body.add_const(entry, ScalarClass::Int, 0);
    // **The slot is the lane's own**, which is what `Space::Warp` states a body owes.
    let slot = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
        vec![lane, width],
        vec![ScalarClass::Int],
    );
    let datum = body.add_op(
        entry,
        KernelInstr::BufferReadCall(ScalarClass::Int),
        vec![position, slot],
        vec![ScalarClass::Int],
    );
    let write = body.add_op(
        entry,
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        vec![ordinal, slot, datum],
        vec![ScalarClass::Int],
    );
    let mut total = None;
    let mut first_read = None;
    for index in 0..count {
        let address = body.add_const(entry, ScalarClass::Int, i64::from(index));
        let read = body.add_op(
            entry,
            KernelInstr::SliceReadCall(ScalarClass::Int),
            vec![ordinal, address, write],
            vec![ScalarClass::Int],
        );
        if first_read.is_none() {
            first_read = Some(read);
        }
        total = Some(match total {
            None => read,
            Some(folded) => body.add_op(
                entry,
                KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
                vec![folded, read],
                vec![ScalarClass::Int],
            ),
        });
    }
    let folded = total.expect("a tile of one element folds");
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![position, lane, folded],
        Vec::new(),
    );
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![folded],
        },
    );
    // **One pair, not one per read**: one barrier serves every read after the first.
    body.barriers.push(Barrier {
        write,
        read: first_read.expect("a tile of one element is read"),
    });
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(LOCAL_SIZE_X),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count,
            space,
        }],
        marks: Vec::new(),
    }
}

/// The first access this body makes to its slice, and the position it sits at.
///
/// # Invariant
/// The gate is read off the **slice**, so it fires at whichever access comes first — here the
/// write, which is what the emitter sees before it has anything to exchange. Read back off the
/// block rather than counted: the `at` a refusal carries is the definition's own position, and a
/// body that gained a step must not make the number it is refused at drift.
fn first_slice_access_at(fragment: &KernelFragment) -> (ValueId, usize) {
    let access = fragment.body.blocks[0]
        .instrs
        .iter()
        .copied()
        .find(|&value| {
            matches!(
                fragment.body.values[value.0 as usize],
                ValueDef::Instr {
                    op: KernelInstr::SliceWriteCall(_) | KernelInstr::SliceReadCall(_),
                    ..
                }
            )
        })
        .expect("the body addresses its slice");
    let at = fragment.body.blocks[0]
        .instrs
        .iter()
        .position(|&value| value == access)
        .expect("the access is an instruction of the entry block");
    (access, at)
}

/// The same body with its recorded pair dropped, which is what makes the pair **owed** rather than
/// already spent.
///
/// # Invariant
/// The scan treats a recorded barrier as one already between the write and the read, so it answers
/// `[]` for a body that holds one. Asking it about the unrecorded body is what says the barrier
/// cannot simply be left out — which the emitter would otherwise emit a run without.
fn unrecorded(fragment: &KernelFragment) -> KernelFragment {
    let mut fragment = fragment.clone();
    fragment.body.barriers.clear();
    fragment
}

/// The same number, reached through the shuffle exchange and through shared memory.
///
/// # Invariant
/// Each lane reads `SUBGROUP_WIDTH` values that live in **other lanes' registers**, so a shuffle
/// naming the wrong lane, or a barrier arriving too late, would answer a different sum rather than
/// the same one. Both runs are differenced against the hand-derived `1 + … + SUBGROUP_WIDTH` too,
/// because two placements agreeing on a wrong number is still wrong.
#[test]
fn a_warp_exchange_answers_what_the_shared_exchange_answers() {
    let Some(context) = common::context("a_warp_exchange_answers_what_the_shared_exchange_answers")
    else {
        return;
    };
    let expected: i64 = (1..=i64::from(SUBGROUP_WIDTH)).sum();
    let data: Vec<i64> = (1..=i64::from(LOCAL_SIZE_X)).collect();
    let packed: Vec<u8> = data.iter().flat_map(|word| word.to_le_bytes()).collect();

    let mut answers = Vec::new();
    for (space, what) in [
        (Space::Warp, "the warp exchange"),
        (Space::Shared, "shared memory"),
    ] {
        let fragment = sums_a_staged_tile(space, SUBGROUP_WIDTH);
        fragment
            .body
            .validate()
            .unwrap_or_else(|broken| panic!("{what} is well formed: {broken}"));
        let lane = fragment.body.parameters()[1];
        let owed = unrecorded(&fragment)
            .barrier_scan(lane)
            .unwrap_or_else(|refused| panic!("{what} owes one barrier: {refused}"));
        assert_eq!(
            owed,
            vec![fragment.body.barriers[0]],
            "{what}: the pair the body records is the one the scan owes, so the \
             barrier cannot be left out"
        );
        let resident = context
            .run(
                &LaunchSet::single(&fragment),
                &[BufferSlot::Host(&packed)],
                LOCAL_SIZE_X as usize,
            )
            .unwrap_or_else(|refusal| panic!("{what} is dispatched: {refusal}"));
        let answer = words(
            context
                .fetch(resident[0], LOCAL_SIZE_X as usize)
                .expect("the answer comes back"),
        );
        context.release(resident[0]);
        assert!(
            answer.iter().all(|&element| element == expected),
            "{what} folds every slot of the writer's own subgroup: {answer:?}"
        );
        answers.push(answer[0]);
    }
    assert_eq!(
        answers[0], answers[1],
        "the exchange and shared memory answer the same fold"
    );
}

/// A slice reaching past one subgroup is refused, and the refusal says what it would have read.
///
/// # Invariant
/// A read at index `j` names the lane `j` of the reader's own subgroup, so a slice of 64 names
/// lane 32 of a 32-lane subgroup — one the reader cannot reach, which the specification leaves
/// undefined rather than merely wrong. Placing it in shared memory instead would compute a
/// *different program*, so it is refused by name
/// (`docs/notes/shared-memory-and-barriers.md` §5).
#[test]
fn a_warp_slice_reaching_past_one_subgroup_is_refused_by_name() {
    let fragment = sums_a_staged_tile(Space::Warp, LOCAL_SIZE_X);
    fragment
        .body
        .validate()
        .expect("the body is well formed; it is the *placement* that is refused");
    let (_, at) = first_slice_access_at(&fragment);
    let refusal = spirv::compile(
        &LaunchSet::single(&fragment),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect_err("a slice wider than a subgroup is refused, not shared");
    match refusal {
        SpirvRefusal::StagingNotOneElementPerLane {
            count,
            subgroup_width,
            at: refused_at,
        } => {
            assert_eq!(count, LOCAL_SIZE_X, "names the slice's count");
            assert_eq!(subgroup_width, SUBGROUP_WIDTH, "names the width");
            assert_eq!(refused_at, at, "names the access that cannot be exchanged");
        }
        other => panic!("refused for the placement, not something else: {other:?}"),
    }
    let message = refusal.to_string();
    assert!(
        message.contains(&LOCAL_SIZE_X.to_string()),
        "names the count: {message}"
    );
    assert!(
        message.contains(&SUBGROUP_WIDTH.to_string()),
        "names the width: {message}"
    );
    assert!(
        message.contains("cannot reach"),
        "says what the read would have named: {message}"
    );
}

/// A read whose readers span more than one warp keeps shared memory and a workgroup barrier.
///
/// # Invariant
/// The same body as the differential, one slot per lane rather than one per subgroup. Asserted is
/// not merely that a barrier exists — the validator cannot see a **missing** one, and a
/// subgroup-scoped one validates — but that it is at `Workgroup` scope and that the module carries
/// no exchange: a stage-wide read ordered by a warp sync reads each sibling's slot unwritten.
#[test]
fn a_cross_warp_read_keeps_shared_memory_and_a_workgroup_barrier() {
    let fragment = sums_a_staged_tile(Space::Shared, LOCAL_SIZE_X);
    fragment
        .body
        .validate()
        .expect("a cross-warp slice is shared memory, which this target emits");
    let words = spirv::compile(
        &LaunchSet::single(&fragment),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect("a cross-warp slice is emitted");

    assert_eq!(
        barrier_scopes(&words),
        vec![SCOPE_WORKGROUP],
        "the stage is ordered at the workgroup's scope, never a subgroup's"
    );
    let seen = instructions(&words);
    let opcodes = seen.iter().map(|(opcode, _)| *opcode).collect::<Vec<_>>();
    assert!(
        !opcodes.contains(&GROUP_NON_UNIFORM_SHUFFLE),
        "a cross-warp read is not exchanged: {opcodes:?}"
    );
    assert!(
        !seen.iter().any(|(opcode, operands)| *opcode == CAPABILITY
            && operands.contains(&GROUP_NON_UNIFORM_SHUFFLE_CAPABILITY)),
        "the module asks for no exchange it does not use: {opcodes:?}"
    );
    assert!(
        seen.iter()
            .filter(|(opcode, _)| *opcode == EXECUTION_MODE)
            .all(|(_, operands)| operands[1] == EXECUTION_MODE_LOCAL_SIZE),
        "no execution mode narrows a workgroup this target schedules whole: {opcodes:?}"
    );
}

/// A `Warp` slice carries its own narrow barrier, and the module still validates.
///
/// # Invariant
/// The scope a pair is ordered at follows the placement rather than the body: a subgroup-scoped
/// barrier is what makes each lane's register visible to its siblings, and it is legal precisely
/// because every reader of the slice is inside the writing lane's subgroup.
#[test]
fn a_warp_exchange_is_ordered_by_a_subgroup_barrier_and_validates() {
    let fragment = sums_a_staged_tile(Space::Warp, SUBGROUP_WIDTH);
    fragment
        .body
        .validate()
        .expect("the warp body is well formed");
    let words = spirv::compile(
        &LaunchSet::single(&fragment),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect("a warp-placed slice is emitted");

    assert_eq!(
        barrier_scopes(&words),
        vec![SCOPE_SUBGROUP],
        "the one recorded pair is ordered inside the subgroup, not the workgroup"
    );
    let seen = instructions(&words);
    let opcodes = seen.iter().map(|(opcode, _)| *opcode).collect::<Vec<_>>();
    assert!(
        opcodes.contains(&GROUP_NON_UNIFORM_SHUFFLE),
        "every staged read is an exchange: {opcodes:?}"
    );
    assert!(
        seen.iter().any(|(opcode, operands)| *opcode == CAPABILITY
            && operands.contains(&GROUP_NON_UNIFORM_SHUFFLE_CAPABILITY)),
        "the module declares the capability the exchange needs: {opcodes:?}"
    );
    if !validate("the warp module", &words) {
        eprintln!("the warp module was not validated");
    }
}

/// Hand one emitted module to `spirv-val`, or say it was not covered.
///
/// # Invariant
/// The return says whether the module was actually checked: a machine without the validator must
/// not look like one that validated. This is the only check that can see a **malformed** exchange
/// — a missing scope operand reads as a truncated instruction rather than as a wrong answer — and
/// the device answers such a module rather than refusing it.
fn validate(what: &str, words: &[u32]) -> bool {
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let mut child = match Command::new(VALIDATOR)
        .args(["--target-env", "vulkan1.1", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "SKIPPED {what}: `{VALIDATOR}` is not on PATH, so that module was not validated"
            );
            return false;
        }
        Err(error) => panic!("`{VALIDATOR}` is on PATH but could not be started: {error}"),
    };
    child
        .stdin
        .take()
        .expect("standard input was piped")
        .write_all(&bytes)
        .expect("the module is handed to the validator");
    let output = child.wait_with_output().expect("the validator finishes");
    assert!(
        output.status.success(),
        "`{VALIDATOR}` rejected {what}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

/// A fetched payload as the `i64` elements these integer fragments produce.
///
/// # Invariant
/// The class is checked: a fetch classed as floats from an integer fragment would be the ABI
/// reading the buffer at the wrong width.
fn words(data: lichen_kernel_ir::ScalarData) -> Vec<i64> {
    match data {
        lichen_kernel_ir::ScalarData::Int(elements) => elements,
        lichen_kernel_ir::ScalarData::Float(elements) => {
            panic!(
                "an integer run's result came back as {} float element(s)",
                elements.len()
            )
        }
    }
}
