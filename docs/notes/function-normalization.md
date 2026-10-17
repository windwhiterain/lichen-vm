# Normalization is a function's birth, not an application

> Status: **current.** The normative definition of "normalize" in this tree: what
> it is, when it runs, and what an apply does instead. It exists because the
> class-read investigation called an apply-time read "an apply clone's run-time
> normalize"; the maintainer's correction is that a normalize belongs to a **new
> function's first appearance**, never to an apply.
> Points at: `crates/lichen-highlevel/src/checker.rs` (the definition pass and its
> lambda-value loop), `crates/lichen-lowlevel/src/static_module/apply.rs`
> (`static_function_apply`, the runtime unify), and
> [late-writes-map](late-writes-map.md) §0 and §2.1, which state the maintainer's
> terms and name the site.

## 1. The concept

**Normalization is the pass that settles what a newly appeared function's own
template owns** — its parameter and return type cells, and the classes those cells
carry — so that later readers of the template see decided cells. It runs **once**,
when the function first appears.

The maintainer's terms are the authority here, and
[late-writes-map](late-writes-map.md) §0 records them: *"`unify` is separated from
`evaluate`: the checker does the initial unify (`expr: expr`), normalization
happens at the moment a function is born, apply does runtime unify, and beyond
those three sites no unify is produced dynamically."*

That note's §2.1 names the site: the **function-birth normalization**, the
lambda-value pass in `checker.rs` — *"Deep-evaluate every lambda's value node
before the definition pass, so a recursive reference stays in place"*.

## 2. What an apply is not

**An apply is not a normalization and does not run one.** In the maintainer's
words:

> runtime normalization 不是 apply time normalization。normalization 指的是一个新的
> 函数首次出现；apply 时的 clone 并不会让 apply 的那个函数产生一个新的 clone
> 函数，只是把它的 body clone 出来了。

Two consequences follow:

- **The applied function is not re-created.** An apply clones the callee's
  **body** into the caller's graph; it does not mint a new clone *of the callee*,
  so there is no new function whose birth could normalize anything.
- **The clones belong to the caller.** [apply-clone-ownership](apply-clone-ownership.md)
  §1: a call's clones are stamped with the enclosing template's owner, and their
  cells are the enclosing call's. A type-term clone's cells are therefore decided
  by whatever the *caller* does next, not by a normalize of the callee.

What an apply does is the third sanctioned unify producer, the **runtime unify**:
the parameter check and the result/codomain wiring (`static_module/apply.rs`'s
`static_function_apply`; `apply.rs:78` and `apply.rs:139`).

## 3. The contract

1. **A function's types are determined at its normalize.** The maintainer's
   ruling: *"normalize 的时候类型应当确定。"* After a function is born, every type
   cell its template carries that the program made **determinable** holds a
   value. A determinable cell still open after the normalize is a defect in the
   normalize, not in a later read: ruling 1 of
   [compile-order-class](compile-order-class.md) §8 exists so that later phases
   *read* decided facts instead of creating them. What may legitimately remain
   open is a cell the program itself left open — a `_` hole or a genuinely
   generic position — and that is reported where a reader needs it (ruling 3).
   **What an un-annotated recursive fold costs, measured**: a fold whose state is
   the kernel's **buffer** is typed without an annotation (the seed decides it),
   and one whose state is an **array** keeps its value with an open type (`?a`);
   but one whose state is a **struct** (`XOut = struct{.y (compute.Buf Float)}`)
   loses the value outright — `raw none` with type `array{Float, 6}`, and no
   diagnostic. Annotating the recursive function (`… : tuple{Int, XOut} -> XOut`)
   restores both the value and the type (`[2.1875, …]: array{Float, 6}`, measured
   on the same three-step ring, for `@loop` and plain recursion alike).
   **The lost value is a slot, and the recursion's *presence* is the trigger**
   (measured on the same ring, distance 0 so the body returns its own argument):
   `@loop m = (s => s(1))` ; `m (0, seed)` collects the seed's numbers, and so do
   a plain lambda and `s(0)`; but the recursive body
   `s => if s(0) == 0 then s(1) else march (…)` returns
   `raw[raw[raw Buffer, raw Float]]` — a **one-element array**, the return pair's
   value slot, not the struct — so `settled(1)` reports `index 1 out of bounds
   (array length 1)` and `settled.y` is `none`. The annotated form returns the
   struct itself. So the apply of a function whose type is cyclic (the
   un-annotated recursive case) wires the template's return **slot** into the
   caller's cell instead of the body's value; the fix belongs where the static
   apply wires its result (`function.rs`, `wire_apply_result`) and needs the slot
   unwrapped, not a change to the fold.
2. **Once, at birth.** A function is normalized exactly once, at its first
   appearance. Later applications do not normalize it again, and its template's
   facts stay as the birth left them.
3. **A checking-time function normalizes at checking.** A function that exists in
   the source (a lambda, a definition) is born and normalized before the
   definition pass — site 2 of [late-writes-map](late-writes-map.md) §2.1.
4. **A function that first appears at run time normalizes at run time.** A
   closure materialized while the program runs is a new function; its birth is
   when its own template settles. Ruling 2 of
   [compile-order-class](compile-order-class.md) §8 is about this case — a
   function's own birth — and it says nothing about the applied callee.
5. **A read may rely on a cell only after the pass that decides it.** A read that
   runs before the function's normalize — or before the apply unify that decides a
   caller-owned clone's cell — sees the cell **empty**, not a default. An
   undecided class is reported, never guessed (ruling 3 of
   [compile-order-class](compile-order-class.md) §8); "the read is early" is an
   ordering fact to fix where the decision belongs, not a reason to seed one.
6. **No fourth unify site.** Beyond the checker's initial unify, a function's
   birth normalize, and an apply's runtime unify, nothing creates a unification
   dynamically. "Normalize the type term when it is cloned" would be such a fourth
   site: it needs a ruling, not an implementation detail.

## 4. What this corrected

The class-read investigation called the decide "an apply clone's run-time
normalize". That is the conflation this note rules out: the kernel reached through
an apply is not a new function, its cells are the caller's clones, and the decide
is the apply's **runtime unify** — or a normalize of the *caller's* own function,
which owns those cells. [compile-order-class](compile-order-class.md) §3 and §8
carry the corrected wording next to the measured facts.

## 5. Other senses of the word, so a reader does not merge them

- **Specialize-then-normalize** ([specialize-normalize](specialize-normalize.md))
  means turning a named access into a subscript once the container's type is
  known: an expression rewrite, not a function pass.
- **`lichen-fusion`'s `normalize`** (`crates/lichen-fusion/src/normalize.rs`) is an
  expression canonicalisation for the algebra path
  ([graph-fusion](graph-fusion.md), [llm-operators](llm-operators.md)): a rewrite
  of a program's algebra, not a type pass.
