<p align="center">
  <img src="https://img.shields.io/github/v/release/LucasVascovici/velo?color=orange&logo=github&label=latest" />
  <img src="https://github.com/LucasVascovici/velo/actions/workflows/ci.yml/badge.svg" />
  <img src="https://github.com/LucasVascovici/velo/actions/workflows/release.yml/badge.svg" />
  <img src="https://img.shields.io/github/license/LucasVascovici/velo?color=blue" />
</p>

<h1 align="center">⚡ Velo</h1>

<p align="center">
  <strong>A timeline engine: verifiable whole-tree snapshots, provenance and
  explicit divergence, in one SQLite file.</strong><br/>
  An embeddable Rust library first, with bindings, an MCP server and a CLI on top.
</p>

<p align="center">
  <em>v4 — a new snapshot format, an API a program can hold, and merges that
  remember what you already resolved.</em>
</p>

> **Note:** Velo was vibe-coded for fun — high-level intent, modern tech stack, tight feedback loop with an AI assistant. It's a real working tool, but built as an experiment in what's possible with that workflow, not as a production-grade Git replacement.

---

---

## What velo is

Velo is a timeline engine: a program (or a person) records verifiable whole-tree
snapshots, branches them, merges them and asks questions of the result, all in
one SQLite file. It is an embeddable Rust library first, with Python, Node, C and
WebAssembly bindings, an MCP server and a Git-shaped CLI on top. To see who wrote
each line of an agent run, open the [sample `velo report`](docs/report-sample.html).

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
velo init
echo "hello world" > app.py
velo save "Initial commit"
velo switch feature/login          # a branch per attempt
echo "login" >> app.py
velo save "Add login page"
velo switch main
velo merge feature/login           # conflicts only where changes really overlap
velo history                       # who changed what, and when
velo fsck                          # recompute every id and prove nothing changed
```

## Install

```bash
cargo install --git https://github.com/LucasVascovici/velo velo-cli
# or: curl -fsSL https://raw.githubusercontent.com/LucasVascovici/velo/main/install.sh | sh
```

Everything is built from source; nothing is published to crates.io, PyPI or npm
yet. The bindings, release assets and Windows are in [docs/INSTALL.md](docs/INSTALL.md).

## Where to go next

| You are | Read |
| :--- | :--- |
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
