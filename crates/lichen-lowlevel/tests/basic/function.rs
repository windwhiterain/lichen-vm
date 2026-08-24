//! Function values and `Apply`: clone-and-map call semantics, nested and
//! higher-order functions, and functions interacting with compaction.

use super::*;

#[test]
fn function_call_operator_clones_body_and_maps_parameter() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, ret, param) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: TestOperator::Id,
            operand: Some(param),
        });
    });

    // The template scope is exactly the body nodes, return and parameter.
    let TestValue::Function(func) = m.nodes[func_node].value.unwrap() else {
        unreachable!("expected a function value")
    };
    assert_eq!(m.functions[func].nodes, HashSet::from([ret, param]));

    // The body is untouched and still callable: its parameter is still the
    // marker and its return node still references it.
    let body = m.nodes[ret].block;
    assert!(m.blocks.contains_key(body));
    assert_eq!(m.nodes[ret].operation.unwrap().operand, Some(param));
    assert!(matches!(
        m.nodes[param].value,
        Some(TestValue::Parameterized)
    ));
    assert_eq!(m.nodes[ret].block, body);
    assert_eq!(m.nodes[param].block, body);

    // The call operator resolves through the argument and caches the
    // result on the call node in its own block.
    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);
    assert_eq!(m.nodes[call].block, root);
    assert_eq!(u128_of(m.nodes[call].value.unwrap()), 42);
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
    let param = m.add_node(f, None, Some(TestValue::Parameterized)); // PARAMETER_IDX
    let seven = u128_node(&mut m, f, 7);
    let slice = m.blocks[f].arena.alloc_slice_copy(&[param, seven]);
    m.nodes[ret].value = Some(TestValue::Array(ArrayRef::new(
        std::ptr::slice_from_raw_parts(slice.as_ptr(), slice.len()),
    )));
    let (func_node, _) = wrap_function(&mut m, f, ret, param);

    // The array embeds the parameter, so the definition pass (evaluating
    // the body with the marker parameter) flags it parameterized.
    m.evaluate_node_deep(ret, None);
    assert_eq!(m.nodes[ret].parameterized_deep, Some(true));

    let arg = u128_node(&mut m, root, 10);
    let call = call_node(&mut m, root, func_node, arg);
    let value = m.evaluate_node_deep(call, None);

    assert_u128_array(&m, value, &[10, 7]);
    // The clone's array holds the cloned parameter — unified with the
    // argument, so it carries the argument's value — and references the
    // body's constant in place; the body's own array still references the
    // parameter.
    let ids = array_ids(m.nodes[call].value.unwrap());
    assert_eq!(ids.len(), 2);
    assert_eq!(
        m.equality_representative(ids[0]),
        m.equality_representative(arg)
    );
    assert!(matches!(m.nodes[ids[0]].value, Some(TestValue::U128(_))));
    assert_eq!(ids[1], seven);
    assert_eq!(m.nodes[seven].block, f); // referenced in place, not cloned
    assert_eq!(array_ids(m.nodes[ret].value.unwrap()), &[param, seven]); // body unchanged
}
#[test]
fn function_call_operator_preserves_parameterized_operand_chain() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Id(Id(x))
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None); // RETURN_IDX
    let param = m.add_node(f, None, Some(TestValue::Parameterized)); // PARAMETER_IDX
    let mid = op_node(&mut m, f, TestOperator::Id, Some(param));
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(mid),
    });
    let (func_node, _) = wrap_function(&mut m, f, ret, param);

    // The argument is itself parameterized, so the call result stays a
    // marker until the argument resolves.
    let arg = m.add_node(root, None, Some(TestValue::Parameterized));
    let call = call_node(&mut m, root, func_node, arg);
    let value = m.evaluate_node_deep(call, None);
    assert!(matches!(value, TestValue::Parameterized));
    assert_eq!(m.nodes[call].parameterized_deep, Some(true));

    // The body is untouched.
    assert!(m.blocks.contains_key(m.nodes[ret].block));
    assert_eq!(m.nodes[mid].operation.unwrap().operand, Some(param));

    // Re-bind the argument node and re-evaluate the call: the cloned chain
    // resolves through it.  The binding goes through `unify` so the whole
    // class — the parameter clone included — carries the value.
    let p = m.blocks[root].arena.alloc(99u128);
    let ninety_nine = m.add_node(root, None, Some(TestValue::U128(Handle(p as *const u128))));
    m.unify(arg, ninety_nine);
    m.nodes[call].value = None; // drop the cached marker
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 99);
}
#[test]
fn function_call_operator_recomputes_stale_definition_markers() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, ret, param) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: TestOperator::Id,
            operand: Some(param),
        });
    });

    // The definition pass evaluates the body with the parameter as a
    // marker: the transient marker is not cached (a later binding must be
    // observed on re-read), so the node keeps no value and is flagged
    // parameterized.
    m.evaluate_node_deep(ret, None);
    assert!(matches!(
        m.nodes[ret].value,
        None | Some(TestValue::Parameterized)
    ));
    assert_eq!(m.nodes[ret].parameterized_deep, Some(true));

    // The call clones the parameterized node unevaluated — the stale
    // marker is recomputed against the concrete argument.
    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);

    // The body is untouched and stays callable.
    assert_eq!(m.nodes[ret].operation.unwrap().operand, Some(param));
    assert!(m.nodes[param].value.is_some());
}
#[test]
fn function_call_operator_references_concrete_body_nodes_in_place() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Id(7): no path depends on the parameter.
    let (func_node, ret, param) = function(&mut m, |m, ret, _param| {
        let seven = u128_node(m, m.nodes[ret].block, 7);
        m.nodes[ret].operation = Some(Operation {
            operator: TestOperator::Id,
            operand: Some(seven),
        });
    });

    // The definition pass resolves the body to a concrete constant.
    m.evaluate_node_deep(ret, None);
    assert_eq!(m.nodes[ret].parameterized_deep, Some(false));

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 7);
    assert!(matches!(
        m.nodes[param].value,
        Some(TestValue::Parameterized)
    )); // body untouched
}
#[test]
fn function_in_local_block_survives_compaction() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // f(x) = Id(x), built inside a local block that is compacted away.
    let ret = m.add_node(child, None, None); // RETURN_IDX
    let param = m.add_node(child, None, Some(TestValue::Parameterized)); // PARAMETER_IDX
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(param),
    });
    let (func_node, _) = wrap_function(&mut m, child, ret, param);

    // Calling the function while it still lives in the local block works.
    let arg = u128_node(&mut m, child, 42);
    let call = call_node(&mut m, child, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);

    // Running the block compacts the function value into the root and maps
    // the template nodes along with it.
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(func_node));
    let TestValue::Function(mapped) = m.evaluate_node_deep(root_node, None) else {
        panic!("expected function")
    };
    assert!(!m.blocks.contains_key(child));
    assert_eq!(m.nodes[ret].block, root); // template mapped into the root
    assert_eq!(m.nodes[param].block, root);
    assert_eq!(m.functions[mapped].nodes, HashSet::from([ret, param]));
    assert!(m.functions.contains_key(mapped)); // the function outlives the block

    // The outer block can still call the mapped function.
    let arg2 = u128_node(&mut m, root, 7);
    let call2 = call_node(&mut m, root, func_node, arg2);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 7);
}
#[test]
fn function_scope_is_dropped_with_its_block() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // f(x) = Id(x), homed in the child but not reachable from the block's
    // return, so it must not survive the compaction.
    let ret = m.add_node(child, None, None);
    let param = m.add_node(child, None, Some(TestValue::Parameterized));
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(param),
    });
    let (func_node, func) = wrap_function(&mut m, child, ret, param);
    assert_eq!(m.functions.len(), 1);

    // Evaluate a *different* node of the child: the block compacts only the
    // return-reachable tree, then releases the rest — the function's home
    // node included, dropping the function and its scope.
    let x = u128_node(&mut m, child, 5);
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(x));
    assert_eq!(u128_of(m.evaluate_node_deep(root_node, None)), 5);

    assert!(!m.blocks.contains_key(child));
    assert!(!m.nodes.contains_key(func_node));
    assert!(!m.functions.contains_key(func)); // scope dropped with the block
}

// --- nested functions, higher-order functions, mixed blocks+functions ---
#[test]
fn nested_function_is_called_by_the_outer_body() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // g(y) = Id(y) is defined inside f's template block, and f(x) = g(x)
    // calls it with f's own parameter.
    let f = m.add_block(None);
    let gret = m.add_node(f, None, None);
    let gparam = m.add_node(f, None, Some(TestValue::Parameterized));
    m.nodes[gret].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(gparam),
    });
    let (g_node, _) = wrap_function(&mut m, f, gret, gparam);
    let ret = m.add_node(f, None, None);
    let param = m.add_node(f, None, Some(TestValue::Parameterized));
    let operands = array_node(&mut m, f, &[g_node, param], None);
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Apply,
        operand: Some(operands),
    });
    let (f_node, _) = wrap_function(&mut m, f, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: the nested g is concrete

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, f_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);

    // The nested g is untouched in the template and still callable directly.
    assert_eq!(m.nodes[g_node].block, f);
    let g_arg = u128_node(&mut m, root, 5);
    let g_call = call_node(&mut m, root, g_node, g_arg);
    assert_eq!(u128_of(m.evaluate_node_deep(g_call, None)), 5);
}
#[test]
fn outer_call_returns_a_nested_function_value() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // g(y) = Id(y) lives inside f's body; f(x) = g returns it.
    let f = m.add_block(None);
    let gret = m.add_node(f, None, None);
    let gparam = m.add_node(f, None, Some(TestValue::Parameterized));
    m.nodes[gret].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(gparam),
    });
    let (g_node, g_id) = wrap_function(&mut m, f, gret, gparam);
    let ret = m.add_node(f, None, None);
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(g_node),
    });
    let param = m.add_node(f, None, Some(TestValue::Parameterized));
    let (f_node, _) = wrap_function(&mut m, f, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: Id(g) is concrete

    let one = u128_node(&mut m, root, 1);
    let call = call_node(&mut m, root, f_node, one);
    let TestValue::Function(got) = m.evaluate_node_deep(call, None) else {
        panic!("expected the nested function value");
    };
    // A fresh closure per call: the concreteness proof of the value node
    // cannot see a nested function's body, so it must never be referenced
    // in place — its captures (if any) bind to this call's clones.
    assert_ne!(got, g_id);

    // The returned function is callable from the outer block.
    let got_node = m.add_node(root, None, Some(TestValue::Function(got)));
    let arg = u128_node(&mut m, root, 7);
    let call2 = call_node(&mut m, root, got_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 7);
    // The template g is untouched and still callable directly.
    let call3 = call_node(&mut m, root, g_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call3, None)), 7);
}
#[test]
fn a_nested_function_value_captures_the_applied_outer_parameter() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = g, where g(y) = Id(x): the nested g's body reads f's parameter
    // directly, so applying f must bind the captured x inside the returned
    // closure.  g lives in its own block; f's scope holds g's value node
    // (so the body clone instantiates it) but not g's internals, so the
    // clone folds g's own scope into the template to rewrite the capture.
    let g = m.add_block(None);
    let gparam = m.add_node(g, None, Some(TestValue::Parameterized));
    let gret = m.add_node(g, None, None);
    let (g_node, _) = wrap_function(&mut m, g, gret, gparam);
    let f = m.add_block(None);
    let param = m.add_node(f, None, Some(TestValue::Parameterized));
    m.nodes[gret].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(param),
    });
    let ret = m.add_node(f, None, None);
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(g_node),
    });
    let mut nodes = m.blocks[f].nodes.clone();
    nodes.push(g_node);
    let func_node = m.add_function(f, ret, param, nodes);

    let forty_two = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, forty_two);
    let TestValue::Function(got) = m.evaluate_node_deep(call, None) else {
        panic!("expected the nested function value");
    };

    // The returned closure reads the applied parameter: g'(7) is Id(42).
    let got_node = m.add_node(root, None, Some(TestValue::Function(got)));
    let seven = u128_node(&mut m, root, 7);
    let call2 = call_node(&mut m, root, got_node, seven);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 42);
}
#[test]
fn higher_order_function_passes_a_function_argument_through() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // apply(g) = g: the parameter is the return node's operand, so calling
    // apply with a function argument hands that function back.
    let (apply_node, ret, _param) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: TestOperator::Id,
            operand: Some(param),
        });
    });
    m.evaluate_node_deep(ret, None); // definition pass: Id(marker) stays a marker

    // g(x) = Id(x).
    let (g_node, _, _) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: TestOperator::Id,
            operand: Some(param),
        });
    });
    let TestValue::Function(g_id) = m.nodes[g_node].value.unwrap() else {
        unreachable!("expected a function value")
    };

    let call = call_node(&mut m, root, apply_node, g_node);
    let TestValue::Function(got) = m.evaluate_node_deep(call, None) else {
        panic!("expected the function argument back");
    };
    assert_eq!(got, g_id);

    // The passed-through function is still callable.
    let arg = u128_node(&mut m, root, 9);
    let call2 = call_node(&mut m, root, g_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 9);
}
#[test]
fn higher_order_function_calls_its_function_argument() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // apply(f) = f(42): the parameter is the target of an Apply node.
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(body, None, Some(TestValue::Parameterized));
    let forty_two = u128_node(&mut m, body, 42);
    let operands = array_node(&mut m, body, &[param, forty_two], None);
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Apply,
        operand: Some(operands),
    });
    let (apply_node, _) = wrap_function(&mut m, body, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: a marker target stays lazy

    // g(x) = Id(x): passing g as the argument makes apply evaluate g(42).
    let (g_node, _, _) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: TestOperator::Id,
            operand: Some(param),
        });
    });
    let call = call_node(&mut m, root, apply_node, g_node);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);
}
#[test]
fn function_can_index_into_parameterized_array() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [x, 7][0]: the array embeds the parameter, so the Index arm
    // sees a marker element and stays lazy during the definition pass.
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(body, None, Some(TestValue::Parameterized));
    let seven = u128_node(&mut m, body, 7);
    let array = array_node(&mut m, body, &[param, seven], None);
    let zero = usize_node(&mut m, body, 0);
    let operands = array_node(&mut m, body, &[array, zero], None);
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Index,
        operand: Some(operands),
    });
    let (f_node, _) = wrap_function(&mut m, body, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: index of a marker stays a marker
    assert_eq!(m.nodes[ret].parameterized_deep, Some(true));

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, f_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);

    // The body is untouched and still parameterized.
    assert!(matches!(
        m.nodes[param].value,
        Some(TestValue::Parameterized)
    ));
}
#[test]
fn manually_partially_evaluated_function_applies_correctly() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Add(Add(x, 1), 2).  The partial definition is built by hand:
    // evaluate_node_deep is called on exactly the two constants, and
    // nothing else — the parameter-dependent chain stays unevaluated.
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(body, None, Some(TestValue::Parameterized));
    let one = u128_node(&mut m, body, 1);
    let inner_ops = array_node(&mut m, body, &[param, one], None);
    let inner = op_node(&mut m, body, TestOperator::Add, Some(inner_ops));
    let two = u128_node(&mut m, body, 2);
    let ret_ops = array_node(&mut m, body, &[inner, two], None);
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Add,
        operand: Some(ret_ops),
    });
    let (f_node, _) = wrap_function(&mut m, body, ret, param);

    // Manually define exactly the constants; the parameter-dependent nodes
    // keep parameterized_deep = None.
    m.evaluate_node_deep(one, None);
    m.evaluate_node_deep(two, None);
    assert_eq!(m.nodes[one].parameterized_deep, Some(false));
    assert_eq!(m.nodes[two].parameterized_deep, Some(false));
    assert_eq!(m.nodes[ret].parameterized_deep, None);
    assert_eq!(m.nodes[inner].parameterized_deep, None);
    assert_eq!(m.nodes[inner_ops].parameterized_deep, None);
    assert_eq!(m.nodes[ret_ops].parameterized_deep, None);

    // The apply reuses the proven constants in place and clones + remaps
    // the unevaluated chain: f(5) = (5 + 1) + 2 = 8, f(9) = 12.
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 8);

    // The clone of the inner operand array keeps the proven constant in
    // place and maps the parameter onto a fresh clone unified with the
    // argument.
    let candidates: Vec<NodeId> = m.blocks[root]
        .nodes
        .iter()
        .copied()
        .filter(|&id| matches!(m.nodes[id].value, Some(TestValue::Array(_))))
        .collect();
    let cloned_inner_ops = candidates
        .into_iter()
        .find(|&id| {
            let ids = array_ids(m.nodes[id].value.unwrap());
            ids.len() == 2
                && ids[1] == one
                && m.equality_representative(ids[0]) == m.equality_representative(five)
        })
        .expect("the cloned inner operand array references the parameter's clone");
    assert_eq!(m.nodes[cloned_inner_ops].block, root);

    let nine = u128_node(&mut m, root, 9);
    let call2 = call_node(&mut m, root, f_node, nine);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 12);
}
#[test]
fn unevaluated_function_applies_correctly() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Add(x, 1), with no evaluate_node / evaluate_node_deep call
    // before applying: every body node keeps parameterized_deep = None.
    let (f_node, ret, param) = function(&mut m, |m, ret, param| {
        let one = u128_node(m, m.nodes[ret].block, 1);
        let operands = array_node(m, m.nodes[ret].block, &[param, one], None);
        m.nodes[ret].operation = Some(Operation {
            operator: TestOperator::Add,
            operand: Some(operands),
        });
    });
    assert_eq!(m.nodes[ret].parameterized_deep, None);
    assert_eq!(m.nodes[param].parameterized_deep, None);

    // The apply clones the whole unevaluated body and resolves it against
    // the argument: f(5) = 6, f(9) = 10.
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 6);

    let nine = u128_node(&mut m, root, 9);
    let call2 = call_node(&mut m, root, f_node, nine);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 10);
}
#[test]
fn mixed_blocks_and_functions_survive_compaction() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // g(x) = Add(x, 7) is built inside the child block; the constant 7
    // lives in the child and is part of g's scope.
    let ret = m.add_node(child, None, None);
    let param = m.add_node(child, None, Some(TestValue::Parameterized));
    let seven = u128_node(&mut m, child, 7);
    let operands = array_node(&mut m, child, &[param, seven], None);
    m.nodes[ret].operation = Some(Operation {
        operator: TestOperator::Add,
        operand: Some(operands),
    });
    let (g_node, g_id) = wrap_function(&mut m, child, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass (Add is marker-aware)

    // Call g from inside the child: 10 + 7.
    let ten = u128_node(&mut m, child, 10);
    let call = call_node(&mut m, child, g_node, ten);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 17);

    // The root pulls g out of the child: compaction re-homes the function
    // and moves its whole scope, then releases the rest of the block.
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(g_node));
    let TestValue::Function(mapped) = m.evaluate_node_deep(root_node, None) else {
        panic!("expected function")
    };
    assert_eq!(mapped, g_id);
    assert_eq!(m.functions[g_id].block, root); // re-homed to the root
    assert_eq!(m.nodes[seven].block, root); // scope constant moved too
    assert!(!m.blocks.contains_key(child));

    // Still callable after compaction: 3 + 7.
    let three = u128_node(&mut m, root, 3);
    let call2 = call_node(&mut m, root, g_node, three);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 10);
}
#[test]
fn call_return_is_shallow_for_container_bodies() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [x, Id(x)] — the second element depends on the parameter, so
    // the definition pass leaves it a marker and the call clones it
    // unevaluated rather than forcing it.
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None); // RETURN_IDX
    let param = m.add_node(f, None, Some(TestValue::Parameterized)); // PARAMETER_IDX
    let id2 = op_node(&mut m, f, TestOperator::Id, Some(param));
    let slice = m.blocks[f].arena.alloc_slice_copy(&[param, id2]);
    m.nodes[ret].value = Some(TestValue::Array(ArrayRef::new(
        std::ptr::slice_from_raw_parts(slice.as_ptr(), slice.len()),
    )));
    let (func_node, _) = wrap_function(&mut m, f, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass flags the array parameterized
    assert_eq!(m.nodes[ret].parameterized_deep, Some(true));

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);

    // Shallow evaluation of the call node returns the array without
    // forcing the elements: the Id(x) clone stays unevaluated.
    let value = m.evaluate_node(call, None);
    let ids = array_ids(value);
    assert_eq!(ids.len(), 2);
    assert_eq!(
        m.equality_representative(ids[0]),
        m.equality_representative(arg)
    ); // the cloned parameter, unified with the argument
    assert!(m.nodes[ids[1]].value.is_none()); // the Id clone is still lazy

    // Deep evaluation forces them.
    let deep = m.evaluate_node_deep(call, None);
    assert_u128_array(&m, deep, &[42, 42]);
}
#[test]
fn apply_evaluates_argument_elements_to_match_the_parameter_pattern() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = x with x = [x0, x1]: an array parameter pattern.  The
    // argument's elements are unevaluated operations — the apply must
    // evaluate them (to the pattern's depth) before the elementwise unify,
    // or they would read as unbound and bind nothing.
    let x0 = m.add_node(root, None, Some(TestValue::Parameterized));
    let x1 = m.add_node(root, None, Some(TestValue::Parameterized));
    let slice = m.blocks[root].arena.alloc_slice_copy(&[x0, x1]);
    let param = m.add_node(
        root,
        None,
        Some(TestValue::Array(ArrayRef::new(
            std::ptr::slice_from_raw_parts(slice.as_ptr(), slice.len()),
        ))),
    );
    let f = m.add_function(root, param, param, [param, x0, x1]);
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
    let value = m.evaluate_node_deep(call, None);
    assert!(m.unify_errors.is_empty());
    assert_u128_array(&m, value, &[3, 2]);
}
#[test]
fn apply_clone_preserves_the_shallow_mask() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [x, Add(x, 1)] with position 1 marked shallow: the deep pass
    // on the definition keeps the Add lazy, and the apply's clone must
    // carry the mask so the marked element stays lazy in the call too.
    let f = m.add_block(None);
    let param = m.add_node(f, None, Some(TestValue::Parameterized));
    let one = u128_node(&mut m, f, 1);
    let add_ops = array_node(&mut m, f, &[param, one], None);
    let add = op_node(&mut m, f, TestOperator::Add, Some(add_ops));
    let ret = array_node(&mut m, f, &[param, add], Some(&[false, true]));
    let (func_node, _) = wrap_function(&mut m, f, ret, param);

    m.evaluate_node_deep(ret, None); // definition pass
    assert_eq!(m.nodes[ret].parameterized_deep, Some(true));

    let arg = u128_node(&mut m, root, 10);
    let call = call_node(&mut m, root, func_node, arg);
    let value = m.evaluate_node_deep(call, None);

    let TestValue::Array(cloned) = value else {
        panic!("expected an array result")
    };
    assert_eq!(cloned.mask(), &[false, true], "the mask travels through the clone");
    assert_eq!(u128_of(m.nodes[cloned.ids()[0]].value.unwrap()), 10);
    assert!(
        m.nodes[cloned.ids()[1]].value.is_none(),
        "the marked element stays lazy in the call"
    );
}
#[test]
fn pattern_argument_evaluation_skips_shallow_positions() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = x with x = [x0, x1]: the pattern's position 1 is shallow, so
    // the apply must not force the argument's element there.
    let x0 = m.add_node(root, None, Some(TestValue::Parameterized));
    let x1 = m.add_node(root, None, Some(TestValue::Parameterized));
    let param = array_node(&mut m, root, &[x0, x1], Some(&[false, true]));
    let f = m.add_function(root, param, param, [param, x0, x1]);
    // argument: [7, Add(1, 2)] — the Add is at the masked position.
    let seven = u128_node(&mut m, root, 7);
    let one = u128_node(&mut m, root, 1);
    let two = u128_node(&mut m, root, 2);
    let add_ops = array_node(&mut m, root, &[one, two], None);
    let add = op_node(&mut m, root, TestOperator::Add, Some(add_ops));
    let arg = array_node(&mut m, root, &[seven, add], None);
    let call = call_node(&mut m, root, f, arg);
    let value = m.evaluate_node_deep(call, None);

    assert!(m.unify_errors.is_empty());
    let ids = array_ids(value);
    assert_eq!(ids.len(), 2);
    assert_eq!(u128_of(m.nodes[ids[0]].value.unwrap()), 7);
    // The shallow pattern position stays lazy: the argument's element there
    // (an unevaluated Add) is never forced by the apply, and the masked
    // position itself is never walked.
    assert!(
        m.nodes[add].value.is_none(),
        "the shallow pattern position is not forced by the apply"
    );
    assert_eq!(
        m.nodes[ids[1]].parameterized_deep,
        None,
        "the masked position is never walked"
    );
}
