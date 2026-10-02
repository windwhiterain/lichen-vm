//! What an algorithm actually costs on each backend: one element-wise kernel at
//! several counts, and a chain of sixteen, since the note's claim is that a
//! single dispatch never repays itself and a chain does.
//!
//! The number reported is the whole program — building the input, the kernels
//! and the read-back — because that is what a lichen program pays; the note's
//! tables measure a dispatch and a fetch separately, which is a different
//! question.

use std::time::Instant;

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;

fn run(source: &str) -> Result<String, Vec<String>> {
    let mut store = PackageStore::<LangProgram>::new();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .map_err(|diags| diags.into_iter().map(|d| d.message).collect())
}

/// `mk` fills a buffer; `axpy` is one `y = 3x + y` dispatch and the answer is
/// read back, so a run pays an upload, a dispatch and a download.
const ONE: &str = r#"
--- compute = import "compute.lichen" ---
mk = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i % 97]
}
kx = compute.parallel mk "BACKEND"
x = compute.plrun kx (COUNT,)
axpy = cfg => {
  n = cfg(0)
  i = compute.range n
  xv = compute.read [cfg(1)(0), i]
  compute.write [n, i, 3 * xv]
}
ka = compute.parallel axpy "BACKEND"
out = compute.plrun ka (COUNT, (x,))
compute.read [out, COUNT / 2]
"#;

/// The same input, then sixteen dependent links recorded as one `compute.graph`.
const CHAIN: &str = r#"
--- compute = import "compute.lichen" ---
mk = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i % 97]
}
kx = compute.parallel mk "BACKEND"
k1 = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read [cfg(1)(0), i]
  compute.write [n, i, v + 1]
}) "BACKEND"
k2 = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read [cfg(1)(0), i]
  compute.write [n, i, v + v]
}) "BACKEND"
grow = ins => {
  a = compute.plrun k1 (ins(0), (ins(1),))
  b = compute.plrun k2 (ins(0), (a,))
  c = compute.plrun k1 (ins(0), (b,))
  d = compute.plrun k2 (ins(0), (c,))
  e = compute.plrun k1 (ins(0), (d,))
  f = compute.plrun k2 (ins(0), (e,))
  g = compute.plrun k1 (ins(0), (f,))
  h = compute.plrun k2 (ins(0), (g,))
  i2 = compute.plrun k1 (ins(0), (h,))
  j = compute.plrun k2 (ins(0), (i2,))
  k = compute.plrun k1 (ins(0), (j,))
  l = compute.plrun k2 (ins(0), (k,))
  m = compute.plrun k1 (ins(0), (l,))
  o = compute.plrun k2 (ins(0), (m,))
  p = compute.plrun k1 (ins(0), (o,))
  compute.plrun k2 (ins(0), (p,))
}
built = compute.graph grow
seed = compute.plrun kx (COUNT,)
compute.read [compute.graphrun built (COUNT, seed), COUNT / 2]
"#;

fn timed(template: &str, backend: &str, count: usize, repeats: usize) -> Result<f64, String> {
    let source = template
        .replace("BACKEND", backend)
        .replace("COUNT", &count.to_string());
    // The first run pays for shader compilation, so it is not in the number.
    run(&source).map_err(|d| d.join(" | "))?;
    let mut best = f64::INFINITY;
    for _ in 0..repeats {
        let start = Instant::now();
        let outcome = run(&source);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        outcome.map_err(|d| d.join(" | "))?;
        if elapsed < best {
            best = elapsed;
        }
    }
    Ok(best)
}

fn main() {
    let backends: Vec<String> = std::env::args().skip(1).collect();
    let backends: Vec<String> = if backends.is_empty() {
        vec!["cpu".to_string()]
    } else {
        backends
    };
    let counts = [65_536usize, 262_144, 1_048_576];
    let repeats = 20;

    for backend in &backends {
        if backend == "gpu" {
            match lichen_compute_gpu::install_default() {
                Ok(()) => println!("(device installed)"),
                Err(reason) => {
                    println!("no device: {reason}");
                    continue;
                }
            }
        }
        println!("\n=== {backend}: whole program, best of {repeats} (ms) ===");
        println!("{:>10}  {:>10}  {:>10}", "count", "1 kernel", "16 links");
        for count in counts {
            let one = timed(ONE, backend, count, repeats);
            let chain = timed(CHAIN, backend, count, repeats);
            let reason = match &chain {
                Ok(_) => None,
                Err(reason) => Some(reason.clone()),
            };
            let show = |value: &Result<f64, String>| match value {
                Ok(ms) => format!("{ms:>10.3}"),
                Err(_) => format!("{:>10}", "n/a"),
            };
            println!("{:>10}  {}  {}", count, show(&one), show(&chain));
            if let Some(reason) = reason {
                println!("           chain refused: {reason}");
            }
        }
        if backend == "gpu" {
            lichen_compute_gpu::uninstall();
        }
    }
}
