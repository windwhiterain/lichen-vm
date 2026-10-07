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
InS  = struct<.a Int>
OutS = struct<.z (compute.Buf _)>
ParS = compute.P (compute.KT _)(.I InS, .O OutS)
seed = compute.parallel ((k : ParS) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i))
}) "BACKEND"
InK  = struct<.b (compute.Buf _)>
OutK = struct<.w (compute.Buf _)>
ParK = compute.P (compute.KT _)(.I InK, .O OutK)
k = compute.parallel ((h : ParK) => {
  i = compute.range h.n
  v = compute.read ((compute.Read _)(.from h.in.b, .at i))
  compute.write ((compute.Write _)(.to h.out.w, .at i, .value v + 1))
}) "BACKEND"
s = (compute.plrun seed ((compute.A InS)(.n 4, .I InS(.a 0))) : OutS)
out = (compute.plrun k ((compute.A InK)(.n 4, .I InK(.b s.z))) : OutK)
compute.read ((compute.Read _)(.from out.w, .at 2))
"#;

/// The same `v + 1` body, twice over, recorded as a graph.
const GRAPHED: &str = r#"
--- compute = import "compute.lichen" ---
InS  = struct<.a Int>
OutS = struct<.z (compute.Buf _)>
ParS = compute.P (compute.KT _)(.I InS, .O OutS)
seed = compute.parallel ((k : ParS) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i))
}) "BACKEND"
InK  = struct<.b (compute.Buf _)>
OutK = struct<.w (compute.Buf _)>
ParK = compute.P (compute.KT _)(.I InK, .O OutK)
k = compute.parallel ((h : ParK) => {
  i = compute.range h.n
  v = compute.read ((compute.Read _)(.from h.in.b, .at i))
  compute.write ((compute.Write _)(.to h.out.w, .at i, .value v + 1))
}) "BACKEND"
InG  = struct<.b (compute.Buf _)>
OutG = struct<.unused (compute.Buf _)>
ParG = compute.P (compute.KT _)(.I InG, .O OutG)
grow = (g : ParG) => compute.plrun k ((compute.A InK)(.n g.n, .I InK(.b g.in.b)))
built = compute.graph grow
s = (compute.plrun seed ((compute.A InS)(.n 4, .I InS(.a 0))) : OutS)
compute.read ((compute.Read _)(.from (compute.graphrun built ((compute.A InG)(.n 4, .I InG(.b s.z))) : (compute.Buf _)), .at 2))
"#;

/// The bench's 16-link chain, verbatim, which is the shape that broke.
const CHAIN: &str = r#"
--- compute = import "compute.lichen" ---
InX  = struct<.a Int>
OutX = struct<.z (compute.Buf _)>
ParX = compute.P (compute.KT _)(.I InX, .O OutX)
mk = (k : ParX) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i % 97))
}
kx = compute.parallel mk "BACKEND"
InK  = struct<.b (compute.Buf _)>
OutK = struct<.w (compute.Buf _)>
ParK = compute.P (compute.KT _)(.I InK, .O OutK)
k1 = compute.parallel ((h : ParK) => {
  i = compute.range h.n
  v = compute.read ((compute.Read _)(.from h.in.b, .at i))
  compute.write ((compute.Write _)(.to h.out.w, .at i, .value v + 1))
}) "BACKEND"
k2 = compute.parallel ((h : ParK) => {
  i = compute.range h.n
  v = compute.read ((compute.Read _)(.from h.in.b, .at i))
  compute.write ((compute.Write _)(.to h.out.w, .at i, .value v + v))
}) "BACKEND"
InG  = struct<.b (compute.Buf _)>
OutG = struct<.unused (compute.Buf _)>
ParG = compute.P (compute.KT _)(.I InG, .O OutG)
grow = (g : ParG) => {
  a = (compute.plrun k1 ((compute.A InK)(.n g.n, .I InK(.b g.in.b))) : OutK)
  b = (compute.plrun k2 ((compute.A InK)(.n g.n, .I InK(.b a.w))) : OutK)
  c = (compute.plrun k1 ((compute.A InK)(.n g.n, .I InK(.b b.w))) : OutK)
  compute.plrun k2 ((compute.A InK)(.n g.n, .I InK(.b c.w)))
}
built = compute.graph grow
seed = (compute.plrun kx ((compute.A InX)(.n 4, .I InX(.a 0))) : OutX)
compute.read ((compute.Read _)(.from (compute.graphrun built ((compute.A InG)(.n 4, .I InG(.b seed.z))) : (compute.Buf _)), .at 1))
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
