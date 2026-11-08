//! Probe: a tensor is a buffer, a named shape, a named pitch, and an origin.
//!
//! # Invariant
//!
//! Print-only. The programs measure the table shape, the closure selector and the contract
//! shape; the answers are recorded in `docs/notes/named-tensors.md`. The two buffer programs
//! this file used to carry went with `compute.buffer`'s removal.
//!
use lichen_compute::language::new_store;

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
sel := 'i -> (x.shape.row, x.shape.column)
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
pick := ('s : (table{Int, Int})) -> s <== 1
pick (table(table{Int, Int})(1 => 10, 2 => 20))
"#;

const SHAPE_TABLE_MISS: &str = r#"
pick := ('s : (table{Int, Int})) -> s <== 1
pick (table(table{Int, Int})(2 => 20))
"#;

/// The same selector over a concrete tensor's shape table — the call-site capture.
const SHAPE_TABLE_CAPTURE: &str = r#"
shape := table(table{string, Int})("r0" => 2, "c0" => 3)
sel := 'i -> shape <== "r0"
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
a := table(table{string, Int})("r0" => 2, "c0" => 3)
b := table(table{string, Int})("r1" => 3, "c1" => 4)
c := a @table_merge b @table_without "c0" @table_without "r1"
(c <== "r0", c <== "c1")
"#;

const CONTRACT_TYPE: &str = r#"
Tensor := 's -> {.shape s, .value @any : Int}
a := table(table{string, Int})("r0" => 2, "c0" => 3)
b := table(table{string, Int})("r1" => 3, "c1" => 4)
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

/// The same read from a selector whose parameter is only **typed**: the open measurement.
///
/// # Invariant
/// Print-only. `dimsOf` is written once for every two-pair tensor; the refusal says whether the
/// checker defers the field read to the application (`docs/notes/named-tensors.md` §5.2).
const GENERIC_SELECTOR: &str = r#"
Pair := {.row @any : Int, .column @any : Int}
Dims := {.shape Pair, .stride Pair}
dimsOf := ('d = Dims) -> (d.shape.row, d.shape.column)
x := Dims{.shape Pair{.row 2, .column 3}, .stride Pair{.row 3, .column 1}}
dimsOf x
"#;

#[test]
fn probe_a_typed_selector() {
    let mut store = new_store(None).expect("the store opens");
    match lichen_language::run::evaluate_raw(GENERIC_SELECTOR, None, &mut store) {
        Ok(text) => println!("[typed-selector] {text}"),
        Err(diagnostics) => println!(
            "[typed-selector] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// The tensor library's address function: two pitches over the same buffer.
///
/// # Invariant
/// Print-only. `tensor.lichen` is a package; the program fills one buffer and builds two
/// views of `Tensor`, then asks each for the address of the same coordinate. The two
/// answers differ and no second buffer is made (`docs/notes/named-tensors.md` §2).
const TENSOR_ADDRESS: &str = r#"
---
  compute = import "compute.lichen"
  t = import "tensor.lichen"
---
Fill := {.n @any : Int}
BufT := {.b (compute.Buf (compute.BufParam{.type Float, .length 6}))}
fill := ('f = Fill) -> {
  i := compute.range f.n
  b := compute.buf (compute.BufParam{.type Float, .length 6})
  ((BufT){.b (compute.write ((compute.Write _){.to b, .at i, .value (int2float (i + 1))}))})
}
f_kernel := compute.parallel fill "cpu" BufT
data := (compute.plrun f_kernel ((Fill){.n 6}) : Struct)
row_major := t.view2 t.Pair{.row 2, .column 3} t.Pair{.row 3, .column 1} 0 data.b
col_major := t.view2 t.Pair{.row 2, .column 3} t.Pair{.row 1, .column 2} 0 data.b
(row_major.shape.row, row_major.stride.row, row_major.origin, t.at2 row_major t.Pair{.row 1, .column 2}, t.at2 col_major t.Pair{.row 1, .column 2})
"#;

/// The same two views **inside a kernel**: reading each lane through `read2`.
///
/// # Invariant
/// Print-only. The tensor is the kernel's input at `.in.t`, so its buffer sits at
/// `.in.t.buffer`: the reading is whether the channel walk and the read agree on that
/// nested position (`docs/notes/named-tensors.md` §5.1).
const TENSOR_READ: &str = r#"
---
  compute = import "compute.lichen"
  t = import "tensor.lichen"
---
Fill := {.n @any : Int}
BufT := {.b (compute.Buf (compute.BufParam{.type Float, .length 6}))}
fill := ('f = Fill) -> {
  i := compute.range f.n
  b := compute.buf (compute.BufParam{.type Float, .length 6})
  ((BufT){.b (compute.write ((compute.Write _){.to b, .at i, .value (int2float (i + 1))}))})
}
f_kernel := compute.parallel fill "cpu" BufT
data := (compute.plrun f_kernel ((Fill){.n 6}) : Struct)
ReadIn := {.t t.Tensor}
ReadPar := {.n @any : Int, .in ReadIn}
ReadOut := {.o (compute.Buf (compute.BufParam{.type Float, .length 6}))}
read_body := ('p = ReadPar) -> {
  i := compute.range p.n
  row := i / 3
  col := i - row * 3
  v := t.read2 p.in.t (t.Pair{.row row, .column col})
  o := compute.buf (compute.BufParam{.type Float, .length 6})
  ((ReadOut){.o (compute.write ((compute.Write _){.to o, .at i, .value v}))})
}
k := compute.parallel read_body "cpu" ReadOut
row_major := t.view2 t.Pair{.row 2, .column 3} t.Pair{.row 3, .column 1} 0 data.b
col_major := t.view2 t.Pair{.row 2, .column 3} t.Pair{.row 1, .column 2} 0 data.b
out_row := (compute.plrun k ((ReadPar){.n 6, .in ReadIn{.t row_major}}) : Struct)
out_col := (compute.plrun k ((ReadPar){.n 6, .in ReadIn{.t col_major}}) : Struct)
(compute.collect out_row.o, compute.collect out_col.o)
"#;

/// The imported (frozen) `Tensor` as a parameter type and as a literal's type.
///
/// # Invariant
/// Print-only. The function is the program's own and only the types cross the package boundary,
/// so the reading separates a frozen type's nominal identity from the library's functions
/// (`docs/notes/named-tensors.md` §7.2).
const FROZEN_TYPE: &str = r#"
---
  compute = import "compute.lichen"
  t = import "tensor.lichen"
---
Fill := {.n @any : Int}
BufT := {.b (compute.Buf (compute.BufParam{.type Float, .length 6}))}
fill := ('f = Fill) -> {
  i := compute.range f.n
  b := compute.buf (compute.BufParam{.type Float, .length 6})
  ((BufT){.b (compute.write ((compute.Write _){.to b, .at i, .value (int2float (i + 1))}))})
}
f_kernel := compute.parallel fill "cpu" BufT
data := (compute.plrun f_kernel ((Fill){.n 6}) : Struct)
show_shape := 'x = t.Tensor -> x.shape
show_origin := 'x = t.Tensor -> x.origin
show_row := 'x = t.Tensor -> x.shape.row
show_id := 'x = t.Tensor -> x
plain_id := 'x -> x
show_pair := 'p = t.Pair -> p
lit := t.Tensor{.buffer data.b, .shape t.Pair{.row 2, .column 3}, .stride t.Pair{.row 3, .column 1}, .origin 0}
(show_shape lit, show_origin lit, show_row lit, lit.shape.row, show_id lit, plain_id lit, show_pair t.Pair{.row 4, .column 5})
"#;

#[test]
fn probe_the_tensor_library() {
    let mut store = new_store(None).expect("the store opens");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tensor.lichen");
    if let Err(diagnostics) = store.load_package(&path) {
        println!(
            "[tensor] library refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        );
        return;
    }
    for (label, program) in [
        ("address", TENSOR_ADDRESS),
        ("read", TENSOR_READ),
        ("frozen-type", FROZEN_TYPE),
    ] {
        match lichen_language::run::evaluate_raw(program, path.parent(), &mut store) {
            Ok(text) => println!("[tensor] {label}: {text}"),
            Err(diagnostics) => println!(
                "[tensor] {label} refused: {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                    .collect::<Vec<_>>()
            ),
        }
    }
}

/// Why a `Tensor` literal is not its nominal type: a plain field, then a type-function one.
///
/// # Invariant
/// Print-only. `Wrap` holds a named struct field and `Box` holds a field whose type is a type
/// function applied to an argument; the readings separate the nominal type rule from the
/// parametric field (`docs/notes/named-tensors.md` §7.2).
const PLAIN_WRAP: &str = r#"
Pair := {.row @any : Int, .column @any : Int}
Wrap := {.p Pair}
read := 'x = Wrap -> x.p.row
read Wrap{.p Pair{.row 1, .column 2}}
"#;

const BOXED_WRAP: &str = r#"
Pair := {.row @any : Int, .column @any : Int}
Box := 'x = Pair -> {.inner x}
B := Box Pair
read := 'x = B -> x.inner.row
read B{.inner Pair{.row 1, .column 2}}
"#;

/// The tensor's own field type, concrete and open: which one loses the nominal type.
///
/// # Invariant
/// Print-only. `T6` states the buffer's length, `TOpen` leaves it open; both are read through
/// a parameter typed by the struct, so the reading is the length's effect on the nominal type.
const TENSOR6: &str = r#"
---
  compute = import "compute.lichen"
---
Fill := {.n @any : Int}
BufT := {.b (compute.Buf (compute.BufParam{.type Float, .length 6}))}
fill := ('f = Fill) -> {
  i := compute.range f.n
  b := compute.buf (compute.BufParam{.type Float, .length 6})
  ((BufT){.b (compute.write ((compute.Write _){.to b, .at i, .value (int2float (i + 1))}))})
}
f_kernel := compute.parallel fill "cpu" BufT
data := (compute.plrun f_kernel ((Fill){.n 6}) : Struct)
Pair := {.row @any : Int, .column @any : Int}
T6 := {.buffer (compute.Buf (compute.BufParam{.type Float, .length 6})), .shape Pair, .stride Pair, .origin @any : Int}
show := 'x = T6 -> x.shape.row
show T6{.buffer data.b, .shape Pair{.row 2, .column 3}, .stride Pair{.row 3, .column 1}, .origin 0}
"#;

const TENSOR_OPEN: &str = r#"
---
  compute = import "compute.lichen"
---
Fill := {.n @any : Int}
BufT := {.b (compute.Buf (compute.BufParam{.type Float, .length 6}))}
fill := ('f = Fill) -> {
  i := compute.range f.n
  b := compute.buf (compute.BufParam{.type Float, .length 6})
  ((BufT){.b (compute.write ((compute.Write _){.to b, .at i, .value (int2float (i + 1))}))})
}
f_kernel := compute.parallel fill "cpu" BufT
data := (compute.plrun f_kernel ((Fill){.n 6}) : Struct)
Pair := {.row @any : Int, .column @any : Int}
TOpen := {.buffer (compute.Buf (compute.BufParam{.type Float, .length _})), .shape Pair, .stride Pair, .origin @any : Int}
show := 'x = TOpen -> x.shape.row
show TOpen{.buffer data.b, .shape Pair{.row 2, .column 3}, .stride Pair{.row 3, .column 1}, .origin 0}
"#;

#[test]
fn probe_a_parametric_field_type() {
    for (label, program) in [
        ("plain", PLAIN_WRAP),
        ("boxed", BOXED_WRAP),
        ("tensor6", TENSOR6),
        ("open", TENSOR_OPEN),
    ] {
        let mut store = new_store(None).expect("the store opens");
        match lichen_language::run::evaluate_raw(program, None, &mut store) {
            Ok(text) => println!("[parametric] {label}: {text}"),
            Err(diagnostics) => println!(
                "[parametric] {label} refused: {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                    .collect::<Vec<_>>()
            ),
        }
    }
}
