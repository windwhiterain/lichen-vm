//! Probe: a tensor is a buffer, a named shape, a named pitch, and an origin.
//!
//! # Invariant
//!
//! Print-only. The programs measure a bare-template tensor, a per-use constructor, the
//! table shape, and the closure selector; the answers and refusals are recorded in
//! `docs/notes/named-tensors.md`.
//!
use super::common;

use lichen_compute::language::new_store;

/// Two buffers of different lengths, the type's length open.
const TWO_BUFFERS: &str = r#"
--- compute = import "compute.lichen" ---
In  := {.w (compute.Buf (compute.BufParam{.type Int, .length _})), .x (compute.Buf (compute.BufParam{.type Int, .length _}))}
Out := {.z (compute.Buf (compute.BufParam{.type Int, .length 3}))}
Par := {.n @any : Int, .in In}
Arg := {.n @any : Int, .I In}
w := (compute.buffer ([@any : Int, 6][1, 2, 3, 4, 5, 6] : Array) : (compute.Buf (compute.BufParam{.type Int, .length 6})))
x := (compute.buffer ([@any : Int, 3][10, 20, 30] : Array) : (compute.Buf (compute.BufParam{.type Int, .length 3})))
body := ('k = Par) => {
  i := compute.range k.n
  a := (compute.read ((compute.Read _){.from k.in.w, .at i}) : Int)
  b := (compute.read ((compute.Read _){.from k.in.x, .at i}) : Int)
  z := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((Out){.z (compute.write ((compute.Write _){.to z, .at i, .value (a + b)}))})
}
k := compute.parallel body "cpu" Out
out := (compute.plrun k ((Arg){.n 3, .I In{.w w, .x x}}) : Struct)
compute.collect out.z : Array
"#;

/// The split matvec: two buffers and two named descriptors, a fold over `w`'s count.
const MATVEC: &str = r#"
--- compute = import "compute.lichen" ---
Pair := {.row @any : Int, .column @any : Int}
Desc := {.shape Pair, .stride Pair, .origin @any : Int}
In   := {.w (compute.Buf (compute.BufParam{.type Int, .length _})), .x (compute.Buf (compute.BufParam{.type Int, .length _})), .wd Desc, .xd Desc}
Out  := {.z (compute.Buf (compute.BufParam{.type Int, .length 2}))}
Par  := {.n @any : Int, .in In}
Arg  := {.n @any : Int, .I In}
w := (compute.buffer ([@any : Int, 6][1, 2, 3, 4, 5, 6] : Array) : (compute.Buf (compute.BufParam{.type Int, .length 6})))
x := (compute.buffer ([@any : Int, 3][1, 1, 1] : Array) : (compute.Buf (compute.BufParam{.type Int, .length 3})))
body := ('k = Par) => {
  i := compute.range k.n
  wb := k.in.w
  xb := k.in.x
  wd := k.in.wd
  xd := k.in.xd
  wsr := wd.stride.row
  wsc := wd.stride.column
  xsc := xd.stride.column
  wc := wd.shape.column
  @loop fold := 's => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (((compute.read ((compute.Read _){.from wb, .at (i * wsr + (s(0) - 1) * wsc)})) : Int) * ((compute.read ((compute.Read _){.from xb, .at ((s(0) - 1) * xsc)})) : Int))))
  z := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((Out){.z (compute.write ((compute.Write _){.to z, .at i, .value (fold (wc, 0))}))})
}
k := compute.parallel body "cpu" Out
wv := Desc{.shape Pair{.row 2, .column 3}, .stride Pair{.row 3, .column 1}, .origin 0}
xv := Desc{.shape Pair{.row 1, .column 3}, .stride Pair{.row 0, .column 1}, .origin 0}
out := (compute.plrun k ((Arg){.n 2, .I In{.w w, .x x, .wd wv, .xd xv}}) : Struct)
compute.collect out.z : Array
"#;

/// What a program renders, or the diagnostics that refused it.
fn rendered(body: &str) -> String {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(body, None, &mut store) {
        Ok(text) => text,
        Err(diagnostics) => format!(
            "refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_named_tensor_parameter() {
    println!("[two open buffers] {}", rendered(TWO_BUFFERS));
}

#[test]
fn probe_a_named_tensor_output() {
    println!("[split matvec] {}", rendered(MATVEC));
}

/// The selector mechanism: a closure reads the names, the caller indexes the result.
///
/// # Invariant
/// Print-only. The selector is the **only** place a dimension name appears; the consumer
/// (`take`) indexes the returned list with a runtime ordinal, so the algorithm is positional.
/// If this answers `(1, 3)`, a name-agnostic operator can be written as ordinary lichen:
/// names at the selector, data at the algorithm.
const SELECTOR: &str = r#"
Pair := {.row @any : Int, .column @any : Int}
Dims := {.shape Pair, .stride Pair}
x := Dims{.shape Pair{.row 2, .column 3}, .stride Pair{.row 3, .column 1}}
sel := 'i => (x.shape.row, x.shape.column)
(sel 0)(1)
"#;

#[test]
fn probe_a_selector_closure() {
    let mut store = new_store(None).expect("the store opens");
    match lichen_language::run::evaluate_raw(SELECTOR, None, &mut store) {
        Ok(text) => println!("[selector] {text}"),
        Err(diagnostics) => println!(
            "[selector] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        ),
    }
}

/// The table route's three semantic questions, in the current spelling.
///
/// # Invariant
/// Print-only. `pick` is generic over the shape table (an annotated parameter, no concrete
/// keys); the first program applies it to a table holding the key, the second to one that
/// does not, and the third only defines it. A determined `TableGet` is supposed to be
/// evaluated at the use; a miss is `EvalError::TableMiss`, not "undecided".
const SHAPE_TABLE: &str = r#"
pick := ('s : (table{Int, Int})) => s <== 1
pick (table(1 ==> 10, 2 ==> 20))
"#;

const SHAPE_TABLE_MISS: &str = r#"
pick := ('s : (table{Int, Int})) => s <== 1
pick (table(2 ==> 20))
"#;

/// The same selector over a concrete tensor's shape table — the call-site capture.
const SHAPE_TABLE_CAPTURE: &str = r#"
shape := table("r0" ==> 2, "c0" ==> 3)
sel := 'i => shape <== "r0"
sel 0
"#;

#[test]
fn probe_a_shape_table() {
    for (label, program) in [
        ("[table hit]", SHAPE_TABLE),
        ("[table miss]", SHAPE_TABLE_MISS),
        ("[table capture]", SHAPE_TABLE_CAPTURE),
    ] {
        let mut store = new_store(None).expect("the store opens");
        match lichen_language::run::evaluate_raw(program, None, &mut store) {
            Ok(text) => println!("{label} {text}"),
            Err(diagnostics) => println!(
                "{label} refused: {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.clone())
                    .collect::<Vec<_>>()
            ),
        }
    }
}

/// The contraction's result shape, and the same shape in a **type position**.
///
/// # Invariant
/// Print-only. The first program is the result-shape arithmetic on values (merge the
/// operands, drop the contracted labels); the second puts that computed shape into a
/// type and reads a label back through the type — the `contract` result type's question.
const CONTRACT_SHAPE: &str = r#"
a := table("r0" ==> 2, "c0" ==> 3)
b := table("r1" ==> 3, "c1" ==> 4)
c := a @table_merge b @table_without "c0" @table_without "r1"
(c <== "r0", c <== "c1")
"#;

const CONTRACT_TYPE: &str = r#"
Tensor := 's => {.shape s, .value @any : Int}
a := table("r0" ==> 2, "c0" ==> 3)
b := table("r1" ==> 3, "c1" ==> 4)
shape := a @table_merge b @table_without "c0" @table_without "r1"
t := (Tensor shape){.shape shape, .value 1}
t.shape <== "r0"
"#;

#[test]
fn probe_a_contract_shape() {
    for (label, program) in [
        ("[shape value]", CONTRACT_SHAPE),
        ("[shape type]", CONTRACT_TYPE),
    ] {
        let mut store = new_store(None).expect("the store opens");
        match lichen_language::run::evaluate_raw(program, None, &mut store) {
            Ok(text) => println!("{label} {text}"),
            Err(diagnostics) => println!(
                "{label} refused: {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.clone())
                    .collect::<Vec<_>>()
            ),
        }
    }
}
