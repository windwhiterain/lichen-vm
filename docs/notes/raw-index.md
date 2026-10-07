# The raw index `X<e>`

> Status: current.
> Points at: `crates/lichen-language-lex` (`KwArray`), `crates/lichen-language-parser`
> (`Expr::RawIndex`, `array_type`), `crates/lichen-language/src/compile.rs`
> (`ExprKind::RawIndex`), `crates/lichen-highlevel/src/ir.rs`
> (`ExprKind::RawIndex`), `crates/lichen-highlevel/src/checker.rs` (`check_raw_index`),
> `crates/lichen-render/src/render.rs`.  Syntax and semantics are the spec's business:
> [language-spec.md §2, §2.1, §3](../language-spec.md).

The glued `<` postfix — the delimiter the array type used to occupy — reads component
`e` of an expression's **value** from a **tuple type value**: `X<e>`.  The array type is
spelled `array<T, n>` (keyword-led), so the two cannot be confused.

## What it does

`X<e>` lowers to the lowlevel `Index` and resolves the component structurally, bypassing
every *value* guard the typed forms apply:

- no array-type pinning (unlike `e[i]` → `Index`),
- no `IndexTarget` / shape-derived type (unlike `a(k)` → `Field`),
- no bounds assert.

The container's **type** is checked, though: it must be the tuple kind, stated once as a
unify.  A decided container is refused where it stands:

- a tuple *value* (`(1, 2)<0>`): its type is the tuple shape `<Int, Int>`, not the kind
  `TypeTuple` — `expected TypeTuple, found <Int, Int>`;
- a struct type value: its kind is `TypeStruct` and its components read by name (`X::a`)
  — `expected TypeTuple, found TypeStruct`;
- an array or an atomic type: `expected TypeTuple, found array<Int, 2>` / `found Int`;

An undecided container (a parameter, a call result) is **pinned** to the tuple kind,
so the apply that binds it refuses a wrong-kind actual per call.  It used to validate
nothing, which is why an array container reached the element read at run time.

So it reads a component of a *type-as-value*: `<Int, string><0>` is the `Int` type.  An
out-of-bounds subscript is still an evaluation error, recorded during the definition
pass, so the build is refused.

The result is the component's own pair — value slot element 0, type slot element 1.  The
**value** slot is read lazily; the **type** slot is computed when the read is checked, and
it is the component type value's *kind* (`Type` for `<Int, string><0>`, a `TypeStruct` kind
for a component that is itself a struct type), which is what lets a later check compare the
read's type instead of writing into it — the same rule, and the same measurement, as the
named sibling ([raw-field.md](raw-field.md#check-time-not-raw)).  A read the kind
requirement refused is **not built**: it carries the hole a refused definition carries,
because over a container that is not a tuple type value the two slot reads land on whatever
the container's type holds (measured: an array's length).  Every component of a
type-as-value is such a pair, which is what the form is for; a runtime array's element is
read with `e[i]`, and `X<e>` is not a second way to spell it.

The pin is what makes it usable generically: `f = k => k<0>` reads the first component of
whatever *tuple type* `k` is applied to, exactly the laziness the compute-wrapper field
reads rely on (see [compute-kernel-struct.md](compute-kernel-struct.md)).

## Why

Before this, the only way to read a component of a type value was through the wrapper's
own lazy `Field` reads, which require the container's *type* to be positional.  A
type-as-value's type is `Type` (or a kind), not a tuple/struct type, so the guarded reads
reject it.  `X<e>` is the escape hatch for generic code that inspects type values.
