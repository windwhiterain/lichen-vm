use crate::module::ExprId;

pub enum Kind {
    Literal,
}

pub struct Expr{
    pub kind: Kind,
    pub child: ExprId
}

