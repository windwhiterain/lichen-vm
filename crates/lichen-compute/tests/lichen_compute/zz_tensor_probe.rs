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
