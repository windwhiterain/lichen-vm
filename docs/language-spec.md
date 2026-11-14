# Language spec 

Illustration only, not precise.

## Syntax 

core:
```sytax
program := block_inner
expr := name | atom | unify | lambda | apply | block | annotated | compound | access | instantiate
block := `{` block_inner `}`
block_inner := ( (stmt | expr) sep)* expr
sep := `,` | `;` | `\n`
stmt := bind | assert
bind := [`let`] name `:=` expr
assert := `@assert` expr
unify := expr `=` expr
lambda := expr -> expr
apply := expr expr
annotated := expr annotation+
annotation := annotator expr
annotator := `:` | `!` | `?` | `#`
compound := tuple | array | struct | table | set
tuple := `(` (expr sep)* `)`
array := `[` expr'template sep expr'length `]`
struct := `{` (`.` name expr sep)* `}`
table := `table` glue `{` expr'key_template sep expr'valye_template `}`
set := `set` glue `{` expr'template `}`
access := tuple_access | struct_access | array_access
instanciate := array_instanciate | struct_instanciate | table_instanciate | set_instanciate
array_instanciate := expr'array glue `[` (expr sep)* `]` 
struct_instanciate := expr'struct glue `{` (`.` name expr sep)* `}`
table_instanciate := `table` glue `(` expr'table `)` glue `(` (expr'key `=>` expr'value sep)* `)`
set_instanciate := `set` glue `(` expr'set `)` glue `(` (expr sep)* `)`
tuple_access := tuple glue `(` expr `)` 
struct_access := struct `.` name
array_access := array glue `[` expr `]`
atom := number | string | `_` | builtin
```

> In ```sytax <sytax>'<tag>```, `'<tag>` has no sytax meaning, just a tag to that sytax.

sugar:
```sytax
`a <: b ` => `a = b _`  
condition := `if <expr'1> then <expr'2> else <expr'3>` => `(<expr'3>, <expr'2>)(<expr'1>)
bin_operation := `<expr'1> <bin_operator> <expr'2>` => `<bin_operator> (<expr'1>, <expr'2>)`
` ...a... -> ...b...` => `Function{a := _; b:= _; @parameter ...a...; @body ...b...}`
`{...; return <expr>; ...; <stmt>}` => `{...; <expr>}`
`{...; a := expr'1; ...; b := expr'2; ...; <stmt>}` => `{...; {.a expr'1, .b expr'2}}`
`{...; pub a := expr'1; ...; b := expr'2; ...; <stmt>}` => `{...; {.a expr'1}}` 
`{.'a ..., ...}` => `{.a 'a ..., ...}`
```

> Inside ```sytax `...` ``` is lichen code ```lichen ...```, inside ```sytax `<...>` ```  is sytax ```sytax ...```.
> ```sytax ... => ...``` is sytax conversion.
> `<expr> @raw-> <expr>` is lowlevel function.
> `Function{<block_inner>}` is a function scope.

## Ideal API
1. Uniform signature: use if all parameter play equal role.
```
sum3 := [_ -> (_: Int), 3]['a, 'b, 'c] -> 'a + 'b + 'c
sum3 (_[1, 2, 3])
```
2. Named signature: use if the signature will not be used in other API.
```
div := {.'dividend: Int, .'dividor: Int} -> dividend / dividor
div _{.dividend 2, .dividor 1} 
```
3. Struct signature: use if the signature will be used in other API.
```
Div := _ -> {.dividend _: Int, .dividor _: Int}
div := 'x = Div _ -> x.dividend / x.dividor
div _{.dividend 2, .dividor 1}
```

4. Function generic: can formulate any constraint.
```
div := 'T -> {.'dividend: T, .'dividor: T} -> dividend / dividor
div _ _{.dividen 1, .dividor 2}, div _ _{.dividen 1.0,.dividor 2.0}
```

5. Nested
```
A := _ -> {.'x: Int, .'y: Float}
B := _ -> {.a A _, .'z: String}
f := {.a A _, .b B _, .'w: Type} -> ...
```

## Lowlevel

Lowlevel is a graph of nodes.
A node can have an value, an operation, and a class.
An operation can have an operand node.
Nodes inside same class must share same value, class is merged by unify.
An value can be Type or an array of nodes.

## Unify
Unify is a lowlevel recursive action that unify two node will merge into one node if those satisfied:
- recursive into nodes both side refers and apply the same rule.
- if both side are recursively equal or any side is undecided, merge into one class.
or else produce an unify error. 

## Merge
nodes are merged into one class by unify. evaluating node will trigger operations in all nodes in its class run. 

after running an operation, unify the value to the class's value.

for memory locality, the first value writing to a class will propagate to all member nodes.

## Operation
for effeciancy, one operation only run once.

## Attribute

An expression is a raw array of attributes, value attribute is a lowlevel value, other attribute is an expression recursively, attributes is extentable.

```sytax
expr := `@raw[@raw value, type, refinement, doc, ...]`
```

> ```lichen @raw[...]``` is a lowlevel array that each element is a lowlevel value.
> ```lichen @raw <lowlevel_value>``` is a lowlevel value.

### Annotaion

An annotated expr creats a new expression with each annotated attribute:
- unified with the corresponding annotation expression.
- replaced by the corresponding annotation expression.

```sytax
`@raw[@raw value, ann1, ...] : ann2` -> `{ann1 = ann2; @raw[@raw value, ann2, ...]}`
``` 

## Type

Annotation: ```lichen <expr> : <type>```.

expression with unit vairant lowlevel value has type `Type`

`Type` is self referencial expression:

```sytax
`Type` := `@raw[@raw TypeType, Type, ...]`
```

## Compound

Compound type is an layout documentation of lowleval array value, e.g. tuple, array, struct. 

Tuple has no special layout.

```sytax
`Tuple` := `@raw[@raw TupleType, Type, ...]`

`(e1, e2)` := `@raw[@raw[e1, e2], TupleType, ...]`

`t(n)` := `{(type_of t) = Tuple; @raw t.value[n]}` 
```

Array unify every element expression to a template expression.

```sytax
`Array` := `@raw[@raw ArrayType, Type]`

`[template, n]` := `@raw[@raw[template, <`_`*n>], Array, ...]`

`e0[e1, e2]` := `{
  let [template, length] := e0
  length = 2
  e1 <: template
  e2 <: template
  raw[raw[template, e1, e2], Array, ...]
}`

`a[n]` := `{(type_of t) = Array; @raw t.value[n+1]}` 
```

### Struct

Struct is a special tuple that: 
- Each index has a name.
- Nominal over the sytax position.

```sytax
`Struct` := `@raw[@raw StructType, Type, ...]`

`
s := _ -> {.n1 e1 .n2 e2}
name_table := @raw Table
` 

`s` := `_ -> @raw[@raw[name_table, nominal_id, e1, e2], Struct, ...]`

`s{.n2 e3, .n1 e4}` 
:= 
`{
  let _{.n1 e1, .n2 e2} <: s ;
  v := @raw[name_table, nominal_id, @raw None, @raw None]
  e3 = @raw v[name_table["n2"]+2]
  e4 = @raw v[name_table["n1"]+2]
  @raw v[name_table["n1"]+2] = e1;
  @raw v[name_table["n2"]+2] = e2;
  @raw[@raw v, Struct, ...]
}`

`s.<name>` := `{type_of s = Struct; @raw s.value[s.value.name_table[<name>]+2]}`
```

> ```lichen raw t[name_table["n1"]+2]` is lowlevel computation.

### Table
```sytax
`Table = @raw[@raw TableType, Type, ...]`

`table{kt, vt}` := `@raw[@raw[kt, vt], Table, ...]`

`table(t)(k1 => v1, k2 => v2)` :=
`
{
  let table{kt, vt} = t
  k1 <: kt
  k2 <: kt
  v1 <: vt
  v2 <: vt
  @raw[kt, vt, @raw Table(k1 => v1, k2 => v2), ...]
}
`
```

### Set
```sytax
`Set = @raw[@raw SetType, Type, ...]`

`set{t}` := `@raw[@raw[t], Set, ...]`

`set(t)(e1, e2)` :=
`
{
  let set{t} = t
  e1 <: t
  e2 <: t
  @raw[t, @raw Set(e1, e2), ...]
}
`
```

### Attribute Subtype

Attributes can resgister a custom function `is_subtype` to override its subtype behavior.

Subtype via post unify error suppression.

## Undecided

Undecide is a state of lowlevel node that has no value.

Placeholder `_` is an undecided expression that its attribute array node is undecided.

```sytax
`_` := `@raw None`
```

Nodes inside same class can be classified by tag.

```sytax
`{x := _; (x, x, _)}` := `@raw['a, 'a, 'b]`
```

## Function

Function is a lowlevel scope with specified two expr: parameter and body.

Function scope is hierarchical, nodes in child scope also belongs to parent scope.

Polymorphism at every application: @clone function's expressions, unify the argument with the cloned parameter.

```sytax
`Function` := `@raw[@raw FunctionType, Type, ...]`
`<p> -> <body>` := `@raw[ <p> @raw-> <body>, Function, ...]`
`{f := <p> -> <body>; f a}` => `{let (p_clone -> body_clone) = @clone (<p> -> <body>); a = p_clone; body_clone}`
```

> `p @raw-> body`: lowlevel function.
> `@clone f`: see section `### Clone`.   

### Clone
Clone is a recursive clone within certain scope, triggered by function apply.

Undecided node, if the outer most scope among its class members is or inside the applying function's scope, must be clone. 

Clone more nodes should make not change to the language behaviour.

For efficency:
- Clone recursive into a decided node if it the "deep_decided" flag which set during deep evaluate is false.
- Reference node outside the applying function's scope

Clone an undecided node will detached from its original class for let-polymorphsim.

Cloned node lives in the scope where the apply happens.

Auto create a side apply for inner function apply, the original apply's body unify a side apply's body, the side apply will not be able to evaluate util the next time be cloned by the outer function's apply, so that if the outer function applying clone made the innner applying function specialized, the specialized side apply's body can unify the unspecialized oringinal apply's body.

## Preprocessor block

A lichen source file has at most one single preprocessor `---...---` block.

Preprocessor block can define metadata:

`name = "value"`

or import module:
- `name = import "path"`: import a lichen source module.
- `name = link "path"`: import a dynamic linked module.
- `name = plug "path"`: import a compiler plugin module.

or depend on package:

`name = depend "url" [rev = "…"] [branch = "…"] [tag = "…"] [package = "…"] [sub = "…"]`

