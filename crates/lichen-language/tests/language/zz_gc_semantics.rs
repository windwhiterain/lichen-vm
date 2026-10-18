//! A probe for the `@gc` trigger's semantics: one row per program, no assertions.

use lichen_language::program::LangProgram;
use lichen_lowlevel::Module;

/// The checker's failure counts, the run's answer, and the block census after the run.
fn row(label: &str, source: &str) {
    let report = lichen_language::compile(source);
    let ok = report.ok();
    let mut counts = (0, 0, 0, 0);
    let mut census = String::from("no build");
    if let Some(build) = report.build {
        let mut module: Module<LangProgram> = build.module;
        counts = (
            module.assert_errors.len(),
            module.eval_errors.len(),
            module.unify_errors.len(),
            module.apply_errors.len(),
        );
        let _ = module.evaluate_node_deep(build.root_val, None);
        let _ = module.evaluate_node_deep(build.root_ty, None);
        let declared = module
            .blocks
            .iter()
            .filter(|(_, block)| block.scope_root.is_some())
            .count();
        census = format!(
            "blocks={} nodes={} scopes={declared}",
            module.blocks.len(),
            module.nodes.len()
        );
    }
    let run = lichen_language::run::evaluate(source)
        .map(|output| output)
        .unwrap_or_else(|diagnostics| format!("refused: {diagnostics:?}",));
    println!("{label:<44} ok={ok:<5} a/e/u/p={counts:?} {census} run={run:?}");
}

#[test]
fn gc_trigger_semantics() {
    println!(
        "mode={:?}",
        std::env::var("LICHEN_GC_TRIGGER").unwrap_or_default()
    );
    row(
        "false assert in a scope",
        "x = @gc { @assert (1 > 2)\n3 }\nx",
    );
    row(
        "true assert in a scope",
        "x = @gc { @assert (2 > 1)\n3 }\nx",
    );
    row(
        "false assert after a scope",
        "x = @gc { a = 2; a }\n@assert (x > 5)\nx",
    );
    row(
        "template scope decided at compile",
        "f = n => @gc { a = 1; a }\nf 0 + f 1",
    );
    row(
        "scope call reused later",
        "f = n => n + 1\nx = @gc f 41\nx + 1 + x",
    );
    row(
        "nested scopes",
        "x = @gc { a = @gc { b = 2; b }\na + 1 }\nx",
    );
    row(
        "scope value escapes into an array",
        "y = [@gc { a = 2; a }, 3]\ny[0] + y[1]",
    );
    row(
        "scope with a call inside",
        "x = @gc { f = n => n * 2\n@gc f 3 + 1 }\nx",
    );
    row(
        "unused call in a scope",
        "f = n => @assert (n > 0)\nx = @gc { y = f 0\n7 }\nx",
    );
    row(
        "unforced element in a scope",
        "f = n => { @assert (n > 0)\nn }\nx = @gc [f 0, 2]\nx",
    );
    row(
        "forced element in a scope",
        "f = n => { @assert (n > 0)\nn }\nx = (@gc [f 0, 2])[0]\nx",
    );
    row(
        "unused array statement in a scope",
        "x = @gc { a = [1, 2, 3]\n2 }\nx",
    );
}
