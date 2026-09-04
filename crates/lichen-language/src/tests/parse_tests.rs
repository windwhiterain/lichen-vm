use super::*;
use crate::lex::lex;

/// The final expression of a program (bindings aside), asserting the
/// parse is clean.
fn parse_ok(source: &str) -> Expr {
    let tokens = lex(source).tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert!(errors.is_empty(), "unexpected parse errors: {errors:?}");
    program.expr
}

/// The first parse error.
fn parse_err(source: &str) -> Diag {
    let tokens = lex(source).tokens;
    let Parsed { errors, .. } = parse(&tokens);
    assert!(!errors.is_empty(), "expected a parse error for {source:?}");
    errors.into_iter().next().unwrap()
}

/// The binding statements of a program, in order.
fn bindings(program: &Program) -> Vec<&Binding> {
    program
        .statements
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Binding(binding) => Some(binding),
            Stmt::Expr(_) => None,
        })
        .collect()
}

#[test]
fn a_program_records_statement_token_ranges() {
    let tokens = lex("a = 1\nb = 2\na + b").tokens;
    let program = parse(&tokens).program;
    // One entry per statement plus one for the final expression, each the
    // token-index range `[start, end)` its parse consumed.
    assert_eq!(program.statements.len() + 1, program.stmt_ranges.len());
    // Tokens: a = 1 SEP b = 2 SEP a + b EOF.
    assert_eq!(program.stmt_ranges, vec![(0, 3), (4, 7), (8, 11)]);
    // Ranges are monotone non-overlapping and in-bounds.
    for w in &program.stmt_ranges {
        assert!(w.0 <= w.1 && w.1 <= tokens.len());
    }
}

#[test]
fn a_program_is_bindings_followed_by_the_final_expression() {
    let tokens = lex("a = [1, 2]; b = 0; a[b]").tokens;
    let program = parse(&tokens).program;
    let binds = bindings(&program);
    assert_eq!(binds.len(), 2);
    assert_eq!(binds[0].name, "a");
    assert_eq!(binds[1].name, "b");
    assert!(matches!(binds[0].value, Expr::Array(..)));
    assert!(matches!(program.expr, Expr::Index { .. }));
}

#[test]
fn a_region_over_the_whole_stream_equals_the_statement_sequence() {
    // Ends in an expression so the program parses cleanly.
    let tokens = lex("a = 1\nb = 2\na + b").tokens;
    let program = parse(&tokens).program;
    // The full logical statement list = the program's `statements` (all but the
    // last) plus the final expression as its own statement.
    let expected_full = {
        let mut v = program.statements.clone();
        v.push(Stmt::Expr(program.expr.clone()));
        v
    };
    // The region parser over the whole stream (everything before Eof) must
    // reproduce exactly that sequence.
    let (region_stmts, errors) = parse_statement_region(&tokens, 0, tokens.len() - 1);
    assert!(errors.is_empty(), "unexpected region errors: {errors:?}");
    assert_eq!(region_stmts.len(), expected_full.len());
    // Statements don't derive PartialEq: compare their debug rendering.
    assert_eq!(format!("{region_stmts:?}"), format!("{expected_full:?}"));
}

#[test]
fn a_region_parses_a_middle_window() {
    let tokens = lex("a = 1\nb = 2\nc = a + b").tokens;
    // The tokens: a = 1 SEP b = 2 SEP c = a + b EOF.
    // Statement "b = 2" spans token indices 4..7.
    let (region_stmts, errors) = parse_statement_region(&tokens, 4, 7);
    assert!(errors.is_empty(), "unexpected region errors: {errors:?}");
    assert_eq!(region_stmts.len(), 1);
    match &region_stmts[0] {
        Stmt::Binding(binding) => assert_eq!(binding.name, "b"),
        _ => panic!("expected a binding, got {:?}", region_stmts[0]),
    }
}

#[test]
fn a_traced_region_reports_absolute_token_ranges() {
    let tokens = lex("a = 1\nb = 2\na + b").tokens;
    // The region over the whole stream (everything before Eof) must produce the
    // same per-statement ranges as the whole-program parser.
    let (stmts, ranges, errors) = parse_statement_region_traced(&tokens, 0, tokens.len() - 1);
    assert!(errors.is_empty(), "unexpected region errors: {errors:?}");
    assert_eq!(ranges, vec![(0, 3), (4, 7), (8, 11)]);
    assert_eq!(stmts.len(), ranges.len(), "one range per statement");

    // A middle window reports its ranges offset back into the whole stream
    // (absolute token indices), not relative to the region slice.
    let (stmts, ranges, _) = parse_statement_region_traced(&tokens, 4, 7);
    assert_eq!(stmts.len(), 1);
    assert_eq!(ranges, vec![(4, 7)]);
}

#[test]
fn a_region_recovering_over_a_broken_window_still_produces_statements() {
    // An unclosed `(` makes the trailing *expression* statement an error block;
    // the region parser must still return that statement (recovered), like the
    // whole program parser would.
    let source = "a = 1\n(2";
    let tokens = lex(source).tokens;
    let whole_program = parse(&tokens).program;
    let (region_stmts, _) = parse_statement_region(&tokens, 0, tokens.len() - 1);
    // The whole program's statement list, with the recovered tail expression
    // appended as its own statement (the form the region parser produces).
    let expected_full = {
        let mut v = whole_program.statements.clone();
        v.push(Stmt::Expr(whole_program.expr.clone()));
        v
    };
    assert_eq!(region_stmts.len(), expected_full.len());
    assert_eq!(format!("{region_stmts:?}"), format!("{expected_full:?}"));
}

#[test]
fn statement_errors_carry_spans() {
    // A binding-only program has no final expression.
    let err = parse_err("a = 5");
    assert_eq!(err.stage, Stage::Parse);
    assert!(err.message.contains("must end with an expression"));
    let err = parse_err("5; a = 1");
    assert!(err.message.contains("must end with an expression"));
    let err = parse_err("a = 1; 5; a = 2");
    assert!(err.message.contains("must end with an expression"));
    let err = parse_err("a = 1;");
    assert!(err.message.contains("must end with an expression"));
    // A binding without a value.
    let err = parse_err("a = ; 5");
    assert_eq!(err.message, "expected '!' or an expression, found a separator");
    assert_eq!(err.span, Some((1, 5)));
}

#[test]
fn bare_expressions_are_statements_anywhere() {
    // 5; a = 1; a — an expression before and between bindings; the last
    // statement is the final expression.
    let tokens = lex("5; a = 1; 6; a").tokens;
    let program = parse(&tokens).program;
    assert_eq!(program.statements.len(), 3);
    assert!(matches!(program.statements[0], Stmt::Expr(..)));
    assert!(matches!(program.statements[1], Stmt::Binding(..)));
    assert!(matches!(program.statements[2], Stmt::Expr(..)));
    assert!(matches!(program.expr, Expr::Name(name, _) if name == "a"));
    // 5; 6 — the last expression is the value.
    let tokens = lex("5; 6").tokens;
    let program = parse(&tokens).program;
    assert_eq!(program.statements.len(), 1);
    assert!(matches!(program.statements[0], Stmt::Expr(..)));
    assert!(matches!(program.expr, Expr::Int(6, _)));
}

#[test]
fn newlines_separate_statements() {
    let tokens = lex("a = [1, 2]\nb = 0\na[b]").tokens;
    let program = parse(&tokens).program;
    let binds = bindings(&program);
    assert_eq!(binds.len(), 2);
    assert_eq!(binds[0].name, "a");
    assert_eq!(binds[1].name, "b");
    assert!(matches!(program.expr, Expr::Index { .. }));
    // A trailing newline after the final expression is not an error.
    parse(&lex("a = 1\na\n").tokens);
    // `;` and newlines mix, and consecutive separators are empty
    // statements.
    let tokens = lex("a = 1;\nb = 2; c = 3\nc").tokens;
    assert_eq!(bindings(&parse(&tokens).program).len(), 3);
    // Leading newlines (e.g. a top-of-file comment's) are skipped.
    let tokens = lex("\n\na = 1\na").tokens;
    assert_eq!(bindings(&parse(&tokens).program).len(), 1);
}

#[test]
fn application_is_left_associative() {
    let Expr::Apply {
        function, argument, ..
    } = parse_ok("x y z")
    else {
        panic!("expected an apply")
    };
    let Expr::Apply {
        argument: inner, ..
    } = *function
    else {
        panic!("the function must be the nested apply")
    };
    assert!(matches!(*inner, Expr::Name(..)));
    assert!(matches!(*argument, Expr::Name(..)));
}

#[test]
fn arrows_are_right_associative() {
    let Expr::Arrow {
        parameter,
        r#return,
        ..
    } = parse_ok("Int -> Int -> Int")
    else {
        panic!("expected an arrow")
    };
    assert!(matches!(*parameter, Expr::TypeConst(TypeConst::Int, _)));
    assert!(matches!(*r#return, Expr::Arrow { .. }));
}

#[test]
fn annotation_binds_looser_than_arrow_and_apply() {
    let Expr::Annotation { value, r#type, .. } = parse_ok("5 : Int -> Int") else {
        panic!("expected an annotation")
    };
    assert!(matches!(*value, Expr::Int(5, _)));
    assert!(matches!(*r#type.as_deref().unwrap(), Expr::Arrow { .. }));
    let Expr::Annotation { value, .. } = parse_ok("x y : Int") else {
        panic!("expected an annotation")
    };
    assert!(matches!(*value, Expr::Apply { .. }));
}

#[test]
fn lambda_bodies_extend_maximally() {
    let Expr::Lambda { r#return, .. } = parse_ok("x => e : Int") else {
        panic!("expected a lambda")
    };
    assert!(matches!(*r#return, Expr::Annotation { .. }));
    let Expr::Lambda { r#return, .. } = parse_ok("x => y => e") else {
        panic!("expected a lambda")
    };
    assert!(matches!(*r#return, Expr::Lambda { .. }));
}

#[test]
fn an_annotated_parameter_is_a_lambda() {
    let Expr::Lambda {
        parameter,
        parameter_type,
        r#return,
        ..
    } = parse_ok("x : Int => x")
    else {
        panic!("expected a lambda")
    };
    assert_eq!(parameter, "x");
    assert!(matches!(
        parameter_type,
        Some(t) if matches!(*t, Expr::TypeConst(TypeConst::Int, _))
    ));
    assert!(matches!(*r#return, Expr::Name(name, _) if name == "x"));
    // An unannotated lambda has no parameter type.
    assert!(matches!(
        parse_ok("x => x"),
        Expr::Lambda {
            parameter_type: None,
            ..
        }
    ));
    // The annotation can be a compound type: x : Int -> Int => e.
    let Expr::Lambda { parameter_type, .. } = parse_ok("x : Int -> Int => x") else {
        panic!("expected a lambda")
    };
    assert!(matches!(parameter_type, Some(t) if matches!(*t, Expr::Arrow { .. })));
    // x : Int alone (no `=>`) stays an annotation.
    assert!(matches!(parse_ok("x : Int"), Expr::Annotation { .. }));
    // An annotated lambda parenthesized is the same form.
    assert!(matches!(
        parse_ok("(x : Int) => x"),
        Expr::Lambda {
            parameter_type: Some(..),
            ..
        }
    ));
}

#[test]
fn tuples_and_type_tuples_by_position() {
    // `(Int, Int)` is a TypeTuple in type position, a Tuple value in term
    // position.
    let Expr::Annotation { r#type, .. } = parse_ok("x : (Int, Int)") else {
        panic!("expected an annotation")
    };
    assert!(matches!(*r#type.as_deref().unwrap(), Expr::TypeTuple(..)));
    let Expr::Apply { argument, .. } = parse_ok("f (Int, Int)") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::Tuple(..)));
}

#[test]
fn angle_brackets_are_exclusively_type_level() {
    let Expr::Annotation { r#type, .. } = parse_ok("x : <Int, Type>") else {
        panic!("expected an annotation")
    };
    assert!(matches!(*r#type.as_deref().unwrap(), Expr::TypeTuple(..)));
    let Expr::TypeTuple(elements, _) = parse_ok("<Int, Type>") else {
        panic!("expected a type tuple")
    };
    assert_eq!(elements.len(), 2);
    // a type tuple in argument position needs parens.
    let Expr::Apply { argument, .. } = parse_ok("f (<Int, Type>)") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::TypeTuple(..)));
}

#[test]
fn struct_types_are_positional_fields_in_angle_brackets() {
    let Expr::StructType(fields, _) = parse_ok("struct<Int, Int -> Int>") else {
        panic!("expected a struct type")
    };
    assert_eq!(fields.len(), 2);
    assert!(matches!(fields[0].ty, Expr::TypeConst(TypeConst::Int, _)));
    assert!(matches!(fields[1].ty, Expr::Arrow { .. }));
    // all fields are unnamed (positional).
    assert!(fields.iter().all(|f| f.name.is_none()));
    // a single field is the newtype form.
    assert!(matches!(
        parse_ok("struct<Int>"),
        Expr::StructType(f, _) if f.len() == 1
    ));
    // struct types are first-class values: an apply argument, a binding.
    let Expr::Apply { argument, .. } = parse_ok("f (struct<Int>)") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::StructType(..)));
    // fields are type expressions, so `(Int, Type)` inside is a
    // TypeTuple, not a Tuple.
    let Expr::StructType(fields, _) = parse_ok("struct<(Int, Type)>") else {
        panic!("expected a struct type")
    };
    assert!(matches!(fields[0].ty, Expr::TypeTuple(..)));
}

#[test]
fn struct_types_carry_optional_field_names() {
    // `struct<.a Int, .b Type>` — named fields.
    let Expr::StructType(fields, _) = parse_ok("struct<.a Int, .b Type>") else {
        panic!("expected a struct type")
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name.as_deref(), Some("a"));
    assert!(matches!(fields[0].ty, Expr::TypeConst(TypeConst::Int, _)));
    assert_eq!(fields[1].name.as_deref(), Some("b"));
    assert!(matches!(fields[1].ty, Expr::TypeConst(TypeConst::Type, _)));
    // a name is optional per field — a mixed struct parses.
    let Expr::StructType(fields, _) = parse_ok("struct<.a Int, Type>") else {
        panic!("expected a struct type")
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name.as_deref(), Some("a"));
    assert!(fields[1].name.is_none());
}

#[test]
fn named_field_read_is_dot_postfix() {
    // `a.b` — a named field read over a struct instance.
    let Expr::NamedFieldRead { container, name, .. } = parse_ok("a.b") else {
        panic!("expected a named field read")
    };
    assert!(matches!(*container, Expr::Name(n, _) if n == "a"));
    assert_eq!(name, "b");
    // `a.b.c` chains left.
    let Expr::NamedFieldRead { container, name, .. } = parse_ok("a.b.c") else {
        panic!("expected a named field read")
    };
    assert_eq!(name, "c");
    assert!(matches!(*container, Expr::NamedFieldRead { name: inner, .. } if inner == "b"));
    // a dot postfix after an index folds left too: `a[0].b`.
    let Expr::NamedFieldRead { container, name, .. } = parse_ok("a[0].b") else {
        panic!("expected a named field read")
    };
    assert_eq!(name, "b");
    assert!(matches!(*container, Expr::Index { .. }));
}

#[test]
fn struct_instantiation_is_adjacent_parens() {
    // `C(f1, f2)` — the `(` directly after the callee (no space) is
    // struct instantiation, not a function apply.
    let Expr::StructInst { callee, fields, .. } = parse_ok("A(1, Int)") else {
        panic!("expected a struct instance")
    };
    assert!(matches!(*callee, Expr::Name(n, _) if n == "A"));
    assert_eq!(fields.len(), 2);
    assert!(matches!(fields[0].value, Expr::Int(1, _)));
    assert!(matches!(fields[1].value, Expr::TypeConst(TypeConst::Int, _)));
    // a single field carries a trailing comma — the bare `A(1)` is the
    // positional slot read.
    assert!(matches!(
        parse_ok("A(1,)"),
        Expr::StructInst { fields, .. } if fields.len() == 1
    ));
    // a field-less struct instance parses in both spellings — `A()` and
    // the empty-tuple form `A(,)` (the instantiation mirror of the
    // tuple grammar's `(,)` empty tuple vs the future `()` unit).
    assert!(matches!(
        parse_ok("A()"),
        Expr::StructInst { fields, .. } if fields.is_empty()
    ));
    assert!(matches!(
        parse_ok("A(,)"),
        Expr::StructInst { fields, .. } if fields.is_empty()
    ));
    // the bare single-expression paren is the slot read, not an
    // instantiation.
    let Expr::FieldRead { container, key, .. } = parse_ok("A(1)") else {
        panic!("expected a slot read")
    };
    assert!(matches!(*container, Expr::Name(n, _) if n == "A"));
    assert!(matches!(*key, Expr::Int(1, _)));
    // the callee may be an inline struct type.
    let Expr::StructInst { callee, .. } = parse_ok("struct<Int, Int>(1, 2)") else {
        panic!("expected a struct instance")
    };
    assert!(matches!(*callee, Expr::StructType(..)));
}

#[test]
fn struct_instantiation_carries_optional_field_names() {
    // `A(.x 1, .y Int)` — a leading `.` names an argument; a bare expression
    // is a positional argument.
    let Expr::StructInst { fields, .. } = parse_ok("A(.x 1, .y Int)") else {
        panic!("expected a struct instance")
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name.as_deref(), Some("x"));
    assert!(matches!(fields[0].value, Expr::Int(1, _)));
    assert_eq!(fields[1].name.as_deref(), Some("y"));
    assert!(matches!(fields[1].value, Expr::TypeConst(TypeConst::Int, _)));
    // mixed positional and named
    let Expr::StructInst { fields, .. } = parse_ok("A(1, .y Int)") else {
        panic!("expected a struct instance")
    };
    assert_eq!(fields[0].name, None);
    assert!(matches!(fields[0].value, Expr::Int(1, _)));
    assert_eq!(fields[1].name.as_deref(), Some("y"));
    // a single named argument is an instantiation, never the positional slot
    // read.
    assert!(matches!(
        parse_ok("A(.x 1)"),
        Expr::StructInst { fields, .. } if fields.len() == 1 && fields[0].name.as_deref() == Some("x")
    ));
}

#[test]
fn spaced_parens_are_apply_not_struct_instantiation() {
    // `f (1, 2)` — a space before the `(` — is a function apply whose
    // argument is the tuple.
    let Expr::Apply { argument, .. } = parse_ok("f (1, 2)") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::Tuple(..)));
    // juxtaposition stays application too.
    assert!(matches!(parse_ok("f x"), Expr::Apply { .. }));
    // a spaced single field is an apply of the grouped value.
    assert!(matches!(parse_ok("A (1)"), Expr::Apply { .. }));
}

#[test]
fn struct_type_errors_carry_spans() {
    let err = parse_err("struct");
    assert_eq!(err.span, Some((1, 7)));
    let err = parse_err("struct<");
    assert_eq!(err.span, Some((1, 8)));
    let err = parse_err("struct<>");
    assert_eq!(err.span, Some((1, 8)));
    let err = parse_err("struct<Int");
    assert_eq!(err.span, Some((1, 11)));
}

#[test]
fn the_angle_bracket_array_type() {
    let Expr::TypeArray {
        element_type,
        length,
        ..
    } = parse_ok("Int<3>")
    else {
        panic!("expected an array type")
    };
    assert!(matches!(*element_type, Expr::TypeConst(TypeConst::Int, _)));
    assert!(matches!(*length, Expr::Int(3, _)));
    // A `[` right after an expression is an index, never an array
    // literal — an array argument needs parens.
    let Expr::Apply { argument, .. } = parse_ok("f ([1, 2])") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::Array(..)));
    // chained postfix: Int<2><3> = (Int<2>)<3>
    let Expr::TypeArray { element_type, .. } = parse_ok("Int<2><3>") else {
        panic!("expected an array type")
    };
    assert!(matches!(*element_type, Expr::TypeArray { .. }));
    // A glued `<` after an expression is the array type (never an
    // application); a spaced `<` is a fresh tuple-type atom.
    let Expr::TypeArray { element_type, .. } = parse_ok("f<3>") else {
        panic!("expected an array type")
    };
    assert!(matches!(*element_type, Expr::Name(..)));
}

#[test]
fn the_index_postfix() {
    let Expr::Index { array, index, .. } = parse_ok("a[0]") else {
        panic!("expected an index")
    };
    assert!(matches!(*array, Expr::Name(..)));
    assert!(matches!(*index, Expr::Int(0, _)));
    // A spaced `[` is a fresh array literal: `a [0]` is an application.
    assert!(matches!(parse_ok("a [0]"), Expr::Apply { .. }));
    // Chained: a[0][1] = (a[0])[1].
    let Expr::Index { array, .. } = parse_ok("a[0][1]") else {
        panic!("expected an index")
    };
    assert!(matches!(*array, Expr::Index { .. }));
    // An array literal is indexable: ([1, 2])[1].
    let Expr::Index { array, .. } = parse_ok("([1, 2])[1]") else {
        panic!("expected an index")
    };
    assert!(matches!(*array, Expr::Array(..)));
}

#[test]
fn index_errors_carry_spans() {
    // `a[]` is application of `a` to an empty array literal — the
    // element error mentions the `~` prefix the array parser now accepts.
    let err = parse_err("a[]");
    assert_eq!(err.message, "expected '~', '!', or an expression, found ']'");
    let err = parse_err("a[0");
    assert_eq!(err.span, Some((1, 4)));
}

#[test]
fn a_single_element_angle_bracket_is_a_parse_error() {
    let err = parse_err("<Int>");
    assert_eq!(err.span, Some((1, 5)));
}

#[test]
fn parse_errors_carry_spans() {
    let err = parse_err("x =>");
    assert_eq!(err.stage, Stage::Parse);
    assert_eq!(
        err.message,
        "expected '!' or an expression, found the end of the program"
    );
    assert_eq!(err.span, Some((1, 5)));
    let err = parse_err("(x");
    assert_eq!(err.span, Some((1, 3)));
    let err = parse_err("x )");
    assert_eq!(err.span, Some((1, 3)));
    let err = parse_err("[1, 2");
    assert_eq!(err.span, Some((1, 6)));
}

#[test]
fn a_name_before_arrow_is_not_a_lambda_operand() {
    // `f x => e` — the lambda only starts in prefix position, so the
    // arrow is left dangling.
    let err = parse_err("f x => e");
    assert_eq!(err.stage, Stage::Parse);
    assert!(err.message.contains("=>"));
}

#[test]
fn an_underscore_in_type_position_is_a_placeholder() {
    let Expr::Annotation { r#type, .. } = parse_ok("x : _") else {
        panic!("expected an annotation")
    };
    assert!(matches!(*r#type.as_deref().unwrap(), Expr::Placeholder(_)));
    // Nested: `x : Int -> _` — the arrow's return is a placeholder.
    let Expr::Annotation { r#type, .. } = parse_ok("x : Int -> _") else {
        panic!("expected an annotation")
    };
    let Expr::Arrow { r#return, .. } = *r#type.expect("an arrow type") else {
        panic!("expected an arrow type")
    };
    assert!(matches!(*r#return, Expr::Placeholder(_)));
    // In term position `_` stays an ordinary name.
    assert!(matches!(parse_ok("_"), Expr::Name(name, _) if name == "_"));
}

#[test]
fn a_bang_prefix_parses_as_an_assert() {
    let e = parse_ok("!(1 == 1)");
    let Expr::Assert { value, span } = e else {
        panic!("expected an assert, got {e:?}")
    };
    assert!(matches!(
        *value,
        Expr::BinOp {
            operator: BinOp::Eq,
            ..
        }
    ));
    assert_eq!(span, (1, 1), "the assert's span starts at the `!`");
}

#[test]
fn a_bang_binds_a_full_application_but_tighter_than_a_binary_operator() {
    // `! f x` asserts `f x` — the application is the operand.
    let e = parse_ok("!f x");
    assert!(matches!(
        e,
        Expr::Assert { value, .. } if matches!(*value, Expr::Apply { .. })
    ));
    // `! x <= 3` is `(!x) <= 3`, not `!(x <= 3)` — `!` binds tighter than `<=`.
    let e = parse_ok("!x <= 3");
    assert!(matches!(
        e,
        Expr::BinOp {
            operator: BinOp::Leq,
            left,
            ..
        } if matches!(*left, Expr::Assert { .. })
    ));
}

#[test]
fn a_block_is_bindings_followed_by_a_final_expression() {
    let Expr::Block {
        statements, expr, ..
    } = parse_ok("{a = 1; a}")
    else {
        panic!("expected a block")
    };
    assert_eq!(statements.len(), 1);
    let Stmt::Binding(binding) = &statements[0] else {
        panic!("expected a binding")
    };
    assert_eq!(binding.name, "a");
    assert!(matches!(binding.value, Expr::Int(1, _)));
    assert!(matches!(*expr, Expr::Name(name, _) if name == "a"));
    // A block with only a final expression.
    assert!(matches!(parse_ok("{5}"), Expr::Block { .. }));
    // Statements are graph-shared bindings, not just literals.
    assert!(matches!(
        parse_ok("{a = [1, 2]; b = a[0]; b}"),
        Expr::Block { statements, .. } if statements.len() == 2
    ));
}

#[test]
fn a_block_can_be_a_lambda_body() {
    let Expr::Lambda { r#return, .. } = parse_ok("x => {a = x; a}") else {
        panic!("expected a lambda")
    };
    assert!(matches!(*r#return, Expr::Block { .. }));
}

#[test]
fn a_block_can_be_written_without_semicolons() {
    // {a = 1\nb = 2\nb} — newlines separate the block's statements.
    let Expr::Block {
        statements, expr, ..
    } = parse_ok("{a = 1\nb = 2\nb}")
    else {
        panic!("expected a block")
    };
    assert_eq!(statements.len(), 2);
    let Stmt::Binding(a) = &statements[0] else {
        panic!("expected a binding")
    };
    let Stmt::Binding(b) = &statements[1] else {
        panic!("expected a binding")
    };
    assert_eq!(a.name, "a");
    assert_eq!(b.name, "b");
    assert!(matches!(*expr, Expr::Name(name, _) if name == "b"));
    // A trailing newline before the `}` is fine.
    assert!(matches!(parse_ok("{a = 1\na\n}"), Expr::Block { .. }));
}

#[test]
fn a_block_is_an_atom() {
    let Expr::Apply { argument, .. } = parse_ok("f {a = 1; a}") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::Block { .. }));
    // Blocks nest.
    let Expr::Block { expr, .. } = parse_ok("{{a = 1; a}}") else {
        panic!("expected a block")
    };
    assert!(matches!(*expr, Expr::Block { .. }));
    // A block composes with postfix and annotation forms.
    assert!(matches!(
        parse_ok("{a = 1; a}[0]"),
        Expr::Index { array, .. } if matches!(*array, Expr::Block { .. })
    ));
    assert!(matches!(
        parse_ok("{a = 1; a} : Int"),
        Expr::Annotation { .. }
    ));
}

#[test]
fn block_errors_carry_spans() {
    let err = parse_err("{a = 1; a");
    assert_eq!(err.stage, Stage::Parse);
    assert_eq!(err.span, Some((1, 10)));
    // An empty block is not a block: the value expression is missing.
    let err = parse_err("{}");
    assert!(err.message.contains("found '}'"));
    // A block whose last statement is a binding has no tail expression; it
    // parses as a struct-returning block (an anonymous struct instance).
    let Expr::RecordBlock { fields, .. } = parse_ok("{a = 1}") else {
        panic!("expected a struct-returning block");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name.as_deref(), Some("a"));
    assert!(fields[0].field, "a block-wide binding is a field");
    // A stray `}` after the program.
    let err = parse_err("x }");
    assert_eq!(err.span, Some((1, 3)));
}

#[test]
fn broken_statements_are_recovered() {
    // A broken binding value skips to the next separator and the rest of
    // the program is reached.
    let tokens = lex("a = ; b = 2; 5").tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert_eq!(errors.len(), 1, "the broken binding's error");
    assert_eq!(
        errors[0].message,
        "expected '!' or an expression, found a separator"
    );
    assert_eq!(program.statements.len(), 2);
    assert!(matches!(program.expr, Expr::Int(5, _)));
    // An unclosed paren in a value is recovered the same way.
    let tokens = lex("a = (x; 5").tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert_eq!(errors.len(), 1);
    assert!(matches!(program.expr, Expr::Int(5, _)));
    // A statement that cannot start is skipped entirely, and the parse
    // continues to the final expression.
    let tokens = lex("a = 1; -> ; b = 2; b").tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert_eq!(errors.len(), 1);
    assert!(matches!(program.expr, Expr::Name(name, _) if name == "b"));
}

#[test]
fn dangling_operators_are_recovered() {
    // An operator with a missing operand consumes the operator and
    // recovers the operand as an error node — one precise error, no
    // "could not be parsed" cascade, and the rest of the program is
    // reached.  Same across every operator level; the recovered
    // expression keeps the operator's shape with an error-node operand.
    let cases: &[(&str, (u32, u32))] = &[
        ("a = 1 + ; b = 2; b", (1, 9)),
        ("a = 1 <= ; b = 2; b", (1, 10)),
        ("a = 1 -> ; b = 2; b", (1, 10)),
        ("a = 1 : ; b = 2; b", (1, 9)),
        ("a = x => ; b = 2; b", (1, 10)),
    ];
    for (source, span) in cases {
        let tokens = lex(source).tokens;
        let Parsed { program, errors } = parse(&tokens);
        assert_eq!(errors.len(), 1, "{source}: one precise error");
        assert_eq!(
            errors[0].span,
            Some(*span),
            "{source}: at the missing operand"
        );
        assert_eq!(program.statements.len(), 2, "{source}: both statements");
        assert!(matches!(program.expr, Expr::Name(name, _) if name == "b"));
        let Stmt::Binding(binding) = &program.statements[0] else {
            panic!("{source}: first statement is a binding");
        };
        // The dangling operator is consumed — the value keeps the
        // operator's shape (BinOp/Arrow/Annotation/Lambda) with an
        // error node in the operand slot.
        let recovered = matches!(
            &binding.value,
            Expr::BinOp { right, .. }
                if matches!(**right, Expr::Err { .. } | Expr::Placeholder(_))
        ) || matches!(
            &binding.value,
            Expr::Arrow { r#return, .. }
                if matches!(**r#return, Expr::Err { .. } | Expr::Placeholder(_))
        ) || matches!(
            &binding.value,
            Expr::Annotation { r#type, .. }
                if matches!(*r#type.as_deref().unwrap(), Expr::Err { .. } | Expr::Placeholder(_))
        ) || matches!(
            &binding.value,
            Expr::Lambda { r#return, .. }
                if matches!(**r#return, Expr::Err { .. } | Expr::Placeholder(_))
        );
        assert!(recovered, "{source}: recovered shape: {:?}", binding.value);
    }
}
