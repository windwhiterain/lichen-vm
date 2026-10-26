//! Probe: what the constructor pipeline's parallel path records, printing and asserting nothing.

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
                "\n    unify[{index}] root_a={:?} root_b={:?} a={:?} b={:?} steps={:?}",
                error.root_a, error.root_b, error.a, error.b, error.steps
            ));
            out.push_str(&format!("\n      root_a shape={}", shape(module, error.root_a)));
            out.push_str(&format!("\n      root_b shape={}", shape(module, error.root_b)));
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

const PARALLEL: &str = r#"
---
  compute = import "compute.lichen"
---
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

const CASES: &[(&str, &str)] = &[
    ("plain struct instance", "In := _x => {.a @any : Int}\nIn{.a 0}\n"),
    ("plain struct applied", "In := _x => {.a @any : Int}\nIn _\n"),
    (
        "frozen A instance with import",
        "--- compute = import \"compute.lichen\" ---\ncompute.A{.n 4, .I 1}\n",
    ),
    (
        "frozen K instance",
        "--- compute = import \"compute.lichen\" ---\ncompute.K{.native 1, .I 2, .O 3}\n",
    ),
    (
        "parallel only",
        r#"--- compute = import "compute.lichen" ---
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
"#,
    ),
    (
        "parallel without trailing hole",
        r#"--- compute = import "compute.lichen" ---
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
"#,
    ),
    (
        "parallel with parenthesised instance",
        r#"--- compute = import "compute.lichen" ---
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
"#,
    ),
    (
        "parallel with parenthesised instance, then plrun",
        r#"--- compute = import "compute.lichen" ---
In  := _x => {.a @any : Int}
Out := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Par := _x => {.n @any : Int, .in In _}
f := (k <: Par) => {
  i := compute.range k.n
  y := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((Out){.z (compute.write (compute.Write{.to y, .at i, .value i + i}))})
}
k := compute.parallel f "cpu" (Out _)
out := (compute.plrun k (compute.A{.n 4, .I In{.a 0}}) : Struct)
(compute.read (compute.Read{.from out.z, .at 2}) : Int)
"#,
    ),
    (
        "parallel with a constant body",
        r#"--- compute = import "compute.lichen" ---
In  := _x => {.a @any : Int}
Out := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Par := _x => {.n @any : Int, .in In _}
f := (k <: Par) => { 1 }
k := compute.parallel f "cpu" Out
k
"#,
    ),
    (
        "parallel with a range body",
        r#"--- compute = import "compute.lichen" ---
In  := _x => {.a @any : Int}
Out := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Par := _x => {.n @any : Int, .in In _}
f := (k <: Par) => { i := compute.range k.n; i }
k := compute.parallel f "cpu" Out
k
"#,
    ),
    (
        "parallel with a write body, no Out",
        r#"--- compute = import "compute.lichen" ---
In  := _x => {.a @any : Int}
Out := _x => {.z (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Par := _x => {.n @any : Int, .in In _}
f := (k <: Par) => {
  i := compute.range k.n
  y := compute.buf (compute.BufParam{.type Int, .length k.n})
  (compute.write (compute.Write{.to y, .at i, .value i + i}))
}
k := compute.parallel f "cpu" Out
k
"#,
    ),
];

#[test]
fn probe_parallel_pipeline() {
    println!("[parallel]\n{}", recorded(PARALLEL));
    for (name, source) in CASES {
        println!("[{name}] {}", rendered(source));
    }
}
