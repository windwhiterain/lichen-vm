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

const HEADER: &str = "--- compute = import \"compute.lichen\" ---\n";

/// A **scalar** kernel whose body calls another kernel. The chain's `jit`
/// compiles; this checks the call actually runs.
const CALL_IN_SCALAR_BODY: &str = r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (v : Int => v + 1)
k1 = compute.jit (v : Int => compute.call k0 v + 1)
compute.launch k1 3
"#;

/// The same call, from inside a **parallel** body.
const CALL_IN_PARALLEL_BODY: &str = r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (v : Int => v + 1)
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value compute.call k0 i))
}) "BACKEND"
compute.read ((compute.Read _)(.from compute.plrun p (8,), .at 3))
"#;

/// A **module-level** helper called from inside a parallel body — the
/// composition case, and the one a library would actually be written in.
const MODULE_HELPER: &str = r#"
--- compute = import "compute.lichen" ---
square = x => x * x
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  compute.write ((compute.Write _)(.to n, .at i, .value square v))
}) "BACKEND"
seed = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i + 1))
}) "BACKEND"
s = compute.plrun seed (8,)
compute.collect (compute.plrun p (8, (s,)))
"#;

/// A single-parameter recursive helper whose trip count is a **literal** at the
/// call site. Expansion only terminates if the conditional's selector folds to a
/// constant, so this is the program that says whether it does.
const RECURSIVE_LITERAL: &str = r#"
--- compute = import "compute.lichen" ---
steps = k => if k == 0 then 0 else steps (k - 1) + 1
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value steps 4 + compute.range n * 0))
}) "BACKEND"
compute.read ((compute.Read _)(.from compute.plrun p (8,), .at 3))
"#;

/// A **body-local alias** and no call at all. This is the control for the two
/// probes above: if it fails, the alias — not the inlining — is what cannot be
/// resolved in an unapplied template.
const BODY_LOCAL_ALIAS: &str = r#"
--- compute = import "compute.lichen" ---
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  compute.write ((compute.Write _)(.to n, .at i, .value v))
}) "BACKEND"
seed = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i + 1))
}) "BACKEND"
s = compute.plrun seed (8,)
compute.collect (compute.plrun p (8, (s,)))
"#;

/// The helper called with a **literal** argument, so nothing but the callee is
/// unresolved. This separates "the callee cannot be found" from "the argument
/// cannot be re-emitted".
const HELPER_LITERAL_ARG: &str = r#"
--- compute = import "compute.lichen" ---
square = x => x * x
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value square 3))
}) "BACKEND"
compute.collect (compute.plrun p (4,))
"#;

/// The same helper, called with an argument the **host cannot reduce** —
/// `i + 1` depends on the loop index, so the call survives to the kernel
/// compiler. This is the case static expansion exists for.
const HELPER_INDEX_ARG: &str = r#"
--- compute = import "compute.lichen" ---
square = x => x * x
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value square (i + 1)))
}) "BACKEND"
compute.collect (compute.plrun p (4,))
"#;

/// **A `loop` operator written in pure lichen — no Rust, no new operator.** It is
/// the proposal as a one-line library function, using the recursion the
/// interpreter already has:
///
/// ```lichen
/// loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)
/// ```
///
/// If this works in a kernel body, the whole feature is free and the roadmap
/// item is a library function rather than a codegen task.
const LOOP_IN_LICHEN: &str = r#"
--- compute = import "compute.lichen" ---
loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)
inc = x => x + 1
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value loop inc 3 i))
}) "BACKEND"
compute.collect (compute.plrun p (4,))
"#;

/// The same, with the trip count the **kernel's own count** — a runtime value.
/// This is the case §4.1 says is refused, and `loop` is supposed to be worse,
/// not better: `n` is not decided when the body is lowered.  The `@loop` mark
/// is what makes the refusal name itself.
const LOOP_RUNTIME_COUNT: &str = r#"
--- compute = import "compute.lichen" ---
@loop loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)
inc = x => x + 1
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value loop inc n i))
}) "BACKEND"
compute.read ((compute.Read _)(.from compute.plrun p (4,), .at 3))
"#;

/// The same, `@loop`-marked, with the trip count a **literal** — the marker as
/// permission rather than a command.  This is the control for the row above:
/// the marker must not turn a recursion the unroll already handles into a
/// refusal.
const LOOP_RUNTIME_COUNT_MARKED_DECIDABLE: &str = r#"
--- compute = import "compute.lichen" ---
@loop loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)
inc = x => x + 1
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value loop inc 3 i))
}) "BACKEND"
compute.read ((compute.Read _)(.from compute.plrun p (4,), .at 3))
"#;

/// A `@loop`-marked recursion whose trip count is **decided** and whose whole
/// state is decided too — the shape the unroll handles, marked.  This is the
/// row that must keep answering, and the proof that the marker is permission
/// and not a command.
const RECURSIVE_LITERAL_MARKED: &str = r#"
--- compute = import "compute.lichen" ---
@loop steps = k => if k == 0 then 0 else steps (k - 1) + 1
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value steps 4 + compute.range n * 0))
}) "BACKEND"
compute.read ((compute.Read _)(.from compute.plrun p (8,), .at 3))
"#;

/// Two-stage **curried** recursion — `sum_to (n - 1) (x + 1)` is *two*
/// applications, where `fib`'s single application works. If this is what the
/// `loop` combinator's type error is, the fix is a one-argument shape.
const TWO_STAGE_CURRIED: &str = r#"
--- compute = import "compute.lichen" ---
sum_to = n => x => if n == 0 then x else sum_to (n - 1) (x + 1)
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value sum_to 3 i))
}) "BACKEND"
compute.collect (compute.plrun p (4,))
"#;

/// The same, with the two arguments in **one** tuple — one application, so one
/// instantiation of the binding.
const ONE_STAGE_TUPLE: &str = r#"
--- compute = import "compute.lichen" ---
sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value sum_to (3, i)))
}) "BACKEND"
compute.collect (compute.plrun p (4,))
"#;

/// The proposed operator, written over a **tuple** state instead of a curried
/// pair, so the recursive call is a single application.
const LOOP_TUPLE: &str = r#"
--- compute = import "compute.lichen" ---
loop = f => s => if s(0) == 0 then s(1) else loop f (s(0) - 1, f s(1))
inc = x => x + 1
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value loop inc (3, i)))
}) "BACKEND"
compute.collect (compute.plrun p (4,))
"#;

/// `loop` in the shape that **does** check: the step is baked into the binding
/// and the recursive call is a single application of one tuple argument. This is
/// what a trip count is measured against — the expansion is `n` copies of the
/// step's body, so the interesting number is how that grows.
fn loop_program(trip: usize) -> String {
    format!(
        r#"--- compute = import "compute.lichen" ---
sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)
p = compute.parallel (cfg => {{
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value sum_to ({trip}, i)))
}}) "BACKEND"
compute.read ((compute.Read _)(.from compute.plrun p (4,), .at 3))
"#
    )
}

/// The same expansion with **no kernel at all** — a plain host recursion. If this
/// overflows too, the depth is in the graph the host builds, not in the emitter
/// that lowers it, and the two need different fixes.
fn host_program(trip: usize) -> String {
    format!(
        "sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)\n\
         sum_to ({trip}, 0)\n"
    )
}

/// A kernel body carrying a **marked recursion whose trip count is the
/// kernel's own count** — a run-time value, so the definition pass cannot
/// expand it and the checker has to say what it is instead. What comes back is
/// the conversion's verdict: the shape rule that refused it, or the missing
/// backend for a shape that converts.
fn runtime_count_probe(recursion: &str, call: &str) -> String {
    format!(
        "--- compute = import \"compute.lichen\" ---\n\
         p = compute.parallel (cfg => {{\n  n = cfg(0)\n  i = compute.range n\n  {recursion}\n  \
         compute.write ((compute.Write _)(.to n, .at i, .value {call}))\n}}) \"BACKEND\"\n\
         compute.read ((compute.Read _)(.from compute.plrun p (8,), .at 3))\n"
    )
}

/// The marked recursions the verdict is read from, one per shape: the two
/// convertible ones (a scalar state and a tuple state) and the three refusals
/// (a call outside tail position, a mutual recursion, the curried combinator
/// whose call hides in nested lambdas).
const VERDICT_ROWS: &[(&str, &str, &str)] = &[
    (
        "tail recursion, scalar state",
        "@loop count = k => if k == 0 then 0 else count (k - 1)",
        "count i",
    ),
    (
        "tail recursion, tuple state",
        "@loop sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)",
        "sum_to (i, 0)",
    ),
    (
        "recursion outside tail position",
        "@loop steps = k => if k == 0 then 0 else steps (k - 1) + 1",
        "steps i",
    ),
    (
        "mutual recursion",
        "@loop even = k => if k == 0 then 1 else odd (k - 1)\n  \
         @loop odd = k => if k == 0 then 0 else even (k - 1)",
        "even i",
    ),
    (
        "the `loop` combinator, curried",
        "@loop loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)\n  \
         inc = x => x + 1",
        "loop inc i 0",
    ),
];

/// The acceptance case's shape in a **host** program: a marked reduction over a
/// tuple state whose count is a literal, so the evaluator can decide the whole
/// state — and the only thing between it and a value is *how the recursion is
/// run*. `marked` is the whole difference between the two rows the probe prints.
fn host_reduction(marked: bool, count: usize) -> String {
    let marker = if marked { "@loop " } else { "" };
    format!(
        "{marker}sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)\n\
         sum_to ({count}, 0)\n"
    )
}

/// The proposed operator **with its type written out** — the escape P1-33 names
/// as "the whole difference" for the two-argument curried self-reference.
const LOOP_ANNOTATED: &str = r#"
--- compute = import "compute.lichen" ---
loop = (f => n => x => if n == 0 then x else loop f (n - 1) (f x)) : (Int -> Int) -> Int -> Int -> Int
inc = x => x + 1
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value loop inc 3 i))
}) "BACKEND"
compute.collect (compute.plrun p (4,))
"#;

/// The annotated operator, with **no kernel** — which separates "the annotation
/// fixed the type" from "the annotation plus a kernel works".
const LOOP_ANNOTATED_HOST: &str = r#"
loop = (f => n => x => if n == 0 then x else loop f (n - 1) (f x)) : (Int -> Int) -> Int -> Int -> Int
inc = x => x + 1
(loop inc 3 0, loop inc 10 5, loop inc 0 7)
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
  compute.write ((compute.Write _)(.to n, .at i, .value compute.call k{last} i))
}}) \"BACKEND\"\ncompute.read ((compute.Read _)(.from compute.plrun p (COUNT,), .at 3))\n",
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
  compute.write ((compute.Write _)(.to n, .at i, .value {body}))
}}) \"BACKEND\"
compute.read ((compute.Read _)(.from compute.plrun p (COUNT,), .at 3))\n"
    )
}

/// A self-recursive function, called from inside a kernel body.
const RECURSIVE_INLINE: &str = r#"
--- compute = import "compute.lichen" ---
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  @loop count_up = s => k => if k == 0 then s else count_up (s + 1) (k - 1)
  compute.write ((compute.Write _)(.to n, .at i, .value count_up 0 i))
}) "BACKEND"
compute.read ((compute.Read _)(.from compute.plrun p (8,), .at 3))
"#;

/// A kernel whose own body names the kernel it is being compiled into.
const RECURSIVE_CROSS: &str = r#"
--- compute = import "compute.lichen" ---
k = compute.jit (v : Int => compute.call k v + 1)
compute.launch k 3
"#;

/// Two kernels naming each other — a cycle through the kernel registry.
const MUTUAL: &str = r#"
--- compute = import "compute.lichen" ---
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

        println!("  -- the `loop` operator, as a lichen function --");
        probe(
            "loop annotated, no kernel",
            &LOOP_ANNOTATED_HOST.to_string(),
        );
        probe(
            "loop, its type written out",
            &LOOP_ANNOTATED.replace("BACKEND", backend),
        );
        probe(
            "loop, decidable trip count",
            &LOOP_IN_LICHEN.replace("BACKEND", backend),
        );
        probe(
            "loop, trip count = the kernel count",
            &LOOP_RUNTIME_COUNT.replace("BACKEND", backend),
        );
        probe(
            "loop marked, decidable trip count",
            &LOOP_RUNTIME_COUNT_MARKED_DECIDABLE.replace("BACKEND", backend),
        );
        probe(
            "two-stage curried recursion",
            &TWO_STAGE_CURRIED.replace("BACKEND", backend),
        );
        probe(
            "one-stage, tuple argument",
            &ONE_STAGE_TUPLE.replace("BACKEND", backend),
        );
        probe(
            "loop over a tuple state",
            &LOOP_TUPLE.replace("BACKEND", backend),
        );
        println!("  -- where the depth goes: host graph vs emitter --");
        for trip in [100usize, 400, 1_000, 4_000] {
            let start = Instant::now();
            match run(&host_program(trip)) {
                Ok(out) => println!(
                    "  host   {trip:>6}  {:>9.1} ms  {out}",
                    start.elapsed().as_secs_f64() * 1000.0
                ),
                Err(diags) => println!(
                    "  host   {trip:>6}  {:>9.1} ms  refused: {}",
                    start.elapsed().as_secs_f64() * 1000.0,
                    diags.join(" | ")
                ),
            }
        }
        println!("  -- what a trip count costs: expansion is O(n) code --");
        println!("  {:>8}  {:>12}  {}", "trip", "compile+run", "answer");
        for trip in [1usize, 10, 100, 400] {
            let source = loop_program(trip).replace("BACKEND", backend);
            let start = Instant::now();
            match run(&source) {
                Ok(out) => println!(
                    "  {trip:>8}  {:>10.1} ms  {}",
                    start.elapsed().as_secs_f64() * 1000.0,
                    out
                ),
                Err(diags) => println!(
                    "  {trip:>8}  {:>10.1} ms  refused: {}",
                    start.elapsed().as_secs_f64() * 1000.0,
                    diags.join(" | ")
                ),
            }
        }
        println!("  -- the conversion's verdict on a run-time count --");
        for (name, recursion, call) in VERDICT_ROWS {
            probe(
                name,
                &runtime_count_probe(recursion, call).replace("BACKEND", backend),
            );
        }
        println!("  -- the host loop: a literal count, run rather than expanded --");
        // Host rows are backend-independent, so the counts stay small: the
        // point is where the two shapes part company, and each loop iteration
        // instantiates the body (the cost M3 exists to remove).
        for count in [400usize, 1_000] {
            for marked in [true, false] {
                let shape = if marked { "loop  " } else { "unroll" };
                let start = Instant::now();
                match run(&host_reduction(marked, count)) {
                    Ok(out) => println!(
                        "  {shape}  {count:>6}  {:>9.1} ms  {out}",
                        start.elapsed().as_secs_f64() * 1000.0
                    ),
                    Err(diags) => println!(
                        "  {shape}  {count:>6}  {:>9.1} ms  refused: {}",
                        start.elapsed().as_secs_f64() * 1000.0,
                        diags.join(" | ")
                    ),
                }
            }
        }
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
            "the same, @loop-marked",
            &RECURSIVE_LITERAL_MARKED.replace("BACKEND", backend),
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
