//! A `.in` group holding **both** a buffer and a scalar.
//! See `docs/notes/compute-kernel-struct.md`.
//!
//! # Invariant
//! A buffer field takes no wasm local, so a group holding one and a scalar flattens to
//! the scalar alone while the path through the group still counts the buffer's field —
//! the role table is the only place the two are reconciled.

use super::common;

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

