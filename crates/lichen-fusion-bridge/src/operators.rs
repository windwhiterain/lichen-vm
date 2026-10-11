//! The operator table: which lowered instructions the symbolic language carries.

use lichen_fusion::BinaryOp;
use lichen_kernel_ir::KernelBin;

/// The symbolic operator a lowered binary instruction means, or `None` when it has none.
///
/// # Invariant
/// The two comparisons keep their direction: the IR's operands are the source's, so a
/// translation that reordered them would invert every mask a program wrote. `Rem` and the
/// three bitwise operators have no counterpart: the symbolic language is over real numbers.
pub fn binary(operator: KernelBin) -> Option<BinaryOp> {
    match operator {
        KernelBin::Add => Some(BinaryOp::Add),
        KernelBin::Sub => Some(BinaryOp::Sub),
        KernelBin::Mul => Some(BinaryOp::Mul),
        KernelBin::Div => Some(BinaryOp::Div),
        KernelBin::Max => Some(BinaryOp::Max),
        KernelBin::Min => Some(BinaryOp::Min),
        KernelBin::Lt => Some(BinaryOp::LessThan),
        KernelBin::Gt => Some(BinaryOp::GreaterThan),
        KernelBin::Leq => Some(BinaryOp::LessEqual),
        KernelBin::Geq => Some(BinaryOp::GreaterEqual),
        KernelBin::Eq => Some(BinaryOp::Equal),
        KernelBin::Neq => Some(BinaryOp::NotEqual),
        KernelBin::Rem | KernelBin::BitAnd | KernelBin::BitOr | KernelBin::BitXor => None,
    }
}

/// The operator as it is written in a refusal.
///
/// # Invariant
/// One spelling per operator, so a report naming one can be searched for; the four the
/// symbolic language lacks are spelled as the source writes them.
pub fn operator_spelling(operator: KernelBin) -> &'static str {
    match operator {
        KernelBin::Add => "+",
        KernelBin::Sub => "-",
        KernelBin::Mul => "*",
        KernelBin::Div => "/",
        KernelBin::Rem => "%",
        KernelBin::Max => "max",
        KernelBin::Min => "min",
        KernelBin::Lt => "<",
        KernelBin::Gt => ">",
        KernelBin::Leq => "<=",
        KernelBin::Geq => ">=",
        KernelBin::Eq => "==",
        KernelBin::Neq => "!=",
        KernelBin::BitAnd => "&",
        KernelBin::BitOr => "|",
        KernelBin::BitXor => "^",
    }
}
