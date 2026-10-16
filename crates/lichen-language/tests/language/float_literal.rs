//! `1.5` builds `[Float(1.5), [float, Type]]`; `1.0` must never build an `Int`
//! pair. See floating-point.md §3.4.

use lichen_language::compile;
use lichen_language::program::LangProgram as P;
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// The structural value the node holds.
fn value_of(module: &Module<P>, node: NodeId) -> Option<LowValue> {
    let value = module.node_value(AnyNodeId::Dynamic(node))?;
    value.as_enum()
}

#[test]
fn a_float_literal_builds_a_float_pair_and_never_an_int_shaped_one() {
    // `1.5` — the value slot is `Float(1.5)`, the type slot is the shared
    // `[float, Type]` type expression.
    let report = compile("1.5");
    assert!(report.ok(), "1.5 must check: {:?}", report.diagnostics);
    let build = report.build.expect("a build");
    assert_eq!(
        value_of(&build.module, build.root_val),
        Some(LowValue::Float(1.5))
    );
    assert_eq!(
        build.root_ty, build.float_type,
        "the type slot is the shared `[float, Type]` expression"
    );

    // `1.0` must not build an `Int`-shaped pair.
    let report = compile("1.0");
    assert!(report.ok(), "1.0 must check: {:?}", report.diagnostics);
    let build = report.build.expect("a build");
    assert_eq!(
        value_of(&build.module, build.root_val),
        Some(LowValue::Float(1.0))
    );
    assert_ne!(
        value_of(&build.module, build.root_val),
        Some(LowValue::USize(1)),
        "1.0 read back as 1 would be a different LowValue, and so a different type"
    );
    assert_eq!(build.root_ty, build.float_type);
    assert_ne!(
        build.root_ty, build.int_type,
        "the type slot of a float literal is never the `[int, Type]` expression"
    );
}
