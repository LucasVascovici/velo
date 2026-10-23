# Friction log

Template. One row per event the agent hit; copy this file to
`friction-log-<tag>.md` for the first round and `friction-log-<tag>-round2.md`
for round two (a separate file; keep both). Empty until the owner runs the
protocol in [`README.md`](README.md); nothing below is a result.

Release tag `____`, date `____`. Two runs per wedge, one fresh agent each.

Kind is one of `guess` (the agent had to guess at something the docs did not
say), `error` (a command or call failed) or `missing method` (it reached for a
method, flag or tool that does not exist). `Run` is the run number from
`run.json`. `Defect` is a short stable name (for example `no-sync-example`), the
same string on every row that hits the same defect, because `rank.py` counts
distinct runs per name.

| Run | Wedge | Minute | Kind | Quote | Defect | Doc or API defect | Proposed fix | Deferred or fixed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
|  |  |  |  |  |  |  |  |  |

## Per-run summary

Validity is `valid`, or `invalid: <audit reason>` when `audit.py` exited 1;
an invalid run is repeated and not counted toward the threshold.

| Run | Wedge | Validity | Minutes to first successful save | Timed out at 45 min | check.py result | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Python agent checkpointing |  |  |  |  |  |
| 2 | Python agent checkpointing |  |  |  |  |  |
| 1 | Node config registry |  |  |  |  |  |
| 2 | Node config registry |  |  |  |  |  |
| 1 | Rust document editor |  |  |  |  |  |
| 2 | Rust document editor |  |  |  |  |  |
| 1 | MCP client |  |  |  |  |  |
| 2 | MCP client |  |  |  |  |  |

## Ranked

Paste the output of `python rank.py <this file>` here: defects ordered by the
number of distinct runs that hit them.

| Defect | Runs hit | Kinds | Wedges |
| --- | --- | --- | --- |
