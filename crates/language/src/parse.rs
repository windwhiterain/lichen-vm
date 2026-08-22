//! The recursive-descent parser: tokens → AST.
//!
//! A program is `name = expr; …` bindings followed by a final expression
//! (see [`Program`]).  Within an expression, one grammar covers terms and
//! types (types are expressions); a *type-mode* flag — set inside an
//! annotation's right side — decides whether `(a, b)` is a `Tuple` value or
//! a `TypeTuple` type expression.  Angle brackets and braces are
//! exclusively type-level and need no flag: `<a, b>` is always a
//! `TypeTuple`, `struct { T1, T2 }` always a `StructType`, and `T<e>` (a
//! `<` directly after an expression) is always the array type.  A `[`
//! directly after an expression is always an index `e[i]` — the array
//! literal is the prefix `[e, …]` form, so no whitespace rule decides, and
//! an array literal in argument position needs parens: `f ([1, 2])`.
//! Precedence (loosest → tightest): `:` (right) → `->` (right) →
//! application (left) → postfix `<e>` / `[e]` / atoms.  `name =>` starts a
//! lambda only in prefix position, so `f x => e` is a parse error rather
//! than `f (x => e)`.

use lichen_highlevel::ir::Span;

use crate::ast::{Binding, Expr, Program, TypeConst};
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

    /// The statement list: `name = expr;` bindings, then the final
    /// expression.  A `name =` at statement start is a binding; everything
    /// else is an expression — a bare expression statement is only legal as
    /// the last one (there is no dead code).  Bindings require the trailing
    /// `;`; the final expression is the program's value.
    fn parse_program(&mut self) -> Result<Program, Diag> {
        let mut bindings = Vec::new();
        loop {
            let binding = matches!(
                (&self.peek().kind, self.tokens.get(self.pos + 1).map(|t| &t.kind)),
                (TokenKind::Name(_), Some(TokenKind::Equals))
            );
            if !binding {
                break;
            }
            let name = self.next();
            let TokenKind::Name(binding_name) = name.kind else {
                unreachable!()
            };
            self.pos += 1; // the `=`
            let value = self.parse_expr(false)?;
            bindings.push(Binding {
                name: binding_name,
                span: name.span,
                value,
            });
            if !self.eat(&TokenKind::Semicolon) {
                return Err(self.error_found("';'"));
            }
        }
        let expr = self.parse_expr(false)?;
        Ok(Program { bindings, expr })
    }

    /// An expression.  A name directly followed by `=>` starts a lambda whose
    /// body extends maximally — through `:` and `->`.
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
                r#return: Box::new(body),
                span: name.span,
            });
        }
        self.parse_infix(type_mode, 0)
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
            let (bp, is_annotation) = match &self.peek().kind {
                TokenKind::Colon => (10, true),
                TokenKind::Arrow => (20, false),
                _ => break,
            };
            if bp < min_bp {
                break;
            }
            self.next();
            // The annotation's right side is a type expression; `->` keeps
            // the current mode.
            let rhs = self.parse_infix(if is_annotation { true } else { type_mode }, bp)?;
            let span = lhs.span();
            lhs = if is_annotation {
                Expr::Annotation {
                    value: Box::new(lhs),
                    r#type: Box::new(rhs),
                    span,
                }
            } else {
                Expr::Arrow {
                    parameter: Box::new(lhs),
                    r#return: Box::new(rhs),
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
            TokenKind::Name(name) => Expr::Name(name.clone(), start.span),
            TokenKind::LParen => self.paren_or_tuple(type_mode, start.span)?,
            TokenKind::LBracket => self.array_literal(type_mode, start.span)?,
            TokenKind::LAngle => self.angle_tuple(type_mode, start.span)?,
            TokenKind::KwStruct => self.struct_type(start.span)?,
            _ => {
                return Err(Diag::new(
                    Stage::Parse,
                    start.span,
                    format!(
                        "expected an expression, found {}",
                        start.kind.describe()
                    ),
                ))
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

    /// `struct { T1, ..., Tn }` — a nominal struct type, positional fields.
    /// The fields are type expressions (parsed in type mode — braces are
    /// exclusively type-level), at least one.  The opener `struct` has been
    /// consumed by [`Parser::parse_atom`].
    fn struct_type(&mut self, span: Span) -> Result<Expr, Diag> {
        if !self.eat(&TokenKind::LBrace) {
            return Err(self.error_found("'{'"));
        }
        if self.at(&TokenKind::RBrace) {
            return Err(self.error_found("an expression"));
        }
        let mut fields = Vec::new();
        loop {
            fields.push(self.parse_expr(true)?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.at(&TokenKind::RBrace) {
                break;
            }
        }
        if !self.eat(&TokenKind::RBrace) {
            return Err(self.error_found("'}'"));
        }
        Ok(Expr::StructType(fields, span))
    }

    fn atom_start(&self) -> bool {
        matches!(
            &self.peek().kind,
            TokenKind::Int(_)
                | TokenKind::KwInt
                | TokenKind::KwType
                | TokenKind::KwStruct
                | TokenKind::Name(_)
                | TokenKind::LParen
                | TokenKind::LBracket
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

    #[test]
    fn a_program_is_bindings_followed_by_the_final_expression() {
        let tokens = lex("a = [1, 2]; b = 0; a[b]").unwrap();
        let program = parse(&tokens).unwrap();
        assert_eq!(program.bindings.len(), 2);
        assert_eq!(program.bindings[0].name, "a");
        assert_eq!(program.bindings[1].name, "b");
        assert!(matches!(program.bindings[0].value, Expr::Array(..)));
        assert!(matches!(program.expr, Expr::Index { .. }));
    }

    #[test]
    fn statement_errors_carry_spans() {
        // A binding must be followed by `;`.
        let err = parse_err("a = 5");
        assert_eq!(err.message, "expected ';', found the end of the program");
        // A bare expression statement is only legal as the last one.
        let err = parse_err("5; a");
        assert_eq!(err.message, "unexpected ';' after the program");
        // A binding without a value.
        let err = parse_err("a = ; 5");
        assert_eq!(err.message, "expected an expression, found ';'");
    }

    #[test]
    fn application_is_left_associative() {
        // x y z = (x y) z
        let Expr::Apply { function, argument, .. } = parse_ok("x y z") else {
            panic!("expected an apply")
        };
        let Expr::Apply { argument: inner, .. } = *function else {
            panic!("the function must be the nested apply")
        };
        assert!(matches!(*inner, Expr::Name(..)));
        assert!(matches!(*argument, Expr::Name(..)));
    }

    #[test]
    fn arrows_are_right_associative() {
        // Int -> Int -> Int = Int -> (Int -> Int)
        let Expr::Arrow { parameter, r#return, .. } = parse_ok("Int -> Int -> Int") else {
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
    fn struct_types_are_positional_fields_in_braces() {
        // struct { Int, Int -> Int } — fields are type expressions.
        let Expr::StructType(fields, _) = parse_ok("struct { Int, Int -> Int }") else {
            panic!("expected a struct type")
        };
        assert_eq!(fields.len(), 2);
        assert!(matches!(fields[0], Expr::TypeConst(TypeConst::Int, _)));
        assert!(matches!(fields[1], Expr::Arrow { .. }));
        // a single field is the newtype form.
        assert!(matches!(
            parse_ok("struct { Int }"),
            Expr::StructType(f, _) if f.len() == 1
        ));
        // struct types are first-class values: an apply argument, a binding.
        let Expr::Apply { argument, .. } = parse_ok("f (struct { Int })") else {
            panic!("expected an apply")
        };
        assert!(matches!(*argument, Expr::StructType(..)));
        // fields are type expressions, so `(Int, Type)` inside is a
        // TypeTuple, not a Tuple.
        let Expr::StructType(fields, _) = parse_ok("struct { (Int, Type) }") else {
            panic!("expected a struct type")
        };
        assert!(matches!(fields[0], Expr::TypeTuple(..)));
    }

    #[test]
    fn struct_type_errors_carry_spans() {
        let err = parse_err("struct");
        assert_eq!(err.message, "expected '{', found the end of the program");
        let err = parse_err("struct {");
        assert_eq!(err.message, "expected an expression, found the end of the program");
        let err = parse_err("struct { }");
        assert_eq!(err.message, "expected an expression, found '}'");
        let err = parse_err("struct { Int");
        assert_eq!(err.message, "expected '}', found the end of the program");
    }

    #[test]
    fn the_angle_bracket_array_type() {
        // Int<3> — an array type.
        let Expr::TypeArray { element_type, length, .. } = parse_ok("Int<3>") else {
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
        assert_eq!(err.message, "expected an expression, found the end of the program");
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
}
