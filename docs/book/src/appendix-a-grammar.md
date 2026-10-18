# Appendix A: Keywords and Syntax Forms

This appendix is a reference, not a lesson: the forms the language has,
in one place. The precise grammar is in the
[language spec](../../language-spec.md) §2, and every form below is taught
in the chapter named beside it.

## Reserved words

The keywords cannot be bound or used as names:

```text
Int Float string Type tuple struct array table set
let if then else return pub cache
int2float float2int max min exp sqrt floor sin cos ln
```

and the `@`-led words:

```text
@loop @assert @shallow @gc @in
```

The underscore `_` is not a name; it is the placeholder (Chapter 5). A
name is a lowercase or mixed-case identifier.

## Punctuation

```text
= => -> : :: # ? ! ~ $ ==> <== @
+ - * / % < > <= >= == != & | ^
( ) [ ] { } , ; and newline
```

The three separators — newline, `;`, `,` — are the same token; their
quantity never matters. `---` opens and closes the header block and is
reserved elsewhere.

## The program

```text
program  := (statement separator)* [expr]
statement := ['pub'] [binding | expr]
binding  := ['@loop'] ['cache'] ['let'] name '=' expr
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
annotated := arrow (((':' | '#' | '!' | '?') arrow) | ('~' string))*
arrow     := find ('->' find)*                        right
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
primary   := int | string | 'Int' | 'Float' | 'string' | 'Type' | '_'
           | name | '$' name '(' args ')' | '(' expr ')' | '(' tuple ')'
           | '[' array ']' | 'table' '(' pairs ')' | 'set' '(' args ')'
           | '{' block '}' | 'tuple' '{' slots '}' | 'struct' '{' fields '}'
           | 'array' '{' expr ',' expr '}' | 'set' '{' slot '}'
           | 'table' '{' expr ',' expr '}' | 'if' expr 'then' expr 'else' expr
postfix   := glued '[' expr ']'        -- array read
           | glued '(' fields ')'      -- tuple read, or struct instantiation
           | glued '{' expr '}'        -- type access X{n}
           | glued '::' name           -- named type access X::a
           | '.' name                  -- struct field read
element   := ['@shallow' ['(' n ')']] glued-expr
pair      := expr '==>' expr
```

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
---
```

Metadata entries are strings; directives are imports and dependencies
(Chapter 15). The block is removed before lexing, so prose before it is
ignored and prose inside it is the only prose a file can carry.

## The chapter map

| form | chapter |
|---|---|
| bindings, separators, blocks, `if` | 3 |
| lambdas, application, recursion | 4 |
| annotations, `_`, `raw` | 5 |
| laziness, `@shallow`, `@gc`, `none` | 6 |
| tuples and arrays | 7 |
| structs and records | 8 |
| tables and sets, `@in` | 9 |
| `X{n}`, `X::a`, pins, dependent lengths | 10 |
| refinements, classes | 11 |
| recursive and dependent types | 12 |
| `#`, `?`, `~` | 13 |
| diagnostics and `@assert` | 14 |
| imports and packages | 15 |
| the example suite | 16 |
| kernels and the compute layer | 17 |
| the toolchain | 18 |
| plugins and `$name` | 19 |
