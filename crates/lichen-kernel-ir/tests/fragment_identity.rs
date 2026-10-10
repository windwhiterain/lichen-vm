//! The digest's stated invariant, exercised through the crate's public surface.

use lichen_kernel_ir::{
    IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    ScalarClass,
};

/// A one-parameter body: `p0 + first`, in SSA.
///
/// # Invariant
/// Built the way a lowering builds one — `add_param`, `add_op` in dependency order,
/// `set_terminator` last — because the digest is over the shape a consumer reads.
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

    // One mutation per field the digest hashes.

    // `int_width` is absent and that is a real gap: `IntWidth` has one variant, so the
    // case cannot be constructed.

    // A leaf's class rides on `param_shape`, which is hashed whole; each buffer-class
    // list is its own hashed field.
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
    // Content addressing: one fragment must digest the same every time and every build,
    // or the module cache never hits.
    assert_eq!(
        lichen_kernel_ir::fragment_digest(&fragment()),
        lichen_kernel_ir::fragment_digest(&fragment())
    );
}

#[test]
fn a_domain_flattens_to_exactly_its_leaf_count() {
    // The parameter list is built from this count: a different flattening would pass
    // another kernel's arguments.
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
