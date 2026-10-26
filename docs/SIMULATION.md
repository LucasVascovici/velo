# Simulation runs

A simulation run has several simulated developers drive the real velo binaries against shared remotes, round after round, doing what a team does: saving, branching, merging, pushing, being refused, reconciling, undoing. An independent oracle, which does not use velo's own code, checks the final state and the content of every snapshot. This page records what each run exercised, what it found and what became of each finding. It records findings only. A value this page does not hold is written "not recorded"; none is estimated.

## Summary

| Run | Version tested | Date | Developers and rounds | Commands | Bugs found | Commands per refused push | Oracle verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| One | not recorded | not recorded | not recorded | not recorded | silent content loss in save, amend, resolve and stash, among others | not recorded | not recorded |
| Two | not recorded | not recorded | not recorded | not recorded | 7 findings fixed in 5.1.1 | not recorded | not recorded |
| Three | not recorded | not recorded | not recorded | not recorded | 5 findings fixed in 5.1.2 | not recorded | not recorded |
| Four (pilot) | 6.0.1 | not recorded | 4 developers, 2 rounds | not recorded | counted with run five below | not recorded | not recorded |
| Five (full) | 6.0.1 | not recorded | 9 developers, 4 rounds | about 2,700 | 3 wrong-result bugs, 1 `fsck` classification question, message defects | 8 to 27 for eight of nine developers; `velo sync` ran once | "blame attribution mismatch" warnings, unexplained at the time |
| Six | 6.2.1 | 2026-10-23 | 9 developers: 4 rounds, then rounds 1 and 2 again | 2,354 and 933 | 3 (1 medium display, 2 low), fixed in 6.2.2; no data loss | 32.5, then 38.6 with velo's own recipe; `velo sync` used 35 times in the second pass | clones equal the remote and `fsck` clean everywhere; five missing markers traced to deliberate edits |

## Run one

Version tested, developers, rounds, commands and oracle verdict: not recorded. The findings below are the ones the 5.1.0 release notes list as coming from it.

| Finding | Severity | Disposition |
| --- | --- | --- |
| `save`, amend, `resolve` and stash could silently lose content | wrong result, data loss | fixed in 5.1.0 |
| `resolve` needed a terminal | blocks scripted use | fixed in 5.1.0 |
| Lock holders were not named and not waited for | usability, contention | fixed in 5.1.0: bounded wait with jittered backoff, holder named |
| Restore, import, grep, save and merge were slow | performance | fixed in 5.1.0 |
| Exit codes were not distinct, and several reconcile recipes disagreed | usability | fixed in 5.1.0: distinct exit codes, one recipe |

Not exercised: not recorded.

## Run two

Version tested, developers, rounds, commands and oracle verdict: not recorded.

| Finding | Severity | Disposition |
| --- | --- | --- |
| `undo` orphaned another branch's history, leaving a missing parent that failed `fsck` | wrong result, damaged repository | fixed in 5.1.1: `undo` refuses and names the branches; `redo` recovers an already damaged repository |
| `resolve --mark` dropped the other side's clean changes | wrong result, data loss | fixed in 5.1.1: refused unless `--force`, listing the lines |
| A merge of a diverged tip with equal content printed "Already up to date" and left the branch diverged | wrong result | fixed in 5.1.1: it records a merge |
| Merges did not follow moves | wrong result | fixed in 5.1.1 |
| A fast-forward merge lost the feature's history in `history`, `blame` and ahead/behind | wrong result | fixed in 5.1.1 |
| `remotes/<remote>/<branch>` and `<remote>/<branch>` named different snapshots | wrong result | fixed in 5.1.1 |
| `switch` to the current branch left a stale working position | minor | fixed in 5.1.1 |

## Run three

Version tested, developers, rounds, commands and oracle verdict: not recorded.

| Finding | Severity | Disposition |
| --- | --- | --- |
| `fetch` doubled an undone snapshot's tree; `merge` of an absent tracking ref made a parentless merge | wrong result | fixed in 5.1.2 |
| `rebase` replayed snapshots already reachable through a merge's second parent as spurious conflicts | wrong result | fixed in 5.1.2 |
| `merge --abort` altered files with CR line endings; `resolve` turned CRLF files into LF | wrong result | fixed in 5.1.2 |
| `resolve --take` leaked the other side's lines when the ancestor ended in a blank line | wrong result | fixed in 5.1.2 |
| `push` did not name the side branches it published; `bundle apply` did not count tags; squash text did not say originals are not restorable | message defects | fixed in 5.1.2 |

The release notes also list performance work in pack import, `fetch`, `grep`, `undo`, `restore`, `history --file`, `fsck` and `gc` alongside these fixes.

## Run four: the 6.0.1 pilot

4 developers, 2 rounds against 6.0.1. Commands and oracle verdict: not recorded. Its findings are reported together with run five, since both ran against the same version.

## Run five: the 6.0.1 full run

9 developers, 4 rounds, about 2,700 commands, against 6.0.1.

Commands per refused push: 8 to 27 for eight of the nine developers, and `velo sync` ran once. Developers did not find the reconcile path from the refusal text.

| Finding | Severity | Disposition |
| --- | --- | --- |
| A merge recorded the wrong second parent | wrong result | fixed in 6.1.0: MERGE_HEAD records the resolved source snapshot, and save and `merge --abort` use it |
| `redo` after a fast-forward pull restored a stale snapshot | wrong result | fixed in 6.1.0: `redo` refuses with a typed error when the branch moved since the undo |
| `switch` was allowed during a merge in progress | wrong result | fixed in 6.1.0: `switch`, whole-tree `restore` and `stash pop` refuse while a merge or rebase is in progress |
| `fsck` classification of delta rows after undo-heavy use | question, then a real gap in `save` | fixed in 6.1.0: `save` removed the branch's trash rows but left their stored delta trees; it now removes them in the same transaction, and `fsck` reports an unreachable orphan chain as cruft that `--repair` clears |
| A refused push did not say what was true, and the recipe began with `pull` | message defect | fixed in 6.1.0: the refusal states the remote's position and the recipe starts with `velo sync` |
| The HTTP push refusal did not name the token variable or server option | message defect | fixed in 6.1.0 |
| `resolve`, `rebase --continue` and `merge` misdescribed what was wrong | message defects | fixed in 6.1.0 |
| Cherry-pick counts, the merge summary after `mv`, `conflicts` context, blame past end of file | message defects | fixed in 6.1.0 |
| Git-habit spellings (`stash push -m`, `HEAD`, `ref:path`, `switch <new>`) were refused without naming the velo spelling | usability | fixed in 6.1.0: the cheap spellings are accepted, the rest name the velo one |
| Exit codes for not-found and nothing-to-do were inconsistent | usability | fixed in 6.1.0: not-found is 1, nothing-to-do is 0 |
| `push` did not publish tags whose snapshot the remote already had | missing behaviour | fixed in 6.1.0, and it says so |
| "blame attribution mismatch" oracle warnings | warning | by design: they come from the oracle's parsing plus the replay authorship velo records; the CLI now prints `(unknown)` in an empty author cell, and the blame walk was fixed after merge, fast-forward, rebase, cherry-pick and take-theirs |

Not exercised: `restore <path>`, `stash drop`, `stash show`, rename-aware merge, `mcp`, `push-branch`, `remote add`, `remote remove` and `remote ref`, HTTP remotes, large files, `.veloignore`.

Open after this run: none of the findings above. The blind spots above remain untested.

## Run six: the 6.2.1 runs

Two passes against 6.2.1 (built from `main` at `6114905`), 2026-10-23, both with
9 developers and an HTTP remote that requires a write token.

- **Full run:** 4 rounds, 2,354 commands. All nine clones ended at the remote's
  tip, and `fsck` was clean on the remote and on every clone.
- **Second pass:** rounds 1 and 2 again, 933 commands, with the integration
  routine changed from "pull, then merge" to "do what velo's refusal message
  says", so that commands per refused push measured velo's own guidance and not
  the harness's. Converged with no oracle problem.
- **Blind spots:** the full run's blind-spot missions did not reach the
  developers; the second pass exercised `stash drop`, `stash show`,
  `remote add` and `remove`, path restore and `.veloignore`, and a scripted
  check ran the rest against the same build. Twelve checks, all passing
  ([results](../bench/results/2026-10-23-run-six-blind-spots.json)).

Commands per refused push: 32.5 in the full run, where the harness still told
developers to pull and merge, and 38.6 in the second pass, against a target of
under 4. In the second pass `velo sync` was used 35 times (once in the full run)
and by seven of the nine developers, and it worked whenever it was used. The
reviewers' reading: the refusal prints `velo sync` and then "(or by hand:)",
and developers took the manual recipe as the real instructions.

| Finding | Severity | Disposition |
| --- | --- | --- |
| Five merged work markers missing from main at the end of the full run | oracle problem ("data loss?") | not a velo loss. Traced through the remote's history: four were removed when a developer rewrote the `shop/config.py` lines that carried them, on a mission to edit those lines (snapshot `3a97c25`), and every later merge correctly kept the edit; one was dropped by `resolve --mark --force` after velo refused without `--force` and named the line. The oracle counts a marker removed by a deliberate edit of its own line as lost |
| `velo conflicts` showed lines neither side has when both sides moved the same line | medium, display only | fixed in 6.2.2: hunk context is now text both sides agree on, not the ancestor's lines |
| A path restore with `--force` reported discarding unsaved changes outside its paths, and without `--force` refused over them | low | fixed in 6.2.2: only changes under the requested paths count |
| `undo` of a local merge warned that it rewrote history pushed to the merged branch | low | fixed in 6.2.2: undo and amend exclude what every parent reaches |
| A 5.3.0 client against a 6.2.1 remote, and a 6.2.1 bundle applied by 5.3.0 | mixed versions | both work; 5.3.0 opening a v4 repository directly is refused with a message |
| The refused-push loop costs far more than the target | usability | open: see above |
| An unborn branch forgets its start point after switching away and back | usability | open: the start point is not stored; the message should say "from your current position" or the point should be kept |
| `velo push <branch>` with no remote reads the branch as a remote name | usability | open |
| Refs are positional in `show` and `diff`, flags in `blame`, `grep` and `history`; `show ref:path` is refused | usability | open |
| `push` and `sync` publish branches and tags nobody named | usability | open; by design so far (side branches since 5.1.2, tags since 6.1.0), and the reviewers flagged it as a safety surprise |
| Blame attribution warnings | warning | by design, as in run five (replay authorship and the oracle's parsing) |

Not exercised by simulated developers: `mcp`, push of a large file over HTTP
in the second pass, `tag --force` and `tag --delete` in the second pass. The
scripted checks cover `velo-mcp` over stdio and a 32 MB file over HTTP; the full
run covered the tag commands.

Open after this run: no bug. The usability findings above remain open.
