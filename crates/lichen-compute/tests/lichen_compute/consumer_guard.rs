//! The tiling pass refuses a consumer node and leaves its recorded fragment alone.
//! See docs/notes/graph-fusion.md §5.

use super::common;

use super::common::LANES;
use lichen_compute::automatic_tiling::install_tiling;
use lichen_compute::compute::{clear_graph_pass, consult_graph_pass, take_graph_pass_lines};
use lichen_compute::fusion_claim::{Instance, claim_of, fragment_of, kernel_id};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_graph_ir::{Count, Graph, KernelNode, Node};
use lichen_kernel_ir::{KernelFragment, fragment_digest};
use lichen_lowlevel::Module;

/// One workgroup covers the launch, so the tile divides the extent exactly.
const COUNT: usize = LANES as usize;

/// The elements the program folds: `2.0 + 4.0 + 6.0` is the producer's own sum.
///
/// # Invariant
/// No element equals the sum, so a consumer reading the producer's value answers a number its own
/// element does not have, and a consumer that read the value as a datum answers the element.
const DATA: [f32; 3] = [2.0, 4.0, 6.0];

/// A `Σ` producer feeding a `max(d − r)` consumer, over a two-node recording.
///
/// # Invariant
/// The consumer reads the producer's output buffer at input position 1, and the node-local
/// one reads as a datum.
///
/// The recording is the root's third element, so a test reads the kernels and the graph
/// without dispatching: a dispatch needs a device.
///
/// No parameter carries an `.out` group: the body declares its buffer with
/// `compute.buf`, and the write is the codomain's value.
fn chain() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
OutR := _x => {{.r (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
Par  := _x => {{.n @any : Int, .in In _}}
producer_raw := (k <: Par) => {{
  i := compute.range k.n
  r := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read (compute.Read{{.from k.in.d, .at s(0) - 1}}) : Float)) ~ "combine"))
  ((OutR){{.r (compute.write (compute.Write{{.to r, .at i, .value fold (k.n, 0.0 ~ "identity")}}))}})
}}
producer := compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "gpu" (OutR _)
In2  := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}})), .p (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
OutZ := _x => {{.z (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
Par2 := _x => {{.n @any : Int, .in In2 _}}
consumer_raw := (k <: Par2) => {{
  i := compute.range k.n
  r := (compute.read (compute.Read{{.from k.in.p, .at 0}}) : Float)
  z := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((((compute.read (compute.Read{{.from k.in.d, .at s(0) - 1}}) : Float)) - r) ~ "element")) ~ "combine")
  ((OutZ){{.z (compute.write (compute.Write{{.to z, .at i, .value fold (k.n, (0.0 - 1000000.0) ~ "identity")}}))}})
}}
consumer := compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (OutZ _)
GArg := _x => {{.n @any : Int, .in In _}}
step := (s <: GArg) => {{
  first := (compute.plrun producer (compute.A{{.n s.n, .I In{{.d s.in.d}}}}) : Struct)
  compute.plrun consumer (compute.A{{.n s.n, .I In2{{.d s.in.d, .p first.r}}}})
}}
built := compute.graph step
(producer, consumer, built)
"#
    )
}

/// The chain with its recording run over the data the fill kernel writes, which is what a
/// differential needs.
///
/// # Invariant
/// The run is appended rather than fused into [`chain`], so the recording is one program and a
/// program that runs it is that program plus one statement.
fn running() -> String {
    with_run(chain())
}

/// The value the fill kernel writes at lane `i`.
///
/// # Invariant
/// The first three lanes carry [`DATA`] and every other lane carries zero, which a sum is
/// indifferent to. The cases are **summed** and not nested: a conditional in a kernel body
/// reaches the lowering as one node, so an `else` arm that is itself a conditional is a node
/// the body cannot emit.
fn datum() -> String {
    DATA.iter()
        .enumerate()
        .map(|(lane, element)| format!("(if i == {lane} then {element:?} else 0.0)"))
        .collect::<Vec<String>>()
        .join(" + ")
}

/// The fill kernel that builds the launch's input data, and the run that hands its buffer back.
///
/// # Invariant
/// One lane per element and each writes the datum at its own lane, so the buffer holds
/// [`DATA`] and then zeros. The kernel runs on the host's own backend, because a host builds
/// its data before a device ever sees it, and its parameter declares no `.in` group at all
/// (`docs/notes/kernel-buffer-declarations.md`).
fn filled() -> String {
    format!(
        r#"
Data := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
Fill := _x => {{.n @any : Int}}
fill := (c <: Fill) => {{
  i := compute.range c.n
  y := compute.buf (compute.BufParam{{.type Float, .length {COUNT}}})
  v := {datum}
  ((Data){{.d (compute.write (compute.Write{{.to y, .at i, .value v}}))}})
}}
filler := compute.parallel fill "cpu" (Data _)
"#,
        datum = datum(),
    )
}

/// `source`'s recording, run over the data the fill kernel writes.
///
/// # Invariant
/// The buffer is the first value the graph takes and the count the second, which is the order
/// `compute.graphrun` hands them over in; the source names its own three root elements. The
/// data is a kernel's own output, because a host builds its data by running a kernel
/// (`docs/notes/kernel-buffer-declarations.md`).
fn with_run(source: String) -> String {
    format!(
        "{source}\n{fill}\ndata := (compute.plrun filler ((Fill){{.n {count}}}) : Struct)\nout := compute.collect (compute.graphrun built ({count}, data.d))\nout\n",
        fill = filled(),
        count = COUNT,
    )
}

/// A `Σ` producer feeding a `combine`-only consumer whose `combine` reads the producer's value.
///
/// # Invariant
/// The consumer's claim is `Reduction(combine,identity)` and nothing else — the shape
/// `examples/reduction_interface.rs` records, where the between-code is a separate
/// `compute.jit … "claim:Reduction(element)"` rather than a role of the consumer's own claim — so
/// the role rule cannot see it and only the graph's edge says the value is not a datum.
fn bare_consumer() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
OutR := _x => {{.r (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
Par  := _x => {{.n @any : Int, .in In _}}
producer_raw := (k <: Par) => {{
  i := compute.range k.n
  r := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read (compute.Read{{.from k.in.d, .at s(0) - 1}}) : Float)) ~ "combine"))
  ((OutR){{.r (compute.write (compute.Write{{.to r, .at i, .value fold (k.n, 0.0 ~ "identity")}}))}})
}}
producer := compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "gpu" (OutR _)
In2  := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}})), .p (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
OutZ := _x => {{.z (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
Par2 := _x => {{.n @any : Int, .in In2 _}}
consumer_raw := (k <: Par2) => {{
  i := compute.range k.n
  z := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read (compute.Read{{.from k.in.d, .at s(0) - 1}}) : Float) * (compute.read (compute.Read{{.from k.in.p, .at 0}}) : Float))) ~ "combine"))
  ((OutZ){{.z (compute.write (compute.Write{{.to z, .at i, .value fold (i + 1, 0.0 ~ "identity")}}))}})
}}
consumer := compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity)") "gpu" (OutZ _)
GArg := _x => {{.n @any : Int, .in In _}}
step := (s <: GArg) => {{
  first := (compute.plrun producer (compute.A{{.n s.n, .I In{{.d s.in.d}}}}) : Struct)
  compute.plrun consumer (compute.A{{.n s.n, .I In2{{.d s.in.d, .p first.r}}}})
}}
built := compute.graph step
(producer, consumer, built)
"#
    )
}

/// One program as the guard's test reads it.
struct Run {
    module: Module<LangProgram>,
    value: LangValue,
    kernels: Vec<Instance>,
}

/// Compile, evaluate and read the chain's two kernels, a list and not the pass's join.
///
/// # Invariant
/// The same walk [`install_tiling`] performs over a module: a node whose own value names a
/// kernel is that kernel, and a kernel two nodes name is one entry because its id is. A
/// kernel claiming no interface is not the chain's: the fill kernel is no subject of the
/// guard. The registry is global, so the kernels are read from **its own** module, not from
/// what the registry gained.
fn run(source: &str) -> Run {
    let (module, value, _root) = common::run(source);
    let mut ids: Vec<usize> = module
        .nodes
        .iter()
        .filter_map(|(node, _value)| kernel_id(&module, node).ok())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut kernels: Vec<Instance> = Vec::new();
    for kernel in ids {
        let Some(claim) = claim_of(kernel) else {
            continue;
        };
        kernels.push(Instance {
            kernel,
            claim,
            fragment: fragment_of(kernel).expect("the registry holds the fragment it compiled"),
        });
    }
    // By buffer count, not by id: an id sort follows registration, not the source's order.
    kernels.sort_by_key(|kernel| kernel.fragment.inputs);
    Run {
        module,
        value,
        kernels,
    }
}

/// The two-node graph the recording's own fragments make, with the producer feeding the consumer.
///
/// # Invariant
/// The wiring is the recording's: the producer reads the graph's input 0, and the consumer reads
/// input 0 at ordinal 0 and the producer's output at ordinal 1, which is the read the region
/// derivation names with `ReadRoles::consumer(1)`. The fragments are the registry's own, so the
/// node a digest is read from holds the body the pass is handed.
fn chain_graph(producer: &KernelFragment, consumer: &KernelFragment) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let first = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: producer.clone(),
                inputs: vec![0],
                count: Count::Constant(COUNT),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    let second = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: consumer.clone(),
                inputs: vec![0, first],
                count: Count::Constant(COUNT),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![second])
        .expect("a value the graph defines");
    graph
}

/// The digest of a graph node's fragment.
fn digest_of(graph: &Graph, node: usize) -> u64 {
    let Node::Kernel(kernel) = &graph.nodes()[node];
    fragment_digest(&kernel.fragment)
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

/// The producer is tiled, the consumer is refused by name, and its recorded fragment is untouched.
///
/// # Invariant
/// The graph is built from the **registry**'s own two fragments, so both nodes hold exactly the
/// bodies the pass is handed, and the pass is consulted before any dispatch: the digests it
/// answers are the ones a run would dispatch. The refusal is asserted as the whole line, and
/// that line **now carries the check's name**, because every arm states its refusal by name.
#[test]
fn a_consumer_is_refused_by_name_and_its_recorded_fragment_is_untouched() {
    clear_graph_pass();
    let source = chain();
    let recorded = run(&source);
    assert_eq!(
        recorded.kernels[0].claim.roles,
        vec!["combine".to_string(), "identity".to_string()],
        "the producer's claim declares the node-local algebra and nothing else"
    );
    assert_eq!(
        recorded.kernels[1].claim.roles,
        vec![
            "combine".to_string(),
            "identity".to_string(),
            "element".to_string()
        ],
        "the consumer's claim declares the `element` role, whose code reads the producer"
    );

    install_tiling(&recorded.module);
    let graph = chain_graph(&recorded.kernels[0].fragment, &recorded.kernels[1].fragment);
    let before = (digest_of(&graph, 0), digest_of(&graph, 1));
    let rewritten = consult_graph_pass(&graph, &|_node| Some(COUNT))
        .expect("the installed pass answers a graph");
    let lines = take_graph_pass_lines();
    let after = (digest_of(&rewritten, 0), digest_of(&rewritten, 1));
    eprintln!("the recording's digests {before:?}, the pass's {after:?}");
    eprintln!("the pass's own lines {lines:?}");

    assert_ne!(
        before.0, after.0,
        "the producer's own marks license a tiled fold, so the pass lands one in its node"
    );
    assert_eq!(
        before.1, after.1,
        "the consumer's fragment digest is unchanged, so the node still holds what the program wrote"
    );

    let refused = format!(
        "node 1: refused: BeyondANode: the instance at {} claims the role(s) element beyond \
         `combine` and `identity`, whose code reads the value another node produced, and this \
         reader names no producer",
        recorded.kernels[1].kernel,
    );
    assert!(
        lines.contains(&refused),
        "the pass refuses the consumer in the schema refusal's own words, **with the check's \
         name**: the derivation's refusals used to reach the report through an arm that printed \
         the sentence alone, which left `BeyondANode` unmatchable against its variant: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("node 0:") && line.contains("tiled")),
        "and the producer's line reports its tiling: {lines:?}"
    );
}

/// The program answers the same with the pass installed and without it.
///
/// # Invariant
/// The differential the guard exists for: the consumer's own body reads the producer's value, and
/// a pass that tiled it would replace that read with a datum read and answer a different function.
#[test]
fn the_program_answers_the_same_with_the_pass_and_without_it() {
    let (_stub, _guard) = common::stub();
    clear_graph_pass();
    let source = running();
    let (base_module, base_value, _root) = common::run(&source);
    let base = common::float_array(&base_module, &base_value);
    eprintln!("the program with no pass: {base:?}");

    let (recorded_module, _value, _root) = common::run(&source);
    install_tiling(&recorded_module);
    let (passed_module, passed_value, _root) = common::run(&source);
    let passed = common::float_array(&passed_module, &passed_value);
    eprintln!("the program with the pass: {passed:?}");
    eprintln!("the pass's report: {:?}", pass_report(&passed_module));

    assert_eq!(
        passed, base,
        "the guard refuses the consumer and leaves its body, so the answer is the program's own"
    );
    assert!(
        pass_report(&passed_module)
            .iter()
            .any(|line| line.contains("node 1: refused")),
        "and the run's own report names the refusal: {:?}",
        pass_report(&passed_module)
    );
}

/// A `combine`-only consumer is refused by the graph's edge, and the differential holds.
///
/// # Invariant
/// The claim's roles cannot say this node reads another node's output, so the refusal is the one
/// the graph's own value numbering answers: the consumer's second input, value 1, is the
/// producer's output. The digest is compared across the pass, which is what says nothing landed.
#[test]
fn a_combine_only_consumer_is_refused_by_the_edge_and_the_differential_holds() {
    let (_stub, _guard) = common::stub();
    clear_graph_pass();
    let source = with_run(bare_consumer());
    let recorded = run(&source);
    let base = common::float_array(&recorded.module, &recorded.value);
    assert_eq!(
        recorded.kernels.len(),
        2,
        "the bare chain is two claimed kernels: a producer and a consumer that reads it"
    );
    assert_eq!(
        recorded.kernels[0].claim.roles, recorded.kernels[1].claim.roles,
        "both claims declare the node-local algebra, so the role rule cannot tell them apart"
    );
    assert_ne!(
        fragment_digest(&recorded.kernels[0].fragment),
        fragment_digest(&recorded.kernels[1].fragment),
        "the two fragments differ, so the consumer is a row of its own"
    );

    install_tiling(&recorded.module);
    let graph = chain_graph(&recorded.kernels[0].fragment, &recorded.kernels[1].fragment);
    let rewritten = consult_graph_pass(&graph, &|_node| Some(COUNT))
        .expect("the installed pass answers a graph");
    let lines = take_graph_pass_lines();
    eprintln!("the edge refusal's own lines {lines:?}");

    assert_ne!(
        digest_of(&graph, 0),
        digest_of(&rewritten, 0),
        "the producer's own marks license a tiled fold, so the pass lands one in its node"
    );
    assert_eq!(
        digest_of(&graph, 1),
        digest_of(&rewritten, 1),
        "the consumer's fragment digest is unchanged, so the node still holds what the program wrote"
    );
    let refused = format!(
        "node 1: refused: the instance at 1 reads value 1, which another node of the graph \
         produced, so the value it reads as its own datum is not its own"
    );
    assert!(
        lines.contains(&refused),
        "the refusal names the node and the value it reads: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("node 0:") && line.contains("tiled")),
        "and the producer's line reports its tiling: {lines:?}"
    );

    let (passed_module, passed_value, _root) = common::run(&source);
    let passed = common::float_array(&passed_module, &passed_value);
    let report = pass_report(&passed_module);
    eprintln!("the bare chain with no pass: {} element(s)", base.len());
    eprintln!("the bare chain with the pass: {} element(s)", passed.len());
    eprintln!("the bare run's report: {report:?}");
    assert_eq!(
        passed, base,
        "the edge check refuses the consumer and leaves its body, so the answer is the program's own"
    );
    assert!(
        report.iter().any(|line| line.contains("node 1: refused:")
            && line.contains("reads value")
            && line.contains("which another node")),
        "and the run's own report names the refused node and the value it reads: {report:?}"
    );
    assert!(
        report
            .iter()
            .any(|line| line.contains("node 0:") && line.contains("tiled")),
        "and the producer still tiles: {report:?}"
    );
}
