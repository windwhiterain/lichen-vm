//! A mark names the value its expression produced, whatever the spelling.
//! See docs/notes/graph-fusion.md §3.

mod common;

use lichen_compute::fusion_claim::{
    Instance, RegionSchema, derive_regions, fragment_of, kernel_id, marked_value,
};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_fusion::{BinaryOp, Expr, Reduction, UnaryOp, Variable, normalize};
use lichen_fusion_bridge::{ReadRoles, projection};
use lichen_graph_ir::Graph;
use lichen_lowlevel::{AnyNodeId, LowValue, Module};

/// The chain's producer: `e[i] = d[i] * 2.0`, marked the natural way.
///
/// # Invariant
/// The mark rides a **top-level routed operator**, which `compile.rs` lowers to a call of
/// the prelude's `mul`, and the consumer's rides a **marked application**. Neither spelling
/// is the one `elementwise_chain` uses, so neither is covered by it.
fn elementwise_natural() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   = struct{{.d (compute.Buf Float)}}
OutE = struct{{.e (compute.Buf Float)}}
In2  = struct{{.p (compute.Buf Float)}}
OutF = struct{{.f (compute.Buf Float)}}
ParE = compute.P (compute.KT Float)(.I In, .O OutE)
Par2 = compute.P (compute.KT Float)(.I In2, .O OutF)
scale_raw = (k : ParE) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) * 2.0) ~ "element")))
}}
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu"
rescale_raw = (k : Par2) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.f, .at i, .value ((exp (compute.read ((compute.Read _)(.from k.in.p, .at i)))) ~ "element")))
}}
rescale = compute.parallel (rescale_raw ~ "claim:Elementwise(element)") "gpu"
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun rescale ((compute.A In2)(.n s.n, .I In2(.p first.e)))
}}
built = compute.graph step
(scale, rescale, built)
"#
    )
}

/// The same producer, consumed by a fold whose `combine` is marked **inside a `@loop`**.
///
/// # Invariant
/// The producer is the natural spelling, so one recording pins both a top-level marked
/// operator and an in-loop mark against a single region.
fn reduction_in_loop() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
{SCALE}
In2  = struct{{.d (compute.Buf Float), .p (compute.Buf Float)}}
OutZ = struct{{.z (compute.Buf Float)}}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutZ)
consumer_raw = (k : Par2) => {{
  i = compute.range k.n
  r = compute.read ((compute.Read _)(.from k.in.p, .at 0))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (((compute.read ((compute.Read _)(.from k.in.d, .at i)) - r) ~ "element")) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, 0.0 ~ "identity")))
}}
consumer = compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.e)))
}}
built = compute.graph step
(scale, consumer, built)
"#
    )
}

/// The producer kernel both recordings share: `e[i] = d[i] * 2.0`, mark on the operator.
const SCALE: &str = r#"
In   = struct{.d (compute.Buf Float)}
OutE = struct{.e (compute.Buf Float)}
ParE = compute.P (compute.KT Float)(.I In, .O OutE)
scale_raw = (k : ParE) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) * 2.0) ~ "element")))
}
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu"
"#;

/// The root tuple's items, read from the value the program answered.
fn root_items(value: &LangValue) -> &[lichen_lowlevel::ArrayItem] {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("the program's root is a tuple");
    };
    // SAFETY: `array` is the payload of the root value just produced.
    unsafe { array.items() }
}

/// The two claimed kernels the recording's root tuple holds.
fn instances(module: &Module<LangProgram>, value: &LangValue) -> Vec<Instance> {
    root_items(value)
        .iter()
        .take(2)
        .map(|item| {
            let AnyNodeId::Dynamic(node) = item.node else {
                panic!("a kernel comes from a frozen artifact");
            };
            let kernel = kernel_id(module, node).expect("the item names a kernel");
            Instance {
                kernel,
                claim: lichen_compute::fusion_claim::claim_of(kernel)
                    .expect("the kernel claims an interface"),
                fragment: fragment_of(kernel).expect("the registry holds the dispatched fragment"),
            }
        })
        .collect()
}

/// The graph the recording's third root element names.
fn recorded(module: &Module<LangProgram>, value: &LangValue) -> Graph {
    let item = root_items(value)
        .get(2)
        .expect("the program's root holds a graph");
    let AnyNodeId::Dynamic(node) = item.node else {
        panic!("the graph comes from a frozen artifact");
    };
    let graph = module
        .node_value(AnyNodeId::Dynamic(node))
        .or_else(|| module.node_value(AnyNodeId::Dynamic(node)));
    let Some(lichen_compute::ComputeValue::Graph(id, _)) = graph
        .as_ref()
        .map(|value| lichen_utils::extend::AsEnum::<lichen_compute::ComputeValue>::as_enum(value))
        .flatten()
    else {
        panic!("the program's third root element is not a graph");
    };
    lichen_compute::compute::graph::lookup(id).expect("the recorded graph is registered")
}

/// The one schema `source`'s recording derives, with the instances behind it.
fn derived(source: &str) -> (Vec<Instance>, RegionSchema) {
    let (module, value, _root) = common::run(source);
    let instances = instances(&module, &value);
    let graph = recorded(&module, &value);
    let regions = derive_regions(&graph, &instances);
    assert_eq!(regions.len(), 1, "the recording is one claimed region");
    (
        instances,
        regions
            .into_iter()
            .next()
            .expect("the one region")
            .expect("an elementwise chain derives a schema"),
    )
}

/// A binary operator over two operands, in the order the logic wrote it.
fn binary(operator: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary(operator, Box::new(left), Box::new(right))
}

/// A projection's canonical form, so an assertion reads the tree and not an ordering.
fn canonical(expr: Expr) -> Expr {
    normalize(&expr).expect("a projection the size of these normalizes")
}

fn datum() -> Expr {
    Expr::Variable(Variable::DATUM)
}

fn producer() -> Expr {
    Expr::Variable(Variable::PRODUCER)
}

fn state() -> Expr {
    Expr::Variable(Variable::CONSUMER_STATE)
}

/// A marked top-level routed operator anchors to the **whole** expression, and a marked
/// application still does.
#[test]
fn a_mark_on_a_top_level_operator_names_its_whole_expression() {
    let (instances, schema) = derived(&elementwise_natural());
    assert_eq!(
        schema.nodes[0].canonical("element"),
        Some(&canonical(binary(
            BinaryOp::Mul,
            datum(),
            Expr::constant(2.0)
        ))),
        "the mark on `read * 2.0` names the product, not the read: `docs/notes/graph-fusion.md` §3 \
         measured the left operand here"
    );
    assert_eq!(
        schema.nodes[1].canonical("element"),
        Some(&canonical(Expr::Unary(UnaryOp::Exp, Box::new(producer())))),
        "a mark on an application still names the whole expression"
    );
    assert_eq!(
        schema.between[0].canonical,
        canonical(Expr::Unary(UnaryOp::Exp, Box::new(producer()))),
        "and the region records the consumer's element as the code between the two instances"
    );
    let form = schema.canonical_form();
    let printed: Vec<&str> = form.lines().collect();
    assert_eq!(
        printed[..5],
        [
            "region 2",
            "node 0 interface Elementwise",
            "role element (* 2.0 d)",
            "node 1 interface Elementwise",
            "role element (exp r)",
        ],
        "the canonical form is what a pack binds by, so the mark's value is pinned in it too"
    );
    assert_eq!(
        schema.nodes[0].elementwise().unwrap(),
        &canonical(binary(BinaryOp::Mul, Expr::constant(2.0), datum())),
        "and the elementwise reading is the element function the marked expression computes"
    );

    // The value the mark is published against, read through the public surface.
    let marked = marked_value(&instances[0].fragment, "Elementwise", "element")
        .expect("the producer's body marks `element`");
    let bridged = projection(&instances[0].fragment, marked, ReadRoles::DATUM_ONLY)
        .expect("the producer's element bridges over a datum");
    assert_eq!(
        normalize(&bridged).as_ref(),
        Some(&canonical(binary(
            BinaryOp::Mul,
            datum(),
            Expr::constant(2.0)
        ))),
        "the published value is the operator the body computed, read as the datum's product"
    );
}

/// A mark inside a `@loop` still names the whole expression, so a fold's algebra survives.
#[test]
fn a_mark_inside_a_loop_still_names_its_whole_expression() {
    let (_instances, schema) = derived(&reduction_in_loop());
    assert_eq!(
        schema.nodes[1].canonical("combine"),
        Some(&canonical(binary(
            BinaryOp::Add,
            binary(BinaryOp::Sub, datum(), producer()),
            state()
        ))),
        "the in-loop `combine` is the whole add, not the carried state its left operand reads"
    );
    assert_eq!(
        schema.nodes[1].canonical("element"),
        Some(&canonical(binary(BinaryOp::Sub, datum(), producer()))),
        "and the in-loop `element` mark is the difference, not the datum it subtracts from"
    );
    assert_eq!(
        schema.nodes[1].canonical("identity"),
        Some(&Expr::constant(0.0)),
        "and the loop's entering seed is the literal it wrote"
    );
    assert_eq!(
        schema.nodes[1].reduction(),
        Ok(Reduction {
            combine: BinaryOp::Add,
            identity: 0.0
        }),
        "so the reduction the marks declare is a sum from zero, derivable rather than refused"
    );
    assert_eq!(
        schema.nodes[0].canonical("element"),
        Some(&canonical(binary(
            BinaryOp::Mul,
            datum(),
            Expr::constant(2.0)
        ))),
        "the producer's top-level marked operator is unaffected by the consumer's loop"
    );
}
