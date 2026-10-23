//! Probe: the `if` in a fold's step, and the body the loop conversion lowers.
//!
//! # Invariant
//!
//! Print-only. Each variant differs from the others in one spelling of the `if`, and the
//! reading is five things: the refusal the check recorded, the comparison nodes the graph
//! holds, each one's operand classes, the step at which `Module::parameter_value_path`'s
//! walk stops, and the count of comparison instructions the lowered body carries.
//!
//! **Where the doubling is, measured.** The graph is not where it lives: the refusing
//! spellings and the clean control hold the same 13 comparison nodes in the same classes.
//! The body is: with the selector wrapped in an addition (`(s(0) > 1) + 0`) the walk
//! lowers, and the surface operator emits `Gt` over **the routed call's own result** —
//! `Bin(Gt)[block parameter, 1]` then `Bin(Gt)[that, 1]`. The four order comparisons
//! are routed through the prelude's bindings, so one source `>` is two nodes (an
//! operator and an `Apply` of a frozen callee) and both emit; only the state case takes
//! the second one over the first.
//!
//! **Why the second emission ranges over the first, measured.** The surface operator's
//! operand is the state read, and `parameter_value_path` cannot walk it: the walk reaches
//! that `Index` node and `operands_of` fails on it (`walk … -> no operands`), so the
//! reading is [`None`]. `Lower::loop_value` therefore cannot place the read as a block
//! parameter, `Lower::value_inner` falls through to `define_in`, and `define_in` answers
//! the class member that already computed — the routed comparison. A clone's operand
//! array being unreadable is the defect; the `None` is its symptom.

use super::common;
use lichen_compute::language::LangProgram;
use lichen_highlevel::program::TypeOperator;
use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, FunctionId, LowOperator, LowValue, Module, NodeId, Operation,
    Selection,
};
use lichen_utils::extend::AsEnum;

/// The program, with the `if`'s condition and arms spelled by the caller.
fn program(condition: &str, then: &str, otherwise: &str) -> String {
    format!(
        r#"
--- compute = import "compute.lichen" ---
TIn  = struct{{.x (compute.Buf Float)}}
TOut = struct{{.y (compute.Buf Float)}}
TPar = struct{{.n Int, .taps Int, .in TIn, .out TOut}}
TArg = struct{{.n Int, .taps Int, .I TIn}}
tiny_raw = (c : TPar) => {{
  i = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (if {condition} then {then} else {otherwise})) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}}
tiny = compute.parallel (tiny_raw ~ "claim:Reduction(combine,identity,element)") "cpu" TOut
x = (compute.buffer ([1.0, 1.0] : array{{Float, 2}}) : (compute.Buf Float))
out = (compute.plrun tiny ((TArg)(.n 2, .taps 3, .I TIn(.x x))) : TOut)
compute.collect out.y : array{{Float, 2}}
"#
    )
}

/// The one word naming an operator, so a dump reads in the program's own spelling.
///
/// # Invariant
///
/// The name comes off the vocabulary's own `AsEnum` conversion — the same one the
/// checker's classification uses — so a probe cannot drift from the class it reads.
fn operator_name(operator: &TypeOperator) -> String {
    format!("{operator:?}")
}

/// Whether the name is one of the four order comparisons the probe counts.
fn is_order_comparison(name: &str) -> bool {
    matches!(name, "Gt" | "Lt" | "Geq" | "Leq")
}

/// The operator's own name, structural operators included — `Index` and `Apply`
/// are where the clone's own kind differs from what the refusal hinges on.
fn operation_name(operation: &Operation<LangProgram>) -> String {
    if let Some(operator) = AsEnum::<TypeOperator>::as_enum(&operation.operator) {
        return operator_name(&operator);
    }
    match AsEnum::<LowOperator>::as_enum(&operation.operator) {
        Some(LowOperator::Index) => "Index".to_string(),
        Some(LowOperator::Apply) => "Apply".to_string(),
        _ => "structural".to_string(),
    }
}

/// A node's own operation name, or "a value node".
fn node_name(module: &Module<LangProgram>, node: NodeId) -> String {
    match module.node_operation(node) {
        Some(operation) => operation_name(&operation),
        None => "a value node".to_string(),
    }
}

/// Whether an operand array the walk could not read is still a node of the module,
/// and whether it still holds an array value.
///
/// # Invariant
///
/// The walk stopping at an unreadable operand is a fact about **when** it is asked:
/// the clone walk answered the same operand as a readable two-element array when it
/// made it. Whether the node is still present, and whether its value is still an
/// array, tells the two apart — a released node from a replaced one.
fn operand_fate(module: &Module<LangProgram>, node: NodeId) -> String {
    let operation = module.node_operation(node);
    let operand = operation.as_ref().and_then(|operation| operation.operand);
    match operand {
        None => format!("{node:?} has no operand at all"),
        Some(operand) => {
            let present = module.nodes.contains_key(operand);
            let holds = unsafe { module.array_items(operand) }
                .map(|items| {
                    let elements: Vec<String> = items
                        .iter()
                        .map(|item| match item.node {
                            AnyNodeId::Dynamic(node) => format!("dynamic({node:?})"),
                            AnyNodeId::Static(reference) => {
                                format!("STATIC({reference:?})")
                            }
                        })
                        .collect();
                    format!("{} element(s) [{}]", items.len(), elements.join(", "))
                })
                .unwrap_or_else(|| match module.node_value(AnyNodeId::Dynamic(operand)) {
                    Some(value) => format!("a non-array value ({value:?})"),
                    None => "no value".to_string(),
                });
            format!(
                "{node:?} -> operand {operand:?}: node present {present}, holds {holds}, \
                 operand root {:?}, operand origin {:?}, node root {:?}",
                module.class_root(operand),
                module.node_origin(operand),
                module.class_root(node)
            )
        }
    }
}

/// The `Index` chain `Module::parameter_value_path` walks, step by step, so the step a
/// clone's read stops at is named rather than inferred.
///
/// # Invariant
///
/// One line per step, in the order the walk takes them: the node, whether its class
/// root is the parameter value's, and the position its subscript reads. The walk stops
/// where the reading stops, and the step it stops at is the whole question.
fn walk_state_read(module: &Module<LangProgram>, node: NodeId) -> String {
    let function = match looping_function(module) {
        Some(function) => function,
        None => return "no marked function".to_string(),
    };
    let parameter = module.functions[function].parameter;
    let value_cell = module.pair_value_half(parameter).unwrap_or(parameter);
    let value_class = module.class_root(value_cell);
    let mut steps = Vec::new();
    let mut current = node;
    for _ in 0..16 {
        let at_value = module.class_root(current) == value_class;
        steps.push(format!(
            "{current:?}({}, origin {:?}, subscript {})",
            node_name(module, current),
            module.node_origin(current),
            module
                .operands_of(current)
                .ok()
                .and_then(|operands| operands.get(1).copied())
                .and_then(|selector| module.usize_value(selector))
                .map(|position| position.to_string())
                .unwrap_or_else(|| "<undecided>".to_string()),
        ));
        if at_value {
            return format!("{} -> reaches the state's value", steps.join(" -> "));
        }
        let Some(operation) = module.node_operation(current) else {
            // **The walk bottomed out**: the origin is the node the clone
            // instantiates, so the walk continues where the template's read is.
            let Some(origin) = module.node_origin(current) else {
                return format!("{} -> no operation, no origin", steps.join(" -> "));
            };
            if origin == current {
                return format!(
                    "{} -> no operation, the origin is itself",
                    steps.join(" -> ")
                );
            }
            steps.push(format!(
                "{current:?}(a value node, origin {origin:?}, value class {value_class:?})"
            ));
            current = origin;
            continue;
        };
        if operation_name(&operation) != "Index" {
            return format!("{} -> not an Index", steps.join(" -> "));
        }
        // **The walk as it now stands**: the target through the operand array, the
        // position through `read_position`, so a frozen position answers too.
        let Some(operand) = operation.operand else {
            return format!("{} -> no operand array", steps.join(" -> "));
        };
        let Some(items) = module.operand_items(operand).ok() else {
            return format!("{} -> operand array unreadable", steps.join(" -> "));
        };
        let Some(target) = items
            .first()
            .map(|item| item.node)
            .and_then(|n| n.dynamic())
        else {
            return format!("{} -> target is not this module's", steps.join(" -> "));
        };
        let position_item = items.get(1).map(|item| item.node);
        let Some(position) = position_item.and_then(|item| module.read_position(item)) else {
            let frozen = match position_item {
                Some(AnyNodeId::Static(reference)) => format!(
                    "the frozen node holds {}",
                    module
                        .static_read(reference)
                        .map(|value| format!("{value:?}"))
                        .unwrap_or_else(|| "<nothing readable>".to_string())
                ),
                _ => "no position".to_string(),
            };
            return format!("{} -> position unreadable ({frozen})", steps.join(" -> "));
        };
        steps.push(format!("{current:?}(position {position})"));
        current = target;
    }
    format!("{} -> depth cut", steps.join(" -> "))
}

/// How many `Gt` nodes the graph holds, and how many comparison instructions the lowered
/// body carries — through a refusal as much as through a clean check.
///
/// # Invariant
///
/// A refused check still hands back the build it refused (`Report::build` is `Some` on
/// every path), so the graph is readable whether or not the body lowered; only the body
/// count is missing when the refusal came from the body walk itself.
fn counts(source: &str) -> String {
    use lichen_kernel_ir::KernelInstr;

    let report = common::check(source);
    let clean = report.ok();
    let refusal = if clean {
        "clean".to_string()
    } else {
        format!(
            "refused: {}",
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<String>>()
                .join(" | ")
        )
    };
    let Some(build) = report.build else {
        return format!("{refusal}; no build");
    };
    let mut graph_gts = 0;
    for (id, _node) in build.module.nodes.iter() {
        let Some(operation) = build.module.node_operation(id) else {
            continue;
        };
        if is_order_comparison(&operator_name(
            &AsEnum::<TypeOperator>::as_enum(&operation.operator).unwrap_or(TypeOperator::Add),
        )) {
            graph_gts += 1;
        }
    }
    // The body is only lowered when the check reached it; a refusal from the body walk
    // leaves no fragment to count, so the count is the clean check's alone.
    if !clean {
        return format!(
            "{refusal}; graph: {graph_gts} order-comparison node(s); body: not lowered"
        );
    }
    let body = common::sole_fragment(&build.module).body;
    let mut body_gts = 0;
    for block in &body.blocks {
        for &instr in &block.instrs {
            let Some(lichen_kernel_ir::ValueDef::Instr { op, .. }) =
                body.values.get(instr.0 as usize)
            else {
                continue;
            };
            match op {
                KernelInstr::Bin(_, op)
                    if !matches!(
                        op,
                        lichen_kernel_ir::KernelBin::Add
                            | lichen_kernel_ir::KernelBin::Sub
                            | lichen_kernel_ir::KernelBin::Mul
                            | lichen_kernel_ir::KernelBin::Div
                    ) =>
                {
                    body_gts += 1
                }
                _ => {}
            }
        }
    }
    format!(
        "{refusal}; graph: {graph_gts} order-comparison node(s); body: {body_gts} comparison(s)"
    )
}

/// The node every route of a comparison shares: its own operands, the routed call that
/// also carries them, and the conditional that selects on it.
///
/// # Invariant
///
/// The four order comparisons are **routed through the prelude's bindings**, so one source
/// `>` is two nodes in the graph — a `TypeOperator` operator and an `Apply` of a frozen
/// callee — and both emit a comparison when the body walk meets them. A doubled comparison
/// is therefore a question about which operands each of the two routes reads, so all three
/// are printed together, each against its class root.
fn dump_comparisons(source: &str) -> String {
    let report = common::check(source);
    let Some(build) = report.build else {
        return "no build".to_string();
    };
    let module = &build.module;
    let mut lines = Vec::new();
    for (id, _node) in module.nodes.iter() {
        let Some(operation) = module.node_operation(id) else {
            continue;
        };
        let read = |node: NodeId| {
            format!(
                "{node:?}[root {:?}{}]",
                module.class_root(node),
                match module.node_origin(node) {
                    Some(origin) => format!(", origin {origin:?}"),
                    None => String::new(),
                }
            )
        };
        let operator = AsEnum::<TypeOperator>::as_enum(&operation.operator);
        let low = AsEnum::<LowOperator>::as_enum(&operation.operator);
        if let Some(operator) = operator {
            let name = operator_name(&operator);
            if !is_order_comparison(&name) && name != "Eq" {
                continue;
            }
            let items = operation
                .operand
                .and_then(|operand| module.operand_items(operand).ok())
                .map(|items| {
                    items
                        .iter()
                        .map(|item| match item.node {
                            lichen_lowlevel::AnyNodeId::Static(reference) => {
                                format!("static({reference:?})")
                            }
                            lichen_lowlevel::AnyNodeId::Dynamic(node) => read(node),
                        })
                        .collect::<Vec<String>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "<no operand array>".to_string());
            lines.push(format!("operator  {name} {}", read(id)));
            lines.push(format!("            operands [{items}]"));
            // The surface operator's own operands are where the clone's state read
            // has to be recognised; a template that answers and a clone that does not
            // is the whole difference between the two lowerings.
            if is_order_comparison(&name) {
                for item in operation
                    .operand
                    .and_then(|operand| module.operand_items(operand).ok())
                    .unwrap_or_default()
                {
                    let Some(leaf) = item.node.dynamic() else {
                        continue;
                    };
                    let path = looping_function(module).and_then(|function| {
                        module
                            .parameter_value_path(function, leaf)
                            .map(|path| format!("{path:?}"))
                    });
                    lines.push(format!(
                        "            surface operand {}, kind {}, state path {path:?}",
                        read(leaf),
                        node_name(module, leaf)
                    ));
                    lines.push(format!(
                        "            walk {}",
                        walk_state_read(module, leaf)
                    ));
                }
            }
        } else if low == Some(LowOperator::Apply) {
            let items = operation
                .operand
                .and_then(|operand| module.operand_items(operand).ok());
            let callee = items
                .and_then(|items| items.first())
                .map(|item| item.node)
                .and_then(|node| node.dynamic());
            // A routed call is one whose callee is a frozen prelude binding.
            let routed = callee
                .and_then(|callee| {
                    module
                        .static_function_of_callee(callee)
                        .is_some()
                        .then_some(true)
                        .or_else(|| {
                            module
                                .node_value(AnyNodeId::Dynamic(callee))
                                .filter(|value| is_static_function_value(value))
                                .is_some()
                                .then_some(true)
                        })
                })
                .unwrap_or(false);
            if !routed {
                continue;
            }
            let operands = items
                .map(|items| {
                    items
                        .iter()
                        .map(|item| match item.node {
                            lichen_lowlevel::AnyNodeId::Static(reference) => {
                                format!("static({reference:?})")
                            }
                            AnyNodeId::Dynamic(node) => read(node),
                        })
                        .collect::<Vec<String>>()
                        .join(", ")
                })
                .unwrap_or_default();
            // The routed call's argument: its value half's leaves are what it emits over.
            let argument_leaves = items
                .and_then(|items| items.get(1).map(|item| item.node))
                .and_then(|node| node.dynamic())
                .map(|argument| {
                    let value = module.pair_value_half(argument).unwrap_or(argument);
                    format!("{:?}", module.value_leaves(value))
                })
                .unwrap_or_else(|| "<no argument>".to_string());
            lines.push(format!("routed    Apply {}", read(id)));
            lines.push(format!("            operands [{operands}]"));
            lines.push(format!("            leaves   {argument_leaves}"));
            // The comparison's own operands are the reading: a leaf that names a
            // state slot must lower to a block parameter, and one that does not is
            // what the doubling ranges over. A two-leaf routed call is the
            // comparison's own route; the one-leaf ones are the index reads.
            let state = module.nodes.iter().find_map(|(candidate, _)| {
                (module.callee_function(candidate) == looping_function(module))
                    .then(|| module.operands_of(candidate).ok())
                    .flatten()
                    .and_then(|operands| operands.get(1).copied())
            });
            for leaf in items
                .and_then(|items| items.get(1).map(|item| item.node))
                .and_then(|node| node.dynamic())
                .map(|argument| {
                    let value = module.pair_value_half(argument).unwrap_or(argument);
                    module.value_leaves(value).unwrap_or_default()
                })
                .unwrap_or_default()
            {
                let members: Vec<String> = module
                    .nodes
                    .iter()
                    .filter(|(member, _)| module.class_root(*member) == module.class_root(leaf))
                    .map(|(member, _)| match module.node_operation(member) {
                        Some(own) => match AsEnum::<TypeOperator>::as_enum(&own.operator) {
                            Some(operator) => format!("{member:?}={}", operator_name(&operator)),
                            None => format!("{member:?}=Apply"),
                        },
                        None => format!("{member:?}="),
                    })
                    .collect();
                let belongs = looping_function(module)
                    .and_then(|function| module.belongs_to(leaf, function).ok());
                let paths = looping_function(module).and_then(|function| {
                    let direct = module
                        .parameter_value_path(function, leaf)
                        .map(|path| format!("{path:?}"));
                    let through_origin = module.node_origin(leaf).and_then(|origin| {
                        module
                            .parameter_value_path(function, origin)
                            .map(|path| format!("{path:?}"))
                    });
                    let entry = state.and_then(|state| module.entry_state_path(state, leaf));
                    Some((direct, through_origin, entry))
                });
                lines.push(format!(
                    "            operand {}, kind {}, belongs {belongs:?}, paths {paths:?}",
                    read(leaf),
                    node_name(module, leaf)
                ));
                lines.push(format!(
                    "            operand's class [{}]",
                    members.join(", ")
                ));
            }
        } else if low == Some(LowOperator::Index)
            && module.selection_of(id) == Some(lichen_lowlevel::Selection::Computed)
        {
            let operands = operation
                .operand
                .and_then(|operand| module.operand_items(operand).ok())
                .map(|items| {
                    items
                        .iter()
                        .map(|item| match item.node {
                            lichen_lowlevel::AnyNodeId::Static(reference) => {
                                format!("static({reference:?})")
                            }
                            lichen_lowlevel::AnyNodeId::Dynamic(node) => read(node),
                        })
                        .collect::<Vec<String>>()
                        .join(", ")
                })
                .unwrap_or_default();
            lines.push(format!("if        Index {}", read(id)));
            lines.push(format!(
                "            operands [arms, selector] = [{operands}]"
            ));
            // The selector is what the refusal names, so its own definition and its
            // class are printed with it.
            let selector = operation
                .operand
                .and_then(|operand| module.operand_items(operand).ok())
                .and_then(|items| items.get(1).map(|item| item.node))
                .and_then(|node| node.dynamic());
            if let Some(selector) = selector {
                let own = match module.node_operation(selector) {
                    Some(own) => match AsEnum::<TypeOperator>::as_enum(&own.operator) {
                        Some(operator) => operator_name(&operator),
                        None => "an Apply".to_string(),
                    },
                    None => "no operation of its own".to_string(),
                };
                let members: Vec<String> = module
                    .nodes
                    .iter()
                    .filter(|(member, _)| module.class_root(*member) == module.class_root(selector))
                    .map(|(member, _)| match module.node_operation(member) {
                        Some(own) => match AsEnum::<TypeOperator>::as_enum(&own.operator) {
                            Some(operator) => format!("{member:?}={}", operator_name(&operator)),
                            None => format!("{member:?}=Apply"),
                        },
                        None => format!("{member:?}="),
                    })
                    .collect();
                let path = looping_function(module).and_then(|function| {
                    module
                        .parameter_value_path(function, selector)
                        .map(|path| format!("{path:?}"))
                });
                lines.push(format!(
                    "            selector {}, own operator {own}, state path {path:?}",
                    read(selector)
                ));
                lines.push(format!(
                    "            selector's class [{}]",
                    members.join(", ")
                ));
                // A routed selector emits over its argument's leaves, so each leaf is
                // read with the operator it carries of its own. A routed apply may be
                // reached through its type side, so the value side is followed too.
                if own == "an Apply" {
                    let mut frontier = vec![selector];
                    if let Some(value_side) = module.apply_pairs.get(&selector).copied() {
                        frontier.push(value_side);
                        lines.push(format!(
                            "            selector's value-side apply {}",
                            read(value_side)
                        ));
                    }
                    for apply in frontier {
                        let Some(apply_operation) = module.node_operation(apply) else {
                            continue;
                        };
                        let Some(items) = apply_operation
                            .operand
                            .and_then(|operand| module.operand_items(operand).ok())
                        else {
                            continue;
                        };
                        for (position, item) in items.iter().enumerate() {
                            let Some(container) = item.node.dynamic() else {
                                continue;
                            };
                            let value = module.pair_value_half(container).unwrap_or(container);
                            let leaves = module.value_leaves(value).unwrap_or_default();
                            lines.push(format!(
                                "            apply {position} {}, leaves {leaves:?}",
                                read(container)
                            ));
                            for leaf in leaves {
                                let leaf_operator = match module.node_operation(leaf) {
                                    Some(leaf_operation) => {
                                        match AsEnum::<TypeOperator>::as_enum(
                                            &leaf_operation.operator,
                                        ) {
                                            Some(operator) => operator_name(&operator),
                                            None => "an Apply".to_string(),
                                        }
                                    }
                                    None => "no operation".to_string(),
                                };
                                let leaf_path = looping_function(module).and_then(|function| {
                                    module
                                        .parameter_value_path(function, leaf)
                                        .map(|path| format!("{path:?}"))
                                });
                                lines.push(format!(
                                    "                leaf {}, operator {leaf_operator}, state path {leaf_path:?}",
                                    read(leaf)
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    lines.join("\n    ")
}

/// Whether a value names a frozen function, the marker of a routed prelude binding.
fn is_static_function_value(value: &lichen_compute::language::LangValue) -> bool {
    matches!(
        value.as_enum(),
        Some(LowValue::Function(AnyFunctionId::Static(_)))
    )
}

/// The module's one `@loop`-marked function, the fold the probe programs all carry.
fn looping_function(module: &Module<LangProgram>) -> Option<FunctionId> {
    module
        .functions
        .keys()
        .find(|function| module.functions[*function].looping.is_some())
}

/// What the program answers, or what it refused with.
fn run_answer(source: &str) -> String {
    let report = common::check(source);
    if !report.ok() {
        return format!(
            "refused: {}",
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<String>>()
                .join(" | ")
        );
    }
    let build = report.build.expect("a checked program builds");
    let (module, value, _root) = lichen_language::testing::finish(build);
    format!("answered {:?}", common::float_array(&module, &value))
}

/// The lowered body, instruction by instruction, so the two comparisons the order
/// comparison emits are read with the operands each of them was given.
///
/// # Invariant
///
/// Only a clean check has a body, so this is the control's reading; it names what a
/// correct lowering looks like for the same shape the refusing variants have in the graph.
fn dump_body(source: &str) -> String {
    let report = common::check(source);
    if !report.ok() {
        return "refused, so no body".to_string();
    }
    let build = report.build.expect("a checked program builds");
    let body = common::sole_fragment(&build.module).body;
    let mut lines = Vec::new();
    for (index, block) in body.blocks.iter().enumerate() {
        lines.push(format!(
            "block {index} params {:?} terminator {:?}",
            block.params, block.terminator
        ));
        for &instr in &block.instrs {
            let Some(def) = body.values.get(instr.0 as usize) else {
                continue;
            };
            match def {
                lichen_kernel_ir::ValueDef::Instr { op, args, .. } => {
                    lines.push(format!("  v{} = {op:?}{args:?}", instr.0));
                }
                other => lines.push(format!("  v{} = {other:?}", instr.0)),
            }
        }
    }
    lines.join("\n    ")
}

#[test]
fn probe_the_body_an_if_in_a_fold_lowers_to() {
    let _slot = common::slot();
    for (label, condition, then, otherwise) in [
        ("state_gt_1", "s(0) > 1", "1.0", "0.0"),
        ("state_gt_leaf", "s(0) > c.taps", "1.0", "0.0"),
        ("leaf_gt_state", "c.taps > s(0)", "1.0", "0.0"),
        ("two_state_gt", "s(0) > s(0)", "1.0", "0.0"),
        ("leaf_gt_1", "c.taps > 1", "1.0", "0.0"),
        ("one_lt_state", "1 < s(0)", "1.0", "0.0"),
        ("state_gt_1_equal_arms", "s(0) > 1", "1.0", "1.0"),
        ("state_eq_2", "s(0) == 2", "1.0", "0.0"),
        // **The selector is an addition, not the comparison**: `value_is_comparison`
        // is false for `Add`, so the body walk lowers and the body's own instructions
        // are readable. What the comparison itself emitted is one of those
        // instructions' arguments.
        ("state_gt_1_plus_zero", "(s(0) > 1) + 0", "1.0", "0.0"),
        ("leaf_gt_1_plus_zero", "(c.taps > 1) + 0", "1.0", "0.0"),
        ("state_eq_2_plus_zero", "(s(0) == 2) + 0", "1.0", "0.0"),
    ] {
        let source = program(condition, then, otherwise);
        println!("[{label}] {}", counts(&source));
        println!("    {}", dump_comparisons(&source));
        println!("    body: {}", dump_body(&source));
    }
    println!(
        "[run state_gt_1] {}",
        run_answer(&program("s(0) > 1", "1.0", "0.0"))
    );
    println!(
        "[run state_gt_1_plus_zero] {}",
        run_answer(&program("(s(0) > 1) + 0", "1.0", "0.0"))
    );
    println!(
        "[run leaf_gt_1] {}",
        run_answer(&program("c.taps > 1", "1.0", "0.0"))
    );
    println!(
        "[run state_eq_2] {}",
        run_answer(&program("s(0) == 2", "1.0", "0.0"))
    );
}
