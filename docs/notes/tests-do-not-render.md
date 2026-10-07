# Tests do not assert what the printer prints

> Status: **current, and a rule from the maintainer.** Points at: every test that
> renders a `TypePrinter`/`ValuePrinter` result — `crates/lichen-language/tests/`
> (`*_render_*`, the kernel value/type assertions), `crates/lichen-language-server`
> (hovers, statement values), `crates/lichen-render`'s own renderer tests.

A test asserts a **semantic fact** — a value, a type compared as a structure, a
diagnostic's kind and position, a store's or a session's effect — never a string
the printer produced. Three reasons, each already paid for:

- **The printer's spelling is a display decision, not a contract.**  The `raw[…]`
  mark ([raw-rendering-mark](raw-rendering-mark.md)) was introduced precisely
  because a fallback reading was ambiguous with a form the printer understood; a
  test that pinned the old spelling failed for a *better* rendering.  Measured:
  fixing the cross-module universe reading turned
  `(43, 44): <raw[Int, Type], raw[Int, Type]>` back into `<Int, Int>`, and a test
  asserting the string would have had to be edited for a printer bug fix.
- **It couples a test to a layer it is not testing.**  A kernel test that reads a
  type through the printer starts failing when a *lowering* decision changes how
  the type is encoded, so the failure names the wrong subsystem and the fix is
  looked for in the wrong place.
- **The printer has its own tests.**  A rendering question belongs there, where the
  subject *is* the rendering and a change of spelling is the change under test.

The rule as given: **no test may depend on the printer.**  Two well-known surfaces
are the open question, because a rendering *is* the thing under test in both:

- `crates/lichen-render`'s own tests, where the printer is the subject;
- the `examples/*.lichen` `output =` declarations, which are the language's
  *documented observable behaviour* for a reader, mirrored into `README.md` by the
  repository's own `sync-readme`.

Both are recorded here as the scope still to be settled, not as exceptions already
granted.

**What the rule has already converted, as a record of how it is applied.**  The
operator routing's open class made three expectations printer-dependent at once
and each was converted on its own terms
([operator-polymorphism](operator-polymorphism.md) §7.1 cost 2):

- `crates/lichen-language-server`'s imported-field hover asserted `?a -> ?a`; it
  now asserts the fact the string stood for — the type's two sides are the *same*
  open cell and both are named — and says nothing about how the operand group's
  placeholder pair is spelled.
- the wrapper hover asserted the type variables' **letters**; it now reads the
  names out and asserts the relation (the wrapper's `.I`/`.O` are its own two
  cells), which is insensitive to which cell the checker
  numbered first.
- the two `examples/import/*.lichen` `output =` declarations are in the second
  surface above, so they were **re-pinned**, not converted: an example's declared
  output is asserted as the language's observable behaviour by
  `crates/lichen-language/tests/examples.rs`, and there is no semantic form for it
  yet.  That conversion is the open half this note records, and until it happens a
  printer spelling change costs those two files an edit.
