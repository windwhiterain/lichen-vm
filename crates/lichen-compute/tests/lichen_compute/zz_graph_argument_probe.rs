//! Probe: where a graph whose step reads an argument buffer refuses, printing and asserting nothing.

use super::common;

use lichen_compute::language::{LangProgram, new_store};

const ONE_BUFFER: &str = r#"
--- compute = import "compute.lichen" ---
In2  = struct{.b (compute.Buf Int)}
Out2 = struct{.w (compute.Buf Int)}
Par2 = compute.P (compute.KT Int)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}
k2 = compute.parallel f2 "gpu"
GArg = struct{.n Int, .in In2}
step = (s : GArg) => {
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b s.in.b)))
}
built = compute.graph step
built
"#;

const SINGLE: &str = r#"
--- compute = import "compute.lichen" ---
In1  = struct{.a Int}
Out1 = struct{.z (compute.Buf _)}
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
k1 = compute.parallel f1 "gpu"
GArg = struct{.n Int, .in In1}
step = (s : GArg) => {
  compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0)))
}
built = compute.graph step
built
"#;

#[test]
fn probe_graph_argument_buffer() {
    let Some((_context, _guard)) = common::device("zz_graph_argument_probe") else {
        return;
    };
    for (name, source) in [("single", SINGLE), ("one_buffer", ONE_BUFFER)] {
        let mut store = new_store(None).expect("the embedded compute package registers");
        match lichen_language::run::evaluate_raw::<LangProgram>(source, None, &mut store) {
            Ok(rendered) => println!("[{name}] runs: {rendered}"),
            Err(diagnostics) => {
                println!("[{name}] refused at:");
                for diagnostic in &diagnostics {
                    println!("    {:?}: {}", diagnostic.span, diagnostic.message);
                }
            }
        }
    }
}
