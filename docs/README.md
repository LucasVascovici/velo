# velo documentation

Every page in this directory, by who it is for. The front page is the
[README](../README.md).

## Deciding

- [IS_VELO_FOR_YOU.md](IS_VELO_FOR_YOU.md): where velo is slower and faster than Git, what it costs in disk, and what only it gives you, from the measured benchmarks.
- Comparisons: [agent checkpointing](compare/agent-checkpointing.md) (shadow Git and `git worktree`), [registries](compare/registries.md) (an append-only log or SQLite), [document history](compare/document-history.md) (Git).

## Agent builders

- [LIBRARY.md](LIBRARY.md): the Rust library and the Python, Node, C and WebAssembly bindings, and the MCP server.
- Five-minute quickstarts, each run in CI from the built packages: [Python agent checkpointing](quickstart/python-agent-checkpointing.md), [Node config registry](quickstart/node-config-registry.md), [Rust document editor](quickstart/rust-document-editor.md) and [MCP](quickstart/mcp.md).
- [cookbook/](cookbook/): worked recipes, starting with [agent checkpointing](cookbook/agent-checkpointing.md).
- [report-sample.html](report-sample.html): a sample `velo report` of an agent run; a static page that works offline.

## App builders

- [LIBRARY.md](LIBRARY.md): the per-language guides.
- [cookbook/](cookbook/): [config registry](cookbook/config-registry.md), [document editor](cookbook/document-editor.md).
- [STABILITY.md](STABILITY.md): what is promised to stay stable.
- [FORMAT.md](FORMAT.md): the on-disk and bundle formats.
- [NAMES.md](NAMES.md): the naming rules for branches, tags and paths.

## CLI users

- [CLI.md](CLI.md): why velo, velo versus Git, the command reference, exit codes and the merge workflow.
- [COLLABORATION.md](COLLABORATION.md): clone, push, pull, diverging, bundles and hosting a remote.
- [INTEGRITY.md](INTEGRITY.md): integrity checks, signing, and leaving velo and coming back.
- [WINDOWS.md](WINDOWS.md): notes for Windows.
- [demo/](demo/): the scripted demo, with its [transcript](demo/transcript.md).

## Operators

- [INSTALL.md](INSTALL.md): every install path.
- [CRASH_SAFETY.md](CRASH_SAFETY.md): what survives a crash and what does not.
- [THREAT_MODEL.md](THREAT_MODEL.md): what velo defends against.
- [SCALE.md](SCALE.md): the measured scale envelope.
- [PERF.md](PERF.md): where the time goes.
- [PRODUCTION.md](PRODUCTION.md): the production checklist, each line green with its evidence or red with its reason.
- [SIMULATION.md](SIMULATION.md): what each team simulation run exercised, found and fixed, and the plan for the next one.
- [SECURITY.md](../SECURITY.md): reporting a vulnerability.

## Contributors

- [CONTRIBUTING.md](../CONTRIBUTING.md), [SUPPORT.md](../SUPPORT.md) and [CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md).
- [INTERNALS.md](INTERNALS.md): what velo is, where it fits, the architecture and the repository layout.
- [ROADMAP.md](ROADMAP.md): what is planned and what was decided.
- [REVIEW_KIT.md](REVIEW_KIT.md): the external-review kit.
- [CHANGELOG.md](../CHANGELOG.md): what changed, release by release.

## This site

These pages and the generated Python and Node API reference are built into a static site by `.github/workflows/docs-site.yml` on every pull request, with an offline link check. Links that leave `docs/` (the front README, CHANGELOG, `bench/`, `examples/`) are rewritten to GitHub URLs in the built site; the non-markdown files here are copied in unchanged. Publishing is the owner's switch: turn on Pages (Settings, Pages, source: GitHub Actions) and set the repository variable `DOCS_PAGES` to `true`; the next push to `main` deploys.
