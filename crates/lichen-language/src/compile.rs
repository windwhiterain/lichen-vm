//! AST → IR compilation with name resolution; node allocators in `alloc`.
//!
//! # Invariant
//!
//! A use of a name is the binder's own `ExprId`, so the IR is a graph: a
//! block-wide binding reserves its id before its value compiles, and the
//! checker's scope stack is keyed by the same ids. See `docs/language-spec.md`,
//! "Name resolution".

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
use crate::program::{LangAttr, LangProgram, Perspective, Refinement};
use lichen_doc::Doc;
use lichen_lowlevel::StaticNodeId;

mod alloc;

/// `ExprId` → the source span the expression lowers from; parallel to `IR.expr`.
pub type SpanIndex = Vec<Option<Span>>;

pub fn compile(program: &mut Program) -> (IR<LangAttr>, SpanIndex, Vec<Diag<LangProgram>>) {
    compile_with_imports(program, &[])
}

/// Lower an already-resolved program (its AST fields carry the `BinderId`s).
///
/// # Invariant
///
/// Lowering is total: a recovered parse error and an unresolved name both lower
/// to an inert `ExprKind::ErrorBlock` the checker skips, so an `IR` is always
/// produced and the resolve diagnostics ride out alongside it.
pub fn compile_with_imports(
    program: &mut Program,
    imports: &[ResolvedImport],
) -> (IR<LangAttr>, SpanIndex, Vec<Diag<LangProgram>>) {
    let (ir, spans, diagnostics, _) = compile_with_imports_with_cells(program, imports, None);
    (ir, spans, diagnostics)
}

/// [`compile_with_imports`] with a cell store, returning the marked bindings it compiled.
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
    let resolved = crate::resolve::resolve(program, imports);
    let (ir, spans, compiled_cells) =
        compile_resolved_with_cells(program, &resolved.import_binders, &resolved.prelude, cells);
    (ir, spans, resolved.diagnostics, compiled_cells)
}

/// Lower a resolved program, reusing the resolver's `BinderId` annotations.
///
/// # Invariant
///
/// A clean cell is lowered to a static read of its frozen pair, so its body is
/// never lowered, checked or evaluated. See `docs/notes/incremental-update.md`.
pub(crate) fn compile_resolved_with_cells(
    program: &Program,
    import_binders: &[crate::resolve::ImportBinder],
    prelude: &[(String, BinderId)],
    cells: Option<&CellStore>,
) -> (IR<LangAttr>, SpanIndex, Vec<(Path, ExprId)>) {
    let mut compiler = Compiler::new();
    compiler.seed_imports(import_binders);
    compiler.seed_prelude(prelude);
    compiler.cached_paths = cached_bindings(program);
    if let Some(cells) = cells {
        for (binder, path) in &compiler.cached_paths {
            if let Some(reference) = cells.reference(path) {
                compiler.reusable.insert(*binder, reference);
            }
        }
    }
    let (statements, final_id) = match &program.expr {
        Some(final_expr) => {
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
    // No tuple cascade at the top level: the statement ids go to `stmt_roots`.
    compiler.ir.set_stmt_roots(statements);
    compiler.ir.set_root(final_id);
    (compiler.ir, compiler.spans, compiler.compiled_cells)
}

/// The marked bindings of a resolved program, by binder, with their paths.
///
/// # Invariant
///
/// A binding whose value spells an annotation is never returned: a static read
/// materializes the artifact's two-wide pair, while an annotation's schema tail
/// widens the source's pair, so the cell would be read back at the wrong arity.
/// See `docs/notes/incremental-update.md` §3.3.
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
    /// The marked bindings' occurrence paths, by binder (see [`cached_bindings`]).
    cached_paths: HashMap<BinderId, Path>,
    /// Clean cells: the frozen reference a marked binding's node lowers to.
    reusable: HashMap<BinderId, StaticNodeId>,
    /// The marked bindings this build compiled, with their paths, to be frozen.
    compiled_cells: Vec<(Path, ExprId)>,
    /// `BinderId` → the IR node of that binding; every binder gets exactly one.
    binder_to_expr: Vec<Option<ExprId>>,
    /// The enclosing `Function` nodes, each a reserved id pushed for its body.
    ///
    /// # Invariant
    ///
    /// A parent is pushed for the body's span only, never for the parameter's
    /// type or attribute: a lambda in this lambda's own annotation is a
    /// same-level sibling, so a sibling compiled only because this body
    /// references it finds nothing to hang under and stays outside this
    /// template.
    fn_parents: Vec<ExprId>,
    /// Interned native-operator names: `&'static str`, because `ExprKind` stays `Copy`.
    op_names: HashMap<String, &'static str>,
    /// Interned struct field and named-read names, `&'static str` for the same reason.
    str_names: HashMap<String, &'static str>,
    /// The source span of each IR node, keyed by [`ExprId`].
    spans: Vec<Option<Span>>,
    /// The prelude's binders by name: what surface operators route onto.
    ///
    /// # Invariant
    ///
    /// Empty for the built-in module's own compilation, so its body keeps the
    /// machine operators and the routing terminates.
    prelude: HashMap<String, BinderId>,
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
            prelude: HashMap::new(),
        }
    }

    /// Record the prelude's binders for [`Self::routed_operator`].
    fn seed_prelude(&mut self, prelude: &[(String, BinderId)]) {
        for (name, binder) in prelude {
            self.prelude.insert(name.clone(), *binder);
        }
    }

    /// The prelude binding a surface operator routes onto, if the prelude exports it.
    ///
    /// # Invariant
    ///
    /// Only the operators the prelude carries are routed; `==`, `!=`, `%` and
    /// the bitwise trio are never routed, their contract being a class the
    /// checker already pins. See `docs/notes/operator-polymorphism.md` §7.
    fn routed_operator(&self, operator: &crate::ast::BinOp) -> Option<BinderId> {
        let name = match operator {
            crate::ast::BinOp::Add => "add",
            crate::ast::BinOp::Sub => "sub",
            crate::ast::BinOp::Mul => "mul",
            crate::ast::BinOp::Div => "div",
            crate::ast::BinOp::Lt => "less",
            crate::ast::BinOp::Gt => "greater",
            crate::ast::BinOp::Leq => "less_or_equal",
            crate::ast::BinOp::Geq => "greater_or_equal",
            _ => return None,
        };
        self.prelude.get(name).copied()
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

    /// The IR node a resolved name use points to; the binder is always emitted first.
    fn binder(&self, binder: BinderId) -> ExprId {
        self.binder_to_expr[binder].expect("a resolved binder is emitted before use")
    }
    /// Compile a statement list, returning one id per statement in order.
    ///
    /// # Invariant
    ///
    /// Every block-wide binding's name is entered before any value compiles, so
    /// a value may forward- or mutually reference the block's bindings; a
    /// restrictive `let` is entered during the pass, so its name is visible only
    /// to later statements.
    fn compile_scope_statements(&mut self, statements: &[Stmt]) -> Vec<ExprId> {
        // Pre-pass: reserve a `Placeholder` per block-wide binding, under its id.
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
                // Clean cell: the frozen pair, read in place; no body is lowered.
                Stmt::Binding(binding)
                    if binding
                        .binder
                        .is_some_and(|binder| self.reusable.contains_key(&binder)) =>
                {
                    let binder = binding.binder.expect("a resolved cached binding");
                    let export = self.reusable[&binder];
                    if binding.restrictive {
                        // A `let`: no placeholder was reserved, so this node is the only one.
                        let id = self.alloc(ExprKind::Static { export }, &binding.span);
                        self.set_binder(binder, id);
                        id
                    } else {
                        // A block-wide binding's reserved placeholder *is* the static read.
                        let p = self.binder(binder);
                        self.ir.set_kind(p, ExprKind::Static { export });
                        self.set_binder(binder, p);
                        p
                    }
                }
                Stmt::Binding(binding) if binding.restrictive => {
                    // `let a = e`: the value compiles before the name is recorded.
                    let id = self.compile_expr(&binding.value);
                    let binder = binding.binder.expect("a resolved let binding");
                    self.set_binder(binder, id);
                    id
                }
                Stmt::Binding(binding) => {
                    // Block-wide binding: compile the value, then fill its reserved placeholder.
                    let binder = binding.binder.expect("a resolved block-wide binding");
                    let p = self.binder(binder);
                    let value = self.compile_expr(&binding.value);
                    if matches!(&binding.value, Expr::Name(..)) {
                        // A bare name reference aliases the resolved id; re-point the placeholder.
                        self.ir.repoint(p, value);
                        self.set_binder(binder, value);
                        value
                    } else {
                        let mut kind = self.ir.expr[value.0 as usize].kind;
                        // `@loop`: stamp the transplanted `Function` kind the binding shares.
                        if binding.looping
                            && let ExprKind::Function { looping, .. } = &mut kind
                        {
                            *looping = true;
                        }
                        self.ir.set_kind(p, kind);
                        // The transplanted kind may name the now-dead `value` id: re-point it.
                        self.ir.repoint(value, p);
                        self.spans[p.0 as usize] = self.spans[value.0 as usize];
                        // The schema rides the expression, not the kind, so it is carried too.
                        self.ir.set_schema(p, self.ir.schema(value).clone());
                        p
                    }
                }
                Stmt::Expr(e) => self.compile_expr(e),
            };
            // Only a marked binding that was compiled is frozen; a clean cell is not.
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

    /// Wire the statements into the root so the checker checks and runs every one.
    ///
    /// # Invariant
    ///
    /// A trailing statement that already is the final expression's own node is
    /// dropped, so a block whose last statement is its final expression stays
    /// that expression's node.
    fn wrap(&mut self, statements: Vec<ExprId>, final_id: ExprId, span: &Span) -> ExprId {
        let mut statements = statements;
        if statements.last() == Some(&final_id) {
            statements.pop();
        }
        if statements.is_empty() {
            return final_id;
        }
        statements.push(final_id);
        // The statements ride in a tuple (per-element type slots), read positionally.
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

    /// Intern a native-operator name to a `&'static str`; the leak is `D14` in
    /// `docs/notes/code-audit.md`.
    fn intern_op(&mut self, name: &str) -> &'static str {
        if let Some(&s) = self.op_names.get(name) {
            return s;
        }
        let s: &'static str = Box::leak(name.to_string().into_boxed_str());
        self.op_names.insert(name.to_string(), s);
        s
    }

    /// Intern a source string to a `&'static str`, once per unique string (`D14`).
    fn intern_str(&mut self, s: &str) -> &'static str {
        if let Some(&leaked) = self.str_names.get(s) {
            return leaked;
        }
        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        self.str_names.insert(s.to_string(), leaked);
        leaked
    }

    /// Lower one expression; `#[stacksafe]` grows the stack instead of overflowing.
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
            // A string literal leaks its content to a `&'static str`, with no dedup (`D14`).
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
                    // An unresolved name lowers to the same inert `ErrorBlock` a parse error uses.
                    let _ = name;
                    self.alloc(ExprKind::ErrorBlock, span)
                }
            },
            Expr::Placeholder(span) => self.alloc(ExprKind::Placeholder, span),
            // A recovered parse error: an opaque [`ExprKind::ErrorBlock`], not a `_`.
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
                // Reserved before the parameter and body compile (see `Self::fn_parents`).
                let function_id = self.alloc(ExprKind::Placeholder, span);
                let parameter_id = self.alloc(ExprKind::Parameter, parameter_span);
                // A `x # n` parameter carries the perspective tail in its static schema.
                if parameter_perspective.is_some() {
                    self.ir.set_schema(
                        parameter_id,
                        Schema {
                            tail: vec![LangAttr::Perspective(Perspective)],
                        },
                    );
                }
                // The parameter is the binder the body and its own annotation resolve to.
                let binder = parameter_binder.expect("a resolved lambda parameter");
                self.set_binder(binder, parameter_id);
                // The type and attribute compile in scope too: either may name the parameter.
                let parameter_type = parameter_type.as_ref().map(|t| self.compile_expr(t));
                let parameter_attribute =
                    parameter_perspective.as_ref().map(|p| self.compile_expr(p));
                // Pushed for the body only (see `Self::fn_parents`).
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
                        // Stamped by the binding that owns this lambda, at its transplant.
                        looping: false,
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
            // `C(f1, …, fn)` with the `(` adjacent: one positional tuple, a struct instantiation.
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
                let left = self.compile_expr(left);
                let right = self.compile_expr(right);
                // The routing: a prelude in scope sends `a + b` to its binding.
                // See `docs/notes/operator-polymorphism.md` §7.
                if let Some(binder) = self.routed_operator(operator) {
                    let operands = self.alloc_array(&[left, right], span);
                    let function = self.binder(binder);
                    return self.alloc(
                        ExprKind::Apply {
                            function,
                            argument: operands,
                        },
                        span,
                    );
                }
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
                    crate::ast::BinOp::In => BinOp::In,
                };
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
                // `if c then t else e` is the lazy branch index `[e, t][c]`.
                // See `docs/notes/operator-polymorphism.md` §4.
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
                // `@assert e`: the condition itself; the span is the construct's own.
                let condition = self.compile_expr(value);
                self.alloc(ExprKind::Assert { condition }, span)
            }
            Expr::Convert {
                operator,
                value,
                span,
            } => {
                // `int2float e` / `float2int e`: the direction rides along for the checker.
                let operator = match operator {
                    crate::ast::ConvOp::Int2Float => ConvOp::Int2Float,
                    crate::ast::ConvOp::Float2Int => ConvOp::Float2Int,
                };
                let value = self.compile_expr(value);
                self.alloc(ExprKind::Convert { operator, value }, span)
            }
            Expr::NativeCall { op, args, span } => {
                // `$name(args…)`: args go into the children arena, the op name is interned.
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
                refinement,
                doc,
                span,
            } => {
                let value = self.compile_expr(value);
                let r#type = r#type.as_ref().map(|t| self.compile_expr(t));
                // Values are laid out in the canonical attribute order, aligned with the tail.
                let mut spelled: Vec<(LangAttr, ExprId)> = Vec::new();
                if let Some(p) = &perspective {
                    spelled.push((LangAttr::Perspective(Perspective), self.compile_expr(p)));
                }
                if let Some(d) = &doc {
                    spelled.push((LangAttr::Doc(Doc), self.compile_expr(d)));
                }
                if let Some(r) = &refinement {
                    spelled.push((LangAttr::Refinement(Refinement), self.compile_expr(r)));
                }
                spelled.sort_by_key(|(marker, _)| marker.order_index());
                let tail: Vec<LangAttr> = spelled.iter().map(|(marker, _)| *marker).collect();
                let attrs: Vec<ExprId> = spelled.into_iter().map(|(_, value)| value).collect();
                let id = self.alloc_annotation(value, r#type, &attrs, span);
                // The attribute tail stamps the annotated node's static schema.
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
                // A `~`-marked element contributes its inner expression plus a depth.
                let mut ids = Vec::with_capacity(elements.len());
                let mut depths = Vec::with_capacity(elements.len());
                for element in elements {
                    match element {
                        Expr::Shallow(inner, depth, _) => {
                            ids.push(self.compile_expr(inner));
                            depths.push(*depth);
                        }
                        // Every other kind is a plain element; exhaustive, to force classification.
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
                        | Expr::Set(..)
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
                // Each entry's key and value are their own nodes; the pairs feed the Table kind.
                let mut pairs = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    pairs.push((self.compile_expr(key), self.compile_expr(value)));
                }
                self.alloc_table(&pairs, span)
            }
            Expr::Set(members, span) => {
                // The members compile like array elements; the kind makes it a set.
                let ids = self.compile_all(members);
                self.alloc_set(&ids, span)
            }
            Expr::Shallow(inner, _, _) => {
                // Unreachable through the parser; compiles the inner expression to stay total.
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
                // Block-scoped, but otherwise like a program's statements (see `Self::wrap`).
                let stmts = self.compile_scope_statements(statements);
                let body = self.compile_expr(expr);
                self.wrap(stmts, body, span)
            }
            Expr::RecordBlock { fields, span } => {
                // A struct-returning block: an anonymous struct of its field statements.
                let (_, root) = self.compile_record_fields(fields, span);
                root
            }
        }
    }

    /// Compile a record block's fields, shared by `RecordBlock` and a record program.
    ///
    /// # Invariant
    ///
    /// The returned ids are the field values in source order, including the
    /// non-emitted `let` locals; only `pub` fields are emitted when any is `pub`.
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
            // A `let` local is never a field; only `pub` fields when any is `pub`.
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
