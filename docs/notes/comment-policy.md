> **Status:** current — the rule is enforced in CI, and the tree satisfies it

# The comment policy

A comment earns its place by stating something the code cannot: an invariant
that must hold, or the condition that makes an `unsafe` block sound. An
implementation invariant belongs at the code it governs, not only in a note: the
comment states the rule, and [`docs/notes`](../README.md) carries the argument, the
measurement and the history the comment links to. An agent changing the code
reads the comment, not the note, so a note is never the only home of a rule.

The rule is enforced by [`comments-check`](https://github.com/windwhiterain/comments-test),
run from the `ci` workflow against [`comments-test.toml`](../../comments-test.toml).
The configuration is the single source of truth: it states the rule and the
numbers, and the CI step only names it.

## What is checked

A comment is measured per **markdown section**, in Unicode characters, so a
non-ASCII comment is not charged more than a reader would see. A section's
budget covers everything below it, so a limit states what one concern may cost
rather than how tall a comment block may be.

Every comment kind is bound to a template, and each template admits two headings:

| Section            | Budget | What it is for                                    |
| ------------------ | ------ | ------------------------------------------------- |
| `path = ""`        | 120    | The prose before the first heading — and the whole comment when it has no heading at all. A summary line plus one pointer. |
| `# Invariant`      | 400    | What must hold, stated at the code that must keep it. |
| `# Safety`         | 400    | What makes calling into `unsafe` sound.           |

Any other heading is a `heading-not-allowed` violation, at any depth. The two
above are the only statements admitted that outrank prose; `# Contract` and
`# Reference`, which the tool's own repository uses, are not accepted here
because the contract is what `# Invariant` states, and a reference is one
pointer — which is what the root budget is sized for.

## Where an invariant lives

The **code comment is the contract and the note is its argument**. A rule the
code must keep is stated in a `# Invariant` section at the site that must keep
it; the note records why it exists, what was measured, and what was rejected.
The note links to the code section when it needs to name the rule, rather than
restating it in different words — two statements of one rule drift apart, and
the one an editor reads is the code's.

- **A pointer is not a rule.** `See docs/notes/...` under a `# Invariant` heading
  is background, not an invariant; the sentence that must hold has to be there.
- **The budget is for the rule, not the argument.** 400 characters is enough for
  what must hold. If the argument does not fit, it belongs in the note; the rule
  does not shrink to make room for it.
- **A local rule uses a `//` comment.** An inline comment is bounded at 120
  characters, so it carries only the condition the next line relies on; the
  argument goes to the note.
- **The language contract is the exception.** `docs/language-spec.md` and the
  book state what a program means; a code comment citing one is implementing it,
  not restating it.
- **This is not mechanically checked.** `comments-check` checks a comment's
  shape; it cannot tell whether the sentence is true or whether a note is the
  only place a rule is written. The rule above is review, and the code comment is
  what the reviewer reads.

Inline `//` comments get no heading to carry and are bounded at the root budget
instead. They are still measured: a bare note is exactly what drifts into noise.

## Two ways to opt out

Both are statements the author makes deliberately, rather than gaps in the check.

- **Length.** A marker line inside the comment silences one rule or all of them:

  ```rust
  /// # Safety
  /// This section is long because it is the specification of a wire format.
  /// comments-check: allow length
  ```

- **Path.** `exempt` takes `/`-separated patterns with `.gitignore` syntax. This
  repository exempts nothing it owns; the one entry is the nested clone under
  `lichen-language-zed/grammars/lichen`, whose comments are the source tree's
  comments one directory down.

## What is not checked

`exclude` matches a **directory name** at any depth, not a path, so it can only
name a leaf: `target`, `.git`, `.worktrees`, and `.scratch-core`. Anything
needing a path pattern goes in `exempt` instead.

The workspace's `examples/` and `helix/` trees hold no Rust, so they contribute
nothing today; a `.lichen` example is not a comment and is out of scope for a
Rust comment checker.

A **directory walk** does not check a Markdown file at all. `comments-check
--config comments-test.toml . --max-violations 0` reports nothing for a note
whose prose is over budget, while naming that file directly does check it. This
was proved with a fixture, and it is why a slice that edits a note passes the
file's own path as well as the repository root: the root run is the gate, and
the per-file run is what actually reads the note.

## The state of the tree

The gate covers the whole tree, which is the honest scope, and it passes: run at
the CI step's own settings, `comments-check --config comments-test.toml` reports
no violations. The remediation that brought it there is the body of work that
also produced these notes — the argument a comment budget could not hold was
moved here, where it can be read rather than skimmed. The rule itself stayed at
the code; only the argument moved.

The check runs **first** in the `gate` job, above `Check formatting`. Nothing
about a comment depends on the code compiling, and a comment that breaks the
policy is the cheapest thing in the job to find out about.

## Reading a report

`comments-check` prints **10 violations** by default and folds the rest, because
a full report on a tree this size is megabytes of JSON — enough to fill a
reader's context on its own, and the reason an agent would re-read it and get
less out each time. Pass `--max-violations N` to print more, or `--max-violations
0` for every one.

The fold is a rendering choice and changes nothing else: the exit code is decided
from what the run found, and the per-code counts cover all of it. What stands in
for the omitted violations is the distribution by file, so the next step is to
name a file from that list and check it on its own — the target is positional,
so `comments-check crates/lichen-compute/src/compute.rs`.

**A caller that counts the printed violations is reading a truncated report.** The
default is a *rendering* limit, so a script that filters the report for its location
lines and counts them answers "ten" on a tree with more — and the summary that says how
many were folded is precisely what such a filter discards. Anything that needs the real
number passes `--max-violations 0`. That mistake was made here: a verification compared
two reports by their location lines, read "10 on each side, identical sets", and drew a
conclusion from two truncated views. The sets did match; the *count* was never the
tree's.

