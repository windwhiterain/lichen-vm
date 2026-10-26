//! Probe: one program, one annotation, and a struct fold that loses its value.
//!
//! # Invariant
//!
//! Print-only, for the type system's owner: the three cases are the same ring, seed
//! and body, and only the recursive function's annotation differs. Since
//! `53463b11` all three are refused at the **check**, so this reads the checker and
//! never a run; `docs/notes/operator-gaps.md` row three carries it, and
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
seed := (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x x}}) : Struct)
"#;

/// The fold as written: no annotation on the recursive function.
const UNANNOTATED: &str = r#"
@loop march := (s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x s(1).y}}) : Struct)))
settled := march (2, seed)
compute.collect settled.y : Array
"#;

/// The same fold with the recursive function's type stated: the state is a struct.
const ANNOTATED: &str = r#"
@loop march := ((s <: (_x => (@any : Int, XOut _))) => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x s(1).y}}) : Struct)))
settled := march (2, seed)
compute.collect settled.y : Array
"#;

/// The same annotated fold with the field read **on the apply result**: `(march (2, seed)).y`.
///
/// # Invariant
/// The read's chains root at the container expr, so evaluating it forces the apply; before `096b75bb`
/// this printed `raw none` even annotated, and it is the case that fix was measured against.
const ANNOTATED_INLINE: &str = r#"
@loop march := ((s <: (_x => (@any : Int, XOut _))) => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg){.n 6, .alpha 0.25, .I XIn{.x s(1).y}}) : Struct)))
compute.collect (march (2, seed)).y : Array
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
