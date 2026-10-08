# Cookbook: document editor

Script: [`bindings/python/examples/document_editor.py`](../../bindings/python/examples/document_editor.py)

A collaborative editor on velo: drafts are branches, saving is a snapshot, and
blame tells you who wrote each sentence.

## One sentence per line

Merge and blame work on lines, so store one sentence per line. A reworded
sentence then conflicts, and is attributed, as a unit.

## Rename with an edge

When a document is renamed, record the edge so history follows it:

```python
repo.save_tree(branch="main", message="rename draft to essay", parent=base,
               entries={"essay.txt": TEXT}, renames=[("draft.txt", "essay.txt")])
```

The script asserts that blame of line 1 of `essay.txt` resolves to the original
snapshot and to the path `draft.txt`.

## Drafts as branches

A collaborator forks a draft branch (`repo.create_branch("draft-grace", at=...)`)
and saves to it. Meanwhile the author edits the same sentence on `main`.

## Merge, and resolve a conflict with bytes

```python
plan = repo.merge_plan(m1, g)       # plan.is_clean is False
repo.merge_commit(branch="main", ours=m1, theirs=g, message="merge",
                  resolutions={"essay.txt": resolved_bytes})
```

Without a resolution, `merge_commit` raises `velo.Conflicts` with the paths. A
resolution is the exact bytes of the merged file, or `"ours"` / `"theirs"`. The
result is a two-parent snapshot.

## Blame per line

```python
for line in repo.blame("essay.txt", at=merged).lines:
    print(line.origin.author.name, line.origin.branch, line.text)
```

The untouched lines are attributed to the original author on `main`, and the
resolved sentence to Grace on `draft-grace`, because the resolution matched
her line.
