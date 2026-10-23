//! A kernel's classes must not depend on which kernel compiled first.
//! See `docs/notes/compute-graph-jit.md`.
//!
//! # Invariant
//!
//! **Ignored, and the reason is the defect it pins.** Both assertions fail on the
//! revision this file was written against: a module defining the step that
//! dispatches both kernels forces `attend` to compile first, and `attend` then
//! comes out `Int` where the program declares `Float`. The recording carries that
//! node, so a device run is refused by the backend's shape check.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::fusion_claim::{fragment_of, kernel_id};
use lichen_kernel_ir::ScalarClass;
use lichen_lowlevel::{AnyNodeId, Module};
use lichen_utils::extend::AsEnum;

/// The two attention kernels: `scores` over `Q`, then `attend` over the scores and `V`.
///
/// # Invariant
///
/// Both declare `Float` buffers on every side, so **every** class in both fragments
/// is `Float`: the pair differs only in what it computes. A difference between the
/// fragments is a difference between the compilations, never one the program asked
/// for.
///
/// No parameter carries an `.out` group: the body declares its buffer with
/// `compute.buf` (`docs/notes/kernel-buffer-declarations.md`).
const KERNELS: &str = r#"
QK = struct{.q (compute.Buf ((compute.BufParam _)(.type Float, .length 192)))}
Scored = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length 192)))}
ParS = struct{.n Int, .keys Int, .dim Int, .in QK}
ArgS = struct{.n Int, .keys Int, .dim Int, .I QK}
scores_raw = (k : ParS) => {
  i = compute.range k.n
  query = compute.row k
  key = compute.col k
  @loop dot = s => if s(0) == 0 then s(1) else dot (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.q, .at (query * k.dim + s(0) - 1))) : Float) * (compute.read ((compute.Read _)(.from k.in.q, .at (key * k.dim + s(0) - 1))) : Float)) ~ "combine")
  z = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Scored)(.s (compute.write ((compute.Write _)(.to z, .at i, .value ((dot (k.dim, 0.0 ~ "identity") / sqrt (int2float k.dim)) ~ "element"))))))
}
scores = compute.parallel (scores_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Scored
QSV = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length 192))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 192)))}
OutA = struct{.o (compute.Buf ((compute.BufParam _)(.type Float, .length 192)))}
ParA = struct{.n Int, .dim Int, .keys Int, .in QSV}
ArgA = struct{.n Int, .dim Int, .keys Int, .I QSV}
attend_raw = (k : ParA) => {
  i = compute.range k.n
  query = compute.row k
  column = compute.col k
  @loop top = s => if s(0) == 0 then s(1) else top (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.s, .at (query * k.keys + s(0) - 1))) : Float) ~ "element")) ~ "combine")
  m = top (k.keys, ((0.0 - 1.0) / 0.0) ~ "identity")
  @loop den = s => if s(0) == 0 then s(1) else den (s(0) - 1, (s(1) + (exp ((compute.read ((compute.Read _)(.from k.in.s, .at (query * k.keys + s(0) - 1))) : Float) - m) ~ "element")) ~ "combine")
  denominator = den (k.keys, 0.0 ~ "identity")
  @loop acc = s => if s(0) == 0 then s(1) else acc (s(0) - 1, (s(1) + (exp ((compute.read ((compute.Read _)(.from k.in.s, .at (query * k.keys + s(0) - 1))) : Float) - m) ~ "element") * (compute.read ((compute.Read _)(.from k.in.v, .at ((s(0) - 1) * k.dim + column))) : Float)) ~ "combine", s(2))
  z = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutA)(.o (compute.write ((compute.Write _)(.to z, .at i, .value ((acc (k.keys, 0.0 ~ "identity", m) / denominator) ~ "element"))))))
}
attend = compute.parallel (attend_raw ~ "claim:Reduction(combine,identity,element)") "gpu" OutA
"#;

/// The same two kernels, and nothing that dispatches them.
///
/// # Invariant
///
/// The root is the pair, so the module forces `scores` before `attend` — the order
/// in which both compile the same bodies the test program compiles the other way
/// round. Nothing else differs: the kernel declarations are the same in both.
const CONTROL: &str = "(scores, attend)\n";

/// The recorded step, which forces `attend` before `scores`.
///
/// # Invariant
///
/// `step` names `attend` in its body and `compute.graph` forces both kernels while
/// the recording is open, so this module compiles them in the opposite order to
/// [`CONTROL`] over byte-identical kernel declarations.
const STEP: &str = r#"QKV = struct{.q (compute.Buf ((compute.BufParam _)(.type Float, .length 192))), .k (compute.Buf ((compute.BufParam _)(.type Float, .length 192))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 192)))}
GArg = struct{.lanes Int, .on Int, .keys Int, .dim Int, .in QKV}
step = (a : GArg) => {
  scale = (compute.plrun scores ((ArgS)(.n a.lanes, .keys a.keys, .dim a.dim, .I QK(.q a.in.q))) : Scored)
  (compute.plrun attend ((ArgA)(.n a.on, .dim a.dim, .keys a.keys, .I QSV(.s scale.s, .v a.in.v))) : OutA)
}
built = compute.graph step
built
"#;

/// The classes a module's own nodes name for every kernel it holds, sorted.
///
/// # Invariant
///
/// **The multiset, not a named kernel.** Which kernel compiles first is a property
/// of the module and not of the source, so a test pinning one kernel by name would
/// be pinning the order; the two compilations are compared as the two fragments
/// they produced.
fn classes(module: &Module<lichen_compute::language::LangProgram>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    for (id, _node) in module.nodes.iter() {
        let Ok(kernel) = kernel_id(module, id) else {
            continue;
        };
        if seen.contains(&kernel) {
            continue;
        }
        let Ok(fragment) = fragment_of(kernel) else {
            continue;
        };
        seen.push(kernel);
        found.push(format!(
            "in {:?} out {:?} ({} input(s), {} output(s))",
            fragment.input_classes, fragment.output_classes, fragment.inputs, fragment.outputs
        ));
    }
    found.sort();
    found
}

/// The classes `extra` and the shared kernels give, as a program of their own.
fn compiled(extra: &str) -> Vec<String> {
    let build = common::check(&format!(
        "---\n  compute = import \"compute.lichen\"\n---\n{KERNELS}{extra}"
    ))
    .build
    .expect("the program checks");
    let mut module = build.module;
    // The classes are stated while the module's own deep pass runs, so it runs here.
    let _ = module.evaluate_node_deep(build.root_val, None);
    classes(&module)
}

/// **A kernel's classes are a fact about its body, not about what compiled before it.**
///
/// # Invariant
///
/// The two programs differ by `step` alone and name the same two bodies, so a
/// compiler that reads a declaration the same way both times returns the same two
/// fragments. One that reads it from whatever the module happened to evaluate
/// first returns a different answer for the same body, and the recorded graph then
/// carries a kernel the program never wrote.
#[test]
fn a_kernels_classes_do_not_depend_on_which_kernel_compiled_first() {
    let control = compiled(CONTROL);
    let recorded = compiled(STEP);
    assert_eq!(
        control.len(),
        2,
        "both programs compile the same two kernels: {control:?}"
    );
    assert_eq!(
        control, recorded,
        "the same two bodies, compiled in opposite orders, must come out as the same two \
         fragments"
    );
    assert!(
        control.iter().all(|entry| entry.contains("Float")),
        "and the program's own declarations are `Float` on every side: {control:?}"
    );
}

/// The recorded node carries the program's classes, not the fallback.
///
/// # Invariant
///
/// Node 1 is the consumer by position: a chain records its dispatches in the order
/// the body performs them, and the body dispatches `scores` first.
#[test]
fn the_recorded_consumer_carries_the_programs_float_class() {
    let build = common::check(&format!(
        "---\n  compute = import \"compute.lichen\"\n---\n{KERNELS}{STEP}"
    ))
    .build
    .expect("the program checks");
    let mut module = build.module;
    // The graph is recorded while the module's own deep pass runs, so it runs here.
    let _ = module.evaluate_node_deep(build.root_val, None);
    let value = module
        .node_value(AnyNodeId::Dynamic(build.root_val))
        .expect("the root is decided");
    assert!(
        module
            .extension_diagnostics
            .iter()
            .all(|entry| !entry.message.contains("not a value a graph can hand back")),
        "the chain records; nothing is refused at the return"
    );
    let ComputeValue::Graph(id, _) =
        AsEnum::<ComputeValue>::as_enum(&value).expect("the root is the graph the step recorded")
    else {
        panic!("the step recorded no graph");
    };
    let graph = lichen_compute::compute::graph::lookup(id).expect("the graph is registered");
    assert_eq!(
        graph.len(),
        2,
        "a chain of two dispatches is two nodes: {graph:?}"
    );
    let lichen_graph_ir::Node::Kernel(kernel) = &graph.nodes()[1];
    assert_eq!(
        kernel.fragment.output_classes,
        vec![ScalarClass::Float],
        "`attend` writes a `Float` buffer, so the recorded node says Float whatever order \
         the module compiled the two kernels in"
    );
    assert_eq!(
        kernel.fragment.input_classes,
        vec![ScalarClass::Float, ScalarClass::Float],
        "and it reads two `Float` buffers, the scores and `V`"
    );
}
