//! AST → IR compilation with name resolution.
//!
//! The node allocators live in the sibling module `alloc`.
//!
//! A use of a name *is* the binder's own `ExprId`: compiling `x => e`
//! allocates the `Parameter` expression first (span = the name's), pushes `x`
//! on a scope stack, compiles `e`, then wraps `Function { parameter, return }`.
//! A statement binding `a = e` is the same resolution without a lambda: a
//! block-wide binding reserves a `Placeholder` id, enters the name, compiles
//! the value, then fills the placeholder with the value's kind — so a value
//! may reference its own name (and any other binding of the block, in either
//! direction), and the IR becomes a cycle where it does.  A use of the name
//! is the value's own id; the IR is a graph and the binding is pure sharing
//! (no `let`-as-application desugaring).  A restrictive binding `let a = e`
//! compiles the value *before* entering the name, so the name is visible only
//! to later statements — the sequential, non-recursive case.  The IR
//! therefore carries no name strings, and the checker's scope stack is keyed
//! by the same ids.
//! A block `{ …; e }` is a program-shaped expression: its statements share
//! the same way, its scope frames are popped at the `}`, and it compiles to
//! its final expression's own node.
//!
//! Every statement — a binding or a bare expression — is compiled, and the
//! statement list is wrapped in the root as `Index(Tuple([stmt₁, …, stmtₙ,
//! final]), n)`: the checker compiles and runs every statement (the runtime
//! *is* the typechecker), while the program's value stays the final
//! expression.  The wrap is dropped when it would select the final
//! expression's own node (`a = 1; a` stays the `1` node), preserving the
//! sharing.
//!
//! An annotated parameter `x : T => e` desugars to `(x => e) : (T -> _)` —
//! the annotation and arrow are ordinary expressions, so no new IR form is
//! needed.  A conditional `if c then t else e` desugars to the lazy branch
//! `[e, t][c]` — the existing `Index` form, so no new IR form either.  A
//! binary operation `a op b` compiles to `ExprKind::BinOp`.  A prefix assert
//! `!e` compiles straight to the highlevel `ExprKind::Assert` — a side
//! constraint (the condition must evaluate to `USize(1)`), not a new IR form.
//! Shadowing is allowed (the inner binding wins); an unknown name is a
//! resolve diagnostic — the checker's `lookup` panics on unresolved ids, so
//! resolution completes here.  Every emitted expression carries its source
//! span.

use std::collections::HashMap;

use stacksafe::stacksafe;

use lichen_highlevel::attr::AttrSet;
use lichen_highlevel::ir::{BinOp, ChildRange, ConvOp, ExprId, ExprKind, IR, Schema};
use lichen_highlevel::program::{
    FloatLit, FloatTypeLit, HighProgramLiteral, IntLit, IntTypeLit, StrLit, StringTypeLit,
    TypeTypeLit,
};
use lichen_language_lex::Span;

use crate::ast::{BinderId, Binding, Expr, Program, RecordField, Stmt, TypeConst};
use crate::cells::CellStore;
use crate::diag::Diag;
use crate::path::Path;
use crate::preprocess::ResolvedImport;
use crate::program::{LangAttr, LangProgram, Perspective};
use lichen_doc::Doc;
use lichen_lowlevel::StaticNodeId;

mod alloc;

/// `ExprId` → the source span the expr lowers from.  Built here, exactly where
/// each highlevel node is created via the IR alloc API; parallel to `IR.expr`.
/// Highlevel is span-free, so this secondary map — owned by this crate — is the
/// only record of source positions for IR nodes.
pub type SpanIndex = Vec<Option<Span>>;

pub fn compile(program: &mut Program) -> (IR<LangAttr>, SpanIndex, Vec<Diag<LangProgram>>) {
    compile_with_imports(program, &[])
}

/// Lower an already-resolved program (its AST fields carry the `BinderId`s).
///
/// The lowering is **total**: it does not stop at the first problem.  A
/// recovered parse error and an *unresolved name* both lower to the **same**
/// inert [`ExprKind::ErrorBlock`] (masked, checked-skipped); the resolver
/// reports the `Resolve` diagnostic for the unresolved name, and the lower
/// layers proceed on the effective content.  So the pipeline always produces an
/// `IR`; the resolve diagnostics ride out alongside it.
pub fn compile_with_imports(
    program: &mut Program,
    imports: &[ResolvedImport],
) -> (IR<LangAttr>, SpanIndex, Vec<Diag<LangProgram>>) {
    let (ir, spans, diagnostics, _) = compile_with_imports_with_cells(program, imports, None);
    (ir, spans, diagnostics)
}

/// [`compile_with_imports`] with a **cell store**, also returning the marked
/// bindings it compiled (with their paths) so the caller can freeze them once the
/// build is solved.
pub fn compile_with_imports_with_cells(
    program: &mut Program,
    imports: &[ResolvedImport],
    cells: Option<&CellStore>,
) -> (
    IR<LangAttr>,
    SpanIndex,
    Vec<Diag<LangProgram>>,
    Vec<(Path, ExprId)>,
) {
    // Resolution is its own stage: it assigns each binder a `BinderId` (writing
    // it into the AST's resolve fields) and yields the resolve diagnostics.  The
    // lowering below reads those fields instead of re-resolving.
    let resolved = crate::resolve::resolve(program, imports);
    let (ir, spans, compiled_cells) =
        compile_resolved_with_cells(program, &resolved.import_binders, cells);
    (ir, spans, resolved.diagnostics, compiled_cells)
}

/// Lower a *resolved* program.  `program` must already carry its `BinderId`
/// annotations (from [`crate::resolve`]); `import_binders` are the base-scope
/// import binders the resolver assigned, each lowering to a `Static` node.  The
/// incremental session calls this directly (it already ran the resolver for its
/// reuse decision), so resolution is not repeated.
///
/// With a **cell store**: a marked binding whose cell is clean is lowered to a
/// static read of its frozen pair — its body is never lowered, checked or
/// evaluated — and the marked bindings that *were* compiled come back with their
/// paths so the caller can freeze them once the build is solved
/// (`docs/notes/incremental-update.md`).
pub(crate) fn compile_resolved_with_cells(
    program: &Program,
    import_binders: &[crate::resolve::ImportBinder],
    cells: Option<&CellStore>,
) -> (IR<LangAttr>, SpanIndex, Vec<(Path, ExprId)>) {
    let mut compiler = Compiler::new();
    compiler.seed_imports(import_binders);
    // One `for_each` pass over the resolved AST is what gives every marked binding
    // its occurrence path; the lowering walk below never carries one.
    compiler.cached_paths = cached_bindings(program);
    if let Some(cells) = cells {
        for (binder, path) in &compiler.cached_paths {
            if let Some(reference) = cells.reference(path) {
                compiler.reusable.insert(*binder, reference);
            }
        }
    }
    // The whole program is one scope (established by the resolver): block-wide
    // bindings are entered before any value compiles, restrictive `let` bindings
    // are entered as they're seen; the scope is never popped, so later
    // statements (and the final expression) see every earlier binding.
    let (statements, final_id) = match &program.expr {
        Some(final_expr) => {
            // An ordinary program: the top-level statements followed by the
            // tail expression.  `pub` marks on the statements are irrelevant
            // here (a tail program's value is its tail, not a module), so they
            // are folded away, exactly as a `{ …; e }` tail block ignores them.
            let stmts: Vec<Stmt> = program
                .statements
                .iter()
                .map(|bs| bs.stmt.clone())
                .collect();
            let statements = compiler.compile_scope_statements(&stmts);
            let final_id = compiler.compile_expr(final_expr);
            (statements, final_id)
        }
        None => {
            // A record program (a module): the top level has no tail
            // expression, so its value is an anonymous struct built from the
            // statements — the module's exported bindings.  `pub` marks which
            // fields are exported (when any is `pub`, only the `pub` ones).
            let fields: Vec<RecordField> = program
                .statements
                .iter()
                .map(|bs| {
                    let (name, value, binder, field, cached, looping) = match &bs.stmt {
                        Stmt::Binding(b) => (
                            Some(b.name.clone()),
                            b.value.clone(),
                            b.binder,
                            !b.restrictive,
                            b.cached,
                            b.looping,
                        ),
                        Stmt::Expr(e) => (None, e.clone(), None, true, false, false),
                    };
                    RecordField {
                        name,
                        binder,
                        value,
                        public: bs.public,
                        field,
                        cached,
                        looping,
                        span: bs.stmt.span(),
                    }
                })
                .collect();
            let span = program
                .statements
                .first()
                .map(|bs| bs.stmt.span())
                .unwrap_or((1, 1));
            let (statements, root) = compiler.compile_record_fields(&fields, &span);
            (statements, root)
        }
    };
    // Option B: no tuple cascade at the top level.  The statement ids are the
    // "stack of user-written expressions" — recorded as `stmt_roots`, which the
    // checker's `build_with` type-checks and evaluates one by one.  The build
    // root is the final expression directly; a non-terminating statement is
    // reported as a diagnostic instead of being silently deferred by a tuple
    // cascade.  (Nested blocks still use the tuple wrap.)
    compiler.ir.set_stmt_roots(statements);
    compiler.ir.set_root(final_id);
    (compiler.ir, compiler.spans, compiler.compiled_cells)
}

/// The marked bindings of a resolved program, by binder, with their occurrence
/// paths.
///
/// A binding whose value spells an **annotation** is not eligible: a static read
/// materializes the pair the artifact froze, and the artifact's pair is two wide,
/// while an annotation's schema tail makes the source's pair wider — so an
/// annotated cell would be read back at the wrong arity.  It is left to be
/// compiled and re-frozen as an ordinary (unretained) binding, which is honest
/// rather than silently wrong.
///
/// This is also the set the cell store reconciles against (a position the
/// program no longer marks is dropped) and the path lookup dirty propagation
/// needs — see `crate::dirty`.
pub(crate) fn cached_bindings(program: &Program) -> HashMap<BinderId, Path> {
    let mut out = HashMap::new();
    crate::path::for_each(program, &mut |path, node| {
        let (cached, binder, annotated) = match node {
            crate::path::Node::Binding(binding) => (
                binding.cached,
                binding.binder,
                matches!(&binding.value, Expr::Annotation { .. }),
            ),
            crate::path::Node::Field(field) => (
                field.cached,
                field.binder,
                matches!(&field.value, Expr::Annotation { .. }),
            ),
            crate::path::Node::Expr(_) => return,
        };
        if cached
            && !annotated
            && let Some(binder) = binder
        {
            out.insert(binder, path.clone());
        }
    });
    out
}

struct Compiler {
    ir: IR<LangAttr>,
    /// The marked bindings' occurrence paths, by binder (see
    /// [`cached_bindings`]).
    cached_paths: HashMap<BinderId, Path>,
    /// The marked bindings whose cell is clean: the frozen reference their node
    /// is lowered to, instead of compiling their body.
    reusable: HashMap<BinderId, StaticNodeId>,
    /// The marked bindings this build compiled, with their paths — what the
    /// caller freezes and records once the build is solved.
    compiled_cells: Vec<(Path, ExprId)>,
    /// `BinderId` → the IR `ExprId` of that binding's node.  Every binder (a
    /// block-wide or `let` binding, a lambda parameter, a record field, an
    /// import) gets exactly one node, and a use of the name *is* that node —
    /// the IR's graph-sharing invariant.  Filled as binders are emitted, read
    /// by name uses.  Keyed by the resolver's dense `BinderId`.
    binder_to_expr: Vec<Option<ExprId>>,
    /// The `Function` nodes enclosing the current compilation point, each one
    /// a **reserved id** pushed before its own parameter and body compile —
    /// hence stacking ids, not tabs: a nested closure's parent link is filled
    /// in as the closure's own kind *after* its body compiles, so the enclosing
    /// node's id must already exist and be reachable from here.  The reserved
    /// node carries `Placeholder` until the real kind overwrites it, exactly
    /// as a block-wide binding does.
    ///
    /// The invariant this encodes (the mutual-recursion sibling rule): the
    /// parent is pushed only for the span of the **body**, never for the
    /// parameter's type or attribute — a lambda sitting in this lambda's own
    /// annotation is a same-level sibling of it, not a nested child.  A
    /// same-depth sibling binding compiled here only because this lambda's
    /// body references it therefore finds nothing to hang under, so its
    /// closure stays outside this template, referenced in place: the
    /// recursion re-applies the sibling's never-bound template instead of a
    /// bound instance.
    fn_parents: Vec<ExprId>,
    /// The interned native-operator names — a `$name`'s name is interned to a
    /// `&'static str` (leaked once per unique name), because an
    /// `ExprKind::NativeCall`'s op field is a `&'static str` (the `ExprKind`
    /// must stay `Copy`).
    op_names: HashMap<String, &'static str>,
    /// The interned struct field names / named-field-read names — leaked once
    /// per unique string, so [`ExprKind::NamedField`] and the IR struct-name
    /// arena can hold `&'static str` (the `ExprKind` must stay `Copy`).
    str_names: HashMap<String, &'static str>,
    /// The source span of each IR node, keyed by [ExprId] — the crate's own
    /// position index; highlevel itself is span-free.
    spans: Vec<Option<Span>>,
}

impl Compiler {
    fn new() -> Self {
        Compiler {
            ir: IR::new(),
            cached_paths: HashMap::new(),
            reusable: HashMap::new(),
            compiled_cells: Vec::new(),
            binder_to_expr: Vec::new(),
            fn_parents: Vec::new(),
            op_names: HashMap::new(),
            str_names: HashMap::new(),
            spans: Vec::new(),
        }
    }

    /// Emit the `Static` node for each import binder, keyed by its `BinderId`.
    fn seed_imports(&mut self, import_binders: &[crate::resolve::ImportBinder]) {
        for ib in import_binders {
            let id = self.alloc(ExprKind::Static { export: ib.export }, &ib.span);
            self.set_binder(ib.binder, id);
        }
    }

    /// Record `binder`'s IR node (growing the map as the dense ids demand).
    fn set_binder(&mut self, binder: BinderId, expr: ExprId) {
        if binder >= self.binder_to_expr.len() {
            self.binder_to_expr.resize(binder + 1, None);
        }
        self.binder_to_expr[binder] = Some(expr);
    }

    /// The IR node a resolved name use points to — a binder is always emitted
    /// before a use of it (block-wide bindings pre-reserve a placeholder).
    fn binder(&self, binder: BinderId) -> ExprId {
        self.binder_to_expr[binder].expect("a resolved binder is emitted before use")
    }
    /// Compile a statement list, entering every block-wide binding's name
    /// *before* any value compiles so a value may forward- or mutually-
    /// reference the block's bindings.  Restrictive `let` bindings are
    /// entered during the pass (their value compiles first, so the name is
    /// visible only to later statements).
    fn compile_scope_statements(&mut self, statements: &[Stmt]) -> Vec<ExprId> {
        // Pre-pass: reserve a `Placeholder` per block-wide binding and record
        // it under the binding's id.  A scope's block-wide bindings are
        // mutually visible in both directions, so a value may reference itself
        // or a later binding (which resolves to the placeholder).
        for stmt in statements {
            if let Stmt::Binding(binding) = stmt
                && !binding.restrictive
            {
                let binder = binding.binder.expect("a resolved block-wide binding");
                let p = self.alloc(ExprKind::Placeholder, &binding.span);
                self.ir.block_roots.insert(p);
                self.set_binder(binder, p);
            }
        }

        // Compile pass: each statement becomes one id in order.
        let mut out = Vec::new();
        for stmt in statements {
            let id = match stmt {
                // A **clean cell**: the frozen pair, read in place.  The body is
                // not lowered at all — that is what makes a retained cell a
                // saving rather than a cache — so the node is the static read
                // wherever the binding would have been the value's own node.
                Stmt::Binding(binding)
                    if binding
                        .binder
                        .is_some_and(|binder| self.reusable.contains_key(&binder)) =>
                {
                    let binder = binding.binder.expect("a resolved cached binding");
                    let export = self.reusable[&binder];
                    if binding.restrictive {
                        // A `let`: the name enters scope after the value, exactly
                        // as the compiled path does, and there is no placeholder to
                        // reuse — so this is the one node.
                        let id = self.alloc(ExprKind::Static { export }, &binding.span);
                        self.set_binder(binder, id);
                        id
                    } else {
                        // A block-wide binding's placeholder was reserved in the
                        // pre-pass and forward references captured it, so the
                        // placeholder *is* the static read — made one here rather
                        // than transplanted from a second node, which would leave
                        // that node behind in the IR.
                        let p = self.binder(binder);
                        self.ir.set_kind(p, ExprKind::Static { export });
                        self.set_binder(binder, p);
                        p
                    }
                }
                Stmt::Binding(binding) if binding.restrictive => {
                    // `let a = e` — the value compiles before the name is
                    // recorded, so it is visible only to later statements and
                    // never to itself.  `let a = a` resolves `a` to the outer
                    // (or block-wise) binding, exactly the sequential case.
                    let id = self.compile_expr(&binding.value);
                    let binder = binding.binder.expect("a resolved let binding");
                    self.set_binder(binder, id);
                    id
                }
                Stmt::Binding(binding) => {
                    // Block-wide binding: its placeholder `p` was reserved in
                    // the pre-pass.  Compile the value; if the value *is* a
                    // bare name reference (`b = a`, `a = a`), alias this
                    // binding to it — otherwise transplant the value's kind
                    // into `p` so the placeholder becomes the value node and
                    // any self/mutual reference (which resolves to `p`) points
                    // at the value.
                    let binder = binding.binder.expect("a resolved block-wide binding");
                    let p = self.binder(binder);
                    let value = self.compile_expr(&binding.value);
                    if matches!(&binding.value, Expr::Name(..)) {
                        // A bare name reference (`b = a`, `y = x`, and the
                        // degenerate `a = a`): share the resolved id rather
                        // than copying the kind, so the binding aliases it —
                        // one compilation (a transplanted struct type would
                        // recompile under a second nominal id).  Uses compiled
                        // *before* this statement captured the reserved
                        // placeholder `p` (a forward reference); re-point them
                        // to the aliased id, or they would keep the stale
                        // `Placeholder` kind and lose the value's type.
                        self.ir.repoint(p, value);
                        self.set_binder(binder, value);
                        value
                    } else {
                        self.ir.set_kind(p, self.ir.expr[value.0 as usize].kind);
                        // The transplanted kind is identity-sensitive, unlike
                        // the depth it replaced: anything inside it naming the
                        // now-dead `value` id (a nested closure's parent link)
                        // must be re-pointed at `p`, since only `p` is ever
                        // compiled — `value` never receives a node of its own.
                        self.ir.repoint(value, p);
                        self.spans[p.0 as usize] = self.spans[value.0 as usize];
                        // The schema (an attribute tail, e.g. `# p` or `? e`)
                        // rides the *expression*, not the kind, so the
                        // transplant must carry it too — otherwise a bound
                        // annotated value (`a = 5 ? doc`) drops its tail and
                        // the checker panics reading `schema(p).tail[0]`.
                        self.ir.set_schema(p, self.ir.schema(value).clone());
                        p
                    }
                }
                Stmt::Expr(e) => self.compile_expr(e),
            };
            // A marked binding that was *compiled* is what the caller freezes
            // once the build is solved; a clean cell read in place is not.
            if let Stmt::Binding(binding) = stmt
                && binding.cached
                && let Some(binder) = binding.binder
                && !self.reusable.contains_key(&binder)
                && let Some(path) = self.cached_paths.get(&binder)
            {
                self.compiled_cells.push((path.clone(), id));
            }
            out.push(id);
        }
        out
    }

    /// Wire the statements into the root so the checker compiles and runs
    /// every one of them: `Index(Tuple([stmt₁, …, stmtₙ, final]), n)`
    /// selects the final expression as the program's value.  The tuple is
    /// heterogeneous (no element-type unification), so a statement's
    /// polymorphic type is not monomorphized by being wrapped.  A trailing
    /// statement that already *is* the final expression's node (`a = 1; a`)
    /// is dropped — the wrap would only select it again — so a program whose
    /// last statement is its final expression stays that expression's own
    /// node.
    fn wrap(&mut self, statements: Vec<ExprId>, final_id: ExprId, span: &Span) -> ExprId {
        let mut statements = statements;
        if statements.last() == Some(&final_id) {
            statements.pop();
        }
        if statements.is_empty() {
            return final_id;
        }
        statements.push(final_id);
        // The statements ride in a *tuple* (per-element type slots — the
        // statements may be heterogeneous), and the wrapper reads the final
        // one with the positional slot form, whose type extraction indexes
        // the shape list at the key.
        let tuple = self.alloc_tuple(&statements, span);
        let index = self.alloc(
            ExprKind::Literal(HighProgramLiteral::from(IntLit(statements.len() - 1))),
            span,
        );
        self.alloc(
            ExprKind::Field {
                container: tuple,
                key: index,
            },
            span,
        )
    }

    /// Intern a native-operator name to a `&'static str` (leaked once per
    /// unique name), so an [`ExprKind::NativeCall`]'s `op` stays `Copy`.
    ///
    /// **The leak is deliberate and measured** (`D14` in
    /// `docs/notes/code-audit.md`): this map lives for one compile, so the
    /// dedup it provides is *within* a compile, and the bytes never come back.
    /// Measured at 5 bytes per distinct name per compile and 31 bytes/compile
    /// on an editor-like stream of changing sources — about 3 MB per 100k
    /// keystrokes.  It is kept because the `&'static str` is load-bearing
    /// (`ExprKind` must stay `Copy`) and reclaiming it means owning the strings
    /// in the IR, which is a lifetime parameter rippling through `IR`, the
    /// checker and `persist`.  Re-measure before reconsidering: if the rate
    /// ever justifies that, `D14` is the decision to revisit, not this comment.
    fn intern_op(&mut self, name: &str) -> &'static str {
        if let Some(&s) = self.op_names.get(name) {
            return s;
        }
        let s: &'static str = Box::leak(name.to_string().into_boxed_str());
        self.op_names.insert(name.to_string(), s);
        s
    }

    /// Intern an arbitrary source string to a `&'static str` (leaked once per
    /// unique string), so an [`ExprKind::NamedField`]'s field name stays
    /// `Copy`, and struct field names can be stored in the IR's name arena.
    /// Leaks the same way [`Self::intern_op`] does; see it for the measured
    /// rate and the decision (`D14`).
    fn intern_str(&mut self, s: &str) -> &'static str {
        if let Some(&leaked) = self.str_names.get(s) {
            return leaked;
        }
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        self.str_names.insert(s.to_string(), leaked);
        leaked
    }

    /// The lowering's recursion: one frame per nested expression (through the
    /// per-kind arms, [`Self::compile_scope_statements`] and
    /// [`Self::compile_record_fields`] for a block, and back here), so a deep
    /// program overflows the caller's stack — the parser runs on a worker
    /// thread with a large stack, but nothing after it does.  `#[stacksafe]`:
    /// the recursion grows the stack instead of overflowing the process.
    #[stacksafe]
    fn compile_expr(&mut self, e: &Expr) -> ExprId {
        match e {
            Expr::Int(n, span) => self.alloc(
                ExprKind::Literal(HighProgramLiteral::from(IntLit(*n))),
                span,
            ),
            Expr::Float(n, span) => self.alloc(
                ExprKind::Literal(HighProgramLiteral::from(FloatLit(*n))),
                span,
            ),
            // A string literal: the content is leaked once to a `&'static str`
            // (the value node holds a `Copy` `LowValue::Str`), exactly as the
            // native-operator names are interned — and, unlike those, with no
            // dedup at all, so every occurrence leaks again.  Measured and
            // deliberate: 1–11 bytes per literal per compile, `D14` in
            // `docs/notes/code-audit.md`.  The type is the shared
            // `[string, Type]` expression the literal builds.
            Expr::Str(s, span) => self.alloc(
                ExprKind::Literal(HighProgramLiteral::from(StrLit(Box::leak(
                    s.clone().into_boxed_str(),
                )))),
                span,
            ),
            Expr::TypeConst(TypeConst::Int, span) => self.alloc(
                ExprKind::Literal(HighProgramLiteral::from(IntTypeLit)),
                span,
            ),
            Expr::TypeConst(TypeConst::Float, span) => self.alloc(
                ExprKind::Literal(HighProgramLiteral::from(FloatTypeLit)),
                span,
            ),
            Expr::TypeConst(TypeConst::String, span) => self.alloc(
                ExprKind::Literal(HighProgramLiteral::from(StringTypeLit)),
                span,
            ),
            Expr::TypeConst(TypeConst::Type, span) => self.alloc(
                ExprKind::Literal(HighProgramLiteral::from(TypeTypeLit)),
                span,
            ),
            Expr::Name(name, span, binder) => match binder {
                Some(id) => self.binder(*id),
                None => {
                    // An unresolved name was already reported by the resolver
                    // (a `Resolve` diagnostic, possibly with a did-you-mean
                    // clause); it lowers to the same inert `ErrorBlock` a parse
                    // error uses, so the region is masked and the checker skips
                    // it.
                    let _ = name;
                    self.alloc(ExprKind::ErrorBlock, span)
                }
            },
            Expr::Placeholder(span) => self.alloc(ExprKind::Placeholder, span),
            // A recovered parse error: a masked error block — an opaque leaf
            // ([`ExprKind::ErrorBlock`]) the checker skips, never a real
            // placeholder.  The partial program still compiles and checks
            // (the parse diagnostic is reported alongside; the region is
            // distinct from a genuine `_` so a diff can exclude it).
            Expr::Err { start, .. } => self.alloc(ExprKind::ErrorBlock, start),
            Expr::Lambda {
                parameter: _,
                parameter_span,
                parameter_binder,
                parameter_type,
                parameter_perspective,
                r#return,
                span,
            } => {
                let parent = self.fn_parents.last().copied();
                // Reserved before the parameter and body compile — see
                // `Self::fn_parents`: a nested closure names this node as its
                // parent while this node is still a bare `Placeholder`.
                let function_id = self.alloc(ExprKind::Placeholder, span);
                let parameter_id = self.alloc(ExprKind::Parameter, parameter_span);
                // A `x # n` parameter carries the perspective tail in its
                // static schema — the checker reads `schema(parameter).tail[0]`
                // to dispatch the apply's attribute equality check.
                if parameter_perspective.is_some() {
                    self.ir.set_schema(
                        parameter_id,
                        Schema {
                            tail: vec![LangAttr::Perspective(Perspective)],
                        },
                    );
                }
                // The parameter is the binder the body (and its own annotation)
                // resolves to — no scope frame is pushed; the resolver already
                // assigned the id, and the body's uses read it via the map.
                let binder = parameter_binder.expect("a resolved lambda parameter");
                self.set_binder(binder, parameter_id);
                // The annotated parameter's type and attribute are compiled in
                // scope too — either may reference the parameter
                // (`x : x -> Int`).
                let parameter_type = parameter_type.as_ref().map(|t| self.compile_expr(t));
                let parameter_attribute =
                    parameter_perspective.as_ref().map(|p| self.compile_expr(p));
                // Pushed for the **body** only, never for the parameter's type
                // or attribute: a lambda sitting in this lambda's own
                // annotation is a sibling of it, not a nested child.
                let body = {
                    self.fn_parents.push(function_id);
                    let body = self.compile_expr(r#return);
                    self.fn_parents.pop();
                    body
                };
                self.ir.set_kind(
                    function_id,
                    ExprKind::Function {
                        parameter: parameter_id,
                        parameter_type,
                        parameter_attribute,
                        r#return: body,
                        parent,
                    },
                );
                function_id
            }
            Expr::Apply {
                function,
                argument,
                span,
            } => {
                let function = self.compile_expr(function);
                let argument = self.compile_expr(argument);
                self.alloc(ExprKind::Apply { function, argument }, span)
            }
            // Struct instantiation is a *syntactic* form now — `C(f1, …, fn)`
            // with the `(` adjacent to the callee (no space).  It always wraps
            // the field values in one positional tuple and lowers to
            // [`ExprKind::Instantiate`]; a spaced `C (…)` is a plain apply
            // and reaches the Apply arm above.  There is no compile-time
            // callee-kind dispatch — the checker decides whether the callee
            // is a struct type, and a callee that is not one fails there.  A
            // `.x 1` argument carries its name through to the checker, which
            // reorders the values to the definition's positional order.
            Expr::StructInst {
                callee,
                fields,
                span,
            } => {
                let type_expr = self.compile_expr(callee);
                let field_ids: Vec<ExprId> =
                    fields.iter().map(|f| self.compile_expr(&f.value)).collect();
                let names: Vec<Option<&'static str>> = fields
                    .iter()
                    .map(|f| f.name.as_deref().map(|n| self.intern_str(n)))
                    .collect();
                let value = self.alloc_tuple(&field_ids, span);
                self.alloc_instantiate(type_expr, value, &names, span)
            }
            Expr::BinOp {
                operator,
                left,
                right,
                span,
            } => {
                let operator = match operator {
                    crate::ast::BinOp::Add => BinOp::Add,
                    crate::ast::BinOp::Sub => BinOp::Sub,
                    crate::ast::BinOp::Mul => BinOp::Mul,
                    crate::ast::BinOp::Div => BinOp::Div,
                    crate::ast::BinOp::Rem => BinOp::Rem,
                    crate::ast::BinOp::Lt => BinOp::Lt,
                    crate::ast::BinOp::Gt => BinOp::Gt,
                    crate::ast::BinOp::Leq => BinOp::Leq,
                    crate::ast::BinOp::Geq => BinOp::Geq,
                    crate::ast::BinOp::Eq => BinOp::Eq,
                    crate::ast::BinOp::Neq => BinOp::Neq,
                    crate::ast::BinOp::BitAnd => BinOp::BitAnd,
                    crate::ast::BinOp::BitOr => BinOp::BitOr,
                    crate::ast::BinOp::BitXor => BinOp::BitXor,
                };
                let left = self.compile_expr(left);
                let right = self.compile_expr(right);
                self.alloc(
                    ExprKind::BinOp {
                        operator,
                        left,
                        right,
                    },
                    span,
                )
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
                span,
            } => {
                // `if c then t else e` ≡ `[e, t][c]` — the condition (0/1)
                // selects the branch through the existing lazy `Index`, so
                // the untaken branch is never evaluated.  The branch array
                // is homogeneous (both branches share one type, like any
                // conditional).
                let condition = self.compile_expr(condition);
                let then_branch = self.compile_expr(then_branch);
                let else_branch = self.compile_expr(else_branch);
                let branches = self.alloc_array(&[else_branch, then_branch], span);
                self.alloc(
                    ExprKind::Index {
                        array: branches,
                        index: condition,
                    },
                    span,
                )
            }
            Expr::Assert { value, span } => {
                // `! e` — the highlevel `Assert` form: a side constraint, not
                // a unify.  The checker force-evaluates the condition and
                // requires `USize(1)`; the expression compiles to the
                // condition itself (an assert checks its subject, it does not
                // replace it), so its value and type are the condition's.
                // The compiled span is the construct's own, so a failed
                // assert points its caret at the `!`.
                let condition = self.compile_expr(value);
                self.alloc(ExprKind::Assert { condition }, span)
            }
            Expr::Convert {
                operator,
                value,
                span,
            } => {
                // `int2float e` / `float2int e` — the direction rides along and
                // the checker does the class work: it pins the operand to the
                // source class and gives the expression the target one.
                let operator = match operator {
                    crate::ast::ConvOp::Int2Float => ConvOp::Int2Float,
                    crate::ast::ConvOp::Float2Int => ConvOp::Float2Int,
                };
                let value = self.compile_expr(value);
                self.alloc(ExprKind::Convert { operator, value }, span)
            }
            Expr::NativeCall { op, args, span } => {
                // `$name(args…)` — compile each arg into the children arena,
                // intern the op name, and alloc the NativeCall IR.  The name
                // is validated against the compiling module's *private* native
                // registry at check time (the frontend cannot see it).
                let arg_ids: Vec<ExprId> = args.iter().map(|a| self.compile_expr(a)).collect();
                let start = self.ir.children.len() as u32;
                self.ir.children.extend_from_slice(&arg_ids);
                let range = ChildRange {
                    start,
                    end: self.ir.children.len() as u32,
                };
                let op = self.intern_op(op);
                self.alloc(ExprKind::NativeCall { op, args: range }, span)
            }
            Expr::Annotation {
                value,
                r#type,
                perspective,
                doc,
                span,
            } => {
                let value = self.compile_expr(value);
                let r#type = r#type.as_ref().map(|t| self.compile_expr(t));
                // Attribute value expressions, laid out in the **canonical
                // attribute order** (the composed set's order — the single
                // authority the checker's slot merge sorts into as well), so
                // the schema tail and the `attributes` range are aligned with
                // each other and with the merged tail by construction rather
                // than by a hand-spelled sequence.  The mechanism is generic:
                // each annotation kind contributes one (marker, value
                // expression) pair.
                let mut spelled: Vec<(LangAttr, ExprId)> = Vec::new();
                if let Some(p) = &perspective {
                    spelled.push((LangAttr::Perspective(Perspective), self.compile_expr(p)));
                }
                if let Some(d) = &doc {
                    spelled.push((LangAttr::Doc(Doc), self.compile_expr(d)));
                }
                spelled.sort_by_key(|(marker, _)| marker.order_index());
                let tail: Vec<LangAttr> = spelled.iter().map(|(marker, _)| *marker).collect();
                let attrs: Vec<ExprId> = spelled.into_iter().map(|(_, value)| value).collect();
                let id = self.alloc_annotation(value, r#type, &attrs, span);
                // The attribute tail stamps the annotated node's static
                // schema — the one asymmetry with `:` (the slots come into
                // existence by being annotated).
                if !tail.is_empty() {
                    self.ir.set_schema(id, Schema { tail });
                }
                id
            }
            Expr::Index { array, index, span } => {
                let array = self.compile_expr(array);
                let index = self.compile_expr(index);
                self.alloc(ExprKind::Index { array, index }, span)
            }
            Expr::RawIndex {
                container,
                index,
                span,
            } => {
                let container = self.compile_expr(container);
                let index = self.compile_expr(index);
                self.alloc(ExprKind::RawIndex { container, index }, span)
            }
            Expr::TableFind {
                container,
                key,
                span,
            } => {
                let container = self.compile_expr(container);
                let key = self.compile_expr(key);
                self.alloc(ExprKind::Find { container, key }, span)
            }
            Expr::FieldRead {
                container,
                key,
                span,
            } => {
                let container = self.compile_expr(container);
                let key = self.compile_expr(key);
                self.alloc(ExprKind::Field { container, key }, span)
            }
            Expr::NamedFieldRead {
                container,
                name,
                span,
            } => {
                let container = self.compile_expr(container);
                let name = self.intern_str(name);
                self.alloc(ExprKind::NamedField { container, name }, span)
            }
            Expr::RawNamedField {
                container,
                name,
                span,
            } => {
                let container = self.compile_expr(container);
                let name = self.intern_str(name);
                self.alloc(ExprKind::RawNamedField { container, name }, span)
            }
            Expr::Arrow {
                parameter,
                r#return,
                span,
            } => {
                let parameter = self.compile_expr(parameter);
                let r#return = self.compile_expr(r#return);
                self.alloc(
                    ExprKind::TypeFunction {
                        parameter,
                        r#return,
                    },
                    span,
                )
            }
            Expr::Tuple(elements, span) => {
                let ids = self.compile_all(elements);
                self.alloc_tuple(&ids, span)
            }
            Expr::TypeTuple(elements, span) => {
                let ids = self.compile_all(elements);
                self.alloc_type_tuple(&ids, span)
            }
            Expr::StructType(fields, span) => {
                let field_ids: Vec<(ExprId, Option<&'static str>)> = fields
                    .iter()
                    .map(|field| {
                        let ty = self.compile_expr(&field.ty);
                        let name = field.name.as_deref().map(|n| self.intern_str(n));
                        (ty, name)
                    })
                    .collect();
                self.alloc_type_struct(&field_ids, span)
            }
            Expr::Array(elements, span) => {
                // A `~`-marked element (the parser accepts `~` only inside
                // array literals) contributes its inner expression plus a
                // depth; a plain element contributes depth 0.  Any non-zero
                // depth makes the array a shallow array.
                let mut ids = Vec::with_capacity(elements.len());
                let mut depths = Vec::with_capacity(elements.len());
                for element in elements {
                    match element {
                        Expr::Shallow(inner, depth, _) => {
                            ids.push(self.compile_expr(inner));
                            depths.push(*depth);
                        }
                        // Every other kind is a plain element — compile it
                        // with depth 0.  Named exhaustively, so a new kind has
                        // to be classified here instead of silently taking the
                        // `~`-free path.
                        Expr::Int(..)
                        | Expr::Float(..)
                        | Expr::Str(..)
                        | Expr::TypeConst(..)
                        | Expr::Name(..)
                        | Expr::Placeholder(..)
                        | Expr::Lambda { .. }
                        | Expr::Apply { .. }
                        | Expr::BinOp { .. }
                        | Expr::If { .. }
                        | Expr::Assert { .. }
                        | Expr::Convert { .. }
                        | Expr::NativeCall { .. }
                        | Expr::Index { .. }
                        | Expr::RawIndex { .. }
                        | Expr::FieldRead { .. }
                        | Expr::NamedFieldRead { .. }
                        | Expr::RawNamedField { .. }
                        | Expr::TableFind { .. }
                        | Expr::Annotation { .. }
                        | Expr::Arrow { .. }
                        | Expr::Tuple(..)
                        | Expr::TypeTuple(..)
                        | Expr::StructType(..)
                        | Expr::StructInst { .. }
                        | Expr::Array(..)
                        | Expr::Table(..)
                        | Expr::TypeArray { .. }
                        | Expr::Block { .. }
                        | Expr::RecordBlock { .. }
                        | Expr::Err { .. } => {
                            ids.push(self.compile_expr(element));
                            depths.push(0);
                        }
                    }
                }
                if depths.iter().any(|&d| d != 0) {
                    let elements: Vec<(ExprId, usize)> = ids.into_iter().zip(depths).collect();
                    self.alloc_shallow_array(&elements, span)
                } else {
                    self.alloc_array(&ids, span)
                }
            }
            Expr::Table(entries, span) => {
                // Each entry compiles to its key's and value's own nodes
                // (graph-shared like every expression); the pair list feeds
                // the dedicated `ExprKind::Table`.
                let mut pairs = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    pairs.push((self.compile_expr(key), self.compile_expr(value)));
                }
                self.alloc_table(&pairs, span)
            }
            Expr::Shallow(inner, _, _) => {
                // Unreachable through the parser (`~` is accepted only as an
                // array element, which the `Array` arm unwraps); compile the
                // inner expression defensively so the match stays total.
                self.compile_expr(inner)
            }
            Expr::TypeArray {
                element_type,
                length,
                span,
            } => {
                let element_type = self.compile_expr(element_type);
                let length = self.compile_expr(length);
                self.alloc(
                    ExprKind::TypeArray {
                        element_type,
                        length,
                    },
                    span,
                )
            }
            Expr::Block {
                statements,
                expr,
                span,
            } => {
                // The same graph sharing as a program's statements — each
                // value compiles once and a use of a name is its binding's own
                // id — but the block's bindings are block-scoped (the resolver
                // gave them distinct ids), so a block compiles to its final
                // expression's own node (wired through the statement wrapper
                // like a program's).
                let stmts = self.compile_scope_statements(statements);
                let body = self.compile_expr(expr);
                self.wrap(stmts, body, span)
            }
            Expr::RecordBlock { fields, span } => {
                // A struct-returning block: the statements are scoped exactly
                // like a block's, but the block's value is an anonymous struct
                // instance built from the field statements.  A `let` field is a
                // block-local (restrictive) and is never emitted; only the
                // `pub` subset is emitted when any field is `pub`.
                let (_, root) = self.compile_record_fields(fields, span);
                root
            }
        }
    }

    /// Compile a record (struct-returning) block's fields — shared by a
    /// `{ … }` block with no tail (`Expr::RecordBlock`) and a record *program*.
    ///
    /// The field statements are entered in a fresh block scope (block-wide
    /// bindings mutually visible), compiled, then the block's value is built:
    /// an anonymous struct instance over the emitted field ids.  A `let` field
    /// is a block-local (restrictive) and is never emitted; when any field is
    /// `pub`, only the `pub` fields are emitted.  Returns the compiled
    /// statement ids (the field values, in source order, including the
    /// non-emitted `let` locals) and the record node itself.
    fn compile_record_fields(
        &mut self,
        fields: &[RecordField],
        span: &Span,
    ) -> (Vec<ExprId>, ExprId) {
        let stmts: Vec<Stmt> = fields
            .iter()
            .map(|f| match &f.name {
                Some(name) => Stmt::Binding(Binding {
                    name: name.clone(),
                    binder: f.binder,
                    value: f.value.clone(),
                    span: f.span,
                    restrictive: !f.field,
                    cached: f.cached,
                    looping: f.looping,
                }),
                None => Stmt::Expr(f.value.clone()),
            })
            .collect();
        let ids = self.compile_scope_statements(&stmts);
        let any_pub = fields.iter().any(|f| f.public);
        let mut emitted = Vec::with_capacity(fields.len());
        for (f, id) in fields.iter().zip(ids.iter()) {
            // A `let` local is never a struct field; when any field is
            // `pub`, only the `pub` fields are emitted.
            if !f.field || (any_pub && !f.public) {
                continue;
            }
            let name = f.name.as_deref().map(|n| self.intern_str(n));
            emitted.push((name, *id));
        }
        let field_ids: Vec<ExprId> = emitted.iter().map(|(_, v)| *v).collect();
        let names: Vec<Option<&'static str>> = emitted.iter().map(|(n, _)| *n).collect();
        let value = self.alloc_tuple(&field_ids, span);
        let record = self.alloc_record(value, &names, span);
        (ids, record)
    }

    fn compile_all(&mut self, elements: &[Expr]) -> Vec<ExprId> {
        elements.iter().map(|e| self.compile_expr(e)).collect()
    }
}

#[cfg(test)]
#[path = "tests/compile_tests.rs"]
mod tests;
