<p align="center">
  <img src="https://img.shields.io/github/v/release/LucasVascovici/velo?color=orange&logo=github&label=latest" />
  <img src="https://github.com/LucasVascovici/velo/actions/workflows/ci.yml/badge.svg" />
  <img src="https://github.com/LucasVascovici/velo/actions/workflows/release.yml/badge.svg" />
  <img src="https://img.shields.io/github/license/LucasVascovici/velo?color=blue" />
</p>

<h1 align="center">⚡ Velo</h1>

<p align="center">
  <strong>A fast, safe, and intuitive version control system built in Rust.</strong><br/>
  Git's power — without Git's sharp edges.
</p>

<p align="center">
  <em>v3 — now with collaboration: clone, push, pull, and offline bundles.</em>
</p>

> **Note:** Velo was vibe-coded for fun — high-level intent, modern tech stack, tight feedback loop with an AI assistant. It's a real working tool, but built as an experiment in what's possible with that workflow, not as a production-grade Git replacement.

---

Git is a masterpiece of engineering — but its interface was designed in 2005 for the Linux kernel, not for everyday developers in 2026. Velo keeps what works (content-addressed snapshots, cheap branching, cryptographic hashing, delta compression) and replaces what repeatedly trips people up.

---

## Why Velo

| Pain point | Git | Velo |
| :--- | :--- | :--- |
| **Staging area** | `git add` every file before every commit | Nothing. Disk = snapshot. |
| **Losing work** | Easy to `checkout` or `reset --hard` the wrong thing | Guards block destructive ops when you have unsaved changes |
| **Merge conflicts** | `<<<<<<<` markers break your code during resolution | Interactive hunk-by-hunk TUI — your files stay valid throughout |
| **Undo a commit** | `git reset --soft HEAD~1`, `git reset --hard`, `git reflog`... | `velo undo` — one command, reversible |
| **Redo an undo** | Not built-in — requires `git reflog` | `velo redo` |
| **Abort a merge** | `git merge --abort` (resets working tree) | `velo merge --abort` (restores to exact pre-merge state, even after all conflicts resolved) |
| **Named stashes** | `git stash push -m "name"`, recalled by index | `velo stash push <name>`, recalled by name |
| **Branch history** | `git log --all --graph --oneline --decorate` | `velo history --all --graph` |
| **View a snapshot** | `git show <hash>` | `velo show <hash>` |
| **Apply one commit** | `git cherry-pick <hash>` | `velo cherry-pick <hash>` |
| **Accidental force-push** | `git push --force` overwrites remote history | `velo push` is fast-forward-only — divergence is refused with instructions |
| **Surprise rebases on pull** | `git pull` may merge or rebase depending on config | `velo pull` fast-forwards or stops and tells you — never silently rewrites |
| **Verify the repo** | `git fsck` (cryptic output) | `velo fsck` — plain-English report, `--repair` for fixable cruft |

---

## Velo vs Git — workflow comparison

### Daily workflow

| Task | Git | Velo |
| :--- | :--- | :--- |
| Start tracking a folder | `git init` | `velo init` |
| Save your work | `git add -A && git commit -m "msg"` | `velo save "msg"` |
| See what changed | `git status` | `velo status` |
| See line-level diff | `git diff` | `velo diff` |
| View history | `git log` | `velo history` |
| View one commit | `git show <hash>` | `velo show <hash>` |
| Time-travel to a past state | `git checkout <hash>` | `velo restore <hash>` |
| Undo the last commit | `git reset --soft HEAD~1` | `velo undo` |
| Redo an undone commit | `git reflog` + `git reset` | `velo redo` |
| Annotate file with blame | `git blame <file>` | `velo blame <file>` |
| Search tracked files | `git grep <pattern>` | `velo grep <pattern>` |
| Squash last N commits | interactive rebase | `velo squash <n> "msg"` |
| Diff two commits | `git diff <a> <b>` | `velo diff-range <a>..<b>` |
| Rebase branch | `git rebase <target>` | `velo rebase <target>` |
| Fix the last commit message | `git commit --amend` | `velo save "new msg" --amend` |

### Branches

| Task | Git | Velo |
| :--- | :--- | :--- |
| Create and switch branch | `git switch -c <name>` | `velo switch <name>` |
| Switch to existing branch | `git switch <name>` | `velo switch <name>` |
| List branches | `git branch` | `velo branches` |
| Delete a branch | `git branch -d <name>` | `velo branches --delete <name>` |
| Merge a branch | `git merge <branch>` | `velo merge <branch>` |
| Abort a merge | `git merge --abort` | `velo merge --abort` |
| Apply one commit | `git cherry-pick <hash>` | `velo cherry-pick <hash>` |
| Rebase branch | `git rebase <target>` | `velo rebase <target>` |
| Squash commits | `git rebase -i HEAD~N` | `velo squash <n> "msg"` |

### Conflict resolution

| Task | Git | Velo |
| :--- | :--- | :--- |
| See conflicts | `<<<<<<<` markers in file | Interactive TUI — `velo resolve <file>` |
| Take our version | `git checkout --ours <file>` | `velo resolve <file> --take ours` |
| Take their version | `git checkout --theirs <file>` | `velo resolve <file> --take theirs` |
| Resolve all at once | — | `velo resolve --all --take theirs` |
| Code validity during merge | ❌ Markers break syntax | ✅ File untouched; TUI shows both sides |
| Abort after resolving | ❌ `--abort` fails if you started editing | ✅ `--abort` works until `velo save` |

### Stash

| Task | Git | Velo |
| :--- | :--- | :--- |
| Save dirty state | `git stash push -m "name"` | `velo stash push <name>` |
| List stashes | `git stash list` | `velo stash list` |
| Apply a stash | `git stash pop` or `git stash apply stash@{2}` | `velo stash pop <name>` |
| Drop a stash | `git stash drop stash@{2}` | `velo stash drop <name>` |
| Inspect a stash | `git stash show stash@{2} -p` | `velo stash show <name>` |

### Collaboration

| Task | Git | Velo |
| :--- | :--- | :--- |
| Copy a repository | `git clone <url>` | `velo clone <url>` |
| Add a remote | `git remote add origin <url>` | `velo remote add origin <url>` |
| List remotes | `git remote -v` | `velo remote` |
| Download without merging | `git fetch` | `velo fetch` |
| Publish your commits | `git push` | `velo push` (fast-forward only) |
| Integrate remote commits | `git pull` | `velo pull` |
| See if you're ahead/behind | `git status` (after fetch) | `velo status` (after fetch) |
| Share history without a server | `git bundle create` | `velo bundle create <file>` |
| Import a shared history file | `git bundle unbundle` | `velo bundle apply <file>` |

### Tags & maintenance

| Task | Git | Velo |
| :--- | :--- | :--- |
| Create a tag | `git tag v1.0` | `velo tag v1.0` |
| Tag a past commit | `git tag v1.0 <hash>` | `velo tag v1.0 <hash>` |
| List tags | `git tag` | `velo tag` |
| Delete a tag | `git tag -d v1.0` | `velo tag --delete v1.0` |
| Clean up old data | `git gc` | `velo gc` |
| Check repository integrity | `git fsck` | `velo fsck` / `velo fsck --repair` |

---

## Where Velo is intentionally different

**No staging area.** `git add` is a source of confusion and lost work for new and experienced users alike. Velo removes it entirely. Every save snapshots exactly what is on disk.

**Conflict resolution as a TUI, not markers.** When Velo detects a true conflict it stores both versions in the database and presents them hunk-by-hunk in an interactive navigator. Your file on disk is never modified until you confirm a resolution. Per-hunk: keep ours, take theirs, both in either order, or open `$EDITOR`. Sessions are resumable — progress is persisted to the database between runs.

**`merge --abort` always works.** Git's `--abort` fails if you have begun editing conflict files. Velo's `--abort` restores the working tree to its exact pre-merge state at any point — during conflicts, after resolving all conflicts, right up until `velo save` finalises the merge.

**Named stash shelves.** `git stash apply stash@{2}` requires you to remember an index in a list. `velo stash pop wip-auth` is self-documenting.

**Branch names resolve everywhere.** Any command that accepts a hash or tag (`show`, `cherry-pick`, `restore`) also accepts a branch name — it resolves to the branch tip automatically. Remote-tracking refs work too: `velo merge origin/main`, `velo show origin/main`.

**Sync that refuses to surprise you.** `velo push` is fast-forward-only: if the remote has commits you don't, the push is refused with the exact commands to fix it — there is no `--force` footgun. `velo pull` either fast-forwards or stops and tells you the branches diverged; it never silently merges or rewrites your history. Reconciliation is always an explicit `velo merge origin/<branch>` or `velo rebase origin/<branch>`.

**Verifiable snapshots.** A snapshot's ID is a BLAKE3 hash of its *content* — the full file tree (paths, object hashes, and modes) plus its parents, message, and timestamp. That means a snapshot can be checked against what it claims to contain, which is exactly what `velo fsck` does, and what makes importing history from another machine safe: every object is re-hashed and every snapshot ID recomputed on arrival before anything is trusted.

---

## Performance

Benchmarked on a monorepo with 571 files across 6 language modules, 40 incremental saves, and 8 concurrent branches.

| Command | Latency | How |
| :--- | :--- | :--- |
| `velo status` (warm) | ~35–60 ms | mtime+size index cache — no rehashing on unchanged files |
| `velo status` (cold) | ~50–200 ms | Parallel BLAKE3 across all CPU cores via Rayon |
| `velo save` (incremental) | ~285 ms avg | Parallel hashing + single SQLite transaction + Zstd |
| `velo restore` | ~200–800 ms | Parallel file writes; scales with number of changed files |
| `velo merge` | <100 ms | LCA found via recursive CTE; no file I/O needed |
| `velo history --all` | ~35 ms | Indexed ancestry walk in SQLite WAL mode |

The warm-cache path for `velo status` is essentially N × `stat()` — no file reads, no hashing. Only files whose `mtime` or `size` changed since the last run are rehashed.

---

## Installation

### Unix (Linux & macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh
```

By default the binary is installed to `/usr/local/bin` (with `sudo` if needed) or `~/.local/bin` if sudo is unavailable. Options:

```bash
# Install to a custom directory
curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh -s -- --dir ~/.local/bin

# Preview what the script would do without making any changes
curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh -s -- --dry-run
```

### Windows

Download the latest `velo-x86_64-windows.zip` from the [Releases page](https://github.com/LucasVascovici/velo/releases), extract `velo.exe`, and place it anywhere on your `PATH`.

### Pre-built binaries

| Platform | File |
| :--- | :--- |
| Linux x86-64 (musl, static) | `velo-x86_64-linux.tar.gz` |
| Linux ARM64 (musl, static) | `velo-aarch64-linux.tar.gz` |
| macOS Apple Silicon | `velo-aarch64-macos.tar.gz` |
| macOS Intel | `velo-x86_64-macos.tar.gz` |
| Windows x86-64 | `velo-x86_64-windows.zip` |

### Build from source

Requires Rust 1.75 or later.

```bash
git clone https://github.com/LucasVascovici/velo.git
cd velo
cargo build --release
# Binary is at: target/release/velo
```

---

## Quick start

```bash
# Initialise a repository
velo init

# Save a snapshot
echo "hello world" > app.py
velo save "Initial commit"

# See what changed
velo status
velo diff

# View history and time-travel
velo history
velo restore <hash>

# Work on a feature branch
velo switch feature/login
# ... edit files ...
velo save "Add login page"

# Merge back
velo switch main
velo merge feature/login
velo save "Merge feature/login"

# Share it
velo remote add origin /shared/app     # or ssh://user@host/srv/app
velo push
velo pull
```

---

## Command reference

### Core workflow

| Command | Description |
| :--- | :--- |
| `velo init` | Initialise a new repository in the current directory |
| `velo save "<message>"` | Snapshot all tracked files with a description |
| `velo save "<message>" -- <path>` | Snapshot only the listed paths; other changes remain unsaved |
| `velo save "<message>" --amend` | Replace the last snapshot (keeps same parent) |
| `velo status` | Show new, modified, and deleted files vs the last snapshot |
| `velo status -- <path>` | Restrict status output to specific paths |
| `velo diff [<file>]` | Show line-level diff against the last snapshot |
| `velo show <target>` | Inspect a past snapshot without restoring — hash, prefix, tag, or branch name |
| `velo show <target> -- <path>` | Restrict the diff to a specific file or directory |
| `velo blame <file>` | Annotate each line with the snapshot that last changed it |
| `velo blame <file> --at <target>` | Blame at a specific past snapshot, tag, or branch |
| `velo grep <pattern>` | Search tracked files for a regex pattern |
| `velo grep <pattern> --snapshot <target>` | Search inside a stored snapshot |
| `velo grep <pattern> -i` | Case-insensitive search |
| `velo grep <pattern> -l` | Print only file names with matches |
| `velo grep <pattern> -C <n>` | Show N lines of context around each match |

### History and time-travel

| Command | Description |
| :--- | :--- |
| `velo history` | Linear history of the current branch (last 20 by default) |
| `velo history --all` | History across all branches |
| `velo history --graph` | ASCII branch graph with coloured lanes |
| `velo history --graph --all` | Full topology graph across all branches |
| `velo history --branch <n>` | History for a specific branch without switching |
| `velo history --file <path>` | Only snapshots that touched this file or directory |
| `velo history --oneline` | Compact one-line-per-snapshot format |
| `velo history --limit <n>` | Limit the number of entries shown |
| `velo restore <target>` | Restore the working tree to a hash, prefix, tag, or branch name |
| `velo restore <target> --force` | Restore, discarding any unsaved changes |
| `velo restore <target> -- <path>` | Restore only specific files (PARENT is not updated) |
| `velo diff-range <a>..<b>` | Diff between two snapshots; hash prefixes, tags, and branch names accepted |
| `velo diff-range <a>..<b> -- <path>` | Restrict the diff to specific paths |
| `velo diff-range <a>` | Compare a snapshot against the current working tree |
| `velo squash <n> "<msg>"` | Collapse the last N snapshots into one with a new message |
| `velo undo` | Remove the most recent snapshot and rewind the working tree |
| `velo redo` | Re-apply the most recently undone snapshot |

### Branches

| Command | Description |
| :--- | :--- |
| `velo switch <name>` | Switch to a branch (creates it if it doesn't exist) |
| `velo switch <name> --force` | Switch, discarding any unsaved changes |
| `velo branches` | List all branches with their latest snapshot |
| `velo branches --delete <name>` | Soft-delete a branch (history preserved, purged by `velo gc`) |

### Merging and conflict resolution

| Command | Description |
| :--- | :--- |
| `velo merge <branch>` | 3-way merge `<branch>` into the current branch |
| `velo merge --abort` | Restore the exact pre-merge state (works at any point before `velo save`) |
| `velo resolve <file>` | Interactive hunk-by-hunk conflict resolver |
| `velo resolve <file> --take ours` | Non-interactive: keep the current branch's version |
| `velo resolve <file> --take theirs` | Non-interactive: take the incoming branch's version |
| `velo resolve --all --take <ours\|theirs>` | Resolve all outstanding conflicts non-interactively |
| `velo cherry-pick <target>` | Apply the diff from one snapshot onto the current branch |
| `velo rebase <target>` | Replay current branch commits on top of another branch |
| `velo rebase --abort` | Abort the rebase and restore the original branch state |
| `velo rebase --continue` | Continue after resolving a rebase conflict |

### Stash

| Command | Description |
| :--- | :--- |
| `velo stash push <name>` | Shelve dirty working-tree state under a name |
| `velo stash list` | List all stash shelves |
| `velo stash pop <name>` | Restore a shelf and delete it |
| `velo stash drop <name>` | Delete a shelf without restoring |
| `velo stash show <name>` | Inspect a shelf's contents |

### Tags

| Command | Description |
| :--- | :--- |
| `velo tag <name>` | Tag the current snapshot |
| `velo tag <name> <target>` | Tag a specific snapshot by hash, prefix, or branch name |
| `velo tag <name> --force` | Overwrite an existing tag |
| `velo tag` | List all tags |
| `velo tag --delete <name>` | Delete a tag |

### Collaboration & sync

| Command | Description |
| :--- | :--- |
| `velo clone <url> [dir]` | Copy a repository, set up `origin`, and check out its default branch |
| `velo remote add <name> <url>` | Add a remote (a filesystem path or `ssh://[user@]host[:port]/path`) |
| `velo remote` | List configured remotes |
| `velo remote remove <name>` | Remove a remote and its tracking refs |
| `velo fetch [remote]` | Download remote history into `remotes/<remote>/*` — never touches your branches or working tree |
| `velo push [remote] [branch]` | Publish a branch (fast-forward only; refuses to overwrite remote work) |
| `velo pull [remote]` | Fetch the current branch, then fast-forward — or report divergence and stop |

Remote defaults to `origin`, and branch defaults to the current branch.
After a `fetch`, `velo status` shows ahead/behind, and `origin/<branch>`
can be used anywhere a ref is accepted (`merge`, `rebase`, `show`, `diff-range`).

### Offline transfer

| Command | Description |
| :--- | :--- |
| `velo bundle create <file>` | Pack the entire repository into one self-contained file |
| `velo bundle create <file> <ref>` | Pack only the history reachable from a snapshot, tag, or branch |
| `velo bundle apply <file>` | Import a bundle into this repository (verified, idempotent) |

### Maintenance

| Command | Description |
| :--- | :--- |
| `velo gc` | Remove orphaned objects and stale undo/conflict state |
| `velo gc --keep-days <n>` | Retain undo history for `n` days before purging (default: 30) |
| `velo fsck` | Verify repository integrity; exits non-zero if anything is wrong |
| `velo fsck --repair` | Also clean up safely-fixable cruft (orphaned rows, stale tracking refs) |

---

## Merge workflow example

```bash
# Start a feature branch
velo switch feature/payments
echo "stripe_key = 'live_...'" > config.py
velo save "Add payment config"

# Back on main, make a conflicting change
velo switch main
echo "stripe_key = 'test_...'" > config.py
velo save "Set test payment key"

# Merge — Velo finds the common ancestor automatically
velo merge feature/payments
# → Conflict: config.py

# Resolve interactively — hunk-by-hunk TUI, your file stays valid throughout
velo resolve config.py
# [1] Keep ours  [2] Take theirs  [3] Both  [e] Edit  [q] Quit

# Or resolve non-interactively
velo resolve config.py --take theirs

# Changed your mind? Abort at any point before saving
velo merge --abort   # ← restores exact pre-merge state

# Finalise
velo save "Merge feature/payments"
```

The `--graph` flag shows the merge in history:

```
●  a709f1062fbec2f1  (main)  2026-07-25 13:51:25  Merge feature/payments
│ ╲
│ │
○ │  3ca49fa15b5d1461  (main)  2026-07-25 13:51:25  Set test payment key
│ │
│ ○  7ba17e5444285026  (feature/payments)  2026-07-25 13:51:24  Add payment config
│ ╱
│
○  b663407a874fb830  (main)  2026-07-25 13:51:24  Initial commit
```

### Conflicts only when the changes really overlap

Velo performs a real line-level 3-way merge. If both branches touched the same
file but in different places, both sets of changes are combined automatically —
you are only asked to resolve regions that genuinely overlap:

```bash
# ancestor:  DEBUG = False  …  RETRIES = 3
# feature/payments changed line 3;  main changed line 5
velo merge feature/payments
```
```
Merging 'feature/payments' into 'main' (ancestor: 38735ed3e87fca9f)…
  ~ Auto-merged: config.py

Merge summary
  New:      0
  Updated:  1
  Deleted:  0
  Conflicts: 0

✔ Clean merge! Run velo save "Merge <branch>" to finalise.
```

The merged file keeps **both** sides — `DEBUG = True` from the feature branch and
`RETRIES = 5` from main. The same engine backs `cherry-pick` and `rebase`.

---

## Collaboration

Velo repositories can be shared over a filesystem path (including a network or
shared drive), over SSH, or as a single self-contained file — no server to run.

### Clone, push, pull

```bash
# Clone from a path or over SSH
velo clone /shared/project
velo clone ssh://user@host/srv/project        # ssh://[user@]host[:port]/path

# Everyday loop
velo save "Add login page"
velo push                  # fast-forward only
velo pull                  # fast-forward, or tells you it diverged

# Remotes
velo remote add origin /shared/project
velo remote                             # list
velo remote remove origin

# Download without touching your branches or working tree
velo fetch
```

`velo status` tells you where you stand relative to the last-fetched remote state
(no network access needed):

```
Branch: main  Position: 45877e2a3b0b3fa1  "Add login page"
  ↑ 1 ahead of origin/main — velo push to publish
```
```
  ↓ 1 behind origin/main — velo pull to catch up
  ↕ diverged from origin/main (1 ahead, 1 behind) — velo pull then velo merge origin/main
  ✔ up to date with origin/main
```

### When two people diverge

```bash
velo push
# → Push rejected — 'main' has commits you don't have (non-fast-forward).
#   Run 'velo pull origin' and reconcile, then push again.

velo pull
# → ! 'main' and 'origin/main' have diverged.
#     Reconcile with velo merge origin/main then velo save "Merge …"

velo merge origin/main     # the normal 3-way merge — auto-merges what it can
velo save "Merge origin/main"
velo push                  # now a fast-forward
```

Nothing is ever force-overwritten, and `pull` never rewrites your history behind
your back.

### Offline transfer with bundles

A bundle is one self-contained file carrying snapshots, all the objects they
reference, and their tags. Useful for air-gapped machines, backups, or emailing a
branch to someone.

```bash
velo bundle create backup.velo              # whole repository
velo bundle create feature.velo feature     # everything reachable from a ref
velo bundle apply backup.velo               # import into another repository
```

Applying a bundle is **idempotent** — re-applying imports nothing and reports
that you're already up to date.

### Only what's missing goes over the wire

Both `push` and `fetch` negotiate: the peer's known snapshots are subtracted, and
objects the peer already holds are skipped. A one-line change in a 20-file project
transfers a single object, not the whole tree — a snapshot references its entire
file tree, so naive syncing would resend everything on every push. (`bundle create`
deliberately opts out of this, since a bundle must stand alone.)

---

## Integrity & safety

```bash
velo fsck            # verify everything (read-only)
velo fsck --repair   # also tidy safely-fixable cruft
```

```
Checking repository integrity…
  ✔ Objects: 4 referenced, 4 verified
  ✔ Snapshots: 3 checked, 3 ids verified
  ✔ Refs: PARENT, tags, stash
  ✔ State: no cruft

✔ Repository is healthy.
```

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
- **Repository lock.** Mutating commands take an advisory lock on `.velo/lock`, so
  two concurrent `velo` processes can't race (a `gc` can't delete an object a
  `save` is still committing). Read-only commands never block.
- **Verified imports.** Anything received from a bundle or a remote is fully
  verified — objects re-hashed, snapshot IDs recomputed — inside one transaction
  before it is trusted.

---

## Architecture

| Layer | Technology | Role |
| :--- | :--- | :--- |
| Hashing | BLAKE3 | Collision-proof, 10× faster than SHA-1; `rayon` parallelises large files |
| Compression | Zstd level 1 | Fast compression on save; transparent decompression on restore |
| Metadata | SQLite (WAL mode) | Snapshots, branches, tags, ancestry, conflicts, stash, remotes — indexed queries |
| Mtime cache | `index_cache` table | `(path, mtime_ns, size, hash)` — skips rehashing unchanged files |
| Concurrency | Rayon | Parallel filesystem walk, hash-and-compress, and file writes on restore |
| I/O | memmap2 | Memory-maps files ≥256 KB to avoid kernel→userspace copy |
| Locking | fs2 advisory lock | Serialises mutating commands across processes (`.velo/lock`) |
| Transport | Filesystem · SSH · bundle | Direct path access, a pack protocol over `ssh`, or a self-contained file |

**Content-addressed snapshots.** A snapshot's ID is `BLAKE3(tree ‖ parents ‖ message ‖ timestamp)`, truncated to 16 hex characters (64 bits), where *tree* is every `(path, object-hash, mode)` triple in sorted order. Because the ID commits to the tree, it can be verified against its contents — the property `fsck` checks and that makes accepting history from another machine safe. The branch is deliberately **not** hashed, so renaming or deleting a branch never changes the identity of its commits.

**File model.** Each tree entry records a mode: regular, executable, or symlink. The executable bit survives save→restore (on Unix), and symlinks are stored as their target and recreated as real links rather than being followed and flattened into copies. On Windows the executable bit isn't observable from the filesystem, so it is carried through history unchanged, and symlink creation falls back to a regular file when the platform disallows it.

**Delta storage.** Each snapshot records only changed files. Unchanged files are stored as references to the same object from the parent. A 1000-file project where 10 files change creates 10 new objects, not 1000.

**Object store.** Content-addressed storage under `.velo/objects/`. Each object is Zstd-compressed file content named by its BLAKE3 hash. Identical content across branches and snapshots is stored exactly once.

**Merge parents.** Merge commits record both their primary parent and their merge-source parent (`merge_parent` column in the `snapshots` table). This is what enables the two-parent topology in `velo history --graph`.

**Schema migrations.** The database schema is versioned via `pragma_table_info` checks. New columns are added automatically on first use — existing repositories are upgraded in place with no manual intervention.

---

## Repository layout

```
.velo/
├── velo.db       # SQLite: snapshots, file trees, branches, tags, stash,
│                 #         conflicts, remotes, remote-tracking refs
├── objects/      # Content-addressed object store (Zstd-compressed, named by BLAKE3 hash)
├── HEAD          # Current branch name
├── PARENT        # Hash of the current snapshot
├── lock          # Advisory lock held by mutating commands
├── MERGE_HEAD    # Present only during an in-progress merge or cherry-pick
│                 # Format: "<pre-merge-hash>:<source-branch>"
└── REBASE_STATE  # Present only during an in-progress rebase
                  # (alongside REBASE_ONTO and REBASE_ORIG_HEAD)
```

Remote-tracking history is stored on internal branches named
`remotes/<remote>/<branch>`, with each remote's last-known tips in the
`remote_refs` table. Your local branches are never touched by `fetch`.

---

## Development

See [DEVELOPING.md](DEVELOPING.md) for the full build/test/release guide.

```bash
cargo build --release       # binary at target/release/velo
cargo test                  # unit + integration tests
./workflow_sim.sh           # end-to-end, two-developer workflow simulation
```

The test suite has three layers: unit and property tests (including randomised
3-way-merge and object-store round-trip properties), CLI integration tests that
drive the real binary and assert its output, and `workflow_sim.sh` — a scripted
simulation of a long-lived project that exercises every command in sequence and
ends by verifying the repository with `fsck`.

---

## License

MIT — see [LICENSE](LICENSE).

Built with 🦀 by [Lucas Vascovici](https://github.com/LucasVascovici).