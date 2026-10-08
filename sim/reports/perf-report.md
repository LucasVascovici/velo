# velo performance report (run1)

Scope: 1000 files / 11.4 MB, 200 snapshots, 16 MB big file; idle-machine bench (`perf/results.json`) plus 510 calls from 4 concurrent simulated developers (`perf/cmdstats.json`). Source reviewed read-only; nothing was profiled or executed, so every cause below comes from reading code, plus a read-only look at `perf/work/deep/.velo/velo.db` (immutable open). Causes are marked **[read]** (seen in code) or **[inferred]**.

**Overall grade: B.** Everyday commands (status, save, history, show, fsck, gc, tag, branches) sit at or near process-start cost and mostly beat git. The grade is held down by one design weakness that shows up in five flagged metrics: every whole-tree rewrite (undo, redo, switch, restore, clone checkout) rewrites all files regardless of what differs. A second weakness is that `file_map` stores a full tree copy per snapshot, so metadata grows as snapshots x files.

## Caveat on git numbers

git gc 151 s, git fsck 36 s and git initial add+commit 17 s are not credible for 11 MB / 200 commits (typically 1-5 s, 0.5-2 s, 0.3-1 s). They point to Defender / file-system scanning of the many small loose files git creates, or machine noise. The ratios on those three (velo 360x gc, 53x fsck, 3.6x initial save) should not be quoted. The "save latency growth" git figure (1216 ms) is also not informative (git does add+commit per iteration). Below ~50 ms both tools are at process-start cost and differences are noise.

## Metric table

Budgets are guesses; the last column says whether the flag is real, noise, or a budget problem.

| Metric | velo ms | git ms | budget | verdict | Real? / comment |
|---|---:|---:|---:|---|---|
| startup --version | 21 | - | 40 | ok | fine |
| startup status (tiny) | 50 | 88 | 60 | ok | fine |
| init | 88 | 76 | 100 | ok | noise-level |
| save initial (1000 files) | 4752 | 17048 | 3000 | over-budget | Modest and real: ~4.7 ms/file, I/O bound (F6). git number suspect. Budget slightly tight |
| status clean | 81 | 48 | 150 | ok | 1.7x git but both near floor (F8) |
| status 1 modified | 93 | 53 | 200 | ok | same |
| diff worktree | 72 | 55 | 200 | ok | fine |
| save 1 file | 122 | 225 | 200 | ok | good |
| save no change | 143 | 153 | 200 | ok | slower than status (81) for the same work (F8) |
| save 20 files | 226 | 639 | 400 | ok | good |
| diff snapshot..snapshot | 270 | 82 | 200 | over-budget | **Real** (F3). Budget fair |
| show snapshot | 66 | 58 | 150 | ok | fine |
| grep regex | 584 | 97 | 400 | over-budget | **Real** (F4): serial, 6x git. Budget fair |
| history default | 35 | 39 | 100 | ok | fine |
| branches | 53 | 40 | 100 | ok | noise |
| tag | 47 | 56 | 100 | ok | fine |
| save latency growth (last50/first50) | 178 -> 353 | 1216 | ratio 1.5 | over-budget | **Real, moderate** (F2): ratio 1.98, p95 464 ms |
| history --oneline --limit 20 | 47 | 62 | 100 | ok | fine |
| history --graph --all | 40 | 59 | 800 | ok | budget far too loose |
| blame (deep, 200 snaps) | 274 | 248 | 500 | ok | on par with git |
| history --file (deep) | 147 | 144 | 500 | ok | on par; budget loose |
| fsck | 678 | 36013 | 3000 | ok | strength (git number suspect) |
| undo | 531 | - | 200 | over-budget | **Real** (F1). 200 ms is a fair UX target |
| redo | 1911 | - | 200 | over-budget | **Real** (F1), 3.6x undo |
| squash 10 | 254 | - | 1000 | ok | fine |
| gc | 414 | 151667 | 3000 | ok | strength (git number is noise) |
| restore old snapshot | 317 | - | 800 | ok | same root cause as F1 |
| switch branch (200 files differ) | 1534 | 201 | 500 | over-budget | **Real** (F1): 7.6x git |
| merge clean (200+200) | 411 | 60 | 1000 | slow-vs-git | Mild; serial reconcile and writes (F7). Low priority |
| merge with 20 conflicts | 109 | - | 1500 | ok | fine; budget loose |
| stash push+pop | 390 | - | 500 | ok | dirty check + restore underneath |
| clone (path) | 5168 | 3745 | 2000 | over-budget | **Real** (F5); git is also slow here |
| push 50 snapshots | 758 | - | 1500 | ok | fine |
| fetch (no change) | 229 | - | 300 | ok | high for a no-op; minor |
| pull 50 snapshots | 480 | - | 800 | ok | fine |
| clone (http) | 5041 | - | 2500 | over-budget | same as path clone: import side, not transport |
| bundle create | 855 | 753 | 2000 | ok | on par |
| bundle apply | 4654 | - | 2000 | over-budget | **Real** (F5): 5.4x create |
| save big file initial (16 MB) | 512 | 181 | 3000 | ok | 2.8x git (multiple passes, F6); absolute fine |
| save big file 64 KB edit | 227 | 1016 | 800 | ok | strength |
| parallel 4 writers (wall, 10 saves each) | 19252 | - | 6000 | over-budget | Mostly expected; budget meaningless (see Concurrency) |
| status while a writer saves | 57 | - | 500 | ok | strength |

## Findings, ranked by severity

### F1 (high): whole-tree restore rewrites every file and leaves the next dirty check cold
Affects undo (531 ms), redo (1911 ms), switch (1534 ms vs git 201), restore, stash pop, clone checkout. These are the "feel" commands.

Likely causes **[read]**:
1. `crates/velo-core/src/commands/restore.rs`, `run()` -> `write_files()`: iterates all `snapshot_files` of the target and decompresses and writes each (`storage::apply_file` does `fs::write` unconditionally). It never compares with the tree being left. Undo differs by one commit yet rewrites 1000 files; `switch.rs` uses the same path, so a 200-file difference costs the same as a 1000-file one.
2. Same function, afterwards: `invalidate_cache_entries()` (`commands/mod.rs`) deletes the `index_cache` row of every written file and nothing re-seeds it. The next `get_dirty_files` (start of every redo, switch, merge, save, status) re-reads and BLAKE3-hashes the whole tree. This explains redo (1911) being about 3.6x undo (531): undo runs on a warm cache left by the preceding saves; redo runs right after undo's rewrite, so its dirty check is cold.
3. `get_dirty_files` runs twice per undo/redo/switch (once in the command, once in `restore::run`), each loading `file_map` and the whole `index_cache` into HashMaps.

Suggestions (highest value change):
- In `restore::run`, load the current tree (the `file_map` of `.velo/PARENT`; `remove_ghosts` already queries it) and write only paths whose `(hash, mode)` differ or are missing. Unchanged paths are skipped. With `force`, rewrite only paths the dirty scan flagged plus the differing ones.
- After `write_files`, stat each written file and `INSERT OR REPLACE` its `(mtime_ns, size, hash)` into `index_cache` rather than deleting the row; the hash is the object name, already known. Then the following status/save is warm.
- Pass the dirty set computed by `undo.rs`/`redo.rs`/`switch.rs` into `restore::run` to avoid the second scan.
- Expected result: undo/redo/switch near status-plus-delta cost (about 100-150 ms).

### F2 (medium): save latency doubles over 200 snapshots; metadata dominates storage
First50 median 178 ms, last50 353 ms, p95 464 ms. The dirty check does not depend on history, so growth is in the write path.

Likely causes:
- **[read]** `commands/save.rs` inserts the entire tree into `file_map` for every snapshot (`for (p,h,m) in &tree { ins.execute(...) }`) even when one file changed: about 1000 row inserts into a table with two indexes (`idx_filemap_snap`, `idx_filemap_path`, `db.rs`). `idx_filemap_path` is non-unique on `path` (up to 200 duplicates per path), so these are scattered inserts into a growing B-tree. Save also reads the parent's whole tree back and `commands::snapshot_id` sorts and hashes all entries.
- **[inferred]** WAL growth and checkpoint cost as the DB approaches 64 MB (`apply_pragmas`: WAL, synchronous NORMAL, default autocheckpoint). Not measured.
- Not verified which dominates; time the insert loop and check with a sampling profiler.

Suggestions: (a) check who needs `idx_filemap_path` (blame / history --file probably) and drop or narrow it if possible; (b) carry unchanged files forward with one `INSERT INTO file_map SELECT ?new, path, hash, mode FROM file_map WHERE snapshot_hash = ?parent AND path NOT IN (changed/deleted)` so those rows never cross into Rust, and compute the id incrementally; (c) longer term, a per-directory tree-object model makes save O(changed paths x depth) (see F9).

### F3 (medium): `diff <snap>..<snap>` decompresses every file of both snapshots
270 ms vs git 82 ms. **[read]** `commands/diff.rs`, `between()`, `Some(b)` arm: for every path in the union it calls `read_opt` on both sides (zstd decode plus UTF-8 conversion) before comparing, then discards identical files (`if old == new { continue; }`). The sibling `snapshot_diff()` already short-circuits correctly (`(Some(oh), Some(nh)) if oh != nh`).
Suggestion: compare `a_files.get(&path) == b_files.get(&path)` first and skip without touching the object store; cost then scales with changed files. The worktree arm (`None`) can likewise skip paths whose hash already matches the snapshot. Side note, not verified: `snapshot_diff` tests `is_binary(&full_path)` on the working-tree file, not the snapshot object, which looks wrong for historical diffs.

### F4 (medium): grep is single-threaded, 6x slower than git
584 vs 97 ms. **[read]** `commands/grep.rs`: `grep_working_tree` and `grep_snapshot` are serial `for` loops (read or decompress, then regex). Status, save and restore already use rayon.
Suggestion: `par_iter` over files (snapshot path: `ObjectStore::par_with_content`, which exists for batching on the database backend), sort at the end for stable output. A literal prefilter (memchr) for plain patterns helps further.

### F5 (medium): clone and bundle apply, about 5 s for 243 snapshots / 1444 objects
Bundle create is 855 ms; apply 4654 ms; clone 5.0-5.2 s over either transport, so the cost is the import side plus checkout, not network.
Likely causes **[read]** `commands/bundle.rs`, `import_pack()`:
1. The object loop is serial: per object zstd decode, BLAKE3 re-verify, `path.exists()`, then `write_atomic` (temp file write plus rename, two metadata operations each, scanned by Defender on Windows). 1444 x about 2 ms is about 3 s, matching the observation. Parallelise (rayon, as `put_paths` does). Do the `exists()` check before the decode and re-verify for objects already present.
2. Then it rebuilds `Vec<(path, hash, mode)>` per snapshot (clones 243k rows of strings), recomputes `snapshot_id` for each snapshot and inserts 243k `file_map` rows into two indexes. **[inferred]** a few hundred ms; scales with history x files.
3. Clone then checks out 1000 files (F1 write path, parallel).
Split between 1, 2 and 3 is not verified.

### F6 (low): save reads each changed file up to three times
**[read]** `save.rs` calls `get_dirty_files` (hashes cache-miss files), then `ObjectStore::put_file` (`storage.rs`) hashes again and, if the object is new, reads a third time to compress, then `write_atomic`s it. Visible on the initial 1001-file save (4.75 s) and the 16 MB file (512 ms vs git 181 ms).
Suggestion: return the hashes the dirty scan already computed, and make `put_file` take a known hash and a single read buffer (read once, hash, compress, write).

### F7 (low): merge applies files serially
Clean merge of 400 files 411 ms vs git 60 ms. **[read]** `commands/apply.rs` writes one file at a time through `storage::apply_file`; `merge.rs` plan loop calls `reconcile_file` serially (decompresses base, ours and theirs for both-changed files). Parallelise the writes (as restore does) and the reconcile fan-out. Conflicting merge is only 109 ms, so this is low priority.

### F8 (low): warm status/save floor
status clean 81 vs git 48 ms; `save no change` 143 ms vs status 81. **[read]** `get_dirty_files` loads the entire `file_map` of PARENT and the entire `index_cache` into `HashMap<String,...>` (cloning hash strings) on each call. Fine at 1000 files; costly at 50k+. A join iterated during the walk avoids the big maps. `save` with nothing to save should cost the same as `status`.

### F9 (informational): full tree per snapshot in `file_map`
`velo.db` is 64 MB for 243 snapshots x 1000 files = 243,243 `file_map` rows (about 260 B/row with two indexes), while `objects/` is only 7.4 MB. About 88% of `.velo` is tree metadata, growing as snapshots x files rather than with the amount of change. At 10k files and 2k snapshots this is about 20 M rows (several GB). Mitigations: store hashes as 32-byte blobs instead of 64-char hex (halves the dominant column and index key), `WITHOUT ROWID` with primary key `(snapshot_hash, path)`, drop the path index, and medium term a tree-object or delta model. `compact.rs` exists but was not exercised here.

### F10 (high for robustness): interactive resolve hangs without a TTY while holding the lock
`velo resolve <file>` without `--take` launches the TUI whether or not stdin/stdout is a terminal (`velo-cli/src/main.rs` `run_resolve` -> `velo_tui::resolve_interactive`; `velo-tui/src/lib.rs` has no `is_terminal` check) **[read]**. In the simulation three calls blocked 140-146 s (rc 1), which is the whole `resolve` p95 of 141 s in cmdstats. `run_resolve` holds the repository write guard for the entire session, so other mutating commands on that repository fail meanwhile. The 4 "another Velo operation is in progress" hits in cmdstats are all `resolve`, 27-190 ms **[inferred]**: lock held by another simulated agent's hung or long resolve.
Suggestion: refuse the TUI when stdin or stdout is not a TTY with a clear message ("use --take ours|theirs, or run in a terminal"), and take the repo lock per decision commit, not per interactive session.

## Scaling notes

- **History depth:** `save` doubles over 200 snapshots (F2). `history`, `--graph --all` and `show` are flat at 35-66 ms, so they are not O(history) in practice. `blame` (274 ms) and `history --file` (147 ms) with 200 touching snapshots are on par with git. `fsck` 678 ms and `gc` 414 ms are fine.
- **File count, O(files) where O(changes) is expected:** undo, redo, switch, restore (F1); `diff snap..snap` (F3); `save` file_map copy (F2); grep (F4); status/save floor (F8); clone/bundle apply (F5).
- **File size:** well handled. 16 MB incompressible initial save 512 ms; a 64 KB edit 227 ms with +0.12 MB growth thanks to FastCDC chunking (`storage.rs` `store()`); git 1016 ms.
- **undo vs redo asymmetry** (0.5 vs 1.9 s) is explained by F1 item 2 (cold cache after undo's rewrite), not inherent.

## Concurrency

- **4 writers, 19.3 s wall, 10 saves each:** 40 saves each followed by pull/merge/push. 50 refused pushes (about 1.25 per success) is what fast-forward-only push implies under 4-way racing, so retries are expected, not a bug. About 0.5 s per successful publish cycle is consistent with save (150-350 ms) plus fetch plus push (80-450 ms each) under contention. The 6 s budget has no basis (depends on retry policy); replace it with "no starvation and no data loss".
- **w3 never succeeded:** starvation from the harness retry cap, not a velo fault. Suggest randomised backoff in the harness; optionally a velo `sync` command that does fetch+merge+push under one lock to shrink the race window. The push refusal text is good.
- **Lock contention (4 of 510 calls, 0.8%):** all from `resolve` (F10); no save/push pair collided, so short writers rarely overlap in this workload. The lock is fail-fast (`lock.rs`, `try_lock_exclusive`, no wait), so any overlap becomes a user-visible error. Suggest a bounded wait (2-5 s with 10-50 ms backoff) in the CLI before surfacing `Locked`.
- **status while a writer saves:** 57 ms, never blocked (read-only commands take no lock; WAL). In cmdstats `status` p50 68 ms, p95 296 ms over 143 calls under 4-way contention.
- Other contended-run numbers: save p50 78 ms / p95 335 ms (small sim project, not comparable with the 1000-file bench), push/pull/fetch p50 77-93 ms. Outliers (fetch 542 ms, status 459 ms) look like CPU/disk contention; no pathological tail except the TUI resolves.
- cmdstats failures are mostly expected refusals (push 10/25) or commands the simulated developers mistyped (`log`, `diff-worktree`, `stash-push`, `history-all` do not exist). Only `stash show` (862 ms, rc 1) was a slow failure; not investigated.

## Storage

| | velo | git |
|---|---:|---:|
| after 200 snapshots (tree 11.4 MB) | 58.7 MB | 86.5 MB |
| metadata (`velo.db`, at 243 snapshots) | about 64 MB (88%) | - |
| objects + chunks | about 7.4 MB | - |
| 64 KB edit of 16 MB file | +0.12 MB | not reported |

velo is 32% smaller than the git repo as measured, but git's 86.5 MB is almost certainly loose objects before `git gc` (and the 151 s gc was not reported with a size). A packed git repo would usually be much smaller, so do not read this as a win without a packed comparison. Object storage itself (7.4 MB for 200 snapshots) is excellent; `file_map` metadata is the cost (F9). Chunk dedup on the big file is near-ideal.

## Strengths

- fsck 678 ms and gc 414 ms (git 36 s and 152 s, suspect; velo wins even after a 10x discount).
- save 20 files 226 vs 639 ms (2.8x); save 1 file 122 vs 225 ms (1.8x).
- big-file 64 KB edit 227 vs 1016 ms (4.5x) with +0.12 MB growth; chunked storage is the standout design.
- history --graph --all 40 vs 59 ms; --oneline 47 vs 62; tag 47 vs 56; startup status 50 vs 88.
- Deep-history blame and history --file on par with git.
- Reads never block on writers (status during save 57 ms); advisory lock is crash-safe.
- Conflicting merge 109 ms, squash 254 ms, stash push+pop 390 ms, push/pull of 50 snapshots 758/480 ms, startup 21 ms.

## Suggested order of work

1. F1 restore: write only changed files, re-seed `index_cache`, single dirty scan (fixes undo, redo, switch, restore, stash pop, clone checkout).
2. F10: TTY check for interactive resolve, shorter lock scope, bounded lock wait.
3. F3 diff hash short-circuit and F4 parallel grep (small, local).
4. F5 parallel `import_pack` object loop.
5. F2/F9 `file_map` insert-select, index review, binary hashes; plan a tree-object model before repos exceed about 10k files.
6. F6-F8 polish.

## Not verified

No profiler or timing was run and no file was modified. The cache re-seed explanation for redo, the `file_map` insert cost behind F2, the per-object write cost in F5 and the origin of the 4 lock hits are inferred from code and data. Confirm with a trace (sampling profiler on `velo redo` and `velo bundle apply`; Process Monitor for file-op counts). git baselines are unreliable for gc, fsck and initial save.
