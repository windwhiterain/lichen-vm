# Proposal v14: an unstated attribute position is open, not `Any`

> Status: proposal — needs the maintainer's ruling.
> Points at: `docs/language-spec.md` §Attribute, §Annotation, §Any;
> `crates/lichen-highlevel/src/checker.rs` (`slot_expr`),
> `crates/lichen-highlevel/src/checker/annotations.rs`
> (`adopt_stated_attributes`), `crates/lichen-lowlevel/src/equality.rs`
> (`clear_wildcard`), `docs/notes/attributes.md` §Perspective,
> `docs/notes/array-attribute-view.md`.

## What "`Any` erasure" is

The checker mints a wildcard into **every attribute position** of every expr it
builds: `slot_expr` pushes `self.any()` per position. So `r`'s reference expr is
`[value, type, Any]`.

The SPEC's `Any` never decides (§Any: "a guarantee that it will never be
decided"), and its unify exception "only merges if right side is undecided". A
statement that states an attribute therefore cannot reach the value:

| # | minimal lichen | expected | actual |
|---|---|---|---|
| 1 | `g := x => {r := x; r = (r # 32)}`<br>`(g 0) # 64` | refused, `expected 64, found 32` | **admitted** (`ok=true`) |
| 2 | `g := x => (x # 32)`<br>`(g 0) # 64` | refused, `expected 64, found 32` | refused (control) |

`# 32` lives in `Checker::state[e].attr` and in the annotation's *own* expr; the
value the statement names keeps the wildcard. "`Any` erasure" is the local repair
I landed (`62f11028`): clear the wildcard (`Module::clear_wildcard`, only a lone
one) so the statement's own unify writes the stated slot through. It makes example
1 refuse correctly — measured — but it buys that by adding a second writer of an
occupied slot, while `write_node_value` still states that occupied slots are only
rewritten by GC relocation, and by emptying a value the SPEC says never decides.
It also cannot tell "not stated here" from an explicit `@any`.

## The gap the proposal fills

The SPEC says what an annotation does (§Annotation: unified with, and replaced by,
the annotation expression) but never says **what an expression carries where it
states no attribute**. The implementation answers `Any`, which is also the
user-visible wildcard — one value, two meanings.

## Proposed SPEC text (v14)

1. §Attribute, after the expr encoding:

   > An attribute position an expression does not spell is an **open slot**: it
   > states nothing to a reader and is not the wildcard. A statement may decide it
   > (§Annotation); an open slot that no statement decides stays undecided.

2. §Any, appended to the exception:

   > `Any` is written only by `@any`. An unstated attribute position is **not**
   > `Any`: it is an open slot, so `@any`'s "never decided" rule does not protect
   > it.

3. §Annotation, appended:

   > A `unify` whose **expected** side is annotated states the actual's attribute:
   > each open slot of the actual takes the annotation's slot at the same canonical
   > position. A slot the actual already states is its own, and the unify compares
   > the two.

Example 1 then needs no erasure: the actual's open slot is undecided, and the
statement's ordinary unify *writes* the stated slot (the existing
undecided-against-decided arm). `clear_wildcard` is withdrawn.

## Rejected alternatives

- **Unstated position is the missing slot `0`.** `0` is the perspective's *stated*
  top, not "nothing stated", so it cannot carry an annotation (`r = (r # 32)` would
  compare `0` against `32` instead of taking it), and a caller's `0` would then be
  a provider a callee could observe. (The invariant in `array-attribute-view.md`
  that says a literal is `[1, Int, 0, 0, 0]` describes this rejected reading and
  should be corrected with the ruling.)
- **Keep `Any`; persist the return certificate in the artifact.** Conforms to the
  layering better, but the "which field" reference must still reach the call site:
  a clone turns a wildcard into a fresh hole that names no operation, and measured
  a call site then reads the argument's meet (`(w # 0)` prints `expected 0, found
  1` where `.to` states 32). Larger, and later.

## What the ruling must also settle

Whether a **template's** open slot may be bound by a call. The note in
`attributes.md` ("an undecided declaration would bind the template's own cell to
the first call's argument", `f (5 # 8)` then `f (5 # 2)` answering
`expected 8, found 2`) says the *template* must not be pinned; the per-call
**clone's** open slot may be, which is what makes a markerless parameter take the
caller's slot. The proposal needs the maintainer to say the open slot is a
per-clone fact and the template keeps its own state — otherwise this change
re-opens that measured regression.

## Impact if approved

No persisted shape changes (`ARTIFACT_FORMAT_VERSION` stays), `clear_wildcard` is
removed, `adopt_stated_attributes` disappears into the ordinary unify, and the
`stated_slot` hole-and-wildcard conflation raised in review is moot. Two checks
must be re-measured after the change: the markerless-parameter family
(`id (5 # 1)` answers `# 1`) and the template-pinning family (`f (5 # 8)` then
`f (5 # 2)` must not answer `expected 8, found 2`).
