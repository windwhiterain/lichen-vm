use super::*;
use lichen_highlevel::attr::AttrSet;
use lichen_highlevel::shape;
use lichen_lowlevel::{AnyNodeId, ArrayItem, BlockId, LowValue, Module, OperatorExt};
use lichen_utils::extend::AsEnum;

/// The canonical attribute order is the pair layout a compiled artifact encodes.
///
/// # Invariant
///
/// The refinement was appended, so the slots below `4` keep their numbers and
/// no persisted pair is renumbered. See docs/notes/attributes.md.
#[test]
fn the_canonical_order_is_the_persisted_pair_layout() {
    assert_eq!(
        LANG_ATTR_ORDER,
        [
            LangAttr::Perspective(Perspective),
            LangAttr::Doc(Doc),
            LangAttr::Refinement(Refinement),
        ]
    );
    assert_eq!(
        shape::attr_slot(LangAttr::Perspective(Perspective).order_index()),
        2
    );
    assert_eq!(shape::attr_slot(LangAttr::Doc(Doc).order_index()), 3);
    assert_eq!(
        shape::attr_slot(LangAttr::Refinement(Refinement).order_index()),
        4
    );
}

/// Feed `values` (as the operand array) to the language's `Gcd` operator
/// and return the computed meet.
fn gcd_run(values: &[usize]) -> usize {
    let mut module = Module::<LangProgram>::new();
    let block: BlockId = module.add_block(None);
    let items: Vec<ArrayItem> = values
        .iter()
        .map(|&n| {
            let node = module.add_node(block, None, Some(LangValue::from(LowValue::USize(n))));
            ArrayItem::new(AnyNodeId::Dynamic(node))
        })
        .collect();
    let array = module.alloc_array(&items, block);
    let operand = LangValue::from(LowValue::Array(array));
    let out = LangOperator::GcdOp(GcdOp::Gcd)
        .run(operand, block, &mut module)
        .expect("the gcd extension decides for concrete operands");
    let Some(LowValue::USize(n)) = out.as_enum() else {
        panic!("Gcd must evaluate to a USize meet")
    };
    n
}

#[test]
fn gcd_is_the_divisibility_meet() {
    assert_eq!(gcd(0, 0), 0);
    assert_eq!(gcd(0, 4), 4); // 0 is the top/identity
    assert_eq!(gcd(4, 0), 4);
    assert_eq!(gcd(4, 6), 2);
    assert_eq!(gcd(12, 8), 4);
    assert_eq!(gcd(2, 4), 2); // 2 | 4
}

#[test]
fn gcd_op_folds_the_operand_array() {
    assert_eq!(gcd_run(&[]), 0); // empty => the meet identity / top
    assert_eq!(gcd_run(&[4]), 4);
    assert_eq!(gcd_run(&[4, 6]), 2);
    assert_eq!(gcd_run(&[4, 0]), 4); // an absent child reads 0
    assert_eq!(gcd_run(&[2, 4, 6]), 2);
}

#[test]
fn divides_is_the_subtype_order() {
    // `0` is the top: `divides(sub, sup)` is uniform-`sup` implying uniform-`sub`.
    assert!(divides(0, 0));
    assert!(!divides(0, 4));
    assert!(divides(4, 0));
    assert!(divides(4, 4));
    assert!(divides(2, 4));
    assert!(divides(1, 4));
    assert!(!divides(4, 2));
    assert!(!divides(5, 2));
}
