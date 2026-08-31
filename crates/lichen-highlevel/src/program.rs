//! The highlevel's concrete lowlevel program.
//!
//! Each layer provides a plain enum of its own variants: the lowlevel's
//! [`LowValue`]/[`LowOperator`], the highlevel's type values
//! ([`TypeValue`]) and type-level operators ([`TypeOperator`]).  The
//! composed vocabularies [`HighProgramValue`] and [`HighProgramOperator`]
//! are flat unions — one `lichen_utils::enum_ext!` invocation carrying each
//! extension whole as one sibling variant — so the checker builds and
//! inspects every value and emits every operator without an `Ext` wrapper:
//! a structural value sits one carry variant down
//! (`HighProgramValue::LowValue(..)`), the highlevel's type values sit in
//! theirs, and nothing nests.

use std::marker::PhantomData;

use lichen_lowlevel::{
    BlockId, GlobalExt, LowOperator, LowValue, Module, OperatorExt, Program, ValueExt,
};
use lichen_utils::compose::AsField;
use lichen_utils::extend::AsEnum;

/// The fresh-nominal-type-id state — one extension component of
/// [`HighGlobalExt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HighGlobal {
    /// The next nominal type id — [`HighProgramOperator::Fresh`] reads and
    /// increments it, so each call yields a distinct
    /// [`HighProgramValue::TypeId`].
    pub type_id_counter: usize,
}

impl HighGlobal {
    /// Consume the next nominal type id: read the counter, increment it, and
    /// return the previous value.
    pub fn next_type_id(&mut self) -> usize {
        let id = self.type_id_counter;
        self.type_id_counter += 1;
        id
    }
}

lichen_utils::compose_ext! {
    /// The highlevel's global extension state, injected into the lowlevel
    /// [`Module`]'s `global_ext` slot and threaded through the extension
    /// operators.
    ///
    /// It is a *tuple* host built by [`lichen_utils::compose_ext!`] over its
    /// extension components — a downstream composes more components by adding
    /// their types to this tuple and reads or mutates each one through
    /// [`lichen_utils::compose::AsField`] and the component's own methods (no
    /// per-component accessor trait).
    ///
    /// ```
    /// use lichen_highlevel::program::{HighGlobal, HighGlobalExt};
    /// use lichen_utils::compose::AsField;
    ///
    /// let mut ext = HighGlobalExt::default();
    /// assert_eq!(
    ///     AsField::<HighGlobal>::get_mut(&mut ext).next_type_id(),
    ///     0
    /// );
    /// assert_eq!(
    ///     AsField::<HighGlobal>::get_mut(&mut ext).next_type_id(),
    ///     1
    /// );
    /// assert_eq!(AsField::<HighGlobal>::get(&ext).type_id_counter, 2);
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct HighGlobalExt(
        HighGlobal,
    );
}
impl GlobalExt for HighGlobalExt {}

/// Per-package metadata for the highlevel `Program`.  The package layer
/// stores the single exported `[value, type]` pair ref here; the lowlevel's
/// [`Package`] only carries this as an opaque `Default` slot, keeping
/// highlevel concepts out of the lowlevel registry machinery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HighPackageMeta {
    /// The package's exported final-expression pair, filled by the language
    /// package store after freezing.  `None` until a higher layer records it.
    pub export: Option<lichen_lowlevel::StaticNodeId>,
}

/// The highlevel's own value extension — a plain enum of the type constants,
/// provided whole for the compositions below (and for a language crate
/// composing its own vocabulary from [`LowValue`] + this).
///
/// Every variant is a *type constant*: its own type is the canonical
/// universe (`Type : Type`), which makes the composed vocabulary's
/// `ValueType::type_of` a one-arm answer for this whole branch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TypeValue {
    /// The `int` type constant.
    TypeInt,
    /// The `Type` constant — the canonical universe node itself
    /// (`Type : Type`).
    TypeType,
    /// The kind marker of function type expressions — the pair's second
    /// element is a `Function` value.
    TypeFunction,
    /// The kind marker of tuple type expressions — the shape is the
    /// element-type list.
    TypeTuple,
    /// The kind marker of array type expressions — the shape is
    /// `[element type, length]`.
    TypeArray,
    /// The kind marker of struct type expressions — the shape is
    /// `[TypeId(n), fields_types_array]`: the nominal id bundled with
    /// the positional field-type list.
    TypeStruct,
    /// The kind marker of table type expressions — the shape is
    /// `[key type, value type]`.
    TypeTable,
    /// A nominal type id — a struct type's identity marker, living at
    /// `shape[0]` of a `TypeStruct`-kinded pair.  Equal ids unify,
    /// different ids don't (nominal identity), and an id never unifies
    /// with the structural markers above.
    TypeId(usize),
}

// The highlevel program's value vocabulary: a flat union of the lowlevel
// structural values and the highlevel type values, each carried whole as
// one sibling variant — one `lichen_utils::enum_ext!` invocation listing
// every layer's enum.  A language crate composes its own vocabulary the
// same way: `+ LowValue as LowValue; + TypeValue as TypeValue;` plus its
// own variants.
lichen_utils::enum_ext! {
    /// The highlevel program's value vocabulary: the lowlevel structural
    /// values and the highlevel type values, as sibling carry variants.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum HighProgramValue {
    }
    + LowValue as LowValue;
    + TypeValue as TypeValue;
}

impl ValueExt for HighProgramValue {
    fn is_handle(&self) -> bool {
        false
    }
}

/// The value→type contract a value vocabulary must satisfy to flow through
/// the checker: the type-constant markers it installs, the value→type
/// mapping for constants, and the kind classification the checker's
/// structural type checks dispatch on.  Every value union — the highlevel's
/// own [`HighProgramValue`] or an extended one — implements this; the
/// checker is generic over it.
pub trait ValueType: ValueExt + From<LowValue> + AsEnum<LowValue> + Clone {
    /// The `int` type marker — `USize` literals pair with `[Self::int_marker(), K]`.
    fn int_marker() -> Self;
    /// The `Type` marker — the canonical universe node itself (`Type : Type`).
    fn type_marker() -> Self;
    /// The kind marker of function type expressions.
    fn function_type_marker() -> Self;
    /// The kind marker of tuple type expressions.
    fn tuple_type_marker() -> Self;
    /// The kind marker of array type expressions.
    fn array_type_marker() -> Self;
    /// The kind marker of struct type expressions — the shape is
    /// `[TypeId(n), fields_types_array]`.
    fn type_struct_marker() -> Self;
    /// The kind marker of table type expressions — the shape is
    /// `[key type, value type]`.
    fn table_type_marker() -> Self;
    /// The value→type mapping: what type this value, used as a constant,
    /// pairs with.  `USize(_)` → the int marker; every type constant → the
    /// `Type` marker.  The checker only asks for constants (an int literal
    /// or a type value).
    fn type_of(&self) -> Self;
    /// The nominal id of a struct type value, if this is one.
    fn type_id(&self) -> Option<usize>;
    /// A nominal type id value — what the checker's `Fresh` operator yields.
    fn type_id_value(n: usize) -> Self;
}

impl ValueType for HighProgramValue {
    fn int_marker() -> Self {
        Self::TypeValue(TypeValue::TypeInt)
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
    fn type_of(&self) -> Self {
        match self {
            // Every type constant — the whole TypeValue branch — has the
            // canonical universe as its type.
            Self::TypeValue(_) => Self::type_marker(),
            // Int literals are structural values.
            Self::LowValue(LowValue::USize(_)) => Self::int_marker(),
            // Array, Function, None, Parameterized are built by other
            // expression kinds — the checker never asks their type.
            Self::LowValue(_) => {
                unreachable!("a structural non-USize value is not a constant")
            }
        }
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
}

// The highlevel's own operator extension — a plain enum of the type-level
// computations that have no structural operator form, provided whole for the
// composition below.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TypeOperator {
    /// A fresh nominal type id: each call reads and increments
    /// [`HighGlobal::next_type_id`] and returns a `TypeId(n)` type value.
    /// Nullary — the checker emits it with no operand, so it fires once per
    /// source occurrence and the cached value is reused wherever the struct
    /// type it tags is referenced.
    Fresh,
    /// Binary integer operators: `Add`/`Sub` compute; `Leq`/`Eq` compare
    /// and yield `USize(0/1)` — no `Bool` value exists, the comparison
    /// result drives the lazy `Index` branch of an `if` directly.
    ///
    /// Operand: `[left, right]`.  The lowlevel deep-evaluates the operand
    /// and gates on its parameterized subtree before calling `run`, so an
    /// unbound operand (a template parameter during the definition pass)
    /// is already the lazy marker; the checker pins both operand types to
    /// `Int`, so a wrong-shape operand here is an invariant violation, not
    /// a user error.
    Add,
    Sub,
    Leq,
    Eq,
}

// The highlevel program's operator vocabulary: a flat union of the
// structural [`LowOperator`] and the highlevel type-level
// [`TypeOperator`], each carried whole as one sibling variant.
// [`HighProgram`]'s second type parameter lets a downstream that needs more
// operators compose its own union over these same leaves and still reuse the
// lowlevel runtime/registry machinery.
lichen_utils::enum_ext! {
    /// The highlevel program's operator vocabulary: the structural and
    /// type-level operators, as sibling carry variants.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum HighProgramOperator {
    }
    + LowOperator as LowOperator;
    + TypeOperator as TypeOperator;
}

impl<V: ValueType> OperatorExt<HighProgram<V, HighProgramOperator>> for HighProgramOperator {
    fn run(&self, operand: V, _block: BlockId, module: &mut Module<HighProgram<V, HighProgramOperator>>) -> V {
        match self {
            // The structural operators never reach `run`: the VM dispatches
            // them through `AsEnum` before falling through.
            HighProgramOperator::LowOperator(_) => {
                unreachable!("structural operators are dispatched by the VM")
            }
            HighProgramOperator::TypeOperator(TypeOperator::Fresh) => {
                let id = AsField::<HighGlobal>::get_mut(&mut module.global_ext).next_type_id();
                V::type_id_value(id)
            }
            HighProgramOperator::TypeOperator(
                TypeOperator::Add | TypeOperator::Sub | TypeOperator::Leq | TypeOperator::Eq,
            ) => {
                // The VM already deep-evaluates the operand and gates on its
                // parameterized subtree, so an unbound operand is the lazy
                // marker (the definition pass flags the node).
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return V::from(LowValue::Parameterized);
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
                    return V::from(LowValue::Parameterized);
                };
                let Some(right) = module
                    .node_value(operands[1].node)
                    .and_then(|value| value.as_enum())
                    .and_then(|value| match value {
                        LowValue::USize(n) => Some(n),
                        _ => None,
                    })
                else {
                    return V::from(LowValue::Parameterized);
                };
                match self {
                    HighProgramOperator::TypeOperator(TypeOperator::Add) => {
                        V::from(LowValue::USize(left.wrapping_add(right)))
                    }
                    HighProgramOperator::TypeOperator(TypeOperator::Sub) => {
                        V::from(LowValue::USize(left.wrapping_sub(right)))
                    }
                    HighProgramOperator::TypeOperator(TypeOperator::Leq) => {
                        V::from(LowValue::USize((left <= right) as usize))
                    }
                    HighProgramOperator::TypeOperator(TypeOperator::Eq) => {
                        V::from(LowValue::USize((left == right) as usize))
                    }
                    _ => unreachable!("all binary operators are handled above"),
                }
            }
        }
    }
}

/// The highlevel's concrete lowlevel program: a marker generic over the
/// value vocabulary and the operator vocabulary.
///
/// The default [`HighProgramOperator`] is what the checked highlevel builder
/// emits.  A downstream that needs additional lowlevel operators can compose
/// its own operator enum with `lichen_utils::enum_ext!` (carrying
/// [`LowOperator`] and [`TypeOperator`] as siblings) and use
/// `Module<HighProgram<V, MyOperator>>`; the checker itself is still tied to
/// [`HighProgramOperator`], but the runtime/static-module/registry machinery
/// is now reusable with the extended operator set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighProgram<
    V: ValueType = HighProgramValue,
    O: std::fmt::Debug + Copy + PartialEq = HighProgramOperator,
>(PhantomData<(V, O)>);

impl<V, O> Program for HighProgram<V, O>
where
    V: ValueType,
    O: lichen_lowlevel::OperatorExt<HighProgram<V, O>>
        + From<LowOperator>
        + lichen_utils::extend::AsEnum<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq,
{
    type Value = V;
    type Operator = O;
    type GlobalExt = HighGlobalExt;
    type PackageMeta = HighPackageMeta;
}
