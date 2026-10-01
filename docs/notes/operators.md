# The computational operators

> Status: current.  The operator vocabulary a lichen program computes with, the
> five layers one operator lives in, and the three decisions that were made to
> add it: precedence, the two tokens that are both a bracket and a comparison,
> and the unsigned reading of `Int`.
> Points at: `crates/lichen-highlevel/src/ir.rs` (`BinOp`),
> `crates/lichen-highlevel/src/program.rs` (`TypeOperator`: the vocabulary, its
> codec tags, and its `run`),
> `crates/lichen-highlevel/src/checker/operators.rs` (the check),
> `crates/lichen-kernel-ir/src/lib.rs` (`KernelBin`),
> `crates/lichen-compute/src/compute.rs` (`kernel_bin`, the lowering, and the
> wasm emitter), `crates/lichen-compute-gpu/src/spirv.rs` (the SPIR-V emitter),
> and [language-spec](../language-spec.md) §2/§3 for the syntax and semantics a
> user sees.

## 1. The set, and what it was for

The language shipped with `+ - <= ==`: enough for the examples, not enough to
write an algorithm — no product, no division, no remainder, and only one
direction of the order comparisons. The set is now:

| family | operators | result |
|---|---|---|
| arithmetic | `+` `-` `*` `/` `%` | `Int` |
| comparison | `<` `>` `<=` `>=` `==` `!=` | `Int` (`0`/`1`) |
| bitwise | `&` `\|` `^` | `Int` |

There is no `Bool` value in the language, so a comparison yields the `0`/`1`
scalar and drives an `if` directly; that is also what makes `&`/`|`/`^` the
language's `and`/`or`/`xor` over two comparison results. `==`/`!=` are the
**generalized** equality — any two same-typed values, `Int`s or type values —
while every other operator is `Int`-only. See
[language-spec §3](../language-spec.md#3-semantics) for the semantics as a user
reads them; this note is about what it took to have them.

## 2. One operator, five layers

An operator spelled in source has to exist, consistently, in five places. They
are not a pipeline of transformations of one list — each is a separate enum, and
the joins are hand-written — so a new operator is exactly this checklist:

| # | layer | file | what to add |
|---|---|---|---|
| 1 | lexer | `lichen-language-lex` | a `TokenKind` + `RawToken` + `describe` + `raw_to_kind` arm, unless the operator reuses an existing token (`<` `>` `+` `-` …) |
| 2 | parser | `lichen-language-parser/src/ast.rs`, `parse.rs` | an `ast::BinOp` variant and a precedence level to fold it at |
| 3 | frontend | `lichen-language/src/compile.rs`, `resolve/content_key.rs` | the `ast::BinOp` → `ir::BinOp` mapping, and a **content-key discriminant** (the incremental resolver keys an expression's identity on it; append, never renumber) |
| 4 | IR, checker, interpreter | `lichen-highlevel/src/ir.rs`, `program.rs`, `checker/operators.rs` | an `ir::BinOp` variant, the `From<BinOp> for TypeOperator` arm, a `TypeOperator` variant with a **codec tag** (the persisted-artifact contract: append, never renumber), the `run` arm, `low_type`, and the check rule |
| 5 | lowered kernel IR + both backends | `lichen-kernel-ir/src/lib.rs`, `lichen-compute/src/compute.rs`, `lichen-compute-gpu/src/spirv.rs` | a `KernelBin` variant, the `kernel_bin` arm, the wasm instruction, and the SPIR-V opcode |

Two more lists are exhaustive matches over the token set and fail to compile
without their arm, which is deliberate: the language server's semantic-token
classification (`lichen-language-server/src/analysis.rs`) and the
`TypeOperator::ALL` round-trip the persist codec iterates.

The compiler's own error messages carry the vocabulary too: a `KernelBin` outside
what a target can express is refused **by name** rather than approximated (see
[compute-jit-low-types](compute-jit-low-types.md) for why the lowering is a
refusal and not a fallback).

## 3. Precedence

Loosest to tightest, which is the ladder `parse.rs` builds bottom-up:

```
=>            lambda
: # ?         annotations
->            function type
< > <= >= == !=   comparison
|                 bitwise or
^                 bitwise exclusive or
&                 bitwise and
+ -               sum
* / %             product
! e               prefix assert
application       juxtaposition
postfix, atoms
```

The bitwise trio keeps C's nesting (`&` in `^` in `|`) rather than flattening to
one level, because `a | b & c` reading as `(a | b) & c` is a footgun every reader
would have to learn. They sit **tighter than a comparison** — so `a & b == c` is
`(a & b) == c`, which is the reading every modern language uses and the opposite
of C's. Both choices are pinned by `operator_precedence_and_associativity` in
`crates/lichen-language/tests/pipeline.rs`.

A comparison is one level, left-associative, so `1 < 2 == 1` is `(1 < 2) == 1`.

## 4. Two tokens, two jobs: `<` and `>`

`<` and `>` were already the delimiters of every angle-bracket form —
`<a, b>` (tuple type), `struct<…>`, `array<T, n>`, and `X<e>` (raw index) — and
they are now comparisons as well. There is no mode flag and no whitespace rule
beyond the **Glue** marker that was already there (a delimiter glued to the
previous token is a postfix form); the two jobs are told apart by the shape
around the token:

- **`>`** — a `>` followed by an expression is the comparison; a `>` followed by
  anything else closes the bracket it is in. An infix operator needs a right
  operand, so this is not a heuristic: it is the grammar reading its own rule.
  Two implementation points, and the second is the one that is easy to get wrong:
  - The test is a **zero-width lookahead *at* the `>`**, not a peek at the token
    after it. A `filter` on the following token records its error one token
    further on, which wins chumsky's furthest-error rule — and then every
    malformed angle bracket (`<Int>`, `f <3>`) reports "at the end of the
    program" instead of at the bracket it is in. The parser's own unit test for
    that span is what caught it.
  - The check has to include **`Glue`**. A glued delimiter after the `>` belongs
    to the angle form, so it is not the start of an operand: without that,
    `struct<Int, Int>(1, 2)` parses as a comparison whose right operand is the
    tuple `(1, 2)`, and the instantiation stops existing. This is the case the
    first version of the guard missed, and two pre-existing tests failed on it.
- **`<`** — a **glued** `<` is still the raw index `X<e>`, and a spaced one is
  either the comparison or a tuple type at an operand position. Application binds
  tighter, so `f <Int, Type>` is still `f` applied to the tuple type, and
  `a < b` becomes the comparison **only because the application fails first**:
  the argument `<b>` is not a tuple type (one element), so the atom attempt is
  rewound and the comparator gets its chance. No existing form is displaced —
  what used to be a parse error is now a comparison.

Pinned by `comparisons_share_their_tokens_with_the_angle_bracket_forms`.

## 5. An `Int` is unsigned

`Int` is a machine-sized **unsigned** integer (`LowValue::USize`; `+`, `-` and
`*` wrap), so `/` and `%` are the unsigned division and remainder and the four
order comparisons are the unsigned ones. This is one decision with three
consequences, and all three are the same decision:

- **The interpreter** runs the operators on `usize` directly
  (`TypeOperator::run`).
- **The wasm backend** emits `I64DivU`/`I64RemU`/`I64LtU`… — not their `S`
  siblings, which agree with the interpreter for every value below `2^63` and
  disagree above it. `0 - 1` is how a program gets there, so the disagreement is
  reachable, silent, and would differ between a jitted and an unjitted run of the
  same source.
- **The SPIR-V backend** declares its value type `OpTypeInt 64 0` — **unsigned** —
  because SPIR-V *checks* this: `OpUDiv`, `OpUMod` and `OpULessThan` require
  operands whose signedness is `0`, so a module that declared the type signed and
  used the unsigned opcodes is invalid rather than merely wrong. `spirv-val`
  says so offline (`Expected unsigned int scalar or vector type as Result Type:
  UDiv`), which is how the emitter's type was found. The same change removed the
  `OpBitcast` the old index prologue needed: the widened invocation id *is* the
  value type now.

`0 - 1 > 1` is `1` and `(0 - 1) / 2` is `usize::MAX / 2`; both are pinned in
`an_int_is_unsigned_where_the_two_readings_differ`, and the device side in
`an_unsigned_reading_is_what_the_language_means`
(`crates/lichen-compute-gpu/tests/gpu_matches_cpu.rs`), whose inputs are `2^63`
and above — the values where a signed reading would take the other branch.

## 6. Division by zero

`x / 0` and `x % 0` have no value. The interpreter is the only layer that can say
so: `TypeOperator::run` records `operator.divide_by_zero` on the lowlevel's
general extension channel and answers the lazy marker, exactly as every other
refused computation in this language does — so the program reports an unbound
result and the recorded reason explains it. Checked by
`a_zero_divisor_is_recorded_rather_than_answered`.

A jitted kernel does **not** guard it, and that is a decision rather than an
oversight:

- wasm's integer division **traps**, which is a defined, reportable failure.
- SPIR-V's is undefined — no guard, no trap, whatever the device does.
- A guard would need a **branch**, and the GPU backend's straight-line body is
  what makes its uninitialised output buffers sound (every lane writes its own
  slot, so no slot is read before being written) and keeps the shader
  branch-free on its hottest path. A `select` cannot guard a division, because
  both operands are evaluated to reach it.
- Refusing a *literal* zero divisor at lowering time was considered and rejected:
  it catches only the case a reader can see anyway, while making the rule "zero
  is refused, except when it is not" — the dynamic case, which is the one that
  actually bites, would still be undefined. The subset's rule is stated once, in
  `KernelBin`'s documentation, and a program that divides by a computed value
  owns that value's range.

## 7. What is deliberately not here

- **Shifts (`<<`, `>>`).** A `>>` token swallows the adjacent closers of nested
  angle types — `array<array<Int, 2>, 3>` ends `3>>` — and a `logos` lexer cannot
  split it back; the fix is parser-level token splitting, which is a change to
  the frontend's token model rather than one more operator. Named rather than
  half-done.
- **Unary minus and unary bitwise-not.** `-x` would collide with the existing
  `-` (binary only) and `~x` with the array literal's shallow marker `~e`/`~2 e`.
  Both are spellable (`0 - x`, `x ^ (0 - 1)`) and neither is worth a grammar
  ambiguity.
- **`and` / `or` / `not` keywords.** `&`/`|` over `0`/`1` results *are* them; a
  second spelling of one operation would need its own precedence and its own
  short-circuit rule, and there is no `Bool` type for it to be a rule about.
- **`min`/`max`, floating point, and any operator a target cannot express
  identically.** The kernel-safe subset is the intersection of what the two
  backends compute the same way; an operator outside it would be a silent
  divergence between backends, not a feature.

## 8. Where the coverage is

- Interpreter: `the_extended_operator_set_evaluates` and
  `an_int_is_unsigned_where_the_two_readings_differ`
  (`crates/lichen-language/tests/pipeline.rs`).
- wasm JIT: `jit_lowers_the_arithmetic_comparison_and_bitwise_operators`
  (`crates/lichen-language/tests/compute.rs`) — one kernel per operator, so each
  is on the lowering path.
- SPIR-V, on a device: `arithmetic`, `predicates` and `unsigned_reading` in
  `crates/lichen-compute-gpu/tests/gpu_matches_cpu.rs`, each checked against a
  hand-written expectation **and** an independent CPU reading of the IR.
- The example programs `examples/operators.lichen` and `examples/gcd.lichen` are
  the user-visible statement of the set, and `tests/examples.rs` runs them.
