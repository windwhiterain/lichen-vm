//! The `lichen-std-native` native plugin (`SortOp` + `std.lichen`); see
//! docs/notes/plugin-taxonomy.md.

use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg};
use lichen_highlevel::program::{Ctx, HighProgram, ValueType};
use lichen_highlevel::shape;
use lichen_lowlevel::codec::{OperatorCodec, Reader, Writer};
use lichen_lowlevel::{
    AnyNodeId, ArrayItem, ArrayValue, BlockId, ExtensionDiagnostic, LowValue, Module, NodeId,
    OperatorExt, Program,
};
use lichen_utils::extend::AsEnum;

// Re-exported so a host builds the plugin's private op registry without
// depending on `lichen_highlevel`.
pub use lichen_highlevel::native::{NativeOp, NativeOps};

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

/// The plugin leaf's artifact codec: a one-tag round-trip for `Sort`, so a
/// built compiler keeps its `~/.lichen` cache.
impl OperatorCodec for SortOp {
    fn write_operator(w: &mut Writer, op: Self) -> Result<(), String> {
        match op {
            SortOp::Sort => w.u8(0),
        }
        Ok(())
    }

    fn read_operator(r: &mut Reader<'_>) -> Result<Self, String> {
        match r.u8()? {
            0 => Ok(SortOp::Sort),
            tag => Err(format!("unknown sort-operator tag {tag}")),
        }
    }
}

/// The program-generic VM dispatch for [`SortOp`]: sort the operand array's
/// `USize` values into a new array.
impl<P> OperatorExt<P> for SortOp
where
    P: Program,
    P::Value: From<LowValue> + AsEnum<LowValue>,
{
    fn run(&self, operand: P::Value, block: BlockId, module: &mut Module<P>) -> Option<P::Value> {
        // `None` is the trait's spelling of "undecided"; the closure gives the
        // early return and the answer one type.
        (|| {
            let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(&operand) else {
                // A non-array target is the checker's reported type error, not an
                // invariant violation: stay lazy, do not panic.
                return None;
            };
            // SAFETY: `array` is the operand value's payload; its home block is
            // alive for all of `run`, so the slice stays valid.
            let input = unsafe { array.items() };
            let template = *input.first()?;
            let element_type = input
                .get(shape::ARRAY_VALUE_ELEMENT_BASE)
                .and_then(|item| shape::slot_type(module, item.node))
                .or_else(|| shape::slot_type(module, template.node));
            let mut values: Vec<usize> = Vec::new();
            for item in input
                .get(shape::ARRAY_VALUE_ELEMENT_BASE..)
                .unwrap_or_default()
            {
                // A compound element is a slot: the value is its first position.
                let Some(element) = module.element_value_node(item.node) else {
                    return None;
                };
                let value = match module
                    .evaluate_node_deep_ref(element, Some(block))
                    .or_else(|| module.node_value(element))
                {
                    Some(value) => value,
                    None => {
                        return None;
                    }
                };
                match value.as_enum() {
                    Some(LowValue::USize(n)) => values.push(n),
                    Some(LowValue::Error) => return Some(P::Value::from(LowValue::Error)),
                    Some(LowValue::Any) => return None,
                    _ => {
                        module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                            "std.sort",
                            None,
                            "std.sort expects an array of Int elements".to_string(),
                        ));
                        return None;
                    }
                }
            }
            values.sort_unstable();
            // The result's elements are slots.
            // SAFETY: the default attributes are a live array of this module.
            let width = unsafe { module.default_attributes()?.items() }.len();
            let indices: Vec<u32> =
                (0..width.saturating_sub(shape::EXPR_ATTR_BASE) as u32).collect();
            let layout = module.layout_with_indices(&indices)?;
            let element_type = element_type.map(|node| module.as_dynamic(node, block));
            let mut items: Vec<ArrayItem> = Vec::with_capacity(values.len() + 1);
            // Preserve the input's element template, length, and template marker.
            items.push(ArrayItem {
                template: template.template,
                ..ArrayItem::new(template.node)
            });
            for n in values {
                let value = module.add_node(
                    block,
                    None,
                    Some(<P::Value as From<LowValue>>::from(LowValue::USize(n))),
                );
                let mut slot = vec![ArrayItem::new(AnyNodeId::Dynamic(value))];
                slot.push(ArrayItem::new(AnyNodeId::Dynamic(
                    element_type.unwrap_or_else(|| module.add_node(block, None, None)),
                )));
                for _ in indices.iter() {
                    let any = module.add_node(
                        block,
                        None,
                        Some(<P::Value as From<LowValue>>::from(LowValue::Any)),
                    );
                    slot.push(ArrayItem::new(AnyNodeId::Dynamic(any)));
                }
                let handle = module.alloc_array(&slot, block);
                let slot = module.add_node(
                    block,
                    None,
                    Some(<P::Value as From<LowValue>>::from(LowValue::Array(
                        ArrayValue::with_layout(handle, layout),
                    ))),
                );
                items.push(ArrayItem::new(AnyNodeId::Dynamic(slot)));
            }
            let handle = module.alloc_array(&items, block);
            Some(<P::Value as From<LowValue>>::from(LowValue::Array(
                handle.into(),
            )))
        })()
    }

    /// Evaluate even with open `@any` type cells; elements decide laziness.
    fn run_deferred(
        &self,
        operand: Option<NodeId>,
        block: BlockId,
        module: &mut Module<P>,
    ) -> Option<P::Value> {
        let value = match operand {
            Some(node) => {
                module.evaluate_node_deep(node, Some(block));
                module.node_value(AnyNodeId::Dynamic(node))?
            }
            None => P::Value::from(LowValue::Error),
        };
        self.run(value, block, module)
    }
}

/// The compile-time lowering of [`SortOp`] for `$sort(a)`: emit `Sort`; the
/// wrapper source's annotations carry the types.
impl<P> NativeOp<P> for SortOp
where
    P: HighProgram,
    P::Value: ValueType,
    P::Operator: From<SortOp>,
{
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let a = &args[0];
        // The bare operator over the array value; the wrapper states the
        // `[Int, len]` result.  A sort preserves the length.
        let op = ctx.op_node(P::Operator::from(SortOp::Sort), Some(a.value));
        NativeApply {
            value: op,
            decided: true,
        }
    }
}

/// The plugin's embedded `std.lichen`: the user-facing `sort` over native
/// `$sort`, served as a virtual package.
pub const WRAPPER_SOURCE: &str = include_str!("std.lichen");

/// Assemble the plugin's private op registry for a host `$program`, expanding
/// to a `&'static` [`NativeOps`].
#[macro_export]
macro_rules! lichen_std_native_ops {
    ($program:ty) => {{
        static SORT: $crate::SortOp = $crate::SortOp::Sort;
        let ops: Vec<(&'static str, &'static dyn $crate::NativeOp<$program>)> =
            vec![("sort", &SORT as &dyn $crate::NativeOp<$program>)];
        Box::leak(ops.into_boxed_slice()) as $crate::NativeOps<$program>
    }};
}

/// Contribute the `SortOp` leaf into a
/// [`lichen_language::lang_compose_vocabulary!`] composition.
#[macro_export]
macro_rules! lichen_std_native_leaves {
    ($next:path, [ $($oa:tt)* ][ $($va:tt)* ][ $($aa:tt)* ][ $($b:tt)* ][ $($rn:tt)* ] ; [ $($rest:tt)* ] ;) => {
        $next! {
            @absorb (
                operators: [ lichen_std_native::SortOp as SortOp; ];
                values: [ ];
                attrs: [ ];
                [ $($oa)* ][ $($va)* ][ $($aa)* ][ $($b)* ][ $($rn)* ] ; [ $($rest)* ] ;
            )
        }
    };
}
