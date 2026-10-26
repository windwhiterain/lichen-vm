//! A frozen buffer cell feeds the next launch (§9.17).

use lichen_compute::language::{LangProgram, new_store};
use lichen_language::package::PackageStore;
use lichen_language::preprocess::preprocess;
use lichen_language::session::BufferSession;
use lichen_lowlevel::AnyNodeId;

/// Two stages whose first result is retained between compiles.
///
/// # Invariant
/// Neither parameter carries an `.out` group: each body declares its buffer with
/// `compute.buf` and the write is the codomain field's value
/// (`docs/notes/kernel-buffer-declarations.md`). The root reads the second launch's buffer
/// through `compute.collect`, the outbound boundary, because a buffer's payload is not
/// readable as a value: that read is refused by name.
const SOURCE: &str = r#"---
  compute = import "compute.lichen"
---
InA  := _x => {.a @any : Int}
OutA := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length 4}))}
ParA := _x => {.n @any : Int, .in InA _}
adder := compute.parallel ((k <: ParA) => {
  i := compute.range k.n
  z := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((OutA){.z (compute.write (compute.Write{.to z, .at i, .value i + 3}))})
}) "cpu" OutA _
InB  := _x => {.b (compute.Buf (compute.BufParam{.type Int, .length 4}))}
OutB := _x => {.w (compute.Buf (compute.BufParam{.type Int, .length 4}))}
ParB := _x => {.n @any : Int, .in InB _}
doubler := compute.parallel ((k <: ParB) => {
  i := compute.range k.n
  j := (compute.read (compute.Read{.from k.in.b, .at i}) : Int)
  w := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((OutB){.w (compute.write (compute.Write{.to w, .at i, .value j + j}))})
}) "cpu" OutB _
raw := (compute.plrun adder (compute.A{.n 4, .I InA{.a 0}}) : Struct)
@cache mid := raw.z
raw2 := (compute.plrun doubler (compute.A{.n 4, .I InB{.b mid}}) : Struct)
compute.collect raw2.w : Array
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

/// The frozen cell feeds the next launch, and the next launch's buffer answers.
///
/// # Invariant
/// The first launch's buffer is frozen in `mid` and read by the second, so the answer is
/// the doubled row `[6, 8, 10, 12]`: both stages ran, and the frozen cell carried the
/// first one's result into the second.
#[test]
fn a_frozen_buffer_cell_still_feeds_the_next_launch() {
    let mut store = new_store(None).expect("the compute package registers");
    let mut session =
        BufferSession::<LangProgram>::with_registry("", "reuse.lichen", store.registry());
    let answer = compile(&mut store, &mut session, SOURCE);
    assert_eq!(
        answer, "[6, 8, 10, 12]",
        "the frozen cell fed the next launch, and the next launch doubled the row"
    );
}
