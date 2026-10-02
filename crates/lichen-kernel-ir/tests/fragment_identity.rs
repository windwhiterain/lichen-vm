//! The digest's stated invariant, and the one thing in the IR that both a
//! backend and the wasm emitter depend on getting right.
//!
//! These are kept as integration tests rather than unit tests in the crate so
//! that they exercise the crate exactly as a backend would: through its public
//! surface only.

use lichen_kernel_ir::{
    IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape, ScalarClass,
};

fn fragment() -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Scalar(ScalarClass::Int),
        body: vec![KernelInstr::Const(1), KernelInstr::LocalGet(0)].into(),
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
        results: 1,
        int_width: IntWidth::I64,
    }
}

#[test]
fn the_digest_separates_fragments_that_differ_in_any_field_it_hashes() {
    let base = fragment();
    let base_digest = lichen_kernel_ir::fragment_digest(&base);

    // One mutation per field the digest hashes.  `int_width` is absent from
    // this list and that is a real gap, not an oversight: `IntWidth` has one
    // variant, so two fragments differing *only* in the declared width cannot
    // be constructed, and this test cannot reach that cell.  The invariant it
    // guards is the reason the field is hashed at all — see the `IntWidth` docs
    // for why the width has to be declared rather than assumed.
    //
    // A leaf's class needs no cell of its own here: it rides on `param_shape`,
    // which is hashed whole, and the fragment-wide `IntWidth` gap above is the
    // one this list cannot reach.  The two buffer-class lists are their own
    // fields, so each gets one.
    let mut variants: Vec<(&str, KernelFragment)> = vec![
        (
            "param_shape",
            KernelFragment {
                param_shape: KernelShape::Tuple(vec![
                    KernelShape::Scalar(ScalarClass::Int),
                    KernelShape::Scalar(ScalarClass::Int),
                ]),
                ..base.clone()
            },
        ),
        (
            "body",
            KernelFragment {
                body: vec![KernelInstr::Const(2), KernelInstr::LocalGet(0)].into(),
                ..base.clone()
            },
        ),
        (
            "inputs",
            KernelFragment {
                inputs: 1,
                ..base.clone()
            },
        ),
        (
            "outputs",
            KernelFragment {
                outputs: 1,
                ..base.clone()
            },
        ),
        (
            "input_classes",
            KernelFragment {
                input_classes: vec![ScalarClass::Int],
                ..base.clone()
            },
        ),
        (
            "output_classes",
            KernelFragment {
                output_classes: vec![ScalarClass::Float],
                ..base.clone()
            },
        ),
        (
            "results",
            KernelFragment {
                results: 2,
                ..base.clone()
            },
        ),
    ];
    variants.push((
        "body instruction kind",
        KernelFragment {
            body: vec![KernelInstr::Const(1), KernelInstr::Bin(KernelBin::Add)].into(),
            ..base.clone()
        },
    ));

    for (field, variant) in variants {
        assert_ne!(
            lichen_kernel_ir::fragment_digest(&variant),
            base_digest,
            "a fragment differing in {field} must not intern to the same id, or the module \
             cache serves one kernel's compiled form for another's"
        );
    }
}

#[test]
fn the_digest_is_a_function_of_the_fragment_alone() {
    // Content addressing is what lets the launcher recompile a kernel on every
    // keystroke and still hit the module cache, so the same fragment has to
    // digest the same every time and on every build.
    assert_eq!(
        lichen_kernel_ir::fragment_digest(&fragment()),
        lichen_kernel_ir::fragment_digest(&fragment())
    );
}

#[test]
fn a_domain_flattens_to_exactly_its_leaf_count() {
    // Load-bearing for every backend: the parameter list is built from this
    // count, so a backend that flattened differently would call one kernel with
    // another's arguments.  A leaf's class does not move the count: a float
    // local and an integer local are one local each.
    assert_eq!(KernelShape::Scalar(ScalarClass::Int).flat_arity(), 1);
    assert_eq!(KernelShape::Scalar(ScalarClass::Float).flat_arity(), 1);
    assert_eq!(KernelShape::Tuple(vec![]).flat_arity(), 0);
    assert_eq!(
        KernelShape::Tuple(vec![KernelShape::Scalar(ScalarClass::Int); 3]).flat_arity(),
        3
    );
    // A nested domain, which a `jit` of a `((Int, Int), Int)` produces.
    assert_eq!(
        KernelShape::Tuple(vec![
            KernelShape::Tuple(vec![
                KernelShape::Scalar(ScalarClass::Int),
                KernelShape::Scalar(ScalarClass::Int),
            ]),
            KernelShape::Scalar(ScalarClass::Int),
        ])
        .flat_arity(),
        3
    );
}
