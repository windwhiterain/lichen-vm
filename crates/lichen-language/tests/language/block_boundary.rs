//! The block boundary: what a payload or operand may name across a block edge.

use lichen_language::compile;
use lichen_language::program::LangProgram;
use lichen_lowlevel::{AnyNodeId, BlockId, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// Whether `item`, named from `block`, is guaranteed to outlive the referrer.
///
/// # Invariant
/// A referenced node must live in the referrer's block, in an ancestor (the ancestor
/// outlives it), or in a parentless block (a function body or the module root, never
/// released) — or be the declared root of a child scope, whose parent's read collects it.
fn may_be_named(module: &Module<LangProgram>, block: BlockId, item: NodeId) -> bool {
    let item_block = module.node_block(item);
    if module.blocks[item_block].parent.is_none() {
        return true;
    }
    let mut current = Some(block);
    while let Some(current_block) = current {
        if current_block == item_block {
            return true;
        }
        current = module.blocks[current_block].parent;
    }
    module.blocks[item_block].scope_root == Some(item)
}

/// The cross-block references a collect could leave dangling.
fn violations(module: &Module<LangProgram>) -> Vec<(NodeId, BlockId, NodeId, BlockId)> {
    let mut found = Vec::new();
    let mut note = |from: NodeId, to: AnyNodeId| {
        let AnyNodeId::Dynamic(to) = to else {
            return;
        };
        let block = module.node_block(from);
        if !module.nodes.contains_key(to) || !may_be_named(module, block, to) {
            found.push((from, block, to, module.node_block(to)));
        }
    };
    for (node, _) in module.nodes.iter() {
        if let Some(value) = module.node_value(AnyNodeId::Dynamic(node)) {
            match value.as_enum() {
                Some(LowValue::Array(array)) => {
                    // SAFETY: the payload of a live node, released only with its block.
                    for item in unsafe { array.items() } {
                        note(node, item.node);
                    }
                }
                Some(LowValue::Table(table)) => {
                    // SAFETY: the payload of a live node, released only with its block.
                    for item in unsafe { table.items() } {
                        note(node, item.key);
                        note(node, item.value);
                    }
                }
                _ => {}
            }
        }
        if let Some(operand) = module.node_operation(node).and_then(|entry| entry.operand) {
            // SAFETY: the operand array of a live node, released only with its block.
            for item in unsafe { module.array_items(operand) }.unwrap_or(&[]) {
                note(node, item.node);
            }
        }
    }
    found
}

/// The checker's built IR never names a non-root node of a descendant block.
///
/// # Invariant
/// `@gc` gives the parent `Index(expr, 0/1)` reads, so the declared root is the only child
/// node a parent names. The aliasing repairs were measured and rejected: a parent cell
/// united with the inner's answers `none` for `@gc f 41`, and the root as the value handle
/// breaks a block scope. See `docs/notes/gc-block.md`.
#[test]
fn the_checker_builds_every_cross_block_reference_to_outlive_its_referrer() {
    let corpus = [
        "@gc { a = 1; b = 2 }",
        "x = @gc { a = 2; b = 3; a + b }\nx",
        "x = @gc { f = y => y + 1\n5 }\nx",
        "f = n => @gc { a = n + 1\nb = a * 2\nb }\nf 3 + f 4",
        "y = [@gc (x => x + 1)]\n(y[0]) 3",
        "mk = n => @gc { f = x => x + n\nf }\na = mk 1\nb = a 2\nb",
        "x = [@gc 1, 2]\nx[0]",
        "x = if 1 then @gc 1 else 2\nx",
        "let a = 2\nlet b = 3\na + b",
    ];
    for source in corpus {
        let report = compile(source);
        assert!(
            report.ok(),
            "expected {source:?} to check, got: {:?}",
            report.diagnostics
        );
        let build = report.build.expect("a checked program has a build");
        let found = violations(&build.module);
        assert!(
            found.is_empty(),
            "{source:?} names across the block boundary: {found:?}"
        );
    }
}
