## The Velody CLI

The `velody` command is one consumer of the engine: a Git-shaped tool for people
who want the same guarantees on a working directory.

Git is a masterpiece of engineering — but its interface was designed in 2005 for the Linux kernel, not for everyday developers in 2026. Velody keeps what works (content-addressed snapshots, cheap branching, cryptographic hashing, delta compression) and replaces what repeatedly trips people up.

### Why Velody

| Pain point | Git | Velody |
| :--- | :--- | :--- |
| **Staging area** | `git add` every file before every commit | Nothing. Disk = snapshot. |
| **Losing work** | Easy to `checkout` or `reset --hard` the wrong thing | Guards block destructive ops when you have unsaved changes |
| **Merge conflicts** | `<<<<<<<` markers break your code during resolution | Interactive hunk-by-hunk TUI — your files stay valid throughout |
| **Undo a commit** | `git reset --soft HEAD~1`, `git reset --hard`, `git reflog`... | `velody undo` — one command, reversible |
| **Redo an undo** | Not built-in — requires `git reflog` | `velody redo` |
| **Abort a merge** | `git merge --abort` (resets working tree) | `velody merge --abort` (restores to exact pre-merge state, even after all conflicts resolved) |
| **Named stashes** | `git stash push -m "name"`, recalled by index | `velody stash push <name>` (or `-m <name>`), recalled by name |
| **Branch history** | `git log --all --graph --oneline --decorate` | `velody history --all --graph` |
| **View a snapshot** | `git show <hash>` | `velody show <hash>` |
| **Apply one commit** | `git cherry-pick <hash>` | `velody cherry-pick <hash>` |
| **Accidental force-push** | `git push --force` overwrites remote history | `velody push` is fast-forward-only — divergence is refused with instructions |
| **Surprise rebases on pull** | `git pull` may merge or rebase depending on config | `velody pull` fast-forwards or stops and tells you — never silently rewrites |
| **Verify the repo** | `git fsck` (cryptic output) | `velody fsck` — plain-English report, `--repair` for fixable cruft |

---

### Velody vs Git — workflow comparison

#### Daily workflow

| Task | Git | Velody |
| :--- | :--- | :--- |
| Start tracking a folder | `git init` | `velody init` |
| Save your work | `git add -A && git commit -m "msg"` | `velody save "msg"` |
| **Rename a file** | `git mv a b` — then blame guesses by similarity | `velody mv a b` — the move is recorded, not inferred |
| See what changed | `git status` | `velody status` |
| See line-level diff | `git diff` | `velody diff` |
| View history | `git log` | `velody history` |
| View one commit | `git show <hash>` | `velody show <hash>` |
| Time-travel to a past state | `git checkout <hash>` | `velody restore <hash>` |
| Undo the last commit | `git reset --soft HEAD~1` | `velody undo` |
| Redo an undone commit | `git reflog` + `git reset` | `velody redo` |
| Annotate file with blame | `git blame <file>` | `velody blame <file>` |
| Search tracked files | `git grep <pattern>` | `velody grep <pattern>` |
| Squash last N commits | interactive rebase | `velody squash <n> "msg"` |
| Diff two commits | `git diff <a> <b>` | `velody diff <a> <b>` |
| Rebase branch | `git rebase <target>` | `velody rebase <target>` |
| Amend the last commit | `git commit --amend --no-edit` | `velody save --amend` |

#### Branches

| Task | Git | Velody |
| :--- | :--- | :--- |
| Create and switch branch | `git switch -c <name>` | `velody switch <name>` |
| Switch to existing branch | `git switch <name>` | `velody switch <name>` |
| List branches | `git branch` | `velody branches` |
| Delete a branch | `git branch -d <name>` | `velody branches --delete <name>` |
| Merge a branch | `git merge <branch>` | `velody merge <branch>` |
| Abort a merge | `git merge --abort` | `velody merge --abort` |
| Apply one commit | `git cherry-pick <hash>` | `velody cherry-pick <hash>` |
| Rebase branch | `git rebase <target>` | `velody rebase <target>` |
| Squash commits | `git rebase -i HEAD~N` | `velody squash <n> "msg"` |

#### Conflict resolution

| Task | Git | Velody |
| :--- | :--- | :--- |
| See conflicts | `<<<<<<<` markers in file | `velody conflicts <file>` (text or `--json`); interactive TUI — `velody resolve <file>` |
| Take our version | `git checkout --ours <file>` | `velody resolve <file> --take ours` |
| Take their version | `git checkout --theirs <file>` | `velody resolve <file> --take theirs` |
| Keep both sides | edit the markers by hand | `velody resolve <file> --take both` (ours then theirs, per hunk) |
| Accept your own edit | delete markers, `git add` | `velody resolve --mark <file>` |
| Resolve all at once | — | `velody resolve --all --take theirs` |
| Code validity during merge | ❌ Markers break syntax | ✅ File untouched; TUI shows both sides |
| Abort after resolving | ❌ `--abort` fails if you started editing | ✅ `--abort` works until `velody save` |

#### Stash

| Task | Git | Velody |
| :--- | :--- | :--- |
| Save dirty state | `git stash push -m "name"` | `velody stash push <name>` (`-m` also works) |
| List stashes | `git stash list` | `velody stash list` |
| Apply a stash | `git stash pop` or `git stash apply stash@{2}` | `velody stash pop <name>` |
| Drop a stash | `git stash drop stash@{2}` | `velody stash drop <name>` |
| Inspect a stash | `git stash show stash@{2} -p` | `velody stash show <name>` |

#### Collaboration

| Task | Git | Velody |
| :--- | :--- | :--- |
| Copy a repository | `git clone <url>` | `velody clone <url>` |
| Add a remote | `git remote add origin <url>` | `velody remote add origin <url>` |
| List remotes | `git remote -v` | `velody remote` |
| Download without merging | `git fetch` | `velody fetch` |
| Publish your commits | `git push` | `velody push` (fast-forward only) |
| Integrate remote commits | `git pull` | `velody pull` |
| See if you're ahead/behind | `git status` (after fetch) | `velody status` (after fetch) |
| Share history without a server | `git bundle create` | `velody bundle create <file>` |
| Import a shared history file | `git bundle unbundle` | `velody bundle apply <file>` |

#### Tags & maintenance

| Task | Git | Velody |
| :--- | :--- | :--- |
| Create a tag | `git tag v1.0` | `velody tag v1.0` |
| Tag a past commit | `git tag v1.0 <hash>` | `velody tag v1.0 <hash>` |
| List tags | `git tag` | `velody tag` |
| Delete a tag | `git tag -d v1.0` | `velody tag --delete v1.0` |
| Clean up old data | `git gc` | `velody gc` |
| Check repository integrity | `git fsck` | `velody fsck` / `velody fsck --repair` |

---

### Where Velody is intentionally different

**No staging area.** `git add` is a source of confusion and lost work for new and experienced users alike. Velody removes it entirely. Every save snapshots exactly what is on disk.

**Conflict resolution as a TUI, not markers.** When Velody detects a true conflict it stores both versions in the database and presents them hunk-by-hunk in an interactive navigator. Your file on disk is never modified until you confirm a resolution. Per-hunk: keep ours, take theirs, both in either order, or open `$EDITOR`. Sessions are resumable — progress is persisted to the database between runs.

**`merge --abort` always works.** Git's `--abort` fails if you have begun editing conflict files. Velody's `--abort` restores the working tree to its exact pre-merge state at any point — during conflicts, after resolving all conflicts, right up until `velody save` finalises the merge.

**Named stash shelves.** `git stash apply stash@{2}` requires you to remember an index in a list. `velody stash pop wip-auth` is self-documenting.

**Branch names resolve everywhere.** Any command that accepts a hash or tag (`show`, `cherry-pick`, `restore`) also accepts a branch name — it resolves to the branch tip automatically. Remote-tracking refs work too: `velody merge origin/main`, `velody show origin/main`.

**Sync that refuses to surprise you.** `velody push` is fast-forward-only: if the remote has commits you don't, the push is refused with the exact commands to fix it — there is no `--force` footgun. `velody pull` either fast-forwards or stops and tells you the branches diverged; it never silently merges or rewrites your history. Reconciliation is always an explicit `velody merge origin/<branch>` or `velody rebase origin/<branch>`. Branches whose snapshots the pushed branch merged are published with it, under their own names.

**Verifiable snapshots.** A snapshot's ID is a BLAKE3 hash of its *content* — the full file tree (paths, object hashes, and modes) plus its parents, message, timestamp, and any attached metadata. That means a snapshot can be checked against what it claims to contain, which is exactly what `velody fsck` does, and what makes importing history from another machine safe: every object is re-hashed and every snapshot ID recomputed on arrival before anything is trusted.

---

### Performance

Benchmarked on a monorepo with 571 files across 6 language modules, 40 incremental saves, and 8 concurrent branches.

| Command | Latency | How |
| :--- | :--- | :--- |
| `velody status` (warm) | ~35–60 ms | mtime+size index cache — no rehashing on unchanged files |
| `velody status` (cold) | ~50–200 ms | Parallel BLAKE3 across all CPU cores via Rayon |
| `velody save` (incremental) | ~285 ms avg | Parallel hashing + single SQLite transaction + Zstd |
| `velody restore` | ~200–800 ms | Parallel file writes; scales with number of changed files |
| `velody merge` | <100 ms | LCA found via recursive CTE; no file I/O needed |
| `velody history --all` | ~35 ms | Indexed ancestry walk in SQLite WAL mode |

At scale: tested to 10,000 files x 2,000 snapshots and 100,000 files x 200
snapshots (20 million metadata rows). History commands stay under 120 ms there;
a save settles near 2 s at 10,000 files and reaches 14-20 s at 100,000 files
after 200 snapshots. [docs/SCALE.md](SCALE.md) has the latency tables for
Windows and Linux, where velody degrades, the retention policy that bounds it, and
how to reproduce every number.

The warm-cache path for `velody status` is essentially N × `stat()` — no file reads, no hashing. Only files whose `mtime` or `size` changed since the last run are rehashed.


### Quick start

```bash
# Initialise a repository
velody init

# Save a snapshot
echo "hello world" > app.py
velody save "Initial commit"

# See what changed
velody status
velody diff

# View history and time-travel
velody history              # also: velody log, velody history -n 5
velody restore <hash>

# Work on a feature branch
velody switch feature/login
# ... edit files ...
velody save "Add login page"

# Merge back
velody switch main
velody merge feature/login
velody save "Merge feature/login"

# Share it
velody remote add origin /shared/app     # or ssh://user@host/srv/app
velody push
velody pull
```

---

### Command reference

#### Core workflow

| Command | Description |
| :--- | :--- |
| `velody init` | Initialise a new repository in the current directory (`velody init --objects database` keeps objects in `velo.db`, fixed for the repo) |
| `velody save "<message>"` | Snapshot all tracked files with a description |
| `velody mv <from> <to>` | Move a file **and record that it moved**, so `blame` and `history --file` follow it across |
| `velody save "<message>" -- <path>` | Snapshot only the listed paths; other changes remain unsaved |
| `velody save --amend` | Fold changes into the last snapshot, keeping its message |
| `velody save "<message>" --amend` | Replace the last snapshot and reword it (keeps same parent) |
| `velody status` | Show new, modified, and deleted files vs the last snapshot |
| `velody status -- <path>` | Restrict status output to specific paths |
| `velody diff [<file>]` | Show line-level diff against the last snapshot (see below for comparing snapshots) |
| `velody show <target>` | Inspect a past snapshot without restoring — hash, prefix, tag, or branch name |
| `velody show <target> -- <path>` | Restrict the diff to a specific file or directory |
| `velody blame <file>` | Annotate each line with the snapshot that last changed it |
| `velody blame <file> --at <target>` | Blame at a specific past snapshot, tag, or branch |
| `velody blame <file> --lines N-M` | Only these lines — and faster, since the walk stops sooner |
| | Shows the author alongside each line, when the history records one |
| `velody grep <pattern>` | Search tracked files for a regex pattern |
| `velody grep <pattern> --snapshot <target>` | Search inside a stored snapshot |
| `velody grep <pattern> -i` | Case-insensitive search |
| `velody grep <pattern> -l` | Print only file names with matches |
| `velody grep <pattern> -C <n>` | Show N lines of context around each match |

#### History and time-travel

| Command | Description |
| :--- | :--- |
| `velody history` | Linear history of the current branch (last 20 by default) |
| `velody history --all` | History across all branches |
| `velody history --graph` | ASCII branch graph with coloured lanes |
| `velody history --graph --all` | Full topology graph across all branches |
| `velody history --branch <n>` | History for a specific branch without switching |
| `velody history --file <path>` | Only snapshots that **changed** this file or directory (repeatable) |
| `velody history --from <target>` | What led to a snapshot, across branches — follows both parents through merges |
| `velody history --oneline` | Compact one-line-per-snapshot format |
| `velody history --limit <n>` | Limit the number of entries shown |
| `velody restore <target>` | Restore the working tree to a hash, prefix, tag, or branch name |
| `velody restore <target> --force` | Restore, discarding any unsaved changes |
| `velody restore <target> -- <path>` | Restore only specific files (PARENT is not updated) |
| `velody diff <a> <b>` | Diff between two snapshots; hash prefixes, tags, branches, and remote refs accepted |
| `velody diff <a>..<b>` | Same, using range syntax |
| `velody diff <a>` | Compare a snapshot against the current working tree |
| `velody diff <a> <b> -- <path>` | Restrict the diff to specific paths |
| `velody squash <n> "<msg>"` | Collapse the last N snapshots into one with a new message |
| `velody undo` | Remove the most recent snapshot and rewind the working tree |
| `velody redo` | Re-apply the most recently undone snapshot |

#### Branches

| Command | Description |
| :--- | :--- |
| `velody switch <name>` | Switch to a branch (creates it if it doesn't exist) |
| `velody switch --create <name>` (`-c`) | Create a branch even if its name is close to an existing one; errors if it exists |
| `velody switch <name> --force` | Switch, discarding any unsaved changes |
| `velody branches` | List all branches with their latest snapshot |
| `velody branches --delete <name>` | Soft-delete a branch (history preserved, purged by `velody gc`) |

#### Merging and conflict resolution

| Command | Description |
| :--- | :--- |
| `velody merge <branch>` | 3-way merge `<branch>` into the current branch |
| `velody merge --abort` | Restore the exact pre-merge state (works at any point before `velody save`) |
| `velody conflicts [<file>] [--json]` | Show both sides of each conflicting hunk, as text or JSON |
| `velody resolve <file>` | Interactive hunk-by-hunk conflict resolver |
| `velody resolve <file> --take ours` | Non-interactive: keep the current branch's version |
| `velody resolve <file> --take theirs` | Non-interactive: take the incoming branch's version |
| `velody resolve <file> --take both` | Non-interactive: keep both sides, ours then theirs, per hunk |
| `velody resolve --mark <file>` | Accept your hand edit of the file as the resolution; refused if it drops lines the other side changed cleanly (`velody conflicts` lists them), unless you add `--force` |
| `velody resolve --all --take <ours\|theirs\|both>` | Resolve all outstanding conflicts non-interactively |
| `velody cherry-pick <target>` | Apply the diff from one snapshot onto the current branch |
| `velody rebase <target>` | Replay current branch commits on top of another branch |
| `velody rebase --abort` | Abort the rebase and restore the original branch state |
| `velody rebase --continue` | Continue after resolving a rebase conflict |

#### Stash

| Command | Description |
| :--- | :--- |
| `velody stash push <name>` | Shelve dirty working-tree state under a name |
| `velody stash list` | List all stash shelves |
| `velody stash pop <name>` | Restore a shelf and delete it |
| `velody stash drop <name>` | Delete a shelf without restoring |
| `velody stash show <name>` | Inspect a shelf's contents |

#### Tags

| Command | Description |
| :--- | :--- |
| `velody tag <name>` | Tag the current snapshot |
| `velody tag <name> <target>` | Tag a specific snapshot by hash, prefix, or branch name |
| `velody tag <name> --force` | Overwrite an existing tag |
| `velody tag` | List all tags |
| `velody tag --delete <name>` | Delete a tag |

#### Collaboration & sync

| Command | Description |
| :--- | :--- |
| `velody clone <url> [dir]` | Copy a repository, set up `origin`, and check out its default branch |
| `velody remote add <name> <url>` | Add a remote (a filesystem path, `ssh://[user@]host[:port]/path` or `http://host[:port]/`) |
| `velody remote` | List configured remotes |
| `velody remote remove <name>` | Remove a remote and its tracking refs |
| `velody fetch [remote]` | Download remote history into `remotes/<remote>/*` — never touches your branches or working tree |
| `velody push [remote] [branch]` | Publish a branch (fast-forward only; refuses to overwrite remote work; merged side branches are published with it under their own names; tags on pushed history are published too, even ones made after their snapshot was pushed, and an existing remote tag is never moved) |
| `velody pull [remote]` | Fetch the current branch, then fast-forward — or report divergence and stop |
| `velody sync [remote] [branch]` | Fetch, then fast-forward, push, or merge and push if the merge is clean; a conflict stops with exit 3 and the merge left in progress (`resolve`, `save`, `sync`). Never pushes a conflicted merge, never forces |

Remote defaults to `origin`, and branch defaults to the current branch.

### Exit codes

Scripts can decide on the exit code:

| Code | Meaning |
|------|---------|
| 0 | Success, including every nothing-to-do: "nothing to save", "already up to date", "nothing to pull", "nothing to undo" and "nothing to redo" |
| 1 | Error, including a name that does not resolve and `switch` refusing a near-miss branch name (`mian` for `main`) without a terminal; `velody switch --create <name>` creates it anyway |
| 2 | Command-line usage error rejected by the argument parser, and nothing else |
| 3 | Conflicts left to resolve (`merge`, `cherry-pick`, `rebase` or `sync` stopped on conflicts) |
| 4 | Histories diverged (`pull`, or a `push` or `sync` the remote refused) |
| 5 | Repository locked by another velody process |

To host a repository over HTTP, run `velody serve-http <path> [--listen <addr>]`
(default `127.0.0.1:8417`); it prints `listening on http://<addr>/`. Then
`velody clone http://<addr>/` and the usual `push` / `pull` work against it, with
the same fast-forward-only rule. By default fetch is anonymous and **push is
refused** until you configure a token; there is no TLS. See "Hosting a remote"
below.

After a `fetch`, `velody status` shows ahead/behind, and `origin/<branch>`
can be used anywhere a ref is accepted (`merge`, `rebase`, `show`, `diff`).

#### Offline transfer

| Command | Description |
| :--- | :--- |
| `velody bundle create <file>` | Pack the entire repository into one self-contained file |
| `velody bundle create <file> <ref>` | Pack only the history reachable from a snapshot, tag, or branch |
| `velody bundle apply <file>` | Import a bundle into this repository (verified, idempotent) |

#### Maintenance

| Command | Description |
| :--- | :--- |
| `velody gc` | Remove orphaned objects and stale undo/conflict state |
| `velody gc --dry-run` | Report what `gc` would collect and remove nothing |
| `velody gc --keep-days <n>` | Retain undo history for `n` days before purging (default: 30) |
| `velody gc --vacuum` | Also run a full vacuum, converting a repository created before 5.3.0 to incremental `auto_vacuum` (needs free disk about the size of `velo.db`) |
| `velody repack [--dry-run]` | Convert a repository's stored full snapshots into deltas, a few at a time, to shrink it; no snapshot id changes. A dry run reports the estimated saving and writes nothing |
| `velody retention [--keep <age>=<every>] [--dry-run]` | Thin old snapshots by age (default: all for a day, hourly for a month, daily after), keeping signed, tagged and checked-out ones, then run `gc` and return the freed space |
| `velody fsck` | Verify repository integrity; exits non-zero if anything is wrong |
| `velody fsck --repair` | Also clean up safely-fixable cruft (orphaned rows, stale tracking refs) |
| `velody keygen --out <file>` | Write a new signing key (a secret) to `file` and print its public key |
| `velody sign [snapshot] --key <file>` | Sign a snapshot (default: the current one) with a key file |
| `velody report [--branch B] [--since T] [-o FILE] [--json] [--trust FILE] [--no-fsck]` | Write a self-contained HTML audit report that opens offline: an fsck verdict, a timeline per branch with signature badges, and line provenance. Everything in it is escaped and the file contains no script. `--json` prints the same data |
| `velody verify [snapshot] --trust <file>` | Check a snapshot's signatures against a file of trusted public keys; exits 1 unless one is trusted |

---

### Merge workflow example

```bash
# Start a feature branch
velody switch feature/payments
echo "stripe_key = 'live_...'" > config.py
velody save "Add payment config"

# Back on main, make a conflicting change
velody switch main
echo "stripe_key = 'test_...'" > config.py
velody save "Set test payment key"

# Merge — Velody finds the common ancestor automatically
velody merge feature/payments
# → Conflict: config.py

# Resolve interactively — hunk-by-hunk TUI, your file stays valid throughout
velody resolve config.py
# [1] Keep ours  [2] Take theirs  [3] Both  [e] Edit  [q] Quit

# Or resolve non-interactively
velody resolve config.py --take theirs

# Changed your mind? Abort at any point before saving
velody merge --abort   # ← restores exact pre-merge state

# Finalise
velody save "Merge feature/payments"
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

#### Conflicts only when the changes really overlap

Velody performs a real line-level 3-way merge. If both branches touched the same
file but in different places, both sets of changes are combined automatically —
you are only asked to resolve regions that genuinely overlap:

```bash
# ancestor:  DEBUG = False  …  RETRIES = 3
# feature/payments changed line 3;  main changed line 5
velody merge feature/payments
```
```
Merging 'feature/payments' into 'main' (ancestor: 38735ed3e87fca9f)…
  ~ Auto-merged: config.py

Merge summary
  New:      0
  Updated:  1
  Deleted:  0
  Conflicts: 0

✔ Clean merge! Run velody save "Merge <branch>" to finalise.
```

The merged file keeps **both** sides — `DEBUG = True` from the feature branch and
`RETRIES = 5` from main. The same engine backs `cherry-pick` and `rebase`.
