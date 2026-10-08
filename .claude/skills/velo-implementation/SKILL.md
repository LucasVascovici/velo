---
name: velo-implementation
description: Procedure for implementing one planned task in the velo Rust workspace — branch setup (main checkout or worktree), coding conventions, the check suite, commit message style, and the rebase-and-push protocol. Preloaded by the phase-implementer agent.
user-invocable: false
---

# Implementing a velo task

Your prompt gives you `BRANCH` (the phase branch, e.g. `phase-14`), `MODE`
(`main` or `worktree`), and in worktree mode `TARGET_DIR`.

Tokens are the budget. Every command's output stays in your context for the
rest of the task, so keep output short (filter it) and run each expensive
command as few times as the work allows.

## 1. Setup

- **MODE=main** — you are in the main checkout, on `BRANCH`.
  1. `git status --porcelain`. If anything is listed, it is left over from an
     interrupted agent, not yours: `git stash push -u -m "orphan before <task id>"`
     and mention it in `notes`. Never build on it, never delete it.
  2. `git pull --rebase origin BRANCH`.
  Never commit `ARCHITECTURE.md` or `CHANGELOG.md`.
- **MODE=worktree** — you are in a fresh git worktree on a throwaway branch.
  `git fetch origin BRANCH && git reset --hard origin/BRANCH` first (safe: the
  worktree is new). Prefix **every** cargo command with
  `CARGO_TARGET_DIR=<TARGET_DIR>` — that directory is reused across tasks in
  your lane, so builds are incremental instead of cold.

**Already done?** A rerun after an interruption can hand you a task that is
already on the branch. Check first (`git log --oneline origin/BRANCH` for a
commit matching the task). If it is there and complete, change nothing:
report `status: "done"` with **those commits** (`pushed: true`) and say so in
`notes`. Never report an empty commit list for finished work.

If your prompt lists commits that landed after the brief was written, read
them (`git show --stat`) before coding. Where the brief and the code
disagree, follow the code and keep the brief's intent; say so in `notes`.

## 2. Implement

- Read the code you are changing first, and match it: naming, error style,
  doc-comment density (velo documents *why*, in full sentences).
- `velo-core` is a library: `#![deny(clippy::print_stdout, clippy::print_stderr,
  clippy::exit)]`. Errors are typed `VeloError` variants, not strings.
- Public enums and option structs are `#[non_exhaustive]`; options structs get
  a `Default`.
- Any change to what is stored on disk or in bundles updates `docs/FORMAT.md`
  in the same commit.
- Mind `#[cfg(unix)]` / `#[cfg(not(unix))]` pairs — you are on Windows, so the
  Unix branch is not compiled locally; keep both sides symmetric by reading.
- **Edit with the Edit tool.** If you script an edit in Python, open files with
  `newline=''` on both read and write; a plain `open(..., 'w')` on Windows
  rewrites every line ending and turns the diff into the whole file.

## 3. Tests

Write tests that would fail without your change.

- **Core:** a new feature gets its own self-contained module in
  `crates/velo-core/src/tests/<feature>.rs` (its own `setup()`, its own `use`
  lines — see `tests/compaction.rs`), declared by appending
  `#[cfg(test)]\nmod <feature>;` to the end of `crates/velo-core/src/tests.rs`.
  Extend an existing module only when the brief says to. Do not read
  `tests.rs` whole — it is 12,000 lines; grep it.
- CLI black-box: `crates/velo-cli/tests/cli.rs`. Merge engine:
  `crates/velo-merge/tests/`. Shared fixtures: `crates/velo-testkit`.

## 4. Checks

While developing, run only what you touched, with a timeout and filtered output:

```bash
timeout 600 cargo test -p velo-core <module_or_test_name> 2>&1 | grep -E "^test result|FAILED|panicked|^error" | head -20
```

When the work is done, run the full gate **once**, in this order, stopping at
the first failure:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | grep -E "^(warning|error)" -A5 | head -40
timeout 900 cargo test --workspace --locked 2>&1 | grep -E "^test result|FAILED|panicked|^error" | head -30
```

If you changed CLI behaviour, also run `timeout 900 ./workflow_sim.sh 2>&1 | tail -5`.

- **Never** run cargo in the background or poll for it. Always use `timeout`;
  a test that times out is a hang — find it with a targeted run, do not
  rerun the suite hoping it passes.
- If `--locked` fails because you added a dependency the brief allowed, run
  `cargo build` once to update `Cargo.lock` and commit it.
- Fix failures; never weaken or delete existing tests to get green. If you
  cannot get green, stop: do not commit, report `failed`.

## 5. Commit

Stage only your files by path (`git add <paths>`; never `git add -A`).
Message style matches the history — a plain-English title saying what changed
(no `feat:` prefixes, ≤ 72 chars), a blank line, then prose explaining **why**
and any non-obvious decision. End with:

```
Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
```

Use a heredoc or `git commit -F <file>` for multi-line messages. One commit per
task is the norm; a fix round adds a new commit, never amends a pushed one.

## 6. Push

```bash
git pull --rebase origin BRANCH
git push origin HEAD:BRANCH
```

- If the rebase conflicts in a **shared file** (`tests.rs`, `commands/mod.rs`,
  `lib.rs`, `error.rs`, `Cargo.toml`, `Cargo.lock`, `velo-cli/src/main.rs`) and
  both sides only *added* lines — a `mod` line, an enum variant, a dependency,
  a match arm — keep both, then re-run the full gate before continuing. For
  `Cargo.lock`, take theirs and run `cargo build` to regenerate.
- A conflict that changes existing lines you do not own: `git rebase --abort`
  and report `failed` with the paths.
- If the push itself fails (network, server error), retry once. If it fails
  again, **stop** — report `status: "done"`, `pushed: false`. A separate step
  delivers it; do not loop. Never force-push.

## 7. Report

Return the structured result: status, `pushed`, every commit SHA you made
(`git log --format='%H %s' origin/BRANCH..HEAD` before pushing, or the SHAs
after), files changed, number of tests added, each check's result, a two-line
summary, and notes/follow-ups (anything you deferred, any place the brief
disagreed with the code).
