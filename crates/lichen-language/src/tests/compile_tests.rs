use super::*;
use crate::lex::lex;
use crate::parse::parse;
use lichen_highlevel::program::{HighProgramLiteral, IntLit, IntTypeLit};

fn compile_ok(source: &str) -> IR<Perspective> {
    let tokens = lex(source).tokens;
    let ast = parse(&tokens).program;
    compile(&ast).0
}

fn compile_err(source: &str) -> Diag {
    let tokens = lex(source).tokens;
    let ast = parse(&tokens).program;
    // The lowering is total but collects its resolve errors; the tests check
    // the first one (an unresolved-name program yields exactly one).
    compile(&ast).1.into_iter().next().expect("expected a resolve diagnostic")
}

fn kind(ir: &IR<Perspective>, id: ExprId) -> ExprKind<HighProgramLiteral> {
    ir[id].kind
}

/// The node the statement wrapper selects: the root is either the final
/// expression's own node (no wrap) or `Field(Tuple([…, final]), n)`,
/// which unwraps to the final expression.
fn wrapped(ir: &IR<Perspective>) -> ExprId {
    match ir[ir.root].kind {
        ExprKind::Field { container: array, key: index } => {
            let ExprKind::Tuple(range) = ir[array].kind else {
                panic!("expected the wrapped tuple")
            };
            let ExprKind::Literal(HighProgramLiteral::Int(IntLit(n))) = ir[index].kind else {
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
        ExprKind::Literal(HighProgramLiteral::Int(IntLit(5)))
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
    assert!(matches!(
        kind(&ir, r#type.expect("the annotated type")),
        ExprKind::Placeholder
    ));
}

#[test]
fn a_bang_prefix_compiles_to_an_assert() {
    // !(1 == 1) — the highlevel Assert form, whose condition is the operand.
    let ir = compile_ok("!(1 == 1)");
    let ExprKind::Assert { condition } = kind(&ir, ir.root) else {
        panic!("expected an assert")
    };
    assert!(matches!(
        kind(&ir, condition),
        ExprKind::BinOp {
            operator: lichen_highlevel::ir::BinOp::Eq,
            ..
        }
    ));
    // The assert node carries the `!`'s span (the caret points at the `!`).
    assert_eq!(ir[ir.root].span, Some((1, 1)));
}

#[test]
fn a_block_compiles_to_its_final_expression() {
    // {a = 1; a} — the block is its final expression's own node; the
    // binding is pure sharing, not a new IR form.
    let ir = compile_ok("{a = 1; a}");
    assert!(matches!(
        kind(&ir, ir.root),
        ExprKind::Literal(HighProgramLiteral::Int(IntLit(1)))
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
        ExprKind::Literal(HighProgramLiteral::Int(IntLit(1)))
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
        ExprKind::Literal(HighProgramLiteral::Int(IntLit(1)))
    ));
    assert!(matches!(
        kind(&ir, argument),
        ExprKind::Literal(HighProgramLiteral::Int(IntLit(2)))
    ));
}

#[test]
fn a_statement_expression_is_a_statement_root() {
    // Option B: the top-level program has no tuple cascade.  The bare
    // statement is recorded as a *statement root* (so the checker evaluates
    // it) and the final expression is the program root directly.
    let ir = compile_ok("5; 7");
    assert!(matches!(
        kind(&ir, ir.root),
        ExprKind::Literal(HighProgramLiteral::Int(IntLit(7)))
    ));
    assert_eq!(ir.stmt_roots.len(), 1);
    assert!(matches!(
        kind(&ir, ir.stmt_roots[0]),
        ExprKind::Literal(HighProgramLiteral::Int(IntLit(5)))
    ));
    // A trailing statement identical to the final expression is not wrapped:
    // `a = 1; a` stays the `1` node, with the binding as a statement root.
    let ir = compile_ok("a = 1; a");
    assert!(matches!(
        kind(&ir, ir.root),
        ExprKind::Literal(HighProgramLiteral::Int(IntLit(1)))
    ));
    assert_eq!(ir.stmt_roots.len(), 1);
    // A bare expression statement between bindings is a statement root too.
    let ir = compile_ok("a = 1; 5; a");
    assert!(matches!(
        kind(&ir, ir.root),
        ExprKind::Literal(HighProgramLiteral::Int(IntLit(1)))
    ));
    assert_eq!(ir.stmt_roots.len(), 2);
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
        ExprKind::Literal(HighProgramLiteral::IntType(IntTypeLit))
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
