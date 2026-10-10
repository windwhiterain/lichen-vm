//! Could recursion expansion stand in for a loop as the unroll's surface?

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
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value compute.call k0 i))
}
p = compute.parallel f "BACKEND"
out = (compute.plrun p (Host(.n 8)) : Out)
compute.read ((compute.Read _)(.from out.z, .at 3))
"#;

/// A **module-level** helper called from inside a parallel body.
const MODULE_HELPER: &str = r#"
--- compute = import "compute.lichen" ---
square = x => x * x
Out   = struct<.z (compute.Buf _)>
Par0  = struct<.n Int, .out Out>
Host0 = struct<.n Int>
seedf = (k : Par0) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 1))
}
seed = compute.parallel seedf "BACKEND"
s = (compute.plrun seed (Host0(.n 8)) : Out)
In1  = struct<.b (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out)
pf = (k : Par1) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value square v))
}
p = compute.parallel pf "BACKEND"
out = (compute.plrun p ((compute.A In1)(.n 8, .I In1(.b s.z))) : Out)
compute.collect out.z
"#;

/// A recursion with a **literal** trip count: expansion terminates only if the
/// selector folds to a constant.
const RECURSIVE_LITERAL: &str = r#"
--- compute = import "compute.lichen" ---
steps = k => if k == 0 then 0 else steps (k - 1) + 1
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value steps 4 + i * 0))
}
p = compute.parallel f "BACKEND"
out = (compute.plrun p (Host(.n 8)) : Out)
compute.read ((compute.Read _)(.from out.z, .at 3))
"#;

/// A **body-local alias** and no call: the control for the probes above.
const BODY_LOCAL_ALIAS: &str = r#"
--- compute = import "compute.lichen" ---
Out   = struct<.z (compute.Buf _)>
Par0  = struct<.n Int, .out Out>
Host0 = struct<.n Int>
seedf = (k : Par0) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 1))
}
seed = compute.parallel seedf "BACKEND"
s = (compute.plrun seed (Host0(.n 8)) : Out)
In1  = struct<.b (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out)
pf = (k : Par1) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value v))
}
p = compute.parallel pf "BACKEND"
out = (compute.plrun p ((compute.A In1)(.n 8, .I In1(.b s.z))) : Out)
compute.collect out.z
"#;

/// The helper called with a **literal** argument, so only the callee is
/// unresolved.
const HELPER_LITERAL_ARG: &str = r#"
--- compute = import "compute.lichen" ---
square = x => x * x
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value square 3))
}
p = compute.parallel f "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.collect out.z
"#;

/// The same helper, called with an argument the **host cannot reduce**.
const HELPER_INDEX_ARG: &str = r#"
--- compute = import "compute.lichen" ---
square = x => x * x
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value square (i + 1)))
}
p = compute.parallel f "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.collect out.z
"#;

/// **A `loop` operator written in pure lichen**, using the recursion the
/// interpreter already has.
const LOOP_IN_LICHEN: &str = r#"
--- compute = import "compute.lichen" ---
loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)
inc = x => x + 1
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
body = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value loop inc 3 i))
}
p = compute.parallel body "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.collect out.z
"#;

/// The same, with the trip count the **kernel's own count**: `n` is not decided
/// when the body is lowered.
const LOOP_RUNTIME_COUNT: &str = r#"
--- compute = import "compute.lichen" ---
@loop loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)
inc = x => x + 1
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
body = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value loop inc k.n i))
}
p = compute.parallel body "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.read ((compute.Read _)(.from out.z, .at 3))
"#;

/// The same, `@loop`-marked, with a **literal** trip count: the marker is
/// permission, not a command.
const LOOP_RUNTIME_COUNT_MARKED_DECIDABLE: &str = r#"
--- compute = import "compute.lichen" ---
@loop loop = f => n => x => if n == 0 then x else loop f (n - 1) (f x)
inc = x => x + 1
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
body = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value loop inc 3 i))
}
p = compute.parallel body "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.read ((compute.Read _)(.from out.z, .at 3))
"#;

/// A `@loop`-marked recursion with a **decided** count and a decided state.
const RECURSIVE_LITERAL_MARKED: &str = r#"
--- compute = import "compute.lichen" ---
@loop steps = k => if k == 0 then 0 else steps (k - 1) + 1
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value steps 4 + i * 0))
}
p = compute.parallel f "BACKEND"
out = (compute.plrun p (Host(.n 8)) : Out)
compute.read ((compute.Read _)(.from out.z, .at 3))
"#;

/// Two-stage **curried** recursion: `sum_to (n - 1) (x + 1)` is two
/// applications.
const TWO_STAGE_CURRIED: &str = r#"
--- compute = import "compute.lichen" ---
sum_to = n => x => if n == 0 then x else sum_to (n - 1) (x + 1)
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value sum_to 3 i))
}
p = compute.parallel f "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.collect out.z
"#;

/// The same, with the two arguments in **one** tuple — one application, so one
/// instantiation of the binding.
const ONE_STAGE_TUPLE: &str = r#"
--- compute = import "compute.lichen" ---
sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value sum_to (3, i)))
}
p = compute.parallel f "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.collect out.z
"#;

/// The proposed operator over a **tuple** state, so the recursive call is one
/// application.
const LOOP_TUPLE: &str = r#"
--- compute = import "compute.lichen" ---
loop = f => s => if s(0) == 0 then s(1) else loop f (s(0) - 1, f s(1))
inc = x => x + 1
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
body = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value loop inc (3, i)))
}
p = compute.parallel body "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.collect out.z
"#;

/// `loop` in the shape that **does** check: the step is baked into the binding.
fn loop_program(trip: usize) -> String {
    format!(
        r#"--- compute = import "compute.lichen" ---
sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
f = (k : Par) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value sum_to ({trip}, i)))
}}
p = compute.parallel f "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.read ((compute.Read _)(.from out.z, .at 3))
"#
    )
}

/// The same expansion with **no kernel**: the depth is then in the host's graph,
/// not the emitter's.
fn host_program(trip: usize) -> String {
    format!(
        "sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)\n\
         sum_to ({trip}, 0)\n"
    )
}

/// A **marked recursion whose trip count is the kernel's own count**: what comes
/// back is the conversion's verdict.
fn runtime_count_probe(recursion: &str, call: &str) -> String {
    format!(
        "--- compute = import \"compute.lichen\" ---\n\
         Out  = struct<.z (compute.Buf _)>\n\
         Par  = struct<.n Int, .out Out>\n\
         Host = struct<.n Int>\n\
         f = (k : Par) => {{\n  i = compute.range k.n\n  {recursion}\n  \
         compute.write ((compute.Write _)(.to k.out.z, .at i, .value {call}))\n}}\n\
         p = compute.parallel f \"BACKEND\"\n\
         out = (compute.plrun p (Host(.n 8)) : Out)\n\
         compute.read ((compute.Read _)(.from out.z, .at 3))\n"
    )
}

/// The marked recursions the verdict is read from, one per shape.
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

/// The acceptance case's shape in a **host** program: `marked` is the whole
/// difference between the two rows.
fn host_reduction(marked: bool, count: usize) -> String {
    let marker = if marked { "@loop " } else { "" };
    format!(
        "{marker}sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)\n\
         sum_to ({count}, 0)\n"
    )
}

/// The proposed operator **with its type written out** — the escape P1-33 names.
const LOOP_ANNOTATED: &str = r#"
--- compute = import "compute.lichen" ---
loop = (f => n => x => if n == 0 then x else loop f (n - 1) (f x)) : (Int -> Int) -> Int -> Int -> Int
inc = x => x + 1
Out  = struct<.z (compute.Buf _)>
Par  = struct<.n Int, .out Out>
Host = struct<.n Int>
body = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value loop inc 3 i))
}
p = compute.parallel body "BACKEND"
out = (compute.plrun p (Host(.n 4)) : Out)
compute.collect out.z
"#;

/// The annotated operator, with **no kernel**: does the annotation alone fix it?
const LOOP_ANNOTATED_HOST: &str = r#"
loop = (f => n => x => if n == 0 then x else loop f (n - 1) (f x)) : (Int -> Int) -> Int -> Int -> Int
inc = x => x + 1
(loop inc 3 0, loop inc 10 5, loop inc 0 7)
"#;

/// A `jit` chain, each level adding one: the hand-written, un-expanded loop.
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
        "Out = struct<.z (compute.Buf _)>\nPar = struct<.n Int, .out Out>\nHost = struct<.n Int>\nf = (k : Par) => {{\n  i = compute.range k.n\n  compute.write ((compute.Write _)(.to k.out.z, .at i, .value compute.call k{last} i))\n}}\np = compute.parallel f \"BACKEND\"\nout = (compute.plrun p (Host(.n COUNT)) : Out)\ncompute.read ((compute.Read _)(.from out.z, .at 3))\n",
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
        "{HEADER}In = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value {body}))
}}
p = compute.parallel f \"BACKEND\"
out = (compute.plrun p ((compute.A In)(.n COUNT, .I In(.a 0))) : Out)
compute.read ((compute.Read _)(.from out.z, .at 3))\n"
    )
}

/// A self-recursive function, called from inside a kernel body.
const RECURSIVE_INLINE: &str = r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  @loop count_up = s => m => if m == 0 then s else count_up (s + 1) (m - 1)
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value count_up 0 i))
}
p = compute.parallel f "BACKEND"
out = (compute.plrun p ((compute.A In)(.n 8, .I In(.a 0))) : Out)
compute.read ((compute.Read _)(.from out.z, .at 3))
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
    if std::env::var("RECURSION_PROBE").is_ok() {
        for backend in ["cpu", "gpu"] {
            probe(
                "helper, literal argument",
                &HELPER_LITERAL_ARG.replace("BACKEND", backend),
            );
            probe(
                "module-level helper in a body",
                &MODULE_HELPER.replace("BACKEND", backend),
            );
            probe(
                "loop, decidable trip count",
                &LOOP_IN_LICHEN.replace("BACKEND", backend),
            );
        }
        lichen_compute_gpu::uninstall();
        return;
    }
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
        // Host rows are backend-independent, so the counts stay small.
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
