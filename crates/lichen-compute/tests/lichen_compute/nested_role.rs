//! A `.in`/`.out` **group that is a struct**, and the scalar leaf in it.
//! See `docs/notes/compute-kernel-struct.md`.
//!
//! # Invariant
//! A parameter path is read by name at every depth, so a group reached as `k.in.g`
//! answers what the same group reached as `k.in` answers: the two are one ABI.

use super::common;

use lichen_compute::language::{LangProgram, LangValue};
use lichen_lowlevel::{Module, NodeId};

/// Compile and run `source`, returning the module, the root value and the root type node.
fn run(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    common::run(source)
}

/// The lanes the kernel runs over.
///
/// # Invariant
/// Every answer is a lane-wise map of the source's elements, so a read at the
/// wrong offset shows as a shifted answer rather than a uniform change.
const LANES: usize = 3;

/// The source buffer's element `lane` holds: `[10.0, 11.0, 12.0]`.
fn source_at(lane: usize) -> f32 {
    (lane + 10) as f32
}

/// A parallel kernel over `declarations`, writing `written` per lane.
/// `inbuf` is a `Float` source buffer.
///
/// # Invariant
/// The source holds [`source_at`] per lane, so every answer below is a map of it.
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
source_kernel = compute.parallel fill "cpu" SrcOut
inbuf = (compute.plrun source_kernel ((compute.A Src)(.n 3, .I Src(.a 0))) : SrcOut)
{declarations}
f = (k : Par) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value {written}))
}}
k = compute.parallel f "cpu" Out
out = (compute.plrun k ({argument}) : Out)
compute.collect out.w
"#,
    )
}

/// A buffer under a **grouped** `.in` answers what the flat group answers.
///
/// # Invariant
/// The `compute.P` domain is a cell while the body is checked, so every step of
/// the path arrives as the checker's lazy `TableGet`; the walk must still answer
/// each nested name against the type the step before it selected.
#[test]
fn a_buffer_under_a_grouped_input_is_read_at_its_own_depth() {
    let (flat_module, flat, _root) = run(&kernel(
        r#"
In = struct{.b (compute.Buf Float)}
Out = struct{.w (compute.Buf Float)}
Par = compute.P (compute.KT _)(.I In, .O Out)"#,
        "((compute.read ((compute.Read _)(.from k.in.b, .at i)) : Float) * 2.0)",
        "(compute.A In)(.n 3, .I In(.b inbuf.z))",
    ));
    let (grouped_module, grouped, _root) = run(&kernel(
        r#"
Inner = struct{.b (compute.Buf Float)}
In = struct{.g Inner}
Out = struct{.w (compute.Buf Float)}
Par = compute.P (compute.KT _)(.I In, .O Out)"#,
        "((compute.read ((compute.Read _)(.from k.in.g.b, .at i)) : Float) * 2.0)",
        "(compute.A In)(.n 3, .I In(.g Inner(.b inbuf.z)))",
    ));
    let doubled: Vec<f32> = (0..LANES).map(|lane| source_at(lane) * 2.0).collect();
    assert_eq!(
        common::float_array(&flat_module, &flat),
        doubled,
        "the flat group doubles each element"
    );
    assert_eq!(
        common::float_array(&grouped_module, &grouped),
        doubled,
        "the grouped `.in` doubles each element exactly as the flat one does"
    );
}

/// The scalar the grouped leaf is pushed as, and the answer it answers with.
///
/// # Invariant
/// A pushed scalar is uniform across lanes, so the answer is the same value at
/// every lane; the flat field beside it is the control that says the class, not
/// the depth, is what the launch reads.
const LEAF: f32 = 7.0;

/// A `Float` scalar under a **grouped** `.in` launches with the class it declares.
///
/// # Invariant
/// The class is the leaf's own type read at the leaf's own path, so the depth it
/// sits at does not decide it; the launch still compares it against what the
/// program passes.
#[test]
fn a_float_scalar_under_a_grouped_input_keeps_its_class() {
    let (flat_module, flat, _root) = run(&kernel(
        r#"
In = struct{.s Float}
Out = struct{.w (compute.Buf Float)}
Par = struct{.n Int, .in In, .out Out}
Host = struct{.n Int, .I In}"#,
        "int2float (float2int k.in.s)",
        "Host(.n 3, .I In(.s 7.0))",
    ));
    let (grouped_module, grouped, _root) = run(&kernel(
        r#"
Inner = struct{.s Float}
In = struct{.g Inner}
Out = struct{.w (compute.Buf Float)}
Par = struct{.n Int, .in In, .out Out}
Host = struct{.n Int, .I In}"#,
        "int2float (float2int k.in.g.s)",
        "Host(.n 3, .I In(.g Inner(.s 7.0)))",
    ));
    let pushed = vec![LEAF; LANES];
    assert_eq!(
        common::float_array(&flat_module, &flat),
        pushed,
        "the flat `.in` pushes the `Float` leaf as a `Float`"
    );
    assert_eq!(
        common::float_array(&grouped_module, &grouped),
        pushed,
        "the grouped `.in` pushes it as a `Float` too, as the flat one does"
    );
}
