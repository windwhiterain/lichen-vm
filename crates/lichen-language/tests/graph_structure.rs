//! What a `plrun` chain actually looks like to a lowering, before anything is
//! built on top of it.
//!
//! The graph lowering walks a function and reads dispatches out of it without
//! running any. Every fact it depends on was inferred from the shape
//! `ComputeOperator::ParLaunch`'s `run` branch is handed, which is the shape of
//! an *already evaluated* operand. This file is where those inferences are
//! checked against the raw node structure instead, because a lowering written
//! on inferred shapes is a lowering written on guesses — and the first guess
//! here was simply wrong.
//!
//! **A native call is an `Apply`, and the operator the native op built lives in
//! a synthesized per-call-site function that the `Apply` enters.** The source
//! function's own node list contains no compute operator at all. A lowering that
//! looked for `ParLaunch` in the body would find nothing and record an empty
//! graph from a program that dispatches.
//!
//! The rest of what a lowering needs, and what the tests below check:
//!
//! - an `Apply` names its callee as a `[callee, argument, _]` operand array, and
//!   the callee is reached through the same `Index(pair, 0)` extraction the
//!   checker uses;
//! - the callee's body holds the compute operator node, and *that* node's
//!   operand array is `[kernel, cfg]`, readable without evaluating it — which is
//!   what keeps a **build** from running a dispatch;
//! - the `cfg` reaches the dispatch through a `value_of`-style `Index`
//!   extraction, and the tuple behind it is readable with no evaluation at all.
//!
//! **But nothing in a template's body is decided**, and that is the fact the
//! lowering turns on: the count and the buffers in the `cfg` are
//! `Parameterized` until the function is applied. So a graph is built by
//! *applying* the function and recording what it dispatches, not by reading a
//! template. See
//! [`a_templates_cfg_is_readable_but_nothing_in_it_is_decided_until_it_is_applied`].
//!
//! **And a read of the parameter is a bare cell, while the parameter itself is a
//! tuple with one cell per read.** So a graph's *arity* is readable from the
//! unapplied function — `array_items(function.parameter).len()` — which is what
//! sizes the placeholder tuple a build binds the parameter to, and there is no
//! ceiling and nothing to trim. Which read is which slot is not visible
//! structurally at all; the apply settles it, and it settles it by the position
//! the source names rather than by the order the body reads. See
//! [`a_parameter_read_is_a_bare_cell_and_the_arity_is_the_tuples_length`] and
//! [`a_parameter_is_bound_by_the_position_the_source_names_and_not_by_read_order`].

use std::sync::Arc;

use lichen_language::package::PackageStore;
use lichen_language::preprocess::preprocess;
use lichen_language::program::LangProgram;
use lichen_language::{compile_with_imports_at, lex};
use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, FunctionId, LowOperator, LowValue, Module, NodeId,
};
use lichen_utils::extend::AsEnum;

use lichen_compute::{ComputeOperator, ComputeValue};

/// Compile a program the way a host does — imports resolved through a package
/// store, so `import "compute.lichen"` brings the compute plugin in — and hand
/// back the module rather than a rendered value, which is the point: a
/// lowering wants to look at nodes nothing has applied.
fn run(source: &str) -> (Module<LangProgram>, NodeId) {
    let mut store = PackageStore::<LangProgram>::new();
    let (preprocessed, diags) = preprocess(source, None, &mut store);
    assert!(diags.is_empty(), "expected imports to resolve: {diags:?}");
    let line_starts = lex::line_starts(source);
    let report = compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(Arc::clone(&store.registry())),
        preprocessed.code_base,
        &line_starts,
        lichen_highlevel::no_native_ops(),
    );
    assert!(
        report.ok(),
        "expected the program to check, got: {:?}",
        report.diagnostics
    );
    let build = report.build.unwrap();
    (build.module, build.root_val)
}

fn dyn_node(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => unreachable!("language graphs are dynamic"),
    }
}

/// The node ids of an array node's items.
fn items(module: &Module<LangProgram>, node: NodeId) -> Vec<NodeId> {
    // SAFETY: `node` is a live node of `module`, whose blocks are all alive —
    // nothing has been dropped.
    unsafe { module.array_items(node) }
        .expect("an array node")
        .iter()
        .map(|item| dyn_node(item.node))
        .collect()
}

/// The function the program evaluates to, without applying it.
fn function_of(module: &mut Module<LangProgram>, root: NodeId) -> AnyFunctionId {
    let value = module.evaluate_node_deep(root, None);
    match value.as_enum() {
        Some(LowValue::Function(function)) => function,
        other => panic!("expected the program to evaluate to a function, got {other:?}"),
    }
}

/// The dynamic [`FunctionId`] behind a function value. A language function is
/// never a static one, so a static ref here would mean the program is not the
/// shape these tests are about.
fn dynamic(fid: AnyFunctionId) -> FunctionId {
    match fid {
        AnyFunctionId::Dynamic(fid) => fid,
        AnyFunctionId::Static(_) => panic!("a language function is dynamic"),
    }
}

/// The `Apply` nodes in a function's body, in order.
fn applies(module: &Module<LangProgram>, fid: AnyFunctionId) -> Vec<NodeId> {
    module.functions[dynamic(fid)]
        .nodes
        .iter()
        .copied()
        .filter(|node| {
            module.node_operation(*node).is_some_and(|operation| {
                matches!(
                    AsEnum::<LowOperator>::as_enum(&operation.operator),
                    Some(LowOperator::Apply)
                )
            })
        })
        .collect()
}

/// The compute operator nodes held in a function's body.
///
/// A **static** callee has no body in this module — it lives in a frozen
/// package — so it contributes nothing and yields nothing. That is not a shape
/// to work around: a source body mixes both kinds of call, and a dispatch is
/// only ever the dynamic kind.
fn compute_nodes(
    module: &Module<LangProgram>,
    fid: AnyFunctionId,
) -> Vec<(NodeId, ComputeOperator)> {
    let AnyFunctionId::Dynamic(fid) = fid else {
        return Vec::new();
    };
    module.functions[fid]
        .nodes
        .iter()
        .copied()
        .filter_map(|node| {
            let operation = module.node_operation(node)?;
            let op = AsEnum::<ComputeOperator>::as_enum(&operation.operator)?;
            Some((node, op))
        })
        .collect()
}

/// The function an `Apply` node calls.
///
/// The callee slot holds a `value_of` extraction whose **own cached value is
/// already the function** — `Index(pair, 0)` evaluates to the pair's value, so
/// following the `Index` further would land on the pair rather than on the
/// function. Reading the node's value is the whole of it.
fn callee_of(module: &Module<LangProgram>, apply: NodeId) -> Option<AnyFunctionId> {
    let operation = module.node_operation(apply)?;
    let operand = items(module, operation.operand?).into_iter().next()?;
    match module.node_value(AnyNodeId::Dynamic(operand))?.as_enum() {
        Some(LowValue::Function(function)) => Some(function),
        _ => None,
    }
}

/// The one `ParLaunch` reachable from a function's body, through whatever
/// indirection it takes. This is the whole of "find the dispatches".
fn the_dispatch(module: &Module<LangProgram>, fid: AnyFunctionId) -> Option<NodeId> {
    applies(module, fid).into_iter().find_map(|apply| {
        let callee = callee_of(module, apply)?;
        compute_nodes(module, callee)
            .into_iter()
            .find(|(_, op)| *op == ComputeOperator::ParLaunch)
            .map(|(node, _)| node)
    })
}

/// A `plrun` chain in a function that closes over its data. This is the shape a
/// graph is compiled from.
///
/// **`x => body` is the only lambda there is** — the grammar's
/// `lambda := annotated ('=>' expr)?` puts a *name* on the left, so a lichen
/// function always has exactly one parameter and a "free variable" is simply a
/// name the body reads that is not that parameter. `unused` is therefore a
/// parameter rather than a free variable, which is what makes `data` one.
const CHAIN: &str = r#"---
  compute = import "compute.lichen"
---
adder = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 3]
}) "Cpu"
doubler = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  j = compute.read [cfg(1), i]
  compute.write [n, i, j + j]
}) "Cpu"
data = compute.plrun adder (4,)
step = unused => {
  out = compute.plrun doubler (4, data)
  out
}
step
"#;

#[test]
fn a_body_dispatches_through_an_apply_whose_callee_holds_the_operator() {
    let (mut module, root) = run(CHAIN);
    let function = function_of(&mut module, root);

    // The body itself holds no compute operator at all, and that is the finding.
    assert_eq!(
        compute_nodes(&module, function),
        Vec::new(),
        "the source function's own nodes carry no compute operator, so a lowering that \
         searched the body for one would record an empty graph from a program that \
         dispatches"
    );

    assert!(
        !applies(&module, function).is_empty(),
        "the body does call something, and what it calls through is an Apply"
    );

    let dispatch = the_dispatch(&module, function).expect("the body dispatches once");

    // The operator node's own operand array is [kernel, cfg], readable without
    // evaluating it — which is what keeps a build from dispatching.
    let operation = module.node_operation(dispatch).expect("a dispatch node");
    let operands = items(&module, operation.operand.expect("operands"));
    assert_eq!(operands.len(), 2, "[kernel, cfg]");

    // How far into the cfg a value-level read gets: **nowhere**. The cfg slot is
    // an unevaluated array-constructing node, so `array_items` — which reads a
    // node's cached payload — has nothing to read. `ParLaunch`'s `run` branch
    // sees `[kernel, cfg]` as *values* because the VM evaluated them first, and
    // that evaluation is the part a lowering cannot simply borrow: doing it
    // would run the dispatch.
    assert!(
        module.node_value(AnyNodeId::Dynamic(operands[1])).is_none()
            || unsafe { module.array_items(operands[1]) }.is_none(),
        "the cfg slot is a node rather than a read array, so the count and the buffers \
         have to be reached through its operation instead of its payload"
    );
}

/// Follow a `value_of` extraction — `Index(x, i)` — to `x`, which is how the
/// checker reaches through a pair or a struct field to the value itself.
fn through_index(module: &Module<LangProgram>, node: NodeId) -> NodeId {
    let operation = module.node_operation(node).expect("an Index node");
    assert_eq!(
        format!("{:?}", operation.operator),
        "LowOperator(Index)",
        "this is the extraction the shape is about"
    );
    items(module, operation.operand.expect("an operand"))[0]
}

/// **Nothing in a template's body is decided, and that is the fact that decides
/// how a graph has to be built.**
///
/// The cfg tuple is readable without any evaluation, and reading it is not
/// enough: its two elements — the count and the buffer — are `Parameterized`.
/// The `4` and the `data` in the body are unbound cells until the function is
/// applied, because nothing has applied it.
///
/// So a graph cannot be built by *reading* a template. It has to be built by
/// **applying** the function and recording what it dispatches, which is what
/// `ValueId`'s own note says ("the graph is built by recording an evaluation
/// that has already happened"). Walking the structure without evaluating gets
/// you the graph's shape and none of its values, and the values are the part a
/// run needs.
///
/// Applying is safe, and is equivalent to not applying, for the reason the
/// language gives for free: **every** lichen function has exactly one
/// parameter, and this one does not read it, so what is applied does not
/// matter. A body that dispatched its own parameter is the unrecordable case.
#[test]
fn a_templates_cfg_is_readable_but_nothing_in_it_is_decided_until_it_is_applied() {
    let (mut module, root) = run(CHAIN);
    let function = function_of(&mut module, root);
    let dispatch = the_dispatch(&module, function).expect("the body dispatches");
    let operands = items(
        &module,
        module.node_operation(dispatch).unwrap().operand.unwrap(),
    );

    // The cfg reaches the dispatch through an `Index` extraction, so following
    // it is the whole of the lookup — the same walk `value_of_node` does.
    let cfg = through_index(&module, operands[1]);
    let cfg_items = items(&module, cfg);
    assert_eq!(cfg_items.len(), 2, "(count, buffers), read off the value");
    for (position, &node) in cfg_items.iter().enumerate() {
        let value = module
            .node_value(AnyNodeId::Dynamic(node))
            .and_then(|v| AsEnum::<LowValue>::as_enum(&v));
        assert_eq!(
            value,
            Some(LowValue::Parameterized),
            "cfg[{position}] is {node:?}, and it is an unbound cell: the count and the buffer \
             in the body are decided by applying the function, not before it"
        );
    }
}

#[test]
fn the_kernel_slot_is_a_field_read_whose_target_is_the_captured_kernel_struct() {
    let (mut module, root) = run(CHAIN);
    let function = function_of(&mut module, root);
    let dispatch = the_dispatch(&module, function).expect("the body dispatches");
    let operands = items(
        &module,
        module.node_operation(dispatch).unwrap().operand.unwrap(),
    );

    // `k.native` — an `Index` into the captured kernel struct, with neither the
    // struct nor the field index carrying a value, for the same reason the cfg
    // does not. What settles it is the application, not the read.
    assert_eq!(
        format!(
            "{:?}",
            module.node_operation(operands[0]).map(|o| o.operator)
        ),
        "Some(LowOperator(Index))"
    );
    let target = through_index(&module, operands[0]);
    assert_eq!(
        module.node_value(AnyNodeId::Dynamic(target)),
        None,
        "and the captured struct it reads out of has no value yet either"
    );
}

/// The index a `value_of` extraction reads at, if it is a decided number.
fn index_position(module: &Module<LangProgram>, node: NodeId) -> Option<usize> {
    let operation = module.node_operation(node)?;
    let pair = items(module, operation.operand?);
    let subscript = *pair.get(1)?;
    match module.node_value(AnyNodeId::Dynamic(subscript))?.as_enum() {
        Some(LowValue::USize(index)) => Some(index),
        _ => None,
    }
}

/// A graph function: its dispatch reads both its count and its buffer out of
/// its own parameter, which is where a graph's inputs come from.
///
/// The buffer slot is a **tuple**, because that is the shape `ParLaunch` reads
/// (`cfg(1)` is a tuple of `Buffer` values) and therefore the shape a source
/// program has to write — `compute.plrun k (n, (buffer,))`, not
/// `compute.plrun k (n, buffer)`.
const FROM_PARAMETER: &str = r#"---
  compute = import "compute.lichen"
---
adder = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 3]
}) "Cpu"
step = ins => {
  out = compute.plrun adder (ins(0), (ins(1),))
  out
}
step
"#;

/// **A parameter read is a bare cell, and the arity is the parameter tuple's
/// length.** The second correction this probe forced, and it is what decides how
/// a graph is sized.
///
/// `ins(0)` and `ins(1)` do not compile to two extractions off the parameter.
/// The compiler resolves each read into a cell of its own, so the cfg that
/// reaches the dispatch has two elements that carry **no operation, no
/// subscript, and an unbound value** — there is no `ins(i)` whose `i` a walk
/// could read. The design had assumed there was, because the operand pair's
/// subscript is a decided `USize(0)`; that subscript belongs to the extraction
/// of the **cfg slot**, not to the body's read, and reading it as an input
/// position would conclude the graph takes one input and silently drop the rest.
///
/// **But the arity is still decidable before the apply**, and from the parameter
/// rather than from the body: the parameter cell is a tuple with one cell per
/// read, so its length is how many inputs a caller has to satisfy. That is what
/// sizes the placeholder tuple `$graph` binds the parameter to, and it is why
/// there is no ceiling, no trim, and no refusal for reading past one. The design
/// had a ceiling and a named refusal for it; the body does not need either.
///
/// Which read is which slot is *not* visible here — the cfg's cells are not the
/// parameter's cells, and are not even in their classes. That half is settled by
/// the apply, and is checked behaviourally in
/// [`a_parameter_is_bound_by_the_position_the_source_names_and_not_by_read_order`].
#[test]
fn a_parameter_read_is_a_bare_cell_and_the_arity_is_the_tuples_length() {
    let (mut module, root) = run(FROM_PARAMETER);
    let function = function_of(&mut module, root);
    let parameter = module.functions[dynamic(function)].parameter;

    let dispatch = the_dispatch(&module, function).expect("the body dispatches");
    let operands = items(
        &module,
        module.node_operation(dispatch).unwrap().operand.unwrap(),
    );
    let cfg = through_index(&module, operands[1]);

    // The cfg is a readable two-element array, and both of its elements are
    // **bare cells**: no operation, and an unbound value. There is no `ins(i)`
    // subscript anywhere in them.
    let cfg_items = items(&module, cfg);
    assert_eq!(cfg_items.len(), 2, "(count, buffers)");
    for (position, &cell) in cfg_items.iter().enumerate() {
        assert!(
            module.node_operation(cell).is_none(),
            "cfg[{position}] is {cell:?} and carries no operation, so it is a \
             cell the apply binds rather than an extraction that names a slot"
        );
        assert_eq!(
            module
                .node_value(AnyNodeId::Dynamic(cell))
                .and_then(|v| AsEnum::<LowValue>::as_enum(&v)),
            Some(LowValue::Parameterized),
            "cfg[{position}] is unbound until the function is applied"
        );
    }

    // The one subscript that *is* decided belongs to the extraction of the cfg
    // slot out of the operand pair, so it says nothing about `ins(i)`. Reading
    // it as an input position is the mistake this test is named for.
    assert_eq!(
        index_position(&module, operands[1]),
        Some(0),
        "and the cfg sits at position 0 of the operand pair, which is not an \
         input position"
    );

    // The parameter is a **tuple of cells, one per read of it**, and that is the
    // arity: it is readable with no evaluation at all, which is what sizes the
    // placeholder tuple `$graph` binds the parameter to. No ceiling and no trim.
    let slots = items(&module, parameter);
    assert_eq!(slots.len(), 2, "one cell per `ins(i)` the body reads");
    for (position, &cell) in slots.iter().enumerate() {
        assert!(
            module.node_operation(cell).is_none(),
            "slot {position} is {cell:?} and carries no operation, so it is a \
             cell the apply binds rather than a value"
        );
        assert_eq!(
            module
                .node_value(AnyNodeId::Dynamic(cell))
                .and_then(|v| AsEnum::<LowValue>::as_enum(&v)),
            Some(LowValue::Parameterized),
            "and slot {position} is unbound until the function is applied"
        );
    }
}

/// A graph function that reads its parameter **back to front**, and then runs,
/// which is the only way to tell position from order.
const BACK_TO_FRONT: &str = r#"---
  compute = import "compute.lichen"
---
adder = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 3]
}) "cpu"
doubler = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  j = compute.read [cfg(1)(0), i]
  compute.write [n, i, j + j]
}) "cpu"
data = compute.plrun adder (4,)
step = ins => {
  buffer = ins(1)
  count = ins(0)
  compute.plrun doubler (count, (buffer,))
}
step (4, data)
"#;

/// The `i64`s behind a buffer value.
fn buffer_data(handle: &lichen_lowlevel::AnyHandle<[i64]>) -> Vec<i64> {
    let pointer = match handle {
        lichen_lowlevel::AnyHandle::Dynamic(dynamic) => dynamic.as_ptr(),
        lichen_lowlevel::AnyHandle::Static(statics) => statics.as_ptr(),
    };
    let length = <*const [i64]>::len(pointer);
    // SAFETY: the handle is the value's own arena payload, its home block is the
    // live one the evaluation just ran in, and nothing has dropped it.
    unsafe { std::slice::from_raw_parts(pointer as *const i64, length) }.to_vec()
}

/// **`ins(i)` is the `i`-th argument, and the only way to know that is to apply
/// the function — the unapplied body does not say.**
///
/// The third correction this probe has forced, and the one that would have been
/// the worst to get wrong.
///
/// A body read of `ins(i)` compiles to a bare cell: no operation, no subscript,
/// and not even a member of the parameter's class, so **nothing in the
/// unapplied body links a read to a slot**. The parameter cell is a tuple of
/// cells, one per read, and its length is therefore the arity — that part *is*
/// readable — but which read is which slot is settled by the apply.
///
/// If it were settled by read order, `step` below would take its two arguments
/// swapped: a caller passing `(4, data)` would get a dispatch over four
/// elements reading the **number** as a buffer. That is a silently wrong answer
/// rather than a refusal, which is the only class of bug this repository cares
/// most about, so it is checked by running the program and reading the numbers
/// out of the buffer.
///
/// The numbers are derived, not copied: `data` is `i + 3` over `[0, 4)` and
/// `doubler` writes `j + j`, so the result is `[6, 8, 10, 12]`.
#[test]
fn a_parameter_is_bound_by_the_position_the_source_names_and_not_by_read_order() {
    let (mut module, root) = run(BACK_TO_FRONT);

    let value = module.evaluate_node_deep(root, None);
    let Some(ComputeValue::Buffer(handle)) = AsEnum::<ComputeValue>::as_enum(&value) else {
        panic!("the dispatch ran, so the root is a buffer, got {value:?}");
    };
    assert_eq!(
        buffer_data(&handle),
        vec![6, 8, 10, 12],
        "the count reached the count slot and the buffer reached the buffer slot, \
         so the cells are indexed by the position the source writes rather than by \
         when the body got round to reading them"
    );
}
