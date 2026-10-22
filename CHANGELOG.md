# Changelog

Notable changes to Velo. Repository **format** changes are called out separately
from API changes, because they are the ones that can make existing data
unreadable. The normative format spec is [`docs/FORMAT.md`](docs/FORMAT.md).

This file starts at the format v2 break. Earlier releases are in the git history.

## 6.2.0

Phase 22: put it in front of a stranger.

### Added

- **22.2-a Dated name availability table and distribution-name recommendation.** Added docs/NAMES.md, dated 2026-10-22, with live registry checks and the URL used per row. It recommends PyPI velo-core and unscoped npm velo-core, with velo-timeline as the fallback, and lists the cost of each option.

### Changed

- **22.0-a Open 6.2.0: workspace and lockfile versions, semver baseline v6.1.0.** Workspace version and the four internal deps are now 6.2.0. The four lockfiles show only velo-* version lines. The semver baseline is v6.1.0. `cargo build --workspace --locked` passes. The workspace tests passed, except one flaky test noted below.
- **22.2-b Apply the distribution names and align binding versions with the workspace.** Fixed all four review items. The node lockfile and index.js are regenerated at 6.2.0, and the packages workflow now strips CR when reading the workspace version.

### Fixed

- **22.0-b Fix the README fit table against the code.** Rewrote the regulated audit trails row of the README fit table to describe shipped Ed25519 signed snapshots and bundles, fsck tamper detection, and what is still missing. The other rows checked out against the code and were left alone.

## 6.1.0

Phase 21: fix what the team simulation found, then re-measure.

### Changed

- **21.6-a `push` publishes tags whose snapshot the remote already has, and says so.** The receiver appends the names of the tags it inserted to the OK line after the count. `LocalRemote` returns the same list, and `PushReport.tags` lists only names that were both offered and inserted. The names reach `push` through a new provided method, `Remote::published_tags`, so `PushOutcome::Ok` keeps its fields and the change is not breaking.

### Fixed

- **21.10-c Fix the dominant cost of clone over HTTP, or write the reason.** `bundle::import` now calls `timing::begin()` at its start and laps a new `import.verify` stage before `import.objects`, so pull, push and serve rows no longer count earlier work as `import.objects`. PERF.md notes the new lap.
- **21.1-a MERGE_HEAD records the resolved source snapshot; save, abort and status read it.** MERGE_HEAD now stores `<pre>:<source>:<source-id>`, written and parsed only in the new `commands/merge_head.rs`. Save and `merge --abort` use the stored id. The old two-field form is still read and re-resolved. The repro test fails without the save.rs change and passes with it.
- **21.2-a redo refuses with a typed error when the branch moved since the undo.** The stray `#[cfg(test)]` at tests.rs:26 was already gone, so no new commit was needed for it.
- **21.3-a switch, whole-tree restore and stash pop refuse while a merge or rebase is in progress.** Added `commands::refuse_in_progress` (rebase named first, then merge), called from `switch::run` unless forced, from `restore::run` for whole-tree restores without force, and from `stash::pop`.
- **21.4-c fsck: an unreachable orphan delta chain is cruft that --repair clears.** The undo-heavy reproduction was a gap in save, not undo. A new save deleted the branch's trash rows but left their stored delta trees behind, so an orphan chain stayed whose base was itself an unreferenced tree. `save` now removes those trees with the fold in the same transaction. The new test `orphan_delta_repro` replays Redo, Undo, Edit, Save on the format-v4 golden at K=1 and asserts fsck reports no problems and no cruft.
- **21.5-a Push refusal states what is true; the reconcile recipe follows the repository state, with `velo sync` first.** A refused push now says "origin/main is at <id>, not fetched yet; velo sync ..." and mentions undo only when the id is in the trash. The recipe takes a `RecipeState` and has no `velo pull` step.
- **21.7-a The HTTP push refusal names the token variable and the server option.** A refused push now names `VELO_TOKEN_<REMOTE>` and, when no token was sent, also `VELO_SERVE_TOKEN` and `--token-file`. When a token was sent, it says the token was rejected and never prints the value. HTTP statuses, refusal kinds and the exit code are unchanged.
- **21.8-a resolve, rebase --continue and merge say what is actually wrong.** Resolving an already-resolved path says "not in conflict", `rebase --continue` before save says to run `velo save` (exit 1), merging an unborn branch returns `UnbornBranch`, and merging into an unborn branch states that it adopts the source's history.
- **21.8-b cherry-pick counts, merge summary after mv, conflicts context and blame past EOF.** A cherry-pick of a change already present no longer counts as Updated or Auto-merged and prints only "Nothing to apply". The conflicts text shows context under both sides, blame past EOF prints "file has N lines" on stderr, and the merge summary after mv plus an edit now says New, as status does.
- **21.11-a Blame's walk is correct after merge, fast-forward, rebase, cherry-pick and take-theirs.** The oracle warnings come from its parsing plus by-design replay authorship. The CLI now prints `(unknown)` in an empty author cell.

### Added

- **21.9-a Accept the cheap git spellings and make every git-habit refusal name the velo spelling.** `stash push -m` and plain `HEAD` are now accepted. `HEAD` resolves to the current position and prints a stderr note. `ref:path` hints now cover restore, diff, history and blame as well as show, and `switch <new>` prints "Created branch" and mentions `--create`.
- **21.9-b One exit-code rule: not-found is 1, nothing-to-do is 0.** Added typed `NothingToUndo` and `NothingToRedo` errors, mirrored in the Python, Node and FFI bindings. The CLI prints them as plain text and exits 0. The switch near-miss refusal now exits 1. The README exit-code table and the `--help` block state the rule.
- **21.4-a Differential harness: a Pull op, an undo-heavy mix, and pinned undo/pull/redo sequences.** Added `Op::Pull`, a `Mix::undo_heavy` switch, the `VELO_DIFF_MIX=undo` knob and a second nightly step, with tests `undo_pull_redo_sequence`, `undo_heavy_smoke` and `default_mix_op_list_is_unchanged`.
- **21.4-b Undo-heavy differential runs.** The docs give a 25-slice loop, one hour per slice, each with a distinct `VELO_DIFF_SEED`, and say how to total the sequence count.
- **21.10-a A committed harness for the team run's over-target rows.** Added `team_rows_profile.rs`. An ignored `profile_team_rows` measures clone (path and HTTP), bundle, grep, stash, blame, push/pull (path and HTTP) and 4 parallel writers, with timing laps and JSON output. A smoke test, `team_rows_run_at_a_small_size`, runs every row in the normal suite.
- **21.12-a CI: the semver baseline is now v6.0.1.** The stale comment in `ci.yml` is gone.
- **21.12-b Re-measure the Linux 500-step agent run and fix the <worktree> placeholder in PERF.md.** The sandbox refused every wsl call, so the 500-step Linux run was not made. `docs/PERF.md` now says so and gives the repo-root owner command; no numbers were invented.

## 6.0.1

### Fixed

- **A file rewritten within one timestamp tick could be saved with its old content.** The stat cache trusted `(mtime_ns, size)` alone, so two same-size writes that landed in one filesystem tick (the second after the cache row was written) looked unchanged and `save` stored the first content. Cache rows are now written only for files whose mtime is more than 2 s older than the write (git's racy-clean rule); a fresher file has its row dropped and is re-hashed on the next scan. The on-disk format is unchanged, and a freshly restored or just-saved file now costs one re-hash on the next scan. Surfaced by `every_point_leaves_old_or_new` failing about half the runs on ext4.

### Security

- **Lockfile bump for RUSTSEC-2026-0382 and RUSTSEC-2026-0381.** rustls-webpki 0.103.16 and webpki-roots 1.0.10 reject the distrusted emSign intermediates. Lockfile only.

## 6.0.0

Phase 20: format v4 (delta snapshots): the fixture, differential harness, tree accessor, schema, delta write path, fsck and fold primitive.

### Added

- **20.3-a Fix the v4 storage schema normatively in FORMAT.md.** Added FORMAT.md section 11.6, the normative v4 storage spec, and updated the v4 row in section 7.1 and the table and index lists in section 7.2.

- **20.1-a Independent v4 fixture written from the spec by a stdlib Python script.** Trash deleted_at_ms now rises in undo order: 13 gets ...100000 and 12 gets ...100001, so redo picks 12 first. The undo ops carry the same values. Regenerated velo.db and expected/format-v4.json.

- **20.1-b Seeded differential harness: reference repo vs subject repo, asserted after every operation.** Added a per-handle cfg(test) clock (Repo::now_ms) used by every core snapshot and trash stamp site, and a seeded differential harness in tests/differential.rs. It compares ids, trees, history, blame, status and fsck after every op, with shrinking on failure. It passes against itself.

- **20.2-a tree_store module and the single-snapshot readers.** Added tree_store.rs with tree_of, file_at, files_at, has_tree and tree_with_index_cache. Moved every listed single-snapshot file_map read behind it, with timing lap names unchanged. Added 6 direct tests, including the 499/500 batch boundary.

- **20.2-b tree_store: whole-table scans and path-keyed probes (fsck, gc, history, blame).** tree_store now holds the scans and path probes (for_each_tree_in_order, referenced_objects, delete_orphan_trees, prune_stale_index_cache, duplicate_rows, remove_duplicate_rows, changed_under, PathVersions, changes_of). fsck, gc, history and blame build no SQL naming file_map. The SQL was moved, not rewritten.

- **20.2-c tree_store: every tree write, delete and re-key, and bundle export/import.** Added write_tree, delete_trees, rekey_tree, tree_rows_for_export and a first_parent helper to tree_store. Migrated save, tree, squash, stash, rebase, compact and bundle so none of them names file_map in SQL.

- **20.2-d Lint: no SQL outside tree_store names a tree table; audit the tables that hold trees.** Added a source-scan lint that fails when a string literal outside tree_store.rs, db.rs and the tests names file_map. The audit table is in the tree_store docs and FORMAT.md, and the gc text now says "tree rows".

- **20.0-a Open 6.0.0: versions, lockfiles, README tag line, semver baseline.** Workspace and internal deps are now 6.0.0, with the root and three binding lockfiles refreshed. STABILITY.md names 6.0 as the format-v4 release, and ci.yml has a comment saying the baseline moves to v5.3.0 once it is tagged.

- **20.3-b Schema v4, O(1) migration, and delta-aware single-snapshot reads.** The format is now v4: the schema has both delta tables and indexes, and FORMAT_VERSION is 4. The six single-snapshot readers in tree_store.rs reconstruct delta snapshots, and a bad chain gives Error::Corrupt instead of hanging. Migration is O(1), and the spec-written format-v4 fixture reads back through a new corpus test.

- **20.3-c Delta-aware scans and probes: fsck stream, gc references, history and blame paths.** tree_store scans and probes are now delta-aware: referenced_objects includes delta-row objects (gc keeps them), for_each_tree_in_order streams in chain order and returns unreached deltas, delete_orphan_trees never removes a base a remaining delta needs, and PathVersions, changed_under and changes_of resolve through chains. Seven tests are in storage_v4.rs and one fixture test is in corpus.rs; the full velo-cli and velo-core gate is green.

- **20.4-a Delta write path for every local producer, K knob, and the never-orphan delete guard.** save, save_tree, save_changes, amend, stash and squash now write canonical deltas against the first parent, with a keyframe at depth K, and the K knob and the never-orphan delete guard are in. Compaction materializes dependants first, bundle export reconstructs delta trees, and the differential smoke test compares K=1 against K=2, 3 and 128.

- **20.4-b Import writes deltas: bundle apply, clone, fetch, pull; adopt or replace, never duplicate.** No code change was needed. The velo-core suite passes on origin/phase-20 once the workspace builds the sibling velo binary.

- **20.4-c Regenerate the v4 fixture with the real writer and compare it with the independent one.** Added `the_real_writer_reproduces_the_spec_written_v4_fixture`. It replays the fixture's ops through the real engine at K=4 and compares every table the ops control against the fixture. The real writer and the spec-written fixture agree on all of them, so there was no spec bug and no implementation bug.

- **20.3-d CI: an old release refuses v4, and the bundle way back to 5.x works.** Replaced the raw 0xB7 byte in the ci.yml job name on line 363 with UTF-8 C2 B7. The whole file now decodes as UTF-8, and only that one line changed.

- **20.5-a fsck verifies delta snapshots by reconstruction; new checks and repairs.** Repair now removes the IdMismatch, DeltaCycle, DeltaBaseMissing and DeltaBaseNotFirstParent findings for each snapshot where it drops the delta. It also counts that snapshot in ids_verified. The test now asserts the repair report itself is healthy.

- **20.5-b Measure fsck on v4 against 5.3 at growing history sizes.** Added the ignored fsck_v4_scale harness (real save_tree history, K via set_keyframe_interval, healthy assert, JSON output) plus a small non-ignored smoke test. Measured six runs, committed the raw lines, and added a PERF.md subsection: K=128 fsck was faster than K=1 at every size, so there is no regression.

- **20.6-a The fold primitive, its property test, and gc/trash using it.** The undo/save/gc/redo test now asserts that undo succeeds and that redo restores the trashed id with its recorded tree. Unchecked tree reads now return Corrupt for a base that is unknown and has no rows. gc leaves such an orphan alone, and a new test covers it.

- **20.6-b Compaction, retention, squash and amend fold through the primitive; harness with removals.** compaction.rs (16 tests) and retention_scale.rs (the non-ignored test) now run at K=3 through `setup_k` and an `at_k3!` wrapper per test. compaction_in_place also gets `a_dry_run_writes_nothing_at_k3`. `cargo test -p velo-core` and `cargo test -p velo-cli` were both run on the final tree and pass.

- **20.7-a Failpoints for delta write, keyframe write, fold and migration; crash scenarios on v4.** Added the four failpoints, and db::migrate now runs the schema script and the version stamp in one transaction. crash.rs runs every scenario at the production K and at K=3, plus new compact_fold and migrate_v3 scenarios. All pass on Windows with no new known window.

- **20.7-b Hostile delta tables are typed errors; a cargo-fuzz target for the stored table.** Delta rows now decode through one shared helper, and a hash/mode NULL mismatch is Corrupt in every reader of tree_delta_rows. descend no longer swallows row errors. PathVersions::at returns Result, and tree_at enforces max_string_bytes. The delta_table fuzz target is added with its seeds, README line and CI matrix entry, plus 9 tests in hostile_deltas.rs.

- **20.8-a repack core: convert existing keyframes to deltas in short, resumable transactions.** Added commands::repack::run, which converts keyframes to canonical deltas in per-chunk write-locked transactions, re-chunks overlong chains, and finishes with an incremental vacuum. It has a dry run, cancellation between chunks and the repack.after_chunk failpoint. A repack crash scenario and 7 unit tests pass.

- **20.8-b velo repack CLI, Python/Node bindings, MCP tool, parity.** Added the missing MCP test for velo_repack in recovery_tools.rs. It covers dry_run true, false, omitted and an invalid value, and checks that all eight result fields are present.

- **20.11-a Cold-start test kit: protocol, task cards, independent checker, friction log.** Added bench/coldstart with the protocol README, four task cards, stdlib check.py and an empty friction log with an Adapter decision section. check.py ran against a repo made by the local velo: PASS for the real actor, FAIL for a wrong one.

- **20.12-a Nightly differential run with fresh seeds, starting from every migrated golden.** Added Start::{Empty,Golden} to the differential Config, copying the golden into the TempDir before open_and_migrate. Added the non-ignored differential_from_goldens, made differential_long cover all starts and rotating Ks with a failure file, and added nightly.yml.

- **20.12-b Mixed-version CI: 5.x client against 6.0 server and remote, and the reverse.** Added tests/compat/mixed_versions.py with four cells: HTTP in both directions, a new client against a v3 path remote, and an old client refused by a v4 path remote. Its README has the 2x2 table, and CI runs it as a new step in the old-release job. All four cells passed in one local run on Windows.

- **20.10-b (Conditional) Opt-in group commit for agent loops: measured, not built.** Ran profile_agent_workload for 200 steps on Windows and on WSL2 ext4, and committed both raw JSON files with the exact commands. The Linux v4 commit stage is 0.090 ms of a 4.97 ms median step, so the batch bound is 1.6% to 1.8% and the "not built" verdict holds on both platforms.

## 5.3.0

Phase 19: profiling, `save_changes`, batch reads and `materialize`, shrinking retention and `velo retention`, the snapshot-storage decision (FORMAT.md 11.5), Windows first-run cost, `velo report`, and release-artifact packaging.

### Added

- **19.0-a Open the 5.3.0 release.** Bumped the workspace and the velo-* dependency versions to 5.3.0 and updated the four lockfiles, touching velo-* entries only. Updated the README tag line and pointed the semver job's baseline-rev at v5.2.0.

- **19.1-a Stage timers behind a `timing` feature in velo-core.** Added the timing module and `timing` feature to velo-core. It has the exact lap names in the specified order for save, save_tree, restore, compact and retention. Added 5 tests, a CI step and a STABILITY.md line.

- **19.1-b Profiling harness: save, save_tree, restore and the agent workload.** Added the perf_support and perf_profile test modules. They give two ignored measurement tests (profile_save_restore, profile_agent_workload) that emit JSON lines, plus non-ignored small-size tests so the harness cannot rot. No production code changed.

- **19.1-c SQLite option experiments at the 100,000-file point.** The schema_without_rowid and schema_blob_hashes lines now carry delta_ms, delta_pct, delta_bytes_per_row and delta_bytes_pct against schema_rowid_two_indexes. That baseline line carries zero deltas. The small run asserts the fields.

- **19.1-d Run the profile matrix and publish docs/PERF.md.** Ran the profile matrix on Windows and WSL2 Linux and published docs/PERF.md with the stage tables, top costs, agent split, SQLite deltas, derived targets and a not-measured list. Raw JSON lines and the reproduce commands are committed. Binding overhead is 10.5% of the step time on Windows and 5.6% on Linux.

- **19.2-a `WriteGuard::save_changes`: save a parent plus changes, return ids.** save_changes now refuses a Rename whose source was already renamed or deleted in the call, even after a re-Put. It also refuses a Delete of, or later Rename onto, a path that an earlier Rename targeted. The show test asserts the rename edge exactly.

- **19.2-b Bind save_changes in Python and Node.** Python Repo.save_changes (dict or velo.Change list, returns Saved) and Node saveChanges (ChangeInput ops, returns SavedInfo) are bound, and the parity exemption is removed. The example, cookbook and READMEs are updated.

- **19.2-c MCP: velo_save_tree gains a changes form and returns object ids.** On the files route, null paths are checked against the parent with one indexed lookup each, using a new Repo::object_at. Paths the parent lacks are dropped, as the old tree.remove did, and the tree is still never loaded. A new store_only test covers a null for a missing path.

- **19.2-d Re-run the agent benchmark with save_changes and materialize.** Fixed the three wrong cookbook factors (2.6x, 5.3x, 13.8x) and added disk factors to the Disk bullet. Reworded the PERF.md Linux sentence so the single-run point is a possible explanation and the 37.4 s vs 39.1 s comparison is marked cross-day and indicative.

- **19.3-a `Repo::read_files_at`: batch reads in one result.** Added Repo::read_files_at in tree.rs: snapshot check, then one file_map statement per chunk of paths, then parallel object reads through par_with_content, with results in input order. Added 7 tests and a pending parity exemption.

- **19.3-b `commands::restore::materialize`: write a snapshot into any directory.** canonical_prefix now canonicalises the longest existing prefix of the absolute path and resolves the missing tail by text. A `..` pops the last tail component or the canonical base, and a raw path is never returned. This closes the Unix hole where `<root>/sub/..` skipped the root and .velo refusals.

- **19.3-c Bind read_files_at and materialize in Python, Node and MCP.** Python and Node now have read_files_at and materialize, and MCP has the velo_read_files store tool with a total byte budget and an omitted list. Both pending exemptions are removed and the parity check passes. Python pytest (64), the Python stubtest, the Node batch_reads tests (4) and the velo-mcp tests all pass.

- **19.4-a Make compaction and retention fast: re-mint in place, batch the deletes.** The golden test now calls .replace("\r\n", "\n") with real escape sequences, checked by od -c. I also added a *.golden eol=lf rule to .gitattributes.

- **19.4-b Return freed pages: incremental auto_vacuum for new repositories, `gc::vacuum`.** New repositories set auto_vacuum=INCREMENTAL in create_at_path. Added gc::vacuum with Vacuum, AutoVacuum and Vacuumed. FORMAT.md 7.4 records the decision and the parity exemption is added.

- **19.4-c `velo retention` command, and `velo gc --vacuum`.** Added `velo retention` (--branch, repeatable --keep AGE=EVERY, --dry-run, --keep-days) and `velo gc --vacuum`. A plain `velo gc` now also runs an incremental vacuum, and `--dry-run --vacuum` is rejected. README and the cookbook are updated.

- **19.4-d Bind gc::vacuum in Python and Node.** Added repo.vacuum to the Python and Node bindings (dict and VacuumInfo with bytes and auto_vacuum). Removed the parity exemption; the parity check passes, and the Python (54) and Node (46) suites pass.

- **19.4-e Repeat the 18.10 retention run and publish it.** Repeated the 18.10 run on Windows: velo.db went from 5.18 GiB to 2.06 GiB. Retention took 415.6 s (18.10: 1,219.5 s), gc 165.2 s, vacuum 192.5 s. The first run did not shrink the file because gc::vacuum had a bug, which I fixed and then re-ran.

- **19.5-a Prototype parent deltas with keyframes and measure them.** Added the delta/keyframe prototype on scratch tables, with a non-ignored correctness test (tree and real ids match for K 1, 4, 16) and the ignored matrix. Ran it at 100,000 files on Windows and WSL2, committed the JSON, and added PERF.md section 8. A one-file save is about 83-110 ms under A, against 1.75 s (Linux) and 5.3 s (Windows) today, and all of it is identity (parent-tree read plus hash). Disk per snapshot falls from 27.9 MB to 2.3 MB at K=8 and 36 KB at K=512.

- **19.5-b Record the snapshot-storage decision in FORMAT.md.** Added FORMAT.md 11.5 choosing option A (parent deltas, keyframe every K=128, identity unchanged), with the prototype numbers, migration, wire stance and the compaction fold rule. Also added the planned v4 row to 7.1, updated the Non-goals bullet, and pointed 11.4's closing at 11.5.

- **19.6-a Count filesystem operations per save and remove the redundant ones.** Added the six fs.* counters on the save path and an ignored fs_ops_per_save report. A first save now opens each new file once: the scan no longer hashes untracked files, and save seeds their cache rows after the commit. Per file, first save stat went 5.02 to 2.01, exists 2.00 to 1.00 and open_read 2.00 to 1.00. Clean status stat went 2.01 to 1.01.

- **19.6-b `velo init --objects` and a matching scale.py flag.** velo init takes --objects files|database (clap ValueEnum, default files) and calls init::run_with. scale.py has --objects, passes it to init and records it in env. READMEs updated.

- **19.6-c Measure Windows first-run cost and write docs/WINDOWS.md.** Measured Windows first save, gc and clean status at 10k and 100k files for defaults and --objects database, wrote docs/WINDOWS.md, and linked it from README and SCALE.md. The Defender exclusion was not measured because the account is not an administrator.

- **19.7-a A performance guard in CI with calibrated ratios and committed baselines.** Replaced the raw 0xB7 byte on line 245 of ci.yml with the UTF-8 middle dot (0xC2 0xB7). iconv -f utf-8 now accepts the whole file.

- **19.8-a Core report model: `commands::report::build`.** Added commands::report::build and the non_exhaustive report model types, with the module doc stating that strings are verbatim and escaping is the renderer's job. Four tests cover the model, and the parity exemption is in place.

- **19.8-b `velo report`: self-contained HTML and JSON, hostile input escaped.** Added `velo report`. It writes a self-contained offline HTML file with a strict CSP, no script element and every history string escaped, or the same data as JSON with `--json`. A proptest and hostile-corpus test of the renderer, a black-box test file and the README entry are included.

- **19.8-c The cookbook's agent run ends with a report; README links a sample.** Added report_cookbook.rs, which rebuilds the cookbook scenario and asserts app.py line authors (agent-run-1 vs human) plus the fsck verdict. Committed a reproducible docs/report-sample.html, a cookbook section, and a README link.

- **19.10-a Attach wheels and Node tarballs, with checksums, to GitHub releases.** packages.yml is now reusable and names Node tarballs per platform. release.yml gains bindings and attach-bindings jobs that attach 3 wheels, 3 tarballs and SHA256SUMS-bindings.txt. The README documents pip, npm, cargo --tag and checksum verification.

- **19.10-b Generated API reference for the Python and Node bindings.** Added branches: ["**"] under on.push in apidocs.yml so branch pushes touching bindings/** trigger it. Tag trigger is unchanged.

- **19.10-c A tested serve-http deployment example: reverse proxy, tokens, read-only mirror.** Added examples/serve-http (Caddy-fronted compose stack, read-only mirror compose, tokens.example, README, smoke.sh) and the deploy-example CI workflow; README hosting section links it. No Rust changed, so cargo checks were skipped.

- **19.11-a Re-run the benchmarks against the targets and report the losses.** Target 3 now reports disk after 500 steps (152 MB, 41x Linux, 47x Windows) in PERF.md and the cookbook, with final disk labelled separately. Docs only.

## 5.2.0

Phase 18: hostile-input hardening, crash safety, a format-compatibility corpus, an authenticated HTTP remote, signed snapshots, `velo sync`, and binding/MCP/WASM work. Public API changes in `velo-core` are additive; one additive table (`signatures`); bundle wire version 4 is written only when a bundle carries signatures.

### Added

- **Open 5.2.0** Bumped the workspace to 5.2.0 (`Cargo.toml`, the four lockfiles, the README tag line) with an empty 5.2.0 section here.

- **One path rule: unsafe tree paths are refused at ingestion and before any working-tree write.** `safe_path` (store rule, platform rule, tree exclusivity, symlink checks, `join_checked`) and `Error::UnsafePath`, FFI code 26. Enforced in `save_tree` and bundle import before any object is stored; restore, apply, stash, abort and resolve validate the whole target before writing. `FORMAT.md` 3.1 documents it.

- **Read limits on the handle.** `Limits` and `Limit`, `Repo::limited`, `Repo::limits`, `Error::LimitExceeded` and `VELO_ERR_LIMIT_EXCEEDED = 27`. Bundle decode and apply, bounded zstd decompression of received frames, the wire readers, serve receive and git import are bounded; `FORMAT.md` 9 documents the ceilings.

- **Fuzzing.** Six `cargo-fuzz` targets (bundles, wire, HTTP service, git import, paths, ids), run in CI.

- **Crash safety.** A `failpoints` feature with named crash points and a self-exec crash harness covering save, restore, switch, undo, redo, merge, `merge --abort`, rebase, gc, compact, retention, bundle apply and clone, with a `crash` CI job on Linux and Windows. It found one real window in `undo`, now fixed, and `continue` now refuses with `DirtyWorkingTree` when no merge is pending and the tree is dirty.

- **Golden corpus.** Repositories and bundles written by released builds, including signed, bad-signature and unknown-algorithm v4 bundles; a corpus test and CI job assert that every released format opens, migrates or refuses.

- **Hosted HTTP remote.** `serve::http` (`Service`, `Request`, `Response`, `Operation`, `Access`, `Authorize`, `Tokens`): constant-time bearer auth or a host hook, read-only mode, request and object limits, directory mode with traversal refused (`FORMAT.md` 9.1). The client takes tokens from the caller, never the URL, and reports typed `AccessDenied` and limit refusals. `velo serve-http` adds tokens, read-only, limits, connections and directory serving.

- **Signed snapshots.** `velo_core::sign` over an additive `signatures` table; `fsck` reports `Problem::BadSignature` and `Cruft::OrphanSignatures`. Bundles and sync packs carry signatures as wire v4, only when present (unsigned bundles stay v3); import verifies every row first and refuses the whole import with `UntrustedData` on a bad one. `compact`, retention, `gc`, `squash`, `rebase` and `save --amend` leave signed snapshots alone (`Protected::Signed`). CLI: `velo keygen`, `velo sign`, `velo verify` and a `[signed]` marker in `history` and `show`.

- **`velo sync`.** `commands::sync::sync` returning `Synced`, and `velo sync [REMOTE] [BRANCH]`: a clean diverged merge is saved and pushed, a conflict stops where `velo merge` does (exit 3). The push, pull and status recipe now leads with `velo sync`. Git-habit hints for `show`, `history`, `stash push -m`, `grep -n` and `diff --stat`, and `switch --create`/`-c` with a near-miss refusal.

- **MCP.** Store-only mode (`save_tree`, `tree_at`, `read_file`), recovery tools (`velo_undo`, `velo_redo`, `velo_tag`, `velo_conflicts`, `velo_find`, blame line ranges), a busy repository that names its holder, `velo_history` paging by `offset`/`next_offset` (the `next_from` cursor is gone), conformance runs with real clients in CI, and `velo-mcp --http ADDR` (Streamable HTTP with bearer tokens, Origin checks and a non-loopback bind guard).

- **Bindings.** Python and Node bind retention, compaction, tags, `fsck`, change events, merge drivers, signing and `sync`, with a re-entrancy guard and documented threading (Python releases the GIL on long calls). Every core `Error` variant has its own Python exception class and Node code; `py.typed`, a stubtest check and a generated `index.d.ts` drift check are in CI, and a parity check requires every core entry point to be bound or exempted with a reason. `packages.yml` builds wheels and Node prebuilds as artifacts and install-tests them without a Rust toolchain.

- **WASM.** Headless Chrome and Firefox tests in CI, an `fsck` binding, persistent reopen, a Playwright page-reload test, and a release profile that cut the gzip size from 1,267,189 to 863,812 bytes with a 950,000-byte CI budget. There is no IndexedDB fallback.

- **Scale and benchmarks.** `bench/scale/scale.py` and a `workflow_dispatch` benchmark workflow, the measured scale envelope and retention recommendation, `file_map` measurements in `FORMAT.md` 11.4 (carry-forward saves under 3% of a 100k-file save, so it is not adopted), and a six-system agent-checkpointing benchmark with committed Windows and Linux results.

- **API stability and supply chain.** A blocking `semver` job (`cargo-semver-checks` on `velo-core` and `velo-merge` against `v5.1.2`), `deny` and `audit` jobs with `deny.toml`, `forbid(unsafe_code)` or justified `allow` with `SAFETY` comments in every crate, an SBOM attached to release artifacts, a reproducible-build check, `SECURITY.md` and `docs/THREAT_MODEL.md`.

## 5.1.2

Phase 17: fixes for what the third team simulation found. No repository
format change; public API changes in `velo-core` are additive.

### Fixed

- **`fetch` no longer doubles an undone snapshot's tree, and `merge` refuses a
  tracking ref whose snapshot is gone.** Import clears leftover file rows before
  rewriting them, `redo` adopts a snapshot a fetch brought back, and `fsck`
  repairs duplicate rows and says to re-fetch on a tracking mismatch. A merge of
  an absent ref is a typed error instead of a parentless merge.

- **`rebase` skips snapshots the target already reaches through a merge's second
  parent**, so it no longer replays them as spurious conflicts.

- **`merge --abort` restores the exact bytes of files with CR line endings**
  (saved in a transient `.velo/MERGE_ORIG`), and `resolve` keeps a CRLF file CRLF
  when it writes the resolution.

- **`resolve --take` no longer leaks the other side's lines** when the ancestor
  ends in a blank line. It reports the conflicts taken and the clean lines kept,
  and `--mark` counts ignore CR.

- **`push` names the side branches its history publishes**, `bundle apply` counts
  the tags it imports, and squash help and output say the originals are not
  restorable.

### Performance

- Pack import writes objects serially after parallel verification; `fetch` with
  nothing new no longer scans the database; `init` opens the database once.
- Working-tree `grep` walks without per-file metadata; `undo` and `restore` reuse
  the dirty scan's walk; `history --file` answers from one path lookup.
- `fsck` and `gc` read file rows once and hash on the pool; lock-wait backoff is
  a pure schedule.

## 5.1.1

Phase 16: fixes for what the second team simulation found. No repository
format change; public API changes in `velo-core` are additive.

### Fixed

- **`undo` no longer orphans another branch's history.** It refuses, naming the
  branches (or the stash shelf) that build on the snapshot, instead of leaving
  them with a missing parent that fails `fsck`. A repository already damaged
  that way is recoverable with `velo redo`: `fsck` says so, and `save` and `gc`
  keep the trash row such a snapshot needs.

- **`resolve --mark` no longer drops the other side's clean changes.** A hand
  edit that is missing lines the other side added outside every conflict is
  refused, listing the lines; `--mark --force` accepts it and says what was
  dropped. `velo conflicts` (text and `--json`) shows how many clean changes are
  missing from each file's working copy.

- **A merge of a diverged tip with equal content records a merge.** It no longer
  prints "Already up to date" and leaves the branch diverged, so the push recipe
  ends in a push. "Already up to date" is reserved for a tip that is an ancestor.

- **Merges follow moves.** A file we moved and they edited is merged at its new
  path, and the reverse; neither resurrects the old path, and modify/delete is a
  conflict. `merge --abort` removes the files the merge added and the
  directories that leaves empty.

- **A fast-forward merge keeps the feature's history.** The snapshot it records
  has the target as its merge parent and carries the target's author, so
  `history`, `blame` and the ahead/behind counts see the feature's snapshots. The merge base after a
  fast-forward is the feature tip, whatever the ids.

- **`remotes/<remote>/<branch>` and `<remote>/<branch>` mean the same snapshot.**
  Both resolve through the remote-tracking refs, and `history --all` labels
  remote tips from them.

- **`switch` to the current branch repositions a stale working position**, such
  as the empty one `bundle apply` leaves, with the usual guard for unsaved work.

## 5.1.0

Phase 15: what the first team simulation found. Fixes for silent content loss
in `save`, amend, `resolve` and stash; `resolve` that works without a terminal;
lock holders that are named and waited for; faster restore, import, grep, save
and merge; distinct exit codes and one reconcile recipe. No repository format
change; new public API in `velo-core` is additive.

### Added

- **Interactive resolve takes the lock per decision; core revalidates the
  conflict before writing.** The busy notice is now a status value that `draw()`
  prints under the header after clearing the screen, so it is visible on a TTY.
  It is dropped after the next key. A busy finalise says that pressing a key
  retries the write.

- **CLI: bounded lock wait with jittered backoff, and the holder is named.**
  Mutating commands wait up to 3 s (`VELO_LOCK_WAIT_MS` overrides) with 10-50 ms
  jittered sleeps before reporting `Locked`. The hint gives the holder's age and
  whether its pid is alive. The `Repo` is opened with a "velo ARGS" lock label.

- **CLI: distinct exit codes, one reconcile recipe everywhere, merge-aware
  status.** The mid-merge status test now uses a diverged, fetched clone (pair,
  pull exits 4, merge origin/main exits 3) and asserts "to finish merging" is
  present and `velo pull` is absent. Test-only change.

- **Discoverability.** `log` and `-n` aliases, git-habit hints for unknown
  subcommands (exit code stays 2), and `history --branch origin/main` /
  `remotes/origin/main`. Undo says "Working files rewound to", and "Nothing to
  save" after an undo hints `velo redo` via the read-only `redo::peek`.
  Duplicated `[default: N]` help text is removed and the `mv` Examples block
  keeps its line breaks.

- **`gc --dry-run`.** gc commits the prune transaction right after the
  `referenced` query (a dry run drops it, which rolls it back). The commit after
  the chunk pass is gone, so a real run makes its prunes durable before it
  deletes any file.

- **Warning before rewriting pushed history.** `sync::published` and a CLI
  stderr warning for `save --amend`, `undo`, `squash` and `rebase` when the
  replaced snapshots are already in a remote-tracking ref. The command still
  runs and exits normally.

- **Every `velo ...` command quoted in help text is tested to parse.**
  `help_check.rs` walks each subcommand's long help, extracts quoted
  invocations and parses each with clap, with a self-test that a bad example is
  rejected. Three help lines it caught were fixed in `main.rs`.

- **`resolve --take both`, `resolve --mark` and `velo conflicts [file] [--json]`.**
  Core gains `TakeOption::Both` (ours then theirs, `Decision::BothOursFirst`;
  `TakeOption` is now `#[non_exhaustive]`), `resolve::mark` and
  `resolve::merge_source`. `mark` accepts a hand-edited file and refuses when it
  is missing, equals ours or theirs, or still has conflict markers. `conflicts`
  is read-only and prints both sides of each hunk. The help no longer promises a
  sidecar file or `diff --conflict`, and the hints, the TTY refusal text and the
  README are corrected.

- **`restore` writes only paths that differ from the tree being left**, and
  scans for dirty files once per command. A path is skipped only when `PARENT`
  has the same hash and mode, it is not dirty and its on-disk mode matches.
  `undo`, `redo` and `switch` pass their scan in. A property test holds the
  result byte-identical to rewriting everything.

- **Import is faster.** `bundle apply`, `clone` and fetch check for a held
  object before decoding it, verify the rest and the snapshot ids in parallel,
  and no longer rebuild a tree per snapshot. Errors and ids are unchanged.

- **`save` reuses the dirty scan's hashes**, so each new object is read once,
  and the scan loads the snapshot file map and index cache in one joined query.

- **`merge` apply is parallel.** Deletes run first, then the merged and taken
  files are written in parallel, so a file can become a directory.

- **`file_map` growth is documented.** `docs/FORMAT.md` 11.4 lists who reads
  `idx_filemap_path` and `idx_filemap_snap`; no format change.

- **`resolve --take` refuses to replace hand edits.** Core checks every file
  first and returns the typed `WouldOverwriteEdits` (paths sorted, nothing
  written) unless the caller passes `OnEdits::Replace`. The CLI's `--force`
  replaces, prints the replaced and discarded line counts, and says that "ours"
  means your last saved snapshot.

- **`diff a..b` and the worktree diff skip hash-equal paths** without reading
  the object store, so the cost follows what changed, not the tree size.

## 5.0.0

Phase 14: velo as a timeline engine. Language bindings (C, Python, Node, WASM),
an MCP server, pluggable merge drivers, metadata queries, change events,
chunked storage, compaction and retention, HTTP sync, git interchange and
single-file repositories. A repository that stores a chunked object becomes
format v3, which 4.0.0 refuses, and `velo-merge`'s public surface changed
(`MergeDriver`); both are breaking, hence a major version. See the format
section below.

### Added

- **The Node binding covers history, blame, merge and branches.** `history`
  (from, branch, all, paths, limit, meta), `findSnapshots`, `blame` (with
  `lineCount` and a 1-based inclusive window), `mergeBase`, `mergePlan`,
  `mergeCommit`, `branches`, `createBranch` and `setBranchTip`. A conflicted
  `mergeCommit` with no resolutions rejects with `code: 'Conflicts'` and
  `err.paths`. Every method is async.

- **`velo-wasm`, a wasm-bindgen package over single-file repositories**
  (`bindings/wasm`, its own workspace), with `Repo` creation, snapshot,
  history, blame and merge, six `wasm_bindgen_tests` and a CI job. The wasm
  code has not been compiled or run yet; only the host build is checked.

- **The README presents Velo as a timeline engine**, with runnable cookbooks
  for agent checkpointing, a config registry and a document editor.

- **`velo-mcp`**, a synchronous stdio MCP server over a repository with a
  working tree: `velo_save`, `velo_restore`, `velo_status`, `velo_diff`,
  `velo_history`, `velo_metadata`, `velo_branch`, `velo_merge_plan`,
  `velo_merge_apply` and `velo_blame`. Write tools record the run (`--run`, else
  `$VELO_MCP_RUN`) in the `mcp` metadata namespace, so blame names the run that
  wrote a line. There is no force option, and merging is always plan, then
  apply; an unresolved conflict returns `Conflicts` with the paths and the next
  step.

- **Metadata queries.** An index on `snapshot_meta`, a history filter,
  `Repo::find_snapshots` and `velo history --where`, composing with the path
  and ancestry scopes.

- **`merge::commit`**, with `merge::Resolution` and `merge::MergeCommit`:
  concludes a planned merge without a working tree, from a recomputed plan plus
  the caller's resolutions. `save::Options.meta` records metadata on a save.

- **Change events.** The `events` module (`Event`, `Ref`, `Listener`, an
  `mpsc::Sender` impl) and `Repo::listening`: `Saved`, `Merged`, `RefMoved` and
  `Imported` are delivered after commit, per handle.

- **`MergeDriver`** in `velo-merge`, with `LineDriver` and `whole_file_conflict`,
  chosen per path on the handle (`Repo::merging`) and used by plan, merge,
  cherry-pick and rebase.

- **Key-aware merge drivers** (14.4): `JsonDriver`, `YamlDriver` and `TomlDriver`
  in `velo-merge`, each behind its own feature (`json`, `yaml`, `toml`, none on
  by default, forwarded by `velo-core`) and registered by `velo-cli` for
  `*.json`, `*.yaml`, `*.yml` and `*.toml`. Keys merge independently, arrays and
  scalars are atomic, and a side that fails to parse falls back to diff3
  exactly. Re-serialising can change formatting, and YAML comments are dropped.

- **Diff and blame follow the path's driver**, so they answer at the driver's
  granularity rather than always by line.

- **Chunked objects** below object identity, with `gc` collecting unreferenced
  chunks and `fsck` reporting `MissingChunk`, `CorruptChunk` and
  `Cruft::UnreferencedChunks`.

- **Compaction**: `commands::compact` and a `compactions` table, so a compacted
  id yields `Error::Compacted` / `compacted_into` instead of `NotFound`.
  `commands::retention` applies a tiered keep policy over it.

- **HTTP sync**: `serve::http` handlers, an `HttpRemote` client behind the
  `http` feature, and `velo serve-http` (no authentication; see the README).

- **git interchange**: `velo export-git` writes a fast-import stream with Velo
  trailers; `velo import-git` reads a fast-export stream and restores ids
  losslessly when the trailers are present. Octopus merges are `Unsupported`.

- **A second object backend**: objects as zstd blobs in the SQLite database,
  chosen by `Repo::init_with(InitOptions)` and recorded in a `settings` table,
  and **single-file repositories** with no working tree. Working-tree commands,
  including `diff::between` against the tree, return `Unsupported` there.

### Changed

- **`ObjectStore`** is now the one seam behind every object access;
  `store_raw`, `hash_and_compress` and `read_object` keep their signatures.
- **`gc`'s trash expiry** compared the wrong way round; the cutoff is now
  computed in Rust and bound as an integer.
- `FileKind::from_mode` is `pub(crate)`.

### Changed — repository format

- **Repository format v3**: large objects may be stored as chunks behind a
  manifest, and objects may live in the database. Object hashes and bundles are
  unchanged. `FORMAT.md` records signatures, chunked objects and compaction as
  decisions (§10 D5 to D7, §11), documents the HTTP transport (§9.1) and updates
  §2, §7.1 and §7.2.

## 4.0.0

The v4.0.0 tag has been **moved** rather than superseded, twice. It was first
pushed before Phases 5 to 11 landed, and again before Phase 13, and nothing was
depending on either — Velum, the only consumer outside this repository, pins a
commit rather than a tag. Everything below therefore ships as one version.

Nothing is on crates.io yet, which is the only reason this is available. Once a
version is published there it is permanent: it cannot be replaced, re-uploaded
or deleted.

### Added

- **Velo has a rename concept.** It had none anywhere, and a snapshot is a whole
  tree — so a move arrived as a delete plus an add and was gone by construction.
  Renames are now **recorded, not detected**: `velo mv` writes the edge, and
  `SaveTree.renames` takes it from a consumer that moved the file itself.
  Content-similarity detection is deliberately not implemented; it is a
  heuristic where an exact answer is available.

  Reading the edges back is one shared primitive, `commands::paths`. Its
  starting point is a parameter rather than implicit: following renames
  backwards needs to know where backwards starts, and the only endpoint
  available to invent is `.velo/PARENT`, which is exactly the wrong guess for a
  consumer built on `save_tree`.

- **`velo blame --lines N-M`, and `blame::Options`.** The walk reads and diffs
  two whole file texts per snapshot and stops once the requested lines are
  explained, so a window ends it far sooner — a gutter's worth of a document
  whose first line is original otherwise walks the entire history. `Options`
  also carries an observer and a cancel, and an `at` that falls back to the
  checked-out branch tip so the default works without a working tree.

- **`LineOrigin.branch`**, and `show` reports its rename edges. Both on the
  argument that put the author on `LineOrigin`: the column is in the row the
  walk already reads, and otherwise every consumer writes the same lookup.

### Fixed

- **Blame credited a merge with everything the branch it absorbed wrote.** A
  merge's diff against its first parent contains all of it, so the wrong time,
  the wrong message and the wrong person. Blame now asks *which parent explains
  this line*: a line is the merge's own only when no parent had it, which is
  exactly a conflict resolution. Where drafts are the normal way to write, this
  was the common path, not an edge case.

- **Blame stopped dead at a rename**, crediting the whole file to whoever moved
  it — the parent text came back empty on the old path, so every line looked
  new. **`history --file` truncated there too**, for the same reason: it was a
  filter on a name rather than on a file.

### Changed — breaking (API only)

- **`blame::run` takes options.** `run(repo, file, at)` is now
  `run(repo, file, blame::Options { at, .. })`. `Blame.path` is a `PathBuf`,
  `Blame.snapshot` and `LineOrigin.hash` are typed ids, and `LineOrigin` gained
  `branch` and `path`.

- **`SaveTree` gained `renames`.** `&[]` is the previous behaviour.

- **`show::SnapshotDetail` was missed by the typed-ids pass.** `branch` is a
  `BranchName`, `parent` a `SnapshotId`; `merge_parent` and `renames` are new.

- **`FileChange::Renamed`** is a new variant, so a `match` over `FileChange`
  needs an arm for it.

### Changed — bundle format v3

- **Bundles carry rename edges**, appended after `objects` and changing nothing
  before it. A version-2 bundle is a version-3 bundle with no edges, and readers
  accept both; older builds reject v3, which is the honest outcome since they
  would otherwise import the history and quietly drop the edges.

### Added

- **Every crate is ready to publish.** `velo-core`, `velo-merge`, `velo-tui`,
  `velo-testkit` and `velo-cli` carry the metadata crates.io requires — per-crate
  README and LICENSE, keywords, categories, homepage, and docs.rs configured to
  build all features so `bundle` and `ssh` are not hidden. Each declares
  `rust-version = "1.88"`, the highest MSRV in the dependency tree rather than a
  guess, and CI builds on exactly that toolchain and packages every crate on
  every push, so neither the floor nor the packaging can drift unnoticed. The
  upload itself is a deliberate manual step; see `DEVELOPING.md`.

- **`blame` says who, not just when.** `LineOrigin` carries the snapshot's
  `Author`, resolved once per snapshot visited rather than once per line. Every
  consumer that wanted per-line provenance would otherwise have written the same
  lookup and the same deduplication. `velo blame` shows the name in its own
  column, which appears only when the history has authors — the first place the
  CLI surfaces authorship at all.

- **`gc` and the sync commands take options.** `gc::Options`, `sync::Options`
  and `sync::CloneOptions` carry a per-call observer and a cancellation flag, so
  the longest local operation and the four that wait on a network can all be
  watched and stopped. That completes 7.6 for every command it named.

- **`history` can scope to a snapshot's ancestry.** `Options::from` — and
  `velo history --from <snapshot>` — lists what led to a snapshot, across
  branches, rather than what was recorded on one branch. A branch listing stops
  at the fork point, because shared history carries the branch it was recorded
  on; this one does not. It takes precedence over `--branch` and `--all`, which
  the CLI rejects outright rather than silently ignoring. Unlike the default
  scope it needs no `.velo/PARENT`, so it works for a consumer with no working
  tree.

### Fixed

- **A cancelled transfer no longer leaves the remote process running.**
  Cancellation is checked between transfer chunks, and the child is killed
  explicitly, because `std::process::Child` does not kill on drop — returning
  through `?` would have left a server talking to a client that had gone.

- **Merging the same branch twice no longer re-raises settled conflicts.**
  Ancestry was walked through `parent_hash` alone, so the tip a merge absorbed
  was invisible to the next merge: the base fell back to the branch point, and
  every conflict resolved the first time came back the second. `merge_base`,
  `merge::plan` and `velo merge` all read the same walk, so all three were
  wrong together and are right together now. The walk follows both parents,
  bounds its depth, and states in a comment that "lowest" is minimum depth —
  an approximation that differs from the true merge base only when histories
  criss-cross.

- **`velo history` shows the work a merge brought in.** Same cause, and it made
  a branch that had merged another list none of the merged snapshots. Entries
  are now ordered by time rather than by walk order, because with two parents
  there is no single chain to follow.

- **`velo history --file` now means what it says.** It matched whether a path
  *existed* in a snapshot, and a velo tree is the complete file set — so it
  listed every snapshot since the file was created. It now compares the path
  against the parent, so additions, deletions, edits and mode changes count and
  nothing else does. A directory matches everything beneath it, which the help
  has always advertised and which previously matched nothing at all. With a
  filter set, `--limit` counts matches rather than candidates.

### Changed — breaking (API only)

- **`gc`, `clone`, `fetch`, `push` and `pull` take an options struct.**
  `gc::run(guard, keep_days)` is now `gc::run(guard, gc::Options { keep_days,
  .. })`; `clone(url, dir, spawn, observer)` is now `clone(url, spawn,
  CloneOptions { dir, observer, cancel })`, and `dir` is a `&Path` rather than a
  `&str`, per 8.3. `fetch`, `push` and `pull` gain a trailing
  `sync::Options`. `Default::default()` is the previous behaviour in every case.

- **The public surface shrank before it can be frozen.** Nine items that were
  `pub` only because two modules inside `velo-core` needed them are `pub(crate)`:
  `get_tracked_files`, `is_binary`, `FileRef`, `Reconcile`, `reconcile_file`,
  `remove_empty_parents`, `decision_to_db`, `decision_from_db`, `read_text`. None
  was used outside the crate. This is free today and a major version once
  `velo-core` is published.

- **The read commands take resolved ids and real paths.** `show::run`,
  `blame::run`, `grep::run` and `bundle::create` took a `&str` spec and resolved
  it again, so a caller holding an id had to format it back into text — and could
  be handed `AmbiguousPrefix` for an id it already held unambiguously. They now
  take `&SnapshotId`. `blame::run`, `bundle::create` and `bundle::apply` take
  `&Path` where they mean a path rather than `&str`.
- **`grep::run` takes an `Options` struct.** `run(repo, pat, None, false, true, 2)`
  said nothing about which flag was which.
- **`show::run` takes `&[&Path]`** instead of a single `&Option<String>`.
- **`history::Options.file` becomes `paths: &[&Path]`.** A document and its
  assets are more than one path, and running the query once per path then merging
  gets the limit wrong. `velo history --file` is repeatable to match.
- **`diff::run_range` becomes `diff::between(repo, a, b, paths)`**, taking
  resolved ids and `&[&Path]`. `diff::dispatch` — which parsed `a..b` range
  syntax and decided whether a bare argument was a filename or a ref — moved to
  `velo-cli`, where argv comes from. Core gained `diff::tracks_path`, the one
  query that decision needed.
- **`save::run` takes an `Options` struct** — `amend`, `paths: &[&Path]`,
  `author`, `observer` and `cancel` — and `run_with_paths` is gone. A cancelled
  save records nothing, which is clean rather than partial: hashing writes
  objects, and an object nothing references is exactly what `gc` collects.
- **`cherry_pick::run` and `rebase::run` take an author**, so the snapshots they
  create on your behalf are attributed like any other.
- **`restore::run` takes `&SnapshotId` and an `Options` struct** carrying `force`,
  `paths: &[&Path]`, and the observer and cancellation token above.
- **`merge::run` and `rebase::run` take a mode enum.** `rebase::run(guard,
  target, abort, cont)` encoded three states in two booleans and the fourth
  combination was meaningless; `merge::run` had a `(Option<&str>, bool)` pair
  whose "no target, no abort" case was a run-time error. Both are now
  `Mode` enums the caller picks once.
- **`cherry_pick::run` takes `&SnapshotId`**, and `rebase::Mode::Start` carries
  one. `merge::Mode::Bring` deliberately keeps a `&str` spec: an exact branch tip
  must win over a hash prefix, and the branch *name* is what `MERGE_HEAD` records
  for the eventual save to resolve into a second parent.

### Added

- **`Scope`** — programmatic ignore rules and scoped roots, set with
  `Repo::scoped`. `Scope::new().ignore(".myapp-cache/**")?` keeps an
  application's own files out of a user's history without writing `.veloignore`
  into their folder; `Scope::new().only("prompts/**")?` restricts to one subtree.
  A scope **narrows and never widens**: the user's own `.veloignore` and
  `.gitignore` still win, which is enforced rather than merely documented.

### Added

- **Cancellation.** `progress::Cancel` is a cheap, cloneable flag; pass it per
  call and the operation stops at its next checkpoint with `Error::Cancelled`.
  Cooperative by design — checked *between* files, so it never interrupts a write
  part-way. It is not a rollback: files already written stay written, the same
  position a killed process would leave, which `velo status` describes
  accurately. Wired into `restore` first, which is where a GUI most needs it.
- **Per-call progress.** `restore::Options.observer` overrides the repository's
  observer for that one call. `Repo::observing` remains as the convenience
  default. This is what the project's own anti-goal asked for — *"Don't use
  globals for progress or cancellation. Pass them per call."* — and a test
  asserts the handle's observer is **not** consulted when a per-call one is
  given, since otherwise it is still a global with extra steps.

### Added

- **Authorship, with no format change.** `Author` (name plus optional email) is
  recorded through `SaveTree.author` and read back with `SnapshotMeta::author()`.
  It lives in the reserved `velo` metadata namespace, which means it is **hashed
  into the snapshot id** — so a rewritten author is reported by `fsck` rather than
  believed. That is the whole argument: rewritable provenance in a repository that
  syncs is worse than none, because it still looks trustworthy. No schema change
  was needed, because `snapshot_meta` already existed, was already hashed, already
  travelled with bundles and sync, and already refused the `velo` namespace to
  application callers. `velo save` reads `VELO_AUTHOR_NAME` and
  `VELO_AUTHOR_EMAIL`; an absent author is not an error, a malformed one is.
- **`merge::plan`** — what a merge would do, with none of it done. `merge::run`
  needs a clean working tree, writes files and records conflict state, which an
  application with unsaved buffers can use none of. `plan` takes a `&Repo` rather
  than a `&WriteGuard`, reports the same classification, hands back the merged
  content for auto-merges and the three sides by object for conflicts.
- **`branches::create` and `branches::set_tip`** — a branch can be created
  pointing at any past snapshot, without switching to it. Previously a branch came
  into existence only via `switch` (which also makes it current and leaves it
  unborn) or as a side effect of `save_tree`.
- **`merge::merge_base`** — the shared ancestor two snapshots merge against, so a
  consumer that merges need not reimplement an indexed recursive query in memory.
- **`Repo::snapshot`** — everything recorded about one snapshot, by the id a
  consumer already holds, instead of formatting it back into text to be resolved
  again.
- **`Repo::head_token`** — one integer that changes whenever snapshots, branches
  or tags change, for an application that has to notice a second window or a
  `pull`.
- **`SaveTree.timestamp_ms`** — a caller can supply the creation time, `None`
  meaning now. The timestamp is part of a snapshot's identity, so with the clock
  read internally an embedder could not control the id: no test could assert one
  against a constant, and importing history from another system would stamp every
  snapshot with the moment of the import. It was also the last ambient dependency
  in a crate that otherwise reads no process environment. Note that a branch tip
  is derived from the newest `created_at_ms`, so a backdated snapshot does not
  become the tip.
- **`SaveTree.merge_parent`** — a snapshot recorded through the embedder API can
  be a merge. The column, the id recipe and `history::Entry::is_merge` already
  supported a second parent; only the write path did not, so an embedder's merges
  were stored as linear history. Beyond the graph, that left the next merge
  computing its base from too old an ancestor and re-raising resolved conflicts.

### Changed — breaking (API only; the repository format is untouched)

- **`history::run` takes an `Options` struct.** It was five positional arguments,
  `run(&repo, false, 0, None, None)`, which said nothing at the call site — the
  same problem `SnapshotIdentity` fixed for `snapshot_id`. `limit` is now
  `Option<usize>`: `None` means all of them and is the default, `Some(n)` the
  newest n. Previously the limit reached SQL as `LIMIT ?`, so `0` — the obvious
  way to ask for everything — returned nothing, and `usize::MAX` worked only
  because the cast to `i64` wrapped to `-1`.
- **`history` results carry typed ids.** `Entry.hash` is a `SnapshotId`,
  `Entry.branch` a `BranchName`, `parent`/`merge_parent` `Option<SnapshotId>`,
  `tag` an `Option<TagName>`; `BranchRef.name` is a `BranchName`,
  `History.current` an `Option<SnapshotId>`, and `refs` is keyed by `SnapshotId`.
  2.3 typed every other result struct and missed this one, so walking history to
  look something up needed a `.parse()` whose error could never fire.
- **`SaveTree.branch` is `&BranchName`**, matching `parent`, so a consumer that
  publishes to one branch no longer clones the name on every save.
- **`tag::create` takes `Option<&SnapshotId>`** rather than a spec. Callers with an
  id in hand no longer hand back text to be resolved again; the CLI resolves at the
  argv boundary.
- **`TreeEntry.content` is a `Content` enum.** `Content::Bytes(Vec<u8>)` is what
  it was; `Content::Stored(ObjectHash)` is new. The `file`/`executable`/`symlink`
  constructors are unchanged, so only code that matched on `.content` is affected.

### Added

- **`Repo::branch_tip` and `Repo::branch_exists`** — ask about a branch directly
  instead of resolving it as a spec and interpreting `NotFound`. The two answers
  differ: a branch created by `switch` exists with no tip.
- **`TreeEntry::stored(path, object, kind)`** — a tree entry that references
  content already in the store. A snapshot is a whole tree, so changing one file
  meant re-supplying the bytes of every other file: each save cost the size of the
  entire tree in decompression, hashing and recompression. Carrying a tree forward
  is now a map with no I/O. A referenced object that the store does not hold is
  refused with `MissingObject`, so the API cannot record a snapshot naming content
  that is not there.

### Fixed

- **Saving an identical snapshot twice is idempotent.** A snapshot's identity
  excludes the branch, so the same tree with the same message and parent recorded
  on two branches inside one millisecond is *one* snapshot — but the second write
  surfaced a raw `UNIQUE constraint failed: snapshots.hash`. The existing rows are
  now left alone and the branch is still registered, so both branches see the tip.
- **Full-width snapshot ids leaked into terminal output.** Format v2 stores ids at
  64 characters, and eight renderers still printed them verbatim — `status`, `tag`,
  `undo`, `redo`, `show`'s parent line, `restore`, `switch`, `merge` and
  `history --graph`. Under v1's 16-character ids this looked right by accident;
  `velo tag`'s table blew out to a 64-wide column. Every site now goes through one
  abbreviation. The v4.0.0 notes claimed this was already consistent; it was not.
- `resolve_snapshot_id` reports an unresolvable spec as
  `Error::NotFound { kind: RefKind::Any, .. }` rather than `Error::InvalidInput`.
  A consumer asking for a branch that has no snapshots yet needs to tell "nothing
  there" from "malformed request", and matching the message text is what typed
  errors exist to avoid. Found by writing an actual consumer.

### Added

- `examples/prompt-registry` — a worked example of embedding `velo-core` with no
  working tree, plus `FINDINGS.md` recording where the API was awkward.

### ⚠️ Repository format v2 — breaking

**Existing repositories cannot be opened.** Format v2 changes every snapshot id,
so a pre-v2 repository is refused rather than migrated: `Repo::open` and
`Repo::open_and_migrate` both fail with `Error::FormatTooOld`, and the CLI
explains what to do instead. To carry work across, copy the working tree into a
fresh repository and save it there — a v1 bundle cannot help, because it carries
v1 ids.

Repositories written by this version and later are stamped with their format
version at creation, so this situation is detectable rather than guessed at.

All four Phase 0 decisions landed together, because splitting them would have
meant four separate id-invalidating migrations:

- **Snapshot ids are stored at full width** — 64 hex characters instead of a
  16-character truncation that was also the primary key. Truncating a BLAKE3
  digest to 64 bits put a ~50% collision risk near 5·10⁹ snapshots, which is thin
  for a store several applications write to. Abbreviation is now display-only.
- **Timestamps are epoch milliseconds** (`created_at_ms INTEGER`), hashed as their
  decimal form. Previously the id committed to formatted text, so a change to a
  format string could have altered snapshot ids.
- **Snapshot metadata**, hashed as part of the identity (see below).
- **Schema versioning** via `PRAGMA user_version`, with refuse-if-newer — this
  part shipped earlier and is what makes the refusal above possible.

The domain separator is now `velo-snapshot-v2\n`, so a v1 and a v2 id can never
collide even given identical inputs. Bundles and sync packs use magic `VELOBND2`;
a reader rejects an unknown version rather than guessing.

Objects are **format-stable** — no object file changed, and object hashes are
unaffected.

### Added

- **Snapshot metadata.** `SnapshotMeta` attaches app-namespaced
  `(namespace, key) → value` pairs to a snapshot via `SaveTree.meta`, read back
  with `Repo::snapshot_meta`. Consumers get somewhere to put provenance instead of
  encoding it into the message or inventing a sidecar file.

  Metadata is **covered by the snapshot hash**, and therefore immutable: changing
  a value produces a different snapshot. That is deliberate — metadata is mostly
  provenance, and provenance that can be silently rewritten is worth nothing.
  `fsck` reports a rewritten metadata row as an id mismatch. It also means
  metadata travels with bundles and sync automatically, because a receiver that
  did not get it would recompute a different id.

  The namespace `velo` is reserved. `velo save` attaches no metadata.

- `commands::timestamp_from_ms` for rendering a stored timestamp as
  `DateTime<Utc>`.

### Changed

- Commands that report a time (`history`, `show`, `blame`, `branches`,
  `stash list`) now return `DateTime<Utc>` rather than a preformatted string, so
  a consumer can format it however it likes.
- `commands::snapshot_id` takes a `SnapshotIdentity` struct instead of six
  positional arguments. Three of them were adjacent `&str`s, where swapping two
  would compile and produce a plausible-looking id.
- Snapshot ids are abbreviated consistently wherever they are printed. Some
  commands previously showed 8 characters and others 16; with full-width ids the
  inconsistency would have produced a 64-character column in `velo history`.
- The schema is one idempotent definition rather than an initial schema plus a
  chain of `ALTER TABLE` migrations. Those existed only to bring v1 repositories
  forward, which v2 refuses to do.

### Fixed

- **A merge resolved entirely in favour of your own side can be recorded.**
  `velo resolve --take ours --all` leaves the working tree unchanged, so `save`
  reported "Nothing to save", exited 0 without recording the merge, and left
  `MERGE_HEAD` in place. That wedged the repository — every later merge, rebase,
  undo, redo and cherry-pick refused with "a merge is already in progress", and
  the only escape was `merge --abort`, which discards the merge. A merge is real
  information regardless of the resulting tree, because it records the second
  parent, so it now gets a snapshot. A clean tree with no merge pending still
  reports nothing to save.
- `Display` for the id newtypes uses `f.pad`, so `{:<20}` aligns them. `write_str`
  silently ignores width, which had un-aligned `velo tag`'s table.
