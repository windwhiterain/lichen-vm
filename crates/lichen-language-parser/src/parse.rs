//! The parser: tokens to AST, with error recovery.  Grammar:
//! `docs/language-spec.md` §2.
//!
//! # Invariant
//! `parse` produces a program for almost any input; only an input with no
//! parseable statement at all fails outright.

use chumsky::input::Stream;
use chumsky::prelude::*;
use std::collections::HashSet;

use lichen_language_lex::{Span, Token, TokenKind};

#[path = "parse/diagnostics.rs"]
mod diagnostics;
#[path = "parse/error_blocks.rs"]
mod error_blocks;

use diagnostics::diag_from;

pub use error_blocks::collect_error_blocks;

use crate::ast::{
    BinOp, Binding, BlockStmt, ConvOp, Expr, Program, RecordField, Stmt, StructField,
    StructInstArg, TypeConst,
};

/// A parse diagnostic: a message plus its source position; widened to the
/// language crate's `Diag` at `Stage::Parse`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseDiag {
    pub span: Option<Span>,
    pub message: String,
}

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
    pub errors: Vec<ParseDiag>,
}

/// The stack the parse runs on: the combinator grammar recurses too deep for
/// the caller's stack ([`ParseWorker`]).
const WORKER_STACK: usize = 16 * 1024 * 1024;

/// The process-lived worker every parse runs on: a job sent to it, run on its
/// warm stack, answered on a channel.
///
/// # Invariant
/// One worker serialises parses, and a job owns its input — a job is `'static`,
/// so no borrow of the caller's token slice crosses the channel
/// (`docs/notes/code-audit.md` D13).
struct ParseWorker {
    jobs: std::sync::Mutex<std::sync::mpsc::Sender<Job>>,
}

/// One unit of work for the worker: a closure that runs on the worker's stack.
type Job = Box<dyn FnOnce() + Send>;

impl ParseWorker {
    fn global() -> &'static ParseWorker {
        static WORKER: std::sync::OnceLock<ParseWorker> = std::sync::OnceLock::new();
        WORKER.get_or_init(|| {
            let (jobs, receiver) = std::sync::mpsc::channel::<Job>();
            std::thread::Builder::new()
                .stack_size(WORKER_STACK)
                .spawn(move || {
                    // A send error means every caller's receiver is gone, which
                    // means the process is shutting down: nothing left to serve.
                    while let Ok(job) = receiver.recv() {
                        job();
                    }
                })
                .expect("spawn the parse worker");
            ParseWorker {
                jobs: std::sync::Mutex::new(jobs),
            }
        })
    }

    /// Run `f` on the worker and block until it answers.
    ///
    /// # Invariant
    /// A panic is caught on the worker and resumed on the caller, so the worker
    /// survives a panicking parse.
    fn run<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        let job: Job = Box::new(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            let _ = tx.send(outcome);
        });
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send(job)
            .expect("the parse worker is gone");
        match rx.recv() {
            Ok(Ok(value)) => value,
            Ok(Err(payload)) => std::panic::resume_unwind(payload),
            Err(_) => panic!("the parse worker died before it answered"),
        }
    }
}

/// Parse a token stream on the process-lived [`ParseWorker`].
pub fn parse(tokens: &[Token]) -> Parsed {
    // Owned, because the worker is `'static`: see `ParseWorker` for why the
    // copy is the accepted half of `D13`.
    let owned: Vec<Token> = tokens.to_vec();
    let (program, errors) = ParseWorker::global().run(move || parse_inner(&owned));
    Parsed { program, errors }
}

/// The worker's result: the program plus its diagnostics.
type ParseOut = (Program, Vec<ParseDiag>);

fn parse_inner(tokens: &[Token]) -> ParseOut {
    let stream = Stream::from_iter(tokens.iter().cloned());
    let parser = program_parser(tokens);
    let (output, errs) = parser.parse(stream).into_output_errors();
    let mut errors: Vec<ParseDiag> = Vec::new();
    // Dedup by whole diagnostic content: chumsky emits the same rich error more
    // than once.
    let mut seen: HashSet<(Option<Span>, String)> = HashSet::new();
    for e in &errs {
        let diag = diag_from(tokens, e);
        if seen.insert((diag.span, diag.message.clone())) {
            errors.push(diag);
        }
    }
    let mut program = match output {
        Some(program) => program,
        None => {
            let span = errors
                .first()
                .and_then(|d| d.span)
                .unwrap_or_else(|| span_at(tokens, tokens.len().saturating_sub(1)));
            errors.push(ParseDiag {
                span: Some(span),
                message: "the program could not be parsed".to_string(),
            });
            // The whole (unparseable) stream is one masked error block.
            let range = (
                tokens.first().map(|t| t.range.0).unwrap_or(0),
                tokens.last().map(|t| t.range.1).unwrap_or(0),
            );
            Program {
                statements: Vec::new(),
                expr: Some(Expr::Err { range, start: span }),
                error_blocks: Vec::new(),
                stmt_ranges: Vec::new(),
            }
        }
    };
    // Surface the recovered error regions on the program, in source order.
    program.error_blocks = collect_error_blocks(&program);
    (program, errors)
}

/// Re-parse the statement window `tokens[start..end]` into its statements, for
/// incremental splicing.
pub fn parse_statement_region(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> (Vec<BlockStmt>, Vec<ParseDiag>) {
    let (statements, _ranges, errors) = parse_statement_region_traced(tokens, start, end);
    (statements, errors)
}

/// As [`parse_statement_region`], plus each statement's absolute token-index
/// range.
pub fn parse_statement_region_traced(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> (Vec<BlockStmt>, Vec<(usize, usize)>, Vec<ParseDiag>) {
    // Owned for the same reason [`parse`] copies: the worker outlives the call.
    let owned: Vec<Token> = tokens.to_vec();
    ParseWorker::global().run(move || region_inner(&owned, start, end))
}

/// The worker's result for a region: the statements, their absolute
/// token-index ranges, and the diagnostics.
type RegionOut = (Vec<BlockStmt>, Vec<(usize, usize)>, Vec<ParseDiag>);

fn region_inner(tokens: &[Token], start: usize, end: usize) -> RegionOut {
    let region = &tokens[start..end];
    let expr = expression(region);
    // `seps elem (seps elem)* seps` — the statement list without the tail pop.
    let seps = separator_run();
    let seps1 = separators_between();
    let elem = block_statement(region, expr.clone())
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
                .collect::<Vec<(BlockStmt, (usize, usize))>>()
        });
    let stream = Stream::from_iter(region.iter().cloned());
    let (output, errs) = parser.parse(stream).into_output_errors();
    let mut errors: Vec<ParseDiag> = Vec::new();
    // The same content-keyed dedup as [`parse_inner`]'s.
    let mut seen: HashSet<(Option<Span>, String)> = HashSet::new();
    for e in &errs {
        let diag = diag_from(region, e);
        if seen.insert((diag.span, diag.message.clone())) {
            errors.push(diag);
        }
    }
    // Byte positions are absolute; the token-index ranges are region-relative,
    // so offset them by `start`.
    let elems: Vec<(BlockStmt, (usize, usize))> = output.unwrap_or_default();
    let statements: Vec<BlockStmt> = elems.iter().map(|(s, _)| s.clone()).collect();
    let ranges: Vec<(usize, usize)> = elems
        .iter()
        .map(|(_, r)| (r.0 + start, r.1 + start))
        .collect();
    (statements, ranges, errors)
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

/// A **run** of separators, any quantity including none — the one rule the
/// statement level and every list form share.
///
/// # Invariant
/// `repeated` rewinds, so a run that is not there leaves the enclosing closer
/// or the end of input to match at the same position.
fn separator_run<'a>() -> impl Parser<'a, In<'a>, Vec<()>, E<'a>> + Clone {
    token(TokenKind::Separator)
        .ignored()
        .repeated()
        .collect::<Vec<_>>()
}

/// A run of separators of at least one — the separator between two items.
fn separators_between<'a>() -> impl Parser<'a, In<'a>, Vec<()>, E<'a>> + Clone {
    token(TokenKind::Separator)
        .ignored()
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>()
}

/// A name token; `_` lexes as its own [`TokenKind::Placeholder`] token.
fn name<'a>() -> impl Parser<'a, In<'a>, (String, Span), E<'a>> + Clone {
    any::<In<'a>, E<'a>>()
        .filter(|t: &Token| matches!(t.kind, TokenKind::Name(_)))
        .map(|t| match t.kind {
            TokenKind::Name(n) => (n, t.span),
            _ => unreachable!("filtered for a name"),
        })
        .labelled("a name")
}

/// The `(line, col)` span of the token at `index`, or the last token's past
/// the end.
fn span_at(tokens: &[Token], index: usize) -> Span {
    tokens
        .get(index)
        .map(|t| t.span)
        .unwrap_or_else(|| tokens.last().map(|t| t.span).unwrap_or((1, 1)))
}

/// The byte offset at token-index `i`, or the end of the source past the end.
fn token_byte(tokens: &[Token], index: usize) -> u32 {
    tokens
        .get(index)
        .map(|t| t.range.0)
        .or_else(|| tokens.last().map(|t| t.range.1))
        .unwrap_or(0)
}

/// The byte range a token-index span covers; a zero-width or past-the-end span
/// collapses to a point.
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

/// Build a recovered-error node: the fallback's byte range plus where it began.
fn err_node(tokens: &[Token], span: SimpleSpan<usize>) -> Expr {
    Expr::Err {
        range: byte_range(tokens, span),
        start: span_at(tokens, span.start),
    }
}

/// The program: the block body (the top level is a block) then the end of the
/// input.
fn program_parser<'a>(tokens: &'a [Token]) -> impl Parser<'a, In<'a>, Program, E<'a>> {
    let expr = expression(tokens);
    // The lexer appends a real `Eof`, so `end()` would fail with it unconsumed.
    block_body(tokens, expr)
        .then_ignore(token(TokenKind::Eof).ignored())
        .map(|items| {
            let (statements, stmt_ranges, expr) = split_block_items(items);
            Program {
                statements,
                expr,
                error_blocks: Vec::new(),
                stmt_ranges,
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

/// `['@loop'] ['cache'] ['let'] name = expr` — a binding, with three
/// independent marks.
fn binding<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Binding, E<'a>> + Clone {
    let head = token(TokenKind::KwLoop)
        .ignored()
        .or_not()
        .then(
            token(TokenKind::KwCache)
                .ignored()
                .or_not()
                .then(choice((
                    token(TokenKind::KwLet)
                        .ignore_then(name())
                        .then_ignore(token(TokenKind::Equals))
                        .map(|n| (n, true)),
                    name()
                        .then_ignore(token(TokenKind::Equals))
                        .map(|n| (n, false)),
                )))
                .map(|(cached, (n, restrictive))| (n, restrictive, cached.is_some())),
        )
        .map(|(looping, (n, restrictive, cached))| (n, restrictive, cached, looping.is_some()));
    // A broken value skips to the next separator or the input's end (never Eof)
    // and substitutes an error node.
    let value = expr.recover_with(via_parser(
        any::<In<'a>, E<'a>>()
            .filter(|t: &Token| t.kind != TokenKind::Separator && t.kind != TokenKind::Eof)
            .ignored()
            .repeated()
            .map_with(move |_, me| err_node(tokens, me.span())),
    ));
    head.then(value).map(
        |(((name, span), restrictive, cached, looping), value)| Binding {
            name,
            span,
            binder: None,
            value,
            restrictive,
            cached,
            looping,
        },
    )
}

/// An operand: a full expression, or a non-consuming error node when the
/// operator is dangling at a statement boundary.
fn operand<'a>(
    tokens: &'a [Token],
    p: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    p.recover_with(via_parser(
        empty::<In<'a>, E<'a>>().map_with(move |_, me| err_node(tokens, me.span())),
    ))
}

/// One prefix conversion, `int2float e` / `float2int e`.
fn prefix_conv<'a>(
    tokens: &'a [Token],
    kind: TokenKind,
    operator: ConvOp,
    operand: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone + 'a,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(kind)
        .ignore_then(operand)
        .map_with(move |e, me| Expr::Convert {
            operator,
            value: Box::new(e),
            span: span_at(tokens, me.span().start),
        })
        .boxed()
}

/// The precedence chain over atoms, with `=>` as the loosest operator.
fn expression<'a>(tokens: &'a [Token]) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    recursive(|expr| {
        let atom = atom_parser(tokens, expr.clone());

        // Application: juxtaposition, left-associative, binds tighter than
        // every operator.
        let application = atom
            .clone()
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
            })
            .boxed();

        // `@assert e` — a prefix assert, tighter than every binary operator but
        // looser than application.
        let unary = token(TokenKind::KwAssert)
            .ignore_then(application.clone())
            .map_with(|e, me| Expr::Assert {
                value: Box::new(e),
                span: span_at(tokens, me.span().start),
            })
            // The two prefix conversions, at the assert's level: one operand, no
            // infix token, so no new rung.
            .or(choice((
                prefix_conv(
                    tokens,
                    TokenKind::KwInt2Float,
                    ConvOp::Int2Float,
                    application.clone(),
                ),
                prefix_conv(
                    tokens,
                    TokenKind::KwFloat2Int,
                    ConvOp::Float2Int,
                    application.clone(),
                ),
            )))
            .or(application.clone())
            .boxed();

        // Each level ends in `.boxed()` for build time, not behaviour: see
        // `docs/notes/build-performance.md`.

        // `*` / `/` / `%`, left-associative — the tightest binary level, so
        // `a + b * c` is `a + (b * c)`.
        let product_op = choice((
            token(TokenKind::Star).to(BinOp::Mul),
            token(TokenKind::Slash).to(BinOp::Div),
            token(TokenKind::Percent).to(BinOp::Rem),
        ));
        let product = unary
            .clone()
            .foldl(
                product_op.then(operand(tokens, unary.clone())).repeated(),
                |lhs, (op, rhs)| bin_op(lhs, op, rhs),
            )
            .boxed();

        // `+` / `-`, left-associative.
        let sum_op = choice((
            token(TokenKind::Plus).to(BinOp::Add),
            token(TokenKind::Minus).to(BinOp::Sub),
        ));
        let sum = product
            .clone()
            .foldl(
                sum_op.then(operand(tokens, product.clone())).repeated(),
                |lhs, (op, rhs)| bin_op(lhs, op, rhs),
            )
            .boxed();

        // `&`, `^`, `|` — three levels, tightest first; over `0`/`1` results
        // they are the language's `and`/`xor`/`or`.
        let bitand = sum
            .clone()
            .foldl(
                token(TokenKind::Amp)
                    .to(BinOp::BitAnd)
                    .then(operand(tokens, sum.clone()))
                    .repeated(),
                |lhs, (op, rhs)| bin_op(lhs, op, rhs),
            )
            .boxed();
        let bitxor = bitand
            .clone()
            .foldl(
                token(TokenKind::Caret)
                    .to(BinOp::BitXor)
                    .then(operand(tokens, bitand.clone()))
                    .repeated(),
                |lhs, (op, rhs)| bin_op(lhs, op, rhs),
            )
            .boxed();
        let bitor = bitxor
            .clone()
            .foldl(
                token(TokenKind::Pipe)
                    .to(BinOp::BitOr)
                    .then(operand(tokens, bitxor.clone()))
                    .repeated(),
                |lhs, (op, rhs)| bin_op(lhs, op, rhs),
            )
            .boxed();

        // The comparisons, left-associative, and the loosest binary level; `@in`
        // sits here too (it yields the same `0`/`1`).

        // `>` is both the comparison and a delimiter: it compares only when an
        // expression follows it **unglued**.

        // Otherwise it closes the angle bracket it is in
        // (`docs/notes/operators.md` §4).
        let comparison_loose = choice((
            token(TokenKind::LAngle).to(BinOp::Lt),
            token(TokenKind::Leq).to(BinOp::Leq),
            token(TokenKind::Geq).to(BinOp::Geq),
            token(TokenKind::Eq).to(BinOp::Eq),
            token(TokenKind::Neq).to(BinOp::Neq),
            token(TokenKind::KwIn).to(BinOp::In),
        ))
        .then(operand(tokens, bitor.clone()));
        let comparison_gt = a_comparison_follows(tokens)
            .ignore_then(token(TokenKind::RAngle))
            .to(BinOp::Gt)
            .then(operand(tokens, bitor.clone()));
        let comparison = bitor
            .clone()
            .foldl(
                choice((comparison_loose, comparison_gt)).repeated(),
                |lhs, (op, rhs)| bin_op(lhs, op, rhs),
            )
            .boxed();

        // `->`, right-associative.
        let term3 = comparison
            .clone()
            .then(
                token(TokenKind::Arrow)
                    .ignore_then(operand(tokens, comparison.clone()))
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

        // `:` `#` `!` `?` at the same precedence, right-associative; each right
        // side parses at the `->` level.
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
                    token(TokenKind::Bang)
                        .ignore_then(operand(tokens, term3.clone()))
                        .map(AnnPiece::Refinement),
                    token(TokenKind::Question)
                        .ignore_then(operand(tokens, term3.clone()))
                        .map(AnnPiece::Doc),
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
                    Expr::Name(parameter, span, _) => Expr::Lambda {
                        parameter,
                        parameter_span: span,
                        parameter_binder: None,
                        parameter_type: None,
                        parameter_perspective: None,
                        r#return: rhs,
                        span,
                    },
                    Expr::Annotation {
                        value,
                        r#type,
                        perspective,
                        refinement,
                        span,
                        ..
                    } => match *value {
                        Expr::Name(parameter, parameter_span, _) => {
                            // A parameter refinement desugars to `x => { x ! p; e }`
                            // (`docs/notes/operator-polymorphism.md` §3).
                            let r#return = match refinement {
                                None => rhs,
                                Some(refinement) => {
                                    let parameter_value =
                                        Expr::Name(parameter.clone(), parameter_span, None);
                                    Box::new(Expr::Block {
                                        statements: vec![Stmt::Expr(Expr::Annotation {
                                            value: Box::new(parameter_value),
                                            r#type: None,
                                            perspective: None,
                                            refinement: Some(refinement),
                                            doc: None,
                                            span: parameter_span,
                                        })],
                                        expr: rhs,
                                        span,
                                    })
                                }
                            };
                            Expr::Lambda {
                                parameter,
                                parameter_span,
                                parameter_binder: None,
                                parameter_type: r#type,
                                parameter_perspective: perspective,
                                r#return,
                                span,
                            }
                        }
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
            .boxed()
    })
}

/// Fold a binary operator into its [`Expr`] node, spanning from the left
/// operand.
fn bin_op(left: Expr, operator: BinOp, right: Expr) -> Expr {
    let span = left.span();
    Expr::BinOp {
        operator,
        left: Box::new(left),
        right: Box::new(right),
        span,
    }
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

/// One `: T`, `# p`, `! r`, or `? e` partner of an annotation chain.
#[derive(Clone, Debug)]
enum AnnPiece {
    Type(Expr),
    Perspective(Expr),
    Refinement(Expr),
    Doc(Expr),
}

/// Accumulate a `: T` / `# p` / `! r` / `? e` chain into one
/// [`Expr::Annotation`]; a later one of a kind overwrites.
fn fold_annotations(first: Expr, rest: Vec<AnnPiece>) -> Expr {
    if rest.is_empty() {
        return first;
    }
    let span = first.span();
    let value = Box::new(first);
    let mut r#type = None;
    let mut perspective = None;
    let mut refinement = None;
    let mut doc = None;
    for piece in rest {
        match piece {
            AnnPiece::Type(t) => r#type = Some(Box::new(t)),
            AnnPiece::Perspective(p) => perspective = Some(Box::new(p)),
            AnnPiece::Refinement(r) => refinement = Some(Box::new(r)),
            AnnPiece::Doc(d) => doc = Some(Box::new(d)),
        }
    }
    Expr::Annotation {
        value,
        r#type,
        perspective,
        refinement,
        doc,
        span,
    }
}

/// A `=>` chain before it is validated into a lambda.
#[derive(Clone, Debug)]
enum Pre {
    E(Expr),
    FatArrow(Box<Expr>, Box<Expr>),
}

/// Whether a token can begin an expression — the `>` comparison lookahead.
///
/// # Invariant
/// It mirrors [`atom_parser`]'s primaries, minus `Glue`: a glued delimiter is a
/// postfix marker, never the start of a new operand.  Drift is loud rather than
/// silent — either direction is a parse error at the token, never a misparse.
fn starts_an_expression(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Str(_)
            | TokenKind::Name(_)
            | TokenKind::Placeholder
            | TokenKind::KwInt
            | TokenKind::KwFloat
            | TokenKind::KwString
            | TokenKind::KwType
            | TokenKind::KwStruct
            | TokenKind::KwTable
            | TokenKind::KwSet
            | TokenKind::KwIf
            | TokenKind::KwArray
            | TokenKind::KwAssert
            // The `unary` level's prefix operators begin an expression without
            // being atoms, so `a > int2float b` compares.
            | TokenKind::KwInt2Float
            | TokenKind::KwFloat2Int
            | TokenKind::Dollar
            | TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::LBrace
            | TokenKind::LAngle
    )
}

/// The zero-width lookahead deciding whether the `>` at the cursor compares.
///
/// # Invariant
/// It consumes nothing and matches **at** the `>` — see the comparison level in
/// [`expression`] for why the position matters.
fn a_comparison_follows<'a>(tokens: &'a [Token]) -> impl Parser<'a, In<'a>, (), E<'a>> + Clone {
    // The span of a zero-width match is the cursor's own token index, so the
    // token after the `>` is `at + 1`.
    empty::<In<'a>, E<'a>>()
        .map_with(move |_, me| {
            tokens
                .get(me.span().start + 1)
                .is_some_and(|token| starts_an_expression(&token.kind))
        })
        .filter(|follows: &bool| *follows)
        .ignored()
}

/// The atoms, with their postfix forms: `e[i]`, `a(k)`, `t{k}`, `X<e>`, `X::a`,
/// and `C(...)`.
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
                .filter(|t: &Token| matches!(t.kind, TokenKind::Float(_)))
                .map(|t| match t.kind {
                    TokenKind::Float(n) => Expr::Float(n, t.span),
                    _ => unreachable!("filtered for a float"),
                }),
            any::<In<'a>, E<'a>>()
                .filter(|t: &Token| matches!(t.kind, TokenKind::Str(_)))
                .map(|t| match t.kind {
                    TokenKind::Str(s) => Expr::Str(s, t.span),
                    _ => unreachable!("filtered for a string"),
                }),
            token(TokenKind::KwInt).map(|t| Expr::TypeConst(TypeConst::Int, t.span)),
            token(TokenKind::KwFloat).map(|t| Expr::TypeConst(TypeConst::Float, t.span)),
            token(TokenKind::KwString).map(|t| Expr::TypeConst(TypeConst::String, t.span)),
            token(TokenKind::KwType).map(|t| Expr::TypeConst(TypeConst::Type, t.span)),
            // `_` is its own token, never a name: it can appear as a value but
            // cannot be bound.
            token(TokenKind::Placeholder).map(|t| Expr::Placeholder(t.span)),
            name().map(|(n, span)| Expr::Name(n, span, None)),
            native_call(tokens, expr.clone()),
            paren(tokens, expr.clone()),
            array_literal(tokens, expr.clone()),
            table_literal(tokens, expr.clone()),
            set_literal(tokens, expr.clone()),
            block(tokens, expr.clone()),
            angle_tuple(tokens, expr.clone()),
            struct_type(tokens, expr.clone()),
            array_type(tokens, expr.clone()),
            if_expr(tokens, expr.clone()),
        )))
        .labelled("an expression");

    // The postfix forms, chained left: `[` is an index, a glued `<` is `X<e>`,
    // and a glued `::name` is `X::a`.
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
            .map(Postfix::RawIndex),
        glue.clone()
            .ignore_then(token(TokenKind::DoubleColon))
            .ignore_then(name())
            .map(|(field_name, _)| Postfix::RawFieldName(field_name)),
        glue.clone()
            .ignore_then(token(TokenKind::LBrace))
            .ignore_then(expr.clone())
            .then_ignore(token(TokenKind::RBrace))
            .map(Postfix::TableFind),
        glue.clone()
            .ignore_then(token(TokenKind::LParen))
            .ignore_then(struct_inst_fields(expr.clone()))
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
                    Postfix::RawIndex(index) => Expr::RawIndex {
                        container: Box::new(acc),
                        index: Box::new(index),
                        span,
                    },
                    Postfix::RawFieldName(name) => Expr::RawNamedField {
                        container: Box::new(acc),
                        name,
                        span,
                    },
                    Postfix::Paren((fields, saw_comma)) => {
                        // A single comma-free expression is the slot read; every empty
                        // or comma-bearing form instantiates.
                        if fields.len() == 1 && !saw_comma && fields[0].name.is_none() {
                            let mut iter = fields.into_iter();
                            let only = iter.next().unwrap();
                            Expr::FieldRead {
                                container: Box::new(acc),
                                key: Box::new(only.value),
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
    RawIndex(Expr),
    RawFieldName(String),
    Paren((Vec<StructInstArg>, bool)),
}

/// `.name expr` or a plain expression — one instantiation field argument.
fn struct_inst_field<'a>(
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, StructInstArg, E<'a>> + Clone {
    choice((
        token(TokenKind::Dot)
            .ignore_then(name())
            .then(expr.clone())
            .map(|((name, _span), value)| StructInstArg {
                name: Some(name),
                value,
            }),
        expr.clone()
            .map(|value| StructInstArg { name: None, value }),
    ))
}

/// A comma-separated list of zero or more items, runs tolerated — plus whether
/// any comma appeared.
///
/// # Invariant
/// Every list form shares this one combinator, so their comma discipline cannot
/// differ.
fn comma_list<'a, T>(
    item: impl Parser<'a, In<'a>, T, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, (Vec<T>, bool), E<'a>> + Clone {
    let first = item.clone().or_not();
    first
        .then(
            separators_between()
                .ignore_then(item)
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(separator_run())
        .map(|((first, rest), trailing)| {
            // A separator between items or after the last means instantiation; a
            // bare `A(1)` is the slot read.
            let saw_comma = !rest.is_empty() || !trailing.is_empty();
            let mut items = Vec::new();
            if let Some(first) = first {
                items.push(first);
            }
            items.extend(rest);
            (items, saw_comma)
        })
}

/// The content of an adjacent `(` in an instantiation position, plus whether
/// any comma appeared.
fn struct_inst_fields<'a>(
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, (Vec<StructInstArg>, bool), E<'a>> + Clone {
    comma_list(struct_inst_field(expr))
}

/// The content of an adjacent `(`: expressions plus whether any comma appeared.
fn paren_fields<'a>(
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, (Vec<Expr>, bool), E<'a>> + Clone {
    comma_list(expr)
}

/// `(e)` — transparent grouping; `(e1, …, en)` — a tuple value (angle brackets
/// spell the tuple type).
fn paren<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::LParen)
        .ignore_then(expr.clone())
        .then(
            separators_between()
                .ignore_then(expr.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(separator_run())
        .then_ignore(token(TokenKind::RParen))
        .map_with(|((first, rest), trailing), me| {
            let span = span_at(tokens, me.span().start);
            if rest.is_empty() && trailing.is_empty() {
                first
            } else {
                Expr::Tuple(std::iter::once(first).chain(rest).collect(), span)
            }
        })
}

/// `$name(args…)` — a native-operator call; the name resolves only against the
/// module's private registry.
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

/// `[e1, …, en]` — an array literal, where an element may carry the `~` shallow
/// marker.
fn array_literal<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    let element = tilde_marked(expr.clone());
    token(TokenKind::LBracket)
        .ignore_then(element.clone())
        .then(
            separators_between()
                .ignore_then(element.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(separator_run())
        .then_ignore(token(TokenKind::RBracket))
        .map_with(|((first, rest), _trailing), me| {
            Expr::Array(
                std::iter::once(first).chain(rest).collect(),
                span_at(tokens, me.span().start),
            )
        })
}

/// `table { k ==> v, … }` — a constant table literal; `==>` separates the pair
/// unambiguously.
fn table_literal<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    let entry = expr
        .clone()
        .then(
            token(TokenKind::TableArrow)
                .ignore_then(operand(tokens, expr.clone()))
                .or_not(),
        )
        .validate(|(key, value), me, emit| match value {
            Some(value) => (key, value),
            None => {
                emit.emit(Rich::custom(
                    me.span(),
                    "a table entry must be a `key ==> value` pair",
                ));
                // Recover like any parse error: the entry's key compiles,
                // its value is a masked error block.
                (key, err_node(tokens, me.span()))
            }
        });
    token(TokenKind::KwTable)
        .ignore_then(token(TokenKind::Glue).ignored().or_not())
        .ignore_then(token(TokenKind::LBrace))
        .ignore_then(comma_list(entry))
        .then_ignore(token(TokenKind::RBrace))
        .map_with(|(entries, _), me| Expr::Table(entries, span_at(tokens, me.span().start)))
}

/// `set{a, b, …}` — a set of ordinary values, keyword-led like `table{…}`.
fn set_literal<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::KwSet)
        .ignore_then(token(TokenKind::Glue).ignored().or_not())
        .ignore_then(token(TokenKind::LBrace))
        .ignore_then(comma_list(expr))
        .then_ignore(token(TokenKind::RBrace))
        .map_with(|(members, _), me| Expr::Set(members, span_at(tokens, me.span().start)))
}

/// An array element with an optional `~` prefix; the marker wraps it as
/// [`Expr::Shallow`].
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

/// `<e1, …, en>` — always a `TypeTuple`, with at least two elements.
fn angle_tuple<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::LAngle)
        .ignore_then(expr.clone())
        .then(
            separators_between()
                .ignore_then(expr.clone())
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then(separator_run())
        .then_ignore(token(TokenKind::RAngle))
        .map_with(|((first, rest), _trailing), me| {
            Expr::TypeTuple(
                std::iter::once(first).chain(rest).collect(),
                span_at(tokens, me.span().start),
            )
        })
}

/// One `struct<…>` field: `.name type`, or a bare (positional) type.
fn struct_field<'a>(
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, StructField, E<'a>> + Clone {
    let named = token(TokenKind::Dot)
        .ignore_then(name())
        .then(expr.clone())
        .map(|((n, _span), ty)| StructField { name: Some(n), ty });
    let unnamed = expr.map(|ty| StructField { name: None, ty });
    choice((named, unnamed))
}

/// `struct<…>` — a nominal struct type; the field list is optional (`struct<>`).
fn struct_type<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    let field = struct_field(expr.clone());
    token(TokenKind::KwStruct)
        .ignore_then(token(TokenKind::Glue).ignored().or_not())
        .ignore_then(token(TokenKind::LAngle))
        // The separator flag is the instantiation split's, not a struct type's.
        .ignore_then(comma_list(field).map(|(fields, _saw_separator)| fields))
        .then_ignore(token(TokenKind::RAngle))
        .map_with(|fields, me| Expr::StructType(fields, span_at(tokens, me.span().start)))
}

/// `array<T, n>` — the array type, keyword-led like `struct<…>`, with exactly
/// two fields.
fn array_type<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::KwArray)
        .ignore_then(token(TokenKind::Glue).ignored().or_not())
        .ignore_then(token(TokenKind::LAngle))
        .ignore_then(expr.clone())
        .then(separators_between().ignore_then(expr.clone()))
        .then(separator_run())
        .then_ignore(token(TokenKind::RAngle))
        .map_with(|((element_type, length), _trailing), me| Expr::TypeArray {
            element_type: Box::new(element_type),
            length: Box::new(length),
            span: span_at(tokens, me.span().start),
        })
}

/// `{ stmt; …; expr }` — a block: a program's statement list plus its value.
fn block<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    token(TokenKind::LBrace)
        .ignore_then(block_body(tokens, expr))
        .then_ignore(token(TokenKind::RBrace))
        .map_with(|items, me| {
            let (statements, _ranges, tail) = split_block_items(items);
            let span = span_at(tokens, me.span().start);
            match tail {
                Some(e) => Expr::Block {
                    statements: statements.into_iter().map(|b| b.stmt).collect(),
                    expr: Box::new(e),
                    span,
                },
                None => Expr::RecordBlock {
                    fields: statements
                        .into_iter()
                        .map(|b| {
                            let public = b.public;
                            let span = b.stmt.span();
                            let (name, value, field, cached, looping) = match b.stmt {
                                Stmt::Binding(binding) => (
                                    Some(binding.name),
                                    binding.value,
                                    // A `let` binding is a block-local, never a
                                    // struct field.
                                    !binding.restrictive,
                                    binding.cached,
                                    binding.looping,
                                ),
                                Stmt::Expr(e) => (None, e, true, false, false),
                            };
                            RecordField {
                                name,
                                binder: None,
                                value,
                                public,
                                field,
                                cached,
                                looping,
                                span,
                            }
                        })
                        .collect(),
                    span,
                },
            }
        })
}

/// A block-body item: either a (possibly `pub`) statement, or a `return
/// <expr>` tail marker.
enum BlockItem {
    Stmt(BlockStmt),
    Return(Expr),
}

/// A block body: a separator-separated list of body items — a possibly `pub`
/// statement or a `return <expr>` tail marker.
///
/// # Invariant
/// The end of the input or a `}` is **not** a separator; it terminates the list.
/// The tail is resolved later by [`split_block_items`].
type BlockBody = Vec<(BlockItem, (usize, usize))>;

fn block_body<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, BlockBody, E<'a>> + Clone {
    let seps = separator_run();
    let seps1 = separators_between();
    // A broken item skips tokens and retries, stopping at Eof or a `}` so it
    // never swallows the terminator.
    let item = choice((
        token(TokenKind::KwReturn)
            .ignore_then(expr.clone())
            .map(BlockItem::Return),
        block_statement(tokens, expr.clone()).map(BlockItem::Stmt),
    ))
    .recover_with(skip_then_retry_until(
        any::<In<'a>, E<'a>>().ignored(),
        choice((
            token(TokenKind::Eof).ignored(),
            token(TokenKind::RBrace).ignored(),
        )),
    ))
    .map_with(|i, me| (i, (me.span().start, me.span().end)));
    seps.clone()
        .then(item.clone())
        .then(
            (seps1.clone().then(item.clone()))
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then(seps)
        .map(|(((_, first), rest), _trailing)| {
            std::iter::once(first)
                .chain(rest.into_iter().map(|(_, e)| e))
                .collect::<Vec<(BlockItem, (usize, usize))>>()
        })
}

/// Resolve a block body's **tail**: an explicit `return <expr>`, else the last
/// bare expression.
fn split_block_items(
    items: Vec<(BlockItem, (usize, usize))>,
) -> (Vec<BlockStmt>, Vec<(usize, usize)>, Option<Expr>) {
    // An explicit `return <expr>` designates the tail; a second `return` is
    // dropped.
    if items
        .iter()
        .any(|(i, _)| matches!(i, BlockItem::Return(..)))
    {
        let mut tail = None;
        let mut tail_range = None;
        let mut out = Vec::new();
        for (item, range) in items {
            match item {
                BlockItem::Return(e) => {
                    if tail.is_none() {
                        tail = Some(e);
                        tail_range = Some(range);
                    }
                }
                BlockItem::Stmt(b) => out.push((b, range)),
            }
        }
        let (statements, mut ranges): (Vec<BlockStmt>, Vec<(usize, usize)>) =
            out.into_iter().unzip();
        if let Some(r) = tail_range {
            ranges.push(r);
        }
        return (statements, ranges, tail);
    }
    // No `return`: the last bare expression is the body's tail value; a
    // trailing binding (or an empty body) leaves no tail.
    let mut items = items;
    match items.pop() {
        Some((
            BlockItem::Stmt(BlockStmt {
                stmt: Stmt::Expr(e),
                ..
            }),
            range,
        )) => {
            let (statements, mut ranges): (Vec<BlockStmt>, Vec<(usize, usize)>) = items
                .into_iter()
                .map(|(i, r)| match i {
                    BlockItem::Stmt(b) => (b, r),
                    BlockItem::Return(..) => unreachable!("handled above"),
                })
                .unzip();
            ranges.push(range);
            (statements, ranges, Some(e))
        }
        Some(other) => {
            items.push(other);
            let (statements, ranges): (Vec<BlockStmt>, Vec<(usize, usize)>) = items
                .into_iter()
                .map(|(i, r)| match i {
                    BlockItem::Stmt(b) => (b, r),
                    BlockItem::Return(..) => unreachable!("handled above"),
                })
                .unzip();
            (statements, ranges, None)
        }
        None => (Vec::new(), Vec::new(), None),
    }
}

/// A statement inside a block: an optional `pub` prefix; `pub` cannot mark a
/// `let` binding.
fn block_statement<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone,
) -> impl Parser<'a, In<'a>, BlockStmt, E<'a>> + Clone {
    token(TokenKind::KwPub)
        .ignored()
        .or_not()
        .then(statement(tokens, expr.clone()))
        .validate(|(public, stmt), me, emit| {
            if public.is_some() && matches!(&stmt, Stmt::Binding(b) if b.restrictive) {
                emit.emit(Rich::custom(me.span(), "pub cannot mark a let binding"));
            }
            BlockStmt {
                stmt,
                public: public.is_some(),
            }
        })
}

/// `if cond then e1 else e2` — the keywords delimit the condition and the
/// branches extend maximally.
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

#[cfg(test)]
#[path = "tests/parse_tests.rs"]
mod tests;
