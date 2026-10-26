<p align="center">
  <img src="https://img.shields.io/github/v/release/LucasVascovici/velody?color=orange&logo=github&label=latest" />
  <img src="https://github.com/LucasVascovici/velody/actions/workflows/ci.yml/badge.svg" />
  <img src="https://github.com/LucasVascovici/velody/actions/workflows/release.yml/badge.svg" />
  <img src="https://img.shields.io/github/license/LucasVascovici/velody?color=blue" />
</p>

<h1 align="center">⚡ Velody</h1>

<p align="center">
  <strong>A timeline engine: verifiable whole-tree snapshots, provenance and
  explicit divergence, in one SQLite database.</strong><br/>
  An embeddable Rust library first, with bindings, an MCP server and a CLI on top.
</p>

<p align="center">
  <em>delta snapshots, signed history, an API a program can hold, and merges
  that remember what you already resolved.</em>
</p>

> **Formerly Velo.** From 7.0.0 the project, the command (`velody`), the crates, the Python and
> npm packages and the MCP tools are named Velody; `velo` still runs as an alias for one release. The
> `.velo` folder, `.veloignore`, snapshot ids and the wire format are unchanged, so a repository, a
> bundle or a remote written by 6.x works as it is. Environment variables begin `VELODY_` (the old
> `VELO_` spelling is still read).

> **Note:** Velody was vibe-coded for fun — high-level intent, modern tech stack, tight feedback loop with an AI assistant. It's a real working tool, but built as an experiment in what's possible with that workflow, not as a production-grade Git replacement.
>
> The [production checklist](docs/PRODUCTION.md) judges each line against its evidence; 1 of its 11 lines is red.

---

---

## What Velody is

Velody is a timeline engine: a program (or a person) records verifiable whole-tree
snapshots, branches them, merges them and asks questions of the result, all in
a SQLite database (a `.velo` folder, or one file in store-only mode). It is an embeddable Rust library first, with Python, Node, C and
WebAssembly bindings, an MCP server and a Git-shaped CLI on top. To see who wrote
each line of an agent run, open the [sample `velody report`](docs/report-sample.html).

## Who it is for, and who it is not for

| Use | Fit |
| :--- | :--- |
| AI agent workspaces and checkpointing: a snapshot per tool call, a branch per attempt | Excellent, the wedge |
| Prompt, config and policy registries: publish, tag, roll back, audit | Excellent |
| Document and knowledge editors: drafts as branches, per-line provenance | Very good for text |
| Regulated audit trails: recomputable ids, signed snapshots | Good; no key storage or revocation |
| Local-first apps, headless CMS, reproducible research | Medium |

**Not a fit:** real-time co-editing (CRDT territory), high-frequency event
sourcing, large binary assets (game art, CAD, datasets), and use as a database.
The full table is in [Internals](docs/INTERNALS.md#where-it-fits).

## Ten-line example

```bash
velody init
echo "hello world" > app.py
velody save "Initial commit"
velody switch feature/login          # a branch per attempt
echo "login" >> app.py
velody save "Add login page"
velody switch main
velody merge feature/login           # conflicts only where changes really overlap
velody history                       # who changed what, and when
velody fsck                          # recompute every id and prove nothing changed
```

## Install

```bash
cargo install --git https://github.com/LucasVascovici/velody velody-cli
# or: curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velody/main/install.sh | sh
```

Everything is built from source; nothing is published to crates.io, PyPI or npm
yet. The bindings, release assets and Windows are in [docs/INSTALL.md](docs/INSTALL.md).

## Where to go next

| You are | Read |
| :--- | :--- |
| Deciding | [Is velody for you?](docs/IS_VELODY_FOR_YOU.md), with comparisons for [agent checkpointing](docs/compare/agent-checkpointing.md), [registries](docs/compare/registries.md) and [document history](docs/compare/document-history.md) |
| An agent builder | [Library and bindings, MCP server](docs/LIBRARY.md), [agent checkpointing cookbook](docs/cookbook/agent-checkpointing.md) |
| An app builder | [Library and bindings](docs/LIBRARY.md), [STABILITY.md](docs/STABILITY.md), [FORMAT.md](docs/FORMAT.md) |
| A CLI user | [The CLI](docs/CLI.md), [Collaboration](docs/COLLABORATION.md), [Integrity and signing](docs/INTEGRITY.md) |
| An operator | [Install](docs/INSTALL.md), [hosting a remote](docs/COLLABORATION.md#hosting-a-remote), [THREAT_MODEL.md](docs/THREAT_MODEL.md), [SCALE.md](docs/SCALE.md) |
| A contributor | [CONTRIBUTING.md](CONTRIBUTING.md), [Internals](docs/INTERNALS.md), [ROADMAP.md](docs/ROADMAP.md), [SUPPORT.md](SUPPORT.md) |

Every page is indexed in [docs/README.md](docs/README.md). What changed is in
[CHANGELOG.md](CHANGELOG.md); what is promised to stay stable is in
[STABILITY.md](docs/STABILITY.md).

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE).
