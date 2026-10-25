//! Function values and `Apply`: clone-and-map calls, nesting, and compaction.

use super::*;

#[test]
fn function_call_operator_clones_body_and_maps_parameter() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, ret, param) = function(&mut m, |m, ret, param| {
        m.close_operation_cycle(
            ret,
            Operation {
                operator: TestOperator::Id,
                operand: Some(param),
            },
        );
    });

    // The template scope is exactly the body nodes, return and parameter.
    let func = dyn_function(m.node_value(AnyNodeId::Dynamic(func_node)).unwrap());
    assert_eq!(
        m.functions[func]
            .nodes
            .iter()
            .copied()
            .collect::<HashSet<_>>(),
        HashSet::from([ret, param])
    );

    // The body is untouched and still callable: its parameter is still the
    // marker and its return node still references it.
    let body = m.node_block(ret);
    assert!(m.blocks.contains_key(body));
    assert_eq!(m.node_operation(ret).unwrap().operand, Some(param));
    assert!(
        m.node_value(AnyNodeId::Dynamic(param)).is_none(),
        "the parameter stays an empty slot"
    );
    assert_eq!(m.node_block(ret), body);
    assert_eq!(m.node_block(param), body);

    // The call operator resolves through the argument and caches the
    // result on the call node in its own block.
    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 42);
    assert_eq!(m.node_block(call), root);
    assert_eq!(u128_of(m.node_value(AnyNodeId::Dynamic(call)).unwrap()), 42);
    assert_eq!(
        m.nodes.len(),
        8 // ret + param + func + arg + operands + call + clone_ret + clone_param
    );
}
#[test]
fn function_call_operator_clones_array_body() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [x, 7]
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None); // RETURN_IDX
    let param = m.add_node(f, None, None); // PARAMETER_IDX
    let seven = u128_node(&mut m, f, 7);
    let items = [item(param), item(seven)];
    m.write_node_value(
        ret,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&items, f).into(),
        ))),
    );
    let (func_node, _) = wrap_function(&mut m, f, ret, param);

    // The array embeds the parameter, so the definition pass flags it undecided.
    m.evaluate_node_deep(ret, None);
    assert_eq!(
        m.node_evaluated_deep(ret),
        Some(EvaluatedDeep { undecided: true })
    );

    let arg = u128_node(&mut m, root, 10);
    let call = call_node(&mut m, root, func_node, arg);
    let value = m.evaluate_node_deep(call, None).unwrap();

    assert_u128_array(&m, value, &[10, 7]);
    // The clone's array holds the parameter's clone, unified with the argument,
    // and references the body's constant in place.
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(call)).unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        m.equality_representative(ids[0]),
        m.equality_representative(arg)
    );
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(ids[0])),
        Some(TestValue::U128(_))
    ));
    assert_eq!(ids[1], seven);
    assert_eq!(m.node_block(seven), f); // referenced in place, not cloned
    assert_eq!(
        array_ids(m.node_value(AnyNodeId::Dynamic(ret)).unwrap()),
        &[param, seven]
    ); // body unchanged
}
#[test]
fn function_call_operator_preserves_undecided_operand_chain() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Id(Id(x))
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None); // RETURN_IDX
    let param = m.add_node(f, None, None); // PARAMETER_IDX
    let mid = op_node(&mut m, f, TestOperator::Id, Some(param));
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::Id,
            operand: Some(mid),
        },
    );
    let (func_node, _) = wrap_function(&mut m, f, ret, param);

    // The argument is itself undecided, so the call result stays
    // marker until the argument resolves.
    let arg = m.add_node(root, None, None);
    let call = call_node(&mut m, root, func_node, arg);
    let value = m.evaluate_node_deep(call, None);
    assert!(value.is_none(), "an undecided argument stays lazy");
    assert_eq!(
        m.node_evaluated_deep(call),
        Some(EvaluatedDeep { undecided: true })
    );

    // The body is untouched.
    assert!(m.blocks.contains_key(m.node_block(ret)));
    assert_eq!(m.node_operation(mid).unwrap().operand, Some(param));
}
#[test]
fn function_call_operator_recomputes_stale_definition_markers() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, ret, param) = function(&mut m, |m, ret, param| {
        m.close_operation_cycle(
            ret,
            Operation {
                operator: TestOperator::Id,
                operand: Some(param),
            },
        );
    });

    // The definition pass's transient marker is not cached, so a later binding
    // is observed on re-read.
    m.evaluate_node_deep(ret, None);
    assert!(
        m.node_value(AnyNodeId::Dynamic(ret)).is_none(),
        "the node keeps no value"
    );
    assert_eq!(
        m.node_evaluated_deep(ret),
        Some(EvaluatedDeep { undecided: true })
    );

    // The call clones the undecided node unevaluated — the stale
    // marker is recomputed against the concrete argument.
    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 42);

    // The clone pass leaves the body's parameter an empty cell.
    assert_eq!(m.node_operation(ret).unwrap().operand, Some(param));
    assert!(m.node_value(AnyNodeId::Dynamic(param)).is_none());
}
#[test]
fn function_call_operator_references_concrete_body_nodes_in_place() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Id(7): no path depends on the parameter.
    let (func_node, ret, param) = function(&mut m, |m, ret, _param| {
        let seven = u128_node(m, m.node_block(ret), 7);
        m.close_operation_cycle(
            ret,
            Operation {
                operator: TestOperator::Id,
                operand: Some(seven),
            },
        );
    });

    // The definition pass resolves the body to a concrete constant.
    m.evaluate_node_deep(ret, None);
    assert_eq!(
        m.node_evaluated_deep(ret),
        Some(EvaluatedDeep { undecided: false })
    );

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 7);
    assert!(
        m.node_value(AnyNodeId::Dynamic(param)).is_none(),
        "body untouched"
    );
}
#[test]
fn function_in_local_block_survives_compaction() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // f(x) = Id(x), built inside a local block that is compacted away.
    let ret = m.add_node(child, None, None); // RETURN_IDX
    let param = m.add_node(child, None, None); // PARAMETER_IDX
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::Id,
            operand: Some(param),
        },
    );
    let (func_node, _) = wrap_function(&mut m, child, ret, param);

    // Calling the function while it still lives in the local block works.
    let arg = u128_node(&mut m, child, 42);
    let call = call_node(&mut m, child, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 42);

    // Running the block compacts the function value into the root and maps
    // the template nodes along with it.
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(func_node));
    let mapped = dyn_function(m.evaluate_node_deep(root_node, None).unwrap());
    assert!(!m.blocks.contains_key(child));
    assert_eq!(m.node_block(ret), root); // template mapped into the root
    assert_eq!(m.node_block(param), root);
    assert_eq!(
        m.functions[mapped]
            .nodes
            .iter()
            .copied()
            .collect::<HashSet<_>>(),
        HashSet::from([ret, param])
    );
    assert!(m.functions.contains_key(mapped)); // the function outlives the block

    // The outer block can still call the mapped function.
    let arg2 = u128_node(&mut m, root, 7);
    let call2 = call_node(&mut m, root, func_node, arg2);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None).unwrap()), 7);
}
#[test]
fn function_scope_is_dropped_with_its_block() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // f(x) = Id(x), homed in the child but not reachable from the block's
    // return, so it must not survive the compaction.
    let ret = m.add_node(child, None, None);
    let param = m.add_node(child, None, None);
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::Id,
            operand: Some(param),
        },
    );
    let (func_node, func) = wrap_function(&mut m, child, ret, param);
    assert_eq!(m.functions.len(), 1);

    // Compaction keeps only the return-reachable tree, then releases the rest,
    // the function's home node included.
    let x = u128_node(&mut m, child, 5);
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(x));
    assert_eq!(u128_of(m.evaluate_node_deep(root_node, None).unwrap()), 5);

    assert!(!m.blocks.contains_key(child));
    assert!(!m.nodes.contains_key(func_node));
    assert!(!m.functions.contains_key(func)); // scope dropped with the block
}

// --- nested functions, higher-order functions, mixed blocks+functions ---
#[test]
fn nested_function_is_called_by_the_outer_body() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // The parent link makes g's scope part of f's template, so the clone folds
    // it in.

    // f's template holds g's value node but not g's internals.
    let g = m.add_block(None);
    let gret = m.add_node(g, None, None);
    let gparam = m.add_node(g, None, None);
    m.close_operation_cycle(
        gret,
        Operation {
            operator: TestOperator::Id,
            operand: Some(gparam),
        },
    );
    let (g_node, g_id) = wrap_function(&mut m, g, gret, gparam);
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None);
    let param = m.add_node(f, None, None);
    let operands = array_node(&mut m, f, &[g_node, param], None);
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::LowOperator(LowOperator::Apply),
            operand: Some(operands),
        },
    );
    let (f_node, f_id) = wrap_function(&mut m, f, ret, param);
    m.functions[g_id].parent = Some(f_id);
    m.evaluate_node_deep(ret, None); // definition pass: the nested g is concrete

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, f_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 42);

    // The nested g is untouched in the template and still callable directly.
    assert_eq!(m.node_block(g_node), g);
    let g_arg = u128_node(&mut m, root, 5);
    let g_call = call_node(&mut m, root, g_node, g_arg);
    assert_eq!(u128_of(m.evaluate_node_deep(g_call, None).unwrap()), 5);
}
#[test]
fn outer_call_returns_a_nested_function_value() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // g(y) = Id(y) lives in its own block; f's template holds its value
    // node; f(x) = g returns it.
    let g = m.add_block(None);
    let gret = m.add_node(g, None, None);
    let gparam = m.add_node(g, None, None);
    m.close_operation_cycle(
        gret,
        Operation {
            operator: TestOperator::Id,
            operand: Some(gparam),
        },
    );
    let (g_node, g_id) = wrap_function(&mut m, g, gret, gparam);
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None);
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::Id,
            operand: Some(g_node),
        },
    );
    let param = m.add_node(f, None, None);
    let (f_node, f_id) = wrap_function(&mut m, f, ret, param);
    m.functions[g_id].parent = Some(f_id);
    m.evaluate_node_deep(ret, None); // definition pass: Id(g) is concrete

    let one = u128_node(&mut m, root, 1);
    let call = call_node(&mut m, root, f_node, one);
    let got = dyn_function(m.evaluate_node_deep(call, None).unwrap());
    // The concreteness proof cannot see a nested body, so it is never
    // referenced in place: each call gets a fresh closure.
    assert_ne!(got, g_id);

    // The returned function is callable from the outer block.
    let got_node = m.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Function(
            AnyFunctionId::Dynamic(got),
        ))),
    );
    let arg = u128_node(&mut m, root, 7);
    let call2 = call_node(&mut m, root, got_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None).unwrap()), 7);
    // The template g is untouched and still callable directly.
    let call3 = call_node(&mut m, root, g_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call3, None).unwrap()), 7);
}
#[test]
fn a_nested_function_value_captures_the_applied_outer_parameter() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // g lives in its own block, and f's template holds g's value node but not
    // g's internals.

    // The clone folds g's own scope into the template to rewrite the capture.
    let g = m.add_block(None);
    let gparam = m.add_node(g, None, None);
    let gret = m.add_node(g, None, None);
    let (g_node, g_id) = wrap_function(&mut m, g, gret, gparam);
    let f = m.add_block(None);
    let param = m.add_node(f, None, None);
    m.close_operation_cycle(
        gret,
        Operation {
            operator: TestOperator::Id,
            operand: Some(param),
        },
    );
    let ret = m.add_node(f, None, None);
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::Id,
            operand: Some(g_node),
        },
    );
    let (func_node, f_id) = wrap_function(&mut m, f, ret, param);
    m.functions[g_id].parent = Some(f_id);

    let forty_two = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, forty_two);
    let got = dyn_function(m.evaluate_node_deep(call, None).unwrap());

    // The returned closure reads the applied parameter: g'(7) is Id(42).
    let got_node = m.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Function(
            AnyFunctionId::Dynamic(got),
        ))),
    );
    let seven = u128_node(&mut m, root, 7);
    let call2 = call_node(&mut m, root, got_node, seven);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None).unwrap()), 42);
}
#[test]
fn higher_order_function_passes_a_function_argument_through() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // apply(g) = g: the return node's operand is the parameter.
    let (apply_node, ret, _param) = function(&mut m, |m, ret, param| {
        m.close_operation_cycle(
            ret,
            Operation {
                operator: TestOperator::Id,
                operand: Some(param),
            },
        );
    });
    m.evaluate_node_deep(ret, None); // definition pass: Id(marker) stays a marker

    // g(x) = Id(x).
    let (g_node, _, _) = function(&mut m, |m, ret, param| {
        m.close_operation_cycle(
            ret,
            Operation {
                operator: TestOperator::Id,
                operand: Some(param),
            },
        );
    });
    let g_id = dyn_function(m.node_value(AnyNodeId::Dynamic(g_node)).unwrap());

    let call = call_node(&mut m, root, apply_node, g_node);
    let got = dyn_function(m.evaluate_node_deep(call, None).unwrap());
    assert_eq!(got, g_id);

    // The passed-through function is still callable.
    let arg = u128_node(&mut m, root, 9);
    let call2 = call_node(&mut m, root, g_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None).unwrap()), 9);
}
#[test]
fn higher_order_function_calls_its_function_argument() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // apply(f) = f(42): the parameter is the target of an Apply node.
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(body, None, None);
    let forty_two = u128_node(&mut m, body, 42);
    let operands = array_node(&mut m, body, &[param, forty_two], None);
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::LowOperator(LowOperator::Apply),
            operand: Some(operands),
        },
    );
    let (apply_node, _) = wrap_function(&mut m, body, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: a marker target stays lazy

    // g(x) = Id(x): passing g as the argument makes apply evaluate g(42).
    let (g_node, _, _) = function(&mut m, |m, ret, param| {
        m.close_operation_cycle(
            ret,
            Operation {
                operator: TestOperator::Id,
                operand: Some(param),
            },
        );
    });
    let call = call_node(&mut m, root, apply_node, g_node);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 42);
}
#[test]
fn function_can_index_into_undecided_array() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [x, 7][0]: the Index arm sees a marker element and stays lazy.
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(body, None, None);
    let seven = u128_node(&mut m, body, 7);
    let array = array_node(&mut m, body, &[param, seven], None);
    let zero = usize_node(&mut m, body, 0);
    let operands = array_node(&mut m, body, &[array, zero], None);
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::LowOperator(LowOperator::Index),
            operand: Some(operands),
        },
    );
    let (f_node, _) = wrap_function(&mut m, body, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: index of a marker stays a marker
    assert_eq!(
        m.node_evaluated_deep(ret),
        Some(EvaluatedDeep { undecided: true })
    );

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, f_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 42);

    // The body is untouched and still undecided.
    assert!(
        m.node_value(AnyNodeId::Dynamic(param)).is_none(),
        "body untouched"
    );
}
#[test]
fn manually_partially_evaluated_function_applies_correctly() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Add(Add(x, 1), 2): only the two constants are defined by hand.
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(body, None, None);
    let one = u128_node(&mut m, body, 1);
    let inner_ops = array_node(&mut m, body, &[param, one], None);
    let inner = op_node(&mut m, body, TestOperator::Add, Some(inner_ops));
    let two = u128_node(&mut m, body, 2);
    let ret_ops = array_node(&mut m, body, &[inner, two], None);
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::Add,
            operand: Some(ret_ops),
        },
    );
    let (f_node, _) = wrap_function(&mut m, body, ret, param);

    // Manually define exactly the constants; the parameter-dependent nodes
    // keep evaluated_deep = None.
    m.evaluate_node_deep(one, None);
    m.evaluate_node_deep(two, None);
    assert_eq!(
        m.node_evaluated_deep(one),
        Some(EvaluatedDeep { undecided: false })
    );
    assert_eq!(
        m.node_evaluated_deep(two),
        Some(EvaluatedDeep { undecided: false })
    );
    assert_eq!(m.node_evaluated_deep(ret), None);
    assert_eq!(m.node_evaluated_deep(inner), None);
    assert_eq!(m.node_evaluated_deep(inner_ops), None);
    assert_eq!(m.node_evaluated_deep(ret_ops), None);

    // The apply reuses the proven constants in place and remaps the rest.
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 8);

    // The cloned operand array keeps the constant and maps the parameter onto
    // a clone unified with the argument.
    let candidates: Vec<NodeId> = m.blocks[root]
        .nodes
        .iter()
        .copied()
        .filter(|&id| {
            matches!(
                m.node_value(AnyNodeId::Dynamic(id)),
                Some(TestValue::LowValue(LowValue::Array(_)))
            )
        })
        .collect();
    let cloned_inner_ops = candidates
        .into_iter()
        .find(|&id| {
            let ids = array_ids(m.node_value(AnyNodeId::Dynamic(id)).unwrap());
            ids.len() == 2
                && ids[1] == one
                && m.equality_representative(ids[0]) == m.equality_representative(five)
        })
        .expect("the cloned inner operand array references the parameter's clone");
    assert_eq!(m.node_block(cloned_inner_ops), root);

    let nine = u128_node(&mut m, root, 9);
    let call2 = call_node(&mut m, root, f_node, nine);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None).unwrap()), 12);
}
#[test]
fn unevaluated_function_applies_correctly() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Add(x, 1), defined by hand with no evaluation pass.
    let (f_node, ret, param) = function(&mut m, |m, ret, param| {
        let one = u128_node(m, m.node_block(ret), 1);
        let operands = array_node(m, m.node_block(ret), &[param, one], None);
        m.close_operation_cycle(
            ret,
            Operation {
                operator: TestOperator::Add,
                operand: Some(operands),
            },
        );
    });
    assert_eq!(m.node_evaluated_deep(ret), None);
    assert_eq!(m.node_evaluated_deep(param), None);

    // The apply clones the whole unevaluated body and resolves it against
    // the argument: f(5) = 6, f(9) = 10.
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 6);

    let nine = u128_node(&mut m, root, 9);
    let call2 = call_node(&mut m, root, f_node, nine);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None).unwrap()), 10);
}
#[test]
fn mixed_blocks_and_functions_survive_compaction() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // g(x) = Add(x, 7) is built inside the child block; the constant 7
    // lives in the child and is part of g's scope.
    let ret = m.add_node(child, None, None);
    let param = m.add_node(child, None, None);
    let seven = u128_node(&mut m, child, 7);
    let operands = array_node(&mut m, child, &[param, seven], None);
    m.close_operation_cycle(
        ret,
        Operation {
            operator: TestOperator::Add,
            operand: Some(operands),
        },
    );
    let (g_node, g_id) = wrap_function(&mut m, child, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass (Add is marker-aware)

    // Call g from inside the child: 10 + 7.
    let ten = u128_node(&mut m, child, 10);
    let call = call_node(&mut m, child, g_node, ten);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None).unwrap()), 17);

    // Compaction re-homes the function and its whole scope into the root.
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(g_node));
    let mapped = dyn_function(m.evaluate_node_deep(root_node, None).unwrap());
    assert_eq!(mapped, g_id);
    assert_eq!(m.functions[g_id].block, root); // re-homed to the root
    assert_eq!(m.node_block(seven), root); // scope constant moved too
    assert!(!m.blocks.contains_key(child));

    // Still callable after compaction: 3 + 7.
    let three = u128_node(&mut m, root, 3);
    let call2 = call_node(&mut m, root, g_node, three);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None).unwrap()), 10);
}
#[test]
fn call_return_is_shallow_for_container_bodies() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // The call clones the unevaluated array rather than forcing it.
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None); // RETURN_IDX
    let param = m.add_node(f, None, None); // PARAMETER_IDX
    let id2 = op_node(&mut m, f, TestOperator::Id, Some(param));
    let items = [item(param), item(id2)];
    m.write_node_value(
        ret,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&items, f).into(),
        ))),
    );
    let (func_node, _) = wrap_function(&mut m, f, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass flags the array undecided
    assert_eq!(
        m.node_evaluated_deep(ret),
        Some(EvaluatedDeep { undecided: true })
    );

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);

    // Shallow evaluation of the call node returns the array without
    // forcing the elements: the Id(x) clone stays unevaluated.
    let value = m.evaluate_node(AnyNodeId::Dynamic(call), None).unwrap();
    let ids = array_ids(value);
    assert_eq!(ids.len(), 2);
    assert_eq!(
        m.equality_representative(ids[0]),
        m.equality_representative(arg)
    ); // the cloned parameter, unified with the argument
    assert!(m.node_value(AnyNodeId::Dynamic(ids[1])).is_none()); // the Id clone is still lazy

    // Deep evaluation forces them.
    let deep = m.evaluate_node_deep(call, None).unwrap();
    assert_u128_array(&m, deep, &[42, 42]);
}
#[test]
fn apply_evaluates_argument_elements_to_match_the_parameter_pattern() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // The apply evaluates the argument's elements to the pattern's depth first.

    // An unevaluated element would read as undecided and bind nothing.
    let x0 = m.add_node(root, None, None);
    let x1 = m.add_node(root, None, None);
    let items = [item(x0), item(x1)];
    let param = m.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&items, root).into(),
        ))),
    );
    let f = m.add_function(root, param, param, [param, x0, x1], []);
    // argument: [Add(1, 2), Sub(5, 3)] = [3, 2]
    let one = u128_node(&mut m, root, 1);
    let two = u128_node(&mut m, root, 2);
    let add_ops = array_node(&mut m, root, &[one, two], None);
    let add = op_node(&mut m, root, TestOperator::Add, Some(add_ops));
    let five = u128_node(&mut m, root, 5);
    let three = u128_node(&mut m, root, 3);
    let sub_ops = array_node(&mut m, root, &[five, three], None);
    let sub = op_node(&mut m, root, TestOperator::Sub, Some(sub_ops));
    let arg = array_node(&mut m, root, &[add, sub], None);
    let call = call_node(&mut m, root, f, arg);
    let value = m.evaluate_node_deep(call, None).unwrap();
    assert!(m.unify_errors.is_empty());
    assert_u128_array(&m, value, &[3, 2]);
}
#[test]
fn apply_clone_preserves_the_shallow_mask() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // The apply's clone must carry the shallow mask onto the cloned array.
    let f = m.add_block(None);
    let param = m.add_node(f, None, None);
    let one = u128_node(&mut m, f, 1);
    let add_ops = array_node(&mut m, f, &[param, one], None);
    let add = op_node(&mut m, f, TestOperator::Add, Some(add_ops));
    let ret = array_node(&mut m, f, &[param, add], Some(&[false, true]));
    let (func_node, _) = wrap_function(&mut m, f, ret, param);

    m.evaluate_node_deep(ret, None); // definition pass
    assert_eq!(
        m.node_evaluated_deep(ret),
        Some(EvaluatedDeep { undecided: true })
    );

    let arg = u128_node(&mut m, root, 10);
    let call = call_node(&mut m, root, func_node, arg);
    let value = m.evaluate_node_deep(call, None).unwrap();

    assert_eq!(
        array_mask(value),
        [false, true],
        "the mask travels through the clone"
    );
    let ids = array_ids(value);
    assert_eq!(
        u128_of(m.node_value(AnyNodeId::Dynamic(ids[0])).unwrap()),
        10
    );
    assert!(
        m.node_value(AnyNodeId::Dynamic(ids[1])).is_none(),
        "the marked element stays lazy in the call"
    );
}
#[test]
fn pattern_argument_evaluation_skips_shallow_positions() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = x with x = [x0, x1]: position 1 of the pattern is shallow.
    let x0 = m.add_node(root, None, None);
    let x1 = m.add_node(root, None, None);
    let param = array_node(&mut m, root, &[x0, x1], Some(&[false, true]));
    let f = m.add_function(root, param, param, [param, x0, x1], []);
    // argument: [7, Add(1, 2)] — the Add is at the masked position.
    let seven = u128_node(&mut m, root, 7);
    let one = u128_node(&mut m, root, 1);
    let two = u128_node(&mut m, root, 2);
    let add_ops = array_node(&mut m, root, &[one, two], None);
    let add = op_node(&mut m, root, TestOperator::Add, Some(add_ops));
    let arg = array_node(&mut m, root, &[seven, add], None);
    let call = call_node(&mut m, root, f, arg);
    let value = m.evaluate_node_deep(call, None).unwrap();

    assert!(m.unify_errors.is_empty());
    let ids = array_ids(value);
    assert_eq!(ids.len(), 2);
    assert_eq!(
        u128_of(m.node_value(AnyNodeId::Dynamic(ids[0])).unwrap()),
        7
    );
    // A shallow pattern position is never walked, so the argument's element
    // there is never forced.
    assert!(
        m.node_value(AnyNodeId::Dynamic(add)).is_none(),
        "the shallow pattern position is not forced by the apply"
    );
    assert_eq!(
        m.node_evaluated_deep(ids[1]),
        None,
        "the masked position is never walked"
    );
}

// --- the template instantiation ---------------------------------------

/// The items of an array value, dynamic and frozen alike.
fn template_items(value: impl Into<Option<TestValue>>) -> Vec<AnyNodeId> {
    let TestValue::LowValue(LowValue::Array(array)) = value.into().expect("expected an array")
    else {
        panic!("expected an array value")
    };
    // SAFETY: the value was just produced by the module under test, whose
    // block has not been dropped.
    unsafe { array.items() }
        .iter()
        .map(|item| item.node)
        .collect()
}

#[test]
fn template_clone_makes_the_template_cells_fresh() {
    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let template = m.add_block(None);
    // T = (_, _): two open cells under one template.
    let left = undecided_node(&mut m, template);
    let right = undecided_node(&mut m, template);
    let t = array_node(&mut m, template, &[left, right], None);

    let instance = m
        .clone_template(t, root, None, &lichen_lowlevel::TemplateBoundary::new())
        .expect("the template instantiates");

    assert_ne!(instance.root, t, "the root is the instance's own cell");
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], left);
    assert_ne!(ids[1], right);
    assert_ne!(ids[0], ids[1]);
    // The template keeps its own open cells: the instance's are fresh.
    assert!(m.node_value(AnyNodeId::Dynamic(left)).is_none());
    assert!(m.node_value(AnyNodeId::Dynamic(right)).is_none());
    assert!(m.node_value(AnyNodeId::Dynamic(ids[0])).is_none());
    assert!(m.node_value(AnyNodeId::Dynamic(ids[1])).is_none());
    // The map names one instance cell per template cell.
    assert_eq!(instance.node_of(t), instance.root);
    assert_eq!(instance.node_of(left), ids[0]);
    assert_eq!(instance.node_of(right), ids[1]);

    // The caller unifies the instance with the actual value: the instance's own
    // cells take it, the template's stay open.
    let one = u128_node(&mut m, root, 1);
    let two = u128_node(&mut m, root, 2);
    let actual = array_node(&mut m, root, &[one, two], None);
    m.unify(instance.root, actual);
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_eq!(
        u128_of(m.node_value(AnyNodeId::Dynamic(ids[0])).unwrap()),
        1
    );
    assert_eq!(
        u128_of(m.node_value(AnyNodeId::Dynamic(ids[1])).unwrap()),
        2
    );
    assert!(m.node_value(AnyNodeId::Dynamic(left)).is_none());
    assert!(m.node_value(AnyNodeId::Dynamic(right)).is_none());

    // A second instance is its own: the template's cells are still open.
    let second = m
        .clone_template(t, root, None, &lichen_lowlevel::TemplateBoundary::new())
        .unwrap();
    assert_ne!(second.root, instance.root);
    assert_ne!(second.node_of(left), instance.node_of(left));
    assert!(m.node_value(AnyNodeId::Dynamic(left)).is_none());
}

#[test]
fn template_clone_shares_the_marked_boundary() {
    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let template = m.add_block(None);
    // The template names an outer binding: the caller's own open cell.
    let outer = undecided_node(&mut m, root);
    let hole = undecided_node(&mut m, template);
    let t = array_node(&mut m, template, &[outer, hole], None);

    // Unmarked, the outer binding is cloned like any other cell.
    let instance = m
        .clone_template(t, root, None, &lichen_lowlevel::TemplateBoundary::new())
        .unwrap();
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_ne!(ids[0], outer, "an unmarked outer cell is cloned");

    // Marked, it is the caller's own: the instance references it in place.
    let mut boundary = lichen_lowlevel::TemplateBoundary::new();
    boundary.share(outer);
    let instance = m.clone_template(t, root, None, &boundary).unwrap();
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_eq!(ids[0], outer, "the marked cell is referenced in place");
    assert_ne!(ids[1], hole, "the template's own cell is still fresh");
    assert!(instance.shared.contains(&outer));
    assert!(m.node_value(AnyNodeId::Dynamic(hole)).is_none());
}

#[test]
fn template_clone_keeps_internal_sharing_and_cycles() {
    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let template = m.add_block(None);
    // T = (h, h): one cell named twice.
    let shared = undecided_node(&mut m, template);
    let t = array_node(&mut m, template, &[shared, shared], None);
    let instance = m
        .clone_template(t, root, None, &lichen_lowlevel::TemplateBoundary::new())
        .unwrap();
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_eq!(ids[0], ids[1], "one template cell has one instance cell");

    // A cycle: the cell's own payload names the cell.
    let cyclic = m.add_node(template, None, None);
    let cyclic_hole = undecided_node(&mut m, template);
    let items = [item(cyclic), item(cyclic_hole)];
    m.write_node_value(
        cyclic,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&items, template).into(),
        ))),
    );
    let instance = m
        .clone_template(
            cyclic,
            root,
            None,
            &lichen_lowlevel::TemplateBoundary::new(),
        )
        .unwrap();
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_eq!(
        ids[0], instance.root,
        "the cycle names the instance's own cell"
    );
    assert_ne!(ids[1], cyclic_hole, "the open cell beside it is fresh");
}

#[test]
fn template_clone_rebuilds_the_equality_classes() {
    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let template = m.add_block(None);
    let a = undecided_node(&mut m, template);
    let b = undecided_node(&mut m, template);
    // The template states that its two cells are one expression.
    m.unify(a, b);
    let t = array_node(&mut m, template, &[a, b], None);

    let instance = m
        .clone_template(t, root, None, &lichen_lowlevel::TemplateBoundary::new())
        .unwrap();
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_ne!(ids[0], ids[1], "the instance has its own cells");
    assert_eq!(
        m.equality_representative(ids[0]),
        m.equality_representative(ids[1]),
        "the template's class topology is rebuilt among the clones"
    );
    assert_ne!(
        m.equality_representative(ids[0]),
        m.equality_representative(a),
        "the instance's class is not the template's"
    );
}

#[test]
fn template_clone_keeps_the_operator_and_clones_the_operand() {
    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let template = m.add_block(None);
    let operand = undecided_node(&mut m, template);
    let cell = op_node(&mut m, template, TestOperator::Id, Some(operand));

    let instance = m
        .clone_template(cell, root, None, &lichen_lowlevel::TemplateBoundary::new())
        .unwrap();
    let operation = m.node_operation(instance.root).unwrap();
    assert_eq!(operation.operator, TestOperator::Id);
    let cloned_operand = operation.operand.unwrap();
    assert_ne!(cloned_operand, operand);
    assert_eq!(instance.node_of(operand), cloned_operand);
    // The instance's operator runs on the instance's own operand.
    assert!(m.node_value(AnyNodeId::Dynamic(cloned_operand)).is_none());
}

#[test]
fn template_clone_shares_a_closed_value_and_clones_an_open_interior() {
    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let template = m.add_block(None);
    let one = u128_node(&mut m, template, 1);
    let two = u128_node(&mut m, template, 2);
    // Every element decided: the tuple is closed.
    let closed = array_node(&mut m, template, &[one, two], None);
    // One element open: the tuple's interior is open.
    let hole = undecided_node(&mut m, template);
    let opened = array_node(&mut m, template, &[one, hole], None);

    let instance = m
        .clone_template(
            closed,
            root,
            None,
            &lichen_lowlevel::TemplateBoundary::new(),
        )
        .unwrap();
    assert_ne!(
        instance.root, closed,
        "the root is always the instance's own"
    );
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_eq!(ids[0], one, "a decided element is shared");
    assert_eq!(ids[1], two);
    assert!(instance.shared.contains(&one));

    let instance = m
        .clone_template(
            opened,
            root,
            None,
            &lichen_lowlevel::TemplateBoundary::new(),
        )
        .unwrap();
    let ids = array_ids(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_eq!(ids[0], one, "a decided element is still shared");
    assert_ne!(ids[1], hole, "an open element is fresh");
    assert!(m.node_value(AnyNodeId::Dynamic(hole)).is_none());
}

#[test]
fn template_clone_leaves_the_wildcard_uncarried() {
    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let template = m.add_block(None);
    let wildcard = m.add_node(template, None, Some(TestValue::LowValue(LowValue::Any)));
    let instance = m
        .clone_template(
            wildcard,
            root,
            None,
            &lichen_lowlevel::TemplateBoundary::new(),
        )
        .unwrap();
    assert_ne!(instance.root, wildcard);
    assert!(
        m.node_value(AnyNodeId::Dynamic(instance.root)).is_none(),
        "the wildcard is decided but never carried: the instance's own cell replaces it"
    );
}

#[test]
fn template_clone_registers_the_clones_with_the_owner() {
    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let template = m.add_block(None);
    let hole = undecided_node(&mut m, template);
    let t = array_node(&mut m, template, &[hole], None);
    let owner = m.begin_function(root, None);

    let instance = m
        .clone_template(
            t,
            root,
            Some(owner),
            &lichen_lowlevel::TemplateBoundary::new(),
        )
        .unwrap();
    assert_eq!(m.node_function(instance.root), Some(owner));
    assert!(m.functions[owner].nodes.contains(&instance.root));
    let hole_clone = instance.node_of(hole);
    assert!(m.functions[owner].nodes.contains(&hole_clone));
}

#[test]
fn template_clone_refuses_a_budget_it_cannot_pay() {
    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let template = m.add_block(None);
    let hole = undecided_node(&mut m, template);
    let t = array_node(&mut m, template, &[hole], None);
    m.apply_depth_limit = 0;

    assert!(
        m.clone_template(t, root, None, &lichen_lowlevel::TemplateBoundary::new())
            .is_none(),
        "a budget is not a success"
    );
}

#[test]
fn template_clone_makes_an_open_frozen_cell_dynamic_and_keeps_a_closed_one_frozen() {
    use lichen_lowlevel::{ModuleKey, StaticNodeId};

    let mut source = Module::new();
    let body = source.add_block(None);
    let open = undecided_node(&mut source, body);
    let closed = u128_node(&mut source, body, 42);

    let mut m: Module<TestProgram> = Module::new();
    let root = m.add_block(None);
    let frozen = m.freeze_mapped(&source, ModuleKey::from_raw(1), [0; 32]);
    let open_ref = StaticNodeId {
        module: frozen.key,
        index: frozen.node_map[&open],
    };
    let closed_ref = StaticNodeId {
        module: frozen.key,
        index: frozen.node_map[&closed],
    };
    // The template names one open frozen cell and one decided one.
    let items = [
        ArrayItem::new(AnyNodeId::Static(open_ref)),
        ArrayItem::new(AnyNodeId::Static(closed_ref)),
    ];
    let t = m.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&items, root).into(),
        ))),
    );

    let instance = m
        .clone_template(t, root, None, &lichen_lowlevel::TemplateBoundary::new())
        .unwrap();
    let ids = template_items(m.node_value(AnyNodeId::Dynamic(instance.root)).unwrap());
    assert_eq!(ids.len(), 2);
    assert!(
        matches!(ids[0], AnyNodeId::Dynamic(_)),
        "an open frozen cell cannot be written, so it becomes the instance's own"
    );
    assert_eq!(
        ids[1],
        AnyNodeId::Static(closed_ref),
        "a decided frozen cell is read in place"
    );
}
