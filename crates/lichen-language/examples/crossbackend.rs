//! Is a `"cpu"` run able to poison a later `"gpu"` one in the same process?
//!
//! The design says the backend lives on the value and is deliberately *not*
//! hashed into `fragment_digest`, so two programs that compile the same body
//! for different backends share one fragment id. This probes whether sharing an
//! id also shares the answer.

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;

fn run(source: &str) -> Result<String, Vec<String>> {
    let mut store = PackageStore::<LangProgram>::new();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .map_err(|diags| diags.into_iter().map(|d| d.message).collect())
}

/// A plain `v + 1` dispatch over `n` elements, read back at one index.
const PLAIN: &str = r#"
--- compute = import "compute.lichen" ---
k = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read [cfg(1)(0), i]
  compute.write [n, i, v + 1]
}) "BACKEND"
seed = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i]
}) "BACKEND"
s = compute.plrun seed (4,)
compute.read [compute.plrun k (4, (s,)), 2]
"#;

/// The same `v + 1` body, twice over, recorded as a graph.
const GRAPHED: &str = r#"
--- compute = import "compute.lichen" ---
k = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read [cfg(1)(0), i]
  compute.write [n, i, v + 1]
}) "BACKEND"
seed = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i]
}) "BACKEND"
grow = ins => compute.plrun k (ins(0), (ins(1),))
built = compute.graph grow
s = compute.plrun seed (4,)
compute.read [compute.graphrun built (4, s), 2]
"#;

/// The bench's 16-link chain, verbatim, which is the shape that broke.
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
  compute.plrun k2 (ins(0), (c,))
}
built = compute.graph grow
seed = compute.plrun kx (4,)
compute.read [compute.graphrun built (4, seed), 1]
"#;

fn probe(label: &str, template: &str, backend: &str) {
    let source = template.replace("BACKEND", backend);
    match run(&source) {
        Ok(out) => println!("  {label:<34} {backend:<4} OK      {out}"),
        Err(diags) => println!("  {label:<34} {backend:<4} REFUSED {}", diags.join(" | ")),
    }
}

fn main() {
    println!("\nno backend installed yet:");
    probe("16-link chain", CHAIN, "cpu");
    let _ = lichen_compute_gpu::install_default();
    println!("\nbackend installed:");
    probe("16-link chain", CHAIN, "gpu");
    probe("plain plrun", PLAIN, "gpu");
    probe("graph", GRAPHED, "gpu");
    probe("16-link chain", CHAIN, "gpu");
    lichen_compute_gpu::uninstall();
}
