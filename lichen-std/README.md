# lichen-std

A small standard library for lichen, packaged as a git-fetchable dependency used
to test the package manager's `depend "url"` directive.

It lives in this monorepo's `lichen-std/` directory, so depend on it with the
`sub` option:

```lichen
@{
  depend "https://github.com/windwhiterain/lichen-vm" as std sub = "lichen-std"
  std = import "std"
@}
(std.add 40 2, std.double 5, std.inc_twice 5, std.succ 41)
```

The entry package is `lib.lichen`, whose final expression (a struct of
functions) is the module's export.  `import "std"` binds that module; fields
are accessed as `std.add`, `std.succ`, and so on.
