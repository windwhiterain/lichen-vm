//! Downstream `GlobalExt` extension: a downstream composes its own global
//! state by listing the upstream's components *flat* (by symbol) alongside
//! its own — one `compose_ext!` invocation, no per-upstream wrapper.  Flat is
//! load-bearing: per-type [`AsField`] access (including the highlevel's own
//! operators reading `HighGlobal`, like `Fresh`) resolves only against a
//! flat tuple, so nesting the upstream host would break them.

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

// The composed host: the upstream's `HighGlobal` listed flat, then the
// downstream's own.  The downstream opts into the lowlevel contract with
// the explicit `GlobalExt` impl.
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

// The downstream's own operator vocabulary: composed the same way as the
// values — its own variant plus the structural operators carried flat.
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
    ) -> HighProgramValue {
        match self {
            MyOperator::Bump => {
                // The upstream's component is reachable in the composed host.
                let _counter = AsField::<HighGlobal>::get(&module.global_ext).type_id_counter;
                let n = AsField::<MyState>::get_mut(&mut module.global_ext).bump();
                HighProgramValue::LowValue(lichen_lowlevel::LowValue::USize(n))
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
        HighProgramValue::LowValue(lichen_lowlevel::LowValue::USize(0))
    );
    let value = m.evaluate_node_deep(bump2, None);
    assert_eq!(
        value,
        HighProgramValue::LowValue(lichen_lowlevel::LowValue::USize(1))
    );

    // The operator's `Fresh`-style co-existence check never fired: the
    // upstream component sat untouched in the shared host, and the type-id
    // counter is exactly where `HighProgramOperator::TypeOperator(
    // TypeOperator::Fresh)` would find it on the highlevel program.
    assert_eq!(AsField::<HighGlobal>::get(&m.global_ext).type_id_counter, 0);
    assert_eq!(AsField::<MyState>::get(&m.global_ext).count, 2);
}
