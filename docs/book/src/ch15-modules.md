# Modules, Imports, and Packages

Chapter 8 established that a file ending in bindings is a **record
program**: its value is an anonymous struct of its bindings. This chapter
uses that: a module is a file, importing it binds its value, and a package
is a module that a project declared a dependency on.

## The header's directives

A file may open with one `---…---` block (Chapter 1). The block's entries
come in two kinds: **metadata** that tools read (`doc`, `output`,
`order`), and **directives** that change what the file can reach:

```text
name = import "path"          -- another program, as a package value
name = depend "url" [rev = "…"] [branch = "…"] [tag = "…"]
                    [package = "…"] [sub = "…"]
name = plug "url" [rev = "…"] [branch = "…"] [tag = "…"]
                  [package = "…"] [sub = "…"]
name = link "path"           -- a dynamic library the loader opens
```

An `import` is loaded when the file compiles. A `depend` is a git
dependency the package manager fetches; the source is then staged where
the import path can reach it (below). A `plug` declares a **native plugin**
— a Rust crate that extends the compiler — and it is fetched and composed
by the package manager the same way (Chapter 19). The language compiler
itself never fetches anything; it parses and surfaces the directives.

A `link` names a **dynamic library** the loader opens when the file compiles:
a Rust `dylib` whose manifest carries both the lichen source of its
operators and the entries they run. Nothing is fetched and nothing is
rebuilt — the library is opened per run, and its build tag must match the
compiler ([a native link library](https://github.com/windwhiterain/lichen-vm/blob/dev/docs/notes/native-link-library.md)).

## Importing a module

Two files in one directory:

```lichen math.lichen
succ := x => x + 1
add := (x : Int) => (y : Int) => x + y
```

```lichen main.lichen
---
  math = import "math.lichen"
---
(math.succ 41, math.add 1 2)
```

```text
(42, 3): Tuple
```

`main.lichen`'s header binds `math` to the value of `math.lichen`. That
value is a record — a struct, so its type is `Struct` — and `math.succ`
reads the record's field. The path is relative to the **importing file's**
directory.

The imported value need not be a record. It is whatever the imported
program evaluates to, so a file that ends in an expression imports as that
expression's value:

```lichen plain.lichen
1 + 2
```

```lichen uses-plain.lichen
---
  p = import "plain.lichen"
---
p
```

```text
3: Int
```

The record idiom is the useful one, because it gives the module named
exports.

### Nested imports

An imported module may import its own dependencies. Here `geo.lichen`
imports `math.lichen`, and the top-level program imports `geo.lichen`:

```lichen geo.lichen
---
  math = import "math.lichen"
---
double := x => math.add x x
```

```lichen uses-geo.lichen
---
  geo = import "geo.lichen"
---
geo.double 5
```

```text
10: Int
```

The import graph is resolved before the importing program is checked, and
each module is loaded once.

### What a module exports

A record's fields are selected by the rules of Chapter 8: a `name := expr`
binding is a field, `let` is never a field, a bare expression is never a
field, and `pub` selects — when any statement is `pub`, only the `pub`
ones are fields.

```lichen library.lichen
pub a := 1
pub b := 2
```

```lichen uses-library.lichen
---
  lib = import "library.lichen"
---
(lib.a, lib.b)
```

```text
(1, 2): Tuple
```

### When a module fails

A module that does not check is reported at the import, with the
module's own message:

```lichen broken.lichen
1 + "a"
```

```lichen uses-broken.lichen
---
  bad = import "broken.lichen"
---
bad
```

```text
error: cannot load package 'broken.lichen': expected Tuple, found Tuple
  --> 2:3
   |
 2 |   bad = import "broken.lichen"
   |   ^
```

The position is the import site, because that is the point in *this* file
that asked for the module; the message is the module's. A path that does
not exist is a similar message, ending with the operating system's own
text:

```text
error: cannot load package 'nope.lichen': cannot read package …: …
```

## The prelude

Every program is seeded with the built-in **core** module. Its names are
in scope with no import: `Num`, `in_num`, and the eight operator
functions `add`, `sub`, `mul`, `div`, `less`, `greater`, `less_or_equal`,
`greater_or_equal` (Chapter 11 quoted part of its source).
The prelude is **shadowable, not reserved**: a program's own binding of a
name replaces the prelude's for that program.

```lichen
add := (operands : Tuple) => operands(0) * operands(1)
add (5, 2)
```

```text
10: Int
```

The program's `add` multiplies; the prelude's would have added. The
prelude is also a module value if you want its names qualified:

```lichen
---
  core = import "core.lichen"
---
core.add (1, 2)
```

```text
3: Int
```

The prelude is materialized as a real file, which is why a contract
failure inside it can name the line that wrote the contract (Chapter 11).

## The standard library

The repository ships a small library in [`lichen-std/_.lichen`](https://github.com/windwhiterain/lichen-vm/blob/dev/lichen-std/_.lichen):
`id`, `succ`, `add`, `double`, `compose`, `inc_twice`, and `type_of`. It is
an ordinary module, and a program in the repository's `examples/`
directory reaches it with a relative path:

```lichen std.lichen
---
  std = import "../lichen-std/_.lichen"
---
(std.type_of (1), std.inc_twice 5)
```

```text
(Int, 7): Tuple
```

The path is the price of not having a package registry in the import
syntax: the standard library is a file like any other. (The
`lichen-std-native` crate is the native-plugin sibling — the same library
carried by a plugin compiler, with operators the interpreter cannot
express.)

## Packages

A project that wants to use someone else's code names a dependency in its
header. `depend` takes a git URL and any of `rev`, `branch`, `tag`,
`package` and `sub`, on the directive's one line:

```text
---
  geometry = depend "https://example.com/geometry.git" tag = "v1.2" sub = "crates/lichen-geometry"
---
```

The **package manager** (`lichen`, in `crates/lichen-package`) reads the
directive, fetches the repository into the lichen-home source cache, and
stages the source under the package's import path. A `name = import
"geometry"` then resolves into the fetched source instead of a sibling
file. The package manager also downloads the compiler and language
server binaries for a lockfile entry, and it **rebuilds the compiler when
a native plugin is imported** — the plugin is composed into the binary,
not loaded dynamically.

The compiler binary alone never fetches: run `lichen-compiler` on a file
whose `depend` was never staged, and the import failure is the one above.
That split is deliberate — the compiler is a pure function of the sources
it is given — and it is the package manager's job to make the sources
present.

## Compiling a module

`lichen-compiler build <file>` compiles a file as a package and prints the
type it exports:

```text
$ lichen-compiler build examples/import/math.lichen
built …/examples/import/math.lichen
type: Struct
```

It is the quickest way to ask a module "what do you export?" The first
line names the package's path; the second is the type of its value — a
record program exports a struct, so its type is the layout type `Struct`.

## Summary

- A file is a program; a file ending in bindings is a record, and importing
  it binds that record.
- The header block carries metadata and directives: `import`, `depend`,
  `plug`.
- `math.succ` reads an imported module's field; the path is relative to the
  importing file; modules may import modules.
- A module's exports follow the record rules: bindings are fields, `let` and
  bare expressions are not, and `pub` selects.
- A module that fails is reported at the import with its own message.
- The core prelude's names are in scope, shadowable, and its module value is
  reachable as `core.lichen`.
- The standard library is an ordinary module; `lichen-compiler build` prints
  what a module exports.
- `depend` and `plug` declare packages the package manager fetches and
  stages; the compiler itself never fetches.

## Exercises

1. Write `math.lichen` and a program that imports it, and read two fields.
2. Write a module whose value is a plain integer and import it; what is the
   import's value?
3. Make a module with two `pub` bindings and one ordinary binding, and list
   which fields the import exposes.
4. Shadow the prelude's `add` with your own and show the difference.
5. Import the standard library and call `std.compose`.
6. Break a module on purpose and read the import failure.
