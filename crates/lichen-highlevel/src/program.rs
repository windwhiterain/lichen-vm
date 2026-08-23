//! The highlevel's concrete lowlevel program.
//!
//! The value universe is the lowlevel structural values (spliced in from
//! the lowlevel `LowValue` enum) together with the type values below — the
//! union [`HighProgramValue`] — so the checker builds and inspects every
//! value without an `Ext` wrapper.

use lichen_lowlevel::{
    BlockId, EvalError, FunctionId, Module, NodeId, OperatorExt, Program, ValueExt,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighProgram;

/// The highlevel's global extension state, injected into the lowlevel
/// [`Module`]'s `global_ext` slot and threaded through the extension
/// operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HighGlobalExt {
    /// The next nominal type id — [`HighOperator::Fresh`] reads and
    /// increments it, so each call yields a distinct
    /// [`HighProgramValue::TypeId`].
    pub type_id_counter: usize,
}

impl Program for HighProgram {
    type Value = HighProgramValue;
    type Operator = HighOperator;
    type GlobalExt = HighGlobalExt;
}

// The highlevel program's value vocabulary: the lowlevel structural values
// (spliced in from the lowlevel `LowValue` enum, so the lowlevel can
// inspect them through `AsEnum`) plus the type values below.
lichen_lowlevel::extend_LowValue! {
    /// The highlevel program's value vocabulary.
    #[derive(Debug, Clone, Copy, PartialEq)]
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

/// The extension operators the checker emits — the type-level computations
/// that have no structural operator form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HighOperator {
    /// The type evaluation of an indexing expression `a[i]`.
    ///
    /// Operand: `[type_pair, index]` where `type_pair` is the indexed
    /// value's type expression.  A tuple type (`TypeTuple` kind) and a
    /// struct type (`TypeId` kind) select their element/field-type list
    /// position — a structural out of bounds.  An array type (`ArrayType`
    /// kind) checks the index against the *length* stored in its shape
    /// `[element_type, length]` — the check the structural `Index` cannot
    /// express (the shape holds the length as data, not as selectable
    /// positions).  Out of bounds records an [`EvalError`] and yields
    /// `HighProgramValue::None`.
    IndexType,
    /// A fresh nominal type id: each call reads and increments
    /// [`HighGlobalExt::type_id_counter`] and returns
    /// `HighProgramValue::TypeId(n)`.  Nullary — the checker emits it with
    /// no operand, so it fires once per source occurrence and the cached
    /// value is reused wherever the struct type it tags is referenced.
    Fresh,
    /// Binary integer operators: `Add`/`Sub` compute; `Leq`/`Eq` compare and
    /// yield `USize(0/1)` — no `Bool` value exists, the comparison result
    /// drives the lazy `Index` branch of an `if` directly.
    ///
    /// Operand: `[left, right]`.  The lowlevel `Ext` arm deep-evaluates the
    /// operand and gates on its parameterized subtree, so an unbound operand
    /// (a template parameter during the definition pass) is already the lazy
    /// marker; the checker pins both operand types to `Int`, so a wrong-shape
    /// operand here is an invariant violation, not a user error.
    Add,
    Sub,
    Leq,
    Eq,
}

impl OperatorExt<HighProgram> for HighOperator {
    fn run(
        &self,
        operand: HighProgramValue,
        _block: BlockId,
        module: &mut Module<HighProgram>,
    ) -> HighProgramValue {
        match self {
            HighOperator::IndexType => {
                // The Ext arm deep-evaluates the operand and gates on its
                // parameterized subtree, so an unbound type or index has
                // already been turned into the lazy marker.
                if matches!(operand, HighProgramValue::Parameterized) {
                    return HighProgramValue::Parameterized;
                }
                let HighProgramValue::Array(operands) = operand else {
                    unreachable!("IndexType expects an operand array of [type, index]")
                };
                let operands = unsafe { &*operands };
                let type_pair = operands[0];
                let index_node = operands[1];
                let HighProgramValue::USize(index) = module.nodes[index_node]
                    .value
                    .expect("the operand was deep-evaluated")
                else {
                    unreachable!("IndexType needs a USize index node")
                };
                // type_pair's value: [shape, [kind marker, K]].
                let HighProgramValue::Array(pair) = module.nodes[type_pair]
                    .value
                    .expect("the operand was deep-evaluated")
                else {
                    unreachable!("IndexType needs a type expression pair")
                };
                let pair = unsafe { &*pair };
                let shape = pair[0];
                let kind_cell = pair[1];
                let HighProgramValue::Array(kind) = module.nodes[kind_cell]
                    .value
                    .expect("the operand was deep-evaluated")
                else {
                    unreachable!("IndexType needs a kind expression")
                };
                let marker = unsafe { &*kind }[0];
                match module.nodes[marker].value {
                    Some(HighProgramValue::TypeTuple) => {
                        let HighProgramValue::Array(elements) = module.nodes[shape]
                            .value
                            .expect("the operand was deep-evaluated")
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
                            HighProgramValue::None
                        }
                    }
                    Some(HighProgramValue::TypeArray) => {
                        let HighProgramValue::Array(shape_ids) = module.nodes[shape]
                            .value
                            .expect("the operand was deep-evaluated")
                        else {
                            unreachable!("an array type shape is [element type, length]")
                        };
                        let shape_ids = unsafe { &*shape_ids };
                        let element_type = shape_ids[0];
                        let HighProgramValue::USize(length) = module.nodes[shape_ids[1]]
                            .value
                            .expect("the operand was deep-evaluated")
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
                            HighProgramValue::None
                        }
                    }
                    Some(HighProgramValue::TypeId(_)) => {
                        // A struct type's shape is its positional field-type
                        // list — selecting an element is field access, exactly
                        // like a tuple type's element-type list.
                        let HighProgramValue::Array(elements) = module.nodes[shape]
                            .value
                            .expect("the operand was deep-evaluated")
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
                            HighProgramValue::None
                        }
                    }
                    _ => unreachable!("IndexType target must be a tuple, array, or struct type"),
                }
            }
            HighOperator::Fresh => {
                let id = module.global_ext.type_id_counter;
                module.global_ext.type_id_counter += 1;
                HighProgramValue::TypeId(id)
            }
            HighOperator::Add | HighOperator::Sub | HighOperator::Leq | HighOperator::Eq => {
                // The Ext arm already deep-evaluates the operand and gates
                // on its parameterized subtree, so an unbound operand is
                // the lazy marker (the definition pass flags the node).
                if matches!(operand, HighProgramValue::Parameterized) {
                    return HighProgramValue::Parameterized;
                }
                let HighProgramValue::Array(operands) = operand else {
                    unreachable!("binary operators expect an operand array of [left, right]")
                };
                let operands = unsafe { &*operands };
                // A non-USize operand is a *reported* type error, not an
                // invariant violation: the checker pins both operands to
                // `Int`, so a wrong shape only arrives here through an
                // argument unify that already failed (recording the
                // diagnostic) — stay lazy instead of panicking.
                let Some(HighProgramValue::USize(left)) = module.nodes[operands[0]].value else {
                    return HighProgramValue::Parameterized;
                };
                let Some(HighProgramValue::USize(right)) = module.nodes[operands[1]].value else {
                    return HighProgramValue::Parameterized;
                };
                match self {
                    HighOperator::Add => HighProgramValue::USize(left.wrapping_add(right)),
                    HighOperator::Sub => HighProgramValue::USize(left.wrapping_sub(right)),
                    HighOperator::Leq => HighProgramValue::USize((left <= right) as usize),
                    HighOperator::Eq => HighProgramValue::USize((left == right) as usize),
                    _ => unreachable!("all binary operators are handled above"),
                }
            }
        }
    }
}
