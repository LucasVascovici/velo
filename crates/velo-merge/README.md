# velo-merge

The three-way (diff3) merge engine behind
[Velo](https://github.com/LucasVascovici/velo).

Pure computation: no I/O, no database, no dependency on the rest of Velo. Give it
three versions of a text and it tells you what merges and what does not.

```rust
let merged = velo_merge::try_auto_merge(
    "one\ntwo\nthree\n",   // ancestor
    "one\nOURS\nthree\n",  // ours
    "one\ntwo\nTHEIRS\n",  // theirs
);
assert_eq!(merged.as_deref(), Some("one\nOURS\nTHEIRS\n"));
```

Changes on both sides merge cleanly when they do not overlap. When they do,
`try_auto_merge` returns `None` and the hunk-level API takes over:
`compute_conflict_hunks` segments the file into conflicting regions, and
`build_resolved_content` rebuilds the file once a decision has been recorded for
each — which is what drives Velo's interactive resolver. The segmentation is
shared between the two, so hunks and the final content always line up.

MIT licensed.
