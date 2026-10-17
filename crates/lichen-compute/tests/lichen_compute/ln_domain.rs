//! `ln`'s body is a compensated quotient, and the wasm backend refuses it by name.
//!
//! # Invariant
//! The device leg is held at zero ulp by `spliced_transcendentals` against the definition, on both
//! an RTX 3060 and an Intel Iris Xe. This file holds the other leg: wasm's MVP has no fused
//! multiply-add, so the operator is **refused** rather than lowered into a program that would round
//! twice — the reason `fused-multiply-add.md` §7 measured and the emitter states.

use super::common;

use lichen_language::testing::float_array;

/// `1/sqrt(2)` as the class holds it: the reduction's closed lower edge.
const BAND_LOW: f32 = 0.707_106_78;
/// `sqrt(2)` as the class holds it: the reduction's open upper edge.
const BAND_HIGH: f32 = 1.414_213_5;
/// The fine phase's own guard: the `2^-4` step's product with the band, `16/sqrt(2)`.
const FINE_EDGE: f32 = BAND_LOW * 16.0;

/// The operand one unit of the last place below `x`, in the class's own order.
fn below(x: f32) -> f32 {
    f32::from_bits(x.to_bits() - 1)
}

/// The operands the refused kernel is built over.
///
/// # Invariant
/// Both band edges with the last `f32` either side of each, the fine phase's own guard, and one
/// operand per reduction phase, so the body being refused is the one the device leg measures.
fn operands() -> Vec<f32> {
    vec![
        1.0,
        0.99999994,
        BAND_LOW,
        below(BAND_LOW),
        BAND_HIGH,
        below(BAND_HIGH),
        FINE_EDGE,
        0.5,
        2.0,
        0.0001,
        1_099_511_627_776.0,
        0.0,
        -1.0,
    ]
}

/// One `compute.parallel` kernel applying `ln` elementwise, and its answers.
fn emitted(operands: &[f32]) -> Vec<f32> {
    // **A negative is written `0.0 - x`**: the language has no unary minus.
    let table: Vec<String> = operands
        .iter()
        .map(|x| {
            if x.is_sign_negative() {
                format!("(0.0 - {:?})", -*x)
            } else {
                format!("{x:?}")
            }
        })
        .collect();
    let source = format!(
        r#"
---
  compute = import "compute.lichen"
---
Args = struct{{.x (compute.Buf Float)}}
Out  = struct{{.v (compute.Buf Float)}}
Par  = compute.P (compute.KT Float)(.I Args, .O Out)
applied = (k : Par) => {{
  i = compute.range k.n
  x = compute.read ((compute.Read _)(.from k.in.x, .at i))
  compute.write ((compute.Write _)(.to k.out.v, .at i, .value (ln x ~ "element")))
}}
kernel = compute.parallel (applied ~ "claim:Elementwise(element)") "cpu"
args = (compute.buffer ([{}] : array{{Float, {}}}) : (compute.Buf Float))
out = (compute.plrun kernel ((compute.A Args)(.n {}, .I Args(.x args))) : Out)
compute.collect out.v : array{{Float, {}}}
"#,
        table.join(", "),
        operands.len(),
        operands.len(),
        operands.len(),
    );
    let (module, value, _root) = common::run(&source);
    let answers = float_array(&module, &value);
    assert_eq!(answers.len(), operands.len(), "every lane wrote its answer");
    answers
}

/// The wasm backend refuses `ln` by name instead of lowering a fused multiply-add into two roundings.
///
/// # Invariant
/// The anchor is the emitter's own sentence: a lowering would answer values the device does not, so
/// the refusal is the contract and not a failure. `expand_transcendentals`' invariant keeps one
/// definition for both targets, which is why this leg cannot be given a different sequence.
#[test]
#[should_panic(expected = "wasm's MVP has no instruction for one")]
fn ln_is_refused_by_name_on_the_wasm_backend() {
    let _ = emitted(&operands());
}
