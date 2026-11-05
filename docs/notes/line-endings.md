# Line endings: LF in the repository, LF in the checkout

> Status: current
> Ruled by: `AGENTS.md`, "Verify" — the line that says `cargo fmt` writes LF.

## The rule

`.gitattributes` states it, and it is the only authority:

```
* text=auto eol=lf
```

The repository holds **LF**, and so does the working tree — on every platform.
`eol=lf` is the half that makes it forced: `text=auto` alone leaves the checkout
form to `core.eol`, whose default is `native`, so a Windows checkout comes out
CRLF and the two machines disagree without saying so.

Nothing is excepted, batch files included. They used to be: "cmd.exe binds its
block and `goto` parsing to CRLF" is old cmd's behaviour, and it does not
reproduce here — a batch file containing only LF ran byte-identically to its CRLF
twin under cmd.exe 10.0.26100 (labels and `goto`, parenthesised blocks, `^`
continuations, delayed expansion, `for /f`, `call :label`, `goto :eof`).

## Why the rule is forced rather than inferred

The checkout form must not be a property of the machine. It was, once:
`core.autocrlf=true` sat in this machine's *system* gitconfig, so 785 of 867
tracked files were CRLF in the working tree while `git status` reported nothing —
and a check that compared worktree bytes against LF reported 7087 problems on a
checkout that git itself considered correct. Both readings were unusable: one
said everything was fine, the other said nothing was.

Naming LF in the repository ends the argument. A clone, a CI runner and a fresh
Windows checkout all see the same bytes, and `rustfmt` (which writes LF) and the
working tree no longer disagree.

## A file that is out of line

A tool that writes a file itself — `rustfmt`, a code generator, an editor with
`files.eol: "auto"` — leaves it in whatever form it chooses, and `core.autocrlf`
on the machine may bring CRLF back. The working tree form is not part of a
commit, so nothing shows up in `git status`; `git ls-files --eol` and
`windows-dev.ps1 eol .` show it. Re-materialise from the index, which is where
the conversion happens:

```bash
rm <path> && git checkout -- <path>
```

Do not `git checkout -- .` (it discards uncommitted edits) and do not hand-edit
the bytes: the index is the source and git already knows how to project it.

## A source file is not text if it holds a NUL

`git grep`, `git diff` and most editors decide "binary" by a single NUL byte, so a
raw NUL inside a `&[u8]` literal turns a `.rs` file into a blob that
`git ls-files --eol` reports as `-text` and that no line-ending rule applies to.
Write the escape (`b"main\0"`), not the byte: the value is identical and the file
stays text. The same applies to a stray lone `CR`: `text=auto` treats it as a
binary signal, which is how `docs/notes/compute-replay-handoff.md` came to store
CRLF in the repository while every other note stored LF.
