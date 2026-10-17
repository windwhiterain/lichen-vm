# Language spec (v1)

> Status: current — the single source of truth for the lichen source language.
> Owned by [`crates/lichen-language`](../crates/lichen-language) together with
> the lexer/parser crates
> ([`lichen-language-lex`](../crates/lichen-language-lex),
> [`lichen-language-parser`](../crates/lichen-language-parser)). This is the one
> retained early document; feature notes describe the surrounding system and refer
> here for syntax ([doc index](README.md)).

*A minimal source language that compiles to the highlevel IR
([`lichen-highlevel`](../crates/lichen-highlevel)) and produces proper diagnostics.
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
program  := (bstmt sep)* [expr]                      -- a block body: statements, then an optional tail (the top level is a block)
stmt     := ['cache'] ['let'] name '=' expr         -- binding (block-wide by default; `let` restrictive; `cache` a retained cell)
          | expr                                     -- bare expression statement
bstmt    := ['pub'] stmt                             -- a (possibly `pub`) statement
sep      := newline | ';' | ','                      -- one uniform Separator token

expr     := lambda
lambda   := annotated ('=>' expr)?                  -- lambda; right-assoc; lhs is a (possibly annotated) name
annotated:= arrow ((':' arrow) | ('#' arrow) | ('!' arrow) | ('?' arrow) | ('~' str_literal))*   -- type (':'), perspective ('#'), refinement ('!'), doc ('?') and/or mark ('~') annotation, right-assoc; each wraps the whole expression to its left
arrow    := find ('->' find)*                        -- function type; right-assoc
find     := cmp ('<==' cmp)*                         -- table read; left-assoc, the loosest binary level
cmp      := bits (('<' | '>' | '<=' | '>=' | '==' | '!=' | '@in') bits)*   -- comparison and set membership, left-assoc; yield 0/1
bits     := bitxor ('|' bitxor)*                    -- bitwise or
bitxor   := bitand ('^' bitand)*                    -- bitwise exclusive or
bitand   := sum ('&' sum)*                          -- bitwise and
sum      := product (('+' | '-') product)*          -- arithmetic, left-assoc
product  := prefix (('*' | '/' | '%') prefix)*      -- product, left-assoc; tighter than '+'/'-'
prefix   := '@assert' apply
          | ('int2float' | 'float2int' | 'exp' | 'sqrt' | 'floor') apply
                                                  -- one operand, by juxtaposition
          | ('max' | 'min') atom atom                       -- two operands, by juxtaposition
          | apply
                                                  -- the prefix assert, the two class crossings and the four scalar keywords exp/sqrt/floor/max/min; tighter than every binary operator, looser than application
apply    := atom atom*                              -- application; left-assoc, tightest
atom     := primary postfix*                        -- a primary, then glued postfix forms
primary  := int_literal
          | str_literal                           -- "…" (no escapes; the builtin `string` value)
          | 'Int' | 'string' | 'Type'             -- the three type constants
          | '_'                                     -- inference placeholder (any position)
          | name
          | '$' name '(' expr (sep expr)* sep? ')'  -- native-operator call  $name(args…)
          | '(' expr ')'                            -- grouping (transparent)
          | '(' expr (sep expr)* sep? ')'           -- tuple value (always a Tuple)
          | '[' element (sep element)* ']'          -- array literal
          | 'table' '(' pair (sep pair)* ')'        -- constant table value
          | 'set' '(' expr (sep expr)* ')'          -- set value (members are values; the instance is a Set, not an array)
          | '{' block '}'                           -- block: statements, then the block's value (or a struct-returning block)
          | 'tuple' '{' expr (sep expr)* sep? '}'   -- tuple type: a *slot* per element
          | 'struct' '{' sfield (sep sfield)* '}'   -- struct type  (nominal, optional field names)
          | 'array' '{' expr sep expr '}'           -- array type  (element type, then length)
          | 'set' '{' expr sep? '}'                 -- set type  (the one element slot)
          | 'table' '{' expr sep expr sep? '}'      -- table type  (the key slot, then the value slot)
          | 'if' expr 'then' expr 'else' expr       -- conditional
block    := (bstmt sep)* ['return' expr]            -- statements + an explicit tail (`return` anywhere)
          | (bstmt sep)*                            -- struct-returning block (no tail): an anonymous struct
bstmt    := ['pub'] stmt                            -- a block statement (`pub` marks one as a struct field)
postfix  := glue ( '[' expr ']'                     -- index  e[i]
                 | '{' expr '}'                     -- type access  X{n}  (a tuple type value's component)
                 | '::' name                        -- named type access  X::a  (a struct type value's field)
                 | '(' fields ')' )                 -- field read  a(k)  or instantiation  A(…)
           | '.' name                               -- named field read  a.name
element  := '@shallow' glued '(' n ')' expr           -- shallow marker, bare or with a depth (inside array literals only)
pair     := expr '==>' expr                         -- table entry: deep-equal key ==> value
sfield   := '.' name expr                           -- named struct field  (a leading '.' marks it)
           | expr                                   -- a bare expression statement: never a struct field
fields   := (farg (sep farg)* sep?)?             -- instantiation/field-read paren content
farg     := '.' name expr                         -- named instantiation argument  .x 1
           | expr                                 -- positional argument
```

- **Keywords:** `Int`, `Float`, `string`, `Type`, `tuple`, `struct`, `array`, `table`, `set`, `let`, `if`, `then`,
  `else`, `return`, `pub`, `cache`, `int2float`, `float2int`, `max`, `min`, `exp`, `sqrt`, `floor`, `=>`, `->`, `:`.  The
  `@`-led keywords — `@loop`, `@assert`, `@shallow`, `@gc`, and the membership operator `@in` — are
  reserved words too (see the `@` note in §2).
  The
  seven prefix keywords open a prefix expression and so cannot be used as
  names; the rest are reserved as words.  `=` binds a name in a statement; `#`, `?`,
  `$`, `::`, `==>`, `<==`, `~`, `!`, and the
  operators `+ - * / % < > <= >= == != & | ^` are punctuation.  A binding is **block-wide** by
  default (its name is in scope throughout the block, forward and backward, so
  it may reference and recurse with the block's other bindings) and gets the
  restrictive, sequential form with `let`.  A statement separator is any of
  newline, `;`, or `,` — they are interchangeable, the lexer produces the same
  `Separator` token for all three, and the quantity never matters.  **The top
  level is itself a block**: a program is a block body (statements + an optional
  tail expression), terminated by the end of the input — the end of the input is
  *not* a separator, it just ends the body.  `{` `}`
  delimit a block (a program-shaped expression).  **There are no comments:** the
  lexer never skips any text, so prose lives in the file's leading `---...---`
  preprocessor block as metadata strings (see §2.2).  Whitespace (space/tab/cr)
  is trivia; `@` is reserved for the block delimiters and cannot appear in code
  outside a string.
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
- **Names:** lowercase or mixed-case identifiers (`x`, `id`, `n2`).  The keywords
  (`Int`, `Float`, `string`, `Type`, `tuple`, `struct`, `array`, `table`, `set`, `let`, `if`, `then`,
  `else`, `return`, `pub`) are reserved — they cannot be bound or used
  as names.
- **The `_` placeholder.**  `_` is an inference placeholder hole in *any*
  position — type and value alike.  In type position (the right side of `:`,
  and the components of the type forms under it) it infers the type from
  context — `x : _`, `x : Int -> _`, `x : array{Int, _}`, `x : tuple{Int, _}`,
  `struct{.f Int, .g _}` — and in value position it is a typed hole: `_ : Int`
  checks as an underdetermined value of type `Int`, and `f _` / `(1, _)`
  leave a hole the context unifies.  `_` is **never a name**: it cannot be
  bound (`_ = 5`) or used as a lambda parameter (`_ => e`) — both are parse
  errors.  The discard idiom is gone; use a real name.
- **Integers:** non-negative decimal literals (`0`, `42`); a literal that
  overflows `usize` is a lex error.
- **Strings:** `"…"` with no escape characters, and it may span newlines; the
  content is any character except `"`.  An unterminated string is a lex error.
  A string is the immutable builtin `string` value — atomic in this universe,
  exactly like an integer: there is no mutation, indexing, or concatenation.
- **Precedence** (loosest → tightest): `=>` → `:` / `#` / `?` → `->` →
  `<=` / `==` / `!=` / `<` / `>` / `>=` → `|` → `^` → `&` → `+` / `-` →
  `*` / `/` / `%` → `!` prefix → application → postfix (glued delimiters) → atoms.  `x => e : T`
  parses as `x => (e : T)` — lambda bodies extend through annotations, as do
  array lengths: `array{Int, x : T}` is the array type whose length is the annotated
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
  yields `0` or `1`, driving an `if` branch; the bitwise operators nest C-style
  (`&` in `^` in `|`) but bind **tighter** than a comparison, so `a & b == c` is
  `(a & b) == c`.  `@assert`
  is a prefix assert: `@assert e` compiles to the highlevel `assert(e)` — a side
  constraint, not a unify.  The checker deep-evaluates `e` after the
  definition pass and requires `USize(1)`; a condition
  that stays undecided — an undecided parameter, or a computation sitting behind a
  lazy marker — is not triggered, and the apply
  clone re-checks the instantiated condition per call.  The expression
  itself *is* the condition — an assert checks its subject, it does not
  replace it — so `@assert e`'s value and type are `e`'s.  It binds tighter than the binary
  operators but looser than application, so `@assert f x` asserts `f x` and
  `@assert x <= 3` is `(@assert x) <= 3`; assert a comparison by parenthesizing
  it (`@assert (x <= 3)`).  The keyword replaced the `!` sigil, which now marks
  a **refinement annotation** (`e : T ! p`) — see
  [operator-polymorphism](notes/operator-polymorphism.md) §3.
- **A refinement, and what it refines.**  `e ! p` attaches a predicate to `e`'s
  own **value**: the checker applies `p` to that value and requires the result to
  be `1` (the assert channel), so `x ! (v => v > 3)` constrains the value.  The
  predicate may instead be written **on a type**, inside the type position:
  `x : (T ! p)` makes `T`'s own value — the *type* — the predicate's argument, so
  `x : (_ ! in_num)` refines the **class** a parameter is used at, with `p`
  receiving the type value and no type read of the value needed.  A refinement
  written on a type survives **anywhere a type expression is consumed** — the
  annotation chain, an annotated parameter, and a compound type's element, field,
  or function-type side — so `x : tuple{(_ ! in_num), (_ ! in_num)}` refines both
  components of a 2-tuple, and each is enforced (`f ("a", 2)` is refused).  That is
  where a *class* contract belongs (`Num = set(Int, Float); in_num = t => t @in
  Num`), and the type it names is the type expression's **denotation** — the
  annotated expression's own term — so the annotation binds the position's type to
  the type, not to the `[type, …, attribute]` group the attribute lives in.  The
  refinement is enforced where it was written: the type expression's own assert
  rides the enclosing function, so an *open* class is re-checked per application
  (`Int` and `Float` pass, a `string` is refused) while a *concrete* one is
  decided at the definition.  An open class's annotated type is the placeholder's
  `[shape, kind]` pair of cells, so a printed signature shows it as the printer's
  honest raw mark — `raw[?a, ?b] -> …` — where `x : (Int ! in_num)` prints
  `Int -> …` ([raw-rendering-mark](notes/raw-rendering-mark.md)).
- **Annotated parameters.**  `x : T => e` is a lambda whose parameter is
  annotated with `T` — the frontend desugars it to `x => { x : T; e }`, so the
  annotation is a leading body statement that unifies the parameter's slot in
  body scope (so a `T` referring to `x` itself is in scope, e.g. `x : x -> Int`)
  while the codomain is inferred from the body.  Likewise `x # n => e` desugars
  to `x => { x # n; e }` — the parameter's perspective slot, checked at each
  apply against the argument's perspective; the IR keeps it as a parameter
  field (`parameter_attribute`) so the apply can compare the argument's own
  attribute against the declared one.  A **refinement** parameter, `x ! p => e`,
  takes the same desugar *without* the optimization — `x => { x ! p; e }` — which
  is the general form and needs no IR field: the annotation rule then registers
  the predicate's assertion on the function being built, so an apply clone
  re-checks it against the call's argument
  ([operator-polymorphism](notes/operator-polymorphism.md) §3).  The body still
  extends maximally:
  `x : T => e : U` is `x : T => (e : U)`.  `x : T` without a following `=>`
  stays an ordinary annotation.
- **One grammar, no type mode.**  Types are expressions, so term and type forms
  share one grammar, and there is no *type-mode* flag: `expr : expr` parses both
  sides the same way.  `(a, b)` is always a `Tuple` *value*; the tuple *type* is
  the keyword-led `tuple{a, b}`, a `TypeTuple` in every position — so a tuple type
  is written `x : tuple{Int, Int}`, never
  `x : (Int, Int)` (the latter is a tuple value whose elements are the type-values
  `Int`).
- **A tuple type's elements are *slots*.**  The type is a list of `[value, type,
  attributes…]` terms, and the braced form writes the list directly: `tuple{Int # 2}` is
  `tuple{@any : (Int # 2)}` — an element with no top-level `:` is a *type
  expression*, and its value position is the wildcard `@any`, which unifies with
  anything and constrains nothing.  An element with a top-level `:` spells the
  value position too, which is a **pin**: `tuple{5 : Int}` accepts `(5, 2)` and
  refuses `(6, 2)`, and because the pin rides in the type value it keeps holding
  through a binding (`t = tuple{5 : Int, Int}; (6, 2) : t` is refused).  An
  attribute written on an element (`tuple{Int # 2, Int}`) is a *requirement*: the
  value's own attribute at that position must be authorized by it, exactly as a
  whole annotation's must (`(1 # 4) : (Int # 2)` holds, `(1 # 2) : (Int # 4)`
  does not).  See `docs/notes/compound-element-slots.md`.

- **Conditionals.**  `if cond then e1 else e2` is an expression: `cond` is any
  expression up to `then` (the keyword delimits it — it is neither an atom nor
  an infix operator, so the condition cannot extend through it), and the
  branches extend maximally like a lambda body.  It desugars to the lazy index
  `[e2, e1][cond]` — the condition (`0`/`1`) selects the branch, and the
  untaken branch is never evaluated.
- **Tables.**  `table(k1 ==> v1, k2 ==> v2, …)` is a constant table value;
  each entry is a deep-equal `key ==> value` pair (the table arrow is not part
  of the expression grammar, so it unambiguously separates the pair).
  `table()` is the empty table.  `t <== k` is the table read
  returning the entry whose stored key is deep-content-equal to `k`.  The
  **table type** is the braced `table{K, V}` — the key slot and the value slot,
  the same shape the value form builds, so a table value checks against a
  `table{Int, string}` annotation exactly as it did against the printer's old
  `table<K, V>` notation.
- **Sets.**  `set(a, b, …)` is a set of ordinary values — a *value* form led by a
  word, exactly like the table value, because a set's members are values.  The
  members are full
  expressions and share one element type (a set is homogeneous, like an array
  literal); `set()` is the empty set.  A set's **value is its members**, and what
  makes it a set is its **type**, whose shape is the element type
  *alone*: a set has no length, so `set(a)` and `set(a, b)` are one type.  The
  **set type** is the braced `set{E}`, one element *slot* that pairs with every
  member: `set{Int}` is the set of `Int`, `set{Int # 2}` states an element
  requirement, and a pin `set{5 : Int}` accepts only the member `5`.  That is
  what separates it from `array{T, n}`: a set is not indexable (`s[i]` is a
  diagnostic — the container unify refuses it exactly as it refuses `t <== k` on an
  array) and can never flow into an `array{T, n}` parameter.  A set of *type
  values* is how a contract names the classes it admits:
  `Num = set(Int, Float)` ([operator-polymorphism](notes/operator-polymorphism.md)
  §3).
- **Shallow markers.**  Inside an array literal, an element may be prefixed
  with `@shallow` (`@shallow e`, `@shallow(2) e`): a *shallow* marker on the
  element's **value**, which stays lazy in the deep pass.  A bare `@shallow`
  keeps the whole value subtree lazy, and `@shallow(n)` keeps the value slot at
  each of the first `n` levels of the element's type spine shallow.  The mark is
  a **value** mark: it does not make the element's *type* lazy.  A marked
  element that is a call has its type applied at compile time as any other call
  does, and a tail whose own type is still open takes the type that call states
  — the coinductive reading, so an infinite structure has the type its first
  level already gave.  That holds for **both** spellings of the mark; what
  differs is the position's **declared slot**, which a bare mark takes from the
  call's own type while `@shallow(n)` keeps open, so a read under the depth's
  mark is answered by the marked levels themselves.  The depth's parens are a
  **glued** postfix, like every other postfix form in the language, so
  `@shallow (2)` is the element `(2)` and not a depth; `@shallow` is accepted
  nowhere else.  The depth is a **bound of at least 1**: a depth of `0` marks
  no level, so the mark is erased before the checker ever sees the element and
  the position is checked exactly as if the marker were absent — the resolver
  refuses that spelling, and the way to write it is a depth of at least 1 or no
  marker at all.
- **Marks (`~`).**  `expr ~ "text"` is a **mark**: compile-time metadata pinned to
  the expression it is written on.  It is not an attribute and never becomes a
  value — no pair slot, no unification, no propagation, no evaluation — and the
  payload is a string literal, never an expression, because a mark the compiler
  cannot place is not a mark.  Several marks may pin one expression
  (`e ~ "a" ~ "b"`), they accumulate, and each wraps the whole expression to its
  left exactly as `#` and `?` do.  Today the only reader is the interface claim
  of graph fusion, which spells its payload `claim:Interface(role,…)`
  ([graph-fusion](notes/graph-fusion.md) §3); the payload's meaning is that
  reader's, not the language's.
- **Native operators.**  `$name(args…)` is an atom: a `$`, a plain name, and a
  parenthesized comma-separated argument list — `$jit(f)`, `$launch(f, n)`,
  `$noop()`.  A trailing separator is tolerated, there is no named-argument
  form, and the parens are required — `$jit f` is a parse error.  The `$` makes
  it a distinct atom: `$name` is not a name, so
  it can be neither bound nor referenced without the prefix, and what a
  `name = $op(a)` statement binds is the *result*.  `name` resolves only
  against the private native-operator registry of the module being compiled,
  which is empty for an ordinary file — so a `$` form is written only in a
  plugin's own embedded source, and two plugins each registering `$jit` never
  collide.  The checker compiles the arguments and adopts the `[value, type]`
  pair the plugin's operator returns; it has no knowledge of what the operator
  does or what its types are, so the check-and-emit is a private contract
  between a plugin and its own source.

### 2.1 Delimiters, postfix forms, and adjacency (Glue)

Brackets `[ ]` and parens `( )` build values; the keyword-led `tuple{…}`,
`struct{…}` and `array{…}` build types.  `[1, 2]` is an array value and
`(1, 2)` a tuple value; `array{Int, 3}` is the array type of length 3,
`tuple{Int, Type}` the tuple type, and `struct{.f Int, .g Type}` a nominal
struct type.

All four postfix delimiters — `(` `{` `[` `::` — are **postfix-only when
glued** to the preceding token.  The lexer emits a zero-width `Glue` token immediately
before any of them that is adjacent (no trivia between) to the previous
token; the parser reads `Glue` to decide postfix versus application.  A spaced
delimiter is a fresh atom — an argument of an application:

- `a[0]` (glued `[`) is an index; `a [0]` (spaced `[`) **applies** `a` to the
  array `[0]`.  `a[0][1]` chains, and `[e1, e2][i]` is the mechanism under the
  conditional form (`if` desugars to it) — with an integer index it selects a
  branch.  An array
  literal in argument position needs no parens when glued, but a spaced
  `f ([1, 2])` applies `f` to the array.
- `X{n}` (glued `{`) is a **type access**: component `n` of `X`'s *value*, read
  from a **tuple type value** (see §3).  It reads a component of a type-as-value
  (`tuple{Int, string}{0}`), and the container's *type* must be the tuple kind.  A
  struct type value's kind is `TypeStruct` (its components read by name,
  `X::a`), a tuple *value*'s type is the tuple shape `tuple{Int, Int}` rather than
  the kind, and an atomic type's kind is `Type` — none of the three is accepted,
  and the requirement is stated as a unify, so an undecided container is refused by
  the apply that binds it.  The index is a full expression (`X{n + 1}`).  A
  **spaced** `{` is never a delimiter: `X {0}` applies `X` to the block `{0}`.
- **`<` and `>` are comparisons alone.**  No postfix form opens with `<`, so
  adjacency no longer changes the token's job: `a < b`, `a<b`, and `2 > 1` all
  compare, and every `>` closes nothing.
- `X::a` (glued `::`) is a **named type access**: field `a` of a **TypeStruct
  value**, whose type must itself be a TypeStruct kind (the name table lies there,
  in the marker payload at
  `container_ty[0][0][1]`).  It reads the field's *type* as a value — `struct{.a
  Int, .b string}::a` is `Int : Type`.  A decided struct type without the named
  component is refused at the read as a named-field miss — `S = struct{.a Int,
  .sub Int}` ; `S::sux` is `no field with this name in the struct type
  struct{.a Int, .sub Int}`, never a runtime table miss.  The requirement is a
  unify for **both**
  tiers: a decided non-struct container is refused where it stands
  (`expected TypeStruct, found array{Int, 2}`), and an undecided one is pinned
  and refused by the apply that binds it.  It is the
  named sibling of `X{n}`; `.` (`.a`) is the named field read over a struct
  *instance*, whose kind (not type) must be TypeStruct — a unify on the kind slot
  where the container's type is decided, and a **condition on the assert
  channel** where it is not: an undecided container (a parameter, a call result)
  is re-checked per application, so `f = x => x.a; f [10, 20]` is refused with
  `expected a struct type, found array{Int, 2}` (the refused body still records
  the runtime table read it fails).  A spaced `::` is not a
  postfix (it would be a bare infix, now ungrammatical since the table
  separator is `==>`).
- `A(1, 2)` (glued `(`) is a struct instantiation (see §3); `f (1, 2)` (spaced
  `(`) applies `f` to the tuple.  A glued `(` holding a single bare expression
  is always a field/slot read (`a(0)`), never an instantiation — the
  one-field instantiation is spelled `a(e,)`, mirroring the tuple grammar's
  comma discipline (`a()` / `a(,)` carry no fields).  A fresh atom may itself open with a glued delimiter,
  e.g. the annotation `x :(Int, Type)` — a tuple *value* of the type-values
  `Int` and `Type` (a tuple type is `x : tuple{Int, Type}`).
- `t <== k` is a **table read**: the entry whose stored key is deep-content-equal
  to `k`.  It is an **infix operator**, not a postfix form: it is the loosest
  binary level and left-associative, so its key is a whole comparison expression
  (`t <== k + 1` reads the key `k + 1`) and `t <== a <== b` reads `(t <== a) <== b`.
  A table value (`table(…)`), a set value (`set(…)`), a table type
  (`table{K, V}`), a set type (`set{E}`), a struct type
  (`struct{…}`) and an array type (`array{…}`) are *keyword-led*, so their
  delimiter sits directly after the keyword.

### 2.2 The `---...---` preprocessor block

A file may open with a single `---...---` block — once, before any code; prose
before it (a markdown header, say) is allowed and ignored.  It is cut out of the
source by a pure byte scan (independent of the lexer), so the language
lexer/parser never see it.  Inside the block is a set of statements,
Separator-separated:

- `name = import "path"` loads a package bound to `name` (the import namespace).
- `name = "value"` defines a string metadata entry (the metadata namespace);
  the two namespaces are separate.
- `name = depend "url" [rev = "…"] [branch = "…"] [tag = "…"]
  [package = "…"] [sub = "…"] [plugin]` declares a git dependency bound to
  `name`.  The package
  manager (`lichen`, in `crates/lichen-package`) fetches it into the lichen-home
  source cache and stages it on the import path, so a `name = import "alias"`
  resolves into the fetched source (or its `sub` subdirectory, for a monorepo
  source).  The language compiler itself does not fetch — it only parses and
  surfaces the directive.
- `name = plug "url" [rev = "…"] [branch = "…"] [tag = "…"] [package = "…"]
  [sub = "…"]` declares a **native plugin** bound to `name`: a Rust crate that
  extends the compiler's value/operator vocabulary, fetched and composed into a
  rebuilt compiler the same way.  A `plug` is always a plugin and never a plain
  import, so it takes the same options as a `depend` minus the `plugin` flag
  (which is what a `plug` implies).

A string is `"…"` with no escape characters and may span newlines; its content
is any character except `"`.  The delimiters are `---`, which is free because
**the language has no comments at all** — prose lives in this block — and
`---` is reserved: it cannot appear in the surrounding code, nor before the
block's own opening.  The block carries `order` / `output` / prose for the README
tooling (see `crates/lichen-tools/src/readme.rs`).  The code to compile is the
source after the block (or the whole source when there is none); the
preprocessor returns that borrowed slice plus a base byte offset so the lexer
maps every span back to the original file.

`@` is **not** the block delimiter: it is the prefix every keyword carries
(`@loop`, and every keyword after it), which is why the block moved to `---`.

## 3. Semantics

- **Programs are pure expressions.**  The checker compiles the IR, runs the
  definition pass (so apply-time type checks fire), and the program's value is
  the evaluation of its root: an `Int`, a `string`, a tuple, an array, a table,
  a function, or a type expression.
- **The prelude.**  Every source is seeded with the built-in **`core`** module
  ([core-prelude](notes/core-prelude.md)), whose names are in scope with no
  import: the class domain `Num`, the predicate `in_num`, and one binding per
  polymorphic operator — `add`, `sub`, `mul`, `div`, `less`, `greater`,
  `less_or_equal`, `greater_or_equal`.  Each takes **one argument — the operand
  group, a 2-wide array** — and its element type carries the **class** refinement,
  so the contract is the whole of it: the elements' shared class is the operand
  *tie* (an `array{T, n}`'s elements are one type), the length is the operator's
  *arity*, and the class is the refinement:

  ```lichen
  add = operands => { operands : array{(_ ! in_num), 2}; operands[0] + operands[1] }
  ```

  `add [1, 2]` is `3`, `add [1.5, 2.5]` is `4.0`, `add [1, 1.5]` is refused by the
  tie, `add [1, 2, 3]` by the length, `add ["a", "b"]` by the class.  The group is
  one argument rather than a curried pair because lichen has no multi-parameter
  lambda (`(a, b) => e` does not parse — `(a, b)` is always a tuple *value*), and
  the group's elements are read with the ordinary index (`operands[i]`; `X{n}` reads
  a *type value*'s pair, not a runtime array's element).  The prelude is
  **shadowable, not reserved**: it is seeded *before* a program's own imports and
  bindings, so a program that binds `add` gets its own.  The module is also
  reachable as a value (`core = import "core"`, then `core.add`), and it is a
  **file**: the toolchain materializes its source under its cache root and blames
  a contract violation on the line that wrote it (`…/builtin/core.lichen:3:39:
  assertion failed: expected 1, found 0`), rather than reporting a failure with
  no position.
- **Statements and bindings.**  A program is a **block body**: a list of
  (possibly `pub`-marked) statements — a `name = expr` binding or a bare
  expression — followed by an optional **tail** expression.  The top level is
  itself a block, so it has exactly the shape of a `{ … }` body: statements are
  separated by a `Separator` (newline, `;`, or `,`; any quantity, anywhere
  between them, and the end of the input is *not* a separator — it just ends
  the body), and the tail is the expression in the final position.  A program
  that ends in a binding (no tail) is a **record program** — a module — whose
  value is an anonymous struct built from the statements (see **Blocks**).  A
  binding is *graph sharing*, not sugar: its value compiles once into the IR
  arena, and every use of the name is that same node id (the IR is a graph).
  There is no `let` node and no desugared lambda — the tail stays the program's
  root, so its type is determinable exactly like a bare program's.  A binding is
  **block-wide** by default: its name is in scope throughout the block —
  forward *and* backward — so a value may reference a later binding, or any
  other binding of the block, and may recurse with them.  It may **not** name
  the binding it binds: the name **is** the value's own node in the graph, so
  such a value is its own input, and it is a **resolve diagnostic** at the name.
  A rebinding is written with `let` (the *restrictive* form, below).  The one
  self-reference the language admits is the **recursive call**: a binding named
  as the head of an apply inside a `=>` body — `f = n => f (n - 1)`,
  `fib = x => [fib (x - 1) + fib (x - 2), x][x <= 1]`,
  `@loop dot = s => … dot (s(0) - 1, …)` — because that edge is a call and not
  a value read.  The
  frontend reserves a placeholder id per block-wide binding, enters all the
  names before compiling any value, then fills each placeholder with its
  value's node — so a *mutual* reference makes the IR a cycle there (the graph
  contains a back-edge), which the checker totalizes by pre-registering a
  skeleton pair and binding the skeleton's cells to the finished pair (no
  recursion, no overflow **in the checker**).  A cycle among **scalar values** is
  refused before that: a block-wide binding's name is its value's own node, so a
  group of scalars that depends on itself is its own input, and the resolver
  leaves the edge that closes it unresolved so the block lowers inert.  What a
  kernel-body lowering walk did with such a cycle before that refusal — a stack
  overflow — is measured in
  [self-referential-binding-overflow](notes/self-referential-binding-overflow.md).
  `let name = expr` is the *restrictive* form: the
  value compiles before the name enters scope, so the name is visible only to
  later statements (`let a = a` resolves `a` to the outer binding — the
  sequential, non-recursive case).  `cache name = expr` marks the binding as a
  **retained cell**: its value is meant to survive a rebuild, identified by the
  binding's occurrence path from the program root.  The mark is independent of
  `let` and the two may appear together (`cache let a = …`); it is accepted in
  every scope and emits no diagnostic.  The retention mechanism is complete and
  wired into the incremental session (`BufferSession` lowers a clean cell to a
  static read of its frozen value and drops only the cells an edit reached), and
  the language server drives one session per open document; outside a session a
  marked binding is compiled like an unmarked one — see
  [incremental-update](notes/incremental-update.md) §6.  Sharing means a bound
  *non-function* value
  has one type across uses, while a bound lambda stays polymorphic — each
  application still instantiates the parameter fresh via the runtime's
  per-apply clones.  A **bare expression
  statement** (`5; 7`, `f 5` before more statements) is no dead code: the
  frontend wires every statement into the root (`Index(Tuple([stmt₁, …,
  stmtₙ, final]), n)`), so each is checked and evaluated — the runtime *is*
  the typechecker — and only the tail is the program's value.
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
  monomorphizes its contents.  `{}` (no statement and no tail) is a parse error,
  like an empty program.
  The block's value is its **tail expression**.  A trailing expression (a bare
  statement in the final position, whose next token is the `}` or the end of
  the input) is the tail; an explicit `return
  expr` anywhere in the block is also the tail, so a value can be pinned
  before or among the statements (`{ a = 1; return 2 }`).  A body whose last
  statement is a binding (and with no `return` anywhere) has **no tail**, and
  instead parses as a **struct-returning block**: its value is an anonymous
  struct instance whose fields are the **bindings** — a `name = value`
  binding is a field (its name is the binding's), and every field is therefore
  named, as in a `struct{…}` declaration (see *Nominal struct types*).  A bare
  expression is an ordinary statement: it is checked, its value is discarded,
  and it is never a field, so `{ 1; x = 2 }` returns a record with the single
  field `.x`.  A `let`
  binding is a block-local and never a field, and a `pub`-marked statement is
  a field (when any statement is `pub`, only the `pub` ones are).  At the top
  level this is a **record program**: a library file that ends in its bindings
  is a module whose value is that anonymous struct — the exported bindings —
  so `import "math.lichen"` gives back the module with no need to wrap the
  body in a `{ … }` (the file's top level *already is* a block).
- **`@gc e` — a scope of its own.**  `@gc` is a prefix marker over any
  expression, at the `@assert` level, so it takes the whole application rather
  than one atom of it (`@gc f 41`, and `@gc { … }` for the block form — a block
  is one expression).  It computes nothing and changes no type: the scope's
  value is `e`'s value.  What it changes is `e`'s **lifetime** — `e` is
  compiled into a block of its own, and once `e` has decided its value the
  block is released, keeping only what that value still reaches.  Measured: a
  block releases no nodes (every statement shares a class with the tail), and a
  call releases the nodes its answer does not reach — so `@gc` pays on a
  per-call basis.  An `e` that
  never decides is not released.  A scope inside a function body is one scope
  **per call**, so a `@loop` that runs many times does not accumulate one
  block per iteration; and `@gc f 41` at a call site releases the call's own
  clones.  See [gc-block](notes/gc-block.md).
- **Every lambda is automatically let-polymorphic.**  Each application
  instantiates the parameter fresh — the lowlevel apply machinery clones the
  parameter per call site — so `(x => x) 5 : Int` and `(x => x) Type : Type`
  both check, and one binder used at two different types
  (`(id => ((id 5 : Int), (id Type : Type))) (x => x)`) checks as well.  No
  generalize/instantiate special form exists or is needed.
- **Types are first-class values.**  `Int`, `string`, `Type`, function types
  (`T -> U`), tuple types, and array types (`array{T, n}`) are ordinary values that
  can be passed around, bound, and used in type position.  `Type : Type`
  holds: the type chain `value → type → kind → …` closes **in a cycle at
  `Type`** — the universe is the self-referential node `K = [Type, K]`.
  `Type` is the type of the *atomic* type markers (`Int : Type`,
  `string : Type`, `Type : Type`) and the terminal of every chain; it is
  **not** a supertype of all types.  A compound type is typed by its kind —
  a `[marker, Type]` pair — not by `Type` itself: a **written** arrow `T -> U`
  is a **function value** — its parameter annotated `T`, its return annotated
  `U` — and a function value's type is the function itself (`f : f`, a
  self-referential node on the universe's pattern, so its chain closes at
  `f`), so the arrow has no term of its own left to denote and the type of a
  function is one thing rather than two (see
  [function-type-merge](notes/function-type-merge.md)); a **recursive**
  function's type therefore follows from its annotation: `gcd = (p => …) :
  tuple{Int, Int} -> Int` decides the self-recursive result (`6: Int`), while
  without the annotation the function's own cells stay open (`?a -> ?b`) and
  every result derived from it is undecided (`?c`) — `examples/gcd.lichen`
  states the constraint, and the fold form annotates the same way, `@loop run =
  ((s => …) : tuple{Int, array{Float, 6}} -> array{Float, 6})`; and
  `array{Int, 3} : Type` *fails*
  (the array type's type is the kind `[ArrayType, Type]`).  There is no
  subtyping relation at all (`Int` is not `<: Type`); kinding is an ordinary
  type check, so a literal in type position is a kinding error, not a separate
  "kind system".
- **Unification is equi-recursive — by design.**  There is deliberately **no
  occurs check**: cyclic types unify, and the universe *requires* a cycle
  (`K = [Type, K]` above).  A recursive struct type is an ordinary cyclic
  type — `A = struct{.f Int, .g B}; B = struct{.f Type, .g A}` checks and runs (see
  `examples/struct_recursion.lichen`).  This is the approved semantics
  ([function-type-merge](notes/function-type-merge.md) records why), not a
  caveat: the flip side of `Type : Type`'s flexibility is that decidability
  of a lichen program is the embedding's responsibility.
- **The computational operators.**  `+ - * / %` are arithmetic on `Int`; the
  comparisons `< > <= >=` compare two `Int`s; `==` / `!=` are the
  **generalized** equality over any two *same-typed* values (two `Int`s, or two
  type values — `S::a == Int` is `1`); and `& | ^` are bitwise, which over two
  comparison results are the language's `and` / `xor` / `or`.  Every operator
  yields an `Int`: there is no `Bool`, so a comparison's `0`/`1` is what drives
  an `if`.  An `Int` is a machine-sized **unsigned** integer, so `+ - *` wrap,
  `/` and `%` are the unsigned division and remainder, the four order
  comparisons are unsigned (`0 - 1 > 1` is `1`), and every implementation —
  interpreter, CPU-JIT, GPU-JIT — reads them the same way.  A `Float` takes
   `+ - * /` and the four order comparisons, and **no `%`**.  Each yields a
   `Float` except a comparison, which yields an `Int` like every other.  `Int`
   and `Float` are **unrelated types**: no operator mixes them, so `1.5 + 1` and
   `1 == 1.5` do not check, and nothing converts silently.  The only crossings
   are the two prefix keywords `int2float e` / `float2int e`, each of which
   checks its operand against its own **source** class and yields its **target**
   — so `int2float 1` is `Float` and `float2int 1.5` is `Int`, and the wrong
   operand class is refused by name.  `int2float` is the nearest `f32` (so
   `int2float 16777217` is `16777216.0`); `float2int` truncates toward zero and
   is **partial** — a `NaN`, an infinity, or a value the unsigned `Int` cannot
   hold records `operator.out_of_range` and answers the lazy marker.  See
   [operators](notes/operators.md) §7.
   Five more prefix keywords are **scalar operators** rather than crossings:
   `max a b` and `min a b` yield the operands' own class (so `max 1 1.5` does not
   check, by the same tie `1 + 1.5` fails), and `exp x`, `sqrt x` and `floor x` are
   `Float`'s alone.  A two-operand keyword takes **atoms**, so `max a b` is the
   maximum of `a` and `b` and `max (a + 1) b` needs its parentheses; the one-operand
   ones take an application, like the crossings.  All three `Float`-only operators
   are **defined in the kernel IR** — a shared polynomial for `exp`, a range
   reduction and Newton's iteration for `sqrt`, an exact conversion for `floor` —
   rather than by a target's instruction, so every backend and the interpreter read
   one sequence; `sqrt`'s worst relative error is `1.19e-7` and `floor` is exact.
   See [notes/floating-point](notes/floating-point.md) §6, §7.  See
   [operators](notes/operators.md) §11.
   `==` / `!=` over two `Float`s are the generalized equality — which is
   the same relation the rest of the language uses for "these are one value" —
   so they compare a float by its bits: `0.0 == -0.0` is `0` and
   `NaN == NaN` is `1`.  See [notes/floating-point](notes/floating-point.md) for
   why there is one relation rather than two.  A **zero divisor**
  has no value: the interpreter records `operator.divide_by_zero` and answers
  the lazy marker, as it does for every other refused computation, while inside
  a jitted kernel it stays the author's responsibility (the CPU kernel's wasm
  traps; a GPU kernel's is undefined, and a guard would cost a branch on the
  device's hottest path).  See [operators](notes/operators.md).
- **Membership.**  `a @in S` tests whether `a` is a **member of the set** `S`,
  and yields `0`/`1` like a comparison.  It is a keyword at the `@` sigil rather
  than punctuation — it is a predicate over a set, and `@` is where the
  language's reserved words live — and it sits at the **comparison level**,
  left-associative, so `x @in S == 1` is `(x @in S) == 1`.  Its right operand is
  checked to be a set: a membership test against anything else is a check-time
  refusal, the same container pin `e[i]` applies.  **Nothing unifies against
  either operand**: a membership test is a fact about a *value*, so it is
  answered by evaluating the value, never by reconciling types.

  A member is matched **by the class it denotes when it denotes one, and by the
  language's own value equality otherwise**.  A set of *type values* — the class
  domain a contract is written over (`Num = set(Int, Float)`) — is therefore
  compared structurally, so a class out of another module matches (`ValueExt::value_eq`
  cannot answer this case: it compares array *handles*).  A set of ordinary
  values is compared by value: `2 @in set(1, 2)` is `1` and `3 @in set(1, 2)` is
  `0`.  See [operator-polymorphism](notes/operator-polymorphism.md) §3.
- **Indexing.**  `e[i]` reads the `i`-th element of an **array**, and `a(k)` —
  the adjacent single-expression paren — reads position `k` of a **tuple**
  (`(0, 1)(1)` is `1`).  A struct instance's fields are read **by name**
  (`s.x`), and a struct *type* value's components with `X::a`, so a struct type
  is refused by `a(k)` like any other
  non-tuple: the operator is chosen by syntax, never by a runtime kind dispatch,
  and each read form states the container kind it accepts.  `e[i]` and `a(k)`
  state it as a **pin** — the container's type is unified with a fresh array /
  tuple type — so a container that is not decided yet is refused by the
  *application* that supplies it, per call, rather than skipped: `x(0)` over an array is
  `expected a tuple type, found array{Int, 2}`, and `x[0]` over a tuple is
  `expected an array type, found tuple{Int, Int}`.  Both name the **kind**
  they require in prose; the older wording spelled the read's own pattern
  instead (`expected tuple{?a, …}, found array{Int, 2}`), which is what a fresh
  tuple-type pattern prints, and the requirement stopped being one when it
  moved to the kind ([eval-before-unify](notes/eval-before-unify.md) §2.2).  The
  named reads state the same requirement as a unify on the corresponding slot:
  `.a` needs the container's *kind* to be a struct kind, `X::a` needs its
  *type* to be one (§3, *the named type access*).  A
  literal index into a statically-known array is checked against its length
  at check time (an out-of-bounds index is an `IndexOutOfBounds`
  diagnostic); an index known only at runtime (a parameter, a call result)
  is checked when evaluated.  Indexing a *concretely* non-indexable type —
  a tuple, a struct, a function, a table or an atomic type — is a refusal,
  never a runtime panic (mirroring the apply guard), at check time when the
  container's type is concrete and at the application that binds it otherwise.
  `[then, else][i]` is the mechanism under the conditional form
  (`if c then e1 else e2` desugars to it) — an integer index selects a branch, and the untaken
  branch is never evaluated (the lowlevel `Index` stays lazy on it).
- **The type access `X{n}`.**  The glued `{` postfix reads component `n` of `X`'s
  **value**, and the container's *type* must be the **tuple kind** — a tuple *type
  value* is what the form is for, and its components are read structurally.  It
  is the way to read a component of a *type-as-value* directly: `tuple{Int, string}{0}`
  is the `Int` type (the tuple type's first component), and the container may be
  a bound name, a parameter or a call result.  The requirement is stated once, as
  a **unify**: a decided container is refused where it stands
  (`expected TypeTuple, found tuple{Int, Int}` for the tuple *value* `(1, 2)`, whose
  type is the tuple shape rather than the kind; `expected TypeTuple, found
  TypeStruct` for a struct type value, whose components are read by name —
  `X::a`; `expected TypeTuple, found array{Int, 2}` for an array), and an
  undecided one is pinned, refused by the apply that binds it.  Nothing else is
  a container.
  The read's result is **the component's own pair**: its value from the pair's
  value slot, and its type from the type slot — the component type value's *kind*
  (`Type` for `tuple{Int, string}{0}`, and a `TypeStruct` kind for a component that is
  itself a struct type).  The value slot is read lazily; the type slot is computed
  when the read is checked, so a later check on the read's own type compares it
  rather than binding it, and a read the kind requirement above refused is not
  built at all.  The read is *untyped* only in that it does not pin an array type,
  guard an index target
  or assert bounds: an out-of-bounds subscript is still an evaluation error,
  recorded during the definition pass, so the build is refused.  `e[i]` is how a
  runtime array's element is read.  This is the syntax the
  array type used to occupy — the array type is now the keyword-led
  `array{T, n}`.
- **The named type access `X::a`.**  The glued `::` postfix reads field `a` from a
  **TypeStruct value** — the container's *type* must itself be a TypeStruct
  kind (`[[TypeId, names, names_in_order], TypeStruct]` under the kind's
  `[marker, K]` pair, the name→index table centred right there
  at `container_ty[0][0][1]`) — the named sibling of the tuple-kind requirement
  `X{n}` states, and likewise a **unify**: a decided container is refused where
  it stands, an undecided one is pinned and refused by the apply that binds it
  (this is what removed the `TableGet` panic a deferred non-struct used to hit).
  It yields the field's *type* as a value, so
  `struct{.a Int, .b string}::a` is `Int : Type` — the type being that field type
  value's *kind*, `Type` for a scalar field and a `TypeStruct` kind for a
  struct-typed one, computed when the read is checked exactly as `X{n}`'s is, so
  a later check on the read's type compares it.  A container the requirement
  refused leaves the read unbuilt: the name table it needs is a walk into that
  container's own type, and the read is defined for a struct type value and
  nothing else.  Its sibling `.a` reads a
  field *value* from a struct instance (whose *kind* must be TypeStruct, table
  at `container_ty[1][0][0][1]`).  Because `::` now means this read, the table
  literal's key/value separator is spelled `==>`.  `==` is generalized to
  compare any two same-typed values (an `Int` or a type value): `S::a == Int`
  is `1`, `S::a == string` is `0`, while a cross-type comparison is a check-time
  `BinOp` error.
- **Nominal struct types.**  `struct{.x T1, ..., .z Tn}` is a *new type*; **every
  field carries a name**, given by the `.name` prefix.  The leading `.`
  unambiguously marks a named field — the language-server-friendly
  discriminator, since a field name and a field-type expression (both
  identifiers) can never be confused while the user is typing — and a field
  *without* one (`struct{Int, Type}`) is a `StructFieldName` check error: a
  struct instance reads by name, so an unnamed field would have no read at all
  (the positional form `a(k)` is the *tuple* read, see *Indexing*).  The field
  list may be **empty**: `struct{}` is a struct type with no fields, and its
  name→index table is *present and empty* — an empty table, not the
  name-table-less marker an anonymous positional struct gets — so a shape with no
  members has a spelling, and a zero-field type is as nominal as any other.  The names
  are stored on the struct type as a name→index table, in the names slot of the
  struct marker's **payload** (`marker = [payload, TypeStruct]`,
  `payload = [TypeId, names, names_in_order]`, the kind's marker
  slot), which lets a
  `a.name` read resolve a field by name.  Its kind is a standard `[marker, K]`
  pair whose marker is that `[payload, TypeStruct]` pair — the marker's *type*
  slot is the `TypeStruct` atom, which is what makes "is this a struct marker?"
  a tag check rather than a shape guess.  The payload also holds a **fresh
  nominal id** — each occurrence of the syntax allocates a new id, so two
  occurrences never unify and a struct never unifies with a same-shape tuple
  type (nominal identity).  Bind one occurrence and it is reusable: the
  checker compiles each expression once, so a bound or parameter-passed
  struct type used many times is the *same* type — `s = struct{.f Int}; [s, s]`
  is a homogeneous array, while `[struct{.f Int}, struct{.f Int}]` (two
  source occurrences) is a nominal conflict.
- **Struct instantiation.**  `s(1, 2)` — an application whose callee is a
  struct type — wraps the positional tuple in the nominal type: it compiles
  to the dedicated `Instantiate` expression, whose element types are checked
  against the field list (arity and field types must match), and whose type
  is the struct type itself.  Instances of different source occurrences are
  different types, even with the same fields.  The form is **syntactic**:
  any `C(f1, …, fn)` with the `(` glued to the callee and the tuple comma
  discipline (`C()`, `C(,)`, `C(e,)`, `C(e1, …, en)` — the bare
  single-expression `C(e)` stays the tuple read) lowers to
  `Instantiate`; there is no frontend callee-kind dispatch, the checker
  decides whether the callee is a struct type, and a callee that is not one
  fails at check time (the `InstantiateCallee` diagnostic — structs are
  nominal, so a tuple or function type cannot instantiate).  An undecided
  callee (a parameter, a deferred read) is *pinned* to a struct kind, so a
  non-struct actual callee is refused per call: the field list is read lazily as
  each element's **slot type**, and that deferred read is the one that reports —
  at the instantiation, with the check's own kind — rather than through the
  apply's argument unify; a
  call-result callee (`(mk (Int))(1, 2)`) is force-evaluated at check time,
  so the static checks see the concrete struct type.  An instance's fields are
  read **by name**: `a.x` resolves `x` through the struct's name→index table to
  the field's positional index, and the read's type is that field's type (a
  `a.x` on a struct without that field is a `NamedField` diagnostic at the read,
  with a did-you-mean clause naming the closest field; a `a.x` on a non-struct is
  refused
  by a struct-kind **Guard** at check time — `f = x => x.a` ; `f (10, 20)`
  reports `expected a struct type, found tuple{Int, Int}` — and the lazy
  name-table read still runs, so the two runtime read messages (`this value is not
  a container`, `table lookup missed`) follow it, each at the read).  The
  positional form
  `s(1, 2)(0)` is **not** this read — `a(k)` is the *tuple* read, and a struct
  instance is refused by it (see *Indexing*) — nor is `s(1, 2)[0]`, which is the
  array read.  Values of struct type beyond the wrapped
  tuple are future work.
- **A field's requirement and its pin.**  A field is a **slot**, so it states
  what a tuple element states: a `# n` on it is a *requirement* and a `v : T` on
  it is a **pin**, both checked against the argument that fills the position, the
  way the same spelling on a tuple element is
  (`docs/notes/compound-element-slots.md`).  `S = struct{.a Int # 4}; S(.a 1 # 2)`
  is refused `expected 4, found 2`, and `S = struct{.a 5 : Int}; S(.a 6)` is
  refused `expected 5, found 6` — the field-type unify compares types only, so
  this reconciliation is the whole of the check.  A **named** argument is checked
  against the field it supplies, not against the position it was written at: in
  `S = struct{.a 5 : Int, .b 7 : Int}`, `S(.b 8, .a 6)` is refused on `.a`.  A
  callee the checker never saw at the source — a parameter, a placeholder, a
  nested instantiation — spells no field requirement, so there is nothing to
  check there and the argument's own requirement travels with it through the
  apply.
- **Named instantiation arguments.**  An argument of an instantiation may be
  prefixed with the same `.name` discriminator a `struct{…}` definition
  uses: `S = struct{.x Int, .y Type}; S(.y Int, .x 1)`.  The names ride the
  `Instantiate` expression to the checker, which validates them against the
  struct type's name table and **reorders** the argument values into the
  definition's positional order — a later `.b` read sees the
  definition's order, not the call's.  Named and positional arguments mix:
  a positional argument fills the lowest-numbered unclaimed position.  The
  structural mismatches are their own diagnostics, each pointing at the
  offending argument: an unknown field (`StructUnknownField`), a duplicate
  (`StructDuplicateField`), a field left unsupplied (`StructMissingField` —
  not reported when another argument's name is unknown, because that name
  explains the gap; one typo yields one diagnostic),
  an excess positional argument (`StructExcessField`), and a `.name`
  argument against a struct type with no names (`StructAnonymousField` — no
  longer reachable from source, since every definition is named, and kept for
  hand-built IR).  After
  reordering, each argument's type is checked against its field's type as
  usual.
  When the struct type is **not statically known** — an undecided callee (a
  parameter) or a placeholder — nothing is refused: the instantiation is
  unresolved too, and the reorder is a **lazy read** that resolves at the
  unification binding the callee's type.  `f = s => s(.y Int, .x 1); f (S)`
  and `f (_(.x 1, .y 2))` reorder at the apply (with `f = x: S => x` and
  `S = struct{.x Int, .y Int}`), the same order the positional form already
  defers in.  The supplying argument is found by a lookup keyed by the
  field's own name — or, for a positional argument, by its rank among the
  positional ones — and, when every argument's type is decided, by that type
  too, so a field whose declared type no supplying argument matches is a
  lookup miss (`TableMiss`) at the moment the type resolves, and the arity is
  the field-list unify's, as for a positional instantiation.  A **duplicate**
  name is refused at check time: one name supplying two positions is a
  structural mismatch whatever the field list is.
- **Dependent array types (pinning).**  The length of `array{T, n}` is an arbitrary
  expression, so `array{Int, n}` where `n` is bound is a legal dependent type.  When
  an annotation compares a value against such a type, the length read — an
  unevaluated `Index` over `n`'s value cell — resolves as a pure reference
  and is pinned to the value it must equal: the checker binds `n` to the
  literal's length, so the parameter is monomorphized.
  `((n => ([1, 2, 3] : array{Int, n})) 3)` checks and runs, and applying any other
  length fails at the apply — the pinned value is enforced per application
  (the apply's argument unify compares the cloned parameter, which carries
  the pinned length, against the argument).
- **An apply applies the callee's type.**  A function type *is* the function it
  names (`f : f`), so an apply whose callee value is not concrete yet still
  applies the signature: the argument's type is compared against a **fresh**
  copy of the domain, and the result cell binds to a **fresh** copy of the
  codomain.  The clone per call is what keeps polymorphism — with
  `type_of = x => {t = _; x: t; t}`, `(type_of (1), type_of Int)` is
  `(Int, Type)`, where comparing against the template's own cells answers
  `expected Int, found Type` at the second call.  A lambda nobody applies is
  still read: `x => { x: Int -> Int; x 1.0 }` is `expected Int, found Float`
  with nothing run, and `x => { x: Int -> Int; x 1 }` states its result.
- **The placeholder is inert.**  An annotation's unconstrained positions hold
  the **wildcard**, which states nothing and binds nothing: it is one shared
  node, and unification neither merges nor writes it, so no class can bind it
  ([compound-element-slots](notes/compound-element-slots.md) §`Any`).  An
  apply's **clone** mints a fresh undecided cell in its place — carrying that
  one node would put it in a class the caller also holds, and would make every
  unify it met return before comparing anything.
- **The `_` placeholder.**  A `_` in any position compiles to an undecided
  cell: the annotation unifies the value's type against it, so the cell
  binds to that type — `5 : _` infers `Int`, `x => x : _` the arrow
  `?a → ?a`, and `[1, 2, 3] : array{Int, _}` the length `3`.  Partial types infer
  the rest: `((x => x) : (Int -> _)) 5` fixes the input to `Int` and infers
  the output.  In value position the same hole is a *typed* hole: `_ : Int`
  checks as an underdetermined `Int` value (its value cell stays undecided,
  an empty slot), and `f _` / `(1, _)` unify the hole's type with
  the context.  Kinding is deferred for `_` like any undecided type, so `_`
  never raises a kinding error; a `_` that never binds leaves the type
  underdetermined — not an error — and a mismatch against a
  partial type is still an error (`5 : Int -> _` fails).
- **Reading a value's type.**  There is no `type_of` form — a type read is an
  ordinary function built from the placeholder and an annotation:
  `type_of = x => {t = _; x: t; t}`.  The placeholder binds a fresh cell, the
  annotation `x : t` unifies that cell with the argument's type, and the body
  returns it, so the call's value *is* the operand's type expression: the value
  is its shape and the type its kind.  Everything a builtin read would give
  follows from that unification, with nothing forced (a read of an undecided
  parameter resolves at the apply): `type_of (1)` is `Int : Type`,
  `type_of [1, 2]` is `array{Int, 2}`, `type_of Type` is `Type`, and in a type
  position it is exactly the operand's type, so `5 : type_of (1)` checks.  The
  standard library ships it (`std.type_of`, `lichen-std/_.lichen`) and
  `examples/type_of.lichen` uses it.

## 4. Compilation: source → IR

Each AST node compiles to exactly one `ExprKind`.  The IR itself is span-free;
the frontend tracks positions in its own `SpanIndex` (an `ExprId → span` map,
spans `(line, column)`, 1-based) filled as each IR node is created:

| source | `ExprKind` |
|---|---|
| `5` | `Literal(IntLit(5))` |
| `1.5` | `Literal(FloatLit(1.5))` |
| `"s"` | `Literal(StrLit("s"))` |
| `Int` | `Literal(IntTypeLit)` |
| `Float` | `Literal(FloatTypeLit)` |
| `string` | `Literal(StringTypeLit)` |
| `Type` | `Literal(TypeTypeLit)` |
| name use | the binder's own `ExprId` (pre-resolved) |
| `x => e` | `Function { parameter, parameter_type: None, parameter_attribute: None, return }` — `parameter` is the `Parameter` expr for `x` |
| `x : T => e` | `Function { parameter, parameter_type: Some(compile(T)), parameter_attribute: None, return }` — the annotated parameter's type, compiled in body scope (the §4.2 desugar kept as an optimization) |
| `x # n => e` | `Function { parameter, parameter_type: None, parameter_attribute: Some(compile(n)), return }` — the annotated parameter's perspective, also body-scope |
| `e1 e2` | `Apply { function, argument }` |
| `a op b` (`+`, `-`, `*`, `/`, `%`, `<`, `>`, `<=`, `>=`, `==`, `!=`, `&`, `\|`, `^`) | `BinOp { operator, left, right }` |
| `@assert e` | `Assert { condition }` — a side constraint: the expression's pair is the condition's own; the condition's value node registers as an assert point the checker deep-evaluates to `USize(1)` |
| `int2float e` / `float2int e` / `max a b` / `min a b` / `exp x` / `sqrt x` / `floor x` | `Prefix { operator, operands }` — `int2float`/`float2int` are the only forms whose type is not their operand's: the operand checks against the direction's source class, the result's type is its target.  `max`/`min` tie their operands to one class and leave it; `exp`, `sqrt` and `floor` check their operand as `Float` and are `Float` |
| `if c then t else e` | `Index { array: [e, t], index: c }` — desugared to the lazy branch index; there is no `If` kind |
| `e[i]` | `Index { array, index }` |
| `a(k)` | `Field { container, key }` — the adjacent single-expression paren form; a positional slot read over a **tuple** element (a struct reads `a.name`) |
| `e : T` | `Annotation { value, type: Some(compile(T)), attributes: <an empty range> }` — `attributes` holds one value expression per schema-tail entry, and a bare `:` annotation has no tail, so the range is empty |
| `# p` / `e : T # p` | `Annotation { value, type: Some(compile(T))?, attributes: <a range over compile(p)> }` — the attribute expression lands in the children range **positionally aligned** with the schema tail it annotates (an `e : T # p ? d` pairs `attributes[0]` with `[Perspective]` and `attributes[1]` with `[Doc]`), and the tail is stamped onto the annotated node's schema |
| `! p` / `e : T ! p` | the same `Annotation` chain's **refinement** piece: `p` is a predicate on the annotated *value*, held in one attribute slot and required to evaluate to `1`.  Like `#`/`?` the right side is one operand at the `->` level, so `e : T ! (x => x > 3)` writes the predicate explicitly; see [operator-polymorphism](notes/operator-polymorphism.md) §3 |
| `_` (any position — type or value) | `Placeholder` |
| `T1 -> T2` | `TypeFunction { parameter, return_type }` (domain, codomain) |
| `(e1, …, en)` | `Tuple(range)` |
| `tuple{T1, …, Tn}` | `TypeTuple(range)` |
| `struct{.a T1, .b T2}` | `TypeStruct { fields, names }` — nominal, fresh id per occurrence; the kind is a `[marker, K]` pair whose marker is the ordinary `[payload, TypeStruct]` pair (`payload = [TypeId, names, names_in_order]`): the marker's type slot is the `TypeStruct` atom, the tag that makes it a struct.  A field without a `.name` (`struct{T1, …}`) is a `StructFieldName` check error |
| `a.name` | `NamedField { container, name }` — the checker resolves `name` through the struct's name→index table to the positional index, then reads the field's type out of the field list |
| `X::a` | `NamedTypeAccess { container, name }` — a named type access over a **TypeStruct value**: the container type (a TypeStruct kind) supplies the name table at `container_ty[0][0][1]`; yields the field's *type* as a value.  The kind is stated as a unify, so a non-struct container is refused at check time, or at the apply that binds it when the container is not decided yet |
| `s(1, 2)` / `s(.x 1, .y 2)` (callee a struct type) | `Instantiate { type_expr, value, names }` — `names` is index-aligned with `value`'s tuple elements (a `.x 1` argument is `Some("x")`, a positional `1` is `None`); the checker reorders named arguments to the definition's positional order |
| `[e1, …, en]` | `Array(range)` |
| `[e1, @shallow e2, @shallow(2) e3]` | `ShallowArray { range, depths }` — any `@shallow`-marked element makes the array shallow: per-element marker depths (0 = unmarked, `usize::MAX` = the bare `@shallow`, n = the value slot shallow at the first n levels of the element's type spine) |
| `e ~ "text"` | **no kind at all** — a mark is an entry in `IR::marks` keyed by `e`'s `ExprId`, and the expression lowers to what it would lower to without it.  An annotation node is allocated only when the chain also spells a type, a perspective, a refinement or a doc |
| `array{T, n}` | `TypeArray { element_type, length }` |
| `set{E}` | `TypeSet(range)` — the one child is the element **slot**; the shape is that slot alone, so a `set{…}` type and its `set(…)` members pair one slot to every member |
| `table{K, V}` | `TypeTable(range)` — the two children are the key and value **slots**, exactly what `Table(range)` writes, so a table type and a table value are one type |
| `table(k1 ==> v1, …)` | `Table(range)` — the entries interleaved `[k1, v1, k2, v2, …]`; keys share one key cell, values one value cell, and a key that is not concrete is dropped with an error |
| `t <== k` | `Find { container, key }` — the infix table read; the entry whose stored key is deep-content-equal to `k` |
| `X{n}` | `TypeAccess { container, index }` — a type access of a **tuple type value**'s component: the container's type is unified against the tuple kind, so a non-tuple container is refused at check time, or at the apply that binds it when the container is not decided yet |
| `$name(args)` | `NativeCall { op, args }` — a native operator registered by the compiling module's plugin; `op` is a private name resolved only against that module's registry, and the checker adopts the `[value, type]` pair the plugin's builder returns |
| a use of an `---…---`-imported package name, or of one of its direct exports | `Static { export }` — the value is read out of the shared registry by its export ref; the checker materializes the pair and leaves the payload in the package's static arena |
| `{ a = e; …; e }` | the final expression's own node — statements are scope-entered (bindings), then popped; a non-final statement list is wired into the root as `Index(Tuple([…, e]), n)` |
| `{ x = 1; …; y = 2 }` (no tail) | `Record { value, names }` — a struct-returning block (the frontend's `RecordBlock` node): `value` is a tuple of the emitted field values and `names` a range of each field's optional name, index-aligned with the tuple; the checker builds the anonymous struct type from the element types |
| `{ …; return e }` | the `return` expression's own node (the frontend's `Block` node carries it as the block's `expr`) — the block's value is its tail, and the `return` may sit anywhere among the statements |

There is no desugar step: bindings are graph sharing, and a block-wide
binding reserves a placeholder id, compiles its value, then fills the id with
the value's node — so a mutual reference makes the IR a cycle there, which the
checker totalizes with a skeleton pair; a cycle among **scalar values** is refused
by the resolver instead, since a group of scalars that depends on itself is its
own input, and the edge that closes it is left unresolved so the use lowers to an
inert `ErrorBlock`, as an unresolved name's does.

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
binding defined *later* or *another* binding of the same block, and may name
itself only as the head of an apply inside a `=>` body, which is the recursive
call.  A use of `a` in a value resolves to the
reserved id; once the value compiles, the id is filled with the value's node
(a bare `a = b` aliases `b`'s node instead).  A use of `a` in its own value is
a **resolve diagnostic** at that use, and the use is left unresolved so it
lowers to an inert `ErrorBlock`.  A restrictive `let a = e`
compiles the value first and pushes `a` afterwards, so the name is visible
only to later statements.  A block `{ a = e; …; e }` does the same and pops
its scope frames (truncates) at the `}` — inside, the block's names shadow
outer ones; after the `}`, the outer names are back.  Shadowing is allowed
(the inner binding wins).  A name in no scope is a **resolve diagnostic** at
the name's span, and the use lowers to the same inert `ErrorBlock` a recovered
parse error uses — an opaque leaf the checker skips — so the partial program
still checks.

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
an unresolved name lowers to an inert `ErrorBlock` (masked, checker-skipped)
— as does a use of a binding's own name inside its own value, which the
resolver refuses — and the checker still runs on the resulting partial program
— so one pass
reports every problem it can find.  Checker diagnostics
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
`tuple{T1, T2}`, `array{T, len}`, `struct{...}`, and undecided cells as stable `?a`,
`?b`, … names (cells in one unification class share a name).  The boxed
highlevel `Diag` in `check` stays raw — it carries the structured facts
(`kind`, the classes `a`/`b` and their values, the `error_index` into
`unify_errors`) and its own raw message (`TypeInt`, `TypeType`, …).

Unification is equi-recursive (§3), so a type can be **infinite**, and a
rendered type is then a finite notation for it rather than a truncation of it:
the walk cuts the spine at the first node it meets again and puts `…` in that
slot, so the reader sees that the type does not end.  A lazy value's spine —
`tuple{Float, tuple{Float, tuple{Float, …}}}` for
`f = x => [x, @shallow f (x + 1.0)]` — and a recursive struct type
(`examples/struct_recursion.lichen`) cut the same way.  The cut is per walk, so
it fires at the first revisit and a cycle of any length terminates; what bounds
the printer is a nesting depth and not the cycle
([code-audit](notes/code-audit.md) `P4-10`).  Two readings share the `…` and
`?` vocabulary and each means something different: a repeated `?a` says two
slots are one unification class, a cut `…` says the spine is back at a node it
has already shown.  The field list of a **decided** tuple type is always
finite, so a `…` inside `tuple{…}` is that cut and never an arity marker:
`tuple{?a, …}` reads as "a cell beside a node the walk has already shown", not
as a shape the check has not fixed.  A cycle inside a raw dump is cut the same
way, and that is what keeps the universe's self-loop from unrolling
([raw-rendering-mark](notes/raw-rendering-mark.md) §1).

The printer has a second arm for a tuple type whose field list is **undecided**
— a shape slot holding no array at all — and it prints that same
`tuple{?a, …}`, so the spelling alone cannot tell the two apart.  **No program
in the language can reach it.** `f4c2b93a` ("checker: a positional read
requires the tuple *kind*, never binds the type") removed the only front-end
site that built one; every site passes an `array_node` now, so the arm is
reachable from IR alone.  A reader who finds it is looking at an orphaned arm,
not hunting for the program that triggers it
([index-read-unify](notes/index-read-unify.md) §7.6).

