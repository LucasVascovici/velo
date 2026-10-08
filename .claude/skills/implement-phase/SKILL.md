---
name: implement-phase
description: Implement a phase or item of ARCHITECTURE.md end to end — Opus plans it into tasks, Sonnet subagents implement, test, commit and push each one, Haiku confirms delivery, reviews are routed by risk, and you get a summary table. Use when the user says "implement phase 14", "do 14.3", "/implement-phase 14".
argument-hint: <phase|item> [--yes] [--parallel N] [--branch NAME] [--skip id,id] [--only id,id]
disable-model-invocation: true
---

# /implement-phase

You (Opus, the main loop) are the orchestrator. The work runs in the
`implement-phase` workflow at `.claude/workflows/implement-phase.js`, using the
agents `phase-planner`, `phase-implementer`, `phase-lander` and
`phase-reviewer`. Do not implement tasks yourself, and keep your own turns
short — every token you spend here is Opus.

Arguments: `$ARGUMENTS`

| Step | Model | Notes |
| :--- | :--- | :--- |
| Plan | Opus, high effort | Once per phase; the plan is saved and reused |
| Refresh brief | Sonnet, medium | Only when the plan predates code that has since landed |
| Implement / fix | Sonnet | |
| Land | Haiku, low | Delivery check, push retry, diff stats |
| Review | Opus if high-risk, else Sonnet | Re-reviews are always Sonnet |
| Finalize | Sonnet | Full suite on the branch head, docs and changelog |

## 1. Parse

- First positional: the phase (`14`) or item (`14.3`). Required — ask if missing.
- `--yes` skips plan approval. `--parallel N` sets `maxParallel` (default 2;
  `0` forces everything into the main checkout). `--branch NAME` overrides
  `phase-<N>` (dots become dashes). `--skip a,b` leaves tasks out of this
  pass. `--only a,b` runs just those tasks.

## 2. Preflight (inline, cheap)

1. `git status --porcelain`. If anything is dirty, show it and stop until the
   user commits or stashes it — worktree agents only see committed files.
2. `git fetch origin`. If the branch exists on origin, check it out and
   `git pull --ff-only`; otherwise `git checkout -b <branch> origin/main` and
   `git push -u origin <branch>`. Agents push to this branch, never to `main`.
3. `cargo build --workspace` once so the main checkout's `target/` is warm.
4. Note the absolute repo root (`pwd -W` in Git Bash) for `repoRoot`.

## 3. Plan

Look for a saved plan at `.claude/plans/<branch>.json` (gitignored). It holds
`{ plan, planBase, done: [task ids approved so far] }`.

- **None:** record `planBase = git rev-parse HEAD`, run
  `Workflow({ scriptPath: ".claude/workflows/implement-phase.js", args: { phase, branch, planOnly: true, skip } })`,
  and wait for its notification.
- **Saved:** reuse it. Do not re-plan unless the user asks.

Show the plan compactly: phase title, summary, skipped items, then a table of
`id | item | title | depends_on | parallel | risk | files`. Unless `--yes`,
ask the user to approve, edit (drop/merge tasks, change parallel or risk
flags) or cancel. Apply edits to the plan object yourself, then write the
file.

## 4. Run

Never paste the briefs into the call — they are long, and you would be
restating in Opus output what is already on disk. Pass a **skeleton** (every
task's `id, item, title, depends_on, parallel_safe, risk, files`, done ones
included) plus `planFile`; each agent prints its own brief with
`.claude/scripts/brief.py`. Build the skeleton with a one-line Python
script and copy its compact JSON.

```
Workflow({ scriptPath: ".claude/workflows/implement-phase.js", args: {
  phase, branch, repoRoot, maxParallel,
  plan: <skeleton>,
  planFile: "<repoRoot>/.claude/plans/<branch>.json",
  planBase,                          // from the saved plan
  refresh: <planBase != HEAD and HEAD has code commits since>,
  doneEarlier: saved.done,           // approved in an earlier pass
  skip,                              // --skip, or everything not in --only
  reviewOnly                         // { id: [sha] } for work that landed but was never approved
} })
```

Tell the user it is running and that `/workflows` shows live progress; then
end your turn and wait for the notification. Never predict results.

## 5. Interruptions

If the result has `interrupted: true` (quota, the process exited, an agent was
skipped) or the notification says the run stopped:

1. `git status --porcelain` in the main checkout. Leftover changes belong to
   the interrupted agent: `git stash push -u -m "partial <task> from interrupted run"`.
2. `git fetch origin && git log --oneline <last known head>..origin/<branch>`
   to see what landed.
3. Resume with `Workflow({ scriptPath, resumeFromRunId, args: <same args> })` —
   finished agents replay from cache. If the user hit a quota limit, tell them
   when it resets and resume only after that.

## 6. Report

When the workflow returns:

1. Add every `done` task id to `done` in the saved plan file.
2. One line: phase title, branch, counts (✅ done / ⚠️ needs-attention /
   ❌ failed / 📤 push-failed / ⛔ blocked / ⏸️ interrupted / ⏳ not-started).
3. The returned `table` verbatim.
4. Final check suite on the branch head (`final.checks`, `final.test_count`,
   `final.head`), or why finalize was skipped.
5. Short bulleted sections, only if non-empty: briefs the refresh changed,
   open review issues per unfinished task, follow-ups the implementers
   deferred, reviewer nits.
6. Next step: for ⚠️/❌/⛔ tasks, offer to re-run just those (`--only`); for 📤,
   push by hand and offer `reviewOnly`. When all is green, offer to open a PR
   from the branch to `main` — ask first; do not open it unprompted.
