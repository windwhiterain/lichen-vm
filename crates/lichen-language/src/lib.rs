//! The minimal source language: text → highlevel IR → checked program.
//!
//! # Invariant
//!
//! The frontend is total and never panics: lex errors accumulate, parse errors
//! recover, and an unresolved name lowers to an inert error block, so one pass
//! reports every problem it can find. See docs/language-spec.md.

// The lexer and parser live in their own crates; re-export them for old paths.
pub use lichen_language_lex as lex;
pub use lichen_language_lex::{LexDiag, Span};
pub use lichen_language_parser as parse;
pub use lichen_language_parser::ast;
pub use lichen_language_parser::path;
pub use lichen_language_parser::{ParseDiag, Parsed};

pub mod cells;
pub mod compile;
pub mod diag;
mod dirty;
pub mod package;
pub mod persist;
pub mod preprocess;
pub mod program;
pub mod render;
pub mod resolve;
pub mod run;
pub mod session;
mod spans;
pub mod suggest;

use std::sync::{Arc, PoisonError, RwLock};

use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::ir::{ExprId, IR};
use lichen_highlevel::program::{
    HighGlobalExt, HighProgram, HighProgramLiteral, TypeOperator, ValueType,
};
use lichen_highlevel::{NativeOps, no_native_ops};
use lichen_lowlevel::{Registry, StaticNodeId};

use crate::cells::CellStore;
use crate::path::Path;
use crate::persist::ProgramCodecOf;
pub use diag::{Diag, Stage};
use preprocess::ResolvedImport;
use program::{GcdOp, LangProgram, lang_attr_ext};

/// A program the tooling drives: `ProgramCodecOf` plus the language's shapes.
pub trait LangProgramShape:
    ProgramCodecOf
    + ::lichen_highlevel::program::HighProgram<
        Attr = program::LangAttr,
        Literal = ::lichen_highlevel::program::HighProgramLiteral,
    > + ::lichen_lowlevel::Program<PackageMeta = ::lichen_highlevel::program::HighPackageMeta>
    + 'static
{
}

impl<T> LangProgramShape for T
where
    T: ProgramCodecOf,
    T: ::lichen_highlevel::program::HighProgram<
            Attr = program::LangAttr,
            Literal = ::lichen_highlevel::program::HighProgramLiteral,
        >,
    T: ::lichen_lowlevel::Program<PackageMeta = ::lichen_highlevel::program::HighPackageMeta>,
    T: 'static,
{
}

/// The inner program shape `LangProgram` wraps: `LangAttr` fixed, `V`/`O` varying.
pub type CompiledProgram<V, O> = lichen_highlevel::program::ProgramImpl<
    V,
    O,
    program::LangAttr,
    HighProgramLiteral,
    HighGlobalExt,
>;

/// The version of `lichen-language`, which the package manager keys its compiler cache by.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The result of compiling and checking a source program.
///
/// # Invariant
///
/// `build` is `Some` on every path this crate produces; `None` is reserved for
/// a caller of [`build_report`] that has no IR to check.
pub struct Report<P: HighProgram>
where
    P::Value: ValueType,
{
    pub build: Option<Build<P>>,
    pub diagnostics: Vec<Diag<P>>,
    /// The source span of each IR node, keyed by [ExprId]; present with `build`.
    pub span_index: Option<compile::SpanIndex>,
}

impl<P: HighProgram> Report<P>
where
    P::Value: ValueType,
{
    /// No errors and the program checked: no diagnostics at all and the
    /// checker reported no unification failures.
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty() && self.build.as_ref().is_some_and(|b| b.ok)
    }
}

/// Compile and check a source program with a **cell store**.
///
/// # Invariant
///
/// A clean cell's body is not lowered, checked or evaluated; a marked binding
/// that is compiled is frozen into `registry` and recorded under its path.
/// `source_id` names the file, and [`CellStore::invalidate_source`] drops it.
/// See docs/notes/incremental-update.md §6.1.
pub fn compile_with_cells(
    source_id: &str,
    source: &str,
    cells: &mut CellStore,
    registry: Arc<RwLock<Registry<LangProgram>>>,
) -> Report<LangProgram> {
    let line_starts = lex::line_starts(source);
    compile_with_imports_at_with_cells::<LangProgram>(
        source,
        &[],
        Some(registry),
        0,
        &line_starts,
        no_native_ops(),
        Some(cells),
        Vec::new(),
        source_id,
    )
}

/// Freeze the marked bindings a build compiled, and record each under its path.
///
/// # Invariant
///
/// Only a solved pair of a clean build is retained: an undecided one has no
/// answer to keep, and a cell frozen from a failed build would carry that
/// failure's silence once its body is skipped on the read back.
fn freeze_cells<P>(
    build: &Build<P>,
    cells: &mut CellStore,
    registry: &Arc<RwLock<Registry<P>>>,
    compiled_cells: Vec<(Path, ExprId)>,
    source_id: &str,
) where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    if !build.ok {
        return;
    }
    let mut registry = registry.write().unwrap_or_else(PoisonError::into_inner);
    for (path, expr) in compiled_cells {
        let Some(pair) = build.state[expr.0 as usize].term else {
            continue;
        };
        if build.module.class_value(pair).is_none() {
            continue;
        }
        let key = registry.allocate_cell_key();
        // The hash is a placeholder: reuse is decided by path, never content.
        let freeze = registry.freeze_closure_mapped(&build.module, key, &[pair], [0; 32]);
        let index = freeze.node_map[&pair];
        cells.record(path, source_id, StaticNodeId { module: key, index });
    }
}

/// Compile and check a source program: the full pipeline (shipping vocabulary).
pub fn compile(source: &str) -> Report<LangProgram> {
    compile_with_imports(source, &[])
}

/// Compile and check a source program with resolved package imports
/// (shipping vocabulary).
pub fn compile_with_imports(source: &str, imports: &[ResolvedImport]) -> Report<LangProgram> {
    compile_with_imports_in(source, imports, None)
}

/// [`compile_with_imports`] with an optional shared registry.
pub fn compile_with_imports_in(
    source: &str,
    imports: &[ResolvedImport],
    registry: Option<Arc<RwLock<Registry<LangProgram>>>>,
) -> Report<LangProgram> {
    let line_starts = lex::line_starts(source);
    compile_with_imports_at::<LangProgram>(
        source,
        imports,
        registry,
        0,
        &line_starts,
        no_native_ops(),
    )
}

/// Compile and check a program over any `P`, as a slice of a larger source.
///
/// # Invariant
///
/// Token spans are absolute positions in the larger source, so diagnostics
/// point at the real file even when `code` is only a suffix.
pub fn compile_with_imports_at<P>(
    code: &str,
    imports: &[ResolvedImport],
    registry: Option<Arc<RwLock<Registry<P>>>>,
    base: u32,
    line_starts: &[usize],
    native_ops: NativeOps<P>,
) -> Report<P>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    compile_with_imports_at_with_cells(
        code,
        imports,
        registry,
        base,
        line_starts,
        native_ops,
        None,
        Vec::new(),
        "",
    )
}

/// [`compile_with_imports_at`] with a **cell store**.
///
/// # Invariant
///
/// A clean cell is lowered to a read of its frozen artifact; compiled marked
/// bindings are frozen into `registry` under `source_id`.
pub fn compile_with_imports_at_with_cells<P>(
    code: &str,
    imports: &[ResolvedImport],
    registry: Option<Arc<RwLock<Registry<P>>>>,
    base: u32,
    line_starts: &[usize],
    native_ops: NativeOps<P>,
    cells: Option<&mut CellStore>,
    compiled_cells: Vec<(Path, ExprId)>,
    source_id: &str,
) -> Report<P>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let (frontend, compiled_cells) = frontend_at_with_cells(
        code,
        base,
        line_starts,
        imports,
        cells.as_ref().map(|cells| &**cells),
        compiled_cells,
    );
    let Frontend {
        ir,
        span_index,
        diagnostics,
    } = frontend;
    // The frontend diagnostics are program-blind, so re-type them onto `P`.
    let diagnostics: Vec<Diag<P>> = diagnostics.into_iter().map(|d| d.retype()).collect();
    build_report(
        ir,
        Some(span_index),
        diagnostics,
        registry,
        native_ops,
        cells,
        compiled_cells,
        source_id,
    )
}

/// The shared tail of the pipeline: check an [`IR`] and render its diagnostics.
///
/// # Invariant
///
/// The checker only runs when the frontend resolved an IR, and diagnostics are
/// rendered only when the build fails.
pub fn build_report<P>(
    ir: Option<IR<program::LangAttr>>,
    span_index: Option<compile::SpanIndex>,
    mut diagnostics: Vec<Diag<P>>,
    registry: Option<Arc<RwLock<Registry<P>>>>,
    native_ops: NativeOps<P>,
    cells: Option<&mut CellStore>,
    compiled_cells: Vec<(Path, ExprId)>,
    source_id: &str,
) -> Report<P>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let Some(ir) = ir else {
        return Report {
            build: None,
            diagnostics,
            span_index,
        };
    };
    let registry = registry.unwrap_or_else(|| Arc::new(RwLock::new(Registry::new())));
    let build =
        Checker::<P>::build_in_attr_native(ir, registry.clone(), lang_attr_ext::<P>(), native_ops);
    // Freeze now: the build is solved, so a frozen artifact is complete.
    if let Some(cells) = cells {
        freeze_cells(&build, cells, &registry, compiled_cells, source_id);
    }
    // One type printer for the whole report, so a class keeps one `?a` name
    // across diagnostics.

    // Rendering is skipped for a clean build: a successful import may hold
    // static refs that must not be descended.
    if !build.ok {
        let mut printer =
            crate::render::TypePrinter::new_with_arrows(&build.module, Some(&build.arrows));
        // The diagnostic printer shows a struct's nominal id so two structs with
        // the same field shape stay distinguishable.
        printer.show_struct_ids();
        diagnostics.extend(
            build
                .diagnostics()
                .into_iter()
                .filter_map(|d| {
                    // PROBE (LICHEN_DIAG_TRACE=1): the diagnostic's origin.
                    if std::env::var_os("LICHEN_DIAG_TRACE").is_some() {
                        eprintln!(
                            "PROBE diag: kind={:?} error_index={:?} loc={:?} a={:?} b={:?} field={:?} msg={}",
                            d.kind,
                            d.error_index,
                            d.loc.as_ref().map(|loc| (loc.expr.0, loc.path.clone())),
                            d.a,
                            d.b,
                            d.field,
                            crate::render::checker_message(
                                &mut crate::render::TypePrinter::new_with_arrows(
                                    &build.module,
                                    Some(&build.arrows)
                                ),
                                &d
                            ),
                        );
                    }
                    // The highlevel is source-blind: a structured `Loc` maps back
                    // to a source span through the frontend's `span_index`.
                    let loc = d.loc().cloned();
                    let span = loc
                        .as_ref()
                        .and_then(|loc| {
                            span_index
                                .as_ref()
                                .and_then(|s| s.get(loc.expr.0 as usize).copied().flatten())
                        })
                        // A unify error recorded outside the checker's checks
                        // carries no `Loc`: resolve it through `node_edges`/`state`.
                        .or_else(|| {
                            // A node the GC released is absent; `node_origin`'s
                            // contract is that the caller checks liveness first.
                            let origin = |node: lichen_lowlevel::NodeId| {
                                build
                                    .module
                                    .nodes
                                    .contains_key(node)
                                    .then(|| build.module.node_origin(node))
                                    .flatten()
                            };
                            let resolve = |node: lichen_lowlevel::NodeId| {
                                build
                                    .node_edges
                                    .get(&node)
                                    .map(|loc| loc.expr.0 as usize)
                                    .or_else(|| {
                                        origin(node).and_then(|origin| {
                                            build
                                                .node_edges
                                                .get(&origin)
                                                .map(|loc| loc.expr.0 as usize)
                                        })
                                    })
                            };
                            let by_state = |node: lichen_lowlevel::NodeId| {
                                let node = origin(node).unwrap_or(node);
                                build.state.iter().position(|state| {
                                    state.term == Some(node)
                                        || state.val == Some(node)
                                        || state.ty == Some(node)
                                })
                            };
                            let span_of = |expr: usize| {
                                span_index
                                    .as_ref()
                                    .and_then(|s| s.get(expr).copied().flatten())
                            };
                            let span = resolve(d.a)
                                .or_else(|| resolve(d.b))
                                .or_else(|| by_state(d.a))
                                .or_else(|| by_state(d.b))
                                .and_then(span_of);
                            if std::env::var_os("LICHEN_DIAG_TRACE").is_some() {
                                eprintln!(
                                    "PROBE span lookup: a={:?} origin_a={:?} b={:?} origin_b={:?} -> {span:?}",
                                    d.a,
                                    origin(d.a),
                                    d.b,
                                    origin(d.b),
                                );
                            }
                            span
                        });
                    // A failure cloned out of a **static** module belongs to that
                    // module's source; no kept source, no diagnostic.
                    let (span, file) = match d.static_template {
                        Some(sref) => {
                            let source = registry
                                .read()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .get(sref.module)
                                .and_then(|package| package.meta.source.clone())?;
                            (Some(source.span_of(sref.index)?), Some(Arc::new(source)))
                        }
                        None => (span, None),
                    };
                    Some(Diag {
                        span,
                        message: crate::render::checker_message(&mut printer, &d),
                        stage: Stage::Check,
                        file,
                        related: None,
                        check: Some(Box::new(d)),
                    })
                })
                .collect::<Vec<_>>(),
        );
        // A layer above the lowlevel renders its own refusals here; no span.
        // See docs/notes/compiler-plugin.md.
        diagnostics.extend(build.module.extension_diagnostics.iter().map(|entry| {
            Diag::unattributed(
                Stage::Check,
                format!("{}: {}", entry.category, entry.message),
            )
        }));
        // A failed build carries at least one diagnostic: synthesise one here so
        // `Err` never renders nothing.
        if diagnostics.is_empty() {
            let unattributed = lichen_highlevel::diagnostic::Diag::unattributed_failure();
            diagnostics.push(Diag {
                span: None,
                message: crate::render::checker_message(&mut printer, &unattributed),
                stage: Stage::Check,
                file: None,
                related: None,
                check: Some(Box::new(unattributed)),
            });
        }
    }
    Report {
        build: Some(build),
        diagnostics,
        span_index,
    }
}

/// The frontend only: text → IR. The checker does not run.
///
/// # Invariant
///
/// `ir` is always `Some`: an unresolved name lowers to the same inert
/// `ErrorBlock` a parse error uses, and every lex, parse and resolve error
/// rides in `diagnostics`.
pub struct Frontend {
    pub ir: Option<IR<program::LangAttr>>,
    /// The `ExprId → span` index built during lowering (highlevel is span-free).
    pub span_index: compile::SpanIndex,
    pub diagnostics: Vec<Diag<LangProgram>>,
}

/// The frontend: text → IR.  See [`Frontend`].
pub fn frontend(source: &str) -> Frontend {
    let line_starts = lex::line_starts(source);
    frontend_at(source, 0, &line_starts, &[])
}

/// [`frontend`] with resolved imports seeded into the compiler's first frame.
pub fn frontend_with_imports(source: &str, imports: &[ResolvedImport]) -> Frontend {
    let line_starts = lex::line_starts(source);
    frontend_at(source, 0, &line_starts, imports)
}

/// The frontend over a slice of a larger source: `code` starts at byte `base`.
pub fn frontend_at(
    code: &str,
    base: u32,
    line_starts: &[usize],
    imports: &[ResolvedImport],
) -> Frontend {
    frontend_at_with_cells(code, base, line_starts, imports, None, Vec::new()).0
}

/// [`frontend_at`] with a cell store: clean cells lower to a static read.
///
/// # Invariant
///
/// The returned list carries the caller's already-collected paths too, so a
/// pipeline threads one list through several stages.
pub fn frontend_at_with_cells(
    code: &str,
    base: u32,
    line_starts: &[usize],
    imports: &[ResolvedImport],
    cells: Option<&CellStore>,
    mut compiled_cells: Vec<(Path, ExprId)>,
) -> (Frontend, Vec<(Path, ExprId)>) {
    let lex::Lexed {
        tokens,
        errors: lex_errors,
    } = lex::lex_with(code, line_starts, base);
    let mut diagnostics: Vec<Diag<LangProgram>> =
        lex_errors.into_iter().map(Diag::from_lex).collect();
    let parse::Parsed {
        mut program,
        errors: parse_errors,
    } = parse::parse(&tokens);
    diagnostics.extend(parse_errors.into_iter().map(Diag::from_parse));
    // The lowering is total, so the frontend always produces an IR.
    let (ir, span_index, resolve_errors, compiled) =
        compile::compile_with_imports_with_cells(&mut program, imports, cells);
    compiled_cells.extend(compiled);
    diagnostics.extend(resolve_errors);
    (
        Frontend {
            ir: Some(ir),
            span_index,
            diagnostics,
        },
        compiled_cells,
    )
}
