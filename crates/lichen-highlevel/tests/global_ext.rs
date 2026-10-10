//! Downstream `GlobalExt` composition; see docs/notes/compiler-plugin.md.

use lichen_highlevel::program::{HighGlobal, HighProgramValue};
use lichen_lowlevel::{BlockId, GlobalExt, Module, NodeId, Operation, OperatorExt, Program};
use lichen_utils::compose::AsField;

/// The downstream's own component: plain struct, its own behaviour.
#[derive(Debug, Default)]
struct MyState {
    count: usize,
}

impl MyState {
    fn bump(&mut self) -> usize {
        let count = self.count;
        self.count += 1;
        count
    }
}

lichen_utils::compose_ext! {
    #[derive(Debug, Default)]
    struct MyGlobalExt(
        HighGlobal,
        MyState,
    );
}
impl GlobalExt for MyGlobalExt {}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MyProgram;

impl Program for MyProgram {
    type Value = HighProgramValue;
    type Operator = MyOperator;
    type GlobalExt = MyGlobalExt;
    type PackageMeta = ();
}

lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum MyOperator {
        /// Bump `MyState`, yield the previous count as `USize`.
        Bump,
    }
    + lichen_lowlevel::LowOperator as LowOperator;
}

impl OperatorExt<MyProgram> for MyOperator {
    fn run(
        &self,
        _operand: HighProgramValue,
        _block: BlockId,
        module: &mut Module<MyProgram>,
    ) -> Option<HighProgramValue> {
        match self {
            MyOperator::Bump => {
                // The upstream's component is reachable in the composed host.
                let _counter = AsField::<HighGlobal>::get(&module.global_ext).type_id_counter;
                let n = AsField::<MyState>::get_mut(&mut module.global_ext).bump();
                Some(HighProgramValue::LowValue(
                    lichen_lowlevel::LowValue::USize(n),
                ))
            }
            // The structural operators never reach `run`: the VM dispatches
            // them through `AsEnum` before falling through.
            MyOperator::LowOperator(_) => {
                unreachable!("structural operators are dispatched by the VM")
            }
        }
    }
}

fn op_node(m: &mut Module<MyProgram>, block: BlockId, operator: MyOperator) -> NodeId {
    m.add_node(
        block,
        Some(Operation {
            operator,
            operand: None,
        }),
        None,
    )
}

#[test]
fn a_downstream_global_ext_composes_flat_and_reaches_both_components() {
    let mut m = Module::<MyProgram>::new();
    let root = m.add_block(None);
    // Two separate operator nodes: the deep pass memoizes a node's value, so
    // stateful behavior shows across distinct nodes.
    let bump1 = op_node(&mut m, root, MyOperator::Bump);
    let bump2 = op_node(&mut m, root, MyOperator::Bump);

    let value = m.evaluate_node_deep(bump1, None);
    assert_eq!(
        value,
        Some(HighProgramValue::LowValue(
            lichen_lowlevel::LowValue::USize(0)
        ))
    );
    let value = m.evaluate_node_deep(bump2, None);
    assert_eq!(
        value,
        Some(HighProgramValue::LowValue(
            lichen_lowlevel::LowValue::USize(1)
        ))
    );

    assert_eq!(AsField::<HighGlobal>::get(&m.global_ext).type_id_counter, 0);
    assert_eq!(AsField::<MyState>::get(&m.global_ext).count, 2);
}
