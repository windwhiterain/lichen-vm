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

use crate::lsp::{self, Diagnostic, DiagnosticSeverity, Position, Range};

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

        Doc {
            source,
            line_starts,
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
}
