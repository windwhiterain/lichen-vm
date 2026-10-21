//! Probe: one program, one annotation, and a struct fold that loses its value.
//!
//! # Invariant
//!
//! Print-only, for the type system's owner: both cases are the same three-step ring, the same seed and
//! the same body; the only difference is the type annotation on the recursive function. Unannotated
//! the loop prints `raw none` and records no diagnostic; annotated it answers the numbers and types.

use super::common;

use lichen_compute::language::new_store;

/// The ring kernel and its input, shared by both cases.
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
seed = (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut)
"#;

/// The fold as written: no annotation on the recursive function.
const UNANNOTATED: &str = r#"
@loop march = (s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s(1).y))) : XOut)))
settled = march (2, seed)
compute.collect settled.y : array{Float, 6}
"#;

/// The same fold with the recursive function's type stated: the state is a struct.
const ANNOTATED: &str = r#"
@loop march = ((s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s(1).y))) : XOut))) : tuple{Int, XOut} -> XOut)
settled = march (2, seed)
compute.collect settled.y : array{Float, 6}
"#;

/// The same annotated fold with the field read **on the apply result**: `(march (2, seed)).y`.
///
/// # Invariant
/// The read's chains root at the container pair, so evaluating it forces the apply; before `096b75bb`
/// this printed `raw none` even annotated, and it is the case that fix was measured against.
const ANNOTATED_INLINE: &str = r#"
@loop march = ((s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s(1).y))) : XOut))) : tuple{Int, XOut} -> XOut)
compute.collect (march (2, seed)).y : array{Float, 6}
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
fn probe_the_struct_annotation() {
    let (_stub, _guard) = common::stub();
    for (label, body) in [
        ("unannotated", UNANNOTATED),
        ("annotated", ANNOTATED),
        ("annotated_inline", ANNOTATED_INLINE),
    ] {
        println!("[{label}] {}", rendered(body));
    }
}
