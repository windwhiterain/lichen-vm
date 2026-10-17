//! Probe: where a graph whose step reads an argument buffer refuses, printing and asserting nothing.

use super::common;

use lichen_compute::language::{LangProgram, new_store};

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

const SINGLE_COLLECT: &str = r#"
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
compute.collect (compute.graphrun built 3)
"#;

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

const ONE_BUFFER_COLLECT: &str = r#"
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
src = (compute.buffer ([1, 2, 3] : array{Int, 3}) : (compute.Buf Int))
compute.collect (compute.graphrun built (3, src))
"#;

const EXAMPLE_SINGLE: &str = r#"
--- compute = import "compute.lichen" ---
In1  = struct{.a Int}
Out1 = struct{.z (compute.Buf _)}
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
k1 = compute.parallel f1 "gpu"
In2  = struct{.b (compute.Buf _)}
Out2 = struct{.w (compute.Buf _)}
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}
k2 = compute.parallel f2 "gpu"
GArg = struct{.n Int, .in In1}
step = (s : GArg) => {
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.z)))
}
built = compute.graph step
compute.collect (compute.graphrun built 3)
"#;
const ONE_BUFFER_CLAIM: &str = r#"
--- compute = import "compute.lichen" ---
In2  = struct{.b (compute.Buf Int)}
Out2 = struct{.w (compute.Buf Int)}
Par2 = compute.P (compute.KT Int)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value ((a + a) ~ "element")))
}
k2 = compute.parallel (f2 ~ "claim:Elementwise(element)") "gpu"
GArg = struct{.n Int, .in In2}
step = (s : GArg) => {
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b s.in.b)))
}
built = compute.graph step
src = (compute.buffer ([1, 2, 3] : array{Int, 3}) : (compute.Buf Int))
compute.collect (compute.graphrun built (3, src))
"#;
const ARG_BUFFER_TWICE: &str = r#"
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
  first = (compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b s.in.b))) : Out2)
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.w)))
}
built = compute.graph step
src = (compute.buffer ([1, 2, 3] : array{Int, 3}) : (compute.Buf Int))
compute.collect (compute.graphrun built (3, src))
"#;
const ONE_BUFFER_ANNOTATED: &str = r#"
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
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b s.in.b))) : Out2
}
built = compute.graph step
src = (compute.buffer ([1, 2, 3] : array{Int, 3}) : (compute.Buf Int))
compute.collect (compute.graphrun built (3, src))
"#;
const GEMV: &str = r#"
--- compute = import "compute.lichen" ---
InG  = struct{.w (compute.Buf Float), .x (compute.Buf Float)}
OutG = struct{.y (compute.Buf Float)}
Par  = struct{.n Int, .kdim Int, .in InG, .out OutG}
ArgK = struct{.n Int, .kdim Int, .I InG}
gemv_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (i * k.kdim + (s(0) - 1)))) * compute.read ((compute.Read _)(.from k.in.x, .at (s(0) - 1))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (fold (k.kdim, 0.0 ~ "identity"))))
}
kg = compute.parallel (gemv_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
step = (g : Par) => (compute.plrun kg ((ArgK)(.n g.n, .kdim g.kdim, .I InG(.w g.in.w, .x g.in.x))) : OutG)
built = compute.graph step
w = (compute.buffer ([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0] : array{Float, 12}) : (compute.Buf Float))
x = (compute.buffer ([1.0, 2.0, 3.0] : array{Float, 3}) : (compute.Buf Float))
out = (compute.graphrun built (4, 3, w, x) : OutG)
compute.collect out.y : array{Float, 4}
"#;
const GEMV_NO_FOLD: &str = r#"
--- compute = import "compute.lichen" ---
InG  = struct{.w (compute.Buf Float), .x (compute.Buf Float)}
OutG = struct{.y (compute.Buf Float)}
Par  = struct{.n Int, .in InG, .out OutG}
ArgK = struct{.n Int, .I InG}
f = (k : Par) => {
  i = compute.range k.n
  y = (compute.read ((compute.Read _)(.from k.in.w, .at (i * 3 + 0))) * compute.read ((compute.Read _)(.from k.in.x, .at 0)))
    + (compute.read ((compute.Read _)(.from k.in.w, .at (i * 3 + 1))) * compute.read ((compute.Read _)(.from k.in.x, .at 1)))
    + (compute.read ((compute.Read _)(.from k.in.w, .at (i * 3 + 2))) * compute.read ((compute.Read _)(.from k.in.x, .at 2)))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value y))
}
kg = compute.parallel f "gpu"
step = (g : Par) => (compute.plrun kg ((ArgK)(.n g.n, .I InG(.w g.in.w, .x g.in.x))) : OutG)
built = compute.graph step
w = (compute.buffer ([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0] : array{Float, 12}) : (compute.Buf Float))
x = (compute.buffer ([1.0, 2.0, 3.0] : array{Float, 3}) : (compute.Buf Float))
out = (compute.graphrun built (4, w, x) : OutG)
compute.collect out.y : array{Float, 4}
"#;
#[test]
fn probe_graph_argument_buffer() {
    let Some((_context, _guard)) = common::device("zz_graph_argument_probe") else {
        return;
    };
    for (name, source) in [
        ("gemv_no_fold", GEMV_NO_FOLD),
        ("gemv_graph", GEMV),
        ("one_buffer_annotated", ONE_BUFFER_ANNOTATED),
        ("arg_buffer_twice", ARG_BUFFER_TWICE),
        ("one_buffer_claim", ONE_BUFFER_CLAIM),
        ("example_single", EXAMPLE_SINGLE),
        ("single", SINGLE),
        ("single_collect", SINGLE_COLLECT),
        ("one_buffer", ONE_BUFFER),
        ("one_buffer_collect", ONE_BUFFER_COLLECT),
    ] {
        let mut store = new_store(None).expect("the embedded compute package registers");
        match lichen_language::run::evaluate_raw::<LangProgram>(source, None, &mut store) {
            Ok(rendered) => println!("[{name}] runs: {rendered}"),
            Err(diagnostics) => {
                println!("[{name}] refused:");
                for diagnostic in &diagnostics {
                    println!("    {diagnostic:?}");
                }
            }
        }
    }
}
