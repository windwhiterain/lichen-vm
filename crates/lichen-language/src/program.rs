//! The language's concrete program: the highlevel's value vocabulary and
//! attribute/operator extension, composed into one [`LangProgram`].
//!
//! The highlevel is attribute-agnostic — it names only the *shape* of "an
//! attribute combines over children" — so the concrete pieces are composed
//! here from the plugin set.  The **perspective compiler plugin**
//! (`lichen-perspective`) supplies the [`Perspective`] attribute (a
//! divisibility lattice) and its combine operator [`GcdOp::Gcd`] (an n-ary gcd
//! meet); the **doc compiler plugin** (`lichen-doc`) supplies the [`Doc`]
//! attribute (a label that attaches struct metadata); the **`lichen-compute`
//! native plugin** supplies the `ComputeValue`/`ComputeOperator` leaves.  This
//! module re-exports those leaves and composes them with the highlevel's
//! `LowValue`/`TypeValue`/`LowOperator`/`TypeOperator` leaves into one flat
//! vocabulary via the [`lang_compose_vocabulary!`] manifest.
//!
//! [`LangProgram`] is the program marker the whole frontend checks with — the
//! `P` of `Module<P>`/`Registry<P>`/`Checker<P>`, with `Value = LangValue`,
//! `Operator = LangOperator`, and `Attr = LangAttr` (`Perspective` + `Doc`).

use lichen_highlevel::program::{TypeOperator, TypeValue};
use lichen_lowlevel::{LowOperator, LowValue};

pub use lichen_perspective::{GcdOp, Perspective, divides, gcd, persp_attr_ext};

use lichen_doc::Doc;
pub use lichen_doc::doc_attr_ext;

/// Compose the language's concrete program marker from a manifest of its
/// vocabulary leaves and attribute set.
///
/// This is the single place the plugin set is declared: the value and operator
/// leaves list each plugin's contribution (a compiler plugin's attribute
/// operator like [`GcdOp`], a native plugin's operators/values —
/// [`lichen_compute::ComputeOperator`]/[`lichen_compute::ComputeValue`] —
/// alongside the core lowlevel/highlevel leaves), and `attrs` names the
/// language's attributes.  Each attribute is a compiler plugin: a marker
/// implementing [`lichen_highlevel::attr::AttrSpec`] plus an
/// [`lichen_highlevel::attr::AttrExt`] impl, listed here as
/// `<Marker> as <VariantName>`.  The trailing `[ … ]` holds any extra
/// where-clause bounds the composed [`lang_attr_ext`] registry needs to
/// instantiate the attributes' `AttrExt`s (e.g. `P::Operator: From<GcdOp>` for
/// a `Perspective` that emits a `Gcd` operator).  A package manager that
/// assembles a new compiler re-invokes this macro with a different plugin set;
/// the impls below (the checker/VM wiring) and the frontend are unchanged.
#[macro_export]
macro_rules! lang_compose_vocabulary {
    // With a plugin set (the package-manager-generated compiler): the shipping
    // leaves are spelled inline and each plugin contributes its own leaves via a
    // `[#macro_export] macro_rules! liche_leaves` the plugin crate exports.  The
    // `plugins = [<crate> as <leaves>; ...]` spells each plugin's crate path AND
    // the (uniquely-named) leaf macro it exports — a fixed name like
    // `liche_leaves` across several plugins collides in the extern prelude, so
    // each plugin's leaf macro has a distinct name.
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

    // Terminal: every plugin's leaves have been absorbed; emit the program.
    //
    // The operator and value lists are decomposed positionally: the first two
    // operator leaves must be the structural [`lichen_lowlevel::LowOperator`]
    // and the highlevel `TypeOperator` (named `LowOperator`/`TypeOperator`),
    // and the first two value leaves the structural [`lichen_lowlevel::LowValue`]
    // and the highlevel `TypeValue` (named `LowValue`/`TypeValue`).  These are
    // the language's core leaves, present in every composition; the generated
    // `ValueType`/`OperatorExt` impls reference them.
    (
        @run
        [ $lowop:path as $lowop_name:ident ; $tyop:path as $tyop_name:ident ; $( $extra_op:path as $extra_op_name:ident ; )* ]
        [ $low:path as $low_name:ident ; $tyv:path as $tyv_name:ident ; $( $extra_v:path as $extra_v_name:ident ; )* ]
        [ $( $attr:path as $attr_name:ident ; )* ] [ $( $bound:tt )* ];
        [ ] ;
    ) => {
        ::lichen_utils::enum_ext! {
            /// The language program's operator vocabulary: a flat union of the
            /// structural [`lichen_lowlevel::LowOperator`], the highlevel's
            /// `TypeOperator`, and each plugin's operators — one carry variant
            /// per extension.
            #[derive(Debug, Clone, Copy, PartialEq)]
            pub enum LangOperator {
            }
            + $lowop as $lowop_name ;
            + $tyop as $tyop_name ;
            $( + $extra_op as $extra_op_name ; )*
        }

        ::lichen_utils::enum_ext! {
            /// The language program's value vocabulary: a flat union of the
            /// lowlevel structural values, the highlevel type values, and each
            /// plugin's values.
            #[derive(Debug, Clone, Copy, PartialEq)]
            pub enum LangValue {
            }
            + $low as $low_name ;
            + $tyv as $tyv_name ;
            $( + $extra_v as $extra_v_name ; )*
        }

        /// The language's compile-time attributes, composed from the manifest:
        /// one variant per plugin marker, so a single program can carry any of
        /// them (an expression's schema tail holds one entry per attached
        /// attribute).  Each marker's behaviour lives in its own
        /// [`lichen_highlevel::attr::AttrExt`].
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum LangAttr {
            $( $attr_name($attr) ),*
        }

        impl ::lichen_highlevel::attr::AttrSpec for LangAttr {}

        /// The attribute-extension registry for the language's [`LangAttr`]:
        /// maps each composed marker to its behaviour, so the checker
        /// dispatches each attribute through its own semantics.
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

        /// The language's concrete program marker: `Value = LangValue`,
        /// `Operator = LangOperator`, `Attr = LangAttr`.
        pub type LangProgram = ::lichen_highlevel::program::ProgramImpl<LangValue, LangOperator, LangAttr>;

        // ── The runtime-wiring impls the composed program needs to be a
        //    `Program`/`HighProgram`: the structural value traits on the value
        //    union and the operator dispatch on the operator union.  These are
        //    what make a plugin-built compiler's vocabulary executable.

        // The composed values are structurally inert (handle payloads are the
        // lowlevel's own), so `is_handle` is always false.
        impl ::lichen_lowlevel::ValueExt for LangValue {
            fn is_handle(&self) -> bool {
                false
            }
        }

        // The type-constant markers all live in the core `TypeValue` leaf, so
        // the composed vocabulary delegates every marker to that leaf.  The
        // `<path>::Variant` qualified path bypasses the macro_rules rule that
        // a `$path:path` fragment cannot be followed directly by `::`.
        impl ::lichen_highlevel::program::ValueType for LangValue {
            fn int_marker() -> Self {
                Self::$tyv_name(<$tyv>::TypeInt)
            }
            fn string_marker() -> Self {
                Self::$tyv_name(<$tyv>::TypeString)
            }
            fn type_marker() -> Self {
                Self::$tyv_name(<$tyv>::TypeType)
            }
            fn function_type_marker() -> Self {
                Self::$tyv_name(<$tyv>::TypeFunction)
            }
            fn tuple_type_marker() -> Self {
                Self::$tyv_name(<$tyv>::TypeTuple)
            }
            fn array_type_marker() -> Self {
                Self::$tyv_name(<$tyv>::TypeArray)
            }
            fn type_struct_marker() -> Self {
                Self::$tyv_name(<$tyv>::TypeStruct)
            }
            fn table_type_marker() -> Self {
                Self::$tyv_name(<$tyv>::TypeTable)
            }
            fn type_id(&self) -> Option<usize> {
                match self {
                    Self::$tyv_name(inner) => inner.as_type_id(),
                    _ => None,
                }
            }
            fn type_id_value(n: usize) -> Self {
                Self::$tyv_name(<$tyv>::TypeId(n))
            }
            // A leaf re-heads the universe (e.g. `lichen-compute`'s
            // `TypeKernel`) to form a function-kind marker; delegate the
            // classification to each leaf so a plugin-built compiler's
            // renderer still spells those kinds as `in -> out`.
            fn is_function_kind(&self) -> bool {
                match self {
                    LangValue::$low_name(v) => {
                        <$low as ::lichen_utils::extend::FunctionKind>::is_function_kind(v)
                    }
                    LangValue::$tyv_name(v) => {
                        <$tyv as ::lichen_utils::extend::FunctionKind>::is_function_kind(v)
                    }
                    $(
                        LangValue::$extra_v_name(v) => {
                            <$extra_v as ::lichen_utils::extend::FunctionKind>::is_function_kind(v)
                        }
                    )*
                }
            }
        }

        // The operator union's `run` is a uniform dispatch: each leaf handles
        // itself (the structural lowlevel operator is unreachable — the VM
        // routes it through `AsEnum` first; the type operators run through
        // their own [`::lichen_lowlevel::OperatorExt`] impl; each plugin
        // operator runs its own).  This is the arm that lets a composed
        // program's operators actually execute.
        impl ::lichen_lowlevel::OperatorExt<LangProgram> for LangOperator {
            fn run(
                &self,
                operand: <LangProgram as ::lichen_lowlevel::Program>::Value,
                block: ::lichen_lowlevel::BlockId,
                module: &mut ::lichen_lowlevel::Module<LangProgram>,
            ) -> <LangProgram as ::lichen_lowlevel::Program>::Value {
                match self {
                    LangOperator::$lowop_name(op) => op.run(operand, block, module),
                    LangOperator::$tyop_name(op) => op.run(operand, block, module),
                    $( LangOperator::$extra_op_name(op) => op.run(operand, block, module), )*
                }
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

// The language program's value/operator vocabulary and program marker: a flat
// union of the structural [`LowOperator`]/[`LowValue`], the highlevel's
// [`TypeOperator`]/[`TypeValue`], the perspective compiler plugin's [`GcdOp`],
// and the `lichen-compute` native plugin's
// [`ComputeOperator`]/[`ComputeValue`].  This is the one manifest that fixes
// the compiler's plugin set.
crate::lang_compose_vocabulary! {
    attrs = [
        Perspective as Perspective;
        Doc as Doc;
    ]
    // A `Perspective` emits a `Gcd` operator, so its `AttrExt` needs the
    // operator bound; a `Doc` label needs none.
    [ P::Operator: From<GcdOp> ];
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

/// A probe plugin, used to exercise the `plugins = [...]` arm: it contributes
/// no leaves (its `liche_leaves!` hands back empty lists) but threads the
/// composition's accumulator, proving the tt-muncher composes a plugin set.
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
        // The `plugins = [...]` arm absorbed both probe plugins (threading the
        // tt-muncher accumulator): the composed enums exist.  The runtime
        // `ValueType`/`OperatorExt` impls for a composed set are the follow-up
        // tooling generalization, so this pins the composition at the type
        // level only.
        let _ = std::any::type_name::<LangValue>();
        let _ = std::any::type_name::<LangOperator>();
    }
}

#[cfg(test)]
mod sort_op_tests {
    //! Compose a program over the `lichen-std-native` native plugin and run its
    //! `SortOp` leaf end-to-end: the composition macro now generates the
    //! `ValueType`/`ValueExt`/`OperatorExt` impls, so a plugin vocabulary is a
    //! real, executable `Program` — this pins that a plugin-built compiler can
    //! actually *run* a plugin operator, not just type-check its composition.
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
        let out = LangOperator::SortOp(SortOp::Sort).run(operand, block, &mut module);
        let Some(LowValue::Array(array)) = out.as_enum() else {
            panic!("Sort must yield a USize array");
        };
        let sorted: Vec<usize> = array
            .items()
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
