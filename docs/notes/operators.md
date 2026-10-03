# The computational operators

> Status: current.  The operator vocabulary a lichen program computes with, the
> five layers one operator lives in, the three decisions that were made to add
> it (precedence, the two tokens that are both a bracket and a comparison, and
> the unsigned reading of `Int`), the two prefix keywords that cross the
> language's scalar classes, and the editor grammar's own reading of the same
> set.
> Points at: `crates/lichen-highlevel/src/ir.rs` (`BinOp`),
> `crates/lichen-highlevel/src/program.rs` (`TypeOperator`: the vocabulary, its
> codec tags, and its `run`),
> `crates/lichen-highlevel/src/checker/operators.rs` (the check, and
> `check_convert`),
> `crates/lichen-kernel-ir/src/lib.rs` (`KernelBin`, `KernelInstr::Conv`),
> `crates/lichen-compute/src/compute.rs` (`kernel_bin`, `conv_of`,
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
each of the first two.**  `int2float`/`float2int` are not an `ast::BinOp` variant
at a new rung but an `ast::Expr::Convert` with a `ConvOp`, parsed inside the
existing unary level (§3), and not a `KernelBin` variant but the
target-neutral `KernelInstr::Conv { from, to }` — a stack machine cannot tell the
two directions apart from its operand, so the IR carries the pair and each
backend answers it alone (§7).

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
int2float e       prefix crossing   (the same unary level)
float2int e
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
reading that makes `a + int2float b` need no parentheses.

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

## 7. The two class crossings

`int2float e` and `float2int e` are the only place the language's two scalar
classes meet, and the only expression form whose result type is not its operand's:
the operand is unified against the direction's **source** and the result is its
**target** (`check_convert`). A wrong-class operand gets the refusal every other
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
(`crates/lichen-language/tests/pipeline.rs`), and on the kernel side by
`a_jit_kernel_crosses_the_two_classes_both_ways`,
`a_body_may_compute_in_one_class_and_cross` and
`a_varying_float_element_is_seeded_from_the_index`
(`crates/lichen-language/tests/compute.rs`).

## 8. What is deliberately not here

- **Class-polymorphic arithmetic as a *library*.** The operators themselves are
  polymorphic now — `+ - * /` and the four order comparisons accept either scalar
  class and never mix them, with the operand tie and a refinement condition
  ([operator-polymorphism](operator-polymorphism.md) §3, §9 Phase 1) — and the
  library form is **landed**: the built-in [prelude `core`](core-prelude.md)
  carries `Num`, `in_num` and one binding per polymorphic operator (`add`, `sub`,
  `mul`, `div`, `less`, `greater`, `less_or_equal`, `greater_or_equal`), each
  refining its operands' **classes** (`x : (_ ! in_num)`).  What is still not
  here is the *routing*: `+` remains the checker's special case (R3) rather than
  a binding the surface operator resolves to, so `core`'s bindings are wrappers
  over the builtin's contract rather than the contract itself
  ([operator-polymorphism](operator-polymorphism.md) §5, §9 Phase 3).
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
- **`min`/`max`, and any operator a target cannot express identically.** The
  kernel-safe subset is the intersection of what the two backends compute the
  same way; an operator outside it would be a silent divergence between backends,
  not a feature. Floating point is now in the set (`+ - * /` and the four order
  comparisons over `Float`, plus §7's two crossings), and
  [floating-point §5](floating-point.md) is the record of which of those the two
  backends answer alike.

## 9. Where the coverage is

- Interpreter: `the_extended_operator_set_evaluates` and
  `an_int_is_unsigned_where_the_two_readings_differ`
  (`crates/lichen-language/tests/pipeline.rs`).
- Membership: `a_membership_test_compares_a_value_with_a_sets_members` and
  `a_membership_test_matches_a_type_value_against_a_class_domain`
  (`crates/lichen-language/tests/membership.rs`); its parse (the comparison rung
  and the left-associativity) is pinned by
  `an_at_in_membership_test_parses_at_the_comparison_level`
  (`crates/lichen-language-parser/src/tests/parse_tests.rs`).
- wasm JIT: `jit_lowers_the_arithmetic_comparison_and_bitwise_operators`
  (`crates/lichen-language/tests/compute.rs`) — one kernel per operator, so each
  is on the lowering path.
- SPIR-V, on a device: `arithmetic`, `predicates` and `unsigned_reading` in
  `crates/lichen-compute-gpu/tests/gpu_matches_cpu.rs`, each checked against a
  hand-written expectation **and** an independent CPU reading of the IR.
- SPIR-V, without a device: `the_emitted_module_validates` and
  `the_two_conversions_validate_in_a_float_module`
  (`crates/lichen-compute-gpu/tests/spirv_validation.rs`) hand the emitted words
  to `spirv-val`, and `an_integer_module_refuses_a_conversion_by_name`
  (`crates/lichen-compute-gpu/tests/refusals.rs`) is the refusal that needs no
  validator at all.
- The example programs `examples/operators.lichen` and `examples/gcd.lichen` are
  the user-visible statement of the set, and `tests/examples.rs` runs them.
- Editor grammar: `tree-sitter-lichen`'s own two tests, which parse every
  example — `operators.lichen` and `gcd.lichen` included — and assert no ERROR
  node, plus the `grammar-consistency` guard that every `.scm` query still
  compiles. See §10.

## 10. The editor grammar

`tree-sitter-lichen/grammar.js` is the language's *editor* face, and it is the
one part of the operator set that does not share a parser with the compiler: it
is generated by the tree-sitter CLI and knows nothing of the checker. It needed
two changes, and the second is the interesting one.

- **The ladder.** §3's order is now the grammar's, one rule per level
  (`binary_comparison`, `binary_or`, `binary_xor`, `binary_and`,
  `binary_addition`, `binary_product`) with `left`/`right`/`operator` fields, so
  `1 + 2 * 3` nests as `binary_addition(1, binary_product(2, 3))` in an editor
  too, and `queries/highlights.scm` (mirrored in the Zed extension) colours
  every operator in §1's table.
- **`<` and `>` are two tokens, not one.** The real parser re-reads the
  expression it just parsed to decide whether a `<` opens a bracket or
  compares (§4). A tree-sitter table is LR: it must decide at the token, and
  the decision needs more lookahead than one token — `a < b` and `f <Int, Type>`
  are both decided *at* the `<`. Whitelisting the conflict in `conflicts` and
  letting the GLR fork pick was tried and is unsound here: the two stacks
  merge again two tokens later, and the losing reading is dropped mid-parse
  (it turns `f <Int, Type>` into a comparison with a `MISSING` operand).

  What works is deciding it in the *lexer*, with the one Glue rule the real
  language has: `angle_tuple` opens with `choice(token.immediate('<'), '<')`,
  so a **glued** `<` is a different token from a **spaced** one, and
  `application`'s left-associativity then reads a spaced `<` as the comparison
  and a glued one as a fresh atom. No lookahead, no fork, and the generated
  lexer shows the two paths plainly (`<` → the immediate accept before the
  whitespace skip, → the plain accept after it).

  Two consequences are worth naming, because they are deviations from the real
  parser rather than costs of the approach:

  - **A tuple type applied to a bare atom must be written glued**: `f<A, B>`,
    not `f <A, B>`. Every occurrence in this repository already writes it glued
    (`array<Int, 3>`, `struct<…>`), and an operand position is unaffected
    (`x : <Int, Int>`).
  - **A comparison is not an angle form's element.** `_angle_element` is every
    expression form but a comparison, because the `>` that *closes* an angle
    form is also the comparison operator and the table would otherwise read
    `<a, b, c>` as `(<a, b) > (c > …)`. A comparison is never a type and never
    an index, so nothing real is lost; without it, every angle form with three
    or more elements stops parsing.

The one thing this grammar still cannot do is model the raw index `X<e>` as its
own node: the fresh-atom reading of a glued `<` is the lenient one-or-more angle
form the grammar has always had, so `X<0>` and `array<Int, 3>` are an
application of an atom to an `angle_tuple`. That is the tree it produced before
the comparison operators existed, so nothing regresses; the node is only a
coarser reading, and an editor grammar's job is colouring.

**And the one thing it does not yet know is §7's two keywords.** `int2float` and
`float2int` colour as identifiers there, because the rule would be a third
`prec(PREC.assertion, seq(keyword, field('value', $.application)))` in a grammar
this workspace cannot build: the generated parser is gitignored and
`tree-sitter generate` needs a CLI that is not on this machine, and
[tree-sitter-generated-files](tree-sitter-generated-files.md) puts a grammar
change in the hands of whoever can test it. Nothing about the *language* waits
for it — the compiler's own lexer, parser and checker have the two operators, the
language server classifies them as keywords (the exhaustive `TokenKind` match in
`lichen-language-server/src/analysis.rs` is what forces the arm), and an editor
that highlights through the server's semantic tokens colours them today.

Verified with `cargo test --manifest-path tree-sitter-lichen/Cargo.toml` and
`cargo test -p lichen-language-zed --features grammar-consistency`; see
[tree-sitter-generated-files](tree-sitter-generated-files.md) for the CLI and
the generated-files policy.
