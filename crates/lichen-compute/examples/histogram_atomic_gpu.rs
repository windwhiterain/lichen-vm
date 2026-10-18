//! The contested scatter on both backends. docs/notes/gpu-algorithms-ladder.md §5.

use lichen_compute::language::{LangProgram, new_store};
use lichen_kernel_ir::ScalarClass;
use lichen_language::package::PackageStore;
use lichen_language::preprocess::preprocess;
use lichen_lowlevel::{AnyNodeId, Module, NodeId};

/// The same histogram, written with `verb` and the struct `structure` names, on `backend`.
fn source(backend: &str, verb: &str, structure: &str, element: &str, value: &str) -> String {
    format!(
        r#"---
  compute = import "compute.lichen"
---
count = 65536
buckets = 8
In = struct{{.n Int}}
Out = struct{{.hist (compute.Buf {element})}}
P = compute.P (compute.KT _)(.I In, .O Out)
hist = compute.parallel ((k : P) => {{
  i = compute.range k.n
  j = i - ((i / buckets) * buckets)
  compute.{verb} ((compute.{structure} _)(.to k.out.hist, .at j, .value {value}))
}}) "{backend}"
raw = (compute.plrun hist ((compute.A In)(.n count, .I In(.n count))) : Out)
compute.collect raw.hist
"#
    )
}

fn value_of(module: &Module<LangProgram>, root: NodeId, root_ty: NodeId) -> String {
    let Some(value) = module.node_value(AnyNodeId::Dynamic(root)) else {
        return "<raw none>".to_string();
    };
    // **A shape, not the array**: rendering 65536 elements would bury the counts.
    let rendered = lichen_language::render::print_value_lang(module, value, root_ty);
    let head: String = rendered.chars().take(24).collect();
    format!("{head}… ({} chars)", rendered.len())
}

/// The collected buckets: an `Int` element is a `usize`, a `Float` one an `f32`.
fn elements(module: &Module<LangProgram>, root: NodeId, class: ScalarClass) -> Vec<f64> {
    let Some(value) = module.node_value(AnyNodeId::Dynamic(root)) else {
        return Vec::new();
    };
    lichen_language::testing::array_values(module, &value)
        .iter()
        .map(|item| match class {
            ScalarClass::Int => lichen_language::testing::usize_of(item) as f64,
            _ => f64::from(lichen_language::testing::float_of(item)),
        })
        .collect()
}

/// Compile and run `text`, reporting rather than panicking: a refusal must survive.
fn compile(
    store: &mut PackageStore<LangProgram>,
    text: &str,
    class: ScalarClass,
) -> (String, Vec<f64>, Vec<String>) {
    let (preprocessed, diagnostics) = preprocess(text, None, store);
    let mut said: Vec<String> = diagnostics
        .iter()
        .take(4)
        .map(|diagnostic| format!("preprocess: {}", diagnostic.message))
        .collect();
    let line_starts = lichen_language::lex::line_starts(text);
    let report = lichen_language::compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        lichen_highlevel::no_native_ops(),
    );
    said.extend(
        report
            .diagnostics
            .iter()
            .take(4)
            .map(|diagnostic| format!("diagnostic: {}", diagnostic.message)),
    );
    let Some(build) = report.build else {
        said.push("<the program did not check>".to_string());
        return ("<no build>".to_string(), Vec::new(), said);
    };
    said.extend(
        build
            .module
            .extension_diagnostics
            .iter()
            .take(8)
            .map(|entry| format!("refused {}: {}", entry.category, entry.message)),
    );
    let mut module = build.module;
    let root = build.root_val;
    let root_ty = build.root_ty;
    // **The root is evaluated here**: a dispatch is recorded while the value is produced.
    module.evaluate_node_deep(root, None);
    (
        value_of(&module, root, root_ty),
        elements(&module, root, class),
        said,
    )
}

fn arm(backend: &str, verb: &str, structure: &str, element: &str, value: &str) {
    let class = match element {
        "Int" => ScalarClass::Int,
        _ => ScalarClass::Float,
    };
    let mut store = new_store(None).expect("the compute package registers");
    let text = source(backend, verb, structure, element, value);
    let (rendered, counts, said) = compile(&mut store, &text, class);
    let sum: f64 = counts.iter().sum();
    println!(
        "{backend} {verb} ({structure}, {element}): {rendered}, workers {}, counts {:?}, sum {sum}",
        lichen_compute::compute::parallel_launch_workers(),
        &counts[..counts.len().min(8)]
    );
    for line in said {
        println!("    {line}");
    }
}

fn main() {
    // `lichen-compute` never names the GPU crate, so the device is installed here.
    match lichen_compute_gpu::install_default() {
        Ok(()) => println!(
            "installed backend {:?}",
            lichen_compute_gpu::installed_backend_name()
        ),
        Err(detail) => println!("no gpu backend installed: {detail}"),
    }
    for backend in ["cpu", "gpu"] {
        arm(backend, "write_atomic", "WriteAtomic", "Int", "1");
        arm(backend, "write", "Write", "Int", "1");
        arm(backend, "write_atomic", "WriteAtomic", "Float", "0.5");
        arm(backend, "write", "Write", "Float", "0.5");
    }
    // Uninstalling drops the context, so its device buffers go back at once.
    lichen_compute_gpu::uninstall();
}
