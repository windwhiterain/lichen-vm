# A native link library: a Rust generic behind a lichen generic

> Status: **proposed — ruled by the maintainer; the phases are not started.**
> §2 is measured; §4 is ruled (a Rust `dylib` loaded through a `link` directive);
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
become lichen generics, the instantiations are **explicit**, one dynamic
library holds their object code, and downstream Rust crates link that library
instead of instantiating again.

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

The native-op boundary can express the whole wrapping surface. The table names the
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
| the wrapper's own types | the source the library's manifest carries | `std.lichen`, [compiler-plugin](compiler-plugin.md) |
| per-application instantiation of the wrapper | the apply clone walk | `function.rs:72`, `:159`, `:254` |
| the link library | a Rust dynamic library (§4) | `crate-type = ["dylib"]` |

Two facts make the boundary cheap. **A native op sees raw values, never a type**
(`native.rs:24–43`): the wrapper hands the type argument over as an ordinary
value, which is exactly what types already are. And **the operand is cloned per
application** (`function.rs:159`: *"a call site never monomorphises a polymorphic
template"*), so a wrapper written once dispatches per call on the argument the
caller actually passed.

## 4. The shape, as ruled: a self-describing `dylib` and a `link` directive

Three rulings fix it (§7): the instantiated code is **dynamically loaded**; it is
a **Rust `dylib` that depends on lichen** and shares the host's lichen; and a
**`link` directive** in the `---…---` block names it, so the toolchain loads it
instead of having it built in.

```
vbd-instances   the instantiated code: a Rust dylib generated from the recorded
                set. Depends on lichen (the same source as the host) and vbd-rs.
                Exports a manifest — the lichen wrapper source and the instance
                table — and one extern "C" entry per instance, taking the host's
                module and node ids.
      │  ---  vbd = link "…/libvbd_instances.so"  ---      (libloading)
the toolchain   reads the manifest, compiles the wrapper source as a virtual
                package whose $ops lower to External { library, instance }, and
                resolves the entries.
```

**Why dynamic and not static.** An rlib is an archive: linking it copies its
object code into every downstream binary. §2's two binaries are byte-for-byte the
same size with and without the instance rlib, so an rlib buys *codegen* time and
nothing else. A dynamic library is the artifact that buys *link* time and binary
size — the downstream link is a reference, and the code is mapped once.

**Why a `dylib` and not a `cdylib`.** The ruling crosses lichen values: an entry
takes `*mut Module<LangProgram>` and node ids, so an array or a struct needs no
marshalling and the library builds its results with lichen's own code. A `cdylib`
over plain data would be the sound form and is what this trades away; the unsafe
boundary it creates is the first price below.

**What stays thin.** The compiler-side operator is one core `External` variant
plus a runtime table; the wrapper source is compiled like any other embedded
native package; the instantiated code inside the library is non-generic. Nothing
about the library is compiled into the toolchain.

**How the loader works.** `libloading` (already in `Cargo.lock` at 0.8.9)
resolves the manifest entry and the per-instance entries; the manifest is the only
thing the compiler reads before checking a program, so the library describes its
own lichen surface.

The library is **loaded, never linked**: a program that names no `link` library
never opens one, and the toolchain binary carries no project's instances.

Three consequences the design must pay for, and they are not small:

**The ABI is an unsafe boundary.** A `dylib` can carry lichen types — the entry
points take `*mut Module<LangProgram>` and node ids — but rustc does not compare
the two sides' layouts. *Same lichen source and same rustc* is the only thing
that makes the pointers agree, and a mismatch is undefined behaviour, not a
diagnostic. What can be enforced:

- the manifest names a **build tag** (lichen revision, rustc version, the
  `LangProgram` identity) and the loader refuses anything else;
- every exported entry is `extern "C"` and must not unwind — a panic in the
  library aborts, it never crosses the boundary;
- the library holds no lichen global state (no `Registry` of its own): it reads
  and writes only the module it is handed, so it cannot freeze a second world.

The sound alternative — a `cdylib` over plain data — is what this design trades
away for crossing lichen values; §9's P0-a is the measurement that decides
whether the trade is viable on this toolchain.

**The operator vocabulary is compile-time.** `P::Operator` is an enum composed at
build time, so a library loaded at run time cannot add a variant to it. The
`link` path needs one core operator that names a library instance (e.g.
`LowOperator::External { library, instance }`) whose `run` dispatches through a
runtime table the loader fills. That is the language-layer cost of `link`, and it
is larger than the recorder's.

**The toolchain is pinned.** The library must be built by the same rustc as the
compiler that loads it, so a `link` directive is portable within one toolchain
and a rebuild across toolchains.

## 5. The lichen surface, and the readings it must be confirmed with

The surface the existing mechanisms express:

```lichen
-- the wrapper source the library's manifest carries
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

1. **The instantiated code is dynamically loaded** — a Rust `dylib`, not an rlib.
   An rlib copies its object code into every consumer (§2); a dylib shares one
   mapping.
2. **The library depends on lichen and shares the host's lichen** (the same lichen
   source, the same rustc); its entries take the host's module and node ids.
3. **A `link` directive names it** in the `---…---` block. The toolchain loads it
   at run time — never links it — and reads the surface the library describes.
4. **The instantiation set is generated by a recorder**, not declared by hand
   (§6).
5. **Only lichen's own widths** are exposed: `Float = f32`, `Int = usize`. An
   `f64`/`i32` instantiation is out of scope, and no wider scalar is added to
   lichen for it.

## 8. Limits

- **Scalar widths** — ruled: lichen's own only (§7.3). `Float => f32` is the only
  lossless mapping, so a function generic over `f64` is unreachable from lichen.
- **The dynamic ABI is unsafe and version-pinned.** A `dylib` carries lichen types
  across the boundary; only the same lichen source and the same rustc make the two
  sides' layouts agree, and a mismatch is undefined behaviour. The gate is the
  manifest's build tag and the no-unwind rule (§4).
- **A loaded library cannot extend `P::Operator`.** The vocabulary is a
  compile-time enum, so `link` needs one core `External` operator plus a runtime
  table (§4).
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

- **P0-a — the `dylib` boundary.** A hand-built `crate-type = ["dylib"]` crate
  built by the same rustc, loaded with `libloading`: does an `extern "C"` entry
  taking `*mut Module<LangProgram>` and node ids resolve, read the host's module,
  and hand back a node id the host can use? Does a manifest string read back? This
  is the measurement that says whether ruling 2 is viable on this toolchain.
- **P0-b — the `External` operator.** One core variant plus a runtime table, and a
  `$name` op that lowers to it and runs through `OperatorExt::run`.
- **P0-c — the lichen surface.** The four §5 readings: the refinement's
  per-application re-check, the decided type value, the dependent annotation, and
  the result type. No lichen change until these three pass; if one fails, the
  shape changes.
- **P1 — the recorder.** The whole-program scan (§6) over a real vbd-shaped
  wrapper: the used (entry, kind) set.
- **P2 — the builder.** From the recorded set and the declared mapping, write the
  dylib (manifest + one entry per instance), build it with cargo, and cache it by
  a content key — the same shape as the kernel disk cache
  ([kernel-disk-cache](kernel-disk-cache.md)).
- **P3 — `link` in the language layer.** The block directive, the loader, the
  manifest parser, the version gate, and the per-module registration of the
  library's ops.

Not decided yet:

- **The manifest encoding** — text, JSON, or a binary section — and how the
  wrapper source and the instance table share it.
- **What the recorder's output artifact is**: a Rust source fragment, a JSON list,
  or a cache entry the builder reads.
- **Whether the same source is also built as an rlib**, for Rust consumers that
  want static linking instead of the dylib.
