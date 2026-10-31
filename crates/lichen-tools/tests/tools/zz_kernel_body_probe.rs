//! Probe: the instructions a kernel body compiles to.
//! `docs/notes/class-carries-the-call.md`

use lichen_compute::fusion_claim::{fragment_of, kernel_id};
use lichen_compute::language::LangProgram;

#[test]
fn matmul_kernel_body() {
    let name = std::env::var("PROBE_EXAMPLE").unwrap_or_else(|_| "matmul.lichen".into());
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    let source = lichen_tools::readme::read_normalized(&path);
    let mut store = lichen_compute::language::new_store(None).expect("the package registers");
    let (module, _value, _root) =
        lichen_language::testing::run_at::<LangProgram>(&source, Some(&path), &mut store);
    let mut seen: Vec<usize> = Vec::new();
    for (id, _node) in module.nodes.iter() {
        let Ok(kernel) = kernel_id(&module, id) else {
            continue;
        };
        if seen.contains(&kernel) {
            continue;
        }
        seen.push(kernel);
        let Ok(fragment) = fragment_of(kernel) else {
            continue;
        };
        println!("PROBE params: {:?}", fragment.body.parameters());
        for (index, value) in fragment.body.values.iter().enumerate() {
            println!("  {index}: {value:?}");
        }
    }
}
