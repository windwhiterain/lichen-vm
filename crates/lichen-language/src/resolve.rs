//! Name resolution as a single, explicit stage: `parse → AST → resolve → compile`.
//!
//! # Invariant
//!
//! The resolver is the only authority for the frontend's scope rules; the
//! lowering reads its `BinderId` fields instead of re-resolving.
//! See docs/notes/language-toolchain.md.

use std::collections::HashMap;

use stacksafe::stacksafe;

use crate::ast::{BinderId, BlockStmt, Expr, Program, RecordField, Stmt};
use crate::diag::{Diag, Stage};
use crate::preprocess::ResolvedImport;
use crate::program::LangProgram;
use crate::suggest;

mod content_key;

pub use content_key::content_key;

/// One imported name bound as a base-scope binder: its id and static export.
pub struct ImportBinder {
    pub binder: BinderId,
    pub export: lichen_lowlevel::StaticNodeId,
    pub span: (u32, u32),
}

/// The outcome of a resolution pass: import binders, prelude names and diagnostics.
pub struct Resolved {
    /// The import binders in `BinderId` order, for `Static` emission.
    pub import_binders: Vec<ImportBinder>,
    /// The **prelude**'s bindings, by name, seeded from the built-in `core`
    /// module; empty when the source has no prelude.
    ///
    /// # Invariant
    ///
    /// The compiler routes the surface operators onto these binders, so the
    /// contract a program meets is the module's, not a built-in's.
    /// See docs/notes/operator-polymorphism.md §7.
    pub prelude: Vec<(String, BinderId)>,
    /// The resolve-layer diagnostics (unresolved names).
    pub diagnostics: Vec<Diag<LangProgram>>,
}

/// Resolve `program` in place, with `imports` seeded into the base scope.
pub fn resolve(program: &mut Program, imports: &[ResolvedImport]) -> Resolved {
    let mut resolver = Resolver {
        scopes: Vec::new(),
        next_binder: 0,
        diagnostics: Vec::new(),
        prelude: Vec::new(),
    };
    let import_binders = resolver.seed_imports(imports);
    // The program is one scope, never popped, so every later statement (and the
    // tail) sees every earlier binding.
    resolver.resolve_scope(&mut stmt_refs(&mut program.statements));
    if let Some(final_expr) = program.expr.as_mut() {
        resolver.resolve_expr(final_expr);
    }
    Resolved {
        import_binders,
        prelude: resolver.prelude,
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

/// The name-resolution scope machine and its single pass.
struct Resolver {
    scopes: Vec<HashMap<String, BinderId>>,
    next_binder: BinderId,
    diagnostics: Vec<Diag<LangProgram>>,
    /// The prelude's own binders, recorded as they are seeded (see
    /// [`Resolved::prelude`]).
    prelude: Vec<(String, BinderId)>,
}

impl Resolver {
    fn lookup(&self, name: &str) -> Option<BinderId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).copied())
    }

    /// Seed the base scope with the imports and their direct exports.
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
            // Record the prelude's names as they are seeded ([`Resolved::prelude`]).
            let prelude = crate::package::is_prelude_import(import);
            for (name, export) in &import.direct {
                let id = self.next_binder;
                self.next_binder += 1;
                frame.insert(name.clone(), id);
                out.push(ImportBinder {
                    binder: id,
                    export: *export,
                    span: import.span,
                });
                if prelude {
                    self.prelude.push((name.clone(), id));
                }
            }
        }
        self.scopes.push(frame);
        out
    }

    /// A statement scope: pre-enter the block-wide bindings in one frame, then
    /// resolve the statements in order.
    fn resolve_scope(&mut self, refs: &mut [&mut Stmt]) {
        let mut frame = HashMap::new();
        for s in refs.iter_mut() {
            if let Stmt::Binding(binding) = &mut **s
                && !binding.restrictive
            {
                let id = self.next_binder;
                self.next_binder += 1;
                binding.binder = Some(id);
                frame.insert(binding.name.clone(), id);
            }
        }
        if !frame.is_empty() {
            self.scopes.push(frame);
        }
        for s in refs.iter_mut() {
            self.resolve_stmt(s);
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

    /// Resolve a record block's fields: named non-`let` fields are block-wide.
    fn resolve_record_fields(&mut self, fields: &mut [RecordField]) {
        let mut frame = HashMap::new();
        for f in fields.iter_mut() {
            if let Some(name) = &f.name
                && f.field
            {
                let id = self.next_binder;
                self.next_binder += 1;
                f.binder = Some(id);
                frame.insert(name.clone(), id);
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
                    self.scopes.push(HashMap::from([(name.clone(), id)]));
                }
                (None, _) => self.resolve_expr(&mut f.value),
            }
        }
    }

    /// The resolver's recursion: one frame per nested expression.
    ///
    /// # Invariant
    ///
    /// `#[stacksafe]`: the recursion grows the stack instead of overflowing the
    /// process.
    #[stacksafe]
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
                self.scopes.push(HashMap::from([(parameter.clone(), id)]));
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
            Expr::Convert { value, .. } => self.resolve_expr(value),
            Expr::NativeCall { args, .. } => {
                for a in args {
                    self.resolve_expr(a);
                }
            }
            Expr::Index { array, index, .. } => {
                self.resolve_expr(array);
                self.resolve_expr(index);
            }
            Expr::RawIndex {
                container, index, ..
            } => {
                self.resolve_expr(container);
                self.resolve_expr(index);
            }
            Expr::FieldRead { container, key, .. } => {
                self.resolve_expr(container);
                self.resolve_expr(key);
            }
            Expr::NamedFieldRead { container, .. } => self.resolve_expr(container),
            Expr::RawNamedField { container, .. } => self.resolve_expr(container),
            Expr::TableFind { container, key, .. } => {
                self.resolve_expr(container);
                self.resolve_expr(key);
            }
            Expr::Annotation {
                value,
                r#type,
                perspective,
                doc,
                refinement,
                ..
            } => {
                self.resolve_expr(value);
                if let Some(t) = r#type {
                    self.resolve_expr(t);
                }
                if let Some(p) = perspective {
                    self.resolve_expr(p);
                }
                if let Some(r) = refinement {
                    self.resolve_expr(r);
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
            Expr::Tuple(elems, _)
            | Expr::TypeTuple(elems, _)
            | Expr::Array(elems, _)
            | Expr::Set(elems, _) => {
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
            Expr::Block {
                statements, expr, ..
            } => {
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
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::TypeConst(..)
            | Expr::Placeholder(..)
            | Expr::Err { .. } => {}
        }
    }
}

/// The sentinel used to mark an unresolved name (as distinct from any
/// `BinderId`, which is always a dense `0..n` index).
const UNRESOLVED: u64 = u64::MAX;
