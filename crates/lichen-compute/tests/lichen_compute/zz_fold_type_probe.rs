//! Probe: a fold containing a dispatch prints its value, not its type.
//!
//! # Invariant
//!
//! Print-only, for the type system's owner: the straight-line form is the control (same six numbers,
//! typed `array{Float, 6}`), the loop over an array state answers the same numbers as `?a`, and the
//! same loop with its root annotated is a third reading. Recorded at `dev@2b5c9933`; the readings it
//! produced are in `docs/notes/physics-operators.md`.

use lichen_compute::language::new_store;

/// The kernels and the input buffer the three cases below share.
const KERNEL: &str = r#"
XIn  = struct{.x (compute.Buf Float)}
XOut = struct{.y (compute.Buf Float)}
XPar = struct{.n Int, .alpha Float, .in XIn, .out XOut}
XArg = struct{.n Int, .alpha Float, .I XIn}
ring_raw = (c : XPar) => {
  i = compute.range c.n
  centre = (compute.read ((compute.Read _)(.from c.in.x, .at i)) : Float)
  left = if i == 0 then ((compute.read ((compute.Read _)(.from c.in.x, .at (c.n - 1))) : Float)) else ((compute.read ((compute.Read _)(.from c.in.x, .at (i - 1))) : Float))
  right = if i == (c.n - 1) then ((compute.read ((compute.Read _)(.from c.in.x, .at 0)) : Float)) else ((compute.read ((compute.Read _)(.from c.in.x, .at (i + 1))) : Float))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre))))
}
ring = compute.parallel ring_raw "cpu" XOut
x = (compute.buffer ([4.0, 0.0, 0.0, 0.0, 0.0, 4.0] : array{Float, 6}) : (compute.Buf Float))
"#;

/// Three `plrun`s in a row: the control, whose answer is typed.
const STRAIGHT: &str = r#"
s1 = (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut)
s2 = (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s1.y))) : XOut)
s3 = (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s2.y))) : XOut)
compute.collect s3.y : array{Float, 6}
"#;

/// The same three steps as a top-level fold over an `array{Float, 6}` state.
const LOOP: &str = r#"
seed = (compute.collect (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut).y : array{Float, 6})
@loop run = s => if s(0) == 0 then s(1) else run (s(0) - 1, (compute.collect (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x (compute.buffer (s(1)) : (compute.Buf Float))))) : XOut).y : array{Float, 6}))
out = run (2, seed)
out
"#;

/// The same fold with its root annotated: does the annotation reach the container cell?
const LOOP_ANNOTATED: &str = r#"
seed = (compute.collect (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut).y : array{Float, 6})
@loop run = s => if s(0) == 0 then s(1) else run (s(0) - 1, (compute.collect (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x (compute.buffer (s(1)) : (compute.Buf Float))))) : XOut).y : array{Float, 6}))
out = run (2, seed)
(out : array{Float, 6})
"#;

/// The same fold with the **recursive function** annotated, which is the rule the type system states.
const LOOP_RECURSION_ANNOTATED: &str = r#"
seed = (compute.collect (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut).y : array{Float, 6})
@loop run = ((s => if s(0) == 0 then s(1) else run (s(0) - 1, (compute.collect (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x (compute.buffer (s(1)) : (compute.Buf Float))))) : XOut).y : array{Float, 6}))) : tuple{Int, array{Float, 6}} -> array{Float, 6})
out = run (2, seed)
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
    for (label, body) in [
        ("straight", STRAIGHT),
        ("loop", LOOP),
        ("loop_root_annotated", LOOP_ANNOTATED),
        ("loop_recursion_annotated", LOOP_RECURSION_ANNOTATED),
    ] {
        println!("[{label}] {}", rendered(body));
    }
}
