# The attribute isomorphism — handoff

> Status: **in progress**, handed over. Branch `spec-literals` (base `dev`), HEAD
> `0ae17d02`, working tree clean, not merged. The rule is landed in the language
> spec and the attribute-specific record machinery is gone; **five tests stay red**
> and their two mechanisms are analysed below.
>
> What this note is: the state a successor resumes from — what the rule is, what
> landed, the exact readings, the two open mechanisms with the probes that found
> them, and the traps that cost this session its time.
>
> Points at: `docs/language-spec.md` §1 and §3, `lichen-highlevel`
> (`checker`, `checker/annotations`, `checker/lambda`, `shape`), `lichen-lowlevel`
> (`apply`, `static_module/apply`), `lichen-compute` (`compute.rs`
> `compile_fragment`), and the notes
> [array-attribute-view](array-attribute-view.md),
> [kernel-parameter-class](kernel-parameter-class.md),
> [apply-type-side-check](apply-type-side-check.md).

## The rule

Every position of an expression is an expression: the value, the type, and one
per attribute (`#`, `?`, `!`). The **type position is the model** for every
attribute: it holds an expression, it is unified, it rides the function's
parameter and return expression so an applied clone re-materializes it per call,
and an absent position is an undecided cell. A value/type unification pairs the
attribute positions **one-to-one in canonical order**, and the pairing is
recursive into a compound's element slots. There is **no attribute-specific
record, table or reference**.

A literal decides its own attributes: a constant states `# 0` and the wildcard
for `?`/`!`; `_` is undecided in every attribute, one hole per position. An
annotation is syntax — what it spells is replaced and unified; its own order
decides relaxation. `@any` states nothing, at any position.

## What landed

| commit | change |
|---|---|
| `1d8e6d5f` | the rule in the spec; `Function`/`StaticFunction.return_marker`/`return_field`, `attribute_reference`/`attr_refs`, the static mirrors and their persisted bytes deleted (**artifact format 21**); a value literal decides its attributes, `_` is undecided in every position |
| `1911f8c8` | `pair` → `expr` everywhere: the record is an expression |
| `5dc79926` | `unify_parameter_slots`: the non-reporting static apply binds the frozen clone to the argument |
| `7b93eb1a` | a reference in an attribute names the field's own slot |
| `6521974a` | one materialization per call: the signature remap is shared by the parameter and the return copy (measured: no change alone) |
| `2721a1ab` | a reference names the field record's attribute position, not a result cell |
| `592b0854` | the non-reporting static apply keeps the signature's record (reverted the earlier bind; see the trap below) |
| `5d585047` | the unify covers **every** position, the type included (the `slot == 1` skip was a masking and is gone) |
| `cdb1d638` | `annotation_value_slot` uses the expression's own node, no class representative |
| `3577398e` | the return copy rides the **dynamic clone rule** (`static_node_apply`), per `kernel-parameter-class.md` |
| `017f3db5` | a per-call cell enters the signature remap, so the mirror rule resolves a reference to this call's clone — **and `a_write_returns...` passes** |
| `0ae17d02` | `shape`'s low-type readers cross the frozen boundary (`static_read`) |

## Readings (the tool's own `test result:` lines)

| suite | result |
|---|---|
| `lichen-highlevel --test highlevel` | **136 passed; 0 failed** |
| `lichen-language --test language` | **274 passed; 1 failed** |
| `lichen-compute --test lichen_compute` | **232 passed; 5 failed; 1 ignored** |

Red:

- `compute::jit_inline_lichen_function`
- `compute::jit_inline_nested_function`
- `compute::a_float_domain_is_permitted_at_every_position_the_walk_reaches`
- `compute::a_jit_kernel_crosses_the_two_classes_both_ways`
- `perspective_requirements::a_read_takes_its_perspective_from_the_buffer_and_the_index`
- `language::field_read_kinds::a_named_read_of_a_deferred_struct_is_accepted`

## Open mechanism 1 — a read's lane-varying index stays warp-uniform

The read wrapper has no `#` annotation; its result must be the meet of the
buffer's and the index's perspectives, so a lane-varying index must be refused
with "expected 32, found 1".

Measured with a temporary print in `check_app`'s certificate branch: the read
call's certificate decides to **`Some(Any)`**, so `(r # warp)` unifies with the
wildcard and is accepted. `stated_attributes`
(`lichen-highlevel/src/checker.rs`) skips an attribute item whose **own** value is
the wildcard; the certificate here is a **pair whose value position is the
wildcard**, which it does not skip.

Tried and **rejected**: extending `stated_attributes` to skip a wildcard-valued
slot too. It changed nothing, which means the certificate does not come from
`stated_attributes` at all (or the pair is built later).

Next probe: in `check_app`, print the certificate's expression with its origin
(`Loc`/span and `ExprKind`) for the read call specifically; the session's last
`ExprKind` dump printed ids that differ per compilation, so the print must
identify the call by its source span (the `compute.read` application), not by
`ExprId`.

## Open mechanism 2 — a jitted kernel's class reads undecided

`compute.jit` refuses with `UNDECIDED_DOMAIN` for
`k = compute.jit (x => helper x + 1)`, which must check.

Measured in `compile_fragment`
(`crates/lichen-compute/src/compute.rs`):

- the kernel's `param_type` slot **has a value** (a type record array), but
  `shape::low_type_of_slot` returns `None` and `struct_fields_of_slot` is false;
- that record's **items are static refs** (`node.dynamic()` is `None`), and
  `Module::static_read` on them is `None` — the frozen node carries no cached
  answer;
- a temporary print in `unify_parameter_slots` showed the slot-1 operands are
  both proper five-position records with shape/kind arrays and wildcard
  attributes, so the **pairing is right**; the loss is after the unify, in the
  clone/class write-back of the frozen record.

The note [kernel-parameter-class](kernel-parameter-class.md) names the rule: the
static walk must run the dynamic clone rule, and **a class write must cross the
frozen boundary**. `static_node_apply` is the mirroring walk; `shape`'s readers
now read static refs, and that alone did not close it.

Next probe: print a frozen type record's element nodes and their
`runned`/`evaluated_deep` at freeze, and the same after `static_node_apply`, to
name the rule that drops the answer.

## Traps (each cost time this session)

- **Do not special-case an attribute against the type.** A `slot == 1` skip in
  `unify_parameter_slots` made five pinned renderings pass and was rejected: it
  binds the value and the attributes but not the type. The same applies to a
  "combine ignores an undecided contribution" rule.
- **The class invariant is the reading.** Every member of a class *is* the same
  expression, so a node-level read is the class read; `annotation_value_slot`
  must not select a representative.
- **A static apply mirrors the dynamic one.** `kernel-parameter-class.md`'s
  load-bearing statement; the signature clone carries every per-call cell into
  the remap, and residual operations keep their frozen reference.
- **The doc/code pair moves together.** This branch is **not mergeable** while
  the five tests are red, because the spec on this branch states the rule the
  code does not yet wholly keep.

## Verify

```pwsh
$env:CARGO_TARGET_DIR='C:\resource\lichen-vm\.worktrees\spec-literals\target'
cargo check --manifest-path C:\resource\lichen-vm\.worktrees\spec-literals\Cargo.toml --workspace --all-targets
cargo test --manifest-path C:\resource\lichen-vm\.worktrees\spec-literals\Cargo.toml -p lichen-highlevel --test highlevel
cargo test --manifest-path C:\resource\lichen-vm\.worktrees\spec-literals\Cargo.toml -p lichen-language --test language
cargo test --manifest-path C:\resource\lichen-vm\.worktrees\spec-literals\Cargo.toml -p lichen-compute --test lichen_compute
```
