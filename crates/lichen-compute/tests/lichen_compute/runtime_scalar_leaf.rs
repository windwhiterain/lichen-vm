//! A normalise whose `eps` is a parameter: the shape of `examples/layernorm.lichen`.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::{
    GraphPass, LaunchExtent, clear_graph_pass, consult_graph_pass, graph, set_graph_pass,
    take_graph_pass_lines,
};
use lichen_compute::fusion_claim::{
    Instance, RegionSchema, claim_of, derive_regions, fragment_of, kernel_id, region_positions,
};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::region_merge::{install_region_merge, region_merge_pass};
use lichen_graph_ir::{Graph, Node};
use lichen_kernel_ir::{
    ComposeRefusal, KernelFragment, KernelInstr, KernelShape, Producer, ScalarClass, ValueDef,
    compose_fan_in,
};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The row the diamond normalises: `x = 1 … 4` and `w = 3, 6, 3, 6`.
const ROW: [f32; 4] = [1.0, 2.0, 3.0, 4.0];
const WEIGHT: [f32; 4] = [3.0, 6.0, 3.0, 6.0];

/// The launch's `eps`, the runtime scalar the normalise reads beside its extent.
const EPS: f32 = 1.0;

/// The literal the counterfactual's element carries, and where it is moved to.
///
/// # Invariant
/// **Neither is a constant the `sqrt` expansion holds.** The merged body carries that expansion,
/// and its seed and step constants are in the element's own cone: a bias the Newton sequence
/// already spells would be two constants, not one.
const BIAS: f32 = 7.0;
const MOVED_BIAS: f32 = 10.0;

/// The three kernels the recording names, in region order.
const KERNELS: usize = 3;

/// The normalise's absorbed inputs: the sum at `.s` and the sum of squares at `.q`.
const SUM: usize = 2;
const SUMSQ: usize = 3;

/// The one-pass LayerNorm diamond, with `eps` a parameter and the element carrying `bias`.
///
/// # Invariant
/// **The committed example's shape, with one literal added**: `bias` appears in the normalise's
/// element alone, so the counterfactual has exactly one constant to move and no recording can
/// answer the moved value. Everything else is `examples/layernorm.lichen`'s: two independent
/// producers and one normalise that reads both, with `eps` read as `k.eps`.
fn program(step: &str, bias: Option<f32>) -> String {
    let term = match bias {
        Some(bias) => format!("(((x - m) / d * w + {bias:.1}) ~ \"element\")"),
        None => "(((x - m) / d * w) ~ \"element\")".to_string(),
    };
    format!(
        r#"
---
  compute = import "compute.lichen"
---
Row   = struct{{.x (compute.Buf ((compute.BufParam _)(.type Float, .length {count})))}}
Sums  = struct{{.s (compute.Buf ((compute.BufParam _)(.type Float, .length {count})))}}
ParS  = struct{{.n Int, .in Row}}
row_sum_raw = (k : ParS) => {{
  i = compute.range k.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float))) ~ "combine"))
  ((Sums)(.s (compute.write ((compute.Write _)(.to y, .at i, .value (fold (k.n, 0.0 ~ "identity")))))))
}}
row_sum = compute.parallel (row_sum_raw ~ "claim:Reduction(combine,identity)") "gpu" Sums
Sumsq = struct{{.q (compute.Buf ((compute.BufParam _)(.type Float, .length {count})))}}
ParQ  = struct{{.n Int, .in Row}}
row_sumsq_raw = (k : ParQ) => {{
  i = compute.range k.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float))) ~ "combine" ~ "element"))
  ((Sumsq)(.q (compute.write ((compute.Write _)(.to y, .at i, .value (fold (k.n, 0.0 ~ "identity")))))))
}}
row_sumsq = compute.parallel (row_sumsq_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Sumsq
Moments = struct{{.x (compute.Buf ((compute.BufParam _)(.type Float, .length {count}))), .w (compute.Buf ((compute.BufParam _)(.type Float, .length {count}))), .s (compute.Buf ((compute.BufParam _)(.type Float, .length {count}))), .q (compute.Buf ((compute.BufParam _)(.type Float, .length {count})))}}
OutY    = struct{{.y (compute.Buf ((compute.BufParam _)(.type Float, .length {count})))}}
ParY    = struct{{.n Int, .eps Float, .in Moments}}
normalise_raw = (k : ParY) => {{
  i = compute.range k.n
  count = int2float k.n
  m = (compute.read ((compute.Read _)(.from k.in.s, .at 0)) : Float) / count
  v = (compute.read ((compute.Read _)(.from k.in.q, .at 0)) : Float) / count - m * m
  d = sqrt (v + k.eps)
  x = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  w = (compute.read ((compute.Read _)(.from k.in.w, .at i)) : Float)
  y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutY)(.y (compute.write ((compute.Write _)(.to y, .at i, .value {term})))))
}}
normalise = compute.parallel (normalise_raw ~ "claim:Reduction(element)") "gpu" OutY
All   = struct{{.x (compute.Buf ((compute.BufParam _)(.type Float, .length {count}))), .w (compute.Buf ((compute.BufParam _)(.type Float, .length {count})))}}
GArg  = struct{{.n Int, .eps Float, .in All}}
ArgY  = struct{{.n Int, .eps Float, .I Moments}}
{step}
built = compute.graph step
"#,
        count = ROW.len(),
    )
}

/// The step a recording runs, over `eps` as the parameter's own scalar leaf.
///
/// # Invariant
/// **`.n` and `.eps` are read off the graph's own argument**, so the recording carries `eps` as an
/// edge of its own rather than as the literal it was written with — which is what a runtime scalar
/// is: a number the launch supplies beside its extent.
fn step() -> &'static str {
    r#"step = (s : GArg) => {
  a = (compute.plrun row_sum ((compute.A Row)(.n s.n, .I Row(.x s.in.x))) : Sums)
  b = (compute.plrun row_sumsq ((compute.A Row)(.n s.n, .I Row(.x s.in.x))) : Sumsq)
  compute.plrun normalise ((ArgY)(.n s.n, .eps s.eps, .I Moments(.x s.in.x, .w s.in.w, .s a.s, .q b.q))) : OutY
}"#
}

/// `program` as a recording: the three kernels and the graph they recorded, nothing run.
///
/// # Invariant
/// Each body declares its buffer with `compute.buf` and returns the write as the codomain
/// field's value (`docs/notes/kernel-buffer-declarations.md`).
fn recording(bias: Option<f32>) -> String {
    format!(
        "{}(row_sum, row_sumsq, normalise, built)\n",
        program(step(), bias)
    )
}

/// The two fill kernels that build the launch's data, one per buffer.
///
/// # Invariant
/// One lane per element and each lane writes the element at its own index, so each buffer holds
/// the list the old `compute.buffer` call held: a host builds its data with a kernel, because
/// there is no array-to-buffer conversion any more (`docs/notes/kernel-buffer-declarations.md`).
fn filled() -> String {
    format!(
        r#"
Fill = struct{{.n Int}}
Weights = struct{{.w (compute.Buf ((compute.BufParam _)(.type Float, .length {count})))}}
row_raw = (c : Fill) => {{
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length {count}))
  ((Row)(.x (compute.write ((compute.Write _)(.to y, .at i, .value {row})))))
}}
weight_raw = (c : Fill) => {{
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length {count}))
  ((Weights)(.w (compute.write ((compute.Write _)(.to y, .at i, .value {weight})))))
}}
row_filler = compute.parallel row_raw "cpu" Row
weight_filler = compute.parallel weight_raw "cpu" Weights
"#,
        row = table(&ROW),
        weight = table(&WEIGHT),
        count = ROW.len(),
    )
}

/// The elements of `row`, as the sum of one-level `if`s the fill kernel writes.
///
/// # Invariant
/// One arm per element and each arm names its own lane, so the sum is the list the old array
/// literal held and not a formula of it: a nested `if` chain is refused
/// (`docs/notes/kernel-buffer-declarations.md`).
fn table(row: &[f32]) -> String {
    row.iter()
        .enumerate()
        .map(|(position, value)| format!("(if i == {position} then {value:.1} else 0.0)"))
        .collect::<Vec<String>>()
        .join(" + ")
}

/// `recording`, run over the row through `graphrun`.
fn running(bias: Option<f32>) -> String {
    format!(
        "{recording}\n{fill}row = (compute.plrun row_filler ((Fill)(.n {elements})) : Row)\nweight = \
         (compute.plrun weight_filler ((Fill)(.n {elements})) : Weights)\nout = compute.collect \
         (compute.graphrun built (4, {eps:.1}, row.x, weight.w))\nout\n",
        recording = recording(bias),
        fill = filled(),
        elements = ROW.len(),
        eps = EPS,
    )
}

/// The program's answer: the `Float` elements the running program's root holds.
fn answer(module: &Module<LangProgram>, value: &LangValue) -> Vec<f32> {
    common::float_array(module, value)
}

/// The root tuple's items, read from the value the program answered.
///
/// # Invariant
/// The module's root node is not read back: a program whose root is a value has one node for its
/// value and its type, and `common::run` evaluates the type after the value.
fn root_items(value: &LangValue) -> &[ArrayItem] {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("the program's root is a tuple");
    };
    // SAFETY: `array` is the payload of the root value just produced, whose block is alive.
    unsafe { array.items() }
}

/// The claimed instances the recording's root tuple names, in region order.
fn instances(module: &Module<LangProgram>, value: &LangValue) -> Vec<Instance> {
    root_items(value)
        .iter()
        .take(KERNELS)
        .map(|item| {
            let AnyNodeId::Dynamic(node) = item.node else {
                panic!("a kernel comes from a frozen artifact");
            };
            let kernel = kernel_id(module, node).expect("the item names a kernel");
            Instance {
                kernel,
                claim: claim_of(kernel).expect("the kernel claims an interface"),
                fragment: fragment_of(kernel).expect("the registry holds the dispatched fragment"),
            }
        })
        .collect()
}

/// The graph the recording's last root element names.
fn recorded(module: &Module<LangProgram>, value: &LangValue) -> Graph {
    let item = root_items(value)
        .get(KERNELS)
        .expect("the root holds a graph");
    let AnyNodeId::Dynamic(node) = item.node else {
        panic!("the graph comes from a frozen artifact");
    };
    let value = module
        .element_value_node(AnyNodeId::Dynamic(node))
        .or(Some(AnyNodeId::Dynamic(node)))
        .and_then(|node| module.node_value(node))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value));
    let Some(ComputeValue::Graph(id, _)) = value else {
        panic!("the program's last root element is not a graph");
    };
    graph::lookup(id).expect("the recorded graph is registered")
}

/// The one schema the diamond derives, or the refusal.
fn schema(instances: &[Instance], graph: &Graph) -> Result<RegionSchema, String> {
    let mut regions = derive_regions(graph, instances);
    assert_eq!(regions.len(), 1, "the diamond is one region");
    regions
        .pop()
        .expect("one region")
        .map_err(|refusal| format!("{}: {}", refusal.name(), refusal.message()))
}

/// The lines the installed pass reported for a module, through the run report's own channel.
fn pass_report(module: &Module<LangProgram>) -> Vec<String> {
    module
        .extension_diagnostics
        .iter()
        .filter(|entry| entry.category.contains("pass"))
        .map(|entry| entry.message.clone())
        .collect()
}

/// The merge with the normalise's element bias moved, as one pass that wraps the merge.
///
/// # Invariant
/// The mutation is inside the composed body alone, so an answer that moves says the composed node
/// executed and an answer that does not says the recording did.
struct MovedBias(Box<dyn GraphPass>);

impl GraphPass for MovedBias {
    fn apply(
        &self,
        graph: &Graph,
        extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        let rewritten = self.0.apply(graph, extent, lines)?;
        let mut mutated = rewritten.clone();
        let Node::Kernel(node) = &rewritten.nodes()[0];
        let mut fragment = node.fragment.clone();
        move_bias(&mut fragment);
        mutated
            .replace_kernel(0, fragment)
            .expect("a moved constant is the same shape");
        Some(mutated)
    }
}

/// Move the composed body's **one** `BIAS` constant, read inside the merged `element`, into
/// `MOVED_BIAS`.
///
/// # Invariant
/// The search is the mark's own cone, so the `sqrt` expansion's seed and the fold's `0.0` — both
/// constants of the composed body — are not in scope: `BIAS` is in the normalise's element alone,
/// and no other kernel's arithmetic can answer the moved value.
fn move_bias(fragment: &mut KernelFragment) {
    let held = BIAS.to_bits() as i64;
    let moved = MOVED_BIAS.to_bits() as i64;
    let element = fragment
        .marks
        .iter()
        .find(|mark| mark.payload.iter().any(|field| field.text == "element"))
        .expect("the merged node keeps the survivor's element mark")
        .value;
    let mut cone = vec![element];
    let mut seen: Vec<usize> = vec![element.0 as usize];
    let mut biases: Vec<usize> = Vec::new();
    while let Some(value) = cone.pop() {
        match &fragment.body.values[value.0 as usize] {
            ValueDef::Instr {
                op: KernelInstr::Const(ScalarClass::Float, bits),
                ..
            } if *bits == held => biases.push(value.0 as usize),
            ValueDef::Instr { args, .. } => {
                for argument in args {
                    if seen.contains(&(argument.0 as usize)) {
                        continue;
                    }
                    seen.push(argument.0 as usize);
                    cone.push(*argument);
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        biases.len(),
        1,
        "the element holds the normalise's bias alone"
    );
    let ValueDef::Instr {
        op: KernelInstr::Const(ScalarClass::Float, bits),
        ..
    } = &mut fragment.body.values[biases[0]]
    else {
        panic!("the held definition is the constant it was found as");
    };
    *bits = moved;
}

/// `examples/layernorm.lichen`'s own shape derives, names `eps` as a third free value, and merges.
///
/// # Invariant
/// **The element names `s0` beside `n` and both producers' slots**, and the merged node's domain is
/// the consumer's own — the splice is inside the merged fragment, and its `eps` leaf is the one
/// the survivor was launched with.
#[test]
fn a_normalise_reading_a_runtime_scalar_merges_into_one_node() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&recording(None));
    let instances = instances(&module, &value);
    let graph = recorded(&module, &value);
    assert_eq!(region_positions(&graph, &instances), vec![vec![0, 1, 2]]);
    assert_eq!(
        instances[2].fragment.param_shape,
        KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Float),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        "the normalise declares one runtime scalar beside its extent"
    );
    assert_eq!(
        instances[0].fragment.param_shape,
        KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        "and each producer declares none, which is what the splice is sound for"
    );

    let schema =
        schema(&instances, &graph).expect("a runtime scalar is one function of the launch");
    let variables: Vec<String> = schema.nodes[2]
        .projection("element")
        .expect("the normalise declares an element")
        .variables()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        variables == vec!["d", "n", "r", "r1", "s0"],
        "the element names the datum, the launch's extent, both producers' slots, and the runtime \
         scalar: {variables:?}"
    );
    assert!(
        schema.canonical_form().contains("s0"),
        "and the region's own canonical form carries the name a pack is keyed on"
    );

    clear_graph_pass();
    install_region_merge(&module);
    let rewritten = consult_graph_pass(&graph, &|_node| Some(4));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    let rewritten = rewritten.expect("the floor's own proof licenses a proved fan-in");
    assert_eq!(rewritten.len(), 1, "the diamond becomes one node");
    assert_eq!(
        lines,
        vec![format!(
            "region 0: nodes 0, 1, 2 merged into one node at 2 (the survivor), absorbing input \
             position {SUM} of node 2 and input position {SUMSQ} of node 2, licensed by the floor: \
             every node of the region claims Reduction and nothing else; the merged fragment is registered with the conservative claim `Opaque`: a pass reading the registry names it and refuses it by name, and no reader reads algebra from it; the merged node's identity is digest:a8d4f7f652dba1ef"
        )],
        "the pass's own line: one region, three nodes, both arrivals absorbed"
    );
    let Node::Kernel(node) = &rewritten.nodes()[0];
    assert_eq!(
        node.fragment.param_shape, instances[2].fragment.param_shape,
        "the merged domain is the consumer's own, the runtime scalar spliced in"
    );
    assert_eq!(
        node.fragment.runtime_scalars(),
        1,
        "and the merged body reads the launch's scalar beside its extent"
    );
    assert_eq!(
        node.fragment.body.parameters().len(),
        3,
        "one entry-block parameter per leaf: the extent, `eps`, and the index"
    );
    assert_eq!(
        node.fragment,
        compose_fan_in(&[
            lichen_kernel_ir::Producer {
                fragment: &instances[0].fragment,
                absorbs: Vec::new(),
            },
            lichen_kernel_ir::Producer {
                fragment: &instances[1].fragment,
                absorbs: Vec::new(),
            },
            lichen_kernel_ir::Producer {
                fragment: &instances[2].fragment,
                absorbs: vec![(0, SUM), (1, SUMSQ)],
            },
        ])
        .expect("the recorded diamond composes"),
        "the surviving node holds `compose_fan_in` of the region's own bodies, element for element"
    );
}

/// On a device the merged diamond answers the un-passed program, and a moved bias moves it.
///
/// # Invariant
/// **The counterfactual is the proof**: a fused node computes the same numbers unfused, so only an
/// answer that moves with the composed body says which node the device dispatched. The bias is in
/// the normalise's element alone, so no recording can answer the moved value.
#[test]
fn the_merged_diamond_answers_the_body_and_a_moved_bias_moves_it() {
    let (stub, _guard) = common::stub();
    clear_graph_pass();
    let before = stub.calls().len();
    let (base_module, _base_value, _root) = common::run(&running(None));
    let recorded = stub.calls()[before..].to_vec();
    assert!(
        !recorded.is_empty(),
        "the program dispatches its three kernels"
    );

    install_region_merge(&base_module);
    let before = stub.calls().len();
    let (merged_module, _merged_value, _root) = common::run(&running(None));
    let merged = stub.calls()[before..].to_vec();
    let report = pass_report(&merged_module);
    println!(
        "the device ran, and the pass said:\n  {}",
        report.join("\n  ")
    );
    println!(
        "recorded {} dispatch(es), merged {} dispatch(es), and the merged node's own domain {:?}",
        recorded.len(),
        merged.len(),
        {
            let (module, value, _root) = common::run(&recording(None));
            let instances = instances(&module, &value);
            instances[2].fragment.param_shape.clone()
        }
    );
    assert!(
        report.iter().any(
            |line| line.contains("nodes 0, 1, 2 merged into one node at 2")
                && line.contains("licensed by the floor")
        ),
        "the run's own report says the pass merged while the run was dispatched: {report:?}"
    );
    // The merged leg's last dispatch is a node the recording did not: the composed body.
    let composed = merged.last().expect("the merged leg dispatches");
    for call in &recorded {
        assert_ne!(
            composed.fragment, call.fragment,
            "the composed node is not one the recording dispatched"
        );
    }

    let before = stub.calls().len();
    let (bias_module, _bias_value, _root) = common::run(&running(Some(BIAS)));
    let with_bias = stub.calls()[before..].to_vec();
    set_graph_pass(Box::new(MovedBias(region_merge_pass(&bias_module))));
    let before = stub.calls().len();
    let (_moved_module, _moved_value, _root) = common::run(&running(Some(BIAS)));
    let moved = stub.calls()[before..].to_vec();
    clear_graph_pass();
    // The staged bias is part of the composed body.
    let moved_composed = moved.last().expect("the moved bias dispatches");
    let biased_composed = with_bias.last().expect("the biased run dispatches");
    assert_ne!(
        moved_composed.fragment, biased_composed.fragment,
        "the answer moved with the composed body, so the merged node is what the run dispatched"
    );
    println!(
        "with a {BIAS} bias the run dispatches {} and the moved bias {}",
        with_bias.len(),
        moved.len()
    );
}

/// A **producer** that declares a runtime scalar of its own is still refused by name.
///
/// # Invariant
/// The merged leaf carries the *consumer*''s number and nothing compares the two nodes'' values, so
/// a producer that read one would read the consumer''s: the merge is licensed for a producer that
/// declares none and for nothing else. **The shape is decided before a body is touched**, so
/// widening the head''s declared domain alone is the whole experiment.
#[test]
fn a_producer_declaring_a_runtime_scalar_is_refused_by_name() {
    let (module, value, _root) = common::run(&recording(None));
    let instances = instances(&module, &value);
    let mut head = instances[0].fragment.clone();
    head.param_shape = KernelShape::Tuple(vec![
        KernelShape::Scalar(ScalarClass::Int),
        KernelShape::Scalar(ScalarClass::Float),
        KernelShape::Scalar(ScalarClass::Int),
    ]);
    assert_eq!(
        head.runtime_scalars(),
        1,
        "the head''s declared domain now carries a runtime scalar of its own"
    );
    let refusal = compose_fan_in(&[
        Producer {
            fragment: &head,
            absorbs: Vec::new(),
        },
        Producer {
            fragment: &instances[1].fragment,
            absorbs: Vec::new(),
        },
        Producer {
            fragment: &instances[2].fragment,
            absorbs: vec![(0, SUM), (1, SUMSQ)],
        },
    ])
    .expect_err("a producer that read a runtime scalar would read the consumer''s");
    assert!(
        matches!(refusal, ComposeRefusal::ShapesDiffer { .. }),
        "and the refusal is the shape''s own: {refusal}"
    );
    assert!(
        refusal.to_string().contains(
            "are not the same shape and not one with runtime scalars \
             spliced in"
        ),
        "and it names the relation that would have been allowed: {refusal}"
    );
}
