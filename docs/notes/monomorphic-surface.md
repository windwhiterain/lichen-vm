# The monomorphic surface: an unprimed lambda binder (ruling 3)

> Status: **the surface landed; reading (b) confirmed by the maintainer** (`227bbed0`,
> `feature/monomorphic-surface`). Ruling 3 and its (a)-deferral are [the contract note](polymorphic-flag.md)'s;
> the empty unprimed census is [the recon](polymorphic-flag-recon.md) §2.4's; the
> scope-relative selection this builds on is [slice 3](polymorphic-flag-slice3.md)'s.
>
> Points at: `crates/lichen-language-parser/src/ast.rs` (`Expr::Formal.primed`),
> `crates/lichen-language/src/resolve.rs` (the free-name rule in `Expr::Name`),
> `crates/lichen-language/src/compile.rs` (`ir.monomorphic_formals`),
> `crates/lichen-highlevel/src/checker/lambda.rs` (`check_lam`'s conditional mark).

## 1. The surface

A **primed** binder `'x` is a polymorphic formal; an **unprimed** binder `x` is a
monomorphic formal (SPEC §Polymorphic, ruling 3). The prime-adjacency lexer rule is
unchanged. The distinction is a `primed: bool` on `Expr::Formal`.

**The free-name rule.** The parser's old invariant was "a formal reference (primed) is the
only way a lambda declares a parameter; the parameter side's other names are outer-scope
expressions." The resolver now relaxes it: in a lambda parameter side, an unprimed name
that is **free** (not bound in any enclosing scope) declares a **monomorphic** formal; an
unprimed name that **resolves** to an enclosing binding stays a reference. So ideal4's
inner `{.'dividend: T, .'dividor: T}` keeps `T` as a reference to the outer `'T`. A free
unprimed name was an `unresolved name` error before; it is now a monomorphic binder.

## 2. The mechanism

- The resolver converts the free `Name` to `Formal { primed: false }`, reusing one binder
  for repeated occurrences (as the primed path does).
- The frontend records the **exception**: `ir.monomorphic_formals` holds the unprimed
  formal exprs. It is an exception set, not a primed set, so the **IR/TestKit path** (which
  builds IR directly, with no frontend) defaults to polymorphic — a primed set would have
  left every IR formal unmarked and broken the lowlevel/highlevel suites.
- `check_lam` marks a formal's value/type cells polymorphic **unless** it is in
  `monomorphic_formals`; the identity-return `wildcard_fill` is gated on the lambda having
  a primed binder. A monomorphic formal's cells are unmarked, so the scope-relative
  selection references them in place.

## 3. The census is empty

As [recon](polymorphic-flag-recon.md) §2.4 predicted, none of the 51 `.lichen` files in
`examples/`, `lichen-std/`, `lichen-std-native/` has an unprimed-binder lambda — the form
was inexpressible (`f := x -> x` was `unresolved name x`). The core prelude is entirely
primed (`core.lichen`), so it does not move. **No existing program's answer changes.**

## 4. Acceptance (the tool's own lines, all unmoved)

- `ideal1` `6: Int`, `ideal2` `2: Int`, `ideal3` `2: Int`, `ideal4` `(2, 0.5)`, `ideal5`
  `Int: Type`; `any3` `(2, 2)`, `any4` `(2, 0.5)`, `any6` `((1, 2), (1.5, 2.5))`, `any7`
  `(43, 44)`; all 22 [nested-apply-none](nested-apply-none.md) §27 probes (`v3` `4: Int`).
- `cargo test -p lichen-lowlevel`: `167 passed; 0 failed; 1 ignored`.
- `cargo test -p lichen-highlevel --test highlevel`: `136 passed; 0 failed`.
- `cargo test -p lichen-language --lib`: `69 passed; 0 failed`.
- `cargo test -p lichen-language --test language`: `284 passed; 5 failed` — the same five
  pre-existing names, zero new.
- Infinite-stream budget canary `ok` in `0.05s`; examples unchanged; `comments-check
  --max-violations 0` and `cargo fmt --all -- --check` clean.

## 5. Monomorphic behaviour, and the confirmed identity edge

A monomorphic body is referenced in place: `f := x -> x + 1` called Int-then-Float refuses
`expected Int, found Float` (the body's type is pinned); single calls and same-type calls
work. **Identity edge — reading (b) confirmed:** a monomorphic *identity* `f := x -> x` called `(f 1, f 2.0)` answers
`(1, 2.0)` — the parameter cell still freshens under `node_apply`'s `in_parameter` force,
and the identity's body is just that parameter. The two readings weighed were:
(a) "only `'`-marked binders opt into per-call instantiation" means the monomorphic
*parameter* is shared too, so the identity must refuse; (b) monomorphic = the body's
*computation* is instantiated once while the argument varies, so an identity (no
computation) passes each argument through. A naive gate of `in_parameter` on the
polymorphic flag was tried for (a) and **broke primed** `w7` (`f 3; f 4` → `expected 3,
found 4`), so it was reverted; (a) needs a parameter-walk change that does not suppress
polymorphic freshening. The maintainer confirmed (b) as the within-SPEC fix ("修复or SPEC
proposal"): the monomorphic body computation is referenced in place, the parameter
(argument fill) freshens per call, identity passes through. Reading (b) stands.