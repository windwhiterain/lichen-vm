//! AST → IR compilation with name resolution.
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
//! binary operation `a op b` compiles to `ExprKind::BinOp`.
//! Shadowing is allowed (the inner binding wins); an unknown name is a
//! resolve diagnostic — the checker's `lookup` panics on unresolved ids, so
//! resolution completes here.  Every emitted expression carries its source
//! span.

use std::collections::HashMap;

use lichen_highlevel::ir::{BinOp, ExprId, ExprKind, IR, Span};
use lichen_highlevel::program::{HighProgramValue, TypeValue};
use lichen_lowlevel::LowValue;

use crate::ast::{Expr, Program, Stmt, TypeConst};
use crate::diag::{Diag, Stage};
use crate::preprocess::ResolvedImport;

pub fn compile(program: &Program) -> Result<IR, Diag> {
    compile_with_imports(program, &[])
}

/// Compile a parsed program with resolved imports pre-seeded in the first
/// scope frame.  Local bindings may shadow an imported name because the
/// import frame is below the block-wide binding frames.
pub fn compile_with_imports(program: &Program, imports: &[ResolvedImport]) -> Result<IR, Diag> {
    let mut compiler = Compiler {
        ir: IR::new(),
        scopes: Vec::new(),
        fn_depth: 0,
    };
    if !imports.is_empty() {
        let mut frame = HashMap::new();
        for import in imports {
            let id = compiler.alloc(
                ExprKind::Static {
                    export: import.export,
                },
                &import.span,
            );
            frame.insert(import.name.clone(), id);
        }
        compiler.scopes.push(frame);
    }
    // The whole program is one scope: block-wide bindings are entered before
    // any value compiles, restrictive `let` bindings are entered as they're
    // seen; the scope is never popped, so later statements (and the final
    // expression) see every earlier binding.
    let statements = compiler.compile_scope_statements(&program.statements)?;
    let final_id = compiler.compile_expr(&program.expr)?;
    let root = compiler.wrap(statements, final_id, &program.expr.span());
    compiler.ir.set_root(root);
    Ok(compiler.ir)
}

struct Compiler {
    ir: IR,
    /// The in-scope binders, innermost last.
    scopes: Vec<HashMap<String, ExprId>>,
    /// The count of enclosing function scopes at the current compilation
    /// point — the `depth` carried on each lambda's [`ExprKind::Function`],
    /// which the checker uses to make sibling functions' template scopes
    /// disjoint while absorbing nested closures into their parent's scope.
    /// Incremented only around a lambda's `r#return` compilation (a lambda's
    /// own depth is the value *before* its body opens).
    fn_depth: usize,
}

impl Compiler {
    /// Compile a statement list, entering every block-wide binding's name
    /// *before* any value compiles so a value may forward- or mutually-
    /// reference the block's bindings.  Restrictive `let` bindings are
    /// entered during the pass (their value compiles first, so the name is
    /// visible only to later statements).
    fn compile_scope_statements(&mut self, statements: &[Stmt]) -> Result<Vec<ExprId>, Diag> {
        // Pre-pass: reserve a `Placeholder` per block-wide binding and enter
        // the name, in one frame.  A scope's block-wide bindings are mutually
        // visible in both directions.
        let mut frame = HashMap::new();
        for stmt in statements {
            if let Stmt::Binding(binding) = stmt
                && !binding.restrictive
            {
                let p = self.alloc(ExprKind::Placeholder, &binding.span);
                frame.insert(binding.name.clone(), p);
                self.ir.block_roots.insert(p);
            }
        }
        if !frame.is_empty() {
            self.scopes.push(frame);
        }

        // Compile pass: each statement becomes one id in order.
        let mut out = Vec::new();
        for stmt in statements {
            out.push(match stmt {
                Stmt::Binding(binding) if binding.restrictive => {
                    // `let a = e` — the value compiles before the name enters
                    // scope, so it is visible only to later statements and
                    // never to itself.  `let a = a` resolves `a` to the outer
                    // (or block-wise) binding, exactly the sequential case.
                    let id = self.compile_expr(&binding.value)?;
                    self.scopes
                        .push(HashMap::from([(binding.name.clone(), id)]));
                    id
                }
                Stmt::Binding(binding) => {
                    // Block-wide binding: the name already maps to the
                    // reserved placeholder `p`.  Compile the value; if the
                    // value *is* a block-wide placeholder (a bare name
                    // reference, e.g. `b = a` or the degenerate `a = a`),
                    // alias the name to it — otherwise transplant the value's
                    // kind into `p` so the placeholder becomes the value node
                    // and any self/mutual reference (which resolves to `p`)
                    // points at the value.
                    let p = self
                        .lookup(&binding.name)
                        .expect("a block-wide binding's name is pre-entered");
                    let value = self.compile_expr(&binding.value)?;
                    if matches!(&binding.value, Expr::Name(..)) {
                        // A bare name reference (`b = a`, `y = x`, and the
                        // degenerate `a = a`): share the resolved id rather
                        // than copying the kind, so the binding aliases it.
                        self.remap(&binding.name, p, value);
                        value
                    } else {
                        self.ir.expr[p.0 as usize].kind = self.ir.expr[value.0 as usize].kind;
                        self.ir.expr[p.0 as usize].span = self.ir.expr[value.0 as usize].span;
                        p
                    }
                }
                Stmt::Expr(e) => self.compile_expr(e)?,
            });
        }
        Ok(out)
    }

    /// Change the mapping of `name` from the reserved placeholder `p` to `to`
    /// (the shared value), in the frame holding that placeholder.  The
    /// placeholder is unique to this binding, so the innermost frame whose
    /// `name` maps to exactly `p` is the pre-pass frame; a restrictive `let`
    /// frame shadowing the same name maps it to a different id and is left
    /// alone (it stays shadowed, as intended).
    fn remap(&mut self, name: &str, p: ExprId, to: ExprId) {
        for frame in self.scopes.iter_mut().rev() {
            if frame.get(name) == Some(&p) {
                frame.insert(name.to_string(), to);
                return;
            }
        }
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
        let tuple = self.ir.alloc_tuple(&statements, Some(*span));
        let index = self.ir.alloc(
            ExprKind::Constant(HighProgramValue::from(LowValue::USize(
                statements.len() - 1,
            ))),
            Some(*span),
        );
        self.ir.alloc(
            ExprKind::Index {
                array: tuple,
                index,
            },
            Some(*span),
        )
    }
    fn compile_expr(&mut self, e: &Expr) -> Result<ExprId, Diag> {
        let id = match e {
            Expr::Int(n, span) => self.alloc(
                ExprKind::Constant(HighProgramValue::from(LowValue::USize(*n))),
                span,
            ),
            Expr::TypeConst(TypeConst::Int, span) => self.alloc(
                ExprKind::Constant(HighProgramValue::TypeValue(TypeValue::TypeInt)),
                span,
            ),
            Expr::TypeConst(TypeConst::Type, span) => self.alloc(
                ExprKind::Constant(HighProgramValue::TypeValue(TypeValue::TypeType)),
                span,
            ),
            Expr::Name(name, span) => self.lookup(name).ok_or_else(|| {
                Diag::new(Stage::Resolve, *span, format!("unresolved name '{name}'"))
            })?,
            Expr::Placeholder(span) => self.alloc(ExprKind::Placeholder, span),
            // A recovered parse error: a compile-time leaf — an inference
            // placeholder, so the partial program still compiles and checks
            // (the parse diagnostic is reported alongside; the placeholder's
            // type is inferred, never a source of spurious check errors).
            Expr::Err(span) => self.alloc(ExprKind::Placeholder, span),
            Expr::Lambda {
                parameter,
                parameter_span,
                parameter_type,
                r#return,
                span,
            } => {
                let depth = self.fn_depth as u32;
                let parameter_id = self.alloc(ExprKind::Parameter, parameter_span);
                self.scopes
                    .push(HashMap::from([(parameter.clone(), parameter_id)]));
                // The annotated parameter's type is compiled in scope too —
                // a type may reference the parameter (`x : x -> Int`).
                let body = {
                    self.fn_depth += 1;
                    let body = self.compile_expr(r#return);
                    self.fn_depth -= 1;
                    body
                };
                let parameter_type = parameter_type
                    .as_ref()
                    .map(|t| self.compile_expr(t))
                    .transpose();
                self.scopes.pop();
                let body = body?;
                // `x : T => e` — the annotated type rides the `Function`
                // itself instead of an outer `(x => e) : (T -> _)`
                // annotation: the checker compiles it in body scope, so
                // in-body readers of the parameter see the annotated kind
                // (an array annotation's length, a function annotation's
                // arrow) while the parameter's type slot still performs the
                // argument check at each apply.  An unannotated parameter
                // is `None`.
                let parameter_type = parameter_type?;
                self.alloc(
                    ExprKind::Function {
                        parameter: parameter_id,
                        parameter_type,
                        r#return: body,
                        depth,
                    },
                    span,
                )
            }
            Expr::Apply {
                function,
                argument,
                span,
            } => {
                let function = self.compile_expr(function)?;
                let argument = self.compile_expr(argument)?;
                self.alloc(ExprKind::Apply { function, argument }, span)
            }
            // Struct instantiation is a *syntactic* form now — `C(f1, …, fn)`
            // with the `(` adjacent to the callee (no space).  It always wraps
            // the field values in one positional tuple and lowers to
            // [`ExprKind::Instantiate`]; a spaced `C (…)` is a plain apply
            // and reaches the Apply arm above.  There is no compile-time
            // callee-kind dispatch — the checker decides whether the callee
            // is a struct type, and a callee that is not one fails there.
            Expr::StructInst {
                callee,
                fields,
                span,
            } => {
                let type_expr = self.compile_expr(callee)?;
                let field_ids = self.compile_all(fields)?;
                let value = self.ir.alloc_tuple(&field_ids, Some(*span));
                self.alloc(ExprKind::Instantiate { type_expr, value }, span)
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
                    crate::ast::BinOp::Leq => BinOp::Leq,
                    crate::ast::BinOp::Eq => BinOp::Eq,
                };
                let left = self.compile_expr(left)?;
                let right = self.compile_expr(right)?;
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
                let condition = self.compile_expr(condition)?;
                let then_branch = self.compile_expr(then_branch)?;
                let else_branch = self.compile_expr(else_branch)?;
                let branches = self
                    .ir
                    .alloc_array(&[else_branch, then_branch], Some(*span));
                self.alloc(
                    ExprKind::Index {
                        array: branches,
                        index: condition,
                    },
                    span,
                )
            }
            Expr::Annotation {
                value,
                r#type,
                span,
            } => {
                let value = self.compile_expr(value)?;
                let r#type = self.compile_expr(r#type)?;
                self.alloc(ExprKind::Annotation { value, r#type }, span)
            }
            Expr::Index { array, index, span } => {
                let array = self.compile_expr(array)?;
                let index = self.compile_expr(index)?;
                self.alloc(ExprKind::Index { array, index }, span)
            }
            Expr::Arrow {
                parameter,
                r#return,
                span,
            } => {
                let parameter = self.compile_expr(parameter)?;
                let r#return = self.compile_expr(r#return)?;
                self.alloc(
                    ExprKind::TypeFunction {
                        parameter,
                        r#return,
                    },
                    span,
                )
            }
            Expr::Tuple(elements, span) => {
                let ids = self.compile_all(elements)?;
                self.ir.alloc_tuple(&ids, Some(*span))
            }
            Expr::TypeTuple(elements, span) => {
                let ids = self.compile_all(elements)?;
                self.ir.alloc_type_tuple(&ids, Some(*span))
            }
            Expr::StructType(fields, span) => {
                let ids = self.compile_all(fields)?;
                self.ir.alloc_type_struct(&ids, Some(*span))
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
                            ids.push(self.compile_expr(inner)?);
                            depths.push(*depth);
                        }
                        _ => {
                            ids.push(self.compile_expr(element)?);
                            depths.push(0);
                        }
                    }
                }
                if depths.iter().any(|&d| d != 0) {
                    let elements: Vec<(ExprId, usize)> = ids.into_iter().zip(depths).collect();
                    self.ir.alloc_shallow_array(&elements, Some(*span))
                } else {
                    self.ir.alloc_array(&ids, Some(*span))
                }
            }
            Expr::Shallow(inner, _, _) => {
                // Unreachable through the parser (`~` is accepted only as an
                // array element, which the `Array` arm unwraps); compile the
                // inner expression defensively so the match stays total.
                self.compile_expr(inner)?
            }
            Expr::TypeArray {
                element_type,
                length,
                span,
            } => {
                let element_type = self.compile_expr(element_type)?;
                let length = self.compile_expr(length)?;
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
                // value compiles once and names resolve to its own id — but
                // the block's scope frames are dropped at the `}`, so a
                // block compiles to its final expression's own node (wired
                // through the statement wrapper like a program's).
                let scope_len = self.scopes.len();
                let stmts = self.compile_scope_statements(statements)?;
                let body = self.compile_expr(expr);
                self.scopes.truncate(scope_len);
                let body = body?;
                self.wrap(stmts, body, span)
            }
        };
        Ok(id)
    }

    fn compile_all(&mut self, elements: &[Expr]) -> Result<Vec<ExprId>, Diag> {
        elements.iter().map(|e| self.compile_expr(e)).collect()
    }

    fn lookup(&self, name: &str) -> Option<ExprId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn alloc(&mut self, kind: ExprKind<HighProgramValue>, span: &Span) -> ExprId {
        self.ir.alloc(kind, Some(*span))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::lex;
    use crate::parse::parse;

    fn compile_ok(source: &str) -> IR {
        let tokens = lex(source).tokens;
        let ast = parse(&tokens).program;
        compile(&ast).unwrap()
    }

    fn compile_err(source: &str) -> Diag {
        let tokens = lex(source).tokens;
        let ast = parse(&tokens).program;
        compile(&ast).unwrap_err()
    }

    fn kind(ir: &IR, id: ExprId) -> ExprKind<HighProgramValue> {
        ir[id].kind
    }

    /// The node the statement wrapper selects: the root is either the final
    /// expression's own node (no wrap) or `Index(Tuple([…, final]), n)`,
    /// which unwraps to the final expression.
    fn wrapped(ir: &IR) -> ExprId {
        match ir[ir.root].kind {
            ExprKind::Index { array, index } => {
                let ExprKind::Tuple(range) = ir[array].kind else {
                    panic!("expected the wrapped tuple")
                };
                let ExprKind::Constant(HighProgramValue::LowValue(LowValue::USize(n))) =
                    ir[index].kind
                else {
                    panic!("expected a constant index")
                };
                ir.children[range.start as usize + n]
            }
            _ => ir.root,
        }
    }

    #[test]
    fn a_use_is_the_binders_own_id() {
        // x => x — the body's use is the parameter itself.
        let ir = compile_ok("x => x");
        let ExprKind::Function {
            parameter,
            r#return,
            ..
        } = kind(&ir, ir.root)
        else {
            panic!("expected a function")
        };
        assert_eq!(r#return, parameter, "the use is the parameter's own id");
        assert!(matches!(kind(&ir, parameter), ExprKind::Parameter));
    }

    #[test]
    fn let_is_just_application() {
        // (x => x) 5 — a desugared binding needs no special form.
        let ir = compile_ok("(x => x) 5");
        let ExprKind::Apply { function, argument } = kind(&ir, ir.root) else {
            panic!("expected an apply")
        };
        assert!(matches!(kind(&ir, function), ExprKind::Function { .. }));
        assert!(matches!(
            kind(&ir, argument),
            ExprKind::Constant(HighProgramValue::LowValue(LowValue::USize(5)))
        ));
    }

    #[test]
    fn shadowing_resolves_to_the_inner_binder() {
        // x => (x => x) — the inner use refers to the inner parameter.
        let ir = compile_ok("x => (x => x)");
        let ExprKind::Function {
            r#return: outer_body,
            ..
        } = kind(&ir, ir.root)
        else {
            panic!("expected a function")
        };
        let ExprKind::Function {
            parameter: inner,
            r#return,
            ..
        } = kind(&ir, outer_body)
        else {
            panic!("expected the inner function")
        };
        assert_eq!(r#return, inner, "the use resolves to the inner binder");
    }

    #[test]
    fn every_expression_carries_a_span() {
        let ir = compile_ok("x => x");
        for expr in &ir.expr {
            assert!(expr.span.is_some(), "expression {expr:?} lost its span");
        }
    }

    #[test]
    fn an_unresolved_name_is_a_resolve_diagnostic() {
        let err = compile_err("x => y");
        assert_eq!(err.stage, Stage::Resolve);
        assert_eq!(err.message, "unresolved name 'y'");
        assert_eq!(err.span, Some((1, 6)));
    }

    #[test]
    fn a_type_position_underscore_compiles_to_a_placeholder() {
        // x => x : _ — the annotation's type is the placeholder kind.
        let ir = compile_ok("x => x : _");
        let ExprKind::Function { r#return, .. } = kind(&ir, ir.root) else {
            panic!("expected a function")
        };
        let ExprKind::Annotation { r#type, .. } = kind(&ir, r#return) else {
            panic!("expected an annotation")
        };
        assert!(matches!(kind(&ir, r#type), ExprKind::Placeholder));
    }

    #[test]
    fn a_block_compiles_to_its_final_expression() {
        // {a = 1; a} — the block is its final expression's own node; the
        // binding is pure sharing, not a new IR form.
        let ir = compile_ok("{a = 1; a}");
        assert!(matches!(
            kind(&ir, ir.root),
            ExprKind::Constant(HighProgramValue::LowValue(LowValue::USize(1)))
        ));
        // The same holds through a lambda body: x => {y = x; y} is the
        // identity function, whose return is the parameter itself.
        let ir = compile_ok("x => {y = x; y}");
        let ExprKind::Function {
            parameter,
            r#return,
            ..
        } = kind(&ir, ir.root)
        else {
            panic!("expected a function")
        };
        assert_eq!(r#return, parameter, "the block is the parameter's own id");
    }

    #[test]
    fn a_block_scopes_its_bindings() {
        // a = 2; {a = 1; a} — inside the block the name is the inner
        // binding.  The program's own binding (the `2`) is wrapped into the
        // root; the block unwraps to the `1` node.
        let ir = compile_ok("a = 2; {a = 1; a}");
        assert!(matches!(
            kind(&ir, wrapped(&ir)),
            ExprKind::Constant(HighProgramValue::LowValue(LowValue::USize(1)))
        ));
        // After the `}`, the block's bindings are gone and the outer name
        // resolves again: `{a = 1; a} a` applies the block (the `1` node) to
        // the outer `a` (the `2` node).
        let ir = compile_ok("a = 2; {a = 1; a} a");
        let ExprKind::Apply { function, argument } = kind(&ir, wrapped(&ir)) else {
            panic!("expected an apply")
        };
        assert!(matches!(
            kind(&ir, function),
            ExprKind::Constant(HighProgramValue::LowValue(LowValue::USize(1)))
        ));
        assert!(matches!(
            kind(&ir, argument),
            ExprKind::Constant(HighProgramValue::LowValue(LowValue::USize(2)))
        ));
    }

    #[test]
    fn a_statement_expression_is_wired_into_the_root() {
        // 5; 7 — the bare statement and the final expression ride in a
        // tuple; the root selects the final one.
        let ir = compile_ok("5; 7");
        let ExprKind::Index { array, index } = kind(&ir, ir.root) else {
            panic!("expected the statement wrapper")
        };
        assert!(matches!(
            kind(&ir, array),
            ExprKind::Tuple(range) if range.end - range.start == 2
        ));
        assert!(matches!(
            kind(&ir, index),
            ExprKind::Constant(HighProgramValue::LowValue(LowValue::USize(1)))
        ));
        assert!(matches!(
            kind(&ir, wrapped(&ir)),
            ExprKind::Constant(HighProgramValue::LowValue(LowValue::USize(7)))
        ));
        // A trailing statement identical to the final expression is not
        // wrapped: `a = 1; a` stays the `1` node.
        let ir = compile_ok("a = 1; a");
        assert!(matches!(
            kind(&ir, ir.root),
            ExprKind::Constant(HighProgramValue::LowValue(LowValue::USize(1)))
        ));
        // A bare expression statement between bindings is compiled too.
        let ir = compile_ok("a = 1; 5; a");
        let ExprKind::Tuple(range) = kind(
            &ir,
            match kind(&ir, ir.root) {
                ExprKind::Index { array, .. } => array,
                _ => panic!("expected the statement wrapper"),
            },
        ) else {
            panic!("expected the wrapped tuple")
        };
        assert_eq!(range.end - range.start, 3);
    }

    #[test]
    fn an_annotated_parameter_rides_the_function() {
        // x : Int => x — the annotation is the parameter's in-scope type on
        // the `Function` itself, not an outer annotation of the lambda.
        let ir = compile_ok("x : Int => x");
        let ExprKind::Function {
            parameter,
            parameter_type,
            r#return,
            ..
        } = kind(&ir, ir.root)
        else {
            panic!("expected a function")
        };
        assert_eq!(r#return, parameter, "the identity's body is the parameter");
        assert!(matches!(
            kind(&ir, parameter_type.expect("the annotated type")),
            ExprKind::Constant(HighProgramValue::TypeValue(TypeValue::TypeInt))
        ));
        // An unannotated lambda carries no parameter type.
        let ir = compile_ok("x => x");
        let ExprKind::Function { parameter_type, .. } = kind(&ir, ir.root) else {
            panic!("expected a function")
        };
        assert!(parameter_type.is_none());
    }

    #[test]
    fn an_unresolved_name_inside_a_block_is_a_resolve_diagnostic() {
        let err = compile_err("{a = 1; b}");
        assert_eq!(err.stage, Stage::Resolve);
        assert_eq!(err.message, "unresolved name 'b'");
        assert_eq!(err.span, Some((1, 9)));
        // A block's bindings don't leak out: the name is unresolved after `}`.
        let err = compile_err("{a = 1; a} a");
        assert_eq!(err.message, "unresolved name 'a'");
    }
}
