//! The `Type : Type` "unprecedented crisis": probes that construct
//! Girard's / Hurkens' paradox against this language, whose universe is the
//! self-referential node `K = [Type, K]` (the checker's `install_constants`).
//!
//! The probes are observational: they compile a program, catch any panic
//! (the checker's definition pass runs inside `Checker::build`), and report
//! what happened — accepted, rejected with diagnostics, or panicked.  The
//! interesting outcomes are:
//!
//! - a **checker-certified** program whose runtime value does not match its
//!   claimed type (the type/value mismatch), and
//! - a checker-certified pure-λ program (no `rec`, no constructors) that
//!   does not terminate, or that breaks the VM's shape invariants.
//!
//! The surface language has no `rec`-free way to write the dependent
//! `∀A:Type. ...` types of the textbook paradox, so the transcription is
//! adapted to the checker's rules: unannotated lambdas (unbound parameter
//! types defer the apply guard and kinding), the opaque `U = Type`
//! placeholder (the λU term never destructs `U`), call-result arrow domains
//! (kinding defers on unbound cells), and a lazy-branch guard around the one
//! direct apply whose argument cannot resolve while its parameter is unbound.

use std::panic::{AssertUnwindSafe, catch_unwind};

use lichen_language::compile;

/// Compile a program, catching the checker's panics; report what happened.
fn probe(source: &str) -> String {
    match catch_unwind(AssertUnwindSafe(|| compile(source))) {
        Ok(report) => {
            if report.diagnostics.is_empty() && report.build.as_ref().is_some_and(|b| b.ok) {
                let build = report.build.unwrap();
                let mut module = build.module;
                // The definition pass already ran inside `build`; evaluate the
                // root value and type so a remaining divergence or mismatch
                // shows up here (and any panic is caught above).
                let value = module.evaluate_node_deep(build.root_val, None);
                let ty = module.evaluate_node_deep(build.root_ty, None);
                format!("ACCEPTED  root value={value:?}  claimed type={ty:?}")
            } else {
                let messages: Vec<String> = report
                    .diagnostics
                    .iter()
                    .map(|d| d.message.clone())
                    .collect();
                format!("REJECTED  {} diagnostic(s): {messages:?}", messages.len())
            }
        }
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| format!("{payload:?}"));
            format!("PANIC: {msg}")
        }
    }
}

#[test]
fn probe_self_application() {
    // The equi-recursive universe admits the classic non-well-founded term:
    // `x => x x` needs no occurs check (deferred by design), the apply guard
    // defers on the unbound parameter type, so the checker certifies it —
    // and the definition pass runs the self-application forever.
    println!("{}", probe("Omega = x => x x; (Omega Omega : Int)"));
}

#[test]
fn probe_paradox() {
    // Geuvers' direct λU (Type : Type) transcription of Hurkens' paradox,
    // adapted to the checker's rules (see the module doc).  `paradox` is a
    // closed pure-λ term claimed to have type `Int`.
    let source = r#"
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
paradox : Int
"#;
    println!("{}", probe(source));
}

#[test]
fn probe_paradox_unannotated_root() {
    // Same term, but the root is not annotated: the checker's claimed type is
    // whatever the runtime syncs (or an underdetermined cell).
    let source = r#"
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
lemma2 omega
"#;
    println!("{}", probe(source));
}

#[test]
fn probe_mismatch_candidates() {
    // The naive mismatch routes: the runtime result-cell sync
    // (`function.rs`'s apply arm) re-checks every evaluated call's type, so
    // these are expected to be rejected — verify.
    println!("{}", probe("f = x => x; (f (y => y) : Int)"));
    println!("{}", probe("f = x => x; [f (y => y), 5]"));
    // A call-result of underdetermined type applied as a function: the apply
    // guard defers on the unbound cell, and the runtime hits its shape
    // invariant instead of a clean error.
    println!("{}", probe("f = x => x; (f 5) (f 3)"));
    // The hidden mismatch: an `Int<2>` array whose second element is a lambda
    // behind a lazy branch (the element-type cell binds to `Int` at check
    // time; the apply never runs because the branch is never selected).
    println!(
        "{}",
        probe("f = x => x; g = if 0 then ([5, f (y => y)][0]) else 5; g")
    );
}
