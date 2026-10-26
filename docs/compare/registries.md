# Registries: Velody against an append-only log or a SQLite store

The audience: someone keeping prompts, configs or policies that are published,
tagged, rolled back and audited. Summary page:
[Is velody for you?](../IS_VELODY_FOR_YOU.md).

**No registry-shaped benchmark was run.** There is no measured publish, lookup
or rollback timing for a registry of prompts. The only measured data that
includes an append-only log and a SQLite store is the agent benchmark
([method](../../bench/agent/README.md)), whose "jsonl log" and "sqlite blobs"
rows are the closest, and they are an agent workload, not a registry one: 1,000
files, 500 steps. Read the table as what a log and a SQLite store cost for that
shape, not for yours.

## From the agent benchmark

File: [agent-2026-10-16-windows.json](../../bench/results/agent-2026-10-16-windows.json),
Windows 11, Defender on, 2026-10-16, all systems in one run. Velody in this run is
format v3, before v4, so its disk and step figures are out of date. The current
velody figures, from other runs on the same machine, are beside it.

| | velody, 2026-10-16 (v3) | velody, 2026-10-20 (v4) | jsonl log | sqlite blobs |
| --- | ---: | ---: | ---: | ---: |
| 500 steps | 64.4 s | 19.4 s | 1.39 s | 8.84 s |
| restore step 250, median | 216 ms | 480 ms | 705 ms | 738 ms |
| who wrote this line, x20 | 2.95 s | 9.30 s (3.29 s on 2026-10-22) | 0.55 s | 0.002 s |
| disk after steps | 152.3 MB | 11.03 MB | 6.35 MB | 28.5 MB |
| verify a step afterwards | ids recompute, signatures | same | not possible | not possible |

The v4 column is from [2026-10-20-windows-agent-velody.json](../../bench/results/2026-10-20-windows-agent-velody.json)
and [2026-10-22-windows-agent-who-wrote.json](../../bench/results/2026-10-22-windows-agent-who-wrote.json).
The 480 ms restore moved with the machine's drift that day (shadow git went from
422 ms to 480 ms as well) and is not a regression finding
([PERF.md](../PERF.md), section 6). Columns from different days do not compare
cleanly.

Plainly: a JSONL log writes a step about 14 times faster and uses about 60% of
velody's disk, and SQLite answers "who wrote" from its own manifest in
milliseconds. Neither has branches, a merge, per-snapshot verification or a
rollback that anything checks. Linux, Windows with the latest build, and any
registry-sized repository (thousands of small documents, many tags) are
**not measured** for these systems.

## From the prompt registry example

[examples/prompt-registry/FINDINGS.md](../../examples/prompt-registry/FINDINGS.md)
is a record of building a registry on `velody-core`, with no timings: publish,
history, tag, retrieval by tag or hash prefix and durable reopening came to about
200 lines. Snapshot metadata replaced encoding state in commit messages, and
carrying the other files forward by reference (`TreeEntry::stored`) means a
publish does not decompress and rehash the whole registry. How much that saves
was **not measured**.

## What Velody adds, and what it does not

- Each version's author and metadata are hashed into its id, so "which version of
  this prompt was live on Tuesday, and who published it" is checkable by
  `velody fsck`, and a version can be signed ([INTEGRITY.md](../INTEGRITY.md)). A log
  or table you write yourself can hold the same fields, but nothing detects an
  edit to them.
- Tags and branches give a rollback and a staging line without extra schema.
- It does not store signing keys or revoke them.
- It is a larger store than a log: see the disk row.

Choose an append-only log or SQLite if you need the cheapest write and nobody
needs to prove the history later. See the
[config registry cookbook](../cookbook/config-registry.md) for the velody shape.
