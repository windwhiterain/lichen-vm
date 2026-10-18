//! A probe for the `@gc` trigger: a scope block's census. Prints, asserts nothing.

use std::collections::{HashMap, HashSet};

use lichen_highlevel::ir::ExprKind;
use lichen_language::program::LangProgram;
use lichen_lowlevel::{AnyFunctionId, AnyNodeId, BlockId, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// The class members of `node`, headed by the representative.
fn class_members(module: &Module<LangProgram>, node: NodeId) -> Vec<NodeId> {
    if !module.nodes.contains_key(node) {
        return Vec::new();
    }
    let mut current = Some(module.class_root(node));
    let mut members = Vec::new();
    // A member list visits each node at most once, so it cannot outrun the node table.
    for _ in 0..=module.nodes.len() {
        let Some(member) = current else {
            return members;
        };
        if !module.nodes.contains_key(member) {
            return members;
        }
        members.push(member);
        current = module.node_equality(member).next();
    }
    members
}

/// Whether `block` is `source` or descends from it.
fn descends_from(module: &Module<LangProgram>, block: BlockId, source: BlockId) -> bool {
    let mut current = Some(block);
    for _ in 0..=module.blocks.len() {
        let Some(block) = current else { return false };
        if block == source {
            return true;
        }
        current = module.blocks[block].parent;
    }
    false
}

/// The set `garbage_collect_node` would keep, mirroring its edges.
fn walk(module: &Module<LangProgram>, node: NodeId, source: BlockId, keep: &mut HashSet<NodeId>) {
    if !module.nodes.contains_key(node) || !keep.insert(node) {
        return;
    }
    for member in class_members(module, node) {
        if module.nodes.contains_key(member) {
            walk(module, member, source, keep);
        }
    }
    if !descends_from(module, module.node_block(node), source) {
        return;
    }
    // The operand edge is followed only while the operator still owes its answer.
    if module.has_no_result_yet(node)
        && let Some(operand) = module
            .node_operation(node)
            .and_then(|operation| operation.operand)
    {
        walk(module, operand, source, keep);
    }
    let Some(value) = module.node_value(AnyNodeId::Dynamic(node)) else {
        return;
    };
    match value.as_enum() {
        Some(LowValue::Array(array)) => {
            // SAFETY: a live payload of the module under test.
            for item in unsafe { array.items() } {
                if let Some(node) = item.node.dynamic() {
                    walk(module, node, source, keep);
                }
            }
        }
        Some(LowValue::Table(table)) => {
            // SAFETY: a live payload of the module under test.
            for item in unsafe { table.items() } {
                for id in [item.key, item.value] {
                    if let Some(node) = id.dynamic() {
                        walk(module, node, source, keep);
                    }
                }
            }
        }
        Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
            let Some(function) = module.functions.get(function) else {
                return;
            };
            for &id in &function.nodes {
                walk(module, id, source, keep);
            }
            for &id in &function.asserts {
                walk(module, id, source, keep);
            }
        }
        _ => {}
    }
}

/// Every node `node` names through its own edges, as `(kind, node)`.
fn named_by_node(module: &Module<LangProgram>, node: NodeId) -> Vec<(&'static str, NodeId)> {
    let mut named = Vec::new();
    if let Some(operand) = module
        .node_operation(node)
        .and_then(|operation| operation.operand)
    {
        named.push(("operand", operand));
    }
    let Some(value) = module.node_value(AnyNodeId::Dynamic(node)) else {
        return named;
    };
    match value.as_enum() {
        Some(LowValue::Array(array)) => {
            // SAFETY: a live payload of the module under test.
            for item in unsafe { array.items() } {
                if let Some(node) = item.node.dynamic() {
                    named.push(("array item", node));
                }
            }
        }
        Some(LowValue::Table(table)) => {
            // SAFETY: a live payload of the module under test.
            for item in unsafe { table.items() } {
                for (kind, id) in [("table key", item.key), ("table value", item.value)] {
                    if let Some(node) = id.dynamic() {
                        named.push((kind, node));
                    }
                }
            }
        }
        Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
            let Some(function) = module.functions.get(function) else {
                return named;
            };
            for &id in &function.nodes {
                named.push(("function node", id));
            }
            for &id in &function.asserts {
                named.push(("function assert", id));
            }
        }
        _ => {}
    }
    named
}

/// One scope block's census, with every external edge into it and whether the walk keeps the
/// target.
fn report_scope(module: &Module<LangProgram>, block: BlockId, root: NodeId) {
    let nodes: Vec<NodeId> = module
        .blocks
        .get(block)
        .map(|block| block.nodes.clone())
        .unwrap_or_default();
    let parent = module.blocks.get(block).and_then(|block| block.parent);
    let owes = nodes
        .iter()
        .filter(|&&node| module.has_no_result_yet(node))
        .count();
    println!(
        "  block {block:?} root {root:?} parent {parent:?}: nodes={} owes={owes}",
        nodes.len()
    );
    if !module.nodes.contains_key(root) {
        println!("    root released");
        return;
    }
    println!(
        "    root: block={:?} value={} deep={:?}",
        module.node_block(root),
        module.node_value(AnyNodeId::Dynamic(root)).is_some(),
        module.node_evaluated_deep(root)
    );
    // The nodes that hold no value at all: what an undecided-count trigger would read.
    let mut undecided = 0;
    for &node in &nodes {
        let has_value = module
            .node_value(AnyNodeId::Dynamic(node))
            .is_some_and(|value| !matches!(value.as_enum(), Some(LowValue::Any)));
        if has_value {
            continue;
        }
        undecided += 1;
        let operation = module.node_operation(node);
        let operand = operation.and_then(|operation| operation.operand);
        let operand_note = operand.map(|operand| {
            (
                format!("{operand:?}"),
                module.node_value(AnyNodeId::Dynamic(operand)).is_some(),
                module.has_no_result_yet(operand),
            )
        });
        println!(
            "    undecided {node:?}: op={} owes={} origin={:?} depth={} operand={operand_note:?}",
            operation.is_some(),
            module.has_no_result_yet(node),
            module.node_origin(node),
            module.node_depth(node),
        );
    }
    println!("    undecided nodes: {undecided}");
    let mut keep = HashSet::new();
    walk(module, root, block, &mut keep);
    let kept = nodes.iter().filter(|node| keep.contains(node)).count();
    println!(
        "    walk from the root keeps {kept} of {} nodes; releases {}",
        nodes.len(),
        nodes.len() - kept
    );
    // Every live node outside the block that names one inside it.
    let mut edges: HashMap<&'static str, usize> = HashMap::new();
    let mut unsafe_edges: HashMap<&'static str, Vec<(NodeId, NodeId)>> = HashMap::new();
    let mut note = |kind: &'static str, source: NodeId, target: NodeId, kept: bool| {
        *edges.entry(kind).or_default() += 1;
        if !kept {
            unsafe_edges.entry(kind).or_default().push((source, target));
        }
    };
    for (node, _) in module.nodes.iter() {
        if module.node_block(node) == block {
            continue;
        }
        for (kind, target) in named_by_node(module, node) {
            if module.nodes.contains_key(target) && module.node_block(target) == block {
                note(kind, node, target, keep.contains(&target));
            }
        }
    }
    // A class that crosses the boundary: each block node's class's outside members.
    for &member in &nodes {
        if !module.nodes.contains_key(member) {
            continue;
        }
        for outside in class_members(module, member) {
            if module.node_block(outside) == block {
                continue;
            }
            note("class", outside, member, keep.contains(&member));
        }
    }
    // The two module-level registries that name nodes.
    for entry in &module.asserts {
        for (kind, target) in [
            ("pending assert condition", Some(entry.condition)),
            ("pending assert template", entry.template.dynamic()),
        ] {
            let Some(target) = target else { continue };
            if module.node_block(target) == block {
                note(kind, target, target, keep.contains(&target));
            }
        }
    }
    println!("    edges into the block: {edges:?}");
    for (kind, list) in &unsafe_edges {
        println!(
            "    not kept by the walk: {kind} x{} {:?}",
            list.len(),
            &list[..list.len().min(4)]
        );
    }
}

fn census(label: &str, source: &str) {
    println!("=== {label}");
    println!("  source: {source:?}");
    let report = lichen_language::compile(source);
    println!(
        "  report: ok={} diagnostics={:?}",
        report.ok(),
        report.diagnostics.len()
    );
    let Some(build) = report.build else {
        println!("  does not check");
        return;
    };
    println!(
        "  failures: assert_errors={} eval_errors={} unify_errors={} apply_errors={}",
        build.module.assert_errors.len(),
        build.module.eval_errors.len(),
        build.module.unify_errors.len(),
        build.module.apply_errors.len()
    );
    let mut module = build.module;
    // Every `@gc` expression the IR wrote, with the root the checker compiled it to.
    let mut scopes = Vec::new();
    for index in 0..build.ir.expr.len() {
        let expression = lichen_highlevel::ir::ExprId(index as u32);
        if matches!(build.ir[expression].kind, ExprKind::Gc { .. })
            && let Some(root) = build.state[expression].term
        {
            scopes.push(root);
        }
    }
    println!(
        "  after compile: blocks={} nodes={} scopes={:?}",
        module.blocks.len(),
        module.nodes.len(),
        scopes
            .iter()
            .map(|root| (*root, module.node_block(*root)))
            .collect::<Vec<_>>()
    );
    let value = module.evaluate_node_deep(build.root_val, None);
    let _ = module.evaluate_node_deep(build.root_ty, None);
    println!(
        "  after the run: blocks={} nodes={} root answer={}",
        module.blocks.len(),
        module.nodes.len(),
        value.is_some()
    );
    for root in scopes {
        if !module.nodes.contains_key(root) {
            println!("  scope root {root:?}: released");
            continue;
        }
        let block = module.node_block(root);
        report_scope(&module, block, root);
    }
    let answer =
        lichen_language::run::evaluate(source).map_err(|diagnostics| format!("{diagnostics:?}"));
    println!("  run: {answer:?}");
}

#[test]
fn gc_trigger_readings() {
    census(
        "block, value used later",
        "x = @gc { a = 2; b = 3; a + b }\nx + 1",
    );
    census(
        "block, value is the root",
        "x = @gc { a = 2; b = 3; a + b }\nx",
    );
    census(
        "call, value used later",
        "f = n => n + 1\nx = @gc f 41\nx + 1",
    );
    census("call, value is the root", "f = n => n + 1\nx = @gc f 41\nx");
    census("array in a scope", "x = @gc [1, 2, 3]\nx");
    census(
        "unused statement in a scope",
        "x = @gc { a = [1, 2, 3]\n2 }\nx",
    );
    census("lambda in a scope", "x = @gc { f = y => y + 1\n5 }\nx");
    census(
        "scope inside a body, two calls",
        "f = n => @gc { a = n + 1\nb = a * 2\nb }\nf 3 + f 4",
    );
    census("undecided scope", "@gc { let a = _\na }");
    census("assert in a scope", "x = @gc { @assert (2 > 1)\n3 }\nx");
    census(
        "false assert in a scope",
        "x = @gc { @assert (1 > 2)\n3 }\nx",
    );
    census(
        "unused call with an assert in a scope",
        "f = n => { @assert (n > 0)\nn }\nx = @gc { y = f 0\n7 }\nx",
    );
}
