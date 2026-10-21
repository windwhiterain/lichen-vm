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

