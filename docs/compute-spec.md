# Compute spec

> Status: current — the outward contract of the compute sub-language: the forms a `.lichen` program can write against `compute`,
> what those forms mean and type as, the hand-off to the kernel-IR layer, the named refusals, and how a backend is chosen.  Owned
> by: `crates/lichen-compute` — the wrapper `src/compute.lichen` and its host side `src/language.rs` and `src/compute.rs`.  Points
> at: `crates/lichen-compute/src/compute.lichen` (spelling), `crates/lichen-compute/src/language.rs` (the sub-language composition
> and the virtual `compute.lichen` import), `crates/lichen-compute/src/compute.rs` (the native core),
> `crates/lichen-compute/tests/lichen_compute/` and `crates/lichen-compute-gpu/tests/compute_gpu/` (the witnesses).

Vocabulary is the language spec's: `x : t` annotates, `?a` is undecided, `_` is a wildcard, `@any` never decides, `.name` is struct
access. Spelling belongs to the wrapper: this file states signatures and meanings, and a spelling change edits `compute.lichen`
alone while a meaning change edits both. Message text belongs to the code: a refusal is cited here by condition and site, never
quoted.

## 1 Surface

Genre: behaviour.

Every name the wrapper exports, with its signature and its meaning. The export list itself — one name per wrapper statement, the
registry pairing names with statements by position — is pinned by an `# Invariant` in `src/language.rs` (`WRAPPER_BINDINGS`); a name
not on that list is not part of this surface.

Functions:

- `jit` — `(f : 'I -> 'O) -> K` — compiles the function value `f` to a kernel; the kernel's `.I` and `.O` are `f`'s domain and
  codomain. A tuple codomain returns several values, one slot per leaf. Witness: `tests/lichen_compute/compute.rs`
  (`jit_then_launch_scalar`, `jit_tuple_codomain_returns_several_values`).

- `launch` — `(k : K, a : 'I) -> 'O` — runs the kernel on one decided argument; a tuple domain takes a tuple argument, one leaf per
  parameter; the answer checks at the kernel's `.O`. Witness: `examples/compute_jit.lichen`, `output = "6: Int"`;
  `tests/lichen_compute/compute.rs` (`jit_multi_arg_tuple`).

- `call` — `(k : K, a : 'I) -> 'O` — a cross-kernel call: the callee is emitted into the caller's own module, so both kernels share
  one launch set. Witness: `tests/lichen_compute/compute.rs` (`jit_cross_kernel_call`); `tests/compute_gpu/refusals.rs`
  (`a_cross_kernel_call_is_emitted_into_the_launch_sets_module`).

- `parallel` — `(f : 'P -> 'O, b : String, o : Type) -> K` — compiles the index function `f` to a parallel kernel for the backend
  `b` names; `b` is a required argument, and `o` names the outputs' struct type. Witness: `tests/lichen_compute/compute.rs`
  (`parallel_rejects_an_unknown_backend_name`).

- `plrun` — `(k : K, cfg : A) -> 'O` — runs the parallel kernel over `[0, cfg.n)` and binds the answer at the kernel's `.O`; the
  host reads a field by name. Witness: `tests/lichen_compute/compute.rs` (`parallel_range_write_is_map`).

- `range` — `(n : Int) -> Int` — the lane's own index `i`, `i ∈ [0, n)`. Witness: `tests/lichen_compute/compute.rs`
  (`parallel_range_write_is_map`).

- `row` — `(cfg : 'P) -> Int` — the lane's row: the index over the config's second scalar extent. Witness:
  `examples/kvcache.lichen`, `output = "([1.0, 2.0, 3.0, 4.0, 100.0, 200.0, 0.0, 0.0], [5.0, 6.0, 7.0, 8.0, 300.0, 400.0, 0.0,
  0.0]): Tuple"` — the program computes its write address from `row` and `col`.

- `col` — `(cfg : 'P) -> Int` — the lane's column: the index modulo the config's second scalar extent. Witness:
  `examples/kvcache.lichen` (the same `output =`).

- `read` — `(r : Read) -> ?b` — one buffer element of `r.from` at the index `r.at`; the element class is the class the buffer value
  carries. Witness: `tests/lichen_compute/compute.rs` (`parallel_read_input_buffer`).

- `write` — `(w : Write) -> Buf` — the value of one output position; the answer names the buffer written, at the buffer's own
  perspective. Witness: `tests/lichen_compute/compute.rs` (`a_statement_write_is_dropped_and_the_codowns_write_fills_the_output`).

- `write_atomic` — `(w : WriteAtomic) -> Buf` — one read-modify-write per lane: a scatter accumulates, and the order of the adds is
  the implementation's, never the program's. Witness: `# Invariant` at `crates/lichen-compute/src/compute.rs`
  (`ComputeOperator::WriteAtomic`).

- `collect` — `(b : Buf) -> Array` — the whole buffer as a lichen array. Witness: `tests/lichen_compute/compute.rs`
  (`parallel_write_only_collects_whole_buffer`).

- `buf` — `(p : BufParam) -> ?buf` — declares a slice: a name and a length, undecided on purpose, never an allocation. Witness: `# Invariant` at `crates/lichen-compute/src/compute.rs` (`ComputeOperator::Buf`).

- `graph` — `(f : 'a -> 'b) -> Graph` — records the dispatches `f` performs into a graph value; the operand is deep-evaluated first,
  because a graph is a bare function. Witness: `tests/lichen_compute/graph_jit.rs`
  (`a_graph_dispatches_what_the_program_dispatches_with_repeats_collapsed`).

- `graphrun` — `(g : Graph, args...) -> 'b` — runs the graph over the values the source function took; buffers are collected after
  the run. Witness: `tests/lichen_compute/graph_jit.rs` (`a_graph_recorded_for_one_backend_runs_on_another`).

- `mma` — `(m : Mma) -> Buf` — the cooperative matrix product `c' = a·b + c` for the backend `m.s` names, over the shape the backend
  advertises. Witness: `tests/compute_gpu/mma_shader.rs` (`the_cooperative_matrix_product_matches_the_host`).

Types:

- `K` — the kernel struct: `.native` the compiled artifact, `.I` the domain, `.O` the codomain; a kernel has no type marker — its
  struct is its type. Witness: `tests/lichen_compute/compute.rs` (`a_kernel_value_and_type_render_by_name`).

- `KT` — the kernel-type template: `.I` and `.O`, both `@any` until a kernel binds them; the type-level spelling of the same two
  fields. Witness: `tests/lichen_compute/compute.rs` (`a_kernel_value_and_type_render_by_name`).

- `Buf` — the buffer struct: `.native` the runtime payload, `.type` the element type, `.length` the element count. Witness: `# Invariant` at `crates/lichen-compute/src/compute.rs` (`ComputeValue::Buffer`).

- `BufParam` — the buffer-parameter struct: `.type` the element type, `.length` the element count; what `buf` and the `Buf` family
  take. Witness: `tests/lichen_compute/compute.rs` (`parallel_range_write_is_map`).

- `Read` — the read argument struct: `.from` the buffer, `.at` the index. Witness: `tests/lichen_compute/compute.rs`
  (`parallel_read_input_buffer`).

- `Write` — the write argument struct: `.to` the buffer, `.at` the index, `.value` the element. Witness:
  `tests/lichen_compute/compute.rs` (`parallel_range_write_is_map`).

- `WriteAtomic` — the atomic write argument struct: `.to`, `.at`, `.value`, as `Write`. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`ComputeOperator::WriteAtomic`).

- `Mma` — the product argument struct: `.s` the backend name, `.a`/`.b` the byte operands, `.c` the word accumulator. Witness:
  `tests/compute_gpu/mma_shader.rs` (`the_context_advertises_the_shape_it_can_run`).

## 2 Three usage shapes

Genre: behaviour.

- Scalar: compile a function, run it on one argument — `compute.launch (compute.jit ('x -> x + 1)) 5` answers `6 : Int`, typed `Int`
  at the root. One runnable witness: `examples/compute_jit.lichen`, `output = "6: Int"`; the same shape through
  `tests/lichen_compute/compute.rs` (`jit_then_launch_scalar`).

- Parallel: an index function over a named parameter struct (`.n` the extent, `.in` the input group; the outputs are the struct type
  `parallel` names), compiled for a named backend and run with `plrun`; the answer binds at that type and its fields are read by
  name. A one-kernel map and a two-kernel chain through buffers both take this shape. One runnable witness: `examples/silu.lichen`,
  `output = "[0.0, 0.73105794, -0.13447031, 7.0463777]: Array"`; the same binding through `tests/lichen_compute/compute.rs`
  (`parallel_range_write_is_map`).

- Graph: record a function's dispatches with `graph`, run the recording with `graphrun`, and collect after the run —
  `compute.collect (compute.graphrun built 3)` answers the source function's value. One runnable witness:
  `tests/lichen_compute/graph_jit.rs` (`a_graph_recorded_for_one_backend_runs_on_another`, answering `[2, 2, 2]` on the stub).

## 3 Lowlevel

Genre: behaviour.

- A kernel is a struct `.native`/`.I`/`.O`: `.native` holds the compiled artifact and the signature rides on the struct; the
  artifact is runtime-only, so the artifact format cannot serialize it. Witness: `# Invariant` at
  `crates/lichen-compute/src/language.rs` (`native_packages`); `tests/lichen_compute/compute.rs`
  (`a_kernel_value_and_type_render_by_name`).

- A buffer is a struct `.native`/`.type`/`.length`: `.native` the packed payload, and the element class rides on the value, read off
  the producing fragment at the ordinal — the payload cannot say it. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`ComputeValue::Buffer`); `tests/lichen_compute/buffer_payload.rs`
  (`a_collected_payload_is_relocated_when_its_block_is_released`).

- A parallel parameter is a named struct: `.n` the extent, `.in` the input group; the parameter no longer carries `.out` — the
  outputs are the codomain type the compile names, and a run's result binds at the kernel's `.O`. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`codomain_roles`, linking
  [kernel-buffer-declarations](notes/kernel-buffer-declarations.md)); `tests/lichen_compute/compute.rs`
  (`a_struct_parameter_kernel_runs_through_the_signature_carrying_wrapper`).

- A value's class is a fact of the value, not of the buffer or kernel that carries it: `Int` and `Float` cross where the signature
  declares the crossing — one `jit` kernel serves both classes — and a wrong class arriving where a leaf was declared is refused by
  name. Witness: `tests/lichen_compute/compute.rs` (`a_jit_kernel_crosses_the_two_classes_both_ways`;
  `a_float_argument_to_an_int_parameter_is_refused_by_class`).

- A write is the value of an output position, not a state threaded through values: a write the codomain does not reach is a dead
  expression, and dropping a dead expression is what pure-function semantics does. The write that fills the output is the one the
  returned value names. Witness: `tests/lichen_compute/compute.rs`
  (`a_statement_write_is_dropped_and_the_codowns_write_fills_the_output`); decision record [code-audit](notes/code-audit.md)
  (`P1-42`, `wontfix:D17`).

- An undecided `?a` on a buffer result stays a declaration: `buf` never evaluates to a payload — the graph keeps the node and the
  kernel's own walk decides which output it is — so a slice is a name and a length, not an allocation. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`ComputeOperator::Buf`).

## 4 One section per form

Genre: behaviour, with each form's refusals. One rule a line: form — meaning; each refusal names its condition and its site.

### 4.1 jit

- `jit f` compiles `f` to a kernel; the claim the compiler reads is stored beside the fragment — the value carries none. A closure
  over a constant compiles; the constant is baked into the body. Witness: `tests/lichen_compute/compute.rs`
  (`jit_then_launch_scalar`, `jit_closes_over_constant`).

- Refused: a compile that leaves the kernel-safe subset, or a parameter whose domain is undecided, is recorded under `compute.jit`;
  a non-function operand is the checker's own type error, not a refusal. Witness: `tests/lichen_compute/compute.rs`
  (`jit_rejects_a_non_function`).

### 4.2 launch

- `launch k a` runs the kernel on the decided argument and answers at `.O`. An undecided argument defers the launch rather than
  refusing it. Witness: `tests/lichen_compute/compute.rs` (`jit_then_launch_scalar`).

- Refused: an argument that is not a concrete `Int` tuple — a bare string, an element that is not a concrete `Int`, named to its
  path — or a run the wasm reports, is recorded under `compute.kernel_launch` in the run's own words. Witness:
  `tests/lichen_compute/compute.rs` (`a_refused_call_argument_element_says_why`, `a_refused_call_argument_shape_says_why`,
  `a_refused_call_run_says_why`).

### 4.3 call

- `call k a` is a cross-kernel call: the callee is emitted into the caller's module and runs on `a`. An operator applied inside the
  argument is routed through the frozen callee's body, so `k0 (x + 1)` emits as written. Witness: `tests/lichen_compute/compute.rs`
  (`jit_cross_kernel_call`, `jit_an_operator_inside_a_cross_kernel_argument_is_emitted`).

- Refused: a callee that returns more than one value is refused — a body reads the callee's result as a single value, and the kernel
  instruction set has no instruction that re-materialises a tuple. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute/body.rs` (`CROSS_KERNEL_RESULT_ARITY`); `tests/lichen_compute/compute.rs`
  (`jit_refuses_a_cross_kernel_call_to_a_multi_value_kernel`).

- Refused: a call inside a **parallel** body is refused by name under `compute.parallel`. The message names the shared fact — a
  kernel compiles from a template before any apply — and names the three program shapes that share it, deliberately not saying which
  one it is; the identical call in a scalar body runs. Witness: `tests/lichen_compute/refusal_over_an_answer.rs`
  (`a_refusal_over_an_input_buffer_is_reported_beside_an_answer_that_arrived` — the `compute.parallel` category refusing by name
  beside an answer that arrived); the refusal itself lives at `crates/lichen-compute/src/compute/body.rs` (`opaque`), recorded under
  `compute.parallel` at the parallel compile site; decision record [code-audit](notes/code-audit.md) (`P1-39`).

### 4.4 parallel

- `parallel f b o` compiles the index function `f` for the backend `b`; the choice is named and required — a program always says
  where its parallel kernels run — and it rides on the value, so two programs compiling one function for different backends share
  one id. The parameter's scalar leaves are positional: the first is the launch extent, the others arrive as real arguments beside
  the extent and the index. Witness: `# Invariant` at `crates/lichen-compute/src/compute.rs` (`Backend`);
  `tests/lichen_compute/compute.rs` (`parallel_rejects_an_unknown_backend_name`,
  `a_runtime_scalar_reaches_the_body_beside_the_extent`).

- A `Float` domain is permitted at every position the walk reaches; the leaf's own field declares the class it arrives in. Witness:
  `tests/lichen_compute/compute.rs` (`a_float_domain_is_permitted_at_every_position_the_walk_reaches`).

- Refused: a backend name that is neither `cpu` nor `gpu`, and a non-string argument alike, is reported with what was written and
  what is accepted — a typo is reported, never defaulted. Witness: `tests/lichen_compute/compute.rs`
  (`parallel_rejects_an_unknown_backend_name`, `parallel_rejects_a_non_string_backend`).

- Refused: a scalar leaf arriving as the class its own field did not declare is refused by name. Witness:
  `tests/lichen_compute/compute.rs` (`a_scalar_leaf_of_the_wrong_class_is_refused_by_name`).

### 4.5 plrun

- `plrun k cfg` runs the kernel over `[0, cfg.n)`; the answer binds at the kernel's `.O`. A run is identical sequential and
  parallel, and a run over the threshold covers every index. Witness: `tests/lichen_compute/compute.rs`
  (`parallel_range_write_is_map`, `a_multi_output_parallel_run_is_identical_sequential_and_parallel`,
  `a_parallel_run_over_the_threshold_covers_every_index`).

- Refused: a count past the bound — `1048576` — is refused by name, naming the count asked for and the limit; one refusal for the
  run, not one per evaluation. Witness: `tests/lichen_compute/compute.rs` (`a_refused_plrun_count_says_why`).

### 4.6 range

- `range n` is the lane's own index, `i ∈ [0, n)`; a body that is a fold may write inside the fold's step, and the step's store
  lands in the loop's step block. Witness: `tests/lichen_compute/compute.rs` (`parallel_range_write_is_map`);
  `tests/lichen_compute/loop_write.rs` (`a_body_that_is_a_fold_writes_inside_the_folds_step`).

### 4.7 row

- `row cfg` is the lane's row over the config's second scalar leaf; the leaf is read positionally — the extent is the first scalar
  leaf, and a body that names no coordinates leaves its other leaves ordinary runtime scalars. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`ComputeOperator::Row`); `examples/kvcache.lichen` (the `output =` in section 1).

### 4.8 col

- `col cfg` is the lane's column over the same second scalar leaf — the row's pair, differing in one instruction. Witness: `# Invariant` at `crates/lichen-compute/src/compute.rs` (`ComputeOperator::Col`); `examples/kvcache.lichen` (the `output =` in
  section 1).

- Refused: a config with one extent has no row or column to take and says so — one refusal naming both the extent it found and the
  primitive that wanted another. Witness: `tests/lichen_compute/lane_coordinates.rs` (`a_one_extent_config_is_refused_by_name`).

### 4.9 read

- `read r` answers the element of `r.from` at `r.at`, at the class the buffer carries. Witness: `tests/lichen_compute/compute.rs`
  (`parallel_read_input_buffer`).

- Refused: a read of a write's result — not an input buffer — is refused by name under `compute.parallel`. Witness:
  `tests/lichen_compute/compute.rs` (`a_read_of_a_writes_result_is_refused_by_name`).

- Refused: a read of a value one of the graph's own dispatches produced is refused at recording: what a dispatch produces stays on
  the device until the run ends, so the number is derived from the function's own argument, or the read waits until after the run.
  Witness: `tests/lichen_compute/graph_jit.rs` (`what_a_recorded_body_may_not_reach_for_is_refused_by_name`).

### 4.10 collect

- `collect b` materialises the whole buffer into a lichen array. Witness: `tests/lichen_compute/compute.rs`
  (`parallel_write_only_collects_whole_buffer`).

- Refused: a collect of a graph's own dispatch's value is refused at recording, with the read's repair: a `gpu` chain's results
  reach the host only once the run has ended, and the collect belongs there. Witness: `tests/lichen_compute/graph_jit.rs`
  (`what_a_recorded_body_may_not_reach_for_is_refused_by_name`).

### 4.11 write

- `write w` is the value of the output position `(w.to, w.at)`, not a state threaded through values: a write the codomain does not
  reach is a dead expression, and dropping a dead expression is what pure-function semantics does — a write that is only a
  statement leaves the buffer it named empty. Witness: `tests/lichen_compute/compute.rs`
  (`a_statement_write_is_dropped_and_the_codowns_write_fills_the_output`); decision record [code-audit](notes/code-audit.md)
  (`P1-42`, `wontfix:D17`).

- A write fills the output position of the buffer it names: a write naming another output's buffer exchanges the outputs. Witness:
  `tests/lichen_compute/compute.rs` (`a_write_to_another_output_than_its_position_exchanges_the_outputs`).

- Refused: a write inside a conditional's arm is refused — a kernel body's conditional emits both arms on every lane. Witness:
  `tests/lichen_compute/compute.rs` (`a_write_inside_a_conditional_is_refused`).

- Refused: a write whose `.to` names an input buffer is refused by name under `compute.parallel`, beside the answer the program's
  other statements arrived at. Witness: `tests/lichen_compute/refusal_over_an_answer.rs`
  (`a_refusal_over_an_input_buffer_is_reported_beside_an_answer_that_arrived`); program
  `tests/lichen_compute/inplace_write_refusal.lichen`.

- Refused: a write naming another write's result is refused — the buffer map's counterpart of the read's rule, neither side falling
  back to a position. Witness: `tests/lichen_compute/compute.rs` (`a_write_naming_a_writes_result_is_refused`).

### 4.12 write_atomic

- `write_atomic w` is one read-modify-write per lane: a scatter accumulates, the order of the adds is the implementation's — a
  `Float` accumulator's answer is a sum whose order is not the program's — and the synchronization the primitive provides is the
  add's atomicity and nothing beyond it. A body that scatters needs this primitive instead of `write`, whose contract is disjoint.
  Witness: `# Invariant` at `crates/lichen-compute/src/compute.rs` (`ComputeOperator::Write`, `ComputeOperator::WriteAtomic`).

### 4.13 buf

- `buf p` declares a slice of `p.length` elements of `p.type`; the operation never evaluates to a payload — the graph keeps the node
  and the kernel's own walk decides which output it is. Witness: `# Invariant` at `crates/lichen-compute/src/compute.rs`
  (`ComputeOperator::Buf`).

- Refused: an output position that holds a decided value rather than a buffer is refused — every codomain position holds a buffer.
  Witness: `tests/lichen_compute/compute.rs` (`an_output_position_that_is_not_a_buffer_is_refused`).

### 4.14 mma

- `mma m` is the cooperative matrix product `c' = a·b + c`: `a`/`b` hold signed bytes and `c` signed words, row-major, over the
  shape `COOPERATIVE_MATRIX_M` states; the backend is a required argument, as `parallel`'s — a program says where its product runs —
  and the two paths answer the same two's-complement words. Witness: `# Invariant` at `crates/lichen-compute/src/compute.rs`
  (`ComputeOperator::Mma`); `tests/compute_gpu/mma_shader.rs` (`the_cooperative_matrix_product_matches_the_host`).

- Refused: a select over a call's result and a constant of the other class is refused by name — the arms' classes must agree.
  Witness: `tests/compute_gpu/call_class.rs` (`a_select_over_a_calls_result_and_a_constant_of_the_other_class_is_refused_by_name`).

## 5 Hand-off

Genre: contract, not implementation.

What the surface guarantees the fragment carries. Each line is owned by `lichen-compute` and witnessed on its own tests; the IR
layer below appears as premise and link only, never restated.

- Roles: a parameter path is read by name at every depth — a group reached as `.in.g` answers what the same group reached as `.in`
  answers; the two are one ABI. Witness: `# Invariant` at `tests/lichen_compute/nested_role.rs`.

- Output count: the codomain's arity, fixed before any index runs; a write position counts outputs, not the combined buffer list.
  Witness: `tests/compute_gpu/refusals.rs` (`a_write_position_counts_outputs_not_the_combined_buffer_list`);
  `tests/lichen_compute/compute.rs` (`parallel_multi_output_writes_every_output_in_one_pass`).

- Identity: a kernel id is the fragment's content digest, so the id names the same fragment in every process; the wire form is the
  digest's `u64`, refused by name when it does not fit the host's `usize`. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`KERNEL_TAG`, `write_kernel_id`); `tests/lichen_compute/pack_placement.rs`
  (`a_matching_pack_fires_and_the_plan_it_states_is_what_lands` — a plan that steers the emit lands a fragment whose digest is not
  the floor's).

- The backend name rides on the value, not on the fragment: two programs compiling one function for different backends share one id,
  and a graph recorded for one backend runs on another with nothing contributed by the first recording. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`Backend`; `ComputeValue::ParKernel`); `tests/lichen_compute/graph_jit.rs`
  (`a_graph_recorded_for_one_backend_runs_on_another`).

- A cross-kernel call names its callee in the IR: the kernel id is the registry slot a call names, and a callee outside the launch
  set is refused by name. Witness: `# Invariant` at `crates/lichen-kernel-ir/src/lib.rs` (`KernelId`);
  `tests/compute_gpu/refusals.rs` (`a_callee_outside_the_launch_set_is_refused_by_name`).

Premises, stated once and owned elsewhere:

- The IR is narrower than any target's instruction set, the body is structured control flow, and the crate depends on nothing — the
  premise every fragment hands down; see [compute-jit-low-types](notes/compute-jit-low-types.md) and the `lichen-kernel-ir`
  rustdoc.

- The runtime's artifact layer reads IR alone and takes no `Module` — the seam this sub-language hands across; see
  [graph-fusion](notes/graph-fusion.md) §5 and the `lichen-runtime` rustdoc.

- Nothing in the workspace depends on the GPU backend crate, so a build that never dispatches a kernel never links a Vulkan loader;
  see [lichen-compute-gpu](notes/lichen-compute-gpu.md) and that crate's rustdoc.

Split test — the rule that decides whether a clause of this file ever moves into a second spec file:

- A second file is justified only when one of — **T1** a different reader (one who does not link `lichen-compute`: a kernel-IR
  author, a backend author); **T2** a different falsifier (only `cargo test -p lichen-kernel-ir` can falsify the clause); **T3** a
  different cadence (a change that touches no `lichen-compute` file can change the clause) — covers more than one third of the
  clauses.

- The names and owners are fixed in advance: `docs/kernel-ir-spec.md` (`lichen-kernel-ir`) and `docs/graph-ir-spec.md`
  (`lichen-graph-ir`); there is no third.

- Verdict: nothing is split this round. The two clauses nearest the seam — the IR is narrower than any target's instruction set; a
  kernel id lives in the IR so a cross-kernel call names its callee — each has exactly one governing site and stay in rustdoc and `# Invariant`.

## 6 Refusals and diagnostics

Genre: behaviour.

The categories the plugin records through the extension channel, each named for the path that refuses. This file names conditions
and sites; message text belongs to the code and is never copied here.

- `compute.kernel_launch` — the `launch` and `call` path; the two refuse on the same three facts: an argument that is not a
  parameter vector, a tuple element that is not a concrete `Int`, and a run that failed. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`KERNEL_LAUNCH_DIAGNOSTIC`); `tests/lichen_compute/compute.rs`
  (`a_refused_call_argument_element_says_why`, `a_refused_call_argument_shape_says_why`, `a_refused_call_run_says_why`).

- `compute.parallel` — the parallel path's own: body refusals (a conditional's arm write, a misplaced read or write, an unresolvable
  template binding) are recorded under the path the body compiles on, beside an answer that arrived if one did. Witness:
  `tests/lichen_compute/refusal_over_an_answer.rs` (`a_refusal_over_an_input_buffer_is_reported_beside_an_answer_that_arrived`);
  `tests/lichen_compute/compute.rs` (`a_write_inside_a_conditional_is_refused`).

- `compute.graph` — the recording and the run: a body that closes over a live buffer, a body that reaches for its own dispatch's
  value, a graph whose dispatches name two backends — each refused while the graph is built or run, and filed under the graph.
  Witness: `tests/lichen_compute/graph_jit.rs` (`a_buffer_the_body_closed_over_is_refused_by_the_capture`,
  `what_a_recorded_body_may_not_reach_for_is_refused_by_name`, `one_backend_for_the_whole_graph_is_checked_while_it_is_built`).

- `compute.graph.pass` — the pass channel: a run that installed a pass reports the pass's lines there, in stage order, and the
  recording stays what it was when no stage rewrote. Witness: `# Invariant` at `crates/lichen-compute/src/language.rs` (`wiring`);
  `tests/lichen_compute/pass_chain.rs` (`a_stage_sees_the_graph_the_stage_before_it_answered`).

- `compute.jit` and `compute.buffer` — the compile path's and the array-conversion path's own channels. Witness:
  `tests/lichen_compute/compute.rs` (`jit_rejects_a_non_function`); `tests/lichen_compute/buffer_payload.rs`
  (`a_collected_payload_is_relocated_when_its_block_is_released`).

Rules that hold across every category:

- No silent fall-back: a backend that declines is a named failure, and one backend for the whole graph is resolved while the graph
  is built — dropping one at run time would silently change what the program asked for. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`Backend`); `tests/lichen_compute/graph_jit.rs`
  (`one_backend_for_the_whole_graph_is_checked_while_it_is_built`).

- An unusable backend name is a named refusal: the message names what was written and what is accepted, because a string parameter
  has no enum to check against — a typo is reported rather than defaulted. Witness: `# Invariant` at
  `crates/lichen-compute/src/compute.rs` (`unknown_backend`); `tests/lichen_compute/compute.rs`
  (`parallel_rejects_an_unknown_backend_name`).

- A graph that dispatches nothing has no backend to run on and says what is absent rather than picking one. Witness:
  `tests/lichen_compute/graph_jit.rs` (`a_function_that_dispatches_nothing_has_no_backend_to_run_on`).

## 7 Not in this file

Genre: pointer.

What a program may write here, and the note that owns what it is charged:

- Two-dimensional launch and workgroup: a program may write `row`/`col` and give the config a second scalar leaf; what a shaped
  launch and a workgroup height charge is owned by [two-dimensional-launch](notes/two-dimensional-launch.md) and
  [two-dimensional-workgroup](notes/two-dimensional-workgroup.md).

- Shared memory and barriers: a program may stamp a staged-reduction claim on its fold; what shared memory and a barrier then charge
  the body is owned by [shared-memory-and-barriers](notes/shared-memory-and-barriers.md).

- Half precision: the element class `BFloat16` exists in the IR vocabulary; what a program may write in half precision is owned by
  [half-precision](notes/half-precision.md).

- The mma warp-uniform proof: a program names `mma` with a backend and the shape the backend advertises; the warp-uniform argument
  is owned by [gpu-mma](notes/gpu-mma.md).

- The wasm half: the `cpu` backend runs the same fragment in an interpreter; the hand-off and the control-flow contract are owned by
  [wasm-backend-handoff](notes/wasm-backend-handoff.md) and [wasm-control-flow](notes/wasm-control-flow.md).

- Placement packs: a run may install placement packs before anything compiles, and a pack that fires steers the emit; the pack file
  and its contract are owned by [graph-fusion](notes/graph-fusion.md).

- Fusion: a recorded graph may be rewritten by installed passes; what fusion claims and what a region merge charges are owned by
  [automatic-fusion](notes/automatic-fusion.md) and [graph-fusion](notes/graph-fusion.md).
