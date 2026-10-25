# The template instantiation

> Status: **current.** The lowlevel base `@clone` needs: one walk that clones a
> template's value graph, keeps the template's own cells fresh, shares the cells
> the caller marked, and rebuilds the graph's shape in the instance.
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
source cell, the operator travelling with its operand, the wildcard never
carried, and the class topology rebuilt by the **same**
`apply::regroup_clones` / `apply::unify_clone_groups` the apply and the static
apply use. A third traversal with its own freshness rule would be a third
semantics; this one is the apply's walk with two decisions swapped.

## 3. The rules

- **The root is always cloned.** An instantiation is an instance, and the root's
  own class may hold the template's internal cells.
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
  operator, and every cell its value's payload reaches is closed too. The
  top-level value alone does not decide it: a tuple whose second element is an
  open hole is not closed, however decided the tuple itself is.
- **The wildcard is decided but never carried.** `LowValue::Any` is a value, not
  an open hole; the instance's cell replaces it, so the caller's value can fill
  it — a class holding the wildcard neither merges nor writes, so sharing the
  template's wildcard cell would lose the value.
- **A frozen cell is read in place when it is closed, and becomes the
  instance's own when it is open.** A frozen cell is readable and not writable,
  so an open one cannot be the instance's cell: it is materialized into the
  instance's block. A closed one crosses as the reference it is.
- **The class topology is rebuilt among the instance's cells**, by the apply's
  own regroup: the template's classes are the instance's classes. A shared cell
  joins the group of its own class, because the instance's cell for a template
  cell that is one expression with it must be one expression with it too.
- **An operator travels with the instance; its operand is the instance's own.**
  A computed answer travels only when the deep pass ran on it — a fact of the
  template; otherwise the instance's operator computes it again.

## 4. What it does not do

- **A closure captured by the template is shared, not re-homed.** The apply
  re-homes a closure because it mints one per call; the template walk mints
  none, so a captured closure's body is the template's. A template that
  captures a closure therefore shares that closure's cells — the one case where
  the instance's cells are not the instance's own.
- **Every clone lands in the target block.** The apply maps a scope's content
  to a fresh child block; the template walk does not, because the caller names
  the block the instance lives in.

## 5. The readings

`lichen-lowlevel`'s own lines, on this branch:

```text
cargo test -p lichen-lowlevel --test basic
test result: ok. 174 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out

cargo test -p lichen-lowlevel --lib
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The ten `template_clone_*` tests in `crates/lichen-lowlevel/tests/basic/function.rs`
cover the rules above: the fresh cells, the shared boundary, the internal
sharing and the cycle, the rebuilt classes, the operator and its operand, the
closed and the open interior, the wildcard, the owner's registration, the
budget's refusal, and the frozen cell that is open beside the one that is
decided.
