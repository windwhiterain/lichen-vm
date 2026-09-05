# Language spec (v1)

> Status: current — the single source of truth for the lichen source language.
> Owned by [`crates/lichen-language`](../crates/lichen-language). This is the one
> retained early document; feature notes describe the surrounding system and refer
> here for syntax ([doc index](README.md)).

*A minimal source language that compiles to the highlevel IR
([`lichen-highlevel`](crates/lichen-highlevel)) and produces proper diagnostics.
Brainstormed 2026-08-22; the surface decisions (lambda syntax, `let` bindings,
type literal names, program shape) were settled by the user. Status: spec for the
`crates/lichen-language` crate.*

The language is deliberately small: a pure lambda calculus with annotations,
tuples, and arrays, over a single `Type : Type` universe. It exists to be the
first real text → IR pipeline in the repo — a logos-generated lexer, a chumsky
parser, a name resolver, and an IR emitter on top of the existing highlevel
checker, which runs *unchanged*.

---

## 1. Goals

1. **A real pipeline.** Source text → lex → parse → resolve → IR
   (`lichen_highlevel::ir::IR`) → `Checker::build` → rendered
   diagnostics. Every stage is testable on its own.
2. **Proper diagnostics.** Every error — frontend or checker — is a value with
   a span and a message; bad input never panics. Checker failures keep the
   expected/found wording and the `?a` flow, rendered by the same pretty
   printer as the CLI output (§5).
3. **No changes to the type layer.** The checker, the IR, and the lowlevel VM
   are used as-is; the frontend is a pure producer of the `IR`.

## 2. Syntax

```
program  := (stmt sep)* expr                        -- statements, then the final expression
stmt     := ['let'] name '=' expr                   -- binding (block-wide by default; `let` restrictive)
          | expr                                    -- bare expression statement
sep      := newline | ';' | ','                     -- one uniform Separator token

expr     := lambda
lambda   := annotated ('=>' expr)?                  -- lambda; right-assoc; lhs is a (possibly annotated) name
annotated:= arrow ((':' arrow) | ('#' arrow) | ('?' arrow))*   -- type (':'), perspective ('#'), and/or doc ('?') annotation, right-assoc
arrow    := cmp ('->' cmp)*                         -- function type; right-assoc
cmp      := arith (('<=' | '==') arith)*            -- comparison, left-assoc; yields 0/1
arith    := prefix (('+' | '-') prefix)*            -- arithmetic, left-assoc
prefix   := '!' apply | apply                       -- prefix assert: `!e` asserts `e`; tighter than binary ops
apply    := atom atom*                              -- application; left-assoc, tightest
atom     := primary postfix*                        -- a primary, then glued postfix forms
primary  := int_literal
          | 'Int' | 'Type'                          -- the two type constants
          | '_'                                     -- inference placeholder (type position only)
          | name
          | '(' expr ')'                            -- grouping (transparent)
          | '(' expr (sep expr)* sep? ')'           -- tuple  (TypeTuple in type position)
          | '[' element (sep element)* ']'          -- array literal
          | 'table' '{' pair (sep pair)* '}'        -- constant table literal
          | '{' block '}'                           -- block: statements, then the block's value (or a struct-returning block)
          | '<' expr (sep expr)+ '>'                -- tuple type  (always TypeTuple; >= 2 elements)
          | 'struct' '<' sfield (sep sfield)* '>'     -- struct type  (nominal, optional field names)
          | 'if' expr 'then' expr 'else' expr       -- conditional
block    := (bstmt sep)* ['return' expr]            -- statements + an explicit tail (`return` anywhere)
          | (bstmt sep)*                            -- struct-returning block (no tail): an anonymous struct
bstmt    := ['pub'] stmt                            -- a block statement (`pub` marks one as a struct field)
postfix  := glue ( '[' expr ']'                     -- index  e[i]
                 | '<' expr '>'                     -- array type  T<e>
                 | '{' expr '}'                     -- table lookup  t{k}
                 | '(' fields ')' )                 -- field read  a(k)  or instantiation  A(…)
           | '.' name                               -- named field read  a.name
element  := '~'n? expr                              -- shallow marker (inside array literals only)
pair     := expr '::' expr                          -- table entry: deep-equal key :: value
sfield   := '.' name expr                           -- named struct field  (a leading '.' marks it)
           | expr                                   -- unnamed (positional) struct field
fields   := (expr (sep expr)* sep?)?                -- instantiation/field-read paren content
```

- **Keywords:** `Int`, `Type`, `struct`, `table`, `let`, `if`, `then`, `else`,
  `return`, `pub`, `=>`, `->`, `:`.  `=` binds a name in a statement; `#`, `?`,
  `::`,
  `~`, `!`, and the
  operators `+ - <= ==` are punctuation.  A binding is **block-wide** by
  default (its name is in scope throughout the block, forward and backward, so
  it may reference and recurse with the block's other bindings) and gets the
  restrictive, sequential form with `let`.  A statement separator is any of
  newline, `;`, or `,` — they are interchangeable, the lexer produces the same
  `Separator` token for all three, and the quantity never matters.  `{` `}`
  delimit a block (a program-shaped expression).  **There are no comments:** the
  lexer never skips any text, so prose lives in the file's leading `@{...@}`
  preprocessor block as metadata strings (see §2.2).  Whitespace (space/tab/cr)
  is trivia; `@` is reserved for the block delimiters and cannot appear in code
  or in a string.
- **Newlines, semicolons, and commas are all one separator, and a separator is
  never whitespace.**  `\n`, `;`, and `,` lex to the same `Separator` token, and
  their quantity is irrelevant: `a = 1\nb = 2\na`, `a = 1; b = 2; a`, and
  `a = 1, b = 2, a` all mean the same thing, and a run of them (a blank line,
  a stray trailing separator) is tolerated.  The same separator separates the
  elements of a tuple, array, or struct, so `(a, b)`, `(a; b)`, and a newline
  between the elements are the same tuple.  The flip side is that an expression
  cannot continue across a separator: a lambda body must start on the same line
  as `=>` (`x =>\n  x + 1` is a parse error), and a tuple or array cannot be
  broken across lines without parens.
- **Names:** lowercase or mixed-case identifiers (`x`, `id`, `n2`).  `Int`,
  `Type`, and `struct` are reserved — they cannot be bound or used as names.
- **The `_` placeholder.**  In type position (the right side of `:`, and the
  components of the type forms under it), `_` is an inference placeholder:
  the checker infers the type from context — `x : _`, `x : Int -> _`,
  `x : Int<_>`, `x : <Int, _>`, `struct<Int, _>`.  Outside type positions
  `_` is an ordinary name, so `_ = 5; _` and `_ => _` stay legal.
- **Integers:** non-negative decimal literals (`0`, `42`); a literal that
  overflows `usize` is a lex error.
- **Precedence** (loosest → tightest): `=>` → `:` / `#` / `?` → `->` → `<=` / `==`
  → `+` / `-` → `!` prefix → application → postfix (glued delimiters) → atoms.  `x => e : T`
  parses as `x => (e : T)` — lambda bodies extend through annotations, as do
  array lengths: `Int<x : T>` is the array type whose length is the annotated
  expression.  `#` and `?` bind at the same precedence as `:`, so
  `e : T # p ? d` annotates the type, perspective, and doc slots, and
  `1 # 4 + 2 # 6` is `(1 # 4) + (2 # 6)`.  `?` is the **label** (doc) slot:
  metadata that never constrains, so `e ? d` attaches a value (a user struct
  instance) and — unlike `#`, which a compound lives with and the apply-time
  check enforces — the attribute's own `is_subtype` (a doc returns `true`)
  allows a later `? d'` to override an earlier `? d` without conflict (a label
  contributes no apply-time constraint slot).  A constraint annotation (`# p`)
  over a value that already carries one **replaces** the slot with `p` (the
  *requirement*, a subtype) and validates it against the value's existing
  attribute (the *provider*, a supertype): `(x # 8) # 4` checks (`4 | 8`) and
  the value becomes `# 4`; `(x # 4) # 8` does not.  An annotation replaces
  **only the slots it spells** and preserves the rest — `(x # 8 ? doc) # 4`
  re-checks the perspective and keeps the doc, `(x # 8 ? a) ? b` keeps the
  perspective and replaces the doc.  A comparison
  (`<=` / `==`) yields `0` or `1`, driving an `if` branch.  `!`
  is a prefix assert: `!e` compiles to the highlevel `assert(e)` — the checker
  force-evaluates `e` and requires `USize(1)`.  It binds tighter than the binary
  operators but looser than application, so `! f x` asserts `f x` and `! x <= 3`
  is `(!x) <= 3`; assert a comparison by parenthesizing it (`!(x <= 3)`).
- **Annotated parameters.**  `x : T => e` is a lambda whose parameter is
  annotated with `T` — the frontend desugars it to `x => { x : T; e }`, so the
  annotation is a leading body statement that unifies the parameter's slot in
  body scope (so a `T` referring to `x` itself is in scope, e.g. `x : x -> Int`)
  while the codomain is inferred from the body.  Likewise `x # n => e` desugars
  to `x => { x # n; e }` — the parameter's perspective slot, checked at each
  apply against the argument's perspective.  The body still extends maximally:
  `x : T => e : U` is `x : T => (e : U)`.  `x : T` without a following `=>`
  stays an ordinary annotation.
- **One grammar, one position flag.**  Types are expressions, so term and type
  forms share one grammar.  The parser threads a *type-mode* flag that only
  decides `(a, b)` — a `Tuple` value in term position, a `TypeTuple` type
  expression in type position (after `:`).  Angle brackets need no flag:
  `<a, b>` is always a `TypeTuple`, in both positions.

- **Conditionals.**  `if cond then e1 else e2` is an expression: `cond` is any
  expression up to `then` (the keyword delimits it — it is neither an atom nor
  an infix operator, so the condition cannot extend through it), and the
  branches extend maximally like a lambda body.  It desugars to the lazy index
  `[e2, e1][cond]` — the condition (`0`/`1`) selects the branch, and the
  untaken branch is never evaluated.
- **Tables.**  `table { k1 :: v1, k2 :: v2, … }` is a constant table literal;
  each entry is a deep-equal `key :: value` pair (the double colon is not part
  of the expression grammar, so it unambiguously separates the pair).
  `table {}` is the empty table.  `t{k}` (a glued `{`) is a table lookup
  returning the entry whose stored key is deep-content-equal to `k`.
- **Shallow markers.**  Inside an array literal, an element may be prefixed
  with `~` (`~e`, `~2 e`): a *shallow* marker that keeps the value slot at
  each of the first `n` levels of the element's type spine shallow (a bare
  `~` marks the whole subtree).  `~` is accepted nowhere else.

### 2.1 Delimiters, postfix forms, and adjacency (Glue)

Brackets `[ ]` and parens `( )` build values; angle brackets `< >` build
types.  `[1, 2]` is an array value and `(1, 2)` a tuple value; `T<3>` is the
array type of length 3, `<Int, Type>` the tuple type, and
`struct<Int, Type>` a nominal struct type.

All four postfix delimiters — `(` `{` `<` `[` — are **postfix-only when glued**
to the preceding token.  The lexer emits a zero-width `Glue` token immediately
before any of them that is adjacent (no trivia between) to the previous
token; the parser reads `Glue` to decide postfix versus application.  A spaced
delimiter is a fresh atom — an argument of an application:

- `a[0]` (glued `[`) is an index; `a [0]` (spaced `[`) **applies** `a` to the
  array `[0]`.  `a[0][1]` chains, and `[e1, e2][i]` is the language's only
  conditional form — with an integer index it selects a branch.  An array
  literal in argument position needs no parens when glued, but a spaced
  `f ([1, 2])` applies `f` to the array.
- `Int<3>` (glued `<`) is the array type of length 3; `f <3>` (spaced `<`)
  applies `f` to `3`.  A type tuple in argument position is parenthesized:
  `f (<Int, Type>)`.  A single-element `<Int>` is a parse error — `(Int)`
  is the grouping form.
- `A(1, 2)` (glued `(`) is a struct instantiation (see §3); `f (1, 2)` (spaced
  `(`) applies `f` to the tuple.  A glued `(` after a container is a field/
  slot read (`a(0)`).  A fresh atom may itself open with a glued delimiter,
  e.g. the annotation `x :(Int, Type)`.
- `t{k}` (glued `{`) is a table lookup; `t {k}` (spaced `{`) applies `t` to
  `{k}`.  A table literal (`table{…}`) and a struct type (`struct<…>`) are
  *keyword-led*, so their delimiter sits directly after the keyword.

### 2.2 The `@{...@}` preprocessor block

A file may open with a single `@{...@}` block — once, before any code; a
non-`@` prefix is allowed and ignored.  It is cut out of the source by a pure
byte scan (independent of the lexer), so the language lexer/parser never see
it.  Inside the block is a set of statements, Separator-separated:

- `name = import "path"` loads a package bound to `name` (the import namespace).
- `name = "value"` defines a string metadata entry (the metadata namespace);
  the two namespaces are separate.

A string is `"…"` with no escape characters and may span newlines; its content
is any character except `"` or `@`.  `@` is reserved for the block delimiters,
so it cannot appear in the surrounding code or inside a string.  The block
carries `order` / `output` / prose for the README tooling (see
`crates/lichen-language/src/readme.rs`).  The code to compile is the source
after the block (or the whole source when there is no `@`); the preprocessor
returns that borrowed slice plus a base byte offset so the lexer maps every
span back to the original file.

## 3. Semantics

- **Programs are pure expressions.**  The checker compiles the IR, runs the
  definition pass (so apply-time type checks fire), and the program's value is
  the evaluation of its root: an `Int`, a tuple, an array, a function, or a
  type expression.
- **Statements and bindings.**  A program is a sequence of statements — a
  `name = expr` binding or a bare expression — followed by a final expression,
  each statement ended by a `Separator` (newline, `;`, or `,`).  A binding is *graph sharing*, not
  sugar: its value compiles once into the IR arena, and every use of the name
  is that same node id (the IR is a graph).  There is no `let` node and no
  desugared lambda — the final expression stays the program's root, so its
  type is determinable exactly like a bare program's.  A binding is
  **block-wide** by default: its name is in scope throughout the block —
  forward *and* backward — so a value may reference its own name, a later
  binding, or any other binding of the block, and may recurse with them.  The
  frontend reserves a placeholder id per block-wide binding, enters all the
  names before compiling any value, then fills each placeholder with its
  value's node — so a self/mutual reference makes the IR a cycle there (the
  graph contains a back-edge), which the checker totalizes by pre-registering
  a skeleton pair and binding the skeleton's cells to the finished pair (no
  recursion, no overflow).  `let name = expr` is the *restrictive* form: the
  value compiles before the name enters scope, so the name is visible only to
  later statements (`let a = a` resolves `a` to the outer binding — the
  sequential, non-recursive case).  Sharing means a bound *non-function* value
  has one type across uses, while a bound lambda stays polymorphic — each
  application still instantiates the parameter fresh via the runtime's
  per-apply clones.  A **bare expression
  statement** (`5; 7`, `f 5` before more statements) is no dead code: the
  frontend wires every statement into the root (`Index(Tuple([stmt₁, …,
  stmtₙ, final]), n)`), so each is checked and evaluated — the runtime *is*
  the typechecker — and only the final expression is the program's value.
- **Blocks.**  `{ stmt …; expr }` is an expression: the same
  statement list as a program (separators again a `Separator`, bare
  expression statements included), scoped to
  the block.  Bindings are block-wide inside it
  exactly as at the top level (each value compiles once, and a use of the name
  is the value's own node) and shadow outer names; the block's names are gone
  after the `}`.  Because a block is an expression, a function body can be one
  naturally — `f = x => { a = 1; a }` (or, newline-separated, a body written
  as a multi-line block) — and so can any other subexpression
  (`{ a = 5; a }` as a program, an argument, a nested block).  A block
  compiles to its final expression's own node: there is no block node in the
  IR, so a bound lambda inside a block stays polymorphic and a block never
  monomorphizes its contents.  `{}` (no final expression) is a parse error,
  like a program without one.
  The block's value is its **tail expression**.  A trailing expression (a bare
  statement whose next token is the `}`) is the tail; an explicit `return
  expr` anywhere in the block is also the tail, so a value can be pinned
  before or among the statements (`{ a = 1; return 2 }`).  A block whose last
  statement is a binding (and with no `return` anywhere) has **no tail**, and
  instead parses as a **struct-returning block**: its value is an anonymous
  struct instance whose fields are the block's statements — a `name = value`
  binding is a named field, a bare expression a positional one, a `let`
  binding is a block-local and never a field, and a `pub`-marked statement is
  a field (when any statement is `pub`, only the `pub` ones are).
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
- **Indexing.**  `e[i]` reads the `i`-th element of an array, tuple, or
  struct instance (a struct instance's positional fields are its wrapped
  tuple's elements).  A
  literal index into a statically-known array is checked against its length
  at check time (an out-of-bounds index is an `IndexOutOfBounds`
  diagnostic); an index known only at runtime (a parameter, a call result)
  is checked when evaluated.  Indexing a *concretely* non-indexable type —
  a function or an atomic type — is an `IndexTarget`
  diagnostic at check time, not a runtime panic (mirroring the apply
  guard).  `[then, else][i]` is the language's only
  conditional form — an integer index selects a branch, and the untaken
  branch is never evaluated (the lowlevel `Index` stays lazy on it).
- **Nominal struct types.**  `struct<T1, ..., Tn>` is a *new type* with
  positional fields; a field may carry an optional name prefix (`.name`), so
  `struct<.x Int, .y Type>` names its fields.  The leading `.` unambiguously
  marks a named field — the language-server-friendly discriminator, since a
  field name and a field-type expression (both identifiers) can never be
  confused while the user is typing.  The names are stored on the struct type
  as a name→index table, in the second field of the struct's **two-field
  marker** (`TypeStruct{id, names}`, the kind's marker slot), which lets a
  `a.name` read resolve a field by name.  Its kind is a standard `[marker, K]`
  pair whose marker is that two-field value.  The kind also holds a **fresh
  nominal id** — each occurrence of the syntax allocates a new id, so two
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
  non-function is a VM panic, not a diagnostic).  Indexing an instance reads
  its positional fields: `s(1, 2)[0]` is the first field, and its type is
  the corresponding field type (an out-of-bounds field index is an
  `IndexOutOfBounds` diagnostic).  A struct instance with named fields also
  reads by name: `a.x` resolves `x` through the struct's name→index table to
  the field's positional index (a `a.x` on a struct without that field is a
  `NamedField` diagnostic; a `a.x` on a non-struct is an `IndexTarget`
  diagnostic).  Values of struct type beyond the wrapped
  tuple are future work.
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
| `x => e` | `Function { parameter, parameter_type: None, parameter_attribute: None, return }` — `parameter` is the `Parameter` expr for `x` |
| `x : T => e` | `Function { parameter, parameter_type: Some(compile(T)), parameter_attribute: None, return }` — the annotated parameter's type, compiled in body scope (the §4.2 desugar kept as an optimization) |
| `x # n => e` | `Function { parameter, parameter_type: None, parameter_attribute: Some(compile(n)), return }` — the annotated parameter's perspective, also body-scope |
| `e1 e2` | `Apply { function, argument }` |
| `e[i]` | `Index { array, index }` |
| `e : T` | `Annotation { value, type: Some(compile(T)), attribute: None }` |
| `# p` / `e : T # p` | `Annotation { value, type: Some(compile(T))?, attribute: Some(compile(p)) }` — the annotated node's schema gains the `[Perspective]` tail |
| `_` (type position) | `Placeholder` |
| `T1 -> T2` | `TypeFunction { parameter, return }` (domain, codomain) |
| `(e1, …, en)` | `Tuple(range)` |
| `<T1, …, Tn>` | `TypeTuple(range)` |
| `struct<T1, …, Tn>` / `struct<.a T1, .b T2>` | `TypeStruct { fields, names }` — nominal, fresh id per occurrence; the kind is a `[marker, K]` pair whose marker is the two-field `TypeStruct{id, names}` value |
| `a.name` | `NamedField { container, name }` — the checker resolves `name` through the struct's name→index table to the positional index, then reads like `a(k)` |
| `s(1, 2)` (callee a struct type) | `Instantiate { type_expr, value }` |
| `[e1, …, en]` | `Array(range)` |
| `T<e>` | `TypeArray { element_type, length }` |
| `{ a = e; …; e }` | the final expression's own node — statements are scope-entered (bindings), then popped; a non-final statement list is wired into the root as `Index(Tuple([…, e]), n)` |
| `{ x = 1; …; y = 2 }` (no tail) | `RecordBlock { fields }` — a struct-returning block; each field carries an optional name, its value, a `pub` mark, and a `field` flag (false for a `let` local) |
| `{ …; return e }` | `Block { statements, expr: e }` — the `return` expression is the block's tail |

There is no desugar step: bindings are graph sharing, and a block-wide
binding reserves a placeholder id, compiles its value, then fills the id with
the value's node — so a self/mutual reference makes the IR a cycle there,
which the checker totalizes with a skeleton pair.

### Name resolution

A scope stack of `name → ExprId`.  Compiling `x => e`:

1. allocate the `Parameter` expression (span = `x`'s span),
2. push `x` onto the scope stack,
3. compile `e`,
4. pop, then allocate `Function { parameter, return }`.

A use of `x` in `e` therefore *is* the parameter's own `ExprId` — the checker's
scope stack is keyed by it, and the IR carries no name strings.  A block-wide
binding `a = e` resolves differently: the frontend first reserves a
`Placeholder` id for every block-wide binding of the scope, enters all the
names in one frame, then compiles each value — so a value may reference a
binding defined *later* or *itself*.  A use of `a` in a value resolves to the
reserved id; once the value compiles, the id is filled with the value's node
(a bare `a = b` aliases `b`'s node instead).  A restrictive `let a = e`
compiles the value first and pushes `a` afterwards, so the name is visible
only to later statements.  A block `{ a = e; …; e }` does the same and pops
its scope frames (truncates) at the `}` — inside, the block's names shadow
outer ones; after the `}`, the outer names are back.  Shadowing is allowed
(the inner binding wins).  A name in no scope is a **resolve diagnostic** at
the name's span — the checker's `lookup` panics on unresolved ids, so the
frontend guarantees resolution before the IR leaves the crate.

## 5. Diagnostics

### Stages

| stage | example |
|---|---|
| Preprocess | `cannot load package 'inner.lichen': unresolved name 'y'` |
| Lex | `unexpected character '@'` |
| Parse | `expected ')', found ']'` |
| Resolve | `unresolved name 'y'` |
| Check | `expected Int -> Int, found Int` (+ the `?a` flow lines) |

A `Diag { span: Option<(u32, u32)>, message: String, stage: Stage, check: Option<Box<...>> }`.
The `message` is the rendered form for display (`render`); `stage` says which
pipeline stage produced it.  Checker diagnostics additionally carry their
structured facts in `check` — the highlevel `Diag` (`kind`, the conflicting
classes `a`/`b` and their recorded values, `span`) — which tests and tooling
match on instead of the message; frontend errors leave it `None`.  The
frontend *recovers*: lex errors accumulate (an unexpected character is skipped
and lexing continues), the parser skips a broken statement and reports it,
and the checker still runs on the resulting partial program — so one pass
reports every problem it can find.  Only an unresolved name (the resolve
stage) stops the pipeline, since no IR exists to check.  Checker diagnostics
can be many, in order.

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

Checker messages are rendered by the same printer as the CLI output, so
types appear in the language's own syntax: `Int`, `Type`, `T1 -> T2`,
`<T1, T2>`, `T<len>`, `struct<...>`, and unbound cells as stable `?a`,
`?b`, … names (cells in one unification class share a name).  The boxed
highlevel `Diag` in `check` stays raw — it carries the structured facts
(`kind`, the classes `a`/`b` and their values, the `error_index` into
`unify_errors`) and its own raw message (`TypeInt`, `TypeType`, …).

