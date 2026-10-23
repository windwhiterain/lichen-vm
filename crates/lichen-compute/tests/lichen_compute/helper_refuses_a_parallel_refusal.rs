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
Data = struct{.d (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Out = struct{.e (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 3))
  ((Data)(.d (compute.write ((compute.Write _)(.to d, .at i, .value 1.0)))))
}
filler = compute.parallel fill "cpu" Data
data = (compute.plrun filler ((Fill)(.n 3)) : Data)
In  = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Par = struct{.n Int, .in In}
Arg = struct{.n Int, .I In}
f = (k : Par) => {
  i = compute.range k.n
  ((Out)(.e (compute.write ((compute.Write _)(.to k.in.k, .at i, .value (99.0 : Float))))))
}
k = compute.parallel f "cpu" Out
out = (compute.plrun k ((Arg)(.n 3, .I In(.k data.d))) : Out)
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
        message.contains("compute.parallel")
            && message.contains("a kernel body's index cannot be placed"),
        "the refusal is named by the helper's own gate: {message}"
    );
}
