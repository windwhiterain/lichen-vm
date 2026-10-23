//! A `.in` **group that is a struct**, and the scalar leaf in it.
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
///
/// No parameter carries an `.out` group: each body declares its own buffer with
/// `compute.buf` and returns the write as its codomain field's value, and the host's
/// source is a fill kernel's own output (`docs/notes/kernel-buffer-declarations.md`).
fn kernel(declarations: &str, written: &str, argument: &str) -> String {
    format!(
        r#"
--- compute = import "compute.lichen" ---
Src = struct{{.a Int}}
SrcOut = struct{{.z (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}}
SrcPar = struct{{.n Int, .in Src}}
fill = (k : SrcPar) => {{
  i = compute.range k.n
  z = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((SrcOut)(.z (compute.write ((compute.Write _)(.to z, .at i, .value int2float (i + 10))))))
}}
source_kernel = compute.parallel fill "cpu" SrcOut
inbuf = (compute.plrun source_kernel ((compute.A Src)(.n 3, .I Src(.a 0))) : SrcOut)
{declarations}
f = (k : Par) => {{
  i = compute.range k.n
  w = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Out)(.w (compute.write ((compute.Write _)(.to w, .at i, .value {written})))))
}}
k = compute.parallel f "cpu" Out
out = (compute.plrun k ({argument}) : Out)
compute.collect out.w
"#
    )
}

/// The scalar the grouped leaf is pushed as, and the answer it answers with.
///
/// # Invariant
/// A pushed scalar is uniform across lanes, so the answer is the same value at
/// every lane; the flat field beside it is the control that says the class, not
/// the depth, is what the launch reads.
const LEAF: f32 = 7.0;
