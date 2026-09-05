use super::*;
use lichen_lowlevel::{AnyNodeId, ArrayItem, BlockId, LowValue, Module, OperatorExt};
use lichen_utils::extend::AsEnum;

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
    let out = LangOperator::GcdOp(GcdOp::Gcd).run(operand, block, &mut module);
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
    // uniform-`sup` implies uniform-`sub` iff sub | sup.  `0` is the top
    // ("uniform over all threads", the `∞` fold): only `0` satisfies a
    // `0` requirement, but a `0` value satisfies any requirement.
    assert!(divides(0, 0)); // uniform-over-all vs uniform-over-all
    assert!(!divides(0, 4)); // a `# 4` value is not uniform over all threads
    assert!(divides(4, 0)); // a uniform-over-all value is uniform over 4
    assert!(divides(4, 4)); // equal
    assert!(divides(2, 4)); // uniform-4 implies uniform-2
    assert!(divides(1, 4)); // uniform-4 implies uniform-1 (trivially)
    assert!(!divides(4, 2)); // uniform-2 does not imply uniform-4
    assert!(!divides(5, 2)); // incomparable
}
