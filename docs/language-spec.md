# Language spec 

Illustration only, not precise.

## Syntax 

core:
```sytax
program := block_inner
expr := name | atom | unify | lambda | apply | clone | block | annotated | compound | access | instanciate
block := `{` block_inner `}`
block_inner := ( (stmt | expr) sep)* expr
sep := `,` | `;` | `\n`
stmt := bind | assert
bind := [`let`] name `:=` expr
assert := `@assert` expr
unify := expr `=` expr
lambda := name => expr
apply := expr expr
clone := @clone expr
annotated := expr annotation+
annotation := annotator expr
annotator := `:` | `!` | `?` | `#`
compound := tuple | array | struct
tuple := `(` (expr sep)* `)`
array := `[` expr'template sep expr'length `]`
struct := `{` (`.` name expr sep)* `}`
access := tuple_access | struct_access | array_access
instanciate := array_instanciate | struct_instanciate
array_instanciate := expr'array glue `[` (expr sep)* `]` 
struct_instanciate := expr'struct glue `{` (`.` name expr sep)* `}`
tuple_access := tuple glue `(` expr `)` 
struct_access := struct `.` name
array_access := array glue `[` expr `]`
atom := number | string | `_` | @any | builtin
```

> In ```sytax <sytax>'<tag>```, `'<tag>` has no sytax meaning, just a tag to that sytax.

sugar:
```sytax
`<expr'1> <: <expr'2>` => `<expr'1> = @clone <expr'2>`
condition := `if <expr'1> then <expr'2> else <expr'3>` => `(<expr'3>, <expr'2>)(<expr'1>)
bin_operation := `<expr'1> <bin_operator> <expr'2>` => `<bin_operator> [<expr'1>, <expr'2>]`
`<name> <annoation*> => <expr>` => `<name> => {<name> <annoation*>; <expr>}`
`<name> <: <expr'1> => <expr'2>` => `<name> => {<name> <: <expr'1>; <expr'2>}`
`{...; return <expr>; ...; <stmt>}` => `{...; <expr>}`
`{...; a := expr'1; ...; b := expr'2; ...; <stmt>}` => `{...; {.a expr'1, .b expr'2}}`
`{...; pub a := expr'1; ...; b := expr'2; ...; <stmt>}` => `{...; {.a expr'1}}` 
```

> Inside ```sytax `...` ``` is lichen code ```lichen ...```, inside ```sytax `<...>` ```  is sytax ```sytax ...```.
> ```sytax ... => ...``` is sytax conversion.

## Lowlevel

Lowlevel is a graph of nodes.
A node can have an value, an operation, and a class.
An operation can have an operand node.
Nodes inside same class must share same value, class is merged by unify.
An value can be atomic or an array of nodes.

## Attribute

An expression is a raw array of attributes, value attribute is a lowlevel value, other attribute is an expression recursively, attributes is extentable.

```sytax
expr := `raw[raw value, type, refinement, doc, ...]`
```

> ```lichen @raw[...]``` is a lowlevel array that each element is a lowlevel value.
> ```lichen @raw <lowlevel_value>``` is a lowlevel value.

### Annotaion

An annotated expr creats a new expression with each annotated attribute:
- unified with the corresponding annotation expression.
- replaced by the corresponding annotation expression.

```sytax
`@raw[value, ann1, ...] : ann2` => `{ann1 = ann2; @raw[value, ann2, ...]}`
``` 

## Type

Annotation: ```lichen <expr> : <type>```.

expression with atomic lowlevel value has type `Atomic`

`Atomic` is self referencial expression:

```sytax
`Atomic` := `@raw[@raw AtomicType, Atomic，...]`
```

### Compound

Compound type is an layout documentation of lowleval array value, e.g. tuple, array, struct. 

Tuple has no special layout.

```sytax
`Tuple` := `@raw[@raw TupleType, Atomic, ...]`

`(e1, e2)` := `@raw[@raw[e1, e2], TupleType, ...]`

`t(n)` := `{(type_of t) = Tuple; @raw t.value[n]}` 
```

Array unify every element expression to a template expression.

```sytax
`Array` := `@raw[@raw ArrayType, Atomic]`

`[template, n]` := `@raw[@raw[template, <`_`*n>], Array, ...]`

`e0[e1, e2]` := `{
  let [template, length] := @clone e0
  (e1, 2) = (template, length)
  let [template, length] := @clone e0
  (e2, 2) = (template, length)
  raw[raw[template, e1, e2], Array, ...]
}`

`a[n]` := `{(type_of t) = Array; @raw t.value[n+1]}` 
```

## Subtype

Unify is directed, extend to subtype.

```sytax
`a = b` => `a <: b`
```

### Any
Subtype extension in lowlevel, `Any` is a lowlevel value which is super type of all others.

Literal `@any` is an any expression that its attribute array node has value `Any`.

```sytax
`@any` := `@raw Any`
```  

### Attribute

Attributes can resgister a custom function `is_subtype` to override its subtype behavior.

## Undecide

Undeicde is a state of lowlevel node that has no value.

Placeholder `_` is an undecide expression that its attribute array node is undecide.

```sytax
`_` := `@raw None`
```

Nodes inside same class can be classified by tag.

```sytax
`{x := _; (x, x, _)}` := `@raw['a, 'a, 'b]`
```

## Function

Function is an expression with a specified undecide expression as parameter.

Polymorphism at every application: @clone its expression, unify the argument with cloned parameter.

```sytax
`{f := p => body; f a}` => `{let (p_clone => body_clone) = @clone (p => body); a = p_clone; body_clone}`
```

## Struct

Struct is a special tuple that: 
- Each index has a name.
- Nominal over the sytax position.

```sytax
`Struct` = `@raw[@raw StructType, Struct, ...]`

`s := {.n1 e1 .n2 e2}` 

`s` := `@raw[@raw[name_table, nominal_id, e1, e2], Struct, ...]`

`s{.n2 e3, .n1 e4}` 
:= 
`{
  let {.n1 e1, .n2 e2} = @clone s
  v := raw[name_table, nominal_id, @raw None, @raw None]
  raw v[name_table["n1"]+2] = e1;
  raw v[name_table["n2"]+2] = e2
  e3 = raw v[name_table["n2"]+2]
  e4 = raw v[name_table["n1"]+2]
  raw[raw v, Struct, ...]
}`

`s.<name>` := `{type_of s = Struct; @raw s.value[s.value.name_table[<name>]+2]}`
```

> ```lichen raw t[name_table["n1"]+2]` is lowlevel computation.

## Preprocessor block

A lichen source file has at most one single preprocessor `---...---` block.

Preprocessor block can define metadata:

`name = "value" `

or import module:
- `name = import "path"`: import a lichen source module.
- `name = link "path"`: import a dynamic linked module.
- `name = plug "path"`: import a compiler plugin module.

or depend on package:

`name = depend "url" [rev = "…"] [branch = "…"] [tag = "…"] [package = "…"] [sub = "…"]`

