//! Three graph programs on a device: one input, and two of unequal length.
//! See `docs/notes/llm-operators.md`.

use lichen_compute::language::new_store;

fn run(source: &str) -> Result<String, Vec<String>> {
    let mut store = new_store(None).expect("the embedded compute package registers");
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .map_err(|diagnostics| diagnostics.into_iter().map(|d| d.message).collect())
}

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

const MATMUL: &str = r#"
--- compute = import "compute.lichen" ---
InG  = struct{.a (compute.Buf Float), .b (compute.Buf Float)}
OutG = struct{.c (compute.Buf Float)}
Par  = struct{.n Int, .cols Int, .in InG, .out OutG}
ArgK = struct{.n Int, .cols Int, .I InG}
matmul_raw = (k : Par) => {
  i = compute.range k.n
  r = compute.row k
  c = compute.col k
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.a, .at (r * 3 + (s(0) - 1)))) * compute.read ((compute.Read _)(.from k.in.b, .at ((s(0) - 1) * 3 + c))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.c, .at i, .value (fold (3, 0.0 ~ "identity"))))
}
kg = compute.parallel (matmul_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
step = (g : Par) => compute.plrun kg ((ArgK)(.n g.n, .cols g.cols, .I InG(.a g.in.a, .b g.in.b)))
built = compute.graph step
a = (compute.buffer ([1.0, 2.0, 3.0, 4.0, 5.0, 6.0] : array{Float, 6}) : (compute.Buf Float))
b = (compute.buffer ([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0] : array{Float, 9}) : (compute.Buf Float))
out = (compute.graphrun built (6, 3, a, b) : OutG)
compute.collect out.c : array{Float, 6}
"#;

const ONE_BUFFER: &str = r#"
--- compute = import "compute.lichen" ---
In2  = struct{.b (compute.Buf Float)}
Out2 = struct{.w (compute.Buf Float)}
Par2 = compute.P (compute.KT Float)(.I In2, .O Out2)
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
src = (compute.buffer ([1.0, 2.0, 3.0] : array{Float, 3}) : (compute.Buf Float))
compute.collect (compute.graphrun built (3, src))
"#;

const UNUSED_BUFFER: &str = r#"
--- compute = import "compute.lichen" ---
In2  = struct{.b (compute.Buf Int)}
Out2 = struct{.w (compute.Buf Int)}
Par2 = compute.P (compute.KT Int)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value i + 10))
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
fn main() {
    if let Err(reason) = lichen_compute_gpu::install_default() {
        println!("no device: {reason}");
        return;
    }
    let only: Option<String> = std::env::args().nth(1);
    for (name, template) in [
        ("single", SINGLE),
        ("one_buffer", ONE_BUFFER),
        ("unused_buffer", UNUSED_BUFFER),
        ("gemv", GEMV),
        ("matmul", MATMUL),
    ] {
        if only.as_deref().is_some_and(|wanted| wanted != name) {
            continue;
        }
        print!("[gpu] {name}: ");
        match run(template) {
            Ok(answer) => println!("OK {answer}"),
            Err(diagnostics) => {
                println!("REFUSED");
                for message in diagnostics {
                    println!("    {message}");
                }
            }
        }
    }
    lichen_compute_gpu::uninstall();
}
