# Agent-checkpointing benchmark

`agent_bench.py` compares velody with the things an agent author would actually
reach for when they want a trail of what an agent did and a way back. Python 3,
standard library plus the velody Python binding and the `git` CLI.

## Systems

| system | how a step is stored |
|---|---|
| velody | `save_changes` per step with only the changed files (the core carries the rest of the parent's tree forward); author and `agent` metadata on every snapshot |
| velody (5.2 API) | the baseline: `save_tree` per step; untouched files carried forward with `TreeEntry.stored`, the tree re-read with `tree_at` after every save; restore by `tree_at` plus `read_file_at` per file |
| shadow git | `git --git-dir=shadow.git --work-tree=work add -A && commit` per step, author `agent-run-1` |
| git worktree | an ordinary repository; each attempt in the branch workload gets its own `git worktree add -b` |
| dir copy | `shutil.copytree` of the whole project per step |
| jsonl log | one JSON line per step holding the full text of the changed files, `fsync`ed |
| sqlite blobs | `blobs(step, path, data)` plus a `manifest(step, path, blob_step)` of the whole tree per step, committed per step |

All seven sit behind one interface (`setup`, `checkpoint`, `branch`, `merge`,
`restore`, `who_wrote`, `disk`), so the driver is identical for each.

## Workloads

1. **Steps.** 500 steps over a 1,000-file project (20 lines of about 60 bytes per
   file, about 1 KB). Each step rewrites 1 to 20 files, 1 to 3 lines each, then
   checkpoints. Timed: the `checkpoint` calls only. The driver writes the edited
   files to the working directory before each call, untimed, because git and the
   copy need them there; velody is handed the bytes directly and never reads them.
2. **Branches.** From the final main state, 8 attempts of 10 steps each (same
   edit generator), then the last attempt "wins" and is merged. Timed: branch
   creation, every checkpoint and the merge. Merge means a real three-way merge
   in velody and git; the copy, log and SQLite stores have no merge, so the winner
   becomes a new step holding the winner's state (copied directory, appended
   changed files, a new manifest). The disk column is measured after this.
3. **Restore.** Restore step 250 (half way) 100 times into an empty directory;
   the file count is asserted. Velody does this with one `materialize` call (the 5.2 API row
   uses `tree_at` for the file list plus `read_file_at` per file, written to disk); git uses
   `git checkout <commit> -- .` into a separate work tree with its own index;
   the log is replayed from the start; SQLite joins the manifest to the blobs.
4. **Who wrote this line.** 20 seeded (file, line) pairs at the end of the main
   line. Velody uses `blame`; git uses `git blame -L`; the others scan versions
   newest first until the line changes. Edits never add or remove lines, so a
   line number is stable and every answer is checked against the generator's
   own record (the run aborts on a wrong answer).

The RNG is seeded (`--seed`, default 1), so every system sees the identical
project and edits.

## Disk

Bytes on disk of each store: the sum of file sizes (apparent size, not blocks;
Windows reports no block count). Velody is its repository directory; shadow git
is `shadow.git`; git worktree is `.git` plus every extra worktree (the attempts'
checked-out trees are part of the cost); dir copy is every step directory and
attempt directory; the log is the file; SQLite is the database file. The live
working directory is not counted for any system.

## Provenance

The last column states what each system can verify about a step after the fact,
without extra discipline. Velody hashes author and metadata into the snapshot id;
git hashes content and commit fields but an agent identity is whatever the
committer typed; the rest record nothing that can be verified.

## Run

```
# build the Python binding in release mode (see bindings/python/README.md)
python -m venv .venv
.venv/bin/pip install maturin          # .venv/Scripts on Windows
.venv/bin/maturin develop --release --manifest-path bindings/python/Cargo.toml

.venv/bin/python bench/agent/agent_bench.py --out bench/results/agent-<date>-<platform>.json
```

`--quick` runs 50 steps over 100 files with 10 restores (about a minute on
Windows) and is for checking the harness. `--only "velody,dir copy"` runs a
subset. The JSON holds every number plus the environment (platform, Python,
git, SQLite, the commit the script was run from, the parameters). A markdown
table is printed to stdout. A debug build of the binding makes velody several
times slower; always benchmark `--release`.

The `agent` job in `.github/workflows/bench.yml` does the same on Ubuntu and
uploads the JSON (Actions, Run workflow, suite `agent`).

## Windows Defender caveat

On Windows every file write and process start can be scanned. The numbers for
the systems that write many small files or spawn processes (git, dir copy) move
a lot with real-time protection and with exclusions. Pass
`--defender-note "<what you checked>"`; without it the JSON records
"assume real-time scanning is ON". Compare systems within a platform, not
across platforms.
