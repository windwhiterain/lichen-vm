# HM-loc: Explaining Type Errors by Showing Where Types Flow

*Beginner-friendly guide to the "flow-based" type error messages from the paper
**Getting into the Flow** (Bhanuka, Parreaux, Binder & Brachthäuser, OOPSLA 2023).
No prior knowledge of type systems is assumed beyond "expressions have types".*

**TL;DR.** When type checking fails, ordinary compilers point at a single
location — *"here is where the solver gave up."* That location is often not
where you made the mistake. HM-loc does something different: it shows the whole
*journey* of each conflicting type — where it was born, how it flowed through
the program, and where two incompatible types finally collided. The error
message becomes a short story instead of a single finger-point.

---

## 1. The problem: type errors point at the wrong place

Suppose you write this OCaml program (the paper's opening example). Two library
functions exist:

```ocaml
val show_major    : string -> string
val parse_version : string -> string
```

Your program:

```ocaml
let appInfo = ("My Application", 1.5)
let process (name, vers) = name ^ show_major (parse_version vers)
let main () = process appInfo
```

What you did: `appInfo` is a pair whose second component is `1.5` — a **float**.
But `process` expects that second component (`vers`) to be a **string**, because
it feeds it to `parse_version`, which only accepts strings.

What OCaml prints:

```
Error: This expression has type string * float
       but an expression was expected of type string * string
       Type float is not compatible with type string
```

...pointing at the line `let main () = process appInfo`.

**The confusion:** the error points at the *call site* (`process appInfo`), but
the actual mistake is the literal `1.5` — three lines above, inside a *different
function*. The compiler knew two types collided, but it only shows the one spot
where its internal solving algorithm happened to stop. The journey that carried
the float to the collision is invisible.

Why does this happen? Type inference (see §2) solves *constraints* one after
another, and when a constraint fails it reports *that constraint* — but which
constraint fails first is an accident of ordering, not of your intent. This
"wrong location" problem has been studied since the 1980s (Wand, 1986); it is
genuinely hard, not a compiler bug.

What HM-loc prints for the same program:

```
[ERROR] Type 'float' does not match 'string'

(float) comes from
  - l.4  let appInfo = ("My Application", 1.5)
                                  ^^^
  - l.5  let process (name, vers) =
              ^^^^
  - l.9  name ^ show_major (parse_version vers)
                              ^^^^^

(string) comes from
  - l.2  val parse_version : string -> string
              ^^^^^
```

Now you can *read* the mistake: the float is born at `1.5`, flows into
`process`'s parameter `vers`, flows into the argument position of
`parse_version`... and collides with `string`, which is what `parse_version`'s
signature expects. Three locations tell the story instead of one.

**The one-sentence summary of the whole paper:**

> Type errors happen when incompatible types *flow into each other* through the
> program; good error messages should therefore show the flow — every place the
> types passed through — not just the one spot where the solver gave up.

---

## 2. Background, in plain words

If you already know this, skip to §3.

- **Types.** Every expression has a type: `5` has type `int`, `"hi"` has type
  `string`, a function from ints to ints has type `int -> int`. Types are built
  with *type constructors*: `->` builds function types, `*` builds pair types,
  `list` builds list types.
- **Type inference.** You don't have to write the type of everything; the
  compiler *infers* it. For anything unknown it invents a **type variable** —
  think of a blank tile `?a` — then collects **constraints**: statements like
  "the type of this expression must equal the type of that expression".
- **Unification.** The step that solves constraints: combine two types by
  filling in blanks. `?a` unifies with `int` by setting `?a := int`. Two *known*
  types unify only if they have the same shape (`int` with `int`); if they
  differ (`int` with `bool`) → **type error**.
- **Hindley-Milner (HM).** The classic inference algorithm/type system used by
  ML, OCaml and Haskell. "HM-loc" literally means "HM + locations": the same
  type system, but with source locations carried along so errors can be
  explained.

---

## 3. The core idea: types flow like data

Analogy: every value in the program carries a colored sticker — its type.

- A **literal** is born with its sticker: `1.5` is born `float`.
- When a value **moves**, its sticker moves with it: pass `appInfo` into
  `process`, and both components' stickers travel along.
- When two stickers land on the same spot and don't match → **type error**.

So a type error is never "at one place". It is the meeting point of two
*journeys*. A good diagnostic shows both journeys: "this float came from here,
went through there, and here it met a string."

In the example above: the `float` sticker rides inside `appInfo`'s pair, hops
into `vers`, and is passed to `parse_version` — where the `string` sticker from
the library signature is waiting. That *is* the error message HM-loc prints.

---

## 4. Making it precise: directional constraints

**Standard inference uses equalities.** For `process appInfo` the checker
generates something like:

```
type(appInfo)  =  type(process's parameter)
```

Equalities are *symmetric*: `a = b` and `b = a` say the same thing. Symmetry is
convenient for solving, but it **throws away direction** — which side is the
*source* (where a value was created) and which side is the *sink* (where a value
is expected). Once direction is gone, the journey is gone, and all you can
report is "these two things are unequal, somewhere".

**HM-loc writes arrows instead.** Each constraint is read as a flow:

```
type(x)  flows into  type(y)
```

(Technically written τ₁ <: τ₂, the "subtype" symbol — but read it as "τ₁ flows
into τ₂". The language does *not* gain subtyping; the arrows exist only to keep
direction for explanations.)

Why does direction matter? Because the explanation is a *path*: "float starts
here → flows through `vers` → collides with string there". Without arrows you
have a pile of disconnected facts; with arrows you have a route you can walk and
print.

This is also why the paper builds its engine on *subtyping machinery* rather
than plain unification: subtype constraints are naturally directional (subtyping
means "a value of this type can be used where that type is expected" —
inherently source → sink). But again: the language itself stays plain HM; the
direction machinery exists only for error messages.

---

## 5. Provenance: every type carries a diary

The second ingredient is **provenance** — a record of where a type has been.

Every type, while being inferred, drags along a diary of the source locations it
passed through (plus which position inside a type constructor it sat in, like
"the left half of a pair"). The diary has only two operations:

- when a constraint combines two types, their diaries are **concatenated**;
- when two types collide, the solver **dumps both diaries** — and that dump
  *is* the error message.

In the example, the float's diary reads `[l.4: the literal 1.5] → [l.5: the
parameter vers] → [l.9: the argument position of parse_version]`. The string's
diary reads `[l.2: parse_version's signature]`. Print them under a header and
you get the "comes from" message in §1.

Mechanically, provenance is just a list of locations with a little structure;
concatenation is the only operation. It is cheap and completely mechanical — the
"intelligence" is in deciding *what to print*, not in the tracking.

---

## 6. Not all errors are equal: the level classification

The paper's second big idea: **classify errors by how many times the flow
changes direction** along the chain connecting the two conflicting types. Each
change of direction is a place the explanation must account for — and different
kinds of errors deserve different message templates. (Most compilers use exactly
one template for everything, which is part of why messages feel unhelpful.)

In the diagrams below, arrows point in the flow direction: `X ---> Y` means "X
flows into Y". The **level** = how many times the arrows flip direction along
the chain.

### Level 0 — a straight flow (the common case)

One type flows directly into a position that expects a different type. Example:

```ocaml
let x = 2
let y = if x then true else false
```

`x` is an int (born at the literal `2`), but it is used as the *condition* of an
`if`, which must be a bool. Message:

```
[ERROR] Type 'int' does not match 'bool'

(int) comes from
  - l.1  let x = 2
              ^

(bool) comes from
  - l.2  let y = if x then true else false
                    ^
```

Message shape: *"X comes from …"* / *"Y comes from …"*. The appInfo example at
the top is also level 0.

### Level 1 — a confluence: two flows meet in one variable

Two different types flow into the *same* unknown. Example:

```ocaml
let x = 2
let y = if true then x else "x"
```

The result of the `if` receives `int` (through `x`) and `string` (the literal
`"x"`). Message:

```
[ERROR] Type 'int' does not match 'string'

(int) ---> (?a) <--- (string)

(int) comes from
  - l.1  let x = 2
              ^
  - l.2  let y = if true then x else "x"
                          ^

(?a) is assumed here
  - l.2  let y = if true then x else "x"
          ^^^^^^^^^^^^^^^^^^^^^^^^

(string) comes from
  - l.2  let y = if true then x else "x"
                                       ^^^
```

Two things to notice:

1. **The middle variable gets a name.** Unknowns are numbered in the order they
   appear: `?a`, `?b`, `?c`… The diagram `(int) ---> (?a) <--- (string)` reads
   "both an int and a string flow into the same variable `?a`" — and the message
   then shows *where `?a` was created* ("assumed here").
2. **Confluence means "the variable is over-constrained"**, so fixing the error
   involves changing one of the flows.

There is a second shape of level 1: one variable used in two conflicting ways
(the paper calls this *divergent* flows):

```ocaml
let f x = (not x, x + 1)   (* x must be a bool (for not) and an int (for +) *)
```

```
[ERROR] Type 'int' does not match 'bool'

(int) <--- (?a) ---> (bool)
```

Read as "`?a` (which is `x`) is forced to be both an int and a bool".

### Level 2 — a chain through two variables (rare)

```ocaml
let g x = (not x, if true then x else 5)
```

Here the conflict is int vs bool, but it travels through *two* unknowns:

```
(bool) <--- (?a) ---> (?b) <--- (int)
```

The paper's machinery still produces a complete explanation for these — the
level just tells you the message needs more "stops". The authors note that
errors above level 2 are rare, and that even for level 2 it is no longer obvious
what the best message looks like — but the machinery still lists all the
essential places.

**Rule of thumb:** level 0 is what beginners hit, and a simple "comes from"
template already fixes the worst of the misattribution problem. Levels 1 and up
are where flow messages really earn their keep.

---

## 7. How it is actually built (one page of machinery)

1. **Generate directional constraints.** Instead of equalities τ₁ = τ₂, the
   checker emits flows τ₁ <: τ₂ (source → sink) at each construct: a literal
   flows into its binding; an argument flows into the parameter position; each
   branch of an `if` flows into the result; and so on. Each constraint carries
   the source location of the construct that generated it.
2. **Solve with bounds instead of substitutions.** Ordinary unification
   substitutes `?a := int`. HM-loc instead gives each unknown `?a` two lists: a
   **lower bound** (types that flow *into* it) and an **upper bound** (types it
   must flow *into*). Written compactly: `τ̄ <: α <: τ̄` — "everything in the
   first list flows into α, and α flows into everything in the second list". An
   error appears when the bounds are incompatible.
3. **Thread the diaries.** Every type in every bound carries its provenance;
   combining types concatenates diaries. Pure bookkeeping.
4. **Report by walking the chain.** When bounds clash, walk the flow chain
   between the two conflicting types, count direction changes → level, pick the
   message template for that level, and print the "comes from" blocks from the
   diaries.

**Why not plain unification?** Because unification applies symmetry at every
step and the direction becomes unrecoverable afterwards. The paper's honest
claim: *it is easier to create helpful error messages from a
subtype-inference-based system than from a unification-based one*. (They
implement it as an extension of Simple-sub, a small algebraic-subtyping engine,
and add one extra phase so that exactly the same programs are accepted as plain
HM — subtyping is a means to better messages, not a change to the language.)

---

## 8. Does it work?

The paper includes a user study comparing HM-loc's messages with OCaml's and
with Helium's (a student-oriented Haskell compiler built on the earlier
"Generalizing Hindley-Milner" line of work):

- **Quantitative results were mixed** — no statistically significant
  improvement in how quickly people localized or understood errors.
- **Qualitative results were positive** — users clearly valued flow-based
  messages, especially for the more complex (level ≥ 1) errors.

The takeaway is not "this is solved" but "flow is the right *kind* of
explanation". Even a minimal version — "X comes from … / Y comes from …" —
already fixes the worst misattribution, and the level classification tells you
where better templates are worth the effort.

---

## 9. Glossary

| Term | Meaning |
|---|---|
| **Type variable** | An unknown type (`?a`) invented during inference, filled in later by unification. |
| **Constraint** | A statement generated while checking, e.g. "the type of e₁ must equal the type of e₂". |
| **Unification** | Solving constraints by filling in unknowns; fails when two known types differ. |
| **Type constructor** | A way to build types from types: `->` (functions), `*` (pairs), `list`, … |
| **Subtype constraint** | τ₁ <: τ₂, read "τ₁ is usable where τ₂ is expected" — a *directional* relation. |
| **Algebraic subtyping / MLsub / Simple-sub** | A family of inference engines that solve subtype constraints efficiently; the engine HM-loc is built on. |
| **Provenance** | The diary of locations a type passed through; used to explain errors. |
| **Confluence** | Two flows (types) meeting at the same variable — the level-1 situation. |
| **Level** | How many times the flow direction flips along the chain between two conflicting types. |
| **Algorithm W / M** | The classic HM inference algorithms; they solve constraints in a fixed order, which is why their error location is often "wrong". |

---

## Appendix: why this matters for our lichen-vm

In our VM, the two ingredients of HM-loc are already (almost) free:

- **Types are values.** There is no separate "type" concept in lowlevel — a
  node whose representative value is `USize(5)` *is* the number type; an
  unbound class (`value: None` or `Value::Parameterized`) is a type variable; a
  `Value::Function`'s parameter/return nodes are the function type. So the
  "types" in every message are just values.
- **The DSU member list is the diary.** Our `disjoint::union` appends the
  joining class's member list onto the surviving root's list, so an equality
  class remembers, in order, every node ever unified into it. Walking the
  members of a failing class *is* the "comes from" list — provenance without
  storing a diary.
- **Direction needs a small side table.** The only thing unification erases is
  which side was source and which was sink; our checker records the role
  (`ApplicationArg`, `OperatorOperand`, `Branch`, `Annotation`) per constraint
  site. That single table gives us expected/actual attribution (annotation
  members = expected, uses = actual) and the level classification.
- **`f(x) = [x, f x]`** — the VM runs this lazily forever, but a checker with
  an occurs check rejects it as an infinite type (`?a = Array(?a)`). The message
  names where `?a` was introduced (the parameter) and where the recursive array
  was built. Whether that boundary is intended is a design decision to make
  deliberately.

---

*Primary reference:* Ishan Bhanuka, Lionel Parreaux, David Binder, Jonathan
Immanuel Brachthäuser, **"Getting into the Flow: Towards Better Type Error
Messages for Constraint-Based Type Inference"**, OOPSLA 2023 — arXiv:2402.12637;
implementation (hmloc) at github.com/hkust-taco/hmloc.

*If you want to go deeper:* Heeren, Hage & Swierstra, "Generalizing
Hindley-Milner Type Inference Algorithms" (constraints-first solving, multiple
messages); Wand, "Finding the Source of Type Errors" (POPL 1986, the original
misattribution analysis); Chitil, "Compositional Explanation of Types" (ICFP
2001, error slicing); Tikhon Jelvis, "Debugging Haskell Type Errors" (jelv.is —
the practical mindset).
