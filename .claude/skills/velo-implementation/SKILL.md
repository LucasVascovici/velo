---
name: velo-implementation
description: Procedure for implementing one planned task in the velo Rust workspace — branch setup (main checkout or worktree), coding conventions, the check suite, commit message style, and the rebase-and-push protocol. Preloaded by the phase-implementer agent.
user-invocable: false
---

# Implementing a velo task

Your prompt gives you `BRANCH` (the phase branch, e.g. `phase-14`) and `MODE`
(`main` or `worktree`).

## 1. Setup

- **MODE=main** — you are in the main checkout, already on `BRANCH`.
  `git pull --rebase origin BRANCH` so you start on top of anything parallel
  tasks pushed. Never touch files outside your task; never commit
  `ARCHITECTURE.md` or `CHANGELOG.md`.
- **MODE=worktree** — you are in a fresh git worktree on a throwaway branch.
  `git fetch origin BRANCH && git reset --hard origin/BRANCH` before anything
  else (safe: the worktree is new and empty of work). The `target/` dir is
  cold here, so the first build is slow — that is expected.

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

## 3. Tests

Write tests that would fail without your change. Locations:
`crates/velo-core/src/tests.rs` (core, in-process), `crates/velo-cli/tests/cli.rs`
(CLI black-box), `crates/velo-merge/tests/` (merge engine), shared fixtures in
`crates/velo-testkit`.

## 4. Checks — all must pass before committing

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --locked
```

If you changed CLI behaviour, also run `./workflow_sim.sh` (Git Bash). If
`--locked` fails because you added a dependency, run `cargo build` once to
update `Cargo.lock` and commit it — but only if the brief allowed the dep.

Fix failures; do not weaken or delete existing tests to get green. If you
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

If the push is rejected, repeat both (up to 3 times). If the rebase conflicts
in files you own, resolve, re-run the checks, continue. If it conflicts in
files you do not own, `git rebase --abort` and report `failed` with the
conflicting paths. Never force-push.

## 7. Report

Return the structured result: status, every commit SHA you pushed (`git rev-parse
HEAD` after push), files changed, number of tests added, each check's result,
a two-line summary, and notes/follow-ups (anything you deferred, any place the
brief disagreed with the code).
