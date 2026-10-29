# A nested apply loses the parameter it consumed

> Status: **fixed.** §27 records the repair as landed: every one of the twenty-two failing programs answers its
> correct value, and none of the other four hundred and seven answers moved. What follows is the derivation,
> kept because the mechanisms it refuted are what a later reader needs.
> **In one paragraph.** `mk = x => y => x; f = n => (mk n) 1; f 3` answers `none` because the
> checker evaluates **every function's return node once, by design** ("so its checks fire even
> unapplied"), and for this shape that node *is* the body's apply — so the pass **runs the call with
> the parameter unbound**, mints the closure generation whose cells stay empty, and its own invariant
> **pins that generation into the run**; the run's binding goes to a clone of its own generation while
> the answer's read lands on the pinned, empty one. Both readings of the failure are exact over the whole
> corpus and select the same twenty-two programs: a program answers `none` **iff** its root evaluation
> performs a walk-less read of a node an earlier walk minted (§25, reproduced), and **iff** some undecided
> read has a same-origin sibling that holds a value — the pinned clone was read while the value landed on
> the other clone of that template cell (§26). Thirteen candidate mechanisms were refuted on the way and
> are listed with the instrument that killed each (§13); five predictions made on programs the cause was
> not derived from have held (§26). Read §26 for the cause, §25 for the first exact predicate, §13 for
> what is ruled out, and §28 for how to reproduce any line.
>
> Status: **cause found**. This note is the coordination document for the second
> `none`: `mk = x => y => x; f = n => mk n 1; f 3` answers `none` where `dev`
> refuses it. It is **not** the cause of [function-unify](function-unify.md): that one needed a clone to
> meet its template in one class, and this program has no such merge — every merge of two decided
> closures in it is clone against clone — and the repair that removed the capture loss leaves it
> unchanged. The two share only their visible face, a `none` where a captured value belongs.
>
> Points at: `crates/lichen-highlevel/src/checker.rs` (the per-function return pass at line 557),
> `crates/lichen-lowlevel/src/evaluation.rs` (the deep walk's descent into a value's items),
> `crates/lichen-lowlevel/src/apply.rs` (`wire_apply_result`).

## 1. The shape that fails, narrowed by measurement

`mk = x => y => x` unless stated otherwise. Every line below is the program's answer on
this branch, `dev` refusing all of them with `expected ?a => Int, found ?b => Int`.

| program | answer |
| --- | --- |
| `mk 3 1` | `3` |
| `(mk 3) 1` — the returned closure applied on the spot | `3` |
| `g = n => (mk 3) 1; g 0` — the same inside a lambda body | `3` |
| `f = n => { c = mk n; c 1 }; f 3` — the returned closure bound to a local first | `3` |
| `f = n => { w = n; (mk w) 1 }; f 3` — the argument read into a local first | `3` |
| `f = n => { z = 0; (mk n) 1 }; f 3` — any other statement in the body | `3` |
| `f = n => { (mk n) 1 }; f 3` — the body's value is that apply and nothing else | **`none`** |
| `f = n => (mk n) 1; f 3` — the same with a bare expression body | **`none`** |
| `g = x => y => 2; f = n => (g n) 1; f 3` — the returned closure reads no capture | `2` |

So the failing shape is: **the lambda's value is an apply whose callee is another apply
that took the lambda's parameter, with no other statement in the body.** One other
statement anywhere, or one local hop for either the argument or the result, and the
answer is right; a returned closure that reads no captured cell is right even inside the
failing shape.

Two further measurements narrow it to the body's *shape* rather than to what the extra
statement does:

| program | answer |
| --- | --- |
| `f = n => { 0; (mk n) 1 }; f 3` — a bare, value-less statement first | `3` |
| `f = n => { n; (mk n) 1 }; f 3` — a bare read of the parameter first | `3` |
| `f = n => { (mk n) 1 }; f 3` — a block whose only statement is that apply | **`none`** |

So the trigger is that the lambda's **value node is the outer apply node itself**: add any
second statement and the body's value becomes its own cell, and the answer is right. That
is the same "the term is its own type" territory as
[function-unify](function-unify.md), but reached without any class merge.

**The callee's form matters too, and this is the narrowest handle so far:** the same
program with the closure written inline answers correctly.

| program | answer |
| --- | --- |
| `f = n => ((x => y => x) n) 1; f 3` — the callee is an inline lambda | `3` |
| `f = n => ((x => y => x) 3) 1; f 0` — inline, and a literal argument | `3` |
| `mk = x => y => x; f = n => (mk n) 1; f 3` — the callee is a **binding** | **`none`** |
| `mk = x => y => x; f = n => (mk 3) 1; f 0` — binding, literal argument | `3` |

So the loss needs a **binding** as the callee *and* the parameter as its argument *and* the
body's value to be that apply. A binding is what the apply's callee reference resolves
through, which points the next measurement at the reference's own class rather than at the
closure's.

## 2. What is excluded so far

- Not the function-arm merge of [function-unify](function-unify.md): no merge in these
  programs has a template on one side, and the repair that removed the capture loss
  leaves every program above unchanged.
- Not the type fallback of `apply_callee_type`: traced runs of the failing shape show no
  `f : f` fallback (no undecided callee read) at all, so the apply never takes that path.
- Not a bare expression body by itself: `f = n => { (mk n) 1 }` fails with a block body
  too, and `n1c`'s bare body with a literal argument answers `3`.
- Not the callee's kind: the outer apply of the failing shape never reaches the arm that
  matches on a callee value, which places the loss at the operand array read.

## 3. Two negatives from the first days

Traced on the language runner, with the minting site and the apply's callee printed side
by side, the failing and the repaired forms are **the same in every id**: both mint the
returned closure's instance (`source=F2 fresh=F4` and a second `fresh=F5`), both apply the
returned closure's **template** at the outer apply, and the minted instance (`F5`) is never
applied in either program. So "which generation is applied" does not separate them, and
neither does the applied closure's captured scope: printed cell by cell, with each cell's
own slot, its class's answer, and its expr's head, **neither program's applied template
holds the captured `3`**, while one answers `3` and the other `none`.

The failing shape's outer apply also returns undecided **before** it reads a callee: the
`Apply` arm's first read is its operand array, and that array reads as decided with the
same four item states in both forms, so the difference is downstream of it. What the
sections below narrow it to is recorded in §6 onwards; §13 lists every reading that has been
refuted since, each with the instrument that refuted it.

## 4. The candidate cause, and where it stands

`check_lam` wires a function's return to the **term** of its body:

```rust
let ret = self.check_expr(r#return);            // `check_term` → `state[body].term`
self.module.functions[function].r#return = ret;
```

and an apply expression's `term` is its **operation node** (`check_app` sets
`state[e].term = Some(node)`, `state[e].val = None`, `state[e].ty = Some(c)`), while every
other body kind's term is a `[value, type]` expr. Measured, that wiring shows on the
instantiation path: `RETURN … operation=true` appears for the apply-bodied function in the
failing form, and the failing run **stops** at that instantiation while the working inline
form continues into further ones.

**But the wiring is necessary and not sufficient**, and the note says so rather than
promoting it: the inline-callee program (`f = n => ((x => y => x) n) 1`, which answers `3`)
has a bare apply body too, so it carries the same operation-wired return. What separates the
two must therefore be what the **binding** contributes — the reference the callee resolves
through — and not the return's shape.

Refuted along the way, each by measurement: the *return's* shape (an expr with two items in
both the failing and the inline form), the *applied ids* (both forms mint the same
instances and apply the same functions), and the *clone decisions on operation nodes* (both
forms clone them, in similar numbers). Also measured: `wire_apply_result` writes the result
into the apply's node and unifies the caller's cell only in its expr-valued arm, so a
**function-valued** result is returned without being written — which is why a direct chain
that reads the node and a local hop that carries the value behave differently.

Next: separate what the binding contributes, with two programs that keep the failing shape
but move the binding — a local `g = mk` first, then another top-level `h = mk` — and read,
for each, whether the loss follows the binding's *kind* or the *specific* `mk`.

## 5. The callee's reference: the loss follows a **top-level** binding

The narrowest handle yet, measured by moving only the callee:

| callee | answer |
| --- | --- |
| the top-level binding `mk` | `none` |
| a second top-level binding, `h = mk`, called as `h n` | `none` |
| the top-level binding with an unrelated `u = 0` added before it | `none` |
| a **local** binding, `{ g = mk; (g n) 1 }` | `3` |
| an **inline** lambda | `3` |

So the loss follows a reference whose read resolves **outside the calling function** — a
top-level binding — and neither a local binding nor an inline lambda shows it. An extra
top-level statement does not repair it, so it is not the number of statements at the top
level.

Where that difference is *not*: the clone walk. Printed for the same reference node in the
local form (which answers `3`) and the top-level form (which answers `none`), the decision
is identical in every dimension measured — `proven=true scoped=false in_place=false`, the
class's answer the same function, the same member count. The difference is therefore in the
**evaluation**, not in how the walk images the reference.

## 6. The read that ends undecided, scoped to the root's own evaluation

Tracing undecided reads *inside the program root's evaluation* (rather than by log order,
which had attributed one to a prelude node) separates the two forms cleanly: the failing
form has exactly two, the working form none.

```
READ-NONE n69  function=F1   operation=false block=4 scoped=false origin=Some(n21)
            members=[n43=None n36=None n69=None n105=None]
READ-NONE n105 function=None operation=true  block=1 scoped=false origin=None
            members=[n43=None n36=None n69=None n105=None]
```

Both reads are of members of **one class whose four members all hold nothing**, and the
class mixes roles: `n105` is the **top-level operation** (the program's root apply, no
function, block 1), while `n69` is a **clone of a top-level node** (`origin=Some(n21)`)
owned by the call's function `F1`, beside `n43` and `n36`. So the root apply's own node is
unified with a reference's clone and two more nodes, and **the class that carries the
answer never gets a value at all** — the "which generation" question was the wrong one from
the start for this defect: nothing was ever written here.

The next measurement follows from that: trace **writes** to the members of this class
(`n43`, `n36`, `n69`, `n105`) — which of them is written in the working form, by which
node, and what the failing form leaves out.

## 7. The observable: the call's clone is in no class with the cell it should copy

Tracing writes inside the build-time root evaluation (the window is `checker.rs`'s
`evaluate_node_deep(root_term)`), the two forms write the same values into the same roles —
with one exception, and it is the answer's own cell. The top-level leaf `n21` (a cell with
no value of its own) has clones in both forms; writing `3` into a clone reaches, in the
working form, **both blocks**:

```
s1 (answers 3)
WRITE n122 (F1, block 1, origin n21) value=3
      class=[n93(b1)=3 n96(b1)=3 n105(b1)=3 n108(b1)=3 n73(b1)=3 n122(b1)=None]
WRITE n86  (F1, block 4, origin n21) value=3
      class=[…the block-1 clones… n44(b4)=3 n36(b4)=3 n86(b4)=None n71(b4)=None n58(b4)=None]

s2 (answers none)
WRITE n98  (F1, block 1, origin n21) value=3
      class=[n82(b1)=3 n85(b1)=3 n56(b1)=3 n98(b1)=None]
```

In the working form the clones of `n21` in the caller's block and in the lambda's own body
block are **one class**, so the value bound for the call reaches the clone the answer reads.
In the failing form that class contains **only the caller's block**, and the body-block clone
the answer reads (`n69`, `function=F1 block=4 origin=Some(n21)`) is in no class with it — so
**nothing ever writes it**, its read is the last undecided read of the run, and the root
answers `none`.

That is the cause, stated as the measurement gives it: **not a wrong generation of a
closure, but a missing class between the two clones of one cell** — the value the call binds
stops at the clone it was written into. Why the forms differ in that class is the next
question, and the regroups of one call are where to look: in the working form something
unifies the body-block clone with the caller's, and in the failing form it does not.

## 8. Which merge is missing

Tracing every class merge inside the same window names it. The working form makes **two**
merges involving a clone of `n21`; the failing form makes one, and it is not the one the
answer needs:

```
s1 (answers 3)
MERGE a=n122 (F1, block 1, origin n21) b=n105 (top level, block 1, operation, origin n44, value 3)
MERGE a=n96  (top level, block 1, operation, origin n58, value 3)
      b=n86  (F1, block 4, origin n21)                     ← the merge the answer needs

s2 (answers none)
MERGE a=n98  (F1, block 1, origin n21) b=n82 (top level, block 1, operation, origin n43, value 3)
```

So in the working form one merge joins a node that already holds the answer with the
**body-block clone** of the cell the answer reads; in the failing form no merge touches that
clone at all. Which caller performs it — the apply's own result wiring, a group unify, or a
parameter check — is the next measurement, and a site marker on the merge is what names it.

## 9. Which caller, named by a site marker — and the inference it does not support

Each unify site in the apply path carries a marker: 1 is `wire_apply_result`'s
`unify(node, applied)`, 2 is its `unify(cell, item)` — the bind of the checker-wired **result
cell** to the instantiated return's own value — 3 is `unify_clone_groups`, and 4 is
`apply_parameter_check`'s `unify(cloned_param, argument)`. Inside the window:

```
s1 (answers 3): site 1 ten times, site 2 eight, site 3 seven, site 4 nine
   MERGE site=4 a=n122 (F1, b1, origin n21) b=n105 (top level, op, origin n44, value 3)
   MERGE site=2 a=n96  (top level, op, origin n58, value 3) b=n86 (F1, b4, origin n21)

s2 (answers none): site 1 ten times, site 2 six, site 3 five, site 4 nine
   MERGE site=4 a=n98 (F1, b1, origin n21) b=n82 (top level, op, origin n43, value 3)
```

**The counts do not support "two binds are missing", and that reading is withdrawn.** A
`MERGE` line is a *class merge*; a `unify` whose two sides already share a class logs
nothing, so fewer merges is not fewer binds. Measured directly instead: `wire_apply_result`
is called **three times in both forms and takes its expr arm in all three** — so the arm is
not what differs, and the failing form does reach the bind.

What the expr arm does *not* achieve in the failing form is the class itself: there the
result cell's class holds only the caller's block (measured in §7: four members, all in
block 1), while the body-block clone the answer reads is in another class, and in the working
form the site-2 merge is what puts the two in one. So the open question is narrower than a
missing call: **which unify should have put the body-block clone into the result cell's
class, and why it did not** — with the answer's class membership, not the call counts, as the
thing to trace.

## 10. A link measured here, not the last one: the wiring names the expr's other cell

Tracing every `unify` whose either side is a clone (the entry, so a no-op unification is
logged too) answers it. In the failing form **no unification mentions the cell the answer
reads at all** — not even a no-op one — while the working form has exactly one that does:

```
s1 (answers 3), its last two events
UNIFY a=n113 (top level, origin n54, value array) b=n87 (F1, block 4, origin n22)
UNIFY a=n96  (top level, origin n58, value 3)     b=n86 (F1, block 4, origin n21)   ← the answer's cell

s2 (answers none), its last three events
WIRE-ARM node=n61 cell=Some(n59) result=array arm=expr
UNIFY a=n61 (top level, operation) b=n75 (top level, operation, origin n55)
UNIFY a=n59 (top level, origin None, value None)  b=n70 (F1, block 4, origin n22)   ← the other cell
```

So the failing form's wiring binds the **result cell** to the body-block clone of `n22` — the
other cell of the same top-level expr — and the clone of `n21`, which the answer reads, is
never offered to any unification. The working form binds both. That measures **which node the
apply's return expr names**, and at the time it was written this looked like the last link; it is
not, and §26 has the cause. What it does establish is the shape of the loss: in the failing form
the cell nothing offers to a unification is the one the answer reads.

## 11. What the answer's cell is, and a reading the confirmation refuted

Reading the undecided cell together with the nodes that name it identifies its role without
any prior id knowledge — it is an operand of the body's own apply, and it is a clone of a
top-level leaf, which is where a lambda's parameter cells live:

```
READ-NONE n69 (F1, block 4, leaf, origin n21)
  referrers=[ n55 (F3, block 4, operation)          ← the body's apply, which names it
              n61 (top level, operation)            ← the root apply
              n68 (F4, block 4, leaf, origin n23)   ← a clone of the *expr*, not of a cell
              n71, n100 ]                           ← its own clones
```

Printing a function's own shape then **refutes** the reading this section first carried — that
one parameter has two template nodes:

```
FUNCTION-SHAPE function=F1 parameter=n23 items=[n21 n22] return=n32
```

`n23` is the parameter's **expr** node and `n21`/`n22` are its two **cells** (value and type).
So the parameter check binds a clone of the *expr* (`origin=n23`) while the answer reads a
clone of the *value cell* (`origin=n21`) — the two origins are an expr and one of its cells, not
two templates, and "one parameter, two template nodes" is withdrawn.

What stands, measured repeatedly: in the failing form the value cell's clone is neither
written nor ever offered to a unification, while the wiring's other bind lands on the type
cell's clone. So the last question is why the parameter check's structural descent does not
reach the value cell's clone in the failing form, when it does in the working one.

## 12. The two clones are real; the double-instantiation reading is withdrawn

Comparing **within a single run** (ids drift between runs, so only a same-run comparison is
valid) shows one template with two clones in one call:

```
s2 (answers none)
DESCEND index=0 a=n85 (origin=Some(n36), no value) b=n56 (no origin, value 3)
DESCEND index=0 a=n98 (origin=Some(n21), no value) b=n82 (origin=Some(n43), value 3)
READ-NONE      n69 (F1, block 4, origin=Some(n21)) with its class root holding no value
```

`n98` is a clone of `n21` that a parameter check *does* unify with the value; `n69` is a
clone of the same template that the answer reads and that no unification touches. The two are
in different classes. The descent is not what fails — it reaches the value side in both
forms, in identical 36-step traces.

**The explanation this section first carried — that a call instantiates its function twice —
is withdrawn.** Instrumenting every instantiation path and printing the path with the
function, the sequence is *identical* in all five measured programs, each function once, all
through the value path:

```
m4 (none)  F3/s1 F1/s1 F4/s1      s1 (3)     F3/s1 F1/s1 F4/s1
n1e (3)    F3/s1 F1/s1 F4/s1      s2 (none)  F3/s1 F1/s1 F4/s1
m3 (3)     F3/s1 F1/s1 F4/s1
```

The `F1, F4, F3, F1, F4` sequence quoted here earlier came from a different print in an
earlier round and a different program expr, and citing it as this expr's trace was a mistake.
So the double instantiation is refuted, and the last link is open again: **why one
instantiation leaves two clones of one template in one call, with a value in one and nothing
in the other.**

## 13. What has been ruled out, and what is left

Each of these was a candidate cause, and each was refuted by the instrument built for it. This is the
ledger the later sections add to, and it is the count the standing section quotes:

| # | candidate | refuted by |
| --- | --- | --- |
| 1 | two fewer result-cell binds | the arm trace: three wirings, all the expr arm, in both forms |
| 2 | one parameter with two template nodes | the function's own shape: the node is the expr, the two are its cells |
| 3 | the descent not reaching the value cell | the descent trace: identical 36-step traces |
| 4 | one call instantiating twice | the path trace: identical sequences in five programs |
| 5 | the walk remapping the parameter's cells differently | the walk trace: 18 identical events, same remap states |
| 6 | walk identities differing | the failing read is walk-less |
| 7 | the function's return wired to an operation | `t1`, `z1` and `r1` are wired the same way and answer correctly |
| 8 | stale references leaking into the graph | a full scan finds ninety in every program, working ones included |
| 9 | the carrier-plus-reader story (a subagent's) | its reader half fires nowhere, and its counterexamples keep the stale item empty |
| 10 | the three reachability fields (class, array items, plus apply-to-return edges) | each reports the same value for a failing and a working form |
| 11 | the applied-instance event as the cause | necessary over the corpus (19 of 19) but failing sufficiency by 236 |
| 12 | the in-place-versus-clone decision | the failing form leaves nothing in place; the difference is one of scale |
| 13 | the carry of all-pre-window-clone arrays | suppressing it changes no answer in the corpus, and it fires in only 10 of the 19 |

Everything structural measured so far — walks, remaps, descents, instantiation paths, merge
sites, arms — is **identical** between a form that answers `3` and one that answers `none`.
What differs is only *where a value lands*: one clone of one template receives the value, and
the clone the answer reads receives nothing and is never offered to a unification. So the
remaining question is one of **which context's clones a read resolves in**: the answer's read
may land in a clone minted by the *inner* apply's nested walk, while the parameter check binds
the clone of the outer one. Tagging a walk with the context that made it is the next
measurement, and nothing is claimed about the answer until that trace says so.

## 14. Which walk a clone belongs to

Tagging every walk with an identity (a per-walk id, carried on a thread-local stack) and
printing it beside each parameter clone, each parameter check and each undecided read gives
one sharp difference, and it is not in the walks themselves:

```
s2 (answers none), its tail
PARAM-CHECK function=F1 cloned=n97 walk=Some(17)
INSTANTIATE function=F4 walk=18 … PARAM-CHECK F4 walk=Some(18)
READ-NONE node=n69 f=Some(F1) b=b4 origin=Some(n21) walk=None

s1 (answers 3): no READ-NONE line in the window at all
```

Two facts, both new: the failing read happens **outside every walk** (`walk=None`) — it is the
top-level evaluation reading after all instantiations finished — and the node it reads is a
clone **minted by F1's walk** (`walk=Some(17)`) whose origin is **not that function's
parameter**: F1's parameter in the same run is `n29` with cells `n27`/`n28`, so an origin of
`n21` names something else entirely.

So the shape's failing read is a top-level, walk-less read of a clone that a walk minted for a
node which is not the parameter being checked — and the working form performs no such read.
Identifying that origin's role is the next measurement, and no cause is claimed before it.

## 15. The clone the answer reads belongs to an earlier walk

A side table recording, for every clone, the walk that minted it, resolves the apparent
contradiction of one walk minting one clone per cell:

```
s2 (answers none)
READ-NONE n69 (F1, block 4, origin n21) walk=None minted=Some(14)
  same-origin = [ n69 (block 4, no value,        minted=14)   ← what the answer reads
                  n98 (block 1, value USize(3),   minted=17) ] ← where the value landed
  class       = [ n43=None, n36=None, n69=None (minted=14) ]
```

One template cell has **two clones from two different walks**: the value lands in the clone of
walk 17 and the answer reads the clone of walk 14. Which walk is which is then plain:

```
s2 (none)  F3/w1 F3/w2 F8/w3..w5 F1/w6..w14 F4/w15 | F3/w16* F1/w17* F4/w18*
s1 (3)     F3/w1 F3/w2 F8/w3..w5 F1/w6..w14 F4/w15 | F3/w16* F1/w17* F4/w18*
m4 (none)  F3/w1 F3/w2 F8/w3..w5 F1/w6..w14 F4/w15 | F3/w16* F1/w17* F4/w18*
```

The sequences are **identical in all three** — including eleven instantiations of F1 before the
window — so the number of check-time instantiations is not what separates them. What separates
them is **which generation's clone a read resolves to**: the failing form's answer reads the
clone minted by walk 14, a generation from before the root's own evaluation, and the value
lands in walk 17's clone of the same template. The working form performs no such read.

So the cause is a **reference that names an earlier generation's clone**: the shape leads the
check to wire a read to a clone an earlier instantiation minted, while the value is bound in
the instantiation the program's own evaluation makes. Which reference that is, and which walk
mints the node the working form reads instead, is the next measurement.

## 16. The reference, named

Printing the nodes that name the stale clone — with the walk that minted each — names the
reference:

```
READ-NONE n69 (F1, block 4, origin n21) minted=14
named-node-by = [ n55 (F3, block 4, operation, value expr) minted=None
                  n61 (top level, operation)                minted=None
                  n68 (F4, block 4) minted=14, n71 (F3, block 4) minted=15,
                  n75 (top level) minted=16, n100 (top level) minted=18 ]
```

`n55` is the return node of the apply-bodied function — the measured
`FUNCTION-SHAPE function=F3 parameter=n38 items=[n36 n37] return=n55 (operation)` — so the
**body's own apply operation names the clone minted by the check-time walk 14**, while the
value is bound in walk 17's clone of the same template cell. A clone has been written where
the operation's operand should name the template cell or the run-time generation, and the two
are in different classes, so the run-time binding cannot reach the position the body reads.

The next measurement is the operand array of that return operation in the failing and the
working form, item by item: which item names the template cell and which names a clone, and
which walk minted it — the last step before this is stated as the cause.

## 17. The return shape is not the cause either

Printing whether a function's own `r#return` is an operation separates the two forms measured
first — the failing ones wire it to an operation, the working ones to an expr:

| program | answer | the apply-bodied function's `r#return` |
| --- | --- | --- |
| `s2`, `m4` | `none` | operation |
| `s1`, `n1e` | `3` | expr |

**That reading is withdrawn: it fails its own prediction.** If an operation-wired return caused
the loss, every program with one would lose it — and these do not:

| program | answer | window instantiations | of which operation-wired |
| --- | --- | --- | --- |
| `m4` | `none` | 3 | 1 |
| `t1` (`f = n => (mk 3) 1`) | `3` | 1 | **1** |
| `z1` (inline callee) | `3` | 3 | **1** |
| `r1` (`0 + (…)`) | `3` | 3 | **1** |
| `r4` (`[…]`) | `[3]` | 3 | 0 |

So an operation-wired return is necessary-looking but not sufficient, and it is not the
differentiator. What survives the refutation is the observable the whole investigation keeps
returning to, measured within single runs: **the failing form's answer reads a clone minted by
an earlier walk than the one that binds the value**, and no unification ever touches that
clone, while the working form performs no such read.

**That observable is not "stale references leak into the graph" either.** Scanning, at the end
of the window, every live reference to a clone whose minting walk is older than the window
gives the same count in every program measured — ninety, in both the failing and the working
forms, including the forms that answer correctly — so references to earlier generations are
ubiquitous here and separate nothing.

## 18. The instrument, and what it does not do yet
`crates/lichen-highlevel/tests/highlevel/zz_nested_apply_probe.rs` reproduces the failing shape at
the IR level and prints readings by name: the root, body and call answers through their
exprs' own value slots, each expression's expr together with its class and its class
members' slots. It reproduces the undecided root — and it reports an undecided root for
the **literal control** as well, which the language front end answers `3`. An IR probe
carries no core prelude, so it is not a faithful runner for a program's answer (the
TestKit section of `AGENTS.md` says which half to use for which question); the
language-level programs above remain the measurement for behaviour, and this probe is the
instrument for the *shape* — which expression holds which node, and what its class
answers.

The probe needed three readings the harness did not have, and they are now in `Probe`:
`try_value_of`, `try_node_of` and `class_of`, with the lowlevel read they rest on,
`class_members_with_values`. What each is for is stated once, in
[the harness note](test-harness.md), which is where the readings belong; what this
investigation needed them for is the distinction below.

## 19. A subagent's chain, and the corrections it forces — refuted below, cause in §26

A dedicated investigation (a subagent, briefed with this note, working only with traces) proposed a
chain and corrected three of this note's readings. **Its causal story does not survive this section's
own last measurement** — the carrier it rests on is a correlate with twenty-seven counterexamples and
its reader half fires nowhere — so read what follows as the record of a refuted chain and its three
useful corrections, not as the mechanism; §26 has the cause. Its chain, with the parts this note had
already measured marked:

1. `checker.rs` evaluates **every function's `r#return` once** at check time, and `check_lam`
   wires `r#return` to `check_term(body)` — for the failing shape, the apply's operation node
   (§4). So the failing shape's body **runs at check time**.
2. That run instantiates the callee (`mk`, a top-level binding, already concrete), and
   `wire_apply_result` writes the result — the closure instance with its **capture cells** —
   into the apply node's own value slot. Its print `RETURN-CACHE` shows exactly that on `f`'s
   return node, whose items are clones minted by a **check-time** walk of *another* function's
   parameter cells, empty.
3. The run **carries** that cached array onto the per-call clone, so the call applies the
   **check-time instance** rather than the one the call itself instantiates; the argument is
   bound into the run-time instance's clone, and that instance is never applied (§15's two
   clones of one template, minted 14 and 17).
4. The failing read is the root's own descent into the carried array (§14: walk-less, minted by
   the earlier walk, in no class with the value).
5. **The discriminator is a reader, not a generation**: in every working shape the lambda's body
   value is read by an `Index` node **inside the lambda's own scope**, whose `alias_read` joins
   the reader's class into that stale capture cell, so `fill_class_holes` writes the argument
   into it. In the failing shape `r#return` *is* the value node, so nothing reads it and no such
   join happens. That is also why the callee's form and the argument's form matter: only a
   concrete callee is instantiated at check time (inline lambdas are not), and only a
   parameter argument leaves the cached cell empty (a literal is already in it).

**Corrections this note accepts**, all of which its own instruments support:

- §16's "the body's apply operation **names** a check-time clone" is **withdrawn**: the stale
  node is in the apply node's **own value slot** (the cached result array), not in its operand
  array. The referrer scan that produced §16 treated *any* array-valued node as naming its
  items, so it read a node's own value slot as an operand reference — visible in the instrument's
  own code, and the reason §16's claim is retracted.
- §13's "the answer's read may land in a clone minted by the inner apply's nested walk" is
  **wrong**: the clone is minted by the instantiation of the callee at the *outer* apply node,
  at check time.
- §15's finding stands but is **mislocated** the same way: the reference is a carried cached
  value, not a reference the walk left in the operand graph.
- §17's withdrawal is confirmed and sharpened: the working forms also apply the same
  check-time instance, so "which generation is applied" is not the discriminator.

**What the mechanism still owes**, stated so it is not mistaken for settled: its discriminating
predicate (a cached return array naming another function's still-empty cell, *and* no in-scope
reader) was measured over twelve programs, not the whole corpus, and **the corpus-wide test
does not reproduce it**. Running the whole scratch corpus (424 programs that answer at all) with
a deliberately weaker first half — "a `RETURN-CACHE` whose items include a clone that still
holds nothing" — gives:

```
answers none: 16, of which carrying: 15
answers other: 408, of which carrying: 261
```

So the first half alone is far too weak to be the predicate: it is present in most programs that
answer correctly. That does not refute the conjunction, because the second half — *no in-scope
reader of the value node* — is what excludes those 261, and **its instrument is not in the
delivered patch**: the report quotes `INDEX-READ-CLONE` lines that no committed or saved patch
prints, so the twelve-program table cannot be reproduced as stated. One program (`m2`) answers
`none` without the weaker predicate matching, which is either a second mechanism or a parsing
miss (the scan matched item text after `items=[` with one regular expression).

Independent corroboration already in hand: this note's own §7 measurement — the working form
writes `USize(3)` into a body-block clone of the parameter cell and the failing form never does —
and the site-6 (alias-read) merges inside the window, two in the failing form and four in the
repaired one. Those stand on their own; the conjunction does not, until its second half is
instrumented and the corpus test is re-run with it.

**The first half, rebuilt and measured.** A predicate printed at the end of each program's own
build — "some function's return node holds an array of cached clones, and one of them is owned by
another function and still holds nothing" — reads:

| program | answer | first half | stale items |
| --- | --- | --- | --- |
| `m4`, `m2`, `s2` | `none` | **true** | 2, 1, 2 |
| `n1e`, `s1`, `t1`, `r1`, `r4`, `p3` | `3`, `3`, `3`, `3`, `[3]`, `2` | false | 0 |
| `z1` (inline callee) | `3` | **true** | **4** |

So it is necessary-looking — every program answering `none` carries it — and **not sufficient**:
`z1` carries four stale items and answers `3`, which is precisely why the mechanism's own reading
needs its second half. That half is **still not instrumented**: the rebuild's reader test (an
`Index` node naming the stale cell) never fires in any program, so its operator test does not
match, and the conjunction stays untested.

Run over the whole corpus, the first half reads:

```
programs scanned: 424
answers none:  16, all 16 carrying          — no false negatives
answers other: 408, 27 carrying             — 27 false positives
sample: bs-c1l-litcallee, g1, h1, h2, h4, i1, i2, i3, i5, i6, j3, j6, k3, k4, k5, …
```

So the predicate has full recall and a small exclusion set, and the mechanism says that set is
exactly the programs a reader rescues. Those 27 programs are the test set for the second half:
when its instrument matches (`Index` operators are matched by the evaluation's own route, which
the rebuild's test did not use), it must fire for them and for no other reason.

**And that is where the mechanism fails its test.** The reader half was rebuilt against the
carrier itself — an `Index` whose array operand names the cached array, matched exactly as the
evaluation matches operators — and it fires in **no** program, the failing ones and the
twenty-seven alike. Worse, the stale item is still **empty at the end of the run** in the
twenty-seven:

```
g1: carrier=true stale=1 (still empty) answer 3      h1: carrier=true stale=1 (still empty) answer 6
i3: carrier=true stale=1 (still empty) answer 2      k4: carrier=true stale=1 (still empty) answer 6
m4: carrier=true stale=2 (still empty) answer none
```

So an empty foreign cached clone is a **common, benign** state — it coexists with a correct
answer and nothing ever fills it in those programs — and the mechanism is **refuted as the
complete cause**: its first half is a correlate with full recall but twenty-seven counterexamples,
and its second half has no instrument that fires. The earlier acceptance of that mechanism,
recorded in §19's opening, is withdrawn with it; what stands from it are the parts this note
measured independently (the carrier's existence and content on `m4`, and the alias-read merge
counts), not the causal story built on them.

## 20. The differential field that does discriminate, and its counterexamples

A field the investigation had never recorded — whether the instance the call applies is one the
**run itself minted** — splits the programs where the carrier alone could not:

| program | answer | carrier | applied a run-minted instance |
| --- | --- | --- | --- |
| `m4`, `m2`, `s2` | `none` | true | **false** |
| `z1`, `g1`, `i3` | `3`, `3`, `2` | true | **true** |
| `h1`, `k4` | `6`, `6` | true | **false** |
| `n1e`, `s1`, `t1`, `r1` | correct | false | false |

So every program answering `none` carries a stale cached clone **and** never applies an instance
it minted itself, while the three that carry the same cache and answer correctly do apply one —
which is the rescue the earlier mechanism described. Two programs (`h1`, `k4`) carry the cache,
apply no run-minted instance, and still answer correctly: they are the remaining counterexamples,
and the narrowest set to differentiate against. Nothing is concluded from this field yet beyond
that split.

**A field that came back vacuous, recorded so it is not retried.** Asking whether the answer's own
node shares a class with a stale item sounds like the right differential — in the failing form the
answer's read *is* the stale cell — but the field is false in **every** program, the failing ones
included, because an apply root has no value node at all: `state[root].val` is empty by
construction for an apply, and the answer lives in the root expr's own value slot. Measuring
"where the answer comes from" therefore has to start from that slot, not from the root's value
node — the same distinction the TestKit reading `try_node_of` was added for in this note's earlier
work.

**Its successor came back vacuous for a second reason.** Starting from the root apply's result cell
and descending through array items — the shape the failing read takes — still reports false
everywhere, the failing form included, because the edge that matters is not an array item: an apply
reaches its callee's **return node** through the function it applies, and the carried cached array
lives on that return node. A reachability field therefore has to follow apply-to-return edges as
well as array items; without that edge the field cannot see the path this note measured directly.

**And with that edge added it is still false in every program**, measured. Three successive fields —
class membership of the answer, array-item reachability from the result cell, and the same
reachability with apply-to-return edges — report the same value for a form that answers `none` and a
form that answers `3`. They are recorded here as attempted and vacuous, with the reason each was
expected to work, so that the next attempt does not start from any of them.

## 21. Where the question stands

**The answer this note can defend.** The failure is *exactly* characterised — a program answers `none`
if and only if its root evaluation performs a walk-less read of a node minted by an earlier walk, which
holds on all twenty-two failing programs and on none of the other four hundred and seven — and its cause is
named: the checker evaluates every function's `r#return` once, by design, and for this shape that node is
the body's apply, so the pass **runs the call with the parameter unbound**, mints the generation whose
cells stay empty, and its own invariant pins that generation into the run; the run's binding then goes to
a clone of its own generation while the answer's read lands on the pinned, empty one. Every link of that
chain is measured or read in code (§26), including the pass that mints it, and it accounts for every
shape discriminator this note measured (§1). What remains open is narrower and is recorded as such: two
programs (`h1`, `k4`) share the pinned generation and still answer correctly, and one more (`m2`-family)
reading of the two instances' cells is unexamined.

**Why `f = n => (mk n) 1; f 3` answers `none`, end to end.** Each step is measured or read in code, and
the question the investigation was opened for is answered by them:

1. the root expression is an apply, and the checker deep-evaluates the root term at site 9 (measured);
2. evaluating that apply runs the call (the deep walk's own body, line 536);
3. the value the call produces is an array, and one of its items is a clone minted by a **check-time**
   walk — measured: origin `n21`, minted by walk 14, read with no walk running;
4. the deep walk descends into that item (lines 538–550) and reads it;
5. the run's binding writes a **different** clone of the same template — measured: minted by walk 17 —
   so nothing ever binds the one being read;
6. the read is therefore undecided and the root's answer is empty, which is the `none`;
7. and the characterisation is exact, not one story among many: the same signature holds on every
   failing program in the corpus and on no other.

What the refutations above leave open is a question *upstream* of this one: why the value the run
produces contains a clone from a check-time walk at all. That question is answered later in this section
— the checker's own per-function return pass runs the body's apply with the parameter unbound and pins
the generation it mints — and the thirteen refutations in the ledger are what that answer had to survive.

**Established, reproducibly.** The shape that fails, narrowed by measurement (§1). The observable:
in the failing form the answer's read lands on a clone minted by an earlier walk than the one that
binds the value, in a class no unification touches, while the working form performs no such read
(§14, §15) — all of it measured within single runs. The shape ladder that every working variant
satisfies (§1, §5). Two corpus-level fields that correlate: the carrier (full recall over the
programs answering `none` — nineteen when that scan ran, twenty-two now — with twenty-seven
counterexamples) and whether the call applies an instance it minted itself (§20). The cause, with
every link measured or read in code, in §26: the checker's per-function return pass runs the body's
apply with the parameter unbound, mints the empty generation at site 6, and pins it into the run.

**Refuted along the way.** Thirteen candidates, each by the instrument built for it, each recorded in
the ledger with the reason it failed — the result-cell bind count, two template nodes for one
parameter, the descent's reach, double instantiation, the remap decisions, walk identities, the
return's shape, stale references in general, a subagent's carrier-plus-reader story (its reader half
fires nowhere, and its counterexamples keep their stale item empty), the three reachability fields,
the applied-instance event as a cause (necessary, failing sufficiency by 236), the in-place decision,
and the carry of all-pre-window-clone arrays. Four of those were this note's own
instruments coming back vacuous rather than refuting anything.

**For whoever picks this up.** Everything needed is outside the branch: the reproduction programs,
the instrumentation patches with what each prints, and the two scan scripts and their CSVs are in
`C:\resource\lichen-vm\.scratch-core\`, and §28 gives the commands. Ids drift between runs, so
compare within one run or by origin and value. The narrowest unexplored handle is the two-program
counterexample set (`h1`, `k4`): they carry the same cache and apply no run-minted instance, and
still answer correctly — so whatever separates them from `m4` is what the mechanism turns on.

Three angles this investigation never watched, recorded because the refutations above say what does
*not* separate the forms and these are where nothing has been looked at yet:

- **the check-time pass's own decisions.** Every decision print in this note is gated on the run's
  window (`IN-PLACE`, `CLONED`, `CARRIED`), and the stale clone is a *check-time* artefact, so the
  decisions that produced it were never observed. The same prints without the gate are the first thing
  to read. **Read, and they do not separate either**: with the gate lifted, the failing and the working
  form both make **five** in-place decisions during the check-time pass, and differ only in how many
  clones they make (144 against 174) — the same scale-only difference as the in-window counts, so this
  is the fourteenth reading that separates nothing.
- **the two instances' cell sets, side by side.** The comparisons here are all of one form's states
  against the other's; the check-time instance's cells and the run's were never listed against each
  other within a single program, which is where "the value went to the other clone" would show up.
  **Listed, and this one does differ**: at the end of the run the working form's *template* instance
  holds the argument in its middle cell (`USize(3)`), while in the failing form the template's cell and
  both minted instances' corresponding cells are all empty — the value is in no instance the listing
  shows. That is the second state difference this note has found and the first about *where a value
  landed*; like the carry's firing before it, it is a difference, not a cause, until a probe moves it.
- **what the produced value's array *is*.** Its items were printed with owners and minting walks but
  never identified as a role — result cell, operand, capture holder — so the answer's own top-level
  structure has not been named. **Answered instead by searching for the value**: listing every node that
  holds the argument at the end of the run gives eight nodes in the failing program — the literal, two
  clones from walk 16, and the run's own clone from walk 17 — and **none from the check-time walk 14**,
  while the working program lists seventeen, among them a clone minted by walk 14 that does hold it.
  Since the failing read lands on a walk-14 clone, the sharpest statement this note has is that in the
  failing form the check-time generation's clone never receives the argument and in the working form it
  does. That is the third state difference and the first that names the value's destination; a probe
  that moves it would have to write through the class machinery rather than directly, which is where the
  earlier mirror attempt failed.

**Why the clone exists: the checker runs every body once, by design.** The code says so at the site
itself:

```
checker.rs:557   // Every body runs once, so its checks fire even unapplied.
                 // Invariant: the deep pass pins what the clone copies.
                 checker.module.evaluate_node_deep(ret, None);
```

For the failing shape, `ret` — the function's `r#return` — **is the body's apply** (§4), so this pass
does not merely inspect the body: it **runs the call with nothing bound to the parameter**, instantiating
the callee and minting the walk-14 generation whose cells stay empty, and the pass's own invariant then
*carries that generation into the run*. The run binds the argument into its own generation's clone, and
the answer's read — descending from the root into the produced value's items — lands on the pinned, empty
one. That is why the value is in none of the instances a run-side listing shows.

It accounts for every discriminator this note measured, including the ones measured before it was known:

- the body must be **exactly** the apply — otherwise the return node is not something the pass can run;
- the callee must be a **top-level binding** — concrete at check time, so it can be instantiated with no
  argument;
- the argument must be the **parameter** — that is the cell nothing binds in that pass;
- **any wrapper or statement repairs it**, because the return stops being an apply (`t1`'s literal
  argument needs no binding at all, which is why it answers correctly);
- a **local** lambda repairs it — and that is the first discriminator this note ever measured (§1),
  because a local lambda is not in the checker's `functions`, so the pass never evaluates its return.

The residual is the same two programs (`h1`, `k4`), whose closures read their own parameter. One print
would still pin the last link — the minting walk of the check-time generation against the site number —
but the behavioural predictions above already hold on every measurement this note made.

**And the residual is not a counterexample: those programs fill the pinned generation.** Searching for
the argument at the end of each run:

```
m4 (answers none): 8 nodes hold 3, and none of them was minted by a check-time walk
h1 (answers 6):   24 nodes hold 3, four of them minted by walks 14 and 15
k4 (answers 6):   24 nodes hold 3, four of them likewise
```

So the two programs the mechanism seemed to miss are the ones whose check-time generation *does* receive
the value, and the failing program is the one whose does not. That makes the mechanism's condition sharper
and testable: the failure is that the argument never reaches the generation the checker's pass pinned,
which holds on the failing program and on both of the programs that answer correctly.

**Predictions on cases the mechanism was not derived from, and they hold.** Two programs written for this
purpose only:

```
w2 = mk = x => y => x ; f = n => (mk n) 1 ; 7        answers 7    — f is never called
w4 = mk = x => y => x ; f = n => (mk n) 1 ; g = n => (mk n) 2 ; f 3   answers none — two such functions
```

The second says the pass is per function and by shape, and the first says its run is inert unless
something reads the generation it pins: a body the program never calls still gets run by the checker's
pass, and the program is unharmed because nothing reads what that run produced. That is the strongest
support the mechanism has, since neither program was used to build it.

**Three more, and one of them tests the mechanism's own internal claim.**

```
w6 = mk = x => y => x ; h = t => { f = n => (mk n) 1 ; f 3 } ; h 0     answers 3
w7 = mk = x => y => x ; f = n => (mk n) 1 ; (f 3, f 4)                 answers (none, none)
w8 = mk = x => y => x ; f = n => (mk n) 1 ; h = k => k 3 ; h f         answers none
```

`w6` carries the **identical failing body** and answers `3`, only because its lambda is a **local**
binding and therefore not among the functions the checker's pass iterates — which is the direct test of
the claim that the pass is what runs the body, and it passes. `w7` loses both calls' answers, so the loss
is not per call site but per pinned generation, and `w8` loses it when the function is passed as an
argument too. Five predictions made on programs the mechanism was not derived from have now held, and none
has failed.

**The antecedent holds in every failing program.** Reading the set as it stood when this was checked: of
those nineteen, eleven have a lambda whose value *is* the apply (`f = n => mk n 1`), and eight have a block
whose value is (`f = n => { a = n + 1; b = add a a; b }`), so in **nineteen of nineteen** some function's
return node is the apply the checker's pass then runs unapplied — and the three programs that have joined
the corpus since are the same shape. That is the mechanism's necessary half, checked on the whole failing
set rather than on the three programs it was derived from; the sufficient half is the read-side signature,
which is exact over the corpus and says the value landed on a sibling clone (§26).

**And the sufficient half does not generalise, which is recorded here rather than left standing.** Running
the value search over every program whose final line is a call with a literal argument (145 of them), and
asking whether any clone minted before the window holds that argument:

```
answers none:  18 — and three of them DO have such a clone: g3, h3, h5 (all of family B)
answers other: 127 — and forty-five of them do not
```

So "the argument never reaches the pinned generation" holds in the three programs it was derived from and
fails on family B, which loses the value *with* a pre-window clone holding it, and it is common among
programs that answer correctly. It is withdrawn as a condition: family A's neighbourhood is explained by
it, family B is not, exactly as in §23 where the two families' behavioural rules were found to be
opposites.

**One attempt at family B's read-side, and it produced nothing because the instrument was broken.** The
plan was to print, beside each undecided read, every node sharing the read node's `origin` — so that
"same template cell, one clone filled and another read" would be visible in one line. Three successive
edits to that print left the crate not compiling, so the runs that followed used the previous binary and
their output describes the *old* print, not the new one. The tree was restored instead of drawing
conclusions from it, and the reading is left undone: `condition-scan.patch`'s neighbourhood is where to
retry, with the edit made in one piece rather than by substitution.

**Retried in one piece, and it returns the same signature in both families.**

```
m4 (family A): read n69  — walk 14, origin n21, walkless, empty — sibling n98 (walk 17) holds 3
g3 (family B): read n127 — walk 14, origin n21, walkless, empty — sibling n314 (walk 17) holds 4
h3, h5       : the same shape, the sibling likewise holding 4
```

In all four the read lands on a check-time clone of the parameter cell that nothing filled, while its
later sibling holds the value the closure's parameter should have received — and for family B that value
is `4`, the local `a = n + 1`, which is precisely the ingredient §23 isolated for that family. So the two
families share the read-side signature after all, and the three "exceptions" recorded a moment ago are
withdrawn: that scan looked for the *final call's literal argument* (`3`) while the value that matters for
the closure's parameter is the local's (`4`). It measured the wrong value, not a different mechanism.

**Swept over the corpus, that signature is the second exact predicate.**

```
programs scanned: 429
answers none:  22 — all twenty-two have a read whose same-origin sibling holds a value
answers other: 407 — none does
```

Twenty-two rather than nineteen because the corpus grew with this note's own later experiments (`w4`,
`w5`, `w8` all answer `none`). So "an undecided read whose sibling holds the value the closure's parameter
should have received" holds on every failing program and on no other, and unlike the fingerprint it says
*where the value went* rather than only that a read was made. A first run of this scan reported one
over-prediction (`b1`); that was this print's fault, not the program's — the rewrite that added the
sibling list had dropped the guard that a sibling comparison only makes sense for a node that *has* an
origin, so for `b1`'s origin-less read it compared against every other origin-less node and matched an
unrelated one that happened to hold a value. With the guard restored the predicate has no false positive
at all.

**And the last link is pinned, with a detail that explains the identical readings.** Printing the site
number beside each mint:

```
m4 (answers none): CLOSURE-MINT site=6 walk=14 source=F2 fresh=F4 window=false
                   CLOSURE-MINT site=9 walk=17 source=F2 fresh=F5 window=true
v3 (answers 4):    CLOSURE-MINT site=6 walk=14 source=F2 fresh=F4 window=false
                   CLOSURE-MINT site=9 walk=17 source=F2 fresh=F5 window=true
```

The check-time generation is minted at **site 6** — the `evaluate_node_deep(ret, None)` pass — exactly as
the mechanism says. And the two forms mint it **identically**: same site, same walk, same instance ids.
That is why every state, content and decision comparison in this note came back identical — they were
comparing the outputs of a pass that runs the same way in both forms. What differs is what happens to the
pinned generation afterwards, and the measured statement of that is narrow: in the failing form the value
ends up in a **later** clone of the same template (walk 17) while the answer's read uses the **pinned,
empty** one (walk 14), and in the working form the read does not land on the pinned generation at all. So
the two generations are real and the value is in the wrong one — **which instance the run's own apply
invokes** is a further question, and it is the one the refuted chain in §19 asserted rather than measured,
so this note does not claim it.

**And that mint happens in every failing program.** Swept over the corpus:

```
programs scanned: 429
answers none:  22 — all twenty-two mint a closure instance at site 6
answers other: 407 — forty-three of them do too
```

So the pass's run is the mechanism's *antecedent*, confirmed on the whole failing set rather than on the
two programs the site number was first read from, and it is necessary rather than sufficient — which is
what the mechanism predicts, because those forty-three answer correctly for the reason the read-side
predicate captures: their reads do not land on the pinned generation.

## 22. The corpus answers `none` in three families

Reading the programs of the corpus that answer `none` — nineteen now, sixteen when this section was
first written — shows they are **not one shape**: they are three, and every mechanism this note has
examined was fitted to the first:

| family | programs | text |
| --- | --- | --- |
| **A** | `j2`, `k6`, `m2`, `m4`, `o2`, `o9`, `p1a`, `s2`, `s3`, `u2`, `v4` | the closure's body reads only its capture (`mk = x => y => x`) and the lambda's value is that apply |
| **B** | `g2`, `g3`, `h3`, `h5`, `h6`, `b6`, `b7` | a block with a local hop: `f = n => { a = n + 1; b = add a a; b }` |
| **C** | `i7` | the closure is returned and applied in another call: `f = n => add n; g = f 3; g 4` |

Those are the nineteen programs the corpus's fingerprint scan reports, and three of them — `b6`,
`b7` and `v4` — are programs this note's own experiments added while it was written, which is why an
earlier scan over the same directory saw sixteen.

The behavioural 2×2 that produced this reading, on family A's neighbourhood:

| closure body | outer argument | program | answer |
| --- | --- | --- | --- |
| capture only (`x`) | literal | `m4` | `none` |
| capture and own parameter (`x + y`) | literal | `v3` | `4` |
| capture only (`x`) | the parameter | `v4` | `none` |
| capture and own parameter (`x + y`) | the parameter | `h1` | `6` |

So *within family A* the flip is whether the returned closure's body reads its own parameter, and
the whole corpus' correct programs that carry the cache read it (`h1`, `h2`, `h4`, `i1`–`i6`, `j3`,
`j6`, `k3`, `k4`, `k5`, `g1` all compute `x + y`). But family B reads its own parameter and answers
`none` anyway, so that reading does not generalise — and family C is a third configuration again.
Any mechanism claimed for this note's observable must be stated for the family it was measured in,
and families B and C remain unexamined.

## 23. Family B's rule, isolated

Minimising family B by behaviour alone gives one rule, and it is a different rule from family A's:

| program | text | answer |
| --- | --- | --- |
| `h2` | `f = n => { b = add n n; b }` — the closure's own parameter comes from **the parameter** | `6` |
| `g3` | `f = n => { a = n + 1; b = add a a; b }` | `none` |
| `b6` | `f = n => { a = 1; add n a }` — a **literal-bound local**, no hop | `none` |
| `b7` | `f = n => { a = n + 1; add a a }` — no hop at all | `none` |

So in family B the failure needs only that the **closure's own parameter be supplied from a local
cell**: whether that cell holds a literal (`b6`), a computation (`b7`, `g3`) or the parameter itself
(`h6`) makes no difference, and the local hop is irrelevant (`b6` and `b7` have none). Supplying it
from the parameter or from a literal directly is correct (`h2`, `i1`, `i2`).

That is the **opposite** of family A, where supplying the parameter to the inner apply is what fails
— so the two families are two rules, which is why every single mechanism examined here fitted one of
them and broke on the other. They can be described together only loosely: in both, a value bound for
the applied closure lands in a cell that closure does not read — the **capture** in family A, the
closure's **own parameter** in family B.

## 24. The observable is shared by all three families, once placeholder reads are excluded

A first version of this section concluded from undecided-read counts that the families were three
different defects. **That conclusion was an artefact of the instrument and is withdrawn.** The
counts included reads of cells that `node_apply` mints per reach as a wildcard's replacement —
which are *meant* to be empty — so a program with more wildcards simply reported more "findings"
(a working program reported six).

Excluding those cells — noted at the minting site, `node_apply`'s wildcard branch — the real
undecided reads are:

| program | family | answer | real undecided reads | shape |
| --- | --- | --- | --- | --- |
| `m4` | A | `none` | **1** | `f=F1 block 4 origin=n21 walkless minted=14` |
| `b7` | B | `none` | **7** | the same signature |
| `g3` | B | `none` | **7** | the same |
| `i7` | C | `none` | **5** | the same |
| `h2` | — (answers `6`) | — | **0** | — |

So **all three families share family A's observable**: every failing program's real undecided read
is a clone of the same template, minted by an earlier walk, read with no walk running — and the
working contrast performs none. The behavioural rules that separate the families (§23) stand, since
they came from behaviour rather than from this instrument; what does not stand is the claim that the
phenomenon differs between them.

What this also says about the method: a count of undecided reads is only a finding if the read was
meant to resolve, and this note had already been misled once by counting reads without that
distinction.

## 25. The phenomenon has an exact corpus fingerprint

With placeholder reads excluded (§24), one predicate over the whole corpus is exact:

> **the program's root evaluation performs a walk-less read of a node minted by an earlier walk.**

Measured over every program in the scratch corpus that answers at all:

```
programs scanned: 426
answers none:  19 — all 19 with a real undecided read, all 19 with a walk-less read of a minted node
answers other: 407 — one with any real read, none with a walk-less read of a minted node
false negatives: none          false positives (the strong form): none
```

So the observable this note established is not merely correlated with `none`: it holds on every
failing program and on no other, which gives anyone continuing a predicate to test a candidate
mechanism against instead of a prose description. Re-running the scan with the same patch on the same
corpus reproduces it exactly — twenty-two of twenty-two, none of the other four hundred and seven, no
false negative and no false positive in the strong form.

Two caveats on the numbers. The corpus grew while this note was written, and this scan sees
nineteen programs answering `none` where an earlier scan over the same directory saw sixteen — the
earlier one only counted programs that printed its own predicate line, so three failing programs
never appeared in it. And one program answers correctly while performing a real undecided read: it
does not perform the walk-less read of a minted node, which is why the weak form has that single
false positive and the strong form has none.

**A fourth field that does not discriminate, recorded so it is not retried.** Counting, at the end
of the root evaluation, how many clones of parameter cells are still empty separates nothing:

```
m4 (none) 5   v4 (none) 6   g3 (none) 4   b7 (none) 4   i7 (none) 3
v3 (4)    2   h1 (6)    2   h2 (6)    2   s1 (3)    4   n1e (3)   4
```

Working programs carry as many empty parameter clones as failing ones, so the *state* of those
clones is not the signal — which is exactly why the fingerprint is stated as a **read** (a walk-less
read of a minted node) rather than as a state: the fingerprint is the only reading in this note that
holds on every failing program and on no other.

## 26. How the failing read is reached: the evaluation chain

A field the static attempts could not reach — the chain of `evaluate_node` calls that led to the
walk-less read — separates the families once more, and this time by *depth* rather than by state:

```
m4 (family A): via = [n69]                                              — a direct, top-level read
b7 (family B): via = [n262 -> n288 -> n403 -> n418 -> n420 -> n388 -> n124]
                                                                        — nested six deep
```

The instrument is a thread-local stack pushed in `evaluate_node_body` and a guard that keeps the
last finished evaluation's snapshot, read by the print (§28's patch table gains
`eval-chain.patch`). It replaces a first attempt that printed the stack from `evaluate_node` — after
the body's guard had already popped it — and so reported an empty chain for every program; both the
working form and the failing form of the earlier fields failed for the same class of reason, which is
why this note records the reason each field was expected to work.

So the fingerprint is shared (§25) while the *path* to it differs: family A's failing read is made
directly at the root, family B's inside a six-deep descent. That is consistent with the two families
having opposite behavioural rules (§23) even though the read that fails looks the same.

Naming the chain's nodes is the next measurement: each entry printed with its function, block,
operation and value turns the path into something readable, and then the question is what the node
one step above the read is and why the descent beneath it ends on the stale clone.

**Naming them refutes the inference that reading the depth suggested.** With every entry described,
family B's chain is six nested **top-level operations**, each a clone (`origin` set), ending on the
stale clone; family A's is a single entry, which reads as "a top-level `evaluate_node` call made the
read". The natural inference — that the checker's root term *is* that node — is wrong, and one
print settles it:

```
m4: root_term = n61, first real read = n69      b7: root_term = n114, first real read = n124
v3: root_term = n80, no real read               h2: root_term = n83, no real read
```

So the failing read is a **top-level `evaluate_node` call that is not the root's** — one of the other
deep evaluations the checker performs — and family A's chain of one says exactly that. Labelling
those call sites is the next measurement, and it names who reads the stale clone at top level.

**Labelling them names the reader, and corrects the line above.** With a marker set before each of
the checker's nine deep evaluations, every failing program reports the same site:

```
m4 site=9   s2 site=9   b7 site=9   g3 site=9   i7 site=9      v3, h2: no real read
```

Site 9 is `checker.module.evaluate_node_deep(root_term, None)` — **the root term's own deep
evaluation**. So the read is inside the root's evaluation after all, and the chain of one did not say
otherwise: `evaluate_node_deep` is a different function from `evaluate_node_body`, so it pushes no
frame and the chain simply omits it. That correction is the sixth reading of this note retracted in
favour of a measurement.

What the chain does say, then, is that the descent from the root term reaches the stale clone without
passing through any *other* node's `evaluate_node_body` frame — the path used is the deep walk's own,
which is where the next instrument belongs.

**Instrumenting the deep walk's own path leaves a contradiction to resolve.** With a stack pushed in
`evaluate_node_deep_inner` (and the site marker now cleared after each call, since the first version
left it set for whatever ran later), every failing program reports `site=9` **and an empty deep
path**:

```
m4 site=9 deep=[]   s2 site=9 deep=[]   b7 site=9 deep=[]   g3 site=9 deep=[]   i7 site=9 deep=[]
```

The two readings disagree: the marker says the read happens inside the root term's deep evaluation,
but if it happened in that evaluation's outer frame the deep stack would hold at least the root term,
and it holds nothing. So the read is made while the site marker is set and no deep frame is alive —
either from code that runs after the deep function's guard has dropped but before the marker is
cleared, or from a path that reaches `evaluate_node` without the deep walk at all. Reconciling those
two is the next measurement, and the counter the walk already keeps (`deep_depth`) is the cheaper
witness to compare against the stack.

**The two readings reconcile, and they name the path.** Printing the counter and the stack beside each
other gives, for every failing program:

```
READ-NONE site=9 node=n69 (origin=n21, minted=14, walkless)  via=[n69]  deep=[n61 -> n69]  depth=2  evals=0
```

`n61` is the **root term** — one print of `root_term` reports exactly it — and the deep walk descends
from it **directly into the stale clone**: two deep frames, no `evaluate_node_body` frame at all. So
the root term's own structure references a clone minted by a check-time walk, nothing binds that clone
when the program runs, and the root's answer is therefore empty.

Two readings of this instrument were wrong before this one, both the same way: an earlier run reported
an empty deep path because the binary had not rebuilt (a filtered build log hid the failure), and the
run before that reported an empty evaluation chain because the print sat after the guard had popped it.
That is the seventh and eighth reading in this note corrected by looking again rather than by new
theory.

The root term being an operation whose operand array holds a check-time clone is the statement to
verify next: printing that array item by item names which item carries the clone and what the others
are, which is what a repair would have to change.

**That statement was written before it was verified and is wrong**, which is the ninth correction here
and the first one this note made by predicting instead of measuring. Item by item, the root operation
looks the same in a failing and in a working program:

```
m4  n61: [0] n39 (F3, holds a function)  [1] n58 (top level, expr)  [2] n59 (empty)  [3] n41 (F3, expr)
v3  n80: [0] n58 (F3, holds a function)  [1] n77 (top level, expr)  [2] n78 (empty)  [3] n60 (F3, expr)
```

Neither array holds the stale clone, so the operand array does not carry it. What *is* measured is the
deep walk's path, `n61 -> n69`, so that edge exists and is not an operand-array edge — it is the edge
the previous instrument was looking for. Enumerating the edges `evaluate_node_deep_inner` actually
follows from an apply node, in code, is what names it.

**Reading that code names the edge.** The deep walk's own body says it:

```
line 536   _ => self.evaluate_node(Dyn(node), current),        — evaluate the node first
line 538   if let Some(LowValue::Array(array)) = value …       — if its value is an array
line 545   for item in unsafe { array.items() }                — for each item
line 547       if item.shallow { continue; }                   — unless the position is lazy
line 550       module.evaluate_node_deep_inner(item.node, …);  — descend into it
```

So the edge is **"an item of the node's own evaluated value"**: the root apply is evaluated first, its
value is an array, and the stale clone is one of that array's items, which the walk then descends into
and reads. That is why the path has exactly two frames and no `evaluate_node_body` frame — the second
frame is a *value item*, not an operand and not a nested evaluation.

Family A's chain is therefore complete in every link except one, each link measured or read in code:

1. the root expression is an apply, evaluated at `checker.module.evaluate_node_deep(root_term, None)`
   (site 9, measured);
2. the apply runs the call (line 536, code);
3. its result value is an **array** whose items include a clone minted by a check-time walk
   (`n69`, `origin=n21`, `minted=14`, measured);
4. the walk descends into that item and evaluates it, and nothing bound it when the program ran
   (the walk-less read the corpus fingerprint is defined by, measured);
5. the root's answer is therefore empty (`none`).

What remains is the one "why" behind step 3: why the result array of a run contains a clone from a
check-time generation. That is the carrier this note measured on `m4` — a return node's own value slot
holding clones of another function's still-empty cells — and it is where a repair would have to act.

**The carrier's own decision does not discriminate either**, which is the tenth field recorded as
non-discriminating. Printing `node_apply`'s decision — `carried`, whether the template answered,
whether the value is a wildcard, whether it holds a foreign function, and the value — gives byte-identical
lines for a failing and for two working programs:

```
m4, v3, h2:  carried=true template_answer=false wildcard=false foreign=false value=expr   (first lines
             identical in all three)
```

So the run carries the same kind of value in every form; what differs is not *whether* the value is
carried but *what its items are* — which generation minted them and which function owns them. That
content comparison, item by item and on the program's own lines rather than the prelude's, is what the
next instrument has to make.

**Comparing the items does not discriminate either.** Restricted to the window and printed item by
item, the carried values of a failing and a working program have the same shape: the same owners
(`F3`, `F1`), the same `origin=None` and `minted=None` on their items, and the same `carried=true` on
the same kinds of node. So neither the decision nor the content separates them — the sixth reading in
this note to come back identical across forms.

**What does separate them is an event, not a state**, and this note measured that event before it knew
why it mattered. The field "does the call apply an instance the run itself minted" splits the programs
(§20): the three failing ones never do, and the working ones that carry the same cache do. Every state
comparison — cell values, clone counts, class membership, reachability, cache contents, carried items
— comes back identical, while the one reading that is a *read*, and the one field that is an *event*,
both separate. So the mechanism this note can support is:

> in the failing forms the run never applies a closure instance it minted itself, so the instance
> applied is one the check-time pass made, whose cells that pass left unbound; the root's deep walk
> then descends into the value that instance produced and reads one of those cells, which nothing
> binds when the program runs.

The residual is explicit and small: two programs (`h1`, `k4`) answer correctly while applying no
run-minted instance, and their closures read their own parameter, which the other instance supplies.

**And the event field is only a necessary condition — this reading is withdrawn as a cause.** Run over
the whole corpus it reads:

```
programs scanned: 426
answers none:  19 — all 19 do not apply a run-minted instance      (recall, no false negatives)
answers other: 407 — 236 of them do not apply one either           (236 counterexamples)
```

So failing to apply a run-minted instance is common among programs that answer correctly, and the
sentence above cannot be the cause: it fails sufficiency by two hundred and thirty-six programs. What
survives from it is the necessary half — every failing program has that property — and, unchanged, the
fingerprint, which is the only reading in this note that is exact in both directions.

**The decision the code makes per node was the next candidate, and it is refuted too.** `node_apply`
reuses a node in place when its value is proven concrete, not scoped, not the parameter, and holds no
foreign function; the candidate was that a failing program reuses the check-time node there. Printed
per in-window call:

```
m4 (none): in-place 0,  cloned 29
v3 (4):    in-place 3,  cloned 105     (the three are proven_concrete=true, foreign=false)
```

The failing program leaves **nothing** in place and clones less overall, so the candidate is wrong, and
the difference is one of scale rather than a condition — a symptom of the two executions differing, not
a pivot that isolates a cause. That is the eleventh reading refuted here.

**A causal probe fires in the failing form only, and still does not change the answer.** The
counterfactual: drop a carry whose items are *all* clones minted before the window opened, which is a
state the investigation can recognise, behind an environment variable so the default behaviour is
untouched. Measured with the probe on:

```
m4 (none): probe fires once (a value with two such items)  — suppressing the carry leaves the answer none
v3 (4):    probe never fires
```

Two findings, and they point different ways. The **firing** is the first state-level difference this
note has found between a failing and a working program: the failing run holds a carried value whose
items are all clones from before the window, and the working one holds none. The **suppression** still
answers `none`, so that carry alone is not what loses the parameter — the twelfth candidate refuted,
and the first refuted causally rather than by a reading that separates nothing. So either another path
moves the same clones, or the loss happens before the carry is consulted.

**Run over the corpus, that probe is neither necessary nor sufficient, and its suppression moves no
answer at all.**

```
programs scanned: 426
baseline none:  19 — the probe fires in 10 of them, and not in the other 9
baseline other: 407 — the probe fires in 37 of them
answers the suppression changed: none
```

So the firing is a correlate of one family's shape rather than a discriminator, and removing every such
carry changes no program's answer anywhere in the corpus. That is a corpus-wide causal refutation of
the carry as the mechanism, and it leaves the apparatus behind: an environment-gated probe that
suppresses one step and reports whether any answer moved is the cheapest way to test the next candidate
causally, in one build and one sweep.

**One more probe, and it is not a valid counterfactual — recorded so it is not mistaken for a result.**
The candidate was derived rather than guessed: the value is bound into one clone of the template cell
while the failing read uses another, so hand the same argument to every other clone of that template.
Two faults, both mine. It keys on the *cloned parameter's own* origin, which is usually absent, so it
never fires in the failing program at all (measured: zero mirrors there); and where it does fire it
writes node values directly, bypassing the class machinery, which leaves the program with no answer at
all — destructive, not an intervention. It is saved as `mirror-clones.patch` marked invalid, and no
conclusion is drawn from it.

## 27. Where the repair acts — implemented, and what is left

The cause is the checker's per-function return pass (§26): it evaluates a function's `r#return` once by
design, and for an apply-bodied function that node *is* the body's apply, so the pass runs the call with
the parameter unbound and pins the empty generation it mints. **The repair that landed is neither of the
options below**: it makes the apply run **both** of its slots (the type's as well as the value's, which had
only ever run for a callee whose value was not concrete) and makes the type application read the
instantiated return's expr **after** evaluating that clone, where before it read the expr first and
returned early on an operation's absent value. Measured: every previously failing program answers its
correct value and no other program's answer moved. The options below are kept as the record of what was
weighed.

**The acceptance test was already written**: the two exact predicates were expected to go from `22 of 22`
failing to none failing while the other `407` programs keep their answers, plus the `h1`/`k4`-style
contrast and the five predictions in §26 still holding. Measured after the repair: the values of all
twenty-two are correct, **zero** of the other four hundred and seven answers moved, and the four programs
whose type is still undecided are `b7`, `g2`, `g3`, `h5` (an undecided expr) and `w5` (an undecided
scalar).

**Capture/run-stamp follow-up.** An independent probe of `mk := f => { g := y => f 1; g 1 }`
verified the exact boundary rather than only the returned value. The enclosing capture's node
chain kept `runned=false` and was never cloned; the body's `f 1` went from template node 51 to
255/455 and then 280/480 on successive calls, each a fresh Apply evaluated with its own argument.
Calls with doubling and squaring functions answered `2` and `1`. The target shape created no
replay; return-the-capture variants did enter replay, whose instantiation still referenced the
same capture and created no clone of it. Thus the ordinary and replay paths both preserve the
capture's identity and run stamp while re-running the new `f 1` operation.

**Both ends of what was left are repaired, and the acceptance is the corpus.** `w5` failed because its
root apply has a **static** callee: element 3 carried the type and it resolved to a function, but of the
`AnyFunctionId::Static` variant, which the dynamic path's guard refuses, so the type side was skipped and
the root's type cell stayed empty. The static path now applies its type slot the same way the dynamic one
does — after the instantiated return is evaluated, take its type half, evaluate it, and unify it with the
checker-wired result cell. The dynamic path evaluates that clone only when its expr is absent, since
forcing one that already answers costs an apply the depth budget counts, and the expr's elements are
resolved only inside a deep pass and never at a position the mask marks shallow. That also repaired `b7`,
`g2`, `g3` and `h5`, whose type chains pass through static callees further down. Measured over the corpus
afterwards: **twenty-two answers moved, every one of them a program that previously answered `none`**, all
twenty-two now answering fully correct values and `Int` types, and **none of the other four hundred and
seven answers moved**.

**One budget expectation moved with the repair.** A type side instantiates once per apply, and each
instantiation stamps its clones one level deeper than the apply, so evaluating an unbounded stream now
reaches the apply-depth ceiling before the evaluate-depth ceiling. Which of the two guards trips first is a
fact about how much work an apply does, so the test that watched it now asserts that a depth guard recorded
its budget rather than which one.

**Three routes to those slots were tried and refuted**, so a later attempt need not repeat them:
evaluating the expr's own items changed no answer; evaluating the chain's operation-bearing producer
changed no answer; and unifying the slot with the chain's value-bearing element is **harmful** — it turned
five previously correct programs into `expected Type, found Int` and the four into nested `raw[?a, …]`
types. The type application's post-processing is not where the repair belongs.

**What the mint does, measured.** The clone hole is real and was measured: a node that is a clone whose own
origin carries an operation while it does not fires **forty-six times** in `g3` and **zero times** in `m4`,
matching `node_apply` publishing a clone into the remap before assigning its operation. Completing those
clones was tried and is **not** the repair either: copying the operation from the origin chain produces
nested `raw[raw[…]]` types, because an apply's value is an expr while a type slot needs one element of it.
The hole is recorded here as a fact about the engine, not as the cause of this defect — the type slot is
filled by the application above, not by completing those clones.

| option | what it changes | pros | cons |
| --- | --- | --- | --- |
| **A. Run the pass without pinning** — evaluate the return for its checks but mark what it computes unpinnable, so the generation it mints cannot be copied into the run | the pinning rule the pass relies on ("the deep pass pins what the clone copies") | keeps the pass's stated purpose, the narrowest change, and the failing programs keep their checks | touches the pinning invariant, which other notes rely on; needs a rule for *which* results are unpinnable and a reason that is not "this shape" |
| **B. Give the pass a check-only mode** — evaluate the body for its asserts and discard everything else | the pass's contract at its call site (`checker.rs:557`) | matches the comment's stated purpose exactly ("its checks fire even unapplied"); no lowlevel change | the largest change to the checker's side; a body that must be *evaluated* to fire its checks still runs the call, so the fix is about what is *kept*, not about what runs |
| **C. Reach the pinned generation from the run** — when the call binds, let the value also reach the clone the pass pinned | `node_apply`'s binding / the class machinery | no checker change; it is what already happens in the `h1`/`k4` contrast, so it generalises an existing path | the broadest effect on sharing: it makes two generations of one template see one value, which is exactly the kind of merge the earlier `function-unify` repair had to be careful about |
| **D. Give an apply-bodied function a return expr of its own** — so the pass evaluates an expr rather than the apply | `check_lam`'s wiring of `r#return` | removes the antecedent entirely; the pass then cannot run the body | moves what every call of such a function reads, and is the candidate §4 examined first; it changes shape for programs that answer correctly today |

Option **A** is the narrowest and the one whose acceptance test the investigation has already built; **D**
is the only one that removes the antecedent instead of compensating for it. If a repair is wanted, the
choice between them is a design decision, not a measurement.

Each option's site, located so the choice can be weighed by what it touches rather than by description:

| option | site |
| --- | --- |
| A | `crates/lichen-lowlevel/src/evaluation.rs:532` and `:568` (the pass writing `evaluated_deep`), with the cut at `:513`/`:570` (`assumed_concrete`) |
| B | `crates/lichen-highlevel/src/checker.rs:557`, the single `evaluate_node_deep(ret, None)` in the per-function loop |
| C | `crates/lichen-lowlevel/src/apply.rs:65` (`apply_parameter_check`, the argument's unification) and `apply.rs:116` (`wire_apply_result`) |
| D | `crates/lichen-highlevel/src/checker/lambda.rs:143`, where `r#return` is set to the body's checked term |

**If a recommendation is wanted: A, with D as the alternative worth arguing for.** A is the narrowest, it
keeps the pass doing what its comment says it is for — the body's checks still fire — and its acceptance
test is already built (the two exact predicates, the `h1`/`k4` contrast, and the five predictions). Its
cost is that it edits a rule other notes lean on, so the *reason* it writes has to be general: "a result
computed by a pass that ran with an unbound parameter is not something a later run may copy", not "this
shape fails". D is the principled alternative precisely because it removes the antecedent instead of
compensating for it, at the price of changing what every call of an apply-bodied function reads. C is the
one to be wary of: it generalises a path that already works in the `h1`/`k4` contrast, but by making two
generations of one template share a value — the sharing the earlier `function-unify` repair had to reason
about. B is the largest checker-side change and buys the least, since a body whose checks must fire still
has to be evaluated.

**The decision record has no item on this pass**, checked before proposing any of the four, because a
`wontfix:D<n>` line in [code-audit](code-audit.md) is a contract and one of those would settle the
question. Its entries near the deep pass are about the two depth guards' opposite policies (`deep_depth`
scoped, `apply_depth` sticky) and about an inflated nesting counter surviving until
`reset_apply_budget` — neither is about which generation a value lands in, so nothing there forbids or
decides a repair of this.

## 28. How to reproduce any line above

Behaviour, which is the measurement for what a program answers:

```
cd C:\resource\lichen-vm\.worktrees\<this worktree>
cargo build -p lichen-compiler -j 2          # env: CARGO_INCREMENTAL=0, CARGO_PROFILE_*_DEBUG=0
cd C:\resource\lichen-vm\.scratch-core
..\.worktrees\named-capture-none\target\debug\lichen-compiler.exe m4.lichen
```

The binary path is one level up from the scratch directory, which is itself inside the repository root —
written with three `..` it fails to resolve, which is worth knowing before concluding anything about a
program that "does not run".

**The premise still holds on `dev`**, checked with the parent checkout's own binary against the whole
failing set: all twenty-two programs answer `none` there too, and the parent is on `dev` at the commit this
branch forked from. So the defect is live on `dev`, this branch neither masks nor changes it — it touches
no behaviour at all — and every repair option in §27 acts on `dev` as it stands. One caveat on that check:
the binary used was built earlier the same day rather than rebuilt for the check, which was deliberate, so
that a check on shared history does not write into the parent checkout's target. The check is otherwise
sound because the two checkouts do not share a target: `CARGO_TARGET_DIR` is unset here, and the parent's
and the worktree's `lichen-compiler.exe` are different files built at different times, so the binary that
answered was built from `dev`'s sources and not from this branch's.

Every internal reading in this note comes from instrumentation that is deliberately **not**
committed; it is kept as patches outside the branch, applied from the worktree with
`git apply --include='crates/*' C:\resource\lichen-vm\.scratch-core\<patch>` and reverted with
`git checkout -- crates`. The directory holds the twenty-seven this investigation used, grouped by
what they print; four others there (`clone-test`, `fn-unify`, `loopconv-docs`,
`user-loop-conversion-edit`) belong to other work and **must not** be applied for this one.

| patches | what they print |
| --- | --- |
| `root-window` | the window around the build-time root evaluation, plus the apply-wiring arm |
| `walk-id`, `param-walk`, `function-shape`, `instantiate-path` | walk identities, the parameter walk, a function's parameter/return shape, instantiation paths |
| `mint-walk`, `named-reference`, `referrer-trace`, `stale-ref` | which walk minted each clone, the nodes naming an undecided read, operand referrers, every live reference to an earlier-minted clone |
| `unify-trace`, `descent-trace`, `fn-unify`, `return-shape`, `return-prediction` | unifications, descents, function unification, the return's shape per instantiation, the prediction that refuted it |
| `operand-alias-read` (a subagent's) | `RETURN-CACHE`, `MERGE`/`MERGED` with site markers, `INDEX`, `APPLY`, `INSTANTIATE` |
| `predicate-half`, `reader-half`, `applied-mint`, `wildcard-filter` | the corpus predicates: the carrier, its reader half, the applied-instance event, and the exclusion of wildcard placeholder reads |
| `eval-chain`, `named-chain`, `eval-site`, `deep-path`, `root-array`, `ungated-decisions` | the evaluation chain, its named nodes, which top-level deep evaluation a read belongs to, the deep walk's own path, a node's operand array item by item, the decision prints without the window gate |
| `carried-decision`, `carried-items`, `in-place-decision`, `param-clone-values`, `instance-cells` | `node_apply`'s carried decision and the items it carries, its in-place-versus-clone decision, parameter clones with their values, every instance's cells side by side |
| `carry-counterfactual`, `mirror-clones`, `feed-check-time`, `find-value`, `find-value-any` | the causal probes: the carry suppression, and two that are recorded as invalid — the direct mirror write and the unify-with-siblings feed that fired nowhere in the failing program and left five others without an answer — with the value search and its parameterised form |
| `read-siblings`, `read-siblings-guarded`, `mint-site` | the read print with same-origin siblings, the same with the origin guard that makes the predicate exact, and the site number beside each mint |

The scans are `predicate-scan`, `predicate-scan2`, `fingerprint-scan`, `event-field-scan`,
`counterfactual-scan`, `condition-scan`, `sibling-scan` and `mint-site-scan` (with their CSVs), each a
corpus sweep over `*.lichen` reading one predicate's columns. **`condition-scan`'s verdict is withdrawn**:
it searched for the final call's literal argument, while the value that matters for a closure's parameter
is the local's, so the three "exceptions" it reported are a mis-measurement rather than evidence — the CSV
is kept because the scan and its patch are what the corrected reading was derived from. Together with the
patches they are
fifty-nine files of about a megabyte outside version control — the forty-one patches, the seventeen scans
and CSVs, and a copy of this note named for the commit it matches — so the directory is the one thing this
note depends on that a cleanup could take: the inventory's `name size` manifest hashes to
`e85ce9c087a48a9db435a5628699b781b30602bb3b0a6c8578136899e7940696`, which makes a loss detectable even
though it cannot be undone from here. Refreshing that snapshot changes the inventory, so re-hash it
whenever it is refreshed; the hash describes the directory as it stood when the note's latest commit was
written.

**That directory is shared and growing**, and its file count is not its program count: it held 429 files
when the last scan ran and 817 a few rounds later, of which **429 answer** — the rest are fragments other
work is writing, with no answer to judge. Every count above is therefore a snapshot, dated by the CSV that
recorded it: each scan's CSV lists the exact programs it judged, and a reader re-running a scan today
should compare against the CSV rather than against the totals quoted here. Re-running the fingerprint
against the grown directory was done as this was written and it still holds exactly — twenty-two of
twenty-two and none of the other four hundred and seven — with the new files contributing no answers yet,
so neither a confirmation nor a counterexample.

**The branch carried no behaviour change until the repair landed, and now it does.** The read-only part
stands: the probe test and `Probe`'s readings live in `lichen-testkit`, which `lichen-highlevel` lists
under `[dev-dependencies]` — its own manifest comment says "so no library half of the workspace links
it" — so they cannot reach a production path, and `class_members_with_values` sits in the library but its
only caller anywhere is that probe surface (`probe.rs:194`). The repair is two lowlevel edits: every apply
applies **both** slots, the type side for a concrete callee included, and the type application reads the
instantiated return's expr **after** evaluating that clone. Measured over the corpus: all twenty-two
failing programs now answer their correct value — eighteen of them fully, four (`b7`, `g2`, `g3`, `h5`)
with an undecided expr type and `w5` with an undecided scalar type — and **no other program's answer
moved**: twenty-two of four hundred and twenty-nine answered differently, every one of them a program
that previously answered `none`.

**Merging this branch.** The parent checkout is on `dev` at the commit this branch forked from, with no
modified tracked files and one untracked file of another session's (`probe.lichen`), which collides with
none of the twelve paths here — `AGENTS.md`, `docs/README.md`, this note and `test-harness.md`, the probe
test, `probe.rs`, `equality.rs`, and the five code files the repair touched (`apply.rs`, `evaluation.rs`,
`function.rs`, `static_module/apply.rs`, `checker/lambda.rs`) — so stage those paths explicitly and never
`git add .`, or that file rides into the merge commit. The repair means the branch now **changes
behaviour**, and the acceptance for it is the corpus comparison in §27: twenty-two answers moved, every one
of them a program that previously answered `none`, and none of the other four hundred and seven. One path
is a judgement call rather than a fact: the probe test (`zz_nested_apply_probe.rs`) prints and asserts
nothing, and `dev`'s test tree has no such file — the no-assert files there are harness helpers and a
readme test — so merging it establishes the practice as well as the instance. The TestKit section this
branch adds to `AGENTS.md` describes exactly that practice, so the two belong together: keep both, or drop
the file and that bullet with it.

**Its footprint in the repository is zero, and that is not the same as unreachable.** Running `dev`'s
binary over every `.lichen` and `.lk` file the repository ships — outside this investigation's scratch
directory and the worktrees — eighty-two files answer something and **none answers `none`**. So no example,
fixture or test program triggers it: the shape it needs (a top-level function whose value is exactly the
apply, over a closure that reads only its capture) is one the shipped programs do not take, and the
twenty-two failing programs are all constructed for the purpose. What was measured is that the shipped
programs do not take that shape, not that the language cannot express it — the twenty-two are ordinary
`.lichen` programs a user could write — so the defect is user-reachable and merely unexercised, which is
worth weighing when deciding whether to repair it at all, and it explains why it could sit unnoticed.

**It is not a regression.** Searching the history for the call site's own text dates the per-function
return pass to `bf9c2a67` (2026-08-22), a day after the checker file itself was created — so the design has
been there from the beginning rather than arriving with a recent change. Two earlier searches on the
comment's wording and on the `r#return` assignment landed on August rewordings and file moves instead,
which is why the call site's text is what to search for. For a repair that means changing a design the
checker has always had, and it is the second reason the defect went unnoticed: not only does no shipped
program take the shape, the pass that makes the shape fail has been there all along.

**Every number above was re-derived from the scan CSVs as a final pass, and they agree**:
`fingerprint-scan` (429 rows, twenty-two failing, twenty-two with the strong read, none of the other four
hundred and seven, one correct program with a weak read), `predicate-scan2` (424 rows from before the
corpus grew, sixteen failing, all sixteen carrying, twenty-seven correct ones carrying too),
`event-field-scan` (426 rows, nineteen of nineteen failing without a run-minted instance, two hundred and
thirty-six correct ones too) and `counterfactual-scan` (the probe fires in ten of the nineteen and
thirty-seven of the others, and moves no answer).

Every trace is gated on `LICHEN_DBG=1`. Ids in a trace **shift between runs**: compare them
within one run, or by `origin` and value, never across runs — one reading in §12 was wrong
for exactly that reason and is recorded as withdrawn.

**The load-bearing programs, identified so a change is detectable.** The directory is shared, so
another session may edit these; the hashes are the files as measured, and a different hash means
re-run before trusting the answers quoted here (`sha256`, first twelve characters):

```
m4 8b5546f1c171 answers none     v3 04d2fe135e49 answers 4      h1 32bb8e2063b0 answers 6
k4 379355e32d22 answers 6        i7 b518890fe2ce answers none   g3 99a419b05c3d answers none
```
