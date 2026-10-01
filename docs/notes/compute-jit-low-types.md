# The compute JIT on low types

> Status: current — describes how `lichen-compute` consumes the low type
> layer. The **mechanism** (what a `LowShape` is, who maintains it, how it
> refines) is [lowlevel-low-types](lowlevel-low-types.md); this note records
> only the JIT's use of it, and the facts about kernel compilation that are
> its own.
> Points at: `crates/lichen-compute/src/compute.rs`.

The JIT compiles a **function template**, before any apply. That is the whole
reason the low type layer exists: the value graph cannot decide a template's
domain, because a template is never evaluated and an apply binds the clones.

## Seed, pass, read

`compile_fragment` does three things, in this order, and holds no local copy of
the answer at any point:

1. **Seed.** The parameter's type slot is decoded by the encoding authority
   (`lichen_highlevel::shape::low_type_of_slot`) and handed to
   `Module::seed_class_low_type` as a lower bound on the parameter's class. A
   parameter with no type cell seeds `Unknown`, which is the honest statement.
2. **Pass.** `Module::infer_template_low_types` runs the fixed-point pass over
   the template, so the body's own low types exist before any apply.
3. **Read.** The domain is `Module::low_type_of_node(param_value)` — the same
   read any other backend would do.

`compile_parallel_fragment` is the same chain, except its seed is the **host
ABI's** rather than a type fact: the parallel signature is `(n, index)` by
construction, whatever the lichen type says, so it is stated rather than
decoded.

A domain that is undecided after the pass is refused, with a message that says
what to write. A domain the wasm signature cannot express (a `string`, an
array) is refused with its own message. Neither falls back.

## The callee's domain is read, not inferred

A cross-kernel call's domain is a fact of the **callee's registration**: the
`LowShape` stored with the callee's fragment, not anything the call site says.
The bare `k x` apply states no signature at all (that is why the caller's own
parameter needs an annotation to have a decided domain), so the emitter reads
the callee's shape out of the kernel registry and flattens the argument to
match it — see
[lichen-compute §8](lichen-compute.md#multi-arity-cross-kernel-calls).

That read is **cloned out and the lock released before any emission**, because
emitting an argument can reach a further cross-kernel call, which locks the
same registry again, and the lock is not reentrant. The count is a correctness
requirement rather than a lowering choice: the callee is
`(i64) * flat_arity(domain)`, so a shorter argument would make the callee read
whatever follows it on the stack.

## The one node the value graph cannot decide: the parameter

The parameter's domain is read from its **type slot**, never inferred from body
usage, and that choice is a correctness requirement rather than a convenience:
`launch` passes **exactly the declared domain arity** (`compute.launch k (5, 3,
2)` → 3 args → the wasm function must take 3 parameters), and a body-usage
inference would under-read in the annotated-but-unused-element case and
mismatch the launch arg count.

## What the emitter still does by hand

The domain is a low type; the *body* is still a graph walk, and that is where
the remaining coupling lives (see
[checker-encoding-instability](checker-encoding-instability.md)):

- `flat_arity` counts the scalar leaves for the wasm signature, so a nested
  tuple `((Int, Int), Int)` flattens to three `i64` parameters.
- `emit_node` reads a parameter at an index path (`param_path` +
  `flatten_offset`) and maps it to its flattened wasm local — one mechanism for
  scalar, flat-tuple, and nested-tuple reads. It also looks through the
  checker's `value_of` extraction (`Index(pair, 0)`) to reach a value's actual
  computation, and lowers an `if c then a else b` (a 2-element array indexed by
  a computed selector) to a wasm `select`.
- `run_kernel` uses the dynamic `wasmi::Func::call` over an `&[i64]` argument
  vector; `Launch` flattens a (possibly nested) tuple argument into it.
- A kernel body may close over a module-level constant (graph-shared `USize`,
  lowered to `i64.const`).

## Out of scope

- **A backend that reads the body's low types.** v1 reads the domain only.
  The pass computes every body node's low type and stores it, which is what a
  follow-on emitter would read instead of walking operand chains.
- **Per-call-site specialization** — compiling a polymorphic template against
  a call site's argument. A fact about the language, not a mechanism gap: see
  §3 of the low-types note.
- **Higher-order kernels and recursion** — a body that applies itself or a
  helper; the checked graph is a `value_of` extraction over a shallow-array
  branch whose callee is an `Apply` of a function value, a shape that needs its
  own distinct handling before `Apply` can lower to a wasm `call`.
- **Arrays/tables as first-class kernel values** — only the tuple-of-scalars
  domain and scalar body operations are covered.
- **Static/imported kernel functions** (compute v1 rejects them; the
  freeze/persist plumbing is already in place for when they are supported).
- **SPIRV / GPU backend** — `wasm-encoder` + `wasmi` only today.
