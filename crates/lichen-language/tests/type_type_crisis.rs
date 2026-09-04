//! The `Type : Type` paradox, constructed.
//!
//! The checker's universe is the self-referential node `K = [Type, K]`
//! (`Checker::install_constants`, `crates/lichen-highlevel/src/checker.rs`):
//! the term `Type` has type `Type`.  The classic consequence (Girard's
//! paradox; Hurkens' 1995 simplification) is that the pure λ-calculus over
//! such a universe contains a closed term of *every* type, and well-typed
//! terms need not terminate.  Here, where the runtime *is* the typechecker,
//! that shows up as a top-level binding whose value computation cannot run to
//! completion.
//!
//! Option B: the checker evaluates every user-written top-level statement, so
//! a binding whose value never terminates (like `paradox = lemma2 omega`) is
//! **reported as a `NonTerminating` error** — the writer is told their
//! paradox is an error, and the checker survives instead of crashing.  The
//! build catches the VM's apply/depth guard that used to panic.
//!
//! The surface language cannot express the textbook `∀A:Type. ...` types, so
//! the paradox is transcribed in the checker's own terms: unannotated
//! lambdas (an unbound parameter type defers the apply guard),
//! the opaque `U = Type` placeholder (the term never destructs `U`), a
//! call-result arrow domain (an unbound cell defers), and a lazy
//! `if 1 then 5 else …` around the one direct apply whose argument cannot
//! resolve while its parameter is unbound.  The term is Geuvers' direct `λU`
//! transcription of Hurkens' paradox (*Inconsistency of classical logic in
//! type theory*, §2).

use lichen_highlevel::diagnostic::DiagKind;
use lichen_language::compile;
use lichen_language::diag::Stage;

/// The adapted λU paradox.  `paradox` is a closed pure-λ term — no `rec`,
/// no constructors — claimed to have type `Int`.
const PARADOX: &str = r#"
U = Type
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
"#;

/// The `paradox` binding is reported as a `NonTerminating` diagnostic, and the
/// checker survives (no panic) whether or not the binding is also the value.
#[test]
fn the_nonterminating_paradox_is_reported_as_an_error() {
    let certified = format!("{PARADOX}\nif 0 then (paradox : Int) else 5");
    let report = compile(&certified);
    assert!(
        !report.ok(),
        "a non-terminating binding must not certify: {:?}",
        report.diagnostics
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.stage == Stage::Check
                && d.check.as_ref().is_some_and(|c| c.kind == DiagKind::NonTerminating)),
        "expected a NonTerminating diagnostic, got {:?}",
        report.diagnostics
    );

    // The same binding as the value is also reported (and does not panic).
    let run = format!("{PARADOX}\nparadox : Int");
    let report = compile(&run);
    assert!(!report.ok(), "a non-terminating binding must not certify");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.stage == Stage::Check
                && d.check.as_ref().is_some_and(|c| c.kind == DiagKind::NonTerminating))
            .count(),
        1,
        "exactly one NonTerminating diagnostic, got {:?}",
        report.diagnostics
    );
}

