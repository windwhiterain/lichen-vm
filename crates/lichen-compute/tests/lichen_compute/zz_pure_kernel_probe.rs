//! Probe: a kernel that declares its own buffer, and a kernel-built input.
//!
//! # Invariant
//!
//! Print-only. A body declares its output with `compute.slice` and returns the codomain;
//! a host builds data by running a kernel (`docs/notes/kernel-buffer-declarations.md`).
//! Each reading is a run's answer or its refusal.

use lichen_compute::language::new_store;

use super::common;

/// The program: one kernel builds its input, another declares its output.
const PROGRAM: &str = r#"
--- compute = import "compute.lichen" ---
In  = struct{.unused (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
Out = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
Fill = struct{.n Int}
FillArg = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((Out)(.y (compute.write ((compute.Write _)(.to y, .at i, .value 1.0)))))
}
fk = compute.parallel fill "cpu" Out
filled = (compute.plrun fk ((FillArg)(.n 2)) : Out)
Par = struct{.n Int, .in In}
Arg = struct{.n Int, .I In}
raw = (c : Par) => {
  i = compute.range c.n
  x = (compute.read ((compute.Read _)(.from c.in.unused, .at i)) : Float)
  y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((Out)(.y (compute.write ((compute.Write _)(.to y, .at i, .value x)))))
}
k = compute.parallel raw "cpu" Out
out = (compute.plrun k ((Arg)(.n 2, .I In(.unused filled.y))) : Out)
compute.collect out.y : array{Float, 2}
"#;

#[test]
fn probe_a_kernel_that_declares_its_output() {
    let _slot = common::slot();
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(PROGRAM, None, &mut store) {
        Ok(text) => println!("[declared] answered {text}"),
        Err(diagnostics) => println!(
            "[declared] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}

/// The declared length is the allocation: three lanes fill a three-element buffer.
///
/// # Invariant
/// The write is the codomain position's value, which is the form the graph keeps
/// (`docs/notes/kernel-buffer-declarations.md`): a write that is only a statement is not
/// reachable from the return and does not survive the freeze.
const LENGTH: &str = r#"
--- compute = import "compute.lichen" ---
OutThree = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length 3))
  ((OutThree)(.y (compute.write ((compute.Write _)(.to y, .at i, .value (int2float i + 1.0))))))
}
fk = compute.parallel fill "cpu" OutThree
data = (compute.plrun fk ((Fill)(.n 3)) : OutThree)
compute.collect data.y : array{Float, 3}
"#;

#[test]
fn probe_a_declared_length_is_the_allocation() {
    let _slot = common::slot();
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(LENGTH, None, &mut store) {
        Ok(text) => println!("[length] answered {text}"),
        Err(diagnostics) => println!(
            "[length] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}

/// The codomain may hold a loop's result: the version a step's write returned.
///
/// # Invariant
/// The field is the fold's result, not a `compute.write` node — the state carries the
/// version the step wrote, so the pure value is the loop's, and the store is inside its
/// step (`docs/notes/kernel-buffer-declarations.md`).
const LOOPED: &str = r#"
--- compute = import "compute.lichen" ---
Out = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
LPar = struct{.n Int, .taps Int}
fill = (c : LPar) => {
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (compute.write ((compute.Write _)(.to s(1), .at i, .value (int2float s(0))))))
  ((Out)(.y (fold (c.taps, y))))
}
k = compute.parallel fill "cpu" Out
data = (compute.plrun k ((LPar)(.n 2, .taps 3)) : Out)
compute.collect data.y : array{Float, 2}
"#;

#[test]
fn probe_the_codomain_may_hold_a_loops_result() {
    let _slot = common::slot();
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(LOOPED, None, &mut store) {
        Ok(text) => println!("[looped] answered {text}"),
        Err(diagnostics) => println!(
            "[looped] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}

/// A per-lane constant selected by the index: the shape arbitrary literal data needs.
///
/// # Invariant
/// The write's value is an `if` chain, which is how a fill kernel would write a literal
/// that is not a formula of the lane index. Print-only: the reading is whether the body
/// compiles (`docs/notes/kernel-buffer-declarations.md`).
const SELECTED: &str = r#"
--- compute = import "compute.lichen" ---
Out = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length 3))
  ((Out)(.y (compute.write ((compute.Write _)(.to y, .at i, .value (if i == 0 then 1.0 else 2.0))))))
}
k = compute.parallel fill "cpu" Out
data = (compute.plrun k ((Fill)(.n 3)) : Out)
compute.collect data.y : array{Float, 3}
"#;

#[test]
fn probe_a_literal_selected_by_the_index() {
    let _slot = common::slot();
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(SELECTED, None, &mut store) {
        Ok(text) => println!("[selected] answered {text}"),
        Err(diagnostics) => println!(
            "[selected] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}
/// A **nested** `if` chain in the body: the shape a literal with three values needs.
///
/// # Invariant
/// The two-arm chain above answers; a third arm refuses with the template's own message,
/// so a fill kernel can build data of at most two distinct values until that is fixed
/// (`docs/notes/kernel-buffer-declarations.md`).
const NESTED: &str = r#"
--- compute = import "compute.lichen" ---
Out = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length 3))
  ((Out)(.y (compute.write ((compute.Write _)(.to y, .at i, .value (if i == 0 then 1.0 else (if i == 1 then 2.0 else 3.0)))))))
}
k = compute.parallel fill "cpu" Out
data = (compute.plrun k ((Fill)(.n 3)) : Out)
compute.collect data.y : array{Float, 3}
"#;

#[test]
fn probe_a_nested_chain_in_the_body() {
    let _slot = common::slot();
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(NESTED, None, &mut store) {
        Ok(text) => println!("[nested] answered {text}"),
        Err(diagnostics) => println!(
            "[nested] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}

/// Two outputs: the codomain's second field must reach the host too.
///
/// # Invariant
/// Print-only. A kernel that fills two buffers and returns both; the reading is
/// whether `collect` reaches the second field (`docs/notes/kernel-buffer-declarations.md`).
const TWO: &str = r#"
--- compute = import "compute.lichen" ---
Out = struct{.c (compute.Buf ((compute.BufParam _)(.type Int, .length 2))), .s (compute.Buf ((compute.BufParam _)(.type Int, .length 2)))}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  a = compute.buf ((compute.BufParam _)(.type Int, .length c.n))
  b = compute.buf ((compute.BufParam _)(.type Int, .length c.n))
  ((Out)(.c (compute.write ((compute.Write _)(.to a, .at i, .value i))), .s (compute.write ((compute.Write _)(.to b, .at i, .value i)))))
}
k = compute.parallel fill "cpu" Out
out = (compute.plrun k ((Fill)(.n 2)) : Out)
compute.collect out.c : array{Int, 2}
"#;

#[test]
fn probe_two_outputs() {
    let _slot = common::slot();
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(TWO, None, &mut store) {
        Ok(text) => println!("[two] answered {text}"),
        Err(diagnostics) => println!(
            "[two] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}

/// The settled spelling: a param struct, a value-indexed buffer type, and the
/// declaration call.
///
/// # Invariant
/// Print-only. It asks the front end two questions at once: does a **callee-inferred
/// instantiation** (`_(.type Float, .length 192)`) parse and check, and does a **type
/// function whose parameter is a value** (`Buf = x : (BufParam _) => …`) build a type
/// from that value's fields?
const SPELLING: &str = r#"
--- compute = import "compute.lichen" ---
Param = _x => struct{.type _, .length _}
BufT = x : (Param _) => struct{.native Int, .type x.type, .length x.length}
buf = (x : (Param _)) => (BufT x)(.native 0, .type @any, .length @any)
e = buf ((Param _)(.type Float, .length 192))
e
"#;

#[test]
fn probe_the_settled_spelling() {
    let _slot = common::slot();
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(SPELLING, None, &mut store) {
        Ok(text) => println!("[spelling] answered {text}"),
        Err(diagnostics) => println!(
            "[spelling] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}
