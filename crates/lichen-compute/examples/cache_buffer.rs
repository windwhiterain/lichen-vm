//! Temporary: a compute `Buffer` survives a freeze and reads back identically.
//! See heavy-value-cache.md §9.5.
//!
//! # Invariant
//!
//! No device is opened: a `Buffer` is a packed payload in the block arena, so the question
//! "does a heavy compute value survive a cell freeze" is answerable without one.

use std::collections::HashMap;
use std::time::Instant;

use lichen_compute::ComputeValue;
use lichen_compute::language::{LangProgram, LangValue, ProgramCodec};
use lichen_kernel_ir::ScalarClass;
use lichen_language::persist::{deserialize_artifact_with, serialize_artifact_with};
use lichen_lowlevel::{AnyNodeId, FreezeMode, Module, StaticNodeId};
use lichen_utils::extend::AsEnum;

/// The payload's elements, decoded at the class the value carries.
fn items(value: &LangValue) -> Option<Vec<i64>> {
    let compute = AsEnum::<ComputeValue>::as_enum(value)?;
    let ComputeValue::Buffer(handle, class) = compute else {
        return None;
    };
    // SAFETY: the handle names a payload in a live arena, dynamic or artifact.
    let bytes = unsafe { std::slice::from_raw_parts(handle.as_ptr(), handle.len()) };
    Some(
        bytes
            .chunks_exact(class.byte_width())
            .map(|element| match class {
                ScalarClass::Int => i64::from_le_bytes(element.try_into().unwrap_or_default()),
                ScalarClass::Float => {
                    i64::from(u32::from_le_bytes(element.try_into().unwrap_or_default()))
                }
            })
            .collect(),
    )
}

fn main() {
    let elements: usize = std::env::var("ELEMENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4096);
    let mut module = Module::<LangProgram>::new();
    let root = module.add_block(None);
    let words: Vec<u8> = (0..elements)
        .flat_map(|index| (index as i64).to_le_bytes())
        .collect();
    let payload = module.alloc_payload(&words, root);
    let value: LangValue = ComputeValue::Buffer(payload, ScalarClass::Int).into();
    let node = module.add_node(root, None, Some(value));

    let before = module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|value| items(&value))
        .expect("the node holds a buffer");
    println!(
        "== a compute Buffer of {elements} Int elements ({} bytes) ==",
        elements * 8,
    );

    let registry = module.registry.clone();
    for (label, mode) in [
        ("Portable", FreezeMode::Portable),
        ("ReadOnly", FreezeMode::ReadOnly),
    ] {
        let start = Instant::now();
        let (index, key, artifact) = {
            let mut registry = registry
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let key = registry.allocate_cell_key();
            let freeze = registry.freeze_closure_mapped(&module, key, &[node], [0; 32], mode);
            let package = registry
                .get(key)
                .expect("the freeze registered the artifact");
            (
                freeze.node_map[&node],
                key,
                (package.module.nodes.len(), package.module.arena.len()),
            )
        };
        let freeze_ms = start.elapsed().as_secs_f64() * 1000.0;

        let back = module
            .node_value(AnyNodeId::Static(StaticNodeId { module: key, index }))
            .and_then(|value| items(&value));
        let equal = back.as_ref().is_some_and(|back| *back == before);
        println!(
            "{label:<9} freeze {freeze_ms:6.2} ms, artifact {} nodes / {} arena bytes, \
             read back {}, equal {equal}",
            artifact.0,
            artifact.1,
            back.map_or("no".to_string(), |back| format!("{} elements", back.len())),
        );
    }

    // The persistence half: a collected buffer is host bytes, so it may file as an artifact.
    let (key, artifact, index) = {
        let mut registry = registry
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = registry.allocate_cell_key();
        let freeze =
            registry.freeze_closure_mapped(&module, key, &[node], [0; 32], FreezeMode::Portable);
        let index = freeze.node_map[&node];
        let package = registry
            .get(key)
            .expect("the freeze registered the artifact");
        (key, package.module.clone(), index)
    };
    let mut modules: HashMap<_, _> = HashMap::new();
    modules.insert(key, artifact);
    match serialize_artifact_with(&modules[&key], &modules, [0; 32], index, ProgramCodec) {
        Err(reason) => println!("round trip: refused: {reason}"),
        Ok(bytes) => {
            let reloaded = deserialize_artifact_with(&bytes, key, [0; 32], &modules, ProgramCodec);
            match reloaded {
                Err(reason) => println!(
                    "round trip: {} bytes, reload refused: {reason}",
                    bytes.len()
                ),
                Ok((reloaded, export)) => {
                    let back = reloaded.nodes[export.index]
                        .value
                        .and_then(|value| items(&value));
                    println!(
                        "round trip: {} artifact bytes, reloaded {}, equal {}",
                        bytes.len(),
                        back.as_ref()
                            .map_or("no".to_string(), |back| format!("{} elements", back.len())),
                        back.as_ref().is_some_and(|back| *back == before),
                    );
                }
            }
        }
    }
}
