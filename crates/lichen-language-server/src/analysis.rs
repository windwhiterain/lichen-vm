//! [`Doc`]: one source parsed once, held as the shared frontend artifacts plus
//! an editor-view index built on top of them.
//!
//! `Doc` reuses the *actual* frontend artifacts from `lichen-language` — the
//! tokens (byte ranges), the AST (source spans) — and the *checker* via
//! [`BufferSession`] for the full diagnostic set. It then adds the one thing the
//! raw frontend does not give you: name resolution for editing.
//!
//! Why the resolution is re-derived here: `compile` resolves names at lowering
//! but collapses a name *use* onto the binder's `ExprId`, so the IR never
//! records the use's own span. The AST keeps the use's span but not its
//! binding. So [`Doc`] walks the AST with its own scope stack (mirroring the
//! compiler's scope rules) and records a use-span → definition map. This is
//! exactly what the tooling layer is *for*: interpreting the shared syntax
//! artifact for an editor, without forking the compiler.

use std::collections::HashMap;

use lichen_highlevel::ir::Span;
use lichen_highlevel::no_native_ops;
use lichen_language::ast::{Expr, Program, Stmt};
use lichen_language::diag::{Diag, Stage};
use lichen_language::lex;
use lichen_language::lex::{Token, TokenKind};
use lichen_language::package::PackageStore;
use lichen_language::parse;
use lichen_language::preprocess;
use lichen_language::{build_report, frontend_at};

use crate::lsp::{
    self, Diagnostic, DiagnosticSeverity, Position, Range, SemanticTokenData, SemanticTokenModifier,
    SemanticTokens, SemanticTokenType,
};

/// A definition site: a binding name or a lambda parameter.
#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub span: Span,
}

/// A name *use* and (when it resolves) the [`Definition`] it points to.
#[derive(Clone, Debug)]
pub struct Reference {
    pub name: String,
    pub span: Span,
    pub definition: Option<usize>,
}

/// A parsed + checked source, ready for editor lookups.
pub struct Doc {
    /// The full source text.
    pub source: String,
    /// Byte offset at which each line begins (line 1 = 0).
    pub line_starts: Vec<usize>,
    /// Byte offset where the compiled code begins within `source` — past the
    /// leading `@{…@}` preprocessor block (0 when there is no block).  Everything
    /// before it is the preprocessor/metadata block.
    pub code_base: u32,
    /// The token stream (with byte ranges) — the frontend's lexer output.
    pub tokens: Vec<Token>,
    /// The parsed AST — the frontend's parser output.
    pub program: Program,
    /// The full diagnostic set (lex + parse + resolve + check).
    pub diagnostics: Vec<Diag>,
    /// Every definition site, in declaration order.
    pub defs: Vec<Definition>,
    /// Span of a name *use* → index into [`Doc::defs`].
    resolve: HashMap<Span, usize>,
    /// Span of a definition site → index into [`Doc::defs`].
    def_index: HashMap<Span, usize>,
}

impl Doc {
    /// Parse, lower and check `source`, keeping the frontend artifacts and
    /// indexing name resolution.  The leading `@{…@}` preprocessor block (with
    /// its `import`/metadata directives) is cut out and resolved first, so a
    /// real lichen file compiles on the code that follows the block, with spans
    /// still absolute in the original file.
    pub fn new(source: impl Into<String>) -> Doc {
        let source = source.into();
        let line_starts = lex::line_starts(&source);

        // Cut the leading `@{…@}` block (metadata + imports) off the frontend
        // input, resolving imports through a fresh in-memory package store so
        // the shared registry can serve any loaded imports.
        let mut store = PackageStore::new();
        let (pre, mut diagnostics) = preprocess::preprocess(&source, None, &mut store);

        // The frontend artifacts (for the editor index): tokens + AST in
        // absolute file coordinates (the lexer maps through `code_base` and the
        // full line starts).
        let lexed = lex::lex_with(pre.code, &line_starts, pre.code_base);
        let tokens = lexed.tokens;
        let parsed = parse::parse(&tokens);
        let program = parsed.program;

        // The full pipeline diagnostics (lex + parse + resolve + check):
        // preprocess first, then the frontend over the preprocessed code, then
        // the checker over the IR.  All spans are absolute.
        let frontend = frontend_at(pre.code, pre.code_base, &line_starts, &pre.imports);
        diagnostics.extend(frontend.diagnostics);
        let report = build_report(frontend.ir, diagnostics, Some(store.registry()), no_native_ops());
        let diagnostics = report.diagnostics;

        let (defs, resolve, def_index) = index(&program);
        // `pre.code` borrows `source`, so copy the offset out before moving
        // `source` into the result.
        let code_base = pre.code_base;

        Doc {
            source,
            line_starts,
            code_base,
            tokens,
            program,
            diagnostics,
            defs,
            resolve,
            def_index,
        }
    }

    /// The diagnostics as LSP [`Diagnostic`]s.
    pub fn lsp_diagnostics(&self) -> Vec<Diagnostic> {
        self.diagnostics
            .iter()
            .map(|d| {
                let range = d
                    .span
                    .map(|s| lsp::range_from_span(&self.source, &self.line_starts, s))
                    .unwrap_or_else(|| Range {
                        start: Position { line: 0, character: 0 },
                        end: Position { line: 0, character: 0 },
                    });
                Diagnostic {
                    range,
                    severity: Some(severity_for(d.stage)),
                    code: None,
                    code_description: None,
                    source: Some("lichen".to_string()),
                    message: d.message.clone(),
                    tags: None,
                    related_information: None,
                    data: None,
                }
            })
            .collect()
    }

    /// The token at byte offset `offset`, if any.
    fn token_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.iter().find(|t| (t.range.0 as usize) <= offset && offset < (t.range.1 as usize))
    }

    fn offset_of(&self, position: Position) -> Option<usize> {
        lsp::offset_from_position(&self.source, &self.line_starts, position)
    }

    /// Hover at a cursor position: the token under it, and — for a name — the
    /// definition it resolves to (or that it *is*).
    pub fn hover_at(&self, position: Position) -> Option<(String, Range)> {
        let offset = self.offset_of(position)?;
        let token = self.token_at(offset)?;
        let range = lsp::range_from_span(&self.source, &self.line_starts, token.span);
        let kind = &token.kind;
        if let TokenKind::Name(name) = kind {
            let def = self
                .resolve
                .get(&token.span)
                .or_else(|| self.def_index.get(&token.span))
                .and_then(|i| self.defs.get(*i));
            let msg = match def {
                Some(d) => format!("`{name}` — defined at line `{}`", d.span.0),
                None => format!("`{name}` — unresolved name"),
            };
            return Some((msg, range));
        }
        // A keyword, literal, or operator: describe it.
        let msg = format!("`{}`", kind.describe());
        Some((msg, range))
    }

    /// Go to definition for a cursor position on a name *use*: the definition's
    /// byte range, if it resolves.
    pub fn definition_at(&self, position: Position) -> Option<Range> {
        let offset = self.offset_of(position)?;
        let token = self.token_at(offset)?;
        if let TokenKind::Name(_) = &token.kind {
            if let Some(idx) = self.resolve.get(&token.span) {
                let def = &self.defs[*idx];
                return Some(lsp::range_from_span(&self.source, &self.line_starts, def.span));
            }
        }
        None
    }

    /// Classify each source token into an LSP semantic token, driven by Lichen's
    /// own frontend (not a tree-sitter grammar): literals, keywords, operators,
    /// and — via the AST — names as declarations / parameters / function calls /
    /// struct fields.  The leading `@{…@}` preprocessor block (if any) is
    /// highlighted as one comment span (it is the language's only prose-like
    /// construct).  This is the `grammar-optional` highlighting path.
    pub fn semantic_tokens(&self) -> Vec<SemanticTokenData> {
        let mut out = Vec::new();

        // The preprocessor/metadata block (bytes `0..code_base`) is the only
        // "comment"-like construct; color it as a comment, split per line so no
        // single token spans a newline (clients require single-line tokens).
        if self.code_base > 0 {
            let end = self.code_base as usize;
            for (i, &ls) in self.line_starts.iter().enumerate() {
                if ls >= end {
                    break;
                }
                let line_end = self
                    .line_starts
                    .get(i + 1)
                    .copied()
                    .unwrap_or(self.source.len())
                    .min(end);
                if line_end > ls {
                    out.push(SemanticTokenData {
                        start: ls as u32,
                        end: line_end as u32,
                        token_type: SemanticTokenType::COMMENT,
                        modifiers: Vec::new(),
                    });
                }
            }
        }

        // Name classifications the AST is needed to infer (a bare name is
        // VARIABLE by default): a lambda parameter, or a name used in function
        // position.
        let name_class = classify_names(&self.program);

        // Walk the token stream; tokens are already in document order.
        let mut prev_was_dot = false;
        for t in &self.tokens {
            let ty_kind = classify_token_kind(&t.kind);
            let (token_type, modifiers) = match &t.kind {
                TokenKind::Name(_) if prev_was_dot => {
                    // `a.name`, `struct<.name T>`, `C(.name v)` — a named field.
                    (SemanticTokenType::PROPERTY, Vec::new())
                }
                TokenKind::Name(_) => name_class
                    .get(&t.span)
                    .cloned()
                    .unwrap_or((SemanticTokenType::VARIABLE, Vec::new())),
                _ => match ty_kind {
                    Some(k) => k,
                    // Delimiters, separators, Glue and Eof have no semantic color.
                    None => {
                        prev_was_dot = t.kind == TokenKind::Dot;
                        continue;
                    }
                },
            };
            out.push(SemanticTokenData {
                start: t.range.0,
                end: t.range.1,
                token_type,
                modifiers,
            });
            prev_was_dot = t.kind == TokenKind::Dot;
        }

        out
    }

    /// The semantic tokens delta-encoded for the LSP client (the endpoint that
    /// serves `textDocument/semanticTokens/full`).
    pub fn semantic_tokens_lsp(&self) -> SemanticTokens {
        lsp::encode_semantic_tokens(&self.source, &self.line_starts, &self.semantic_tokens())
    }
}

fn severity_for(stage: Stage) -> DiagnosticSeverity {
    // The frontend/checker report only errors; keep the mapping explicit so a
    // future warning stage slots in.
    match stage {
        Stage::Preprocess | Stage::Lex | Stage::Parse | Stage::Resolve | Stage::Check => {
            DiagnosticSeverity::ERROR
        }
    }
}

// ---------------------------------------------------------------------------
// Semantic-token classification.
//
// `classify_token_kind` maps the token-level kinds (literals / keywords /
// operators) that need no AST context. `classify_names` records the *name* spans
// the AST is required to disambiguate — a lambda parameter, or a name used in
// function position — so `Doc::semantic_tokens` can color a `Name` as more than
// the default `VARIABLE`, and so the same frontend drives highlighting with no
// tree-sitter grammar in play.

fn classify_token_kind(kind: &TokenKind) -> Option<(SemanticTokenType, Vec<SemanticTokenModifier>)> {
    match kind {
        TokenKind::Int(_) => Some((SemanticTokenType::NUMBER, Vec::new())),
        TokenKind::Str(_) => Some((SemanticTokenType::STRING, Vec::new())),
        // The builtin type constants are type-ish, not keywords.
        TokenKind::KwInt | TokenKind::KwString | TokenKind::KwType => {
            Some((SemanticTokenType::TYPE, Vec::new()))
        }
        TokenKind::KwStruct
        | TokenKind::KwTable
        | TokenKind::KwLet
        | TokenKind::KwIf
        | TokenKind::KwThen
        | TokenKind::KwElse
        | TokenKind::KwReturn
        | TokenKind::KwPub => Some((SemanticTokenType::KEYWORD, Vec::new())),
        // A `Name` is resolved by `classify_names` (or the `.` heuristic).
        TokenKind::Name(_) => None,
        // Operators: arrows, annotations, separators-of-fields, and math.
        TokenKind::Arrow
        | TokenKind::FatArrow
        | TokenKind::Colon
        | TokenKind::DoubleColon
        | TokenKind::Hash
        | TokenKind::Bang
        | TokenKind::Dollar
        | TokenKind::Equals
        | TokenKind::Eq
        | TokenKind::Leq
        | TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Dot
        | TokenKind::Tilde(_) => Some((SemanticTokenType::OPERATOR, Vec::new())),
        // Delimiters, separators, Glue and Eof carry no semantic color.
        TokenKind::LParen
        | TokenKind::RParen
        | TokenKind::LBracket
        | TokenKind::RBracket
        | TokenKind::LBrace
        | TokenKind::RBrace
        | TokenKind::LAngle
        | TokenKind::RAngle
        | TokenKind::Separator
        | TokenKind::Glue
        | TokenKind::Eof => None,
    }
}

fn classify_names(
    program: &Program,
) -> HashMap<Span, (SemanticTokenType, Vec<SemanticTokenModifier>)> {
    let mut map = HashMap::new();
    let mut w = NameClass { map: &mut map };
    w.stmts(&program.statements);
    w.expr(&program.expr);
    map
}

struct NameClass<'a> {
    map: &'a mut HashMap<Span, (SemanticTokenType, Vec<SemanticTokenModifier>)>,
}

impl<'a> NameClass<'a> {
    fn stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Binding(b) => {
                // A binding definition is a VARIABLE declaration; the value is
                // in scope (and may be a lambda / application).
                self.map
                    .insert(b.span, (SemanticTokenType::VARIABLE, vec![SemanticTokenModifier::DECLARATION]));
                self.expr(&b.value);
            }
            Stmt::Expr(e) => self.expr(e),
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Lambda {
                parameter_span,
                parameter_type,
                parameter_perspective,
                r#return,
                ..
            } => {
                self.map.insert(
                    *parameter_span,
                    (SemanticTokenType::PARAMETER, vec![SemanticTokenModifier::DECLARATION]),
                );
                if let Some(t) = parameter_type {
                    self.expr(t);
                }
                if let Some(p) = parameter_perspective {
                    self.expr(p);
                }
                self.expr(r#return);
            }
            Expr::Apply { function, argument, .. } => {
                // A plain name in function position is a function call.
                if let Expr::Name(_, span) = &**function {
                    self.map.insert(*span, (SemanticTokenType::FUNCTION, Vec::new()));
                }
                self.expr(function);
                self.expr(argument);
            }
            Expr::Int(..) | Expr::Str(..) | Expr::TypeConst(..) | Expr::Name(..) | Expr::Placeholder(..) | Expr::Err { .. } => {}
            Expr::BinOp { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.expr(condition);
                self.expr(then_branch);
                self.expr(else_branch);
            }
            Expr::Assert { value, .. } => self.expr(value),
            Expr::NativeCall { args, .. } => {
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Index { array, index, .. } => {
                self.expr(array);
                self.expr(index);
            }
            Expr::FieldRead { container, key, .. } => {
                self.expr(container);
                self.expr(key);
            }
            Expr::NamedFieldRead { container, .. } => self.expr(container),
            Expr::TableFind { container, key, .. } => {
                self.expr(container);
                self.expr(key);
            }
            Expr::Annotation {
                value,
                r#type,
                perspective,
                ..
            } => {
                self.expr(value);
                if let Some(t) = r#type {
                    self.expr(t);
                }
                if let Some(p) = perspective {
                    self.expr(p);
                }
            }
            Expr::Arrow { parameter, r#return, .. } => {
                self.expr(parameter);
                self.expr(r#return);
            }
            Expr::Tuple(elems, _) | Expr::TypeTuple(elems, _) | Expr::Array(elems, _) => {
                for el in elems {
                    self.expr(el);
                }
            }
            Expr::StructType(fields, _) => {
                for f in fields {
                    self.expr(&f.ty);
                }
            }
            Expr::StructInst { callee, fields, .. } => {
                self.expr(callee);
                for f in fields {
                    self.expr(&f.value);
                }
            }
            Expr::Table(entries, _) => {
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            Expr::Shallow(inner, _, _) => self.expr(inner),
            Expr::TypeArray {
                element_type,
                length,
                ..
            } => {
                self.expr(element_type);
                self.expr(length);
            }
            Expr::Block { statements, expr, .. } => {
                self.stmts(statements);
                self.expr(expr);
            }
            Expr::RecordBlock { fields, .. } => {
                for f in fields {
                    self.expr(&f.value);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Name resolution over the AST, mirroring the compiler's scope rules.
//
// A scope's block-wide (non-`let`) bindings are pre-entered before any value so
// they may forward-/mutually-reference; a restrictive `let` enters after its
// value (a fresh frame); a lambda enters its parameter for the body; a block
// pushes/pops its own frame.

fn index(program: &Program) -> (Vec<Definition>, HashMap<Span, usize>, HashMap<Span, usize>) {
    let mut walk = Walk {
        defs: Vec::new(),
        scopes: Vec::new(),
        resolve: HashMap::new(),
        def_index: HashMap::new(),
    };
    walk.scope(&program.statements, Some(&program.expr));
    (walk.defs, walk.resolve, walk.def_index)
}

struct Walk {
    defs: Vec<Definition>,
    scopes: Vec<HashMap<String, usize>>,
    resolve: HashMap<Span, usize>,
    def_index: HashMap<Span, usize>,
}

impl Walk {
    fn enter(&mut self, name: &str, span: Span) -> usize {
        let idx = self.defs.len();
        self.defs.push(Definition {
            name: name.to_string(),
            span,
        });
        self.def_index.insert(span, idx);
        self.scopes.last_mut().expect("a scope frame is pushed").insert(name.to_string(), idx);
        idx
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes.iter().rev().find_map(|f| f.get(name).copied())
    }

    fn scope(&mut self, statements: &[Stmt], expr: Option<&Expr>) {
        let base = self.scopes.len();
        self.scopes.push(HashMap::new());
        for stmt in statements {
            if let Stmt::Binding(b) = stmt
                && !b.restrictive
            {
                self.enter(&b.name, b.span);
            }
        }
        for stmt in statements {
            self.stmt(stmt);
        }
        if let Some(e) = expr {
            self.expr(e);
        }
        self.scopes.truncate(base);
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Binding(b) if b.restrictive => {
                // `let name = e`: the value is in scope [outer]; the name is
                // entered only after it, for later statements.
                self.expr(&b.value);
                self.scopes.push(HashMap::new());
                self.enter(&b.name, b.span);
            }
            Stmt::Binding(b) => self.expr(&b.value),
            Stmt::Expr(e) => self.expr(e),
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Int(..)
            | Expr::Str(..)
            | Expr::TypeConst(..)
            | Expr::Placeholder(..)
            | Expr::Err { .. } => {}
            Expr::Name(name, span) => {
                if let Some(idx) = self.lookup(name) {
                    self.resolve.insert(*span, idx);
                }
            }
            Expr::Lambda {
                parameter,
                parameter_span,
                parameter_type,
                parameter_perspective,
                r#return,
                ..
            } => {
                let base = self.scopes.len();
                self.scopes.push(HashMap::new());
                self.enter(parameter, *parameter_span);
                if let Some(t) = parameter_type {
                    self.expr(t);
                }
                if let Some(p) = parameter_perspective {
                    self.expr(p);
                }
                self.expr(r#return);
                self.scopes.truncate(base);
            }
            Expr::Apply { function, argument, .. } => {
                self.expr(function);
                self.expr(argument);
            }
            Expr::BinOp { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                self.expr(condition);
                self.expr(then_branch);
                self.expr(else_branch);
            }
            Expr::Assert { value, .. } => self.expr(value),
            Expr::NativeCall { args, .. } => {
                for a in args {
                    self.expr(a);
                }
            }
            Expr::Index { array, index, .. } => {
                self.expr(array);
                self.expr(index);
            }
            Expr::FieldRead { container, key, .. } => {
                self.expr(container);
                self.expr(key);
            }
            Expr::NamedFieldRead { container, .. } => self.expr(container),
            Expr::TableFind { container, key, .. } => {
                self.expr(container);
                self.expr(key);
            }
            Expr::Annotation {
                value,
                r#type,
                perspective,
                ..
            } => {
                self.expr(value);
                if let Some(t) = r#type {
                    self.expr(t);
                }
                if let Some(p) = perspective {
                    self.expr(p);
                }
            }
            Expr::Arrow { parameter, r#return, .. } => {
                self.expr(parameter);
                self.expr(r#return);
            }
            Expr::Tuple(elems, _) | Expr::TypeTuple(elems, _) | Expr::Array(elems, _) => {
                for el in elems {
                    self.expr(el);
                }
            }
            Expr::StructType(fields, _) => {
                for field in fields {
                    self.expr(&field.ty);
                }
            }
            Expr::StructInst { callee, fields, .. } => {
                self.expr(callee);
                for f in fields {
                    self.expr(&f.value);
                }
            }
            Expr::Table(entries, _) => {
                for (k, v) in entries {
                    self.expr(k);
                    self.expr(v);
                }
            }
            Expr::Shallow(inner, _, _) => self.expr(inner),
            Expr::TypeArray {
                element_type,
                length,
                ..
            } => {
                self.expr(element_type);
                self.expr(length);
            }
            Expr::Block { statements, expr, .. } => self.scope(statements, Some(expr)),
            Expr::RecordBlock { fields, .. } => {
                for f in fields {
                    self.expr(&f.value);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::Position;

    fn doc(source: &str) -> Doc {
        Doc::new(source)
    }

    #[test]
    fn unresolved_name_is_reported() {
        let d = doc("a = 1\nb = unknown\nb");
        let msgs: Vec<String> = d
            .lsp_diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        assert!(
            msgs.iter().any(|m| m.contains("unresolved name 'unknown'")),
            "expected an unresolved-name diagnostic, got {msgs:?}"
        );
    }

    #[test]
    fn clean_source_has_no_diagnostics_and_one_binding() {
        let d = doc("a = 1\n(a, a)");
        assert!(d.diagnostics.is_empty(), "got {:?}", d.diagnostics);
        assert_eq!(d.defs.len(), 1, "one binding");
    }

    #[test]
    fn preprocessor_block_is_cut_out() {
        // A real lichen file opens with an `@{…@}` metadata block; it must not
        // leak into the lexer/parser as code, and the file after it compiles.
        let d = doc("@{ order = \"1\"\noutput = \"3: Int\"\n@}\na = 1\nb = 2\na + b\n");
        assert!(d.diagnostics.is_empty(), "got {:?}", d.diagnostics);
        assert_eq!(d.defs.len(), 2, "two bindings (a, b)");
    }

    #[test]
    fn hover_resolves_a_use_to_its_binding() {
        let d = doc("a = 1\nb = a + 1\nb");
        let (msg, _range) = d.hover_at(Position { line: 1, character: 4 }).expect("hover on `a`");
        assert!(msg.contains("defined at line `1`"), "hover msg = {msg}");
    }

    #[test]
    fn hover_on_unresolved_name_says_so() {
        let d = doc("a = 1\nb = unknown");
        let (msg, _) = d.hover_at(Position { line: 1, character: 4 }).expect("hover on `unknown`");
        assert!(msg.contains("unresolved name"), "hover msg = {msg}");
    }

    #[test]
    fn definition_jumps_to_the_binding() {
        let d = doc("a = 1\nb = a + 1\nb");
        let range = d
            .definition_at(Position { line: 2, character: 0 })
            .expect("definition for the final `b`");
        assert_eq!(range.start, Position { line: 1, character: 0 });
    }

    #[test]
    fn definition_is_none_on_a_non_name() {
        let d = doc("a = 1\na + 1\n");
        // Cursor on the `1` literal in `a + 1` (line index 1, char 4).
        assert!(d.definition_at(Position { line: 1, character: 4 }).is_none());
    }

    #[test]
    fn semantic_tokens_cover_the_source_and_stay_in_bounds() {
        let d = doc("a = 1\nb = 2\na + b\n");
        let toks = d.semantic_tokens();
        assert!(!toks.is_empty(), "expected some semantic tokens");
        for t in &toks {
            assert!(t.end >= t.start, "token range is ordered {:?}", (t.start, t.end));
            assert!(t.end as usize <= d.source.len(), "token end in bounds");
        }
    }

    #[test]
    fn semantic_tokens_classify_literals_operators_and_declarations() {
        let d = doc("f = x => x + 1\nf 7\n");
        let toks = d.semantic_tokens();
        let types = |t: &crate::lsp::SemanticTokenType| toks.iter().any(|x| &x.token_type == t);
        assert!(types(&crate::lsp::SemanticTokenType::NUMBER), "a literal is a number");
        assert!(types(&crate::lsp::SemanticTokenType::OPERATOR), "an operator is colored");
        // The binding `f` is a declaration.
        assert!(
            toks.iter().any(|t| t.token_type == crate::lsp::SemanticTokenType::VARIABLE
                && t.modifiers.contains(&crate::lsp::SemanticTokenModifier::DECLARATION)),
            "binding definition is a declared variable"
        );
        // The lambda parameter `x` is a parameter declaration.
        assert!(
            toks.iter().any(|t| t.token_type == crate::lsp::SemanticTokenType::PARAMETER
                && t.modifiers.contains(&crate::lsp::SemanticTokenModifier::DECLARATION)),
            "lambda parameter is a declared parameter"
        );
        // The `f` use in `f 7` is a function call.
        assert!(
            toks.iter().any(|t| t.token_type == crate::lsp::SemanticTokenType::FUNCTION),
            "a name in function position is a function"
        );
    }

    #[test]
    fn semantic_tokens_comment_the_preprocess_block() {
        let d = doc("@{ order = \"1\"\noutput = \"3: Int\"\n@}\na = 1\nb = 2\na + b\n");
        let toks = d.semantic_tokens();
        let comments: Vec<_> = toks
            .iter()
            .filter(|t| t.token_type == crate::lsp::SemanticTokenType::COMMENT)
            .collect();
        assert!(!comments.is_empty(), "expected the preprocess block as a comment");
        for t in &comments {
            assert!(t.end as usize <= d.code_base as usize, "comment stays before the code");
        }
        // The compiled code region is still classified.
        assert!(
            toks.iter().any(|t| t.token_type == crate::lsp::SemanticTokenType::NUMBER),
            "code numbers are classified past the block"
        );
    }
}
