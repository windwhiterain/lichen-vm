# Undecided reads are refused

> Status: **current.** Points at
> `crates/lichen-highlevel/src/checker/{structs,indexing}.rs` (the read gates
> and `DiagKind::UndecidedContainer`), `checker/lambda.rs` (a parameter
> annotation is decided before the body), and
> `crates/lichen-compute/src/compute.lichen` (the ABI this ruling changed).
>
> Supersedes the accepted tier of [eval-before-unify](eval-before-unify.md)
> §2.2/§2.4/§6.2 option 1: the per-apply **kind assert** for an undecided
> container is withdrawn. The measured facts in that note still stand; the
> ruling at the end of this note is what replaced its design.

## 1. The ruling

A **read** states its requirement from the container's type. When that type is
still an undecided cell where the read is checked, the read is **refused** with

```text
error: this <form> cannot be checked: the container's type is not decided
here — annotate the container's type
```

and the container must be annotated. The forms are the tuple slot `a(k)`, the
array element `a[i]`, the struct field `a.name`, the type accesses `X{n}` and
`X::a`, and (see §4) the table read and set membership. The forms whose
container's type is decided are unchanged.

The author's ruling: *a read on a container whose type is not decided cannot
be checked there, and the apply applies the value and type slots separately,
so a requirement deferred to the apply can leave a pair whose value and type
disagree; that is the same failure `raw` reports, and it is not a legitimate
answer.*

The same principle answered the type-level reads: **if a read needs an element
of a type that has no corresponding value, pass the element rather than the
type.** A type value whose structure is known but whose elements are not is a
generic; a generic is obtained by applying a generic function to the element.
That is why `compute.parallel` now takes the output type as an argument
(§5) instead of reading `I::out` off the parameter type.

## 2. Why the deferred tier was unsound (measured)

All measured through the CLI on `dev` before the ruling. Each program is
accepted (exit 0) and prints a pair whose value is not of its type:

```lichen
f = x => { a = x(0); a : Int; a }
f ("a", 2)                    -- "a": Int     value string, type Int
f = x => { a = x[0]; a : Int; a }
f ["a"]                       -- "a": Int
f = x => { a = x(0); a : string; a }
f (1, 2)                      -- 1: string
f = x => { a = x{0}; a : Int; a }
f (tuple{string, Int})        -- string: Int
f = x => { a = x::a; a : Int; a }
f (struct{.a string})         -- string: Int
f = x => { a = x.a; a : Int; a }
f (S(.a "s"))                 -- "s": Int     (S = struct{.a string})
f = x => { a = (g x)(0); a : Int; a }; g = x => ("s", 2); f 1
                              -- "s": Int     the container is a call result
```

The mechanism: the read's type is a lazy `Index` path, and an annotation or a
use decides it **before** the apply binds the container. The apply then binds
the container and the read's *value* is computed from the real container, but
the type slot keeps the earlier decision; the per-apply assert compared only
the container's **kind** (tuple/array/struct), never the element's or field's
type, so nothing reconciled the two slots. The table read did not have the
hole: `check_table_find` pins the container's whole type to
`table{?, ?}`, and the apply's argument unify checks the pinned value type
(measured: `f = x => { a = x <== 1; a : Int; a }; f (table(1 ==> "a"))` is
refused with `expected table{?a, Int}, found table{Int, string}`).

The access-caused `raw` answers were the same failure: before the ruling,
`addpair = p => p(0) + p(1); addpair (1, 2)` printed
`raw 3: raw[?a, ?b]`. After the ruling it is refused at `p(0)`.

## 3. What the checker does now

- `check_field` / `check_index` / `check_named_field` /
  `check_named_type_access` / `check_type_access` test the container's type
  for being decided (a type-function annotation is *deep-evaluated* first, so
  a written `(k : K _)` is decided), and refuse otherwise with
  `DiagKind::UndecidedContainer`, whose `field` slot names the read form.
- A refused read answers the checker's refused pair, so no graph is built
  behind the refusal.
- A lambda parameter's written type is deep-evaluated **before the body**
  (`checker/lambda.rs`), so a body's read on an annotated parameter is
  checked against a decided type. This is what makes annotations like
  `(k : compute.K _)` and `(s : tuple{Int, _})` usable without changing the
  annotation language.
- The `IsTupleType`/`IsArrayType` kind-assert registrations are gone with the
  tier; the `AssertSpelling::StructKind`/`TupleKind`/`ArrayKind` variants are
  registered nowhere now (removing them is a follow-up cleanup).

The rule is *language-level*: the spec says a read needs its container's type
decided at the read, and an undecided one must be annotated. The diagnostic's
position is the container.

## 4. What still defers, and why

`check_table_find` keeps its **pin**: the container's type is unified with a
fresh `table{key, value}` type whose value cell is the read's type, so the
apply's argument unify checks the *whole* container (measured in §2). The set
membership `@in` is a `TypeOperator::InDomain` check whose result type is
always `Int`; its container's requirement is checked where the set's type is
read, and a wrong-kind set is refused by the apply. Neither can print a
value/type disagreement.

If the author wants every read on an undecided container refused, including
these two, they are the same kind of gate and can be strictified; the reason
they were kept is that their *check is guaranteed at the apply* by a full
type unify, which is the property the ruling asks for. The reads in §3 did
not have that property.

A read's container is not always a parameter. A **call's result** is a cell,
and the read takes the call's type only when that type is decided. The
checker asks the engine for the call's **own instance** of the callee's
signature (the same per-call clone the runtime applies, with the parameter
check skipped) and binds the result cell to that instance's codomain, so
`(f 1).y` reads `f`'s return type directly. Measured: `S = struct{.y Int};
f = (x : Int) => S(.y x); (f 1).y` answers `1: Int` (previously refused), and
the old deferred-read exploit `f = x => { a = (g x)(0); a : Int; a }; g = x =>
("s", 2); f 1` is now refused at the read with `expected Int, found string`
instead of printing a string as `Int`.

The residual: a call to a function **still under check** is skipped (its
signature is not built, and a clone would disconnect a recursive result from
it), and an undecided callee's type is not a function type at all, so its
result stays a cell and the read is refused (`f = g => (g 1).y` still needs the
call-expression annotation). A call on a cyclic signature shows the instance's
open cells instead of the shared cut; the spec's §5 states that.

## 5. The compute ABI change

`compute.lichen` had two reads the ruling refuses:

- `parallel` read the output type off the raw body's parameter type
  (`I::out`). `I` is a hole, so the read was undecided; the element (the
  output type) is passed instead:

  ```lichen
  parallel = f => b: string => o: _ => {I = _; O = _; f: I -> O; (K _)(.native $parallel(f, b), .I I, .O o)}
  ```

  and every call site passes its output type (`compute.parallel (body ~
  "claim:…") "cpu" Out`). The examples and the book's kernel chapters were
  updated with the wrapper; the three compute tests whose programs build a
  kernel pass their output type the same way.
- `read` typed its result from the buffer value's `.element` field,
  `read = (x : (Read _)) => $read(x.from, x.at): x.from.element`. That field
  holds the element *type* as a value while its declared type is the element
  class (`Buf = T => struct{.native _, .element T}`), so the annotation's
  type expression has the class as its static type, not `Type`, and the
  strict annotation check refuses the value-as-type read. The wrapper now
  returns the native value and the **caller** states the element type at the
  call site — `(compute.read ((compute.Read _)(.from buf, .at i)) : Float)`.
  `write` keeps its shape. The class inference still reads the declared
  element class from `Buf`'s field, which is why `Buf` keeps `.element T`.

## 6. The measurements that remain

- `lichen-highlevel`: 136 tests pass; the four tests whose IR relied on a
  deferred read build a typed parameter now (`Writer::lam_typed`,
  `Writer::close_binding_typed`).
- `lichen-language`: 265 tests pass; the deferred-read expectations now assert
  the refusal, and the programs annotate their containers. The call-result
  bind is measured in §4; a cycle through a call result shows the instance's
  cells (the spec's §5).
- the example suite: 48/48, each printing the `output` it declares.
- the book: 193/193 blocks verified against the branch compiler (the block
  count after the raw section's removal).
- `lichen-compute`: 245 tests pass, and the serve suite is green; both take the
  new read/parallel ABI.
