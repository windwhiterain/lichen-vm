# A native link library: a Rust generic behind a lichen generic

> Status: **proposed** — a design for wrapping a third-party generic Rust crate
> as a lichen generic, whose instantiations are explicit and whose object code is
> one rlib that downstream Rust crates link. §2 is measured; §5's lichen surface
> is designed from existing mechanisms and is not yet measured.
> Points at: `crates/lichen-highlevel/src/native.rs` (the `$name` boundary),
> `crates/lichen-highlevel/src/checker/native_call.rs`, `crates/lichen-highlevel/src/program.rs`
> (`TypeValue`, `ValueType`, `Ctx`), `crates/lichen-lowlevel/src/function.rs`
> (the per-application clone), `crates/lichen-compute/src/compute.rs`
> (`LaunchOp`, the multi-operand native op), `lichen-std-native` (the reference
> plugin), `crates/lichen-package/src/plugin.rs` (the compositor build), and the
> refinement contract in [operator-polymorphism](operator-polymorphism.md) §3.
> Companions: [compiler-plugin](compiler-plugin.md),
> [plugin-taxonomy](plugin-taxonomy.md), [packages](packages.md),
> [floating-point](floating-point.md) §4.2.

## 1. The question

A PCG project carries a large generic Rust library (vbd-rs). Rust monomorphizes
it in **every** downstream crate, so the object code cannot be shared through an
rlib the way a non-generic library's can, and each downstream crate pays the same
codegen again.

The request: wrap such a crate into lichen as a **link library** — its generics
become lichen generics, the instantiations are **explicit**, one `.rlib` holds
their object code, and downstream Rust crates link that rlib instead of
instantiating again.

This note answers three questions: what the Rust half can and cannot give (§2),
what lichen already has and what it would need (§3–§6), and which decisions are
the maintainer's (§7–§9).

## 2. The Rust half: an rlib is not a monomorphization barrier

**Measured** in a throwaway workspace (`spike-rlib/` under the session worktree,
never part of the tree): a generic library with a macro-generated multi-method
`Grid<T>` impl (`blur`, `sharpen`, … twelve loop-bodied methods), an **instances**
crate whose non-generic wrappers call the generic for `f32`, and three binaries —
one calling the generic directly, one calling `#[inline(never)]` wrappers, one
calling `#[inline]` wrappers from a crate that contains nothing else.

| reading (release, `--emit=llvm-ir`) | generic called directly | `#[inline(never)]` shims | `#[inline]` shims |
|---|---|---|---|
| `define`s naming `generic_lib` in the binary's IR | **12** | **0** | **12** |
| total `define`s | 20 | 8 | 20 |
| IR size | 168 178 B | 38 379 B | 169 326 B |
| the wrapper call as seen in the binary | — | 12 external `declare`s named by the instances crate | the generic re-instantiated under the binary's own hash |
| binary size | 164 352 B | 164 352 B | — |
| rebuild of the binary alone (median of 3) | ~161 ms | ~139 ms | — |

What this pins, and what the rest of the note is built on:

1. **The generic body enters a crate exactly when that crate's own MIR names the
   generic.** Calling a non-generic wrapper names the wrapper, and the wrapper's
   body — which lives in the instances crate — already names a *specific*
   instance, so the downstream crate emits a call to it and instantiates nothing.
2. **`#[inline]` undoes the boundary; `#[inline(never)]` is what makes it
   load-bearing.** With an inline wrapper the downstream optimizer inlines the
   wrapper, sees the generic call, and re-creates the instantiation under its own
   hash — 12 `define`s, the same as calling the generic directly. The earlier
   reading that suggested otherwise was an artefact of a test crate that held
   both wrapper flavours: the `#[inline(never)]` half had already instantiated
   the same types upstream, so the inline half found the instance and referenced
   it. Isolated, the inline half re-instantiates.
3. **The binary does not shrink.** An rlib is static: both binaries are the same
   size, because linking copies the code either way. What the rlib shares is the
   **codegen**, not the bytes. That is the right claim to make to the project:
   the win is compile time, and it scales with the generic IR volume, which the
   IR-size row above shows is 4.4× here.
4. **`-Zshare-generics` is not the mechanism and not a substitute.** The
   instances crate's instance is referenced because the wrapper's MIR names it,
   not because the compiler shares monomorphizations; and the unstable flag is
   nightly, on by default only for non-optimised builds, and only shares
   instantiations the defining crate has already made.
5. **The rlib is ordinary.** No new artifact type, no lichen cache entry: the
   instances crate is a normal cargo library and its `.rlib` is linked the normal
   way. Nothing in `crates/` builds or consumes an rlib today (a full-repo grep
   finds no `crate-type`, no `rustc` invocation, no `-Zshare-generics`).

The contract that follows is about **where the generic is named**, not about what
is linked:

> A downstream crate links the instance rlib and calls only non-generic entry
> points whose bodies live in that rlib. One generic call in the downstream
> crate's own MIR re-creates the instantiation there.

## 3. The lichen half: the pieces already exist

A native plugin can express the whole wrapping surface. The table names the
mechanism each requirement lands on; none of them is new.

| requirement | mechanism | where |
|---|---|---|
| a lichen value that *is* a type | types are first-class values (`Type : Type`) | [overview](overview.md), `examples/struct_generic.lichen` |
| a generic over a type, applied explicitly | `Box = t => struct{.f t}`; `sample Float` | `examples/dependent_type.lichen` |
| a parameter whose type *is* the type argument | a dependent annotation `(x : t)` | `type_of = x => {t = _; x: t; t}` (`lichen-std/_.lichen`) |
| a closed set of admitted type arguments | a **refinement parameter** `(t ! p)`, re-checked per application | [operator-polymorphism](operator-polymorphism.md) §3.4, `crates/lichen-highlevel/src/refinement.rs` |
| the closed set itself | a `set` value: `set(Float, Int)` | `crates/lichen-highlevel/src/set.rs` |
| a native op over several raw values | `ctx.array_node(&[…])` + `ctx.op_node` | `LaunchOp::build`, `compute.rs:7223` |
| selecting a Rust implementation by a *value* | `OperatorExt::run` reads the operand | `SortOp::run`, `lichen-std-native/src/lib.rs` |
| the wrapper's own types | the plugin's embedded lichen source | `std.lichen`, [compiler-plugin](compiler-plugin.md) |
| per-application instantiation of the wrapper | the apply clone walk | `function.rs:72`, `:159`, `:254` |
| the link library | an ordinary Rust crate | — (§2's instances crate) |

Two facts make the boundary cheap. **A native op sees raw values, never a type**
(`native.rs:24–43`): the wrapper hands the type argument over as an ordinary
value, which is exactly what types already are. And **the operand is cloned per
application** (`function.rs:159`: *"a call site never monomorphises a polymorphic
template"*), so a wrapper written once dispatches per call on the argument the
caller actually passed.

## 4. The shape: one declaration, three expansions

```
vbd-spec            the one declaration: lichen type ↔ Rust type ↔ entry point
   ├── vbd-instances   non-generic monomorphic wrappers              → the rlib (§2)
   ├── vbd-lichen      the wrapper source + $vbd ops + dispatch      → a native plugin
   └── downstream      Rust crates depend on vbd-instances and call its wrappers
```

The heavy code is instantiated **outside** the `P`-generic plugin: `vbd-lichen`
is thin (the surface, the set, the dispatch) and monomorphises into the host like
any other plugin, while `vbd-instances` is plain Rust with no lichen dependency
at all. That split is what keeps the rlib shareable: nothing lichen-specific is in
it.

The three expansions come from **one** declaration so the lichen type, the Rust
type, and the entry point cannot drift apart. A `macro_rules!` in `vbd-spec`,
invoked once per consuming crate, is enough; a proc macro is not required.

## 5. The lichen surface, and the readings it must be confirmed with

The surface the existing mechanisms express:

```lichen
-- vbd.lichen, the plugin's embedded source
scalars = set(Float, Int)
vbd_kind = t => t @in scalars
sample = (t ! vbd_kind) => (x : t) => { r = $vbd_sample(t, x); r : t; r }
```

Expected readings — **none of them measured yet**, and the first work item is a
probe that measures exactly these:

| program | expected |
|---|---|
| `sample Float 1.5` | `1.5 : Float` — the result's type is the type argument |
| `sample Float 3` | refused by `x : t`: `expected Float, found Int` |
| `sample string "a"` | refused by the refinement: `string` is not in `set(Float, Int)` |
| `(sample Float 1.5, sample Float 3.5)` | both instantiate independently: the apply clones per call |
| `sample` alone | a dependent function: `t` is a value of type `Type`, and both the second parameter and the result take their type from that value |

The native op is the ordinary multi-operand shape:

```rust
// build: hand both raw values over as one operand, state no type
let operands = ctx.array_node(&[args[0].value, args[1].value]);
NativeApply { value: ctx.op_node(VbdOp::Sample, Some(operands)), decided: false }

// run: element 0 is the type value, element 1 the argument; select, convert, call
```

`run` matches the type value it reads against the spec's table and calls the
matching `vbd_instances::*` entry. The wrapper still states the types it knows
(`r : t`) — the same division of labour as `sort` (`compiler-plugin.md:117–133`).

Two things could make the probe fail, and both are known:

- **The refinement on a type-valued parameter.** §3.4 of
  [operator-polymorphism](operator-polymorphism.md) measures a class refinement
  that rides a parameter and re-checks per application, which is the mechanism
  this wrapper needs. The nearest measured counterexample to "a body cell tied to
  the parameter is re-instantiated" is the `type_of` monomorphism
  ([type-of-in-std](type-of-in-std.md), open): a *read* built from an annotation
  is not among the nodes the clone remaps, so the first call pins. Whether the
  refinement's assert is on the right side of that line is exactly what the probe
  must read.
- **A decided type value at run time.** `run` reads the operand; for `sample
  Float` the argument is a decided type constant. For a *computed* type argument
  (`sample (Box Int) …`) the operand is an unevaluated node, and the op must
  either evaluate it or refuse the spelling.

## 6. Why the instantiation is a declaration, and what lichen adds

The set cannot be *derived* from a lichen program: the lichen type `Float` does
not say `f32`, and it is the Rust type that decides which instantiation is
needed. So the set is **declared** in the spec; that is what "explicit
instantiation" means here.

lichen's contribution is the **closure check**: the refinement makes every use's
type argument one of the declared instances, at check time, per application. What
that removes is the failure the boundary would otherwise have — a call whose type
argument has no instance, discovered late and far from the call site.

If a program should be able to *add* an instance, the missing piece is a
**recorder** of the instantiations a whole program used, not a new boundary. Two
candidate seams exist: the checker's per-apply `Build` state (the apply edges are
already recorded) and the engine's per-application `instantiate_stamped`
(`function.rs:270–344`), where `(template, argument type, result type)` is
already in hand and a hook can only read. What that buys is pruning (build only
the used instances) and an on-demand add; it cannot invent the Rust type, so a
human or a rule still names it. It is not needed for the boundary to work.

## 7. Decisions to take

1. **The spec: hand-written or generated.** Hand-written (macro) needs no
   toolchain change and is checkable today; generated needs the recorder and a
   build step, and still needs a declaration for the Rust type.
2. **The link shape: plugin links the rlib, or a runtime table.** The plugin
   depending on `vbd-instances` keeps the existing composition model (a compiler
   rebuilt from its plugin set); a table installed at run time avoids the rebuild
   but adds a registration the plugin's `run` must consult, and the host must
   perform it.
3. **`lichen run` and native generics.** With the plugin linking the rlib, the
   CLI executes them like any other plugin, at the cost of a compositor rebuild
   when the plugin set changes. A `cdylib` + loader is the alternative; it is not
   statically linkable by downstream Rust crates, so it is the *second* artifact,
   not the first.
4. **Scalar width.** lichen's `Int` is `usize` and `Float` is `f32`
   (`docs/language-spec.md` §3, [floating-point](floating-point.md) §4.2). An
   `f64`/`i32` instantiation therefore has no lichen type: either the wrapper
   exposes only the widths lichen has, or lichen gains a wider scalar.

## 8. Limits

- **Scalar widths** — §7.4. `Float => f32` is the only lossless mapping today.
- **Struct instantiations.** A struct type's nominal `TypeId` is a per-build
  counter (`program.rs`, `TypeValue::TypeId`), so it cannot key an instance table
  across runs; a struct instance must be **declared by name** in the spec, and
  `run` must read the argument's declared type, not its id.
- **A computed type argument** (`sample (Box Int) …`) reaches `run` as an
  unevaluated node; the op must evaluate it or the spec must refuse it. `sample
  Float` is the supported spelling.
- **A generic over a function or closure.** The spec names a concrete item; a
  lichen function value has no Rust type to pass. This is the dictionary limit —
  it is real, and it is not solved here.
- **Trait bounds.** Rust's bounds are not lichen's to state; a declared instance
  whose type does not satisfy them is a `vbd-instances` compile error, which is
  early enough but a Rust-shaped diagnostic.
- **The correspondence is trusted.** Nothing checks that the lichen type in the
  spec and the Rust type are the same value; the single declaration is the only
  place they meet, which is why there must be exactly one.

## 9. What is not decided

The four §7 items. This note records the design and the one measured half; the
maintainer's ruling on those items is what turns the rest into work.
