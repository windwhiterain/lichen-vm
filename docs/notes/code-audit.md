# Code audit and remediation queue

> Status: current — the audit inventory below was taken at `dev@e4c0bae`; the
> queue is the work list, and each item's `Status:` field is the live state.
> Points at: the code it names (every claim carries a `file:line`).

This is the one place the audit's findings live. It exists so the fixes can be
done one at a time, reviewed, and committed without re-deriving the analysis.

## How to read this

**Evidence legend** — every finding is marked:

- `verified` — re-read first-hand at the cited lines while writing this note.
- `reported` — found by a scoped read-only sweep and *not* independently
  re-verified. Treat the line reference as a lead, confirm before fixing.

**Status legend** — `todo` / `doing` / `done` / `blocked:<decision-id>` /
`wontfix:<why>`.

**Decisions** — items that cannot be fixed without a design call are marked
`blocked:Dn` and listed in [Decisions](#decisions). Do not silently pick an
answer for one.

**Tests** — this project forbids an agent writing tests without permission.
Items whose only sound proof is a regression test are marked `needs-test`; they
are blocked until that permission is given (see [Decisions](#decisions), `D3`).

**Scope discipline** — one item, one concern, one commit on
`feature/code-audit`. Do not opportunistically fold in an adjacent item: the
queue's order is deliberate.

## Queue

| id | severity | area | item | status |
|---|---|---|---|---|
| P0-1 | critical | lowlevel, registry | Byte-reader bounds: unit mismatch, overflow, alignment | done |
| P0-2a | critical | lowlevel | Private raw-pointer fields, checked constructors, the missing contracts | done |
| P0-2b | critical | lowlevel, all | The arena accessors are safe but unbounded; make them `unsafe` | done |
| P0-2c | critical | highlevel | `shape::array_items` re-exports the unbounded slice from a safe wrapper | done |
| P0-3 | critical | package | `git clone`/`checkout` argument injection | done |
| P0-4 | critical | package | Downloaded binaries have no integrity check | done |
| P0-5 | critical | language, registry | Artifact deserialization: unbounded recursion and allocation | done |
| P0-6 | high | preprocess | `Depend::sub` is an unvalidated path join | done |
| P0-7 | high | language | The artifact container has no body digest | done |
| P1-1 | high | lowlevel | `insert_module` inserts before it asserts | done |
| P1-2 | high | lowlevel | `.unwrap()` on the budget-refusal path; comment contradicts code | done |
| P1-3 | high | lowlevel | Table identity hash is a raw address | done |
| P1-4 | high | lowlevel | `hash_inner` cycle token vs `key_eq` coinduction | done |
| P1-5 | high | language | `content_key` tag collision across four AST forms | done |
| P1-6 | high | highlevel, lowlevel | Non-function apply to a deferred callee is silently accepted | done |
| P1-7 | low | highlevel | The function pass order relies on slotmap's undocumented iteration order | done |
| P1-8 | high | highlevel | Compile work budget is hard-coded and not plumbable | done |
| P1-9 | high | highlevel | `DiaryEntry::errors` doubles as a discriminant | done |
| P1-10 | high | highlevel | `Static` export `items[0]`/`items[1]` unchecked | done |
| P1-11 | high | registry | `store_artifact`: fixed temp name, outside the lock | done |
| P1-12 | high | registry | Unparseable registry discards all state; keys are recycled | done |
| P1-13 | high | package | Compiler-cache key omits `core_repo`; wrong crate's version | done |
| P1-14 | high | language | `run.rs` never checks `Build::ok` | done |
| P1-15 | high | language | `Err(vec![])` — an error carrying no diagnostic | done |
| P1-16 | high | language, language-server | `stage_depends` wired on one of two store entry points | done |
| P1-17 | high | language-server | Every request runs the whole frontend | todo |
| P1-18 | high | compute | Unbounded global registries; per-launch wasm rebuild; unbounded `plrun` | todo |
| P1-19 | medium | lowlevel | `evaluate_block` expects a return the budget may refuse | done |
| P1-20 | low | package | `download` uses a predictable shared temp name and skips `fsync` | done |
| P1-21 | medium | lowlevel, highlevel | A struct value applied through a deferred callee is still silent | done |
| P1-22 | high | language, language-server | The frontend's recursion overflows the caller's stack on a ~500-byte file | done |
| P1-23 | high | language-parser | The parser's 16 MiB worker overflows at 175 nesting levels | wontfix:D9 |
| P1-24 | high | language-parser | The AST's own recursive `Drop` overflows on a deep tree | wontfix:D9 |
| P1-25 | high | package | A dependency's package name is written as Rust source | done |
| P1-26 | high | language | The table-key hash changed meaning without an artifact version bump | done |
| P2-1 | medium | language, language-server | `BufferSession` is built but unwired; rustdoc claims otherwise | todo |
| P2-2 | medium | highlevel, language, language-server | Five hand-written AST traversals; one with a wildcard arm | todo |
| P2-3 | medium | highlevel | `Build` is a god-DTO with four parallel vectors | todo |
| P2-4 | medium | lowlevel | `Node`'s `pub` fields break the documented write choke-point | done |
| P2-5 | medium | highlevel | `NativeApply` is an unvalidated escape hatch | done |
| P2-6 | medium | language | Repo tooling (README generator, `sync-readme`) inside the compiler library | done |
| P2-7 | medium | lowlevel | `visiting` is set by hand, bypassing the `Drop` guard | done |
| P2-8 | medium | highlevel | `missing_slots[order_index()]` guarded only by `debug_assert!` | done |
| P2-9 | medium | highlevel | `no_attr_ext` panics on any annotated program | done |
| P2-10 | medium | highlevel | `check_term` recursion is unbounded; `stacksafe` is an unused dep | done |
| P2-11 | medium | all | God files with named seams | todo |
| P2-12 | medium | language, package, ci | `clap` is linked by every consumer of the compiler library | done |
| P2-13 | medium | lowlevel, utils | Node state is still writable through the node table and `disjoint::Meta` | todo |
| P3-1 | medium | all | Duplication clusters | done |
| P3-2 | medium | all | Workspace manifest duplication | done |
| P3-3 | medium | ci | No test/clippy/fmt gate in CI | done |
| P3-4 | medium | span, language, language-server | Four byte↔line/col implementations with divergent edge behaviour | todo |
| P4-1 | medium | lowlevel | Registry read lock + `Arc` clone per array element | todo |
| P4-2 | medium | lowlevel | `write_node_value` is O(class size); seven sibling full-list walks | todo |
| P4-3 | medium | language-parser | A 16 MiB thread and a rebuilt combinator graph per parse | todo |
| P4-4 | medium | highlevel, language | O(E×D) diagnostics; O(diags×lines) rendering | todo |
| P4-5 | low | lowlevel, compute | `path.contains` as a cycle guard; O(n²) kernel codegen | todo |
| P4-6 | low | lowlevel, language, compute | Per-apply clones, repeated `as_enum`, per-byte `mix`, intern leak | todo |
| P5-1 | low | language | `tests/scratch.rs` has no assertions | done |
| P5-2 | low | docs | `docs/README.md` status disagrees with the note it indexes | done |
| P5-3 | low | all | Stale or contradicted doc comments (list) | done |
| P5-4 | low | compute | `wasm-encoder` 0.258 vs wasmi's `wasmparser` 0.228 | done |
| P5-5 | low | language-lex | `~` overflow silently saturates to `usize::MAX` | done |
| P5-6 | low | package | `build.rs`'s `.git/HEAD` trigger never fires in a worktree | done |
| P5-7 | low | package | `lichen path language-server` pollutes stdout | done |
| P5-8 | low | package | Generated `Cargo.toml`: TOML injection and a Windows path escape | done |
| P5-9 | low | language | `io::Error` modelled as a `(0,0)` source diagnostic, 10 sites | done |
| P5-10 | low | render, language-server | Unguarded parent walk and unchecked index on the render hot path | done |
| P5-11 | low | registry | `virtual:` file IDs can never verify | done |
| P5-12 | medium | workspace | A worktree nested in the checkout breaks `cargo metadata`/`fmt` for `tree-sitter-lichen` | done |
| P5-13 | low | preprocess | `PreprocessDiag::at_zero` fabricates a source span | done |

## P0 — memory safety and supply chain

### P0-1 — Byte-reader bounds: unit mismatch, overflow, alignment `verified`

`crates/lichen-lowlevel/src/codec.rs:163-185` (array, tag 1) and `:206-228`
(table, tag 6) are the same code twice:

```rust
let offset = r.u64()? as usize;
let len    = r.u64()? as usize;                       // element count
let gap = owner_base as usize - owner_arena.as_ptr() as usize;
if offset + len > owner_arena.len() - gap { return Err(...); }
let payload = unsafe { owner_base.add(offset) as *const ArrayItem };
```

Three defects in four lines:

1. **Unit mismatch.** The writer emits `slice.len()` where `slice: &[ArrayItem]`
   (`:117`, `:128`), so `len` is an *element* count, while
   `owner_arena.len()` is a *byte* count (`persist.rs:406-408` reads bytes).
   `offset = L-8, len = 8` passes the check and then reads 56 bytes past the
   allocation.
2. **Overflow.** `offset + len` is an unchecked `usize` add; in release a
   crafted pair wraps and makes the check vacuous, after which
   `ptr::add(offset)` is already UB (out of the object's bounds).
3. **No alignment or stride validation.** `offset` need not be a multiple of
   `align_of::<ArrayItem>()`, and `len` need not be a whole number of items.

`gap` can also underflow `owner_arena.len() - gap` when the arena is shorter
than the alignment padding, wrapping the bound to `usize::MAX`.

`crates/lichen-registry/src/codec.rs:80-87` `Reader::take` has the same
unchecked add (there it degrades to a slice-range panic, not UB).

**Why it matters.** `deserialize_artifact` validates magic, version, key, the
*supplied* hash, and the alignment — but the hash is a header field compared
against a value the caller computes locally, and **the body has no integrity
check at all**. Copying a valid artifact's 48-byte prefix and replacing the body
passes every check. Anyone able to write `~/.lichen/artifacts/` reaches this.

**Fix.** `checked_add` / `checked_mul(len, size_of::<T>())` for both leaf
types; validate `offset % align == 0`; `saturating_sub` for `gap`; share one
generic helper so the array and table arms cannot drift. Consider a body
checksum in the container as the durable fix (see `D1`).

### P0-2 — `&'static` laundering and `pub` raw-pointer fields `verified`

Split into `P0-2a` (encapsulation, contracts, missing `# Safety`) and `P0-2b`
(the accessors become `unsafe`, and every call site acknowledges it) by decision
`D7`. The finding:

`crates/lichen-lowlevel/src/lib.rs:253-275`:

```rust
pub fn items(&self) -> &'static [ArrayItem] {
    match self {
        AnyHandle::Dynamic(handle) => unsafe { &*handle.0 },
```

The signature lifts a borrow of `&self` to `'static`. `drop_block` is `pub`
(`gc.rs:187`) and frees the `Bump` (`gc.rs:232`), so this is UB from safe code:

```rust
let items: &'static [ArrayItem] = module.array_items(node).unwrap();
module.drop_block(block);
println!("{:?}", items[0]);
```

The same laundering is in `array_items` (`utils.rs:18`), `Handle::len`
(`lib.rs:458-466`), `StaticHandle::len` (`:468-476`), and `ValueExt::value_eq`
(`:336-350`). `utils.rs:12-17` puts a `# Safety` header on a **safe** function
whose prose argues about "the lifetime of `&self`" while the signature says
`'static`.

`lib.rs:395-411`: `Handle<T>(pub *const T)` and
`StaticHandle { pub offset: *const T }` are public raw-pointer fields, and
`lib.rs:403`'s comment — *"Not really pointing to anything, just offset encoded
with possible slice length"* — is **false**: the codec resolves `offset` to a
real arena address at load (`codec.rs:180`, `:223`) and it is dereferenced at
`lib.rs:271`, `lib.rs:470`, `codec.rs:115`, `codec.rs:126`,
`static_module.rs:665`, `:684`. It describes the *serialized* integer form and
was left on the in-memory field.

Also `copy_ext` (`utils.rs:49-63`) copies `old.len()` **bytes** into a slot
aligned to `P::Value::alignment()`, and `ValueExt` (`lib.rs:311-351`) has no
trait-level doc at all — no statement that `handle()` must stay stable while the
value lives, that `alignment()` must be a power of two, or that `len()` is a byte
count. `Layout::from_size_align(..).unwrap()` (`utils.rs:55`) panics on a
non-power-of-two `alignment()`. 21 of the crate's 26 `unsafe` sites have no
`SAFETY` justification at all — including the two most dangerous (`items()`,
`value_eq`) — while the crate's prose is otherwise unusually careful.

**P0-2a** — privatise the pointer fields behind checked constructors; correct the
`lib.rs:403` comment; give `ValueExt` its three obligations; add the missing
`# Safety`/`SAFETY` notes. No signature changes, so no call-site churn.

**P0-2b** — mark `items()`, `array_items` and the `[u8]` handle accessors
`unsafe` with one written contract, then update every call site (the compiler
enumerates them). Internal sites that uphold the invariant get a one-line
`SAFETY`; external sites must acknowledge it explicitly.

**Outcome.** `AnyHandle<[ArrayItem]>::items`, `AnyHandle<[TableItem]>::items`
and `Module::array_items` are `unsafe`: the contract is written once on the
first and cited by the other two, and all 149 call sites across eight crates
carry an `unsafe` block plus a one-line `SAFETY` naming that site's own reason.
The `[u8]` length accessors stayed **safe** — the toolchain has the stable
pointer-metadata read (`<*const [T]>::len()`), so
`Handle`/`StaticHandle`/`AnyHandle<[u8]>::len` and `is_empty` get the length
without forming a reference, and two `unsafe` blocks went away instead.
`as_ptr` is deliberately safe as well: producing a raw pointer is not a
dereference, and building a handle is already `unsafe` (`P0-2a`).

**P0-2c — the re-export that reopens the hole.** `lichen-highlevel`'s
`shape::array_items` (`crates/lichen-highlevel/src/shape.rs:181`) is declared
`pub fn ... -> Option<&'static [ArrayItem]>` — **safe** — and forwards the same
slice `Module::array_items` now guards with `unsafe`. Every other accessor in
`shape` is built on it, so after `P0-2b` an out-of-crate caller can still obtain
the unbounded slice without an `unsafe` block, through this one wrapper: `D7`'s
"no out-of-crate caller can obtain the slice from safe code at all" holds for
`lichen-lowlevel` but not for this re-export. `P0-2b` left it alone rather than
silently widening its own scope, which was right.

It is cheap to close: a workspace grep shows all 17 call sites are **inside
`lichen-highlevel`** (15 in `shape` itself, `checker/annotations.rs:112` and
`checker/structs.rs`), so this is a single-crate change — mark it `unsafe` with
the contract it already documents, and give each of its own call sites the
one-line `SAFETY` the rest of `P0-2b` now carries.

### P0-3 — `git clone`/`checkout` argument injection `verified`

`crates/lichen-package/src/git.rs:86-93`:

```rust
git(&["clone", &dep.url, &dir_git], &root_git)?;      // no `--`, no allowlist
if let Some(rev) = rev { git_in(&dir_git, &["checkout", rev])?; }
```

`dep.url` is the right-hand side of a `depend "…"` directive and
`crates/lichen-preprocess/src/parse.rs:24-46` stores it as a bare `String` with
**no validation anywhere**. A source file containing

```lichen
@{ x = depend "--upload-pack=<command>" @}
x
```

makes the process run `git clone --upload-pack=<command> <dir>`, which executes
`<command> <dir>`. Reachable from `lichen fetch` / `run` / `build`
(`main.rs:189, 248, 439, 498`) — cloning an untrusted repo and running one of
its files is enough; no network fetch is needed. The preprocessor's string
regex `"[^"@]*"` (`lichen-preprocess/src/lex.rs:44`) forbids `"` and `@` but
not `-`, `=` or space, so the payload lexes cleanly.

`rev`/`branch`/`tag` (`git.rs:38-43`) reach `git checkout <rev>` the same way:
`rev = "-f"` silently checks out `HEAD` instead of the pin — a supply-chain
downgrade that no test would notice.

**Fix.** `git clone -- <url> <dir>` and reject a value whose first byte is `-`
for all four fields. Scheme policy is `D2`.

### P0-4 — Downloaded binaries have no integrity check `verified`

`crates/lichen-package/src/toolchain.rs:262-284` downloads with
`curl -L --fail`, renames into place, `chmod 0755`s, and hands the path to a
caller that spawns it (`main.rs:302`, and an editor runs it as the LSP).
`update()` (`toolchain.rs:369-378`) does the same for the package manager
itself. There is no hash, no checksum file, no signature, and no pinning
anywhere in the crate.

`toolchain.rs:13-16` reads as a provenance guarantee — *"the toolchain and the
package manager are always the same revision"* — but the **commit is pinned and
the bytes are not**. `-L` follows redirects to any host; the temp name
(`:267`) is predictable and shared; there is no `fsync` before the rename, so a
crash can leave a truncated binary in place. `toolchain.rs:326-333` also falls
back to any `lichen-compiler` on `$PATH` without warning.

**Decision `D4` — accepted risk, fix the docs.** The trust root is HTTPS to
GitHub plus the release the maintainer published; no checksum, signature or host
restriction is added. The change is to **delete the provenance claim the docs
make**: the module doc reads as a guarantee (*"the toolchain and the package
manager are always the same revision"*) while the commit is pinned and the bytes
are not — the pin says which revision was *asked for*, never what arrived. State
the real trust model in its place. The mechanical hardening this does not cover
is `P1-20`.

**Outcome.** Documented and accepted, not hardened. `D4` accepted the trust root,
so no checksum, signature or download-host restriction was added anywhere; what
changed is the documentation. `toolchain.rs`'s module doc now states the trust
model in two parts: the release tag is derived from the binary's embedded build
commit, so the download *addresses* the release that claims to be that revision,
and **nothing verifies that the asset delivered is the one that commit
produced** — the commit is pinned, the contents are trusted as delivered over
HTTPS. Every sibling that repeated the guarantee (`self_commit`,
`toolchain_commit` and `install`'s doc, `main.rs`'s `cmd_install`, `build.rs`,
`crates/lichen-package/README.md`, `docs/notes/language-toolchain.md`, the
`release-lichen` workflow, the Zed extension's docs) was corrected to the address
it can actually make; the docs that stated only the tag convention were left
alone. **The residual risk stands:** the download is unverified content from a
`curl` fetch, and the mechanical hardening of that path remains open as `P1-20`
(a predictable shared temp name, no `fsync` before the rename, and the silent
`$PATH` fallback). A later reader should not expect a check here.

### P0-5 — Artifact deserialization: unbounded recursion and allocation `reported`

`crates/lichen-language/src/persist.rs`:

- `:410-411`, `:470`, `:479`, `:486`, `:336`, `:344` —
  `Vec::with_capacity(r.u64()? as usize)` straight from the file. A 64-byte
  artifact can request a `2^60`-element allocation and abort the process.
- `:331-357` `read_low_shape` recurses once per nesting level; each level costs
  one byte, so ~1 MB of crafted bytes is ~10^6 frames of native stack.
- `:403-405`, `:431-451`, `:472-490` — every `LocalNodeId` (`export`,
  `parent`/`next`/`tail`, `parameter`/`r#return`/`asserts`/`nodes`) is taken
  from the file unchecked. A structurally valid but semantically corrupt
  artifact loads "successfully" and panics much later, far from the cause —
  which contradicts the self-heal claim in
  `crates/lichen-language-server/src/home.rs:14-17`.

**Fix.** `checked_add` in `Reader::take`; a depth cap in `read_low_shape`;
`min(count, remaining_bytes)` before every `with_capacity`; and validate every
`LocalNodeId` against the declared node count at load.

**Residual, deliberately left.** `LowShape::Array(elem, len)` still carries a
length read from the stream and never validated. It is inert: the only consumer
of a domain shape is `lichen-compute`'s `compile_fragment`, which rejects any
shape that is not `USize` or `Tuple` (`compute.rs:674-679`), and
`flatten_offset` rejects the rest — so the field never sizes an allocation. No
follow-up is owed; noted so the next pass does not re-open it.

### P0-6 — `Depend::sub` is an unvalidated path join `verified`

`crates/lichen-preprocess/src/lib.rs:162-166`:

```rust
pub fn vendored_dir(&self) -> PathBuf {
    match &self.sub { Some(sub) => self.sources_dir().join(sub), None => self.sources_dir() }
}
```

`sub` is a free-form string from the source file. `sanitize_alias` (`:171-183`)
is applied to the *alias* only, and only allows `.` among punctuation — so even
the alias could pass `..`, though `parse.rs:66-74` requires an identifier
(`[A-Za-z_][A-Za-z0-9_]*` per `lex.rs:50`), so that path is unreachable. `sub`
has no such gate: `sub = "../../.."` escapes the cache, and
`PathBuf::join` with an absolute path or a drive letter *replaces* the base
entirely. `sub` is a documented, used feature (`lichen-std/README.md`).

**Fix.** Reject `sub` unless it is a relative path with no `..` component, no
root, and no prefix — checked in one place next to `sanitize_alias`, with a
preprocess diagnostic.

**Residual, deliberately left.** The check is lexical, so a `sub` naming a
**symlink** inside the clone still resolves wherever that symlink points. It is
not worth a follow-up on its own: the same repository already supplies the
clone's contents, including any `build.rs` the plugin build runs, so a symlink
grants nothing an attacker did not already have. Resolving it would mean
canonicalising and re-checking containment against `sources_root()`, which
needs the directory to exist and so cannot live in this accessor.

### P0-7 — The artifact container has no body digest `verified`

New item from decision `D1`. The reader
(`crates/lichen-language/src/persist.rs:433`, checks at `:445-455`) validates the
magic (`:445`), the format version (`:449`), the module key and the payload
alignment, then compares a 32-byte `hash` (`:454`) — but that `hash` is the
artifact's *identity*, checked against a value the **caller** computes locally
(the device registry's `verify`). It says nothing about the bytes that follow it
in the file, and the body has no digest of any kind. The layout comment at `:56`
shows the header: magic, version, key, hash, alignment, then the body.

So a file whose prefix is copied from a valid artifact and whose body is
replaced or corrupted passes every check the container makes, and `P0-1`/`P0-5`
are the only things between that and undefined behaviour. They hold now, but the
container should not depend on every future field parser being individually
careful.

**Fix.** Add a digest over the body to the header (`:200-213` is the writer) and
verify it before interpreting any of the body. Bump the format version, since the
header layout changes; an older artifact then fails the version check and
recompiles, which is the intended answer. `sha2` is already a dependency of both
crates involved, so no new dependency is needed.

**What this does and does not buy — say so, do not overclaim.** A digest detects
corruption: bit rot, a truncated write, a bad copy. It does **not** provide
authenticity — whoever can write the artifact file can recompute the digest. The
bound on a deliberate attacker is memory safety plus total field validation
(`P0-1`, `P0-5`, `P0-2`), not the digest. Put that distinction in the format's
doc comment so a later reader does not mistake it for a signature.

## P1 — correctness

### P1-1 — `insert_module` inserts before it asserts `verified`

`crates/lichen-lowlevel/src/lib.rs:912-926` performs
`self.entries.insert(key, …)` *inside* the `assert!`, so the resident `Package`
is dropped and replaced before the duplicate-key assert fires. The sibling
`freeze_mapped` (`:888-891`) checks `contains_key` first — the correct order. If
the panic is caught anywhere up the stack, the registry silently holds the new
artifact under the old key: exactly the shadowing the message claims to prevent.

**Fix.** Hoist the `contains_key` check above the insert.

### P1-2 — `.unwrap()` on the budget-refusal path `verified`

`crates/lichen-lowlevel/src/evaluation.rs`, the extension-operator arm:

```rust
let value = self.evaluate_node_deep(operand, Some(block));
if self.nodes[operand].evaluated_deep.unwrap().parameterized {
```

`evaluate_node_deep_inner` returns **before** writing `evaluated_deep` when it
refuses on budget exhaustion, and the structural-cycle cut returns early too. So
an extension operator over a too-deep operand turned
`BudgetExhausted::EvaluateDepth` — whose whole contract is that the guards
*"refuse to continue instead of unwinding"* — into a panic. The same field is
already read defensively further down the same function via
`self.nodes.get(operand)`, with a comment noting *"a nested block release may
have dropped the node by now"*, so the node may be **absent** as well as the flag
unset.

**Outcome.** The arm now reads
`self.nodes.get(operand).is_none_or(|node| node.evaluated_deep.is_none_or(|deep| deep.parameterized))`
— an absent node or an unset flag is "concreteness unknown", never "proven
concrete". (`is_none_or`, not `map_or(true, ..)`: the latter draws
`unnecessary_map_or` on this toolchain.) The only paths whose behaviour changed
are the ones that used to panic. A regression test lowers
`evaluate_depth_limit` and drives an extension operator past it
(`tests/basic/evaluation.rs`); it was confirmed to fail against the old
`.unwrap()` before being committed.

**The counter comment, and the question it opened.** The comment claimed the
nesting counter *"deliberately stays inflated"* while the code decremented it —
copied from `apply.rs`, where that sentence is **correct** (that path genuinely
skips its decrement). The comment now describes the decrement and argues for the
scoped reading: `deep_depth` is incremented at entry and restored on every exit,
so leaving it inflated would make every later `evaluate_node_deep` in the process
start past the limit.

That argument is not obviously the whole story, so the semantics are **not**
declared settled here — see `D8`. The panic fix above holds either way.

### P1-3 — Table identity hash is a raw address `verified`

`crates/lichen-lowlevel/src/table.rs:232-237` hashes a table/function key by
its **process address**:

```rust
AnyHandle::Dynamic(handle) => handle.0 as *const TableItem as usize as u64,
AnyHandle::Static(handle) => handle.module.as_raw() ^ (handle.offset as *const TableItem as usize as u64),
```

The stored hash travels verbatim through a freeze
(`static_module.rs:805-807`), so after a reload the key recomputes a different
number and `partition_point` (`evaluation.rs:359`) never lands on the entry — a
permanent `TableMiss`. `id_hash(FunctionId)` vs `id_hash(StaticFunctionRef)`
(`table.rs:217`/`:219`) is the same class of bug with two different `Hash`
impls. This falsifies `table.rs:8-11` (*"content-addressed artifacts stay
deterministic"*) and `table.rs:20-21` (*"stable for the table's whole life"*).
No test freezes a table and reads it back.

**Outcome — one change with `P1-4`, per `D5`.**  Step 0 first, because it decides
what "correct" means and therefore how big the item is: **the stored hash is a
pre-filter and `key_eq` is the authority.**  The read takes
`partition_point(|item| item.hash < hash)` to the start of the equal-hash run,
walks that run, and verifies every candidate with `self.key_eq(key, item.key, ..)`
(`evaluation.rs:428-439`) — so the hash owes one direction only, *equal keys hash
equal*, and a collision is always sound.  A stable-but-not-injective content hash
is therefore sufficient and the item stays tractable; had the hash alone decided
the hit, the item would have needed an injective encoding of a cyclic graph and
would have been larger than its scope.

What landed is a canonical **depth-bounded content unfolding** (`table.rs`):
`hash_node` walks the key's structure — array items positionally, a function
template, a table's entry-key hashes — and every position at `UNFOLD_DEPTH` (= 8)
mixes the same `FRONTIER_TOKEN`.  Nothing process-local enters it: no address, no
node identity, no freeze-assigned index.  It is memoized on
`(node, mode, remaining depth)`, which is everything the answer depends on, so
caching cannot change what a key hashes to.

The two identity hashes are gone.  **A table** now hashes as the fold of its
payload entries' *stored key hashes* — the numbers the payload is already sorted
by, so the fold is a function of the key *set* — and a freeze copies the payload
verbatim, which is exactly why the number is the same before and after a reload.
**A function** now hashes as the shape of its template: its return and its asserts,
unfolded in a second, *total* mode (`UnfoldMode::Template`), because a template is
expected to hold unbound cells (its own parameter is one) and `key_eq` never
compares it.  That mode reads a node's operation edge in preference to its
memoized value, so the hash does not drift when the definition pass runs the body
between the build and the freeze — a hazard the regression test pins deliberately.

**Tests** (`crates/lichen-lowlevel/tests/basic/table.rs` — the lowlevel half of the
freeze/reload path; the artifact container's byte round-trip lives in
`lichen-language`, and no test froze a table before this one):
`a_table_key_survives_a_freeze_and_a_reload` and
`a_function_key_survives_a_freeze_and_a_reload`.  Both fail on the unfixed tree with
*"[TableMiss { table: Dynamic(NodeId(1v1)), key: Dynamic(NodeId(2v1)) }]"*.

**A third defect found in the same function, and fixed with it.**
`hash_value`'s `None => unreachable!("a structural value is one of the variants
above")` arm was reachable: a key holding one of the program's own value variants
is ordinary source (a type constant as a table key — `t = table { Int ==> 1 };
t{Int}` panicked the compiler with exactly that message), and a template walk makes
the arm unavoidable, since a function body holds them.  It is now one opaque
`EXTENSION_TOKEN` per position, which is sound for the same reason a collision is
(`key_eq` still tells two of them apart — by payload bytes where the value carries
a handle, by structural equality otherwise), and
`a_key_of_the_program_s_own_value_vocabulary_is_hashed_not_refused` pins both the
match and the miss.

**Residual, and why it is sound.**  The unfolding cannot distinguish two keys that
differ only below `UNFOLD_DEPTH`, two function templates that differ only in which
operator sits at a position (the lowlevel cannot name a program's operator
vocabulary), or two of the program's own value variants.  Each is a collision, and
`key_eq` decides every candidate the hash offers.

**One consequence left with the artifact container's owner.**  The *meaning* of the
hash stored in a table payload changed, and `ARTIFACT_FORMAT_VERSION`
(`crates/lichen-language/src/persist.rs:105`) was not bumped, so a cache written by
an older compiler is still accepted and its payload hashes are stale — a spurious
miss rather than a wrong answer, but a miss.  That constant lives in `P0-7`'s file
and bumping it was outside this item; it is recorded here rather than changed
silently.

### P1-4 — `hash_inner` cycle token vs `key_eq` coinduction `reported`

`table.rs:178-180` cuts a cycle on **node identity plus depth**; `table.rs:248-259`
cuts on the **unordered pair**. `A = [1, A]` and `B = [1, [1, B]]` are equal
under `key_eq` but hash differently; a self-referential universe crossing the
static/dynamic boundary terminates at different depths on each side. Since the
hash only *finds candidates*, a hash disagreement is an unconditional miss.
`tests/basic/table.rs:258` covers only the symmetric case.

**Outcome — the same change as `P1-3`, per `D5`; see its Outcome for the shared
mechanism and the regression tests.**  Step 0 (`key_eq` is the authority, the hash
is a pre-filter) is what makes this half small: the comparison decides a cycle by
the *unordered pair* of nodes on its path, which is equality of the infinite
unfolding, so a hash only has to be *invariant* under that equality rather than
reproduce the comparison's traversal.

**The cycle token, before and after.**  Before, a revisited node returned
`mix(CYCLE_TOKEN ^ depth)` with `depth` its own revisit depth, so the *same* cycle
closed at different depths produced different tokens — `[1, ↺]` at depth 1,
`[1, [1, ↺]]` at depth 2 — and two `key_eq`-equal keys hashed unequal.  The token
is gone, and so is the path check that produced it: the unfolding is cut by
`UNFOLD_DEPTH` instead, and **every** position at that frontier mixes the same
`FRONTIER_TOKEN`.  That is the alignment: `key_eq`'s coinduction is bisimulation,
bisimilar keys have equal unfoldings at *every* depth, so truncating both at one
depth agrees with the comparison by construction — including across the
static/dynamic boundary, where the same content is walked through a static ref at
the same depth.  The bound is now explicitly a *performance* bound, not a
correctness one: a key it truncates can only collide, never be hidden.

**Tests** (`crates/lichen-lowlevel/tests/basic/table.rs`):
`coinductively_equal_cyclic_keys_hash_equal_across_depth` is the `A = [1, A]` /
`B = [1, [1, B]]` pair (fails on the unfixed tree with *"[TableMiss { table:
Dynamic(NodeId(7v1)), key: Dynamic(NodeId(3v1)) }]"*), and
`a_cyclic_key_is_found_across_the_static_boundary` closes the cycle one level
deeper *through a static ref to the frozen module* (fails with *"[TableMiss {
table: Dynamic(NodeId(1v1)), key: Dynamic(NodeId(5v1)) }]"*).  The pre-existing
`cyclic_keys_hash_and_compare_equal`, which covered only the symmetric case, still
passes.

**Decision `D5` — canonical content unfolding.** A hashed key must be a function
of the key's content, so a table or function used as a table key hashes the same
after a freeze and a reload as before it. This and `P1-4` are **one change**: a
self-referential key needs a canonical unfolding so two coinductively equal keys
unfold identically, and `hash_inner`'s cycle token must then agree with
`key_eq`'s coinduction instead of counting depth. Expect the hardest item in the
queue — the unfolding has to be well-defined on a cyclic graph.

### P1-5 — `content_key` tag collision across four AST forms `verified`

`crates/lichen-language/src/resolve.rs`, the expression walk before the fix:

```rust
Expr::Tuple(elems, _) | Expr::TypeTuple(elems, _) | Expr::Array(elems, _) => { self.u(18); … }
Expr::StructType(fields, _) => { self.u(18); … }
```

`Tuple`, `TypeTuple` and `Array` wrote byte-identical keys, while
`compile.rs` lowers all four differently (`alloc_tuple` / `alloc_type_tuple` /
`alloc_array` / `alloc_type_struct`). `session.rs:250` reuses the cached
`Build` whenever `cache.key == key`, so changing `[a, b]` to `(a, b)` — a pure
bracket edit — reused the array's build and reported the array's types and
diagnostics for a tuple. The doc on `content_key` claimed *"Two programs with
equal keys have literally identical lowering-visible content"*; that assertion
was false.

**Impact.** `BufferSession` has no production consumer today (see `P2-1`), so
this is a real bug in a public API, currently unreachable from the CLI or LSP.
It becomes user-visible the moment `P1-17` is done.

**Outcome.** `Tuple`, `TypeTuple` and `Array` now write tags 29, 30 and 31
(`StructType` keeps 18), and `NamedFieldRead` moved from 24 to 32: it shared
that tag with `Str`, and a string's raw bytes share the element space, so
`Str("\u{2}ab")` and `_.ab` encoded alike. The encoding was **not**
self-delimiting: `Apply` concatenates two expression encodings with no
separator, so with count-less lists `(1,) (2, 3)` and `(1, (2,)) 3` flattened
to the same element sequence (both `[0, 2, 8, 18, 0, 1, 18, 0, 2, 0, 3]` under
the old format). Every list — the program's statements, `NativeCall` args,
`StructInst` fields, `Tuple`/`TypeTuple`/`Array` elements, `Table` entries and
`RecordBlock` fields — now writes its length. `KEY_FORMAT_VERSION` (`= 1`) is
written as the key's first element; adding an `Expr` variant or changing any tag
bumps it, which invalidates every cached key so every session rebuilds. The
`content_key` doc now states that version rule and argues the injectivity
rather than asserting it. Regression tests in
`crates/lichen-language/src/tests/session_tests.rs` drive the bracket swap
through the session edit API
(`a_bracket_swap_between_forms_rebuilds_instead_of_reusing`) and pin both
collisions (`the_content_key_distinguishes_list_arities_around_an_apply`,
`the_content_key_distinguishes_a_string_from_a_named_field_read`); the bracket
swap and arity tests were confirmed to fail against the old encoding before the
fix was committed.

### P1-6 — Non-function apply to a deferred callee is silently accepted `verified`

`crates/lichen-highlevel/src/checker/lambda.rs:236-254` gates the
function-ness guard on `concrete`:

```rust
let concrete = …matches!(value.as_enum(), None | Some(USize(_)) | Some(Array(_)));
if concrete && !shape::is_function_type(…) { /* record Guard */ }
```

When the callee is a parameter or a call result its type is an unbound cell,
`concrete` is false, and the guard is **skipped entirely**. Every sibling rule
*pins* instead (`indexing.rs:47-52`, `indexing.rs:149-154`, `structs.rs:424-429`,
with a test at `tests/checker.rs:1900-1912`). `check_app` is the only weakened
site.

The failure is then silent: `crates/lichen-lowlevel/src/evaluation.rs:310-319`
returns `LowValue::Parameterized` for a non-`Function` target and records
nothing, and `apply.rs:142-163`'s `_ => result` arm records nothing either. So
`f = g => g 1` applied to `f 5` yields `Build::ok == true` with zero
diagnostics.

**Both docs are wrong in the same direction**, which is what makes this look
intentional. `checker.rs:576-580` says a non-function apply is one *"the
runtime panics on"* — it does not. `evaluation.rs:316-318` says a genuinely
non-callable target *"is caught by the checker's unification before the deep
pass runs"* — it is not, in the non-concrete case.

**Fix direction: `D3`.** Either the guard pins to a fresh pair per apply, or the
runtime's non-function arm records an `EvalError`. Do not fix one side only.

**Outcome.** Fixed in the runtime arm, with `check_app` untouched. The new fact
is `EvalError::ApplyTarget { function }` — it carries the callee operand node,
so the highlevel attributes the diagnostic to the expression that was applied
(`diagnostic.rs`'s `RuntimeApplyTarget`, rendered as *"this value is not a
function — it cannot be applied"*). The `Apply` arm's catch-all is split: a
scalar, a string, a table, or the unit value records `ApplyTarget` and yields
`Void`; `Void` propagates silently (it is the residue of an already-recorded
failure); everything else — the program's own value *and* a structural array —
stayed lazy (`Parameterized`) *at the time of this item*. `P1-21` later made that
laziness conditional on the program's own dispatch; see its Outcome.
`Build::ok` already required
`module.eval_errors.is_empty()`, so recording the fact is what rejects the build.
The diagnostic carries no caret when the callee's value node has no source edge,
exactly as its `RuntimeIndexTarget` sibling. The sibling `checker.rs:576-580`
comment above is now false a second time — the runtime does not panic on a
non-function apply — but what the definition pass's skip is really for was not
established here, and contradicted doc comments belong to `P5-3`, so that
comment was left as it stands.

**The arm split must keep an array lazy.** A compute `Kernel` is a struct
`[native, sig]`, and a struct value is structurally an array — so classifying
`LowValue::Array` with the non-callables turns a legitimate kernel apply into a
runtime error. `tests/compute.rs`'s `jit_cross_kernel_call` and
`jit_cross_kernel_subexpr` fail exactly there. The arm's laziness exists for that
path, so only the leaf values the lowlevel can prove uncallable are refused.
**Residual, deliberately left at the time — closed by `P1-21`.** applying a
*non-kernel* struct value through a deferred callee (`f = g => g 1` applied to a
struct instance) was still silently accepted, because the lowlevel cannot tell
that array from a kernel's, and the kernel
path is the one that must keep working. Closing it needs the checker to see the
deferred callee's shape, which is `D3`'s other half.

**`check_app`'s `concrete` gate was rejected, and must not be reopened.**
Removing it makes the guard unify the callee's type cell with a fresh arrow. For
a lambda *parameter* that cell is shared by every instantiation of the function,
so the unify resolves it to one concrete arrow and let-polymorphism dies for the
natural pattern `apply = f => x => f x` used at two types —
`examples/let_polymorphism.lichen` is a documented, advertised feature. The gate
is what keeps a callee with an unbound type out of that unify; the deferred case
is now caught in the runtime arm instead.

**Tests.** `crates/lichen-lowlevel/tests/basic/evaluation.rs`'s
`applying_a_non_function_records_an_eval_error` pins the lowlevel fact (recorded
exactly once, blamed on the callee node, `Void` yielded), and
`crates/lichen-language/tests/pipeline.rs`'s
`an_apply_of_a_deferred_non_function_reports_a_runtime_apply_target_error` pins
the user-visible diagnostic for the reproduction. The first was confirmed to fail
against the unfixed arm; `compute`/`examples` pass before and after, and the
`Array` regression above was found by that gate and fixed in the arm, not the
test.

### P1-7 — The function pass order relies on an undocumented iteration order `verified`

**The audit's original claim was wrong and is corrected here.** It said the pass
order was *non-deterministic* and could differ "between two builds of the same
binary if slotmap's hash seed moves". There is no hash seed, and the order is
deterministic: `SlotMap::iter` is implemented as
`self.slots.iter().enumerate()` (skipping the sentinel and vacant slots), and
`SlotMap::keys` is `Keys { inner: self.iter() }` — so the order is **ascending
slot index**, which for a map that only ever inserts is insertion order. Checked
in the vendored source, `slotmap-1.1.1/src/basic.rs:847-855` and `:913-915`. The
same input therefore produces the same diagnostic sequence across runs.

What survives, at a much lower severity, is that the code depends on behaviour
the crate **documents** as *"an arbitrary order"* (`basic.rs:857`, `:917`): the
user-visible ordering of *orphan* unify errors and all runtime `eval_errors` —
the ones `diagnostic.rs` emits in record order with no re-sort, unlike the
diary-attributed ones it re-sorts by `seq` — is an accident of `slotmap`'s
current internals. A switch to `HopSlotMap` (whose order genuinely is arbitrary),
or a change in how the checker inserts functions, would silently reorder
compiler output.

`crates/lichen-highlevel/src/checker.rs` collects the pass order with
`checker.module.functions.keys().collect()`.

**Fix (hardening, not a bug fix).** Sort by a stable key before the pass — the
numeric slot index or the `ExprId` in `function_of` — so that the ordering is a
stated property rather than an inherited one. Cheap; do it when the surrounding
area is next touched. Do **not** cite this item as a reproducibility bug.

**Outcome.** The definition pass's order is now the checker's own statement,
keyed on `ExprId`: before the pass it inverts [`Checker::function_of`] into a
`FunctionId → ExprId` map and sorts the collected `Module::functions` keys by
that expression id (`checker.rs:637-656`). The **`ExprId`** was chosen over the
slot index deliberately: `SlotMap::keys`'s order is what the note above calls an
accident, and the only slotmap-supplied key form (`KeyData::as_ffi`) is
documented as opaque — *"no guarantees about its value are made"* — so sorting
by it would have re-inherited the same dependency instead of removing it. The
expression id is the frontend's own dense, pre-order index, so the pass now
walks functions in source order and survives a switch to `HopSlotMap` unchanged.

*This is not a no-op, and the measured difference is recorded here.* A probe
comparing the collected key order with the sorted one, run across the highlevel
suite and `lichen-language`'s `pipeline`/`registry`/`examples`, found programs
where the two differ: `tests/checker.rs`'s
`a_sibling_lambda_hangs_under_nothing` collects its two functions in the
expression order `[2, 1]`, and the pass now walks `[1, 2]`. So the recorded
sequence of *orphan* unify errors and runtime `eval_errors` — the diagnostics
`diagnostic.rs` emits in record order — is source order from here on, where it
was the checker's `check_lam` call order before. No existing test asserted that
sequence: every suite above passes with the sort in place. Nothing else about
the pass changed, and a function with no expression in the map (the probe found
none — every collected key is in `function_of`) still gets a total order, after
every function that has one. The probe itself was removed after measuring.

### P1-8 — Compile work budget is hard-coded `reported`

`crates/lichen-highlevel/src/checker.rs:490, 502` set
`apply_depth_limit = 500` and `apply_total_limit = 2_000` unconditionally on a
fresh `Module`, and no `build*` entry point accepts a caller-supplied limit. A
*terminating* program that applies more than 2000 times is reported as
`DiagKind::NonTerminating` — indistinguishable to the user from an infinite
loop. The comment at `:484-501` shows the values were tuned against the examples.

**Fix.** Plumb a limits struct through `build_with`, defaulting to today's values.

**Outcome.** The premise held, re-read first-hand before the fix: `build_with`
assigned both limits to the fresh `Module` with no parameter reaching it from
any of the four public constructors, and a *terminating* recursion past 2_000
applications came back as `DiagKind::NonTerminating` — measured pre-fix through
`Checker::build` on a hand-built `count n = [count (n - 1), 0][n == 0]` at
`n = 2500`: `budget_exhausted == Some(ApplyTotal { limit: 2000 })`,
`nonterminating == 1`, one diagnostic indistinguishable from an infinite loop.

The limits are now the caller's. `WorkBudget` (`checker.rs`) carries
`apply_depth_limit` / `apply_total_limit`, and its `Default` is the tuned pair
**unchanged** — the tuning rationale that sat inline at the assignment sites is
now its `Default` doc, stated once. The new public entry point is
`Checker::build_with_budget(ir, work_budget)`, which `build` now delegates to
with `WorkBudget::default()`; `build_in`, `build_in_attr` and
`build_in_attr_native` thread `WorkBudget::default()` into the private
`build_with`, which gained the budget parameter and now only installs it there.
No existing public signature changed and no default number moved.

*The private `build_with` deliberately stayed private.* Making it public and
budget-taking — the ledger's own fix line — was the first shape tried and is
unusable: its `attr_ext` argument is a
`Box<dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>>` that only the checker can
build for a program whose set has no `AttrExt`, which is exactly the case
`Checker::build` exists for, so no host could call it. One convenience entry
point is the additive half that actually reaches a caller.

**Tests.** `crates/lichen-highlevel/tests/checker.rs` adds a hand-built
countdown recursion (a lazily-indexed `[recursive, base]` pair, so the
definition pass really runs it) and three tests:
`a_caller_supplied_budget_bounds_the_definition_pass` (a total of 2 refuses a
four-application program: the caller's limit is the one in force, the refusal
is one `NonTerminating` diagnostic),
`a_raised_budget_lets_a_terminating_program_check` (`count 2500`, 2_501
applications, checks under a raised total), and
`the_default_entry_point_still_uses_the_tuned_budget` /
`the_tuned_total_still_bounds_a_long_but_terminating_recursion` (the default
entry point checks the small program and still refuses the long one at
`ApplyTotal { limit: 2000 }`). The pre-fix observation is the raised test's own
assertion driven through the only entry point the unfixed tree had: it failed
with *"a terminating recursion below the raised total must check: budget=
Some(ApplyTotal { limit: 2000 }), nonterminating=1"*, and the caller-limit test
cannot be written against the unfixed tree at all because no entry point took a
budget.

### P1-9 — `DiaryEntry::errors` doubles as a discriminant `reported`

`crates/lichen-highlevel/src/checker.rs:709-711`:

```rust
fn check_failed(&self) -> bool {
    !self.module.unify_errors.is_empty() || self.diary.iter().any(|e| e.errors.is_empty())
}
```

The second clause works only because a guard records `Range::default()`
(`checker/diagnostics.rs:100`) and a unify always records a non-empty range.
One informational diary entry with an empty range flips this to `true`, which at
`:581` **skips the entire definition pass** — silently disabling every runtime
check in the program. The invariant is unstated and untyped. *(No such entry is
produced in the tree: the consequence is a latent hazard, not a live bug — see
the Outcome.)*

**Fix.** Make the discriminant explicit — `errors: Option<Range<usize>>`, or an
outcome enum.

**Outcome.** The mechanism held, re-read first-hand: `record_guard` wrote
`Range::default()` while `record_unify` wrote the non-empty range
`check_unify`/`check_unify_relaxed` had just produced, so an entry's *kind* was
the emptiness of its range.  Every `check_failed` call site was read too, and
each acts on a `true`: the function-definition pass is skipped (every body's
return and its asserts never deep-evaluate, so no apply-time check and no assert
fires), then the top-level statement pass, then `check_asserts`, and `Build::ok`
becomes false.

**Reachability is latent, and the note's consequence overstated it.** No
producer records an entry that is not a failure — a guard *is* a failure by
construction, and a unify entry exists only when its range is non-empty — so no
in-tree program could flip the clause; `Build::diary` is public, but it is only
read after the passes have run.  What the item names is a hazard on the one
field every future producer must fill in, and that is the thing fixed.

`DiaryEntry::errors` is now `Option<Range<usize>>`: `None` *is* the guard
outcome — nothing unified, so the entry is itself the whole diagnostic — and
`Some(range)` is a failed unification's owned errors, so "a guard produced no
unify errors" and "a check produced an empty range" are different values rather
than the same one.  `check_failed` reads `entry.errors.is_none()`, so the
definition-pass skip still happens for the reason it was written (a guard
failure) and for no other; the diagnostics layer matches on the option for its
guard-versus-owner split, and the two owner lookups
(`orphan_unify_errors`, `mismatch`) filter with
`is_some_and(|range| range.contains(&i))`.

No test was added: the change is a type-level disambiguation with no behaviour
to pin, and the classification it protects is already exercised on both sides by
the suite (a guard failure and a unification failure each fail their build).
The highlevel suite (91 + 5 + 5 + 1 + 5 tests) and `lichen-language`'s
`pipeline` (121 tests) pass unchanged.

### P1-10 — `Static` export operands are unchecked `reported`

`crates/lichen-highlevel/src/checker.rs:1266-1278` guards with
`array_items(pair).is_none()` (the "not an array" half) and then indexes
`items[0]` / `items[1]`. A package export that materializes to a 0- or 1-element
array panics the checker. The lowlevel does this correctly a few hundred lines
away — `crates/lichen-lowlevel/src/apply.rs:143` checks `items().len() >= 2`.

Reachability: **the audit's claim was wrong and is corrected in the Outcome.** A well-formed program usually roots to a 2-element pair, but a raw
read *of* a raw read (`[[1]]<0>`) exports the inner array — a one-element one —
so ordinary source reaches the panic; no crafted artifact is involved.

**Fix.** `if items.len() != 2 { record_guard(ImportExport); return pair; }`.

**Outcome.** The guard now covers the array's *width* as well as its kind: the
`Static` arm takes the export's items only when the array is exactly the
`[value, type]` pair (`checker.rs:1273-1274`, `items.len() == 2`), and every
other shape takes the same `DiagKind::ImportExport` path the non-array export
already took, leaving the same well-formed hole (a fresh pair cell) behind. The
arm's structure is unchanged; only the guard's condition grew. The lowlevel's
sibling check (`apply.rs:148`, `items().len() >= 2`) is the shape the checker
should have had.

*The premise held, and reachability is stronger than reported.* No artifact is
needed. A package whose whole source is `[[1]]<0>` exports the inner array — a
one-element array — and the importer `@{x = import "pkg.lichen"@}x` panicked at
the unfixed `checker.rs:1280` with **"index out of bounds: the len is 1 but the
index is 1"**. The neighbouring `[1, 2]<0>` case stays on the non-array side of
the guard: its export is an unevaluated op node, so `array_items` answers
`None` for it.

**Test.** `crates/lichen-language/tests/registry.rs`'s
`a_package_export_that_is_not_a_pair_reports_an_import_export_error` pins the
`[[1]]<0>` package, the `ImportExport` kind and the caret on the import
directive; it was confirmed to fail against the unfixed checker with the panic
above.

### P1-11 — `store_artifact`: fixed temp name, outside the lock `verified`

`crates/lichen-registry/src/device.rs:226-232`:

```rust
let tmp = self.dir.join("artifacts").join("tmp");   // shared by every process
if std::fs::write(&tmp, bytes).is_ok() { let _ = std::fs::rename(&tmp, path); }
```

Two concurrent compiles interleave on the same temp path: A writes, B
overwrites, A's rename installs **B's bytes** into A's slot. Both errors are
discarded. The call site is `crates/lichen-language/src/package.rs:561`, which
does **not** take `with_lock`, so `device.rs:74-75`'s claim that *"All
mutations go through the cross-process `mkdir` lock and are saved atomically"*
is false for the artifact path. (`save()` at `:137` does hold the lock.)

**Fix.** A unique temp name (pid + counter or a random suffix), written under
the same lock, with `fsync` before the rename, and the errors propagated. Do
this with `P5-11` — one shared `write_atomic` helper.

**Outcome.** The shared name is gone: `store_artifact` writes to
`artifacts/<file-id-hash>.tmp.<pid>.<counter>` — unique across processes and
within one — `fsync`s the file, then renames it into the slot, so a concurrent
compile cannot install another invocation's bytes and a crash cannot install a
truncated artifact. **The registry lock is deliberately not taken here**, which
is where this deviates from the fix line above: the artifact payload is much
larger than the index, and the lock's own stale detection assumes
millisecond-scale holders, so serialising a payload write behind it would hold
the index lock for the length of that write. The rename is what makes the slot
atomic instead, and the errors stay swallowed — the documented fault-tolerance
policy (a failed cache write degrades to a miss and a recompile, never a failed
build) is kept, but it is now honest about its scope: a failed write or `fsync`
removes the temp file and leaves the **previous** artifact in place, where
before a half-written shared temp could be renamed over it. The `DeviceRegistry`
doc now says which mutations the lock serialises (`alloc`/`publish`/`gc`/
`remove`, whose `save` may keep its fixed temp name because it only runs under
that lock) and which the unique name makes atomic (`store_artifact`).
**Residual:** the temp file is only removed best-effort, so a crash mid-write
can leave one stray `*.tmp.*` file per failed attempt in `artifacts/`; nothing
reads it (the slot path is `<hash>.module`), and nothing collects it yet. The
directory entry created by the rename is not `fsync`ed, so on a power loss the
rename itself can still be lost — the slot then holds the previous artifact,
which is a miss, not corruption.

### P1-12 — Unparseable registry discards all state `reported`

`crates/lichen-registry/src/device.rs:118-132`: an `Err` from `parse_registry`
leaves the in-memory state at its previous value, and the next `with_lock`
(`:146-153`) reloads (no-op) then `save()`s an **empty** registry — `next_key`
restarts at 0 while the orphan `.module` files stay on disk. Because
`ModuleKey` is a *recycled* index (`module_key.rs:5-13`) and artifact bytes embed
it as an absolute reference, a stale artifact can be paired with a different
module. The doc at `:115-117` calls this *"the next save repairs it"* — it
discards, it does not repair.

Related: `gc()` (`:262-278`) frees keys that surviving entries still reference
(`remove()` at `:289-292` has the guard, `gc()` does not), and a pending
allocation from a crashed `alloc` (`:186-192`) is never reclaimed, so
`module_key.rs:6-8`'s *"the key space stays bounded"* is not true.

**Fix.** On a parse failure, mark the store degraded and refuse to save (or back
up the unreadable file and start clean with a new key epoch) rather than
silently truncating. Add the reference guard to `gc()`.

**Outcome.** The premise held, re-read first-hand at the then-current lines, and
all three findings were confirmed: `reload` kept the previous (in a fresh
process, empty) state on a parse failure and the next `with_lock` saved it,
discarding the file-ID → key table while the `.module` files stayed on disk;
`gc` freed keys without `remove`'s reference guard; and a pending allocation is
never reclaimed, which is what made `module_key.rs`'s *"the key space stays
bounded"* false.

**Recovery — preserve, then restart over an empty artifact directory.** On a
parse failure the store moves `<dir>/registry` to `<dir>/registry.corrupt[.<n>]`
and `<dir>/artifacts/` to `<dir>/artifacts.corrupt[.<n>]` (the first free name,
so an earlier quarantine is never overwritten), creates a fresh `artifacts/`,
drops the entry table while keeping the process's key frontier and dropping its
free list, and prints where both went to stderr. **Nothing is deleted.** The
mechanism is the one that satisfies the three criteria in order:

1. *never destroy state without a trace* — the bytes survive under a named path
   and the recovery says which, so the loss is diagnosable;
2. *never pair a recycled key with an artifact the registry does not describe* —
   the restarted space has **no artifact on disk at all** to collide with. The
   registry file was the only thing that mapped a file ID to a key, so the
   orphan `.module` files were already unusable; moving them *with* the index
   makes the restart provably collision-free instead of probabilistically so.
   An epoch base (`start above any old key`) cannot be used here: it needs the
   old `next_key`, which is exactly what the unreadable file no longer
   supplies, and a second recovery would have to guess it again. The frontier
   *is* kept, though — the restarted space starts where this process's
   allocator already stood, so it never hands a key to a second module while
   the lowlevel registry in memory still holds the first under it (the free
   list is dropped for the same reason);
3. *keep the toolchain working* — the store recompiles; the ledger's other
   option, "mark degraded and refuse to save", wedges the toolchain and buys
   nothing the move does not.

`reload` additionally stops clearing when the file is **still** unreadable after
a recovery in the same process: without that guard a preserve or a save that did
not take would clear the state a second time and leave `publish` without the
pending entry `alloc` made. *Residuals, stated rather than hidden:* the
quarantined `artifacts.corrupt[.<n>]` directories are described by nothing any
more, so `gc`/`clean` cannot reclaim them and one may accumulate per recovery;
the preserve is best-effort, and the cases that make it fail — an unwritable
directory, or a file another process holds against replacement — also stop
`save` from replacing the file, so the corrupt bytes stay in place rather than
being lost; and recovery is serialised by the registry lock, so a concurrent
process's next `with_lock` adopts the fresh registry instead of recovering too —
the one-process guard is for the case where no save ever landed.

**`gc` — the reference guard.** `gc` now mirrors `remove`: a dead entry whose key
a surviving entry still names is kept and not counted, because freeing that key
is what would hand it to a different module while an artifact on disk still refers
to the old one. In practice no *live* dependency can be dead — `dependency_file_id`
records only `.lichen` paths and `virtual:` names, both of which `gc` keeps — so
this is a defensive guard, but it costs one condition and the two removal paths
now agree.

**The pending-allocation reaper: investigated, deliberately not added.** An entry
is written at `alloc` time (under the lock) and completed by `publish` after a
compile that runs **outside** it, so from another process a live in-flight
allocation is indistinguishable from a crashed one: the entry is a key, an
all-zero source hash and no artifact, with no lease, timestamp or owner to age
out, and a reaper could pull a key from under a compile that is about to publish.
So the entry stays, and the claim is smaller than it reads: a crashed `alloc` is
**reused** by the next run of the same file — `alloc` returns the pending key and
`publish` completes it, pinned by `a_crash_between_alloc_and_publish_recovers` —
while `register_native`/`register_compute` allocate an embedded source's key and
never publish it by design (the frozen module is served in memory or cannot be
serialized), so each distinct `virtual:` name holds one key for the registry's
life. The space is therefore bounded by the file IDs the registry has **held**,
not by the live set; `module_key.rs` now says that instead of *"the key space
stays bounded"*.

**Tests.** `crates/lichen-registry/tests/device_recovery.rs` (new).
`an_unreadable_registry_is_preserved_and_the_key_space_restarts_clean` publishes
an artifact, corrupts `registry`, and asserts the restarted store has no entries,
that `registry.corrupt` holds the original bytes, that `artifacts.corrupt` is a
directory while the restarted `artifacts/` is empty, that the store keeps working
(a new file gets key 0 and survives a reopen), and that a second corruption
preserves both files as `registry.corrupt` / `registry.corrupt.1`.
`gc_keeps_a_key_a_surviving_entry_still_names` gives a dead `junk.txt` entry a
live dependent and asserts `gc` reclaims nothing and the next allocation does not
receive the referenced key. Both were confirmed to fail against the unfixed
`device.rs` — *"the recovery preserved no registry bytes"* and `left: 1,
right: 0`. A third,
`a_recovery_does_not_hand_out_a_key_this_process_already_used`, pins the kept
frontier: it fails with *"a restarted space must not reuse a key already handed
out in this process"* when the recovery resets `next_key` to zero, which is how
that half of the mechanism was checked. The two prose descriptions of the old
behaviour were corrected in place:
`crates/lichen-language-server/src/home.rs` and `docs/notes/liche-lsp-home.md` no
longer say the store "reloads the last-known (or empty) state … and repairs it on
the next save".

### P1-13 — Compiler-cache key omits `core_repo` and uses the wrong version `reported`

`crates/lichen-package/src/compiler_cache.rs:47-65`:

```rust
let mut spec = format!("lichen-language={}", env!("CARGO_PKG_VERSION"));
```

`core_repo` is a parameter of `ensure`/`ensure_lsp` but is **not** in the key, so
`lichen run --repo A` then `--repo B` reuses A's binary. And
`CARGO_PKG_VERSION` here is *lichen-package's* `0.1.0`; the comment at `:55-58`
claims it is *"the toolchain version … the key a change to any core crate should
bump"*, which no core-crate change does.

`crates/lichen-language/src/persist.rs:537-551` derives the **same slot** from
*lichen-language's* version, with a doc claiming the two agree — they agree only
while both crates are `0.1.0`. One version bump in one crate and
`lichen install` writes the binary into a slot the compiler never reads.

**Fix.** Put the slot-key spec in one place both crates call (both already
depend on `lichen-utils`), include `core_repo`, and add one equality test
(`needs-test`).

**Outcome.** The premise held, and both halves of it were re-read first-hand
before the fix: `key` hashed `"lichen-language=" + lichen-package's
`CARGO_PKG_VERSION`` plus the sorted plugin parts, `shipping_cache_root` hashed
the same string from lichen-language's `CARGO_PKG_VERSION` (the two agreed only
because both crates are `0.1.0`), and `key` never received `core_repo` although
`ensure`/`ensure_lsp` take it and hand it to the compositor build.

The derivation now lives **once**, in
`lichen_utils::cache::compiler_slot_key(core_repo, plugins)`
(`crates/lichen-utils/src/cache.rs`) — the leaf crate both sides already link,
whose only dependencies remain `sha2`/`slotmap`/`stacksafe`, so the new module
creates no cycle. `compiler_cache::key` resolves each plugin's version (the only
fallible half) and forwards; `persist::shipping_cache_root()` calls the same
function for the empty plugin set. The version is *that* shared crate's
`CARGO_PKG_VERSION`: the one value both binaries link, so an independent bump in
either caller can no longer move one side's slot and not the other's. The
`lichen-package` comment that claimed its own version *is* the toolchain
version, and that any core-crate change bumps the key, is gone. `core_repo` is
now a key input, so `--repo A` followed by `--repo B` no longer reuses A's built
compositor. The shipping slot is keyed by a **fixed** repository identity
(`lichen_utils::cache::DEFAULT_CORE_REPO`, which `toolchain::DEFAULT_REPO` now
aliases rather than repeats): the slot names the toolchain a home holds, not the
address `lichen install --repo` downloaded its bytes from — keying it by the
download address would put the bytes in a slot the compiler never reads. Keys
change, so every existing slot is invalidated and rebuilds, which is intended.
*Residual:* the version half still moves only when that one crate's version
moves; `core_repo` covers the repository half, nothing covers "a core crate was
edited without any version bump".

**Tests — and what the equality test can and cannot catch.** The derivation's
own inputs are pinned in `crates/lichen-utils/tests/cache.rs` (the repository and
the plugin set are part of the key; the set's order is not), and the equality
test itself is
`crates/lichen-language/tests/persist.rs`'s
`the_shipping_slot_is_the_one_the_package_manager_installs_into`: it compares
`shipping_cache_root()` with
`lichendir()/compilers/<compiler_cache::key(DEFAULT_REPO, &[])>`. That test spans
both crates from one test file, which needed a **dev-dependency** of
`lichen-language` on `lichen-package` — the two have no production edge in either
direction, so a dev-only one is not a cycle and does not change the shipped
graph. Both tests were shown failing against the defect they guard: removing
`core_repo` from the shared spec fails the first with two equal hashes, and
restoring the pre-fix inline derivation in `shipping_cache_root` fails the second
with two different slot hashes (`4377690f…` vs `26c240e8…`). *Stated plainly:*
the original defect was **not** behaviourally catchable at the time — with both
crates at `0.1.0` the two pre-fix derivations produced the same key, so a test
comparing them would have passed on the unfixed tree. What the equality test pins
is that the two sides still share one derivation and one repository identity; the
"one version source" property is structural (a single
`env!("CARGO_PKG_VERSION")`, in one crate) and is not something a test can
observe while the versions are equal.

### P1-14 — `run.rs` never checks `Build::ok` `reported`

`crates/lichen-language/src/run.rs:96-99` gates only on
`!report.diagnostics.is_empty()` and then `report.build.unwrap()`. Its three
siblings check `!b.ok` as well (`package.rs:511`, `:517`, `:695`). `Report::ok()`
(`lib.rs:137-139`) exists to combine both and neither `run.rs` function uses it.

**Fix.** Use `Report::ok()`.

### P1-15 — `Err(vec![])` — an error carrying no diagnostic `reported`

`crates/lichen-language/src/package.rs:511-513` returns `Err(report.diagnostics)`
when the build failed with no rendered diagnostics. `try_reuse` propagates it
with `?`, `render_all` over an empty list prints **nothing** — a silent failure.
`resolve_import` already papers over this hole twice (`:601-609`, `:629-637`)
with a synthesised diagnostic.

**Fix.** Guarantee at least one diagnostic on the failure path (synthesise a
"build failed" diagnostic at the report level), then delete the two workarounds.

**Outcome — the report-level synthesis, and why not to attribute the assert.**
`build_report` (`lib.rs:274-282`) now appends exactly one synthesised `Diag`
when `!build.ok` and the assembled diagnostic list is still empty, so the
invariant "a failure always carries at least one diagnostic" holds where the
report is assembled and every consumer inherits it. The two workarounds in
`resolve_import` are gone: it takes the failed load's own first diagnostic
(`first_diagnostic`, `package.rs:628-632`), which the invariant guarantees
exists, so the seam no longer has to invent a message.

*Reporting the skipped assert instead was rejected, on both skip conditions.*
Both are load-bearing and were left untouched:
- The `Static`-template skip (`highlevel/diagnostic.rs:444`, added in `940ebc6`
  *"assert metadata moves to a highlevel-side secondary map"*) has **no usable
  location**. The template is a node of the *imported* module, while both the
  location table (`node_edges`) and the user-facing flag (`user_asserts`) are
  keyed by **this build's** own dynamic nodes; the apply's clone has no entry in
  either, and the lowlevel's `AssertError` carries no apply site. There is
  nothing to attribute it to. Worse, a static template cannot be *classified*:
  `user_asserts` is this build's table, so a user `assert` and the generated
  array-bounds guard are indistinguishable through it — reporting every
  static-template assert error would re-report exactly the guard failure that
  the second condition exists to suppress (a bounds guard already fails the read
  with `EvalError::Index`). Closing it properly means carrying the flag *and* a
  location across the freeze, in the package metadata — a redesign, not this fix.
- The `user_asserts` condition (`:446`) is that deliberate dedupe. Untouched.

*Reachability, and how `P1-14` widened it.* This is reachable from **ordinary
source**, not a corrupt artifact: with `pkg.lichen` = `x => ! (x == 1)` and an
importer `@{f = import "pkg.lichen"@}f 2`, the apply's assert clone fires in the
*importer's* module with a `Static` template — measured: `build.ok == false`,
`assert_errors == 1`, `eval_errors == 0`, `user_asserts == 0`, `diagnostics`
empty. `ec9c4e4` (`P1-14`) is what puts `run` in this state: `evaluate`/
`evaluate_raw` now gate on `Report::ok()`, so a failed build with nothing
rendered returns `Err(vec![])` where the run previously proceeded. The state is
now a real diagnostic — `DiagKind::UnattributedFailure` with
`Diag::unattributed_failure()` (`highlevel/diagnostic.rs:134`, `:220`), rendered
by `render::checker_message` in the language layer — so consumers can match it
rather than parse a bare string.

**Tests.** `crates/lichen-language/tests/registry.rs:189` pins the direct path
(one `UnattributedFailure`, no `loc`), and `:207` pins the package-load seam the
deleted workarounds sat on. Both were confirmed to fail against the unfixed
sources: the first with `left: 0, right: 1` — the empty diagnostic list — and the
second with the fabricated *"cannot load package 'b.lichen': cannot resolve
import 'b.lichen'"*, the workaround's lie (the load failed, not the resolution).

### P1-16 — `stage_depends` wired on one of two store entry points `reported`

`crates/lichen-compiler/src/cli.rs` stages a file's `depend`/`plug`
directives onto the store before resolving; the editor's path
(`crates/lichen-language-server/src/analysis.rs:224`) calls `preprocess`
directly, so a vendored `import "alias"` fails in the LSP and the user gets a
false "cannot load package" diagnostic.

**Fix.** One line — stage the aliases in the LSP path too. Worth doing early
because it is user-visible and free.

**Outcome.** `Doc::new_with_cache` stages the source's `depend` directives on
the store it already builds, before its `preprocess` call, and folds the
staging diagnostics into the document's diagnostics — the same order the CLI's
`staged_store` uses. A vendored `import "alias"` now resolves in the editor as
it does from the CLI, and a dependency that has not been fetched reports the
missing-dir diagnostic instead of a false "cannot load package".

### P1-17 — Every LSP request runs the whole frontend `verified`

`crates/lichen-language-server/src/server.rs:186, 210, 236, 258` each construct
a fresh `Doc`; `Doc::new_with_cache`
(`crates/lichen-language-server/src/analysis.rs:208-247`) runs preprocess (with
a **new** `PackageStore`) → lex → parse → `frontend_at` → `build_report` (the
whole-IR checker). With `TextDocumentSyncKind::FULL` (`server.rs:125-127`)
there is no incremental sync, no debounce, no cache, and the store holds only
`String`. Each keystroke triggers a `didChange` analysis *plus* a
`semanticTokens/full` analysis.

Amplifiers:
- `analysis.rs:262-269` linearly scans the whole `ExprId → span` index once per
  import → O(imports × expressions).
- Every request re-opens the `DeviceRegistry` (`device.rs:91-102`:
  `create_dir_all` + read + full parse), and `verify` re-reads and re-parses it
  once per dependency file (`device.rs:241-242`) → N-file import graph = N
  registry reads + N parses **per request**.
- `concurrency_level(1)` (`server.rs:273-294`) serialises all requests and
  `spawn_blocking` is detached from the request lifetime, so there is no
  cancellation; one slow analysis stalls `shutdown` too. Combined with `P1-18`'s
  unbounded `plrun`, that is a permanent hang, not a slowdown.
- No document-version tracking (`server.rs:161-168` discards
  `text_document.version`; `publish_diagnostics` always passes `version: None`),
  so a client cannot reject stale diagnostics. No `did_save` /
  `did_change_watched_files` handler, so editing an imported `math.lichen` never
  refreshes the importer.

**Decision `D6` — (a) and (c) first, then (b).** Cache the extracted indexes per
`(uri, version)` and reuse one `PackageStore`/registry handle across requests,
and add cancellation plus a debounce; wire `BufferSession` afterwards (`P1-5` is
done, so the key is now genuinely injective and no longer blocks it). While T3 —
the memoized check — does not exist, (b) avoids the lex and parse but not the
check, so it is worth doing for the keystroke path and is not a substitute for
(a).

### P1-18 — Compute: unbounded globals, per-launch rebuild, unbounded `plrun` `verified`

`crates/lichen-compute/src/compute.rs:90-119` — two process-global
`OnceLock<Mutex<HashMap<…>>>` registries with **no** `remove`, LRU or reset.
Every `$jit` adds a fragment (`:329`, `:433`); every `plrun` adds a
`count`-element `Vec<i64>` (`:505`) that is never freed; every read **clones the
whole buffer** (`:118`). In a long-lived host this is unbounded growth,
compounded by `P1-17` (every keystroke re-runs the checker).

`run_parallel_kernel` (`:1739-1825`) and `run_kernel` (`:1663`) build a fresh
`wasmi::Engine`, `assemble_module` the bytes, `wasmi::Module::new`, `Linker::new`
and instantiate **on every call** — only the fragment is cached. This is the
crate's single largest optimisation opportunity.

`plrun`'s element count is program-controlled and uncapped (`:1818-1823` with
`output = vec![0i64; count]`): `2^40` requests ~8 TiB and 10^12 interpreted
iterations. It is reachable from *analysis*, which is the concrete mechanism
behind `P1-17`'s hang.

Also `:41-45` advertises a data-parallel `plrun` while `:1737-1738` admits it is
sequential — there are **no threads** in the crate. Operator names and module
docs overstate what runs.

**Decision `D6` — not part of the editor work.** Compute's registries, the
per-launch wasm rebuild and `plrun`'s uncapped count share nothing with the
language-server path, so they proceed on their own rather than waiting on the
`P1-17` sequence.

### P1-19 — `evaluate_block` expects a return the budget may refuse `reported`

Found while fixing `P1-2`, same class, second site: `evaluation.rs`'s
`evaluate_block` ends with `.expect("evaluated return node")` after
`evaluate_node_deep(root, None)`. A depth refusal returns `Void` before
`evaluate_node`, so the root's cached value stays `None`, `garbage_collect`
returns `None`, and the `expect` fires — **after** `drop_block` has already run.
`Parameterized` is a first-class, expected answer everywhere else in the crate,
so treating it as an internal error here is inconsistent. Reachability **was**
demonstrated, not merely reasoned: a child-block delegation with `deep_depth`
already at the limit fires it. Bare reads of the same `expect` also found a
**second, more ordinary trigger the item did not name**: a block whose root is
still `Parameterized`, which `evaluate_node_operation`'s postlude deliberately
never caches, so `garbage_collect` has nothing to move there either.
`P2-7` names the same line among the panics inside the descent, but `P2-7`'s
stated fix — the `VisitGuard` — would not change this trigger, so the two are
separate.

**Outcome.** The panic is gone; the fix is one line and does **not** touch
`P2-7`'s `visiting` work. `evaluate_block` now keeps the deep pass's own answer
and falls back to it when the compaction has no moved value:

```rust
let value = self.evaluate_node_deep(root, None);
self.garbage_collect(root).unwrap_or(value)
```

`unwrap_or(value)`, not `unwrap_or_else(Void)`, because the two no-cached-value
cases answer differently and the pass already said which: a refusal returns
`Void` *before* `evaluate_node` and never ran anything (its budget verdict is
already recorded, so the return is a **propagation** of that verdict, not a
second report), while a lazy block returns `Parameterized` and must stay lazy —
yielding `Void` there would forge a "computed nothing" (the residue of a
recorded failure) out of a legitimate "try again later", which readers like the
`TableGet` arm act on. Both markers are leaf values owned by no arena, so
neither needs the relocation `garbage_collect` exists to perform; the postlude
writes every arena-carrying answer, which is why "no cached value" implies the
pass's answer was one of the two leaves.

**Tests.** Both triggers are pinned in
`crates/lichen-lowlevel/tests/basic/evaluation.rs`:
`a_block_root_the_budget_refuses_yields_a_computed_nothing` (limit 2, the
refusal lands on the child block's root; asserts the recorded
`BudgetExhausted::EvaluateDepth` and a `Void` result) and
`a_block_root_that_stays_lazy_is_not_an_internal_error` (an unbound operand
makes the child block's root stay `Parameterized`; asserts the result is
`Parameterized`, not `Void`). Both were confirmed to fail against the unfixed
line — `panicked at crates\lichen-lowlevel\src\evaluation.rs:717:36: evaluated
return node` — before the fix. The whole `basic` target (134 tests) passes
after it.

### P1-20 — `download` uses a predictable shared temp name and skips `fsync` `verified`

Split out of `P0-4` by decision `D4`, which accepted the GitHub-over-TLS trust
root and narrowed `P0-4` to its documentation. What is left is mechanical, and
independent of the integrity question:

- `crates/lichen-package/src/toolchain.rs`'s `download` writes to
  `dest.with_extension("download.tmp")` — a **predictable** name derived from the
  destination, so two concurrent invocations targeting the same tool race on one
  file, and anything else on the machine can pre-create or replace it.
- There is no `fsync` between the write and the `rename`, so a crash can leave a
  truncated binary in place under the final name.
- The same module falls back to any `lichen-compiler` found on `$PATH` when the
  pinned release is unavailable, without saying so; a shadowing binary is then
  executed silently.

**Fix.** A unique temp name in the destination's directory, `fsync` the file (and
the directory) before the rename, and make the `$PATH` fallback say what it
picked. Do not add a checksum or a signature here — `D4` decided against both,
and re-adding one is a new decision, not a finishing touch.

**Outcome.** All three mechanical issues are fixed, and the integrity question
was left exactly as `D4` settled it (no checksum, no signature, no host
restriction). `download` now writes to
`<dest-stem>.download.<pid>.<counter>.tmp` — unique across processes and within
one — instead of the destination-derived `download.tmp`, so two concurrent
installs for the same tool can no longer interleave on one file and nothing can
pre-create or replace the temp by guessing its name. The downloaded file is
flushed with `sync_all` before the rename (the open is for **write** access, not
read: `sync_all` needs it on Windows), so a crash can leave a stray temp but not
a truncated binary under the final name. `resolve` now reports a `$PATH` hit on
**stderr** — *"`<bin>` is not installed in Lichen Home; using the copy on $PATH
at <path>"* — because that path executes whatever binary the environment
carries in place of the pinned release; stderr, not stdout, so `lichen path
language-server` keeps its one-line stdout contract (`P5-7`).
**Residual, deliberately left.** Only the file is flushed, not its directory, so
on a power loss the rename itself can still be lost — the destination then holds
the previous binary (or nothing), a clean re-download, not a truncated install.
The `$PATH` binary is still executed with no further check; the note is the
whole of the fix, by `D4`'s decision.

### P1-21 — A struct value applied through a deferred callee is still silent `verified`

The residual `P1-6` could not close, demonstrated first-hand:

```
$ cat p.lichen
S = struct<.a Int>
f = g => g 1
f S(.a 1)
$ cargo run -q -p lichen-compiler -- p.lichen
parameterized: ?a          # exit 0, no diagnostic
```

**Why it is not closable from the lowlevel as things stand.** A compute `Kernel`
is a struct — `[native, sig]` — and in this runtime a struct value *is*
structurally a `LowValue::Array`. `P1-6` moved `Array` to the lazy side for
exactly that reason, after `tests/compute.rs`'s cross-kernel cases caught the
first, `Array`-recording split (both failed with the new error, so the arm was
fixed rather than the test). The lowlevel therefore cannot tell a kernel's array
from a struct instance's array: only the **program** knows which of its values
are callable.

**Fix shape.** The apply arm needs the program's answer, which means a hook — the
same shape `Program::defer_pending` already establishes (a policy the lowlevel
consults where it would otherwise have to guess, with a default that refuses).
A `Program::is_callable(value) -> bool`, defaulting to `false` for a program that
names none, would let the arm record the error for a value the program disclaims
while leaving a kernel lazy. Cost: a new method on the `Program` trait, so every
program type states it — which is why it is its own item and not a footnote to
`P1-6`. The alternative, having the arm consult the program's operator dispatch,
was not evaluated and may be cheaper.

**Outcome.** The premise held, and the reproduction was confirmed first-hand
before the fix: `compile("S = struct<.a Int>\nf = g => g 1\nf S(.a 1)")` was
`ok` with **zero** diagnostics (the panic message of the new test is the `[]`
diagnostic list — the silence itself).

*What the callee actually is.* Traced through the arm: the deferred callee of the
cross-kernel cases (`jit_cross_kernel_call`, `jit_cross_kernel_subexpr`) reaches
the `_` arm as `LowValue::Array` — a 2-element `[value, type]` pair whose element
0 is the kernel — while a bare `jit` result can also reach it as the program's
own `ComputeValue::Kernel` value. The repro's `S(.a 1)` is an array whose element
0 is a `USize`. So the lowlevel's two lazy classes are exactly one question ("is
this callee applicable?") with two shapes, and only the program can answer it —
`kernel_id_of` is the JIT's answer and cannot distinguish a kernel pair from a
struct instance's pair.

*The route: the operator dispatch, not a `Program` method.* `OperatorExt` gains
`fn is_callable(module: &Module<P>, callee: AnyNodeId) -> bool`, defaulting to
`false` (read-only; the policy reads the module to recognise its own values
inside a structural array). The lowlevel's `Apply` catch-all consults
`<P::Operator as OperatorExt<P>>::is_callable(self, operands[0].node)` and
refuses with the existing `EvalError::ApplyTarget` when the answer is `false`.
The composition macro's generated `impl OperatorExt<LangProgram> for LangOperator`
ORs its **extension** operator leaves' policies (the two structural leaves name
no applicable value, so they are not consulted), and `lichen-compute` overrides
the policy on `ComputeOperator` — the leaf that compiles a kernel apply. No
method was added to `Program`, and no program type has to state anything: this is
the cheaper route the finding itself offered, and it is also the more honest home
— the operator leaf that can lower an apply is what declares the apply possible.

*The override.* `ComputeOperator::is_callable` answers
`kernel_id_of(module, node).is_some() || pending_kernel(module, node)` for a
dynamic callee (`false` for a static one: a kernel artifact is process-local, so
a frozen module carries none). `kernel_id_of` is the JIT's own predicate — the
call `emit_cross_kernel_call` needs to emit — so the lowlevel's laziness and the
JIT's lowering agree by construction. `pending_kernel` is the undecided half: the
lowlevel consults the policy **mid-deep-pass**, before the pair's value slot has
been evaluated (traced: the element's value was `None` while its operation was
`Index`), so a `Jit` that has not run counts as the kernel it will produce while
the walk follows the same two value edges `kernel_id_of` follows
(`value_of_node`'s `Index(pair, 0)` extraction and a struct pair's element 0).
The answer is a superset of the JIT's, so it can only keep a callee lazy that
later turns out not to be a kernel (conservative), never refuse one the JIT would
lower.

*Tests.* `crates/lichen-language/tests/pipeline.rs`'s
`an_apply_of_a_deferred_struct_value_reports_a_runtime_apply_target_error` (new)
pins the reproduction as one `DiagKind::RuntimeApplyTarget`; against the unfixed
arm it fails with `report.ok() == true` and an empty diagnostic list. The kernel
gate `cargo test -p lichen-language --test compute` is green (24 passed) — it
went red under both wrong shapes tried first, so it is load-bearing: recording
`Array` (before `pending_kernel` existed) failed `jit_cross_kernel_call` and
`jit_cross_kernel_subexpr`, and the lowlevel and highlevel suites are unchanged.

### P1-22 — The frontend's recursion overflows the caller's stack on a ~500-byte file `verified`

Found while fixing `P2-10`, which guarded the checker's own recursion. With
`check_term` guarded, a **source** file still aborts the process, and the file is
tiny:

```
$ python -c "print('('*250 + '1' + ')'*250)" > deep.lichen    # ~500 bytes
$ cargo run -p lichen-compiler -- deep.lichen
thread 'main' has overflowed its stack
STATUS_STACK_OVERFLOW (0xc00000fd)          # a process abort, not an error
```

250 nested parentheses abort; 150 succeed. Nested `[` behaves the same.

**Attribution corrected — this finding named the wrong layer.** It originally
claimed the parser was the *guarded* layer and the frontend walks were the
unguarded ones, and that argument is inverted. `(e)` is transparent
(`crates/lichen-language-parser/src/parse.rs:905-930`), so 250 nested parentheses
produce an AST **one node deep** — nothing after the parser recurses on that
input at all. The aborting thread is the parser's own **unnamed worker**, and it
is the *first* layer to overflow: re-measured, the 16 MiB worker dies at **175
nested parentheses (351 bytes)** while 174 (349 bytes) compile, and nested
brackets are identical because they share the path. The recorded `thread 'main'`
line did not reproduce. See `P1-23`.

The frontend walks *do* overflow, on a shape that is flat in the token stream but
left-nested in the AST — `1+1+…`, one `BinOp` per term, which the parser folds, so
only the walks recurse. Measured on the CLI's 1 MiB main thread: 200 terms check,
500 terms (~1 KiB of source) abort.

**Why it matters beyond the CLI.** The language server runs this same pipeline for
every request (`P1-17` makes that every keystroke), so a mere paste or file open
reaches it. A crash reachable from a file a user only *opens* is the cheapest kind
of denial-of-service, and a process abort reports nothing.

**Outcome.**  The guards landed, and the walks were the defect — but not for the
input the finding names.

*What now holds.*  `compile.rs`'s `compile_expr`, `resolve.rs`'s `resolve_expr`,
`resolve.rs`'s `KeyWriter::expr` (the `content_key` walk, reachable through that
public entry point) and the language server's three walks (`NameClass::expr`,
`Walk::expr`, `ScopeCapture::expr`) each recurse once per nested expression on
the caller's thread.  Every cycle in a walk re-enters through its expression
method, so one `#[stacksafe]` on that method — not on each helper it calls —
covers the walk.  `stacksafe = "1"` is now a dependency of `lichen-language` and
`lichen-language-server`; no depth limit was added.  The recursion grows the
stack, so a deep AST no longer aborts the caller.  Measured with the guards
removed one at a time: unguarding `resolve_expr` alone, `compile_expr` alone, or
the server's three walks alone each restores a process abort, so all five are
load-bearing.  `pipeline.rs`'s
`a_deep_operator_chain_compiles_without_an_overflow` (2000 terms) aborts the test
binary before the fix and checks after it in 0.09 s.

*The recorded repro is a different layer, and a shallower one.*  Re-measured
(debug, this revision): the 250-parenthesis file still aborts **after** the fix,
and the aborting thread is the parser's *unnamed worker* — `thread '<unknown>'
… has overflowed its stack` — not `main`; the recorded `thread 'main'` line did
not reproduce.  The 16 MiB worker overflows at **175 nested parentheses (351
bytes); 174 (349 bytes) compile**, and nested brackets are identical (174/175),
because `(e)` is transparent (`language-parser/src/parse.rs:905-930`) — that
input's AST is one node deep, so nothing after the parser recurses at all.  The
parser is therefore the *first* layer to overflow, at roughly 16 MiB / 175 ≈
94 KiB of stack per syntactic nesting level; the 16 MiB thread is itself
reachable at 175 levels, and is not the "deeper than 250" layer this finding
assumed.

*Where the walks do overflow.*  A shape that is flat in the token stream but
left-nested in the AST: `1+1+…`, one `BinOp` per term.  The parser folds it, so
only the walks recurse.  On the CLI's 1 MiB main thread 200 terms check and 500
(~1 KiB of source) abort; the language server builds a 2000-term `Doc` only with
its three walks guarded.  After the fix 2000 and 6000 terms check, and 20 000
terms abort in the parser's worker.

*What still aborts.*  Three paths grow stack outside these walks, so this removes
the abort without bounding stack growth: the parser's 16 MiB worker at ~175
syntactic nesting levels; the AST's own recursive `Drop`, which no `#[stacksafe]`
can reach (lex + parse + `mem::forget` survives 8000 nested terms on a 1 MiB
stack, lex + parse + drop does not); and whatever stack the caller gives the
language server's blocking thread.  Whether the frontend needs a nesting-depth
limit, and where its number should come from, is therefore still open — and the
first number it must clear is the parser's **175 levels**, not the frontend's.
**Resolved by `D9`: no limit, accepted risk** — `P1-23` and `P1-24` are closed
`wontfix:D9`.

### P1-23 — The parser's 16 MiB worker overflows at 175 nesting levels `verified`

Split out of `P1-22` when its attribution was corrected: the parser is the
**first** layer to overflow, not the guarded one. `(e)`, `[e]` and their
neighbours are transparent in the grammar, so the recursion depth is the
*syntactic* nesting depth of the source, one frame per level.

```
$ n=174  → compiles            (349 bytes)
$ n=175  → thread '<unknown>' has overflowed its stack     (351 bytes)
```

Measured on this revision with `(` … `)`; `[` … `]` is identical because they
share the path. That is roughly 16 MiB / 175 ≈ **94 KiB of stack per nesting
level**, which is a lot per frame and suggests the recursion runs through
`chumsky`'s combinatorial machinery rather than one thin function.

**Why it cannot be fixed the way `P1-22` was.** `#[stacksafe]` works by wrapping
*a function of ours*, and the recursion here lives **inside the parser library**.
Growth would have to be arranged around the whole parse — for example running it
on a thread whose stack is grown on demand rather than a fixed 16 MiB — which is
a real design change, not an annotation. It is therefore part of the open
nesting-depth question rather than an independent fix.

**Why it matters.** A 351-byte file aborts the process, and the language server
parses on every request, so this is the crash a user reaches first. `P4-3` names
the walk duplication and the parser's cost; this is the crash.

**Closed `wontfix:D9`** — accepted risk, not repaired. See `D9` for why a limit
safe to enforce today would be dictated by this thread size, and for what
revisiting it would take.

### P1-24 — The AST's own recursive `Drop` overflows on a deep tree `verified`

Found while measuring `P1-22`. `#[stacksafe]` guards the walks, but the tree is
still freed by the compiler-generated recursive `Drop` for a boxed recursive
enum, and **no annotation of ours can reach it**:

```
lex + parse + mem::forget   survives 8000 nested terms on a 1 MiB stack
lex + parse + drop          does not
```

So a program deep enough to survive parsing and checking still aborts on the way
out — after all the work succeeded, which makes it the most confusing of the
three to debug. The only fixes are an iterative `Drop` impl or a
`ManuallyDrop`-based teardown for the expression type, which is a change to the
AST's shape and not a local guard; it is therefore part of the same nesting-depth
question as `P1-23`.

**Closed `wontfix:D9`** — accepted risk, not repaired. The cost of leaving it is
stated where it bites: the abort lands *after* a successful parse and check, so
the only symptom is a process that dies having printed nothing wrong.

### P1-25 — A dependency's package name is written as Rust source `verified`

Found while fixing `P5-8`, which closed the same class in the generated
`Cargo.toml`. This is the other generated document, and it is worse: the manifest
is data, but **this one is code**.

`crates/lichen-package/src/plugin.rs`'s `compose_source` writes a generated
`src/main.rs` containing

```rust
use {crate_ident} as {crate_ident}_leaves;
```

as **Rust source**, where `crate_ident` is the dependency's `package` field with
`-` replaced by `_`, and `native_package_lines` writes the same identifier inside
a Rust **string literal**. `package` reaches that point as a source-file string,
and the preprocessor's string literal is `"[^"@]*"` — documented *"no escapes,
may be multiline"* and pinned by its own test
(`crates/lichen-preprocess/src/lex.rs:42-48`, `:198-206`). A newline or a `}` in a
`depend` declaration's `package` therefore closes the generated item and injects
arbitrary Rust, which `cargo build` then **compiles and, for a plugin, runs**.

**Fix.** The generated source must be assembled from tokens the generator
controls, not concatenated from a caller's string: emit the identifier only after
checking it is a legal Rust identifier (the crate-name alphabet is known and
narrow — letters, digits, `_`, not leading-digit), and refuse the plugin with a
diagnostic otherwise. The string-literal site needs the same treatment as the
manifest got: escape, or better, stop interpolating an arbitrary string into a
literal and pass the value through a generated constant or an argument.
`P5-8`'s `toml_string` helper is the precedent for the escaping half — do not
invent a second convention.

**Severity is high, not low, which is why it is here and not in `P5`:** the input
is a dependency declaration, which is exactly what a user pastes from a README,
and the outcome is code execution inside the compiler's own build. The manifest
half was bad enough to be its own item; this half is what makes the pair a
supply-chain concern rather than a formatting bug.

**Outcome.** The identifier half held, and was reproduced first-hand before the
fix. Re-derived on this revision: `crate_ident` was
`git::crate_name(dep).replace('-', "_")`, written as **Rust code** at two sites
(`compose_source`'s `plugins = [<ident> as <ident>_leaves;]` arm and
`native_package_lines`'s tuple), and `git::crate_name` is `dep.package` else the
binding name. `dep.package` is a source-file string: `parse_depend` reads it with
`expect_string_after_eq`, whose token is the lexer's `"[^"@]*"`
(`lichen-preprocess/src/lex.rs:42-48`, pinned by
`a_string_may_span_lines_and_hold_commas`, `:197-206`), documented "no escapes,
may be multiline" — so a `}` or a newline reaches generation intact.

*Pre-fix, observed.* A plugin whose `package` was
`x;\n}\nfn injected_by_a_package_name() {}\n//` generated, verbatim,
`plugins = [ x;⏎}⏎fn injected_by_a_package_name() {}⏎// as x; …` — a top-level
function injected beside the composition, compiled by `cargo build` and run by
the plugin-built compiler. The same `package` was repeated inside the
`native_package_lines` tuple. The new
`crates/lichen-package/src/tests/plugin_source_tests.rs` fails against the
unfixed generator with that text in the panic message and passes after the fix.

*One correction to the finding's mechanism.* The string-literal site does **not**
carry the identifier. `native_package_lines` writes `{ident}` as code (three
times) and puts the **alias** — `dep.alias()`, the `name` binding — inside
`"{alias}.lichen"`. That alias is not arbitrary from a source file: `parse`
lexes it from a `Name` token, `[A-Za-z_][A-Za-z0-9_]*`, so a source-file
`depend`/`plug` declaration can never carry the `"` that would end the literal.
It *is* arbitrary from a host that composes a `Depend` by hand and calls the
public `plugin::rebuild`/`rebuild_lsp` — `Depend` has public fields — and that
host can inject through the literal, which the second test pins with
`name = evil"); fn injected_by_an_alias() {} //` (pre-fix output:
`("evil"); fn injected_by_an_alias() {} //.lichen", …`).

*Fix.* `crate_ident` now returns `Result`: hyphens become underscores, then the
result must be a Rust identifier (ASCII letter or `_` first, ASCII
alphanumerics or `_` after) and not a keyword, so `compose_source` and
`native_package_lines` refuse the dependency with a diagnostic —
`"plugin '<alias>' has package name '<package>', which is not a Rust crate
identifier"`, the same refusal style `Depend::vendored_dir` uses for a bad
`sub`. Both functions return `Result`, and both `write_*_main_rs` callers
propagate before any source is written.

*The string-literal site needs no escaper.* The alias is required to be spelled
in the identifier alphabet — the alphabet a source-file binding name is lexed
from, so every in-tree `Depend` passes — which is the construction that needs no
escaping at all. `toml_string` was not reused as an escaper: TOML spells a
control character `\uXXXX` and Rust spells it `\u{…}`, so the manifest helper's
output is not a Rust literal, and a second escaper is exactly the convention
this item says not to add. No behaviour changes for a source-file plugin; a
hand-composed alias carrying `-`, `.`, or `"` is now refused rather than
written.

### P1-26 — The table-key hash changed meaning without an artifact version bump `verified`

Found while fixing `P1-3`/`P1-4`, which changed **what a stored table-key hash
means**. The artifact container's version was not bumped, and it is the reader's
only guard: `crates/lichen-language/src/persist.rs`'s `ARTIFACT_FORMAT_VERSION`
is still `4`, so a cache written by an earlier build is accepted, its payload
hashes are stale, its payload entries are **sorted by the old hash**, and every
lookup for a cyclic-array, table or function key misses — silently, as a
`TableMiss` the user cannot distinguish from a genuine one.

Scalar and acyclic-array hashes are byte-identical to the old function (`P1-3`
kept `NONE_TOKEN`, `ARRAY_SEED` and the fold structure), so the exposure is
exactly the key kinds the old hash could not survive a freeze for anyway — which
is why this is a one-line fix and not a migration: the artifacts it invalidates
were already useless.

**Fix.** Bump `ARTIFACT_FORMAT_VERSION` from `4` to `5`. The constant's own doc
already states the rule this restores (*"a change to either half bumps it and
retires the artifacts written before the change: they fail the version check and
recompile"*), so the fix is the number and nothing else. Do not add a
compatibility path — there is no version of this format worth reading.

**Outcome.** The premise held as written, and the fix is the one number. The
constant is a private `const` with a doc at `persist.rs:100-105` — `4` before,
`5` after — the writer emits it at `:309`, and the reader's check at `:483` is
its only comparison. The doc names no version, so it reads unchanged after the
bump; the header layout, the hash implementation and the compatibility surface
are untouched.

*Shown by hand, on this revision, with a throwaway test that was deleted before
the commit.* The pre-bump build compiled a one-line package into
`artifacts/<file-id>.module`; the header's version field decoded to `4`, and the
**pre-bump** reader accepted those bytes — the check at `:483` compared `4` to
itself, which is the defect. The artifact was copied aside before the edit.
Against the bumped build, the saved bytes are rejected with precisely
`unknown artifact format version` — the version check, before the key check
(`:486`), the hash check (`:489`) and the body digest (`:501`) — while a fresh
compile plus a second store's reload still round-trips
(`cache_round_trip_across_stores`), so the writer and the reader agree on `5`.
The invalidation is exactly the set `P1-3`/`P1-4` already made useless: scalar
and acyclic-array hashes are byte-identical to the old function, so only the
cyclic-array, table and function-key payloads — the ones the old hash could not
survive a freeze for — carry a rewritten hash.

## P2 — architecture

### P2-1 — `BufferSession` is built but unwired `verified`

~1100 lines of incremental machinery (`language/src/session.rs`,
`resolve.rs:385-706` `content_key`, `lex::lex_resume`,
`parse::parse_statement_region_traced`, `parse::collect_error_blocks`) have **no
production consumer** — only their own unit tests. The LSP, the one component
that needs them, builds its own pipeline through `frontend_at`/`build_report`
and re-derives name resolution with its own scope walk.

`crates/lichen-language-server/src/analysis.rs:5-7` still claims the opposite:

> `Doc` … the *checker* via [`BufferSession`] for the full diagnostic set.

The file neither imports `session` nor calls it. This stale doc is what hides
`P1-17`. `docs/notes/incremental-parse-compile.md` marked "current" is also
generous: T3 (memoized check) is not implemented, so even a wired session would
only avoid lex/parse.

**Fix.** Correct the doc immediately; wiring is `D6`.

### P2-2 — Five hand-written AST traversals `verified`

`analysis.rs` alone has three near-identical ~110-line recursive walks over the
same ~30 `Expr` variants: `NameClass::expr` (`:1292-1439`), `Walk::expr`
(`:1540-1697`), `ScopeCapture::expr` (`:1832-1987`) — plus `resolve.rs:478` and
`compile.rs:379`. Adding an `Expr` variant means editing five sites.

`lowerlevel`'s sibling `checker.rs:985-994` `range_children` uses a **wildcard**
`_ => unreachable!("expected a variadic expression kind")`, so a new variadic
kind compiles cleanly and panics at runtime — while its two siblings
(`annotations.rs:29-81`, `ir.rs:451-540`) are fully enumerated and *would* break
the build. `range_children` should be enumerated.

**Fix.** Enumerate `range_children` (small, do it early). A shared visitor is a
bigger design call — propose before doing.

### P2-3 — `Build` is a god-DTO `verified`

`checker.rs:355-412` exposes four index-aligned vectors
(`term`/`val`/`ty`/`attr: Vec<Option<NodeId>>`) plus ~20 more `pub` fields, and
the crate reads them with `self.ty[x].unwrap()` / `self.term[x].unwrap()` about
**30 times** across `checker.rs`, `checker/{structs,lambda,indexing,annotations}.rs`.
The invariant "every expression has a type by the time we read it" is assumed,
not encoded, so a checker bug becomes a panic on user input. Downstream
(`analysis.rs` 9 sites, one test) indexes the vectors directly, making any
layout change cross-crate.

**Fix.** A single `Vec<ExprState>` (four `Option<NodeId>` fields) removes the
"indices disagree" hazard; a checked accessor that records a diagnostic removes
the panic surface. Both are invasive — propose before doing.

### P2-4 — `Node`'s `pub` fields break the write choke-point `reported`

`lowlevel/src/lib.rs:636` makes `value` private with an explicit contract
(*"External crates must never touch the field directly"*), but `:646-666` leaves
`operation`, `function`, `block`, `visiting`, `evaluated_deep` and `equality`
`pub`. From safe host code: writing `evaluated_deep = Some(..parameterized:
false)` makes `function.rs:315` treat a parameter-dependent body as proven
concrete and reuse it across calls (silently wrong results); a wrong `block`
makes GC (`gc.rs:148, 216, 224`) drop a live node or retain a dead one; a wrong
`equality` meta breaks `disjoint`'s documented contract and hits
`disjoint.rs:105`. The crate's own tests write these fields in ~20 places, so
this is a de-facto API.

**Fix.** Privatise the invariant-bearing fields behind `&mut`-taking methods, as
`value` already is.

**Outcome — done: all six fields are private, read through `Module` accessors,
and the two that had no write path got one that establishes the invariant.**
`D11` settled the scope after the refutation below was recorded; the refutation
and the measured cost are re-derived first-hand here and both still stand.

*What the choke-point is.* The crate documents exactly one write choke-point and
it is scoped to the **value**: `Module::write_node_value` (`equality.rs:79-107`)
— *"This is the single choke-point for value writes: every place a value lands on
a node that might be a member of a unified class goes through here"* — with
`Node::value`'s own doc (`lib.rs:756-762`) adding *"written only through the
controlled `Module::write_node_value` API … External crates must never touch the
field directly"*. Its sibling is `Module::set_node_shape`
(`static_module.rs:86-90`), the only writer of the private `low_shape`.

*The premise's mechanism is false: the value choke-point is implemented, not
prose.* `value` and `low_shape` are already private, and the whole crate writes
`value` in exactly two statements — `equality.rs:94` and the replication loop at
`:101` — both inside `write_node_value`; `low_shape` is written once, at
`static_module.rs:88`, inside `set_node_shape`. **No public field lets any code
bypass the value write path**, and the class-consistency invariant its doc claims
*is* maintained at one site. The ledger's lines drifted as well: the six fields
are at `lib.rs:773-793`, not `:646-666`.

*The defect behind it is real, but it is not the choke-point's.* `operation`,
`function`, `block`, `visiting`, `evaluated_deep` and `equality` are `pub` and
carry invariants of their own — the deep pass's proven-concreteness flag, the
template-ownership tag, GC's block membership, `disjoint`'s class links — so a
safe host can break any of them. What it cannot do is perform a value write other
than through the choke-point.

*Why that makes the fix a redesign, not a privatisation.* No documented write
path covers those six fields: the crate writes them from ten of its own modules
(`add_node`, `add_function`, `gc`'s compaction, `function.rs`'s apply clone,
`static_module.rs`'s static apply, `evaluation.rs`'s mark and deep pass,
`equality.rs`'s `force_pending`), so making "the write path" the only one would
mean **inventing** a choke-point for node state.  `D11` accepted exactly two
invented write paths (`operation` and the function tag) rather than a general
choke-point, and rejected leaving the fields public as they are.
The call-site cost is not local either. A temporary privatisation of the six
fields, run through `cargo check --workspace --all-targets`, produced **119**
`Node`-field errors: 116 in the crate's own integration tree and 3 in
`lichen-highlevel`, and that is a floor — the compiler stops before
`lichen-compute` and everything downstream of the failing crate, so a grep adds
~12 `operation` reads and one `equality.parent` read there. Of the 116 test-tree
errors, 26 are **writes** (24 to `operation`, one to `function` at `main.rs:436`,
one to `equality.parent` at `equality.rs:43`) and 90 are **reads**, including
`block` (~24), `evaluated_deep` (~13) and `visiting` (3) which have no accessor
at all. Rust cannot make a field write-private while leaving it read-public, so
every one of those sites needs an accessor — and the reads are internal state
(`evaluated_deep`'s proven flag, `visiting`'s in-progress mark, `disjoint`'s
`Meta`) whose exposure would **widen** the public surface rather than narrow it.

*The tests did not conflict with the choke-point; they conflict with the
privatisation.* Every test **value** write goes through `write_node_value`
(`tests/basic/assert.rs:139,186,261,378`, `equality.rs:439-440`,
`function.rs:62,155,696`, `main.rs:506,548,613,619`, `evaluation.rs:223`) — the
path is respected throughout the suite, which is the opposite of the ledger's
"the tests … do write them" read as a bypass of the choke-point. The direct
writes are real but are graph construction: the 24 `operation` writes close an
operation cycle the public API cannot express, because the second `add_node` needs
the first node's id. The extent the ledger gives ("~20 places") is wrong in one
direction: it is **26** writes, plus ~90 reads. So the tests can construct and
inspect the graph only by touching fields, and the tension is between the
invariant and the *harness*, not between the invariant and the choke-point.

**What landed.** The six fields are private; every external reader goes through
a `Module` accessor, and the missing write paths are supplied — two invented
(`operation`, the function tag), one routed to the operation the union-find
already exposes (`equality.parent`).  Behaviour is identical:
the crate's own code still writes the fields directly (privacy is crate-wide),
no public signature that existed before changed meaning, and no evaluation,
checking or hashing result moved — the whole verification battery passes and
`cargo clippy` reports the same warning locations as before the change (65
distinct; `D12`'s 66 counts a duplicate target).

*The read surface, field by field.*  Each accessor's doc states what the reader
may rely on; that contract is what a public field cannot give.

- `operation` → `Module::node_operation(node)` → `Option<Operation<P>>`.  The
  operation is **defined once** — by `add_node`, or by `close_operation_cycle`
  when the operand is only nameable after the node — and never replaced, so an
  operand edge read once is the edge that computes the node.  `Some` does **not**
  mean "unevaluated": an operation node caches its result, read
  `node_value` for the current value.  The operand is a graph edge, so a walk
  must keep a visited set, exactly as the deep pass does.
- `function` → `Module::node_function(node)` → `Option<FunctionId>`.  The tag's
  chain through `Function::parent` **is** the apply clone walk's membership test
  (not a lookup in `Function::nodes`), so a wrongly tagged node is cloned or
  referenced as a member of the wrong template.
- `block` → `Module::node_block(node)` → `BlockId`.  The garbage-collection unit
  whose lifetime bounds the node, and the arena its compound payloads live in;
  exactly one live block per node.
- `visiting` → `Module::node_visiting(node)` → `bool`.  A **liveness** mark, not
  a "was visited" flag.  `P2-7`'s guard releases it on every exit — the cached
  answer, the lazy answer and an unwinding panic alike — so `true` means an
  active frame is computing the node *right now*; it never means "already
  evaluated" (read `node_value`) and never means "known concrete" (read
  `node_evaluated_deep`).  Because it is never sticky, `true` on a node with no
  cached value is a genuine cyclic read.
- `evaluated_deep` → `Module::node_evaluated_deep(node)` →
  `Option<EvaluatedDeep>`.  `Some` means the deep pass ran on the node and
  `parameterized` records whether any node in its reachable subtree is
  `Parameterized` — the pass could **not** prove the subtree concrete.  `None`
  means concreteness is **unknown** and must be read as parameterized, never as
  proven concrete: a budget refusal and a node reached only as an operand both
  leave `None`, and the apply clone walk and the operation postlude both read it
  that way.
- `equality` → `Module::node_equality(node)` → `disjoint::Meta<NodeId>`.
  `parent` is the union-find link (`None` = this node is the class's root);
  `next`/`tail`/`size` describe the member list and are meaningful only at the
  representative.  A reader that needs the representative uses
  `equality_representative`, which also compresses, never a hand parent walk.

*The invented write paths — why neither is a bare setter.*

- `operation`: `Module::close_operation_cycle(node, operation)` is the **late
  half of a two-phase construction**, named for the cycle it closes: a recursive
  value's operation names a node allocated after it, up to the self-referential
  `node := op(node)`, so a constructor cannot express it in one call.  It
  refuses a second definition (an operation is defined once — replacing it would
  strand the previous operand edge), refuses a node that already holds a
  concrete value (a decided node never runs its operation, so the edge would be
  dead), and **clears the node's deep-pass verdict**, which predated the new
  operand edge and no longer describes the graph.  On the freshly allocated node
  every caller passes, that clearing is already a no-op — which is why the
  repair is invisible in results while it closes the hole a plain field write
  left open.
- `function`: `Module::register_in_function(function, node)` writes the owner
  tag **and** appends the node to `Function::nodes`, because the two halves must
  agree and are read for different purposes: the clone walk follows the tag
  chain, while garbage collection and a nested closure's clone walk start from
  the scope list.  The function's own value node is the one node tagged without
  joining the scope, and `add_function` does that in place.
- `equality.parent`: **no new writer**.  Its legitimate writer is the union-find
  itself, and the union-find's operation is already public — `add_equality`.  The
  single external site that hand-built a parent chain is a test, and it now
  builds the chain with `add_equality`; nothing else outside the crate wrote the
  link.
- `block`, `visiting`, `evaluated_deep`: no public writer at all.  Their only
  legitimate writers are the crate's own garbage collection, evaluation mark and
  deep pass, and those field writes stay inside the crate.

*Reads through the module, not the slotmap.*  `Module::nodes` stays public: it is
the node **table** — the slot allocation that names a node and owns its lifetime,
walked by the artifact codec and by `disjoint::members` — but it can no longer
reveal node **state**, because all six fields are private, so the accessors above
are the only route.  No per-`Node` accessor was added; that would have been the
second route the decision's scope note forbids.  Making the table itself private
is a storage-API change (184 `.nodes` uses outside `lichen-lowlevel/src`, across
five crates) and belongs to a different item.

*Residual — a `lichen-utils` hole, not an absent `Node` accessor.*  Because
`Module::nodes` is public, a caller can still obtain `&mut Node<P>`, and the
crate's `impl disjoint::Node for Node<P>` exposes `meta_mut() -> &mut Meta<NodeId>`;
`lichen-utils`' `Meta` has public fields, so an external crate can still write
`equality.parent` with `m.nodes[id].meta_mut().parent = Some(p)`.  Closing that
means making `Meta` opaque (with read accessors and a constructor for the frozen
mirror `static_module.rs` builds) or making the node table private.  It is a
container/`lichen-utils` API change outside `D11`'s six-field scope; it is
reported here rather than taken.

*The tests.*  Every direct write in the integration tree is expressed through the
new surface.  Two tests needed more than a rename, and both preserve their
meaning:

- `root_node_compresses_deep_paths` built a four-deep parent chain by writing
  `equality.parent` — a shape `union`'s size rule cannot produce.  It now builds
  the deepest chain the union-find **can** produce at five nodes (one class root
  attached under a larger class root, leaving a member two edges deep) through
  `add_equality`, and asserts the same property: the whole path flattened onto
  the representative.  The deep-chain compression case is still covered where it
  belongs, in `lichen-utils`' own `disjoint` tests.
- `cyclic_operations_panic_instead_of_looping` used to *replace* `a`'s operation
  to close its cycle; the define-once contract refused that, and the test now
  allocates `a` operation-free and closes the cycle once — the graph and the
  expected panic are unchanged.

*Cost, measured.*  The trial privatisation re-run on this checkout produced
**115** `cargo check --workspace --all-targets` errors in `lichen-lowlevel`'s
integration tree (116 when the previous attempt measured it) and 3 in
`lichen-highlevel`'s library, and the compiler stopped before `lichen-compute`.
Counted past the blocked targets, the error list is **132** sites: those 118,
plus 12 in `lichen-compute`, 1 in `lichen-render` and 1 in `lichen-highlevel`'s
own test.  The change rewrote **131** external sites — 114 in the lowlevel
integration tree, 4 in `lichen-highlevel` (3 library, 1 test), 12 in
`lichen-compute` and 1 in `lichen-render` — and one more inside the crate
(`add_function` now registers through the API), 132 call sites in all.  The
`compute`/`render` share is the promised "roughly a dozen more" the compiler
never reached.

### P2-5 — `NativeApply` is an unvalidated escape hatch `reported`

`highlevel/src/native.rs:29-34` hands a plugin's `build` back as three raw
`NodeId`s, and `checker.rs:1349-1352` adopts all three with **zero validation** —
not that `node` is a `[value, type]` pair, not that `ty` is in the current
block, not that `node` is a member of the enclosing function's template. This
directly contradicts `program.rs:120-134` (*"an extension can never build into
the wrong block"*). Every other extension returns nodes the checker itself
allocated through `Ctx`.

**Fix.** Validate shape and block membership, recording a `NativeOpContract`
guard on failure — the same pattern `ImportExport`
(`checker.rs:1266-1278`) and `NativeOpUnresolved` (`:1335-1346`) already use.

**Outcome.** The premise held in its main half and is corrected in the other.
Re-derived: `NativeApply` is a public type (`highlevel/src/native.rs`), `NativeOp`
is a public trait, and `native_ops` reaches the checker through the public
`Checker::build_in_attr_native` — so this is an extension point a **host
composes**, not an internal helper, and the checker adopted all three records
with no validation (the now-current `check_native_call`). A plugin can return
the three ids from anywhere it can reach: a bare `ctx.value_node`, the pair's
slots disagreeing with `val`/`ty`, or a node it stashed from an earlier module.

*Corrected: the block-membership half is not a contract for `ty`.* Measured
first-hand with a probe that printed each builder's blocks: compute's `$range`
returns `node_block=12v1 ty_block=1v1 current=12v1` — its `ty` is
`ctx.int_type()`, the checker's canonical shared type expression, allocated once
in the **root** block and deliberately referenced from a pair built in a kernel
body (as `Ctx`'s own doc says: the markers and canonical types are *referenced,
not rebuilt*). A "`ty` must be in the current block" check would refuse a
legitimate in-tree plugin. The template-membership claim is likewise not the
lever: `Ctx` registers every node it allocates in the current function's
template, so a plugin that builds through `Ctx` satisfies it by construction,
and nothing the checker reads distinguishes a stashed in-block node from a
built one.

**Fix.** The shape is validated where it is relied on, in `check_native_call`:
`node` must be a two-slot array in the block the call is compiled into, element
1 exactly `ty`, and element 0 exactly `val` when `val` is `Some` (compute's
`$jit`/`$launch`/… legitimately return `val: None`, leaving element 0 the op
node the runtime reads). A failure records the new `DiagKind::NativeOpContract`
(a guard, `field` = the operator name) and leaves the well-formed hole the
`ImportExport` and `NoAttributeExtension` guards leave — a fresh-cell pair — so
nothing downstream reads a malformed term. The message is rendered by
`render::checker_message` as *"native operator '{name}' returned a malformed
term — a native operator must return the [value, type] pair it built in the
current block"*. `val` is copied into the guard path instead of being consumed,
and the location is cloned for the builder call, so the guard still has both.

**Tests.** `crates/lichen-highlevel/tests/native.rs` (new) composes a probe
`NativeOp` into `Checker::build_in_attr_native` over a hand-built
`ExprKind::NativeCall` and pins three facts: a builder that returns a bare value
node is refused, a builder whose `ty` is not the pair's element 1 is refused,
and a well-formed builder is adopted. Against the unfixed checker — the guard
bypassed, everything else in place — the two hostile cases fail exactly as the
finding predicts: `build.ok == true`, i.e. the malformed term is **accepted**,
not refused. The well-formed case passes both ways.

### P2-6 — Repo tooling inside the compiler library `reported`

`language/src/readme.rs` plus the `sync-readme` binary is a README/example
generator that reaches two `..` hops out of `CARGO_MANIFEST_DIR` (`:55`, `:60`),
executes every example program (`:296-308`), panics on a missing examples dir
(`:77`), and follows symlinks with unbounded recursion (`:83`, `:191-242`). Under
`cargo install` the directory does not exist, so it panics. It ships inside the
library, reachable by any consumer.

Related: `clap` is a hard dependency of `lichen-language` but only `cli.rs` uses
it, so `lichen-language-server` links it for nothing; `logos` and `sha2` are
**unused** dependencies of that crate.

**Fix.** Move `readme.rs` + `sync-readme` to a tools crate (or behind a
`readme-sync` feature), move the CLI behind a feature or into `main.rs`, and drop
the two unused deps.

**Outcome (partial — the item stays open).** The premise held, re-derived, and one
half landed: `logos` and `sha2` were **unused** direct dependencies of
`lichen-language` and are gone (`Cargo.toml`, lockfile), which is the whole of the
second finding. The ownership move was **stopped** one step in, and a later reader
should not read this item as closed:

*Why the CLI cannot simply move into `main.rs`.* It is not only the binary's
entry: `crates/lichen-package/src/plugin.rs` generates a plugin compiler's
`main.rs` that calls `lichen_compiler::cli::main_with_native_packages::<crate::LangProgram>(…)`,
so `cli` is a **public library API** with an out-of-tree caller this repository
itself generates. Moving it needs the generated manifest and the release
workflows to move with it; that is `P2-12`, split out so this item's remaining
half (the *repo tooling*) can be fixed on its own.

*The remaining half is the one that matters.* `readme.rs` reaches two `..` hops
out of `CARGO_MANIFEST_DIR`, executes every example, panics on a missing examples
directory and follows symlinks with unbounded recursion — so under `cargo install`
it panics, and it ships in a library any consumer links. That is a real defect
independent of the `clap` question, and it is what this item still owes.

*What is where.* `crates/lichen-language/Cargo.toml` carries one explicit
`[[bin]] lichen-compiler` (`src/main.rs`) **plus** an auto-discovered
`src/bin/sync-readme.rs`, and `src/lib.rs` lists `pub mod cli;` and
`pub mod readme;`. `clap` is a hard dependency and the whole crate's direct use
of it is `cli.rs`.

*Why the CLI cannot simply move into `main.rs`.* It is not only the binary's
entry: `crates/lichen-package/src/plugin.rs` generates a plugin compiler's
`main.rs` that calls `lichen_compiler::cli::main_with_native_packages::<crate::LangProgram>(…)`
(`plugin.rs:560`), so `cli` is a **public library API** every plugin-built
compiler depends on. Moving it into the binary would break them; feature-gating
it means the generated `Cargo.toml` must enable the feature (the generator's
`core_dep_line` emits `lichen-language = { path = … }` with no features today),
the crate's own `[[bin]]` needs `required-features`, and the release builds that
produce the prebuilt toolchain assets must pass it
(`.github/workflows/build.yml:35`, `release-lichen.yml:58`,
`scripts/venv-test.sh:111`). That is an ownership change across three crates and
the release pipeline, not a move inside this one.

*Why the README generator cannot be feature-gated off.* It is called from the
library's own tests: `src/tests/readme_tests.rs`, `tests/examples.rs`
(`readme::example_files`/`read_normalized`/`declared_output`/`program_output`)
and `tests/readme.rs` (`readme::render_examples`/`readme_path`/
`replace_examples`) — the case this item's instruction names. The mandated
verification runs `--test examples --test readme` without a feature flag, so
gating the module off would silently turn those tests vacuous rather than move
the generator. (`lichen-language-server` does depend on `lichen-language`,
`Cargo.toml:25`, so the "links clap for nothing" half of the finding is real.)

*What landed.* `logos` and `sha2` are removed from
`crates/lichen-language/Cargo.toml`; a search of the whole crate for either
name finds only the manifest lines — the crate's lexer is
`lichen-language-lex`'s, and its hashing is `lichen_utils::hash::sha256`
(re-exported at `persist.rs:43`). `Cargo.lock` drops the two edges and no
target changes behaviour.

*Verified but not acted on.* `readme::crate_dir()` is the compile-time
`CARGO_MANIFEST_DIR` (`readme.rs:49-51`), `example_dir()` walks two directories
up from it (`:55-57`), and `example_files` panics on a directory it cannot read
(`:77-78`) — so the `cargo install` panic is real for a caller that invokes the
generator, which is an argument for the move, not against it. What blocks the
move is that those callers are inside this crate's own test targets.

**Outcome — the remaining half; the item is closed.** The premise held, re-derived
first-hand at the then-current lines. The extent it names was checked: *two `..`
hops* (`crate_dir()` = the compile-time `CARGO_MANIFEST_DIR`, `example_dir()` and
`readme_path()` both `../../…`) is exact, and it is why the move kept working —
the tools crate sits at the same depth (`crates/lichen-tools`), so both paths
still resolve to the repository root and no path constant changed. *Executes
every example program* is exact: `sync_output_comments` runs
`program_output` over every file the walk yields. *Panics on a missing examples
dir* is exact (the walk's `fs::read_dir(..).unwrap_or_else(panic!)`), and
*follows symlinks with unbounded recursion* is exact in both walks — `walk`'s
`path.is_dir()` and `render_dir`'s entry classification both follow a directory
symlink, so one pointing at an ancestor recurses forever.

*The move.* The renderer is now `crates/lichen-tools/src/readme.rs`, the command
`crates/lichen-tools/src/bin/sync-readme.rs` (binary name unchanged, so the
README's command is `cargo run -p lichen-tools --bin sync-readme`), and the
crate is `lichen-tools` — package, library `lichen_tools`, its own workspace
member. `lichen-language` lost `pub mod readme;` and its only binary, so no
consumer of the library links the generator: the defect is the *library*
shipping it, and that is what changed. The unit tests moved with the module
(`crates/lichen-tools/src/tests/readme_tests.rs`) because they call private
items (`declared_order`, `replace_output_comment`, `render_examples_in`,
`DEFAULT_ORDER`) and the fixture tree moved to
`crates/lichen-tools/tests/fixtures/readme/` (the unit test's
`crate_dir()/tests/fixtures/readme`).

*The two tests that did **not** move, and why that is not a dodge.*
`tests/readme.rs` and `tests/examples.rs` stay in `lichen-language`'s test tree
and reach the generator through a **dev-dependency** on `lichen-tools`. A
dev-dependency is not linked by a consumer of the library, so the shipped
surface is unchanged; what it buys is that the mandated
`cargo test -p lichen-language --test examples --test readme` keeps naming
targets that exist and that still mean the same thing. The alternative — moving
both suites into `lichen-tools` — would break that command's target selection,
which is a worse outcome for a suite whose subject is the repository's examples,
not the compiler. There is no test here that *can only* live in the library: the
two integration tests use only public items, and the unit tests moved.

*The panic became a diagnostic.* The two tree walks and the three public drives
now return `ReadmeResult` (`Result<_, String>`, the module's own convention — the
one `replace_examples` already used) instead of panicking: a missing `examples/`
yields `read <path>: <io error>`, and `sync-readme` prints it and returns
`ExitCode::FAILURE`. Left as panics on purpose, because none is "the tool invoked
in the wrong place": `read_normalized` (a read of a file the walk just found, or
of the README itself), `program_output` (which *is* the diagnostic rendering for
a failing example, and what `tests/examples.rs` asserts on), and
`declared_order`'s non-numeric `order =` — the typo the sync command exists to
catch.

*The symlink recursion is bounded, not merely tolerated.* `MAX_DEPTH` (= 32,
`pub` like the markers) is checked at the entry of both walks and exceeding it is
an error naming the path, so a directory symlink cycle is a diagnostic instead of
a stack overflow; the bound is a report, never a silent truncation of the README.

*Tests.* `crates/lichen-tools/src/tests/readme_tests.rs` adds
`a_missing_example_directory_is_reported_not_a_panic`: it asserts
`render_examples_in` on an absent tree is an `Err` naming the path. Against the
unfixed panic path (temporarily restored for the check) it failed with
*"panicked at readme.rs: … read …\tests\fixtures\no-such-tree: … (os error 3)"*;
with the fix it passes. The other nine unit tests, and
`tests/examples.rs` and `tests/readme.rs` under `-p lichen-language`, pass
unchanged — the README was not rewritten, so the rendered section is byte-identical
to what the pre-move code produced. The depth bound is reasoned rather than
demonstrated: tripping it needs a 33-level tree, and no fixture can carry one
without a directory per level.

*One stale reference this left behind, now re-pointed.* `P2-12`'s Outcome ("What
did not move, and why") said the generator stayed and that `lichen-language`
"still has a binary"; both became false when this item landed. It was corrected in
the same `docs:` pass as the reference sweep below, on the same reasoning: a record
naming a file that no longer exists is stale, not historical.

### P2-7 — `visiting` is set by hand, bypassing the `Drop` guard `reported`

`lowlevel/src/evaluation.rs:557/570` and `:576/582` set and clear
`Node::visiting` directly inside the deep pass, while the crate goes to real
trouble to make the same flag unwind-safe elsewhere (`:94-110`, with
`impl Drop for VisitGuard` at `:27-31` and a comment explaining that this is what
keeps *"a future internal panic inside an attempt [costing] one node instead of
poisoning the module for the rest of the build"*). Any panic inside the descent —
`unreachable!("cycle detected")` (`:139`), `operands[1]` (`:166`),
`evaluate_block`'s `expect` (`:642`) — leaves `visiting = true` on the node and
every ancestor frame, permanently.

**Fix.** Wrap the descent in the same guard.

**Outcome.** The premise held with drifted lines and a bounded extent. Re-derived, `Node::visiting` is written in four places: the constructor's initialiser (`lib.rs`), `retain_node` — which already went through `VisitGuard` — and exactly **two** hand-written pairs in `evaluate_node_deep_inner`, the array descent (`:636`/`:653`) and the table descent (`:659`/`:669`); the ledger's `:557/570` and `:576/582` are those two, moved. The readers are the deep pass's structural-cycle cut (`:584`) and the attempt path's cycle panic (`:146`). So "several sites" resolves to two pairs.

Both descents now take the mark through `retain_node` and run their loop inside `VisitGuard::run`, which routes the body through the guard so only `Drop` releases the mark. `run` became generic in its result (`R = ()` for a descent, `P::Value` for an attempt) rather than gaining a second method; the guard is a stack value, so no allocation is added on the hot path. Nothing moved in time: the mark is still taken after `evaluate_node` returns, the home block is still read after the mark, and the mark is still released after the last descent call — `Drop` is now what releases it, so an unwind inside the descent costs one node's mark instead of leaving it set, which is the recorded failure mode (a node stuck `visiting` *with* a cached value silently takes the structural-cycle cut on every later evaluation).

**No test; the panic path is reasoned, not demonstrated.** Reaching an abandoned mark needs an unwind, and this harness can only provoke one through `#[should_panic]` (`cyclic_operations_panic_instead_of_looping`), which cannot inspect the module afterwards; asserting `!visiting` after the unwind would take `catch_unwind` over a `&mut Module`. The normal path is already pinned by `tests/basic/evaluation.rs`'s `visiting_markers_are_cleared_after_evaluation` and the `!n.visiting` assertion in the self-referential-value test, and the lowlevel suite (139 tests) passes unchanged.

### P2-8 — `missing_slots[order_index()]` guarded only in debug `reported`

`highlevel/src/checker/annotations.rs:149-153` indexes `missing_slots`, sized
from `P::Attr::ORDER.len()` (`checker.rs:536`), with a plugin-controlled index.
`AttrSet` is a public trait a downstream implements, and the only check is a
`debug_assert!` (`checker.rs:480-483`). `attr.rs:74-76` acknowledges the
invariant is debug-only. A hand-written set that forgets it compiles and panics
in release.

**Fix.** `.get(index).copied()` plus a recorded guard.

**Outcome.** The premise held, re-read first-hand at the now-current lines
(the ledger's have drifted): the index came from `AttrSet::order_index`
(`annotations.rs`, `missing_slot_of`'s read and its write), a public trait
method a downstream set supplies, while the vector is sized
`P::Attr::ORDER.len()` (`checker.rs`'s `Checker` construction) and the only
thing making the two agree is `debug_assert!(order_is_canonical::<P::Attr>())`
in `build_with` (`checker.rs:548-551`) — compiled out in release, where an
out-of-range index panics and an in-range-but-wrong one silently aliases
another attribute's cached slot.

**Chosen: the correct-by-construction key, not the checked access.** The cache
is now keyed by the marker's **position in `P::Attr::ORDER`** — the same list
that sizes the vector — found with
`P::Attr::ORDER.iter().position(|attr| attr == marker)`, so the index cannot
leave the vector and cannot name another attribute's slot; `order_index()` is no
longer consulted on this path at all. For a canonical set the position *is* the
order index, which is exactly what `order_is_canonical` asserts, so no behaviour
moves and the highlevel suite is unchanged. A marker absent from the set's own
list takes the crate's malformed-input convention: `no_attr_ext_guard` records
the guard and returns the well-formed `[value, type]` hole, so the build fails
with a diagnostic instead of panicking on a path the checker reads to decide
whether a slot is missing. The ledger's `.get(index).copied()` was rejected as
the *only* change because it makes the bound real but leaves the finding's
"worse" half standing: an index that is in range yet disagrees with `ORDER`
would still be accepted, and would still alias the wrong slot.
`missing_slots`' own doc now names the position in `ORDER` as its key.

**No test.** The harness cannot observe this in a debug build: a malformed
`AttrSet` panics at `order_is_canonical`'s `debug_assert!` in `build_with`
before `missing_slot_of` is ever reached, so the fixed path is reachable only in
a release build; pinning it would take a `--release` test over a deliberately
non-canonical set. For every canonical set — all the suite uses — the change is
a bound that no longer needs asserting, with no behaviour to pin.

### P2-9 — `no_attr_ext` panics on any annotated program `reported`

`highlevel/src/checker.rs:465-469` installs an `unreachable!` closure as the
attribute registry, used by the public `build` (`:422`) and `build_in` (`:430`).
Any program whose schema carries an attribute — i.e. any host that plugs in
`Perspective` and calls `Checker::build` instead of `build_in_attr` — panics
mid-check instead of getting a diagnostic. The crate already has the right
pattern a few dozen lines away (`NativeOpUnresolved`, `:1335-1341`).
`build_in_attr` and `build_in_attr_native` have **zero** test coverage.

**Fix.** Record a guard and return a hole.

**Outcome.** The premise held, and the panic was reproduced first-hand before
the fix: a hand-built program whose annotation schema carries an attribute,
checked through `Checker::build`, aborted with *"panicked at
crates\lichen-highlevel\src\checker.rs:529:13: internal error: entered
unreachable code: this program has no attribute extension"*.  `no_attr_ext`
installed that closure and both `build` and `build_in` passed it, so the
public "simplest" entry point panicked mid-check on any schema carrying an
attribute — a host that composes an attribute set and forgets `build_in_attr`
got a panic instead of a diagnostic.

*One correction to the finding:* `build_in_attr_native` is not uncovered — it
is `lichen-language`'s only entry point (`lib.rs:228`), so every language-layer
test exercises it.  *Direct* coverage was absent, and it is no longer: the new
tests call both it and `build_in_attr` on an annotated program.

The registry is now `Option<Box<dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>>>`
(`attr_ext`); `build` and `build_in` pass `None`, and the `unreachable!`
closure is gone.  A marker that cannot be resolved is a check-time refusal:
`Checker::no_attr_ext_guard` records `DiagKind::NoAttributeExtension` — the new
kind, rendered by `render::checker_message` as *"this expression carries an
attribute, but this build has no attribute extension to lower it"* — and
returns the same well-formed hole the other check-time guards leave: a fresh
unbound `[value, type]` pair, the shape every attribute slot has, so the
runtime pair keeps the arity its schema declared and no reader meets an absent
element.  Nothing unifies against the hole (the guard has already failed the
build, so `check_failed` skips the definition pass), and the guard is recorded
**once per build**, at the first site that read the attribute — `check_ann`'s
per-marker loop, the `x # n` parameter annotation in `check_lam`, or
`missing_slot_of` when a missing slot is needed — because the fact reported is
a property of the build rather than of one expression; the apply site then
skips its attribute unify instead of reporting it a second time.
`missing_slot_of` gained the `Loc` it attributes with (`attr_or_missing` and its
two `check_ann` callers pass the expression's attribute slot).  No public
signature changed: `build_in_attr` and `build_in_attr_native` still take the
boxed closure and wrap it in `Some`.

**Tests.** `crates/lichen-highlevel/tests/attributes.rs` (new) defines a probe
attribute (`Tag`), its `AttrExt`, and a `TaggedProgram`, and pins five facts:
the annotated program reports the new kind at the annotation and fails; the
pair keeps its three-wide `[value, type, attribute]` arity with the hole in the
slot; an annotated *parameter* is reported exactly once; an unannotated program
through the same entry point is unaffected; and `build_in_attr` with the
extension installed lowers the annotation with no diagnostic.  The two panic
cases were confirmed to abort against the unfixed sources with the message
above; the highlevel, `lichen-language` (including `pipeline`, `registry`,
`examples`, `compute`), `lichen-perspective` and `lichen-doc` suites pass after
the fix.

### P2-10 — `check_term` recursion is unbounded `reported`

`highlevel/src/checker.rs:1069, 1084` recurses once per nested expression with
no depth counter. `stacksafe` is declared in `lichen-highlevel`'s `Cargo.toml`
but **never imported by the crate** (it is used only in `lichen-lowlevel` and
`lichen-utils`). A generated file with 10^5 nested parentheses overflows the
native stack before any budget fires. The crate is aware of the constraint for
the *runtime* (`checker.rs:484-490`) but not for the checker itself.

**Fix.** `#[stacksafe]` on `check_term` (or an explicit depth counter that
records a diagnostic) — the dependency is already there.

**Outcome.** The premise held: `check_expr` forwards to `check_term`, the
per-kind rules call `check_expr` back, and nothing on that cycle counted depth;
and `stacksafe` was a `[dependencies]` entry of `lichen-highlevel` that no
source file imported (a workspace grep for the import found only
`lichen-lowlevel` and `lichen-utils`).  Measured first-hand by reverting only
the source change: a hand-built IR nesting a literal in 200_000 annotations,
compiled through `Checker::build`, aborted the test binary with *"has
overflowed its stack"* (`STATUS_STACK_OVERFLOW`, `0xc00000fd`) — an abort, not
a failed assertion.

The fix is `#[stacksafe]` on `check_term`, as the lowlevel annotates its own
recursive entry points.  The macro's requirements are a plain (non-`async`,
non-`const`) function and a non-`impl Trait` return; this method's `&mut self`
receiver and `-> NodeId` satisfy both, so it applies cleanly and the depth
counter the item offered as a fallback was not needed.  The attribute wraps the
body in `stacker::maybe_grow`, so the recursion allocates a fresh 2 MiB stack
segment when less than 128 KiB remains.  There is therefore **no limit to trip
and no diagnostic to assert** — the mechanism is stack growth, not a guard —
which is why the test pins the success case only.  The same 200_000-deep build
now checks, in 0.75 s, and the crate exercises its `stacksafe` dependency
instead of merely declaring it.

**The file-level consequence the finding states is real, but it is not this
recursion's — corrected here.**  Post-fix (with `check_term` guarded), a source
file of **250 nested parentheses** — about 500 bytes — still aborts the same
way: 250 fails, 150 succeeds, on the `lichen-language` binary.  So the pass
that overflows first from source is one of the frontend's own unguarded
recursions, not the checker's: the parser's grammar recursion (which is why it
gets a 16 MiB thread, `language-parser/src/parse.rs:94-103`) and the language
layer's recursive walks (`compile.rs:379`'s `compile_expr` calls itself 42
times, and `resolve.rs`'s and `analysis.rs`'s three walks are `P2-2`'s) all run
with no `#[stacksafe]` and no depth guard.  Which of them is hit first was not
isolated.  `P2-10`'s defect is therefore reachable through the IR-level public
API — a host that generates and compiles an IR, which is what `Checker::build`
is for — while a *source*-level repro of this particular recursion cannot be
built, because the frontend dies first at roughly four orders of magnitude less
nesting.  That source-level gap is a new finding of the same class, adjacent to
`P2-2`/`P4-3`; it is reported, not fixed, and is not this item.

**Test.** `crates/lichen-highlevel/tests/checker.rs`'s
`a_deeply_nested_program_is_checked_without_an_overflow` builds
`DEEP_NESTING` (200_000) nested annotations and requires the build to check.
Against the unfixed sources it aborts the test binary with the message above
instead of failing an assertion — a stack overflow is a process abort, so the
pre-fix observation is an abort rather than a red test, and no test can pin
that side; after the fix it passes.

### P2-11 — God files with named seams

| file | lines | seam |
|---|---|---|
| `language-server/src/analysis.rs` | 2566 | pipeline / snapshot / imports / resolve / scope / diagnostics / hover / completion / semantic tokens; the highest-value cut is merging the three walks (`P2-2`) |
| `compute/src/compute.rs` | 2182 | registry / vocab / vm / wasm_emit / codegen / run / ops |
| `highlevel/src/checker.rs` | 1502 | one `impl` block of 1090 lines; six concrete check rules still inline while their siblings already live in `checker/` |
| `lowlevel/src/lib.rs` | 1179 | 145 `pub` items, ≥7 responsibilities: handle (all the `unsafe`), registry, module, vocabulary |
| `lowlevel/src/static_module.rs` | 848 | `impl Module` block mixes importer-apply, generic node helpers (`node_shape`, `materialize_leaf`, `as_dynamic` — nothing to do with static modules) and freeze |
| `render/src/render.rs` | 1168 | `TypePrinter` (490) / `ValuePrinter` (340) / struct-kind helpers (8 free fns) |
| `language/src/compile.rs` | 952 | `compile_expr` is a **single 453-line function** (`:379-832`); 9 `alloc_*` wrappers are pure boilerplate with an unguarded invariant |
| `language/src/resolve.rs` | 706 | scope machine (1-379) + content-key serializer (**385-706 = 45%**) |
| `language/src/package.rs` | 916 | load pipeline / native virtual-package registration (`compute.lichen` is hard-coded at `:230`) / vendored path resolution / 70 lines of inline tests |
| `language/src/persist.rs` | 650 | codec traits + container / cache-root resolver / inline tests |
| `language-parser/src/parse.rs` | 1584 | thread driver / token utils / statement grammar / precedence ladder / atoms+postfix / type constructors / AST walk / diagnostics |

### P2-12 — `clap` is linked by every consumer of the compiler library `verified`

Split out of `P2-6`, whose ownership move stopped here. `clap` is a hard
dependency of `lichen-language` and the entire crate's use of it is `cli.rs`, so
`lichen-language-server` — and every embedder — links a command-line parser it
never calls.

It is not a local move, which is why it is not folded back into `P2-6`:
`crates/lichen-package/src/plugin.rs` generates a plugin compiler's `main.rs`
that calls `lichen_compiler::cli::main_with_native_packages::<crate::LangProgram>(…)`,
so `cli` is a **public library API with a caller this repository generates at
build time**. Any answer has to move three things together: the generated plugin
manifest (`core_dep_line` emits no features today), `[[bin]] required-features`
for the compiler binary, and the release workflows that build it.

**Resolved by `D10`: its own crate.** The feature route was considered and
rejected there; see that entry for what the move must carry with it.

**Outcome.** The CLI is a workspace member of its own, **`lichen-compiler`**
(`crates/lichen-compiler`; package and binary `lichen-compiler`, library
`lichen_compiler`).  `cli.rs` moved there as `src/cli.rs` — the module path
survives, so a plugin's generated `main` names `lichen_compiler::cli::…` — and
`src/main.rs` moved with it; the `clap` dependency, `default-run` and the
`[[bin]]` target left `lichen-language`, which keeps no `pub mod cli;`.  The
crate graph gained exactly one edge, `lichen-compiler → lichen-language`:
`cargo tree` shows `clap` gone from both `lichen-language` and
`lichen-language-server`, and `Cargo.lock` lists neither `clap` nor
`lichen-compiler` among `lichen-language`'s dependencies.

*No library item had to be widened.*  Every item `cli.rs` calls was already
public — `LangProgramShape`, `package::PackageStore`
(`new`/`with_cache_dir`/`register_native`/`load_package`),
`persist::{ArtifactCodec, shipping_cache_root}`, `preprocess::stage_depends`,
`program::GcdOp`, `diag::{Diag, Stage}`, `render::render_all`,
`run::evaluate_raw` — so the argument parsing moved and no piece of the
pipeline did.

**The callers that changed.**  ① The compiler binary's target: `[[bin]]` and
`default-run` moved to the new manifest, and `src/main.rs` now calls
`lichen_compiler::cli::main::<lichen_language::program::LangProgram>()`.
② The generated plugin compiler's source: `write_compiler_main_rs` emits
`lichen_compiler::cli::main_with_native_packages::<crate::LangProgram>(…)` in
place of the `lichen_language::cli::…` call it used to emit.  ③ The generated
manifest:
`rebuild` passes `core_dep_line(core_repo, "lichen-compiler")` through
`write_cargo_toml`'s `extra_deps` — the interpolation site `server_dep` already
used — so a generated *compiler* gains
`"lichen-compiler" = { path = "<core_repo>/crates/lichen-compiler" }` (or the
git form) and a generated *server* does not, which is the asymmetry the split
exists for.  The build callers followed: `-p lichen-language` became
`-p lichen-compiler` in `.github/workflows/build.yml`,
`.github/workflows/release-lichen.yml` and `scripts/venv-test.sh`, with nothing
else about those steps changed.

**Tests.**  The only suites that reached the CLI were the generated-text pins,
and they pin the new text: `plugin_manifest_tests.rs` asserts the compiler
manifest carries `lichen-compiler` (and the language-server manifest does not),
and `generated_main_tests` pins
`lichen_compiler::cli::main_with_native_packages::<crate::LangProgram>(`.  No
other suite named `cli::`.  The flag surface, help text, exit codes and
behaviour are unchanged: the files moved, nothing in them was tidied.  The
shipped binary was run end to end after the move (`--help`, `--version`,
`examples/array.lichen` → `[1, 2, 3]: array<Int, 3>`,
`examples/recursion.lichen` → `55: Int`), and a hand-written crate with a
generated compiler's shape (depending on `lichen-compiler` by path and calling
`main_with_native_packages`) `cargo check`s — so the generated manifest's line
and the generated call both resolve.

**What did not move at the time, and where it went.**  The README generator and
`sync-readme` (`readme.rs`, `src/bin/sync-readme.rs`) stayed here because they are
`P2-6`'s, which is why `lichen-language` still had a binary after this commit.
`P2-6` has since moved both to a tools crate, so the library now has **no** binary
at all.  The library half (`package.rs`, `persist.rs`, `run.rs`, `render.rs`,
`preprocess`, `program`) stayed because the CLI only calls it; `clap` left the
library's manifest with no replacement.

**Residual references.**  This Outcome originally left two sections naming the old
path — `P1-16`'s Outcome (`crates/lichen-language/src/cli.rs:232`) and `P2-6`'s
text (`lichen_language::cli::main_with_native_packages`) — on the grounds that each
is its own item's record. That was the wrong call: a record that names a file or a
command which no longer exists is stale, not historical, and a later reader cannot
tell the two apart. Both were re-pointed after this commit, along with the two
`cargo run -p lichen-language --` transcripts in `P1-21` and `P1-22` (that package
has no runnable binary now). A closed plan's methodology in
`docs/notes/type-system-cleanup-plan.md` (`cargo run -p lichen-language --bin
lichen-compiler`) is genuinely historical and was left. Every live reference was
re-pointed: `README.md`, `crates/lichen-package/{README.md,src/lib.rs}` and
`docs/notes/{artifact-cache,language-toolchain,plugin-taxonomy,venv-test,package-manager}.md`.

### P2-13 — Node state is still writable through the node table and `disjoint::Meta` `verified`

Found while implementing `D11`, and recorded rather than quietly closed: `P2-4`
privatised the six fields, but two routes around them survive, and both are
outside the six-field scope `D11` set.

1. `Module::nodes` is `pub` and is a `SlotMap` of `Node<P>`, so an external crate
   can still obtain `&mut Node<P>` from it.
2. `crates/lichen-utils`' `impl disjoint::Node for Node<P>` exposes
   `meta_mut() -> &mut Meta<NodeId>`, and **`Meta`'s fields are public** — so
   `module.nodes[id].meta_mut().parent = Some(other)` writes the union-find link
   from outside, which is exactly the write `P2-4` routed through `add_equality`
   and declared to have no other legitimate writer.

**Fix.** One of two shapes, and the choice depends on how the frozen mirror is
built: make `Meta` **opaque** (private fields, read accessors, plus a constructor
for the mirror `static_module.rs` assembles at load), or make the node table
itself private behind the accessors `P2-4` just added. Making the table private is
the larger change — 184 `.nodes` uses live outside `lichen-lowlevel/src` across
five crates — so opacity for `Meta` is the narrower first step, and the container
question is worth its own decision if it comes to that.

**Why this is not a `P2-4` footnote:** `D11`'s whole point was that a public field
is an invitation rather than a contract, and leaving a public `Meta` means the
invitation is still open — one indirection away. A reader who checks `Node`'s
fields, finds them private, and concludes the write is impossible would be wrong.

## P3 — refactor

### P3-1 — Duplication clusters

Each of these is two or more sites that must change together:

- **Codec array/table arms** are 20-line near-clones
  (`lowlevel/src/codec.rs:163-185` vs `:206-228`, and the write halves at
  `:111-132`). Merge while fixing `P0-1`.
- **`restatic` rewrite appears three times verbatim**
  (`static_module.rs:752-761`, `:788-797`, `:798-804`).
- **`concrete` predicate written five times** (`structs.rs:295-304` extracts it,
  then re-inlines at `:45-53`, `:114-122`, `:192-200`, and `lambda.rs:237-245`).
- **`nstart`/`name_range` block three times** (`ir.rs:669-674`, `:683-688`, and
  the `ChildRange` push repeated at seven sites).
- **`regroup_clones` was extracted and then its *call* was copy-pasted**
  (`apply.rs:173-186` is the helper; `function.rs:172-184` and
  `static_module.rs:159-173` repeat the 13-line tail).
- **Assert instantiation four times** (`function.rs:151-159`, `:476-485`,
  `static_module.rs:144-153`, `:345-355`).
- **Atomic write three ways, three policies** (`device.rs:137-140` locked with
  errors swallowed; `device.rs:228-231` unlocked, fixed name; `toolchain.rs:267-281`
  errors returned). One `write_atomic(dest, bytes)` fixes `P1-11` too.
- **Subprocess invoke + status + stderr→String six times** (`git.rs:60,107,124`,
  `toolchain.rs:188,268`, `plugin.rs:124,171,194`); **tool-on-PATH probe twice**;
  **`.exe` suffix three times**.
- **Compile pipeline written five times and diverging**
  (`lib.rs:163`, `run.rs:88`, `package.rs:301`, `:503`, `:676`) — see `P1-14`.
- **"evaluate → freeze → export → publish meta" three times in one file**
  (`package.rs:320-349`, `:517-538`, `:695-730`), including the duplicated
  comment at `:516` and `:694`.
- **Output-line formatter twice** (`run.rs:45-60` ≡ `:103-118`).
- **`Diagnostic` construction boilerplate at seven sites**
  (`highlevel/src/diagnostic.rs`); each lists all ten fields and sets seven to
  `None`. A `Diag::factual(kind, loc)` constructor shrinks it and makes adding a
  field a one-line change.
- **List combinator written five times** in `parse.rs:850-903`, `:912-1077`.
- **`rebuild` vs `rebuild_lsp`** 40 lines differing in a name and a main-file
  writer (`package/src/plugin.rs:103-140` vs `:148-190`).
- **Positional-read and raw-read pairs**: `structs.rs:66-83` ≡ `:244-259`;
  `structs.rs:152-165` ≡ `indexing.rs:109-120`.
- **`preprocess()` re-inlines `depend_of`'s normalization** instead of calling it
  (`preprocess/src/lib.rs:272-308` vs `:382`), so a new `Depend` field can be
  dropped on one path.
- **`Disjoint` parent walk forked** in `render.rs:932-941` — no cycle guard, no
  `#[stacksafe]`, duplicating `utils/disjoint.rs:66-79` (`P5-10`).

**Outcome.** The list was walked entry by entry against the current tree,
re-deriving every line number and every extent: they had drifted, one entry
names a function that no longer exists, and one is refuted on its merits rather
than merged.  Of the seventeen entries, **eight merged outright, three partly,
three are refuted and three are left**, each with its reason below.

*Merged — one implementation, the other sites calling it:*

- **Codec array/table arms.**  The read halves were already one
  (`relocated_handle`, `P0-1`), but the owner lookup and both *write* halves
  were still two copies.  `codec.rs:155` `read_relocated_handle` is now the one
  read and `codec.rs:179` `write_relocated_handle` the one write, so the two
  `LowValue` arms are a tag byte plus a call and cannot drift.
- **`ChildRange` push, seven sites** (`highlevel/src/ir.rs`), in three
  spellings; one of them built a temporary `names` vector only to copy it.
  `ir.rs:96` `extend_range(arena, values)` is the one append-and-range, and
  `alloc_type_struct`'s temporary is gone.
- **`concrete` predicate.**  `type_is_concrete` (`structs.rs:259`) was re-inlined
  verbatim at **four** sites — `structs.rs:45`, `:122`, `:181` and
  `lambda.rs:244`.  So the note's "five times" counts the extraction itself plus
  four inlines, which is right; all four call the helper now (`pub(super)` for
  `lambda.rs`).  The existing call at `structs.rs:347` was already correct.
- **Positional-read pairs.**  Both pairs the note names are real.
  `structs.rs:72` `slot_read` is the shared shape-slot read (`check_field`,
  `check_named_field`), and `indexing.rs:121` `element_read` the shared
  value-pair read (`check_raw_named_field`, `check_raw_index`).  Each builds the
  `Index` nodes in the same order the two sites did, so node ids are unchanged.
- **`regroup_clones` call.**  The 13-line group-and-unify tail was identical in
  `function.rs:173` and `static_module.rs:160` apart from the representative
  function.  `apply.rs:197` `unify_clone_groups` is the unification policy; each
  caller still picks its own representative, because a live `disjoint::find` and
  a `static_find` are different union-find structures.
- **`.exe` suffix, three sites.**  `plugin.rs:199`/`:204` now call
  `toolchain.rs:112`'s `exe_suffix` (which became `pub(crate)`).
- **Tool-on-PATH probe, two sites.**  `git_available` and `cargo_available` were
  the same `--version`-and-status probe; `lib.rs:38` `tool_available` is the
  probe and the two keep only their own error wording.
- **The identical `cargo build --release` body** in `rebuild` and `rebuild_lsp`
  is now `plugin.rs:178` `cargo_build`, error text unchanged.
- **`depend_of` normalization.**  `preprocess` re-inlined both `Depend`/`Plug`
  arms that `depend_of` (`preprocess/src/lib.rs:409`) already normalizes; the
  loop's fallthrough arm now calls it, so a new `Depend` field cannot be
  dropped on this path.
- **Diagnostic construction.**  `highlevel/src/diagnostic.rs` spells the struct
  out at **12** sites, eight of which set every field but `kind`/`loc` to empty.
  `diagnostic.rs:245` `Diag::factual(kind, loc)` plus struct-update syntax
  covers them; `unattributed_failure` is one call to it.
- **List combinator.**  `parse.rs:851` `comma_list` is the shared
  comma-separated list.  The note says five; the file has **five** such
  parsers, three of which are the same combinator (`struct_inst_fields`,
  `paren_fields`, `table_literal`) and now call it.
- **Output-line formatter.**  `evaluate` and `evaluate_raw` repeated the
  deep-evaluate-and-render block verbatim; `run.rs:33` `render_build` is it, and
  the two keep only how they obtain the build.

*Left, with the reason:*

- **`restatic` rewrite three times — refuted, stale.**  There is no `restatic`
  anywhere in the tree, and `git log -S restatic` across the whole repository
  history finds only the commit that wrote this note: the name was never code.
  The three cited ranges now fall inside `rewrite_value`'s per-payload arms,
  which are typed — the array arm rewrites `ArrayItem::node`, the table arm
  rewrites `TableItem::key` *and* `.value`, and the ext arm rewrites a third
  payload kind with its own handle protocol.
- **Atomic write three ways — refuted, three contracts.**  The three are not one
  helper with three policies; they are three owners with three error contracts.
  `Registry::save` (`registry/src/device.rs:212`) runs **under the registry
  lock**, which is exactly what makes its fixed temp name safe, and swallows
  errors because `reload` recovers the index.  `store_artifact`
  (`device.rs:317`) is **deliberately outside** that lock, so it needs a unique
  temp name and an `fsync`, and swallows errors because the artifact cache
  degrades to a miss and a recompile.  `download`
  (`package/src/toolchain.rs:281`) must **return** its error — a failed install
  cannot fail silently — and chmods the result.  Their `fsync` helpers are not
  interchangeable either: `device.rs:419` `write_synced` creates and writes the
  bytes, `toolchain.rs:312` `flush_to_disk` opens a file `curl` already wrote.
  Merging them would move a registry-lock invariant and an install policy into
  one crate that owns neither.
- **Assert instantiation four times — left.**  The four sites share the
  `PendingAssert { condition, template }` construction, not a body.  The two
  dynamic sites (`function.rs:154`, `:488`) decide "was it instantiated per
  call" from `instantiated != condition` and name the template `Dyn(condition)`;
  the two static sites (`static_module.rs:148`, `:348`) decide it from the
  template node's own `parameterized` flag, name it `static_ref`, and read nodes
  from a `StaticModule` rather than the live `Module`; two of the four also
  collect the clone list.  One helper would take three axes as parameters and
  still not remove the per-site decision, so the note's "merge" is not obviously
  a reduction here.
- **Compile pipeline five times / "evaluate → freeze → export → publish meta"
  three times — left as a redesign.**  The three sites in
  `language/src/package.rs` (`:315`/`:344`/`:352`, `:519`/`:541`/`:549`,
  `:684`/`:713`/`:721`) diverge in error type (`Vec<String>` vs `Vec<Diag>`),
  which key they allocate, what they hash, and what bookkeeping follows the
  freeze (a store insert, a dependency record, a disk cache write).  Folding
  them needs a parameter object and a generic error, which is the redesign this
  item's scope discipline defers rather than a merge.
- **`rebuild` vs `rebuild_lsp` — partly merged.**  The identical 11-line cargo
  invocation is `cargo_build` now.  What remains differs in the generated crate
  prefix, the dependency line, the main-file writer, the binary-name function
  and the return type; folding *that* into one function is the same
  parameter-object rewrite, and `P3-2` owns neither.
- **`Disjoint` parent walk forked in `render.rs` — not a duplicate.**
  `disjoint::find` (`utils/disjoint.rs:66`) takes `&mut SlotMap` and compresses
  paths; `render::representative` (`render.rs:982`) is a read-only walk over
  `&Module` through the `node_equality` accessor, with a revisit bound.  The
  renderer holds no `&mut Module`, and `P5-10` already added the guard, so the
  fork the note names is gone and what is left cannot call the shared one.

*One intentional non-merge inside a merged cluster.*  `paren` and
`array_literal` are the other two comma-list parsers and were **left** calling
their own shape: both require a first element, so the empty `()` and `[]` do not
parse today, and `comma_list` accepts an empty list.  Pointing them at it would
silently make `()` a value, which is a grammar decision, not a refactor.

*Overlap with `P2-2`.*  None of the merges above touches the five AST traversals
`P2-2` owns; `resolve.rs`'s and `compile.rs`'s walk functions are untouched.

*Gates.*  `cargo clippy --workspace --all-targets -- -D warnings` exits 0,
`cargo fmt --all -- --check` exits 0, and `cargo test --workspace` passes — the
edits reach `lowlevel`, `highlevel`, `language`, `language-parser`,
`preprocess` and `package`, so the influenced set is effectively the workspace.

### P3-2 — Workspace manifest duplication

No `[workspace.package]`, `[workspace.dependencies]` or `[workspace.lints]`.
`version = "0.1.0"` and `edition = "2024"` are repeated in **every** manifest
(17 workspace members plus the two standalone crates), and each `lichen-*` path
dependency is spelled out at each use site. A release bump means editing ~14
files by hand.

**Fix.** `[workspace.package] version/edition`, `[workspace.dependencies]` for
the shared externals and the internal path deps, `version.workspace = true` per
crate.

**Outcome.** The premise held, but its **extent is wrong in one direction**:
re-derived from `cargo metadata --no-deps`, this workspace has **19 members**,
not 17, and **one** excluded standalone crate (`tree-sitter-lichen`), not two —
so `version`/`edition` were stated in **19** member manifests and a release bump
edited 19 files, not ~14.  Everything landed in the root `Cargo.toml`:

- `[workspace.package]` carries `version = "0.1.0"` and `edition = "2024"`;
  every member writes `version.workspace = true` / `edition.workspace = true`.
- `[workspace.dependencies]` carries the externals named by more than one member
  (`clap`, `logos`, `serde`, `serde_json`, `sha2`, `slotmap`, `stacksafe`) and
  all the internal crates used as path deps, so each path is written once; the
  members inherit with `<name>.workspace = true`.

*Deliberately left inline, and why:*

- **`lichen-std-native`'s three git dependencies.**  They must stay *git* deps —
  that is the whole point of the crate (an externally generated compositor
  resolves the core subtree from git) — and the root `[patch]` redirects them to
  the local path members for the monorepo build.  Inheriting a path entry would
  change their source and the resolution.
- **`tree-sitter-lichen`'s path dependency** (in `lichen-language-zed`).  It
  points at a crate the root deliberately *excludes*, so it must not become a
  workspace-level path entry; and the grammar crate is its own workspace root
  (`[workspace]` in its manifest), so it cannot inherit the table either.
- **Single-use externals** (`wasmi`, `wasm-encoder`, `chumsky`, `bumpalo`,
  `lsp-types`, `tokio`, `tower-lsp`, `toml`, `zed_extension_api`, `tree-sitter`)
  stay in the one manifest that uses them: a version stated once is already
  stated once.

*Scope discipline, measured.*  No dependency's version changed, none was added
or removed, and the `members` list is untouched; only the two new tables and the
`workspace = true` inheritances changed.  **`git diff Cargo.lock` is empty** — no
resolution moved.

*The forward reference is dropped.*  The item's own fix line ended "Then `P3-3`
can hang lints off the same table", expecting a `[workspace.lints]` table.  No
such table was added: `D12` replaced it with the three-command gate, so the
manifest tables here hold packages and dependencies only (see `P3-3`'s
Outcome).

### P3-3 — No test/clippy/fmt gate in CI

`.github/workflows/build.yml` only runs `cargo build --release --locked`. No
`cargo test`, no `clippy`, no `fmt --check`, and there is no lint policy
anywhere (`cargo clippy --workspace --all-targets` currently reports **56
warnings**). `dev` has no quality gate, which is why the warnings are there.

**Fix.** Clean the 56 warnings, add `[workspace.lints]`, and add a CI job
running `fmt --check` + `clippy -D warnings` + `test`. Note the baseline: the
test suite compiles (`cargo test --workspace --no-run` succeeded) and the suite
is 15.1k lines against 31.5k of source.

**Outcome.** `D12`'s order held: the backlog was cleared first, then the gate
landed, so the gate has never run red. The inventory measured on this revision
is **65 distinct warning locations** — the `66` in `D12` (and in `P2-4`'s
Outcome above) and the `56` in this item's own text are all stale counts. It was
28 `collapsible_if`, 8 `type_complexity`, 8 `needless_borrow`, 5
`arc_with_non_send_sync`, 3 `needless_range_loop`, 2 `get(0)` → `first()`, and
one each of `map(..).flatten()`, `unnecessary_cast`, `explicit_auto_deref`,
`manual_is_multiple_of`, `len_zero`, `redundant_redefinition`, `question_mark`,
`extra_unused_lifetimes`, `expect_fun_call` and `single_match`. Those rows sum to
64, not 65: the location the breakdown above omits is
`lowlevel/tests/basic/main.rs`'s `iter().copied().collect()`, which `to_vec()`
replaces.

**What the tool did and what was done by hand.** `cargo clippy --fix
--allow-dirty --workspace --all-targets` cleared 46 of the 65: all 28
`collapsible_if` (edition 2024's `if a && let Some(b) = c {}` absorbs each), all
8 `needless_borrow`, both `get(0)`, and the `map(..).flatten()`,
`unnecessary_cast`, `explicit_auto_deref`, `manual_is_multiple_of`, `len_zero`,
`expect_fun_call`, `single_match` and test-only `to_vec()` singletons. **Every
one of those rewrites was read back before it was kept**: each is the exact
desugaring of the form it replaces (a nested `if let` whose body is the only
statement, a `match` whose wildcard arm is empty, `Option::expect` on an
`Option` — where `expect` panics with the bare message, so
`unwrap_or_else(|| panic!(..))` is the same panic), and the workspace suite is
the check rather than the fix tool's confidence. Nothing had to be rejected.
The remaining 19 were hand-written: the 8 `type_complexity` and 5
`arc_with_non_send_sync` below, the 3 `needless_range_loop`
(`checker/structs.rs`'s missing-field scan and `suggest.rs`'s two
distance-matrix initializations), `shape.rs`'s `let…else` → `?`, `session.rs`'s
deleted redundant `ranges` rebinding, and `lsp_smoke.rs`'s unused `<'a>`.

**`type_complexity` — named types, no allows.** `AttrExtRegistry<P, Attr>`
(`crates/lichen-highlevel/src/attr.rs`) is
`Box<dyn Fn(&Attr) -> &'static dyn AttrExt<P>>`, the attribute-extension
registry the checker's field, `build_in_attr`, `build_in_attr_native`,
`build_with`, `lichen-perspective`'s `persp_attr_ext`, `lichen-doc`'s
`doc_attr_ext` and `tests/attributes.rs` all spelled out — 6 of the 8 locations,
and the only alias that had to cross a crate boundary.
`LocatedDirective` and `ParseFailure`
(`crates/lichen-preprocess/src/parse.rs`) name the two anonymous halves of that
parser's return type; the three sibling signatures that spelled the same
`Vec<(u32, String)>` were switched to the name as well, since leaving them
spelled out beside the alias would be the same type written twice.

**`arc_with_non_send_sync` — five sites, four of them keeping the `Arc`.** All
five are tests constructing `Arc<RwLock<Registry<…>>>`; the lint fires because
`Handle` holds a raw pointer, so the registry is neither `Send` nor `Sync`. The
invariant they cite is the one `P5-3` already put in `Registry`'s own doc: a
filed value carries arena handles, so the sharing is *within one thread, never
across threads*. `Rc` is not available — `AGENTS.md`'s code taste forbids it —
so a **targeted** `#[allow(clippy::arc_with_non_send_sync)]` stays at the four
sites where the `Arc` is what the API demands: `tests/attributes.rs` and
`tests/native.rs` pass it **by value** to `Checker::build_in_attr` /
`build_in_attr_native`, and `tests/basic/static_module.rs`'s `frozen_dependency`
and `static_apply_keeps_foreign_items_in_place` pass it **by reference** to
`Registry::new_module`, which takes `&Arc<RwLock<Registry<P>>>`. Each allow's
comment states that invariant, why `Rc` is not used, and which call requires the
`Arc`; no crate-level allow exists. The fifth was genuinely unnecessary and was
*discarded* rather than allowed: `freeze_rejects_an_unregistered_dependency_key`'s
`elsewhere` is only ever `write().unwrap().freeze_mapped(..)`, so it is now a
plain owned `Registry::new()`.

**The gate.** `.github/workflows/ci.yml` is new (`name: ci`, on `pull_request`
and on a push to `dev`, mirroring `build.yml`'s trigger block and its
`concurrency` group). One `ubuntu-latest` job runs, in order,
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
warnings` and `cargo test --workspace`. Rust is set up exactly as the two
existing workflows set it up (`actions/checkout@v4`,
`dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`) with
`components: rustfmt, clippy` added — those two workflows only ever run `cargo
build`, so they never needed a component that a gate does. `build.yml` and
`release-lichen.yml` are untouched. There is deliberately **no**
`[workspace.lints]` table and no baseline file: the three commands *are* the
policy, so there is nothing to keep in sync — this item's "fix" line above asked
for `[workspace.lints]`, which `D12` replaced with the stricter form, and
manifest consolidation is still `P3-2`'s. **The grammar crate is covered by a
fourth step:** `tree-sitter-lichen` is a workspace of its own (`P5-12`), so
`cargo fmt --all` from the repository root never reaches it; the step repeats
the same three commands with `--manifest-path tree-sitter-lichen/Cargo.toml`.

**The full suite was run once, deliberately.** `cargo test --workspace` is an
explicit, one-time exception to this project's "never run full-scale tests"
rule: the lint fixes reach every crate, so the set of tests these edits
influence *is* the workspace, and `D12` requires the gate to be green from its
first run rather than from its second. It passed — **740 passed, 0 failed, 0
ignored** — and the grammar crate's own manifest adds 2 more. The checks were
then re-run green on the committed tree: `cargo clippy --workspace --all-targets
-- -D warnings` exits 0, `cargo fmt --all -- --check` exits 0, and `cargo clippy
--workspace --all-targets` reports **zero** warnings (from 65).

**Mechanical edits inside audits this item does not own**, listed so the next
reader can tell a lint collapse from a change with a subject: the two
let-chains and the `explicit_auto_deref` in `language/src/resolve.rs` sit beside
`P2-2`'s named walk rather than in it (`resolve.rs:478` is untouched); the
let-chain inside `analysis.rs`'s `ScopeCapture` walk and the one in
`definition_at` are `collapsible_if` desugarings, not a walk refactor; and the
`redundant_redefinition` in `language/src/session.rs`'s `splice_program` is a
deleted redundant rebinding (`let mut ranges = ranges;`, where `ranges` was
already `mut`) inside the machinery `P2-1` calls unwired — `P2-1`'s doc and
wiring questions are untouched. No arena accessor, artifact container or
version, table hash, or walk structure was changed.


### P3-4 — Four byte↔line/col implementations with divergent edge behaviour

`span/src/lib.rs:31-38` (panics on an empty `starts`; `Err(0)` → index
`usize::MAX`), `language-server/src/lsp.rs:32-42` (returns the **last** line
start for an out-of-range line — a silently wrong offset), `:50-63`, `:87-94`.
`language/src/render.rs:112` re-scans the source from byte 0 per diagnostic.

**Fix.** One shared implementation over the already-computed `line_starts`
(every call site has it), with one documented out-of-range answer.

## P4 — optimization

### P4-1 — Registry read lock and `Arc` clone per array element `verified`

`lowlevel/src/static_module.rs:40-48` takes `self.registry.read()`, looks up, and
**clones the `Arc`** (an atomic refcount bump). It is called inside the deep
pass's inner loop — once per array element and twice per table entry
(`evaluation.rs:602, 607-624`), inside a `matches!` guard, plus `:614`, `:621`,
`equality.rs:549`, `table.rs:138`. For an N-entry table that is 2N lock
acquisitions and 2N refcount bumps per visit. Hoisting
`let module = self.static_module(sref.module);` above the iterator removes
almost all of it.

### P4-2 — `write_node_value` is O(class size) `verified`

`lowlevel/src/equality.rs:93-107` walks the **entire class member list** on
every concrete write, and six sibling predicates repeat the same full walk
(`class_has_pending_op :470-481`, `class_is_pure_cell :488-502`,
`class_is_skeleton :510-536`, `class_committed_value :728-738`, `pending_op
:636-644`, `force_pending :746-753`) — seven independent walks of the same list
per unification step. Classes grow with application count (`regroup_clones`
merges every clone per apply, `apply.rs:173-186`; `MAX_APPLY_TOTAL = 100_000`),
so this is quadratic in the common recursive case.

**Fix.** A small summary maintained at union time (has-concrete-value,
has-pending-op) collapses most of these to O(1).

### P4-3 — A thread and a rebuilt combinator graph per parse `reported`

`language-parser/src/parse.rs:94-103` and `:188-201` each
`thread::scope` + `Builder::new().stack_size(16 MiB).spawn_scoped`, and
`parse_inner` calls `program_parser(tokens)` (`:111`) which recurses through the
whole precedence ladder with ~20 `.boxed()` clones (`:425-539`). So per parse:
two thread spawns and a full combinator-tree rebuild. A `OnceLock`-held parser
(chumsky parsers are `Clone` and reusable) removes the construction; by the same
measure `In<'a> = Stream<Cloned<…>>` (`:77`, `:110`) deep-clones every token
payload — one heap allocation per identifier and per string literal, per parse.

### P4-4 — Quadratic diagnostics `reported`

`highlevel/src/diagnostic.rs:424-428` `orphan_unify_errors` is O(E × D);
per-error it also does `apply_errors.iter().find` (`:435`) and
`diary.iter().find` (`:465`). `diagnostics()` is what an editor calls on every
keystroke. One O(E + D) sweep over the diary (marking owned indices in a
`Vec<bool>`) replaces all three. In `language/src/render.rs:112,124`,
`render_all` is O(diags × lines) for the same reason.

### P4-5 — `path.contains` as a cycle guard; O(n²) kernel codegen `reported`

`equality.rs:272`, `table.rs:178`, `table.rs:257`, `equality.rs:832` do a
**linear scan of a `Vec`** at every recursion level, and `key_eq`/`hash_inner`
*hash* each element, so a table key of depth *d* costs O(d²) hashes. A `HashSet`
beside the existing `Vec` gives O(1) membership with the same push/pop
discipline. Separately, `compute.rs:1009-1011` `class_computation_node` linearly
scans the module's whole node table and is called **per emitted node**
(`emit_node`, `:1069`); `equality_rep` has no path compression.

### P4-6 — Per-apply clones, repeated `as_enum`, per-byte `mix`, intern leak `reported`

- `function.rs:100-106` and `:421-429` clone the template's whole node list per
  apply (plus `asserts`), and `apply.rs:173-186` builds a `HashMap` per apply;
  `static_module.rs:125`, `:327` likewise.
- `evaluation.rs:588-634` evaluates `value.as_enum()` up to four times and
  iterates `array.items()` twice and `table.items()` **four** times, each with a
  nested `self.nodes[node].evaluated_deep` lookup.
- `table.rs:202-205` does three multiplies **per byte** for a string key hash.
- **`Box::leak` per compile** (`language/src/compile.rs:362, 374, 390`):
  `Compiler::new()` is fresh per `compile_resolved` (`:95`), so the two intern
  maps dedupe only *within* one compile. Every compile permanently leaks every
  string literal and every distinct name; the editor leaks per keystroke, and a
  fresh copy per keystroke while editing inside a literal. The deserializer
  leaks the same way (`lowlevel/src/codec.rs:204`). Correctness is unaffected
  (native-op lookup compares `&str` by content) but the growth is unbounded.
- `apply.rs:114` linear dedupe scan over `apply_errors`, which is append-only
  and never cleared.
- `compute.rs:1879` `Box::leak` per `compute_native_ops!` invocation.
- `registry/codec.rs:42-45` writes a `u8` leaf-name length — truncates and
  desynchronises the stream past 255.

## P5 — hygiene and docs

- **P5-1 `verified`** — `crates/lichen-language/tests/scratch.rs` is a 31-line
  debug printer with **no assertions**; it can never fail. Delete it.
- **P5-2 `verified`** — `docs/README.md:45` indexes
  `type-system-cleanup-plan.md` as *"current (Phases 0/1a/1b/1c/2/3a); proposed
  (the rest of Phase 3, Phase 4+)"* while the note itself says *"**Phases 0–5
  complete**"*. Reconcile, and decide whether a completed plan stays (the
  project's own rule is that stale material is removed).
  **Outcome.** Re-read first-hand, and the note's own content decides it: its
  status line says **Phases 0–5 complete.** and its body closes Phase 3a, 3b
  and 3c, Phase 4 and Phase 5, so the README's
  *"proposed (the rest of Phase 3, Phase 4+)"* was the stale half and the note
  is the authority on itself. The row's status cell now reads
  `current (Phases 0–5 complete)` — `current` per the index's own legend,
  because the note describes behaviour the code now has. The completed plan
  **stays** in the index: it is cited from the code
  (`checker.rs:305-306`) and carries decisions the type-system work still refers
  to, so it is a record rather than superseded material. The row is otherwise
  accurate — its label is a shortened form of the note's own title
  (*"Type-system cleanup and standardization plan"*) and its crate list names
  the plan's three subjects. Nothing else in the index changed.
  *Which row, exactly:* this ledger's item was summarized elsewhere as being
  about the **code-audit** row, but `docs/README.md:73` already agreed with this
  note verbatim (*"current — the queue is the work list; each item's status is
  live"*); the one status cell the sweep found in disagreement is the
  `type-system-cleanup-plan.md` row above, as the *Checked and found clean*
  summary records. The latter's "the disagreement is the one status cell,
  `P5-2`" is therefore historical from this commit on.
- **P5-3 `verified`** — doc comments contradicted by the code:
  `analysis.rs:5-7` (claims `BufferSession`, `P2-1`); `device.rs:74-75` (claims
  all mutations are locked, `P1-11`); `device.rs:115-117` (claims the next save
  repairs a corrupt registry, `P1-12`); `module_key.rs:6-8` (key space bounded);
  `table.rs:8-11, 20-21` (determinism and hash stability, `P1-3`/`P1-4`);
  `evaluation.rs:534-535` (the counter claim, `P1-2`); `lowlevel/lib.rs:403`
  (*"not really pointing to anything"*); `lowlevel/lib.rs:709-713, 854-857,
  981-984` (claim cross-thread registry sharing — `Module`/`Registry` are
  `!Send + !Sync` because `AnyHandle` holds raw pointers, there is no
  `unsafe impl Send` in the workspace, and clippy reports three
  `arc_with_non_send_sync`; correct the docs or add a documented unsafe impl);
  `compiler_cache.rs:55-58` and `persist.rs:540-545` (the slot key, `P1-13`);
  `highlevel/ir.rs:366-371` (struct layout is the opposite of
  `checker/structs.rs:709-714`); `checker.rs:576-580` and
  `evaluation.rs:316-318` (`P1-6`); `compute.rs:41-45` (claims parallelism,
  `P1-18`); `preprocess-isolation.md:61-64` (signatures that no longer exist);
  `language-parser/ast.rs:239-240`, `parse.rs:26-29`, `lex:19-22`, `lex:30-33`
  (four grammar claims the grammar contradicts); `readme.rs:25-28` (claims the
  README test rewrites metadata, while both test files say it must *fail*);
  `language/lib.rs:115-117` (claims `build` can be `None`, it cannot);
  `render`'s perspective discriminator doc names a byte the crate never emits.
  Two more leads, both found while fixing `P0-4`: `crates/lichen-package/README.md`
  says `lichen update` goes *"to the repo's latest commit"* while `update()` uses
  `latest_release_tag` (the newest **published release**); and
  `scripts/release.sh` prints a `releases/tag/<full-sha>` URL although the real
  tag is the 12-character short SHA (`RELEASE_TAG_LEN`), so the printed link does
  not resolve.
  **Outcome.** The list was walked entry by entry and every citation re-derived;
  the line numbers had drifted in both directions.  *Corrected in place:*
  `language-server/analysis.rs`'s module doc claimed the checker ran "via
  `BufferSession`" — `Doc` calls `frontend_at` + `build_report` and never
  imports or names the session (this is `P2-1`'s own prescribed "correct the
  doc" step, done here; its wiring half stays `D6`); `checker.rs`'s two "which
  the runtime panics on" clauses (the definition pass's skip comment and
  `check_failed`'s doc) — since `P1-6` the runtime records
  `EvalError::ApplyTarget` instead; `highlevel/ir.rs`'s struct layout, which
  said the shape bundles the nominal id — the checker's `[shape, kind]` pair has
  the field types in the shape and `[TypeStruct{id, names}, K]` in the kind, the
  opposite of the claim; `language/src/lib.rs`'s `Report::build` doc ("`None`
  only when the resolve stage failed") — the lowering is total, so every path
  this crate produces is `Some`; `crates/lichen-tools/src/readme.rs`'s claim that
  `cargo test` self-heals stale `output = "..."` metadata — `tests/readme.rs`
  rewrites only the README and `tests/examples.rs` asserts the metadata and
  fails; the four grammar claims (`parser/ast.rs`'s `T<e>`, `parser/parse.rs`'s
  module doc and `atom_parser` doc, `lex/lib.rs`'s Glue paragraph and keyword
  list) — the array type is the keyword-led `array<T, n>`, a glued `<` is the
  raw type-component read `X<e>`, Glue precedes **five** delimiters (`::`
  included), the keyword list omitted `string`, `return`, `pub`, and `array`,
  and a bare `~` is `Tilde(usize::MAX)` (`P5-5`); `lowlevel/lib.rs`'s doc on the
  re-exported `ModuleKey`, whose first paragraph repeated the pre-`P1-12` "key
  space stays bounded" claim — that paragraph is deleted and the type's own doc
  in `lichen-registry` is the one statement; `Registry`/`Module` docs saying
  modules "executing in threads" share one registry `Arc` — a `Handle` holds a
  raw pointer, so `Arc<RwLock<Registry<…>>>` is neither `Send` nor `Sync`
  (clippy's `arc_with_non_send_sync` on
  `highlevel/tests/attributes.rs:151` is the machine check) and the sharing is
  single-threaded; `language/src/program.rs`'s per-leaf codec comment, which
  described a leaf *position* byte where the codec writes a length-prefixed leaf
  **name** (`Writer::leaf`); `lichen-perspective`'s codesign list, which named
  `u8(9)` for `GcdOp` — the crate writes `u8(0)` as that leaf's payload behind
  the `GcdOp` name tag, so nothing emits 9; `lichen-package/README.md`'s
  "update … to the repo's latest commit" — `update()` follows
  `latest_release_tag`, the newest published release; and `scripts/release.sh`,
  which printed `releases/tag/<full-sha>` where the tag is the 12-character
  short SHA (`RELEASE_TAG_LEN`), so the link never resolved.  The document the
  note called `preprocess-isolation.md` is
  `docs/notes/preprocessor-isolation.md`, and its shim section described
  wrappers generic over `V`/`O`/`C` returning `Diag<CompiledProgram<V,O>>` —
  they are generic over one `P: LangProgramShape` and return
  `(Preprocessed, Vec<Diag<P>>)` / `Vec<Diag<P>>`, and `compile.rs` and
  `readme.rs` are no longer callers.
  **Already correct**, fixed with the item that owned the code and left
  standing: `device.rs`'s lock-scope and recovery docs (`P1-11`, `P1-12`),
  `module_key.rs` and `compiler_cache.rs`/`persist.rs`'s slot-key comments
  (`P1-12`, `P1-13`), `evaluation.rs`'s depth-counter comment (`P1-2`) and its
  non-callable-target comment (`P1-6`; the sentence is gone), and
  `lowlevel/lib.rs`'s `value_eq` SAFETY note (`P0-2`).
  **Refuted:** `compute.rs`'s parenthetical "claims parallelism" — the cited
  bullets describe `parallel`'s type-level effect (the result is a kernel
  *struct*) and `plrun`'s index range, not concurrent execution, and the
  function's own doc says "v1 runs sequentially (the data-parallelism is
  logical)".  Whatever the operator names and module docs still overstate is
  `P1-18`'s finding, and was left to it.
- **P5-4 `verified`** — `Cargo.lock` holds **three** `wasmparser`
  (0.227.1 / 0.228.0 / 0.258.0) and two `wasm-encoder` (0.227.1 / 0.258.0).
  The JIT emits with `wasm-encoder 0.258` and `wasmi 2.0` validates with
  `wasmparser 0.228` — a 30-release skew between an encoder and its validator.
  The current MVP subset happens to work; nothing pins the pair.
  **Outcome.** The premise held, re-derived from `Cargo.lock` and `cargo tree`
  (the note's own numbers are still current): `wasmparser 0.228.0` is
  `wasmi 2.0.0`'s validator and is pulled by nothing else, `wasm-encoder 0.258.0`
  was `lichen-compute`'s encoder (and, separately, `wast`/`wat`'s under
  `wasmi`), and the 0.227.1 pair belongs to the Zed extension's `wit-bindgen`
  graph, not to the JIT path.  **`wasmi` does not re-export its `wasmparser`**
  (its `lib.rs` re-exports `wasmi_core`, its own `Engine`/`Module`/… surface,
  and nothing named `wasmparser`; the only use is
  `src/error.rs`'s private `wasmparser::BinaryReaderError`), so the first option
  is unavailable — validating against wasmi's own parser would mean pinning a
  second, independently-drifting `wasmparser` next to `wasmi`'s.
  **The emitted encoding is now the validator's generation.**
  `lichen-compute` depends on `wasm-encoder = "0.228.0"` — the wasm-tools
  release that `wasmparser 0.228.0` belongs to — with the pairing stated in the
  manifest.  The emitter uses only that crate's `TypeSection`/`ImportSection`/
  `FunctionSection`/`ExportSection`/`CodeSection`, `i32`/`i64` value types and
  the MVP instruction set (`i64.const/add/sub/le_s/eq/extend_i32_u`,
  `i32.wrap_i64`, `local.get`, `select`, `call`, `end`), so the alignment needed
  **no code change**: `cargo check -p lichen-compute` compiles the 0.258 call
  sites unchanged against 0.228.  Cost, stated: the lockfile gains a
  `wasm-encoder 0.228.0` entry and keeps 0.258.0 for `wast`/`wat` under `wasmi`,
  so three `wasm-encoder` versions remain — the skew being closed is the one
  this crate's emissions could exhibit, not the transitive ones in `wasmi`'s own
  text-format dependencies.
  **Residual, deliberately left.** `wasmi 2.0.0` is the authority on the pair
  for as long as it is pinned; a future `wasmi` bump moves `wasmparser` and
  re-opens the gap, which is what the manifest comment exists to catch.  Nothing
  validates the assembled bytes at assembly time: the first validator to see
  them is `wasmi::Module::new` (`compute.rs`'s two launch paths), which returns
  an `Err` rather than executing anything it cannot parse, so a mismatch stays a
  loud kernel-load failure.
- **P5-5 `reported`** — `language-lex/src/lib.rs:665-676`: a `~` count that
  overflows **saturates to `usize::MAX`** (which means "bare `~`"), silently
  changing the program's meaning, while the sibling `Int` path reports the
  identical mistake.
  **Outcome.** The premise held, re-read first-hand: `saturating_mul` /
  `saturating_add` collapsed any run past `usize::MAX` to `usize::MAX`, the very
  payload a bare `~` gets, and the value is consumed as a depth the whole way
  down — `TokenKind::Tilde(n)` → `Expr::Shallow(_, depth, _)` →
  `IR::depths` → `checker/indexing.rs`'s `usize::MAX` arm, which selects the
  **whole subtree**. A typo'd depth was therefore a different marker with no
  complaint. **Reject, not clamp**: this crate already has the channel and the
  precedent — `IntLit` reports *"integer literal out of range"* and returns no
  token — so the arm now parses with `checked_mul`/`checked_add`, records
  *"shallow marker depth out of range"* at the token's own span, and returns
  `None` on overflow, exactly as its sibling does. Clamping was rejected
  because every value it could clamp *to* is meaningful (the bare `~`, or some
  arbitrary smaller depth), which is the confusion being removed; the bare `~`
  and every depth that fits are unchanged.
  **Test.** `crates/lichen-language-lex/src/tests/lex_tests.rs`'s
  `an_overflowing_shallow_depth_is_a_lex_error` pins the one diagnostic and
  that the run contributes no token. Against the unfixed arm it failed with
  `left: 0, right: 1` — no diagnostic at all, the silence itself.
  **Residual, deliberately left.** A spelled depth that genuinely *equals*
  `usize::MAX` (`~18446744073709551615` on a 64-bit target) still encodes the
  bare `~`: the token payload has no second encoding for it, and that is a
  valid number rather than the overflow this item is about. Closing it would
  change the token type, not the parse of an out-of-range literal.
- **P5-6 `verified`** — `crates/lichen-package/build.rs:13-24` uses
  `rerun-if-changed=../../.git/HEAD`. In a git worktree `.git` is a *file*, so
  the trigger never fires and `LICHEN_BUILD_COMMIT` goes stale — and this repo's
  own workflow mandates worktrees. `Command::new("git")` also relies on the
  build script's CWD rather than naming the directory.
  **The audit's stated consequence was wrong and is corrected here; the path
  defect is real.** Measured first-hand on this toolchain (cargo 1.97.1, in this
  worktree): a `rerun-if-changed` path that does not exist does **not** leave the
  directive inert — cargo re-runs the build script on **every** build. So in a
  worktree the missing `.git/HEAD` costs an unconditional recompile of this
  crate (two consecutive `cargo build -p lichen-package` both printed
  *"Compiling lichen-package"*) while the embedded commit is refreshed every
  time and is therefore **never** stale there. The real staleness window is the
  opposite direction and belongs to *both* checkout kinds: only `HEAD` is
  watched, and a commit, amend, reset or rebase on the **current branch** moves
  `refs/heads/<branch>` — a commit on a *packed* ref writes a loose file and
  leaves `packed-refs` untouched (verified in a scratch repository) — so the
  embedded commit does go stale until some unrelated checkout moves `HEAD`.
  The note's second half is not a defect: the build script's CWD is the package
  root, which is exactly where git should walk up from.
  **Outcome.** The trigger now names the **resolved** git directory's `HEAD` and
  the directory holding the branch refs — both exist, and cargo watches a
  directory recursively:
  `rerun-if-changed=<absolute-git-dir>/HEAD` and
  `rerun-if-changed=<git-path refs/heads>`, where `--git-path` relocates the
  latter into the main checkout's git directory for a worktree, since that is
  where a worktree's refs actually live. A branch-ref write is now observed —
  the case the old directive could never see in either checkout kind — and the
  missing-path unconditional rerun is gone. Measured after the change, in this
  worktree: the second of two consecutive builds did nothing; touching the
  resolved `HEAD` rebuilt; touching `refs/heads/feature/code-audit` rebuilt;
  touching an unrelated file did nothing.
  **Residual, deliberately left.** Any ref write under `refs/heads` re-runs the
  script, so creating an unrelated local branch costs one recompile of this
  crate. Narrowing it to the single loose ref below `HEAD` would re-introduce a
  missing-path directive whenever that ref is packed, which is the defect just
  removed. `packed-refs` is not watched: a commit never updates it, and packing
  or unpacking changes the ref files it describes.
- **P5-7 `reported`** — `toolchain.rs:356-359` prints a progress line to
  **stdout** from library code, while `main.rs:446-449`'s command contract is to
  print a path: `SERVER=$(lichen path language-server --project .)` captures two
  lines. Use `eprintln!` or a verbose flag.
- **P5-8 `reported`** — `package/src/plugin.rs:281-288, 305-317, 217-228` build
  `Cargo.toml` by string interpolation: a `--repo` containing `"` injects
  manifest keys, and a Windows local path is emitted as `path = "C:\dir\crate"`
  where `\d` is an invalid TOML escape, so the generated manifest does not parse.
  **Outcome.** The premise held; the cited lines are still the sites
  (`server_dep`, `core_dep_line`, `core_patch`, plus `plugin_lines` and
  `write_cargo_toml`'s own `name =`).  Reachability, each half re-derived:
  *a newline is reachable from a plugin's own manifest* — the preprocessor's
  string literal is `"[^"@]*"`, documented "no escapes, may be multiline" and
  pinned by `a_string_may_span_lines_and_hold_commas`, so `dep.url`,
  `dep.rev`/`branch`/`tag`, and `dep.package` (all strings) can each carry one
  and inject a whole key or table; *a `"` is not reachable from the lexer* (the
  same regex forbids it) *but is reachable from `--repo`*, which is an argv
  string that never passes through that lexer; and a Windows directory path is
  reachable whenever `--repo` (or the monorepo's local-checkout flow) is a local
  directory, which the path branch turns into an unescaped `path = "C:\…"`.
  **The `toml` crate is not a dependency of this workspace** — it appears in no
  manifest and in no `Cargo.lock` entry — so the note's preferred "serialize a
  real value" route was unavailable and the fallback applies.
  **One helper, every value.** `plugin::toml_string` renders a TOML basic-string
  literal (`"`, `\`, `\b`/`\t`/`\n`/`\f`/`\r`, and `\uXXXX` for the other
  control characters and DEL), and *every* interpolated value now goes through
  it — a dependency key as well as a value, since a key is a string too: in
  `server_dep`, `core_dep_line`, `core_patch`'s `[patch.…]` header,
  `plugin_lines` (path, git URL, `package`, `rev`), and `write_cargo_toml`'s
  `name =`.  `extra_deps` stays a pre-rendered fragment, and its only producer
  is the now-escaping `server_dep`, which its doc states.
  **Tests.** `crates/lichen-package/src/tests/plugin_manifest_tests.rs` (new,
  reached through a `#[path]` module) generates the document for a plugin whose
  URL, package name, and revision carry a newline and for a `--repo` that is a
  Windows path, and parses it with the `toml` crate (a new **dev**-dependency
  only; the shipped graph is unchanged).  Against the unfixed generator both
  tests failed with `TOML parse error at line 7, column 32 … invalid escape
  sequence` on `git = "C:\work\lichen-vm"`, and the dumped manifest showed the
  injection beside it: `plug\nbad = 1 = { git = "https://example.com/plug\n
  [package]", package = "plug\nbad = 1", rev = "dead\nbeef" }`.  Both pass after
  the fix, and a generated manifest with quoted keys and escaped Windows paths
  was also accepted by the real consumer (`cargo metadata --no-deps`).
  **Adjacent finding, not fixed here (no item owns it):** the generated
  `src/main.rs` has the same hole in a worse place —
  `compose_source` writes `{crate_ident} as {crate_ident}_leaves;` as **Rust
  code** and `native_package_lines` writes it inside a Rust string literal,
  where `crate_ident` is `dep.package` with `-`→`_`.  `dep.package` is a
  multiline-capable string, so a `}` or a newline there is injected Rust source
  that `cargo build` then compiles.  That is a code-generation defect, not the
- **`src/main.rs` carries the same class** — `compose_source` writes the
  dependency's `package` as **Rust source**, so a newline or a `}` injects code
  the build then compiles. That is `P1-25`, and it is fixed there, not here: the
  manifest is data and this document is code.
- **P5-9 `reported`** — `language/src/package.rs:233, 241, 253, 261, 362, 603,
  631, 810, 829` and `cli.rs:235` report an `io::Error` as a *source* diagnostic
  with a fabricated `(0, 0)` span, which `render.rs:109-116` prints as a caret
  at line 1 column 0. There is no `Stage::Io`, so a permission error and a syntax
  error are indistinguishable to a consumer.
  **Outcome.** The premise held; the count and the mechanism are both corrected
  here.  Re-derived, the sites are **eight**, not ten, and the line numbers had
  drifted: `package.rs:250` (embedded-wrapper registration), `:258` (a non-`.lichen`
  path), `:270` (`fs::canonicalize`), `:278` (circular import), `:379`
  (`fs::read_to_string`), `:823` and `:842` (the vendored entry package missing
  or ambiguous), and `cli.rs:235` (native-package registration).  **Only three
  of the eight are filesystem failures** — `canonicalize`, `read_to_string`, and
  the `fs::read_dir` error that `vendored_entry_file` used to swallow into
  `Vec::new()` and then report as "has no `.lichen` entry package" (a permission
  error read as a missing file); the other five are package-resolution and
  registration failures that fabricated the same span.  The note's "an
  `io::Error`" is true of the first three and false of the last five; the
  fabricated span was real in all eight.
  **Representation.** `Stage::Io` joins the stage enum, and `Diag::unattributed`
  / `Diag::io` build a diagnostic with `span: None` — the shape the highlevel's
  own unattributed failure already takes (`lib.rs`'s `!build.ok` fallback), so
  this is that one convention rather than a second.  `render` already prints a
  span-less diagnostic as its message alone: no `-->`, no line, no caret.  All
  eight sites now carry no span; the three filesystem ones are `Stage::Io`, the
  five others keep `Stage::Preprocess` and are span-less.  Every message keeps
  its path, and the `read_dir` case now keeps the underlying error too.  The
  language server's exhaustive `severity_for` gained `Stage::Io => ERROR`; its
  `lsp_diagnostics` already maps `span: None` to the zero-width `0:0` range the
  protocol requires, so a span-less diagnostic reaches the editor with no
  position rather than a wrong one.
  **Test.** `crates/lichen-language/tests/persist.rs`'s
  `a_missing_package_is_an_io_diagnostic_not_a_line_one_syntax_error` loads a
  path that does not exist and asserts `stage == Stage::Io`, `span.is_none()`,
  that the path survives in the message, and that the rendering has neither a
  caret nor a `-->`.  Against the unfixed tree it failed on the span with
  `Diag { span: Some((0, 0)), message: "cannot read package …absent.lichen: …
  (os error 2)", stage: Preprocess, check: None }` — the missing file reported
  as a preprocess diagnostic at line 1, column 1.
  **Adjacent, not fixed (another crate, and a different item's shape):**
  `lichen-preprocess`'s `PreprocessDiag::at_zero` (`lib.rs:61-66`, used at
  `:468` and `:475`) still fabricates `(0, 0)` for two position-less failures,
  one of which — *"dependency '{alias}' is not fetched … run `lichen fetch`
  first"* — is this item's defect in the preprocessor: a missing directory
  reported as a source diagnostic at 1:1.  Giving it `Stage::Io` needs a kind on
  `PreprocessDiag`, which that type does not have, so it is left for its own
  item rather than redesigned here.  `language-server/analysis.rs:415` and
  `:472`'s `StatementValue { span: (0, 0), … }` is a hover snapshot's fallback,
  not a diagnostic, and is out of this item.
- **P5-10 `reported`** — `render/src/render.rs:869` indexes `kind.items()[0]`
  with no length check (its sibling `kind_is_struct` at `:1034` checks), and
  `:932-941` walks `equality.parent` unguarded, uncompressed and cycle-unsafe,
  forking `disjoint::find`. Both run from `Doc::new`, i.e. every keystroke.
  **Outcome.** The premise held; both sites re-derived (the lines had drifted):
  the index is `struct_marker_value`'s `unsafe { kind.items() }[0]` (now `:906`)
  and the walk is the free `representative` (now `:975-980`), a read-only fork of
  `disjoint::find` because the lowlevel's `Module::equality_representative` takes
  `&mut self` for its path compression while the printers hold `&Module`.
  **Reachability, both first-hand.**  The index panics on a kind array with no
  first element; its sibling `kind_is_struct` guards with `kind_items.len() == 2`
  before indexing, and the two other `kind_items[0]` readers
  (`struct_field_names`, `struct_kind_id`) are guarded by the `kind_is_struct_any`
  call that precedes them, so this was the one unguarded index in the crate.
  The walk **indexed** `module.nodes[n]`, while `Module::node_value` documents
  the opposite contract — "a dynamic ref that names a released node reads `None`
  (via `SlotMap::get`), so the read API is safe for a node the executor may have
  dropped" — so a stale id is a state the renderer's own reads already handle,
  and `drop_block` removes nodes from the table.  It also looped forever on a
  `parent` cycle, which a corrupt equality forest produces.
  **Fix.** `struct_marker_value` reads `unsafe { kind.items() }.first()?`; the
  walk returns `Option<NodeId>`, reading `module.nodes.get(n)` and stopping after
  one step per node in the table (a parent chain cannot be longer than the table,
  so a revisit ends the walk instead of looping).  Each caller answers with the
  crate's own "no answer": `class_name` renders the unknown `?` — the spelling
  `type_constant` already falls back to — and `is_arrow`/`is_universe` answer
  `false`.  No error type was added and the renderer was not restructured.
  **Test.** `crates/lichen-language/src/tests/render_tests.rs`'s
  `a_node_the_module_no_longer_holds_renders_without_panicking` removes the
  root type node from a checked build's table and prints it through the public
  `print_type_lang`.  Against the unfixed walk it panicked at
  `crates/lichen-render/src/render.rs:976:42: invalid SlotMap key used`; after
  the fix the same call answers `?`.  The index site has no test — an empty kind
  array needs a hand-built module — and is covered by `.first()?` plus its
  sibling's existing guard.
  **Also corrected, P5-3's class:** the doc block above `is_struct_kind` carried
  the *universe* paragraph, whose mechanism ("a plain self-referential member
  (`contains(&node)`)") no longer exists — the test is now a class comparison.
  The paragraph moved to `is_universe` in the code's terms; `is_struct_kind`
  keeps its own sentence.
- **P5-11 `verified`** — `registry/src/device.rs`'s `verify_entry` opened every
  recorded dependency as a filesystem path, while `is_lichen_file_id` in the
  same file accepted `virtual:<name>` as a real file ID. **The audit's stated
  mechanism was wrong and is corrected here:** no `virtual:` string ever reached
  `fs::read`, because the caller recorded the embedded source's **display path**
  (`plug.lichen`), not its file ID (`virtual:plug.lichen`) — so
  `state.entries.get(dep_file_id)` returned `None` and the `?` produced the miss
  one line earlier. **The defect is real and reachable**, with the reported
  effect: a package importing an embedded source recompiled on every run. A
  `virtual:` file ID names an embedded (native) source — bytes compiled into the
  compiler binary by `register_native`/`register_compute` — and the artifact
  store is scoped per toolchain/plugin set, so it cannot change under a cache
  root and can never invalidate a dependent.
  **Outcome.** One shared file-ID notion (`virtual_file_id`, `virtual_name`,
  `is_virtual_file_id` in `lichen-registry::device`, with `is_lichen_file_id`
  built on it) now spans both sides. The package store records the dependency's
  **file ID** (`dependency_file_id`) rather than its display path, loads a
  recorded embedded dependency by its registered name, and `verify_entry` checks
  a `virtual:` dependency's recorded key and then treats it as verified — the
  embedded source has no bytes in the device to read or hash. The store's own
  identity strings (`register_native`, `register_compute`) go through the same
  helper, so the two sides cannot drift. A registry written before this change
  records the display path, fails verification once, and rewrites itself on the
  next compile.
  **Test.** `crates/lichen-language/tests/persist.rs`'s
  `a_package_that_imports_an_embedded_source_verifies_across_stores` registers a
  `plug.lichen` embedded source, loads an importing package through one store
  and then through a second store over the same cache directory, and requires
  the second to serve the artifact (`loaded_from_cache == 1`). It was confirmed
  to fail against the unfixed sources with `left: 0, right: 1`, and a probe of
  the registry file showed the entry keyed `virtual:plug.lichen` beside the
  recorded dependency `plug.lichen` — the mechanism above, not the reported one.
  **Residual.** Verification still requires the embedded source's entry to exist
  with the recorded key, so the native package must be registered (as the CLI's
  `staged_store` does) before a cached dependent can verify. Its recorded source
  hash stays the all-zero *pending* value, since verification never consults it,
  so a changed embedded source under the **same** cache root (a rebuild without
  a version bump) is not detected — the same slot-key hole `P1-13` names.
- **P5-12 `verified`** — `tree-sitter-lichen` is deliberately outside the
  workspace (the root manifest `exclude`s it, and its own manifest says so), but
  its manifest has **no `[workspace]` table of its own**, so resolving it as a
  package makes cargo walk *upward* for a workspace root. In the main checkout
  that walk stops at the root manifest, whose `exclude` covers it, and works. In
  a worktree the crate sits deeper (`.worktrees/<name>/tree-sitter-lichen`) and
  the walk lands on the **main** checkout's manifest — which does not exclude
  that path — so cargo fails:

  ```
  error: current package believes it's in a workspace when it's not:
  current:   C:\resource\lichen-vm\.worktrees\code-audit\tree-sitter-lichen\Cargo.toml
  workspace: C:\resource\lichen-vm\Cargo.toml
  ```

  Reproduce with `cargo metadata --no-deps --manifest-path
  tree-sitter-lichen/Cargo.toml` from any worktree. Not caused by this branch:
  `cargo fmt --all -- --check` exits 0 in the main checkout and fails in the
  worktree. The trigger is `lichen-language-zed`'s optional path dependency on
  the crate, so anything resolving the graph with that feature enabled hits it
  too; `cargo check --workspace` and `cargo test --workspace` are unaffected.

  **Why it matters.** `cargo fmt --all` fails in every worktree, and `AGENTS.md`
  both mandates working in a worktree and requires `cargo fmt` before a final
  commit. It also blocks `P3-3`'s workspace-wide `fmt --check` gate.

  **Fix.** Add an empty `[workspace]` table to `tree-sitter-lichen/Cargo.toml` —
  cargo's own suggested third option, and the standard way to declare a crate
  that is intentionally its own workspace. It stops the upward walk at that
  manifest. Verify that `cargo fmt --all -- --check` and `cargo metadata` then
  succeed from a worktree, that `cargo test --manifest-path
  tree-sitter-lichen/Cargo.toml` still works, and that resolving
  `lichen-language-zed` with the `grammar-consistency` feature still finds the
  path dependency.

### P5-13 — `PreprocessDiag::at_zero` fabricates a source span `verified`

The same class as `P5-9`, in the layer below it. `crates/lichen-preprocess`'s
`PreprocessDiag::at_zero` (`lib.rs:61-66`, used at `:468` and `:475`) builds a
diagnostic whose span is a fabricated `(0, 0)`, so the user is shown a
line-1-column-1 source problem for something that has nothing to do with source
syntax. The concrete case is the one a user meets most: *"dependency '{alias}' is
not fetched … run `lichen fetch` first"* — a **missing directory** reported as a
syntax error at the first character of a file.

**Fix.** `P5-9` gave the language layer a span-less `Diag` (`Stage::Io` plus
`Diag::unattributed`), which renders as its message alone. `PreprocessDiag` has no
equivalent kind, so the honest fix is to give it one — a diagnostic that carries
no span — rather than for the language layer to launder a `(0, 0)` into a
span-less diagnostic at its boundary. Which of the two is right depends on
whether `PreprocessDiag`'s consumers ever need a span at all; `P5-9`'s
implementation settled it for the language layer, so read that before choosing.
Report which you chose and why.

**Outcome.** The premise held, and the whole set was one defect: both `at_zero`
call sites sit in `stage_depends` and both report a failure that is not a
property of any source text. The choice the note left open went to the
preprocessor, on the consumers' evidence rather than on symmetry.

* `PreprocessDiag::span` was already an `Option`. The language layer's widening
  (`diag.rs:101-108`) copies it verbatim into `Diag::span`; `render`
  (`render.rs:111-123`) already prints a span-less diagnostic as its message
  alone; the language server (`analysis.rs:552-582`, the `None` arm at `:556-568`)
  already maps `None` to the zero-width `0:0` range the protocol requires.
  Nothing a consumer does with a preprocess diagnostic needs a span.
* The preprocessor has no honest position to offer either: a `Depend` carries no
  span at all (`lib.rs:131-144`), and the two failures are a missing clone
  directory (`lib.rs:481`) and a `sub` path that escapes its clone
  (`lib.rs:474`).
* So `P5-9`'s convention is kept, not duplicated. `at_zero` is **deleted**;
  `PreprocessDiag::unattributed` (`lib.rs:65-72`) is the same name and the same
  shape as `Diag::unattributed`, and the language layer needed **no change** —
  no second laundering convention, because there was never a span to launder.

*Not this defect.* An `import` that fails to resolve already gets the real span
of its `@import` directive: `preprocess` overwrites the resolver's span with the
statement's own (`lib.rs:290-295`). That path was honest before and after; only
`stage_depends` fabricated a position.

**Test.** `crates/lichen-language/tests/preprocess.rs`'s
`an_unfetched_dependency_is_not_reported_at_line_one` stages a `depend` under a
unique alias — so no fetch could have created the directory — and asserts the
message names the directory and `lichen fetch`, that `span` is `None`, and that
the rendering has neither `-->` nor a caret. Against the unfixed tree it fails
with `span: Some((0, 0))` (the panic prints the whole `Diag`). A second test,
`a_dependency_sub_path_outside_its_clone_is_not_reported_at_line_one`, pins the
other `at_zero` site.

## Decisions

These block the items marked `blocked:Dn`. Do not pick an answer silently.

- **D1 — Is `~/.lichen/artifacts/` untrusted input? — DECIDED: untrusted.**
  What that commits to: the reader must be **memory-safe and total against
  arbitrary bytes** (`P0-1` and `P0-5` are done; `P0-2` remains), and the
  container gets a **body digest** so corruption is detected before any of the
  body is interpreted (`P0-7`). One honesty note recorded with the decision: a
  digest detects *corruption*, not a deliberate writer — whoever can write the
  file can recompute the digest. Authenticity is not what a digest buys, and
  the docs must not claim it does; memory safety and total validation are what
  actually bound an attacker, and the digest is what turns bit rot into a clean
  recompile.
  *Rejected — trusted:* `checked_*` arithmetic only, signatures unchanged. It
  would have left the `P0-2` UB-from-safe-code path open.
- **D2 — Which git URL schemes are legitimate? — DECIDED: the minimal fix.**
  `git clone -- <url> <dir>` and reject any `url`/`rev`/`branch`/`tag` whose
  first byte is `-`. That closes argument injection (`--upload-pack`, and a
  `rev = "-f"` pin downgrade) without rejecting any legitimate URL form.
  *Rejected for now — a scheme allowlist:* stricter, but `ext::` is already
  disabled by default in modern git and an allowlist risks rejecting a form
  someone relies on (a local path dependency, say). If a scheme policy is ever
  wanted, it is a new item, not a reopening of `P0-3`.
- **D3 — Tests. — DECIDED: allowed, minimal, per item.** Each fix may add the
  smallest test that falsifies the defect it fixes, and only that. This
  unblocks `P1-5`, `P1-6` and `P1-13`, whose only sound proof is a test, and it
  means `P1-6` can now be pinned before it is touched. Tests stay in separate
  files from sources per `AGENTS.md`, and a fix must still not run the
  full-scale suite.
- **D4 — Toolchain integrity. — DECIDED: accept GitHub-over-TLS, and fix the
  docs instead.** The trust root is HTTPS to GitHub plus the release the
  maintainer published; no checksum or signature is added, and the download is
  not narrowed to github.com. What this commits to, and what the change must do:
  **delete the provenance claim the docs currently make.** `toolchain.rs`'s
  module doc reads as a guarantee (*"the toolchain and the package manager are
  always the same revision"*) while the commit is pinned and the bytes are not —
  the pin says *which* revision was asked for, never *what* arrived. State the
  actual trust model in its place, and state it once. The mechanical hardening
  the decision does not cover is split out as `P1-20` (a predictable shared temp
  name and no `fsync` before the rename), and the undocumented `$PATH` fallback
  is named there too.
  *Rejected — `SHA256SUMS`:* it would have kept the claim honest, at the cost of
  a release step and a verification path. *Rejected — signing:* strongest, and
  the most work; revisit if the toolchain ever ships to third parties.
- **D5 — Table key hashing. — DECIDED: canonical content unfolding.**
  A hashed key must be a function of the key's *content*, so a table or function
  value used as a table key hashes the same after a freeze and a reload as it did
  before. Chosen over a freeze-assigned structural identity (which would make the
  hash depend on the layout pass rather than on the value) and over declaring
  such keys unsupported (which would turn a silent miss into a refusal, narrowing
  the language rather than fixing the contract).
  What this commits to: `P1-3` and `P1-4` are one change, because they are the
  same key-comparison contract — a self-referential key needs a *canonical*
  unfolding so that two coinductively equal keys unfold identically, and the
  cycle token `hash_inner` uses must then agree with `key_eq`'s coinduction
  rather than counting depth. Expect the pair to be the hardest item in the
  queue: the unfolding has to be well-defined for a cyclic graph.
- **D6 — How far to wire incrementality. — DECIDED: (a) and (c) first, then (b).**
  Order: cache the extracted indexes per `(uri, version)` and reuse one
  `PackageStore` across requests, and add cancellation plus a debounce; wire
  `BufferSession` afterwards, once the first two have landed. `P1-5` is done, so
  the key is now genuinely injective and (b) is no longer blocked by it. Note
  what (b) does and does not buy while T3 (the memoized check) does not exist: it
  avoids the lex and parse, not the check, so the follow-up is worth doing for
  the keystroke path and is not a substitute for (a).
  `P1-18` — compute's registry eviction, per-launch wasm rebuild and unbounded
  `plrun` — is **not** part of this: it shares nothing with the editor path and
  proceeds on its own.
- **D7 — How to close `P0-2`. — DECIDED: make the obligation explicit (`B'`).**
  The accessors stay unbounded but become `unsafe` with one written contract, and
  the raw-pointer fields are privatised behind checked constructors.

  *Rejected — full lifetime binding (`A'`):* tying the returned slice to `&self`
  does not compile at the VM's own hot spots, because they read an arena slice
  **while mutating the module**. Verified at `gc.rs:57-68` (iterates
  `array.items()` across `self.garbage_collect_node`, then passes
  `array.items()` to `self.alloc_array`), `evaluation.rs:559-570` and `:578-582`
  (the deep pass), and `equality.rs:391-424` (holds `pa.items()`/`pb.items()`
  across `self.unify_inner`/`add_equality`/`record_error`); the same shape is at
  `function.rs:252`, `:555-580`, `table.rs:222`, `:264`, `apply.rs:143-144`.
  Every one of those would have to copy its node ids into a `Vec` before
  descending — an extra allocation per array/table visit in the deep pass,
  unification and GC. That is a hot-path regression, and it works directly
  against `P4-6`, which exists to remove exactly that kind of per-visit clone.
  Type safety bought with a permanent per-visit allocation in the VM's three
  hottest loops is not the trade this project should make.

  *Rejected — the audience split:* a lifetime-bound accessor for outside crates
  plus a crate-private raw one for the VM internals avoids the allocation
  entirely, but it still forces the callers that mutate inside the loop to be
  restructured (the checker's table walks, among others), and it adds a second
  accessor to a type that is already hard to read. Not worth the extra surface
  when `B'` reaches the same bound on the external hole.

  *What `B'` leaves open, stated so it is not mistaken for done:* inside the
  crate the one-clause invariant is enforced by discipline and review rather than
  by the type system. The mitigation is that the obligation now lives in exactly
  one written contract, every slicing site is `unsafe` and greppable, and no
  out-of-crate caller can obtain the slice from safe code at all.

- **D8 — Is a budget refusal sticky or scoped? (open; found by `P1-2`.)**
  The two guards implement opposite policies and nothing states which is
  intended.

  *Scoped* is what `deep_depth` does: incremented at entry, restored on every
  exit, so a refusal applies only to the subtree past the limit and shallow
  siblings still walk and are decided. `P1-2` documented the counter that way,
  on the argument that an inflated nesting counter is never restored except by
  `reset_apply_budget`, so every later `evaluate_node_deep` in the process would
  start past the limit.

  *Sticky* is what `apply_depth` does: its refusal returns before the decrement,
  and the comment there says so deliberately — *"so a caller still inside a
  refused apply cannot re-enter the walk"*. The `BudgetExhausted` doc points the
  same way: the verdict is latched because *"a second walk must know it ran
  against an already abandoned graph rather than discovering the exhaustion
  again"*. Read plainly, that says once abandoned, stay abandoned.

  So the asymmetry is the symptom, and one of the two is the anomaly. It is not
  a live bug: in production `evaluate_depth_limit` is 300 000 and the limits are
  only lowered by tests, so the difference is observable only when a limit is
  actually tripped. Resolve it as one policy stated on both counters —
  `P1-2`'s panic fix is independent of the answer.
- **D9 — Does the frontend need a nesting-depth limit? — DECIDED: no; accepted
  risk.** The three surviving stack-exhaustion paths (`P1-22` is fixed; `P1-23`
  the parser's worker and `P1-24` the AST's recursive `Drop` are not) are left as
  they are, and both items are closed `wontfix:D9`.

  The reasoning, recorded so it is not re-opened by accident: a source file that
  aborts the command-line compiler is a denial of service against the user's own
  machine, with no privilege boundary crossed — the compiler is not a service and
  the file being compiled is the user's own input. On the editor side the same
  input takes down the language server, which *is* user-visible, but the exposure
  is the same shape: a file the user chose to open (§ "What this does not
  cover", below).

  *Rejected — a depth limit with a diagnostic:* it would have to be a documented
  **language** limit, and the rejection is not about the work. `P1-23`'s number is
  imposed by the parser's fixed 16 MiB worker (175 nested levels before it dies),
  so any limit safe to enforce today is below that — i.e. the limit's value would
  be dictated by an implementation constant that is itself arbitrary, and the
  language would carry a restriction whose only justification is one crate's
  thread size. Fixing *that* first (growing the parser's stack on demand) is the
  other rejected option.
  *Rejected — unbounded stack growth:* it trades a crash for input-proportional
  memory, which is a worse failure for the language server, and it needs the same
  invasive iterative `Drop` for `P1-24`.

  **What this does not cover, so it is not mistaken for gone:** `P1-23` and
  `P1-24` remain real. A 351-byte file still aborts the parser, and a tree deep
  enough to be accepted still aborts while being freed. Revisit if the compiler
  ever becomes a service, if the language server starts accepting files it was
  not handed by the user, or if a legitimate program is found in the wild that
  nests past 175 levels.

- **D10 — Where the command-line surface lives. — DECIDED: its own crate.**
  `lichen-language` is the compiler **library**, and `clap` is a hard dependency
  whose entire use is `cli.rs`, so every embedder — including
  `lichen-language-server` — links a command-line parser it never calls. The CLI
  moves out of the library into a crate of its own (`P2-12`).

  *Rejected — a feature of `lichen-language`:* it is additive, so the surface
  stays in the library and every consumer still compiles the module; the generated
  plugin manifest would have to emit the feature, `[[bin]]` would need
  `required-features`, and the release workflows would have to pass it — the same
  three things that have to move for a separate crate, but with the CLI still
  inside the library afterwards.

  **What the move must carry, because each of these is a caller and not a detail:**
  `crates/lichen-package/src/plugin.rs` generates a plugin compiler's `main.rs`
  that calls `lichen_compiler::cli::main_with_native_packages::<crate::LangProgram>`,
  so the generated manifest's dependency line changes with the move; the compiler
  binary's target moves with it; and the release workflows that build and ship it
  follow. Keep the flag surface and the behaviour identical — this is a move, not
  a redesign, and `AGENTS.md`'s rule about not considering forward compatibility
  unless asked applies to any temptation to tidy the flags while in there.

  **Not part of this decision:** the README generator and `sync-readme`
  (`P2-6`'s remaining half) are repo tooling rather than a user-facing surface,
  and they leave the library on their own terms.
- **D11 — How far to encapsulate node state. — DECIDED: all six fields.**
  `P2-4`'s premise was partly refuted before this decision: the documented
  write choke-point (`Module::write_node_value`) **is** implemented, and
  `Node::value`/`low_shape` are private with a single writer each — so no public
  field bypasses the *value* write path. The real defect is the other six fields
  (`operation`, `function`, `block`, `visiting`, `evaluated_deep`, `equality`),
  which are `pub` with invariants of their own and no choke-point covering them.

  *Chosen — privatise all six and add the accessors.* The measured cost is **119**
  `cargo check` errors (116 in `lichen-lowlevel`'s own integration tree, 3 in
  `lichen-highlevel`, plus roughly a dozen more in `lichen-compute` that the
  compiler never reached). The acknowledged tension, recorded so it is not
  discovered mid-refactor: Rust cannot make a field write-private and read-public,
  so a write path must be **invented** for `operation` and `equality.parent` (they
  have none today), and read accessors must be added for `evaluated_deep`,
  `visiting` and `disjoint::Meta` — the read surface therefore **widens** while the
  write surface narrows. That is accepted: a documented reader is a contract,
  whereas a public field is an invitation.

  *Rejected — correcting the docs and leaving the fields:* it would close the item
  with the encapsulation the crate's own prose claims still absent, and the
  fields' invariants are load-bearing for *answers* rather than for memory safety,
  which is exactly the kind of breakage no test catches.
  *Rejected — privatising only the load-bearing few:* it leaves the boundary
  arbitrary, and the reader cannot tell which fields are contract and which are
  convenience.

  **Scope note:** `Module::nodes` is itself `pub`, so the refactor must also
  decide whether node state is read through the module or through the slotmap.
  Prefer the module (it is where a checked accessor can live); do not leave both.
- **D12 — What the CI gate enforces. — DECIDED: clear the backlog, then `-D
  warnings`.** `P3-3`: the workspace has 66 clippy warnings and CI enforces
  nothing — no `fmt --check`, no `cargo test`, no clippy.

  *Chosen — pay the backlog down first, then gate hard:* clear the 66 (the
  dominant clusters are 26 `collapsible_if`, 8 `type_complexity`, 8
  `needless_borrow`, 3 `arc_with_non_send_sync`), then add a job running
  `cargo fmt --all -- --check`, `cargo test` and
  `cargo clippy --workspace --all-targets -- -D warnings`. One gate, one meaning,
  and no baseline file to keep in sync.

  *Rejected — gating on "no new warnings" from a baseline:* it gets regression
  protection sooner but adds a file whose drift is itself a maintenance hazard,
  and this branch has already shown how quickly warning locations move.
  *Rejected — `fmt` and `test` only:* it would leave `arc_with_non_send_sync` —
  which is a real finding, not a style preference — unenforced.

  **Order matters and is part of the decision:** the backlog clearing and the gate
  are one item's work, and the gate must not land before the backlog is gone, or
  CI is red from its first run.

## Checked and found clean

Recorded so the next pass does not re-audit them.

- **No shell injection:** every subprocess uses `Command::new` with an argument
  vector; there is no `sh -c` and no shell string anywhere in scope. The risk is
  *argument* injection (`P0-3`), not shell injection.
- **No archive extraction** in any crate, so no `../`-in-entry-name surface.
- **No `set_current_dir` / `set_var` / `remove_var`** anywhere in the workspace.
  Every `current_dir` is `Command::current_dir`.
- **No `unsafe` outside `lichen-lowlevel`** (and none in `registry`, `compute`,
  `package`, `language`, `language-server`, `render`, `perspective`, `doc`,
  `utils`, `span`). No `transmute`. No `unsafe impl Send`/`Sync` anywhere. No
  `static mut`, no `thread_local!`, no lazy statics outside compute's two
  registries.
- **No `Rc`/`RefCell`/`Cell` in the lowlevel** — zero interior mutability, zero
  borrow-flag panics, zero reference cycles; the `&mut Module` discipline is
  explicit and consistent.
- **The wire format is sound in itself:** uniformly explicit little-endian
  (`to_le_bytes`/`from_le_bytes`), every multi-byte read via `take(n)` +
  `try_into`, and the registry carries a magic **and** a checked version
  (`device.rs:407-412`). The container has magic, a checked version, an
  alignment guard and a trailing-bytes check (`persist.rs:386-401`, `:499-501`).
  The problems are the cache management around it, not the encoding.
- **`disjoint` is correct:** path compression in `find` plus union-by-size, O(1)
  member-list splice, no allocation.
- **The registry's *mutation* path is correct:** cross-process `mkdir` lock,
  reload-under-lock, tmp+rename save.
- **No `todo!`/`unimplemented!`/`FIXME`/`TODO` marker anywhere** in the crates.
- **`#[stacksafe]` coverage in the lowlevel is complete** — all eleven recursive
  entry points are annotated, and the frontend's own walks are now annotated too
  (`P1-22`). The surviving overflow paths are not functions of ours and so cannot
  be annotated: the parser's worker stack (`P1-23`) and the AST's recursive
  `Drop` (`P1-24`).
- **`no_attr_ext` aside, no `format!` in a hot path** in the lowlevel or
  highlevel; `format!` appears in the highlevel only in two codec error paths.
- **`docs/README.md`'s index is complete** — every note is linked and no link is
  dangling. (This line used to add *"the disagreement is the one status cell,
  `P5-2`"* and to count 33 notes; `P5-2` is `done` and the count is now 34, so
  both clauses are removed rather than left to go stale again.)
- **The preprocessor extraction is exemplary** —
  `crates/lichen-language/src/preprocess/mod.rs` is an 82-line shim with no
  re-implementation; both crates are live. This is the standard other
  extractions should meet.
- **`examples/` is a real living spec** — 23 files, exactly the 23 README blocks,
  enforced by `crates/lichen-language/tests/readme.rs`.
- **`AttrExt` (`highlevel/src/attr.rs:113-232`) is the best-designed trait in the
  codebase** — it states `share_missing_slot`'s invariant and why, documents
  `is_subtype`'s conservative answer for unbound values, and explains that
  label-vs-constraint is purely semantic. The model `NativeOp` should follow
  (`P2-5`).
