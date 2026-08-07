# velo-core

The embeddable core of [Velo](https://github.com/LucasVascovici/velo): a
content-addressed repository with history, branching, three-way merge and sync.

This crate is the library half. It has no terminal output at all — the boundary
is enforced by `#![deny(clippy::print_stdout, clippy::print_stderr)]` — so every
command returns data and the caller decides how, or whether, to show it. That
makes it usable from a GUI, a server, or a program with no working tree at all.

```rust
use velo_core::{commands, Repo};

let mut repo = Repo::discover(std::path::Path::new("."))?;

// Read commands take &Repo.
let history = commands::history::run(&repo, commands::history::Options {
    limit: Some(20),
    ..Default::default()
})?;

// Write commands take a guard, which holds the repository lock.
let guard = repo.write()?;
let saved = commands::save::run(&guard, Some("a message"), Default::default())?;
# Ok::<(), velo_core::Error>(())
```

## What you get

- **Snapshots without a staging area.** What is on disk is what is recorded.
- **Content-addressed storage.** BLAKE3 object hashes, Zstd compression, an
  SQLite index in WAL mode.
- **A merge engine that reports rather than writes.** `merge::plan` classifies
  every file before anything touches the working tree, so a consumer can present
  the outcome first.
- **Progress and cancellation per call.** Long operations take an `Observer` and
  a `Cancel` on the call, not on the handle — no globals.
- **No working tree required.** `save_tree` records a snapshot from an in-memory
  file set, which is how a document editor or a registry uses this crate.

## Features

Nothing is on by default: an embedder should not pay for the CLI's dependencies.

| Feature | What it adds |
| :--- | :--- |
| `bundle` | Offline history transfer — create and apply a bundle file |
| `ssh` | Sync over a spawned server process (`ssh://`, `child:`) |

## Format

Repositories use format v2. The normative specification is
[`docs/FORMAT.md`](https://github.com/LucasVascovici/velo/blob/main/docs/FORMAT.md);
format changes are called out separately from API changes in the
[changelog](https://github.com/LucasVascovici/velo/blob/main/CHANGELOG.md).

## A note on what this is

Velo was vibe-coded for fun — a real working tool, built as an experiment in a
tight loop with an AI assistant, not as a production-grade Git replacement.

MIT licensed.
