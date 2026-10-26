# Appendix D: Glossary

Terms the book uses, each with the chapter that teaches it.

**annotation** — a suffix on an expression that states a type, a
perspective, a refinement, or a doc: `e : T # p ! q ? d`. A mark `~ "…"`
is not an attribute. Chapter 13.

**attribute** — one of the schema slots an annotation fills: perspective,
refinement, doc. Each slot has its own relation and its own check.
Chapter 13.

**buffer** — a compute-layer value holding an array on a backend, read and
written by kernels. Chapter 17.

**cell** — a graph node whose value may not be computed yet; an *open*
cell is one whose type is not decided, printed `?a`. Chapter 5.

**claim** — the `~ "claim:…"` mark on a kernel body, stating its shape for
the fusion machinery. Chapter 17.

**class** — a set of type values used as a contract, such as
`set(Int, Float)`. Chapter 11.

**closure** — a function that kept the names visible where it was written.
Chapter 4.

**constructor** — a function that returns a fresh requirement when it is
applied: `S := _x => {.a _, .b _}`. A constructor is what `<:` applies.
Chapter 7.

**currying** — writing a multi-argument function as nested one-argument
lambdas; `x => y => …`. Chapter 4.

**cycle cut** — the `…` in a printed type or value, marking a node the walk
has already shown. Chapter 12.

**dependent requirement** — a requirement with a value in it, such as
`[@any : Int, n]`; the value is decided by unification. Chapters 10 and 12.

**doc** — the `?` slot: a label for tools that never constrains. Chapter 13.

**expression** — the unit of a lichen program; everything is one.
Chapter 3.

**function template** — a function written where a function would be, used
as the requirement of a plain `=` unification: `(x : Int => _: Int)`.
Chapter 4.

**function type** — a function's type is the function itself (`f : f`), so
its printed halves are one signature, written with `=>`. Chapters 4 and 12.

**generic (parametric) constructor** — a constructor built by a function,
such as `Box Int`. Chapter 10.

**hole** — the `_` placeholder in value position: a place the context
supplies, printing `none` if nothing does. Chapter 5.

**identity (nominal)** — a struct constructor's identity is its own name
table, so two constructors written apart never unify. Chapter 8.

**instance** — a compound built from a constructor: `T[x, y]` for an array,
`S{.x v}` for a struct. Chapter 7.

**kernel** — a function written in the compute language that runs once per
lane. Chapter 17.

**lane** — one invocation of a kernel, indexed by `compute.range`.
Chapter 17.

**lazy marker** — `none`: the value of a computation with no answer, or of
an unfilled hole. Chapter 6.

**layout type** — `Tuple`, `Array` or `Struct`: the shared type every
compound of that form has. The value owns the shape. Chapter 5.

**mark** — `e ~ "text"`: compile-time metadata with no value and no slot.
Chapter 13.

**module** — a file imported as a value; usually a record program.
Chapter 15.

**nominal** — see identity.

**package** — a module with a declared dependency; fetched by the package
manager. Chapter 15.

**expr** — every expression's value and type, printed `value: type`. The
book's first fact. Chapter 5.

**perspective** — the `#` slot: a uniformity claim, ordered by divisibility.
Chapter 13.

**pin** — a requirement that states a value, `5 : Int`; the pin rides in
the requirement the constructor returns and is checked against the value
that fills the position. Chapter 10.

**placeholder** — `_`. Chapter 5.

**plugin** — a crate composed into a compiler at build time, contributing
vocabulary, operators, attributes, or a sub-language. Chapter 19.

**prelude** — the built-in `core` module whose names are in scope in every
program. Chapter 11.

**record** — a block ending in bindings, whose value is an anonymous struct
of them. Chapter 8.

**refinement** — the `!` slot: a predicate that must answer `1`; on a type,
it receives the type value. Chapter 11.

**requirement** — one position of a compound: what the value in that
position must be. `@any : Int` asks for any `Int`; `5 : Int` pins the
value as well. Chapter 10.

**slot** — one position of a compound: its value, its type, and its
attributes. Chapter 10.

**struct** — a compound with named fields; a struct's requirement is
`{.name requirement, …}` and its instance is `S{.name value}`, where `S`
is a constructor. Chapter 8.

**table** — a constant key→value mapping with its own read. Chapter 9.

**template** — the requirement a constructor returns; `<:` applies the
constructor once and unifies the actual with the result. Chapter 7.

**type** — a value; the second half of every expr. Chapter 5.

**unification** — the operation that finds a type by making two described
shapes agree. Chapter 5.

**value** — the first half of every expr; what a program computes.
Chapter 5.

**wildcard** — the unconstrained value position of a requirement;
`(@any : Int, @any : Int)` accepts any two integers. Chapter 10.
