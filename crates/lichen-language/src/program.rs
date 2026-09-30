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
//! `Operator = LangOperator`, and `Attr = LangAttr` (`Perspective` + `Doc`).  It
//! is a **local newtype** around [`ProgramImpl`], not a type alias, so the
//! `Program`/`HighProgram`/`ProgramCodecOf` wiring attached to it stays
//! orphan-legal from an external composition crate (a plugin-built compiler).

use lichen_highlevel::program::{TypeOperator, TypeValue};
use lichen_lowlevel::{LowOperator, LowValue};

pub use lichen_perspective::{GcdOp, Perspective, divides, gcd, persp_attr_ext};

use lichen_doc::Doc;
pub use lichen_doc::doc_attr_ext;

/// The position of the tokens counted so far, as a constant expression —
/// `macro_rules!` cannot add a metavariable, so the index of a manifest entry
/// is spelled as a sum of ones (the expansion is a literal expression, which
/// the const evaluator folds).  Machinery for
/// [`lang_compose_vocabulary!`], not a public API.
#[doc(hidden)]
#[macro_export]
macro_rules! lang_attr_position {
    () => { 0usize };
    ($head:tt $($rest:tt)*) => { 1usize + $crate::lang_attr_position!($($rest)*) };
}

/// The canonical-order index of `$value` over the manifest's attribute list,
/// as one `if let` level per entry (macro expansion cannot produce match arms,
/// so the chain is built as nested `if let`s and the innermost position — which
/// no entry can reach — diverges).  Machinery for
/// [`lang_compose_vocabulary!`], not a public API.
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

/// Compose the language's concrete program marker from a manifest of its
/// vocabulary leaves and attribute set.
///
/// This is the single place the plugin set is declared: the value and operator
/// leaves list each plugin's contribution (a compiler plugin's attribute
/// operator like [`GcdOp`], a native plugin's operators/values —
/// [`lichen_compute::ComputeOperator`]/[`lichen_compute::ComputeValue`] —
/// alongside the core lowlevel/highlevel leaves), and `attrs` names the
/// language's attributes **in the canonical attribute order** — the one
/// declaration that fixes the pair-slot layout (the attribute at position `i`
/// occupies pair slot `attr_slot(i)`).  Each attribute is a compiler plugin: a
/// marker implementing [`lichen_highlevel::attr::AttrSpec`] plus an
/// [`lichen_highlevel::attr::AttrExt`] impl, listed here as
/// `<Marker> as <VariantName>`.  The trailing `[ … ]` holds any extra
/// where-clause bounds the composed [`lang_attr_ext`] registry needs to
/// instantiate the attributes' `AttrExt`s (e.g. `P::Operator: From<GcdOp>` for
/// a `Perspective` that emits a `Gcd` operator).  A package manager that
/// assembles a new compiler re-invokes this macro with a different plugin set;
/// the impls below (the checker/VM wiring) and the frontend are unchanged.
///
/// **Adding an attribute is a one-list edit**: append its marker to `attrs`.
/// The order index, the `LANG_ATTR_ORDER` data, the build-time slot check and
/// (through them) the frontend tail order and the checker's slot merge are all
/// derived from that list, and the marker itself never names a slot number.
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
        ///
        /// The declaration order below **is** the canonical attribute order
        /// (see [`LANG_ATTR_ORDER`]): it is the single authority for the
        /// pair-slot layout, so nothing downstream keeps a second list.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum LangAttr {
            $( $attr_name($attr) ),*
        }

        impl ::lichen_highlevel::attr::AttrSpec for LangAttr {}

        impl LangAttr {
            /// This attribute's position in the canonical order, as a constant
            /// expression (each manifest entry owns exactly the index its
            /// position gives it, so the order is dense and collision-free by
            /// construction).
            pub const fn order_index_of(self) -> usize {
                $crate::lang_attr_order_index!(self, LangAttr; [] $( $attr_name ; )*)
            }
        }

        impl ::lichen_highlevel::attr::AttrSet for LangAttr {
            /// The canonical order as data — the pair layout itself.
            const ORDER: &'static [Self] = LANG_ATTR_ORDER;

            /// The attribute's index in the canonical order — the single
            /// authority for the pair-slot layout, so the pair slot of the
            /// `i`-th attribute is `attr_slot(i)`.
            fn order_index(&self) -> usize {
                self.order_index_of()
            }
        }

        /// The canonical attribute order: every composed attribute, in the
        /// order its pair slot follows (the attribute at index `i` occupies
        /// pair slot `attr_slot(i)`).  The frontend lays an annotation's
        /// schema tail out in this order and the checker sorts a merged tail
        /// into it, so the two can never disagree.
        pub const LANG_ATTR_ORDER: &[LangAttr] = &[ $( LangAttr::$attr_name($attr) ),* ];

        // The build-time slot check: every attribute's canonical index is its
        // position in the canonical order.  The assertion is what makes that
        // a *checked* property — a hand-edited index fails the build here
        // instead of silently mis-pairing a pair at runtime.
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
        ///
        /// This is a **local newtype** around the highlevel's
        /// [`::lichen_highlevel::program::ProgramImpl`] marker, not a type
        /// alias.  A composition site *inside* `lichen_language` may implement
        /// any trait for an alias of its own types, but an **external**
        /// composition site (a plugin-built compiler crate; `std_native.rs`'s
        /// `HostProgram`) cannot: writing `impl ProgramCodecOf for
        /// <alias-of-ProgramImpl>` is E0117, because the alias unwraps to a
        /// foreign `ProgramImpl` and the trait is foreign.  Making the marker
        /// a fresh nominal type fixes that — the `Program`/`HighProgram`/
        /// `ProgramCodecOf`/`OperatorExt` impls below are then orphan-legal
        /// from any crate that composes this vocabulary.
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct LangProgram(
            ::lichen_highlevel::program::ProgramImpl<LangValue, LangOperator, $crate::program::LangAttr>,
        );

        // The marker's `Program`/`HighProgram` wiring, delegating the
        // associated types to the inner [`::lichen_highlevel::program::ProgramImpl`]
        // it wraps.  They are spelled out rather than read through
        // `<Inner as Program>::…`, so this marker never requires the inner
        // `ProgramImpl` to itself be a `Program` (which would need a
        // `LangOperator: OperatorExt<Inner>` impl the composed vocabulary no
        // longer carries).
        impl ::lichen_lowlevel::Program for LangProgram {
            type Value = LangValue;
            type Operator = LangOperator;
            type GlobalExt = ::lichen_highlevel::program::HighGlobalExt;
            type PackageMeta = ::lichen_highlevel::program::HighPackageMeta;

            // The highlevel's unification deferral policy: a pending
            // field/positional read may merge with a class holding a type,
            // because "holds a type" is a fact about the pair encoding the
            // highlevel owns.  Wiring it here is what makes every composed
            // program — plugin-built ones included — inherit it; a program
            // that never states a policy keeps the lowlevel's honest default
            // (conflict).  See `lichen_highlevel::shape::defer_pending`.
            fn defer_pending(
                module: &mut ::lichen_lowlevel::Module<Self>,
                sides: &::lichen_lowlevel::PendingSides,
            ) -> Option<::lichen_lowlevel::Deferral> {
                ::lichen_highlevel::shape::defer_pending(module, sides)
            }
        }

        impl ::lichen_highlevel::program::HighProgram for LangProgram {
            // The attribute set is fixed by the language design (Perspective +
            // Doc), not by the composition — so every composed program reuses
            // the language crate's own `LangAttr`.  That is what lets a
            // plugin-built program satisfy `LangProgramShape`/the frontend's
            // `IR<program::LangAttr>` and drive the shared `cli`/`server`.
            type Attr = $crate::program::LangAttr;
            type Literal = ::lichen_highlevel::program::HighProgramLiteral;
        }

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
        // the trait's registry-derived default bodies
        // (`Self::from(TypeValue::$variant)`, over the composed `From` impl)
        // already provide every marker — the impl spells only the two
        // nominal-id methods, which have no default.  The
        // `<path>::Variant` qualified path bypasses the macro_rules rule that
        // a `$path:path` fragment cannot be followed directly by `::`.
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
        }

        // The operator union's `run` is a uniform dispatch: each leaf handles
        // itself (the structural lowlevel operator is unreachable — the VM
        // routes it through `AsEnum` first; the type operators run through
        // their program-generic [`::lichen_lowlevel::OperatorExt`] impl in the
        // highlevel; each plugin operator runs its own).  This is the arm that
        // lets a composed program's operators actually execute.
        //
        // `low_type` is the same uniform dispatch for the low-type pass: each
        // leaf states what its own computation produces, so a composed
        // program inherits its plugin's transfer without the lowlevel knowing
        // the vocabulary.
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
            fn low_type(
                &self,
                arguments: &[Option<::lichen_lowlevel::LowShape>],
            ) -> Option<::lichen_lowlevel::LowShape> {
                // Every leaf's transfer is a method of the program-generic
                // `OperatorExt` impl, and it mentions no `P`-typed argument, so
                // an unqualified call cannot infer *which* program's impl is
                // meant — the impl is named, once, as `LangProgram`.
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
        }

        // ── The per-leaf artifact codec for the composed vocabulary.
        //
        // `ProgramCodec` implements [`$crate::persist::ArtifactCodec`] by
        // dispatching each carry variant to its leaf's [`ValueCodec`]/
        // [`OperatorCodec`]: a leaf discriminator byte (the leaf's position in
        // the composition — `0`/`1` are the structural leaves, `2 3 …` each
        // plugin in order), then the leaf's own payload.  Self-consistent per
        // compiler; a plugin-built compiler's `cli` uses it for a real
        // `~/.lichen` device cache.
        #[derive(Default)]
        pub struct ProgramCodec;

        // Bind the program's codec into the associated-type collector, so the
        // tooling is generic over a single `P` and reads `P::Codec` rather than
        // threading the codec as a separate generic.
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
            ) {
                if let Some(v) = <LangValue as ::lichen_utils::extend::AsEnum<$low>>::as_enum(&value)
                {
                    w.leaf(stringify!($low_name));
                    <$low as ::lichen_lowlevel::codec::ValueCodec>::write_value(w, v, modules);
                    return;
                }
                if let Some(v) = <LangValue as ::lichen_utils::extend::AsEnum<$tyv>>::as_enum(&value)
                {
                    w.leaf(stringify!($tyv_name));
                    <$tyv as ::lichen_lowlevel::codec::ValueCodec>::write_value(w, v, modules);
                    return;
                }
                $(
                    if let Some(v) =
                        <LangValue as ::lichen_utils::extend::AsEnum<$extra_v>>::as_enum(&value)
                    {
                        w.leaf(stringify!($extra_v_name));
                        <$extra_v as ::lichen_lowlevel::codec::ValueCodec>::write_value(
                            w, v, modules,
                        );
                        return;
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

            fn write_operator(w: &mut $crate::persist::Writer, operator: LangOperator) {
                if let Some(op) =
                    <LangOperator as ::lichen_utils::extend::AsEnum<$lowop>>::as_enum(&operator)
                {
                    w.leaf(stringify!($lowop_name));
                    <$lowop as ::lichen_lowlevel::codec::OperatorCodec>::write_operator(w, op);
                    return;
                }
                if let Some(op) =
                    <LangOperator as ::lichen_utils::extend::AsEnum<$tyop>>::as_enum(&operator)
                {
                    w.leaf(stringify!($tyop_name));
                    <$tyop as ::lichen_lowlevel::codec::OperatorCodec>::write_operator(w, op);
                    return;
                }
                $(
                    if let Some(op) =
                        <LangOperator as ::lichen_utils::extend::AsEnum<$extra_op>>::as_enum(&operator)
                    {
                        w.leaf(stringify!($extra_op_name));
                        <$extra_op as ::lichen_lowlevel::codec::OperatorCodec>::write_operator(w, op);
                        return;
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
