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
    pub(super) fn alloc_type_set(&mut self, element: ExprId, span: &Span) -> ExprId {
        let id = self.ir.alloc_type_set(element);
        self.spans.push(Some(*span));
        id
    }
    pub(super) fn alloc_type_table(&mut self, key: ExprId, value: ExprId, span: &Span) -> ExprId {
        let id = self.ir.alloc_type_table(key, value);
        self.spans.push(Some(*span));
        id
    }
    /// Allocate a struct template: the field expressions and their names.
    ///
    /// # Invariant
    ///
    /// The two ranges are written here and are index-aligned: `names[i]` is the
    /// name of `fields[i]`, so the checker reads a field by name against the
    /// same position.
    pub(super) fn alloc_struct(
        &mut self,
        fields: &[(ExprId, Option<&'static str>)],
        span: &Span,
    ) -> ExprId {
        let fields_start = self.ir.children.len();
        self.ir
            .children
            .extend(fields.iter().map(|&(field, _)| field));
        let names_start = self.ir.struct_names.len();
        self.ir
            .struct_names
            .extend(fields.iter().map(|&(_, name)| name));
        let id = self.ir.alloc(ExprKind::Struct {
            fields: ChildRange {
                start: fields_start as u32,
                end: self.ir.children.len() as u32,
            },
            names: ChildRange {
                start: names_start as u32,
                end: self.ir.struct_names.len() as u32,
            },
        });
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
    /// Allocate an array instance: its template, its elements and their depths.
    ///
    /// # Invariant
    ///
    /// The depths range is **empty** when no element is marked, and otherwise
    /// one entry per element, aligned with the element ids. See
    /// `docs/notes/shallow-depth-mechanism.md`.
    pub(super) fn alloc_array_inst(
        &mut self,
        template: ExprId,
        elements: &[ExprId],
        depths: &[usize],
        span: &Span,
    ) -> ExprId {
        let elements_start = self.ir.children.len();
        self.ir.children.extend_from_slice(elements);
        let depths_start = self.ir.depths.len();
        self.ir.depths.extend_from_slice(depths);
        let id = self.ir.alloc(ExprKind::ArrayInst {
            template,
            elements: ChildRange {
                start: elements_start as u32,
                end: self.ir.children.len() as u32,
            },
            depths: ChildRange {
                start: depths_start as u32,
                end: self.ir.depths.len() as u32,
            },
        });
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
