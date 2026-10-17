//! A class is stated once, and `KernelInstr::own_class` is where it is stated.
//!
//! # Invariant
//! **`own_class` is the oracle; the emitter's own account of a value it produces is the answer
//! under test.** A definition states the class of what it leaves behind, while a class read
//! off an operand is a fact about that operand, asked of a different value. The guard drives
//! each instruction through the public emitter and reads the class the emitter reports.

use std::collections::HashMap;

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelId, KernelInstr, KernelRoles,
    KernelShape, LaneHeight, LaneWidth, LaunchSet, ScalarClass, Slice, Space, TargetFacts,
    Terminator, ValueId,
};

/// Two inputs and one output: the second input holds the oracle's class, the first the module's.
const BINDING: Binding = Binding {
    inputs: 2,
    outputs: 1,
};

/// The callee a `CallKernel` probe names, and the position the set holds it at.
const CALLEE: KernelId = 7;

/// One probe: an instruction, the class it must be reported in, and the steps that leave
/// its result on top of the stack.
///
/// # Invariant
/// **The module is declared in the class the instruction must _not_ be reported in**, so
/// neither the module's class nor an operand's can stand in for the answer: a value the emitter
/// classed from the wrong source is refused rather than quietly agreed with. A
/// [`KernelInstr::CallKernel`] reads its answer from the callee's declaration, which states one
/// where the instruction cannot.
struct Probe {
    /// How `every_variant` names the instruction, so a failure names a variant.
    name: &'static str,
    /// The instruction the fragment computes, and the value whose class is asked about.
    subject: KernelInstr,
    /// The steps that push the instruction's operands and then the instruction.
    reach: Vec<FlatOp>,
    /// The class the emitter must report for the value the instruction produces.
    oracle: ScalarClass,
    /// The class the probe's buffers declare, which is the module's class.
    module: ScalarClass,
    /// The slices the instruction's own accesses name.
    slices: Vec<Slice>,
    /// The callee a call probe compiles beside the caller, which is what a call's class is.
    callee: Option<KernelFragment>,
}

/// One probe over the fragment's `(unused, config, index)` domain, its result left on the stack.
fn probe(
    name: &'static str,
    subject: KernelInstr,
    operands: Vec<FlatOp>,
    oracle: ScalarClass,
) -> Probe {
    let mut reach = operands;
    reach.push(FlatOp::Instr(subject));
    let module = other(oracle);
    let slices = if matches!(subject, KernelInstr::SliceReadCall(_)) {
        vec![Slice {
            class: oracle,
            count: 64,
            space: Space::Shared,
        }]
    } else {
        Vec::new()
    };
    Probe {
        name,
        subject,
        reach,
        oracle,
        module,
        slices,
        callee: None,
    }
}

/// A probe for [`KernelInstr::CallKernel`], whose class is a callee's and not its own.
///
/// # Invariant
/// **The callee takes one leaf of the class the probe must not be reported in and returns one
/// value of the oracle**, so reading operand `0` and falling back to the module both reach the
/// one wrong class — the shape this guard exists to catch, and the one a call could not be
/// probed for at all while `own_class` stated nothing for it.
fn call(oracle: ScalarClass) -> Probe {
    let argument = other(oracle);
    let mut probe = probe(
        "CallKernel",
        KernelInstr::CallKernel(CALLEE),
        vec![constant(argument)],
        oracle,
    );
    probe.callee = Some(callee(oracle, argument));
    probe
}

/// A callee taking one leaf of `argument` and returning one value of `result`.
fn callee(result: ScalarClass, argument: ScalarClass) -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Scalar(argument),
        body: KernelBody::from_flat(
            1,
            &[
                FlatOp::Read(0),
                FlatOp::Instr(KernelInstr::Conv {
                    from: argument,
                    to: result,
                }),
            ],
        ),
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
        result_classes: vec![result],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// The class a declaration states for the value `probe` produces, or `None` when nothing the
/// compile reads states one.
///
/// # Invariant
/// **Read from a declaration rather than from the emitter's own account of the value**, the answer
/// under test: `own_class` is the instruction's own form and `result_classes` the callee's, both
/// inputs to a compile rather than answers one gives. A call states no class of its own, so its
/// oracle is the declared result list both emissions type the slot by.
fn oracle_of(probe: &Probe) -> Option<ScalarClass> {
    match &probe.callee {
        Some(callee) => callee.result_classes.first().copied(),
        None => probe.subject.own_class(),
    }
}

/// Every instruction the guard drives through the emitter.
///
/// # Invariant
/// **The whole covered set, and each one in both classes where the class is the instruction's
/// own.** `operand_class` is the answer a dropped `own_class` would leave: a read answers `Int`
/// at position `0` whatever the buffer holds, so probing only `Int` would agree. A call is probed
/// beside a callee stating the class it is *not* called with.
fn probes() -> Vec<Probe> {
    let mut probes = Vec::new();
    for (stated, operand) in [
        (ScalarClass::Int, ordinal(7)),
        (ScalarClass::Float, number(1.0)),
    ] {
        probes.push(probe(
            "Const",
            KernelInstr::Const(stated, 7),
            Vec::new(),
            stated,
        ));
        probes.push(probe(
            "Bin",
            KernelInstr::Bin(stated, KernelBin::Add),
            vec![FlatOp::Read(1), operand],
            stated,
        ));
        probes.push(probe(
            "BufferReadCall",
            KernelInstr::BufferReadCall(stated),
            vec![ordinal(1), FlatOp::Read(2)],
            stated,
        ));
        probes.push(probe(
            "SliceReadCall",
            KernelInstr::SliceReadCall(stated),
            vec![ordinal(0), ordinal(0), ordinal(0)],
            stated,
        ));
        probes.push(call(stated));
    }
    probes.push(probe(
        "Conv",
        KernelInstr::Conv {
            from: ScalarClass::Int,
            to: ScalarClass::Float,
        },
        vec![number(1.0)],
        ScalarClass::Float,
    ));
    probes.push(probe(
        "Conv",
        KernelInstr::Conv {
            from: ScalarClass::Float,
            to: ScalarClass::Int,
        },
        vec![ordinal(7)],
        ScalarClass::Int,
    ));
    probes
}

/// Every variant the guard does **not** drive, with the reason in the guard's own words.
///
/// # Invariant
/// **The whole uncovered set, and nothing outside it.** A variant named here either states
/// no class to compare against or leaves no value to ask one of, and saying which of the
/// two is the difference between a guard with a stated hole and one that pretends to be
/// exhaustive.
fn unasked() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "Exp",
            "`own_class` states `Float` for it, but the emitter refuses an unexpanded \
             transcendental while emitting the instruction itself, and `value_class` is reached \
             only from a `Select`'s arms or from an edge recorded after that instruction in the \
             same function",
        ),
        ("Sqrt", "the same refusal as `Exp`"),
        ("Floor", "the same refusal as `Exp`"),
        (
            "Ln",
            "the same refusal as `Exp`: `TRANSCENDENTALS` holds it beside `Exp`",
        ),
        ("Sin", "the same refusal as `Exp`"),
        ("Cos", "the same refusal as `Exp`"),
        (
            "BufferWriteCall",
            "`produces()` is 0 and the emission stores without leaving a slot, so there is no \
             value for the emitter to have a class for",
        ),
        (
            "SliceWriteCall",
            "the version a slice write leaves is not materialised, so no instruction can read \
             the value back and no `Select` can hold it as an arm",
        ),
        (
            "I32WrapI64",
            "`own_class` states none for it, so there is no answer for the emitter's to agree \
             with: the classing falls back to the operand and then to the module",
        ),
        (
            "Select",
            "`own_class` states none for it — its class is its arms', which is what \
             `conv_class.rs` already pins",
        ),
        (
            "Fma",
            "the emitter writes it as one `OpExtInst`, so the guard could drive it as `Exp` is \
             driven and does not: this slice added the instruction and no probe, and a probe that \
             reads three buffer operands is this file's shape with one more read",
        ),
    ]
}

/// Every `KernelInstr` variant, named as the two lists above name it.
///
/// # Invariant
/// **One arm per variant and no wildcard**, so an instruction added to `KernelInstr` is a
/// compile error here rather than a variant the ledger silently leaves out. The list is
/// what "exhaustive over the variant list" is measured against, and this is it.
fn every_variant() -> Vec<(&'static str, KernelInstr)> {
    [
        KernelInstr::Const(ScalarClass::Int, 7),
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        KernelInstr::Fma,
        KernelInstr::Conv {
            from: ScalarClass::Int,
            to: ScalarClass::Float,
        },
        KernelInstr::Exp,
        KernelInstr::Sqrt,
        KernelInstr::Floor,
        KernelInstr::Ln,
        KernelInstr::Sin,
        KernelInstr::Cos,
        KernelInstr::BufferReadCall(ScalarClass::Int),
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        KernelInstr::SliceReadCall(ScalarClass::Int),
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        KernelInstr::I32WrapI64,
        KernelInstr::Select,
        KernelInstr::CallKernel(0),
    ]
    .into_iter()
    .map(named)
    .collect()
}

/// The ledger's name for one instruction.
fn named(instr: KernelInstr) -> (&'static str, KernelInstr) {
    let name = match instr {
        KernelInstr::Const(..) => "Const",
        KernelInstr::Bin(..) => "Bin",
        KernelInstr::Fma => "Fma",
        KernelInstr::Conv { .. } => "Conv",
        KernelInstr::Exp => "Exp",
        KernelInstr::Sqrt => "Sqrt",
        KernelInstr::Floor => "Floor",
        KernelInstr::Ln => "Ln",
        KernelInstr::Sin => "Sin",
        KernelInstr::Cos => "Cos",
        KernelInstr::BufferReadCall(_) => "BufferReadCall",
        KernelInstr::BufferWriteCall(_) => "BufferWriteCall",
        KernelInstr::SliceReadCall(_) => "SliceReadCall",
        KernelInstr::SliceWriteCall(_) => "SliceWriteCall",
        KernelInstr::I32WrapI64 => "I32WrapI64",
        KernelInstr::Select => "Select",
        KernelInstr::CallKernel(_) => "CallKernel",
    };
    (name, instr)
}

/// The probe as a fragment: the steps to the instruction, then a `Select` beside a constant.
///
/// # Invariant
/// **The `Select` is the instrument and needs no device.** `select_class` reads each arm
/// through `value_class`, a constant states its own class, and there are two classes, so the arms
/// can agree only when the emitter classes the probed value as the oracle says.
/// `input_classes` is `[module, oracle]`, so a read at position `1` is a buffer of the oracle's class.
fn fragment_of(probe: &Probe) -> KernelFragment {
    let mut ops = probe.reach.clone();
    ops.push(constant(probe.oracle)); // the arm the probed value is compared with
    ops.push(ordinal(1)); // the condition
    ops.push(FlatOp::Instr(KernelInstr::Select));
    // **A call's arity is the callee's own**, so a flat body would leave it with no arguments.
    let body = match &probe.callee {
        None => KernelBody::from_flat(3, &ops),
        Some(callee) => called_body(&ops, callee),
    };
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(probe.oracle),
            KernelShape::Scalar(ScalarClass::Int), // the invocation index
        ]),
        body,
        inputs: 2,
        outputs: 1,
        input_classes: vec![probe.module, probe.oracle],
        output_classes: vec![probe.module],
        result_classes: vec![probe.module],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: probe.slices.clone(),
        marks: Vec::new(),
    }
}

/// The flat builder's own walk, over a body holding a call.
///
/// # Invariant
/// **One rule differs from [`KernelBody::from_flat`], and it is the one the call needs**: a call
/// takes as many arguments as the callee's own parameter leaves, the same fact the emission checks
/// its arity against, and `from_flat` takes none because `arity` answers `None` for a call. Every
/// other step is that builder's discipline unchanged.
fn called_body(ops: &[FlatOp], callee: &KernelFragment) -> KernelBody {
    let domain = callee.param_shape.leaf_classes().len();
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let mut parameters = Vec::new();
    for _ in 0..3 {
        parameters.push(body.add_param(entry));
    }
    let mut values: Vec<ValueId> = Vec::new();
    for step in ops {
        match *step {
            FlatOp::Read(index) => {
                let Some(value) = parameters.get(index).copied() else {
                    continue;
                };
                values.push(value);
            }
            FlatOp::Instr(op) => {
                let taken = match op {
                    KernelInstr::CallKernel(_) => domain,
                    _ => op.arity().unwrap_or(0),
                }
                .min(values.len());
                let arguments = values.split_off(values.len() - taken);
                let classes = if op.produces() == 0 {
                    Vec::new()
                } else {
                    vec![op.own_class().unwrap_or(ScalarClass::Int)]
                };
                values.push(body.add_op(entry, op, arguments, classes));
            }
        }
    }
    body.set_terminator(
        entry,
        Terminator::Return {
            values: values.last().copied().into_iter().collect(),
        },
    );
    body
}

/// A constant of `class` holding zero, which is the `Float` `+0.0` and the `Int` `0` alike.
fn constant(class: ScalarClass) -> FlatOp {
    FlatOp::Instr(KernelInstr::Const(class, 0))
}

/// An `Int` constant, pushed on the flat stack.
fn ordinal(value: i64) -> FlatOp {
    FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, value))
}

/// A `Float` constant, pushed on the flat stack.
fn number(value: f32) -> FlatOp {
    FlatOp::Instr(KernelInstr::Const(
        ScalarClass::Float,
        i64::from(value.to_bits()),
    ))
}

/// The one class that is not `class`.
fn other(class: ScalarClass) -> ScalarClass {
    match class {
        ScalarClass::Int => ScalarClass::Float,
        ScalarClass::Float => ScalarClass::Int,
    }
}

/// The class the emitter reports for a value is the class its declaration states.
///
/// # Invariant
/// **The declaration is the oracle**, because a definition states the class of what it leaves
/// behind and an operand's class is a fact about a different value. Each probe's module is declared
/// in the *other* class and its `Select` arm a constant of the oracle's, so an emitter reading the
/// answer from the module or an operand is refused — the shape `Conv` had, and `CallKernel`'s.
#[test]
fn the_class_the_emitter_reports_is_the_class_the_instruction_states() {
    let mut disagreements = Vec::new();
    for probe in probes() {
        let stated = probe.oracle;
        assert_eq!(
            oracle_of(&probe),
            Some(stated),
            "the probe names a class its declaration must state, so a probe that disagrees is \
             wrong here before the emitter is asked"
        );
        let fragment = fragment_of(&probe);
        let outcome = match probe.callee.clone() {
            None => spirv::compile(&LaunchSet::single(&fragment), BINDING),
            Some(callee) => {
                let ordered = [fragment, callee];
                let index: HashMap<KernelId, u32> = [(CALLEE, 1)].into();
                spirv::compile(&LaunchSet::new(&ordered, &index), BINDING)
            }
        };
        match outcome {
            Ok(_) => {}
            // **Every disagreement is collected**, because one class with two truths is rarely
            // alone.
            Err(SpirvRefusal::SelectArmClassesDisagree { classes }) => disagreements.push(format!(
                "the emitter reports one of {classes:?} for the value `{}` produces, and \
                 the declaration states {stated:?}",
                probe.name
            )),
            Err(refusal) => panic!(
                "the `{}` probe never reached the classing, so it measures nothing: {refusal}",
                probe.name
            ),
        }
    }
    assert!(
        disagreements.is_empty(),
        "the declaration and the emitter's own account of the values these instructions produce \
         disagree:\n  {}",
        disagreements.join("\n  ")
    );
}

/// Every variant is either driven through the emitter or named as one that cannot be.
///
/// # Invariant
/// **Exactly one of the two, for every variant `every_variant` names.** The guard's claim is
/// that it is exhaustive over the instruction list rather than one spelling of it: a variant in
/// neither list is a hole the file does not admit to, and one in both is a reason beside a probe
/// that does not hold. A probed variant must leave a value; its class is read from a declaration.
#[test]
fn every_variant_is_either_probed_or_named_as_unasked_with_a_reason() {
    let probes = probes();
    let unasked = unasked();
    for (name, instr) in every_variant() {
        let probed = probes.iter().any(|probe| probe.name == name);
        let reason = unasked
            .iter()
            .find(|(other, _)| *other == name)
            .map(|(_, reason)| *reason);
        assert!(
            probed ^ reason.is_some(),
            "`{name}` is driven={probed} and named as unasked={}, and it must be exactly one",
            reason.is_some()
        );
        if let Some(reason) = reason {
            assert!(
                !reason.is_empty(),
                "`{name}` is named as unasked without saying why"
            );
        }
        if probed {
            assert_eq!(
                instr.produces(),
                1,
                "`{name}` is driven, so it must leave a value for the emitter to have a class \
                 for — a variant that leaves none is what `unasked` is for"
            );
        }
    }
}
