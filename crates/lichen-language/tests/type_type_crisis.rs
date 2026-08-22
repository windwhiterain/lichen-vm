//! The `Type : Type` paradox, constructed.
//!
//! The checker's universe is the self-referential node `K = [Type, K]`
//! (`Checker::install_constants`, `crates/lichen-highlevel/src/checker.rs`):
//! the term `Type` has type `Type`.  The classic consequence (Girard's
//! paradox; Hurkens' 1995 simplification) is that the pure λ-calculus over
//! such a universe contains a closed term of *every* type, and well-typed
//! terms need not terminate.  Here, where the runtime *is* the typechecker,
//! that shows up as a checker-certified program the checker's own definition
//! pass cannot run to completion.
//!
//! The surface language cannot express the textbook `∀A:Type. ...` types, so
//! the paradox is transcribed in the checker's own terms: unannotated
//! lambdas (an unbound parameter type defers the apply guard and kinding),
//! the opaque `U = Type` placeholder (the term never destructs `U`), a
//! call-result arrow domain (kinding defers on an unbound cell), and a lazy
//! `if 1 then 5 else …` around the one direct apply whose argument cannot
//! resolve while its parameter is unbound.  The term is Geuvers' direct `λU`
//! transcription of Hurkens' paradox (*Inconsistency of classical logic in
//! type theory*, §2).

use std::panic::{AssertUnwindSafe, catch_unwind};

use lichen_language::compile;

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

#[test]
fn the_paradox_is_certified_as_int_and_diverges_when_run() {
    // The checker certifies `paradox : Int` with no diagnostics: the
    // annotated subterm checks cleanly when it sits in an unselected `if`
    // branch (the annotation unify binds the call's result cell to `Int`,
    // and nothing forces the call).
    let certified = format!("{PARADOX}\nif 0 then (paradox : Int) else 5");
    let report = compile(&certified);
    assert!(
        report.ok(),
        "the checker certifies `paradox : Int`: {:?}",
        report.diagnostics
    );

    // Running the same term as the root diverges: the definition pass
    // evaluates `lemma2 omega`, and the paradox's self-referential structure
    // recurses until a VM guard panics.  So the typechecker — which runs the
    // program to check it — panics on a program it certified: the
    // "well-typed terms need not terminate" face of `Type : Type`.
    let run = format!("{PARADOX}\nparadox : Int");
    let outcome = match catch_unwind(AssertUnwindSafe(|| compile(&run))) {
        Ok(_) => panic!("the certified paradox must not run to completion"),
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| format!("{payload:?}"));
            format!("PANIC: {msg}")
        }
    };
    println!("{outcome}");
    assert!(
        outcome.contains("non-terminating"),
        "the certified pure-λ term diverges at a VM guard (depth or total-application): {outcome}"
    );
}
