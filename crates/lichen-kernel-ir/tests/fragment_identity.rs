//! The digest's stated invariant, and the one thing in the IR that both a
//! backend and the wasm emitter depend on getting right.
//!
//! These are kept as integration tests rather than unit tests in the crate so
//! that they exercise the crate exactly as a backend would: through its public
//! surface only.

use lichen_kernel_ir::{
    IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    ScalarClass,
};

/// A one-parameter body: `p0 + first`, in SSA.
///
/// **Built the way a lowering builds one** — `add_param` for the domain leaf,
/// `add_op` in dependency order, `set_terminator` last — because the digest's
/// invariant is over the shape a consumer actually reads, and a fixture assembled
/// some other way would not exercise it.
fn body(first: i64, combine: bool) -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let parameter = body.add_param(entry);
    let literal = body.add_op(
        entry,
        KernelInstr::Const(ScalarClass::Int, first),
        Vec::new(),
        vec![ScalarClass::Int],
    );
    let result = if combine {
        body.add_op(
            entry,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![literal, parameter],
            vec![ScalarClass::Int],
        )
    } else {
        literal
    };
    body.set_terminator(
        entry,
        lichen_kernel_ir::Terminator::Return {
            values: vec![result],
        },
    );
    body
}

fn fragment() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Scalar(ScalarClass::Int),
        body: body(1, true),
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
        result_classes: vec![ScalarClass::Int],
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
                roles: KernelRoles::default(),
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
                roles: KernelRoles::default(),
                body: body(2, true),
                ..base.clone()
            },
        ),
        (
            "inputs",
            KernelFragment {
                roles: KernelRoles::default(),
                inputs: 1,
                ..base.clone()
            },
        ),
        (
            "outputs",
            KernelFragment {
                roles: KernelRoles::default(),
                outputs: 1,
                ..base.clone()
            },
        ),
        (
            "input_classes",
            KernelFragment {
                roles: KernelRoles::default(),
                input_classes: vec![ScalarClass::Int],
                ..base.clone()
            },
        ),
        (
            "output_classes",
            KernelFragment {
                roles: KernelRoles::default(),
                output_classes: vec![ScalarClass::Float],
                ..base.clone()
            },
        ),
        (
            "result_classes",
            KernelFragment {
                roles: KernelRoles::default(),
                result_classes: vec![ScalarClass::Int, ScalarClass::Int],
                ..base.clone()
            },
        ),
    ];
    variants.push((
        "body instruction kind",
        KernelFragment {
            roles: KernelRoles::default(),
            body: body(1, false),
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
