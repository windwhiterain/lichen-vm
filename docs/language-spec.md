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
   highlevel's existing rendering (expected/found, the `?a` flow).
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
          | '_'                                   -- inference placeholder (type position only)
          | name
          | '(' expr ')'                          -- grouping
          | '(' expr ',' expr (',' expr)* ')'     -- tuple  (TypeTuple in type position)
          | '[' expr (',' expr)* ']'              -- array literal
          | '<' expr ',' expr (',' expr)* '>'     -- tuple type  (always TypeTuple)
          | 'struct' '<' expr (',' expr)* '>'     -- struct type  (nominal, positional fields)
```

- **Keywords:** `Int`, `Type`, `struct`, `=>`, `->`, `:`.  `=` binds a name in
  a statement, `;` separates statements.  `--` starts a line comment (to end
  of line).
- **Names:** lowercase or mixed-case identifiers (`x`, `id`, `n2`).  `Int`,
  `Type`, and `struct` are reserved — they cannot be bound or used as names.
- **The `_` placeholder.**  In type position (the right side of `:`, and the
  components of the type forms under it), `_` is an inference placeholder:
  the checker infers the type from context — `x : _`, `x : Int -> _`,
  `x : Int<_>`, `x : <Int, _>`, `struct<Int, _>`.  Outside type positions
  `_` is an ordinary name, so `_ = 5; _` and `_ => _` stay legal.
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

Brackets `[ ]` and parens `( )` build values; angle brackets `< >` build
types.  `[1, 2]` is an array value and `(1, 2)` a tuple value; `T<3>` is the
array type of length 3, `<Int, Type>` the tuple type, and
`struct<Int, Type>` a nominal struct type.  A `<` directly
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
  is checked when evaluated.  Indexing a *concretely* non-indexable type —
  a function, an atomic type, a struct type — is an `IndexTarget`
  diagnostic at check time, not a runtime panic (mirroring the apply
  guard).  `[then, else][i]` is the language's only
  conditional form — an integer index selects a branch, and the untaken
  branch is never evaluated (the lowlevel `Index` stays lazy on it).
- **Nominal struct types.**  `struct<T1, ..., Tn>` is a *new type* with
  positional fields (no names in v1).  Its kind slot holds a **fresh nominal
  id** — each occurrence of the syntax allocates a new id, so two
  occurrences never unify and a struct never unifies with a same-shape tuple
  type (nominal identity).  Bind one occurrence and it is reusable: the
  checker compiles each expression once, so a bound or parameter-passed
  struct type used many times is the *same* type — `s = struct<Int>; [s, s]`
  is a homogeneous array, while `[struct<Int>, struct<Int>]` (two
  source occurrences) is a nominal conflict.
- **Struct instantiation.**  `s(1, 2)` — an application whose callee is a
  struct type — wraps the positional tuple in the nominal type: it compiles
  to the dedicated `Instantiate` expression, whose element types are checked
  against the field list (arity and field types must match), and whose type
  is the struct type itself.  A literal is not a positional value —
  `s(5)` conflicts.  Instances of different source occurrences are
  different types, even with the same fields.  The callee is recognized by
  the frontend from its IR node (the literal `struct<...>` or a name bound
  to one); a struct type arriving through a parameter is not recognized and
  falls through to a plain application, which fails at runtime (applying a
  non-function is a VM panic, not a diagnostic).  Field access and values of
  struct type beyond the wrapped tuple are future work.
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
- **The `_` placeholder.**  A `_` in type position compiles to an unbound
  cell: the annotation unifies the value's type against it, so the cell
  binds to that type — `5 : _` infers `int`, `x => x : _` the arrow
  `?a → ?a`, and `[1, 2, 3] : Int<_>` the length `3`.  Partial types infer
  the rest: `((x => x) : (Int -> _)) 5` fixes the input to `int` and infers
  the output.  Kinding is deferred for `_` like any unbound type, so `_`
  never raises a kinding error; a `_` that never binds leaves the type
  underdetermined — not an error — and a mismatch against a
  partial type is still an error (`5 : Int -> _` fails).

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
| `_` (type position) | `Placeholder` |
| `T1 -> T2` | `TypeFunction { parameter, return }` (domain, codomain) |
| `(e1, …, en)` | `Tuple(range)` |
| `<T1, …, Tn>` | `TypeTuple(range)` |
| `struct<T1, …, Tn>` | `TypeStruct(range)` — nominal, fresh id per occurrence |
| `s(1, 2)` (callee a struct type) | `Instantiate { type_expr, value }` |
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

