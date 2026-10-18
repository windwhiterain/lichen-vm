//! Temporary: a reloaded cell answers what the compile that froze it did.
//! See docs/notes/heavy-value-cache.md §9.12.
//!
//! # Invariant
//!
//! The second registry is empty before the load, and the source it compiles is unchanged, so a
//! difference in the root value can only come from the reloaded cell.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use lichen_language::cells::CellStore;
use lichen_language::compile_with_cells;
use lichen_language::persist::{deserialize_artifact_with, serialize_artifact_with};
use lichen_language::program::{LangProgram, ProgramCodec};
use lichen_language::session::BufferSession;
use lichen_language_parser::path::Path;
use lichen_lowlevel::{AnyNodeId, Registry, StaticModule, StaticNodeId};

const SOURCE: &str = "\
cache a = 1 + 2
cache b = a + 3
a + b
";

/// The root value a compile answered, rendered: the scalar the program computes.
fn root_value(report: &lichen_language::Report<LangProgram>) -> Option<String> {
    let build = report.build.as_ref()?;
    let value = build
        .module
        .node_value(AnyNodeId::Dynamic(build.root_val))?;
    Some(format!("{value:?}"))
}

fn main() {
    // The paths first: a `Report` carries no AST, so they come from a session that does.
    let mut session = BufferSession::<LangProgram>::with_source_id(SOURCE, "cell.lichen");
    let first = session.compile();
    let mut marked: Vec<Path> = Vec::new();
    lichen_language::path::for_each(&first.program, &mut |path, node| {
        if let lichen_language::path::Node::Binding(binding) = node
            && binding.cached
        {
            marked.push(path.clone());
        }
    });
    drop(session);

    let registry = Arc::new(RwLock::new(Registry::<LangProgram>::new()));
    let mut cells = CellStore::new();
    let original = compile_with_cells("cell.lichen", SOURCE, &mut cells, registry.clone());
    let expected = root_value(&original);
    println!(
        "== {} marked bindings, {} cells, root {:?} ==",
        marked.len(),
        cells.len(),
        expected
    );

    // Serialize the first marked cell's artifact, then reload it into an empty registry.
    let path = marked.first().expect("the source marks a binding").clone();
    let reference = cells.reference(&path).expect("the cell froze");
    let bytes = {
        let registry = registry
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let package = registry
            .get(reference.module)
            .expect("the cell filed an artifact");
        let mut modules: HashMap<_, Arc<StaticModule<LangProgram>>> = HashMap::new();
        modules.insert(reference.module, package.module.clone());
        serialize_artifact_with(
            &package.module,
            &modules,
            [0; 32],
            reference.index,
            ProgramCodec,
        )
        .expect("a cell artifact serializes")
    };

    let fresh = Arc::new(RwLock::new(Registry::<LangProgram>::new()));
    let empty: HashMap<_, Arc<StaticModule<LangProgram>>> = HashMap::new();
    let (module, export) =
        deserialize_artifact_with(&bytes, reference.module, [0; 32], &empty, ProgramCodec)
            .expect("the cell artifact reloads");
    fresh
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert_module(reference.module, [0; 32], module);
    fresh
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .reserve_cell_key(reference.module);

    let mut restored = CellStore::new();
    restored.record(
        path,
        "cell.lichen",
        StaticNodeId {
            module: reference.module,
            index: export,
        },
    );
    let answer = compile_with_cells("cell.lichen", SOURCE, &mut restored, fresh);
    let got = root_value(&answer);
    println!(
        "{} artifact bytes; restored root {:?}, equal {}",
        bytes.len(),
        got,
        got == expected
    );
}
