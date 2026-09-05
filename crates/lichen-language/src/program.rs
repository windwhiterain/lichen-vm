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

use lichen_highlevel::program::{HighGlobal, TypeOperator, TypeValue, ValueType};
use lichen_lowlevel::{LowOperator, LowValue, Module, OperatorExt, ValueExt};
use lichen_utils::compose::AsField;
use lichen_utils::extend::AsEnum;

pub use lichen_perspective::{GcdOp, Perspective, divides, gcd, persp_attr_ext};

use lichen_doc::Doc;
pub use lichen_doc::doc_attr_ext;

use lichen_compute::ComputeValue;

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
    // `[#macro_export] macro_rules! lichen_leaves` the plugin crate exports.
    (
        attrs = [ $( $attr:path as $attr_name:ident ; )* ] [ $( $bound:tt )* ];
        values = [ $( $value:path as $value_name:ident ; )* ];
        operators = [ $( $operator:path as $operator_name:ident ; )* ];
        plugins = [ $( $plugin:ident ; )* ];
    ) => {
        $crate::lang_compose_vocabulary! {
            @run
            [ $( $operator as $operator_name ; )* ] [ $( $value as $value_name ; )* ] [ $( $attr as $attr_name ; )* ] [ $( $bound )* ];
            [ $( $plugin ; )* ];
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
    (
        @run
        [ $( $operator:path as $operator_name:ident ; )* ] [ $( $value:path as $value_name:ident ; )* ] [ $( $attr:path as $attr_name:ident ; )* ] [ $( $bound:tt )* ];
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
            $( + $operator as $operator_name ; )*
        }

        ::lichen_utils::enum_ext! {
            /// The language program's value vocabulary: a flat union of the
            /// lowlevel structural values, the highlevel type values, and each
            /// plugin's values.
            #[derive(Debug, Clone, Copy, PartialEq)]
            pub enum LangValue {
            }
            $( + $value as $value_name ; )*
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
    };

    // Thread the next plugin's `liche_leaves!`, passing the accumulator.
    (
        @run
        [ $( $oa:tt )* ] [ $( $va:tt )* ] [ $( $aa:tt )* ] [ $( $b:tt )* ];
        [ $plugin:ident ; $( $rest:tt )* ];
    ) => {
        $plugin::liche_leaves! {
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

impl ValueExt for LangValue {
    fn is_handle(&self) -> bool {
        false
    }
}

// The compute vocabulary's only new type-constant marker is `TypeKernel`
// (a kernel's type mirror is `[signature, [TypeKernel, Type]]`); every other
// marker delegates to the highlevel type values.
impl ValueType for LangValue {
    fn int_marker() -> Self {
        Self::TypeValue(TypeValue::TypeInt)
    }
    fn string_marker() -> Self {
        Self::TypeValue(TypeValue::TypeString)
    }
    fn type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeType)
    }
    fn function_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeFunction)
    }
    fn tuple_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeTuple)
    }
    fn array_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeArray)
    }
    fn type_struct_marker() -> Self {
        Self::TypeValue(TypeValue::TypeStruct)
    }
    fn table_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeTable)
    }
    fn type_id(&self) -> Option<usize> {
        match self {
            Self::TypeValue(TypeValue::TypeId(n)) => Some(*n),
            _ => None,
        }
    }
    fn type_id_value(n: usize) -> Self {
        Self::TypeValue(TypeValue::TypeId(n))
    }
    fn is_function_kind(&self) -> bool {
        // `TypeKernel` re-heads the universe to form a kernel type
        // `[signature, [TypeKernel, Type]]`, which mirrors a function type —
        // the generic renderer spells that kind as `in -> out`.
        matches!(self, Self::ComputeValue(ComputeValue::TypeKernel))
    }
}

impl OperatorExt<LangProgram> for LangOperator {
    fn run(
        &self,
        operand: <LangProgram as lichen_lowlevel::Program>::Value,
        block: lichen_lowlevel::BlockId,
        module: &mut Module<LangProgram>,
    ) -> <LangProgram as lichen_lowlevel::Program>::Value {
        match self {
            // The structural operators never reach `run`: the VM dispatches
            // them through `AsEnum` before falling through.
            LangOperator::LowOperator(_) => {
                unreachable!("structural operators are dispatched by the VM")
            }
            LangOperator::TypeOperator(TypeOperator::Fresh) => {
                let id = AsField::<HighGlobal>::get_mut(&mut module.global_ext).next_type_id();
                LangValue::type_id_value(id)
            }
            LangOperator::TypeOperator(
                TypeOperator::Add | TypeOperator::Sub | TypeOperator::Leq | TypeOperator::Eq,
            ) => {
                // The VM already deep-evaluates the operand and gates on its
                // parameterized subtree, so an unbound operand is the lazy
                // marker (the definition pass flags the node).
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return LangValue::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("binary operators expect an operand array of [left, right]")
                };
                let operands = operands.items();
                // A non-USize operand is a *reported* type error, not an
                // invariant violation: the checker pins both operands to
                // `Int`, so a wrong shape only arrives here through an
                // argument unify that already failed (recording the
                // diagnostic) — stay lazy instead of panicking.
                let Some(left) = module
                    .node_value(operands[0].node)
                    .and_then(|value| value.as_enum())
                    .and_then(|value| match value {
                        LowValue::USize(n) => Some(n),
                        _ => None,
                    })
                else {
                    return LangValue::from(LowValue::Parameterized);
                };
                let Some(right) = module
                    .node_value(operands[1].node)
                    .and_then(|value| value.as_enum())
                    .and_then(|value| match value {
                        LowValue::USize(n) => Some(n),
                        _ => None,
                    })
                else {
                    return LangValue::from(LowValue::Parameterized);
                };
                match self {
                    LangOperator::TypeOperator(TypeOperator::Add) => {
                        LangValue::from(LowValue::USize(left.wrapping_add(right)))
                    }
                    LangOperator::TypeOperator(TypeOperator::Sub) => {
                        LangValue::from(LowValue::USize(left.wrapping_sub(right)))
                    }
                    LangOperator::TypeOperator(TypeOperator::Leq) => {
                        LangValue::from(LowValue::USize((left <= right) as usize))
                    }
                    LangOperator::TypeOperator(TypeOperator::Eq) => {
                        LangValue::from(LowValue::USize((left == right) as usize))
                    }
                    _ => unreachable!("all binary operators are handled above"),
                }
            }
            // The perspective compiler plugin's operator: dispatched by the
            // plugin's own `OperatorExt` impl.
            LangOperator::GcdOp(op) => op.run(operand, block, module),
            LangOperator::ComputeOperator(op) => op.run(operand, block, module),
        }
    }
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
        plugins = [ ic_probe_plugin; ic_probe_plugin; ];
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
