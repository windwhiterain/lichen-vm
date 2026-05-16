use std::{collections::HashMap, iter::repeat_n};

use crate::{
    plugin::Project,
    runtime::{self, NodeId},
};

pub struct Table(pub HashMap<Kind, usize>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind(usize);

pub struct Module<P: Project> {
    pub expr_property_count: usize,
    pub runtime: runtime::Module<P>,
    pub nodes: Vec<NodeId<P>>,
    pub properties: Vec<ExprId>,
}

impl<P: Project> Module<P> {
    pub fn new(expr_property_count: usize) -> Self {
        Self {
            expr_property_count,
            runtime: runtime::Module::<P>::new(),
            nodes: Default::default(),
            properties: Default::default(),
        }
    }
    pub fn add_expr(
        &mut self,
        node: NodeId<P>,
        properties: impl Iterator<Item = ExprId>,
    ) -> ExprId {
        debug_assert!(properties.size_hint().0 == self.expr_property_count);
        debug_assert!(properties.size_hint().1 == Some(self.expr_property_count));
        self.properties.extend(properties);
        let ret = self.next_expr();
        self.nodes.push(node);
        ret
    }
    pub fn next_expr(&self) -> ExprId {
        ExprId(self.nodes.len())
    }
    pub fn add_unit_expr(&mut self, value: P::Value) -> ExprId {
        let operation_id = self.runtime.add_literal(value);
        self.add_expr(
            operation_id,
            repeat_n(self.next_expr(), self.expr_property_count),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExprId(usize);
