//! The `lichen-std-native` native plugin: a standard library of array sorts.
//!
//! This is a **native plugin** (see [`lichen_highlevel::plugin`]): it extends the
//! core lowlevel/highlevel through the program-generic extension points and
//! never names a host `Program` marker, its IR, its grammar, or its on-disk
//! format.  It contributes exactly one operator leaf — [`SortOp`], which sorts
//! a lichen `[usize]` array — backed by Rust's `sort_unstable`, and its
//! program-generic [`OperatorExt`] `run` so a host can execute it.
//!
//! A host composes it with
//! `liche_language::lang_compose_vocabulary! { … plugins = [ lichen_std_native; ]; }`
//! (the plugin's [`liche_leaves!`] macro contributes the `SortOp` leaf), then
//! drives the produced compiler.

use lichen_lowlevel::{AnyNodeId, ArrayItem, BlockId, LowValue, Module, OperatorExt, Program};
use lichen_utils::extend::AsEnum;

/// The nominal native-plugin marker for this crate (see
/// [`lichen_highlevel::plugin::NativePlugin`]).
pub struct StdNativePlugin;

impl lichen_highlevel::plugin::NativePlugin for StdNativePlugin {}

/// The operator leaf: sort a lichen `[usize]` array (ascending).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortOp {
    /// Ascending sort of the operand array's `usize` values.
    Sort,
}

/// The program-generic VM dispatch for [`SortOp`]: read the operand array's
/// `USize` values, sort them with Rust, and return a new array of the sorted
/// values.
impl<P> OperatorExt<P> for SortOp
where
    P: Program,
    P::Value: From<LowValue> + AsEnum<LowValue>,
{
    fn run(&self, operand: P::Value, block: BlockId, module: &mut Module<P>) -> P::Value {
        let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(&operand) else {
            // A non-array sort target is a *reported* type error (the checker's
            // array gate), not an invariant violation — stay lazy rather than
            // panicking.
            return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
        };
        let mut values: Vec<usize> = array
            .items()
            .iter()
            .filter_map(|item| {
                module
                    .node_value(item.node)
                    .and_then(|value| match value.as_enum() {
                        Some(LowValue::USize(n)) => Some(n),
                        _ => None,
                    })
            })
            .collect();
        values.sort_unstable();
        let items: Vec<ArrayItem> = values
            .into_iter()
            .map(|n| {
                let node = module.add_node(
                    block,
                    None,
                    Some(<P::Value as From<LowValue>>::from(LowValue::USize(n))),
                );
                ArrayItem::new(AnyNodeId::Dynamic(node))
            })
            .collect();
        let handle = module.alloc_array(&items, block);
        <P::Value as From<LowValue>>::from(LowValue::Array(handle))
    }
}

/// Contribute this plugin's vocabulary leaves into a
/// [`liche_language::lang_compose_vocabulary!`] composition (see the
/// `lichen-compute` [`liche_leaves!`] protocol): it hands back the `SortOp`
/// operator leaf, threading the composition's accumulator.
#[macro_export]
macro_rules! lichen_leaves {
    ($next:path, [ $($oa:tt)* ][ $($va:tt)* ][ $($aa:tt)* ][ $($b:tt)* ] ; [ $($rest:tt)* ] ;) => {
        $next! {
            @absorb (
                operators: [ lichen_std_native::SortOp as SortOp; ];
                values: [ ];
                attrs: [ ];
                [ $($oa)* ][ $($va)* ][ $($aa)* ][ $($b)* ] ; [ $($rest)* ] ;
            )
        }
    };
}
