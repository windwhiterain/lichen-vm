//! The parser: tokens → AST, with error recovery.
//!
//! A program is `name = expr; …` bindings and bare expressions followed by a
//! final expression (see [`Program`]) — the same statement list, wrapped in
//! `{ … }`, forms a block expression (see [`Expr::Block`]).  A binding
//! without `let` is *block-wide*: its name is in scope throughout the block,
//! so it may recurse with itself and with the block's other bindings.  A
//! `let` before a binding (`let a = …`) is *restrictive*: the name is in
//! scope only in later statements, never in its own value.  Statements are
//! separated by `;` or a newline (the lexer lexes both as `Semicolon`), and
//! consecutive, leading, and trailing separators are all tolerated.  A
//! binding at statement start is `name =` (or `let name =`); anything else
//! is an expression — a bare expression is a statement anywhere, and only
//! the last statement is the list's value.  Within an expression, one
//! grammar covers terms and types (types are expressions); the *mode* — term
//! or type — is applied by a post-pass ([`apply_type_mode`]): the
//! annotation's right side (and a struct's fields) are type expressions,
//! everywhere else a term, deciding whether `(a, b)` is a `Tuple` value or a
//! `TypeTuple` type expression and whether `_` is a [`Expr::Placeholder`] or
//! an ordinary name.  Angle brackets are exclusively type-level and need no
//! mode: `<a, b>` is always a `TypeTuple`, `struct<T1, T2>` always a
//! `StructType`, and `T<e>` (a `<` directly after an expression) is always
//! the array type.  A `[` directly after an expression is always an index
//! `e[i]` — the array literal is the prefix `[e, …]` form, so no whitespace
//! rule decides, and an array literal in argument position needs parens:
//! `f ([1, 2])`.  Precedence (loosest → tightest): `=>` (right) → `:`
//! (right) → `->` (right) → `<=`/`==` (left) → `+`/`-` (left) → application
//! (left) → postfix `<e>` / `[e]` / `(…)` / atoms.  A `(` immediately after
//! an expression — no space between them — is *struct instantiation*
//! (`C(f1, …, fn)`, zero or more comma-separated fields; a single field
//! needs no trailing comma, `C()` is a field-less instance), and it lowers
//! to [`ExprKind::Instantiate`].  A spaced `(` is a paren atom; the same
//! juxtaposition rule (`apply := atom atom`) makes it the argument, so there
//! is no distinct spaced-apply form.  `name =>` starts a lambda only
//! in prefix position, so `f x => e` is a parse error rather than
//! `f (x => e)`; `name : T => e` (and, parens being transparent for
//! annotations, `(name : T) => e`) is a lambda whose parameter is annotated
//! with `T`.  `if cond then e1 else e2` is a conditional expression (the
//! `then`/`else` keywords delimit the branches, which extend maximally).
//!
//! Errors are *recovered*, not fail-fast: a broken statement is skipped
//! (to the next separator) and parsed again as a fresh statement, a broken
//! final expression becomes an error node, and the parse continues — every
//! recovered error is reported, and the partial program still compiles and
//! checks ([`Expr::Err`] lowers to an inference placeholder).  `parse`
//! therefore produces a program (possibly with error nodes) for almost any
//! input; only an input with no parseable statement at all fails outright.

use chumsky::error::RichReason;
use chumsky::input::Stream;
use chumsky::prelude::*;

use lichen_highlevel::ir::Span;

use crate::ast::{BinOp, Binding, Expr, Program, Stmt, TypeConst};
use crate::diag::{Diag, Stage};
use crate::lex::{Token, TokenKind};

/// The parser's input: the token stream, whose spans are token *indices*
/// (each item is one token).
type In<'a> = Stream<std::iter::Cloned<std::slice::Iter<'a, Token>>>;
/// The parser's error: rich errors whose spans are token-index ranges and
/// whose "found" value is the offending token.
type E<'a> = extra::Err<Rich<'a, Token, SimpleSpan<usize>>>;

/// The result of parsing: the (possibly partial) program plus every error
/// encountered along the way.
pub struct Parsed {
    pub program: Program,
    pub errors: Vec<Diag>,
}

/// Parse a token stream.  See the module docs for the recovery behavior.
///
/// The parser's construction and run recurse deeply (a fixed depth, driven
/// by the size of the combinator grammar) and comfortably exceed the main
/// thread's stack, so the parse runs on a worker thread with a large stack.
pub fn parse(tokens: &[Token]) -> Parsed {
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn_scoped(scope, || parse_inner(tokens))
            .expect("spawn the parse worker");
        let (program, errors) = worker.join().expect("the parse worker panicked");
        Parsed {
            program,
            // Parse diagnostics never carry a checker `Diag`, so the
            // worker's stripped form is exactly reconstructible.
            errors: errors
                .into_iter()
                .map(|(span, message, stage)| Diag {
                    span,
                    message,
                    stage,
                    check: None,
                })
                .collect(),
        }
    })
}

/// The worker's result: the program plus its diagnostics in a `Send` form
/// (the crate's `Diag` embeds the checker's, which is not `Send`).
type ParseOut = (Program, Vec<(Option<Span>, String, Stage)>);

fn parse_inner(tokens: &[Token]) -> ParseOut {
    let stream = Stream::from_iter(tokens.iter().cloned());
    let parser = program_parser(tokens);
    let (output, errs) = parser.parse(stream).into_output_errors();
    let mut errors: Vec<(Option<Span>, String, Stage)> = Vec::new();
    for e in &errs {
        let diag = diag_from(tokens, e);
        if !errors
            .iter()
            .any(|(span, message, _)| *span == diag.span && *message == diag.message)
        {
            errors.push((diag.span, diag.message, diag.stage));
        }
    }
    let program = match output {
        Some(program) => apply_type_mode(program),
        None => {
            let span = errors
                .first()
                .and_then(|(span, ..)| *span)
                .unwrap_or_else(|| span_at(tokens, tokens.len().saturating_sub(1)));
            errors.push((
                Some(span),
                "the program could not be parsed".to_string(),
                Stage::Parse,
            ));
            Program {
                statements: Vec::new(),
                expr: Expr::Err(span),
            }
        }
    };
    (program, errors)
}

// ---------------------------------------------------------------------------
// Grammar

/// A parser matching a single token of the given kind, labelled with the
/// kind's human-readable spelling.
fn token<'a>(kind: TokenKind) -> impl Parser<'a, In<'a>, Token, E<'a>> + Clone {
    let label = kind.describe();
    any::<In<'a>, E<'a>>()
        .filter(move |t: &Token| t.kind == kind)
        .labelled(label)
}

/// A name token — an identifier, `_` included (the mode post-pass decides
/// whether `_` is a placeholder).
fn name<'a>() -> impl Parser<'a, In<'a>, (String, Span), E<'a>> + Clone {
    any::<In<'a>, E<'a>>()
        .filter(|t: &Token| matches!(t.kind, TokenKind::Name(_)))
        .map(|t| match t.kind {
            TokenKind::Name(n) => (n, t.span),
            _ => unreachable!("filtered for a name"),
        })
        .labelled("a name")
}

/// The `(line, col)` span of the token at token-index `i` — the parse
/// fallback for positions at or past the end of the stream.
fn span_at(tokens: &[Token], index: usize) -> Span {
    tokens
        .get(index)
        .map(|t| t.span)
        .unwrap_or_else(|| tokens.last().map(|t| t.span).unwrap_or((1, 1)))
}

/// The program: the statement list, then the end of the input.  One
/// expression parser is built here and threaded through the whole grammar —
/// the statement list, the bindings, and the block bodies all recurse
/// through the same [`expression`] recursion point.
fn program_parser<'a>(tokens: &'a [Token]) -> impl Parser<'a, In<'a>, Program, E<'a>> {
    let expr = expression(tokens);
    // The lexer appends a real `Eof` token, so the end of the input is that
    // token, not stream exhaustion — `end()` would fail with it unconsumed.
    statement_list(tokens, expr)
        .then_ignore(token(TokenKind::Eof).ignored())
        .map(|(statements, expr)| Program { statements, expr })
}

/// The statement list: `elem (seps elem)*`, followed by trailing separators
/// — every statement after the first must be preceded by a separator, and
/// only the last statement is the list's value.  The last element is the
/// final expression; a trailing binding (or an empty list) is an error, and
/// the list's value becomes an error node.
fn statement_list<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, (Vec<Stmt>, Expr), E<'a>> + Clone {
    let seps = token(TokenKind::Semicolon)
        .ignored()
        .repeated()
        .collect::<Vec<_>>();
    let seps1 = token(TokenKind::Semicolon)
        .ignored()
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>();

    // A statement: a binding or a bare expression.  Broken statements are
    // recovered by skipping tokens and retrying the statement parser, so a
    // bad statement does not swallow the rest of the program.
    // A statement: a binding or a bare expression.  Broken statements are
    // recovered by skipping tokens and retrying the statement parser, so a
    // bad statement does not swallow the rest of the program.  The retry
    // stops at the end of the input *or* a block's closing brace — a block
    // must not swallow the tokens after its `}`.
    let elem = statement(tokens, expr).recover_with(skip_then_retry_until(
        any::<In<'a>, E<'a>>().ignored(),
        choice((
            token(TokenKind::Eof).ignored(),
            token(TokenKind::RBrace).ignored(),
        )),
    ));

    seps.clone()
        .then(elem.clone())
        .then(
            (seps1.clone().then(elem.clone()))
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(seps)
        .map(|(((leading, first), rest), _trailing)| {
            let _ = leading;
            std::iter::once(first)
                .chain(rest.into_iter().map(|(_, e)| e))
                .collect::<Vec<Stmt>>()
        })
        .validate(|mut elems, me, emit| match elems.pop() {
            Some(Stmt::Expr(final_expr)) => (elems, final_expr),
            Some(Stmt::Binding(binding)) => {
                emit.emit(Rich::custom(
                    me.span(),
                    "a program must end with an expression",
                ));
                let span = binding.span;
                elems.push(Stmt::Binding(binding));
                (elems, Expr::Err(span))
            }
            None => {
                emit.emit(Rich::custom(
                    me.span(),
                    "a program must end with an expression",
                ));
                let span = span_at(tokens, me.span().start);
                (elems, Expr::Err(span))
            }
        })
}

/// A statement: a binding (`let name = …` / `name = …`) or a bare
/// expression.
fn statement<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Stmt, E<'a>> + Clone {
    choice((
        binding(tokens, expr.clone()).map(Stmt::Binding),
        expr.map(Stmt::Expr),
    ))
}

/// `let name = expr` (restrictive) or `name = expr` (block-wide).
fn binding<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Binding, E<'a>> + Clone {
    let head = choice((
        token(TokenKind::KwLet)
            .ignore_then(name())
            .then_ignore(token(TokenKind::Equals))
            .map(|n| (n, true)),
        name()
            .then_ignore(token(TokenKind::Equals))
            .map(|n| (n, false)),
    ));
    // A broken binding value is recovered, not fatal: skip the offending
    // tokens (stopping before the next separator, which the statement list
    // still consumes) and substitute an error node, so the binding still
    // parses and the rest of the program is reached.
    let value = expr.recover_with(via_parser(
        any::<In<'a>, E<'a>>()
            .filter(|t: &Token| t.kind != TokenKind::Semicolon)
            .ignored()
            .repeated()
            .map_with(move |_, me| Expr::Err(span_at(tokens, me.span().start))),
    ));
    head.then(value)
        .map(|(((name, span), restrictive), value)| Binding {
            name,
            span,
            value,
            restrictive,
        })
}

/// A full expression in an operator's operand position — or, when the
/// operator is dangling (its operand missing at a statement boundary), a
/// non-consuming error node at the point the operand was expected.  The
/// recovery keeps the operator consumed, so a dangling operator never
/// leaks into the statement list and breaks it; the original "expected an
/// expression" error is still emitted.
fn operand<'a>(
    tokens: &'a [Token],
    p: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    p.recover_with(via_parser(
        empty::<In<'a>, E<'a>>().map_with(move |_, me| Expr::Err(span_at(tokens, me.span().start))),
    ))
}

/// An expression: the precedence chain over atoms, with `=>` as the loosest
/// (right-associative) operator, validated into a lambda.
fn expression<'a>(tokens: &'a [Token]) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    recursive(|expr| {
        let atom = atom_parser(tokens, expr.clone());

        // Application: juxtaposition, left-associative, binds tighter than
        // every operator.
        let application =
            atom.clone()
                .then(atom.repeated().collect::<Vec<_>>())
                .map(|(f, args)| {
                    args.into_iter().fold(f, |acc, arg| {
                        let span = acc.span();
                        Expr::Apply {
                            function: Box::new(acc),
                            argument: Box::new(arg),
                            span,
                        }
                    })
                });

        // `+` / `-`, left-associative.
        let arith = choice((
            token(TokenKind::Plus).to(BinOp::Add),
            token(TokenKind::Minus).to(BinOp::Sub),
        ));
        let term1 = application.clone().foldl(
            arith.then(operand(tokens, application.clone())).repeated(),
            |lhs, (op, rhs)| {
                let span = lhs.span();
                Expr::BinOp {
                    operator: op,
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                    span,
                }
            },
        );

        // `<=` / `==`, left-associative.
        let cmp = choice((
            token(TokenKind::Leq).to(BinOp::Leq),
            token(TokenKind::Eq).to(BinOp::Eq),
        ));
        let term2 = term1.clone().foldl(
            cmp.then(operand(tokens, term1.clone())).repeated(),
            |lhs, (op, rhs)| {
                let span = lhs.span();
                Expr::BinOp {
                    operator: op,
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                    span,
                }
            },
        );

        // `->`, right-associative.
        let term3 = term2
            .clone()
            .then(
                token(TokenKind::Arrow)
                    .ignore_then(operand(tokens, term2.clone()))
                    .repeated()
                    .collect::<Vec<_>>(),
            )
            .map(|(first, rest)| {
                fold_right(first, rest, |lhs, rhs| {
                    let span = lhs.span();
                    Expr::Arrow {
                        parameter: Box::new(lhs),
                        r#return: Box::new(rhs),
                        span,
                    }
                })
            });

        // `:` — the annotation, right-associative; its right side is a type
        // expression (applied by the mode post-pass).
        let term4 = term3
            .clone()
            .then(
                token(TokenKind::Colon)
                    .ignore_then(operand(tokens, term3.clone()))
                    .repeated()
                    .collect::<Vec<_>>(),
            )
            .map(|(first, rest)| {
                fold_right(first, rest, |lhs, rhs| {
                    let span = lhs.span();
                    Expr::Annotation {
                        value: Box::new(lhs),
                        r#type: Box::new(rhs),
                        span,
                    }
                })
            });

        term4
            .then(
                token(TokenKind::FatArrow)
                    .ignore_then(operand(tokens, expr.clone()))
                    .repeated()
                    .collect::<Vec<_>>(),
            )
            .map(|(first, rest)| match rest.into_iter().next() {
                Some(rhs) => Pre::FatArrow(Box::new(first), Box::new(rhs)),
                None => Pre::E(first),
            })
            .map(|pre| pre)
            .validate(|pre, me, emit| match pre {
                Pre::E(e) => e,
                Pre::FatArrow(lhs, rhs) => match *lhs {
                    Expr::Name(parameter, span) => Expr::Lambda {
                        parameter,
                        parameter_span: span,
                        parameter_type: None,
                        r#return: rhs,
                        span,
                    },
                    Expr::Annotation {
                        value,
                        r#type,
                        span,
                    } => match *value {
                        Expr::Name(parameter, parameter_span) => Expr::Lambda {
                            parameter,
                            parameter_span,
                            parameter_type: Some(r#type),
                            r#return: rhs,
                            span,
                        },
                        value => {
                            emit.emit(Rich::custom(me.span(), "expected a name before '=>'"));
                            value
                        }
                    },
                    other => {
                        emit.emit(Rich::custom(me.span(), "expected a name before '=>'"));
                        other
                    }
                },
            })
    })
}

/// Right-fold `first (op rhs)*` into `op(first, op(rhs₁, … op(rhsₙ₋₁, rhsₙ)))`.
fn fold_right<F>(first: Expr, rest: Vec<Expr>, combine: F) -> Expr
where
    F: Fn(Expr, Expr) -> Expr,
{
    let mut it = rest.into_iter().rev();
    let mut acc = match it.next() {
        Some(rhs) => rhs,
        None => return first,
    };
    for rhs in it {
        acc = combine(rhs, acc);
    }
    combine(first, acc)
}

/// A `=>` chain before it is validated into a lambda.
#[derive(Clone, Debug)]
enum Pre {
    E(Expr),
    FatArrow(Box<Expr>, Box<Expr>),
}

/// The atoms, with their postfix forms: `e[i]` (array index), `a(k)` (the
/// positional slot read — an adjacent single-expression paren), `t{k}`
/// (table lookup), `T<e>` (array type), and `C(...)` (struct instantiation
/// — any other adjacent paren content).  The bracket and angle forms are
/// always postfix, with no whitespace rule; a paren or a brace is postfix
/// *only when adjacent* (no space before it) — a spaced `(` is a paren
/// atom, and a spaced `{` is a block: the application rule treats either
/// as an argument, never this postfix.
fn atom_parser<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone + 'a,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    let primary = choice((
        any::<In<'a>, E<'a>>()
            .filter(|t: &Token| matches!(t.kind, TokenKind::Int(_)))
            .map(|t| match t.kind {
                TokenKind::Int(n) => Expr::Int(n, t.span),
                _ => unreachable!("filtered for an int"),
            }),
        token(TokenKind::KwInt).map(|t| Expr::TypeConst(TypeConst::Int, t.span)),
        token(TokenKind::KwType).map(|t| Expr::TypeConst(TypeConst::Type, t.span)),
        name().map(|(n, span)| Expr::Name(n, span)),
        paren(tokens, expr.clone()),
        array_literal(tokens, expr.clone()),
        table_literal(tokens, expr.clone()),
        block(tokens, expr.clone()),
        angle_tuple(tokens, expr.clone()),
        struct_type(tokens, expr.clone()),
        if_expr(tokens, expr.clone()),
    ))
    .labelled("an expression");

    // The postfix forms, chained left.  A `[` after an expression is always
    // an index, a `<` always an array type.  A `(` or a `{` is postfix only
    // when *adjacent* — the bracket comes straight after the expression, no
    // space between: a `(` holding a single comma-free expression is the
    // positional slot read `a(k)`, any other adjacent `(` is struct
    // instantiation, an adjacent `{` is a table lookup.  A spaced `(` is a
    // paren atom and a spaced `{` a block; the application rule treats
    // either as an argument, never this postfix.
    let adjacent_paren = any::<In<'a>, E<'a>>()
        .filter(|t: &Token| t.kind == TokenKind::LParen && !t.space_before)
        .labelled("a slot read or struct instantiation '('");
    let adjacent_brace = any::<In<'a>, E<'a>>()
        .filter(|t: &Token| t.kind == TokenKind::LBrace && !t.space_before)
        .labelled("table lookup '{'");
    let postfix = choice((
        token(TokenKind::LBracket)
            .ignore_then(expr.clone())
            .then_ignore(token(TokenKind::RBracket))
            .map(Postfix::Index),
        token(TokenKind::LAngle)
            .ignore_then(expr.clone())
            .then_ignore(token(TokenKind::RAngle))
            .map(Postfix::TypeArray),
        adjacent_brace
            .ignore_then(expr.clone())
            .then_ignore(token(TokenKind::RBrace))
            .map(Postfix::TableFind),
        adjacent_paren
            .ignore_then(paren_fields(expr.clone()))
            .then_ignore(token(TokenKind::RParen))
            .map(Postfix::Paren),
    ));

    primary
        .then(postfix.repeated().collect::<Vec<_>>())
        .map(|(atom, postfixes)| {
            postfixes.into_iter().fold(atom, |acc, p| {
                let span = acc.span();
                match p {
                    Postfix::Index(index) => Expr::Index {
                        array: Box::new(acc),
                        index: Box::new(index),
                        span,
                    },
                    Postfix::TableFind(key) => Expr::TableFind {
                        container: Box::new(acc),
                        key: Box::new(key),
                        span,
                    },
                    Postfix::TypeArray(length) => Expr::TypeArray {
                        element_type: Box::new(acc),
                        length: Box::new(length),
                        span,
                    },
                    Postfix::Paren((fields, saw_comma)) => {
                        // The single comma-free expression `a(e)` is the
                        // positional slot read; every empty or comma-bearing
                        // form instantiates (`a()`, `a(,)`, `a(e,)`,
                        // `a(e1, e2)`), mirroring the tuple grammar's `()`
                        // unit vs `(,)` empty tuple.
                        if fields.len() == 1 && !saw_comma {
                            Expr::FieldRead {
                                container: Box::new(acc),
                                key: Box::new(fields.into_iter().next().unwrap()),
                                span,
                            }
                        } else {
                            Expr::StructInst {
                                callee: Box::new(acc),
                                fields,
                                span,
                            }
                        }
                    }
                }
            })
        })
        .boxed()
}

/// A postfix form's payload, folded left over the atom.
#[derive(Clone, Debug)]
enum Postfix {
    Index(Expr),
    TableFind(Expr),
    TypeArray(Expr),
    Paren((Vec<Expr>, bool)),
}

/// The content of an adjacent `(`: zero or more expressions separated by
/// commas, a trailing comma tolerated — plus *whether any comma appeared*.
/// The caller splits the two single-expression shapes: `a(e)` (one
/// expression, no comma) is the positional slot read, while every
/// comma-bearing or empty form instantiates.  `a(,)` parses as the empty
/// field list (the trailing comma is the whole content) — the instantiation
/// spelling of the tuple grammar's `(,)` empty tuple, against the future
/// `()` unit.
fn paren_fields<'a>(
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, (Vec<Expr>, bool), E<'a>> + Clone {
    let first = expr.clone().or_not();
    first
        .then(
            token(TokenKind::Comma)
                .ignore_then(expr.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(token(TokenKind::Comma).or_not())
        .map(|((first, rest), trailing)| {
            let saw_comma = !rest.is_empty() || trailing.is_some();
            let mut fields = Vec::new();
            if let Some(f0) = first {
                fields.push(f0);
            }
            fields.extend(rest);
            (fields, saw_comma)
        })
}

/// `(e)` — grouping, parens transparent; `(e1, …, en)` — a tuple (a
/// `TypeTuple` in type position, applied by the mode post-pass).  A trailing
/// comma is tolerated.
fn paren<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::LParen)
        .ignore_then(expr.clone())
        .then(
            token(TokenKind::Comma)
                .ignore_then(expr.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(token(TokenKind::Comma).or_not())
        .then_ignore(token(TokenKind::RParen))
        .map_with(|((first, rest), trailing), me| {
            let span = span_at(tokens, me.span().start);
            if rest.is_empty() && trailing.is_none() {
                first
            } else {
                Expr::Tuple(std::iter::once(first).chain(rest).collect(), span)
            }
        })
}

/// `[e1, …, en]` — an array literal (in type position its elements are
/// types, applied by the mode post-pass).  A trailing comma is tolerated.
/// An element may be prefixed with the `~` shallow marker — the only place
/// `~` is accepted.
fn array_literal<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    let element = tilde_marked(expr.clone());
    token(TokenKind::LBracket)
        .ignore_then(element.clone())
        .then(
            token(TokenKind::Comma)
                .ignore_then(element.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(token(TokenKind::Comma).or_not())
        .then_ignore(token(TokenKind::RBracket))
        .map_with(|((first, rest), _trailing), me| {
            Expr::Array(
                std::iter::once(first).chain(rest).collect(),
                span_at(tokens, me.span().start),
            )
        })
}

/// `table { k1 :: v1, k2 :: v2, … }` — a constant table literal.  Each
/// entry is a key/value pair separated by `::`.  Keys and values are full
/// expressions; the double colon is not part of the expression grammar, so
/// it unambiguously separates the pair.  Anything else recovers as a parse
/// error.  Entries are comma-separated with a tolerated trailing comma;
/// `table {}` is the empty table.
fn table_literal<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    let entry = expr
        .clone()
        .then(
            token(TokenKind::DoubleColon)
                .ignore_then(operand(tokens, expr.clone()))
                .or_not(),
        )
        .validate(|(key, value), me, emit| match value {
            Some(value) => (key, value),
            None => {
                emit.emit(Rich::custom(
                    me.span(),
                    "a table entry must be a `key :: value` pair",
                ));
                // Recover like any parse error: the entry's key compiles,
                // its value is a compile-time leaf.
                (key, Expr::Err(span_at(tokens, me.span().start)))
            }
        });
    token(TokenKind::KwTable)
        .ignore_then(token(TokenKind::LBrace))
        .ignore_then(entry.clone().or_not())
        .then(
            token(TokenKind::Comma)
                .ignore_then(entry.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(token(TokenKind::Comma).or_not())
        .then_ignore(token(TokenKind::RBrace))
        .map_with(|((first, mut rest), _trailing), me| {
            let mut entries = Vec::new();
            if let Some(first) = first {
                entries.push(first);
            }
            entries.append(&mut rest);
            Expr::Table(entries, span_at(tokens, me.span().start))
        })
}

/// An array element with an optional `~` prefix: `~ e`, `~2 e`, or a plain
/// `e`.  The marker token carries the depth (`usize::MAX` = the bare `~`);
/// the marker wraps the element as [`Expr::Shallow`], keeping the marker's
/// own span.
fn tilde_marked<'a>(
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    any::<In<'a>, E<'a>>()
        .filter_map(|t: Token| match t.kind {
            TokenKind::Tilde(depth) => Some((depth, t.span)),
            _ => None,
        })
        .labelled("'~'")
        .or_not()
        .then(expr)
        .map(|(marker, element)| match marker {
            Some((depth, span)) => Expr::Shallow(Box::new(element), depth, span),
            None => element,
        })
}

/// `<e1, …, en>` — always a `TypeTuple`, in term and type position alike
/// (angle brackets are exclusively type-level, so there is no mode flag
/// here, unlike `( )`).  At least two elements: a single element is a typo
/// for either `(e)` grouping or a real tuple type.
fn angle_tuple<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::LAngle)
        .ignore_then(expr.clone())
        .then(
            token(TokenKind::Comma)
                .ignore_then(expr.clone())
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(token(TokenKind::RAngle))
        .map_with(|(first, rest), me| {
            Expr::TypeTuple(
                std::iter::once(first).chain(rest).collect(),
                span_at(tokens, me.span().start),
            )
        })
}

/// `struct<T1, …, Tn>` — a nominal struct type, positional fields.  The
/// fields are type expressions (applied by the mode post-pass), at least
/// one.
fn struct_type<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::KwStruct)
        .ignore_then(token(TokenKind::LAngle))
        .ignore_then(expr.clone())
        .then(
            token(TokenKind::Comma)
                .ignore_then(expr.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then_ignore(token(TokenKind::RAngle))
        .map_with(|(first, rest), me| {
            Expr::StructType(
                std::iter::once(first).chain(rest).collect(),
                span_at(tokens, me.span().start),
            )
        })
}

/// `{ stmt; …; expr }` — a block: scoped statements followed by the block's
/// value.  The body is the same statement list as a program's, recursing
/// through the same expression parser.
fn block<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::LBrace)
        .ignore_then(statement_list(tokens, expr))
        .then_ignore(token(TokenKind::RBrace))
        .map_with(|(statements, expr), me| Expr::Block {
            statements,
            expr: Box::new(expr),
            span: span_at(tokens, me.span().start),
        })
}

/// `if cond then e1 else e2` — a conditional.  `cond` is any expression up
/// to the `then` keyword (both keywords delimit it — neither is an atom or
/// an infix operator, so the condition cannot extend through them); the
/// branches extend maximally, like a lambda body.
fn if_expr<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::KwIf)
        .ignore_then(expr.clone())
        .then_ignore(token(TokenKind::KwThen))
        .then(expr.clone())
        .then_ignore(token(TokenKind::KwElse))
        .then(expr.clone())
        .map_with(|((condition, then_branch), else_branch), me| Expr::If {
            condition: Box::new(condition),
            then_branch: Box::new(then_branch),
            else_branch: Box::new(else_branch),
            span: span_at(tokens, me.span().start),
        })
}

// ---------------------------------------------------------------------------
// The mode post-pass

/// Reinterpret the parser's single-mode AST in the language's two modes:
/// the annotation's right side (and a struct's fields) are type expressions,
/// everywhere else is a term.  The parser builds everything as a term
/// (`(a, b)` a [`Expr::Tuple`], `_` a [`Expr::Name`]); this pass flips the
/// mode-sensitive nodes — `Tuple` → `TypeTuple`, `_` → [`Expr::Placeholder`]
/// — inside type positions.
fn apply_type_mode(program: Program) -> Program {
    fn expr(e: Expr, type_mode: bool) -> Expr {
        match e {
            Expr::Name(name, span) if type_mode && name == "_" => Expr::Placeholder(span),
            Expr::Lambda {
                parameter,
                parameter_span,
                parameter_type,
                r#return,
                span,
            } => Expr::Lambda {
                parameter,
                parameter_span,
                parameter_type: parameter_type.map(|t| Box::new(expr(*t, true))),
                r#return: Box::new(expr(*r#return, type_mode)),
                span,
            },
            Expr::Apply {
                function,
                argument,
                span,
            } => Expr::Apply {
                function: Box::new(expr(*function, type_mode)),
                argument: Box::new(expr(*argument, type_mode)),
                span,
            },
            Expr::BinOp {
                operator,
                left,
                right,
                span,
            } => Expr::BinOp {
                operator,
                left: Box::new(expr(*left, type_mode)),
                right: Box::new(expr(*right, type_mode)),
                span,
            },
            Expr::If {
                condition,
                then_branch,
                else_branch,
                span,
            } => Expr::If {
                condition: Box::new(expr(*condition, type_mode)),
                then_branch: Box::new(expr(*then_branch, type_mode)),
                else_branch: Box::new(expr(*else_branch, type_mode)),
                span,
            },
            Expr::Index { array, index, span } => Expr::Index {
                array: Box::new(expr(*array, type_mode)),
                index: Box::new(expr(*index, type_mode)),
                span,
            },
            Expr::TableFind {
                container,
                key,
                span,
            } => Expr::TableFind {
                container: Box::new(expr(*container, type_mode)),
                key: Box::new(expr(*key, type_mode)),
                span,
            },
            Expr::FieldRead {
                container,
                key,
                span,
            } => Expr::FieldRead {
                container: Box::new(expr(*container, type_mode)),
                key: Box::new(expr(*key, type_mode)),
                span,
            },
            Expr::Annotation {
                value,
                r#type,
                span,
            } => Expr::Annotation {
                value: Box::new(expr(*value, type_mode)),
                r#type: Box::new(expr(*r#type, true)),
                span,
            },
            Expr::Arrow {
                parameter,
                r#return,
                span,
            } => Expr::Arrow {
                parameter: Box::new(expr(*parameter, type_mode)),
                r#return: Box::new(expr(*r#return, type_mode)),
                span,
            },
            Expr::Tuple(elements, span) if type_mode => {
                Expr::TypeTuple(elements.into_iter().map(|e| expr(e, true)).collect(), span)
            }
            Expr::Tuple(elements, span) => {
                Expr::Tuple(elements.into_iter().map(|e| expr(e, false)).collect(), span)
            }
            Expr::TypeTuple(elements, span) => Expr::TypeTuple(
                elements.into_iter().map(|e| expr(e, type_mode)).collect(),
                span,
            ),
            Expr::StructType(fields, span) => {
                Expr::StructType(fields.into_iter().map(|e| expr(e, true)).collect(), span)
            }
            Expr::StructInst {
                callee,
                fields,
                span,
            } => Expr::StructInst {
                callee: Box::new(expr(*callee, type_mode)),
                fields: fields.into_iter().map(|e| expr(e, type_mode)).collect(),
                span,
            },
            Expr::Array(elements, span) => Expr::Array(
                elements.into_iter().map(|e| expr(e, type_mode)).collect(),
                span,
            ),
            Expr::Table(entries, span) => Expr::Table(
                entries
                    .into_iter()
                    .map(|(key, value)| (expr(key, false), expr(value, false)))
                    .collect(),
                span,
            ),
            Expr::Shallow(inner, depth, span) => {
                Expr::Shallow(Box::new(expr(*inner, type_mode)), depth, span)
            }
            Expr::TypeArray {
                element_type,
                length,
                span,
            } => Expr::TypeArray {
                element_type: Box::new(expr(*element_type, type_mode)),
                length: Box::new(expr(*length, type_mode)),
                span,
            },
            Expr::Block {
                statements,
                expr: inner,
                span,
            } => Expr::Block {
                statements: statements.into_iter().map(|s| stmt(s, type_mode)).collect(),
                expr: Box::new(expr(*inner, type_mode)),
                span,
            },
            // Int, TypeConst, Placeholder, Err — mode-insensitive.
            e => e,
        }
    }
    fn stmt(s: Stmt, type_mode: bool) -> Stmt {
        match s {
            Stmt::Binding(binding) => Stmt::Binding(Binding {
                value: expr(binding.value, type_mode),
                ..binding
            }),
            Stmt::Expr(e) => Stmt::Expr(expr(e, type_mode)),
        }
    }
    Program {
        statements: program
            .statements
            .into_iter()
            .map(|s| stmt(s, false))
            .collect(),
        expr: expr(program.expr, false),
    }
}

// ---------------------------------------------------------------------------
// Diagnostics

/// Convert a chumsky error (token-index span, found token, expected labels)
/// into the crate's diagnostic.
fn diag_from(tokens: &[Token], e: &Rich<'_, Token, SimpleSpan<usize>>) -> Diag {
    let span = span_at(tokens, e.span().start);
    let message = match e.reason() {
        // A custom error (from the parser's own checks, e.g. a recovered
        // binding value) carries its message directly.
        RichReason::Custom(message) => message.to_string(),
        RichReason::ExpectedFound { .. } => {
            let found = e
                .found()
                .map(|t| t.kind.describe())
                .unwrap_or_else(|| "the end of the program".to_string());
            let expected: Vec<String> = e.expected().map(|p| p.to_string()).collect();
            match expected.as_slice() {
                [] => format!("unexpected {found}"),
                [one] => format!("expected {one}, found {found}"),
                [a, b] => format!("expected {a} or {b}, found {found}"),
                _ => format!(
                    "expected {}, or {}, found {found}",
                    expected[..expected.len() - 1].join(", "),
                    expected[expected.len() - 1],
                ),
            }
        }
    };
    Diag::new(Stage::Parse, span, message)
}

#[cfg(test)]
mod tests {
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
        assert_eq!(err.message, "expected an expression, found ';'");
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
        assert!(matches!(*r#type, Expr::Arrow { .. }));
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
        assert!(matches!(*r#type, Expr::TypeTuple(..)));
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
        assert!(matches!(*r#type, Expr::TypeTuple(..)));
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
        assert!(matches!(fields[0], Expr::TypeConst(TypeConst::Int, _)));
        assert!(matches!(fields[1], Expr::Arrow { .. }));
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
        assert!(matches!(fields[0], Expr::TypeTuple(..)));
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
        assert!(matches!(fields[0], Expr::Int(1, _)));
        assert!(matches!(fields[1], Expr::TypeConst(TypeConst::Int, _)));
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
        // a `<` right after an expression is always the array type, never an
        // application — no whitespace rule.
        let Expr::TypeArray { element_type, .. } = parse_ok("f <3>") else {
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
        // No whitespace rule: `a [0]` is still an index.
        assert!(matches!(parse_ok("a [0]"), Expr::Index { .. }));
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
        assert_eq!(err.message, "expected '~' or an expression, found ']'");
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
            "expected an expression, found the end of the program"
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
        assert!(matches!(*r#type, Expr::Placeholder(_)));
        // Nested: `x : Int -> _` — the arrow's return is a placeholder.
        let Expr::Annotation { r#type, .. } = parse_ok("x : Int -> _") else {
            panic!("expected an annotation")
        };
        let Expr::Arrow { r#return, .. } = *r#type else {
            panic!("expected an arrow type")
        };
        assert!(matches!(*r#return, Expr::Placeholder(_)));
        // In term position `_` stays an ordinary name.
        assert!(matches!(parse_ok("_"), Expr::Name(name, _) if name == "_"));
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
        // A block binding requires `;` — a binding followed by `}` leaves no
        // final expression.
        let err = parse_err("{a = 1}");
        assert!(err.message.contains("must end with an expression"));
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
        assert_eq!(errors[0].message, "expected an expression, found ';'");
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
                    if matches!(**right, Expr::Err(_) | Expr::Placeholder(_))
            ) || matches!(
                &binding.value,
                Expr::Arrow { r#return, .. }
                    if matches!(**r#return, Expr::Err(_) | Expr::Placeholder(_))
            ) || matches!(
                &binding.value,
                Expr::Annotation { r#type, .. }
                    if matches!(**r#type, Expr::Err(_) | Expr::Placeholder(_))
            ) || matches!(
                &binding.value,
                Expr::Lambda { r#return, .. }
                    if matches!(**r#return, Expr::Err(_) | Expr::Placeholder(_))
            );
            assert!(recovered, "{source}: recovered shape: {:?}", binding.value);
        }
    }
}
