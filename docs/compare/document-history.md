# Document history: velo against Git

The audience: someone building an editor, a CMS or a knowledge base where drafts
are branches and readers ask who wrote each line. Summary page:
[Is velo for you?](../IS_VELO_FOR_YOU.md).

**No document-history benchmark was run, against Git or anything else.** The
repository holds no timing of an editing workload over prose, no comparison of
`git blame` and `velo blame` on documents, and no disk comparison for documents.
Every cell that would compare them is **not measured**, and this page does not
estimate one.

| Question | velo | Git | Source |
| --- | --- | --- | --- |
| Time to save a revision of one document | not measured | not measured | none |
| Time to open a draft branch and merge it back | not measured | not measured | none |
| Time to answer "who wrote this line" on documents | not measured | not measured | none |
| Disk for a document history | not measured | not measured | none |
| Behaviour at many thousands of documents | see below | not measured | [SCALE.md](../SCALE.md) |

## The nearest measurements

These use files of about 1 KB edited a few lines at a time, not documents, and
are the only evidence of how the engine behaves on line-edited text:

- Who wrote this line, shadow git against velo, 1,000 files by 500 steps:
  3.77 s for 20 queries in Git and 3.29 s in velo (Windows 11, Defender on,
  2026-10-22,
  [2026-10-22-windows-agent-who-wrote.json](../../bench/results/2026-10-22-windows-agent-who-wrote.json)).
  Velo was 3.1 times slower on 2026-10-20 (9.30 s against 3.01 s,
  [PERF.md](../PERF.md), section 6), and the fix is recorded in section 11. On
  Linux after the fix the comparison is not measured.
- Disk for that workload: velo 11.07 MB against Git's 3.69 MB with the handle
  open, or 2.10x Git's size at rest in a separate seed-0 run (Windows,
  2026-10-22,
  [2026-10-22-agent-disk-breakdown.txt](../../bench/results/2026-10-22-agent-disk-breakdown.txt)).
- Scale of velo alone: 10,000 files by 200 snapshots saved in 0.23 s median per
  save, and a 30,000-file run saved flat at 0.41 s (Windows, 2026-10-22,
  [SCALE.md](../SCALE.md)). A repository of 100,000 files was not completed on
  6.x.

## What is different, by design

- Every snapshot's whole file list is stored and queryable, so the history of
  one file does not walk commits.
- The author and any metadata (for example an editor session or an approval
  state) are part of the snapshot id, so they can be checked with `velo fsck` and
  signed ([INTEGRITY.md](../INTEGRITY.md)).
- Blame is per line ([cookbook](../cookbook/document-editor.md)).

What Git has that velo does not: hosting, review tools, an enormous ecosystem and
a store that is smaller in every measured row. If your document history already
lives in a Git host and nobody asks who wrote a line beyond the commit author,
there is no measured reason to move it.
