# Integrity, signing and leaving Velody

### Integrity & safety

```bash
velody fsck            # verify everything (read-only)
velody fsck --repair   # also tidy safely-fixable cruft
```

```
Checking repository integrity…
  ✔ Objects: 4 referenced, 4 verified
  ✔ Snapshots: 3 checked, 3 ids verified
  ✔ Refs: PARENT, tags, stash
  ✔ State: no cruft

✔ Repository is healthy.
```

What velody defends against and what it does not is in
[docs/THREAT_MODEL.md](THREAT_MODEL.md); to report a vulnerability see
[SECURITY.md](../SECURITY.md).

`fsck` checks that every referenced object exists **and re-hashes to its own
name**, that every snapshot's content-addressed ID recomputes correctly, that
parents and merge parents resolve, and that all refs (`PARENT`, tags, stash,
remote-tracking) point at something real. It exits non-zero when it finds
corruption, so it works in scripts and CI. Cruft (orphaned conflict rows, stale
tracking refs) is reported as a warning and cleaned up by `--repair`.

Underneath, a few things protect your data:

- **Atomic writes.** Objects and refs (`PARENT`, `HEAD`, `MERGE_HEAD`) are written
  to a temp file and renamed into place, so a crash mid-write can never leave a
  truncated ref or a half-written object.
- **Crash-tested.** Every operation that writes more than one file is killed at
  each durable step in the test suite, and what each kill leaves (including the
  few known windows and what a power loss can do) is in
  [docs/CRASH_SAFETY.md](CRASH_SAFETY.md).
- **Repository lock.** Mutating commands take an advisory lock on `.velo/lock`, so
  two concurrent `velody` processes can't race (a `gc` can't delete an object a
  `save` is still committing). Read-only commands never block.
- **Verified imports.** Anything received from a bundle or a remote is fully
  verified — objects re-hashed, snapshot IDs recomputed — inside one transaction
  before it is trusted.

#### Signing snapshots

```bash
velody keygen --out ~/.velo-key        # prints the public key; the file is a secret
velody sign --key ~/.velo-key          # sign the snapshot you are on
velody verify --trust trusted.keys     # exit 0 only if a trusted key signed it
```

A signature covers the snapshot's id, and the id covers the whole tree, the
parents, the message, the time, the metadata and the author, so a valid
signature vouches for all of them. `velody history` and `velody show` mark signed
snapshots with `[signed]`. Trust is your own list: the trust file holds one
public key per line (blank lines and `#` comments are allowed), and velody checks
signatures against it. Velody does not store private keys, manage or revoke keys,
or build a web of trust.

---

### Leaving Velody

You can take your history with you. `velody export-git` writes every branch, tag,
file mode and symlink as a `git fast-import` stream, with no git needed on the
velody side:

```bash
velody export-git | (mkdir ../out && cd ../out && git init -q && git fast-import)
velody export-git --branch main --output history.fi   # one branch, to a file
```

Merge commits keep both parents. What git has no place for -- snapshot metadata,
recorded renames, the branch a snapshot was made on and its exact millisecond
timestamp -- goes into `Velo-*` trailers at the end of each commit message. Snapshot
ids are preserved in the `Velo-Snapshot` trailer, so the history can be brought back
with identical ids. Stashes, deleted-branch shelves and remote-tracking branches are
not exported.

#### ...and coming back

`velody import-git` reads a `git fast-export` stream, from stdin or `--input FILE`:

```bash
git fast-export --all --reencode=yes --signed-tags=strip --tag-of-filtered-object=drop -M | velody import-git
```

A round trip through git preserves ids. Export, let git hold the history, export it
from git and import into a fresh repository, and every snapshot comes back with the
id it had, with its metadata, author, rename edges, branch and millisecond timestamp
restored from the `Velo-*` trailers:

```bash
velody export-git | (mkdir ../git && cd ../git && git init -q && git fast-import)
(cd ../git && git fast-export --all --reencode=yes --signed-tags=strip -M) | (mkdir ../back && cd ../back && velody init && velody import-git)
```

A plain git repository imports too, with its authors, committer timestamps (whole
seconds), branches, tags and merges; a rename git detects with `-M` becomes a rename
edge. Annotated tags become lightweight tags (velody tags carry no message), submodules
are skipped and counted, and an octopus merge is refused because a snapshot has at
most two parents. Importing does not touch the working tree.
