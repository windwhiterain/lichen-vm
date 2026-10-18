//! A frozen buffer cell feeds the next launch (§9.17).

use lichen_compute::language::{LangProgram, new_store};
use lichen_language::package::PackageStore;
use lichen_language::preprocess::preprocess;
use lichen_language::session::BufferSession;
use lichen_lowlevel::AnyNodeId;

/// Two stages whose first result is retained between compiles.
const SOURCE: &str = r#"---
  compute = import "compute.lichen"
---
InA  = struct{.a Int}
OutA = struct{.z (compute.Buf Int)}
ParA = compute.P (compute.KT _)(.I InA, .O OutA)
adder = compute.parallel ((k : ParA) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 3))
}) "cpu" OutA
InB  = struct{.b (compute.Buf Int)}
OutB = struct{.w (compute.Buf Int)}
ParB = compute.P (compute.KT _)(.I InB, .O OutB)
doubler = compute.parallel ((k : ParB) => {
  i = compute.range k.n
  j = (compute.read ((compute.Read _)(.from k.in.b, .at i)) : Int)
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value j + j))
}) "cpu" OutB
raw = (compute.plrun adder ((compute.A InA)(.n 4, .I InA(.a 0))) : OutA)
@cache mid = raw.z
raw2 = (compute.plrun doubler ((compute.A InB)(.n 4, .I InB(.b mid))) : OutB)
raw2.w.native
"#;

/// The rendered root value, through the language's own printer.
fn rendered(report: &lichen_language::session::SessionReport<LangProgram>) -> String {
    let build = report.build.as_ref().expect("the program checks");
    let value = build
        .module
        .node_value(AnyNodeId::Dynamic(build.root_val))
        .expect("the root is decided");
    lichen_language::render::print_value_lang(&build.module, value, build.root_ty)
}

fn compile(
    store: &mut PackageStore<LangProgram>,
    session: &mut BufferSession<LangProgram>,
    source: &str,
) -> String {
    let (preprocessed, diagnostics) = preprocess(source, None, store);
    assert!(diagnostics.is_empty(), "the source preprocesses");
    let starts = lichen_language::lex::line_starts(source);
    session.set_view(
        preprocessed.code,
        preprocessed.code_base,
        &starts,
        &preprocessed.imports,
    );
    rendered(&session.compile())
}

/// # Invariant
/// The first compile freezes `mid`, and the edit touches only the second kernel, so the
/// second compile launches a kernel on a value that arrived through `mid`'s artifact.
#[test]
fn a_frozen_buffer_cell_still_feeds_the_next_launch() {
    let mut store = new_store(None).expect("the compute package registers");
    let mut session =
        BufferSession::<LangProgram>::with_registry("", "reuse.lichen", store.registry());
    let first = compile(&mut store, &mut session, SOURCE);
    assert_eq!(first, "raw Buffer", "the first compile answers a buffer");

    let edited = SOURCE.replace("j + j", "j + j + 1");
    let reused = compile(&mut store, &mut session, &edited);
    assert_eq!(
        reused, "raw Buffer",
        "a launch reading a frozen buffer answers, instead of staying undecided"
    );

    let mut fresh_store = new_store(None).expect("the compute package registers");
    let mut fresh =
        BufferSession::<LangProgram>::with_registry("", "fresh", fresh_store.registry());
    assert_eq!(
        reused,
        compile(&mut fresh_store, &mut fresh, &edited),
        "the reused arm answers what a fresh compile answers"
    );
}
