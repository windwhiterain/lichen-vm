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
| P1-3 | high | lowlevel | Table identity hash is a raw address | todo |
| P1-4 | high | lowlevel | `hash_inner` cycle token vs `key_eq` coinduction | todo |
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
| P1-21 | medium | lowlevel, highlevel | A struct value applied through a deferred callee is still silent | todo |
| P1-22 | high | language, language-server | The frontend's recursion overflows the caller's stack on a ~500-byte file | done |
| P1-23 | high | language-parser | The parser's 16 MiB worker overflows at 175 nesting levels | wontfix:D9 |
| P1-24 | high | language-parser | The AST's own recursive `Drop` overflows on a deep tree | wontfix:D9 |
| P2-1 | medium | language, language-server | `BufferSession` is built but unwired; rustdoc claims otherwise | todo |
| P2-2 | medium | highlevel, language, language-server | Five hand-written AST traversals; one with a wildcard arm | todo |
| P2-3 | medium | highlevel | `Build` is a god-DTO with four parallel vectors | todo |
| P2-4 | medium | lowlevel | `Node`'s `pub` fields break the documented write choke-point | todo |
| P2-5 | medium | highlevel | `NativeApply` is an unvalidated escape hatch | todo |
| P2-6 | medium | language | README generator and `clap` live in the compiler library | todo |
| P2-7 | medium | lowlevel | `visiting` is set by hand, bypassing the `Drop` guard | todo |
| P2-8 | medium | highlevel | `missing_slots[order_index()]` guarded only by `debug_assert!` | done |
| P2-9 | medium | highlevel | `no_attr_ext` panics on any annotated program | done |
| P2-10 | medium | highlevel | `check_term` recursion is unbounded; `stacksafe` is an unused dep | done |
| P2-11 | medium | all | God files with named seams | todo |
| P3-1 | medium | all | Duplication clusters | todo |
| P3-2 | medium | all | Workspace manifest duplication | todo |
| P3-3 | medium | ci | No test/clippy/fmt gate in CI | todo |
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
| P5-4 | low | compute | `wasm-encoder` 0.258 vs wasmi's `wasmparser` 0.228 | todo |
| P5-5 | low | language-lex | `~` overflow silently saturates to `usize::MAX` | done |
| P5-6 | low | package | `build.rs`'s `.git/HEAD` trigger never fires in a worktree | done |
| P5-7 | low | package | `lichen path language-server` pollutes stdout | done |
| P5-8 | low | package | Generated `Cargo.toml`: TOML injection and a Windows path escape | todo |
| P5-9 | low | language | `io::Error` modelled as a `(0,0)` source diagnostic, 10 sites | todo |
| P5-10 | low | render, language-server | Unguarded parent walk and unchecked index on the render hot path | todo |
| P5-11 | low | registry | `virtual:` file IDs can never verify | done |
| P5-12 | medium | workspace | A worktree nested in the checkout breaks `cargo metadata`/`fmt` for `tree-sitter-lichen` | done |

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

### P1-4 — `hash_inner` cycle token vs `key_eq` coinduction `reported`

`table.rs:178-180` cuts a cycle on **node identity plus depth**; `table.rs:248-259`
cuts on the **unordered pair**. `A = [1, A]` and `B = [1, [1, B]]` are equal
under `key_eq` but hash differently; a self-referential universe crossing the
static/dynamic boundary terminates at different depths on each side. Since the
hash only *finds candidates*, a hash disagreement is an unconditional miss.
`tests/basic/table.rs:258` covers only the symmetric case.

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
stays lazy (`Parameterized`). `Build::ok` already required
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
**Residual, deliberately left:** applying a *non-kernel* struct value through a
deferred callee (`f = g => g 1` applied to a struct instance) is still silently
accepted — the lowlevel cannot tell that array from a kernel's, and the kernel
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

`crates/lichen-language/src/cli.rs:232` stages a file's `depend`/`plug`
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
$ cargo run -q -p lichen-language -- p.lichen
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

### P1-22 — The frontend's recursion overflows the caller's stack on a ~500-byte file `verified`

Found while fixing `P2-10`, which guarded the checker's own recursion. With
`check_term` guarded, a **source** file still aborts the process, and the file is
tiny:

```
$ python -c "print('('*250 + '1' + ')'*250)" > deep.lichen    # ~500 bytes
$ cargo run -p lichen-language -- deep.lichen
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

### P3-2 — Workspace manifest duplication

No `[workspace.package]`, `[workspace.dependencies]` or `[workspace.lints]`.
`version = "0.1.0"` and `edition = "2024"` are repeated in **every** manifest
(17 workspace members plus the two standalone crates), and each `lichen-*` path
dependency is spelled out at each use site. A release bump means editing ~14
files by hand.

**Fix.** `[workspace.package] version/edition`, `[workspace.dependencies]` for
the shared externals and the internal path deps, `version.workspace = true` per
crate. Then `P3-3` can hang lints off the same table.

### P3-3 — No test/clippy/fmt gate in CI

`.github/workflows/build.yml` only runs `cargo build --release --locked`. No
`cargo test`, no `clippy`, no `fmt --check`, and there is no lint policy
anywhere (`cargo clippy --workspace --all-targets` currently reports **56
warnings**). `dev` has no quality gate, which is why the warnings are there.

**Fix.** Clean the 56 warnings, add `[workspace.lints]`, and add a CI job
running `fmt --check` + `clippy -D warnings` + `test`. Note the baseline: the
test suite compiles (`cargo test --workspace --no-run` succeeded) and the suite
is 15.1k lines against 31.5k of source.

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
  this crate produces is `Some`; `language/src/readme.rs`'s claim that
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
- **P5-9 `reported`** — `language/src/package.rs:233, 241, 253, 261, 362, 603,
  631, 810, 829` and `cli.rs:235` report an `io::Error` as a *source* diagnostic
  with a fabricated `(0, 0)` span, which `render.rs:109-116` prints as a caret
  at line 1 column 0. There is no `Stage::Io`, so a permission error and a syntax
  error are indistinguishable to a consumer.
- **P5-10 `reported`** — `render/src/render.rs:869` indexes `kind.items()[0]`
  with no length check (its sibling `kind_is_struct` at `:1034` checks), and
  `:932-941` walks `equality.parent` unguarded, uncompressed and cycle-unsafe,
  forking `disjoint::find`. Both run from `Doc::new`, i.e. every keystroke.
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
- **`docs/README.md`'s index is complete** — all 33 notes are linked and no link
  is dangling (the disagreement is the one status cell, `P5-2`).
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
