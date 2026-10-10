//! A float is a `Copy` payload with no arena handle, so freezing must carry it.
//! See floating-point.md §3.1.

use lichen_language::program::LangProgram as P;
use lichen_language::session::BufferSession;
use lichen_lowlevel::{AnyNodeId, LowValue};
use lichen_utils::extend::AsEnum;

#[test]
fn a_cached_float_is_reused_and_reads_back_intact() {
    // The root is marked; the edit is in the unmarked binding before it, so dirty
    // propagation never reaches the cell.
    let source = "cache x = 1.5\nother = 1\nx\n";
    let mut session = BufferSession::<P>::with_source_id(source, "a.lichen");

    let first = session.compile();
    assert!(
        first.ok(),
        "the marked binding must check: {:?}",
        first.diagnostics
    );
    assert_eq!(
        first.cells.frozen, 1,
        "the first build freezes the one cell"
    );
    assert_eq!(first.cells.reused, 0);

    // A later build that skipped the marked binding's body.
    let at = source.find("other = 1").expect("the unmarked binding") + "other = ".len();
    session.replace(at..at + 1, "2");
    let second = session.compile();
    assert!(
        second.ok(),
        "the reused cell must check: {:?}",
        second.diagnostics
    );
    assert_eq!(
        second.cells.reused, 1,
        "the cell is read back, not recompiled"
    );
    assert_eq!(second.cells.frozen, 0, "no artifact is re-frozen");

    let build = second.build.as_ref().expect("a build");
    let value = build
        .module
        .node_value(AnyNodeId::Dynamic(build.root_val))
        .and_then(|value| value.as_enum());
    let Some(LowValue::Float(value)) = value else {
        panic!("the reused cell must read back as a float")
    };
    assert_eq!(value.to_bits(), 1.5f32.to_bits(), "the retained value");
}
