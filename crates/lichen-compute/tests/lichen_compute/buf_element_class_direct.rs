//! The pair's other member: the same kernel, rooted rather than recorded.
//! See `docs/notes/compile-order-class.md`.
//!
//! # Invariant
//!
//! **This is the variant member of the pair**, and it differs from
//! `buf_element_class_recorded.lichen` in its last line alone: the root is `k` rather
//! than a `compute.graph` step that dispatches `k`. The two programs declare the same
//! `In`, `OutE`, `Par` and the same `raw`, byte for byte, and compile the same body.
//! This one answers `Float`; the other answers `Int`.

use super::common;

use lichen_kernel_ir::ScalarClass;

/// The direct program: the kernel is the root.
const PROGRAM: &str = include_str!("buf_element_class_direct.lichen");

/// The compiled fragment names the element class the parameter declares.
///
/// # Invariant
///
/// **This passes on `dev`, and it is what makes the pair a pair.** Nothing about the
/// declaration changed — the parameter still names `compute.Buf Float` on both sides —
/// so a compiler that reads the element from the declaration answers `Float` here and
/// `Int` for the recorded program, and the difference is the order the module evaluated
/// in rather than anything the program said.
#[test]
fn the_compiled_fragment_names_the_declared_element_class() {
    let (module, _value, _root) = common::run(PROGRAM);
    let fragment = common::sole_fragment(&module);
    assert_eq!(
        (fragment.inputs, fragment.input_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "`Par` declares `.in In` and `In` is `struct{{.d (compute.Buf Float)}}`"
    );
    assert_eq!(
        (fragment.outputs, fragment.output_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "`Par` declares `.out OutE` and `OutE` is `struct{{.e (compute.Buf Float)}}`"
    );
}
