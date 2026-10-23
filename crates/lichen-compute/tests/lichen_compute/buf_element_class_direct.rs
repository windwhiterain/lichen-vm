//! The expr's other member: the same kernel, rooted rather than recorded.
//! See `docs/notes/compile-order-class.md`.
//!
//! # Invariant
//!
//! **This is the variant member of the expr**, and it differs from
//! `buf_element_class_recorded.lichen` in its last line alone: the root is `k` rather
//! than a `compute.graph` step that dispatches `k`. The two programs declare the same
//! `In`, `OutE`, `Par` and the same `raw`, byte for byte, and compile the same body.

use super::common;

use lichen_kernel_ir::ScalarClass;

/// The direct program: the kernel is the root.
const PROGRAM: &str = include_str!("buf_element_class_direct.lichen");

/// The compiled fragment names the element class the declaration states.
///
/// # Invariant
///
/// The buffer's type carries its element type, its length and its payload, so the
/// element class is that type's first shape position — `Float` here on both sides.
/// Nothing about the declaration changed between the two programs, so a compiler that
/// reads the element from the declaration answers `Float` for both.
#[test]
fn the_compiled_fragment_names_the_declared_element_class() {
    let (module, _value, _root) = common::run(PROGRAM);
    let fragment = common::sole_fragment(&module);
    assert_eq!(
        (fragment.inputs, fragment.input_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "`Par` declares `.in In` and `In` is `struct{{.d (compute.Buf ((compute.BufParam _)(.type Float, .length 1)))}}`"
    );
    assert_eq!(
        (fragment.outputs, fragment.output_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "`OutE` is the codomain and it is `struct{{.e (compute.Buf ((compute.BufParam _)(.type Float, .length 1)))}}`"
    );
}
