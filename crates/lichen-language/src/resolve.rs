//! Name resolution as a single, explicit stage: `parse → AST → resolve → compile`.
//!
//! [`Resolver`] walks an [`ast::Program`] and assigns each binder a [`BinderId`],
//! writing the resolved id into the AST's resolve fields (a [`Name`](ast::Expr::Name)
//! use carries the binder it resolves to; a [`Binding`](ast::Binding), a lambda
//! parameter, and a record field carry their own id).  It is the *only* authority
//! for the frontend's scope rules — the same rules the compiler's IR emission
//! previously implemented inline (block-wide bindings pre-entered before any
//! value, restrictive `let` bindings entered after their value, a lambda's
//! parameter in scope for its body, blocks pushing/popping a scope frame,
//! innermost-last shadowing, and an unresolved name lowering to the same inert
//! `ErrorBlock` as a parse error plus a `Resolve` diagnostic).
//!
//! The lowering (`compile`) reads these fields instead of re-resolving: it keeps a
//! `BinderId → ExprId` map (one IR node per binder) so the IR's graph-sharing
//! invariant ("a use is the binder's own id") holds by construction.  The
//! incremental session likewise compares the resolved fields for reuse rather
//! than hashing the tree.

use std::collections::HashMap;

use crate::ast::{BlockStmt, Expr, Program, RecordField, Stmt, BinderId};
use crate::diag::{Diag, Stage};
use crate::preprocess::ResolvedImport;
use crate::program::LangProgram;
use crate::suggest;

/// One imported (or direct-export) name bound as a base-scope binder: its
/// [`BinderId`] and the static export it lowers to.  The resolver hands these to
/// the compiler so it can emit the corresponding [`ExprKind::Static`] node for
/// a use that resolves to an import.
pub struct ImportBinder {
    pub binder: BinderId,
    pub export: lichen_lowlevel::StaticNodeId,
    pub span: (u32, u32),
}

/// The outcome of a resolution pass: the `BinderId`-annotated program (resolved
/// in place) plus the import binders and the resolve diagnostics.
pub struct Resolved {
    /// The import (and direct-export) binders in `BinderId` order — the compiler
    /// emits a `Static` node for each, keyed by its id.
    pub import_binders: Vec<ImportBinder>,
    /// The resolve-layer diagnostics (unresolved names).
    pub diagnostics: Vec<Diag<LangProgram>>,
}

/// Resolve `program` (with `imports` seeded into the base scope), mutating its
/// `BinderId`/resolve fields in place.  Returns the import binders and
/// diagnostics.
pub fn resolve(program: &mut Program, imports: &[ResolvedImport]) -> Resolved {
    let mut resolver = Resolver {
        scopes: Vec::new(),
        next_binder: 0,
        diagnostics: Vec::new(),
    };
    let import_binders = resolver.seed_imports(imports);
    // The whole program is one scope: block-wide bindings are entered before any
    // value compiles, restrictive `let` bindings are entered as they're seen; the
    // scope is never popped, so later statements (and the tail) see every earlier
    // binding.  A record program (a module) is the same scope with no tail.
    resolver.resolve_scope(&mut stmt_refs(&mut program.statements));
    if let Some(final_expr) = program.expr.as_mut() {
        resolver.resolve_expr(final_expr);
    }
    Resolved {
        import_binders,
        diagnostics: resolver.diagnostics,
    }
}

/// `&mut [BlockStmt]` → its inner `&mut [Stmt]` (the `pub` mark is irrelevant to
/// resolution).
fn stmt_refs(statements: &mut [BlockStmt]) -> Vec<&mut Stmt> {
    statements.iter_mut().map(|bs| &mut bs.stmt).collect()
}

/// `&mut [Stmt]` → `&mut [&mut Stmt]` (a `{ … }` block's statement list).
fn stmt_list_refs(statements: &mut [Stmt]) -> Vec<&mut Stmt> {
    statements.iter_mut().collect()
}

/// The name-resolution scope machine and its single pass.  Its scope semantics
/// mirror the (now removed) resolution the compiler used to do inline.
struct Resolver {
    scopes: Vec<HashMap<String, BinderId>>,
    next_binder: BinderId,
    diagnostics: Vec<Diag<LangProgram>>,
}

impl Resolver {
    fn lookup(&self, name: &str) -> Option<BinderId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).copied())
    }

    /// Seed the base scope frame with the imports (and their direct exports) as
    /// binders, returning the import binders (in `BinderId` order, so the
    /// compiler can `Static`-emit them contiguously from id 0).
    fn seed_imports(&mut self, imports: &[ResolvedImport]) -> Vec<ImportBinder> {
        let mut out = Vec::new();
        if imports.is_empty() {
            return out;
        }
        // Build the base frame first, then push it — there is no innermost frame
        // to enter the import names into yet.
        let mut frame: HashMap<String, BinderId> = HashMap::new();
        for import in imports {
            let id = self.next_binder;
            self.next_binder += 1;
            frame.insert(import.name.clone(), id);
            out.push(ImportBinder {
                binder: id,
                export: import.export,
                span: import.span,
            });
            for (name, export) in &import.direct {
                let id = self.next_binder;
                self.next_binder += 1;
                frame.insert(name.clone(), id);
                out.push(ImportBinder {
                    binder: id,
                    export: *export,
                    span: import.span,
                });
            }
        }
        self.scopes.push(frame);
        out
    }

    /// A statement scope (a program's top level or a `{ … }` block): pre-enter
    /// every block-wide binding in one frame (recording its `BinderId` on the
    /// binding), then resolve the statements in order (a restrictive `let`
    /// resolves its value first, then pushes a fresh frame).  The frames are
    /// *not* popped — the caller truncates for a block; the program's top level
    /// keeps them for its tail.
    fn resolve_scope(&mut self, refs: &mut [&mut Stmt]) {
        let mut frame = HashMap::new();
        for s in refs.iter_mut() {
            if let Stmt::Binding(binding) = &mut **s {
                if !binding.restrictive {
                    let id = self.next_binder;
                    self.next_binder += 1;
                    binding.binder = Some(id);
                    frame.insert(binding.name.clone(), id);
                }
            }
        }
        if !frame.is_empty() {
            self.scopes.push(frame);
        }
        for s in refs.iter_mut() {
            self.resolve_stmt(&mut **s);
        }
    }

    fn resolve_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::Binding(binding) if binding.restrictive => {
                self.resolve_expr(&mut binding.value);
                let id = self.next_binder;
                self.next_binder += 1;
                binding.binder = Some(id);
                self.scopes
                    .push(HashMap::from([(binding.name.clone(), id)]));
            }
            Stmt::Binding(binding) => {
                self.resolve_expr(&mut binding.value);
            }
            Stmt::Expr(e) => self.resolve_expr(e),
        }
    }

    /// Resolve a record block's fields, mirroring the compiler's
    /// `compile_record_fields`: named, non-`let` fields are block-wide bindings
    /// (pre-entered), `let` fields are restrictive, positional fields are bare
    /// expressions.
    fn resolve_record_fields(&mut self, fields: &mut [RecordField]) {
        let mut frame = HashMap::new();
        for f in fields.iter_mut() {
            if let Some(name) = &f.name {
                if f.field {
                    let id = self.next_binder;
                    self.next_binder += 1;
                    f.binder = Some(id);
                    frame.insert(name.clone(), id);
                }
            }
        }
        if !frame.is_empty() {
            self.scopes.push(frame);
        }
        for f in fields.iter_mut() {
            let name = f.name.clone();
            match (&name, f.field) {
                (Some(_), true) => self.resolve_expr(&mut f.value),
                (Some(name), false) => {
                    self.resolve_expr(&mut f.value);
                    let id = self.next_binder;
                    self.next_binder += 1;
                    f.binder = Some(id);
                    self.scopes
                        .push(HashMap::from([(name.clone(), id)]));
                }
                (None, _) => self.resolve_expr(&mut f.value),
            }
        }
    }

    fn resolve_expr(&mut self, e: &mut Expr) {
        match e {
            Expr::Name(name, span, binder) => {
                *binder = match self.lookup(name) {
                    Some(id) => Some(id),
                    None => {
                        let in_scope: Vec<&str> = self
                            .scopes
                            .iter()
                            .flat_map(|f| f.keys())
                            .map(|s| s.as_str())
                            .collect();
                        self.diagnostics.push(Diag::new(
                            Stage::Resolve,
                            *span,
                            suggest::unresolved_message(name, in_scope),
                        ));
                        None
                    }
                };
            }
            Expr::Lambda {
                parameter,
                parameter_binder,
                parameter_type,
                parameter_perspective,
                r#return,
                ..
            } => {
                let id = self.next_binder;
                self.next_binder += 1;
                *parameter_binder = Some(id);
                self.scopes
                    .push(HashMap::from([(parameter.clone(), id)]));
                if let Some(t) = parameter_type {
                    self.resolve_expr(t);
                }
                if let Some(p) = parameter_perspective {
                    self.resolve_expr(p);
                }
                self.resolve_expr(r#return);
                self.scopes.pop();
            }
            Expr::Apply {
                function, argument, ..
            } => {
                self.resolve_expr(function);
                self.resolve_expr(argument);
            }
            Expr::BinOp { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.resolve_expr(condition);
                self.resolve_expr(then_branch);
                self.resolve_expr(else_branch);
            }
            Expr::Assert { value, .. } => self.resolve_expr(value),
            Expr::NativeCall { args, .. } => {
                for a in args {
                    self.resolve_expr(a);
                }
            }
            Expr::Index { array, index, .. } => {
                self.resolve_expr(array);
                self.resolve_expr(index);
            }
            Expr::FieldRead {
                container, key, ..
            } => {
                self.resolve_expr(container);
                self.resolve_expr(key);
            }
            Expr::NamedFieldRead { container, .. } => self.resolve_expr(container),
            Expr::TableFind { container, key, .. } => {
                self.resolve_expr(container);
                self.resolve_expr(key);
            }
            Expr::Annotation {
                value,
                r#type,
                perspective,
                doc,
                ..
            } => {
                self.resolve_expr(value);
                if let Some(t) = r#type {
                    self.resolve_expr(t);
                }
                if let Some(p) = perspective {
                    self.resolve_expr(p);
                }
                if let Some(d) = doc {
                    self.resolve_expr(d);
                }
            }
            Expr::Arrow {
                parameter,
                r#return,
                ..
            } => {
                self.resolve_expr(parameter);
                self.resolve_expr(r#return);
            }
            Expr::Tuple(elems, _) | Expr::TypeTuple(elems, _) | Expr::Array(elems, _) => {
                for el in elems {
                    self.resolve_expr(el);
                }
            }
            Expr::StructType(fields, _) => {
                for f in fields {
                    self.resolve_expr(&mut f.ty);
                }
            }
            Expr::StructInst { callee, fields, .. } => {
                self.resolve_expr(callee);
                for f in fields {
                    self.resolve_expr(&mut f.value);
                }
            }
            Expr::Table(entries, _) => {
                for (k, v) in entries {
                    self.resolve_expr(k);
                    self.resolve_expr(v);
                }
            }
            Expr::Shallow(inner, _, _) => self.resolve_expr(inner),
            Expr::TypeArray {
                element_type,
                length,
                ..
            } => {
                self.resolve_expr(element_type);
                self.resolve_expr(length);
            }
            Expr::Block { statements, expr, .. } => {
                let base = self.scopes.len();
                self.resolve_scope(&mut stmt_list_refs(statements));
                self.resolve_expr(expr);
                self.scopes.truncate(base);
            }
            Expr::RecordBlock { fields, .. } => {
                let base = self.scopes.len();
                self.resolve_record_fields(fields);
                self.scopes.truncate(base);
            }
            // No children/binders — nothing to resolve.
            Expr::Int(..)
            | Expr::Str(..)
            | Expr::TypeConst(..)
            | Expr::TypeOf(..)
            | Expr::Placeholder(..)
            | Expr::Err { .. } => {}
        }
    }
}
