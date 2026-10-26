//! Name resolution as a single, explicit stage: `parse → AST → resolve → compile`.
//!
//! # Invariant
//!
//! The resolver is the only authority for the frontend's scope rules; the
//! lowering reads its `BinderId` fields instead of re-resolving.
//! See docs/notes/language-toolchain.md.

use std::collections::HashMap;

use stacksafe::stacksafe;

use crate::Span;
use crate::ast::{BinderId, BlockStmt, Expr, Program, RecordField, Stmt};
use crate::diag::{Diag, Stage};
use crate::preprocess::ResolvedImport;
use crate::program::LangProgram;
use crate::suggest;

pub(crate) mod content_key;

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
        open_bindings: Vec::new(),
        lambda_depth: 0,
        call_head: None,
        binding_edges: Vec::new(),
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
    /// The block-wide bindings whose value the pass is inside, innermost last.
    open_bindings: Vec<OpenBinding>,
    /// How many lambda bodies the expression being resolved is under.
    lambda_depth: u32,
    /// The lambda depth the innermost apply's function was entered at, `None` outside one.
    call_head: Option<u32>,
    /// One edge per value read: the binding whose value is being resolved, then the binding it reads.
    binding_edges: Vec<(BinderId, BinderId)>,
}

/// A block-wide binding whose value is being resolved right now.
struct OpenBinding {
    binder: BinderId,
    /// The lambda depth the value started at, so a reference under a `=>` body is not this one.
    depth: u32,
    /// Whether this binding has already been refused, so it speaks once.
    refused: bool,
    /// Whether the value being resolved is a struct or type tree, whose cycles the deep pass cuts.
    tree_valued: bool,
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

    /// Enter a block-wide binding's value, where a name that resolves back to it is its own input.
    fn open_binding(&mut self, binder: Option<BinderId>, tree_valued: bool) {
        if let Some(binder) = binder {
            self.open_bindings.push(OpenBinding {
                binder,
                depth: self.lambda_depth,
                refused: false,
                tree_valued,
            });
        }
    }

    /// Refuse a name whose reference would close a cycle among the block's scalar bindings.
    fn refuse_binding_cycle(&mut self, name: &str, span: Span, binder: &mut Option<BinderId>) {
        let Some(target) = *binder else { return };
        let depth = self.lambda_depth;
        let head = self.call_head;
        // A call head or a struct/type tree's reference is not a scalar value read.
        let read: Vec<BinderId> = self
            .open_bindings
            .iter()
            .filter(|open| !open.tree_valued && !(depth > open.depth && head == Some(depth)))
            .map(|open| open.binder)
            .collect();
        if read.is_empty() {
            return;
        }
        // The cycle closes when the name's own value already reaches one of the values being read.
        let closing = read
            .iter()
            .copied()
            .find(|&from| from != target && self.reaches(target, from));
        let Some(from) = closing else {
            for from in read {
                self.binding_edges.push((from, target));
            }
            return;
        };
        // The use is left unresolved and lowers inert; one edge removed makes the block acyclic.
        *binder = None;
        self.binding_edges.push((from, target));
        self.diagnostics
            .push(Diag::new(Stage::Resolve, span, binding_cycle(name)));
    }

    /// Whether `from` reaches `to` through the value reads recorded so far.
    fn reaches(&self, from: BinderId, to: BinderId) -> bool {
        let mut stack = vec![from];
        let mut seen: Vec<BinderId> = Vec::new();
        while let Some(node) = stack.pop() {
            if node == to {
                return true;
            }
            if seen.contains(&node) {
                continue;
            }
            seen.push(node);
            for &(a, b) in &self.binding_edges {
                if a == node {
                    stack.push(b);
                }
            }
        }
        false
    }

    /// Refuse a name that resolves to the block-wide binding whose value is being resolved.
    fn refuse_self_reference(&mut self, name: &str, span: Span, binder: &mut Option<BinderId>) {
        let Some(id) = *binder else { return };
        let depth = self.lambda_depth;
        let head = self.call_head;
        let Some(open) = self.open_bindings.iter_mut().find(|open| open.binder == id) else {
            return;
        };
        // A binding named as the head of an apply inside a `=>` body is calling itself.
        if depth > open.depth && head == Some(depth) {
            return;
        }

        // The use is left unresolved, so it lowers to the inert leaf an unresolved name's does.
        *binder = None;
        if open.refused {
            return; // The first reference already named the mistake.
        }
        open.refused = true;
        self.diagnostics.push(Diag::new(
            Stage::Resolve,
            span,
            self_referential_binding(name),
        ));
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
                self.open_binding(binding.binder, is_tree_expr(&binding.value));
                self.resolve_expr(&mut binding.value);
                self.open_bindings.pop();
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
                (Some(_), true) => {
                    self.open_binding(f.binder, is_tree_expr(&f.value));
                    self.resolve_expr(&mut f.value);
                    self.open_bindings.pop();
                }
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
                self.refuse_self_reference(name, *span, binder);
                self.refuse_binding_cycle(name, *span, binder);
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
                self.lambda_depth += 1;
                self.resolve_expr(r#return);
                self.lambda_depth -= 1;
                self.scopes.pop();
            }
            Expr::Apply {
                function, argument, ..
            } => {
                let enclosing = self.call_head;
                self.call_head = Some(self.lambda_depth);
                self.resolve_expr(function);
                self.call_head = enclosing;
                self.resolve_expr(argument);
            }
            Expr::BinOp { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            // `a = b`: the actual value and the expected one.
            Expr::Unify {
                actual, expected, ..
            } => {
                self.resolve_expr(actual);
                self.resolve_expr(expected);
            }
            // `actual <: template`: the actual and the template it is matched against.
            Expr::TemplateUnify {
                actual, template, ..
            } => {
                self.resolve_expr(actual);
                self.resolve_expr(template);
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
            Expr::Prefix { operands, .. } => {
                for operand in operands {
                    self.resolve_expr(operand);
                }
            }
            Expr::NativeCall { args, .. } => {
                for a in args {
                    self.resolve_expr(a);
                }
            }
            Expr::Index { array, index, .. } => {
                self.resolve_expr(array);
                self.resolve_expr(index);
            }
            Expr::FieldRead { container, key, .. } => {
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
            Expr::Tuple(elems, _) | Expr::Set(elems, _) => {
                for el in elems {
                    self.resolve_expr(el);
                }
            }
            // `{.a e1, …}`: the struct template's fields.
            Expr::StructValue { fields, .. } => {
                for f in fields {
                    self.resolve_expr(&mut f.value);
                }
            }
            Expr::StructInst { callee, fields, .. } => {
                self.resolve_expr(callee);
                for f in fields {
                    self.resolve_expr(&mut f.value);
                }
            }
            // `[template, length]`: the array template and its length.
            Expr::Array {
                template, length, ..
            } => {
                self.resolve_template_part(template);
                self.resolve_template_part(length);
            }
            // `template[e1, …]`: the array instance's template and its elements.
            Expr::ArrayInst {
                template, elements, ..
            } => {
                self.resolve_expr(template);
                for el in elements {
                    match el {
                        // The element's mark is the depth the instance carries.
                        Expr::Shallow(inner, depth, span) => {
                            self.check_shallow_depth(*depth, *span);
                            self.resolve_expr(inner);
                        }
                        other => self.resolve_expr(other),
                    }
                }
            }
            Expr::Table(entries, _) => {
                for (k, v) in entries {
                    self.resolve_expr(k);
                    self.resolve_expr(v);
                }
            }
            Expr::TypeSet(element, _) => self.resolve_expr(element),
            Expr::TypeTable { key, value, .. } => {
                self.resolve_expr(key);
                self.resolve_expr(value);
            }
            Expr::Shallow(inner, depth, span) => {
                // A mark outside an array instance's element has no carrier,
                // so it is refused rather than dropped.
                self.diagnostics
                    .push(Diag::new(Stage::Resolve, *span, SHALLOW_WITHOUT_CARRIER));
                self.check_shallow_depth(*depth, *span);
                self.resolve_expr(inner);
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
            // `@gc` adds no binder and no scope of its own.
            Expr::Gc(inner, _) => self.resolve_expr(inner),
            // No children/binders — nothing to resolve.
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::TypeConst(..)
            | Expr::Placeholder(..)
            | Expr::Any(..)
            | Expr::Err { .. } => {}
        }
    }

    /// Resolve one part of an array template, refusing a `@shallow` mark.
    ///
    /// # Invariant
    ///
    /// A template carries no depth, so a mark written on its element or its
    /// length is refused here rather than dropped: the mark's only carrier is
    /// an instance's element.
    fn resolve_template_part(&mut self, part: &mut Box<Expr>) {
        match part.as_mut() {
            Expr::Shallow(inner, _, span) => {
                self.diagnostics
                    .push(Diag::new(Stage::Resolve, *span, SHALLOW_IN_ARRAY_TEMPLATE));
                self.resolve_expr(inner);
            }
            other => self.resolve_expr(other),
        }
    }

    /// Refuse a `@shallow` depth of 0, which marks no level.
    fn check_shallow_depth(&mut self, depth: usize, span: Span) {
        if depth == 0 {
            self.diagnostics
                .push(Diag::new(Stage::Resolve, span, SHALLOW_ZERO_DEPTH));
        }
    }
}

/// The sentinel used to mark an unresolved name (as distinct from any
/// `BinderId`, which is always a dense `0..n` index).
const UNRESOLVED: u64 = u64::MAX;

/// The `@shallow(0)` refusal; docs/language-spec.md §"Shallow markers" states the bound.
const SHALLOW_ZERO_DEPTH: &str = "a `@shallow` depth of 0 marks no level — the mark is erased before the checker sees the element, so the position is checked exactly as if it were unmarked — write a depth of at least 1, or drop the marker entirely";

/// The `@shallow`-in-an-array-template refusal: a template carries no depth.
///
/// # Invariant
///
/// A depth belongs to an **instance's element** — the position a value sits in.
/// An array template names the layout an instance is filled against, so a mark
/// written there has no carrier and is refused rather than dropped.
const SHALLOW_IN_ARRAY_TEMPLATE: &str = "an `@shallow` mark has no place in an array template — the template names the layout an instance is filled against, and only an instance's element carries a depth — write the mark on the instance's element (`template[@shallow e]`), or drop it";

/// The `@shallow`-without-a-carrier refusal: only an instance's element is one.
const SHALLOW_WITHOUT_CARRIER: &str = "an `@shallow` mark has no place here — a depth belongs to an array instance's element, and this position is not one — write the mark on the instance's element (`template[@shallow e]`), or drop it";

/// Whether `e` is a struct or type tree, whose cycles the deep pass cuts.
///
/// # Invariant
///
/// A compound value that names a layout — a tuple, an array template, a struct
/// value or a struct instance — is a tree: a cycle among such bindings is the
/// deep pass's to cut, because the layout a value is written against may name
/// itself. An array instance is a plain value, so its cycle is refused like
/// any other value's.
fn is_tree_expr(e: &Expr) -> bool {
    matches!(
        e,
        Expr::StructValue { .. }
            | Expr::StructInst { .. }
            | Expr::Tuple(..)
            | Expr::Array { .. }
            | Expr::TypeSet(..)
            | Expr::TypeTable { .. }
            | Expr::TypeConst(..)
    )
}

/// The scalar-cycle refusal; the same note's §11 records the rule and its limits.
fn binding_cycle(name: &str) -> String {
    format!(
        "`{name}` closes a cycle among the block's bindings: a block-wide binding's name is its \
         value's own node in the IR graph, so a group that depends on itself is its own input. Write \
         the rebinding with `let`, which compiles a value before its name enters scope, and spell \
         recursion as a call, with the name as the head of an apply inside a `=>` body."
    )
}

/// The self-reference refusal, measured in docs/notes/self-referential-binding-overflow.md §11.
fn self_referential_binding(name: &str) -> String {
    format!(
        "`{name}` names itself in its own value: a block-wide binding's value may not reference \
         the binding it binds, because the name **is** the value's own node in the IR graph, so \
         the value would be its own input. Write the rebinding with `let`, which compiles the \
         value before the name enters scope, so `let {name} = …` reads the **outer** `{name}` and \
         binds a new one — recursion is spelled the same way, with the reference under a `=>` \
         parameter (`f = n => … f (n - 1)`), which is a call rather than a self-reference."
    )
}
