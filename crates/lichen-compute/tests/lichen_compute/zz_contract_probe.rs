//! Probe: a 2-D contraction over named shape tables, plan-specialized.
//!
//! # Invariant
//!
//! Print-only. `A` is 2x6 with `A[r][t] = 6r + t + 1`; `B` is 6x2 with `B[t][c] = t + 1 + c`;
//! the contraction over `c0`/`r1` leaves `r0`/`c1`, so `C` is 2x2 and answers
//! `[91, 112, 217, 274]`. Filled by a kernel, the proven `In`/`.meta` parameter, one fold.

use super::common;

use lichen_compute::language::new_store;

const PROGRAM: &str = r#"
--- compute = import "compute.lichen" ---
Meta := {.n @any : Int, .k @any : Int, .c1 @any : Int, .ar @any : Int, .ac @any : Int, .br @any : Int, .bc @any : Int}
In   := {.a (compute.Buf (compute.BufParam{.type Float, .length 12})), .b (compute.Buf (compute.BufParam{.type Float, .length 12}))}
Out  := {.c (compute.Buf (compute.BufParam{.type Float, .length 4}))}
Par  := {.meta Meta, .in In}
Arg  := {.meta Meta, .I In}
Fill := {.n @any : Int}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Float, .length 12})
  b := compute.buf (compute.BufParam{.type Float, .length 12})
  ((In){.a (compute.write ((compute.Write _){.to a, .at i, .value (int2float (i + 1))})), .b (compute.write ((compute.Write _){.to b, .at i, .value (int2float ((i / 2) + 1 + (i - (i / 2) * 2))) }))})
}
filler := compute.parallel fill "cpu" In
data := (compute.plrun filler ((Fill){.n 12}) : Struct)
body := ('p = Par) -> {
  i := compute.range p.meta.n
  row := i / p.meta.c1
  col := i - row * p.meta.c1
  @loop fold := 's -> if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read ((compute.Read _){.from p.in.a, .at (row * p.meta.ar + (s(0) - 1) * p.meta.ac)}) : Float) * (compute.read ((compute.Read _){.from p.in.b, .at ((s(0) - 1) * p.meta.br + col * p.meta.bc)}) : Float))))
  o := compute.buf (compute.BufParam{.type Float, .length p.meta.n})
  ((Out){.c (compute.write ((compute.Write _){.to o, .at i, .value (fold (p.meta.k, 0.0))}))})
}
kernel := compute.parallel body "cpu" Out
a_shape := table(table{string, Int})("r0" => 2, "c0" => 6)
b_shape := table(table{string, Int})("r1" => 6, "c1" => 2)
c_shape := a_shape @table_merge b_shape @table_without "c0" @table_without "r1"
out := (compute.plrun kernel ((Arg){.meta Meta{.n ((c_shape <== "r0") * (c_shape <== "c1")), .k (a_shape <== "c0"), .c1 (c_shape <== "c1"), .ar (a_shape <== "c0"), .ac 1, .br (b_shape <== "c1"), .bc 1}, .I In{.a data.a, .b data.b}}) : Struct)
(compute.collect out.c : Array, c_shape <== "r0", c_shape <== "c1")
"#;

#[test]
#[ignore = "probe: prints and asserts nothing; run with --ignored"]
fn probe_a_two_dimensional_contraction() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(PROGRAM, None, &mut store) {
        Ok(text) => println!("[contract] {text}"),
        Err(diagnostics) => println!(
            "[contract] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// `compute.Buf` without the `.native` field: the same application and struct literal.
///
/// # Invariant
/// Check-only. It isolates the native `$buf_type` term in `Buf`'s `.native` field from the
/// import, the module's `BufParam`, and the application, which are all still here.
const PLAIN_BUF: &str = r#"
--- compute = import "compute.lichen" ---
Box2 := 'x = compute.BufParam -> {.type x.type, .length x.length}
In := {.a (Box2 (compute.BufParam{.type Float, .length 4})), .b (Box2 (compute.BufParam{.type Float, .length 2}))}
a := (Box2 (compute.BufParam{.type Float, .length 4})){.type Float, .length 4}
b := (Box2 (compute.BufParam{.type Float, .length 2})){.type Float, .length 2}
x := (In){.a a, .b b}
(x.a.length, x.b.length)
"#;

#[test]
#[ignore = "probe: prints and asserts nothing; run with --ignored"]
fn probe_a_plain_buf_field() {
    let report = common::check(PLAIN_BUF);
    println!(
        "[plain buf] ok={} {:?}",
        report.ok(),
        report
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
            .collect::<Vec<_>>()
    );
}
