//! The composed [`LangProgram`] marker. See docs/notes/plugin-taxonomy.md.

use lichen_highlevel::program::{TypeOperator, TypeValue};
use lichen_lowlevel::{LowOperator, LowValue};

pub use lichen_perspective::{GcdOp, Perspective, divides, gcd, persp_attr_ext};

use lichen_doc::Doc;
pub use lichen_doc::doc_attr_ext;

pub use lichen_highlevel::refinement::{Refinement, refinement_attr_ext};

/// The index of a manifest entry as a const expression; macro machinery.
#[doc(hidden)]
#[macro_export]
macro_rules! lang_attr_position {
    () => { 0usize };
    ($head:tt $($rest:tt)*) => { 1usize + $crate::lang_attr_position!($($rest)*) };
}

/// The canonical-order index of `$value`, as nested `if let`s; macro machinery.
#[doc(hidden)]
#[macro_export]
macro_rules! lang_attr_order_index {
    ($value:expr, $set:ident; [$($done:tt)*]) => { ::core::unreachable!() };
    ($value:expr, $set:ident; [$($done:tt)*] $head:ident ; $($rest:tt)*) => {
        if let $set::$head(_) = $value {
            $crate::lang_attr_position!($($done)*)
        } else {
            $crate::lang_attr_order_index!($value, $set; [$($done)* $head] $($rest)*)
        }
    };
}

/// Compose the program marker and its attribute order from one manifest.
/// See docs/notes/plugin-taxonomy.md.
///
/// # Invariant
///
/// The `attrs` list is the single authority for the canonical attribute order:
/// the order index, `LANG_ATTR_ORDER` and the build-time slot check derive from
/// it, and no marker names a slot number. See docs/notes/attributes.md.
#[macro_export]
macro_rules! lang_compose_vocabulary {
    // With a plugin set: each plugin contributes leaves via its own exported macro.
    (
        attrs = [ $( $attr:path as $attr_name:ident ; )* ] [ $( $bound:tt )* ];
        values = [ $( $value:path as $value_name:ident ; )* ];
        operators = [ $( $operator:path as $operator_name:ident ; )* ];
        plugins = [ $( $plugin:ident as $leaves:ident ; )* ];
    ) => {
        $crate::lang_compose_vocabulary! {
            @run
            [ $( $operator as $operator_name ; )* ] [ $( $value as $value_name ; )* ] [ $( $attr as $attr_name ; )* ] [ $( $bound )* ];
            [ $( $plugin as $leaves ; )* ];
        }
    };

    // Without a plugin set (the shipping compiler): `plugins = []`.
    (
        attrs = [ $( $attr:path as $attr_name:ident ; )* ] [ $( $bound:tt )* ];
        values = [ $( $value:path as $value_name:ident ; )* ];
        operators = [ $( $operator:path as $operator_name:ident ; )* ];
    ) => {
        $crate::lang_compose_vocabulary! {
            @run
            [ $( $operator as $operator_name ; )* ] [ $( $value as $value_name ; )* ] [ $( $attr as $attr_name ; )* ] [ $( $bound )* ];
            [ ];
        }
    };

    // Terminal: the first two operator and value leaves are the structural ones.
    (
        @run
        [ $lowop:path as $lowop_name:ident ; $tyop:path as $tyop_name:ident ; $( $extra_op:path as $extra_op_name:ident ; )* ]
        [ $low:path as $low_name:ident ; $tyv:path as $tyv_name:ident ; $( $extra_v:path as $extra_v_name:ident ; )* ]
        [ $( $attr:path as $attr_name:ident ; )* ] [ $( $bound:tt )* ];
        [ ] ;
    ) => {
        ::lichen_utils::enum_ext! {
            /// The operator vocabulary: one variant per composed leaf.
            #[derive(Debug, Clone, Copy, PartialEq)]
            pub enum LangOperator {
            }
            + $lowop as $lowop_name ;
            + $tyop as $tyop_name ;
            $( + $extra_op as $extra_op_name ; )*
        }

        ::lichen_utils::enum_ext! {
            /// The value vocabulary: one variant per composed leaf.
            #[derive(Debug, Clone, Copy, PartialEq)]
            pub enum LangValue {
            }
            + $low as $low_name ;
            + $tyv as $tyv_name ;
            $( + $extra_v as $extra_v_name ; )*
        }

        /// The language's compile-time attributes, one variant per marker.
        ///
        /// # Invariant
        ///
        /// The declaration order is the canonical attribute order and the single
        /// authority for the pair-slot layout. See docs/notes/attributes.md.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum LangAttr {
            $( $attr_name($attr) ),*
        }

        impl ::lichen_highlevel::attr::AttrSpec for LangAttr {}

        impl LangAttr {
            /// This attribute's position in the canonical order, as a constant.
            pub const fn order_index_of(self) -> usize {
                $crate::lang_attr_order_index!(self, LangAttr; [] $( $attr_name ; )*)
            }
        }

        impl ::lichen_highlevel::attr::AttrSet for LangAttr {
            /// The canonical order as data — the pair layout itself.
            const ORDER: &'static [Self] = LANG_ATTR_ORDER;

            /// The attribute's index in the canonical order — its pair slot.
            fn order_index(&self) -> usize {
                self.order_index_of()
            }
        }

        /// The canonical attribute order as data: the pair-slot layout itself.
        pub const LANG_ATTR_ORDER: &[LangAttr] = &[ $( LangAttr::$attr_name($attr) ),* ];

        // A hand-edited index fails the build here, not a runtime mis-pairing.
        const _: () = {
            let mut i = 0;
            while i < LANG_ATTR_ORDER.len() {
                assert!(
                    LANG_ATTR_ORDER[i].order_index_of() == i,
                    "an attribute's canonical index must be its position in the canonical order"
                );
                i += 1;
            }
        };

        /// The attribute-extension registry: each marker to its behaviour.
        pub fn lang_attr_ext<P>() -> Box<dyn Fn(&LangAttr) -> &'static dyn ::lichen_highlevel::attr::AttrExt<P>>
        where
            P: ::lichen_highlevel::program::HighProgram,
            P::Value: ::lichen_highlevel::program::ValueType + ::lichen_utils::extend::AsEnum<::lichen_lowlevel::LowValue>,
            $( $bound )*
        {
            Box::new(|attr| -> &'static dyn ::lichen_highlevel::attr::AttrExt<P> {
                match attr {
                    $( LangAttr::$attr_name(_) => &$attr ),*
                }
            })
        }

        /// The concrete program marker: value, operator and attribute unions.
        ///
        /// # Invariant
        ///
        /// A fresh nominal type, not an alias of `ProgramImpl`, so the wiring
        /// impls stay orphan-legal from an external composition crate.
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct LangProgram(
            ::lichen_highlevel::program::ProgramImpl<LangValue, LangOperator, $crate::program::LangAttr>,
        );

        // Spelled out so the inner `ProgramImpl` need not itself be a `Program`.
        impl ::lichen_lowlevel::Program for LangProgram {
            type Value = LangValue;
            type Operator = LangOperator;
            type GlobalExt = ::lichen_highlevel::program::HighGlobalExt;
            type PackageMeta = ::lichen_highlevel::program::HighPackageMeta;
        }

        impl ::lichen_highlevel::program::HighProgram for LangProgram {
            // Every composed program reuses the language crate's own `LangAttr`.
            type Attr = $crate::program::LangAttr;
            type Literal = ::lichen_highlevel::program::HighProgramLiteral;
        }

        // ── The runtime wiring: `ValueExt` on the union, `OperatorExt` on operators.

        // Payload methods dispatch to the leaves, so an arena payload is relocated.
        impl ::lichen_lowlevel::ValueExt for LangValue {
            fn is_handle(&self) -> bool {
                match self {
                    $(
                        Self::$extra_v_name(value) => {
                            <$extra_v as ::lichen_lowlevel::ValueExt>::is_handle(value)
                        }
                    )*
                    _ => false,
                }
            }

            fn handle(&self) -> ::lichen_lowlevel::AnyHandle<[u8]> {
                match self {
                    $(
                        Self::$extra_v_name(value) => {
                            <$extra_v as ::lichen_lowlevel::ValueExt>::handle(value)
                        }
                    )*
                    _ => unreachable!(
                        "only a value whose ValueExt::is_handle is true carries a payload"
                    ),
                }
            }

            /// The leaf list may be empty, so the payload may be unused.
            #[allow(unused_variables)]
            fn set_handle(&mut self, payload: ::lichen_lowlevel::AnyHandle<[u8]>) {
                match self {
                    $(
                        Self::$extra_v_name(value) => {
                            <$extra_v as ::lichen_lowlevel::ValueExt>::set_handle(value, payload)
                        }
                    )*
                    _ => unreachable!(
                        "only a value whose ValueExt::is_handle is true carries a payload"
                    ),
                }
            }

            fn alignment() -> usize {
                // One alignment serves the whole vocabulary: the strictest leaf.
                1 $( .max(<$extra_v as ::lichen_lowlevel::ValueExt>::alignment()) )*
            }

            // Every leaf is asked. `traced` must be forwarded, or a leaf's
            // report is lost.
            fn traced(
                &self,
                context: &dyn ::lichen_lowlevel::TraceContext,
                out: &mut Vec<::lichen_lowlevel::NodeId>,
            ) {
                let _ = (&context, &out);
                match self {
                    $(
                        Self::$extra_v_name(value) => {
                            <$extra_v as ::lichen_lowlevel::ValueExt>::traced(value, context, out)
                        }
                    )*
                    _ => {}
                }
            }

            fn release_obligations(
                &self,
                out: &mut Vec<Box<dyn ::lichen_lowlevel::Release>>,
            ) {
                let _ = &out;
                match self {
                    $(
                        Self::$extra_v_name(value) => {
                            <$extra_v as ::lichen_lowlevel::ValueExt>::release_obligations(
                                value, out,
                            )
                        }
                    )*
                    _ => {}
                }
            }
        }

        // Only the nominal-id methods and the open marker predicate are spelled out.
        impl ::lichen_highlevel::program::ValueType for LangValue {
            fn type_id(&self) -> Option<usize> {
                match self {
                    Self::$tyv_name(inner) => inner.as_type_id(),
                    _ => None,
                }
            }
            fn type_id_value(n: usize) -> Self {
                Self::$tyv_name(<$tyv>::TypeId(n))
            }
            fn is_kind_marker(&self) -> bool {
                match self {
                    // Every `TypeValue` variant but `TypeId` is a kind marker.
                    Self::$tyv_name(inner) => inner.as_type_id().is_none(),
                    $(
                        Self::$extra_v_name(inner) => {
                            < $extra_v as ::lichen_highlevel::program::LeafKindMarkers >
                                ::is_kind_marker(inner)
                        }
                    )*
                    _ => false,
                }
            }
        }

        // Each leaf dispatches to itself; `is_callable` is the OR of the
        // extension leaves' policies.
        impl ::lichen_lowlevel::OperatorExt<LangProgram> for LangOperator {
            fn run(
                &self,
                operand: <LangProgram as ::lichen_lowlevel::Program>::Value,
                block: ::lichen_lowlevel::BlockId,
                module: &mut ::lichen_lowlevel::Module<LangProgram>,
            ) -> Option<<LangProgram as ::lichen_lowlevel::Program>::Value> {
                match self {
                    LangOperator::$lowop_name(op) => op.run(operand, block, module),
                    LangOperator::$tyop_name(op) => op.run(operand, block, module),
                    $( LangOperator::$extra_op_name(op) => op.run(operand, block, module), )*
                }
            }
            fn low_type(
                &self,
                arguments: &[Option<::lichen_lowlevel::LowShape>],
            ) -> Option<::lichen_lowlevel::LowShape> {
                // The impl is named: no `P`-typed argument fixes which program.
                match self {
                    // The structural leaves' transfers are owned by the pass
                    // itself, so the hook is never consulted for them.
                    LangOperator::$lowop_name(_) => None,
                    LangOperator::$tyop_name(op) => {
                        <$tyop as ::lichen_lowlevel::OperatorExt<LangProgram>>::low_type(op, arguments)
                    }
                    $( LangOperator::$extra_op_name(op) => {
                        <$extra_op as ::lichen_lowlevel::OperatorExt<LangProgram>>::low_type(op, arguments)
                    } )*
                }
            }

            fn is_callable(
                module: &::lichen_lowlevel::Module<LangProgram>,
                callee: ::lichen_lowlevel::AnyNodeId,
            ) -> bool {
                false $( || <$extra_op as ::lichen_lowlevel::OperatorExt<LangProgram>>::is_callable(module, callee) )*
            }
        }

        // ── The per-leaf artifact codec: a leaf-name tag, then the payload.
        #[derive(Default)]
        pub struct ProgramCodec;

        // Bind the codec so tooling reads `P::Codec`, not a second generic.
        impl $crate::persist::ProgramCodecOf for LangProgram {
            type Codec = ProgramCodec;
        }

        impl $crate::persist::ArtifactCodec<LangProgram> for ProgramCodec {
            const PERSISTENT: bool = true;

            fn write_value(
                w: &mut $crate::persist::Writer,
                value: LangValue,
                modules: &std::collections::HashMap<
                    ::lichen_lowlevel::ModuleKey,
                    std::sync::Arc<::lichen_lowlevel::StaticModule<LangProgram>>,
                >,
            ) -> Result<(), String> {
                if let Some(v) = <LangValue as ::lichen_utils::extend::AsEnum<$low>>::as_enum(&value)
                {
                    w.leaf(stringify!($low_name))?;
                    <$low as ::lichen_lowlevel::codec::ValueCodec>::write_value(w, v, modules)?;
                    return Ok(());
                }
                if let Some(v) = <LangValue as ::lichen_utils::extend::AsEnum<$tyv>>::as_enum(&value)
                {
                    w.leaf(stringify!($tyv_name))?;
                    <$tyv as ::lichen_lowlevel::codec::ValueCodec>::write_value(w, v, modules)?;
                    return Ok(());
                }
                $(
                    if let Some(v) =
                        <LangValue as ::lichen_utils::extend::AsEnum<$extra_v>>::as_enum(&value)
                    {
                        w.leaf(stringify!($extra_v_name))?;
                        <$extra_v as ::lichen_lowlevel::codec::ValueCodec>::write_value(
                            w, v, modules,
                        )?;
                        return Ok(());
                    }
                )*
                unreachable!("a composed value always carries a leaf")
            }

            fn read_value(
                r: &mut $crate::persist::Reader<'_>,
                self_key: ::lichen_lowlevel::ModuleKey,
                self_arena: &[u8],
                self_base: *const u8,
                modules: &std::collections::HashMap<
                    ::lichen_lowlevel::ModuleKey,
                    std::sync::Arc<::lichen_lowlevel::StaticModule<LangProgram>>,
                >,
            ) -> Result<LangValue, String> {
                let name = r.leaf_name()?;
                if name == stringify!($low_name).as_bytes() {
                    return Ok(LangValue::$low_name(
                        <$low as ::lichen_lowlevel::codec::ValueCodec>::read_value(
                            r,
                            self_key,
                            self_arena,
                            self_base,
                            modules,
                        )?,
                    ));
                }
                if name == stringify!($tyv_name).as_bytes() {
                    return Ok(LangValue::$tyv_name(
                        <$tyv as ::lichen_lowlevel::codec::ValueCodec>::read_value(
                            r,
                            self_key,
                            self_arena,
                            self_base,
                            modules,
                        )?,
                    ));
                }
                $(
                    if name == stringify!($extra_v_name).as_bytes() {
                        return Ok(LangValue::$extra_v_name(
                            <$extra_v as ::lichen_lowlevel::codec::ValueCodec>::read_value(
                                r,
                                self_key,
                                self_arena,
                                self_base,
                                modules,
                            )?,
                        ));
                    }
                )*
                Err(format!(
                    "unknown value leaf '{}'",
                    String::from_utf8_lossy(name)
                ))
            }

            fn write_operator(
                w: &mut $crate::persist::Writer,
                operator: LangOperator,
            ) -> Result<(), String> {
                if let Some(op) =
                    <LangOperator as ::lichen_utils::extend::AsEnum<$lowop>>::as_enum(&operator)
                {
                    w.leaf(stringify!($lowop_name))?;
                    <$lowop as ::lichen_lowlevel::codec::OperatorCodec>::write_operator(w, op)?;
                    return Ok(());
                }
                if let Some(op) =
                    <LangOperator as ::lichen_utils::extend::AsEnum<$tyop>>::as_enum(&operator)
                {
                    w.leaf(stringify!($tyop_name))?;
                    <$tyop as ::lichen_lowlevel::codec::OperatorCodec>::write_operator(w, op)?;
                    return Ok(());
                }
                $(
                    if let Some(op) =
                        <LangOperator as ::lichen_utils::extend::AsEnum<$extra_op>>::as_enum(&operator)
                    {
                        w.leaf(stringify!($extra_op_name))?;
                        <$extra_op as ::lichen_lowlevel::codec::OperatorCodec>::write_operator(w, op)?;
                        return Ok(());
                    }
                )*
                unreachable!("a composed operator always carries a leaf")
            }

            fn read_operator(r: &mut $crate::persist::Reader<'_>) -> Result<LangOperator, String> {
                let name = r.leaf_name()?;
                if name == stringify!($lowop_name).as_bytes() {
                    return Ok(LangOperator::$lowop_name(
                        <$lowop as ::lichen_lowlevel::codec::OperatorCodec>::read_operator(r)?,
                    ));
                }
                if name == stringify!($tyop_name).as_bytes() {
                    return Ok(LangOperator::$tyop_name(
                        <$tyop as ::lichen_lowlevel::codec::OperatorCodec>::read_operator(r)?,
                    ));
                }
                $(
                    if name == stringify!($extra_op_name).as_bytes() {
                        return Ok(LangOperator::$extra_op_name(
                            <$extra_op as ::lichen_lowlevel::codec::OperatorCodec>::read_operator(
                                r,
                            )?,
                        ));
                    }
                )*
                Err(format!(
                    "unknown operator leaf '{}'",
                    String::from_utf8_lossy(name)
                ))
            }
        }
    };

    // Thread the next plugin's leaf macro, passing the accumulator.
    (
        @run
        [ $( $oa:tt )* ] [ $( $va:tt )* ] [ $( $aa:tt )* ] [ $( $b:tt )* ];
        [ $plugin:ident as $leaves:ident ; $( $rest:tt )* ];
    ) => {
        $plugin::$leaves! {
            $crate::lang_compose_vocabulary,
            [ $( $oa )* ] [ $( $va )* ] [ $( $aa )* ] [ $( $b )* ] ; [ $( $rest )* ] ;
        }
    };

    // Absorb one plugin's leaf fragment into the accumulator and recurse.
    (
        @absorb
        ( operators: [ $( $o:path as $on:ident ; )* ]; values: [ $( $v:path as $vn:ident ; )* ]; attrs: [ $( $a:path as $an:ident ; )* ];
          [ $( $oa:tt )* ] [ $( $va:tt )* ] [ $( $aa:tt )* ] [ $( $b:tt )* ] ; [ $( $rest:tt )* ] ; )
    ) => {
        $crate::lang_compose_vocabulary! {
            @run
            [ $( $oa )* $( $o as $on ; )* ] [ $( $va )* $( $v as $vn ; )* ] [ $( $aa )* $( $a as $an ; )* ] [ $( $b )* ] ;
            [ $( $rest )* ] ;
        }
    };

}

// The shipping compiler's plugin set: the one manifest that fixes it.
crate::lang_compose_vocabulary! {
    attrs = [
        Perspective as Perspective;
        Doc as Doc;
        Refinement as Refinement;
    ]
    // Each attribute's `AttrExt` lists the operator bounds it needs.
    [ P::Operator: From<GcdOp> + From<LowOperator> ];
    values = [
        LowValue as LowValue;
        TypeValue as TypeValue;
        lichen_compute::ComputeValue as ComputeValue;
    ];
    operators = [
        LowOperator as LowOperator;
        TypeOperator as TypeOperator;
        GcdOp as GcdOp;
        lichen_compute::ComputeOperator as ComputeOperator;
    ];
}

#[cfg(test)]
#[path = "tests/program_tests.rs"]
mod tests;

/// A probe plugin for the `plugins = [...]` arm: no leaves, threads the accumulator.
#[cfg(test)]
#[macro_export]
macro_rules! ic_probe_leaves {
    ($next:path, [ $($oa:tt)* ][ $($va:tt)* ][ $($aa:tt)* ][ $($b:tt)* ] ; [ $($rest:tt)* ] ;) => {
        $next! {
            @absorb (
                operators: [ ];
                values: [ ];
                attrs: [ ];
                [ $($oa)* ][ $($va)* ][ $($aa)* ][ $($b)* ] ; [ $($rest)* ] ;
            )
        }
    };
}

#[cfg(test)]
mod ic_probe_plugin {
    pub use crate::ic_probe_leaves as liche_leaves;
}

#[cfg(test)]
mod plugins_arm_tests {
    #![allow(dead_code)]
    use super::ic_probe_plugin;

    lang_compose_vocabulary! {
        attrs = [
            lichen_perspective::Perspective as Perspective;
            lichen_doc::Doc as Doc;
        ]
        [ P::Operator: From<lichen_perspective::GcdOp> ];
        values = [
            lichen_lowlevel::LowValue as LowValue;
            lichen_highlevel::program::TypeValue as TypeValue;
        ];
        operators = [
            lichen_lowlevel::LowOperator as LowOperator;
            lichen_highlevel::program::TypeOperator as TypeOperator;
            lichen_perspective::GcdOp as GcdOp;
        ];
        plugins = [ ic_probe_plugin as liche_leaves; ic_probe_plugin as liche_leaves; ];
    }

    #[test]
    fn the_plugins_arm_threads_a_plugin_set() {
        // Both probe plugins were absorbed: the composed enums exist.
        let _ = std::any::type_name::<LangValue>();
        let _ = std::any::type_name::<LangOperator>();
    }
}

#[cfg(test)]
mod sort_op_tests {
    //! A composed plugin vocabulary is executable: run `SortOp` end-to-end.
    #![allow(dead_code)]
    use lichen_lowlevel::{AnyNodeId, ArrayItem, BlockId, LowValue, Module, OperatorExt};
    use lichen_std_native::SortOp;
    use lichen_utils::extend::AsEnum;

    lang_compose_vocabulary! {
        attrs = [
            lichen_perspective::Perspective as Perspective;
            lichen_doc::Doc as Doc;
        ]
        [ P::Operator: From<lichen_perspective::GcdOp> ];
        values = [
            lichen_lowlevel::LowValue as LowValue;
            lichen_highlevel::program::TypeValue as TypeValue;
        ];
        operators = [
            lichen_lowlevel::LowOperator as LowOperator;
            lichen_highlevel::program::TypeOperator as TypeOperator;
        ];
        plugins = [ lichen_std_native as lichen_std_native_leaves; ];
    }

    #[test]
    fn sort_op_sorts_a_usize_array() {
        let mut module = Module::<LangProgram>::new();
        let block: BlockId = module.add_block(None);
        let items: Vec<ArrayItem> = [3usize, 1, 2]
            .iter()
            .map(|&n| {
                let node = module.add_node(block, None, Some(LangValue::from(LowValue::USize(n))));
                ArrayItem::new(AnyNodeId::Dynamic(node))
            })
            .collect();
        let array = module.alloc_array(&items, block);
        let operand = LangValue::from(LowValue::Array(array));
        let out = LangOperator::SortOp(SortOp::Sort)
            .run(operand, block, &mut module)
            .expect("the sort extension decides for a concrete array operand");
        let Some(LowValue::Array(array)) = out.as_enum() else {
            panic!("Sort must yield a USize array");
        };
        // SAFETY: `array` is the payload of the value the sort extension returned,
        // allocated in a live block of this module.
        let sorted: Vec<usize> = unsafe { array.items() }
            .iter()
            .map(|item| {
                module
                    .node_value(item.node)
                    .and_then(|v| match v.as_enum() {
                        Some(LowValue::USize(n)) => Some(n),
                        _ => None,
                    })
                    .expect("each sorted element is a USize node")
            })
            .collect();
        assert_eq!(sorted, vec![1, 2, 3]);
    }
}
