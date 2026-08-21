//! The highlevel's concrete lowlevel program.
//!
//! For v1 this is the minimal value universe the checker needs: the `int`
//! type constant and the `Type` constant.  The real language's frontend will
//! extend (or replace) this when it arrives.

use lichen_vm::lowlevel::{Block, Node, NodeId, OperatorExt, Program, Value, ValueExt};
use slotmap::SlotMap;

#[derive(Clone, Copy)]
pub struct HighProgram;

impl Program for HighProgram {
    type Value = HighValue;
    type Operator = HighOperator;
}

/// Extension values.  For v1: the type constants.  `Int` is the int type;
/// `Type` is the type of types.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HighValue {
    Int,
    Type,
}

impl ValueExt for HighValue {
    fn is_ptr(&self) -> bool {
        false
    }
}

/// No extension operators yet (the IR has no term operators for v1).
#[derive(Clone, Copy)]
pub enum HighOperator {}

impl OperatorExt<HighProgram> for HighOperator {
    fn run(
        &self,
        _operand: Value<HighProgram>,
        _block: &mut Block,
        _nodes: &SlotMap<NodeId, Node<HighProgram>>,
    ) -> Value<HighProgram> {
        match *self {}
    }
}
