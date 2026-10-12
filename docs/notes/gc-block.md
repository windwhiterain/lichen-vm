# `@gc e` — an expression in a scope of its own, released when it decides

> Status: **current.** `@gc` marks an expression; the checker compiles it into a
> **child** lowlevel block and declares the node that ends the scope
> (`Module::mark_scope_root`). Evaluating that node deep-runs the scope and then
> `Module::garbage_collect`s it: the reachable subtree moves into the parent
> block and the rest of the scope is released. The `@`-led keyword list is
> `docs/language-spec.md` §2; the mechanism is
> [`lowlevel-vm.md`](lowlevel-vm.md) §"Block GC".
>
> Points at: `crates/lichen-language-lex` (`KwGc`),
> `crates/lichen-language-parser` (`gc_marked`, `Expr::Gc`),
> `crates/lichen-highlevel/src/checker/gc.rs` (`check_gc`),
> `crates/lichen-lowlevel/src/gc.rs` (`garbage_collect`, `class_walk`,
> `prune_released_failures`), `crates/lichen-lowlevel/src/evaluation.rs`
> (`evaluate_block`), `crates/lichen-lowlevel/src/function.rs` (`clone_target`).

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

`evaluate_block` then refuses to collect an **undecided** root. Some reachable
slot is still empty, so nothing in the scope is proven unreachable and a release
would take a node a later read names. The mark is put back in that case, so a
root that decides later still collects; it is cleared on the way in, because the
deep pass below reads the root again and the mark is what would send it straight
back.

## What the walk keeps

The collect walk follows four edges out of a node: **the equality classes it
aliases into**, its value's items, the nodes its value traces, and **the operand
of an operation it still owes an answer for**. The class edge is the one the
language depends on and the one that was missing: a reader's answer lives in the
class it read into rather than in the reader's own slot (`Module::alias_read`
joins them), so walking values alone would release the very node the reader
answers from. A class is entered once per collect, so the walk visits each member
once and stays linear in the scope's size.

The operand edge is narrower than "always follow": an operator that has already
run keeps its answer, and following a reader's operand would keep the whole
container it reads out of — which is exactly the garbage a scope wants. What
still has to follow is the operand of a node whose run stamp is zero, because a
value written by a unification is not that operator's answer and the operator
still reads the operand for its own.

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
  block, because a function outlives the expression that built it, so `@gc`
  inside a body collects per *instantiation* (below) and never the template.
- **An undecided scope is not collected at all**, by the rule above.