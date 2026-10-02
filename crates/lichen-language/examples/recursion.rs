//! Could recursion expansion stand in for a loop as the unroll's surface?
//!
//! Three questions, in the order they have to be answered:
//!
//! 1. **Is recursion in a kernel expressible today at all?** Inline applies are
//!    refused by name and a cross-kernel call needs a `KernelId` that must exist
//!    before the body that names it is compiled — so a cycle may not be writable.
//! 2. **What does a cross-kernel call cost?** If `CallKernel` is a runtime call in
//!    the assembled module rather than an inline, then expansion is not only about
//!    expressibility: a hand-written chain is the *non-expanded* version of what
//!    B1 would do automatically, so timing it measures what expansion saves.
//! 3. **Does the GPU refuse it?** `SpirvRefusal::CrossKernelCall` says so on
//!    paper; this confirms it from the program side.

use std::time::Instant;

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;

fn run(source: &str) -> Result<String, Vec<String>> {
    let mut store = PackageStore::<LangProgram>::new();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .map_err(|diags| diags.into_iter().map(|d| d.message).collect())
}

fn timed(source: &str, repeats: usize) -> Result<f64, Vec<String>> {
    run(source)?;
    let mut best = f64::INFINITY;
    for _ in 0..repeats {
        let start = Instant::now();
        let outcome = run(source);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        outcome?;
        if elapsed < best {
            best = elapsed;
        }
    }
    Ok(best)
}

const HEADER: &str = "@{ compute = import \"compute.lichen\" @}\n";

/// A **scalar** kernel whose body calls another kernel. The chain's `jit`
/// compiles; this checks the call actually runs.
const CALL_IN_SCALAR_BODY: &str = r#"
@{ compute = import "compute.lichen" @}
k0 = compute.jit (v : Int => v + 1)
k1 = compute.jit (v : Int => compute.call k0 v + 1)
compute.launch k1 3
"#;

/// The same call, from inside a **parallel** body.
const CALL_IN_PARALLEL_BODY: &str = r#"
@{ compute = import "compute.lichen" @}
k0 = compute.jit (v : Int => v + 1)
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, compute.call k0 i]
}) "BACKEND"
compute.read [compute.plrun p (8,), 3]
"#;

/// A **module-level** helper called from inside a parallel body — the
/// composition case, and the one a library would actually be written in.
const MODULE_HELPER: &str = r#"
@{ compute = import "compute.lichen" @}
square = x => x * x
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read [cfg(1)(0), i]
  compute.write [n, i, square v]
}) "BACKEND"
seed = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 1]
}) "BACKEND"
s = compute.plrun seed (8,)
compute.collect (compute.plrun p (8, (s,)))
"#;

/// A single-parameter recursive helper whose trip count is a **literal** at the
/// call site. Expansion only terminates if the conditional's selector folds to a
/// constant, so this is the program that says whether it does.
const RECURSIVE_LITERAL: &str = r#"
@{ compute = import "compute.lichen" @}
steps = k => if k == 0 then 0 else steps (k - 1) + 1
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, steps 4 + compute.range n * 0]
}) "BACKEND"
compute.read [compute.plrun p (8,), 3]
"#;

/// A **body-local alias** and no call at all. This is the control for the two
/// probes above: if it fails, the alias — not the inlining — is what cannot be
/// resolved in an unapplied template.
const BODY_LOCAL_ALIAS: &str = r#"
@{ compute = import "compute.lichen" @}
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read [cfg(1)(0), i]
  compute.write [n, i, v]
}) "BACKEND"
seed = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 1]
}) "BACKEND"
s = compute.plrun seed (8,)
compute.collect (compute.plrun p (8, (s,)))
"#;

/// The helper called with a **literal** argument, so nothing but the callee is
/// unresolved. This separates "the callee cannot be found" from "the argument
/// cannot be re-emitted".
const HELPER_LITERAL_ARG: &str = r#"
@{ compute = import "compute.lichen" @}
square = x => x * x
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, square 3]
}) "BACKEND"
compute.collect (compute.plrun p (4,))
"#;

/// The same helper, called with an argument the **host cannot reduce** —
/// `i + 1` depends on the loop index, so the call survives to the kernel
/// compiler. This is the case static expansion exists for.
const HELPER_INDEX_ARG: &str = r#"
@{ compute = import "compute.lichen" @}
square = x => x * x
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, square (i + 1)]
}) "BACKEND"
compute.collect (compute.plrun p (4,))
"#;

/// A `jit` chain `k0 → k1 → … → k{N-1}`, each adding one, plus a parallel kernel
/// that calls the last. **The hand-written, un-expanded form of a loop.**
fn chain(depth: usize) -> String {
    let mut source = String::from(HEADER);
    source.push_str("k0 = compute.jit (v : Int => v + 1)\n");
    for level in 1..depth {
        source.push_str(&format!(
            "k{level} = compute.jit (v : Int => compute.call k{prev} v + 1)\n",
            prev = level - 1
        ));
    }
    source.push_str(&format!(
        "p = compute.parallel (cfg => {{
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, compute.call k{last} i]
}}) \"BACKEND\"\ncompute.read [compute.plrun p (COUNT,), 3]\n",
        last = depth - 1
    ));
    source
}

/// The same arithmetic as one body with `depth` adds — the expanded form.
fn unrolled(depth: usize) -> String {
    let mut body = String::from("i");
    for _ in 1..depth {
        body.push_str(" + 1");
    }
    format!(
        "{HEADER}p = compute.parallel (cfg => {{
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, {body}]
}}) \"BACKEND\"
compute.read [compute.plrun p (COUNT,), 3]\n"
    )
}

/// A self-recursive function, called from inside a kernel body.
const RECURSIVE_INLINE: &str = r#"
@{ compute = import "compute.lichen" @}
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  count_up = s => k => if k == 0 then s else count_up (s + 1) (k - 1)
  compute.write [n, i, count_up 0 i]
}) "BACKEND"
compute.read [compute.plrun p (8,), 3]
"#;

/// A kernel whose own body names the kernel it is being compiled into.
const RECURSIVE_CROSS: &str = r#"
@{ compute = import "compute.lichen" @}
k = compute.jit (v : Int => compute.call k v + 1)
compute.launch k 3
"#;

/// Two kernels naming each other — a cycle through the kernel registry.
const MUTUAL: &str = r#"
@{ compute = import "compute.lichen" @}
a = compute.jit (v : Int => compute.call b v + 1)
b = compute.jit (v : Int => compute.call a v + 1)
compute.launch a 3
"#;

fn probe(name: &str, source: &str) {
    match run(source) {
        Ok(out) => println!("  {name:<34} OK      {out}"),
        Err(diags) => println!("  {name:<34} REFUSED {}", diags.join(" | ")),
    }
}

fn main() {
    let _ = lichen_compute_gpu::install_default();
    let count = 262_144;
    let repeats = 10;

    for backend in ["cpu", "gpu"] {
        println!("\n=== {backend} ===");
        let show = |value: Result<f64, Vec<String>>| match value {
            Ok(ms) => format!("{ms:>8.2} ms"),
            Err(diags) => format!("n/a: {}", diags.join(" | ")),
        };
        let with = |template: String| {
            template
                .replace("BACKEND", backend)
                .replace("COUNT", &count.to_string())
        };

        println!("  -- expressibility --");
        probe(
            "helper, index-dependent argument",
            &HELPER_INDEX_ARG.replace("BACKEND", backend),
        );
        probe(
            "helper, literal argument",
            &HELPER_LITERAL_ARG.replace("BACKEND", backend),
        );
        probe(
            "body-local alias, no call",
            &BODY_LOCAL_ALIAS.replace("BACKEND", backend),
        );
        probe(
            "module-level helper in a body",
            &MODULE_HELPER.replace("BACKEND", backend),
        );
        probe(
            "recursive helper, literal count",
            &RECURSIVE_LITERAL.replace("BACKEND", backend),
        );
        probe(
            "body-local helper in a body",
            &RECURSIVE_INLINE.replace("BACKEND", backend),
        );
        probe(
            "call in a scalar body",
            &CALL_IN_SCALAR_BODY.replace("BACKEND", backend),
        );
        probe(
            "call in a parallel body",
            &CALL_IN_PARALLEL_BODY.replace("BACKEND", backend),
        );
        probe(
            "inline recursion in a body",
            &RECURSIVE_INLINE.replace("BACKEND", backend),
        );
        probe(
            "self-referential kernel",
            &RECURSIVE_CROSS.replace("BACKEND", backend),
        );
        probe(
            "mutually referential kernels",
            &MUTUAL.replace("BACKEND", backend),
        );

        println!("  -- an 8-deep `jit` chain vs 8 unrolled adds (count {count}) --");
        println!(
            "  {:>4}  unrolled  {}",
            "1",
            show(timed(&with(unrolled(1)), repeats))
        );
        println!(
            "  {:>4}  unrolled  {}",
            "8",
            show(timed(&with(unrolled(8)), repeats))
        );
        for depth in [2usize, 4, 8] {
            println!(
                "  {depth:>4}  chained   {}",
                show(timed(&with(chain(depth)), repeats))
            );
        }
    }
    lichen_compute_gpu::uninstall();
}
