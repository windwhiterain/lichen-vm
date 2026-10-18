//! Temporary: a frozen buffer cell feeding a later launch.
//! See docs/notes/heavy-value-cache.md §9.

use lichen_compute::language::{LangProgram, new_store};
use lichen_language::package::PackageStore;
use lichen_language::preprocess::preprocess;
use lichen_language::session::BufferSession;
use lichen_lowlevel::AnyNodeId;

const SOURCE: &str = r#"---
  compute = import "compute.lichen"
---
InA  = struct{.a Int}
OutA = struct{.z (compute.Buf Int)}
ParA = compute.P (compute.KT _)(.I InA, .O OutA)
adder = compute.parallel ((k : ParA) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 3))
}) "cpu"
InB  = struct{.b (compute.Buf Int)}
OutB = struct{.w (compute.Buf Int)}
ParB = compute.P (compute.KT _)(.I InB, .O OutB)
doubler = compute.parallel ((k : ParB) => {
  i = compute.range k.n
  j = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value j + j))
}) "cpu"
raw = (compute.plrun adder ((compute.A InA)(.n 4, .I InA(.a 0))) : OutA)
cache mid = raw.z
raw2 = (compute.plrun doubler ((compute.A InB)(.n 4, .I InB(.b mid))) : OutB)
(mid, raw2.w.native)
"#;

fn value_of(report: &lichen_language::session::SessionReport<LangProgram>) -> String {
    let Some(build) = report.build.as_ref() else {
        return "<no build>".to_string();
    };
    let Some(value) = build.module.node_value(AnyNodeId::Dynamic(build.root_val)) else {
        return "<raw none>".to_string();
    };
    lichen_language::render::print_value_lang(&build.module, value, build.root_ty)
}

fn compile(
    store: &mut PackageStore<LangProgram>,
    session: &mut BufferSession<LangProgram>,
    source: &str,
) -> String {
    let (preprocessed, diagnostics) = preprocess(source, None, store);
    if !diagnostics.is_empty() {
        println!("  preprocess: {diagnostics:?}");
    }
    let starts = lichen_language::lex::line_starts(source);
    session.set_view(
        preprocessed.code,
        preprocessed.code_base,
        &starts,
        &preprocessed.imports,
    );
    let report = session.compile();
    for diagnostic in report.diagnostics.iter().take(4) {
        println!("  diagnostic: {diagnostic:?}");
    }
    if let Some(build) = report.build.as_ref() {
        for entry in build.module.extension_diagnostics.iter().take(8) {
            println!("  extension {}: {}", entry.category, entry.message);
        }
    }
    println!(
        "  cells reused {}, frozen {}, dropped {}, value {}",
        report.cells.reused,
        report.cells.frozen,
        report.cells.dropped,
        value_of(&report),
    );
    value_of(&report)
}

fn main() {
    let mut store = new_store(None).expect("the compute package registers");
    let mut session =
        BufferSession::<LangProgram>::with_registry("", "reuse.lichen", store.registry());
    println!("first: value {}", compile(&mut store, &mut session, SOURCE));
    let edited = SOURCE.replace("j + j", "j + j + 1");
    println!(
        "edited: value {}",
        compile(&mut store, &mut session, &edited)
    );
    let again = SOURCE.replace("j + j", "j + j + 2");
    println!("again: value {}", compile(&mut store, &mut session, &again));
}
