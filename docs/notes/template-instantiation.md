# The template instantiation

> Status: **the lowlevel API is current; the source-level `@clone` is retired.**
> The walk below is still in the tree — `Module::clone_template` and its
> `TemplateBoundary` — but the parser no longer produces a clone expression
> and the checker has no clone arm: there is no source-level `@clone` any
> more. Freshness comes from **function application** and from the checker's
> **requirement instantiation** for direct compound templates; the latter
> copies open value slots **and open type cells**, and shares decided types and
> attributes (§2.1). That is the model the [direct template
> note](value-templates.md) records. The walk's own
> rules are kept here because the apply's walk shares them and because the API
> is still the lowlevel's only general-purpose instantiation.
>
> Points at: `crates/lichen-lowlevel/src/template.rs` (the walk),
> `crates/lichen-highlevel/src/checker/templates.rs` (requirement instantiation),
> `crates/lichen-highlevel/src/checker/structs.rs` (`applied_parameter`,
> `expected_argument_requirement`) and `lambda.rs` (`check_app`) for the
> returned-function rule,
> `crates/lichen-language/tests/language/template_instances.rs` (the pins),
> `crates/lichen-lowlevel/src/apply.rs` (the regroup the class topology uses),
> `crates/lichen-lowlevel/src/function.rs` (the apply's own clone walk, whose
> rules this one reuses), and
> [`apply-clone-ownership.md`](apply-clone-ownership.md) (the clone rules the
> two share).

## 1. The API

```rust
Module::clone_template(
    root: NodeId,               // the template's root cell
    target: BlockId,            // where the instance's cells land
    owner: Option<FunctionId>,  // the function that owns them, if any
    boundary: &TemplateBoundary,// the cells the caller shares
) -> Option<TemplateInstance>
```

`TemplateBoundary` is the caller's set of **shared** cells: an outer lambda's
parameter, another block's root binding — the cells a template *depends on* from
outside itself. `TemplateInstance` answers `root` (the instance's own root
cell), `remap` (template cell → instance cell) and `shared` (the template cells
the instance references in place).

`None` answers a walk a budget stopped. A budget is not a success: an
incomplete instance is worse than none, so there is none.

## 2. Why a walk of its own, and what it shares

The apply already clones a template — a *function's* template. Its walk answers
two questions the template walk answers differently:

| | the apply | the template |
|---|---|---|
| membership | the cell's **class** lies inside the applied function's scope | the cell's **class** lies inside the template's own scope |
| freshness | the cell depends on the argument | the cell's own content is not closed |

Everything else is the apply's, reused rather than restated: one clone per
source cell, the operator travelling with its operand, and the class topology
rebuilt by the **same** `apply::regroup_clones` / `apply::unify_clone_groups`
the apply and the static apply use. A third traversal with its own freshness
rule would be a third semantics; this one is the apply's walk with two
decisions swapped.

The **wildcard** is the third decision, and it is this walk's own. The apply
clones a function's *body*, so an unconstrained position in it stays a
wildcard: the body's `@any : t` field is a requirement, not a capture
position, and nothing writes into it. This walk clones a *template*, whose
slots are capture positions: the instance's own cell replaces the wildcard, so
the actual's value fills it.

## 2.1 The checker's requirement instantiation

The checker copies a direct compound template per use (`instantiate_requirement` in
`crates/lichen-highlevel/src/checker/templates.rs`), and the same rule serves both a
struct instance and a unify template: **an open value cell and an open type cell are
fresh; a decided type, the attributes, the name table and the nominal id are shared.**
`AnyLit::build` gives `@any` a fresh own type cell, so sharing that cell made the first
instance decide the type every later instance inherited — the failure the spec's Ideal
API 4 is about.

An **application's function is the clone its own call made, not the template**.
`applied_parameter` reads the clone recorded for the call (`Module::applied_exprs`), an
anonymous instance (`_{...}`) is pinned to that clone's parameter, and `check_app` skips
the type side that would unify the template the clone came from. A check the type side
no longer performs still happens at the value side.

Measured:

- `D := {.a @any, .b @any}; (D{.a 1, .b 2}, D{.a 1.5, .b 2.5})` answered
  `expected Int, found Float` at the second instance; it now answers both.
- `div := {.'dividend, .'dividor} -> dividend / dividor` takes an `Int` pair and a
  `Float` pair in one program.
- The annotation still decides: `.a @any : Int` refuses `D{.a 1.5}` at check stage
  (`Annotation`).
- `examples/gc.lichen` declares `[8, 16]: Array` and answered `[none, none]: Array` at
  HEAD; it answers its declared output now, because each call of
  `add := 'x -> 'y -> x + y` needs its own inner parameter.

**Open: the inline literal domain under a type parameter.**
`div := 'T -> {.'dividend: T, .'dividor: T} -> dividend / dividor`, called twice as
`div _ _{…}` with an `Int` and a `Float`, is still refused (`expected Float, found Int`):
the inline literal's type argument is shared by the returned function's parameter, and
the checker has no per-call specialization to copy it. The pinning test is
`template_instances::a_function_generics_literal_domain_takes_anonymous_instances`
(`#[ignore]`); the lowlevel replay in §3 is the mechanism that would supply the call's
own cells.

**Rejected: copying open type expressions.** Making the type slot fresh by copying any
open type expression, rather than only a bare open cell, leaked the compute buffer's raw
marker through `frozen_buffer_reuse::a_frozen_buffer_cell_still_feeds_the_next_launch`
(`[raw 6, raw 8, raw 10, raw 12]` instead of `[6, 8, 10, 12]`): the type expression's own
decided interior was copied with it. **Rejected: using the applied clone as the callee.**
It makes a later call re-instantiate a function whose block has been dropped: `@gc`
programs panicked intermittently in `gc.rs`'s `class_walk` (`invalid SlotMap key used`).

## 3. The rules

- **The class decides, not the cell.** A cell is cloned when its **class** lies
  inside the scope of the function that wrote the template's root cell, and
  the class needs a fresh cell (one of its members is open); otherwise the
  class is referenced in place, as a whole. The scope is the **template's own**,
  not the caller's: the caller names where the instance lands, not the lexical
  scope being instantiated. A template with no owner has no source scope, so
  no unmarked class is inside one: the virtual root is external, never neutral.
  The caller's marking wins over all of this: a marked cell is referenced in
  place, and a marked root shares the whole template. The rule and its
  measurements are in [`apply-clone-ownership.md`](apply-clone-ownership.md) §4.
- **A template's own open cells are fresh.** The instance's cell for an open
  cell is a new empty cell, so a unify that fills it writes into the instance
  and not into the template. The template keeps its own open cells; nothing the
  instance does pollutes them.
- **One template cell has one instance cell.** The map is written before the
  walk descends, so a diamond names one cell and a cycle names the instance's
  own. A *placeholder* is fresh **per source cell**, not per reach — the
  apply's wildcard arm mints a cell per reach, and a template's internal
  sharing must survive the clone.
- **A marked cell is shared whole.** The instance references it in place, so
  the dependency the template states on it is kept, and the marked cell's own
  content is the caller's, not the instance's.
- **A decided and fully closed cell may be shared; a cell with an open interior
  may not.** A cell is **closed** when it holds a decided value, has no
  operator, and every cell its content reaches is closed too. The top-level
  value alone does not decide it: a tuple whose second element is an open hole
  is not closed, however decided the tuple itself is. The reach runs over the
  content, so a cycle among the open cells is open as a whole — the graph is a
  graph, not a tree. The **apply's walk shares the same closed cells**, by the
  same proof, so a decided layout — a name table above all — is one cell in
  the instance and not two: two tables are never one handle, so a copied one
  would conflict with the original.
- **The wildcard is decided but never carried.** `LowValue::Any` is a value, not
  an open hole in the template; the instance replaces it with a fresh empty cell,
  so the caller's value can fill it. Sharing the template's wildcard cell would
  instead keep every instance undecided to readers.
- **A frozen cell is read in place when it is closed, and becomes the
  instance's own when it is open.** The closure test is the deep one, over the
  frozen module's own payload graph: a frozen cell that holds a decided array
  whose element is an open cell is **not** closed, and must not be shared. A
  frozen cell cannot be written, so an open one has to become the instance's
  own cell. The instance's cell for it is the instance's own in every respect:
  homed in the target block, owned by the owner, shaped by the frozen cell, and
  its origin the cell that named the frozen one.
- **The class topology is rebuilt among the instance's cells**, by the apply's
  own regroup: the template's classes are the instance's classes. A shared cell
  joins the group of its own class, because the instance's cell for a template
  cell that is one expression with it must be one expression with it too.
- **An operator travels with the instance; its operand is the instance's own.**
  A computed answer travels only when the deep pass ran on it — a fact of the
  template; otherwise the instance's operator computes it again.
- **An operator's operand gets no parent edge.** An operator's input is not the
  cell's content, so it does not carry openness upward; the cell that holds the
  operator is open by its own account, so the operand cannot make it more open.
  The operand's own closure decides whether it is copied.

## 4. The graph has two edge kinds

- A **payload edge** names a cell the value holds: an array's element, a
  table's entry. The walk clones it, and the instance's value points at the
  instance's cell.
- A **trace edge** names a cell a plugin value keeps alive — the edge the GC
  follows that the graph does not name (`ValueExt::traced`). It decides
  closure: a value that keeps an open cell alive is **open**, so the cell
  holding it is cloned rather than shared. It is not cloned itself, because
  `P::Value` is opaque and `ValueExt` has no hook to rewrite a plugin value's
  internals — the cells a plugin holds stay the template's, the same rule the
  apply's `copy_ext` follows.

## 5. What it does not do

- **A closure captured by the template is shared, not re-homed.** The apply
  re-homes a closure because it mints one per call; the template walk mints
  none, so a captured closure's body is the template's. A template that
  captures a closure therefore shares that closure's cells — the one case where
  the instance's cells are not the instance's own.
- **The cells a plugin value keeps alive stay the template's.** See §4: the
  lowlevel can read the edge but not rewrite it.
- **Every clone lands in the target block.** The apply maps a scope's content
  to a fresh child block; the template walk does not, because the caller names
  the block the instance lives in.

## 6. The readings

The standing reading is the crate's own suite, unchanged by this slice:

```text
cargo test -p lichen-lowlevel --test basic
test result: ok. 164 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
```

The walk's own rules were measured with **probes** during the slice: ten
`template_clone_*` cases over the fresh cells, the shared boundary, the internal
sharing and the cycle, the rebuilt classes, the operator and its operand, the
closed and the open interior, the wildcard, the owner's registration, the
budget's refusal, the frozen cell that is open beside the one that is decided,
the frozen cell whose *interior* is open, and the plugin value that keeps an
open cell alive. They were removed from the tree on the maintainer's
instruction — this workspace does not admit new tests without it — so they are
recorded here and not kept as standing verification.
