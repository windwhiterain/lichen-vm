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

**constructor (type constructor)** — an ordinary function that builds a type
value. Chapter 10.

**currying** — writing a multi-argument function as nested one-argument
lambdas; `x => y => …`. Chapter 4.

**cycle cut** — the `…` in a printed type or value, marking a node the walk
has already shown. Chapter 12.

**dependent type** — a type with a value in it, such as `array{Int, n}`;
the value is decided by unification. Chapters 10 and 12.

**doc** — the `?` slot: a label for tools that never constrains. Chapter 13.

**expression** — the unit of a lichen program; everything is one.
Chapter 3.

**function type** — a function's type is the function itself (`f : f`), so
its printed halves are one signature. Chapters 4 and 12.

**generic (parametric) type** — a type value produced by a function, such as
`Box Int`. Chapter 10.

**hole** — the `_` placeholder in value position: a place the context
supplies, printing `none` if nothing does. Chapter 5.

**identity (nominal)** — a struct type's identity is its occurrence, shown
as `#id` in diagnostics; two occurrences never unify. Chapter 8.

**kernel** — a function written in the compute language that runs once per
lane. Chapter 17.

**kind** — the type of a compound type value: `TypeTuple`, `TypeArray`,
`TypeStruct`, `TypeTable`, `SetType`. A kind is an ordinary type.
Chapters 5 and 10.

**lane** — one invocation of a kernel, indexed by `compute.range`.
Chapter 17.

**lazy marker** — `none`: the value of a computation with no answer, or of
an unfilled hole. Chapter 6.

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

**pin** — a slot with a value, `v : T`; the pin is part of the type value
and is checked against the value that fills the position. Chapter 10.

**placeholder** — `_`. Chapter 5.

**plugin** — a crate composed into a compiler at build time, contributing
vocabulary, operators, attributes, or a sub-language. Chapter 19.

**prelude** — the built-in `core` module whose names are in scope in every
program. Chapter 11.

**record** — a block ending in bindings, whose value is an anonymous struct
of them. Chapter 8.

**refinement** — the `!` slot: a predicate that must answer `1`; on a type,
it receives the type value. Chapter 11.

**slot** — one position of a compound value or type: its value, its type,
and its attributes. Chapter 10.

**struct** — a nominal type with named fields. Chapter 8.

**table** — a constant key→value mapping with its own read. Chapter 9.

**type** — a value; the second half of every expr. Chapter 5.

**unification** — the operation that finds a type by making two described
shapes agree. Chapter 5.

**value** — the first half of every expr; what a program computes.
Chapter 5.

**wildcard** — the unconstrained value position of a compound type's slot;
`tuple{Int}` accepts any integer. Chapter 10.
