//! Probe: one program, one annotation, and a struct fold that loses its value.
//!
//! # Invariant
//!
//! Print-only, for the type system's owner: the three cases are the same three-step ring, the same
//! seed and the same body, and only the annotation on the recursive function differs. Since
//! `53463b11` all three are refused at the **check** (`this field read cannot be checked`), so this
//! reads the checker and never a run; `docs/notes/operator-gaps.md` row three carries it, and
//! `examples/decode_loop.lichen` is the spelling that does run.

use super::common;

/// What the compile recorded: its diagnostics, and nothing a run answered.
fn recorded(body: &str) -> String {
    let report = common::check(&format!(
        "--- compute = import \"compute.lichen\" ---\n{KERNEL}{body}"
    ));
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

#[test]
fn probe_the_struct_annotation() {
    for (label, body) in [
        ("unannotated", UNANNOTATED),
        ("annotated", ANNOTATED),
        ("annotated_inline", ANNOTATED_INLINE),
    ] {
        println!("[{label}] {}", recorded(body));
    }
}
