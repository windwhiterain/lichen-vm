# Code audit and remediation queue

> Status: current — the audit inventory below was taken at `dev@e4c0bae`; the
> queue is the work list, and each item's `Status:` field is the live state.
> Items found later say which `dev` they were seen at in their own evidence.
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
| P1-17 | high | language-server | Every request runs the whole frontend | done (Outcome below; (b) still open) |
| P1-18 | high | compute | Unbounded global registries; per-launch wasm rebuild; unbounded `plrun` | done |
| P1-19 | medium | lowlevel | `evaluate_block` expects a return the budget may refuse | done |
| P1-20 | low | package | `download` uses a predictable shared temp name and skips `fsync` | done |
| P1-21 | medium | lowlevel, highlevel | A struct value applied through a deferred callee is still silent | done |
| P1-22 | high | language, language-server | The frontend's recursion overflows the caller's stack on a ~500-byte file | done |
| P1-23 | high | language-parser | The parser's 16 MiB worker overflows at 175 nesting levels | wontfix:D9 |
| P1-24 | high | language-parser | The AST's own recursive `Drop` overflows on a deep tree | wontfix:D9 |
| P1-25 | high | package | A dependency's package name is written as Rust source | done |
| P1-26 | high | language | The table-key hash changed meaning without an artifact version bump | done |
| P1-27 | high | registry | A leaf name longer than 255 bytes desynchronises the artifact stream | done |
| P1-28 | medium | language-parser, language | One AST walk is unguarded, and a caller runs it on the caller's stack | done |
| P1-29 | medium | compute, registry | A compute value reaching the artifact codec panics | done |
| P1-30 | low | compute | A refused `plrun` count is silent | done |
| P1-31 | medium | lowlevel | The deep-pass verdict conflates "never ran" with "in progress" | done |
| P1-32 | medium | language-parser | A run of separators is refused inside every list form | done |
| P1-33 | medium | highlevel, language | A self-recursive call in a conditional's branch is refused as "expected Int, found Int" | todo |
| P1-34 | medium | highlevel, language, docs | The spec and `check_index` disagree about `e[i]` on a tuple or a struct | done (D16: the spec was the stale half) |
| P1-35 | medium | language, highlevel, lowlevel | A raw read `X<e>` of a runtime container yields `none` with no diagnostic | done |
| P1-36 | medium | highlevel | A duplicate kind-marker tag shadows a codec arm and warns instead of failing | todo |
| P1-37 | high | compute, graph-ir | The graph registry freezes the backend of the first graph of a shape | done (fixed on `feature/gpu-algorithms`; each pin is a test that fails without it) |
| P1-38 | high | compute | A decided non-buffer at a read, a collect or a `cfg` position is answered `parameterized` | done (fixed on `feature/gpu-algorithms`; each pin is a test that fails without it) |
| P1-39 | medium | compute | A `compute.call` inside a **parallel** kernel body is refused with a `NodeId`, so the device has no working call route | todo |
| P1-40 | medium | lowlevel | The apply budget refuses a long **terminating** loop as non-terminating | todo |
| P2-1 | medium | language, language-server | `BufferSession` is built but unwired; rustdoc claims otherwise | done (wired: the server's compile worker, `incremental-update.md` §7.6) |
| P2-2 | medium | highlevel, language, language-server | Five hand-written AST traversals; one with a wildcard arm | done |
| P2-3 | medium | highlevel | `Build` is a god-DTO with four parallel vectors | done |
| P2-4 | medium | lowlevel | `Node`'s `pub` fields break the documented write choke-point | done |
| P2-5 | medium | highlevel | `NativeApply` is an unvalidated escape hatch | done |
| P2-6 | medium | language | Repo tooling (README generator, `sync-readme`) inside the compiler library | done |
| P2-7 | medium | lowlevel | `visiting` is set by hand, bypassing the `Drop` guard | done |
| P2-8 | medium | highlevel | `missing_slots[order_index()]` guarded only by `debug_assert!` | done |
| P2-9 | medium | highlevel | `no_attr_ext` panics on any annotated program | done |
| P2-10 | medium | highlevel | `check_term` recursion is unbounded; `stacksafe` is an unused dep | done |
| P2-11 | medium | all | God files with named seams | done |
| P2-12 | medium | language, package, ci | `clap` is linked by every consumer of the compiler library | done |
| P2-13 | medium | lowlevel, utils | Node state is still writable through the node table and `disjoint::Meta` | done |
| P3-1 | medium | all | Duplication clusters | done |
| P3-2 | medium | all | Workspace manifest duplication | done |
| P3-3 | medium | ci | No test/clippy/fmt gate in CI | done |
| P3-4 | medium | span, language, language-server | Four byte↔line/col implementations with divergent edge behaviour | done |
| P4-1 | medium | lowlevel | Registry read lock + `Arc` clone per array element | done |
| P4-2 | medium | lowlevel | `write_node_value` is O(class size); seven sibling full-list walks | done |
| P4-3 | medium | language-parser | A 16 MiB thread and a rebuilt combinator graph per parse | done |
| P4-4 | medium | highlevel, language | O(E×D) diagnostics; O(diags×lines) rendering | done |
| P4-5 | low | lowlevel, compute | `path.contains` as a cycle guard; O(n²) kernel codegen | done |
| P4-6 | low | lowlevel, language, compute | Per-apply clones, repeated `as_enum`, per-byte `mix`, intern leak | done |
| P4-7 | low | lowlevel | `apply_errors` is deduped with a linear scan | done |
| P4-8 | low | render | Two more ancestor guards scan the path they guard | done |
| P4-9 | low | compute | A `NativeOps` slice is leaked per registration | done |
| P4-10 | medium | render | The type printer recurses per type depth and overflows at ~200 | wontfix:D9 |
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
| P5-14 | low | docs | Citations invalidated by the file splits | done |

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
--- x = depend "--upload-pack=<command>" ---
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
`Error`; `Error` propagates silently (it is the residue of an already-recorded
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
exactly once, blamed on the callee node, `Error` yielded), and
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
one-element array — and the importer `---x = import "pkg.lichen"---x` panicked at
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
importer `---f = import "pkg.lichen"---f 2`, the apply's assert clone fires in the
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

**Outcome — (a) and (c) are in, (b) still is not, and measuring them found a
crash the item had been standing on.** The first half is a plain refactor with a
measured payoff; the second half is a defect in the artifact cache, not in the
editor, and it was reachable from the CLI too — nothing about it is
editor-specific.

**The mechanism.** `Doc` is split in two. The `Send` half is `DocIndex` (the
frontend artifacts, the name-resolution index, and the diagnostics *rendered*
for the protocol); the `!Send` half is the `Doc<P>` handle, which adds the
checker's structured diagnostics and derefs to the index. That split is what
makes a cache possible at all: `tower_lsp` requires `Send + Sync`, and `Doc`
owns arena-bound raw pointers through the checker's diagnostic type, so the
server could never have held one. The server now holds a `DocIndex` per open
document and builds the `!Send` handle only inside the analysis.

The cache is keyed by **the text's own sha256, not the client's version** — a
version is only as injective as the client that sends it, while the text's hash
is injective by construction, and the client number is still not published, so a
client cannot reject a stale diagnostic either way. The second half of the key
is **the sha256 of every imported file the run loaded**; a hit requires all of
them unchanged, since the on-disk artifact cache is shared with other processes
(`docs/notes/artifact-cache.md`). A run that failed to resolve or read a package
is **not** cacheable: no set of *existing* files names the one whose appearance
or repair would change the answer, so a hit could go on reporting a failure the
editor has already fixed. The device registry handle is kept open across
requests (`PackageStore::with_device` / `into_device`) and taken out for the
duration of a run, so two overlapping runs cannot share one handle.

The keystroke path gets a **150 ms debounce** and a **generation gate**: an edit
is analyzed only after the window without a newer edit, and an analysis publishes
only while the text it was launched for is still the document's current one, so
a superseded analysis stops instead of completing. Neither can abort a run that
has already started — `tower-lsp` answers `$/cancelRequest` by dropping the
request future, and a `spawn_blocking` closure is detached from its join handle,
so the run finishes and its result is discarded. That is stated in the module
docs rather than papered over: the gate suppresses a *stale publish*, which is
the part a user sees, and does not buy back the work.

**The numbers** (release builds, same machine, 30 warm hovers and a 10-edit
burst per run; the "before" binary is this branch's own `HEAD` built out):

| | before | after |
|---|---|---|
| hover, 120-statement document | **7.01 ms** median | **0.040 ms** median (≈175×) |
| hover, `examples/import/_.lichen` (two imports) | **1.76 ms** median | **0.128 ms** median (≈14×) |
| 10-edit burst → frontend runs | **10** | **1** |
| 10-edit burst → last diagnostic at | 75 ms | 172 ms |
| `didOpen` → first diagnostic | 7.1 ms | 7.8 ms (unchanged — it was always one run) |

The burst row is the honest cost: the last diagnostic now arrives ~100 ms later
because the edit waits out the debounce instead of being analyzed immediately.
That is the trade being made deliberately — ten frontend runs per burst become
one — and it is a latency-for-throughput swap, not a free win. The unchanged
`didOpen` row is the control: the first analysis of a document still costs a full
run, which is exactly what the cache is not for.

**Every answer is unchanged.** 506 responses — hover, definition and completion
over a grid of positions across the import example, plus the semantic-token
payload and a full diagnostic set after an edit — are byte-identical between the
two builds. A caching change that cannot be shown to answer the same thing is
not worth landing, so this is the assertion that matters more than the timings.

**The crash the measurement found.** The stale-dependency check in the bench —
open a document that imports `math.lichen`, hover, change `math.lichen` on disk,
hover again — **killed the server**, on this branch's `HEAD` as much as on the
new code, with `invalid SlotMap key used` from the union-find
(`lichen-utils/src/disjoint.rs:126`), surfacing as
`compile lichen source: JoinError::Panic` at the `spawn_blocking` join. With the
repo's own `examples/import` shapes it is a hard crash; with smaller ones it is
silently *wrong* instead — the same file, the same sources, differing only in
whether an artifact cache is warm, produced two spurious errors
(`this value is not a container`, `table lookup missed`) against none.

The cause is not in the editor. An artifact's identity was
`sha256(own_source, dependency_keys)`, and a recompile **reuses a dependency's
`ModuleKey`** ("recompiles reuse it, overwriting the slot") — so a dependency's
key survives its own change, and an importer whose own bytes never changed kept
its identity, and therefore kept its frozen artifact. That artifact is full of
cross-module node references written as `(dependency key, index)`; served after
the dependency changed, those indices name the dependency's **new** node layout.
Wrong answers, or an index that is no longer a slot. The `verify` walk does not
catch it, and cannot: a dependency that was *already recompiled earlier in the
same store* matches its record again. This is a **pre-existing defect on `dev`**
(`git show dev:…/device.rs` carries the identical function), and the note's own
claim that dirtiness is transitive — "a dependency change changes the importer's
hash" — was simply false.

**Fixed, at the identity rather than at the symptom.** The fold now takes each
dependency's `(key, identity)`, where a dependency's identity is what *it* was
published as, so the closure is genuinely transitive. Three sites answer "what
does this dependency contribute" and all three read the **registry's own
record**, never the caller's in-memory view — an embedded dependency has no
source file and need have no record, and it must contribute the all-zero
sentinel on both sides or every dependent would miss its cache forever. That
last point was not a prediction: the first version read the store's in-memory
registry on the write side and the device record on the read side, and the
pre-existing test `a_package_that_imports_an_embedded_source_verifies_across_stores`
caught it as a permanent miss. `Entry` now records the identity per file ID
(registry format version 3; a version-2 file reads as unreadable and is recovered
as a fresh registry, costing one full recompile). Pinned by
`lichen-language/tests/artifact_transitivity.rs` and the fold itself in
`lichen-registry/tests/artifact_hash.rs`, and **watched to go red**: with the
identity removed from the fold the transitivity test fails `left: (0, 2)` —
compiled 0, served 2 — exactly the stale-artifact behaviour. See
[artifact-cache](artifact-cache.md).

Two smaller findings from the same work, both worth more than the timings:

- **The bench's restore check was wrong, not the server.** It compared two whole
  responses, `"id":200` against `"id":202`, so it reported `STALE` for a server
  that had in fact returned to the earlier answer exactly. A harness that cannot
  pass is worse than no harness: it would have sent the next reader looking for
  a bug in the code. Fixed to compare the `result` object.
- **A warm cache is a correctness dependency, not just a speed one.** Every
  measurement in this item is conditional on the importer chain being rebuilt
  correctly, and until the identity fix that was not true. The item as written
  (a cache plus a dependency key) is a *cache*; it only became safe once the
  dependency key was made a function of the dependency's content.

**Still open.** (b) — `BufferSession`, to avoid the lex and parse on the
keystroke path — is untouched and remains blocked on T3, the memoized check;
skipping the check without it is not a win. `did_save` /
`did_change_watched_files` are still absent, which is why the dependency half of
the key is load-bearing rather than a belt-and-braces extra: an editor that
never tells the server a file changed leaves the server to notice by hashing, on
the next request. Document version tracking is also still absent, so a client
cannot reject a stale diagnostic set; the text hash is what makes the *cache*
sound, and it is not the same thing as a version the client can compare against.

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

**Outcome — three claims, three verdicts: two fixed, one a redesign.** The
note's line numbers had drifted (the file splits and the later compute items);
every citation below is re-derived.

**Claim 1 — the registries: verified, and it needs an owner rather than a
bound.** `KERNELS` (`compute.rs:91`) and `BUFFERS` (`:106`) are
`OnceLock<Mutex<HashMap<…>>>` whose only writers are the two compile arms
(`:348`, `:458`) and `ParLaunch` (`:535`); a workspace grep finds no `remove`,
`clear`, reset or LRU, and `KernelId`/`BufferId` are `pub type … = usize`, so a
value carries a bare `Copy` index. Measured: **one distinct program evaluation
adds exactly one kernel fragment and one buffer**, monotonically — 60
evaluations took the registries from 179 to 239 kernels and 2 to 62 buffers.

*Why a bound is not the fix.* What makes an entry unreachable is "no live
`Kernel`/`ParKernel`/`Buffer` value references it", and the registry cannot
observe that: the id is `Copy` and is copied into node value caches, equality
classes, apply clones and (through the frozen wrapper) static modules, nothing
reports the last copy dying, and the arena has no per-value `Drop` to hang a
release on. So eviction could only guess, and the first eviction of a live id
turns a later `launch`/`read` into the lazy marker — a **silent wrong answer**,
worse than the leak. A bound that *refuses* new entries instead never drops a
live one, but it permanently bricks a long-lived host at N programs and turns a
working program's answer into the lazy marker, which is a functional
regression rather than a memory bound. The answer needed an owner, and `D15`
found it: buffers moved into the **block arena** (a `Copy` handle, owned by the
block, with no registry and no id at all) and kernels stayed process-global with
**content-addressed** ids. See the `D15` entry for the measurements, including
the one that killed the `Arc`-in-the-value shape — 70 errors across 13 lowlevel
files, because `Copy` is a vocabulary-wide trait bound and not a per-variant
property.

**Claim 2 — the per-launch rebuild: verified, fixed, and much smaller than the
note says.** `run_kernel` and `run_parallel_kernel` each built
`wasmi::Engine::default()` + `assemble_module` + `Module::new` + `Linker::new`
per call with only the fragment cached. A **derived-module cache** now maps
`(LaunchMode, KernelId)` to `(Engine, Module)`, bounded at 64 entries with
oldest-first eviction.

*The key is everything the module depends on.* The mode owns the fragment set
(the root's relative launch set vs one parallel fragment), and the root id is
sufficient because a registered fragment is immutable, so a key's assembly is
fixed for the process's life. Eviction is sound only because the entry is
*derived*: it can always be rebuilt from the fragment, so dropping it cannot
lose anything a value refers to (exactly what is not true of claim 1's
entries).

*What that key did not do, until `D15`.* The id was process-unique and minted per
compile, so **two compiles could never share an entry** — a repeat *launch* of
one kernel hit, but an unchanged program recompiled (every keystroke) missed, and
the bounded cache spent its 64 entries on garbage. Kernel ids are
content-addressed now, so the entry is per *distinct kernel* rather than per
compile; measured on the same `jit`+`launch` program compiled three times in one
process, module-cache misses went from `+1/+1/+1` to `+1/+0/+0`. The paragraph
above used to end "so two programs can never share an entry" and call that a
property; it was the defect.

*The `Engine` is cached with its module, deliberately.* wasmi's default
`CompilationMode::LazyTranslation` validates eagerly and translates each
function on first use into the **engine's** code map, which is append-only and
freed only with the engine — so a shared process-global engine would make this
cache a second unbounded accumulator, and per-entry engines are what make
eviction actually free the translated code. The `Linker` is deliberately **not**
cached: it is the object that carries host-function bindings, keeping it out
makes "no host binding is shared between two launches" true by construction,
and its cost is the two fixed `env.read`/`env.write` registrations. Instances
are never cached either — every launch still makes a fresh `Store` and
instantiates, so two launches share compiled code and no state.

*Measured* (release, one binary, 40 alternating rounds with a temporary cache
kill-switch, so the cache is the only difference between the two runs; the
input is `k = compute.jit (y => y + 1)` plus 41 statements
`r{i} = compute.launch k {i}` and root `r0`; this machine varies ~3% run to
run):

| probe | without the cache | with it |
|---|---|---|
| marginal per launch (2nd..41st), round 1 | 81.06 µs | 76.72 µs (**−4.34 µs, 5.4%**) |
| marginal per launch (2nd..41st), round 2 | 80.95 µs | 77.82 µs (**−3.13 µs, 3.9%**) |
| the 41-launch program (round 1) | 5.248 ms | 5.123 ms (−2.4%) |

The prediction from the step breakdown below is 4.5–5.4 µs per launch, so the
measurement matches the mechanism.

*The note's "single largest optimisation opportunity" is refuted.* One
sequential launch, step by step (minimum of 200 rounds, release):
`Module::new` (validation; translation is lazy) **3.7–4.5 µs**, the first call
(lazy translation + execute) 1.1–1.2, `assemble_module` 0.7–0.8, instantiate
0.4–0.5, `Engine::default` 0.1. Through the whole pipeline a launch *statement*
costs 75–88 µs against 21–23 µs for a plain `r{i} = {i} + 1` statement — a
launch-specific 53–65 µs of which the **entire** wasmi share is 4–6 µs
(measured by skipping the rebuild and the run behind a temporary switch:
88.4 → 82.4 µs and 74.5 → 70.8 µs). `run_kernel` runs exactly once per launch
statement (41 calls for 41 statements), so the remaining ~50 µs is the checker
applying the frozen `launch` wrapper — `launch = k => a => $launch(k.native,
k.sig, a)`, two curried applies — which is `P4-6`'s per-apply-clone territory
and which this cache does not touch. The change moves the number 4–5%, not by a
factor.

**Claim 3 — the `plrun` count: verified, fixed with a refusal.**
`run_parallel_kernel` took the program-controlled `cfg(0)`
(`LowValue::USize`), allocated `vec![0i64; count]` and made one wasm call per
element, uncapped; it is reachable from **checking**, because the checker's
statement pass evaluates every top-level statement
(`checker.rs:714-739`). The bound is `MAX_PARALLEL_ELEMENTS = 1 << 20`
(1,048,576 elements = 8 MiB at the limit, one kernel call per element ≈ 190 ms
measured), checked **before** the allocation.

*Behaviour at the bound: refuse — never truncate, never queue.* Truncating
would be a wrong answer, and there is nothing to queue onto: `plrun` is a
synchronous, caller-blocking call, so the choice is refuse or run. The refusal
returns `Err` and the caller's existing arm turns it into the lazy marker.

*Measured.* Count `2^24` (16,777,216) was **accepted in 2790.9 ms** and left a
128 MiB vector in the then-current buffer registry before; after, the same
program is **refused in 2.7 ms** and allocates nothing. At the bound 1,048,576
is accepted (`0: Int`, 193.7 ms) and one past it (1,048,577) is refused in
2.74 ms. `count = 4` is unchanged (`0: Int`), as are the 24 `compute` tests and
the `compute_jit.lichen` example. (That registry is gone — `D15` moved buffers
into the block arena — so "the registry is unchanged" is no longer a thing this
can be checked against; the refusal, the bound and the timings are what the
measurement established.)

*Residual — taken up and fixed by `P1-30`.* The refusal had **no message**: the
observable answer was `parameterized: Int`, this plugin's channel for every
runtime refusal. The note below correctly declined to invent a variant locally
(`Module::eval_errors` is a closed enum of structural value facts, and every
`BudgetExhausted` variant renders *"this binding never terminates"*, which is
false for a buffer size the user chose). `P1-30` then found the channel that
already existed for it — `Module::extension_diagnostics`, which this plugin was
*already* recording `$jit`'s refusals on, with no reader anywhere — and gave it
one; a refused `plrun` now names both the count and the limit.

**Also in this item's body, and deliberately not changed.** The fourth
paragraph (module docs and operator names advertising a data-parallel `plrun`)
was already refuted by `P5-3`'s outcome: `:41-45` describes `parallel`'s
type-level effect and `plrun`'s index range, and `run_parallel_kernel`'s own
doc says v1 runs sequentially. What remains is the operator *names*, a
language-surface change that no item owns.

**Verification.** `cargo clippy --workspace --all-targets -- -D warnings` exit
0; `cargo fmt --all -- --check` exit 0; `cargo test --workspace` exit 0 (72
targets, 765 passed, 0 failed); `cargo test -p lichen-language --test compute
--test examples` 24 + 1 passed. The measurement probes (a registry-size
accessor, a launch-call counter, the cache kill-switch and a launch-step
breakdown) and `crates/lichen-language/tests/probe_p1_18.rs` were removed
before the commit.

### P1-19 — `evaluate_block` expects a return the budget may refuse `reported`

Found while fixing `P1-2`, same class, second site: `evaluation.rs`'s
`evaluate_block` ends with `.expect("evaluated return node")` after
`evaluate_node_deep(root, None)`. A depth refusal returns `Error` before
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

`unwrap_or(value)`, not `unwrap_or_else(Error)`, because the two no-cached-value
cases answer differently and the pass already said which: a refusal returns
`Error` *before* `evaluate_node` and never ran anything (its budget verdict is
already recorded, so the return is a **propagation** of that verdict, not a
second report), while a lazy block returns `Parameterized` and must stay lazy —
yielding `Error` there would forge an empty value (the residue of a
recorded failure) out of a legitimate "try again later", which readers like the
`TableGet` arm act on. Both markers are leaf values owned by no arena, so
neither needs the relocation `garbage_collect` exists to perform; the postlude
writes every arena-carrying answer, which is why "no cached value" implies the
pass's answer was one of the two leaves.

**Tests.** Both triggers are pinned in
`crates/lichen-lowlevel/tests/basic/evaluation.rs`:
`a_block_root_the_budget_refuses_yields_an_empty_value` (limit 2, the
refusal lands on the child block's root; asserts the recorded
`BudgetExhausted::EvaluateDepth` and an `Error` result) and
`a_block_root_that_stays_lazy_is_not_an_internal_error` (an unbound operand
makes the child block's root stay `Parameterized`; asserts the result is
`Parameterized`, not `Error`). Both were confirmed to fail against the unfixed
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

### P1-27 — A leaf name longer than 255 bytes desynchronises the artifact stream `verified`

Found while measuring `P4-6`; the mechanism is verified first-hand, and the
reachability is answered below.

`crates/lichen-registry/src/codec.rs:42-45`:

```rust
pub fn leaf(&mut self, name: &str) {
    self.u8(name.len() as u8);
    self.bytes(name.as_bytes());
}
```

The length is written as a **`u8`** — `as u8` truncates rather than failing — while
the bytes written are the **whole** name. `Reader::leaf_name` (`:99-102`) then
reads the truncated length and takes that many bytes, leaving the remainder of the
name in the stream. Every field after it is read from the wrong offset, so the
artifact does not fail to parse; it **misparses**, which is the worst of the two
outcomes for a container whose whole contract is "total validation or a clean
error" (`P0-1`, `P0-5`, `P0-7`).

**Outcome — the premise held in full, the reachability question answers
"closed vocabulary", and the fix is the refusal.** Re-derived first-hand before
any change: `name.len() as u8` truncates, `leaf_name` reads the truncated count,
and a probe confirmed the consequence rather than inferring it.

**Reachability: the name is a closed vocabulary, so a long one is a compiler
bug, not source text.** Every producer of a leaf name is a `w.leaf(...)` call in
`crates/lichen-language/src/program.rs`'s `lang_compose_vocabulary!` expansion
(`:393`, `:399`, `:407`, `:473`, `:480`, `:488`) — there is no other caller of
`Writer::leaf` in the workspace. Each passes `stringify!` of a macro parameter
bound by `$name:ident` in the manifest (`program.rs:95-97`, `:131-132`), i.e. the
Rust identifier a plugin declares for its carry variant (`LowValue`, `TypeValue`,
`ComputeValue`, `GcdOp`, …). No binder, string literal, user-defined operator or
any other source-derived text reaches it: the name is fixed at compile time by
the plugin set, so a name past 255 bytes cannot be produced by any program a user
can write. That makes the defect **latent** — but the writer wrote the full name
anyway, so any future leaf whose identifier is long (a composed vocabulary is
generated text) would have shipped a silently misparsed artifact. The
`u8`-widened-to-`u32` half of the fix was rejected on this answer: it would be a
format change (`ARTIFACT_FORMAT_VERSION` 5 → 6) paying to widen a field no
producer can overflow, where a refusal keeps the format and removes the failure
mode.

**The fix.** `Writer::leaf` returns `Result<(), String>` and refuses a name
longer than `MAX_LEAF_NAME_BYTES` (`codec.rs:12-21`, `:64-79`) instead of writing
a truncated length; the writer also records the first refusal, and the new
`Writer::finish` (`:82-88`) hands back the buffer or that error, so a refused
name can never be handed out as bytes. `into_bytes` (`:94-98`) stays for the two
writers that write no leaf name (the artifact *header*, the registry file) and
documents why it cannot fail there. The refusal propagates through the codec's
write side, which is what the read side already looked like:
`ArtifactCodec::write_value` and `write_operator` now return
`Result<(), String>` (`persist.rs:129-152`), the generated `ProgramCodec` uses
`?` on each `w.leaf(...)` (`program.rs:383-415`, `:469-501`), and
`serialize_artifact_with` / `serialize_artifact` return
`Result<Vec<u8>, String>` (`persist.rs:226-244`). The package store's one
production caller maps it to a spanless `Diag` (`package.rs:578-585`) —
reachable only by a future vocabulary with an over-long identifier, never by a
source file. No `assert!`/`panic!` is added on the length path.

**Test** (`crates/lichen-registry/tests/codec.rs`, new; `D3` permits the
minimal falsifier): `a_leaf_name_the_length_field_cannot_hold_is_refused` and
`a_leaf_name_the_length_field_holds_round_trips` (255 bytes must still
round-trip). Run against the unfixed tree through a removed probe, a 256-byte
name wrote **257 bytes** (`length byte 0` + all 256 name bytes) and
`leaf_name()` returned **0 bytes** with **256 bytes left unread** — the
desynchronisation, exactly. `cargo test -p lichen-registry` passes 5/5 with the
fix; `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo fmt --all -- --check` exit 0, and the artifact round-trip gates
(`-p lichen-language --test persist --test registry --test examples --test
compute`) pass.

### P1-28 — One AST walk is unguarded, and a caller runs it on the caller's stack `verified`

Found while splitting files for `P2-11`, and it **corrects a claim `P2-2`'s Outcome
makes**. `P2-2` lists `collect_error_blocks::walk_expr`
(`crates/lichen-language-parser/src/parse.rs`, now `parse/error_blocks.rs`) as one
of the seven AST walks and says all seven carry `#[stacksafe]`. That is wrong:
**`lichen-language-parser` does not depend on `stacksafe` at all** (its manifest
lists `chumsky` and `lichen-language-lex` and nothing else), so that walk has never
been guarded.

It has survived because its in-parser call site sits inside the parser's 16 MiB
worker, where the stack is deep enough. But `crates/lichen-language/src/session.rs`
calls it too, and that call is on the **caller's** stack — a 1 MiB thread — which
is exactly the arrangement `P1-22` fixed for the other six.

**Latent rather than live:** the `session.rs` path is `BufferSession`'s splice,
which `P2-1` records as built but unwired. So this becomes reachable when `P2-1`
wires the session — which is decision `D6`'s (b) step. Fix it before or with that
wiring, not after.

**Fix.** Add `stacksafe` to `lichen-language-parser`'s manifest and annotate the
walk's recursive entry point, matching what `P1-22` did for the other six. Do not
change the parser's worker or its stack size (`P1-23`, `wontfix:D9`).

**Outcome.** The premise held as written, re-derived first-hand before the
change.

*The three facts.*  `crates/lichen-language-parser/Cargo.toml` listed `chumsky`
and `lichen-language-lex` and nothing else, so `stacksafe` was not in that
crate's dependency graph at all.  The walk's two functions are nested in
`collect_error_blocks` (in
`crates/lichen-language-parser/src/parse/error_blocks.rs`): `walk_expr` calls
`walk_stmt` for a block and `walk_stmt` calls back into `walk_expr`, so every
cycle re-enters at `walk_expr`, which is the hub `P1-22`'s rule names.
And the crate's only out-of-parser caller is
`crates/lichen-language/src/session.rs:524`, the `BufferSession` splice that
`P2-1` records as built but unwired; the in-parser call site (`parse.rs:164`)
does sit inside the 16 MiB worker, as the item says.

*What landed.*  `stacksafe.workspace = true` in the parser's manifest (the
workspace root already pins `stacksafe = "1"`), `use stacksafe::stacksafe;` and
one `#[stacksafe]` on `walk_expr` — the re-entry hub, exactly the arrangement
`P1-22` gave the frontend's six walks.  No other function is annotated, and
`walk_stmt` needs none: it is only reachable from `walk_expr`, so the cycle is
covered by the hub.  The parser's worker, its 16 MiB stack and the absence of a
depth limit are untouched (`P1-23`, `D9`).

*What could be pinned, and what could not.*  The guard changes no output — the
`Vec<ErrorBlock>` a deep program yields is identical with and without it — so no
behavioural assertion can falsify its absence, and the only path that reaches
the walk on a shallow stack is the unwired session.  What was pinned is the
abort, following `P1-22`'s pattern:
`crates/lichen-language-parser/src/tests/parse_tests.rs`'s
`the_recovered_error_walk_does_not_overflow_a_shallow_caller_stack` parses a
2000-term `1+1+…` chain (flat in the token stream, left-nested in the AST) and
runs `collect_error_blocks` on it on a 128 KiB thread, standing in for the
caller's stack.  Against the unfixed tree the test binary dies with
`STATUS_STACK_OVERFLOW (0xc00000fd)` — a process abort, not an assertion, which
is the limit `P1-22` recorded for the same kind of pin — and with the attribute
it passes in 0.07 s.  What could **not** be pinned is the session path itself:
driving `splice_program` to that depth would exercise code no production caller
reaches while `P2-1` is open, and the test's 128 KiB thread is an explicit
stand-in rather than any real caller's stack.

*Gates.*  `cargo clippy --workspace --all-targets -- -D warnings` exits 0,
`cargo fmt --all -- --check` exits 0, `cargo test --workspace` passes, and
`cargo test -p lichen-language --test pipeline --test examples --test persist
--test registry` passes.

### P1-29 — A compute value reaching the artifact codec panics `done`

Found while fixing `P1-18`, read but not reproduced, so `reported`:
`ComputeValue::write_value` **panics** on `Kernel`, `ParKernel` and `Buffer`. The
codec's own comment calls that an invariant violation, which is true only if no
such value can reach a freeze.

The reachable shape is a package that `jit`s at its **top level** and is then
**imported**: the importer's freeze walks the imported module's values, meets the
kernel, and panics. `P1-18` did not establish that this is reachable in practice,
and neither does this note — **establish it before fixing**, because the answer
decides whether the fix is a diagnostic or a codec change.

**Fix.** A compute value that cannot be serialized must be refused with a
diagnostic naming the value, not a panic inside a codec — the same standard
`P0-1`/`P0-5`/`P0-7` hold the container to. If the codec *can* legitimately meet
one, it needs an encoding; if it cannot, the panic is in the wrong layer and the
refusal belongs where the freeze decides what to serialize.

**Outcome — reachable, and the refusal belongs to the cache, not the compile.**
Reproduced on the first try, and the shape is exactly the predicted one:

```text
kernels.lichen:  k_double = compute.jit (y => y + y)
_.lichen:        compute.launch kernels.k_double 3
```

The importer's freeze of `kernels.lichen` walks its values, meets the live
kernel, and the process dies at `compute.rs:275` with
*"serializing a compute value (Kernel/ParKernel/Buffer are runtime-only)"*; the
exit code is 101 and no diagnostic is printed. Nothing about it is exotic — the
package does not have to `jit` *alone* to be imported, it only has to `jit` at
its top level.

**The codec *cannot* meet one, so this is a wrong-layer panic.** A kernel is a
process-local registry handle: `KERNELS` is keyed by a `usize` that means
nothing in another process, which is what `D15` is about. There is no encoding to
add. The honest question is what the *caller* does, and the answer is decided by
a measurement the note had not made: **the program is valid and runs.** The same
`jit` at the top level of a *single* file evaluates to `6: Int` today, because
only packages are frozen and serialized — the main program is not. So refusing
the compile would turn working lichen into an error to protect a cache.

That leaves "refuse the cache", and the store already has the shape for it:
every other failure in `build_package` degrades to a miss rather than failing the
build. `serialize_artifact_with` is now allowed to fail and the artifact write +
`publish` are simply skipped, which leaves the pending device entry `alloc_key`
wrote unpublished — and an unpublished entry can never verify, so the package is
uncached *permanently* rather than recompiled into a stale artifact later. The
importer of such a package is uncached too, transitively, because
`verify_entry` walks to a dependency whose `source_hash` is still all-zero.

**The bug underneath was a half-finished refactor, not a design error.** The
outer `ArtifactCodec::write_value`/`write_operator` already returned
`Result<(), String>` — `P1-27` made them fallible for a leaf name that overflows
its discriminator — but the *inner* per-leaf `ValueCodec`/`OperatorCodec` traits
were left infallible, so the leaves that can genuinely refuse had no way to say
so and panicked instead. Both traits are fallible now, all seven leaves
propagate, and the two panicking compute arms refuse by name. The same change
retired **three more panics of the identical kind** in `LowValue::write_value`:
a dynamic array/table/function payload in a module being serialized was also an
`assert`-by-panic, and a dynamic payload in a module that reached the writer is
the same "cannot cache this" fact, not a broken invariant.

Pinned by `crates/lichen-language/tests/runtime_only_package.rs`, which is the
reproduction: the imported-kernel program must *run* (exit 0, `6: Int`), which is
red on a panic and red again if the refusal is ever turned back into a failed
build.

### P1-30 — A refused `plrun` count is silent `done`

`P1-18` bounded `plrun`'s element count (`MAX_PARALLEL_ELEMENTS = 1 << 20`) and
chose to **refuse** rather than truncate, which is right — but the refusal returns
an error that the caller's arm turns into the lazy marker, so the user sees
`parameterized: Int` and no diagnostic. A program asking for 2²⁴ elements is told
nothing about why it got nothing.

**Fix.** Give the refusal a diagnostic. `P1-18`'s Outcome records why it did not:
`eval_errors` is a closed enum of *structural* facts and every `BudgetExhausted`
variant renders *"never terminates"*, which would be false here. So the fix is a
variant in the right place, not a reuse of the wrong one — decide which channel
owns "a resource limit was reached", and if that means extending the budget enum,
say so.

**Outcome — the channel already existed; what was missing was its reader.**
`Module::extension_diagnostics` is exactly "a layer above the lowlevel decided
something the lowlevel cannot describe", and `compute` was **already recording
the `$jit` refusal on it** (`compute.rs:404`) — but nothing anywhere read it, so
*both* refusals were silent. The decision the item asked for is therefore not a
new channel but the one the tree already has, and `compiler-plugin.md` had
already stated the contract that settles it: *"What a host renders from these
entries is the host's own decision."* The host is `lichen-language`, and it now
renders them — at the two points where a `Module` outlives its check, the report
assembly (`lib.rs`, the failure path) and `run::render_build` (the run path,
where a `plrun` actually executes). `BudgetExhausted` is left alone: it is the
*non-termination* verdict, and all three of its renderings say "this binding
never terminates", which is false for a buffer size the user asked for and can
lower.

**A refusal is an explanation, so it is reported only when there is something to
explain.** The first version reported every recorded refusal and broke
`jit_cross_kernel_subexpr` — a *working* program. The checker evaluates
speculatively, so a `$jit` whose parameter domain is not decided *yet* records a
refusal, and a later attempt with the domain known compiles that very kernel; the
program then produces `7: Int`. The line is the outcome, not the channel: a
refusal explains a value that never arrived, and a program that produced one has
nothing to explain. That rule also gives the polymorphic-`jit` case its designed
behaviour for free — `jit` of a template whose domain stays undecided produces
the lazy marker *and* now names the reason, which is what Phase 3c asked for.

**One refusal is one diagnostic.** The channel is append-only, and a node is
deep-evaluated several times (the checker walks the top-level statements and then
the root; the run walks the root again), so the over-limit `plrun` first reported
**three times**. `record_extension_diagnostic` now drops an entry identical to
one it already holds in `(category, node, message)` — refusing twice is not two
findings. A linear scan is the right shape there, unlike `apply_errors`, which is
recorded per apply and keeps a set beside it.

Pinned by `a_refused_plrun_count_says_why` in `crates/lichen-language/tests/
compute.rs`, which asserts one diagnostic naming both the count asked for and the
limit; with the reporting disabled it fails with the old symptom, `expected this
program to fail: "parameterized: Int"`.

### P1-31 — The deep-pass verdict conflates "never ran" with "in progress" `done`

`Node::evaluated_deep: Option<EvaluatedDeep>` is documented as a two-state fact:
`None` means the deep pass never ran on the node, and a reader "must treat [it] as
parameterized, **never** as proven concrete" (`module.rs:200-213`,
`lib.rs:882-889`). Two reads honour that (`evaluation.rs:285-287`,
`is_none_or`, and `freeze.rs:71-73`), and `function.rs:325` is conservative by a
different route (`is_some_and(|e| !e.parameterized)`, where `None` means *not
proven*, so the node is cloned). **Three did not**: the verdict's own array arm
(`evaluation.rs:739-745`), table arm (`:754-764`) and operand arm (`:766-774`) all
use `is_some_and(|e| e.parameterized)`, so `None` reads as *not* parameterized;
`Module::key_state` does the same after forcing the key
(`table.rs:217-222`). Two of those three reads are gone in the follow-up at the end
of this item — the operand arm, and the forced key read with it — so the two value
arms are the whole of the verdict computation now.

The two arms reached by the **value descent** are non-conservative on purpose, and
the reason is a third state `Option` cannot express. The canonical universe is a
self-referential array — `write_node_value(universe, Array([type_marker, universe]))`
(`checker.rs:844-854`) — so the descent reaches `universe` while its own frame
holds the `visiting` mark, the structural-cycle cut returns **without writing a
verdict** (`evaluation.rs:593-597`), and the verdict computation then reads that
`None`. Read conservatively, the universe would be flagged parameterized, and
`checker.rs:627-632` states the consequence: the apply clone machinery would clone
it, "creat[ing] a fresh self-loop that unification cannot equate with the
canonical one" — a path-guard conflict on the `Type : Type` spine. So *"in
progress, assumed concrete"* is a real state, and it is deliberately read as
concrete.

The defect is that the same `None` also meant *"never ran"*, and the fix is to
**name the third state** rather than to flip a read.

**Fix (landed).** `Node` gains a private `assumed_concrete` flag: set where the
structural-cycle cut returns (`evaluate_node_deep_inner`), cleared where the real
verdict is written and where a late operand edge invalidates one
(`Module::close_operation_cycle`). Every verdict read inside the verdict
computation now goes through one helper, `ref_is_parameterized`, whose rule is
**the assumption fills a missing verdict and never overrides one**:

```rust
match entry.evaluated_deep {
    Some(deep) => deep.parameterized,
    None => !entry.assumed_concrete,
}
```

`None` therefore means exactly "the pass never ran here", and a position no frame
is computing reads unproven. Behaviour is unchanged for the coinductive cases (a
direct self-reference and a longer cycle stay concrete) and changes in exactly one
direction: a subtree the pass **refused on** — a depth refusal returns before it
writes (`evaluation.rs:599-625`) — no longer certifies its parent.

**Verified, with the pins.** `crates/lichen-lowlevel/tests/basic/verdict.rs`:
`a_refused_subtree_leaves_its_parent_unproven` fails on the old read
(`parameterized: false` where the fix gives `true`), and
`a_cyclic_value_is_proven_concrete` pins the coinductive case the change must
preserve. The lowlevel suite (139 + 3) and `lichen-highlevel`, `lichen-language`
and `lichen-compute` all pass.

**Two sites an earlier draft of this item named are *not* defects — retracted,
with the reason each.** *(The first retraction is itself superseded: the operand
arm was later deleted outright, see the follow-up at the end of this item. The
second stands.)*

- **The operand arm** (`value_is_parameterized`) is a deliberate exemption, and
  flipping it is not a local fix. A core operator's operand is the argument array
  a layer above synthesized, and the deep pass descends value-reachable edges
  only, so "this operand was never walked" is the *normal* case; reading it as
  unproven would turn every pair read (`Index(pair, 0)`) in a template unproven
  and clone all of them per apply. The reason is now stated at the site, and
  `an_operand_the_pass_never_walked_certifies_the_node` pins both the exemption
  and the harm question below.
- **`Module::key_state`** is correct as it stands, for a reason the draft missed:
  the content unfolding is **total** — it cuts at `UNFOLD_DEPTH` and reports its
  own failure as `TableKeyUnbound` (`table.rs:1-70`) — so a key with no verdict is
  still hashable, and gating it is *too* conservative. Three cyclic-key tests
  (`table::cyclic_keys_hash_and_compare_equal`,
  `coinductively_equal_cyclic_keys_hash_equal_across_depth`,
  `table::a_cyclic_key_is_found_across_the_static_boundary`) fail if that read is
  gated, which is how the retraction was found; `table.rs` keeps the read with a
  comment saying why.

**The harm half, answered for the shape that could show it.** The approved
regression test builds the body `p => [1, p](0)` — a pair whose element 1 depends
on the parameter and whose element 0 is the literal the indexed read takes — and
asserts that the read is certified concrete although its operand was never walked,
*and* that two calls with different arguments both yield `1`. So the exemption
does not corrupt the second call in the shape that exercises it: the baked node's
cached value is call-independent and nothing re-reads the parameter-dependent
sibling. That is a pin, not a proof that no shape is harmed.

**Follow-up — a verdict can outlive what it waited on** (`feature/deferred-instantiate`).
*(Superseded: the operand arm this follow-up patched is deleted outright, below.)*
The operand arm's exemption reads `Some(parameterized: true)` as unproven, and
nothing cleared it when the operand chain *did* resolve: `write_node_value`
caches a resolved value on the operation's own slot, and the forced pass skipped
the operand walk for any node that already held a value
(`if force_operand && self.nodes[node].value.is_none()`).  A node that was walked
while it still waited therefore kept reading unproven forever — visible where a
reader treats the verdict as "not decided yet": `Module::key_state` gates a
`TableGet` on it, so a lookup keyed by a resolved read stayed lazy and the read
it stood for never ran.  The forced pass now re-forces the operand of an
**unproven** node as well as of an unevaluated one
(`evaluation.rs`, next to the arm above), which recomputes the verdict bottom-up
from the values that have since arrived.  It re-walks only nodes flagged
unproven, so no proven node's verdict or baking changes; the lowlevel,
`lichen-highlevel` and `lichen-language` suites and `examples` are green after
it.  This is what the deferred named instantiation's supplying lookup needs, and
why its per-field type check is that lookup's own key comparison rather than a
unify (`crates/lichen-highlevel/src/checker/structs.rs`,
`Checker::lazy_named_instantiate`).

**Why this mattered beyond the contract.** `docs/notes/incremental-evaluation.md`
needs `parameterized` to be a function of the graph rather than of where a walk
started — a dirty-flag recomputation restarts elsewhere — and it needs the deep
pass's redundancy to be worth removing. That note's step 0 measured the redundancy
at 1.7–8.2 node-evaluations per decided node over six shapes, worst on the
canonical cyclic ones. The counters that took those numbers were **temporary** and
have been removed again — the `deep_pass_stats` / `reset_deep_pass_stats`
re-exports and the `deep_pass_stats` example with them — on the same rule this
queue's `P4-1` records ("a temporary counter … removed after"). The numbers are
the record; the instrument is not. The pass's share of a build's wall-clock is
still unmeasured.

**The operand arm and the operand forcing are gone**
(`experiment/verdict-operand-arm`, landed). The arm read the verdict of the
operation's `operand` node — a **static graph edge**, so `evaluate_node_deep`
never enters it, and the only writer of a verdict there is a *forced* pass. The
parent's verdict was therefore a function of which walk happened to run, not of
the graph: with the arm's exemption (a missing verdict reads as fine) an operand
verdict written by one walk leaked into another walk's verdicts, and the stale
`Some(true)` that survived its operand's resolution is exactly what the follow-up
above had to re-force. A temporary probe on the pre-change tree counted the arm's
condition: **10 firings in the whole corpus**, all inside
`compute::a_scalar_leaf_of_the_wrong_class_is_refused_by_name`, on ten
`op/operand` pairs. Four changes, each measured over the whole corpus
(`lichen-lowlevel` 155, `lichen-highlevel` 87, `lichen-language` 139 plus its
other targets, `lichen-compute` 19), green at every step:

| step | change |
|---|---|
| A | the operand arm is deleted from `value_is_parameterized` |
| B | `Module::key_state` runs `evaluate_node_deep`, not the forced walk |
| C | the forced pass drops the follow-up's `unproven` re-force |
| D | `force_operand` is deleted from `evaluate_node_deep_inner` |

The two public walks now differ in exactly one knob, `skip_shallow`; the forced
one is the assert check's, and its only remaining extra work is descending
shallow-marked positions.

**Why it is sound, beyond the tests.** A `LowValue` is a *computed answer*, not a
thunk: a decided value cannot depend on an operand its operator did not read, so
the arm could only ever add unproven-ness for a dependency the answer does not
have. Every dependency that *can* change an answer sits in the value graph, where
`ref_is_parameterized` reads it — an unbound cell is an empty position, and a
shallow position is unproven by its own flag.

**What is *not* established.** Flipping a verdict from unproven to proven is the
direction that can wrong-share (bake instead of clone), and the corpus is not an
oracle for it. The differential harness `incremental-evaluation.md` §5 step 2 asks
for — values, verdicts and diagnostics compared over the corpus — does not exist,
so this is measured support rather than proof. The note's §4.3 obligation about
operand verdicts is void with the arm, which makes the cut it plans easier rather
than riskier.

### P1-32 — A run of separators is refused inside every list form `verified`

**Two documents promise the opposite of what the list parsers do.** The parser's
own module doc (`parse.rs:19-20`) says the three separator characters are one
token *"and consecutive, leading, and trailing separators are all tolerated"*;
`language-spec.md` §2 says *"the quantity never matters … a run of them (a blank
line, a stray trailing separator) is tolerated"*, and adds that a newline between
two elements of a tuple means the same as a comma. The statement level does
tolerate runs — `seps`/`seps1` are `Separator.repeated()`
(`parse.rs:300-308`, `parse.rs:1442-1446`) — which is why a program may have blank
lines between statements. **The list forms do not**, and each one is a different
combinator shape:

| form | site | separator shape |
|---|---|---|
| `$f(…)`, `A(…)` | `comma_list`, `parse.rs:1093-1100` | `(Separator item)*` then `Separator?` |
| `(a, b)` | `paren`, `parse.rs:1145-1152` | same |
| `[a, b]` | `array_literal`, `parse.rs:1196-1203` | same |
| `<a, b>` | `angle_tuple`, `parse.rs:1279-1288` | `(Separator expr).at_least(1)` — no trailing tolerance either |
| `struct<…>` | `struct_type`, `parse.rs:1326-1331` | `(Separator field)*` — no trailing tolerance either |
| `array<T, n>` | `array_type`, `parse.rs:1350-1355` | exactly one `Separator` |

So a *lone* separator between elements is fine — `(1\n2)` is `(1, 2)` and
`s(1\n2)` instantiates — and a **run** is not. In `paren` the reason is visible in
the shape: the fold's own separators are consumed one at a time, so the comma goes
to the *optional trailing* `Separator` (`parse.rs:1150`), and the newline that
follows is then unexpected where `)` is required. The diagnostic therefore points
one token past the comma, which is where the user is not looking.

Reproductions at `dev@a972a79`, first-hand (`cargo run -p lichen-compiler -- <file>`):

```text
(1,
2)             error: expected '!', an expression, or ')', found a separator        --> 1:4
[1,
2]             error: expected '~', '!', an expression, or ']', found a separator   --> 1:4
struct<a,
b>             error: expected '.', '!', or an expression, found a separator        --> 1:10
array<Int,
2>             error: expected '!' or an expression, found a separator              --> 1:11
s(1,
2)             error: expected '.', '!', an expression, or ')', found a separator   --> 1:27
```

**Not this item:** a separator immediately *after* an operator or the table arrow
(`table { 1 ==>\n 2 }`, `1 +\n2`) is refused for a different reason — the
documented "an expression cannot continue across a separator" rule, also in §2. A
fix here must not make those valid.

**Fix direction — not a decision.** Give each site the statement level's
treatment: a *run* between items (`Separator.repeated().at_least(1)`) and a run
after the last (`Separator.repeated()`). `repeated()` rewinds a failed iteration,
so the optional trailing run still leaves the enclosing closer to match. `needs-test`:
a multi-line tuple and a multi-line `array<…>` (the exactly-one site) are the
narrowest proof.

**Outcome.** Every list form takes the statement level's treatment, through one
pair of combinators — `separator_run` (`Separator.repeated()`) and
`separators_between` (`Separator.repeated().at_least(1)`), `parse.rs:378-393` —
which replaced the six hand-written shapes *and* the two local `seps`/`seps1`
copies the statement forms already had (`region_inner`'s `:305-306`,
`block_body`'s `:1484-1485`), so the rule now has one implementation in the
crate rather than eight spellings of it. The sites: `comma_list` (`:1121`,
which `$f(…)`, `A(…)` and `table { … }` share), `paren` (`:1176`),
`array_literal` (`:1226`), `angle_tuple` (`:1312`), `struct_type` (`:1356`) and
`array_type` (`:1387`, the exactly-one site). All six reproductions above are
first-hand fixed on this tree: `(1,\n2)` is `(1, 2): <Int, Int>`, `[1,\n2]` is
`[1, 2]: array<Int, 2>`, `array<Int,\n2>` is `array<Int, 2>: TypeArray`,
`struct<Int,\nstring>(1,\n"a")` is `(1, "a"): struct<Int, string>`,
`A = struct<.x Int,\n.y Type>` instantiates as `A(.x 1,\n.y Int)`, and
`table { 1 ==> 2,\n3 ==> 4 }` reads back `4`.

The struct line's *parser* result is what this item measured, and the newline
tolerance it pins is unchanged; under the later named-field rule that same
program is refused at check time, the named spelling
`struct<.f Int,\n.g string>(1,\n"a")` being `(1, "a"): struct<.f Int, .g string>`.
See the addendum to `P1-34`'s outcome.

**The discriminator had to move with the grammar.** `comma_list`'s
`saw_comma` — what splits the single-argument positional slot read `A(1)` from
the instantiation `A(1,)` — counted a *trailing token*, so it became a trailing
*run*: `!trailing.is_empty()` instead of `trailing.is_some()` (`parse.rs:1139`).
`paren`'s grouping-vs-tuple test moved the same way (`:1194`). That is what
keeps `A(1,\n)` an instantiation and `(1,\n)` a tuple, both pinned below. No
leading-run term was needed for either: the loop pairs every run with the item
that follows it, so a run can never be left over at the front — which is also
why `A(\n1, 2)` parses as an instantiation (`comma_list`'s first item is
optional, so a leading run is consumed by the loop) while `[\n1, 2]` does not
parse at all.

**The flip side is untouched, deliberately.** `1 +\n2`, `table { 1 ==>\n2 }` and
`x =>\n x + 1` are still parse errors, re-measured on this tree: a run *inside*
a list is a separator between items, and the "an expression cannot continue
across a separator" rule is a different rule. The parser's module doc now says
both, so the next reader does not take the first for the second.

**Tests** (`crates/lichen-language-parser/src/tests/parse_tests.rs`):
`a_run_of_separators_inside_a_list_form_is_tolerated` walks all six sites —
including the two the item named as the narrowest proof, the multi-line tuple
and the multi-line `array<…>` — plus the trailing-run half of the
discriminator. It was **watched to go red**: with the fix stashed it fails at
`parse_tests.rs:9`, *"unexpected parse errors"*. `an_expression_cannot_continue_across_a_separator`
is the negative guard, and it passes before *and* after by construction — its
job is to catch a later widening of the list forms, not this fix.
`lichen-language-parser` is 47/47 (45 before this item, plus these two);
`lichen-language`'s `pipeline` (127),
`examples` (all 23), `readme` and `preprocess`, and the four
`lichen-language-server` suites are unchanged.

**A leading run is still refused, and that is the note's own boundary.**
`[\n1, 2]`, `struct<\nInt, string>` and `array<\nInt, 2>` do not parse, while
`A(\n1, 2)` does — not by a decision but because `comma_list`'s first item is
optional and the other five require one. The fix direction above prescribes a
run *between* items and *after* the last, and the repro table is entirely
between-item runs, so leading tolerance was not added: it is a grammar widening
the audit never claimed, and the statement level needs it for a reason that has
no analogue inside a bracket the user is in the middle of typing (a file may
open with a blank line). Adding it is one `separator_run()` per site if a later
pass decides the pretty-printed form `[` newline `1,` newline `2` newline `]`
should work.

### P1-33 — A self-recursive call in a conditional's branch is refused `reported`

`reported` for the *site*, not for the symptom: the refusal reproduces first-hand,
the cause is **not located**.

```text
f = a => b => if b == 0 then a else f b (a - b)
f 48 18
```

`dev@a972a79` answers `error: expected Int, found Int` — no span line, both sides rendered
as the same type name.

**The escape hatch is to annotate the *function*, and that is what makes this an
inference defect rather than a missing feature.** The same body prefixed with
`: Int -> Int -> Int` checks and runs (it then exhausts the apply budget, as
`-` on a wrapping `Int` must); annotating the *parameters* instead
(`f = a => b => { a : Int; b : Int; … }`) still fails with the same message. So the
program is well-typed, the checker cannot get there on its own, and what it
reports is a type conflict between two nodes that both mean `Int` — a message
that names the wrong thing. (`examples/gcd.lichen` writes the function's type out
for the same reason, in the tuple-domain spelling.)

Narrowing, each run first-hand against the same binary (`dev@a972a79`):

| shape | result |
|---|---|
| `g = a => b => a - b; f = a => b => if b == 0 then a else g b (a - b)` | runs (`Int`) — a non-recursive two-argument callee in the branch is fine |
| `f = a => b => [f b (a - b), a][b == 0]` | refused identically — so it is not the `if` desugaring, which is exactly this array index |
| `f = a => b => f b (a - b)` | runs (and exhausts the apply budget, as it must) — a self-reference alone is fine |
| `f = x => if x == 0 then x else f (x - 1)` | runs — single-argument self-recursion in a branch is fine |
| `f = (a => b => if b == 0 then a else f b (a - b)) : Int -> Int -> Int` | **checks and runs** — the function's type written out is the whole difference |
| `f = a => b => { a : Int; b : Int; if b == 0 then a else f b (a - b) }` | still refused — pinning the parameters is not enough |

So the trigger is a **self-referential call as an element of the conditional's
array**, with a curried two-argument function on both sides of it — and the
checker gets there only while the function's own type is *unwritten*.

**The lead, and it is only a lead:** both sides print `Int`, and `Int` does not
fail to unify with `Int`, so the two sides are two *different* nodes that each mean
`Int`. The annotation is the interesting half: writing the function's type out
decides which nodes the recursive reference's domain and codomain cells *are*, and
the failure disappears — so what is wrong is which cell the recursive call's
argument is compared against, not the types involved. The place to look is the
interaction between a block-wide (self-referential) binding's pre-registered
skeleton and the per-apply parameter clones (`lowlevel::function`, and
`checker/operators.rs`'s pin of an `Int` operand to `self.int_type`). Nothing here
has read that path; this is a starting point, not a diagnosis.

**Located, not fixed — re-measured at `dev@cbf4fcc`, and the lead above was
wrong about which side is at fault.** It is not the checker's cells at all: the
diagnostic is `DiagKind::Runtime` with `loc: None` — an **apply-time parameter
check in the VM**, not a checker's pin. The two sides it compares evaluate to
`USize(48)` and `USize(18)`, i.e. the *values* the two arguments were given at
the call site, and they print as `Int` only because a `Type`-typed value is
rendered by its type.

The trigger, narrowed first-hand (the earlier table above all holds; `if c then
t else e` lowers to `[e, t][c]` — the branches are swapped, `compile.rs:754` —
so "the array index" and "the `if`" are one form):

| shape | result |
|---|---|
| `f = a => b => [f b a, 0][b == 0]` | **refused** — the minimal form |
| `f = a => b => [0, f b a][b == 0]` | runs — same call, other element, so the *branch* is never taken |
| `f = a => b => [f b a, 0][0]` | recurses (budget) — a literal index does not trigger it |
| `f = a => b => [f b a, 0][c]`, `c = 0` | recurses — nor does a constant index |
| `f = a => b => [f b a, 0][b - b]` | **refused** — any computation over the parameter does |
| `f = a => b => [f b a, 0][b == b]` | runs — that one evaluates to 1, so the other branch is taken |
| `f = a => b => [f a b, 0][b == 0]` | recurses — passing the parameter to the *second* argument is fine |
| `f = a => b => [f b, 0][b == 0]` | **refused** — one curried apply is enough |
| `f = a => b => [a, 0][b == 0]` | runs — no recursive call |

So: the index expression must **read a parameter**, the selected element must be
a **self-recursive apply passing that same parameter to the function's first
parameter**, and the function's type must be unwritten. Everything else in the
report above is downstream of that.

**Where it goes wrong, traced.** Instrumenting `apply_parameter_check`
(temporary, reverted) on `f = a => b => [f b a, 0][b == 0]; f 48 18` shows
three applies, and the third is the wrong one:

```
APPLY fn=1v1 … template_param=19v1 leaf=Parameterized   argument leaf=USize(48)
APPLY fn=3v1 … template_param=96v1 leaf=Parameterized   argument leaf=USize(18)
APPLY fn=5v1 … template_param=162v1 leaf=USize(48)      argument leaf=USize(18)   <- refused
```

`1v1` is `f` and `3v1` the inner `b => …`; **`5v1` is a fresh closure clone**,
and *its declared parameter already holds 48* — the first argument of the
enclosing call, not the 18 this apply is checking. The unify is
`unify(cloned_param, argument)` at the pair's leaf: 48 against 18. So the
statement to take forward is: **when an apply instantiates the curried closure
that `f`'s body will apply again, the clone's parameter is seeded with the
enclosing apply's argument**, which is exactly the class of defect the note's
lead guessed at, one level down from where it guessed. The candidate sites are
`function.rs`'s closure branch in `value_apply` (`:464-562`: the fresh id is
registered with the *source* parameter at `:485-492` and re-pointed at the
remapped clone at `:517`/`:558`) and `regroup_clones` re-establishing the
template's class topology among the clones (`function.rs:179-187`) — both of
which can seed a fresh parameter from a source that is already bound.

**Why this is recorded and not fixed.** The two candidate sites are the VM's
closure-instantiation and class-regrouping paths, where a wrong answer is a
silently wrong *value* rather than a crash; the audit has no `lowlevel` test
that pins a curried closure's per-call parameter (`tests/basic/evaluation.rs`
covers recursion, not this), and this item's own scope is the queue's. A fix
here is its own item with its own regression test, and the diagnosis above is
what that item needs to start from. The lead paragraph's guess — the checker's
skeleton and `check_binop`'s pin — is refuted: nothing the checker does is
involved, and the parameter annotation's only effect is which argument the
definition pass happens to be holding.

**What wants this fixed, and what it is not.** A `loop` operator for kernel
bodies — `loop f n`, applying `f` `n` times — is the shape that wants this, and
it is *the* thing standing between the language and an unrolled loop in a kernel.
Its own status, and the two ceilings a static expansion runs into instead, are in
[`gpu-algorithm-roadmap.md`](gpu-algorithm-roadmap.md#41-axis-b-already-in-the-language-and-what-it-does-not-reach)
§4.1; `P1-39` is the other half, because in a kernel the operator also needs the
body applied before it is lowered, and `P1-40` is what a *dynamic* loop removes by
construction. **[Loop conversion](loop-conversion.md) is the design that reaches
the kernel case, and it supersedes the `loop f n` surface this entry names** — a
`loop` is a one-node cycle in its general form, and the natural formulations this
item would have to annotate (Euclid below, `mutual_recursion.lichen`) are not of
the form `T -> T` repeated `n` times. It does not fix this item: the marker
supplies no type, so the annotation is still the escape for a two-argument
self-reference on the **host**. Today the only working form is the annotation this
entry records as the escape:

```text
loop = (f => n => x => if n == 0 then x else loop f (n - 1) (f x))
     : (Int -> Int) -> Int -> Int -> Int
inc = x => x + 1
(loop inc 3 0, loop inc 10 5, loop inc 0 7)   -- (3, 15, 7): <Int, Int, Int>
```

**The blast radius is wider than loops, and the repository's own example is
written around this bug without saying so.** The trigger is not "recursion" and
not "loops" — it is a **self-referential two-argument call inside a conditional's
branch**, and the most natural way to write a two-argument recursive algorithm in
a curried language is exactly that. Euclid, unannotated:

```text
gcd = a => b => if b == 0 then a else gcd b (a % b)
gcd 48 18
-- expected Int, found Int
```

The same body annotated checks and runs (`6 : Int`) — so the escape applies, and
Ackermann, a two-parameter fold and a two-parameter tree walk are all in the same
place. **`examples/gcd.lichen` dodges this twice over**: it takes a **tuple**
parameter rather than two curried ones *and* writes the type out —

```text
gcd = (p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))) : <Int, Int> -> Int
```

— and its `doc` attributes the annotation to a *different* reason ("under
self-recursion a call's result is its own type cell"). So the annotation is
documented as an inference nicety while it is also what keeps the example from
being refused, and a reader following the natural curried form gets a diagnostic
that names two identical types. Worth recording because **the shape of the
example is carrying a constraint the note does not state**, and the cost is paid
by every program written the obvious way rather than by the one that is shown.

### P1-34 — The spec and `check_index` disagree about `e[i]` on a tuple or a struct `verified`

**The code says `[i]` is arrays only, deliberately.** `check_index`
(`checker/indexing.rs:39-52`) pins the container's type to a *fresh array type*
with `DiagKind::Guard`, and its doc says so in as many words (`:19-28`): a
concretely non-array container — *"a tuple, a struct, a function, a table"* —
fails here, and *"tuple and struct slots are read with the dedicated positional
form `a(k)`"*, because *"the operator and the type extraction are chosen by
syntax, never by a runtime kind dispatch"*. The examples follow that: the tuple in
`examples/index.lichen` is read `b(0)`/`b(1)`, never `b[0]`.

**The spec says the opposite, twice.** `language-spec.md` §3, *Indexing*: *"`e[i]`
reads the `i`-th element of an array, **tuple, or struct instance** (a struct
instance's positional fields are its wrapped tuple's elements)"*; §3, *Struct
instantiation*: *"Indexing an instance reads its positional fields: `s(1, 2)[0]`
is the first field, and its type is the corresponding field type (an out-of-bounds
field index is an `IndexOutOfBounds` diagnostic)."*

Reproductions at `dev@a972a79`, first-hand:

```text
(1, 2)[0]                            error: expected array<Int, Int>, found <Int, Int>       --> 1:1
s = struct<Int, string>(1, "a"); s[0] error: expected array<Int, string>, found struct<Int, string>  --> 1:5
```

`(1, 2)(0)` works and yields the field. `s(0)` does **not** any more: the paren
read is the *tuple* read, so a struct instance is refused there (see the
addendum to this item's Outcome below).

**Which side is wrong is a call, so this is `blocked:D16`** and neither side was
changed. The evidence leans one way — the code's intent is explicit, its doc
explains *why* (syntax picks the operator), and every example agrees with it, so
the spec's paragraph looks like the stale half — but "the spec is the single
source of truth for the language" is this project's own rule, and a language that
*should* index tuples is a feature, not a doc fix. Both are cheap; picking one is
not this note's to do.

**Outcome — `D16` decided: the spec is the stale half, and the code did not
move.** `docs/language-spec.md` §3 (*Indexing*) now says `e[i]` reads the `i`-th
element of an **array**, that a tuple's and a struct instance's positional slots
are read with `a(k)` or `s.x`, and that the reason is the one `check_index`
already documents — *the operator and the type extraction are chosen by syntax,
never by a runtime kind dispatch* — so `e[i]` over a concretely non-array
container is a diagnostic rather than a silently different read. Its
non-indexable list is now the whole one (a tuple, a struct, a function, a table,
an atomic type), which is what `check_index`'s doc already said. §3 (*Struct
instantiation*) drops *"Indexing an instance reads its positional fields:
`s(1, 2)[0]` is the first field"* for the positional spelling `s(1, 2)(0)` and
says outright that `s(1, 2)[0]` is not that read.

Nothing in the checker, the parser or the examples changed, so the two
reproductions in the report still answer `expected array<Int, Int>, found
<Int, Int>` and `expected array<Int, string>, found struct<Int, string>` —
now because the spec says so. A reader who wants `(1, 2)[0]` to be `1` is asking
for the language `check_index` would have to be taught, which is a new item
rather than a reopening of this one; the shape that answer would need (what a
*struct's* positional index means, given that `a(k)` resolves through the
struct's name table while the spec's answer was "its wrapped tuple's elements")
is recorded in `D16` below.

**Later addendum — the positional spelling is gone too, and struct fields are
named.** The tuple-only positional read (`check_field`, commit `a356ae6`)
narrows D16's answer one step further. `check_field` now pins an *undecided*
container to a fresh tuple type and refuses a *decided* non-tuple, both with
`DiagKind::Guard`; the requirement prints as that open tuple, `<?a, …>`. So
`a(k)` is the **tuple** read and nothing else, `s(1, 2)(0)` is refused exactly as
`s(1, 2)[0]` is, and a struct instance reads by name (`s.x`, `X::a`). The same
change requires **every struct field to be named** (`DiagKind::StructFieldName`:
`struct<.name T>`, or `name = e` in a block), so the anonymous `struct<Int,
string>` spellings recorded earlier in this file — the comma-list reproduction
above, P1-34's own reproduction and P1-35's table below — no longer check. Their
measured outputs are kept as the record of the tree they were taken on; the named
spelling of the same programs is the one that runs now.

Two consequences worth naming for the next pass. A *struct-returning block* is
**not** affected by the name rule: its fields are its bindings, and a bare
expression is an ordinary statement — checked, its value discarded — so
`{ 1; x = 2 }` is a record with the single field `.x` rather than a record with a
positional first field. And with an empty struct type (`struct<>`) and an empty
block (`{}`)
both unspellable, every reachable struct type now has at least one field and all
of them named, so
the name-table-less `Error` struct marker — and `DiagKind::StructAnonymousField`,
the `.name`-argument-against-no-names-table error — is **unreachable from
source**; it survives only for hand-built IR.

**The raw form is not an available substitute, and that is its own item.** The
tempting answer — "spell it `s<0>`" — does not work on a runtime container: see
`P1-35`, where `[1, 2]<0>` and `(1, 2)<0>` read a container that is not a tuple
*type value* and are refused — today **at check time** (`expected TypeTuple,
found array<Int, 2>` / `found <Int, Int>`), and on the tree this passage was
written on as a **reported** runtime error (before that, `none: none`
silently — see that item's Outcome). `X<e>` reads a
component of a *type-as-value* (`<Int, string><0>` is `Int : Type`); over a
runtime array or tuple it produces no value.

### P1-35 — A raw read `X<e>` of a runtime container yields `none`, silently `reported`

`reported` for the *site*, as in `P1-33`: every output below was produced
first-hand on `dev@a972a79`, the mechanism is not located.

The read is specified as working over any container: `docs/notes/raw-index.md:24-33`
says it reads a component of a type-as-value **and** that "an unbound container (a
parameter, a call result) stays lazy and resolves at the apply, which is what makes
it usable generically: `f = k => k<0>` reads the first field of whatever type `k`
is applied to". Its documented failure mode for a non-positional or out-of-bounds
read is *"a **runtime** lowlevel `Index` error, never a static diagnostic"*
(`:25-27`). What happens instead:

| program | output at `dev@a972a79` |
|---|---|
| `<Int, string><0>` | `Int: Type` — works (this is the documented use, and `examples/raw_index.lichen` pins it) |
| `struct<Int, string><1>` | `string: Type` — works |
| `[1, 2]<0>` | **`none: none`** |
| `x = [1, 2]; x<0>` | **`none: none`** |
| `(1, 2)<0>` | **`none: none`** |
| `x = (1, 2); x<0>` | **`none: none`** |

(Kept as the record of that tree.  The last four rows refuse at check time on the
current tree — see the later addendum at the end of this item.)

So the read over a *runtime* container produces no value and no type, and no
diagnostic of any kind: not the runtime `Index` error the note promises, and not a
value. The output line is literally `none: none`.

Not located. All that is visible from outside is that the CLI has a *value* to
print, and it prints it, so whatever happened is not a diagnostic on the path it
takes. Two candidates, and they are on either side of a boundary this pass did not
cross: the read never lowers to an `Index` that can succeed over a value pair, or
its evaluation records something (`Module::eval_errors`) that the report does not
carry. **The second is checkable without a compiler change** — compile `[1, 2]<0>`
and read `Module::eval_errors` — which is why the item's site is `reported`; that
check has not been run.

`needs-test`: whichever mechanism it is, the smallest falsifier is a program whose
value is `[1, 2]<0>` and whose output is `1: Int` — or, if the intended answer is a
refusal, one that reports *something* rather than printing `none`.

**Outcome — located, and it is the second candidate.** `Module::eval_errors` is
empty at the end of a `[1, 2]<0>` build, `ok` is `true`, and the recorded
`IndexTarget` only appears when the *host* then reads `Build::root_val` — i.e.
after the build has already decided it was fine. The cause is
`check_raw_index` (`checker/indexing.rs:110`): it stored the **raw read
operation** as the expression's `term` instead of a `[value, type]` pair, and
left `val` to be derived. Two consequences, both measured:

- `Checker::value_of` derived the value as element 0 of that term, so the read's
  *value* was the element's own value slot — right exactly when the element is
  itself a pair, which is why the type-as-value form worked and
  `[1, 2]<0>`, whose element is an `Int`, read element 0 of a scalar.
- the definition pass evaluates `root_term` (`checker.rs:741`), so with a bare
  read operation as the term it evaluated **only the element** and never the
  type slot read. That failure was recorded, but outside the window
  `Build::ok` is decided in (`checker.rs:770-773`, and `root_val` is computed
  at `:774`, after it) — hence `none: none` with nothing reported.

**The fix is the pair every other expression's term is.** `element_read`
(`:145`) now builds the element, its two slot reads, and the pair of them, and
returns that pair plus the element; both raw reads — `check_raw_index`
(`:110`) and `check_raw_named_field` (`checker/structs.rs:114`), which had the
identical shape — store it. The build therefore evaluates both slot reads, the
recorded failure lands inside `ok`'s window, and `[1, 2]<0>`, `x = [1, 2]; x<0>`
and `(1, 2)<0>` are reported instead of printed.

**Later, and in the other direction — the type slot.** The read's *type* slot is no
longer left lazy: `X<e>` and `X::a` **compute** it when they are checked, so a later
check compares the read's type instead of binding it. Measured on
`S = struct<.a Int, .b string>`: `S::a == 1` and `S::a : Int` were both *accepted*
(printing `0` and `Int: Int`) and are now `BinOp` and annotation refusals, while
`S::a == Int` is `1` before and after — it passed for the wrong reason before. The
type is also per-field-kind, not a constant: `struct<.a struct<.b Int>>::a` is
`struct<.b Int>: TypeStruct` and `(S::a : Type)` over it is refused exactly as
`(S : Type)` is ([raw-field.md](raw-field.md#check-time-not-raw)). Two consequences
for the paragraphs above: the two slot reads are evaluated because the read **is
built**, and a read the kind requirement refuses is built **not at all** — it carries
the hole a refused definition carries (`Checker::refused_pair`). That is also what
keeps the refused container's name-table walk from reaching the lowlevel's
`unreachable!("TableGet target must be a table")`: the walk is only built for a
container that passed the kind check, so `DiagKind::RuntimeRawElement` sits behind
that check as its comment always claimed, and no checker-built program reaches it
today.

**One shape was tried and reverted, and the measurement is why.** Making the
read's *value* the element itself (rather than the element's value slot) also
gives `[1, 2]<0>` a value, but it changes the documented reading:
`<Int, string><0> == Int` answered `1` before it and `0` after, and
`f = x => x<1>; f <Int, string> == string` went from `1: Int` to `0: Int`. The
contract in [raw-index.md](raw-index.md) is the element's *pair*, both slots in
it, and that is what now ships; the four such comparisons are pinned. (The
*type* slot's laziness was dropped later —
[raw-field.md](raw-field.md#check-time-not-raw).)

**The message had to name the right side of the read.** The generic
`RuntimeIndexTarget` wording ("this value is not a container") blames the
container the user wrote, but `[1, 2]` *is* a container — the element `1` is
not. The new `DiagKind::RuntimeRawElement` (`diagnostic.rs:110`, rendered at
`language/src/render.rs:190`) reads *"this raw read found an element that is not
a value/type pair — `X<e>` reads the element's own pair, so the container holds
pairs (a type value); a runtime array's element is read with `e[i]`"*. It is
picked by `is_raw_read_element` (`diagnostic.rs:492`): the raw reads register an
edge for the *element* node at the read's type slot, so a non-container there is
the element and anywhere else is still the container's own failure.

**Tests.** `crates/lichen-language/tests/pipeline.rs` adds
`a_raw_read_of_a_runtime_container_reports_the_non_pair_element` (the failure, its
span, the bound-name form, the deferred form, and the untouched generic wording
for `5<0>`) — **watched to go red**, failing at `pipeline.rs:62` (*"should
fail"*) with the fix stashed — and
`a_raw_read_of_a_type_value_reads_the_components_pair`, which passes before and
after by construction: it is the guard on the four comparisons above, which the
reverted shape broke. Two `registry.rs` tests that pinned the *old* behaviour
are re-pinned, both watched to fail before: a package exporting `[1, 2]<0>` now
fails in **its own** build (`cannot load package 'raw.lichen': …`) rather than
reaching the importer and tripping the `ImportExport` guard, and `[[1]]<0>`
reported its out-of-bounds type slot (`index 1 out of bounds (array length 1)`).
`<Int, string><0>` as a package export imports as `Int : Type` on both trees, so
the fix buys no new capability there — only the honest diagnostic. The stale
comments that credited the raw read's missing pair as the `ImportExport`
guard's reason (`checker.rs`'s `Static` arm, `DiagKind::ImportExport`) were
corrected; the guard itself stays. `lichen-highlevel`, `lichen-language`,
`lichen-compute`, `lichen-lowlevel` and `lichen-language-server` are green.
(The first test is re-pinned by the later kind check and is now
`a_raw_read_of_a_non_tuple_container_is_refused_by_kind`: the same four programs,
asserting the tuple-kind refusal; and `[[1]]<0>` is now pinned as the kind
refusal `expected TypeTuple, found array<array<Int, 1>, 1>`, its out-of-bounds
type slot being unreachable from source once the container must be a tuple type
value.)

**Residual — attribution only.** Through a *deferred* container
(`f = k => k<0>; f [1, 2]`) the failure was reported with the generic wording
and no caret: it lands on an apply clone, which has no expression of its own to
blame — the same limitation `RuntimeApplyTarget` has (`P1-6`). It used to be
silent, so what is new is that it is reported at all.  The later kind check
closes the attribution half: the apply now states the requirement itself, so the
diagnostic reads `expected TypeTuple, found array<Int, 2>` with the caret on the
argument the container was bound to.

**The fork this item does not close.** The audit's preferred falsifier was
`1: Int`. That answer is *not derivable* under the raw form's contract: the
component's type comes from reading its type slot, and `1` has none. Producing it
would mean deriving the result's type from the container's array type instead —
the validated `e[i]`'s derivation, with its bounds assert — which is the
language change `D16` declined to make for `e[i]`, not a fix. The queue takes
the contract as it is written: the container must be a tuple type value, its
components must be pairs, and anything else is refused. `raw-index.md` and
the spec (§2.1 and §3) said "the container may be any expression" at the time;
the later kind check replaced that with the accepted set above.

**Later addendum — the read states its kind, so the failure is static.** `X<e>`
now unifies the container's type against the **tuple kind**
(`check_raw_index`, `checker/indexing.rs`) — the positional sibling of the
`X::a`/`.a` struct-kind requirements — so the runtime rows above are no longer
runtime at all: `[1, 2]<0>` is `expected TypeTuple, found array<Int, 2>` and
`(1, 2)<0>` is `expected TypeTuple, found <Int, Int>`, both `DiagKind::Guard`
with the caret on the container.  A struct type value states `found TypeStruct`
and reads by name (`X::a`, which was the `TableGet` panic on a deferred one);
the named field read `.a` over an array is `expected TypeStruct, found TypeArray`
on the container's kind slot, while `::a` (its type) is
`found array<Int, 2>`.  The
measured rows and their wording stay as the record of the tree they were taken
on; the `RuntimeRawElement` wording this Outcome added now sits behind the kind
check — every container that reaches it is a tuple type value, whose components
are pairs.

### P1-36 — A duplicate kind-marker tag shadows a codec arm and warns instead of failing `verified`

`verified` at `dev@04b5aef`, first-hand at the cited lines; found while adding a
ninth kind marker, which is recorded in
[`floating-point.md`](floating-point.md).

The kind-marker registry (`crates/lichen-highlevel/src/shape.rs:44`) states its
own compatibility contract: *"an existing entry's tag must NEVER change, and a new
marker takes the next unused tag"*. The `TypeValue` codec is generated from that
registry, and `crates/lichen-highlevel/src/program.rs:541-545` additionally
claimed that a registry entry colliding with a hand-spelled arm **"fail[s] to
compile"**.

Neither half of that is true, and the difference is the whole item.

**The tag space is not the registry's.** `define_type_value_codec` expands the
registry's arms and *then* appends `TypeValue::TypeId(n)` — a nominal struct
identity, not a kind marker — which holds tag `8` (`:562-568` write, `:581` read).
So the space is the `TypeValue` codec's, shared with an arm the registry does not
own, and the ninth marker takes `9` rather than `8`.

**A collision warns rather than fails.** The registry arms come first on both
sides, so a marker claiming `8` makes `8 => TypeValue::TypeId` unreachable on
read. The compiler emits `unreachable_patterns` — a **warning**. `cargo check`
passes. Every persisted `TypeId` then decodes as that marker, with the `u64` that
followed it left unread in the stream, and the failure surfaces as a wrong type
somewhere later rather than as a build error.

Measured while implementing: `cargo check -p lichen-highlevel` succeeded with the
warning present, and the collision is caught by an existing round-trip check
(`crates/lichen-language/src/persist/codec.rs:174-193`, which writes a
`TypeId(7)` and then iterates `KIND_MARKERS`) — which is a test this crate does
not run.

**Latent, not live.** No marker claims a duplicate tag today, so nothing is
broken. It is a trap for the next person who adds one, which is the same class as
`P1-26` and `P1-27`: the format's guarantee lives in a comment rather than in a
check.

Note the asymmetry, which is correct and worth preserving: a *gap* in the tag
space falls through to `tag => return Err(format!("unknown type-value tag {tag}"))`
(`:582`) and is a clean error, while a *collision* shadows an arm and corrupts
silently. Any guard added here must tighten the second without softening the
first.

Three ways to close it, none free: generate a compile-time duplicate check
alongside the codec (a `const _: () = assert!(…)`, which is what the project
already uses for the attribute order), add a `build.rs`/macro-time deduplication
that refuses at expansion, or pin the registry's tags to a checked constant.
`floating-point.md` §3.2 records the trap for whoever allocates the next tag; the
guard itself is this item.

### P1-37 — The graph registry freezes the backend of the first graph of a shape `verified`

`verified` at `dev@1fe580c` (still live), first-hand at the cited lines; fixed on
`feature/gpu-algorithms`, merged to `dev` as `6f8f2db`. Found by writing algorithms
against the compute surface — the record is
[`gpu-algorithms-ladder.md`](gpu-algorithms-ladder.md).

`graph_digest` (`crates/lichen-compute/src/compute/graph.rs`) hashes the `Graph`
and deliberately **not** the backend, on the stated ground that *"it is a property
of how the graph is **run**, not of what it computes"*. `intern` stored the whole
`BuiltGraph` — backend included — under the id that digest produced.

So the first build of a shape in a process fixes the backend for every later build
of that shape, and a later build is handed the earlier one's `BuiltGraph` back.
The registries are process-global by design, so this crosses program boundaries:
a process that has ever built a cpu graph of a shape can never run a gpu graph
of it.

```text
a 16-link `compute.graph` chain named "cpu", then the same chain named "gpu"
-- compute.graph: this graph was compiled for the "cpu" backend, and a graph
   runs through the ParallelBackend contract — which the cpu path is not
```

— for a program that never mentions the cpu. The invariant the design wanted
(*"a cache can never serve one backend's graph for another's"*) is true of the
fragment registry, where the backend is genuinely absent from what is stored; it
was false of the graph registry, where the backend is stored and simply not
keyed. The same rule `ComputeValue::ParKernel` already follows.

**Fixed** by moving the backend onto `ComputeValue::Graph(GraphId, Backend)`, so
`graph_digest`'s reasoning is true rather than worked around: the registry stores
a graph and nothing else, and the run reads the backend off the value. `BuiltGraph`
is gone. Pinned by `a_graph_recorded_for_one_backend_runs_on_another`
(`crates/lichen-language/tests/graph_jit.rs`), which builds a cpu graph of a shape
and then a gpu graph of the same shape and requires the second to run; it reaches
a stub backend, so it needs no device, and it fails without the fix.

### P1-38 — A decided non-buffer is answered `parameterized` `verified`

`verified` at `dev@1fe580c` (still live), first-hand at the cited lines; fixed on
`feature/gpu-algorithms`, merged to `dev` as `6f8f2db`.

`compute.read`, `compute.collect` and a parallel launch's `cfg(1)` each matched
their non-buffer arm by falling through to `LowValue::Parameterized`, with no
diagnostic. So a program that passed a plain array where a buffer belonged ran to
completion, printed `parameterized`, and **still showed the type
`array<?a, ?b>`** — a plausible-looking program that computed nothing.

```text
data = [3, 1, 4, 1, 5, 9, 2, 6]
out = compute.plrun (compute.parallel f "cpu") (8, (data,))
compute.collect out          -- parameterized: array<?a, ?b>
```

The lazy fallback is **right for an undecided value and wrong for a decided
one**, which is what makes this a defect rather than a limitation: a program
array *is* decided, it is an ordinary lichen value with ordinary elements, and no
buffer is ever going to arrive for it. This was also the symptom of a missing
feature — there is no `compute.buffer`, so a program has no way to get data onto
a device at all except by running a fill kernel — and the fix is the refusal half
of that: a named refusal is only helpful once the thing the author wanted exists.

**Fixed** at all three sites, each naming its own position and what it holds
(`what_this_is` reads both vocabularies, because a program array is a `LowValue`
and a buffer is a `ComputeValue`). Pinned by
`a_program_value_where_a_buffer_belongs_is_refused`
(`crates/lichen-language/tests/compute.rs`), which requires one diagnostic naming
`cfg(1)`, the position, and `array`; it fails without the fix.

### P1-39 — A `compute.call` in a parallel kernel body is refused with a `NodeId` `verified`

`verified` at `dev@1fe580c`, first-hand at the cited lines. Found by
`crates/lichen-language/examples/recursion.rs`.

```text
k0 = compute.jit (v : Int => v + 1)
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value compute.call k0 i))
}) "gpu"
-- compute.parallel: kernel body hits a node with neither value nor operation
   (node=NodeId(394v1))
```

Three facts make it worse than a missing feature, and they compound.

**It is the only refusal in the compute surface that named nothing.** Every other
one names its own cause — `CONDITIONAL_WRITE`, `UNDECIDED_DOMAIN`, `CALLEE_ARGUMENT`,
the `SpirvRefusal` variants. This one reported a `NodeId`, a compiler-internal
number, which tells a reader nothing they can act on.

**The same call works in a scalar body.** `k1 = compute.jit (v : Int =>
compute.call k0 v + 1)` compiles and runs, answering `5 : Int`. So "one kernel
body calls another" is solved in one of the two body shapes and not the other,
and the difference was not named either.

**It means the device has no working call at all.** Only `parallel` names a
backend, so a cross-kernel call is reachable on a device *only* from inside a
parallel body — and that path fails in the compiler, before any backend sees it.
So `SpirvRefusal::CrossKernelCall`, which
[`lichen-compute-gpu.md`](lichen-compute-gpu.md#not-yet) documents as the reason
cross-kernel calls are out of scope there, **is a refusal no lichen program can
currently provoke.**

**Partly fixed, and the remainder is three cases of one fact.** The message is
now the fact all three share (*a kernel is compiled from a template before any
apply, so a binding the body would fill in at run time is still empty*) and lists
the three shapes, **deliberately without claiming which one it is** — a refusal
that names the wrong cause sends the reader to the wrong place, and the first
version of this message did exactly that by asserting a body-local binding when
the case that exposed it was a `compute.call`. The three cases, all confirmed on
both backends:

| program | result |
|---|---|
| a `compute.call` inside a **parallel** body (the identical call in a *scalar* body runs) | refused |
| a module-level helper called with a **body-local alias** fed by a read | refused |
| a helper **defined in the body** and called there | refused |

All three are the same **missing apply**, not three bugs: the enabling change is
compiling a kernel against an *applied* body, which
[`compute-jit-low-types.md`](compute-jit-low-types.md) calls "a fact about the
language, not a mechanism gap" and parks as per-call-site specialisation. That is
also the prerequisite for `P1-33`'s intended use (below), so the two are worth
looking at together. Recorded in
[`gpu-algorithm-roadmap.md`](gpu-algorithm-roadmap.md#8-the-defects-and-which-are-fixed)
§4.1 and §8.

### P1-40 — The apply budget refuses a long *terminating* loop as non-terminating `verified`

`verified` at `dev@cd1ddeb`, first-hand, via `crates/lichen-language/examples/recursion.rs`.

The VM's budget is a real and good thing — it is how a runaway recursion is caught
rather than hung. But it reports one verdict for two different situations, and the
message names the wrong one:

```text
sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)
sum_to (1000, 0)     -- 1000: ?a          25.4 ms
sum_to (4000, 0)     -- this binding never terminates — it applied a function
                      -- more than 2000 times (non-terminating recursion)
```

The second program **terminates**, in 62 ms. The budget exists to say "this never
terminates"; here it says that about a loop that finishes, and the reader is sent
to look for non-termination that is not there. This is the same class as `P1-31`
(conflated verdicts), and the same fix shape: **the verdict needs to distinguish
"stopped because it exceeded a budget" from "stopped because it cannot
terminate."** A budget that refuses must say which.

**Related and separate — and this half is now fixed.** The second ceiling an
unrolled loop runs into is the emitter's own recursion, which **overflowed the
stack between 400 and 1000 iterations with no diagnostic at all** — a crash
rather than a refusal. Measured: 100 iterations 4.4 ms, 400 iterations 11.6 ms
for a four-element kernel (about 29 µs of compile time per iteration), and a hard
overflow at 1000. **Re-measured first-hand on this machine at 100**, on the main
thread of a debug build, via `crates/lichen-language/examples/recursion.rs` — the
probe completes trips 1 and 10 and overflows on 100. So the figure above is the
low end on a thread with a larger stack, and the low end is a property of the
thread as much as of the walk.

**The overflow was `emit_node`, and it is now a named refusal.** Instrumented
first-hand, the walk reaches level ~175 of ~1 MiB of main-thread stack and dies
— and the `Flow` walk behind it never runs at all (a conditional lowers to a
`select`, so `lower_instrs` iterates a flat list). The walk's depth is the trip
count itself: an unmarked recursion is expanded, and every expanded copy nests
inside the previous one's else arm, so the graph is a chain. Measured on the
probe, `depth ≈ 18 + 3.1 × trip` — trip 1 reaches 21, trip 10 reaches 49.

**Why `#[stacksafe]` had not already covered it:** `lichen-compute` did not
depend on the crate at all, and `#[stacksafe]` only tests for room **at an
annotated frame** — so although `compile` is annotated and grows a segment, the
whole subtree below it shares that one segment and nothing inside ever asks for
another. Both halves were needed and neither was enough:

- `#[stacksafe]` on `emit_node` is what makes a deep body *survivable* at all:
  with the budget but no annotation, trip 100 still crashes below the limit.
- the budget is what makes it *bounded and named*: with the annotation but no
  budget, trip 400 compiles fine (answers 403 in 66 ms on `cpu`, 220 ms on `gpu`)
  at ~1260 levels and ~7 MiB of stack, and a trip of 4000 would do the same.

**The limit is 512 levels of the walk** (`MAX_KERNEL_BODY_DEPTH`,
`crates/lichen-compute/src/compute.rs`), which is about **160 expanded copies**
of a step this size. A constant, deliberately: a threshold derived from the
thread's stack would differ between a debug and a release build of the same
program, and a number that changes with the build profile is not one a program
can be written against — which is the bar this item sets. The refusal names the
limit, the cause (the expansion, not the program), and the fix: mark the
recursion `@loop`
([loop-conversion](loop-conversion.md) §1.1), or write a small trip count out by
hand. Before/after on the probe:

```text
                                   before                    after
  trip 100     thread 'main' has overflowed its stack    24.8 ms  103: ?a      (cpu)
                                                             55.7 ms  103: ?a      (gpu)
  trip 400     (never reached)                           refused: a kernel body's expression nests
                                                        more than 512 levels, so lowering it would
                                                        recurse deeper than a compile should spend on
                                                        its stack … Mark the recursion `@loop` so it
                                                        may become a dynamic loop instead of an
                                                        expansion (`docs/notes/loop-conversion.md` §1.1)
```

Note that a *dynamic* loop — one the JIT emits into the backend IR rather than
expanding — removes **both** of this item's ceilings by construction, which is
why
[`gpu-algorithm-roadmap.md`](gpu-algorithm-roadmap.md#41-axis-b-already-in-the-language-and-what-it-does-not-reach)
§4.1 measures an unrolled loop before recommending it, and is the argument for
`P1-33`'s operator being a builtin rather than a library function. **[Loop
conversion](loop-conversion.md)
keeps that argument and changes the operator**, and its Stage 1 is also the fix
shape for the second ceiling: the structured body is what turns the emitter's
400-to-1000 **crash** into a named refusal, independently of whether any loop is
ever written.

## P2 — architecture

### P2-1 — `BufferSession` is built but unwired `done`

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

**Outcome — the doc half is done and the wiring half is a decision, not a gap.**
The code-side claim this item was filed for is gone: `analysis.rs`'s module doc
named `BufferSession` for the checker run, and `P5-3` corrected it (to
`frontend_at` + `build_report`); `P1-17` rewrote that module doc again when the
index split out, and it still names the real path.  What remained was in the
*notes*, which is where the claim was load-bearing: three plan documents
(`artifact-cache.md`, `liche-lsp-home.md`, `language-toolchain.md`) stated as
settled design that the live buffer is re-analyzed *by* `BufferSession`. It is
not, and `P1-17` makes that definite rather than incidental — the server caches a
`DocIndex` per text and runs no session at all. All three now say what exists and
name the wiring as `P2-1`'s open half.

That half is `D6`'s (b), and the item is explicit that it is a wiring job rather
than a defect, so it is left to the decision that sequences it — with one thing
now known that was not when `D6` was written: (a) landed, and it changed what (b)
is worth. The index cache removes *every* repeat request for a text, but it does
nothing when the text **changes** — which is every keystroke, and is exactly the
case `BufferSession`'s splice addresses by resuming the lex and the
statement-region parse. So (b) is no longer "worth doing for the keystroke path
and not a substitute for (a)"; it is the only remaining win on that path, and T3
(the memoized check) is still what would keep the check from dominating it.

**Wired.** (b) landed: the server drives one `BufferSession` per open document
through a dedicated compile worker thread (the session is `!Send` and must
outlive its request, so neither `Backend` nor a `spawn_blocking` closure can hold
it), the session takes the caller's preprocessed view (code region, base, line
starts, imports) so spans stay absolute in the file the user edits, and the
`DocIndex` is built from the session's own frontend artifacts — the one-shot
`frontend_at` + `build_report` path is now the fallback for a compile that
panics. Measured end to end (600 bindings, 75 marks, `--release`): the first
analysis is 30–32 ms against the old path's 24–28 ms, and every edit is
3.5–5.4 ms (0.15–0.2×) with only the cells the edit reached re-derived. The
first-analysis surcharge was the per-cell artifact size, and that is now paid
(`incremental-update.md` §7.7): a scalar cell's closure went from ~611 nodes to
6–27, so the 75-cell first compile is ~18 ms against the old path's 24–28 ms and
the pathological 600-cell one is ~21 ms against 166.7 ms. The `T3` caveat above is
answered for the key-unchanged case only: a session whose *resolved content* is
unchanged reuses the established `Build` and skips the check entirely, which is
the typing path — an edit that changes the content still re-checks.

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

**Outcome — the count is six, not five, and the wildcard is not in one of them.**
The walks, re-derived from this revision (all `#[stacksafe]`, all exhaustive over
the parser's **31** `Expr` variants, so a new variant breaks the build at every
one of them):

| walk | extent | note |
|---|---|---|
| `analysis.rs` `NameClass::expr` | `:1304-1451` (148) | `:1292-1439` (148) |
| `analysis.rs` `Walk::expr` | `:1556-1713` (158) | `:1540-1697` (158) |
| `analysis.rs` `ScopeCapture::expr` | `:1852-2007` (156) | `:1832-1987` (156) |
| `resolve.rs` `resolve_expr` | `:220-386` (167) | *not listed* |
| `resolve.rs` `KeyWriter::expr` | `:512-749` (238) | `resolve.rs:478` (the serializer cluster, `:447-749`) |
| `compile.rs` `compile_expr` | `:388-829` (442) | `:379-832` |
| `dirty.rs` `walk` | added after this item (`incremental-update`'s dirty propagation) | *not listed* — the newest site |

The note's spans are right but drifted 12-20 lines, and its "~110-line" is wrong:
each of the three analysis walks is ~150 lines. "Adding an `Expr` variant means
editing five sites" is **seven** in the table above — `resolve.rs` has two walks,
the resolver's rewrite and the content-key serializer, not one — and **eight**
workspace-wide: `lichen-language-parser`'s `collect_error_blocks::walk_expr`
(`parse.rs:1369-1512`) walks the same AST and is exhaustive too (it is outside
the note's crate list), and `lichen-language`'s `dirty.rs::walk` joined them with
the incremental-update landing (its exhaustiveness is load-bearing there: a missed
edge is a stale retained cell, `incremental-update.md` §12.4).

**The wildcard is where the note says it is — and it is not one of the walks.**
`range_children` (`checker.rs:1060-1072` before this change, in
`lichen-highlevel`, not the "`lowerlevel`'s sibling" the note calls it) matched
`Tuple`/`TypeTuple`/`Array`/`ShallowArray`/`Table` → their range, `TypeStruct` →
`fields`, `NativeCall` → `args`, and `_ => unreachable!("expected a variadic
expression kind")`.
*Verified with a scratch variant*: adding `ExprKind::Probe` to the IR broke
exactly three matches — `persp_combine_children` (`annotations.rs:72`),
`check_term` (`checker.rs:1204`) and `repoint` (`ir.rs:467`) — and left
`range_children` compiling, so the note's "its two siblings are fully enumerated
and *would* break the build" is exact, and a new variadic kind would have
panicked at run time. The ledger row's "one of them has a wildcard arm" therefore
fuses two findings: **none** of the six parser-AST walks is partial.

**Nothing was being skipped.** `range_children`'s wildcard arm is reachable only
by asking a non-variadic kind for children, so no variant relies on it and no
test can move when it goes. With the fix in place and the same scratch `Probe`
variant still present, the check reported **four** sites — `range_children`
(`checker.rs:1066`) joined the three above. That is the whole proof: a new
variadic kind is now a compile error rather than a run-time panic.

**The other wildcard, the one inside a walk.** `compile_expr`'s array arm
(`:755-764`) matched `Expr::Shallow(inner, depth, _)` against
`_ => compile it as a plain element, depth 0`. Unlike `range_children` its
default is *correct* — all 30 non-`Shallow` variants rely on that arm, and that
is what they should do — so enumerating them (`:755-796`, what landed) is
behaviour-preserving, not a bug fix. What it buys is the item's stated
property: a new kind appearing inside an array literal is a compile error instead
of a silent depth-0 element.

*Three wildcards deliberately left, and the line.* `range_children`'s neighbour
`range_depths` (`:1103-1109`), `ir.rs`'s `annotation_attrs` (`:603-608`) and
`structs.rs`'s `check_type_struct` (`:697-700`) are single-variant extractors:
their wildcard **is** the caller's contract ("on a non-… expression") and a new
variant cannot legitimately reach them. The line is whether the match
*classifies* an open set of kinds — `range_children` (which kinds store children
as a range: a new one may) and `compile_expr`'s element test (every kind) — or
*extracts* one known variant.

**Preference 1 — one generic walk — is a redesign, and is left proposed.** The
six walks' needs genuinely diverge: three per-variant analyses (which name is
defined where, which frame a statement opens, the scope snapshot at a byte
offset), an in-place `&mut Expr` rewrite, a *tagged* encoder whose output is the
artifact's content key (`KEY_FORMAT_VERSION` moves with the variant set), and an
IR lowering that allocates `ExprId`s. A shared walk needs either a 31-method
visitor whose methods are the present arms — the per-variant bodies are where
the 148-442 lines are, so little would shrink — or a children-range function plus
hooks; and every walk's recursion would move into the shared walk, so its
`#[stacksafe]` guard has to sit on the new entry point exactly as it sits on the
old ones or `P1-22`'s overflow returns. That is a design change of the
language-server's analysis and of the lowering, not a refactor of duplication —
worth its own decision rather than this item.

**`P1-22`'s guards are intact.** No walk was collapsed and no attribute moved:
`compile_expr` (`compile.rs:387`), `resolve_expr` (`resolve.rs:219`),
`KeyWriter::expr` (`resolve.rs:511`), the three analysis walks (`analysis.rs:1303`,
`:1555`, `:1851`) and the checker's own `check_term` (`checker.rs:1194`) all still
carry `#[stacksafe]`.

**Evidence.** Scratch `ExprKind` variant before/after as above (removed from the
commit). `cargo clippy --workspace --all-targets -- -D warnings` exits 0,
`cargo fmt --all -- --check` exits 0, `cargo test --workspace` passes (72 test
binaries, 764 tests, 0 failed), and `cargo test -p lichen-language --test
pipeline --test examples --test compute --test table` passes (24 + 1 + 123 + 11).

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

**Outcome — the four vectors are one; the read surface is untouched.**
Re-derived first, because the mechanism needed pinning before the shape could
be chosen: the four are `term`, `val`, `ty` and `attr`, all
`Vec<Option<NodeId>>`, all sized once at `n = ir.expr.len()` in the single
`Checker` initializer (`checker.rs:571-574` before this change) and moved into
`Build` at its single construction site (`:749-755`).  **No code pushes to any
of them** — every write is `self.<field>[e] = …` — so the four were always the
same length and the "silently misaligned build" was a latent hazard (a future
`push`), not a live one.  Measured on the pre-change tree: 157 indexed reads
inside `lichen-highlevel/src` (218 workspace-wide), 43 of them immediately
`.unwrap()`/`.expect()` — the ledger's "about 30" undercounts — and 35 `term`,
34 `val`, 35 `ty`, 2 `attr` write sites.  The sparsity is real, and it is
*presence*, not alignment: `val` is `None` for a call result (the lazy
`value_of` memo) and `attr` only for an expression whose schema carries a
constraint.

What landed is the ledger's own first shape: `ExprState` (four `Option` fields,
so presence sparsity is unchanged) and `Build::state: Vec<ExprState>` — plus
`Checker::state` behind it.  The `ExprId` indexing impl that made four parallel
vectors convenient is moved from `Vec<Option<NodeId>>` to `Vec<ExprState>`
(`ir.rs`'s copy is deleted), so the ordinal belongs to one vector and
misalignment is unrepresentable instead of conventional.  Behaviour is
identical: 225 access sites (223 code, 2 doc comments) become one index plus a
field, with the same values read, the same diagnostics in the same order, and
no expression checked differently; `cargo clippy --workspace --all-targets --
-D warnings` exits 0, `cargo fmt --all -- --check` exits 0, `cargo test
--workspace` passes, and `cargo test -p lichen-language --test pipeline --test
examples --test persist --test registry --test compute` passes (24 + 1 + 15 +
123 + 16, the counts the sibling items report).

*Residual, deliberately left — the read surface.*  The ledger's second half (a
checked accessor that records a diagnostic instead of panicking) is **not**
done: the 43 `.unwrap()`/`.expect()` reads are the checker's own contract
assertions, and answering one with a recorded diagnostic changes what the
compiler does on a checker bug — a behaviour change, which this item's brief
forbids.  The panics are exactly where they were.

*The public surface moved, and that is visible.*  `Build::term`/`val`/`ty`/
`attr` are now `Build::state[expression].term`/`val`/`ty`/`attr`; the 11 sites
in `lichen-language-server/src/analysis.rs` were rewritten with the rest
(`analysis.rs:82`'s own prose note included), so an out-of-tree reader of the
old four fields is the only thing this breaks.  Nothing serialises `Build`: the
artifact codec reads the frozen `Module`, and the checker's diagnostic assembly
reads `diary`/`apply_edges`/`node_edges`, none of which the four vectors feed.
`P1-5`'s content key and `P1-9`'s diary discriminant do not touch these fields
and are unaffected.

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

**Outcome — the seam table re-verified; nine of the eleven files are split, and two are left with their reason.**  Every extent above had drifted, so each was re-measured before that commit (true line counts; it is the `first commit` column below, and the two files it left only for batch size were taken in the follow-up pass recorded after the seam list):

| file | ledger | re-measured | first commit |
|---|---|---|---|
| `language-server/src/analysis.rs` | 2566 | 2689 | **left** |
| `compute/src/compute.rs` | 2182 | 2371 | **left** |
| `highlevel/src/checker.rs` | 1502 | 1765 | 1528 — `asserts` 50, `native_call` 118, `operators` 63, `tuples` 60 |
| `lowlevel/src/lib.rs` | 1179 | 1505 | **left here** |
| `lowlevel/src/static_module.rs` | 848 | 933 | 248 — `apply` 366, `freeze` 334 |
| `render/src/render.rs` | 1168 | 1302 | 485 — `type_printer` 485, `value_printer` 344 |
| `language/src/compile.rs` | 952 | 994 | 922 — `alloc` 86 |
| `language/src/resolve.rs` | 706 | 760 | 397 — `content_key` 373 |
| `language/src/package.rs` | 916 | 930 | 787 — `vendored` 151 |
| `language/src/persist.rs` | 650 | 744 | **left here** |
| `language-parser/src/parse.rs` | 1584 | 1566 | 1378 — `error_blocks` 168, `diagnostics` 36 |

Both extents the item text calls out are wrong.  `compile_expr` is **475** lines,
not the 453 of the table and not the 442 a previous item reports: its body is
`compile.rs:388-862` at the pre-commit revision (484 with its doc comment from
`:381`), and `:829` — the end line that item gives — is inside the
`Expr::TypeArray` arm.  It was **not** touched: its `#[stacksafe]` is at `:387` and
the item text puts that recursion out of scope.  The checker `impl` block is
**1201** lines (`checker.rs:492-1692`), not 1090, and 957 now (`:499-1455`).

*The seams taken.*

- `static_module.rs` (933) → `static_module/apply.rs`: the static-function apply
  half of the mixed `impl Module` block plus its capture analysis and
  `StaticApplyCtx` (the `#[stacksafe]` guard moved with `static_node_apply`);
  and `static_module/freeze.rs`: the whole `impl StaticModule` freeze plus
  `align_up` and `rewrite_value`.  The parent keeps the registry-facing reads,
  `static_find`, `static_ref` and `referenced_keys`.
- `render.rs` (1302) → `render/type_printer.rs` and `render/value_printer.rs`
  (the two `impl` blocks).  The struct definitions and the struct-kind helpers
  stay, see the boundary below.
- `resolve.rs` (760) → `resolve/content_key.rs`: `KEY_FORMAT_VERSION`,
  `content_key` and `KeyWriter`, the serializer cluster the item calls 45 percent
  of the file, with `pub use content_key::content_key;` so `resolve::content_key`
  still resolves from `session.rs` and the tests.
- `checker.rs` (1765) → the six concrete check rules still inline are now four
  sibling modules, following the `lambda`/`structs`/`indexing`/`annotations`
  pattern already in `checker/`: `tuples` (tuple terms, tuple type expressions,
  the type-position helper), `operators` (`check_binop`), `asserts`
  (`check_assert`, `register_assert`) and `native_call` (`check_native_call`).
  The `#[stacksafe]` dispatcher `check_term` and the `build_with` pass driver
  stayed in the root.
- `compile.rs` (994) → `compile/alloc.rs`: the ten allocators (`alloc` plus the
  nine `alloc_*` wrappers).
- `package.rs` (930) → `package/vendored.rs`: `vendored_alias`,
  `vendored_entry_file` and the `vendored_tests` module that tests them, so the
  inline tests move with their subject.
- `parse.rs` (1566) → `parse/error_blocks.rs` (the recovered-error AST walk) and
  `parse/diagnostics.rs` (`diag_from`), declared with `#[path]` because `parse.rs`
  is itself reached by `#[path]` from the crate root.

*How the moves were checked.*  A split here is a move and nothing else: the
parent was sliced at the seam by line range and the slice written to the child
verbatim, with only (a) a module doc, (b) `use super::*;` plus, where the parent
no longer carried a name the child needs, one explicit import, (c) an `impl`
wrapper where the moved block was part of a mixed `impl` block, and (d) the
visibility of a method the parent still calls.  Every child body was then diffed
against that slice of `HEAD:crates/...` and is byte-identical apart from exactly
those adjustments.  The visibility changes are private → `pub(super)` and never
toward `pub`:

- `render/value_printer.rs` `ValuePrinter::element_any` and
  `render/type_printer.rs` `TypePrinter::is_universe_any` are the only two
  methods raised: the `render.rs` public free function `render_struct_fields_named`
  and the value printer each call across the new boundary.
- `checker/`: `check_native_call`, `check_binop`, `check_assert`,
  `register_assert` (`indexing.rs` calls it too), `check_tuple_term`,
  `check_tuple_type`, `check_type_element` (`structs.rs` calls it too) — the same
  `pub(super)` the existing `checker/` rules already use.
- `compile/alloc.rs`: the ten allocators.  `package/vendored.rs`:
  `vendored_alias` and `vendored_entry_file`, re-imported into the parent by
  name.  `parse/diagnostics.rs`: `diag_from`.

No `#[stacksafe]` guard moved off its function: `check_term`
(`checker.rs:1226`), `KeyWriter::expr` (`resolve/content_key.rs:124`),
`static_node_apply` (`static_module/apply.rs:91`) and `compile_expr`
(`compile.rs:387`) all carry the attribute exactly as before, and `analysis.rs`,
`resolve_expr` and the three analysis walks were not touched at all.  Each split
parent module doc now names its new siblings.

*Left by that commit, and why.*  Four files:

- `analysis.rs` (2689).  The highest-value cut the item names there — merging the
  three ~150-line walks — **is the P2-2 Preference 1**, which the P2-2 Outcome
  deliberately left proposed: it is a redesign of the analysis, and collapsing
  the walks would move three `#[stacksafe]` guards.  What remains is one 902-line
  `impl Doc` over the shared report/snapshot state plus a token-classifier cluster
  (`classify_token_kind`, `classify_names`, `NameClass`) that the scope walk
  reads.  That cluster is a real seam but a small one, and taking it needs
  `pub(super)` on a struct and its methods while the 902-line block stays.
- `compute.rs` (2371).  The responsibilities the item lists are real (the kernel
  and buffer registries, the value and operator vocabularies with their codecs,
  the emit/codegen stack, the two runners, the op vocabulary), but they are all
  **free functions over `Module<P>`** that call one another.  There is no
  type-owned `impl` block to lift, so any seam needs `pub(super)` on most of
  roughly 30 signatures for a layout with no call-site benefit.
- `lowlevel/lib.rs` (1505).  Two seams pass the same test as the seven taken —
  `impl Module` (`:1139`, 367 lines) and `impl Registry` (`:1017`, 115) — and are
  left only because this commit already adds twelve modules; the vocabulary half
  of the file is a different matter, since those are the crate public types and
  moving them needs `pub use` re-exports rather than a plain `mod`.
- `persist.rs` (744).  The container (the writer and reader, the artifact header
  and the body digest), the cache-root resolver (`load_artifact`,
  `shipping_cache_root`) and the codec traits above them are separable and share
  no mutable state.  Left for the same reason as `lowlevel/lib.rs`.

*The struct-kind helpers, deliberately not split.*  The `render.rs` helpers
`representative`, `letter_name`, `is_struct_kind`, `is_universe`, `is_universe_any`,
`marker_is_struct`, `kind_is_struct`, `struct_field_names`, `struct_kind_id`,
`struct_fields_with_names` and the two public renderers stay in the parent: the
two printer modules and the parent all call them, so a third file would mean
`pub(super)` on twelve items for a shared utility base, not a responsibility
boundary.  This is the item own line about a widened surface being a worse trade
than a long file.

**Follow-up pass — the two batch-size files are split; the two real costs stay,
each with its boundary re-derived.**  Both splits are moves, checked the same way
as the seven before them: the parent was sliced at the seam and the slice written
to the child verbatim apart from a module doc, its `use` lines and the one
visibility below.

- `lowlevel/lib.rs` (1505) → `lowlevel/module.rs` (377: `impl Default for Module`
  plus the whole `impl Module`, parent lines 1133-1505) and
  `lowlevel/registry.rs` (125: `impl Default for Registry` plus `impl Registry`,
  lines 1011-1131).  The parent keeps its 1010 lines of types, traits and handles
  and declares the two with plain `mod`, so no public path moved.  `lib.rs`
  carries no module doc to extend, so the sibling naming lives in the two
  children's own docs; `persist.rs`'s module doc, below, now lists its three.
  One visibility changed: `Module::with_registry` is `pub(super)`, because
  `Registry::new_module` now sits in the sibling.  Diffed against `HEAD:crates/lichen-lowlevel/src/lib.rs`:
  `registry.rs`'s body is byte-identical to lines 1011-1131, `module.rs`'s to
  1133-1505 except that one line, and `lib.rs` is lines 1-1010 plus the two `mod`
  declarations.
- `persist.rs` (744) → `persist/container.rs` (403: `serialize_artifact` /
  `serialize_artifact_with`, the shape encoders, `reserve`, `check_node_index`,
  `read_node_id` and `deserialize_artifact` / `deserialize_artifact_with`, lines
  219-607), `persist/codec.rs` (222: `ArtifactCodec`, `ProgramCodecOf`,
  `NoPersist` — lines 107-217 — and the `codec_roundtrip` tests, which moved with
  the trait their doc points at, lines 647-744) and `persist/cache.rs` (43:
  `load_artifact` and `shipping_cache_root`, lines 609-626 and 633-645).  The
  parent keeps the module doc, the format comment, `ARTIFACT_FORMAT_VERSION`,
  every re-export and the `mod` declarations — 111 lines.  `container.rs` and
  `cache.rs` are byte-identical to their slices; `codec.rs` differs at exactly
  two lines (the test module's own imports, and the trait doc's `ProgramCodec`
  link, now spelled as a path because the import that resolved it stayed behind).
  No visibility had to move at all.

**Why the two cost-deferred files stay deferred, re-derived rather than
inherited.**

- `analysis.rs` (2689).  The file is one 902-line `impl Doc` (`:173-1075`) plus
  three visitor clusters — `Walk` (`:1493-1729`), `ScopeCapture` (`:1730-2009`)
  and the name classifier (`classify_token_kind` / `classify_names` /
  `NameClass`, `:1198-1461`, which carries one of `P1-22`'s three `#[stacksafe]`
  walks in `NameClass::expr`) — and ~680 lines of tests.  The only seam that is
  neither the redesign nor the tests is the classifier: 264 lines, read from
  `impl Doc` at `:1033`/`:1038`.  Taking it costs `pub(super)` on two functions,
  the struct and its methods and moves a guard, and leaves 2425 lines with the
  902-line `impl Doc` untouched — the same god file with a smaller tail.  The cut
  worth taking there is the one P2-2 names (one generic walk over the three
  visitors), which that Outcome deliberately left *proposed* as a redesign of the
  analysis, with the guards riding on exactly the entry points it would replace.
  `P2-11` is a move-only item and cannot take a redesign.
- `compute.rs` (2371).  Re-measured, the item's stated cost is right about the
  file's core and too strong about its edges: the file is 33 free functions over
  `Module<P>` that call one another (`:641-1928` is the lower / emit / run stack),
  but the first pass's "no type-owned `impl` block to lift" is not exact — the
  eleven `NativeOp` impls (`:2002-2370`) reference exactly **one** of those 33
  free functions (`buffers`), so that seam alone would cost one `pub(super)`.  It
  is still not taken, on `AGENTS.md`'s line: it moves 369 lines out of a file and
  leaves 2002 lines of the mutual-recursion web the item itself names, so it buys
  a file boundary for 15 percent of the file without reaching the boundary that
  matters.  The cut that would reach it — separating the registries, the
  emit/codegen stack and the runners — is a `pub(super)` on most of those 33
  signatures because they call one another, and that is a redesign of the codegen
  stack, not a move: the same call as `analysis.rs`.  Recorded rather than done,
  and available cheaply if a later item wants the native-operator surface in its
  own file.

**Residual, and the status.**  Nine of the eleven files are split; `analysis.rs`
and `compute.rs` are not, each on the boundary stated above rather than on batch
size.  The row is `done`: the two deferrals that were only about commit size are
split, and the two that remain are the item's own two real costs, re-measured
here.

**Evidence, final pass.**  `cargo clippy --workspace --all-targets -- -D warnings`
exits 0, `cargo fmt --all -- --check` exits 0, `cargo test --workspace` passes
with zero failures in every binary, and `cargo test -p lichen-language --test
pipeline --test examples --test persist --test registry` passes (123 + 1 + 15 +
16).

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

**Outcome — done, but not by opacity alone: the accessor was the route, not the
fields.** Both routes are re-derived first-hand.

*Route 2 reproduces as recorded.* From `crates/lichen-lowlevel/tests/basic/` — an
external crate to both `lichen-lowlevel` and `lichen-utils` — this compiles and
makes `equality_representative(a)` return `b`:

```text
m.nodes[a].meta_mut().parent = Some(b);
```

*A probe was run against exactly that intermediate state.* With `Meta`'s fields
private, the read accessors added and `Meta::new` supplied for the frozen mirror,
both of these still compile and still write the live link:

```text
*m.nodes[a].meta_mut() = Meta::new(Some(b), None, None, 1);   // an arbitrary parent
*m.nodes[other].meta_mut() = Meta::default();                 // split a node out of its class
```

The constructor has to be public — `static_module.rs`'s freeze and
`persist.rs`'s artifact decoder both build a `Meta` from solved links — and the
accessor hands out `&mut Meta`, so assignment through it is the field write one
indirection further out; `Default` alone is enough for the second. **Opacity does
not close the write**, and `D11`'s "a documented reader is a contract, whereas a
public field is an invitation" applies to the mutable accessor too: it was the
accessor that had to change.

*Route 1 is the same statement's other half.* `nodes` being public is what makes
`&mut Node<P>` nameable, but after `P2-4` every state field of `Node` is private,
so the only *write* a `&mut Node<P>` yields is `meta_mut`. Closing it closed both
recorded routes; `Module::nodes` therefore stays public, as this item required.

**What landed.** `Meta` is opaque (`disjoint.rs`): private fields, four read
accessors (`parent`/`next`/`tail`/`size`), and `Meta::new(parent, next, tail,
size)` for the two external construction sites (`static_module.rs:588`,
`persist.rs:564`). `Node::meta_mut` now takes a `MetaPermit` whose field is
private to `lichen_utils::disjoint`, so no other crate can mint one; the four
operations inside the module do. The one legitimate writer outside it —
garbage collection's class splice — moved into the union-find as
`disjoint::rebuild`: `gc.rs`'s `flatten_class` walks the member list read-only
(`meta().next()`) collecting the survivors, then hands them over in visit order,
and `rebuild` writes the same links the in-place loop wrote (each survivor's
parent to the first survivor, the list re-linked in order, the last one
terminated, the representative's `tail`/`size`) — so the tree, the member list
and the counts are identical. `flatten_class` now collects the survivors in a
`Vec<NodeId>` — one allocation per class spliced, where the old loop wrote in
place; `drop_block` already builds a `HashSet` per call, and no other pass
changed. `find`/`union`/`make_set` are unchanged apart from minting the permit.

*The read surface widened exactly as `D11` accepted it.* Every read of the four
links now goes through an accessor: 23 sites outside `lichen-utils` — eleven in
`equality.rs` (three `equality.parent`, one `equality.next`, seven member-list
`meta().next`), five in `static_module.rs` (the `static_find` walk and the
freeze's four-link remap), four in `persist.rs` (the artifact encoder), and one
each in `compute.rs`, `render.rs` and `lichen-lowlevel`'s own equality test —
plus nine in `lichen-utils`' test node.

*Two things this deliberately leaves alone.* `disjoint::union`/`find`/`make_set`
remain public and callable on `Module::nodes`: they are the union-find's own
invariant-preserving API — and `Module::add_equality`/`equality_representative`
are thin wrappers over them — so a host merging two classes that way performs a
legitimate merge, not the wrong-link write this item is about. The frozen
mirror's own links (`StaticNode::equality` is a `pub` field) stay `pub` too: a
`StaticModule` is only reachable as `Arc<StaticModule<P>>`, so no `&mut
StaticNode` exists to write through.

*The extent the note gives does not reproduce.* "184 `.nodes` uses outside
`lichen-lowlevel/src`" measures as **56** field uses (`\.nodes`) — 42 in
`lichen-lowlevel`'s own integration tests and 14 across `lichen-language` (7),
`lichen-highlevel` (3), `lichen-render` (3) and `lichen-compute` (1). That is
still five crates including the test tree, and the table is still the larger
change; the number is corrected, not the deferral.

**Evidence.** The probe was an integration test in `lichen-lowlevel`'s `basic`
crate (an external crate): the two writes above passed before the gate and fail
to compile after it — `E0061` (the missing `MetaPermit`), then `E0423` (`cannot
initialize a tuple struct which contains private fields`) once the permit is
passed explicitly. The probe was deleted before the commit. `cargo clippy
--workspace --all-targets -- -D warnings` exits 0, `cargo fmt --all -- --check`
exits 0, `cargo test --workspace` passes — including the frozen-mirror path
(`static_module`'s freeze and `persist`'s artifact round-trip) and the DSU suite
(`find`'s stack-safety on a 100,000-deep hand-built chain included, whose
`link` helper now assigns the test's own node field instead of writing through
the union-find).

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

**Outcome — the premise held, and the divergences are worse than "disagree":
two of the conversions *panicked* on inputs the others answered.**  The count is
not four.  The four sites the item names (`span`'s `line_col`, `lsp`'s
`offset_of_span`, `position_at_offset` and `span_of_offset`) all exist, plus
`render.rs`; but `lsp::offset_from_position` (`lsp.rs:66-83`) is a **fifth**
conversion — the UTF-16 reverse of `position_at_offset` — that the item never
mentions, and `render.rs:112` is not a conversion at all.  It is a second
**line model** (`str::lines()`), and that is where it diverged.

*Measured before the fix.*  `line_col` from `lichen-span`; the rest from
`lichen_language_server::lsp`.  All inputs listed as "byte N" are byte offsets.

| input | `line_col` | `span_of_offset` | `position_at_offset` | `offset_of_span` |
|---|---|---|---|---|
| empty `starts`, byte 0 | **panic**: `attempt to subtract with overflow` | `(1, 1)` | — | `0` |
| `"é=1"`, byte 1 (inside `é`) | `(1, 2)` | `(1, 2)` | **panic**: `byte index 1 is not a char boundary` | — |
| `"ab"`, byte 5 (past the end) | `(1, 6)` | `(1, 6)` | `(0, 2)` — clamped | — |
| `"ab"`, span `(99, 3)` | — | — | — | `0` |
| `"ab\n"`, span `(99, 3)` | — | — | — | `3` |
| `"ab"`, span `(1, 999)` | — | — | — | `998` |

`line_col` and `span_of_offset` were already the same formula, so they never
disagreed — what the first row shows is that the shared formula was not total.
The line model differed too: `"ab\n"` is `["ab"]` under `str::lines()` but
`["ab\n", ""]` under `line_starts`; `""` is `[]` against `[""]`; and `"a\r\nb"`
is `["a", "b"]` against `["a\r\n", "b"]`, so a span at or past the `\r` had its
caret placed against text one byte shorter than the column counted.  A lone `\r`
and a byte inside a multi-byte character are *not* line-model divergences:
`line_starts` breaks on `\n` only and `line_col`'s column is byte-exact.

*What landed — `lichen-span` owns the concept, and its doc states the model.*

- `line_col` (`span/src/lib.rs:57`) is **total**: an empty `starts` is "line 1
  starts at byte 0", a table that does not begin at byte 0 saturates the column
  instead of underflowing, and nothing panics on any table.  The column is
  **1-based and byte-exact**, and **end of file is a valid position** (for a
  source ending in `\n`, `source.len()` is the empty line after it, column 1).
- `offset_of_span` (`:76`) is new to the crate — the reverse had lived only in
  the language server.  Its out-of-range answer is **saturation on the line with
  the column kept**: a line past the last is the last line, a line below 1 is
  line 1, and an empty `starts` is line 1 at byte 0, so it can no longer answer
  with a *different* line's start.
- `line_text` (`:86`) is the line's text without its terminator — the display
  half of the same model.
- `lichen-language-lex` re-exports both new names, so `lichen_language::lex`
  keeps being the path a crate with no direct `lichen-span` dependency uses.

*The language server keeps only the protocol dialect.*  `span_of_offset`
(`lsp.rs:113`) is `line_col` under the name its callers use, and
`offset_of_span` (`:40`) is the span crate's.  `position_at_offset` (`:74`) gets
its line and byte column from `line_col` and applies the two clamps LSP's units
force: the offset is clamped to `source.len()`, and a byte inside a multi-byte
character is floored to the character's start (`floor_char_boundary`, `:52`),
because a `character` in UTF-16 code units cannot name a mid-character position.
`offset_from_position` (`:91`) stays as that dialect's reverse and returns `None`
for a line outside the source — the protocol's "no such position".  Every doc
comment names the units, 0- or 1-based, so the two dialects cannot be mistaken
for two implementations.

*`render.rs` uses the one model.*  `render.rs:111` slices the line through
`line_text(source, &line_starts(source), line)` instead of `str::lines()`, so
the text and the `(line, col)` name the same line; it strips the terminator, so
display behaviour for every ordinary line (including `\r\n`) is unchanged.  One
behaviour change follows: a diagnostic on the valid empty line after a trailing
newline — or on line 1 of an empty source — now prints that empty line instead
of nothing.  No existing test pinned that (the only span-carrying render test is
`pipeline.rs`'s `diagnostics_render_with_carets`).  The per-call scan is
*unchanged* in cost; `P4-4` owns it.

*Tests, and the before-state.*  `D3` grants permission, and the tests live in
their own files.  `crates/lichen-span/tests/line_model.rs` (13 tests) pins the
decided model — the empty table, a table not beginning at byte 0, end of file, a
byte inside a character, a byte past the end, `\r\n` and a lone `\r`, the
inverse, and the saturated out-of-range answer.
`crates/lichen-language-server/tests/lsp_position.rs`
(9 tests) pins the boundary against that model.  **Four of the boundary tests
fail on the unfixed tree**, with the exact defects measured above:
`an_empty_line_start_table_is_a_total_input` panicked at `lichen-span/src/lib.rs:37`
with *"attempt to subtract with overflow"*; both
`a_byte_inside_a_character_clamps_to_the_character_start` and
`the_lsp_line_is_the_span_line_zero_based` panicked at `lsp.rs:58` with *"byte
index 1 is not a char boundary; it is inside 'é'"*; and
`an_out_of_range_span_saturates_instead_of_naming_another_line` failed
`assert_eq!(offset_of_span(&[0], (99, 3)), 2)` with `left: 0, right: 2`.  The
span-side test `a_table_that_does_not_begin_at_zero_is_total_too` covers the
item's own second symptom, *"`Err(0)` → index `usize::MAX`"*: the old
`starts[line - 1]` indexed `usize::MAX` when the first recorded start was past
`pos`.

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

**Outcome.** The premise held; the cited lines had drifted, and the extent is
larger than the note's account. Re-derived, the deep pass's per-element calls
are in `evaluate_node_deep_inner`: the **descent** loop over an array's items
(`evaluation.rs:646-661` before the fix — one `static_read` per element, which
is one registry lookup) *and* the parameterized check's `.iter().any(...)`
closures for the array arm and the table's key/value arms
(`evaluation.rs:703-746`, one lookup per static element each). The equality
side is `value_is_skeleton`
(`equality.rs:554-579`), called once per array element from
`class_is_skeleton`.

*Measured, before:* one `evaluate_node_deep` of a 20,000-element array whose
items are all static refs into one frozen module — **40,000 registry lookups**
(2 per element: the descent read plus the parameterized check), 9.9 ms in the
`test` profile. *After:* **1 lookup**, 2.4 ms, same input and profile. (The
counter was a temporary probe inside `Module::static_module`; it is removed.)

*The fix.* `StaticModuleCache` (`static_module.rs:41-73`) is a one-entry
`(ModuleKey, Arc<StaticModule>)` memo: each lookup takes the registry read lock
exactly as `Module::static_module` does and releases it before the next, so the
lock's scope is unchanged and no writer can be blocked by a walk. The deep pass
threads one cache through the whole walk
(`evaluate_node_deep`/`evaluate_node_forced` create it, `evaluation.rs:483-487`,
`:502-506`; `evaluate_node_deep_inner` carries it, `:564-571`), so the descent
read and the parameterized check share it — the check moved into
`value_is_parameterized` (`:716-771`), whose `.any()` closures now go through
`cache.node_parameterized`. The equality walk threads one cache per
`class_is_skeleton` (`equality.rs:529-596`). The lookup's *result* is unchanged:
`StaticModuleCache::read` is `StaticModule::read` on the same registered
module, and a key's entry is never replaced once registered (`freeze_mapped`
and `insert_module` both assert the key is free), so a cached key cannot go
stale.

*Deliberately not done.* `table.rs:224` (`key_state`'s static arm) and the
other single-shot lookups (`static_read`, `node_value`) are not in a loop and
have no walk to share a cache with; the table's `hash_step` lookups
(`table.rs:405`, `:454`) are per-function/per-node calls inside an unfolding
memo, not per-element walks.

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

**Outcome — half held, half refuted.**

*The class-size walk: held, and it is the whole of the real cost.* `write_node_value`
(`equality.rs:310-366`, the walk now named `propagate_class_value`) does walk the
entire member list on every concrete write.
The note's extrapolation — *"classes grow with application count … quadratic in
the common recursive case"* — is **refuted by measurement**: on a real recursive
apply (`fib(16)` through the `lowlevel` harness: definition pass, then one call)
the **largest class reached by any write is 2 members**. Classes are grown only
by `disjoint::union` (`bind`, `add_equality`, `unify_clone_groups`), and a
template's class topology — not the apply count — sets their size; the 100 000
apply budget bounds how many *copies* exist, not how many members one class has.

*The seven walks: held as a count, refuted as "a narrower one would do".* The
seven are `write_node_value` plus the six predicates the note names, and each
does walk the whole member list (the seventh is `propagate_class_value` since the
member-local write rule landed — `class-channel.md` §1.1). Re-derived, each needs
the whole list:

| site | what it needs |
|---|---|
| `propagate_class_value` `:342` | every operation-free member — a member that knows nothing takes the value, one that holds a value has it compared and is left at its own when the two cannot be one (`class-channel.md` §1.1) |
| `class_has_pending_op` `:489` | **any** member with an unbound operation |
| `class_is_pure_cell` `:507` | **every** member is not an independent pending computation |
| `class_is_skeleton` `:529` | **every** member is a cell or a skeleton array |
| `pending_op` `:673` | the **first** pending member *in member-list order* — its identity is used (`alias_index` binds that node, `force_pending` evaluates it) |
| `class_committed_value` `:765` | the **first** concrete value in list order (the doc says why it is not on the representative) |
| `force_pending` `:783` | acts on the **first** pending member |

So a narrower walk is not available at any of the six predicates: an `∃`/`∀` over
a list, or an order-dependent `first`, is the list. The only way to collapse them
is the summary the note proposes, which would have to live in `disjoint::Meta`
or in node state maintained on every value/operation transition — the state
`P2-13` is about, and a wider change than this item. The six also repeat the
same walk inside one `unify_inner` failure step (`class_has_pending_op` once,
`pending_op` up to twice, `class_is_skeleton` up to twice per side); that is a
redundancy, not a narrower walk, and the step is not reached at all in the
workload measured below.

*Measured, before → after* (all on the `lowlevel` harness, `test` profile;
counters were temporary probes in `write_node_value` and the six loops, removed
before commit):

| input | concrete writes | writes that walked a class | member visits |
|---|---|---|---|
| 20 000 nodes, each its own class | 20 000 | 20 000 → **0** | 20 000 → **0** |
| one 2 000-member class, every member written | 2 000 | 2 000 → 2 000 | 4 000 000 → **4 000 000** |
| `fib(16)`: definition pass + one call | 49 490 | 49 490 → **3 193** | 52 683 → **6 386** |

The 2 000-member row does not move, and by design: every member must see the
new value, so that walk is the item's *correctness* requirement, not its cost.
Measured member visits in that shape are what "O(class size)" means; the shape
itself does not arise from the apply path the note blames — `fib(16)` above runs
the real `Apply`/`regroup_clones` machinery and never exceeds **2** members. The
checker's own type-variable unification was not measured here.

*The fix.* Skip the walk for a singleton class — `parent` and `next` both `None`
on `node`, `disjoint::Meta`'s contract for a representative with no second
member (`equality.rs:97-105`); `make_set` and `flatten_class` (`gc.rs:154-189`)
are the only writers of that metadata and both leave a lone member exactly that
way. For a singleton the loop could only have re-visited `node` itself, whose
slot the write above already set, so the skip is provably the same effect. That
one test removes 88% of the member visits on `fib(16)` and all of them on the
singleton workload, and is what the item's title costs in practice.

*Deliberately not done.* The `PendingSides` construction
(`equality.rs:356-370`) calls `class_is_pure_cell` even when
`class_has_pending_op` returned false, where the answer is provably `true`
(`pure_cell` is false only for a pending operation). It is left alone: the
`PendingSides` site is not reached at all in the `fib(16)` workload (measured: 0
constructions), so the change could not be measured, and the same full walk
stays in place for the six predicates where it is required.

### P4-3 — A thread and a rebuilt combinator graph per parse `verified`

`language-parser/src/parse.rs:94-103` and `:188-201` each
`thread::scope` + `Builder::new().stack_size(16 MiB).spawn_scoped`, and
`parse_inner` calls `program_parser(tokens)` (`:111`) which recurses through the
whole precedence ladder with ~20 `.boxed()` clones (`:425-539`). So per parse:
two thread spawns and a full combinator-tree rebuild. A `OnceLock`-held parser
(chumsky parsers are `Clone` and reusable) removes the construction; by the same
measure `In<'a> = Stream<Cloned<…>>` (`:77`, `:110`) deep-clones every token
payload — one heap allocation per identifier and per string literal, per parse.

**Outcome — the mechanism holds, the fix does not: neither named cost is
removable within this item, and no code changed.** Re-derived first-hand and
measured on this revision; input A is `examples/struct_generic.lichen` (551 B,
38 tokens), input B a generated clean 85 567 B / 24 002-token file (2000
`value_i = [i, i + 1, "name_i"]` bindings). The numbers below are the fastest of
five *alternating* rounds of `parse` and `parse_inner`, so process drift cannot
masquerade as a difference between them; the allocation counts come from a
temporary counting global allocator; every probe was removed before the commit.

| release profile | input A (551 B) | input B (85 KiB) |
|---|---|---|
| `parse` (worker + work) | 608 µs | 117.13 ms |
| `parse_inner` (work, no worker) | 319 µs | 115.73 ms |
| ⇒ thread's share | **289 µs (47%)** | **1.4 ms (1.2%)** |
| empty spawn + join, 16 MiB | 169 µs | 44 µs |
| empty spawn + join, 1 MiB | 173 µs | 40 µs |
| grammar build (`program_parser`) | 47 µs / 247 allocations | 19 µs / 247 allocations |
| token copy (`tokens.to_vec()`) | 1.6 µs / 13 allocations | 592 µs / 4002 allocations |
| allocations for one parse | 2 696 | 1 522 496 |

Repeated runs put the thread's share of input A between 34% and 47% (the machine
is noisy; the alternation keeps the *comparison* honest, not the absolute
figure).

*The spawn is the cost, not the size.* An empty worker at 16 MiB and at 1 MiB
measure the same inside one run (169 µs / 173 µs here; 55 µs / 51 µs in an
earlier one), so the frozen 16 MiB reservation is not what the item is paying
for — `std::thread`'s creation is. The rest of the thread's share is the fresh
stack: the parse's combinator recursion is deep enough (P1-23 measures ~94 KiB of
stack per nesting level) that a new thread's stack pages are faulted in on every
parse.

*The graph cannot be hoisted — it captures the per-parse token slice.* Thirteen
closures in the grammar capture `tokens` to name a recovery node's byte range or
a node's `(line, col)`: `span_at(tokens, …)` / `err_node(tokens, …)` at
`:396, 419, 459, 917, 944, 970, 1001, 1009, 1054, 1098, 1121, 1137, 1359`. The
borrow is in the value's type, so `LazyLock`/`OnceLock` (the note's fix) is
rejected at compile time — asking the compiler to store the built grammar behind
a `T: 'static` bound fails with *"`tokens` does not live long enough … argument
requires that `tokens` is borrowed for `'static`"* (E0597). Hoisting would need
the position data to come from somewhere other than a captured slice — a parser
input whose span carries the position, or a post-pass rewriting token indices
into byte ranges and lines over the AST — which is a redesign of the position
plumbing, not a hoist. The rebuild is also small: 19–47 µs, 247 allocations, the
same for both inputs (a fixed per-parse cost, 8% of input A's parse and 0.02% of
input B's).

*The extent is wrong in one direction, and the token claim is small in size.*
(a) `parse` and `parse_statement_region_traced` are **alternative** entry points,
so a parse call spawns **one** worker, not the note's two. (b) The grammar has
**three** `.boxed()` calls (`:444`, `:588`, `:814`), not ~20; the build's 247
allocations are chumsky's combinator clones. (c) `In<'a> = Stream<Cloned<…>>`
does clone every token payload once during the parse, which is 13 allocations on
input A and 4002 on input B — 0.5% and 0.26% of that parse's 2 696 and 1 522 496
— and a full `tokens.to_vec()` costs 1.6 µs / 592 µs (0.3% / 0.5% of the full
parse). Real, and not where the parse's cost is.

*Deliberately not done, and why it is not this item's call.* A process-lived
16 MiB worker would remove the spawn and keep the stack warm **without** changing
the overflow behaviour `P1-23`/`D9` fixed — the whole combinator recursion would
still run on a 16 MiB stack — but it is a design change, not a local fix, and it
needs a decision among: a per-parse copy of the token stream (592 µs on input B,
where the thread's whole share is only ~1.4 ms, so the copy could eat the win), a
lifetime-erased borrow of the caller's tokens sent to a `'static` worker
(`unsafe`, with the caller blocking until the reply so the borrow cannot outlive
the tokens), or an `Arc<[Token]>` lexer/parser API (crosses three crates). It
would also serialize concurrent parses, which one worker per parse does not.
`P1-23` calls the same kind of change *"a real design change, not an
annotation"*; this item is not the place to pick one. Measured, the rest of the
parse is 53% (input A) to 98.8%
(input B) of the cost and is untouched by either proposed fix.

**Status corrected to `blocked:D13`.** This item was briefly flipped to `done` on
the strength of "no code changed, the refutation is recorded", which is wrong: an
optimization item whose measured cost is real and whose fix is unimplemented is
not done. The cost is real — **47% of a 551-byte parse is the worker's spawn**,
which is exactly the language server's per-keystroke case — and the fix needs a
decision, which is `D13`.

**Outcome — decided, landed, and measured at −37% on the case that motivated it.**
`D13` chose the process-lived worker (one `OnceLock`'d `ParseWorker`, 16 MiB
stack, a job per parse owning its tokens); the decision entry records the choice
against the two alternatives and the re-measurement. On this input the parse is
now **164–203 µs** where it was **260–296 µs** (minima, same session, alternating
rounds), and the 85 KiB input is unchanged inside 2% — the direction and
magnitude the decomposition called for. The refuted half stands as refuted: the
combinator graph is still rebuilt per parse, because it cannot be hoisted, and
that rebuild is 19–47 µs against a worker share that was 289 µs.

### P4-4 — Quadratic diagnostics `reported`

`highlevel/src/diagnostic.rs:424-428` `orphan_unify_errors` is O(E × D);
per-error it also does `apply_errors.iter().find` (`:435`) and
`diary.iter().find` (`:465`). `diagnostics()` is what an editor calls on every
keystroke. One O(E + D) sweep over the diary (marking owned indices in a
`Vec<bool>`) replaces all three. In `language/src/render.rs:112,124`,
`render_all` is O(diags × lines) for the same reason.

**Outcome — four scans, three premises held, and the parser's is two loops
rather than one.**  Re-derived, every cited line had drifted ten to twenty
lines, and the parser site the sweep reported (`parse.rs:114-123`) is not the
only one: the same `errors.iter().any` dedup sits again at `:247-256` in
`region_inner` (the incremental re-parse path).  Both were fixed here — it is
the same defect in the same shape, and splitting one dedup across two items
would have left the other quadratic.

*What each scan was for.*  All four are a linear rescan of a list that grows
as it is built, but they answer three different questions, and only two of
them wanted an index over the diary:

| site (before) | what the scan answered | replacement |
|---|---|---|
| `diagnostic.rs:434-443` `orphan_unify_errors` | "does any diary entry own error `i`?" — asked once per error, over every entry | one `Vec<Option<usize>>` of owners |
| `diagnostic.rs:450` `apply_errors.iter().find` | "which apply error names error `i`?" — the same short list rescanned per error | one `Vec<Option<usize>>` of apply-error indices |
| `diagnostic.rs:480-483` `diary.iter().find` | "which entry owns error `i`?" | the same owner vector |
| `render.rs:119` `line_starts` per diagnostic | the line model, recomputed per caret block | hoisted into `render_all` |
| `parse.rs:117-120` and `:250-253` `errors.iter().any` | "has this exact diagnostic already been emitted?" — a membership test with no set | one `HashSet` keyed on the diagnostic's content |

*The index, and the order it has to preserve.*  `Build::unify_error_index`
(`diagnostic.rs`) fills both vectors in one pass each: the diary's owned ranges
are disjoint slices of the append-only error list, so filling `owner` visits
each index at most once, and both vectors take the **first** writer — `find`'s
semantics, not `insert`'s.  `orphan_indexes()` is then a read of that vector,
so the orphan set and its ascending order are exactly what the old filter
produced, and `mismatch` reads both vectors instead of scanning.  The
`Vec<bool>` the note proposed would have worked for the orphan filter too, but
`Vec<Option<usize>>` is the same size and also answers *which* entry, so the
attribution path reads it rather than keeping a second structure.

*Measured, before → after* (`test` profile, fastest of three rounds; the input
is generated source, described per row).  The parser row's before/after counts
come from a temporary probe that counted loop iterations and was removed:

| input | measure | before | after |
|---|---|---|---|
| N independent `a{i} = 1 + "s{i}"` mismatches, E = D = N, N = 50/100/200/400 | `Build::diagnostics()` | 0.046 / 0.140 / 0.501 / 1.803 ms | 0.019 / 0.035 / 0.077 / 0.208 ms |
| N unresolved names, one per line: D = N diagnostics over L = N lines, N = 200/400/800/1600 | `render::render_all` | 7.005 / 28.998 / 116.400 / 528.302 ms | 0.317 / 0.654 / 1.227 / 2.639 ms |
| 2000 unresolved names over 2001 lines | `render_all` | 854.611 ms | 3.384 ms |
| N statements of `x{i} a => => =>`, 4·N rich errors, N = 4000/8000 | `parse::parse` | 1455.998 / 9967.020 ms | 665.933 / 1273.213 ms |

The first two rows are the claimed shape and it is unmistakable: both grew ~4×
per doubling before and ~2× after, and at 2000 diagnostics the render is 252×
faster.  The parser row is the part of the measurement that needs saying out
loud: **the dedup is genuinely quadratic, and at the error counts a normal file
produces it is invisible.**  The same input at 1600 errors read 303.9 ms
before against 289.2 ms after — inside the noise, because chumsky's recovery
(~0.2 ms per error) dominates the scan (~1 ns per comparison).  At 32 000
errors the linear `any` costs about 8.7 s of the 9.97 s parse — 512 M element
comparisons — and the set makes that parse 1.27 s.  The comparison count is the
premise's own number: `n(n-1)/2` before, `n` after.

*No behavioural change, checked directly.*  A probe dumped `render_all`'s whole
output plus the highlevel diagnostic count for ten failing programs (a unify
mismatch, a runtime apply of a non-function, an array-element mismatch, an
unresolved name, a table miss, a parse-recovery failure, an unresolved name
inside a lambda, a `p = q => q 1; p 5` apply, and two clean programs); the
before and after dumps are identical line for line — same diagnostics, same
order, same carets.  The probe is removed and the workspace suite passes
unchanged.

*Deliberately not done.*  `render` keeps its own `line_starts` call: it is the
public single-diagnostic entry point and has no caller-provided line model to
reuse, so only `render_all` — which already owns the list — hoists it.  The
set's extra allocation (one `String` clone per *unique* diagnostic, to key the
set) is left in: it is one allocation per diagnostic against the n² element
comparisons the scan performed, and avoiding it needs a key borrowed from the
list being built, which the borrow checker will not allow while the list is
being pushed to.

### P4-5 — `path.contains` as a cycle guard; O(n²) kernel codegen `reported`

`equality.rs:272`, `table.rs:178`, `table.rs:257`, `equality.rs:832` do a
**linear scan of a `Vec`** at every recursion level, and `key_eq`/`hash_inner`
*hash* each element, so a table key of depth *d* costs O(d²) hashes. A `HashSet`
beside the existing `Vec` gives O(1) membership with the same push/pop
discipline. Separately, `compute.rs:1009-1011` `class_computation_node` linearly
scans the module's whole node table and is called **per emitted node**
(`emit_node`, `:1069`); `equality_rep` has no path compression.

**Outcome — both premises held as defects, but each needed its extent and its
mechanism corrected; one half of the first claim is stale.**  The guards are
**three**, not four, and none of them is where the note says.  The two halves
of the cycle claim needed opposite verdicts, so they are separated below.

### The `path.contains` guards — three sites, ancestor semantics

Re-derived, the two `Vec` guards are `equality.rs:287` (inside `unify_inner`)
and `equality.rs:872` (inside `reconcile_node`), and the table one is
`table.rs:481` (inside `key_eq`).  `table.rs:178`/`:257` are not guards at all
any more: `:178` is `TableItem` construction in `build_table` and `:257` is
`hash_step`, because `P1-4` deleted the hash's own path check and replaced it
with the `UNFOLD_DEPTH` frontier — the module docs at `table.rs:24-33` state
that.  So the note's "*`key_eq`/`hash_inner` hash each element, so a table key
of depth d costs O(d²) hashes*" is **refuted**: `hash_inner` no longer exists,
and the surviving guard hashes nothing.

*What the guard asks.*  All three test `path.contains(&(a, b)) ||
path.contains(&(b, a))` — the **unordered** pair against the current recursion
path.  That is an ancestor relation, not a visited mark: a pair the walk meets
again in a sibling subtree is not on the path and must be compared again, so
the replacement has to be removed from on the way out.  A plain visited set
would answer "have I seen this node at all?" and would cut comparisons the
comparison is required to make.

*The replacement.*  `AncestorPairs<K>` (`crates/lichen-lowlevel/src/ancestors.rs`,
new) is a `HashSet<(K, K)>` whose `insert` stores **both** orientations and
whose `remove` deletes both, so one hash probe answers the symmetric test.
Each of the three call sites keeps its exact push/pop discipline — the same
frame that inserts removes, on every exit — which is what makes the decision
identical.  The key type is `NodeId` for `unify_inner` (class representatives)
and `AnyNodeId` for `reconcile_node`/`key_eq` (raw nodes), unchanged.

*Measured, before → after* (`test` profile; the input is a right-nested chain
`[0, [0, …]]` of depth *d*, so the walk descends *d* levels and the path is
*d* long at the bottom; the counts are exact — a temporary counter in the
guard, removed before commit — and the times are the minimum of 5 before / 4
after alternating rounds, because this machine varies by ~3× run to run):

| probe | guard scans at d = 50/100/200/400/800, before | after |
|---|---|---|
| `Module::unify` of two equal chains | 5 100 / 20 200 / 80 400 / 320 800 / 1 281 600 | 101 / 201 / 401 / 801 / **1 601** |
| a table read (`key_eq`) against a deep equal key | same | same |
| forcing a deferred read against a committed deep value (`reconcile_node`) | 4 900 / 19 800 / 79 600 / 319 200 / 1 278 400 | 101 / 201 / 401 / 801 / **1 601** |

Before: exactly `2·d(d+1)` entries scanned for the first two probes and
`2·d(d-1)` for the third (its path is one pair shorter) — both quadratic.
After: `d + 1` O(1) probes.  Wall time at d = 800: `unify`
8.62 → 5.30 ms, `key_eq` 9.83 → 4.66 ms, `reconcile` 9.67 → 4.46 ms — about 2×,
not 800×, because the guard was a small share of a walk that also does
disjoint-set finds and node reads; the scan count is the claim, and it is the
number that collapses.

*Also found by this sweep, and deliberately not changed.*  `render.rs:196`
(`TypePrinter::path`) and `render.rs:863` (`TypePrinter::path`/`tpath`) in
`lichen-render` are the same `Vec`-probed guard on the type/value walk — though
a **single-node** test, not this item's unordered-pair one.  They are
outside this item's area (`lowlevel, compute`), no queue item owned them at the
time, and changing them would need this item's measurement repeated on a
type-printing workload; they are recorded here so the next sweep does not have
to find them again rather than folded in.  *`P4-8` later took them, and
corrected this paragraph's arity: see its Outcome.*

### The kernel codegen — quadratic, but only for bodies that reach a bare cell

Re-derived, `class_computation_node` is `compute.rs:1042-1063` and `emit_node`
is `:1096`; the note's lines had drifted about 35 lines.  The mechanism the note
names is real, and the extent is narrower than "codegen":

| program | `class_computation_node` calls | node-table entries scanned |
|---|---|---|
| `k{i} = compute.jit (x => x + i)`, N = 25…400 | **0** | 0 |
| `k{i} = compute.jit (x => compute.launch k0 (x + i))`, N = 10/20/40/80/160 | N | 4 050 / 14 700 / 55 800 / 217 200 / **856 800** |

A plain kernel body is an operation node with a value, so `class_computation_node`
is never reached and its compile time is linear (19.3 / 30.8 / 57.8 / 119.2 /
263.2 ms at N = 25/50/100/200/400).  A **wrapper** body — the
`compute.launch k0 (x + i)` form — collapses the argument to a bare
`Parameterized` cell, which is exactly the case `emit_node` resolves through
`class_computation_node`, so each of the N kernels scans the module's whole
node table (which holds every kernel's nodes): quadratic.  The claim therefore
holds for wrapper-shaped kernels and not in general.

*The fix.*  The class's own member list already lists its members, so the scan
was a lookup with no index: `disjoint::members(&module.nodes, root)`.  A
class is 2–3 members here, so the per-call cost becomes O(class) instead of
O(module).

*Measured, before → after* (min of 3 alternating rounds each; same generated
wrapper program, N kernels):

| N | entries scanned before | members visited after | compile before | compile after |
|---|---|---|---|---|
| 10 | 4 050 | 30 | 18.2 ms | 16.8 ms |
| 20 | 14 700 | 60 | 27.9 ms | 26.1 ms |
| 40 | 55 800 | 120 | 50.8 ms | 48.3 ms |
| 80 | 217 200 | 240 | 103.9 ms | 90.5 ms |
| 160 | 856 800 | 480 | 317.7 ms | **182.9 ms** |

The scan count goes from ~N² to 3N; the wall clock moves from 1.7× at N = 160
and grows, since the removed term is the growing one.  The plain-kernel row is
unchanged, as its zero calls predict.

*The one selection-order hazard, measured rather than argued.*  The old scan
returned the first computational member in `nodes.keys()` (slot) order; the
member walk returns the first in the class's *member-list* order, which
`disjoint::union` builds by splicing the smaller set onto the larger one's
tail.  The two can differ only when a class holds **more than one**
computational member, and then the emitter could emit a different one of
them — a behavioural change, not a refactor.  A temporary probe counted the
candidates per call across the whole workspace suite (`cargo test --workspace
-- --nocapture`): **no class ever held more than one**, so on every class any
test reaches the two orders return the same member, and the suite's kernel
execution tests assert the emitted code's results unchanged.  The residual is
stated rather than hidden: a future program whose equality class unifies two
*different* computations could select a different member than the scan did.

*Deliberately not done.*  `equality_rep`'s lack of path compression
(`compute.rs:1044-1055`) is untouched.  The note names it beside the scan, but
it is not what was quadratic: `disjoint::union` attaches the smaller set under
the larger, so a chain is O(log n) deep, and adding compression needs `&mut`
(a read-only `&Module` cannot carry it) or a second representation.

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

**Outcome — two claims fixed, one refused by this batch's own constraints, one
that is a redesign and needs a decision.**  The four claims got four verdicts,
and the first one's *mechanism* was wrong while its defect was real.

### Per-apply clones — the node-list clone is not per apply; two other clones are

**Refuted as stated.**  `function.rs:100-106` clones **`asserts` only**
(`function.asserts.clone()`); it never clones the template's node list.  The
node-list clone the note points at is `function.rs:430-438`, the
**nested-closure** branch, and it fires per *closure instantiation*, not per
apply: on `fib(16)` the counters read `nodes_clones = 0` across 3 193 applies.
`static_module.rs:125` is `set_node_shape` and `:327` is a match guard, neither
a clone; the static apply's per-call clones are `asserts.clone()` (`:164`) and
`HashMap::new()` (`:169`) plus `regroup_clones` (`:199`).

**What was really there, and what it cost.**  Two clones on every apply, at
both apply entry points:

| site | what it was | cost |
|---|---|---|
| `function.rs:105`, `static_module.rs:164` | a `Vec` clone of the function's assert list, taken because the loop body needs `&mut module` | one clone per apply; an allocation only when the function carries asserts |
| `apply.rs:178-191` `regroup_clones` | a `HashMap<K, Vec<NodeId>>` of the clones grouped by template representative | one hash-table allocation **plus one `Vec` per group**, per apply |

*The fix.*  Both assert lists are now walked **by index** (`for index in
0..assert_count`, reading `module.functions[function].asserts[index]` per
iteration), which drops the clone without holding a borrow across the call: the
list is only read, and the entries the loop adds go to `module.asserts`, the
per-call registry, not to the function's own.  `regroup_clones` now returns one
`Vec<(K, NodeId)>` sorted by representative, and `unify_clone_groups` reads it
as runs; `LocalNodeId` gains `PartialOrd, Ord` (its own `usize` index, not an
opaque key encoding) for the static path's key type.

*Why the order is safe.*  A group's clones are paired `first` with each other
clone in the order the walk inserted them — preserved, because the sort is
stable and the within-group sequence comes from the same `remap` iteration the
old code pushed in.  Only the **order the groups are visited in** changes, from
the iteration order of a `HashMap<K, Vec<NodeId>>` to the representative's
order.  That order was already arbitrary — `HashMap`'s hasher is seeded per
process — so nothing could depend on it, and the groups are disjoint sets of
freshly minted nodes, so unifying one cannot affect another.

*Measured, before → after* (`fib(n)` built by the lowlevel harness, the
definition pass driven first so the recursion really runs, then the call
deep-evaluated; a temporary counting global allocator and site counters, all
removed before commit):

| input | allocations/apply | total allocations | assert-list clones/apply |
|---|---|---|---|
| `fib(10)` (177 applies) | 34.14 → **14.14** | 6 043 → 2 503 | 1 → 0 |
| `fib(12)` (465) | 34.06 → **14.06** | 15 838 → 6 538 | 1 → 0 |
| `fib(14)` (1 219) | 34.03 → **14.03** | 41 480 → 17 100 | 1 → 0 |
| `fib(16)` (3 193) | 34.01 → **14.01** | 108 599 → **44 739** | 1 → 0 |

Twenty allocations per apply removed, and `fib(16)`'s wall time fell from
150.6 ms to 129.0 ms in the same session (the machine varies by up to 3× run to
run; the allocation count is the deterministic measure).  The template's
node-list clone was not touched — it is not on this path.

### Repeated `as_enum` — held, extent corrected

`value_is_parameterized` (`evaluation.rs:716-771`, not the note's `:588-634`,
which is the cycle-cut region above it) took `value.as_enum()` **three** times,
not four, and iterated `array.items()` twice and `table.items()` **twice**, not
four times.  The view is now taken once into a local.  Measured: one
`evaluate_node_deep` of a table-valued node goes from **9 to 3** `as_enum`
calls (the table node and its two leaves, one view each), and over `fib(16)`
from 71 832 to 23 944 (22.5 → 7.5 per apply).  The `items()` calls are pointer
arithmetic and were left as they are.

### Per-byte `mix` — real, and refused by this batch's constraints

`table.rs:287-290` folds a string key one byte at a time through `mix` (three
multiplies and two shifts per byte).  Measured with `build_table` over one
string key (`test` profile): 1 KB → 0.0147 ms, 10 KB → 0.044 ms, 100 KB →
0.418 ms, 1 MB → **4.29 ms** — linear in bytes at ~4.3 ns/byte.

**Not fixed, and the reason is a constraint, not the cost:** that fold *is* the
table-key content hash.  A word-at-a-time rewrite cannot preserve the value —
each byte's `mix` depends on the previous one — so it would change every string
key's hash, the payload sort order, and every hash stored in every existing
artifact, and the batch's standing constraint is *"do not touch … the table
hash"* (with the artifact container's version equally out of bounds).  Marked
`wontfix:<constraint>`: a hash-preserving speed-up does not exist, and a
hash-changing one is a format change with an owner elsewhere.

### The intern leak — real, measured, and a redesign: `D14`

Confirmed first-hand.  `Compiler::new()` is fresh per `compile_resolved`, so
`op_names`/`str_names` dedupe only *within* one compile, and `Expr::Str`
(`compile.rs:398-403`) leaks **every string literal unconditionally**, with no
dedup at all.  Measured with a tracking global allocator, net live bytes after
each block of compiles of the *same* source (nothing retains a `Report`, so
every other allocation a compile makes nets to zero):

| source | leaked per compile | after 1 000 compiles |
|---|---|---|
| `a = "hello world"` (an 11-byte literal) | **11 bytes** | 11 000 bytes |
| a named-field read (`x.alpha`, a 5-byte name) | **5 bytes** | 5 000 bytes |
| `a = 1; b = "x"; a + 1` (a 1-byte literal) | **1 byte** | 1 000 bytes |
| an editor-like stream of changing sources | **31 bytes** | 31 000 bytes |

Unbounded and linear in compiles; the editor case leaks on every keystroke,
exactly as the note says.

**Not fixed: this is the redesign the note warned about, and `D14` decided to
keep it.**  The `&'static str`
is load-bearing — `ExprKind` must stay `Copy` (`compile.rs:192-199`) and the
literal rides in the highlevel's `HighProgramLiteral::StrLit(&'static str)` —
so tying the lifetime means a lifetime parameter on the literal and on
`ExprKind`, rippling through `IR`, the checker and `persist`.  A process-global
intern table is the other candidate, and it is *not* a fix: it bounds growth to
the number of *distinct* strings ever seen while still never reclaiming any,
and it does nothing for the editor, where each keystroke is a new string.
Choosing between "own the strings in the IR" and "keep a process-lifetime
intern table" is a policy for how long interned data lives, so it became
`D14` — see [Decisions](#decisions) — and nothing was changed for it here.
**`D14` then decided to keep the leak** (the rate is ~3 MB per 100 000
keystrokes) and to state it at the leak sites, so the two candidates above are
recorded as *rejected for now* rather than unimplemented.
The deserializer's `codec.rs` leak is the same shape and carries the same note.

### Found, not one of the four, and not fixed here

- `apply_errors` was deduped with a linear `iter().any(...)` (`apply.rs:114`
  before this item's rewrite — the note's `:114`) over a list that is
  append-only and never cleared: the same shape as `P4-4`'s parser dedup, on a
  different list.  `P4-4` is done and its scope was the highlevel/render/parser;
  this site was unowned, so it was reported rather than folded in — and `P4-7`
  has since taken it.
- `compute.rs:1968` (the note's `:1879`) leaked the 9-entry `NativeOps` slice
  once per `compute_native_ops!` call — 144 bytes per
  `PackageStore::register_compute`, which is once per store because the handle
  is then served from `native`.  Bounded, and not this item's claim; `P4-9` has
  since stated the bound in the macro's doc (`compute.rs:1942-1951`).
- `registry/codec.rs:42-45` (the note's citation, before `P1-27` grew the leaf
  writer) wrote a `u8` leaf-name length, so a carry-variant name past 255 bytes
  truncated and desynchronised the artifact stream.  That is an artifact-codec
  correctness defect, not an optimization, and it belongs to `P0-5`/`P0-7`'s
  container; reported here, not changed.  `P1-27` has since taken it
  (`Writer::leaf` is now `codec.rs:64-79`).

### P4-7 — `apply_errors` is deduped with a linear scan `verified`

The same shape `P4-4` fixed on two other lists, at a site `P4-4`'s scope did not
include: `crates/lichen-lowlevel/src/apply.rs` dedupes `apply_errors` with a linear
`iter().any(...)` over a list that is **append-only and never cleared**, so a run
that records `n` apply errors pays `n(n−1)/2` comparisons.

**Outcome — premise held at the re-derived line, and the measurement is the
note's own `n(n−1)/2` exactly.** Re-read first-hand: `apply.rs:114-123` was
`!self.apply_errors.iter().any(|e| e.apply_node == node)` before the push, and
`apply_errors` is the `pub` field at `lib.rs:932` that nothing clears. Counted
with the method `P4-4` used — a temporary counter in the replaced scan, removed
before commit — on a workload that records many apply errors: a frozen
`f([a, b]) = a` with `a` and `b` unified (a homogeneous array parameter, the
shape `static_parameter_topology_is_reestablished_among_clones` builds), driven
through `N` distinct failing call sites, so each records one `ApplyError`.

*Measured, before → after* (`test` profile; the count is exact):

| N failing call sites | `apply_errors` | scan entries visited, before | index probes, after |
|---|---|---|---|
| 50 | 50 | 1 225 | **50** |
| 100 | 100 | 4 950 | **100** |
| 200 | 200 | 19 900 | **200** |
| 400 | 400 | 79 800 | **400** |
| 800 | 800 | 319 600 | **800** |

Before: exactly `n(n−1)/2` per the table (`800·799/2 = 319 600`), quadratic.
After: `n`, one hash probe per recorded error.

**The replacement, and why it is a set rather than `P4-4`'s index.**
`P4-4`'s two sites asked *which* entry owns an error index, so a
`Vec<Option<usize>>` beside the list answered them in one pass. This site asks
only *has this apply node already been recorded* — a membership test keyed on
`NodeId`, the `apply_node` field — so the structure beside the list is a
`HashSet<NodeId>` (`Module::apply_error_nodes`, `lib.rs:933-938`), and the check
becomes `if self.apply_error_nodes.insert(node)` (`apply.rs:114`). `insert`
returns whether the node was new, which is exactly the old `any`'s negation.

**No behavioural change, checked directly.** The decision is identical by
construction: the same nodes are new on the same calls, so the same entries push
in the same order, and the diagnostic sequence `P4-4`'s `unify_error_index`
re-reads from this list is unchanged. Both stay append-only and are never
cleared, so the set cannot drift from the list; the field is private, so no
out-of-crate writer can push to one without the other. The workspace suite
passes — `cargo test -p lichen-lowlevel` (139 tests, including the static
parameter-topology test that asserts the `apply_errors` entry) and
`cargo test -p lichen-language --test pipeline` (123) both green — and
`cargo clippy --workspace --all-targets -- -D warnings` and
`cargo fmt --all -- --check` exit 0.

### P4-8 — Two more ancestor guards scan the path they guard `verified`

`crates/lichen-render/src/render.rs`'s type walk (`TypePrinter::path` and the
`path`/`tpath` pair in `element_any`) uses the same `path.contains` ancestor guard
`P4-5` replaced with `crates/lichen-lowlevel/src/ancestors.rs`, in a crate `P4-5`
did not name.

**Outcome — the defect is real and the sites are the two named; the prescribed
reuse was the wrong type, and the difference is the guard's arity, not its
semantics.**  Re-derived first-hand, both guards are exactly where the note
says — `render.rs:196` (`TypePrinter::node`) and `render.rs:863`
(`ValuePrinter::element_any`) — and both are a linear `Vec::contains` on a path
that grows with the recursion depth.

**What `P4-5`'s Outcome does *not* carry over, and why.**  `P4-5` established
that its three guards ask *"is this unordered **pair** on the current path"*,
which is why `AncestorPairs` stores both orientations.  These two ask a
different question: `if self.path.contains(&node)` and
`if self.path.contains(&id) || self.tpath.contains(&ty)` test a **single node**
against the path.  Substituting `AncestorPairs` would be neither a reuse nor
behaviour-preserving: it would cut only when a value/type *pair* recurs, where
the current guard cuts whenever either half recurs with any partner — a wider
relation, so more cycles would print a level deeper.  The task's instruction
("reuse `ancestors` … the test is the unordered pair") therefore did not hold
for these two sites, and `AncestorPairs` was not used.

**What was done instead.**  The shared guard structure *was* reused — the
module `P4-5` created is now the one place both arities live.  `ancestors`
gains `AncestorNodes<K>` (`ancestors.rs:23-73`), the one-node guard whose
contract is `AncestorPairs`'s read for a single node (insert after a `false`
`contains`, remove on every exit), and `AncestorPairs` now wraps it
(`:75-123`) so the `HashSet` has one implementation rather than two.  The module
became `pub` (`lib.rs:18`; both types are `#[doc(hidden)]` — the contract is
internal, it is not a host-facing API) because `lichen-render` depends on
`lichen-lowlevel` and must be able to name the guard; nothing else about the
module changed, and the three `P4-5` call sites are untouched.

In `render.rs`, `TypePrinter::path` (`:127`, `:174`) and `ValuePrinter`'s
`path`/`tpath` (`:632`, `:634`, `:654-655`) become `AncestorNodes<NodeId>`, and
the six guard operations become `contains`/`insert`/`remove` (`:197`, `:209`,
`:211`; `:864`, `:867-868`, `:874-875`).  Each site keeps its exact push/pop
discipline, so the decision is identical.

*Measured, before → after.*  `P4-5`'s method — a temporary counter at the
guard, removed before commit — on a type-printing workload, since `P4-5`'s
numbers were taken on unification.  The input is a right-nested **array type**
`[…[[Int, 0], 1]…, d]` built by hand at depth *d*, printed by the real
`TypePrinter`; one level is three nodes, so the walk descends *d* levels and the
path holds *d* nodes at the bottom.  The counts are exact and the printed text
is byte-identical before and after at every depth (4481 bytes at *d* = 170):

| depth | nodes visited | guard entries scanned, before | index probes, after |
|---|---|---|---|
| 100 | 701 | 70 600 | **701** |
| 120 | 841 | 101 520 | **841** |
| 150 | 1 051 | 158 400 | **1 051** |
| 170 | 1 191 | 203 320 | **1 191** |

Before: `nodes × depth` — one scan per recursion level, quadratic; after: one
probe per visit, `nodes`.

**Wall time on this workload did not move, and that is reported rather than
smoothed over.**  2000 prints at *d* = 170 read 4716–4968 ms with the `Vec`
guard against 4973–5151 ms with the set (min of four alternating rounds) — the
set is not slower by more than the machine's own spread and is not faster
either.  Two facts bound the item: the guard's scan count is the claim and it
collapses 171×, and **the depth is capped by the printer's own recursion** — a
depth of 200 overflows the test harness's stack *both before and after* this
change, so the guard was never the binding constraint on how deep a printable
type can be.  The quadratic term is real and removed; it was not what made
type printing slow at the depths reachable here.  The set's memory cost (one
`HashSet` per printer, at most *depth* entries during a walk, against the `Vec`
it replaces) is not a regression.

**Verification.**  `cargo test -p lichen-render -p lichen-language -p
lichen-lowlevel` passes unchanged (the render and language suites, including
every type- and value-printing test, 70 + 123 + 139 among them);
`cargo clippy --workspace --all-targets -- -D warnings` and
`cargo fmt --all -- --check` exit 0.

### P4-9 — A `NativeOps` slice is leaked per registration `verified`

`crates/lichen-compute/src/compute.rs` leaks a 9-entry `NativeOps` slice once per
`compute_native_ops!` call — 144 bytes per `PackageStore::register_compute`. It is
**bounded** (once per store, because the handle is then served from `native`), so
this is a tidiness item rather than a leak in the growing sense.

**Outcome — the premise held, the "`&'static` initializer" half of the fix was
attempted and is structurally unavailable, and the leak is documented as
deliberate rather than removed.**  Re-derived first-hand: `compute.rs:1980` is
`Box::leak(ops.into_boxed_slice()) as NativeOps<$program>` (the counter addresses
19 fields below the note's `:1879`), the table is the nine `(name, &dyn NativeOp)`
entries above it (`:1969-1979`), and `&'static dyn` is two words, so the slice is
`9 × 16 = 144` bytes on a 64-bit target — the note's own figure.

*The fix that does not exist, and the evidence.*  Returning the slice from a
`&'static` initializer — a `static TABLE: OnceLock<Box<[...]>>` in the macro
expansion, with `get_or_init` — was written and **fails to compile**:
`error[E0401]: can't use generic parameters from outer item`, at the static in
the expansion of `lichen-language/src/package.rs:53` (the macro is invoked from
the generic `compute_native_ops::<P>()`).  This is not a syntax problem: a
`static` may never name a type parameter, and an item declared in a macro
expansion is a *distinct item per invocation*, so two invocations naming the
same program cannot share one `static` either.  A host-side cache is no better —
the table would be built per store, which is exactly the one call per store that
happens today.  The attempt was reverted, and `Box::leak` stays; no second
mechanism was added for 144 bytes (`P4-9`'s own constraint: do not restructure
the registration path for it).  What changed is that the macro now **states the
bound** (`compute.rs:1942-1951`): the slice is 144 bytes, exactly one is
allocated per call, the initializer alternative is impossible for the reason
above, and the one call site runs once per store.

*Why "once per store" is exact here.*  `register_compute` is reached from
`load_package` when the path's file name is `compute.lichen`
(`package.rs:247-252`), and it inserts the frozen handle into `self.native`
(`:364-365`).  `load_package` consults `self.native` first (`:240-243`), so a
second import of the same store's `compute.lichen` is served from the map and
does not recompile.  The leak is therefore one 144-byte slice per `PackageStore`
that ever imports compute — bounded, and not per keystroke or per compile.

**Verification.**  `cargo test -p lichen-language --test compute` (24 tests,
including the `jit_cross_kernel_*` paths that resolve `$jit`/`$launch` through
this registry) and `cargo test -p lichen-compute -p lichen-language --test
std_native` pass; `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo fmt --all -- --check` exit 0.

**Worth stating while reasoning about the bound, and not a finding of this
item.**  `load_package` consults `self.native` (`package.rs:240-243`) **before**
the `compute.lichen` arm (`:247-252`), so after the first registration
(`:364-365`) a second import is served from the map rather than recompiling the
wrapper.  This is pre-existing — `P4-9` changed no load-path behaviour — and it
is what makes "once per store" exact; the comment at `:245-246` ("it
self-registers on first import") describes the import that compiles.

### P4-10 — The type printer recurses per type depth and overflows at ~200 `verified`

Found while measuring `P4-8`, and recorded rather than left as a footnote: the
guard's scan count collapsed by 171× but **wall time did not move at all** (2000
prints at depth 170: 4716–4968 ms with the `Vec`, 4973–5151 ms with the set),
because the binding constraint is the printer's **own recursion**, not the guard.
A hand-built right-nested array type of depth 200 **stack-overflows the process**,
before and after that change.

That makes a **fourth** stack-exhaustion path, and unlike the other three it is in
the renderer, which the language server reaches on hover — so it is reachable from
a deeply nested *type* in a file the user merely opened.

**Closed `wontfix:D9`, with `D9`'s reasoning transferred:** a file that aborts a
tool the user ran on their own machine crosses no privilege boundary, and the
editor exposure has the same shape as `P1-23`'s. It is recorded separately rather
than folded into `P1-23` because the *site* is different — that one is the parser's
worker, this is a printer — and because anyone measuring render cost needs to know
the depth is capped by recursion rather than by the guard. If that reasoning is
wrong for the renderer specifically, this is the item to reopen.

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
  the field types in the shape and `[[id, names, names_in_order], TypeStruct]`
  in the kind, the
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

### P5-14 — Citations invalidated by the file splits `verified`

`P2-11` moved code into twelve new modules, so several notes' `file:line`
citations now name a file that no longer holds the line, or a line that has moved.
Two are known:

- `docs/notes/lowlevel-low-types.md:125` cites `static_module.rs:539`, which is now
  in `static_module/freeze.rs`.
- `docs/notes/type-system-cleanup-plan.md` carries stale line references of its own
  (a closed plan, but its citations are still read).

**Fix.** Re-derive and re-point them, or delete the citation where the line number
adds nothing. A closed plan's *methodology* is historical and stays; its
*pointers* to live code are not. Sweep the other notes for the same drift while
you are there — the splits touched `render.rs`, `static_module.rs`, `resolve.rs`,
`checker.rs`, `compile.rs`, `package.rs` and `parse.rs`, so any note citing those
by line is suspect.

Also fix the truncated doc comment `P2-11` moved verbatim:
`crates/lichen-highlevel/src/checker/native_call.rs`'s module doc jumps from
*"…the private contract with its own source."* straight into a parenthetical — the
lead-in sentence was already missing before the move (`P5-3`'s class).

**Outcome.** Both known citations reproduced, and the sweep found the drift
confined to three notes; nothing else in the 33 notes (the ledger aside) cites a
split file by line.

*The sweep's method, so it can be repeated.*  Every note under `docs/notes/`
except `code-audit.md` was searched for a split file followed by a line number
(`(render|static_module|resolve|checker|compile|package|parse)\.rs:<digit>`) and
for the bare shorthand form (`` `:<digit> ``).  After the fix the only surviving
match is `type-system-cleanup-plan.md`'s `tests/checker.rs:167-184, 1066-1070` —
a *test* file, which the splits did not touch.  The ledger itself was left
alone: its citations are each item's own pre-fix evidence, not pointers to live
code, and rewriting them would rewrite the records.

*`lowlevel-low-types.md:125` — re-pointed.*  `static_module.rs:539` at the
pre-split revision (`f5b2d72^`) is the `from_module` doc block, the freeze entry.
The split moved the whole `impl StaticModule` freeze to
`crates/lichen-lowlevel/src/static_module/freeze.rs` (the doc at `:13`,
`from_module` at `:34`, `align_up` at `:183`, `rewrite_value` at `:195`), so the
citation is now `static_module/freeze.rs` — the file, no line, since the note's
claim is only that the plumbing exists.

*`frontend-syntax-separation.md:109` — line deleted.*  Its `compile.rs:201` is
not the split's doing: even at the pre-split revision that line is the `spans`
field's doc comment, while the row describes a *span copy*.  The live analogue
is `compile.rs:310` (`self.spans[p.0 as usize] = self.spans[value.0 as usize]`,
the placeholder's span index taking the resolved value's), but the table is the
note's own pre-split coupling inventory — its status line says so — so
re-pointing the row at a live line would misdate it.  The line number was
deleted; the file reference stays.

*`type-system-cleanup-plan.md` — 16 citations.*  Every `checker.rs:<line>` in the
diagnosis (`:81`, `:82`, `:104`, `:105`), the B1 row (`:115`) and the panic
census (`:303`, `:315-322`) names the pre-split checker root, which is 1765 lines
at `f5b2d72^` and 957 now; `lowlevel/static_module.rs:45, 747, 783, 835`
(`:344`) names the pre-split 933-line file, 248 now.  The two that still have a
live home were re-pointed: `tag_descent`'s structural guess, which the plan's own
§3 says moved to `shape.rs` (`tag_descent` is at `shape.rs:616`), and the
`static_module` row, whose four sites are now `static_module.rs`
(`StaticModuleCache::new`, `:52`) and `static_module/freeze.rs` (the phase-2
layout).  The rest lost their line numbers, keeping the file, and the census
carries a note saying why.  A closed plan's methodology — the classification, the
probe results, the branch and commit ids — is untouched.

*The truncated doc.*  The lead-in was already missing when the sentence was
written, not produced by the move: `a8a0be8` introduced
*"…the private contract with its own source."* immediately followed by
*"a diagnostic rather than a panic (…)"*, with no subject.  Read off the
function below it, the doc now reads: *"An unregistered `name` is refused with a
diagnostic rather than a panic (the frontend compiles `$name` blind, so the
checker is the first layer that can see the registry)."* — which is what the
`None` arm does (`record_guard` with `DiagKind::NativeOpUnresolved` at the
call's own span, leaving the expression uncompiled so `check_failed` skips the
definition pass).

*Found and left, because it is not this item.*  The same census probes with
`cargo run -p lichen-language --bin lichen-compiler -- <file>`, which `P2-12`
invalidated when the CLI moved to its own crate; that is a `P2-12` residual
reference, not split drift, and is reported rather than fixed here.

*Gates.*  `cargo clippy --workspace --all-targets -- -D warnings` exits 0,
`cargo fmt --all -- --check` exits 0, `cargo test --workspace` passes, and
`cargo test -p lichen-language --test pipeline --test examples --test persist
--test registry` passes.

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
- **D13 — The parser's per-parse worker. — DECIDED: (a), a process-lived worker
  fed a per-parse copy of the tokens. Landed.** Measured: a
  fresh thread is **47%** of a 551-byte parse (289 µs of 608 µs, release) and
  **1.2%** of an 85 KiB one, and the cost is the **spawn**, not the 16 MiB stack
  (1/16/64 MiB spawns measure the same). The other half of `P4-3`'s finding is
  closed as refuted: the combinator graph **cannot** be hoisted, because 13 of its
  closures capture the token slice, and storing it behind a `'static` bound fails
  to compile (`E0597`, "`tokens` does not live long enough").

  *Chosen — (a), the copy.*  The three ways to feed a process-lived worker were
  all unpalatable on paper; the tie-break is that they differ by **three orders
  of magnitude on the input the item exists for**. The token copy is 1.6 µs on
  the editor's 551-byte file, where the spawn it removes is 289 µs; on the 85 KiB
  one it is 592 µs against a 1.4 ms spawn share, so it eats less than half the
  win there and leaves that input ahead. That makes (a) the *safe* choice and
  the *fast* one at once, where (b) is `unsafe` for no measured gain and (c)
  crosses three crates' APIs for none. `ParseWorker` is therefore one
  `OnceLock`'d thread with the 16 MiB stack, each parse a job that owns its
  tokens and answers on a channel.

  *Rejected — the serialization objection, on the facts.*  One worker does
  serialise concurrent parses where per-parse threads did not, which is a real
  change in shape.  It is accepted because nothing that parses concurrently is
  on a hot path: the language server serialises requests anyway
  (`concurrency_level(1)`) and the CLI parses one file.  A pool would need reply
  routing for no measured win, so the shape is recorded rather than abstracted
  away.

  *Rejected — leaving it to `P1-17`'s cache.*  The cache removes a repeat request
  for one text and does nothing when the text changes, which is every keystroke.
  That is exactly the input (a) is measured on.

  **Re-measured on landing, same session, alternating rounds, release** (input A
  = `examples/struct_generic.lichen`, 539 bytes / 38 tokens; input B = the
  generated 81 560-byte / 24 001-token file, as `P4-3` describes):

  | | before | after |
  |---|---|---|
  | A, min | 259.6–296.1 µs | **164.3–203 µs** |
  | A, median | 354.7–422.1 µs | **177.6–271.9 µs** |
  | B, min | 102.9–105.5 ms | 101.1–103.8 ms |

  Best-against-best on the minimum — the statistic that survived the noise — A
  goes **259.6 → 164.3 µs, −37%**, and B is inside 2%, which is what the
  decomposition predicted (a 1.2% spawn share against a 0.6% copy).  Absolute
  figures are *not* comparable to the ones `P4-3` recorded: that session's
  baseline was ~608 µs for the same input where this one measures ~270 µs, which
  is why the comparison above was re-derived by building both sides in one
  sitting rather than against the note.

  Pinned by `parses_share_one_worker_thread` (both parses on one thread, and not
  the caller's) and `a_panicking_parse_leaves_the_worker_alive`.  The second
  guards the hazard the reuse *introduces*: a per-parse thread contained a panic
  to its own parse for free, and a shared one does not unless the panic is caught
  on the worker and resumed on the caller.  Watched to go red — with the catch
  removed, the first parse's panic kills the worker and the **next** parse fails
  with "the parse worker is gone", which is what a long-lived host would see and
  a single-shot test never would.
- **D14 — Where the IR's strings live. — DECIDED: (c), keep the leak and
  document the rate; re-measure before revisiting.** `P4-6`'s intern
  leak is real and measured: every compile permanently leaks every string
  literal (`Expr::Str` has no dedup at all) and every distinct interned name, at
  1–11 bytes per literal per compile and 31 bytes/compile on an editor-like
  stream of changing sources. Nothing reclaims any of it.

  *Chosen — neither fix, because the note never priced the leak in absolute
  terms and the absolute terms are small.*  31 bytes per compile is **about 3 MB
  per 100 000 keystrokes**; a heavy editing day is 10 000–50 000 edits, so under
  1.6 MB.  That is an unbounded growth, and unbounded is what the item is about —
  but it is unbounded at a rate that does not justify either candidate:
  - **own the strings in the IR** — a lifetime parameter on the literal and on
    `ExprKind` (or an owning arena the IR borrows from), rippling through `IR`,
    the checker, and `persist`'s codec.  This is the only *real* fix (it
    reclaims), and it is the largest change in the ledger, to be paid against
    bytes per keystroke.
  - **a process-global intern table** — dedups identical strings across
    compiles, but still never reclaims anything and does nothing for the editor,
    where each keystroke's literal is a new distinct string.  It bounds the
    growth rate, not the growth.

  The decision is therefore recorded **at the leak sites themselves**
  (`compile.rs`'s `intern_op` / `intern_str` and the `Expr::Str` arm), not only
  here, because the failure mode this guards against is a reader concluding the
  leak was overlooked: the sites now state the measured rate, why the
  `&'static str` is load-bearing, and that `D14` is the decision to revisit
  rather than the comment to delete.  `lowlevel/codec.rs`'s deserializer leak is
  the same shape and carries the same note.

  *What would revisit it:* the rate changing by orders of magnitude (a host that
  compiles far more often than a keystroke stream), or `ExprKind`'s `Copy`
  ceasing to be a requirement for another reason — at which point (a) costs only
  the ripple and buys the whole leak back.
- **D15 — Who owns a compiled kernel or buffer? — DECIDED and landed: the
  arena owns buffers, the process owns kernels (content-addressed).** The
  original finding stands: `KERNELS`/`BUFFERS` grew without
  bound — one fragment per `$jit`/`$parallel` evaluation and one `count`-element
  vector per `plrun`, nothing ever removed; measured, one distinct program
  evaluation adds exactly one of each (60 evaluations took the registries from
  179 to 239 kernels and 2 to 62 buffers).  (`P1-18`'s other two claims are fixed
  — the derived-module cache and `plrun`'s element bound.)

  The reason it was a decision: the registry cannot tell when an entry is
  unreachable. `KernelId`/`BufferId` were `pub type … = usize`, so the id is
  `Copy` and is copied into node value caches, equality classes, apply clones
  and static modules; nothing observes the last copy dying, and the arena has
  no per-value `Drop` to hang a release on. Eviction would have to guess, and
  the first eviction of a live id turns a later `launch`/`read` into the lazy
  marker — a silent wrong answer, worse than the leak. A bound that *refuses*
  new entries instead never drops a live one, but it permanently bricks a
  long-lived host at N programs and changes a working program's answer, which
  is a functional regression rather than a memory bound.

  **The `Arc`-in-the-value shape is priced, and it is the whole lowlevel.**  The
  `Copy` requirement is not per-variant: `P::Value`'s contract is
  `ValueExt: Debug + Copy + PartialEq` (`lowlevel/lib.rs:416`), so one non-`Copy`
  variant forces the bound off the *entire* vocabulary.  Dropping `Copy` from
  `ComputeValue`, `LangValue` and that bound turns the workspace into **70 errors
  across 13 files in `lichen-lowlevel`** — every site that copies a node value,
  in a VM whose value read and write paths are hot.  That kills the option, and
  with it the whole "owner in the value" family.

  **Buffers: the block arena, as `AnyHandle<[i64]>`.**  The mechanism the note
  never considered is the one the codebase already uses for compound data: a
  `LowValue::Array` is a `Copy` handle into a block's bump arena, and
  `AnyHandle<T>` is `Copy` for any `T` (`lowlevel/lib.rs:601`).  `ValueExt`
  defines the **ext-handle payload** contract for a vocabulary's own payload
  (`is_handle`/`handle`/`set_handle`/`alignment`), and the lowlevel's copy path
  routes a *program-specific* value to `copy_ext`, which consults it — so a
  buffer payload is relocated like any other and **dies with its block**.  No
  registry, no id, no eviction, no aliasing, and `Copy` is untouched: the value
  stays a handle.

  What was actually missing was smaller than "unexercised": the composed
  `LangValue` hard-coded `is_handle() -> false` ("the composed values are
  structurally inert"), so **no plugin leaf could own a payload at all**.  The
  composition now dispatches `is_handle`/`handle`/`set_handle`/`alignment` to
  its leaves, which is what makes the `None => copy_ext` arm in the lowlevel's
  copy paths reach a plugin rather than silently skipping it.  Measured caveat,
  recorded rather than hidden: a `Bump` never reclaims per-object, so repeated
  `plrun`s inside one *live* module accumulate in that block until GC drops it —
  the editor's case is exact (P1-17 drops the `Build` per analysis, so buffer
  memory is bounded by open documents), and a long-lived single module leans on
  `garbage_collect`.

  **Kernels: the process, content-addressed.**  A fragment is not a leaf payload:
  `KernelInstr::CallKernel(KernelId)` makes it reference *other* fragments, and
  assembly resolves those through the registry, so "immutable artifacts shared
  across modules" is load-bearing for the call graph and not merely a cache.
  They stay process-global, but ids become a **content digest** with an intern
  index and a unique-id fallback on a genuine collision (so an id can never alias
  a different fragment — the failure mode is a recompile, never a wrong kernel).

  Content addressing is not a tidiness change; it is what makes the derived
  module cache work at all.  That cache is keyed on `(LaunchMode, KernelId)`, so
  a fresh id per compile meant it **could never hit**, and every keystroke
  re-assembled and re-ran `wasmi::Module::new`.  Measured on the same
  `jit`+`launch` program compiled three times in one process, counting module
  cache misses:

  | | round 0 | round 1 | round 2 |
  |---|---|---|---|
  | a fresh id per compile | +1 | +1 | +1 |
  | content-addressed | +1 | **+0** | **+0** |

  The counter is `compute::module_cache_misses`, visible for tests and
  measurement for the same reason the package store's `compiled`/
  `loaded_from_cache` are.

  Pinned by `lichen-language/tests/buffer_payload.rs` (a pointer comparison, not
  a content read — a dangling bump payload frequently still *reads* correctly,
  so a content-only test would pass on a broken relocation) and by the two
  `kernel_intern_tests` in `compute.rs`.  All three were watched to go red: with
  the leaf answering `is_handle = false` the relocation test fails at the
  dispatch, with `set_handle` a no-op it fails at the address, and the intern
  tests fail if the id is per-compile again.
- **D16 — Is `e[i]` an array read, or a positional read of any container? —
  DECIDED: arrays only; the spec is corrected.** The spec and the checker state
  two different languages.

  *Arrays only* is what `check_index` implements and documents
  (`checker/indexing.rs:19-28`, `:39-52`): the container's type is pinned to a
  fresh array type, so a tuple or a struct instance is refused with
  `DiagKind::Guard`, and the positional read of a **tuple** is the dedicated
  `a(k)` (a struct instance reads by name, `s.x` — the tuple-only narrowing is
  the addendum to `P1-34`'s outcome) — the operator is chosen by syntax, never by
  a runtime kind dispatch. Every example agrees (`examples/index.lichen` reads
  `b(0)`).

  *Any container* is what `language-spec.md` §3 stated, twice and explicitly:
  `e[i]` reads the `i`-th element of "an array, tuple, or struct instance", and
  `s(1, 2)[0]` "is the first field".

  The two are not reconcilable, and they differ on what a written program means,
  not on how it is spelled: `(1, 2)[0]` is `expected array<Int, Int>, found
  <Int, Int>` today and would be `1` under the spec. **Chosen — the doc fix:**
  the code's intent is explicit, its doc argues *why* (syntax picks the
  operator, never a runtime kind dispatch — the same rule that keeps `e[i]`,
  `a(k)` and `t{k}` three different things), every example agrees, and a
  language that *should* index tuples is a feature rather than a
  reconciliation. `P1-34` corrected §3's two sentences; nothing in the checker,
  the parser or the examples moved, so both reproductions still answer with the
  guard.

  *Rejected — teaching `check_index` the tuple and struct kinds:* it is the
  larger of the two and it would have to answer a question the deleted sentence
  answered badly — what a *struct's* positional index means, given that the
  spec's answer was "its wrapped tuple's elements" while the struct read has its
  own resolution through the struct's name table (`s.x`). A user who wants
  `(1, 2)[0]` to be `1` files that as a new item, and it starts by answering the
  struct question.

  One adjacent fact belongs with the decision rather than the item: the raw form
  `X<e>` *looks* like it could have spelled the spec's meaning without touching
  `check_index`, and it cannot — over a runtime container it reads components of
  something that is not a tuple type value, which is refused at check time (the
  container's type must be the tuple kind; `P1-35`), and on the tree this was
  written on was a **reported** runtime error where it used to print `none`
  silently (`P1-35`). So the choice really was between the
  two sides above; there is no third spelling available today.

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
  `static mut`, no `thread_local!`, no lazy statics outside compute's own
  registries and the derived-module cache beside them.
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
