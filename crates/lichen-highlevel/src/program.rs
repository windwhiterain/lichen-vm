//! The highlevel's concrete lowlevel program.
//!
//! For v1 this is the minimal value universe the checker needs: the `int`
//! type constant and the `Type` constant.  The real language's frontend will
//! extend (or replace) this when it arrives.

use lichen_lowlevel::{BlockId, EvalError, Module, OperatorExt, Program, Value, ValueExt};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighProgram;

/// The highlevel's global extension state, injected into the lowlevel
/// [`Module`]'s `global_ext` slot and threaded through the extension
/// operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HighGlobalExt {
    /// The next nominal type id — [`HighOperator::Fresh`] reads and
    /// increments it, so each call yields a distinct [`HighValue::TypeId`].
    pub type_id_counter: usize,
}

impl Program for HighProgram {
    type Value = HighValue;
    type Operator = HighOperator;
    type GlobalExt = HighGlobalExt;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HighValue {
    /// # Instance: 
    /// [`Value::USize`]
    TypeInt,
    /// # Instance
    /// all Self::TypeXXX, e.g. [`Self::TypeInt`], [`Self::TypeType`], ...
    TypeType,
    /// # Instance
    /// [`Value::Array`]
    /// - [0]: parameter type
    /// - [1]: return type 
    /// # Instance's Instance
    /// [`Value::Function`]
    TypeFunction,
    /// # Instance: [`Value::Array`]
    /// - [x]: the type of element x — an instance of [`Self::TypeType`]
    /// # Instance's Instance
    /// - [x]: instance of instance[x]
    TypeTuple,
    /// # Instance: [`Value::Array`]
    /// - [0]: the type shared by all elements
    /// - [1]: the length
    /// # Instance's Instance
    /// - [x]: instance of instance[0]
    TypeArray,
    /// A nominal type id — the kind marker of a struct type.  Equal ids
    /// unify, different ids don't (nominal identity), and an id never
    /// unifies with the structural markers above.
    TypeId(usize),
}

impl ValueExt for HighValue {
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
    /// value's type expression.  A tuple type (`TypeTuple` kind) selects
    /// its element-type list position — a structural out of bounds.  An
    /// array type (`ArrayType` kind) checks the index against the *length*
    /// stored in its shape `[element_type, length]` — the check the
    /// structural `Index` cannot express (the shape holds the length as
    /// data, not as selectable positions).  Out of bounds records an
    /// [`EvalError`] and yields [`Value::None`].
    IndexType,
    /// A fresh nominal type id: each call reads and increments
    /// [`HighGlobalExt::type_id_counter`] and returns
    /// [`Value::Ext(HighValue::TypeId(n))`].  Nullary — the checker emits
    /// it with no operand, so it fires once per source occurrence and the
    /// cached value is reused wherever the struct type it tags is
    /// referenced.
    Fresh,
}

impl OperatorExt<HighProgram> for HighOperator {
    fn run(
        &self,
        operand: Value<HighProgram>,
        _block: BlockId,
        module: &mut Module<HighProgram>,
    ) -> Value<HighProgram> {
        match self {
            HighOperator::IndexType => {
                // The Ext arm deep-evaluates the operand and gates on its
                // parameterized subtree, so an unbound type or index has
                // already been turned into the lazy marker.
                if matches!(operand, Value::Parameterized) {
                    return Value::Parameterized;
                }
                let Value::Array(operands) = operand else {
                    unreachable!("IndexType expects an operand array of [type, index]")
                };
                let operands = unsafe { &*operands };
                let type_pair = operands[0];
                let index_node = operands[1];
                let Value::USize(index) = module.nodes[index_node]
                    .value
                    .expect("the operand was deep-evaluated")
                else {
                    unreachable!("IndexType needs a USize index node")
                };
                // type_pair's value: [shape, [kind marker, K]].
                let Value::Array(pair) = module.nodes[type_pair]
                    .value
                    .expect("the operand was deep-evaluated")
                else {
                    unreachable!("IndexType needs a type expression pair")
                };
                let pair = unsafe { &*pair };
                let shape = pair[0];
                let kind_cell = pair[1];
                let Value::Array(kind) = module.nodes[kind_cell]
                    .value
                    .expect("the operand was deep-evaluated")
                else {
                    unreachable!("IndexType needs a kind expression")
                };
                let marker = unsafe { &*kind }[0];
                match module.nodes[marker].value {
                    Some(Value::Ext(HighValue::TypeTuple)) => {
                        let Value::Array(elements) = module.nodes[shape]
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
                            Value::None
                        }
                    }
                    Some(Value::Ext(HighValue::TypeArray)) => {
                        let Value::Array(shape_ids) = module.nodes[shape]
                            .value
                            .expect("the operand was deep-evaluated")
                        else {
                            unreachable!("an array type shape is [element type, length]")
                        };
                        let shape_ids = unsafe { &*shape_ids };
                        let element_type = shape_ids[0];
                        let Value::USize(length) = module.nodes[shape_ids[1]]
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
                            Value::None
                        }
                    }
                    _ => unreachable!("IndexType target must be a tuple or array type"),
                }
            }
            HighOperator::Fresh => {
                let id = module.global_ext.type_id_counter;
                module.global_ext.type_id_counter += 1;
                Value::Ext(HighValue::TypeId(id))
            }
        }
    }
}
