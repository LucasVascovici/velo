---
name: implement-phase
description: Implement a phase or item of ARCHITECTURE.md end to end — Opus plans it into tasks, Sonnet subagents implement, test, commit and push each one, Opus reviews each, and you get a summary table. Use when the user says "implement phase 14", "do 14.3", "/implement-phase 14".
argument-hint: <phase|item> [--yes] [--parallel N] [--branch NAME] [--skip id,id]
disable-model-invocation: true
---

# /implement-phase

You (Opus, the main loop) are the orchestrator. The heavy lifting runs in the
`implement-phase` workflow at `.claude/workflows/implement-phase.js`, which uses
the `phase-planner` (Opus), `phase-implementer` (Sonnet) and `phase-reviewer`
(Opus) agents. Do not implement tasks yourself.

Arguments: `$ARGUMENTS`

## 1. Parse

- First positional: the phase (`14`) or item (`14.3`). Required — ask if missing.
- `--yes` skips plan approval. `--parallel N` sets `maxParallel` (default 2;
  `0` forces everything sequential in the main checkout). `--branch NAME`
  overrides the default `phase-<N>` (dots become dashes). `--skip a,b` drops
  task ids.

## 2. Preflight (inline, cheap)

1. `git status --porcelain`. If anything is dirty, show it and stop until the
   user commits or stashes it — worktree agents only see committed files, and
   the planner must read the same `ARCHITECTURE.md` they will.
2. `git fetch origin`. If the branch exists on origin, check it out and
   `git pull --ff-only`; otherwise `git checkout -b <branch> origin/main` and
   `git push -u origin <branch>`. Agents push to this branch, never to `main`.
3. `cargo build --workspace` once so the main checkout's `target/` is warm.

## 3. Plan

Run `Workflow({ scriptPath: ".claude/workflows/implement-phase.js", args: { phase, branch, planOnly: true, skip } })`
and wait for its notification.

Show the plan compactly: phase title, summary, skipped items, then a table of
`id | item | title | depends_on | parallel? | files`. Unless `--yes`, ask the
user to approve, edit (drop/merge tasks, change parallel flags) or cancel.
Apply edits to the plan object yourself.

## 4. Run

Run `Workflow({ scriptPath: ".claude/workflows/implement-phase.js", args: { phase, branch, maxParallel, plan } })`
passing the **approved plan object** so the planner does not run again. Tell
the user it is running and that `/workflows` shows live progress; then end
your turn and wait for the notification. Never predict results.

## 5. Report

When the workflow returns, print:

1. One line: phase title, branch, counts (✅ done / ⚠️ needs-attention /
   ❌ failed / ⛔ blocked).
2. The returned `table` verbatim.
3. Final check suite on the branch head (`final.checks`, `final.test_count`,
   `final.head`), or why finalize was skipped.
4. Short bulleted sections, only if non-empty: open review issues per
   unfinished task, follow-ups the implementers deferred, reviewer nits.
5. Next step: for ⚠️/❌/⛔ tasks, offer to re-run just those
   (`args.plan` with only those tasks + `skip` the rest). When all is green,
   offer to open a PR from the branch to `main` — ask first; do not open it
   unprompted.
