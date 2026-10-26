# The template instantiation

> Status: **the lowlevel API is current; the source-level `@clone` is retired.**
> The walk below is still in the tree — `Module::clone_template` and its
> `TemplateBoundary` — and the checker still carries the `ExprKind::Clone`
> arm, but the parser no longer produces a clone expression: there is no
> source-level `@clone` any more. Freshness comes from **function
> application** instead, which instantiates a function's parameter and body
> and shares the captured cells; that is the model the
> [constructor note](value-templates.md) records. The walk's own rules are
> kept here because the apply's walk shares them and because the API is
> still the lowlevel's only general-purpose instantiation.
>
> Points at: `crates/lichen-lowlevel/src/template.rs` (the walk),
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
| membership | the cell belongs to the applied function | the cell is reachable from the root |
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

## 3. The rules

- **The root is always cloned.** An instantiation is an instance, and the root's
  own class may hold the template's internal cells. (The caller's marking wins
  over this: a marked root shares the whole template.)
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
  an open hole; the instance's cell replaces it, so the caller's value can fill
  it — a class holding the wildcard neither merges nor writes, so sharing the
  template's wildcard cell would lose the value.
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
