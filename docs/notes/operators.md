# The computational operators

> Status: current — the operator vocabulary a lichen program computes with, the
> five layers one operator lives in, the decisions behind it (precedence, the
> unsigned reading of `Int`, and the `<`/`>` reading the retired angle forms
> forced), the seven prefix keywords (the two class
> crossings and the five scalar operators `max`, `min`, `exp`, `sqrt`, `floor`),
> and the editor grammar's own reading of the same set.
> Points at: `crates/lichen-highlevel/src/ir.rs` (`BinOp`, `PrefixOp`),
> `crates/lichen-highlevel/src/program.rs` (`TypeOperator`: the vocabulary, its
> codec tags, and its `run`),
> `crates/lichen-highlevel/src/checker/operators.rs` (the checks,
> `arithmetic_class` and `check_prefix`),
> `crates/lichen-kernel-ir/src/lib.rs` (`KernelBin`, `KernelInstr`),
> `crates/lichen-kernel-ir/src/unary.rs` (`exp`'s one definition),
> `crates/lichen-compute/src/compute.rs` (`kernel_bin`, `conv_of`, `unary_class`,
> `emit_convert`, and the wasm emitter), `crates/lichen-compute-gpu/src/spirv.rs`
> (the SPIR-V emitter),
> `tree-sitter-lichen/grammar.js` (the editor grammar, §10), and
> [language-spec](../language-spec.md) §2/§3 for the syntax and semantics a
> user sees.

## 1. The set, and what it was for

The language shipped with `+ - <= ==`: enough for the examples, not enough to
write an algorithm — no product, no division, no remainder, and only one
direction of the order comparisons. The set is now:

| family | operators | result |
|---|---|---|
| arithmetic | `+` `-` `*` `/` `%` | the operands' class |
| comparison | `<` `>` `<=` `>=` `==` `!=` | `Int` (`0`/`1`) |
| bitwise | `&` `\|` `^` | `Int` |
| scalar | `max a b`, `min a b` (prefix keywords) | the operands' class |
| transcendental | `exp x` (a prefix keyword) | `Float` |
| class crossing | `int2float e`, `float2int e` (prefix keywords) | the direction's own class |

There is no `Bool` value in the language, so a comparison yields the `0`/`1`
scalar and drives an `if` directly; that is also what makes `&`/`|`/`^` the
language's `and`/`or`/`xor` over two comparison results. `==`/`!=` are the
**generalized** equality — any two same-typed values, `Int`s or type values —
while every other operator is same-class-only. Arithmetic is `Int` over `Int`s
and `Float` over `Float`s, and the two classes meet only at the two prefix
conversions — see §5 of
[floating-point](floating-point.md) for what a `Float` is and why nothing else
crosses. See
[language-spec §3](../language-spec.md#3-semantics) for the semantics as a user
reads them; this note is about what it took to have them.

The three scalar operators are **prefix keywords**, not punctuation: `max a b`,
`min a b`, `exp x`. §11 is why, and what they cost.

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

**A prefix keyword operator is the same five layers with a different shape in
each of the first two.**  `int2float`/`float2int`, `max`/`min` and `exp` are not an
`ast::BinOp` variant at a new rung but an `ast::Expr::Prefix` with a `PrefixOp`,
parsed inside the existing unary level (§3), and each carries its class to a
`TypeOperator` — a `KernelBin` variant for `max`/`min`, the target-neutral
`KernelInstr::Exp` for the transcendental, and the target-neutral
`KernelInstr::Conv { from, to }` for a crossing, because a stack machine cannot
tell the two directions apart from its operand, so the IR carries the expr and
each backend answers it alone (§7). One AST form carries all five because the
arity is the operator's own: a one-operand keyword takes an application
(`int2float f x` converts `f x`), a two-operand one takes atoms (`max a b` is the
maximum of `a` and `b`).

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
< > <= >= == != @in   comparison and set membership
|                 bitwise or
^                 bitwise exclusive or
&                 bitwise and
+ -               sum
* / %             product
@assert e         prefix assert
int2float e       prefix keywords  (the same unary level)
float2int e
exp x
max a b
min a b
application       juxtaposition
postfix, atoms
```

The bitwise trio keeps C's nesting (`&` in `^` in `|`) rather than flattening to
one level, because `a | b & c` reading as `(a | b) & c` is a footgun every reader
would have to learn. They sit **tighter than a comparison** — so `a & b == c` is
`(a & b) == c`, which is the reading every modern language uses and the opposite
of C's. Both choices are pinned by `operator_precedence_and_associativity` in
`crates/lichen-language/tests/language/pipeline.rs`.

A comparison is one level, left-associative, so `1 < 2 == 1` is `(1 < 2) == 1`.

`@in` (set membership, [operator-polymorphism](operator-polymorphism.md) §3) shares
that rung: it yields the same `0`/`1` a comparison yields, so `x @in S == 1` reads
`(x @in S) == 1`, and its right operand is an ordinary expression at this level —
the set being tested. It is the one *keyword* operator: the word carries the `@`
sigil the language's reserved words live at, because it is a predicate over a set
rather than punctuation.

The two conversions take the assert's level rather than a new rung: a prefix
keyword takes its operand by juxtaposition and no infix token, so
`int2float a + 1` converts `a` (tighter than every binary operator) and
`int2float f x` converts `f x` (looser than application), which is the one
reading that makes `a + int2float b` need no parentheses. A **two-operand**
prefix keyword takes its operands as atoms instead, so `max a + 1 b` is not a
spelling and `max (a + 1) b` is — the operator's own arity decides the level, and
`PrefixOp::arity` is the one list both the parser and the checker read.

## 4. `<` and `>` compare alone, and `<==` reads a table

`<` and `>` are comparisons and nothing else: no postfix form opens with `<`, so
adjacency no longer changes a token's job — `a<b`, `a < b` and `2 > 1` all compare,
and the zero-width `>` lookahead the retired angle-bracket index needed is gone
with it.
No form is displaced: what used to be an atom at a spaced `<` is still a
comparison, and a glued `<` is now the same comparison.

`<==` is the **table read** (`t <== k`), at the loosest **binary** level: above it
sit only `->`, the annotation pieces and `=>`. It is left-associative, and its
right operand is a whole comparison expression, so `t <== k + 1` reads the key
`k + 1`, `t <== a <== b` reads `(t <== a) <== b`, and — the one asymmetry the
choice creates — `t <== a == b` reads `t <== (a == b)`: the comparison to the right
of the arrow is part of the key, exactly as `k + 1` is. Its left operand must be a
comparison expression, so an annotated container needs parens
(`(t : Table) <== k`). The lexer's longest match keeps `<==` from ever lexing as
`<=` `=`, and `a <= = b` was never a spelling.

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
(`crates/lichen-compute-gpu/tests/compute_gpu/gpu_matches_cpu.rs`), whose inputs are `2^63`
and above — the values where a signed reading would take the other branch.

## 6. Division by zero

`x / 0` and `x % 0` have no value. The interpreter is the only layer that can say
so: `TypeOperator::run` records `operator.divide_by_zero` on the lowlevel's
general extension channel and answers the lazy marker, exactly as every other
refused computation in this language does — so the program reports an undecided
result and the recorded reason explains it. **The reason is carried, not lost**:
`RunOutput` has a `refusals` field (`category: message` lines) filled when the root
has no value from `REFUSAL_CATEGORIES`, the CLI prints them to stderr beside the
`none` answer, and `run::evaluate_reporting` exposes them to a caller. Checked by
`a_zero_divisor_is_recorded_rather_than_answered` and its `float2int` sibling, which
assert both the `none: Int` answer and the named reason.

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

## 7. The two class crossings

`int2float e` and `float2int e` are the only place the language's two scalar
classes meet, and the only expression form whose result type is not its operand's:
the operand is unified against the direction's **source** and the result is its
**target** (`check_prefix`'s `unify_stated_class`). A wrong-class operand gets the
refusal every other
operator issues — the diagnostic names the class it expected, and nothing here
converts silently. [floating-point §4](floating-point.md) owns why the rest of the
language does not cross; this section is about what the crossing cost.

- **The checker unifies only a class the operand already states.** A unify binds
  every cell the operand's class shares, and a body's
  `compute.write ((compute.Write _)(.to n, .at i, .value int2float i))` is one array literal whose integer
  positions and float value hold *one* element-type cell: pinning the index here
  would bind the float written beside it and refuse the very program these two
  words exist to write. So an undecided operand stays undecided and
  `TypeOperator::run` answers the lazy marker at run time — a weaker message than
  a parameter pinned at its apply, paid for by the conversion being usable where
  the classes have not been decided yet.
- **`int2float` is the nearest `f32`, so it is not injective.** `int2float
  16777217` is `16777216.0`: the integer the language holds is machine-sized and
  the float is not, and the rounding is IEEE's, not an error.
- **`float2int` is partial, and the interpreter is the only layer that can say
  so.** It truncates toward zero, and `NaN`, `±inf`, a negative, and anything the
  unsigned `Int` cannot hold record `operator.out_of_range` and answer the lazy
  marker — the same channel `operator.divide_by_zero` (§6) uses.
- **A kernel cannot record a diagnostic**, so the emitter refuses at compile time
  what the interpreter would refuse at run time: a `float2int` of a *literal* that
  is negative, not finite, or too large is refused by name, since wasm would trap
  and SPIR-V is undefined and neither is an answer the program can read.
- **Every value carries its own class, so a crossing has no operand-shaped
  limit.** A parameter read, a literal and a callee's result all cross, and so
  does an expression the body computes in the *other* class first: `int2float
  (x + 1)` over an `Int` parameter is an integer add and then a crossing, and a
  float literal has a form in an integer body. What stays refused is a genuine
  mix inside one operation (`x + 0.5` with no crossing), which no conversion can
  serve — and that refusal is the shared validator's, so one program gets one
  answer whichever backend it is then handed to.
- **The two backends answer one crossing differently and agree on the number.**
  A `Float` fragment's index rides in an `f32` holding the exact integer, so wasm
  emits nothing for `int2float i` where SPIR-V emits `OpConvertUToF` on its 32-bit
  invocation id; an integer SPIR-V module declares the float type it needs and
  emits the same `OpConvertUToF`, from its 64-bit integer. §5.1 of
  [floating-point](floating-point.md) is the record of why that asymmetry is the
  right one, and of the width it costs when the two classes meet in a *float*
  module — `Int` data is 32-bit there.

Checked by `the_two_conversions_cross_in_the_direction_each_one_names`,
`a_conversion_applied_to_the_other_class_is_refused_by_name` and
`a_float_with_no_int_to_truncate_toward_is_recorded_rather_than_answered`
(`crates/lichen-language/tests/language/pipeline.rs`), and on the kernel side by
`a_jit_kernel_crosses_the_two_classes_both_ways`,
`a_body_may_compute_in_one_class_and_cross` and
`a_varying_float_element_is_seeded_from_the_index`
(`crates/lichen-compute/tests/lichen_compute/compute.rs`).

## 8. What is deliberately not here

- **Class-polymorphic arithmetic as a *library*.** The operators themselves are
  polymorphic now — `+ - * /` and the four order comparisons accept either scalar
  class and never mix them, with the operand tie and a refinement condition
  ([operator-polymorphism](operator-polymorphism.md) §3, §9 Phase 1) — and the
  library form is **landed and routed**: the built-in
  [prelude `core`](core-prelude.md) carries `Num`, `in_num` and one binding per
  polymorphic operator (`add`, `sub`, `mul`, `div`, `less`, `greater`,
  `less_or_equal`, `greater_or_equal`), each taking the **operand group** and
  refining its elements' **classes** (`array{(_ ! in_num), 2}` — the array's
  homogeneity is the operand tie and its length the arity), and the *surface*
  operators lower onto those bindings, so `1 + 1.5` is refused by the prelude's own
  tie.  The costs that routing carries — kernel bodies until the kernel
  workstream's specialize pass, an open class's honest `raw[?a, ?b]` rendering, and
  a routed statement's missing value snapshot — are measured in
  [operator-polymorphism](operator-polymorphism.md) §7.1.
- **Shifts (`<<`, `>>`).** No token exists, so `a << b` lexes as two `<` comparisons
  and does not parse. The former blocker — a `>>` token swallowing the adjacent
  closers of nested angle forms — retired with the angle forms themselves
  ([no-type-mode](no-type-mode.md)), so what is left is a missing operator rather
  than a blocked one.
- **Unary minus and unary bitwise-not.** `-x` would collide with the existing
  `-` (binary only), and `~x` with the array literal's shallow marker.  Both are
  spellable (`0 - x`, `x ^ (0 - 1)`) and neither is worth a grammar ambiguity.
  The shallow marker took `@shallow` when `~` became the mark sigil
  (`e ~ "payload"`, [graph-fusion](graph-fusion.md) §3), so `~` is free for a
  unary spelling and is not used by one.
- **`and` / `or` / `not` keywords.** `&`/`|` over `0`/`1` results *are* them; a
  second spelling of one operation would need its own precedence and its own
  short-circuit rule, and there is no `Bool` type for it to be a rule about.
- **A native transcendental, and any operator a target cannot express identically.**
  The kernel-safe subset is the intersection of what the two backends compute the
  same way; an operator outside it would be a silent divergence between backends,
  not a feature. Floating point is in the set (`+ - * /` and the four order
  comparisons over `Float`, plus §7's two crossings) and so are `max`, `min`,
  `exp`, `sqrt` and `floor` (§11) — but the last three are in the set as
  **definitions in the IR** rather than as either target's instruction, and `ln` and
  the trigonometric family wait for a definition the whole domain can be reduced
  into. §11 and [floating-point §6, §7](floating-point.md) are the record.

## 9. Where the coverage is

- Interpreter: `the_extended_operator_set_evaluates` and
  `an_int_is_unsigned_where_the_two_readings_differ`
  (`crates/lichen-language/tests/language/pipeline.rs`).
- Membership: `a_membership_test_compares_a_value_with_a_sets_members` and
  `a_membership_test_matches_a_type_value_against_a_class_domain`
  (`crates/lichen-language/tests/language/membership.rs`); its parse (the comparison rung
  and the left-associativity) is pinned by
  `an_at_in_membership_test_parses_at_the_comparison_level`
  (`crates/lichen-language-parser/src/tests/parse_tests.rs`).
- wasm JIT: `jit_lowers_the_arithmetic_comparison_and_bitwise_operators`
  (`crates/lichen-compute/tests/lichen_compute/compute.rs`) — one kernel per operator, so each
  is on the lowering path.
- The prefix scalar operators: `fusion_numbers`
  (`crates/lichen-compute/examples/fusion_numbers.rs`), whose kernel column writes a
  `Float` consumer with `max` over `d - r` and a `Float` producer with a sum, runs
  them unfused on the cpu backend, and compares them against the two host forms;
  `reduction_interface` (same directory) is the same for a marked `max`-shaped
  reduction. Neither is a test, deliberately: the operator's *numbers* are what the
  example prints, and the two backends are compared on the device by
  `arithmetic`/`predicates` above.
- SPIR-V, on a device: `arithmetic`, `predicates` and `unsigned_reading` in
  `crates/lichen-compute-gpu/tests/compute_gpu/gpu_matches_cpu.rs`, each checked against a
  hand-written expectation **and** an independent CPU reading of the IR.
- SPIR-V, without a device: `the_emitted_module_validates` and
  `the_two_conversions_validate_in_a_float_module`
  (`crates/lichen-compute-gpu/tests/compute_gpu/spirv_validation.rs`) hand the emitted words
  to `spirv-val`, and `an_integer_module_refuses_a_conversion_by_name`
  (`crates/lichen-compute-gpu/tests/compute_gpu/refusals.rs`) is the refusal that needs no
  validator at all.
- The example programs `examples/operators.lichen` and `examples/gcd.lichen` are
  the user-visible statement of the set, and `tests/lichen_compute/examples.rs` runs them.
- Editor grammar: `tree-sitter-lichen`'s own tests, which parse every
  example — `operators.lichen`, `gcd.lichen` and `table.lichen` included — and
  assert no ERROR node, plus the
  `grammar-consistency` guard that every `.scm` query still compiles. See §10.

## 10. The editor grammar

`tree-sitter-lichen/grammar.js` is the language's *editor* face, and it is the
one part of the operator set that does not share a parser with the compiler: it
is generated by the tree-sitter CLI and knows nothing of the checker. It needed
three changes, and the first is the interesting one.

- **The ladder.** §3's order is now the grammar's, one rule per level
  (`binary_comparison`, `binary_or`, `binary_xor`, `binary_and`,
  `binary_addition`, `binary_product`) with `left`/`right`/`operator` fields, so
  `1 + 2 * 3` nests as `binary_addition(1, binary_product(2, 3))` in an editor
  too, and `queries/highlights.scm` (mirrored in the Zed extension) colours
  every operator in §1's table.
- **`<` and `>` are comparisons alone, and the glued `<` token is gone.** They
  were two tokens in the grammar while it still opened a retired `angle_tuple`
  with a glued `<`: a tree-sitter table is LR, it must decide at the token, and
  `a < b` against `f<A, B>` is decided *at* the `<` with more lookahead than one
  token. Whitelisting the conflict in `conflicts` and letting the GLR fork pick
  was tried and is unsound there: the two stacks merge again two tokens later,
  and the losing reading is dropped mid-parse (it turns `f<A, B>` into a
  comparison with a `MISSING` operand). What worked was deciding it in the
  *lexer*, with the one Glue rule the real language has: `angle_tuple` opens with
  `choice(token.immediate('<'), '<')`, so a **glued** `<` was a different token
  from a **spaced** one, and `application`'s left-associativity then read a
  spaced `<` as the comparison. The angle forms retired, the second token retired
  with them, and `_angle_element` (a comparison is not an angle form's element)
  retired too — no `>` opens or closes an angle form now.
- **The retired syntax is gone.** The grammar models the current surface: the
  keyword-led braced types `tuple{…}`, `struct{…}`, `array{T, n}`, `set{E}` and
  `table{K, V}` ([no-type-mode](no-type-mode.md)); the parenthesised values
  `set(a, b)` and `table(k1 ==> v1)`; the glued postfix reads `a(k)` and `a[i]`;
  the infix `t <== k`; `@in`, `@any`, `@shallow` and `@loop`;
  and §7's and §11's seven prefix keywords (`int2float`, `float2int`, `max`,
  `min`, `exp`, `sqrt`, `floor`). `struct<…>`, `array<…, n>`, the bare `<…>`
  tuple type and the raw index `X<e>` no longer parse at all.  The type accesses
  `X{n}` and `X::a` were removed after the grammar migration
  ([type-access](type-access.md)); the grammar has no braced postfix.

Verified with `cargo test --manifest-path tree-sitter-lichen/Cargo.toml` and
`cargo test -p lichen-language-zed --features grammar-consistency`; see
[tree-sitter-generated-files](tree-sitter-generated-files.md) for the CLI and
the generated-files policy.

## 11. The prefix scalar operators: `max`, `min`, `exp`, `sqrt`, `floor`

Five operators were missing for the same reason and landed in two steps.

**`max`/`min`, because the language could not write a maximum.** The only
spelling was `if a > b then a else b`, which lowers to a comparison, an
`I32WrapI64` and a `Select` — and a **select-valued write is classed by nothing
the checker recorded**, so the kernel emitter read the buffer position as `Int`
and a `Float` buffer was refused at the run
(`compute.parallel: input buffer 0 holds Float elements, but this kernel reads
that position as Int`). The reference fusion instance's consumer is a maximum, so
the kernel column `docs/notes/graph-fusion.md` §9 asked for could not exist.

**`exp`, because the first transcendental has no target-neutral instruction.**
wasm has none, and SPIR-V's GLSL `Exp` is implementation-defined per vendor, so
bit-for-bit agreement through native instructions is not achievable. The decision,
its measurement and its cost are [floating-point §6](floating-point.md)'s; what
belongs here is the *surface*.

**`sqrt` and `floor`, because the second and third could be *defined* even where they
could not be emitted.** Both are the same surface — a prefix keyword, one operand by
juxtaposition, `Float`'s alone — and both are *definitions in the IR* rather than a
target's instruction, which is the decision `exp` established:

- **`floor` has no core SPIR-V instruction at all.** GLSL's `floor` is an
  `OpExtInst` call into `GLSL.std.450` — a second extension import and capability, and
  an instruction set wasm does not share — so a native emission would be one
  operation against two instruction sets. Its definition is exact for every operand in
  the class: it converts the *magnitude* (the conversion refuses a negative, and
  `select` evaluates both arms, so a guarded conversion would refuse every negative),
  and puts the sign back with the one-back correction a negative floor needs.
- **`sqrt` could be emitted** — wasm's `f32.sqrt` and SPIR-V's `OpSqrt` are both
  correctly rounded — **but it is defined for the interpreter's sake and for the
  measurement's**: one sequence all three execution models read, and an accuracy
  number that is about the definition rather than about a target.
- **`ln` is not here, and the blocker is now precise.** See
  [floating-point §7](floating-point.md): its range reduction is a *loop*, and the
  inliner that `exp`, `sqrt` and `floor` ride is deliberately one-block straight-line.

What the choice costs, stated plainly:

- **A prefix keyword is reserved, so `max`, `min`, `exp`, `sqrt` and `floor` are no
  longer names a program can bind.** `int2float`/`float2int` already were; the
  prelude's `add` and `mul` are not, and are shadowable. That asymmetry is the price
  of the spelling.
- **The kernel emitter reaches no binding, so it lowers the operator node in the
  author's own body.** That is the property the task wanted: `compute/body.rs`
  answers `TypeOperator::Max`/`Min` through `kernel_bin` and `TypeOperator::Exp`,
  `Sqrt` and `Floor` through `unary_class`, and none inlines a lichen-level body that
  would collapse into `Select`.
- **A definition is a bigger body than an instruction.** `exp` inlines to about
  forty instructions, `sqrt` to about four hundred and forty and `floor` to about
  twenty — once per call site. That is the price of one sequence the two backends and
  the interpreter all read, and it is paid at lowering time rather than at run time.

### The rule the three `Float`-only operators share

`max`/`min` are the arithmetic tie, not comparisons. `Checker::arithmetic_class`
is the one function `+ - * /` and `max`/`min` both call: a concrete `Float` operand
selects `Float` for the whole operation, two stated operands must agree, and two
unstated ones share one open class gated by the same `@in Num` refinement. So
`max 1 1.5` is refused by the tie that refuses `1 + 1.5`, the result of
`max a b` is a value of the operands' class rather than the `0`/`1` a comparison
yields, and a `Float` maximum is emitted as `Float`.

**The reading of a `NaN` operand is the language's, not a target's.** `max` lowers
to a comparison and a `select` — `if a > b then a else b` — in **both** backends,
because wasm's `f32.max` answers NaN where SPIR-V's `OpFMax` answers the other
operand. The two native forms are therefore not usable: a program's numbers would
depend on which backend ran it. `KernelBin::Max`/`Min` state the operator; each
emitter states the same three instructions.

`exp` is `Float`'s alone and pins whatever operand it is given: the instruction has
no other class to be, so an undecided operand has exactly one possible answer. The
conversions deliberately do **not** pin (a unify binds every cell the operand's
class shares, and an undecided one may be shared with a position the author did not
mean to commit) — `exp` can, because there is no second reading to protect.

Two defects in the emission path surfaced while making the kernel column work, and
both are `lichen-compute`'s rather than the operator's:

- **The class of an emitted value was filed under the wrong node.** `Lower::emit`
  recorded a value's class against "the node being defined", which an operand's
  own emission overwrites: `compute.read ((…)(.at s(0) - 1)) + s(1)` filed the
  `+`'s class under the read's node, so the operation was emitted `Int` and the two
  classes met in one instruction. The map is now keyed by the **value** the
  instruction produced, which is a fact of the instruction and cannot be
  overwritten.
- **A buffer's element class came from the body rather than from the parameter.**
  `Positions::element_class` was "the first write's class, a fallback", so a
  `Float` buffer whose write was a `select` (or a `@loop` fold) was read as `Int`.
  `KernelRoles` now carries the class each `Buf T` **states**, and a stated one is
  the authority; an open element cell still falls back to the write's class, which
  is what the older spelling means. `compute-buffer-wrapper.md` has the model.

### What `sqrt` and `floor` cost, measured

`crates/lichen-compute/examples/math_operators.rs` sweeps each definition over every
exponent of the class and prints the numbers, and runs each as a kernel on both
backends over one fixed sample set. The measurements:

- **`sqrt`**: worst relative error `1.19e-7` against the correctly rounded `f32`
  square root over 1,134,592 samples spanning `2^-149` through `f32::MAX`, with 74.9%
  of them bit-exact. `0`, a negative and `NaN` answer `+0` — the definition's own
  answer, since the reduction maps a non-positive operand to zero — and `+inf`
  answers `NaN`.
- **`floor`**: **zero differences** from the host's `floor` over the same sweep, both
  signs, subnormals and infinities included. The definition is exact, not an
  approximation that happens to round right: every operation in it is one the class
  performs exactly.
- **The device disagrees with `sqrt`'s definition by one unit in the last place on 21
  of those 64 kernel elements, and agrees with `floor`'s on all 64.** That is the
  device's `OpFDiv`, not the definition: a probe of 256 quotients differed from the
  correctly rounded `f32` answer in **85 of them for `/3` and 139 for `/7`**, and in
  none for `/4`. This device's floating-point division is not correctly rounded, so
  every Newton step written with a division can diverge — which is why
  [floating-point §4.4](floating-point.md) already refuses to specify a kernel's
  float division.
