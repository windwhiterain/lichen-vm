use lichen_language::compile;
use lichen_lowlevel::AnyNodeId;
use lichen_utils::disjoint;

#[test]
fn annotated_rec_scratch() {
    let report = compile("f = n : Int => if n <= 0 then 0 else f (n - 1); f 5 : Int");
    let build = report.build.unwrap();
    for (i, e) in build.diary.iter().enumerate() {
        println!(
            "diary {i}: error_index={} kind={:?} a={:?} b={:?}",
            e.error_index, e.kind, e.a, e.b
        );
    }
    for (i, err) in build.module.unify_errors.iter().enumerate() {
        println!(
            "--- error {i} a={:?} b={:?} va={:?} vb={:?}",
            err.a, err.b, err.value_a, err.value_b
        );
        for rep in [err.a, err.b] {
            println!("  class {rep:?}:");
            for m in disjoint::members(&build.module.nodes, rep) {
                println!(
                    "    {m:?} value={:?} op={:?}",
                    build.module.node_value(AnyNodeId::Dynamic(m)),
                    build.module.nodes[m].operation
                );
            }
        }
    }
}
