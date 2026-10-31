# Proposal: a struct instance's field is a class reference

> Status: **proposal — for review, not applied.** Proposed for
> [`docs/language-spec.md`](../language-spec.md) §Struct. The wording below is written
> in the spec's own form (`:=`, `@raw`, `sytax`) so it can be applied verbatim if it is
> accepted. The spec file itself is untouched.

## Text proposed for §Struct

The reduction of an instance and its read, replacing the current §Struct reduction:

```sytax
`s{.n2 e3, .n1 e4}` 
:= 
`{
  let {.n1 d1, .n2 d2} = s;
  v := raw[name_table, nominal_id, @raw None, @raw None];
  d1 = raw v[name_table["n1"]+2];
  d2 = raw v[name_table["n2"]+2];
  e3 = raw v[name_table["n2"]+2];
  e4 = raw v[name_table["n1"]+2];
  raw[raw v, Struct, ...]
}`

`s.<name>` := `{type_of s = Struct; @raw s.value[s.value.name_table[<name>]+2]}`
```

One normative sentence is added after the block:

> A struct instance's field holds **no value of its own**: it is the class of the
> declaration's field and the supplied expression, and a field read answers that
> **class's value**. A class holding no value is undecided; the declaration states a
> requirement, not an answer.

## What changes against today's §Struct

Today's reduction writes the declaration's field into the new instance and then unifies
the argument with it:

```sytax
v := raw[name_table, nominal_id, @raw None, @raw None]
raw v[name_table["n1"]+2] = e1;
raw v[name_table["n2"]+2] = e2;
e3 = raw v[name_table["n2"]+2]
```

The difference is one symbol: `raw v[...] = d` is a **unify** (the declaration's field
joins the class) where today's text writes a value into the field's own slot. While the
class is open, today's write is what a read answers; under one class the read answers
the class's value, which is what a later `e = ...` bind can fill.

## The implementation it requires (not spec text)

- An instance's field node (the place `v[name+2]`) must be created with **no value**;
  the declaration's field and the argument join its class.
- A read (`Module::slot_value_of` in the compute layer, and the lowlevel `Index` read)
  must answer the field's **class value**, not the node's own slot.
- `Module::fill_class_holes` then fills the place with the argument's value when the
  class is decided, and the read answers it.

## The failures it governs

Measured on the current tree (compute suite 215/25): the 22 failures that read a
declaration through a value are this rule's measurement —

| area | tests |
|---|---|
| `chain_merge` | 4 |
| `runtime_scalar_leaf` | 3 |
| `nested_config` | 3 |
| `cooperative_product` | 3 |
| `perspective_requirements` | 2 |
| `float_leaf` | 2 |
| `sibling_diamond` | 1 |
| `fan_out_merge` | 1 |
| `graph_argument` | 1 |
| `graph_runtime_scalar` | 1 |
| `graph_jit` | 1 |

The two documented parks (`compute::a_float_domain_is_permitted_at_every_position_the_walk_reaches`,
`compute::jit_inline_nested_function`) and `fuse_licence`'s stale address are not this
proposal's.

## Compatibility

- A decided instance answers as today: the class holds one value and the read returns it.
- An **open** field answers undecided or the class's later value instead of the
  declaration — which is exactly the `plrun` result's `.r` read the failing tests ask for.
- No syntax changes; the annotator, attribute and clone rules are untouched.
