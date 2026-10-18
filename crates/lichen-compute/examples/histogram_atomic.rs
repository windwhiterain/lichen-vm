//! Temporary: a contested scatter, with and without the atomic add.
//! See docs/notes/gpu-algorithms-ladder.md §5.

use lichen_compute::ComputeValue;
use lichen_compute::language::{LangProgram, new_store};
use lichen_kernel_ir::ScalarClass;
use lichen_language::package::PackageStore;
use lichen_language::preprocess::preprocess;
use lichen_language::session::{BufferSession, SessionReport};
use lichen_lowlevel::{AnyHandle, AnyNodeId};
use lichen_utils::extend::AsEnum;

/// The same histogram, written with `verb` and the struct `structure` names.
fn source(verb: &str, structure: &str, element: &str, value: &str) -> String {
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
}}) "cpu"
raw = (compute.plrun hist ((compute.A In)(.n count, .I In(.n count))) : Out)
raw.hist.native
"#
    )
}

fn value_of(report: &SessionReport<LangProgram>) -> String {
    let Some(build) = report.build.as_ref() else {
        return "<no build>".to_string();
    };
    let Some(value) = build.module.node_value(AnyNodeId::Dynamic(build.root_val)) else {
        return "<raw none>".to_string();
    };
    lichen_language::render::print_value_lang(&build.module, value, build.root_ty)
}

/// The root buffer's elements, in the buffer's own class.
fn elements(report: &SessionReport<LangProgram>) -> Vec<f64> {
    let Some(build) = report.build.as_ref() else {
        return Vec::new();
    };
    let Some(value) = build.module.node_value(AnyNodeId::Dynamic(build.root_val)) else {
        return Vec::new();
    };
    let Some(compute) = AsEnum::<ComputeValue>::as_enum(&value) else {
        return Vec::new();
    };
    let ComputeValue::Buffer(handle, class) = compute else {
        return Vec::new();
    };
    // SAFETY: the value was read out of the module on a live borrow, so the payload's
    // home block is alive for this read.
    let bytes = match handle {
        AnyHandle::Static(handle) => unsafe { &*handle.as_ptr() },
        AnyHandle::Dynamic(handle) => unsafe { &*handle.as_ptr() },
    };
    bytes
        .chunks_exact(class.byte_width())
        .map(|word| match class {
            ScalarClass::Int => u64::from_le_bytes(word.try_into().unwrap_or_default()) as f64,
            ScalarClass::Float => f64::from(f32::from_bits(u32::from_le_bytes(
                word.try_into().unwrap_or_default(),
            ))),
            ScalarClass::BFloat16 => 0.0,
        })
        .collect()
}

fn compile(
    store: &mut PackageStore<LangProgram>,
    session: &mut BufferSession<LangProgram>,
    source: &str,
) -> (String, Vec<f64>) {
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
    (value_of(&report), elements(&report))
}

fn arm(verb: &str, structure: &str, element: &str, value: &str) {
    let mut store = new_store(None).expect("the compute package registers");
    let mut session =
        BufferSession::<LangProgram>::with_registry("", "histogram.lichen", store.registry());
    let text = source(verb, structure, element, value);
    let (rendered, elements) = compile(&mut store, &mut session, &text);
    let sum: f64 = elements.iter().sum();
    println!(
        "{verb} ({structure}, {element}): {rendered}, workers {}, counts {:?}, sum {sum}",
        lichen_compute::compute::parallel_launch_workers(),
        &elements[..elements.len().min(8)]
    );
}

fn main() {
    arm("write_atomic", "WriteAtomic", "Int", "1");
    arm("write", "Write", "Int", "1");
    arm("write_atomic", "WriteAtomic", "Float", "0.5");
    arm("write", "Write", "Float", "0.5");
}
