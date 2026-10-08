# velo-core

The engine of [Velo](https://github.com/LucasVascovici/velo), a timeline engine
you can embed: a content-addressed repository with history, branching,
three-way merge and sync.

- **Verifiable whole-tree snapshots** — content-addressed, `fsck` recomputes every id.
- **Tamper-evident provenance** — hashed, namespaced metadata; authorship; parent-aware, rename-aware blame with author and branch per line.
- **No working tree required** — `save_tree` / `tree_at`, caller-supplied timestamps, `Repo::scoped`.
- **Explicit divergence** — branches, a pure merge engine, resumable resolutions, undo/redo, sync that refuses rather than guesses.
- **Trivial to deploy** — one SQLite file, synchronous core, typed errors, per-call progress and cancellation.

Anything that has to answer *"what did this look like at T, who changed it, why,
and can I prove it?"* is a candidate.

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

## From other languages

All of these are layers over this crate, in the
[repository](https://github.com/LucasVascovici/velo). They are built from
source; nothing is published yet.

- [`bindings/python`](https://github.com/LucasVascovici/velo/tree/main/bindings/python) — PyO3, built with maturin.
- [`bindings/node`](https://github.com/LucasVascovici/velo/tree/main/bindings/node) — napi-rs.
- [`bindings/wasm`](https://github.com/LucasVascovici/velo/tree/main/bindings/wasm) — wasm-bindgen over single-file repositories.
- [`crates/velo-ffi`](https://github.com/LucasVascovici/velo/tree/main/crates/velo-ffi) — a C ABI with `include/velo.h`.
- [`crates/velo-mcp`](https://github.com/LucasVascovici/velo/tree/main/crates/velo-mcp) — a stdio MCP server for agent checkpointing.

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
