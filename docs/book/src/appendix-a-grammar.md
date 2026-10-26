# Appendix A: Keywords and Syntax Forms

This appendix is a reference, not a lesson: the forms the language has,
in one place. The precise grammar is in the
[language spec](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/language-spec.md) §2, and every form below is taught
in the chapter named beside it.

## Reserved words

The keywords cannot be bound or used as names:

```text
Int Float string Type Tuple Array Struct table set
let if then else return pub
int2float float2int max min exp sqrt floor sin cos ln
```

and the `@`-led words:

```text
@loop @assert @shallow @gc @in @cache @any
```

The underscore `_` is not a name; it is the placeholder (Chapter 5). A
name is a lowercase or mixed-case identifier.

## Punctuation

```text
= := => : # ? ! ~ $ ==> <== @
+ - * / % < > <= >= == != & | ^
( ) [ ] { } , ; and newline
```

The three separators — newline, `;`, `,` — are the same token; their
quantity never matters. `---` opens and closes the header block and is
reserved elsewhere.

There is **no** `->`: a function's signature is written with `=>`, and
a constraint on one is a function template unified with plain `=`
(Chapter 4).

## The program

```text
program  := (statement separator)* [expr]
statement := ['pub'] [binding | expr]
binding  := ['@loop'] ['@cache'] ['let'] name ':=' expr
```

A program is a block: statements, then an optional tail expression. A
program that ends in a binding is a record program (Chapter 15). The top
level is itself a block.

## The expression ladder

From loosest to tightest; each level is left-associative unless marked.
This is an abridged copy of the spec's grammar — arguments, fields and
blocks are elided:

```text
expr      := lambda
lambda    := annotated ('=>' expr)?                  right
annotated := unify (((':' | '#' | '!' | '?') unify) | ('~' string))*
unify     := find (('=' | '<:') find)*                 left
find      := cmp ('<==' cmp)*                         table read
cmp       := bits (('<'|'>'|'<='|'>='|'=='|'!='|'@in') bits)*
bits      := bitxor ('|' bitxor)*
bitxor    := bitand ('^' bitand)*
bitand    := sum ('&' sum)*
sum       := product (('+'|'-') product)*
product   := prefix (('*'|'/'|'%') prefix)*
prefix    := '@assert' apply
           | '@gc' apply
           | ('int2float'|'float2int'|'exp'|'sqrt'|'floor'|'sin'|'cos'|'ln') apply
           | ('max'|'min') atom atom
           | apply
apply     := atom atom*
atom      := primary postfix*
primary   := int | float | string | 'Int' | 'Float' | 'string' | 'Type'
           | 'Tuple' | 'Array' | 'Struct'
           | '_' | '@any' | name
           | '$' name '(' args ')' | '(' expr ')' | '(' tuple ')'
           | '[' array-template ']' | 'table' '(' exprs ')' | 'set' '(' args ')'
           | '{' block '}' | '{' struct-template '}' | 'if' expr 'then' expr 'else' expr
postfix   := glued '[' expr ']'        -- array read, or array instance
           | glued '(' expr ')'        -- tuple read
           | glued '{' fields '}'      -- struct instance
           | '.' name                  -- struct field read
element   := ['@shallow' ['(' n ')']] glued-expr
expr      := expr '==>' expr
```

The compound forms, in one place:

```text
tuple value        := '(' (expr sep)+ ')'
tuple requirement  := '(' requirement (sep requirement)+ ')'
array requirement  := '[' requirement sep length ']'
array instance     := constructor-glue '[' (expr sep)* ']'
struct requirement := '{' ('.' name requirement sep)* '}'
struct instance    := constructor-glue '{' ('.' name expr sep)* '}'
requirement        := '@any' [':' type] | value [':' type]
```

The **layout types** `Tuple`, `Array` and `Struct` are values, not
spellings of a type; the shape of a compound lives in its value, and a
**constructor** is the function that returns a fresh requirement when it is
applied (Chapter 7). The match operator `<:` applies its right hand side
exactly once — `actual <: S` is `actual = S _` — and plain `=` applies
nothing.

Where this differs from the spec, the spec is right.

## The header block

A file may open with one header:

```text
---
  doc = "prose"
  output = "value: type"
  order = "12"
  name = import "path"
  name = depend "url" [rev = "…"] [branch = "…"] [tag = "…"] [package = "…"] [sub = "…"] [plugin]
  name = plug "url" [rev = "…"] [branch = "…"] [tag = "…"] [package = "…"] [sub = "…"]
  name = link "path"
---
```

Metadata entries are strings; directives are imports, dependencies, and link
(Chapter 15). The block is removed before lexing, so prose before it is
ignored and prose inside it is the only prose a file can carry.

## The chapter map

| form | chapter |
|---|---|
| bindings, separators, blocks, `if` | 3 |
| lambdas, application, recursion, function templates | 4 |
| annotations, `_`, `raw`, the layout types | 5 |
| laziness, `@shallow`, `@gc`, `none` | 6 |
| tuples, arrays, requirements, `<:` | 7 |
| structs, instances, records | 8 |
| tables and sets, `@in` | 9 |
| constructors, requirements, pinned lengths | 10 |
| refinements, classes | 11 |
| recursive and dependent shapes | 12 |
| `#`, `?`, `~` | 13 |
| diagnostics and `@assert` | 14 |
| imports and packages | 15 |
| the example suite | 16 |
| kernels and the compute layer | 17 |
| the toolchain | 18 |
| plugins and `$name` | 19 |
