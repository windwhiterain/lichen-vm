# Proposal (v8): the match covers the checker's initial unify and the birth normalize; pending requirements are records; the attribute relation reads the candidate; frozen artifacts carry the set

> Status: **proposal — for review, not applied.** Supersedes v7 (`396f7383`). v8 adds the
> site v7's path table missed: the checker's **initial general unify**
> (`check_unify_expr`) and the function's **birth normalize**, the other two of the three
> dynamic unification sites `Module::unify`'s own invariant names. The spec text of §1–2
> is unchanged; the paths and the acceptance argument are extended. The spec file is
> untouched.

## 1. The match rule (spec text, as v6)

> A **template match** is `a = t` where `t` is a template. The match reads `t`'s
> expression children, and for each position: the **type** is `t`'s type child's
> denotation, shared and checked; the **attributes** are `t`'s attribute children,
> shared, and the match applies **the attribute's own relation with the declaration on
> the left** (`declared ⊑ value` for `Perspective`); the **value** child: a **closed**
> value is a **pin** (`a`'s value must equal it), an **open** value is a fresh cell for
> this use filled by `a`, a **type expression** states the type requirement and carries
> no value; a **group** child is matched position by position.
>
> The match's answer is `a`; `t` is never specialized and never a place's value. A read
> of a matched place answers `a`'s value for that part.

A **closed** value child with an **open actual** registers the pending requirement; an
**open** value child is fresh-filled and contributes no pin.

## 2. Pending requirements (spec text, as v6)

> A class may carry **pending requirements**: a **pin** (a closed value the bound value
> must equal) or an **attribute requirement** (the attribute's own relation, declaration
> on the left). A requirement is recorded on the **actual's class** when the actual is
> not yet decided; it is not installed as a value.
>
> - **Union.** A class union merges the members' requirement sets.
> - **Clone.** A dynamic clone copies its template's set through the clone walk's remap;
>   a static clone copies it from the frozen artifact's set through the static remap.
> - **Pre-write gate.** A value becomes visible to a class only after every pending
>   requirement of the class accepts it. A refusal leaves the value uninstalled.
> - **Movement.** GC relocation and freeze move an already-checked value; they keep the
>   set and do not re-check.

## 3. The mechanisms (not spec text)

### 3.1 A candidate-aware relation, not `is_subtype` on nodes

The existing `AttrExt::is_subtype` reads its operands through `Ctx::node_value`
(`program.rs:225–237`), and the gate runs **before** the value is installed, so it would
read `None` and answer `false` — refusing a legal `# 8`. The relation the gate calls
therefore takes the candidate **as a value**:

```text
trait AttrRelation<P> {
    fn accepts(&self, module: &Module<P>, declared: AnyNodeId, candidate: &P::Value) -> bool;
}
```

`Perspective` reads `declared`'s value from the module and compares `declared ⊑ candidate`;
`Refinement` implements it from its predicate. No value is installed for the call, and no
read of the candidate goes through the node slot.

### 3.2 The gate's decision is `equality || subtype`, not subtype alone

`AttrExt::is_subtype` is documented as the fallback **after** a failed equality
(`attr.rs:155–168`), and `Refinement` does not override it — two equal predicates pass by
plain equality (`refinement.rs:45–58`). The gate composes both, with a read-only equality
(never the installing unify):

```text
match requirement:
  Pin { expects }            => value_eq(expects, candidate)
  Attribute { ext, expects } => value_eq(expects, candidate)
                                || attr_relations[ext].accepts(module, expects, candidate)
```

Acceptance case: the same `Refinement` predicate under a deferred actual passes by
equality; `Perspective # 4` against `# 8` passes by `declared ⊑ value`; `# 4` against
`# 2` fails both.

### 3.3 One gate, every visibility path, and a distinct name

```text
Module::pending_requirements: HashMap<class-root, Vec<PendingRequirement>>
Module::attr_relations: HashMap<extension-id, Box<dyn AttrRelation<P>>>
Module::install_value(class, value) -> Result<(), UnifyError>
```

The name is `pending_requirements`, distinct from the existing `NodeId → AnyFunctionId`
requirement registry (`lib.rs:1084`). `install_value` replaces the direct writes in
`write_node_value`, `write_node_answer`, `propagate_class_value`, `fill_class_holes` and
`add_equality`'s pre-union fill; a refusal returns before the write. GC's
`relocate_node_value` and freeze's direct `node.value` write (`freeze.rs:268–279`) are
**licensed movements** of an already-checked value: they keep the set and skip the gate.

### 3.3a The three dynamic unification sites, all routed

`Module::unify`'s invariant names exactly three sites: **the checker's initial unify, a
function's birth normalize, and an apply's runtime unify**. A match that covered only the
last two would leave the checker's own `r = k.O` installing the declaration before any
clone exists, so all three are in the match's path:

1. **The checker's initial unify** — `check_unify_expr` in `checker/templates.rs`. Its
   function-template branch already does the right thing for an undecided actual (it
   records the requirement and unifies types only); the general branch does not, and
   `r = k.O` (a struct template, not a function type) takes the general branch. The
   general branch becomes the same registration: a template against an **undecided**
   actual registers the records (§2) and unifies the **types only**; the actual's value
   slot stays open. A decided actual keeps the whole-term unify, whose agreement is the
   pins' check and whose disagreement is the existing diagnostic.
2. **A function's birth normalize** — the checker's normalization: after each top-level
   statement, every new lambda's value node is deep-evaluated before the definition pass
   (`checker.rs`'s `build_with`). The SPEC's sugar `'x => {…; expr}` => `{…; @any = x}`
   is that site. In the implementation the wildcard completion is a **write** of `Any`
   (`checker/lambda.rs`, the identity-return completion), which installs no declaration
   and therefore needs no gate today; any unify the normalize performs is the same
   registration as site 1.
3. **An apply's runtime unify** — the parameter slots; the clone walk copies the records
   through its remap (the clone rule in §2), and the gate checks them at the bind.

The acceptance argument for the plrun case, stated against this: after the checker's
initial unify, `r` holds **no** value — it holds the codomain's records; `first.r` cannot
read a declaration the checker installed because the checker installed none. The read
answers whatever the runtime fills (the recorded buffer), or is undecided before the fill.

### 3.4 Frozen and static paths

A frozen artifact carries the requirement set with the cells it names, or a static
application cannot check what the dynamic one checked:

- `StaticModule` gains a `pending_requirements` table keyed by the static class root.
- **Freeze** maps each dynamic requirement into the frozen tables the same way it maps an
  edge: the declared cell and the expectation's node refs are frozen with their usual
  remap (`freeze.rs`'s `freeze` construction), and the set is stored per class.
- **Container format** persists it where the other frozen maps are written:
  `crates/lichen-language/src/persist/container.rs` writes `apply_result_owners` and
  `field_sources` as sorted node-index pairs and gates the whole artifact on
  `ARTIFACT_FORMAT_VERSION`. The pending requirements are written the same way (sorted by
  class root, each entry with its expectation refs), the format version is bumped, and a
  reader of an older format is refused by the existing
  `unknown artifact format version` check.
- **Load** restores the table and rewrites the refs with the artifact's own module ids.
- **The static clone walk** (`static_module/apply.rs`'s remap) copies the set through its
  static remap, as the dynamic walk does.

## 4. §Struct: the reduction text is unchanged

`raw v[...] = e1` is the match; `e = raw v[...]` fills the place. The rule states what
the match does with the template's value; no symbolic change is needed.

## 5. Paths

| path | what happens |
|---|---|
| checker, initial unify (`check_unify_expr`) | a template against an undecided actual registers the records and unifies types only; a decided actual keeps the whole-term unify (its agreement is the pin check) |
| checker, birth normalize (`@any = x`) | the same registration; no template value is installed |
| checker (`check_instantiate`) | resolves each field's type/attributes; a decided actual checks now; a closed pin with an open actual pushes a `Pin`; an open attribute with an open actual pushes an `Attribute`; an open value child stays a fresh cell |
| dynamic apply (clone walk) | copies the set through the remap; a union merges the sets |
| static apply | copies the frozen set through the static remap |
| every visibility site | `install_value`; refusal before the write |
| GC / freeze | licensed movement; set kept, no re-check |
| reads | unchanged |

## 6. Acceptance cases

0. **The checker's initial unify**: `r := $plrun(k.native, a, k.O); r : type_of (k.O);
   r = k.O` — after the unify `r` has no value; a read of `r.r` before the result runs is
   undecided, and after the result fills it answers the recorded buffer. The declaration
   `k.O` is nowhere in `r`'s class.
1. **Deferred pin**: `S := {.a 5 : Int}; x := _; S{.a x}; x = 5` accepted, `x = 6`
   refused `expected 5, found 6`, no pre-fill.
2. **Two pins on one class**: `S1{.a x}` with 5 and `S2{.a x}` with 6, `x = 5` accepted
   by `S1` and refused by `S2`; both are checked.
3. **Per-call clone**: a template pin 5 cloned with an open argument bound to 6 refuses
   on the clone; the template is untouched.
4. **Deferred Perspective**: declared `# 4` with an open actual bound to `# 8` is
   accepted by `declared ⊑ value`; bound to `# 2` it is refused; the relation reads the
   candidate value, not the uninstalled slot.
5. **Equal Refinement predicates**: the same predicate under a deferred actual is
   accepted by equality (`equality || subtype`, not subtype alone).
6. **Frozen path**: an imported module's deferred requirement refuses a wrong bind after
   load, showing the artifact carried the set.
7. **Type parameter / `@any : Int` / plrun codomain** as v5.

## 7. Rulings kept

`=` is the only template match; `:` is not a match; a direct template is instantiated per
use; the call-result template stays `r : type_of (k.O); r = k.O`; the annotation rule is
untouched; the attribute extensions keep their own relations, called with the candidate
value and composed after equality.
