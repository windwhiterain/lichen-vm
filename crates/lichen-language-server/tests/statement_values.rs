//! `Doc::statement_values` / `Doc::statement_at` — the read-only per-statement
//! type/value snapshot the language server exposes.  Type is always reported;
//! a value only when the build produced a concrete one (a lazy/recursive
//! binding, whose value is a deferred `Parameterized` cell, reports `None`).

use std::path::Path;

use lichen_language_server::Doc;
use lichen_language_server::lsp::Position;

#[test]
fn statement_values_report_type_and_concrete_value() {
    // `x` and `y` are statements; the trailing `y` is the final expression and
    // is NOT a statement root.
    let doc = Doc::new("x = 3\ny = x + 4\ny\n");
    let vals = doc.statement_values();
    assert_eq!(
        vals.len(),
        2,
        "x and y are the two statements; got {vals:#?}"
    );

    // x = 3 → type Int, value "3".
    assert!(vals[0].ty.contains("Int"), "x type = {:?}", vals[0].ty);
    assert_eq!(vals[0].value.as_deref(), Some("3"));

    // y = x + 4 → type Int, value "7".
    assert!(vals[1].ty.contains("Int"), "y type = {:?}", vals[1].ty);
    assert_eq!(vals[1].value.as_deref(), Some("7"));
}

#[test]
fn statement_at_finds_the_containing_statement() {
    let doc = Doc::new("x = 3\ny = x + 4\ny\n");
    // byte offset 0 is inside `x = 3`.
    let s0 = doc.statement_at(0).expect("first statement at offset 0");
    assert_eq!(s0.value.as_deref(), Some("3"));
    // `x = 3\n` is 6 bytes; offset 6 is the start of `y = x + 4`.
    let s1 = doc.statement_at(6).expect("second statement at offset 6");
    assert_eq!(s1.value.as_deref(), Some("7"));
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
    let doc = Doc::new(source);
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
    // a `Kernel` value whose type mirrors a function.  The editor snapshot must
    // spell them `Kernel : Int -> Int`, not the raw recursive-pair layout with
    // `?` (the pre-fix `? : [[Int, Int], ?]`).  This is the LSP-visible fix.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../lichen-language/examples/programs");
    let source = std::fs::read_to_string(dir.join("compute_jit.lichen")).unwrap();
    let doc = Doc::new_with_base(source, Some(&dir));

    let vals = doc.statement_values();
    assert_eq!(vals.len(), 2, "k_double and k_outer are the two statements");
    for sv in vals {
        assert_eq!(
            sv.value.as_deref(),
            Some("Kernel"),
            "value = {:?}",
            sv.value
        );
        assert_eq!(sv.ty, "Int -> Int", "type = {:?}", sv.ty);
        assert!(
            !sv.ty.contains('?'),
            "no unresolved `?` in a resolved kernel type: {:?}",
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
    assert_eq!(hover, "`k_double` — `Kernel : Int -> Int`");
}

#[test]
fn compute_wrapper_functions_hover_with_named_type_variables() {
    // `compute.jit` / `compute.launch` are generic wrappers from a frozen
    // module.  Their type variables are unbound cells that must render as
    // *named* `?a`/`?b` (and stay shared across a kernel's signature), not as
    // an opaque bare `? -> ? -> ? -> ?` — the LSP-visible half of the same
    // "raw layout" bug for the wrapper functions themselves.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../lichen-language/examples/programs");
    let source = std::fs::read_to_string(dir.join("compute_jit.lichen")).unwrap();
    let doc = Doc::new_with_base(source, Some(&dir));

    // `jit` at line 5 (0-based): "k_double = compute.jit (y => y + y)" — char 19.
    let (hover, _range) = doc
        .hover_at(Position {
            line: 5,
            character: 19,
        })
        .expect("hover on `jit`");
    assert_eq!(hover, "`.jit` — `Function : ?a -> ?b -> ?a -> ?b`");

    // `launch` at line 7 (0-based): "compute.launch k_outer 3" — char 9.  The
    // module struct renders `.jit` first, so `launch`'s cells continue the
    // letter sequence (`?c`/`?d`/`?e`); the kernel's codomain `?d` is shared
    // with the result, while the argument is a separate generic cell `?e`.
    let (hover, _range) = doc
        .hover_at(Position {
            line: 7,
            character: 9,
        })
        .expect("hover on `launch`");
    assert_eq!(hover, "`.launch` — `Function : ?c -> ?d -> ?e -> ?d`");
}
