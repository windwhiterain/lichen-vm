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
//! `liche_language::lang_compose_vocabulary! { … plugins = [ lichen_std_native as lichen_std_native_leaves; ]; }`
//! (the plugin's `lichen_std_native_leaves!` macro contributes the `SortOp` leaf),
//! then drives the produced compiler.  The plugin also wires the **native-call
//! extension point** ([`NativeOp`]) over [`SortOp`] and carries an embedded
//! [`WRAPPER_SOURCE`] — a `std.lichen` lichen source that wraps the raw
//! `$sort` native call into a real, user-facing typed `sort` function — the
//! same shape as the reference `lichen-compute` native plugin.  A host serves
//! that source (see the package store's native-package registration) and
//! builds the plugin's private per-module registry with
//! [`lichen_std_native_ops!`], so `$sort` resolves privately against the
//! plugin's own source.

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg};
use lichen_highlevel::program::{Ctx, HighProgram, ValueType};
use lichen_lowlevel::codec::{OperatorCodec, Reader, Writer};
use lichen_lowlevel::{AnyNodeId, ArrayItem, BlockId, LowValue, Module, OperatorExt, Program};
use lichen_utils::extend::AsEnum;

// The native-op extension point, re-exported so a host builds the plugin's
// private registry with `lichen_std_native_ops!` without depending on
// `lichen_highlevel`.
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

/// The plugin leaf's per-leaf artifact codec: the single `Sort` operator.
///
/// A native plugin that wants its built compiler to keep a persistent
/// `~/.lichen` cache implements [`OperatorCodec`] (and [`ValueCodec`] for any
/// value leaf) for its leaves; this one is a scalar operator, so the codec is
/// a one-tag identical round-trip.
impl OperatorCodec for SortOp {
    fn write_operator(w: &mut Writer, op: Self) {
        match op {
            SortOp::Sort => w.u8(0),
        }
    }

    fn read_operator(r: &mut Reader<'_>) -> Result<Self, String> {
        match r.u8()? {
            0 => Ok(SortOp::Sort),
            tag => Err(format!("unknown sort-operator tag {tag}")),
        }
    }
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

/// The compile-time lowering of [`SortOp`] as a native operator: the checker's
/// `$sort(a)` route.  It gates the argument as a `[usize]` array (the element
/// type pinned to `Int` and the length left a fresh cell), emits the
/// [`SortOp::Sort`] operator node over the argument's value, and returns that
/// constrained array type as the result — so the lichen wrapper's `sort`
/// function reads with a real `[Int, len] -> [Int, len]` type rather than an
/// opaque native application.
impl<P> NativeOp<P> for SortOp
where
    P: HighProgram,
    P::Value: ValueType,
    P::Operator: From<SortOp>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let a = &args[0];
        // Array gate: `a : [Int, len]` — the type of a `[usize]` array.  The
        // element type is pinned to `[int, Type]` (`Ctx::int_type`, the type
        // of every usize value) and the length is a fresh cell — a sort
        // preserves the length but the checker need not observe it until the
        // array's length is read.
        let len = ctx.fresh();
        let shape = ctx.array_node(&[ctx.int_type(), len]);
        let kind = ctx.kind_expr(ctx.array_type_marker_node());
        let array_ty = ctx.array_node(&[shape, kind]);
        ctx.check_unify(a.ty, array_ty, loc, DiagKind::Guard);
        // The bare sort operator over the array value; its result is the
        // constrained array type.
        let op = ctx.op_node(P::Operator::from(SortOp::Sort), Some(a.value));
        let pair = ctx.array_node(&[op, array_ty]);
        NativeApply {
            node: pair,
            val: Some(op),
            ty: array_ty,
        }
    }
}

/// The `lichen-std-native` plugin's embedded lichen source — the actual `std`
/// plugin file, kept as a `.lichen` source file and embedded with
/// [`include_str!`].  It defines the user-facing `sort` function as ordinary
/// typed lichen (whose body calls the native `$sort`), and exports it as a
/// **named struct** (`std.sort`).  A host compiles this against the plugin's
/// private native registry (`lichen_std_native_ops!`) and serves it as a
/// virtual package (`std.lichen`), the package-manager plug shape.
pub const WRAPPER_SOURCE: &str = include_str!("std.lichen");

/// Assemble `lichen-std-native`'s private native-operator registry for a host
/// program `$program`, expanding to a `&'static` [`NativeOps`].
///
/// Invoked by a host that composes the plugin (see the package store's native
/// package registration), so the `$sort` name stays private to the plugin's
/// own embedded source.  The host names only the plugin crate and its program
/// marker — never the plugin's op structs — so this is the composition point a
/// package manager would generate.
#[macro_export]
macro_rules! lichen_std_native_ops {
    ($program:ty) => {{
        static SORT: $crate::SortOp = $crate::SortOp::Sort;
        let ops: Vec<(&'static str, &'static dyn $crate::NativeOp<$program>)> =
            vec![("sort", &SORT as &dyn $crate::NativeOp<$program>)];
        Box::leak(ops.into_boxed_slice()) as $crate::NativeOps<$program>
    }};
}

/// Contribute this plugin's vocabulary leaves into a
/// [`liche_language::lang_compose_vocabulary!`] composition (see the
/// `lichen-compute` [`liche_leaves!`] protocol): it hands back the `SortOp`
/// operator leaf, threading the composition's accumulator.
///
/// The macro name is `<crate_ident>_leaves` (`lichen_std_native_leaves`), not
/// a fixed `liche_leaves`: two `#[macro_export]` macros named identically in
/// the dependency graph collide in the extern prelude, so each plugin's leaf
/// macro has a distinct, crate-derivable name.
#[macro_export]
macro_rules! lichen_std_native_leaves {
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
