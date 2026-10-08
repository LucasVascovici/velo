# velo UX report (pilot run, run1)

Developers observed: alice, bob, carol, frank (4 of 8; about 510 commands in total, about 27 non-zero exits). Sources: per-developer reviews built from the command logs, the failure clusters, a CLI help audit, and self-reports.

## Verdict

velo is easy at the single-user core and not yet easy at the shared-branch edge. Save, status, switch, undo/redo, amend, partial save, named stash, push success and fsck worked first try for every developer, and the output teaches the next command. Ease ratings were 2 to 4 in the busy rounds, and 4 or 5 in quiet sync rounds. The cost is concentrated in one flow: diverge, pull, merge, resolve, save, push. There, a bare `velo resolve <file>` hangs without a TTY (140-146 s, hit by alice, bob and frank), `--take ours` can silently overwrite hand edits, `pull` says "diverged" and exits 0 (even when only ahead), and the push-refusal advice does not match the real recovery steps. Two silent content losses were seen (`--take` over hand edits, `save --amend` on a merge). Fix resolve and the pull/push messaging and velo would be clearly easier than git for this workflow.

## Scorecard

| Dimension | Score | Evidence |
|---|---|---|
| Discoverability | 3 | `velo log` (bob x2, frank) and `diff-worktree`/`history-all` (alice, carol) failed with weak or no hints; `history -n` rejected; yet the "similar subcommand" tip fixed `stash-push` in one step. |
| Error messages | 2 | `velo resolve CHANGELOG.md` without TTY repainted a TUI for 141 s then rc 1 (alice #40); "another Velo operation is in progress" names no holder (4 hits, 3 developers). |
| Consistency | 3 | The `--` path separator and save/status/diff family are uniform, but help cites `diff --conflict` (does not exist) and a `<file>.conflict` sidecar never created; --force and --abort exist on some commands only. |
| Recoverability | 3 | `undo` then `redo` restored the same hash (frank #15-17), but `--take ours` overwrote a hand merge (carol #41), amend dropped a merge parent (frank #103), and a stuck lock needed killing processes (alice). |
| Output clarity | 3 | status and merge summaries list next commands, but `pull` printed "diverged" while status said "2 ahead" (alice, 7 times), and status told carol "velo pull then velo merge" mid-merge. |
| Learnability for git users | 4 | save/switch/history/diff/stash map directly; surprises are `undo` rewinding files, `restore` moving the branch, and `pull` not merging. |
| Collaboration flow | 2 | Each push-refused round cost 4 to 9 commands (bob 4 refusals, carol 3, frank 3; 10 total in clusters.json) and there is no one-step integrate; non-interactive conflict handling is whole-file only. |
| Help text | 3 | save/merge/resolve/stash help is strong; push/pull/fetch/remote add are bare clap defaults; `resolve <file>` help says "mark as resolved" but opens a TUI. |
| Safety | 3 | Dirty-tree aborts name the file and the fix (frank #97), but `--take` overwrites silently, amend drops merge parents, and the bare resolve hangs and locks the repo. |

## Ranked improvements

Ranked by (pain x developers hit) / effort.

### 1. Make bare `velo resolve <file>` safe without a terminal (high impact, small effort)
Affects: `resolve`, `merge`, `status`.
Evidence: alice #40 `velo resolve CHANGELOG.md` rc 1 after 141 s; bob #23 rc 1 after 146 s; frank #50 after 140 s. Alice also got the TUI with no merge in progress, then "No merge in progress" on the next call. Help says `velo resolve src/auth.py # mark manually edited file as resolved`, which is not what happens.
Change: if stdin or stdout is not a TTY, exit at once with `error: interactive resolve needs a terminal. Use --take ours|theirs|both, or edit the file and run: velo resolve --mark FILE`. Check "merge in progress and FILE conflicted" before drawing anything. Stop repainting on EOF. Fix the help example.

### 2. Add a non-interactive "I edited it" path and `--take both` (high, medium)
Affects: `resolve`.
Evidence: alice #24/#32-33, bob #28/#30, frank #47-48/#61-63 all used whole-file `--take` where both sides' lines were wanted (CHANGELOG, registry). Bob's r1 ledger shows three of his markers lost. Alice: "the only way to keep hand-merged content ... was --take theirs followed by re-editing".
Change: `velo resolve --mark FILE` (accept a file with no conflict markers) and `--take both`. Echo what was dropped: `took theirs; discarded 1 line from ours`.

### 3. Stop `--take` silently destroying hand edits (high, small)
Affects: `resolve --take`.
Evidence: carol #41 "Resolved shop/config.py (took ours)" reverted her hand-merged edit; frank reported the same on three files. One success line, no detail.
Change: if the working file differs from both sides, refuse unless `--force`, or print `replaced your unsaved edits in shop/config.py (3 lines)`. State in the output `ours = your last saved main`.

### 4. Fix `pull` classification and exit code (high, small-medium)
Affects: `pull`, `status`, `merge`.
Evidence: alice's pull printed "'main' and 'origin/main' have diverged. Reconcile with velo merge origin/main" 7 times (#46, #66, #79, #84, #89 ...) while status said "2 ahead"; the suggested `velo merge origin/main` replied "Already up to date" (#49). Carol (#28, #47) and frank (#53, #67, #92, #115, #124) saw real divergence exit 0.
Change: ahead-only prints `Nothing to pull; N local snapshots. Run velo push.`; truly diverged lists both counts and exits with a distinct non-zero code; optional `velo pull --merge`.

### 5. One recovery recipe after a refused push (high, small)
Affects: `push`, `pull`, `status`.
Evidence: 10 refusals in clusters.json (bob, carol, frank). Text said "velo pull origin bring their commits in first / velo push origin then send yours", yet plain `velo pull` then printed "diverged", forcing merge, save, push. Three different recipes across push, pull and status.
Change: use the same line everywhere: `velo pull`, then `velo merge origin/main`, `velo save`, `velo push`. Drop the `origin` argument form. Let a refused push refresh the tracking ref so status stops saying "ahead - velo push to publish" (frank #123).

### 6. Name the lock holder and clear stale locks (medium-high, medium)
Affects: `resolve` and any command under a held lock.
Evidence: 4 hits across alice (#28, #29), bob (#86), frank (#45). Alice cleared it by killing her own processes while status advised `velo resolve`.
Change: `error: another velo operation holds the lock (pid 4812, 'velo resolve CHANGELOG.md', started 2m ago)`. Say whether the pid is alive; auto-clear if dead or offer `velo unlock`.

### 7. No contradictory sync advice during a merge (medium, small)
Affects: `status`.
Evidence: carol #32, #40, #57, #63 "Merge in progress" next to "diverged ... velo pull then velo merge origin/main"; frank #113 "diverged" right after `merge` said "Already up to date".
Change: while merging show `resolve X, then velo save to finish the merge of origin/main`; after resolve show `merge pending - velo save`; label counts "as of last fetch".

### 8. `save --amend` on a merge snapshot must keep both parents (high severity, small-medium; one developer)
Evidence: frank #103-119: after amend, history and show listed one parent, `merge origin/main` said "Already up to date", status said diverged; about 10 commands to recover. Help only says "keeps the same parent".
Change: preserve all parents or refuse (`cannot amend a merge snapshot`). Show `parents: A, B` in `show` and a (merge) marker in `history`.

### 9. Git-habit aliases and did-you-mean hints (medium, small)
Evidence: `velo log` x3 (bob #56, #79; frank #79), `history -n 8` (carol #98), `diff-worktree` (alice #64, carol #25), `history-all` (alice #70).
Change: alias `log` to `history`, `-n` to `--limit`; hints mapping `history-all` to `history --all` and `diff-worktree` to `diff`.

### 10. Show the other side of a conflict without the TUI, and fix help (medium, medium)
Evidence: help promises `<file>.conflict`, none created (carol #36); merge help step 2 cites `velo diff <file> --conflict`, which errors (audit); bob used `show origin/main` and `diff origin/main -- file` to find the sides.
Change: write the sidecar or correct the help; add `velo conflicts FILE` printing both hunks. Add a test that every command quoted in help parses.

### 11. Smaller items (low-medium, small)
- `undo` removes files from disk (alice #10-12): say `working files rewound to <hash>`, offer `undo --keep`, and hint `velo redo` when "Nothing to save" follows an undo.
- `stash list` shows 1970-01-01 00:00; `stash pop` says "14 file(s) restored" when 2 changed; not-found lists no shelves (frank #25-33).
- `history --branch origin/main` finds nothing while `--all` shows `remotes/origin/main` (alice r2): accept both names.
- From the help audit: document exit codes, add `gc --dry-run`, warn before rewriting pushed history, fix the `mv` Examples block.

## Quick wins (under an hour each)

- TTY check in `resolve` with a clear message, and correct the `resolve <file>` help line.
- Alias `log` to `history`, `-n` to `--limit`; did-you-mean hints for `history-all` and `diff-worktree`.
- Reword the push-refusal and pull-diverged text into one recipe.
- Pull on ahead-only prints "nothing to pull; velo push".
- Print lock holder pid and command in the lock error.
- Status during a merge replaces the sync hint with the merge next step.
- Remove or fix the `.conflict` and `diff --conflict` references in help.
- Fix the duplicated `[default: N]` in help.

## What works well (do not change)

- `save "msg" -- file` partial snapshots and the counted summary.
- `undo`/`redo` symmetry and "Bring it back with velo redo"; redo restored the identical hash.
- `save --amend` on ordinary snapshots; named `stash push`/`pop` matching exactly.
- `switch <new-branch>` explaining "no commits yet; your first velo save will start it from <hash>".
- Merge conflict summary: per-file New/Updated/Conflict, Quick-take line, `--all --take` shortcut, and the end message "All conflicts resolved! ... velo merge --abort to cancel".
- Dirty-tree refusal naming the file and offering save or stash (frank #97).
- status: branch, position, ahead/behind and next command in one fast block (50-250 ms).
- Push success text, including "the remote's working tree is unchanged; it updates on its next velo pull".
- fetch listing each remote tip, the `fsck` checklist, and `show`/`diff origin/main` for inspecting the other side before merging.
- Exit 0 with plain text for "nothing to save" and "already up to date".
- Honest help in `serve-http`, merge's CONFLICT RESOLUTION WORKFLOW, and clap usage errors with tips.

## Per-developer summaries

**Alice** (feature developer, 118 commands, 6 non-zero). The core loop was smooth. She lost 141 s to a bare `resolve CHANGELOG.md` TUI that also left a lock, was told "diverged" seven times when only ahead, and saw her code vanish from disk after `undo` (recovered with `redo`). Ratings 3, 4, 4, 5, 5.

**Bob** (duplicates alice's work, 131 commands, 8 non-zero). Recovered from everything. He lost 146 s to the resolve TUI, ran four push-pull-merge-push loops, used whole-file `--take` where both sides were wanted (three markers lost in r1), twice typed `velo log`, and could not see both sides of a conflict. Ratings 2, 3, 4, 5, 5.

**Carol** (conflict magnet, 103 commands, 5 non-zero). Round 1 and sync rounds were smooth. In round 2 `--take ours` reverted her hand-merged file, status told her to pull and merge mid-merge, merge and pull exited 0 on conflict or divergence, and the documented `.conflict` file did not exist. Ratings 4, 3, 5, 5, 4.

**Frank** (chaos developer, 158 commands, 10 non-zero). Undo, redo, amend, partial save and stash worked first try. The pain was the sync loop (about 25 commands in round 1, about 20 in round 2), a 140 s resolve hang, `--take ours` replacing edits, and `save --amend` silently dropping a merge parent. Ratings 3 (r1) and 4 (r2).

## Appendix: self-reports versus logs

- Frank rated r2 a 4 and said merge conflicts were not an issue, yet the log shows the amend-dropped-parent episode (about 10 commands) and three push attempts. The 4 is generous.
- Bob's r1 summary says his removed markers were "replaced by alice's identical bulk_discount", which hides that his own whole-file `--take` choices discarded his lines.
- Alice says the hang left a lock; the log shows the hang call (#40) came after the merge was already saved and pushed, so the TUI also opens with no merge in progress. She did not mention the `diff-worktree` and `history-all` typos.
- Carol says `resolve <file>` blocks, but her log has no such call (inference; the other three logs confirm it). Her edit reversion is consistent with the log, though edits happen outside velo and are not logged.
- Frank praised `stash list` readability, but the log shows a 1970-01-01 timestamp; he did not report the lock error (#45) or `velo log` (#79).
- Sync-round ratings of 4 or 5 are fair; those rounds ran only status, pull, fsck and fetch.
- Limits: only four developers ran in this pilot; `merge --abort` was never exercised; the exit-0-on-conflict claims rest on the developer reviews (digest or raw-log reads), and I did not re-read the raw logs myself.
