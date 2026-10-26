# Attributes and Perspectives

An annotation in lichen can carry more than a type. The suffixes after
`:` are the language's **attributes**, and each one is a slot with its own
meaning: the perspective `#`, the refinement `!`, and the doc `?`. There is
also a mark `~` that is not an attribute at all. This chapter is about
what each slot does, how they combine, and where they are checked.

## The annotation chain

An expression may be followed by any of these, in any order, each wrapping
everything to its left:

| suffix | slot | meaning |
|---|---|---|
| `e : T` | type | the value's type |
| `e # p` | perspective | how the value may be used in parallel |
| `e ! p` | refinement | a predicate on the value (Chapter 11) |
| `e ? d` | doc | a label for tools |
| `e ~ "text"` | mark | compile-time metadata, not an attribute |

So `e : T # p ? d ~ "m"` writes type, perspective, doc and mark on one
expression. They bind at the same precedence as `:`, so `e : T # p`
annotates the type and the perspective of the same expression, and
`1 # 4 + 2 # 6` is `(1 # 4) + (2 # 6)`.

## The perspective `#`

A perspective is a value attached to the value it annotates, and the
printer shows it between the value and its type:

```lichen
(5 # 8)
```

```text
5 # 8: Int
```

`8` is the perspective. What it means is a claim about *parallel
uniformity*: a value with perspective `n` is uniform over `n`-way
alignments. The exact reading is the GPU chapters' business (Chapter 17);
what matters here is that a perspective is a value, and that the language
checks one when a value meets a declaration.

### Declarations and checks

A function's parameter may declare a perspective it requires:

```lichen
f := (x # 8) => x
f (5 # 4)
```

```text
error: expected 8, found 4
  --> 2:1
   |
 2 | f (5 # 4)
   | ^
```

The parameter requires `8` and the argument carries `4`. A requirement
accepts an argument whose perspective is a **multiple** of it — a value
uniform over `n` is also uniform over any divisor of `n`:

```lichen
f := (x # 4) => x
f (5 # 8)
```

```text
5: Int
```

`4` divides `8`, so the call is accepted. `2` would reject `3`, and
accept `4`; `1` accepts everything, because it divides everything; and a
requirement of `0` — the "uniform everywhere" claim — accepts only an
argument that makes the same claim.

```lichen
f := (x # 0) => x
f (5 # 8)
```

```text
error: expected 0, found 8
  --> 2:1
   |
 2 | f (5 # 8)
   | ^
```

A parameter that declares **no** perspective declares no requirement, so
it accepts any argument — annotated or not:

```lichen
f := x => x
f (5 # 8)
```

```text
5: Int
```

### Replacing a perspective

An annotation that spells `#` replaces the perspective, and validates the
new one against the old: the new value is the *requirement*, the old one
the *provider*:

```lichen
((5 # 8) # 4)
```

```text
5 # 4: Int
```

`4` divides `8`, so the replacement holds and the value becomes `5 # 4`.
The other direction does not:

```lichen
((5 # 4) # 8)
```

```text
error: expected 8, found 4
  --> 1:3
   |
 1 | ((5 # 4) # 8)
   |   ^
```

The diagnostic points at the value whose perspective disagrees with the
new requirement.

### Combining

Two values that meet in a compound combine their perspectives by the
lattice's meet, which is the greatest common divisor: a tuple of a `# 4`
and a `# 6` element is uniform over `2`. The `0` of an unannotated value
is neutral in that meet (`gcd(n, 0) = n`), so a compound of unannotated
parts has no perspective. The perspective's placement in the attribute
schema is what makes this automatic; a program does not write the gcd.

## The doc `?`

The doc slot attaches a label. It never constrains: any doc is authorized
by any other, so a later `?` overrides an earlier one, and the printed
value carries it:

```lichen
D := _x => {.tag @any : Int}
((5 ? D{.tag 1}) ? D{.tag 2})
```

```text
5 ? tag = 2: Int
```

The printer shows the doc's struct as a compact `? field = value`. A doc
is for tools — the language server displays it (Chapter 18) — and the
language never branches on it. It contributes no apply-time constraint
slot, which is exactly why an override is allowed.

## The mark `~`

`e ~ "text"` is a **mark**: compile-time metadata pinned to the
expression. It is not an attribute: it never becomes a value, has no slot,
and does not appear in the printed answer:

```lichen
(1 ~ "note")
```

```text
1: Int
```

Several marks may pin one expression, and they accumulate. Today the only
reader is the interface claim of graph fusion (Chapter 17), which spells
its payload `claim:Interface(role,…)`; the payload's meaning belongs to
that reader, not to the language.

## What an annotation replaces

An annotation replaces **only the slots it spells**, and preserves the
rest. So a new perspective keeps an old doc, and a new doc keeps an old
perspective:

```lichen
D := _x => {.tag @any : Int}
((5 # 8 ? D{.tag 1}) # 4)
```

```text
5 # 4 ? tag = 1: Int
```

The perspective was replaced and validated; the doc was untouched. This
is what makes the slots independent: each one has its own relation, its
own check, and its own reader.

## Where attributes come from

The three attribute slots are a **schema**, and the language's own
extensions are entries in it. The perspective's order is divisibility and
its meet is `gcd`; the refinement's check is the assert channel; the doc's
relation authorizes everything. A plugin can add its own slot with its
own order and check — that is the compiler-plugin model of Chapter 19.
The schema is why the annotations compose: each suffix names a slot, and
the compiler walks them positionally.

## Summary

- An annotation may carry a type, a perspective `#`, a refinement `!`, and
  a doc `?`; a mark `~` is metadata, not an attribute.
- A perspective claims parallel uniformity; a requirement `q` accepts a
  provider `n` exactly when `q` divides `n`.
- An unannotated parameter declares no requirement and accepts any
  argument; a requirement of `0` accepts only a `0`.
- A new perspective replaces the old one after validating it, and an
  annotation replaces only the slots it spells.
- A compound's perspective is the meet (`gcd`) of its parts.
- A doc never constrains, is overridable, and prints as `? field = value`.
- A mark is compile-time metadata read by its consumer, never a value.
- The attributes are a schema that a plugin can extend.

## Exercises

1. Print `(5 # 8)` and explain both halves.
2. Write a function whose parameter requires `# 4`, and call it with `# 8`
   and with `# 3`. Which call holds?
3. Explain why `f := x => x; f (5 # 8)` holds, and what declaration would
   make it fail.
4. Check that `((5 # 8) # 4)` holds and `((5 # 4) # 8)` does not, and state
   the divisibility test each one performs.
5. Attach two docs to one value and show which one survives.
6. Write a value with a perspective, a doc, and a mark; say which of the
   three appears in the printed answer.
