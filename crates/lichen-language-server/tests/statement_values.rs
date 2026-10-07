//! `Doc::statement_values` / `Doc::statement_at` — the read-only per-statement
//! type/value snapshot the language server exposes.  Type is always reported;
//! a value only when the build produced a concrete one: a lazy/recursive
//! binding, whose value is a deferred `Parameterized` cell, reports `None`, and
//! so does a statement whose expression is a **call** — a routed operator
//! (`y = x + 4`) is an apply of the `core` prelude's binding and stays lazy at
//! snapshot time (`docs/notes/operator-polymorphism.md` §7.1, cost 3).

use std::path::Path;

use lichen_language::program::LangProgram;
use lichen_language_server::Doc;
use lichen_language_server::lsp::Position;

/// The shipping-vocabulary `Doc` the server tests drive (the crate's `Doc` is
/// generic over the program collector).
type ShipsDoc = Doc<LangProgram>;

#[test]
fn statement_values_report_type_and_concrete_value() {
    // `x` and `y` are statements; the trailing `y` is the final expression and
    // is NOT a statement root.
    let doc = ShipsDoc::new("x = 3\ny = x + 4\ny\n");
    let vals = doc.statement_values();
    assert_eq!(
        vals.len(),
        2,
        "x and y are the two statements; got {vals:#?}"
    );

    // x = 3 → type Int, value "3": a literal is a concrete value at check time.
    assert!(vals[0].ty.contains("Int"), "x type = {:?}", vals[0].ty);
    assert_eq!(vals[0].value.as_deref(), Some("3"));

    // y = x + 4 → type Int, **no** value: `+` routes onto the prelude's binding,
    // which the checker applies rather than folds, and a call is lazy — so the
    // build computed no concrete value for this snapshot to report.  The value is
    // still computed at run time; only the static snapshot loses it, which is the
    // price the routing recorded (`docs/notes/operator-polymorphism.md` §7.1,
    // cost 3).
    assert!(vals[1].ty.contains("Int"), "y type = {:?}", vals[1].ty);
    assert_eq!(
        vals[1].value, None,
        "a routed operator's statement reports its type only"
    );
}

#[test]
fn statement_at_finds_the_containing_statement() {
    let doc = ShipsDoc::new("x = 3\ny = x + 4\ny\n");
    // byte offset 0 is inside `x = 3`.
    let s0 = doc.statement_at(0).expect("first statement at offset 0");
    assert_eq!(s0.value.as_deref(), Some("3"));
    // `x = 3\n` is 6 bytes; offset 6 is the start of `y = x + 4`.  The statement
    // is found and its type is reported; its value is not, for the reason above.
    let s1 = doc.statement_at(6).expect("second statement at offset 6");
    assert!(s1.ty.contains("Int"), "y type = {:?}", s1.ty);
    assert_eq!(
        s1.value, None,
        "a routed operator's statement reports its type only"
    );
}

#[test]
fn a_lazy_binding_reports_type_but_no_value() {
    // `paradox = lemma2 omega` is only referenced in an unselected `if` branch,
    // so the cascade leaves its value as a deferred `Parameterized` cell.
    // `Doc::new` and `statement_values` must complete (no divergence), and the
    // `paradox` binding reports its type with `value: None` — never forced.
    let source = "U = Type
D = (x => x) Type
sb = A => r => a => z => r (z A r) a
le = i => x => x (A => r => a => i (sb A r a))
induct = i => x => (le i x) -> (i x)
WF = z => if 1 then 5 else induct (z U le)
I = x => D -> Int
omega = i => y => y WF (x => y (sb U le x))
lemma = x => p => q => q I p (i => q (y => i (sb U le y)))
lemma2 = x => (x I lemma) (i => x (y => i (sb U le y)))
paradox = lemma2 omega
if 0 then (paradox : Int) else 5";
    let doc = ShipsDoc::new(source);
    let vals = doc.statement_values();
    // The `paradox` binding is the last statement (the `if` is the final expr).
    let paradox = vals.last().expect("a paradox statement");
    assert!(
        paradox.ty.contains("Int") || paradox.ty.contains("->"),
        "paradox type = {:?}",
        paradox.ty
    );
    assert_eq!(
        paradox.value, None,
        "a deferred binding reports no concrete value: {:?}",
        paradox.value
    );
}

#[test]
fn compute_kernel_bindings_render_by_name_not_raw_layout() {
    // The `compute_jit` example's two kernel bindings are `compute.jit` results:
    // a kernel struct whose `.I`/`.O` fields carry the signature's two sides.
    // Dropping `TypeKernel` means no renderer special-case: the value renders by
    // name via the compute vocabulary hook (`Kernel`, and the two classes the
    // body decided — `Int`, `Int`) and the type renders as the struct
    // `struct<.native <_>, .I <_>, .O <_>`, not the raw recursive-pair layout.
    // The struct's type still names no class for its fields — the field *type*
    // cells are the wrapper's own, read back through `.I`/`.O` — so each renders
    // under the raw mark, which says the printer dumped the field rather than
    // spelling it like a form the chain explained.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let source = std::fs::read_to_string(dir.join("compute_jit.lichen")).unwrap();
    let doc = ShipsDoc::new_with_base(source, Some(&dir));

    let vals = doc.statement_values();
    assert_eq!(vals.len(), 2, "k_double and k_outer are the two statements");
    for sv in vals {
        assert_eq!(
            sv.value.as_deref(),
            Some("(raw Kernel, raw Int, raw Int)"),
            "value = {:?}",
            sv.value
        );
        assert_eq!(
            sv.ty, "struct<.native raw[?a, ?b], .I raw[?c, ?d], .O raw[?e, ?f]>",
            "type = {:?}",
            sv.ty
        );
    }

    // The hover on the `k_double` binding renders the snapshot's `value : type`.
    let (hover, _range) = doc
        .hover_at(Position {
            line: 5,
            character: 0,
        })
        .expect("hover on k_double");
    assert_eq!(
        hover,
        "`k_double` — `(raw Kernel, raw Int, raw Int) : \
         struct<.native raw[?a, ?b], .I raw[?c, ?d], .O raw[?e, ?f]>`"
    );
}

/// The named type variables (`?a`, …) a rendered line mentions, in order.
///
/// The *names* are positional — which cell the checker numbered first is an
/// encoding detail, not a contract
/// ([checker-encoding-instability](../../../docs/notes/checker-encoding-instability.md))
/// — so a test whose subject is "these cells are named, and the same cell is
/// named twice" reads the names out and asserts the relation rather than the
/// letters ([tests-do-not-render](../../../docs/notes/tests-do-not-render.md)).
fn type_variables(rendered: &str) -> Vec<String> {
    let chars: Vec<char> = rendered.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if chars[at] != '?' {
            at += 1;
            continue;
        }
        at += 1;
        let start = at;
        while at < chars.len() && chars[at].is_ascii_alphanumeric() {
            at += 1;
        }
        if at > start {
            out.push(chars[start..at].iter().collect());
        }
    }
    out
}

#[test]
fn compute_wrapper_functions_hover_with_named_type_variables() {
    // `compute.jit` / `compute.launch` are generic wrappers from a frozen
    // module.  Their type variables are undecided cells that must render as
    // *named* `?a`/`?b` (and stay shared across a kernel's signature), not as
    // an opaque bare `? -> ? -> ? -> ?` — the LSP-visible half of the same
    // "raw layout" bug for the wrapper functions themselves.  A `jit` result is
    // a kernel struct, so its type renders as `struct<.native <_>, .I <_>, .O <_>`.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let source = std::fs::read_to_string(dir.join("compute_jit.lichen")).unwrap();
    let doc = ShipsDoc::new_with_base(source, Some(&dir));

    // `jit` at line 5 (0-based): "k_double = compute.jit (y : Int => y + y)" —
    // char 19.
    let (hover, _range) = doc
        .hover_at(Position {
            line: 5,
            character: 19,
        })
        .expect("hover on `jit`");
    // `jit` names twelve cells: its two arguments' own names in the wrapper's
    // two arrows, the kernel struct's `.native` raw pair, and then `.I`'s and
    // `.O`'s domain/codomain — which are those same argument cells rather than
    // fresh ones, and that is the property this test is about.  A frozen
    // module's own type lambdas claim cells before these, so the letters begin
    // wherever they begin.
    let cells = type_variables(&hover);
    assert_eq!(
        cells.len(),
        12,
        "jit names twelve cells (four per arrow side, four in `.I`/`.O`, and the \
         two signature sides repeated): {hover}"
    );
    assert_eq!(
        (&cells[8], &cells[9]),
        (&cells[1], &cells[2]),
        "`.I`'s domain and codomain are the parameter's own cells: {hover}"
    );
    assert_eq!(
        (&cells[10], &cells[11]),
        (&cells[4], &cells[5]),
        "`.O`'s domain and codomain are the return's own cells: {hover}"
    );
    assert_ne!(
        cells[0], cells[1],
        "the wrapper's two arguments are distinct cells: {hover}"
    );
    assert!(
        hover.contains(".I") && hover.contains(".O") && !hover.contains(".sig"),
        "a kernel's signature is named as the struct's `.I`/`.O` fields: {hover}"
    );

    // `launch` at line 7 (0-based): "compute.launch k_outer 3" — char 9.  It
    // reads the kernel's `.O` and returns it, so it stays a generic function of
    // named, distinct cells rather than the opaque `? -> ? -> ?`.
    let (hover, _range) = doc
        .hover_at(Position {
            line: 7,
            character: 9,
        })
        .expect("hover on `launch`");
    let cells = type_variables(&hover);
    assert_eq!(cells.len(), 5, "launch names five cells: {hover}");
    for (index, cell) in cells.iter().enumerate() {
        assert!(
            !cells[..index].contains(cell),
            "launch's cells are distinct, `{cell}` repeats: {hover}"
        );
    }
}
