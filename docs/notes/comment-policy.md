> **Status:** current — the rule is enforced in CI; the tree is not yet compliant

# The comment policy

A comment earns its place by stating something the code cannot: an invariant
that must hold, or the condition that makes an `unsafe` block sound. Anything
longer belongs in [`docs/notes`](README.md), where it can be linked to instead
of duplicated next to the code.

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
| `# Invariant`      | 400    | What must hold. The strongest statement a comment can make about code. |
| `# Safety`         | 400    | What makes calling into `unsafe` sound.           |

Any other heading is a `heading-not-allowed` violation, at any depth. The two
above are the only statements admitted that outrank prose; `# Contract` and
`# Reference`, which the tool's own repository uses, are not accepted here
because a contract belongs in the note this comment links to, and a reference is
one pointer — which is what the root budget is sized for.

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

## The state of the tree

The gate covers the whole tree, which is the honest scope, and the tree does not
yet satisfy it. Measured at `af6f6f2` on `dev`, the check reports **4,158**
violations across 222 files read — 4,058 `section-too-long` and 100
`heading-not-allowed`, of which 879 sit in `tests/` directories.

That is a deliberate position, not an oversight: the rule is stated once and
enforced everywhere, and the remediation is a separate piece of work from landing
the rule. Three consequences follow for anyone working on the tree meanwhile:

- The comment check runs **last** in the `gate` job, after formatting, lint and
  tests. Checking it first would hide the other three until the tree is
  compliant.
- `ci` is red on this rule alone until the backlog is cleared. Everything else
  still reports.
- The count moves whenever the tree does, so re-run the command for a current
  number rather than trusting the one above.

Both are worth revisiting when the backlog reaches zero; the workflow comment at
the step says where to move it.

## Working through the backlog

`comments-check` prints **10 violations** by default and folds the rest, because
a full report on this tree is about six megabytes of JSON — enough to fill a
reader's context on its own, and the reason an agent would re-read it and get
less out each time. Pass `--max-violations N` to print more, or `--max-violations
0` for every one.

The fold is a rendering choice and changes nothing else: the exit code is decided
from what the run found, and the per-code counts cover all of it. What stands in
for the omitted violations is the distribution by file, so the next step is to
name a file from that list and check it on its own — the target is positional,
so `comments-check crates/lichen-compute/src/compute.rs`.

