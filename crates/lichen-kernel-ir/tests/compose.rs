//! Two fragments in a chain become one: the mechanics and every refusal.
//! See `docs/notes/graph-fusion.md` §4.

use lichen_kernel_ir::{
    Br, ComposeRefusal, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelMark,
    KernelRoles, KernelShape, LaneWidth, MarkField, ScalarClass, Slice, Space, TargetFacts,
    Terminator, ValueDef, ValueId, compose,
};

/// The lane count one workgroup runs.
const LANES: u32 = 64;

/// The domain both fragments share: the launch's extent, then the invocation index.
fn shape() -> KernelShape {
    KernelShape::Tuple(vec![
        KernelShape::Scalar(ScalarClass::Int),
        KernelShape::Scalar(ScalarClass::Int),
    ])
}

fn target() -> TargetFacts {
    TargetFacts {
        int_width: IntWidth::I64,
        lane_width: LaneWidth::new(LANES),
    }
}

fn fragment(body: KernelBody, inputs: usize, input_classes: Vec<ScalarClass>) -> KernelFragment {
    KernelFragment {
        param_shape: shape(),
        roles: KernelRoles::default(),
        body,
        inputs,
        outputs: 1,
        input_classes,
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Int],
        target: target(),
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// A one-block producer that folds nothing and stages one `Float`.
fn producer_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let index = body.add_param(entry);
    let position = body.add_const(entry, ScalarClass::Int, 0);
    let forwarded = body.add_const(entry, ScalarClass::Float, 42);
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![position, index, forwarded],
        Vec::new(),
    );
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![position],
        },
    );
    body
}

fn producer(inputs: usize, input_classes: Vec<ScalarClass>) -> KernelFragment {
    fragment(producer_body(), inputs, input_classes)
}

/// A producer that reads one buffer, at `position`, and stages what it read.
///
/// # Invariant
/// The read's address is a constant, so what it produces does not depend on the lane: a
/// lane-dependent forward is a cross-lane read and is refused before this one is reached.
fn producer_reading(position: i64) -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let index = body.add_param(entry);
    let at = body.add_const(entry, ScalarClass::Int, position);
    let address = body.add_const(entry, ScalarClass::Int, 0);
    let read = body.add_op(
        entry,
        KernelInstr::BufferReadCall(ScalarClass::Float),
        vec![at, address],
        vec![ScalarClass::Float],
    );
    let out = body.add_const(entry, ScalarClass::Int, 0);
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![out, index, read],
        Vec::new(),
    );
    let dummy = body.add_const(entry, ScalarClass::Int, 0);
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![dummy],
        },
    );
    body
}

/// A one-block producer whose forwarded value is the lane's own index as a `Float`.
///
/// # Invariant
/// The write fills `index`, so every lane writes the element it computed: a forwarded value
/// reachable from the invocation index whose read is at that same address is bindable in-lane.
fn lane_producer_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let index = body.add_param(entry);
    let forwarded = body.add_op(
        entry,
        KernelInstr::Conv {
            from: ScalarClass::Int,
            to: ScalarClass::Float,
        },
        vec![index],
        vec![ScalarClass::Float],
    );
    let out = body.add_const(entry, ScalarClass::Int, 0);
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![out, index, forwarded],
        Vec::new(),
    );
    let dummy = body.add_const(entry, ScalarClass::Int, 0);
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![dummy],
        },
    );
    body
}

/// A one-block consumer that reads each of `positions` and stages their sum.
///
/// # Invariant
/// `shifted` addresses the **last** read one element past the invocation index, which is a
/// value no entry parameter is: a shape read-site recomputation serves and in-lane binding does
/// not.
fn consumer_body(positions: &[i64]) -> KernelBody {
    consumer_reading(positions, false)
}

fn consumer_reading(positions: &[i64], shifted: bool) -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let extent = body.add_param(entry);
    let index = body.add_param(entry);
    // The extent is read so that the entry block's own leaves are both live.
    let _used = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Eq),
        vec![extent, index],
        vec![ScalarClass::Int],
    );
    let last = positions.len().saturating_sub(1);
    let mut reads = Vec::new();
    for (slot, position) in positions.iter().enumerate() {
        let position = body.add_const(entry, ScalarClass::Int, *position);
        let address = if shifted && slot == last {
            let one = body.add_const(entry, ScalarClass::Int, 1);
            body.add_op(
                entry,
                KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
                vec![index, one],
                vec![ScalarClass::Int],
            )
        } else {
            index
        };
        reads.push(body.add_op(
            entry,
            KernelInstr::BufferReadCall(ScalarClass::Float),
            vec![position, address],
            vec![ScalarClass::Float],
        ));
    }
    let mut value = reads[0];
    for read in &reads[1..] {
        value = body.add_op(
            entry,
            KernelInstr::Bin(ScalarClass::Float, KernelBin::Add),
            vec![value, *read],
            vec![ScalarClass::Float],
        );
    }
    let out = body.add_const(entry, ScalarClass::Int, 0);
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![out, index, value],
        Vec::new(),
    );
    let dummy = body.add_const(entry, ScalarClass::Int, 0);
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![dummy],
        },
    );
    body
}

fn consumer() -> KernelFragment {
    fragment(
        consumer_body(&[0, 1]),
        2,
        vec![ScalarClass::Float, ScalarClass::Float],
    )
}

/// The position each buffer read in `body` addresses, in table order.
fn read_positions(body: &KernelBody) -> Vec<i64> {
    body.blocks
        .iter()
        .flat_map(|block| block.instrs.iter())
        .filter_map(|&value| match &body.values[value.0 as usize] {
            ValueDef::Instr {
                op: KernelInstr::BufferReadCall(_),
                args,
                ..
            } => constant_of(body, args[0]),
            _ => None,
        })
        .collect()
}

/// How many instructions of `body` are buffer writes.
fn write_count(body: &KernelBody) -> usize {
    body.blocks
        .iter()
        .flat_map(|block| block.instrs.iter())
        .filter(|&&value| {
            matches!(
                body.values[value.0 as usize],
                ValueDef::Instr {
                    op: KernelInstr::BufferWriteCall(_),
                    ..
                }
            )
        })
        .count()
}

/// The integer constant `value` holds, when it holds one.
fn constant_of(body: &KernelBody, value: ValueId) -> Option<i64> {
    match &body.values[value.0 as usize] {
        ValueDef::Instr {
            op: KernelInstr::Const(ScalarClass::Int, bits),
            ..
        } => Some(*bits),
        _ => None,
    }
}

/// Whether every `BlockParam` is a parameter of the block it names, at the index it names.
///
/// # Invariant
/// The artifact reader re-derives a parameter's index from the block that receives it, so an orphan
/// passes `KernelBody::validate` and is refused only when the artifact is read.
fn assert_parameters_are_numbered(body: &KernelBody) {
    let mut listed = Vec::new();
    for (block, basic) in body.blocks.iter().enumerate() {
        for (index, &param) in basic.params.iter().enumerate() {
            listed.push(param);
            assert_eq!(
                body.values[param.0 as usize],
                ValueDef::BlockParam {
                    block,
                    index: index as u32
                },
                "parameter {param:?} is the {index}-th of block {block}"
            );
        }
    }
    for (index, definition) in body.values.iter().enumerate() {
        if matches!(definition, ValueDef::BlockParam { .. }) {
            assert!(
                listed.contains(&ValueId(index as u32)),
                "value {index} is a parameter no block receives"
            );
        }
    }
}

fn refusal(fragment: &KernelFragment, absorbed: u32) -> ComposeRefusal {
    compose(&producer(1, vec![ScalarClass::Float]), fragment, absorbed)
        .expect_err("the fragments do not compose")
}

#[test]
fn a_chain_composes_and_compacts() {
    let producer = producer(1, vec![ScalarClass::Float]);
    let consumer = consumer();
    let merged = compose(&producer, &consumer, 1).expect("the chain composes");

    merged
        .body
        .validate()
        .expect("the merged body is well formed");
    merged
        .validate()
        .expect("the merged fragment is well formed");
    assert_eq!(
        (merged.inputs, merged.input_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "the merged inputs are the consumer's with the absorbed position removed"
    );
    assert_eq!(
        (merged.outputs, merged.output_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "the codomain is the consumer's"
    );
    assert_eq!(
        merged.result_classes,
        vec![ScalarClass::Int],
        "the results are the consumer's"
    );
    assert_eq!(
        merged.param_shape,
        shape(),
        "the shape both fragments already share"
    );
    assert_eq!(
        merged.roles,
        KernelRoles::default(),
        "a merged fragment has no single parameter struct for a path into roles to name"
    );
    assert!(merged.slices.is_empty(), "neither body stages");
    assert!(merged.marks.is_empty(), "neither body marks");

    assert_eq!(
        merged.body.blocks.len(),
        1,
        "the consumer's entry block is inlined into the producer's return block, not spliced"
    );
    assert_eq!(
        merged.body.values.len(),
        9,
        "the merge drops the entry's parameters, the absorbed read, its operands, the producer's \
         store and the dead position constant, and the producer's dead return value"
    );
    assert!(
        merged.body.values.len() < producer.body.values.len() + consumer.body.values.len(),
        "compaction removes what substitution and inlining left dead"
    );
    assert_eq!(
        read_positions(&merged.body),
        vec![0],
        "the absorbed read is gone and the surviving read still addresses position 0"
    );
    assert_eq!(
        write_count(&merged.body),
        1,
        "the producer's store goes: its buffer is not an output of the merged fragment"
    );
    assert_parameters_are_numbered(&merged.body);

    // The consumer's computation reads the producer's forwarded value where it read the buffer.
    let sum = merged
        .body
        .values
        .iter()
        .enumerate()
        .find_map(|(value, definition)| match definition {
            ValueDef::Instr {
                op: KernelInstr::Bin(ScalarClass::Float, KernelBin::Add),
                args,
                ..
            } => Some((ValueId(value as u32), args.clone())),
            _ => None,
        })
        .expect("the consumer's element computation survives the merge");
    let kinds: Vec<&str> = sum
        .1
        .iter()
        .map(|arg| match &merged.body.values[arg.0 as usize] {
            ValueDef::Instr {
                op: KernelInstr::Const(ScalarClass::Float, _),
                ..
            } => "the producer's forwarded value",
            ValueDef::Instr {
                op: KernelInstr::BufferReadCall(_),
                ..
            } => "the consumer's read",
            other => panic!("the sum reads {other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["the consumer's read", "the producer's forwarded value"],
        "the absorbed read is bound to what the producer's write staged"
    );
    let staged = merged
        .body
        .values
        .iter()
        .find_map(|definition| match definition {
            ValueDef::Instr {
                op: KernelInstr::BufferWriteCall(_),
                args,
                ..
            } => Some(args.clone()),
            _ => None,
        })
        .expect("the consumer's write survives");
    assert_eq!(
        staged[2], sum.0,
        "the one store left writes the consumer's computation, not the producer's fold"
    );

    // The producer's return is gone and the consumer's is the merged body's.
    let Terminator::Return { values } = &merged.body.blocks[0].terminator else {
        panic!("the merged body returns in its entry block")
    };
    assert_eq!(
        values.len(),
        1,
        "the merged body returns what the consumer's entry block returned"
    );
    assert!(
        matches!(
            merged.body.values[values[0].0 as usize],
            ValueDef::Instr {
                op: KernelInstr::Const(ScalarClass::Int, 0),
                ..
            }
        ),
        "the consumer's own result survives"
    );
}

#[test]
fn a_read_above_the_absorbed_position_moves_down_one() {
    let consumer = fragment(consumer_body(&[0, 1, 2]), 3, vec![ScalarClass::Float; 3]);
    let merged =
        compose(&producer(1, vec![ScalarClass::Float]), &consumer, 1).expect("the chain composes");
    merged
        .body
        .validate()
        .expect("the merged body is well formed");
    assert_eq!(
        (merged.inputs, merged.input_classes.clone()),
        (2, vec![ScalarClass::Float, ScalarClass::Float]),
        "the absorbed position leaves the consumer's other two"
    );
    assert_eq!(
        read_positions(&merged.body),
        vec![0, 1],
        "a read at position 2 addresses position 1 once the absorbed input is gone"
    );
}

#[test]
fn compaction_drops_an_orphan_parameter() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let extent = body.add_param(entry);
    let index = body.add_param(entry);
    let carried = body.add_block();
    let _orphan = body.add_param(carried);
    body.set_terminator(
        entry,
        Terminator::Br(Br {
            target: carried,
            args: vec![extent],
        }),
    );
    let position = body.add_const(carried, ScalarClass::Int, 0);
    let forwarded = body.add_const(carried, ScalarClass::Float, 42);
    body.add_op(
        carried,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![position, index, forwarded],
        Vec::new(),
    );
    body.set_terminator(
        carried,
        Terminator::Return {
            values: vec![position],
        },
    );

    let merged = compose(&fragment(body, 1, vec![ScalarClass::Float]), &consumer(), 1)
        .expect("the chain composes");
    merged
        .body
        .validate()
        .expect("the merged body is well formed");
    let Terminator::Br(branch) = &merged.body.blocks[0].terminator else {
        panic!("the producer's entry block still branches to its return block")
    };
    assert!(
        merged.body.blocks[branch.target].params.is_empty(),
        "a parameter nothing reads is removed from the block that receives it"
    );
    assert!(
        branch.args.is_empty(),
        "and with the branch argument that fed it"
    );
    assert_parameters_are_numbered(&merged.body);
}

#[test]
fn different_domains_are_refused() {
    let mut consumer = consumer();
    consumer.param_shape = KernelShape::Tuple(vec![KernelShape::Scalar(ScalarClass::Int)]);
    let refusal = refusal(&consumer, 1);
    assert_eq!(refusal.name(), "ShapesDiffer");
    assert!(
        matches!(refusal, ComposeRefusal::ShapesDiffer { .. }),
        "{refusal}"
    );
}

#[test]
fn different_targets_are_refused() {
    let mut consumer = consumer();
    consumer.target.lane_width = LaneWidth::new(LANES / 2);
    let refusal = refusal(&consumer, 1);
    assert_eq!(refusal.name(), "TargetsDiffer");
}

#[test]
fn a_producer_class_the_input_does_not_expect_is_refused() {
    let mut consumer = consumer();
    consumer.input_classes = vec![ScalarClass::Float, ScalarClass::Int];
    let refusal = refusal(&consumer, 1);
    assert!(
        matches!(
            refusal,
            ComposeRefusal::ClassesDiffer {
                produced: ScalarClass::Float,
                expected: ScalarClass::Int
            }
        ),
        "the refusal names both classes: {refusal}"
    );
    assert!(
        refusal.message().contains("Float") && refusal.message().contains("Int"),
        "the refusal names both classes: {refusal}"
    );
}

#[test]
fn entry_blocks_of_different_arity_are_refused() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let extent = body.add_param(entry);
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![extent],
        },
    );
    let mut consumer = consumer();
    consumer.body = body;
    let refusal = refusal(&consumer, 1);
    assert_eq!(refusal.name(), "ParameterCountsDiffer");
}

#[test]
fn an_absorbed_position_the_consumer_does_not_declare_is_refused() {
    let refusal = refusal(&consumer(), 2);
    assert!(
        matches!(
            refusal,
            ComposeRefusal::AbsorbedIsOutOfRange {
                absorbed: 2,
                inputs: 2
            }
        ),
        "the refusal names the position and the count: {refusal}"
    );
}

#[test]
fn a_position_the_consumer_never_reads_is_refused() {
    let consumer = fragment(consumer_body(&[0]), 2, vec![ScalarClass::Float; 2]);
    let refusal = refusal(&consumer, 1);
    assert!(
        matches!(refusal, ComposeRefusal::AbsorbedIsNotRead { absorbed: 1 }),
        "the refusal names the position: {refusal}"
    );
}

#[test]
fn a_read_whose_position_is_not_a_constant_is_refused() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let index = body.add_param(entry);
    let read = body.add_op(
        entry,
        KernelInstr::BufferReadCall(ScalarClass::Float),
        vec![index, index],
        vec![ScalarClass::Float],
    );
    let out = body.add_const(entry, ScalarClass::Int, 0);
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![out, index, read],
        Vec::new(),
    );
    let dummy = body.add_const(entry, ScalarClass::Int, 0);
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![dummy],
        },
    );
    let refusal = refusal(&fragment(body, 2, vec![ScalarClass::Float; 2]), 1);
    assert_eq!(refusal.name(), "ReadPositionIsNotAConstant");
}

#[test]
fn a_producer_reading_the_absorbed_input_is_refused() {
    let producer = fragment(
        producer_reading(1),
        2,
        vec![ScalarClass::Float, ScalarClass::Float],
    );
    let refusal = compose(&producer, &consumer(), 1)
        .expect_err("the producer reads the buffer the consumer absorbs");
    assert!(
        matches!(
            refusal,
            ComposeRefusal::ProducerReadsTheAbsorbedInput { position: 1 }
        ),
        "{refusal}"
    );
}

#[test]
fn a_producer_reading_an_input_the_merge_lacks_is_refused() {
    let producer = fragment(producer_reading(2), 3, vec![ScalarClass::Float; 3]);
    let refusal =
        compose(&producer, &consumer(), 1).expect_err("the merged fragment declares one input");
    assert!(
        matches!(
            refusal,
            ComposeRefusal::ProducerReadsOutsideTheMerge {
                position: 2,
                inputs: 1
            }
        ),
        "{refusal}"
    );
}

#[test]
fn a_producer_that_writes_nothing_is_refused() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let extent = body.add_param(entry);
    let _index = body.add_param(entry);
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![extent],
        },
    );
    let refusal = compose(&fragment(body, 1, vec![ScalarClass::Float]), &consumer(), 1)
        .expect_err("a producer that writes nothing forwards nothing");
    assert!(
        matches!(refusal, ComposeRefusal::ProducerWritesNothing),
        "{refusal}"
    );
}

#[test]
fn a_producer_that_writes_several_buffers_is_refused() {
    let mut body = producer_body();
    let entry = body.entry;
    let index = body.blocks[entry].params[1];
    let position = body.add_const(entry, ScalarClass::Int, 1);
    let value = body.add_const(entry, ScalarClass::Float, 7);
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![position, index, value],
        Vec::new(),
    );
    let refusal = compose(&fragment(body, 1, vec![ScalarClass::Float]), &consumer(), 1)
        .expect_err("two writes name two outputs");
    assert!(
        matches!(refusal, ComposeRefusal::ProducerWritesSeveral { writes: 2 }),
        "{refusal}"
    );
}

#[test]
fn a_producer_returning_from_two_blocks_is_refused() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let extent = body.add_param(entry);
    let index = body.add_param(entry);
    let zero = body.add_const(entry, ScalarClass::Int, 0);
    let value = body.add_const(entry, ScalarClass::Float, 7);
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![zero, index, value],
        Vec::new(),
    );
    let cond = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Eq),
        vec![extent, zero],
        vec![ScalarClass::Int],
    );
    let left = body.add_block();
    let right = body.add_block();
    body.set_terminator(
        entry,
        Terminator::CondBr {
            cond,
            if_true: Br {
                target: left,
                args: Vec::new(),
            },
            if_false: Br {
                target: right,
                args: Vec::new(),
            },
        },
    );
    let refusal = compose(&fragment(body, 1, vec![ScalarClass::Float]), &consumer(), 1)
        .expect_err("two return blocks cannot take one inlined entry");
    assert!(
        matches!(
            refusal,
            ComposeRefusal::ProducerDoesNotReturnInOneBlock { returns: 2 }
        ),
        "{refusal}"
    );
}

#[test]
fn a_consumer_whose_entry_is_branched_to_is_refused() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let extent = body.add_param(entry);
    let index = body.add_param(entry);
    let zero = body.add_const(entry, ScalarClass::Int, 0);
    let cond = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Eq),
        vec![extent, zero],
        vec![ScalarClass::Int],
    );
    let exit = body.add_block();
    body.set_terminator(
        entry,
        Terminator::CondBr {
            cond,
            if_true: Br {
                target: exit,
                args: Vec::new(),
            },
            if_false: Br {
                target: entry,
                args: vec![extent, index],
            },
        },
    );
    let out = body.add_const(exit, ScalarClass::Int, 0);
    let value = body.add_const(exit, ScalarClass::Float, 7);
    body.add_op(
        exit,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![out, index, value],
        Vec::new(),
    );
    let dummy = body.add_const(exit, ScalarClass::Int, 0);
    body.set_terminator(
        exit,
        Terminator::Return {
            values: vec![dummy],
        },
    );
    let refusal = refusal(&fragment(body, 2, vec![ScalarClass::Float; 2]), 1);
    assert!(
        matches!(
            refusal,
            ComposeRefusal::ConsumerEntryIsABranchTarget { block: 0 }
        ),
        "the refusal names the block that branches there: {refusal}"
    );
}

/// A lane-dependent value read at the producer's own address is bound in-lane, not refused.
///
/// # Invariant
/// The lane dependence is what makes the value recomputable: every lane wrote the element it
/// computed, and the read asks for that element, so the binding needs no slice and no barrier.
#[test]
fn a_lane_dependent_value_read_at_its_own_address_is_bound_in_lane() {
    let producer = fragment(lane_producer_body(), 1, vec![ScalarClass::Float]);
    let merged = compose(&producer, &consumer(), 1).expect("the chain composes in-lane");
    merged
        .body
        .validate()
        .expect("the merged body is well formed");
    merged
        .validate()
        .expect("the merged fragment is well formed");
    assert_eq!(
        read_positions(&merged.body),
        vec![0],
        "the absorbed read is gone: the value is the lane's own, not a buffer's"
    );
    let sum = merged
        .body
        .values
        .iter()
        .enumerate()
        .find_map(|(value, definition)| match definition {
            ValueDef::Instr {
                op: KernelInstr::Bin(ScalarClass::Float, KernelBin::Add),
                args,
                ..
            } => Some((ValueId(value as u32), args.clone())),
            _ => None,
        })
        .expect("the consumer's element computation survives the merge");
    let conv = merged
        .body
        .values
        .iter()
        .enumerate()
        .find_map(|(value, definition)| match definition {
            ValueDef::Instr {
                op:
                    KernelInstr::Conv {
                        from: ScalarClass::Int,
                        to: ScalarClass::Float,
                    },
                ..
            } => Some(ValueId(value as u32)),
            _ => None,
        })
        .expect("the producer's own conversion survives the merge");
    assert!(
        sum.1.contains(&conv),
        "the consumer's computation reads the lane's own conversion, not the buffer: {sum:?}"
    );
}

/// A lane-dependent value the consumer reads at another address is refused, naming the read.
///
/// # Invariant
/// The read asks for `index + 1`, which no lane wrote: answering it in-lane would hand a lane
/// its neighbour's value, and the shape that serves it is read-site recomputation — a later
/// slice this crate must not fake.
#[test]
fn a_lane_dependent_value_read_at_another_address_is_refused() {
    let producer = fragment(lane_producer_body(), 1, vec![ScalarClass::Float]);
    let consumer = fragment(
        consumer_reading(&[0, 1], true),
        2,
        vec![ScalarClass::Float; 2],
    );
    let refusal = compose(&producer, &consumer, 1)
        .expect_err("a value reachable from the lane cannot be bound in-lane");
    assert_eq!(refusal.name(), "ForwardedValueIsLaneDependent");
    assert!(
        matches!(
            refusal,
            ComposeRefusal::ForwardedValueIsLaneDependent { read, .. } if read
                == buffer_reads_at(&consumer.body, 1)[0]
        ),
        "the refusal names the read that puts it out of reach: {refusal}"
    );
    let message = refusal.message();
    assert!(
        message.contains("index") && message.contains("address"),
        "the refusal says what the value depends on and what the read asks for: {message}"
    );
}

/// The value ids of the reads addressing `position`, in table order.
fn buffer_reads_at(body: &KernelBody, position: i64) -> Vec<ValueId> {
    body.blocks
        .iter()
        .flat_map(|block| block.instrs.iter().copied())
        .filter(|value| match &body.values[value.0 as usize] {
            ValueDef::Instr {
                op: KernelInstr::BufferReadCall(_),
                args,
                ..
            } => constant_of(body, args[0]) == Some(position),
            _ => false,
        })
        .collect()
}

#[test]
fn a_producers_marks_go_with_its_store() {
    let mark = |value: ValueId| KernelMark {
        value,
        payload: vec![MarkField {
            name: None,
            text: "combine".to_string(),
        }],
    };
    let mut producer = producer(1, vec![ScalarClass::Float]);
    let mut consumer = consumer();
    producer.marks = vec![mark(ValueId(3))];
    consumer.marks = vec![mark(ValueId(4))];
    let merged = compose(&producer, &consumer, 1)
        .expect("a producer's mark names a role of an interface the merge drops");
    assert_eq!(
        merged.marks.len(),
        1,
        "the surviving marks are the consumer's"
    );
    assert!(
        matches!(
            merged.body.values[merged.marks[0].value.0 as usize],
            ValueDef::Instr {
                op: KernelInstr::BufferReadCall(_),
                ..
            }
        ),
        "the mark that survives still rides the consumer's own value"
    );
}

#[test]
fn two_marks_the_merge_keeps_are_refused() {
    let mark = |value: ValueId| KernelMark {
        value,
        payload: vec![MarkField {
            name: None,
            text: "combine".to_string(),
        }],
    };
    let mut consumer = consumer();
    consumer.marks = vec![mark(ValueId(3)), mark(ValueId(4))];
    let refusal = refusal(&consumer, 1);
    assert!(
        matches!(refusal, ComposeRefusal::DuplicatedMark { ref text } if text == "combine"),
        "the refusal names the text: {refusal}"
    );
    assert!(
        refusal.message().contains("combine"),
        "the refusal names the text: {refusal}"
    );
}

#[test]
fn two_slices_are_refused() {
    let mut producer = producer(1, vec![ScalarClass::Float]);
    let mut consumer = consumer();
    let slice = Slice {
        class: ScalarClass::Float,
        count: LANES,
        space: Space::Shared,
    };
    producer.slices = vec![slice];
    consumer.slices = vec![slice];
    let refusal = compose(&producer, &consumer, 1).expect_err("two slices are not one");
    assert!(
        matches!(
            refusal,
            ComposeRefusal::SlicesCollide {
                producer: 0,
                consumer: 0
            }
        ),
        "the refusal names both ordinals: {refusal}"
    );
}

#[test]
fn a_merged_fragment_a_gate_refuses_is_refused() {
    let mut consumer = consumer();
    consumer.slices = vec![Slice {
        class: ScalarClass::Float,
        count: 0,
        space: Space::Register,
    }];
    let refusal = refusal(&consumer, 1);
    assert_eq!(refusal.name(), "MergedFragmentIsRefused");
}

#[test]
fn a_merge_no_gate_would_accept_is_refused() {
    // The producer's return block is unreachable, so it does not dominate the inlined entry.
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let extent = body.add_param(entry);
    let _index = body.add_param(entry);
    let looped = body.add_block();
    let carried = body.add_param(looped);
    body.set_terminator(
        entry,
        Terminator::Br(Br {
            target: looped,
            args: vec![extent],
        }),
    );
    body.set_terminator(
        looped,
        Terminator::Br(Br {
            target: looped,
            args: vec![carried],
        }),
    );
    let unreachable = body.add_block();
    let address = body.add_param(unreachable);
    let position = body.add_const(unreachable, ScalarClass::Int, 0);
    let value = body.add_const(unreachable, ScalarClass::Float, 42);
    body.add_op(
        unreachable,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![position, address, value],
        Vec::new(),
    );
    body.set_terminator(
        unreachable,
        Terminator::Return {
            values: vec![position],
        },
    );
    let refusal = compose(&fragment(body, 1, vec![ScalarClass::Float]), &consumer(), 1)
        .expect_err("the merged body does not pass its own gate");
    assert_eq!(refusal.name(), "MergedBodyIsRefused");
    assert!(
        refusal.message().contains("KernelBody::validate"),
        "the refusal names the gate: {refusal}"
    );
}

#[test]
fn a_body_a_gate_refuses_is_refused() {
    let mut broken = KernelBody::new();
    let entry = broken.add_block();
    broken.add_param(entry);
    broken.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![ValueId(99), ValueId(98)],
        vec![ScalarClass::Int],
    );
    broken.set_terminator(entry, Terminator::Return { values: Vec::new() });
    let refusal = compose(
        &fragment(broken.clone(), 1, vec![ScalarClass::Float]),
        &consumer(),
        1,
    )
    .expect_err("a body that is not well formed is refused");
    assert_eq!(refusal.name(), "ProducerIsNotWellFormed");

    let refusal = compose(
        &producer(1, vec![ScalarClass::Float]),
        &fragment(broken, 2, vec![ScalarClass::Float; 2]),
        1,
    )
    .expect_err("a body that is not well formed is refused");
    assert_eq!(refusal.name(), "ConsumerIsNotWellFormed");
}
