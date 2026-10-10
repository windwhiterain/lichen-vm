//! [`Doc`]: the frontend artifacts plus the editor-view index over them.
//! Layering: `docs/notes/language-toolchain.md`.

use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use stacksafe::stacksafe;

use lichen_compute::{ComputeOperator, ComputeValue};
use lichen_highlevel::ir::ExprId;
use lichen_highlevel::no_native_ops;
use lichen_highlevel::program::{PackageSource, TypeOperator, ValueType};
use lichen_language::LangProgramShape;
use lichen_language::ast::{Binding, Expr, Program, Stmt};
use lichen_language::diag::{Diag, Stage};
use lichen_language::lex;
use lichen_language::lex::Span;
use lichen_language::lex::{Token, TokenKind};
use lichen_language::package::PackageStore;
use lichen_language::parse;
use lichen_language::persist::ArtifactCodec;
use lichen_language::preprocess;
use lichen_language::preprocess::ResolvedImport;
use lichen_language::program::GcdOp;
use lichen_language::render::{print_type_lang, print_value_lang, struct_type_named_fields};
use lichen_language::{build_report, frontend_at};
use lichen_lowlevel::AnyNodeId;
use lichen_utils::extend::AsEnum;

use crate::lsp::{
    self, Diagnostic, DiagnosticSeverity, Position, Range, SemanticTokenData,
    SemanticTokenModifier, SemanticTokenType, SemanticTokens,
};
use crate::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, InsertTextFormat, TextEdit,
};

/// A definition site: a binding name, a parameter, or a built-in
/// module's export, positioned in that module's own file.
#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub span: Span,
    /// The other file `span` is a position in; `None` means this document.
    ///
    /// # Invariant
    /// The file exists on disk, so an editor can open it
    /// (`docs/notes/core-prelude.md` §4).
    pub file: Option<Arc<PackageSource>>,
}

/// One name a **built-in** module exposes, as a definition *in that module*.
///
/// # Invariant
/// A document's own binding of the same name shadows it — the built-in frame
/// sits below every document frame — so the prelude is shadowable, not reserved.
/// Their spans are positions in another file, which is why they are kept out of
/// [`DocIndex::defs`] and every document-keyed table.
#[derive(Clone, Debug)]
pub struct BuiltinName {
    pub name: String,
    /// The definition site: the built-in's file and the export's position in it.
    pub def: Definition,
}

/// One *other* file's diagnostics, rendered for the protocol.
///
/// # Invariant
/// A failure inside a built-in module is one of these, never part of the
/// document's own set (`docs/notes/core-prelude.md` §4).
#[derive(Clone, Debug)]
pub struct FileDiagnostics {
    /// The file the diagnostics belong to — its path on disk, which the client
    /// opens (`Url::from_file_path`).
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
}

/// An imported binding, indexed for the editor.
///
/// # Invariant
/// A use of the name resolves to this, so hovering an imported module (or a
/// field of it) is not "unresolved".
#[derive(Clone, Debug)]
struct ImportBinding {
    /// The binding name the import is available under (`math`).
    name: String,
    /// The `@import` directive's start span: where a use resolves to.
    span: Span,
    /// The import path (`math.lichen`), for a descriptive hover.
    path: String,
    /// The imported module's rendered type (its export's type), when the build
    /// computed one.
    ty: Option<String>,
}

/// One top-level statement's checked type and, when concrete, its value.
///
/// # Invariant
/// The snapshot only *reads* `build.ty`/`build.val`/`module.node_value`; it
/// never evaluates a node or forces a lazy cell, so a deferred binding reports
/// `value: None` rather than diverging on the compiler's recursion clones.
#[derive(Clone, Debug)]
pub struct StatementValue {
    /// The statement's source span (its start position, 1-based `(line, col)`).
    pub span: Span,
    /// The statement's checked type, rendered in lichen's type syntax.
    pub ty: String,
    /// The value when the cascade computed one; `None` for a lazy statement.
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
///
/// # Invariant
/// Artifacts, name-resolution index, and pipeline diagnostics. This is the
/// `Send` half of an analysis: [`Doc`] wraps it with the checker's `!Send`
/// structured diagnostics and re-exports it through [`Deref`], so every lookup
/// reads the same from either.
pub struct DocIndex {
    /// The full source text.
    pub source: String,
    /// Byte offset at which each line begins (line 1 = 0).
    pub line_starts: Vec<usize>,
    /// Byte offset where the compiled code begins, past the leading `---…---`
    /// preprocessor block (0 when there is none).
    pub code_base: u32,
    /// The token stream with byte ranges, shared with its producer.
    pub tokens: Arc<Vec<Token>>,
    /// The parsed AST — the frontend's parser output, shared for the same reason.
    pub program: Arc<Program>,
    /// Every definition site **in this document**, in declaration order.
    pub defs: Vec<Definition>,
    /// The **built-in** definitions this document can use without an import.
    ///
    /// # Invariant
    /// Kept apart from [`DocIndex::defs`], whose entries are what the editor's
    /// own name tables key on, because a built-in's span is a position in
    /// another file (`docs/notes/core-prelude.md` §4).
    pub builtin_names: Vec<BuiltinName>,
    /// The whole diagnostic set (lex, parse, resolve, check), minus the
    /// checker's arena-bound facts.
    lsp_diagnostics: Vec<Diagnostic>,
    /// Diagnostics belonging to **another file**, never to the document.
    file_diagnostics: Vec<FileDiagnostics>,
    /// Span of a name *use* → the definition it resolves to.
    ///
    /// # Invariant
    /// A built-in's target is an index into [`DocIndex::builtin_names`], never
    /// into [`DocIndex::defs`] — the two lists have different coordinates.
    resolve: HashMap<Span, ScopeValue>,
    /// Span of a definition site → index into [`DocIndex::defs`].
    def_index: HashMap<Span, usize>,
    /// Span of a binding name → the index of the statement that defines it.
    stmt_by_span: HashMap<Span, usize>,
    /// Per top-level statement, in source order: see [`StatementValue`].
    statements: Vec<StatementValue>,
    /// The byte offset of each statement's start, source order (the span index
    /// backing [`DocIndex::statement_at`]).
    stmt_starts: Vec<u32>,
    /// The imported bindings this file resolves, in source order.
    imports: Vec<ImportBinding>,
    /// Import-directive span → index into [`DocIndex::imports`].
    import_by_span: HashMap<Span, usize>,
    /// Per struct-block field, keyed by name: its checked `value : type`.
    ///
    /// # Invariant
    /// Field names are not IR nodes, so a field's type is read from the `Record`
    /// node's field-value tuple.
    field_types: HashMap<String, StatementValue>,
    /// Per field access on an imported module, keyed by (module, field).
    ///
    /// # Invariant
    /// Read from each `NamedField` IR node whose container is the module's
    /// imported `Static`, so hovering an access renders the field's value/type.
    module_field_types: HashMap<(String, String), StatementValue>,
    /// Per imported-module binding: the exported field names it offers.
    module_fields: HashMap<String, Vec<String>>,
    /// Per top-level statement with a concrete struct type: its field names.
    struct_fields_by_stmt: HashMap<usize, Vec<String>>,
}

/// A parsed + checked source plus the checker's structured diagnostics.
///
/// # Invariant
/// The handle is **not** `Send`: a checker diagnostic carries the program's
/// arena-bound facts, so a host that caches across threads keeps the `Send`
/// [`DocIndex`] and lets the handle drop.
pub struct Doc<P: LangProgramShape> {
    index: Arc<DocIndex>,
    /// The full diagnostic set, with the checker's structured facts.
    pub diagnostics: Vec<Diag<P>>,
}

impl<P: LangProgramShape> Deref for Doc<P> {
    type Target = DocIndex;

    fn deref(&self) -> &DocIndex {
        &self.index
    }
}

impl<P: LangProgramShape> Doc<P>
where
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    /// Parse, lower and check `source`, keeping the frontend artifacts and
    /// indexing name resolution.
    ///
    /// # Invariant
    /// The leading `---…---` block is cut out and resolved first, so a real
    /// file compiles on the code after it with spans absolute in the original.
    /// Imports resolve against the current directory; use [`Doc::new_with_base`]
    /// for the LSP server's relative case.
    pub fn new(source: impl Into<String>) -> Doc<P> {
        Doc::new_with_base(source, None)
    }

    /// [`Doc::new`] with an explicit base path: `@import` paths resolve
    /// against it; `None` means the current directory.
    pub fn new_with_base(source: impl Into<String>, base: Option<&Path>) -> Doc<P> {
        Doc::new_with_cache(source, base, None)
    }

    /// [`Doc::new_with_base`] with a persistent cache root:
    /// this vocabulary's `compilers/<plugin-set-key>` slot.
    ///
    /// # Invariant
    /// A healthy home is never silently dropped to in-memory: that happens only
    /// when no root was given or the codec cannot persist. See
    /// `docs/notes/liche-lsp-home.md`.
    pub fn new_with_cache(
        source: impl Into<String>,
        base: Option<&Path>,
        cache_root: Option<&Path>,
    ) -> Doc<P> {
        let mut store: PackageStore<P> = match cache_root {
            Some(root) if P::Codec::PERSISTENT => PackageStore::with_cache_dir(root.to_path_buf()),
            _ => PackageStore::new(),
        };
        Doc::new_with_store(source, base, &mut store)
    }

    /// [`Doc::new_with_cache`] over a caller-owned store, reused per document.
    pub fn new_with_store(
        source: impl Into<String>,
        base: Option<&Path>,
        store: &mut PackageStore<P>,
    ) -> Doc<P> {
        let (index, diagnostics) = analyze::<P>(source, base, store);
        Doc {
            index: Arc::new(index),
            diagnostics,
        }
    }

    /// The `Send` editor index this analysis produced.
    pub fn index(&self) -> Arc<DocIndex> {
        Arc::clone(&self.index)
    }
}

/// The frontend artifacts an editor index is built from.
///
/// # Invariant
/// Both producers — the one-shot frontend and the incremental `BufferSession` —
/// end in [`index`], so an editor sees one analysis whatever drove it.
pub struct Artifacts<P: LangProgramShape>
where
    P::Value: ValueType,
{
    /// The token stream, in absolute source positions.
    pub tokens: Arc<Vec<Token>>,
    /// The resolved AST, in absolute source positions.
    pub program: Arc<Program>,
    /// The build's `ExprId → span` index, when a build ran.
    pub span_index: Option<Arc<Vec<Option<Span>>>>,
    /// The checked build, when the frontend resolved one.
    pub build: Option<Arc<lichen_highlevel::checker::Build<P>>>,
    /// Every diagnostic the producing pipeline reported; preprocess ones are
    /// the caller's.
    pub diagnostics: Vec<Diag<P>>,
}

/// One full frontend run: preprocess → lex → parse → resolve → check.
///
/// # Invariant
/// This is the **one-shot** path — [`Doc::new`] and the compile worker's
/// fallback — as opposed to [`index`], the shared tail it and the incremental
/// session both end in.
pub(crate) fn analyze<P>(
    source: impl Into<String>,
    base: Option<&Path>,
    store: &mut PackageStore<P>,
) -> (DocIndex, Vec<Diag<P>>)
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    let source = source.into();
    let line_starts = lex::line_starts(&source);

    // Cut the leading `---…---` block off the input; `base` resolves imports.
    let mut diagnostics = preprocess::stage_depends::<P>(store, &source);
    let (pre, preprocess_diagnostics) = preprocess::preprocess(&source, base, store);
    diagnostics.extend(preprocess_diagnostics);

    // The frontend artifacts, in absolute file coordinates.
    let lexed = lex::lex_with(pre.code, &line_starts, pre.code_base);
    let tokens = lexed.tokens;
    let parsed = parse::parse(&tokens);
    let program = parsed.program;

    // The full pipeline diagnostics, all spans absolute: preprocess, then the
    // frontend, then the checker over the IR.
    let frontend = frontend_at(pre.code, pre.code_base, &line_starts, &pre.imports);
    diagnostics.extend(frontend.diagnostics.into_iter().map(|d| d.retype()));
    let mut report = build_report::<P>(
        frontend.ir,
        Some(frontend.span_index),
        diagnostics,
        Some(store.registry()),
        no_native_ops(),
        None,
        Vec::new(),
        "",
    );
    let span_index = report.span_index.take().map(Arc::new);
    let build = report.build.take().map(Arc::new);
    let artifacts = Artifacts {
        tokens: Arc::new(tokens),
        program: Arc::new(program),
        span_index,
        build,
        diagnostics: report.diagnostics,
    };
    index::<P>(&source, line_starts, &pre, artifacts, store)
}

/// The editor index over frontend artifacts, plus the pipeline diagnostics.
///
/// # Invariant
/// The one place the editor's view of a program is derived — [`analyze`] and
/// the compile worker both end here, so the incremental and one-shot paths
/// cannot drift apart. `store` is where built-in source records are read from.
pub fn index<P>(
    source: &str,
    line_starts: Vec<usize>,
    pre: &preprocess::Preprocessed<'_>,
    artifacts: Artifacts<P>,
    store: &PackageStore<P>,
) -> (DocIndex, Vec<Diag<P>>)
where
    P: LangProgramShape,
    P::Value: ValueType + AsEnum<ComputeValue> + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    let Artifacts {
        tokens,
        program,
        span_index,
        build,
        diagnostics,
    } = artifacts;

    // The imported module's checked type, per `@import` directive span: the
    // compiler allocates a `Static` node there.
    let mut import_ty: HashMap<Span, String> = HashMap::new();
    // Per imported-module binding: its exported field names.
    let mut module_fields: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(build) = &build {
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
                && let Some(t) = build.state[eid].ty
            {
                import_ty.insert(imp.span, print_type_lang(&build.module, t));
                if let Some(names) = struct_type_named_fields(&build.module, t) {
                    let fields = names
                        .into_iter()
                        .flatten()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>();
                    module_fields.insert(imp.name.clone(), fields);
                }
            }
        }
    }

    // Directive span → import binding name, so a field access's container
    // traces back to its module.
    let import_name_by_span: HashMap<Span, &str> = pre
        .imports
        .iter()
        .map(|i| (i.span, i.name.as_str()))
        .collect();

    // A read-only per-statement type/value snapshot; see [`StatementValue`].
    let (statements, stmt_starts, field_types, module_field_types, struct_fields_by_stmt) =
        match &build {
            Some(build) => {
                let mut statements = Vec::new();
                let mut starts = Vec::new();
                // Per top-level struct binding: its named-field list (for the
                // `point.…` field completion), indexed by statement.
                let mut struct_fields_by_stmt: HashMap<usize, Vec<String>> = HashMap::new();
                for (i, &id) in build.ir.stmt_roots.iter().enumerate() {
                    // The IR statement's own span points at its *value*
                    // expression, so the index uses the AST statement's start.
                    let (span, start) = match program.statements.get(i).map(|bs| &bs.stmt) {
                        Some(Stmt::Binding(b)) => {
                            (b.span, lsp::offset_of_span(&line_starts, b.span))
                        }
                        Some(Stmt::Expr(e)) => {
                            let s = e.span();
                            (s, lsp::offset_of_span(&line_starts, s))
                        }
                        // The tail expression (a statement the AST lists
                        // nowhere): its own span is the AST's, not the build's.
                        None => {
                            let s = program
                                .expr
                                .as_ref()
                                .map(|e| e.span())
                                .or_else(|| {
                                    span_index
                                        .as_ref()
                                        .and_then(|idx| idx.get(id.0 as usize).copied().flatten())
                                })
                                .unwrap_or((0, 0));
                            (s, lsp::offset_of_span(&line_starts, s))
                        }
                    };
                    let ty_node = build.state[id].ty;
                    let ty = match ty_node {
                        Some(t) => print_type_lang(&build.module, t),
                        None => String::new(),
                    };
                    // A concrete struct binding offers its fields after a `.`.
                    if let Some(t) = ty_node
                        && let Some(names) = struct_type_named_fields(&build.module, t)
                    {
                        struct_fields_by_stmt.insert(
                            i,
                            names.into_iter().flatten().map(|n| n.to_string()).collect(),
                        );
                    }
                    let value = build.state[id].val.and_then(|vn| {
                        // An empty slot is a deferred (lazy / recursive)
                        // binding — report type only, never force.
                        build.module.node_value(AnyNodeId::Dynamic(vn)).map(|v| {
                            print_value_lang(
                                &build.module,
                                v,
                                build.state[id].ty.unwrap_or_default(),
                            )
                        })
                    });
                    statements.push(StatementValue { span, ty, value });
                    starts.push(start as u32);
                }
                // A struct-block field's value:type snapshot, keyed by field
                // name: field names are not IR nodes.
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
                        let ty = match build.state[val_id].ty {
                            Some(t) => print_type_lang(&build.module, t),
                            None => String::new(),
                        };
                        let value = build.state[val_id].val.and_then(|vn| {
                            // An empty slot is a deferred binding: type only.
                            build.module.node_value(AnyNodeId::Dynamic(vn)).map(|v| {
                                print_value_lang(
                                    &build.module,
                                    v,
                                    build.state[val_id].ty.unwrap_or_default(),
                                )
                            })
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
                // A field *access* on an imported module: a `NamedField` over the
                // module's `Static`, whose own type slot stays lazy.
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
                    let value = build.state[eid].val.and_then(|vn| {
                        // An empty slot is a deferred binding: type only.
                        build.module.node_value(AnyNodeId::Dynamic(vn)).map(|v| {
                            print_value_lang(
                                &build.module,
                                v,
                                build.state[eid].ty.unwrap_or_default(),
                            )
                        })
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
                (
                    statements,
                    starts,
                    field_types,
                    module_field_types,
                    struct_fields_by_stmt,
                )
            }
            None => (
                Vec::new(),
                Vec::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
            ),
        };

    // The names a **built-in** module exposes as bare names, with the
    // definition site in the built-in's own file.
    let builtin_names = builtin_names(&pre.imports, store);
    // The **prelude** import's bindings are `builtin_names`; its `(1, 1)`
    // span collides with every document position.
    let written_imports: Vec<&ResolvedImport> = pre
        .imports
        .iter()
        .filter(|imp| !lichen_language::package::is_prelude_import(imp))
        .collect();
    let (defs, resolve, def_index) = index_names(&program, &written_imports, &builtin_names);
    // Each statement's span → its statement index, so a name bound by
    // one reaches that statement's value/type.
    let stmt_by_span: HashMap<Span, usize> = statements
        .iter()
        .enumerate()
        .map(|(i, s)| (s.span, i))
        .collect();
    // The imported bindings and their type (for the hover).
    let imports: Vec<ImportBinding> = written_imports
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
    let lsp_diagnostics = render_diagnostics(&diagnostics, source, &line_starts);
    // A built-in's failure is a property of *its* file (`Diag::file`), so it is
    // published separately.
    let file_diagnostics = file_diagnostics(&diagnostics);

    (
        DocIndex {
            source: source.to_string(),
            line_starts,
            code_base: pre.code_base,
            tokens,
            program,
            defs,
            builtin_names,
            lsp_diagnostics,
            file_diagnostics,
            resolve,
            def_index,
            stmt_by_span,
            statements,
            stmt_starts,
            imports,
            import_by_span,
            field_types,
            module_field_types,
            module_fields,
            struct_fields_by_stmt,
        },
        diagnostics,
    )
}

/// The names the built-in packages in `imports` expose as bare names, in
/// import order.
///
/// # Invariant
/// Each name is positioned by its own binding in that built-in's file, and
/// only the seeded **prelude** contributes: a built-in a program imported
/// explicitly exposes its names through its binding, as any package does.
fn builtin_names<P>(imports: &[ResolvedImport], store: &PackageStore<P>) -> Vec<BuiltinName>
where
    P: LangProgramShape,
    P::Value: ValueType + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    let mut names = Vec::new();
    for import in imports {
        // A built-in is filed under its own key, not the path cache.
        let Some(source) = store.package_source(import.export.module) else {
            continue;
        };
        // Only a **seeded** built-in contributes bare names: its module is the
        // prelude, in scope with no import.
        if !source
            .path
            .file_name()
            .is_some_and(|name| import.path.file_name() == Some(name))
        {
            continue;
        }
        let source = Arc::new(source);
        // The built-in's own top-level bindings: the record's `spans`
        // hold only the frozen nodes a failure can name.
        let positions = definition_spans(&source.code);
        for (name, _export) in &import.direct {
            let Some(span) = positions.iter().find(|(n, _)| n == name).map(|(_, s)| *s) else {
                continue;
            };
            names.push(BuiltinName {
                name: name.clone(),
                def: Definition {
                    name: name.clone(),
                    span,
                    file: Some(Arc::clone(&source)),
                },
            });
        }
    }
    names
}

/// The position of every top-level **binding** in a package's source text, in
/// source order.
///
/// # Invariant
/// Read from the text, not the record's frozen-node positions: those cover the
/// nodes a *failure* can name, and a lambda binding freezes as none.
fn definition_spans(code: &str) -> Vec<(String, Span)> {
    let tokens = lex::lex(code).tokens;
    let parsed = parse::parse(&tokens);
    parsed
        .program
        .statements
        .iter()
        .filter_map(|block| match &block.stmt {
            Stmt::Binding(binding) => Some((binding.name.clone(), binding.span)),
            Stmt::Expr(_) => None,
        })
        .collect()
}

/// The pipeline diagnostics **of this document**, as LSP [`Diagnostic`]s.
///
/// # Invariant
/// A diagnostic naming another file (`Diag::file`) is left to
/// [`file_diagnostics`], so the document is never blamed for a position it does
/// not contain.
fn render_diagnostics<P: LangProgramShape>(
    diagnostics: &[Diag<P>],
    source: &str,
    line_starts: &[usize],
) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| d.file.is_none())
        .map(|d| {
            let range = d
                .span
                .map(|s| lsp::range_from_span(source, line_starts, s))
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

/// The pipeline diagnostics that belong to **another file**, grouped
/// by that file and rendered on its own positions.
///
/// # Invariant
/// A diagnostic with a `file` but no `span` is dropped rather than given
/// the document's coordinates, which are what it must not be given.
fn file_diagnostics<P: LangProgramShape>(diagnostics: &[Diag<P>]) -> Vec<FileDiagnostics> {
    let mut out: Vec<FileDiagnostics> = Vec::new();
    for d in diagnostics {
        let (Some(file), Some(span)) = (&d.file, d.span) else {
            continue;
        };
        let line_starts = lex::line_starts(&file.code);
        let rendered = Diagnostic {
            range: lsp::range_from_span(&file.code, &line_starts, span),
            severity: Some(severity_for(d.stage)),
            code: None,
            code_description: None,
            source: Some("lichen".to_string()),
            message: d.message.clone(),
            tags: None,
            related_information: None,
            data: None,
        };
        match out.iter_mut().find(|f| f.path == file.path) {
            Some(existing) => existing.diagnostics.push(rendered),
            None => out.push(FileDiagnostics {
                path: file.path.clone(),
                diagnostics: vec![rendered],
            }),
        }
    }
    out
}

impl DocIndex {
    /// The pipeline diagnostics **of this document** as LSP [`Diagnostic`]s.
    ///
    /// # Invariant
    /// A diagnostic inside a built-in module is not one of these; it is
    /// [`DocIndex::file_diagnostics`].
    pub fn lsp_diagnostics(&self) -> Vec<Diagnostic> {
        self.lsp_diagnostics.clone()
    }

    /// The diagnostics of every **other file** this analysis reported.
    ///
    /// # Invariant
    /// Published against that file's own URI (`docs/notes/core-prelude.md` §4).
    pub fn file_diagnostics(&self) -> &[FileDiagnostics] {
        &self.file_diagnostics
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

    /// Every top-level statement's checked type and value, in source order.
    ///
    /// # Invariant
    /// Read-only: a statement the cascade left lazy reports
    /// [`StatementValue::value`] as `None` and its type only.
    pub fn statement_values(&self) -> &[StatementValue] {
        &self.statements
    }

    /// The top-level statement whose source range contains byte `offset`.
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
    /// definition it resolves to (or that it *is*).
    ///
    /// # Invariant
    /// A built-in's definition has no position in this document, so it is
    /// described by the file it is defined in, never by a document line.
    pub fn hover_at(&self, position: Position) -> Option<(String, Range)> {
        let offset = self.offset_of(position)?;
        let token = self.token_at(offset)?;
        let range = lsp::range_from_span(&self.source, &self.line_starts, token.span);
        let kind = &token.kind;
        if let TokenKind::Name(name) = kind {
            // Resolve the hovered name: a use to its binding, or the binding's
            // own definition site.
            let builtin = self
                .resolve
                .get(&token.span)
                .and_then(|def| def.builtin())
                .map(|i| &self.builtin_names[i].def);
            let def_idx = self
                .resolve
                .get(&token.span)
                .and_then(|def| def.document())
                .or_else(|| self.def_index.get(&token.span).copied());
            let msg = match (builtin, def_idx) {
                (Some(def), _) => builtin_hover(name, def),
                (None, Some(i)) => {
                    let def = &self.defs[i];
                    // An imported module: the use resolves to its `@import`
                    // directive, so render the module's type.
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
                (None, None) => {
                    // A field access: the field belongs to its container,
                    // so it is not unresolved; hover it as `value : type`.
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

    /// The LSP byte range a definition spans **in its own file**.
    pub fn definition_range(&self, def: &Definition) -> Range {
        match &def.file {
            Some(file) => lsp::range_from_span(&file.code, &lex::line_starts(&file.code), def.span),
            None => lsp::range_from_span(&self.source, &self.line_starts, def.span),
        }
    }

    /// Go to definition for a cursor position on a name *use*, if any.
    ///
    /// # Invariant
    /// Read [`Definition::file`] to tell a definition in this document from one
    /// in a built-in's file.
    pub fn definition_at(&self, position: Position) -> Option<Definition> {
        let offset = self.offset_of(position)?;
        let token = self.token_at(offset)?;
        if let TokenKind::Name(_) = &token.kind
            && let Some(def) = self.resolve.get(&token.span).copied()
        {
            return Some(def.definition(self));
        }
        None
    }

    /// Completion items for a cursor position: every name *in scope* there,
    /// filtered by the word being typed.
    ///
    /// # Invariant
    /// The names are those visible under the compiler's scope rules (mirrored
    /// by [`index`]), so a completion never proposes a name unusable there.
    pub fn completion_at(&self, position: Position) -> Vec<CompletionItem> {
        let Some(offset) = self.offset_of(position) else {
            return Vec::new();
        };
        // Field access (`a.…`): the container's struct fields, not bare names.
        if self.in_field_access(offset) {
            return self.field_completion(offset);
        }
        let (prefix, replace) = self.completion_word(offset);
        let imports: Vec<(String, Span)> = self
            .imports
            .iter()
            .map(|i| (i.name.clone(), i.span))
            .collect();
        let builtin: Vec<String> = self.builtin_names.iter().map(|b| b.name.clone()).collect();
        let in_scope = scope_names_at(&self.program, &imports, &builtin, &self.line_starts, offset);
        in_scope
            .into_iter()
            .filter(|name| name.name.starts_with(&prefix))
            .map(|name| self.completion_item(&name, replace))
            .collect()
    }

    /// Completion for a field access `container.…`: the container's fields,
    /// else empty.
    fn field_completion(&self, offset: usize) -> Vec<CompletionItem> {
        let Some(dot_idx) = self
            .tokens
            .iter()
            .rposition(|t| (t.range.1 as usize) <= offset && t.kind == TokenKind::Dot)
        else {
            return Vec::new();
        };
        // The container is the token immediately before the `.`.
        if dot_idx < 1 {
            return Vec::new();
        }
        let container = &self.tokens[dot_idx - 1];
        let TokenKind::Name(container_name) = &container.kind else {
            return Vec::new();
        };
        // The partial field name, if any: a Name token after the dot
        // ending at/before the cursor; else insert at the cursor.
        let (prefix, replace) = match self.tokens.get(dot_idx + 1) {
            Some(field)
                if matches!(field.kind, TokenKind::Name(_))
                    && (field.range.1 as usize) <= offset =>
            {
                let TokenKind::Name(name) = &field.kind else {
                    unreachable!();
                };
                (name.clone(), (field.range.0, field.range.1))
            }
            _ => (String::new(), (offset as u32, offset as u32)),
        };
        self.container_field_names(container.span)
            .into_iter()
            .filter(|name| name.starts_with(&prefix))
            .map(|name| self.field_completion_item(container_name, &name, replace))
            .collect()
    }

    /// The named fields of a `container.…` container: an imported
    /// module's, or a local struct binding's.
    fn container_field_names(&self, container_span: Span) -> Vec<String> {
        let Some(def) = self
            .resolve
            .get(&container_span)
            .copied()
            .and_then(|def| match def {
                // A built-in's container: its definition is in another file,
                // and no struct type was read for it.
                ScopeValue::Builtin(_) => None,
                ScopeValue::Document(i) => Some(&self.defs[i]),
            })
        else {
            return Vec::new();
        };
        if let Some(import_i) = self.import_by_span.get(&def.span).copied() {
            let module = &self.imports[import_i].name;
            return self.module_fields.get(module).cloned().unwrap_or_default();
        }
        if let Some(stmt_i) = self.stmt_by_span.get(&def.span).copied() {
            return self
                .struct_fields_by_stmt
                .get(&stmt_i)
                .cloned()
                .unwrap_or_default();
        }
        Vec::new()
    }

    /// A completion item for one struct field of a `container.…` access: a
    /// FIELD kind with the field's `value : type`.
    fn field_completion_item(
        &self,
        container_name: &str,
        name: &str,
        replace: (u32, u32),
    ) -> CompletionItem {
        let detail = if self.imports.iter().any(|im| im.name == container_name) {
            self.module_field_types
                .get(&(container_name.to_string(), name.to_string()))
                .map(|sv| sv.ty.clone())
        } else {
            // A local struct field: the flat field table (a field
            // name is not an IR node).
            self.field_types.get(name).map(|sv| sv.ty.clone())
        };
        let range = lsp::range_from_byte_range(&self.source, &self.line_starts, replace);
        CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FIELD),
            detail,
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: name.to_string(),
            })),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            ..Default::default()
        }
    }

    /// Whether the cursor at byte `offset` is in a field access: a `.` before it,
    /// or a partial field name after a `.`.
    fn in_field_access(&self, offset: usize) -> bool {
        let Some(prev_idx) = self
            .tokens
            .iter()
            .rposition(|t| (t.range.1 as usize) <= offset)
        else {
            return false;
        };
        match &self.tokens[prev_idx].kind {
            TokenKind::Dot => true,
            TokenKind::Name(_) => prev_idx > 0 && self.tokens[prev_idx - 1].kind == TokenKind::Dot,
            _ => false,
        }
    }

    /// The word being completed at `offset` and the byte range it spans:
    /// a `Name` token, else empty.
    fn completion_word(&self, offset: usize) -> (String, (u32, u32)) {
        let is_name = |t: &Token| matches!(t.kind, TokenKind::Name(_));
        // Cursor strictly inside a token (the usual mid-typing case).
        if let Some(t) = self.token_at(offset) {
            if is_name(t) {
                let TokenKind::Name(name) = &t.kind else {
                    unreachable!();
                };
                return (name.clone(), (t.range.0, t.range.1));
            }
            // On a non-name token: nothing to complete.
            return (String::new(), (offset as u32, offset as u32));
        }
        // Cursor exactly at a token boundary: a Name that starts or ends here.
        if let Some(t) = self
            .tokens
            .iter()
            .find(|t| is_name(t) && (t.range.0 as usize == offset || t.range.1 as usize == offset))
        {
            let TokenKind::Name(name) = &t.kind else {
                unreachable!();
            };
            return (name.clone(), (t.range.0, t.range.1));
        }
        (String::new(), (offset as u32, offset as u32))
    }

    /// A completion item for one in-scope name: a module for an import,
    /// a function for an arrow type, else a variable.
    fn completion_item(&self, scoped: &ScopedName, replace: (u32, u32)) -> CompletionItem {
        let name = &scoped.name;
        let detail = match scoped.document {
            Some(span) => self.doc_detail(span),
            None => self.builtin_detail(name),
        };
        let kind = if scoped
            .document
            .is_some_and(|span| self.import_by_span.contains_key(&span))
        {
            Some(CompletionItemKind::MODULE)
        } else if detail.as_deref().is_some_and(|d| d.contains("->")) {
            Some(CompletionItemKind::FUNCTION)
        } else {
            Some(CompletionItemKind::VARIABLE)
        };
        let range = lsp::range_from_byte_range(&self.source, &self.line_starts, replace);
        CompletionItem {
            label: name.clone(),
            kind,
            detail,
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: name.clone(),
            })),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            ..Default::default()
        }
    }

    /// The informational `detail` for a **built-in** name: the built-in's own
    /// file, since no checked type is available here.
    fn builtin_detail(&self, name: &str) -> Option<String> {
        let def = &self.builtin_names.iter().find(|b| b.name == name)?.def;
        Some(match &def.file {
            Some(file) => format!("built-in (from `{}`)", source_name(file)),
            None => "built-in".to_string(),
        })
    }

    /// The informational `detail` for a name the **document** binds.
    fn doc_detail(&self, span: Span) -> Option<String> {
        if let Some(i) = self.import_by_span.get(&span) {
            let imp = &self.imports[*i];
            return Some(match &imp.ty {
                Some(ty) => format!("imported module : {ty}"),
                None => format!("imported module (from `{}`)", imp.path),
            });
        }
        let def = self.defs.get(self.def_index.get(&span).copied()?)?;
        let stmt_i = self.stmt_by_span.get(&def.span).copied()?;
        let sv = &self.statements[stmt_i];
        (!sv.ty.is_empty()).then(|| sv.ty.clone())
    }

    /// When the hovered name is a field access, the field's own `value : type`.
    ///
    /// # Invariant
    /// Detected by a preceding `.` whose container resolves to a knowable struct; a
    /// field is then never reported as "unresolved".
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

        // An imported-module container: the module's field table. A
        // built-in container has no directive to key on.
        if let Some(d) = container_def
            && let Some(doc_index) = d.document()
            && let Some(import_i) = self.import_by_span.get(&self.defs[doc_index].span).copied()
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
    /// own frontend.
    ///
    /// # Invariant
    /// The leading `---…---` preprocessor block is the language's only
    /// prose-like construct, so it is the only span colored as a comment.
    pub fn semantic_tokens(&self) -> Vec<SemanticTokenData> {
        let mut out = Vec::new();

        // The preprocessor block, split per line: clients require single-line
        // tokens, so no one token may span a newline.
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

        // Name classifications the AST must infer: a lambda parameter,
        // or a name used in function position.
        let name_class = classify_names(&self.program);

        // Walk the token stream; tokens are already in document order.
        let mut prev_was_dot = false;
        for t in self.tokens.iter() {
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
        Stage::Preprocess
        | Stage::Lex
        | Stage::Parse
        | Stage::Resolve
        | Stage::Check
        | Stage::Io => DiagnosticSeverity::ERROR,
    }
}

/// Render a `value : type` hover snapshot: a concrete value, else just the
/// type, else the caller's `fallback`.
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

/// The hover text for a use of an imported module: the module's checked
/// type, else a description naming its file.
fn import_hover(name: &str, imp: &ImportBinding) -> String {
    match &imp.ty {
        Some(ty) => format!("`{name}` — imported module : {ty}"),
        None => format!("`{name}` — imported module (from `{}`)", imp.path),
    }
}

/// The hover text for a name resolving to a **built-in** module's file: the
/// name and the line in *that* file.
///
/// # Invariant
/// No `value : type` is rendered: the statement snapshot is that module's build,
/// which this analysis does not hold (`docs/notes/core-prelude.md` §5).
fn builtin_hover(name: &str, def: &Definition) -> String {
    match &def.file {
        Some(file) => format!(
            "`{name}` — defined at line `{}` of `{}`",
            def.span.0,
            source_name(file)
        ),
        None => format!("`{name}` — defined at line `{}`", def.span.0),
    }
}

/// A file's display name for a hover or a completion `detail`: its file name
/// (`core.lichen`), else the whole path.
fn source_name(source: &PackageSource) -> String {
    source
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.path.display().to_string())
}

/// The type of the named field `field` inside a rendered `struct<...>` type.
///
/// # Invariant
/// A field-access IR node's own type slot is a lazy cell, so an access on an
/// imported module resolves its type from the module's rendered type instead.
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

/// The substring up to the `>` that closes an already-stripped `struct<...>`.
/// The `>` of an arrow `->` is not a close.
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

/// The token kinds needing no AST context; `classify_names` disambiguates the
/// `Name`s.
fn classify_token_kind(
    kind: &TokenKind,
) -> Option<(SemanticTokenType, Vec<SemanticTokenModifier>)> {
    match kind {
        TokenKind::Int(_) => Some((SemanticTokenType::NUMBER, Vec::new())),
        TokenKind::Float(_) => Some((SemanticTokenType::NUMBER, Vec::new())),
        TokenKind::Str(_) => Some((SemanticTokenType::STRING, Vec::new())),
        // The builtin type constants are type-ish, not keywords.
        TokenKind::KwInt | TokenKind::KwFloat | TokenKind::KwString | TokenKind::KwType => {
            Some((SemanticTokenType::TYPE, Vec::new()))
        }
        TokenKind::KwStruct
        | TokenKind::KwTable
        | TokenKind::KwSet
        | TokenKind::KwArray
        | TokenKind::KwLet
        | TokenKind::KwIf
        | TokenKind::KwThen
        | TokenKind::KwElse
        | TokenKind::KwReturn
        | TokenKind::KwPub
        | TokenKind::KwCache
        | TokenKind::KwLoop
        // The class conversions and the assert are prefix operators the lexer
        // reserves words for.
        | TokenKind::KwInt2Float
        | TokenKind::KwFloat2Int
        | TokenKind::KwAssert
        // `@in` is the one keyword that is an *infix* operator.
        | TokenKind::KwIn
        // `_` — a placeholder is a reserved inference form, never a name.
        | TokenKind::Placeholder => Some((SemanticTokenType::KEYWORD, Vec::new())),
        // A `Name` is resolved by `classify_names` (or the `.` heuristic).
        TokenKind::Name(_) => None,
        // Operators: arrows, annotations, separators-of-fields, and math.
        TokenKind::Arrow
        | TokenKind::FatArrow
        | TokenKind::TableArrow
        | TokenKind::Colon
        | TokenKind::DoubleColon
        | TokenKind::Hash
        | TokenKind::Question
        | TokenKind::Bang
        | TokenKind::Dollar
        | TokenKind::Equals
        | TokenKind::Eq
        | TokenKind::Neq
        | TokenKind::Leq
        | TokenKind::Geq
        | TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::Amp
        | TokenKind::Pipe
        | TokenKind::Caret
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
    let top_stmts: Vec<Stmt> = program
        .statements
        .iter()
        .map(|bs| bs.stmt.clone())
        .collect();
    w.stmts(&top_stmts);
    if let Some(e) = &program.expr {
        w.expr(e);
    }
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

    // SAFETY: `#[stacksafe]` grows the stack per nested expression instead of
    // overflowing the process.
    #[stacksafe]
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
                if let Expr::Name(_, span, _) = &**function {
                    self.map
                        .insert(*span, (SemanticTokenType::FUNCTION, Vec::new()));
                }
                self.expr(function);
                self.expr(argument);
            }
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::TypeConst(..)
            | Expr::Name(..)
            | Expr::Placeholder(..)
            | Expr::Err { .. } => {}
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
            Expr::Convert { value, .. } => self.expr(value),
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
            Expr::RawNamedField { container, .. } => self.expr(container),
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
            Expr::Tuple(elems, _)
            | Expr::TypeTuple(elems, _)
            | Expr::Array(elems, _)
            | Expr::Set(elems, _) => {
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
            Expr::RawIndex {
                container, index, ..
            } => {
                self.expr(container);
                self.expr(index);
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

// Name resolution over the AST, mirroring the compiler's scope rules.

fn index_names(
    program: &Program,
    imports: &[&ResolvedImport],
    builtins: &[BuiltinName],
) -> (
    Vec<Definition>,
    HashMap<Span, ScopeValue>,
    HashMap<Span, usize>,
) {
    let mut walk = Walk {
        defs: Vec::new(),
        scopes: Vec::new(),
        resolve: HashMap::new(),
        def_index: HashMap::new(),
    };
    // Base scope frames: imports below the bindings, then the built-ins —
    // the `core` prelude, needing no import.

    // A built-in's definition is in another file, so it stays out of
    // `def_index`: only a *use* resolves to one.
    walk.scopes.push(HashMap::new());
    for imp in imports {
        walk.enter(&imp.name, imp.span);
    }
    walk.scopes.push(HashMap::new());
    for (index, builtin) in builtins.iter().enumerate() {
        walk.scopes
            .last_mut()
            .expect("a scope frame is pushed")
            .insert(builtin.name.clone(), ScopeValue::Builtin(index));
    }
    // The top level is a block; `pub` is irrelevant to name resolution.
    let top_stmts: Vec<Stmt> = program
        .statements
        .iter()
        .map(|bs| bs.stmt.clone())
        .collect();
    walk.scope(&top_stmts, program.expr.as_ref());
    (walk.defs, walk.resolve, walk.def_index)
}

/// What a name in scope resolves to: an index into [`DocIndex::defs`],
/// or into [`DocIndex::builtin_names`].
///
/// # Invariant
/// The distinction is the whole point: the two have different coordinates, so a
/// built-in definition must never be read as a document index.
#[derive(Clone, Copy, Debug)]
enum ScopeValue {
    Document(usize),
    Builtin(usize),
}

impl ScopeValue {
    /// The index into [`DocIndex::defs`], when this is a document definition.
    fn document(self) -> Option<usize> {
        match self {
            ScopeValue::Document(i) => Some(i),
            ScopeValue::Builtin(_) => None,
        }
    }

    /// The index into [`DocIndex::builtin_names`], when this is a name a
    /// **built-in** module exposes.
    fn builtin(self) -> Option<usize> {
        match self {
            ScopeValue::Builtin(i) => Some(i),
            ScopeValue::Document(_) => None,
        }
    }

    /// The definition itself, read from the document it belongs to.
    fn definition(self, index: &DocIndex) -> Definition {
        match self {
            ScopeValue::Document(i) => index.defs[i].clone(),
            ScopeValue::Builtin(i) => index.builtin_names[i].def.clone(),
        }
    }
}

struct Walk {
    defs: Vec<Definition>,
    scopes: Vec<HashMap<String, ScopeValue>>,
    resolve: HashMap<Span, ScopeValue>,
    def_index: HashMap<Span, usize>,
}

impl Walk {
    /// Enter a definition of this document at `span`: pushed onto
    /// [`DocIndex::defs`].
    fn enter(&mut self, name: &str, span: Span) -> usize {
        let idx = self.defs.len();
        self.defs.push(Definition {
            name: name.to_string(),
            span,
            file: None,
        });
        self.def_index.insert(span, idx);
        self.scopes
            .last_mut()
            .expect("a scope frame is pushed")
            .insert(name.to_string(), ScopeValue::Document(idx));
        idx
    }

    fn lookup(&self, name: &str) -> Option<ScopeValue> {
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

    // SAFETY: `#[stacksafe]` grows the stack per nested expression instead of
    // overflowing the process.
    #[stacksafe]
    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::TypeConst(..)
            | Expr::Placeholder(..)
            | Expr::Err { .. } => {}
            Expr::Name(name, span, _) => {
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
            Expr::Convert { value, .. } => self.expr(value),
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
            Expr::RawNamedField { container, .. } => self.expr(container),
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
            Expr::Tuple(elems, _)
            | Expr::TypeTuple(elems, _)
            | Expr::Array(elems, _)
            | Expr::Set(elems, _) => {
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
            Expr::RawIndex {
                container, index, ..
            } => {
                self.expr(container);
                self.expr(index);
            }
            Expr::Block {
                statements, expr, ..
            } => self.scope(statements, Some(expr)),
            Expr::RecordBlock { fields, .. } => {
                // A struct-returning block scopes its field statements like a block:
                // a named field is a binding, so its names resolve.
                let stmts: Vec<Stmt> = fields
                    .iter()
                    .map(|f| match &f.name {
                        Some(name) => Stmt::Binding(lichen_language::ast::Binding {
                            name: name.clone(),
                            binder: f.binder,
                            value: f.value.clone(),
                            span: f.span,
                            restrictive: !f.field,
                            cached: f.cached,
                            looping: f.looping,
                        }),
                        None => Stmt::Expr(f.value.clone()),
                    })
                    .collect();
                self.scope(&stmts, None);
            }
        }
    }
}

// In-scope names at a position, for completion: [`index`]'s scope
// rules again, visited in source order.

/// One name in scope at the cursor: the name and the definition **span** when
/// this document binds it.
#[derive(Clone, Debug)]
struct ScopedName {
    name: String,
    /// The document definition site's span, or `None` for a built-in's name.
    document: Option<Span>,
}

/// The names in scope at byte `offset`, innermost scope first, deduplicated.
fn scope_names_at(
    program: &Program,
    imports: &[(String, Span)],
    builtins: &[String],
    line_starts: &[usize],
    offset: usize,
) -> Vec<ScopedName> {
    let mut w = ScopeCapture {
        scopes: Vec::new(),
        line_starts,
        offset,
        result: None,
    };
    // Base scope frames, mirroring [`index`]: imports, then the built-ins
    // they expose; document bindings shadow both.
    w.scopes.push(HashMap::new());
    for (name, span) in imports {
        w.scopes
            .last_mut()
            .expect("a scope frame is pushed")
            .insert(
                name.clone(),
                ScopedName {
                    name: name.clone(),
                    document: Some(*span),
                },
            );
    }
    w.scopes.push(HashMap::new());
    for name in builtins {
        w.scopes
            .last_mut()
            .expect("a scope frame is pushed")
            .insert(
                name.clone(),
                ScopedName {
                    name: name.clone(),
                    document: None,
                },
            );
    }
    let top_stmts: Vec<Stmt> = program
        .statements
        .iter()
        .map(|bs| bs.stmt.clone())
        .collect();
    w.scope(&top_stmts, program.expr.as_ref());
    w.result.unwrap_or_default()
}

/// The scope-stack snapshot used by [`scope_names_at`].
struct ScopeCapture<'a> {
    /// The active scope frames, innermost last; each frame is name → name.
    scopes: Vec<HashMap<String, ScopedName>>,
    line_starts: &'a [usize],
    offset: usize,
    /// The captured in-scope names, once the walk reaches the offset.
    result: Option<Vec<ScopedName>>,
}

impl<'a> ScopeCapture<'a> {
    fn enter(&mut self, name: &str, span: Span) {
        self.scopes
            .last_mut()
            .expect("a scope frame is pushed")
            .insert(
                name.to_string(),
                ScopedName {
                    name: name.to_string(),
                    document: Some(span),
                },
            );
    }

    /// At the first node whose start is at/past the cursor, snapshot the scope.
    fn check(&mut self, span: Span) {
        if self.result.is_some() {
            return;
        }
        if lsp::offset_of_span(self.line_starts, span) >= self.offset {
            self.capture();
        }
    }

    fn capture(&mut self) {
        if self.result.is_some() {
            return;
        }
        let mut out: Vec<ScopedName> = Vec::new();
        for frame in self.scopes.iter().rev() {
            for (name, entry) in frame {
                if !out.iter().any(|existing| existing.name == *name) {
                    out.push(entry.clone());
                }
            }
        }
        self.result = Some(out);
    }

    fn scope(&mut self, statements: &[Stmt], expr: Option<&Expr>) {
        if self.result.is_some() {
            return;
        }
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
            if self.result.is_some() {
                break;
            }
        }
        if self.result.is_none()
            && let Some(e) = expr
        {
            self.expr(e);
        }
        self.scopes.truncate(base);
    }

    fn stmt(&mut self, s: &Stmt) {
        if self.result.is_some() {
            return;
        }
        match s {
            Stmt::Binding(b) => {
                self.expr(&b.value);
                if self.result.is_some() {
                    return;
                }
                if b.restrictive {
                    self.scopes.push(HashMap::new());
                    self.enter(&b.name, b.span);
                }
            }
            Stmt::Expr(e) => self.expr(e),
        }
    }

    // SAFETY: `#[stacksafe]` grows the stack per nested expression instead of
    // overflowing the process.
    #[stacksafe]
    fn expr(&mut self, e: &Expr) {
        if self.result.is_some() {
            return;
        }
        self.check(e.span());
        if self.result.is_some() {
            return;
        }
        match e {
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::TypeConst(..)
            | Expr::Placeholder(..)
            | Expr::Err { .. }
            | Expr::Name(..) => {}
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
            Expr::Convert { value, .. } => self.expr(value),
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
            Expr::RawNamedField { container, .. } => self.expr(container),
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
            Expr::Tuple(elems, _)
            | Expr::TypeTuple(elems, _)
            | Expr::Array(elems, _)
            | Expr::Set(elems, _) => {
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
            Expr::RawIndex {
                container, index, ..
            } => {
                self.expr(container);
                self.expr(index);
            }
            Expr::Block {
                statements, expr, ..
            } => self.scope(statements, Some(expr)),
            Expr::RecordBlock { fields, .. } => {
                let stmts: Vec<Stmt> = fields
                    .iter()
                    .map(|f| match &f.name {
                        Some(name) => Stmt::Binding(Binding {
                            name: name.clone(),
                            binder: f.binder,
                            value: f.value.clone(),
                            span: f.span,
                            restrictive: !f.field,
                            cached: f.cached,
                            looping: f.looping,
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

    fn doc(source: &str) -> Doc<lichen_language::program::LangProgram> {
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
    fn unresolved_name_with_a_close_candidate_suggests_it() {
        // The block-wide binding `unknown` is in scope at the `unkown` use.
        let d = doc("unknown = 1\nunkown");
        let msgs: Vec<String> = d
            .lsp_diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        assert!(
            msgs.iter().any(|m| m.contains("did you mean 'unknown'?")),
            "expected a did-you-mean suggestion, got {msgs:?}"
        );
    }

    #[test]
    fn unresolved_name_without_a_close_candidate_stays_plain() {
        // `y` shares no first character with the in-scope `x`, so the message
        // stays plain.
        let d = doc("x => y");
        let msgs: Vec<String> = d
            .lsp_diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        assert!(
            msgs.iter().any(|m| m == "unresolved name 'y'"),
            "expected the plain message, got {msgs:?}"
        );
    }

    #[test]
    fn completion_offers_in_scope_names_filtered_by_the_prefix() {
        // Typing `a` at the tail offers the block-wide `aa` (in scope) but not
        // `bb` (different prefix).
        let d = doc("aa = 1\nbb = 2\na");
        let items = d.completion_at(Position {
            line: 2,
            character: 0,
        });
        let labels: Vec<String> = items.iter().map(|i| i.label.clone()).collect();
        assert!(
            labels.contains(&"aa".to_string()),
            "expected `aa` in the completion, got {labels:?}"
        );
        assert!(
            !labels.contains(&"bb".to_string()),
            "`bb` should be filtered by the prefix, got {labels:?}"
        );
    }

    #[test]
    fn completion_is_scope_aware_a_block_local_is_not_offered_outside() {
        // Inside the block, `inner` is in scope and completes.
        let d = doc("a = 1\n{inner = 2; inner}\ninner");
        let inside = d.completion_at(Position {
            line: 1,
            character: 12,
        });
        let inside_labels: Vec<String> = inside.iter().map(|i| i.label.clone()).collect();
        assert!(
            inside_labels.contains(&"inner".to_string()),
            "`inner` should complete inside its block, got {inside_labels:?}"
        );
        // After the block closes, `inner` is out of scope.
        let outside = d.completion_at(Position {
            line: 2,
            character: 0,
        });
        let outside_labels: Vec<String> = outside.iter().map(|i| i.label.clone()).collect();
        assert!(
            !outside_labels.contains(&"inner".to_string()),
            "`inner` must not be offered outside its block, got {outside_labels:?}"
        );
    }

    #[test]
    fn completion_items_carry_a_worked_type_detail() {
        // The binding `x = 3` has a concrete type `Int`, which the completion
        // surfaces as `detail`.
        let d = doc("x = 3\nx");
        let items = d.completion_at(Position {
            line: 1,
            character: 0,
        });
        assert!(
            items
                .iter()
                .any(|i| i.label == "x" && i.detail.as_deref() == Some("Int")),
            "expected a `x` completion with `Int` detail, got {items:?}"
        );
    }

    #[test]
    fn field_access_error_suggests_a_close_field() {
        // `point.sux` reads a field the container lacks; the diagnostic
        // appends the closest field name as a did-you-mean clause.
        let d = doc("point = { x = 1, y = 2, sub = 3 }\npoint.sux\n");
        let msgs: Vec<String> = d
            .lsp_diagnostics()
            .iter()
            .map(|d| d.message.clone())
            .collect();
        assert!(
            msgs.iter()
                .any(|m| m.contains("no field") && m.contains("did you mean 'sub'?")),
            "expected a field-access did-you-mean, got {msgs:?}"
        );
    }

    #[test]
    fn completion_after_a_dot_offers_the_structs_fields() {
        // Typing `point.s` offers the struct's own field `sub`, never a bare name.
        let d = doc("point = { x = 1, y = 2, sub = 3 }\npoint.s\n");
        let items = d.completion_at(Position {
            line: 1,
            character: 7,
        });
        let labels: Vec<String> = items.iter().map(|i| i.label.clone()).collect();
        assert!(
            labels.contains(&"sub".to_string()),
            "expected the struct field `sub`, got {labels:?}"
        );
        assert!(
            !labels.contains(&"point".to_string()),
            "bare names are not offered after a dot, got {labels:?}"
        );
        assert!(
            !labels.contains(&"x".to_string()),
            "a field with a different prefix is filtered, got {labels:?}"
        );
    }

    #[test]
    fn field_completion_is_filtered_by_the_typed_prefix() {
        // Typing `y` after `point.` offers only the field(s) sharing that prefix.
        let d = doc("point = { x = 1, y = 2, sub = 3 }\npoint.y\n");
        let items = d.completion_at(Position {
            line: 1,
            character: 7,
        });
        let labels: Vec<String> = items.iter().map(|i| i.label.clone()).collect();
        assert!(
            labels.contains(&"y".to_string()) && !labels.contains(&"x".to_string()),
            "the field completion should be prefix-filtered, got {labels:?}"
        );
    }

    #[test]
    fn completion_after_a_module_dot_offers_the_modules_fields() {
        // `math.s` after the import: the module's *own* fields, prefix-filtered.
        let dir = temp_dir("modcomp");
        write(
            &dir,
            "math.lichen",
            "{\n  succ = x => x + 1\n  add = x => y => x + y\n}\n",
        );
        let main_path = write(
            &dir,
            "main.lichen",
            "---\n  math = import \"math.lichen\"\n---\nmath.s\n",
        );
        let d: Doc<lichen_language::program::LangProgram> = Doc::new_with_base(
            fs::read_to_string(&main_path).unwrap(),
            Some(main_path.as_path()),
        );
        let items = d.completion_at(Position {
            line: 3,
            character: 6,
        });
        let labels: Vec<String> = items.iter().map(|i| i.label.clone()).collect();
        assert!(
            labels.contains(&"succ".to_string()),
            "expected the module field `succ`, got {labels:?}"
        );
        assert!(
            labels.iter().all(|l| l.starts_with('s')),
            "the module completion should be prefix-filtered, got {labels:?}"
        );
        assert!(
            !labels.contains(&"math".to_string()),
            "the import binding is not a field, got {labels:?}"
        );
    }

    #[test]
    fn clean_source_has_no_diagnostics_and_one_binding() {
        let d = doc("a = 1\n(a, a)");
        assert!(d.diagnostics.is_empty(), "got {:?}", d.diagnostics);
        assert_eq!(d.defs.len(), 1, "one binding");
        // The prelude's names are in scope but are **not** this document's
        // bindings: they live apart from `defs`.
        assert!(
            d.builtin_names.iter().any(|b| b.name == "add"),
            "the prelude's `add` should be seeded, got {:?}",
            d.builtin_names
                .iter()
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_prelude_name_use_jumps_into_the_builtins_own_file() {
        // `add` has no binding and no import: the prelude is seeded, and a
        // use resolves to the built-in's file.
        let d = doc("add [1, 2]\n");
        let def = d
            .definition_at(Position {
                line: 0,
                character: 0,
            })
            .expect("definition for the prelude's `add`");
        let file = def
            .file
            .as_ref()
            .expect("`add` is defined in a built-in file");
        assert_eq!(
            file.path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
            Some("core.lichen".to_string()),
            "the jump should land in the built-in's file"
        );
        assert!(
            file.code.contains("add = operands"),
            "the built-in's text is the record's: got {:?}",
            file.code
        );
        // The position is the built-in's own, in the built-in's coordinates.
        let range = d.definition_range(&def);
        assert_eq!(range.start.line, 2, "`add` is on line 3 of core.lichen");

        // A failure *inside* the built-in belongs to its file, not the document
        // (`docs/notes/core-prelude.md` §4).
        let failing = doc("add [\"a\", \"b\"]\n");
        let files = failing.file_diagnostics();
        assert!(
            files.iter().any(|file| {
                file.path.file_name().is_some_and(|n| n == "core.lichen")
                    && file
                        .diagnostics
                        .iter()
                        .any(|d| d.message.contains("assertion failed"))
            }),
            "expected the built-in's failure on the built-in's file, got {files:?}"
        );
    }

    #[test]
    fn preprocessor_block_is_cut_out() {
        // A real lichen file opens with an `---…---` block; it must not leak into
        // the lexer or parser as code.
        let d = doc("--- order = \"1\"\noutput = \"3: Int\"\n---\na = 1\nb = 2\na + b\n");
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
        // there is no `value : type` snapshot.
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
        let def = d
            .definition_at(Position {
                line: 2,
                character: 0,
            })
            .expect("definition for the final `b`");
        assert!(def.file.is_none(), "`b` is this document's binding");
        let range = d.definition_range(&def);
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
        let d = doc("--- order = \"1\"\noutput = \"3: Int\"\n---\na = 1\nb = 2\na + b\n");
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
        // The `import` example as an editor would open it: the document's URI
        // is the base for `@import`.
        let dir = temp_dir("import");
        write(
            &dir,
            "math.lichen",
            "---output = \"(Function, Function): struct<.succ Int -> Int, .add Int -> Int -> Int>\"---\n{\n  succ = x => x + 1\n  add = x => y => x + y\n}\n",
        );
        write(
            &dir,
            "geometry.lichen",
            "---math = import \"math.lichen\"\noutput = \"(Function, Function): struct<.double Int -> Int, .inc_twice Int -> Int>\"---\n{\n  double = x => math.add x x\n  inc_twice = x => math.succ (math.succ x)\n}\n",
        );
        let main_path = write(
            &dir,
            "_.lichen",
            "---order = \"5\"\nmath = import \"math.lichen\"\ngeo = import \"geometry.lichen\"\noutput = \"(42, 10, 7): <Int, Int, Int>\"---\n(math.succ 41, geo.double 5, geo.inc_twice 5)\n",
        );

        let d: Doc<lichen_language::program::LangProgram> = Doc::new_with_base(
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
    fn new_with_cache_writes_imports_to_the_lichen_home() {
        // The settled imports are compiled into the device cache at `cache_root`.
        let dir = temp_dir("cachewrite");
        write(&dir, "math.lichen", "{\n  succ = x => x + 1\n}\n");
        let main_path = write(
            &dir,
            "main.lichen",
            "---\n  math = import \"math.lichen\"\n  output = \"(42): Int\"\n---\nmath.succ 41\n",
        );
        let cache = temp_dir("cachehome");

        let d: Doc<lichen_language::program::LangProgram> = Doc::new_with_cache(
            fs::read_to_string(&main_path).unwrap(),
            Some(main_path.as_path()),
            Some(cache.as_path()),
        );
        assert!(
            d.diagnostics.is_empty(),
            "imports should resolve through the cache; got {:?}",
            d.diagnostics
        );

        // The compiled package was serialized into the home: a registry file and
        // a `.module` artifact under `artifacts/`.
        assert!(
            cache.join("registry").is_file(),
            "the cache registry should be written"
        );
        let artifacts = cache.join("artifacts");
        let any_module = fs::read_dir(&artifacts)
            .expect("artifacts dir should exist")
            .filter_map(|e| e.ok())
            .any(|e| e.path().extension().is_some_and(|ext| ext == "module"));
        assert!(any_module, "a compiled package artifact should be cached");
    }

    #[test]
    fn hover_resolves_imports_and_their_fields() {
        // Hovering an imported module (or a field of it) must not say
        // "unresolved name".
        let dir = temp_dir("hoverimport");
        write(
            &dir,
            "math.lichen",
            "{\n  succ = x => x + 1\n  add = x => y => x + y\n}\n",
        );
        let main_path = write(
            &dir,
            "main.lichen",
            "---\n  math = import \"math.lichen\"\n---\nmath.succ 41\n",
        );
        let d: Doc<lichen_language::program::LangProgram> = Doc::new_with_base(
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

        // Go-to-definition on the module use jumps to the import directive.
        let def = d
            .definition_at(Position {
                line: 3,
                character: 0,
            })
            .expect("def on `math`");
        assert!(def.file.is_none(), "`math` is this document's import");
        assert_eq!(
            d.definition_range(&def).start,
            Position {
                line: 1,
                character: 2
            }
        );
    }

    #[test]
    fn imported_field_access_hovers_with_value_and_type() {
        // The repo's living spec `import/_.lichen`: an accessed field
        // of an imported module renders its `value : type`.
        let examples =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import");
        let main_path = examples.join("_.lichen");
        let d: Doc<lichen_language::program::LangProgram> = Doc::new_with_base(
            fs::read_to_string(&main_path).unwrap(),
            Some(main_path.as_path()),
        );
        assert!(
            d.diagnostics.is_empty(),
            "repo import example should check clean; got {:?}",
            d.diagnostics
        );

        // Line 7 (0-based line 6): `(math.succ 41, geo.double 5, geo.inc_twice 5)`.
        for (pos, expected) in [
            // `.succ` field access on `math`
            (
                Position {
                    line: 6,
                    character: 6,
                },
                "Function : Int -> Int",
            ),
            // `.inc_twice` field access on `geo`
            (
                Position {
                    line: 6,
                    character: 33,
                },
                "Function : Int -> Int",
            ),
        ] {
            let (msg, _) = d.hover_at(pos).expect("hover on an imported field access");
            assert!(
                msg.contains(expected),
                "field access hover msg = {msg}, expected to contain {expected}"
            );
            assert!(
                !msg.contains("unresolved") && !msg.contains("field of imported module"),
                "field access hover msg = {msg}"
            );
        }

        // `.double` is `x => math.add x x` over a refined `add` whose class the body never pins.

        // Both sides are one open cell (`docs/notes/tests-do-not-render.md`).
        let (msg, _) = d
            .hover_at(Position {
                line: 6,
                character: 19,
            })
            .expect("hover on an imported field access");
        assert!(
            !msg.contains("unresolved") && !msg.contains("field of imported module"),
            "field access hover msg = {msg}"
        );
        let rendered = msg
            .split_once("Function : ")
            .map(|(_, ty)| ty)
            .unwrap_or_else(|| panic!("the field renders as a value : type; got {msg}"));
        let sides: Vec<&str> = rendered
            .split("->")
            .map(|side| side.trim().trim_end_matches('`').trim())
            .collect();
        assert_eq!(sides.len(), 2, "one arrow, domain to codomain: {msg}");
        assert_eq!(
            sides[0], sides[1],
            "the domain and codomain are the same open cell: {msg}"
        );
        assert!(
            sides[0].contains('?'),
            "the cell is an open placeholder, not a decided type: {msg}"
        );
    }

    #[test]
    fn local_struct_field_access_hovers_with_value_and_type() {
        // A field access on a *local* struct binding renders the field's
        // value:type, from this file's struct-block field table.
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
        // Inside a module file (`{succ = …, add = …}`), hovering a record field
        // definition must not say "unresolved name".
        let dir = temp_dir("modulefields");
        let math_path = write(
            &dir,
            "math.lichen",
            "{\n  succ = x => x + 1\n  add = x => y => x + y\n}\n",
        );
        let d: Doc<lichen_language::program::LangProgram> = Doc::new_with_base(
            fs::read_to_string(&math_path).unwrap(),
            Some(math_path.as_path()),
        );

        // `succ` at line 1, char 2; `add` at line 2, char 2.
        for (line, name) in [(1usize, "succ"), (2, "add")] {
            let (msg, _) = d
                .hover_at(Position {
                    line: line as u32,
                    character: 2,
                })
                .unwrap_or_else(|| panic!("hover on `{name}`"));
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
        // With `base = None` the imports resolve against the process CWD.
        let d = doc("---math = import \"math.lichen\"---math\n");
        assert!(
            d.diagnostics
                .iter()
                .any(|x| x.message.contains("cannot load package 'math.lichen'")),
            "expected a cannot-load diagnostic, got {:?}",
            d.diagnostics
        );
    }
}
