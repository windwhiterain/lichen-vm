# Tests do not assert what the printer prints

> Status: current — a rule from the maintainer.
> Points at: every test that
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

A dependency on a printer spelling is converted on its own terms, not by deleting the
assertion: the test asserts the semantic fact the string stood for. The
operator routing's open class
([operator-polymorphism](operator-polymorphism.md) §7.1 cost 2) is the worked
example — an imported-field hover now asserts that the type's two sides are the
*same* open cell and both are named, and the wrapper hover reads the type
variables' names out and asserts their relation, instead of pinning the
printer's spelling of either.

## Recovered measurements

**A member with no value is a refusal, not a `false`.** A value that was never
evaluated — a lazy read nothing forced — has no answer to compare, so the shared
comparison helper **panics** on it rather than returning `false`: silently calling
two of them equal is what would make a comparison of two backends vacuous.

**A run-time refusal means the value never arrived.** `common::finish` mirrors
`run::render_build`'s refusal gate: a program that produced nothing while a refusal
was recorded is a failure, not an empty answer. A refusal explains a value that
never arrived; a program that produced one has nothing to explain.
