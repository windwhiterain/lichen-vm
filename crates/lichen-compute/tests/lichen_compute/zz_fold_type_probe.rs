//! Probe: a fold containing a dispatch prints its value, not its type.
//!
//! # Invariant
//!
//! Print-only, for the type system's owner: the straight-line form is the control (same six numbers,
//! typed `array{Float, 6}`), the loop over an array state answers the same numbers as `?a`, and the
//! same loop with its root annotated is a third reading. Recorded at `dev@2b5c9933`; the readings it
//! produced are in `docs/notes/physics-operators.md`.

use lichen_compute::language::new_store;

use super::common;

/// The kernels and the input buffer the three cases below share.
const KERNEL: &str = r#"
XIn  := _x => {.x (compute.Buf (compute.BufParam{.type Float, .length _}))}
XOut := _x => {.y (compute.Buf (compute.BufParam{.type Float, .length _}))}
XPar := _x => {.n @any : Int, .alpha @any : Float, .in XIn _, .out XOut}
XArg := _x => {.n @any : Int, .alpha @any : Float, .I XIn _}
ring_raw := (c <: XPar) => {
  i := compute.range c.n
  centre := (compute.read (compute.Read{.from c.in.x, .at i}) : Float)
  left := if i == 0 then ((compute.read (compute.Read{.from c.in.x, .at (c.n - 1)}) : Float)) else ((compute.read (compute.Read{.from c.in.x, .at (i - 1)}) : Float))
  right := if i == (c.n - 1) then ((compute.read (compute.Read{.from c.in.x, .at 0}) : Float)) else ((compute.read (compute.Read{.from c.in.x, .at (i + 1)}) : Float))
  compute.write (compute.Write{.to c.out.y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre))})
}
ring := compute.parallel ring_raw "cpu" (XOut _)
x := (compute.buffer ([@any : Float, 6][4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
"#;

/// Three `plrun`s in a row: the control, whose answer is typed.
const STRAIGHT: &str = r#"
s1 := (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x x}}) : Struct)
s2 := (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x s1.y}}) : Struct)
s3 := (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x s2.y}}) : Struct)
compute.collect s3.y : Array
"#;

/// The same three steps as a top-level fold over an `array{Float, 6}` state.
const LOOP: &str = r#"
seed := (compute.collect (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x x}}) : Struct).y : Array)
@loop run := s => if s(0) == 0 then s(1) else run (s(0) - 1, (compute.collect (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x (compute.buffer (s(1)) : (compute.Buf (compute.BufParam{.type Float, .length _})))}}) : Struct).y : Array))
out := run (2, seed)
out
"#;

/// The same fold with its root annotated: does the annotation reach the container cell?
const LOOP_ANNOTATED: &str = r#"
seed := (compute.collect (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x x}}) : Struct).y : Array)
@loop run := s => if s(0) == 0 then s(1) else run (s(0) - 1, (compute.collect (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x (compute.buffer (s(1)) : (compute.Buf (compute.BufParam{.type Float, .length _})))}}) : Struct).y : Array))
out := run (2, seed)
(out : Array)
"#;

/// The same fold with the **recursive function** annotated, which is the rule the type system states.
const LOOP_RECURSION_ANNOTATED: &str = r#"
seed := (compute.collect (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x x}}) : Struct).y : Array)
@loop run := ((s <: (_x => (@any : Int, @any : Array))) => if s(0) == 0 then s(1) else run (s(0) - 1, (compute.collect (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x (compute.buffer (s(1)) : (compute.Buf (compute.BufParam{.type Float, .length _})))}}) : Struct).y : Array)))
out := run (2, seed)
out
"#;

/// What the raw runner renders for `body`, or how it refused.
fn rendered(body: &str) -> String {
    let source = format!("--- compute = import \"compute.lichen\" ---\n{KERNEL}{body}");
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(&source, None, &mut store) {
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
fn probe_the_fold_type() {
    let _slot = common::slot();
    for (label, body) in [
        ("straight", STRAIGHT),
        ("loop", LOOP),
        ("loop_annotated", LOOP_ANNOTATED),
        ("loop_recursion_annotated", LOOP_RECURSION_ANNOTATED),
    ] {
        println!("[{label}] {}", rendered(body));
    }
}
