//! What a `plrun` chain looks like to a graph lowering.  See
//! `docs/notes/compute-graph-jit.md`.

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

/// Compile the way a host does — imports through a `PackageStore` — and return
/// the **module**, not a rendered value.
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
    let value = module.evaluate_node_deep(root, None).unwrap();
    match value.as_enum() {
        Some(LowValue::Function(function)) => function,
        other => panic!("expected the program to evaluate to a function, got {other:?}"),
    }
}

/// The dynamic [`FunctionId`] behind a function value; a language function is
/// never static.
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
/// # Invariant
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
/// # Invariant
///
/// The callee slot holds a `value_of` extraction whose **own cached value is
/// already the function** — `Index(pair, 0)` evaluates to the pair's value, so
/// following the `Index` further would land on the pair rather than on the
/// function.
fn callee_of(module: &Module<LangProgram>, apply: NodeId) -> Option<AnyFunctionId> {
    let operation = module.node_operation(apply)?;
    let operand = items(module, operation.operand?).into_iter().next()?;
    match module.node_value(AnyNodeId::Dynamic(operand))?.as_enum() {
        Some(LowValue::Function(function)) => Some(function),
        _ => None,
    }
}

/// The one `ParLaunch` reachable from a function's body. This is the whole of
/// "find the dispatches".
fn the_dispatch(module: &Module<LangProgram>, fid: AnyFunctionId) -> Option<NodeId> {
    applies(module, fid).into_iter().find_map(|apply| {
        let callee = callee_of(module, apply)?;
        compute_nodes(module, callee)
            .into_iter()
            .find(|(_, op)| *op == ComputeOperator::ParLaunch)
            .map(|(node, _)| node)
    })
}

/// A `plrun` chain in a function that closes over its kernels and its data — the
/// shape a graph is compiled from.
///
/// # Invariant
///
/// `x => body` is the only lambda there is, and the resolver gives it exactly
/// one binder, so a "free variable" is just a name the body reads that is not
/// that parameter. That is what makes `s` a parameter and `doubler`/`data` free
/// ones.
const CHAIN: &str = r#"---
  compute = import "compute.lichen"
---
InA  = struct<.a Int>
OutA = struct<.z (compute.Buf _)>
ParA = compute.P (compute.KT _)(.I InA, .O OutA)
adder = compute.parallel ((k : ParA) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 3))
}) "Cpu"
InB  = struct<.b (compute.Buf _)>
OutB = struct<.w (compute.Buf _)>
ParB = compute.P (compute.KT _)(.I InB, .O OutB)
doubler = compute.parallel ((k : ParB) => {
  i = compute.range k.n
  j = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value j + j))
}) "Cpu"
data = (compute.plrun adder ((compute.A InA)(.n 4, .I InA(.a 0))) : OutA)
InS  = struct<.b (compute.Buf _)>
OutS = struct<.unused (compute.Buf _)>
ParS = compute.P (compute.KT _)(.I InS, .O OutS)
step = (s : ParS) => {
  out = (compute.plrun doubler ((compute.A InB)(.n 4, .I InB(.b data.z))) : OutB)
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

    // The operand array is [kernel, cfg], readable without evaluating it — which
    // keeps a build from dispatching.
    let operation = module.node_operation(dispatch).expect("a dispatch node");
    let operands = items(&module, operation.operand.expect("operands"));
    assert_eq!(operands.len(), 2, "[kernel, cfg]");

    // A value-level read into the cfg gets **nowhere**: the slot is an
    // unevaluated node, so `array_items` finds nothing.

    // `ParLaunch`'s `run` sees values there only because the VM
    // evaluated them; borrowing that step would run the dispatch.
    assert!(
        module.node_value(AnyNodeId::Dynamic(operands[1])).is_none()
            || unsafe { module.array_items(operands[1]) }.is_none(),
        "the cfg slot is a node rather than a read array, so the count and the buffers \
         have to be reached through its operation instead of its payload"
    );
}

/// Follow a `value_of` extraction — `Index(x, i)` — to `x`, as the
/// checker reaches a pair's or a struct field's value.
fn through_index(module: &Module<LangProgram>, node: NodeId) -> NodeId {
    let operation = module.node_operation(node).expect("an Index node");
    assert_eq!(
        format!("{:?}", operation.operator),
        "LowOperator(Index)",
        "this is the extraction the shape is about"
    );
    items(module, operation.operand.expect("an operand"))[0]
}

/// A graph function whose dispatch reads **both its count and its buffer out of
/// its own parameter**.
///
/// # Invariant
///
/// The parameter is the named struct, so the argument the dispatch is handed is
/// that struct built from the parameter's own fields — there is no literal and
/// no captured value anywhere in it. That is what makes the leaves of the
/// argument undecided cells.
const FROM_PARAMETER: &str = r#"---
  compute = import "compute.lichen"
---
InA  = struct<.a Int>
OutA = struct<.z (compute.Buf _)>
ParA = compute.P (compute.KT _)(.I InA, .O OutA)
adder = compute.parallel ((k : ParA) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 3))
}) "Cpu"
InB  = struct<.b (compute.Buf _)>
OutB = struct<.w (compute.Buf _)>
ParB = compute.P (compute.KT _)(.I InB, .O OutB)
step = (s : ParB) => {
  out = (compute.plrun adder ((compute.A InA)(.n s.n, .I InA(.a 0))) : OutA)
  compute.plrun adder ((compute.A InA)(.n s.n, .I InA(.a out.z)))
}
step
"#;

/// The nodes a value is made of at its leaves: a nested array is descended, and
/// anything else is a leaf.
///
/// # Invariant
///
/// A struct value is an array ([`lichen_highlevel::shape`]), so this is how a
/// test reaches the cells an argument is built from rather than the argument
/// itself.
fn leaves_of(module: &Module<LangProgram>, node: NodeId) -> Vec<NodeId> {
    // SAFETY: `node` is a live node of `module`, whose blocks are all alive —
    // nothing has been dropped.
    match unsafe { module.array_items(node) } {
        Some(items) => items
            .iter()
            .flat_map(|item| leaves_of(module, dyn_node(item.node)))
            .collect(),
        None => vec![node],
    }
}

/// **Nothing in a template's body is decided, and that decides how a graph has
/// to be built.**
///
/// # Invariant
///
/// The argument's **leaves** — the count and the buffer the body reads from its
/// parameter — are undecided cells until the function is applied, so a graph is
/// built by **applying** it and recording what it dispatches, never by reading
/// the template (`ValueId`'s note).
///
/// Applying is equivalent to not applying here: the function has one parameter
/// and this one reads it.
#[test]
fn a_templates_cfg_is_readable_but_nothing_in_it_is_decided_until_it_is_applied() {
    let (mut module, root) = run(FROM_PARAMETER);
    let function = function_of(&mut module, root);
    let dispatch = the_dispatch(&module, function).expect("the body dispatches");
    let operands = items(
        &module,
        module.node_operation(dispatch).unwrap().operand.unwrap(),
    );

    // The argument reaches the dispatch through an `Index`; following
    // it is the whole lookup, the walk `value_of_node` does.
    let cfg = through_index(&module, operands[1]);
    let cfg_items = items(&module, cfg);
    assert_eq!(
        cfg_items.len(),
        2,
        "the argument is the parameter's own structure: the extent and the input group"
    );
    let leaves = leaves_of(&module, cfg);
    assert!(
        !leaves.is_empty(),
        "the argument names the cells a graph has to bind"
    );
    for &cell in &leaves {
        assert_eq!(
            module
                .node_value(AnyNodeId::Dynamic(cell))
                .and_then(|v| AsEnum::<LowValue>::as_enum(&v)),
            None,
            "the leaf {cell:?} is an undecided cell: the count and the buffer in the body are \
             decided by applying the function, not before it"
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

    // `k.native` — an `Index` into the captured kernel struct;
    // neither it nor the field index holds a value until applied.
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

/// A graph function that reads its parameter **back to front**, then runs —
/// the only way to tell position from order.
const BACK_TO_FRONT: &str = r#"---
  compute = import "compute.lichen"
---
InA  = struct<.a Int>
OutA = struct<.z (compute.Buf _)>
ParA = compute.P (compute.KT _)(.I InA, .O OutA)
adder = compute.parallel ((k : ParA) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 3))
}) "cpu"
InB  = struct<.b (compute.Buf _)>
OutB = struct<.w (compute.Buf _)>
ParB = compute.P (compute.KT _)(.I InB, .O OutB)
doubler = compute.parallel ((k : ParB) => {
  i = compute.range k.n
  j = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value j + j))
}) "cpu"
data = (compute.plrun adder ((compute.A InA)(.n 4, .I InA(.a 0))) : OutA)
step = (s : ParB) => {
  buffer = s.in.b
  count = s.n
  out = (compute.plrun doubler ((compute.A InB)(.n count, .I InB(.b buffer))) : OutB)
  out.w.native
}
step ((ParB)(.n 4, .in InB(.b data.z), .out OutB(.w data.z)))
"#;

/// The `i64`s behind a buffer value.
///
/// # Invariant
///
/// The payload is the class's packed elements — eight bytes each for an `Int`
/// buffer ([`lichen_kernel_ir::ScalarClass::byte_width`]) — so the words are
/// decoded rather than viewed: a payload is bytes, and a `&[i64]` view of it
/// would be the old word-per-element layout the ABI no longer has.
fn buffer_data(handle: &lichen_lowlevel::AnyHandle<[u8]>) -> Vec<i64> {
    // SAFETY: the handle is the value's own arena payload and its home block is
    // the live one the evaluation just ran in.
    let bytes = unsafe { std::slice::from_raw_parts(handle.as_ptr(), handle.len()) };
    bytes
        .chunks_exact(std::mem::size_of::<i64>())
        .map(|word| i64::from_le_bytes(word.try_into().unwrap_or_default()))
        .collect()
}

/// **`ins(i)` is the `i`-th argument, and only applying the function says so.**
///
/// # Invariant
///
/// A body read of `ins(i)` compiles to a bare cell — no operation, no subscript,
/// not even a member of the parameter's class — so **nothing in the unapplied
/// body links a read to a slot**, and the arity is unstated.
///
/// Which read is which slot is settled by the apply: read order would swap
/// `step`'s arguments, so `(4, data)` would read the **number** as a
/// buffer — silently wrong.
#[test]
fn a_parameter_is_bound_by_the_position_the_source_names_and_not_by_read_order() {
    let (mut module, root) = run(BACK_TO_FRONT);

    let value = module.evaluate_node_deep(root, None).unwrap();
    let Some(ComputeValue::Buffer(handle, _)) = AsEnum::<ComputeValue>::as_enum(&value) else {
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
