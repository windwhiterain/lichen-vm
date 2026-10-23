//! A consumer that reads only its **second** input, and the class list that names it.
//!
//! # Invariant
//! See `docs/notes/graph-fusion.md` §3: a fragment's `input_classes` is a position-indexed
//! list, so a body that reads one input out of two is described by the classes **at the
//! positions**, and a hole takes the class the parameter's own `Buf T` declares for it.

use super::common;

use lichen_compute::fusion_claim::{fragment_of, kernel_id};
use lichen_compute::language::LangProgram;
use lichen_kernel_ir::{KernelFragment, ScalarClass};
use lichen_lowlevel::Module;

/// The consumer's input position the producer's output feeds: `.e`, never `.d`.
const ABSORBED: usize = 1;

/// The chain: an elementwise producer, then a reduction consumer that reads only its second input.
///
/// # Invariant
/// The consumer's body names `lane.in.e` and **never** `lane.in.d`, so the parameter declares
/// two float inputs while the body reads position 1 alone: the case where a class list built in
/// emission order and a list read by position part company. Neither parameter carries an `.out`
/// group: each body declares its output and returns the write.
const SOURCE: &str = r#"
---
  compute = import "compute.lichen"
---
In   = struct{.d (compute.Buf Float)}
OutE = struct{.e (compute.Buf Float)}
Par  = struct{.n Int, .in In}
producer_raw = (lane : Par) => {
  at = compute.range lane.n
  e = compute.slice ((compute.Slice _)(.count lane.n, .element Float))
  ((OutE)(.e (compute.write ((compute.Write _)(.to e, .at at, .value (2.0 * (compute.read ((compute.Read _)(.from lane.in.d, .at at)) : Float)))))))
}
producer = compute.parallel producer_raw "cpu" OutE
In2  = struct{.d (compute.Buf Float), .e (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = struct{.n Int, .in In2}
consumer_raw = (lane : Par2) => {
  at = compute.range lane.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from lane.in.e, .at s(0) - 1)) : Float)))
  z = compute.slice ((compute.Slice _)(.count lane.n, .element Float))
  ((OutZ)(.z (compute.write ((compute.Write _)(.to z, .at at, .value fold (lane.n, 0.0))))))
}
consumer = compute.parallel consumer_raw "cpu" OutZ
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  first = (compute.plrun producer ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .e first.e)))
}
built = compute.graph step
(producer, consumer, built)
"#;

#[test]
fn the_consumer_declares_a_float_for_each_of_its_two_inputs() {
    let (module, _value, _root) = common::run(SOURCE);
    let (_producer, consumer) = instances(&module);

    // The parameter's own declaration: two float inputs, position by position.
    assert_eq!(
        consumer.roles.input_classes,
        vec![Some(ScalarClass::Float), Some(ScalarClass::Float)],
        "the consumer's parameter declares a `Float` for each of its two `.in` buffers"
    );
    // The fragment's list, indexed the way every reader indexes it.
    assert_eq!(
        (consumer.inputs, consumer.input_classes.clone()),
        (2, vec![ScalarClass::Float, ScalarClass::Float]),
        "the class list is one entry per input **position**: the position the body never read \
         takes the class the parameter declared for it, not the position after it"
    );
    assert_eq!(
        consumer.input_classes[ABSORBED],
        ScalarClass::Float,
        "the position the producer's output feeds is the one the body reads, so its own class is \
         the class the read declares"
    );
}

/// The chain's two fragments, the consumer first: the one that declares two input buffers.
fn instances(module: &Module<LangProgram>) -> (KernelFragment, KernelFragment) {
    let mut fragments: Vec<KernelFragment> = module
        .nodes
        .iter()
        .filter_map(|(id, _node)| kernel_id(module, id).ok())
        .filter_map(|id| fragment_of(id).ok())
        .collect();
    fragments.sort_by_key(|fragment| fragment.inputs);
    let consumer = fragments
        .pop()
        .expect("the program dispatched a two-input consumer");
    let producer = fragments.pop().expect("the program dispatched a producer");
    (producer, consumer)
}
