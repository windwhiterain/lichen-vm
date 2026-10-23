//! A lane's row and column, read over a launch's **second** extent.
//! See `docs/notes/two-dimensional-launch.md` §3(a).
//!
//! # Invariant
//! The extent is the configuration's second scalar leaf and not the scalar a body
//! names, so a launch that declares a third one answers the same whichever of the two
//! the body happens to read for itself.

use super::common;

use lichen_compute::language::new_store;

/// Six lanes over three columns, each writing `source[r * k.stride + c]`.
///
/// # Invariant
/// `.stride` is a **third** scalar leaf, so it is a number the body reads that is not
/// the second extent — which is what makes the coordinates' own read observable. The
/// answer is worked out from the shape: lanes `0..3` are row `0` and lanes `3..6` row
/// `1`, and each reads its own element of a row `2` wide.
const TWO_EXTENTS: &str = r#"
--- compute = import "compute.lichen" ---
In = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
Out = struct{.z (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
Par = struct{.n Int, .cols Int, .stride Int, .in In}
Host = struct{.n Int, .cols Int, .stride Int, .I In}
Data = struct{.d (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 6))
  ((Data)(.d (compute.write ((compute.Write _)(.to d, .at i, .value (int2float i + 1.0))))))
}
filler = compute.parallel fill "cpu" Data
body = (k : Par) => {
  i = compute.range k.n
  r = compute.row k
  c = compute.col k
  z = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Out)(.z (compute.write ((compute.Write _)(.to z, .at i, .value ((compute.read ((compute.Read _)(.from k.in.a, .at (r * k.stride + c))) : Float)))))))
}
k = compute.parallel body "cpu" Out
a = (compute.plrun filler ((Fill)(.n 6)) : Data)
out = (compute.plrun k ((Host)(.n 6, .cols 3, .stride 2, .I In(.a a.d))) : Out)
compute.collect out.z
"#;

/// [`TWO_EXTENTS`] with `.cols` gone, so the parameter declares **one** scalar leaf.
///
/// # Invariant
/// The bodies differ in the configuration alone, so a refusal below is about the
/// number of extents and not about anything the body computes. Dropping `.stride` as
/// well is the point: a configuration that still declared a second number would answer
/// `i / stride`, which is a different program and not a refusal.
const ONE_EXTENT: &str = r#"
--- compute = import "compute.lichen" ---
In = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
Out = struct{.z (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
Par = struct{.n Int, .in In}
Host = struct{.n Int, .I In}
Data = struct{.d (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 6))
  ((Data)(.d (compute.write ((compute.Write _)(.to d, .at i, .value (int2float i + 1.0))))))
}
filler = compute.parallel fill "cpu" Data
body = (k : Par) => {
  i = compute.range k.n
  r = compute.row k
  z = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Out)(.z (compute.write ((compute.Write _)(.to z, .at i, .value ((compute.read ((compute.Read _)(.from k.in.a, .at r)) : Float)))))))
}
k = compute.parallel body "cpu" Out
a = (compute.plrun filler ((Fill)(.n 6)) : Data)
out = (compute.plrun k ((Host)(.n 6, .I In(.a a.d))) : Out)
compute.collect out.z
"#;

/// The rendered diagnostics `program` refuses with.
fn attempt(program: &str) -> Result<String, Vec<String>> {
    let mut store = new_store(None).expect("the embedded compute package registers");
    lichen_language::run::evaluate_raw(program, None, &mut store)
        .map_err(|diags| diags.into_iter().map(|diag| diag.message).collect())
}

/// A configuration with one extent has no row or column to take, and says so.
///
/// # Invariant
/// A refusal by name rather than a number: dividing by a second extent that does not
/// exist would answer a lane the launch never declared, which is the hazard
/// `docs/notes/two-dimensional-launch.md` §7 calls the silent wrong answer.
#[test]
fn a_one_extent_config_is_refused_by_name() {
    let messages =
        attempt(ONE_EXTENT).expect_err("one extent is not a shape a row or a column has");
    assert_eq!(
        messages.len(),
        1,
        "one missing extent is one diagnostic: {messages:?}"
    );
    assert!(
        messages[0].contains("one extent") && messages[0].contains("compute.row"),
        "the refusal must name both the extent it found and the primitive that wanted \
         another: {messages:?}"
    );
}
