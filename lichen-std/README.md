# lichen-std

A small standard library for lichen, packaged as a git-fetchable dependency used
to test the package manager's `depend "url"` directive.

It lives in this monorepo's `lichen-std/` directory, so depend on it with the
`sub` option:

```lichen
@{
  std = depend "https://github.com/windwhiterain/lichen-vm" sub = "lichen-std"
  std = import "std"
@}
(std.add 40 2, std.double 5, std.inc_twice 5, std.succ 41, std.type_of (1))
```

The entry package is `_.lichen`, whose final expression (a struct of
functions) is the module's export.  `import "std"` binds that module; fields
are accessed as `std.add`, `std.succ`, and so on.

`std.type_of` is the type read: `std.type_of (1)` is `Int : Type`,
`std.type_of [1, 2]` is `array<Int, 2>`, and in a type position it *is* the
operand's type, so `5 : std.type_of (5)` checks.  It is an ordinary lichen
function —

```lichen
type_of = x => {t = _; x: t; t}
```

— the placeholder binds the cell, the annotation unifies it with the
argument's type, and the body returns it.  That is why it lives here and not
in the language: nothing about it needs compiler support.  Bind it
(`type_of = std.type_of`) or pass it like any other function value.
