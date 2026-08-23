//! The highlevel's concrete lowlevel program.
//!
//! The value universe is the lowlevel structural values (spliced in from
//! the lowlevel `LowValue` enum) together with the type values below — the
//! union [`HighProgramValue`] — and the operator universe is the lowlevel
//! `LowOperator` enum together with the type-level operators below — the
//! union [`HighProgramOperator`] — so the checker builds and inspects every
//! value and emits every operator without an `Ext` wrapper.

use std::marker::PhantomData;

use lichen_lowlevel::{
    BlockId, EvalError, FunctionId, LowValue, Module, NodeId, OperatorExt, Program, ValueExt,
};
use lichen_utils::extend::AsEnum;

/// The highlevel's global extension state, injected into the lowlevel
/// [`Module`]'s `global_ext` slot and threaded through the extension
/// operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HighGlobalExt {
    /// The next nominal type id — [`HighProgramOperator::Fresh`] reads and
    /// increments it, so each call yields a distinct
    /// [`HighProgramValue::TypeId`].
    pub type_id_counter: usize,
}

// The highlevel program's value vocabulary: the lowlevel structural values
// (spliced in from the lowlevel `LowValue` enum, so the lowlevel can
// inspect them through `AsEnum`) plus the type values below.  The
// `#[enum_ext]` makes this union itself an extension point: an extended
// vocabulary (a crate that adds variants) calls the generated
// `extend_HighProgramValue!` carrier, which re-splices these variants into
// its own flat union.
lichen_lowlevel::extend_LowValue! {
    /// The highlevel program's value vocabulary.
    #[derive(Debug, Clone, Copy, PartialEq)]
    #[lichen_extend::enum_ext]
    pub enum HighProgramValue {
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
        /// A nominal type id — the kind marker of a struct type.  Equal ids
        /// unify, different ids don't (nominal identity), and an id never
        /// unifies with the structural markers above.
        TypeId(usize),
    }
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
    /// The value→type mapping: what type this value, used as a constant,
    /// pairs with.  `USize(_)` → the int marker; every type constant → the
    /// `Type` marker.  The checker only asks for constants (an int literal
    /// or a type value).
    fn type_of(&self) -> Self;
    /// Whether this value is a compound-type kind marker (function, tuple,
    /// array) or a nominal type id — the kinding check's vocabulary.
    fn is_kind(&self) -> bool;
    /// The nominal id of a struct type value, if this is one.
    fn type_id(&self) -> Option<usize>;
    /// A nominal type id value — what the checker's `Fresh` operator yields.
    fn type_id_value(n: usize) -> Self;
}

impl ValueType for HighProgramValue {
    fn int_marker() -> Self {
        HighProgramValue::TypeInt
    }
    fn type_marker() -> Self {
        HighProgramValue::TypeType
    }
    fn function_type_marker() -> Self {
        HighProgramValue::TypeFunction
    }
    fn tuple_type_marker() -> Self {
        HighProgramValue::TypeTuple
    }
    fn array_type_marker() -> Self {
        HighProgramValue::TypeArray
    }
    fn type_of(&self) -> Self {
        match self {
            HighProgramValue::USize(_) => HighProgramValue::TypeInt,
            HighProgramValue::TypeInt
            | HighProgramValue::TypeType
            | HighProgramValue::TypeFunction
            | HighProgramValue::TypeTuple
            | HighProgramValue::TypeArray
            | HighProgramValue::TypeId(_) => HighProgramValue::TypeType,
            // Array, Function, None, Parameterized are built by other
            // expression kinds — the checker never asks their type.
            _ => unreachable!("a structural non-USize value is not a constant"),
        }
    }
    fn is_kind(&self) -> bool {
        matches!(
            self,
            HighProgramValue::TypeFunction
                | HighProgramValue::TypeTuple
                | HighProgramValue::TypeArray
                | HighProgramValue::TypeId(_)
        )
    }
    fn type_id(&self) -> Option<usize> {
        match self {
            HighProgramValue::TypeId(n) => Some(*n),
            _ => None,
        }
    }
    fn type_id_value(n: usize) -> Self {
        HighProgramValue::TypeId(n)
    }
}

// The extension operators the checker emits — the type-level computations
// that have no structural operator form.
lichen_lowlevel::extend_LowOperator! {
    /// The highlevel program's operator vocabulary: the structural
    /// [`LowOperator::Index`]/[`LowOperator::Apply`] spliced in from the
    /// lowlevel, plus the type-level operators below.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum HighProgramOperator {
        /// The type evaluation of an indexing expression `a[i]`.
        ///
        /// Operand: `[type_pair, index]` where `type_pair` is the indexed
        /// value's type expression.  A tuple type (`TypeTuple` kind) and a
        /// struct type (`TypeId` kind) select their element/field-type list
        /// position — a structural out of bounds.  An array type
        /// (`TypeArray` kind) checks the index against the *length* stored
        /// in its shape `[element_type, length]` — the check the structural
        /// `Index` cannot express (the shape holds the length as data, not
        /// as selectable positions).  Out of bounds records an
        /// [`EvalError`] and yields `HighProgramValue::None`.
        IndexType,
        /// A fresh nominal type id: each call reads and increments
        /// [`HighGlobalExt::type_id_counter`] and returns
        /// `HighProgramValue::TypeId(n)`.  Nullary — the checker emits it
        /// with no operand, so it fires once per source occurrence and the
        /// cached value is reused wherever the struct type it tags is
        /// referenced.
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
}

impl<V: ValueType> OperatorExt<HighProgram<V>> for HighProgramOperator {
    fn run(&self, operand: V, _block: BlockId, module: &mut Module<HighProgram<V>>) -> V {
        match self {
            // The structural operators never reach `run`: the VM dispatches
            // them through `AsEnum` before falling through.
            HighProgramOperator::Index | HighProgramOperator::Apply => {
                unreachable!("structural operators are dispatched by the VM")
            }
            HighProgramOperator::IndexType => {
                // The VM deep-evaluates the operand and gates on its
                // parameterized subtree before calling `run`, so an unbound
                // type or index has already been turned into the lazy marker.
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return V::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("IndexType expects an operand array of [type, index]")
                };
                let operands = unsafe { &*operands };
                let type_pair = operands[0];
                let index_node = operands[1];
                let Some(LowValue::USize(index)) = module.nodes[index_node]
                    .value
                    .expect("the operand was deep-evaluated")
                    .as_enum()
                else {
                    unreachable!("IndexType needs a USize index node")
                };
                // type_pair's value: [shape, [kind marker, K]].
                let Some(LowValue::Array(pair)) = module.nodes[type_pair]
                    .value
                    .expect("the operand was deep-evaluated")
                    .as_enum()
                else {
                    unreachable!("IndexType needs a type expression pair")
                };
                let pair = unsafe { &*pair };
                let shape = pair[0];
                let kind_cell = pair[1];
                let Some(LowValue::Array(kind)) = module.nodes[kind_cell]
                    .value
                    .expect("the operand was deep-evaluated")
                    .as_enum()
                else {
                    unreachable!("IndexType needs a kind expression")
                };
                let marker = unsafe { &*kind }[0];
                let marker_value = module.nodes[marker].value;
                if marker_value == Some(V::tuple_type_marker()) {
                    let Some(LowValue::Array(elements)) = module.nodes[shape]
                        .value
                        .expect("the operand was deep-evaluated")
                        .as_enum()
                    else {
                        unreachable!("a tuple type shape is its element-type list")
                    };
                    let elements = unsafe { &*elements };
                    if index < elements.len() {
                        module.nodes[elements[index]]
                            .value
                            .expect("the operand was deep-evaluated")
                    } else {
                        module.eval_errors.push(EvalError {
                            index: index_node,
                            index_value: index,
                            length: elements.len(),
                        });
                        V::from(LowValue::None)
                    }
                } else if marker_value == Some(V::array_type_marker()) {
                    let Some(LowValue::Array(shape_ids)) = module.nodes[shape]
                        .value
                        .expect("the operand was deep-evaluated")
                        .as_enum()
                    else {
                        unreachable!("an array type shape is [element type, length]")
                    };
                    let shape_ids = unsafe { &*shape_ids };
                    let element_type = shape_ids[0];
                    let Some(LowValue::USize(length)) = module.nodes[shape_ids[1]]
                        .value
                        .expect("the operand was deep-evaluated")
                        .as_enum()
                    else {
                        unreachable!("the ArrayType length must be a USize")
                    };
                    if index < length {
                        module.nodes[element_type]
                            .value
                            .expect("the operand was deep-evaluated")
                    } else {
                        module.eval_errors.push(EvalError {
                            index: index_node,
                            index_value: index,
                            length,
                        });
                        V::from(LowValue::None)
                    }
                } else if marker_value.is_some_and(|value| value.type_id().is_some()) {
                    // A struct type's shape is its positional field-type
                    // list — selecting an element is field access, exactly
                    // like a tuple type's element-type list.
                    let Some(LowValue::Array(elements)) = module.nodes[shape]
                        .value
                        .expect("the operand was deep-evaluated")
                        .as_enum()
                    else {
                        unreachable!("a struct type shape is its field-type list")
                    };
                    let elements = unsafe { &*elements };
                    if index < elements.len() {
                        module.nodes[elements[index]]
                            .value
                            .expect("the operand was deep-evaluated")
                    } else {
                        module.eval_errors.push(EvalError {
                            index: index_node,
                            index_value: index,
                            length: elements.len(),
                        });
                        V::from(LowValue::None)
                    }
                } else {
                    unreachable!("IndexType target must be a tuple, array, or struct type")
                }
            }
            HighProgramOperator::Fresh => {
                let id = module.global_ext.type_id_counter;
                module.global_ext.type_id_counter += 1;
                V::type_id_value(id)
            }
            HighProgramOperator::Add
            | HighProgramOperator::Sub
            | HighProgramOperator::Leq
            | HighProgramOperator::Eq => {
                // The VM already deep-evaluates the operand and gates on its
                // parameterized subtree, so an unbound operand is the lazy
                // marker (the definition pass flags the node).
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return V::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("binary operators expect an operand array of [left, right]")
                };
                let operands = unsafe { &*operands };
                // A non-USize operand is a *reported* type error, not an
                // invariant violation: the checker pins both operands to
                // `Int`, so a wrong shape only arrives here through an
                // argument unify that already failed (recording the
                // diagnostic) — stay lazy instead of panicking.
                let Some(LowValue::USize(left)) = module.nodes[operands[0]]
                    .value
                    .as_ref()
                    .and_then(|value| value.as_enum())
                else {
                    return V::from(LowValue::Parameterized);
                };
                let Some(LowValue::USize(right)) = module.nodes[operands[1]]
                    .value
                    .as_ref()
                    .and_then(|value| value.as_enum())
                else {
                    return V::from(LowValue::Parameterized);
                };
                match self {
                    HighProgramOperator::Add => V::from(LowValue::USize(left.wrapping_add(right))),
                    HighProgramOperator::Sub => V::from(LowValue::USize(left.wrapping_sub(right))),
                    HighProgramOperator::Leq => V::from(LowValue::USize((left <= right) as usize)),
                    HighProgramOperator::Eq => V::from(LowValue::USize((left == right) as usize)),
                    _ => unreachable!("all binary operators are handled above"),
                }
            }
        }
    }
}

/// The highlevel's concrete lowlevel program: a marker generic over the
/// value vocabulary (defaults to [`HighProgramValue`]), so the checker runs
/// on any union that implements [`ValueType`] — the operators and the global
/// extension state are the highlevel's own regardless of the value type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighProgram<V: ValueType = HighProgramValue>(PhantomData<V>);

impl<V: ValueType> Program for HighProgram<V> {
    type Value = V;
    type Operator = HighProgramOperator;
    type GlobalExt = HighGlobalExt;
}
