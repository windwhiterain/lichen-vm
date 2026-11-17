# Upgrade-propagation perf: the incremental reverse-edge class upgrade

> Status: **landed** (`13e102bd` extract + `f1a16e8f` incremental, `feature/upgrade-perf`);
> the maintainer confirmed **2.6× is sufficient** and ruled **out** caching the reverse
> edges. The upgrade-staleness mechanism this optimizes is
> [slice 2](polymorphic-flag-slice2.md)'s; the mark-scope antichain it carries is
> [slice 3](polymorphic-flag-slice3.md)'s; the contract is
> [polymorphic-flag](polymorphic-flag.md)'s.
>
> Points at: `crates/lichen-lowlevel/src/polymorphic.rs` (`polymorphic_referrers`,
> `upgrade_polymorphic_class`), `crates/lichen-lowlevel/src/equality.rs` (`add_equality`'s
> upgrading-union branch).

## 1. The cost

Slice 2's upgrade-staleness rule re-propagates on every union that joins a polymorphic and
a non-polymorphic class: `add_equality` called `propagate_polymorphic`, which recomputes
`compute_polymorphic` over the **whole module** (structural walk + re-seed every mark +
antichain fixpoint from all seeds). Measured ~5× on the language suite at slice 2. The
infinite-stream canary stayed fast only because the O(1) skip
(`polymorphic_unflagged == 0 && !dirty`) usually fires.

## 2. The optimization

An upgrading union now spreads **incrementally from the merged class**, not by a whole-module
recompute:

- `polymorphic_referrers(roots)` is the extracted recursion-set reverse-edge build
  (`referrers[v]` = every `u` recursing into `v`), shared with `compute_polymorphic`.
- `upgrade_polymorphic_class(representative, seed_scopes)` **warm-starts from the existing
  mark-scope antichains** (`Node.polymorphic_scope`), seeds the merged class's members with
  the polymorphic side's scopes, re-seeds any pending creation marks / parameters / nested
  closures (cheap when already present), and runs the fixpoint **along reverse edges only**,
  re-entering just the nodes whose antichain changed. It refreshes the bool erasure for the
  touched nodes. It is antichain-aware: it carries scopes and prunes subsumed ones, exactly
  as the full pass does.
- `add_equality` captures the polymorphic side's scopes and calls the incremental upgrade; a
  poly side whose scope is not yet established falls back to the full `propagate_polymorphic`.

The reached-mark set only grows, so warm-starting from the previous fixpoint and spreading
the delta yields the same verdict as a from-scratch recompute — the acceptance suite is the
oracle and is unmoved.

## 3. Measurement and acceptance (the tool's own lines)

- Language suite: **`9.13s → 3.53s` (~2.6×)** (`cargo test -p lichen-language --test language`).
- Infinite-stream budget canary: `ok` in `0.05s` (unchanged).
- Unmoved: `ideal1-5`, `any3/4/6/7`, all 22 [nested-apply-none](nested-apply-none.md) §27
  probes, and the monomorphic probes; lowlevel `167/0/1`, highlevel `136/0`, language lib
  `69/0`, language external `284/5` (the same five pre-existing); examples;
  `comments-check --max-violations 0` and `cargo fmt --all -- --check` clean.

## 4. The verdict: do not cache the reverse edges

The upgrade still **rebuilds** the reverse-edge map (a whole-module structural walk) and
rescans seeds per union; only the fixpoint is incremental. Caching/reusing the reverse edges
would cut more, but needs invalidation at **every** structural-mutation site (`add_node`,
value/operation writes, function setup, GC) plus incremental class-edge maintenance on union.
A missed invalidation would silently under-upgrade — re-introducing the very staleness class
the flag migration eliminated. The maintainer ruled the marginal perf **not worth that
correctness risk**; 2.6× stands. Do not pursue the cached version without a new ruling.