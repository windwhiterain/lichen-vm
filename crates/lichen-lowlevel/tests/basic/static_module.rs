//! Static module dependencies: a fully-solved [`StaticModule`] registered
//! in the device's [`Registry`] and used in place.  The crisis demos of the
//! feature note (`docs/notes/static-modules.md`): residual re-open,
//! constant baking, the shared-arena rule, device-key resolution,
//! recursion, parameter topology, per-call asserts, GC, and static closures.

use super::*;
use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, LowOperator, ModuleKey, Registry,
    StaticFunctionId, StaticFunctionRef, StaticNodeId,
};
use std::sync::{Arc, RwLock};

/// Freeze `source` into `m`'s registry under a device key — the tests pick
/// distinct compact indices (the device registry allocates them in
/// production; the lowlevel only files under the caller-provided key).
fn freeze(m: &mut Module<TestProgram>, source: &Module<TestProgram>, key: u64) -> ModuleKey {
    m.freeze(source, ModuleKey::from_raw(key), [0; 32])
}

/// A static function value ref — absolute, as the walk emits it.
fn static_func_value(
    m: &mut Module<TestProgram>,
    block: BlockId,
    module: ModuleKey,
    index: usize,
) -> NodeId {
    m.add_node(
        block,
        None,
        Some(TestValue::LowValue(LowValue::Function(
            AnyFunctionId::Static(StaticFunctionRef {
                module,
                index: StaticFunctionId(index),
            }),
        ))),
    )
}

/// The raw items of an evaluated array value.
fn raw_items(value: TestValue) -> Vec<ArrayItem> {
    let TestValue::LowValue(LowValue::Array(array)) = value else {
        panic!("expected an array value")
    };
    array.items().to_vec()
}

#[test]
fn static_apply_reruns_the_residual_spine_against_the_argument() {
    // Source: f(x) = x + 1.  At solve time the Add reads the marker
    // parameter and freezes `Parameterized` with a dead residual operation;
    // the materialize walk must clone that spine and re-run it per call.
    let mut m = Module::new();
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let one = u128_node(&mut m, body, 1);
    let add_ops = array_node(&mut m, body, &[param, one], None);
    let add = op_node(&mut m, body, TestOperator::Add, Some(add_ops));
    let (func_node, _) = wrap_function(&mut m, body, add, param);
    m.evaluate_node_deep(add, None);

    let mut imp = Module::new();
    let root = imp.add_block(None);
    let key = freeze(&mut imp, &m, 1);
    let f = static_func_value(&mut imp, root, key, 0);
    let _ = func_node;
    let arg = u128_node(&mut imp, root, 41);
    let call = call_node(&mut imp, root, f, arg);
    assert_eq!(u128_of(imp.evaluate_node_deep(call, None)), 42);

    // A second call materializes a fresh instance.
    let arg = u128_node(&mut imp, root, 10);
    let call = call_node(&mut imp, root, f, arg);
    assert_eq!(u128_of(imp.evaluate_node_deep(call, None)), 11);
}

#[test]
fn static_apply_bakes_constants_in_place() {
    // Source: f(x) = [x, 42] — the return pair's second element is a
    // constant: the walk bakes it (an inline absolute static ref), so the
    // result shares the module's payload instead of copying it.
    let mut m = Module::new();
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let forty_two = u128_node(&mut m, body, 42);
    m.write_node_value(
        ret,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&[item(param), item(forty_two)], body),
        ))),
    );
    let (func_node, _) = wrap_function(&mut m, body, ret, param);
    m.evaluate_node_deep(ret, None);
    let mut imp = Module::new();
    let root = imp.add_block(None);
    let key = freeze(&mut imp, &m, 1);
    let f = static_func_value(&mut imp, root, key, 0);
    let arg = u128_node(&mut imp, root, 7);
    let call = call_node(&mut imp, root, f, arg);

    let value = imp.evaluate_node_deep(call, None);
    let _ = func_node;
    let items = raw_items(value);
    assert_eq!(items.len(), 2);
    // Element 1 is the baked constant: an inline static ref (no importer
    // node was created for it).
    let AnyNodeId::Static(_) = items[1].node else {
        panic!("the constant must be referenced in place, not cloned")
    };
    assert_eq!(u128_of(imp.evaluate_node(items[1].node, None)), 42);

    // The baked constant's payload lives in the module's shared arena —
    // the read copied nothing into the importer.
    let module = imp
        .registry
        .read()
        .unwrap()
        .get(key)
        .expect("the registered module")
        .module
        .clone();
    let (base, end) = {
        let arena = &module.arena;
        (
            arena.as_ptr() as usize,
            arena.as_ptr() as usize + arena.len(),
        )
    };
    let constant = imp.evaluate_node(items[1].node, None);
    let TestValue::U128(AnyHandle::Static(h)) = constant else {
        panic!("expected the baked static constant")
    };
    let addr = h.offset as usize;
    assert!(
        addr >= base && addr < end,
        "the payload must stay in the shared static arena"
    );
}

#[test]
fn nested_index_over_a_static_array_reads_shared_values() {
    // Source: f(x) = [[1,2],[3,4]] — a fully-baked constant.  The importer
    // reads it through a nested index: the inner Index node caches the
    // element array as the module's own static payload, and a later re-read
    // of the same node must still resolve (every ref is keyed and absolute,
    // so the cached value is unambiguous from any context).
    let mut m = Module::new();
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let n1 = u128_node(&mut m, body, 1);
    let n2 = u128_node(&mut m, body, 2);
    let n3 = u128_node(&mut m, body, 3);
    let n4 = u128_node(&mut m, body, 4);
    let inner1 = array_node(&mut m, body, &[n1, n2], None);
    let inner2 = array_node(&mut m, body, &[n3, n4], None);
    let arr = array_node(&mut m, body, &[inner1, inner2], None);
    let (func_node, _) = wrap_function(&mut m, body, arr, param);
    m.evaluate_node_deep(arr, None);

    let mut imp = Module::new();
    let root = imp.add_block(None);
    let key = freeze(&mut imp, &m, 1);
    let f = static_func_value(&mut imp, root, key, 0);
    let arg = u128_node(&mut imp, root, 0);
    let call = call_node(&mut imp, root, f, arg);
    // arr[1] — the element is the module's own inner array, read in place.
    let one = usize_node(&mut imp, root, 1);
    let idx1_ops = array_node(&mut imp, root, &[call, one], None);
    let inner = op_node(
        &mut imp,
        root,
        TestOperator::LowOperator(LowOperator::Index),
        Some(idx1_ops),
    );
    // arr[1][0]
    let zero = usize_node(&mut imp, root, 0);
    let idx2_ops = array_node(&mut imp, root, &[inner, zero], None);
    let idx2 = op_node(
        &mut imp,
        root,
        TestOperator::LowOperator(LowOperator::Index),
        Some(idx2_ops),
    );
    assert_eq!(u128_of(imp.evaluate_node_deep(idx2, None)), 3);

    // The inner Index node cached the shared [3,4]; a direct re-read must
    // resolve it like any memoized node.
    let again = imp.evaluate_node(Dyn(inner), None);
    let items = raw_items(again);
    assert_eq!(items.len(), 2);
    assert_eq!(u128_of(imp.evaluate_node(items[0].node, None)), 3);
    assert_eq!(u128_of(imp.evaluate_node(items[1].node, None)), 4);
    let _ = func_node;
}

#[test]
fn registry_resolves_artifacts_by_device_key() {
    // One freeze → one device key → one artifact: refs baked with the key
    // resolve through the registry, and repeated `get`s return the same
    // resident module.  Re-freezing the same source compiles a NEW artifact
    // under a NEW key — both resolve independently, since a registration is
    // per artifact, not per source.
    let mut m = Module::new();
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let seven = u128_node(&mut m, body, 7);
    m.write_node_value(
        ret,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&[item(param), item(seven)], body),
        ))),
    );
    let (func_node, _) = wrap_function(&mut m, body, ret, param);
    m.evaluate_node_deep(ret, None);

    let mut imp = Module::new();
    let root = imp.add_block(None);
    let k1 = freeze(&mut imp, &m, 1);
    let k2 = freeze(&mut imp, &m, 2);
    assert_ne!(k1, k2, "a distinct key files a distinct artifact");
    for k in [k1, k2] {
        let f = static_func_value(&mut imp, root, k, 0);
        let arg = u128_node(&mut imp, root, 0);
        let call = call_node(&mut imp, root, f, arg);
        let value = imp.evaluate_node_deep(call, None);
        let items = raw_items(value);
        assert_eq!(u128_of(imp.evaluate_node(items[1].node, None)), 7);
    }
    // Repeated gets of one key return the same resident module.
    let a = imp.registry.read().unwrap().get(k1).unwrap().module.clone();
    let b = imp.registry.read().unwrap().get(k1).unwrap().module.clone();
    assert!(
        std::sync::Arc::ptr_eq(&a, &b),
        "the registry stores one artifact per key"
    );
    let _ = func_node;
}

#[test]
fn static_recursion_counts_down_through_a_lazy_branch() {
    // Source: f(x) = if x == 0 then 0 else f(x - 1), with the branch lazy
    // (`Index([f(x-1), 0], x == 0)`).  The self-reference is the baked
    // static function value; each importer apply materializes one level and
    // dispatches back to `static_function_apply` for the next.
    let mut m = Module::new();
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let func_node = m.add_node(body, None, None); // placeholder self-ref
    let zero = u128_node(&mut m, body, 0);
    let one = u128_node(&mut m, body, 1);
    let sub_ops = array_node(&mut m, body, &[param, one], None);
    let sub = op_node(&mut m, body, TestOperator::Sub, Some(sub_ops));
    let rec_ops = array_node(&mut m, body, &[func_node, sub], None);
    let rec = op_node(
        &mut m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(rec_ops),
    );
    let eq_ops = array_node(&mut m, body, &[param, zero], None);
    let eq = op_node(&mut m, body, TestOperator::Eq, Some(eq_ops));
    let branch = array_node(&mut m, body, &[rec, zero], None);
    let idx_ops = array_node(&mut m, body, &[branch, eq], None);
    let ret = op_node(
        &mut m,
        body,
        TestOperator::LowOperator(LowOperator::Index),
        Some(idx_ops),
    );
    finish_function(&mut m, body, ret, param, func_node);
    m.evaluate_node_deep(ret, None);
    let mut imp = Module::new();
    let root = imp.add_block(None);
    imp.apply_depth_limit = 200;
    imp.apply_total_limit = 10_000;
    let key = freeze(&mut imp, &m, 1);
    let f = static_func_value(&mut imp, root, key, 0);
    let arg = u128_node(&mut imp, root, 3);
    let call = call_node(&mut imp, root, f, arg);
    assert_eq!(u128_of(imp.evaluate_node_deep(call, None)), 0);

    // The depth guard still bounds a non-terminating static self-apply.
    let mut m = Module::new();
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let g_func = m.add_node(body, None, None);
    let g_ops = array_node(&mut m, body, &[g_func, param], None);
    let g_ret = op_node(
        &mut m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(g_ops),
    );
    finish_function(&mut m, body, g_ret, param, g_func);
    // No solve pass: an unconditional self-apply never terminates, so every
    // node stays unproven (parameterized) and materializes per call — the
    // static depth guard bounds it.
    let mut imp = Module::new();
    let root = imp.add_block(None);
    imp.apply_depth_limit = 20;
    imp.apply_total_limit = 1_000;
    let key = freeze(&mut imp, &m, 1);
    let g = static_func_value(&mut imp, root, key, 0);
    let arg = u128_node(&mut imp, root, 0);
    let call = call_node(&mut imp, root, g, arg);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        imp.evaluate_node_deep(call, None)
    }));
    assert!(
        result.is_err(),
        "the depth guard must trip on static self-apply"
    );
}

#[test]
fn static_parameter_topology_is_reestablished_among_clones() {
    // Source: f([x0, x1]) = x0 with x0 and x1 unified at build (a
    // homogeneous array pattern).  The materialize walk must re-unify the
    // parameter's clones, so an argument whose elements differ fails the
    // parameter check with an ApplyError — otherwise the mismatch would
    // silently pass.
    let mut m = Module::new();
    let body = m.add_block(None);
    let x0 = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let x1 = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    m.unify(x0, x1);
    let items = [item(x0), item(x1)];
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&items, body),
        ))),
    );
    let (func_node, _) = wrap_function(&mut m, body, x0, param);
    m.evaluate_node_deep(x0, None);

    let mut imp = Module::new();
    let root = imp.add_block(None);
    let key = freeze(&mut imp, &m, 1);
    let f = static_func_value(&mut imp, root, key, 0);

    // Equal elements: the pattern is satisfied.
    let a = u128_node(&mut imp, root, 3);
    let b = u128_node(&mut imp, root, 3);
    let arg = array_node(&mut imp, root, &[a, b], None);
    let call = call_node(&mut imp, root, f, arg);
    assert_eq!(u128_of(imp.evaluate_node_deep(call, None)), 3);
    assert!(imp.apply_errors.is_empty());

    // Differing elements: the re-established topology rejects the argument.
    let a = u128_node(&mut imp, root, 3);
    let b = u128_node(&mut imp, root, 4);
    let arg = array_node(&mut imp, root, &[a, b], None);
    let call = call_node(&mut imp, root, f, arg);
    let _ = imp.evaluate_node_deep(call, None);
    assert!(
        !imp.apply_errors.is_empty(),
        "the mismatch must fail the parameter check"
    );
    let _ = func_node;
}

#[test]
fn static_assert_rechecks_per_call() {
    // Source: f(x) = x with the body assert `x == 1` — parameterized at
    // solve, so it stays pending; the importer's materialize instantiates
    // it per call, and check_asserts sees the argument.
    let mut m = Module::new();
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let one = u128_node(&mut m, body, 1);
    let eq_ops = array_node(&mut m, body, &[param, one], None);
    let condition = op_node(&mut m, body, TestOperator::Eq, Some(eq_ops));
    m.add_assert(condition);
    m.write_node_value(
        ret,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&[item(param)], body),
        ))),
    );
    let (func_node, _) = wrap_function_asserts(&mut m, body, ret, param, [condition]);
    m.evaluate_node_deep(ret, None);
    let mut imp = Module::new();
    let root = imp.add_block(None);
    let key = freeze(&mut imp, &m, 1);
    let f = static_func_value(&mut imp, root, key, 0);

    // A satisfying argument: the per-call clone checks out.
    let arg = u128_node(&mut imp, root, 1);
    let call = call_node(&mut imp, root, f, arg);
    let _ = imp.evaluate_node_deep(call, None);
    imp.check_asserts();
    assert!(imp.assert_errors.is_empty());

    // A violating argument: the instantiated condition resolves to 0.
    let arg = u128_node(&mut imp, root, 2);
    let call = call_node(&mut imp, root, f, arg);
    let _ = imp.evaluate_node_deep(call, None);
    imp.check_asserts();
    assert!(
        !imp.assert_errors.is_empty(),
        "the per-call assert must fire for x = 2"
    );
    let _ = func_node;
}

#[test]
fn materialized_clones_survive_block_release() {
    // The materialized clones land in the apply's block; compaction must
    // move them with it, and a second apply must still work.
    let mut m = Module::new();
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let one = u128_node(&mut m, body, 1);
    let add_ops = array_node(&mut m, body, &[param, one], None);
    let add = op_node(&mut m, body, TestOperator::Add, Some(add_ops));
    let (func_node, _) = wrap_function(&mut m, body, add, param);
    m.evaluate_node_deep(add, None);

    let mut imp = Module::new();
    let root = imp.add_block(None);
    let key = freeze(&mut imp, &m, 1);
    let f = static_func_value(&mut imp, root, key, 0);

    // Apply inside a child block: compaction moves the materialized clones
    // into the root and releases the block.
    let child = imp.add_block(Some(root));
    let arg = u128_node(&mut imp, child, 41);
    let call = call_node(&mut imp, child, f, arg);
    assert_eq!(u128_of(imp.evaluate_node_deep(call, None)), 42);
    imp.garbage_collect(call).expect("the evaluated call node");
    assert!(
        !imp.blocks.contains_key(child),
        "the child block was released"
    );

    // The static function is still callable afterwards.
    let arg = u128_node(&mut imp, root, 10);
    let call = call_node(&mut imp, root, f, arg);
    assert_eq!(u128_of(imp.evaluate_node_deep(call, None)), 11);
    let _ = func_node;
}

#[test]
fn static_closure_value_applies_from_dynamic_context() {
    // Source: g(x) = x + 1 and f(y) = [y, g] — the closure g rides inside
    // f's return pair as a static function value (baked).  The importer
    // extracts it and applies it.
    let mut m = Module::new();
    // g(x) = x + 1
    let g_body = m.add_block(None);
    let g_param = m.add_node(
        g_body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let one = u128_node(&mut m, g_body, 1);
    let g_ops = array_node(&mut m, g_body, &[g_param, one], None);
    let g_add = op_node(&mut m, g_body, TestOperator::Add, Some(g_ops));
    let (g_func_node, _) = wrap_function(&mut m, g_body, g_add, g_param);
    // f(y) = [y, g]
    let f_body = m.add_block(None);
    let f_ret = m.add_node(f_body, None, None);
    let f_param = m.add_node(
        f_body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let g_value = m.add_node(f_body, None, None); // placeholder: g's value node
    m.write_node_value(g_value, m.node_value(AnyNodeId::Dynamic(g_func_node)));
    m.write_node_value(
        f_ret,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&[item(f_param), item(g_value)], f_body),
        ))),
    );
    let (f_func_node, _) = wrap_function(&mut m, f_body, f_ret, f_param);
    // Solve both bodies.
    m.evaluate_node_deep(g_add, None);
    m.evaluate_node_deep(f_ret, None);

    let mut imp = Module::new();
    let root = imp.add_block(None);
    let key = freeze(&mut imp, &m, 1);
    // g was inserted first (function 0), f second (function 1).
    let f = static_func_value(&mut imp, root, key, 1);
    let arg = u128_node(&mut imp, root, 5);
    let call = call_node(&mut imp, root, f, arg);
    let value = imp.evaluate_node_deep(call, None);
    let items = raw_items(value);
    assert_eq!(items.len(), 2);
    // The second element is the baked g value: a static function ref.
    let g_val = imp.evaluate_node(items[1].node, None);
    let TestValue::LowValue(LowValue::Function(AnyFunctionId::Static(_))) = g_val else {
        panic!("expected a baked static function value")
    };
    let g_node = imp.add_node(root, None, Some(g_val));
    let arg = u128_node(&mut imp, root, 41);
    let g_call = call_node(&mut imp, root, g_node, arg);
    assert_eq!(u128_of(imp.evaluate_node_deep(g_call, None)), 42);
    let _ = f_func_node;
}

#[test]
fn from_module_dedupes_shared_payloads() {
    // Two nodes sharing one arena payload must share one static region, so
    // their handles stay identity-equal.
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = u128_node(&mut m, block, 42);
    let b = m.add_node(block, None, None);
    m.write_node_value(b, m.node_value(AnyNodeId::Dynamic(a))); // the same handle
    let mut imp = Module::new();
    let _root = imp.add_block(None);
    let key = freeze(&mut imp, &m, 1);
    let sm = imp
        .registry
        .read()
        .unwrap()
        .get(key)
        .expect("the registered module")
        .module
        .clone();
    let va = sm.nodes[0].value.unwrap();
    let vb = sm.nodes[1].value.unwrap();
    assert_eq!(va, vb, "deduped payloads keep identity equality");
    assert_eq!(u128_of(va), 42);
}

#[test]
fn static_array_cache_survives_block_release_verbatim() {
    // Source: f(x) = [[1,2],[3,4]].  An Index over the result caches the
    // module's own [3,4] payload as the importer node's value; compaction
    // must keep such a value verbatim (there is no block to move it out
    // of), and the cached read must still resolve afterwards.
    let mut m = Module::new();
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let n1 = u128_node(&mut m, body, 1);
    let n2 = u128_node(&mut m, body, 2);
    let n3 = u128_node(&mut m, body, 3);
    let n4 = u128_node(&mut m, body, 4);
    let inner1 = array_node(&mut m, body, &[n1, n2], None);
    let inner2 = array_node(&mut m, body, &[n3, n4], None);
    let arr = array_node(&mut m, body, &[inner1, inner2], None);
    let (func_node, _) = wrap_function(&mut m, body, arr, param);
    m.evaluate_node_deep(arr, None);

    let mut imp = Module::new();
    let root = imp.add_block(None);
    let key = freeze(&mut imp, &m, 1);
    let f = static_func_value(&mut imp, root, key, 0);
    let child = imp.add_block(Some(root));
    let arg = u128_node(&mut imp, child, 0);
    let call = call_node(&mut imp, child, f, arg);
    let one = usize_node(&mut imp, child, 1);
    let idx_ops = array_node(&mut imp, child, &[call, one], None);
    let inner = op_node(
        &mut imp,
        child,
        TestOperator::LowOperator(LowOperator::Index),
        Some(idx_ops),
    );
    imp.evaluate_node_deep(inner, None);

    // The Index node's cached value is the module's own [3,4] payload.
    let cached = imp
        .node_value(AnyNodeId::Dynamic(inner))
        .expect("the index cached the element");
    let TestValue::LowValue(LowValue::Array(AnyHandle::Static(_))) = cached else {
        panic!("the cached element must be the module's own static payload")
    };

    // Compaction moves the Index node into the root and releases the child:
    // the static value rides along verbatim and still resolves.
    imp.garbage_collect(inner)
        .expect("the evaluated index node");
    assert!(
        !imp.blocks.contains_key(child),
        "the child block was released"
    );
    let moved = imp
        .node_value(AnyNodeId::Dynamic(inner))
        .expect("the value survived compaction");
    let TestValue::LowValue(LowValue::Array(AnyHandle::Static(_))) = moved else {
        panic!("compaction must keep a static payload verbatim")
    };
    let items = raw_items(moved);
    assert_eq!(items.len(), 2);
    assert_eq!(u128_of(imp.evaluate_node(items[0].node, None)), 3);
    assert_eq!(u128_of(imp.evaluate_node(items[1].node, None)), 4);
    let _ = func_node;
}

#[test]
fn freeze_mapped_returns_consistent_node_indices() {
    let mut m = Module::new();
    let body = m.add_block(None);
    let a = u128_node(&mut m, body, 10);
    let b = u128_node(&mut m, body, 20);
    let arr = array_node(&mut m, body, &[a, b], None);
    m.evaluate_node_deep(arr, None);

    let mut imp = Module::new();
    let root = imp.add_block(None);
    let freeze = imp.freeze_mapped(&m, ModuleKey::from_raw(1), [0; 32]);
    let node_map = &freeze.node_map;

    assert_eq!(node_map.len(), m.nodes.len());
    let arr_sref = lichen_lowlevel::StaticNodeId {
        module: freeze.key,
        index: node_map[&arr],
    };
    let raw = raw_items(imp.evaluate_node(AnyNodeId::Static(arr_sref), Some(root)));
    assert_eq!(raw.len(), 2);
    assert_eq!(u128_of(imp.evaluate_node(raw[0].node, None)), 10);
    assert_eq!(u128_of(imp.evaluate_node(raw[1].node, None)), 20);
}

/// A dependency module frozen into a shared registry, plus the static refs
/// an importer may hold: the registry, A's key, and srefs to A's constant
/// and array nodes.
fn frozen_dependency() -> (
    Arc<RwLock<Registry<TestProgram>>>,
    ModuleKey,
    StaticNodeId,
    StaticNodeId,
) {
    let registry = Arc::new(RwLock::new(Registry::new()));
    let mut a = Registry::new_module(&registry);
    let block = a.add_block(None);
    let constant = u128_node(&mut a, block, 42);
    let n10 = u128_node(&mut a, block, 10);
    let n20 = u128_node(&mut a, block, 20);
    let arr = array_node(&mut a, block, &[n10, n20], None);
    a.evaluate_node_deep(arr, None);
    let freeze = registry
        .write()
        .unwrap()
        .freeze_mapped(&a, ModuleKey::from_raw(1), [0; 32]);
    (
        registry,
        freeze.key,
        StaticNodeId {
            module: freeze.key,
            index: freeze.node_map[&constant],
        },
        StaticNodeId {
            module: freeze.key,
            index: freeze.node_map[&arr],
        },
    )
}

#[test]
fn freezing_keeps_dependency_refs_verbatim() {
    // A transitive freeze: module B is bound to the registry that already
    // holds A, and B's values reference A (an inline static item and a
    // materialized A leaf).  Freezing B keeps every A ref verbatim — keyed
    // by A, payload in A's shared arena — and B's artifact resolves through
    // the shared registry from any importer.
    let (registry, key_a, const_sref, arr_sref) = frozen_dependency();

    let mut b = Registry::new_module(&registry);
    let root = b.add_block(None);
    let local = u128_node(&mut b, root, 7);
    let a_leaf = b.materialize_leaf(const_sref, root);
    let items = [
        item(local),
        ArrayItem::new(AnyNodeId::Static(arr_sref)),
        item(a_leaf),
    ];
    let holder = b.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Array(
            b.alloc_array(&items, root),
        ))),
    );
    b.evaluate_node_deep(holder, None);

    let freeze_b = registry
        .write()
        .unwrap()
        .freeze_mapped(&b, ModuleKey::from_raw(2), [0; 32]);
    assert_ne!(freeze_b.key, key_a, "B is a distinct artifact");

    // The frozen array's middle item still names module A, verbatim.
    let b_module = registry
        .read()
        .unwrap()
        .get(freeze_b.key)
        .expect("the frozen importer")
        .module
        .clone();
    let holder_value = b_module.nodes[freeze_b.node_map[&holder].index]
        .value
        .expect("the holder's frozen value");
    let items = raw_items(holder_value);
    assert_eq!(
        items[1].node,
        AnyNodeId::Static(arr_sref),
        "the dependency ref is kept verbatim, keyed by the dependency"
    );
    // The materialized A leaf keeps A's static handle — the payload stays
    // in A's arena, never copied into B's.
    let TestValue::U128(AnyHandle::Static(handle)) = b_module.nodes
        [freeze_b.node_map[&a_leaf].index]
        .value
        .expect("the leaf's value")
    else {
        panic!("expected the baked static constant");
    };
    assert_eq!(handle.module, key_a, "the baked payload stays in A's arena");

    // An importer of B reads through both modules: B's local item, A's
    // array item, and A's baked constant resolve through the one registry.
    let mut imp = Registry::new_module(&registry);
    let iroot = imp.add_block(None);
    let holder_sref = StaticNodeId {
        module: freeze_b.key,
        index: freeze_b.node_map[&holder],
    };
    let node = imp.materialize_leaf(holder_sref, iroot);
    let value = imp.evaluate_node(Dyn(node), None);
    let items = raw_items(value);
    assert_eq!(u128_of(imp.evaluate_node(items[0].node, None)), 7);
    let inner = raw_items(imp.evaluate_node(items[1].node, None));
    assert_eq!(u128_of(imp.evaluate_node(inner[0].node, None)), 10);
    assert_eq!(u128_of(imp.evaluate_node(inner[1].node, None)), 20);
    assert_eq!(u128_of(imp.evaluate_node(items[2].node, None)), 42);
}

#[test]
#[should_panic(expected = "which is not registered here")]
fn freeze_rejects_an_unregistered_dependency_key() {
    // B is built against a registry holding A, so its values carry A-keyed
    // refs; freezing B into a *different* registry would bake refs that
    // cannot resolve there.  The freeze rejects the unregistered key.
    let (registry, _key_a, _const_sref, arr_sref) = frozen_dependency();

    let mut b = Registry::new_module(&registry);
    let root = b.add_block(None);
    let local = u128_node(&mut b, root, 7);
    let items = [item(local), ArrayItem::new(AnyNodeId::Static(arr_sref))];
    let holder = b.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Array(
            b.alloc_array(&items, root),
        ))),
    );
    b.evaluate_node_deep(holder, None);

    let elsewhere = Arc::new(RwLock::new(Registry::new()));
    let _ = elsewhere
        .write()
        .unwrap()
        .freeze_mapped(&b, ModuleKey::from_raw(1), [0; 32]);
}

#[test]
fn static_apply_keeps_foreign_items_in_place() {
    // B's function template f(x) = [x, A-item]: the return's array holds a
    // foreign (module A) static item alongside the parameter.  Applying f
    // from an importer re-points the parameter item at its clone but must
    // keep the foreign item in place — a foreign local index may never be
    // looked up in B's node table (A's array sits at local index 62, far
    // past B's node count, so the unguarded lookup would panic).
    let registry = Arc::new(RwLock::new(Registry::new()));
    let mut a = Registry::new_module(&registry);
    let ablock = a.add_block(None);
    for i in 0..60 {
        u128_node(&mut a, ablock, i);
    }
    let n10 = u128_node(&mut a, ablock, 10);
    let n20 = u128_node(&mut a, ablock, 20);
    let arr = array_node(&mut a, ablock, &[n10, n20], None);
    a.evaluate_node_deep(arr, None);
    let freeze_a = registry
        .write()
        .unwrap()
        .freeze_mapped(&a, ModuleKey::from_raw(1), [0; 32]);
    let arr_sref = StaticNodeId {
        module: freeze_a.key,
        index: freeze_a.node_map[&arr],
    };

    let mut b = Registry::new_module(&registry);
    let body = b.add_block(None);
    let param = b.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let ret = b.add_node(body, None, None);
    b.write_node_value(
        ret,
        Some(TestValue::LowValue(LowValue::Array(b.alloc_array(
            &[item(param), ArrayItem::new(AnyNodeId::Static(arr_sref))],
            body,
        )))),
    );
    let (func_node, _) = wrap_function(&mut b, body, ret, param);
    b.evaluate_node_deep(ret, None);
    let freeze_b = registry
        .write()
        .unwrap()
        .freeze_mapped(&b, ModuleKey::from_raw(2), [0; 32]);

    let mut imp = Registry::new_module(&registry);
    let root = imp.add_block(None);
    let f = static_func_value(&mut imp, root, freeze_b.key, 0);
    let arg = u128_node(&mut imp, root, 5);
    let call = call_node(&mut imp, root, f, arg);
    let value = imp.evaluate_node_deep(call, None);
    let items = raw_items(value);
    assert_eq!(u128_of(imp.evaluate_node(items[0].node, None)), 5);
    assert_eq!(
        items[1].node,
        AnyNodeId::Static(arr_sref),
        "the foreign item stays in place through the apply"
    );
    let inner = raw_items(imp.evaluate_node(items[1].node, None));
    assert_eq!(u128_of(imp.evaluate_node(inner[0].node, None)), 10);
    assert_eq!(u128_of(imp.evaluate_node(inner[1].node, None)), 20);
    let _ = func_node;
}
