//! Probe: a dispatch over extent 0, plain and inside a graph.
//!
//! # Invariant
//!
//! Print-only. The simplest kernel, `z[i] = i + 10`, launched with `.n 0`, in both forms; the
//! reading is whether an end-of-sequence flag can gate a step by driving its extent to zero.

use std::path::Path;

use super::common;

use lichen_compute::language::{COMPUTE_PATH, LangProgram, LangValue, new_store};
use lichen_highlevel::checker::Mode;
use lichen_highlevel::ir::ExprId;
use lichen_highlevel::program::HighProgramLiteral;
use lichen_language::program::LangAttr;
use lichen_language::render::print_answer_lang;
use lichen_lowlevel::LowValue;
use lichen_testkit::writer::Writer;

/// The two programs the probe asks about.
#[derive(Clone, Copy)]
enum Body {
    /// One `compute.plrun` at extent 0, then the collect.
    Plain,
    /// The same dispatch as a `compute.graph` step run at extent 0.
    Recorded,
}

/// What the kernel is written against: the compute pair, and the prelude's `add`.
///
/// # Invariant
/// `add` is the prelude's own `add` rather than a `BinOp`: a source `+` is routed to the
/// prelude binding, so the written program routes it the same way or it is not the same program.
struct Imports {
    pair: ExprId,
    add: ExprId,
}

/// The kernel's module-level bindings the bodies read.
struct Kernel {
    in0: ExprId,
    out0: ExprId,
    kg: ExprId,
}

/// One name of the compute package, read off its exported pair.
fn bundled(
    w: &Writer<LangAttr, HighProgramLiteral>,
    package: ExprId,
    field: &'static str,
) -> ExprId {
    w.named_field(field, package, field)
}

/// The field tuple an instantiation carries: one element per field, named by the instantiation.
fn fields(w: &Writer<LangAttr, HighProgramLiteral>, name: &str, values: &[ExprId]) -> ExprId {
    w.tuple(name, values)
}

/// The kernel, as IR: `In0`, `Out0`, `Par0`, `g` and `kg`.
fn write_kernel(w: &Writer<LangAttr, HighProgramLiteral>, imports: &Imports) -> Kernel {
    let int = w.int_type("Int");
    let in0 = w.statement(w.struct_type("In0", &[(int, Some("a"))]));

    let buf = bundled(w, imports.pair, "Buf");
    let out0 = w.app("Buf_of_element", buf, w.hole("z_element"));
    let out0 = w.statement(w.struct_type("Out0", &[(out0, Some("z"))]));

    let kt = bundled(w, imports.pair, "KT");
    let kt = w.app("KT_of_element", kt, w.hole("kt_element"));
    let kt = w.instantiate(
        "KT_of_fields",
        kt,
        fields(w, "Par0_fields", &[in0, out0]),
        &[Some("I"), Some("O")],
    );
    let par0 = w.statement(w.app("Par0", bundled(w, imports.pair, "P"), kt));

    let k = w.param("k");
    let i = w.app(
        "i",
        bundled(w, imports.pair, "range"),
        w.named_field("k_n", k, "n"),
    );
    let write_type = w.app(
        "Write_of_element",
        bundled(w, imports.pair, "Write"),
        w.hole("write_element"),
    );
    let ten = w.app(
        "i_plus_ten",
        imports.add,
        w.array("operands", &[i, w.int("ten", 10)]),
    );
    let write = w.instantiate(
        "write_argument",
        write_type,
        fields(
            w,
            "argument_fields",
            &[
                w.named_field("k_out_z", w.named_field("k_out", k, "out"), "z"),
                i,
                ten,
            ],
        ),
        &[Some("to"), Some("at"), Some("value")],
    );
    let body = w.app("write_call", bundled(w, imports.pair, "write"), write);
    // A block is a tuple of its statements read positionally, and its last statement is the one
    // the block answers with.
    let body = w.field("g_last", w.tuple("g_stmts", &[i, body]), w.int("last", 1));
    let g = w.statement(w.lam_typed("g", k, Some(par0), body, false));

    let parallel = w.app("parallel_g", bundled(w, imports.pair, "parallel"), g);
    let parallel = w.app("parallel_backend", parallel, w.string("gpu", "gpu"));
    let kg = w.statement(w.app("kg", parallel, out0));

    Kernel { in0, out0, kg }
}

/// The launch argument both bodies dispatch: `In0` over `.n`, the extent itself varied.
fn launch_argument(
    w: &Writer<LangAttr, HighProgramLiteral>,
    imports: &Imports,
    kernel: &Kernel,
    n: ExprId,
) -> ExprId {
    let datum = w.instantiate(
        "In0_datum",
        kernel.in0,
        fields(w, "In0_fields", &[w.int("a", 0)]),
        &[Some("a")],
    );
    let argument = w.app("A_of_In0", bundled(w, imports.pair, "A"), kernel.in0);
    w.instantiate(
        "launch_argument",
        argument,
        fields(w, "launch_fields", &[n, datum]),
        &[Some("n"), Some("I")],
    )
}

/// The dispatch both bodies make, launched over `n`.
fn dispatch(
    w: &Writer<LangAttr, HighProgramLiteral>,
    imports: &Imports,
    kernel: &Kernel,
    n: ExprId,
) -> ExprId {
    let plrun = w.app("plrun_kg", bundled(w, imports.pair, "plrun"), kernel.kg);
    let argument = launch_argument(w, imports, kernel, n);
    w.app("dispatch", plrun, argument)
}

/// The plain form's root: the dispatch at extent 0, then the collect of its buffer.
fn write_plain(
    w: &Writer<LangAttr, HighProgramLiteral>,
    imports: &Imports,
    kernel: &Kernel,
) -> ExprId {
    let out = w.statement(w.annotate(
        "out",
        dispatch(w, imports, kernel, w.int("n", 0)),
        kernel.out0,
    ));
    w.app(
        "collected",
        bundled(w, imports.pair, "collect"),
        w.named_field("out_z", out, "z"),
    )
}

/// The recorded form's root: the same dispatch as a graph run at extent 0.
fn write_recorded(
    w: &Writer<LangAttr, HighProgramLiteral>,
    imports: &Imports,
    kernel: &Kernel,
) -> ExprId {
    let garg = w.statement(w.struct_type(
        "GArg",
        &[(w.int_type("Int"), Some("n")), (kernel.in0, Some("in"))],
    ));

    let s = w.param("s");
    let step = w.statement(w.lam_typed(
        "step",
        s,
        Some(garg),
        w.annotate(
            "out",
            dispatch(w, imports, kernel, w.named_field("s_n", s, "n")),
            kernel.out0,
        ),
        false,
    ));
    let built = w.statement(w.app("built", bundled(w, imports.pair, "graph"), step));
    let run = w.app(
        "graphrun_built",
        bundled(w, imports.pair, "graphrun"),
        built,
    );
    let run = w.app("run_at_zero", run, w.int("zero", 0));
    w.app("collected", bundled(w, imports.pair, "collect"), run)
}

/// What the runner renders for `body`, or how it refused.
fn rendered(body: Body) -> String {
    let mut store = new_store(None).expect("the embedded compute package registers");
    // `load_package` serves the wrapper `new_store` already registered: registering it again
    // recompiles it.
    let package = store
        .load_package(Path::new(COMPUTE_PATH))
        .expect("the embedded compute package registers");
    let core = store
        .load_package(Path::new("core.lichen"))
        .expect("the core prelude registers");
    let add = core
        .direct
        .iter()
        .find(|(name, _)| name == "add")
        .map(|(_, export)| *export)
        .expect("the prelude binds `add`");

    let w = Writer::<LangAttr, HighProgramLiteral>::new();
    let imports = Imports {
        pair: w.static_ref("compute", package.export),
        add: w.static_ref("add", add),
    };
    let kernel = write_kernel(&w, &imports);
    let root = match body {
        Body::Plain => write_plain(&w, &imports, &kernel),
        Body::Recorded => write_recorded(&w, &imports, &kernel),
    };
    let report = lichen_language::build_report::<LangProgram>(
        Some(w.finish(root).ir),
        None,
        Vec::new(),
        Some(store.registry()),
        lichen_highlevel::no_native_ops(),
        None,
        Vec::new(),
        "",
        Mode::Evaluate,
    );
    let messages = |diagnostics: Vec<String>| format!("refused: {diagnostics:?}");
    if !report.ok() {
        return messages(
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect(),
        );
    }
    let build = report.build.expect("the check produced a build");
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    let _ = module.evaluate_node_deep(build.root_ty, None);
    if module.has_extension_failures() {
        return messages(
            module
                .extension_diagnostics
                .iter()
                .map(|entry| format!("{}: {}", entry.category, entry.message))
                .collect(),
        );
    }
    let value = value.unwrap_or_else(|| LangValue::from(LowValue::None));
    let (value_text, type_text) = print_answer_lang::<LangProgram>(&module, value, build.root_ty);
    format!("{value_text}: {type_text}")
}

#[test]
fn probe_the_zero_extent() {
    let (_stub, _guard) = common::stub();
    println!("[plain] {}", rendered(Body::Plain));
    println!("[recorded] {}", rendered(Body::Recorded));
}
