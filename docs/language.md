# Language spec (v1)

*A minimal source language that compiles to the highlevel IR
([`lichen-highlevel`](crates/lichen-highlevel)) and produces proper diagnostics.
Brainstormed 2026-08-22; the surface decisions (lambda syntax, no `let`, type
literal names, program shape) were settled by the user. Status: spec for the
`crates/lichen-language` crate.*

The language is deliberately small: a pure lambda calculus with annotations,
tuples, and arrays, over a single `Type : Type` universe. It exists to be the
first real text → IR pipeline in the repo — a hand-written lexer, parser, name
resolver, and IR emitter on top of the existing highlevel checker, which runs
*unchanged*.

---

## 1. Goals

1. **A real pipeline.** Source text → lex → parse → resolve → IR
   (`lichen_highlevel::ir::ExprTable`) → `Checker::build` → rendered
   diagnostics. Every stage is testable on its own.
2. **Proper diagnostics.** Every error — frontend or checker — is a value with
   a span and a message; bad input never panics. Checker failures keep the
   highlevel's existing rendering (expected/found, the `?a` flow, ambiguity).
3. **No changes to the type layer.** The checker, the IR, and the lowlevel VM
   are used as-is; the frontend is a pure producer of `ExprTable`s.

## 2. Syntax

```
program  := (name '=' expr ';')* expr              -- bindings, then the final expression

expr     := name '=>' expr                        -- lambda; body extends maximally
          | expr ':' expr                         -- annotation; right-assoc, loosest
          | expr '->' expr                        -- function type; right-assoc
          | atom '<' expr '>'                     -- array type  T<e>
          | atom '[' expr ']'                     -- index  e[i]
          | expr expr                             -- application; left-assoc, tightest
          | atom
atom     := int_literal
          | 'Int' | 'Type'                        -- the two type constants
          | name
          | '(' expr ')'                          -- grouping
          | '(' expr ',' expr (',' expr)* ')'     -- tuple  (TypeTuple in type position)
          | '[' expr (',' expr)* ']'              -- array literal
          | '<' expr ',' expr (',' expr)* '>'     -- tuple type  (always TypeTuple)
          | 'struct' '{' expr (',' expr)* '}'     -- struct type  (nominal, positional fields)
```

- **Keywords:** `Int`, `Type`, `struct`, `=>`, `->`, `:`.  `=` binds a name in
  a statement, `;` separates statements.  `--` starts a line comment (to end
  of line).
- **Names:** lowercase or mixed-case identifiers (`x`, `id`, `n2`).  `Int`,
  `Type`, and `struct` are reserved — they cannot be bound or used as names.
- **Integers:** non-negative decimal literals (`0`, `42`); a literal that
  overflows `usize` is a lex error.
- **Precedence** (loosest → tightest): `:` → `->` → application → postfix
  `<e>` / `[e]` / atoms.  `x => e : T` parses as `x => (e : T)` — lambda bodies
  extend through annotations, as do array lengths: `Int<x : T>` is the array
  type whose length is the annotated expression.
- **One grammar, one position flag.**  Types are expressions, so term and type
  forms share one grammar.  The parser threads a *type-mode* flag that only
  decides `(a, b)` — a `Tuple` value in term position, a `TypeTuple` type
  expression in type position (after `:`).  Angle brackets need no flag:
  `<a, b>` is always a `TypeTuple`, in both positions.

### 2.1 Distinct delimiters

Brackets `[ ]` and parens `( )` build values; angle brackets `< >` and
braces `{ }` build types.  `[1, 2]` is an array value and `(1, 2)` a tuple
value; `T<3>` is the array type of length 3, `<Int, Type>` the tuple type,
and `struct { Int, Type }` a nominal struct type.  A `<` directly
after an expression is *always* the array type — application is
juxtaposition, so no whitespace rule exists (the earlier `T[e]` design needed
one to tell `Int[3]` from `f [3]`; angle brackets removed it).  A type tuple
in argument position is parenthesized: `f (<Int, Type>)`.  A single-element
`<Int>` is a parse error — `(Int)` is the grouping form.

`e[i]` indexes an array or tuple.  A `[` directly after an expression is
*always* an index (position decides, never whitespace), so an array literal
in argument position needs parens too: `f ([1, 2])`.  `a[0][1]` chains, and
`[e1, e2][i]` is the language's only conditional form — with an integer
index it selects a branch.

## 3. Semantics

- **Programs are pure expressions.**  The checker compiles the IR, runs the
  definition pass (so apply-time type checks fire), and the program's value is
  the evaluation of its root: an `Int`, a tuple, an array, a function, or a
  type expression.
- **Statements and bindings.**  A program is a sequence of `name = expr;`
  bindings followed by a final expression.  A binding is *graph sharing*, not
  sugar: its value compiles once into the IR arena, and every use of the name
  is that same node id (the IR is a graph).  There is no `let` node and no
  desugared lambda — the final expression stays the program's root, so its
  type is determinable exactly like a bare program's.  Bindings are
  sequential (a value may read earlier bindings) and shadowable; a binding
  cannot refer to itself (its value compiles before the name is in scope).
  Sharing means a bound *non-function* value has one type across uses, while
  a bound lambda stays polymorphic — each application still instantiates the
  parameter fresh via the runtime's per-apply clones.
- **Every lambda is automatically let-polymorphic.**  Each application
  instantiates the parameter fresh — the lowlevel apply machinery clones the
  parameter per call site — so `(x => x) 5 : Int` and `(x => x) Type : Type`
  both check, and one binder used at two different types
  (`(id => ((id 5 : Int), (id Type : Type))) (x => x)`) checks as well.  No
  generalize/instantiate special form exists or is needed.
- **Types are first-class values.**  `Int`, `Type`, function types
  (`T -> U`), tuple types, and array types (`T<e>`) are ordinary values that
  can be passed around, bound, and used in type position.  `Type : Type`
  holds in a single universe; kinding is an ordinary type check (a literal in
  type position is a kinding error, not a separate "kind system").
- **Indexing.**  `e[i]` reads the `i`-th element of an array or tuple.  A
  literal index into a statically-known array is checked against its length
  at check time (an out-of-bounds index is an `IndexOutOfBounds`
  diagnostic); an index known only at runtime (a parameter, a call result)
  is checked when evaluated.  `[then, else][i]` is the language's only
  conditional form — an integer index selects a branch, and the untaken
  branch is never evaluated (the lowlevel `Index` stays lazy on it).
- **Nominal struct types.**  `struct { T1, ..., Tn }` is a *new type* with
  positional fields (no names in v1).  Its kind slot holds a **fresh nominal
  id** — each occurrence of the syntax allocates a new id, so two
  occurrences never unify and a struct never unifies with a same-shape tuple
  type (nominal identity).  Bind one occurrence and it is reusable: the
  checker compiles each expression once, so a bound or parameter-passed
  struct type used many times is the *same* type — `s = struct { Int }; [s, s]`
  is a homogeneous array, while `[struct { Int }, struct { Int }]` (two
  source occurrences) is a nominal conflict.
- **Struct instantiation.**  `(v1, ..., vn) : struct { T1, ..., Tn }` wraps
  a positional tuple in the nominal type: the element types are checked
  against the field list (arity and field types must match), and the
  annotation's type is the struct type itself.  A literal is not a struct
  value — `5 : struct { Int }` conflicts.  Re-annotating an instance with
  the *same* struct type passes; a different source occurrence is a
  different type.  Field access and values of struct type beyond the
  wrapped tuple are future work.
- **Dependent array types (pinning).**  The length of `T<e>` is an arbitrary
  expression, so `Int<n>` where `n` is bound is a legal dependent type.  When
  an annotation compares a value against such a type, the length read — an
  unevaluated `Index` over `n`'s value cell — resolves as a pure reference
  and is pinned to the value it must equal: the checker binds `n` to the
  literal's length, so the parameter is monomorphized.
  `((n => ([1, 2, 3] : Int<n>)) 3)` checks and runs, and applying any other
  length fails at the apply — the pinned value is enforced per application
  (the apply's argument unify compares the cloned parameter, which carries
  the pinned length, against the argument).

## 4. Compilation: source → IR

Each AST node compiles to exactly one `ExprKind` (all spans `(line, column)`,
1-based, supplied by the frontend per the IR contract):

| source | `ExprKind` |
|---|---|
| `5` | `Constant(USize(5))` |
| `Int` | `Constant(TypeInt)` |
| `Type` | `Constant(TypeType)` |
| name use | the binder's own `ExprId` (pre-resolved) |
| `x => e` | `Function { parameter, return }` — `parameter` is the `Parameter` expr for `x` |
| `e1 e2` | `Apply { function, argument }` |
| `e[i]` | `Index { array, index }` |
| `e : T` | `Annotation { value, type }` |
| `T1 -> T2` | `TypeFunction { parameter, return }` (domain, codomain) |
| `(e1, …, en)` | `Tuple(range)` |
| `<T1, …, Tn>` | `TypeTuple(range)` |
| `struct { T1, …, Tn }` | `TypeStruct(range)` — nominal, fresh id per occurrence |
| `[e1, …, en]` | `Array(range)` |
| `T<e>` | `TypeArray { element_type, length }` |

There is no desugar step (no `let`).

### Name resolution

A scope stack of `name → ExprId`.  Compiling `x => e`:

1. allocate the `Parameter` expression (span = `x`'s span),
2. push `x` onto the scope stack,
3. compile `e`,
4. pop, then allocate `Function { parameter, return }`.

A use of `x` in `e` therefore *is* the parameter's own `ExprId` — the checker's
scope stack is keyed by it, and the IR carries no name strings.  A statement
binding `a = e` resolves the same way without a lambda: the value compiles to
one `ExprId`, `a` is pushed onto the stack (never popped — later statements
see it), and a use of `a` is the value's own `ExprId`.  Shadowing is allowed
(the inner binding wins).  A name in no scope is a **resolve diagnostic** at
the name's span — the checker's `lookup` panics on unresolved ids, so the
frontend guarantees resolution before the IR leaves the crate.

## 5. Diagnostics

### Stages

| stage | example |
|---|---|
| Lex | `unexpected character '@'` |
| Parse | `expected ')', found ']'` |
| Resolve | `unresolved name 'y'` |
| Check | `expected TypeInt → TypeInt, found TypeInt` (+ the `?a` flow lines) |

A `Diag { span: Option<(u32, u32)>, message: String, stage: Stage, check: Option<Box<...>> }`.
The `message` is the rendered form for display (`render`); `stage` says which
pipeline stage produced it.  Checker diagnostics additionally carry their
structured facts in `check` — the highlevel `Diag` (`kind`, the conflicting
classes `a`/`b` and their recorded values, `span`) — which tests and tooling
match on instead of the message; frontend errors leave it `None`.  The
frontend stops at the first lex/parse/resolve error (no recovery in v1);
checker diagnostics can be many, in order.

### Rendering

```
error: unresolved name 'y'
  --> test.lichen:1:5
   |
 1 | x => y
   |      ^
```

`render(source, &diag)` prints the stage prefix, the `line:col` header, the
offending line, and a caret at the column.

### The bar

Every error is grounded in a span and a message — no panics, no "internal"
messages.  Garbage input (`""`, `"("`, `"3 :"`, `"\@"`, `"x =>"`) returns
diagnostics.

### Checker spellings

The highlevel printer renders constants by their canonical names, so
diagnostics show `TypeInt`, `TypeType`, `TypeFunction`, `TypeTuple`,
`TypeArray`, `none`, `Function` even though the source spells the first two
`Int` and `Type`.  `Int` (the value) prints as its own digits.

## 6. Examples

Well-typed programs (each checks and evaluates):

```
5                                        -- 5 : Int
(x => x) 5 : Int                         -- 5
x => x                                   -- a function, type ?a → ?a
(((id => (id 5 : Int)) (x => x)) : Int) -- 5 (the root apply is annotated)
(((id => ((id 5 : Int), (id Type : Type))) (x => x)) : <Int, Type>)  -- the polymorphic id
(1, (x => x))                            -- a heterogeneous tuple
([1, 2, 3] : Int<3>)                     -- array literal against its array type
(((n => ([1, 2, 3] : Int<n>)) 3) : Int<3>)  -- dependent: the length pins n to 3
([1, 2, 3])[1]                           -- 2 (an index)
((i => [10, 20][i]) 1 : Int)             -- 20 (the index is a runtime parameter)
a = [1, 2]; b = 0; a[b]                  -- 1 (statements: bindings, then the final expression)
a = x => x; ((a 5 : Int), (a Type : Type))  -- (5, Type) — a bound lambda stays polymorphic
struct { Int, Int }                      -- [Int, Int] (a nominal struct type, first-class value)
s = struct { Int }; (s, s)               -- [[Int], [Int]] — one occurrence, reused
(1, 2) : struct { Int, Int }             -- [1, 2] (instantiation: a tuple wrapped in the struct type)
s = struct { Int, Int }; ((1, 2) : s)    -- [1, 2] (the same bound type, instantiated)
```

(A top-level *unannotated* application's result type is a lazy cell, so the
program's type is ambiguous — annotate the root application, as above.
Statement bindings don't wrap the root in an application, so a binding
program's final expression is checked like a bare program.)

Ill-typed programs (expected diagnostics):

| program | diagnostic |
|---|---|
| `x` | `unresolved name 'x'` (resolve) |
| `((id => id 5) (x => x))` | `cannot determine the type of the program: ?a is ambiguous` (check) |
| `5 : Int -> Int` | `expected TypeInt → TypeInt, found TypeInt` (check) |
| `5 : 5` | `expected TypeType, found TypeInt` (check, kinding) |
| `(5 3)` | `expected a function, found TypeInt` (check, guard) |
| `[1, x => x]` | the array elements do not share one type (check) |
| `([1, 2, 3])[5]` | `index 5 out of bounds (array length 3)` (check) |
| `a = 5; y` | `unresolved name 'y'` (resolve) |
| `[struct { Int }, struct { Int }]` | `expected TypeId(1), found TypeId(0)` (check — two occurrences never unify) |
| `(((n => ([1, 2, 3] : Int<n>)) 5) : Int<3>)` | `expected 3, found 5` (check, runtime — `n` is pinned to 3 by the annotation) |

## 7. Non-goals (v1)

- **No arithmetic** (`+`, `-`, …) — the lowlevel VM has no arithmetic
  builtins, and adding them is a lowlevel change, not a frontend one.
- **No dedicated `if`** — a branch is written `[then, else][i]` with an
  integer index; there are no comparison operators, so a runtime condition
  must come from a parameter or a call result.
- **No recursion** — the IR is an acyclic graph; a function cannot mention
  itself, and a binding cannot refer to itself (its value compiles before the
  name is in scope).
- **No multi-parameter functions** (single `Parameter` per `Function`; currying
  is written `x => y => e`).
- **No parameter annotations** (`x : T => e`), strings, booleans, or unit.
- **No error recovery** — the frontend reports the first lex/parse/resolve
  error.
- **No modules or mutual definitions** — a program is statement bindings
  followed by one final expression.

## 8. Future work

- Arithmetic and conditionals: promote a small operator set into the lowlevel
  and add an `if`-branch form to the IR + checker.
- Recursion: needs either cyclic IR or top-level `def`s with a
  definition-pass environment.
- Genuinely dependent lengths: v1 pins a bound length to the value the check
  sees, monomorphizing the parameter.  Per-application checking — the same
  template yielding a different type per argument — requires the annotation
  check to travel with the apply (a runtime annotation), which is future
  work.
- Parameter annotations: desugar `x : T => e` to `x => (y => e) (x : T)` with
  a fresh binder (the annotation binds `x`'s type cell at check time).
- Error recovery / multi-error reporting; caret spans (start/end) instead of
  single positions.

## 9. Running the examples

`crates/lichen-language/examples/programs/` holds one file per key feature (a
literal, a lambda, polymorphism, tuples, arrays, indexing, the index-as-
conditional, the dependent length, statements, first-class types, nested
arrays), each with an `-- output:` comment promising its result.  Run one
program from the workspace root:

```
cargo run -p lichen-language -- crates/lichen-language/examples/programs/bindings.lichen
```

or a whole directory (one `file: output` line per program):

```
cargo run -p lichen-language -- crates/lichen-language/examples/programs
```

The runner also installs as a standalone CLI named `lichen` — `cargo install
--path crates/lichen-language` from a checkout of the repo, or `cargo install
--git <repo-url> lichen-language` — after which `lichen <program.lichen |
directory>` runs the same commands.

`tests/examples.rs` checks every example file against its promised output,
so the examples stay the living spec.  The value printer renders `USize`s as
digits, arrays — and tuples, which are the same runtime value — as
`[a, b]`, functions as `Function`, and the type constants as `Int` / `Type`.
