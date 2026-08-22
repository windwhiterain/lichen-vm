//! AST → IR compilation with name resolution.
//!
//! A use of a name *is* the binder's own `ExprId`: compiling `x => e`
//! allocates the `Parameter` expression first (span = the name's), pushes `x`
//! on a scope stack, compiles `e`, then wraps `Function { parameter, return }`.
//! A statement binding `a = e` is the same resolution without a lambda: the
//! value compiles to one `ExprId`, the scope maps `a` to it, and every use of
//! `a` is that same id — the IR is a graph and the binding is pure sharing
//! (no `let`, no desugared application).  The IR therefore carries no name
//! strings, and the checker's scope stack is keyed by the same ids.
//! A recursive binding `rec fib = n => e` reverses the order: the id is
//! allocated *first* (its kind filled in after the body compiles), the name
//! is entered, and a use of `fib` inside the body resolves to the id being
//! defined — the IR becomes a cycle there, recorded in `IR.recursive` so the
//! checker registers the function's pair before its body compiles.
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

use lichen_highlevel::ir::{BinOp, Constant, ExprId, ExprKind, IR, Span};

use crate::ast::{Binding, Expr, Program, Stmt, TypeConst};
use crate::diag::{Diag, Stage};

pub fn compile(program: &Program) -> Result<IR, Diag> {
    let mut compiler = Compiler {
        ir: IR::new(),
        scopes: Vec::new(),
    };
    // Each statement compiles once and enters the scope (bindings) or just
    // compiles (a bare expression); the scope is never popped, so later
    // statements (and the final expression) see every earlier binding.
    let mut statements = Vec::new();
    for stmt in &program.statements {
        statements.push(compiler.compile_stmt(stmt)?);
    }
    let final_id = compiler.compile_expr(&program.expr)?;
    let root = compiler.wrap(statements, final_id, &program.expr.span());
    compiler.ir.set_root(root);
    Ok(compiler.ir)
}

struct Compiler {
    ir: IR,
    /// The in-scope binders, innermost last.
    scopes: Vec<HashMap<String, ExprId>>,
}

impl Compiler {
    fn compile_stmt(&mut self, stmt: &Stmt) -> Result<ExprId, Diag> {
        match stmt {
            Stmt::Binding(binding) if binding.recursive => {
                // The name is entered *before* the value compiles — the
                // body may reference it — and stays in scope for later
                // statements, exactly like an ordinary binding.  The id is
                // reserved first; the value fills it in below.
                let id = self.alloc(ExprKind::Placeholder, &binding.span);
                self.ir.recursive.insert(id);
                self.scopes
                    .push(HashMap::from([(binding.name.clone(), id)]));
                let value = self.compile_rec_value(binding, id)?;
                // The body resolved the name to the function id; later
                // statements resolve it to the value itself (a desugared
                // annotation wrapper, when the parameter was annotated).
                self.scopes
                    .last_mut()
                    .expect("the recursive binding's frame")
                    .insert(binding.name.clone(), value);
                Ok(value)
            }
            Stmt::Binding(binding) => {
                let id = self.compile_expr(&binding.value)?;
                self.scopes
                    .push(HashMap::from([(binding.name.clone(), id)]));
                Ok(id)
            }
            Stmt::Expr(e) => self.compile_expr(e),
        }
    }

    /// The value of a recursive binding, compiled into the reserved `id`:
    /// `rec fib = n => e`.  A use of `fib` inside
    /// the body resolves to `id` — the IR is a cycle there, recorded in
    /// [`IR::recursive`] so the checker registers the function's pair before
    /// the body compiles.  The value must be a lambda.  An annotated
    /// parameter desugars exactly like a plain lambda's — `n : Int => e` is
    /// `(n => e) : (Int -> _)`, the annotation wrapping the function node
    /// (the checker's `check_ann` unifies the function's arrow against the
    /// annotation's; the `_` placeholder binds lazily, so no pending
    /// computation is forced at check time).
    fn compile_rec_value(&mut self, binding: &Binding, id: ExprId) -> Result<ExprId, Diag> {
        let Expr::Lambda {
            parameter,
            parameter_span,
            parameter_type,
            r#return,
            span,
        } = &binding.value
        else {
            return Err(Diag::new(
                Stage::Resolve,
                binding.span,
                format!(
                    "a recursive binding's value must be a lambda ('{}')",
                    binding.name
                ),
            ));
        };
        let parameter_id = self.alloc(ExprKind::Parameter, parameter_span);
        self.scopes
            .push(HashMap::from([(parameter.clone(), parameter_id)]));
        // The annotated parameter's type is compiled in scope too — a type
        // may reference the parameter (`x : x -> Int`).
        let body = self.compile_expr(r#return);
        let parameter_type = parameter_type
            .as_ref()
            .map(|t| self.compile_expr(t))
            .transpose();
        self.scopes.pop();
        let body = body?;
        self.ir.expr[id.0 as usize].kind = ExprKind::Function {
            parameter: parameter_id,
            r#return: body,
        };
        self.ir.expr[id.0 as usize].span = Some(*span);
        match parameter_type? {
            // `rec f = n : T => e`  ≡  `rec f = (n => e) : (T -> _)` — the
            // parameter annotation is an ordinary annotation of the lambda
            // with an arrow whose codomain is inferred.
            Some(t) => {
                let placeholder = self.alloc(ExprKind::Placeholder, span);
                let arrow = self.alloc(
                    ExprKind::TypeFunction {
                        parameter: t,
                        r#return: placeholder,
                    },
                    span,
                );
                Ok(self.alloc(
                    ExprKind::Annotation {
                        value: id,
                        r#type: arrow,
                    },
                    span,
                ))
            }
            None => Ok(id),
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
            ExprKind::Constant(Constant::USize(statements.len() - 1)),
            Some(*span),
        );
        self.ir
            .alloc(ExprKind::Index { array: tuple, index }, Some(*span))
    }
    fn compile_expr(&mut self, e: &Expr) -> Result<ExprId, Diag> {
        let id = match e {
            Expr::Int(n, span) => self.alloc(ExprKind::Constant(Constant::USize(*n)), span),
            Expr::TypeConst(TypeConst::Int, span) => {
                self.alloc(ExprKind::Constant(Constant::TypeInt), span)
            }
            Expr::TypeConst(TypeConst::Type, span) => {
                self.alloc(ExprKind::Constant(Constant::TypeType), span)
            }
            Expr::Name(name, span) => self.lookup(name).ok_or_else(|| {
                Diag::new(Stage::Resolve, *span, format!("unresolved name '{name}'"))
            })?,
            Expr::Placeholder(span) => self.alloc(ExprKind::Placeholder, span),
            Expr::Lambda {
                parameter,
                parameter_span,
                parameter_type,
                r#return,
                span,
            } => {
                let parameter_id = self.alloc(ExprKind::Parameter, parameter_span);
                self.scopes
                    .push(HashMap::from([(parameter.clone(), parameter_id)]));
                // The annotated parameter's type is compiled in scope too —
                // a type may reference the parameter (`x : x -> Int`).
                let body = self.compile_expr(r#return);
                let parameter_type = parameter_type
                    .as_ref()
                    .map(|t| self.compile_expr(t))
                    .transpose();
                self.scopes.pop();
                let body = body?;
                let function = self.alloc(
                    ExprKind::Function {
                        parameter: parameter_id,
                        r#return: body,
                    },
                    span,
                );
                match parameter_type? {
                    // `x : T => e`  ≡  `(x => e) : (T -> _)` — the parameter
                    // annotation is an ordinary annotation of the lambda
                    // with an arrow whose codomain is inferred.
                    Some(t) => {
                        let placeholder = self.alloc(ExprKind::Placeholder, span);
                        let arrow = self.alloc(
                            ExprKind::TypeFunction {
                                parameter: t,
                                r#return: placeholder,
                            },
                            span,
                        );
                        self.alloc(
                            ExprKind::Annotation {
                                value: function,
                                r#type: arrow,
                            },
                            span,
                        )
                    }
                    None => function,
                }
            }
            Expr::Apply {
                function,
                argument,
                span,
            } => {
                let function = self.compile_expr(function)?;
                let argument = self.compile_expr(argument)?;
                // An application whose callee is a struct type is
                // instantiation, not function application: `s(1, 2)` with
                // `s` bound to `struct<Int, Int>` wraps the tuple in the
                // nominal type.  The callee is recognized by its IR node —
                // the literal `struct<...>` or a name that resolved to one.
                if matches!(self.ir[function].kind, ExprKind::TypeStruct(_)) {
                    self.alloc(
                        ExprKind::Instantiate {
                            type_expr: function,
                            value: argument,
                        },
                        span,
                    )
                } else {
                    self.alloc(ExprKind::Apply { function, argument }, span)
                }
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
                let branches = self.ir.alloc_array(&[else_branch, then_branch], Some(*span));
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
                let ids = self.compile_all(elements)?;
                self.ir.alloc_array(&ids, Some(*span))
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
            Expr::Block { statements, expr, span } => {
                // The same graph sharing as a program's statements — each
                // value compiles once and names resolve to its own id — but
                // the block's scope frames are dropped at the `}`, so a
                // block compiles to its final expression's own node (wired
                // through the statement wrapper like a program's).
                let scope_len = self.scopes.len();
                let mut stmts = Vec::new();
                for stmt in statements {
                    stmts.push(self.compile_stmt(stmt)?);
                }
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

    fn alloc(&mut self, kind: ExprKind, span: &Span) -> ExprId {
        self.ir.alloc(kind, Some(*span))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::lex;
    use crate::parse::parse;

    fn compile_ok(source: &str) -> IR {
        let tokens = lex(source).unwrap();
        let ast = parse(&tokens).unwrap();
        compile(&ast).unwrap()
    }

    fn compile_err(source: &str) -> Diag {
        let tokens = lex(source).unwrap();
        let ast = parse(&tokens).unwrap();
        compile(&ast).unwrap_err()
    }

    fn kind(ir: &IR, id: ExprId) -> ExprKind {
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
                let ExprKind::Constant(Constant::USize(n)) = ir[index].kind else {
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
            ExprKind::Constant(Constant::USize(5))
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
            ExprKind::Constant(Constant::USize(1))
        ));
        // The same holds through a lambda body: x => {y = x; y} is the
        // identity function, whose return is the parameter itself.
        let ir = compile_ok("x => {y = x; y}");
        let ExprKind::Function {
            parameter,
            r#return,
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
            ExprKind::Constant(Constant::USize(1))
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
            ExprKind::Constant(Constant::USize(1))
        ));
        assert!(matches!(
            kind(&ir, argument),
            ExprKind::Constant(Constant::USize(2))
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
            ExprKind::Constant(Constant::USize(1))
        ));
        assert!(matches!(kind(&ir, wrapped(&ir)), ExprKind::Constant(Constant::USize(7))));
        // A trailing statement identical to the final expression is not
        // wrapped: `a = 1; a` stays the `1` node.
        let ir = compile_ok("a = 1; a");
        assert!(matches!(
            kind(&ir, ir.root),
            ExprKind::Constant(Constant::USize(1))
        ));
        // A bare expression statement between bindings is compiled too.
        let ir = compile_ok("a = 1; 5; a");
        let ExprKind::Tuple(range) = kind(&ir, match kind(&ir, ir.root) {
            ExprKind::Index { array, .. } => array,
            _ => panic!("expected the statement wrapper"),
        }) else {
            panic!("expected the wrapped tuple")
        };
        assert_eq!(range.end - range.start, 3);
    }

    #[test]
    fn an_annotated_parameter_desugars_to_an_annotation() {
        // x : Int => x  ≡  (x => x) : (Int -> _).
        let ir = compile_ok("x : Int => x");
        let ExprKind::Annotation { value, r#type } = kind(&ir, ir.root) else {
            panic!("expected an annotation")
        };
        let ExprKind::Function { parameter, r#return } = kind(&ir, value) else {
            panic!("expected the function")
        };
        assert_eq!(r#return, parameter, "the identity's body is the parameter");
        let ExprKind::TypeFunction {
            parameter: domain,
            r#return: codomain,
        } = kind(&ir, r#type)
        else {
            panic!("expected an arrow type")
        };
        assert!(matches!(
            kind(&ir, domain),
            ExprKind::Constant(Constant::TypeInt)
        ));
        assert!(matches!(kind(&ir, codomain), ExprKind::Placeholder));
        // An unannotated lambda desugars to nothing.
        let ir = compile_ok("x => x");
        assert!(matches!(kind(&ir, ir.root), ExprKind::Function { .. }));
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
