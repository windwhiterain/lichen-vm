# A native link library: a Rust generic behind a lichen generic

> Status: **proposed — ruled by the maintainer; P0-a, P0-b, P0-c and the end-to-end
> proof are measured.**
> §2, §5 and P0-a are measured readings; §4 is ruled (a Rust `dylib` loaded
> through a `link` directive); §10 records the proof and the defect it found.
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

## 5. The lichen surface, measured

The surface the existing mechanisms express:

```lichen
-- the wrapper source the library's manifest carries
scalars = set(Float, Int)
vbd_kind = t => t @in scalars
sample = (t ! vbd_kind) => (x : t) => { r = $vbd_sample(t, x); r : t; r }
```

**Measured (P0-c)** with `crates/lichen-language/tests/language/link_probe.rs` — a
probe over a host composed exactly like `std_native.rs`, whose native op prints
what it sees and hands the argument back. The readings are the run's **rendered
type** (the value column is not a reading: the probe's op hands a node back
instead of emitting an operation, so the value is undecided — see the fourth
finding).

| program | reading |
|---|---|
| `p.sample Float 1.5` | `type: Float` — the result's type is the type argument |
| `p.sample Float 3` | refused: `expected Float, found Int` — the argument follows the type argument |
| `p.sample string "a"` | refused: `the refinement's predicate did not answer 1` — the closed world |
| `(p.sample Float 1.5, p.sample Int 3)` | `type: tuple{Float, Int}` — two instantiations coexist |
| `p.sample` alone | `type: ?a -> raw[?b, ?a] -> raw[?b, ?a]` — a dependent signature with open cells |
| the op's `build` | the type argument's node value is `None` |

Four findings, and they are the design's load-bearing readings:

1. **The refinement rides the parameter and re-checks per application.** The
   third row is refused while the first passes on the same wrapper: the closed
   world §6 is built on is a measured property, not a hope. The `type_of`
   monomorphism that could have pinned the first call does not apply here — the
   refinement is registered on the enclosing function and its clone re-checks.
2. **The result type is the type argument.** `r : t` gives the wrapper the
   dependent signature, and a wrong argument is refused where it is written.
3. **Two instantiations coexist in one program** — the apply clones per call, so
   a program using `Float` and `Int` gets both, independently.
4. **The native op cannot see the instantiation.** Built once per wrapper
   template, `build` reads `None` for the type argument. The dispatch is therefore
   necessarily at **run time**, from the operand the clone carries — which is what
   §4's table assumes and what fixes the shape of the op below.

The native op is the ordinary multi-operand shape:

```rust
// build: hand both raw values over as one operand, state no type
let operands = ctx.array_node(&[args[0].value, args[1].value]);
NativeApply { value: ctx.op_node(External { library, instance }, Some(operands)), decided: false }

// run: element 0 is the type value, element 1 the argument; select, convert, call
```

**The op must emit an operation, not hand a node back.** The probe returns the
argument's own value node as its `NativeApply.value`. The apply's clone does not
own that node, so the runtime read is undecided — and a tuple of two calls at one
type showed the value drifting between `(none, 2.5)` and `(none, none)` across
runs. An operator node over the operand is part of the body, is cloned with it,
and `run` computes the value — which is why the real op emits `External` rather
than a value.

`run` matches the type value it reads against the instance table and calls the
matching non-generic entry. The wrapper still states the types it knows
(`r : t`) — the same division of labour as `sort` (`compiler-plugin.md:117–133`).

One question the probe did not answer, and the design must: a *computed* type
argument (`sample (Box Int) …`) reaches `run` as an unevaluated node; the op must
either evaluate it or the manifest must refuse the spelling. `sample Float` is
the supported form.

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

- **P0-a — the `dylib` boundary: measured, it works.** A scratch probe
  (`target/link-probe`, not in the tree) built a `crate-type = ["dylib"]` crate
  against lichen and loaded it from a host built from the same source by the same
  rustc: the `#[unsafe(no_mangle)] extern "C"` entries resolved through
  `libloading`, took `*mut Module<LangProgram>` and `*const NodeId`, read a host
  node (`41`), read an f32 payload by bits (`0x3fc00000`), added a block and a
  node to the host's module, and the host read the new node back (`USize(99)`); a
  manifest string round-tripped. The dylib also produced an import library, so a
  Rust consumer can link it dynamically. This is the reading that says ruling 2 is
  viable on this toolchain.
- **P0-b — the `External` operator: done.** One core variant
  (`LowOperator::External { library, instance }`), a runtime table
  (`Module::install_external` plus a process-wide table, because a loaded library
  is process-wide and the definition pass runs before a module is handed back),
  the codec, and the low-type arm. Measured in-process
  (`crates/lichen-lowlevel/tests/basic/external.rs`): the entry is called with the
  operand's value and the module, `7 + 1` comes back as `8`, and a missing entry
  is a note plus an undecided node, never a panic.
- **P2 — the end-to-end proof: reached, with one defect (§10).** A generator wrote
  a Rust crate from `[(Float, f32), (Int, usize)]`, built it as a `dylib`, loaded
  it, and the lichen program `p.sample Float 1.5` answered `2.5: Float` and
  `p.sample Int 3` answered `4: Int`; `p.sample string "a"` was refused by the
  wrapper's refinement. The same run is **order-dependent**, and that is §10's
  finding.
- **P0-c — the lichen surface: measured, it works** (§5).
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

## 10. The end-to-end proof, and the defect it found

The proof (a scratch workspace under `target/link-e2e`, not in the tree) is the
whole chain in one run:

1. the generator writes `gen/src/lib.rs` from the declared set — a generic
   `sample_impl<T>` plus one `#[unsafe(no_mangle)] extern "C"` **entry per
   family**, whose match arms are the monomorphizations;
2. `cargo build --release -p gen` builds it as a `dylib`;
3. the host `libloading`s it and installs the entry in the process-wide table;
4. the same declaration emits the lichen wrapper
   (`sample = (t ! link_kind) => (x : t) => { r = $link_sample x; r : t; r }`);
5. the program runs: `p.sample Float 1.5` → `2.5: Float`, `p.sample Int 3` →
   `4: Int`, `p.sample string "a"` → refused by the refinement.

Two design answers the proof settled, and both are **refinements of the ruling**:

- **`External { library, instance }` names a family, not a monomorphization.** The
  native op's `build` cannot see the type argument (§5's fourth reading), and the
  type value at run time is an implementation detail of the wrapper. What the
  library dispatches on is the **operand's own representation** — `LowValue::Float`
  versus `LowValue::USize` — which is one match inside the generated crate. The
  monomorphizations live behind that match, which is exactly where source
  generation puts them, and the lichen side never names them.
- **The entry table is process-wide.** The definition pass evaluates the program
  before any module is handed to the caller, so a per-module table installed after
  the build is too late. `Module::install_external` still overrides per module;
  the process-wide table is the loader's path.

**The defect.** With a fresh store per program, the same binary run repeatedly
answers correctly on some runs and `none` on others. The failing cases call the op
node `NodeId(64v1)`, whose operand — the frozen wrapper's parameter value cell —
is **undecided**; the working cases call `NodeId(128v1)`, whose operand the call's
unify did write. The in-process probe is deterministic (7 → 8), so the operator is
not the source. The hypothesis the readings support is that the frozen apply mints
the parameter's value clone more than once, and the body's operand edge does not
reach the cell the instantiation writes; *which* record it reaches varies between
processes, which is what makes the failure intermittent rather than constant.

That is the next work item, and it is a **pre-existing engine defect** this proof
surfaces rather than causes. Two candidate fixes, in order of preference: one remap
for the whole instantiation (the signature clone seeds it and the body clone joins
it), or a first-class multi-argument operand on the native-op boundary that carries
the apply's own argument nodes instead of a value snapshot.
