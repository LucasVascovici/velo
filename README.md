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

### Tags & maintenance

| Task | Git | Velo |
| :--- | :--- | :--- |
| Create a tag | `git tag v1.0` | `velo tag v1.0` |
| Tag a past commit | `git tag v1.0 <hash>` | `velo tag v1.0 <hash>` |
| List tags | `git tag` | `velo tag` |
| Delete a tag | `git tag -d v1.0` | `velo tag --delete v1.0` |
| Clean up old data | `git gc` | `velo gc` |

---

## Where Velo is intentionally different

**No staging area.** `git add` is a source of confusion and lost work for new and experienced users alike. Velo removes it entirely. Every save snapshots exactly what is on disk.

**Conflict resolution as a TUI, not markers.** When Velo detects a true conflict it stores both versions in the database and presents them hunk-by-hunk in an interactive navigator. Your file on disk is never modified until you confirm a resolution. Per-hunk: keep ours, take theirs, both in either order, or open `$EDITOR`. Sessions are resumable — progress is persisted to the database between runs.

**`merge --abort` always works.** Git's `--abort` fails if you have begun editing conflict files. Velo's `--abort` restores the working tree to its exact pre-merge state at any point — during conflicts, after resolving all conflicts, right up until `velo save` finalises the merge.

**Named stash shelves.** `git stash apply stash@{2}` requires you to remember an index in a list. `velo stash pop wip-auth` is self-documenting.

**Branch names resolve everywhere.** Any command that accepts a hash or tag (`show`, `cherry-pick`, `restore`) also accepts a branch name — it resolves to the branch tip automatically.

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
```

---

## Command reference

### Core workflow

| Command | Description |
| :--- | :--- |
| `velo init` | Initialise a new repository in the current directory |
| `velo save "<message>"` | Snapshot all tracked files with a description |
| `velo save "<message>" --amend` | Replace the last snapshot (keeps same parent) |
| `velo status` | Show new, modified, and deleted files vs the last snapshot |
| `velo diff [<file>]` | Show line-level diff against the last snapshot |
| `velo show <target>` | Inspect a past snapshot without restoring — hash, prefix, tag, or branch name |
| `velo show <target> -- <path>` | Restrict the diff to a specific file or directory |

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

### Maintenance

| Command | Description |
| :--- | :--- |
| `velo gc` | Remove orphaned objects and stale undo/conflict state |
| `velo gc --keep-days <n>` | Retain undo history for `n` days before purging (default: 30) |

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
●  a1b2c3d4  (main)   2026-03-22  Merge feature/payments
│╲
│ ○  e5f6a7b8  (feature/payments)  2026-03-22  Add payment config
│ ╱
│
○  c9d0e1f2  (main)   2026-03-22  Set test payment key
│
○  a3b4c5d6  (main)   2026-03-22  Initial commit
```

---

## Architecture

| Layer | Technology | Role |
| :--- | :--- | :--- |
| Hashing | BLAKE3 | Collision-proof, 10× faster than SHA-1; `rayon` parallelises large files |
| Compression | Zstd level 1 | Fast compression on save; transparent decompression on restore |
| Metadata | SQLite (WAL mode) | Snapshots, branches, tags, ancestry, conflicts, stash — indexed queries |
| Mtime cache | `index_cache` table | `(path, mtime_ns, size, hash)` — skips rehashing unchanged files |
| Concurrency | Rayon | Parallel filesystem walk, hash-and-compress, and file writes on restore |
| I/O | memmap2 | Memory-maps files ≥256 KB to avoid kernel→userspace copy |

**Delta storage.** Each snapshot records only changed files. Unchanged files are stored as references to the same object from the parent. A 1000-file project where 10 files change creates 10 new objects, not 1000.

**Object store.** Content-addressed storage under `.velo/objects/`. Each object is Zstd-compressed file content named by its BLAKE3 hash. Identical content across branches and snapshots is stored exactly once.

**Merge parents.** Merge commits record both their primary parent and their merge-source parent (`merge_parent` column in the `snapshots` table). This is what enables the two-parent topology in `velo history --graph`.

**Schema migrations.** The database schema is versioned via `pragma_table_info` checks. New columns are added automatically on first use — existing repositories are upgraded in place with no manual intervention.

---

## Repository layout

```
.velo/
├── velo.db       # SQLite database: snapshots, branches, tags, ancestry, stash, conflicts
├── objects/      # Content-addressed object store (Zstd-compressed, named by BLAKE3 hash)
├── HEAD          # Current branch name
├── PARENT        # Hash of the current snapshot
└── MERGE_HEAD    # Present only during an in-progress merge or cherry-pick
                  # Format: "<pre-merge-hash>:<source-branch>"
```

---

## License

MIT — see [LICENSE](LICENSE).

Built with 🦀 by [Lucas Vascovici](https://github.com/LucasVascovici).