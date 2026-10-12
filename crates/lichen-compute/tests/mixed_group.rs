//! A `.in` group holding **both** a buffer and a scalar.
//! See `docs/notes/compute-kernel-struct.md`.
//!
//! # Invariant
//! A buffer field takes no wasm local, so a group holding one and a scalar flattens to
//! the scalar alone while the path through the group still counts the buffer's field —
//! the role table is the only place the two are reconciled.

mod common;

use lichen_compute::language::{LangProgram, LangValue};
use lichen_lowlevel::{Module, NodeId};

/// Compile and run `source`, returning the module, the root value and the root type node.
fn run(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    common::run(source)
}

/// The lanes the kernel runs over.
const LANES: usize = 3;

/// The scalar the kernel is handed, uniform across lanes.
const LEAF: f32 = 7.0;

/// A parallel kernel over `declarations`, writing `written` per lane.
///
/// # Invariant
/// The source buffer holds `int2float (i + 10)` over `LANES`, so a doubled read answers
/// `[20.0, 22.0, 24.0]` and the added scalar answers `LEAF` at every lane.
fn kernel(declarations: &str, written: &str, argument: &str) -> String {
    format!(
        r#"
--- compute = import "compute.lichen" ---
Src = struct{{.a Int}}
SrcOut = struct{{.z (compute.Buf Float)}}
SrcPar = compute.P (compute.KT _)(.I Src, .O SrcOut)
fill = (k : SrcPar) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value int2float (i + 10)))
}}
source_kernel = compute.parallel fill "cpu"
inbuf = (compute.plrun source_kernel ((compute.A Src)(.n 3, .I Src(.a 0))) : SrcOut)
{declarations}
f = (k : Par) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value {written}))
}}
k = compute.parallel f "cpu"
out = (compute.plrun k ({argument}) : Out)
compute.collect out.w
"#,
    )
}

/// A group holding a buffer **and** a scalar answers what the flat control answers.
///
/// # Invariant
/// Both parameter structs flatten to the same two scalar locals in the same order, so
/// the two kernels are the same ABI and a read placed at one path must place at the
/// other.
#[test]
fn a_mixed_group_answers_what_the_flat_control_answers() {
    let (flat_module, flat, _root) = run(&kernel(
        r#"
In = struct{.b (compute.Buf Float)}
Out = struct{.w (compute.Buf Float)}
Par = struct{.n Int, .s Float, .in In, .out Out}
Host = struct{.n Int, .S Float, .I In}"#,
        r#"(compute.read ((compute.Read _)(.from k.in.b, .at i)) * 2.0 + int2float (float2int k.s))"#,
        "Host(.n 3, .S 7.0, .I In(.b inbuf.z))",
    ));
    let (mixed_module, mixed, _root) = run(&kernel(
        r#"
In = struct{.b (compute.Buf Float), .s Float}
Out = struct{.w (compute.Buf Float)}
Par = struct{.n Int, .in In, .out Out}
Host = struct{.n Int, .I In}"#,
        r#"(compute.read ((compute.Read _)(.from k.in.b, .at i)) * 2.0 + int2float (float2int k.in.s))"#,
        "Host(.n 3, .I In(.b inbuf.z, .s 7.0))",
    ));
    let expected: Vec<f32> = (0..LANES)
        .map(|lane| (lane as f32 + 10.0) * 2.0 + LEAF)
        .collect();
    assert_eq!(
        common::float_array(&flat_module, &flat),
        expected,
        "the flat control doubles the buffer and adds the scalar beside the group"
    );
    assert_eq!(
        common::float_array(&mixed_module, &mixed),
        expected,
        "the mixed group doubles the buffer and adds the scalar inside the group"
    );
}
