//! Probe: where a graph whose step reads an argument buffer refuses, printing and asserting nothing.

use super::common;

/// What the compile recorded: its diagnostics, and nothing a run answered.
fn recorded(source: &str) -> String {
    let report = common::check(source);
    format!(
        "ok={} {}",
        report.ok(),
        report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<String>>()
            .join(" | ")
    )
}

const SINGLE: &str = r#"
--- compute = import "compute.lichen" ---
In1  := _x => {.a @any : Int}
Out1 := _x => {.z (compute.Buf _)}
Par1 := _x => compute.P compute.KT{.I In1 _, .O Out1 _}
f1 := (k <: Par1) => {
  i := compute.range k.n
  compute.write (compute.Write{.to k.out.z, .at i, .value i + 10})
}
k1 := compute.parallel f1 "gpu" (Out1 _)
GArg := _x => {.n @any : Int, .in In1 _}
step := (s <: GArg) => {
  compute.plrun k1 (compute.A{.n s.n, .I In1{.a 0}})
}
built := compute.graph step
built
"#;

const SINGLE_COLLECT: &str = r#"
--- compute = import "compute.lichen" ---
In1  := _x => {.a @any : Int}
Out1 := _x => {.z (compute.Buf _)}
Par1 := _x => compute.P compute.KT{.I In1 _, .O Out1 _}
f1 := (k <: Par1) => {
  i := compute.range k.n
  compute.write (compute.Write{.to k.out.z, .at i, .value i + 10})
}
k1 := compute.parallel f1 "gpu" (Out1 _)
GArg := _x => {.n @any : Int, .in In1 _}
step := (s <: GArg) => {
  compute.plrun k1 (compute.A{.n s.n, .I In1{.a 0}})
}
built := compute.graph step
compute.collect (compute.graphrun built 3)
"#;

const ONE_BUFFER: &str = r#"
--- compute = import "compute.lichen" ---
In2  := _x => {.b (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Out2 := _x => {.w (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Par2 := _x => compute.P compute.KT{.I In2 _, .O Out2 _}
f2 := (k <: Par2) => {
  i := compute.range k.n
  a := (compute.read (compute.Read{.from k.in.b, .at i}) : Int)
  compute.write (compute.Write{.to k.out.w, .at i, .value a + a})
}
k2 := compute.parallel f2 "gpu" (Out2 _)
GArg := _x => {.n @any : Int, .in In2 _}
step := (s <: GArg) => {
  compute.plrun k2 (compute.A{.n s.n, .I In2{.b s.in.b}})
}
built := compute.graph step
built
"#;

const ONE_BUFFER_COLLECT: &str = r#"
--- compute = import "compute.lichen" ---
In2  := _x => {.b (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Out2 := _x => {.w (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Par2 := _x => compute.P compute.KT{.I In2 _, .O Out2 _}
f2 := (k <: Par2) => {
  i := compute.range k.n
  a := (compute.read (compute.Read{.from k.in.b, .at i}) : Int)
  compute.write (compute.Write{.to k.out.w, .at i, .value a + a})
}
k2 := compute.parallel f2 "gpu" (Out2 _)
GArg := _x => {.n @any : Int, .in In2 _}
step := (s <: GArg) => {
  compute.plrun k2 (compute.A{.n s.n, .I In2{.b s.in.b}})
}
built := compute.graph step
src := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
compute.collect (compute.graphrun built (3, src))
"#;

const EXAMPLE_SINGLE: &str = r#"
--- compute = import "compute.lichen" ---
In1  := _x => {.a @any : Int}
Out1 := _x => {.z (compute.Buf _)}
Par1 := _x => compute.P compute.KT{.I In1 _, .O Out1 _}
f1 := (k <: Par1) => {
  i := compute.range k.n
  compute.write (compute.Write{.to k.out.z, .at i, .value i + 10})
}
k1 := compute.parallel f1 "gpu" (Out1 _)
In2  := _x => {.b (compute.Buf _)}
Out2 := _x => {.w (compute.Buf _)}
Par2 := _x => compute.P compute.KT{.I In2 _, .O Out2 _}
f2 := (k <: Par2) => {
  i := compute.range k.n
  a := (compute.read (compute.Read{.from k.in.b, .at i}) : Int)
  compute.write (compute.Write{.to k.out.w, .at i, .value a + a})
}
k2 := compute.parallel f2 "gpu" (Out2 _)
GArg := _x => {.n @any : Int, .in In1 _}
step := (s <: GArg) => {
  first := (compute.plrun k1 (compute.A{.n s.n, .I In1{.a 0}}) : Struct)
  compute.plrun k2 (compute.A{.n s.n, .I In2{.b first.z}})
}
built := compute.graph step
compute.collect (compute.graphrun built 3)
"#;
const ONE_BUFFER_CLAIM: &str = r#"
--- compute = import "compute.lichen" ---
In2  := _x => {.b (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Out2 := _x => {.w (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Par2 := _x => compute.P compute.KT{.I In2 _, .O Out2 _}
f2 := (k <: Par2) => {
  i := compute.range k.n
  a := (compute.read (compute.Read{.from k.in.b, .at i}) : Int)
  compute.write (compute.Write{.to k.out.w, .at i, .value ((a + a) ~ "element")})
}
k2 := compute.parallel (f2 ~ "claim:Elementwise(element)") "gpu" (Out2 _)
GArg := _x => {.n @any : Int, .in In2 _}
step := (s <: GArg) => {
  compute.plrun k2 (compute.A{.n s.n, .I In2{.b s.in.b}})
}
built := compute.graph step
src := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
compute.collect (compute.graphrun built (3, src))
"#;
const ARG_BUFFER_TWICE: &str = r#"
--- compute = import "compute.lichen" ---
In2  := _x => {.b (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Out2 := _x => {.w (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Par2 := _x => compute.P compute.KT{.I In2 _, .O Out2 _}
f2 := (k <: Par2) => {
  i := compute.range k.n
  a := (compute.read (compute.Read{.from k.in.b, .at i}) : Int)
  compute.write (compute.Write{.to k.out.w, .at i, .value a + a})
}
k2 := compute.parallel f2 "gpu" (Out2 _)
GArg := _x => {.n @any : Int, .in In2 _}
step := (s <: GArg) => {
  first := (compute.plrun k2 (compute.A{.n s.n, .I In2{.b s.in.b}}) : Struct)
  compute.plrun k2 (compute.A{.n s.n, .I In2{.b first.w}})
}
built := compute.graph step
src := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
compute.collect (compute.graphrun built (3, src))
"#;
const ONE_BUFFER_ANNOTATED: &str = r#"
--- compute = import "compute.lichen" ---
In2  := _x => {.b (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Out2 := _x => {.w (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Par2 := _x => compute.P compute.KT{.I In2 _, .O Out2 _}
f2 := (k <: Par2) => {
  i := compute.range k.n
  a := (compute.read (compute.Read{.from k.in.b, .at i}) : Int)
  compute.write (compute.Write{.to k.out.w, .at i, .value a + a})
}
k2 := compute.parallel f2 "gpu" (Out2 _)
GArg := _x => {.n @any : Int, .in In2 _}
step := (s <: GArg) => {
  compute.plrun k2 (compute.A{.n s.n, .I In2{.b s.in.b}}) : Struct
}
built := compute.graph step
src := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
compute.collect (compute.graphrun built (3, src))
"#;
const GEMV: &str = r#"
--- compute = import "compute.lichen" ---
InG  := _x => {.w (compute.Buf (compute.BufParam{{.type Float, .length _}})), .x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
OutG := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Par  := _x => {.n @any : Int, .kdim @any : Int, .in InG _, .out OutG}
ArgK := _x => {.n @any : Int, .kdim @any : Int, .I InG _}
gemv_raw := (k <: Par) => {
  i := compute.range k.n
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read (compute.Read{.from k.in.w, .at (i * k.kdim + (s(0) - 1))}) : Float) * (compute.read (compute.Read{.from k.in.x, .at (s(0) - 1)}) : Float))) ~ "combine" ~ "element"))
  compute.write (compute.Write{.to k.out.y, .at i, .value (fold (k.kdim, 0.0 ~ "identity"))})
}
kg := compute.parallel (gemv_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (OutG _)
step := (g <: Par) => (compute.plrun kg ((ArgK){.n g.n, .kdim g.kdim, .I InG{.w g.in.w, .x g.in.x}}) : Struct)
built := compute.graph step
w := (compute.buffer ([@any : Float, 12][1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
x := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
out := (compute.graphrun built (4, 3, w, x) : Struct)
compute.collect out.y : Array
"#;
const GEMV_NO_FOLD: &str = r#"
--- compute = import "compute.lichen" ---
InG  := _x => {.w (compute.Buf (compute.BufParam{{.type Float, .length _}})), .x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
OutG := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Par  := _x => {.n @any : Int, .in InG _, .out OutG}
ArgK := _x => {.n @any : Int, .I InG _}
f := (k <: Par) => {
  i := compute.range k.n
  y := ((compute.read (compute.Read{.from k.in.w, .at (i * 3 + 0)}) : Float) * (compute.read (compute.Read{.from k.in.x, .at 0}) : Float)) + ((compute.read (compute.Read{.from k.in.w, .at (i * 3 + 1)}) : Float) * (compute.read (compute.Read{.from k.in.x, .at 1}) : Float)) + ((compute.read (compute.Read{.from k.in.w, .at (i * 3 + 2)}) : Float) * (compute.read (compute.Read{.from k.in.x, .at 2}) : Float))
  compute.write (compute.Write{.to k.out.y, .at i, .value y})
}
kg := compute.parallel f "gpu" (OutG _)
step := (g <: Par) => (compute.plrun kg ((ArgK){.n g.n, .I InG{.w g.in.w, .x g.in.x}}) : Struct)
built := compute.graph step
w := (compute.buffer ([@any : Float, 12][1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
x := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
out := (compute.graphrun built (4, w, x) : Struct)
compute.collect out.y : Array
"#;
const TWO_BUFFERS_ANNOTATED: &str = r#"
--- compute = import "compute.lichen" ---
In2  := _x => {.b (compute.Buf (compute.BufParam{{.type Int, .length _}})), .c (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Out2 := _x => {.w (compute.Buf (compute.BufParam{{.type Int, .length _}}))}
Par2 := _x => compute.P compute.KT{.I In2 _, .O Out2 _}
f2 := (k <: Par2) => {
  i := compute.range k.n
  x := (compute.read (compute.Read{.from k.in.b, .at i}) : Int)
  y := (compute.read (compute.Read{.from k.in.c, .at i}) : Int)
  compute.write (compute.Write{.to k.out.w, .at i, .value x + y})
}
k2 := compute.parallel f2 "gpu" (Out2 _)
GArg := _x => {.n @any : Int, .in In2 _}
step := (s <: GArg) => {
  compute.plrun k2 (compute.A{.n s.n, .I In2{.b s.in.b, .c s.in.c}}) : Struct
}
built := compute.graph step
p := (compute.buffer ([@any : Int, 3][1, 2, 3] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
q := (compute.buffer ([@any : Int, 3][10, 20, 30] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
compute.collect (compute.graphrun built (3, p, q))
"#;
const TWO_BUFFERS_FLOAT: &str = r#"
--- compute = import "compute.lichen" ---
In2  := _x => {.b (compute.Buf (compute.BufParam{{.type Float, .length _}})), .c (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Out2 := _x => {.w (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Par2 := _x => compute.P compute.KT{.I In2 _, .O Out2 _}
f2 := (k <: Par2) => {
  i := compute.range k.n
  x := (compute.read (compute.Read{.from k.in.b, .at i}) : Float)
  y := (compute.read (compute.Read{.from k.in.c, .at i}) : Float)
  compute.write (compute.Write{.to k.out.w, .at i, .value x + y})
}
k2 := compute.parallel f2 "gpu" (Out2 _)
GArg := _x => {.n @any : Int, .in In2 _}
step := (s <: GArg) => (compute.plrun k2 (compute.A{.n s.n, .I In2{.b s.in.b, .c s.in.c}}) : Struct)
built := compute.graph step
p := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
q := (compute.buffer ([@any : Float, 3][10.0, 20.0, 30.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
compute.collect (compute.graphrun built (3, p, q))
"#;
const GEMV_P_A: &str = r#"
--- compute = import "compute.lichen" ---
InG  := _x => {.w (compute.Buf (compute.BufParam{{.type Float, .length _}})), .x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
OutG := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Par  := _x => compute.P compute.KT{.I InG _, .O OutG _}
f := (k <: Par) => {
  i := compute.range k.n
  a := (compute.read (compute.Read{.from k.in.w, .at i}) : Float)
  b := (compute.read (compute.Read{.from k.in.x, .at i}) : Float)
  compute.write (compute.Write{.to k.out.y, .at i, .value a + b})
}
kg := compute.parallel f "gpu" (OutG _)
GArg := _x => {.n @any : Int, .in InG _}
step := (g <: GArg) => (compute.plrun kg (compute.A{.n g.n, .I InG{.w g.in.w, .x g.in.x}}) : Struct)
built := compute.graph step
w := (compute.buffer ([@any : Float, 4][1.0, 2.0, 3.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
x := (compute.buffer ([@any : Float, 4][1.0, 2.0, 3.0, 4.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
out := (compute.graphrun built (4, w, x) : Struct)
compute.collect out.y : Array
"#;
const TWO_BUFFERS_FIELD: &str = r#"
--- compute = import "compute.lichen" ---
In2  := _x => {.b (compute.Buf (compute.BufParam{{.type Float, .length _}})), .c (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Out2 := _x => {.w (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Par2 := _x => compute.P compute.KT{.I In2 _, .O Out2 _}
f2 := (k <: Par2) => {
  i := compute.range k.n
  x := (compute.read (compute.Read{.from k.in.b, .at i}) : Float)
  y := (compute.read (compute.Read{.from k.in.c, .at i}) : Float)
  compute.write (compute.Write{.to k.out.w, .at i, .value x + y})
}
k2 := compute.parallel f2 "gpu" (Out2 _)
GArg := _x => {.n @any : Int, .in In2 _}
step := (s <: GArg) => (compute.plrun k2 (compute.A{.n s.n, .I In2{.b s.in.b, .c s.in.c}}) : Struct)
built := compute.graph step
p := (compute.buffer ([@any : Float, 3][1.0, 2.0, 3.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
q := (compute.buffer ([@any : Float, 3][10.0, 20.0, 30.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
out := (compute.graphrun built (3, p, q) : Struct)
compute.collect out.w
"#;
const GRAPH_OF_A_CONSTANT: &str = r#"
--- compute = import "compute.lichen" ---
step := (n : Int) => n + 1
built := compute.graph step
built
"#;
#[test]
fn probe_graph_argument_buffer() {
    for (name, source) in [
        ("graph_of_a_constant", GRAPH_OF_A_CONSTANT),
        ("two_buffers_float", TWO_BUFFERS_FLOAT),
        ("two_buffers_annotated", TWO_BUFFERS_ANNOTATED),
        ("gemv_no_fold", GEMV_NO_FOLD),
        ("gemv_graph", GEMV),
        ("one_buffer_annotated", ONE_BUFFER_ANNOTATED),
        ("arg_buffer_twice", ARG_BUFFER_TWICE),
        ("one_buffer_claim", ONE_BUFFER_CLAIM),
        ("example_single", EXAMPLE_SINGLE),
        ("gemv_p_a", GEMV_P_A),
        ("two_buffers_field", TWO_BUFFERS_FIELD),
        ("single", SINGLE),
        ("single_collect", SINGLE_COLLECT),
        ("one_buffer", ONE_BUFFER),
        ("one_buffer_collect", ONE_BUFFER_COLLECT),
    ] {
        println!("[{name}] {}", recorded(source));
    }
}
