# `@gc e` — an expression in a scope of its own, released when it decides

> Status: **current — the scope compiles and releases automatically.** `@gc` marks an
> expression; the checker compiles it into a **child** lowlevel block and declares the
> node that ends the scope (`Module::mark_scope_root`). The scope's value is `inner`'s and
> nothing is computed differently. The release happens at the **read** of that root: the
> read is shallow, the root's own slot says whether the scope is done, and the collect then
> keeps what the value reaches. The collector itself is `Module::garbage_collect`,
> `Module::drop_block` and `prune_released_failures`. The trigger's design, the readings
> behind it, the two collector defects it had to repair and the one that is still open are
> [gc-trigger-design](gc-trigger-design.md); the retired read-path collection is
> [gc-trigger-disabled](gc-trigger-disabled.md).
>
> Points at: `crates/lichen-language-lex` (`KwGc`),
> `crates/lichen-language-parser` (`gc_marked`, `Expr::Gc`),
> `crates/lichen-highlevel/src/checker/gc.rs` (`check_gc`),
> `crates/lichen-lowlevel/src/gc.rs` (`garbage_collect`, `class_walk`,
> `relocate_class_value`, `owned_by_surviving_function`, `prune_released_failures`),
> `crates/lichen-lowlevel/tests/basic/gc_safety.rs` (the safety contracts),
> `crates/lichen-lowlevel/src/evaluation.rs` (`evaluate_block`, `is_scope_root_read`),
> `crates/lichen-lowlevel/src/function.rs` (`clone_target`).

## What it is

`@gc` is a **prefix marker over any expression**, at the `@assert` level, so it
takes the whole application rather than one atom of it. A block is one form of
an expression, which is why `@gc { … }` needs no grammar of its own:

```lichen
@gc { a = build(); b = build(); a }   -- the block, in a scope
@gc f 41                               -- the call, in a scope
x = @gc [1, 2, 3]                      -- a value, in a scope
```

The marker allocates nothing and computes nothing. It compiles to
`ExprKind::Gc { inner }`, which allocates no cell of its own: `inner`'s node is
the scope's root and every slot is the inner expression's. The only thing the
marker adds is the block boundary.

What it buys is **a lifetime**. Every node the marked expression allocates —
its cells, its array and table payloads, its apply clones — is homed in one
block with its own bump arena, and the whole of it goes when the expression has
decided its value. The place it pays is a scope that runs more than once:

```lichen
f = n => @gc { a = n + 1; b = a * 2; b }   -- each call gets its own scope
```

and a call site that allocates a lot the caller does not want to keep:

```lichen
@gc f 41          -- the apply's clones die with the call, not with the module
```

## How the collect is triggered

A scope ends at **its declared root**, whichever block reads it. That is what
`Module::mark_scope_root` records, and it is not the same question as the
lowlevel's older rule — "a child-block root is whichever node the parent block
reads" — which is kept for the blocks no layer declared. The reason it is not
enough is measured: `x = @gc { a = 2; b = 3; a + b }` never ends a scope under
that rule, because a language block compiles to `Field(Tuple(stmts…, tail), n)`,
the tail's value is a scalar, and `Index`'s element read passes the *element's
own block* as the referer, so no parent-to-scope read ever happens. A scope that
a layer declared has one node that ends it, and the checker says which.

`evaluate_block` reads that root **shallowly** — `evaluate_node`, never
`evaluate_node_deep` — and collects only when the root's own slot holds a real
value (not the wildcard). The read is the signal, so nothing is forced to find
out whether the scope is done, and an unread expression never runs because of the
marker. An undecided root puts the mark back, so a later read retries; the value
the reader receives is the collect's own relocation, so it never holds a payload
from the released arena. The deep pass must not answer for such a read out of its
verdict cache, which is why `evaluate_node_deep_inner` asks
`is_scope_root_read` first.

The walk keeps a **pending assert's condition** as a second root. The assert pass
(`check_asserts`) drains after the pass that collects, so a condition released
here would silently unregister its assert: measured before the fix, a failing
`@assert` inside a scope made its build pass. See
[gc-trigger-design](gc-trigger-design.md).

## What the walk keeps

The collect walk follows four edges out of a node: **the equality classes it
aliases into**, its value's items, the nodes its value traces, and **the operand
of an operation it still owes an answer for**. The class edge is the one the
language depends on and the one that was missing: a reader's answer lives in the
class it read into rather than in the reader's own slot (`Module::alias_read`
joins them), so walking values alone would release the very node the reader
answers from. A class is entered once per collect, so the walk visits each member
once and stays linear in the scope's size.

**A node outside the vacated subtree is walked too.** A live node may name a node
inside it, or owe an answer whose operand is inside, and neither is reachable from
the root — measured: a parent block's array named the scope's own array node,
which the collect then released, and the next deep frame died on it. Such a node
keeps its block; only a class member's payload moves into that member's own block,
because a class write hands every member the same payload and that payload may
live in the arena this collect releases (`relocate_class_value`).

**The class walk starts at the representative.** The member list is headed by it,
so entering at a member behind the head reached only the members behind *that* and
released the ones ahead — including the representative itself, which silently split
the class. Measured: a three-member class whose head was homed in the collected
block, entered at a surviving member, lost its head and became two.

**This keeps more of the graph alive than it used to.** A member a walked node
aliases is now re-homed into the target block instead of being released with the
vacated one. Before, such members died and the class lost its identity with them.

The operand edge is narrower than "always follow": an operator that has already
run keeps its answer, and following a reader's operand would keep the whole
container it reads out of — which is exactly the garbage a scope wants. What
still has to follow is the operand of a node whose run stamp is zero, because a
value written by a unification is not that operator's answer and the operator
still reads the operand for its own.

**A surviving function's template is a root set of its own.** A function's home block
and its template's nodes can diverge — a per-call closure is homed at the call site while
its nodes land in the call's fresh scope block — so the walk cannot always reach them from
the root. The node's owner is the one record that says which function it belongs to, and
the collect keeps what a function whose home outlives the block names, whether the walk
reaches it or not (`owned_by_surviving_function`). Measured on the example corpus: one
surviving function named 23 nodes of the released block; after the fix the two collects
leave **no** dangling function node.

**The safety contracts are pinned in `gc_safety.rs`**: a surviving function's template and
its assert conditions move with the collect, a function homed in the released block dies
with its template, a class member's payload is re-allocated in its own arena, a value cycle
terminates, and a table entry's nodes move.

**The block boundary holds at the language layer.** The collector may release only what no
surviving structure names, and the contract is: a referrer may name its own block, an
ancestor (the ancestor outlives it), a parentless block (a function body or the module root,
never released), or a child scope's **declared root**. The front end holds it by
construction: `check_gc` gives the parent `Index(pair, 0)` and `Index(pair, 1)` reads of the
scope's pair, so the parent names the declared root only, and reading that handle is what
collects the scope. The `block_boundary` test scans every payload and operand of a language
corpus and measures **0** violations. The engine does not validate the contract, by decision,
so a hand-built module can still break it — the `#[ignore]`d repro in `gc_safety.rs` records
that host contract (`D21` in [code-audit](code-audit.md)).

## What it actually frees, measured

A block is **not** a tuple of statements — it is a tuple of the statements'
*compiled expressions*, read at the tail's position (`Compiler::wrap`), so that
every statement is checked and run. Those expressions are reachable only through
the reader's operand, which the walk does not follow, so one would expect a block
scope to release them all. It does not: a block-wide binding's placeholder is
unified with the value the tail reads, so the **class edge** puts every statement
back in reach.

Two builds, each against its own unmarked twin:

| source | nodes | blocks |
|---|---|---|
| `let a = 2` `let b = 3` `a + b` | 24 | 1 |
| `x = @gc { a = 2; b = 3; a + b }` `x` | 24 | 1 |
| `f = n => n + 1` `x = f 41` `x` | 52 | 2 |
| `f = n => n + 1` `x = @gc f 41` `x` | 43 | 2 |

So `@gc` over a **block** frees the block record and its arena and **no nodes**,
and `@gc` over a **call** frees the nine nodes the answer's class does not reach
— the operand array, the argument pair, and the cells nothing binds. That is the
honest shape of the feature today: the per-call case is the one that pays, which
is the case the call-site idiom exists for.

## What a release takes with it

A released node is gone: `Module::nodes` is a `SlotMap`, so indexing a released
key panics. Three tables could still name one, and each is pruned at the
release:

- the lowlevel's own failure registries (`unify_errors`, `eval_errors`,
  `assert_errors`, `apply_errors`), on the same rule the assert worklist already
  used — a failure inside a released scope goes with it;
- the checker's `state`, `arrows`, `node_edges`, `apply_edges`, `user_asserts` and
  `assert_spellings` (`Checker::prune_released`), so an expression inside a
  collected scope reads as having no compiled slot rather than naming a released
  node.

This is why the checker collects nothing itself: the collect fires inside the VM,
and the tables are pruned in the build that follows it. A host that reads a
`Build` after running it sees the same pruned tables.

## What it does not do

- **A `@gc` in an imported file has no scope in the importer.** The frozen
  artifact is a decided graph with no block structure, and the scope was
  released when the module that wrote it was checked; the importer materializes
  what survived.
- **A lambda's body block is not a scope.** `check_lam` opens a parentless
  block, because a function outlives the expression that built it, so a scope
  inside a body is collected per *instantiation* (below) while the template's own
  scope is still undecided at compile time.
- **A template scope that decides at compile time is released then**, and its
  calls then have no scope of their own — the maintainer's decision: the per-call
  release is not promised for a body whose scope a definition-pass read already
  finished.
- **An undecided scope is not collected at all**, by the rule above.
- **The engine does not validate the block boundary.** The language front end holds it by
  construction (`check_gc`'s parent reads, pinned by `block_boundary.rs`), but a hand-built
  module may still name a non-root scope node from its parent; the collector then keeps it
  only when another edge reaches it. That is the `#[ignore]`d host-contract repro in
  `gc_safety.rs` (`D21`).