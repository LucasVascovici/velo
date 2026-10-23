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
| Six | 6.2.x release | planned | planned | planned | planned | planned | planned |

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

## Run six (planned, not yet run)

Version: the 6.2.x release.

Blind spots to exercise, none of which any earlier run covered: `restore <path>`, `stash drop`, `stash show`, rename-aware merge, `mcp`, `push-branch`, `remote add`, `remote remove` and `remote ref`, HTTP remotes, large files and `.veloignore`.

Fixes under test:

- commands per refused push below 4
- tag publishing
- the HTTP token message
- the mid-merge refusals
- the exit-code rule
- the git-habit spellings

Mixed versions: a 5.3 client against a 6.x remote, and the bundle round trip back.

A quiet machine: Windows Defender exclusions as in [WINDOWS.md](WINDOWS.md), or Linux, so that timing and lock contention reflect velo and not a scanner.

Convergence rule: the run repeats until nothing above green is open.
