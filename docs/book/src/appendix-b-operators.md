# Appendix B: The Operators, by Signature

The precedence ladder is in Chapter 3 (and the spec's §2); this appendix
is the other half of the reference: what each operator takes, what it
answers, and where it is taught.

## Arithmetic and comparison

| form | operand classes | result | notes |
|---|---|---|---|
| `a + b`, `a - b`, `a * b` | both `Int`, or both `Float` | the same class | unsigned wrap for `Int`; IEEE for `Float` |
| `a / b` | both `Int`, or both `Float` | the same class | `Int` division truncates; zero divisor answers `none` and reports the reason |
| `a % b` | both `Int` only | `Int` | no `%` on `Float` |
| `a < b`, `a > b`, `a <= b`, `a >= b` | both `Int`, or both `Float` | `Int` (0/1) | unsigned order on `Int` |
| `a == b`, `a != b` | any two values of one type | `Int` (0/1) | generalized equality; floats compare by bits |
| `a & b`, `a \| b`, `a ^ b` | both `Int` | `Int` | bitwise; over 0/1 they are and/or/xor |

`Int` and `Float` never mix. The only crossings are the prefix keywords
below.

## Prefix keywords

| form | operand | result | notes |
|---|---|---|---|
| `int2float e` | `Int` | `Float` | nearest `f32` |
| `float2int e` | `Float` | `Int` | truncates toward zero; partial (out-of-range answers `none`) |
| `max a b`, `min a b` | two atoms of one class | that class | the operands tie; a mixture is refused |
| `exp e`, `sqrt e`, `floor e`, `sin e`, `cos e`, `ln e` | `Float` | `Float` | the language's own sequences, not the platform's; `floor` is exact |
| `@assert e` | any expression | `e`'s own value and type | requires `e` to answer `1` |
| `@gc e` | any expression | `e`'s own value and type | a scope of its own (Chapter 6) |

## Reads

| form | container | result | notes |
|---|---|---|---|
| `a(k)` | tuple value | the slot's value | index is a full expression |
| `a[i]` | array value | the element | out of bounds is refused |
| `a.name` | struct instance | the field's value | missing name is refused with a suggestion |
| `X::a` | struct type value | the field's type, as a value | container must be a struct type |
| `X{n}` | tuple type value | component `n` of the type | container must be a tuple type |
| `t <== k` | table value | the value stored under `k` | a miss is a diagnostic |
| `x @in s` | value, set | `Int` (0/1) | set membership |

The container kind is required by each form; a wrong kind is a check-time
refusal, never a runtime dispatch (Chapter 7's table).

## Annotations and marks

| form | slot | checked how |
|---|---|---|
| `e : T` | type | unification |
| `e # p` | perspective | divisibility of the requirement against the provider (Chapter 13) |
| `e ! p` | refinement | the predicate must answer `1` (Chapter 11) |
| `e ? d` | doc | never constrains; a later doc overrides |
| `e ~ "text"` | mark | compile-time metadata; no slot, no value |

## The `$` form

`$name(args…)` is a native operator: an atom whose name resolves only
against the compiling module's private plugin registry (Chapter 19). It is
written only in a plugin's own source.

## What is not an operator

- There is no unary minus; write `0 - x` (and `0.0 - x`).
- There is no concatenation for strings, and no `+` on them at all: the
  arithmetic operators are class-tied to `{Int, Float}`.
- There is no boolean type; comparisons answer `0`/`1`, and `& | ^` over
  them are the logical connectives.
- There is no ternary; `if` is an expression over a lazy two-element array.
