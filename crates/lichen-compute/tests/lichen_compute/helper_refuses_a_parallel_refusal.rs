//! A refusal recorded by `compute.parallel` fails a helper-built suite.

use lichen_compute::language::{LangProgram, new_store};

/// The misplaced write of [`swallowed_refusal`](swallowed_refusal.rs), run as a suite would.
///
/// # Invariant
///
/// The body is byte-identical to the one two tests already pin against the CLI, so what this
/// adds is the second reader: the helper `common::run` calls, and not a gate of its own.
const MISPLACED_WRITE: &str = r#"
--- compute = import "compute.lichen" ---
In  = struct{.a Int}
Out = struct{.c (compute.Buf _), .s (compute.Buf _)}
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.s, .at i, .value i + 100)), compute.write ((compute.Write _)(.to k.out.c, .at i, .value i)))
}
k = compute.parallel f "cpu" Out
outs = (compute.plrun k ((compute.A In)(.n 3, .I In(.a 0))) : Out)
compute.collect (compute.collect outs.c, compute.collect outs.s)
"#;

/// The suite's own helper refuses the program, by the CLI's own predicate.
///
/// # Invariant
///
/// The gate is read, not re-implemented, so a suite cannot accept a refusal the CLI rejects.
/// Which of the helper's two gates fires first is not what this pins: `has_extension_failures()`
/// is inside the checker's `ok`, so a `compute.parallel` body compiles while the program is
/// checked, and the suite fails either way.
#[test]
fn a_compute_parallel_refusal_fails_the_helper() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lichen_language::testing::run_at::<LangProgram>(MISPLACED_WRITE, None, &mut store)
    }));
    let message = panic
        .err()
        .and_then(|payload| payload.downcast_ref::<String>().cloned())
        .expect("the helper refuses a program whose parallel kernel wrote the wrong output");
    assert!(
        message.contains("compute.parallel") && message.contains("to output 1"),
        "the refusal is named by the helper's own gate: {message}"
    );
}
