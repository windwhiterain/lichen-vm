//! Probe: the constructor pipeline's parallel and jit paths, printing and asserting nothing.
//!
//! The cases name what each reading shows: a frozen constructor's application,
//! the `parallel` call's arity, the kernel body's lowering, and the jit
//! launch's result type.

use lichen_compute::language::{LangProgram, new_store};
use lichen_lowlevel::{AnyNodeId, LowValue, NodeId};
use lichen_utils::extend::AsEnum;

/// Compile `source` in a fresh store and report what the check recorded.
fn recorded(source: &str) -> String {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(source, None, &mut store);
    if !diags.is_empty() {
        return format!("preprocess: {diags:?}");
    }
    let report = lichen_language::compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &lichen_language::lex::line_starts(source),
        lichen_highlevel::no_native_ops(),
    );
    let mut out = format!(
        "ok={} diags={}",
        report.ok(),
        report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<String>>()
            .join(" | ")
    );
    if let Some(build) = report.build {
        let module = &build.module;
        out.push_str(&format!("\n  apply_errors={}", module.apply_errors.len()));
        for error in &module.apply_errors {
            out.push_str(&format!(
                "\n    apply fn={:?}\n      param node={:?} value={}\n      arg   node={:?} value={}",
                error.function,
                error.parameter_type,
                shape(module, error.parameter_type),
                error.argument_type,
                shape(module, error.argument_type),
            ));
        }
        out.push_str(&format!("\n  eval_errors={:?}", module.eval_errors));
        for (index, error) in module.unify_errors.iter().enumerate() {
            out.push_str(&format!(
                "\n    unify[{index}] root_a={:?} root_b={:?} steps={:?}",
                error.root_a, error.root_b, error.steps
            ));
            out.push_str(&format!(
                "\n      root_a shape={}",
                shape(module, error.root_a)
            ));
            out.push_str(&format!(
                "\n      root_b shape={}",
                shape(module, error.root_b)
            ));
        }
    }
    out
}

/// A node's own shape: its value, and the items of an array value.
fn shape(module: &lichen_lowlevel::Module<LangProgram>, node: NodeId) -> String {
    let Some(value) = module.node_value(AnyNodeId::Dynamic(node)) else {
        return "<no value>".to_string();
    };
    let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(&value) else {
        return format!("{value:?}");
    };
    // SAFETY: the node is live in this module.
    let items = unsafe { array.items() };
    let mut out = format!("array[{}] layout={}", items.len(), array.layout());
    for (position, item) in items.iter().enumerate() {
        out.push_str(&format!("\n        {position}: {:?}", item.node));
    }
    out
}

/// Render a program's own output, or the diagnostics that stopped it.
fn rendered(source: &str) -> String {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw::<LangProgram>(source, None, &mut store) {
        Ok(output) => output,
        Err(diags) => format!(
            "diagnostics: {}",
            diags
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<String>>()
                .join(" | ")
        ),
    }
}

const CASES: &[(&str, &str)] = &[
    (
        "a struct constructor applied once",
        "In := _x => {.a @any : Int}
In{.a 0}
",
    ),
    (
        "a frozen constructor applied once",
        "--- compute = import \"compute.lichen\" ---
compute.A{.n 4, .I 1}
",
    ),
    (
        "the frozen kernel struct applied once",
        "--- compute = import \"compute.lichen\" ---
compute.K{.native 1, .I 2, .O 3}
",
    ),
    ("parallel with a trailing hole", PARALLEL_TRAILING_HOLE),
    ("parallel with three arguments", PARALLEL_THREE),
    ("parallel with an output instance", PARALLEL_INSTANCE),
    (
        "a conditional applied to a hole",
        "f := (x => if x <= 3 then 10 else 20)
f _
",
    ),
    (
        "an arithmetic applied to a hole",
        "f := (x => x + 1)
f _
",
    ),
    (
        "the jit launch of a conditional",
        "--- compute = import \"compute.lichen\" ---
k := compute.jit (x => if x <= 3 then 10 else 20)
compute.launch k 5
",
    ),
    (
        "the jit launch of an arithmetic",
        "--- compute = import \"compute.lichen\" ---
k := compute.jit (x => x + 1)
compute.launch k 5
",
    ),
];

/// The three parallel spellings, each with the kernel the cases share: a
/// trailing hole, the wrapper's own arity, and an output instance.
const PARALLEL_TRAILING_HOLE: &str = r#"--- compute = import "compute.lichen" ---
In  := _x => {.a @any : Int}
Out := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Par := _x => {.n @any : Int, .in In _}
f := (k <: Par) => {
  i := compute.range k.n
  y := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((Out){.z (compute.write (compute.Write{.to y, .at i, .value i + i}))})
}
k := compute.parallel f "cpu" Out _
k
"#;

const PARALLEL_THREE: &str = r#"--- compute = import "compute.lichen" ---
In  := _x => {.a @any : Int}
Out := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Par := _x => {.n @any : Int, .in In _}
f := (k <: Par) => {
  i := compute.range k.n
  y := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((Out){.z (compute.write (compute.Write{.to y, .at i, .value i + i}))})
}
k := compute.parallel f "cpu" Out
k
"#;

const PARALLEL_INSTANCE: &str = r#"--- compute = import "compute.lichen" ---
In  := _x => {.a @any : Int}
Out := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Par := _x => {.n @any : Int, .in In _}
f := (k <: Par) => {
  i := compute.range k.n
  y := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((Out){.z (compute.write (compute.Write{.to y, .at i, .value i + i}))})
}
k := compute.parallel f "cpu" (Out _)
k
"#;

/// The program the compute suite's parallel case runs.
const FAILING: &str = r#"--- compute = import "compute.lichen" ---
In  := _x => {.a @any : Int}
Out := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Par := _x => {.n @any : Int, .in In _}
f := (k <: Par) => {
  i := compute.range k.n
  y := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((Out){.z (compute.write (compute.Write{.to y, .at i, .value i + i}))})
}
k := compute.parallel f "cpu" Out _
out := (compute.plrun k (compute.A{.n 4, .I In{.a 0}}) : Struct)
(compute.read (compute.Read{.from out.z, .at 2}) : Int)
"#;

#[test]
fn probe_parallel_pipeline() {
    for (name, source) in CASES {
        println!("[{name}] {}", rendered(source));
    }
    println!(
        "[the failing program]
{}",
        recorded(FAILING)
    );
}
