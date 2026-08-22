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
//! Shadowing is allowed (the inner binding wins); an unknown name is a
//! resolve diagnostic — the checker's `lookup` panics on unresolved ids, so
//! resolution completes here.  Every emitted expression carries its source
//! span.

use std::collections::HashMap;

use lichen_highlevel::ir::{Constant, ExprId, ExprKind, IR, Span};

use crate::ast::{Expr, Program, TypeConst};
use crate::diag::{Diag, Stage};

pub fn compile(program: &Program) -> Result<IR, Diag> {
    let mut compiler = Compiler {
        ir: IR::new(),
        scopes: Vec::new(),
    };
    // Each binding compiles its value once and enters the scope; the scope
    // is never popped, so later statements (and the final expression) see
    // every earlier binding.
    for binding in &program.bindings {
        let id = compiler.compile_expr(&binding.value)?;
        compiler.scopes.push(HashMap::from([(binding.name.clone(), id)]));
    }
    let id = compiler.compile_expr(&program.expr)?;
    compiler.ir.set_root(id);
    Ok(compiler.ir)
}

struct Compiler {
    ir: IR,
    /// The in-scope binders, innermost last.
    scopes: Vec<HashMap<String, ExprId>>,
}

impl Compiler {
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
                r#return,
                span,
            } => {
                let parameter_id = self.alloc(ExprKind::Parameter, parameter_span);
                self.scopes
                    .push(HashMap::from([(parameter.clone(), parameter_id)]));
                let body = self.compile_expr(r#return);
                self.scopes.pop();
                let body = body?;
                self.alloc(
                    ExprKind::Function {
                        parameter: parameter_id,
                        r#return: body,
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
}
