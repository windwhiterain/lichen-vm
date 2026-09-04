//! The parser: tokens → AST, with error recovery.
//!
//! A program is `name = expr; …` bindings and bare expressions followed by a
//! final expression (see [`Program`]) — the same statement list, wrapped in
//! `{ … }`, forms a block expression (see [`Expr::Block`]).  A binding
//! without `let` is *block-wide*: its name is in scope throughout the block,
//! so it may recurse with itself and with the block's other bindings.  A
//! `let` before a binding (`let a = …`) is *restrictive*: the name is in
//! scope only in later statements, never in its own value.  Statements are
//! separated by `;`, `,`, or a newline (the lexer lexes all as `Separator`), and
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
//! the array type.  Postfix forms are marked by a `Glue` token (the lexer
//! emits it when the delimiter is directly glued to the previous token): a
//! glued `[` is an index `e[i]`, a glued `<` an array type.  A spaced `[` is
//! a fresh array-literal atom (so `f ([1, 2])` applies `f` to the array) and
//! a spaced `<` a tuple-type atom.  Precedence (loosest → tightest): `=>` (right) → `:`
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
//! checks ([`Expr::Err`] lowers to a masked [`ExprKind::ErrorBlock`] the
//! checker skips).  `parse`
//! therefore produces a program (possibly with error nodes) for almost any
//! input; only an input with no parseable statement at all fails outright.

use chumsky::error::RichReason;
use chumsky::input::Stream;
use chumsky::prelude::*;

use lichen_highlevel::ir::Span;

use crate::ast::{
    BinOp, Binding, ErrorBlock, Expr, Program, Stmt, StructField, TypeConst,
};
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
    let mut program = match output {
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
            // The whole (unparseable) stream is one masked error block.
            let range = (
                tokens.first().map(|t| t.range.0).unwrap_or(0),
                tokens.last().map(|t| t.range.1).unwrap_or(0),
            );
            Program {
                statements: Vec::new(),
                expr: Expr::Err { range, start: span },
                error_blocks: Vec::new(),
                stmt_ranges: Vec::new(),
            }
        }
    };
    // Surface the recovered error regions on the program, in source order, so
    // the frontend can mask them out of a content signature / diff.
    program.error_blocks = collect_error_blocks(&program);
    (program, errors)
}

/// Re-parse a contiguous statement *window* `tokens[start..end]` into its
/// statements, for incremental splicing.
///
/// The window must begin at a token-before-a-statement (it may open with
/// leading separators and close with trailing ones, both dropped) — the caller
/// chooses it to cover exactly the statements that a user edit touched.  The
/// statements are produced with the [`apply_type_mode`] post-pass applied (a
/// top-level statement is in term mode).  Like [`parse`], the window carries
/// recovered errors rather than failing.
///
/// This is the incremental-parse primitive: the tokens it consumes already
/// carry *absolute* byte ranges and (line, col) spans (the lexer emits them),
/// so the resulting statements are directly spliceable into a program without
/// any position re-mapping.  It reuses the recovery behavior of the whole
/// statement list, so a statement in the window recovers in exactly the way it
/// would when parsed as part of a full program.
pub fn parse_statement_region(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> (Vec<Stmt>, Vec<Diag>) {
    let (statements, _ranges, errors) = parse_statement_region_traced(tokens, start, end);
    (statements, errors)
}

/// Re-parse a contiguous statement *window* `tokens[start..end]` into its
/// statements **and** the token-index range each covered, for incremental
/// splicing.
///
/// This is [`parse_statement_region`] plus the ranges: the second element is
/// one `(start, end)` per returned statement, absolute token indices into
/// `tokens` (the region's spans are offset by `start` back into the whole
/// stream).  The ranges let the session splice the window into a program and
/// keep [`Program::stmt_ranges`] correct without re-parsing the untouched
/// prefix and suffix.
pub fn parse_statement_region_traced(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> (Vec<Stmt>, Vec<(usize, usize)>, Vec<Diag>) {
    let (statements, ranges, errors) = std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn_scoped(scope, || region_inner(tokens, start, end))
            .expect("spawn the region parse worker");
        worker.join().expect("the region parse worker panicked")
    });
    let errors = errors
        .into_iter()
        .map(|(span, message, stage)| Diag {
            span,
            message,
            stage,
            check: None,
        })
        .collect();
    (statements, ranges, errors)
}

/// The worker's result for a region: the statements, their absolute
/// token-index ranges, and the diagnostics in a `Send` form (the crate's `Diag`
/// embeds the checker's, which is not `Send`).
type RegionOut = (
    Vec<Stmt>,
    Vec<(usize, usize)>,
    Vec<(Option<Span>, String, Stage)>,
);

fn region_inner(tokens: &[Token], start: usize, end: usize) -> RegionOut {
    let region = &tokens[start..end];
    let expr = expression(region);
    // `seps elem (seps elem)* seps` — like the statement list, but without the
    // final-expr pop.  Leading/trailing separators are consumed and dropped.
    let seps = token(TokenKind::Separator).ignored().repeated().collect::<Vec<_>>();
    let seps1 = token(TokenKind::Separator)
        .ignored()
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>();
    let elem = statement(region, expr.clone())
        .recover_with(skip_then_retry_until(
            any::<In<'_>, E<'_>>().ignored(),
            choice((
                token(TokenKind::Eof).ignored(),
                token(TokenKind::RBrace).ignored(),
            )),
        ))
        .map_with(|s, me| (s, (me.span().start, me.span().end)));
    let parser = seps
        .clone()
        .then(elem.clone())
        .then(
            (seps1.clone().then(elem.clone()))
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(seps)
        .map(|(((_, first), rest), _)| {
            std::iter::once(first)
                .chain(rest.into_iter().map(|(_, e)| e))
                .collect::<Vec<(Stmt, (usize, usize))>>()
        });
    let stream = Stream::from_iter(region.iter().cloned());
    let (output, errs) = parser.parse(stream).into_output_errors();
    let mut errors: Vec<(Option<Span>, String, Stage)> = Vec::new();
    for e in &errs {
        let diag = diag_from(region, e);
        if !errors
            .iter()
            .any(|(span, message, _)| *span == diag.span && *message == diag.message)
        {
            errors.push((diag.span, diag.message, diag.stage));
        }
    }
    // The region's tokens carry absolute *byte* positions, so the statements are
    // spliceable as-is; only the *token-index* spans are region-relative, so
    // offset them back by `start` into the whole stream.
    let elems: Vec<(Stmt, (usize, usize))> = output.unwrap_or_default();
    let statements: Vec<Stmt> = elems.iter().map(|(s, _)| s.clone()).collect();
    let ranges: Vec<(usize, usize)> = elems
        .iter()
        .map(|(_, r)| (r.0 + start, r.1 + start))
        .collect();
    // Apply the type-mode post-pass (top-level statements are in term mode),
    // reusing the whole-program pass on a synthetic program.
    let program = Program {
        statements,
        expr: Expr::Int(0, (1, 1)),
        error_blocks: Vec::new(),
        stmt_ranges: ranges.clone(),
    };
    let program = apply_type_mode(program);
    (program.statements, ranges, errors)
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

/// The byte offset at token-index `i`, or the end of the source when `i` is
/// at or past the end (the lexer's Eof token sits there).
fn token_byte(tokens: &[Token], index: usize) -> u32 {
    tokens
        .get(index)
        .map(|t| t.range.0)
        .or_else(|| tokens.last().map(|t| t.range.1))
        .unwrap_or(0)
}

/// The byte range a token-index span `[start, end)` covers — the region the
/// recovered construct's fallback consumed.  A zero-width range (the missing
/// operand at `start`) or a past-the-end boundary collapses to a point.
fn byte_range(tokens: &[Token], span: SimpleSpan<usize>) -> (u32, u32) {
    let start = token_byte(tokens, span.start);
    if span.end > span.start {
        match tokens.get(span.end - 1) {
            Some(last) => (start, last.range.1),
            None => (start, start),
        }
    } else {
        (start, start)
    }
}

/// Build a recovered-error AST node from the token-index span a recovery
/// produced: the byte `range` the fallback covered (the mask), plus the
/// (line, col) where the broken construct began.
fn err_node(tokens: &[Token], span: SimpleSpan<usize>) -> Expr {
    Expr::Err {
        range: byte_range(tokens, span),
        start: span_at(tokens, span.start),
    }
}

/// The byte offset of the first token at the given (line, col) span — the
/// position a known-`Span` error node should mask (a zero-width point).
fn byte_at_span(tokens: &[Token], span: Span) -> u32 {
    tokens
        .iter()
        .find(|t| t.span == span)
        .map(|t| t.range.0)
        .or_else(|| tokens.last().map(|t| t.range.1))
        .unwrap_or(0)
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
        .map(|(statements, expr, stmt_ranges)| Program {
            statements,
            expr,
            error_blocks: Vec::new(),
            stmt_ranges,
        })
}

/// The statement list: `elem (seps elem)*`, followed by trailing separators
/// — every statement after the first must be preceded by a separator, and
/// only the last statement is the list's value.  The last element is the
/// final expression; a trailing binding (or an empty list) is an error, and
/// the list's value becomes an error node.
///
/// Also yields the **token-index range** each logical statement covers, in
/// source order — one per returned statement plus one for the final expression.
fn statement_list<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, (Vec<Stmt>, Expr, Vec<(usize, usize)>), E<'a>> + Clone {
    let seps = token(TokenKind::Separator)
        .ignored()
        .repeated()
        .collect::<Vec<_>>();
    let seps1 = token(TokenKind::Separator)
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
    let elem = statement(tokens, expr)
        .recover_with(skip_then_retry_until(
            any::<In<'a>, E<'a>>().ignored(),
            choice((
                token(TokenKind::Eof).ignored(),
                token(TokenKind::RBrace).ignored(),
            )),
        ))
        .map_with(|s, me| (s, (me.span().start, me.span().end)));

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
                .collect::<Vec<(Stmt, (usize, usize))>>()
        })
        .validate(|mut elems, me, emit| match elems.pop() {
            Some((Stmt::Expr(final_expr), last_range)) => {
                let mut ranges: Vec<(usize, usize)> = elems.iter().map(|(_, r)| *r).collect();
                ranges.push(last_range);
                (elems.into_iter().map(|(s, _)| s).collect(), final_expr, ranges)
            }
            Some((Stmt::Binding(binding), last_range)) => {
                emit.emit(Rich::custom(
                    me.span(),
                    "a program must end with an expression",
                ));
                let span = binding.span;
                let mut ranges: Vec<(usize, usize)> = elems.iter().map(|(_, r)| *r).collect();
                ranges.push(last_range);
                let mut stmts: Vec<Stmt> = elems.into_iter().map(|(s, _)| s).collect();
                stmts.push(Stmt::Binding(binding));
                // The trailing-binding error is a masked point at the
                // binding's own position (a zero-width mask, stable across
                // edits that grow an earlier region).
                let pos = byte_at_span(tokens, span);
                (stmts, Expr::Err { range: (pos, pos), start: span }, ranges)
            }
            None => {
                emit.emit(Rich::custom(
                    me.span(),
                    "a program must end with an expression",
                ));
                (
                    vec![],
                    err_node(tokens, me.span()),
                    vec![(me.span().start, me.span().end)],
                )
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
            .filter(|t: &Token| t.kind != TokenKind::Separator)
            .ignored()
            .repeated()
            .map_with(move |_, me| err_node(tokens, me.span())),
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
        empty::<In<'a>, E<'a>>().map_with(move |_, me| err_node(tokens, me.span())),
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
        // `! e` — a prefix assert.  It binds tighter than the binary
        // operators but looser than application: `! f x` asserts `f x`,
        // `! (x <= 3)` the comparison.  Asserting a comparison under the
        // binary operators requires parens — `! x <= 3` is `(!x) <= 3`.
        let unary = token(TokenKind::Bang)
            .ignore_then(application.clone())
            .map_with(|e, me| Expr::Assert {
                value: Box::new(e),
                span: span_at(tokens, me.span().start),
            })
            .or(application.clone());
        let term1 = unary.clone().foldl(
            arith.then(operand(tokens, unary.clone())).repeated(),
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

        // `:` (the type annotation) and `#` (the perspective annotation) at the
        // same precedence, right-associative.  Either may appear alone; the two
        // fold into a single `Annotation` carrying whichever are present (at
        // most one of each).  Both right sides are parsed at the `->` level:
        // `e : Int -> Int` annotates with the arrow type, `e # n` with `n`.
        let term4 = term3
            .clone()
            .then(
                choice((
                    token(TokenKind::Colon)
                        .ignore_then(operand(tokens, term3.clone()))
                        .map(AnnPiece::Type),
                    token(TokenKind::Hash)
                        .ignore_then(operand(tokens, term3.clone()))
                        .map(AnnPiece::Perspective),
                ))
                .repeated()
                .collect::<Vec<_>>(),
            )
            .map(|(first, rest)| fold_annotations(first, rest));

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
                        parameter_perspective: None,
                        r#return: rhs,
                        span,
                    },
                    Expr::Annotation {
                        value,
                        r#type,
                        perspective,
                        span,
                    } => match *value {
                        Expr::Name(parameter, parameter_span) => Expr::Lambda {
                            parameter,
                            parameter_span,
                            parameter_type: r#type,
                            parameter_perspective: perspective,
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

/// One `: T` or `# p` partner of an annotation chain.
#[derive(Clone, Debug)]
enum AnnPiece {
    Type(Expr),
    Perspective(Expr),
}

/// Accumulate a `: T` / `# p` chain into one [`Expr::Annotation`], carrying
/// whichever of the two annotations are present (at most one of each — a
/// later one of the same kind overwrites, matching the `expr [: expr] [# expr]`
/// grammar).  `e : A : B` keeps `B` (rightmost wins), as before.  An
/// expression with no annotation (`rest` empty) is returned unchanged — it is
/// not wrapped in a no-op `Annotation`, so the grammar stays faithful.
fn fold_annotations(first: Expr, rest: Vec<AnnPiece>) -> Expr {
    if rest.is_empty() {
        return first;
    }
    let span = first.span();
    let value = Box::new(first);
    let mut r#type = None;
    let mut perspective = None;
    for piece in rest {
        match piece {
            AnnPiece::Type(t) => r#type = Some(Box::new(t)),
            AnnPiece::Perspective(p) => perspective = Some(Box::new(p)),
        }
    }
    Expr::Annotation {
        value,
        r#type,
        perspective,
        span,
    }
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
    let primary = token(TokenKind::Glue)
        .ignored()
        .or_not()
        .ignore_then(choice((
            any::<In<'a>, E<'a>>()
                .filter(|t: &Token| matches!(t.kind, TokenKind::Int(_)))
                .map(|t| match t.kind {
                    TokenKind::Int(n) => Expr::Int(n, t.span),
                    _ => unreachable!("filtered for an int"),
                }),
            any::<In<'a>, E<'a>>()
                .filter(|t: &Token| matches!(t.kind, TokenKind::Str(_)))
                .map(|t| match t.kind {
                    TokenKind::Str(s) => Expr::Str(s, t.span),
                    _ => unreachable!("filtered for a string"),
                }),
            token(TokenKind::KwInt).map(|t| Expr::TypeConst(TypeConst::Int, t.span)),
            token(TokenKind::KwString).map(|t| Expr::TypeConst(TypeConst::String, t.span)),
            token(TokenKind::KwType).map(|t| Expr::TypeConst(TypeConst::Type, t.span)),
            name().map(|(n, span)| Expr::Name(n, span)),
            native_call(tokens, expr.clone()),
            paren(tokens, expr.clone()),
            array_literal(tokens, expr.clone()),
            table_literal(tokens, expr.clone()),
            block(tokens, expr.clone()),
            angle_tuple(tokens, expr.clone()),
            struct_type(tokens, expr.clone()),
            if_expr(tokens, expr.clone()),
        )))
        .labelled("an expression");

    // The postfix forms, chained left.  A `[` after an expression is always
    // an index, a `<` always an array type.  A `(` or a `{` is postfix only
    // when *adjacent* — the bracket comes straight after the expression, no
    // space between: a `(` holding a single comma-free expression is the
    // positional slot read `a(k)`, any other adjacent `(` is struct
    // instantiation, an adjacent `{` is a table lookup.  A spaced `(` is a
    // paren atom and a spaced `{` a block; the application rule treats
    // either as an argument, never this postfix.
    let glue = token(TokenKind::Glue).ignored();
    let postfix = choice((
        token(TokenKind::Dot)
            .ignore_then(name())
            .map(|(field_name, _)| Postfix::DotName(field_name)),
        glue.clone()
            .ignore_then(token(TokenKind::LBracket))
            .ignore_then(expr.clone())
            .then_ignore(token(TokenKind::RBracket))
            .map(Postfix::Index),
        glue.clone()
            .ignore_then(token(TokenKind::LAngle))
            .ignore_then(expr.clone())
            .then_ignore(token(TokenKind::RAngle))
            .map(Postfix::TypeArray),
        glue.clone()
            .ignore_then(token(TokenKind::LBrace))
            .ignore_then(expr.clone())
            .then_ignore(token(TokenKind::RBrace))
            .map(Postfix::TableFind),
        glue.clone()
            .ignore_then(token(TokenKind::LParen))
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
                    Postfix::DotName(name) => Expr::NamedFieldRead {
                        container: Box::new(acc),
                        name,
                        span,
                    },
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
    DotName(String),
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
            token(TokenKind::Separator)
                .ignore_then(expr.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(token(TokenKind::Separator).or_not())
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
            token(TokenKind::Separator)
                .ignore_then(expr.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(token(TokenKind::Separator).or_not())
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

/// `$name(args…)` — a native-operator call.  A plugin's embedded source uses
/// this to call one of its registered native operators; the name resolves only
/// against the compiling module's private registry.  The arg list is the
/// comma-separated expr list of a parenthesized form (adjacent parens — a
/// `$` native call never uses a spaced paren).
fn native_call<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::Dollar)
        .ignore_then(name())
        .then(token(TokenKind::Glue).or_not())
        .then_ignore(token(TokenKind::LParen))
        .then(paren_fields(expr.clone()))
        .then_ignore(token(TokenKind::RParen))
        .map_with(|(((op, _), _), (args, _)), me| Expr::NativeCall {
            op,
            args,
            span: span_at(tokens, me.span().start),
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
            token(TokenKind::Separator)
                .ignore_then(element.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(token(TokenKind::Separator).or_not())
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
                // its value is a masked error block.
                (key, err_node(tokens, me.span()))
            }
        });
    token(TokenKind::KwTable)
        .ignore_then(token(TokenKind::Glue).ignored().or_not())
        .ignore_then(token(TokenKind::LBrace))
        .ignore_then(entry.clone().or_not())
        .then(
            token(TokenKind::Separator)
                .ignore_then(entry.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(token(TokenKind::Separator).or_not())
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
            token(TokenKind::Separator)
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

/// One `struct<…>` field: a `.name` prefix followed by the field's type
/// expression, or a bare type expression.  The leading `.` is the
/// language-server-friendly discriminator — it unambiguously marks a named
/// field, so a field name can never be confused with a field type while the
/// user is typing.  A bare type is an unnamed (positional) field; a
/// `.name Type` is a named field.
fn struct_field<'a>(
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, StructField, E<'a>> + Clone {
    let named = token(TokenKind::Dot)
        .ignore_then(name())
        .then(expr.clone())
        .map(|((n, _span), ty)| StructField {
            name: Some(n),
            ty,
        });
    let unnamed = expr.map(|ty| StructField { name: None, ty });
    choice((named, unnamed))
}

/// `struct<T1, …, Tn>` — a nominal struct type.  Each field may carry an
/// optional name (`.name type`); a bare field is positional.  At least one
/// field.
fn struct_type<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    let field = struct_field(expr.clone());
    token(TokenKind::KwStruct)
        .ignore_then(token(TokenKind::Glue).ignored().or_not())
        .ignore_then(token(TokenKind::LAngle))
        .ignore_then(field.clone())
        .then(
            token(TokenKind::Separator)
                .ignore_then(field)
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
        .map_with(|(statements, expr, _ranges), me| Expr::Block {
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
                parameter_perspective,
                r#return,
                span,
            } => Expr::Lambda {
                parameter,
                parameter_span,
                parameter_type: parameter_type.map(|t| Box::new(expr(*t, true))),
                parameter_perspective: parameter_perspective.map(|p| Box::new(expr(*p, false))),
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
            Expr::Assert { value, span } => Expr::Assert {
                value: Box::new(expr(*value, type_mode)),
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
            Expr::NamedFieldRead {
                container,
                name,
                span,
            } => Expr::NamedFieldRead {
                container: Box::new(expr(*container, type_mode)),
                name,
                span,
            },
            Expr::Annotation {
                value,
                r#type,
                perspective,
                span,
            } => Expr::Annotation {
                value: Box::new(expr(*value, type_mode)),
                r#type: r#type.map(|t| Box::new(expr(*t, true))),
                perspective: perspective.map(|p| Box::new(expr(*p, false))),
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
            Expr::StructType(fields, span) => Expr::StructType(
                fields
                    .into_iter()
                    .map(|field| StructField {
                        name: field.name,
                        ty: expr(field.ty, true),
                    })
                    .collect(),
                span,
            ),
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
        error_blocks: program.error_blocks,
        stmt_ranges: program.stmt_ranges,
    }
}

/// Collect the byte-range masks of every recovered-error node in the AST, in
/// source order.  [`Program::error_blocks`] carries these so the frontend can
/// exclude the error regions from a content signature / diff.
pub(crate) fn collect_error_blocks(program: &Program) -> Vec<ErrorBlock> {
    fn walk_expr(e: &Expr, out: &mut Vec<ErrorBlock>) {
        match e {
            Expr::Err { range, start } => out.push(ErrorBlock { range: *range, start: *start }),
            Expr::Int(..) | Expr::Str(..) | Expr::TypeConst(..) | Expr::Name(..) | Expr::Placeholder(..) => {}
            Expr::Lambda {
                parameter_type,
                parameter_perspective,
                r#return,
                ..
            } => {
                if let Some(t) = parameter_type {
                    walk_expr(t, out);
                }
                if let Some(p) = parameter_perspective {
                    walk_expr(p, out);
                }
                walk_expr(r#return, out);
            }
            Expr::Apply {
                function,
                argument,
                ..
            } => {
                walk_expr(function, out);
                walk_expr(argument, out);
            }
            Expr::BinOp { left, right, .. } => {
                walk_expr(left, out);
                walk_expr(right, out);
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                walk_expr(condition, out);
                walk_expr(then_branch, out);
                walk_expr(else_branch, out);
            }
            Expr::Assert { value, .. } => walk_expr(value, out),
            Expr::NativeCall { args, .. } => {
                for a in args {
                    walk_expr(a, out);
                }
            }
            Expr::Index { array, index, .. } => {
                walk_expr(array, out);
                walk_expr(index, out);
            }
            Expr::FieldRead { container, key, .. } => {
                walk_expr(container, out);
                walk_expr(key, out);
            }
            Expr::NamedFieldRead { container, .. } => {
                walk_expr(container, out);
            }
            Expr::TableFind { container, key, .. } => {
                walk_expr(container, out);
                walk_expr(key, out);
            }
            Expr::Annotation {
                value,
                r#type,
                perspective,
                ..
            } => {
                walk_expr(value, out);
                if let Some(t) = r#type {
                    walk_expr(t, out);
                }
                if let Some(p) = perspective {
                    walk_expr(p, out);
                }
            }
            Expr::Arrow {
                parameter,
                r#return,
                ..
            } => {
                walk_expr(parameter, out);
                walk_expr(r#return, out);
            }
            Expr::Tuple(elems, _) | Expr::TypeTuple(elems, _) | Expr::Array(elems, _) => {
                for el in elems {
                    walk_expr(el, out);
                }
            }
            Expr::StructType(fields, _) => {
                for field in fields {
                    walk_expr(&field.ty, out);
                }
            }
            Expr::StructInst { callee, fields, .. } => {
                walk_expr(callee, out);
                for f in fields {
                    walk_expr(f, out);
                }
            }
            Expr::Table(entries, _) => {
                for (k, v) in entries {
                    walk_expr(k, out);
                    walk_expr(v, out);
                }
            }
            Expr::Shallow(inner, _, _) => walk_expr(inner, out),
            Expr::TypeArray {
                element_type,
                length,
                ..
            } => {
                walk_expr(element_type, out);
                walk_expr(length, out);
            }
            Expr::Block { statements, expr, .. } => {
                for s in statements {
                    walk_stmt(s, out);
                }
                walk_expr(expr, out);
            }
        }
    }
    fn walk_stmt(s: &Stmt, out: &mut Vec<ErrorBlock>) {
        match s {
            Stmt::Binding(binding) => walk_expr(&binding.value, out),
            Stmt::Expr(e) => walk_expr(e, out),
        }
    }
    let mut out = Vec::new();
    for s in &program.statements {
        walk_stmt(s, &mut out);
    }
    walk_expr(&program.expr, &mut out);
    out
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
#[path = "tests/parse_tests.rs"]
mod tests;
