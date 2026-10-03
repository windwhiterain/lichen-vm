//! A ladder of GPU algorithms, run against whichever backend is named.
//!
//! This is exploration tooling, not a test: it prints what each program answers
//! and prints the refusal when it cannot run, so a gap in the language shows up
//! as a named refusal rather than as a failing assertion somewhere else.

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;

fn run(source: &str) -> Result<String, Vec<String>> {
    let mut store = PackageStore::<LangProgram>::new();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .map_err(|diags| diags.into_iter().map(|d| d.message).collect())
}

/// `(name, source)` — each is a complete program.
const PROGRAMS: &[(&str, &str)] = &[
    (
        "1. fill — write a constant per index",
        r#"
--- compute = import "compute.lichen" ---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value 7))
}
k = compute.parallel f "BACKEND"
b = compute.plrun k (8,)
compute.collect b
"#,
    ),
    (
        "2. axpy — y = a*x + y, two inputs one output",
        r#"
--- compute = import "compute.lichen" ---
mk = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i + 1))
}
kx = compute.parallel mk "BACKEND"
x = compute.plrun kx (8,)
ky = compute.parallel mk "BACKEND"
y = compute.plrun ky (8,)
axpy = cfg => {
  n = cfg(0)
  i = compute.range n
  xv = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  yv = compute.read ((compute.Read _)(.from cfg(1)(1), .at i))
  compute.write ((compute.Write _)(.to n, .at i, .value 3 * xv + yv))
}
ka = compute.parallel axpy "BACKEND"
out = compute.plrun ka (8, (x, y))
compute.collect out
"#,
    ),
    (
        "3. transpose — gather, write index is not the read index",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (8,)
tr = cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read ((compute.Read _)(.from cfg(1)(0), .at i % 4))
  compute.write ((compute.Write _)(.to n, .at i, .value v))
}
kt = compute.parallel tr "BACKEND"
out = compute.plrun kt (8, (src,))
compute.collect out
"#,
    ),
    (
        "4. stencil — a neighbour read at a computed, clamped index",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (8,)
st = cfg => {
  n = cfg(0)
  i = compute.range n
  left = if i == 0 then 0 else i - 1
  v = compute.read ((compute.Read _)(.from cfg(1)(0), .at left))
  compute.write ((compute.Write _)(.to n, .at i, .value v))
}
ks = compute.parallel st "BACKEND"
out = compute.plrun ks (8, (src,))
compute.collect out
"#,
    ),
    (
        "5. select-into-read — clamp by arithmetic, no conditional",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (8,)
st = cfg => {
  n = cfg(0)
  i = compute.range n
  j = i - (i == 0) * i
  v = compute.read ((compute.Read _)(.from cfg(1)(0), .at j))
  compute.write ((compute.Write _)(.to n, .at i, .value v))
}
ks = compute.parallel st "BACKEND"
out = compute.plrun ks (8, (src,))
compute.collect out
"#,
    ),
    (
        "6. dot4 — an unrolled 4-term reduction, fixed length",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (4,)
dot = cfg => {
  n = cfg(0)
  i = compute.range n
  a = compute.read ((compute.Read _)(.from cfg(1)(0), .at 0))
  b = compute.read ((compute.Read _)(.from cfg(1)(0), .at 1))
  c = compute.read ((compute.Read _)(.from cfg(1)(0), .at 2))
  d = compute.read ((compute.Read _)(.from cfg(1)(0), .at 3))
  compute.write ((compute.Write _)(.to n, .at i, .value a * a + b * b + c * c + d * d))
}
kd = compute.parallel dot "BACKEND"
out = compute.plrun kd (1, (src,))
compute.read ((compute.Read _)(.from out, .at 0))
"#,
    ),
    (
        "7. mat2 — a correct 2x2 multiply",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i + 1))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (4,)
mm = cfg => {
  n = cfg(0)
  i = compute.range n
  row = i / 2
  col = i % 2
  a00 = compute.read ((compute.Read _)(.from cfg(1)(0), .at 0))
  a01 = compute.read ((compute.Read _)(.from cfg(1)(0), .at 1))
  a10 = compute.read ((compute.Read _)(.from cfg(1)(0), .at 2))
  a11 = compute.read ((compute.Read _)(.from cfg(1)(0), .at 3))
  b0 = compute.read ((compute.Read _)(.from cfg(1)(1), .at col * 2 + 0))
  b1 = compute.read ((compute.Read _)(.from cfg(1)(1), .at col * 2 + 1))
  (compute.write ((compute.Write _)(.to n, .at i, .value a00 * b0 + a01 * b1)),
   compute.write ((compute.Write _)(.to n, .at i, .value a10 * b0 + a11 * b1)))
}
km = compute.parallel mm "BACKEND"
outs = compute.plrun km (4, (src, src))
(compute.collect outs(0), compute.collect outs(1))
"#,
    ),
    (
        "8. reduction — a sum, the classic cross-index algorithm",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (8,)
red = cfg => {
  n = cfg(0)
  i = compute.range n
  a = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  compute.write ((compute.Write _)(.to n, .at i, .value a + 1))
}
kr = compute.parallel red "BACKEND"
out = compute.plrun kr (8, (src,))
compute.read ((compute.Read _)(.from out, .at 3))
"#,
    ),
    (
        "9. two-buffer contraction — does a write and a read share an index space?",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value 2))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (8,)
mix = cfg => {
  n = cfg(0)
  i = compute.range n
  a = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  b = compute.read ((compute.Read _)(.from cfg(1)(0), .at i - i))
  compute.write ((compute.Write _)(.to n, .at i, .value a + b))
}
km = compute.parallel mix "BACKEND"
out = compute.plrun km (8, (src,))
compute.read ((compute.Read _)(.from out, .at 3))
"#,
    ),
    (
        "8b. tree reduction — halve the length, four levels",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
a = compute.plrun k0 (16,)
pair = cfg => {
  n = cfg(0)
  i = compute.range n
  x = compute.read ((compute.Read _)(.from cfg(1)(0), .at i * 2))
  y = compute.read ((compute.Read _)(.from cfg(1)(0), .at i * 2 + 1))
  compute.write ((compute.Write _)(.to n, .at i, .value x + y))
}
kp = compute.parallel pair "BACKEND"
b = compute.plrun kp (8, (a,))
c = compute.plrun kp (4, (b,))
d = compute.plrun kp (2, (c,))
e = compute.plrun kp (1, (d,))
compute.read ((compute.Read _)(.from e, .at 0))
"#,
    ),
    (
        "8c. prefix sum — a scan, one write per index from many reads",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value 1))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (4,)
scan = cfg => {
  n = cfg(0)
  i = compute.range n
  a = compute.read ((compute.Read _)(.from cfg(1)(0), .at 0))
  b = compute.read ((compute.Read _)(.from cfg(1)(0), .at 1))
  c = compute.read ((compute.Read _)(.from cfg(1)(0), .at 2))
  d = compute.read ((compute.Read _)(.from cfg(1)(0), .at 3))
  compute.write ((compute.Write _)(.to n, .at i, .value a + b + c + d))
}
ks = compute.parallel scan "BACKEND"
out = compute.plrun ks (4, (src,))
compute.collect out
"#,
    ),
    (
        "8d. out-of-range read — is a bad index refused?",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i + 1))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (4,)
oob = cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read ((compute.Read _)(.from cfg(1)(0), .at 4))
  compute.write ((compute.Write _)(.to n, .at i, .value v))
}
ko = compute.parallel oob "BACKEND"
out = compute.plrun ko (2, (src,))
compute.collect out
"#,
    ),
    (
        "8e. scalar kernel parameter — is anything but a buffer allowed?",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i + 1))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (4,)
sc = cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  compute.write ((compute.Write _)(.to n, .at i, .value v * cfg(2)))
}
ks = compute.parallel sc "BACKEND"
out = compute.plrun ks (4, (src, 5))
compute.collect out
"#,
    ),
    (
        "9. cross-kernel call — a shared helper inside a parallel body",
        r#"
--- compute = import "compute.lichen" ---
helper = compute.jit (v : Int => v * v : Int)
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i + 1))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (8,)
use = cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  compute.write ((compute.Write _)(.to n, .at i, .value helper.native v))
}
ku = compute.parallel use "BACKEND"
out = compute.plrun ku (8, (src,))
compute.collect out
"#,
    ),
    (
        "9b. histogram — a scatter-accumulate into a shared slot",
        r#"
--- compute = import "compute.lichen" ---
n0 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i % 3))
}
k0 = compute.parallel n0 "BACKEND"
src = compute.plrun k0 (64,)
hist = cfg => {
  n = cfg(0)
  i = compute.range n
  key = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  compute.write ((compute.Write _)(.to 3, .at key, .value 1))
}
kh = compute.parallel hist "BACKEND"
out = compute.plrun kh (64, (src,))
(compute.read ((compute.Read _)(.from out, .at 0)), compute.read ((compute.Read _)(.from out, .at 1)), compute.read ((compute.Read _)(.from out, .at 2)))
"#,
    ),
    (
        "10. host data in — an input array the program owns",
        r#"
--- compute = import "compute.lichen" ---
data = [3, 1, 4, 1, 5, 9, 2, 6]
k = cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  compute.write ((compute.Write _)(.to n, .at i, .value v + 1))
}
kk = compute.parallel k "BACKEND"
out = compute.plrun kk (8, (data,))
compute.collect out
"#,
    ),
    (
        "11. float — is there a real number at all?",
        r#"
x = 1.5
x + 1
"#,
    ),
    (
        "12. graph — one submission for a two-link chain",
        r#"
--- compute = import "compute.lichen" ---
f1 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value i + 10))
}
k1 = compute.parallel f1 "BACKEND"
f2 = cfg => {
  n = cfg(0)
  i = compute.range n
  a = compute.read ((compute.Read _)(.from cfg(1)(0), .at i))
  compute.write ((compute.Write _)(.to n, .at i, .value a + a))
}
k2 = compute.parallel f2 "BACKEND"
step = ins => compute.plrun k2 (ins(0), (compute.plrun k1 (ins(0),),))
built = compute.graph step
out = compute.graphrun built (4,)
compute.collect out
"#,
    ),
];

fn main() {
    let backends: Vec<String> = std::env::args().skip(1).collect();
    let backends: Vec<String> = if backends.is_empty() {
        vec!["cpu".to_string()]
    } else {
        backends
    };

    for backend in backends {
        println!("\n=== backend: {backend} ===");
        let gpu = backend == "gpu";
        if gpu {
            match lichen_compute_gpu::install_default() {
                Ok(()) => println!("(device installed)"),
                Err(reason) => {
                    println!("no device: {reason}");
                    continue;
                }
            }
        }
        for (name, template) in PROGRAMS {
            let source = template.replace("BACKEND", &backend);
            print!("{name}\n  ");
            match run(&source) {
                Ok(out) => println!("OK  {out}"),
                Err(diags) => {
                    println!("REFUSED");
                    for message in diags {
                        println!("      {message}");
                    }
                }
            }
        }
        if gpu {
            lichen_compute_gpu::uninstall();
        }
    }
}
