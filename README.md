<p align="center">
  <img src="https://img.shields.io/github/v/release/LucasVascovici/velo?color=orange&logo=github&label=latest" />
  <img src="https://github.com/LucasVascovici/velo/actions/workflows/ci.yml/badge.svg" />
  <img src="https://github.com/LucasVascovici/velo/actions/workflows/release.yml/badge.svg" />
  <img src="https://img.shields.io/github/license/LucasVascovici/velo?color=blue" />
</p>

<h1 align="center">🚀 Velo</h1>

<p align="center">
  <strong>A fast, safe, and intuitive version control system built in Rust.</strong><br/>
  Git without the footguns.
</p>

> **Note:** This project was fully vibe-coded for fun — high-level intent, modern tech stack, tight feedback loop with an AI assistant. The result is a real, working tool, but it was built as an experiment in what's possible with that workflow, not as a production-grade replacement for Git.

---

Git is a masterpiece of engineering — but its interface was designed in 2005 for the Linux kernel, not for everyday developers in 2025. Velo keeps what works (snapshots, branching, hashing, compression) and replaces what doesn't.

- **No staging area.** What's on your disk is what gets saved. There is no `git add`.
- **Conflict files, not conflict markers.** Merge conflicts write a `.conflict` sidecar file. Your original code stays valid and runnable during the resolution process.
- **Destructive-operation guards.** Velo blocks branch switches, merges, and restores when you have unsaved changes. By default, you cannot lose work.
- **Undo that actually works.** `velo undo` removes the last snapshot *and* restores the working tree. `velo redo` puts it back.
- **True 3-way merges.** Velo finds the lowest common ancestor before merging — a file changed only on one side is never flagged as a conflict.

---

## Performance

Benchmarked on a monorepo with 571 files across 6 language modules, 40 incremental saves, and 8 concurrent branches.

| Command | Latency | How |
| :--- | :--- | :--- |
| `velo status` (warm) | ~35–60 ms | mtime+size index cache — no rehashing on unchanged files |
| `velo status` (cold) | ~50–200 ms | parallel BLAKE3 across all CPU cores via Rayon |
| `velo save` (incremental) | ~285 ms avg | parallel hashing + single SQLite transaction + Zstd |
| `velo restore` | ~200–800 ms | parallel file writes; scales with number of changed files |
| `velo merge` | <100 ms | LCA found via recursive CTE; no file I/O needed |
| `velo logs --all` | ~35 ms | indexed ancestry walk in SQLite WAL mode |

The warm-cache path for `velo status` is essentially `N × stat()` — no file reads, no hashing. Only files whose `mtime` or `size` changed since the last run are rehashed.

---

## Installation

### Unix (Linux & macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh
```

By default the binary is installed to `/usr/local/bin` (with `sudo` if needed) or `~/.local/bin` if sudo is unavailable. To install to a custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh -s -- --dir ~/.local/bin
```

To preview what the script would do without installing anything:

```bash
curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh -s -- --dry-run
```

### Windows

Download the latest `velo-x86_64-windows.zip` from the [Releases page](https://github.com/LucasVascovici/velo/releases), extract `velo.exe`, and place it anywhere on your `PATH`.

### All platforms — pre-built binaries

| Platform | File |
| :--- | :--- |
| Linux x86-64 (musl, static) | `velo-x86_64-linux.tar.gz` |
| Linux ARM64 (musl, static)  | `velo-aarch64-linux.tar.gz` |
| macOS Apple Silicon         | `velo-aarch64-macos.tar.gz` |
| macOS Intel                 | `velo-x86_64-macos.tar.gz` |
| Windows x86-64              | `velo-x86_64-windows.zip` |

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

# Time-travel
velo logs
velo restore <hash>
```

---

## Command reference

### Core workflow

| Command | Description |
| :--- | :--- |
| `velo init` | Initialise a new repository in the current directory |
| `velo save "<message>"` | Snapshot all tracked files with a description |
| `velo status` | Show new, modified, and deleted files vs the last snapshot |
| `velo diff [<file>]` | Show line-level diff against the last snapshot |
| `velo diff <file> --conflict` | Diff your version against the `.conflict` sidecar |

### History and time-travel

| Command | Description |
| :--- | :--- |
| `velo logs` | Linear history of the current branch (last 20 by default) |
| `velo logs --all` | History across all branches |
| `velo logs --branch <name>` | History for a specific branch without switching to it |
| `velo logs --oneline` | Compact one-line format |
| `velo logs --limit <n>` | Limit the number of entries shown |
| `velo restore <target>` | Restore the working tree to a snapshot hash, prefix, or tag |
| `velo restore <target> --force` | Restore, discarding any unsaved changes |
| `velo undo` | Remove the most recent snapshot and rewind the working tree |
| `velo redo` | Re-apply the most recently undone snapshot |

### Branches

| Command | Description |
| :--- | :--- |
| `velo switch <name>` | Switch to a branch (creates it if it doesn't exist) |
| `velo switch <name> --force` | Switch, discarding any unsaved changes |
| `velo branches` | List all branches with their latest snapshot |
| `velo branches --delete <name>` | Soft-delete a branch (history is preserved, can be GC'd later) |

### Merging

| Command | Description |
| :--- | :--- |
| `velo merge <branch>` | 3-way merge `<branch>` into the current branch |
| `velo merge --abort` | Abort an in-progress merge and remove all conflict files |
| `velo resolve <file> --take ours` | Resolve a conflict by keeping the current branch's version |
| `velo resolve <file> --take theirs` | Resolve a conflict by taking the incoming branch's version |
| `velo resolve <file>` | Mark a conflict as resolved after manual editing |
| `velo resolve --all --take <ours\|theirs>` | Resolve all outstanding conflicts at once |

### Tags

| Command | Description |
| :--- | :--- |
| `velo tag <name>` | Tag the current snapshot |
| `velo tag <name> <hash>` | Tag a specific past snapshot by hash or prefix |
| `velo tag <name> --force` | Overwrite an existing tag |
| `velo tag` | List all tags |
| `velo tag --delete <name>` | Delete a tag |

### Maintenance

| Command | Description |
| :--- | :--- |
| `velo gc` | Remove orphaned objects and old undo history |
| `velo gc --keep-days <n>` | Keep undo history for `n` days before permanent deletion (default: 30) |

---

## Merge workflow example

```bash
# Start a feature branch
velo switch feature/payments
echo "stripe_key = '...'" > config.py
velo save "Add payment config"

# Back on main, make a conflicting change
velo switch main
echo "stripe_key = 'test'" > config.py
velo save "Set test payment key"

# Merge — Velo finds the common ancestor automatically
velo merge feature/payments
# → config.py has a conflict (both sides changed since the ancestor)

# Inspect the conflict
velo diff config.py --conflict

# Accept one version, or edit config.py manually and then resolve
velo resolve config.py --take theirs
# or: velo resolve config.py  (after manual edit)

# Finalise
velo save "Merge feature/payments"
```

---

## How `.conflict` files work

When Velo detects a true conflict (both branches modified the same file since their common ancestor), it writes the incoming branch's version as `<file>.conflict` and leaves your version intact as `<file>`. Your code stays in a valid, runnable state — there are no `<<<<<<<` markers inside your file.

```
config.py          ← your version (current branch)
config.py.conflict ← their version (incoming branch)
```

Use `velo diff config.py --conflict` to see the two versions side by side. Once resolved, run `velo resolve config.py` to clear the sidecar file.

---

## Architecture

| Layer | Technology | Role |
| :--- | :--- | :--- |
| Hashing | BLAKE3 | Collision-proof, 10× faster than SHA-1; `rayon` splits large files across CPU cores |
| Compression | Zstd level 1 | Fast compression on save; transparent decompression on restore |
| Storage | SQLite (WAL mode) | Structured metadata — snapshots, branches, tags, ancestry — with indexed queries |
| Mtime cache | `index_cache` table | Stores `(path, mtime_ns, size, hash)`; skips rehashing unchanged files |
| Concurrency | Rayon | Parallel filesystem walk, parallel hash-and-compress, parallel file writes on restore |
| I/O | memmap2 | Memory-maps files ≥256 KB to avoid kernel→userspace copy during hashing |

**Delta storage:** each snapshot records only the files that changed. Unchanged files are stored as references to the same object from the parent snapshot. A 1000-file project where 10 files change creates 10 new objects, not 1000.

**Object store:** content-addressed storage under `.velo/objects/`. Each object is the Zstd-compressed file content, named by its full BLAKE3 hash. Identical content is stored exactly once across all snapshots and branches.

---

## Repository layout

```
.velo/
├── velo.db       # SQLite database: snapshots, branches, tags, ancestry, index cache
├── objects/      # Content-addressed object store (Zstd-compressed)
├── HEAD          # Current branch name
├── PARENT        # Hash of the current snapshot
└── MERGE_HEAD    # Present only during an in-progress merge
```

---

## License

MIT — see [LICENSE](LICENSE).

Built with 🦀 by [Lucas Vascovici](https://github.com/LucasVascovici).
