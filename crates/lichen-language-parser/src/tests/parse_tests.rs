use super::*;
use lichen_language_lex::lex;

/// The final expression of a program (bindings aside), asserting the
/// parse is clean.
fn parse_ok(source: &str) -> Expr {
    let tokens = lex(source).tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert!(errors.is_empty(), "unexpected parse errors: {errors:?}");
    program.expr.expect("a clean program has a tail expression")
}

/// The first parse error.
fn parse_err(source: &str) -> ParseDiag {
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
        .filter_map(|bs| match &bs.stmt {
            Stmt::Binding(binding) => Some(binding),
            Stmt::Expr(_) => None,
        })
        .collect()
}

#[test]
fn a_program_records_statement_token_ranges() {
    let tokens = lex("a := 1\nb := 2\na + b").tokens;
    let program = parse(&tokens).program;
    // One per statement, plus one for the final expression: the token-index
    // range `[start, end)` each parse consumed.
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
    let tokens = lex("a := [1, 2]; b := 0; a[b]").tokens;
    let program = parse(&tokens).program;
    let binds = bindings(&program);
    assert_eq!(binds.len(), 2);
    assert_eq!(binds[0].name, "a");
    assert_eq!(binds[1].name, "b");
    assert!(matches!(binds[0].value, Expr::Array { .. }));
    assert!(matches!(program.expr, Some(Expr::Index { .. })));
    // `=` is an expression-level unify, never a binding.
    let tokens = lex("a = 1").tokens;
    let program = parse(&tokens).program;
    assert!(bindings(&program).is_empty(), "`=` binds no name");
    assert!(matches!(program.expr, Some(Expr::Unify { .. })));
}

#[test]
fn a_region_over_the_whole_stream_equals_the_statement_sequence() {
    // Ends in an expression so the program parses cleanly.
    let tokens = lex("a := 1\nb := 2\na + b").tokens;
    let program = parse(&tokens).program;
    // The program's `statements` (all but the last) plus the final expression
    // as its own block statement.
    let expected_full = {
        let mut v = program.statements.clone();
        v.push(BlockStmt {
            stmt: Stmt::Expr(program.expr.clone().unwrap()),
            public: false,
        });
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
    let tokens = lex("a := 1\nb := 2\nc := a + b").tokens;
    // The tokens: a := 1 SEP b := 2 SEP c := a + b EOF.
    // Statement "b := 2" spans token indices 4..7.
    let (region_stmts, errors) = parse_statement_region(&tokens, 4, 7);
    assert!(errors.is_empty(), "unexpected region errors: {errors:?}");
    assert_eq!(region_stmts.len(), 1);
    match &region_stmts[0].stmt {
        Stmt::Binding(binding) => assert_eq!(binding.name, "b"),
        _ => panic!("expected a binding, got {:?}", region_stmts[0]),
    }
}

#[test]
fn a_traced_region_reports_absolute_token_ranges() {
    let tokens = lex("a := 1\nb := 2\na + b").tokens;
    // The whole-stream region must produce the same per-statement ranges as
    // the whole-program parser.
    let (stmts, ranges, errors) = parse_statement_region_traced(&tokens, 0, tokens.len() - 1);
    assert!(errors.is_empty(), "unexpected region errors: {errors:?}");
    assert_eq!(ranges, vec![(0, 3), (4, 7), (8, 11)]);
    assert_eq!(stmts.len(), ranges.len(), "one range per statement");

    // A middle window reports absolute token indices, not offsets into the
    // slice.
    let (stmts, ranges, _) = parse_statement_region_traced(&tokens, 4, 7);
    assert_eq!(stmts.len(), 1);
    assert_eq!(ranges, vec![(4, 7)]);
}

#[test]
fn a_region_recovering_over_a_broken_window_still_produces_statements() {
    // An unclosed `(` makes the trailing statement an error block, which the
    // region parser must still return, recovered.
    let source = "a := 1\n(2";
    let tokens = lex(source).tokens;
    let whole_program = parse(&tokens).program;
    let (region_stmts, _) = parse_statement_region(&tokens, 0, tokens.len() - 1);
    // The whole program's list, with the recovered tail appended as its own
    // statement.
    let expected_full = {
        let mut v = whole_program.statements.clone();
        v.push(BlockStmt {
            stmt: Stmt::Expr(whole_program.expr.clone().unwrap()),
            public: false,
        });
        v
    };
    assert_eq!(region_stmts.len(), expected_full.len());
    assert_eq!(format!("{region_stmts:?}"), format!("{expected_full:?}"));
}

#[test]
fn a_trailing_binding_program_is_a_record_program() {
    // A program whose last statement is a binding has no tail: it is a record
    // program (a module).
    let tokens = lex("a := 5").tokens;
    let program = parse(&tokens).program;
    assert!(program.expr.is_none(), "no tail on a record program");
    assert_eq!(program.statements.len(), 1);
    assert!(matches!(program.statements[0].stmt, Stmt::Binding(..)));
    // A bare expression before a trailing binding is a positional field.
    let tokens = lex("5; a := 1").tokens;
    let program = parse(&tokens).program;
    assert!(program.expr.is_none());
    assert_eq!(program.statements.len(), 2);
    // A multi-statement record program.
    let tokens = lex("a := 1; 5; a := 2").tokens;
    let program = parse(&tokens).program;
    assert!(program.expr.is_none());
    assert_eq!(program.statements.len(), 3);
    // A trailing separator after a binding is still a record program.
    let tokens = lex("a := 1;").tokens;
    let program = parse(&tokens).program;
    assert!(program.expr.is_none());
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn statement_errors_carry_spans() {
    // A binding without a value.
    let err = parse_err("a := ; 5");
    assert_eq!(
        err.message,
        "expected '@assert', '@gc', 'int2float', 'float2int', 'exp', 'sqrt', 'floor', 'sin', 'cos', 'ln', 'max', 'min', 'table', 'set', an expression, ':', '#', '!', '?', or '~', found a separator"
    );
    assert_eq!(err.span, Some((1, 6)));
}

#[test]
fn bare_expressions_are_statements_anywhere() {
    // 5; a = 1; a — an expression before and between bindings; the last
    // statement is the final expression.
    let tokens = lex("5; a := 1; 6; a").tokens;
    let program = parse(&tokens).program;
    assert_eq!(program.statements.len(), 3);
    assert!(matches!(program.statements[0].stmt, Stmt::Expr(..)));
    assert!(matches!(program.statements[1].stmt, Stmt::Binding(..)));
    assert!(matches!(program.statements[2].stmt, Stmt::Expr(..)));
    assert!(matches!(program.expr, Some(Expr::Name(name, _, _)) if name == "a"));
    // 5; 6 — the last expression is the value.
    let tokens = lex("5; 6").tokens;
    let program = parse(&tokens).program;
    assert_eq!(program.statements.len(), 1);
    assert!(matches!(program.statements[0].stmt, Stmt::Expr(..)));
    assert!(matches!(program.expr, Some(Expr::Int(6, _))));
}

#[test]
fn newlines_separate_statements() {
    let tokens = lex("a := [1, 2]\nb := 0\na[b]").tokens;
    let program = parse(&tokens).program;
    let binds = bindings(&program);
    assert_eq!(binds.len(), 2);
    assert_eq!(binds[0].name, "a");
    assert_eq!(binds[1].name, "b");
    assert!(matches!(program.expr, Some(Expr::Index { .. })));
    // A trailing newline after the final expression is not an error.
    parse(&lex("a := 1\na\n").tokens);
    // `;` and newlines mix, and consecutive separators are empty
    // statements.
    let tokens = lex("a := 1;\nb := 2; c := 3\nc").tokens;
    assert_eq!(bindings(&parse(&tokens).program).len(), 3);
    // Leading newlines (e.g. a top-of-file comment's) are skipped.
    let tokens = lex("\n\na := 1\na").tokens;
    assert_eq!(bindings(&parse(&tokens).program).len(), 1);
}

#[test]
fn a_run_of_separators_inside_a_list_form_is_tolerated() {
    // A run of separators between items is tolerated; a run after the last item
    // is the same trailing separator.
    let Expr::Tuple(elements, _) = parse_ok("(1,\n2)") else {
        panic!("expected a tuple")
    };
    assert_eq!(elements.len(), 2);
    let Expr::Array {
        template, length, ..
    } = parse_ok("[Int,\n2]")
    else {
        panic!("expected an array template")
    };
    assert!(matches!(*template, Expr::TypeConst(TypeConst::Int, _)));
    assert!(matches!(*length, Expr::Int(2, _)));
    // The remaining list forms, one item per line.
    assert!(matches!(
        parse_ok("T[1,\n2]"),
        Expr::ArrayInst { elements, .. } if elements.len() == 2
    ));
    assert!(matches!(
        parse_ok("S{.a 1,\n.b 2}"),
        Expr::StructInst { fields, .. } if fields.len() == 2
    ));
    assert!(matches!(
        parse_ok("table(1 ==> 2,\n3 ==> 4)"),
        Expr::Table(entries, _) if entries.len() == 2
    ));
    // The two *type* forms write their slots in braces, one per line.
    assert!(matches!(
        parse_ok("set{\nInt}"),
        Expr::TypeSet(element, _)
            if matches!(*element, Expr::TypeConst(TypeConst::Int, _))
    ));
    assert!(matches!(
        parse_ok("table{Int,\nstring}"),
        Expr::TypeTable { key, value, .. }
            if matches!(*key, Expr::TypeConst(TypeConst::Int, _))
                && matches!(*value, Expr::TypeConst(TypeConst::String, _))
    ));
    // A run after the last item still keeps the form a tuple (a bare `(1)` is
    // transparent grouping) or an instantiation.
    assert!(matches!(
        parse_ok("(1,\n)"),
        Expr::Tuple(elements, _) if elements.len() == 1
    ));
    assert!(matches!(
        parse_ok("S{.a 1,\n}"),
        Expr::StructInst { fields, .. } if fields.len() == 1
    ));
}

#[test]
fn an_expression_cannot_continue_across_a_separator() {
    // The flip side: an expression still stops at a separator.
    for source in ["1 +\n2", "x =>\n x + 1", "table(1 ==>\n2)"] {
        let tokens = lex(source).tokens;
        let Parsed { errors, .. } = parse(&tokens);
        assert!(!errors.is_empty(), "{source:?} must not parse");
    }
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
fn annotation_binds_looser_than_apply() {
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
    assert!(matches!(*r#return, Expr::Name(name, _, _) if name == "x"));
    // An unannotated lambda has no parameter type.
    assert!(matches!(
        parse_ok("x => x"),
        Expr::Lambda {
            parameter_type: None,
            ..
        }
    ));
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
    // A parameter template desugars into the body: `x <: T => e` is
    // `x => { x <: T; e }` (`docs/notes/value-templates.md`).
    let Expr::Lambda {
        parameter,
        parameter_type,
        r#return,
        ..
    } = parse_ok("x <: T => x")
    else {
        panic!("expected a lambda")
    };
    assert_eq!(parameter, "x");
    assert!(
        parameter_type.is_none(),
        "the template is not a parameter type"
    );
    let Expr::Block {
        statements, expr, ..
    } = *r#return
    else {
        panic!("expected a block body")
    };
    assert_eq!(statements.len(), 1);
    let Stmt::Expr(Expr::TemplateUnify {
        actual, template, ..
    }) = &statements[0]
    else {
        panic!("expected a template unify")
    };
    assert!(matches!(actual.as_ref(), Expr::Name(n, _, _) if n == "x"));
    assert!(matches!(template.as_ref(), Expr::Name(n, _, _) if n == "T"));
    assert!(matches!(*expr, Expr::Name(n, _, _) if n == "x"));
}

#[test]
fn parens_are_always_tuple_values() {
    // `(a, b)` is a tuple *value* in every position — there is no type/value
    // mode.
    let Expr::Annotation { r#type, .. } = parse_ok("x : (Int, Int)") else {
        panic!("expected an annotation")
    };
    assert!(matches!(*r#type.as_deref().unwrap(), Expr::Tuple(..)));
    let Expr::Apply { argument, .. } = parse_ok("f (Int, Int)") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::Tuple(..)));
}

#[test]
fn the_tuple_is_a_value_and_only_a_value() {
    // `(Int, Type)` is a tuple value; there is no separate tuple type form
    // any more.
    let Expr::Tuple(elements, _) = parse_ok("(Int, Type)") else {
        panic!("expected a tuple")
    };
    assert_eq!(elements.len(), 2);
    // A tuple value is an atom, so it applies as an argument directly.
    let Expr::Apply { argument, .. } = parse_ok("f (Int, Type)") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::Tuple(..)));
    // The tuple's template is the builtin `Tuple` constant.
    assert!(matches!(
        parse_ok("Tuple"),
        Expr::TypeConst(TypeConst::Tuple, _)
    ));
}

#[test]
fn a_struct_value_names_its_fields() {
    // `{.a 1, .b 2}` — a struct value: every field is named.
    let Expr::StructValue { fields, .. } = parse_ok("{.a 1, .b 2}") else {
        panic!("expected a struct value")
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "a");
    assert!(matches!(fields[0].value, Expr::Int(1, _)));
    assert_eq!(fields[1].name, "b");
    assert!(matches!(fields[1].value, Expr::Int(2, _)));
    // A struct value is an atom, so it applies as an argument directly.
    let Expr::Apply { argument, .. } = parse_ok("f ({.a 1})") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::StructValue { .. }));
    // An empty struct value is the `{}` form; the empty block is gone.
    let Expr::StructValue { fields, .. } = parse_ok("{}") else {
        panic!("expected a struct value")
    };
    assert!(fields.is_empty());
    // The struct's template is the builtin `Struct` constant.
    assert!(matches!(
        parse_ok("Struct"),
        Expr::TypeConst(TypeConst::Struct, _)
    ));
}

#[test]
fn named_field_read_is_dot_postfix() {
    // `a.b` — a named field read over a struct instance.
    let Expr::NamedFieldRead {
        container, name, ..
    } = parse_ok("a.b")
    else {
        panic!("expected a named field read")
    };
    assert!(matches!(*container, Expr::Name(n, _, _) if n == "a"));
    assert_eq!(name, "b");
    // `a.b.c` chains left.
    let Expr::NamedFieldRead {
        container, name, ..
    } = parse_ok("a.b.c")
    else {
        panic!("expected a named field read")
    };
    assert_eq!(name, "c");
    assert!(matches!(*container, Expr::NamedFieldRead { name: inner, .. } if inner == "b"));
    // a dot postfix after an index folds left too: `a[0].b`.
    let Expr::NamedFieldRead {
        container, name, ..
    } = parse_ok("a[0].b")
    else {
        panic!("expected a named field read")
    };
    assert_eq!(name, "b");
    assert!(matches!(*container, Expr::Index { .. }));
}

#[test]
fn struct_instantiation_is_an_adjacent_brace() {
    // `S{.a 1, .b 2}` — the `{` is adjacent to the callee (no space) and
    // names the fields it fills.
    let Expr::StructInst { callee, fields, .. } = parse_ok("S{.a 1, .b Int}") else {
        panic!("expected a struct instance")
    };
    assert!(matches!(*callee, Expr::Name(n, _, _) if n == "S"));
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "a");
    assert!(matches!(fields[0].value, Expr::Int(1, _)));
    assert_eq!(fields[1].name, "b");
    assert!(matches!(
        fields[1].value,
        Expr::TypeConst(TypeConst::Int, _)
    ));
    // A field-less struct instance parses: `S{}`.
    assert!(matches!(
        parse_ok("S{}"),
        Expr::StructInst { fields, .. } if fields.is_empty()
    ));
    // A spaced brace is not an instantiation: it is an apply of a struct value.
    let Expr::Apply { argument, .. } = parse_ok("S {.a 1}") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::StructValue { .. }));
}

#[test]
fn spaced_parens_are_apply_not_a_tuple_read() {
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
    // a glued paren is the tuple read, never an apply.
    let Expr::FieldRead { container, key, .. } = parse_ok("A(1)") else {
        panic!("expected a tuple read")
    };
    assert!(matches!(*container, Expr::Name(n, _, _) if n == "A"));
    assert!(matches!(*key, Expr::Int(1, _)));
}

#[test]
fn struct_value_errors_carry_spans() {
    // A struct value's brace must close.
    let err = parse_err("{.a 1");
    assert_eq!(err.span, Some((1, 6)));
    // `{}` — an empty field list — is a struct value, not an error.
    let Expr::StructValue { fields, span } = parse_ok("{}") else {
        panic!("expected a struct value")
    };
    assert!(fields.is_empty());
    assert_eq!(span, (1, 1));
}

#[test]
fn the_array_template_is_a_binary_form() {
    // `[template, length]` — the array template.
    let Expr::Array {
        template, length, ..
    } = parse_ok("[Int, 3]")
    else {
        panic!("expected an array template")
    };
    assert!(matches!(*template, Expr::TypeConst(TypeConst::Int, _)));
    assert!(matches!(*length, Expr::Int(3, _)));
    // The template is itself recursive: `[[Int, 2], 3]`.
    let Expr::Array { template, .. } = parse_ok("[[Int, 2], 3]") else {
        panic!("expected an array template")
    };
    assert!(matches!(*template, Expr::Array { .. }));
    // A `[` right after an expression is an index, never a template — an
    // array argument needs parens.
    let Expr::Apply { argument, .. } = parse_ok("f ([1, 2])") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::Array { .. }));
    // The array's template is the builtin `Array` constant.
    assert!(matches!(
        parse_ok("Array"),
        Expr::TypeConst(TypeConst::Array, _)
    ));
}

#[test]
fn an_array_instance_is_a_glued_bracket() {
    // `T[1, 2]` — an instance of the template `T`.
    let Expr::ArrayInst {
        template, elements, ..
    } = parse_ok("T[1, 2]")
    else {
        panic!("expected an array instance")
    };
    assert!(matches!(*template, Expr::Name(n, _, _) if n == "T"));
    assert_eq!(elements.len(), 2);
    // A trailing separator makes a single-element instance.
    let Expr::ArrayInst { elements, .. } = parse_ok("T[1,]") else {
        panic!("expected an array instance")
    };
    assert_eq!(elements.len(), 1);
    // An empty instance has no elements.
    let Expr::ArrayInst { elements, .. } = parse_ok("T[]") else {
        panic!("expected an array instance")
    };
    assert!(elements.is_empty());
    // An instance's element may carry the `@shallow` marker.
    let Expr::ArrayInst { elements, .. } = parse_ok("T[@shallow x,]") else {
        panic!("expected an array instance")
    };
    assert_eq!(elements.len(), 1);
    assert!(matches!(elements[0], Expr::Shallow(..)));
}

#[test]
fn the_index_postfix() {
    let Expr::Index { array, index, .. } = parse_ok("a[0]") else {
        panic!("expected an index")
    };
    assert!(matches!(*array, Expr::Name(..)));
    assert!(matches!(*index, Expr::Int(0, _)));
    // A spaced `[` is a fresh array template: `a [0, 1]` is an application.
    assert!(matches!(
        parse_ok("a [0, 1]"),
        Expr::Apply {
            argument,
            ..
        } if matches!(*argument, Expr::Array { .. })
    ));
    // Chained: a[0][1] = (a[0])[1].
    let Expr::Index { array, .. } = parse_ok("a[0][1]") else {
        panic!("expected an index")
    };
    assert!(matches!(*array, Expr::Index { .. }));
    // An array template is indexable: ([1, 2])[1].
    let Expr::Index { array, .. } = parse_ok("([1, 2])[1]") else {
        panic!("expected an index")
    };
    assert!(matches!(*array, Expr::Array { .. }));
}

#[test]
fn index_errors_carry_spans() {
    let err = parse_err("a[0");
    assert_eq!(err.span, Some((1, 4)));
}

#[test]
fn the_retired_angle_type_prefix_is_a_parse_error() {
    let err = parse_err("<Int>");
    assert_eq!(err.span, Some((1, 1)));
}

#[test]
fn parse_errors_carry_spans() {
    let err = parse_err("x =>");
    assert_eq!(
        err.message,
        "expected '@assert', '@gc', 'int2float', 'float2int', 'exp', 'sqrt', 'floor', 'sin', 'cos', 'ln', 'max', 'min', 'table', 'set', an expression, ':', '#', '!', '?', or '~', found the end of the program"
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
    assert!(err.message.contains("=>"));
}

#[test]
fn an_underscore_is_a_placeholder_in_type_and_term_position() {
    let Expr::Annotation { r#type, .. } = parse_ok("x : _") else {
        panic!("expected an annotation")
    };
    assert!(matches!(*r#type.as_deref().unwrap(), Expr::Placeholder(_)));
    // In term position `_` is the same placeholder, not an ordinary name.
    assert!(matches!(parse_ok("_"), Expr::Placeholder(_)));
    // As an annotated *value*, `_` is the placeholder in the value slot.
    let Expr::Annotation { value, .. } = parse_ok("_ : Int") else {
        panic!("expected an annotation")
    };
    assert!(matches!(*value, Expr::Placeholder(_)));
}

#[test]
fn an_at_prefix_marker_parses_as_its_own_node() {
    let e = parse_ok("@assert (1 == 1)");
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
fn an_at_assert_binds_a_full_application_but_tighter_than_a_binary_operator() {
    // `! f x` asserts `f x` — the application is the operand.
    let e = parse_ok("@assert f x");
    assert!(matches!(
        e,
        Expr::Assert { value, .. } if matches!(*value, Expr::Apply { .. })
    ));
    // `@assert x <= 3` is `(@assert x) <= 3`, not `@assert (x <= 3)` — `@assert` binds tighter than `<=`.
    let e = parse_ok("@assert x <= 3");
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
fn an_at_in_membership_test_parses_at_the_comparison_level() {
    // `a @in S` — the left operand is the application (it binds tighter).
    let e = parse_ok("type_of v @in set(Int, Float)");
    let Expr::BinOp {
        operator: BinOp::In,
        left,
        right,
        ..
    } = e
    else {
        panic!("expected a membership test, got {e:?}")
    };
    assert!(
        matches!(*left, Expr::Apply { .. }),
        "`type_of v` is the left operand"
    );
    assert!(matches!(*right, Expr::Set(..)));
    // Left-associative at the comparison rung: `x @in S == 1` is
    // `(x @in S) == 1`.
    let e = parse_ok("Int @in set(Int) == 1");
    assert!(matches!(
        e,
        Expr::BinOp {
            operator: BinOp::Eq,
            left,
            ..
        } if matches!(*left, Expr::BinOp { operator: BinOp::In, .. })
    ));
}

#[test]
fn a_block_is_bindings_followed_by_a_final_expression() {
    let Expr::Block {
        statements, expr, ..
    } = parse_ok("{a := 1; a}")
    else {
        panic!("expected a block")
    };
    assert_eq!(statements.len(), 1);
    let Stmt::Binding(binding) = &statements[0] else {
        panic!("expected a binding")
    };
    assert_eq!(binding.name, "a");
    assert!(matches!(binding.value, Expr::Int(1, _)));
    assert!(matches!(*expr, Expr::Name(name, _, _) if name == "a"));
    // A block with only a final expression.
    assert!(matches!(parse_ok("{5}"), Expr::Block { .. }));
    // Statements are graph-shared bindings, not just literals.
    assert!(matches!(
        parse_ok("{a := [1, 2]; b := a[0]; b}"),
        Expr::Block { statements, .. } if statements.len() == 2
    ));
}

#[test]
fn a_block_can_be_a_lambda_body() {
    let Expr::Lambda { r#return, .. } = parse_ok("x => {a := x; a}") else {
        panic!("expected a lambda")
    };
    assert!(matches!(*r#return, Expr::Block { .. }));
}

#[test]
fn a_block_can_be_written_without_semicolons() {
    // {a = 1\nb = 2\nb} — newlines separate the block's statements.
    let Expr::Block {
        statements, expr, ..
    } = parse_ok("{a := 1\nb := 2\nb}")
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
    assert!(matches!(*expr, Expr::Name(name, _, _) if name == "b"));
    // A trailing newline before the `}` is fine.
    assert!(matches!(parse_ok("{a := 1\na\n}"), Expr::Block { .. }));
}

#[test]
fn a_block_is_an_atom() {
    let Expr::Apply { argument, .. } = parse_ok("f {a := 1; a}") else {
        panic!("expected an apply")
    };
    assert!(matches!(*argument, Expr::Block { .. }));
    // Blocks nest.
    let Expr::Block { expr, .. } = parse_ok("{{a := 1; a}}") else {
        panic!("expected a block")
    };
    assert!(matches!(*expr, Expr::Block { .. }));
    // A block composes with postfix and annotation forms.
    assert!(matches!(
        parse_ok("{a := 1; a}[0]"),
        Expr::Index { array, .. } if matches!(*array, Expr::Block { .. })
    ));
    assert!(matches!(
        parse_ok("{a := 1; a} : Int"),
        Expr::Annotation { .. }
    ));
}

#[test]
fn block_errors_carry_spans() {
    let err = parse_err("{a := 1; a");
    assert_eq!(err.span, Some((1, 11)));
    // The empty block is gone: `{}` is the empty struct value.
    assert!(matches!(
        parse_ok("{}"),
        Expr::StructValue { fields, .. } if fields.is_empty()
    ));
    // A block whose last statement is a binding is a struct-returning block.
    let Expr::RecordBlock { fields, .. } = parse_ok("{a := 1}") else {
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
fn the_recovered_error_walk_does_not_overflow_a_shallow_caller_stack() {
    // The walk recurses per term on the caller's thread; a shallow one stands
    // in (`docs/notes/code-audit.md` P1-23).
    const TERMS: usize = 2000;
    const STACK_BYTES: usize = 128 * 1024;
    let tokens = lex(&("1+".repeat(TERMS) + "1")).tokens;
    let Parsed { program, .. } = parse(&tokens);
    let (program, blocks) = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let blocks = collect_error_blocks(&program);
            (program, blocks)
        })
        .expect("spawn the shallow-stack thread")
        .join()
        .expect("the walk returns rather than aborting");
    assert!(blocks.is_empty(), "the chain carries no recovered errors");
    drop(program);
}

#[test]
fn broken_statements_are_recovered() {
    // A broken binding value skips to the next separator and the rest of
    // the program is reached.
    let tokens = lex("a := ; b := 2; 5").tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert_eq!(errors.len(), 1, "the broken binding's error");
    assert_eq!(
        errors[0].message,
        "expected '@assert', '@gc', 'int2float', 'float2int', 'exp', 'sqrt', 'floor', 'sin', 'cos', 'ln', 'max', 'min', 'table', 'set', an expression, ':', '#', '!', '?', or '~', found a separator"
    );
    assert_eq!(program.statements.len(), 2);
    assert!(matches!(program.expr, Some(Expr::Int(5, _))));
    // An unclosed paren in a value is recovered the same way.
    let tokens = lex("a := (x; 5").tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert_eq!(errors.len(), 1);
    assert!(matches!(program.expr, Some(Expr::Int(5, _))));
    // A statement that cannot start is skipped entirely, and the parse
    // continues to the final expression.
    let tokens = lex("a := 1; => ; b := 2; b").tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert_eq!(errors.len(), 1);
    assert!(matches!(program.expr, Some(Expr::Name(name, _, _)) if name == "b"));
}

#[test]
fn dangling_operators_are_recovered() {
    // A missing operand consumes the operator and recovers the operand as an
    // error node, with no cascade.
    let cases: &[(&str, (u32, u32))] = &[
        ("a := 1 + ; b := 2; b", (1, 10)),
        ("a := 1 <= ; b := 2; b", (1, 11)),
        ("a := 1 : ; b := 2; b", (1, 10)),
        ("a := x => ; b := 2; b", (1, 11)),
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
        assert!(matches!(program.expr, Some(Expr::Name(name, _, _)) if name == "b"));
        let Stmt::Binding(binding) = &program.statements[0].stmt else {
            panic!("{source}: first statement is a binding");
        };
        // The value keeps the operator's shape with an error node in the
        // operand slot.
        let recovered = matches!(
            &binding.value,
            Expr::BinOp { right, .. }
                if matches!(**right, Expr::Err { .. } | Expr::Placeholder(_))
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

/// The parse worker is process-lived, so a panic in one parse must not take it
/// down (`docs/notes/code-audit.md` D13).
#[test]
fn a_panicking_parse_leaves_the_worker_alive() {
    let worker = ParseWorker::global();
    let caught = std::panic::catch_unwind(|| worker.run(|| panic!("deliberate: a parse panicked")));
    assert!(caught.is_err(), "the panic must reach the caller");

    // The worker still answers, and a real parse after it is unaffected.
    let tokens = lex("a := 1\na\n").tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert!(errors.is_empty(), "unexpected parse errors: {errors:?}");
    assert!(
        program.expr.is_some(),
        "the parse after a panic is complete"
    );
}

/// Both parses share one worker thread, which is not the caller's
/// (`docs/notes/code-audit.md` D13).
#[test]
fn parses_share_one_worker_thread() {
    let here = std::thread::current().id();
    let worker = ParseWorker::global();
    let first = worker.run(|| std::thread::current().id());
    let second = worker.run(|| std::thread::current().id());
    assert_eq!(first, second, "the worker is reused rather than respawned");
    assert_ne!(first, here, "the parse runs off the caller's stack");

    let tokens = lex("a := 1\na\n").tokens;
    assert!(parse(&tokens).errors.is_empty());
}
