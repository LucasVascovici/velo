# Velo v3 — Collaboration & Sync: Implementation Plan

This is the design + work-breakdown for adding sharing/sync to Velo. Read it
before implementing. It assumes the v2.x "solid base" work is done
(content-addressed snapshots, object integrity, `fsck`, repo lock, atomic
writes, file-mode/symlink model).

**Difficulty legend:** 🟢 Easy · 🟡 Medium · 🟠 Hard · 🔴 Very hard
**Size legend:** S (hours) · M (a day) · L (a few days) · XL (a week+)

**Progress:**
- ✅ **Phase 1 (bundle) shipped** — `bundle create [ref]` / `bundle apply`, versioned
  self-contained pack format, reachability walk, verified idempotent import.
- ✅ **Phase 2 (filesystem remotes) shipped** — `remote add/list/remove`, `clone`,
  `fetch` (remote-tracking `remotes/<remote>/<branch>` + `remote_refs`), `push`
  (fast-forward-only), `pull` (ff or advise `merge <remote>/<branch>`). Merge/save
  now resolve remote refs (`origin/main`). Reuses the Phase-1 pack machinery.
  Covered by a full two-repo collaboration-loop CLI test + a fetch test, `fsck`
  as oracle on both sides.
- ✅ **Phase 3 (network transport / ssh) shipped** — a `Remote` trait with
  `LocalRemote` (direct path) and `StreamRemote` (subprocess pack protocol);
  `serve-upload`/`serve-receive` server side; `ssh://[user@]host[:port]/path`
  URLs (auth delegated to ssh) and a `child:` scheme for local streaming/tests.
  Sync verbs are now transport-agnostic. Covered by ssh-URL parsing unit tests
  and a full streaming clone/push/pull/diverge CLI test.
- ✅ **Phase 4 (optimisation & UX) shipped** — minimal-transfer negotiation
  (`build_pack_excluding`: send only commits *and objects* the peer lacks; push
  previously re-sent all history every time), ancestry-aware fast-forward check
  that works with minimal packs, ahead/behind/diverged in `velo status` (offline,
  from `remote_refs`), and `fsck` awareness of remote-tracking refs (+ `--repair`).
  Bundles remain self-contained (regression-tested).
- ⏭️ **Optional next:** HTTP transport (deferred; needs a security review);
  remote refs in `history --graph`.

**v3 sync is feature-complete: bundle → filesystem remotes → ssh → optimised.**

---

## 1. Goal & scope

Let more than one person (or one person on more than one machine) work on the
same Velo project and exchange history — the single biggest gap between Velo and
Git today.

We deliberately phase this so each step ships something useful and de-risks the
next:

1. **Phase 1 — Bundle (offline):** pack history into a file, apply it elsewhere.
   No network, no protocol, no auth. Exercises every core sync primitive.
2. **Phase 2 — Filesystem remotes:** `clone` / `fetch` / `push` / `pull` against
   a repo on a local path or shared/network drive. Adds remotes + ref
   reconciliation, still no network protocol.
3. **Phase 3 — Network transport:** ssh (and/or http) so remotes can be on
   another host. Adds transport + auth.
4. **Phase 4 — Optimisation & UX:** minimal-transfer negotiation,
   remote-tracking branch UX, `velo status` ahead/behind.

You get real value at the end of Phase 1, and Phases 1–2 have **no networking**,
which is where most of the risk and security surface lives.

---

## 2. Why the foundation is ready (and what it buys us)

The v2.x work was chosen precisely to make sync tractable. Sync rests on three
invariants we now have:

| Invariant | Status | Why sync needs it |
| :--- | :--- | :--- |
| **Snapshot id = hash of content** (tree+parents+message+time), branch-independent | ✅ done | Two machines can agree "we have the same commit" by id; receiving a commit lets you verify it. |
| **Objects content-addressed + verifiable** (`fsck` re-hashes) | ✅ done | Transfer/import can dedup by name and detect corruption on arrival. |
| **Repo lock + atomic writes** | ✅ done | A `pull`/import mutating the store concurrently with another process can't corrupt it. |
| **Merge/rebase engine, content-addressed** | ✅ done | Divergent histories on `pull` reconcile via the *existing* engine — no new merge logic. |

The practical upshot: **most of the hard algorithmic work is already done.** v3 is
mostly *plumbing* — enumerate, serialize, transfer, import, verify, reconcile —
built on primitives that already exist.

---

## 3. Design decisions to make first

These shape everything downstream. Recommendations given; confirm before Phase 1.

1. **Topology — centralized-ish vs peer.**
   *Recommendation:* Git-style named remotes, starting with a single `origin`.
   Simple mental model; peer-to-peer is just "multiple remotes" later.

2. **Wire/bundle format.**
   Options: (a) custom binary, (b) JSON manifest + raw object files in a
   tarball/zip, (c) ship a stripped SQLite file + objects.
   *Recommendation:* **(b)** — a versioned manifest (`bincode` or JSON) listing
   snapshot rows + file_map + tags, plus the referenced objects verbatim (already
   compressed). Human-inspectable, trivial to version, reuses the object files
   as-is. Add a `format_version` header from day one.

3. **Divergence handling on pull.**
   *Recommendation:* **fetch + explicit reconcile**, like Git. `pull` = `fetch`
   then, if diverged, tell the user to `velo merge origin/<branch>` or
   `velo rebase origin/<branch>`. Never silently auto-merge. Reuses the existing
   engine and keeps surprises out.

4. **How much Git model to adopt.**
   *Recommendation:* Minimal. Track per-remote branch tips ("remote refs"); skip
   refspecs/tag-following config until it's needed.

5. **Transfer completeness (Phase 1).**
   *Recommendation:* Start dumb — bundle/push sends *all reachable* objects the
   other side doesn't obviously have. Optimise negotiation in Phase 4.

6. **Identity edge cases to accept (document, don't fix):**
   - 64-bit snapshot ids → birthday collisions ~4B commits. Fine for now; revisit
     only if a real deployment approaches it.
   - Same logical change made independently on two machines = two different
     commit ids (timestamps differ). Objects still dedup perfectly. This is
     Git-like and acceptable.

---

## 4. New components to build

| Component | Phase | Difficulty | Size | Notes |
| :--- | :--- | :--- | :--- | :--- |
| `pack.rs` — (de)serialize snapshots+file_map+tags+objects to the versioned format | 1 | 🟡 | M | Core of everything; get the format + versioning right. |
| `commands/bundle.rs` — `bundle create` / `bundle apply` | 1 | 🟡 | M | Thin layer over pack + import. |
| Reachability: "objects & snapshots reachable from a ref/range" in `commands/mod.rs` | 1 | 🟡 | S–M | Ancestry walk (exists in merge/rebase) + collect file_map hashes (like `gc`). |
| Import routine — insert received snapshots/objects, dedup, verify | 1 | 🟠 | M | Transactional; must be idempotent and integrity-check. Reuse `fsck` logic. |
| `db.rs` — `remotes` + `remote_refs` tables (+ migration) | 2 | 🟢 | S | Schema is already migration-friendly. |
| `commands/remote.rs` — add/list/remove remotes | 2 | 🟢 | S | Pure CRUD. |
| `transport.rs` — trait abstracting where a remote lives | 2 | 🟡 | M | Filesystem impl first; keep the trait small. |
| `commands/clone.rs` — init + import + set up tracking + checkout | 2 | 🟡 | M | Mostly composition of existing pieces. |
| `commands/fetch.rs` — pull remote objects+refs into `remote_refs` (no working-tree change) | 2 | 🟠 | M | Negotiation lives here (dumb first). |
| `commands/push.rs` — send local objects+refs; reject non-fast-forward | 2 | 🟠 | M | Safety (don't clobber) is the tricky part. |
| `commands/pull.rs` — fetch + fast-forward or advise merge | 2 | 🟡 | S–M | Thin; leans on fetch + existing merge. |
| SSH/HTTP transport impls | 3 | 🔴 | L–XL | Networking + auth; defer auth to ssh. |
| Negotiation optimisation (have/want sets) | 4 | 🟠 | M | Only after correctness. |

---

## 5. Impact on existing modules

How much each current module has to change. Most are **reuse**, which is the
payoff of the v2.x work.

| Module | Change | Difficulty | Notes |
| :--- | :--- | :--- | :--- |
| `commands/mod.rs` (`snapshot_id`, identity) | **None** | 🟢 | The hard prerequisite — already content-addressed & branch-independent. |
| `storage.rs` (object store) | Add `has_object`, bulk export/import helpers | 🟢 | `read_object`/`store_raw`/`write_atomic` already do the heavy lifting. |
| `db.rs` | Add `remotes`, `remote_refs` tables + migration | 🟢 | Follows the existing idempotent-migration pattern. |
| `commands/fsck.rs` | Extract a reusable "verify these snapshots/objects" fn for import | 🟡 | Logic exists; needs to be callable on a subset. |
| Merge / rebase / resolve | **Reuse as-is** for pull reconciliation | 🟢 | Divergent histories merge via the existing engine. Big win. |
| `lock.rs` | Classify `push`/`pull`/`fetch`/`clone`/`bundle apply` as mutating | 🟢 | One-line additions to the read-only matcher. |
| `main.rs` | New subcommands + dispatch (`bundle`, `remote`, `clone`, `fetch`, `push`, `pull`) | 🟡 | Mechanical clap plumbing; the bulk is line count, not difficulty. |
| Refs (`HEAD`/`PARENT` handling) | Add remote-tracking refs (`remote_refs` table, not files) | 🟡 | Keep local `HEAD`/`PARENT` semantics unchanged; remotes are separate. |
| `commands/history.rs` (`--graph`) | Optionally show remote refs / ahead-behind | 🟡 | Nice-to-have, Phase 4. |
| `commands/status.rs` | "ahead N / behind M of origin/<branch>" | 🟡 | Phase 4 UX. |
| Tests + `workflow_sim.sh` | New multi-repo scenarios | 🟠 | Biggest test surface: two-repo clone/push/pull/diverge. |

**Nothing in the core storage/identity layer needs to change.** That's the whole
point of having done it first.

---

## 6. Phase-by-phase work breakdown

### Phase 1 — Bundle (offline transfer) 🟡 · L
The MVP of sync. No network, no remotes.

1. **`pack.rs`: define the format** (🟡 M). Versioned header; sections for
   snapshots, file_map, tags, and objects. Encode/decode + a format-version guard.
2. **Reachability helper** (🟡 S–M). `reachable(root, from_refs, [stop_at])` →
   the set of snapshots + object hashes. Reuse the ancestry CTE from `merge.rs`
   and the `file_map` collection from `gc.rs`.
3. **`bundle create <file> [range]`** (🟡 M). Walk reachability → pack → write file.
4. **Import routine** (🟠 M). Given a pack: begin a transaction, insert unknown
   snapshots/file_map/tags, write unknown objects (atomic, dedup by name),
   **verify with fsck logic**, commit. Idempotent (re-applying is a no-op).
5. **`bundle apply <file>`** (🟡 S). Read pack → import → report what was added.
   Leave refs/branches to the user (or fast-forward local branches that are
   strict ancestors of bundled tips).
6. **Tests** (🟡 M): pack roundtrip (property test — arbitrary history in→out),
   `bundle create` in repo A → `bundle apply` in repo B → histories match →
   `fsck` clean in B. Add to `tests/cli.rs`.

*Exit criteria:* two independent repos can exchange full history via a file, and
`fsck` passes on the receiver.

### Phase 2 — Filesystem remotes 🟠 · L–XL
Add the remote concept and the fetch/push/pull/clone verbs, transport = a path.

1. **Schema** (🟢 S): `remotes(name, url)`, `remote_refs(remote, branch, hash)`.
2. **`transport.rs` trait** (🟡 M): `list_refs`, `read_objects(hashes)`,
   `write_objects`, `read_pack`/`write_pack`. Filesystem impl (a remote is a
   directory containing a `.velo/`).
3. **`velo remote add/list/remove`** (🟢 S).
4. **`velo clone <path> [dir]`** (🟡 M): init + import everything + record the
   remote + set `remote_refs` + checkout the default branch.
5. **`velo fetch [remote]`** (🟠 M): compare local vs remote tips, pull missing
   snapshots+objects into the local store, update `remote_refs`. **Does not touch
   the working tree.** Dumb negotiation (send-all-reachable) to start.
6. **`velo push [remote] [branch]`** (🟠 M): compute what the remote lacks, write
   it, advance the remote branch ref — **but reject non-fast-forward** (remote
   moved and you'd clobber it). This safety check is the crux.
7. **`velo pull`** (🟡 S–M): `fetch`, then fast-forward the local branch if the
   remote is ahead; if diverged, stop and tell the user to
   `velo merge origin/<branch>` (reuses the engine). Guard against clobbering
   uncommitted work (existing dirty-tree pattern).
8. **Tests** (🟠 L): clone, push, pull, fast-forward, and a genuine divergence
   that reconciles via merge — all across two temp repos.

*Exit criteria:* full clone/push/pull loop against a shared-drive path, with
non-fast-forward pushes rejected and divergence handled via merge.

### Phase 3 — Network transport 🔴 · L–XL
1. **SSH transport** (🟠 L): shell out to `ssh` running `velo` in remote mode
   (like Git's `git-upload-pack`/`receive-pack`). Auth is delegated to ssh.
2. **HTTP transport** (🔴 XL, optional): a small server mode; auth tokens. Larger
   surface; only if there's demand.

*Security note:* keep auth **out** of Velo by leaning on ssh. Any HTTP mode needs
a real security review before shipping.

### Phase 4 — Optimisation & UX 🟠 · M–L
- Minimal-transfer negotiation (have/want) instead of send-all.
- `origin/<branch>` in `history --graph`; ahead/behind in `status`.
- `fsck` awareness of remote refs.

---

## 7. Risks & mitigations

| Risk | Likelihood | Mitigation |
| :--- | :--- | :--- |
| **Divergence UX is confusing** ("your branch and origin diverged") | High | Copy Git's model exactly: fetch is safe/read-only, reconcile is explicit. Clear messages. |
| **Non-fast-forward push clobbers remote work** | High if unguarded | Hard-reject in `push`; require the user to pull+merge first. Test it explicitly. |
| **Pull clobbers uncommitted local work** | Medium | Reuse the existing dirty-tree guard; refuse pull with unsaved changes. |
| **Format drift breaks old bundles** | Medium | `format_version` header from day one; refuse unknown versions with a clear message. |
| **Partial/interrupted import corrupts the repo** | Medium | Import in one transaction + objects via atomic writes; verify (fsck) before commit; repo lock prevents concurrent mutation. |
| **Cross-platform paths/modes in transferred trees** | Low–Medium | File model already normalises paths (forward slashes) and modes; symlink/exec degrade gracefully (documented). |
| **Network transport security** | High (Phase 3 only) | Delegate to ssh; gate any HTTP mode behind a security review. |

---

## 8. Testing strategy

- **Pack roundtrip property test** — arbitrary history → pack → unpack → identical
  snapshots/objects (proptest).
- **Two-repo integration tests** (`tests/cli.rs`) — clone/push/pull/fetch,
  fast-forward, divergence-via-merge, non-fast-forward rejection, idempotent
  re-apply.
- **`fsck` as the oracle** — every sync test ends by asserting `velo fsck` passes
  on the receiver.
- **Extend `workflow_sim.sh`** — a "two developers on two clones" act:
  Alice pushes, Bob pulls, both edit, reconcile, both `fsck` clean.
- **Interruption/crash tests** — kill an import mid-way (or simulate), assert the
  repo is still consistent (transaction + lock should guarantee it).

---

## 9. Recommended order & rough sizing

1. **Confirm §3 design decisions.** (discussion)
2. **Phase 1: bundle.** 🟡 L — highest value-to-risk; no networking.
3. **Phase 2: filesystem remotes.** 🟠 L–XL — the real clone/push/pull loop.
4. **Phase 3: ssh transport.** 🔴 L–XL — only after 1–2 are rock-solid.
5. **Phase 4: negotiation + UX.** 🟠 M–L.

The whole of Phases 1–2 involves **no new cryptography, no network code, and no
changes to the identity/storage core** — it's serialize / transfer / import /
reconcile on top of primitives that already exist and are tested. That is by
design, and it's why doing the base first was worth it.

---

## 10. Open questions for you

1. Topology: single `origin` to start, or design for multiple remotes now?
2. Bundle format: JSON manifest (inspectable) or binary (`bincode`, compact)?
3. Do you want `clone`/`push`/`pull` to a **shared drive / path** to be the v3
   headline (Phase 2), with ssh explicitly deferred?
4. Should `pull` ever auto-merge on divergence, or always require an explicit
   `merge`/`rebase` (recommended)?
5. Any appetite for an HTTP transport, or is ssh + filesystem enough?
