//! An inlined transcendental reads as its operator wherever the walk reaches it.
//! See docs/notes/llm-operators.md.

use super::common;

use lichen_compute::fusion_claim::{fragment_of, kernel_id, marked_value};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_fusion::{BinaryOp, Expr, UnaryOp, Variable, normalize};
use lichen_fusion_bridge::{ReadRoles, projection};
use lichen_kernel_ir::KernelFragment;
use lichen_lowlevel::{AnyNodeId, LowValue, Module};

/// The extent the program's buffer types state.
///
/// # Invariant
/// The kernel is never launched - the program's root is the kernel - so the length is nominal:
/// it names the extent the kernel would run at, and the runtime takes the count from the launch.
const COUNT: usize = 8;
/// `silu`'s producer alone, the mark riding the whole quotient.
///
/// # Invariant
/// The kernel, its claim and the mark are the operator's own (`examples/silu.lichen`), so every
/// reading measured here is about **where the expansion sits** — two operators below the marked
/// value rather than at its root — and about nothing else. The parameter carries no `.out`
/// group: the body declares its output and returns the write.
fn producer() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
Gate  = struct{{.g (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
SiluA = struct{{.a (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
ParA  = struct{{.n Int, .in Gate}}
silu_raw = (k : ParA) => {{
  i = compute.range k.n
  g = (compute.read ((compute.Read _)(.from k.in.g, .at i)) : Float)
  a = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((SiluA)(.a (compute.write ((compute.Write _)(.to a, .at i, .value ((g / (1.0 + exp (0.0 - g))) ~ "element"))))))
}}
silu = compute.parallel (silu_raw ~ "claim:Elementwise(element)") "cpu" SiluA
silu
"#
    )
}

/// The producer's own fragment, the one the dispatcher registered.
///
/// # Invariant
/// The program's root **is** the kernel, so one root item names it, and the registry holds the
/// *expanded* fragment — `exp_body`'s instructions already inlined into it, which is the body
/// both readers match against.
fn producer_fragment(module: &Module<LangProgram>, value: &LangValue) -> KernelFragment {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("the program's root is a tuple");
    };
    let items: &[lichen_lowlevel::ArrayItem] = unsafe { array.items() };
    let item = items.first().expect("the root holds the kernel");
    let AnyNodeId::Dynamic(node) = item.node else {
        panic!("a kernel comes from a frozen artifact");
    };
    let kernel = kernel_id(module, node).expect("the root names a kernel");
    fragment_of(kernel).expect("the registry holds the dispatched fragment")
}

/// The marked `element`, read as one expression over the datum.
fn bridged(fragment: &KernelFragment) -> Expr {
    let marked = marked_value(fragment, "Elementwise", "element")
        .expect("the producer's body marks `element`");
    let expr = projection(fragment, marked, ReadRoles::DATUM_ONLY)
        .expect("an inlined `exp` below the marked value's root still reads as `exp`");
    normalize(&expr).expect("a projection the size of this one normalizes")
}

/// A binary operator over two operands, in the order the logic wrote it.
fn binary(operator: BinaryOp, left: Expr, right: Expr) -> Expr {
    Expr::Binary(operator, Box::new(left), Box::new(right))
}

/// `exp` bridges below the mark's own root, and the arithmetic around it still reads as itself.
#[test]
fn an_inlined_transcendental_below_the_marks_root_still_reads_as_the_operator() {
    let (module, value, _root) = common::run(&producer());
    let fragment = producer_fragment(&module, &value);
    let silu = |x: Expr| {
        binary(
            BinaryOp::Div,
            x.clone(),
            binary(
                BinaryOp::Add,
                Expr::constant(1.0),
                Expr::Unary(
                    UnaryOp::Exp,
                    Box::new(Expr::Unary(UnaryOp::Neg, Box::new(x))),
                ),
            ),
        )
    };
    assert_eq!(
        bridged(&fragment),
        silu(Expr::Variable(Variable::DATUM)),
        "the reader is asked at every value the walk reaches, so an expansion under a quotient \
         and a sum reads as `exp` while the arithmetic around it reads as itself"
    );
}
