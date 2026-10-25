# Language spec 

## Syntax 

Illustration only, not precise.

core:
```sytax
program := block_inner
expr := name | atom | block | lambda | annotated | apply | compound | access | type | instanciate
block := `{` block_inner `}`
block_inner := ( (stmt | expr) sep)* expr
sep := `,` | `;` | `\n`
stmt := bind | assert
bind := name `=` expr
assert := `@assert` expr
lambda := name => expr
annotated := expr annotation+
annotation := annotator expr
annotator := `:` | `!` | `?` | `#`
apply := expr expr
compound := tuple | struct | array
tuple := `(` (expr sep)* `)`
struct := expr `{` (`.` name expr sep)* `}`
array := `[` (expr sep)* `]`
access := tuple_access | struct_access | array_access
tuple_access := tuple glue `(` expr `)` 
struct_access := struct `.` name
array_access := array glue `[` expr `]`
type := tuple_type | struct_type | array_type
tuple_type := `tuple` `{` (expr sep)* `}` 
struct_type := `struct` `{` (`.` name expr sep)* `}` 
array_type := `tuple` `{` expr'type sep expr'length `}` 
instanciate := expr `(` (`.` name expr sep)* `)`
atom := number | string | `_` | @any | builtin
```

> In ```sytax <sytax>'<tag>```, `'<tag>` has no sytax meaning, just a tag to that sytax.

sugar:
```sytax
condition := `if <expr'1> then <expr'2> else <expr'3>` => `[<expr'3>, <expr'2> ][<expr'1>]
bin_operation := `<expr'1> <bin_operator> <expr'2>` => `<bin_operator> [<expr'1>, <expr'2>]`
`<name> <annoation*> => <expr>` => `<name> => {<name> <annoation*>; <expr>}`
`{...; return <expr>; ...; <stmt>}` => `{...; <expr>}`
`{...; a = expr'1; ...; b = expr'2; ...; <stmt>}` => `{...; struct{.a _, .b _}(a, b)}`
`{...; pub a = expr'1; ...; b = expr'2; ...; <stmt>}` => `{...; struct{.a}(a)}` 
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

> ```lichen raw[...]``` is a lowlevel array that each element is a lowlevel value.
> ```lichen raw <lowlevel value>``` is a lowlevel value.

### Annotaion

An annotated expr creats a new expression with each annotated attribute:
- unified with the corresponding annotation expression.
- replaced by the corresponding annotation expression.

```sytax
`raw[value, type1, ...] : type2` => `{type1 unify type2; raw[value, type2, ...]}`
``` 

## Type

Annotation: ```lichen <expr> : <type>```.

### Compound

Compound type is the type of container value, e.g. tuple, struct, array. 

A compound type's value is an representation of a raw array of expressions, each unify its corresponding element in container value.

```sytax
`tuple{e, ...}` := `raw[raw[e, ...], TupleType, ...]`
`(e1, e2)` := `raw[[e1, e2], tuple{e1, e2}, ...]`
```

```sytax
`array{e0, length}` := `raw[raw[e0, length], ArrayType, ...]`
`[e1, e2]` := `raw[raw[e1, e2], {e1 unify e0; e2 unify e0; array{e0, 2}}]` 
```

## Subtype

Unify is directed, extend to subtype.

```sytax
`a unify b` => `a <: b`
```

### Any

Subtype extension in lowlevel, `Any` is a lowlevel value which is super type of all others.

Literal `@any` is an any expression that its attribute array node has value `Any`.

```sytax
`@any` := `raw Any`
```  

### Attribute

Attributes can resgister a custom function `is_subtype` to override its subtype behavior.

## Undecide

Undeicde is a state of lowlevel node that has no value.

Placeholder `_` is an undecide expression that its attribute array node is undecide.

```sytax
`_` := `raw None`
```

Nodes inside same class can be classified by tag.

```sytax
`{x = _; (x, x, _)}` := `raw['a, 'a, 'b]`
```

## Function

Function is an expression with a specified undecide expression as parameter.

Polymorphism at every application: clone its expression, unify the argument with cloned parameter.

```sytax
`{f = p => body; f a}` => `{let (p_clone => body_clone) = clone (p => body); a unify p_clone; body_clone}`
```

## Struct

Struct is a special tuple that: 
- Each index has a name.
- Nominal over the sytax position.

```sytax
`{s = struct{.n1 e1 .n2 e2}; s}` := `raw[raw[e1, e2], raw[[raw name_table, raw mominal_id], StructType, ...], ...]`

`s(.n2 e3, .n1 e4)` 
:= 
`{
  v = raw[raw None, raw None]; 
  t = raw[e1, e2];
  raw v[name_table["n2"]] unify e3; 
  raw v[name_table["n1"]] unify e4;
  raw t[name_table["n2"]] unify e3; 
  raw t[name_table["n1"]] unify e4;
  raw[v, s, ...]
}`
```

> ```lichen raw t[name_table["n1"]]` is lowlevel computation.

## Preprocessor block

A lichen source file has at most one single preprocessor `---...---` block.

Preprocessor block can define metadata:

`name = "value" `

or import module:
- `name = import "path"`: import a lichen file module.
- `name = link "path"`: import a dynamic linked module.
- `name = plug "path"`: import a compiler plugin module.

or depend on package:

`name = depend "url" [rev = "…"] [branch = "…"] [tag = "…"] [package = "…"] [sub = "…"]`

