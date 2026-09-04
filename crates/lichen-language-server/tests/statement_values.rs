//! `Doc::statement_values` / `Doc::statement_at` — the read-only per-statement
//! type/value snapshot the language server exposes.  Type is always reported;
//! a value only when the build produced a concrete one (a lazy/recursive
//! binding, whose value is a deferred `Parameterized` cell, reports `None`).

use lichen_language_server::Doc;

#[test]
fn statement_values_report_type_and_concrete_value() {
    // `x` and `y` are statements; the trailing `y` is the final expression and
    // is NOT a statement root.
    let doc = Doc::new("x = 3\ny = x + 4\ny\n");
    let vals = doc.statement_values();
    assert_eq!(vals.len(), 2, "x and y are the two statements; got {vals:#?}");

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
        paradox.value,
        None,
        "a deferred binding reports no concrete value: {:?}",
        paradox.value
    );
}
