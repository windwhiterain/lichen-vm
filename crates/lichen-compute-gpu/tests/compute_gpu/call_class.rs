//! A call's value is the callee's **result** class, never its first argument's.

use std::collections::HashMap;

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBody, KernelFragment, KernelId, KernelInstr, KernelRoles, KernelShape,
    LaneWidth, LaunchSet, ScalarClass, TargetFacts, Terminator,
};

/// One input and one output; neither row reads or writes, the `Select` being the whole body.
const BINDING: Binding = Binding {
    inputs: 1,
    outputs: 1,
};

/// The callee every row here calls.
const CALLEE: KernelId = 7;

/// A callee taking one leaf of `argument` and returning one value of `result`.
///
/// # Invariant
/// **The crossing is in the callee, not the caller**, so the value the call leaves is `result`
/// while the value it is handed is `argument`: an emitter reading either the wrong one of the
/// two, or the caller's module, is wrong in a way one class could not express.
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
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// An `Int` module that calls the callee and selects its result against a constant of `arm`.
///
/// # Invariant
/// **Hand-built, not `from_flat`**: a call's arity is the callee's own, and `from_flat` takes
/// what [`KernelInstr::arity`] answers, which is none for a call. The arms are the call's result
/// and the constant, so the classing is asked what the call left behind and the only other answer
/// available is the constant's own class.
fn caller(arm: ScalarClass) -> KernelFragment {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _config = body.add_param(entry);
    let _index = body.add_param(entry);
    let argument = body.add_const(entry, ScalarClass::Float, 0);
    let called = body.add_op(
        entry,
        KernelInstr::CallKernel(CALLEE),
        vec![argument],
        vec![ScalarClass::Int],
    );
    let other = body.add_const(entry, arm, 0);
    let condition = body.add_const(entry, ScalarClass::Int, 1);
    let selected = body.add_op(
        entry,
        KernelInstr::Select,
        vec![called, other, condition],
        vec![arm],
    );
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![selected],
        },
    );
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Float),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// The two rows, compiled over one launch set whose callee takes a `Float` and returns an `Int`.
fn refusal_over(arm: ScalarClass) -> Result<Vec<u32>, SpirvRefusal> {
    let caller = caller(arm);
    let callee = callee(ScalarClass::Int, ScalarClass::Float);
    let ordered = [caller, callee];
    let index: HashMap<KernelId, u32> = [(CALLEE, 1)].into();
    spirv::compile(&LaunchSet::new(&ordered, &index), BINDING)
}

/// A `Select` whose arms are a call's `Int` result and an `Int` constant is emitted.
///
/// # Invariant
/// **The row that was refused, verbatim.** Before the classing read the callee's result class this
/// fragment answered `Err(SelectArmClassesDisagree { classes: [Int, Float] })`: the call's result
/// was classed as `Float`, the callee's *argument*, so two honest `Int` arms were called different
/// classes. `select_class` still refuses arms that really differ — the second row is that.
#[test]
fn a_select_over_a_calls_result_and_a_constant_of_that_class_is_emitted() {
    refusal_over(ScalarClass::Int).expect(
        "a call's result is the class its callee declares, so an `Int` arm beside it is one class \
         and the select is emitted",
    );
}

/// A `Select` whose arms are a call's `Int` result and a `Float` constant is refused by name.
///
/// # Invariant
/// **The other row, verbatim, and `MixedClasses` must not be what refuses it.** Before the fix
/// this fragment answered `Err(MixedClasses { at: 3 })`: the arms *agreed* — both read as the
/// callee's `Float` argument — and the emitter then refused the call's `Int` slot where the agreed
/// `Float` wanted it. Now they are the disagreement they always were.
#[test]
fn a_select_over_a_calls_result_and_a_constant_of_the_other_class_is_refused_by_name() {
    assert_eq!(
        refusal_over(ScalarClass::Float).expect_err(
            "two arms that really are different classes are refused, as `conv_class.rs` pins"
        ),
        SpirvRefusal::SelectArmClassesDisagree {
            classes: vec![ScalarClass::Int, ScalarClass::Float],
        },
        "the refusal names the two classes, and is the same refusal `conv_class.rs` already pins: \
         the fix changed what the call's result is classed as, not what the check accepts"
    );
}
