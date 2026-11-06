# A function body is checked per application

> Status: **current, first slice landed.** Points at
> `crates/lichen-highlevel/src/checker/{structs,lambda}.rs` (the read's
> deferral, the call's reconciliation, `refuse_unresolved_reads`),
> `crates/lichen-language/tests/language/field_read_kinds.rs` (the pin), and
> [undecided-reads](undecided-reads.md) (the ruling this narrows).

## 1. The ruling this answers

[undecided-reads](undecided-reads.md) refuses a read on a container whose type
is undecided. The spec's field read is
`{type_of s = Struct; s.value[name_table[name]+2]}` — the kind and the value, no
concreteness requirement — and the spec's apply expansion is
`{clone; a = p_clone; body_clone}`: the argument is unified with the cloned
parameter **before** the body's reads are checked. Under that order the
measured exploit is not a mismatch but an error:

```lichen
S := {.a @any : string}
f := 'x => { a := x.a; a : Int; a }
f (S{.a "s"})          # expected Int, found string
```

The measured `"s": Int` was the implementation's **definition-time** body
check: the read's type cell was decided by the annotation before any
application bound the container, and the apply bound the value only
(`undecided-reads` §2).

## 2. The architecture, and what the first slice does

- The body is checked once at definition, per source expression; the read's
  type cell is per source expression.
- `check_app`'s type side (`apply_function_type`) clones the signature, binds
  the result cell and checks the parameter; it did not re-check the body.
- **Slice 1, landed: a direct formal's field read is deferred.** When the
  container is the current function's formal and its type is undecided,
  `check_named_field` builds the lazy read and records it instead of refusing.
  At each application of that function, after the type-side apply, `check_app`
  reads the field's declared type from the **argument's value** and unifies it
  with the read's type cell, so an annotation that disagrees is an error at the
  read's site.
- **An unresolved deferred read is still refused** at the end of the check with
  the undecided-read diagnostic: an unapplied function's body has no argument to
  decide the read, the case `undecided-reads` governs.

## 3. The pin

`field_read_kinds.rs::a_formal_field_read_is_checked_against_the_argument_at_the_call`
— the program above must be refused, and the message must name `Int` and
`string`. The existing `field_read_kinds` refusals still pass: the tuple, array
and table reads and the unresolved field read are unchanged.

## 4. What slice 1 does not do

- **The read's type cell is shared across applications.** A function applied to
  two structs whose field types differ unifies the same cell twice; the second
  application conflicts. Per-call cells — the spec's per-application clone of
  the body's check state — are slice 2.
- **Nested reads stay deferred and refused.** In `x.a.b`, the outer read's
  container is `x.a`, a read, not the formal; `direct` is false and the read is
  refused. Slice 2 resolves the chain in order.
- **The body beyond the read is still checked once.** The reconciliation covers
  the type the read's cell was bound to, not an independent re-check of every
  body expression per call.
- **The tuple, array and table reads** keep the definition-time refusal.

## 5. The slices

1. **landed** — a direct formal field read deferred to the call and reconciled
   against the argument's value; an unresolved read refused at the end.
2. Per-call cells for the deferred read (polymorphism): clone the read's type
   cell per application, as the signature clone does.
3. Nested formal reads (`x.a.b`) resolved in order.
4. The general per-application body check: the spec's `@clone` of the body's
   check state, if the body's own operations prove they need it.
5. The unapplied policy, recursion, `@loop`, Check mode, and the diagnostics.
