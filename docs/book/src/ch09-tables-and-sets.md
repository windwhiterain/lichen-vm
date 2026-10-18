# Tables and Sets

The previous chapter grouped values by name. This chapter adds two more
constant forms: the **table**, a lookup keyed by value, and the **set**, a
collection whose membership is a predicate. Both are constants — their
entries are fixed where they are written — and both are ordinary values
with their own type forms.

## Tables

A table is a constant mapping written with `==>` pairs:

```lichen
t = table(1 ==> 10, 2 ==> 20)
t
```

```text
table(1 ==> 10, 2 ==> 20): table{Int, Int}
```

The type is `table{K, V}`: the key and value **slots**, one type each.
Every entry's key shares the key slot and every value the value slot, so
a table is homogeneous in both directions. A table's keys and values may
be any values, including tuples, arrays, and type values:

```lichen
t = table([1, 2] ==> 10, [3, 4] ==> 20)
(t <== [1, 2], t <== [3, 4])
```

```text
(10, 20): tuple{Int, Int}
```

### Reading a table

`t <== k` is the table read. It answers the value stored under the key
equal to `k`:

```lichen
t = table((1, 2) ==> "pair", (3, 4) ==> "other")
(t <== (1, 2), t <== (3, 4))
```

```text
("pair", "other"): tuple{string, string}
```

Keys are compared by the language's own value equality: scalars, tuples
and arrays compare by **content**, a nested table compares by
**identity**, and a function compares by its **id**. So two content-equal
tuples are the same key, while two separately built tables are not:

```lichen
t = table(table() ==> 1)
t <== table()
```

```text
error: table lookup missed — no entry for this key
  --> 2:12
   |
 2 | t <== table()
   |            ^
```

The fresh `table()` is a different object from the stored one, so it is a
different key. A function, on the other hand, is found by its identity:

```lichen
f = x => x
t = table(f ==> 1)
t <== f
```

```text
1: Int
```

### A miss is a diagnostic

Looking up a key that is not in the table is refused where the read is
written:

```lichen
t = table(1 ==> 10, 2 ==> 20)
t <== 9
```

```text
error: table lookup missed — no entry for key 9
  --> 2:7
   |
 2 | t <== 9
   |       ^
```

The message names a scalar key, and says only `no entry for this key` when
the key has no printable scalar form (an array or a table, for example).
Either way the program does not get a default value: a table read is
total only over the keys the table stores.

### The table read's precedence

`<==` is an **infix operator**, not a postfix form, and it is the loosest
binary level — looser than a comparison, so its key is a whole comparison
expression:

```lichen
t = table(1 ==> 10, 2 ==> 20)
t <== 1 + 1
```

```text
20: Int
```

The key is `1 + 1`, and the read returns its entry. (A table read on an
empty table is a miss, and `table()` is a valid empty table whose type
carries open slots: `table(): table{?a, ?b}`.)

A table is a constant: its entries are evaluated where the table is
written, and a key that has no concrete value is refused rather than
stored. The read itself is an ordinary expression; it can appear wherever
a value can.

## Sets

A set is written `set(a, b, …)`:

```lichen
set(1, 2, 3)
```

```text
set(1, 2, 3): set{Int}
```

A set is homogeneous like an array: its members share one type. The type
is `set{E}`, and it has **no length** — a set's type says which values may
be members, not how many there are:

```lichen
(set(1), set(1, 2))
```

```text
(set(1), set(1, 2)): tuple{set{Int}, set{Int}}
```

The value of a set is its members; what makes it a set is its type. A set
is not an array: it is not indexable, and it cannot flow into an
`array{…}` parameter. `set()` is the empty set, with an open element slot.

### Membership

`a @in S` tests whether `a` is a member of the set `S`, and yields `1` or
`0` like a comparison:

```lichen
(1 @in set(1, 2), 4 @in set(1, 2))
```

```text
(1, 0): tuple{Int, Int}
```

The keyword sits at the comparison level, so `2 @in set(1, 2) == 1` is
`(2 @in set(1, 2)) == 1`, and its right operand must be a set:

```lichen
1 @in 5
```

```text
error: expected set{?a}, found Int
  --> 1:7
   |
 1 | 1 @in 5
   |       ^
```

Membership is the only read a set has. `set(1, 2)[0]` is refused with
`expected TypeArray, found SetType`: the bracket read requires an array
kind, and a set is a different kind.

### Sets of types are classes

The members of a set may be *types*, and that is how the language names a
group of types:

```lichen
Num = set(Int, Float)
(Int @in Num, Float @in Num, string @in Num)
```

```text
(1, 1, 0): tuple{Int, Int, Int}
```

`Num` is a set value whose type is `set{Type}`; membership compares the
type values by the class they denote, so a class imported from another
module matches too. This is the mechanism the core prelude uses to state
that the arithmetic operators accept `{Int, Float}`, and Chapter 11 is
about writing such contracts yourself.

## Summary

- `table(k ==> v, …)` is a constant lookup; its type is `table{K, V}`, one
  slot for keys and one for values, so both are homogeneous.
- `t <== k` reads the entry whose key equals `k`; a miss is a diagnostic and
  there is no default.
- Keys compare by content for scalars, tuples and arrays, by identity for
  tables, and by id for functions.
- `table()` is the empty table, with open slots.
- `<==` is the loosest binary operator, so its key is a whole comparison
  expression.
- `set(a, …)` is a homogeneous collection whose type `set{E}` has no length;
  `set()` is empty.
- `a @in S` is membership, yielding `0`/`1`, and it is the only read a set
  has.
- A set of type values is a class, and the class is what a contract is
  written over.

## Exercises

1. Build a table from three integers to their squares and read two entries.
2. Explain why `t <== table()` misses the entry stored under a `table()`
   built earlier.
3. Write `t <== 1 + 1` and say which key is looked up.
4. Write a set of the first three primes and test membership of `2`, `4`,
   and `Int`.
5. Explain why `set(1, 2)[0]` is refused, and what the diagnostic names.
6. Write `Num = set(Int, Float)` and test `Int`, `Float` and `string`
   against it.
