//! Compiler node allocators: every allocation records the node's span.
//!
//! # Invariant
//!
//! Each allocator pushes exactly one span, so the span index stays aligned with
//! the IR arena's node ids.

use super::*;
impl Compiler {
    pub(super) fn alloc(&mut self, kind: ExprKind<HighProgramLiteral>, span: &Span) -> ExprId {
        let id = self.ir.alloc(kind);
        self.spans.push(Some(*span));
        id
    }

    // Wrappers for the distinct `IR` allocators, so every node records its span.
    pub(super) fn alloc_tuple(&mut self, elements: &[ExprId], span: &Span) -> ExprId {
        let id = self.ir.alloc_tuple(elements);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_type_tuple(&mut self, elements: &[ExprId], span: &Span) -> ExprId {
        let id = self.ir.alloc_type_tuple(elements);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_type_struct(
        &mut self,
        fields: &[(ExprId, Option<&'static str>)],
        span: &Span,
    ) -> ExprId {
        let id = self.ir.alloc_type_struct(fields);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_instantiate(
        &mut self,
        type_expr: ExprId,
        value: ExprId,
        names: &[Option<&'static str>],
        span: &Span,
    ) -> ExprId {
        let id = self.ir.alloc_instantiate(type_expr, value, names);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_record(
        &mut self,
        value: ExprId,
        names: &[Option<&'static str>],
        span: &Span,
    ) -> ExprId {
        let id = self.ir.alloc_record(value, names);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_array(&mut self, elements: &[ExprId], span: &Span) -> ExprId {
        let id = self.ir.alloc_array(elements);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_set(&mut self, members: &[ExprId], span: &Span) -> ExprId {
        let id = self.ir.alloc_set(members);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_table(&mut self, entries: &[(ExprId, ExprId)], span: &Span) -> ExprId {
        let id = self.ir.alloc_table(entries);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_shallow_array(
        &mut self,
        elements: &[(ExprId, usize)],
        span: &Span,
    ) -> ExprId {
        let id = self.ir.alloc_shallow_array(elements);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_annotation(
        &mut self,
        value: ExprId,
        r#type: Option<ExprId>,
        attrs: &[ExprId],
        span: &Span,
    ) -> ExprId {
        let id = self.ir.alloc_annotation(value, r#type, attrs);
        self.spans.push(Some(*span));
        id
    }
}
