# A native link library: a Rust generic behind a lichen generic

> Status: **proposed — ruled by the maintainer; the phases are not started.**
> §2 is measured; §4 is ruled (the instantiated code is dynamically loaded);
> §5's lichen surface is designed from existing mechanisms and is not measured.
> Points at: `crates/lichen-highlevel/src/native.rs` (the `$name` boundary),
> `crates/lichen-highlevel/src/checker/native_call.rs`, `crates/lichen-highlevel/src/program.rs`
> (`TypeValue`, `ValueType`, `Ctx`), `crates/lichen-lowlevel/src/function.rs`
> (the per-application clone), `crates/lichen-compute/src/compute.rs`
> (`LaunchOp`, the multi-operand native op), `lichen-std-native` (the reference
> plugin), `lang_compose_vocabulary!` (`crates/lichen-language/src/program.rs`),
> and the refinement contract in [operator-polymorphism](operator-polymorphism.md)
> §3.
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

## 2. The Rust half: an rlib is not a monomorphization barrier

**Measured** in a throwaway workspace (`spike-rlib/` under the session worktree,
never part of the tree): a generic library with a macro-generated multi-method
`Grid<T>` impl (`blur`, `sharpen`, … twelve loop-bodied methods), an **instances**
crate whose non-generic wrappers call the generic, and three binaries — one
calling the generic directly, one calling `#[inline(never)]` wrappers, one
calling `#[inline]` wrappers from a crate that contains nothing else.

| reading (release, `--emit=llvm-ir`) | generic called directly | `#[inline(never)]` shims | `#[inline]` shims |
|---|---|---|---|
| `define`s naming `generic_lib` in the binary's IR | **12** | **0** | **12** |
| total `define`s | 20 | 8 | 20 |
| IR size | 168 178 B | 38 379 B | 169 326 B |
| the call as seen in the binary | — | 12 external `declare`s named by the instances crate | the generic re-instantiated under the binary's own hash |
| binary size | 164 352 B | 164 352 B | — |
| rebuild of the binary alone (median of 3) | ~161 ms | ~139 ms | — |

What this pins:

1. **The generic body enters a crate exactly when that crate's own MIR names the
   generic.** Calling a non-generic wrapper names the wrapper, and the wrapper's
   body — which lives in the instances crate — already names a *specific*
   instance, so the downstream crate emits a call to it and instantiates nothing.
2. **`#[inline]` undoes the boundary.** With an inline wrapper the downstream
   optimizer inlines the wrapper, sees the generic call, and re-creates the
   instantiation under its own hash — 12 `define`s, exactly as calling the
   generic directly. A test crate that held both wrapper flavours first suggested
   the opposite: its `#[inline(never)]` half had already instantiated the same
   types upstream, so the inline half found the instance and referenced it.
   Isolated, the inline half re-instantiates.
3. **The binary does not shrink.** An rlib is static: both binaries are the same
   size, because linking copies the code either way. What the rlib shares is the
   **codegen**, not the bytes — and the IR-size row shows the ratio that scales
   (4.4× here, with a small body).
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

> A consumer calls only non-generic entry points whose bodies were compiled once,
> in the instance library. One generic call in a downstream crate's own MIR
> re-creates the instantiation there.

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

## 4. The shape, as ruled: a thin plugin and a dynamically loaded library

Two rulings fix it: the instantiated code is **dynamically loaded**, and the
plugin side is **native-plugin-shaped and depends on lichen**; neither enters the
lichen compiler (§7).

```
vbd-lichen      the plugin: the lichen wrapper source, the $vbd ops, the value
                conversion, the dispatch. Depends on lichen-highlevel/lowlevel,
                and is composed by the project's own host the way
                lichen-std-native is.
      │  loads at run time (libloading)
vbd-instances   the instantiated code: a dynamic library generated from the
                recorded set. Its ABI is plain data — scalars, pointer + length,
                opaque handles — so it can be loaded into a process that already
                has its own lichen.
      │
downstream      the project's Rust crates link the same library dynamically;
                the heavy code is one file on disk and one mapping per process,
                not one copy per downstream binary.
```

**Why dynamic and not static.** An rlib is an archive: linking it copies its
object code into every downstream binary. §2's two binaries are byte-for-byte the
same size with and without the instance rlib, so an rlib buys *codegen* time and
nothing else. A dynamic library is the artifact that buys *link* time and binary
size — the downstream link is a reference, and the code is mapped once.

**Why the split is forced, not chosen.** A dynamically loaded library cannot be
generic over the host's `Program`, and it must not carry a second copy of
lichen's runtime: a second `Registry` would freeze its own modules, and the
module identities lichen's artifacts carry would stop agreeing. So what crosses
the boundary is plain data, and the lichen value conversion stays in the plugin.

**How the plugin reaches it.** The library exports one `extern "C"` entry per
instance (plus a descriptor naming the lichen kind each entry serves); the plugin
resolves them with `libloading` — already in `Cargo.lock` at 0.8.9 — and its
`run` dispatches through the resolved table.

The stock `lichen-compiler` and the LSP never load `vbd-instances`; a program
that uses vbd's generics runs under the project's host. That is the same
arrangement `lichen-std-native` already has, and it is deliberate: the shipped
toolchain carries no project-specific instantiation set.

One question is still open inside this shape (§9): the **artifact kind** — a
`cdylib` with the plain C ABI above, or a Rust `dylib` that can carry lichen
types across the boundary but binds the library to the host's exact lichen build
and compiler.

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

`run` matches the type value it reads against the instance table and calls the
matching non-generic entry. The wrapper still states the types it knows
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

## 6. The set is derived; the mapping is declared

The maintainer ruled that the instantiation set is **generated by a recorder**,
not written by hand. That splits the problem cleanly:

- **Declared** is the *mapping* — which lichen kind maps to which Rust type
  (`Float => f32`, `Int => usize`) and which entry points the surface exposes.
  This is real knowledge the lichen program does not carry: `Float` does not say
  `f32`.
- **Derived** is the *set* — which of those (surface entry, kind) pairs the
  program actually uses. A recorder reads it off the built program, and the
  generator emits exactly those instantiations.

With the mapping a rule over kind markers, derivation can *invent* an instance: a
program that uses `Float` gets the `f32` instantiation with no human in the loop.
What it cannot invent is a *structural* type, because lichen's struct identity is
a per-build counter (`ValueType::type_id`); a struct instantiation needs a
declared name and that is the limit §8 records.

The recorder must see, per call, the **decided** type argument. Two candidate
seams, in ascending cost:

- **A whole-program scan after the check.** The wrapper's applies are in the
  module; an apply's operand is an array whose first element is the type
  argument's node, and a decided type constant is readable with
  `Module::node_value`. The scan needs the wrapper's function identities (the
  plugin can expose them) and nothing from the engine. It cannot see an
  instantiation that only becomes concrete at run time — which is the same
  boundary the refinement states.
- **A hook at the engine's per-application instantiation**
  (`instantiate_stamped`, `function.rs:270–344`), where `(template, argument
  type, result type)` is already in hand. It sees every application, including
  ones a module scan would have to reconstruct from nodes; it costs a read per
  apply on the hot path.

The first is the design's default; the second is the fallback if a scan cannot
recover an instantiation the program used.

## 7. The ruling

The maintainer decided, in this session:

1. **The instantiated code is dynamically loaded**, and the plugin side is
   native-plugin-shaped and depends on lichen; neither enters the lichen
   compiler. The artifact is therefore a dynamic library, not an rlib — the rlib
   is only the static form downstream Rust crates may also want (§4).
2. **The instantiation set is generated by a recorder**, not declared by hand
   (§6).
3. **Only lichen's own widths** are exposed: `Float = f32`, `Int = usize`. An
   `f64`/`i32` instantiation is out of scope, and no wider scalar is added to
   lichen for it.

## 8. Limits

- **Scalar widths** — ruled: lichen's own only (§7.3). `Float => f32` is the only
  lossless mapping, so a function generic over `f64` is unreachable from lichen.
- **The dynamic ABI is plain data.** Loading the library rules out generics,
  function values, and lichen's own value type across the boundary: scalars cross
  by value, arrays as pointer + length, and a struct as an opaque handle with
  exported accessors. A `dylib` can lift this at the price §9 records.
- **Struct instantiations.** A struct type's nominal `TypeId` is a per-build
  counter (`program.rs`, `TypeValue::TypeId`), so it cannot key an instance table
  across runs; a struct instance must be **declared by name** in the mapping, and
  the table must read that declaration, not the id.
- **A computed type argument** (`sample (Box Int) …`) reaches `run` as an
  unevaluated node; the op must evaluate it or refuse the spelling. `sample
  Float` is the supported form.
- **A generic over a function or closure.** The mapping names a concrete item; a
  lichen function value has no Rust type to pass. This is the dictionary limit —
  it is real, and it is not solved here.
- **Trait bounds.** Rust's bounds are not lichen's to state; a declared instance
  whose type does not satisfy them is a compile error in the generated crate,
  which is early enough but a Rust-shaped diagnostic.
- **The correspondence is trusted.** Nothing checks that the lichen kind and the
  Rust type in the mapping are the same value; the single mapping is the only
  place they meet, which is why there must be exactly one.

## 9. The phases, and what is not decided

- **P0 — the surface probe, including the dynamic boundary.** A throwaway native
  plugin with one hand-written instance table, a tiny hand-built dynamic library
  behind it, and the four §5 readings measured. No lichen change. This is the
  gate: if the refinement does not re-check per application, the closed-world
  half of the design changes before any generator is written; if `libloading`
  cannot give the plugin a stable table, the artifact kind changes.
- **P1 — the recorder.** The whole-program scan (§6) over a real vbd-shaped
  wrapper: the used (entry, kind) set, emitted as a plain list.
- **P2 — the builder.** From the recorded set and the declared mapping, write the
  dynamic library (one `extern "C"` entry per instance), build it with cargo, and
  cache it by a content key — the same shape as the kernel disk cache
  ([kernel-disk-cache](kernel-disk-cache.md)).
- **P3 — the host.** The project's host composes the plugin and names the library
  to load; the stock toolchain stays unchanged.

Not decided yet, and needed before P2:

- **The artifact kind.** A `cdylib` with the plain C ABI (§4) is the robust
  dynamic form — it loads into any process and cannot duplicate lichen's runtime,
  at the cost of marshalling every value. A Rust `dylib` can carry lichen types
  and functions across the boundary, but it binds the library to the host's exact
  lichen build and compiler, and downstream Rust crates can still instantiate
  generics from its metadata.
- **Who names the library.** The project's host installs its path (the shape
  above), or the language learns a `link` dependency kind in the `---…---` block
  so the library describes its own surface and the stock toolchain can load it.
  The second is a language feature with a loader and a manifest; the first needs
  no lichen change.
- **What the recorder's output artifact is**: a Rust source fragment, a JSON list,
  or a cache entry the builder reads.
