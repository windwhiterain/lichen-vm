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
use std::path::Path;

use lichen_highlevel::ir::ExprId;
use lichen_highlevel::no_native_ops;
use lichen_language::ast::{Expr, Program, Stmt};
use lichen_language::diag::{Diag, Stage};
use lichen_language::lex;
use lichen_language::lex::Span;
use lichen_language::lex::{Token, TokenKind};
use lichen_language::package::PackageStore;
use lichen_language::parse;
use lichen_language::preprocess;
use lichen_language::preprocess::ResolvedImport;
use lichen_language::program::LangValue;
use lichen_language::render::{print_type_lang, print_value_lang};
use lichen_language::{build_report, frontend_at};
use lichen_lowlevel::{AnyNodeId, LowValue};

use crate::lsp::{
    self, Diagnostic, DiagnosticSeverity, Position, Range, SemanticTokenData,
    SemanticTokenModifier, SemanticTokenType, SemanticTokens,
};

/// A definition site: a binding name or a lambda parameter.
#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub span: Span,
}

/// An imported binding, indexed for the editor: the name it is bound to, the
/// site of its `@import` directive (a definition in this file), the import
/// path, and the imported module's rendered type (for hover).  A use of the
/// name resolves to this, so hovering an imported module (or a field of it) is
/// not "unresolved".
#[derive(Clone, Debug)]
struct ImportBinding {
    /// The binding name the import is available under (`math`).
    name: String,
    /// The (start) span of the `@import` directive in the original file — the
    /// definition site a use of the imported name resolves to.
    span: Span,
    /// The import path (`math.lichen`), for a descriptive hover.
    path: String,
    /// The imported module's rendered type (its export's type), when the build
    /// computed one.
    ty: Option<String>,
}

/// The checked type and — when the build produced a concrete one — the value
/// of one top-level (outer-block) statement.  A **read-only snapshot** taken at
/// [`Doc::new`] time: the build's cascade deep pass already type-checked every
/// statement and computed (or deliberately left lazy) each one's value.  The
/// snapshot is taken by *reading* `build.ty`/`build.val`/`module.node_value`;
/// it never re-evaluates a node, never forces a lazy cell, and never calls
/// `evaluate_node`/`evaluate_node_deep`.  A lazy or recursive binding whose
/// value the program defers (a `Parameterized` cell, e.g. `paradox` in the
/// `Type : Type` encoding) reports `value: None` and its type only — forcing
/// it would run the compiler-generated recursion clones, which are not
/// user-written and are irrelevant to editor needs.
#[derive(Clone, Debug)]
pub struct StatementValue {
    /// The statement's source span (its start position, 1-based `(line, col)`).
    pub span: Span,
    /// The statement's checked type, rendered in lichen's type syntax.
    pub ty: String,
    /// The statement's value, rendered when the cascade computed a concrete
    /// one; `None` when the statement is lazy/recursive (its value is a
    /// deferred `Parameterized` cell) or has no value node.
    pub value: Option<String>,
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
    /// Span of a binding name → index into [`Doc::statements`] for the
    /// statement that defines it.  A name — the binding's own name or any use
    /// of it — resolves to this statement's value/type for the hover.
    stmt_by_span: HashMap<Span, usize>,
    /// Per top-level statement (source order): its checked type and, when
    /// concrete, its value.  See [`StatementValue`] for the read-only contract.
    statements: Vec<StatementValue>,
    /// The byte offset of each statement's start, source order (the span index
    /// backing [`Doc::statement_at`]).
    stmt_starts: Vec<u32>,
    /// The imported bindings this file resolves, source order — so a use of an
    /// imported module resolves to its `@import` directive and hovers as the
    /// imported module (with its type).
    imports: Vec<ImportBinding>,
    /// Import-directive span → index into [`Doc::imports`].
    import_by_span: HashMap<Span, usize>,
    /// Per struct-block field (an exported struct's `succ`/`add`), keyed by
    /// field name: the field value's checked `value : type` snapshot.  Field
    /// names are not IR nodes, so to hover a field *definition* with its type
    /// (`succ` → `Function : Int -> Int`) we read it from the `Record` node's
    /// field-value tuple.
    field_types: HashMap<String, StatementValue>,
    /// Per field access on an imported module (`math.succ`), keyed by
    /// `(import binding name, field name)`: the accessed field checked
    /// `value : type` snapshot.  Read from each `NamedField` IR node whose
    /// container is the module's imported `Static`, so hovering a field
    /// *access* renders the field's value:type too (not just "field of module").
    module_field_types: HashMap<(String, String), StatementValue>,
}

impl Doc {
    /// Parse, lower and check `source`, keeping the frontend artifacts and
    /// indexing name resolution.  The leading `@{…@}` preprocessor block (with
    /// its `import`/metadata directives) is cut out and resolved first, so a
    /// real lichen file compiles on the code that follows the block, with spans
    /// still absolute in the original file.
    ///
    /// Imports resolve against the current directory (`base = None`); use
    /// [`Doc::new_with_base`] for a file whose `@import` lines should resolve
    /// relative to the file (the LSP server's case).
    pub fn new(source: impl Into<String>) -> Doc {
        Doc::new_with_base(source, None)
    }

    /// [`Doc::new`] with an explicit base path for import resolution.  `base` is
    /// the source file's path (or a directory): `@import` paths resolve relative
    /// to it — a relative import `"math.lichen"` in `dir/main.lichen` loads
    /// `dir/math.lichen`.  `None` keeps the pre-LSP behavior (relative to the
    /// current directory).
    pub fn new_with_base(source: impl Into<String>, base: Option<&Path>) -> Doc {
        let source = source.into();
        let line_starts = lex::line_starts(&source);

        // Cut the leading `@{…@}` block (metadata + imports) off the frontend
        // input, resolving imports through a fresh in-memory package store so
        // the shared registry can serve any loaded imports.  `base` lets the
        // store resolve relative `@import` paths against the file's directory.
        let mut store = PackageStore::new();
        let (pre, mut diagnostics) = preprocess::preprocess(&source, base, &mut store);

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
        let report = build_report(
            frontend.ir,
            Some(frontend.span_index),
            diagnostics,
            Some(store.registry()),
            no_native_ops(),
        );
        let diagnostics = report.diagnostics;
        // The frontend's `ExprId → span` index (highlevel is span-free).
        let span_index = report.span_index;

        // The imported module's checked type, per `@import` directive span: the
        // compiler allocates a `Static` node at the directive's span, so we
        // find it in the IR and read its type for the hover.  Borrowed here so
        // `report.build` is still owned by the match below.
        let mut import_ty: HashMap<Span, String> = HashMap::new();
        if let Some(build) = &report.build {
            for imp in &pre.imports {
                let eid = span_index.as_ref().and_then(|s| {
                    s.iter().enumerate().find_map(|(i, sp)| {
                        (sp == &Some(imp.span)
                            && matches!(
                                build.ir.expr[i].kind,
                                lichen_highlevel::ir::ExprKind::Static { .. }
                            ))
                        .then_some(ExprId(i as u32))
                    })
                });
                if let Some(eid) = eid
                    && let Some(t) = build.ty[eid]
                {
                    import_ty.insert(imp.span, print_type_lang(&build.module, t));
                }
            }
        }

        // Directive span → import binding name, so a field access's container
        // (an imported module's `Static` node) can be traced back to its module.
        let import_name_by_span: HashMap<Span, &str> = pre
            .imports
            .iter()
            .map(|i| (i.span, i.name.as_str()))
            .collect();

        // A read-only per-statement type/value snapshot.  It is computed here,
        // once, by reading the built module's cached values — never by
        // re-evaluating a node or forcing a lazy cell (see [`StatementValue`]).
        let (statements, stmt_starts, field_types, module_field_types) = match report.build {
            Some(build) => {
                let mut statements = Vec::new();
                let mut starts = Vec::new();
                for (i, &id) in build.ir.stmt_roots.iter().enumerate() {
                    // The IR statement's own span points at its *value*
                    // expression; for the span index use the AST statement's
                    // start (the binding name / bare-expression start), which
                    // is where the statement begins in the source.  Statements
                    // align 1:1 with `program.statements` in source order.
                    let (span, start) = match program.statements.get(i) {
                        Some(Stmt::Binding(b)) => {
                            (b.span, lsp::offset_of_span(&line_starts, b.span))
                        }
                        Some(Stmt::Expr(e)) => {
                            let s = e.span();
                            (s, lsp::offset_of_span(&line_starts, s))
                        }
                        None => {
                            let s = span_index
                                .as_ref()
                                .and_then(|idx| idx.get(id.0 as usize).copied().flatten())
                                .unwrap_or((0, 0));
                            (s, lsp::offset_of_span(&line_starts, s))
                        }
                    };
                    let ty = match build.ty[id] {
                        Some(t) => print_type_lang(&build.module, t),
                        None => String::new(),
                    };
                    let value = build.val[id].and_then(|vn| {
                        match build.module.node_value(AnyNodeId::Dynamic(vn)) {
                            // A `Parameterized` value is a deferred (lazy /
                            // recursive) binding — report type only, never force.
                            Some(LangValue::LowValue(LowValue::Parameterized)) => None,
                            Some(v) => Some(print_value_lang(
                                &build.module,
                                v,
                                build.ty[id].unwrap_or_default(),
                            )),
                            None => None,
                        }
                    });
                    statements.push(StatementValue { span, ty, value });
                    starts.push(start as u32);
                }
                // A struct-block field's value:type snapshot, keyed by field
                // name.  A `RecordBlock` emits a `Record` node whose value is a
                // tuple of the field values (parallel to its `struct_names`),
                // so we read each field's value/type from the checker.  Field
                // names are not IR nodes, hence this name-keyed table — it is
                // what lets hovering a field *definition* (`succ` in
                // `{succ = x => x + 1}`) render `Function : Int -> Int`.
                let mut field_types: HashMap<String, StatementValue> = HashMap::new();
                for id in 0..build.ir.expr.len() {
                    let eid = ExprId(id as u32);
                    let lichen_highlevel::ir::ExprKind::Record { value, names } =
                        build.ir[eid].kind
                    else {
                        continue;
                    };
                    let lichen_highlevel::ir::ExprKind::Tuple(tuple_range) = build.ir[value].kind
                    else {
                        continue;
                    };
                    let vals =
                        &build.ir.children[tuple_range.start as usize..tuple_range.end as usize];
                    let field_names =
                        &build.ir.struct_names[names.start as usize..names.end as usize];
                    for (name, &val_id) in field_names.iter().zip(vals.iter()) {
                        let Some(name) = name else { continue };
                        let ty = match build.ty[val_id] {
                            Some(t) => print_type_lang(&build.module, t),
                            None => String::new(),
                        };
                        let value = build.val[val_id].and_then(|vn| {
                            match build.module.node_value(AnyNodeId::Dynamic(vn)) {
                                Some(LangValue::LowValue(LowValue::Parameterized)) => None,
                                Some(v) => Some(print_value_lang(
                                    &build.module,
                                    v,
                                    build.ty[val_id].unwrap_or_default(),
                                )),
                                None => None,
                            }
                        });
                        field_types.insert(
                            name.to_string(),
                            StatementValue {
                                span: (0, 0),
                                ty,
                                value,
                            },
                        );
                    }
                }
                // A field *access* on an imported module (`math.succ`): the
                // compiler lowers it to a `NamedField` IR node whose container
                // is the module's imported `Static`.  The field's value node
                // resolves (read below), but the field-access node's *type* slot
                // stays a lazy cell (`?a`), so the field's type is read from the
                // module's own rendered `struct<...>` type.  The table is keyed
                // by `(import binding name, field name)`.
                let mut module_field_types: HashMap<(String, String), StatementValue> =
                    HashMap::new();
                for id in 0..build.ir.expr.len() {
                    let eid = ExprId(id as u32);
                    let lichen_highlevel::ir::ExprKind::NamedField { container, name } =
                        build.ir[eid].kind
                    else {
                        continue;
                    };
                    let lichen_highlevel::ir::ExprKind::Static { .. } = build.ir[container].kind
                    else {
                        continue;
                    };
                    let Some(container_span) = span_index
                        .as_ref()
                        .and_then(|idx| idx.get(container.0 as usize).copied().flatten())
                    else {
                        continue;
                    };
                    let Some(&module_name) = import_name_by_span.get(&container_span) else {
                        continue;
                    };
                    let ty = import_ty
                        .get(&container_span)
                        .and_then(|mty| field_type_in_struct(mty, name))
                        .unwrap_or_default();
                    let value = build.val[eid].and_then(|vn| {
                        match build.module.node_value(AnyNodeId::Dynamic(vn)) {
                            Some(LangValue::LowValue(LowValue::Parameterized)) => None,
                            Some(v) => Some(print_value_lang(
                                &build.module,
                                v,
                                build.ty[eid].unwrap_or_default(),
                            )),
                            None => None,
                        }
                    });
                    module_field_types.insert(
                        (module_name.to_string(), name.to_string()),
                        StatementValue {
                            span: (0, 0),
                            ty,
                            value,
                        },
                    );
                }
                (statements, starts, field_types, module_field_types)
            }
            None => (Vec::new(), Vec::new(), HashMap::new(), HashMap::new()),
        };

        let (defs, resolve, def_index) = index(&program, &pre.imports);
        // Map each statement's span (a binding's name span, or an
        // expression's start span) to its statement index, so a name
        // (resolved to a binding) can reach the binding's value/type.
        let stmt_by_span: HashMap<Span, usize> = statements
            .iter()
            .enumerate()
            .map(|(i, s)| (s.span, i))
            .collect();
        // The imported bindings and their type (for the hover).  `imp.path` is
        // the canonical resolved path; display its file name (`math.lichen`).
        let imports: Vec<ImportBinding> = pre
            .imports
            .iter()
            .map(|imp| ImportBinding {
                name: imp.name.clone(),
                span: imp.span,
                path: imp
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| imp.path.display().to_string()),
                ty: import_ty.get(&imp.span).cloned(),
            })
            .collect();
        let import_by_span: HashMap<Span, usize> = imports
            .iter()
            .enumerate()
            .map(|(i, b)| (b.span, i))
            .collect();
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
            stmt_by_span,
            statements,
            stmt_starts,
            imports,
            import_by_span,
            field_types,
            module_field_types,
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
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: 0,
                            character: 0,
                        },
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
        self.tokens
            .iter()
            .find(|t| (t.range.0 as usize) <= offset && offset < (t.range.1 as usize))
    }

    fn offset_of(&self, position: Position) -> Option<usize> {
        lsp::offset_from_position(&self.source, &self.line_starts, position)
    }

    /// Every top-level statement's checked type and (when the build produced a
    /// concrete value) its value, in source order.
    ///
    /// This is a **read-only** snapshot already taken at [`Doc::new`].  It
    /// never re-evaluates an expression, never forces a lazy cell, and never
    /// calls `evaluate_node` / `evaluate_node_deep`: a statement the cascade
    /// left lazy/recursive (a deferred `Parameterized` cell) reports
    /// [`StatementValue::value`] as `None` and its type only.  The statement's
    /// *value* is only present when the build actually computed a concrete one
    /// for that user-written statement (a terminal binding, a literal).
    pub fn statement_values(&self) -> &[StatementValue] {
        &self.statements
    }

    /// The top-level statement whose source range contains byte `offset`, if
    /// any.  Statements are disjoint and stored in source order, so the
    /// containing statement is the last one whose start is not past `offset`.
    pub fn statement_at(&self, offset: usize) -> Option<&StatementValue> {
        let idx = self
            .stmt_starts
            .partition_point(|&s| (s as usize) <= offset);
        if idx == 0 {
            None
        } else {
            self.statements.get(idx - 1)
        }
    }

    /// Hover at a cursor position: the token under it, and — for a name — the
    /// definition it resolves to (or that it *is*).  For a *top-level binding*
    /// the hover renders the bound expression's `value : type` from the
    /// checked snapshot (e.g. `` `a` — `1 : Int` `` for `a = 1`); a definition
    /// that is not a binding (a lambda parameter) and an unresolved name report
    /// the definition's line / the unresolved-name message.
    pub fn hover_at(&self, position: Position) -> Option<(String, Range)> {
        let offset = self.offset_of(position)?;
        let token = self.token_at(offset)?;
        let range = lsp::range_from_span(&self.source, &self.line_starts, token.span);
        let kind = &token.kind;
        if let TokenKind::Name(name) = kind {
            // Resolve the hovered name: a use to its binding, or the binding's
            // own definition site.  Then, if it is a top-level binding, render
            // the bound expression's `value : type` from the checked snapshot
            // (the read-only `StatementValue` for that statement).
            let def_idx = self
                .resolve
                .get(&token.span)
                .or_else(|| self.def_index.get(&token.span))
                .copied();
            let msg = match def_idx {
                Some(i) => {
                    let def = &self.defs[i];
                    // An imported module: the use resolves to its `@import`
                    // directive, so render the imported module's type rather
                    // than "defined at line".
                    if let Some(import_i) = self.import_by_span.get(&def.span).copied() {
                        return Some((import_hover(name, &self.imports[import_i]), range));
                    }
                    match self.stmt_by_span.get(&def.span).copied() {
                        Some(stmt_i) => {
                            let sv = &self.statements[stmt_i];
                            snapshot_hover(
                                name,
                                sv,
                                format!("`{name}` — defined at line `{}`", def.span.0),
                            )
                        }
                        // A struct-block field (`succ` in `{succ = …}`): its
                        // value:type from the Record node's field table.
                        None => match self.field_types.get(&def.name) {
                            Some(sv) => snapshot_hover(
                                name,
                                sv,
                                format!("`{name}` — defined at line `{}`", def.span.0),
                            ),
                            None => format!("`{name}` — defined at line `{}`", def.span.0),
                        },
                    }
                }
                None => {
                    // A field access (`math.succ`, `point.x`): the field belongs
                    // to its container — an imported module or a local struct —
                    // so it is not unresolved.  The hover renders the field's
                    // own `value : type`.
                    match self.field_access_hover(name, token.span) {
                        Some(msg) => return Some((msg, range)),
                        None => format!("`{name}` — unresolved name"),
                    }
                }
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
                return Some(lsp::range_from_span(
                    &self.source,
                    &self.line_starts,
                    def.span,
                ));
            }
        }
        None
    }

    /// When the hovered name is a field access (`math.succ`, `point.x`), the
    /// field belongs to its container — an imported module or a local struct
    /// binding — so it is not "unresolved".  Detected by the name being preceded
    /// by a `.` and the container resolving to a knowable struct.  The hover
    /// renders the field's own `value : type` from the checker: an imported
    /// module's field via [`Doc::module_field_types`], a local struct field via
    /// [`Doc::field_types`].
    fn field_access_hover(&self, name: &str, field_span: Span) -> Option<String> {
        let idx = self.tokens.iter().position(|t| t.span == field_span)?;
        if idx < 2 || self.tokens[idx - 1].kind != TokenKind::Dot {
            return None;
        }
        let container_span = match &self.tokens[idx - 2].kind {
            TokenKind::Name(_) => self.tokens[idx - 2].span,
            _ => return None,
        };
        let container_def = self.resolve.get(&container_span).copied();

        // An imported-module container (`math.succ`): look the field up in the
        // module's field table.
        if let Some(d) = container_def
            && let Some(import_i) = self.import_by_span.get(&self.defs[d].span).copied()
        {
            let module = &self.imports[import_i].name;
            let fallback = format!("`.{name}` — field of imported module `{module}`");
            return Some(
                match self
                    .module_field_types
                    .get(&(module.clone(), name.to_string()))
                {
                    Some(sv) => snapshot_hover(&format!(".{name}"), sv, fallback),
                    None => fallback,
                },
            );
        }

        // A local struct-binding container (`point.x`): the field's value:type
        // from this file's struct-block table.
        if let Some(sv) = self.field_types.get(name) {
            return Some(snapshot_hover(
                &format!(".{name}"),
                sv,
                format!("`{name}` — unresolved name"),
            ));
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

/// Render a `value : type` hover snapshot for a resolved definition: a concrete
/// value (`v : ty`), else just the type (a lazy / recursive binding), else the
/// caller's `fallback` (no type was computed).
fn snapshot_hover(display: &str, sv: &StatementValue, fallback: String) -> String {
    match (&sv.value, sv.ty.is_empty()) {
        // A concrete value: `value : type`.
        (Some(v), false) => format!("`{display}` — `{v} : {}`", sv.ty),
        // A lazy / recursive binding: type only.
        (None, false) => format!("`{display}` — `{}`", sv.ty),
        // No type either: the caller's fallback.
        _ => fallback,
    }
}

/// The hover text for a use of an imported module: the imported module's type
/// (its export's checked type) when the build computed one, else a plain
/// "imported module" description naming the imported file.
fn import_hover(name: &str, imp: &ImportBinding) -> String {
    match &imp.ty {
        Some(ty) => format!("`{name}` — imported module : {ty}"),
        None => format!("`{name}` — imported module (from `{}`)", imp.path),
    }
}

/// The type of the named field `field` inside a rendered `struct<...>` type
/// string, e.g. `field_type_in_struct("struct<.succ Int -> Int, .add Int -> Int
/// -> Int>", "succ")` → `Some("Int -> Int")`.  The field-access IR node's own
/// type slot is a lazy cell (it reads `?a` until a later resolve), so a field
/// access on an imported module resolves its type from the module's rendered
/// type instead.  Handles arrow types (`->`) and nested `struct<...>` values.
fn field_type_in_struct(struct_ty: &str, field: &str) -> Option<String> {
    let inner = struct_ty.strip_prefix("struct<")?;
    // Find the matching `>` for the opening `<` (ignoring the `>` of `->`),
    // then split the interior on top-level commas.
    let body = match_close(inner)?;
    for seg in split_top_level(body, ',') {
        let seg = seg.trim();
        let Some(rest) = seg.strip_prefix('.') else {
            continue; // positional field, not named.
        };
        // A well-formed named field is `.name type`; skip a malformed segment.
        let Some(name_end) = rest.find(|c: char| c.is_whitespace()) else {
            continue;
        };
        if &rest[..name_end] == field {
            return Some(rest[name_end..].trim().to_string());
        }
    }
    None
}

/// The substring up to the `>` that closes a depth-1 `struct<...>` opening that
/// was already stripped.  The `>` of an arrow `->` is not a close.
fn match_close(inner: &str) -> Option<&str> {
    let bytes = inner.as_bytes();
    let mut depth = 1;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' if !(i > 0 && bytes[i - 1] == b'-') => {
                depth -= 1;
                if depth == 0 {
                    return Some(&inner[..i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split `s` on `sep` at bracket depth 0 (over `()[]<>`, with the `>` of `->`
/// ignored), returning the segments.
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < s.len() {
        let c = bytes[i] as char;
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' => depth -= 1,
            '>' if !(i > 0 && bytes[i - 1] == b'-') => depth -= 1,
            _ if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&s[start..]);
    out
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

fn classify_token_kind(
    kind: &TokenKind,
) -> Option<(SemanticTokenType, Vec<SemanticTokenModifier>)> {
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
        | TokenKind::KwPub
        | TokenKind::KwTypeOf
        // `_` — a placeholder is a reserved inference form, never a name.
        | TokenKind::Placeholder => Some((SemanticTokenType::KEYWORD, Vec::new())),
        // A `Name` is resolved by `classify_names` (or the `.` heuristic).
        TokenKind::Name(_) => None,
        // Operators: arrows, annotations, separators-of-fields, and math.
        TokenKind::Arrow
        | TokenKind::FatArrow
        | TokenKind::Colon
        | TokenKind::DoubleColon
        | TokenKind::Hash
        | TokenKind::Question
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
                self.map.insert(
                    b.span,
                    (
                        SemanticTokenType::VARIABLE,
                        vec![SemanticTokenModifier::DECLARATION],
                    ),
                );
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
                    (
                        SemanticTokenType::PARAMETER,
                        vec![SemanticTokenModifier::DECLARATION],
                    ),
                );
                if let Some(t) = parameter_type {
                    self.expr(t);
                }
                if let Some(p) = parameter_perspective {
                    self.expr(p);
                }
                self.expr(r#return);
            }
            Expr::Apply {
                function, argument, ..
            } => {
                // A plain name in function position is a function call.
                if let Expr::Name(_, span) = &**function {
                    self.map
                        .insert(*span, (SemanticTokenType::FUNCTION, Vec::new()));
                }
                self.expr(function);
                self.expr(argument);
            }
            Expr::Int(..)
            | Expr::Str(..)
            | Expr::TypeConst(..)
            | Expr::Name(..)
            | Expr::Placeholder(..)
            | Expr::Err { .. }
            | Expr::TypeOf(..) => {}
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
            Expr::Arrow {
                parameter,
                r#return,
                ..
            } => {
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
            Expr::Block {
                statements, expr, ..
            } => {
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

fn index(
    program: &Program,
    imports: &[ResolvedImport],
) -> (Vec<Definition>, HashMap<Span, usize>, HashMap<Span, usize>) {
    let mut walk = Walk {
        defs: Vec::new(),
        scopes: Vec::new(),
        resolve: HashMap::new(),
        def_index: HashMap::new(),
    };
    // Seed the imported bindings into a base scope frame, mirroring the
    // compiler's import frame below the block-wide binding frames: a use of an
    // imported module resolves to its `@import` directive, and a local binding
    // may shadow an imported name (the local frame sits above the import one).
    if !imports.is_empty() {
        walk.scopes.push(HashMap::new());
        for imp in imports {
            walk.enter(&imp.name, imp.span);
        }
    }
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
        self.scopes
            .last_mut()
            .expect("a scope frame is pushed")
            .insert(name.to_string(), idx);
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
            | Expr::Err { .. }
            | Expr::TypeOf(..) => {}
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
            Expr::Apply {
                function, argument, ..
            } => {
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
            Expr::Arrow {
                parameter,
                r#return,
                ..
            } => {
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
            Expr::Block {
                statements, expr, ..
            } => self.scope(statements, Some(expr)),
            Expr::RecordBlock { fields, .. } => {
                // A struct-returning block scopes its field statements exactly
                // like a block: a named field is a binding (a `let` a
                // restrictive one), so the field names resolve — both as the
                // definition site (hovering `succ` in `{succ = ...}`) and as
                // in-block uses.
                let stmts: Vec<Stmt> = fields
                    .iter()
                    .map(|f| match &f.name {
                        Some(name) => Stmt::Binding(lichen_language::ast::Binding {
                            name: name.clone(),
                            value: f.value.clone(),
                            span: f.span,
                            restrictive: !f.field,
                        }),
                        None => Stmt::Expr(f.value.clone()),
                    })
                    .collect();
                self.scope(&stmts, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::Position;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn doc(source: &str) -> Doc {
        Doc::new(source)
    }

    /// A fresh temporary directory (mirrors the language crate's test helper).
    fn temp_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lichen-server-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
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
        // A use of `a` resolves to the binding; its hover shows the bound
        // expr's `value : type` (a = 1 → 1 : Int).
        let (msg, _range) = d
            .hover_at(Position {
                line: 1,
                character: 4,
            })
            .expect("hover on `a`");
        assert!(msg.contains("1 : Int"), "hover msg = {msg}");
    }

    #[test]
    fn hover_on_a_non_binding_definition_reports_its_line() {
        // A lambda parameter is a definition but not a top-level statement, so
        // there is no `value : type` snapshot — the hover falls back to the
        // definition-site line.
        let d = doc("f = x => x\nf 1\n");
        let (msg, _) = d
            .hover_at(Position {
                line: 0,
                character: 4,
            })
            .expect("hover on `x`");
        assert!(msg.contains("defined at line `1`"), "hover msg = {msg}");
    }

    #[test]
    fn hover_renders_a_binding_value_and_type() {
        // A binding hover shows the bound expr's `value : type` — for the
        // binding's own name and for any use of it.
        let d = doc("x = 3\ny = x + 4\nx + y\n");
        // On the definition site of `x`: value 3 : Int.
        let (msg, _) = d
            .hover_at(Position {
                line: 0,
                character: 0,
            })
            .expect("hover on `x` def");
        assert!(msg.contains("3 : Int"), "hover msg = {msg}");
        // On the use of `y` in the final expression `x + y`: value 7 : Int.
        let (msg, _) = d
            .hover_at(Position {
                line: 2,
                character: 4,
            })
            .expect("hover on `y` use");
        assert!(msg.contains("7 : Int"), "hover msg = {msg}");
    }

    #[test]
    fn hover_on_unresolved_name_says_so() {
        let d = doc("a = 1\nb = unknown");
        let (msg, _) = d
            .hover_at(Position {
                line: 1,
                character: 4,
            })
            .expect("hover on `unknown`");
        assert!(msg.contains("unresolved name"), "hover msg = {msg}");
    }

    #[test]
    fn definition_jumps_to_the_binding() {
        let d = doc("a = 1\nb = a + 1\nb");
        let range = d
            .definition_at(Position {
                line: 2,
                character: 0,
            })
            .expect("definition for the final `b`");
        assert_eq!(
            range.start,
            Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn definition_is_none_on_a_non_name() {
        let d = doc("a = 1\na + 1\n");
        // Cursor on the `1` literal in `a + 1` (line index 1, char 4).
        assert!(
            d.definition_at(Position {
                line: 1,
                character: 4
            })
            .is_none()
        );
    }

    #[test]
    fn semantic_tokens_cover_the_source_and_stay_in_bounds() {
        let d = doc("a = 1\nb = 2\na + b\n");
        let toks = d.semantic_tokens();
        assert!(!toks.is_empty(), "expected some semantic tokens");
        for t in &toks {
            assert!(
                t.end >= t.start,
                "token range is ordered {:?}",
                (t.start, t.end)
            );
            assert!(t.end as usize <= d.source.len(), "token end in bounds");
        }
    }

    #[test]
    fn semantic_tokens_classify_literals_operators_and_declarations() {
        let d = doc("f = x => x + 1\nf 7\n");
        let toks = d.semantic_tokens();
        let types = |t: &crate::lsp::SemanticTokenType| toks.iter().any(|x| &x.token_type == t);
        assert!(
            types(&crate::lsp::SemanticTokenType::NUMBER),
            "a literal is a number"
        );
        assert!(
            types(&crate::lsp::SemanticTokenType::OPERATOR),
            "an operator is colored"
        );
        // The binding `f` is a declaration.
        assert!(
            toks.iter()
                .any(|t| t.token_type == crate::lsp::SemanticTokenType::VARIABLE
                    && t.modifiers
                        .contains(&crate::lsp::SemanticTokenModifier::DECLARATION)),
            "binding definition is a declared variable"
        );
        // The lambda parameter `x` is a parameter declaration.
        assert!(
            toks.iter()
                .any(|t| t.token_type == crate::lsp::SemanticTokenType::PARAMETER
                    && t.modifiers
                        .contains(&crate::lsp::SemanticTokenModifier::DECLARATION)),
            "lambda parameter is a declared parameter"
        );
        // The `f` use in `f 7` is a function call.
        assert!(
            toks.iter()
                .any(|t| t.token_type == crate::lsp::SemanticTokenType::FUNCTION),
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
        assert!(
            !comments.is_empty(),
            "expected the preprocess block as a comment"
        );
        for t in &comments {
            assert!(
                t.end as usize <= d.code_base as usize,
                "comment stays before the code"
            );
        }
        // The compiled code region is still classified.
        assert!(
            toks.iter()
                .any(|t| t.token_type == crate::lsp::SemanticTokenType::NUMBER),
            "code numbers are classified past the block"
        );
    }

    #[test]
    fn relative_imports_resolve_against_the_files_directory() {
        // The `import` example, as an editor would open it: the document's URI
        // gives a base path, so `@import "math.lichen"` / `"geometry.lichen"`
        // resolve in the file's own directory (not the process CWD).  This is
        // the LSP path: `Doc` is built with the file path as `base`.
        let dir = temp_dir("import");
        write(
            &dir,
            "math.lichen",
            "@{output = \"(Function, Function): struct<.succ Int -> Int, .add Int -> Int -> Int>\"@}\n{\n  succ = x => x + 1\n  add = x => y => x + y\n}\n",
        );
        write(
            &dir,
            "geometry.lichen",
            "@{math = import \"math.lichen\"\noutput = \"(Function, Function): struct<.double Int -> Int, .inc_twice Int -> Int>\"@}\n{\n  double = x => math.add x x\n  inc_twice = x => math.succ (math.succ x)\n}\n",
        );
        let main_path = write(
            &dir,
            "_.lichen",
            "@{order = \"5\"\nmath = import \"math.lichen\"\ngeo = import \"geometry.lichen\"\noutput = \"(42, 10, 7): <Int, Int, Int>\"@}\n(math.succ 41, geo.double 5, geo.inc_twice 5)\n",
        );

        let d = Doc::new_with_base(
            fs::read_to_string(&main_path).unwrap(),
            Some(main_path.as_path()),
        );
        assert!(
            d.diagnostics.is_empty(),
            "relative imports should resolve; got {:?}",
            d.diagnostics
        );
    }

    #[test]
    fn hover_resolves_imports_and_their_fields() {
        // Hovering an imported module (or a field of it) must not say
        // "unresolved name": the module resolves to its `@import` directive
        // (and hovers with the module's type), and a field resolves to the
        // imported module it belongs to.
        let dir = temp_dir("hoverimport");
        write(
            &dir,
            "math.lichen",
            "{\n  succ = x => x + 1\n  add = x => y => x + y\n}\n",
        );
        let main_path = write(
            &dir,
            "main.lichen",
            "@{\n  math = import \"math.lichen\"\n@}\nmath.succ 41\n",
        );
        let d = Doc::new_with_base(
            fs::read_to_string(&main_path).unwrap(),
            Some(main_path.as_path()),
        );

        // The module name `math` (line 3, char 0): imported module + its type.
        let (msg, _) = d
            .hover_at(Position {
                line: 3,
                character: 0,
            })
            .expect("hover on `math`");
        assert!(msg.contains("imported module"), "module hover msg = {msg}");
        assert!(msg.contains("Int -> Int"), "module hover type = {msg}");

        // The field `succ` (line 3, char 5): a field of the imported module —
        // its `value : type`, not "unresolved".
        let (msg, _) = d
            .hover_at(Position {
                line: 3,
                character: 5,
            })
            .expect("hover on `succ`");
        assert!(msg.contains("Int -> Int"), "field hover msg = {msg}");
        assert!(!msg.contains("unresolved"), "field hover msg = {msg}");

        // Go-to-definition on the module use jumps to the import directive
        // (`math` in `@{`...` math = import ...` at line 1, char 2).
        let def = d
            .definition_at(Position {
                line: 3,
                character: 0,
            })
            .expect("def on `math`");
        assert_eq!(
            def.start,
            Position {
                line: 1,
                character: 2
            }
        );
    }

    #[test]
    fn imported_field_access_hovers_with_value_and_type() {
        // The repo's living spec `import/_.lichen`: hovering an accessed field
        // of an imported module (`math.succ`, `geo.double`, `geo.inc_twice`)
        // renders the field's `value : type` (the module's field table), not
        // "field of imported module `X`" and never "unresolved".
        let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../lichen-language/examples/programs/import");
        let main_path = examples.join("_.lichen");
        let d = Doc::new_with_base(
            fs::read_to_string(&main_path).unwrap(),
            Some(main_path.as_path()),
        );
        assert!(
            d.diagnostics.is_empty(),
            "repo import example should check clean; got {:?}",
            d.diagnostics
        );

        // Line 7 (0-based line 6): `(math.succ 41, geo.double 5, geo.inc_twice 5)`
        for pos in [
            // `.succ` field access on `math`
            Position {
                line: 6,
                character: 6,
            },
            // `.double` field access on `geo`
            Position {
                line: 6,
                character: 19,
            },
            // `.inc_twice` field access on `geo`
            Position {
                line: 6,
                character: 33,
            },
        ] {
            let (msg, _) = d.hover_at(pos).expect("hover on an imported field access");
            assert!(
                msg.contains("Function : Int -> Int"),
                "field access hover msg = {msg}"
            );
            assert!(
                !msg.contains("unresolved") && !msg.contains("field of imported module"),
                "field access hover msg = {msg}"
            );
        }
    }

    #[test]
    fn local_struct_field_access_hovers_with_value_and_type() {
        // A field access on a *local* struct binding (`point.x`) renders the
        // field's value:type too — not "unresolved".  It reads the current
        // file's struct-block field table.
        let d = doc("point = { x = 1, y = 2 }\npoint.x\n");
        let (msg, _) = d
            .hover_at(Position {
                line: 1,
                character: 6,
            })
            .expect("hover on `.x`");
        assert!(
            msg.contains("Int") && !msg.contains("unresolved"),
            "local field access hover msg = {msg}"
        );
    }

    #[test]
    fn module_field_definitions_resolve() {
        // Inside a module file (math.lichen = `{succ = …, add = …}`), hovering
        // a record *field* definition (`succ`, `add`) must not say
        // "unresolved name" — a struct block scopes its field bindings just
        // like a block, so the field name is a definition site.
        let dir = temp_dir("modulefields");
        let math_path = write(
            &dir,
            "math.lichen",
            "{\n  succ = x => x + 1\n  add = x => y => x + y\n}\n",
        );
        let d = Doc::new_with_base(
            fs::read_to_string(&math_path).unwrap(),
            Some(math_path.as_path()),
        );

        // `succ` at line 1, char 2; `add` at line 2, char 2.  Each hovers with
        // its value:type (`Function : Int -> Int` / `... -> Int -> Int`), never
        // "unresolved" and never merely "defined at line".
        for (line, name) in [(1usize, "succ"), (2, "add")] {
            let (msg, _) = d
                .hover_at(Position {
                    line: line as u32,
                    character: 2,
                })
                .expect(&format!("hover on `{name}`"));
            assert!(
                !msg.contains("unresolved"),
                "`{name}` should resolve; got {msg}"
            );
            assert!(
                msg.contains("->"),
                "`{name}` should show its type; got {msg}"
            );
        }
    }

    #[test]
    fn relative_imports_with_no_base_resolve_nowhere() {
        // The pre-fix behaviour: with `base = None` the same relative imports
        // resolve against the process CWD, which is almost never the file's
        // directory, so they fail — this documents why the LSP must pass a base.
        let d = doc("@{math = import \"math.lichen\"@}math\n");
        assert!(
            d.diagnostics
                .iter()
                .any(|x| x.message.contains("cannot load package 'math.lichen'")),
            "expected a cannot-load diagnostic, got {:?}",
            d.diagnostics
        );
    }
}
