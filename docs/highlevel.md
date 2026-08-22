# Highlevel spec (draft)

*Design contract for the highlevel layer: a Hindley-Milner type system with
first-class types and `Type : Type`, encoded into the lowlevel VM, with rich
diagnostics. Brainstormed 2026-08-21; the three load-bearing decisions (§3) were
settled by the user. Status: spec only — no implementation started.*

**v1 posture.** Be permissive, not conservative. Do not build checker-side
guardrails to reject programs; if anything genuinely cannot terminate, the
lowlevel's existing recursion-depth guards panic ("recursion depth exceeded
…"). The checker inherits that contract: every walk it does over VM graphs is
cycle-guarded and depth-limited, or it is finite by construction (AST walks).

---

## 1. Goals

1. **HM type system.** Inference with let-polymorphism: `let`-bound variables
   generalize, λ-bound variables stay monomorphic, every use of a polymorphic
   variable instantiates.
2. **First-class types.** Types are ordinary values: they can be passed around,
   bound, and computed. There is *no type universe* — one node universe.
3. **`Type : Type`.** The type of `Type` is `Type`. A single universe; kinding
   is just an ordinary type check (see §4), so no separate kind system exists.
4. **Rich diagnostics.** Errors are grounded in a span, a unification class, and
   a role; messages explain *how* two types collided, not just *where* the
   solver gave up (HM-loc style, see `docs/hm-loc.md`).

## 2. The uniform model

- **One expression class.** The highlevel `Expr` IR has no term/type split — a
  type expression is just an `Expr` used in type position. `Type` is a constant
  expression. `e : T` is an annotation.
- **Compilation.** Each source expression compiles to a lowlevel node. The
  checker keeps two side tables: `ty: node → type node` and `span: node → Span`
  (checker-owned; the VM stays generic).
- **Types are the values of type-position expressions.** Checking an annotation
  `x : T` is: `check(T)` (which demands `unify(ty(T), Type)`), then
  **force-evaluate** `T`, then `unify(ty(x), value(T))`. Consequences:
  - `let T = int in (x : T)` works — `T` evaluates to the `int` node.
  - `x : y` where `y : int` fails with a *uniform* mismatch ("`y` has type
    `int`, expected `Type`") — there is no separate "kind error" category.
  - First-class types fall out: any expression of type `Type` can be passed
    around and later used in type position.
- **Axiom.** `ty(Type) = Type`. Nothing else needs special-casing: the universe
  is a single node, and impredicativity is fine for a checker (we prove no
  logical consistency, so Girard's paradox etc. are non-issues; non-terminating
  type evaluation is caught by the depth guards, not by a theorem).

## 3. Settled decisions (v1)

### D1 — No occurs check: equi-recursive unification

Unify binds a placeholder `?a := T` even when `?a` occurs inside `T`; cyclic
type classes are allowed. The lowlevel's existing cycle guards (the unify Vec
path, `visiting` flags) and depth limits are the safety net — a walk that does
not terminate panics.

Consequences to keep in mind:

- **Cyclic type rendering.** The type printer must detect re-entry (via the
  visiting machinery) and render e.g. `?a = array of ?a` instead of looping.
- **Cycle-safe copy.** Under D2, instantiation copies the type template; a
  cyclic template (which D1 makes reachable) requires a memoized/cycle-safe
  copy. `copy_nodes` must not loop on cyclic graphs.
- **No "infinite type" error.** Programs that standard HM rejects with
  "infinite type" (e.g. `f(x) = [x, f(x)]`) now *typecheck* with a cyclic type.
  The runtime already handles these values fine (lazy graph, cycles).
- **Soundness note.** Cyclic types break HM's consistency guarantees, but this
  is a checker for a `Type : Type` language — consistency is not a goal. The
  depth guards bound the damage.

### D2 — Eager-copy instantiation

Generalization stores a *template* (the checked type graph plus its free
placeholder list). At each use of a generalized variable, the checker **copies
the template nodes into the current block** — the copied unbound nodes *are*
the fresh instance placeholders, no separate allocation — then unifies the copy
with the use-site expected type.

This reuses the lowlevel's existing clone discipline (`function_apply` clones
parameters; fresh nodes get fresh equality classes; class topology is
re-established among clones). The copies are checker-transient bookkeeping and
can be garbage-collected after the pass, so the cost lands at typecheck time,
never at runtime. Unify stays purely structural — no forcing inside unify
(forcing happens before unify; see §6).

### D3 — Dependency allowed

A type expression may mention runtime values (`Array(n, int)` where `n` is a
term variable holding `3` is legal). The checker force-evaluates type
expressions before unifying, so typechecking can evaluate term code. The
lowlevel depth guards are the backstop (v1 posture).

What this costs or gives:

- **Computed equality works via forcing.** `Array(1 + 2, int)` and
  `Array(3, int)` both evaluate to the same node — structurally equal.
- **Open parameters stay symbolic.** Checking a function body with unbound
  parameters, `Array(n, int)` is a partially applied type constructor holding
  an unbound node; structural unify handles it (the array elementwise rule
  makes `Array(n, int)` unify with `Array(m, int)` by binding `n ~ m`).
- **Result types can depend on argument values.** This is the deepest
  consequence. When a function's result type mentions its parameter, the type
  is only fully known *after* the argument is known. The substitution is
  exactly the lowlevel apply machinery (clone parameter, unify with argument,
  evaluate body), so the checker computes such result types by running the
  apply in the VM — the lowlevel *is* the typechecker's interpreter. **Watch
  item:** the exact rule for computing dependent result types during checking
  is not yet pinned down (§9).

## 4. The checking relation

Bidirectional, guarded, with the checker owning a context stack:

- **Direction.** Every check event is either **synth** (infer) or **check**
  (against an expected type). Annotation = expected. Directions and roles
  (term position vs type position) are recorded per event for diagnostics.
- **Application guard.** To check `f x`: synth `f`; **guard** — `ty(f)` must
  unify with `Arrow(?d, ?c)` for fresh placeholders `?d`, `?c` (a placeholder
  binds; a non-arrow value fails here with a mismatch); `check(x, ?d)`;
  result `?c`.
- **Literals** synth; arrays synth elementwise; `Index` selects a branch's
  type; etc. The operation set is whatever the surface needs on top of the
  lowlevel ops.
- **Indexing** `a[i]`: the value is the structural `Index` over `value(a)`.
  The type evaluation splits on the indexed value's type — a tuple's
  element-type list is structural, so `ty(a[i])` is an `Index` over it (an
  out-of-bounds index is then caught like any `Index`); an array's type
  shape `[element_type, length]` holds the length *as data* rather than as
  selectable positions, so `ty(a[i])` runs the custom
  `HighOperator::IndexType`, which checks `i` against the ArrayType's
  length and yields the element type.  A type known only at runtime (a
  parameter, a call result) takes the operator too — it dispatches on the
  kind the bound type carries, so the check stays lazy until the apply
  binds the array's length.
- **Context.** A checker-owned scope stack of name → type node. λ-bound
  parameters get a fresh placeholder (monomorphic); let-bound variables get
  generalized (below). "Free in context" is the standard generalization test.

## 5. Let-polymorphism

- `let x = e in b` is distinguished from ordinary application **by the
  checker** (the compiler walks the AST, so it knows which applies are lets).
  The VM stays generic — no new operator, no marker. The VM's
  `Apply(Function, arg)` encoding is purely a runtime concern.
- **Generalization** (after checking `e`): walk `ty(e)`'s graph (cycle-guarded;
  may force evaluation), collect class representatives that are unbound and not
  in the context → `(template, freevars)`. This must run *before* `b` is
  checked, and uses of `x` in `b` must never touch the template's classes —
  they go through copies.
- **Instantiation** (at each use): see D2 — copy the template, unify the copy
  with the use-site expected type.
- **Class hygiene.** The checker creates a fresh unbound node for every parameter
  and every fresh placeholder — never reuses — or two uses of `id`
  monomorphize each other. This is the classic silent bug; worth a test.

## 6. Where evaluation happens

Unify operates on **values**. Callers evaluate before unifying, mirroring the
existing apply rule (the argument is evaluated before `unify(cloned_param,
argument)`): the checker force-evaluates type expressions before comparing
them. Forcing is deep, guarded by `evaluate_depth_limit`. On-demand forcing
*inside* unify is a possible later optimization, deliberately not in v1 —
keeping unify structural and callers responsible is simpler.

## 7. Diagnostics

- **Data.** The checker's **diary**: its own unification sequence, each event
  `{a, b, span, role/direction}`. Ground truth at failure: the lowlevel
  `unify_errors` collection (`UnifyError{a, b, value_a, value_b}`). Best-effort
  context: the DSU member lists. Side tables: `node → span` (HashMap, the only
  checker-owned global — needed because rendering explains *member nodes*, not
  just events).
- **Runtime evaluation failures.** An out-of-bounds `Index` (the value side
  of any indexing, the type side of tuple indexing) and the `IndexType`
  operator (the type side of array indexing) record an
  `EvalError{index, index_value, length}` in the lowlevel's `eval_errors`
  collection instead of panicking — the same append-only, no-Result-threading
  contract as `unify_errors`.  `Build::diagnostics` renders each (deduplicated
  by its facts: the value and type evaluation of one expression record the
  same error twice) as `index 5 out of bounds (array length 3)`, attributed
  to the index literal's span.
- **Rendering.** Stable class names `?a` (one class, one name within a
  diagnostic — confluence). Failure at span S renders:
  - bidirectional: "expected X, found Y" (the expected side is the annotation
    or guard);
  - two synths meeting: "cannot match X with Y";
  - then the **flow**: walk the class's member list / diary to show the
    journey — "`?a` was fixed to `int` by the annotation at line 3; `bool`
    comes from `true` at line 5" — HM-loc/Wand/Chitil provenance style.
- **Ambiguity.** Residual unbound placeholders at the top level render as
  "cannot determine type of `x`: `?a` is ambiguous", with the constraints that
  kept it open.
- **Cyclic types.** Render with cycle detection, e.g. `?a = array of ?a` (D1).
- **The bar.** Every error message is grounded in (span, class, role) — no
  panics, no "internal" messages. The earlier value-based-`PartialEq` bug was
  exactly a violation of this bar (spurious errors from pointer equality).

## 8. Module split

- **Layering (2026-08-21 correction).** The highlevel is *not* the language —
  it is a VM layer of its own. The real language (syntax frontend, not yet
  built) compiles *to* the highlevel; the highlevel builds the lowlevel Module
  from its own data structures:

  ```
  language (source) ──compiles──▶ highlevel (Expr data structure + builder/checker)
                                        │ builds + checks
                                        ▼
                                   lowlevel (Module: nodes, unify, apply, GC)
  ```

- **New highlevel crate**: the `Expr` data structure (the IR the language will
  compile into) + the builder/checker pass that constructs and checks the
  lowlevel Module from an `Expr` (roles, guards, context stack,
  generalize/instantiate) + type printer + diagnostics engine (diary + flow
  rendering). API shape: `build(expr) -> Result<Build, Diagnostics>` where
  `Build` holds the typed Module and the root term/type nodes. **No parser** —
  spans live in the `Expr` (optional, supplied by the language layer later).
- **Expr IR representation (2026-08-21)**: dense, id-referenced, no `Box`, no
  name strings — but *not* slotmap-shaped: the IR never changes, runs, or is
  GC'd (it is built once by the frontend and walked by the checker), so a plain
  `expr: Vec<Expr>` with `ExprId(u32)` indices suffices — no blocks, no
  generational keys, no deletion. One `children: Vec<ExprId>` arena serves all
  variadic lists — `Tuple` (a tuple value), `TypeTuple` (a tuple type), and
  `Array` (an array value) each hold a `Range` into it.  Naming follows the
  convention that an instance variant uses the type name (`Tuple`, `Array`)
  while the type variant gets the `Type` prefix (`TypeTuple`, `TypeArray`).
  The real array type is the fixed-arity struct `TypeArray { element_type,
  length }` compiling to `[[element_type, length], [TypeArray, Type]]`
  (instance[0]: the type shared by all elements, instance[1]: the length);
  an `Array` literal compiles to the same shape with the element types
  unified into one shared cell and the length set to the element count.
  Note: the spec's surface examples write `Array(n, int)` (length first);
  the IR's fields are ordered by the instance — `element_type` first,
  `length` second — the frontend maps the surface operands positionally. A use of a binding is
  the pre-resolved `Parameter` expression's own `ExprId` — the language frontend
  resolves names to ids before the highlevel sees the tree, and the checker's
  scope stack is keyed by `ExprId` (no strings anywhere in the crate). `let`
  is desugared by the frontend into an apply of a function over the value
  (`Apply { function: Function { parameter, return }, argument: value }`;
  the variants are struct-style with named fields — 2026-08-22: the `Var` and
  `Let` variants were removed; the checker's
  `Parameter` arm resolves the use to the compiled parameter pair). The IR stays
  pure
  (structure + span only); the checker's products live in dense parallel
  arrays indexed by `ExprId` — `term: Vec<Option<NodeId>>` (compiled value
  node) and `ty: Vec<Option<ExprId>>` (**the type of an expression is itself an
  expression**, not a compiled node).
- **Types are expressions (2026-08-21, user: "ty of an expression should still
  be an expression, or else we can not express the type_of() function").**
  `ty: ExprId → ExprId`; `type_of(e)` is expressible exactly as the Expr
  `ty(e)` — an expression whose value is e's type. Consequences:
  (a) new `ExprKind::TyVar` — a fresh type variable, compiles to a fresh
  unbound node; each parameter.s type starts as a TyVar, and after unify its
  node's class carries the type value, so it remains a correct type expression
  for the parameter.s whole lifetime;
  (b) the checker appends synthesized type expressions (TyVars, canonical
  type constants, structural arrow/array types) to the table — the
  frontend-built part is immutable, the checker only grows it;
  (c) instantiation moves into the IR domain: copying the generalized type
  *expression* (with fresh TyVars) and compiling the copy replaces the earlier
  lowlevel `copy_subgraph` idea — one less lowlevel change;
  (d) typing judgment = `unify(node(e), node(ty(e)))` for bindable nodes
  (vars/params — unbound, so unify binds); for concrete nodes (literals) the
  highlevel's own shape-compatibility rule applies instead — the
  concrete-vs-concrete rule is the highlevel's concern, the lowlevel stays
  structural (this is the one part of the model to confirm at Milestone B).
- **Lowlevel additions** (small): expose a cycle-safe copy API for
  instantiation (or confirm the existing copy machinery handles cycles);
  nothing else required for D1 (no occurs check). Unify, apply, GC, guards
  stay as they are.
- **Nominal struct types (2026-08-22).** New types are created at runtime by
  a fresh-type-id mechanism; identity is nominal, not structural.  The IR
  form is `ExprKind::TypeStruct(ChildRange)` — the positional field types
  (no names in v1) — compiling to the pair
  `[field types, [TypeId(n), Type]]`: the same shape as a tuple type, but
  the kind slot holds a *fresh nominal id* (`HighValue::TypeId(n)`) instead
  of the fixed `TypeTuple` marker.  The id comes from `HighOperator::Fresh`,
  a nullary extension operator that reads and increments
  `HighGlobalExt::type_id_counter` — the highlevel's global extension state,
  carried in the lowlevel `Module`'s `global_ext` slot via the new
  `Program::GlobalExt` associated type (the lowlevel stays generic; the
  counter lives in the highlevel's own state).  Because `Fresh` fires once
  per source occurrence (and is cached), each `TypeStruct` expression
  allocates a distinct id: unify (`PartialEq` on `TypeId`) merges equal ids
  and conflicts on different ones, so two occurrences never unify, and a
  struct never unifies with a same-shape tuple type.  A struct type is
  reused by binding it once through a parameter — the same first-class-type
  reuse as any type value.  Kinding accepts `[TypeId(n), Type]` as a kind;
  an unevaluated `Fresh` marker (pending at check time) defers the kinding
  check to the runtime.  Diagnostics render a struct as `struct#n { f1, f2 }`
  and a bare id as `TypeId(n)`.  Out of scope for now: struct *value*
  construction (an instance form / constructor), top-level `struct A { .. }`
  declarations (the program is one expression; bind the type via a
  parameter), and `struct` source syntax in the language layer.

## 9. Open items

- Exact rule for computing dependent result types through apply (D3) — the
  one piece of the checker design not yet pinned down.
- Cycle-safety of `copy_nodes` for cyclic templates (D1 × D2).
- Whether the FV walk for generalization forces unevaluated type expressions,
  and how that interacts with open parameters (D3).
- The highlevel `Expr` IR shape — the minimal constructor set; the language
  layer will compile source into it (concrete syntax is out of scope for the
  highlevel itself).
- Worked examples to drive tests and diagnostics: polymorphic identity, fib,
  a dependent `Array(n, int)` example, and the target diagnostic for the
  `let id = \x. x in (let y : int = id true in y)` case.
