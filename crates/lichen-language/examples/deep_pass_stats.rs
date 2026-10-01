//! Step 0 of the incremental-evaluation study: how redundant is the deep pass?
//!
//! The deep pass memoizes no subtree across entry points (see
//! `docs/notes/incremental-evaluation.md`), so its work is
//! `walks × reachable-graph` rather than the graph's size.  This harness
//! compiles a few shapes and prints, per program, the two counters the lowlevel
//! keeps ([`lichen_lowlevel::deep_pass_stats`]) against the node table:
//!
//! - `walks` — `evaluate_node_deep` / `evaluate_node_forced` calls, i.e. the
//!   pass's entry points;
//! - `visits` — `evaluate_node_deep_inner` entries, i.e. the work, counted once
//!   per entry point that reaches a node;
//! - `nodes` — the module's node count;
//! - `stamped` — how many nodes carry a verdict (`evaluated_deep`), i.e. the
//!   distinct work one build actually kept;
//! - `cheap` — the visits that returned without evaluating a node (a static
//!   leaf, a cycle cut), so `real = visits - cheap` is the visits that did a
//!   node's work.
//!
//! `real / stamped` is the redundancy factor: how many node-evaluations the pass
//! spent per node it decided.  A build whose deep pass were memoized across
//! entry points would sit at 1.
//!
//! Run: `cargo run -p lichen-language --example deep_pass_stats`

fn measure(name: &str, source: &str) {
    lichen_lowlevel::reset_deep_pass_stats();
    let report = lichen_language::compile(source);
    let stats = lichen_lowlevel::deep_pass_stats();
    let ok = report.ok();
    let (nodes, stamped) = match report.build.as_ref() {
        Some(build) => (
            build.module.nodes.len(),
            build
                .module
                .nodes
                .keys()
                .filter(|&key| build.module.node_evaluated_deep(key).is_some())
                .count(),
        ),
        None => (0, 0),
    };
    let real = stats.visits - stats.cheap_returns;
    let redundancy = real as f64 / stamped.max(1) as f64;
    let revisit_share = stats.decided_revisits as f64 / stats.visits.max(1) as f64;
    println!(
        "{name:<22} walks {:>5}  visits {:>8}  cheap {:>7}  revisit {:>7} ({:>5.1}%)  nodes {:>7}  stamped {:>7}  real/stamped {:>6.2}  {}",
        stats.walks,
        stats.visits,
        stats.cheap_returns,
        stats.decided_revisits,
        revisit_share * 100.0,
        nodes,
        stamped,
        redundancy,
        if ok { "ok" } else { "diagnostics" }
    );
}

fn main() {
    // A wide recursion: the apply/clone machinery at work.
    measure(
        "recursion (fib 10)",
        "fib = x => [fib (x - 1) + fib (x - 2), x][x <= 1]\nfib 10\n",
    );
    // Nested closures, so one value node is reached from several entries.
    measure(
        "closure (nested)",
        "a = 1\nf1 = x => {\n    b = 2\n    f2 = y => [a, b, x, y]\n    f2\n}\nf1 3 4\n",
    );
    // The canonical cyclic structures (`Type : Type`, recursive nominal
    // structs) — the case the cycle cut's assumption exists for.
    measure(
        "struct_recursion",
        "A = struct<Int, B>\nB = struct<Type, A>\na = A(1, b)\nb = B(Int, a)\n(A, B, a, b)\n",
    );
    // Deep-content table keys and their forcing.
    measure(
        "table (deep keys)",
        "t = table{ [1, 2] ==> 10, [3, 4] ==> 20 }\n(t{[1, 2]}, t{[3, 4]})\n",
    );
    // A polymorphic template instantiated at several types.
    measure("let_polymorphism", "f = x => x\n(f, f 1, f Int)\n");
    // An explicit constraint, drained by the assert pass.
    measure("assert", "n = 5\n(! (n <= 5), n)\n");
}
