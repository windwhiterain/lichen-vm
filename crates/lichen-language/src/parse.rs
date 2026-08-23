//! The recursive-descent parser: tokens → AST.
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
//! is an expression —
//! a bare expression is a statement anywhere, and only the last statement is
//! the list's value.  Within an expression, one grammar covers terms and
//! types (types are expressions); a *type-mode* flag — set inside an
//! annotation's right side — decides whether `(a, b)` is a `Tuple` value or a
//! `TypeTuple` type expression.  Angle brackets are exclusively type-level
//! and need no flag: `<a, b>` is always a `TypeTuple`, `struct<T1, T2>`
//! always a `StructType`, and `T<e>` (a `<` directly after an expression) is
//! always the array type.  A `[` directly after an expression is always an
//! index `e[i]` — the array literal is the prefix `[e, …]` form, so no
//! whitespace rule decides, and an array literal in argument position needs
//! parens: `f ([1, 2])`.  Precedence (loosest → tightest): `:` (right) →
//! `->` (right) → `<=`/`==` (left) → `+`/`-` (left) → application (left) →
//! postfix `<e>` / `[e]` / atoms.
//! `name =>` starts a lambda only in prefix position, so `f x => e` is a
//! parse error rather than `f (x => e)`; `name : T => e` is a lambda whose
//! parameter is annotated with `T`.  `if cond then e1 else e2` is a
//! conditional expression (the `then`/`else` keywords delimit the branches,
//! which extend maximally).

use lichen_highlevel::ir::Span;

use crate::ast::{BinOp, Binding, Expr, Program, Stmt, TypeConst};
use crate::diag::{Diag, Stage};
use crate::lex::{Token, TokenKind};

pub fn parse(tokens: &[Token]) -> Result<Program, Diag> {
    let mut parser = Parser { tokens, pos: 0 };
    let program = parser.parse_program()?;
    if !parser.at(&TokenKind::Eof) {
        return Err(parser.error(format!(
            "unexpected {} after the program",
            parser.peek().kind.describe()
        )));
    }
    Ok(program)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn next(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        self.pos += 1;
        token
    }

    fn at(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn error(&self, message: String) -> Diag {
        Diag::new(Stage::Parse, self.peek().span, message)
    }

    /// "expected X, found Y" at the current token.
    fn error_found(&self, expected: &str) -> Diag {
        let found = self.peek().kind.describe();
        self.error(format!("expected {expected}, found {found}"))
    }

    /// The program: the statement list, then `Eof`.
    fn parse_program(&mut self) -> Result<Program, Diag> {
        let (statements, expr) = self.parse_bindings_and_expr(false)?;
        Ok(Program { statements, expr })
    }

    /// The statement list: `name = expr;` bindings and bare expressions,
    /// then the final expression.  A `name =` at statement start is a
    /// binding; anything else is an expression — a bare expression is a
    /// statement anywhere in the list (there is no dead code: the compiler
    /// checks every statement), and only the *last* one is the list's value.
    /// A binding without `let` is *block-wide*: its name is in scope
    /// throughout the block, forward and backward, so bindings may reference
    /// and recurse with each other.  A `let name =` binding is *restrictive*:
    /// the name is in scope only in later statements (see
    /// [`Parser::parse_bindings_and_expr`]).
    /// Every non-final statement requires a trailing separator — `;` or a
    /// newline (the lexer merges both into `Semicolon`).  The same list
    /// forms a block's body (see [`Parser::block`]).
    fn parse_bindings_and_expr(&mut self, type_mode: bool) -> Result<(Vec<Stmt>, Expr), Diag> {
        let mut statements = Vec::new();
        self.skip_separators();
        loop {
            // `let name = …` — the restrictive form: the `let` keyword, then
            // the ordinary `name =`.  Without `let`, a binding is block-wide
            // (visible throughout the block, so it may recurse with itself).
            let restrictive = matches!(
                (
                    &self.peek().kind,
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    self.tokens.get(self.pos + 2).map(|t| &t.kind),
                ),
                (
                    TokenKind::KwLet,
                    Some(TokenKind::Name(_)),
                    Some(TokenKind::Equals)
                )
            );
            let binding = restrictive
                || matches!(
                    (
                        &self.peek().kind,
                        self.tokens.get(self.pos + 1).map(|t| &t.kind)
                    ),
                    (TokenKind::Name(_), Some(TokenKind::Equals))
                );
            if binding {
                let (name, span, restrictive) = if restrictive {
                    self.next(); // the `let`
                    let name = self.next();
                    let TokenKind::Name(binding_name) = name.kind else {
                        unreachable!()
                    };
                    self.pos += 1; // the `=`
                    (binding_name, name.span, true)
                } else {
                    let name = self.next();
                    let TokenKind::Name(binding_name) = name.kind else {
                        unreachable!()
                    };
                    self.pos += 1; // the `=`
                    (binding_name, name.span, false)
                };
                let value = self.parse_expr(type_mode)?;
                statements.push(Stmt::Binding(Binding {
                    name,
                    span,
                    value,
                    restrictive,
                }));
                if !self.eat(&TokenKind::Semicolon) {
                    return Err(self.error_found("';'"));
                }
                self.skip_separators();
                continue;
            }
            // A bare expression: the final one unless a separator leads to
            // more statements.
            let expr = self.parse_expr(type_mode)?;
            if self.at(&TokenKind::Semicolon) {
                self.skip_separators();
                if self.at(&TokenKind::Eof) || self.at(&TokenKind::RBrace) {
                    // Trailing separators after the final expression.
                    return Ok((statements, expr));
                }
                statements.push(Stmt::Expr(expr));
                continue;
            }
            return Ok((statements, expr));
        }
    }

    /// Skip statement separators — `;` or a newline, both lexed as
    /// `Semicolon`.  Consecutive ones are empty statements, and leading and
    /// trailing ones are ignored: `a = 1;\nb = 2` and a top-of-file comment's
    /// newline both parse.
    fn skip_separators(&mut self) {
        while self.eat(&TokenKind::Semicolon) {}
    }

    /// An expression.  A name directly followed by `=>` starts a lambda whose
    /// body extends maximally — through `:` and `->`.  An annotation whose
    /// value is a bare name directly followed by `=>` (`x : Int => e`) is
    /// likewise a lambda — the parameter annotated with the annotation's
    /// type.
    fn parse_expr(&mut self, type_mode: bool) -> Result<Expr, Diag> {
        let lambda = match &self.peek().kind {
            TokenKind::Name(_) => matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::FatArrow)
            ),
            _ => false,
        };
        if lambda {
            let name = self.next();
            let TokenKind::Name(parameter) = name.kind else {
                unreachable!()
            };
            self.pos += 1; // the `=>`
            let body = self.parse_expr(type_mode)?;
            return Ok(Expr::Lambda {
                parameter,
                parameter_span: name.span,
                parameter_type: None,
                r#return: Box::new(body),
                span: name.span,
            });
        }
        let expr = self.parse_infix(type_mode, 0)?;
        // `x : T => e` — the annotation's value is a bare name and `=>`
        // follows: a lambda whose parameter is annotated with `T`.  This is
        // the only parse of that token sequence (`=>` is not an infix
        // operator, so the annotation cannot extend through it), so there is
        // no ambiguity with a plain annotation.
        let (parameter, parameter_span, parameter_type, span) = match expr {
            Expr::Annotation {
                value,
                r#type,
                span,
            } => match *value {
                Expr::Name(parameter, parameter_span) => (parameter, parameter_span, r#type, span),
                value => {
                    return Ok(Expr::Annotation {
                        value: Box::new(value),
                        r#type,
                        span,
                    });
                }
            },
            expr => return Ok(expr),
        };
        if self.at(&TokenKind::FatArrow) {
            self.next();
            let body = self.parse_expr(type_mode)?;
            return Ok(Expr::Lambda {
                parameter,
                parameter_span,
                parameter_type: Some(parameter_type),
                r#return: Box::new(body),
                span,
            });
        }
        Ok(Expr::Annotation {
            value: Box::new(Expr::Name(parameter, parameter_span)),
            r#type: parameter_type,
            span,
        })
    }

    fn parse_infix(&mut self, type_mode: bool, min_bp: u8) -> Result<Expr, Diag> {
        let mut lhs = self.parse_atom(type_mode)?;
        loop {
            if self.atom_start() {
                // Application binds tightest: `x y : Int` = `(x y) : Int`.
                let rhs = self.parse_atom(type_mode)?;
                let span = lhs.span();
                lhs = Expr::Apply {
                    function: Box::new(lhs),
                    argument: Box::new(rhs),
                    span,
                };
                continue;
            }
            let operator = self.peek().kind.clone();
            let (bp, is_annotation, is_left_assoc) = match &operator {
                TokenKind::Colon => (10, true, false),
                TokenKind::Arrow => (20, false, false),
                // Comparisons bind looser than arithmetic; both are
                // left-associative (unlike `:` and `->`, which are right).
                TokenKind::Leq | TokenKind::Eq => (25, false, true),
                TokenKind::Plus | TokenKind::Minus => (30, false, true),
                _ => break,
            };
            if bp < min_bp {
                break;
            }
            self.next();
            // The annotation's right side is a type expression; the other
            // operators keep the current mode.
            let rhs = self.parse_infix(
                if is_annotation { true } else { type_mode },
                if is_left_assoc { bp + 1 } else { bp },
            )?;
            let span = lhs.span();
            lhs = if is_annotation {
                Expr::Annotation {
                    value: Box::new(lhs),
                    r#type: Box::new(rhs),
                    span,
                }
            } else if matches!(operator, TokenKind::Arrow) {
                Expr::Arrow {
                    parameter: Box::new(lhs),
                    r#return: Box::new(rhs),
                    span,
                }
            } else {
                let operator = match operator {
                    TokenKind::Plus => BinOp::Add,
                    TokenKind::Minus => BinOp::Sub,
                    TokenKind::Leq => BinOp::Leq,
                    TokenKind::Eq => BinOp::Eq,
                    _ => unreachable!("only the binary operators reach here"),
                };
                Expr::BinOp {
                    operator,
                    left: Box::new(lhs),
                    right: Box::new(rhs),
                    span,
                }
            };
        }
        Ok(lhs)
    }

    fn parse_atom(&mut self, type_mode: bool) -> Result<Expr, Diag> {
        let start = self.next();
        let atom = match &start.kind {
            TokenKind::Int(n) => Expr::Int(*n, start.span),
            TokenKind::KwInt => Expr::TypeConst(TypeConst::Int, start.span),
            TokenKind::KwType => Expr::TypeConst(TypeConst::Type, start.span),
            TokenKind::Name(name) if type_mode && name == "_" => Expr::Placeholder(start.span),
            TokenKind::Name(name) => Expr::Name(name.clone(), start.span),
            TokenKind::LParen => self.paren_or_tuple(type_mode, start.span)?,
            TokenKind::LBracket => self.array_literal(type_mode, start.span)?,
            TokenKind::LBrace => self.block(type_mode, start.span)?,
            TokenKind::LAngle => self.angle_tuple(type_mode, start.span)?,
            TokenKind::KwStruct => self.struct_type(start.span)?,
            TokenKind::KwIf => self.if_expr(type_mode, start.span)?,
            _ => {
                return Err(Diag::new(
                    Stage::Parse,
                    start.span,
                    format!("expected an expression, found {}", start.kind.describe()),
                ));
            }
        };
        self.postfix(atom, type_mode, start.span)
    }

    /// The postfix forms: `e[i]` (index) and `T<e>` (array type), chained and
    /// with no whitespace rule — a bracket right after an expression is
    /// always the postfix form, application is juxtaposition.
    fn postfix(&mut self, atom: Expr, type_mode: bool, span: Span) -> Result<Expr, Diag> {
        if self.at(&TokenKind::LBracket) {
            self.next();
            if self.at(&TokenKind::RBracket) {
                return Err(self.error_found("an expression"));
            }
            let index = self.parse_expr(type_mode)?;
            if !self.eat(&TokenKind::RBracket) {
                return Err(self.error_found("']'"));
            }
            return self.postfix(
                Expr::Index {
                    array: Box::new(atom),
                    index: Box::new(index),
                    span,
                },
                type_mode,
                span,
            );
        }
        if self.at(&TokenKind::LAngle) {
            self.next();
            let length = self.parse_expr(type_mode)?;
            if !self.eat(&TokenKind::RAngle) {
                return Err(self.error_found("'>'"));
            }
            return self.postfix(
                Expr::TypeArray {
                    element_type: Box::new(atom),
                    length: Box::new(length),
                    span,
                },
                type_mode,
                span,
            );
        }
        Ok(atom)
    }

    /// `(e)` or `(e1, ..., en)` — the comma form is a `TypeTuple` in type
    /// position, a `Tuple` value in term position.  The opener has been
    /// consumed by [`Parser::parse_atom`].
    fn paren_or_tuple(&mut self, type_mode: bool, span: Span) -> Result<Expr, Diag> {
        if self.at(&TokenKind::RParen) {
            return Err(self.error_found("an expression"));
        }
        let first = self.parse_expr(type_mode)?;
        if !self.eat(&TokenKind::Comma) {
            if !self.eat(&TokenKind::RParen) {
                return Err(self.error_found("')'"));
            }
            return Ok(first);
        }
        let mut elements = vec![first];
        loop {
            if self.at(&TokenKind::RParen) {
                break;
            }
            elements.push(self.parse_expr(type_mode)?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        if !self.eat(&TokenKind::RParen) {
            return Err(self.error_found("')'"));
        }
        if type_mode {
            Ok(Expr::TypeTuple(elements, span))
        } else {
            Ok(Expr::Tuple(elements, span))
        }
    }

    /// `<e1, ..., en>` — always a `TypeTuple`, in term and type position
    /// alike (angle brackets are exclusively type-level, so there is no
    /// mode flag here, unlike `( )`).  At least two elements: a single
    /// element is a typo for either `(e)` grouping or a real tuple type.
    /// The opener has been consumed by [`Parser::parse_atom`].
    fn angle_tuple(&mut self, type_mode: bool, span: Span) -> Result<Expr, Diag> {
        if self.at(&TokenKind::RAngle) {
            return Err(self.error_found("an expression"));
        }
        let first = self.parse_expr(type_mode)?;
        if !self.eat(&TokenKind::Comma) {
            return Err(self.error_found("','"));
        }
        let mut elements = vec![first];
        loop {
            if self.at(&TokenKind::RAngle) {
                break;
            }
            elements.push(self.parse_expr(type_mode)?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        if !self.eat(&TokenKind::RAngle) {
            return Err(self.error_found("'>'"));
        }
        Ok(Expr::TypeTuple(elements, span))
    }

    /// `[e1, ..., en]` — an array literal.  The opener has been consumed by
    /// [`Parser::parse_atom`].
    fn array_literal(&mut self, type_mode: bool, span: Span) -> Result<Expr, Diag> {
        if self.at(&TokenKind::RBracket) {
            return Err(self.error_found("an expression"));
        }
        let mut elements = Vec::new();
        loop {
            elements.push(self.parse_expr(type_mode)?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.at(&TokenKind::RBracket) {
                break;
            }
        }
        if !self.eat(&TokenKind::RBracket) {
            return Err(self.error_found("']'"));
        }
        Ok(Expr::Array(elements, span))
    }

    /// `struct<T1, ..., Tn>` — a nominal struct type, positional fields.
    /// The fields are type expressions (parsed in type mode — angle
    /// brackets are exclusively type-level), at least one.  The opener
    /// `struct` has been consumed by [`Parser::parse_atom`].
    fn struct_type(&mut self, span: Span) -> Result<Expr, Diag> {
        if !self.eat(&TokenKind::LAngle) {
            return Err(self.error_found("'<'"));
        }
        if self.at(&TokenKind::RAngle) {
            return Err(self.error_found("an expression"));
        }
        let mut fields = Vec::new();
        loop {
            fields.push(self.parse_expr(true)?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.at(&TokenKind::RAngle) {
                break;
            }
        }
        if !self.eat(&TokenKind::RAngle) {
            return Err(self.error_found("'>'"));
        }
        Ok(Expr::StructType(fields, span))
    }

    /// `{ stmt; …; expr }` — a block: scoped statements followed by the
    /// block's value.  The opener has been consumed by
    /// [`Parser::parse_atom`]; the body is the same statement list as a
    /// program's, ending in the `}`.
    fn block(&mut self, type_mode: bool, span: Span) -> Result<Expr, Diag> {
        let (statements, expr) = self.parse_bindings_and_expr(type_mode)?;
        if !self.eat(&TokenKind::RBrace) {
            return Err(self.error_found("'}'"));
        }
        Ok(Expr::Block {
            statements,
            expr: Box::new(expr),
            span,
        })
    }

    /// `if cond then e1 else e2` — a conditional.  `cond` is any expression
    /// up to the `then` keyword (both keywords delimit it — neither is an
    /// atom or an infix operator, so the condition cannot extend through
    /// them); the branches extend maximally, like a lambda body.  The opener
    /// `if` has been consumed by [`Parser::parse_atom`].
    fn if_expr(&mut self, type_mode: bool, span: Span) -> Result<Expr, Diag> {
        let condition = self.parse_expr(type_mode)?;
        if !self.eat(&TokenKind::KwThen) {
            return Err(self.error_found("'then'"));
        }
        let then_branch = self.parse_expr(type_mode)?;
        if !self.eat(&TokenKind::KwElse) {
            return Err(self.error_found("'else'"));
        }
        let else_branch = self.parse_expr(type_mode)?;
        Ok(Expr::If {
            condition: Box::new(condition),
            then_branch: Box::new(then_branch),
            else_branch: Box::new(else_branch),
            span,
        })
    }

    fn atom_start(&self) -> bool {
        matches!(
            &self.peek().kind,
            TokenKind::Int(_)
                | TokenKind::KwInt
                | TokenKind::KwType
                | TokenKind::KwStruct
                | TokenKind::KwIf
                | TokenKind::Name(_)
                | TokenKind::LParen
                | TokenKind::LBracket
                | TokenKind::LBrace
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex::lex;

    /// The final expression of a program (bindings aside).
    fn parse_ok(source: &str) -> Expr {
        let tokens = lex(source).unwrap();
        parse(&tokens).unwrap().expr
    }

    fn parse_err(source: &str) -> Diag {
        let tokens = lex(source).unwrap();
        parse(&tokens).unwrap_err()
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
        let tokens = lex("a = [1, 2]; b = 0; a[b]").unwrap();
        let program = parse(&tokens).unwrap();
        let binds = bindings(&program);
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].name, "a");
        assert_eq!(binds[1].name, "b");
        assert!(matches!(binds[0].value, Expr::Array(..)));
        assert!(matches!(program.expr, Expr::Index { .. }));
    }

    #[test]
    fn statement_errors_carry_spans() {
        // A binding must be followed by a separator.
        let err = parse_err("a = 5");
        assert_eq!(err.message, "expected ';', found the end of the program");
        // A bare expression is accepted anywhere, but the list must end in a
        // final expression — a trailing binding (with its required
        // separator) leaves none.
        let err = parse_err("5; a = 1");
        assert_eq!(err.message, "expected ';', found the end of the program");
        let err = parse_err("a = 1; 5; a = 2");
        assert_eq!(err.message, "expected ';', found the end of the program");
        let err = parse_err("a = 1;");
        assert_eq!(
            err.message,
            "expected an expression, found the end of the program"
        );
        // A binding without a value.
        let err = parse_err("a = ; 5");
        assert_eq!(err.message, "expected an expression, found ';'");
    }

    #[test]
    fn bare_expressions_are_statements_anywhere() {
        // 5; a = 1; a — an expression before and between bindings; the last
        // statement is the final expression.
        let tokens = lex("5; a = 1; 6; a").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.statements.len(), 3);
        assert!(matches!(program.statements[0], Stmt::Expr(..)));
        assert!(matches!(program.statements[1], Stmt::Binding(..)));
        assert!(matches!(program.statements[2], Stmt::Expr(..)));
        assert!(matches!(program.expr, Expr::Name(name, _) if name == "a"));
        // 5; 6 — the last expression is the value.
        let tokens = lex("5; 6").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.statements.len(), 1);
        assert!(matches!(program.statements[0], Stmt::Expr(..)));
        assert!(matches!(program.expr, Expr::Int(6, _)));
    }

    #[test]
    fn newlines_separate_statements() {
        // a = [1, 2]\nb = 0\na[b] — each binding ends at its newline.
        let tokens = lex("a = [1, 2]\nb = 0\na[b]").unwrap();
        let program = parse(&tokens).unwrap();
        let binds = bindings(&program);
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].name, "a");
        assert_eq!(binds[1].name, "b");
        assert!(matches!(program.expr, Expr::Index { .. }));
        // A trailing newline after the final expression is not an error.
        parse(&lex("a = 1\na\n").unwrap()).unwrap();
        // `;` and newlines mix, and consecutive separators are empty
        // statements.
        let tokens = lex("a = 1;\nb = 2; c = 3\nc").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(bindings(&program).len(), 3);
        // Leading newlines (e.g. a top-of-file comment's) are skipped.
        let tokens = lex("\n\na = 1\na").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(bindings(&program).len(), 1);
    }

    #[test]
    fn application_is_left_associative() {
        // x y z = (x y) z
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
        // Int -> Int -> Int = Int -> (Int -> Int)
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
        // 5 : Int -> Int = 5 : (Int -> Int)
        let Expr::Annotation { value, r#type, .. } = parse_ok("5 : Int -> Int") else {
            panic!("expected an annotation")
        };
        assert!(matches!(*value, Expr::Int(5, _)));
        assert!(matches!(*r#type, Expr::Arrow { .. }));
        // x y : Int = (x y) : Int
        let Expr::Annotation { value, .. } = parse_ok("x y : Int") else {
            panic!("expected an annotation")
        };
        assert!(matches!(*value, Expr::Apply { .. }));
    }

    #[test]
    fn lambda_bodies_extend_maximally() {
        // x => e : Int = x => (e : Int)
        let Expr::Lambda { r#return, .. } = parse_ok("x => e : Int") else {
            panic!("expected a lambda")
        };
        assert!(matches!(*r#return, Expr::Annotation { .. }));
        // x => y => e = x => (y => e)
        let Expr::Lambda { r#return, .. } = parse_ok("x => y => e") else {
            panic!("expected a lambda")
        };
        assert!(matches!(*r#return, Expr::Lambda { .. }));
    }

    #[test]
    fn an_annotated_parameter_is_a_lambda() {
        // x : Int => x — the parameter carries the annotation.
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
        // `<Int, Type>` is a TypeTuple in type position — and stays one in
        // term position, unlike `(a, b)`.
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
        // struct<Int, Int -> Int> — fields are type expressions.
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
    fn struct_type_errors_carry_spans() {
        let err = parse_err("struct");
        assert_eq!(err.message, "expected '<', found the end of the program");
        let err = parse_err("struct<");
        assert_eq!(
            err.message,
            "expected an expression, found the end of the program"
        );
        let err = parse_err("struct<>");
        assert_eq!(err.message, "expected an expression, found '>'");
        let err = parse_err("struct<Int");
        assert_eq!(err.message, "expected '>', found the end of the program");
    }

    #[test]
    fn the_angle_bracket_array_type() {
        // Int<3> — an array type.
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
        let err = parse_err("f [1, 2]");
        assert_eq!(err.message, "expected ']', found ','");
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
        // a[0] — an index.
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
        let err = parse_err("a[]");
        assert_eq!(err.message, "expected an expression, found ']'");
        let err = parse_err("a[0");
        assert_eq!(err.message, "expected ']', found the end of the program");
    }

    #[test]
    fn a_single_element_angle_bracket_is_a_parse_error() {
        // `<Int>` is a typo for `(Int)` or a real tuple type.
        let err = parse_err("<Int>");
        assert_eq!(err.message, "expected ',', found '>'");
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
        assert_eq!(err.message, "expected ')', found the end of the program");

        let err = parse_err("x )");
        assert_eq!(err.message, "unexpected ')' after the program");

        let err = parse_err("[1, 2");
        assert_eq!(err.message, "expected ']', found the end of the program");
    }

    #[test]
    fn a_name_before_arrow_is_not_a_lambda_operand() {
        // `f x => e` — the lambda only starts in prefix position, so the
        // arrow is left dangling.
        let err = parse_err("f x => e");
        assert_eq!(err.stage, Stage::Parse);
    }

    #[test]
    fn an_underscore_in_type_position_is_a_placeholder() {
        // `x : _` — the annotation's type is a placeholder.
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
        // A block in argument position is an application, like any atom.
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
        assert_eq!(err.message, "expected '}', found the end of the program");
        // A block needs a final expression, like a program.
        let err = parse_err("{}");
        assert_eq!(err.message, "expected an expression, found '}'");
        // A block binding requires `;`.
        let err = parse_err("{a = 1}");
        assert_eq!(err.message, "expected ';', found '}'");
        // A stray `}` after the program.
        let err = parse_err("x }");
        assert_eq!(err.message, "unexpected '}' after the program");
    }
}
