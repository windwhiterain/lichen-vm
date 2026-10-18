//! Probe: a fold that threads a buffer, against one that threads an array.
//!
//! # Invariant
//!
//! Print-only, for the type system's owner. Four shapes of the same three-step ring: state as the
//! kernel's buffer (no `collect` inside the step), the same with no annotation, state as the output
//! struct, and state as an `array{Float, 6}` rebuilt with `compute.buffer` — the last a control.

use super::common;

use lichen_compute::language::new_store;

/// The ring kernel and its input, shared by the three cases.
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

/// The state is the kernel's **buffer**: no `collect` inside the step, so no readback per iteration.
const BUFFER_STATE: &str = r#"
seed = (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut).y
@loop march = ((s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s(1)))) : XOut).y)) : tuple{Int, compute.Buf Float} -> compute.Buf Float)
settled = march (2, seed)
compute.collect settled : array{Float, 6}
"#;

/// The same fold with no annotation, so the type reading is the fold's own.
const BUFFER_STATE_UNANNOTATED: &str = r#"
seed = (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut).y
@loop march = (s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s(1)))) : XOut).y))
settled = march (2, seed)
compute.collect settled : array{Float, 6}
"#;

/// The state is the kernel's output **struct**, annotated: the type system's rule makes it resolve.
const STRUCT_STATE_ANNOTATED: &str = r#"
seed = (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut)
@loop march = ((s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s(1).y))) : XOut))) : tuple{Int, XOut} -> XOut)
settled = march (2, seed)
compute.collect settled.y : array{Float, 6}
"#;

/// The state is the kernel's output **struct** with no annotation: it silently loses its value.
const STRUCT_STATE: &str = r#"
seed = (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut)
@loop march = (s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s(1).y))) : XOut)))
settled = march (2, seed)
compute.collect settled.y : array{Float, 6}
"#;

/// The state is an `array{Float, 6}` rebuilt each iteration: the control, and what works today.
const ARRAY_STATE: &str = r#"
seed = (compute.collect (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut).y : array{Float, 6})
@loop march = ((s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.collect (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x (compute.buffer (s(1)) : (compute.Buf Float))))) : XOut).y : array{Float, 6}))) : tuple{Int, array{Float, 6}} -> array{Float, 6})
settled = march (2, seed)
settled
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
fn probe_the_fold_state() {
    let _guard = common::device("zz_fold_buffer_probe");
    for (label, body) in [
        ("buffer_state", BUFFER_STATE),
        ("buffer_state_unannotated", BUFFER_STATE_UNANNOTATED),
        ("struct_state_annotated", STRUCT_STATE_ANNOTATED),
        ("struct_state", STRUCT_STATE),
        ("array_state", ARRAY_STATE),
    ] {
        println!("[{label}] {}", rendered(body));
    }
}
