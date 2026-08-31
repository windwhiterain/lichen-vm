# Build-time investigation: `lichen-language`

## Summary

Before the fix, a clean build of the `lichen-language` crate took **~10 minutes**
even though all workspace dependencies compiled in about **15 seconds**.

The bottleneck was **not code generation** and **not the dependencies**. It was
rustc's frontend spending **~9 minutes** type-checking one large `chumsky`
parser-combinator type in `crates/lichen-language/src/parse.rs`.

The fix was to add **one strategic `.boxed()` call** in `atom_parser()`, which
erases the monolithic parser type before it is threaded through the expression
precedence chain. A clean `lichen-language` build now takes **~8.5 seconds**.

## Reproduction / measurements

Commands used:

```powershell
cargo clean -p lichen-language
cargo build -p lichen-language --timings --message-format=short
```

### Before the fix

| Item | Time |
|---|---:|
| `cargo` total | 10m 16.3s |
| `lichen-language` lib | 614.8s |
| rustc frontend | 560.87s |
| rustc codegen | 53.93s |
| `lichen` binary | ~1.4s |
| `sync-readme` binary | ~1.4s |

This showed clearly that the long build was rustc's type-checker/frontend, not
dependency compilation or final code generation.

### After the fix

| Item | Time |
|---|---:|
| `cargo` total | 8.55s |
| `lichen-language` lib | 7.79s |
| rustc frontend | 3.54s |
| rustc codegen | ~4.25s |
| `lichen` binary | ~0.7s |
| `sync-readme` binary | ~0.5s |

## Root cause

`crates/lichen-language/src/parse.rs` uses `chumsky`'s typed combinator API.
`atom_parser()` builds a very large parser type:

* `choice(...)` over integer literals, type constants, names, parenthesised
  expressions, array/table literals, blocks, angle tuples, struct types, and
  if-expressions;
* followed by a postfix chain for indexing, table lookup, array types, and
  paren/struct forms;
* that whole typed parser is then used as the atom parser throughout the
  recursive precedence-based `expression()` grammar.

`chumsky`'s guide specifically warns that long combinator chains can cause
exponential work for Rust's trait solver. Without type erasure, the compiler
has to reason about one enormous concrete parser type, and it did so for
several minutes.

## Fix applied

In `atom_parser()`:

```rust
fn atom_parser<'a>(
    tokens: &'a [Token],
    expr: impl Parser<'a, In<'a>, Expr, E<'a>> + Clone + 'a,
) -> impl Parser<'a, In<'a>, Expr, E<'a>> + Clone {
    ...
    primary
        .then(postfix.repeated().collect::<Vec<_>>())
        .map(|...| { ... })
        .boxed()
}
```

* `.boxed()` converts the huge concrete parser type into `Boxed`, an
  `Rc<dyn Parser>`. The allocation happens only when the parser is **created**,
  not during parsing.
* `+ 'a` is required on the generic `expr` parameter to satisfy the lifetime
  bound of `chumsky::Parser::boxed()`.

## Additional type-erasure experiments

After the main fix, I also tried boxing more parser-producing functions:

* `paren`, `array_literal`, `table_literal`, `tilde_marked`,
  `angle_tuple`, `struct_type`, `block`, `if_expr`
* `paren_fields`, `statement_list`, `statement`, `binding`, `operand`,
  `expression`, `program_parser`

### Boxing the atom sub-parsers

Boxing the helper parsers used inside `atom_parser()` produced only a small
change:

| Variant | Clean build |
|---|---:|
| `atom_parser` only | 8.55s |
| `atom_parser` + atom sub-parsers | 8.48s |

The difference was within run-to-run noise: the main monolithic type was
already erased before it propagated into the precedence chain, so the extra
erasure did not meaningfully reduce frontend work.

### Boxing even more of the grammar pipeline

Boxing `statement_list`, `statement`, `binding`, `operand`, `expression`,
`program_parser`, etc. made the build slightly **worse**:

| Variant | Clean build | Frontend | Codegen |
|---|---:|---:|---:|
| `atom_parser` only | 8.55s | 3.54s | ~4.25s |
| all additional helpers boxed | 9.89s | 3.78s | ~5.23s |

The extra `boxed()` calls added more indirection and dynamic dispatch at
parser-construction time without removing a remaining type-checking bottleneck.

## Conclusion

* The huge build time was caused by one un-erased `chumsky` parser type in
  `atom_parser()`.
* One `.boxed()` call there reduced the clean build from **~10 minutes** to
  **~8.5 seconds**.
* Additional `.boxed()` calls in the rest of `parse.rs` do not materially
  improve compile time; some make it slightly worse.
* The minimal, focused fix is the best option for this codebase.
